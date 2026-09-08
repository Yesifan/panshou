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
    let hash = Regex::new(r"(?i)/hash/([a-f0-9]{40})\.html").expect("regex");
    let date = Regex::new(r"添加时间[:：]\s*(\d{4}-\d{2}-\d{2})").expect("regex");
    let ads = Regex::new(r"【[^】]*】").expect("regex");
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
                    .send()
                    .await
                    .map_err(|error| ProviderError::Network(error.to_string()))?;
                if !r.status().is_success() {
                    return Err(ProviderError::Protocol(format!("HTTP {}", r.status())));
                }
                let body = r
                    .text()
                    .await
                    .map_err(|error| ProviderError::Network(error.to_string()))?;
                parse_page(&body)
            }
        });
        let pages = tokio::time::timeout(ctx.timeout, join_all(jobs))
            .await
            .map_err(|_| ProviderError::Timeout)?;
        let mut out = Vec::new();
        let mut successes = 0usize;
        let mut first_error = None;
        for page in pages {
            match page {
                Ok(page) => {
                    successes += 1;
                    out.extend(page);
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            };
        }
        if successes == 0 {
            return Err(first_error.unwrap_or_else(|| {
                ProviderError::Unavailable("all cldi search pages failed".into())
            }));
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
