use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{Link, ProviderError, SearchResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashSet, time::Duration};
use url::Url;
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub search: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            search: Url::parse("https://ys.66ds.de/search").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Xdyh {
    endpoints: Endpoints,
}
impl Xdyh {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
#[derive(Deserialize)]
struct Response {
    status: String,
    #[serde(default)]
    data: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    title: String,
    #[serde(default)]
    post_date: String,
    #[serde(default)]
    drive_links: Vec<String>,
    #[serde(default)]
    password: String,
    #[serde(default)]
    has_password: bool,
    #[serde(default)]
    source_site: String,
    #[serde(default)]
    link_count: usize,
    #[serde(default)]
    file_preview: String,
}
fn parse(body: &[u8]) -> Result<Vec<SearchResult>, ProviderError> {
    let response: Response =
        serde_json::from_slice(body).map_err(|e| ProviderError::Parse(e.to_string()))?;
    if response.status != "success" {
        return Err(ProviderError::Protocol(
            "xdyh API did not report success".into(),
        ));
    }
    let mut seen = HashSet::new();
    Ok(response
        .data
        .into_iter()
        .enumerate()
        .filter_map(|(i, item)| {
            if !seen.insert((item.title.clone(), item.source_site.clone())) {
                return None;
            }
            let links = item
                .drive_links
                .into_iter()
                .filter(|s| {
                    Url::parse(s).is_ok_and(|u| {
                        matches!(u.scheme(), "http" | "https") && u.host_str().is_some()
                    })
                })
                .map(|s| {
                    let mut l = Link::new(s);
                    l.password = (!item.password.is_empty()).then(|| item.password.clone());
                    l
                })
                .collect::<Vec<_>>();
            if links.is_empty() {
                return None;
            }
            let mut row = SearchResult::provider(format!("xdyh-{i}"), "xdyh");
            row.datetime = date(&item.post_date)
                .or_else(|| {
                    ["%Y/%m/%d", "%m/%d/%Y"]
                        .iter()
                        .find_map(|f| chrono::NaiveDate::parse_from_str(&item.post_date, f).ok())
                        .and_then(|d| d.and_hms_opt(0, 0, 0))
                        .map(|d| d.and_utc())
                })
                .or_else(|| Some(chrono::Utc::now()));
            let mut parts = vec![];
            if !item.source_site.is_empty() {
                parts.push(format!("来源: {}", item.source_site));
                row.tags.push(item.source_site)
            }
            if item.link_count > 0 {
                parts.push(format!("链接数: {}", item.link_count))
            }
            if item.has_password && !item.password.is_empty() {
                parts.push(format!("密码: {}", item.password))
            }
            if !item.file_preview.is_empty() {
                let mut preview = item.file_preview.replace("<em>", "").replace("</em>", "");
                if preview.chars().count() > 100 {
                    preview = preview.chars().take(100).collect::<String>() + "..."
                }
                parts.push(format!("预览: {preview}"))
            }
            for (key, tag) in [
                ("4k", "4K"),
                ("1080p", "1080P"),
                ("720p", "720P"),
                ("蓝光", "蓝光"),
                ("高清", "高清"),
                ("更新", "更新中"),
                ("完结", "完结"),
                ("电影", "电影"),
                ("剧集", "剧集"),
                ("动漫", "动漫"),
                ("综艺", "综艺"),
            ] {
                if item.title.to_lowercase().contains(key) {
                    row.tags.push(tag.into())
                }
            }
            row.title = item.title;
            row.content = parts.join(" | ");
            row.links = links;
            Some(row)
        })
        .collect())
}
#[async_trait]
impl Provider for Xdyh {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("xdyh", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let payload = json!({"keyword":query,"sites":null,"max_workers":10,"save_to_file":false,"split_links":true});
        let body = tokio::time::timeout(
            ctx.timeout.min(Duration::from_secs(15)),
            request(
                || {
                    ctx.client
                        .post(self.endpoints.search.clone())
                        .header("accept", "application/json, text/plain, */*")
                        .header("referer", "https://ys.66ds.de/")
                        .header("origin", "https://ys.66ds.de")
                        .json(&payload)
                },
                3,
                Duration::from_millis(500),
                BODY_LIMIT,
            ),
        )
        .await
        .map_err(|_| ProviderError::Timeout)??;
        complete(ctx, parse(&body))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn responses() {
        let rows=parse(br#"{"status":"success","data":[{"title":"test 4K","drive_links":["https://pan.baidu.com/s/a","bad"],"password":"abcd"},{"title":"test 4K","drive_links":["https://pan.quark.cn/s/b"]}]}"#).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].links.len(), 1);
        assert_eq!(rows[0].links[0].password.as_deref(), Some("abcd"));
        assert!(rows[0].tags.contains(&"4K".into()));
        assert!(
            parse(br#"{"status":"success","data":[]}"#)
                .unwrap()
                .is_empty()
        );
        assert!(parse(br#"{"status":"error"}"#).is_err());
        assert!(parse(b"bad").is_err());
    }
}
