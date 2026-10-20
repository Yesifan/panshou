//! 高清888: search cards, then playlist/download links (including /go/play wrappers).
use super::{UA, clean, fetch, password};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{Html, Selector};
use std::{collections::HashSet, sync::LazyLock, time::Duration};
use url::Url;
#[derive(Debug, Clone)]
pub struct Gaoqing888Provider {
    base: Url,
}
impl Default for Gaoqing888Provider {
    fn default() -> Self {
        Self::new(Url::parse("https://www.gaoqing888.com/").unwrap())
    }
}
impl Gaoqing888Provider {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
}
struct Item {
    id: String,
    title: String,
    url: Url,
    image: String,
    content: String,
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn items(body: &str, base: &Url) -> Result<Vec<Item>, ProviderError> {
    static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/(\d+)/detail").unwrap());
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    for row in doc.select(&sel("div.wp-list.search-list div.video-row")) {
        let Some(a) = row.select(&sel("a.title-link")).next() else {
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
        let image = row
            .select(&sel("a.cover-link img.cover"))
            .next()
            .and_then(|n| n.value().attr("src"))
            .and_then(|s| base.join(s).ok())
            .map(|u| u.to_string())
            .unwrap_or_default();
        let content = row
            .select(&sel("div.meta"))
            .take(2)
            .map(|n| clean(&n.inner_html()))
            .collect::<Vec<_>>()
            .join(" | ");
        out.push(Item {
            id,
            title,
            url,
            image,
            content,
        });
    }
    if out.is_empty() && doc.select(&sel("div.wp-list.search-list")).next().is_none() {
        return Err(ProviderError::Parse(
            "gaoqing888 search: missing search-list".into(),
        ));
    }
    Ok(out)
}
fn detail(body: &str, item: Item, base: &Url) -> Option<SearchResult> {
    static MAGNET: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"magnet:\?xt=urn:btih:[0-9A-Za-z]+[^\"' <]*"#).unwrap());
    let doc = Html::parse_document(body);
    let mut raw: Vec<String> = doc
        .select(&sel("ul.playlist a[href], div.wp-download a[href]"))
        .filter_map(|a| a.value().attr("href").map(str::to_owned))
        .collect();
    raw.extend(MAGNET.find_iter(body).map(|m| m.as_str().to_owned()));
    let mut links = Vec::new();
    let mut seen = HashSet::new();
    for raw in raw {
        let Ok(mut url) = base.join(raw.trim()) else {
            continue;
        };
        if url.path() == "/go/play" {
            let target = url
                .query_pairs()
                .find(|(k, _)| k == "url")
                .map(|(_, v)| v.into_owned());
            if let Some(target) = target {
                let Ok(u) = Url::parse(&target) else {
                    continue;
                };
                url = u;
            }
        }
        let mut link = Link::new(url.to_string());
        if !matches!(
            link.cloud_type,
            CloudType::Magnet
                | CloudType::Quark
                | CloudType::Baidu
                | CloudType::Xunlei
                | CloudType::Aliyun
                | CloudType::Uc
        ) {
            continue;
        }
        if !seen.insert(link.url.clone()) {
            continue;
        }
        link.password = password(&link.url, "");
        links.push(link);
    }
    if links.is_empty() {
        return None;
    }
    let mut r = SearchResult::provider(format!("gaoqing888-{}", item.id), "gaoqing888");
    r.title = item.title;
    r.content = doc
        .select(&sel("div.wp-content.video-detail p"))
        .next()
        .map(|n| clean(&n.inner_html()))
        .filter(|s| !s.is_empty())
        .or_else(|| {
            doc.select(&sel("meta[name='description']"))
                .next()
                .and_then(|n| n.value().attr("content").map(str::to_owned))
        })
        .unwrap_or(item.content);
    r.links = links;
    if !item.image.is_empty() {
        r.images.push(item.image);
    }
    if let Some(n) = doc.select(&sel("div.info ul li")).nth(3) {
        let tag = clean(&n.inner_html());
        if !tag.is_empty() {
            r.tags.push(tag);
        }
    }
    Some(r)
}
#[async_trait]
impl Provider for Gaoqing888Provider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("gaoqing888", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let get = |url: Url, stage: &'static str, seconds| {
            fetch(
                ctx.client
                    .get(url)
                    .header("User-Agent", UA)
                    .header("Referer", self.base.as_str())
                    .header(
                        "Accept",
                        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
                    ),
                "gaoqing888",
                stage,
                ctx.timeout.min(Duration::from_secs(seconds)),
                16 << 20,
                &[200, 400],
            )
        };
        let mut url = self.base.join("search").unwrap();
        url.query_pairs_mut().append_pair("kw", query);
        let rows = items(&get(url, "search", 15).await?, &self.base)?;
        let failures = &std::sync::Mutex::new(None);
        let rows = stream::iter(rows.into_iter().map(|item| async move {
            let body =
                super::remember_failure(get(item.url.clone(), "detail", 10).await, failures)?;
            let result = detail(&body, item, &self.base);
            if let Some(row) = &result {
                ctx.publish(vec![row.clone()]);
            }
            result
        }))
        .buffered(8)
        .collect::<Vec<_>>()
        .await;
        if let Some(error) = failures.lock().unwrap().take() {
            return Err(error);
        }
        if rows.iter().any(Option::is_none) {
            return Err(ProviderError::Unavailable(
                "gaoqing888: one or more details failed or contained no usable links".into(),
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
        let base = Url::parse("https://www.gaoqing888.com/").unwrap();
        let item = items(
            include_str!("../../tests/fixtures/providers/gaoqing888/success.html"),
            &base,
        )
        .unwrap()
        .remove(0);
        let r = detail(
            include_str!("../../tests/fixtures/providers/gaoqing888/detail.html"),
            item,
            &base,
        )
        .unwrap();
        assert_eq!(r.id, "gaoqing888-123");
        assert_eq!(r.links.len(), 2);
        assert_eq!(r.links[0].password.as_deref(), Some("abcd"));
        assert!(
            items(
                include_str!("../../tests/fixtures/providers/gaoqing888/empty.html"),
                &base
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            items(
                include_str!("../../tests/fixtures/providers/gaoqing888/malformed.html"),
                &base
            )
            .is_err()
        );
    }
}
