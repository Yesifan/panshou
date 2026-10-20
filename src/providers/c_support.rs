//! Bounded request primitives shared by the C-group site protocols.
use crate::core::ProviderError;
use std::time::Duration;

pub(crate) const BODY_LIMIT: usize = 16 * 1024 * 1024;

#[cfg(test)]
#[path = "c_support_tests.rs"]
mod tests;

/// Total attempts includes the first request. Dropping this future cancels both
/// transport/body consumption and backoff; no task outlives the caller.
pub(crate) async fn request(
    make: impl Fn() -> reqwest::RequestBuilder,
    attempts: usize,
    delay: Duration,
    limit: usize,
) -> Result<Vec<u8>, ProviderError> {
    let mut last = ProviderError::Protocol("no request attempts".into());
    for attempt in 0..attempts {
        let result = async {
            let response = crate::http::execute(make()).await.map_err(network)?;
            if response.status() != reqwest::StatusCode::OK {
                return Err(ProviderError::Protocol(format!(
                    "HTTP {}",
                    response.status().as_u16()
                )));
            }
            Ok(response)
        }
        .await;
        match result {
            Ok(response) => {
                return crate::http::read_response(response, limit)
                    .await
                    .map_err(|e| ProviderError::Network(e.to_string()));
            }
            Err(error) => last = error,
        }
        if attempt + 1 < attempts {
            tokio::time::sleep(delay * (1 << attempt)).await;
        }
    }
    Err(last)
}

/// Publish successful siblings immediately, but never report an incomplete
/// source as completed. Keep request/status errors rather than erasing them.
pub(crate) async fn collect_details<I, F>(
    ctx: &super::SearchContext,
    jobs: I,
    concurrency: usize,
) -> Result<Vec<crate::core::SearchResult>, ProviderError>
where
    I: IntoIterator<Item = F>,
    F: std::future::Future<Output = Result<crate::core::SearchResult, ProviderError>>,
{
    use futures::StreamExt;
    let jobs: Vec<_> = jobs.into_iter().enumerate().collect();
    let mut pending = futures::stream::iter(jobs.into_iter().map(|(index, job)| async move {
        if ctx.is_cancelled() {
            return (index, Err(ProviderError::Timeout));
        }
        let result = job.await.and_then(|row| {
            if row.links.is_empty() {
                return Err(ProviderError::Parse(
                    "detail contained no usable links".into(),
                ));
            }
            if !ctx.publish(vec![row.clone()]) {
                return Err(ProviderError::Timeout);
            }
            Ok(row)
        });
        (index, result)
    }))
    .buffer_unordered(concurrency);
    let mut rows = Vec::new();
    let mut first = None;
    while let Some((index, result)) = pending.next().await {
        match result {
            Ok(row) => rows.push((index, row)),
            Err(error) => {
                first.get_or_insert(error);
            }
        }
    }
    if ctx.is_cancelled() {
        return Err(ProviderError::Timeout);
    }
    if let Some(error) = first {
        return Err(error);
    }
    rows.sort_by_key(|(index, _)| *index);
    Ok(rows.into_iter().map(|(_, row)| row).collect())
}

pub(crate) fn complete(
    ctx: &super::SearchContext,
    result: Result<Vec<crate::core::SearchResult>, ProviderError>,
) -> Result<Vec<crate::core::SearchResult>, ProviderError> {
    if ctx.is_cancelled() {
        Err(ProviderError::Timeout)
    } else {
        result
    }
}

pub(crate) fn network(error: impl Into<anyhow::Error>) -> ProviderError {
    let error = error.into();
    match error.downcast::<reqwest::Error>() {
        Ok(e) if e.is_timeout() => ProviderError::Timeout,
        Ok(e) => ProviderError::Network(e.without_url().to_string()),
        Err(e) => ProviderError::Network(e.to_string()),
    }
}

pub(crate) fn date(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|x| x.with_timezone(&Utc))
        .or_else(|| {
            ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"]
                .iter()
                .find_map(|f| NaiveDateTime::parse_from_str(raw, f).ok())
                .map(|x| x.and_utc())
        })
        .or_else(|| {
            NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)
                .map(|x| x.and_utc())
        })
}

/// net/url.QueryEscape-compatible encoding for opaque values embedded in JSON.
/// Unlike HTML form encoders, Go preserves '~' and escapes '*'.
pub(crate) fn query_escape(raw: &str) -> String {
    raw.as_bytes()
        .iter()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (*b as char).to_string()
            }
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub(crate) fn password(raw: &str) -> Option<String> {
    url::Url::parse(raw).ok()?.query_pairs().find_map(|(k, v)| {
        (["pwd", "password", "passcode", "code"].contains(&k.to_ascii_lowercase().as_str())
            && !v.trim().is_empty())
        .then(|| v.trim().to_owned())
    })
}
