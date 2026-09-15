use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use chrono::Utc;
use futures::{FutureExt, StreamExt, stream::FuturesUnordered};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};

use crate::{
    core::{
        CloudType, FilterOptions, MergedLink, ProviderError, SearchResult, filter_results,
        merge_links, merge_search_results, rank_results,
    },
    providers::{KeywordFilterMode, Provider, SearchContext},
};

use super::TelegramSource;

#[derive(Clone)]
pub struct SearchOptions {
    pub query: String,
    pub providers: Vec<Arc<dyn Provider>>,
    pub channels: Vec<String>,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub clouds: Vec<CloudType>,
    pub jobs: usize,
    pub timeout: Duration,
    pub all_timeout: Duration,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            query: String::new(),
            providers: vec![],
            channels: vec![],
            include: vec![],
            exclude: vec![],
            clouds: vec![],
            jobs: 8,
            timeout: Duration::from_secs(30),
            all_timeout: Duration::from_secs(600),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Completed,
    Deadline,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchSummary {
    pub total_links: usize,
    pub sources_selected: usize,
    pub sources_completed: usize,
    pub sources_failed: usize,
    pub sources_cancelled: usize,
    pub sources_not_started: usize,
    pub partial: bool,
    pub finish_reason: FinishReason,
    pub search_duration_ms: u128,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum SearchEvent {
    Result {
        id: String,
        link: MergedLink,
    },
    ResultUpdate {
        id: String,
        link: MergedLink,
    },
    SourceError {
        error: SourceError,
    },
    Progress {
        completed: usize,
        selected: usize,
        links: usize,
    },
    Summary {
        summary: SearchSummary,
    },
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

    /// Stream source completions. Dropping the owned source futures cancels their
    /// requests; no detached search tasks survive a deadline or interruption.
    pub async fn search_stream(
        &self,
        options: SearchOptions,
        sender: mpsc::UnboundedSender<SearchEvent>,
        cancellation: watch::Receiver<bool>,
    ) -> SearchSummary {
        self.run(options, Some(sender), cancellation).await.1
    }

    pub async fn search(&self, options: SearchOptions) -> SearchOutcome {
        let (_cancel, cancellation) = watch::channel(false);
        self.run(options, None, cancellation).await.0
    }

    async fn run(
        &self,
        options: SearchOptions,
        sender: Option<mpsc::UnboundedSender<SearchEvent>>,
        mut cancellation: watch::Receiver<bool>,
    ) -> (SearchOutcome, SearchSummary) {
        let started = Instant::now();
        let deadline = tokio::time::sleep(options.all_timeout);
        tokio::pin!(deadline);
        let priorities: HashMap<String, i32> = options
            .providers
            .iter()
            .map(|p| (p.meta().name.to_owned(), p.meta().priority))
            .collect();
        let modes: HashMap<String, KeywordFilterMode> = options
            .providers
            .iter()
            .map(|p| (p.meta().name.to_owned(), p.meta().keyword_filter))
            .collect();
        let mut queue = VecDeque::new();
        let sources_started = Arc::new(AtomicUsize::new(0));
        for provider in &options.providers {
            let index = queue.len();
            let provider = provider.clone();
            let ctx = self.context.clone();
            let query = options.query.clone();
            let timeout = options.timeout;
            let sources_started = sources_started.clone();
            queue.push_back(
                async move {
                    sources_started.fetch_add(1, Ordering::Relaxed);
                    let source = format!("provider:{}", provider.meta().name);
                    let result = tokio::time::timeout(timeout, provider.search(&ctx, &query))
                        .await
                        .map_err(|_| ProviderError::Timeout)
                        .and_then(|value| value);
                    (index, source, result)
                }
                .boxed(),
            );
        }
        for channel in &options.channels {
            let index = queue.len();
            let channel = channel.clone();
            let telegram = self.telegram.clone();
            let query = options.query.clone();
            let timeout = options.timeout;
            let sources_started = sources_started.clone();
            queue.push_back(
                async move {
                    sources_started.fetch_add(1, Ordering::Relaxed);
                    let result = tokio::time::timeout(timeout, telegram.search(&channel, &query))
                        .await
                        .map_err(|_| ProviderError::Timeout)
                        .and_then(|value| value);
                    (index, format!("tg:{channel}"), result)
                }
                .boxed(),
            );
        }
        let selected = queue.len();
        let mut tasks = FuturesUnordered::new();
        let mut batches = Vec::new();
        let mut errors = Vec::new();
        let mut emitted = HashMap::<String, MergedLink>::new();
        let mut successful_sources = 0;
        let mut reason = FinishReason::Completed;
        let emit = |event| {
            if let Some(sender) = &sender {
                let _ = sender.send(event);
            }
        };
        loop {
            if *cancellation.borrow() || sender.as_ref().is_some_and(|s| s.is_closed()) {
                reason = FinishReason::Interrupted;
                break;
            }
            while tasks.len() < options.jobs.max(1) {
                match queue.pop_front() {
                    Some(task) => tasks.push(task),
                    None => break,
                }
            }
            if tasks.is_empty() {
                break;
            }
            let finished = tokio::select! {
                biased;
                _ = &mut deadline => {
                    reason = FinishReason::Deadline;
                    break;
                }
                _ = async {
                    loop {
                        if *cancellation.borrow() { break; }
                        if cancellation.changed().await.is_err() {
                            futures::future::pending::<()>().await;
                        }
                    }
                } => {
                    reason = FinishReason::Interrupted;
                    break;
                }
                value = tasks.next() => value,
            };
            let Some((index, source, result)) = finished else {
                break;
            };
            match result {
                Ok(mut batch) => {
                    successful_sources += 1;
                    apply_query_filter(&mut batch, &options.query, &modes);
                    batch = filter_results(
                        batch,
                        &FilterOptions {
                            include: options.include.clone(),
                            exclude: options.exclude.clone(),
                            cloud_types: options.clouds.iter().copied().collect(),
                        },
                    );
                    rank_results(&mut batch, Utc::now(), &priorities);
                    batches.push((index, batch));
                    // Preserve source completion order for stream presentation.
                    let rows = merge_search_results(batches.iter().map(|(_, batch)| batch.clone()));
                    for mut link in merge_links(&rows, None, &options.clouds) {
                        let id = crate::core::canonical_url_key(&link.url);
                        // A newer duplicate must not erase an already known password.
                        if let Some(previous) = emitted.get(&id) {
                            if link.password.is_none() {
                                link.password = previous.password.clone();
                            }
                            if previous == &link {
                                continue;
                            }
                            emit(SearchEvent::ResultUpdate {
                                id: id.clone(),
                                link: link.clone(),
                            });
                        } else {
                            emit(SearchEvent::Result {
                                id: id.clone(),
                                link: link.clone(),
                            });
                        }
                        emitted.insert(id, link);
                    }
                }
                Err(error) => {
                    let error = source_error(source, error);
                    emit(SearchEvent::SourceError {
                        error: error.clone(),
                    });
                    errors.push((index, error));
                }
            }
            emit(SearchEvent::Progress {
                completed: successful_sources + errors.len(),
                selected,
                links: emitted.len(),
            });
        }
        let sources_started = sources_started.load(Ordering::Relaxed);
        let cancelled = sources_started - successful_sources - errors.len();
        let not_started = selected - sources_started;
        drop(tasks);
        drop(queue);
        let elapsed = started.elapsed().as_millis();
        let summary = SearchSummary {
            total_links: emitted.len(),
            sources_selected: selected,
            sources_completed: successful_sources,
            sources_failed: errors.len(),
            sources_cancelled: cancelled,
            sources_not_started: not_started,
            partial: successful_sources < selected,
            finish_reason: reason,
            search_duration_ms: elapsed,
            duration_ms: elapsed,
        };
        emit(SearchEvent::Summary {
            summary: summary.clone(),
        });
        batches.sort_by_key(|(index, _)| *index);
        errors.sort_by_key(|(index, _)| *index);
        let mut results = merge_search_results(batches.into_iter().map(|(_, batch)| batch));
        rank_results(&mut results, Utc::now(), &priorities);
        let mut links_by_type = std::collections::BTreeMap::<String, Vec<MergedLink>>::new();
        for link in merge_links(&results, None, &options.clouds) {
            links_by_type
                .entry(link.cloud_type.to_string())
                .or_default()
                .push(link);
        }
        let outcome = SearchOutcome {
            query: options.query,
            total_results: results.len(),
            total_links: summary.total_links,
            results,
            links_by_type,
            source_errors: errors.into_iter().map(|(_, error)| error).collect(),
            duration_ms: elapsed,
            successful_sources,
        };
        (outcome, summary)
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
        if matches!(result.source, crate::core::Source::Telegram { .. })
            && result.content.to_lowercase().contains(&query)
        {
            return !result.links.is_empty();
        }
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
        ProviderError::InvalidSource(_) => "invalid_source",
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

    #[test]
    fn telegram_body_match_preserves_links_but_provider_body_does_not() {
        let mut tg = result("tg", "unrelated title", "https://pan.quark.cn/s/abc");
        tg.source = Source::Telegram {
            channel: "fixture".into(),
        };
        tg.content = "A MOVIE appears in this message".into();
        let mut provider = result("provider", "unrelated title", "https://pan.quark.cn/s/def");
        provider.content = tg.content.clone();
        let mut rows = vec![tg, provider];
        apply_query_filter(&mut rows, " movie ", &HashMap::new());
        assert_eq!(rows.len(), 1);
        assert!(matches!(rows[0].source, Source::Telegram { .. }));
    }

    struct ActiveGuard(Arc<std::sync::atomic::AtomicUsize>);
    impl Drop for ActiveGuard {
        fn drop(&mut self) {
            self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[derive(Debug)]
    struct TimedProvider {
        name: &'static str,
        delay: Duration,
        active: Arc<std::sync::atomic::AtomicUsize>,
        peak: Arc<std::sync::atomic::AtomicUsize>,
        password: Option<&'static str>,
    }
    #[async_trait]
    impl Provider for TimedProvider {
        fn meta(&self) -> ProviderMeta {
            ProviderMeta::stateless(self.name, 2, KeywordFilterMode::Provider)
        }
        async fn search(
            &self,
            _: &SearchContext,
            _: &str,
        ) -> Result<Vec<SearchResult>, ProviderError> {
            use std::sync::atomic::Ordering::SeqCst;
            let active = self.active.fetch_add(1, SeqCst) + 1;
            self.peak.fetch_max(active, SeqCst);
            let _guard = ActiveGuard(self.active.clone());
            tokio::time::sleep(self.delay).await;
            let mut row = result(self.name, "Movie", "https://pan.quark.cn/s/abc");
            row.links[0].password = self.password.map(str::to_owned);
            Ok(vec![row])
        }
    }

    fn timed(name: &'static str, seconds: u64) -> TimedProvider {
        TimedProvider {
            name,
            delay: Duration::from_secs(seconds),
            active: Arc::default(),
            peak: Arc::default(),
            password: None,
        }
    }

    fn test_engine() -> SearchEngine {
        SearchEngine::new(SearchContext::new(
            reqwest::Client::new(),
            Duration::from_secs(30),
        ))
    }

    #[tokio::test(start_paused = true)]
    async fn streams_before_last_source_and_updates_duplicate_password() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (_cancel, cancelled) = watch::channel(false);
        let mut second = timed("second", 10);
        second.password = Some("1234");
        let handle = tokio::spawn(async move {
            test_engine()
                .search_stream(
                    SearchOptions {
                        providers: vec![Arc::new(timed("first", 1)), Arc::new(second)],
                        ..Default::default()
                    },
                    tx,
                    cancelled,
                )
                .await
        });
        let SearchEvent::Result { id, .. } = rx.recv().await.unwrap() else {
            panic!("first event is a result");
        };
        assert!(!handle.is_finished());
        let mut updates = 0;
        while let Some(event) = rx.recv().await {
            if let SearchEvent::ResultUpdate {
                id: update_id,
                link,
            } = event
            {
                assert_eq!(id, update_id);
                assert_eq!(link.password.as_deref(), Some("1234"));
                updates += 1;
            }
        }
        let summary = handle.await.unwrap();
        assert_eq!(updates, 1);
        assert_eq!(summary.total_links, 1);
        assert_eq!(summary.sources_completed, 2);
        assert!(!summary.partial);
    }

    #[tokio::test(start_paused = true)]
    async fn deadline_cancels_actual_requests_and_counts_queued_sources() {
        use std::sync::atomic::Ordering::SeqCst;
        let first = timed("first", 100);
        let active = first.active.clone();
        let (tx, _rx) = mpsc::unbounded_channel();
        let (_cancel, cancelled) = watch::channel(false);
        let summary = test_engine()
            .search_stream(
                SearchOptions {
                    providers: vec![Arc::new(first), Arc::new(timed("second", 100))],
                    jobs: 1,
                    timeout: Duration::from_secs(100),
                    all_timeout: Duration::from_secs(5),
                    ..Default::default()
                },
                tx,
                cancelled,
            )
            .await;
        assert_eq!(summary.finish_reason, FinishReason::Deadline);
        assert_eq!(summary.sources_cancelled, 1);
        assert_eq!(summary.sources_not_started, 1);
        assert_eq!(active.load(SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn default_concurrency_is_eight_and_queue_time_is_not_source_timeout() {
        use std::sync::atomic::Ordering::SeqCst;
        let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let providers = (0..17)
            .map(|_| {
                Arc::new(TimedProvider {
                    active: active.clone(),
                    peak: peak.clone(),
                    ..timed("source", 5)
                }) as Arc<dyn Provider>
            })
            .collect();
        let (tx, _rx) = mpsc::unbounded_channel();
        let (_cancel, cancelled) = watch::channel(false);
        let summary = test_engine()
            .search_stream(
                SearchOptions {
                    providers,
                    timeout: Duration::from_secs(6),
                    ..Default::default()
                },
                tx,
                cancelled,
            )
            .await;
        assert_eq!(peak.load(SeqCst), 8);
        assert_eq!(summary.sources_completed, 17);
        assert_eq!(summary.sources_failed, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn explicit_cancellation_preserves_completed_results() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (cancel, cancelled) = watch::channel(false);
        let handle = tokio::spawn(async move {
            test_engine()
                .search_stream(
                    SearchOptions {
                        providers: vec![Arc::new(timed("fast", 1)), Arc::new(timed("slow", 100))],
                        ..Default::default()
                    },
                    tx,
                    cancelled,
                )
                .await
        });
        assert!(matches!(rx.recv().await, Some(SearchEvent::Result { .. })));
        cancel.send(true).unwrap();
        let summary = handle.await.unwrap();
        assert_eq!(summary.finish_reason, FinishReason::Interrupted);
        assert_eq!(summary.total_links, 1);
        assert_eq!(summary.sources_completed, 1);
        assert_eq!(summary.sources_cancelled, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn source_timeout_releases_slot_and_keeps_searching() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (_cancel, cancelled) = watch::channel(false);
        let summary = test_engine()
            .search_stream(
                SearchOptions {
                    providers: vec![Arc::new(timed("slow", 100)), Arc::new(timed("fast", 1))],
                    jobs: 1,
                    timeout: Duration::from_secs(2),
                    ..Default::default()
                },
                tx,
                cancelled,
            )
            .await;
        assert_eq!(summary.finish_reason, FinishReason::Completed);
        assert_eq!(summary.sources_failed, 1);
        assert_eq!(summary.sources_completed, 1);
        assert_eq!(summary.sources_cancelled, 0);
        assert!(summary.partial);
        assert!(
            matches!(rx.recv().await, Some(SearchEvent::SourceError { error }) if error.kind == "timeout")
        );
    }
}
