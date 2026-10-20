//! Transport, URL validation and owned per-command detail memo shared by two
//! HTML adapters. DOM and episode interpretation intentionally remain separate.
use crate::{
    core::{CloudType, Link, ProviderError, SearchResult, Source},
    providers::SearchContext,
};
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};
use tokio::{sync::Semaphore, time::Instant};
use url::Url;

pub(super) const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/131.0 Safari/537.36";
pub(super) fn parse_error(message: &str) -> ProviderError {
    ProviderError::Parse(message.into())
}
pub(super) fn network(_: impl std::fmt::Display) -> ProviderError {
    ProviderError::Network("upstream HTTP request failed".into())
}
pub(super) fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("static provider selector")
}
pub(super) fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
pub(super) fn text(node: ElementRef<'_>) -> String {
    clean(&node.text().collect::<String>())
}
pub(super) fn first(node: ElementRef<'_>, css: &str) -> String {
    node.select(&selector(css))
        .next()
        .map(text)
        .unwrap_or_default()
}
pub(super) fn unique(values: &mut Vec<String>, value: String) {
    if !value.is_empty() && !values.contains(&value) {
        values.push(value);
    }
}
pub(super) fn words_match(title: &str, query: &str) -> bool {
    let title = title.to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|word| title.contains(word))
}
pub(super) fn image(node: ElementRef<'_>, base: &Url) -> Option<String> {
    for key in ["data-original", "data-src", "src"] {
        if let Some(raw) = node
            .value()
            .attr(key)
            .filter(|s| !s.trim().is_empty() && !s.contains("load.gif"))
            && let Ok(url) = base.join(raw)
            && matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none()
        {
            return Some(url.to_string());
        }
    }
    None
}
static DETAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/(?:index\.php/)?vod/detail/id/([0-9]+)\.html$").unwrap());
static SEARCH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/(?:index\.php/)?vod/search(?:/|\.html$)").unwrap());
static SHARE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^/s/[A-Za-z0-9_-]+/?$").unwrap());
static TIANYI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^/t/[A-Za-z0-9]+/?$").unwrap());
pub(super) static URLS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?://[A-Za-z0-9._~:/?#\[\]@!$&+,;=%-]+").unwrap());
static PASSWORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:提取码|访问码|密码|pwd|password)\s*[:：=]\s*([A-Za-z0-9]{4,8})(?:[^A-Za-z0-9]|$)",
    )
    .unwrap()
});
static MAGNET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"magnet:\?xt=urn:btih:[0-9a-fA-F]{40}(?:[&][^\s\"'<>]*)?"#).unwrap()
});
static ED2K: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"ed2k://\|file\|[^|]+\|\d+\|[0-9a-fA-F]{32}\|/?"#).unwrap());
pub(super) fn valid_password(s: &str) -> bool {
    (4..=8).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric())
}
pub(super) fn password(text: &str) -> Option<String> {
    PASSWORD.captures(text).map(|m| m[1].to_owned())
}
pub(super) fn internal(base: &Url, raw: &str, detail: bool) -> Option<Url> {
    let authority = raw
        .split_once("://")
        .map(|(_, rest)| rest)
        .or_else(|| raw.strip_prefix("//"));
    if authority.is_some_and(|rest| rest.split('/').next().unwrap_or("").contains('@')) {
        return None;
    }
    if raw.trim().is_empty() {
        return None;
    }
    let mut url = base.join(raw.trim()).ok()?;
    if url.origin() != base.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || !(if detail { &DETAIL } else { &SEARCH }).is_match(url.path())
    {
        return None;
    }
    url.set_fragment(None);
    Some(url)
}
pub(super) fn entry(base: &Url, name: &str, anchor: ElementRef<'_>) -> Option<Entry> {
    let url = internal(base, anchor.value().attr("href")?, true)?;
    let title = text(anchor);
    let title = if title.is_empty() {
        clean(anchor.value().attr("title").unwrap_or(""))
    } else {
        title
    };
    if title.is_empty() {
        return None;
    }
    let id = DETAIL.captures(url.path())?[1].to_owned();
    let mut row = SearchResult::new(format!("{name}-{id}"), Source::provider(name));
    row.title = title;
    Some(Entry { url, row })
}
pub(super) fn next_page(doc: &Html, base: &Url) -> Option<Url> {
    doc.select(&selector("#page a.page-next[title='下一页']"))
        .find_map(|a| internal(base, a.value().attr("href")?, false))
}
pub(super) fn share(raw: &str, nearby: &str, extended: bool) -> Option<Link> {
    if extended && let Some(m) = MAGNET.find(raw).or_else(|| ED2K.find(raw)) {
        if raw
            .as_bytes()
            .get(m.end())
            .is_some_and(u8::is_ascii_alphanumeric)
        {
            return None;
        }
        return Some(Link::new(m.as_str()));
    }
    let candidate = if extended {
        URLS.find(raw)?.as_str()
    } else {
        raw.trim()
    };
    let mut u = Url::parse(candidate).ok()?;
    if !matches!(u.scheme(), "http" | "https")
        || !u.username().is_empty()
        || u.password().is_some()
        || u.port().is_some()
    {
        return None;
    }
    // Explicit default ports disappear in Url::port(), but are still disallowed.
    if candidate
        .split_once("://")?
        .1
        .split('/')
        .next()?
        .contains([':', '@'])
    {
        return None;
    }
    let host = u.host_str()?.strip_prefix("www.").unwrap_or(u.host_str()?);
    let kind = match host {
        "cloud.189.cn"
            if TIANYI.is_match(u.path())
                || (u.path() == "/web/share"
                    && u.query_pairs().any(|(k, v)| {
                        k == "code" && !v.is_empty() && v.bytes().all(|b| b.is_ascii_alphanumeric())
                    })) =>
        {
            CloudType::Tianyi
        }
        "caiyun.feixin.10086.cn" | "yun.139.com"
            if extended && u.path() != "/" && !u.path().is_empty() =>
        {
            CloudType::Mobile
        }
        _ if SHARE.is_match(u.path()) => match host {
            "pan.quark.cn" => CloudType::Quark,
            "drive.uc.cn" => CloudType::Uc,
            "pan.baidu.com" => CloudType::Baidu,
            "alipan.com" | "aliyundrive.com" => CloudType::Aliyun,
            "pan.xunlei.com" => CloudType::Xunlei,
            "115.com" | "115cdn.com" | "anxia.com" => CloudType::One15,
            "mypikpak.com" if extended => CloudType::Pikpak,
            "guangyapan.com" => CloudType::Guangya,
            "123pan.com" | "123pan.cn" | "123684.com" | "123685.com" | "123865.com"
            | "123912.com" | "123592.com" => CloudType::Pan123,
            _ => return None,
        },
        _ => return None,
    };
    let mut pairs = u
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect::<Vec<_>>();
    pairs.sort();
    let pass = ["pwd", "password"]
        .into_iter()
        .find_map(|key| {
            pairs
                .iter()
                .find(|(k, v)| k == key && valid_password(v))
                .map(|(_, v)| v.clone())
        })
        .or_else(|| password(&URLS.replace_all(nearby, " ")));
    u.set_query(None);
    if !pairs.is_empty() {
        u.query_pairs_mut().extend_pairs(&pairs);
    }
    let mut link = Link::new(u.to_string());
    link.cloud_type = kind;
    link.password = pass;
    Some(link)
}

