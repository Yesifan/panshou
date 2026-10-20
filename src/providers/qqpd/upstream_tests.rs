use super::*;
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path, query_param},
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
    (dir, provider)
}
fn lookup(id: &str, code: u64) -> serde_json::Value {
    json!({"retcode":0,"error":{"code":0},"data":{"cmd0xf57_rsp":{"rpt_rsp_guild_info_list":[{"uint64_guild_id":id,"uint32_result":code}]}}})
}
#[test]
fn canonical_positive_u64_and_response_error_contract() {
    assert!(valid_guild_id("18446744073709551615"));
    for invalid in ["0", "01", "+1", "-1", "1.0", "18446744073709551616", ""] {
        assert!(!valid_guild_id(invalid));
    }
    assert!(
        parse_search_response(r#"{"retcode":0,"data":{"union_result":null}}"#, "ch")
            .unwrap()
            .is_empty()
    );
    for invalid in [
        r#"{"data":{}}"#,
        r#"{"retcode":155,"message":"not logged in"}"#,
        r#"{"retcode":0,"error":{"code":221}}"#,
        r#"{"retcode":0}"#,
        "<html>EO-Bot-Js-Token</html>",
    ] {
        assert!(parse_search_response(invalid, "ch").is_err());
    }
}
#[tokio::test]
async fn official_lookup_contract_and_no_network_numeric_fast_path() {
    let server = MockServer::start().await;
    let (_dir, provider) = setup(&server);
    provider
        .save_login("main", "p_skey=fixture-key", "masked")
        .unwrap();
    Mock::given(method("POST")).and(path("/lookup")).and(query_param("forGuildId","1")).and(query_param("bkn",bkn("fixture-key").to_string())).and(header("X-QQ-Client-AppId","537246381")).and(header("x-oidb",r#"{"uint32_service_type":1}"#)).and(body_json(json!({"guild_num":"named","cmd0xf57_req":{"msg_field_filter":{"msg_guild_info_filter":{"uint32_guild_number":1}},"rpt_req_guild_info_list":[{}]}}))).respond_with(ResponseTemplate::new(200).set_body_json(lookup("18446744073709551615",0))).expect(1).mount(&server).await;
    provider
        .add_channels(&Client::new(), "main", ["named", "18446744073709551615"])
        .await
        .unwrap();
    provider
        .add_channels(&Client::new(), "main", ["named"])
        .await
        .unwrap();
    let state = provider.load_profile("main").unwrap().unwrap();
    assert_eq!(state.channel_guild_ids.len(), 2);
    assert!(provider.take_diagnostics().is_empty());
}
#[tokio::test]
async fn failed_lookup_keeps_configuration_retries_and_isolates_only_explicit_missing() {
    let server = MockServer::start().await;
    let (_dir, provider) = setup(&server);
    provider
        .save_login("main", "p_skey=fixture", "masked")
        .unwrap();
    Mock::given(path("/lookup"))
        .respond_with(ResponseTemplate::new(200).set_body_json(lookup("", 7)))
        .expect(1)
        .mount(&server)
        .await;
    provider
        .add_channels(&Client::new(), "main", ["named"])
        .await
        .unwrap();
    let state = provider.load_profile("main").unwrap().unwrap();
    assert_eq!(state.channels, ["named"]);
    assert!(state.channel_guild_ids.is_empty());
    assert!(state.dead_channel_failures.is_empty());
    assert!(!provider.take_diagnostics().is_empty());
    server.reset().await;
    Mock::given(path("/lookup"))
        .respond_with(ResponseTemplate::new(200).set_body_json(lookup("", 10003)))
        .expect(2)
        .mount(&server)
        .await;
    provider
        .repair_profile(&Client::new(), "main")
        .await
        .unwrap();
    provider
        .repair_profile(&Client::new(), "main")
        .await
        .unwrap();
    provider
        .repair_profile(&Client::new(), "main")
        .await
        .unwrap();
    let state = provider.load_profile("main").unwrap().unwrap();
    assert_eq!(state.dead_channel_failures.get("named"), Some(&2));
    assert_eq!(state.channels, ["named"]);
    provider
        .save_login("other", "p_skey=fixture", "masked")
        .unwrap();
    assert!(
        provider
            .load_profile("other")
            .unwrap()
            .unwrap()
            .dead_channel_failures
            .is_empty()
    );
    server.reset().await;
    Mock::given(path("/lookup"))
        .respond_with(ResponseTemplate::new(200).set_body_json(lookup("987", 0)))
        .expect(1)
        .mount(&server)
        .await;
    provider
        .add_channels(&Client::new(), "main", ["named"])
        .await
        .unwrap();
    assert!(
        provider
            .load_profile("main")
            .unwrap()
            .unwrap()
            .dead_channel_failures
            .is_empty()
    );
}
#[tokio::test]
async fn old_indexes_repaired_and_twelve_concurrent_writes_are_not_lost() {
    let server = MockServer::start().await;
    let (dir, provider) = setup(&server);
    provider
        .save_login("main", "p_skey=fixture", "masked")
        .unwrap();
    let mut old = provider.load_profile("main").unwrap().unwrap();
    old.channels = (0..12).map(|i| format!("ch{i}")).collect();
    old.channel_guild_ids = old
        .channels
        .iter()
        .map(|ch| (ch.clone(), ch.clone()))
        .collect();
    StateStore::with_cipher(dir.path(), None)
        .save("qqpd", "main", &old)
        .unwrap();
    assert!(
        provider
            .load_profile("main")
            .unwrap()
            .unwrap()
            .is_ready(Utc::now())
    );
    Mock::given(path("/lookup"))
        .respond_with(ResponseTemplate::new(200).set_body_json(lookup("987", 0)))
        .expect(12)
        .mount(&server)
        .await;
    provider
        .repair_profile(&Client::new(), "main")
        .await
        .unwrap();
    let repaired = provider.load_profile("main").unwrap().unwrap();
    assert_eq!(repaired.channel_guild_ids.len(), 12);
    assert_eq!(repaired.channels, old.channels);
    assert_eq!(repaired.cookie, old.cookie);
    provider
        .repair_profile(&Client::new(), "main")
        .await
        .unwrap();
}
#[tokio::test]
async fn deleted_channel_is_not_revived_by_inflight_lookup() {
    let server = MockServer::start().await;
    let (_dir, provider) = setup(&server);
    provider
        .save_login("main", "p_skey=fixture", "masked")
        .unwrap();
    Mock::given(path("/lookup"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(100))
                .set_body_json(lookup("123", 0)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new();
    let (added, _) = tokio::join!(provider.add_channels(&client, "main", ["named"]), async {
        tokio::time::sleep(Duration::from_millis(30)).await;
        provider.delete_channels("main", ["named"]).unwrap();
    });
    assert!(added.unwrap().is_empty());
    let state = provider.load_profile("main").unwrap().unwrap();
    assert!(state.channels.is_empty());
    assert!(state.channel_guild_ids.is_empty());
}
#[tokio::test]
async fn overlimit_challenge_and_transport_error_do_not_leak_auth_url() {
    let server = MockServer::start().await;
    let (_dir, provider) = setup(&server);
    Mock::given(path("/lookup"))
        .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(4 * 1024 * 1024 + 1)))
        .mount(&server)
        .await;
    let result = request_api(
        &Client::new(),
        &provider.endpoints.guild_page,
        "p_skey=private-fixture",
        "{}",
        json!({}),
        true,
    )
    .await;
    assert!(result.err().unwrap().to_string().contains("4MiB"));
    let result = request_api(
        &Client::new(),
        &Url::parse("http://127.0.0.1:1/lookup").unwrap(),
        "p_skey=private-fixture",
        "{}",
        json!({}),
        true,
    )
    .await;
    let error = result.err().unwrap().to_string();
    assert!(!error.contains("bkn"));
    assert!(!error.contains("private-fixture"));
}
