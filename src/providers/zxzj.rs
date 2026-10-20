use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use std::{sync::LazyLock, time::Duration};
use url::Url;
static ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/(?:voddetail|detail)/(\d+)\.html").unwrap());
static PLAYER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"var\s+player_aaaa\s*=\s*(\{[^;]+\})").unwrap());
static PWD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"pwd=([a-zA-Z0-9]{4})").unwrap());
static TIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"更新[：:]\s*(\d{4}-\d{2}-\d{2}\s+\d{2}:\d{2}:\d{2}|\d{4}-\d{2}-\d{2})").unwrap()
});
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://www.zxzjys.com/").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Zxzj {
    endpoints: Endpoints,
}
impl Zxzj {
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
fn list(body: &str, base: &Url) -> Vec<(Url, SearchResult)> {
    let d = Html::parse_document(body);
    d.select(&sel("ul.stui-vodlist li"))
        .filter_map(|e| {
            let a = e.select(&sel(".stui-vodlist__detail h4.title a")).next()?;
            let title = text(a);
            if title.is_empty() {
                return None;
            }
            let u = base.join(a.value().attr("href")?).ok()?;
            let id = ID.captures(u.path())?[1].to_owned();
            let mut row = SearchResult::provider(format!("zxzj-{id}"), "zxzj");
            row.title = title;
            Some((u, row))
        })
        .take(10)
        .collect()
}
fn detail(body: &str, base: &Url, row: &mut SearchResult) -> Vec<(Url, CloudType)> {
    let d = Html::parse_document(body);
    if let Some(title) = d
        .select(&sel(".stui-content__detail h1.title"))
        .next()
        .map(text)
        .filter(|s| !s.is_empty())
    {
        row.title = title
    }
    let description = d
        .select(&sel(".stui-content__detail p.data"))
        .map(text)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    row.datetime = description
        .iter()
        .filter(|s| s.contains("更新"))
        .find_map(|s| TIME.captures(s).and_then(|c| date(&c[1])))
        .or_else(|| Some(chrono::Utc::now()));
    row.content = description.join("\n");
    let mut play = vec![];
    for head in d.select(&sel(".stui-vodlist__head")) {
        let title = head.select(&sel("h3")).next().map(text).unwrap_or_default();
        let kind = if title.contains("百度") {
            CloudType::Baidu
        } else if title.contains("夸克") {
            CloudType::Quark
        } else if title.contains("迅雷") {
            CloudType::Xunlei
        } else {
            continue;
        };
        let mut next = head.next_sibling();
        while let Some(node) = next {
            if let Some(e) = ElementRef::wrap(node)
                && e.value().name() == "ul"
                && e.value().has_class(
                    "stui-content__playlist",
                    scraper::CaseSensitivity::CaseSensitive,
                )
            {
                for a in e.select(&sel("li a[href]")) {
                    if let Some(url) = a.value().attr("href").and_then(|s| base.join(s).ok()) {
                        play.push((url, kind))
                    }
                }
                break;
            }
            next = node.next_sibling();
        }
    }
    play
}
fn player(body: &str, line: CloudType) -> Option<Link> {
    #[derive(Deserialize)]
    struct Player {
        url: String,
    }
    let captures = PLAYER.captures(body)?;
    let data: Player = serde_json::from_str(&captures[1]).ok()?;
    let raw = data.url.trim().replace(r"\/", "/");
    if raw.is_empty() {
        return None;
    }
    let mut link = Link::new(&raw);
    link.password = password(&raw)
        .filter(|p| p.len() == 4)
        .or_else(|| {
            raw.split_once('|')
                .map(|(_, p)| p.trim().to_owned())
                .filter(|p| p.len() == 4)
        })
        .or_else(|| PWD.captures(&raw).map(|c| c[1].into()));
    if !matches!(
        link.cloud_type,
        CloudType::Baidu | CloudType::Quark | CloudType::Xunlei | CloudType::Aliyun
    ) {
        link.cloud_type = line
    }
    Some(link)
}
impl Zxzj {
    async fn fetch(&self, ctx: &SearchContext, url: Url) -> Result<Vec<u8>, ProviderError> {
        tokio::time::timeout(ctx.request_timeout(Duration::from_secs(30)),request(||ctx.client.get(url.clone()).header("referer",self.endpoints.base.as_str()).header("user-agent","Mozilla/5.0 Chrome/91.0.4472.124 Safari/537.36").header("accept","text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8"),3,Duration::from_millis(200),BODY_LIMIT)).await.map_err(|_|ProviderError::Timeout)?
    }
    async fn resolve(
        &self,
        ctx: &SearchContext,
        url: Url,
        mut row: SearchResult,
    ) -> Result<SearchResult, ProviderError> {
        let body = self.fetch(ctx, url).await?;
        let play = detail(
            &String::from_utf8_lossy(&body),
            &self.endpoints.base,
            &mut row,
        );
        if !row.links.is_empty() && !ctx.publish(vec![row.clone()]) {
            return Err(ProviderError::Timeout);
        }
        let mut pending = stream::iter(play.into_iter().map(|(url, kind)| async move {
            let body = self.fetch(ctx, url).await?;
            player(&String::from_utf8_lossy(&body), kind)
                .ok_or_else(|| ProviderError::Parse("zxzj player has no usable link".into()))
        }))
        .buffer_unordered(5);
        let mut first = None;
        while let Some(result) = pending.next().await {
            match result {
                Ok(link) => {
                    row.links.push(link);
                    if !ctx.publish(vec![row.clone()]) {
                        return Err(ProviderError::Timeout);
                    }
                }
                Err(error) => {
                    first.get_or_insert(error);
                }
            }
        }
        if let Some(error) = first {
            return Err(error);
        }
        Ok(row)
    }
}
#[async_trait]
impl Provider for Zxzj {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("zxzj", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut url = self
            .endpoints
            .base
            .join("/vodsearch/-------------.html")
            .unwrap();
        url.query_pairs_mut()
            .append_pair("wd", query)
            .append_pair("submit", "");
        let body = self.fetch(ctx, url).await?;
        let rows = list(&String::from_utf8_lossy(&body), &self.endpoints.base);
        collect_details(
            ctx,
            rows.into_iter()
                .map(|(url, row)| self.resolve(ctx, url, row)),
            5,
        )
        .await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn player_protocol() {
        let l=player(r#"var player_aaaa={"url":"https:\/\/pan.baidu.com\/s\/test?pwd=abcd","from":"baidu"};"#,CloudType::Quark).unwrap();
        assert_eq!(l.cloud_type, CloudType::Baidu);
        assert_eq!(l.password.as_deref(), Some("abcd"));
        assert!(player("var player_aaaa={bad};", CloudType::Baidu).is_none());
        assert!(player(r#"var player_aaaa={"url":""};"#, CloudType::Baidu).is_none());
        assert!(player("bad", CloudType::Baidu).is_none());
    }
    #[test]
    fn detail_playlist() {
        let mut row = SearchResult::provider("1", "zxzj");
        let play = detail(
            r#"<div class="stui-content__detail"><h1 class="title">movie</h1><p class="data">更新：2026-10-20</p></div><div class="stui-vodlist__head"><h3>百度网盘</h3></div><div>spacer</div><ul class="stui-content__playlist"><li><a href="/play/1.html">open</a></li></ul>"#,
            &Endpoints::default().base,
            &mut row,
        );
        assert_eq!(play.len(), 1);
        assert_eq!(play[0].0.as_str(), "https://www.zxzjys.com/play/1.html");
        assert!(row.datetime.is_some());
        assert!(list("bad", &Endpoints::default().base).is_empty());
    }
}
