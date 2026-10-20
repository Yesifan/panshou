trait ExecuteRequest {
    async fn execute(self) -> anyhow::Result<reqwest::Response>;
}
impl ExecuteRequest for reqwest::RequestBuilder {
    async fn execute(self) -> anyhow::Result<reqwest::Response> {
        crate::http::execute(self).await
    }
}

trait RetrySend {
    async fn send_retry(self) -> anyhow::Result<reqwest::Response>;
}
impl RetrySend for reqwest::RequestBuilder {
    async fn send_retry(self) -> anyhow::Result<reqwest::Response> {
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(200 << (attempt - 1))).await;
            }
            let request = self
                .try_clone()
                .expect("provider request body is replayable");
            let result = request.execute().await.and_then(|r| {
                if r.status().as_u16() == 200 {
                    Ok(r)
                } else {
                    Err(anyhow::anyhow!("HTTP {}", r.status()))
                }
            });
            match result {
                Ok(response) => return Ok(response),
                Err(error) if attempt == 2 => return Err(error),
                Err(_) => {}
            }
        }
        unreachable!("three attempts return a response or error")
    }
}

async fn read_text(response: reqwest::Response) -> anyhow::Result<String> {
    let status = response.status();
    if status != reqwest::StatusCode::OK {
        return Err(anyhow::anyhow!("duanjuw HTTP {status}"));
    }
    let body = crate::http::read_response(response, 16 * 1024 * 1024)
        .await
        .map_err(|error| error.context(format!("duanjuw response body HTTP {status}")))?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

use crate::core::{CloudType, Link, ProviderError, SearchResult, Source};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use regex::Regex;
use scraper::{Html, Selector};
use url::Url;

const USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 16_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148";

fn request(client: &reqwest::Client, url: Url, referer: &Url) -> reqwest::RequestBuilder {
    client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(
            reqwest::header::ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header(reqwest::header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
        .header(reqwest::header::CONNECTION, "keep-alive")
        .header(reqwest::header::REFERER, referer.as_str())
}
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://sm3.cc/").expect("static URL"),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct DuanjuwProvider {
    endpoints: Endpoints,
}
impl DuanjuwProvider {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}
#[derive(Debug)]
struct Pending {
    result: SearchResult,
    detail: Option<Url>,
}
fn extract_links(fragment: &str) -> Vec<Link> {
    let doc = Html::parse_fragment(fragment);
    let selector = Selector::parse("a[href]").expect("selector");
    static PWD_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?i)(?:提取码|密码|pwd|code)\s*[=:：]?\s*([0-9A-Za-z]+)").expect("regex")
    });
    let pwd = &*PWD_RE;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for a in doc.select(&selector) {
        let Some(raw) = a.value().attr("href").map(str::trim) else {
            continue;
        };
        let mut link = Link::new(raw);
        if link.cloud_type == CloudType::Others || !seen.insert(raw.to_owned()) {
            continue;
        }
        link.password = Url::parse(raw)
            .ok()
            .and_then(|u| {
                u.query_pairs()
                    .find(|(k, _)| matches!(k.as_ref(), "pwd" | "passcode" | "code"))
                    .map(|(_, v)| v.into_owned())
            })
            .or_else(|| {
                let t = a
                    .parent()
                    .and_then(scraper::ElementRef::wrap)
                    .map(|p| p.text().collect::<String>())
                    .unwrap_or_default();
                pwd.captures(&t)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_str().to_owned())
            });
        out.push(link)
    }
    out
}

