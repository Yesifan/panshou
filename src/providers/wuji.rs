use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, support::*};
use crate::core::{Link, ProviderError, SearchResult};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use std::sync::LazyLock;
use url::Url;
static ADS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [r"【[^】]*】", r"^\d+【[^】]*】", r"\[[^\]]*\]", r"\s+"]
        .into_iter()
        .map(|s| Regex::new(s).unwrap())
        .collect()
});
#[derive(Debug, Clone)]
pub struct Wuji {
    pub base: Url,
}
impl Default for Wuji {
    fn default() -> Self {
        Self {
            base: Url::parse("https://xcili.net/").unwrap(),
        }
    }
}
fn items(bytes: &[u8], base: &Url) -> Result<Vec<(Url, SearchResult)>, ProviderError> {
    let doc = html(bytes)?;
    let mut rows = vec![];
    for node in doc.select(&sel("table.file-list tbody tr")) {
        let Some(anchor) = node.select(&sel("td a[href]")).next() else {
            continue;
        };
        let href = anchor.value().attr("href").unwrap();
        let Ok(url) = base.join(href) else { continue };
        let sample = text(anchor, "p.sample");
        let mut title = anchor.text().collect::<String>().replacen(&sample, "", 1);
        for re in ADS.iter() {
            title = re.replace_all(&title, " ").into_owned();
        }
        let title = clean(&title);
        if title.is_empty() {
            continue;
        }
        let mut row = SearchResult::provider(format!("wuji-{}", url.path()), "wuji");
        row.title = title;
        row.tags = vec!["magnet".into()];
        row.content = [(!sample.is_empty()).then(|| format!("文件: {sample}")), {
            let size = text(node, "td.td-size");
            (!size.is_empty()).then(|| format!("大小: {size}"))
        }]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
        rows.push((url, row));
    }
    Ok(rows)
}
impl Wuji {
    async fn resolve(
        &self,
        ctx: &SearchContext,
        tasks: Vec<(Url, SearchResult)>,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        collect(
            ctx,
            stream::iter(tasks.into_iter().map(|(url, mut row)| async move {
                let bytes = body(
                    ctx,
                    || {
                        ctx.client
                            .get(url.clone())
                            .header("referer", self.base.as_str())
                    },
                    30,
                    WUJI,
                )
                .await?;
                let raw = {
                    let doc = html(&bytes)?;
                    attr(doc.root_element(), "input#input-magnet", "value")
                };
                let raw = raw.filter(|s| magnet(s)).ok_or_else(|| {
                    ProviderError::Protocol("wuji detail has no valid magnet".into())
                })?;
                row.links = vec![Link::new(raw)];
                Ok(vec![row])
            }))
            .buffer_unordered(10),
        )
        .await
    }
    async fn page(
        &self,
        ctx: &SearchContext,
        query: &str,
        page: usize,
    ) -> Result<Vec<(Url, SearchResult)>, ProviderError> {
        let mut url = self.base.join("search").unwrap();
        url.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("page", &page.to_string());
        let bytes = body(
            ctx,
            || {
                ctx.client
                    .get(url.clone())
                    .header("referer", self.base.as_str())
            },
            30,
            WUJI,
        )
        .await?;
        items(&bytes, &self.base)
    }
}
#[async_trait]
impl Provider for Wuji {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("wuji", 3, KeywordFilterMode::Core)
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
        let first = self.page(ctx, query, 1).await?;
        let mut rows = self.resolve(ctx, first).await?;
        let more = collect(
            ctx,
            stream::iter((2..=5).map(|page| async move {
                tokio::time::sleep(std::time::Duration::from_millis((page % 3) * 100)).await;
                let tasks = self.page(ctx, query, page as usize).await?;
                self.resolve(ctx, tasks).await
            }))
            .buffer_unordered(1),
        )
        .await?;
        rows.extend(more);
        Ok(rows)
    }
}
