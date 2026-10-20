//! BT之家 JSON index → first forum post, including torrent attachments.
use super::support::*;
use crate::{
    core::{CloudType, Link, ProviderError, SearchResult},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use serde::Deserialize;
use std::{collections::HashSet, sync::LazyLock};
use url::Url;
#[derive(Debug, Clone)]
pub struct Lou1 {
    base: Url,
}
impl Default for Lou1 {
    fn default() -> Self {
        Self::new(Url::parse("http://www.1lou.me/").unwrap())
    }
}
#[derive(Deserialize)]
struct Response {
    ok: bool,
    data: Data,
}
#[derive(Deserialize)]
struct Data {
    hits: Vec<Hit>,
}
#[derive(Deserialize)]
struct Hit {
    subject: String,
    thread_url: String,
}
impl Lou1 {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"thread-(\d+)").unwrap());
        let mut url = self
            .base
            .join("search/api/search.php")
            .map_err(|_| protocol("lou1 invalid base"))?;
        url.query_pairs_mut()
            .append_pair("q", query.trim())
            .append_pair("page", "1")
            .append_pair("page_size", "12");
        let body = text(
            ctx,
            browser(ctx.client.get(url), self.base.as_str(), HTML),
            3,
            12,
        )
        .await?;
        let response: Response = serde_json::from_str(&body).map_err(parse)?;
        if !response.ok {
            return Err(protocol("lou1 search API returned ok=false"));
        }
        let items = response
            .data
            .hits
            .into_iter()
            .filter_map(|hit| {
                let url = self
                    .base
                    .join(hit.thread_url.trim().trim_start_matches("./"))
                    .ok()?;
                (!hit.subject.trim().is_empty()).then_some((hit.subject, url))
            })
            .take(12)
            .collect::<Vec<_>>();
        let mut jobs = stream::iter(items.into_iter().map(|(title, url)| async move {
            let body = text(
                ctx,
                browser(ctx.client.get(url.clone()), self.base.as_str(), HTML),
                3,
                12,
            )
            .await?;
            let doc = require_document(&body, ".message, .card-thread, article", "lou1 detail")?;
            let container = doc
                .select(&sel("div.message[isfirst='1']"))
                .next()
                .or_else(|| doc.select(&sel(".message")).next())
                .unwrap_or_else(|| doc.root_element());
            let id = ID
                .captures(url.as_str())
                .map(|c| c[1].to_owned())
                .unwrap_or_else(|| crc32(url.as_str().as_bytes()).to_string());
            let mut row = SearchResult::provider(format!("lou1-{id}"), "lou1");
            row.title = title.trim().into();
            row.content = attr(&doc, "meta[name='description']", "content")
                .unwrap_or_else(|| short(&text_of(container), 200));
            row.datetime = doc
                .select(&sel(".card-thread span.date"))
                .next()
                .and_then(|n| date(&text_of(n)));
            let mut tags = HashSet::new();
            for n in doc.select(&sel(".breadcrumb a, h4 a.badge")) {
                let s = text_of(n);
                if !s.is_empty() && s != "首页" && tags.insert(s.clone()) {
                    row.tags.push(s);
                }
            }
            row.links = links_in(container)
                .into_iter()
                .filter(|link| {
                    matches!(
                        link.cloud_type,
                        CloudType::Quark
                            | CloudType::Baidu
                            | CloudType::Xunlei
                            | CloudType::Aliyun
                            | CloudType::Uc
                            | CloudType::Pan123
                            | CloudType::Pikpak
                            | CloudType::Magnet
                            | CloudType::Ed2k
                    )
                })
                .collect();
            let mut seen = row
                .links
                .iter()
                .map(|l| l.url.clone())
                .collect::<HashSet<_>>();
            for anchor in container.select(&sel("a[href]")) {
                let href = anchor.value().attr("href").unwrap_or("");
                if href.trim_start_matches('/').starts_with("attach-download-")
                    && let Ok(url) = self.base.join(href)
                {
                    let raw = url.to_string();
                    if seen.insert(raw.clone()) {
                        let mut link = Link::new(raw);
                        link.password = password(&text_of(anchor)).or_else(|| {
                            anchor
                                .parent()
                                .and_then(scraper::ElementRef::wrap)
                                .and_then(|n| password(&text_of(n)))
                        });
                        row.links.push(link);
                    }
                }
            }
            if row.links.is_empty() {
                return Err(protocol(
                    "lou1 detail has no supported links or torrent attachments",
                ));
            }
            Ok::<_, ProviderError>(publish(ctx, row, query))
        }))
        .buffer_unordered(6);
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
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb88320 } else { 0 };
        }
    }
    !crc
}
#[async_trait]
impl Provider for Lou1 {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("lou1", 1, KeywordFilterMode::Core)
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
