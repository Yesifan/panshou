use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use encoding_rs::GBK;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use url::Url;

use crate::core::{ProviderError, SearchResult, Source, extract_links};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};

const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/124 Safari/537.36";

fn browser_headers(request: reqwest::RequestBuilder, referer: &str) -> reqwest::RequestBuilder {
    request
        .header("user-agent", UA)
        .header(
            "accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
        )
        .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("cache-control", "max-age=0")
        .header("referer", referer)
}

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("http://www.yulinshufa.cn").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Yulinshufa {
    endpoints: Endpoints,
}
impl Yulinshufa {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}

fn gbk_escape(s: &str) -> String {
    let (bytes, _, _) = GBK.encode(s);
    bytes
        .iter()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(b) {
                (*b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn decode_gbk(bytes: &[u8]) -> String {
    let (v, _, _) = GBK.decode(bytes);
    v.into_owned()
}
fn text(e: ElementRef<'_>) -> String {
    e.text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
#[derive(Clone)]
struct Candidate {
    id: String,
    title: String,
    url: Url,
    summary: String,
    date: Option<DateTime<Utc>>,
    category: Option<String>,
    poster: Option<String>,
}
fn parse_candidates(body: &str, base: &Url) -> Result<Vec<Candidate>, ProviderError> {
    let doc = Html::parse_document(body);
    let li = Selector::parse("div.main-list-con ul.main-list > li").unwrap();
    let a = Selector::parse("div.list-con p.s-title a").unwrap();
    let desc = Selector::parse("div.list-con p.s-desc").unwrap();
    let ext = Selector::parse("div.list-con div.s-ext span.item").unwrap();
    let cat = Selector::parse("a").unwrap();
    let img = Selector::parse("div.list-pic img").unwrap();
    let idre = Regex::new(r"/xz/(\d+)").unwrap();
    let mut out = vec![];
    for row in doc.select(&li).take(15) {
        let Some(link) = row.select(&a).next() else {
            continue;
        };
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        let Ok(url) = base.join(href) else { continue };
        let title = text(link);
        if title.is_empty() {
            continue;
        }
        let id = idre
            .captures(url.path())
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_owned())
            .unwrap_or_else(|| url.path().to_owned());
        let exts = row.select(&ext).collect::<Vec<_>>();
        let category = exts
            .first()
            .and_then(|e| e.select(&cat).next())
            .map(text)
            .filter(|s| !s.is_empty());
        let date = exts
            .last()
            .and_then(|e| NaiveDate::parse_from_str(&text(*e), "%Y-%m-%d").ok())
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| DateTime::from_naive_utc_and_offset(d, Utc));
        let poster = row
            .select(&img)
            .next()
            .and_then(|e| e.value().attr("src"))
            .and_then(|u| base.join(u).ok())
            .map(|u| u.to_string());
        out.push(Candidate {
            id,
            title,
            url,
            summary: row.select(&desc).next().map(text).unwrap_or_default(),
            date,
            category,
            poster,
        });
    }
    Ok(out)
}
fn parse_detail(
    body: &str,
    c: &Candidate,
    base: &Url,
) -> Result<Option<SearchResult>, ProviderError> {
    let doc = Html::parse_document(body);
    let content_sel = Selector::parse("div.content").unwrap();
    let Some(content) = doc.select(&content_sel).next() else {
        return Ok(None);
    };
    let title_sel = Selector::parse("div.content-tit h1").unwrap();
    let info_sel = Selector::parse("div.content-info span").unwrap();
    let crumb = Selector::parse("div.content-loc a").unwrap();
    let img = Selector::parse("img").unwrap();
    let mut title = doc
        .select(&title_sel)
        .next()
        .map(text)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| c.title.clone());
    let raw = content.inner_html();
    let mut links = extract_links(&raw);
    if links.is_empty() {
        links = extract_links(&content.text().collect::<Vec<_>>().join(" "));
    }
    let p = Regex::new(r"(?i)(?:提取码|密码)[:：\s]*([0-9A-Za-z]{3,8})").unwrap();
    if let Some(code) = p
        .captures(&content.text().collect::<String>())
        .and_then(|x| x.get(1))
        .map(|x| x.as_str().to_owned())
    {
        for l in &mut links {
            if l.password.is_none() {
                l.password = Some(code.clone())
            }
        }
    }
    if links.is_empty() {
        return Ok(None);
    }
    let info = doc.select(&info_sel).collect::<Vec<_>>();
    let datetime = info
        .get(1)
        .and_then(|e| {
            NaiveDateTime::parse_from_str(&e.text().collect::<String>(), "%Y-%m-%d %H:%M:%S").ok()
        })
        .map(|d| DateTime::from_naive_utc_and_offset(d, Utc))
        .or(c.date);
    let mut tags = c.category.clone().into_iter().collect::<Vec<_>>();
    if let Some(x) = doc
        .select(&crumb)
        .next_back()
        .map(text)
        .filter(|x| !x.is_empty() && !tags.contains(x))
    {
        tags.push(x)
    }
    let images = content
        .select(&img)
        .filter_map(|e| e.value().attr("src"))
        .filter_map(|u| base.join(u).ok())
        .map(|u| u.to_string())
        .chain(
            c.poster
                .clone()
                .filter(|_| content.select(&img).next().is_none()),
        )
        .collect();
    if title.is_empty() {
        title = c.title.clone()
    }
    Ok(Some(SearchResult {
        id: format!("yulinshufa-{}", c.id),
        source: Source::provider("yulinshufa"),
        datetime,
        title,
        content: if text(content).is_empty() {
            c.summary.clone()
        } else {
            text(content)
        },
        links,
        tags,
        images,
    }))
}

#[async_trait]
impl Provider for Yulinshufa {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("yulinshufa", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let url = self
            .endpoints
            .base
            .join(&format!("/plus/search.php?q={}", gbk_escape(query)))
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        let referer = format!("{}/", self.endpoints.base.as_str().trim_end_matches('/'));
        let bytes = browser_headers(ctx.client.get(url), &referer)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?
            .error_for_status()
            .map_err(|e| ProviderError::Network(e.to_string()))?
            .bytes()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        let candidates = parse_candidates(&decode_gbk(&bytes), &self.endpoints.base)?;
        let base = self.endpoints.base.clone();
        let client = ctx.client.clone();
        let referer = referer.clone();
        let found = stream::iter(candidates.into_iter().map(|c| {
            let client = client.clone();
            let base = base.clone();
            let referer = referer.clone();
            async move {
                let bytes = browser_headers(client.get(c.url.clone()), &referer)
                    .send()
                    .await
                    .map_err(|e| ProviderError::Network(e.to_string()))?
                    .error_for_status()
                    .map_err(|e| ProviderError::Network(e.to_string()))?
                    .bytes()
                    .await
                    .map_err(|e| ProviderError::Network(e.to_string()))?;
                parse_detail(&decode_gbk(&bytes), &c, &base)
            }
        }))
        .buffered(6)
        .collect::<Vec<_>>()
        .await;
        let mut results = Vec::new();
        let mut first_error = None;
        let mut successful_details = 0usize;
        for detail in found {
            match detail {
                Ok(result) => {
                    successful_details += 1;
                    results.extend(result);
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            };
        }
        if successful_details == 0
            && let Some(error) = first_error
        {
            return Err(error);
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_success() {
        let base = Endpoints::default().base;
        let list = decode_gbk(include_bytes!(
            "../../tests/fixtures/providers/yulinshufa/success-list.gbk"
        ));
        assert!(list.contains("仙逆"));
        let c = parse_candidates(&list, &base).unwrap().pop().unwrap();
        let d = parse_detail(
            include_str!("../../tests/fixtures/providers/yulinshufa/success-detail.html"),
            &c,
            &base,
        )
        .unwrap()
        .unwrap();
        assert_eq!(d.links[0].password.as_deref(), Some("abcd"));
    }
    #[test]
    fn fixture_empty() {
        assert!(
            parse_candidates(
                include_str!("../../tests/fixtures/providers/yulinshufa/empty.html"),
                &Endpoints::default().base,
            )
            .unwrap()
            .is_empty()
        )
    }
    #[test]
    fn fixture_malformed() {
        let c = Candidate {
            id: "1".into(),
            title: "x".into(),
            url: Endpoints::default().base.clone(),
            summary: "".into(),
            date: None,
            category: None,
            poster: None,
        };
        assert!(
            parse_detail(
                include_str!("../../tests/fixtures/providers/yulinshufa/malformed.html"),
                &c,
                &Endpoints::default().base,
            )
            .unwrap()
            .is_none()
        )
    }
}
