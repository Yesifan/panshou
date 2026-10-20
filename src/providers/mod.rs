//! Search provider contracts and the explicit built-in registry.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use reqwest::Client;

use crate::core::{ProviderError, SearchResult};

pub mod a_registry;
pub mod b_registry;
pub mod c_registry;
pub mod c_support;
pub mod cldi;
pub mod clmao;
pub mod clxiong;
pub mod cyg;
pub mod d_registry;
pub mod djgou;
pub mod duanjuw;
pub mod dyyj;
pub mod e_registry;
pub mod f_registry;
pub mod g_registry;
pub mod gying;
pub mod h_registry;
pub mod hdmoli;
pub mod jsnoteclub;
pub mod jupansou;
pub mod meitizy;
pub mod panlian;
pub mod pansearch;
pub mod qqpd;
pub mod susu;
pub mod weibo;
pub mod woniu;
pub mod yulinshufa;
pub mod yunsou;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeywordFilterMode {
    Core,
    Provider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    None,
    Qr,
    Password,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderMeta {
    pub name: &'static str,
    pub priority: i32,
    pub requires_auth: bool,
    pub auth_kind: AuthKind,
    pub keyword_filter: KeywordFilterMode,
}

impl ProviderMeta {
    pub const fn stateless(
        name: &'static str,
        priority: i32,
        keyword_filter: KeywordFilterMode,
    ) -> Self {
        Self {
            name,
            priority,
            requires_auth: false,
            auth_kind: AuthKind::None,
            keyword_filter,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchContext {
    pub client: Client,
    pub timeout: Duration,
    deadline: Option<tokio::time::Instant>,
    collector: Arc<Mutex<SourceCollector>>,
}

#[derive(Debug, Default)]
struct SourceCollector {
    sealed: bool,
    results: Vec<SearchResult>,
}

impl SearchContext {
    pub fn new(client: Client, timeout: Duration) -> Self {
        Self {
            client,
            timeout,
            deadline: None,
            collector: Arc::default(),
        }
    }

    /// Start an isolated source budget. Call only after acquiring a source slot.
    pub fn for_source(&self, deadline: tokio::time::Instant) -> Self {
        Self {
            client: self.client.clone(),
            timeout: deadline.saturating_duration_since(tokio::time::Instant::now()),
            deadline: Some(deadline),
            collector: Arc::default(),
        }
    }

    pub fn deadline(&self) -> Option<tokio::time::Instant> {
        self.deadline
    }

    pub fn remaining(&self) -> Duration {
        self.deadline.map_or(self.timeout, |deadline| {
            deadline.saturating_duration_since(tokio::time::Instant::now())
        })
    }

    pub fn request_timeout(&self, maximum: Duration) -> Duration {
        self.remaining().min(maximum)
    }

    pub fn has_results(&self) -> bool {
        !self.collector.lock().unwrap().results.is_empty()
    }

    pub fn is_cancelled(&self) -> bool {
        self.remaining().is_zero() || self.collector.lock().unwrap().sealed
    }

    /// Deliver fully parsed work before a later request can fail or be dropped.
    /// Returns false once the source deadline or immutable snapshot is reached.
    pub fn publish(&self, results: Vec<SearchResult>) -> bool {
        let mut collector = self.collector.lock().unwrap();
        if collector.sealed || self.remaining().is_zero() {
            return false;
        }
        collector.results.extend(results);
        true
    }

    /// Freeze delivery before dropping source futures. Subsequent writes fail.
    pub(crate) fn finish(&self, results: Vec<SearchResult>) -> Vec<SearchResult> {
        let mut collector = self.collector.lock().unwrap();
        if !collector.sealed {
            collector.results.extend(results);
            collector.sealed = true;
        }
        let mut rows = Vec::<SearchResult>::new();
        for row in &collector.results {
            if let Some(previous) = rows.iter_mut().find(|previous| {
                previous.source == row.source
                    && crate::core::merge::result_key(previous)
                        == crate::core::merge::result_key(row)
            }) {
                if !row.title.is_empty() {
                    previous.title = row.title.clone();
                }
                if !row.content.is_empty() {
                    previous.content = row.content.clone();
                }
                previous.datetime = row.datetime.or(previous.datetime);
                for tag in &row.tags {
                    if !previous.tags.contains(tag) {
                        previous.tags.push(tag.clone());
                    }
                }
                for image in &row.images {
                    if !previous.images.contains(image) {
                        previous.images.push(image.clone());
                    }
                }
                for link in &row.links {
                    let key = crate::core::canonical_url_key(&link.url);
                    if let Some(old) = previous
                        .links
                        .iter_mut()
                        .find(|old| crate::core::canonical_url_key(&old.url) == key)
                    {
                        let mut updated = link.clone();
                        updated.password = updated
                            .password
                            .filter(|v| !v.is_empty())
                            .or_else(|| old.password.clone());
                        updated.work_title = updated
                            .work_title
                            .filter(|v| !v.is_empty())
                            .or_else(|| old.work_title.clone());
                        updated.datetime = updated.datetime.or(old.datetime);
                        *old = updated;
                    } else {
                        previous.links.push(link.clone());
                    }
                }
            } else {
                rows.push(row.clone());
            }
        }
        rows
    }
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn meta(&self) -> ProviderMeta;

    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError>;
}

/// Bound detail fan-out while publishing each completed item immediately,
/// even when final source ordering waits for an earlier slow item.
pub(crate) async fn collect_partial<I, F>(
    ctx: &SearchContext,
    jobs: I,
    concurrency: usize,
) -> Result<Vec<SearchResult>, ProviderError>
where
    I: IntoIterator<Item = F>,
    F: std::future::Future<Output = Result<Option<SearchResult>, ProviderError>>,
{
    use futures::StreamExt;
    let jobs: Vec<_> = jobs.into_iter().collect();
    let mut rows = futures::stream::iter(jobs.into_iter().enumerate())
        .map(|(index, job)| async move {
            let row = job.await;
            if let Ok(Some(row)) = &row {
                ctx.publish(vec![row.clone()]);
            }
            (index, row)
        })
        .buffer_unordered(concurrency)
        .collect::<Vec<_>>()
        .await;
    rows.sort_by_key(|(index, _)| *index);
    let mut results = Vec::new();
    let mut first_error = None;
    for (_, row) in rows {
        match row {
            Ok(Some(row)) => results.push(row),
            Ok(None) => {
                first_error.get_or_insert_with(|| {
                    ProviderError::Unavailable("detail contained no usable links".into())
                });
            }
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(results),
    }
}

pub struct ProviderRegistration {
    pub provider: Arc<dyn Provider>,
    pub ready_profiles: usize,
}

impl ProviderRegistration {
    pub fn is_ready(&self) -> bool {
        !self.provider.meta().requires_auth || self.ready_profiles > 0
    }
}

pub fn builtin_stateless_providers() -> Vec<Arc<dyn Provider>> {
    builtin_stateless_providers_with_options(&crate::http::ClientOptions::default())
}

pub fn builtin_stateless_providers_with_options(
    options: &crate::http::ClientOptions,
) -> Vec<Arc<dyn Provider>> {
    let mut providers: Vec<Arc<dyn Provider>> = vec![
        Arc::new(pansearch::Pansearch::default()),
        Arc::new(yunsou::YunsouProvider::default()),
        Arc::new(djgou::Djgou::default()),
        Arc::new(hdmoli::HdmoliProvider::default()),
        Arc::new(meitizy::MeitizyProvider::default()),
        Arc::new(yulinshufa::Yulinshufa::default()),
        Arc::new(clxiong::ClxiongProvider::default()),
        Arc::new(jsnoteclub::Jsnoteclub::default()),
        Arc::new(duanjuw::DuanjuwProvider::default()),
        Arc::new(dyyj::DyyjProvider::default()),
        Arc::new(jupansou::Jupansou::default()),
        Arc::new(cldi::CldiProvider::default()),
        Arc::new(clmao::Clmao::default()),
        Arc::new(cyg::CygProvider::default()),
        Arc::new(susu::Susu::default()),
    ];
    providers.extend(a_registry::providers());
    providers.extend(b_registry::providers());
    providers.extend(c_registry::providers_with_options(options));
    providers.extend(d_registry::providers_with_options(options));
    providers.extend(e_registry::providers());
    providers.extend(f_registry::providers_with_options(options));
    providers.extend(g_registry::providers());
    providers.extend(h_registry::providers_with_options(options));
    providers
}

pub fn find_stateless(name: &str) -> Option<Arc<dyn Provider>> {
    builtin_stateless_providers()
        .into_iter()
        .find(|provider| provider.meta().name == name)
}

#[cfg(test)]
mod partial_detail_tests {
    use super::*;
    use crate::core::{Link, Source};

    #[tokio::test]
    async fn detail_collection_retains_the_actual_error_and_successful_snapshot() {
        let ctx = SearchContext::new(Client::new(), Duration::from_secs(2));
        let mut row = SearchResult::new("fast", Source::provider("demo"));
        row.links.push(Link::new("https://pan.quark.cn/s/fast"));
        let error = ProviderError::Protocol("HTTP 503".into());
        let jobs = [
            futures::future::ready(Ok(Some(row))),
            futures::future::ready(Err(error.clone())),
        ];
        assert_eq!(collect_partial(&ctx, jobs, 2).await.unwrap_err(), error);
        assert_eq!(ctx.finish(vec![]).len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn ordered_detail_collection_publishes_fast_items_before_a_slow_first_item() {
        let ctx = SearchContext::new(Client::new(), Duration::from_secs(3))
            .for_source(tokio::time::Instant::now() + Duration::from_secs(3));
        let jobs = [("slow", 100), ("fast", 1), ("fast2", 1)].into_iter().map(
            |(name, seconds)| async move {
                tokio::time::sleep(Duration::from_secs(seconds)).await;
                Ok(Some(SearchResult {
                    id: name.into(),
                    source: Source::provider("demo"),
                    datetime: None,
                    title: "movie".into(),
                    content: String::new(),
                    links: vec![Link::new(format!("https://pan.quark.cn/s/{name}"))],
                    tags: vec![],
                    images: vec![],
                }))
            },
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(3), collect_partial(&ctx, jobs, 2))
                .await
                .is_err()
        );
        let snapshot = ctx.finish(vec![]);
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].id, "fast");
        assert_eq!(snapshot[1].id, "fast2");
        assert!(!ctx.publish(snapshot));
    }
}
