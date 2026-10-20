//! Labi MacCMS mirrors: race search responses, then resolve only the winning site's downloads.
use super::support::*;
use crate::{
    core::{CloudType, Link, ProviderError, SearchResult},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use std::{collections::HashSet, sync::LazyLock};
use url::Url;
#[derive(Debug, Clone)]
pub struct Labi {
    bases: Vec<Url>,
}
impl Default for Labi {
    fn default() -> Self {
        Self::new(
            [
                "http://www.xiaocgege.shop/",
                "http://feimo.fun/",
                "http://xiaocgege.shop/",
            ]
            .into_iter()
            .map(|s| Url::parse(s).unwrap())
            .collect(),
        )
    }
}
impl Labi {
    pub fn new(bases: Vec<Url>) -> Self {
        Self { bases }
    }
    async fn at_base(
        &self,
        ctx: &SearchContext,
        base: Url,
        query: &str,
    ) -> Result<(Url, Vec<SearchResult>), ProviderError> {
        static ID: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"/vod/detail/id/(\d+)\.html").unwrap());
        let encoded = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
        let url = base
            .join(&format!("index.php/vod/search/wd/{encoded}.html"))
            .map_err(|_| protocol("labi invalid search URL"))?;
        let body = text(
            ctx,
            browser(ctx.client.get(url), base.as_str(), HTML)
                .header("upgrade-insecure-requests", "1")
                .header("cache-control", "max-age=0"),
            3,
            8,
        )
        .await?;
        let doc = require_document(
            &body,
            ".module-search-item, .module-search, .module-main, .module-items, .search-empty",
            "labi",
        )?;
        let mut rows = Vec::new();
        for node in doc.select(&sel(".module-search-item")) {
            let Some(a) = node.select(&sel(".module-item-pic a[href]")).next() else {
                continue;
            };
            let Some(id) = ID
                .captures(a.value().attr("href").unwrap_or(""))
                .map(|c| c[1].to_owned())
            else {
                continue;
            };
            let mut row = SearchResult::provider(format!("labi-{id}"), "labi");
            row.title = node
                .select(&sel(".video-info-header h3 a"))
                .next()
                .map(text_of)
                .unwrap_or_default();
            if row.title.is_empty() {
                continue;
            }
            row.tags = node
                .select(&sel(".video-info-aux .tag-link a"))
                .map(text_of)
                .filter(|v| !v.is_empty())
                .collect();
            let mut parts = Vec::new();
            if let Some(q) = node
                .select(&sel(".video-serial"))
                .next()
                .map(text_of)
                .filter(|v| !v.is_empty())
            {
                parts.push(format!("【{q}】"));
            }
            for item in node.select(&sel(".video-info-items")) {
                let label = item
                    .select(&sel(".video-info-itemtitle"))
                    .next()
                    .map(text_of)
                    .unwrap_or_default();
                if label.contains("导演") || label.contains("主演") {
                    let people = item
                        .select(&sel(".video-info-actor a"))
                        .map(text_of)
                        .take(if label.contains("主演") {
                            3
                        } else {
                            usize::MAX
                        })
                        .collect::<Vec<_>>()
                        .join("、");
                    if !people.is_empty() {
                        parts.push(format!("{label}{people}"));
                    }
                } else if label.contains("剧情")
                    && let Some(s) = item.select(&sel(".video-info-item")).next().map(text_of)
                {
                    parts.push(s);
                }
            }
            row.content = parts.join("\n");
            if let Some(image) = node
                .select(&sel(".module-item-pic > img"))
                .next()
                .and_then(|n| n.value().attr("data-src"))
                .and_then(|s| base.join(s).ok())
            {
                row.images.push(image.into());
            }
            rows.push(row);
        }
        Ok((base, rows))
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut mirrors = stream::iter(
            self.bases
                .clone()
                .into_iter()
                .map(|base| self.at_base(ctx, base, query)),
        )
        .buffer_unordered(3);
        let mut winner = None;
        let mut empty = false;
        let mut error = None;
        while let Some(result) = mirrors.next().await {
            match result {
                Ok((base, rows)) if !rows.is_empty() => {
                    winner = Some((base, rows));
                    break;
                }
                Ok(_) => empty = true,
                Err(e) => error = Some(e),
            }
        }
        drop(mirrors);
        let Some((base, rows)) = winner else {
            return if empty {
                Ok(Vec::new())
            } else {
                Err(error.unwrap_or_else(|| protocol("labi has no configured mirrors")))
            };
        };
        let mut jobs=stream::iter(rows.into_iter().map(|mut row|{let base=base.clone();async move{
            let id=row.id.strip_prefix("labi-").unwrap_or("");let url=base.join(&format!("index.php/vod/detail/id/{id}.html")).map_err(|_|protocol("labi invalid detail URL"))?;
            let body=text(ctx,browser(ctx.client.get(url),base.as_str(),HTML),3,6).await?;
            let doc=require_document(&body,"#download-list, .module-item-pic","labi detail")?;
            let mut seen=HashSet::new();
            for n in doc.select(&sel("#download-list .module-row-one [data-clipboard-text], #download-list .module-row-one a[href]")){
                let raw=n.value().attr("data-clipboard-text").or_else(||n.value().attr("href")).unwrap_or("").trim();
                let link=Link::new(raw);
                if !raw.contains('#') && link.cloud_type==CloudType::Quark && Url::parse(raw).is_ok() && seen.insert(raw.to_owned()){row.links.push(link);}
            }
            if let Some(image)=attr(&doc,".module-item-pic > img","data-src").and_then(|s|base.join(&s).ok()){row.images=vec![image.into()];}
            Ok::<_,ProviderError>(publish(ctx,row,query))
        }})).buffer_unordered(20);
        let mut out = Vec::new();
        let mut error = None;
        while let Some(item) = jobs.next().await {
            match item {
                Ok(Some(r)) => out.push(r),
                Ok(None) => {}
                Err(e) => error = Some(e),
            }
        }
        if let Some(e) = error {
            return Err(e);
        }
        Ok(out)
    }
}
#[async_trait]
impl Provider for Labi {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("labi", 1, KeywordFilterMode::Core)
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
