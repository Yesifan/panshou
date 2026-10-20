//! ahhhhfs: fixed upstream WordPress search and source-specific detail DOM.
use super::{anchors, clean, date, fetch, headers, matches, sel, text};
use crate::core::{ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::Html;
use std::{collections::HashSet, sync::LazyLock, time::Duration};
use url::Url;
#[derive(Debug, Clone)]
pub struct AhhhhfsProvider {
    base: Url,
}
impl Default for AhhhhfsProvider {
    fn default() -> Self {
        Self::new(Url::parse("https://www.ahhhhfs.com/").unwrap())
    }
}
impl AhhhhfsProvider {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
}
fn items(body: &str, base: &Url) -> Result<Vec<(SearchResult, Url)>, ProviderError> {
    static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/(\d+)/?$").unwrap());
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    if doc
        .select(&sel("article.post-item.item-list"))
        .next()
        .is_none()
        && doc.select(&sel("body")).next().is_none()
    {
        return Err(ProviderError::Parse("ahhhhfs: missing document".into()));
    }
    for node in doc.select(&sel("article.post-item.item-list")) {
        let Some(a) = node.select(&sel(".entry-title a")).next() else {
            continue;
        };
        let title = clean(&a.inner_html());
        let title = if title.is_empty() {
            a.value().attr("title").unwrap_or("").to_owned()
        } else {
            title
        };
        let Some(url) = a.value().attr("href").and_then(|s| base.join(s).ok()) else {
            continue;
        };
        let Some(id) = ID.captures(url.as_str()).map(|c| c[1].to_owned()) else {
            continue;
        };
        if title.is_empty() || !seen.insert(id.clone()) {
            continue;
        }
        let mut row = SearchResult::provider(format!("ahhhhfs-{id}"), "ahhhhfs");
        row.title = title;
        row.content = text(node, ".entry-desc");
        row.tags = node
            .select(&sel(".entry-cat-dot a"))
            .map(|n| clean(&n.inner_html()))
            .filter(|s| !s.is_empty())
            .collect();
        row.datetime = node.select(&sel("time")).next().and_then(|n| {
            n.value()
                .attr("datetime")
                .and_then(date)
                .or_else(|| date(&clean(&n.inner_html())))
        });
        out.push((row, url));
    }
    if out.is_empty()
        && !body.contains("<html")
        && !body.contains("<body")
        && !body.contains("暂无")
    {
        return Err(ProviderError::Parse(
            "ahhhhfs: malformed search document".into(),
        ));
    }
    Ok(out)
}
fn detail(body: &str) -> Vec<crate::core::Link> {
    anchors(body, ".post-content a[href]")
        .into_iter()
        .filter(|link| {
            matches!(
                link.cloud_type,
                crate::core::CloudType::Quark
                    | crate::core::CloudType::Baidu
                    | crate::core::CloudType::Aliyun
                    | crate::core::CloudType::Uc
                    | crate::core::CloudType::Xunlei
                    | crate::core::CloudType::Tianyi
                    | crate::core::CloudType::One15
                    | crate::core::CloudType::Pan123
                    | crate::core::CloudType::Pikpak
            )
        })
        .collect()
}
#[async_trait]
impl Provider for AhhhhfsProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("ahhhhfs", 2, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self.base.clone();
        url.query_pairs_mut().append_pair("cat", "");
        url.query_pairs_mut().append_pair("s", query);
        let body = fetch(
            ctx,
            headers(ctx.client.get(url), self.base.as_str()),
            "ahhhhfs",
            "search",
            Duration::from_secs(10),
            16 << 20,
            &[200, 400],
        )
        .await?;
        let items = items(&body, &self.base)?;
        let results = stream::iter(items.into_iter().map(|(mut row, url)| async move {
            let body = fetch(
                ctx,
                headers(ctx.client.get(url.clone()), self.base.as_str()),
                "ahhhhfs",
                "detail",
                Duration::from_secs(8),
                16 << 20,
                &[],
            )
            .await?;
            row.links = detail(&body);
            if row.links.is_empty() {
                return Err(ProviderError::Parse(
                    "ahhhhfs: missing supported detail links".into(),
                ));
            }
            for link in &mut row.links {
                link.work_title = Some(row.title.clone());
            }
            if matches(&row, query) {
                ctx.publish(vec![row.clone()]);
                Ok(Some(row))
            } else {
                Ok(None)
            }
        }))
        .buffer_unordered(15)
        .collect::<Vec<Result<Option<SearchResult>, ProviderError>>>()
        .await;
        let mut out = Vec::new();
        let mut error = None;
        for result in results {
            match result {
                Ok(Some(row)) => out.push(row),
                Ok(None) => {}
                Err(e) => {
                    error.get_or_insert(e);
                }
            }
        }
        if let Some(error) = error {
            return Err(error);
        }
        Ok(out)
    }
}
