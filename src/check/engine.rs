use super::{CheckCache, CheckCloudType, builtin_checkers, normalize_share_link};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckState {
    Ok,
    Bad,
    Locked,
    Unsupported,
    Uncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckItem {
    pub cloud_type: CheckCloudType,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

impl From<&crate::core::Link> for CheckItem {
    fn from(link: &crate::core::Link) -> Self {
        Self {
            cloud_type: link.cloud_type.into(),
            url: link.url.clone(),
            password: link.password.clone(),
        }
    }
}

impl CheckItem {
    pub fn detect(url: impl Into<String>) -> Self {
        let url = url.into();
        Self {
            cloud_type: super::detect_check_cloud_type(&url),
            url,
            password: None,
        }
    }

    pub fn from_link(link: &crate::core::Link) -> Self {
        link.into()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub cloud_type: CheckCloudType,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalized_url: Option<String>,
    pub state: CheckState,
    pub cache_hit: bool,
    pub checked_at_ms: i64,
    pub expires_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CheckContext {
    pub client: reqwest::Client,
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct CheckOptions {
    pub jobs: usize,
    pub timeout: Duration,
    pub refresh: bool,
    pub no_cache: bool,
    /// Already-redacted cache scope. Use `proxy_scope` to construct it.
    pub proxy_scope: Option<String>,
}
impl Default for CheckOptions {
    fn default() -> Self {
        Self {
            jobs: 8,
            timeout: Duration::from_secs(30),
            refresh: false,
            no_cache: false,
            proxy_scope: None,
        }
    }
}
impl CheckOptions {
    pub fn with_proxy_url(mut self, proxy_url: &str) -> Self {
        self.proxy_scope = (!proxy_url.trim().is_empty()).then(|| proxy_scope(proxy_url));
        self
    }
}

#[derive(Debug, Error)]
pub enum CheckError {
    #[error("network request failed: {0}")]
    Network(#[from] reqwest::Error),
    #[error("checker timed out")]
    Timeout,
    #[error("response parse failed: {0}")]
    Parse(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("check cache failed: {0}")]
    Cache(#[from] super::CacheError),
}

#[derive(Debug, Clone)]
pub struct CheckEvaluation {
    pub state: CheckState,
    pub summary: String,
}
impl CheckEvaluation {
    pub fn new(state: CheckState, summary: impl Into<String>) -> Self {
        Self {
            state,
            summary: summary.into(),
        }
    }
}

#[async_trait]
pub trait LinkChecker: Send + Sync {
    fn cloud_type(&self) -> CheckCloudType;

    /// Public core-facing API used by search/check integration.
    async fn check(
        &self,
        ctx: &CheckContext,
        link: &crate::core::Link,
    ) -> Result<CheckEvaluation, CheckError> {
        let item = CheckItem::from_link(link);
        self.check_normalized(ctx, &item, &link.url).await
    }

    #[doc(hidden)]
    async fn check_normalized(
        &self,
        ctx: &CheckContext,
        item: &CheckItem,
        normalized: &str,
    ) -> Result<CheckEvaluation, CheckError>;
}

pub struct CheckEngine {
    client: reqwest::Client,
    cache: Option<CheckCache>,
    checkers: HashMap<CheckCloudType, Arc<dyn LinkChecker>>,
}

type UniqueCheck = (CheckItem, String, Vec<(usize, String)>);

impl CheckEngine {
    pub fn new(client: reqwest::Client, cache: Option<CheckCache>) -> Self {
        Self::with_checkers(client, cache, builtin_checkers())
    }
    pub fn with_checkers(
        client: reqwest::Client,
        cache: Option<CheckCache>,
        checkers: Vec<Arc<dyn LinkChecker>>,
    ) -> Self {
        Self {
            client,
            cache,
            checkers: checkers.into_iter().map(|c| (c.cloud_type(), c)).collect(),
        }
    }

    /// Checks each unique `(type, normalized URL, proxy scope)` once, then restores input order.
    pub async fn check(&self, items: Vec<CheckItem>, options: CheckOptions) -> Vec<CheckResult> {
        if items.is_empty() {
            return Vec::new();
        }
        let mut slots = vec![None; items.len()];
        let mut unique: HashMap<String, UniqueCheck> = HashMap::new();
        for (index, item) in items.into_iter().enumerate() {
            let Some(normalized) =
                normalize_share_link(item.cloud_type, &item.url, item.password.as_deref())
            else {
                slots[index] = Some(result(
                    &item,
                    None,
                    CheckState::Uncertain,
                    false,
                    "链接格式无效",
                ));
                continue;
            };
            let key = cache_key(
                item.cloud_type,
                &normalized,
                item.password.as_deref(),
                options.proxy_scope.as_deref(),
            );
            unique
                .entry(key)
                .and_modify(|(_, _, inputs)| inputs.push((index, item.url.clone())))
                .or_insert_with(|| {
                    let original_url = item.url.clone();
                    (item, normalized, vec![(index, original_url)])
                });
        }

        let ctx = CheckContext {
            client: self.client.clone(),
            timeout: options.timeout,
        };
        let checked = stream::iter(unique.into_iter().map(|(key, (item, normalized, inputs))| {
            let ctx = ctx.clone();
            let checker = self.checkers.get(&item.cloud_type).cloned();
            let cache = self.cache.clone();
            let options = options.clone();
            async move {
                if !options.no_cache
                    && !options.refresh
                    && let Some(cache) = &cache
                {
                    match cache.get(&key) {
                        Ok(Some(mut cached)) => {
                            cached.cache_hit = true;
                            return (inputs, cached);
                        }
                        Ok(None) => {}
                        Err(error) => {
                            tracing::warn!(error = %error, "check cache lookup failed")
                        }
                    }
                }
                let evaluation = match checker {
                    None => Ok(CheckEvaluation::new(
                        CheckState::Unsupported,
                        "当前平台暂不支持检测",
                    )),
                    Some(checker) => match tokio::time::timeout(
                        options.timeout,
                        checker.check_normalized(&ctx, &item, &normalized),
                    )
                    .await
                    {
                        Ok(value) => value,
                        Err(_) => Err(CheckError::Timeout),
                    },
                };
                let (answer, cacheable) = match evaluation {
                    Ok(e) => (
                        result(&item, Some(normalized), e.state, false, e.summary),
                        true,
                    ),
                    Err(CheckError::Timeout) => (
                        result(
                            &item,
                            Some(normalized),
                            CheckState::Uncertain,
                            false,
                            "检测超时",
                        ),
                        false,
                    ),
                    Err(CheckError::Parse(_)) | Err(CheckError::Protocol(_)) => (
                        result(
                            &item,
                            Some(normalized),
                            CheckState::Uncertain,
                            false,
                            "无法确认链接状态",
                        ),
                        true,
                    ),
                    Err(CheckError::Network(_)) | Err(CheckError::Cache(_)) => (
                        result(
                            &item,
                            Some(normalized),
                            CheckState::Uncertain,
                            false,
                            "检测失败",
                        ),
                        false,
                    ),
                };
                if cacheable
                    && !options.no_cache
                    && let Some(cache) = &cache
                    && let Err(error) = cache.put(&key, &answer)
                {
                    tracing::warn!(error = %error, "check cache write failed");
                }
                (inputs, answer)
            }
        }))
        .buffer_unordered(options.jobs.max(1))
        .collect::<Vec<_>>()
        .await;
        for (inputs, answer) in checked {
            for (index, original_url) in inputs {
                let mut restored = answer.clone();
                restored.url = original_url;
                slots[index] = Some(restored);
            }
        }
        slots.into_iter().flatten().collect()
    }
}

pub fn proxy_scope(proxy_url: &str) -> String {
    format!("proxy:{:x}", Sha256::digest(proxy_url.trim().as_bytes()))
}

fn cache_key(
    cloud: CheckCloudType,
    normalized: &str,
    password: Option<&str>,
    scope: Option<&str>,
) -> String {
    let password_scope = password
        .filter(|value| !value.is_empty())
        .map(|value| format!("|password:{:x}", Sha256::digest(value.as_bytes())))
        .unwrap_or_default();
    match scope {
        Some(s) if !s.is_empty() => format!("{cloud}|{normalized}{password_scope}|{s}"),
        _ => format!("{cloud}|{normalized}{password_scope}"),
    }
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
pub fn ttl_for_state(state: CheckState) -> Duration {
    match state {
        CheckState::Ok => Duration::from_secs(86400),
        CheckState::Bad => Duration::from_secs(21600),
        CheckState::Locked => Duration::from_secs(43200),
        CheckState::Unsupported => Duration::from_secs(86400),
        CheckState::Uncertain => Duration::from_secs(1800),
    }
}
fn result(
    item: &CheckItem,
    normalized: Option<String>,
    state: CheckState,
    cache_hit: bool,
    summary: impl Into<String>,
) -> CheckResult {
    let checked = now_ms();
    CheckResult {
        cloud_type: item.cloud_type,
        url: item.url.clone(),
        normalized_url: normalized,
        state,
        cache_hit,
        checked_at_ms: checked,
        expires_at_ms: checked + ttl_for_state(state).as_millis() as i64,
        summary: Some(summary.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Fake(AtomicUsize);
    #[async_trait]
    impl LinkChecker for Fake {
        fn cloud_type(&self) -> CheckCloudType {
            CheckCloudType::Quark
        }
        async fn check_normalized(
            &self,
            _: &CheckContext,
            _: &CheckItem,
            _: &str,
        ) -> Result<CheckEvaluation, CheckError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(CheckEvaluation::new(CheckState::Ok, "ok"))
        }
    }
    #[tokio::test]
    async fn deduplicates_and_restores_order() {
        let fake = Arc::new(Fake(AtomicUsize::new(0)));
        let engine = CheckEngine::with_checkers(reqwest::Client::new(), None, vec![fake.clone()]);
        let a = CheckItem {
            cloud_type: CheckCloudType::Quark,
            url: "https://pan.quark.cn/s/a".into(),
            password: None,
        };
        let b = CheckItem {
            cloud_type: CheckCloudType::Quark,
            url: "https://pan.quark.cn/s/b".into(),
            password: None,
        };
        let out = engine
            .check(vec![a.clone(), b, a], CheckOptions::default())
            .await;
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].normalized_url, out[2].normalized_url);
        assert_eq!(fake.0.load(Ordering::SeqCst), 2);
    }
    #[tokio::test]
    async fn cache_refresh_and_no_cache_have_exact_semantics() {
        let fake = Arc::new(Fake(AtomicUsize::new(0)));
        let dir = tempfile::tempdir().unwrap();
        let cache = CheckCache::open(dir.path().join("check.redb")).unwrap();
        let engine =
            CheckEngine::with_checkers(reqwest::Client::new(), Some(cache), vec![fake.clone()]);
        let item = CheckItem {
            cloud_type: CheckCloudType::Quark,
            url: "https://pan.quark.cn/s/a".into(),
            password: None,
        };
        assert!(
            !engine
                .check(vec![item.clone()], CheckOptions::default())
                .await[0]
                .cache_hit
        );
        assert!(
            engine
                .check(vec![item.clone()], CheckOptions::default())
                .await[0]
                .cache_hit
        );
        assert_eq!(fake.0.load(Ordering::SeqCst), 1);
        let refresh = CheckOptions {
            refresh: true,
            ..CheckOptions::default()
        };
        engine.check(vec![item.clone()], refresh).await;
        assert_eq!(fake.0.load(Ordering::SeqCst), 2);
        let no_cache = CheckOptions {
            no_cache: true,
            ..CheckOptions::default()
        };
        engine.check(vec![item.clone()], no_cache.clone()).await;
        engine.check(vec![item], no_cache).await;
        assert_eq!(fake.0.load(Ordering::SeqCst), 4);
    }
    #[test]
    fn keeps_go_ttls() {
        assert_eq!(ttl_for_state(CheckState::Ok), Duration::from_secs(86400));
        assert_eq!(
            ttl_for_state(CheckState::Uncertain),
            Duration::from_secs(1800)
        );
    }
    #[test]
    fn proxy_is_not_exposed() {
        let scope = proxy_scope("http://user:secret@localhost:1");
        assert!(!scope.contains("secret"));
    }

    #[test]
    fn password_sensitive_cache_keys_are_distinct_and_redacted() {
        let first = cache_key(
            CheckCloudType::One15,
            "https://115.com/s/example",
            Some("a1b2"),
            None,
        );
        let second = cache_key(
            CheckCloudType::One15,
            "https://115.com/s/example",
            Some("c3d4"),
            None,
        );
        assert_ne!(first, second);
        assert!(!first.contains("a1b2"));
    }
}
