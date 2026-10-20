use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::core::{Link, ProviderError, SearchResult};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use regex::Regex;
use scraper::{Html, Selector};
use serde::Deserialize;
use std::{sync::LazyLock, time::Duration};
use url::Url;
static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"const\s+DToken\s*=\s*\"([^\"]+)\""#).unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").unwrap());
static SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{4}-\d{2}-\d{2}\s+\d{2}:\d{2}:\d{2})").unwrap());
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://www.yunso.net/").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Xys {
    endpoints: Endpoints,
}
impl Xys {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
fn parse(body: &str) -> Vec<SearchResult> {
    let doc = Html::parse_fragment(body);
    let a = Selector::parse("a[onclick='open_sid(this)']").unwrap();
    let time = Selector::parse(".layui-icon-time").unwrap();
    doc.select(&Selector::parse(".layui-card[data-qid]").unwrap())
        .enumerate()
        .filter_map(|(index, card)| {
            let qid = card.value().attr("data-qid").filter(|s| !s.is_empty())?;
            let anchor = card.select(&a).next()?;
            let title = SPACE
                .replace_all(
                    TAG.replace_all(&anchor.text().collect::<String>(), "")
                        .replace('@', "")
                        .trim(),
                    " ",
                )
                .into_owned();
            if title.is_empty() {
                return None;
            }
            let raw = anchor
                .value()
                .attr("href")
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    String::from_utf8(STANDARD.decode(anchor.value().attr("url")?).ok()?).ok()
                })?;
            let mut link = Link::new(raw);
            link.password = anchor
                .value()
                .attr("pa")
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let datetime = card
                .select(&time)
                .next()
                .and_then(|e| e.parent())
                .and_then(scraper::ElementRef::wrap)
                .map(|e| e.text().collect::<String>())
                .and_then(|s| DATE.captures(&s).and_then(|c| date(&c[1])))
                .or_else(|| Some(chrono::Utc::now()));
            let mut row = SearchResult::provider(format!("xys-{qid}-{}", index + 1), "xys");
            row.title = title;
            row.datetime = datetime;
            row.tags = vec![link.cloud_type.to_string()];
            row.content = format!("来源：{}", link.cloud_type);
            row.links.push(link);
            Some(row)
        })
        .take(50)
        .collect()
}
#[async_trait]
impl Provider for Xys {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("xys", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut bootstrap = self.endpoints.base.join("/index/user/s").unwrap();
        bootstrap.query_pairs_mut().extend_pairs([
            ("wd", query),
            ("mode", "undefined"),
            ("stype", "undefined"),
        ]);
        let raw = tokio::time::timeout(
            ctx.request_timeout(Duration::from_secs(30)),
            request(
                || {
                    ctx.client
                        .get(bootstrap.clone())
                        .header("referer", self.endpoints.base.as_str())
                        .header("user-agent", "Mozilla/5.0 Chrome/138.0.0.0 Safari/537.36")
                },
                3,
                Duration::from_millis(200),
                BODY_LIMIT,
            ),
        )
        .await
        .map_err(|_| ProviderError::Timeout)??;
        let text = String::from_utf8_lossy(&raw);
        let token = TOKEN
            .captures(&text)
            .map(|c| c[1].to_owned())
            .ok_or_else(|| ProviderError::Parse("xys DToken missing".into()))?;
        let mut url = self.endpoints.base.join("/api/validate/searchX2").unwrap();
        url.query_pairs_mut().extend_pairs([
            ("DToken2", token.as_str()),
            ("requestID", "undefined"),
            ("mode", "90002"),
            ("stype", "undefined"),
            ("scope_content", "0"),
            ("wd", query),
            ("uk", ""),
            ("page", "1"),
            ("limit", "20"),
            ("screen_filetype", ""),
        ]);
        let raw = tokio::time::timeout(
            ctx.request_timeout(Duration::from_secs(30)),
            request(
                || {
                    ctx.client
                        .post(url.clone())
                        .header("content-type", "application/x-www-form-urlencoded")
                        .header("referer", self.endpoints.base.as_str())
                        .header("origin", self.endpoints.base.origin().ascii_serialization())
                        .header("x-requested-with", "XMLHttpRequest")
                        .header("accept", "application/json, text/plain, */*")
                },
                3,
                Duration::from_millis(200),
                BODY_LIMIT,
            ),
        )
        .await
        .map_err(|_| ProviderError::Timeout)??;
        #[derive(Deserialize)]
        struct Response {
            code: i32,
            data: String,
        }
        let r: Response =
            serde_json::from_slice(&raw).map_err(|e| ProviderError::Parse(e.to_string()))?;
        if r.code != 0 {
            return Err(ProviderError::Protocol(format!("xys API code {}", r.code)));
        }
        complete(ctx, Ok(parse(&r.data)))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bootstrap_and_results() {
        assert_eq!(
            &TOKEN.captures(r#"const DToken = "opaque";"#).unwrap()[1],
            "opaque"
        );
        let h = r#"<div class="layui-card" data-qid="1"><a onclick="open_sid(this)" url="aHR0cHM6Ly9wYW4ucXVhcmsuY24vcy9h" pa="abcd">@ movie</a><span>2026-10-20 12:00:00<i class="layui-icon-time"></i></span></div>"#;
        let rows = parse(h);
        assert_eq!(rows[0].title, "movie");
        assert_eq!(rows[0].links[0].url, "https://pan.quark.cn/s/a");
        assert_eq!(rows[0].links[0].password.as_deref(), Some("abcd"));
        assert!(rows[0].datetime.is_some());
        assert!(parse("bad").is_empty());
    }
}
