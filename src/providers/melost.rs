//! 影盘社 three-page JSON search, preserving the API's exact browser payload.
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use chrono::NaiveDateTime;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::Html;
use serde::Deserialize;
use serde_json::Value;
use std::{collections::HashMap, sync::LazyLock, time::Duration};
use url::Url;
#[derive(Debug, Clone)]
pub struct Melost {
    endpoint: Url,
}
impl Default for Melost {
    fn default() -> Self {
        Self::new(Url::parse("https://www.melost.cn/v1/search/disk").unwrap())
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
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
struct Item {
    disk_id: String,
    disk_name: String,
    disk_pass: String,
    disk_type: String,
    files: String,
    share_user: String,
    shared_time: String,
    tags: Value,
    link: String,
}
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());
fn clean(s: &str) -> String {
    let plain = Html::parse_fragment(s)
        .root_element()
        .text()
        .collect::<String>();
    TAG.replace_all(&plain, "")
        .replace("\r\n", "\n")
        .trim()
        .to_owned()
}
fn key(item: &Item) -> String {
    if !item.disk_id.is_empty() {
        item.disk_id.clone()
    } else if !item.link.is_empty() {
        item.link.clone()
    } else {
        format!("{}|{}", item.disk_name, item.disk_type)
    }
}
fn score(i: &Item) -> usize {
    i.files.len()
        + usize::from(!i.disk_pass.trim().is_empty()) * 5
        + usize::from(!i.shared_time.trim().is_empty()) * 3
        + usize::from(!i.share_user.trim().is_empty()) * 2
        + usize::from(!i.tags.is_null()) * 2
}
fn convert(i: &Item, query: &str) -> Option<SearchResult> {
    if i.link.trim().is_empty() {
        return None;
    }
    let title = clean(&i.disk_name);
    let files = clean(&i.files);
    let tags = i
        .tags
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut parts = Vec::new();
    if !files.is_empty() {
        parts.push(files);
    }
    if !i.share_user.trim().is_empty() {
        parts.push(format!("分享用户: {}", i.share_user.trim()));
    }
    if !tags.is_empty() {
        parts.push(format!("标签: {}", tags.join("、")));
    }
    let content = parts.join("\n");
    let hay = format!("{title} {content}").to_lowercase();
    if !query
        .to_lowercase()
        .split_whitespace()
        .all(|w| hay.contains(w))
    {
        return None;
    }
    let mut r = SearchResult::provider(
        if i.disk_id.is_empty() {
            format!("melost-{:x}", md5::compute(key(i)))
        } else {
            format!("melost-{}", i.disk_id)
        },
        "melost",
    );
    r.title = title;
    r.content = content;
    r.tags = tags;
    r.datetime = NaiveDateTime::parse_from_str(i.shared_time.trim(), "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|d| d.and_utc());
    let mut link = Link::new(i.link.trim());
    link.cloud_type = match i.disk_type.trim().to_uppercase().as_str() {
        "BDY" | "BAIDU" => CloudType::Baidu,
        "ALY" | "ALIYUN" => CloudType::Aliyun,
        "QUARK" => CloudType::Quark,
        "TIANYI" => CloudType::Tianyi,
        "UC" => CloudType::Uc,
        "CAIYUN" | "MOBILE" => CloudType::Mobile,
        "115" => CloudType::One15,
        "XUNLEI" => CloudType::Xunlei,
        "123" | "123PAN" => CloudType::Pan123,
        "PIKPAK" => CloudType::Pikpak,
        "LANZOU" => CloudType::Lanzou,
        _ => link.cloud_type,
    };
    if !i.disk_pass.trim().is_empty() {
        link.password = Some(i.disk_pass.trim().to_owned());
    }
    link.work_title = Some(r.title.clone());
    r.links.push(link);
    Some(r)
}
fn parse(body: &str) -> Result<Vec<Item>, ProviderError> {
    let r: Response =
        serde_json::from_str(body).map_err(|e| ProviderError::Parse(e.to_string()))?;
    if r.code != 200 {
        return Err(ProviderError::Protocol(format!(
            "melost API code={}",
            r.code
        )));
    }
    Ok(r.data.list)
}
impl Melost {
    pub fn new(endpoint: Url) -> Self {
        Self { endpoint }
    }
    async fn page(
        &self,
        ctx: &SearchContext,
        query: &str,
        page: u8,
    ) -> Result<Vec<Item>, ProviderError> {
        let payload = serde_json::json!({"page":page,"q":query,"user":"","exact":false,"user_distinct":false,"format":[],"share_time":"","share_year":"","size":30,"order":"","type":"","search_ticket":"","exclude_user":[],"adv_params":{"wechat_pwd":"","search_code":"","platform":"pc","fp_data":"","automated":"0"}});
        let body = crate::providers::b_registry::http::text(
            ctx.client
                .post(self.endpoint.clone())
                .header("accept", "application/json, text/plain, */*")
                .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
                .header("origin", "https://www.melost.cn")
                .header("referer", "https://www.melost.cn/search")
                .header("user-agent", "Mozilla/5.0 Chrome/136.0.0.0 Safari/537.36")
                .json(&payload),
            1,
            0,
            false,
            Duration::from_secs(30),
            16 << 20,
        )
        .await?;
        let items = parse(&body)?;
        ctx.publish(items.iter().filter_map(|i| convert(i, query)).collect());
        Ok(items)
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut pages = stream::iter((1..=3).map(|p| self.page(ctx, query, p))).buffer_unordered(3);
        let mut all = HashMap::<String, Item>::new();
        let mut error = None;
        while let Some(page) = pages.next().await {
            match page {
                Ok(items) => {
                    for i in items {
                        let k = key(&i);
                        if all.get(&k).is_none_or(|old| score(&i) > score(old)) {
                            all.insert(k, i);
                        }
                    }
                }
                Err(e) => error = Some(e),
            }
        }
        if let Some(e) = error {
            return Err(e);
        }
        let mut rows = all
            .values()
            .filter_map(|i| convert(i, query))
            .collect::<Vec<_>>();
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(rows)
    }
}
#[async_trait]
impl Provider for Melost {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("melost", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        tokio::time::timeout(ctx.timeout, self.run(ctx, query))
            .await
            .map_err(|_| ProviderError::Timeout)?
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn success_empty_malformed() {
        let items=parse(r#"{"code":200,"data":{"list":[{"disk_id":"1","disk_name":"<em>Film</em>","disk_type":"BDY","disk_pass":"1234","link":"https://pan.baidu.com/s/abc","shared_time":"2026-10-12 12:00:00","files":"Movie.mkv","tags":["HD",1,""]}]}}"#).unwrap();
        let r = convert(&items[0], "film").unwrap();
        assert_eq!(r.title, "Film");
        assert_eq!(r.tags, ["HD"]);
        assert_eq!(r.links[0].password.as_deref(), Some("1234"));
        assert!(r.datetime.is_some());
        assert!(
            parse(r#"{"code":200,"data":{"list":[]}}"#)
                .unwrap()
                .is_empty()
        );
        assert!(parse("{}").is_err());
    }
    #[tokio::test]
    async fn all_pages_exact_payload_and_dedup() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{body_partial_json, header, method},
        };
        let s = MockServer::start().await;
        for p in 1..=3 {
            Mock::given(method("POST")).and(body_partial_json(serde_json::json!({"page":p,"q":"film","size":30,"adv_params":{"automated":"0","platform":"pc"}}))).and(header("origin","https://www.melost.cn")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"code":200,"data":{"list":[{"disk_id":"1","disk_name":"Film","link":"https://pan.baidu.com/s/abc","disk_pass":if p==2 {"abcd"} else {""}}]}}))).expect(1).mount(&s).await;
        }
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(3));
        let rows = Melost::new(Url::parse(&s.uri()).unwrap())
            .search(&ctx, "film")
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].links[0].password.as_deref(), Some("abcd"));
    }
}
