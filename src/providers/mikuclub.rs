//! Mikuclub WordPress utils/v2 category-specific post lists and article downloads.
use super::support::*;
use crate::{
    core::{CloudType, ProviderError, SearchResult},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use serde::Deserialize;
use std::collections::HashSet;
use url::Url;
#[derive(Debug, Clone)]
pub struct Mikuclub {
    base: Url,
}
impl Default for Mikuclub {
    fn default() -> Self {
        Self::new(Url::parse("https://www.mikuclub.uk/").unwrap())
    }
}
#[derive(Deserialize)]
struct Response {
    posts: Vec<Post>,
}
#[derive(Deserialize)]
struct Post {
    id: u64,
    post_title: String,
    post_href: String,
    #[serde(default)]
    post_main_cat_name: String,
    #[serde(default)]
    post_cat_name: String,
    #[serde(default)]
    post_date: String,
    #[serde(default)]
    post_rank_description: String,
    #[serde(default)]
    post_views: u64,
}
impl Mikuclub {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
    async fn category(
        &self,
        ctx: &SearchContext,
        query: &str,
        category: &str,
    ) -> Result<Vec<Post>, ProviderError> {
        let mut url = self
            .base
            .join("wp-json/utils/v2/post_list")
            .map_err(|_| protocol("mikuclub invalid base"))?;
        url.query_pairs_mut().extend_pairs([
            ("search", query),
            ("s", query),
            ("page", ""),
            ("pagename", "search_page"),
            ("page_type", "search"),
            ("paged", "1"),
            ("custom_orderby", "relevance"),
            ("no_cache", "1"),
            ("custom_cat", category),
        ]);
        let body = text(
            ctx,
            browser(
                ctx.client.get(url),
                self.base.as_str(),
                "application/json, text/html;q=0.9,*/*;q=0.8",
            ),
            3,
            12,
        )
        .await?;
        Ok(serde_json::from_str::<Response>(&body)
            .map_err(parse)?
            .posts)
    }
    async fn detail(
        &self,
        ctx: &SearchContext,
        query: &str,
        post: Post,
    ) -> Result<Option<SearchResult>, ProviderError> {
        let url = self
            .base
            .join(&post.post_href)
            .map_err(|_| protocol("mikuclub invalid post URL"))?;
        let body = text(
            ctx,
            browser(
                ctx.client.get(url.clone()),
                url.as_str(),
                "application/json, text/html;q=0.9,*/*;q=0.8",
            ),
            2,
            10,
        )
        .await?;
        let doc = require_document(
            &body,
            ".article_content, article.post, .entry-content",
            "mikuclub detail",
        )?;
        let container = doc
            .select(&sel(".article_content"))
            .next()
            .or_else(|| doc.select(&sel("article.post, .entry-content")).next())
            .unwrap_or_else(|| doc.root_element());
        let mut row = SearchResult::provider(format!("mikuclub-{}", post.id), "mikuclub");
        row.title = post.post_title.trim().into();
        let mut parts = Vec::new();
        if !post.post_rank_description.trim().is_empty() {
            parts.push(format!("口碑：{}", post.post_rank_description.trim()));
        }
        if post.post_views > 0 {
            parts.push(format!("浏览：{}", post.post_views));
        }
        row.content = parts.join(" ");
        row.tags = [post.post_main_cat_name, post.post_cat_name]
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .collect();
        row.datetime = date(&post.post_date);
        row.links = links_in(container)
            .into_iter()
            .filter(|link| {
                matches!(
                    link.cloud_type,
                    CloudType::Quark
                        | CloudType::Aliyun
                        | CloudType::Baidu
                        | CloudType::Xunlei
                        | CloudType::Uc
                        | CloudType::Pikpak
                        | CloudType::Mobile
                        | CloudType::Magnet
                        | CloudType::Pan123
                )
            })
            .collect();
        Ok(publish(ctx, row, query))
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(HashSet::new()));
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(12));
        let categories = vec!["9305".to_owned(), "942".to_owned()];
        let mut jobs = stream::iter(categories.into_iter().map(|category| {
            let seen = seen.clone();
            let semaphore = semaphore.clone();
            async move {
                let posts = self.category(ctx, query, &category).await?;
                let posts = {
                    let mut seen = seen
                        .lock()
                        .map_err(|_| protocol("mikuclub ID lock poisoned"))?;
                    posts
                        .into_iter()
                        .filter(|p| p.id > 0 && seen.insert(p.id))
                        .collect::<Vec<_>>()
                };
                let mut details = stream::iter(posts.into_iter().map(|post| {
                    let semaphore = semaphore.clone();
                    async move {
                        let _permit = semaphore
                            .acquire()
                            .await
                            .map_err(|_| protocol("mikuclub detail semaphore closed"))?;
                        self.detail(ctx, query, post).await
                    }
                }))
                .buffer_unordered(12);
                let mut rows = Vec::new();
                let mut error = None;
                while let Some(result) = details.next().await {
                    match result {
                        Ok(Some(row)) => rows.push(row),
                        Ok(None) => {}
                        Err(e) => error = Some(e),
                    }
                }
                if let Some(e) = error {
                    return Err(e);
                }
                Ok::<_, ProviderError>(rows)
            }
        }))
        .buffer_unordered(2);
        let mut rows = Vec::new();
        let mut error = None;
        while let Some(result) = jobs.next().await {
            match result {
                Ok(mut results) => rows.append(&mut results),
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
impl Provider for Mikuclub {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("mikuclub", 2, KeywordFilterMode::Core)
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
