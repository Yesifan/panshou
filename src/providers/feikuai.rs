//! 飞快 JSON torrent search, with the actual external-search HTML fallback.
use super::{UA, clean, fetch, password};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use regex::Regex;
use scraper::{Html, Selector};
use serde::Deserialize;
use std::{collections::HashSet, sync::LazyLock, time::Duration};
use url::Url;
#[derive(Debug, Clone)]
pub struct FeikuaiProvider {
    api: Url,
    web: Url,
}
impl Default for FeikuaiProvider {
    fn default() -> Self {
        Self::new(
            Url::parse("https://feikuai.tv/").unwrap(),
            Url::parse("https://feikuai.in/").unwrap(),
        )
    }
}
impl FeikuaiProvider {
    pub fn new(api: Url, web: Url) -> Self {
        Self { api, web }
    }
}
#[derive(Deserialize)]
struct Response {
    code: i64,
    items: Vec<Item>,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct Item {
    title: String,
    #[serde(rename = "type")]
    kind: String,
    year: Option<i32>,
    torrents: Vec<Torrent>,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct Torrent {
    info_hash: String,
    magnet: String,
    name: String,
    size_bytes: u64,
    size_gb: f64,
    seeders: i64,
    leechers: i64,
    published_at: String,
    published_ago: String,
    file_path: String,
    file_ext: String,
}
fn date(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&chrono::Utc))
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|d| d.and_utc())
        })
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|d| d.and_utc())
        })
}
fn parse(body: &str, q: &str) -> Result<Vec<SearchResult>, ProviderError> {
    static EXT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\.(mkv|mp4|avi|rmvb|wmv|flv|mov|ts|m2ts|iso)$").unwrap());
    static SIZE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s*·\s*[\d.]+\s*[KMGT]B\s*$").unwrap());
    let data: Response = serde_json::from_str(body)
        .map_err(|_| ProviderError::Parse("feikuai API: malformed JSON".into()))?;
    if data.code != 0 {
        return Err(ProviderError::Protocol(format!(
            "feikuai API code {}",
            data.code
        )));
    }
    let mut out = Vec::new();
    for item in data.items {
        for torrent in item.torrents {
            if !torrent.magnet.to_ascii_lowercase().starts_with("magnet:?") {
                continue;
            }
            let name = EXT.replace_all(&torrent.name, "");
            let name = SIZE.replace_all(&name, "");
            let name = name.split('@').next().unwrap_or("").trim();
            let title = if q
                .split_whitespace()
                .all(|w| name.to_lowercase().contains(&w.to_lowercase()))
            {
                name.to_owned()
            } else {
                format!("{q}-{name}")
            };
            let mut l = Link::new(torrent.magnet);
            l.work_title = Some(title.clone());
            l.datetime = date(&torrent.published_at);
            let mut r = SearchResult::provider(format!("feikuai-{}", torrent.info_hash), "feikuai");
            r.title = title;
            r.datetime = l.datetime;
            r.content = format!(
                "{} | 类型：{} | 年份：{} | 大小：{:.2} GB ({} bytes) | 做种：{} | 下载：{} | {} | {} | {}",
                item.title,
                item.kind,
                item.year.map(|y| y.to_string()).unwrap_or_default(),
                torrent.size_gb,
                torrent.size_bytes,
                torrent.seeders,
                torrent.leechers,
                torrent.published_ago,
                torrent.file_path,
                torrent.file_ext
            );
            r.tags = vec![item.kind.clone()];
            if let Some(y) = item.year {
                r.tags.push(y.to_string());
            }
            r.links.push(l);
            out.push(r);
        }
    }
    Ok(out)
}
fn web(body: &str, q: &str) -> Result<Vec<SearchResult>, ProviderError> {
    static DATE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\d{4}-\d{2}-\d{2}(?:[T ]\d{2}:\d{2}:\d{2})?Z?").unwrap());
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (selector, limit) in [
        ("#panel-pan .module-row-info a.module-row-text[href]", 50),
        ("#panel-bm .bm-item a.module-row-text[href]", 100),
    ] {
        for node in doc.select(&Selector::parse(selector).unwrap()) {
            if out.len() >= limit {
                break;
            }
            let Some(raw) = node.value().attr("href") else {
                continue;
            };
            let mut l = Link::new(raw);
            if l.cloud_type == CloudType::Others || !seen.insert(l.url.clone()) {
                continue;
            }
            let title = node
                .value()
                .attr("title")
                .map(str::to_owned)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    node.select(&Selector::parse("h4, .bm-file").unwrap())
                        .next()
                        .map(|n| clean(&n.inner_html()))
                        .unwrap_or_else(|| q.into())
                });
            let content = node
                .select(&Selector::parse(".ext-meta").unwrap())
                .next()
                .map(|n| clean(&n.inner_html()))
                .unwrap_or_default();
            l.password = password(raw, &content);
            l.work_title = Some(title.clone());
            l.datetime = DATE.find(&content).and_then(|m| date(m.as_str()));
            let digest = format!("{:x}", md5::compute(raw));
            let mut r = SearchResult::provider(format!("feikuai-{}", &digest[..16]), "feikuai");
            r.title = title;
            r.content = content;
            r.datetime = l.datetime;
            r.links.push(l);
            out.push(r);
        }
    }
    if out.is_empty() {
        return Err(ProviderError::Parse(
            "feikuai fallback: no valid resource rows".into(),
        ));
    }
    Ok(out)
}
#[async_trait]
impl Provider for FeikuaiProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("feikuai", 3, KeywordFilterMode::Provider)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        q: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut api = self.api.join("t_search/bm_search.php").unwrap();
        api.query_pairs_mut().append_pair("kw", q);
        let result = fetch(
            ctx.client
                .get(api)
                .header("User-Agent", UA)
                .header("Accept", "application/json, text/plain, */*")
                .header("Referer", self.api.as_str()),
            "feikuai",
            "API",
            ctx.request_timeout(Duration::from_secs(15)),
            16 << 20,
            &[200, 400],
        )
        .await
        .and_then(|b| parse(&b, q));
        let mut rows = match result {
            Ok(rows) => rows,
            Err(_) => {
                let mut url = self.web.join("vodsearch/-------------.html").unwrap();
                url.query_pairs_mut()
                    .append_pair("wd", q)
                    .append_pair("ext", "1");
                let body = fetch(
                    ctx.client
                        .get(url)
                        .header("User-Agent", UA)
                        .header("Referer", self.web.as_str()),
                    "feikuai",
                    "web fallback",
                    ctx.request_timeout(Duration::from_secs(15)),
                    16 << 20,
                    &[],
                )
                .await?;
                web(&body, q)?
            }
        };
        rows.retain(|r| {
            let text = format!("{} {}", r.title, r.content).to_lowercase();
            q.split_whitespace()
                .all(|w| text.contains(&w.to_lowercase()))
        });
        ctx.publish(rows.clone());
        Ok(rows)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixtures() {
        let rows = parse(
            include_str!("../../tests/fixtures/providers/feikuai/success.json"),
            "测试",
        )
        .unwrap();
        assert_eq!(
            rows[0].links[0].work_title.as_deref(),
            Some("测试-English.Release")
        );
        assert_eq!(
            web(
                include_str!("../../tests/fixtures/providers/feikuai/web.html"),
                "测试"
            )
            .unwrap()
            .len(),
            1
        );
        assert!(
            parse(
                include_str!("../../tests/fixtures/providers/feikuai/empty.json"),
                "测试"
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            parse(
                include_str!("../../tests/fixtures/providers/feikuai/malformed.json"),
                "测试"
            )
            .is_err()
        );
    }
}
