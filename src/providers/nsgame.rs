//! NSGame's visitor-cookie session, SHA-256 proof of work, and game details.
use std::time::Duration;

use async_trait::async_trait;
use futures::{StreamExt, stream};
use serde::Deserialize;
use sha2::Digest;
use tokio::sync::{Mutex, OnceCell};
use url::Url;

use super::{UA, body, bounded, check, protocol, send, session};
use crate::{
    core::{CloudType, ProviderError, SearchResult, extract_links},
    http::{
        ClientOptions, Session,
        retry::{RetryDecision, RetryPolicy, retry},
    },
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_NONCES: u32 = 10_000_000;

pub struct Nsgame {
    base: Url,
    options: ClientOptions,
    session: OnceCell<Session>,
    flight: Mutex<()>,
    visitor: String,
}
impl Default for Nsgame {
    fn default() -> Self {
        Self::new("https://nsthwj.cn/thwj/".parse().unwrap())
    }
}
impl Nsgame {
    pub fn new(base: Url) -> Self {
        Self::with_options(base, ClientOptions::default())
    }
    pub fn with_options(mut base: Url, options: ClientOptions) -> Self {
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let mut bytes: [u8; 16] = rand::random();
        bytes[6] = (bytes[6] & 15) | 64;
        bytes[8] = (bytes[8] & 63) | 128;
        let hex = bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let visitor = format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        );
        Self {
            base,
            options,
            session: OnceCell::new(),
            flight: Mutex::new(()),
            visitor,
        }
    }
    async fn client(&self) -> Result<&reqwest::Client, ProviderError> {
        Ok(self
            .session
            .get_or_try_init(|| async { session(&self.options) })
            .await?
            .client())
    }
    fn headers(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let origin = self.base.origin().ascii_serialization();
        request
            .header("User-Agent", UA)
            .header("Accept", "application/json, text/plain, */*")
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
            .header("Origin", &origin)
            .header("X-Visitor-Id", &self.visitor)
            .header("X-Entry-Referrer", format!("{origin}/"))
            .header("X-Client-Timezone", "Asia/Shanghai")
            .header("X-Client-Viewport", "1920x1080")
            .header("X-Client-Screen", "1920x1080@1")
    }
    async fn post(
        &self,
        ctx: &SearchContext,
        path: &str,
        payload: Option<serde_json::Value>,
    ) -> Result<String, ProviderError> {
        let client = self.client().await?;
        let mut request = self.headers(client.post(self.base.join(path).unwrap()));
        if let Some(payload) = payload {
            request = request.json(&payload);
        }
        let response = send(ctx, request, TIMEOUT).await?;
        // Deliberately reject status before inspecting an arbitrary error body.
        if response.status() != reqwest::StatusCode::OK {
            return Err(protocol(format!(
                "nsgame session HTTP {}",
                response.status().as_u16()
            )));
        }
        body(response, 16 << 20).await
    }
    async fn ensure_session(&self, ctx: &SearchContext) -> Result<(), ProviderError> {
        let _guard = self.flight.lock().await;
        check(ctx)?;
        let status: Status = decode(
            &self.post(ctx, "traffic/session/status", None).await?,
            "session status",
        )?;
        if !status.success
            || status
                .code
                .as_ref()
                .is_some_and(|code| code != "200" && !code.is_empty())
        {
            return Err(protocol("nsgame session status rejected"));
        }
        if status.data {
            return Ok(());
        }
        let challenge: ChallengeResponse = decode(
            &self.post(ctx, "traffic/session/challenge", None).await?,
            "challenge",
        )?;
        if !challenge.success || challenge.data.challenge.is_empty() {
            return Err(protocol("nsgame challenge rejected"));
        }
        let nonce = solve_challenge(
            ctx,
            &challenge.data.challenge,
            challenge.data.difficulty_bits,
        )
        .await?;
        let issue: Issue = decode(&self.post(ctx, "traffic/session/issue", Some(serde_json::json!({"challenge":challenge.data.challenge,"nonce":nonce.to_string()}))).await?, "session issue")?;
        if !issue.success
            || issue
                .code
                .as_ref()
                .is_some_and(|code| code != "200" && !code.is_empty())
        {
            return Err(protocol("nsgame session issue rejected"));
        }
        Ok(())
    }
    async fn get(&self, ctx: &SearchContext, url: Url) -> Result<String, ProviderError> {
        let client = self.client().await?;
        let policy = RetryPolicy::exponential(3, Duration::from_millis(200), 2.0);
        let response = retry(&policy, |_| {
            let request = self.headers(client.get(url.clone()));
            async move {
                let response = send(ctx, request, TIMEOUT)
                    .await
                    .map_err(RetryDecision::Retry)?;
                if response.status() != reqwest::StatusCode::OK {
                    return Err(RetryDecision::Retry(protocol(format!(
                        "nsgame HTTP {}",
                        response.status().as_u16()
                    ))));
                }
                Ok(response)
            }
        })
        .await
        .map_err(|error| error.into_inner())?;
        body(response, 16 << 20).await
    }
    async fn detail(&self, ctx: &SearchContext, item: Item) -> Result<SearchResult, ProviderError> {
        let mut url = self.base.join("game/detail").unwrap();
        url.query_pairs_mut()
            .append_pair("id", &item.id.to_string());
        let payload: Detail = decode(&self.get(ctx, url).await?, "detail")?;
        if !payload.success
            || payload.data.id != item.id
            || payload
                .code
                .as_ref()
                .is_some_and(|code| code != "200" && !code.is_empty())
        {
            return Err(protocol("nsgame invalid detail response"));
        }
        let detail = payload.data;
        let name = if detail.name.is_empty() {
            item.name
        } else {
            detail.name
        };
        if name.trim().is_empty() {
            return Err(protocol("nsgame detail has no game name"));
        }
        let version = if detail.password.is_empty() {
            item.password
        } else {
            detail.password
        };
        let mut links = extract_links(&detail.url);
        links.retain(|link| {
            matches!(
                link.cloud_type,
                CloudType::Quark | CloudType::Uc | CloudType::Baidu
            )
        });
        if links.is_empty() {
            return Err(protocol("nsgame detail has no supported shares"));
        }
        for link in &mut links {
            link.work_title = Some(name.clone());
            if link.cloud_type == CloudType::Baidu
                && let Ok(mut url) = Url::parse(&link.url)
            {
                url.set_query(None);
                link.url = url.to_string();
            }
        }
        let identity = format!("{:x}", md5::compute(name.as_bytes()));
        let mut row = SearchResult::provider(format!("nsgame-{}", &identity[..21]), "nsgame");
        row.title = if version.is_empty() {
            name
        } else {
            format!(
                "{name}（{}）",
                version.split_whitespace().collect::<Vec<_>>().join(" ")
            )
        };
        row.content = version;
        row.tags = vec!["NS游戏".into(), "Switch".into()];
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
        self.ensure_session(ctx).await?;
        let mut url = self.base.join("game/query").unwrap();
        url.query_pairs_mut()
            .append_pair("pageNum", "1")
            .append_pair("pageSize", "50")
            .append_pair("type", "")
            .append_pair("queryName", query);
        let payload: Search = decode(&self.get(ctx, url).await?, "search")?;
        if !payload.success
            || payload
                .code
                .as_ref()
                .is_some_and(|code| code != "200" && !code.is_empty())
        {
            return Err(protocol("nsgame search API rejected"));
        }
        let mut seen = std::collections::HashSet::new();
        let items = payload
            .data
            .page_data
            .data
            .into_iter()
            .filter(|item| item.id > 0 && seen.insert(item.id))
            .take(50)
            .collect::<Vec<_>>();
        let count = items.len();
        let mut jobs = stream::iter(
            items
                .into_iter()
                .enumerate()
                .map(|(index, item)| async move { (index, self.detail(ctx, item).await) }),
        )
        .buffer_unordered(8);
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
                    "nsgame all {count} details failed: {error}"
                )));
            }
            return Err(protocol(format!(
                "nsgame incomplete details ({}/{count} completed): {error}",
                rows.len()
            )));
        }
        rows.sort_by_key(|(index, _)| *index);
        Ok(rows.into_iter().map(|(_, row)| row).collect())
    }
}
fn decode<T: serde::de::DeserializeOwned>(source: &str, phase: &str) -> Result<T, ProviderError> {
    serde_json::from_str(source)
        .map_err(|_| ProviderError::Parse(format!("nsgame malformed {phase} response")))
}
#[derive(Deserialize)]
struct Status {
    success: bool,
    data: bool,
    code: Option<String>,
}
#[derive(Deserialize)]
struct Issue {
    success: bool,
    code: Option<String>,
}
#[derive(Deserialize)]
struct ChallengeResponse {
    success: bool,
    data: Challenge,
}
#[derive(Deserialize)]
struct Challenge {
    challenge: String,
    #[serde(rename = "difficultyBits")]
    difficulty_bits: i32,
}
#[derive(Deserialize)]
struct Search {
    success: bool,
    code: Option<String>,
    data: SearchData,
}
#[derive(Deserialize)]
struct SearchData {
    #[serde(rename = "pageData")]
    page_data: PageData,
}
#[derive(Deserialize)]
struct PageData {
    data: Vec<Item>,
}
#[derive(Deserialize)]
struct Detail {
    success: bool,
    code: Option<String>,
    data: Item,
}
#[derive(Deserialize)]
struct Item {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    password: String,
}
fn leading_zero_bits(digest: &[u8; 32], bits: u32) -> bool {
    let full = (bits / 8) as usize;
    let remainder = bits % 8;
    digest[..full].iter().all(|byte| *byte == 0)
        && (remainder == 0 || digest[full] >> (8 - remainder) == 0)
}
async fn solve_challenge(
    ctx: &SearchContext,
    challenge: &str,
    bits: i32,
) -> Result<u32, ProviderError> {
    if !(0..=256).contains(&bits) {
        return Err(protocol(
            "nsgame invalid challenge difficulty (expected 0..256)",
        ));
    }
    // The source deadline bounds elapsed CPU time; the nonce ceiling bounds
    // total work even if the caller configured an unusually long deadline.
    let cpu_deadline = tokio::time::Instant::now() + ctx.remaining().min(TIMEOUT);
    let mut prefix = super::hash_prefix(ctx, challenge.as_bytes(), cpu_deadline).await?;
    prefix.update(b":");
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
        let digest: [u8; 32] = hasher.finalize().into();
        if leading_zero_bits(&digest, bits as u32) {
            return Ok(nonce);
        }
    }
    Err(protocol("nsgame challenge exhausted 10000000 attempts"))
}
#[async_trait]
impl Provider for Nsgame {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("nsgame", 2, KeywordFilterMode::Core)
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
    use std::{
        io::{Read, Write},
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, header, method, path, query_param},
    };
    async fn active(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/thwj/traffic/session/status"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"success":true,"data":true,"code":"200"})),
            )
            .mount(server)
            .await;
    }
    #[tokio::test]
    async fn visitor_pow_issue_cookie_search_and_detail_full_chain() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/thwj/traffic/session/status"))
            .and(header("X-Client-Timezone", "Asia/Shanghai"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"success":true,"data":false})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST")).and(path("/thwj/traffic/session/challenge"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"success":true,"data":{"challenge":"fixture","difficultyBits":0}}))).expect(1).mount(&server).await;
        Mock::given(method("POST"))
            .and(path("/thwj/traffic/session/issue"))
            .and(body_json(
                serde_json::json!({"challenge":"fixture","nonce":"0"}),
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Set-Cookie", "access=granted; Path=/")
                    .set_body_json(serde_json::json!({"success":true,"code":"200"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET")).and(path("/thwj/game/query")).and(header("cookie", "access=granted")).and(query_param("pageSize", "50")).and(query_param("queryName", "Game"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"success":true,"data":{"pageData":{"data":[{"id":1,"name":"Game"}]}}}))).expect(1).mount(&server).await;
        Mock::given(method("GET")).and(path("/thwj/game/detail")).and(query_param("id", "1")).and(header("cookie", "access=granted"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"success":true,"data":{"id":1,"name":"Game","password":"v1\nDLC","url":"[夸克网盘]：https://pan.quark.cn/s/abcdef\n[UC网盘]：https://drive.uc.cn/s/abcdef\nhttps://pan.baidu.com/s/abcdef?pwd=a1b2"}}))).expect(1).mount(&server).await;
        let provider = Nsgame::new(format!("{}/thwj/", server.uri()).parse().unwrap());
        let ctx = super::super::context();
        let rows = provider.search(&ctx, "Game").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Game（v1 DLC）");
        assert_eq!(rows[0].links.len(), 3);
        let baidu = rows[0]
            .links
            .iter()
            .find(|link| link.cloud_type == CloudType::Baidu)
            .unwrap();
        assert_eq!(baidu.password.as_deref(), Some("a1b2"));
        assert!(!baidu.url.contains('?'));
        assert_eq!(rows[0].links[0].work_title.as_deref(), Some("Game"));
        assert!(ctx.has_results());
        assert_eq!(provider.visitor.len(), 36);
    }
    #[tokio::test]
    async fn empty_malformed_non200_and_schema_failures() {
        let server = MockServer::start().await;
        for (response, message) in [
            (
                ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"success":true,"data":{"pageData":{"data":[]}}}),
                ),
                None,
            ),
            (
                ResponseTemplate::new(200).set_body_string("not json"),
                Some("malformed search"),
            ),
            (
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"success":true,"data":{}})),
                Some("malformed search"),
            ),
            (
                ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"success":false,"data":{"pageData":{"data":[]}}}),
                ),
                Some("API rejected"),
            ),
            (
                ResponseTemplate::new(503).set_body_string("irrelevant error body"),
                Some("503"),
            ),
        ] {
            server.reset().await;
            active(&server).await;
            Mock::given(path("/thwj/game/query"))
                .respond_with(response)
                .mount(&server)
                .await;
            let result = Nsgame::new(format!("{}/thwj/", server.uri()).parse().unwrap())
                .search(&super::super::context(), "Game")
                .await;
            if let Some(message) = message {
                assert!(result.unwrap_err().to_string().contains(message));
            } else {
                assert!(result.unwrap().is_empty());
            }
        }
        server.reset().await;
        Mock::given(path("/thwj/traffic/session/status"))
            .respond_with(ResponseTemplate::new(403).set_body_string("not JSON"))
            .expect(1)
            .mount(&server)
            .await;
        assert!(
            Nsgame::new(format!("{}/thwj/", server.uri()).parse().unwrap())
                .search(&super::super::context(), "Game")
                .await
                .unwrap_err()
                .to_string()
                .contains("session HTTP 403")
        );
    }
    #[tokio::test]
    async fn retry_counts_partial_details_and_cancel_backoff() {
        let server = MockServer::start().await;
        active(&server).await;
        Mock::given(path("/thwj/game/query")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"success":true,"data":{"pageData":{"data":[{"id":1,"name":"Game"},{"id":2,"name":"Game 2"}]}}}))).mount(&server).await;
        let attempts = Arc::new(AtomicUsize::new(0));
        let counter = attempts.clone();
        Mock::given(path("/thwj/game/detail")).and(query_param("id", "1")).respond_with(move |_: &wiremock::Request| {
            if counter.fetch_add(1, Ordering::SeqCst) < 2 { ResponseTemplate::new(500) }
            else { ResponseTemplate::new(200).set_body_json(serde_json::json!({"success":true,"data":{"id":1,"name":"Game","url":"https://pan.quark.cn/s/abcdef"}})) }
        }).expect(3).mount(&server).await;
        Mock::given(path("/thwj/game/detail"))
            .and(query_param("id", "2"))
            .respond_with(ResponseTemplate::new(503))
            .expect(3)
            .mount(&server)
            .await;
        let provider = Arc::new(Nsgame::new(
            format!("{}/thwj/", server.uri()).parse().unwrap(),
        ));
        assert_eq!(
            super::super::assert_failed_partial(provider.clone(), "Game")
                .await
                .len(),
            1
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        server.reset().await;
        active(&server).await;
        Mock::given(path("/thwj/game/query"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = super::super::context()
            .for_source(tokio::time::Instant::now() + Duration::from_millis(80));
        assert_eq!(
            provider.search(&ctx, "Game").await.unwrap_err(),
            ProviderError::Timeout
        );
    }
    #[tokio::test]
    async fn completed_detail_survives_source_deadline() {
        let server = MockServer::start().await;
        active(&server).await;
        Mock::given(path("/thwj/game/query")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"success":true,"data":{"pageData":{"data":[{"id":1,"name":"Game"},{"id":2,"name":"Game 2"}]}}}))).mount(&server).await;
        Mock::given(path("/thwj/game/detail")).and(query_param("id", "1")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"success":true,"data":{"id":1,"name":"Game","url":"https://pan.quark.cn/s/abcdef"}}))).mount(&server).await;
        Mock::given(path("/thwj/game/detail"))
            .and(query_param("id", "2"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(1)))
            .mount(&server)
            .await;
        let ctx = super::super::context()
            .for_source(tokio::time::Instant::now() + Duration::from_millis(150));
        let provider = Nsgame::new(format!("{}/thwj/", server.uri()).parse().unwrap());
        assert_eq!(
            provider.search(&ctx, "Game").await.unwrap_err(),
            ProviderError::Timeout
        );
        assert!(ctx.has_results());
    }
    #[tokio::test]
    async fn difficulty_endpoints_and_cpu_cancellation() {
        let ctx = super::super::context();
        for invalid in [-8, -1, 257, 260, 263, 264, 512, 4096] {
            assert!(solve_challenge(&ctx, "fixture", invalid).await.is_err());
        }
        assert_eq!(solve_challenge(&ctx, "fixture", 0).await.unwrap(), 0);
        assert!(leading_zero_bits(&[0; 32], 256));
        let mut digest = [0; 32];
        digest[31] = 1;
        assert!(leading_zero_bits(&digest, 255));
        assert!(!leading_zero_bits(&digest, 256));
        let ctx = super::super::context()
            .for_source(tokio::time::Instant::now() + Duration::from_millis(2));
        assert_eq!(
            solve_challenge(&ctx, "fixture", 256).await.unwrap_err(),
            ProviderError::Timeout
        );
        assert_eq!(MAX_NONCES, 10_000_000);
    }
    #[tokio::test]
    async fn truncated_session_body_is_not_a_success() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"success\":true")
                .unwrap();
        });
        let provider = Nsgame::new(format!("http://{address}/thwj/").parse().unwrap());
        assert!(
            provider
                .post(&super::super::context(), "traffic/session/status", None)
                .await
                .is_err()
        );
        server.join().unwrap();
    }
}
