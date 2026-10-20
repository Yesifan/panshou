use super::*;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{path, query_param},
};
#[tokio::test]
async fn fast_direct_route_is_retained_while_other_routes_hit_source_deadline() {
    let server = MockServer::start().await;
    let event = format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({"title":"Movie","url":"https://pan.quark.cn/s/fast","is_type":0})
    );
    Mock::given(path("/api/other/web_search"))
        .and(query_param("is_type", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_string(event))
        .mount(&server)
        .await;
    for kind in [2, 4] {
        Mock::given(path("/api/other/web_search"))
            .and(query_param("is_type", kind.to_string()))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("data: [DONE]\n\n")
                    .set_delay(Duration::from_secs(3)),
            )
            .mount(&server)
            .await;
    }
    Mock::given(path("/api2/content"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"success":true,"data":{"data":[]}})),
        )
        .mount(&server)
        .await;
    let provider = Buerchen::with_base(Url::parse(&server.uri()).unwrap());
    let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5))
        .for_source(Instant::now() + Duration::from_secs(1));
    assert!(provider.search(&ctx, "Movie").await.is_err());
    let rows = ctx.finish(vec![]);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].links[0].url.ends_with("/fast"));
}
#[tokio::test]
async fn malformed_daily_and_decrypt_responses_are_errors_not_empty_success() {
    for decrypt in [false, true] {
        let server = MockServer::start().await;
        Mock::given(path("/api/other/web_search"))
            .respond_with(ResponseTemplate::new(200).set_body_string("data: [DONE]\n\n"))
            .mount(&server)
            .await;
        let daily = if decrypt {
            json!({"success":true,"data":{"data":[{"title":"Movie","quarkLink":"original+/=="}]}})
        } else {
            json!({"success":true,"data":{}})
        };
        Mock::given(path("/api2/content"))
            .respond_with(ResponseTemplate::new(200).set_body_json(daily))
            .mount(&server)
            .await;
        if decrypt {
            Mock::given(path("/api2/decrypt"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"success":true,"data":{}})),
                )
                .expect(1)
                .mount(&server)
                .await;
        }
        let provider = Buerchen::with_base(Url::parse(&server.uri()).unwrap());
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
        assert!(provider.search(&ctx, "Movie").await.is_err());
        assert!(ctx.finish(vec![]).is_empty());
    }
}
