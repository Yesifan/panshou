use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{Link, ProviderError, SearchResult};
use async_trait::async_trait;
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use std::{collections::HashSet, sync::LazyLock, time::Duration};
use url::Url;
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/(\d+)\.html").unwrap());
static GO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/go\.html\?url=([A-Za-z0-9+/]+=*)").unwrap());
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://www.xiaojitv.com/").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Xiaoji {
    endpoints: Endpoints,
}
impl Xiaoji {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn text(e: ElementRef<'_>) -> String {
    e.text().collect::<String>().trim().into()
}
fn first(e: ElementRef<'_>, s: &str) -> String {
    e.select(&sel(s)).next().map(text).unwrap_or_default()
}
fn list(body: &str, base: &Url) -> Vec<(Url, SearchResult)> {
    let d = Html::parse_document(body);
    d.select(&sel("article.poster-item"))
        .filter_map(|e| {
            let href = e
                .select(&sel(".poster-link"))
                .next()?
                .value()
                .attr("href")?;
            let url = base.join(href).ok()?;
            let id = ID.captures(url.path())?[1].to_owned();
            let title = first(e, ".poster-title a");
            if title.is_empty() {
                return None;
            }
            let mut r = SearchResult::provider(format!("xiaoji-{id}"), "xiaoji");
            r.title = title;
            r.datetime = Some(chrono::Utc::now());
            r.tags = e
                .select(&sel(".poster-tags a"))
                .map(text)
                .filter(|s| !s.is_empty())
                .collect();
            r.content = format!("分类: {}", first(e, ".poster-category a"));
            let rating = first(e, ".rating-score");
            if !rating.is_empty() {
                r.content.push_str(&format!(" | 评分: {rating}"))
            }
            if !r.tags.is_empty() {
                r.content
                    .push_str(&format!(" | 标签: {}", r.tags.join(", ")))
            }
            Some((url, r))
        })
        .collect()
}
fn valid(raw: &str) -> bool {
    raw.starts_with("magnet:") && raw.contains("xt=")
        || raw.starts_with("ed2k://") && raw.len() > 7
        || Url::parse(raw).is_ok_and(|u| {
            matches!(u.scheme(), "http" | "https") && u.host_str().is_some_and(|h| h.contains('.'))
        })
}
fn detail(body: &str) -> Vec<Link> {
    let d = Html::parse_document(body);
    let mut seen = HashSet::new();
    d.select(&sel(".resource-compact-link a[href]"))
        .filter_map(|a| {
            let raw = a.value().attr("href")?;
            let raw = if raw.contains("/go.html?url=") {
                let encoded = GO.captures(raw)?[1].to_owned();
                String::from_utf8(
                    STANDARD
                        .decode(&encoded)
                        .or_else(|_| STANDARD_NO_PAD.decode(&encoded))
                        .ok()?,
                )
                .ok()?
                .trim()
                .to_owned()
            } else {
                raw.into()
            };
            (valid(&raw) && seen.insert(raw.clone())).then(|| Link::new(raw))
        })
        .collect()
}
impl Xiaoji {
    async fn fetch(
        &self,
        ctx: &SearchContext,
        url: Url,
        maximum: u64,
    ) -> Result<Vec<u8>, ProviderError> {
        tokio::time::timeout(ctx.request_timeout(Duration::from_secs(maximum)),request(||ctx.client.get(url.clone()).header("referer",self.endpoints.base.as_str()).header("user-agent","Mozilla/5.0 Chrome/91.0.4472.124 Safari/537.36").header("accept","text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8").header("accept-language","zh-CN,zh;q=0.9,en;q=0.8").header("cache-control","max-age=0"),3,Duration::from_millis(200),BODY_LIMIT)).await.map_err(|_|ProviderError::Timeout)?
    }
}
#[async_trait]
impl Provider for Xiaoji {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("xiaoji", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut url = self.endpoints.base.clone();
        url.query_pairs_mut().append_pair("s", query);
        let body = self.fetch(ctx, url, 10).await?;
        let rows = list(&String::from_utf8_lossy(&body), &self.endpoints.base);
        collect_details(
            ctx,
            rows.into_iter().map(|(url, mut row)| async move {
                let body = self.fetch(ctx, url, 8).await?;
                row.links = detail(&String::from_utf8_lossy(&body));
                Ok(row)
            }),
            4,
        )
        .await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn direct_and_encoded() {
        let encoded = STANDARD.encode("https://pan.baidu.com/s/test");
        let h = format!(
            r#"<div class="resource-compact-link"><a href="/go.html?url={encoded}">open</a><a href="https://pan.baidu.com/s/test">same</a><a href="magnet:?xt=urn:btih:abc">magnet</a><a href="javascript:bad">bad</a></div>"#
        );
        let r = detail(&h);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].url, "https://pan.baidu.com/s/test");
        assert!(detail("bad").is_empty());
        assert!(list("bad", &Endpoints::default().base).is_empty());
    }
}
