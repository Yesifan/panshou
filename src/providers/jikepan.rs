use crate::core::{CloudType, Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use serde::Deserialize;
use url::Url;
#[derive(Clone, Debug)]
pub struct Jikepan {
    endpoint: Url,
    is_all: bool,
}
impl Default for Jikepan {
    fn default() -> Self {
        Self::new(Url::parse("https://api.jikepan.xyz/search").unwrap())
    }
}
impl Jikepan {
    pub fn new(endpoint: Url) -> Self {
        Self {
            endpoint,
            is_all: false,
        }
    }
    pub fn with_all_results(mut self, is_all: bool) -> Self {
        self.is_all = is_all;
        self
    }
}
#[derive(Deserialize)]
struct Response {
    msg: String,
    list: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    name: String,
    links: Vec<ApiLink>,
}
#[derive(Deserialize)]
struct ApiLink {
    service: String,
    link: String,
    #[serde(default)]
    pwd: String,
}
fn parse(body: &[u8]) -> Result<Vec<SearchResult>, ProviderError> {
    let response: Response =
        serde_json::from_slice(body).map_err(|e| ProviderError::Parse(e.to_string()))?;
    if response.msg != "success" {
        return Err(ProviderError::Protocol(
            "jikepan API did not report success".into(),
        ));
    }
    Ok(response
        .list
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let mut result = SearchResult::provider(format!("jikepan-{index}"), "jikepan");
            result.title = item.name;
            for api in item.links {
                let kind = match api.service.to_lowercase().as_str() {
                    "baidu" => CloudType::Baidu,
                    "aliyun" => CloudType::Aliyun,
                    "xunlei" => CloudType::Xunlei,
                    "quark" => CloudType::Quark,
                    "189cloud" => CloudType::Tianyi,
                    "115" => CloudType::One15,
                    "123" => CloudType::Pan123,
                    "pikpak" => CloudType::Pikpak,
                    "caiyun" => CloudType::Mobile,
                    "ed2k" => CloudType::Ed2k,
                    "magnet" => CloudType::Magnet,
                    "unknown" => continue,
                    _ => CloudType::Others,
                };
                let mut link = Link::new(api.link);
                if kind != CloudType::Others || link.cloud_type != CloudType::Uc {
                    link.cloud_type = kind;
                }
                if !api.pwd.is_empty() {
                    link.password = Some(api.pwd);
                }
                link.work_title = Some(result.title.clone());
                result.links.push(link);
            }
            (!result.links.is_empty()).then_some(result)
        })
        .collect())
}
#[async_trait]
impl Provider for Jikepan {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("jikepan", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        tokio::time::timeout(ctx.timeout, async {
            let response = crate::http::execute(
                ctx.client
                    .post(self.endpoint.clone())
                    .header("referer", "https://jikepan.xyz/")
                    .header(
                        "user-agent",
                        "Mozilla/5.0 Chrome/91.0.4472.124 Safari/537.36",
                    )
                    .json(&serde_json::json!({"name":query,"is_all":self.is_all})),
            )
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
            if response.status().as_u16() != 200 {
                return Err(ProviderError::Protocol(format!(
                    "jikepan HTTP {}",
                    response.status()
                )));
            }
            let body = crate::http::read_response(response, 16 << 20)
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?;
            parse(&body)
        })
        .await
        .map_err(|_| ProviderError::Timeout)?
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn success_empty_malformed() {
        let r=parse(br#"{"msg":"success","list":[{"name":"Film","links":[{"service":"189cloud","link":"https://cloud.189.cn/t/abc","pwd":"abcd"},{"service":"unknown","link":"x"}]}]}"#).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].links[0].cloud_type, CloudType::Tianyi);
        assert_eq!(r[0].links[0].password.as_deref(), Some("abcd"));
        assert!(parse(br#"{"msg":"success","list":[]}"#).unwrap().is_empty());
        assert!(parse(b"{}").is_err());
        assert!(parse(br#"{"msg":"failure","list":[]}"#).is_err());
    }
    #[tokio::test]
    async fn request_contract() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{body_json, header, method},
        };
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_json(serde_json::json!({"name":"film","is_all":false})))
            .and(header("referer", "https://jikepan.xyz/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"msg":"success","list":[]})),
            )
            .expect(1)
            .mount(&s)
            .await;
        let p = Jikepan::new(Url::parse(&s.uri()).unwrap());
        let c = SearchContext::new(reqwest::Client::new(), std::time::Duration::from_secs(2));
        assert!(p.search(&c, "film").await.unwrap().is_empty());
    }
}
