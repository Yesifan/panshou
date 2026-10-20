//! 低端影视: current movie cards, legacy posts, and lazy atob share links.
use super::{anchors, clean, date, fetch, headers, matches, password, sel, text};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{ElementRef, Html};
use std::{collections::HashSet, sync::LazyLock, time::Duration};
use url::Url;
#[derive(Debug, Clone)]
pub struct DiduanProvider {
    base: Url,
}
impl Default for DiduanProvider {
    fn default() -> Self {
        Self::new(Url::parse("https://ddys.io/").unwrap())
    }
}
impl DiduanProvider {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
}
fn items(body: &str, base: &Url) -> Result<Vec<(SearchResult, Url)>, ProviderError> {
    static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"post-(\d+)").unwrap());
    let doc = Html::parse_document(body);
    let heading = doc
        .select(&sel("h2"))
        .find(|n| clean(&n.inner_html()).starts_with("影视"));
    let container = heading
        .and_then(|n| n.parent())
        .and_then(|n| n.parent())
        .and_then(ElementRef::wrap);
    let mut cards = container
        .map(|n| n.select(&sel(".movie-card")).collect::<Vec<_>>())
        .unwrap_or_default();
    if cards.is_empty() {
        cards = doc.select(&sel("article[class*='post-']")).collect();
    }
    if cards.is_empty() && !body.contains("<html") && !body.contains("<body") {
        return Err(ProviderError::Parse(
            "diduan: malformed search document".into(),
        ));
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for node in cards {
        let current = node
            .select(&sel("h3 a[href^='/movie/'], a[href^='/movie/']"))
            .next();
        let a = current.or_else(|| node.select(&sel(".post-title a")).next());
        let Some(a) = a else {
            continue;
        };
        let Some(url) = a.value().attr("href").and_then(|s| base.join(s).ok()) else {
            continue;
        };
        if !seen.insert(url.clone()) {
            continue;
        }
        let title = clean(&a.inner_html());
        if title.is_empty() {
            continue;
        }
        let id = if current.is_some() {
            url.path()
                .trim_start_matches("/movie/")
                .trim_matches('/')
                .to_owned()
        } else {
            ID.captures(node.value().attr("class").unwrap_or(""))
                .map(|c| c[1].to_owned())
                .unwrap_or_else(|| out.len().to_string())
        };
        let mut row = SearchResult::provider(format!("diduan-{id}"), "diduan");
        row.title = title;
        row.content = if current.is_some() {
            text(node, "h3")
        } else {
            text(node, ".entry-content")
        }
        .chars()
        .take(200)
        .collect();
        row.datetime = node
            .select(&sel(".meta_date time.entry-date"))
            .next()
            .and_then(|n| n.value().attr("datetime"))
            .and_then(date);
        row.tags = node
            .select(&sel(".meta_categories .cat-links a"))
            .map(|n| clean(&n.inner_html()))
            .collect();
        row.images = node
            .select(&sel("img[src]"))
            .filter_map(|n| n.value().attr("src").map(str::to_owned))
            .collect();
        out.push((row, url));
        if out.len() == 50 {
            break;
        }
    }
    Ok(out)
}
fn detail(body: &str) -> Vec<Link> {
    static ENCODED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?i)atob\(\s*['\"]([A-Za-z0-9+/=_-]+)['\"]\s*\)"#).unwrap());
    let mut links = anchors(body, "a[href]");
    for capture in ENCODED.captures_iter(body) {
        let Ok(bytes) = STANDARD.decode(capture[1].replace('-', "+")) else {
            continue;
        };
        let Ok(raw) = String::from_utf8(bytes) else {
            continue;
        };
        let raw = raw.trim();
        let mut link = Link::new(raw);
        if link.cloud_type == CloudType::Others || links.iter().any(|l| l.url == raw) {
            continue;
        }
        link.password = password(raw, "");
        links.push(link);
    }
    links
}
#[async_trait]
impl Provider for DiduanProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("diduan", 1, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self.base.join("search").unwrap();
        url.query_pairs_mut().append_pair("q", query);
        let body = fetch(
            ctx,
            headers(ctx.client.get(url), self.base.as_str()),
            "diduan",
            "search",
            Duration::from_secs(30),
            16 << 20,
            &[],
        )
        .await?;
        let results = stream::iter(items(&body, &self.base)?.into_iter().map(
            |(mut row, url)| async move {
                let body = fetch(
                    ctx,
                    headers(ctx.client.get(url), self.base.as_str()),
                    "diduan",
                    "detail",
                    Duration::from_secs(30),
                    16 << 20,
                    &[],
                )
                .await?;
                row.links = detail(&body);
                if row.links.is_empty() {
                    return Err(ProviderError::Parse(
                        "diduan: missing supported detail links".into(),
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
            },
        ))
        .buffer_unordered(20)
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
