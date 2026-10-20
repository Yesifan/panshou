//! 雷鲸 topic summaries first, then missing Tianyi links from topicContent.
use super::support::*;
use crate::{
    core::{CloudType, Link, ProviderError, SearchResult},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use std::{collections::HashSet, sync::LazyLock, time::Duration};
use url::Url;
static TIANYI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https://cloud\.189\.cn/t/[A-Za-z0-9]+").unwrap());
#[derive(Debug, Clone)]
pub struct Leijing {
    base: Url,
}
impl Default for Leijing {
    fn default() -> Self {
        Self::new(Url::parse("https://leijing.xyz/").unwrap())
    }
}
impl Leijing {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
    async fn fetch(&self, ctx: &SearchContext, url: Url) -> Result<String, ProviderError> {
        text(
            ctx,
            browser(ctx.client.get(url), self.base.as_str(), HTML)
                .header("accept-encoding", "gzip, deflate")
                .header("cache-control", "no-cache")
                .header("pragma", "no-cache"),
            1,
            30,
        )
        .await
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self
            .base
            .join("search")
            .map_err(|_| protocol("leijing invalid base"))?;
        url.query_pairs_mut().append_pair("keyword", query);
        let body = self.fetch(ctx, url).await?;
        let items = {
            let doc = require_document(
                &body,
                ".topicItem, .topicList, .topic-list, .no-results",
                "leijing",
            )?;
            doc.select(&sel(".topicItem"))
                .filter_map(|node| {
                    let anchor = node.select(&sel(".title a[href]")).next()?;
                    let url = self.base.join(anchor.value().attr("href")?).ok()?;
                    let title = text_of(anchor);
                    if title.is_empty() {
                        return None;
                    }
                    let id = url
                        .query_pairs()
                        .find(|(k, _)| k == "topicId")
                        .map(|(_, v)| v.into_owned())
                        .filter(|v| !v.is_empty())
                        .unwrap_or_else(|| format!("{:x}", md5::compute(url.as_str())));
                    let mut row = SearchResult::provider(format!("leijing-{id}"), "leijing");
                    row.title = title;
                    row.content = node
                        .select(&sel(".summary"))
                        .next()
                        .map(text_of)
                        .unwrap_or_default();
                    row.datetime = node
                        .select(&sel(".postTime"))
                        .next()
                        .and_then(|n| date(text_of(n).trim_start_matches("发表时间：")));
                    row.links = TIANYI
                        .find_iter(&row.content)
                        .map(|m| Link::new(m.as_str()))
                        .collect();
                    Some((url, row))
                })
                .collect::<Vec<_>>()
        };
        let mut jobs = stream::iter(items.into_iter().enumerate().map(
            |(i, (url, mut row))| async move {
                if row.links.is_empty() {
                    tokio::time::sleep(Duration::from_millis(i as u64 * 50)).await;
                    let body = self.fetch(ctx, url).await?;
                    let doc = require_document(&body, ".topicContent", "leijing detail")?;
                    let mut seen = HashSet::new();
                    for node in doc.select(&sel(".topicContent a[href]")) {
                        let raw = node.value().attr("href").unwrap_or("");
                        let link = Link::new(raw);
                        if link.cloud_type == CloudType::Tianyi && seen.insert(raw.to_owned()) {
                            row.links.push(link);
                        }
                    }
                    if row.links.is_empty() {
                        for n in doc.select(&sel(".topicContent")) {
                            for m in TIANYI.find_iter(&text_of(n)) {
                                if seen.insert(m.as_str().to_owned()) {
                                    row.links.push(Link::new(m.as_str()));
                                }
                            }
                        }
                    }
                }
                Ok::<_, ProviderError>(publish(ctx, row, query))
            },
        ))
        .buffer_unordered(20);
        let mut rows = Vec::new();
        let mut error = None;
        while let Some(item) = jobs.next().await {
            match item {
                Ok(Some(r)) => rows.push(r),
                Ok(None) => {}
                Err(e) => error = Some(e),
            }
        }
        if let Some(e) = error {
            return Err(e);
        }
        Ok(rows)
    }
}
#[async_trait]
impl Provider for Leijing {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("leijing", 2, KeywordFilterMode::Core)
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