fn parse_detail(body: &str) -> (Vec<Link>, String) {
    let doc = Html::parse_document(body);
    let content_selector = Selector::parse("div.content").expect("selector");
    let legacy_content_selector = Selector::parse("div.tx-text").expect("selector");
    let meta_selector = Selector::parse("meta[name='description']").expect("selector");
    let content = doc
        .select(&content_selector)
        .next()
        .or_else(|| doc.select(&legacy_content_selector).next());
    let mut links = content
        .as_ref()
        .map(|node| extract_links(&node.html()))
        .unwrap_or_default();
    if links.is_empty() {
        links = extract_links(body);
    }
    let description = doc
        .select(&meta_selector)
        .next()
        .and_then(|node| node.value().attr("content"))
        .map(clean_text)
        .filter(|text| !text.is_empty())
        .or_else(|| content.map(|node| clean_text(&node.text().collect::<String>())))
        .unwrap_or_default();
    (links, description)
}

fn clean_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn parse_search(body: &str, base: &Url) -> Result<Vec<Pending>, ProviderError> {
    let doc = Html::parse_document(body);
    let bubble = Selector::parse(".message.system .bubble")
        .map_err(|e| ProviderError::Parse(e.to_string()))?;
    if let Some(node) = doc.select(&bubble).next() {
        let html = node
            .inner_html()
            .replace("<br>", "\n")
            .replace("<br/>", "\n")
            .replace("<br />", "\n");
        static STARTS_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
            Regex::new(r"(?m)(?:^|\n)\s*\d+\s*[、.．]\s*《").expect("regex")
        });
        let starts = STARTS_RE
            .find_iter(&html)
            .map(|m| m.start())
            .collect::<Vec<_>>();
        let mut out = Vec::new();
        for (i, start) in starts.iter().enumerate() {
            let end = starts.get(i + 1).copied().unwrap_or(html.len());
            let seg = &html[*start..end];
            let Some(open) = seg.find('《') else {
                continue;
            };
            let Some(close_rel) = seg[open + '《'.len_utf8()..].find('》') else {
                continue;
            };
            let close = open + '《'.len_utf8() + close_rel;
            let title = Html::parse_fragment(&seg[open + '《'.len_utf8()..close])
                .root_element()
                .text()
                .collect::<String>()
                .trim()
                .to_owned();
            let links = extract_links(seg);
            if title.is_empty() || links.is_empty() {
                continue;
            }
            out.push(Pending {
                result: SearchResult {
                    id: format!("duanjuw-{title}-{}", links[0].url),
                    source: Source::provider("duanjuw"),
                    datetime: None,
                    title,
                    content: Html::parse_fragment(seg)
                        .root_element()
                        .text()
                        .collect::<Vec<_>>()
                        .join(" "),
                    links,
                    tags: vec![],
                    images: vec![],
                },
                detail: None,
            });
        }
        return Ok(out);
    }
    let cards = Selector::parse("li.col-6").expect("selector");
    let anchor = Selector::parse("h3.f-14 a").expect("selector");
    let image = Selector::parse("img.lazy").expect("selector");
    let mut out = Vec::new();
    for card in doc.select(&cards) {
        let Some(a) = card.select(&anchor).next() else {
            continue;
        };
        let title = a
            .text()
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let Some(href) = a.value().attr("href") else {
            continue;
        };
        let Ok(detail) = base.join(href) else {
            continue;
        };
        let images = card
            .select(&image)
            .next()
            .and_then(|n| n.value().attr("data-original"))
            .and_then(|v| base.join(v).ok())
            .map(|u| vec![u.to_string()])
            .unwrap_or_default();
        out.push(Pending {
            result: SearchResult {
                id: format!("duanjuw-{}", detail),
                source: Source::provider("duanjuw"),
                datetime: None,
                title,
                content: String::new(),
                links: vec![],
                tags: vec![],
                images,
            },
            detail: Some(detail),
        });
    }
    Ok(out)
}
#[async_trait]
impl Provider for DuanjuwProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("duanjuw", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self
            .endpoints
            .base
            .join("so/search.php")
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("act", "search")
            .append_pair("q", query)
            .append_pair("page", "1");
        let r = tokio::time::timeout(
            ctx.timeout,
            request(&ctx.client, url, &self.endpoints.base).send_retry(),
        )
        .await
        .map_err(|_| ProviderError::Timeout)?
        .map_err(|e| ProviderError::Network(e.to_string()))?;
        if !r.status().is_success() {
            return Err(ProviderError::Protocol(format!("HTTP {}", r.status())));
        }
        let rows = parse_search(
            &read_text(r)
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?,
            &self.endpoints.base,
        )?;
        let jobs = rows.into_iter().map(|mut p| async move {
            let Some(url) = p.detail else {
                return Ok(Some(p.result));
            };
            let r = request(&ctx.client, url, &self.endpoints.base)
                .send_retry()
                .await
                .map_err(super::c_support::network)?;
            if r.status() != reqwest::StatusCode::OK {
                return Err(ProviderError::Protocol(format!(
                    "duanjuw detail HTTP {}",
                    r.status()
                )));
            }
            let text = read_text(r).await.map_err(super::c_support::network)?;
            let (links, content) = parse_detail(&text);
            if links.is_empty() {
                return Ok(None);
            }
            p.result.links = links;
            p.result.content = content;
            Ok::<_, ProviderError>(Some(p.result))
        });
        super::collect_partial(ctx, jobs, 6).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn detail_status_is_preserved_and_completed_rows_survive() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        let server = MockServer::start().await;
        Mock::given(path("/so/search.php")).respond_with(ResponseTemplate::new(200).set_body_string(r#"<ul><li class="col-6"><h3 class="f-14"><a href="/good">仙逆</a></h3></li><li class="col-6"><h3 class="f-14"><a href="/failed">仙逆 2</a></h3></li></ul>"#)).mount(&server).await;
        Mock::given(path("/good"))
            .respond_with(ResponseTemplate::new(200).set_body_string(include_str!(
                "../../tests/fixtures/providers/duanjuw/detail_scope.html"
            )))
            .mount(&server)
            .await;
        Mock::given(path("/failed"))
            .respond_with(ResponseTemplate::new(503))
            .expect(3)
            .mount(&server)
            .await;
        let base: Url = server.uri().parse().unwrap();
        let provider = DuanjuwProvider::new(Endpoints { base });
        let options = crate::http::ClientOptions {
            redirect: crate::http::RedirectPolicy::None,
            ..Default::default()
        };
        let client = crate::http::HttpClientFactory::new()
            .client(&options)
            .unwrap();
        let ctx = SearchContext::new(client, std::time::Duration::from_secs(5));
        let error = provider.search(&ctx, "仙逆").await.unwrap_err();
        assert!(error.to_string().contains("503"), "{error}");
        let rows = ctx.finish(Vec::new());
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].links.is_empty());
    }

    #[test]
    fn success() {
        let b = Url::parse("https://sm3.cc/").unwrap();
        let r = parse_search(
            include_str!("../../tests/fixtures/providers/duanjuw/success.html"),
            &b,
        )
        .unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].result.links[0].password.as_deref(), Some("abcd"));
    }
    #[test]
    fn empty() {
        let b = Url::parse("https://x/").unwrap();
        assert!(
            parse_search(
                include_str!("../../tests/fixtures/providers/duanjuw/empty.html"),
                &b
            )
            .unwrap()
            .is_empty()
        );
    }
    #[test]
    fn malformed() {
        assert!(
            extract_links(include_str!(
                "../../tests/fixtures/providers/duanjuw/malformed.html"
            ))
            .is_empty()
        );
    }
    #[test]
    fn detail_prefers_content_links_over_footer() {
        let (links, content) = parse_detail(include_str!(
            "../../tests/fixtures/providers/duanjuw/detail_scope.html"
        ));
        assert_eq!(links.len(), 1);
        assert!(links[0].url.contains("wanted"));
        assert!(!links[0].url.contains("footer"));
        assert_eq!(content, "正片资源 提取码 abcd");
    }
}
