use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::Future,
    io::Write,
    sync::Arc,
    time::{Duration, Instant},
};

use futures::{FutureExt, StreamExt, future::BoxFuture, stream::FuturesUnordered};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use tokio::sync::{mpsc, watch};

use crate::{
    channel::ChannelStore,
    check::{CheckEngine, CheckItem, CheckOptions, CheckResult},
    core::{CloudType, MergedLink, canonical_url_key},
    output::{OutputFormat, SearchCheckSummary, stdout, write_search},
    search::{
        FinishReason, SearchEngine, SearchEvent, SearchOptions, SearchOutcome, SearchSummary,
    },
};

use super::{EXIT_DEADLINE, EXIT_INTERRUPTED, EXIT_OK, EXIT_SEARCH_FAILED};

#[allow(clippy::too_many_arguments)]
pub(super) async fn run(
    engine: SearchEngine,
    options: SearchOptions,
    checker: Option<Arc<CheckEngine>>,
    check_options: CheckOptions,
    format: OutputFormat,
    require_ok: bool,
    verbose: bool,
    no_progress: bool,
    channel_store: ChannelStore,
) -> anyhow::Result<i32> {
    drive(
        engine,
        options,
        checker,
        check_options,
        format,
        require_ok,
        verbose,
        no_progress,
        Some(channel_store),
        stdout(),
        tokio::signal::ctrl_c(),
    )
    .await
}

type CheckTask = BoxFuture<'static, (String, CheckResult)>;

struct SearchProgress {
    multi: MultiProgress,
    sources: ProgressBar,
    checks: Option<ProgressBar>,
}

impl SearchProgress {
    fn new(sources: usize, checks_enabled: bool, hidden: bool) -> Self {
        let target = if hidden {
            ProgressDrawTarget::hidden()
        } else {
            ProgressDrawTarget::stderr()
        };
        let multi = MultiProgress::with_draw_target(target);
        let source_style = ProgressStyle::with_template(
            "{spinner:.cyan} Searching [{bar:30.cyan/blue}] {pos}/{len} · {msg}",
        )
        .expect("valid search progress template")
        .progress_chars("=>-");
        let sources = multi.add(ProgressBar::new(sources as u64));
        sources.set_style(source_style);
        sources.set_message("0 candidates");
        sources.enable_steady_tick(Duration::from_millis(120));

        let checks = checks_enabled.then(|| {
            let style = ProgressStyle::with_template(
                "{spinner:.green} Checking  [{bar:30.green/blue}] {pos}/{len} · {msg}",
            )
            .expect("valid check progress template")
            .progress_chars("=>-");
            let bar = multi.add(ProgressBar::new(0));
            bar.set_style(style);
            bar.set_message("0 valid");
            bar.enable_steady_tick(Duration::from_millis(120));
            bar
        });

        Self {
            multi,
            sources,
            checks,
        }
    }

    fn update_sources(&self, completed: usize, links: usize) {
        self.sources.set_position(completed as u64);
        self.sources.set_message(format!("{links} candidates"));
    }

    fn add_check(&self) {
        if let Some(checks) = &self.checks {
            checks.inc_length(1);
        }
    }

    fn complete_check(&self, valid: usize) {
        if let Some(checks) = &self.checks {
            checks.inc(1);
            checks.set_message(format!("{valid} valid"));
        }
    }

    fn clear(&self) {
        self.sources.finish_and_clear();
        if let Some(checks) = &self.checks {
            checks.finish_and_clear();
        }
        let _ = self.multi.clear();
    }
}

