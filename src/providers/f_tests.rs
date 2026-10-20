use super::*;
include!("f_partial_tests.rs");
include!("f_protocol_tests.rs");
use crate::{
    core::ProviderError,
    http::{ClientOptions, HttpClientFactory, RedirectPolicy},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use wiremock::{
    Mock, MockServer, Request, Respond, ResponseTemplate,
    matchers::{body_string_contains, header, method, path, query_param},
};
const QUARKTV_SEARCH: &str = include_str!("../../tests/fixtures/providers/f/quarktv-search.html");
const QUARKTV_DETAIL: &str = include_str!("../../tests/fixtures/providers/f/quarktv-detail.html");
const QUARKTV_API: &str = include_str!("../../tests/fixtures/providers/f/quarktv-api.json");
const QUPANSHE: &str = include_str!("../../tests/fixtures/providers/f/qupanshe-search.html");
const SDSO: &str = include_str!("../../tests/fixtures/providers/f/sdso.json");
const SHANDIAN_SEARCH: &str = include_str!("../../tests/fixtures/providers/f/shandian-search.html");
const SHANDIAN_DETAIL: &str = include_str!("../../tests/fixtures/providers/f/shandian-detail.html");
const SOUSOU_SEARCH: &str = include_str!("../../tests/fixtures/providers/f/sousou-search.html");
const SOUSOU_DETAIL: &str = include_str!("../../tests/fixtures/providers/f/sousou-detail.html");
const TPB: &str = include_str!("../../tests/fixtures/providers/f/thepiratebay.html");
const WUJI_SEARCH: &str = include_str!("../../tests/fixtures/providers/f/wuji-search.html");
const WUJI_DETAIL: &str = include_str!("../../tests/fixtures/providers/f/wuji-detail.html");
const XB6V_SEARCH: &str = include_str!("../../tests/fixtures/providers/f/xb6v-search.html");
const XB6V_DETAIL: &str = include_str!("../../tests/fixtures/providers/f/xb6v-detail.html");
#[derive(Clone, Copy, Debug)]
enum Site {
    Quarktv,
    Qupanshe,
    Sdso,
    Shandian,
    Sousou,
    Tpb,
    Wuji,
    Xb6v,
}
const SITES: [Site; 8] = [
    Site::Quarktv,
    Site::Qupanshe,
    Site::Sdso,
    Site::Shandian,
    Site::Sousou,
    Site::Tpb,
    Site::Wuji,
    Site::Xb6v,
];
fn options() -> ClientOptions {
    ClientOptions {
        timeout: Duration::from_secs(30),
        redirect: RedirectPolicy::None,
        no_proxy: Some("*".into()),
        ..ClientOptions::default()
    }
}
fn context() -> SearchContext {
    SearchContext::new(
        HttpClientFactory::new().client(&options()).unwrap(),
        Duration::from_secs(30),
    )
}
fn provider(site: Site, base: &str) -> Arc<dyn Provider> {
    let url: url::Url = base.parse().unwrap();
    match site {
        Site::Quarktv => Arc::new(quarktv::Quarktv { base: url }),
        Site::Qupanshe => Arc::new(qupanshe::Qupanshe::with_options(url, options())),
        Site::Sdso => Arc::new(sdso::Sdso {
            base: url,
            pages_per_type: 2,
        }),
        Site::Shandian => Arc::new(shandian::Shandian { bases: vec![url] }),
        Site::Sousou => Arc::new(sousou::Sousou {
            api: url.join("api.php").unwrap(),
            web: url.join("search").unwrap(),
        }),
        Site::Tpb => Arc::new(thepiratebay::Thepiratebay { base: url }),
        Site::Wuji => Arc::new(wuji::Wuji { base: url }),
        Site::Xb6v => Arc::new(xb6v::Xb6v::with_options(url, options())),
    }
}
async fn get(server: &MockServer, route: &str, body: impl Into<String>) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_string(body.into()))
        .mount(server)
        .await;
}
async fn setup(server: &MockServer, site: Site, empty: bool) {
    let blank = "<html><body>No results</body></html>";
    match site {
        Site::Quarktv => {
            get(server, "/", if empty { blank } else { QUARKTV_SEARCH }).await;
            get(server, "/1.html", QUARKTV_DETAIL).await;
            Mock::given(path("/wp-json/postkit/v1/pan_view"))
                .and(query_param("post_id", "1"))
                .respond_with(ResponseTemplate::new(200).set_body_string(QUARKTV_API))
                .mount(server)
                .await;
        }
        Site::Qupanshe => {
            Mock::given(method("GET"))
                .and(path("/"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("set-cookie", "session=fixture; Path=/")
                        .set_body_string(
                            "<html><input name='formhash' value='fixturehash'></html>",
                        ),
                )
                .mount(server)
                .await;
            Mock::given(method("POST"))
                .and(path("/search.php"))
                .and(query_param("mod", "forum"))
                .and(header("cookie", "session=fixture"))
                .and(body_string_contains("formhash=fixturehash"))
                .and(body_string_contains("srchtxt=movie"))
                .respond_with(
                    ResponseTemplate::new(302).insert_header("location", "/search.php?searchid=7"),
                )
                .mount(server)
                .await;
            Mock::given(method("GET"))
                .and(path("/search.php"))
                .and(query_param("searchid", "7"))
                .and(header("cookie", "session=fixture"))
                .respond_with(ResponseTemplate::new(200).set_body_string(if empty {
                    blank
                } else {
                    QUPANSHE
                }))
                .mount(server)
                .await;
        }
        Site::Sdso => {
            get(
                server,
                "/api/sd/search",
                if empty {
                    r#"{"code":200,"data":{"list":[]}}"#
                } else {
                    SDSO
                },
            )
            .await
        }
        Site::Shandian => {
            get(
                server,
                "/index.php/vod/search/wd/movie.html",
                if empty { blank } else { SHANDIAN_SEARCH },
            )
            .await;
            get(server, "/index.php/vod/detail/id/1.html", SHANDIAN_DETAIL).await;
        }
        Site::Sousou => {
            get(server, "/search", if empty { blank } else { SOUSOU_SEARCH }).await;
            get(server, "/doc/1", SOUSOU_DETAIL).await;
            get(server, "/api.php", r#"{"code":200,"data":{"list":[]}}"#).await;
        }
        Site::Tpb => {
            get(
                server,
                "/search/movie/1/99/0",
                if empty { blank } else { TPB },
            )
            .await
        }
        Site::Wuji => {
            Mock::given(path("/search"))
                .respond_with(ResponseTemplate::new(200).set_body_string(blank))
                .with_priority(10)
                .mount(server)
                .await;
            Mock::given(path("/search"))
                .and(query_param("page", "1"))
                .respond_with(ResponseTemplate::new(200).set_body_string(if empty {
                    blank
                } else {
                    WUJI_SEARCH
                }))
                .with_priority(2)
                .mount(server)
                .await;
            get(server, "/hash/1", WUJI_DETAIL).await;
        }
        Site::Xb6v => {
            Mock::given(method("POST"))
                .and(path("/e/search/11index.php"))
                .and(body_string_contains("keyboard=movie"))
                .respond_with(
                    ResponseTemplate::new(302).insert_header("location", "result/?searchid=7"),
                )
                .mount(server)
                .await;
            get(
                server,
                "/e/search/result/",
                if empty { blank } else { XB6V_SEARCH },
            )
            .await;
            get(server, "/movie/1.html", XB6V_DETAIL).await;
        }
    }
}
#[tokio::test]
async fn all_eight_success_empty_and_item_metadata() {
    for site in SITES {
        for empty in [false, true] {
            let server = MockServer::start().await;
            setup(&server, site, empty).await;
            let rows = provider(site, &format!("{}/", server.uri()))
                .search(&context(), "movie")
                .await
                .unwrap_or_else(|e| panic!("{site:?}: {e}"));
            assert_eq!(rows.is_empty(), empty, "{site:?}");
            if !empty {
                assert!(
                    rows.iter()
                        .all(|r| r.title.contains("movie") && !r.links.is_empty())
                );
                assert!(
                    rows.iter()
                        .flat_map(|r| &r.links)
                        .all(|l| l.cloud_type != crate::core::CloudType::Others)
                );
                match site {
                    Site::Quarktv => {
                        assert_eq!(rows[0].images.len(), 2);
                        assert_eq!(rows[0].links.len(), 1);
                        assert_eq!(rows[0].links[0].password.as_deref(), Some("a1b2"));
                    }
                    Site::Qupanshe => assert!(rows[0].content.contains("author")),
                    Site::Sdso => assert!(rows[0].datetime.is_some()),
                    Site::Shandian => {
                        assert_eq!(rows[0].links.len(), 1);
                        assert!(rows[0].content.contains("Director"));
                    }
                    Site::Sousou => {
                        assert_eq!(rows[0].title, "movie 4K");
                        assert_eq!(rows[0].links[0].password.as_deref(), Some("a1b2"));
                    }
                    Site::Tpb => {
                        assert!(rows[0].content.contains("1.5GiB"));
                        assert_eq!(
                            rows[0].datetime.unwrap().date_naive().to_string(),
                            "2023-10-30"
                        );
                    }
                    Site::Wuji => {
                        assert_eq!(rows[0].title, "movie");
                        assert!(rows[0].content.contains("movie.mkv"));
                    }
                    Site::Xb6v => {
                        assert_eq!(rows[0].title, "movie-4K");
                        assert_eq!(
                            rows[0].datetime.unwrap().date_naive().to_string(),
                            "2026-01-02"
                        );
                    }
                }
            }
            server.verify().await;
        }
    }
}
#[tokio::test]
async fn malformed_and_non_200_are_not_empty_success() {
    for site in SITES {
        for status in [200, 503] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(status).set_body_string("not HTML or JSON"))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(status).set_body_string("not HTML or JSON"))
                .mount(&server)
                .await;
            let answer = provider(site, &format!("{}/", server.uri()))
                .search(&context(), "movie")
                .await;
            assert!(answer.is_err(), "{site:?} status {status}");
            if status == 503 {
                assert!(answer.unwrap_err().to_string().contains("503"), "{site:?}");
            }
        }
    }
}
#[tokio::test]
async fn cancelling_drops_transport_and_prevents_retries() {
    for site in SITES {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503).set_delay(Duration::from_secs(60)))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).set_delay(Duration::from_secs(60)))
            .mount(&server)
            .await;
        let provider = provider(site, &format!("{}/", server.uri()));
        let ctx = context().for_source(tokio::time::Instant::now() + Duration::from_millis(30));
        // Require the source's own deadline to fail, not merely the watchdog.
        // Tokio's outer timeout polls its inner future first, so Ok(Err(Timeout))
        // is a correct cancellation outcome that the old assertion rejected.
        let answer = tokio::time::timeout(Duration::from_secs(2), provider.search(&ctx, "movie"))
            .await
            .unwrap_or_else(|_| panic!("{site:?}: ignored source deadline"));
        assert!(
            matches!(answer, Err(ProviderError::Timeout)),
            "{site:?}: {answer:?}"
        );
        let requests = server.received_requests().await.unwrap();
        let count = requests.len();
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(server.received_requests().await.unwrap().len(), count);
        if matches!(site, Site::Sdso) {
            // Four types x two pages are distinct initial requests, not retries.
            assert!(count <= 8, "{site:?}");
            let identities = requests
                .iter()
                .map(|request| request.url.as_str())
                .collect::<std::collections::HashSet<_>>();
            assert_eq!(
                identities.len(),
                count,
                "{site:?}: retried a cancelled page"
            );
        } else {
            assert!(count <= 1, "{site:?}");
        }
        assert!(!ctx.has_results());
    }
}
#[tokio::test]
async fn caller_drop_cancels_inflight_requests_without_later_retries() {
    for site in SITES {
        let server = MockServer::start().await;
        let seen = Arc::new(tokio::sync::Notify::new());
        for verb in ["GET", "POST"] {
            let seen = seen.clone();
            Mock::given(method(verb))
                .respond_with(move |_: &Request| {
                    seen.notify_one();
                    ResponseTemplate::new(503).set_delay(Duration::from_millis(50))
                })
                .mount(&server)
                .await;
        }
        let source = provider(site, &format!("{}/", server.uri()));
        let ctx = context();
        let mut pending = Box::pin(source.search(&ctx, "movie"));
        tokio::select! {
            biased;
            () = seen.notified() => {}
            answer = &mut pending => panic!("{site:?}: completed before in-flight cancellation: {answer:?}"),
        }
        drop(pending);
        // Include both the delayed 503 and the first 200 ms retry interval.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let requests = server.received_requests().await.unwrap();
        assert!(!requests.is_empty(), "{site:?}");
        let identities = requests
            .iter()
            .map(|request| request.url.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            identities.len(),
            requests.len(),
            "{site:?}: retried after caller drop"
        );
        assert!(
            requests.len() <= if matches!(site, Site::Sdso) { 8 } else { 1 },
            "{site:?}"
        );
        assert!(!ctx.has_results());
    }
}
#[tokio::test(start_paused = true)]
async fn completed_after_deadline_is_timeout_not_empty_success() {
    let ctx = context().for_source(tokio::time::Instant::now() + Duration::from_secs(1));
    let jobs = futures::stream::once(async {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let mut row = crate::core::SearchResult::provider("late", "sdso");
        row.links
            .push(crate::core::Link::new("https://pan.quark.cn/s/late"));
        Ok(vec![row])
    });
    assert!(matches!(
        support::collect(&ctx, jobs).await,
        Err(ProviderError::Timeout)
    ));
    assert!(!ctx.has_results());
}
#[tokio::test]
async fn sousou_passwords_respect_dom_boundaries_and_url_precedence() {
    for (search, detail, expected) in [
        (SOUSOU_SEARCH.to_owned(), SOUSOU_DETAIL.to_owned(), "a1b2"),
        (
            SOUSOU_SEARCH.replace("提取码：a1b2", ""),
            SOUSOU_DETAIL.replace(
                "<div class=\"resource-files\">",
                "<p>提取码：<b>a1b2</b></p><div class=\"resource-files\">",
            ),
            "a1b2",
        ),
        (
            SOUSOU_SEARCH.to_owned(),
            SOUSOU_DETAIL.replace(
                "https://pan.quark.cn/s/movie",
                "https://pan.quark.cn/s/movie?pwd=z9y8",
            ),
            "z9y8",
        ),
    ] {
        let server = MockServer::start().await;
        setup(&server, Site::Sousou, false).await;
        Mock::given(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_string(search))
            .with_priority(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/doc/1"))
            .respond_with(ResponseTemplate::new(200).set_body_string(detail))
            .with_priority(1)
            .expect(1)
            .mount(&server)
            .await;
        let rows = provider(Site::Sousou, &format!("{}/", server.uri()))
            .search(&context(), "movie")
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].links[0].password.as_deref(), Some(expected));
    }
}
#[tokio::test]
async fn expired_source_never_starts_http() {
    for site in SITES {
        let server = MockServer::start().await;
        let ctx = context().for_source(tokio::time::Instant::now());
        assert!(matches!(
            provider(site, &format!("{}/", server.uri()))
                .search(&ctx, "movie")
                .await,
            Err(ProviderError::Timeout)
        ));
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
struct RetryThenSuccess {
    calls: Arc<AtomicUsize>,
}
impl Respond for RetryThenSuccess {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        if self.calls.fetch_add(1, Ordering::SeqCst) < 2 {
            ResponseTemplate::new(503)
        } else {
            ResponseTemplate::new(200).set_body_string(QUARKTV_SEARCH)
        }
    }
}
#[tokio::test]
async fn quarktv_retries_status_and_bounded_read_failures() {
    let server = MockServer::start().await;
    setup(&server, Site::Quarktv, false).await;
    let calls = Arc::new(AtomicUsize::new(0));
    Mock::given(path("/"))
        .respond_with(RetryThenSuccess {
            calls: calls.clone(),
        })
        .with_priority(1)
        .mount(&server)
        .await;
    assert!(
        !provider(Site::Quarktv, &format!("{}/", server.uri()))
            .search(&context(), "movie")
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    server.reset().await;
    Mock::given(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; support::LIMIT + 1]))
        .expect(3)
        .mount(&server)
        .await;
    assert!(
        provider(Site::Quarktv, &format!("{}/", server.uri()))
            .search(&context(), "movie")
            .await
            .is_err()
    );
    server.verify().await;
}
#[tokio::test]
async fn wuji_does_not_retry_http_status() {
    let server = MockServer::start().await;
    Mock::given(path("/search"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    assert!(
        provider(Site::Wuji, &format!("{}/", server.uri()))
            .search(&context(), "movie")
            .await
            .unwrap_err()
            .to_string()
            .contains("503")
    );
    server.verify().await;
}
#[tokio::test]
async fn xb6v_javascript_redirect_and_sousou_api_fallback() {
    let server = MockServer::start().await;
    setup(&server, Site::Xb6v, false).await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<script>window.location.href='result/?searchid=7';</script>"),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    assert!(
        !provider(Site::Xb6v, &format!("{}/", server.uri()))
            .search(&context(), "movie")
            .await
            .unwrap()
            .is_empty()
    );
    server.reset().await;
    get(&server, "/search", "<html>no web documents</html>").await;
    Mock::given(path("/api.php"))
        .and(query_param("action", "search"))
        .and(query_param("per_size", "30"))
        .respond_with(ResponseTemplate::new(200).set_body_string(include_str!(
            "../../tests/fixtures/providers/f/sousou-api.json"
        )))
        .expect(18)
        .mount(&server)
        .await;
    let rows = provider(Site::Sousou, &format!("{}/", server.uri()))
        .search(&context(), "movie")
        .await
        .unwrap();
    assert!(!rows.is_empty());
    assert_eq!(rows[0].links[0].password.as_deref(), Some("a1b2"));
    server.verify().await;
}
