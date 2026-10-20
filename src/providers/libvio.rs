//! LIBVIO's direct/legacy playback protocol and command-local CDN PoW guard.
use std::{
    collections::HashSet,
    sync::{LazyLock, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::Html;
use sha2::Digest;
use tokio::sync::{Mutex as AsyncMutex, OnceCell};
use url::Url;

use super::{UA, body, bounded, check, protocol, selector, send, text};
use crate::{
    core::{CloudType, Link, ProviderError, SearchResult, extract_password},
    http::{ClientOptions, HttpClientFactory},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};

const TIMEOUT: Duration = Duration::from_secs(30);
const VERIFIED_TTL: Duration = Duration::from_secs(25 * 60);
const MAX_NONCES: u32 = 1 << 22;
static PARAMS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"var\s+TS\s*=\s*"(\d+)"\s*,\s*SIG\s*=\s*"([0-9a-f]+)"\s*,\s*DIFF\s*=\s*"([^"]*)"\s*,\s*MODE\s*=\s*"([A-Za-z0-9_-]+)""#).unwrap()
});
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/detail/(\d+)\.html").unwrap());
static PLAYER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)var\s+player_aaaa\s*=\s*(\{[^}]+\})").unwrap());

pub struct Libvio {
    base: Url,
    options: ClientOptions,
    clients: OnceCell<(reqwest::Client, reqwest::Client)>,
    verified: Mutex<Option<(String, tokio::time::Instant)>>,
    flight: AsyncMutex<()>,
}
impl Default for Libvio {
    fn default() -> Self {
        Self::new("https://libvio.host/".parse().unwrap())
    }
}
impl Libvio {
    pub fn new(base: Url) -> Self {
        Self::with_options(base, ClientOptions::default())
    }
    pub fn with_options(base: Url, options: ClientOptions) -> Self {
        Self {
            base,
            options,
            clients: OnceCell::new(),
            verified: Mutex::new(None),
            flight: AsyncMutex::new(()),
        }
    }
    async fn clients(&self) -> Result<&(reqwest::Client, reqwest::Client), ProviderError> {
        self.clients
            .get_or_try_init(|| async {
                let factory = HttpClientFactory::new();
                // Neither client has a jar: __cdn_verified and __cdn_pow must never
                // be duplicated by a cookie store on the handshake request.
                Ok((
                    factory
                        .client(&self.options)
                        .map_err(|e| protocol(e.to_string()))?,
                    factory
                        .no_follow(&self.options)
                        .map_err(|e| protocol(e.to_string()))?,
                ))
            })
            .await
    }
    fn cookie(&self) -> Option<String> {
        let mut cookie = self.verified.lock().unwrap();
        if cookie
            .as_ref()
            .is_some_and(|(_, expiry)| *expiry <= tokio::time::Instant::now())
        {
            *cookie = None;
        }
        cookie.as_ref().map(|(value, _)| value.clone())
    }
    fn clear_cookie(&self, rejected: &str) {
        let mut cookie = self.verified.lock().unwrap();
        if cookie.as_ref().is_some_and(|(value, _)| value == rejected) {
            *cookie = None;
        }
    }
    async fn once(
        &self,
        ctx: &SearchContext,
        url: Url,
        referer: &str,
        cookie: Option<&str>,
        no_follow: bool,
    ) -> Result<reqwest::Response, ProviderError> {
        let clients = self.clients().await?;
        let client = if no_follow { &clients.1 } else { &clients.0 };
        let mut request = client
            .get(url)
            .header("User-Agent", UA)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
            .header("Referer", referer)
            .header("Cache-Control", "no-cache");
        if let Some(cookie) = cookie {
            request = request.header("Cookie", cookie);
        }
        send(ctx, request, TIMEOUT).await
    }
    async fn page(
        &self,
        ctx: &SearchContext,
        url: Url,
        referer: &str,
    ) -> Result<String, ProviderError> {
        let used = self.cookie();
        let cookie_header = used.as_ref().map(|value| format!("__cdn_verified={value}"));
        let response = self
            .once(ctx, url.clone(), referer, cookie_header.as_deref(), false)
            .await?;
        if response.status() != reqwest::StatusCode::FORBIDDEN {
            return normal_body(response).await;
        }
        let header_challenge = response
            .headers()
            .get("X-CDN-Challenge")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("required"));
        let challenge = if header_challenge {
            drop(response);
            true
        } else {
            // A geographic block is not a puzzle. Bound the complete response
            // by the normal HTTP limit, but inspect only its first 64 KiB. A
            // longer challenge page is not itself a probe-overflow error.
            let source = body(response, 16 << 20).await?;
            source.as_bytes()[..source.len().min(64 << 10)]
                .windows(b"__cdn_pow".len())
                .any(|window| window == b"__cdn_pow")
        };
        if !challenge {
            return Err(protocol("libvio HTTP 403 (not a CDN challenge)"));
        }
        let guard = self.flight.lock().await;
        check(ctx)?;
        let current = self.cookie();
        let verified = if let Some(current) = current.filter(|value| Some(value) != used.as_ref()) {
            current
        } else {
            if let Some(value) = used {
                self.clear_cookie(&value);
            }
            let verified = self.solve(ctx).await?;
            *self.verified.lock().unwrap() =
                Some((verified.clone(), tokio::time::Instant::now() + VERIFIED_TTL));
            verified
        };
        drop(guard);
        let response = self
            .once(
                ctx,
                url,
                referer,
                Some(&format!("__cdn_verified={verified}")),
                false,
            )
            .await?;
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            self.clear_cookie(&verified);
        }
        normal_body(response).await
    }
    async fn solve(&self, ctx: &SearchContext) -> Result<String, ProviderError> {
        let response = self
            .once(ctx, self.base.clone(), self.base.as_str(), None, false)
            .await?;
        if !matches!(response.status().as_u16(), 200 | 403) {
            return Err(protocol(format!(
                "libvio challenge HTTP {}",
                response.status().as_u16()
            )));
        }
        let source = body(response, 1 << 20).await?;
        let params = PARAMS
            .captures(&source)
            .ok_or_else(|| ProviderError::Parse("libvio challenge parameters missing".into()))?;
        let nonce = solve_nonce(ctx, &params[2], &params[3]).await?;
        let cookie = format!(
            "__cdn_pow={}_{}_{}_{}",
            &params[1], &params[4], nonce, &params[2]
        );
        let response = self
            .once(
                ctx,
                self.base.clone(),
                self.base.as_str(),
                Some(&cookie),
                true,
            )
            .await?;
        let status = response.status().as_u16();
        if !matches!(status, 200 | 301 | 302 | 303 | 307 | 308) {
            return Err(protocol(format!("libvio PoW submission HTTP {status}")));
        }
        let verified = response
            .cookies()
            .find(|cookie| {
                cookie.name() == "__cdn_verified"
                    && !cookie.value().is_empty()
                    && cookie.max_age().is_none_or(|age| !age.is_zero())
            })
            .map(|cookie| cookie.value().to_owned());
        // No redirect, no jar, and no echo of cookies/PoW parameters in errors.
        drop(response);
        verified.ok_or_else(|| {
            protocol(format!(
                "libvio verification cookie missing (HTTP {status})"
            ))
        })
    }
    fn items(&self, source: &str) -> Result<Vec<(SearchResult, Url)>, ProviderError> {
        let doc = Html::parse_document(source);
        if doc.select(&selector("ul.stui-vodlist")).next().is_none() {
            return Err(ProviderError::Parse(
                "libvio missing search result list".into(),
            ));
        }
        let mut rows = Vec::new();
        let mut seen = HashSet::new();
        for item in doc.select(&selector("ul.stui-vodlist li")) {
            let anchor = item.select(&selector(".stui-vodlist__detail h4 a")).next();
            let title = anchor
                .map(text)
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    anchor
                        .and_then(|node| node.value().attr("title"))
                        .map(str::to_owned)
                });
            let href = anchor
                .and_then(|node| node.value().attr("href"))
                .or_else(|| {
                    item.select(&selector("a.stui-vodlist__thumb"))
                        .next()
                        .and_then(|node| node.value().attr("href"))
                });
            let (Some(title), Some(href)) = (title, href) else {
                continue;
            };
            let Some(id) = ID
                .captures(href)
                .and_then(|m| m.get(1))
                .map(|m| m.as_str().to_owned())
            else {
                continue;
            };
            if !seen.insert(id.clone()) {
                continue;
            }
            let url = self
                .base
                .join(href)
                .map_err(|_| protocol("libvio invalid detail URL"))?;
            if url.origin() != self.base.origin() {
                continue;
            }
            let mut row = SearchResult::provider(format!("libvio-{id}"), "libvio");
            row.title = title;
            row.content = [".pic-text", ".pic-tag"]
                .iter()
                .filter_map(|css| item.select(&selector(css)).next().map(text))
                .collect::<Vec<_>>()
                .join(" | ");
            if let Some(image) = item
                .select(&selector("a.stui-vodlist__thumb"))
                .next()
                .and_then(|node| {
                    node.value()
                        .attr("data-original")
                        .or_else(|| node.value().attr("data-src"))
                })
                && let Ok(url) = self.base.join(image)
            {
                row.images.push(url.to_string());
            }
            rows.push((row, url));
        }
        if rows.is_empty() && doc.select(&selector("ul.stui-vodlist li")).next().is_some() {
            return Err(ProviderError::Parse("libvio malformed search items".into()));
        }
        Ok(rows)
    }
    async fn detail(
        &self,
        ctx: &SearchContext,
        mut row: SearchResult,
        url: Url,
    ) -> Result<SearchResult, ProviderError> {
        let source = self.page(ctx, url.clone(), self.base.as_str()).await?;
        let (mut links, plays) = {
            let doc = Html::parse_document(&source);
            let mut seen = HashSet::new();
            let links = doc
                .select(&selector(".netdisk-panel a.netdisk-item[href]"))
                .filter_map(|node| {
                    let raw = node.value().attr("href")?;
                    let link = share(raw, &row.title)?;
                    seen.insert(link.url.clone()).then_some(link)
                })
                .collect::<Vec<_>>();
            let plays = if links.is_empty() {
                doc.select(&selector(".stui-vodlist__head"))
                    .filter(|node| {
                        node.select(&selector("h3"))
                            .next()
                            .is_some_and(|title| text(title).contains("下载"))
                    })
                    .filter_map(|node| {
                        node.select(&selector(".stui-content__playlist li a[href]"))
                            .next()
                            .and_then(|node| node.value().attr("href"))
                    })
                    .filter_map(|href| self.base.join(href).ok())
                    .filter(|url| url.origin() == self.base.origin())
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            (links, plays)
        };
        let mut first_error = None;
        for play in plays {
            let outcome = async {
                let source = self.page(ctx, play, url.as_str()).await?;
                let raw = PLAYER
                    .captures(&source)
                    .and_then(|m| m.get(1))
                    .ok_or_else(|| ProviderError::Parse("libvio player_aaaa missing".into()))?;
                let player: serde_json::Value = serde_json::from_str(raw.as_str())
                    .map_err(|_| ProviderError::Parse("libvio malformed player_aaaa".into()))?;
                let link = player
                    .get("url")
                    .and_then(|value| value.as_str())
                    .and_then(|raw| share(raw, &row.title))
                    .ok_or_else(|| protocol("libvio invalid playback share"))?;
                Ok::<_, ProviderError>(link)
            }
            .await;
            match outcome {
                Ok(link) => {
                    if !links.iter().any(|old| old.url == link.url) {
                        links.push(link);
                        // Publish a completed playback before starting the next
                        // one; a slow sibling must not erase usable downloads.
                        let mut completed = row.clone();
                        completed.links = links.clone();
                        ctx.publish(vec![completed]);
                    }
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        if links.is_empty() {
            return Err(first_error.unwrap_or_else(|| protocol("libvio detail has no downloads")));
        }
        if let Some(error) = first_error {
            return Err(protocol(format!(
                "libvio incomplete playback downloads: {error}"
            )));
        }
        row.links = links;
        Ok(row)
    }
    async fn search_inner(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let mut url = self.base.join("search/-------------.html").unwrap();
        url.query_pairs_mut()
            .append_pair("wd", query)
            .append_pair("submit", "");
        let source = self.page(ctx, url, self.base.as_str()).await?;
        let items = self.items(&source)?;
        let count = items.len();
        let mut jobs = stream::iter(
            items
                .into_iter()
                .enumerate()
                .map(
                    |(index, (row, url))| async move { (index, self.detail(ctx, row, url).await) },
                ),
        )
        .buffer_unordered(6);
        let mut rows = Vec::new();
        let mut first_error = None;
        while let Some((index, outcome)) = jobs.next().await {
            match outcome {
                Ok(row) => {
                    ctx.publish(vec![row.clone()]);
                    rows.push((index, row));
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        if let Some(error) = first_error {
            if rows.is_empty() {
                return Err(protocol(format!(
                    "libvio all {count} details failed: {error}"
                )));
            }
            return Err(protocol(format!(
                "libvio incomplete details ({}/{count} completed): {error}",
                rows.len()
            )));
        }
        rows.sort_by_key(|(index, _)| *index);
        Ok(rows.into_iter().map(|(_, row)| row).collect())
    }
}
async fn normal_body(response: reqwest::Response) -> Result<String, ProviderError> {
    if response.status() != reqwest::StatusCode::OK {
        return Err(protocol(format!(
            "libvio HTTP {}",
            response.status().as_u16()
        )));
    }
    body(response, 16 << 20).await
}
fn share(raw: &str, title: &str) -> Option<Link> {
    let url = Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let mut link = Link::new(url.to_string());
    if link.cloud_type == CloudType::Others {
        return None;
    }
    link.password = extract_password("", &link.url);
    link.work_title = Some(title.to_owned());
    Some(link)
}
async fn solve_nonce(ctx: &SearchContext, sig: &str, diff: &str) -> Result<u32, ProviderError> {
    if diff.len() > 64
        || !diff
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(protocol("libvio invalid PoW difficulty"));
    }
    let cpu_deadline = tokio::time::Instant::now() + ctx.remaining().min(Duration::from_secs(10));
    let prefix = super::hash_prefix(ctx, sig.as_bytes(), cpu_deadline).await?;
    for nonce in 0..MAX_NONCES {
        if nonce % 256 == 0 {
            check(ctx)?;
            if tokio::time::Instant::now() >= cpu_deadline {
                return Err(ProviderError::Timeout);
            }
            tokio::task::yield_now().await;
        }
        let mut hasher = prefix.clone();
        hasher.update(nonce.to_string().as_bytes());
        let digest = hasher.finalize();
        // Compare hexadecimal nibbles without allocating a hex string per hash.
        if diff.bytes().enumerate().all(|(index, byte)| {
            let value = if index % 2 == 0 {
                digest[index / 2] >> 4
            } else {
                digest[index / 2] & 15
            };
            value
                == if byte.is_ascii_digit() {
                    byte - b'0'
                } else {
                    byte - b'a' + 10
                }
        }) {
            return Ok(nonce);
        }
    }
    Err(protocol("libvio PoW exhausted 4194304 attempts"))
}
#[async_trait]
impl Provider for Libvio {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("libvio", 1, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        bounded(ctx, self.search_inner(ctx, query)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
    const CHALLENGE: &str = r#"<script>var TS = "1", SIG = "abc123", DIFF = "", MODE = "auto"; var POW="__cdn_pow";</script>"#;
    const LIST: &str = r#"<ul class="stui-vodlist"><li><div class="stui-vodlist__detail"><h4><a href="/detail/1.html">Movie</a></h4></div></li></ul>"#;
    async fn handshake(server: &MockServer, count: Arc<AtomicUsize>) {
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(move |request: &wiremock::Request| {
                if request
                    .headers
                    .get("cookie")
                    .is_some_and(|value| value == "__cdn_pow=1_auto_0_abc123")
                {
                    let generation = count.fetch_add(1, Ordering::SeqCst) + 1;
                    ResponseTemplate::new(302)
                        .insert_header(
                            "Set-Cookie",
                            format!("__cdn_verified=v{generation}; Path=/; Max-Age=1800"),
                        )
                        .insert_header("Location", "/never-follow")
                } else {
                    ResponseTemplate::new(403).set_body_string(CHALLENGE)
                }
            })
            .mount(server)
            .await;
        Mock::given(path("/never-follow"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(server)
            .await;
    }
    async fn guarded(
        server: &MockServer,
        endpoint: &'static str,
        page: &'static str,
        body_only: bool,
    ) {
        let attempts = Arc::new(AtomicUsize::new(0));
        Mock::given(path(endpoint))
            .respond_with(move |_: &wiremock::Request| {
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    if body_only {
                        ResponseTemplate::new(403).set_body_string(CHALLENGE)
                    } else {
                        ResponseTemplate::new(403).insert_header("x-cdn-challenge", "ReQuIrEd")
                    }
                } else {
                    ResponseTemplate::new(200).set_body_string(page)
                }
            })
            .expect(2)
            .mount(server)
            .await;
    }
    #[tokio::test]
    async fn challenge_on_search_detail_and_legacy_playback() {
        let server = MockServer::start().await;
        let count = Arc::new(AtomicUsize::new(0));
        handshake(&server, count.clone()).await;
        guarded(&server, "/search/-------------.html", LIST, false).await;
        guarded(&server, "/detail/1.html", r#"<div class="stui-vodlist__head"><h3>夸克下载</h3><ul class="stui-content__playlist"><li><a href="/play/1.html">合集</a></li></ul></div>"#, true).await;
        guarded(&server, "/play/1.html", r#"<script>var player_aaaa={"from":"quark","url":"https:\/\/pan.quark.cn\/s\/abcdef?pwd=1234"};</script>"#, false).await;
        let provider = Libvio::new(server.uri().parse().unwrap());
        let rows = provider
            .search(&super::super::context(), "Movie")
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].links[0].password.as_deref(), Some("1234"));
        assert_eq!(rows[0].links[0].work_title.as_deref(), Some("Movie"));
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }
    #[tokio::test]
    async fn concurrent_requests_share_handshake_and_verified_cookie() {
        let server = MockServer::start().await;
        let count = Arc::new(AtomicUsize::new(0));
        handshake(&server, count.clone()).await;
        Mock::given(path("/resource"))
            .respond_with(|request: &wiremock::Request| {
                if request
                    .headers
                    .get("cookie")
                    .is_some_and(|value| value == "__cdn_verified=v1")
                {
                    ResponseTemplate::new(200).set_body_string("ok")
                } else {
                    ResponseTemplate::new(403).insert_header("X-CDN-Challenge", "required")
                }
            })
            .mount(&server)
            .await;
        let provider = Libvio::new(server.uri().parse().unwrap());
        let ctx = super::super::context();
        let url = provider.base.join("resource").unwrap();
        let (first, second) = tokio::join!(
            provider.page(&ctx, url.clone(), provider.base.as_str()),
            provider.page(&ctx, url, provider.base.as_str())
        );
        assert_eq!(first.unwrap(), "ok");
        assert_eq!(second.unwrap(), "ok");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(
            provider
                .page(
                    &ctx,
                    provider.base.join("resource").unwrap(),
                    provider.base.as_str()
                )
                .await
                .unwrap(),
            "ok"
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn empty_malformed_regional_block_and_missing_cookie() {
        let server = MockServer::start().await;
        for (response, expected) in [
            (
                ResponseTemplate::new(200).set_body_string("<ul class=stui-vodlist></ul>"),
                None,
            ),
            (
                ResponseTemplate::new(200).set_body_string("broken search"),
                Some("missing search result list"),
            ),
            (
                ResponseTemplate::new(403)
                    .set_body_string("Access denied by geographic restriction"),
                Some("not a CDN challenge"),
            ),
            (ResponseTemplate::new(500), Some("500")),
        ] {
            server.reset().await;
            Mock::given(path("/search/-------------.html"))
                .respond_with(response)
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(path("/"))
                .respond_with(ResponseTemplate::new(200))
                .expect(0)
                .mount(&server)
                .await;
            let result = Libvio::new(server.uri().parse().unwrap())
                .search(&super::super::context(), "Movie")
                .await;
            if let Some(message) = expected {
                assert!(result.unwrap_err().to_string().contains(message));
            } else {
                assert!(result.unwrap().is_empty());
            }
        }
        server.reset().await;
        Mock::given(path("/search/-------------.html"))
            .respond_with(ResponseTemplate::new(403).insert_header("X-CDN-Challenge", "required"))
            .mount(&server)
            .await;
        Mock::given(path("/"))
            .respond_with(|request: &wiremock::Request| {
                if request.headers.contains_key("cookie") {
                    ResponseTemplate::new(302)
                } else {
                    ResponseTemplate::new(403).set_body_string(CHALLENGE)
                }
            })
            .mount(&server)
            .await;
        assert!(
            Libvio::new(server.uri().parse().unwrap())
                .search(&super::super::context(), "Movie")
                .await
                .unwrap_err()
                .to_string()
                .contains("verification cookie missing")
        );
    }
    #[tokio::test]
    async fn direct_download_partial_and_cancellation_publish_completed() {
        let server = MockServer::start().await;
        let list = LIST.replace("</ul>", r#"<li><div class="stui-vodlist__detail"><h4><a href="/detail/2.html">Movie 2</a></h4></div></li></ul>"#);
        Mock::given(path("/search/-------------.html"))
            .respond_with(ResponseTemplate::new(200).set_body_string(list))
            .mount(&server)
            .await;
        Mock::given(path("/detail/1.html")).respond_with(ResponseTemplate::new(200).set_body_string(r#"<div class="netdisk-panel"><a class="netdisk-item" href="https://pan.baidu.com/s/abcdef?pwd=a1b2">下载</a><a class="netdisk-item" href="https://evil.invalid/s/nope">ignore</a></div>"#)).mount(&server).await;
        let failing = Mock::given(path("/detail/2.html"))
            .respond_with(ResponseTemplate::new(503))
            .mount_as_scoped(&server)
            .await;
        let provider = Arc::new(Libvio::new(server.uri().parse().unwrap()));
        let rows = super::super::assert_failed_partial(provider.clone(), "Movie").await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].links.len(), 1);
        assert_eq!(rows[0].links[0].password.as_deref(), Some("a1b2"));
        drop(failing);
        Mock::given(path("/detail/2.html"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(1)))
            .mount(&server)
            .await;
        let ctx = super::super::context()
            .for_source(tokio::time::Instant::now() + Duration::from_millis(150));
        assert_eq!(
            provider.search(&ctx, "Movie").await.unwrap_err(),
            ProviderError::Timeout
        );
        assert!(ctx.has_results());
    }
    #[tokio::test]
    async fn first_playback_is_published_before_slow_sibling() {
        let server = MockServer::start().await;
        Mock::given(path("/search/-------------.html"))
            .respond_with(ResponseTemplate::new(200).set_body_string(LIST))
            .mount(&server)
            .await;
        Mock::given(path("/detail/1.html")).respond_with(ResponseTemplate::new(200).set_body_string(r#"<div class="stui-vodlist__head"><h3>夸克下载</h3><ul class="stui-content__playlist"><li><a href="/play/first">合集</a></li></ul></div><div class="stui-vodlist__head"><h3>百度下载</h3><ul class="stui-content__playlist"><li><a href="/play/slow">合集</a></li></ul></div>"#)).mount(&server).await;
        Mock::given(path("/play/first"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"var player_aaaa={"url":"https://pan.quark.cn/s/abcdef"};"#),
            )
            .mount(&server)
            .await;
        Mock::given(path("/play/slow"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(1)))
            .mount(&server)
            .await;
        let ctx = super::super::context()
            .for_source(tokio::time::Instant::now() + Duration::from_millis(150));
        assert_eq!(
            Libvio::new(server.uri().parse().unwrap())
                .search(&ctx, "Movie")
                .await
                .unwrap_err(),
            ProviderError::Timeout
        );
        assert!(ctx.has_results());
    }

    #[tokio::test]
    async fn mixed_playback_failure_marks_source_incomplete() {
        let server = MockServer::start().await;
        Mock::given(path("/search/-------------.html"))
            .respond_with(ResponseTemplate::new(200).set_body_string(LIST))
            .mount(&server)
            .await;
        Mock::given(path("/detail/1.html")).respond_with(ResponseTemplate::new(200).set_body_string(r#"<div class="stui-vodlist__head"><h3>夸克下载</h3><ul class="stui-content__playlist"><li><a href="/play/first">合集</a></li></ul></div><div class="stui-vodlist__head"><h3>百度下载</h3><ul class="stui-content__playlist"><li><a href="/play/failed">合集</a></li></ul></div>"#)).mount(&server).await;
        Mock::given(path("/play/first"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"var player_aaaa={"url":"https://pan.quark.cn/s/abcdef"};"#),
            )
            .mount(&server)
            .await;
        Mock::given(path("/play/failed"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let rows = super::super::assert_failed_partial(
            Arc::new(Libvio::new(server.uri().parse().unwrap())),
            "Movie",
        )
        .await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].links.len(), 1);
    }

    #[tokio::test]
    async fn challenge_marker_in_prefix_of_large_403_body_is_recognized() {
        let server = MockServer::start().await;
        let count = Arc::new(AtomicUsize::new(0));
        handshake(&server, count.clone()).await;
        let oversized_probe = format!("{CHALLENGE}{}", " ".repeat(70 * 1024));
        Mock::given(path("/search/-------------.html"))
            .respond_with(move |request: &wiremock::Request| {
                if request
                    .headers
                    .get("cookie")
                    .is_some_and(|value| value == "__cdn_verified=v1")
                {
                    ResponseTemplate::new(200).set_body_string(LIST)
                } else {
                    ResponseTemplate::new(403).set_body_string(oversized_probe.clone())
                }
            })
            .mount(&server)
            .await;
        Mock::given(path("/detail/1.html")).respond_with(ResponseTemplate::new(200).set_body_string(r#"<div class="netdisk-panel"><a class="netdisk-item" href="https://pan.quark.cn/s/abcdef">下载</a></div>"#)).mount(&server).await;
        let rows = Libvio::new(server.uri().parse().unwrap())
            .search(&super::super::context(), "Movie")
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        server.reset().await;
        let outside_probe = format!("{}__cdn_pow", " ".repeat(64 * 1024));
        Mock::given(path("/search/-------------.html"))
            .respond_with(ResponseTemplate::new(403).set_body_string(outside_probe))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        assert!(
            Libvio::new(server.uri().parse().unwrap())
                .search(&super::super::context(), "Movie")
                .await
                .unwrap_err()
                .to_string()
                .contains("not a CDN challenge")
        );
    }

    #[tokio::test]
    async fn known_minimum_nonces_cookie_expiry_and_cpu_cancel() {
        let ctx = super::super::context();
        assert_eq!(
            solve_nonce(
                &ctx,
                "0a8db1316598f1f8da9fe22470e654ffdf1c988ad685026f3ab86860222c383e",
                "0000"
            )
            .await
            .unwrap(),
            1657
        );
        assert_eq!(
            solve_nonce(&ctx, "18f3ad6e47ebd6e44704eab02137", "0000")
                .await
                .unwrap(),
            133839
        );
        let provider = Libvio::default();
        *provider.verified.lock().unwrap() = Some((
            "private".into(),
            tokio::time::Instant::now() - Duration::from_secs(1),
        ));
        assert!(provider.cookie().is_none());
        *provider.verified.lock().unwrap() =
            Some(("private".into(), tokio::time::Instant::now() + VERIFIED_TTL));
        provider.clear_cookie("different");
        assert!(provider.cookie().is_some());
        provider.clear_cookie("private");
        assert!(provider.cookie().is_none());
        let ctx = super::super::context()
            .for_source(tokio::time::Instant::now() + Duration::from_millis(2));
        assert_eq!(
            solve_nonce(&ctx, "abc123", &"0".repeat(64))
                .await
                .unwrap_err(),
            ProviderError::Timeout
        );
        assert!(
            solve_nonce(&super::super::context(), "abc", &"0".repeat(65))
                .await
                .is_err()
        );
        assert_eq!(MAX_NONCES, 4_194_304);
    }
}
