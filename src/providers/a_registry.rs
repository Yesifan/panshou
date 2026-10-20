//! Group A adapters. Integration: declare `pub mod a_registry;` in providers/mod.rs
//! and extend builtin_stateless_providers() with `a_registry::providers()`.
use super::Provider;
use crate::core::ProviderError;
use std::{sync::Arc, time::Duration};

#[path = "aikanzy.rs"]
pub mod aikanzy;
#[path = "aipan.rs"]
pub mod aipan;
#[path = "ash.rs"]
pub mod ash;
#[path = "btbtlb.rs"]
pub mod btbtlb;
#[path = "dy4k.rs"]
pub mod dy4k;
#[path = "dyyjpro.rs"]
pub mod dyyjpro;
#[path = "erxiao.rs"]
pub mod erxiao;
#[path = "feikuai.rs"]
pub mod feikuai;
#[path = "gaoqing888.rs"]
pub mod gaoqing888;
#[path = "haisou.rs"]
pub mod haisou;
#[path = "hdr4k.rs"]
pub mod hdr4k;
#[path = "huban.rs"]
pub mod huban;
#[path = "hunhepan.rs"]
pub mod hunhepan;
#[path = "ikantv.rs"]
pub mod ikantv;
#[path = "javdb.rs"]
pub mod javdb;
#[path = "run488.rs"]
pub mod run488;

pub fn providers() -> Vec<Arc<dyn Provider>> {
    vec![
        Arc::new(run488::Run488Provider::default()),
        Arc::new(aikanzy::AikanzyProvider::default()),
        Arc::new(erxiao::ErxiaoProvider::default()),
        Arc::new(huban::HubanProvider::default()),
        Arc::new(dy4k::Dy4kProvider::default()),
        Arc::new(dyyjpro::DyyjproProvider::default()),
        Arc::new(haisou::HaisouProvider::default()),
        Arc::new(hdr4k::Hdr4kProvider::default()),
        Arc::new(javdb::JavdbProvider::default()),
        Arc::new(feikuai::FeikuaiProvider::default()),
        Arc::new(aipan::AipanProvider::default()),
        Arc::new(ikantv::IkantvProvider::default()),
        Arc::new(ash::AshProvider::default()),
        Arc::new(hunhepan::HunhepanProvider::default()),
        Arc::new(btbtlb::BtbtlbProvider::default()),
        Arc::new(gaoqing888::Gaoqing888Provider::default()),
    ]
}

pub(crate) const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/138.0.0.0 Safari/537.36";

/// Replayable GET/POST builder, with per-attempt timeout and cancellable backoff.
/// Protocol parsers deliberately remain in their individual adapter modules.
pub(crate) async fn fetch(
    request: reqwest::RequestBuilder,
    source: &str,
    stage: &str,
    timeout: Duration,
    limit: usize,
    delays: &[u64],
) -> Result<String, ProviderError> {
    let mut last = ProviderError::Unavailable(format!("{source} {stage}"));
    for attempt in 0..=delays.len() {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(delays[attempt - 1])).await;
        }
        let request = request.try_clone().ok_or_else(|| {
            ProviderError::Protocol(format!("{source} {stage}: request cannot be replayed"))
        })?;
        let result = tokio::time::timeout(timeout, async {
            let response = crate::http::execute(request).await.map_err(|_| {
                ProviderError::Network(format!("{source} {stage}: transport error"))
            })?;
            let status = response.status();
            if status != reqwest::StatusCode::OK {
                return Err(ProviderError::Protocol(format!(
                    "{source} {stage}: HTTP {status}"
                )));
            }
            let body = crate::http::read_response(response, limit)
                .await
                .map_err(|e| {
                    ProviderError::Protocol(format!("{source} {stage}: HTTP {status}: {e}"))
                })?;
            Ok(String::from_utf8_lossy(&body).into_owned())
        })
        .await;
        match result {
            Ok(Ok(body)) => return Ok(body),
            Ok(Err(e)) => last = e,
            Err(_) => last = ProviderError::Timeout,
        }
    }
    Err(last)
}

pub(crate) fn clean(value: &str) -> String {
    scraper::Html::parse_fragment(value)
        .root_element()
        .text()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Common URL extraction only; each adapter selects its own actual protocol DOM.
pub(crate) fn html_links(scope: scraper::ElementRef<'_>) -> Vec<crate::core::Link> {
    use crate::core::{CloudType, Link};
    static URL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?i)(?:https?://|magnet:\?xt=urn:btih:|ed2k://)[^\s\"'<>]+"#).unwrap()
    });
    let mut seen = std::collections::HashSet::new();
    let mut links = Vec::new();
    let mut add = |raw: &str, context: &str| {
        let raw = raw
            .trim()
            .trim_matches(|c| "\"'<>()，,。.;".contains(c))
            .replace("&amp;", "&");
        let Ok(url) = url::Url::parse(&raw) else {
            return;
        };
        if !url.username().is_empty() || url.password().is_some() {
            return;
        }
        let mut link = Link::new(raw);
        if link.cloud_type == CloudType::Others {
            return;
        }
        link.password = password(&link.url, context);
        if seen.insert((link.url.clone(), link.password.clone())) {
            links.push(link);
        }
    };
    for a in scope.select(&scraper::Selector::parse("a[href]").unwrap()) {
        let context = a
            .parent()
            .and_then(scraper::ElementRef::wrap)
            .map(|n| clean(&n.inner_html()))
            .unwrap_or_default();
        if let Some(raw) = a.value().attr("href") {
            add(raw, &context);
        }
    }
    let text = scope.text().collect::<Vec<_>>().join(" ");
    for m in URL.find_iter(&text) {
        add(m.as_str(), "");
    }
    links
}

pub(crate) fn password(raw: &str, label: &str) -> Option<String> {
    if let Ok(u) = url::Url::parse(raw) {
        for key in ["pwd", "password", "passcode", "code"] {
            if let Some((_, value)) = u
                .query_pairs()
                .find(|(k, v)| k == key && !v.trim().is_empty())
            {
                return Some(value.trim().to_owned());
            }
        }
    }
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)(?:提取码|访问码|密码|取件码|password|pwd|code)\s*[:：=]?\s*([A-Za-z0-9_-]{3,32})").unwrap()
    });
    RE.captures(label).map(|c| c[1].to_owned())
}

// Preserve the actual HTTP/decode failure while Option-based detail parsers
// finish publishing successful siblings.
pub(crate) fn remember_failure<T>(
    result: Result<T, ProviderError>,
    first: &std::sync::Mutex<Option<ProviderError>>,
) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            first.lock().unwrap().get_or_insert(error);
            None
        }
    }
}
