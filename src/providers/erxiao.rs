//! Original 二小盘 mirror adapter; independent of the new erxiaopan site.
//! Huban uses the identical upstream MacCMS search/download protocol.
use super::{UA, clean, fetch, password};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{Html, Selector};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};
use url::Url;
#[derive(Clone)]
pub(crate) struct MacCms {
    pub name: &'static str,
    pub bases: Vec<Url>,
    cache: Arc<Mutex<HashMap<String, (Instant, SearchResult)>>>,
}
impl MacCms {
    pub fn new(name: &'static str, bases: Vec<Url>) -> Self {
        Self {
            name,
            bases,
            cache: Arc::default(),
        }
    }
}
pub struct ErxiaoProvider {
    inner: MacCms,
}
impl Default for ErxiaoProvider {
    fn default() -> Self {
        Self::new(
            ["https://www.wexwp.cc/", "https://www.2xiaopan.top/"]
                .iter()
                .map(|s| Url::parse(s).unwrap())
                .collect(),
        )
    }
}
impl ErxiaoProvider {
    pub fn new(bases: Vec<Url>) -> Self {
        Self {
            inner: MacCms::new("erxiao", bases),
        }
    }
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn items(body: &str, base: &Url, name: &str) -> Result<Vec<(Url, SearchResult)>, ProviderError> {
    static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/id/(\d+)").unwrap());
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    for node in doc.select(&sel(".module-search-item")) {
        let Some(a) = node.select(&sel(".video-info-header h3 a")).next() else {
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
        let mut r = SearchResult::provider(format!("{name}-{id}"), name);
        r.title = title;
        r.content = clean(&node.inner_html());
        for n in node.select(&sel(".video-info-items")) {
            let label = n
                .select(&sel(".video-info-itemtitle"))
                .next()
                .map(|n| clean(&n.inner_html()))
                .unwrap_or_default();
            if label.contains("类别") || label.contains("分类") {
                r.tags.extend(
                    n.select(&sel(".video-info-item"))
                        .map(|n| clean(&n.inner_html()))
                        .filter(|s| !s.is_empty()),
                );
            }
        }
        if let Some(image) = node
            .select(&sel(".module-item-pic > img"))
            .next()
            .and_then(|n| n.value().attr("data-src"))
            .and_then(|s| base.join(s).ok())
        {
            r.images.push(image.to_string());
        }
        out.push((url, r));
    }
    if out.is_empty()
        && doc
            .select(&sel(".mac_total, .module-search, .module-main"))
            .next()
            .is_none()
    {
        return Err(ProviderError::Parse(format!(
            "{name} search: missing result structure"
        )));
    }
    Ok(out)
}
fn detail(body: &str, mut result: SearchResult, base: &Url) -> Option<SearchResult> {
    let doc = Html::parse_document(body);
    let mut seen = HashSet::new();
    for row in doc.select(&sel("#download-list .module-row-one")) {
        let Some(raw) = row
            .select(&sel("[data-clipboard-text]"))
            .next()
            .and_then(|n| n.value().attr("data-clipboard-text"))
        else {
            continue;
        };
        let mut link = Link::new(raw.trim());
        if link.cloud_type == CloudType::Others || !seen.insert(link.url.clone()) {
            continue;
        }
        if result.source.name() == "erxiao"
            && matches!(link.cloud_type, CloudType::Weiyun | CloudType::Lanzou)
        {
            continue;
        }
        link.password = password(&link.url, "");
        result.links.push(link);
    }
    if result.links.is_empty() {
        return None;
    }
    if let Some(image) = doc
        .select(&sel(".mobile-play .lazyload"))
        .next()
        .and_then(|n| n.value().attr("data-src"))
        .and_then(|s| base.join(s).ok())
    {
        result.images = vec![image.to_string()];
    }
    Some(result)
}
impl MacCms {
    pub async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut tasks = Vec::new();
        for base in &self.bases {
            tasks.push(base.clone());
        }
        let batches = stream::iter(tasks.into_iter().map(|base| async move {
            let encoded =
                url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
            let url = base
                .join(&format!("index.php/vod/search/wd/{encoded}.html"))
                .unwrap();
            let request = |url: Url| {
                ctx.client
                    .get(url)
                    .header("User-Agent", UA)
                    .header("Referer", base.as_str())
            };
            let body = fetch(
                request(url),
                self.name,
                "search",
                ctx.request_timeout(Duration::from_secs(8)),
                16 << 20,
                &[200, 400],
            )
            .await?;
            let entries = items(&body, &base, self.name)?;
            let failures = &std::sync::Mutex::new(None);
            let rows = stream::iter(entries.into_iter().map(|(url, row)| {
                let request = &request;
                let base = &base;
                async move {
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
                    let body = fetch(
                        request(url.clone()),
                        self.name,
                        "detail",
                        ctx.request_timeout(Duration::from_secs(6)),
                        16 << 20,
                        &[200, 400],
                    )
                    .await;
                    let body = super::remember_failure(body, failures)?;
                    let row = detail(&body, row, base)?;
                    let mut cache = self.cache.lock().unwrap();
                    if cache.len() >= 512
                        && let Some(key) = cache
                            .iter()
                            .min_by_key(|(_, v)| v.0)
                            .map(|(k, _)| k.clone())
                    {
                        cache.remove(&key);
                    }
                    cache.insert(url.to_string(), (Instant::now(), row.clone()));
                    drop(cache);
                    ctx.publish(vec![row.clone()]);
                    Some(row)
                }
            }))
            .buffered(20)
            .collect::<Vec<_>>()
            .await;
            if let Some(error) = failures.lock().unwrap().take() {
                return Err(error);
            }
            if rows.iter().any(Option::is_none) {
                return Err(ProviderError::Unavailable(format!(
                    "{}: one or more details failed",
                    self.name
                )));
            }
            Ok::<_, ProviderError>(rows.into_iter().flatten().collect::<Vec<_>>())
        }))
        .buffered(self.bases.len().max(1))
        .collect::<Vec<_>>()
        .await;
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut error = None;
        let mut success = 0;
        for batch in batches {
            match batch {
                Ok(rows) => {
                    success += 1;
                    out.extend(rows.into_iter().filter(|r| {
                        seen.insert((
                            r.id.clone(),
                            r.links.iter().map(|l| l.url.clone()).collect::<Vec<_>>(),
                        ))
                    }));
                }
                Err(e) => {
                    error.get_or_insert(e);
                }
            }
        }
        if success == 0 {
            return Err(error.unwrap_or_else(|| {
                ProviderError::Unavailable(format!("{}: no mirrors", self.name))
            }));
        }
        if let Some(error) = error {
            return Err(error);
        }
        Ok(out)
    }
}
#[async_trait]
impl Provider for ErxiaoProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("erxiao", 1, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        q: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        self.inner.search(ctx, q).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixtures() {
        let base = Url::parse("https://www.wexwp.cc/").unwrap();
        let (_, r) = items(
            include_str!("../../tests/fixtures/providers/erxiao/success.html"),
            &base,
            "erxiao",
        )
        .unwrap()
        .remove(0);
        let r = detail(
            include_str!("../../tests/fixtures/providers/erxiao/detail.html"),
            r,
            &base,
        )
        .unwrap();
        assert_eq!(r.links.len(), 1);
        assert!(
            items(
                include_str!("../../tests/fixtures/providers/erxiao/empty.html"),
                &base,
                "erxiao"
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            items(
                include_str!("../../tests/fixtures/providers/erxiao/malformed.html"),
                &base,
                "erxiao"
            )
            .is_err()
        );
    }
}
