use super::engine::CheckEvaluation;
use super::protocol::{contains_any, json, number, string};
use super::{CheckCloudType, CheckContext, CheckError, CheckItem, CheckState, LinkChecker};
use async_trait::async_trait;
use regex::Regex;
use url::Url;
pub struct XunleiChecker {
    captcha: String,
    endpoint: String,
}
impl Default for XunleiChecker {
    fn default() -> Self {
        Self {
            captcha: "https://xluser-ssl.xunlei.com/v1/shield/captcha/init".into(),
            endpoint: "https://api-pan.xunlei.com/drive/v1/share".into(),
        }
    }
}
fn info(n: &str) -> Option<(String, String)> {
    let id = Regex::new(r"pan\.xunlei\.com/s/([^?/#]+)")
        .unwrap()
        .captures(n)?
        .get(1)?
        .as_str()
        .to_owned();
    let pwd = Url::parse(n)
        .ok()
        .and_then(|u| {
            u.query_pairs()
                .find(|(k, _)| k == "pwd")
                .map(|(_, v)| v.into_owned())
        })
        .unwrap_or_default();
    Some((id, pwd))
}
#[async_trait]
impl LinkChecker for XunleiChecker {
    fn cloud_type(&self) -> CheckCloudType {
        CheckCloudType::Xunlei
    }
    async fn check_normalized(
        &self,
        ctx: &CheckContext,
        _: &CheckItem,
        n: &str,
    ) -> Result<CheckEvaluation, CheckError> {
        let (id, pwd) = info(n).ok_or_else(|| CheckError::Protocol("missing share id".into()))?;
        let token = self.captcha(ctx).await.unwrap_or_default();
        let mut req = ctx
            .client
            .get(&self.endpoint)
            .query(&[
                ("share_id", id),
                ("pass_code", pwd),
                ("limit", "100".into()),
                ("pass_code_token", "".into()),
                ("page_token", "".into()),
                ("thumbnail_size", "SIZE_SMALL".into()),
            ])
            .header("x-client-id", "ZUBzD9J_XPXfn7f7")
            .header("x-device-id", "5505bd0cab8c9469b98e5891d9fb3e0d")
            .header("origin", "https://pan.xunlei.com");
        if !token.is_empty() {
            req = req.header("x-captcha-token", token);
        }
        let r = req.send().await?;
        if matches!(
            r.status(),
            reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::FORBIDDEN
        ) {
            return Ok(CheckEvaluation::new(CheckState::Bad, "链接失效"));
        }
        let v = json(&r.bytes().await?)?;
        let status = string(&v, &["share_status"]);
        let err = format!(
            "{} {} {}",
            string(&v, &["error"]),
            string(&v, &["error_description"]),
            string(&v, &["share_status_text"])
        );
        Ok(
            if status == "OK"
                || !string(&v, &["share_id", "share_name"]).is_empty()
                || number(&v, &["file_count"]) > 0
            {
                CheckEvaluation::new(CheckState::Ok, "链接有效")
            } else if contains_any(&err, &["pass_code", "提取码", "密码"]) {
                CheckEvaluation::new(CheckState::Locked, err)
            } else if !status.is_empty()
                || number(&v, &["error_code"]) != 0
                || !err.trim().is_empty()
            {
                if contains_any(
                    &err,
                    &["不存在", "失效", "过期", "not found", "deleted", "参数错误"],
                ) {
                    CheckEvaluation::new(CheckState::Bad, err)
                } else {
                    CheckEvaluation::new(CheckState::Uncertain, err)
                }
            } else {
                CheckEvaluation::new(CheckState::Uncertain, "无法确认链接状态")
            },
        )
    }
}
impl XunleiChecker {
    async fn captcha(&self, ctx: &CheckContext) -> Result<String, CheckError> {
        let device = "5505bd0cab8c9469b98e5891d9fb3e0d";
        let client = "ZUBzD9J_XPXfn7f7";
        let version = "1.10.0.2633";
        let package = "com.xunlei.browser";
        let (timestamp, sign) = signature(client, version, package, device);
        let v=json(&ctx.client.post(&self.captcha).header("x-device-id",device).header("x-client-id",client).header("x-client-version",version).json(&serde_json::json!({"action":"get:/drive/v1/share","captcha_token":"","client_id":client,"device_id":device,"meta":{"timestamp":timestamp,"captcha_sign":sign,"client_version":version,"package_name":package},"redirect_uri":"xlaccsdk01://xunlei.com/callback?state=harbor"})).send().await?.bytes().await?)?;
        if !string(&v, &["url"]).is_empty() {
            return Err(CheckError::Protocol("xunlei captcha required".into()));
        }
        Ok(string(&v, &["captcha_token"]).to_owned())
    }
}
fn signature(client: &str, version: &str, package: &str, device: &str) -> (String, String) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string();
    let mut content = format!("{client}{version}{package}{device}{ts}");
    for p in [
        "uWRwO7gPfdPB/0NfPtfQO+71",
        "F93x+qPluYy6jdgNpq+lwdH1ap6WOM+nfz8/V",
        "0HbpxvpXFsBK5CoTKam",
        "dQhzbhzFRcawnsZqRETT9AuPAJ+wTQso82mRv",
        "SAH98AmLZLRa6DB2u68sGhyiDh15guJpXhBzI",
        "unqfo7Z64Rie9RNHMOB",
        "7yxUdFADp3DOBvXdz0DPuKNVT35wqa5z0DEyEvf",
        "RBG",
        "ThTWPG5eC0UBqlbQ+04nZAptqGCdpv9o55A",
    ] {
        content = format!("{:x}", md5::compute(format!("{content}{p}")));
    }
    (ts, format!("1.{content}"))
}
