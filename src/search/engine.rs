use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::Utc;
use futures::{StreamExt, stream::FuturesUnordered};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

use crate::{
    core::{
        CloudType, FilterOptions, MergedLink, ProviderError, SearchResult, filter_results,
        merge_links, merge_search_results, rank_results,
    },
    providers::{KeywordFilterMode, Provider, SearchContext},
};

use super::TelegramSource;

#[derive(Clone, Default)]
pub struct SearchOptions {
    pub query: String,
    pub providers: Vec<Arc<dyn Provider>>,
    pub channels: Vec<String>,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub clouds: Vec<CloudType>,
    pub jobs: usize,
    pub timeout: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceError {
    pub source: String,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchOutcome {
    pub query: String,
    pub total_results: usize,
    pub total_links: usize,
    pub results: Vec<SearchResult>,
    pub links_by_type: std::collections::BTreeMap<String, Vec<MergedLink>>,
    pub source_errors: Vec<SourceError>,
    pub duration_ms: u128,
    #[serde(skip)]
    pub successful_sources: usize,
}

#[derive(Clone)]
pub struct SearchEngine {
    context: SearchContext,
    telegram: TelegramSource,
}

impl SearchEngine {
    pub fn new(context: SearchContext) -> Self {
        Self {
            telegram: TelegramSource::new(context.client.clone()),
            context,
        }
    }

    pub fn with_telegram(mut self, telegram: TelegramSource) -> Self {
        self.telegram = telegram;
        self
    }

    pub async fn search(&self, options: SearchOptions) -> SearchOutcome {
        let started = Instant::now();
        let jobs = options.jobs.max(1);
        let semaphore = Arc::new(Semaphore::new(jobs));
        let mut tasks = FuturesUnordered::new();
        let priorities: HashMap<String, i32> = options
            .providers
            .iter()
            .map(|provider| (provider.meta().name.to_owned(), provider.meta().priority))
            .collect();
        let filter_modes: HashMap<String, KeywordFilterMode> = options
            .providers
            .iter()
            .map(|provider| {
                (
                    provider.meta().name.to_owned(),
                    provider.meta().keyword_filter,
                )
            })
            .collect();

        let mut source_index = 0usize;
        for provider in options.providers {
            let index = source_index;
            source_index += 1;
            let permit = semaphore.clone();
            let ctx = self.context.clone();
            let query = options.query.clone();
            let timeout = options.timeout;
            let name = provider.meta().name.to_owned();
            tasks.push(tokio::spawn(async move {
                let Ok(_permit) = permit.acquire_owned().await else {
                    return (
                        index,
                        format!("provider:{name}"),
                        Err(ProviderError::Unavailable(
                            "search concurrency controller closed".into(),
                        )),
                    );
                };
                let started = Instant::now();
                let result = tokio::time::timeout(timeout, provider.search(&ctx, &query))
                    .await
                    .map_err(|_| ProviderError::Timeout)
                    .and_then(|value| value);
                let source = format!("provider:{name}");
                tracing::debug!(
                    source = %source,
                    duration_ms = started.elapsed().as_millis(),
                    status = if result.is_ok() { "ok" } else { "error" },
                    "search source finished"
                );
                (index, source, result)
            }));
        }
        for channel in options.channels {
            let index = source_index;
            source_index += 1;
            let permit = semaphore.clone();
            let query = options.query.clone();
            let timeout = options.timeout;
            let telegram = self.telegram.clone();
            tasks.push(tokio::spawn(async move {
                let Ok(_permit) = permit.acquire_owned().await else {
                    return (
                        index,
                        format!("tg:{channel}"),
                        Err(ProviderError::Unavailable(
                            "search concurrency controller closed".into(),
                        )),
                    );
                };
                let started = Instant::now();
                let result = tokio::time::timeout(timeout, telegram.search(&channel, &query))
                    .await
                    .map_err(|_| ProviderError::Timeout)
                    .and_then(|value| value);
                let source = format!("tg:{channel}");
                tracing::debug!(
                    source = %source,
                    duration_ms = started.elapsed().as_millis(),
                    status = if result.is_ok() { "ok" } else { "error" },
                    "search source finished"
                );
                (index, source, result)
            }));
        }

        let mut batches = Vec::new();
        let mut errors = Vec::new();
        let mut successful_sources = 0;
        while let Some(joined) = tasks.next().await {
            match joined {
                Ok((index, _, Ok(results))) => {
                    successful_sources += 1;
                    batches.push((index, results));
                }
                Ok((index, source, Err(error))) => {
                    errors.push((index, source_error(source, error)))
                }
                Err(error) => errors.push((
                    usize::MAX,
                    SourceError {
                        source: "internal".into(),
                        kind: "task".into(),
                        message: error.to_string(),
                    },
                )),
            }
        }
        batches.sort_by_key(|(index, _)| *index);
        errors.sort_by_key(|(index, _)| *index);
        let errors = errors.into_iter().map(|(_, error)| error).collect();
        let mut results = merge_search_results(batches.into_iter().map(|(_, batch)| batch));
        apply_query_filter(&mut results, &options.query, &filter_modes);
        let cloud_types = options.clouds.iter().copied().collect();
        results = filter_results(
            results,
            &FilterOptions {
                include: options.include,
                exclude: options.exclude,
                cloud_types,
            },
        );
        rank_results(&mut results, Utc::now(), &priorities);
        // Providers declaring Provider-side filtering and Telegram results must
        // not be subjected to the legacy core link-title substring filter.
        // Individual Core providers already apply their query semantics.
        let links = merge_links(&results, None, &options.clouds);
        let mut links_by_type = std::collections::BTreeMap::<String, Vec<MergedLink>>::new();
        for link in links {
            links_by_type
                .entry(link.cloud_type.to_string())
                .or_default()
                .push(link);
        }
        let total_links = links_by_type.values().map(Vec::len).sum();
        SearchOutcome {
            query: options.query,
            total_results: results.len(),
            total_links,
            results,
            links_by_type,
            source_errors: errors,
            duration_ms: started.elapsed().as_millis(),
            successful_sources,
        }
    }
}

fn apply_query_filter(
    results: &mut Vec<SearchResult>,
    query: &str,
    modes: &HashMap<String, KeywordFilterMode>,
) {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return;
    }
    results.retain_mut(|result| {
        let should_filter = match &result.source {
            crate::core::Source::Telegram { .. } => true,
            crate::core::Source::Provider { name } => {
                modes.get(name).copied().unwrap_or(KeywordFilterMode::Core)
                    == KeywordFilterMode::Core
            }
        };
        if !should_filter {
            return true;
        }
        result.links.retain(|link| {
            link.work_title
                .as_deref()
                .unwrap_or(&result.title)
                .to_lowercase()
                .contains(&query)
        });
        !result.links.is_empty()
    });
}

fn source_error(source: String, error: ProviderError) -> SourceError {
    let kind = match &error {
        ProviderError::Timeout => "timeout",
        ProviderError::Network(_) => "network",
        ProviderError::Parse(_) => "parse",
        ProviderError::AuthRequired => "auth_required",
        ProviderError::RateLimited => "rate_limited",
        ProviderError::Blocked => "blocked",
        ProviderError::Protocol(_) => "protocol",
        ProviderError::Unavailable(_) => "unavailable",
    }
    .to_owned();
    SourceError {
        source,
        kind,
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::{Link, Source},
        providers::{KeywordFilterMode, ProviderMeta},
    };
    use async_trait::async_trait;

    #[derive(Debug)]
    struct FakeProvider {
        name: &'static str,
        mode: KeywordFilterMode,
        result: Result<Vec<SearchResult>, ProviderError>,
    }

    #[derive(Debug)]
    struct SlowFailure {
        name: &'static str,
        delay_ms: u64,
    }

    #[async_trait]
    impl Provider for FakeProvider {
        fn meta(&self) -> ProviderMeta {
            ProviderMeta::stateless(self.name, 2, self.mode)
        }
        async fn search(
            &self,
            _: &SearchContext,
            _: &str,
        ) -> Result<Vec<SearchResult>, ProviderError> {
            self.result.clone()
        }
    }

    #[async_trait]
    impl Provider for SlowFailure {
        fn meta(&self) -> ProviderMeta {
            ProviderMeta::stateless(self.name, 2, KeywordFilterMode::Core)
        }

        async fn search(
            &self,
            _: &SearchContext,
            _: &str,
        ) -> Result<Vec<SearchResult>, ProviderError> {
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            Err(ProviderError::Network(self.name.into()))
        }
    }

    fn result(name: &str, title: &str, url: &str) -> SearchResult {
        SearchResult {
            id: format!("{name}-1"),
            source: Source::provider(name),
            datetime: None,
            title: title.into(),
            content: String::new(),
            links: vec![Link::new(url)],
            tags: vec![],
            images: vec![],
        }
    }

    #[tokio::test]
    async fn failures_are_fail_soft_and_core_query_filter_applies() {
        let client = reqwest::Client::new();
        let engine = SearchEngine::new(SearchContext::new(client, Duration::from_secs(1)));
        let providers: Vec<Arc<dyn Provider>> = vec![
            Arc::new(FakeProvider {
                name: "ok",
                mode: KeywordFilterMode::Core,
                result: Ok(vec![result(
                    "ok",
                    "仙逆 全集",
                    "https://pan.quark.cn/s/abc",
                )]),
            }),
            Arc::new(FakeProvider {
                name: "bad",
                mode: KeywordFilterMode::Core,
                result: Err(ProviderError::Network("offline".into())),
            }),
        ];
        let outcome = engine
            .search(SearchOptions {
                query: "仙逆".into(),
                providers,
                channels: vec![],
                jobs: 2,
                timeout: Duration::from_secs(1),
                ..SearchOptions::default()
            })
            .await;
        assert_eq!(outcome.successful_sources, 1);
        assert_eq!(outcome.total_links, 1);
        assert_eq!(outcome.source_errors[0].kind, "network");
    }

    #[test]
    fn provider_side_mode_skips_core_query_filter() {
        let mut rows = vec![result("magnet", "unrelated", "magnet:?xt=urn:btih:abc")];
        let modes = HashMap::from([("magnet".into(), KeywordFilterMode::Provider)]);
        apply_query_filter(&mut rows, "仙逆", &modes);
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn source_errors_follow_requested_source_order() {
        let engine = SearchEngine::new(SearchContext::new(
            reqwest::Client::new(),
            Duration::from_secs(1),
        ));
        let outcome = engine
            .search(SearchOptions {
                query: "q".into(),
                providers: vec![
                    Arc::new(SlowFailure {
                        name: "first",
                        delay_ms: 20,
                    }),
                    Arc::new(SlowFailure {
                        name: "second",
                        delay_ms: 0,
                    }),
                ],
                jobs: 2,
                timeout: Duration::from_secs(1),
                ..SearchOptions::default()
            })
            .await;
        assert_eq!(outcome.source_errors[0].source, "provider:first");
        assert_eq!(outcome.source_errors[1].source, "provider:second");
    }
}
