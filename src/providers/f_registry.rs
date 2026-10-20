//! Protocol-specific adapters from the fixed October upstream snapshot.
use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use std::sync::Arc;
#[path = "quarktv.rs"]
pub mod quarktv;
#[path = "qupanshe.rs"]
pub mod qupanshe;
#[path = "sdso.rs"]
pub mod sdso;
#[path = "shandian.rs"]
pub mod shandian;
#[path = "sousou.rs"]
pub mod sousou;
#[path = "thepiratebay.rs"]
pub mod thepiratebay;
#[path = "wuji.rs"]
pub mod wuji;
#[path = "xb6v.rs"]
pub mod xb6v;
pub fn providers() -> Vec<Arc<dyn Provider>> {
    providers_with_options(&crate::http::ClientOptions::default())
}
pub fn providers_with_options(options: &crate::http::ClientOptions) -> Vec<Arc<dyn Provider>> {
    vec![
        Arc::new(quarktv::Quarktv::default()),
        Arc::new(qupanshe::Qupanshe::with_options(
            "https://www.qupanshe.com/".parse().unwrap(),
            options.clone(),
        )),
        Arc::new(sdso::Sdso::default()),
        Arc::new(shandian::Shandian::default()),
        Arc::new(sousou::Sousou::default()),
        Arc::new(thepiratebay::Thepiratebay::default()),
        Arc::new(wuji::Wuji::default()),
        Arc::new(xb6v::Xb6v::with_options(
            "https://www.66ss.org/".parse().unwrap(),
            options.clone(),
        )),
    ]
}
mod support {
    use super::SearchContext;
    use crate::core::{ProviderError, SearchResult};
    use futures::{Stream, StreamExt};
    use regex::Regex;
    use scraper::{ElementRef, Html, Selector};
    use std::{sync::LazyLock, time::Duration};
    pub const LIMIT: usize = 16 * 1024 * 1024;
    pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/124.0.0.0 Safari/537.36";
    pub(crate) use crate::providers::c_support::{date, password};
    #[derive(Clone, Copy)]
    pub struct Retry {
        pub attempts: usize,
        pub delay_ms: u64,
        pub status: bool,
        pub body: bool,
        pub per_attempt: bool,
    }
    pub const ONCE: Retry = Retry {
        attempts: 1,
        delay_ms: 0,
        status: false,
        body: false,
        per_attempt: false,
    };
    pub const STANDARD: Retry = Retry {
        attempts: 3,
        delay_ms: 200,
        status: true,
        body: false,
        per_attempt: false,
    };
    pub const QUARKTV: Retry = Retry {
        body: true,
        per_attempt: true,
        ..STANDARD
    };
    pub const WUJI: Retry = Retry {
        attempts: 3,
        delay_ms: 1000,
        status: false,
        body: false,
        per_attempt: false,
    };
    pub fn network(error: impl Into<anyhow::Error>) -> ProviderError {
        crate::providers::c_support::network(error)
    }
    pub async fn body(
        ctx: &SearchContext,
        make: impl Fn() -> reqwest::RequestBuilder,
        seconds: u64,
        retry: Retry,
    ) -> Result<Vec<u8>, ProviderError> {
        let run = async {
            let mut last = ProviderError::Timeout;
            for attempt in 0..retry.attempts {
                if ctx.is_cancelled() {
                    return Err(ProviderError::Timeout);
                }
                if attempt > 0 {
                    let delay = if retry.delay_ms == 1000 {
                        1000 * attempt as u64
                    } else {
                        retry.delay_ms * (1 << (attempt - 1))
                    };
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                if ctx.is_cancelled() {
                    return Err(ProviderError::Timeout);
                }
                let request = async {
                    let response = crate::http::execute(
                        make()
                            .header("user-agent", UA)
                            .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8"),
                    )
                    .await
                    .map_err(network)?;
                    if response.status().as_u16() != 200 {
                        let error =
                            ProviderError::Protocol(format!("HTTP {}", response.status().as_u16()));
                        return Ok(Err((error, retry.status)));
                    }
                    Ok::<_, ProviderError>(
                        match crate::http::read_response(response, LIMIT).await {
                            Ok(body) => Ok(body),
                            Err(error) => Err((network(error), retry.body)),
                        },
                    )
                };
                let outcome = if retry.per_attempt {
                    tokio::time::timeout(ctx.request_timeout(Duration::from_secs(seconds)), request)
                        .await
                        .map_err(|_| ProviderError::Timeout)
                        .and_then(|r| r)
                } else {
                    request.await
                };
                match outcome {
                    Ok(Ok(body)) => return Ok(body),
                    Ok(Err((error, false))) => return Err(error),
                    Ok(Err((error, true))) | Err(error) => last = error,
                }
            }
            Err(last)
        };
        if retry.per_attempt {
            run.await
        } else {
            tokio::time::timeout(ctx.request_timeout(Duration::from_secs(seconds)), run)
                .await
                .map_err(|_| ProviderError::Timeout)?
        }
    }
    pub fn sel(s: &str) -> Selector {
        Selector::parse(s).expect("static selector")
    }
    pub fn clean(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }
    pub fn text(node: ElementRef<'_>, s: &str) -> String {
        clean(
            &node
                .select(&sel(s))
                .flat_map(|x| x.text())
                .collect::<String>(),
        )
    }
    pub fn attr(node: ElementRef<'_>, s: &str, a: &str) -> Option<String> {
        node.select(&sel(s))
            .next()?
            .value()
            .attr(a)
            .map(str::to_owned)
    }
    pub fn html(body: &[u8]) -> Result<Html, ProviderError> {
        let s = String::from_utf8_lossy(body);
        if !s.contains('<') {
            return Err(ProviderError::Parse("expected HTML document".into()));
        }
        Ok(Html::parse_document(&s))
    }
    pub fn id(url: &str) -> Option<String> {
        static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/(\d+)\.html?$").unwrap());
        ID.captures(url)?.get(1).map(|m| m.as_str().into())
    }
    pub fn magnet(raw: &str) -> bool {
        static RE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(?i)^magnet:\?xt=urn:btih:[a-f0-9]{40}(?:&[^\s]*)?$").unwrap()
        });
        RE.is_match(raw)
    }
    /// Every fully parsed batch is published before waiting for slower work.
    /// Any failed task remains a failed source, even when its partial rows survive.
    pub async fn collect(
        ctx: &SearchContext,
        jobs: impl Stream<Item = Result<Vec<SearchResult>, ProviderError>>,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        futures::pin_mut!(jobs);
        let mut rows = vec![];
        let mut error = None;
        while let Some(batch) = jobs.next().await {
            match batch {
                Ok(batch) => {
                    if ctx.publish(batch.clone()) {
                        rows.extend(batch);
                    } else {
                        error.get_or_insert(ProviderError::Timeout);
                    }
                }
                Err(e) => {
                    if error.is_none() {
                        error = Some(e);
                    }
                }
            }
        }
        if ctx.is_cancelled() {
            return Err(ProviderError::Timeout);
        }
        match error {
            Some(e) => Err(e),
            None => Ok(rows),
        }
    }
}
#[cfg(test)]
#[path = "f_tests.rs"]
mod tests;
