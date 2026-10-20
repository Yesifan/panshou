use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use async_trait::async_trait;
use futures::{FutureExt, StreamExt, stream::FuturesUnordered};
use regex::Regex;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::LazyLock,
    time::Duration,
};
use url::Url;
const LIMIT: usize = 2 * 1024 * 1024;
const LINE_LIMIT: usize = 256 * 1024;
static SHARE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"https?://[^\s\"'<>]+"#).unwrap());
static PASS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:提取码|密码|访问码|访问密码|password|pwd|code)\s*[:：=]?\s*([a-z0-9]{3,16})",
    )
    .unwrap()
});
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://sopanya.com/").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Sopanya {
    endpoints: Endpoints,
}
impl Sopanya {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
#[derive(Debug, Deserialize)]
struct Event {
    title: String,
    url: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    is_type: i64,
    #[serde(default, rename = "type")]
    kind: String,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Resource {
    title: String,
    url: String,
    code: String,
    content: String,
    description: String,
    vod_content: String,
    update_time: Value,
    create_time: Value,
}
#[derive(Deserialize)]
struct Saved {
    #[serde(default)]
    message: String,
    data: Option<Resource>,
}
#[derive(Deserialize)]
struct Status {
    data: Option<StatusData>,
}
#[derive(Deserialize)]
struct StatusData {
    ready: bool,
    resource: Option<Resource>,
}
fn shares(raw: &str) -> Vec<String> {
    let raw = raw.replace(r"\/", "/");
    let raw = scraper::Html::parse_fragment(&raw)
        .root_element()
        .text()
        .collect::<String>();
    let mut seen = HashSet::new();
    SHARE
        .find_iter(&raw)
        .filter_map(|m| {
            let raw = m.as_str().trim_end_matches([
                '.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '。', '，', '；', '：', '！',
                '？', '）',
            ]);
            let link = Link::new(raw);
            matches!(
                link.cloud_type,
                CloudType::Quark
                    | CloudType::Aliyun
                    | CloudType::Baidu
                    | CloudType::Uc
                    | CloudType::Xunlei
                    | CloudType::Tianyi
                    | CloudType::One15
                    | CloudType::Pan123
                    | CloudType::Pikpak
            )
            .then(|| raw.to_owned())
        })
        .filter(|s| seen.insert(s.clone()))
        .collect()
}
fn source_time(raw: &Value) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Some(n) = raw.as_i64().filter(|n| *n > 0) {
        return chrono::DateTime::from_timestamp(
            if n > 1_000_000_000_000 { n / 1000 } else { n },
            0,
        );
    }
    if let Some(n) = raw.as_f64().filter(|n| *n > 0.0) {
        return chrono::DateTime::from_timestamp((if n > 1e12 { n / 1000.0 } else { n }) as i64, 0);
    }
    date(raw.as_str()?)
}
fn build(event: Event, data: Resource) -> Option<SearchResult> {
    let mut seen = HashSet::new();
    let urls = [&data.url, &data.content, &data.vod_content]
        .into_iter()
        .flat_map(|s| shares(s))
        .filter(|s| seen.insert(s.clone()))
        .collect::<Vec<_>>();
    if urls.is_empty() {
        return None;
    }
    let fallback = if (3..=16).contains(&data.code.len())
        && data.code.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        Some(data.code.clone())
    } else {
        [
            &data.code,
            &data.url,
            &data.content,
            &data.description,
            &event.description,
        ]
        .iter()
        .find_map(|s| password(s).or_else(|| PASS.captures(s).map(|c| c[1].into())))
    };
    let datetime = source_time(&data.update_time).or_else(|| source_time(&data.create_time));
    let links = urls
        .into_iter()
        .map(|url| {
            let mut link = Link::new(url);
            link.password = password(&link.url).or_else(|| fallback.clone());
            link.datetime = datetime;
            link
        })
        .collect::<Vec<_>>();
    let identity = format!(
        "{}\0{}",
        links[0].url,
        links[0].password.as_deref().unwrap_or("")
    );
    let digest = Sha256::digest(identity.as_bytes());
    let hex = digest[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let mut row = SearchResult::provider(format!("sopanya-{hex}"), "sopanya");
    row.title = if data.title.trim().is_empty() {
        event.title
    } else {
        data.title
    };
    row.content = [data.description, event.description, data.content]
        .into_iter()
        .find(|s| !s.trim().is_empty())
        .unwrap_or_default();
    row.datetime = datetime.or_else(|| Some(chrono::Utc::now()));
    row.links = links;
    Some(row)
}
fn permanent(message: &str) -> bool {
    let m = message.to_lowercase();
    [
        "封禁",
        "登录状态异常",
        "登录异常",
        "链接受限",
        "无权限",
        "forbidden",
        "unauthorized",
    ]
    .iter()
    .any(|s| m.contains(s))
}
impl Sopanya {
    fn headers(&self, r: reqwest::RequestBuilder, sse: bool) -> reqwest::RequestBuilder {
        r.header("user-agent", "Mozilla/5.0 Chrome/136.0.0.0 Safari/537.36")
            .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
            .header("origin", self.endpoints.base.origin().ascii_serialization())
            .header("referer", self.endpoints.base.as_str())
            .header(
                "accept",
                if sse {
                    "text/event-stream, application/json, text/plain, */*"
                } else {
                    "application/json, text/plain, */*"
                },
            )
    }
    async fn post<T: DeserializeOwned>(
        &self,
        ctx: &SearchContext,
        path: &str,
        payload: &Value,
    ) -> Result<T, ProviderError> {
        let r = crate::http::execute(
            self.headers(
                ctx.client
                    .post(self.endpoints.base.join(path).unwrap())
                    .timeout(ctx.remaining())
                    .json(payload),
                false,
            ),
        )
        .await
        .map_err(network)?;
        if !r.status().is_success() {
            return Err(ProviderError::Protocol(format!(
                "HTTP {}",
                r.status().as_u16()
            )));
        }
        let bytes = crate::http::read_response(r, LIMIT)
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        serde_json::from_slice(&bytes).map_err(|e| ProviderError::Parse(e.to_string()))
    }
    async fn resolve(
        &self,
        ctx: &SearchContext,
        event: Event,
    ) -> Result<SearchResult, ProviderError> {
        let direct = shares(&event.url);
        if !direct.is_empty() {
            let resource = Resource {
                title: event.title.clone(),
                url: direct[0].clone(),
                content: direct[1..].join("\n"),
                description: event.description.clone(),
                ..Default::default()
            };
            return build(event, resource).ok_or_else(|| {
                ProviderError::Parse("sopanya direct resource has no usable link".into())
            });
        }
        // Go QueryEscape uses form encoding (space -> +), not encodeURIComponent.
        let escaped = query_escape(&event.url);
        let payload =
            json!({"url":escaped,"title":event.title,"stoken":"","description":event.description});
        let first: Saved = self.post(ctx, "/api/other/save_url", &payload).await?;
        let mut data = first.data;
        if data.as_ref().is_none_or(|d| shares(&d.url).is_empty()) {
            if permanent(&first.message) {
                return Err(ProviderError::Unavailable(
                    "sopanya resource cannot be saved".into(),
                ));
            }
            for _ in 0..8 {
                tokio::time::sleep(Duration::from_millis(300)).await;
                let status = self
                    .post::<Status>(ctx, "/api/other/save_status", &payload)
                    .await?;
                let Some(status) = status.data.filter(|d| d.ready) else {
                    continue;
                };
                let Some(resource) = status.resource else {
                    continue;
                };
                let ready = !shares(&resource.url).is_empty();
                data = Some(resource);
                if ready {
                    break;
                }
            }
        }
        let data = data.ok_or_else(|| {
            ProviderError::Unavailable("sopanya save did not become ready".into())
        })?;
        build(event, data)
            .ok_or_else(|| ProviderError::Parse("sopanya saved resource has no usable link".into()))
    }
    async fn search_inner(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self.endpoints.base.join("/api/other/web_search").unwrap();
        url.query_pairs_mut()
            .append_pair("title", query)
            .append_pair("is_type", "-1");
        let response = crate::http::execute(
            self.headers(
                ctx.client
                    .get(url)
                    .timeout(ctx.remaining())
                    .header("accept-encoding", "identity"),
                true,
            ),
        )
        .await
        .map_err(network)?;
        if !response.status().is_success() {
            return Err(ProviderError::Protocol(format!(
                "HTTP {}",
                response.status().as_u16()
            )));
        }
        // Prefer incremental identity SSE. If a server ignores our identity
        // negotiation, use the shared bounded decoder rather than rejecting
        // valid compressed events or allowing unbounded decoded output.
        let compressed = response
            .headers()
            .get("content-encoding")
            .is_some_and(|v| v != "identity");
        let (mut response, mut buffered) = if compressed {
            let bytes = crate::http::read_response(response, LIMIT)
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?;
            (None, Some(bytes))
        } else {
            (Some(response), None)
        };
        let mut carry = Vec::with_capacity(4096);
        let mut total = 0;
        let mut done = false;
        let mut accepted = 0;
        let mut counts = HashMap::new();
        let mut tokens = HashSet::new();
        let mut queue = VecDeque::new();
        let mut pending = FuturesUnordered::new();
        let mut resolved = vec![];
        let mut first_error = None;
        while !done || !queue.is_empty() || !pending.is_empty() {
            while pending.len() < 6 {
                let Some((index, event)) = queue.pop_front() else {
                    break;
                };
                pending.push(async move { (index, self.resolve(ctx, event).await) }.boxed());
            }
            tokio::select! {
                result=pending.next(),if !pending.is_empty()=>{
                    if let Some((index,result))=result {
                        match result {
                            Ok(row)=>{
                                if !ctx.publish(vec![row.clone()]){return Err(ProviderError::Timeout)}
                                resolved.push((index,row));
                            }
                            Err(error)=>{first_error.get_or_insert(error);}
                        }
                    }
                }
                chunk=async {
                    if let Some(bytes)=buffered.take(){return Ok(Some(bytes))}
                    if let Some(response)=response.as_mut(){return response.chunk().await.map(|chunk|chunk.map(|bytes|bytes.to_vec()))}
                    Ok::<_,reqwest::Error>(None)
                },if !done=>{
                    match chunk {
                        Ok(Some(chunk))=>{
                            total+=chunk.len();
                            if total>LIMIT{return Err(ProviderError::Protocol("sopanya stream exceeds 2 MiB".into()))}
                            carry.extend_from_slice(&chunk);
                        }
                        Err(error)=>return Err(network(error)),
                        Ok(None)=>{
                            done=true;
                            if !carry.is_empty(){carry.push(b'\n')}
                        }
                    }
                    while let Some(end)=carry.iter().position(|b|*b==b'\n'){
                        if end>LINE_LIMIT{return Err(ProviderError::Protocol("sopanya SSE line exceeds 256 KiB".into()))}
                        let line=carry.drain(..=end).collect::<Vec<_>>();
                        let line=String::from_utf8_lossy(&line);
                        let Some(payload)=line.trim().strip_prefix("data:").map(str::trim)else{continue};
                        if payload=="[DONE]"{
                            done=true;carry.clear();break;
                        }
                        if payload.is_empty(){continue}
                        let Ok(event)=serde_json::from_str::<Event>(payload)else{continue};
                        if event.title.is_empty()||event.url.is_empty()||!event.kind.is_empty(){continue}
                        if *counts.get(&event.is_type).unwrap_or(&0)>=8||!tokens.insert(event.url.clone()){continue}
                        *counts.entry(event.is_type).or_insert(0)+=1;
                        queue.push_back((accepted,event));accepted+=1;
                        if accepted>=24{done=true;carry.clear();break}
                    }
                    if carry.len()>LINE_LIMIT{return Err(ProviderError::Protocol("sopanya SSE line exceeds 256 KiB".into()))}
                }
            }
        }
        if ctx.is_cancelled() {
            return Err(ProviderError::Timeout);
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        resolved.sort_by_key(|(i, _)| *i);
        let mut seen = HashSet::new();
        Ok(resolved
            .into_iter()
            .filter_map(|(_, row)| {
                let mut new = false;
                for l in &row.links {
                    new |= seen.insert((l.url.clone(), l.password.clone()))
                }
                new.then_some(row)
            })
            .collect())
    }
}
#[async_trait]
impl Provider for Sopanya {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("sopanya", 3, KeywordFilterMode::Core)
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
            self.search_inner(ctx, query.trim()),
        )
        .await
        .map_err(|_| ProviderError::Timeout)?
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shares_passwords_time() {
        assert_eq!(query_escape("a b/+*~"), "a+b%2F%2B%2A~");
        assert!(shares("https://evil.test/?u=pan.quark.cn").is_empty());
        let urls = shares("https://pan.baidu.com/s/a?pwd=abcd。 https://pan.quark.cn/s/b，");
        assert_eq!(urls.len(), 2);
        let row = build(
            Event {
                title: "movie".into(),
                url: "token".into(),
                description: "".into(),
                is_type: 0,
                kind: "".into(),
            },
            Resource {
                url: urls[1].clone(),
                code: "bare".into(),
                update_time: json!(1760000000001_i64),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(row.links[0].password.as_deref(), Some("bare"));
        assert!(row.id.starts_with("sopanya-"));
        assert_eq!(row.datetime.unwrap().timestamp(), 1760000000);
    }
    #[tokio::test]
    async fn opaque_token_poll_and_progress() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{body_json, path},
        };
        let server = MockServer::start().await;
        Mock::given(path("/api/other/web_search")).respond_with(ResponseTemplate::new(200).set_body_string("data: {\"title\":\"ignored\",\"url\":\"p\",\"type\":\"progress\"}\ndata: bad\ndata: {\"title\":\"movie\",\"url\":\"a b/+\"}\ndata: [DONE]\n")).expect(1).mount(&server).await;
        let payload = json!({"url":"a+b%2F%2B","title":"movie","description":"","stoken":""});
        Mock::given(path("/api/other/save_url"))
            .and(body_json(payload.clone()))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"code":500,"data":null})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/api/other/save_status")).and(body_json(payload)).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{"ready":true,"resource":{"url":"https://pan.baidu.com/s/a?pwd=xeqy"}}}))).expect(1).mount(&server).await;
        let p = Sopanya::new(Endpoints {
            base: Url::parse(&server.uri()).unwrap(),
        });
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
        let rows = p.search(&ctx, "movie").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].links[0].password.as_deref(), Some("xeqy"));
    }

    #[tokio::test]
    async fn failed_save_or_poll_returns_error_but_keeps_direct_sibling() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        for fail_poll in [false, true] {
            let server = MockServer::start().await;
            Mock::given(path("/api/other/web_search")).respond_with(ResponseTemplate::new(200).set_body_string("data: {\"title\":\"movie good\",\"url\":\"https://pan.quark.cn/s/good\"}\ndata: {\"title\":\"movie save\",\"url\":\"opaque\"}\ndata: [DONE]\n")).expect(1).mount(&server).await;
            Mock::given(path("/api/other/save_url"))
                .respond_with(if fail_poll {
                    ResponseTemplate::new(200).set_body_json(json!({"data":null}))
                } else {
                    ResponseTemplate::new(503)
                })
                .expect(1)
                .mount(&server)
                .await;
            if fail_poll {
                Mock::given(path("/api/other/save_status"))
                    .respond_with(ResponseTemplate::new(502))
                    .expect(1)
                    .mount(&server)
                    .await;
            }
            let provider = Sopanya::new(Endpoints {
                base: Url::parse(&server.uri()).unwrap(),
            });
            let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5))
                .for_source(tokio::time::Instant::now() + Duration::from_secs(5));
            let error = provider.search(&ctx, "movie").await.unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(if fail_poll { "502" } else { "503" })
            );
            assert_eq!(ctx.finish(vec![]).len(), 1);
        }
    }

    #[tokio::test]
    async fn sse_line_and_total_limits_are_explicit_and_gzip_is_bounded() {
        use std::io::Write;
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        for (size, line, compressed, success) in [
            (LINE_LIMIT, true, false, true),
            (LINE_LIMIT + 1, true, false, false),
            (LIMIT, false, false, true),
            (LIMIT + 1, false, false, false),
            (LIMIT, false, true, true),
            (LIMIT + 1, false, true, false),
        ] {
            let server = MockServer::start().await;
            let mut body = vec![b':'; size];
            if compressed {
                // Deterministic high-entropy comments exercise the source cap,
                // not the independent 100:1 / 400 KiB gzip ratio limit.
                let mut state = 0x12345678_u32;
                for (index, byte) in body.iter_mut().enumerate() {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    *byte = if index % 1024 == 0 {
                        b':'
                    } else {
                        b'A' + (state % 26) as u8
                    };
                }
            }
            if !line {
                for end in (1023..size).step_by(1024) {
                    body[end] = b'\n';
                }
            }
            let mut template = ResponseTemplate::new(200);
            if compressed {
                let mut encoder =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(&body).unwrap();
                body = encoder.finish().unwrap();
                template = template.insert_header("content-encoding", "gzip");
            }
            Mock::given(path("/api/other/web_search"))
                .respond_with(template.set_body_bytes(body))
                .expect(1)
                .mount(&server)
                .await;
            let provider = Sopanya::new(Endpoints {
                base: Url::parse(&server.uri()).unwrap(),
            });
            let client = crate::http::HttpClientFactory::new()
                .client(&crate::http::ClientOptions::default())
                .unwrap();
            let ctx = SearchContext::new(client, Duration::from_secs(5));
            let result = provider.search(&ctx, "movie").await;
            assert_eq!(
                result.is_ok(),
                success,
                "size={size} line={line} compressed={compressed}"
            );
        }
    }

    #[tokio::test]
    async fn candidate_type_and_total_caps_and_empty_stream() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        for empty in [true, false] {
            let server = MockServer::start().await;
            let mut body = String::new();
            if !empty {
                for kind in 0..4 {
                    for index in 0..10 {
                        body.push_str(&format!("data: {}\n",json!({"title":"movie","url":format!("https://pan.quark.cn/s/{kind}-{index}"),"is_type":kind})));
                    }
                }
            }
            body.push_str("data: [DONE]\n");
            Mock::given(path("/api/other/web_search"))
                .respond_with(ResponseTemplate::new(200).set_body_string(body))
                .expect(1)
                .mount(&server)
                .await;
            let provider = Sopanya::new(Endpoints {
                base: Url::parse(&server.uri()).unwrap(),
            });
            let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
            assert_eq!(
                provider.search(&ctx, "movie").await.unwrap().len(),
                if empty { 0 } else { 24 }
            );
        }
    }

    #[tokio::test]
    async fn save_poll_exhaustion_is_eight_attempts_and_permanent_failure_does_not_poll() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        for permanent in [false, true] {
            let server = MockServer::start().await;
            Mock::given(path("/api/other/web_search"))
                .respond_with(ResponseTemplate::new(200).set_body_string(
                    "data: {\"title\":\"movie\",\"url\":\"opaque\"}\ndata: [DONE]\n",
                ))
                .mount(&server)
                .await;
            Mock::given(path("/api/other/save_url"))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    json!({"data":null,"message":if permanent{"forbidden"}else{""}}),
                ))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(path("/api/other/save_status"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"data":{"ready":false}})),
                )
                .expect(if permanent { 0 } else { 8 })
                .mount(&server)
                .await;
            let provider = Sopanya::new(Endpoints {
                base: Url::parse(&server.uri()).unwrap(),
            });
            let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
            assert!(provider.search(&ctx, "movie").await.is_err());
        }
    }

    #[tokio::test]
    async fn save_workers_never_exceed_six_and_drop_cancels_queued_saves() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        let server = MockServer::start().await;
        let mut body = String::new();
        for index in 0..12 {
            body.push_str(&format!(
                "data: {}\n",
                json!({"title":"movie","url":format!("opaque-{index}"),"is_type":index%2})
            ));
        }
        body.push_str("data: [DONE]\n");
        Mock::given(path("/api/other/web_search"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;
        Mock::given(path("/api/other/save_url"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(30))
                    .set_body_json(json!({"data":null})),
            )
            .expect(6)
            .mount(&server)
            .await;
        let provider = Sopanya::new(Endpoints {
            base: Url::parse(&server.uri()).unwrap(),
        });
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(10));
        tokio::time::timeout(Duration::from_secs(5),async{
            tokio::select!{
                result=provider.search(&ctx,"movie")=>panic!("search unexpectedly finished: {result:?}"),
                _=async{
                    loop{
                        let requests=server.received_requests().await.unwrap();
                        if requests.iter().filter(|r|r.url.path()=="/api/other/save_url").count()==6{break}
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }=>{}
            }
        }).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.url.path() == "/api/other/save_url")
                .count(),
            6
        );
    }

    #[tokio::test]
    async fn highly_compressible_sse_still_enforces_shared_ratio_limit() {
        use std::io::Write;
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        let server = MockServer::start().await;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&vec![b':'; LIMIT]).unwrap();
        Mock::given(path("/api/other/web_search"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-encoding", "gzip")
                    .set_body_bytes(encoder.finish().unwrap()),
            )
            .mount(&server)
            .await;
        let provider = Sopanya::new(Endpoints {
            base: Url::parse(&server.uri()).unwrap(),
        });
        let client = crate::http::HttpClientFactory::new()
            .client(&crate::http::ClientOptions::default())
            .unwrap();
        let ctx = SearchContext::new(client, Duration::from_secs(5));
        let error = provider.search(&ctx, "movie").await.unwrap_err();
        assert!(error.to_string().contains("byte limit"));
    }
}
