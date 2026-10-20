//! HTTP challenge providers. Use `providers_with_options` when registering
//! command-scoped instances so explicit CLI proxy settings reach private jars.
use std::{future::Future, sync::Arc, time::Duration};

use crate::{
    core::ProviderError,
    http::{ClientOptions, HttpClientFactory, Session},
    providers::{Provider, SearchContext},
};

#[path = "leso.rs"]
pub mod leso;
#[path = "libvio.rs"]
pub mod libvio;
#[path = "nsgame.rs"]
pub mod nsgame;

pub fn providers() -> Vec<Arc<dyn Provider>> {
    providers_with_options(&ClientOptions::default())
}

pub fn providers_with_options(options: &ClientOptions) -> Vec<Arc<dyn Provider>> {
    vec![
        Arc::new(libvio::Libvio::with_options(
            "https://libvio.host/".parse().unwrap(),
            options.clone(),
        )),
        Arc::new(leso::Leso::with_options(
            "https://www.leso.cc/".parse().unwrap(),
            options.clone(),
        )),
        Arc::new(nsgame::Nsgame::with_options(
            "https://nsthwj.cn/thwj/".parse().unwrap(),
            options.clone(),
        )),
    ]
}

pub(crate) const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";

pub(crate) fn protocol(message: impl Into<String>) -> ProviderError {
    ProviderError::Protocol(message.into())
}
pub(crate) fn network(error: anyhow::Error) -> ProviderError {
    if error
        .downcast_ref::<reqwest::Error>()
        .is_some_and(|e| e.is_timeout())
    {
        ProviderError::Timeout
    } else {
        ProviderError::Network(error.to_string())
    }
}
pub(crate) fn check(ctx: &SearchContext) -> Result<(), ProviderError> {
    if ctx.is_cancelled() {
        Err(ProviderError::Timeout)
    } else {
        Ok(())
    }
}
pub(crate) async fn bounded<T>(
    ctx: &SearchContext,
    future: impl Future<Output = Result<T, ProviderError>>,
) -> Result<T, ProviderError> {
    check(ctx)?;
    let result = tokio::time::timeout(ctx.remaining(), future)
        .await
        .map_err(|_| ProviderError::Timeout)?;
    check(ctx)?;
    result
}
pub(crate) async fn send(
    ctx: &SearchContext,
    request: reqwest::RequestBuilder,
    maximum: Duration,
) -> Result<reqwest::Response, ProviderError> {
    check(ctx)?;
    let timeout = ctx.request_timeout(maximum);
    tokio::time::timeout(timeout, crate::http::execute(request.timeout(timeout)))
        .await
        .map_err(|_| ProviderError::Timeout)?
        .map_err(network)
}
pub(crate) async fn body(
    response: reqwest::Response,
    limit: usize,
) -> Result<String, ProviderError> {
    let bytes = crate::http::read_response(response, limit)
        .await
        .map_err(network)?;
    String::from_utf8(bytes).map_err(|_| ProviderError::Parse("response is not UTF-8".into()))
}
pub(crate) fn session(options: &ClientOptions) -> Result<Session, ProviderError> {
    HttpClientFactory::new()
        .session(options)
        .map_err(|e| protocol(e.to_string()))
}
/// Prehash untrusted challenge prefixes cooperatively. Cloning this SHA-256
/// state per nonce avoids repeatedly hashing a potentially large server token.
pub(crate) async fn hash_prefix(
    ctx: &SearchContext,
    prefix: &[u8],
    deadline: tokio::time::Instant,
) -> Result<sha2::Sha256, ProviderError> {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    for chunk in prefix.chunks(8192) {
        check(ctx)?;
        if tokio::time::Instant::now() >= deadline {
            return Err(ProviderError::Timeout);
        }
        tokio::task::yield_now().await;
        hasher.update(chunk);
    }
    Ok(hasher)
}

pub(crate) fn text(element: scraper::ElementRef<'_>) -> String {
    element
        .text()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
pub(crate) fn selector(value: &str) -> scraper::Selector {
    scraper::Selector::parse(value).expect("static provider selector")
}

#[cfg(test)]
pub(crate) async fn assert_failed_partial(
    provider: Arc<dyn Provider>,
    query: &str,
) -> Vec<crate::core::SearchResult> {
    let engine = crate::search::SearchEngine::new(context());
    let options = crate::search::SearchOptions {
        query: query.into(),
        providers: vec![provider],
        timeout: Duration::from_secs(5),
        all_timeout: Duration::from_secs(10),
        ..Default::default()
    };
    let (sender, _events) = tokio::sync::mpsc::unbounded_channel();
    let (_cancel, cancellation) = tokio::sync::watch::channel(false);
    let (outcome, summary) = engine.search_stream(options, sender, cancellation).await;
    assert_eq!(summary.sources_completed, 0);
    assert_eq!(summary.sources_failed, 1);
    assert!(summary.partial);
    assert_eq!(summary.exit_code(), 3);
    assert_eq!(outcome.source_errors.len(), 1);
    assert!(!outcome.results.is_empty());
    outcome.results
}

#[cfg(test)]
pub(crate) fn context() -> SearchContext {
    let options = ClientOptions::default();
    SearchContext::new(
        HttpClientFactory::new().client(&options).unwrap(),
        Duration::from_secs(5),
    )
}
