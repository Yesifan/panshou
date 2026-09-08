use super::engine::CheckEvaluation;
use super::normalize::last_path_segment;
use super::protocol::{contains_any, json, number, string};
use super::{CheckCloudType, CheckContext, CheckError, CheckItem, CheckState, LinkChecker};
use async_trait::async_trait;

pub struct AliyunChecker {
    endpoint: String,
}
impl Default for AliyunChecker {
    fn default() -> Self {
        Self {
            endpoint: "https://api.aliyundrive.com/adrive/v3/share_link/get_share_by_anonymous"
                .into(),
        }
    }
}
#[async_trait]
impl LinkChecker for AliyunChecker {
    fn cloud_type(&self) -> CheckCloudType {
        CheckCloudType::Aliyun
    }
    async fn check_normalized(
        &self,
        ctx: &CheckContext,
        _: &CheckItem,
        normalized: &str,
    ) -> Result<CheckEvaluation, CheckError> {
        let id = last_path_segment(normalized)
            .ok_or_else(|| CheckError::Protocol("missing share id".into()))?;
        let response = ctx
            .client
            .post(format!("{}?share_id={id}", self.endpoint))
            .header("origin", "https://www.alipan.com")
            .header("referer", "https://www.alipan.com/")
            .header("x-canary", "client=web,app=share,version=v2.3.1")
            .json(&serde_json::json!({"share_id":id}))
            .send()
            .await?;
        let status = response.status();
        let body = response.bytes().await?;
        let value = json(&body)?;
        let code = string(&value, &["code"]);
        let message = string(&value, &["message"]);
        if !code.is_empty() {
            return Ok(
                if contains_any(
                    code,
                    &[
                        "sharelink",
                        "notfound",
                        "cancelled",
                        "canceled",
                        "forbidden",
                        "expired",
                    ],
                ) {
                    CheckEvaluation::new(
                        CheckState::Bad,
                        if message.is_empty() { code } else { message },
                    )
                } else {
                    CheckEvaluation::new(
                        CheckState::Uncertain,
                        if message.is_empty() { code } else { message },
                    )
                },
            );
        }
        if number(&value, &["file_count"]) == 0
            && string(&value, &["share_name"]).is_empty()
            && value.get("file_count").is_some()
        {
            return Ok(CheckEvaluation::new(
                CheckState::Bad,
                "分享内容为空(file_count=0)",
            ));
        }
        let share_status = string(&value, &["share_status"]);
        if !share_status.is_empty()
            && !matches!(share_status, "enabled" | "normal")
            && contains_any(
                share_status,
                &[
                    "forbidden",
                    "cancel",
                    "expired",
                    "illegal",
                    "invalid",
                    "disabled",
                ],
            )
        {
            return Ok(CheckEvaluation::new(CheckState::Bad, "链接失效"));
        }
        if status.is_success()
            && (!string(&value, &["share_name", "share_title"]).is_empty()
                || number(&value, &["file_count"]) > 0)
        {
            Ok(CheckEvaluation::new(CheckState::Ok, "链接有效"))
        } else {
            Ok(CheckEvaluation::new(
                CheckState::Uncertain,
                format!("HTTP状态码: {}", status.as_u16()),
            ))
        }
    }
}
