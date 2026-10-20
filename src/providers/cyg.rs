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
        return Err(anyhow::anyhow!("cyg HTTP {status}"));
    }
    let body = crate::http::read_response(response, 16 * 1024 * 1024)
        .await
        .map_err(|error| error.context(format!("cyg response body HTTP {status}")))?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use serde::Deserialize;
use url::Url;

use crate::core::{Link, ProviderError, SearchResult, Source};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};

const USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 16_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.6 Mobile/15E148 Safari/604.1";

fn request(client: &reqwest::Client, url: Url, referer: &Url) -> reqwest::RequestBuilder {
    client
        .get(url)
        .header(reqwest::header::REFERER, referer.as_str())
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::ACCEPT, "application/json, text/plain, */*")
        .header(reqwest::header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
        .header(reqwest::header::CONNECTION, "keep-alive")
}

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://www.acgndog.com/").expect("static URL"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CygProvider {
    endpoints: Endpoints,
}
impl CygProvider {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}

#[derive(Debug, Default, Deserialize)]
struct Rendered {
    #[serde(default)]
    rendered: String,
}
#[derive(Debug, Deserialize)]
struct Post {
    id: i64,
    #[serde(default)]
    date: String,
    #[serde(default)]
    title: Rendered,
    #[serde(default)]
    excerpt: Rendered,
    #[serde(default)]
    category_name: String,
}
#[derive(Debug, Deserialize)]
struct Download {
    #[serde(default)]
    name: String,
    #[serde(default)]
    url: String,
    #[serde(default, rename = "downloadPwd")]
    download_pwd: String,
}

fn parse_posts(body: &str) -> Result<Vec<Post>, ProviderError> {
    serde_json::from_str(body).map_err(|e| ProviderError::Parse(e.to_string()))
}
fn parse_downloads(body: &str) -> Result<Vec<Link>, ProviderError> {
    let rows: Vec<Download> =
        serde_json::from_str(body).map_err(|e| ProviderError::Parse(e.to_string()))?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let raw = row.url.trim();
            if raw.is_empty() {
                return None;
            }
            let mut link = Link::new(raw);
            if link.cloud_type == crate::core::CloudType::Others {
                link.cloud_type = cloud_from_name(&row.name);
            }
            link.password = (!row.download_pwd.trim().is_empty()).then_some(row.download_pwd);
            Some(link)
        })
        .collect())
}
fn cloud_from_name(name: &str) -> crate::core::CloudType {
    match name.trim().to_ascii_lowercase().as_str() {
        "夸克" | "夸克网盘" => crate::core::CloudType::Quark,
        "uc" | "uc网盘" => crate::core::CloudType::Uc,
        "百度" | "百度网盘" | "baidu" => crate::core::CloudType::Baidu,
        "阿里" | "阿里云盘" | "阿里网盘" | "aliyun" => crate::core::CloudType::Aliyun,
        "迅雷" | "迅雷网盘" | "xunlei" => crate::core::CloudType::Xunlei,
        "天翼" | "天翼云盘" | "189" | "189云盘" => crate::core::CloudType::Tianyi,
        "115" | "115网盘" => crate::core::CloudType::One15,
        "123" | "123网盘" | "123pan" => crate::core::CloudType::Pan123,
        "pikpak" | "pikpak网盘" => crate::core::CloudType::Pikpak,
        "磁力链接" | "magnet" => crate::core::CloudType::Magnet,
        "ed2k" => crate::core::CloudType::Ed2k,
        _ => crate::core::CloudType::Others,
    }
}
fn clean_html(value: &str) -> String {
    let fragment = scraper::Html::parse_fragment(value);
    fragment
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn convert(post: Post, links: Vec<Link>) -> SearchResult {
    let datetime = DateTime::parse_from_rfc3339(&post.date)
        .ok()
        .map(|v| v.with_timezone(&Utc))
        .or_else(|| {
            NaiveDateTime::parse_from_str(&post.date, "%Y-%m-%dT%H:%M:%S")
                .ok()
                .map(|v| v.and_utc())
        });
    SearchResult {
        id: format!("cyg-{}", post.id),
        source: Source::provider("cyg"),
        datetime,
        title: clean_html(&post.title.rendered),
        content: clean_html(&post.excerpt.rendered),
        links,
        tags: (!post.category_name.is_empty())
            .then_some(vec![post.category_name])
            .unwrap_or_default(),
        images: vec![],
    }
}

#[async_trait]
impl Provider for CygProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("cyg", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self
            .endpoints
            .base
            .join("wp-json/wp/v2/posts")
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("per_page", "20")
            .append_pair("orderby", "date")
            .append_pair("order", "desc")
            .append_pair("page", "1")
            .append_pair("search", query);
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
        let posts = parse_posts(
            &read_text(response)
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?,
        )?;
        let jobs = posts.into_iter().map(|post| async move {
            let mut url = self
                .endpoints
                .base
                .join("wp-json/acg-studio/v1/download")
                .map_err(|error| ProviderError::Protocol(error.to_string()))?;
            url.query_pairs_mut()
                .append_pair("id", &post.id.to_string());
            let response = request(&ctx.client, url, &self.endpoints.base)
                .send_retry()
                .await
                .map_err(super::c_support::network)?;
            if response.status() != reqwest::StatusCode::OK {
                return Err(ProviderError::Protocol(format!(
                    "cyg download HTTP {}",
                    response.status()
                )));
            }
            let body = read_text(response)
                .await
                .map_err(super::c_support::network)?;
            let links = parse_downloads(&body)?;
            Ok::<_, ProviderError>((!links.is_empty()).then(|| convert(post, links)))
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
        let mut posts: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/providers/cyg/success.json"
        ))
        .unwrap();
        let mut second = posts[0].clone();
        second["id"] = serde_json::json!(43);
        posts.as_array_mut().unwrap().push(second);
        Mock::given(path("/wp-json/wp/v2/posts"))
            .respond_with(ResponseTemplate::new(200).set_body_json(posts))
            .mount(&server)
            .await;
        Mock::given(path("/wp-json/acg-studio/v1/download"))
            .and(wiremock::matchers::query_param("id", "42"))
            .respond_with(ResponseTemplate::new(200).set_body_string(include_str!(
                "../../tests/fixtures/providers/cyg/download.json"
            )))
            .mount(&server)
            .await;
        Mock::given(path("/wp-json/acg-studio/v1/download"))
            .and(wiremock::matchers::query_param("id", "43"))
            .respond_with(ResponseTemplate::new(503))
            .expect(3)
            .mount(&server)
            .await;
        let base: Url = server.uri().parse().unwrap();
        let provider = CygProvider::new(Endpoints { base });
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
        let p = parse_posts(include_str!(
            "../../tests/fixtures/providers/cyg/success.json"
        ))
        .unwrap();
        let l = parse_downloads(include_str!(
            "../../tests/fixtures/providers/cyg/download.json"
        ))
        .unwrap();
        assert_eq!(
            convert(p.into_iter().next().unwrap(), l).links[0]
                .password
                .as_deref(),
            Some("1iq9")
        );
    }
    #[test]
    fn empty() {
        assert!(
            parse_posts(include_str!(
                "../../tests/fixtures/providers/cyg/empty.json"
            ))
            .unwrap()
            .is_empty()
        );
    }
    #[test]
    fn malformed() {
        assert!(
            parse_posts(include_str!(
                "../../tests/fixtures/providers/cyg/malformed.json"
            ))
            .is_err()
        );
    }
}
