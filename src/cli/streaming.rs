use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::Future,
    io::Write,
    sync::Arc,
    time::Instant,
};

use futures::{FutureExt, StreamExt, future::BoxFuture, stream::FuturesUnordered};
use tokio::sync::{mpsc, watch};

use crate::{
    channel::ChannelStore,
    check::{CheckEngine, CheckItem, CheckOptions, CheckResult},
    core::{MergedLink, canonical_url_key},
    output::{OutputFormat, stdout, write_search_event},
    search::{FinishReason, SearchEngine, SearchEvent, SearchOptions, SearchSummary},
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
    quiet: bool,
    channel_store: ChannelStore,
) -> anyhow::Result<i32> {
    drive(
        engine,
        options,
        checker,
        check_options,
        format,
        require_ok,
        quiet,
        Some(channel_store),
        stdout(),
        tokio::signal::ctrl_c(),
    )
    .await
}

type CheckTask = BoxFuture<'static, (String, String, CheckResult)>;

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
    quiet: bool,
    channel_store: Option<ChannelStore>,
    mut writer: W,
    interrupt: F,
) -> anyhow::Result<i32> {
    let started = Instant::now();
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let (cancel, cancellation) = watch::channel(false);
    let search = engine.search_stream(options, sender, cancellation);
    tokio::pin!(search, interrupt);
    let mut summary: Option<SearchSummary> = None;
    let mut interrupted = false;
    let mut latest = HashMap::<String, MergedLink>::new();
    let mut emitted = HashMap::<String, MergedLink>::new();
    let mut checked = HashMap::<String, CheckResult>::new();
    let mut scheduled = HashSet::new();
    let mut pending = VecDeque::<(String, String, CheckItem)>::new();
    let mut checks = FuturesUnordered::<CheckTask>::new();
    let mut invalid_channels = HashSet::<String>::new();

    loop {
        if let Some(checker) = &checker {
            while !interrupted && checks.len() < check_options.jobs.max(1) {
                let Some((id, key, item)) = pending.pop_front() else {
                    break;
                };
                let checker = checker.clone();
                let mut options = check_options.clone();
                options.jobs = 1;
                checks.push(
                    async move {
                        let result = checker.check(vec![item], options).await.remove(0);
                        (id, key, result)
                    }
                    .boxed(),
                );
            }
        }
        if summary.is_some() && receiver.is_empty() && checks.is_empty() && pending.is_empty() {
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
            result = &mut search, if summary.is_none() => { summary = Some(result); }
            Some(event) = receiver.recv() => {
                match event {
                    SearchEvent::Result { id, link } | SearchEvent::ResultUpdate { id, link } => {
                        if interrupted { continue; }
                        latest.insert(id.clone(), link.clone());
                        if checker.is_none() {
                            emit_link(&mut writer, &mut emitted, id, link, format, require_ok)?;
                        } else {
                            let key = check_key(&link);
                            if let Some(result) = checked.get(&key) {
                                emit_link(&mut writer, &mut emitted, id, attach_check(link, result), format, require_ok)?;
                            } else if scheduled.insert(key.clone()) {
                                let mut item = CheckItem::detect(link.url);
                                item.password = link.password;
                                pending.push_back((id, key, item));
                            }
                        }
                    }
                    SearchEvent::Progress { completed, selected, links } => {
                        if !quiet { eprintln!("Sources: {completed}/{selected} finished · Links: {links}"); }
                    }
                    SearchEvent::Summary { .. } => {} // Final summary includes completed checks below.
                    SearchEvent::SourceError { ref error } => {
                        if error.kind == "invalid_source"
                            && let Some(channel) = error.source.strip_prefix("tg:")
                        {
                            invalid_channels.insert(channel.to_owned());
                        }
                        if !quiet { eprintln!("{}: {}", error.source, error.message); }
                        write_search_event(&mut writer, &event, format)?;
                    }
                }
            }
            Some((id, key, result)) = checks.next(), if !checks.is_empty() => {
                if let Some(link) = latest.get(&id).filter(|link| check_key(link) == key) {
                    emit_link(&mut writer, &mut emitted, id, attach_check(link.clone(), &result), format, require_ok)?;
                }
                checked.insert(key, result);
            }
        }
    }
    let mut summary = summary.expect("search must complete before output loop ends");
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
    summary.total_links = emitted.len();
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
    write_search_event(&mut writer, &SearchEvent::Summary { summary }, format)?;
    Ok(code)
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

fn emit_link<W: Write>(
    writer: &mut W,
    emitted: &mut HashMap<String, MergedLink>,
    id: String,
    link: MergedLink,
    format: OutputFormat,
    require_ok: bool,
) -> anyhow::Result<()> {
    if require_ok
        && !link
            .check
            .as_ref()
            .is_some_and(|check| check.state == crate::core::CheckState::Ok)
    {
        return Ok(());
    }
    if emitted.get(&id) == Some(&link) {
        return Ok(());
    }
    let update = emitted.insert(id.clone(), link.clone()).is_some();
    let event = if update {
        SearchEvent::ResultUpdate { id, link }
    } else {
        SearchEvent::Result { id, link }
    };
    write_search_event(writer, &event, format)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        channel::ChannelStore,
        check::{CheckCloudType, CheckContext, CheckError, CheckEvaluation, LinkChecker},
        core::{Link, MergedLink, ProviderError, SearchResult, Source},
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
        fn events(&self) -> Vec<serde_json::Value> {
            String::from_utf8(self.0.lock().unwrap().clone())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
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

    fn checked_link(state: crate::core::CheckState) -> MergedLink {
        MergedLink {
            cloud_type: crate::core::CloudType::Quark,
            url: "https://pan.quark.cn/s/abc123".into(),
            password: None,
            note: "fixture".into(),
            datetime: None,
            source: Source::provider("fixture"),
            images: Vec::new(),
            check: Some(crate::core::CheckResult {
                state,
                cache_hit: false,
                checked_at: None,
                expires_at: None,
                summary: None,
            }),
        }
    }

    #[test]
    fn require_ok_emits_ok_and_filters_every_other_check_state() {
        let mut emitted = HashMap::new();
        let mut output = Vec::new();
        emit_link(
            &mut output,
            &mut emitted,
            "ok".into(),
            checked_link(crate::core::CheckState::Ok),
            OutputFormat::Jsonl,
            true,
        )
        .unwrap();
        assert_eq!(emitted.len(), 1);

        for state in [
            crate::core::CheckState::Bad,
            crate::core::CheckState::Locked,
            crate::core::CheckState::Uncertain,
            crate::core::CheckState::Unsupported,
        ] {
            emit_link(
                &mut output,
                &mut emitted,
                format!("{state:?}"),
                checked_link(state),
                OutputFormat::Jsonl,
                true,
            )
            .unwrap();
        }

        assert_eq!(emitted.len(), 1);
        assert_eq!(
            String::from_utf8(output)
                .unwrap()
                .lines()
                .filter(|line| !line.is_empty())
                .count(),
            1
        );
    }

    #[test]
    fn unchecked_mode_emits_results_without_check_metadata() {
        let mut link = checked_link(crate::core::CheckState::Bad);
        link.check = None;
        let mut emitted = HashMap::new();
        let mut output = Vec::new();
        emit_link(
            &mut output,
            &mut emitted,
            "unchecked".into(),
            link,
            OutputFormat::Jsonl,
            false,
        )
        .unwrap();

        let event: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(event["event"], "result");
        assert!(event["link"].get("check").is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn writes_stdout_before_slow_source_finishes() {
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
            let events = buffer.events();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0]["event"], "result");
        };
        let output = drive(
            engine,
            options,
            None,
            CheckOptions::default(),
            OutputFormat::Jsonl,
            false,
            true,
            None,
            buffer.clone(),
            futures::future::pending(),
        );
        let (result, _) = tokio::join!(output, observer);
        assert_eq!(result.unwrap(), EXIT_OK);
        assert_eq!(buffer.events().last().unwrap()["event"], "summary");
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
            OutputFormat::Jsonl,
            true,
            true,
            None,
            buffer.clone(),
            futures::future::pending(),
        )
        .await
        .unwrap();
        assert_eq!(code, EXIT_DEADLINE);
        let events = buffer.events();
        assert_eq!(events[0]["event"], "result");
        assert_eq!(events[0]["link"]["check"]["state"], "ok");
        let summary = &events.last().unwrap()["summary"];
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
            OutputFormat::Jsonl,
            true,
            true,
            None,
            buffer.clone(),
            futures::future::pending(),
        )
        .await
        .unwrap();
        assert_eq!(code, EXIT_OK);
        let events = buffer.events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["link"]["password"], "abcd");
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
            OutputFormat::Jsonl,
            false,
            true,
            None,
            buffer.clone(),
            interrupt,
        )
        .await
        .unwrap();
        assert_eq!(code, EXIT_INTERRUPTED);
        assert_eq!(buffer.events().len(), 1);
        assert_eq!(
            buffer.events()[0]["summary"]["finish_reason"],
            "interrupted"
        );
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
            OutputFormat::Jsonl,
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
