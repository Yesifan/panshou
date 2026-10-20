//! The target upstream deliberately uses HTTP on www.mihdr.top (not HTTPS).
use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use async_trait::async_trait;
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use std::{collections::HashSet, sync::LazyLock, time::Duration};
use url::Url;
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/vod/detail/id/(\d+)\.html").unwrap());
static LEGACY_LANZOU: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:www\.)?(?:lanzou[uixys]*|lan[zs]o[ux])\.(?:com|net|org)$").unwrap()
});
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("http://www.mihdr.top/").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Zhizhen {
    endpoints: Endpoints,
}
impl Zhizhen {
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
fn parse_list(body: &str) -> Vec<SearchResult> {
    let doc = Html::parse_document(body);
    doc.select(&sel(".module-search-item"))
        .filter_map(|e| {
            let a = e.select(&sel(".video-info-header h3 a")).next()?;
            let id = ID.captures(a.value().attr("href")?)?[1].to_owned();
            let mut row = SearchResult::provider(format!("zhizhen-{id}"), "zhizhen");
            row.title = text(a);
            row.tags = e
                .select(&sel(".video-info-aux .tag-link a"))
                .map(text)
                .filter(|s| !s.is_empty())
                .collect();
            let quality = first(e, ".video-serial");
            let mut parts = vec![];
            if !quality.is_empty() {
                parts.push(format!("【{quality}】"))
            }
            for info in e.select(&sel(".video-info-items")) {
                let label = first(info, ".video-info-itemtitle");
                if label.contains("导演") {
                    let director = first(info, ".video-info-actor a");
                    if !director.is_empty() {
                        parts.push(format!("导演：{director}"))
                    }
                } else if label.contains("主演") {
                    let actors = info
                        .select(&sel(".video-info-actor a"))
                        .map(text)
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>();
                    if !actors.is_empty() {
                        parts.push(format!(
                            "主演：{}{}",
                            actors
                                .iter()
                                .take(3)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join("、"),
                            if actors.len() > 3 { "等" } else { "" }
                        ))
                    }
                } else if label.contains("剧情") {
                    let plot = first(info, ".video-info-item");
                    if !plot.is_empty() {
                        parts.push(plot)
                    }
                }
            }
            row.content = parts.join("\n");
            row.images = e
                .select(&sel(".module-item-pic > img"))
                .next()
                .and_then(|i| i.value().attr("data-src"))
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .into_iter()
                .collect();
            Some(row)
        })
        .collect()
}
fn parse_detail(body: &str, row: &mut SearchResult) {
    let doc = Html::parse_document(body);
    let poster = doc
        .select(&sel(".mobile-play .lazyload"))
        .next()
        .and_then(|i| i.value().attr("data-src"))
        .filter(|s| !s.is_empty());
    if let Some(poster) = poster {
        row.images = vec![poster.into()]
    }
    let mut seen = HashSet::new();
    for item in doc.select(&sel("#download-list .module-row-one")) {
        let copy = item
            .select(&sel("[data-clipboard-text]"))
            .next()
            .and_then(|e| e.value().attr("data-clipboard-text"));
        let anchors = sel("a[href]");
        for raw in copy
            .into_iter()
            .chain(item.select(&anchors).filter_map(|a| a.value().attr("href")))
        {
            if raw.contains('#') || raw.contains("javascript:") {
                continue;
            }
            let mut link = Link::new(raw);
            let legacy = Url::parse(raw).ok().is_some_and(|u| {
                let host = u.host_str().unwrap_or("");
                if LEGACY_LANZOU.is_match(host) {
                    link.cloud_type = CloudType::Lanzou;
                }
                matches!(host, "jianguoyun.com" | "www.jianguoyun.com")
                    && u.path().starts_with("/p/")
            });
            if (legacy || link.cloud_type != CloudType::Others) && seen.insert(link.url.clone()) {
                row.links.push(link)
            }
        }
    }
}
impl Zhizhen {
    async fn fetch(&self, ctx: &SearchContext, url: &Url) -> Result<Vec<u8>, ProviderError> {
        tokio::time::timeout(ctx.remaining().min(Duration::from_secs(25)),request(||ctx.client.get(url.clone()).header("referer",self.endpoints.base.as_str()).header("user-agent","Mozilla/5.0 Chrome/91.0.4472.124 Safari/537.36").header("accept","text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8").header("accept-language","zh-CN,zh;q=0.9,en;q=0.8").header("cache-control","max-age=0"),3,Duration::from_millis(200),BODY_LIMIT)).await.map_err(|_|ProviderError::Timeout)?
    }
}
#[async_trait]
impl Provider for Zhizhen {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("zhizhen", 1, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let query = query_escape(query);
        let url = self
            .endpoints
            .base
            .join(&format!("/index.php/vod/search/wd/{query}.html"))
            .unwrap();
        let body = self.fetch(ctx, &url).await?;
        let rows = parse_list(&String::from_utf8_lossy(&body));
        collect_details(
            ctx,
            rows.into_iter().map(|mut row| async move {
                let id = row.id.trim_start_matches("zhizhen-");
                let url = self
                    .endpoints
                    .base
                    .join(&format!("/index.php/vod/detail/id/{id}.html"))
                    .unwrap();
                let body = self.fetch(ctx, &url).await?;
                parse_detail(&String::from_utf8_lossy(&body), &mut row);
                Ok(row)
            }),
            20,
        )
        .await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol() {
        let base = Endpoints::default().base;
        assert_eq!(base.as_str(), "http://www.mihdr.top/");
        let mut rows = parse_list(
            r#"<div class="module-search-item"><div class="video-info-header"><h3><a href="/index.php/vod/detail/id/123.html">movie</a></h3></div><div class="video-serial">4K</div></div>"#,
        );
        assert_eq!(rows[0].id, "zhizhen-123");
        parse_detail(
            r#"<div id="download-list"><div class="module-row-one"><button data-clipboard-text="https://pan.quark.cn/s/a"></button><a href="https://pan.quark.cn/s/a">same</a></div></div>"#,
            &mut rows[0],
        );
        assert_eq!(rows[0].links.len(), 1);
        assert!(parse_list("bad").is_empty());
    }
}
