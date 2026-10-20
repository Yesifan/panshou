use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use async_trait::async_trait;
use serde::Deserialize;
use url::Url;
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://squark.cc.cd/").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Quarkres {
    endpoints: Endpoints,
}
impl Quarkres {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
#[derive(Deserialize)]
struct Response {
    code: i32,
    #[serde(default)]
    data: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    unique_id: String,
    title: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    datetime: Option<chrono::DateTime<chrono::Utc>>,
    links: Vec<ApiLink>,
}
#[derive(Deserialize)]
struct ApiLink {
    url: String,
    #[serde(default)]
    password: String,
}
fn parse(body: &[u8]) -> Result<Vec<SearchResult>, ProviderError> {
    let response: Response =
        serde_json::from_slice(body).map_err(|e| ProviderError::Parse(e.to_string()))?;
    if response.code != 200 {
        return Err(ProviderError::Protocol(format!(
            "quarkres API code {}",
            response.code
        )));
    }
    Ok(response
        .data
        .into_iter()
        .filter_map(|item| {
            let links = item
                .links
                .into_iter()
                .filter_map(|raw| {
                    let mut link = Link::new(raw.url);
                    if link.cloud_type != CloudType::Quark {
                        return None;
                    }
                    link.password = (!raw.password.is_empty()).then_some(raw.password);
                    link.datetime = item.datetime;
                    Some(link)
                })
                .collect::<Vec<_>>();
            if links.is_empty() {
                return None;
            }
            let mut row = SearchResult::provider(item.unique_id, "quarkres");
            row.title = item.title;
            row.content = item.content;
            row.datetime = item.datetime;
            row.links = links;
            Some(row)
        })
        .collect())
}
#[async_trait]
impl Provider for Quarkres {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("quarkres", 2, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut url = self.endpoints.base.join("api/search").unwrap();
        url.query_pairs_mut().append_pair("kw", query);
        let body = request(
            || {
                ctx.client
                    .get(url.clone())
                    .timeout(ctx.timeout)
                    .header("referer", self.endpoints.base.as_str())
                    .header("accept", "application/json, text/plain, */*")
                    .header("accept-language", "zh-CN,zh;q=0.9")
                    .header("user-agent", "Mozilla/5.0 Chrome/126.0.0.0 Safari/537.36")
            },
            1,
            std::time::Duration::ZERO,
            BODY_LIMIT,
        )
        .await?;
        complete(ctx, parse(&body))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn responses() {
        let rows=parse(br#"{"code":200,"data":[{"unique_id":"1","title":"test","links":[{"url":"https://pan.quark.cn/s/a","password":"abcd"},{"url":"https://evil.test/?u=pan.quark.cn"}]}]}"#).unwrap();
        assert_eq!(rows[0].links.len(), 1);
        assert_eq!(rows[0].links[0].password.as_deref(), Some("abcd"));
        assert!(parse(br#"{"code":200,"data":[]}"#).unwrap().is_empty());
        assert!(parse(br#"{"code":500}"#).is_err());
        assert!(parse(b"bad").is_err());
    }
}
