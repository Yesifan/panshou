use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use async_trait::async_trait;
use serde::Deserialize;
use std::{collections::HashSet, time::Duration};
use url::Url;
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://www.zlxapp.top/").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Zlxapp {
    endpoints: Endpoints,
}
impl Zlxapp {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Category {
    name: String,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct RawLink {
    url: String,
    code: String,
    target_name: String,
    source_title: String,
    status: i32,
    is_delete: i32,
}
#[derive(Deserialize)]
struct Item {
    id: i64,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    times: String,
    #[serde(default)]
    category: Category,
    #[serde(default)]
    tag_names: Vec<String>,
    #[serde(default)]
    links: Vec<RawLink>,
}
fn clean(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn parse(body: &str) -> Result<Vec<SearchResult>, ProviderError> {
    let pos = body
        .find("const listItemsData = ")
        .ok_or_else(|| ProviderError::Parse("zlxapp listItemsData missing".into()))?
        + "const listItemsData = ".len();
    let tail = &body[pos..];
    let start = tail
        .find('[')
        .ok_or_else(|| ProviderError::Parse("zlxapp list array missing".into()))?;
    let items: Vec<Item> = serde_json::Deserializer::from_str(&tail[start..])
        .into_iter()
        .next()
        .ok_or_else(|| ProviderError::Parse("empty zlxapp script".into()))?
        .map_err(|e| ProviderError::Parse(e.to_string()))?;
    let mut rows = vec![];
    let mut results = HashSet::new();
    for mut item in items {
        let title = clean(&item.title);
        if item.id <= 0 || title.is_empty() {
            continue;
        }
        if item.links.is_empty() && !item.url.trim().is_empty() {
            item.links.push(RawLink {
                url: item.url,
                code: item.code,
                source_title: title.clone(),
                status: 1,
                ..Default::default()
            })
        }
        let mut links = vec![];
        let mut seen = HashSet::new();
        for raw in item.links {
            if raw.is_delete != 0 || raw.status < 0 {
                continue;
            }
            let Ok(url) = Url::parse(raw.url.trim()) else {
                continue;
            };
            if !matches!(url.scheme(), "http" | "https") {
                continue;
            }
            let mut link = Link::new(url.to_string());
            if !matches!(
                link.cloud_type,
                CloudType::Quark
                    | CloudType::Uc
                    | CloudType::Baidu
                    | CloudType::Aliyun
                    | CloudType::Xunlei
                    | CloudType::Guangya
            ) {
                continue;
            }
            link.password = (!raw.code.trim().is_empty())
                .then(|| raw.code.trim().into())
                .or_else(|| password(&link.url));
            let work = [raw.target_name, raw.source_title, title.clone()]
                .into_iter()
                .map(|s| clean(&s))
                .find(|s| !s.is_empty())
                .unwrap();
            link.work_title = Some(work);
            if seen.insert((link.url.clone(), link.password.clone())) {
                links.push(link)
            }
        }
        if links.is_empty() {
            continue;
        }
        let key = links
            .iter()
            .map(|l| (l.url.clone(), l.password.clone()))
            .collect::<Vec<_>>();
        if !results.insert(key) {
            continue;
        }
        let mut row = SearchResult::provider(format!("zlxapp-{}", item.id), "zlxapp");
        row.title = title;
        row.content = clean(&item.description);
        if row.content.is_empty() {
            row.content = row.title.clone()
        }
        row.datetime = date(item.times.trim()).or_else(|| Some(chrono::Utc::now()));
        row.tags = item.tag_names;
        if !item.category.name.is_empty() && !row.tags.contains(&item.category.name) {
            row.tags.push(item.category.name)
        }
        row.links = links;
        rows.push(row)
    }
    Ok(rows)
}
#[async_trait]
impl Provider for Zlxapp {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("zlxapp", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let query = clean(query);
        if query.is_empty() {
            return Ok(vec![]);
        }
        let escaped =
            percent_encoding::utf8_percent_encode(&query, percent_encoding::NON_ALPHANUMERIC);
        let url = self
            .endpoints
            .base
            .join(&format!("/s/{escaped}.html"))
            .unwrap();
        let body=tokio::time::timeout(ctx.remaining().min(Duration::from_secs(20)),request(||ctx.client.get(url.clone()).header("referer",self.endpoints.base.as_str()).header("accept","text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8").header("cache-control","no-cache"),3,Duration::from_millis(200),2*1024*1024)).await.map_err(|_|ProviderError::Timeout)??;
        complete(ctx, parse(&String::from_utf8_lossy(&body)))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn script_responses() {
        let rows=parse(r#"<script>const listItemsData = [{"id":9007199254740993,"title":"movie ] title","url":"https://pan.quark.cn/s/a","code":"abcd","tag_names":["4K"],"category":{"name":"movie"}}]; otherJS();</script>"#).unwrap();
        assert_eq!(rows[0].id, "zlxapp-9007199254740993");
        assert_eq!(rows[0].links[0].password.as_deref(), Some("abcd"));
        assert_eq!(rows[0].tags, vec!["4K", "movie"]);
        assert!(parse("const listItemsData = [];").unwrap().is_empty());
        assert!(parse("bad").is_err());
        assert!(parse("const listItemsData = [").is_err());
    }
    #[tokio::test]
    async fn cancellation_during_backoff() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::any};
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(503))
            .expect(1)
            .mount(&server)
            .await;
        let p = Zlxapp::new(Endpoints {
            base: Url::parse(&server.uri()).unwrap(),
        });
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_millis(80));
        assert_eq!(p.search(&ctx, "movie").await, Err(ProviderError::Timeout));
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}
