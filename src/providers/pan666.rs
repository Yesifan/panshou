//! Pan666's Flarum JSON:API search (two 50-discussion pages).
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use scraper::Html;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use url::Url;
#[derive(Debug, Clone)]
pub struct Pan666 {
    endpoint: Url,
}
impl Default for Pan666 {
    fn default() -> Self {
        Self::new(Url::parse("https://pan666.net/api/discussions").unwrap())
    }
}
impl Pan666 {
    pub fn new(endpoint: Url) -> Self {
        Self { endpoint }
    }
}
#[derive(Deserialize)]
struct Api {
    data: Vec<Discussion>,
    included: Vec<Post>,
}
#[derive(Deserialize)]
struct Discussion {
    id: String,
    attributes: DiscussionAttributes,
    relationships: Relationships,
}
#[derive(Deserialize)]
struct DiscussionAttributes {
    title: String,
    #[serde(rename = "createdAt")]
    created_at: String,
}
#[derive(Deserialize)]
struct Relationships {
    #[serde(rename = "mostRelevantPost")]
    post: Relationship,
}
#[derive(Deserialize)]
struct Relationship {
    data: Resource,
}
#[derive(Deserialize)]
struct Resource {
    id: String,
}
#[derive(Deserialize)]
struct Post {
    id: String,
    #[serde(default, rename = "type")]
    kind: String,
    attributes: serde_json::Value,
}
fn clean_html(body: &str) -> String {
    let body = body
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n");
    Html::parse_fragment(&body)
        .root_element()
        .text()
        .collect::<String>()
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
fn links(body: &str, quark: bool) -> Vec<Link> {
    let lines = body.lines().collect::<Vec<_>>();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(start) = line.find("http://").or_else(|| line.find("https://")) else {
            continue;
        };
        let raw = line[start..]
            .split([
                ' ', '\t', '\n', '"', '\'', '<', '>', ')', ']', '}', ',', ';',
            ])
            .next()
            .unwrap_or("");
        let Ok(url) = Url::parse(raw) else {
            continue;
        };
        let mut link = Link::new(raw);
        let allowed = if quark {
            link.cloud_type == CloudType::Quark
        } else {
            matches!(
                link.cloud_type,
                CloudType::Baidu | CloudType::Aliyun | CloudType::Tianyi
            )
        };
        if !allowed {
            continue;
        }
        link.password = ["pwd", "password", "passcode", "code"]
            .into_iter()
            .find_map(|key| {
                url.query_pairs()
                    .find(|(k, v)| k == key && !v.is_empty())
                    .map(|(_, v)| v.into_owned())
            });
        if link.password.is_none() {
            let mut candidates = Vec::new();
            for (j, text) in lines.iter().enumerate() {
                let keys = if link.cloud_type == CloudType::Tianyi {
                    ["访问码", "密码"]
                } else {
                    ["提取码", "密码"]
                };
                if keys.iter().any(|k| text.contains(k))
                    && let Some((_, code)) = text.split_once(':').or_else(|| text.split_once('：'))
                {
                    let code = code.trim();
                    if !code.is_empty() && code.len() <= 10 && i.abs_diff(j) <= 3 {
                        candidates.push((i.abs_diff(j), j, code));
                    }
                }
            }
            if let Some((_, _, code)) = candidates
                .into_iter()
                .min_by_key(|(distance, j, _)| (*distance, *j))
            {
                link.password = Some(code.to_owned());
            }
        }
        out.push(link);
    }
    out
}
pub(super) fn parse(
    body: &str,
    name: &str,
    query: &str,
) -> Result<Vec<SearchResult>, ProviderError> {
    let api: Api = serde_json::from_str(body).map_err(|e| ProviderError::Parse(e.to_string()))?;
    let quark = name == "quark4k";
    let posts = api
        .included
        .into_iter()
        .filter(|p| !quark || p.kind == "posts")
        .map(|p| (p.id, p.attributes))
        .collect::<HashMap<_, _>>();
    let mut out = Vec::new();
    for d in api.data {
        if !query
            .to_lowercase()
            .split_whitespace()
            .all(|w| d.attributes.title.to_lowercase().contains(w))
        {
            continue;
        }
        let Some(body) = posts
            .get(&d.relationships.post.data.id)
            .and_then(|a| a["contentHtml"].as_str())
        else {
            continue;
        };
        let mut result = SearchResult::provider(format!("{name}-{}", d.id), name);
        result.title = d.attributes.title;
        result.links = links(&clean_html(body), quark);
        if result.links.is_empty() {
            continue;
        }
        for link in &mut result.links {
            link.work_title = Some(result.title.clone());
        }
        result.datetime = DateTime::parse_from_rfc3339(&d.attributes.created_at)
            .ok()
            .map(|d| d.with_timezone(&Utc));
        out.push(result);
    }
    Ok(out)
}
pub(super) async fn fetch_page(
    ctx: &SearchContext,
    endpoint: &Url,
    name: &str,
    query: &str,
    offset: usize,
) -> Result<Vec<SearchResult>, ProviderError> {
    let include = if name == "quark4k" {
        "user,lastPostedUser,mostRelevantPost,mostRelevantPost.user,tags,tags.parent,firstPost"
    } else {
        "mostRelevantPost"
    };
    let mut request = ctx
        .client
        .get(endpoint.clone())
        .query(&[
            ("filter[q]", query),
            ("include", include),
            ("page[offset]", &offset.to_string()),
            ("page[limit]", "50"),
        ])
        .header(
            "user-agent",
            "Mozilla/5.0 Chrome/91.0.4472.124 Safari/537.36",
        )
        .header("accept", "application/json, text/plain, */*")
        .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("sec-fetch-dest", "empty")
        .header("sec-fetch-mode", "cors")
        .header("sec-fetch-site", "same-origin");
    if name == "quark4k" {
        request = request
            .query(&[("sort", "")])
            .header("referer", "https://quark4k.com/");
    }
    let body =
        crate::providers::b_registry::http::text(request, 3, 500, false, ctx.timeout, 16 << 20)
            .await?;
    let rows = parse(&body, name, query)?;
    ctx.publish(rows.clone());
    Ok(rows)
}
pub(super) async fn search_pages(
    ctx: &SearchContext,
    endpoint: &Url,
    name: &str,
    query: &str,
) -> Result<Vec<SearchResult>, ProviderError> {
    tokio::time::timeout(ctx.timeout, async {
        let (a, b) = tokio::join!(
            fetch_page(ctx, endpoint, name, query, 0),
            fetch_page(ctx, endpoint, name, query, 50)
        );
        let mut error = None;
        let mut out = Vec::new();
        for page in [a, b] {
            match page {
                Ok(v) => out.extend(v),
                Err(e) => error = Some(e),
            }
        }
        let mut seen = HashSet::new();
        out.retain(|r| seen.insert(r.id.clone()));
        out.sort_by_key(|a| std::cmp::Reverse(a.datetime));
        if let Some(e) = error {
            return Err(e);
        }
        Ok(out)
    })
    .await
    .map_err(|_| ProviderError::Timeout)?
}
#[async_trait]
impl Provider for Pan666 {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("pan666", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        search_pages(ctx, &self.endpoint, "pan666", query).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    pub(super) fn fixture(host: &str) -> String {
        serde_json::json!({"data":[{"id":"7","attributes":{"title":"Film","createdAt":"2026-10-12T00:00:00Z"},"relationships":{"mostRelevantPost":{"data":{"id":"8"}}}}],"included":[{"type":"posts","id":"8","attributes":{"contentHtml":format!("https://{host}/s/abc<br>提取码：1234")}}]}).to_string()
    }
    #[test]
    fn success_empty_malformed() {
        let r = parse(&fixture("pan.baidu.com"), "pan666", "film").unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].links[0].password.as_deref(), Some("1234"));
        assert!(
            parse(r#"{"data":[],"included":[]}"#, "pan666", "film")
                .unwrap()
                .is_empty()
        );
        assert!(parse("{}", "pan666", "film").is_err());
    }
    #[tokio::test]
    async fn exact_fixed_retry() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, query_param},
        };
        let s = MockServer::start().await;
        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();
        Mock::given(method("GET"))
            .and(query_param("page[limit]", "50"))
            .respond_with(move |_: &wiremock::Request| {
                if c.fetch_add(1, Ordering::SeqCst) < 2 {
                    ResponseTemplate::new(500)
                } else {
                    ResponseTemplate::new(200).set_body_string(fixture("pan.baidu.com"))
                }
            })
            .expect(3)
            .mount(&s)
            .await;
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
        let start = std::time::Instant::now();
        assert_eq!(
            fetch_page(&ctx, &Url::parse(&s.uri()).unwrap(), "pan666", "film", 0)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(start.elapsed() >= Duration::from_secs(1));
    }
    #[tokio::test]
    async fn exhaustion() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .expect(3)
            .mount(&s)
            .await;
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
        assert!(
            fetch_page(&ctx, &Url::parse(&s.uri()).unwrap(), "pan666", "film", 0)
                .await
                .unwrap_err()
                .to_string()
                .contains("503")
        );
    }
}
