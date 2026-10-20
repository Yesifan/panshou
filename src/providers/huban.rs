//! 虎斑 mirrors use the same upstream MacCMS protocol as original erxiao.
use super::erxiao::MacCms;
use crate::{
    core::{ProviderError, SearchResult},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use url::Url;
pub struct HubanProvider {
    inner: MacCms,
}
impl Default for HubanProvider {
    fn default() -> Self {
        Self::new(
            [
                "http://121.205.88.174:16969/",
                "http://38.76.197.172:16969/",
                "http://xhban.xyz:20720/",
            ]
            .iter()
            .map(|s| Url::parse(s).unwrap())
            .collect(),
        )
    }
}
impl HubanProvider {
    pub fn new(bases: Vec<Url>) -> Self {
        Self {
            inner: MacCms::new("huban", bases),
        }
    }
}
#[async_trait]
impl Provider for HubanProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("huban", 2, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        q: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        self.inner.search(ctx, q).await
    }
}
