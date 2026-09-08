use crate::core::{Link, ProviderError, SearchResult, Source};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use futures::future::join_all;
use scraper::{Html, Selector};
use url::Url;

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";

fn request(client: &reqwest::Client, url: Url, referer: &Url) -> reqwest::RequestBuilder {
    client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(
            reqwest::header::ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
        )
        .header(reqwest::header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
        .header(reqwest::header::CONNECTION, "keep-alive")
        .header("Upgrade-Insecure-Requests", "1")
        .header(reqwest::header::CACHE_CONTROL, "max-age=0")
        .header(reqwest::header::REFERER, referer.as_str())
}

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://www.hdmoli.com/").expect("static URL"),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct HdmoliProvider {
    endpoints: Endpoints,
}
impl HdmoliProvider {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
#[derive(Debug)]
struct SearchItem {
    id: String,
    title: String,
    content: String,
    tags: Vec<String>,
    detail: Url,
}

fn parse_search(body: &str, base: &Url) -> Result<Vec<SearchItem>, ProviderError> {
    let doc = Html::parse_document(body);
    let items = Selector::parse("#searchList > li.active.clearfix")
        .map_err(|e| ProviderError::Parse(e.to_string()))?;
    let title_sel = Selector::parse(".detail h4.title a").expect("selector");
    let desc = Selector::parse(".detail .desc, .detail .detail-sketch").expect("selector");
    let mut out = Vec::new();
    for item in doc.select(&items).take(50) {
        let Some(a) = item.select(&title_sel).next() else {
            continue;
        };
        let title = clean(&a.text().collect::<String>());
        let Some(href) = a.value().attr("href") else {
            continue;
        };
        if title.is_empty() {
            continue;
        }
        let Ok(detail) = base.join(href) else {
            continue;
        };
        let path = detail.path().trim_matches('/');
        let content = item
            .select(&desc)
            .next()
            .map(|n| clean(&n.text().collect::<String>()))
            .unwrap_or_default();
        out.push(SearchItem {
            id: format!("hdmoli-{path}"),
            title,
            content,
            tags: vec![],
            detail,
        });
    }
    Ok(out)
}
fn parse_detail(body: &str) -> Result<Vec<Link>, ProviderError> {
    let doc = Html::parse_document(body);
    let selector = Selector::parse(".downlist a[href], a[href]")
        .map_err(|e| ProviderError::Parse(e.to_string()))?;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for a in doc.select(&selector) {
        let Some(raw) = a.value().attr("href").map(str::trim) else {
            continue;
        };
        let mut link = Link::new(raw);
        if matches!(link.cloud_type, crate::core::CloudType::Others) || !seen.insert(raw.to_owned())
        {
            continue;
        }
        link.password = Url::parse(raw).ok().and_then(|u| {
            u.query_pairs()
                .find(|(k, _)| k == "pwd")
                .map(|(_, v)| v.into_owned())
        });
        out.push(link)
    }
    Ok(out)
}
fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[async_trait]
impl Provider for HdmoliProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("hdmoli", 2, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self
            .endpoints
            .base
            .join("search.php")
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("searchkey", query)
            .append_pair("submit", "");
        let response = tokio::time::timeout(
            ctx.timeout,
            request(&ctx.client, url, &self.endpoints.base).send(),
        )
        .await
        .map_err(|_| ProviderError::Timeout)?
        .map_err(|e| ProviderError::Network(e.to_string()))?;
        if !response.status().is_success() {
            return Err(ProviderError::Protocol(format!(
                "HTTP {}",
                response.status()
            )));
        }
        let rows = parse_search(
            &response
                .text()
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?,
            &self.endpoints.base,
        )?;
        let jobs = rows.into_iter().map(|row| async move {
            let response = request(&ctx.client, row.detail, &self.endpoints.base)
                .send()
                .await
                .ok()?;
            if !response.status().is_success() {
                return None;
            }
            let links = parse_detail(&response.text().await.ok()?).ok()?;
            if links.is_empty() {
                return None;
            }
            Some(SearchResult {
                id: row.id,
                source: Source::provider("hdmoli"),
                datetime: None,
                title: row.title,
                content: row.content,
                links,
                tags: row.tags,
                images: vec![],
            })
        });
        Ok(join_all(jobs).await.into_iter().flatten().collect())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn success() {
        let b = Url::parse("https://www.hdmoli.com/").unwrap();
        let r = parse_search(
            include_str!("../../tests/fixtures/providers/hdmoli/success.html"),
            &b,
        )
        .unwrap();
        let l = parse_detail(include_str!(
            "../../tests/fixtures/providers/hdmoli/detail.html"
        ))
        .unwrap();
        assert_eq!(r[0].title, "仙逆");
        assert_eq!(l.len(), 2);
    }
    #[test]
    fn empty() {
        let b = Url::parse("https://x/").unwrap();
        assert!(
            parse_search(
                include_str!("../../tests/fixtures/providers/hdmoli/empty.html"),
                &b
            )
            .unwrap()
            .is_empty()
        );
    }
    #[test]
    fn malformed_no_panic() {
        assert!(
            parse_detail(include_str!(
                "../../tests/fixtures/providers/hdmoli/malformed.html"
            ))
            .unwrap()
            .is_empty()
        );
    }
}
