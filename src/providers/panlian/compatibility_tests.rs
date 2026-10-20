use super::*;
use crate::state::StateCipher;
use serde_json::json;
use std::time::Duration;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
fn test_store(root: &std::path::Path) -> StateStore {
    StateStore::with_cipher(root, Some(StateCipher::from_bytes([17; 32])))
}

fn setup(server: &MockServer) -> (tempfile::TempDir, PanlianProvider) {
    let dir = tempfile::tempdir().unwrap();
    let store = test_store(dir.path());
    store
        .save(
            "panlian",
            "main",
            &PanlianProfile {
                username: "fixture".into(),
                password: Some("saved-secret".into()),
                cookie: "admin_session=fixture".into(),
                status: "active".into(),
                last_access: None,
            },
        )
        .unwrap();
    let provider = PanlianProvider::load(
        store,
        Endpoints {
            base: Url::parse(&format!("{}/", server.uri())).unwrap(),
        },
        [],
    )
    .unwrap();
    (dir, provider)
}
#[tokio::test]
async fn real_old_endpoint_falls_back_but_modern_auth_failure_never_does() {
    let server = MockServer::start().await;
    let (_dir, provider) = setup(&server);
    Mock::given(path("/api/videos"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/api/get_videos.php"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"code":1,"list":[{"vod_id":7,"vod_name":"film"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/api/search_pan_links.php")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"data":{"quark":{"links":[{"url":"https://pan.quark.cn/s/fixture"}]}}}))).expect(1).mount(&server).await;
    let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(3));
    let rows = provider.search(&ctx, "film").await.unwrap();
    assert_eq!(rows.len(), 1);
    server.reset().await;
    Mock::given(path("/api/videos"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"success":false,"message":"请先登录"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert!(matches!(
        provider.search(&ctx, "film").await,
        Err(ProviderError::AuthRequired)
    ));
    assert!(provider.profile_names().unwrap().is_empty());
}
#[tokio::test]
async fn expired_cookie_is_cleared_without_silent_saved_password_login() {
    let server = MockServer::start().await;
    let (dir, provider) = setup(&server);
    Mock::given(path("/api/videos"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(3));
    assert!(matches!(
        provider.search(&ctx, "film").await,
        Err(ProviderError::AuthRequired)
    ));
    let expired = test_store(dir.path())
        .load::<PanlianProfile>("panlian", "main")
        .unwrap()
        .unwrap();
    assert!(expired.cookie.is_empty());
    assert!(expired.password.is_none());
    assert_eq!(expired.status, "expired");
}
#[test]
fn profile_and_provider_debug_do_not_leak_credentials() {
    let profile = PanlianProfile {
        username: "private-identity".into(),
        password: Some("private-password".into()),
        cookie: "private-cookie".into(),
        status: "active".into(),
        last_access: None,
    };
    assert!(!format!("{profile:?}").contains("private"));
    let dir = tempfile::tempdir().unwrap();
    let store = test_store(dir.path());
    assert!(store.encryption_enabled());
    store.save("panlian", "main", &profile).unwrap();
    let provider = PanlianProvider::load(store, Endpoints::default(), []).unwrap();
    assert!(!format!("{provider:?}").contains("private"));
}
