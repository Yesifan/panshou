//! BT影视's three-stage movie -> resource page -> real share/magnet protocol.
use super::{UA, clean, fetch, password};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use std::{collections::HashSet, sync::LazyLock, time::Duration};
use url::Url;
#[derive(Debug, Clone)]
pub struct BtbtlbProvider {
    base: Url,
}
impl Default for BtbtlbProvider {
    fn default() -> Self {
        Self::new(Url::parse("https://www.btbtlb.com/").unwrap())
    }
}
impl BtbtlbProvider {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
}
#[derive(Clone)]
struct Movie {
    title: String,
    url: Url,
    content: String,
    image: String,
    tags: Vec<String>,
}
#[derive(Clone)]
struct Resource {
    title: String,
    url: Url,
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn text(node: ElementRef<'_>, selector: &str) -> String {
    node.select(&sel(selector))
        .next()
        .map(|n| clean(&n.inner_html()))
        .unwrap_or_default()
}
fn image(node: ElementRef<'_>, base: &Url) -> String {
    node.select(&sel("img"))
        .next()
        .and_then(|n| n.value().attr("data-src").or_else(|| n.value().attr("src")))
        .and_then(|s| base.join(s).ok())
        .map(|u| u.to_string())
        .unwrap_or_default()
}
fn search_items(body: &str, base: &Url) -> Result<Vec<Movie>, ProviderError> {
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/detail/(\d+)\.html$").unwrap());
    for n in doc.select(&sel(".module-items .module-item")) {
        let Some(a) = n
            .select(&sel(".module-item-title[href], a[href*='/detail/']"))
            .next()
        else {
            continue;
        };
        let Some(url) = a.value().attr("href").and_then(|s| base.join(s).ok()) else {
            continue;
        };
        if !ID.is_match(url.path()) || url.origin() != base.origin() || !seen.insert(url.clone()) {
            continue;
        }
        let title = clean(a.value().attr("title").unwrap_or(&a.inner_html()));
        if title.is_empty() {
            continue;
        }
        out.push(Movie {
            title,
            url,
            content: text(n, ".video-text"),
            image: image(n, base),
            tags: n
                .select(&sel(".module-item-caption span"))
                .map(|n| clean(&n.inner_html()))
                .filter(|s| !s.is_empty())
                .collect(),
        });
        if out.len() == 10 {
            break;
        }
    }
    if out.is_empty()
        && doc
            .select(&sel(".module-items, .module-heading, .search"))
            .next()
            .is_none()
    {
        return Err(ProviderError::Parse(
            "btbtlb search: missing result structure".into(),
        ));
    }
    Ok(out)
}
fn movie_detail(body: &str, mut movie: Movie, base: &Url) -> (Movie, Vec<Resource>) {
    let doc = Html::parse_document(body);
    let root = doc.root_element();
    let title = text(root, ".video-info-header > h1.page-title");
    if !title.is_empty() {
        movie.title = title;
    }
    let content = text(root, ".vod_content");
    if !content.is_empty() {
        movie.content = content;
    }
    if let Some(n) = doc.select(&sel(".video-cover")).next() {
        let v = image(n, base);
        if !v.is_empty() {
            movie.image = v;
        }
    }
    if movie.tags.is_empty() {
        movie.tags = doc
            .select(&sel(".video-info-aux a"))
            .map(|n| clean(&n.inner_html()))
            .filter(|s| !s.is_empty() && s != "Movie")
            .collect();
    }
    let mut torrents = Vec::new();
    let mut clouds = Vec::new();
    let mut seen = HashSet::new();
    for a in doc.select(&sel(".module-row-info a.module-row-text[href]")) {
        let Some(url) = a.value().attr("href").and_then(|s| base.join(s).ok()) else {
            continue;
        };
        if url.origin() != base.origin() || !seen.insert(url.clone()) {
            continue;
        }
        let cloud = url.path().starts_with("/pdown/");
        if !cloud && !url.path().starts_with("/tdown/") {
            continue;
        }
        if (cloud && clouds.len() == 4) || (!cloud && torrents.len() == 12) {
            continue;
        }
        let mut title = resource_title(&text(a, ".module-row-title h4"));
        if title.is_empty() {
            title = resource_title(a.value().attr("title").unwrap_or(""));
        }
        if title.is_empty() {
            title = movie.title.clone();
        }
        let r = Resource { title, url };
        if cloud {
            clouds.push(r)
        } else {
            torrents.push(r)
        }
    }
    torrents.extend(clouds);
    (movie, torrents)
}
fn labeled(doc: &Html, label: &str) -> String {
    for n in doc.select(&sel(".video-info-items")) {
        if text(n, ".video-info-itemtitle").contains(label) {
            return text(n, ".video-info-item");
        }
    }
    String::new()
}
fn date(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .or_else(|| chrono::DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S %z %Z").ok())
        .or_else(|| chrono::DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S %z").ok())
        .map(|d| d.with_timezone(&chrono::Utc))
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|d| d.and_utc())
        })
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|d| d.and_utc())
        })
}
fn resource_title(s: &str) -> String {
    clean(s)
        .trim_end_matches(".torrent")
        .trim_end_matches(".torren")
        .trim()
        .to_owned()
}
fn resource_result(body: &str, movie: Movie, resource: Resource) -> Option<SearchResult> {
    let doc = Html::parse_document(body);
    let body_text = doc.root_element().text().collect::<Vec<_>>().join(" ");
    static URL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)(?:https?://|magnet:\?xt=urn:btih:)[^\s\"'<>]+"#).unwrap()
    });
    static HASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^[0-9a-f]{40}$").unwrap());
    let mut raw: Vec<String> = doc
        .select(&sel("a[href]"))
        .filter_map(|a| a.value().attr("href").map(str::to_owned))
        .collect();
    raw.extend(URL.find_iter(&body_text).map(|m| m.as_str().to_owned()));
    let mut seen = HashSet::new();
    let mut links = Vec::new();
    for raw in raw {
        let raw = raw.trim().trim_matches(|c| "'\"<>),.;".contains(c));
        let Ok(url) = Url::parse(raw) else {
            continue;
        };
        let mut link = Link::new(raw);
        if !matches!(url.scheme(), "http" | "https" | "magnet")
            || link.cloud_type == CloudType::Others
        {
            continue;
        }
        if url.scheme() != "magnet" {
            link.password = password(raw, &body_text);
        }
        link.work_title = Some(movie.title.clone());
        if seen.insert((link.url.clone(), link.password.clone())) {
            links.push(link);
        }
    }
    if links.is_empty() {
        let hash = labeled(&doc, "Hash");
        if HASH.is_match(&hash) {
            let mut l = Link::new(format!("magnet:?xt=urn:btih:{}", hash.to_ascii_lowercase()));
            l.work_title = Some(movie.title.clone());
            links.push(l);
        }
    }
    if links.is_empty() {
        return None;
    }
    let title = resource_title(&text(doc.root_element(), ".tinfo .page-title"));
    let title = if title.is_empty() {
        resource.title
    } else {
        title
    };
    let id = resource
        .url
        .path_segments()?
        .next_back()?
        .trim_end_matches(".html");
    let mut result = SearchResult::provider(format!("btbtlb-{id}"), "btbtlb");
    result.title = title;
    result.content = format!("{}\n资源：{}", movie.content, result.title);
    result.links = links;
    result.tags = vec!["BT影视".into()];
    result.tags.extend(movie.tags);
    if !movie.image.is_empty() {
        result.images.push(movie.image);
    }
    result.datetime = date(&labeled(&doc, "更新时间")).or_else(|| date(&labeled(&doc, "种子时间")));
    Some(result)
}
#[async_trait]
impl Provider for BtbtlbProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("btbtlb", 3, KeywordFilterMode::Provider)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let get = |url: Url, stage: &'static str| {
            fetch(
                ctx.client
                    .get(url)
                    .header("User-Agent", UA)
                    .header("Referer", self.base.as_str()),
                "btbtlb",
                stage,
                ctx.timeout.min(Duration::from_secs(20)),
                8 << 20,
                &[],
            )
        };
        let mut url = self.base.join("search/").unwrap();
        url.path_segments_mut().unwrap().pop_if_empty().push(query);
        let items = search_items(&get(url, "search").await?, &self.base)?;
        let failures = &std::sync::Mutex::new(None);
        let movies = stream::iter(items.into_iter().map(|m| async move {
            let body = super::remember_failure(get(m.url.clone(), "movie").await, failures)?;
            Some(movie_detail(&body, m, &self.base))
        }))
        .buffered(4)
        .collect::<Vec<_>>()
        .await;
        let movie_failed = movies.iter().any(|movie| {
            movie
                .as_ref()
                .is_none_or(|(_, resources)| resources.is_empty())
        });
        let mut resources = Vec::new();
        let mut seen = HashSet::new();
        for (m, rs) in movies.into_iter().flatten() {
            for r in rs {
                if resources.len() < 40 && seen.insert(r.url.clone()) {
                    resources.push((m.clone(), r));
                }
            }
        }
        let rows = stream::iter(resources.into_iter().map(|(m, r)| async move {
            let body = super::remember_failure(get(r.url.clone(), "resource").await, failures)?;
            let result = resource_result(&body, m, r);
            if let Some(row) = &result {
                let text = format!(
                    "{} {} {}",
                    row.title,
                    row.content,
                    row.links
                        .iter()
                        .filter_map(|l| l.work_title.as_deref())
                        .collect::<Vec<_>>()
                        .join(" ")
                )
                .to_lowercase()
                .replace('.', " ");
                if query
                    .split_whitespace()
                    .all(|w| text.contains(&w.to_lowercase()))
                {
                    ctx.publish(vec![row.clone()]);
                }
            }
            result
        }))
        .buffered(8)
        .collect::<Vec<_>>()
        .await;
        if let Some(error) = failures.lock().unwrap().take() {
            return Err(error);
        }
        if movie_failed || rows.iter().any(Option::is_none) {
            return Err(ProviderError::Unavailable(
                "btbtlb: one or more movie/resource pages failed".into(),
            ));
        }
        // Match work titles as well as dotted release titles; don't delegate to core.
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        Ok(rows
            .into_iter()
            .flatten()
            .filter(|r| {
                let text = format!(
                    "{} {} {}",
                    r.title,
                    r.content,
                    r.links
                        .iter()
                        .filter_map(|l| l.work_title.as_deref())
                        .collect::<Vec<_>>()
                        .join(" ")
                )
                .to_lowercase()
                .replace('.', " ");
                words.iter().all(|w| text.contains(w))
            })
            .collect())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixtures() {
        let base = Url::parse("https://www.btbtlb.com/").unwrap();
        let m = search_items(
            include_str!("../../tests/fixtures/providers/btbtlb/success.html"),
            &base,
        )
        .unwrap()
        .remove(0);
        let (m, rs) = movie_detail(
            include_str!("../../tests/fixtures/providers/btbtlb/movie.html"),
            m,
            &base,
        );
        assert_eq!(rs.len(), 2);
        let r = resource_result(
            include_str!("../../tests/fixtures/providers/btbtlb/resource.html"),
            m.clone(),
            rs[0].clone(),
        )
        .unwrap();
        assert_eq!(r.links[0].password.as_deref(), Some("QWER"));
        assert_eq!(r.links[0].work_title.as_deref(), Some("测试影片"));
        assert!(
            resource_result(
                include_str!("../../tests/fixtures/providers/btbtlb/hash.html"),
                m,
                rs[1].clone()
            )
            .unwrap()
            .links[0]
                .url
                .starts_with("magnet:?xt=urn:btih:abcdef")
        );
        assert!(
            search_items(
                include_str!("../../tests/fixtures/providers/btbtlb/empty.html"),
                &base
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            search_items(
                include_str!("../../tests/fixtures/providers/btbtlb/malformed.html"),
                &base
            )
            .is_err()
        );
    }
}
