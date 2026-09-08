use super::engine::CheckEvaluation;
use super::protocol::{contains_any, json, number, string};
use super::{CheckCloudType, CheckContext, CheckError, CheckItem, CheckState, LinkChecker};
use async_trait::async_trait;
use regex::Regex;
use url::Url;
pub struct TianyiChecker {
    endpoint: String,
}
impl Default for TianyiChecker {
    fn default() -> Self {
        Self {
            endpoint: "https://cloud.189.cn/api/open/share/getShareInfoByCodeV2.action".into(),
        }
    }
}
const BAD_CODES: [&str; 6] = [
    "ShareInfoNotFound",
    "ShareNotFound",
    "FileNotFound",
    "ShareExpiredError",
    "ShareAuditNotPass",
    "FolderNotFound",
];
fn info(raw: &str, fallback: Option<&str>) -> Option<(String, String)> {
    let u = Url::parse(raw).ok()?;
    let mut code = Regex::new(r"(?i)cloud\.189\.cn/t/([^/?#\s（]+)")
        .expect("static Tianyi URL regex")
        .captures(raw)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned())
        .or_else(|| {
            u.query_pairs()
                .find(|(k, _)| k == "code")
                .map(|(_, v)| v.into_owned())
        })
        .or_else(|| {
            u.fragment()
                .and_then(|f| f.strip_prefix("/t/"))
                .map(str::to_owned)
        })?;
    if let Some(i) = code.find('/') {
        code.truncate(i);
    }
    let embedded = Regex::new(r"（访问码[：:]\s*([A-Za-z0-9]+)）")
        .unwrap()
        .captures(raw)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_owned());
    Some((
        code,
        embedded
            .or_else(|| fallback.map(str::to_owned))
            .unwrap_or_default(),
    ))
}
fn bad_code(text: &str) -> Option<&'static str> {
    BAD_CODES.into_iter().find(|c| text.contains(c))
}
#[async_trait]
impl LinkChecker for TianyiChecker {
    fn cloud_type(&self) -> CheckCloudType {
        CheckCloudType::Tianyi
    }
    async fn check_normalized(
        &self,
        ctx: &CheckContext,
        item: &CheckItem,
        n: &str,
    ) -> Result<CheckEvaluation, CheckError> {
        let (code, pwd) = info(n, item.password.as_deref())
            .ok_or_else(|| CheckError::Protocol("missing share code".into()))?;
        let param = if pwd.is_empty() {
            code
        } else {
            format!("{code}（访问码：{pwd}）")
        };
        let r = ctx
            .client
            .get(&self.endpoint)
            .query(&[
                ("noCache", format!("{}", rand::random::<f64>())),
                ("shareCode", param),
            ])
            .header("referer", n)
            .header("sign-type", "1")
            .send()
            .await?;
        let body = r.bytes().await?;
        let text = String::from_utf8_lossy(&body);
        if text.contains("<shareVO>")
            && (text.contains("<shareId>")
                || text.contains("<fileName>")
                || text.contains("<needAccessCode>1</needAccessCode>"))
        {
            return Ok(CheckEvaluation::new(CheckState::Ok, "链接有效"));
        }
        if let Some(code) = bad_code(&text) {
            return Ok(CheckEvaluation::new(CheckState::Bad, map_error(code)));
        }
        if contains_any(
            &text,
            &[
                "ErrorAccessCode",
                "NeedAccessCode",
                "访问码",
                "提取码",
                "密码",
            ],
        ) {
            return Ok(CheckEvaluation::new(CheckState::Locked, "需要访问码"));
        }
        if let Ok(v) = json(&body) {
            if number(&v, &["shareId"]) > 0
                || !string(&v, &["fileName"]).is_empty()
                || number(&v, &["needAccessCode"]) == 1
            {
                return Ok(CheckEvaluation::new(CheckState::Ok, "链接有效"));
            }
            let msg = string(&v, &["res_message"]);
            if contains_any(msg, &["访问码", "提取码", "密码"]) {
                return Ok(CheckEvaluation::new(CheckState::Locked, msg));
            }
        }
        Ok(CheckEvaluation::new(
            CheckState::Uncertain,
            "无法确认链接状态",
        ))
    }
}
fn map_error(c: &str) -> &'static str {
    match c {
        "ShareInfoNotFound" => "分享信息不存在",
        "ShareNotFound" => "分享链接不存在",
        "FileNotFound" => "分享文件不存在",
        "ShareExpiredError" => "分享链接已过期",
        "ShareAuditNotPass" => "分享因审核未通过已失效",
        "FolderNotFound" => "分享文件夹不存在",
        _ => "链接失效",
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracts_code_and_embedded_password() {
        assert_eq!(
            info("https://cloud.189.cn/t/abc（访问码：9xY1）", None).unwrap(),
            ("abc".into(), "9xY1".into())
        );
        assert_eq!(
            info("https://cloud.189.cn/web/share?code=xyz", Some("p")).unwrap(),
            ("xyz".into(), "p".into())
        );
    }
}