/// Check completion is polled alongside source events, so slow checks never
/// prevent the search from making progress or reaching its own deadline.
#[allow(clippy::too_many_arguments)]
async fn drive<W: Write, F: Future<Output = std::io::Result<()>>>(
    engine: SearchEngine,
    options: SearchOptions,
    checker: Option<Arc<CheckEngine>>,
    check_options: CheckOptions,
    format: OutputFormat,
    require_ok: bool,
    verbose: bool,
    no_progress: bool,
    channel_store: Option<ChannelStore>,
    mut writer: W,
    interrupt: F,
) -> anyhow::Result<i32> {
    let started = Instant::now();
    let source_count = options.providers.len() + options.channels.len();
    let progress = SearchProgress::new(source_count, checker.is_some(), no_progress);
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let (cancel, cancellation) = watch::channel(false);
    let search = engine.search_stream(options, sender, cancellation);
    tokio::pin!(search, interrupt);
    let mut completed: Option<(SearchOutcome, SearchSummary)> = None;
    let mut interrupted = false;
    let mut checked = HashMap::<String, CheckResult>::new();
    let mut scheduled = HashSet::new();
    let mut pending = VecDeque::<(String, String, CheckItem)>::new();
    let mut checks = FuturesUnordered::<CheckTask>::new();
    let mut invalid_channels = HashSet::<String>::new();
    let mut valid_checks = 0;

    loop {
        if let Some(checker) = &checker {
            while !interrupted && checks.len() < check_options.jobs.max(1) {
                let Some((_id, key, item)) = pending.pop_front() else {
                    break;
                };
                let checker = checker.clone();
                let mut options = check_options.clone();
                options.jobs = 1;
                checks.push(
                    async move {
                        let result = checker.check(vec![item], options).await.remove(0);
                        (key, result)
                    }
                    .boxed(),
                );
            }
        }
        if completed.is_some() && receiver.is_empty() && checks.is_empty() && pending.is_empty() {
            break;
        }
        tokio::select! {
            biased;
            signal = &mut interrupt, if !interrupted => {
                signal?;
                interrupted = true;
                let _ = cancel.send(true);
                checks.clear();
                pending.clear();
            }
            result = &mut search, if completed.is_none() => { completed = Some(result); }
            Some(event) = receiver.recv() => {
                match event {
                    SearchEvent::Result { id, link } | SearchEvent::ResultUpdate { id, link } => {
                        if interrupted { continue; }
                        if checker.is_some() && requires_link_check(link.cloud_type) {
                            let key = check_key(&link);
                            if !checked.contains_key(&key) && scheduled.insert(key.clone()) {
                                let mut item = CheckItem::detect(link.url);
                                item.password = link.password;
                                pending.push_back((id, key, item));
                                progress.add_check();
                            }
                        }
                    }
                    SearchEvent::Progress { completed, selected, links } => {
                        debug_assert_eq!(selected, source_count);
                        progress.update_sources(completed, links);
                    }
                    SearchEvent::Summary { .. } => {} // Final summary includes completed checks below.
                    SearchEvent::SourceError { ref error } => {
                        if error.kind == "invalid_source"
                            && let Some(channel) = error.source.strip_prefix("tg:")
                        {
                            invalid_channels.insert(channel.to_owned());
                        }
                    }
                }
            }
            Some((key, result)) = checks.next(), if !checks.is_empty() => {
                if result.state == crate::check::CheckState::Ok {
                    valid_checks += 1;
                }
                checked.insert(key, result);
                progress.complete_check(valid_checks);
            }
        }
    }
    progress.clear();
    let (mut outcome, mut summary) =
        completed.expect("search must complete before output loop ends");
    if let Some(store) = channel_store.filter(|_| !invalid_channels.is_empty()) {
        let mut names = invalid_channels.into_iter().collect::<Vec<_>>();
        names.sort();
        let changed = store.disable_existing(&names)?;
        if !changed.changed_names.is_empty() {
            eprintln!(
                "Disabled invalid Telegram channel{}: {}",
                if changed.changed_names.len() == 1 {
                    ""
                } else {
                    "s"
                },
                changed.changed_names.join(", ")
            );
        }
    }
    if verbose {
        for error in &outcome.source_errors {
            eprintln!("{}: {}", error.source, error.message);
        }
    }

    let candidates = outcome.total_links;
    let mut final_checked = 0;
    let mut final_valid = 0;
    for links in outcome.links_by_type.values_mut() {
        links.retain_mut(|link| {
            let Some(result) = checked.get(&check_key(link)) else {
                return !require_ok || !requires_link_check(link.cloud_type);
            };
            final_checked += 1;
            *link = attach_check(link.clone(), result);
            let valid = result.state == crate::check::CheckState::Ok;
            final_valid += usize::from(valid);
            valid || !require_ok
        });
    }
    outcome.links_by_type.retain(|_, links| !links.is_empty());
    outcome.total_links = outcome.links_by_type.values().map(Vec::len).sum();
    outcome.duration_ms = started.elapsed().as_millis();
    summary.total_links = outcome.total_links;
    summary.duration_ms = started.elapsed().as_millis();
    if interrupted {
        summary.finish_reason = FinishReason::Interrupted;
    }
    let code = match summary.finish_reason {
        FinishReason::Interrupted => EXIT_INTERRUPTED,
        FinishReason::Deadline => EXIT_DEADLINE,
        FinishReason::Completed if summary.sources_completed == 0 => EXIT_SEARCH_FAILED,
        FinishReason::Completed => EXIT_OK,
    };
    let checks = SearchCheckSummary {
        enabled: checker.is_some(),
        candidates,
        checked: final_checked,
        valid: final_valid,
    };
    write_search(&mut writer, &outcome, &summary, &checks, format)?;
    Ok(code)
}

