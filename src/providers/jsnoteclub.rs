use crate::core::{ProviderError, SearchResult, Source, extract_links};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{Html, Selector};
use serde::Deserialize;
use url::Url;

const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/123 Safari/537.36";

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://jsnoteclub.com/").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Jsnoteclub {
    endpoints: Endpoints,
}
impl Jsnoteclub {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
#[derive(Debug, Clone, Deserialize)]
struct Post {
    id: String,
    #[serde(default)]
    slug: String,
    title: String,
    #[serde(default)]
    excerpt: String,
    url: String,
    #[serde(default)]
    updated_at: String,
}
#[derive(Deserialize)]
struct Posts {
    #[serde(default)]
    posts: Vec<Post>,
}
fn parse_key(body: &str) -> Result<String, ProviderError> {
    Regex::new(r#"data-key="([^"]+)""#)
        .unwrap()
        .captures(body)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_owned())
        .ok_or_else(|| ProviderError::Parse("Ghost data-key missing".into()))
}
fn parse_posts(body: &str) -> Result<Vec<Post>, ProviderError> {
    serde_json::from_str::<Posts>(body)
        .map(|p| p.posts)
        .map_err(|e| ProviderError::Parse(e.to_string()))
}
fn parse_links(body: &str) -> Vec<crate::core::Link> {
    let mut links = extract_links(body);
    let plain = Html::parse_document(body)
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join(" ");
    let pass = Regex::new(r"(?i)(?:提取码|密码|pwd|code)\s*[=:：]?\s*([0-9A-Za-z]+)")
        .unwrap()
        .captures(&plain)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_owned());
    if let Some(p) = pass {
        for l in &mut links {
            if l.password.is_none() {
                l.password = Some(p.clone())
            }
        }
    }
    links
}

#[async_trait]
impl Provider for Jsnoteclub {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("jsnoteclub", 2, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Ok(vec![]);
        }
        let home = crate::providers::b_registry::http::text(
            ctx.client
                .get(self.endpoints.base.clone())
                .header("user-agent", UA)
                .header(
                    "accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
                )
                .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
                .header("referer", self.endpoints.base.as_str()),
            3,
            200,
            true,
            std::time::Duration::from_secs(12),
            16 << 20,
        )
        .await?;
        let key = parse_key(&home)?;
        let mut u = self
            .endpoints
            .base
            .join("ghost/api/content/posts/")
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        u.query_pairs_mut()
            .append_pair("key", &key)
            .append_pair("limit", "10000")
            .append_pair("fields", "id,slug,title,excerpt,url,updated_at,visibility")
            .append_pair("order", "updated_at DESC");
        let body = crate::providers::b_registry::http::text(
            ctx.client
                .get(u)
                .header("user-agent", UA)
                .header("accept", "application/json, text/plain, */*")
                .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
                .header("referer", self.endpoints.base.as_str()),
            3,
            200,
            true,
            std::time::Duration::from_secs(12),
            16 << 20,
        )
        .await?;
        let words = q.split_whitespace().collect::<Vec<_>>();
        let posts = parse_posts(&body)?
            .into_iter()
            .filter(|p| {
                let h = format!("{} {} {}", p.title, p.excerpt, p.slug).to_lowercase();
                words.iter().all(|w| h.contains(w))
            })
            .take(30)
            .collect::<Vec<_>>();
        let client = ctx.client.clone();
        let errors = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let errors_ref = errors.clone();
        let rows = stream::iter(posts.into_iter().map(|p| {
            let client = client.clone();
            let errors = errors_ref.clone();
            async move {
                let body = crate::providers::b_registry::http::text(client
                    .get(&p.url)
                    .header("user-agent", UA)
                    .header(
                        "accept",
                        "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
                    )
                    .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
                    .header("referer", &p.url)
                    , 3, 200, true, std::time::Duration::from_secs(10), 16 << 20).await.inspect_err(|e| {
                        errors.lock().unwrap().push(e.to_string());
                    }).ok()?;
                let doc = Html::parse_document(&body);
                let s = Selector::parse("section.gh-content, .gh-content, article").unwrap();
                let fragment = doc
                    .select(&s)
                    .next()
                    .map(|x| x.inner_html())
                    .unwrap_or(body);
                let links = parse_links(&fragment);
                if links.is_empty() {
                    return None;
                }
                let result = SearchResult {
                    id: format!("jsnoteclub-{}", p.id),
                    source: Source::provider("jsnoteclub"),
                    datetime: DateTime::parse_from_rfc3339(&p.updated_at)
                        .ok()
                        .map(|d| d.with_timezone(&Utc)),
                    title: p.title.trim().into(),
                    content: p.excerpt.trim().into(),
                    links,
                    tags: if p.slug.is_empty() {
                        vec![]
                    } else {
                        vec![p.slug]
                    },
                    images: vec![],
                };
                ctx.publish(vec![result.clone()]);
                Some(result)
            }
        }))
        .buffered(8)
        .collect::<Vec<_>>()
        .await;
        let failures = errors.lock().unwrap();
        if let Some(error) = failures.first() {
            return Err(ProviderError::Unavailable(format!(
                "jsnoteclub {} detail requests failed: {error}",
                failures.len()
            )));
        }
        Ok(rows.into_iter().flatten().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_success() {
        assert_eq!(
            parse_key(include_str!(
                "../../tests/fixtures/providers/jsnoteclub/success-home.html"
            ))
            .unwrap(),
            "abc123_blocked"
        );
        let p = parse_posts(include_str!(
            "../../tests/fixtures/providers/jsnoteclub/success.json"
        ))
        .unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(
            parse_links("https://pan.baidu.com/s/abc 提取码：a1b2")[0]
                .password
                .as_deref(),
            Some("a1b2")
        );
    }
    #[test]
    fn fixture_empty() {
        assert!(
            parse_posts(include_str!(
                "../../tests/fixtures/providers/jsnoteclub/empty.json"
            ))
            .unwrap()
            .is_empty()
        );
        assert!(parse_links("none").is_empty())
    }
    #[test]
    fn fixture_malformed() {
        assert!(parse_key("<html/>").is_err());
        assert!(
            parse_posts(include_str!(
                "../../tests/fixtures/providers/jsnoteclub/malformed.json"
            ))
            .is_err()
        )
    }
}
