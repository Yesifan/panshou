//! Provider-boundary fixtures: wire paths, retry counts, partial delivery and cancellation.
#[tokio::test]
async fn miaoso_session_token_never_follows_cross_origin_redirects() {
    use wiremock::matchers::header;
    let origin = MockServer::start().await;
    let target = MockServer::start().await;
    let origin_url = Url::parse(&origin.uri()).unwrap();
    let target_url = Url::parse(&target.uri()).unwrap();
    assert_eq!(origin_url.host_str(), target_url.host_str());
    assert_ne!(origin_url.port(), target_url.port());
    Mock::given(|_: &wiremock::Request| true)
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&target)
        .await;
    let token = "caller-owned-redirect-fixture-token";
    let mut alternate_host = target_url.join("landing").unwrap();
    alternate_host.set_host(Some("localhost")).unwrap();
    for destination in [target_url.join("landing").unwrap(), alternate_host] {
        for status in [301, 302, 303, 307, 308] {
            origin.reset().await;
            Mock::given(path("/api/secendsearch"))
                .and(header("satoken", token))
                .respond_with(
                    ResponseTemplate::new(status).insert_header("location", destination.as_str()),
                )
                .expect(3)
                .mount(&origin)
                .await;
            let source = miaoso::Miaoso::with_options(
                origin_url.join("api/secendsearch").unwrap(),
                crate::http::ClientOptions::default(),
            )
            .with_token(token.into());
            // The context's ordinary client follows redirects; Miaoso must not use it.
            let ctx = context();
            let error = source.search(&ctx, "film").await.unwrap_err().to_string();
            assert!(error.contains(&status.to_string()), "{error}");
            assert!(!error.contains(token));
            assert!(!format!("{source:?}").contains(token));
            assert!(!ctx.has_results());
            origin.verify().await;
            assert!(target.received_requests().await.unwrap().is_empty());
        }
    }
    target.verify().await;
}

