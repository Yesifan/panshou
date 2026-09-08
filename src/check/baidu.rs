use super::engine::CheckEvaluation;
use super::protocol::{json, number, string};
use super::{CheckCloudType, CheckContext, CheckError, CheckItem, CheckState, LinkChecker};
use async_trait::async_trait;
use url::Url;
pub struct BaiduChecker {
    verify: String,
    list: String,
}
impl Default for BaiduChecker {
    fn default() -> Self {
        Self {
            verify: "https://pan.baidu.com/share/verify".into(),
            list: "https://pan.baidu.com/share/list".into(),
        }
    }
}
fn info(raw: &str) -> Option<(String, String, String)> {
    let u = Url::parse(raw).ok()?;
    let pwd = u
        .query_pairs()
        .find(|(k, _)| k == "pwd")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default();
    let id = if let Some(v) = u.path().strip_prefix("/s/") {
        v.to_owned()
    } else if u.path() == "/share/init" {
        u.query_pairs()
            .find(|(k, _)| k == "surl")
            .map(|(_, v)| v.into_owned())?
    } else {
        return None;
    };
    let short = id.strip_prefix('1').unwrap_or(&id).to_owned();
    Some((id, short, pwd))
}
#[async_trait]
impl LinkChecker for BaiduChecker {
    fn cloud_type(&self) -> CheckCloudType {
        CheckCloudType::Baidu
    }
    async fn check_normalized(
        &self,
        ctx: &CheckContext,
        _: &CheckItem,
        n: &str,
    ) -> Result<CheckEvaluation, CheckError> {
        let (_, short, pwd) =
            info(n).ok_or_else(|| CheckError::Protocol("missing share id".into()))?;
        let mut bdclnd = None;
        if !pwd.is_empty() {
            let v = json(
                &ctx.client
                    .post(&self.verify)
                    .query(&[("surl", short.as_str()), ("pwd", pwd.as_str())])
                    .header("referer", n)
                    .form(&[("pwd", pwd.as_str()), ("vcode", ""), ("vcode_str", "")])
                    .send()
                    .await?
                    .bytes()
                    .await?,
            )?;
            match number(&v, &["errno"]) {
                0 => bdclnd = Some(string(&v, &["randsk"]).to_owned()),
                -9 | -12 => {
                    return Ok(CheckEvaluation::new(CheckState::Locked, "提取码错误或缺失"));
                }
                _ => {
                    return Ok(CheckEvaluation::new(
                        CheckState::Uncertain,
                        string(&v, &["errmsg"]),
                    ));
                }
            }
        }
        let mut req = ctx
            .client
            .get(&self.list)
            .query(&[
                ("web", "1"),
                ("page", "1"),
                ("num", "20"),
                ("order", "time"),
                ("desc", "1"),
                ("showempty", "0"),
                ("shorturl", short.as_str()),
                ("root", "1"),
                ("clienttype", "0"),
            ])
            .header("referer", n);
        if let Some(c) = bdclnd {
            req = req.header("cookie", format!("BDCLND={c}"));
        }
        let v = json(&req.send().await?.bytes().await?)?;
        Ok(match number(&v, &["errno"]) {
            0 if v
                .get("list")
                .and_then(|v| v.as_array())
                .is_some_and(|l| !l.is_empty()) =>
            {
                CheckEvaluation::new(CheckState::Ok, "链接有效")
            }
            0 => CheckEvaluation::new(CheckState::Bad, "链接失效"),
            -9 | -12 => CheckEvaluation::new(CheckState::Locked, "需要提取码"),
            -7 | 105 | 115 | 117 | 145 => CheckEvaluation::new(CheckState::Bad, "链接失效"),
            _ => CheckEvaluation::new(CheckState::Uncertain, string(&v, &["errmsg"])),
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracts_forms() {
        assert_eq!(
            info("https://pan.baidu.com/s/1abc?pwd=1234").unwrap(),
            ("1abc".into(), "abc".into(), "1234".into())
        );
        assert_eq!(
            info("https://pan.baidu.com/share/init?surl=1def")
                .unwrap()
                .1,
            "def"
        );
    }
}
