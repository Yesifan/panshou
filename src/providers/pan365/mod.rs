//! 365 聚合站的双线路搜索和公开分享转存协议。
use crate::core::{Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::Html;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex, Weak},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use url::Url;

const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/131.0.0.0 Safari/537.36";
const LIMIT: usize = 2 << 20;
static PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^/s/[A-Za-z0-9_-]+/?$").unwrap());
static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9]{4,8}$").unwrap());
static CODE_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:提取码|访问码|密码|pwd|password)\s*[:：=]?\s*([a-z0-9]{4,8})(?:[^a-z0-9]|$)",
    )
    .unwrap()
});
#[derive(Clone)]
struct Share {
    link: Link,
    identity: String,
}
struct State {
    memo: HashMap<String, (Instant, Share)>,
    blocked: Option<(Instant, bool)>,
    in_flight: HashMap<String, Weak<tokio::sync::Mutex<()>>>,
}
pub struct Pan365 {
    base: Url,
    slots: Semaphore,
    state: Mutex<State>,
}
impl Default for Pan365 {
    fn default() -> Self {
        Self::new(Url::parse("https://pan.365wp.top/").unwrap())
    }
}
#[derive(Deserialize, Clone)]
struct Item {
    title: String,
    url: String,
    #[serde(default)]
    source: String,
    disk_type: String,
}
#[derive(Deserialize)]
struct SearchResponse {
    code: i64,
    data: Option<SearchData>,
}
#[derive(Deserialize)]
struct SearchData {
    list: Option<Vec<Item>>,
    #[serde(default, rename = "search_stats")]
    stats: Vec<Stat>,
}
#[derive(Deserialize)]
struct Stat {
    success: bool,
}
#[derive(Deserialize)]
struct TransferResponse {
    code: i64,
    data: Option<TransferData>,
}
#[derive(Deserialize)]
struct TransferData {
    #[serde(default)]
    original_url: String,
    share_url: String,
    #[serde(default)]
    passcode: String,
}

