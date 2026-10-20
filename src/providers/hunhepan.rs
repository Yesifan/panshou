//! Mixed-disk federation: four actual upstream APIs, three pages each.
use super::{UA, clean, fetch};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, time::Duration};
use url::Url;
#[derive(Debug, Clone)]
pub struct HunhepanProvider {
    apis: Vec<Url>,
}
impl Default for HunhepanProvider {
    fn default() -> Self {
        Self::new(
            [
                "https://hunhepan.com/open/search/disk",
                "https://qkpanso.com/v1/search/disk",
                "https://kuake8.com/v1/search/disk",
                "https://www.misoso.cc/v1/search/disk",
            ]
            .iter()
            .map(|s| Url::parse(s).unwrap())
            .collect(),
        )
    }
}
impl HunhepanProvider {
    pub fn new(apis: Vec<Url>) -> Self {
        Self { apis }
    }
}
#[derive(Deserialize)]
struct Response {
    code: i64,
    data: Data,
}
#[derive(Deserialize)]
struct Data {
    list: Vec<Item>,
}
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Item {
    disk_id: String,
    disk_name: String,
    disk_pass: String,
    disk_type: String,
    files: String,
    shared_time: String,
    link: String,
}
fn parse(body: &str) -> Result<Vec<Item>, ProviderError> {
    let r: Response = serde_json::from_str(body)
        .map_err(|_| ProviderError::Parse("hunhepan search: malformed JSON".into()))?;
    if r.code != 200 {
        return Err(ProviderError::Protocol(format!(
            "hunhepan search: API code {}",
            r.code
        )));
    }
    Ok(r.data.list)
}
fn convert(items: Vec<Item>) -> Vec<SearchResult> {
    let mut order = Vec::new();
    let mut unique: HashMap<String, Item> = HashMap::new();
    for mut item in items {
        item.disk_name = clean(&item.disk_name);
        let key = if !item.disk_id.is_empty() {
            item.disk_id.clone()
        } else {
            format!("{}|{}|{}", item.link, item.disk_name, item.disk_type)
        };
        if let Some(old) = unique.get(&key) {
            let score = item.files.len()
                + usize::from(old.disk_pass.is_empty() && !item.disk_pass.is_empty()) * 5
                + usize::from(old.shared_time.is_empty() && !item.shared_time.is_empty()) * 3;
            if score <= old.files.len() {
                continue;
            }
        } else {
            order.push(key.clone());
        }
        unique.insert(key, item);
    }
    order
        .into_iter()
        .filter_map(|key| {
            let item = unique.remove(&key)?;
            let url = Url::parse(&item.link).ok()?;
            if !matches!(url.scheme(), "http" | "https") {
                return None;
            }
            let ty = match item.disk_type.as_str() {
                "BDY" => CloudType::Baidu,
                "ALY" => CloudType::Aliyun,
                "QUARK" => CloudType::Quark,
                "TIANYI" => CloudType::Tianyi,
                "UC" => CloudType::Uc,
                "CAIYUN" => CloudType::Mobile,
                "115" => CloudType::One15,
                "XUNLEI" => CloudType::Xunlei,
                "123PAN" => CloudType::Pan123,
                "PIKPAK" => CloudType::Pikpak,
                _ => CloudType::Others,
            };
            let mut link = Link::new(item.link);
            link.cloud_type = ty;
            link.password = (!item.disk_pass.is_empty()).then_some(item.disk_pass);
            let id = if item.disk_id.is_empty() {
                format!("{:x}", Sha256::digest(key.as_bytes()))
            } else {
                item.disk_id
            };
            let mut r = SearchResult::provider(format!("hunhepan-{id}"), "hunhepan");
            r.title = item.disk_name;
            r.content = item.files;
            r.links = vec![link];
            r.datetime =
                chrono::NaiveDateTime::parse_from_str(&item.shared_time, "%Y-%m-%d %H:%M:%S")
                    .ok()
                    .map(|d| d.and_utc());
            Some(r)
        })
        .collect()
}
#[async_trait]
impl Provider for HunhepanProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("hunhepan", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut requests = Vec::new();
        for api in &self.apis {
            for page in 1..=3 {
                requests.push((api.clone(), page));
            }
        }
        let pages=stream::iter(requests.into_iter().map(|(api,page)|async move{
   let body=serde_json::json!({"page":page,"q":query,"user":"","exact":false,"format":[],"share_time":"","size":30,"type":"","exclude_user":[],"adv_params":{"wechat_pwd":"","platform":"pc"}});
   let origin=api.origin().ascii_serialization();
   let body=fetch(ctx.client.post(api.clone()).json(&body).header("User-Agent",UA).header("Accept","application/json, text/plain, */*").header("Referer",format!("{origin}/search")).header("Origin",origin),"hunhepan","page",ctx.timeout.min(Duration::from_secs(30)),16<<20,&[]).await?;
   let rows = parse(&body)?;
   ctx.publish(convert(rows.clone()));
   Ok::<_,ProviderError>(rows)
  })).buffered(12).collect::<Vec<_>>().await;
        let mut all = Vec::new();
        let mut error = None;
        let mut success = 0;
        for p in pages {
            match p {
                Ok(items) => {
                    success += 1;
                    all.extend(items)
                }
                Err(e) => {
                    error.get_or_insert(e);
                }
            }
        }
        if success == 0 {
            return Err(error
                .unwrap_or_else(|| ProviderError::Unavailable("hunhepan: no endpoints".into())));
        }
        let rows = convert(all);
        ctx.publish(rows.clone());
        if let Some(error) = error {
            return Err(error);
        }
        Ok(rows)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixtures() {
        let r = convert(
            parse(include_str!(
                "../../tests/fixtures/providers/hunhepan/success.json"
            ))
            .unwrap(),
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "测试影片");
        assert_eq!(r[0].links[0].password.as_deref(), Some("abcd"));
        assert!(
            parse(include_str!(
                "../../tests/fixtures/providers/hunhepan/empty.json"
            ))
            .unwrap()
            .is_empty()
        );
        assert!(
            parse(include_str!(
                "../../tests/fixtures/providers/hunhepan/malformed.json"
            ))
            .is_err()
        );
    }
}
