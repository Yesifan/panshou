use super::*;
use crate::{
    http::{ClientOptions, HttpClientFactory},
    providers::SearchContext,
};
use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

fn client() -> reqwest::Client {
    HttpClientFactory::new()
        .client(&ClientOptions::default())
        .unwrap()
}

#[tokio::test]
async fn request_retries_status_and_preserves_exhausted_status_without_final_sleep() {
    let server = MockServer::start().await;
    let hits = Arc::new(AtomicUsize::new(0));
    let count = hits.clone();
    Mock::given(path("/retry"))
        .respond_with(move |_: &wiremock::Request| {
            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(200).set_body_string("ok")
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path("/failed"))
        .respond_with(ResponseTemplate::new(403))
        .expect(3)
        .mount(&server)
        .await;
    Mock::given(path("/once"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;
    let client = client();
    assert_eq!(
        request(
            || client.get(format!("{}/retry", server.uri())),
            3,
            Duration::from_millis(1),
            8
        )
        .await
        .unwrap(),
        b"ok"
    );
    let error = request(
        || client.get(format!("{}/failed", server.uri())),
        3,
        Duration::from_millis(1),
        8,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("403"));
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        request(
            || client.get(format!("{}/once", server.uri())),
            1,
            Duration::from_secs(3600),
            8,
        ),
    )
    .await
    .unwrap();
    assert!(result.unwrap_err().to_string().contains("404"));
}

#[tokio::test]
async fn exact_limit_succeeds_overflow_and_invalid_gzip_are_not_retried() {
    let server = MockServer::start().await;
    for (name, bytes) in [("exact", vec![b'x'; 1024]), ("over", vec![b'x'; 1025])] {
        Mock::given(path(format!("/{name}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(path("/badgzip"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-encoding", "gzip")
                .set_body_bytes(b"not gzip".to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = client();
    assert_eq!(
        request(
            || client.get(format!("{}/exact", server.uri())),
            3,
            Duration::ZERO,
            1024
        )
        .await
        .unwrap()
        .len(),
        1024
    );
    for name in ["over", "badgzip"] {
        assert!(
            request(
                || client.get(format!("{}/{name}", server.uri())),
                3,
                Duration::ZERO,
                1024
            )
            .await
            .is_err()
        );
    }
}

#[tokio::test]
async fn gzip_uses_decoded_source_limit_and_cancellation_stops_retry_backoff() {
    let server = MockServer::start().await;
    for (name, len) in [("exact", 1024), ("over", 1025)] {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&vec![b'x'; len]).unwrap();
        Mock::given(path(format!("/{name}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-encoding", "gzip")
                    .set_body_bytes(encoder.finish().unwrap()),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(path("/retry"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let client = client();
    assert_eq!(
        request(
            || client.get(format!("{}/exact", server.uri())),
            1,
            Duration::ZERO,
            1024
        )
        .await
        .unwrap()
        .len(),
        1024
    );
    assert!(
        request(
            || client.get(format!("{}/over", server.uri())),
            1,
            Duration::ZERO,
            1024
        )
        .await
        .is_err()
    );
    assert!(
        tokio::time::timeout(
            Duration::from_millis(100),
            request(
                || client.get(format!("{}/retry", server.uri())),
                3,
                Duration::from_secs(60),
                1024
            )
        )
        .await
        .is_err()
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
}

#[tokio::test]
async fn single_xinjuc_source_failed_sibling_retains_links_and_exits_three() {
    use crate::{
        providers::c_registry::xinjuc::{Endpoints, Xinjuc},
        search::{SearchEngine, SearchOptions},
    };
    let server = MockServer::start().await;
    Mock::given(path("/")).respond_with(ResponseTemplate::new(200).set_body_string(r#"<div class="row-xs post-list"><article class="post-item"><div class="post-image"><a href="/1.html"></a></div><h5 class="post-title"><a>movie good</a></h5></article><article class="post-item"><div class="post-image"><a href="/2.html"></a></div><h5 class="post-title"><a>movie bad</a></h5></article></div>"#)).mount(&server).await;
    Mock::given(path("/1.html")).respond_with(ResponseTemplate::new(200).set_body_string(r#"<div class="article-content"><a href="https://pan.baidu.com/s/1234567890abcdef">share</a></div>"#)).expect(1).mount(&server).await;
    Mock::given(path("/2.html"))
        .respond_with(ResponseTemplate::new(503).set_delay(Duration::from_millis(30)))
        .expect(1)
        .mount(&server)
        .await;
    let ctx = SearchContext::new(client(), Duration::from_secs(2));
    let engine = SearchEngine::new(ctx);
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let (_cancel, cancellation) = tokio::sync::watch::channel(false);
    let (outcome, summary) = engine
        .search_stream(
            SearchOptions {
                query: "movie".into(),
                providers: vec![Arc::new(Xinjuc::new(Endpoints {
                    base: url::Url::parse(&server.uri()).unwrap(),
                }))],
                timeout: Duration::from_secs(2),
                ..Default::default()
            },
            sender,
            cancellation,
        )
        .await;
    assert_eq!(outcome.total_links, 1);
    assert_eq!(summary.sources_completed, 0);
    assert_eq!(summary.sources_failed, 1);
    assert!(summary.partial);
    assert_eq!(summary.exit_code(), 3);
    assert!(outcome.source_errors[0].message.contains("503"));
}

#[tokio::test(start_paused = true)]
async fn collector_publishes_fast_sibling_before_deadline_and_rejects_empty_details() {
    use crate::core::{Link, SearchResult};
    let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(2))
        .for_source(tokio::time::Instant::now() + Duration::from_secs(2));
    let jobs = [100, 1].into_iter().map(|seconds| async move {
        tokio::time::sleep(Duration::from_secs(seconds)).await;
        let mut row = SearchResult::provider(format!("c-{seconds}"), "xiaoji");
        row.title = "movie".into();
        row.links
            .push(Link::new(format!("https://pan.quark.cn/s/{seconds}")));
        Ok(row)
    });
    assert!(
        tokio::time::timeout(Duration::from_secs(2), collect_details(&ctx, jobs, 2))
            .await
            .is_err()
    );
    assert_eq!(ctx.finish(vec![]).len(), 1);
    let fresh = SearchContext::new(reqwest::Client::new(), Duration::from_secs(2));
    assert!(
        collect_details(
            &fresh,
            [async { Ok(SearchResult::provider("empty", "xiaoji")) }],
            1
        )
        .await
        .is_err()
    );
    assert!(
        collect_details(
            &fresh,
            Vec::<std::future::Ready<Result<SearchResult, ProviderError>>>::new(),
            1
        )
        .await
        .unwrap()
        .is_empty()
    );
}

#[tokio::test]
async fn request_unknown_length_chunk_overflow_and_false_length_are_not_success() {
    use std::{io::Read, net::TcpListener};
    for (length, declared, chunked, valid) in [
        (1024, None, false, true),
        (1025, None, false, false),
        (1023, Some(1024), false, false),
        (1025, None, true, false),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/boundary", listener.local_addr().unwrap());
        let serving = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0; 8192];
            assert!(socket.read(&mut request).unwrap() > 0);
            let mut response = b"HTTP/1.1 200 OK\r\nConnection: close\r\n".to_vec();
            if let Some(declared) = declared {
                response.extend_from_slice(format!("Content-Length: {declared}\r\n").as_bytes());
            }
            if chunked {
                response.extend_from_slice(b"Transfer-Encoding: chunked\r\n");
            }
            response.extend_from_slice(b"\r\n");
            if chunked {
                response.extend_from_slice(format!("{length:X}\r\n").as_bytes());
            }
            response.extend_from_slice(&vec![b'x'; length]);
            if chunked {
                response.extend_from_slice(b"\r\n0\r\n\r\n");
            }
            socket.write_all(&response).unwrap();
        });
        let client = client();
        let result = request(
            || client.get(&url).timeout(Duration::from_secs(2)),
            1,
            Duration::ZERO,
            1024,
        )
        .await;
        assert_eq!(
            result.is_ok(),
            valid,
            "length={length} declared={declared:?} chunked={chunked}"
        );
        serving.join().unwrap();
    }
}

#[tokio::test]
async fn transport_failure_redacts_url_and_request_timeout_keeps_timeout_kind() {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let client = client();
    let error = request(
        || client.get(format!("http://private:password@{address}/?token=secret")),
        1,
        Duration::ZERO,
        1024,
    )
    .await
    .unwrap_err();
    let diagnostic = error.to_string();
    for secret in ["private", "password", "secret", "token="] {
        assert!(!diagnostic.contains(secret));
    }
    let server = MockServer::start().await;
    Mock::given(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(1)))
        .expect(1)
        .mount(&server)
        .await;
    let error = request(
        || {
            client
                .get(format!("{}/slow", server.uri()))
                .timeout(Duration::from_millis(100))
        },
        1,
        Duration::ZERO,
        1024,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, ProviderError::Timeout));
}

#[tokio::test]
async fn configured_post_retries_rebuild_identical_body() {
    use wiremock::matchers::{body_json, method};
    let server = MockServer::start().await;
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = hits.clone();
    let payload = serde_json::json!({"query":"中文 movie","token":"opaque /+*~"});
    Mock::given(path("/post"))
        .and(method("POST"))
        .and(body_json(payload.clone()))
        .respond_with(move |_: &wiremock::Request| {
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(200).set_body_string("ok")
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    let client = client();
    assert_eq!(
        request(
            || client.post(format!("{}/post", server.uri())).json(&payload),
            2,
            Duration::from_millis(1),
            1024
        )
        .await
        .unwrap(),
        b"ok"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}
