use crate::core::{Link, ProviderError, SearchResult, Source};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use base64::Engine;
use chrono::{DateTime, NaiveDateTime, Utc};
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{Html, Selector};
use serde::Deserialize;
use url::Url;

const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/138 Safari/537.36";
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://susuifa.com").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Susu {
    endpoints: Endpoints,
}
impl Susu {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
#[derive(Clone)]
struct Post {
    id: String,
    title: String,
    content: String,
    datetime: Option<DateTime<Utc>>,
    tags: Vec<String>,
}
fn posts(body: &str, query: &str) -> Vec<Post> {
    let d = Html::parse_document(body);
    let row = Selector::parse(".post-list-item").unwrap();
    let title = Selector::parse(".post-info h2 a").unwrap();
    let excerpt = Selector::parse(".post-excerpt").unwrap();
    let time = Selector::parse(".list-footer time.b2timeago").unwrap();
    let tag = Selector::parse(".post-list-cat-item").unwrap();
    let idre = Regex::new(r"/(\d+)\.html").unwrap();
    let words = query
        .to_lowercase()
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    d.select(&row)
        .filter_map(|r| {
            let a = r.select(&title).next()?;
            let t = a.text().collect::<String>().trim().to_owned();
            if !words.iter().all(|w| t.to_lowercase().contains(w)) {
                return None;
            }
            let id = r
                .value()
                .attr("id")
                .and_then(|x| x.strip_prefix("item-"))
                .map(str::to_owned)
                .or_else(|| {
                    a.value()
                        .attr("href")
                        .and_then(|x| idre.captures(x))
                        .and_then(|c| c.get(1))
                        .map(|m| m.as_str().to_owned())
                })?;
            let dt = r
                .select(&time)
                .next()
                .and_then(|x| x.value().attr("datetime"))
                .and_then(|x| NaiveDateTime::parse_from_str(x, "%Y-%m-%d %H:%M:%S").ok())
                .map(|x| DateTime::from_naive_utc_and_offset(x, Utc));
            Some(Post {
                id,
                title: t,
                content: r
                    .select(&excerpt)
                    .next()
                    .map(|x| x.text().collect::<String>().trim().into())
                    .unwrap_or_default(),
                datetime: dt,
                tags: r
                    .select(&tag)
                    .map(|x| x.text().collect::<String>().trim().into())
                    .filter(|x: &String| !x.is_empty())
                    .collect(),
            })
        })
        .collect()
}
#[derive(Debug, Deserialize)]
struct Group {
    #[serde(default)]
    button: Vec<Button>,
}
#[derive(Debug, Deserialize)]
struct Button {
    #[serde(default, rename = "name")]
    _name: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    attr: Attr,
}
#[derive(Debug, Default, Deserialize)]
struct Attr {
    #[serde(default)]
    tq: String,
}
#[derive(Deserialize)]
struct Detail {
    button: Button,
}
fn groups(body: &str) -> Result<Vec<(usize, usize)>, ProviderError> {
    let gs: Vec<Group> =
        serde_json::from_str(body).map_err(|e| ProviderError::Parse(e.to_string()))?;
    Ok(gs
        .iter()
        .enumerate()
        .flat_map(|(g, x)| x.button.iter().enumerate().map(move |(i, _)| (g, i)))
        .collect())
}
fn jwt_url(token: &str) -> Result<String, ProviderError> {
    if Url::parse(token)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https"))
        .is_some()
    {
        return Ok(token.trim().into());
    }
    let p = token.split('.').collect::<Vec<_>>();
    if p.len() != 3 {
        return Err(ProviderError::Parse("invalid susu JWT".into()));
    }
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(p[1])
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(p[1]))
        .map_err(|e| ProviderError::Parse(e.to_string()))?;
    let v: serde_json::Value =
        serde_json::from_slice(&raw).map_err(|e| ProviderError::Parse(e.to_string()))?;
    v.pointer("/data/url")
        .and_then(|x| x.as_str())
        .filter(|x| !x.trim().is_empty())
        .map(|x| x.trim().into())
        .ok_or_else(|| ProviderError::Parse("susu JWT URL missing".into()))
}
fn detail(body: &str) -> Result<Link, ProviderError> {
    let d: Detail = serde_json::from_str(body).map_err(|e| ProviderError::Parse(e.to_string()))?;
    let url = jwt_url(&d.button.url)?;
    Url::parse(&url)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https") && u.host().is_some())
        .ok_or_else(|| ProviderError::Parse("invalid susu link".into()))?;
    let mut l = Link::new(url);
    let from_url = Url::parse(&l.url).ok().and_then(|u| {
        ["pwd", "password", "passcode", "code"]
            .iter()
            .find_map(|k| {
                u.query_pairs()
                    .find(|(x, _)| x == *k)
                    .map(|(_, v)| v.into_owned())
            })
    });
    l.password = from_url.or_else(|| {
        (!d.button.attr.tq.trim().is_empty()).then(|| d.button.attr.tq.trim().to_owned())
    });
    Ok(l)
}
async fn links(ctx: &SearchContext, base: &Url, id: &str) -> Result<Vec<Link>, ProviderError> {
    let list = base
        .join("?rest_route=/b2/v1/getDownloadData")
        .map_err(|e| ProviderError::Protocol(e.to_string()))?;
    let origin = base.origin().ascii_serialization();
    let root = base.as_str().trim_end_matches('/');
    let referer = format!("{root}/{id}.html");
    let body = ctx
        .client
        .post(list)
        .header("user-agent", UA)
        .header("accept", "application/json, text/plain, */*")
        .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("origin", &origin)
        .header("referer", &referer)
        .form(&[("post_id", id), ("guest", "")])
        .send()
        .await
        .map_err(|e| ProviderError::Network(e.to_string()))?
        .error_for_status()
        .map_err(|e| ProviderError::Network(e.to_string()))?
        .text()
        .await
        .map_err(|e| ProviderError::Network(e.to_string()))?;
    let jobs = groups(&body)?;
    let detail_url = base
        .join("?rest_route=/b2/v1/getDownloadPageData")
        .map_err(|e| ProviderError::Protocol(e.to_string()))?;
    let client = ctx.client.clone();
    let id = id.to_owned();
    let root = root.to_owned();
    let rows = stream::iter(jobs.into_iter().map(|(index, i)| {
        let client = client.clone();
        let u = detail_url.clone();
        let id = id.clone();
        let origin = origin.clone();
        let root = root.clone();
        async move {
            let b = client
                .post(u)
                .header("user-agent", UA)
                .header("accept", "application/json, text/plain, */*")
                .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
                .header("origin", origin)
                .header(
                    "referer",
                    format!("{root}/download?post_id={id}&index={index}&i={i}"),
                )
                .form(&[
                    ("post_id", id),
                    ("index", index.to_string()),
                    ("i", i.to_string()),
                    ("guest", "".into()),
                ])
                .send()
                .await
                .ok()?
                .error_for_status()
                .ok()?
                .text()
                .await
                .ok()?;
            detail(&b).ok()
        }
    }))
    .buffered(4)
    .collect::<Vec<_>>()
    .await;
    let mut seen = std::collections::HashSet::new();
    Ok(rows
        .into_iter()
        .flatten()
        .filter(|l| seen.insert((l.cloud_type, l.url.clone(), l.password.clone())))
        .collect())
}
#[async_trait]
impl Provider for Susu {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("susu", 1, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut url = self.endpoints.base.clone();
        url.query_pairs_mut()
            .append_pair("type", "post")
            .append_pair("s", query);
        let body = ctx
            .client
            .get(url)
            .header("user-agent", UA)
            .header(
                "accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
            )
            .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
            .header(
                "referer",
                format!("{}/", self.endpoints.base.as_str().trim_end_matches('/')),
            )
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?
            .error_for_status()
            .map_err(|e| ProviderError::Network(e.to_string()))?
            .text()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        let base = self.endpoints.base.clone();
        let futures = posts(&body, query).into_iter().map(|post| {
            let base = base.clone();
            async move {
                let found = links(ctx, &base, &post.id).await.ok()?;
                (!found.is_empty()).then(|| SearchResult {
                    id: format!("susu-{}", post.id),
                    source: Source::provider("susu"),
                    datetime: post.datetime,
                    title: post.title,
                    content: post.content,
                    links: found,
                    tags: post.tags,
                    images: vec![],
                })
            }
        });
        let rows = stream::iter(futures).buffered(4).collect::<Vec<_>>().await;
        Ok(rows.into_iter().flatten().collect())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_success() {
        let p = posts(
            include_str!("../../tests/fixtures/providers/susu/success-post.html"),
            "瑞克",
        );
        assert_eq!(p[0].id, "12");
        assert_eq!(
            groups(include_str!(
                "../../tests/fixtures/providers/susu/success-buttons.json"
            ))
            .unwrap(),
            vec![(0, 0), (0, 1), (1, 0)]
        );
        let l = detail(include_str!(
            "../../tests/fixtures/providers/susu/success.json"
        ))
        .unwrap();
        assert_eq!(l.password.as_deref(), Some("abcd"));
    }
    #[test]
    fn fixture_empty() {
        assert!(posts("<html/>", "x").is_empty());
        assert!(
            groups(include_str!(
                "../../tests/fixtures/providers/susu/empty.json"
            ))
            .unwrap()
            .is_empty()
        )
    }
    #[test]
    fn fixture_malformed() {
        assert!(
            groups(include_str!(
                "../../tests/fixtures/providers/susu/malformed-buttons.json"
            ))
            .is_err()
        );
        assert!(jwt_url("bad").is_err());
        assert!(
            detail(include_str!(
                "../../tests/fixtures/providers/susu/malformed.json"
            ))
            .is_err()
        )
    }
}
