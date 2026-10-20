use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{Link, ProviderError, SearchResult};
use async_trait::async_trait;
use regex::Regex;
use scraper::{Html, Selector};
use std::{collections::HashSet, sync::LazyLock, time::Duration};
use url::Url;
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/(\d+)\.html").unwrap());
static BAIDU: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://pan\.baidu\.com/s/[0-9a-zA-Z_\-]{10,}(?:\?pwd=[0-9a-zA-Z]+)?").unwrap()
});
static PWD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"提取码[:：]\s*([a-zA-Z0-9]{4})").unwrap());
static CLEAN: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"\s+",
        r"百度云网盘资源下载地址[:：]?\s*",
        r"链接[:：]?\s*https?://pan\.baidu\.com/[^\s]+",
        r"提取码[:：]?\s*[a-zA-Z0-9]{4}",
    ]
    .into_iter()
    .map(|s| Regex::new(s).unwrap())
    .collect()
});
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://www.xinjuc.com/").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Xinjuc {
    endpoints: Endpoints,
}
impl Xinjuc {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn list(body: &str, base: &Url) -> Vec<(Url, SearchResult)> {
    let d = Html::parse_document(body);
    d.select(&sel("div.row-xs.post-list article.post-item"))
        .filter_map(|e| {
            let u = base
                .join(
                    e.select(&sel("div.post-image a"))
                        .next()?
                        .value()
                        .attr("href")?,
                )
                .ok()?;
            let id = ID.captures(u.path())?[1].to_owned();
            let mut r = SearchResult::provider(format!("xinjuc-{id}"), "xinjuc");
            r.title = e
                .select(&sel("h5.post-title a"))
                .next()
                .map(|e| e.text().collect::<String>().trim().into())
                .unwrap_or_default();
            r.tags = e
                .select(&sel("div.mark span"))
                .map(|e| e.text().collect::<String>().trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect();
            let when = e
                .select(&sel("div.post-footer span.time"))
                .next()
                .map(|e| e.text().collect::<String>().replace(" 更新", ""))
                .unwrap_or_default();
            r.datetime = date(when.trim())
                .or_else(|| {
                    date(&format!(
                        "{}-{}",
                        chrono::Utc::now().format("%Y"),
                        when.trim()
                    ))
                })
                .or_else(|| Some(chrono::Utc::now()));
            Some((u, r))
        })
        .collect()
}
fn detail(body: &str) -> (Vec<Link>, String) {
    let d = Html::parse_document(body);
    let Some(article) = d.select(&sel("div.article-content")).next() else {
        return (vec![], String::new());
    };
    let code = PWD.captures(body).map(|c| c[1].to_owned());
    let mut seen = HashSet::new();
    let mut links = vec![];
    let raw_regex = BAIDU.find_iter(body).map(|m| m.as_str());
    let anchors = sel("a[href]");
    for raw in raw_regex.chain(d.select(&anchors).filter_map(|a| a.value().attr("href"))) {
        let raw = raw.trim();
        let Ok(u) = Url::parse(raw) else { continue };
        if u.host_str() != Some("pan.baidu.com")
            || !BAIDU.is_match(raw)
            || !seen.insert(raw.to_owned())
        {
            continue;
        }
        let mut l = Link::new(raw);
        l.password = password(raw).or_else(|| code.clone());
        links.push(l)
    }
    let mut content = article.text().collect::<String>();
    for (i, re) in CLEAN.iter().enumerate() {
        content = re
            .replace_all(&content, if i == 0 { " " } else { "" })
            .into_owned()
    }
    content = content.trim().into();
    if content.chars().count() > 300 {
        content = content.chars().take(300).collect::<String>() + "..."
    }
    (links, content)
}
impl Xinjuc {
    async fn fetch(
        &self,
        ctx: &SearchContext,
        url: Url,
        detail: bool,
    ) -> Result<Vec<u8>, ProviderError> {
        tokio::time::timeout(
            ctx.request_timeout(Duration::from_secs(if detail { 8 } else { 10 })),
            request(
                || {
                    ctx.client
                        .get(url.clone())
                        .header("referer", self.endpoints.base.as_str())
                        .header(
                            "user-agent",
                            "Mozilla/5.0 Chrome/91.0.4472.124 Safari/537.36",
                        )
                },
                if detail { 1 } else { 3 },
                Duration::from_millis(200),
                BODY_LIMIT,
            ),
        )
        .await
        .map_err(|_| ProviderError::Timeout)?
    }
}
#[async_trait]
impl Provider for Xinjuc {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("xinjuc", 2, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut url = self.endpoints.base.clone();
        url.query_pairs_mut().append_pair("s", query);
        let body = self.fetch(ctx, url, false).await?;
        let rows = list(&String::from_utf8_lossy(&body), &self.endpoints.base);
        collect_details(
            ctx,
            rows.into_iter().map(|(url, mut row)| async move {
                let body = self.fetch(ctx, url, true).await?;
                let (links, content) = detail(&String::from_utf8_lossy(&body));
                row.links = links;
                row.content = content;
                Ok(row)
            }),
            15,
        )
        .await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn baidu_specific_and_password_priority() {
        let h = r#"<div class="article-content">description 提取码：text</div><a href="https://pan.baidu.com/s/1234567890?pwd=url1">open</a><a href="https://pan.baidu.com/s/short">invalid</a><a href="https://pan.quark.cn/s/abc">other</a>"#;
        let (l, c) = detail(h);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].password.as_deref(), Some("url1"));
        assert_eq!(c, "description");
        assert!(detail("bad").0.is_empty());
        assert!(list("bad", &Endpoints::default().base).is_empty());
    }
}
