use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, support::*};
use crate::core::{Link, ProviderError, SearchResult};
use async_trait::async_trait;
use chrono::{Datelike, Utc};
use futures::{StreamExt, stream};
use regex::Regex;
use std::sync::LazyLock;
use url::Url;
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/torrent/(\d+)/").unwrap());
static CURRENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{2}-\d{2})\s+(\d{2}:\d{2})").unwrap());
static HISTORICAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{2}-\d{2})\s+(\d{4})").unwrap());
static SIZE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Size\s+([0-9.]+)\s*([KMGT]?i?B)").unwrap());
#[derive(Debug, Clone)]
pub struct Thepiratebay {
    pub base: Url,
}
impl Default for Thepiratebay {
    fn default() -> Self {
        Self {
            base: Url::parse("https://thpibay.xyz/").unwrap(),
        }
    }
}
fn parse(bytes: &[u8]) -> Result<(Vec<SearchResult>, usize), ProviderError> {
    let doc = html(bytes)?;
    let mut pages = 1;
    for node in doc.select(&sel("table#searchResult + * a, td[colspan='9'] a")) {
        let href = node.value().attr("href").unwrap_or("");
        let page = href
            .split('/')
            .nth(3)
            .and_then(|s| s.parse::<usize>().ok())
            .or_else(|| node.text().collect::<String>().trim().parse().ok())
            .unwrap_or(1);
        pages = pages.max(page.min(30));
    }
    let mut rows = vec![];
    for node in doc.select(&sel("table#searchResult tr")) {
        let title = text(node, ".detName a.detLink").replace('.', " ");
        if title.is_empty() {
            continue;
        }
        let Some(href) = attr(node, ".detName a.detLink", "href") else {
            continue;
        };
        let Some(id) = ID
            .captures(&href)
            .and_then(|c| c.get(1))
            .map(|s| s.as_str().to_owned())
        else {
            continue;
        };
        let Some(raw) = attr(node, "a[href^='magnet:']", "href").filter(|s| magnet(s)) else {
            continue;
        };
        let desc = text(node, ".detDesc").replace('\u{a0}', " ");
        let mut row = SearchResult::provider(format!("thepiratebay-{id}"), "thepiratebay");
        row.title = title;
        row.tags = node
            .select(&sel(".vertTh a"))
            .map(|n| clean(&n.text().collect::<String>()))
            .filter(|s| !s.is_empty())
            .collect();
        let size = SIZE
            .captures(&desc)
            .map(|c| format!("文件大小: {}{}, ", &c[1], &c[2]))
            .unwrap_or_default();
        let counts: Vec<_> = node
            .select(&sel("td"))
            .map(|n| clean(&n.text().collect::<String>()))
            .collect();
        row.content = format!("{size}上传信息: {desc}");
        if let (Some(seed), Some(leech)) = (counts.get(2), counts.get(3)) {
            row.content
                .push_str(&format!(", Seeders: {seed}, Leechers: {leech}"));
        }
        row.datetime = CURRENT
            .captures(&desc)
            .and_then(|c| date(&format!("{}-{} {}", Utc::now().year(), &c[1], &c[2])))
            .or_else(|| {
                HISTORICAL
                    .captures(&desc)
                    .and_then(|c| date(&format!("{}-{}", &c[2], &c[1])))
            });
        row.links = vec![Link::new(raw)];
        rows.push(row);
    }
    Ok((rows, pages))
}
impl Thepiratebay {
    async fn page(
        &self,
        ctx: &SearchContext,
        query: &str,
        page: usize,
    ) -> Result<(Vec<SearchResult>, usize), ProviderError> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .unwrap()
            .clear()
            .push("search")
            .push(query)
            .push(&page.to_string())
            .push("99")
            .push("0");
        let bytes = body(
            ctx,
            || {
                ctx.client
                    .get(url.clone())
                    .header("referer", self.base.as_str())
            },
            10,
            STANDARD,
        )
        .await?;
        parse(&bytes)
    }
}
#[async_trait]
impl Provider for Thepiratebay {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("thepiratebay", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if ctx.is_cancelled() {
            return Err(ProviderError::Timeout);
        }
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let (mut rows, pages) = self.page(ctx, query, 1).await?;
        ctx.publish(rows.clone());
        let more = collect(
            ctx,
            stream::iter((2..=pages).map(|page| async move {
                let (rows, _) = self.page(ctx, query, page).await?;
                ctx.publish(rows.clone());
                Ok(rows)
            }))
            .buffered(8),
        )
        .await?;
        rows.extend(more);
        Ok(rows)
    }
}
