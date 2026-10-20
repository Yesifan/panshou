//! daishudj: fixed upstream WordPress search and source-specific detail DOM.
use super::{anchors, clean, date, fetch, headers, matches, raw_links, sel, text};
use crate::core::{ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::Html;
use std::{collections::HashSet, sync::LazyLock, time::Duration};
use url::Url;
#[derive(Debug, Clone)]
pub struct DaishudjProvider {
    base: Url,
}
impl Default for DaishudjProvider {
    fn default() -> Self {
        Self::new(Url::parse("https://www.daishuduanju.com/").unwrap())
    }
}
impl DaishudjProvider {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
}
fn items(body: &str, base: &Url) -> Result<Vec<(SearchResult, Url)>, ProviderError> {
    static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/(\d+)/").unwrap());
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    if doc.select(&sel(".item-jx.item-blog")).next().is_none()
        && doc.select(&sel("body")).next().is_none()
    {
        return Err(ProviderError::Parse("daishudj: missing document".into()));
    }
    for node in doc.select(&sel(".item-jx.item-blog")) {
        let Some(a) = node.select(&sel(".subtitle h5 a")).next() else {
            continue;
        };
        let title = clean(&a.inner_html());
        let Some(url) = a.value().attr("href").and_then(|s| base.join(s).ok()) else {
            continue;
        };
        let Some(id) = ID.captures(url.as_str()).map(|c| c[1].to_owned()) else {
            continue;
        };
        if title.is_empty() || !seen.insert(id.clone()) {
            continue;
        }
        let mut row = SearchResult::provider(format!("daishudj-{id}"), "daishudj");
        row.title = title;
        row.content = text(node, ".subtitle p.pdesc");
        row.tags = node
            .select(&sel(".sortbox a.sort"))
            .map(|n| clean(&n.inner_html()))
            .filter(|s| !s.is_empty())
            .collect();
        row.datetime = node
            .select(&sel("time"))
            .next()
            .and_then(|n| n.value().attr("datetime"))
            .and_then(date)
            .or_else(|| date(&text(node, ".pmbox .time")));
        out.push((row, url));
    }
    if out.is_empty()
        && !body.contains("<html")
        && !body.contains("<body")
        && !body.contains("暂无")
    {
        return Err(ProviderError::Parse(
            "daishudj: malformed search document".into(),
        ));
    }
    Ok(out)
}
fn detail(body: &str) -> Vec<crate::core::Link> {
    let links = anchors(body, ".article-body a[href], article.post a[href]");
    let doc = Html::parse_document(body);
    let container = doc
        .select(&sel(".article-body"))
        .next()
        .or_else(|| doc.select(&sel("article.post")).next());
    let scope = container
        .map(|n| n.inner_html())
        .unwrap_or_else(|| body.to_owned());
    let mut links = if links.is_empty() {
        anchors(&scope, "a[href]")
    } else {
        links
    };
    for link in raw_links(&clean(&scope)) {
        if !links.iter().any(|l| l.url == link.url) {
            links.push(link);
        }
    }
    links
        .into_iter()
        .filter(|link| {
            matches!(
                link.cloud_type,
                crate::core::CloudType::Quark
                    | crate::core::CloudType::Aliyun
                    | crate::core::CloudType::Baidu
                    | crate::core::CloudType::Xunlei
                    | crate::core::CloudType::Uc
                    | crate::core::CloudType::Pikpak
                    | crate::core::CloudType::Mobile
                    | crate::core::CloudType::Magnet
                    | crate::core::CloudType::Pan123
            )
        })
        .collect()
}
#[async_trait]
impl Provider for DaishudjProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("daishudj", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self.base.clone();
        url.query_pairs_mut().append_pair("s", query);
        let body = fetch(
            ctx,
            headers(ctx.client.get(url), self.base.as_str()),
            "daishudj",
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
                "daishudj",
                "detail",
                Duration::from_secs(8),
                16 << 20,
                &[200, 400],
            )
            .await?;
            row.links = detail(&body);
            if row.links.is_empty() {
                return Err(ProviderError::Parse(
                    "daishudj: missing supported detail links".into(),
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
        .buffer_unordered(10)
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
