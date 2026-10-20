use super::markup::{crc32, get, matches, sel, text, time};
use crate::{
    core::{CloudType, Link, ProviderError, SearchResult},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{ElementRef, Html};
use std::{collections::HashSet, sync::LazyLock};
use url::Url;
#[derive(Clone, Debug)]
pub struct Mizixing {
    base: Url,
}
impl Default for Mizixing {
    fn default() -> Self {
        Self::new(Url::parse("https://mizixing.com/").unwrap())
    }
}
static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [r"https?://pan\.quark\.cn/(?:s|g)/[0-9A-Za-z]+",r"https?://pan\.baidu\.com/s/[0-9A-Za-z\-_?=&]+",r"https?://pan\.xunlei\.com/s/[0-9A-Za-z\-_?=&]+",r"https?://(?:www\.)?(aliyundrive\.com|alipan\.com)/s/[0-9A-Za-z]+",r"https?://drive\.uc\.cn/s/[0-9A-Za-z]+",r"https?://(?:www\.)?(123pan\.com|123pan\.cn|123684\.com|123685\.com|123912\.com|123592\.com)/s/[0-9A-Za-z]+",r"https?://(?:www\.)?mypikpak\.com/s/[0-9A-Za-z]+",r#"https?://caiyun\.139\.com/[^\s<>"']+"#,r"magnet:\?xt=urn:btih:[0-9A-Za-z]+",r#"ed2k://[^\s<>"']+"#].into_iter().map(|s|Regex::new(s).unwrap()).collect()
});
static PASSWORD: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"提取码[:：]?\s*([0-9A-Za-z]+)",
        r"密码[:：]?\s*([0-9A-Za-z]+)",
        r"pwd\s*[=:：]\s*([0-9A-Za-z]+)",
        r"code\s*[=:：]\s*([0-9A-Za-z]+)",
    ]
    .into_iter()
    .map(|s| Regex::new(s).unwrap())
    .collect()
});
static URLS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"https?://[^\s<>"']+"#).unwrap());
struct Item {
    url: Url,
    result: SearchResult,
}
fn pass(s: &str) -> Option<String> {
    PASSWORD
        .iter()
        .find_map(|r| r.captures(s).map(|c| c[1].to_owned()))
}
fn classify(s: &str) -> Option<Link> {
    let raw = PATTERNS.iter().find_map(|r| r.find(s))?.as_str();
    let mut l = Link::new(raw);
    if raw.contains("123pan.cn/") {
        l.cloud_type = CloudType::Pan123;
    }
    Some(l)
}
fn noise(node: ElementRef<'_>) -> bool {
    node.ancestors()
        .filter_map(ElementRef::wrap)
        .chain(std::iter::once(node))
        .any(|n| {
            matches!(n.value().name(), "script" | "style")
                || n.value().id() == Some("respond")
                || n.value()
                    .classes()
                    .any(|c| ["bdsharebuttonbox", "post-views", "share", "relates"].contains(&c))
        })
}
fn search_items(body: &str, base: &Url) -> Result<Vec<Item>, ProviderError> {
    let doc = Html::parse_document(body);
    if doc
        .select(&sel(
            "article.excerpt,.content,.no-results,.search-no-results",
        ))
        .next()
        .is_none()
    {
        return Err(ProviderError::Parse(
            "mizixing search document changed".into(),
        ));
    }
    let mut out = Vec::new();
    for row in doc.select(&sel("article.excerpt")).take(12) {
        let Some(a) = row.select(&sel("h2 a")).next() else {
            continue;
        };
        let Some(url) = a.value().attr("href").and_then(|s| base.join(s).ok()) else {
            continue;
        };
        let mut title = text(a);
        if title.is_empty() {
            title = row.select(&sel("h2")).next().map(text).unwrap_or_default();
        }
        let mut result = SearchResult::provider(
            format!("mizixing-{}", crc32(url.as_str().as_bytes())),
            "mizixing",
        );
        result.title = title;
        result.content = row
            .select(&sel("p.note"))
            .next()
            .map(text)
            .unwrap_or_default();
        result.tags = row
            .select(&sel("header .label"))
            .next()
            .map(text)
            .filter(|s| !s.is_empty())
            .into_iter()
            .collect();
        out.push(Item { url, result });
    }
    Ok(out)
}
fn detail(body: &str, mut r: SearchResult) -> Result<SearchResult, ProviderError> {
    let doc = Html::parse_document(body);
    let content = [
        "article.article-content",
        ".article-content",
        ".entry-content",
    ]
    .iter()
    .find_map(|s| doc.select(&sel(s)).next())
    .unwrap_or(doc.root_element());
    let mut seen = HashSet::new();
    for a in content.select(&sel("a[href]")).filter(|a| !noise(*a)) {
        let Some(mut l) = a.value().attr("href").and_then(classify) else {
            continue;
        };
        if !seen.insert(l.url.clone()) {
            continue;
        }
        let parent = a.parent().and_then(ElementRef::wrap);
        let next_parent = parent.and_then(|p| p.next_siblings().find_map(ElementRef::wrap));
        let sibling = a.next_siblings().find_map(ElementRef::wrap);
        l.password = pass(&text(a))
            .or_else(|| a.value().attr("title").and_then(pass))
            .or_else(|| parent.and_then(|p| pass(&text(p))))
            .or_else(|| next_parent.and_then(|p| pass(&text(p))))
            .or_else(|| sibling.and_then(|p| pass(&text(p))));
        r.links.push(l);
    }
    let plain = content
        .descendants()
        .filter(|n| n.value().is_text())
        .filter(|n| !n.ancestors().filter_map(ElementRef::wrap).any(noise))
        .filter_map(|n| n.value().as_text())
        .map(|t| t.text.as_ref())
        .collect::<String>();
    for m in URLS.find_iter(&plain) {
        if let Some(mut l) = classify(m.as_str()) {
            if !seen.insert(l.url.clone()) {
                continue;
            }
            let start = plain[..m.start()]
                .char_indices()
                .rev()
                .nth(79)
                .map(|(i, _)| i)
                .unwrap_or(0);
            let end = plain[m.end()..]
                .char_indices()
                .nth(80)
                .map(|(i, _)| m.end() + i)
                .unwrap_or(plain.len());
            l.password = pass(&plain[start..end]);
            r.links.push(l);
        }
    }
    if r.links.is_empty() {
        return Err(ProviderError::Parse(
            "mizixing detail has no supported links".into(),
        ));
    }
    if r.content.is_empty() {
        r.content = doc
            .select(&sel("meta[name='description']"))
            .next()
            .and_then(|n| n.value().attr("content"))
            .unwrap_or("")
            .to_owned();
    }
    for s in [
        "meta[property='article:modified_time']",
        "meta[property='article:published_time']",
        "meta[name='article:modified_time']",
        "meta[name='article:published_time']",
    ] {
        if let Some(t) = doc
            .select(&sel(s))
            .next()
            .and_then(|n| n.value().attr("content"))
            .and_then(time)
        {
            r.datetime = Some(t);
            break;
        }
    }
    r.tags.extend(
        doc.select(&sel(".breadcrumbs a"))
            .map(text)
            .filter(|s| !s.is_empty() && !s.contains("首页")),
    );
    r.tags.sort();
    r.tags.dedup();
    for l in &mut r.links {
        l.work_title = Some(r.title.clone());
    }
    Ok(r)
}
impl Mizixing {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self.base.clone();
        url.query_pairs_mut().append_pair("s", query);
        let items = search_items(&get(ctx, url, self.base.as_str(), 12, 3).await?, &self.base)?;
        let mut jobs = stream::iter(items.into_iter().map(|i| async move {
            let r = detail(
                &get(ctx, i.url.clone(), i.url.as_str(), 10, 3).await?,
                i.result,
            )?;
            if matches(&format!("{} {}", r.title, r.content), query) {
                ctx.publish(vec![r.clone()]);
                Ok(Some(r))
            } else {
                Ok(None)
            }
        }))
        .buffer_unordered(6);
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
impl Provider for Mizixing {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("mizixing", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        tokio::time::timeout(ctx.remaining(), self.run(ctx, query))
            .await
            .map_err(|_| ProviderError::Timeout)?
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn full_chain_retry_metadata_and_empty() {
        use std::{
            sync::{
                Arc,
                atomic::{AtomicUsize, Ordering},
            },
            time::Duration,
        };
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{path, query_param},
        };
        let s = MockServer::start().await;
        let n = Arc::new(AtomicUsize::new(0));
        let count = n.clone();
        Mock::given(path("/")).and(query_param("s","film")).respond_with(move |_:&wiremock::Request|if count.fetch_add(1,Ordering::SeqCst)<2 {ResponseTemplate::new(500)}else{ResponseTemplate::new(200).set_body_string("<article class='excerpt'><h2><a href='/1'>Film</a></h2><header><span class='label'>电影</span></header></article>")}).expect(3).mount(&s).await;
        Mock::given(path("/1")).respond_with(ResponseTemplate::new(200).set_body_string("<meta name='description' content='summary'><meta property='article:modified_time' content='2026-10-12T00:00:00Z'><article class='article-content'><p><a href='https://pan.quark.cn/s/abc'>提取码：abcd</a></p></article>")).expect(1).mount(&s).await;
        let p = Mizixing::new(Url::parse(&s.uri()).unwrap());
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
        let r = p.search(&ctx, "film").await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].links[0].password.as_deref(), Some("abcd"));
        assert_eq!(r[0].content, "summary");
        assert!(r[0].datetime.is_some());
        assert!(
            search_items("<div class='no-results'></div>", &p.base)
                .unwrap()
                .is_empty()
        );
        assert!(search_items("changed", &p.base).is_err());
    }
}
