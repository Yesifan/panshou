use super::engine::CheckEvaluation;
use super::protocol::{contains_any, json, number, string};
use super::{CheckCloudType, CheckContext, CheckError, CheckItem, CheckState, LinkChecker};
use async_trait::async_trait;
use regex::Regex;
use url::Url;
pub struct QuarkChecker {
    token: String,
    detail: String,
}
impl Default for QuarkChecker {
    fn default() -> Self {
        Self {
            token: "https://drive-h.quark.cn/1/clouddrive/share/sharepage/token".into(),
            detail: "https://drive-pc.quark.cn/1/clouddrive/share/sharepage/detail".into(),
        }
    }
}
#[async_trait]
impl LinkChecker for QuarkChecker {
    fn cloud_type(&self) -> CheckCloudType {
        CheckCloudType::Quark
    }
    async fn check_normalized(
        &self,
        ctx: &CheckContext,
        _: &CheckItem,
        n: &str,
    ) -> Result<CheckEvaluation, CheckError> {
        let id = Regex::new(r"/s/([A-Za-z0-9]+)")
            .unwrap()
            .captures(n)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| CheckError::Protocol("missing share id".into()))?;
        let pwd = Url::parse(n)
            .ok()
            .and_then(|u| {
                u.query_pairs()
                    .find(|(k, _)| k == "pwd")
                    .map(|(_, v)| v.into_owned())
            })
            .unwrap_or_default();
        let v=json(&ctx.client.post(&self.token).header("origin","https://pan.quark.cn").header("referer","https://pan.quark.cn/").json(&serde_json::json!({"pwd_id":id,"passcode":pwd,"support_visit_limit_private_share":true})).send().await?.bytes().await?)?;
        let code = number(&v, &["code"]);
        let msg = string(&v, &["message"]);
        if code != 0 {
            return Ok(match code {
                41008 => CheckEvaluation::new(CheckState::Locked, "需要提取码"),
                41004 | 41010 | 41011 => CheckEvaluation::new(CheckState::Bad, "链接失效"),
                _ if contains_any(msg, &["提取码", "密码", "passcode"]) => {
                    CheckEvaluation::new(CheckState::Locked, msg)
                }
                _ if contains_any(msg, &["不存在", "失效", "违规", "过期", "取消"]) => {
                    CheckEvaluation::new(CheckState::Bad, msg)
                }
                _ => CheckEvaluation::new(CheckState::Uncertain, msg),
            });
        }
        let stoken = v
            .pointer("/data/stoken")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CheckError::Protocol("missing stoken".into()))?;
        let v = json(
            &ctx.client
                .get(&self.detail)
                .query(&[
                    ("pwd_id", id),
                    ("stoken", stoken),
                    ("ver", "2"),
                    ("pr", "ucpro"),
                ])
                .header("origin", "https://pan.quark.cn")
                .header("referer", "https://pan.quark.cn/")
                .send()
                .await?
                .bytes()
                .await?,
        )?;
        let code = number(&v, &["code"]);
        let msg = string(&v, &["message"]);
        if code != 0 {
            return Ok(if contains_any(msg, &["提取码", "密码", "passcode"]) {
                CheckEvaluation::new(CheckState::Locked, msg)
            } else if contains_any(msg, &["不存在", "失效", "违规", "过期", "取消"]) {
                CheckEvaluation::new(CheckState::Bad, msg)
            } else {
                CheckEvaluation::new(CheckState::Uncertain, msg)
            });
        }
        let list = v.pointer("/data/list").and_then(|v| v.as_array());
        let status = v
            .pointer("/data/share/status")
            .and_then(|v| v.as_i64())
            .unwrap_or_default();
        let partial = v
            .pointer("/data/share/partial_violation")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let expired = v
            .pointer("/data/is_expire")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if list.is_none_or(|l| l.is_empty()) {
            return Ok(CheckEvaluation::new(
                CheckState::Bad,
                if expired {
                    "分享链接已过期"
                } else if partial {
                    "分享链接部分违规已失效"
                } else {
                    "分享链接无效：文件列表为空"
                },
            ));
        }
        if status == 3 && !partial {
            return Ok(CheckEvaluation::new(CheckState::Ok, "链接有效"));
        }
        if status > 1 {
            return Ok(CheckEvaluation::new(
                CheckState::Bad,
                format!("分享链接已失效(share_status={status})"),
            ));
        }
        Ok(CheckEvaluation::new(
            CheckState::Ok,
            if partial {
                "链接有效但部分文件违规"
            } else {
                "链接有效"
            },
        ))
    }
}
