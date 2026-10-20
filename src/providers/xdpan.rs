use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use async_trait::async_trait;
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use std::{sync::LazyLock, time::Duration};
use url::Url;
static TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"时间:\s*(\d{4}-\d{1,2}-\d{1,2})").unwrap());
static FORMAT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"格式:\s*([^\s]+)").unwrap());
static OPEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"window\.open\(\s*[\"']([^\"']*pan\.baidu\.com[^\"']*)[\"']"#).unwrap()
});
static LOCATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:window\.)?location\.href\s*=\s*[\"']([^\"']*pan\.baidu\.com[^\"']*)[\"']"#)
        .unwrap()
});
static DETAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"详情:\s*(https?://[^\s]+)").unwrap());
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://aipanso.com/").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Xdpan {
    endpoints: Endpoints,
}
impl Xdpan {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn text(e: ElementRef<'_>) -> String {
    e.text().collect::<String>().trim().into()
}
fn list(body: &str, base: &Url) -> Result<Vec<SearchResult>, ProviderError> {
    let doc = Html::parse_document(body);
    if doc
        .select(&sel("title"))
        .next()
        .is_some_and(|e| text(e) == "Just a moment...")
    {
        return Err(ProviderError::Blocked);
    }
    Ok(doc
        .select(&sel("van-row"))
        .filter_map(|e| {
            let href = e
                .select(&sel("a[href^='/s/']"))
                .next()?
                .value()
                .attr("href")?;
            let id = href.split('/').nth(2)?;
            let u = base.join(href).ok()?;
            let spans = e
                .select(&sel("div[name='content-title'] span"))
                .map(text)
                .collect::<String>();
            let title = if spans.is_empty() {
                e.select(&sel("div[name='content-title']"))
                    .next()
                    .map(text)?
            } else {
                spans
            };
            if title.is_empty() {
                return None;
            }
            let bottom = text(e);
            let when = TIME
                .captures(&bottom)
                .map(|c| c[1].to_owned())
                .unwrap_or_default();
            let format = e
                .select(&sel("template b"))
                .next()
                .map(text)
                .filter(|s| !s.is_empty())
                .or_else(|| FORMAT.captures(&bottom).map(|c| c[1].into()))
                .unwrap_or_default();
            let mut row = SearchResult::provider(format!("xdpan-{id}"), "xdpan");
            row.title = title;
            row.datetime = date(&when).or_else(|| Some(chrono::Utc::now()));
            row.content = format!("类型: {format} | 分享时间: {when} | 详情: {u}");
            Some(row)
        })
        .collect())
}
fn links(body: &str) -> Vec<Link> {
    let doc = Html::parse_document(body);
    let code = doc
        .select(&sel("van-cell[title='密码'] b"))
        .next()
        .map(text)
        .unwrap_or_default();
    doc.select(&sel("script"))
        .filter_map(|e| {
            let script = e.text().collect::<String>();
            let raw = [&*OPEN, &*LOCATION]
                .into_iter()
                .find_map(|re| re.captures(&script).map(|c| c[1].to_owned()))?;
            let mut link = Link::new(&raw);
            if link.cloud_type != CloudType::Baidu {
                return None;
            }
            link.password = password(&raw).or_else(|| (!code.is_empty()).then(|| code.clone()));
            if password(&raw).is_none() && !code.is_empty() {
                let mut u = Url::parse(&raw).ok()?;
                u.query_pairs_mut().append_pair("pwd", &code);
                link.url = u.to_string()
            }
            Some(link)
        })
        .collect()
}
impl Xdpan {
    async fn fetch(&self, ctx: &SearchContext, url: Url) -> Result<Vec<u8>, ProviderError> {
        let mut last = ProviderError::Unavailable("xdpan request failed".into());
        for attempt in 0..3 {
            if ctx.is_cancelled() {
                return Err(ProviderError::Timeout);
            }
            let result = async {
                let r = crate::http::execute(
                    ctx.client
                        .get(url.clone())
                        .timeout(ctx.request_timeout(Duration::from_secs(30)))
                        .header("referer", self.endpoints.base.as_str())
                        .header(
                            "user-agent",
                            "Mozilla/5.0 Chrome/91.0.4472.124 Safari/537.36",
                        ),
                )
                .await
                .map_err(network)?;
                if r.headers()
                    .get("cf-mitigated")
                    .is_some_and(|s| s == "challenge")
                {
                    return Err(ProviderError::Blocked);
                }
                if r.status() != 200 {
                    return Err(ProviderError::Protocol(format!(
                        "HTTP {}",
                        r.status().as_u16()
                    )));
                }
                Ok(r)
            }
            .await;
            match result {
                Ok(r) => {
                    return crate::http::read_response(r, 3 * 1024 * 1024)
                        .await
                        .map_err(|e| ProviderError::Network(e.to_string()));
                }
                Err(ProviderError::Blocked) => return Err(ProviderError::Blocked),
                Err(e) => last = e,
            }
            if attempt < 2 {
                tokio::time::sleep(Duration::from_millis(500 * (1 << attempt))).await
            }
        }
        Err(last)
    }
}
#[async_trait]
impl Provider for Xdpan {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("xdpan", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut url = self.endpoints.base.join("/search").unwrap();
        url.query_pairs_mut()
            .extend_pairs([("page", "1"), ("k", query), ("p", "baidu")]);
        let body = self.fetch(ctx, url).await?;
        let rows = list(&String::from_utf8_lossy(&body), &self.endpoints.base)?;
        collect_details(
            ctx,
            rows.into_iter()
                .enumerate()
                .map(|(index, mut row)| async move {
                    tokio::time::sleep(Duration::from_millis((index % 3) as u64 * 200)).await;
                    if ctx.is_cancelled() {
                        return Err(ProviderError::Timeout);
                    }
                    let url = DETAIL
                        .captures(&row.content)
                        .and_then(|c| Url::parse(&c[1]).ok())
                        .ok_or_else(|| ProviderError::Parse("xdpan detail URL missing".into()))?;
                    let body = self.fetch(ctx, url).await?;
                    row.links = links(&String::from_utf8_lossy(&body));
                    for link in &mut row.links {
                        link.work_title = Some(row.title.clone())
                    }
                    Ok(row)
                }),
            10,
        )
        .await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scripts_and_metadata() {
        for script in [
            "window.open('https://pan.baidu.com/s/a')",
            "location.href = 'https://pan.baidu.com/s/a'",
            "window.location.href='https://pan.baidu.com/s/a'",
        ] {
            let links = links(&format!(
                "<van-cell title=密码><b>abcd</b></van-cell><script>{script}</script>"
            ));
            assert_eq!(links[0].password.as_deref(), Some("abcd"));
            assert!(links[0].url.ends_with("?pwd=abcd"));
        }
        let rows=list("<van-row><a href='/s/1'></a><div name='content-title'><span>movie</span></div><div>时间: 2026-1-2 格式: mkv</div></van-row>",&Endpoints::default().base).unwrap();
        assert_eq!(rows[0].title, "movie");
        assert!(rows[0].content.contains("mkv"));
        assert!(list("bad", &Endpoints::default().base).unwrap().is_empty());
        assert!(links("bad").is_empty());
    }
}
