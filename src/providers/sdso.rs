use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, support::*};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use futures::{StreamExt, stream};
use serde::Deserialize;
use url::Url;
#[derive(Debug, Clone)]
pub struct Sdso {
    pub base: Url,
    pub pages_per_type: usize,
}
impl Default for Sdso {
    fn default() -> Self {
        Self {
            base: Url::parse("https://sdso.top/").unwrap(),
            pages_per_type: 2,
        }
    }
}
#[derive(Deserialize)]
struct Response {
    code: i32,
    data: Data,
}
#[derive(Deserialize)]
struct Data {
    list: Vec<Item>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    id: String,
    name: String,
    url: String,
    from: String,
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    gmt_share: String,
    #[serde(default)]
    creator_name: String,
    #[serde(default)]
    file_count: u64,
}
fn decrypt(raw: &str) -> Result<String, ProviderError> {
    let mut bytes = BASE64
        .decode(raw)
        .map_err(|_| ProviderError::Parse("sdso invalid ciphertext base64".into()))?;
    let plain =
        cbc::Decryptor::<aes::Aes128>::new(b"4OToScUFOaeVTrHE".into(), b"9CLGao1vHKqm17Oz".into())
            .decrypt_padded_mut::<Pkcs7>(&mut bytes)
            .map_err(|_| ProviderError::Parse("sdso invalid encrypted URL padding".into()))?;
    String::from_utf8(plain.to_vec())
        .map_err(|_| ProviderError::Parse("sdso invalid URL encoding".into()))
}
fn parse(bytes: &[u8], kind: &str, page: usize) -> Result<Vec<SearchResult>, ProviderError> {
    let response: Response =
        serde_json::from_slice(bytes).map_err(|e| ProviderError::Parse(e.to_string()))?;
    if response.code != 200 {
        return Err(ProviderError::Protocol(format!(
            "sdso API code {}",
            response.code
        )));
    }
    let mut rows = vec![];
    for item in response.data.list {
        let Ok(url) = decrypt(&item.url) else {
            continue;
        };
        let link = Link::new(url);
        if !matches!(
            link.cloud_type,
            CloudType::Baidu | CloudType::Quark | CloudType::Xunlei | CloudType::Aliyun
        ) || !matches!(
            item.from.as_str(),
            "baidu" | "quark" | "xunlei" | "ali" | "aliyun"
        ) {
            continue;
        }
        let mut row = SearchResult::provider(format!("sdso-{}-{kind}-{page}", item.id), "sdso");
        row.title = clean(
            &scraper::Html::parse_fragment(&item.name)
                .root_element()
                .text()
                .collect::<String>(),
        );
        row.content = format!(
            "分享者: {} | 文件数量: {} | 网盘类型: {kind}",
            item.creator_name, item.file_count
        );
        row.datetime = date(&item.gmt_share);
        row.tags = vec![item.from, item.r#type];
        row.links = vec![link];
        rows.push(row);
    }
    Ok(rows)
}
#[async_trait]
impl Provider for Sdso {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("sdso", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if ctx.is_cancelled() {
            return Err(ProviderError::Timeout);
        }
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut tasks = vec![];
        for kind in ["baidu", "quark", "xunlei", "ali"] {
            for page in 1..=self.pages_per_type.clamp(1, 5) {
                tasks.push((kind.to_owned(), page));
            }
        }
        let jobs = stream::iter(tasks.into_iter().map(|(kind, page)| async move {
            let mut url = self.base.join("api/sd/search").unwrap();
            url.query_pairs_mut()
                .append_pair("name", query)
                .append_pair("pageNo", &page.to_string())
                .append_pair("from", &kind);
            let bytes = body(
                ctx,
                || {
                    ctx.client
                        .get(url.clone())
                        .header("referer", self.base.as_str())
                },
                30,
                STANDARD,
            )
            .await?;
            parse(&bytes, &kind, page)
        }))
        .buffer_unordered(8);
        collect(ctx, jobs).await
    }
}
