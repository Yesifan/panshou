use super::engine::CheckEvaluation;
use super::mobile_crypto::{decrypt, encrypt};
use super::protocol::{contains_any, json, string};
use super::{CheckCloudType, CheckContext, CheckError, CheckItem, CheckState, LinkChecker};
use async_trait::async_trait;
use regex::Regex;
pub struct MobileChecker {
    endpoint: String,
}
impl Default for MobileChecker {
    fn default() -> Self {
        Self{endpoint:"https://share-kd-njs.yun.139.com/yun-share/richlifeApp/devapp/IOutLink/getOutLinkInfoV6".into()}
    }
}
fn share_id(raw: &str) -> Option<String> {
    for p in [
        r"https?://(?:www\.)?yun\.139\.com/shareweb/#/w/i/([^&/?#]+)",
        r"https?://(?:www\.)?caiyun\.139\.com/w/i/([^&/?#]+)",
        r"https?://(?:www\.)?caiyun\.139\.com/m/i\?([^&/?#]+)",
        r"https?://caiyun\.feixin\.10086\.cn/([^&/?#]+)",
    ] {
        if let Some(v) = Regex::new(p)
            .unwrap()
            .captures(raw)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_owned())
        {
            return Some(v);
        }
    }
    None
}
#[async_trait]
impl LinkChecker for MobileChecker {
    fn cloud_type(&self) -> CheckCloudType {
        CheckCloudType::Mobile
    }
    async fn check_normalized(
        &self,
        ctx: &CheckContext,
        item: &CheckItem,
        n: &str,
    ) -> Result<CheckEvaluation, CheckError> {
        let id = share_id(n).ok_or_else(|| CheckError::Protocol("missing share id".into()))?;
        let raw=serde_json::to_vec(&serde_json::json!({"getOutLinkInfoReq":{"account":"","linkID":id,"passwd":item.password.as_deref().unwrap_or(""),"caSrt":1,"coSrt":1,"srtDr":0,"bNum":1,"pCaID":"root","eNum":200},"commonAccountInfo":{"account":"","accountType":1}})).map_err(|e|CheckError::Parse(e.to_string()))?;
        let encrypted = encrypt(&raw);
        let body=ctx.client.post(&self.endpoint).header("hcy-cool-flag","1").header("x-deviceinfo","||3|12.27.0|chrome|131.0.0.0|5c7c68368f048245e1ce47f1c0f8f2d0||windows 10|1536X695|zh-CN|||").json(&encrypted).send().await?.bytes().await?;
        let decrypted = decrypt(&String::from_utf8_lossy(&body))
            .map_err(|e| CheckError::Parse(e.to_string()))?;
        let v = json(&decrypted)?;
        let code = string(&v, &["resultCode"]);
        let desc = string(&v, &["desc"]);
        Ok(
            if code == "0" && v.get("data").is_some_and(|v| !v.is_null()) {
                CheckEvaluation::new(CheckState::Ok, "链接有效")
            } else if contains_any(desc, &["提取码", "密码", "访问码"]) {
                CheckEvaluation::new(CheckState::Locked, desc)
            } else if contains_any(desc, &["失效", "不存在", "过期", "取消"]) {
                CheckEvaluation::new(CheckState::Bad, desc)
            } else if !desc.is_empty() {
                CheckEvaluation::new(CheckState::Uncertain, desc)
            } else if !code.is_empty() {
                CheckEvaluation::new(CheckState::Bad, format!("错误码: {code}"))
            } else {
                CheckEvaluation::new(CheckState::Uncertain, "无法确认链接状态")
            },
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracts_all_forms() {
        for (url, id) in [
            ("https://yun.139.com/shareweb/#/w/i/ABC", "ABC"),
            ("https://caiyun.139.com/w/i/DEF", "DEF"),
            ("https://caiyun.139.com/m/i?GHI", "GHI"),
            ("https://caiyun.feixin.10086.cn/JKL", "JKL"),
        ] {
            assert_eq!(share_id(url).as_deref(), Some(id));
        }
    }
}
