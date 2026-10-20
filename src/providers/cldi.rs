trait ExecuteRequest {
    async fn execute(self) -> anyhow::Result<reqwest::Response>;
}
impl ExecuteRequest for reqwest::RequestBuilder {
    async fn execute(self) -> anyhow::Result<reqwest::Response> {
        crate::http::execute(self).await
    }
}

trait RetrySend {
    async fn send_retry(self) -> anyhow::Result<reqwest::Response>;
}
impl RetrySend for reqwest::RequestBuilder {
    async fn send_retry(self) -> anyhow::Result<reqwest::Response> {
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(200 << (attempt - 1))).await;
            }
            let request = self
                .try_clone()
                .expect("provider request body is replayable");
            let result = request.execute().await.and_then(|r| {
                if r.status().as_u16() == 200 {
                    Ok(r)
                } else {
                    Err(anyhow::anyhow!("HTTP {}", r.status()))
                }
            });
            match result {
                Ok(response) => return Ok(response),
                Err(error) if attempt == 2 => return Err(error),
                Err(_) => {}
            }
        }
        unreachable!("three attempts return a response or error")
    }
}

async fn read_text(response: reqwest::Response) -> anyhow::Result<String> {
    let status = response.status();
    let body = crate::http::read_response(response, 16 * 1024 * 1024)
        .await
        .map_err(|e| anyhow::anyhow!("cldi response body HTTP {status}: {e}"))?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

use crate::core::{Link, ProviderError, SearchResult, Source};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use chrono::NaiveDate;
use futures::future::join_all;
use regex::Regex;
use scraper::{Html, Selector};
use url::Url;
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://cm7jll1f.1122137.xyz/").expect("static URL"),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct CldiProvider {
    endpoints: Endpoints,
}
impl CldiProvider {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
fn parse_page(body: &str) -> Result<Vec<SearchResult>, ProviderError> {
    let doc = Html::parse_document(body);
    let card =
        Selector::parse("article.resource").map_err(|e| ProviderError::Parse(e.to_string()))?;
    let anchor = Selector::parse("h2 a[href]").expect("selector");
    let meta = Selector::parse(".meta").expect("selector");
    static HASH_RE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"(?i)/hash/([a-f0-9]{40})\.html").expect("regex"));
    let hash = &*HASH_RE;
    static DATE_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"添加时间[:：]\s*(\d{4}-\d{2}-\d{2})").expect("regex")
    });
    let date = &*DATE_RE;
    static ADS_RE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"【[^】]*】").expect("regex"));
    let ads = &*ADS_RE;
    let mut out = Vec::new();
    for article in doc.select(&card) {
        let Some(a) = article.select(&anchor).next() else {
            continue;
        };
        let Some(cap) = a.value().attr("href").and_then(|h| hash.captures(h)) else {
            continue;
        };
        let Some(btih) = cap.get(1).map(|m| m.as_str().to_ascii_lowercase()) else {
            continue;
        };
        let title = ads
            .replace_all(&a.text().collect::<String>(), "")
            .trim()
            .to_owned();
        if title.is_empty() {
            continue;
        }
        let content = article
            .select(&meta)
            .next()
            .map(|n| n.text().collect::<String>())
            .unwrap_or_default();
        let datetime = date
            .captures(&content)
            .and_then(|c| NaiveDate::parse_from_str(c.get(1)?.as_str(), "%Y-%m-%d").ok())
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| d.and_utc());
        let mut link = Link::new(format!("magnet:?xt=urn:btih:{btih}"));
        link.work_title = Some(title.clone());
        out.push(SearchResult {
            id: format!("cldi-{btih}"),
            source: Source::provider("cldi"),
            datetime,
            title,
            content: content.trim().to_owned(),
            links: vec![link],
            tags: vec![],
            images: vec![],
        });
    }
    if !out.is_empty() {
        return Ok(out);
    }
    let legacy = Selector::parse(".tbox .ssbox").expect("selector");
    let title_sel = Selector::parse(".title h3 a").expect("selector");
    let magnet = Selector::parse("a[href^='magnet:']").expect("selector");
    for (i, node) in doc.select(&legacy).enumerate() {
        let title = node
            .select(&title_sel)
            .next()
            .map(|n| {
                ads.replace_all(&n.text().collect::<String>(), "")
                    .trim()
                    .to_owned()
            })
            .unwrap_or_default();
        let Some(raw) = node
            .select(&magnet)
            .next()
            .and_then(|n| n.value().attr("href"))
        else {
            continue;
        };
        if title.is_empty() {
            continue;
        }
        out.push(SearchResult {
            id: format!("cldi-legacy-{i}"),
            source: Source::provider("cldi"),
            datetime: None,
            title,
            content: node.text().collect::<Vec<_>>().join(" "),
            links: vec![Link::new(raw)],
            tags: vec![],
            images: vec![],
        });
    }
    Ok(out)
}
#[async_trait]
impl Provider for CldiProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("cldi", 3, KeywordFilterMode::Provider)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let encoded =
            percent_encoding::utf8_percent_encode(query, percent_encoding::NON_ALPHANUMERIC)
                .to_string();
        let jobs = (1..=5).map(|page| {
            let encoded = encoded.clone();
            async move {
                let url = self
                    .endpoints
                    .base
                    .join(&format!("search-{encoded}-0-2-{page}.html"))
                    .map_err(|error| ProviderError::Protocol(error.to_string()))?;
                let r = ctx
                    .client
                    .get(url)
                    .header(reqwest::header::USER_AGENT, "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
                    .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8")
                    .header(reqwest::header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
                    .header(reqwest::header::CONNECTION, "keep-alive")
                    .header(reqwest::header::CACHE_CONTROL, "no-cache")
                    .header(reqwest::header::PRAGMA, "no-cache")
                    .header(reqwest::header::REFERER, self.endpoints.base.as_str())
                    .send_retry()
                    .await
                    .map_err(|error| ProviderError::Network(error.to_string()))?;
                if !r.status().is_success() {
                    return Err(ProviderError::Protocol(format!("HTTP {}", r.status())));
                }
                let body = read_text(r).await
                    .map_err(|error| ProviderError::Network(error.to_string()))?;
                let rows = parse_page(&body)?;
                ctx.publish(rows.clone());
                Ok(rows)
            }
        });
        let pages = tokio::time::timeout(ctx.timeout, join_all(jobs))
            .await
            .map_err(|_| ProviderError::Timeout)?;
        let mut out = Vec::new();
        let mut first_error = None;
        for page in pages {
            match page {
                Ok(page) => {
                    out.extend(page);
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            };
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(out)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn success() {
        let r = parse_page(include_str!(
            "../../tests/fixtures/providers/cldi/success.html"
        ))
        .unwrap();
        assert_eq!(r.len(), 1);
        assert!(
            r[0].links[0]
                .url
                .ends_with("0123456789abcdef0123456789abcdef01234567")
        );
    }
    #[test]
    fn empty() {
        assert!(
            parse_page(include_str!(
                "../../tests/fixtures/providers/cldi/empty.html"
            ))
            .unwrap()
            .is_empty()
        );
    }
    #[test]
    fn malformed() {
        assert!(
            parse_page(include_str!(
                "../../tests/fixtures/providers/cldi/malformed.html"
            ))
            .unwrap()
            .is_empty()
        );
    }
}
