use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{Link, ProviderError, SearchResult};
use async_trait::async_trait;
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, sync::LazyLock, time::Duration};
use url::Url;
static URL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"https?://[^\s\"'<>]+"#).unwrap());
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://xykmovie.com/").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Xiaoyu {
    endpoints: Endpoints,
}
impl Xiaoyu {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn text(e: ElementRef<'_>) -> String {
    e.text()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn clean_code(raw: &str) -> String {
    raw.trim()
        .trim_matches([
            '#', '，', ',', '。', '.', ';', '；', ':', '：', '(', ')', '（', '）', '[', ']', '【',
            '】',
        ])
        .into()
}
fn decode(raw: &str) -> Option<String> {
    let bytes = STANDARD
        .decode(raw.trim())
        .or_else(|_| STANDARD_NO_PAD.decode(raw.trim()))
        .ok()?;
    let raw = String::from_utf8(bytes).ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        None
    } else if raw.contains("://") {
        Some(raw.into())
    } else {
        Some(format!("https://{}", raw.trim_start_matches('/')))
    }
}
fn parse(body: &str) -> (Vec<SearchResult>, usize) {
    let doc = Html::parse_document(body);
    let pages = doc
        .select(&sel(".count strong"))
        .nth(1)
        .map(text)
        .and_then(|s| {
            s.split_once('/')
                .and_then(|(_, n)| n.trim().parse::<usize>().ok())
        })
        .unwrap_or(1)
        .clamp(1, 10);
    let rows = doc
        .select(&sel(".search-list .item"))
        .filter_map(|item| {
            let open = item.select(&sel(".name a.open")).next()?;
            let title = text(open);
            if title.is_empty() {
                return None;
            }
            let copy = item
                .select(&sel("a.copy[data-code]"))
                .next()
                .and_then(|a| a.value().attr("data-code"))
                .unwrap_or("");
            let raw = open
                .value()
                .attr("data-url")
                .and_then(decode)
                .or_else(|| URL_RE.find(copy).map(|m| m.as_str().to_owned()))?;
            let url = Url::parse(raw.trim()).ok()?;
            if !matches!(url.scheme(), "http" | "https")
                || !matches!(
                    url.host_str(),
                    Some(
                        "pan.quark.cn"
                            | "pan.baidu.com"
                            | "pan.xunlei.com"
                            | "drive.uc.cn"
                            | "www.alipan.com"
                            | "alipan.com"
                            | "www.aliyundrive.com"
                            | "aliyundrive.com"
                            | "cloud.189.cn"
                            | "115.com"
                            | "115cdn.com"
                            | "www.123pan.com"
                            | "123pan.com"
                            | "www.123684.com"
                            | "123684.com"
                            | "caiyun.139.com"
                            | "mypikpak.com"
                    )
                )
            {
                return None;
            }
            let mut link = Link::new(url.to_string());
            link.password = open
                .value()
                .attr("data-code")
                .map(clean_code)
                .filter(|s| !s.is_empty())
                .or_else(|| password(&link.url).map(|s| clean_code(&s)))
                .or_else(|| {
                    [
                        "提取码:",
                        "提取码：",
                        "访问码:",
                        "访问码：",
                        "密码:",
                        "密码：",
                    ]
                    .iter()
                    .find_map(|m| {
                        copy.split_once(m)?
                            .1
                            .split_whitespace()
                            .next()
                            .map(clean_code)
                    })
                });
            link.work_title = Some(title.clone());
            let id = open
                .value()
                .attr("data-id")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    Sha256::digest(format!("{title}\0{}", link.url).as_bytes())[..8]
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect()
                });
            let mut row = SearchResult::provider(format!("xiaoyu-{id}"), "xiaoyu");
            row.title = title;
            row.datetime = item
                .select(&sel(".atips"))
                .next()
                .map(text)
                .and_then(|s| date(&s))
                .or_else(|| Some(chrono::Utc::now()));
            row.content = item
                .select(&sel(".atips"))
                .nth(1)
                .map(text)
                .unwrap_or_default()
                .trim_start_matches("【内容】：")
                .trim()
                .into();
            row.links.push(link);
            Some(row)
        })
        .collect();
    (rows, pages)
}
impl Xiaoyu {
    async fn page(
        &self,
        ctx: &SearchContext,
        query: &str,
        page: usize,
    ) -> Result<(Vec<SearchResult>, usize), ProviderError> {
        let query =
            percent_encoding::utf8_percent_encode(query, percent_encoding::NON_ALPHANUMERIC);
        let url = self
            .endpoints
            .base
            .join(&format!("/s/{page}/{query}"))
            .unwrap();
        let body=tokio::time::timeout(ctx.request_timeout(Duration::from_secs(15)),request(||ctx.client.get(url.clone()).header("referer",self.endpoints.base.as_str()).header("user-agent","Mozilla/5.0 Chrome/124.0.0.0 Safari/537.36").header("accept","text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8"),3,Duration::from_millis(200),2*1024*1024)).await.map_err(|_|ProviderError::Timeout)??;
        let rows = parse(&String::from_utf8_lossy(&body));
        if !ctx.publish(rows.0.clone()) {
            return Err(ProviderError::Timeout);
        }
        Ok(rows)
    }
}
#[async_trait]
impl Provider for Xiaoyu {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("xiaoyu", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let query = query.split_whitespace().collect::<Vec<_>>().join(" ");
        if query.is_empty() {
            return Ok(vec![]);
        }
        let (mut out, pages) = self.page(ctx, &query, 1).await?;
        let mut pending = stream::iter((2..=pages).map(|p| self.page(ctx, &query, p))).buffered(5);
        let mut first_error = None;
        while let Some(result) = pending.next().await {
            match result {
                Ok((rows, _)) => out.extend(rows),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        if ctx.is_cancelled() {
            return Err(ProviderError::Timeout);
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        let mut seen = HashSet::new();
        out.retain(|row| {
            let l = &row.links[0];
            seen.insert((l.url.clone(), l.password.clone()))
        });
        Ok(out)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pages_and_links() {
        let h = r#"<div class="count"><strong>20</strong><strong>1/12</strong></div><div class="search-list"><div class="item"><div class="name"><a class="open" data-id="1" data-url="cGFuLnF1YXJrLmNuL3MvYQ==" data-code="abcd">movie</a></div><div class="atips">2026-10-20 12:00:00</div><div class="atips">【内容】：description</div></div></div>"#;
        let (r, p) = parse(h);
        assert_eq!(p, 10);
        assert_eq!(r[0].links[0].url, "https://pan.quark.cn/s/a");
        assert_eq!(r[0].links[0].password.as_deref(), Some("abcd"));
        assert_eq!(r[0].content, "description");
        assert!(parse("bad").0.is_empty());
        assert!(decode("bad!").is_none());
    }
}
