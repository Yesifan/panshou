use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use async_trait::async_trait;
use scraper::{Html, Selector};
use std::{collections::HashSet, time::Duration};
use url::Url;
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub search: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            search: Url::parse("https://quarksoo.cc/search.php").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Quarksoo {
    endpoints: Endpoints,
}
impl Quarksoo {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
fn parse(body: &str, query: &str) -> Vec<SearchResult> {
    let doc = Html::parse_document(body);
    let td = Selector::parse("td").unwrap();
    let a = Selector::parse("a[href]").unwrap();
    let words = query
        .to_lowercase()
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut seen = HashSet::new();
    let mut rows = vec![];
    for tr in doc.select(&Selector::parse("tr").unwrap()) {
        let cells = tr.select(&td).collect::<Vec<_>>();
        if cells.len() < 2 {
            continue;
        }
        let title = cells[0]
            .text()
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if title.is_empty()
            || title.contains("剧名")
            || title.contains("网盘链接")
            || !words.iter().all(|w| title.to_lowercase().contains(w))
        {
            continue;
        }
        let Some(raw) = cells[1]
            .select(&a)
            .next()
            .or_else(|| tr.select(&a).next())
            .and_then(|a| a.value().attr("href"))
        else {
            continue;
        };
        let Ok(url) = Url::parse(raw.trim()) else {
            continue;
        };
        if !matches!(url.scheme(), "http" | "https")
            || !matches!(url.host_str(), Some("pan.quark.cn" | "pan.qoark.cn"))
        {
            continue;
        }
        let raw = raw.trim();
        let hash = md5::compute(format!("{title}|{raw}"));
        let id = format!(
            "quarksoo-{}",
            hash.0[..8]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        if !seen.insert(id.clone()) {
            continue;
        }
        let mut row = SearchResult::provider(id, "quarksoo");
        row.title = title;
        row.datetime = Some(chrono::Utc::now());
        let mut link = Link::new(raw);
        link.cloud_type = CloudType::Quark;
        row.links.push(link);
        rows.push(row)
    }
    rows.sort_by(|a, b| a.title.cmp(&b.title));
    rows
}
#[async_trait]
impl Provider for Quarksoo {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("quarksoo", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut url = self.endpoints.search.clone();
        url.query_pairs_mut().append_pair("q", query);
        let mut last = ProviderError::Unavailable("quarksoo request failed".into());
        for attempt in 0..3 {
            if ctx.is_cancelled() {
                return Err(ProviderError::Timeout);
            }
            let result=async{let r=crate::http::execute(ctx.client.get(url.clone()).timeout(ctx.remaining()).header("referer","https://quarksoo.cc/").header("user-agent","Mozilla/5.0 Chrome/91.0.4472.124 Safari/537.36").header("accept","text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8")).await.map_err(network)?;let status=r.status();let body=crate::http::read_response(r,BODY_LIMIT).await.map_err(|e|ProviderError::Network(e.to_string()))?;if status!=200{return Err(ProviderError::Protocol(format!("HTTP {}",status.as_u16())))}Ok(body)}.await;
            match result {
                Ok(body) => {
                    return complete(ctx, Ok(parse(&String::from_utf8_lossy(&body), query)));
                }
                Err(e) => last = e,
            }
            if attempt < 2 {
                tokio::time::sleep(Duration::from_millis(500)).await
            }
        }
        Err(last)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rows() {
        let h = r#"<table><tr><td>movie</td><td><a href="https://pan.quark.cn/s/a">link</a></td></tr><tr><td>movie</td><td><a href="https://pan.quark.cn/s/a">link</a></td></tr><tr><td>movie</td><td><a href="https://evil.test/?u=pan.quark.cn">evil</a></td></tr></table>"#;
        assert_eq!(parse(h, "movie").len(), 1);
        assert!(parse(h, "other").is_empty());
        assert!(parse("bad", "movie").is_empty());
    }
    #[tokio::test]
    async fn retries_exactly_three() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::any};
        for recover in [true, false] {
            let server = MockServer::start().await;
            let count = Arc::new(AtomicUsize::new(0));
            let c = count.clone();
            Mock::given(any())
                .respond_with(move |_: &wiremock::Request| {
                    let n = c.fetch_add(1, Ordering::SeqCst);
                    ResponseTemplate::new(if recover && n >= 2 { 200 } else { 500 })
                        .set_body_string("<table></table>")
                })
                .expect(3)
                .mount(&server)
                .await;
            let p = Quarksoo::new(Endpoints {
                search: Url::parse(&server.uri()).unwrap(),
            });
            let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
            let result = p.search(&ctx, "movie").await;
            if recover {
                assert!(result.unwrap().is_empty())
            } else {
                assert!(result.unwrap_err().to_string().contains("500"))
            }
            assert_eq!(count.load(Ordering::SeqCst), 3);
        }
    }
}