fn requires_link_check(cloud_type: CloudType) -> bool {
    !matches!(cloud_type, CloudType::Magnet | CloudType::Ed2k)
}

fn check_key(link: &MergedLink) -> String {
    // Password is part of check identity; a newly discovered code needs rechecking.
    format!(
        "{}\0{}",
        canonical_url_key(&link.url),
        link.password.as_deref().unwrap_or_default()
    )
}

fn attach_check(mut link: MergedLink, value: &CheckResult) -> MergedLink {
    use crate::{check::CheckState as Input, core::CheckState as Output};
    link.check = Some(crate::core::CheckResult {
        state: match value.state {
            Input::Ok => Output::Ok,
            Input::Bad => Output::Bad,
            Input::Locked => Output::Locked,
            Input::Unsupported => Output::Unsupported,
            Input::Uncertain => Output::Uncertain,
        },
        cache_hit: value.cache_hit,
        checked_at: chrono::DateTime::from_timestamp_millis(value.checked_at_ms),
        expires_at: chrono::DateTime::from_timestamp_millis(value.expires_at_ms),
        summary: value.summary.clone(),
    });
    link
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        channel::ChannelStore,
        check::{CheckCloudType, CheckContext, CheckError, CheckEvaluation, LinkChecker},
        core::{Link, ProviderError, SearchResult},
        providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
        search::TelegramSource,
    };
    use async_trait::async_trait;
    use std::{
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };
    use url::Url;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);
    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Buffer {
        fn is_empty(&self) -> bool {
            self.0.lock().unwrap().is_empty()
        }

        fn document(&self) -> serde_json::Value {
            serde_json::from_slice(&self.0.lock().unwrap()).unwrap()
        }
    }

    struct FixtureProvider {
        delay: u64,
        password: Option<&'static str>,
    }
    #[async_trait]
    impl Provider for FixtureProvider {
        fn meta(&self) -> ProviderMeta {
            ProviderMeta::stateless("fixture", 3, KeywordFilterMode::Provider)
        }
        async fn search(
            &self,
            _: &SearchContext,
            _: &str,
        ) -> Result<Vec<SearchResult>, ProviderError> {
            tokio::time::sleep(Duration::from_secs(self.delay)).await;
            let mut result = SearchResult::provider(format!("fixture-{}", self.delay), "fixture");
            result.title = "query".into();
            let mut link = Link::new("https://pan.quark.cn/s/abc123");
            link.password = self.password.map(str::to_owned);
            result.links.push(link);
            Ok(vec![result])
        }
    }

    struct MixedLinksProvider;
    #[async_trait]
    impl Provider for MixedLinksProvider {
        fn meta(&self) -> ProviderMeta {
            ProviderMeta::stateless("mixed-fixture", 3, KeywordFilterMode::Provider)
        }

        async fn search(
            &self,
            _: &SearchContext,
            _: &str,
        ) -> Result<Vec<SearchResult>, ProviderError> {
            let mut result = SearchResult::provider("mixed-fixture", "mixed-fixture");
            result.title = "query".into();
            result.links.extend([
                Link::new("https://pan.quark.cn/s/abc123"),
                Link::new("magnet:?xt=urn:btih:abcdef1234567890"),
                Link::new("ed2k://|file|example.mkv|123|ABCDEF|/"),
            ]);
            Ok(vec![result])
        }
    }

    struct SlowChecker(Arc<AtomicUsize>);
    #[async_trait]
    impl LinkChecker for SlowChecker {
        fn cloud_type(&self) -> CheckCloudType {
            CheckCloudType::Quark
        }
        async fn check_normalized(
            &self,
            _: &CheckContext,
            item: &CheckItem,
            _: &str,
        ) -> Result<CheckEvaluation, CheckError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_secs(if item.password.is_some() {
                2
            } else {
                10
            }))
            .await;
            Ok(CheckEvaluation::new(
                crate::check::CheckState::Ok,
                "verified",
            ))
        }
    }

    struct BadChecker;
    #[async_trait]
    impl LinkChecker for BadChecker {
        fn cloud_type(&self) -> CheckCloudType {
            CheckCloudType::Quark
        }

        async fn check_normalized(
            &self,
            _: &CheckContext,
            _: &CheckItem,
            _: &str,
        ) -> Result<CheckEvaluation, CheckError> {
            Ok(CheckEvaluation::new(
                crate::check::CheckState::Bad,
                "invalid",
            ))
        }
    }

    fn setup(providers: Vec<Arc<dyn Provider>>) -> (SearchEngine, SearchOptions) {
        (
            SearchEngine::new(SearchContext::new(
                reqwest::Client::new(),
                Duration::from_secs(30),
            )),
            SearchOptions {
                query: "query".into(),
                providers,
                ..SearchOptions::default()
            },
        )
    }

    #[test]
    fn no_progress_uses_hidden_bars() {
        let progress = SearchProgress::new(2, true, true);
        assert!(progress.multi.is_hidden());
        assert!(progress.sources.is_hidden());
        assert!(progress.checks.as_ref().unwrap().is_hidden());
    }

    #[tokio::test(start_paused = true)]
    async fn final_output_filters_invalid_checked_links() {
        let (engine, options) = setup(vec![Arc::new(FixtureProvider {
            delay: 0,
            password: None,
        })]);
        let checker = Arc::new(CheckEngine::with_checkers(
            reqwest::Client::new(),
            None,
            vec![Arc::new(BadChecker)],
        ));
        let mut output = Vec::new();
        let code = drive(
            engine,
            options,
            Some(checker),
            CheckOptions::default(),
            OutputFormat::Json,
            true,
            false,
            true,
            None,
            &mut output,
            futures::future::pending(),
        )
        .await
        .unwrap();

        assert_eq!(code, EXIT_OK);
        let document: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(document["total_links"], 0);
        assert_eq!(document["checks"]["candidates"], 1);
        assert_eq!(document["checks"]["checked"], 1);
        assert_eq!(document["checks"]["valid"], 0);
    }

    #[tokio::test]
    async fn unchecked_link_types_survive_valid_only_filter() {
        let (engine, options) = setup(vec![Arc::new(MixedLinksProvider)]);
        let checker = Arc::new(CheckEngine::with_checkers(
            reqwest::Client::new(),
            None,
            vec![Arc::new(BadChecker)],
        ));
        let mut output = Vec::new();
        let code = drive(
            engine,
            options,
            Some(checker),
            CheckOptions::default(),
            OutputFormat::Json,
            true,
            false,
            true,
            None,
            &mut output,
            futures::future::pending(),
        )
        .await
        .unwrap();

        assert_eq!(code, EXIT_OK);
        let document: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(document["total_links"], 2);
        assert!(document["links_by_type"].get("quark").is_none());
        assert_eq!(
            document["links_by_type"]["magnet"][0]["cloud_type"],
            "magnet"
        );
        assert!(
            document["links_by_type"]["magnet"][0]
                .get("check")
                .is_none()
        );
        assert_eq!(document["links_by_type"]["ed2k"][0]["cloud_type"], "ed2k");
        assert!(document["links_by_type"]["ed2k"][0].get("check").is_none());
        assert_eq!(document["checks"]["candidates"], 3);
        assert_eq!(document["checks"]["checked"], 1);
        assert_eq!(document["checks"]["valid"], 0);
    }

    #[tokio::test(start_paused = true)]
    async fn withholds_stdout_until_slow_source_finishes() {
        let (engine, options) = setup(vec![
            Arc::new(FixtureProvider {
                delay: 0,
                password: None,
            }),
            Arc::new(FixtureProvider {
                delay: 10,
                password: Some("abcd"),
            }),
        ]);
        let buffer = Buffer::default();
        let observer = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            assert!(buffer.is_empty());
        };
        let output = drive(
            engine,
            options,
            None,
            CheckOptions::default(),
            OutputFormat::Json,
            false,
            false,
            true,
            None,
            buffer.clone(),
            futures::future::pending(),
        );
        let (result, _) = tokio::join!(output, observer);
        assert_eq!(result.unwrap(), EXIT_OK);
        assert_eq!(buffer.document()["total_links"], 1);
    }

    #[tokio::test(start_paused = true)]
    async fn checks_continue_after_search_deadline_and_do_not_block_it() {
        let (engine, mut options) = setup(vec![
            Arc::new(FixtureProvider {
                delay: 0,
                password: None,
            }),
            Arc::new(FixtureProvider {
                delay: 20,
                password: None,
            }),
        ]);
        options.all_timeout = Duration::from_secs(2);
        let calls = Arc::new(AtomicUsize::new(0));
        let checker = Arc::new(CheckEngine::with_checkers(
            reqwest::Client::new(),
            None,
            vec![Arc::new(SlowChecker(calls.clone()))],
        ));
        let buffer = Buffer::default();
        let code = drive(
            engine,
            options,
            Some(checker),
            CheckOptions::default(),
            OutputFormat::Json,
            true,
            false,
            true,
            None,
            buffer.clone(),
            futures::future::pending(),
        )
        .await
        .unwrap();
        assert_eq!(code, EXIT_DEADLINE);
        let document = buffer.document();
        assert_eq!(
            document["links_by_type"]["quark"][0]["check"]["state"],
            "ok"
        );
        let summary = &document["summary"];
        assert_eq!(summary["finish_reason"], "deadline");
        assert_eq!(summary["sources_completed"], 1);
        assert_eq!(summary["sources_cancelled"], 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn newer_password_check_wins_and_stale_check_is_not_printed() {
        let (engine, options) = setup(vec![
            Arc::new(FixtureProvider {
                delay: 0,
                password: None,
            }),
            Arc::new(FixtureProvider {
                delay: 1,
                password: Some("abcd"),
            }),
        ]);
        let calls = Arc::new(AtomicUsize::new(0));
        let checker = Arc::new(CheckEngine::with_checkers(
            reqwest::Client::new(),
            None,
            vec![Arc::new(SlowChecker(calls.clone()))],
        ));
        let buffer = Buffer::default();
        let code = drive(
            engine,
            options,
            Some(checker),
            CheckOptions::default(),
            OutputFormat::Json,
            true,
            false,
            true,
            None,
            buffer.clone(),
            futures::future::pending(),
        )
        .await
        .unwrap();
        assert_eq!(code, EXIT_OK);
        let document = buffer.document();
        assert_eq!(document["links_by_type"]["quark"][0]["password"], "abcd");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn interrupt_cancels_pending_checks_and_reports_130() {
        let (engine, options) = setup(vec![Arc::new(FixtureProvider {
            delay: 0,
            password: None,
        })]);
        let checker = Arc::new(CheckEngine::with_checkers(
            reqwest::Client::new(),
            None,
            vec![Arc::new(SlowChecker(Arc::new(AtomicUsize::new(0))))],
        ));
        let buffer = Buffer::default();
        let interrupt = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Ok(())
        };
        let code = drive(
            engine,
            options,
            Some(checker),
            CheckOptions::default(),
            OutputFormat::Json,
            false,
            false,
            true,
            None,
            buffer.clone(),
            interrupt,
        )
        .await
        .unwrap();
        assert_eq!(code, EXIT_INTERRUPTED);
        assert_eq!(buffer.document()["summary"]["finish_reason"], "interrupted");
    }

    #[tokio::test]
    async fn invalid_saved_channels_are_disabled_but_other_sources_are_untouched() {
        let server = MockServer::start().await;
        for (name, response) in [
            ("invalid", ResponseTemplate::new(404)),
            ("temporary", ResponseTemplate::new(410)),
            ("busy", ResponseTemplate::new(503)),
            (
                "empty",
                ResponseTemplate::new(200).set_body_string(
                    r#"<div class="tgme_channel_info"></div><div class="tme_no_messages_found">No posts found</div>"#,
                ),
            ),
        ] {
            Mock::given(method("GET"))
                .and(path(format!("/s/{name}")))
                .respond_with(response)
                .mount(&server)
                .await;
        }
        let dir = tempfile::tempdir().unwrap();
        let store = ChannelStore::new(dir.path().join("channels.toml"));
        store
            .add(&["invalid".into(), "busy".into(), "empty".into()], true)
            .unwrap();
        let engine = SearchEngine::new(SearchContext::new(
            reqwest::Client::new(),
            Duration::from_secs(5),
        ))
        .with_telegram(TelegramSource::with_base_url(
            reqwest::Client::new(),
            Url::parse(&format!("{}/", server.uri())).unwrap(),
        ));
        let options = SearchOptions {
            query: "needle".into(),
            channels: vec![
                "invalid".into(),
                "temporary".into(),
                "busy".into(),
                "empty".into(),
            ],
            ..SearchOptions::default()
        };

        let code = drive(
            engine,
            options,
            None,
            CheckOptions::default(),
            OutputFormat::Json,
            false,
            false,
            true,
            Some(store.clone()),
            Vec::new(),
            futures::future::pending(),
        )
        .await
        .unwrap();

        assert_eq!(code, EXIT_OK);
        let list = store.load().unwrap();
        assert!(
            !list
                .channels
                .iter()
                .find(|c| c.name == "invalid")
                .unwrap()
                .enabled
        );
        assert!(
            list.channels
                .iter()
                .find(|c| c.name == "busy")
                .unwrap()
                .enabled
        );
        assert!(
            list.channels
                .iter()
                .find(|c| c.name == "empty")
                .unwrap()
                .enabled
        );
        assert!(!list.channels.iter().any(|c| c.name == "temporary"));
    }
}
