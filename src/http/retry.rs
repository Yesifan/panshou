//! Cancellation-safe retries: dropping the future drops the active attempt or
//! timer. Callers classify protocol/status errors, and replay POST bodies by
//! rebuilding a request inside the attempt closure. No tasks are spawned.
use std::{future::Future, sync::Arc, time::Duration};

#[derive(Clone)]
pub struct RetryPolicy {
    /// Total attempts, including the first (zero normalizes to one).
    pub attempts: usize,
    pub base_delay: Duration,
    pub multiplier: f64,
    pub max_delay: Option<Duration>,
    pub jitter: bool,
    /// Receives the zero-based failed attempt, overriding all other delays.
    pub delay: Option<Arc<dyn Fn(usize) -> Duration + Send + Sync>>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 1,
            base_delay: Duration::from_millis(500),
            multiplier: 1.0,
            max_delay: None,
            jitter: false,
            delay: None,
        }
    }
}

impl RetryPolicy {
    pub fn fixed(attempts: usize, delay: Duration) -> Self {
        Self {
            attempts,
            base_delay: delay,
            ..Self::default()
        }
    }

    pub fn linear(attempts: usize, step: Duration) -> Self {
        Self {
            attempts,
            delay: Some(Arc::new(move |attempt| {
                step.saturating_mul((attempt + 1).min(u32::MAX as usize) as u32)
            })),
            ..Self::default()
        }
    }

    pub fn exponential(attempts: usize, base: Duration, multiplier: f64) -> Self {
        Self {
            attempts,
            base_delay: base,
            multiplier,
            ..Self::default()
        }
    }

    pub fn wait_after(&self, attempt: usize) -> Duration {
        if let Some(delay) = &self.delay {
            return delay(attempt);
        }
        let base = if self.base_delay.is_zero() {
            Duration::from_millis(500)
        } else {
            self.base_delay
        };
        let mut seconds = base.as_secs_f64();
        if self.multiplier > 1.0 {
            seconds *= self.multiplier.powf(attempt as f64);
        }
        if let Some(cap) = self.max_delay.filter(|cap| !cap.is_zero()) {
            seconds = seconds.min(cap.as_secs_f64());
        }
        if self.jitter {
            seconds *= rand::random_range(0.75..1.25);
        }
        Duration::try_from_secs_f64(seconds).unwrap_or(Duration::MAX)
    }
}

#[derive(Debug)]
pub enum RetryDecision<E> {
    Retry(E),
    Abort(E),
}

