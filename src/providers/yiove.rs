use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use async_trait::async_trait;
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use std::{collections::HashSet, sync::LazyLock, time::Duration};
use url::Url;
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"thread-(\d+)").unwrap());
static SHARE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^(?:https?://(?:pan\.quark\.cn/[sg]/[0-9A-Za-z]+|pan\.(?:baidu|xunlei)\.com/s/[0-9A-Za-z_?=&\-]+|(?:www\.)?(?:aliyundrive\.com|alipan\.com)/s/[0-9A-Za-z]+|drive\.uc\.cn/s/[0-9A-Za-z]+|(?:www\.)?(?:123pan\.com|123pan\.cn|123684\.com|123685\.com|123912\.com|123592\.com)/s/[0-9A-Za-z]+|(?:www\.)?mypikpak\.com/s/[0-9A-Za-z]+|(?:caiyun\.139\.com|tianyi\.cloud)/[^\s<>\"']+)|magnet:\?xt=urn:btih:[0-9A-Za-z]+|ed2k://[^\s<>\"']+)"#).unwrap()
});
static URL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"https?://[^\s<>\"']+"#).unwrap());
static PWD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:提取码[:：]?\s*|密码[:：]?\s*|pwd\s*[=:：]\s*|code\s*[=:：]\s*)([0-9A-Za-z]+)")
        .unwrap()
});
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://bbs.yiove.com/").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Yiove {
    endpoints: Endpoints,
}
impl Yiove {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn text(e: ElementRef<'_>) -> String {
    e.descendants()
        .filter(|n| {
            !n.ancestors().any(|a| {
                a.value()
                    .as_element()
                    .is_some_and(|e| matches!(e.name(), "script" | "style"))
            })
        })
        .filter_map(|n| n.value().as_text())
        .map(|s| s.text.to_string())
        .collect::<String>()
        .trim()
        .into()
}
fn crc32(raw: &str) -> u32 {
    let mut c = !0u32;
    for b in raw.bytes() {
        c ^= b as u32;
        for _ in 0..8 {
            c = (c >> 1) ^ if c & 1 != 0 { 0xedb88320 } else { 0 }
        }
    }
    !c
}
fn list(body: &str, base: &Url) -> Vec<(Url, SearchResult)> {
    let d = Html::parse_document(body);
    d.select(&sel("ul.threadlist li.thread"))
        .filter_map(|e| {
            let a = e.select(&sel(".subject a")).next()?;
            let title = text(a);
            if title.is_empty() {
                return None;
            }
            let url = base
                .join(a.value().attr("href")?.trim_start_matches("./"))
                .ok()?;
            let id = ID
                .captures(url.as_str())
                .map(|c| c[1].to_owned())
                .unwrap_or_else(|| crc32(url.as_str()).to_string());
            let mut r = SearchResult::provider(format!("yiove-{id}"), "yiove");
            r.title = title;
            r.tags = e
                .select(&sel(".subject a.badge"))
                .map(text)
                .filter(|s| !s.is_empty())
                .collect();
            Some((url, r))
        })
        .take(12)
        .collect()
}
fn link(raw: &str, context: &str) -> Option<Link> {
    let raw = raw.trim();
    if !SHARE.is_match(raw) {
        return None;
    }
    let mut l = Link::new(raw);
    if let Ok(u) = Url::parse(raw)
        && u.host_str() == Some("tianyi.cloud")
    {
        l.cloud_type = CloudType::Tianyi
    }
    l.password = password(raw).or_else(|| PWD.captures(context).map(|c| c[1].into()));
    Some(l)
}
fn detail(body: &str, row: &mut SearchResult) {
    let doc = Html::parse_document(body);
    let root = doc.root_element();
    let content = doc
        .select(&sel("div.message[isfirst='1']"))
        .next()
        .or_else(|| doc.select(&sel(".message")).next())
        .unwrap_or(root);
    let mut seen = HashSet::new();
    for a in content.select(&sel("a[href]")) {
        let mut context = text(a);
        context.push_str(a.value().attr("title").unwrap_or(""));
        if let Some(parent) = a.parent().and_then(ElementRef::wrap) {
            context.push_str(&text(parent));
            if let Some(next) = parent.next_sibling().and_then(ElementRef::wrap) {
                context.push_str(&text(next))
            }
        }
        if let Some(next) = a.next_sibling().and_then(ElementRef::wrap) {
            context.push_str(&text(next))
        }
        if let Some(l) = link(a.value().attr("href").unwrap(), &context)
            && seen.insert(l.url.clone())
        {
            row.links.push(l)
        }
    }
    let plain = text(content);
    for m in URL_RE.find_iter(&plain) {
        let mut start = m.start().saturating_sub(80);
        let mut end = (m.end() + 80).min(plain.len());
        while !plain.is_char_boundary(start) {
            start += 1
        }
        while !plain.is_char_boundary(end) {
            end -= 1
        }
        if let Some(l) = link(m.as_str(), &plain[start..end])
            && seen.insert(l.url.clone())
        {
            row.links.push(l)
        }
    }
    row.links.truncate(6);
    for l in &mut row.links {
        l.work_title = Some(row.title.clone())
    }
    row.content = doc
        .select(&sel("meta[name='description']"))
        .next()
        .and_then(|e| e.value().attr("content"))
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().into())
        .unwrap_or_else(|| plain.chars().take(200).collect());
    let mut tags = row.tags.iter().cloned().collect::<HashSet<_>>();
    for tag in doc
        .select(&sel(".breadcrumb a, ol.breadcrumb a, h4 a.badge"))
        .map(text)
        .filter(|s| !s.is_empty() && !s.contains("首页"))
    {
        if tags.insert(tag.clone()) {
            row.tags.push(tag)
        }
    }
    row.datetime = doc
        .select(&sel(".card-thread .date"))
        .next()
        .map(text)
        .and_then(|s| date(&s).or_else(|| date(&s.replace('/', "-"))))
        .or_else(|| Some(chrono::Utc::now()));
}
impl Yiove {
    async fn fetch(&self, ctx: &SearchContext, url: Url) -> Result<Vec<u8>, ProviderError> {
        tokio::time::timeout(ctx.request_timeout(Duration::from_secs(12)),request(||ctx.client.get(url.clone()).header("referer",url.as_str()).header("user-agent","Mozilla/5.0 Chrome/120.0.0.0 Safari/537.36").header("accept","text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8"),3,Duration::from_millis(200),BODY_LIMIT)).await.map_err(|_|ProviderError::Timeout)?
    }
}
#[async_trait]
impl Provider for Yiove {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("yiove", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let encoded = query
            .trim()
            .as_bytes()
            .iter()
            .map(|b| format!("_{b:02X}"))
            .collect::<String>();
        let url = self
            .endpoints
            .base
            .join(&format!("/search-{encoded}-1.htm"))
            .unwrap();
        let body = self.fetch(ctx, url).await?;
        let rows = list(&String::from_utf8_lossy(&body), &self.endpoints.base);
        if rows.is_empty() {
            return Ok(vec![]);
        }
        collect_details(
            ctx,
            rows.into_iter().map(|(url, mut row)| async move {
                let body = self.fetch(ctx, url).await?;
                detail(&String::from_utf8_lossy(&body), &mut row);
                Ok(row)
            }),
            6,
        )
        .await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forum_detail() {
        let mut row = SearchResult::provider("yiove-1", "yiove");
        row.title = "movie".into();
        detail(
            r#"<div class="message" isfirst="1"><script>https://pan.baidu.com/s/ignored</script><p><a href="https://pan.quark.cn/g/abc">link</a>提取码：abcd</p></div><div class="breadcrumb"><a>首页</a><a>电影</a></div>"#,
            &mut row,
        );
        assert_eq!(row.links.len(), 1);
        assert_eq!(row.links[0].password.as_deref(), Some("abcd"));
        assert_eq!(row.links[0].work_title.as_deref(), Some("movie"));
        assert_eq!(row.tags, vec!["电影"]);
        assert!(list("bad", &Endpoints::default().base).is_empty());
        assert_eq!(crc32("123456789"), 0xcbf43926);
    }
}
