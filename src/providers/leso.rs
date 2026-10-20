//! Discuz search with an explicit, single redirect hop and a private cookie jar.
use std::{collections::HashSet, sync::LazyLock, time::Duration};

use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::Html;
use tokio::sync::OnceCell;
use url::Url;

use super::{UA, body, bounded, check, protocol, selector, send, session, text};
use crate::{
    core::{ProviderError, SearchResult, extract_links},
    http::{ClientOptions, RedirectPolicy, Session},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};

static THREAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:tid=|thread-)([0-9]+)").unwrap());
static DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"20\d{2}[-/]\d{1,2}[-/]\d{1,2}(?:\s+\d{1,2}:\d{2})?").unwrap());
const TIMEOUT: Duration = Duration::from_secs(20);

pub struct Leso {
    base: Url,
    options: ClientOptions,
    session: OnceCell<Session>,
}
impl Default for Leso {
    fn default() -> Self {
        Self::new("https://www.leso.cc/".parse().unwrap())
    }
}
impl Leso {
    pub fn new(base: Url) -> Self {
        Self::with_options(base, ClientOptions::default())
    }
    pub fn with_options(base: Url, mut options: ClientOptions) -> Self {
        options.redirect = RedirectPolicy::None;
        Self {
            base,
            options,
            session: OnceCell::new(),
        }
    }
    async fn client(&self) -> Result<&reqwest::Client, ProviderError> {
        Ok(self
            .session
            .get_or_try_init(|| async { session(&self.options) })
            .await?
            .client())
    }
    fn headers(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header("User-Agent", UA)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
            .header("Referer", self.base.as_str())
    }
    async fn document(
        &self,
        ctx: &SearchContext,
        url: Url,
        form: Option<&[(&str, &str)]>,
        limit: usize,
    ) -> Result<String, ProviderError> {
        let client = self.client().await?;
        let request = match form {
            Some(form) => client.post(url.clone()).form(form),
            None => client.get(url.clone()),
        };
        let mut response = send(ctx, self.headers(request), TIMEOUT).await?;
        if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            let status = response.status().as_u16();
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| protocol(format!("leso HTTP {status}: missing Location")))?;
            let next = url
                .join(location)
                .map_err(|_| protocol("leso invalid redirect Location"))?;
            if !matches!(next.scheme(), "http" | "https")
                || !next.username().is_empty()
                || next.password().is_some()
            {
                return Err(protocol("leso unsafe redirect Location"));
            }
            drop(response);
            response = send(ctx, self.headers(client.get(next)), TIMEOUT).await?;
        }
        if response.status() != reqwest::StatusCode::OK {
            return Err(protocol(format!(
                "leso HTTP {}",
                response.status().as_u16()
            )));
        }
        body(response, limit).await
    }
    fn items(&self, source: &str) -> Result<Vec<Item>, ProviderError> {
        let doc = Html::parse_document(source);
        if doc.select(&selector(".slst")).next().is_none() {
            return Err(ProviderError::Parse(
                "leso search: missing result list".into(),
            ));
        }
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        for node in doc.select(&selector(".slst li.pbw")) {
            let Some(anchor) = node.select(&selector("h3 a")).next() else {
                continue;
            };
            let Some(href) = anchor.value().attr("href") else {
                continue;
            };
            let Some(id) = THREAD
                .captures(href)
                .and_then(|m| m.get(1))
                .map(|m| m.as_str().to_owned())
            else {
                continue;
            };
            let title = text(anchor);
            if title.is_empty() || !seen.insert(id.clone()) {
                continue;
            }
            let url = self
                .base
                .join(href)
                .map_err(|_| protocol("leso invalid detail URL"))?;
            if url.origin() != self.base.origin() {
                continue;
            }
            let raw_text = text(node);
            let datetime = DATE.find(&raw_text).and_then(|found| {
                let date = found.as_str().replace('/', "-");
                chrono::NaiveDateTime::parse_from_str(&date, "%Y-%m-%d %H:%M")
                    .ok()
                    .or_else(|| {
                        chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d")
                            .ok()?
                            .and_hms_opt(0, 0, 0)
                    })
                    .map(|value| value.and_utc())
            });
            items.push(Item {
                id,
                title,
                url,
                datetime,
            });
            if items.len() == 30 {
                break;
            }
        }
        if items.is_empty() && doc.select(&selector(".slst li.pbw")).next().is_some() {
            return Err(ProviderError::Parse(
                "leso search: malformed result entries".into(),
            ));
        }
        Ok(items)
    }
    async fn detail(&self, ctx: &SearchContext, item: Item) -> Result<SearchResult, ProviderError> {
        check(ctx)?;
        let source = self.document(ctx, item.url, None, 8 << 20).await?;
        let doc = Html::parse_document(&source);
        let content = doc
            .select(&selector("td[id^=postmessage_], td.t_f"))
            .next()
            .or_else(|| doc.select(&selector("body")).next())
            .ok_or_else(|| protocol("leso missing detail body"))?;
        let content_text = text(content);
        let mut links = extract_links(&format!("{}\n{content_text}", content.html()));
        if links.is_empty() {
            return Err(protocol("leso detail has no usable links"));
        }
        for link in &mut links {
            link.work_title = Some(item.title.clone());
        }
        let mut row = SearchResult::provider(format!("leso-{}", item.id), "leso");
        row.title = item.title;
        row.content = content_text.chars().take(600).collect();
        row.datetime = item.datetime;
        row.links = links;
        if let Some(src) = content
            .select(&selector("img[src]"))
            .next()
            .and_then(|node| node.value().attr("src"))
            && let Ok(url) = self.base.join(src)
        {
            row.images.push(url.to_string());
        }
        Ok(row)
    }
    async fn search_inner(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let url = self.base.join("search.php?searchsubmit=yes").unwrap();
        let source = self
            .document(
                ctx,
                url,
                Some(&[
                    ("mod", "forum"),
                    ("srchtxt", query),
                    ("searchsubmit", "yes"),
                ]),
                6 << 20,
            )
            .await?;
        let items = self.items(&source)?;
        let count = items.len();
        let mut jobs = stream::iter(
            items
                .into_iter()
                .enumerate()
                .map(|(index, item)| async move { (index, self.detail(ctx, item).await) }),
        )
        .buffer_unordered(6);
        let mut rows = Vec::new();
        let mut first_error = None;
        while let Some((index, result)) = jobs.next().await {
            match result {
                Ok(row) => {
                    ctx.publish(vec![row.clone()]);
                    rows.push((index, row));
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        if let Some(error) = first_error {
            if rows.is_empty() {
                return Err(protocol(format!(
                    "leso all {count} details failed: {error}"
                )));
            }
            return Err(protocol(format!(
                "leso incomplete details ({}/{count} completed): {error}",
                rows.len()
            )));
        }
        rows.sort_by_key(|(index, _)| *index);
        Ok(rows.into_iter().map(|(_, row)| row).collect())
    }
}
struct Item {
    id: String,
    title: String,
    url: Url,
    datetime: Option<chrono::DateTime<chrono::Utc>>,
}
#[async_trait]
impl Provider for Leso {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("leso", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        bounded(ctx, self.search_inner(ctx, query)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_string, header, method, path},
    };
    const LIST: &str = r#"<div class="slst"><li class="pbw"><h3><a href="thread-101-1-1.html">Movie</a></h3>2026-9-20</li><li class="pbw"><h3><a href="thread-102-1-1.html">Movie 2</a></h3></li></div>"#;
    async fn redirect(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/search.php"))
            .and(body_string("mod=forum&srchtxt=Movie&searchsubmit=yes"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", "results")
                    .insert_header("Set-Cookie", "sid=one; Path=/"),
            )
            .expect(1)
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/results"))
            .and(header("cookie", "sid=one"))
            .respond_with(ResponseTemplate::new(200).set_body_string(LIST))
            .expect(1)
            .mount(server)
            .await;
    }
    #[tokio::test]
    async fn post_relative_get_cookie_details_and_partial() {
        let server = MockServer::start().await;
        redirect(&server).await;
        Mock::given(path("/thread-101-1-1.html")).respond_with(ResponseTemplate::new(200).set_body_string("<table><tr><td class=t_f>Movie https://pan.quark.cn/s/abcdef 提取码: 1234</td></tr></table>")).mount(&server).await;
        Mock::given(path("/thread-102-1-1.html"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let rows = super::super::assert_failed_partial(
            std::sync::Arc::new(Leso::new(server.uri().parse().unwrap())),
            "Movie",
        )
        .await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].links[0].password.as_deref(), Some("1234"));
        assert_eq!(rows[0].links[0].work_title.as_deref(), Some("Movie"));
    }
    #[tokio::test]
    async fn empty_malformed_and_redirect_failures() {
        let server = MockServer::start().await;
        for (response, expected) in [
            (
                ResponseTemplate::new(200)
                    .set_body_string("<div class=slst>没有找到相关结果</div>"),
                None,
            ),
            (
                ResponseTemplate::new(200).set_body_string("not a result document"),
                Some("missing result list"),
            ),
            (ResponseTemplate::new(302), Some("missing Location")),
            (ResponseTemplate::new(503), Some("503")),
        ] {
            server.reset().await;
            Mock::given(method("POST"))
                .respond_with(response)
                .expect(1)
                .mount(&server)
                .await;
            let result = Leso::new(server.uri().parse().unwrap())
                .search(&super::super::context(), "Movie")
                .await;
            if let Some(message) = expected {
                assert!(result.unwrap_err().to_string().contains(message));
            } else {
                assert!(result.unwrap().is_empty());
            }
        }
        server.reset().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(307).insert_header("Location", "/again"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/again"))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", "/never"))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/never"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        assert!(
            Leso::new(server.uri().parse().unwrap())
                .search(&super::super::context(), "Movie")
                .await
                .unwrap_err()
                .to_string()
                .contains("302")
        );
    }
    #[tokio::test]
    async fn all_details_failed_and_deadline_preserves_completed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(LIST))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        assert!(
            Leso::new(server.uri().parse().unwrap())
                .search(&super::super::context(), "Movie")
                .await
                .unwrap_err()
                .to_string()
                .contains("all 2 details failed")
        );
        server.reset().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(LIST))
            .mount(&server)
            .await;
        Mock::given(path("/thread-101-1-1.html"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("<div>https://pan.quark.cn/s/abcdef</div>"),
            )
            .mount(&server)
            .await;
        Mock::given(path("/thread-102-1-1.html"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(1)))
            .mount(&server)
            .await;
        let ctx = super::super::context()
            .for_source(tokio::time::Instant::now() + Duration::from_millis(150));
        assert_eq!(
            Leso::new(server.uri().parse().unwrap())
                .search(&ctx, "Movie")
                .await
                .unwrap_err(),
            ProviderError::Timeout
        );
        assert!(ctx.has_results());
    }
}
