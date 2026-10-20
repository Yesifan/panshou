use super::*;
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, path},
};

fn setup(server: &MockServer) -> (tempfile::TempDir, QqpdProvider) {
    let dir = tempfile::tempdir().unwrap();
    let provider = QqpdProvider::with_endpoints(
        StateStore::with_cipher(dir.path(), None),
        QqpdEndpoints {
            guild_page: Url::parse(&format!("{}/lookup", server.uri())).unwrap(),
            search: Url::parse(&format!("{}/search", server.uri())).unwrap(),
            ..QqpdEndpoints::default()
        },
    );
    provider
        .save_login("main", "p_skey=fixture", "masked")
        .unwrap();
    (dir, provider)
}
fn success() -> serde_json::Value {
    json!({"retcode":0,"data":{"union_result":{"guild_feeds":[{"id":"fixture","title":"名称：film","content":"https://pan.quark.cn/s/fixture"}]}}})
}
#[tokio::test]
async fn partial_failure_publishes_valid_rows_before_error() {
    let server = MockServer::start().await;
    let (_dir, provider) = setup(&server);
    provider
        .add_channels(&Client::new(), "main", ["1", "2"])
        .await
        .unwrap();
    Mock::given(path("/search"))
        .and(body_partial_json(json!({"guild_id":"1"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(success()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/search"))
        .and(body_partial_json(json!({"guild_id":"2"})))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let ctx = SearchContext::new(Client::new(), Duration::from_secs(3));
    assert!(provider.search(&ctx, "film").await.is_err());
    assert_eq!(ctx.finish(Vec::new()).len(), 1);
}
#[tokio::test]
async fn slow_cold_index_does_not_block_fast_channel_delivery() {
    let server = MockServer::start().await;
    let (dir, provider) = setup(&server);
    let mut state = provider.load_profile("main").unwrap().unwrap();
    state.channels = vec!["1".into(), "cold".into()];
    state.channel_guild_ids.insert("1".into(), "1".into());
    StateStore::with_cipher(dir.path(), None)
        .save("qqpd", "main", &state)
        .unwrap();
    Mock::given(path("/lookup")).respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(500)).set_body_json(json!({"retcode":0,"data":{"cmd0xf57_rsp":{"rpt_rsp_guild_info_list":[{"uint64_guild_id":"2","uint32_result":0}]}}}))).mount(&server).await;
    Mock::given(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(success()))
        .expect(1)
        .mount(&server)
        .await;
    let ctx = SearchContext::new(Client::new(), Duration::from_secs(3))
        .for_source(tokio::time::Instant::now() + Duration::from_millis(200));
    assert!(
        tokio::time::timeout(Duration::from_millis(200), provider.search(&ctx, "film"))
            .await
            .is_err()
    );
    assert_eq!(ctx.finish(Vec::new()).len(), 1);
    assert!(
        !provider
            .load_profile("main")
            .unwrap()
            .unwrap()
            .channel_guild_ids
            .contains_key("cold")
    );
}
#[test]
fn profile_debug_does_not_reveal_cookie_or_identifier() {
    let profile = QqpdProfile::authenticated("p_skey=secret-cookie", "secret-identity");
    let debug = format!("{profile:?}");
    assert!(!debug.contains("secret"));
}
