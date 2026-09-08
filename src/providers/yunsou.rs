use async_trait::async_trait;
use chrono::NaiveDate;
use regex::Regex;
use scraper::{Html, Selector};
use url::Url;

use crate::core::{Link, ProviderError, SearchResult, Source};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://wpys.cc/").expect("static URL"),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct YunsouProvider {
    endpoints: Endpoints,
}
impl YunsouProvider {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}

fn parse_page(body: &str) -> Result<(Vec<SearchResult>, usize), ProviderError> {
    let doc = Html::parse_document(body);
    let item_sel =
        Selector::parse(".list .item").map_err(|e| ProviderError::Parse(e.to_string()))?;
    let title_sel = Selector::parse(".title").expect("selector");
    let all_sel = Selector::parse("*").expect("selector");
    let type_sel = Selector::parse(".type").expect("selector");
    let time_sel = Selector::parse(".type.time").expect("selector");
    let copy =
        Regex::new(r#"copyText\([^,]+,\s*'[^']*',\s*'([^']+)',\s*'([^']*)'"#).expect("regex");
    let pwd = Regex::new(r"[?&]pwd=([0-9A-Za-z]+)").expect("regex");
    let mut out = Vec::new();
    for item in doc.select(&item_sel).take(100) {
        let title = item
            .value()
            .attr("data-title")
            .map(str::to_owned)
            .unwrap_or_else(|| {
                item.select(&title_sel)
                    .next()
                    .map(|n| n.text().collect())
                    .unwrap_or_default()
            });
        let title = clean(&title);
        if title.is_empty() {
            continue;
        }
        let found = item.select(&all_sel).find_map(|n| {
            n.value()
                .attr("onclick")
                .or_else(|| n.value().attr("@click.stop"))
                .and_then(|v| copy.captures(v))
                .map(|capture| {
                    (
                        capture.get(1).map(|m| m.as_str().trim().to_owned()),
                        capture.get(2).map(|m| m.as_str().trim().to_owned()),
                    )
                })
        });
        let Some((raw, inline_password)) = found else {
            continue;
        };
        let raw = raw.unwrap_or_default();
        if raw.is_empty() {
            continue;
        }
        let password = inline_password.filter(|s| !s.is_empty()).or_else(|| {
            pwd.captures(&raw)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_owned())
        });
        let mut link = Link::new(&raw);
        link.password = password;
        let date = item
            .select(&time_sel)
            .next()
            .and_then(|n| {
                NaiveDate::parse_from_str(n.text().collect::<String>().trim(), "%Y-%m-%d").ok()
            })
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| d.and_utc());
        let content = item
            .select(&type_sel)
            .next()
            .map(|n| clean(&n.text().collect::<String>()))
            .unwrap_or_default();
        out.push(SearchResult {
            id: format!("yunsou-{}", raw),
            source: Source::provider("yunsou"),
            datetime: date,
            title,
            content,
            links: vec![link],
            tags: vec![],
            images: vec![],
        });
    }
    let pagination = Selector::parse("el-pagination").expect("selector");
    let total = doc
        .select(&pagination)
        .next()
        .and_then(|n| n.value().attr(":total"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(out.len());
    Ok((out, total))
}
fn clean(v: &str) -> String {
    v.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn path_query(query: &str) -> String {
    percent_encoding::utf8_percent_encode(query, percent_encoding::NON_ALPHANUMERIC).to_string()
}

#[async_trait]
impl Provider for YunsouProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("yunsou", 2, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let encoded = path_query(query.trim());
        let mut all = Vec::new();
        for page in 1..=10 {
            let path = if page == 1 {
                format!("s/{encoded}.html")
            } else {
                format!("s/{encoded}-{page}.html")
            };
            let url = self
                .endpoints
                .base
                .join(&path)
                .map_err(|e| ProviderError::Protocol(e.to_string()))?;
            let response = tokio::time::timeout(
                ctx.timeout,
                ctx.client
                    .get(url)
                    .header(reqwest::header::REFERER, self.endpoints.base.as_str())
                    .send(),
            )
            .await
            .map_err(|_| ProviderError::Timeout)?
            .map_err(|e| ProviderError::Network(e.to_string()))?;
            if !response.status().is_success() {
                if page == 1 {
                    return Err(ProviderError::Protocol(format!(
                        "HTTP {}",
                        response.status()
                    )));
                }
                break;
            }
            let (mut rows, total) = parse_page(
                &response
                    .text()
                    .await
                    .map_err(|e| ProviderError::Network(e.to_string()))?,
            )?;
            all.append(&mut rows);
            if page * 10 >= total || all.len() >= 100 {
                break;
            }
        }
        all.truncate(100);
        Ok(all)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn success() {
        let (r, t) = parse_page(include_str!(
            "../../tests/fixtures/providers/yunsou/success.html"
        ))
        .unwrap();
        assert_eq!(t, 1);
        assert_eq!(r[0].links[0].password.as_deref(), Some("abcd"));
    }
    #[test]
    fn empty() {
        assert!(
            parse_page(include_str!(
                "../../tests/fixtures/providers/yunsou/empty.html"
            ))
            .unwrap()
            .0
            .is_empty()
        );
    }
    #[test]
    fn malformed_does_not_panic() {
        assert!(
            parse_page(include_str!(
                "../../tests/fixtures/providers/yunsou/malformed.html"
            ))
            .unwrap()
            .0
            .is_empty()
        );
    }
}
