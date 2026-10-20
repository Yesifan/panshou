//! 91盘他 topic cards and thread content. Retry only transport/5xx.
use super::markup::{matches, sel, text, time};
use crate::{
    core::{CloudType, Link, ProviderError, SearchResult},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{ElementRef, Html};
use std::{
    collections::HashSet,
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use url::Url;
pub struct Panta {
    base: Url,
    samples: Mutex<Vec<Duration>>,
    concurrency: AtomicUsize,
}
impl Default for Panta {
    fn default() -> Self {
        Self::new(Url::parse("https://www.91panta.cn/").unwrap())
    }
}
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"topicId=(\d+)").unwrap());
static POST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"发表时间：(.+)").unwrap());
static PASSWORD: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"提取码[：:]\s*([0-9a-zA-Z]+)",
        r"密码[：:]\s*([0-9a-zA-Z]+)",
        r"pwd[=:：]\s*([0-9a-zA-Z]+)",
    ]
    .into_iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});
static PWD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[?&]pwd=([0-9a-zA-Z]+)").unwrap());
static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"https?://pan\.baidu\.com/s/[0-9a-zA-Z_\-]+(?:\?pwd=[0-9a-zA-Z]+)?",
        r"https?://pan\.quark\.cn/s/[0-9a-zA-Z]+",
        r"https?://www\.aliyundrive\.com/s/[0-9a-zA-Z]+",
        r"https?://alipan\.com/s/[0-9a-zA-Z]+",
        r"https?://pan\.xunlei\.com/s/[0-9a-zA-Z_\-]+(?:\?pwd=[0-9a-zA-Z]+)?[#]?",
        r"https?://cloud\.189\.cn/t/[0-9a-zA-Z]+",
        r"https?://(?:www\.)?caiyun\.139\.com/[mw]/i\?[0-9a-zA-Z]+(?:\?pwd=[0-9a-zA-Z]+)?",
    ]
    .into_iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});
