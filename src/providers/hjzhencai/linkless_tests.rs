use super::*;
use std::time::Duration;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
#[tokio::test]
async fn all_linkless_details_fail_and_mixed_details_still_report_incomplete() {
    for mixed in [false, true] {
        let server = MockServer::start().await;
        let card = |id| {
            format!(
                r#"<div class="module-card-item"><div class="module-card-item-title"><a href="/vod/detail/id/{id}.html">Movie</a></div></div>"#
            )
        };
        let search = if mixed { card(1) + &card(2) } else { card(1) };
        Mock::given(path("/index.php/vod/search.html"))
            .respond_with(ResponseTemplate::new(200).set_body_string(search))
            .mount(&server)
            .await;
        Mock::given(path("/vod/detail/id/1.html")).respond_with(ResponseTemplate::new(200).set_body_string(r#"<div class="module-info-main"></div><div class="down-wrap"><div class="down-card"><a href="https://ads.invalid/s/ad">ad</a></div></div>"#)).expect(1).mount(&server).await;
        if mixed {
            Mock::given(path("/vod/detail/id/2.html")).respond_with(ResponseTemplate::new(200).set_body_string(r#"<div class="down-wrap"><div class="down-card"><a href="https://pan.quark.cn/s/good">good</a></div></div>"#)).mount(&server).await;
        }
        let provider = Hjzhencai::with_base(Url::parse(&server.uri()).unwrap());
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
        assert!(provider.search(&ctx, "Movie").await.is_err());
        assert_eq!(ctx.finish(vec![]).len(), usize::from(mixed));
    }
}
