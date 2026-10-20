use super::markup::{get, matches, sel, text};
use crate::{
    core::{CloudType, Link, ProviderError, SearchResult},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use regex::Regex;
use scraper::Html;
use std::{collections::HashSet, sync::LazyLock};
use url::Url;
#[derive(Clone, Debug)]
pub struct Pianku {
    base: Url,
}
impl Default for Pianku {
    fn default() -> Self {
        Self::new(Url::parse("https://btnull.pro/").unwrap())
    }
}
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/movie/(\d+)\.html").unwrap());
static REGION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"地区：([^　]*?)　+类型：(.*)").unwrap());
static PASSWORDS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"[?&]pwd=([0-9a-zA-Z]+)",
        r"[?&]password=([0-9a-zA-Z]+)",
        r"提取码[：:]\s*([0-9a-zA-Z]+)",
        r"访问码[：:]\s*([0-9a-zA-Z]+)",
        r"密码[：:]\s*([0-9a-zA-Z]+)",
        r"验证码[：:]\s*([0-9a-zA-Z]+)",
        r"口令[：:]\s*([0-9a-zA-Z]+)",
        r"（访问码[：:]\s*([0-9a-zA-Z]+)）",
    ]
    .into_iter()
    .map(|s| Regex::new(s).unwrap())
    .collect()
});
static LINKS: LazyLock<Vec<(CloudType, Regex)>> = LazyLock::new(|| {
    use CloudType::*;
    [
        (Magnet, r#"magnet:\?xt=urn:btih:[0-9a-fA-F]{40}[^"'\s]*"#),
        (Ed2k, r"ed2k://\|file\|[^|]+\|[^|]+\|[^|]+\|/?"),
        (
            Baidu,
            r"https?://pan\.baidu\.com/s/[0-9a-zA-Z_-]+(?:\?pwd=[0-9a-zA-Z]+)?(?:&v=\d+)?",
        ),
        (Aliyun, r"https?://(?:www\.)?alipan\.com/s/[0-9a-zA-Z_-]+"),
        (
            Tianyi,
            r"https?://cloud\.189\.cn/t/[0-9a-zA-Z_-]+(?:\([^)]*\))?",
        ),
        (Uc, r#"https?://drive\.uc\.cn/s/[0-9a-fA-F]+(?:\?[^"\s]*)?"#),
        (Mobile, r#"https?://caiyun\.139\.com/[^"\s]+"#),
        (
            One15,
            r#"https?://(?:115\.com|115cdn\.com)/s/[0-9a-zA-Z_-]+(?:\?[^"\s]*)?"#,
        ),
        (Pikpak, r"https?://mypikpak\.com/s/[0-9a-zA-Z_-]+"),
        (
            Xunlei,
            r"https?://pan\.xunlei\.com/s/[0-9a-zA-Z_-]+(?:\?pwd=[0-9a-zA-Z]+)?",
        ),
        (
            Pan123,
            r#"https?://(?:www\.)?(?:123pan\.com|123684\.com)/s/[0-9a-zA-Z_-]+(?:\?[^"\s]*)?"#,
        ),
        (
            Quark,
            r"https?://pan\.quark\.cn/s/[0-9a-fA-F]+(?:\?pwd=[0-9a-zA-Z]+)?",
        ),
    ]
    .into_iter()
    .map(|(t, p)| (t, Regex::new(p).unwrap()))
    .collect()
});
fn pass(s: &str) -> Option<String> {
    PASSWORDS
        .iter()
        .find_map(|r| r.captures(s).map(|c| c[1].to_owned()))
}
fn search_items(body: &str, base: &Url) -> Result<Vec<(Url, SearchResult)>, ProviderError> {
    let doc = Html::parse_document(body);
    if doc.select(&sel(".sr_lists")).next().is_none() {
        return Err(ProviderError::Parse("pianku search list missing".into()));
    }
    let mut out = Vec::new();
    for dl in doc.select(&sel(".sr_lists dl")) {
        let Some(href) = dl
            .select(&sel("dt a"))
            .next()
            .and_then(|a| a.value().attr("href"))
        else {
            continue;
        };
        let Some(id) = ID.captures(href).map(|c| c[1].to_owned()) else {
            continue;
        };
        let Some(url) = base.join(href).ok() else {
            continue;
        };
        let title = dl
            .select(&sel("dd p:first-child strong a"))
            .next()
            .map(text)
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let mut result = SearchResult::provider(format!("pianku-{id}"), "pianku");
        result.title = title;
        let mut actors = String::new();
        let mut alias = String::new();
        let mut description = String::new();
        for p in dl.select(&sel("dd p")) {
            let s = text(p);
            if let Some(v) = s.strip_prefix("又名：") {
                alias = v.into();
            } else if let Some(c) = REGION.captures(&s) {
                if !c[1].trim().is_empty() {
                    result.tags.push(c[1].trim().into());
                }
                result.tags.extend(
                    c[2].split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned),
                );
            } else if let Some(v) = s.strip_prefix("主演：") {
                actors = v.into();
            } else if let Some(v) = s.strip_prefix("简介：") {
                description = v.into();
            } else if description.is_empty()
                && s.len() > 10
                && !s.contains("名称：")
                && !s.contains("地区：")
            {
                description = s;
            }
        }
        let mut parts = Vec::new();
        if !alias.is_empty() {
            parts.push(format!("又名：{alias}"));
        }
        if !actors.is_empty() {
            parts.push(format!("主演：{actors}"));
        }
        if !description.is_empty() {
            parts.push(description);
        }
        result.content = parts.join("\n");
        if let Some(status) = dl
            .select(&sel("dd p:first-child span.ss1"))
            .next()
            .map(text)
            .filter(|s| !s.is_empty())
        {
            result.tags.push(status);
        }
        out.push((url, result));
    }
    Ok(out)
}
fn detail_links(body: &str) -> Vec<Link> {
    let doc = Html::parse_document(body);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for row in doc.select(&sel("#donLink .down-list2")) {
        let Some(a) = row.select(&sel(".down-list3 a")).next() else {
            continue;
        };
        let Some(raw) = a.value().attr("href") else {
            continue;
        };
        let label = text(a);
        if label.is_empty() || !seen.insert(raw.to_owned()) {
            continue;
        }
        let Some((kind, _)) = LINKS.iter().find(|(_, r)| r.is_match(raw)) else {
            continue;
        };
        let mut link = Link::new(raw);
        link.cloud_type = *kind;
        link.password = pass(raw).or_else(|| pass(&label));
        out.push(link);
    }
    out
}
impl Pianku {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self.base.join("search/-------------.html").unwrap();
        url.query_pairs_mut().append_pair("wd", query);
        let items = search_items(&get(ctx, url, self.base.as_str(), 30, 3).await?, &self.base)?;
        let mut out = Vec::new();
        let mut error = None;
        for (url, mut r) in items {
            match get(ctx, url, self.base.as_str(), 30, 3).await {
                Ok(body) => {
                    r.links = detail_links(&body);
                    if r.links.is_empty() || !matches(&format!("{} {}", r.title, r.content), query)
                    {
                        continue;
                    }
                    for l in &mut r.links {
                        l.work_title = Some(r.title.clone());
                    }
                    ctx.publish(vec![r.clone()]);
                    out.push(r);
                }
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
impl Provider for Pianku {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("pianku", 3, KeywordFilterMode::Provider)
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
        Mock::given(path("/search/-------------.html")).and(query_param("wd","Film")).respond_with(ResponseTemplate::new(200).set_body_string("<div class='sr_lists'><dl><dt><a href='/movie/7.html'></a></dt><dd><p><strong><a>Film</a></strong><span class='ss1'>HD</span></p><p>又名：Alias</p><p>主演：Actor</p></dd></dl></div>")).expect(1).mount(&s).await;
        Mock::given(path("/movie/7.html")).respond_with(ResponseTemplate::new(200).set_body_string("<div id='donLink'><div class='down-list2'><div class='down-list3'><a href='magnet:?xt=urn:btih:0123456789012345678901234567890123456789'>Torrent</a></div></div><div class='down-list2'><div class='down-list3'><a href='https://pan.baidu.com/s/abc?pwd=1234'>Cloud</a></div></div></div>")).expect(1).mount(&s).await;
        let p = Pianku::new(Url::parse(&s.uri()).unwrap());
        let c = SearchContext::new(reqwest::Client::new(), Duration::from_secs(4));
        let r = p.search(&c, "Film").await.unwrap();
        assert_eq!(r[0].links.len(), 2);
        assert_eq!(r[0].links[1].password.as_deref(), Some("1234"));
        assert!(r[0].content.contains("Alias"));
        assert!(
            search_items("<div class='sr_lists'></div>", &p.base)
                .unwrap()
                .is_empty()
        );
        assert!(search_items("bad", &p.base).is_err());
    }
}
