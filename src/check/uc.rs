use super::engine::CheckEvaluation;
use super::protocol::contains_any;
use super::{CheckCloudType, CheckContext, CheckError, CheckItem, CheckState, LinkChecker};
use async_trait::async_trait;
pub struct UcChecker;
impl Default for UcChecker {
    fn default() -> Self {
        Self
    }
}
#[async_trait]
impl LinkChecker for UcChecker {
    fn cloud_type(&self) -> CheckCloudType {
        CheckCloudType::Uc
    }
    async fn check_normalized(
        &self,
        ctx: &CheckContext,
        _: &CheckItem,
        n: &str,
    ) -> Result<CheckEvaluation, CheckError> {
        let r=ctx.client.get(n).header("user-agent","Mozilla/5.0 (Linux; Android 10; Mobile) AppleWebKit/537.36 Chrome/124.0 Safari/537.36").send().await?;
        if r.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(CheckEvaluation::new(CheckState::Bad, "链接失效"));
        }
        let t = r.text().await?;
        Ok(
            if contains_any(&t, &["失效", "不存在", "违规", "删除", "已过期", "被取消"])
            {
                CheckEvaluation::new(CheckState::Bad, "链接失效")
            } else if contains_any(&t, &["提取码", "访问码", "请输入密码"]) {
                CheckEvaluation::new(CheckState::Locked, "需要提取码")
            } else if contains_any(&t, &["文件", "分享", "drive.uc.cn"]) {
                CheckEvaluation::new(CheckState::Ok, "链接有效")
            } else {
                CheckEvaluation::new(CheckState::Uncertain, "无法确认链接状态")
            },
        )
    }
}
