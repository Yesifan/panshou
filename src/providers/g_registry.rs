//! Fixed-snapshot Group A2 adapters; protocol parsing remains source-local.
use super::{Provider, SearchContext};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use scraper::{ElementRef, Html, Selector};
use std::{collections::HashSet, sync::Arc, time::Duration};

#[path = "ahhhhfs.rs"]
pub mod ahhhhfs;
#[path = "alupan.rs"]
pub mod alupan;
#[path = "bixin.rs"]
pub mod bixin;
#[path = "daishudj.rs"]
pub mod daishudj;
#[path = "diduan.rs"]
pub mod diduan;
#[path = "discourse.rs"]
pub mod discourse;
#[path = "duoduo.rs"]
pub mod duoduo;
#[path = "ys5266.rs"]
pub mod ys5266;

pub fn providers() -> Vec<Arc<dyn Provider>> {
    vec![
        Arc::new(ys5266::Ys5266Provider::default()),
        Arc::new(ahhhhfs::AhhhhfsProvider::default()),
        Arc::new(alupan::AlupanProvider::default()),
        Arc::new(bixin::BixinProvider::default()),
        Arc::new(daishudj::DaishudjProvider::default()),
        Arc::new(diduan::DiduanProvider::default()),
        Arc::new(discourse::DiscourseProvider::default()),
        Arc::new(duoduo::DuoduoProvider::default()),
    ]
}
pub(crate) const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120.0.0.0 Safari/537.36";
pub(crate) fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
pub(crate) fn clean(s: &str) -> String {
    Html::parse_fragment(s)
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
pub(crate) fn text(node: ElementRef<'_>, selector: &str) -> String {
    node.select(&sel(selector))
        .map(|n| clean(&n.inner_html()))
        .collect::<Vec<_>>()
        .join(" ")
}
pub(crate) fn password(raw: &str, label: &str) -> Option<String> {
    if let Ok(url) = url::Url::parse(raw) {
        for key in ["pwd", "password", "passcode", "code"] {
            if let Some((_, v)) = url.query_pairs().find(|(k, v)| k == key && !v.is_empty()) {
                return Some(v.into_owned());
            }
        }
    }
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)(?:提取码|访问码|密码|pwd|code)\s*[:：=]?\s*([a-z0-9_-]{3,32})")
            .unwrap()
    });
    RE.captures(label).map(|c| c[1].to_owned())
}
pub(crate) fn matches(row: &SearchResult, query: &str) -> bool {
    let text = format!("{} {}", row.title, row.content).to_lowercase();
    query
        .split_whitespace()
        .all(|q| text.contains(&q.to_lowercase()))
}
pub(crate) fn date(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&chrono::Utc))
        .or_else(|| {
            ["%Y-%m-%d", "%Y/%m/%d", "%Y年%m月%d日"]
                .into_iter()
                .find_map(|format| {
                    chrono::NaiveDate::parse_from_str(s.trim().trim_matches(['(', ')']), format)
                        .ok()
                })
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|d| d.and_utc())
        })
}
/// Replay transport/status failures at the HTTP boundary, never malformed schemas.
/// Bixin alone includes body reads inside its retry boundary; other upstream
/// adapters finish retrying as soon as HTTP 200 headers have arrived.
pub(crate) async fn fetch(
    ctx: &SearchContext,
    request: reqwest::RequestBuilder,
    source: &str,
    stage: &str,
    maximum: Duration,
    limit: usize,
    delays: &[u64],
) -> Result<String, ProviderError> {
    let deadline = tokio::time::Instant::now() + ctx.request_timeout(maximum);
    let operation = async {
        let mut last = ProviderError::Timeout;
        for attempt in 0..=delays.len() {
            if ctx.is_cancelled() {
                return Err(ProviderError::Timeout);
            }
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(delays[attempt - 1])).await;
            }
            if ctx.is_cancelled() {
                return Err(ProviderError::Timeout);
            }
            let request = request.try_clone().ok_or_else(|| {
                ProviderError::Protocol(format!("{source} {stage}: unreplayable request"))
            })?;
            let result = async {
                let response = crate::http::execute(request).await.map_err(|_| {
                    (
                        ProviderError::Network(format!("{source} {stage}: transport error")),
                        true,
                    )
                })?;
                let status = response.status();
                if status.as_u16() != 200 {
                    return Err((
                        ProviderError::Protocol(format!("{source} {stage}: HTTP {status}")),
                        true,
                    ));
                }
                let body = crate::http::read_response(response, limit)
                    .await
                    .map_err(|e| {
                        (
                            ProviderError::Protocol(format!("{source} {stage}: {e}")),
                            source == "bixin",
                        )
                    })?;
                Ok(String::from_utf8_lossy(&body).into_owned())
            }
            .await;
            match result {
                Ok(body) => return Ok(body),
                Err((error, false)) => return Err(error),
                Err((error, true)) => last = error,
            }
        }
        Err(last)
    };
    tokio::time::timeout_at(deadline, operation)
        .await
        .map_err(|_| ProviderError::Timeout)?
}
pub(crate) fn headers(request: reqwest::RequestBuilder, referer: &str) -> reqwest::RequestBuilder {
    request
        .header("User-Agent", UA)
        .header("Referer", referer)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
        )
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
}
/// Anchor-local passwords avoid assigning another share's code across a page.
pub(crate) fn anchors(body: &str, selector: &str) -> Vec<Link> {
    let doc = Html::parse_document(body);
    let mut seen = HashSet::new();
    let mut links = Vec::new();
    for node in doc.select(&sel(selector)) {
        let Some(raw) = node
            .value()
            .attr("href")
            .or_else(|| node.value().attr("data-clipboard-text"))
        else {
            continue;
        };
        let raw = raw.trim();
        let mut link = Link::new(raw);
        if link.cloud_type == CloudType::Others || !seen.insert(raw.to_owned()) {
            continue;
        }
        let label = format!(
            "{} {}",
            node.value().attr("title").unwrap_or(""),
            clean(&node.inner_html()),
        );
        link.password = password(raw, &label).or_else(|| {
            let parent = node.parent().and_then(ElementRef::wrap)?;
            let anchors = parent
                .select(&sel("a[href], [data-clipboard-text]"))
                .filter(|anchor| {
                    anchor
                        .value()
                        .attr("href")
                        .or_else(|| anchor.value().attr("data-clipboard-text"))
                        .is_some_and(|raw| Link::new(raw).cloud_type != CloudType::Others)
                })
                .count();
            // A shared parent containing several downloads cannot establish
            // which link owns a trailing code.
            (anchors == 1)
                .then(|| password(raw, &clean(&parent.inner_html())))
                .flatten()
        });
        links.push(link);
    }
    links
}
pub(crate) fn raw_links(body: &str) -> Vec<Link> {
    static URL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?:https?://|magnet:\?|ed2k://)[^\s<>\"']+"#).unwrap()
    });
    let decoded = body.replace("&amp;", "&");
    let mut seen = HashSet::new();
    URL.find_iter(&decoded)
        .filter_map(|m| {
            let raw = m.as_str().trim_end_matches([')', '。', '，']);
            let mut link = Link::new(raw);
            if link.cloud_type == CloudType::Others || !seen.insert(raw.to_owned()) {
                return None;
            }
            let rest = &decoded[m.end()..];
            let end = ["http://", "https://", "magnet:", "ed2k:"]
                .iter()
                .filter_map(|prefix| rest.find(prefix))
                .min()
                .unwrap_or(rest.len());
            let tail = rest[..end].chars().take(60).collect::<String>();
            link.password = password(raw, &tail);
            Some(link)
        })
        .collect()
}
#[cfg(test)]
#[path = "g_tests.rs"]
mod tests;
