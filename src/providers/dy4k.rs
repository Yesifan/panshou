//! 4KDY current Tailwind cards and legacy hl-* templates, followed by real downloads.
use super::{UA, clean, fetch, html_links};
use crate::core::{ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};
use url::Url;
pub struct Dy4kProvider {
    base: Url,
    cache: Mutex<HashMap<String, (Instant, SearchResult)>>,
}
impl Default for Dy4kProvider {
    fn default() -> Self {
        Self::new(Url::parse("https://4kdy.vip/").unwrap())
    }
}
impl Dy4kProvider {
    pub fn new(base: Url) -> Self {
        Self {
            base,
            cache: Mutex::default(),
        }
    }
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn text(n: ElementRef<'_>, s: &str) -> String {
    n.select(&sel(s))
        .next()
        .map(|n| clean(&n.inner_html()))
        .unwrap_or_default()
}
fn items(body: &str, base: &Url) -> Result<(Vec<(Url, SearchResult)>, usize), ProviderError> {
    static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/4K-detail/(\d+)\.html").unwrap());
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let legacy = doc.select(&sel(".hl-list-item")).collect::<Vec<_>>();
    let mut add = |node: ElementRef<'_>, a: ElementRef<'_>, title: String| {
        let Some(url) = a.value().attr("href").and_then(|s| base.join(s).ok()) else {
            return;
        };
        let Some(id) = ID.captures(url.path()).map(|c| c[1].to_owned()) else {
            return;
        };
        if title.is_empty() || !seen.insert(id.clone()) {
            return;
        }
        let mut r = SearchResult::provider(format!("dy4k-{id}"), "dy4k");
        r.title = title;
        r.content = clean(&node.inner_html());
        r.tags = node
            .select(&sel(".hl-pic-text .remarks, .hl-item-sub, div.gap-x-3"))
            .map(|n| clean(&n.inner_html()))
            .filter(|s| !s.is_empty())
            .collect();
        if let Some(u) = node
            .select(&sel("img, .hl-item-thumb"))
            .next()
            .and_then(|n| {
                n.value()
                    .attr("data-original")
                    .or_else(|| n.value().attr("src"))
            })
            .and_then(|s| base.join(s).ok())
        {
            r.images.push(u.to_string());
        }
        out.push((url, r));
    };
    if !legacy.is_empty() {
        for n in legacy {
            if let Some(a) = n.select(&sel(".hl-item-pic a")).next() {
                add(n, a, text(n, ".hl-item-title a"));
            }
        }
    } else if let Some(container) = doc.select(&sel("div.space-y-4")).next() {
        for a in container.select(&sel("a[href*='/4K-detail/']")) {
            let Some(card) = a
                .parent()
                .and_then(|n| n.parent())
                .and_then(ElementRef::wrap)
            else {
                continue;
            };
            if card
                .select(&sel("a[href^='/4K-vodplay/']"))
                .next()
                .is_none()
            {
                continue;
            }
            add(card, a, clean(&a.inner_html()));
        }
    }
    let pages = text(doc.root_element(), ".hl-page-tips a")
        .split('/')
        .nth(1)
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 10);
    if out.is_empty()
        && doc
            .select(&sel(
                "div.space-y-4, .hl-list-wrap, .hl-page-tips, .hl-list-item",
            ))
            .next()
            .is_none()
    {
        return Err(ProviderError::Parse(
            "dy4k search: missing result structure".into(),
        ));
    }
    Ok((out, pages))
}
fn detail(body: &str, mut r: SearchResult, base: &Url) -> Option<SearchResult> {
    let doc = Html::parse_document(body);
    let root = doc.root_element();
    let title = text(root, "h1.title-mobile, h2.hl-dc-title");
    if !title.is_empty() {
        r.title = title;
    }
    let content = text(
        root,
        ".info-card-mobile [class*='leading-relaxed'], .hl-content-wrap .hl-content-text",
    );
    if !content.is_empty() {
        r.content = content;
    }
    let mut seen = HashSet::new();
    for node in doc.select(&sel(".download-item, .hl-downs-list, .hl-rb-playlist")) {
        r.links.extend(
            html_links(node)
                .into_iter()
                .filter(|l| seen.insert(l.url.clone())),
        );
    }
    let fragment = Html::parse_fragment(&root.text().collect::<Vec<_>>().join(" "));
    r.links.extend(
        html_links(fragment.root_element())
            .into_iter()
            .filter(|l| seen.insert(l.url.clone())),
    );
    if r.links.is_empty() {
        return None;
    }
    if let Some(u) = doc
        .select(&sel(
            ".info-card-mobile img[data-original], .hl-dc-pic .hl-item-thumb",
        ))
        .next()
        .and_then(|n| n.value().attr("data-original"))
        .and_then(|s| base.join(s).ok())
    {
        r.images = vec![u.to_string()];
    }
    r.tags.extend(
        doc.select(&sel(".hl-vod-data ul li"))
            .map(|n| clean(&n.inner_html()))
            .filter(|s| !s.is_empty()),
    );
    Some(r)
}
#[async_trait]
impl Provider for Dy4kProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("dy4k", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        q: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let get = |url: Url, stage: &'static str| {
            fetch(
                ctx.client
                    .get(url)
                    .header("User-Agent", UA)
                    .header("Referer", self.base.as_str()),
                "dy4k",
                stage,
                ctx.request_timeout(Duration::from_secs(15)),
                16 << 20,
                &[200, 400],
            )
        };
        let search_url = |page: usize| {
            let mut url = self.base.join("4K-search/-------------.html").unwrap();
            url.query_pairs_mut().append_pair("wd", q);
            if page > 1 {
                url.query_pairs_mut().append_pair("page", &page.to_string());
            }
            url
        };
        let (mut entries, pages) = items(&get(search_url(1), "search").await?, &self.base)?;
        let mut page_error = None;
        for page in 2..=pages {
            match get(search_url(page), "search page")
                .await
                .and_then(|body| items(&body, &self.base))
            {
                Ok((more, _)) => entries.extend(more),
                Err(error) if !entries.is_empty() => {
                    page_error = Some(error);
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        let mut seen = HashSet::new();
        entries.retain(|(_, r)| seen.insert(r.id.clone()));
        let failures = &std::sync::Mutex::new(None);
        let rows = stream::iter(entries.into_iter().map(|(url, r)| async move {
            let cached = self
                .cache
                .lock()
                .unwrap()
                .get(url.as_str())
                .filter(|(at, _)| at.elapsed() < Duration::from_secs(3600))
                .map(|(_, r)| r.clone());
            if let Some(r) = cached {
                ctx.publish(vec![r.clone()]);
                return Some(r);
            }
            let body = super::remember_failure(get(url.clone(), "detail").await, failures)?;
            let r = detail(&body, r, &self.base)?;
            let mut cache = self.cache.lock().unwrap();
            if cache.len() >= 512
                && let Some(k) = cache
                    .iter()
                    .min_by_key(|(_, v)| v.0)
                    .map(|(k, _)| k.clone())
            {
                cache.remove(&k);
            }
            cache.insert(url.to_string(), (Instant::now(), r.clone()));
            drop(cache);
            ctx.publish(vec![r.clone()]);
            Some(r)
        }))
        .buffered(6)
        .collect::<Vec<_>>()
        .await;
        if let Some(error) = page_error {
            return Err(error);
        }
        if let Some(error) = failures.lock().unwrap().take() {
            return Err(error);
        }
        if rows.iter().any(Option::is_none) {
            return Err(ProviderError::Unavailable(
                "dy4k: one or more details failed".into(),
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
        let base = Url::parse("https://4kdy.vip/").unwrap();
        let (mut rows, pages) = items(
            include_str!("../../tests/fixtures/providers/dy4k/success.html"),
            &base,
        )
        .unwrap();
        assert_eq!(pages, 1);
        let (_, r) = rows.remove(0);
        assert_eq!(
            detail(
                include_str!("../../tests/fixtures/providers/dy4k/detail.html"),
                r,
                &base
            )
            .unwrap()
            .links
            .len(),
            1
        );
        assert!(
            items(
                include_str!("../../tests/fixtures/providers/dy4k/empty.html"),
                &base
            )
            .unwrap()
            .0
            .is_empty()
        );
        assert!(
            items(
                include_str!("../../tests/fixtures/providers/dy4k/malformed.html"),
                &base
            )
            .is_err()
        );
    }
}
