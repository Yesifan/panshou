use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
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

impl SearchSummary {
    pub fn exit_code(&self) -> i32 {
        match self.finish_reason {
            FinishReason::Deadline => 124,
            FinishReason::Interrupted => 130,
            FinishReason::Completed if self.sources_completed == 0 => 3,
            FinishReason::Completed => 0,
        }
    }
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

/// Timeout futures cannot preempt a synchronous poll that returns after its deadline.
fn reject_late_source_result<T>(
    deadline: tokio::time::Instant,
    result: Result<T, ProviderError>,
) -> Result<T, ProviderError> {
    if tokio::time::Instant::now() >= deadline {
        Err(ProviderError::Timeout)
    } else {
        result
    }
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
    ) -> (SearchOutcome, SearchSummary) {
        self.run(options, Some(sender), cancellation).await
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
        let overall_deadline = tokio::time::Instant::now() + options.all_timeout;
        let collectors = Arc::new(Mutex::new(HashMap::<usize, SearchContext>::new()));
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
            let collectors = collectors.clone();
            queue.push_back(
                async move {
                    sources_started.fetch_add(1, Ordering::Relaxed);
                    let ctx = ctx
                        .for_source((tokio::time::Instant::now() + timeout).min(overall_deadline));
                    collectors.lock().unwrap().insert(index, ctx.clone());
                    let source = format!("provider:{}", provider.meta().name);
                    let source_deadline = ctx.deadline().unwrap();
                    let result = tokio::time::timeout_at(
                        source_deadline,
                        std::panic::AssertUnwindSafe(provider.search(&ctx, &query)).catch_unwind(),
                    )
                    .await
                    .map_err(|_| ProviderError::Timeout)
                    .and_then(|value| {
                        value.map_err(|_| ProviderError::Unavailable("provider panicked".into()))
                    })
                    .and_then(|value| value);
                    // A synchronous parser can finish in the same poll after
                    // its timer elapsed; never accept that late final batch.
                    let result = reject_late_source_result(source_deadline, result);
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
                    let source_deadline =
                        (tokio::time::Instant::now() + timeout).min(overall_deadline);
                    let result = tokio::time::timeout_at(
                        source_deadline,
                        std::panic::AssertUnwindSafe(telegram.search(&channel, &query))
                            .catch_unwind(),
                    )
                    .await
                    .map_err(|_| ProviderError::Timeout)
                    .and_then(|value| {
                        value.map_err(|_| {
                            ProviderError::Unavailable("Telegram source panicked".into())
                        })
                    })
                    .and_then(|value| value);
                    let result = reject_late_source_result(source_deadline, result);
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
                _ = async {
                    match &sender {
                        Some(sender) => sender.closed().await,
                        None => futures::future::pending::<()>().await,
                    }
                } => {
                    reason = FinishReason::Interrupted;
                    break;
                }
                value = tasks.next() => value,
            };
            // select! cannot preempt a long synchronous poll. Recheck the
            // command-wide terminal conditions before accepting its outcome.
            if *cancellation.borrow() || sender.as_ref().is_some_and(|s| s.is_closed()) {
                reason = FinishReason::Interrupted;
                break;
            }
            if tokio::time::Instant::now() >= overall_deadline {
                reason = FinishReason::Deadline;
                break;
            }
            let Some((index, source, result)) = finished else {
                break;
            };
            let batch = match &result {
                Ok(batch) => batch.clone(),
                Err(_) => Vec::new(),
            };
            let batch = collectors
                .lock()
                .unwrap()
                .remove(&index)
                .map_or(batch.clone(), |ctx| ctx.finish(batch));
            match result {
                Ok(_) => successful_sources += 1,
                Err(error) => {
                    let error = source_error(source, error);
                    emit(SearchEvent::SourceError {
                        error: error.clone(),
                    });
                    errors.push((index, error));
                }
            }
            {
                let mut batch = batch;
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
            emit(SearchEvent::Progress {
                completed: successful_sources + errors.len(),
                selected,
                links: emitted.len(),
            });
        }
        let sources_started = sources_started.load(Ordering::Relaxed);
        let cancelled = sources_started - successful_sources - errors.len();
        let not_started = selected - sources_started;
        // Seal before dropping futures: retained clones cannot alter the snapshot.
        let mut snapshots: Vec<_> = collectors.lock().unwrap().drain().collect();
        snapshots.sort_by_key(|(index, _)| *index);
        for (index, ctx) in snapshots {
            let mut batch = ctx.finish(Vec::new());
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
        }
        drop(tasks);
        drop(queue);
        batches.sort_by_key(|(index, _)| *index);
        let rows = merge_search_results(batches.iter().map(|(_, batch)| batch.clone()));
        for mut link in merge_links(&rows, None, &options.clouds) {
            let id = crate::core::canonical_url_key(&link.url);
            if let Some(previous) = emitted.get(&id) {
                if link.password.is_none() {
                    link.password = previous.password.clone();
                }
                if previous == &link {
                    continue;
                }
            }
            let event = if emitted.contains_key(&id) {
                SearchEvent::ResultUpdate {
                    id: id.clone(),
                    link: link.clone(),
                }
            } else {
                SearchEvent::Result {
                    id: id.clone(),
                    link: link.clone(),
                }
            };
            emit(event);
            emitted.insert(id, link);
        }
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

    struct PanickingProvider;
    #[async_trait]
    impl Provider for PanickingProvider {
        fn meta(&self) -> ProviderMeta {
            ProviderMeta::stateless("panic", 2, KeywordFilterMode::Core)
        }
        async fn search(
            &self,
            ctx: &SearchContext,
            _: &str,
        ) -> Result<Vec<SearchResult>, ProviderError> {
            ctx.publish(vec![result(
                "panic",
                "movie early",
                "https://pan.quark.cn/s/early",
            )]);
            panic!("fixture provider panic")
        }
    }

    #[tokio::test]
    async fn provider_panic_preserves_its_snapshot_without_aborting_other_sources() {
        let engine = SearchEngine::new(SearchContext::new(
            reqwest::Client::new(),
            Duration::from_secs(1),
        ));
        let outcome = engine
            .search(SearchOptions {
                query: "movie".into(),
                providers: vec![
                    Arc::new(PanickingProvider),
                    Arc::new(FakeProvider {
                        name: "other",
                        mode: KeywordFilterMode::Core,
                        result: Ok(vec![result(
                            "other",
                            "movie complete",
                            "https://pan.quark.cn/s/other",
                        )]),
                    }),
                ],
                ..SearchOptions::default()
            })
            .await;
        assert_eq!(outcome.successful_sources, 1);
        assert_eq!(outcome.total_links, 2);
        assert_eq!(outcome.source_errors.len(), 1);
        assert_eq!(
            outcome.source_errors[0].message,
            "provider is unavailable: provider panicked"
        );
    }

    #[tokio::test]
    async fn shared_deadline_check_rejects_a_synchronously_ready_late_batch() {
        // Tokio polls the inner future first, even with an already elapsed timer.
        // Model a parser's ready response arriving after the absolute deadline
        // without wall-clock sleeps or scheduler races.
        let deadline = tokio::time::Instant::now() - Duration::from_secs(1);
        let result = tokio::time::timeout_at(
            deadline,
            std::future::ready(Ok(vec![result(
                "late",
                "movie late",
                "https://pan.quark.cn/s/late",
            )])),
        )
        .await
        .expect("a synchronously ready future wins timeout_at's first poll");
        assert!(matches!(
            reject_late_source_result(deadline, result),
            Err(ProviderError::Timeout)
        ));
        assert!(
            reject_late_source_result(
                tokio::time::Instant::now() + Duration::from_secs(1),
                Ok::<_, ProviderError>(Vec::<SearchResult>::new()),
            )
            .is_ok()
        );
    }

    struct BlockingParser;
    #[async_trait]
    impl Provider for BlockingParser {
        fn meta(&self) -> ProviderMeta {
            ProviderMeta::stateless("blocking", 2, KeywordFilterMode::Core)
        }
        async fn search(
            &self,
            ctx: &SearchContext,
            _: &str,
        ) -> Result<Vec<SearchResult>, ProviderError> {
            ctx.publish(vec![result(
                "blocking",
                "movie early",
                "https://pan.quark.cn/s/early",
            )]);
            // Simulate one parser poll that cannot be preempted by timeout_at.
            std::thread::sleep(Duration::from_millis(20));
            Ok(vec![result(
                "late",
                "movie late",
                "https://pan.quark.cn/s/late",
            )])
        }
    }

    #[tokio::test]
    async fn a_late_synchronous_final_batch_cannot_bypass_the_source_deadline() {
        let engine = SearchEngine::new(SearchContext::new(
            reqwest::Client::new(),
            Duration::from_secs(1),
        ));
        let outcome = engine
            .search(SearchOptions {
                query: "movie".into(),
                providers: vec![Arc::new(BlockingParser)],
                timeout: Duration::from_millis(5),
                ..SearchOptions::default()
            })
            .await;
        assert_eq!(outcome.successful_sources, 0);
        assert_eq!(outcome.source_errors[0].kind, "timeout");
        assert_eq!(outcome.total_links, 1);
        assert!(outcome.results[0].links[0].url.ends_with("/early"));
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

    struct DisconnectPendingProvider {
        active: Arc<AtomicUsize>,
        ready: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl Provider for DisconnectPendingProvider {
        fn meta(&self) -> ProviderMeta {
            ProviderMeta::stateless("disconnect-pending", 2, KeywordFilterMode::Provider)
        }
        async fn search(
            &self,
            ctx: &SearchContext,
            _: &str,
        ) -> Result<Vec<SearchResult>, ProviderError> {
            self.active.fetch_add(1, Ordering::SeqCst);
            let _guard = ActiveGuard(self.active.clone());
            ctx.publish(vec![result(
                "early",
                "movie early",
                "https://pan.quark.cn/s/early",
            )]);
            self.ready.notify_one();
            futures::future::pending().await
        }
    }

    #[tokio::test(start_paused = true)]
    async fn consumer_disconnect_drops_pending_provider_and_keeps_interrupted_accounting() {
        let active = Arc::new(AtomicUsize::new(0));
        let ready = Arc::new(tokio::sync::Notify::new());
        let (sender, receiver) = mpsc::unbounded_channel();
        let (_cancel, cancellation) = watch::channel(false);
        let engine = test_engine();
        let search = engine.search_stream(
            SearchOptions {
                query: "movie".into(),
                providers: vec![
                    Arc::new(DisconnectPendingProvider {
                        active: active.clone(),
                        ready: ready.clone(),
                    }),
                    Arc::new(FakeProvider {
                        name: "queued",
                        mode: KeywordFilterMode::Core,
                        result: Ok(vec![]),
                    }),
                ],
                jobs: 1,
                timeout: Duration::from_secs(3600),
                all_timeout: Duration::from_secs(3600),
                ..Default::default()
            },
            sender,
            cancellation,
        );
        tokio::pin!(search);
        tokio::select! {
            _ = &mut search => panic!("pending provider must not finish before disconnect"),
            _ = ready.notified() => {},
        }
        assert_eq!(active.load(Ordering::SeqCst), 1);
        drop(receiver);
        let (outcome, summary) = tokio::time::timeout(Duration::from_millis(1), search)
            .await
            .expect("consumer disconnect must wake the search without waiting for a source");
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert_eq!(summary.finish_reason, FinishReason::Interrupted);
        assert_eq!(summary.exit_code(), 130);
        assert_eq!(summary.sources_selected, 2);
        assert_eq!(summary.sources_completed, 0);
        assert_eq!(summary.sources_failed, 0);
        assert_eq!(summary.sources_cancelled, 1);
        assert_eq!(summary.sources_not_started, 1);
        assert_eq!(outcome.total_links, 1);
        assert!(outcome.results[0].links[0].url.ends_with("/early"));
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

    #[derive(Debug)]
    struct IncrementalProvider {
        delay: Duration,
        fail: bool,
    }

    #[async_trait]
    impl Provider for IncrementalProvider {
        fn meta(&self) -> ProviderMeta {
            ProviderMeta::stateless("incremental", 2, KeywordFilterMode::Provider)
        }
        async fn search(
            &self,
            ctx: &SearchContext,
            _: &str,
        ) -> Result<Vec<SearchResult>, ProviderError> {
            assert!(ctx.deadline().is_some());
            let row = result("incremental", "Movie", "https://pan.quark.cn/s/fast");
            assert!(ctx.publish(vec![row.clone()]));
            tokio::time::sleep(self.delay).await;
            if self.fail {
                return Err(ProviderError::Network("later request failed".into()));
            }
            let mut row = row;
            row.links.push(Link::new("https://pan.quark.cn/s/second"));
            row.links[0].password = Some("abcd".into());
            Ok(vec![row])
        }
    }

    async fn incremental_search(
        timeout: u64,
        all_timeout: u64,
        delay: u64,
        fail: bool,
    ) -> (SearchOutcome, SearchSummary) {
        let (tx, _rx) = mpsc::unbounded_channel();
        let (_cancel, cancelled) = watch::channel(false);
        test_engine()
            .search_stream(
                SearchOptions {
                    providers: vec![Arc::new(IncrementalProvider {
                        delay: Duration::from_secs(delay),
                        fail,
                    })],
                    timeout: Duration::from_secs(timeout),
                    all_timeout: Duration::from_secs(all_timeout),
                    ..Default::default()
                },
                tx,
                cancelled,
            )
            .await
    }

    #[tokio::test(start_paused = true)]
    async fn incremental_deadline_preserves_snapshot_without_success() {
        let (outcome, summary) = incremental_search(100, 5, 100, false).await;
        assert_eq!(outcome.total_links, 1);
        assert_eq!(summary.sources_completed, 0);
        assert_eq!(summary.sources_failed, 0);
        assert_eq!(summary.sources_cancelled, 1);
        assert_eq!(summary.sources_not_started, 0);
        assert_eq!(summary.exit_code(), 124);
        assert!(summary.partial);
    }

    #[tokio::test(start_paused = true)]
    async fn interrupted_incremental_source_keeps_links_and_exit_130() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let (cancel, cancelled) = watch::channel(false);
        let interrupt = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            cancel.send(true).unwrap();
        });
        let (outcome, summary) = test_engine()
            .search_stream(
                SearchOptions {
                    providers: vec![Arc::new(IncrementalProvider {
                        delay: Duration::from_secs(100),
                        fail: false,
                    })],
                    ..Default::default()
                },
                tx,
                cancelled,
            )
            .await;
        interrupt.await.unwrap();
        assert_eq!(outcome.total_links, 1);
        assert_eq!(summary.sources_cancelled, 1);
        assert_eq!(summary.sources_failed, 0);
        assert_eq!(summary.exit_code(), 130);
    }

    #[tokio::test(start_paused = true)]
    async fn complete_empty_source_is_success_even_when_another_fails() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let (_cancel, cancelled) = watch::channel(false);
        let (_, summary) = test_engine()
            .search_stream(
                SearchOptions {
                    providers: vec![
                        Arc::new(FakeProvider {
                            name: "empty",
                            mode: KeywordFilterMode::Core,
                            result: Ok(vec![]),
                        }),
                        Arc::new(FakeProvider {
                            name: "broken",
                            mode: KeywordFilterMode::Core,
                            result: Err(ProviderError::Blocked),
                        }),
                    ],
                    ..Default::default()
                },
                tx,
                cancelled,
            )
            .await;
        assert_eq!(summary.sources_completed, 1);
        assert_eq!(summary.sources_failed, 1);
        assert_eq!(summary.total_links, 0);
        assert_eq!(summary.exit_code(), 0);
        assert!(summary.partial);
    }

    #[tokio::test(start_paused = true)]
    async fn partial_timeout_and_error_are_failures_even_with_links() {
        for (timeout, delay, fail, kind) in [(2, 100, false, "timeout"), (30, 1, true, "network")] {
            let (outcome, summary) = incremental_search(timeout, 600, delay, fail).await;
            assert_eq!(outcome.total_links, 1);
            assert_eq!(outcome.source_errors[0].kind, kind);
            assert_eq!(summary.sources_completed, 0);
            assert_eq!(summary.sources_failed, 1);
            assert_eq!(summary.sources_cancelled, 0);
            assert_eq!(summary.exit_code(), 3);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn normal_return_unions_incremental_rows_and_enriches_passwords() {
        let (outcome, summary) = incremental_search(30, 600, 1, false).await;
        assert_eq!(outcome.total_results, 1);
        assert_eq!(outcome.total_links, 2);
        assert_eq!(
            outcome.results[0].links[0].password.as_deref(),
            Some("abcd")
        );
        assert_eq!(summary.sources_completed, 1);
        assert_eq!(summary.exit_code(), 0);
        assert!(!summary.partial);
    }

    #[derive(Debug)]
    struct CancelledIncremental(&'static str);
    #[async_trait]
    impl Provider for CancelledIncremental {
        fn meta(&self) -> ProviderMeta {
            ProviderMeta::stateless(self.0, 2, KeywordFilterMode::Provider)
        }
        async fn search(
            &self,
            ctx: &SearchContext,
            _: &str,
        ) -> Result<Vec<SearchResult>, ProviderError> {
            ctx.publish(vec![result(
                self.0,
                self.0,
                &format!("https://pan.quark.cn/s/{}", self.0),
            )]);
            futures::future::pending().await
        }
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_snapshots_are_emitted_in_selected_source_order() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (_cancel, cancelled) = watch::channel(false);
        let (_, summary) = test_engine()
            .search_stream(
                SearchOptions {
                    providers: vec![
                        Arc::new(CancelledIncremental("first")),
                        Arc::new(CancelledIncremental("second")),
                    ],
                    all_timeout: Duration::from_secs(1),
                    ..Default::default()
                },
                tx,
                cancelled,
            )
            .await;
        let SearchEvent::Result { link: first, .. } = rx.recv().await.unwrap() else {
            panic!("first snapshot result");
        };
        let SearchEvent::Result { link: second, .. } = rx.recv().await.unwrap() else {
            panic!("second snapshot result");
        };
        assert!(first.url.ends_with("/first"));
        assert!(second.url.ends_with("/second"));
        assert_eq!(summary.sources_cancelled, 2);
        assert_eq!(
            summary.sources_completed
                + summary.sources_failed
                + summary.sources_cancelled
                + summary.sources_not_started,
            summary.sources_selected
        );
    }

    #[tokio::test(start_paused = true)]
    async fn final_metadata_enrichment_precedes_query_filtering() {
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(30));
        let mut early = result("incremental", "placeholder", "https://pan.quark.cn/s/fast");
        early.links[0].work_title = Some("placeholder".into());
        early.links[0].password = Some("abcd".into());
        ctx.publish(vec![early.clone()]);
        let mut final_row = early;
        final_row.title = "Movie title".into();
        final_row.content = "enriched content".into();
        final_row.tags = vec!["tag".into()];
        final_row.images = vec!["image".into()];
        final_row.links[0].password = None;
        final_row.links[0].work_title = Some("Movie episode".into());
        let mut rows = ctx.finish(vec![final_row]);
        apply_query_filter(&mut rows, "Movie", &HashMap::new());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Movie title");
        assert_eq!(rows[0].content, "enriched content");
        assert_eq!(rows[0].tags, ["tag"]);
        assert_eq!(rows[0].images, ["image"]);
        assert_eq!(rows[0].links[0].password.as_deref(), Some("abcd"));
        assert_eq!(
            rows[0].links[0].work_title.as_deref(),
            Some("Movie episode")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn frozen_collector_rejects_late_writes_and_deadline_is_not_reset() {
        let base = SearchContext::new(reqwest::Client::new(), Duration::from_secs(30));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let ctx = base.for_source(deadline);
        let clone = ctx.clone();
        let row = result("incremental", "Movie", "https://pan.quark.cn/s/fast");
        assert!(ctx.publish(vec![row.clone()]));
        let snapshot = ctx.finish(Vec::new());
        assert!(!clone.publish(vec![row.clone()]));
        assert_eq!(clone.finish(Vec::new()).len(), snapshot.len());
        let fresh = base.for_source(deadline);
        tokio::time::advance(Duration::from_secs(5)).await;
        assert!(!fresh.publish(vec![row]));
        assert_eq!(fresh.remaining(), Duration::ZERO);
        assert_eq!(fresh.deadline(), Some(deadline));
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
        let (_, summary) = handle.await.unwrap();
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
        let (_, summary) = test_engine()
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
        let (_, summary) = test_engine()
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
        let (_, summary) = handle.await.unwrap();
        assert_eq!(summary.finish_reason, FinishReason::Interrupted);
        assert_eq!(summary.total_links, 1);
        assert_eq!(summary.sources_completed, 1);
        assert_eq!(summary.sources_cancelled, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn source_timeout_releases_slot_and_keeps_searching() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (_cancel, cancelled) = watch::channel(false);
        let (_, summary) = test_engine()
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
