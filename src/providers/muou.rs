use super::markup::{get, matches, sel, text};
use crate::{
    core::{CloudType, Link, ProviderError, SearchResult},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::Html;
use std::{collections::HashSet, sync::LazyLock};
use url::Url;
#[derive(Clone, Debug)]
pub struct Muou {
    hosts: Vec<Url>,
}
impl Default for Muou {
    fn default() -> Self {
        Self::new(
            [
                "https://www.muoua.top/",
                "http://www.muoua.top/",
                "https://333.333291.xyz/",
                "http://333.333291.xyz/",
                "https://666.666291.xyz/",
                "http://666.666291.xyz/",
            ]
            .into_iter()
            .map(|s| Url::parse(s).unwrap())
            .collect(),
        )
    }
}
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/vod/detail/id/(\d+)\.html").unwrap());
static LINKS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"https?://pan\.quark\.cn/s/[0-9a-zA-Z]+",
        r#"https?://drive\.uc\.cn/s/[0-9a-zA-Z]+(\?[^"'\s]*)?"#,
        r"https?://pan\.baidu\.com/s/[0-9a-zA-Z_\-]+(\?pwd=[0-9a-zA-Z]+)?",
        r"https?://(www\.)?(aliyundrive\.com|alipan\.com)/s/[0-9a-zA-Z]+",
        r"https?://pan\.xunlei\.com/s/[0-9a-zA-Z_\-]+(\?pwd=[0-9a-zA-Z]+)?",
        r"https?://cloud\.189\.cn/t/[0-9a-zA-Z]+",
        r"https?://115\.com/s/[0-9a-zA-Z]+",
        r"https?://caiyun\.feixin\.10086\.cn/[0-9a-zA-Z]+",
        r"https?://share\.weiyun\.com/[0-9a-zA-Z]+",
        r"https?://(www\.)?(lanzou[uixys]*|lan[zs]o[ux])\.(com|net|org)/[0-9a-zA-Z]+",
        r"https?://(www\.)?jianguoyun\.com/p/[0-9a-zA-Z]+",
        r"https?://123pan\.com/s/[0-9a-zA-Z]+",
        r"https?://mypikpak\.com/s/[0-9a-zA-Z]+",
        r"magnet:\?xt=urn:btih:[0-9a-fA-F]{40}",
        r"ed2k://\|file\|.+\|\d+\|[0-9a-fA-F]{32}\|/",
    ]
    .into_iter()
    .map(|s| Regex::new(s).unwrap())
    .collect()
});
fn items(body: &str) -> Result<Vec<(String, SearchResult)>, ProviderError> {
    let doc = Html::parse_document(body);
    if doc
        .select(&sel(
            ".module-search-item,.module-search,.module-items,.module-main",
        ))
        .next()
        .is_none()
    {
        return Err(ProviderError::Parse("muou search structure changed".into()));
    }
    let mut out = Vec::new();
    for row in doc.select(&sel(".module-search-item")) {
        let Some(a) = row.select(&sel(".video-info-header h3 a")).next() else {
            continue;
        };
        let Some(id) = a
            .value()
            .attr("href")
            .and_then(|s| ID.captures(s))
            .map(|c| c[1].to_owned())
        else {
            continue;
        };
        let mut r = SearchResult::provider(format!("muou-{id}"), "muou");
        r.title = text(a);
        r.tags = row
            .select(&sel(".video-info-aux .tag-link a"))
            .map(text)
            .filter(|s| !s.is_empty())
            .collect();
        let mut parts = Vec::new();
        if let Some(q) = row
            .select(&sel(".video-serial"))
            .next()
            .map(text)
            .filter(|s| !s.is_empty())
        {
            parts.push(format!("【{q}】"));
        }
        let mut director = String::new();
        let mut actors = Vec::new();
        let mut plot = String::new();
        for meta in row.select(&sel(".video-info-items")) {
            let label = meta
                .select(&sel(".video-info-itemtitle"))
                .next()
                .map(text)
                .unwrap_or_default();
            if label.contains("导演") {
                director = meta
                    .select(&sel(".video-info-actor a"))
                    .map(text)
                    .collect::<String>();
            }
            if label.contains("主演") {
                actors = meta
                    .select(&sel(".video-info-actor a"))
                    .map(text)
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            if label.contains("剧情") {
                plot = meta
                    .select(&sel(".video-info-item"))
                    .map(text)
                    .collect::<String>();
            }
        }
        if !director.is_empty() {
            parts.push(format!("导演：{director}"));
        }
        if !actors.is_empty() {
            parts.push(format!(
                "主演：{}{}",
                actors
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("、"),
                if actors.len() > 3 { "等" } else { "" }
            ));
        }
        if !plot.is_empty() {
            parts.push(plot);
        }
        r.content = parts.join("\n");
        if let Some(pic) = row
            .select(&sel(".module-item-pic > img"))
            .next()
            .and_then(|n| n.value().attr("data-src"))
            .filter(|s| !s.is_empty())
        {
            r.images.push(pic.into());
        }
        out.push((id, r));
    }
    Ok(out)
}
fn detail(body: &str, base: &Url, mut r: SearchResult) -> Result<SearchResult, ProviderError> {
    let doc = Html::parse_document(body);
    if let Some(pic) = doc
        .select(&sel(".mobile-play .lazyload"))
        .next()
        .and_then(|n| n.value().attr("data-src"))
        .filter(|s| !s.is_empty())
        .and_then(|s| base.join(s).ok())
    {
        r.images = vec![pic.to_string()];
    }
    let mut seen = HashSet::new();
    let anchors = sel("a[href]");
    for row in doc.select(&sel("#download-list .module-row-one")) {
        let raws = row
            .select(&sel("[data-clipboard-text]"))
            .next()
            .and_then(|n| n.value().attr("data-clipboard-text"))
            .into_iter()
            .chain(row.select(&anchors).filter_map(|a| a.value().attr("href")));
        for raw in raws {
            if raw.is_empty()
                || raw.contains(['#'])
                || raw.contains("javascript:")
                || !LINKS.iter().any(|re| re.is_match(raw))
                || !seen.insert(raw.to_owned())
            {
                continue;
            }
            let mut l = Link::new(raw);
            if raw.contains("caiyun.feixin.10086.cn") {
                l.cloud_type = CloudType::Mobile;
            }
            l.work_title = Some(r.title.clone());
            r.links.push(l);
        }
    }
    if r.links.is_empty() {
        return Err(ProviderError::Parse(
            "muou download section contains no supported shares".into(),
        ));
    }
    Ok(r)
}
impl Muou {
    pub fn new(hosts: Vec<Url>) -> Self {
        Self { hosts }
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut searches = stream::iter(self.hosts.iter().cloned().map(|base| async move {
            let encoded = {
                let mut serializer = url::form_urlencoded::Serializer::new(String::new());
                serializer.append_pair("q", query);
                serializer.finish()
            };
            let url = base
                .join(&format!("index.php/vod/search/wd/{}.html", &encoded[2..]))
                .unwrap();
            let body = get(ctx, url, base.as_str(), 8, 3).await?;
            Ok::<_, ProviderError>((base, items(&body)?))
        }))
        .buffer_unordered(6);
        let mut chosen = None;
        let mut empty = false;
        let mut error = None;
        while let Some(r) = searches.next().await {
            match r {
                Ok((base, items)) if !items.is_empty() => {
                    chosen = Some((base, items));
                    break;
                }
                Ok(_) => empty = true,
                Err(e) => error = Some(e),
            }
        }
        drop(searches);
        let Some((base, items)) = chosen else {
            return if empty {
                Ok(vec![])
            } else {
                Err(error.unwrap_or_else(|| {
                    ProviderError::Unavailable("muou no configured hosts".into())
                }))
            };
        };
        let mut jobs = stream::iter(items.into_iter().map(|(id, r)| {
            let base = base.clone();
            async move {
                let url = base
                    .join(&format!("index.php/vod/detail/id/{id}.html"))
                    .unwrap();
                let r = detail(&get(ctx, url, base.as_str(), 6, 3).await?, &base, r)?;
                if matches(&format!("{} {}", r.title, r.content), query) {
                    ctx.publish(vec![r.clone()]);
                    Ok(Some(r))
                } else {
                    Ok(None)
                }
            }
        }))
        .buffer_unordered(20);
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
impl Provider for Muou {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("muou", 2, KeywordFilterMode::Core)
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
    async fn mirrors_and_full_detail() {
        use std::time::Duration;
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        let bad = MockServer::start().await;
        let good = MockServer::start().await;
        Mock::given(path("/index.php/vod/search/wd/Film.html"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&bad)
            .await;
        Mock::given(path("/index.php/vod/search/wd/Film.html")).respond_with(ResponseTemplate::new(200).set_body_string("<div class='module-search-item'><div class='video-info-header'><h3><a href='/index.php/vod/detail/id/1.html'>Film</a></h3></div></div>")).expect(1).mount(&good).await;
        Mock::given(path("/index.php/vod/detail/id/1.html")).respond_with(ResponseTemplate::new(200).set_body_string("<div class='mobile-play'><img class='lazyload' data-src='/poster.jpg'></div><div id='download-list'><div class='module-row-one'><span data-clipboard-text='https://pan.quark.cn/s/abc'></span><a href='https://pan.quark.cn/s/abc'>same</a></div></div>")).expect(1).mount(&good).await;
        let p = Muou::new(vec![
            Url::parse(&bad.uri()).unwrap(),
            Url::parse(&good.uri()).unwrap(),
        ]);
        let c = SearchContext::new(reqwest::Client::new(), Duration::from_secs(4));
        let r = p.search(&c, "Film").await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].links.len(), 1);
        assert_eq!(r[0].images, [format!("{}/poster.jpg", good.uri())]);
        assert!(
            items("<div class='module-search'></div>")
                .unwrap()
                .is_empty()
        );
        assert!(items("changed").is_err());
    }
}
