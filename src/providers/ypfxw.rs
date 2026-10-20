use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{Link, ProviderError, SearchResult};
use async_trait::async_trait;
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use std::{collections::HashSet, sync::LazyLock, time::Duration};
use url::Url;
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/post/(\d+)\.html").unwrap());
static SHARE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https?://(?:pan\.quark\.cn/[sg]/[0-9A-Za-z]+|www\.aliyundrive\.com/(?:s/|drive/folder/)[0-9A-Za-z]+|pan\.(?:baidu|xunlei)\.com/s/[0-9A-Za-z_\-]+|123pan\.com/s/[0-9A-Za-z]+)").unwrap()
});
static URL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"https?://[^\s<>\"']+"#).unwrap());
static PWD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:提取码[:：]?\s*|密码[:：]?\s*|pwd\s*[=:：]\s*|code\s*[=:：]\s*)([0-9A-Za-z]+)")
        .unwrap()
});
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://ypfxw.com/").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Ypfxw {
    endpoints: Endpoints,
}
impl Ypfxw {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn text(e: ElementRef<'_>) -> String {
    e.text().collect::<Vec<_>>().join(" ").trim().into()
}
fn first(e: ElementRef<'_>, s: &str) -> String {
    e.select(&sel(s)).next().map(text).unwrap_or_default()
}
fn list(body: &str, base: &Url) -> Vec<(Url, SearchResult)> {
    let d = Html::parse_document(body);
    d.select(&sel("div.list ul > li"))
        .filter_map(|e| {
            let a = e.select(&sel("div.imgr h2 a")).next()?;
            let title = text(a);
            if title.is_empty() {
                return None;
            }
            let url = base.join(a.value().attr("href")?).ok()?;
            let id = ID.captures(url.path())?[1].to_owned();
            let mut r = SearchResult::provider(format!("ypfxw-{id}"), "ypfxw");
            r.title = title;
            r.content = first(e, "div.imgr p");
            r.tags = e
                .select(&sel(".info span"))
                .next()
                .map(text)
                .filter(|s| !s.is_empty())
                .into_iter()
                .chain(
                    e.select(&sel(".info span.tag a"))
                        .map(text)
                        .filter(|s| !s.is_empty()),
                )
                .collect();
            r.datetime = e
                .select(&sel(".info span i.fa-clock-o"))
                .next()
                .and_then(|e| e.parent())
                .and_then(ElementRef::wrap)
                .map(text)
                .and_then(|s| date(&s))
                .or_else(|| Some(chrono::Utc::now()));
            Some((url, r))
        })
        .collect()
}
fn link(raw: &str, context: &str) -> Option<Link> {
    let raw = raw.trim().trim_end_matches(['。', '，', ',', ';', '；']);
    if !SHARE.is_match(raw) {
        return None;
    }
    let u = Url::parse(raw).ok()?;
    if !matches!(
        u.host_str(),
        Some(
            "pan.quark.cn"
                | "www.aliyundrive.com"
                | "pan.baidu.com"
                | "pan.xunlei.com"
                | "123pan.com"
        )
    ) {
        return None;
    }
    let mut l = Link::new(raw);
    l.password = password(raw).or_else(|| PWD.captures(context).map(|c| c[1].into()));
    Some(l)
}
fn detail(body: &str) -> Vec<Link> {
    let doc = Html::parse_document(body);
    let Some(container) = doc.select(&sel(".article_content")).next() else {
        return vec![];
    };
    let mut seen = HashSet::new();
    let mut out = vec![];
    for a in container.select(&sel("a[href]")) {
        let mut contexts = vec![text(a), a.value().attr("title").unwrap_or("").into()];
        // Prefer the text belonging to this anchor. Do not scan through a
        // sibling link or borrow another link's extraction code.
        let mut sibling = a.next_sibling();
        let mut following = String::new();
        while let Some(node) = sibling {
            let fragment = if let Some(element) = ElementRef::wrap(node) {
                if element.value().name() == "a" || element.select(&sel("a[href]")).next().is_some()
                {
                    break;
                }
                text(element)
            } else if let Some(value) = node.value().as_text() {
                value.text.to_string()
            } else {
                String::new()
            };
            if let Some(next_url) = URL_RE.find(&fragment) {
                following.push_str(&fragment[..next_url.start()]);
                break;
            }
            following.push_str(&fragment);
            following.push(' ');
            sibling = node.next_sibling();
        }
        contexts.push(following);
        if let Some(parent) = a.parent().and_then(ElementRef::wrap)
            && parent.select(&sel("a[href]")).count() == 1
            && !URL_RE.is_match(&text(parent))
        {
            contexts.push(text(parent));
            if let Some(next) = parent.next_sibling().and_then(ElementRef::wrap)
                && next.select(&sel("a[href]")).next().is_none()
                && !URL_RE.is_match(&text(next))
            {
                contexts.push(text(next));
            }
        }
        let context = contexts
            .into_iter()
            .find(|s| PWD.is_match(s))
            .unwrap_or_default();
        if let Some(l) = link(a.value().attr("href").unwrap(), &context)
            && seen.insert(l.url.clone())
        {
            out.push(l)
        }
    }
    let plain = text(container);
    let urls = URL_RE.find_iter(&plain).collect::<Vec<_>>();
    for (index, m) in urls.iter().enumerate() {
        // Codes following this URL stop at the next URL, never at a global
        // look-behind window that can capture a previous resource's code.
        let mut end = urls
            .get(index + 1)
            .map_or(plain.len(), |next| next.start())
            .min(m.end() + 80);
        while !plain.is_char_boundary(end) {
            end -= 1
        }
        if let Some(link) = link(m.as_str(), &plain[m.end()..end])
            && seen.insert(link.url.clone())
        {
            out.push(link)
        }
    }
    out
}
impl Ypfxw {
    async fn fetch(
        &self,
        ctx: &SearchContext,
        url: Url,
        detail: bool,
    ) -> Result<Vec<u8>, ProviderError> {
        tokio::time::timeout(ctx.request_timeout(Duration::from_secs(if detail{10}else{12})),request(||ctx.client.get(url.clone()).header("referer",if detail{url.as_str()}else{self.endpoints.base.as_str()}).header("user-agent","Mozilla/5.0 Chrome/120.0.0.0 Safari/537.36").header("accept","text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8"),if detail{2}else{3},Duration::from_millis(200),BODY_LIMIT)).await.map_err(|_|ProviderError::Timeout)?
    }
}
#[async_trait]
impl Provider for Ypfxw {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("ypfxw", 2, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut url = self.endpoints.base.join("/search.php").unwrap();
        url.query_pairs_mut().append_pair("q", query);
        let body = self.fetch(ctx, url, false).await?;
        let rows = list(&String::from_utf8_lossy(&body), &self.endpoints.base);
        collect_details(
            ctx,
            rows.into_iter().map(|(url, mut row)| async move {
                let body = self.fetch(ctx, url, true).await?;
                row.links = detail(&String::from_utf8_lossy(&body));
                Ok(row)
            }),
            12,
        )
        .await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn passwords_are_local_to_sibling_links_and_text_nodes() {
        for html in [
            r#"<p><a href="https://pan.quark.cn/s/one">one</a><span>提取码：abcd</span><a href="https://pan.quark.cn/s/two">two</a><span>密码：efgh</span><a href="https://pan.quark.cn/s/three">three</a></p>"#,
            r#"<p>https://pan.quark.cn/s/one<span>提取码：abcd</span>https://pan.quark.cn/s/two<span>密码：efgh</span>https://pan.quark.cn/s/three</p>"#,
        ] {
            let rows = detail(&format!("<div class=article_content>{html}</div>"));
            assert_eq!(rows.len(), 3);
            assert_eq!(rows[0].password.as_deref(), Some("abcd"));
            assert_eq!(rows[1].password.as_deref(), Some("efgh"));
            assert_eq!(rows[2].password, None);
        }
    }
    #[test]
    fn anchor_does_not_borrow_code_from_following_plain_url() {
        let links = detail(
            r#"<div class=article_content><p><a href="https://pan.quark.cn/s/one">one</a> https://pan.quark.cn/s/two 密码：efgh</p></div>"#,
        );
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].password, None);
        assert_eq!(links[1].password.as_deref(), Some("efgh"));
    }
    #[test]
    fn article_links_and_codes() {
        let r = detail(
            r#"<div class="article_content"><p><a href="https://pan.quark.cn/g/abc">share</a> 提取码：abcd</p><p>https://pan.baidu.com/s/a 密码: efgh</p></div>"#,
        );
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].password.as_deref(), Some("abcd"));
        assert_eq!(r[1].password.as_deref(), Some("efgh"));
        assert!(link("https://evil.test/?u=https://pan.baidu.com/s/a", "").is_none());
        assert!(detail("bad").is_empty());
        assert!(list("bad", &Endpoints::default().base).is_empty());
    }
}
