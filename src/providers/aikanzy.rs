//! 爱看资源 articles: four total attempts, independent detail fan-out.
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
pub struct AikanzyProvider {
    base: Url,
}
impl Default for AikanzyProvider {
    fn default() -> Self {
        Self::new(Url::parse("https://www.aikanzy.com/").unwrap())
    }
}
impl AikanzyProvider {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
}
struct Article {
    id: String,
    url: Url,
    result: SearchResult,
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn items(body: &str, base: &Url) -> Result<Vec<Article>, ProviderError> {
    static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/[a-z]+/(\d+)\.html").unwrap());
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    for node in doc.select(&sel("article.post-list.contt.blockimg")) {
        let Some(a) = node.select(&sel("a[href]")).next() else {
            continue;
        };
        let Some(url) = a.value().attr("href").and_then(|s| base.join(s).ok()) else {
            continue;
        };
        let Some(id) = ID.captures(url.path()).map(|c| c[1].to_owned()) else {
            continue;
        };
        let title = node
            .select(&sel("header.entry-header span.entry-title a"))
            .next()
            .map(|n| clean(&n.inner_html()))
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let category = node
            .select(&sel("div.entry-meta > a"))
            .next()
            .map(|n| clean(&n.inner_html()))
            .unwrap_or_default();
        let summary = node
            .select(&sel("div.entry-summary.ss p"))
            .map(|n| clean(&n.inner_html()))
            .collect::<Vec<_>>()
            .join(" ");
        let date = node
            .select(&sel("time"))
            .next()
            .map(|n| clean(&n.inner_html()))
            .unwrap_or_default();
        let mut result = SearchResult::provider(format!("aikanzy-{id}"), "aikanzy");
        result.title = title;
        result.content = format!("{summary} | {category} | {date}");
        if !category.is_empty() {
            result.tags.push(category);
        }
        result.datetime = chrono::DateTime::parse_from_rfc3339(&date)
            .ok()
            .map(|d| d.with_timezone(&chrono::Utc))
            .or_else(|| {
                chrono::NaiveDateTime::parse_from_str(&date, "%Y-%m-%d %H:%M:%S")
                    .ok()
                    .map(|d| d.and_utc())
            })
            .or_else(|| {
                chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d")
                    .ok()
                    .and_then(|d| d.and_hms_opt(0, 0, 0))
                    .map(|d| d.and_utc())
            });
        out.push(Article { id, url, result });
    }
    if out.is_empty()
        && doc
            .select(&sel("article, .post-list, .site-main, .content-area"))
            .next()
            .is_none()
    {
        return Err(ProviderError::Parse(
            "aikanzy search: missing article structure".into(),
        ));
    }
    Ok(out)
}
fn links(body: &str) -> Vec<Link> {
    static URL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"https?://(?:pan\.quark\.cn|drive\.uc\.cn|pan\.baidu\.com|pan\.xunlei\.com)/s/[^\s\"'<>]+"#).unwrap()
    });
    let doc = Html::parse_document(body);
    let mut seen = HashSet::new();
    let add = |raw: &str, seen: &mut HashSet<String>| -> Option<Link> {
        let mut l = Link::new(raw.trim().replace("&amp;", "&"));
        if !matches!(
            l.cloud_type,
            CloudType::Quark | CloudType::Uc | CloudType::Baidu | CloudType::Xunlei
        ) || !seen.insert(l.url.clone())
        {
            return None;
        }
        l.password = password(&l.url, "");
        Some(l)
    };
    let mut links = doc
        .select(&sel("a[href]"))
        .filter_map(|a| add(a.value().attr("href")?, &mut seen))
        .collect::<Vec<_>>();
    if links.is_empty() {
        links = URL
            .find_iter(body)
            .filter_map(|m| add(m.as_str(), &mut seen))
            .collect();
    }
    links
}
#[async_trait]
impl Provider for AikanzyProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("aikanzy", 3, KeywordFilterMode::Core)
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
                    .header("Referer", self.base.as_str()),
                "aikanzy",
                stage,
                ctx.timeout.min(Duration::from_secs(seconds)),
                16 << 20,
                &[200, 400, 800],
            )
        };
        let mut url = self.base.join("search").unwrap();
        url.query_pairs_mut()
            .append_pair("word", query)
            .append_pair("molds", "article");
        let rows = items(&get(url, "search", 15).await?, &self.base)?;
        let failures = &std::sync::Mutex::new(None);
        let rows = stream::iter(rows.into_iter().map(|mut a| async move {
            let body = super::remember_failure(get(a.url, "detail", 8).await, failures)?;
            a.result.links = links(&body);
            if a.result.links.is_empty() {
                return None;
            }
            debug_assert_eq!(a.result.id, format!("aikanzy-{}", a.id));
            ctx.publish(vec![a.result.clone()]);
            Some(a.result)
        }))
        .buffered(15)
        .collect::<Vec<_>>()
        .await;
        if let Some(error) = failures.lock().unwrap().take() {
            return Err(error);
        }
        if rows.iter().any(Option::is_none) {
            return Err(ProviderError::Unavailable(
                "aikanzy: one or more details failed or contained no usable links".into(),
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
        let base = Url::parse("https://www.aikanzy.com/").unwrap();
        let r = items(
            include_str!("../../tests/fixtures/providers/aikanzy/success.html"),
            &base,
        )
        .unwrap();
        assert_eq!(r[0].result.title, "测试影片");
        assert_eq!(
            links(include_str!(
                "../../tests/fixtures/providers/aikanzy/detail.html"
            ))[0]
                .password
                .as_deref(),
            Some("abcd")
        );
        assert!(
            items(
                include_str!("../../tests/fixtures/providers/aikanzy/empty.html"),
                &base
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            items(
                include_str!("../../tests/fixtures/providers/aikanzy/malformed.html"),
                &base
            )
            .is_err()
        );
    }
}
