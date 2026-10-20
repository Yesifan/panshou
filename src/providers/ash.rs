//! Allsharehub embeds a JSON array in its search HTML.
use super::{UA, fetch};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use std::{sync::LazyLock, time::Duration};
use url::Url;
#[derive(Debug, Clone)]
pub struct AshProvider {
    base: Url,
}
impl Default for AshProvider {
    fn default() -> Self {
        Self::new(Url::parse("https://so.allsharehub.com/").unwrap())
    }
}
impl AshProvider {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
}
#[derive(Deserialize)]
struct Item {
    id: i64,
    title: String,
    url: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    is_type: i32,
    #[serde(default)]
    source_category_id: usize,
    #[serde(default)]
    code: serde_json::Value,
    #[serde(default)]
    times: String,
}
fn parse(body: &str) -> Result<Vec<SearchResult>, ProviderError> {
    static JSON: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"var jsonData = '(\[.*?\])';").unwrap());
    static CONTROL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\x00-\x1F\x7F]").unwrap());
    let Some(c) = JSON.captures(body) else {
        return Err(ProviderError::Parse("ash search: missing jsonData".into()));
    };
    let raw = c[1].replace("\\/", "/");
    let raw = CONTROL.replace_all(&raw, "");
    let items: Vec<Item> = serde_json::from_str(&raw)
        .map_err(|_| ProviderError::Parse("ash search: malformed jsonData".into()))?;
    let mut out = Vec::new();
    for row in items {
        let Ok(mut url) = Url::parse(row.url.trim()) else {
            continue;
        };
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            continue;
        }
        if url.host_str() == Some("pan.qualk.cn") {
            let _ = url.set_host(Some("pan.quark.cn"));
        }
        let mut link = Link::new(url.to_string());
        link.cloud_type = match row.is_type {
            2 => CloudType::Baidu,
            3 => CloudType::Uc,
            4 => CloudType::Xunlei,
            _ => CloudType::Quark,
        };
        link.password = row
            .code
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let mut r = SearchResult::provider(format!("ash-{}", row.id), "ash");
        r.title = row.title;
        r.content = row.name;
        r.links = vec![link];
        r.datetime = chrono::NaiveDate::parse_from_str(&row.times, "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| d.and_utc());
        if (1..=6).contains(&row.source_category_id) {
            r.tags.push(
                ["短剧", "电影", "电视剧", "动漫", "综艺", "充电视频"][row.source_category_id - 1]
                    .into(),
            );
        }
        out.push(r);
    }
    Ok(out)
}
#[async_trait]
impl Provider for AshProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("ash", 2, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let encoded = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
        let url = self.base.join(&format!("s/{encoded}.html")).unwrap();
        let body = tokio::time::timeout(
            ctx.timeout.min(Duration::from_secs(15)),
            fetch(
                ctx.client
                    .get(url)
                    .header("User-Agent", UA)
                    .header("Referer", self.base.as_str()),
                "ash",
                "search",
                Duration::from_secs(15),
                2 << 20,
                &[100],
            ),
        )
        .await
        .map_err(|_| ProviderError::Timeout)??;
        parse(&body)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixtures() {
        let r = parse(include_str!(
            "../../tests/fixtures/providers/ash/success.html"
        ))
        .unwrap();
        assert_eq!(r[0].links[0].url, "https://pan.quark.cn/s/test");
        assert!(
            parse(include_str!(
                "../../tests/fixtures/providers/ash/empty.html"
            ))
            .unwrap()
            .is_empty()
        );
        assert!(
            parse(include_str!(
                "../../tests/fixtures/providers/ash/malformed.html"
            ))
            .is_err()
        );
    }
}
