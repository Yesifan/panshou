use super::modern::*;
use super::*;
use crate::http::{ClientOptions, HttpClientFactory};
use serde_json::json;
use std::time::Duration;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, body_string_contains, header, method, path, query_param},
};

fn setup(server: &MockServer) -> (tempfile::TempDir, PanlianProvider, Session) {
    let dir = tempfile::tempdir().unwrap();
    let provider = PanlianProvider::load(
        StateStore::with_cipher(dir.path(), None),
        Endpoints {
            base: Url::parse(&format!("{}/", server.uri())).unwrap(),
        },
        [],
    )
    .unwrap();
    let session = HttpClientFactory::new()
        .session(&ClientOptions::default())
        .unwrap();
    (dir, provider, session)
}

#[tokio::test]
async fn human_login_requires_email_and_persists_only_after_confirmation() {
    let server = MockServer::start().await;
    let (dir, provider, session) = setup(&server);
    Mock::given(method("GET")).and(path("/api/auth/captcha")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"data":{"id":"fixture-captcha","image":"data:image/png;base64,aW1hZ2U="}}))).expect(1).mount(&server).await;
    let challenge = provider.captcha(&session).await.unwrap();
    let image = challenge.write_private_image().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(image.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o077,
            0
        );
    }
    Mock::given(method("POST")).and(path("/api/auth/login")).and(body_string_contains("remember=1")).and(body_string_contains("captcha_code=1234")).and(header("x-requested-with","XMLHttpRequest")).respond_with(ResponseTemplate::new(400).set_body_json(json!({"success":false,"details":{"email_verify_required":true,"confirm_id":"fixture-confirm","email":"f***@example.test"}}))).expect(1).mount(&server).await;
    let pending = match provider
        .login_with_captcha(
            &session,
            "main",
            "fixture",
            "password",
            &challenge.id,
            "1234",
        )
        .await
        .unwrap()
    {
        LoginOutcome::EmailRequired(p) => p,
        _ => panic!("must not activate"),
    };
    assert!(provider.profile_names().unwrap().is_empty());
    assert!(
        StateStore::with_cipher(dir.path(), None)
            .load::<PanlianProfile>("panlian", "main")
            .unwrap()
            .is_none()
    );
    Mock::given(method("POST"))
        .and(path("/api/auth/login/confirm-email"))
        .and(body_string_contains("email=fixture%40example.test"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(
                    "set-cookie",
                    "admin_session=fixture-session; Path=/; Max-Age=2592000; HttpOnly",
                )
                .set_body_json(json!({"success":true})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let active = provider
        .confirm_email(&session, "main", &pending, "fixture@example.test")
        .await
        .unwrap();
    assert_eq!(active.status, "active");
    assert!(active.password.is_none());
    assert!(active.cookie.contains("admin_session="));
    assert_eq!(provider.profile_names().unwrap(), ["main"]);
}

#[tokio::test]
async fn malformed_captcha_and_wrong_confirmation_never_activate() {
    let server = MockServer::start().await;
    let (_, provider, session) = setup(&server);
    assert!(
        provider
            .login_with_captcha(&session, "main", "u", "p", "", "1234")
            .await
            .is_err()
    );
    Mock::given(method("POST"))
        .and(path("/api/auth/login"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({"success":false,"details":{"captcha_error":true}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert!(
        provider
            .login_with_captcha(&session, "main", "u", "p", "id", "wrong")
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("captcha")
    );
    Mock::given(method("POST"))
        .and(path("/api/auth/login/confirm-email"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"success":false})))
        .expect(1)
        .mount(&server)
        .await;
    let pending = EmailConfirmation {
        confirm_id: "fixture".into(),
        username: "u".into(),
        masked_email: "***".into(),
    };
    assert!(
        provider
            .confirm_email(&session, "main", &pending, "wrong@example.test")
            .await
            .is_err()
    );
    assert!(provider.profile_names().unwrap().is_empty());
}

#[tokio::test]
async fn modern_chain_diversifies_quota_and_never_uses_metadata_as_url() {
    let server = MockServer::start().await;
    let (_, provider, session) = setup(&server);
    Mock::given(method("GET")).and(path("/api/videos")).and(query_param("search","film")).and(query_param("sort","year_desc")).and(query_param("page_size","10")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"data":{"list":[{"id":7,"title":"film","type":"movie","type_name":"电影"}]}}))).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/api/videos/7")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"data":{"video":{"id":7},"links":[{"id":1,"pan_type":"baidu"},{"id":2,"pan_type":"baidu"},{"id":3,"pan_type":"quark"},{"id":4,"pan_type":"uc"}]}}))).expect(1).mount(&server).await;
    for id in [1, 3, 4] {
        Mock::given(method("POST"))
            .and(path("/api/videos/link-ticket"))
            .and(body_json(json!({"link_id":id})))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"success":true,"data":{"ticket":format!("ticket-{id}")}}),
                ),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET")).and(path(format!("/api/videos/link-open/{id}"))).and(query_param("t",format!("ticket-{id}"))).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"data":{"url":format!("https://pan.quark.cn/s/fixture{id}"),"code":"1234"}}))).expect(1).mount(&server).await;
    }
    let profile = PanlianProfile {
        username: "u".into(),
        password: None,
        cookie: "admin_session=fixture".into(),
        status: "active".into(),
        last_access: None,
    };
    let ctx = SearchContext::new(session.client().clone(), Duration::from_secs(5));
    let rows = provider
        .modern_search(&ctx, &profile, "film")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].links.len(), 3);
    assert_eq!(rows[0].tags[0], "电影");
    assert_eq!(rows[0].links[0].password.as_deref(), Some("1234"));
}

#[tokio::test]
async fn auth_get_aborts_once_and_500_retries_exactly_three() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let server = MockServer::start().await;
    let (_, provider, session) = setup(&server);
    Mock::given(path("/api/auth/captcha"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    assert!(matches!(
        provider.captcha(&session).await,
        Err(ProviderError::AuthRequired)
    ));
    server.reset().await;
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    Mock::given(path("/api/auth/captcha")).respond_with(move |_:&wiremock::Request| {if c.fetch_add(1,Ordering::SeqCst)<2{ResponseTemplate::new(500)}else{ResponseTemplate::new(200).set_body_json(json!({"success":true,"data":{"id":"fixture","image":"data:image/png;base64,aQ=="}}))}}).expect(3).mount(&server).await;
    assert!(provider.captcha(&session).await.is_ok());
    assert_eq!(count.load(Ordering::SeqCst), 3);
}