#[tokio::test]
async fn miaoso_token_client_preserves_explicit_proxy_options() {
    use crate::http::{ClientOptions, ProxyUrl};
    use wiremock::matchers::header;
    let origin = MockServer::start().await;
    let proxy = MockServer::start().await;
    Mock::given(|_: &wiremock::Request| true)
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&origin)
        .await;
    Mock::given(method("GET"))
        .and(header("satoken", "caller-owned-proxy-fixture-token"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("miaoso", "empty")))
        .expect(1)
        .mount(&proxy)
        .await;
    let source = miaoso::Miaoso::with_options(
        Url::parse(&format!("{}/api/secendsearch", origin.uri())).unwrap(),
        ClientOptions {
            proxy: Some(ProxyUrl::parse(&proxy.uri()).unwrap()),
            no_proxy: Some(String::new()),
            ..ClientOptions::default()
        },
    )
    .with_token("caller-owned-proxy-fixture-token".into());
    assert!(source.search(&context(), "film").await.unwrap().is_empty());
    assert!(origin.received_requests().await.unwrap().is_empty());
    origin.verify().await;
    proxy.verify().await;
}
use super::*;
use crate::{
    core::ProviderError,
    providers::{Provider, SearchContext},
};
use std::{sync::Arc, time::Duration};
use url::Url;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};
const NAMES: [&str; 8] = [
    "kkv", "labi", "leijing", "lingjisp", "lou1", "miaoso", "mikuclub", "miosou",
];
fn fixture(name: &str, kind: &str) -> &'static str {
    match (name, kind) {
        ("kkv", "success") => include_str!("../../tests/fixtures/providers/kkv/success.html"),
        ("kkv", "detail") => include_str!("../../tests/fixtures/providers/kkv/detail.html"),
        ("kkv", "empty") => include_str!("../../tests/fixtures/providers/kkv/empty.html"),
        ("labi", "success") => include_str!("../../tests/fixtures/providers/labi/success.html"),
        ("labi", "detail") => include_str!("../../tests/fixtures/providers/labi/detail.html"),
        ("labi", "empty") => include_str!("../../tests/fixtures/providers/labi/empty.html"),
        ("leijing", "success") => {
            include_str!("../../tests/fixtures/providers/leijing/success.html")
        }
        ("leijing", "detail") => include_str!("../../tests/fixtures/providers/leijing/detail.html"),
        ("leijing", "empty") => include_str!("../../tests/fixtures/providers/leijing/empty.html"),
        ("lingjisp", "success") => {
            include_str!("../../tests/fixtures/providers/lingjisp/success.json")
        }
        ("lingjisp", "detail") => {
            include_str!("../../tests/fixtures/providers/lingjisp/detail.json")
        }
        ("lingjisp", "empty") => include_str!("../../tests/fixtures/providers/lingjisp/empty.json"),
        ("lou1", "success") => include_str!("../../tests/fixtures/providers/lou1/success.json"),
        ("lou1", "detail") => include_str!("../../tests/fixtures/providers/lou1/detail.html"),
        ("lou1", "empty") => include_str!("../../tests/fixtures/providers/lou1/empty.json"),
        ("miaoso", "success") => include_str!("../../tests/fixtures/providers/miaoso/success.json"),
        ("miaoso", "partial") => include_str!("../../tests/fixtures/providers/miaoso/partial.json"),
        ("miaoso", "empty") => include_str!("../../tests/fixtures/providers/miaoso/empty.json"),
        ("mikuclub", "success") => {
            include_str!("../../tests/fixtures/providers/mikuclub/success.json")
        }
        ("mikuclub", "detail") => {
            include_str!("../../tests/fixtures/providers/mikuclub/detail.html")
        }
        ("mikuclub", "empty") => include_str!("../../tests/fixtures/providers/mikuclub/empty.json"),
        ("miosou", "success") => include_str!("../../tests/fixtures/providers/miosou/success.sse"),
        ("miosou", "detail") => include_str!("../../tests/fixtures/providers/miosou/detail.json"),
        ("miosou", "empty") => include_str!("../../tests/fixtures/providers/miosou/empty.sse"),
        (_, "malformed") => include_str!("../../tests/fixtures/providers/miosou/malformed.txt"),
        _ => panic!("missing fixture {name}/{kind}"),
    }
}
fn provider(name: &str, server: &MockServer) -> Arc<dyn Provider> {
    let base = Url::parse(&format!("{}/", server.uri())).unwrap();
    match name {
        "kkv" => Arc::new(kkv::Kkv::new(base)),
        "labi" => Arc::new(labi::Labi::new(vec![base])),
        "leijing" => Arc::new(leijing::Leijing::new(base)),
        "lingjisp" => Arc::new(lingjisp::Lingjisp::new(base)),
        "lou1" => Arc::new(lou1::Lou1::new(base)),
        "miaoso" => Arc::new(miaoso::Miaoso::new(base.join("api/secendsearch").unwrap())),
        "mikuclub" => Arc::new(mikuclub::Mikuclub::new(base)),
        "miosou" => Arc::new(miosou::Miosou::new(base)),
        _ => panic!("unknown provider"),
    }
}
fn search_path(name: &str) -> &str {
    match name {
        "kkv" => "/",
        "labi" => "/index.php/vod/search/wd/film.html",
        "leijing" => "/search",
        "lingjisp" => "/getVideoList",
        "lou1" => "/search/api/search.php",
        "miaoso" => "/api/secendsearch",
        "mikuclub" => "/wp-json/utils/v2/post_list",
        "miosou" => "/miov1/search",
        _ => unreachable!(),
    }
}
async fn gate(name: &str, server: &MockServer) {
    if name == "miosou" {
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>ready</html>"))
            .mount(server)
            .await;
    }
}
async fn search_mock(name: &str, server: &MockServer, response: ResponseTemplate, count: u64) {
    let mock = Mock::given(method("GET")).and(path(search_path(name)));
    let mock = match name {
        "kkv" => mock.and(query_param("s", "film")),
        "leijing" => mock.and(query_param("keyword", "film")),
        "lingjisp" => mock
            .and(query_param("sb", "film"))
            .and(query_param("app_id", "83768d9ad4"))
            .and(query_param("identity", "23734adac0301bccdcb107c4aa21f96c"))
            .and(query_param("limit", "20")),
        "lou1" => mock
            .and(query_param("q", "film"))
            .and(query_param("page_size", "12")),
        "miaoso" => mock
            .and(query_param("name", "film"))
            .and(query_param("pageNo", "1")),
        "mikuclub" => mock
            .and(query_param("pagename", "search_page"))
            .and(query_param("custom_orderby", "relevance"))
            .and(query_param("no_cache", "1")),
        "miosou" => mock.and(query_param("keyword", "film")),
        _ => mock,
    };
    let mock = mock.respond_with(response);
    if count > 0 {
        mock.expect(count).mount(server).await;
    } else {
        mock.mount(server).await;
    }
}
async fn detail_mock(name: &str, server: &MockServer, id: u8, fail: bool, delay: Duration) {
    if name == "miaoso" || (id == 7 && ["leijing", "miosou"].contains(&name)) {
        return;
    }
    let detail = match name {
        "kkv" => "/".into(),
        "labi" => format!("/index.php/vod/detail/id/{id}.html"),
        "leijing" => "/thread".into(),
        "lingjisp" => "/getVideoDetail".into(),
        "lou1" => format!("/thread-{id}.htm"),
        "mikuclub" => format!("/{id}.html"),
        "miosou" => "/miov1/share/detail".into(),
        _ => unreachable!(),
    };
    let mock = Mock::given(method("GET")).and(path(detail));
    let mock = match name {
        "kkv" => mock.and(query_param("p", id.to_string())),
        "leijing" => mock.and(query_param("topicId", id.to_string())),
        "lingjisp" => mock.and(query_param("id", id.to_string())),
        "miosou" => mock.and(query_param("share_ref", format!("fixture{id}"))),
        _ => mock,
    };
    let count = if fail {
        match name {
            "leijing" | "miosou" => 1,
            "mikuclub" => 2,
            _ => 3,
        }
    } else {
        1
    };
    let body = if name == "lingjisp" {
        let mut v: serde_json::Value = serde_json::from_str(fixture(name, "detail")).unwrap();
        v["data"]["id"] = id.into();
        v["data"]["doub_id"] = id.into();
        v.to_string()
    } else {
        fixture(name, "detail").into()
    };
    mock.respond_with(if fail {
        ResponseTemplate::new(503)
    } else {
        ResponseTemplate::new(200)
            .set_body_string(body)
            .set_delay(delay)
    })
    .expect(count)
    .mount(server)
    .await;
}
fn context() -> SearchContext {
    SearchContext::new(reqwest::Client::new(), Duration::from_secs(5))
}
#[tokio::test]
async fn eight_real_protocols_success_and_empty() {
    for name in NAMES {
        let server = MockServer::start().await;
        gate(name, &server).await;
        search_mock(
            name,
            &server,
            ResponseTemplate::new(200).set_body_string(fixture(name, "success")),
            if name == "mikuclub" { 2 } else { 1 },
        )
        .await;
        detail_mock(name, &server, 7, false, Duration::ZERO).await;
        detail_mock(name, &server, 8, false, Duration::ZERO).await;
        let source = provider(name, &server);
        let ctx = context();
        let rows = source
            .search(&ctx, "film")
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(!rows.is_empty(), "{name}");
        assert!(
            rows.iter()
                .all(|r| !r.links.is_empty() && r.links.iter().all(|l| l.work_title.is_some())),
            "{name}"
        );
        if name == "lingjisp" {
            assert_eq!(rows[0].links.len(), 2);
            assert_eq!(rows[0].links[1].password.as_deref(), Some("explicit"));
        }
        if name == "lou1" {
            assert!(
                rows[0]
                    .links
                    .iter()
                    .any(|l| l.url.contains("attach-download-"))
            );
        }
        if name == "kkv" {
            assert_eq!(rows[0].links[0].password.as_deref(), Some("a1b2"));
        }
        server.verify().await;
        let empty = MockServer::start().await;
        gate(name, &empty).await;
        search_mock(
            name,
            &empty,
            ResponseTemplate::new(200).set_body_string(fixture(name, "empty")),
            if name == "mikuclub" { 2 } else { 1 },
        )
        .await;
        assert!(
            provider(name, &empty)
                .search(&context(), "film")
                .await
                .unwrap()
                .is_empty(),
            "{name}"
        );
    }
}
#[tokio::test]
async fn eight_protocols_reject_malformed_and_preserve_non200_status_and_attempts() {
    for name in NAMES {
        let server = MockServer::start().await;
        gate(name, &server).await;
        search_mock(
            name,
            &server,
            ResponseTemplate::new(200).set_body_string(fixture(name, "malformed")),
            if name == "mikuclub" { 2 } else { 1 },
        )
        .await;
        assert!(
            provider(name, &server)
                .search(&context(), "film")
                .await
                .is_err(),
            "{name}"
        );
        server.verify().await;
        let server = MockServer::start().await;
        gate(name, &server).await;
        let attempts = match name {
            "leijing" | "miosou" => 1,
            "mikuclub" => 6,
            _ => 3,
        };
        search_mock(name, &server, ResponseTemplate::new(503), attempts).await;
        let error = provider(name, &server)
            .search(&context(), "film")
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("503"), "{name}: {error}");
        server.verify().await;
    }
}
#[tokio::test]
async fn eight_protocols_partial_failures_publish_and_deadlines_cancel() {
    for name in NAMES {
        let server = MockServer::start().await;
        gate(name, &server).await;
        let input = fixture(
            name,
            if name == "miaoso" {
                "partial"
            } else {
                "success"
            },
        );
        search_mock(
            name,
            &server,
            ResponseTemplate::new(200).set_body_string(input),
            if name == "mikuclub" { 2 } else { 1 },
        )
        .await;
        detail_mock(name, &server, 7, false, Duration::ZERO).await;
        detail_mock(name, &server, 8, true, Duration::ZERO).await;
        let ctx = context();
        assert!(
            provider(name, &server).search(&ctx, "film").await.is_err(),
            "{name}"
        );
        assert!(ctx.has_results(), "{name} dropped valid partial results");
        server.verify().await;
        let server = MockServer::start().await;
        gate(name, &server).await;
        search_mock(
            name,
            &server,
            ResponseTemplate::new(200)
                .set_body_string(fixture(name, "empty"))
                .set_delay(Duration::from_secs(2)),
            0,
        )
        .await;
        let ctx = context().for_source(tokio::time::Instant::now() + Duration::from_millis(200));
        assert!(
            matches!(
                provider(name, &server).search(&ctx, "film").await,
                Err(ProviderError::Timeout)
            ),
            "{name}"
        );
        assert!(!ctx.has_results());
    }
}
#[tokio::test]
async fn leijing_decodes_gzip_at_the_provider_boundary() {
    use std::io::Write;
    let server = MockServer::start().await;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(fixture("leijing", "success").as_bytes())
        .unwrap();
    search_mock(
        "leijing",
        &server,
        ResponseTemplate::new(200)
            .insert_header("content-encoding", "gzip")
            .set_body_bytes(encoder.finish().unwrap()),
        1,
    )
    .await;
    detail_mock("leijing", &server, 8, false, Duration::ZERO).await;
    assert_eq!(
        provider("leijing", &server)
            .search(&context(), "film")
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn miosou_terminal_errors_retain_preceding_valid_snapshot() {
    for tail in [
        "event: error\ndata: {\"message\":\"fixture error\"}\n\n",
        "event: snapshot\ndata: broken-json\n\n",
    ] {
        let server = MockServer::start().await;
        gate("miosou", &server).await;
        let body = format!(
            "event: snapshot\ndata: {{\"merged_by_type\":{{\"quark\":[{{\"url\":\"https://pan.quark.cn/s/fixture\",\"note\":\"film\"}}]}}}}\n\n{tail}"
        );
        search_mock(
            "miosou",
            &server,
            ResponseTemplate::new(200).set_body_string(body),
            1,
        )
        .await;
        let ctx = context();
        assert!(
            provider("miosou", &server)
                .search(&ctx, "film")
                .await
                .is_err()
        );
        assert!(ctx.has_results());
    }
}

#[tokio::test]
async fn miosou_anubis_cookie_session_and_snapshot_merge() {
    use sha2::{Digest, Sha256};
    let server = MockServer::start().await;
    Mock::given(path("/")).respond_with(ResponseTemplate::new(200).set_body_string(r#"<script id="anubis_challenge" type="application/json">{"rules":{"algorithm":"fast","difficulty":1},"challenge":{"id":"fixture-id","randomData":"fixture"}}</script>"#)).expect(1).mount(&server).await;
    let nonce = (0u64..1000)
        .find(|n| Sha256::digest(format!("fixture{n}"))[0] >> 4 == 0)
        .unwrap();
    Mock::given(path("/.within.website/x/cmd/anubis/api/pass-challenge"))
        .and(query_param("nonce", nonce.to_string()))
        .and(query_param("id", "fixture-id"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", "/passed")
                .insert_header("set-cookie", "anubis-auth=fixture; Path=/; HttpOnly"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/passed"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/miov1/search")).and(wiremock::matchers::header("cookie","anubis-auth=fixture")).respond_with(ResponseTemplate::new(200).set_body_string("event: snapshot\ndata: {\"merged_by_type\":{\"quark\":[{\"stream_key\":\"7\",\"url\":\"https://pan.quark.cn/s/fixture\",\"password\":\"a1b2\",\"note\":\"film\"}]}}\n\nevent: snapshot\ndata: {\"merged_by_type\":{\"quark\":[{\"stream_key\":\"7\",\"note\":\"film updated\"}]}}\n\n")).expect(1).mount(&server).await;
    let source = miosou::Miosou::new(Url::parse(&format!("{}/", server.uri())).unwrap());
    let rows = source.search(&context(), "film").await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "film updated");
    assert_eq!(rows[0].links[0].password.as_deref(), Some("a1b2"));
}