fn unescape(s: &str) -> String {
    Html::parse_fragment(s)
        .root_element()
        .text()
        .collect::<String>()
}
fn normalize(raw: &str, passcode: &str) -> Option<Link> {
    let raw = unescape(raw);
    let mut fields = raw.split_whitespace();
    let first = fields.next()?;
    let mut url = Url::parse(first).ok()?;
    // URL normalisation removes default ports, so check the original authority too.
    let authority = first.split_once("://")?.1.split(['/', '?', '#']).next()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || authority.contains(['@', ':'])
        || !PATH.is_match(url.path())
    {
        return None;
    }
    let host = url.host_str()?;
    if !matches!(host, "pan.quark.cn" | "pan.baidu.com") {
        return None;
    }
    // form_urlencoded tolerates malformed percent escapes; this protocol does not.
    if let Some(q) = url.query() {
        let b = q.as_bytes();
        for (i, c) in b.iter().enumerate() {
            if *c == b'%'
                && (i + 2 >= b.len()
                    || !b[i + 1].is_ascii_hexdigit()
                    || !b[i + 2].is_ascii_hexdigit())
            {
                return None;
            }
        }
    }
    let password = ["pwd", "password"]
        .into_iter()
        .find_map(|key| {
            url.query_pairs()
                .find(|(k, v)| k == key && CODE.is_match(v))
                .map(|(_, v)| v.into_owned())
        })
        .or_else(|| CODE.is_match(passcode).then(|| passcode.to_owned()))
        .or_else(|| {
            CODE_TEXT
                .captures(&format!(
                    "{} {passcode}",
                    fields.collect::<Vec<_>>().join(" ")
                ))
                .map(|c| c[1].to_owned())
        });
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(&path);
    if url.query() == Some("") {
        url.set_query(None);
    }
    let mut link = Link::new(url.as_str());
    link.password = password;
    Some(link)
}
fn identity(link: &Link) -> String {
    format!(
        "{}\0{}",
        link.cloud_type,
        Url::parse(&link.url).unwrap().path()
    )
}
fn matches(title: &str, query: &str) -> bool {
    !title.is_empty()
        && query
            .to_lowercase()
            .split_whitespace()
            .all(|w| title.to_lowercase().contains(w))
}
impl Pan365 {
    pub fn new(base: Url) -> Self {
        Self {
            base,
            slots: Semaphore::new(3),
            state: Mutex::new(State {
                memo: HashMap::new(),
                blocked: None,
                in_flight: HashMap::new(),
            }),
        }
    }
    fn cooldown(&self) -> Result<(), ProviderError> {
        if let Some((until, challenge)) = self.state.lock().unwrap().blocked
            && Instant::now() < until
        {
            return Err(if challenge {
                ProviderError::Protocol("pan365 CHALLENGE_REQUIRED; 暂停请求 1 分钟".into())
            } else {
                ProviderError::RateLimited
            });
        }
        Ok(())
    }
    fn api_error(&self, code: i64) -> ProviderError {
        if matches!(code, 403 | 429 | 42901) {
            self.state.lock().unwrap().blocked =
                Some((Instant::now() + Duration::from_secs(60), code != 429));
        }
        if code == 429 {
            ProviderError::RateLimited
        } else if matches!(code, 403 | 42901) {
            ProviderError::Protocol(format!("pan365 CHALLENGE_REQUIRED (code={code})"))
        } else {
            ProviderError::Protocol(format!("pan365 code={code}"))
        }
    }
    async fn request<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, ProviderError> {
        self.cooldown()?;
        let future = async {
            let response = crate::http::execute(
                request
                    .header("user-agent", UA)
                    .header("accept", "application/json, */*")
                    .header("accept-language", "zh-CN,zh;q=0.9")
                    .header("referer", self.base.as_str()),
            )
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
            if response.status().as_u16() != 200 {
                return Err(self.api_error(response.status().as_u16().into()));
            }
            let body = crate::http::read_response(response, LIMIT)
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?;
            serde_json::from_slice(&body)
                .map_err(|_| ProviderError::Parse("pan365 响应不是有效的公开分享 JSON".into()))
        };
        tokio::time::timeout(Duration::from_secs(12), future)
            .await
            .map_err(|_| ProviderError::Timeout)?
    }
    async fn batch(
        &self,
        ctx: &SearchContext,
        query: &str,
        kind: &str,
    ) -> Result<Vec<Item>, ProviderError> {
        let response: SearchResponse = self
            .request(
                ctx.client
                    .get(self.base.join("api/interface/search").unwrap())
                    .query(&[
                        ("keyword", query),
                        ("disk_type", kind),
                        ("page", "1"),
                        ("pageSize", "20"),
                    ]),
            )
            .await?;
        if response.code != 200 {
            return Err(self.api_error(response.code));
        }
        let data = response
            .data
            .ok_or_else(|| ProviderError::Protocol("pan365 missing data".into()))?;
        let list = data
            .list
            .ok_or_else(|| ProviderError::Protocol("pan365 missing list".into()))?;
        if list.is_empty() && !data.stats.is_empty() && data.stats.iter().all(|s| !s.success) {
            return Err(ProviderError::Protocol(format!(
                "pan365 {kind} 搜索线路全部失败"
            )));
        }
        let mut seen = HashSet::new();
        Ok(list
            .into_iter()
            .map(|mut i| {
                i.title = unescape(&i.title)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                i.url = i.url.trim().to_owned();
                i
            })
            .filter(|i| {
                i.disk_type == kind
                    && !i.url.is_empty()
                    && i.url.len() <= 8192
                    && matches(&i.title, query)
                    && seen.insert(i.url.clone())
            })
            .take(6)
            .collect())
    }
    fn cached(&self, key: &str) -> Option<Share> {
        self.state
            .lock()
            .unwrap()
            .memo
            .get(key)
            .filter(|(until, _)| Instant::now() < *until)
            .map(|(_, s)| s.clone())
    }
    async fn resolve(&self, ctx: &SearchContext, item: &Item) -> Result<Share, ProviderError> {
        let key = format!("{}\0{}", item.disk_type, item.url);
        if let Some(s) = self.cached(&key) {
            return Ok(s);
        }
        let flight = {
            let mut state = self.state.lock().unwrap();
            state.in_flight.retain(|_, v| v.strong_count() > 0);
            let lock = state
                .in_flight
                .get(&key)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| Arc::new(tokio::sync::Mutex::new(())));
            state.in_flight.insert(key.clone(), Arc::downgrade(&lock));
            lock
        };
        let _flight = flight.lock().await;
        let _permit = self
            .slots
            .acquire()
            .await
            .map_err(|_| ProviderError::Timeout)?;
        if let Some(s) = self.cached(&key) {
            return Ok(s);
        }
        if let Some(link) = normalize(&item.url, "")
            && link.cloud_type.to_string() == item.disk_type
        {
            return Ok(Share {
                identity: identity(&link),
                link,
            });
        }
        let response: TransferResponse = self
            .request(
                ctx.client
                    .post(self.base.join("api/transfer-share/transfer-share").unwrap())
                    .header("origin", self.base.origin().ascii_serialization())
                    .json(&serde_json::json!({"encrypted_url":item.url})),
            )
            .await?;
        if response.code != 200 {
            return Err(self.api_error(response.code));
        }
        let data = response
            .data
            .ok_or_else(|| ProviderError::Protocol("pan365 transfer missing data".into()))?;
        let link = normalize(&data.share_url, &data.passcode)
            .filter(|l| l.cloud_type.to_string() == item.disk_type)
            .ok_or_else(|| ProviderError::Protocol("pan365 无效分享链接".into()))?;
        let stable = normalize(&data.original_url, "")
            .filter(|l| l.cloud_type.to_string() == item.disk_type)
            .map(|l| identity(&l))
            .unwrap_or_else(|| format!("{}\0{}\0{}", item.disk_type, item.source, item.title));
        let share = Share {
            link,
            identity: stable,
        };
        let mut state = self.state.lock().unwrap();
        if state.memo.len() >= 256
            && !state.memo.contains_key(&key)
            && let Some(oldest) = state
                .memo
                .iter()
                .min_by_key(|(_, (t, _))| *t)
                .map(|(k, _)| k.clone())
        {
            state.memo.remove(&oldest);
        }
        state.memo.insert(
            key,
            (Instant::now() + Duration::from_secs(1800), share.clone()),
        );
        Ok(share)
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let batch = tokio::join!(
            tokio::time::timeout(Duration::from_secs(10), self.batch(ctx, query, "quark")),
            tokio::time::timeout(Duration::from_secs(10), self.batch(ctx, query, "baidu"))
        );
        let mut error = None;
        let mut batches = Vec::new();
        for b in [
            batch.0.unwrap_or(Err(ProviderError::Timeout)),
            batch.1.unwrap_or(Err(ProviderError::Timeout)),
        ] {
            match b {
                Ok(v) => batches.push(v),
                Err(e) => {
                    error = Some(e);
                    batches.push(vec![]);
                }
            }
        }
        let entries = (0..6)
            .flat_map(|i| batches.iter().filter_map(move |b| b.get(i).cloned()))
            .collect::<Vec<_>>();
        let mut jobs = stream::iter(entries.into_iter().map(|item| async move {
            let share = self.resolve(ctx, &item).await;
            if let Ok(s) = &share {
                ctx.publish(vec![make_result(&item, s.clone())]);
            }
            (item, share)
        }))
        .buffered(3);
        let mut seen = HashSet::new();
        let mut results = Vec::new();
        while let Some((item, share)) = jobs.next().await {
            match share {
                Ok(s) => {
                    if !seen.insert(s.identity.clone()) {
                        continue;
                    }
                    results.push(make_result(&item, s));
                }
                Err(e) => error = Some(e),
            }
        }
        if let Some(e) = error {
            return Err(e);
        }
        Ok(results)
    }
}
fn make_result(item: &Item, mut share: Share) -> SearchResult {
    let hash = Sha256::digest(share.identity.as_bytes());
    let mut result = SearchResult::provider(
        format!(
            "pan365-{}",
            hash[..16]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ),
        "pan365",
    );
    result.title = item.title.clone();
    result.content = format!("来源：365 聚合站 · {}", item.source);
    share.link.work_title = Some(item.title.clone());
    result.links.push(share.link);
    result
}
#[async_trait]
impl Provider for Pan365 {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("pan365", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        tokio::time::timeout(
            ctx.timeout.min(Duration::from_secs(28)),
            self.run(ctx, query.trim()),
        )
        .await
        .map_err(|_| ProviderError::Timeout)?
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, headers, method, path, query_param},
    };
    #[test]
    fn strict_links_and_password_priority() {
        let l = normalize(
            "https://PAN.BAIDU.COM/s/abc/?pwd=abcd&password=efgh#x",
            "zzzz",
        )
        .unwrap();
        assert_eq!(l.password.as_deref(), Some("abcd"));
        assert!(l.url.contains("/s/abc?"));
        assert!(l.url.ends_with("#x"));
        for bad in [
            "https://pan.baidu.com.evil/s/x",
            "https://user@pan.baidu.com/s/x",
            "https://pan.baidu.com:443/s/x",
            "https://pan.baidu.com/s/",
            "https://pan.baidu.com/s/x?a=%gg",
            "https://evil/s/x?u=https://pan.baidu.com/s/x",
        ] {
            assert!(normalize(bad, "").is_none(), "{bad}");
        }
        assert_eq!(
            normalize("https://pan.quark.cn/s/abc?", "提取码：1234")
                .unwrap()
                .password
                .as_deref(),
            Some("1234")
        );
    }
    #[tokio::test]
    async fn both_lines_transfer_memo_and_stable_identity() {
        let server = MockServer::start().await;
        for (kind, host) in [("quark", "pan.quark.cn"), ("baidu", "pan.baidu.com")] {
            Mock::given(method("GET")).and(path("/api/interface/search")).and(query_param("disk_type",kind)).and(headers("accept",vec!["application/json", "*/*"])).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"code":200,"data":{"list":[{"title":"Film &amp; One","url":format!("{kind}+/="),"source":"s","disk_type":kind}],"search_stats":[{"success":false},{"success":true}]}}))).expect(2).mount(&server).await;
            Mock::given(method("POST")).and(path("/api/transfer-share/transfer-share")).and(body_json(serde_json::json!({"encrypted_url":format!("{kind}+/=")}))).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"code":200,"data":{"original_url":format!("https://{host}/s/original"),"share_url":format!("https://{host}/s/new"),"passcode":"1234","file_info":{"secret":"not public"}}}))).expect(1).mount(&server).await;
        }
        let provider = Pan365::new(Url::parse(&server.uri()).unwrap());
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
        let mut first = provider.search(&ctx, "film one").await.unwrap();
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].title, "Film & One");
        let id = first[0].id.clone();
        first[0].links[0].url = "corrupt".into();
        let second = provider.search(&ctx, "film one").await.unwrap();
        assert_eq!(second[0].id, id);
        assert!(second[0].links[0].url.ends_with("/new"));
    }
    #[tokio::test]
    async fn partial_line_failure_keeps_published_rows_but_fails_source() {
        let s = MockServer::start().await;
        Mock::given(query_param("disk_type","quark")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"code":200,"data":{"list":[{"title":"Film","url":"https://pan.quark.cn/s/direct?pwd=abcd","disk_type":"quark"}]}}))).expect(1).mount(&s).await;
        Mock::given(query_param("disk_type", "baidu"))
            .respond_with(ResponseTemplate::new(503))
            .expect(1)
            .mount(&s)
            .await;
        let p = Pan365::new(Url::parse(&s.uri()).unwrap());
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(3));
        assert!(
            p.search(&ctx, "film")
                .await
                .unwrap_err()
                .to_string()
                .contains("503")
        );
        assert!(ctx.has_results());
    }
    #[tokio::test]
    async fn search_response_limit_plus_one_is_not_empty() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b' '; LIMIT + 1]))
            .mount(&s)
            .await;
        let p = Pan365::new(Url::parse(&s.uri()).unwrap());
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
        assert!(p.search(&ctx, "film").await.is_err());
        assert!(!ctx.has_results());
    }
    #[tokio::test]
    async fn empty_schema_and_cooldown() {
        for (body, ok) in [
            (r#"{"code":200,"data":{"list":[]}}"#, true),
            (r#"{"code":200,"data":{}}"#, false),
            (
                r#"{"code":200,"data":{"list":[],"search_stats":[{"success":false}]}}"#,
                false,
            ),
            ("<html>", false),
        ] {
            let s = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_string(body))
                .mount(&s)
                .await;
            let p = Pan365::new(Url::parse(&s.uri()).unwrap());
            let c = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
            assert_eq!(p.search(&c, "a").await.is_ok(), ok);
        }
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&s)
            .await;
        let p = Pan365::new(Url::parse(&s.uri()).unwrap());
        let c = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
        assert!(p.search(&c, "a").await.is_err());
        let count = s.received_requests().await.unwrap().len();
        assert!(p.search(&c, "a").await.is_err());
        assert_eq!(s.received_requests().await.unwrap().len(), count);
    }
}
