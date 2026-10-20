use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use async_trait::async_trait;
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
    time::Duration,
};
use url::Url;
const KEY: &[u8] = b"pWz1vnL1fTkOvTMW3f9M1jJWfneUIh50";
static DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}").unwrap());
static TYPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/assets/xyso/(\d+)\.png").unwrap());
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://www.yunso.net/").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Yunso {
    endpoints: Endpoints,
}
impl Yunso {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
fn normalize(raw: &str) -> String {
    let raw = raw.trim();
    if let Ok(mut url) = Url::parse(raw)
        && url.query() == Some("")
    {
        url.set_query(None);
        return url.to_string();
    }
    raw.into()
}
fn decrypt(raw: &str) -> Option<String> {
    let raw = Html::parse_fragment(raw)
        .root_element()
        .text()
        .collect::<String>();
    let raw = raw.trim();
    if raw.to_ascii_lowercase().starts_with("http://")
        || raw.to_ascii_lowercase().starts_with("https://")
    {
        return Some(normalize(raw));
    }
    let decoded = STANDARD
        .decode(raw)
        .or_else(|_| STANDARD_NO_PAD.decode(raw))
        .ok()?;
    if let Ok(raw) = std::str::from_utf8(&decoded)
        && Url::parse(raw.trim()).is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
    {
        return Some(normalize(raw));
    }
    let bytes = decoded
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ KEY[i % KEY.len()])
        .collect::<Vec<_>>();
    Some(normalize(std::str::from_utf8(&bytes).ok()?))
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn text(e: ElementRef<'_>) -> String {
    e.text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn first(e: ElementRef<'_>, s: &str) -> String {
    e.select(&sel(s)).next().map(text).unwrap_or_default()
}
fn parse(body: &str) -> Vec<(SearchResult, i32)> {
    let doc = Html::parse_fragment(body);
    let mut rows = vec![];
    for card in doc.select(&sel("div.layui-card[data-qid]")) {
        let Some(anchor) = card.select(&sel("a[onclick*='open_sid']")).next() else {
            continue;
        };
        let title = text(anchor);
        if title.is_empty() {
            continue;
        }
        let Some(url) = anchor.value().attr("url").and_then(decrypt) else {
            continue;
        };
        if !Url::parse(&url)
            .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
        {
            continue;
        }
        let qid = card.value().attr("data-qid").unwrap_or("");
        let id = anchor
            .value()
            .attr("id")
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{qid}-{}", rows.len()));
        let mut row = SearchResult::provider(format!("yunso-{id}"), "yunso");
        row.title = title;
        row.datetime = DATE
            .find(&first(card, ".layui-card-header"))
            .and_then(|m| date(m.as_str()));
        let mut link = Link::new(url);
        link.password = anchor
            .value()
            .attr("pa")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                Url::parse(&link.url)
                    .ok()?
                    .query_pairs()
                    .find_map(|(k, v)| {
                        (["pwd", "pass", "password"].contains(&k.as_ref()) && !v.trim().is_empty())
                            .then(|| v.trim().into())
                    })
            });
        link.datetime = row.datetime;
        let mut type_code = String::new();
        let mut type_name = String::new();
        if let Some(img) = card.select(&sel("img[src*='/assets/xyso/']")).next() {
            type_name = img.value().attr("alt").unwrap_or("").trim().into();
            type_code = img
                .value()
                .attr("src")
                .and_then(|s| TYPE.captures(s))
                .map(|c| c[1].into())
                .unwrap_or_default()
        }
        link.cloud_type = match type_code.as_str() {
            "1" => CloudType::Baidu,
            "20100" => CloudType::Aliyun,
            "20500" => CloudType::Quark,
            "20000" => CloudType::Tianyi,
            "20300" => CloudType::Mobile,
            "20400" => CloudType::Xunlei,
            "20501" => CloudType::Uc,
            "20600" => CloudType::Lanzou,
            _ => link.cloud_type,
        };
        let preview = first(card, "p.result.container.p");
        let mut summary = first(card, ".layui-card-body span");
        if summary.to_uppercase().contains("N/A") {
            summary.clear()
        }
        let score = 8
            + if link.password.is_some() { 5 } else { 0 }
            + if row.datetime.is_some() { 3 } else { 0 }
            + if !summary.is_empty() { 2 } else { 0 }
            + if !preview.is_empty() { 2 } else { 0 }
            + if !type_code.is_empty() { 1 } else { 0 };
        let mut parts = vec![];
        if !preview.is_empty() {
            parts.push(preview)
        }
        if !summary.is_empty() {
            parts.push(summary)
        }
        if !type_name.is_empty() {
            parts.push(format!("网盘: {type_name}"));
            row.tags.push(type_name)
        }
        row.content = parts.join("\n");
        let mut seen = row.tags.iter().cloned().collect::<HashSet<_>>();
        for badge in card.select(&sel(".badge")).map(text) {
            if !badge.is_empty() && seen.insert(badge.clone()) {
                row.tags.push(badge)
            }
        }
        row.links.push(link);
        rows.push((row, score));
    }
    rows
}
impl Yunso {
    async fn page(
        &self,
        ctx: &SearchContext,
        query: &str,
        page: usize,
    ) -> Result<Vec<(SearchResult, i32)>, ProviderError> {
        let mut url = self.endpoints.base.join("/api/Core/search2").unwrap();
        url.query_pairs_mut().extend_pairs([
            ("requestID", ""),
            ("mode", "90002"),
            ("scope_content", "0"),
            ("stype", ""),
            ("wd", query),
            ("uk", ""),
            ("page", &page.to_string()),
            ("limit", "20"),
            ("screen_filetype", ""),
        ]);
        let mut referer = self.endpoints.base.join("/index/user/s").unwrap();
        referer.query_pairs_mut().append_pair("wd", query);
        let body = request(
            || {
                ctx.client
                    .post(url.clone())
                    .timeout(ctx.timeout.min(Duration::from_secs(30)))
                    .header("origin", self.endpoints.base.origin().ascii_serialization())
                    .header("referer", referer.as_str())
                    .header("accept", "application/json, text/plain, */*")
                    .header("x-requested-with", "XMLHttpRequest")
            },
            1,
            Duration::ZERO,
            BODY_LIMIT,
        )
        .await?;
        #[derive(Deserialize)]
        struct Response {
            code: i32,
            data: String,
        }
        let r: Response =
            serde_json::from_slice(&body).map_err(|e| ProviderError::Parse(e.to_string()))?;
        if r.code != 0 {
            return Err(ProviderError::Protocol(format!(
                "yunso API code {}",
                r.code
            )));
        }
        let rows = parse(&r.data);
        if !ctx.publish(rows.iter().map(|(row, _)| row.clone()).collect()) {
            return Err(ProviderError::Timeout);
        }
        Ok(rows)
    }
}
#[async_trait]
impl Provider for Yunso {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("yunso", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut pending = stream::iter((1..=3).map(|p| self.page(ctx, query, p))).buffered(3);
        let mut rows: Vec<(SearchResult, i32)> = vec![];
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut first_error = None;
        while let Some(result) = pending.next().await {
            match result {
                Ok(items) => {
                    for (row, score) in items {
                        let key = row.links[0].url.clone();
                        if let Some(&i) = index.get(&key) {
                            if rows[i].1 < score {
                                rows[i] = (row, score)
                            }
                        } else {
                            index.insert(key, rows.len());
                            rows.push((row, score));
                        }
                    }
                }
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        if ctx.is_cancelled() {
            return Err(ProviderError::Timeout);
        }
        if let Some(e) = first_error {
            return Err(e);
        }
        Ok(rows.into_iter().map(|(r, _)| r).collect())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ten_normalizations() {
        for (raw, want) in [
            ("https://pan.quark.cn/s/a?", "https://pan.quark.cn/s/a"),
            (" https://pan.quark.cn/s/a? ", "https://pan.quark.cn/s/a"),
            (
                "https://pan.quark.cn/s/a?#files",
                "https://pan.quark.cn/s/a#files",
            ),
            (
                "https://pan.baidu.com/s/a?pwd=abcd&sort=name",
                "https://pan.baidu.com/s/a?pwd=abcd&sort=name",
            ),
            (
                "https://pan.quark.cn/s/a?note=what?",
                "https://pan.quark.cn/s/a?note=what?",
            ),
            (
                "https://pan.quark.cn/s/a#files?",
                "https://pan.quark.cn/s/a#files?",
            ),
            ("https://pan.quark.cn/s/a%3F", "https://pan.quark.cn/s/a%3F"),
        ] {
            assert_eq!(decrypt(raw).unwrap(), want)
        }
        let raw = "https://pan.quark.cn/s/a?";
        for encoded in [
            STANDARD.encode(raw),
            STANDARD_NO_PAD.encode(raw),
            STANDARD.encode(
                raw.bytes()
                    .enumerate()
                    .map(|(i, b)| b ^ KEY[i % KEY.len()])
                    .collect::<Vec<_>>(),
            ),
        ] {
            assert_eq!(decrypt(&encoded).unwrap(), "https://pan.quark.cn/s/a");
        }
    }
    #[test]
    fn html_three_cards() {
        let h = r#"<div class="layui-card" data-qid="a"><a onclick="open_sid()" id="a" url="https://pan.quark.cn/s/a?" pa="abcd">movie</a></div><div class="layui-card" data-qid="b"><a onclick="open_sid()" url="https://pan.quark.cn/s/a">movie</a></div><div class="layui-card" data-qid="c"><a onclick="open_sid()" url="https://pan.baidu.com/s/b?pwd=efgh&amp;sort=name">movie</a></div>"#;
        let rows = parse(h);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].0.links[0].url, rows[1].0.links[0].url);
        assert_eq!(rows[0].0.links[0].password.as_deref(), Some("abcd"));
        assert_eq!(rows[2].0.links[0].password.as_deref(), Some("efgh"));
        assert!(parse("bad").is_empty());
    }
}
