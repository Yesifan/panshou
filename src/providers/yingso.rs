use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{Link, ProviderError, SearchResult};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use rand::Rng;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{collections::HashSet, time::Duration};
use url::Url;
const LIMIT: usize = 1024 * 1024;
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://ysapi.yingso.fun/").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Yingso {
    endpoints: Endpoints,
}
impl Yingso {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
#[derive(Deserialize)]
struct Envelope<T> {
    code: i32,
    data: T,
}
#[derive(Deserialize)]
struct Config {
    url_version: String,
    user_id: String,
    start: usize,
    end: usize,
}
#[derive(Deserialize)]
struct Item {
    id: i64,
    title: String,
    root: u8,
}
fn xor_utf16(input: &str, key: &str) -> String {
    let key = key.encode_utf16().collect::<Vec<_>>();
    if key.is_empty() {
        return String::new();
    }
    String::from_utf16_lossy(
        &input
            .encode_utf16()
            .enumerate()
            .map(|(i, x)| x ^ key[i % key.len()])
            .collect::<Vec<_>>(),
    )
}
fn encrypt(payload: &Value, c: &Config) -> Result<Value, ProviderError> {
    if c.start >= c.end || c.end > 24 {
        return Err(ProviderError::Protocol("invalid yingso key range".into()));
    }
    let no = (0..24)
        .map(|_| {
            let i = rand::rng().random_range(0..36);
            b"0123456789abcdefghijklmnopqrstuvwxyz"[i] as char
        })
        .collect::<String>();
    Ok(json!({"info":xor_utf16(&payload.to_string(),&no[c.start..c.end]),"no":no}))
}
impl Yingso {
    async fn api<T: DeserializeOwned>(
        &self,
        ctx: &SearchContext,
        path: &str,
        payload: Option<&Value>,
    ) -> Result<T, ProviderError> {
        let url = self
            .endpoints
            .base
            .join(path)
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        let body = request(
            || {
                let r = if let Some(p) = payload {
                    ctx.client.post(url.clone()).json(p)
                } else {
                    ctx.client.get(url.clone())
                };
                r.timeout(ctx.remaining())
                    .header("origin", "https://yingso.fun")
                    .header("referer", "https://yingso.fun/")
                    .header("accept", "application/json, text/plain, */*")
                    .header("accept-language", "zh-CN,zh;q=0.9")
                    .header("user-agent", "Mozilla/5.0 Chrome/124.0.0.0 Safari/537.36")
            },
            1,
            Duration::ZERO,
            LIMIT,
        )
        .await?;
        let response: Envelope<T> =
            serde_json::from_slice(&body).map_err(|e| ProviderError::Parse(e.to_string()))?;
        if response.code != 200 {
            return Err(ProviderError::Protocol(format!(
                "yingso API code {}",
                response.code
            )));
        }
        Ok(response.data)
    }
    async fn config(&self, ctx: &SearchContext) -> Result<Config, ProviderError> {
        let mut last = ProviderError::Protocol("yingso configuration unavailable".into());
        for (path, key) in [("/test1", "672134612"), ("/test", "12345678")] {
            let result = async {
                let raw: String = self.api(ctx, path, None).await?;
                let c: Config = serde_json::from_str(&xor_utf16(&raw, key))
                    .map_err(|e| ProviderError::Parse(e.to_string()))?;
                if c.url_version.trim().is_empty()
                    || c.user_id.trim().is_empty()
                    || c.start >= c.end
                    || c.end > 24
                {
                    return Err(ProviderError::Protocol(
                        "invalid yingso configuration".into(),
                    ));
                }
                Ok(c)
            }
            .await;
            match result {
                Ok(c) => return Ok(c),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
    async fn resolve(
        &self,
        ctx: &SearchContext,
        c: &Config,
        item: Item,
    ) -> Result<SearchResult, ProviderError> {
        let payload = encrypt(&json!({"id":item.id,"userId":c.user_id}), c)?;
        let version = percent_encoding::utf8_percent_encode(
            &c.url_version,
            percent_encoding::NON_ALPHANUMERIC,
        );
        let mut key = None;
        let mut last = ProviderError::Protocol("empty share key".into());
        for path in ["getKey670I23762183", "getKey"] {
            match self
                .api::<String>(ctx, &format!("/{version}/{path}"), Some(&payload))
                .await
            {
                Ok(raw) if !raw.trim().is_empty() => {
                    key = Some(raw);
                    break;
                }
                Ok(_) => {}
                Err(e) => last = e,
            }
        }
        let key =
            key.ok_or_else(|| ProviderError::Protocol(format!("getKey id={}: {last}", item.id)))?;
        let prefix = match item.root {
            1 => "https://www.aliyundrive.com/s/",
            2 => "https://pan.quark.cn/s/",
            3 => "https://pan.xunlei.com/s/",
            4 => "https://pan.baidu.com/s/",
            5 => "https://drive.uc.cn/s/",
            _ => {
                return Err(ProviderError::Protocol(format!(
                    "unsupported yingso root {} id={}",
                    item.root, item.id
                )));
            }
        };
        let key = key.trim();
        let raw = if key.starts_with("https://") || key.starts_with("http://") {
            key.to_owned()
        } else {
            format!("{prefix}{}", key.trim_start_matches('/'))
        };
        let url = Url::parse(&raw).map_err(|e| ProviderError::Parse(e.to_string()))?;
        if url.host_str().is_none() {
            return Err(ProviderError::Parse("invalid yingso share URL".into()));
        }
        let mut row = SearchResult::provider(format!("yingso-{}", item.id), "yingso");
        row.title = item.title.split_whitespace().collect::<Vec<_>>().join(" ");
        if row.title.is_empty() {
            row.title = format!("影搜资源 {}", item.id)
        }
        row.content = row.title.clone();
        row.datetime = Some(chrono::Utc::now());
        let mut link = Link::new(url.to_string());
        link.password = password(&link.url);
        link.work_title = Some(row.title.clone());
        link.datetime = row.datetime;
        row.links.push(link);
        if !ctx.publish(vec![row.clone()]) {
            return Err(ProviderError::Timeout);
        }
        Ok(row)
    }
    async fn search_inner(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let c = self.config(ctx).await?;
        let payload = encrypt(
            &json!({"pageSize":30,"pageNum":1,"title":query,"root":0,"cat":"all","userId":c.user_id}),
            &c,
        )?;
        let version = percent_encoding::utf8_percent_encode(
            &c.url_version,
            percent_encoding::NON_ALPHANUMERIC,
        );
        let items: Vec<Item> = self
            .api(ctx, &format!("/{version}/search"), Some(&payload))
            .await?;
        let mut pending =
            stream::iter(items.into_iter().map(|item| self.resolve(ctx, &c, item))).buffered(8);
        let mut rows = vec![];
        let mut first = None;
        let mut seen = HashSet::new();
        while let Some(result) = pending.next().await {
            match result {
                Ok(row) => {
                    let l = &row.links[0];
                    if seen.insert((l.url.clone(), l.password.clone())) {
                        rows.push(row)
                    }
                }
                Err(e) => {
                    first.get_or_insert(e);
                }
            }
        }
        if let Some(e) = first {
            return Err(e);
        }
        complete(ctx, Ok(rows))
    }
}
#[async_trait]
impl Provider for Yingso {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("yingso", 3, KeywordFilterMode::Core)
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
        tokio::time::timeout(
            ctx.timeout.min(Duration::from_secs(25)),
            self.search_inner(ctx, &query),
        )
        .await
        .map_err(|_| ProviderError::Timeout)?
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf16_and_ranges() {
        let s = "中文 😀 non-BMP";
        let encrypted = xor_utf16(s, "12345678");
        assert_eq!(encrypted.encode_utf16().count(), s.encode_utf16().count());
        assert_eq!(xor_utf16(&encrypted, "12345678"), s);
        let mut c = Config {
            url_version: "v1".into(),
            user_id: "u".into(),
            start: 0,
            end: 24,
        };
        assert!(encrypt(&json!({"a":"😀"}), &c).is_ok());
        c.end = 25;
        assert!(encrypt(&json!({}), &c).is_err());
        c.end = 0;
        assert!(encrypt(&json!({}), &c).is_err());
    }
    #[tokio::test]
    async fn slow_first_does_not_hide_fast_second_at_deadline() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        let server = MockServer::start().await;
        let config = json!({"url_version":"v1","user_id":"u","start":0,"end":24}).to_string();
        Mock::given(path("/test1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"code":200,"data":xor_utf16(&config,"672134612")})),
            )
            .mount(&server)
            .await;
        Mock::given(path("/v1/search")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"code":200,"data":[{"id":1,"title":"slow","root":2},{"id":2,"title":"fast","root":2}]}))).mount(&server).await;
        Mock::given(path("/v1/getKey670I23762183"))
            .respond_with(|r: &wiremock::Request| {
                let body: Value = serde_json::from_slice(&r.body).unwrap();
                let decoded: Value = serde_json::from_str(&xor_utf16(
                    body["info"].as_str().unwrap(),
                    body["no"].as_str().unwrap(),
                ))
                .unwrap();
                let response = ResponseTemplate::new(200).set_body_json(
                    json!({"code":200,"data":if decoded["id"]==1{"slow"}else{"fast"}}),
                );
                if decoded["id"] == 1 {
                    response.set_delay(Duration::from_secs(2))
                } else {
                    response
                }
            })
            .mount(&server)
            .await;
        let p = Yingso::new(Endpoints {
            base: Url::parse(&server.uri()).unwrap(),
        });
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5))
            .for_source(tokio::time::Instant::now() + Duration::from_millis(150));
        let _ = tokio::time::timeout(Duration::from_millis(180), p.search(&ctx, "movie")).await;
        let rows = ctx.finish(vec![]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "yingso-2");
    }
    #[tokio::test]
    async fn new_and_legacy_config_and_key_fallback() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        for legacy in [false, true] {
            let server = MockServer::start().await;
            let config = json!({"url_version":"v1","user_id":"u","start":1,"end":12}).to_string();
            if legacy {
                Mock::given(path("/test1"))
                    .respond_with(
                        ResponseTemplate::new(200).set_body_json(json!({"code":200,"data":"bad"})),
                    )
                    .expect(1)
                    .mount(&server)
                    .await;
            }
            Mock::given(path(if legacy{"/test"}else{"/test1"})).respond_with(ResponseTemplate::new(200).set_body_json(json!({"code":200,"data":xor_utf16(&config,if legacy{"12345678"}else{"672134612"})}))).expect(1).mount(&server).await;
            Mock::given(method("POST")).and(path("/v1/search")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"code":200,"data":[{"id":9007199254740993_i64,"title":"movie","root":2}]}))).expect(1).mount(&server).await;
            Mock::given(path("/v1/getKey670I23762183"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"code":200,"data":if legacy{" "}else{"new-share"}})),
                )
                .expect(1)
                .mount(&server)
                .await;
            if legacy {
                Mock::given(path("/v1/getKey"))
                    .respond_with(
                        ResponseTemplate::new(200)
                            .set_body_json(json!({"code":200,"data":"legacy-share"})),
                    )
                    .expect(1)
                    .mount(&server)
                    .await;
            }
            let provider = Yingso::new(Endpoints {
                base: Url::parse(&server.uri()).unwrap(),
            });
            let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(10));
            let rows = provider.search(&ctx, "movie").await.unwrap();
            assert_eq!(
                rows[0].links[0].url,
                format!(
                    "https://pan.quark.cn/s/{}-share",
                    if legacy { "legacy" } else { "new" }
                )
            );
        }
    }

    #[tokio::test]
    async fn sibling_key_failure_is_not_complete_success_and_empty_search_is_legal() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        for mode in 0..3 {
            let server = MockServer::start().await;
            let config = json!({"url_version":"v1","user_id":"u","start":0,"end":24}).to_string();
            Mock::given(path("/test1"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"code":200,"data":xor_utf16(&config,"672134612")})),
                )
                .mount(&server)
                .await;
            let items = if mode == 0 {
                json!([])
            } else {
                json!([{"id":1,"title":"movie good","root":2},{"id":2,"title":"movie failed","root":2}])
            };
            Mock::given(path("/v1/search"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"code":200,"data":items})),
                )
                .mount(&server)
                .await;
            for endpoint in ["/v1/getKey670I23762183", "/v1/getKey"] {
                Mock::given(path(endpoint))
                    .respond_with(move |request: &wiremock::Request| {
                        let envelope: Value = serde_json::from_slice(&request.body).unwrap();
                        let payload: Value = serde_json::from_str(&xor_utf16(
                            envelope["info"].as_str().unwrap(),
                            envelope["no"].as_str().unwrap(),
                        ))
                        .unwrap();
                        if mode == 1 && payload["id"] == 1 {
                            ResponseTemplate::new(200)
                                .set_body_json(json!({"code":200,"data":"good-share"}))
                        } else {
                            ResponseTemplate::new(503)
                        }
                    })
                    .mount(&server)
                    .await;
            }
            let provider = Yingso::new(Endpoints {
                base: Url::parse(&server.uri()).unwrap(),
            });
            let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5))
                .for_source(tokio::time::Instant::now() + Duration::from_secs(5));
            let result = provider.search(&ctx, "movie").await;
            if mode == 0 {
                assert!(result.unwrap().is_empty())
            } else {
                assert!(result.is_err())
            }
            assert_eq!(ctx.finish(vec![]).len(), if mode == 1 { 1 } else { 0 });
        }
    }
}
