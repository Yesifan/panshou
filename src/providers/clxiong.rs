use crate::core::{Link, ProviderError, SearchResult, Source};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use chrono::{NaiveDate, Utc};
use futures::future::join_all;
use regex::Regex;
use scraper::{Html, Selector};
use url::Url;

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36";

fn request(client: &reqwest::Client, url: Url, referer: &Url) -> reqwest::RequestBuilder {
    client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::REFERER, referer.as_str())
        .header(
            reqwest::header::ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
        )
}
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
    pub search: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        let base = Url::parse("https://www.cilixiong.org/").expect("static URL");
        let search = base.join("e/search/index.php").expect("static URL");
        Self { base, search }
    }
}
#[derive(Debug, Clone, Default)]
pub struct ClxiongProvider {
    endpoints: Endpoints,
}
impl ClxiongProvider {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
#[derive(Debug)]
struct Item {
    id: String,
    title: String,
    content: String,
    images: Vec<String>,
    detail: Url,
}
fn search_id(location: &str) -> Option<String> {
    Regex::new(r"searchid=(\d+)")
        .ok()?
        .captures(location)?
        .get(1)
        .map(|m| m.as_str().to_owned())
}
fn parse_results(body: &str, base: &Url) -> Result<Vec<Item>, ProviderError> {
    let doc = Html::parse_document(body);
    let cards = Selector::parse(".row.row-cols-2.row-cols-lg-4 .col")
        .map_err(|e| ProviderError::Parse(e.to_string()))?;
    let anchors = Selector::parse("a[href*='/drama/'], a[href*='/movie/']").expect("selector");
    let title_sel = Selector::parse("h2.h4").expect("selector");
    let rank = Selector::parse(".rank").expect("selector");
    let small = Selector::parse(".small").expect("selector");
    let image = Selector::parse(".card-img").expect("selector");
    let image_re = Regex::new(r#"url\(['\"]?([^'\")]+)"#).expect("regex");
    let id_re = Regex::new(r"/(?:drama|movie)/(\d+)\.html").expect("regex");
    let mut out = Vec::new();
    for card in doc.select(&cards).take(30) {
        let Some(a) = card.select(&anchors).next() else {
            continue;
        };
        let Some(href) = a.value().attr("href") else {
            continue;
        };
        let title = a
            .select(&title_sel)
            .next()
            .map(|n| n.text().collect::<String>().trim().to_owned())
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let Ok(detail) = base.join(href) else {
            continue;
        };
        let id = id_re
            .captures(href)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_owned())
            .unwrap_or_else(|| href.to_owned());
        let rating = card
            .select(&rank)
            .next()
            .map(|n| n.text().collect::<String>())
            .unwrap_or_default();
        let year = card
            .select(&small)
            .last()
            .map(|n| n.text().collect::<String>())
            .unwrap_or_default();
        let poster = card
            .select(&image)
            .next()
            .and_then(|n| n.value().attr("style"))
            .and_then(|s| image_re.captures(s))
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_owned());
        let content = format!(
            "评分: {} | 年份: {} | 详情页: {}",
            rating.trim(),
            year.trim(),
            detail
        );
        out.push(Item {
            id: format!("clxiong-{id}"),
            title,
            content,
            images: poster.into_iter().collect(),
            detail,
        });
    }
    Ok(out)
}
fn parse_detail(
    body: &str,
    title: &str,
) -> Result<(Vec<Link>, Option<chrono::DateTime<Utc>>), ProviderError> {
    let doc = Html::parse_document(body);
    let links = Selector::parse(".mv_down a[href^='magnet:']")
        .map_err(|e| ProviderError::Parse(e.to_string()))?;
    let detail = Selector::parse(".mv_detail p").expect("selector");
    let mut out = Vec::new();
    for a in doc.select(&links) {
        let Some(raw) = a.value().attr("href") else {
            continue;
        };
        let name = a.text().collect::<String>().trim().to_owned();
        let mut link = Link::new(raw);
        link.work_title = Some(if name.is_empty() {
            title.to_owned()
        } else {
            format!("{title}-{name}")
        });
        out.push(link)
    }
    let date_re = Regex::new(r"最后更新于：?\s*(\d{4}[-/]\d{1,2}[-/]\d{1,2})").expect("regex");
    let datetime = doc.select(&detail).find_map(|p| {
        date_re
            .captures(&p.text().collect::<String>())
            .and_then(|c| {
                let s = c.get(1)?.as_str().replace('/', "-");
                NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()
            })
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| d.and_utc())
    });
    Ok((out, datetime))
}
#[async_trait]
impl Provider for ClxiongProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("clxiong", 2, KeywordFilterMode::Provider)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let form = [
            ("classid", "1,2"),
            ("show", "title"),
            ("tempid", "1"),
            ("keyboard", query),
        ];
        let first = tokio::time::timeout(
            ctx.timeout,
            ctx.client
                .post(self.endpoints.search.clone())
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .header(reqwest::header::REFERER, self.endpoints.base.as_str())
                .header(
                    reqwest::header::ACCEPT,
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
                )
                .form(&form)
                .send(),
        )
        .await
        .map_err(|_| ProviderError::Timeout)?
        .map_err(|e| ProviderError::Network(e.to_string()))?;
        let location = first
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(first.url().as_str());
        let sid = search_id(location)
            .ok_or_else(|| ProviderError::Protocol("redirect did not contain searchid".into()))?;
        let url = self
            .endpoints
            .base
            .join(&format!("e/search/result/?searchid={sid}"))
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        let response = request(&ctx.client, url, &self.endpoints.base)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        if !response.status().is_success() {
            return Err(ProviderError::Protocol(format!(
                "HTTP {}",
                response.status()
            )));
        }
        let items = parse_results(
            &response
                .text()
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?,
            &self.endpoints.base,
        )?;
        let jobs = items.into_iter().map(|item| async move {
            let response = request(&ctx.client, item.detail, &self.endpoints.base)
                .send()
                .await
                .ok()?;
            let (links, datetime) = parse_detail(&response.text().await.ok()?, &item.title).ok()?;
            if links.is_empty() {
                return None;
            }
            Some(SearchResult {
                id: item.id,
                source: Source::provider("clxiong"),
                datetime,
                title: item.title,
                content: item.content,
                links,
                tags: vec!["磁力链接".into(), "影视".into()],
                images: item.images,
            })
        });
        Ok(join_all(jobs).await.into_iter().flatten().collect())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn success() {
        let b = Url::parse("https://www.cilixiong.org/").unwrap();
        let r = parse_results(
            include_str!("../../tests/fixtures/providers/clxiong/success.html"),
            &b,
        )
        .unwrap();
        let (l, _) = parse_detail(
            include_str!("../../tests/fixtures/providers/clxiong/detail.html"),
            &r[0].title,
        )
        .unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(search_id("result/?searchid=7549").as_deref(), Some("7549"));
    }
    #[test]
    fn empty() {
        let b = Url::parse("https://x/").unwrap();
        assert!(
            parse_results(
                include_str!("../../tests/fixtures/providers/clxiong/empty.html"),
                &b
            )
            .unwrap()
            .is_empty()
        );
    }
    #[test]
    fn malformed() {
        assert!(
            parse_detail(
                include_str!("../../tests/fixtures/providers/clxiong/malformed.html"),
                "x"
            )
            .unwrap()
            .0
            .is_empty()
        );
    }
}
