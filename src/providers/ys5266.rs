//! 66影视's GB18030 EmpireCMS POST search and magnet detail pages.
use super::{UA, clean};
use crate::core::{Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{Html, Selector};
use std::{collections::HashSet, sync::LazyLock, time::Duration};
use url::Url;
#[derive(Debug, Clone)]
pub struct Ys5266Provider {
    base: Url,
}
impl Default for Ys5266Provider {
    fn default() -> Self {
        Self::new(Url::parse("https://www.5266ys.net/").unwrap())
    }
}
impl Ys5266Provider {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
}
struct Item {
    id: String,
    title: String,
    url: Url,
    date: Option<chrono::DateTime<chrono::Utc>>,
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn items(body: &str, base: &Url) -> Result<Vec<Item>, ProviderError> {
    static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/(\d+)\.(?:htm|html?)$").unwrap());
    static DATE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"20\d{2}[-/]\d{1,2}[-/]\d{1,2}").unwrap());
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for node in doc.select(&sel(".listBox li")) {
        let a = node
            .select(&sel("h3 a"))
            .next()
            .or_else(|| node.select(&sel("a[href]")).next());
        let Some(a) = a else {
            continue;
        };
        let Some(url) = a.value().attr("href").and_then(|s| base.join(s).ok()) else {
            continue;
        };
        let title = clean(&a.inner_html());
        if title.is_empty() || !seen.insert(url.clone()) {
            continue;
        }
        let id = ID
            .captures(url.path())
            .map(|c| c[1].to_owned())
            .unwrap_or_else(|| format!("{:x}", fnv(url.as_str())));
        let text = clean(&node.inner_html());
        let date = DATE
            .find(&text)
            .and_then(|m| {
                chrono::NaiveDate::parse_from_str(&m.as_str().replace('/', "-"), "%Y-%m-%d").ok()
            })
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| d.and_utc());
        out.push(Item {
            id,
            title,
            url,
            date,
        });
        if out.len() == 50 {
            break;
        }
    }
    if out.is_empty() && doc.select(&sel(".listBox")).next().is_none() {
        return Err(ProviderError::Parse(
            "5266ys search: missing listBox".into(),
        ));
    }
    Ok(out)
}
fn fnv(s: &str) -> u32 {
    s.bytes()
        .fold(2166136261, |h, b| (h ^ u32::from(b)).wrapping_mul(16777619))
}
fn detail(body: &str, item: Item, base: &Url) -> Vec<SearchResult> {
    static HASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)btih:([a-z0-9]+)").unwrap());
    let doc = Html::parse_document(body);
    let content = doc
        .select(&sel(".contentinfo"))
        .next()
        .or_else(|| doc.select(&sel(".mainleft")).next())
        .unwrap_or_else(|| doc.root_element());
    let content = clean(&content.inner_html())
        .chars()
        .take(500)
        .collect::<String>();
    let image = doc
        .select(&sel("#text img, .contentinfo img, #dede_content img, img"))
        .filter_map(|n| n.value().attr("src"))
        .filter(|s| !s.to_ascii_lowercase().contains("logo"))
        .find_map(|s| base.join(s).ok());
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (i, a) in doc.select(&sel("a[href]")).enumerate() {
        let Some(raw) = a
            .value()
            .attr("href")
            .filter(|s| s.to_ascii_lowercase().starts_with("magnet:?"))
        else {
            continue;
        };
        if !seen.insert(raw.to_owned()) {
            continue;
        }
        let mut subtitle = clean(&a.inner_html());
        if subtitle.is_empty() {
            subtitle = Url::parse(raw)
                .ok()
                .and_then(|u| {
                    u.query_pairs()
                        .find(|(k, _)| k == "dn")
                        .map(|(_, v)| v.into_owned())
                })
                .unwrap_or_default();
        }
        let hash = HASH
            .captures(raw)
            .map(|c| format!("{}-{:x}", c[1].to_ascii_lowercase(), fnv(raw)))
            .unwrap_or_else(|| i.to_string());
        let mut r = SearchResult::provider(format!("5266ys-{}-{hash}", item.id), "5266ys");
        r.title = if subtitle.is_empty() {
            item.title.clone()
        } else {
            format!("{} - {subtitle}", item.title)
        };
        r.content = content.clone();
        r.datetime = item.date;
        let mut l = Link::new(raw);
        l.work_title = Some(item.title.clone());
        r.links.push(l);
        if let Some(image) = &image {
            r.images.push(image.to_string());
        }
        out.push(r);
    }
    out
}
#[async_trait]
impl Provider for Ys5266Provider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("5266ys", 3, KeywordFilterMode::Provider)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        if ctx.is_cancelled() {
            return Err(ProviderError::Timeout);
        }
        let request = |builder: reqwest::RequestBuilder| {
            builder
                .header("User-Agent", UA)
                .header("Referer", self.base.as_str())
                .header(
                    "Accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
                )
        };
        let (encoded, _, bad) = encoding_rs::GB18030.encode(query);
        if bad {
            return Err(ProviderError::Protocol(
                "5266ys search: keyword cannot be encoded".into(),
            ));
        }
        let encoded = url::form_urlencoded::byte_serialize(&encoded).collect::<String>();
        let form =
            format!("show=title%2Csmalltext&tempid=1&tbname=article&keyboard={encoded}&submit=");
        let fetch = |builder: reqwest::RequestBuilder, limit| async move {
            let response = crate::http::execute(builder)
                .await
                .map_err(|_| ProviderError::Network("5266ys: transport error".into()))?;
            let status = response.status();
            if status.as_u16() != 200 {
                return Err(ProviderError::Protocol(format!("5266ys HTTP {status}")));
            }
            let bytes = crate::http::read_response(response, limit)
                .await
                .map_err(|e| ProviderError::Protocol(format!("5266ys HTTP {status}: {e}")))?;
            crate::http::decode_body(&bytes, None, Some(encoding_rs::GB18030))
                .map_err(|_| ProviderError::Parse("5266ys: invalid GB18030".into()))
        };
        let body = tokio::time::timeout(
            ctx.request_timeout(Duration::from_secs(20)),
            fetch(
                request(
                    ctx.client
                        .post(self.base.join("e/search/index.php").unwrap())
                        .header("Content-Type", "application/x-www-form-urlencoded")
                        .body(form),
                ),
                4 << 20,
            ),
        )
        .await
        .map_err(|_| ProviderError::Timeout)??;
        let rows = items(&body, &self.base)?;
        let rows = stream::iter(rows.into_iter().map(|item| async move {
            if ctx.is_cancelled() {
                return Err(ProviderError::Timeout);
            }
            let body = tokio::time::timeout(
                ctx.request_timeout(Duration::from_secs(20)),
                fetch(request(ctx.client.get(item.url.clone())), 6 << 20),
            )
            .await
            .map_err(|_| ProviderError::Timeout)??;
            let rows = detail(&body, item, &self.base);
            if rows.is_empty() {
                return Err(ProviderError::Parse(
                    "5266ys: missing detail magnets".into(),
                ));
            }
            let rows = rows
                .into_iter()
                .filter(|r| super::matches(r, query))
                .collect::<Vec<_>>();
            ctx.publish(rows.clone());
            Ok(rows)
        }))
        .buffer_unordered(8)
        .collect::<Vec<Result<Vec<SearchResult>, ProviderError>>>()
        .await;
        let mut out = Vec::new();
        let mut error = None;
        for rows in rows {
            match rows {
                Ok(rows) => out.extend(rows),
                Err(e) => {
                    error.get_or_insert(e);
                }
            }
        }
        if let Some(error) = error {
            return Err(error);
        }
        Ok(out)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixtures() {
        let base = Url::parse("https://www.5266ys.net/").unwrap();
        let item = items(
            include_str!("../../tests/fixtures/providers/5266ys/success.html"),
            &base,
        )
        .unwrap()
        .remove(0);
        let r = detail(
            include_str!("../../tests/fixtures/providers/5266ys/detail.html"),
            item,
            &base,
        );
        assert_eq!(r.len(), 1);
        assert!(r[0].id.contains("abcdef"));
        assert_eq!(r[0].links[0].work_title.as_deref(), Some("测试影片"));
        assert!(
            items(
                include_str!("../../tests/fixtures/providers/5266ys/empty.html"),
                &base
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            items(
                include_str!("../../tests/fixtures/providers/5266ys/malformed.html"),
                &base
            )
            .is_err()
        );
    }
}
