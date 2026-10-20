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
    if status != reqwest::StatusCode::OK {
        return Err(anyhow::anyhow!("hdmoli HTTP {status}"));
    }
    let body = crate::http::read_response(response, 16 * 1024 * 1024)
        .await
        .map_err(|error| error.context(format!("hdmoli response body HTTP {status}")))?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

use crate::core::{Link, ProviderError, SearchResult, Source};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
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
            request(&ctx.client, url, &self.endpoints.base).send_retry(),
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
            &read_text(response)
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?,
            &self.endpoints.base,
        )?;
        let jobs = rows.into_iter().map(|row| async move {
            let response = request(&ctx.client, row.detail, &self.endpoints.base)
                .send_retry()
                .await
                .map_err(super::c_support::network)?;
            if response.status() != reqwest::StatusCode::OK {
                return Err(ProviderError::Protocol(format!(
                    "hdmoli detail HTTP {}",
                    response.status()
                )));
            }
            let body = read_text(response)
                .await
                .map_err(super::c_support::network)?;
            let links = parse_detail(&body)?;
            if links.is_empty() {
                return Ok(None);
            }
            Ok::<_, ProviderError>(Some(SearchResult {
                id: row.id,
                source: Source::provider("hdmoli"),
                datetime: None,
                title: row.title,
                content: row.content,
                links,
                tags: row.tags,
                images: vec![],
            }))
        });
        super::collect_partial(ctx, jobs, 6).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn detail_status_is_preserved_and_completed_rows_survive() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        let server = MockServer::start().await;
        let fixture = include_str!("../../tests/fixtures/providers/hdmoli/success.html");
        Mock::given(path("/search.php"))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                "{fixture}{}",
                fixture.replace("xiann.html", "failed.html")
            )))
            .mount(&server)
            .await;
        Mock::given(path("/movie/xiann.html"))
            .respond_with(ResponseTemplate::new(200).set_body_string(include_str!(
                "../../tests/fixtures/providers/hdmoli/detail.html"
            )))
            .mount(&server)
            .await;
        Mock::given(path("/movie/failed.html"))
            .respond_with(ResponseTemplate::new(503))
            .expect(3)
            .mount(&server)
            .await;
        let base: Url = server.uri().parse().unwrap();
        let provider = HdmoliProvider::new(Endpoints { base });
        let options = crate::http::ClientOptions {
            redirect: crate::http::RedirectPolicy::None,
            ..Default::default()
        };
        let client = crate::http::HttpClientFactory::new()
            .client(&options)
            .unwrap();
        let ctx = SearchContext::new(client, std::time::Duration::from_secs(5));
        let error = provider.search(&ctx, "仙逆").await.unwrap_err();
        assert!(error.to_string().contains("503"), "{error}");
        let rows = ctx.finish(Vec::new());
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].links.is_empty());
    }

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
