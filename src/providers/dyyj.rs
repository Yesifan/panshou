use crate::core::{CloudType, Link, ProviderError, SearchResult, Source};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use regex::Regex;
use scraper::{Html, Selector};
use serde::Deserialize;
use std::collections::HashMap;
use url::Url;

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://bbs.dyyjmax.org/").expect("static URL"),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct DyyjProvider {
    endpoints: Endpoints,
}
impl DyyjProvider {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}

#[derive(Deserialize)]
struct Payload {
    #[serde(default)]
    data: Vec<Discussion>,
    #[serde(default)]
    included: Vec<Included>,
}
#[derive(Deserialize)]
struct Discussion {
    id: String,
    attributes: DiscussionAttrs,
    relationships: Relationships,
}
#[derive(Deserialize)]
struct DiscussionAttrs {
    #[serde(default)]
    title: String,
    #[serde(default, rename = "createdAt")]
    created_at: String,
}
#[derive(Deserialize)]
struct Relationships {
    #[serde(rename = "mostRelevantPost")]
    most_relevant_post: PostRelationship,
}
#[derive(Deserialize)]
struct PostRelationship {
    data: IdRef,
}
#[derive(Deserialize)]
struct IdRef {
    id: String,
}
#[derive(Deserialize)]
struct Included {
    id: String,
    attributes: PostAttrs,
}
#[derive(Deserialize)]
struct PostAttrs {
    #[serde(default, rename = "contentHtml")]
    content_html: String,
}

fn parse_response(body: &str) -> Result<Vec<SearchResult>, ProviderError> {
    let payload: Payload =
        serde_json::from_str(body).map_err(|e| ProviderError::Parse(e.to_string()))?;
    let posts: HashMap<_, _> = payload
        .included
        .into_iter()
        .map(|p| (p.id, p.attributes.content_html))
        .collect();
    let mut out = Vec::new();
    for row in payload.data {
        let Some(html) = posts.get(&row.relationships.most_relevant_post.data.id) else {
            continue;
        };
        let links = extract_links(html);
        if links.is_empty() {
            continue;
        }
        out.push(SearchResult {
            id: format!("dyyj-{}", row.id),
            source: Source::provider("dyyj"),
            datetime: DateTime::parse_from_rfc3339(&row.attributes.created_at)
                .ok()
                .map(|v| v.with_timezone(&Utc)),
            title: row.attributes.title.trim().to_owned(),
            content: clean_html(html),
            links,
            tags: vec![],
            images: vec![],
        });
    }
    Ok(out)
}
fn extract_links(content: &str) -> Vec<Link> {
    let doc = Html::parse_fragment(content);
    let selector = Selector::parse("a[href]").expect("selector");
    let pwd = Regex::new(r"(?i)(?:提取码|密码|pwd)\s*[=:：]?\s*([A-Za-z0-9]{4,8})").expect("regex");
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for a in doc.select(&selector) {
        let Some(raw) = a.value().attr("href").map(str::trim) else {
            continue;
        };
        let mut link = Link::new(raw);
        if link.cloud_type == CloudType::Others || !seen.insert(raw.to_owned()) {
            continue;
        }
        link.password = Url::parse(raw)
            .ok()
            .and_then(|u| {
                u.query_pairs()
                    .find(|(k, _)| matches!(k.as_ref(), "pwd" | "password" | "code"))
                    .map(|(_, v)| v.into_owned())
            })
            .or_else(|| {
                let context = a
                    .parent()
                    .and_then(scraper::ElementRef::wrap)
                    .map(|p| p.text().collect::<String>())
                    .unwrap_or_else(|| a.text().collect());
                pwd.captures(&context)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_str().to_owned())
            });
        out.push(link)
    }
    out
}
fn clean_html(content: &str) -> String {
    Html::parse_fragment(content)
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[async_trait]
impl Provider for DyyjProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("dyyj", 2, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self
            .endpoints
            .base
            .join("api/discussions")
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("filter[q]", query)
            .append_pair("include", "mostRelevantPost")
            .append_pair("page[limit]", "100");
        let response = tokio::time::timeout(
            ctx.timeout,
            ctx.client
                .get(url)
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .header(
                    reqwest::header::ACCEPT,
                    "application/vnd.api+json, application/json",
                )
                .header(reqwest::header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
                .header(reqwest::header::REFERER, self.endpoints.base.as_str())
                .send(),
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
        parse_response(
            &response
                .text()
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?,
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn success() {
        let r = parse_response(include_str!(
            "../../tests/fixtures/providers/dyyj/success.json"
        ))
        .unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].links[0].password.as_deref(), Some("abcd"));
    }
    #[test]
    fn empty() {
        assert!(
            parse_response(include_str!(
                "../../tests/fixtures/providers/dyyj/empty.json"
            ))
            .unwrap()
            .is_empty()
        );
    }
    #[test]
    fn malformed() {
        assert!(
            parse_response(include_str!(
                "../../tests/fixtures/providers/dyyj/malformed.json"
            ))
            .is_err()
        );
    }
}
