use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{Link, ProviderError, SearchResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use url::Url;
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub search: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            search: Url::parse("https://v.funletu.com/search").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Qupansou {
    endpoints: Endpoints,
}
impl Qupansou {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
#[derive(Deserialize)]
struct Response {
    status: i32,
    #[serde(default)]
    data: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    id: i64,
    title: String,
    url: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    filetype: String,
    #[serde(default)]
    size: String,
    #[serde(default)]
    updatetime: String,
}
fn parse(body: &[u8]) -> Result<Vec<SearchResult>, ProviderError> {
    let r: Response =
        serde_json::from_slice(body).map_err(|e| ProviderError::Parse(e.to_string()))?;
    if r.status != 200 {
        return Err(ProviderError::Protocol(format!(
            "qupansou API status {}",
            r.status
        )));
    }
    Ok(r.data
        .into_iter()
        .filter(|i| !i.url.trim().is_empty())
        .map(|i| {
            let mut row = SearchResult::provider(format!("qupansou-{}", i.id), "qupansou");
            row.title = i.title;
            for tag in ["em", "b", "strong", "i"] {
                row.title = row
                    .title
                    .replace(&format!("<{tag}>"), "")
                    .replace(&format!("</{tag}>"), "")
            }
            row.title = row.title.trim().into();
            row.content = format!(
                "类别: {}, 文件类型: {}, 大小: {}",
                i.category, i.filetype, i.size
            );
            row.datetime = date(&i.updatetime);
            row.links = vec![Link::new(i.url)];
            row
        })
        .collect())
}
#[async_trait]
impl Provider for Qupansou {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("qupansou", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let payload = json!({"style":"get","datasrc":"search","query":{"id":"","datetime":"","courseid":1,"categoryid":"","filetypeid":"","filetype":"","reportid":"","validid":"","searchtext":query},"page":{"pageSize":1000,"pageIndex":1},"order":{"prop":"sort","order":"desc"},"message":"请求资源列表数据"});
        let result = parse(
            &request(
                || {
                    ctx.client
                        .post(self.endpoints.search.clone())
                        .timeout(ctx.timeout.min(std::time::Duration::from_secs(6)))
                        .header("referer", "https://pan.funletu.com/")
                        .json(&payload)
                },
                1,
                std::time::Duration::ZERO,
                BODY_LIMIT,
            )
            .await?,
        );
        complete(ctx, result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn responses() {
        let r=parse(br#"{"status":200,"data":[{"id":9007199254740993,"title":"<em>test</em>","url":"https://pan.quark.cn/s/a","updatetime":"2025-07-05 00:31:38"},{"id":1,"title":"skip","url":""}]}"#).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, "qupansou-9007199254740993");
        assert_eq!(r[0].title, "test");
        assert!(r[0].datetime.is_some());
        assert!(r[0].links[0].password.is_none());
        assert!(parse(br#"{"status":200,"data":[]}"#).unwrap().is_empty());
        assert!(parse(br#"{"status":500}"#).is_err());
        assert!(parse(b"bad").is_err());
    }
}
