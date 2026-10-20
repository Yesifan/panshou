//! 盘尊: paged discussions → first post → optional a.7u9.cn redirect.
use super::markup::{get, matches, time};
use crate::{
    core::{CloudType, Link, ProviderError, SearchResult},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::Html;
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
    time::Duration,
};
use url::Url;
#[derive(Clone, Debug)]
pub struct Panzun {
    base: Url,
    short_origin: Option<Url>,
}
impl Default for Panzun {
    fn default() -> Self {
        Self::new(Url::parse("https://www.panzun.cc/").unwrap())
    }
}
#[derive(Deserialize)]
struct Resource {
    id: String,
    #[serde(default, rename = "type")]
    kind: String,
    attributes: Value,
    #[serde(default)]
    relationships: Value,
}
#[derive(Deserialize)]
struct Search {
    data: Vec<Resource>,
    #[serde(default)]
    links: HashMap<String, String>,
}
#[derive(Deserialize)]
struct Detail {
    data: Resource,
    included: Vec<Resource>,
}
static SHORT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?://a\.7u9\.cn/s/[A-Za-z0-9]+").unwrap());
static REAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://(?:pan\.quark\.cn/s/[0-9A-Za-z]+|drive\.uc\.cn/s/[0-9A-Za-z]+|pan\.baidu\.com/s/[0-9A-Za-z_\-]+(?:\?pwd=[0-9A-Za-z]+)?|cloud\.189\.cn/t/[0-9A-Za-z]+|(?:www\.)?aliyundrive\.com/s/[0-9A-Za-z]+|(?:www\.)?alipan\.com/s/[0-9A-Za-z]+|(?:www\.)?guangyapan\.com/s/[0-9A-Za-z_\-]+|115\.com/s/[0-9A-Za-z]+|pan\.xunlei\.com/s/[0-9A-Za-z_\-]+|www\.123684\.com/s/[0-9A-Za-z]+|www\.123865\.com/s/[0-9A-Za-z]+|www\.123912\.com/s/[0-9A-Za-z]+|www\.123pan\.com/s/[0-9A-Za-z]+)").unwrap()
});
impl Panzun {
    pub fn new(base: Url) -> Self {
        Self {
            base,
            short_origin: None,
        }
    }
    pub fn with_short_origin(mut self, url: Url) -> Self {
        self.short_origin = Some(url);
        self
    }
    async fn short(&self, ctx: &SearchContext, raw: &str) -> Result<String, ProviderError> {
        if !SHORT.is_match(raw) {
            return Ok(raw.to_owned());
        }
        let original = Url::parse(raw).map_err(|e| ProviderError::Protocol(e.to_string()))?;
        let url = if let Some(base) = &self.short_origin {
            base.join(original.path())
                .map_err(|e| ProviderError::Protocol(e.to_string()))?
        } else {
            original
        };
        let response = crate::http::execute(
            ctx.client
                .get(url)
                .header("user-agent", "python-requests/2.31.0")
                .header("accept", "text/html,*/*")
                .header("accept-encoding", "identity")
                .timeout(ctx.request_timeout(Duration::from_secs(30))),
        )
        .await
        .map_err(|e| ProviderError::Network(e.to_string()))?;
        let result = response
            .headers()
            .get("location")
            .and_then(|s| s.to_str().ok())
            .map(str::to_owned)
            .unwrap_or_else(|| response.url().to_string());
        if !REAL.is_match(&result) {
            return Err(ProviderError::Protocol(
                "panzun short link did not resolve to a supported share".into(),
            ));
        }
        Ok(result)
    }
    async fn detail(
        &self,
        ctx: &SearchContext,
        item: Resource,
        query: &str,
    ) -> Result<Option<SearchResult>, ProviderError> {
        let id = item.id.trim();
        let title = item.attributes["title"].as_str().unwrap_or("").trim();
        if id.is_empty() || title.is_empty() {
            return Ok(None);
        }
        let body = get(
            ctx,
            self.base
                .join(&format!("api/discussions/{id}"))
                .map_err(|e| ProviderError::Protocol(e.to_string()))?,
            self.base.as_str(),
            30,
            1,
        )
        .await?;
        let detail: Detail =
            serde_json::from_str(&body).map_err(|e| ProviderError::Parse(e.to_string()))?;
        let map = detail
            .included
            .into_iter()
            .map(|r| (format!("{}:{}", r.kind, r.id), r.attributes))
            .collect::<HashMap<_, _>>();
        let post = detail
            .data
            .relationships
            .pointer("/posts/data/0/id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ProviderError::Protocol("panzun detail omitted first post relationship".into())
            })?;
        let html = map
            .get(&format!("posts:{post}"))
            .and_then(|v| v["contentHtml"].as_str())
            .ok_or_else(|| ProviderError::Protocol("panzun detail omitted first post".into()))?;
        let mut r = SearchResult::provider(format!("panzun-{id}"), "panzun");
        r.title = title.into();
        r.content = Html::parse_fragment(html)
            .root_element()
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if !matches(&format!("{} {}", r.title, r.content), query) {
            return Ok(None);
        }
        let mut seen = HashSet::new();
        let mut failure = None;
        for raw in SHORT
            .find_iter(html)
            .chain(REAL.find_iter(html))
            .map(|m| m.as_str())
        {
            match self.short(ctx, raw).await {
                Ok(real) => {
                    if !seen.insert(real.clone()) {
                        continue;
                    }
                    let mut l = Link::new(&real);
                    if real.contains("www.123865.com/") || real.contains("www.123912.com/") {
                        l.cloud_type = CloudType::Pan123;
                    }
                    l.work_title = Some(r.title.clone());
                    r.links.push(l);
                }
                Err(e) => failure = Some(e),
            }
        }
        r.datetime = detail.data.attributes["createdAt"].as_str().and_then(time);
        for tag in detail
            .data
            .relationships
            .pointer("/tags/data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(name) = tag["id"]
                .as_str()
                .and_then(|id| map.get(&format!("tags:{id}")))
                .and_then(|v| v["name"].as_str())
                .filter(|s| !s.is_empty())
            {
                r.tags.push(name.into());
            }
        }
        if !r.links.is_empty() {
            ctx.publish(vec![r.clone()]);
        }
        if let Some(e) = failure {
            return Err(e);
        }
        Ok((!r.links.is_empty()).then_some(r))
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut failure = None;
        for page in 0..3 {
            if page > 0 {
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            let mut url = self.base.join("api/discussions").unwrap();
            url.query_pairs_mut()
                .append_pair("filter[q]", query)
                .append_pair("page[offset]", &(page * 20).to_string());
            let body = match get(ctx, url, self.base.as_str(), 30, 1).await {
                Ok(s) => s,
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            };
            let search: Search = match serde_json::from_str(&body) {
                Ok(v) => v,
                Err(e) => {
                    failure = Some(ProviderError::Parse(e.to_string()));
                    break;
                }
            };
            let mut jobs = stream::iter(
                search
                    .data
                    .into_iter()
                    .filter(|i| seen.insert(i.id.clone()))
                    .map(|i| self.detail(ctx, i, query)),
            )
            .buffered(4);
            while let Some(r) = jobs.next().await {
                match r {
                    Ok(Some(r)) => out.push(r),
                    Ok(None) => {}
                    Err(e) => failure = Some(e),
                }
            }
            if !search.links.get("next").is_some_and(|s| !s.is_empty()) {
                break;
            }
        }
        if let Some(e) = failure {
            return Err(e);
        }
        Ok(out)
    }
}
#[async_trait]
impl Provider for Panzun {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("panzun", 2, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        tokio::time::timeout(ctx.remaining(), self.run(ctx, query))
            .await
            .map_err(|_| ProviderError::Timeout)?
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn full_chain_and_empty_malformed() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{path, query_param},
        };
        let s = MockServer::start().await;
        Mock::given(path("/api/discussions")).and(query_param("filter[q]","Film")).and(query_param("page[offset]","0")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data":[{"type":"discussions","id":"1","attributes":{"title":"Film"}}],"links":{}}))).expect(1).mount(&s).await;
        Mock::given(path("/api/discussions/1")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data":{"type":"discussions","id":"1","attributes":{"createdAt":"2026-10-12T00:00:00Z"},"relationships":{"posts":{"data":[{"id":"2"}]},"tags":{"data":[{"id":"3"}]}}},"included":[{"type":"posts","id":"2","attributes":{"contentHtml":"Film <a href='https://a.7u9.cn/s/xyz'>download</a>"}},{"type":"tags","id":"3","attributes":{"name":"HD"}}]}))).expect(1).mount(&s).await;
        Mock::given(path("/s/xyz"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "https://pan.quark.cn/s/abc"),
            )
            .expect(1)
            .mount(&s)
            .await;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let ctx = SearchContext::new(client, Duration::from_secs(5));
        let p = Panzun::new(Url::parse(&s.uri()).unwrap())
            .with_short_origin(Url::parse(&s.uri()).unwrap());
        let r = p.search(&ctx, "Film").await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].links[0].url, "https://pan.quark.cn/s/abc");
        assert_eq!(r[0].tags, ["HD"]);
        assert!(
            serde_json::from_str::<Search>("{\"data\":[]}")
                .unwrap()
                .data
                .is_empty()
        );
        assert!(serde_json::from_str::<Search>("{}").is_err());
    }
}
