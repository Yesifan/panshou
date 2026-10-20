use super::*;
use crate::providers::{
    Provider,
    e_registry::{erxiaopan::Erxiaopan, hjzhencai::Hjzhencai},
};
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{path, path_regex},
};
fn card(id: usize, er: bool) -> String {
    if er {
        format!(
            r#"<div class="module-search-item"><div class="video-info-header"><h3><a href="/vod/detail/id/{id}.html">Movie {id}</a></h3></div></div>"#
        )
    } else {
        format!(
            r#"<div class="module-card-item"><div class="module-card-item-title"><a href="/vod/detail/id/{id}.html">Movie {id}</a></div></div>"#
        )
    }
}
fn provider(base: Url, er: bool) -> Arc<dyn Provider> {
    if er {
        Arc::new(Erxiaopan::with_base(base))
    } else {
        Arc::new(Hjzhencai::with_base(base))
    }
}
fn detail(id: &str, er: bool) -> String {
    if er {
        format!(
            r#"<div id="download-list"><div class="module-row-one"><a class="copy" data-clipboard-text="https://pan.quark.cn/s/share{id}"></a></div></div>"#
        )
    } else {
        format!(
            r#"<div class="down-wrap"><div class="down-card"><a href="https://pan.quark.cn/s/share{id}">download</a></div></div>"#
        )
    }
}
#[tokio::test]
async fn html_detail_caps_are_twenty_and_forty_eight() {
    for (er, cap, search) in [
        (true, 20, "/index.php/vod/search/wd/Movie.html"),
        (false, 48, "/index.php/vod/search.html"),
    ] {
        let server = MockServer::start().await;
        let cards = (0..cap + 5).map(|id| card(id, er)).collect::<String>();
        Mock::given(path(search))
            .respond_with(ResponseTemplate::new(200).set_body_string(cards))
            .mount(&server)
            .await;
        Mock::given(path_regex(r"^/vod/detail/id/[0-9]+\.html$"))
            .respond_with(move |request: &Request| {
                let id = request
                    .url
                    .path()
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .trim_end_matches(".html");
                ResponseTemplate::new(200).set_body_string(detail(id, er))
            })
            .expect(cap as u64)
            .mount(&server)
            .await;
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(10));
        assert_eq!(
            provider(Url::parse(&server.uri()).unwrap(), er)
                .search(&ctx, "Movie")
                .await
                .unwrap()
                .len(),
            cap
        );
    }
}
#[tokio::test]
async fn simultaneous_searches_share_five_or_three_detail_slots() {
    for (er, slots, search) in [
        (true, 5, "/index.php/vod/search/wd/Movie.html"),
        (false, 3, "/index.php/vod/search.html"),
    ] {
        let server = MockServer::start().await;
        Mock::given(path(search))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string((0..12).map(|id| card(id, er)).collect::<String>()),
            )
            .mount(&server)
            .await;
        Mock::given(path_regex(r"^/vod/detail/id/[0-9]+\.html$"))
            .respond_with(ResponseTemplate::new(503).set_delay(Duration::from_secs(3)))
            .mount(&server)
            .await;
        let provider = provider(Url::parse(&server.uri()).unwrap(), er);
        let first = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5))
            .for_source(Instant::now() + Duration::from_millis(300));
        let second = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5))
            .for_source(first.deadline().unwrap());
        let (a, b) = tokio::join!(
            provider.search(&first, "Movie"),
            provider.search(&second, "Movie")
        );
        assert!(a.is_err() && b.is_err());
        let count = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path().contains("/detail/"))
            .count();
        assert!(
            count > 0 && count <= slots,
            "{count} exceeded shared {slots} slots"
        );
    }
}
