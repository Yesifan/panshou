use super::engine::CheckEvaluation;
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
                .send()
                .await?
                .bytes()
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
            if list > 0 || count > 0 || !snap.is_empty() {
                return Ok(CheckEvaluation::new(CheckState::Ok, "链接有效"));
            }
            let ss = v
                .pointer("/data/share_state")
                .and_then(|v| v.as_i64())
                .or_else(|| {
                    v.pointer("/data/shareinfo/share_state")
                        .and_then(|v| v.as_i64())
                })
                .unwrap_or(0);
            if ss == 1 {
                return Ok(CheckEvaluation::new(CheckState::Ok, "链接有效"));
            }
            let reason = v
                .pointer("/data/shareinfo/forbid_reason")
                .and_then(|v| v.as_str())
                .unwrap_or("链接状态异常");
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
