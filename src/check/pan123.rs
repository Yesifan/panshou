use super::engine::CheckEvaluation;
use super::normalize::last_path_segment;
use super::protocol::{json, number, string};
use super::{CheckCloudType, CheckContext, CheckError, CheckItem, CheckState, LinkChecker};
use async_trait::async_trait;
pub struct Pan123Checker {
    endpoint: String,
}
impl Default for Pan123Checker {
    fn default() -> Self {
        Self {
            endpoint: "https://www.123pan.com/api/share/info".into(),
        }
    }
}
#[async_trait]
impl LinkChecker for Pan123Checker {
    fn cloud_type(&self) -> CheckCloudType {
        CheckCloudType::Pan123
    }
    async fn check_normalized(
        &self,
        ctx: &CheckContext,
        _: &CheckItem,
        n: &str,
    ) -> Result<CheckEvaluation, CheckError> {
        let key =
            last_path_segment(n).ok_or_else(|| CheckError::Protocol("missing share key".into()))?;
        let r = ctx
            .client
            .get(&self.endpoint)
            .query(&[("shareKey", key)])
            .send()
            .await?;
        if r.status() == reqwest::StatusCode::FORBIDDEN {
            return Ok(CheckEvaluation::new(CheckState::Ok, "链接有效"));
        }
        let v = json(&r.bytes().await?)?;
        Ok(if number(&v, &["code"]) == 0 {
            CheckEvaluation::new(CheckState::Ok, "链接有效")
        } else if v
            .pointer("/data/HasPwd")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            CheckEvaluation::new(CheckState::Locked, "需要提取码")
        } else {
            CheckEvaluation::new(
                CheckState::Bad,
                if string(&v, &["message"]).is_empty() {
                    "链接失效"
                } else {
                    string(&v, &["message"])
                },
            )
        })
    }
}
