//! JavDB movie search and magnet details. Default 429 budget is exactly one request.
use super::{UA, clean};
use crate::core::{Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use scraper::{Html, Selector};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use url::Url;
pub struct JavdbProvider {
    base: Url,
    limited: AtomicBool,
    count: AtomicUsize,
    cache: Mutex<HashMap<String, (Instant, Vec<Link>)>>,
}
impl Default for JavdbProvider {
    fn default() -> Self {
        Self::new(Url::parse("https://javdb.com/").unwrap())
    }
}
impl JavdbProvider {
    pub fn new(base: Url) -> Self {
        Self {
            base,
            limited: AtomicBool::new(false),
            count: AtomicUsize::new(0),
            cache: Mutex::default(),
        }
    }
    pub fn rate_limit_count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }
    pub fn rate_limited(&self) -> bool {
        self.limited.load(Ordering::Relaxed)
    }
    async fn response(
        &self,
        ctx: &SearchContext,
        url: Url,
    ) -> Result<reqwest::Response, ProviderError> {
        let r = crate::http::execute(
            ctx.client
                .get(url)
                .timeout(ctx.request_timeout(Duration::from_secs(30)))
                .header("User-Agent", UA)
                .header("Referer", self.base.as_str())
                .header(
                    "Accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
                ),
        )
        .await
        .map_err(|_| ProviderError::Network("javdb request: transport error".into()))?;
        if r.status().as_u16() == 429 {
            self.limited.store(true, Ordering::Relaxed);
            self.count.fetch_add(1, Ordering::Relaxed);
            return Err(ProviderError::Unavailable(
                "javdb HTTP 429 rate limited, no retry".into(),
            ));
        }
        Ok(r)
    }
    async fn body(&self, ctx: &SearchContext, url: Url) -> Result<String, ProviderError> {
        let r = self.response(ctx, url).await?;
        let status = r.status();
        if status.as_u16() != 200 {
            return Err(ProviderError::Protocol(format!("javdb HTTP {status}")));
        }
        let b = crate::http::read_response(r, 16 << 20)
            .await
            .map_err(|e| ProviderError::Protocol(format!("javdb HTTP {status}: {e}")))?;
        Ok(String::from_utf8_lossy(&b).into_owned())
    }
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn items(body: &str, base: &Url) -> Result<Vec<(Url, SearchResult)>, ProviderError> {
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    for node in doc.select(&sel(".movie-list .item")).take(50) {
        let Some(a) = node.select(&sel("a.box[href]")).next() else {
            continue;
        };
        let Some(url) = a.value().attr("href").and_then(|s| base.join(s).ok()) else {
            continue;
        };
        let title = node
            .select(&sel(".video-title"))
            .next()
            .map(|n| clean(&n.inner_html()))
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let mut r =
            SearchResult::provider(format!("javdb-{:x}", md5::compute(url.as_str())), "javdb");
        r.title = title;
        r.content = node
            .select(&sel(".score .value"))
            .next()
            .map(|n| format!("评分：{}", clean(&n.inner_html())))
            .unwrap_or_default();
        r.tags = node
            .select(&sel(".tags .tag"))
            .map(|n| clean(&n.inner_html()))
            .collect();
        r.datetime = node
            .select(&sel(".meta"))
            .next()
            .and_then(|n| {
                chrono::NaiveDate::parse_from_str(&clean(&n.inner_html()), "%Y-%m-%d").ok()
            })
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| d.and_utc());
        if let Some(u) = node
            .select(&sel("img[src]"))
            .next()
            .and_then(|n| n.value().attr("src"))
            .and_then(|s| base.join(s).ok())
        {
            r.images.push(u.to_string());
        }
        out.push((url, r));
    }
    if out.is_empty() && doc.select(&sel(".movie-list")).next().is_none() {
        return Err(ProviderError::Parse(
            "javdb search: missing movie-list".into(),
        ));
    }
    Ok(out)
}
fn magnets(body: &str) -> Vec<Link> {
    let doc = Html::parse_document(body);
    let mut seen = HashSet::new();
    doc.select(&sel(".magnet-links .item .magnet-name a[href]"))
        .filter_map(|a| {
            let raw = a.value().attr("href")?.replace("&amp;", "&");
            (raw.starts_with("magnet:") && seen.insert(raw.clone())).then(|| Link::new(raw))
        })
        .collect()
}
#[async_trait]
impl Provider for JavdbProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("javdb", 5, KeywordFilterMode::Provider)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        q: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        self.limited.store(false, Ordering::Relaxed);
        let mut url = self.base.join("search").unwrap();
        url.query_pairs_mut()
            .append_pair("q", q)
            .append_pair("f", "all");
        let entries = items(&self.body(ctx, url).await?, &self.base)?;
        let failures = &std::sync::Mutex::new(None);
        let rows = stream::iter(entries.into_iter().map(|(url, mut r)| async move {
            let cached = self
                .cache
                .lock()
                .unwrap()
                .get(url.as_str())
                .filter(|(at, _)| at.elapsed() < Duration::from_secs(1800))
                .map(|(_, l)| l.clone());
            r.links = if let Some(links) = cached {
                links
            } else {
                let links = magnets(&super::remember_failure(
                    self.body(ctx, url.clone()).await,
                    failures,
                )?);
                if links.is_empty() {
                    return None;
                }
                let mut cache = self.cache.lock().unwrap();
                if cache.len() >= 512
                    && let Some(k) = cache
                        .iter()
                        .min_by_key(|(_, v)| v.0)
                        .map(|(k, _)| k.clone())
                {
                    cache.remove(&k);
                }
                cache.insert(url.to_string(), (Instant::now(), links.clone()));
                links
            };
            ctx.publish(vec![r.clone()]);
            Some(r)
        }))
        .buffered(10)
        .collect::<Vec<_>>()
        .await;
        if let Some(error) = failures.lock().unwrap().take() {
            return Err(error);
        }
        if rows.iter().any(Option::is_none) {
            return Err(ProviderError::Unavailable(
                "javdb: one or more details failed".into(),
            ));
        }
        let rows = rows.into_iter().flatten().collect::<Vec<_>>();
        if self.rate_limited() {
            return Err(ProviderError::Unavailable(
                "javdb HTTP 429 rate limited, partial details".into(),
            ));
        }
        Ok(rows)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixtures() {
        let base = Url::parse("https://javdb.com/").unwrap();
        assert_eq!(
            items(
                include_str!("../../tests/fixtures/providers/javdb/success.html"),
                &base
            )
            .unwrap()
            .len(),
            1
        );
        assert_eq!(
            magnets(include_str!(
                "../../tests/fixtures/providers/javdb/detail.html"
            ))
            .len(),
            1
        );
        assert!(
            items(
                include_str!("../../tests/fixtures/providers/javdb/empty.html"),
                &base
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            items(
                include_str!("../../tests/fixtures/providers/javdb/malformed.html"),
                &base
            )
            .is_err()
        );
    }
    #[tokio::test]
    async fn rate_limit_contract() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        let server = MockServer::start().await;
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(2));
        let p = JavdbProvider::new(Url::parse(&server.uri()).unwrap());
        Mock::given(path("/500"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            p.response(&ctx, p.base.join("500").unwrap())
                .await
                .unwrap()
                .status()
                .as_u16(),
            500
        );
        assert!(!p.rate_limited());
        Mock::given(path("/429"))
            .respond_with(ResponseTemplate::new(429))
            .expect(1)
            .mount(&server)
            .await;
        assert!(
            p.response(&ctx, p.base.join("429").unwrap())
                .await
                .unwrap_err()
                .to_string()
                .contains("429")
        );
        assert_eq!(p.rate_limit_count(), 1);
        assert!(p.rate_limited());
    }
}
