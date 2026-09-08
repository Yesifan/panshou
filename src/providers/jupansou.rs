use crate::core::{Link, ProviderError, SearchResult, Source};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use chrono::Utc;
use futures::{StreamExt, stream};
use serde::Deserialize;
use url::Url;

const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/124 Safari/537.36";
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://dyuzi.com").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Jupansou {
    endpoints: Endpoints,
}
impl Jupansou {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
#[derive(Debug, Clone, Deserialize)]
struct Item {
    #[serde(default)]
    title: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    disk_type: String,
    #[serde(default)]
    is_type: i32,
}
fn sse(body: &str) -> Vec<Item> {
    body.lines()
        .filter_map(|line| line.trim().strip_prefix("data:"))
        .map(str::trim)
        .filter(|p| !p.is_empty() && *p != "[DONE]")
        .filter_map(|p| serde_json::from_str::<Item>(p).ok())
        .filter(|x| !x.title.trim().is_empty() && !x.url.trim().is_empty())
        .collect()
}
fn item_type(item: &Item) -> crate::core::CloudType {
    use crate::core::CloudType;
    match item.disk_type.trim().to_ascii_lowercase().as_str() {
        "quark" => CloudType::Quark,
        "aliyun" | "alipan" => CloudType::Aliyun,
        "baidu" => CloudType::Baidu,
        "uc" => CloudType::Uc,
        "xunlei" => CloudType::Xunlei,
        "tianyi" => CloudType::Tianyi,
        "115" => CloudType::One15,
        "123" => CloudType::Pan123,
        "mobile" => CloudType::Mobile,
        "pikpak" => CloudType::Pikpak,
        "magnet" => CloudType::Magnet,
        "ed2k" => CloudType::Ed2k,
        _ => match item.is_type {
            0 => CloudType::Quark,
            1 => CloudType::Aliyun,
            2 => CloudType::Baidu,
            3 => CloudType::Uc,
            4 => CloudType::Xunlei,
            _ => CloudType::Others,
        },
    }
}
#[derive(Deserialize)]
struct Transfer {
    success: bool,
    #[serde(default)]
    data: TransferData,
}
#[derive(Default, Deserialize)]
struct TransferData {
    #[serde(default)]
    share_url: String,
    #[serde(default)]
    pwd: String,
    #[serde(default)]
    file_name: String,
}
fn transfer(body: &str) -> Result<TransferData, ProviderError> {
    let x: Transfer =
        serde_json::from_str(body).map_err(|e| ProviderError::Parse(e.to_string()))?;
    if x.success && !x.data.share_url.trim().is_empty() {
        Ok(x.data)
    } else {
        Err(ProviderError::Protocol("jupansou transfer failed".into()))
    }
}
async fn exchange(
    client: &reqwest::Client,
    base: &Url,
    cookie: &str,
    item: &Item,
) -> Result<TransferData, ProviderError> {
    if let Ok(u) = Url::parse(&item.url)
        && matches!(u.scheme(), "http" | "https")
    {
        let pwd = u
            .query_pairs()
            .find(|(k, _)| matches!(k.as_ref(), "pwd" | "password" | "passcode" | "code"))
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default();
        return Ok(TransferData {
            share_url: item.url.clone(),
            pwd,
            file_name: item.name.clone(),
        });
    }
    let boundary = "----pansou-rust-jupansou";
    let mut body = String::new();
    for (k, v) in [("link", item.url.as_str())]
        .into_iter()
        .chain((!item.disk_type.trim().is_empty()).then_some(("type", item.disk_type.as_str())))
    {
        body.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n"
        ))
    }
    body.push_str(&format!("--{boundary}--\r\n"));
    let u = base
        .join("api/transfer")
        .map_err(|e| ProviderError::Protocol(e.to_string()))?;
    let origin = base.origin().ascii_serialization();
    let referer = format!("{}/", base.as_str().trim_end_matches('/'));
    let mut request = client
        .post(u)
        .header("user-agent", UA)
        .header("accept", "application/json, text/plain, */*")
        .header("origin", origin)
        .header("referer", referer)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body);
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    let raw = request
        .send()
        .await
        .map_err(|e| ProviderError::Network(e.to_string()))?
        .error_for_status()
        .map_err(|e| ProviderError::Network(e.to_string()))?
        .text()
        .await
        .map_err(|e| ProviderError::Network(e.to_string()))?;
    transfer(&raw)
}
#[async_trait]
impl Provider for Jupansou {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("jupansou", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let session_url = self
            .endpoints
            .base
            .join("api/search/session")
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        let referer = format!("{}/", self.endpoints.base.as_str().trim_end_matches('/'));
        let session = ctx
            .client
            .get(session_url)
            .header("user-agent", UA)
            .header("accept", "application/json, text/plain, */*")
            .header("referer", &referer)
            .header("x-requested-with", "XMLHttpRequest")
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?
            .error_for_status()
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        let cookie = session
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|value| value.split(';').next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("; ");
        let mut u = self
            .endpoints
            .base
            .join("api/search/stream")
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        u.query_pairs_mut()
            .append_pair("keyword", query)
            .append_pair("type", "all");
        let mut request = ctx
            .client
            .get(u)
            .header("user-agent", UA)
            .header("accept", "text/event-stream")
            .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
            .header("origin", self.endpoints.base.origin().ascii_serialization())
            .header("referer", referer)
            .header("x-requested-with", "XMLHttpRequest");
        if !cookie.is_empty() {
            request = request.header("cookie", &cookie);
        }
        let body = request
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?
            .error_for_status()
            .map_err(|e| ProviderError::Network(e.to_string()))?
            .text()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        let q = query.to_lowercase();
        let items = sse(&body)
            .into_iter()
            .filter(|x| x.title.to_lowercase().contains(&q))
            .collect::<Vec<_>>();
        let base = self.endpoints.base.clone();
        let client = ctx.client.clone();
        let cookie = cookie.clone();
        let rows = stream::iter(items.into_iter().map(|item| {
            let base = base.clone();
            let client = client.clone();
            let cookie = cookie.clone();
            async move {
                let d = exchange(&client, &base, &cookie, &item).await.ok()?;
                let mut link = Link::new(d.share_url.trim());
                if matches!(link.cloud_type, crate::core::CloudType::Others) {
                    link.cloud_type = item_type(&item);
                }
                if !d.pwd.trim().is_empty() {
                    link.password = Some(d.pwd.trim().into())
                }
                link.work_title = Some(if d.file_name.trim().is_empty() {
                    item.title.trim().into()
                } else {
                    d.file_name.trim().into()
                });
                let id = format!("jupansou-{:x}", md5::compute(link.url.as_bytes()));
                Some(SearchResult {
                    id,
                    source: Source::provider("jupansou"),
                    datetime: Some(Utc::now()),
                    title: item.title.trim().into(),
                    content: "来源: 聚盘搜".into(),
                    tags: vec![link.cloud_type.to_string()],
                    links: vec![link],
                    images: vec![],
                })
            }
        }))
        .buffered(8)
        .collect::<Vec<_>>()
        .await;
        let mut seen = std::collections::HashSet::new();
        Ok(rows
            .into_iter()
            .flatten()
            .filter(|r| seen.insert(r.links[0].url.clone()))
            .collect())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_success() {
        let x = sse(include_str!(
            "../../tests/fixtures/providers/jupansou/success.sse"
        ));
        assert_eq!(x.len(), 1);
        assert_eq!(
            transfer(include_str!(
                "../../tests/fixtures/providers/jupansou/success-transfer.json"
            ))
            .unwrap()
            .pwd,
            "a1b2"
        )
    }
    #[test]
    fn fixture_empty() {
        assert!(
            sse(include_str!(
                "../../tests/fixtures/providers/jupansou/empty.sse"
            ))
            .is_empty()
        )
    }
    #[test]
    fn fixture_multiple_snapshots_and_done() {
        let rows = sse(include_str!(
            "../../tests/fixtures/providers/jupansou/multiple.sse"
        ));
        assert_eq!(rows.len(), 2);
        assert!(
            sse(include_str!(
                "../../tests/fixtures/providers/jupansou/done.sse"
            ))
            .is_empty()
        );
    }
    #[test]
    fn fixture_malformed() {
        assert!(
            sse(include_str!(
                "../../tests/fixtures/providers/jupansou/malformed.sse"
            ))
            .is_empty()
        );
        assert!(transfer("{").is_err())
    }
}
