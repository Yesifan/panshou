//! 欧歌 MacCMS detail API, with parallel $$$ download-source lists.
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use std::{sync::LazyLock, time::Duration};
use url::Url;
#[derive(Debug, Clone)]
pub struct Ouge {
    endpoint: Url,
}
impl Default for Ouge {
    fn default() -> Self {
        Self::new(Url::parse("https://woog.nxog.eu.org/api.php/provide/vod").unwrap())
    }
}
impl Ouge {
    pub fn new(endpoint: Url) -> Self {
        Self { endpoint }
    }
}
static PATTERNS: LazyLock<Vec<(CloudType, Regex)>> = LazyLock::new(|| {
    use CloudType::*;
    [
        (Quark, r"https?://pan\.quark\.cn/s/[0-9a-zA-Z]+"),
        (Uc, r#"https?://drive\.uc\.cn/s/[0-9a-zA-Z]+(\?[^"'\s]*)?"#),
        (
            Baidu,
            r"https?://pan\.baidu\.com/s/[0-9a-zA-Z_\-]+(\?pwd=[0-9a-zA-Z]+)?",
        ),
        (
            Aliyun,
            r"https?://(www\.)?(aliyundrive\.com|alipan\.com)/s/[0-9a-zA-Z]+",
        ),
        (
            Xunlei,
            r"https?://pan\.xunlei\.com/s/[0-9a-zA-Z_\-]+(\?pwd=[0-9a-zA-Z]+)?",
        ),
        (Tianyi, r"https?://cloud\.189\.cn/t/[0-9a-zA-Z]+"),
        (One15, r"https?://115\.com/s/[0-9a-zA-Z]+"),
        (Mobile, r"https?://caiyun\.feixin\.10086\.cn/[0-9a-zA-Z]+"),
        (Pan123, r"https?://123pan\.com/s/[0-9a-zA-Z]+"),
        (Pikpak, r"https?://mypikpak\.com/s/[0-9a-zA-Z]+"),
        (Magnet, r"magnet:\?xt=urn:btih:[0-9a-fA-F]{40}"),
        (Ed2k, r"ed2k://\|file\|.+\|\d+\|[0-9a-fA-F]{32}\|/"),
    ]
    .into_iter()
    .map(|(kind, pattern)| (kind, Regex::new(pattern).unwrap()))
    .collect()
});
static PASSWORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\?pwd=([0-9a-zA-Z]+)").unwrap());
#[derive(Deserialize)]
struct Response {
    code: i64,
    list: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    vod_id: u64,
    vod_name: String,
    #[serde(default)]
    vod_actor: String,
    #[serde(default)]
    vod_director: String,
    #[serde(default)]
    vod_area: String,
    #[serde(default)]
    vod_year: String,
    #[serde(default)]
    vod_remarks: String,
    #[serde(default)]
    vod_pic: String,
    #[serde(default)]
    vod_down_from: String,
    #[serde(default)]
    vod_down_url: String,
}
fn parse(body: &str) -> Result<Vec<SearchResult>, ProviderError> {
    let response: Response =
        serde_json::from_str(body).map_err(|e| ProviderError::Parse(e.to_string()))?;
    if response.code != 1 {
        return Err(ProviderError::Protocol(format!(
            "ouge API code={}",
            response.code
        )));
    }
    let mut out = Vec::new();
    for item in response.list {
        let title = item.vod_name.trim();
        if title.is_empty() {
            continue;
        }
        let mut r = SearchResult::provider(format!("ouge-{}", item.vod_id), "ouge");
        r.title = title.to_owned();
        r.content = [
            ("主演", &item.vod_actor),
            ("导演", &item.vod_director),
            ("地区", &item.vod_area),
            ("年份", &item.vod_year),
            ("状态", &item.vod_remarks),
        ]
        .into_iter()
        .filter(|(_, s)| !s.is_empty())
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join(" | ");
        for (from, raw) in item
            .vod_down_from
            .split("$$$")
            .zip(item.vod_down_url.split("$$$"))
        {
            let raw = raw.trim();
            if raw.is_empty()
                || raw.contains("javascript:")
                || raw.contains('#')
                || !["http", "magnet:", "ed2k:"]
                    .iter()
                    .any(|p| raw.starts_with(p))
            {
                continue;
            }
            let Some((detected, _)) = PATTERNS.iter().find(|(_, re)| re.is_match(raw)) else {
                continue;
            };
            let kind = match from.trim().to_uppercase().as_str() {
                "BD" => CloudType::Baidu,
                "KG" => CloudType::Quark,
                "UC" => CloudType::Uc,
                "ALY" => CloudType::Aliyun,
                "XL" => CloudType::Xunlei,
                "TY" => CloudType::Tianyi,
                "115" => CloudType::One15,
                "MB" => CloudType::Mobile,
                "123" => CloudType::Pan123,
                "PK" => CloudType::Pikpak,
                _ => *detected,
            };
            let mut link = Link::new(raw);
            link.cloud_type = kind;
            link.password = PASSWORD.captures(raw).map(|c| c[1].to_owned());
            link.work_title = Some(r.title.clone());
            r.links.push(link);
        }
        r.tags = [item.vod_year, item.vod_area]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        if !item.vod_pic.is_empty() {
            r.images.push(item.vod_pic);
        }
        if !r.links.is_empty() {
            out.push(r);
        }
    }
    Ok(out)
}
#[async_trait]
impl Provider for Ouge {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("ouge", 2, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        tokio::time::timeout(ctx.timeout, async {
            let body = crate::providers::b_registry::http::text(
                ctx.client
                    .get(self.endpoint.clone())
                    .query(&[("ac", "detail"), ("wd", query)])
                    .header(
                        "user-agent",
                        "Mozilla/5.0 Chrome/91.0.4472.124 Safari/537.36",
                    )
                    .header("accept", "application/json, text/plain, */*")
                    .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
                    .header("referer", "https://woog.nxog.eu.org/")
                    .header("cache-control", "no-cache"),
                2,
                100,
                false,
                Duration::from_secs(8),
                16 << 20,
            )
            .await?;
            parse(&body)
        })
        .await
        .map_err(|_| ProviderError::Timeout)?
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn success_empty_malformed() {
        let r=parse(r#"{"code":1,"list":[{"vod_id":123,"vod_name":"Film","vod_actor":"Actor","vod_year":"2026","vod_pic":"https://images.test/film.jpg","vod_down_from":"KG$$$BD","vod_down_url":"https://pan.quark.cn/s/abc$$$https://pan.baidu.com/s/def?pwd=1234"}]}"#).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].links.len(), 2);
        assert_eq!(r[0].links[1].password.as_deref(), Some("1234"));
        assert_eq!(r[0].tags, ["2026"]);
        assert!(parse(r#"{"code":1,"list":[]}"#).unwrap().is_empty());
        assert!(parse("{}").is_err());
        assert!(parse(r#"{"code":0,"list":[]}"#).is_err());
    }
    #[tokio::test]
    async fn retry_http_boundary() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, query_param},
        };
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("ac", "detail"))
            .and(query_param("wd", "film"))
            .respond_with(ResponseTemplate::new(503))
            .expect(2)
            .mount(&s)
            .await;
        let p = Ouge::new(Url::parse(&s.uri()).unwrap());
        let c = SearchContext::new(reqwest::Client::new(), Duration::from_secs(2));
        assert!(
            p.search(&c, "film")
                .await
                .unwrap_err()
                .to_string()
                .contains("503")
        );
    }
}