const DOMAINS: &[&str] = &[
    "pan.baidu.com",
    "pan.quark.cn",
    "aliyundrive.com",
    "alipan.com",
    "pan.xunlei.com",
    "cloud.189.cn",
    "caiyun.139.com",
    "drive.uc.cn",
    "115.com",
    "mypikpak.com",
];
const HINTS: &[&str] = &["提取码", "密码", "pwd", "验证码", "口令"];
fn explicit(s: &str) -> Option<String> {
    PASSWORD
        .iter()
        .find_map(|p| p.captures(s).map(|c| c[1].to_owned()))
}
fn supported(raw: &str) -> bool {
    Url::parse(raw).ok().is_some_and(|u| {
        matches!(u.scheme(), "http" | "https")
            && u.host_str().is_some_and(|h| {
                DOMAINS
                    .iter()
                    .any(|d| h == *d || h.ends_with(&format!(".{d}")))
            })
    })
}
fn anchors(scope: ElementRef<'_>) -> Vec<Link> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for a in scope.select(&sel("a[href^='http']")) {
        let Some(raw) = a.value().attr("href").filter(|s| supported(s)) else {
            continue;
        };
        if !seen.insert(raw.to_owned()) {
            continue;
        }
        let mut surrounding = text(a);
        if surrounding.is_empty() {
            surrounding = a
                .parent()
                .and_then(ElementRef::wrap)
                .map(text)
                .unwrap_or_default();
        }
        if surrounding.is_empty() {
            surrounding = text(scope);
        }
        let mut l = Link::new(raw);
        l.password = PWD
            .captures(raw)
            .map(|c| c[1].to_owned())
            .or_else(|| explicit(&surrounding));
        if l.cloud_type == CloudType::Quark && !HINTS.iter().any(|h| surrounding.contains(h)) {
            l.password = None;
        }
        if l.cloud_type == CloudType::Mobile {
            l.password =
                explicit(&surrounding).or_else(|| PWD.captures(raw).map(|c| c[1].to_owned()));
        }
        out.push(l);
    }
    out
}
fn text_links(body: &str) -> Vec<Link> {
    let mut found = PATTERNS
        .iter()
        .flat_map(|r| r.find_iter(body))
        .collect::<Vec<_>>();
    found.sort_by_key(|m| m.start());
    let passwords = PASSWORD
        .iter()
        .flat_map(|r| r.captures_iter(body))
        .map(|c| {
            let m = c.get(0).unwrap();
            (m.start(), m.end(), c[1].to_owned())
        })
        .collect::<Vec<_>>();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (index, m) in found.iter().enumerate() {
        let raw = m.as_str().trim_end_matches('#');
        let mut link = Link::new(raw);
        let base = raw
            .find("?pwd=")
            .or_else(|| raw.find("&pwd="))
            .map(|i| &raw[..i])
            .unwrap_or(raw);
        if !seen.insert(base.to_owned()) {
            continue;
        }
        link.password = PWD.captures(raw).map(|c| c[1].to_owned());
        if link.password.is_none() {
            let next = found.get(index + 1).map(|m| m.start());
            let best = passwords
                .iter()
                .filter(|(start, _, _)| *start > m.end())
                .max_by_key(|(start, _, _)| {
                    let distance = start - m.end();
                    1000isize
                        - distance as isize
                        - if next.is_some_and(|n| *start > n) {
                            500
                        } else {
                            0
                        }
                        + if distance < 50 { 300 } else { 0 }
                        + 200
                });
            if let Some((_, _, p)) = best {
                link.password = Some(p.clone());
            }
            if matches!(link.cloud_type, CloudType::Baidu | CloudType::Xunlei)
                && let Some(p) = &link.password
            {
                link.url = format!(
                    "{base}{}pwd={p}",
                    if base.contains('?') { "&" } else { "?" }
                );
            } else {
                link.url = base.to_owned();
            }
        }
        out.push(link);
    }
    out
}
struct Item {
    id: String,
    r: SearchResult,
}
fn items(body: &str) -> Result<Vec<Item>, ProviderError> {
    let doc = Html::parse_document(body);
    if doc
        .select(&sel(
            "div.topicItem,.topicList,.topic-list,#searchResult,.search-results",
        ))
        .next()
        .is_none()
    {
        return Err(ProviderError::Parse("panta topic list missing".into()));
    }
    let mut out = Vec::new();
    for row in doc.select(&sel("div.topicItem")) {
        let Some(a) = row.select(&sel("a[href^='thread?topicId=']")).next() else {
            continue;
        };
        let Some(id) = a
            .value()
            .attr("href")
            .and_then(|s| ID.captures(s))
            .map(|c| c[1].to_owned())
        else {
            continue;
        };
        let mut r = SearchResult::provider(format!("panta-{id}"), "panta");
        r.title = text(a);
        r.content = row
            .select(&sel("h2.summary"))
            .next()
            .map(text)
            .unwrap_or_default();
        r.tags.push("panta".into());
        r.datetime = row
            .select(&sel("span.postTime"))
            .next()
            .and_then(|n| POST.captures(&text(n)).and_then(|c| time(c[1].trim())));
        r.links = anchors(row);
        out.push(Item { id, r });
    }
    Ok(out)
}
impl Panta {
    pub fn new(base: Url) -> Self {
        Self {
            base,
            samples: Mutex::new(Vec::new()),
            concurrency: AtomicUsize::new(30),
        }
    }
    async fn fetch(&self, ctx: &SearchContext, url: Url) -> Result<String, ProviderError> {
        let mut last = ProviderError::Timeout;
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(100 << (attempt - 1))).await;
            }
            let request = ctx
                .client
                .get(url.clone())
                .timeout(ctx.request_timeout(Duration::from_secs(6)))
                .header(
                    "user-agent",
                    "Mozilla/5.0 Chrome/91.0.4472.124 Safari/537.36",
                )
                .header("referer", self.base.join("index").unwrap().as_str())
                .header(
                    "accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
                )
                .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
                .header("cache-control", "max-age=0");
            let start = Instant::now();
            let response = crate::http::execute(request).await;
            {
                let mut samples = self.samples.lock().unwrap();
                if samples.len() == 100 {
                    samples.remove(0);
                }
                samples.push(start.elapsed());
            }
            match response {
                Err(e) => last = ProviderError::Network(e.to_string()),
                Ok(r) => {
                    let status = r.status().as_u16();
                    if status >= 500 {
                        last = ProviderError::Protocol(format!("panta HTTP {status}"));
                        continue;
                    }
                    if status != 200 {
                        return Err(if status == 429 {
                            ProviderError::RateLimited
                        } else {
                            ProviderError::Protocol(format!("panta HTTP {status}"))
                        });
                    }
                    let bytes = crate::http::read_response(r, 16 << 20)
                        .await
                        .map_err(|e| ProviderError::Network(e.to_string()))?;
                    return Ok(String::from_utf8_lossy(&bytes).into_owned());
                }
            }
        }
        Err(last)
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self.base.join("search").unwrap();
        url.query_pairs_mut().append_pair("keyword", query);
        let items = items(&self.fetch(ctx, url).await?)?;
        let mut jobs = stream::iter(items.into_iter().map(|mut item| async move {
            if item.r.links.is_empty() {
                let mut url = self.base.join("thread").unwrap();
                url.query_pairs_mut().append_pair("topicId", &item.id);
                for attempt in 0..3 {
                    if attempt > 0 {
                        tokio::time::sleep(Duration::from_millis(100 << (attempt - 1))).await;
                    }
                    let body = self.fetch(ctx, url.clone()).await?;
                    let doc = Html::parse_document(&body);
                    let mut seen = HashSet::new();
                    for scope in doc.select(&sel("div.topicContent")) {
                        item.r.links.extend(anchors(scope));
                        item.r.links.extend(text_links(&scope.inner_html()));
                    }
                    item.r.links.retain(|l| seen.insert(l.url.clone()));
                    if !item.r.links.is_empty() {
                        break;
                    }
                }
            }
            if item.r.links.is_empty() {
                return Err(ProviderError::Parse(
                    "panta thread has no share links after 3 attempts".into(),
                ));
            }
            if !matches(&format!("{} {}", item.r.title, item.r.content), query) {
                return Ok(None);
            }
            for l in &mut item.r.links {
                l.work_title = Some(item.r.title.clone());
            }
            ctx.publish(vec![item.r.clone()]);
            Ok(Some(item.r))
        }))
        .buffer_unordered(self.concurrency.load(Ordering::Relaxed));
        let mut out = Vec::new();
        let mut error = None;
        while let Some(r) = jobs.next().await {
            match r {
                Ok(Some(r)) => out.push(r),
                Ok(None) => {}
                Err(e) => error = Some(e),
            }
        }
        {
            let mut samples = self.samples.lock().unwrap();
            if samples.len() >= 5 {
                let average = samples.iter().copied().sum::<Duration>() / samples.len() as u32;
                let current = self.concurrency.load(Ordering::Relaxed);
                self.concurrency.store(
                    if average > Duration::from_millis(500) {
                        current.saturating_sub(5).max(5)
                    } else {
                        (current + 5).min(50)
                    },
                    Ordering::Relaxed,
                );
                samples.clear();
            }
        }
        if let Some(e) = error {
            return Err(e);
        }
        Ok(out)
    }
}
#[async_trait]
impl Provider for Panta {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("panta", 1, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        tokio::time::timeout(ctx.remaining(), self.run(ctx, query))
            .await
            .map_err(|_| ProviderError::Timeout)?
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn full_chain_and_retry_classification() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{path, query_param},
        };
        let s = MockServer::start().await;
        let n = Arc::new(AtomicUsize::new(0));
        let c = n.clone();
        Mock::given(path("/search")).and(query_param("keyword","Film")).respond_with(move |_:&wiremock::Request|if c.fetch_add(1,Ordering::SeqCst)<2 {ResponseTemplate::new(500)}else{ResponseTemplate::new(200).set_body_string("<div class='topicItem'><a href='thread?topicId=7'>Film</a><h2 class='summary'>summary</h2><span class='postTime'>发表时间：2026-10-12 12:00:00</span></div>")}).expect(3).mount(&s).await;
        Mock::given(path("/thread"))
            .and(query_param("topicId", "7"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "<div class='topicContent'>https://pan.baidu.com/s/abc 提取码：abcd</div>",
            ))
            .expect(1)
            .mount(&s)
            .await;
        let p = Panta::new(Url::parse(&s.uri()).unwrap());
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(4));
        let r = p.search(&ctx, "Film").await.unwrap();
        assert_eq!(r[0].links[0].password.as_deref(), Some("abcd"));
        assert!(r[0].datetime.is_some());
        s.reset().await;
        Mock::given(path("/search"))
            .respond_with(ResponseTemplate::new(403))
            .expect(1)
            .mount(&s)
            .await;
        assert!(
            p.search(&ctx, "Film")
                .await
                .unwrap_err()
                .to_string()
                .contains("403")
        );
        assert!(items("<div class='topicList'></div>").unwrap().is_empty());
        assert!(items("bad").is_err());
    }
}
