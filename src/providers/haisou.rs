//! 海搜 search-page HSIDs are resolved through the fetch API, never returned as shares.
use super::{UA, clean, fetch};
use crate::core::{Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use serde::Deserialize;
use std::{collections::HashSet, time::Duration};
use url::Url;
#[derive(Debug, Clone)]
pub struct HaisouProvider {
    base: Url,
}
impl Default for HaisouProvider {
    fn default() -> Self {
        Self::new(Url::parse("https://haisou.cc/").unwrap())
    }
}
impl HaisouProvider {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
}
#[derive(Deserialize)]
struct Page {
    code: i64,
    data: Data,
}
#[derive(Deserialize)]
struct Data {
    list: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    hsid: String,
    platform: String,
    share_name: String,
    #[serde(default)]
    stat_file: u64,
    #[serde(default)]
    stat_size: u64,
}
#[derive(Deserialize)]
struct Share {
    code: i64,
    data: ShareData,
}
#[derive(Deserialize)]
struct ShareData {
    share_code: String,
    #[serde(default)]
    share_pwd: Option<String>,
}
fn parse(body: &str) -> Result<Vec<Item>, ProviderError> {
    let r: Page = serde_json::from_str(body)
        .map_err(|_| ProviderError::Parse("haisou page: malformed JSON".into()))?;
    if r.code != 0 {
        return Err(ProviderError::Protocol(format!(
            "haisou page: API code {}",
            r.code
        )));
    }
    Ok(r.data.list)
}
fn resolve(body: &str, item: Item) -> Result<SearchResult, ProviderError> {
    let data: Share = serde_json::from_str(body)
        .map_err(|_| ProviderError::Parse("haisou fetch: malformed JSON".into()))?;
    if data.code != 0 {
        return Err(ProviderError::Protocol(format!(
            "haisou fetch: API code {}",
            data.code
        )));
    }
    let base = match item.platform.as_str() {
        "ali" => "https://www.alipan.com/s/",
        "baidu" => "https://pan.baidu.com/s/",
        "quark" => "https://pan.quark.cn/s/",
        "xunlei" => "https://pan.xunlei.com/s/",
        "tianyi" => "https://cloud.189.cn/t/",
        _ => {
            return Err(ProviderError::Parse(
                "haisou fetch: unsupported platform".into(),
            ));
        }
    };
    if data.data.share_code.is_empty()
        || !data
            .data
            .share_code
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    {
        return Err(ProviderError::Parse(
            "haisou fetch: invalid share code".into(),
        ));
    }
    let mut link = Link::new(format!("{base}{}", data.data.share_code));
    link.password = data.data.share_pwd.filter(|p| !p.is_empty());
    let mut r = SearchResult::provider(format!("haisou-{}", item.hsid), "haisou");
    r.title = clean(&item.share_name);
    r.content = format!(
        "文件数量: {} | 网盘类型: {} | 大小: {} bytes",
        item.stat_file, item.platform, item.stat_size
    );
    r.tags.push(item.platform);
    r.links.push(link);
    Ok(r)
}
#[async_trait]
impl Provider for HaisouProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("haisou", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        q: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let get = |url: Url, stage: &'static str, seconds| {
            fetch(
                ctx.client
                    .get(url)
                    .header("User-Agent", UA)
                    .header("Accept", "application/json, text/plain, */*")
                    .header("Referer", self.base.as_str()),
                "haisou",
                stage,
                ctx.request_timeout(Duration::from_secs(seconds)),
                16 << 20,
                &[200, 400],
            )
        };
        let mut tasks = Vec::new();
        for pan in ["ali", "baidu", "quark", "xunlei", "tianyi"] {
            for page in 1..=2 {
                tasks.push((pan.to_owned(), page));
            }
        }
        let pages = stream::iter(tasks.into_iter().map(|(pan, page)| async move {
            let mut url = self.base.join("api/pan/share/search").unwrap();
            url.query_pairs_mut()
                .append_pair("query", q)
                .append_pair("scope", "title")
                .append_pair("pan", &pan)
                .append_pair("page", &page.to_string())
                .append_pair("filter_valid", "true")
                .append_pair("filter_has_files", "false");
            parse(&get(url, "search page", 30).await?)
        }))
        .buffered(10)
        .collect::<Vec<_>>()
        .await;
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        let mut error = None;
        let mut success = 0;
        for page in pages {
            match page {
                Ok(rows) => {
                    success += 1;
                    items.extend(
                        rows.into_iter()
                            .filter(|i| !i.hsid.is_empty() && seen.insert(i.hsid.clone())),
                    );
                }
                Err(e) => {
                    error.get_or_insert(e);
                }
            }
        }
        if success == 0 {
            return Err(error.unwrap());
        }
        let failures = &std::sync::Mutex::new(None);
        let rows = stream::iter(items.into_iter().map(|item| async move {
            let mut url = self.base.join("api/pan/share/").ok()?;
            url.path_segments_mut()
                .ok()?
                .pop_if_empty()
                .push(&item.hsid)
                .push("fetch");
            let body = super::remember_failure(get(url, "share fetch", 15).await, failures)?;
            let r = resolve(&body, item).ok()?;
            ctx.publish(vec![r.clone()]);
            Some(r)
        }))
        .buffered(20)
        .collect::<Vec<_>>()
        .await;
        if let Some(detail_error) = failures.lock().unwrap().take() {
            return Err(error.unwrap_or(detail_error));
        }
        if error.is_some() || rows.iter().any(Option::is_none) {
            return Err(error.unwrap_or_else(|| {
                ProviderError::Unavailable("haisou: one or more shares failed".into())
            }));
        }
        Ok(rows.into_iter().flatten().collect())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixtures() {
        let item = parse(include_str!(
            "../../tests/fixtures/providers/haisou/success.json"
        ))
        .unwrap()
        .remove(0);
        let r = resolve(
            include_str!("../../tests/fixtures/providers/haisou/share.json"),
            item,
        )
        .unwrap();
        assert_eq!(r.links[0].url, "https://pan.quark.cn/s/realshare");
        assert_eq!(r.links[0].password.as_deref(), Some("abcd"));
        assert!(
            parse(include_str!(
                "../../tests/fixtures/providers/haisou/empty.json"
            ))
            .unwrap()
            .is_empty()
        );
        assert!(
            parse(include_str!(
                "../../tests/fixtures/providers/haisou/malformed.json"
            ))
            .is_err()
        );
    }
}
