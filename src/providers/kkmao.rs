//! 夸克猫 WordPress excerpts → article-content links.
use crate::core::{Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use std::{collections::HashSet, sync::LazyLock, time::Duration};
use url::Url;
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/(\d+)\.html").unwrap());
static SHARE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?://pan\.quark\.cn/s/[0-9A-Za-z]+").unwrap());
static PASSWORDS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"提取码[:：]?\s*([0-9A-Za-z]+)",
        r"密码[:：]?\s*([0-9A-Za-z]+)",
        r"pwd\s*[=:：]\s*([0-9A-Za-z]+)",
        r"code\s*[=:：]\s*([0-9A-Za-z]+)",
    ]
    .into_iter()
    .map(|r| Regex::new(r).unwrap())
    .collect()
});
#[derive(Debug, Clone)]
pub struct Kkmao {
    base: Url,
}
impl Default for Kkmao {
    fn default() -> Self {
        Self::new(Url::parse("https://www.kuakemao.com/").unwrap())
    }
}
struct Item {
    result: SearchResult,
    url: Url,
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn text(n: ElementRef<'_>) -> String {
    n.text().collect::<String>().trim().to_owned()
}
fn timestamp(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
        .or_else(|| {
            NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|t| t.and_utc())
        })
        .or_else(|| {
            NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|t| t.and_utc())
        })
}
fn parse_search(body: &str, base: &Url) -> Result<Vec<Item>, ProviderError> {
    let doc = Html::parse_document(body);
    if doc
        .select(&sel(
            "article.excerpt, #content, .content, .no-results, .search-no-results",
        ))
        .next()
        .is_none()
    {
        return Err(ProviderError::Parse("kkmao search document missing".into()));
    }
    let mut out = Vec::new();
    for article in doc.select(&sel("article.excerpt")) {
        let Some(anchor) = article.select(&sel("header h2 a")).next() else {
            continue;
        };
        let Some(url) = anchor.value().attr("href").and_then(|s| base.join(s).ok()) else {
            continue;
        };
        let Some(id) = ID.captures(url.as_str()).map(|c| c[1].to_owned()) else {
            continue;
        };
        let title = text(anchor);
        if title.is_empty() {
            continue;
        }
        let mut r = SearchResult::provider(format!("kkmao-{id}"), "kkmao");
        r.title = title;
        r.content = article
            .select(&sel("p.note"))
            .next()
            .map(text)
            .unwrap_or_default();
        r.tags = article
            .select(&sel(".meta a.cat"))
            .next()
            .map(text)
            .filter(|s| !s.is_empty())
            .into_iter()
            .collect();
        r.datetime = article
            .select(&sel(".meta time"))
            .next()
            .and_then(|n| timestamp(&text(n)));
        out.push(Item { result: r, url });
    }
    Ok(out)
}
fn password(s: &str) -> Option<String> {
    PASSWORDS
        .iter()
        .find_map(|r| r.captures(s).map(|c| c[1].to_owned()))
}
fn extract(body: &str) -> Vec<Link> {
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for anchor in doc.select(&sel(".article-content a[href]")) {
        let Some(url) = anchor
            .value()
            .attr("href")
            .and_then(|s| SHARE.find(s))
            .map(|m| m.as_str().to_owned())
        else {
            continue;
        };
        if !seen.insert(url.clone()) {
            continue;
        }
        let parent = anchor.parent().and_then(ElementRef::wrap);
        let next_parent = parent.and_then(|p| p.next_siblings().find_map(ElementRef::wrap));
        let sibling = anchor.next_siblings().find_map(ElementRef::wrap);
        let pass = password(&text(anchor))
            .or_else(|| anchor.value().attr("title").and_then(password))
            .or_else(|| parent.and_then(|p| password(&text(p))))
            .or_else(|| next_parent.and_then(|p| password(&text(p))))
            .or_else(|| sibling.and_then(|p| password(&text(p))));
        let mut link = Link::new(url);
        link.password = pass;
        out.push(link);
    }
    out
}
impl Kkmao {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
    async fn fetch(
        &self,
        ctx: &SearchContext,
        url: &Url,
        referer: &str,
        detail: bool,
    ) -> Result<String, ProviderError> {
        crate::providers::b_registry::http::text(
            ctx.client
                .get(url.clone())
                .header("user-agent", "Mozilla/5.0 Chrome/120.0.0.0 Safari/537.36")
                .header(
                    "accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
                )
                .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
                .header("referer", referer),
            if detail { 2 } else { 3 },
            200,
            true,
            Duration::from_secs(if detail { 10 } else { 12 }),
            16 << 20,
        )
        .await
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut search = self.base.clone();
        search.query_pairs_mut().append_pair("s", query);
        let items = parse_search(
            &self.fetch(ctx, &search, self.base.as_str(), false).await?,
            &self.base,
        )?;
        let mut jobs = stream::iter(items.into_iter().map(|mut item| async move {
            item.result.links =
                extract(&self.fetch(ctx, &item.url, item.url.as_str(), true).await?);
            for l in &mut item.result.links {
                l.work_title = Some(item.result.title.clone());
            }
            let hay = format!("{} {}", item.result.title, item.result.content).to_lowercase();
            if !item.result.links.is_empty()
                && query
                    .to_lowercase()
                    .split_whitespace()
                    .all(|w| hay.contains(w))
            {
                ctx.publish(vec![item.result.clone()]);
                Ok(Some(item.result))
            } else {
                Ok(None)
            }
        }))
        .buffer_unordered(8);
        let mut out = Vec::new();
        let mut error = None;
        while let Some(r) = jobs.next().await {
            match r {
                Ok(Some(r)) => out.push(r),
                Ok(None) => {}
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
impl Provider for Kkmao {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("kkmao", 2, KeywordFilterMode::Core)
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
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, query_param},
    };
    #[test]
    fn parser_contracts() {
        let base = Url::parse("https://www.kuakemao.com/").unwrap();
        let r=parse_search(r#"<article class="excerpt"><header><h2><a href="/123.html">Film</a></h2></header><p class="note">summary</p><div class="meta"><a class="cat">Movies</a><time>2026-10-12</time></div></article>"#,&base).unwrap();
        assert_eq!(r[0].result.id, "kkmao-123");
        assert_eq!(r[0].result.tags, ["Movies"]);
        assert!(r[0].result.datetime.is_some());
        let l = extract(
            r#"<div class="article-content"><p><a href="https://pan.quark.cn/s/abcdef?x=1">link</a> 提取码：1234</p><a href="https://pan.quark.cn/s/abcdef">duplicate</a></div>"#,
        );
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].password.as_deref(), Some("1234"));
        assert!(
            parse_search("<div class='no-results'></div>", &base)
                .unwrap()
                .is_empty()
        );
        assert!(parse_search("<html>bad gateway</html>", &base).is_err());
    }
    #[tokio::test]
    async fn search_detail_retry_counts() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let server = MockServer::start().await;
        let count = Arc::new(AtomicUsize::new(0));
        let n = count.clone();
        Mock::given(method("GET")).and(path("/")).and(query_param("s","Film")).respond_with(move |_:&wiremock::Request|if n.fetch_add(1,Ordering::SeqCst)<2 {ResponseTemplate::new(500)} else {ResponseTemplate::new(200).set_body_string(r#"<article class="excerpt"><header><h2><a href="/1.html">Film</a></h2></header></article>"#)}).expect(3).mount(&server).await;
        Mock::given(path("/1.html"))
            .respond_with(ResponseTemplate::new(403))
            .expect(2)
            .mount(&server)
            .await;
        let p = Kkmao::new(Url::parse(&server.uri()).unwrap());
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
        assert!(
            p.search(&ctx, "Film")
                .await
                .unwrap_err()
                .to_string()
                .contains("403")
        );
    }
}