#[derive(Debug, thiserror::Error)]
pub enum RetryError<E> {
    #[error("retry aborted: {0}")]
    Aborted(#[source] E),
    #[error("request failed after {attempts} attempt(s): {source}")]
    Exhausted {
        attempts: usize,
        #[source]
        source: E,
    },
    #[error("retry wait interrupted: {0}")]
    Wait(#[source] E),
}

impl<E> RetryError<E> {
    pub fn into_inner(self) -> E {
        match self {
            Self::Aborted(error) | Self::Wait(error) | Self::Exhausted { source: error, .. } => {
                error
            }
        }
    }
}

pub async fn retry<T, E, F, Fut>(policy: &RetryPolicy, operation: F) -> Result<T, RetryError<E>>
where
    F: FnMut(usize) -> Fut,
    Fut: Future<Output = Result<T, RetryDecision<E>>>,
{
    retry_with(
        policy,
        operation,
        |_, _, _| {},
        |delay| async move {
            tokio::time::sleep(delay).await;
            Ok(())
        },
    )
    .await
}

/// Custom waits run even for a zero delay. Wait errors are returned immediately
/// and never misreported as retry exhaustion. Observers run only before a real
/// retry, after the failed operation (including its response body) is dropped.
pub async fn retry_with<T, E, F, Fut, O, W, WFut>(
    policy: &RetryPolicy,
    mut operation: F,
    mut on_retry: O,
    mut wait: W,
) -> Result<T, RetryError<E>>
where
    F: FnMut(usize) -> Fut,
    Fut: Future<Output = Result<T, RetryDecision<E>>>,
    O: FnMut(usize, Duration, &E),
    W: FnMut(Duration) -> WFut,
    WFut: Future<Output = Result<(), E>>,
{
    let attempts = policy.attempts.max(1);
    for attempt in 0..attempts {
        match operation(attempt).await {
            Ok(value) => return Ok(value),
            Err(RetryDecision::Abort(error)) => return Err(RetryError::Aborted(error)),
            Err(RetryDecision::Retry(error)) if attempt + 1 == attempts => {
                return Err(RetryError::Exhausted {
                    attempts,
                    source: error,
                });
            }
            Err(RetryDecision::Retry(error)) => {
                let delay = policy.wait_after(attempt);
                on_retry(attempt, delay, &error);
                drop(error);
                wait(delay).await.map_err(RetryError::Wait)?;
            }
        }
    }
    unreachable!("at least one attempt")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, io};

    #[test]
    fn delays_and_jitter() {
        let mut policy = RetryPolicy::exponential(4, Duration::from_millis(100), 2.0);
        policy.max_delay = Some(Duration::from_millis(250));
        assert_eq!(
            (0..3)
                .map(|attempt| policy.wait_after(attempt).as_millis())
                .collect::<Vec<_>>(),
            [100, 200, 250]
        );
        policy.jitter = true;
        for _ in 0..100 {
            assert!((75..125).contains(&policy.wait_after(0).as_millis()));
        }
        let policy = RetryPolicy::linear(3, Duration::from_millis(300));
        assert_eq!(policy.wait_after(1), Duration::from_millis(600));
        assert_eq!(
            RetryPolicy::fixed(2, Duration::ZERO).wait_after(0),
            Duration::from_millis(500)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn attempts_and_no_final_sleep() {
        let calls = Cell::new(0);
        let start = tokio::time::Instant::now();
        let result = retry(
            &RetryPolicy::linear(3, Duration::from_millis(100)),
            |attempt| {
                calls.set(calls.get() + 1);
                async move {
                    Err::<(), _>(RetryDecision::Retry(io::Error::other(format!(
                        "attempt {attempt}"
                    ))))
                }
            },
        )
        .await;
        assert_eq!(calls.get(), 3);
        assert_eq!(start.elapsed(), Duration::from_millis(300));
        assert_eq!(result.unwrap_err().into_inner().to_string(), "attempt 2");
    }

    #[tokio::test(start_paused = true)]
    async fn abort_and_success_do_not_wait() {
        let start = tokio::time::Instant::now();
        let policy = RetryPolicy::fixed(3, Duration::from_secs(5));
        let result = retry(&policy, |_| async {
            Err::<(), _>(RetryDecision::Abort(io::Error::other("stop")))
        })
        .await;
        assert!(matches!(result, Err(RetryError::Aborted(_))));
        assert_eq!(
            retry(&policy, |_| async { Ok::<_, RetryDecision<io::Error>>(42) })
                .await
                .unwrap(),
            42
        );
        assert_eq!(start.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_drops_wait_and_prevents_another_attempt() {
        let calls = Cell::new(0);
        let policy = RetryPolicy::fixed(3, Duration::from_secs(5));
        let future = retry(&policy, |_| {
            calls.set(calls.get() + 1);
            async { Err::<(), _>(RetryDecision::Retry(io::Error::other("transient"))) }
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(80), future)
                .await
                .is_err()
        );
        assert_eq!(calls.get(), 1);
    }

    #[tokio::test]
    async fn zero_delay_custom_wait_error_is_not_exhaustion() {
        let policy = RetryPolicy {
            attempts: 3,
            delay: Some(Arc::new(|_| Duration::ZERO)),
            ..RetryPolicy::default()
        };
        let observed = Cell::new(0);
        let result = retry_with(
            &policy,
            |_| async { Err::<(), _>(RetryDecision::Retry(io::Error::other("request"))) },
            |_, wait, _| {
                assert!(wait.is_zero());
                observed.set(observed.get() + 1);
            },
            |_| async { Err(io::Error::other("canceled")) },
        )
        .await;
        assert_eq!(observed.get(), 1);
        assert!(matches!(result, Err(RetryError::Wait(_))));
    }
}
