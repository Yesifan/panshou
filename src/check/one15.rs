use super::engine::CheckEvaluation;
use super::protocol::{BoundedRequest, BoundedResponse};
use super::protocol::{contains_any, json, number, string};
use super::{CheckCloudType, CheckContext, CheckError, CheckItem, CheckState, LinkChecker};
use async_trait::async_trait;
use url::Url;
pub struct One15Checker {
    endpoint: String,
}
impl Default for One15Checker {
    fn default() -> Self {
        Self {
            endpoint: "https://115cdn.com/webapi/share/snap".into(),
        }
    }
}
fn info(n: &str, fallback: Option<&str>) -> Option<(String, String)> {
    let u = Url::parse(n).ok()?;
    let code = u.path_segments()?.rfind(|s| !s.is_empty())?.to_owned();
    let pwd = u
        .query_pairs()
        .find(|(k, _)| k == "password")
        .map(|(_, v)| v.into_owned())
        .or_else(|| {
            u.fragment()
                .and_then(|f| Url::parse(&format!("x://x/?{f}")).ok())
                .and_then(|u| {
                    u.query_pairs()
                        .find(|(k, _)| k == "password")
                        .map(|(_, v)| v.into_owned())
                })
        })
        .or_else(|| fallback.map(str::to_owned))
        .unwrap_or_default();
    Some((code, pwd))
}
#[async_trait]
impl LinkChecker for One15Checker {
    fn cloud_type(&self) -> CheckCloudType {
        CheckCloudType::One15
    }
    async fn check_normalized(
        &self,
        ctx: &CheckContext,
        item: &CheckItem,
        n: &str,
    ) -> Result<CheckEvaluation, CheckError> {
        let (code, pwd) = info(n, item.password.as_deref())
            .ok_or_else(|| CheckError::Protocol("missing share code".into()))?;
        if pwd.is_empty() {
            return Ok(CheckEvaluation::new(CheckState::Locked, "115 需要提取码"));
        }
        let v = json(
            &ctx.client
                .get(&self.endpoint)
                .query(&[
                    ("share_code", code.as_str()),
                    ("offset", "0"),
                    ("limit", "20"),
                    ("receive_code", pwd.as_str()),
                    ("cid", ""),
                ])
                .header(
                    "referer",
                    format!("https://115cdn.com/s/{code}?password={pwd}&"),
                )
                .header("x-requested-with", "XMLHttpRequest")
                .gated_send()
                .await?
                .bounded_bytes()
                .await?,
        )?;
        let state = v.get("state").and_then(|v| v.as_bool()).unwrap_or(false);
        let errno = number(&v, &["errno"]);
        let error = string(&v, &["error"]);
        if state && errno == 0 {
            let list = v
                .pointer("/data/list")
                .and_then(|v| v.as_array())
                .map_or(0, Vec::len);
            let count = v
                .pointer("/data/count")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let snap = v
                .pointer("/data/shareinfo/snap_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let title = v
                .pointer("/data/shareinfo/share_title")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let top_state = v
                .pointer("/data/share_state")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let ss = if top_state == 0 {
                v.pointer("/data/shareinfo/share_state")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
            } else {
                top_state
            };
            let reason = v
                .pointer("/data/shareinfo/forbid_reason")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if reason.is_empty()
                && matches!(ss, 0 | 1)
                && (ss == 1 || list > 0 || count > 0 || !snap.is_empty() || !title.is_empty())
            {
                return Ok(CheckEvaluation::new(CheckState::Ok, "链接有效"));
            }
            let abnormal = format!("链接状态异常(share_state={ss})");
            let reason = if reason.is_empty() { &abnormal } else { reason };
            return Ok(if contains_any(reason, &["密码", "提取码"]) {
                CheckEvaluation::new(CheckState::Locked, reason)
            } else {
                CheckEvaluation::new(CheckState::Bad, reason)
            });
        }
        Ok(
            if contains_any(error, &["密码", "提取码", "receive_code"]) {
                CheckEvaluation::new(CheckState::Locked, error)
            } else if error.is_empty() {
                CheckEvaluation::new(CheckState::Uncertain, "无法确认链接状态")
            } else {
                CheckEvaluation::new(CheckState::Bad, error)
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, query_param},
    };

    #[tokio::test]
    async fn interrupted_body_is_not_cached_and_next_check_retries() {
        use crate::check::{CheckCache, CheckEngine, CheckOptions};
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for index in 0..2 {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") {
                    if socket.read(&mut byte).unwrap() == 0 {
                        break;
                    }
                    request.push(byte[0]);
                }
                let body = if index == 0 {
                    "{"
                } else {
                    r#"{"state":true,"errno":0,"data":{"share_state":1}}"#
                };
                let length = if index == 0 { 1000 } else { body.len() };
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}"
                )
                .unwrap();
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let cache = CheckCache::open(dir.path().join("check.redb")).unwrap();
        let engine = CheckEngine::with_checkers(
            reqwest::Client::new(),
            Some(cache),
            vec![std::sync::Arc::new(One15Checker {
                endpoint: format!("http://{address}/snap"),
            })],
        );
        let item = CheckItem::detect("https://115cdn.com/s/example?password=CNYY");
        let first = engine
            .check(vec![item.clone()], CheckOptions::default())
            .await;
        assert_eq!(first[0].state, CheckState::Uncertain);
        let second = engine.check(vec![item], CheckOptions::default()).await;
        assert_eq!(second[0].state, CheckState::Ok);
        assert!(!second[0].cache_hit);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn state_and_forbid_reason_precede_every_content_signal() {
        let cases = vec![
            (json!({"share_state":7,"list":[{}]}), CheckState::Bad),
            (json!({"share_state":7,"count":1}), CheckState::Bad),
            (
                json!({"share_state":7,"shareinfo":{"snap_id":"id"}}),
                CheckState::Bad,
            ),
            (
                json!({"share_state":7,"shareinfo":{"share_title":"title"}}),
                CheckState::Bad,
            ),
            (
                json!({"share_state":0,"list":[{}],"shareinfo":{"share_state":7}}),
                CheckState::Bad,
            ),
            (
                json!({"share_state":7,"shareinfo":{"share_state":1}}),
                CheckState::Bad,
            ),
            (
                json!({"share_state":1,"shareinfo":{"forbid_reason":" 已取消 "}}),
                CheckState::Bad,
            ),
            (
                json!({"share_state":1,"list":[{}],"shareinfo":{"forbid_reason":" 密码错误 "}}),
                CheckState::Locked,
            ),
            (json!({"share_state":1}), CheckState::Ok),
            (json!({"share_state":0,"list":[{}]}), CheckState::Ok),
            (json!({"share_state":0,"count":1}), CheckState::Ok),
            (
                json!({"share_state":0,"shareinfo":{"snap_id":"id"}}),
                CheckState::Ok,
            ),
            (
                json!({"share_state":0,"shareinfo":{"share_title":"title","forbid_reason":"  "}}),
                CheckState::Ok,
            ),
            (json!({}), CheckState::Bad),
        ];
        let server = MockServer::start().await;
        let checker = One15Checker {
            endpoint: format!("{}/webapi/share/snap", server.uri()),
        };
        let ctx = CheckContext {
            client: reqwest::Client::new(),
            timeout: std::time::Duration::from_secs(5),
        };
        let item = CheckItem::detect("https://115cdn.com/s/swff16b3ngy?password=CNYY");
        for (data, expected) in cases {
            server.reset().await;
            Mock::given(method("GET"))
                .and(path("/webapi/share/snap"))
                .and(query_param("share_code", "swff16b3ngy"))
                .and(query_param("receive_code", "CNYY"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"state":true,"errno":0,"data":data})),
                )
                .expect(1)
                .mount(&server)
                .await;
            let result = checker
                .check_normalized(&ctx, &item, &item.url)
                .await
                .unwrap();
            assert_eq!(result.state, expected);
            server.verify().await;
        }
    }
}
