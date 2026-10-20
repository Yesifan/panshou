use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{Link, ProviderError, SearchResult};
use async_trait::async_trait;
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use std::{sync::LazyLock, time::Duration};
use url::Url;
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/hash/(\d+)\.html").unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").unwrap());
static SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://www.iyuhuage.fun/").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Yuhuage {
    endpoints: Endpoints,
}
impl Yuhuage {
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
    d.select(&sel(".search-item.detail-width"))
        .filter_map(|e| {
            let a = e.select(&sel(".item-title h3 a")).next()?;
            let title = SPACE
                .replace_all(TAG.replace_all(&text(a), "").trim(), " ")
                .into_owned();
            if title.is_empty() {
                return None;
            }
            let url = base.join(a.value().attr("href")?).ok()?;
            let id = ID
                .captures(url.path())
                .map(|c| c[1].to_owned())
                .unwrap_or_default();
            let get = |label: &str| {
                e.select(&sel(".item-bar span"))
                    .find(|e| text(*e).contains(label))
                    .map(|e| first(e, "b"))
                    .unwrap_or_default()
            };
            let when = get("创建时间");
            let size = first(e, ".item-bar .cpill.blue-pill");
            let count = first(e, ".item-bar .cpill.yellow-pill");
            let hot = get("热度");
            let last = get("最近下载");
            let mut row = SearchResult::provider(format!("yuhuage-{id}"), "yuhuage");
            row.title = title;
            row.content =
                format!("创建时间: {when} | 大小: {size} | 文件数: {count} | 热度: {hot}");
            if !last.is_empty() {
                row.content.push_str(&format!(" | 最近下载: {last}"))
            }
            row.datetime = date(&when).or_else(|| date(&when.replace('/', "-")));
            row.tags = vec!["磁力链接".into()];
            Some((url, row))
        })
        .collect()
}
fn detail(body: &str) -> Vec<Link> {
    let doc = Html::parse_document(body);
    doc.select(&sel(
        "a.download[href^='magnet:'], a.download[href^='thunder:']",
    ))
    .filter_map(|a| a.value().attr("href"))
    .map(Link::new)
    .collect()
}
impl Yuhuage {
    async fn get_detail(&self, ctx: &SearchContext, url: Url) -> Result<Vec<Link>, ProviderError> {
        let mut last = ProviderError::Protocol("yuhuage detail failed".into());
        for attempt in 0..3 {
            if ctx.is_cancelled() {
                return Err(ProviderError::Timeout);
            }
            let response = request(
                || {
                    ctx.client
                        .get(url.clone())
                        .timeout(ctx.request_timeout(Duration::from_secs(15)))
                        .header("referer", self.endpoints.base.as_str())
                        .header(
                            "user-agent",
                            "Mozilla/5.0 Chrome/91.0.4472.124 Safari/537.36",
                        )
                },
                1,
                Duration::ZERO,
                BODY_LIMIT,
            )
            .await;
            match response {
                Ok(body) => return Ok(detail(&String::from_utf8_lossy(&body))),
                Err(error) => last = error,
            }
            if attempt < 2 {
                tokio::time::sleep(Duration::from_secs(attempt + 1)).await
            }
        }
        Err(last)
    }
}
#[async_trait]
impl Provider for Yuhuage {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("yuhuage", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let escaped = query_escape(query);
        let url = self
            .endpoints
            .base
            .join(&format!("/search/{escaped}-1-time.html"))
            .unwrap();
        let body = tokio::time::timeout(
            ctx.request_timeout(Duration::from_secs(30)),
            request(
                || {
                    ctx.client
                        .get(url.clone())
                        .header("referer", self.endpoints.base.as_str())
                        .header(
                            "user-agent",
                            "Mozilla/5.0 Chrome/91.0.4472.124 Safari/537.36",
                        )
                },
                3,
                Duration::from_millis(200),
                BODY_LIMIT,
            ),
        )
        .await
        .map_err(|_| ProviderError::Timeout)??;
        let rows = list(&String::from_utf8_lossy(&body), &self.endpoints.base);
        collect_details(
            ctx,
            rows.into_iter().map(|(url, mut row)| async move {
                row.links = self.get_detail(ctx, url).await?;
                Ok(row)
            }),
            5,
        )
        .await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn magnet_and_thunder() {
        let h = r#"<a class="download" href="magnet:?xt=urn:btih:abc">a</a><a class="download" href="thunder://abc">b</a><a href="magnet:?xt=urn:btih:ignored">ignored</a>"#;
        let r = detail(h);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].cloud_type, crate::core::CloudType::Magnet);
        assert_eq!(r[1].cloud_type, crate::core::CloudType::Others);
        assert!(detail("bad").is_empty());
        assert!(list("bad", &Endpoints::default().base).is_empty());
    }
}
