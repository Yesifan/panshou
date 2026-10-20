//! Request replay for the group-B adapters (all non-200 responses retry).
use crate::core::ProviderError;
use std::time::Duration;

pub(crate) async fn response(
    request: reqwest::RequestBuilder,
    attempts: usize,
    delay_ms: u64,
    exponential: bool,
    timeout: Duration,
) -> Result<reqwest::Response, ProviderError> {
    let mut last = ProviderError::Timeout;
    for attempt in 0..attempts {
        if attempt > 0 {
            let factor = if exponential {
                1u64 << (attempt - 1)
            } else {
                1
            };
            tokio::time::sleep(Duration::from_millis(delay_ms * factor)).await;
        }
        let replay = request
            .try_clone()
            .ok_or_else(|| ProviderError::Protocol("request body is not replayable".into()))?;
        match tokio::time::timeout(timeout, crate::http::execute(replay)).await {
            Ok(Ok(r)) if r.status().as_u16() == 200 => return Ok(r),
            Ok(Ok(r)) => last = ProviderError::Protocol(format!("HTTP {}", r.status())),
            Ok(Err(e)) => last = ProviderError::Network(e.to_string()),
            Err(_) => last = ProviderError::Timeout,
        }
    }
    Err(last)
}
pub(crate) async fn text(
    request: reqwest::RequestBuilder,
    attempts: usize,
    delay_ms: u64,
    exponential: bool,
    timeout: Duration,
    limit: usize,
) -> Result<String, ProviderError> {
    let mut last = ProviderError::Timeout;
    for attempt in 0..attempts {
        if attempt > 0 {
            let factor = if exponential {
                1u64 << (attempt - 1)
            } else {
                1
            };
            tokio::time::sleep(Duration::from_millis(delay_ms * factor)).await;
        }
        let replay = request
            .try_clone()
            .ok_or_else(|| ProviderError::Protocol("request body is not replayable".into()))?;
        let result = tokio::time::timeout(timeout, async {
            let r = crate::http::execute(replay)
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?;
            if r.status().as_u16() != 200 {
                return Err(ProviderError::Protocol(format!("HTTP {}", r.status())));
            }
            let b = crate::http::read_response(r, limit)
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?;
            Ok(String::from_utf8_lossy(&b).into_owned())
        })
        .await
        .unwrap_or(Err(ProviderError::Timeout));
        match result {
            Ok(s) => return Ok(s),
            Err(e) => last = e,
        }
    }
    Err(last)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_string, method},
    };
    #[tokio::test]
    async fn retries_replay_body_and_keep_status() {
        let server = MockServer::start().await;
        let count = Arc::new(AtomicUsize::new(0));
        let n = count.clone();
        Mock::given(method("POST"))
            .and(body_string("complete+/="))
            .respond_with(move |_: &wiremock::Request| {
                ResponseTemplate::new(if n.fetch_add(1, Ordering::SeqCst) < 2 {
                    500
                } else {
                    200
                })
                .set_body_string("ok")
            })
            .expect(3)
            .mount(&server)
            .await;
        let s = text(
            reqwest::Client::new()
                .post(server.uri())
                .body("complete+/="),
            3,
            1,
            true,
            Duration::from_secs(2),
            100,
        )
        .await
        .unwrap();
        assert_eq!(s, "ok");
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403))
            .expect(3)
            .mount(&server)
            .await;
        assert!(
            text(
                reqwest::Client::new().get(server.uri()),
                3,
                1,
                true,
                Duration::from_secs(2),
                100
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("403")
        );
    }
    #[tokio::test]
    async fn exact_limit_plus_one_and_cancel_wait() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("12345"))
            .mount(&server)
            .await;
        assert!(
            text(
                reqwest::Client::new().get(server.uri()),
                1,
                0,
                false,
                Duration::from_secs(1),
                4
            )
            .await
            .is_err()
        );
        assert_eq!(
            text(
                reqwest::Client::new().get(server.uri()),
                1,
                0,
                false,
                Duration::from_secs(1),
                5
            )
            .await
            .unwrap(),
            "12345"
        );
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        assert!(
            tokio::time::timeout(
                Duration::from_millis(30),
                text(
                    reqwest::Client::new().get(server.uri()),
                    3,
                    1000,
                    true,
                    Duration::from_secs(1),
                    5
                )
            )
            .await
            .is_err()
        );
    }
}
