use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, support::*};
use crate::core::{
    CloudType, Link, ProviderError, SearchResult, canonical_url_key, extract_links,
    extract_password,
};
use async_trait::async_trait;
use reqwest::{
    cookie::{CookieStore, Jar},
    header::{COOKIE, LOCATION, SET_COOKIE},
};
use std::time::Duration;
use url::Url;
#[derive(Debug, Clone)]
pub struct Qupanshe {
    pub base: Url,
    pub options: crate::http::ClientOptions,
}
impl Default for Qupanshe {
    fn default() -> Self {
        Self {
            base: Url::parse("https://www.qupanshe.com/").unwrap(),
            options: crate::http::ClientOptions::default(),
        }
    }
}
fn cookies(jar: &Jar, response: &reqwest::Response) {
    for value in response.headers().get_all(SET_COOKIE) {
        if let Ok(value) = value.to_str() {
            jar.add_cookie_str(value, response.url());
        }
    }
}
fn parse(bytes: &[u8], base: &Url) -> Result<Vec<SearchResult>, ProviderError> {
    let doc = html(bytes)?;
    let mut rows = vec![];
    for (index, node) in doc.select(&sel("li.pbw")).enumerate() {
        let title = text(node, "h3.xs3 a");
        if title.is_empty() {
            continue;
        }
        let detail = attr(node, "h3.xs3 a", "href").and_then(|s| base.join(&s).ok());
        let paragraphs: Vec<_> = node.select(&sel("p")).collect();
        let snippet = paragraphs.get(1).copied();
        let content = snippet
            .map(|n| clean(&n.text().collect::<String>()))
            .unwrap_or_default();
        let mut links = extract_links(&content);
        if let Some(snippet) = snippet {
            for anchor in snippet.select(&sel("a[href]")) {
                let raw = anchor.value().attr("href").unwrap();
                let mut link = Link::new(raw);
                if link.cloud_type == CloudType::Others {
                    continue;
                }
                link.password = extract_password(&content, raw);
                let key = canonical_url_key(raw);
                if let Some(old) = links.iter_mut().find(|l| canonical_url_key(&l.url) == key) {
                    if old.password.is_none() {
                        old.password = link.password;
                    }
                } else {
                    links.push(link);
                }
            }
        }
        if links.is_empty() {
            continue;
        }
        let span_selector = sel("span");
        let spans: Vec<_> = paragraphs
            .last()
            .into_iter()
            .flat_map(|n| n.select(&span_selector))
            .map(|n| clean(&n.text().collect::<String>()))
            .collect();
        let mut row = SearchResult::provider(
            format!(
                "qupanshe-{}",
                node.value()
                    .attr("id")
                    .map(str::to_owned)
                    .unwrap_or_else(|| index.to_string())
            ),
            "qupanshe",
        );
        row.title = title;
        row.datetime = spans.first().and_then(|s| date(s));
        row.content = format!(
            "{content} | 作者: {} | 分类: {} | 详情: {}",
            spans.get(1).cloned().unwrap_or_default(),
            spans.get(2).cloned().unwrap_or_default(),
            detail.map(|u| u.to_string()).unwrap_or_default()
        );
        row.links = links;
        rows.push(row);
    }
    Ok(rows)
}
impl Qupanshe {
    pub fn with_options(base: Url, options: crate::http::ClientOptions) -> Self {
        Self { base, options }
    }
    fn no_follow_client(&self) -> Result<reqwest::Client, ProviderError> {
        crate::http::HttpClientFactory::new()
            .no_follow(&self.options)
            .map_err(|e| ProviderError::Protocol(e.to_string()))
    }
    async fn get(
        &self,
        ctx: &SearchContext,
        jar: &Jar,
        url: Url,
        seconds: u64,
    ) -> Result<Vec<u8>, ProviderError> {
        tokio::time::timeout(
            ctx.request_timeout(Duration::from_secs(seconds)),
            self.get_inner(ctx, jar, url, seconds),
        )
        .await
        .map_err(|_| ProviderError::Timeout)?
    }
    async fn get_inner(
        &self,
        ctx: &SearchContext,
        jar: &Jar,
        mut url: Url,
        seconds: u64,
    ) -> Result<Vec<u8>, ProviderError> {
        for _ in 0..10 {
            if ctx.is_cancelled() {
                return Err(ProviderError::Timeout);
            }
            if url.origin() != self.base.origin() {
                return Err(ProviderError::Protocol(
                    "qupanshe rejected cross-origin session redirect".into(),
                ));
            }
            let mut request = self
                .no_follow_client()?
                .get(url.clone())
                .timeout(ctx.request_timeout(Duration::from_secs(seconds)))
                .header("user-agent", UA)
                .header("referer", self.base.as_str());
            if let Some(cookie) = jar.cookies(&url) {
                request = request.header(COOKIE, cookie);
            }
            let response = crate::http::execute(request).await.map_err(network)?;
            cookies(jar, &response);
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|s| s.to_str().ok())
                    .ok_or_else(|| {
                        ProviderError::Protocol("qupanshe redirect missing location".into())
                    })?;
                url = url
                    .join(location)
                    .map_err(|e| ProviderError::Protocol(e.to_string()))?;
                drop(response);
                continue;
            }
            if response.status().as_u16() != 200 {
                return Err(ProviderError::Protocol(format!(
                    "HTTP {}",
                    response.status().as_u16()
                )));
            }
            return crate::http::read_response(response, LIMIT)
                .await
                .map_err(network);
        }
        Err(ProviderError::Protocol(
            "qupanshe redirect limit exceeded".into(),
        ))
    }
    async fn post(
        &self,
        ctx: &SearchContext,
        jar: &Jar,
        query: &str,
        formhash: &str,
    ) -> Result<Url, ProviderError> {
        let url = self.base.join("search.php?mod=forum").unwrap();
        let mut last = ProviderError::Timeout;
        for attempt in 0..3 {
            if ctx.is_cancelled() {
                return Err(ProviderError::Timeout);
            }
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(2 << (attempt - 1))).await;
            }
            if ctx.is_cancelled() {
                return Err(ProviderError::Timeout);
            }
            let mut request = self
                .no_follow_client()?
                .post(url.clone())
                .header("user-agent", UA)
                .header("referer", self.base.as_str())
                .form(&[
                    ("formhash", formhash),
                    ("srchtxt", query),
                    ("searchsubmit", "yes"),
                ]);
            if let Some(cookie) = jar.cookies(&url) {
                request = request.header(COOKIE, cookie);
            }
            match crate::http::execute(request).await {
                Err(error) => last = network(error),
                Ok(response) => {
                    cookies(jar, &response);
                    let status = response.status().as_u16();
                    if status == 503 {
                        last = ProviderError::Protocol("HTTP 503".into());
                        continue;
                    }
                    let location = response
                        .headers()
                        .get(LOCATION)
                        .and_then(|s| s.to_str().ok())
                        .map(str::to_owned);
                    crate::http::read_response(response, LIMIT)
                        .await
                        .map_err(network)?;
                    let location = location.ok_or_else(|| {
                        ProviderError::Protocol(format!(
                            "qupanshe HTTP {status}: missing search redirect"
                        ))
                    })?;
                    return self
                        .base
                        .join(&location)
                        .map_err(|e| ProviderError::Protocol(e.to_string()));
                }
            }
        }
        Err(last)
    }
}
#[async_trait]
impl Provider for Qupanshe {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("qupanshe", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if ctx.is_cancelled() {
            return Err(ProviderError::Timeout);
        }
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let jar = Jar::default();
        let bytes = self.get(ctx, &jar, self.base.clone(), 15).await?;
        let formhash = {
            let doc = html(&bytes)?;
            doc.select(&sel("input[name='formhash']"))
                .filter_map(|n| n.value().attr("value"))
                .rfind(|s| !s.is_empty())
                .map(str::to_owned)
        }
        .ok_or_else(|| ProviderError::Parse("qupanshe missing formhash".into()))?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let url = tokio::time::timeout(
            ctx.request_timeout(Duration::from_secs(15)),
            self.post(ctx, &jar, query, &formhash),
        )
        .await
        .map_err(|_| ProviderError::Timeout)??;
        let bytes = self.get(ctx, &jar, url, 30).await?;
        let rows = parse(&bytes, &self.base)?;
        ctx.publish(rows.clone());
        Ok(rows)
    }
}
