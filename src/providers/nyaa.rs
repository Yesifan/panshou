use crate::core::{Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use regex::Regex;
use scraper::{Html, Selector};
use std::{sync::LazyLock, time::Duration};
use url::Url;
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/view/(\d+)").unwrap());
#[derive(Debug, Clone)]
pub struct Nyaa {
    base: Url,
}
impl Default for Nyaa {
    fn default() -> Self {
        Self::new(Url::parse("https://nyaa.si/").unwrap())
    }
}
impl Nyaa {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn text(n: scraper::ElementRef<'_>) -> String {
    n.text().collect::<String>().trim().to_owned()
}
fn parse(body: &str, query: &str) -> Result<Vec<SearchResult>, ProviderError> {
    let doc = Html::parse_document(body);
    // Empty result pages have the search form and/or a torrent table. HTML error
    // documents are protocol failures rather than a fabricated successful empty.
    if doc
        .select(&sel(
            "table.torrent-list, form[action='/'], input[name='q']",
        ))
        .next()
        .is_none()
    {
        return Err(ProviderError::Parse("nyaa search document missing".into()));
    }
    let mut out = Vec::new();
    for row in doc.select(&sel("table.torrent-list tbody tr")) {
        let Some(anchor) = row
            .select(&sel("td[colspan='2'] a[href*='/view/']"))
            .find(|a| !a.value().classes().any(|c| c == "comments"))
        else {
            continue;
        };
        let Some(id) = anchor
            .value()
            .attr("href")
            .and_then(|h| ID.captures(h))
            .map(|c| c[1].to_owned())
        else {
            continue;
        };
        let mut title = text(anchor);
        if title.is_empty() {
            title = anchor.value().attr("title").unwrap_or("").to_owned();
        }
        let Some(magnet) = row
            .select(&sel("td.text-center a[href^='magnet:']"))
            .next()
            .and_then(|a| a.value().attr("href"))
        else {
            continue;
        };
        if !query
            .to_lowercase()
            .split_whitespace()
            .all(|w| title.to_lowercase().contains(w))
        {
            continue;
        }
        let mut result = SearchResult::provider(format!("nyaa-{id}"), "nyaa");
        result.title = title;
        let mut link = Link::new(magnet);
        link.work_title = Some(result.title.clone());
        result.links.push(link);
        result.datetime = row
            .select(&sel("td.text-center[data-timestamp]"))
            .next()
            .and_then(|n| n.value().attr("data-timestamp"))
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|t| *t > 0)
            .and_then(|t| DateTime::<Utc>::from_timestamp(t, 0));
        let category = row
            .select(&sel("td:nth-child(1) a"))
            .next()
            .and_then(|a| a.value().attr("title"))
            .unwrap_or("");
        let cells = row.select(&sel("td.text-center")).collect::<Vec<_>>();
        let size = cells.get(1).map(|n| text(*n)).unwrap_or_default();
        let stats = if cells.len() >= 6 {
            cells[cells.len() - 3..]
                .iter()
                .map(|n| text(*n))
                .collect::<Vec<_>>()
        } else {
            vec!["0".into(); 3]
        };
        let mut content = Vec::new();
        if !category.is_empty() {
            content.push(format!("分类: {category}"));
            result.tags.push(category.into());
        }
        if !size.is_empty() {
            content.push(format!("大小: {size}"));
        }
        for (label, value) in ["做种", "下载", "完成"].iter().zip(stats) {
            content.push(format!("{label}: {value}"));
            result.tags.push(format!("{label}:{value}"));
        }
        result.content = content.join(" | ");
        out.push(result);
    }
    Ok(out)
}
#[async_trait]
impl Provider for Nyaa {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("nyaa", 3, KeywordFilterMode::Provider)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        tokio::time::timeout(ctx.timeout, async {
            let body = crate::providers::b_registry::http::text(
                ctx.client
                    .get(self.base.clone())
                    .query(&[("f", "0"), ("c", "0_0"), ("q", query)])
                    .header(
                        "user-agent",
                        "Mozilla/5.0 Chrome/91.0.4472.124 Safari/537.36",
                    )
                    .header(
                        "accept",
                        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
                    )
                    .header("accept-language", "en-US,en;q=0.9,zh-CN;q=0.8,zh;q=0.7")
                    .header("referer", self.base.as_str()),
                3,
                200,
                true,
                Duration::from_secs(10),
                16 << 20,
            )
            .await?;
            parse(&body, query)
        })
        .await
        .map_err(|_| ProviderError::Timeout)?
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn success_empty_malformed() {
        let html = r#"<table class="torrent-list"><tbody><tr><td><a title="Anime"></a></td><td colspan="2"><a href="/view/123">Film 1080p</a></td><td class="text-center"><a href="magnet:?xt=urn:btih:0123456789012345678901234567890123456789">m</a></td><td class="text-center">1 GiB</td><td class="text-center" data-timestamp="1700000000"></td><td class="text-center">10</td><td class="text-center">2</td><td class="text-center">100</td></tr></tbody></table>"#;
        let r = parse(html, "film").unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, "nyaa-123");
        assert!(r[0].content.contains("做种: 10"));
        assert!(r[0].datetime.is_some());
        assert!(
            parse(
                "<table class='torrent-list'><tbody></tbody></table>",
                "film"
            )
            .unwrap()
            .is_empty()
        );
        assert!(parse("<html>bad gateway</html>", "film").is_err());
    }
    #[tokio::test]
    async fn status_retry() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403))
            .expect(3)
            .mount(&s)
            .await;
        let p = Nyaa::new(Url::parse(&s.uri()).unwrap());
        let c = SearchContext::new(reqwest::Client::new(), Duration::from_secs(3));
        assert!(
            p.search(&c, "film")
                .await
                .unwrap_err()
                .to_string()
                .contains("403")
        );
    }
}
