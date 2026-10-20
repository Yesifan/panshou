//! Quark4k uses Flarum's expanded includes and only returns Quark shares.
use super::pan666;
use crate::core::{ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use url::Url;
#[derive(Debug, Clone)]
pub struct Quark4k {
    endpoint: Url,
}
impl Default for Quark4k {
    fn default() -> Self {
        Self::new(Url::parse("https://quark4k.com/api/discussions").unwrap())
    }
}
impl Quark4k {
    pub fn new(endpoint: Url) -> Self {
        Self { endpoint }
    }
}
#[async_trait]
impl Provider for Quark4k {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("quark4k", 3, KeywordFilterMode::Provider)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        pan666::search_pages(ctx, &self.endpoint, "quark4k", query).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn success_empty_malformed() {
        let body=serde_json::json!({"data":[{"id":"1","attributes":{"title":"Film","createdAt":"2026-10-12T00:00:00Z"},"relationships":{"mostRelevantPost":{"data":{"id":"2"}}}}],"included":[{"type":"users","id":"3","attributes":{}},{"type":"posts","id":"2","attributes":{"contentHtml":"https://pan.quark.cn/s/abc<br>提取码：1234"}}]}).to_string();
        let r = pan666::parse(&body, "quark4k", "film").unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].links[0].password.as_deref(), Some("1234"));
        assert!(
            pan666::parse(r#"{"data":[],"included":[]}"#, "quark4k", "x")
                .unwrap()
                .is_empty()
        );
        assert!(pan666::parse("{}", "quark4k", "x").is_err());
    }
    #[tokio::test]
    async fn exact_retry_and_expanded_include() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{header, method, query_param},
        };
        let s = MockServer::start().await;
        let count = Arc::new(AtomicUsize::new(0));
        let n = count.clone();
        Mock::given(method("GET")).and(query_param("include","user,lastPostedUser,mostRelevantPost,mostRelevantPost.user,tags,tags.parent,firstPost")).and(header("referer","https://quark4k.com/")).respond_with(move |_:&wiremock::Request|if n.fetch_add(1,Ordering::SeqCst)<2 {ResponseTemplate::new(500)} else {ResponseTemplate::new(200).set_body_string(r#"{"data":[],"included":[]}"#)}).expect(3).mount(&s).await;
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
        assert!(
            pan666::fetch_page(&ctx, &Url::parse(&s.uri()).unwrap(), "quark4k", "x", 0)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
