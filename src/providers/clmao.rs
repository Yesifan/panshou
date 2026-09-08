use crate::core::{Link, ProviderError, SearchResult, Source};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use base64::Engine;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{Html, Selector};
use url::Url;

const UA: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/138 Safari/537.36";

fn browser_headers(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request
        .header("user-agent", UA)
        .header(
            "accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
        )
        .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("upgrade-insecure-requests", "1")
        .header("cache-control", "no-cache")
        .header("pragma", "no-cache")
}
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://clm64.top").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Clmao {
    endpoints: Endpoints,
}
impl Clmao {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
fn decode_payload(raw: &str) -> String {
    let re = Regex::new(r#"(?s)atob\(["']([A-Za-z0-9+/=]+)["']\)"#).unwrap();
    let Some(v) = re.captures(raw).and_then(|c| c.get(1)) else {
        return raw.into();
    };
    let Ok(b) = base64::engine::general_purpose::STANDARD.decode(v.as_str()) else {
        return raw.into();
    };
    percent_encoding::percent_decode(&b)
        .decode_utf8_lossy()
        .into_owned()
}
#[derive(Clone)]
struct Hit {
    id: String,
    title: String,
    url: Url,
    content: String,
}
fn modern_hits(body: &str, base: &Url) -> Vec<Hit> {
    let d = Html::parse_document(body);
    let li = Selector::parse("#Search_list_wrapper li").unwrap();
    let a = Selector::parse("a.SearchListTitle_result_title[href]").unwrap();
    let info = Selector::parse(".Search_list_info").unwrap();
    d.select(&li)
        .filter_map(|x| {
            let a = x.select(&a).next()?;
            let u = base.join(a.value().attr("href")?).ok()?;
            let title = a
                .text()
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if title.is_empty() {
                return None;
            }
            Some(Hit {
                id: u
                    .path_segments()
                    .and_then(|mut p| p.next_back())
                    .unwrap_or("item")
                    .into(),
                title,
                url: u,
                content: x
                    .select(&info)
                    .next()
                    .map(|e| e.text().collect::<String>())
                    .unwrap_or_default(),
            })
        })
        .collect()
}
fn detail(body: &str) -> Option<(String, String, Link)> {
    let decoded = decode_payload(body);
    let d = Html::parse_document(&decoded);
    let a = Selector::parse("a[href^='magnet:']").unwrap();
    let re = Regex::new(r#"(?i)magnet:\?[^\s"'<>]+"#).unwrap();
    let magnet = d
        .select(&a)
        .next()
        .and_then(|x| x.value().attr("href"))
        .map(str::to_owned)
        .or_else(|| re.find(&decoded).map(|m| m.as_str().to_owned()))?;
    let ts = Selector::parse("h1.Information_title").unwrap();
    let cs = Selector::parse(".Information_l_content").unwrap();
    let title = d
        .select(&ts)
        .next()
        .map(|x| x.text().collect::<String>().trim().to_owned())
        .unwrap_or_default();
    let content = d
        .select(&cs)
        .next()
        .map(|x| x.text().collect::<String>().trim().to_owned())
        .unwrap_or_default();
    let mut l = Link::new(magnet);
    if !title.is_empty() {
        l.work_title = Some(title.clone())
    }
    Some((title, content, l))
}
fn legacy(body: &str) -> Vec<SearchResult> {
    let d = Html::parse_document(body);
    let rows = Selector::parse(".tbox .ssbox").unwrap();
    let title = Selector::parse(".title h3 a").unwrap();
    let mag = Selector::parse(".sbar a[href^='magnet:']").unwrap();
    d.select(&rows)
        .enumerate()
        .filter_map(|(i, r)| {
            let t = r
                .select(&title)
                .next()?
                .text()
                .collect::<String>()
                .trim()
                .to_owned();
            let m = r.select(&mag).next()?.value().attr("href")?;
            let mut link = Link::new(m);
            link.work_title = Some(t.clone());
            Some(SearchResult {
                id: format!("clmao-legacy-{i}"),
                source: Source::provider("clmao"),
                datetime: None,
                title: t,
                content: r.text().collect::<String>(),
                links: vec![link],
                tags: vec![],
                images: vec![],
            })
        })
        .collect()
}
#[async_trait]
impl Provider for Clmao {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("clmao", 3, KeywordFilterMode::Provider)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let word = base64::engine::general_purpose::STANDARD.encode(query);
        let mut u = self
            .endpoints
            .base
            .join("search")
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        u.query_pairs_mut()
            .append_pair("word", &word)
            .append_pair("sort", "time");
        let body = browser_headers(ctx.client.get(u))
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?
            .error_for_status()
            .map_err(|e| ProviderError::Network(e.to_string()))?
            .text()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        let decoded = decode_payload(&body);
        let hits = modern_hits(&decoded, &self.endpoints.base);
        if hits.is_empty() {
            return Ok(legacy(&decoded));
        }
        let client = ctx.client.clone();
        let rows = stream::iter(hits.into_iter().map(|h| {
            let client = client.clone();
            async move {
                let b = browser_headers(client.get(h.url))
                    .send()
                    .await
                    .ok()?
                    .error_for_status()
                    .ok()?
                    .text()
                    .await
                    .ok()?;
                let (t, c, l) = detail(&b)?;
                Some(SearchResult {
                    id: format!("clmao-{}", h.id),
                    source: Source::provider("clmao"),
                    datetime: None,
                    title: if t.is_empty() { h.title } else { t },
                    content: if c.is_empty() { h.content } else { c },
                    links: vec![l],
                    tags: vec![],
                    images: vec![],
                })
            }
        }))
        .buffered(10)
        .collect::<Vec<_>>()
        .await;
        Ok(rows.into_iter().flatten().collect())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_success() {
        let hits = modern_hits(
            include_str!("../../tests/fixtures/providers/clmao/success-search.html"),
            &Endpoints::default().base,
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "movie-42");
        let encoded = base64::engine::general_purpose::STANDARD.encode("%3Cdiv%3Eok%3C/div%3E");
        assert_eq!(
            decode_payload(&format!("atob('{encoded}')")),
            "<div>ok</div>"
        );
        assert!(
            detail(include_str!(
                "../../tests/fixtures/providers/clmao/success.html"
            ))
            .is_some()
        );
        let legacy = legacy(include_str!(
            "../../tests/fixtures/providers/clmao/success-legacy.html"
        ));
        assert_eq!(legacy.len(), 1);
        assert_eq!(legacy[0].links[0].work_title.as_deref(), Some("仙逆 全集"));
    }
    #[test]
    fn fixture_empty() {
        let fixture = include_str!("../../tests/fixtures/providers/clmao/empty.html");
        assert!(modern_hits(fixture, &Endpoints::default().base).is_empty());
        assert!(legacy(fixture).is_empty())
    }
    #[test]
    fn fixture_malformed() {
        let fixture = include_str!("../../tests/fixtures/providers/clmao/malformed.html");
        assert_eq!(decode_payload(fixture), fixture);
        assert!(detail(fixture).is_none())
    }
}
