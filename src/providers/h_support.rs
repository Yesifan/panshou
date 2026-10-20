//! Transport and DOM primitives only; each site owns its wire protocol.
use crate::{
    core::{CloudType, Link, ProviderError, SearchResult, extract_links},
    providers::SearchContext,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use std::{collections::HashSet, sync::LazyLock, time::Duration};
pub const LIMIT: usize = 16 * 1024 * 1024;
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
pub const HTML: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8";
pub fn network(e: impl Into<anyhow::Error>) -> ProviderError {
    let e = e.into();
    match e.downcast::<reqwest::Error>() {
        Ok(e) if e.is_timeout() => ProviderError::Timeout,
        Ok(e) => ProviderError::Network(e.without_url().to_string()),
        Err(e) => ProviderError::Network(e.to_string()),
    }
}
pub fn protocol(message: impl Into<String>) -> ProviderError {
    ProviderError::Protocol(message.into())
}
pub fn parse(e: serde_json::Error) -> ProviderError {
    ProviderError::Parse(e.to_string())
}
pub fn browser(
    request: reqwest::RequestBuilder,
    referer: &str,
    accept: &str,
) -> reqwest::RequestBuilder {
    request
        .header("user-agent", UA)
        .header("accept", accept)
        .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("referer", referer)
}
pub async fn request(
    ctx: &SearchContext,
    request: reqwest::RequestBuilder,
    attempts: u32,
    timeout: Duration,
    limit: usize,
) -> Result<Vec<u8>, ProviderError> {
    let mut last = ProviderError::Timeout;
    for attempt in 0..attempts {
        if ctx.is_cancelled() {
            return Err(ProviderError::Timeout);
        }
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(200 * u64::from(attempt))).await;
        }
        let request = request
            .try_clone()
            .ok_or_else(|| protocol("request is not replayable"))?
            .timeout(ctx.request_timeout(timeout));
        let result = async {
            let response = crate::http::execute(request).await.map_err(network)?;
            if response.status().as_u16() != 200 {
                return Err(protocol(format!("HTTP {}", response.status().as_u16())));
            }
            crate::http::read_response(response, limit)
                .await
                .map_err(network)
        }
        .await;
        match result {
            Ok(bytes) => return Ok(bytes),
            Err(e) => last = e,
        }
    }
    Err(last)
}
pub async fn text(
    ctx: &SearchContext,
    builder: reqwest::RequestBuilder,
    attempts: u32,
    seconds: u64,
) -> Result<String, ProviderError> {
    let bytes = tokio::time::timeout(
        ctx.request_timeout(Duration::from_secs(seconds)),
        request(ctx, builder, attempts, Duration::from_secs(seconds), LIMIT),
    )
    .await
    .map_err(|_| ProviderError::Timeout)??;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
pub fn sel(s: &str) -> Selector {
    Selector::parse(s).expect("static selector")
}
pub fn text_of(n: ElementRef<'_>) -> String {
    let mut raw = String::new();
    for node in n.descendants() {
        if node
            .ancestors()
            .filter_map(ElementRef::wrap)
            .any(|a| matches!(a.value().name(), "script" | "style"))
        {
            continue;
        }
        if let Some(text) = node.value().as_text() {
            raw.push_str(text);
        }
    }
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}
pub fn first_text(doc: &Html, s: &str) -> String {
    doc.select(&sel(s)).next().map(text_of).unwrap_or_default()
}
pub fn attr(doc: &Html, s: &str, key: &str) -> Option<String> {
    doc.select(&sel(s))
        .next()?
        .value()
        .attr(key)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
}
pub fn require_document(body: &str, markers: &str, site: &str) -> Result<Html, ProviderError> {
    let doc = Html::parse_document(body);
    if doc.select(&sel(markers)).next().is_none() {
        return Err(ProviderError::Parse(format!(
            "{site} response omitted expected document structure"
        )));
    }
    Ok(doc)
}
pub fn matches(row: &SearchResult, query: &str) -> bool {
    let hay = format!("{} {}", row.title, row.content).to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|s| hay.contains(s))
}
pub fn publish(ctx: &SearchContext, mut row: SearchResult, query: &str) -> Option<SearchResult> {
    if row.links.is_empty() || !matches(&row, query) {
        return None;
    }
    for link in &mut row.links {
        if link.work_title.is_none() {
            link.work_title = Some(row.title.clone());
        }
        link.datetime = link.datetime.or(row.datetime);
    }
    ctx.publish(vec![row.clone()]);
    Some(row)
}
pub fn date(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::{DateTime, NaiveDate, NaiveDateTime};
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|v| v.to_utc())
        .or_else(|| {
            ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y/%-m/%-d %H:%M"]
                .iter()
                .find_map(|f| NaiveDateTime::parse_from_str(s, f).ok())
                .map(|v| v.and_utc())
        })
        .or_else(|| {
            NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)
                .map(|v| v.and_utc())
        })
}
pub fn query_password(url: &str) -> Option<String> {
    url::Url::parse(url).ok()?.query_pairs().find_map(|(k, v)| {
        (["pwd", "password", "code", "passcode"].contains(&k.as_ref()) && !v.is_empty())
            .then(|| v.into_owned())
    })
}
pub fn password(s: &str) -> Option<String> {
    static PASSWORD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(?:提取码|访问码|密码|取件码|pwd|code)\s*[=:：]?\s*([0-9A-Za-z]+)")
            .unwrap()
    });
    PASSWORD.captures(s).map(|c| c[1].to_owned())
}
pub fn links_in(node: ElementRef<'_>) -> Vec<Link> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for a in node.select(&sel("a[href]")) {
        let raw = a.value().attr("href").unwrap_or("");
        let mut links = extract_links(raw);
        let parent = a.parent().and_then(ElementRef::wrap);
        let code = password(&text_of(a))
            .or_else(|| a.value().attr("title").and_then(password))
            .or_else(|| parent.and_then(|n| password(&text_of(n))))
            .or_else(|| {
                parent
                    .and_then(|n| n.next_siblings().find_map(ElementRef::wrap))
                    .and_then(|n| password(&text_of(n)))
            });
        for link in &mut links {
            link.password = query_password(&link.url)
                .or_else(|| code.clone())
                .or_else(|| link.password.clone());
        }
        for link in links {
            if seen.insert(link.url.clone()) {
                out.push(link);
            }
        }
    }
    for link in extract_links(&text_of(node)) {
        if seen.insert(link.url.clone()) {
            out.push(link);
        }
    }
    out
}
pub fn cloud(kind: &str) -> CloudType {
    match kind.trim().to_lowercase().as_str() {
        "ali" | "alipan" | "aliyun" | "aliyundrive" => CloudType::Aliyun,
        "123" | "a123" | "123pan" => CloudType::Pan123,
        "tianyi" => CloudType::Tianyi,
        "115" => CloudType::One15,
        other => other.parse().unwrap_or(CloudType::Others),
    }
}
pub fn short(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
