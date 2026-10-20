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
        return Err(anyhow::anyhow!("djgou HTTP {status}"));
    }
    let body = crate::http::read_response(response, 16 * 1024 * 1024)
        .await
        .map_err(|error| error.context(format!("djgou response body HTTP {status}")))?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

use crate::core::{Link, ProviderError, SearchResult, Source};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use regex::Regex;
use scraper::{Html, Selector};
use url::Url;

const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/114 Safari/537.36";
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://duanjugou.top").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Djgou {
    endpoints: Endpoints,
}
impl Djgou {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
#[derive(Clone)]
struct Hit {
    id: String,
    title: String,
    url: Url,
    date: Option<DateTime<Utc>>,
}
fn parse_search(body: &str, base: &Url) -> Vec<Hit> {
    let d = Html::parse_document(body);
    let row = Selector::parse("article.post-item-row").unwrap();
    let a = Selector::parse("h2.post-title a").unwrap();
    let date = Selector::parse(".post-date").unwrap();
    d.select(&row)
        .filter_map(|r| {
            let a = r.select(&a).next()?;
            let url = base.join(a.value().attr("href")?).ok()?;
            let title = a.text().collect::<String>().trim().to_owned();
            if title.is_empty() {
                return None;
            }
            let ds = r.select(&date).next().map(|x| x.text().collect::<String>());
            let dt = ds
                .and_then(|x| NaiveDate::parse_from_str(x.trim(), "%Y-%m-%d").ok())
                .and_then(|x| x.and_hms_opt(0, 0, 0))
                .map(|x| DateTime::from_naive_utc_and_offset(x, Utc));
            Some(Hit {
                id: url.path().trim_matches('/').replace('/', "_"),
                title,
                url,
                date: dt,
            })
        })
        .collect()
}
fn btwaf(body: &str, base: &Url) -> Option<Url> {
    static BTWAF: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r#"window\.location\.href\s*=\s*["']([^"']*btwaf=[^"']+)["']"#).unwrap()
    });
    BTWAF
        .captures(body)
        .and_then(|c| c.get(1))
        .and_then(|m| base.join(m.as_str()).ok())
}
fn parse_detail(body: &str) -> (Vec<Link>, String) {
    static Q_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"https?://pan\.quark\.cn/s/[0-9A-Za-z_-]+").unwrap()
    });
    let q = &*Q_RE;
    static PWD_RE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"提取码[:：]\s*([0-9A-Za-z]{4})").unwrap());
    let pwd = PWD_RE
        .captures(body)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_owned());
    let mut seen = std::collections::HashSet::new();
    let links = q
        .find_iter(body)
        .filter(|m| seen.insert(m.as_str().to_owned()))
        .map(|m| {
            let mut l = Link::new(m.as_str());
            l.password = pwd.clone();
            l
        })
        .collect();
    let d = Html::parse_document(body);
    let sel = Selector::parse("div.post-content, div.erx-wrap").unwrap();
    let content = d
        .select(&sel)
        .next()
        .map(|x| {
            x.text()
                .collect::<Vec<_>>()
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    (links, content.chars().take(300).collect())
}
#[async_trait]
impl Provider for Djgou {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("djgou", 2, KeywordFilterMode::Core)
    }

    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut url = self
            .endpoints
            .base
            .join("search.php")
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("page", "1");
        let mut body = read_text(ctx
            .client
            .get(url)
            .header("user-agent", UA)
            .header(
                "accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
            )
            .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
            .header("upgrade-insecure-requests", "1")
            .header("cache-control", "max-age=0")
            .header("referer", self.endpoints.base.as_str())
            .send_retry()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?
            .error_for_status()
            .map_err(|e| ProviderError::Network(e.to_string()))?).await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        if parse_search(&body, &self.endpoints.base).is_empty()
            && let Some(challenge) = btwaf(&body, &self.endpoints.base)
        {
            body = read_text(ctx
                .client
                .get(challenge)
                .header("user-agent", UA)
                .header(
                    "accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
                )
                .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
                .header("referer", self.endpoints.base.as_str())
                .send_retry()
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?).await
                .map_err(|e| ProviderError::Network(e.to_string()))?;
            let document = Html::parse_document(&body);
            let search_document =
                Selector::parse("article.post-item-row, main, .post-list, .no-results").unwrap();
            if document.select(&search_document).next().is_none() {
                return Err(ProviderError::Parse(
                    "djgou BTWAF challenge: unrecognized search document".into(),
                ));
            }
        }
        let client = ctx.client.clone();
        let referer = self.endpoints.base.as_str().to_owned();
        let futures = parse_search(&body, &self.endpoints.base)
            .into_iter()
            .map(move |hit| {
                let client = client.clone();
                let referer = referer.clone();
                async move {
                    let body = read_text(client
                        .get(hit.url)
                        .header("user-agent", UA)
                        .header(
                            "accept",
                            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8",
                        )
                        .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
                        .header("referer", referer)
                        .send_retry()
                        .await
                        .map_err(super::c_support::network)?
                        .error_for_status()
                        .map_err(super::c_support::network)?).await
                        .map_err(super::c_support::network)?;
                    let (links, content) = parse_detail(&body);
                    Ok::<_, ProviderError>((!links.is_empty()).then(|| SearchResult {
                        id: format!("djgou-{}", hit.id),
                        source: Source::provider("djgou"),
                        datetime: hit.date,
                        title: hit.title,
                        content,
                        links,
                        tags: vec!["短剧".into()],
                        images: vec![],
                    }))
                }
            });
        super::collect_partial(ctx, futures, 15).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn detail_status_is_preserved_and_completed_rows_survive() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        let server = MockServer::start().await;
        let fixture = include_str!("../../tests/fixtures/providers/djgou/success.html");
        Mock::given(path("/search.php"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(format!("{fixture}{}", fixture.replace("/a/1", "/a/2"))),
            )
            .mount(&server)
            .await;
        Mock::given(path("/a/1"))
            .respond_with(ResponseTemplate::new(200).set_body_string(include_str!(
                "../../tests/fixtures/providers/djgou/success-detail.html"
            )))
            .mount(&server)
            .await;
        Mock::given(path("/a/2"))
            .respond_with(ResponseTemplate::new(503))
            .expect(3)
            .mount(&server)
            .await;
        let base: Url = server.uri().parse().unwrap();
        let provider = Djgou::new(Endpoints { base });
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
    fn fixture_success() {
        let h = parse_search(
            include_str!("../../tests/fixtures/providers/djgou/success.html"),
            &Endpoints::default().base,
        );
        assert_eq!(h.len(), 1);
        let (l, _) = parse_detail(include_str!(
            "../../tests/fixtures/providers/djgou/success-detail.html"
        ));
        assert_eq!(l[0].password.as_deref(), Some("a1b2"));
    }
    #[test]
    fn fixture_empty() {
        assert!(
            parse_search(
                include_str!("../../tests/fixtures/providers/djgou/empty.html"),
                &Endpoints::default().base
            )
            .is_empty()
        );
        assert!(parse_detail("x").0.is_empty())
    }
    #[test]
    fn fixture_malformed() {
        assert!(
            parse_search(
                include_str!("../../tests/fixtures/providers/djgou/malformed.html"),
                &Endpoints::default().base
            )
            .is_empty()
        )
    }
}
