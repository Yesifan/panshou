//! Request-boundary contracts for every Group A2 source; no live network.
use super::*;
use url::Url;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path, query_param},
};

const NAMES: [&str; 8] = [
    "5266ys",
    "ahhhhfs",
    "alupan",
    "bixin",
    "daishudj",
    "diduan",
    "discourse",
    "duoduo",
];
fn provider(name: &str, base: Url, pages: usize) -> Box<dyn Provider> {
    match name {
        "5266ys" => Box::new(ys5266::Ys5266Provider::new(base)),
        "ahhhhfs" => Box::new(ahhhhfs::AhhhhfsProvider::new(base)),
        "alupan" => Box::new(alupan::AlupanProvider::new(base)),
        "bixin" => Box::new(bixin::BixinProvider::new(base)),
        "daishudj" => Box::new(daishudj::DaishudjProvider::new(base)),
        "diduan" => Box::new(diduan::DiduanProvider::new(base)),
        "discourse" => Box::new(discourse::DiscourseProvider::new(base).with_max_pages(pages)),
        "duoduo" => Box::new(duoduo::DuoduoProvider::new(base)),
        _ => unreachable!(),
    }
}
fn search_body(name: &str) -> String {
    match name {
        "5266ys" => include_str!("../../tests/fixtures/providers/5266ys/success.html")
            .replace("测试", "Test"),
        "ahhhhfs" => include_str!("../../tests/fixtures/providers/ahhhhfs/search.html").into(),
        "alupan" => include_str!("../../tests/fixtures/providers/alupan/search.html").into(),
        "bixin" => include_str!("../../tests/fixtures/providers/bixin/success.json")
            .replace("测试", "Test"),
        "daishudj" => include_str!("../../tests/fixtures/providers/daishudj/search.html").into(),
        "diduan" => include_str!("../../tests/fixtures/providers/diduan/search.html").into(),
        "discourse" => include_str!("../../tests/fixtures/providers/discourse/search.json").into(),
        "duoduo" => include_str!("../../tests/fixtures/providers/duoduo/search.html").into(),
        _ => unreachable!(),
    }
}
fn detail_body(name: &str) -> String {
    match name {
        "5266ys" => include_str!("../../tests/fixtures/providers/5266ys/detail.html").into(),
        "ahhhhfs" => include_str!("../../tests/fixtures/providers/ahhhhfs/detail.html").into(),
        "alupan" => include_str!("../../tests/fixtures/providers/alupan/detail.html").into(),
        "daishudj" => include_str!("../../tests/fixtures/providers/daishudj/detail.html").into(),
        "diduan" => include_str!("../../tests/fixtures/providers/diduan/detail.html").into(),
        "duoduo" => include_str!("../../tests/fixtures/providers/duoduo/detail.html").into(),
        _ => unreachable!(),
    }
}
fn search_mock(name: &str) -> wiremock::MockBuilder {
    match name {
        "5266ys" => Mock::given(method("POST"))
            .and(path("/e/search/index.php"))
            .and(header("content-type", "application/x-www-form-urlencoded")),
        "bixin" => Mock::given(method("GET"))
            .and(path("/api/discussions"))
            .and(query_param("filter[q]", "Test"))
            .and(query_param("page[limit]", "50"))
            .and(query_param("include", "mostRelevantPost")),
        "discourse" => Mock::given(method("GET"))
            .and(path("/search.json"))
            .and(query_param("q", "Test in:title #resource")),
        "diduan" => Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "Test")),
        "duoduo" => Mock::given(method("GET")).and(path("/index.php/vod/search/wd/Test.html")),
        _ => Mock::given(method("GET"))
            .and(path("/"))
            .and(query_param("s", "Test")),
    }
}
fn response(name: &str, body: String) -> ResponseTemplate {
    let bytes = if name == "5266ys" {
        encoding_rs::GB18030.encode(&body).0.into_owned()
    } else {
        body.into_bytes()
    };
    ResponseTemplate::new(200).set_body_bytes(bytes)
}
fn detail_path(name: &str, second: bool) -> &'static str {
    match (name, second) {
        ("5266ys", false) => "/2026/123.html",
        ("5266ys", true) => "/2026/124.html",
        ("alupan", _) => "/",
        ("diduan", false) => "/movie/test",
        ("diduan", true) => "/movie/slow",
        ("duoduo", false) => "/index.php/vod/detail/id/123.html",
        ("duoduo", true) => "/index.php/vod/detail/id/124.html",
        (_, false) => "/123/",
        (_, true) => "/124/",
    }
}
async fn mount_detail(
    server: &MockServer,
    name: &str,
    second: bool,
    template: ResponseTemplate,
    expected: u64,
) {
    let mut mock = Mock::given(method("GET")).and(path(detail_path(name, second)));
    if name == "alupan" {
        mock = mock.and(query_param("p", if second { "124" } else { "123" }));
    }
    mock.respond_with(template)
        .expect(expected)
        .mount(server)
        .await;
}
fn context() -> SearchContext {
    SearchContext::new(reqwest::Client::new(), Duration::from_secs(5))
}
#[tokio::test]
async fn all_sources_success_and_protocol() {
    for name in NAMES {
        let server = MockServer::start().await;
        search_mock(name)
            .respond_with(response(name, search_body(name)))
            .expect(if name == "bixin" { 2 } else { 1 })
            .mount(&server)
            .await;
        if name != "bixin" && name != "discourse" {
            mount_detail(&server, name, false, response(name, detail_body(name)), 1).await;
        }
        let ctx = context();
        let rows = provider(name, Url::parse(&server.uri()).unwrap(), 1)
            .search(&ctx, "Test")
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(rows.len(), 1, "{name}");
        assert!(!rows[0].links.is_empty(), "{name}");
        assert!(ctx.has_results(), "{name}");
        if name != "5266ys" && name != "bixin" {
            assert_eq!(rows[0].links[0].password.as_deref(), Some("QWER"), "{name}");
        }
        if name == "5266ys" {
            let requests = server.received_requests().await.unwrap();
            let body = String::from_utf8_lossy(&requests[0].body);
            assert!(body.contains("keyboard=Test"));
            assert!(body.contains("show=title%2Csmalltext"));
        }
    }
}
#[tokio::test]
async fn all_sources_empty_and_malformed() {
    for name in NAMES {
        for malformed in [false, true] {
            let server = MockServer::start().await;
            let body = if malformed {
                "not a valid document".into()
            } else {
                match name {
                    "bixin" => "{\"data\":[],\"included\":[]}".into(),
                    "discourse" => "{\"posts\":[],\"topics\":[]}".into(),
                    "5266ys" => "<div class=\"listBox\"></div>".into(),
                    _ => "<html><body>No results</body></html>".into(),
                }
            };
            search_mock(name)
                .respond_with(response(name, body))
                .expect(if name == "bixin" { 2 } else { 1 })
                .mount(&server)
                .await;
            let result = provider(name, Url::parse(&server.uri()).unwrap(), 1)
                .search(&context(), "Test")
                .await;
            if malformed {
                assert!(result.is_err(), "{name}");
            } else {
                assert!(result.unwrap().is_empty(), "{name}");
            }
        }
    }
}
#[tokio::test]
async fn all_sources_non200_exact_retry_count() {
    for name in NAMES {
        let server = MockServer::start().await;
        let count = match name {
            "5266ys" | "diduan" | "discourse" => 1,
            "bixin" => 6,
            _ => 3,
        };
        search_mock(name)
            .respond_with(ResponseTemplate::new(502))
            .expect(count)
            .mount(&server)
            .await;
        let error = provider(name, Url::parse(&server.uri()).unwrap(), 1)
            .search(&context(), "Test")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("502"), "{name}: {error}");
    }
}
async fn partial(name: &str, cancel: bool, http_failure: bool) {
    let server = MockServer::start().await;
    let slow = if cancel {
        ResponseTemplate::new(200)
            .set_delay(Duration::from_millis(500))
            .set_body_string("broken")
    } else if http_failure {
        ResponseTemplate::new(502)
    } else {
        ResponseTemplate::new(200).set_body_string("broken")
    };
    let failure_attempts = if http_failure {
        match name {
            "alupan" => 2,
            "bixin" | "daishudj" | "duoduo" => 3,
            _ => 1,
        }
    } else {
        1
    };
    if name == "bixin" {
        search_mock(name)
            .and(query_param("page[offset]", "0"))
            .respond_with(response(name, search_body(name)))
            .expect(1)
            .mount(&server)
            .await;
        search_mock(name)
            .and(query_param("page[offset]", "50"))
            .respond_with(slow)
            .expect(failure_attempts)
            .mount(&server)
            .await;
    } else if name == "discourse" {
        search_mock(name)
            .and(query_param("page", "1"))
            .respond_with(response(name, search_body(name)))
            .expect(1)
            .mount(&server)
            .await;
        search_mock(name)
            .and(query_param("page", "2"))
            .respond_with(slow)
            .expect(if cancel { 0 } else { 1 })
            .mount(&server)
            .await;
    } else {
        let body = search_body(name);
        let second = body
            .replace("123", "124")
            .replace("/movie/test", "/movie/slow");
        let body = if name == "diduan" {
            body.replace("</section>", "<div class=\"movie-card\"><h3><a href=\"/movie/slow\">Test slow</a></h3></div></section>")
        } else {
            format!("{body}{second}")
        };
        search_mock(name)
            .respond_with(response(name, body))
            .expect(1)
            .mount(&server)
            .await;
        mount_detail(&server, name, false, response(name, detail_body(name)), 1).await;
        mount_detail(&server, name, true, slow, failure_attempts).await;
    }
    let base = context();
    let ctx = if cancel {
        base.for_source(tokio::time::Instant::now() + Duration::from_millis(150))
    } else {
        base
    };
    let result = provider(name, Url::parse(&server.uri()).unwrap(), 2)
        .search(&ctx, "Test")
        .await;
    assert!(
        result.is_err(),
        "{name}: must not report partial as completed"
    );
    if http_failure {
        assert!(
            result.as_ref().unwrap_err().to_string().contains("502"),
            "{name}: {result:?}"
        );
    }
    let rows = ctx.finish(Vec::new());
    assert_eq!(rows.len(), 1, "{name}: retain fast increment");
    assert!(!ctx.publish(Vec::new()));
    server.verify().await;
}
#[tokio::test]
async fn all_sources_partial_failure_retains_increment() {
    for name in NAMES {
        partial(name, false, false).await;
    }
}
#[tokio::test]
async fn all_sources_partial_http_failure_is_failed_not_completed() {
    for name in NAMES {
        partial(name, false, true).await;
    }
}
#[tokio::test]
async fn all_sources_deadline_retains_fast_increment() {
    for name in NAMES {
        partial(name, true, false).await;
    }
}
#[tokio::test]
async fn source_detail_status_retries_are_not_search_defaults() {
    for name in [
        "5266ys", "ahhhhfs", "alupan", "daishudj", "diduan", "duoduo",
    ] {
        let server = MockServer::start().await;
        search_mock(name)
            .respond_with(response(name, search_body(name)))
            .expect(1)
            .mount(&server)
            .await;
        let attempts = match name {
            "alupan" => 2,
            "daishudj" | "duoduo" => 3,
            _ => 1,
        };
        mount_detail(&server, name, false, ResponseTemplate::new(502), attempts).await;
        let ctx = context();
        let error = provider(name, Url::parse(&server.uri()).unwrap(), 1)
            .search(&ctx, "Test")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("502"), "{name}: {error}");
        assert!(!ctx.has_results());
    }
}
#[tokio::test]
async fn cancelled_sources_do_not_start_requests() {
    for name in NAMES {
        let server = MockServer::start().await;
        let ctx = context().for_source(tokio::time::Instant::now());
        assert!(
            provider(name, Url::parse(&server.uri()).unwrap(), 1)
                .search(&ctx, "Test")
                .await
                .is_err(),
            "{name}"
        );
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "{name}"
        );
    }
}
#[tokio::test]
async fn bounded_http_helper_rejects_limit_plus_one() {
    for size in [8, 9] {
        let server = MockServer::start().await;
        Mock::given(path("/bounded"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; size]))
            .expect(1)
            .mount(&server)
            .await;
        let ctx = context();
        let result = fetch(
            &ctx,
            ctx.client.get(format!("{}/bounded", server.uri())),
            "a2",
            "bounded",
            Duration::from_secs(1),
            8,
            &[],
        )
        .await;
        assert_eq!(result.is_ok(), size == 8);
    }
}
#[tokio::test]
async fn only_bixin_retries_body_read_failures() {
    for (name, stage, delays, attempts) in [
        ("ahhhhfs", "search", &[200, 400][..], 1),
        ("alupan", "detail", &[200][..], 1),
        ("daishudj", "detail", &[200, 400][..], 1),
        ("duoduo", "detail", &[200, 400][..], 1),
        ("bixin", "page", &[500, 500][..], 3),
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/body"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 9]))
            .expect(attempts)
            .mount(&server)
            .await;
        let ctx = context();
        assert!(
            fetch(
                &ctx,
                ctx.client.get(format!("{}/body", server.uri())),
                name,
                stage,
                Duration::from_secs(5),
                8,
                delays
            )
            .await
            .is_err()
        );
        server.verify().await;
    }
}
#[tokio::test]
async fn discourse_duplicate_page_does_not_hide_later_new_posts() {
    let server = MockServer::start().await;
    for page in [1, 2, 3] {
        let mut body: serde_json::Value = serde_json::from_str(&search_body("discourse")).unwrap();
        if page == 3 {
            body["posts"][0]["id"] = 2.into();
            body["posts"][0]["blurb"] = "Test https://pan.quark.cn/s/new?pwd=ASDF".into();
            body["grouped_search_result"]["more_full_page_results"] = false.into();
        }
        search_mock("discourse")
            .and(query_param("page", page.to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
    }
    let ctx = context();
    let rows = provider("discourse", Url::parse(&server.uri()).unwrap(), 3)
        .search(&ctx, "Test")
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(ctx.finish(vec![]).len(), 2);
    server.verify().await;
}
#[tokio::test]
async fn discourse_does_not_borrow_the_next_links_password() {
    let server = MockServer::start().await;
    search_mock("discourse").respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "posts":[{"id":1,"topic_id":1,"blurb":"Test https://pan.quark.cn/s/first https://pan.baidu.com/s/second 提取码 QWER"}],
        "topics":[{"id":1,"title":"Test"}]
    }))).expect(1).mount(&server).await;
    let rows = provider("discourse", Url::parse(&server.uri()).unwrap(), 1)
        .search(&context(), "Test")
        .await
        .unwrap();
    assert_eq!(rows[0].links[0].password, None);
    assert_eq!(rows[0].links[1].password.as_deref(), Some("QWER"));
}
#[tokio::test]
async fn alupan_does_not_assign_a_siblings_code_from_shared_parent() {
    let server = MockServer::start().await;
    search_mock("alupan")
        .respond_with(response("alupan", search_body("alupan")))
        .expect(1)
        .mount(&server)
        .await;
    mount_detail(&server,"alupan",false,ResponseTemplate::new(200).set_body_string(
        "<div class='article-content'><p><a href='https://pan.quark.cn/s/first'>quark</a><a href='https://alipan.com/s/second?pwd=REAL'>ali</a>提取码 QWER</p></div>"
    ),1).await;
    let rows = provider("alupan", Url::parse(&server.uri()).unwrap(), 1)
        .search(&context(), "Test")
        .await
        .unwrap();
    assert_eq!(rows[0].links[0].password, None);
    assert_eq!(rows[0].links[1].password.as_deref(), Some("REAL"));
}
#[tokio::test]
async fn discourse_topic_boundary() {
    let server = MockServer::start().await;
    Mock::given(path("/t/123.json"))
        .and(query_param("track_visit", "true"))
        .and(query_param("forceLoad", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"post_stream":{"posts":[{"cooked":"<p>https://pan.quark.cn/s/abc</p>"}]}}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;
    let rows = discourse::DiscourseProvider::new(Url::parse(&server.uri()).unwrap())
        .topic_detail(&context(), 123)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
}
