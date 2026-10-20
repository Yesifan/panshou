//! 电影优荐 Pro WordPress search and post-content resource parsing.
use super::{UA, clean, fetch, html_links};
use crate::core::{ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{Html, Selector};
use std::{sync::LazyLock, time::Duration};
use url::Url;
#[derive(Debug, Clone)]
pub struct DyyjproProvider {
    base: Url,
}
impl Default for DyyjproProvider {
    fn default() -> Self {
        Self::new(Url::parse("https://dyyjpro.com/").unwrap())
    }
}
impl DyyjproProvider {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn items(body: &str, base: &Url) -> Result<Vec<(Url, SearchResult)>, ProviderError> {
    static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/(\d+)\.html?$").unwrap());
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    for node in doc.select(&sel("article.post-item.item-grid")) {
        let Some(a) = node.select(&sel("h2.entry-title a")).next() else {
            continue;
        };
        let Some(url) = a.value().attr("href").and_then(|s| base.join(s).ok()) else {
            continue;
        };
        let Some(id) = ID.captures(url.path()).map(|c| c[1].to_owned()) else {
            continue;
        };
        let title = clean(&a.inner_html());
        if title.is_empty() {
            continue;
        }
        let mut r = SearchResult::provider(format!("dyyjpro-{id}"), "dyyjpro");
        r.title = title;
        if let Some(tag) = node.select(&sel("span.meta-cat-dot a")).next() {
            let tag = clean(&tag.inner_html());
            if !tag.is_empty() {
                r.tags.push(tag);
            }
        }
        r.datetime = node
            .select(&sel("time.pub-date"))
            .next()
            .and_then(|n| n.value().attr("datetime"))
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&chrono::Utc));
        out.push((url, r));
    }
    if out.is_empty()
        && doc
            .select(&sel(
                ".posts-wrapper, .site-main, .content-area, .post-item, .no-results",
            ))
            .next()
            .is_none()
    {
        return Err(ProviderError::Parse(
            "dyyjpro search: missing article structure".into(),
        ));
    }
    Ok(out)
}
fn detail(body: &str, mut r: SearchResult, base: &Url) -> Option<SearchResult> {
    let doc = Html::parse_document(body);
    let scope = doc
        .select(&sel("article.post-content"))
        .next()
        .unwrap_or_else(|| doc.root_element());
    r.links = html_links(scope);
    if r.links.is_empty() {
        return None;
    }
    r.content = clean(&scope.inner_html()).chars().take(300).collect();
    if let Some(u) = doc
        .select(&sel("div.archive-hero-bg[data-bg]"))
        .next()
        .and_then(|n| n.value().attr("data-bg"))
        .and_then(|s| base.join(s).ok())
    {
        r.images.push(u.to_string());
    }
    if let Some(u) = scope
        .select(&sel("img[src]"))
        .next()
        .and_then(|n| n.value().attr("src"))
        .and_then(|s| base.join(s).ok())
        && !r.images.contains(&u.to_string())
    {
        r.images.push(u.to_string());
    }
    Some(r)
}
#[async_trait]
impl Provider for DyyjproProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("dyyjpro", 2, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        q: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let get = |url: Url, stage: &'static str, seconds| {
            fetch(
                ctx.client
                    .get(url)
                    .header("User-Agent", UA)
                    .header("Referer", self.base.as_str()),
                "dyyjpro",
                stage,
                ctx.request_timeout(Duration::from_secs(seconds)),
                16 << 20,
                &[200, 400],
            )
        };
        let mut url = self.base.clone();
        url.query_pairs_mut()
            .append_pair("cat", "")
            .append_pair("s", q);
        let entries = items(&get(url, "search", 15).await?, &self.base)?;
        let failures = &std::sync::Mutex::new(None);
        let rows = stream::iter(entries.into_iter().map(|(url, r)| async move {
            let body = super::remember_failure(get(url, "detail", 10).await, failures)?;
            let r = detail(&body, r, &self.base)?;
            ctx.publish(vec![r.clone()]);
            Some(r)
        }))
        .buffered(12)
        .collect::<Vec<_>>()
        .await;
        if let Some(error) = failures.lock().unwrap().take() {
            return Err(error);
        }
        if rows.iter().any(Option::is_none) {
            return Err(ProviderError::Unavailable(
                "dyyjpro: one or more details failed or contained no usable links".into(),
            ));
        }
        Ok(rows.into_iter().flatten().collect())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixtures() {
        let base = Url::parse("https://dyyjpro.com/").unwrap();
        let (_, r) = items(
            include_str!("../../tests/fixtures/providers/dyyjpro/success.html"),
            &base,
        )
        .unwrap()
        .remove(0);
        assert_eq!(
            detail(
                include_str!("../../tests/fixtures/providers/dyyjpro/detail.html"),
                r,
                &base
            )
            .unwrap()
            .links[0]
                .password
                .as_deref(),
            Some("abcd")
        );
        assert!(
            items(
                include_str!("../../tests/fixtures/providers/dyyjpro/empty.html"),
                &base
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            items(
                include_str!("../../tests/fixtures/providers/dyyjpro/malformed.html"),
                &base
            )
            .is_err()
        );
    }
}
