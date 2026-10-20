use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{Link, ProviderError, SearchResult};
use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use std::{sync::LazyLock, time::Duration};
use url::Url;
static SHARE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:https?://(?:pan\.quark\.cn/s/[0-9a-zA-Z]+|drive\.uc\.cn/s/[0-9a-zA-Z]+|pan\.baidu\.com/s/[0-9a-zA-Z_\-]+|(?:www\.)?(?:aliyundrive\.com|alipan\.com)/s/[0-9a-zA-Z]+|pan\.xunlei\.com/s/[0-9a-zA-Z_\-]+|cloud\.189\.cn/t/[0-9a-zA-Z]+|115\.com/s/[0-9a-zA-Z]+|caiyun\.feixin\.10086\.cn/[0-9a-zA-Z]+|123pan\.com/s/[0-9a-zA-Z]+|mypikpak\.com/s/[0-9a-zA-Z]+)|magnet:\?xt=urn:btih:[0-9a-fA-F]{40}|ed2k://\|file\|.+\|\d+\|[0-9a-fA-F]{32}\|/)").unwrap()
});
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://woog.nxog.eu.org/").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Wanou {
    endpoints: Endpoints,
}
impl Wanou {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
#[derive(Deserialize)]
struct Response {
    code: i32,
    #[serde(default)]
    list: Vec<Item>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Item {
    vod_id: i64,
    vod_name: String,
    vod_actor: String,
    vod_director: String,
    vod_down_from: String,
    vod_down_url: String,
    vod_remarks: String,
    vod_area: String,
    vod_year: String,
    vod_pic: String,
}
fn parse(body: &[u8]) -> Result<Vec<SearchResult>, ProviderError> {
    let response: Response =
        serde_json::from_slice(body).map_err(|e| ProviderError::Parse(e.to_string()))?;
    if response.code != 1 {
        return Err(ProviderError::Protocol(format!(
            "wanou API code {}",
            response.code
        )));
    }
    Ok(response
        .list
        .into_iter()
        .filter(|i| !i.vod_name.trim().is_empty())
        .map(|item| {
            let mut row = SearchResult::provider(format!("wanou-{}", item.vod_id), "wanou");
            row.title = item.vod_name.trim().into();
            row.content = [
                ("主演", item.vod_actor),
                ("导演", item.vod_director),
                ("地区", item.vod_area.clone()),
                ("年份", item.vod_year.clone()),
                ("状态", item.vod_remarks),
            ]
            .into_iter()
            .filter(|(_, s)| !s.is_empty())
            .map(|(k, s)| format!("{k}: {s}"))
            .collect::<Vec<_>>()
            .join(" | ");
            row.tags = [item.vod_year, item.vod_area]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect();
            row.images = (!item.vod_pic.is_empty())
                .then_some(item.vod_pic)
                .into_iter()
                .collect();
            if !item.vod_down_from.is_empty() && !item.vod_down_url.is_empty() {
                row.links = item
                    .vod_down_from
                    .split("$$$")
                    .zip(item.vod_down_url.split("$$$"))
                    .filter_map(|(_, raw)| {
                        let raw = raw.trim();
                        if raw.contains("javascript:") || raw.contains('#') || !SHARE.is_match(raw)
                        {
                            return None;
                        }
                        let mut link = Link::new(raw);
                        link.password = Url::parse(raw).ok().and_then(|u| {
                            u.query_pairs().find_map(|(k, v)| {
                                (k == "pwd" && !v.is_empty()).then(|| v.into_owned())
                            })
                        });
                        Some(link)
                    })
                    .collect()
            }
            row
        })
        .collect())
}
#[async_trait]
impl Provider for Wanou {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("wanou", 1, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut url = self.endpoints.base.join("/api.php/provide/vod").unwrap();
        url.query_pairs_mut()
            .append_pair("ac", "detail")
            .append_pair("wd", query);
        let body = tokio::time::timeout(
            ctx.remaining().min(Duration::from_secs(8)),
            request(
                || {
                    ctx.client
                        .get(url.clone())
                        .header("referer", self.endpoints.base.as_str())
                        .header("accept", "application/json, text/plain, */*")
                        .header("cache-control", "no-cache")
                },
                2,
                Duration::from_millis(100),
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
    fn vod_protocol() {
        let r=parse(br#"{"code":1,"list":[{"vod_id":1,"vod_name":"movie","vod_down_from":"KG$$$BD$$$XL","vod_down_url":"https://pan.quark.cn/s/abc?pwd=abcd$$$https://pan.baidu.com/s/test?pwd=efgh$$$magnet:?xt=urn:btih:0123456789012345678901234567890123456789","vod_year":"2026"}]}"#).unwrap();
        assert_eq!(r[0].links.len(), 3);
        assert_eq!(r[0].links[1].password.as_deref(), Some("efgh"));
        assert_eq!(r[0].links[2].cloud_type, crate::core::CloudType::Magnet);
        assert!(parse(br#"{"code":1,"list":[]}"#).unwrap().is_empty());
        assert!(parse(br#"{"code":0}"#).is_err());
        assert!(parse(b"bad").is_err());
    }
}
