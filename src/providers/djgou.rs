use crate::core::{Link, ProviderError, SearchResult, Source};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{Html, Selector};
use url::Url;

const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/114 Safari/537.36";
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://duanjugou.top").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Djgou {
    endpoints: Endpoints,
}
impl Djgou {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
#[derive(Clone)]
struct Hit {
    id: String,
    title: String,
    url: Url,
    date: Option<DateTime<Utc>>,
}
fn parse_search(body: &str, base: &Url) -> Vec<Hit> {
    let d = Html::parse_document(body);
    let row = Selector::parse("article.post-item-row").unwrap();
    let a = Selector::parse("h2.post-title a").unwrap();
    let date = Selector::parse(".post-date").unwrap();
    d.select(&row)
        .filter_map(|r| {
            let a = r.select(&a).next()?;
            let url = base.join(a.value().attr("href")?).ok()?;
            let title = a.text().collect::<String>().trim().to_owned();
            if title.is_empty() {
                return None;
            }
            let ds = r.select(&date).next().map(|x| x.text().collect::<String>());
            let dt = ds
                .and_then(|x| NaiveDate::parse_from_str(x.trim(), "%Y-%m-%d").ok())
                .and_then(|x| x.and_hms_opt(0, 0, 0))
                .map(|x| DateTime::from_naive_utc_and_offset(x, Utc));
            Some(Hit {
                id: url.path().trim_matches('/').replace('/', "_"),
                title,
                url,
                date: dt,
            })
        })
        .collect()
}
fn btwaf(body: &str, base: &Url) -> Option<Url> {
    Regex::new(r#"window\.location\.href\s*=\s*["']([^"']*btwaf=[^"']+)["']"#)
        .unwrap()
        .captures(body)
        .and_then(|c| c.get(1))
        .and_then(|m| base.join(m.as_str()).ok())
}
fn parse_detail(body: &str) -> (Vec<Link>, String) {
    let q = Regex::new(r"https?://pan\.quark\.cn/s/[0-9A-Za-z_-]+").unwrap();
    let pwd = Regex::new(r"提取码[:：]\s*([0-9A-Za-z]{4})")
        .unwrap()
        .captures(body)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_owned());
    let mut seen = std::collections::HashSet::new();
    let links = q
        .find_iter(body)
        .filter(|m| seen.insert(m.as_str().to_owned()))
        .map(|m| {
            let mut l = Link::new(m.as_str());
            l.password = pwd.clone();
            l
        })
        .collect();
    let d = Html::parse_document(body);
    let sel = Selector::parse("div.post-content, div.erx-wrap").unwrap();
    let content = d
        .select(&sel)
        .next()
        .map(|x| {
            x.text()
                .collect::<Vec<_>>()
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    (links, content.chars().take(300).collect())
}
#[async_trait]
impl Provider for Djgou {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("djgou", 2, KeywordFilterMode::Core)
    }

    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut url = self
            .endpoints
            .base
            .join("search.php")
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("page", "1");
        let mut body = ctx
            .client
            .get(url)
            .header("user-agent", UA)
            .header(
                "accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
            )
            .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
            .header("upgrade-insecure-requests", "1")
            .header("cache-control", "max-age=0")
            .header("referer", self.endpoints.base.as_str())
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?
            .error_for_status()
            .map_err(|e| ProviderError::Network(e.to_string()))?
            .text()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        if parse_search(&body, &self.endpoints.base).is_empty()
            && let Some(challenge) = btwaf(&body, &self.endpoints.base)
        {
            body = ctx
                .client
                .get(challenge)
                .header("user-agent", UA)
                .header(
                    "accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
                )
                .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
                .header("referer", self.endpoints.base.as_str())
                .send()
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?
                .text()
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?;
        }
        let client = ctx.client.clone();
        let referer = self.endpoints.base.as_str().to_owned();
        let futures = parse_search(&body, &self.endpoints.base)
            .into_iter()
            .map(move |hit| {
                let client = client.clone();
                let referer = referer.clone();
                async move {
                    let body = client
                        .get(hit.url)
                        .header("user-agent", UA)
                        .header(
                            "accept",
                            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8",
                        )
                        .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
                        .header("referer", referer)
                        .send()
                        .await
                        .ok()?
                        .error_for_status()
                        .ok()?
                        .text()
                        .await
                        .ok()?;
                    let (links, content) = parse_detail(&body);
                    (!links.is_empty()).then(|| SearchResult {
                        id: format!("djgou-{}", hit.id),
                        source: Source::provider("djgou"),
                        datetime: hit.date,
                        title: hit.title,
                        content,
                        links,
                        tags: vec!["短剧".into()],
                        images: vec![],
                    })
                }
            });
        let rows = stream::iter(futures).buffered(15).collect::<Vec<_>>().await;
        Ok(rows.into_iter().flatten().collect())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_success() {
        let h = parse_search(
            include_str!("../../tests/fixtures/providers/djgou/success.html"),
            &Endpoints::default().base,
        );
        assert_eq!(h.len(), 1);
        let (l, _) = parse_detail(include_str!(
            "../../tests/fixtures/providers/djgou/success-detail.html"
        ));
        assert_eq!(l[0].password.as_deref(), Some("a1b2"));
    }
    #[test]
    fn fixture_empty() {
        assert!(
            parse_search(
                include_str!("../../tests/fixtures/providers/djgou/empty.html"),
                &Endpoints::default().base
            )
            .is_empty()
        );
        assert!(parse_detail("x").0.is_empty())
    }
    #[test]
    fn fixture_malformed() {
        assert!(
            parse_search(
                include_str!("../../tests/fixtures/providers/djgou/malformed.html"),
                &Endpoints::default().base
            )
            .is_empty()
        )
    }
}
