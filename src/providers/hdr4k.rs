//! 4KHDR Discuz POST search and main/reply post-message resource links.
use super::{UA, clean, html_links};
use crate::core::{ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use scraper::{Html, Selector};
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::{Duration, Instant},
};
use url::Url;
pub struct Hdr4kProvider {
    base: Url,
    cache: Mutex<HashMap<String, (Instant, SearchResult)>>,
}
impl Default for Hdr4kProvider {
    fn default() -> Self {
        Self::new(Url::parse("https://www.4khdr.cn/").unwrap())
    }
}
impl Hdr4kProvider {
    pub fn new(base: Url) -> Self {
        Self {
            base,
            cache: Mutex::default(),
        }
    }
    async fn body(&self, request: reqwest::RequestBuilder) -> Result<String, ProviderError> {
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500 << (attempt - 1))).await;
            }
            let response = crate::http::execute(request.try_clone().unwrap()).await;
            match response {
                Ok(r) => {
                    let status = r.status();
                    if status.as_u16() != 200 {
                        return Err(ProviderError::Protocol(format!("hdr4k HTTP {status}")));
                    }
                    let b = crate::http::read_response(r, 16 << 20).await.map_err(|e| {
                        ProviderError::Protocol(format!("hdr4k HTTP {status}: {e}"))
                    })?;
                    return Ok(String::from_utf8_lossy(&b).into_owned());
                }
                Err(_) if attempt == 2 => {
                    return Err(ProviderError::Network(
                        "hdr4k: transport error after 3 attempts".into(),
                    ));
                }
                Err(_) => {}
            }
        }
        unreachable!()
    }
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn items(body: &str, q: &str) -> Result<Vec<(String, SearchResult)>, ProviderError> {
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    for node in doc.select(&sel(".slst.mtw ul li.pbw")) {
        let Some(id) = node
            .value()
            .attr("id")
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        else {
            continue;
        };
        let title = node
            .select(&sel("h3.xs3 a"))
            .next()
            .map(|n| clean(&n.inner_html()))
            .unwrap_or_default();
        let content = node
            .select(&sel("p"))
            .next()
            .map(|n| clean(&n.inner_html()))
            .unwrap_or_default();
        let text = format!("{title} {content}").to_lowercase();
        if title.is_empty()
            || !q
                .split_whitespace()
                .all(|w| text.contains(&w.to_lowercase()))
        {
            continue;
        }
        let mut r = SearchResult::provider(format!("hdr4k-{id}"), "hdr4k");
        r.title = title;
        r.content = content;
        r.tags = node
            .select(&sel("p span a.xi1"))
            .map(|n| clean(&n.inner_html()))
            .collect();
        r.datetime = node
            .select(&sel("p span"))
            .next()
            .map(|n| clean(&n.inner_html()))
            .and_then(|s| {
                ["%Y-%m-%d %H:%M", "%Y-%m-%d %H:%M:%S"]
                    .iter()
                    .find_map(|f| chrono::NaiveDateTime::parse_from_str(&s, f).ok())
            })
            .map(|d| d.and_utc());
        out.push((id.into(), r));
    }
    if out.is_empty() && doc.select(&sel(".slst.mtw")).next().is_none() {
        return Err(ProviderError::Parse(
            "hdr4k search: missing Discuz list".into(),
        ));
    }
    Ok(out)
}
fn detail(body: &str, mut r: SearchResult) -> Option<SearchResult> {
    let doc = Html::parse_document(body);
    let mut seen = HashSet::new();
    for node in doc.select(&sel(".t_f, [id^='postmessage_']")) {
        if r.content.len() < 50 {
            r.content = clean(&node.inner_html()).chars().take(500).collect();
        }
        r.links.extend(
            html_links(node)
                .into_iter()
                .filter(|l| seen.insert(l.url.clone())),
        );
    }
    (!r.links.is_empty()).then_some(r)
}
#[async_trait]
impl Provider for Hdr4kProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("hdr4k", 1, KeywordFilterMode::Provider)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        q: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let request = |r: reqwest::RequestBuilder| {
            r.timeout(ctx.request_timeout(Duration::from_secs(10)))
                .header("User-Agent", UA)
                .header("Referer", self.base.as_str())
        };
        let mut url = self.base.join("search.php").unwrap();
        url.query_pairs_mut().append_pair("mod", "forum");
        let body = self
            .body(request(
                ctx.client
                    .post(url)
                    .form(&[("srchtxt", q), ("searchsubmit", "yes")]),
            ))
            .await?;
        let entries = items(&body, q)?;
        let failures = &std::sync::Mutex::new(None);
        let rows = stream::iter(entries.into_iter().map(|(id, r)| async move {
            let cached = self
                .cache
                .lock()
                .unwrap()
                .get(&id)
                .filter(|(at, _)| at.elapsed() < Duration::from_secs(3600))
                .map(|(_, r)| r.clone());
            if let Some(r) = cached {
                ctx.publish(vec![r.clone()]);
                return Some(r);
            }
            let url = self.base.join(&format!("thread-{id}-1-1.html")).ok()?;
            let body =
                super::remember_failure(self.body(request(ctx.client.get(url))).await, failures)?;
            let r = detail(&body, r)?;
            let mut cache = self.cache.lock().unwrap();
            if cache.len() >= 512
                && let Some(k) = cache
                    .iter()
                    .min_by_key(|(_, v)| v.0)
                    .map(|(k, _)| k.clone())
            {
                cache.remove(&k);
            }
            cache.insert(id, (Instant::now(), r.clone()));
            drop(cache);
            ctx.publish(vec![r.clone()]);
            Some(r)
        }))
        .buffered(20)
        .collect::<Vec<_>>()
        .await;
        if let Some(error) = failures.lock().unwrap().take() {
            return Err(error);
        }
        if rows.iter().any(Option::is_none) {
            return Err(ProviderError::Unavailable(
                "hdr4k: one or more details failed or contained no usable links".into(),
            ));
        }
        Ok(rows.into_iter().flatten().collect())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixtures() {
        let (_, r) = items(
            include_str!("../../tests/fixtures/providers/hdr4k/success.html"),
            "测试",
        )
        .unwrap()
        .remove(0);
        assert_eq!(
            detail(
                include_str!("../../tests/fixtures/providers/hdr4k/detail.html"),
                r
            )
            .unwrap()
            .links
            .len(),
            1
        );
        assert!(
            items(
                include_str!("../../tests/fixtures/providers/hdr4k/empty.html"),
                "测试"
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            items(
                include_str!("../../tests/fixtures/providers/hdr4k/malformed.html"),
                "测试"
            )
            .is_err()
        );
    }
}
