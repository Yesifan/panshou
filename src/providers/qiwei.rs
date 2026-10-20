//! 七味 suggestion API, mirror detail pages and session-bound MD5 slider.
use super::markup::{matches, sel, text, time};
use crate::{
    core::{CloudType, Link, ProviderError, SearchResult},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use reqwest::cookie::{CookieStore, Jar};
use scraper::{ElementRef, Html};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};
use url::Url;
pub struct Qiwei {
    hosts: Vec<Url>,
    active: Mutex<Option<Url>>,
    jar: Arc<Jar>,
    memo: Mutex<HashMap<String, (Instant, SearchResult)>>,
}
impl Default for Qiwei {
    fn default() -> Self {
        Self::new(
            [
                "qmp4", "gmp4", "qwmp4", "qwmkv", "qwfilm", "qwfun", "qwshow", "qnnull", "qnhot",
                "qncool",
            ]
            .into_iter()
            .map(|s| Url::parse(&format!("https://www.{s}.com/")).unwrap())
            .collect(),
        )
    }
}
static SCRIPT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)<script[^>]+src=["']([^"']*huadong_[^"']+\.js[^"']*)"#).unwrap()
});
static TYPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)type=([a-f0-9]{32})").unwrap());
static KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)key=["']([a-f0-9]{32})["']"#).unwrap());
static VALUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)value=["']([^"']+)["']"#).unwrap());
static ENDPOINT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)/([a-z0-9_]+_yanzheng_huadong\.php)\?type=").unwrap());
static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\(\d{4}\)\s*$").unwrap());
static TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{4}-\d{2}-\d{2}\s+\d{2}:\d{2}:\d{2})").unwrap());
static MAGNET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"magnet:\?[^\s"'<>]+"#).unwrap());
static PASSWORDS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)(?:提取码|密码|pwd)[：:\s]*([a-z0-9]{4,8})",
        r"(?i)\?pwd=([a-z0-9]{4,8})",
    ]
    .into_iter()
    .map(|s| Regex::new(s).unwrap())
    .collect()
});
#[derive(Deserialize)]
struct Suggest {
    code: i64,
    list: Vec<Item>,
}
#[derive(Deserialize, Clone)]
struct Item {
    id: u64,
    name: String,
    #[serde(default)]
    pic: String,
}
fn verify_page(body: &str) -> bool {
    let lower = body.to_lowercase();
    [
        "系统安全验证",
        "verify_check",
        "mac_verify_img",
        "请输入验证码",
        "/_guard/html.js?js=easy_click_html",
    ]
    .iter()
    .any(|s| lower.contains(s))
}
fn clean(s: &str) -> String {
    s.replace(['\u{200b}', '\u{200c}', '\u{200d}', '\u{feff}'], "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn resolve(base: &Url, raw: &str) -> Option<Url> {
    let raw = clean(raw);
    if raw.is_empty() {
        None
    } else {
        base.join(&raw.replace("&amp;", "&")).ok()
    }
}
fn password(raw: &str, contexts: &[String]) -> Option<String> {
    std::iter::once(raw)
        .chain(contexts.iter().map(String::as_str))
        .find_map(|s| {
            PASSWORDS
                .iter()
                .find_map(|r| r.captures(s).map(|c| c[1].to_owned()))
        })
}
fn work(title: &str, label: &str) -> String {
    let label = clean(label);
    if label.is_empty() || label == title || label.contains(title) {
        title.into()
    } else {
        clean(&format!("{title} {label}"))
    }
}
fn cloud(raw: &str) -> CloudType {
    let mut kind = Link::new(raw).cloud_type;
    if let Ok(u) = Url::parse(raw) {
        let host = u.host_str().unwrap_or("");
        if ["anxia.com", "115cdn.com"]
            .iter()
            .any(|h| host == *h || host.ends_with(&format!(".{h}")))
        {
            kind = CloudType::One15;
        }
        if host == "content.21cn.com" {
            kind = CloudType::Tianyi;
        }
        if [
            "123684.com",
            "123685.com",
            "123912.com",
            "123865.com",
            "123pan.com",
            "123pan.cn",
            "123592.com",
        ]
        .iter()
        .any(|h| host == *h || host.ends_with(&format!(".{h}")))
        {
            kind = CloudType::Pan123;
        }
    }
    kind
}
fn score(title: &str, query: &str) -> usize {
    let t = title.to_lowercase();
    let q = query.to_lowercase();
    let markers = [
        "杜比", "dolby", "原盘", "高码", "remux", "蓝光", "hdr10+", "hdr10", "hdr", "4k", "2160p",
        "uhd",
    ];
    usize::from(t.contains(&q)) * 1000
        + usize::from(t.starts_with(&q)) * 200
        + markers
            .iter()
            .enumerate()
            .filter(|(_, m)| t.contains(**m))
            .map(|(i, _)| markers.len() - i)
            .sum::<usize>()
}
fn parse_detail(body: &str, page: &Url, item: &Item) -> Result<SearchResult, ProviderError> {
    let doc = Html::parse_document(body);
    if doc
        .select(&sel(".main-ui-meta,.down-list,.movie-introduce"))
        .next()
        .is_none()
    {
        return Err(ProviderError::Parse("qiwei detail document changed".into()));
    }
    let title = doc
        .select(&sel(".main-ui-meta h1"))
        .next()
        .map(text)
        .map(|s| clean(&s))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| clean(&item.name));
    let title = YEAR.replace(&title, "").to_string();
    let mut r = SearchResult::provider(format!("qiwei-{}", item.id), "qiwei");
    r.title = title;
    let image = doc
        .select(&sel("meta[property='og:image']"))
        .next()
        .and_then(|n| n.value().attr("content"))
        .and_then(|s| resolve(page, s))
        .or_else(|| {
            doc.select(&sel(".main-left .img img"))
                .next()
                .and_then(|n| n.value().attr("src"))
                .and_then(|s| resolve(page, s))
        })
        .or_else(|| resolve(page, &item.pic));
    if let Some(image) = image {
        r.images.push(image.to_string());
    }
    let mut parts = Vec::new();
    if let Some(meta) = doc
        .select(&sel(".main-ui-meta .otherbox"))
        .next()
        .map(text)
        .filter(|s| !s.is_empty())
    {
        parts.push(clean(&meta));
    }
    if let Some(intro) = doc
        .select(&sel(".movie-introduce .sqjj_a"))
        .next()
        .or_else(|| doc.select(&sel(".movie-introduce .zkjj_a")).next())
        .map(text)
        .filter(|s| !s.is_empty())
    {
        let intro = clean(&intro);
        parts.push(format!(
            "{}{}",
            intro.chars().take(320).collect::<String>(),
            if intro.chars().count() > 320 {
                "..."
            } else {
                ""
            }
        ));
    }
    r.content = if parts.is_empty() {
        "来源：七味".into()
    } else {
        parts.join(" | ")
    };
    r.datetime = TIME
        .captures(&doc.root_element().text().collect::<String>())
        .and_then(|c| time(&c[1]));
    let mut seen = HashSet::new();
    let mut cloud_links = Vec::new();
    let mut special = Vec::new();
    let mut play = Vec::new();
    let mut add = |raw: String, label: String, contexts: Vec<String>| {
        if !seen.insert(raw.clone()) {
            return;
        }
        let kind = cloud(&raw);
        if kind == CloudType::Others {
            return;
        }
        let mut l = Link::new(&raw);
        l.cloud_type = kind;
        l.password = password(&raw, &contexts);
        l.work_title = Some(work(&r.title, &label));
        if matches!(kind, CloudType::Magnet | CloudType::Ed2k) {
            special.push(l)
        } else {
            cloud_links.push(l)
        }
    };
    for li in doc.select(&sel(".down-list .content li.down-list2")) {
        let folder = li.select(&sel("a.folder")).next();
        let label = folder
            .map(|n| {
                n.value()
                    .attr("title")
                    .map(clean)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| clean(&text(n)))
            })
            .unwrap_or_default();
        let download = li.select(&sel("span a[href]")).last().or(folder);
        if let Some(raw) = download
            .and_then(|n| n.value().attr("href"))
            .and_then(|s| resolve(page, s))
        {
            add(raw.to_string(), label.clone(), vec![label, text(li)]);
        }
    }
    for a in doc.select(&sel("a[href]")) {
        if let Some(url) = a.value().attr("href").and_then(|s| resolve(page, s)) {
            let label = text(a);
            if cloud(url.as_str()) != CloudType::Others {
                add(
                    url.to_string(),
                    label.clone(),
                    vec![
                        label,
                        a.parent()
                            .and_then(ElementRef::wrap)
                            .map(text)
                            .unwrap_or_default(),
                    ],
                );
            } else if url.path().contains("/py/") && url.path().to_lowercase().ends_with(".html") {
                let mut l = Link::new(url.as_str());
                l.work_title = Some(work(&r.title, &label));
                if !play.iter().any(|x: &Link| x.url == l.url) {
                    play.push(l);
                }
            }
        }
    }
    for m in MAGNET.find_iter(body) {
        add(m.as_str().replace("&amp;", "&"), r.title.clone(), vec![]);
    }
    drop(add);
    r.links = cloud_links;
    r.links.extend(special);
    if r.links.is_empty() {
        let keep = play.len().saturating_sub(6);
        r.links.extend(play.into_iter().skip(keep));
    }
    if r.links.is_empty() {
        let mut l = Link::new(page.as_str());
        l.work_title = Some(r.title.clone());
        r.links.push(l);
    }
    Ok(r)
}
impl Qiwei {
    pub fn new(hosts: Vec<Url>) -> Self {
        Self {
            hosts,
            active: Mutex::new(None),
            jar: Arc::new(Jar::default()),
            memo: Mutex::new(HashMap::new()),
        }
    }
    fn hosts(&self) -> Vec<Url> {
        let mut v = self
            .active
            .lock()
            .unwrap()
            .clone()
            .into_iter()
            .collect::<Vec<_>>();
        for host in &self.hosts {
            if !v.contains(host) {
                v.push(host.clone());
            }
        }
        v
    }
    async fn fetch(
        &self,
        ctx: &SearchContext,
        url: Url,
        referer: &str,
        xhr: bool,
    ) -> Result<String, ProviderError> {
        let mut error = ProviderError::Timeout;
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(200 << (attempt - 1))).await;
            }
            let result = tokio::time::timeout(ctx.request_timeout(Duration::from_secs(8)), async {
                let mut request = ctx.client.get(url.clone())
                    .header("user-agent", "Mozilla/5.0 Chrome/138.0.0.0 Safari/537.36")
                    .header("accept", "text/html,application/xhtml+xml,application/xml;q=0.9,application/json;q=0.8,*/*;q=0.7")
                    .header("referer", referer)
                    .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8");
                if let Some(cookie) = self.jar.cookies(&url) {
                    request = request.header("cookie", cookie);
                }
                if xhr {
                    request = request.header("x-requested-with", "XMLHttpRequest");
                }
                let response = crate::http::execute(request).await.map_err(|e| ProviderError::Network(e.to_string()))?;
                if response.status().as_u16() != 200 {
                    return Err(ProviderError::Protocol(format!("qiwei HTTP {}", response.status())));
                }
                for value in response.headers().get_all("set-cookie") {
                    if let Ok(cookie) = value.to_str() {
                        self.jar.add_cookie_str(cookie, response.url());
                    }
                }
                let bytes = crate::http::read_response(response, 16 << 20).await.map_err(|e| ProviderError::Network(e.to_string()))?;
                let body = String::from_utf8_lossy(&bytes).into_owned();
                Ok(match serde_json::from_str::<String>(&body) {
                    Ok(s) if s.contains("<html") || s.contains("<!DOCTYPE html") => s,
                    _ => body,
                })
            }).await.unwrap_or(Err(ProviderError::Timeout));
            match result {
                Ok(s) => return Ok(s),
                Err(e) => error = e,
            }
        }
        Err(error)
    }
    async fn solve(
        &self,
        ctx: &SearchContext,
        page: &Url,
        body: &str,
    ) -> Result<(), ProviderError> {
        let script = SCRIPT
            .captures(body)
            .and_then(|c| resolve(page, &c[1]))
            .ok_or_else(|| ProviderError::Parse("qiwei missing verification script".into()))?;
        let js = self.fetch(ctx, script, page.as_str(), false).await?;
        let typ = TYPE
            .captures(&js)
            .map(|c| c[1].to_owned())
            .ok_or_else(|| ProviderError::Parse("qiwei missing verification type".into()))?;
        let key = KEY
            .captures(&js)
            .map(|c| c[1].to_owned())
            .ok_or_else(|| ProviderError::Parse("qiwei missing verification key".into()))?;
        let val = VALUE
            .captures(&js)
            .map(|c| c[1].to_owned())
            .ok_or_else(|| ProviderError::Parse("qiwei missing verification value".into()))?;
        let decimals = val
            .chars()
            .map(|c| (u32::from(c) + 1).to_string())
            .collect::<String>();
        let encoded = format!("{:x}", md5::compute(decimals));
        let endpoint = ENDPOINT
            .captures(&js)
            .map(|c| format!("/{}", &c[1]))
            .unwrap_or_else(|| "/a20be899_96a6_40b2_88ba_32f1f75f1552_yanzheng_huadong.php".into());
        let mut url = page
            .join(&endpoint)
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("type", &typ)
            .append_pair("key", &key)
            .append_pair("value", &encoded);
        let reply = self.fetch(ctx, url, page.as_str(), true).await?;
        if verify_page(&reply) {
            return Err(ProviderError::Blocked);
        }
        Ok(())
    }
    async fn verified(
        &self,
        ctx: &SearchContext,
        url: Url,
        referer: &str,
    ) -> Result<String, ProviderError> {
        let body = self.fetch(ctx, url.clone(), referer, false).await?;
        if !verify_page(&body) {
            return Ok(body);
        }
        self.solve(ctx, &url, &body).await?;
        let body = self.fetch(ctx, url, referer, false).await?;
        if verify_page(&body) {
            Err(ProviderError::Blocked)
        } else {
            Ok(body)
        }
    }
    async fn enrich(
        &self,
        ctx: &SearchContext,
        host: &Url,
        item: Item,
        query: &str,
    ) -> Result<Option<SearchResult>, ProviderError> {
        let path = format!("mv/{}.html", item.id);
        let key = host.join(&path).unwrap().to_string();
        if let Some((_, r)) = self
            .memo
            .lock()
            .unwrap()
            .get(&key)
            .filter(|(t, _)| Instant::now() < *t)
        {
            let r = r.clone();
            ctx.publish(vec![r.clone()]);
            return Ok(Some(r));
        }
        let mut error = ProviderError::Unavailable("qiwei no detail hosts".into());
        let mut candidates = vec![host.clone()];
        for h in self.hosts() {
            if !candidates.contains(&h) {
                candidates.push(h);
            }
        }
        for host in candidates {
            let url = host.join(&path).unwrap();
            match self.verified(ctx, url.clone(), url.as_str()).await {
                Ok(body) => {
                    let r = match parse_detail(&body, &url, &item) {
                        Ok(r) => r,
                        Err(e) => {
                            error = e;
                            continue;
                        }
                    };
                    if !matches(&format!("{} {}", r.title, r.content), query) {
                        return Ok(None);
                    }
                    ctx.publish(vec![r.clone()]);
                    let mut memo = self.memo.lock().unwrap();
                    if memo.len() >= 256
                        && let Some(oldest) = memo
                            .iter()
                            .min_by_key(|(_, (t, _))| *t)
                            .map(|(k, _)| k.clone())
                    {
                        memo.remove(&oldest);
                    }
                    memo.insert(key, (Instant::now() + Duration::from_secs(2700), r.clone()));
                    return Ok(Some(r));
                }
                Err(e) => error = e,
            }
        }
        Err(error)
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut chosen = None;
        let mut error = ProviderError::Unavailable("qiwei no hosts".into());
        for host in self.hosts() {
            let mut url = host.join("index.php/ajax/suggest").unwrap();
            url.query_pairs_mut()
                .append_pair("mid", "1")
                .append_pair("limit", "100")
                .append_pair("wd", query);
            match self.verified(ctx, url, host.as_str()).await {
                Ok(body) => match serde_json::from_str::<Suggest>(&body) {
                    Ok(v) if v.code == 1 || !v.list.is_empty() => {
                        *self.active.lock().unwrap() = Some(host.clone());
                        chosen = Some((host, v.list));
                        break;
                    }
                    Ok(v) => {
                        error = ProviderError::Protocol(format!("qiwei suggest code={}", v.code))
                    }
                    Err(e) => error = ProviderError::Parse(e.to_string()),
                },
                Err(e) => error = e,
            }
        }
        let Some((host, mut items)) = chosen else {
            return Err(error);
        };
        let mut seen = HashSet::new();
        items.retain(|i| i.id > 0 && !clean(&i.name).is_empty() && seen.insert(i.id));
        items.sort_by(|a, b| {
            score(&b.name, query)
                .cmp(&score(&a.name, query))
                .then(a.name.len().cmp(&b.name.len()))
        });
        let mut jobs = stream::iter(
            items
                .into_iter()
                .map(|item| self.enrich(ctx, &host, item, query)),
        )
        .buffered(12);
        let mut out = Vec::new();
        let mut error = None;
        while let Some(r) = jobs.next().await {
            match r {
                Ok(Some(r)) => out.push(r),
                Ok(None) => {}
                Err(e) => error = Some(e),
            }
        }
        if let Some(e) = error {
            return Err(e);
        }
        Ok(out)
    }
}
#[async_trait]
impl Provider for Qiwei {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("qiwei", 3, KeywordFilterMode::Provider)
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
    async fn persistent_verification_after_successful_check_remains_blocked() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        let server = MockServer::start().await;
        Mock::given(path("/index.php/ajax/suggest"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "<html>系统安全验证<script src='/huadong_test.js'></script></html>",
            ))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(path("/huadong_test.js"))
            .respond_with(ResponseTemplate::new(200).set_body_string("type=01234567890123456789012345678901;key='01234567890123456789012345678901';value='abc';'/test_yanzheng_huadong.php?type='"))
            .expect(1).mount(&server).await;
        Mock::given(path("/test_yanzheng_huadong.php"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .expect(1)
            .mount(&server)
            .await;
        let provider = Qiwei::new(vec![Url::parse(&server.uri()).unwrap()]);
        let context = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
        assert!(matches!(
            provider.search(&context, "Film").await,
            Err(ProviderError::Blocked)
        ));
        assert!(!context.has_results());
    }
    #[tokio::test]
    async fn verification_full_chain() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{header, path, query_param},
        };
        let s = MockServer::start().await;
        let challenge = "<html>系统安全验证<script src='/huadong_test.js'></script></html>";
        Mock::given(path("/index.php/ajax/suggest"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(challenge)
                    .insert_header("set-cookie", "session=private; Path=/"),
            )
            .with_priority(10)
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(path("/huadong_test.js")).and(header("cookie","session=private")).respond_with(ResponseTemplate::new(200).set_body_string("type=01234567890123456789012345678901;key='01234567890123456789012345678901';value='abc';'/test_yanzheng_huadong.php?type='")).expect(1).mount(&s).await;
        Mock::given(path("/test_yanzheng_huadong.php"))
            .and(query_param(
                "value",
                format!("{:x}", md5::compute("9899100")),
            ))
            .and(header("x-requested-with", "XMLHttpRequest"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("ok")
                    .insert_header("set-cookie", "verified=yes; Path=/"),
            )
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(path("/index.php/ajax/suggest"))
            .and(|r: &wiremock::Request| {
                r.headers
                    .get("cookie")
                    .is_some_and(|v| v.to_str().unwrap_or("").contains("verified=yes"))
            })
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"code":1,"list":[{"id":1,"name":"Film","pic":"/poster.jpg"}]}),
            ))
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(path("/mv/1.html")).respond_with(ResponseTemplate::new(200).set_body_string("<div class='main-ui-meta'><h1>Film (2026)</h1></div><div class='down-list'><div class='content'><li class='down-list2'><a class='folder' title='1080p'>label</a><span><a href='https://pan.baidu.com/s/abc?pwd=abcd'>download</a></span></li></div></div>")).expect(1).mount(&s).await;
        let p = Qiwei::new(vec![Url::parse(&s.uri()).unwrap()]);
        let c = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
        let r = p.search(&c, "Film").await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "Film");
        assert_eq!(r[0].links[0].password.as_deref(), Some("abcd"));
        assert_eq!(r[0].links[0].work_title.as_deref(), Some("Film 1080p"));
        assert!(
            serde_json::from_str::<Suggest>("{\"code\":1,\"list\":[]}")
                .unwrap()
                .list
                .is_empty()
        );
        assert!(serde_json::from_str::<Suggest>("{}").is_err());
    }
}
