trait ExecuteRequest {
    async fn execute(self) -> anyhow::Result<reqwest::Response>;
}
impl ExecuteRequest for reqwest::RequestBuilder {
    async fn execute(self) -> anyhow::Result<reqwest::Response> {
        crate::http::execute(self).await
    }
}

async fn read_text(response: reqwest::Response) -> anyhow::Result<String> {
    let status = response.status();
    if status != reqwest::StatusCode::OK {
        return Err(anyhow::anyhow!("clxiong HTTP {status}"));
    }
    let body = crate::http::read_response(response, 16 * 1024 * 1024)
        .await
        .map_err(|error| error.context(format!("clxiong response body HTTP {status}")))?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

use crate::core::{Link, ProviderError, SearchResult, Source};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use chrono::{NaiveDate, Utc};
use regex::Regex;
use scraper::{Html, Selector};
use url::Url;

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36";

fn request(client: &reqwest::Client, url: Url, referer: &Url) -> reqwest::RequestBuilder {
    client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::REFERER, referer.as_str())
        .header(
            reqwest::header::ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
        )
}
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
    pub search: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        let base = Url::parse("https://www.cilixiong.org/").expect("static URL");
        let search = base.join("e/search/index.php").expect("static URL");
        Self { base, search }
    }
}
#[derive(Debug, Clone, Default)]
pub struct ClxiongProvider {
    endpoints: Endpoints,
}
impl ClxiongProvider {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
#[derive(Debug)]
struct Item {
    id: String,
    title: String,
    content: String,
    images: Vec<String>,
    detail: Url,
}
fn search_id(location: &str) -> Option<String> {
    static SEARCH_ID: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"searchid=(\d+)").unwrap());
    SEARCH_ID
        .captures(location)?
        .get(1)
        .map(|m| m.as_str().to_owned())
}
fn parse_results(body: &str, base: &Url) -> Result<Vec<Item>, ProviderError> {
    let doc = Html::parse_document(body);
    let cards = Selector::parse(".row.row-cols-2.row-cols-lg-4 .col")
        .map_err(|e| ProviderError::Parse(e.to_string()))?;
    let anchors = Selector::parse("a[href*='/drama/'], a[href*='/movie/']").expect("selector");
    let title_sel = Selector::parse("h2.h4").expect("selector");
    let rank = Selector::parse(".rank").expect("selector");
    let small = Selector::parse(".small").expect("selector");
    let image = Selector::parse(".card-img").expect("selector");
    static IMAGE_RE_RE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r#"url\(['\"]?([^'\")]+)"#).expect("regex"));
    let image_re = &*IMAGE_RE_RE;
    static ID_RE_RE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"/(?:drama|movie)/(\d+)\.html").expect("regex"));
    let id_re = &*ID_RE_RE;
    let mut out = Vec::new();
    for card in doc.select(&cards).take(30) {
        let Some(a) = card.select(&anchors).next() else {
            continue;
        };
        let Some(href) = a.value().attr("href") else {
            continue;
        };
        let title = a
            .select(&title_sel)
            .next()
            .map(|n| n.text().collect::<String>().trim().to_owned())
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let Ok(detail) = base.join(href) else {
            continue;
        };
        let id = id_re
            .captures(href)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_owned())
            .unwrap_or_else(|| href.to_owned());
        let rating = card
            .select(&rank)
            .next()
            .map(|n| n.text().collect::<String>())
            .unwrap_or_default();
        let year = card
            .select(&small)
            .last()
            .map(|n| n.text().collect::<String>())
            .unwrap_or_default();
        let poster = card
            .select(&image)
            .next()
            .and_then(|n| n.value().attr("style"))
            .and_then(|s| image_re.captures(s))
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_owned());
        let content = format!(
            "评分: {} | 年份: {} | 详情页: {}",
            rating.trim(),
            year.trim(),
            detail
        );
        out.push(Item {
            id: format!("clxiong-{id}"),
            title,
            content,
            images: poster.into_iter().collect(),
            detail,
        });
    }
    Ok(out)
}
fn parse_detail(
    body: &str,
    title: &str,
) -> Result<(Vec<Link>, Option<chrono::DateTime<Utc>>), ProviderError> {
    let doc = Html::parse_document(body);
    let links = Selector::parse(".mv_down a[href^='magnet:']")
        .map_err(|e| ProviderError::Parse(e.to_string()))?;
    let detail = Selector::parse(".mv_detail p").expect("selector");
    let mut out = Vec::new();
    for a in doc.select(&links) {
        let Some(raw) = a.value().attr("href") else {
            continue;
        };
        let name = a.text().collect::<String>().trim().to_owned();
        let mut link = Link::new(raw);
        link.work_title = Some(if name.is_empty() {
            title.to_owned()
        } else {
            format!("{title}-{name}")
        });
        out.push(link)
    }
    static DATE_RE_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"最后更新于：?\s*(\d{4}[-/]\d{1,2}[-/]\d{1,2})").expect("regex")
    });
    let date_re = &*DATE_RE_RE;
    let datetime = doc.select(&detail).find_map(|p| {
        date_re
            .captures(&p.text().collect::<String>())
            .and_then(|c| {
                let s = c.get(1)?.as_str().replace('/', "-");
                NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()
            })
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| d.and_utc())
    });
    Ok((out, datetime))
}
#[async_trait]
impl Provider for ClxiongProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("clxiong", 2, KeywordFilterMode::Provider)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let form = [
            ("classid", "1,2"),
            ("show", "title"),
            ("tempid", "1"),
            ("keyboard", query),
        ];
        let first = tokio::time::timeout(
            ctx.timeout,
            ctx.client
                .post(self.endpoints.search.clone())
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .header(reqwest::header::REFERER, self.endpoints.base.as_str())
                .header(
                    reqwest::header::ACCEPT,
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
                )
                .form(&form)
                .execute(),
        )
        .await
        .map_err(|_| ProviderError::Timeout)?
        .map_err(|e| ProviderError::Network(e.to_string()))?;
        if first.status().is_client_error() || first.status().is_server_error() {
            return Err(ProviderError::Protocol(format!("HTTP {}", first.status())));
        }
        let location = first
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(first.url().as_str());
        let sid = search_id(location)
            .ok_or_else(|| ProviderError::Protocol("redirect did not contain searchid".into()))?;
        drop(first);
        let url = self
            .endpoints
            .base
            .join(&format!("e/search/result/?searchid={sid}"))
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        let response = request(&ctx.client, url, &self.endpoints.base)
            .execute()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        if !response.status().is_success() {
            return Err(ProviderError::Protocol(format!(
                "HTTP {}",
                response.status()
            )));
        }
        let items = parse_results(
            &read_text(response)
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?,
            &self.endpoints.base,
        )?;
        let jobs = items.into_iter().map(|item| async move {
            let response = request(&ctx.client, item.detail, &self.endpoints.base)
                .execute()
                .await
                .map_err(super::c_support::network)?;
            if response.status() != reqwest::StatusCode::OK {
                return Err(ProviderError::Protocol(format!(
                    "clxiong detail HTTP {}",
                    response.status()
                )));
            }
            let body = read_text(response)
                .await
                .map_err(super::c_support::network)?;
            let (links, datetime) = parse_detail(&body, &item.title)?;
            if links.is_empty() {
                return Ok(None);
            }
            Ok::<_, ProviderError>(Some(SearchResult {
                id: item.id,
                source: Source::provider("clxiong"),
                datetime,
                title: item.title,
                content: item.content,
                links,
                tags: vec!["磁力链接".into(), "影视".into()],
                images: item.images,
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
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let server = MockServer::start().await;
        let fixture = include_str!("../../tests/fixtures/providers/clxiong/success.html");
        Mock::given(method("POST"))
            .and(path("/e/search/index.php"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("Location", "/result?searchid=1"),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/e/search/result/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(format!("{fixture}{}", fixture.replace("4466", "9999"))),
            )
            .mount(&server)
            .await;
        Mock::given(path("/drama/4466.html"))
            .respond_with(ResponseTemplate::new(200).set_body_string(include_str!(
                "../../tests/fixtures/providers/clxiong/detail.html"
            )))
            .mount(&server)
            .await;
        Mock::given(path("/drama/9999.html"))
            .respond_with(ResponseTemplate::new(503))
            .expect(1)
            .mount(&server)
            .await;
        let base: Url = server.uri().parse().unwrap();
        let provider = ClxiongProvider::new(Endpoints {
            base: base.clone(),
            search: base.join("e/search/index.php").unwrap(),
        });
        let options = crate::http::ClientOptions {
            redirect: crate::http::RedirectPolicy::None,
            ..Default::default()
        };
        let client = crate::http::HttpClientFactory::new()
            .client(&options)
            .unwrap();
        let ctx = SearchContext::new(client, std::time::Duration::from_secs(5));
        let error = provider.search(&ctx, "一人之下").await.unwrap_err();
        assert!(error.to_string().contains("503"), "{error}");
        let rows = ctx.finish(Vec::new());
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].links.is_empty());
    }

    #[test]
    fn success() {
        let b = Url::parse("https://www.cilixiong.org/").unwrap();
        let r = parse_results(
            include_str!("../../tests/fixtures/providers/clxiong/success.html"),
            &b,
        )
        .unwrap();
        let (l, _) = parse_detail(
            include_str!("../../tests/fixtures/providers/clxiong/detail.html"),
            &r[0].title,
        )
        .unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(search_id("result/?searchid=7549").as_deref(), Some("7549"));
    }
    #[test]
    fn empty() {
        let b = Url::parse("https://x/").unwrap();
        assert!(
            parse_results(
                include_str!("../../tests/fixtures/providers/clxiong/empty.html"),
                &b
            )
            .unwrap()
            .is_empty()
        );
    }
    #[test]
    fn malformed() {
        assert!(
            parse_detail(
                include_str!("../../tests/fixtures/providers/clxiong/malformed.html"),
                "x"
            )
            .unwrap()
            .0
            .is_empty()
        );
    }
}
