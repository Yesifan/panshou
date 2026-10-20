//! Miosou Anubis HTTP PoW session, SSE snapshots, and non-shallow share detail lookup.
use super::support::*;
use crate::{
    core::{Link, ProviderError, SearchResult},
    http::{ClientOptions, HttpClientFactory, Session},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use futures::{StreamExt, stream::FuturesUnordered};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio::sync::{Mutex, OnceCell};
use url::Url;
#[derive(Clone)]
pub struct Miosou {
    base: Url,
    options: ClientOptions,
    session: Arc<OnceCell<Session>>,
    gate: Arc<Mutex<bool>>,
}
impl std::fmt::Debug for Miosou {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Miosou").finish_non_exhaustive()
    }
}
impl Default for Miosou {
    fn default() -> Self {
        Self::with_options(
            Url::parse("https://miosou.cc/").unwrap(),
            ClientOptions::default(),
        )
    }
}
#[derive(Deserialize)]
struct Challenge {
    rules: Rules,
    challenge: ChallengeData,
}
#[derive(Deserialize)]
struct Rules {
    algorithm: String,
    difficulty: u32,
}
#[derive(Deserialize)]
struct ChallengeData {
    id: String,
    #[serde(rename = "randomData")]
    random_data: String,
}
#[derive(Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
struct Item {
    url: String,
    share_ref: String,
    password: String,
    note: String,
    datetime: String,
    source: String,
    keyword: String,
    stream_key: String,
}
#[derive(Deserialize)]
struct Snapshot {
    merged_by_type: BTreeMap<String, Vec<Item>>,
}
impl Miosou {
    pub fn new(base: Url) -> Self {
        Self::with_options(base, ClientOptions::default())
    }
    pub fn with_options(base: Url, options: ClientOptions) -> Self {
        Self {
            base,
            options,
            session: Arc::new(OnceCell::new()),
            gate: Arc::new(Mutex::new(false)),
        }
    }
    pub fn with_session(base: Url, session: Session) -> Self {
        let cell = OnceCell::new();
        let _ = cell.set(session);
        Self {
            base,
            options: ClientOptions::default(),
            session: Arc::new(cell),
            gate: Arc::new(Mutex::new(false)),
        }
    }
    async fn session(&self) -> Result<&Session, ProviderError> {
        self.session
            .get_or_try_init(|| async {
                HttpClientFactory::new()
                    .session(&self.options)
                    .map_err(|e| protocol(e.to_string()))
            })
            .await
    }
    async fn fetch(
        &self,
        ctx: &SearchContext,
        session: &Session,
        url: Url,
        seconds: u64,
        accept: &str,
        limit: usize,
    ) -> Result<(Vec<u8>, bool, u16), ProviderError> {
        let response = crate::http::execute(
            browser(session.client().get(url), self.base.as_str(), accept)
                .header("origin", self.base.origin().ascii_serialization())
                .header("cache-control", "no-cache")
                .timeout(ctx.request_timeout(Duration::from_secs(seconds))),
        )
        .await
        .map_err(network)?;
        let code = response.status().as_u16();
        let gate = code == 401 || response.url().path().starts_with("/.within.website/");
        let body = crate::http::read_response(response, limit)
            .await
            .map_err(network)?;
        Ok((body, gate, code))
    }
    async fn ensure_gate(
        &self,
        ctx: &SearchContext,
        session: &Session,
    ) -> Result<(), ProviderError> {
        static CHALLENGE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r#"(?s)<script\s+id=["']anubis_challenge["']\s+type=["']application/json["']\s*>\s*(.*?)\s*</script>"#).unwrap()
        });
        let mut ready = self.gate.lock().await;
        if *ready {
            return Ok(());
        }
        let mut last = protocol("miosou Anubis gate not ready");
        for _ in 0..3 {
            let attempt = async {
                let (body, is_gate, status) = self
                    .fetch(ctx, session, self.base.clone(), 30, HTML, 1 << 20)
                    .await?;
                let body = String::from_utf8_lossy(&body);
                let Some(capture) = CHALLENGE.captures(&body) else {
                    if status == 200 && !is_gate {
                        return Ok(());
                    }
                    return Err(protocol(format!("miosou gate HTTP {status}")));
                };
                let document = scraper::Html::parse_fragment(&capture[1])
                    .root_element()
                    .text()
                    .collect::<String>();
                let challenge: Challenge = serde_json::from_str(&document).map_err(parse)?;
                if challenge.challenge.id.is_empty()
                    || challenge.challenge.random_data.is_empty()
                    || !(1..=7).contains(&challenge.rules.difficulty)
                    || !["fast", "slow"].contains(&challenge.rules.algorithm.as_str())
                {
                    return Err(protocol("miosou invalid/unsupported Anubis challenge"));
                }
                let started = std::time::Instant::now();
                let (hash, nonce) = solve(
                    ctx,
                    &challenge.challenge.random_data,
                    challenge.rules.difficulty,
                )
                .await?;
                let mut pass = self
                    .base
                    .join(".within.website/x/cmd/anubis/api/pass-challenge")
                    .map_err(|_| protocol("miosou invalid base"))?;
                pass.query_pairs_mut()
                    .append_pair("id", &challenge.challenge.id)
                    .append_pair("response", &hash)
                    .append_pair("nonce", &nonce.to_string())
                    .append_pair("redir", self.base.as_str())
                    .append_pair(
                        "elapsedTime",
                        &started.elapsed().as_millis().max(1).to_string(),
                    );
                let (_, gate, status) = self.fetch(ctx, session, pass, 30, HTML, 1 << 20).await?;
                if status != 200 || gate {
                    return Err(protocol(format!(
                        "miosou Anubis confirmation HTTP {status}"
                    )));
                }
                Ok(())
            }
            .await;
            match attempt {
                Ok(()) => {
                    *ready = true;
                    return Ok(());
                }
                Err(e) => last = e,
            }
            if ctx.is_cancelled() {
                return Err(ProviderError::Timeout);
            }
        }
        Err(last)
    }
    async fn convert(
        &self,
        ctx: &SearchContext,
        session: &Session,
        mut item: Item,
        query: &str,
    ) -> Result<Option<SearchResult>, ProviderError> {
        if item.url.trim().is_empty() && !item.share_ref.is_empty() {
            let mut url = self
                .base
                .join("miov1/share/detail")
                .map_err(|_| protocol("miosou invalid base"))?;
            url.query_pairs_mut()
                .append_pair("share_ref", &item.share_ref);
            let (body, gate, code) = self
                .fetch(ctx, session, url, 20, "application/json", 2 << 20)
                .await?;
            if gate || code != 200 {
                return Err(protocol(format!("miosou detail HTTP {code}")));
            }
            let value: Value = serde_json::from_slice(&body).map_err(parse)?;
            item.url = value["display_url"].as_str().unwrap_or("").trim().into();
            if item.note.trim().is_empty() {
                item.note = value
                    .pointer("/share/title")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into();
            }
        }
        let raw = item.url.trim();
        let url = Url::parse(raw).map_err(|_| protocol("miosou item omitted valid URL"))?;
        if !["http", "https", "magnet", "ed2k"].contains(&url.scheme())
            || (["http", "https"].contains(&url.scheme()) && url.host_str().is_none())
        {
            return Err(protocol("miosou item has unsupported URL"));
        }
        let mut link = Link::new(raw);
        link.password = (!item.password.trim().is_empty())
            .then(|| item.password.trim().into())
            .or_else(|| query_password(raw))
            .or_else(|| password(&item.note));
        let hash = Sha256::digest(format!("{raw}\0{}", link.password.as_deref().unwrap_or("")));
        let id = hash[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let mut row = SearchResult::provider(format!("miosou-{id}"), "miosou");
        row.title = if item.note.trim().is_empty() {
            raw.into()
        } else {
            item.note.trim().into()
        };
        row.datetime = date(&item.datetime);
        let mut parts = Vec::new();
        if !item.source.trim().is_empty() {
            parts.push(format!("来源: {}", item.source.trim()));
        }
        if !item.share_ref.is_empty() {
            parts.push(format!("分享索引: {}", item.share_ref));
        }
        if !item.keyword.trim().is_empty() {
            parts.push(format!("关键词: {}", item.keyword.trim()));
        }
        row.content = parts.join(" | ");
        row.links.push(link);
        // Publication happens only after the caller checks the snapshot version.
        Ok(matches(&row, query).then_some(row))
    }
    async fn update_snapshot(
        &self,
        ctx: &SearchContext,
        session: &Session,
        query: &str,
        state: &SseState,
        progress: &mut Progress,
    ) {
        let mut candidates: Vec<_> = state
            .groups
            .iter()
            .flat_map(|(cloud, items)| {
                items
                    .iter()
                    .map(move |item| (format!("{cloud}\0{}", item_key(item)), item.clone()))
            })
            .collect();
        // Match upstream's direct-link preference and bounded detail fan-out.
        candidates.sort_by_key(|(_, item)| item.url.trim().is_empty());
        for (key, item) in candidates.into_iter().take(80) {
            if progress.seen.get(&key) == Some(&item)
                || (!progress.seen.contains_key(&key) && progress.seen.len() >= 80)
            {
                continue;
            }
            progress.seen.insert(key.clone(), item.clone());
            progress
                .pending
                .retain(|(pending_key, _)| pending_key != &key);
            if item.url.trim().is_empty() {
                progress.pending.push_back((key, item));
            } else {
                match self.convert(ctx, session, item, query).await {
                    Ok(Some(row)) => {
                        if let Some(row) = publish(ctx, row, query) {
                            progress.rows.insert(key, row);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        progress.error.get_or_insert(error);
                    }
                }
            }
        }
    }
    async fn resolve_ref(
        &self,
        ctx: &SearchContext,
        session: &Session,
        query: &str,
        key: String,
        item: Item,
    ) -> DetailOutcome {
        let result = self.convert(ctx, session, item.clone(), query).await;
        (key, item, result)
    }
    async fn collect_stream(
        &self,
        ctx: &SearchContext,
        session: &Session,
        response: reqwest::Response,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        // Shared decoding owns the response and its outbound permit, bounding
        // wire bytes and each compression layer until EOF/error/drop.
        let mut chunks = Some(crate::http::stream_response(response, LIMIT).map_err(stream_error)?);
        let mut state = SseState::default();
        let mut progress = Progress::default();
        let mut jobs = FuturesUnordered::new();
        let mut line = Vec::new();
        let mut decoded = 0usize;
        while chunks.is_some() || !jobs.is_empty() || !progress.pending.is_empty() {
            if ctx.is_cancelled() {
                return Err(ProviderError::Timeout);
            }
            while jobs.len() < 4 {
                let Some((key, item)) = progress.pending.pop_front() else {
                    break;
                };
                jobs.push(self.resolve_ref(ctx, session, query, key, item));
            }
            tokio::select! {
                Some((key, item, result)) = jobs.next(), if !jobs.is_empty() => {
                    // Ignore an older lookup after a newer snapshot replaced it.
                    if progress.seen.get(&key) == Some(&item) {
                        match result {
                            Ok(Some(row)) => {
                                if let Some(row) = publish(ctx, row, query) {
                                    progress.rows.insert(key, row);
                                }
                            }
                            Ok(None) => {}
                            Err(error) => { progress.error.get_or_insert(error); }
                        }
                    }
                }
                chunk = async { chunks.as_mut().expect("guarded stream").next().await }, if chunks.is_some() => {
                    match chunk.transpose().map_err(stream_error)? {
                        Some(chunk) => {
                            let allowed = chunk.len().min(LIMIT - decoded);
                            decoded += allowed;
                            // Process the accepted prefix first, preserving earlier complete
                            // snapshots even when this same chunk exceeds a later limit.
                            for part in chunk[..allowed].split_inclusive(|byte| *byte == b'\n') {
                                let complete = part.last() == Some(&b'\n');
                                let bytes = if complete { &part[..part.len()-1] } else { part };
                                if line.len() + bytes.len() > SSE_LINE_LIMIT {
                                    return Err(protocol("miosou SSE line exceeds 2MiB"));
                                }
                                line.extend_from_slice(bytes);
                                if complete {
                                    let snapshot = state.line(&line)?;
                                    line.clear();
                                    if snapshot {
                                        self.update_snapshot(ctx, session, query, &state, &mut progress).await;
                                    }
                                }
                            }
                            if allowed != chunk.len() {
                                return Err(protocol("miosou SSE body exceeds 16MiB"));
                            }
                        }
                        None => {
                            // Releasing the SSE permit here also allows queued details to
                            // finish when the shared outbound limit is configured to one.
                            chunks = None;
                            if !line.is_empty() && state.line(&line)? {
                                self.update_snapshot(ctx, session, query, &state, &mut progress).await;
                            }
                            line.clear();
                            if state.flush()? {
                                self.update_snapshot(ctx, session, query, &state, &mut progress).await;
                            }
                        }
                    }
                }
            }
        }
        if let Some(error) = state.error.or(progress.error) {
            return Err(error);
        }
        if !state.recognized {
            return Err(protocol("miosou SSE omitted snapshots"));
        }
        Ok(progress.rows.into_values().collect())
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let session = self.session().await?;
        self.ensure_gate(ctx, session).await?;
        for _ in 0..2 {
            let mut url = self
                .base
                .join("miov1/search")
                .map_err(|_| protocol("miosou invalid base"))?;
            url.query_pairs_mut().append_pair("keyword", query.trim());
            // Shared streaming decoding retains the outbound permit and applies
            // wire/intermediate bounds independently of decoded SSE limits.
            let response = crate::http::execute(
                browser(
                    session.client().get(url),
                    self.base.as_str(),
                    "text/event-stream",
                )
                .header("origin", self.base.origin().ascii_serialization())
                .header("cache-control", "no-cache")
                .header("accept-encoding", "identity, gzip, deflate, br")
                .timeout(ctx.request_timeout(Duration::from_secs(35))),
            )
            .await
            .map_err(network)?;
            let code = response.status().as_u16();
            if code == 401 || response.url().path().starts_with("/.within.website/") {
                drop(response);
                *self.gate.lock().await = false;
                self.ensure_gate(ctx, session).await?;
                continue;
            }
            if code != 200 {
                return Err(protocol(format!("miosou search HTTP {code}")));
            }
            return self.collect_stream(ctx, session, response, query).await;
        }
        Err(protocol("miosou Anubis session expired after retry"))
    }
}
async fn solve(
    ctx: &SearchContext,
    data: &str,
    difficulty: u32,
) -> Result<(String, u64), ProviderError> {
    // Hash large challenge prefixes once, with a cancellation point after at
    // most 32KiB of input. Rehashing the prefix for every nonce can otherwise
    // monopolize the executor for GiB of work between cancellation checks.
    let mut prefix = Sha256::new();
    for chunk in data.as_bytes().chunks(32 * 1024) {
        if ctx.is_cancelled() {
            return Err(ProviderError::Timeout);
        }
        prefix.update(chunk);
        tokio::task::yield_now().await;
    }
    for nonce in 0..u64::MAX {
        // A nonce adds at most two SHA-256 blocks after the cached prefix, so
        // 128 nonces stay comfortably below another 32KiB of hashing work.
        if nonce.is_multiple_of(128) {
            tokio::task::yield_now().await;
            if ctx.is_cancelled() {
                return Err(ProviderError::Timeout);
            }
        }
        let mut candidate = prefix.clone();
        candidate.update(nonce.to_string().as_bytes());
        let hash = candidate.finalize();
        let full = (difficulty / 2) as usize;
        if hash[..full].iter().all(|v| *v == 0)
            && (difficulty.is_multiple_of(2) || hash[full] >> 4 == 0)
        {
            return Ok((hash.iter().map(|b| format!("{b:02x}")).collect(), nonce));
        }
    }
    Err(protocol("miosou PoW nonce space exhausted"))
}
fn stream_error(error: anyhow::Error) -> ProviderError {
    if error
        .downcast_ref::<crate::http::ResponseTooLarge>()
        .is_some_and(|error| error.limit == LIMIT)
    {
        protocol("miosou SSE body exceeds 16MiB")
    } else {
        network(error)
    }
}
const SSE_LINE_LIMIT: usize = 2 * 1024 * 1024;
type SnapshotGroups = BTreeMap<String, Vec<Item>>;
type DetailOutcome = (String, Item, Result<Option<SearchResult>, ProviderError>);
#[derive(Default)]
struct Progress {
    seen: BTreeMap<String, Item>,
    rows: BTreeMap<String, SearchResult>,
    pending: VecDeque<(String, Item)>,
    error: Option<ProviderError>,
}
#[derive(Default)]
struct SseState {
    groups: SnapshotGroups,
    event: String,
    data: String,
    error: Option<ProviderError>,
    recognized: bool,
}
impl SseState {
    fn line(&mut self, bytes: &[u8]) -> Result<bool, ProviderError> {
        let line = std::str::from_utf8(bytes)
            .map_err(|_| protocol("miosou SSE is not UTF-8"))?
            .trim_end_matches('\r');
        if line.is_empty() {
            return self.flush();
        }
        if let Some(value) = line.strip_prefix("event:") {
            self.event = value.trim().into();
        } else if let Some(value) = line.strip_prefix("data:") {
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            self.data.push_str(value.trim());
        }
        Ok(false)
    }
    fn flush(&mut self) -> Result<bool, ProviderError> {
        if self.data.is_empty() {
            self.event.clear();
            return Ok(false);
        }
        let snapshot = matches!(self.event.as_str(), "snapshot" | "");
        match self.event.as_str() {
            "snapshot" | "" => {
                let snapshot: Snapshot = serde_json::from_str(&self.data).map_err(parse)?;
                self.recognized = true;
                for (cloud, items) in snapshot.merged_by_type {
                    let existing = self.groups.entry(cloud).or_default();
                    for mut item in items {
                        if item_key(&item).is_empty() {
                            continue;
                        }
                        if let Some(previous) =
                            existing.iter_mut().find(|v| item_key(v) == item_key(&item))
                        {
                            if item.url.is_empty() {
                                item.url = previous.url.clone();
                            }
                            if item.share_ref.is_empty() {
                                item.share_ref = previous.share_ref.clone();
                            }
                            if item.password.is_empty() {
                                item.password = previous.password.clone();
                            }
                            if item.note.is_empty() {
                                item.note = previous.note.clone();
                            }
                            if item.datetime.is_empty() {
                                item.datetime = previous.datetime.clone();
                            }
                            if item.source.is_empty() {
                                item.source = previous.source.clone();
                            }
                            if item.keyword.is_empty() {
                                item.keyword = previous.keyword.clone();
                            }
                            *previous = item;
                        } else {
                            existing.push(item);
                        }
                    }
                }
            }
            "error" => {
                self.recognized = true;
                self.error = Some(protocol("miosou search stream reported an upstream error"));
            }
            _ => {}
        }
        self.event.clear();
        self.data.clear();
        Ok(snapshot)
    }
}
fn item_key(item: &Item) -> String {
    if !item.stream_key.is_empty() {
        format!("stream:{}", item.stream_key)
    } else if !item.share_ref.is_empty() {
        format!("ref:{}", item.share_ref)
    } else if !item.url.is_empty() {
        format!("url:{}", item.url)
    } else {
        String::new()
    }
}
#[cfg(test)]
mod compression_and_pow_tests {
    use super::*;
    use std::io::Write;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

    const SNAPSHOT: &str = "event: snapshot\ndata: {\"merged_by_type\":{\"quark\":[{\"url\":\"https://pan.quark.cn/s/compressed\",\"note\":\"film 压缩\"}]}}\n\n";

    // Capture a sync-flushed prefix separately from its final checksum/trailer.
    // This makes publication before compressed-stream EOF observable over HTTP.
    fn fixture(encoding: &str) -> (Vec<u8>, Vec<u8>) {
        match encoding {
            "gzip" => {
                let mut encoder =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(SNAPSHOT.as_bytes()).unwrap();
                encoder.flush().unwrap();
                let prefix = encoder.get_ref().clone();
                (prefix, encoder.finish().unwrap())
            }
            "deflate" => {
                let mut encoder =
                    flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(SNAPSHOT.as_bytes()).unwrap();
                encoder.flush().unwrap();
                let prefix = encoder.get_ref().clone();
                (prefix, encoder.finish().unwrap())
            }
            "br" => {
                let mut encoder = brotli::CompressorWriter::new(Vec::new(), 4096, 4, 22);
                encoder.write_all(SNAPSHOT.as_bytes()).unwrap();
                encoder.flush().unwrap();
                let prefix = encoder.get_ref().clone();
                (prefix, encoder.into_inner())
            }
            _ => unreachable!(),
        }
    }

    #[tokio::test]
    async fn gzip_zlib_and_brotli_snapshots_decode_at_provider_boundary() {
        for encoding in ["gzip", "deflate", "br"] {
            let server = MockServer::start().await;
            Mock::given(path("/"))
                .respond_with(ResponseTemplate::new(200).set_body_string("ready"))
                .expect(1)
                .mount(&server)
                .await;
            let (_, bytes) = fixture(encoding);
            Mock::given(path("/miov1/search"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("content-encoding", encoding)
                        .set_body_bytes(bytes),
                )
                .expect(1)
                .mount(&server)
                .await;
            let source = Miosou::new(Url::parse(&format!("{}/", server.uri())).unwrap());
            let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(3));
            let rows = source.search(&ctx, "film").await.unwrap();
            assert_eq!(rows.len(), 1, "{encoding}");
            assert_eq!(rows[0].title, "film 压缩");
            assert_eq!(ctx.finish(Vec::new()).len(), 1);
            server.verify().await;
        }
    }

    async fn request_headers(socket: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
            let mut part = [0; 1024];
            let count = socket.read(&mut part).await.unwrap();
            assert!(count > 0 && bytes.len() < 8192);
            bytes.extend_from_slice(&part[..count]);
        }
        String::from_utf8(bytes).unwrap()
    }

    #[tokio::test]
    async fn compressed_snapshots_publish_before_trailer_and_deadline() {
        for encoding in ["gzip", "deflate", "br"] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let source = Miosou::new(
                Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap(),
            );
            let (prefix, complete) = fixture(encoding);
            assert!(!prefix.is_empty());
            let server = async {
                let (mut gate, _) = listener.accept().await.unwrap();
                assert!(
                    request_headers(&mut gate)
                        .await
                        .starts_with("GET / HTTP/1.1")
                );
                gate.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nready",
                )
                .await
                .unwrap();
                drop(gate);
                let (mut socket, _) = listener.accept().await.unwrap();
                let headers = request_headers(&mut socket).await;
                assert!(headers.starts_with("GET /miov1/search?keyword=film "));
                assert!(headers.to_ascii_lowercase().contains("gzip, deflate, br"));
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Encoding: {encoding}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n",prefix.len()).as_bytes()).await.unwrap();
                socket.write_all(&prefix).await.unwrap();
                socket.write_all(b"\r\n").await.unwrap();
                tokio::time::sleep(Duration::from_millis(600)).await;
                let tail = &complete[prefix.len()..];
                if !tail.is_empty() {
                    let _ = socket
                        .write_all(format!("{:x}\r\n", tail.len()).as_bytes())
                        .await;
                    let _ = socket.write_all(tail).await;
                    let _ = socket.write_all(b"\r\n").await;
                }
                let _ = socket.write_all(b"0\r\n\r\n").await;
            };
            let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(3))
                .for_source(tokio::time::Instant::now() + Duration::from_millis(300));
            let work = async {
                assert!(
                    matches!(
                        source.search(&ctx, "film").await,
                        Err(ProviderError::Timeout)
                    ),
                    "{encoding}"
                );
                let rows = ctx.finish(Vec::new());
                assert_eq!(rows.len(), 1, "{encoding}: snapshot lost before trailer");
                assert_eq!(rows[0].title, "film 压缩");
                assert!(!ctx.publish(Vec::new()));
            };
            tokio::time::timeout(Duration::from_secs(5), async {
                tokio::join!(server, work);
            })
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn cached_pow_prefix_preserves_hash_and_first_nonce() {
        let data = "fixture".repeat(11); // Includes both complete and partial SHA blocks.
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(3));
        let expected = (0u64..10000)
            .find(|nonce| Sha256::digest(format!("{data}{nonce}"))[0] == 0)
            .unwrap();
        let (hash, nonce) = solve(&ctx, &data, 2).await.unwrap();
        assert_eq!(nonce, expected);
        let expected_hash: String = Sha256::digest(format!("{data}{nonce}"))
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(hash, expected_hash);
    }

    #[tokio::test(start_paused = true)]
    async fn large_pow_prefix_yields_to_tiny_deadline_during_prehash() {
        let data = "x".repeat(1024 * 1024);
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(3))
            .for_source(tokio::time::Instant::now() + Duration::from_millis(1));
        let started = std::time::Instant::now();
        let expire = async {
            // Let prefix hashing start before advancing the deadline. The old
            // solver would rehash roughly 4GiB before its next scheduling point.
            for _ in 0..3 {
                tokio::task::yield_now().await;
            }
            tokio::time::advance(Duration::from_millis(2)).await;
        };
        let work = async {
            assert!(matches!(
                solve(&ctx, &data, 7).await,
                Err(ProviderError::Timeout)
            ));
        };
        tokio::join!(work, expire);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "PoW monopolized the executor"
        );
    }
}

#[cfg(test)]
#[path = "miosou_stream_tests.rs"]
mod stream_tests;

#[async_trait]
impl Provider for Miosou {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("miosou", 2, KeywordFilterMode::Core)
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