#[derive(Clone, Debug)]
pub(super) struct Entry {
    pub url: Url,
    pub row: SearchResult,
}
type SearchParser = fn(&str, &Url) -> Result<(Vec<Entry>, Option<Url>), ProviderError>;
type DetailParser = fn(&str, Entry) -> Result<SearchResult, ProviderError>;
#[derive(Debug)]
pub(super) struct Site {
    pub base: Url,
    slots: Arc<Semaphore>,
    cache: Mutex<HashMap<String, (Instant, SearchResult)>>,
    pages: usize,
    details: usize,
    limit: usize,
    attempts: usize,
}
impl Site {
    pub fn new(
        base: Url,
        pages: usize,
        details: usize,
        concurrency: usize,
        limit: usize,
        attempts: usize,
    ) -> Self {
        Self {
            base,
            slots: Arc::new(Semaphore::new(concurrency)),
            cache: Mutex::new(HashMap::new()),
            pages,
            details,
            limit,
            attempts,
        }
    }
    async fn fetch(
        &self,
        ctx: &SearchContext,
        url: &Url,
        validate: impl Fn(&str) -> Result<(), ProviderError>,
    ) -> Result<String, ProviderError> {
        let mut last = ProviderError::Timeout;
        for attempt in 0..self.attempts {
            if ctx.is_cancelled() {
                return Err(ProviderError::Timeout);
            }
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(300 * attempt as u64)).await;
                if ctx.is_cancelled() {
                    return Err(ProviderError::Timeout);
                }
            }
            let response = crate::http::execute(
                ctx.client
                    .get(url.clone())
                    .timeout(ctx.request_timeout(Duration::from_secs(25)))
                    .header("user-agent", UA)
                    .header("referer", self.base.as_str())
                    .header("accept", "text/html,application/xhtml+xml,*/*")
                    .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8"),
            )
            .await;
            let response = match response {
                Ok(r) => r,
                Err(e) => {
                    last = network(e);
                    continue;
                }
            };
            let code = response.status().as_u16();
            if code != 200 {
                last = ProviderError::Protocol(format!("upstream HTTP {code}"));
                if (400..500).contains(&code) && code != 429 {
                    return Err(last);
                }
                continue;
            }
            let body = match crate::http::read_response(response, self.limit).await {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(e) => {
                    if e.downcast_ref::<crate::http::ResponseTooLarge>().is_some() {
                        return Err(parse_error("page exceeds response limit"));
                    }
                    last = network(e);
                    continue;
                }
            };
            match validate(&body) {
                Ok(()) => return Ok(body),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
    pub async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
        url: Url,
        search: SearchParser,
        detail: DetailParser,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        tokio::time::timeout(
            ctx.remaining().min(Duration::from_secs(25)),
            self.inner(ctx, query, url, search, detail),
        )
        .await
        .map_err(|_| ProviderError::Timeout)?
    }
    async fn inner(
        &self,
        ctx: &SearchContext,
        query: &str,
        mut url: Url,
        search: SearchParser,
        detail: DetailParser,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut visited = HashSet::new();
        let mut seen = HashSet::new();
        let mut rows = vec![];
        let mut first_error = None;
        for _ in 0..self.pages {
            if !visited.insert(url.to_string()) {
                break;
            }
            let body = match self
                .fetch(ctx, &url, |body| search(body, &self.base).map(|_| ()))
                .await
            {
                Ok(body) => body,
                Err(e) => {
                    first_error.get_or_insert(e);
                    break;
                }
            };
            let (entries, next) = search(&body, &self.base)?;
            let remaining = self.details.saturating_sub(seen.len());
            let entries = entries
                .into_iter()
                .filter(|e| words_match(&e.row.title, query) && seen.insert(e.url.to_string()))
                .take(remaining)
                .collect::<Vec<_>>();
            // Bound before scheduling; no HTML DOM survives an await.
            let mut pending = stream::iter(entries.into_iter().map(|entry| async move {
                let _slot = self
                    .slots
                    .acquire()
                    .await
                    .map_err(|_| ProviderError::Timeout)?;
                let key = entry.url.to_string();
                let cached = self
                    .cache
                    .lock()
                    .unwrap()
                    .get(&key)
                    .filter(|(until, _)| *until > Instant::now())
                    .map(|(_, row)| row.clone());
                if let Some(row) = cached {
                    return Ok(row);
                }
                let target = entry.url.clone();
                let body = self
                    .fetch(ctx, &target, |body| detail(body, entry.clone()).map(|_| ()))
                    .await?;
                let row = detail(&body, entry)?;
                if !row.links.is_empty() {
                    let mut cache = self.cache.lock().unwrap();
                    if cache.len() >= 512
                        && let Some(old) = cache
                            .iter()
                            .min_by_key(|(_, v)| v.0)
                            .map(|(k, _)| k.clone())
                    {
                        cache.remove(&old);
                    }
                    cache.insert(
                        key,
                        (Instant::now() + Duration::from_secs(1800), row.clone()),
                    );
                }
                Ok::<_, ProviderError>(row)
            }))
            .buffer_unordered(self.details);
            let mut page_rows = vec![];
            while let Some(value) = pending.next().await {
                match value {
                    Ok(row) => {
                        if !row.links.is_empty() {
                            ctx.publish(vec![row.clone()]);
                            page_rows.push(row);
                        }
                    }
                    Err(e) => {
                        first_error.get_or_insert(e);
                    }
                }
            }
            page_rows.sort_by_key(|row| row.id.clone());
            rows.extend(page_rows);
            if seen.len() >= self.details {
                break;
            }
            let Some(next) = next else {
                break;
            };
            url = next;
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(rows)
    }
}

#[cfg(test)]
#[path = "e_html_tests.rs"]
mod tests;
