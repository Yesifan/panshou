use std::collections::HashSet;
use std::str::FromStr;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{Html, Selector};
use serde::Deserialize;
use url::Url;

use crate::core::{Link, ProviderError, SearchResult, Source};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};

const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/124 Safari/537.36";

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub website: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            website: Url::parse("https://www.pansearch.me/search").unwrap(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Pansearch {
    endpoints: Endpoints,
}
impl Pansearch {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}

#[derive(Debug, Deserialize)]
struct Api {
    #[serde(rename = "pageProps")]
    page_props: PageProps,
}
#[derive(Debug, Deserialize)]
struct PageProps {
    data: PageData,
}
#[derive(Debug, Deserialize)]
struct PageData {
    total: usize,
    #[serde(default)]
    data: Vec<Item>,
}
#[derive(Debug, Clone, Deserialize)]
struct Item {
    id: i64,
    #[serde(default)]
    content: String,
    #[serde(default)]
    pan: String,
    #[serde(default)]
    time: String,
    #[serde(default)]
    image: String,
}

fn build_id(body: &str) -> Result<String, ProviderError> {
    let inline = Regex::new(r#""buildId"\s*:\s*"([^"]+)""#).unwrap();
    if let Some(v) = inline.captures(body).and_then(|c| c.get(1)) {
        return Ok(v.as_str().to_owned());
    }
    let re = Regex::new(r#"/_next/static/([^/]+)/_buildManifest\.js"#).unwrap();
    if let Some(v) = re.captures(body).and_then(|c| c.get(1)) {
        return Ok(v.as_str().to_owned());
    }
    let doc = Html::parse_document(body);
    let sel = Selector::parse("script#__NEXT_DATA__").unwrap();
    doc.select(&sel)
        .next()
        .and_then(|n| serde_json::from_str::<serde_json::Value>(&n.inner_html()).ok())
        .and_then(|v| v.get("buildId").and_then(|v| v.as_str()).map(str::to_owned))
        .ok_or_else(|| ProviderError::Parse("pansearch buildId missing".into()))
}

fn parse_api(body: &str) -> Result<Api, ProviderError> {
    serde_json::from_str(body).map_err(|e| ProviderError::Parse(e.to_string()))
}

fn clean_html(value: &str) -> String {
    let value = value
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("</p>", "\n");
    let doc = Html::parse_fragment(&value);
    doc.root_element()
        .text()
        .collect::<Vec<_>>()
        .join("")
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn convert(item: Item, query: &str) -> Option<SearchResult> {
    let doc = Html::parse_fragment(&item.content);
    let sel = Selector::parse("a.resource-link, a[href]").unwrap();
    let raw = doc.select(&sel).next()?.value().attr("href")?.trim();
    let parsed = Url::parse(raw).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let password = ["pwd", "password", "passcode", "code"]
        .iter()
        .find_map(|k| {
            parsed
                .query_pairs()
                .find(|(x, _)| x == *k)
                .map(|(_, v)| v.into_owned())
        })
        .or_else(|| {
            Regex::new(r"(?i)(?:提取码|访问码|密码|pwd|code)\s*[:：=]?\s*([0-9A-Za-z]{4,8})")
                .unwrap()
                .captures(&doc.root_element().text().collect::<String>())
                .and_then(|c| c.get(1).map(|x| x.as_str().to_owned()))
        });
    let content = clean_html(&item.content);
    let title = content
        .lines()
        .find_map(|l| l.strip_prefix("名称：").map(str::trim))
        .filter(|s| !s.is_empty())
        .unwrap_or(query)
        .to_owned();
    let datetime = DateTime::parse_from_rfc3339(&item.time)
        .ok()
        .map(|t| t.with_timezone(&Utc));
    let mut link = Link::new(parsed.as_str().trim_end_matches('#'));
    let normalized_pan = item.pan.trim().to_ascii_lowercase();
    let pan = if normalized_pan == "ali" {
        "aliyun"
    } else {
        normalized_pan.as_str()
    };
    if let Ok(cloud_type) = crate::core::CloudType::from_str(pan) {
        link.cloud_type = cloud_type;
    }
    link.password = password;
    link.work_title = Some(title.clone());
    let mut tags = vec![link.cloud_type.to_string(), "pansearch".into()];
    tags.sort();
    tags.dedup();
    Some(SearchResult {
        id: format!("pansearch-{}", item.id),
        source: Source::provider("pansearch"),
        datetime,
        title,
        content,
        links: vec![link],
        tags,
        images: if item.image.starts_with("http://") || item.image.starts_with("https://") {
            vec![item.image]
        } else {
            vec![]
        },
    })
}

#[async_trait]
impl Provider for Pansearch {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("pansearch", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let response = ctx
            .client
            .get(self.endpoints.website.clone())
            .header("user-agent", UA)
            .header(
                "accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
            )
            .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
            .header("upgrade-insecure-requests", "1")
            .header("cache-control", "max-age=0")
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        if response.status().as_u16() == 403 {
            return Err(ProviderError::Blocked);
        }
        let page = response
            .error_for_status()
            .map_err(|e| ProviderError::Network(e.to_string()))?
            .text()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        let id = build_id(&page)?;
        let origin = self.endpoints.website.origin().ascii_serialization();
        let base = format!("{origin}/_next/data/{id}/search.json");
        let fetch = |offset: usize| {
            let client = ctx.client.clone();
            let base = base.clone();
            let query = query.to_owned();
            let origin = origin.clone();
            async move {
                let mut u =
                    Url::parse(&base).map_err(|e| ProviderError::Protocol(e.to_string()))?;
                u.query_pairs_mut()
                    .append_pair("keyword", &query)
                    .append_pair("offset", &offset.to_string());
                let r = client
                    .get(u)
                    .header("user-agent", UA)
                    .header("referer", format!("{origin}/"))
                    .header("accept", "application/json, text/plain, */*")
                    .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
                    .header("cache-control", "no-cache")
                    .header("pragma", "no-cache")
                    .send()
                    .await
                    .map_err(|e| ProviderError::Network(e.to_string()))?;
                if r.status().as_u16() == 403 {
                    return Err(ProviderError::Blocked);
                }
                if r.status().as_u16() == 429 {
                    return Err(ProviderError::RateLimited);
                }
                parse_api(
                    &r.error_for_status()
                        .map_err(|e| ProviderError::Network(e.to_string()))?
                        .text()
                        .await
                        .map_err(|e| ProviderError::Network(e.to_string()))?,
                )
            }
        };
        let first = fetch(0).await?;
        let pages = first.page_props.data.total.min(50).div_ceil(10).min(5);
        let mut items = first.page_props.data.data;
        let rest = stream::iter((1..pages).map(|p| fetch(p * 10)))
            .buffer_unordered(4)
            .collect::<Vec<_>>()
            .await;
        for api in rest.into_iter().flatten() {
            items.extend(api.page_props.data.data);
        }
        let mut seen = HashSet::new();
        Ok(items
            .into_iter()
            .filter(|i| seen.insert(i.id))
            .filter_map(|i| convert(i, query))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_success() {
        assert_eq!(
            build_id(include_str!(
                "../../tests/fixtures/providers/pansearch/success-home.html"
            ))
            .unwrap(),
            "build-2026_09"
        );
        let api = parse_api(include_str!(
            "../../tests/fixtures/providers/pansearch/success.json"
        ))
        .unwrap();
        assert_eq!(
            convert(api.page_props.data.data[0].clone(), "仙逆")
                .unwrap()
                .links[0]
                .password
                .as_deref(),
            Some("a1b2")
        );
    }
    #[test]
    fn fixture_empty() {
        assert!(
            parse_api(include_str!(
                "../../tests/fixtures/providers/pansearch/empty.json"
            ))
            .unwrap()
            .page_props
            .data
            .data
            .is_empty()
        );
    }
    #[test]
    fn fixture_malformed() {
        assert!(
            parse_api(include_str!(
                "../../tests/fixtures/providers/pansearch/malformed.json"
            ))
            .is_err()
        );
        assert!(build_id("<html/>").is_err());
    }
}
