//! 爱看's public pansou API, preserving API IDs and per-link metadata.
use super::{UA, fetch};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;
use url::Url;
#[derive(Debug, Clone)]
pub struct IkantvProvider {
    base: Url,
}
impl Default for IkantvProvider {
    fn default() -> Self {
        Self::new(Url::parse("https://api.naspt.vip/").unwrap())
    }
}
impl IkantvProvider {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
}
#[derive(Deserialize)]
struct Response {
    code: i64,
    data: Option<Vec<Item>>,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct Item {
    unique_id: String,
    message_id: String,
    datetime: String,
    title: String,
    content: String,
    tags: Vec<String>,
    images: Vec<String>,
    links: Vec<ApiLink>,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct ApiLink {
    #[serde(rename = "type")]
    kind: String,
    url: String,
    password: String,
    datetime: String,
    work_title: String,
}
fn date(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|d| d.with_timezone(&chrono::Utc))
}
fn parse(body: &str) -> Result<Vec<SearchResult>, ProviderError> {
    let response: Response = serde_json::from_str(body)
        .map_err(|_| ProviderError::Parse("ikantv search: malformed JSON".into()))?;
    if response.code != 0 {
        return Err(ProviderError::Protocol(format!(
            "ikantv search: API code {}",
            response.code
        )));
    }
    let mut results = Vec::new();
    for row in response.data.unwrap_or_default() {
        let id = if row.unique_id.trim().is_empty() {
            row.message_id.trim()
        } else {
            row.unique_id.trim()
        };
        if id.is_empty() {
            continue;
        }
        let mut result = SearchResult::provider(
            format!("ikantv-{}", id.trim_start_matches("ikantv-")),
            "ikantv",
        );
        for raw in row.links {
            let Ok(ty) = raw.kind.parse::<CloudType>() else {
                continue;
            };
            if matches!(
                ty,
                CloudType::Others | CloudType::Weiyun | CloudType::Lanzou
            ) {
                continue;
            }
            let Ok(url) = Url::parse(raw.url.trim()) else {
                continue;
            };
            let valid = match ty {
                CloudType::Magnet => raw.url.to_ascii_lowercase().starts_with("magnet:?"),
                CloudType::Ed2k => raw.url.to_ascii_lowercase().starts_with("ed2k://"),
                _ => matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
            };
            if !valid {
                continue;
            }
            let mut link = Link::new(raw.url.trim());
            link.cloud_type = ty;
            link.password = (!raw.password.is_empty()).then_some(raw.password);
            link.datetime = date(&raw.datetime);
            link.work_title = (!raw.work_title.is_empty()).then_some(raw.work_title);
            result.links.push(link);
        }
        if result.links.is_empty() {
            continue;
        }
        result.title = row.title;
        result.content = row.content;
        result.tags = row.tags;
        result.images = row.images;
        result.datetime = date(&row.datetime);
        results.push(result);
    }
    Ok(results)
}
#[async_trait]
impl Provider for IkantvProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("ikantv", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self.base.join("api/open/pansou/search").unwrap();
        url.query_pairs_mut()
            .append_pair("kw", query)
            .append_pair("limit", "50");
        let body = fetch(
            ctx.client
                .get(url)
                .header("User-Agent", UA)
                .header("Accept", "application/json, text/plain, */*")
                .header("Referer", self.base.as_str()),
            "ikantv",
            "search",
            ctx.timeout.min(Duration::from_secs(30)),
            16 << 20,
            &[200, 400],
        )
        .await?;
        parse(&body)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixtures() {
        let rows = parse(include_str!(
            "../../tests/fixtures/providers/ikantv/success.json"
        ))
        .unwrap();
        assert_eq!(rows[0].id, "ikantv-abc");
        assert_eq!(rows[0].links[0].password.as_deref(), Some("abcd"));
        assert!(
            parse(include_str!(
                "../../tests/fixtures/providers/ikantv/empty.json"
            ))
            .unwrap()
            .is_empty()
        );
        assert!(
            parse(include_str!(
                "../../tests/fixtures/providers/ikantv/malformed.json"
            ))
            .is_err()
        );
    }
}
