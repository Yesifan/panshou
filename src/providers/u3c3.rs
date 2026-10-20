use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{Link, ProviderError, SearchResult};
use async_trait::async_trait;
use regex::Regex;
use scraper::{Html, Selector};
use std::{sync::LazyLock, time::Duration};
use url::Url;
static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"var\s+nmefafej\s*=\s*\"([^\"]+)\""#).unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").unwrap());
static SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub bases: Vec<Url>,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            bases: ["https://u3c3.com/", "https://u3c3u3c3.u3c3u3c3u3c3.com/"]
                .into_iter()
                .map(|s| Url::parse(s).unwrap())
                .collect(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct U3c3 {
    endpoints: Endpoints,
}
impl U3c3 {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
fn token(body: &str) -> Option<String> {
    body.lines()
        .map(str::trim)
        .filter(|s| !s.starts_with("//"))
        .find_map(|line| {
            let primary = TOKEN.captures(line).map(|c| c[1].to_owned());
            primary
                .or_else(|| {
                    if !line.contains("nmefafej") {
                        return None;
                    }
                    let start = line.find('"')? + 1;
                    let end = line[start..].find('"')?;
                    Some(line[start..start + end].into())
                })
                .filter(|s| s.len() > 5)
        })
}
fn parse(body: &str) -> Vec<SearchResult> {
    let doc = Html::parse_document(body);
    let td = Selector::parse("td").unwrap();
    let a = Selector::parse("a").unwrap();
    let magnets = Selector::parse("a[href^='magnet:']").unwrap();
    doc.select(&Selector::parse("tbody tr.default").unwrap())
        .filter_map(|tr| {
            let cells = tr.select(&td).collect::<Vec<_>>();
            if cells.len() < 5 {
                return None;
            }
            let title = cells[1].text().collect::<String>();
            if title.contains("[置顶]") {
                return None;
            }
            let title = cells[1].select(&a).next()?.text().collect::<String>();
            let title = SPACE
                .replace_all(TAG.replace_all(&title, "").trim(), " ")
                .into_owned();
            if title.is_empty() {
                return None;
            }
            let size = cells[3].text().collect::<String>().trim().to_owned();
            let when = cells[4].text().collect::<String>().trim().to_owned();
            let category = cells[0]
                .select(&a)
                .next()
                .and_then(|a| a.value().attr("title"))
                .unwrap_or("");
            let hash = format!("u3c3-{title}-{size}")
                .chars()
                .fold(0_i64, |h, c| h.wrapping_mul(31).wrapping_add(c as i64))
                .unsigned_abs();
            let mut row = SearchResult::provider(format!("u3c3-{hash}"), "u3c3");
            row.title = title;
            row.content = [
                ("分类", category),
                ("大小", size.as_str()),
                ("时间", when.as_str()),
            ]
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join(" | ");
            row.datetime = date(&when);
            row.tags = vec!["种子".into(), "磁力链接".into()];
            row.links = cells[2]
                .select(&magnets)
                .filter_map(|a| a.value().attr("href"))
                .filter(|s| !s.is_empty())
                .map(Link::new)
                .collect();
            Some(row)
        })
        .collect()
}
#[async_trait]
impl Provider for U3c3 {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("u3c3", 5, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut selected = None;
        let mut last = ProviderError::Protocol("u3c3 bootstrap has no search2 parameter".into());
        for base in &self.endpoints.bases {
            match request(||ctx.client.get(base.clone()).timeout(ctx.request_timeout(Duration::from_secs(10))).header("user-agent","Mozilla/5.0 Chrome/91.0.4472.124 Safari/537.36").header("accept","text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8"),2,Duration::from_millis(500),BODY_LIMIT).await{Ok(body)=>if let Some(t)=token(&String::from_utf8_lossy(&body)){selected=Some((base,t));break},Err(e)=>last=e}
        }
        let (base, token) = selected.ok_or(last)?;
        let mut url = base.clone();
        url.query_pairs_mut()
            .append_pair("search2", &token)
            .append_pair("search", query);
        let body = request(
            || {
                ctx.client
                    .get(url.clone())
                    .timeout(ctx.request_timeout(Duration::from_secs(15)))
                    .header("referer", base.as_str())
                    .header(
                        "user-agent",
                        "Mozilla/5.0 Chrome/91.0.4472.124 Safari/537.36",
                    )
            },
            2,
            Duration::from_millis(500),
            BODY_LIMIT,
        )
        .await?;
        complete(ctx, Ok(parse(&String::from_utf8_lossy(&body))))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bootstrap_and_rows() {
        assert_eq!(
            token("// var nmefafej = \"comment\"\nvar nmefafej = \"dynamic-token\";").as_deref(),
            Some("dynamic-token")
        );
        assert!(token("bad").is_none());
        let h = r#"<table><tbody><tr class="default"><td><a title="movie"></a></td><td><a>movie</a></td><td><a href="magnet:?xt=urn:btih:0123456789012345678901234567890123456789">download</a></td><td>1GB</td><td>2026-10-20</td></tr></tbody></table>"#;
        let rows = parse(h);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].links.len(), 1);
        assert!(rows[0].datetime.is_some());
        assert!(parse("bad").is_empty());
    }
}
