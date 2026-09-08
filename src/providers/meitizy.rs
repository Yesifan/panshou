use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::core::{Link, ProviderError, SearchResult, Source};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub search: Url,
    pub frontend: Url,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            search: Url::parse("https://apis.451024.xyz/api/media/search").expect("static URL"),
            frontend: Url::parse("https://video.451024.xyz/").expect("static URL"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MeitizyProvider {
    endpoints: Endpoints,
}

impl MeitizyProvider {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}

#[derive(Serialize)]
struct SearchRequest<'a> {
    title: &'a str,
    page: u8,
    size: u8,
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    data: Vec<ApiItem>,
}

#[derive(Deserialize)]
struct ApiItem {
    id: i64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    link: String,
    #[serde(default)]
    link_type: String,
    #[serde(default)]
    tags: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
}

fn parse_response(body: &str) -> Result<Vec<SearchResult>, ProviderError> {
    let payload: SearchResponse =
        serde_json::from_str(body).map_err(|e| ProviderError::Parse(e.to_string()))?;
    Ok(payload.data.into_iter().filter_map(convert_item).collect())
}

fn convert_item(item: ApiItem) -> Option<SearchResult> {
    let raw = item.link.trim();
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    let password = ["pwd", "password", "passcode", "code"]
        .into_iter()
        .find_map(|key| {
            url.query_pairs()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.into_owned())
        })
        .filter(|v| !v.trim().is_empty());
    let mut link = Link::new(raw);
    if link.cloud_type == crate::core::CloudType::Others {
        link.cloud_type = item.link_type.parse().unwrap_or_default();
    }
    link.password = password;
    Some(SearchResult {
        id: format!("meitizy-{}", item.id),
        source: Source::provider("meitizy"),
        datetime: parse_time(&item.created_at).or_else(|| parse_time(&item.updated_at)),
        title: item.title,
        content: item.content,
        links: vec![link],
        tags: (!item.tags.trim().is_empty())
            .then_some(vec![item.tags])
            .unwrap_or_default(),
        images: vec![],
    })
}

fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|v| v.with_timezone(&Utc))
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|v| v.and_utc())
        })
        .or_else(|| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()
                .and_then(|v| v.and_hms_opt(0, 0, 0))
                .map(|v| v.and_utc())
        })
}

#[async_trait]
impl Provider for MeitizyProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("meitizy", 2, KeywordFilterMode::Core)
    }

    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let response = tokio::time::timeout(
            ctx.timeout,
            ctx.client
                .post(self.endpoints.search.clone())
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .header(reqwest::header::ACCEPT, "application/json, text/plain, */*")
                .header(reqwest::header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
                .header(reqwest::header::CONNECTION, "keep-alive")
                .header(
                    reqwest::header::ORIGIN,
                    self.endpoints.frontend.origin().ascii_serialization(),
                )
                .header(reqwest::header::REFERER, self.endpoints.frontend.as_str())
                .json(&SearchRequest {
                    title: query,
                    page: 1,
                    size: 10,
                })
                .send(),
        )
        .await
        .map_err(|_| ProviderError::Timeout)?
        .map_err(|e| ProviderError::Network(e.to_string()))?;
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ProviderError::RateLimited);
        }
        if !response.status().is_success() {
            return Err(ProviderError::Protocol(format!(
                "HTTP {}",
                response.status()
            )));
        }
        let text = response
            .text()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        parse_response(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_success_fixture() {
        let r = parse_response(include_str!(
            "../../tests/fixtures/providers/meitizy/success.json"
        ))
        .unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].links[0].password.as_deref(), Some("abcd"));
    }
    #[test]
    fn parses_empty_fixture() {
        assert!(
            parse_response(include_str!(
                "../../tests/fixtures/providers/meitizy/empty.json"
            ))
            .unwrap()
            .is_empty()
        );
    }
    #[test]
    fn malformed_is_error() {
        assert!(
            parse_response(include_str!(
                "../../tests/fixtures/providers/meitizy/malformed.json"
            ))
            .is_err()
        );
    }
}
