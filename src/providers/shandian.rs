use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, support::*};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use std::sync::LazyLock;
use url::Url;
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/id/(\d+)\.html").unwrap());
#[derive(Debug, Clone)]
pub struct Shandian {
    pub bases: Vec<Url>,
}
impl Default for Shandian {
    fn default() -> Self {
        Self {
            bases: [
                "http://sduc.cloud/",
                "http://shandian.blog/",
                "http://sd.sduc.site/",
            ]
            .into_iter()
            .map(|s| Url::parse(s).unwrap())
            .collect(),
        }
    }
}
fn items(bytes: &[u8]) -> Result<Vec<(String, SearchResult)>, ProviderError> {
    let doc = html(bytes)?;
    let mut rows = vec![];
    for node in doc.select(&sel(".module-search-item")) {
        let Some(href) = attr(node, ".module-item-pic a", "href") else {
            continue;
        };
        let Some(id) = ID
            .captures(&href)
            .and_then(|c| c.get(1))
            .map(|s| s.as_str().to_owned())
        else {
            continue;
        };
        let title = text(node, ".video-info-header h3 a");
        if title.is_empty() {
            continue;
        }
        let mut row = SearchResult::provider(format!("shandian-{id}"), "shandian");
        row.title = title;
        row.tags = node
            .select(&sel(".video-info-aux .tag-link a"))
            .map(|n| clean(&n.text().collect::<String>()))
            .filter(|s| !s.is_empty())
            .collect();
        let mut parts = vec![];
        let quality = text(node, ".video-serial");
        if !quality.is_empty() {
            parts.push(format!("【{quality}】"));
        }
        for item in node.select(&sel(".video-info-items")) {
            let label = text(item, ".video-info-itemtitle");
            if label.contains("导演") {
                parts.push(format!("导演：{}", text(item, ".video-info-actor a")));
            } else if label.contains("主演") {
                let actors: Vec<_> = item
                    .select(&sel(".video-info-actor a"))
                    .map(|n| clean(&n.text().collect::<String>()))
                    .collect();
                let suffix = if actors.len() > 3 { "等" } else { "" };
                parts.push(format!(
                    "主演：{}{suffix}",
                    actors.into_iter().take(3).collect::<Vec<_>>().join("、")
                ));
            } else if label.contains("剧情") {
                parts.push(text(item, ".video-info-item"));
            }
        }
        row.content = parts.join("\n");
        rows.push((id, row));
    }
    Ok(rows)
}
fn links(bytes: &[u8]) -> Result<Vec<Link>, ProviderError> {
    let doc = html(bytes)?;
    let mut links = vec![];
    for node in doc.select(&sel("#download-list .module-row-one")) {
        let mut urls = vec![];
        if let Some(raw) = attr(node, "[data-clipboard-text]", "data-clipboard-text") {
            urls.push(raw);
        }
        urls.extend(
            node.select(&sel("a[href]"))
                .filter_map(|n| n.value().attr("href").map(str::to_owned)),
        );
        for raw in urls {
            if raw.contains('#') {
                continue;
            }
            let link = Link::new(raw);
            if link.cloud_type == CloudType::Uc
                && link.url.contains("/s/")
                && !links.iter().any(|l: &Link| l.url == link.url)
            {
                links.push(link);
            }
        }
    }
    Ok(links)
}
impl Shandian {
    async fn search_base(
        &self,
        ctx: &SearchContext,
        base: &Url,
        query: &str,
    ) -> Result<Vec<(String, SearchResult)>, ProviderError> {
        let encoded = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("wd", query)
            .finish();
        let url = base
            .join(&format!("index.php/vod/search/wd/{}.html", &encoded[3..]))
            .unwrap();
        let bytes = body(
            ctx,
            || ctx.client.get(url.clone()).header("referer", base.as_str()),
            8,
            STANDARD,
        )
        .await?;
        items(&bytes)
    }
    async fn details(
        &self,
        ctx: &SearchContext,
        base: Url,
        tasks: Vec<(String, SearchResult)>,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        collect(
            ctx,
            stream::iter(tasks.into_iter().map(|(id, mut row)| {
                let base = base.clone();
                async move {
                    let url = base
                        .join(&format!("index.php/vod/detail/id/{id}.html"))
                        .unwrap();
                    let bytes = body(
                        ctx,
                        || ctx.client.get(url.clone()).header("referer", base.as_str()),
                        6,
                        STANDARD,
                    )
                    .await?;
                    row.links = links(&bytes)?;
                    if row.links.is_empty() {
                        return Err(ProviderError::Protocol(
                            "shandian detail has no UC links".into(),
                        ));
                    }
                    Ok(vec![row])
                }
            }))
            .buffer_unordered(20),
        )
        .await
    }
}
#[async_trait]
impl Provider for Shandian {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("shandian", 2, KeywordFilterMode::Core)
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
        let mut candidates = stream::iter(self.bases.clone().into_iter().map(|base| async move {
            let result = self.search_base(ctx, &base, query).await;
            (base, result)
        }))
        .buffer_unordered(3);
        let mut successful_empty = false;
        let mut last_error = None;
        let mut selected = None;
        while let Some((base, result)) = candidates.next().await {
            match result {
                Ok(tasks) if !tasks.is_empty() => {
                    selected = Some((base, tasks));
                    break;
                }
                Ok(_) => successful_empty = true,
                Err(error) => last_error = Some(error),
            }
        }
        // Mirror requests are alternatives, not extra sources or detached work.
        drop(candidates);
        match selected {
            Some((base, tasks)) => self.details(ctx, base, tasks).await,
            None if successful_empty => Ok(vec![]),
            None => Err(last_error
                .unwrap_or_else(|| ProviderError::Protocol("shandian has no mirrors".into()))),
        }
    }
}
