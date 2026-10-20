//! 剧透社's search list and dlipp download cards.
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use chrono::NaiveDate;
use regex::Regex;
use scraper::{Html, Selector};
use std::{sync::LazyLock, time::Duration};
use url::Url;
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/([^/]+)/(\d+)\.html").unwrap());
static PASSWORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"pwd=([^&]+)").unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"【([^】]+)】").unwrap());
static DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{4})年(\d{1,2})月(\d{1,2})日").unwrap());
#[derive(Debug, Clone)]
pub struct Jutoushe {
    base: Url,
}
impl Default for Jutoushe {
    fn default() -> Self {
        Self::new(Url::parse("https://1.star2.cn/").unwrap())
    }
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn text(n: scraper::ElementRef<'_>) -> String {
    n.text().collect::<String>().trim().to_owned()
}
fn search_items(body: &str, base: &Url) -> Result<Vec<(Url, SearchResult)>, ProviderError> {
    let doc = Html::parse_document(body);
    if doc.select(&sel("ul.erx-list")).next().is_none() {
        return Err(ProviderError::Parse("jutoushe search list missing".into()));
    }
    let mut out = Vec::new();
    for row in doc.select(&sel("ul.erx-list li.item")) {
        let Some(a) = row.select(&sel(".a a.main")).next() else {
            continue;
        };
        let Some(href) = a.value().attr("href") else {
            continue;
        };
        let title = text(a);
        if title.is_empty() {
            continue;
        }
        let Some(url) = base.join(href).ok() else {
            continue;
        };
        let id = ID
            .captures(href)
            .map(|c| c[2].to_owned())
            .unwrap_or_else(|| href.replace('/', "_"));
        let mut result = SearchResult::provider(format!("jutoushe-{id}"), "jutoushe");
        result.title = title;
        result.content = format!("剧透社影视资源：{}", result.title);
        result.tags = TAG
            .captures_iter(&result.title)
            .map(|c| c[1].to_owned())
            .collect();
        if result.tags.is_empty() {
            result.tags.push("影视资源".into());
        }
        let raw = row
            .select(&sel(".i span.time"))
            .next()
            .map(text)
            .unwrap_or_default();
        let date = NaiveDate::parse_from_str(&raw, "%Y-%m-%d")
            .ok()
            .or_else(|| {
                let c = DATE.captures(&raw)?;
                NaiveDate::from_ymd_opt(c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?)
            });
        result.datetime = date
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|t| t.and_utc());
        out.push((url, result));
    }
    Ok(out)
}
fn detail_links(body: &str) -> Vec<Link> {
    let doc = Html::parse_document(body);
    doc.select(&sel(".dlipp-cont-bd a.dlipp-dl-btn"))
        .filter_map(|a| {
            let href = a.value().attr("href")?;
            let url = Url::parse(href).ok()?;
            if !matches!(url.scheme(), "http" | "https") {
                return None;
            }
            let mut link = Link::new(href);
            if link.cloud_type == CloudType::Others {
                return None;
            }
            // This site only carries extraction codes in Baidu's pwd parameter.
            if link.cloud_type == CloudType::Baidu {
                link.password = PASSWORD.captures(href).map(|c| c[1].to_owned());
            }
            Some(link)
        })
        .collect()
}
impl Jutoushe {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
    async fn fetch(
        &self,
        ctx: &SearchContext,
        url: Url,
        detail: bool,
    ) -> Result<String, ProviderError> {
        crate::providers::b_registry::http::text(
            ctx.client
                .get(url)
                .header(
                    "user-agent",
                    "Mozilla/5.0 Chrome/91.0.4472.124 Safari/537.36",
                )
                .header(
                    "accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
                )
                .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
                .header("referer", self.base.as_str()),
            if detail { 1 } else { 3 },
            200,
            true,
            Duration::from_secs(if detail { 15 } else { 30 }),
            16 << 20,
        )
        .await
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self
            .base
            .join("search/")
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        url.query_pairs_mut().append_pair("keyword", query);
        let items = search_items(&self.fetch(ctx, url, false).await?, &self.base)?;
        let mut out = Vec::new();
        let mut error = None;
        // Upstream deliberately walks these detail pages sequentially.
        for (url, mut result) in items {
            match self.fetch(ctx, url, true).await {
                Ok(body) => {
                    result.links = detail_links(&body);
                    let hay = format!("{} {}", result.title, result.content).to_lowercase();
                    if result.links.is_empty()
                        || !query
                            .to_lowercase()
                            .split_whitespace()
                            .all(|w| hay.contains(w))
                    {
                        continue;
                    }
                    for l in &mut result.links {
                        l.work_title = Some(result.title.clone());
                    }
                    ctx.publish(vec![result.clone()]);
                    out.push(result);
                }
                Err(e) => error = Some(e),
            }
        }
        if let Some(e) = error {
            return Err(e);
        }
        Ok(out)
    }
}
#[async_trait]
impl Provider for Jutoushe {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("jutoushe", 1, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        tokio::time::timeout(ctx.timeout, self.run(ctx, query))
            .await
            .map_err(|_| ProviderError::Timeout)?
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn success_empty_malformed() {
        let rows=search_items(r#"<ul class="erx-list"><li class="item"><div class="a"><a class="main" href="/dm/8100.html">【动漫】Film</a></div><div class="i"><span class="time">2026年10月12日</span></div></li></ul>"#,&Url::parse("https://1.star2.cn/").unwrap()).unwrap();
        assert_eq!(rows[0].1.id, "jutoushe-8100");
        assert_eq!(rows[0].1.tags, ["动漫"]);
        assert!(rows[0].1.datetime.is_some());
        let links = detail_links(
            r#"<div class="dlipp-cont-bd"><a class="dlipp-dl-btn" href="https://pan.baidu.com/s/abc?pwd=abcd"></a><a class="dlipp-dl-btn" href="javascript:bad"></a></div>"#,
        );
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].password.as_deref(), Some("abcd"));
        assert!(
            search_items(
                "<ul class='erx-list'></ul>",
                &Url::parse("https://1.star2.cn/").unwrap()
            )
            .unwrap()
            .is_empty()
        );
        assert!(search_items("<html/>", &Url::parse("https://1.star2.cn/").unwrap()).is_err());
    }
    #[tokio::test]
    async fn full_chain() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{path, query_param},
        };
        let s = MockServer::start().await;
        Mock::given(path("/search/")).and(query_param("keyword","Film")).respond_with(ResponseTemplate::new(200).set_body_string(r#"<ul class="erx-list"><li class="item"><div class="a"><a class="main" href="/dm/1.html">Film</a></div></li></ul>"#)).expect(1).mount(&s).await;
        Mock::given(path("/dm/1.html")).respond_with(ResponseTemplate::new(200).set_body_string(r#"<div class="dlipp-cont-bd"><a class="dlipp-dl-btn" href="https://pan.quark.cn/s/abc">download</a></div>"#)).expect(1).mount(&s).await;
        let p = Jutoushe::new(Url::parse(&s.uri()).unwrap());
        let c = SearchContext::new(reqwest::Client::new(), Duration::from_secs(3));
        assert_eq!(p.search(&c, "Film").await.unwrap().len(), 1);
    }
}
