use super::*;
use std::time::Duration;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
fn card(id: usize) -> String {
    format!(
        r#"<div class="module-search-item"><div class="video-info-header"><h3><a href="/vod/detail/id/{id}.html">Movie {id}</a></h3></div></div>"#
    )
}
fn detail() -> &'static str {
    r#"<div class="video-info-header"><h1>Movie</h1></div><div id="download-list"><div class="module-row-one"><div class="module-row-title"><h4>第1集</h4></div><a class="copy" data-clipboard-text="https://pan.quark.cn/s/first"></a></div></div>"#
}
fn context(seconds: u64) -> SearchContext {
    SearchContext::new(reqwest::Client::new(), Duration::from_secs(seconds))
        .for_source(tokio::time::Instant::now() + Duration::from_secs(seconds))
}
#[test]
fn captured_search_detail_metadata_and_share_boundaries() {
    let base = Url::parse("https://www.2xiaopan.one/").unwrap();
    let (entries, _) = parse_search(include_str!("fixtures/search.html"), &base).unwrap();
    assert_eq!(entries.len(), 2);
    let result = parse_detail(include_str!("fixtures/detail.html"), entries[0].clone()).unwrap();
    assert_eq!(result.links.len(), 3);
    assert!(
        result
            .links
            .iter()
            .all(|l| l.work_title.as_deref() == Some("狂飙"))
    );
    assert!(
        result
            .links
            .iter()
            .any(|l| l.password.as_deref() == Some("tqwi"))
    );
    assert!(
        result
            .links
            .iter()
            .any(|l| l.password.as_deref() == Some("9rnf"))
    );
    assert!(!result.content.is_empty());
    assert!(!result.images.is_empty());
    for invalid in [
        "https://pan.quark.cn.evil/s/a",
        "https://user@pan.quark.cn/s/a",
        "https://pan.quark.cn:443/s/a",
        "https://pan.quark.cn/s/",
        "https://pan.quark.cn/list/share",
    ] {
        assert!(html::share(invalid, "", true).is_none(), "{invalid}");
    }
    let ty = html::share(
        "https://cloud.189.cn/web/share?code=abcd（访问码：9rnf）",
        "访问码：9rnf",
        true,
    )
    .unwrap();
    assert_eq!(ty.password.as_deref(), Some("9rnf"));
}
#[test]
fn episodes_only_count_different_valid_download_labels() {
    let base = Url::parse("https://example.invalid/").unwrap();
    let (entries, _) = parse_search(&card(1), &base).unwrap();
    let body=detail().replace("</div></div>",r#"</div><div class="module-row-one"><div class="module-row-title"><h4>第2集</h4></div><a class="copy" data-clipboard-text="https://pan.baidu.com/s/second?pwd=abcd"></a></div></div>"#);
    let row = parse_detail(&body, entries[0].clone()).unwrap();
    assert_eq!(row.links.len(), 2);
    assert_eq!(row.links[0].work_title.as_deref(), Some("Movie 第1集"));
    assert_eq!(row.links[1].work_title.as_deref(), Some("Movie 第2集"));
}
#[tokio::test]
async fn full_chain_pagination_loop_and_owned_cache() {
    let server = MockServer::start().await;
    let page = format!(
        r#"{}<div id="page"><a class="page-next" title="下一页" href="/vod/search/page/2/wd/Movie.html">next</a><a class="page-next" title="尾页" href="https://evil.invalid/vod/search/page/9/wd/Movie.html">last</a></div>"#,
        card(1)
    );
    Mock::given(path("/index.php/vod/search/wd/Movie.html"))
        .respond_with(ResponseTemplate::new(200).set_body_string(page))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path("/vod/search/page/2/wd/Movie.html"))
        .respond_with(ResponseTemplate::new(200).set_body_string(card(2)))
        .expect(2)
        .mount(&server)
        .await;
    for id in 1..=2 {
        Mock::given(path(format!("/vod/detail/id/{id}.html")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(detail().replace("/first", &format!("/share{id}"))),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    let provider = Erxiaopan::with_base(Url::parse(&server.uri()).unwrap());
    let mut first = provider.search(&context(5), "Movie").await.unwrap();
    assert_eq!(first.len(), 2);
    first[0].links.clear();
    let second = provider.search(&context(5), "Movie").await.unwrap();
    assert_eq!(second.len(), 2);
    assert_eq!(second[0].links.len(), 1);
}
#[tokio::test]
async fn partial_details_are_published_and_503_retries_three_times() {
    let server = MockServer::start().await;
    Mock::given(path("/index.php/vod/search/wd/Movie.html"))
        .respond_with(ResponseTemplate::new(200).set_body_string(card(1) + &card(2)))
        .mount(&server)
        .await;
    Mock::given(path("/vod/detail/id/1.html"))
        .respond_with(ResponseTemplate::new(200).set_body_string(detail()))
        .mount(&server)
        .await;
    Mock::given(path("/vod/detail/id/2.html"))
        .respond_with(ResponseTemplate::new(503))
        .expect(3)
        .mount(&server)
        .await;
    let provider = Erxiaopan::with_base(Url::parse(&server.uri()).unwrap());
    let ctx = context(5);
    assert!(provider.search(&ctx, "Movie").await.is_err());
    assert_eq!(ctx.finish(vec![]).len(), 1);
}
#[tokio::test]
async fn empty_malformed_non200_and_limit_contract() {
    for (status, body, count, ok) in [
        (200, "<div class=mac_total>0</div>".to_owned(), 1, true),
        (200, "<html>challenge</html>".to_owned(), 3, false),
        (403, String::new(), 1, false),
        (200, "x".repeat((4 << 20) + 1), 1, false),
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/index.php/vod/search/wd/Movie.html"))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .expect(count)
            .mount(&server)
            .await;
        let result = Erxiaopan::with_base(Url::parse(&server.uri()).unwrap())
            .search(&context(5), "Movie")
            .await;
        assert_eq!(result.is_ok(), ok);
    }
}
#[tokio::test]
async fn deadline_cancels_retry_wait() {
    let server = MockServer::start().await;
    Mock::given(path("/index.php/vod/search/wd/Movie.html"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let provider = Erxiaopan::with_base(Url::parse(&server.uri()).unwrap());
    let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5))
        .for_source(tokio::time::Instant::now() + Duration::from_millis(100));
    assert!(matches!(
        provider.search(&ctx, "Movie").await,
        Err(ProviderError::Timeout)
    ));
}
