use super::markup::{get, sel, text};
use crate::{
    core::{CloudType, Link, ProviderError, SearchResult},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use chrono::NaiveDateTime;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::Html;
use std::sync::LazyLock;
use url::Url;
#[derive(Clone, Debug)]
pub struct Qingying {
    base: Url,
}
impl Default for Qingying {
    fn default() -> Self {
        Self::new(Url::parse("http://revohd.com/").unwrap())
    }
}
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/voddetail/(\d+)\.html").unwrap());
static TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{4}-\d{2}-\d{2}\s+\d{2}:\d{2}:\d{2})").unwrap());
static PASSWORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"pwd=([a-zA-Z0-9]{4})").unwrap());
fn items(body: &str, base: &Url, query: &str) -> Result<Vec<(Url, SearchResult)>, ProviderError> {
    let doc = Html::parse_document(body);
    if doc
        .select(&sel(
            ".module-search-item,.module-search,.module-items,.module-list,.module-main",
        ))
        .next()
        .is_none()
    {
        return Err(ProviderError::Parse(
            "qingying search document changed".into(),
        ));
    }
    let mut out = Vec::new();
    for row in doc.select(&sel("div.module-search-item")) {
        let Some(a) = row
            .select(&sel(".video-info .video-info-header h3 a"))
            .next()
        else {
            continue;
        };
        let Some(href) = a.value().attr("href") else {
            continue;
        };
        let Some(id) = ID.captures(href).map(|c| c[1].to_owned()) else {
            continue;
        };
        let mut title = text(a);
        if title.is_empty() {
            title = a.value().attr("title").unwrap_or("").trim().into();
        }
        if !title.to_lowercase().contains(&query.to_lowercase()) {
            continue;
        }
        let Some(url) = base.join(href).ok() else {
            continue;
        };
        let mut r = SearchResult::provider(format!("qingying-{id}"), "qingying");
        r.title = title;
        out.push((url, r));
        if out.len() == 10 {
            break;
        }
    }
    Ok(out)
}
fn detail(body: &str, mut r: SearchResult) -> Result<SearchResult, ProviderError> {
    let doc = Html::parse_document(body);
    if let Some(title) = doc
        .select(&sel(".video-info .video-info-header h1.page-title a"))
        .next()
        .map(text)
        .filter(|s| !s.is_empty())
    {
        r.title = title;
    }
    for row in doc.select(&sel(".video-info-items")) {
        let label = row
            .select(&sel(".video-info-itemtitle"))
            .next()
            .map(text)
            .unwrap_or_default();
        if label.contains("更新") {
            let s = row
                .select(&sel(".video-info-item"))
                .next()
                .map(text)
                .unwrap_or_default();
            r.datetime = TIME
                .captures(&s)
                .and_then(|c| NaiveDateTime::parse_from_str(c[1].trim(), "%Y-%m-%d %H:%M:%S").ok())
                .map(|t| t.and_utc());
        }
        if label.contains("剧情") {
            r.content = row
                .select(&sel(".video-info-item.video-info-content span"))
                .next()
                .or_else(|| row.select(&sel(".video-info-item")).next())
                .map(text)
                .unwrap_or_default();
        }
    }
    let labelled = doc
        .select(&sel(".module-heading h2.module-title"))
        .map(text)
        .any(|s| s.contains("123") && s.contains("云盘"));
    if !labelled {
        return Err(ProviderError::Parse(
            "qingying missing 123 cloud section".into(),
        ));
    }
    for row in doc.select(&sel(".module-downlist .module-row-text")) {
        let Some(raw) = row.value().attr("data-clipboard-text").map(str::trim) else {
            continue;
        };
        let Ok(url) = Url::parse(raw) else {
            continue;
        };
        if !matches!(url.scheme(), "http" | "https")
            || !url.host_str().is_some_and(|h| {
                [
                    "123684.com",
                    "123685.com",
                    "123912.com",
                    "123pan.com",
                    "123pan.cn",
                    "123592.com",
                ]
                .iter()
                .any(|d| h == *d || h.ends_with(&format!(".{d}")))
            })
        {
            continue;
        }
        let mut l = Link::new(raw);
        l.cloud_type = CloudType::Pan123;
        l.password = url
            .query_pairs()
            .find(|(k, v)| k == "pwd" && v.len() == 4)
            .map(|(_, v)| v.into_owned())
            .or_else(|| PASSWORD.captures(raw).map(|c| c[1].to_owned()));
        l.work_title = Some(r.title.clone());
        r.links.push(l);
        return Ok(r);
    }
    Err(ProviderError::Parse(
        "qingying missing 123 share URL".into(),
    ))
}
impl Qingying {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self.base.join("vodsearch/-------------.html").unwrap();
        url.query_pairs_mut().append_pair("wd", query);
        let items = items(
            &get(ctx, url, self.base.as_str(), 30, 3).await?,
            &self.base,
            query,
        )?;
        let mut jobs = stream::iter(items.into_iter().map(|(url, r)| async move {
            let r = detail(&get(ctx, url, self.base.as_str(), 30, 3).await?, r)?;
            ctx.publish(vec![r.clone()]);
            Ok(r)
        }))
        .buffer_unordered(3);
        let mut out = Vec::new();
        let mut error = None;
        while let Some(r) = jobs.next().await {
            match r {
                Ok(r) => out.push(r),
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
impl Provider for Qingying {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("qingying", 3, KeywordFilterMode::Provider)
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
    async fn full_chain_and_schema() {
        use std::time::Duration;
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{path, query_param},
        };
        let s = MockServer::start().await;
        Mock::given(path("/vodsearch/-------------.html")).and(query_param("wd","Film")).respond_with(ResponseTemplate::new(200).set_body_string("<div class='module-search-item'><div class='video-info'><div class='video-info-header'><h3><a href='/voddetail/1.html'>Film</a></h3></div></div></div>")).expect(1).mount(&s).await;
        Mock::given(path("/voddetail/1.html")).respond_with(ResponseTemplate::new(200).set_body_string("<div class='video-info-items'><div class='video-info-itemtitle'>更新</div><div class='video-info-item'>2026-10-12 12:34:56</div></div><div class='module-heading'><h2 class='module-title'>123云盘</h2></div><div class='module-downlist'><div class='module-row-text' data-clipboard-text='https://www.123684.com/s/abc?pwd=abcd'></div></div>")).expect(1).mount(&s).await;
        let p = Qingying::new(Url::parse(&s.uri()).unwrap());
        let c = SearchContext::new(reqwest::Client::new(), Duration::from_secs(3));
        let r = p.search(&c, "Film").await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].links[0].password.as_deref(), Some("abcd"));
        assert!(r[0].datetime.is_some());
        assert!(
            items("<div class='module-search'></div>", &p.base, "Film")
                .unwrap()
                .is_empty()
        );
        assert!(items("changed", &p.base, "Film").is_err());
    }
}
