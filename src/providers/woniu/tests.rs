use super::*;
use crate::{
    http::ClientOptions,
    providers::{Provider, SearchContext},
};
use std::time::Duration;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, header, method, path},
};
fn provider(server: &MockServer, dir: &tempfile::TempDir) -> WoniuProvider {
    WoniuProvider::load(
        StateStore::with_cipher(dir.path(), None),
        Endpoints::parse(&server.uri()).unwrap(),
        HttpClientFactory::new(),
        ClientOptions::default(),
    )
    .unwrap()
}
async fn login_mock(s: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/user/login.html"))
        .and(header("x-requested-with", "XMLHttpRequest"))
        .and(body_string_contains("user_name=user&user_pwd=secret"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("set-cookie", "sid=test; Path=/; HttpOnly")
                .set_body_json(serde_json::json!({"code":1})),
        )
        .expect(1)
        .mount(s)
        .await;
}
#[tokio::test]
async fn login_search_logout_private_state() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let p = provider(&server, &dir);
    login_mock(&server).await;
    let profile = p.login("default", "user", "secret").await.unwrap();
    assert!(profile.is_ready(Utc::now()));
    assert!(!format!("{profile:?}").contains("sid=test"));
    let raw = std::fs::read_to_string(dir.path().join("woniu/default.json")).unwrap();
    assert!(!raw.contains("secret"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.path().join("woniu/default.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    Mock::given(path("/user/index.html"))
        .and(header("cookie", "sid=test"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("set-cookie", "sid=rotated; Path=/; HttpOnly")
                .set_body_string("account"),
        )
        .mount(&server)
        .await;
    Mock::given(path("/vodsearch/-------------/"))
        .and(header("cookie", "sid=rotated"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                r#"<a class="video-card" title="movie" href="/voddetail/1/"></a>"#,
            ),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/voddetail/1/")).and(header("cookie","sid=rotated")).respond_with(ResponseTemplate::new(200).set_body_string(r#"<div class="pan-link-item" data-pan-item="https://pan.quark.cn/s/a 提取码abcd"><div class="pan-link-title">4K</div></div>"#)).expect(1).mount(&server).await;
    let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
    let rows = p.search(&ctx, "movie").await.unwrap();
    assert_eq!(rows[0].id, "woniu-1");
    assert_eq!(rows[0].links[0].work_title.as_deref(), Some("movie 4K"));
    assert_eq!(rows[0].links[0].password.as_deref(), Some("abcd"));
    let reloaded = provider(&server, &dir);
    assert_eq!(reloaded.profile_names().unwrap(), vec!["default"]);
    assert_eq!(reloaded.selected().unwrap().1.cookie, "sid=rotated");
    Mock::given(path("/user/logout.html"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    assert!(p.logout("default").await.unwrap());
    assert!(!dir.path().join("woniu/default.json").exists());
}
#[tokio::test]
async fn rejects_missing_cookie_and_failed_login() {
    for (body, cookie) in [
        (serde_json::json!({"code":1}), false),
        (serde_json::json!({"code":1003,"msg":"bad secret"}), true),
    ] {
        let s = MockServer::start().await;
        let d = tempfile::tempdir().unwrap();
        let p = provider(&s, &d);
        let mut response = ResponseTemplate::new(200).set_body_json(body);
        if cookie {
            response = response.insert_header("set-cookie", "sid=x; Path=/")
        };
        Mock::given(path("/user/login.html"))
            .respond_with(response)
            .mount(&s)
            .await;
        let e = p.login("default", "user", "secret").await.unwrap_err();
        assert!(!e.to_string().contains("secret"));
        assert!(p.profile_names().unwrap().is_empty());
    }
}
#[tokio::test]
async fn redirect_expiry_and_base_binding() {
    let s = MockServer::start().await;
    let d = tempfile::tempdir().unwrap();
    let p = provider(&s, &d);
    login_mock(&s).await;
    p.login("default", "user", "secret").await.unwrap();
    Mock::given(path("/user/index.html"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/user/login.html"))
        .mount(&s)
        .await;
    assert!(!p.verify("default").await.unwrap());
    assert!(p.profile_names().unwrap().is_empty());
    let other = MockServer::start().await;
    let changed = WoniuProvider::load(
        p.store.clone(),
        Endpoints::parse(&other.uri()).unwrap(),
        HttpClientFactory::new(),
        ClientOptions::default(),
    )
    .unwrap();
    assert!(changed.profile_names().unwrap().is_empty());
    assert!(Endpoints::parse("").is_err());
    assert_eq!(
        Endpoints::parse("wn4k.com/").unwrap().base.as_str(),
        "https://wn4k.com/"
    );
}
#[tokio::test]
async fn cross_origin_never_receives_cookie() {
    let s = MockServer::start().await;
    let evil = MockServer::start().await;
    let d = tempfile::tempdir().unwrap();
    let p = provider(&s, &d);
    login_mock(&s).await;
    p.login("default", "user", "secret").await.unwrap();
    Mock::given(path("/user/index.html"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("{}/user/index.html", evil.uri())),
        )
        .mount(&s)
        .await;
    assert!(p.verify("default").await.is_err());
    assert!(evil.received_requests().await.unwrap().is_empty());
}
#[tokio::test]
async fn no_account_is_explicit_auth_error() {
    let s = MockServer::start().await;
    let d = tempfile::tempdir().unwrap();
    let p = provider(&s, &d);
    let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
    assert_eq!(
        p.search(&ctx, "movie").await,
        Err(ProviderError::AuthRequired)
    );
    assert!(s.received_requests().await.unwrap().is_empty());
}
#[tokio::test]
async fn partial_details_published_before_deadline() {
    let s = MockServer::start().await;
    let d = tempfile::tempdir().unwrap();
    let p = provider(&s, &d);
    login_mock(&s).await;
    p.login("default", "user", "secret").await.unwrap();
    Mock::given(path("/user/index.html"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&s)
        .await;
    Mock::given(path("/vodsearch/-------------/")).respond_with(ResponseTemplate::new(200).set_body_string(r#"<a class="video-card" title="movie fast" href="/voddetail/1/"></a><a class="video-card" title="movie slow" href="/voddetail/2/"></a>"#)).mount(&s).await;
    Mock::given(path("/voddetail/1/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<div class="pan-link-item" data-pan-item="https://pan.quark.cn/s/fast"></div>"#,
        ))
        .mount(&s)
        .await;
    Mock::given(path("/voddetail/2/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(2))
                .set_body_string("slow"),
        )
        .mount(&s)
        .await;
    let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5))
        .for_source(tokio::time::Instant::now() + Duration::from_millis(150));
    let _ = tokio::time::timeout(Duration::from_millis(180), p.search(&ctx, "movie")).await;
    assert!(ctx.has_results());
    let rows = ctx.finish(vec![]);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].title.ends_with("fast"));
}

#[tokio::test]
async fn verification_accepts_exact_64_kib_but_rejects_one_extra_byte() {
    for (length, valid) in [(64 * 1024, true), (64 * 1024 + 1, false)] {
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let provider = provider(&server, &dir);
        login_mock(&server).await;
        provider.login("default", "user", "secret").await.unwrap();
        Mock::given(path("/user/index.html"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; length]))
            .expect(1)
            .mount(&server)
            .await;
        let result = provider.verify("default").await;
        if valid {
            assert!(result.unwrap())
        } else {
            assert!(result.is_err())
        }
    }
}

#[tokio::test]
async fn verification_rejects_late_truncated_chunked_body_after_exact_limit() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let provider = WoniuProvider::load(
        StateStore::with_cipher(dir.path(), None),
        Endpoints::parse(&base).unwrap(),
        HttpClientFactory::new(),
        ClientOptions::default(),
    )
    .unwrap();
    let now = Utc::now();
    provider
        .save(
            "default",
            WoniuProfile {
                username: "user".into(),
                cookie: "sid=test".into(),
                base_url: base.trim_end_matches('/').into(),
                status: "active".into(),
                expires_at: now + chrono::Duration::days(1),
                last_access_at: now,
                last_check_at: now,
            },
        )
        .unwrap();
    let serving = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = [0; 8192];
        assert!(stream.read(&mut request).unwrap() > 0);
        stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n10000\r\n").unwrap();
        stream.write_all(&vec![b'x'; 64 * 1024]).unwrap();
        stream.write_all(b"\r\n").unwrap();
        // Close without the terminal chunk, after the old drain's early exit.
    });
    assert!(provider.verify("default").await.is_err());
    serving.join().unwrap();
}

#[tokio::test]
async fn failed_detail_does_not_mark_partial_woniu_search_successful() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let provider = provider(&server, &dir);
    login_mock(&server).await;
    provider.login("default", "user", "secret").await.unwrap();
    Mock::given(path("/user/index.html"))
        .respond_with(ResponseTemplate::new(200).set_body_string("account"))
        .mount(&server)
        .await;
    Mock::given(path("/vodsearch/-------------/")).respond_with(ResponseTemplate::new(200).set_body_string(r#"<a class="video-card" title="movie good" href="/voddetail/1/"></a><a class="video-card" title="movie bad" href="/voddetail/2/"></a>"#)).mount(&server).await;
    Mock::given(path("/voddetail/1/")).respond_with(ResponseTemplate::new(200).set_body_string(r#"<div class="pan-link-item" data-pan-item="https://pan.quark.cn/s/good"><div class="pan-link-title">4K</div></div>"#)).expect(1).mount(&server).await;
    Mock::given(path("/voddetail/2/"))
        .respond_with(ResponseTemplate::new(503).set_delay(Duration::from_millis(30)))
        .expect(1)
        .mount(&server)
        .await;
    let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(2))
        .for_source(tokio::time::Instant::now() + Duration::from_secs(2));
    let error = provider.search(&ctx, "movie").await.unwrap_err();
    assert!(error.to_string().contains("503"));
    assert_eq!(ctx.finish(vec![]).len(), 1);
}
