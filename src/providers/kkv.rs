//! KKV WordPress search (?p=ID), paragraph-scoped cloud links.
use super::support::*;
use crate::{
    core::{CloudType, ProviderError, SearchResult},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use std::sync::LazyLock;
use url::Url;
#[derive(Debug, Clone)]
pub struct Kkv {
    base: Url,
}
fn paragraph_password(text: &str) -> Option<String> {
    static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
        [
            r"提取码[：:]\s*([a-zA-Z0-9]{4})",
            r"密码[：:]\s*([a-zA-Z0-9]{4})",
            r"pwd[：:]\s*([a-zA-Z0-9]{4})",
        ]
        .into_iter()
        .map(|s| Regex::new(s).unwrap())
        .collect()
    });
    PATTERNS
        .iter()
        .find_map(|pattern| pattern.captures(text).map(|capture| capture[1].to_owned()))
}
impl Default for Kkv {
    fn default() -> Self {
        Self::new(Url::parse("http://kkv.q-23.cn/").unwrap())
    }
}
impl Kkv {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\?p=(\d+)").unwrap());
        let mut url = self.base.clone();
        url.query_pairs_mut().append_pair("s", query);
        let body = text(
            ctx,
            browser(ctx.client.get(url), self.base.as_str(), HTML),
            3,
            30,
        )
        .await?;
        let items = {
            let doc = require_document(
                &body,
                "article.post, #main, .site-main, .no-results, .search-no-results",
                "kkv",
            )?;
            doc.select(&sel("article.post .entry-header h2.entry-title a[href]"))
                .filter_map(|a| {
                    let url = self.base.join(a.value().attr("href")?).ok()?;
                    let id = ID.captures(url.as_str())?[1].to_owned();
                    let title = text_of(a);
                    title
                        .to_lowercase()
                        .contains(&query.to_lowercase())
                        .then_some((id, title, url))
                })
                .take(10)
                .collect::<Vec<_>>()
        };
        let mut jobs = stream::iter(items.into_iter().map(|(id, title, url)| async move {
            let body = text(
                ctx,
                browser(ctx.client.get(url), self.base.as_str(), HTML),
                3,
                30,
            )
            .await?;
            let doc = require_document(&body, ".entry-content", "kkv detail")?;
            let mut row = SearchResult::provider(format!("kkv-{id}"), "kkv");
            row.title = first_text(&doc, ".entry-header h1.entry-title");
            if row.title.is_empty() {
                row.title = title;
            }
            row.content = short(&first_text(&doc, ".entry-content p"), 200);
            row.datetime = attr(&doc, "time.updated", "datetime").and_then(|v| date(&v));
            for p in doc.select(&sel(".entry-content p")) {
                for mut link in links_in(p) {
                    if !matches!(
                        link.cloud_type,
                        CloudType::Baidu
                            | CloudType::Quark
                            | CloudType::Uc
                            | CloudType::Xunlei
                            | CloudType::Aliyun
                            | CloudType::Tianyi
                            | CloudType::One15
                            | CloudType::Pan123
                            | CloudType::Mobile
                            | CloudType::Pikpak
                    ) {
                        continue;
                    }
                    link.password = query_password(&link.url)
                        .filter(|v| v.len() == 4)
                        .or_else(|| paragraph_password(&text_of(p)));
                    row.links.push(link);
                }
            }
            Ok::<_, ProviderError>(publish(ctx, row, query))
        }))
        .buffer_unordered(3);
        let mut out = Vec::new();
        let mut error = None;
        while let Some(item) = jobs.next().await {
            match item {
                Ok(Some(row)) => out.push(row),
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
impl Provider for Kkv {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("kkv", 3, KeywordFilterMode::Core)
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
