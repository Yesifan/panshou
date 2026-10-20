//! Linux.do Discourse JSON search. Pagination and optional topic reads share the caller's budget.
use super::{clean, date, fetch, headers, raw_links};
use crate::core::{Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};
use url::Url;
#[derive(Debug, Clone)]
pub struct DiscourseProvider {
    base: Url,
    max_pages: usize,
}
impl Default for DiscourseProvider {
    fn default() -> Self {
        Self::new(Url::parse("https://linux.do/").unwrap())
    }
}
impl DiscourseProvider {
    pub fn new(base: Url) -> Self {
        Self { base, max_pages: 1 }
    }
    pub fn with_max_pages(mut self, pages: usize) -> Self {
        self.max_pages = pages.clamp(1, 10);
        self
    }
    pub async fn topic_detail(
        &self,
        ctx: &SearchContext,
        id: u64,
    ) -> Result<Vec<Link>, ProviderError> {
        let mut url = self.base.join(&format!("t/{id}.json")).unwrap();
        url.query_pairs_mut()
            .append_pair("track_visit", "true")
            .append_pair("forceLoad", "true");
        let body = fetch(
            ctx,
            headers(ctx.client.get(url), self.base.as_str()),
            "discourse",
            "topic",
            Duration::from_secs(30),
            16 << 20,
            &[],
        )
        .await?;
        let data: Value = serde_json::from_str(&body)
            .map_err(|_| ProviderError::Parse("discourse: malformed topic JSON".into()))?;
        let posts = data
            .pointer("/post_stream/posts")
            .and_then(Value::as_array)
            .ok_or_else(|| ProviderError::Parse("discourse: missing topic posts".into()))?;
        let mut links = Vec::new();
        for post in posts {
            for link in raw_links(post.get("cooked").and_then(Value::as_str).unwrap_or("")) {
                if !links.iter().any(|l: &Link| l.url == link.url) {
                    links.push(link);
                }
            }
        }
        Ok(links)
    }
}
fn parse(body: &str) -> Result<(Vec<SearchResult>, bool), ProviderError> {
    let data: Value = serde_json::from_str(body)
        .map_err(|_| ProviderError::Parse("discourse: malformed search JSON".into()))?;
    let posts = data
        .get("posts")
        .and_then(Value::as_array)
        .ok_or_else(|| ProviderError::Parse("discourse: missing posts".into()))?;
    let topics: HashMap<u64, &Value> = data
        .get("topics")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| Some((t.get("id")?.as_u64()?, t)))
        .collect();
    let mut out = Vec::new();
    for post in posts {
        let Some(id) = post.get("id").and_then(Value::as_u64) else {
            continue;
        };
        let topic = post
            .get("topic_id")
            .and_then(Value::as_u64)
            .and_then(|id| topics.get(&id));
        let blurb = post.get("blurb").and_then(Value::as_str).unwrap_or("");
        let links = raw_links(blurb);
        if links.is_empty() {
            continue;
        }
        let mut row = SearchResult::provider(format!("discourse-{id}"), "discourse");
        row.title = topic
            .and_then(|t| t.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("未知标题")
            .into();
        row.content = clean(blurb).chars().take(200).collect();
        row.tags = topic
            .and_then(|t| t.get("tags"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        row.datetime = post
            .get("created_at")
            .and_then(Value::as_str)
            .and_then(date);
        row.links = links;
        out.push(row);
    }
    Ok((
        out,
        data.pointer("/grouped_search_result/more_full_page_results")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    ))
}
#[async_trait]
impl Provider for DiscourseProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("discourse", 2, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for page in 1..=self.max_pages {
            if page > 1 {
                tokio::time::timeout(
                    ctx.remaining(),
                    tokio::time::sleep(Duration::from_millis(500)),
                )
                .await
                .map_err(|_| ProviderError::Timeout)?;
            }
            let mut url = self.base.join("search.json").unwrap();
            url.query_pairs_mut()
                .append_pair("q", &format!("{query} in:title #resource"))
                .append_pair("page", &page.to_string());
            let body = fetch(
                ctx,
                headers(ctx.client.get(url), self.base.as_str())
                    .header("Accept", "application/json"),
                "discourse",
                "search",
                Duration::from_secs(30),
                16 << 20,
                &[],
            )
            .await?;
            let (rows, more) = parse(&body)?;
            let empty = rows.is_empty();
            let rows = rows
                .into_iter()
                .filter(|r| seen.insert(r.id.clone()))
                .collect::<Vec<_>>();
            ctx.publish(rows.clone());
            out.extend(rows);
            if !more || empty {
                break;
            }
        }
        Ok(out)
    }
}
