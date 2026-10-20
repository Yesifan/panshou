use crate::{core::ProviderError, providers::SearchContext};
use chrono::{DateTime, NaiveDateTime, Utc};
use scraper::{ElementRef, Selector};
use std::time::Duration;
use url::Url;
pub(super) fn sel(s: &str) -> Selector {
    Selector::parse(s).expect("constant CSS selector")
}
pub(super) fn text(node: ElementRef<'_>) -> String {
    node.text().collect::<String>().trim().to_owned()
}
pub(super) fn matches(title: &str, query: &str) -> bool {
    let title = title.to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|s| title.contains(s))
}
pub(super) fn time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s.trim())
        .ok()
        .map(|t| t.with_timezone(&Utc))
        .or_else(|| {
            NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|t| t.and_utc())
        })
}
pub(super) async fn get(
    ctx: &SearchContext,
    url: Url,
    referer: &str,
    seconds: u64,
    attempts: usize,
) -> Result<String, ProviderError> {
    super::http::text(ctx.client.get(url).header("user-agent","Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/138.0.0.0 Safari/537.36").header("accept","text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8").header("accept-language","zh-CN,zh;q=0.9,en;q=0.8").header("referer",referer),attempts,200,true,ctx.request_timeout(Duration::from_secs(seconds)),16<<20).await
}
pub(super) fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for b in bytes {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb88320 } else { 0 };
        }
    }
    !crc
}
