use super::*;
use std::time::Duration;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
fn card(id: usize) -> String {
    format!(
        r#"<div class="module-card-item"><div class="module-card-item-title"><a href="/vod/detail/id/{id}.html">Movie {id}</a></div></div>"#
    )
}
fn detail() -> &'static str {
    r#"<div class="module-info-main"></div><div class="module-info-heading"><h1>Movie</h1></div><div class="down-wrap"><div class="down-src">quark</div><div class="down-card"><span class="down-card-name">第1集</span><a href="https://pan.quark.cn/s/first?">download</a></div></div>"#
}
fn context() -> SearchContext {
    SearchContext::new(reqwest::Client::new(), Duration::from_secs(5))
        .for_source(tokio::time::Instant::now() + Duration::from_secs(5))
}
#[test]
fn captured_pages_preserve_cst_date_and_do_not_invent_tianyi_password() {
    let base = Url::parse("https://www.hjzhencai.top/").unwrap();
    let (entries, _) = parse_search(include_str!("fixtures/search.html"), &base).unwrap();
    assert_eq!(entries.len(), 3);
    let row = parse_detail(include_str!("fixtures/detail.html"), entries[0].clone()).unwrap();
    assert_eq!(row.links.len(), 3);
    assert_eq!(
        row.datetime.unwrap().to_rfc3339(),
        "2026-02-04T16:00:00+00:00"
    );
    assert!(
        row.links
            .iter()
            .all(|l| l.work_title.as_deref() == Some("流浪地球2"))
    );
    assert!(
        row.links
            .iter()
            .any(|l| l.password.as_deref() == Some("4mm3"))
    );
    let ty = row
        .links
        .iter()
        .find(|l| l.cloud_type == crate::core::CloudType::Tianyi)
        .unwrap();
    assert!(ty.password.is_none());
}
#[test]
fn download_scope_grouped_episode_rules_and_no_text_fallback_if_anchor_exists() {
    let base = Url::parse("https://example.invalid/").unwrap();
    let (entries, _) = parse_search(&card(1), &base).unwrap();
    let body = r#"<div class="module-info-main"></div><a href="https://pan.baidu.com/s/ad">ad</a><div class="down-wrap"><div class="down-src">quark</div><div class="down-card"><span class="down-card-name">第1集</span><a href="https://pan.quark.cn/s/one?pwd=abcd">link</a>密码：wrong</div><div class="down-card"><span class="down-card-name">第2集</span><a href="https://pan.quark.cn/s/two">link</a></div><div class="down-src">baidu</div><div class="down-card"><span class="down-card-name">第1集</span><a href="https://pan.baidu.com/s/full">link</a></div><div class="down-card"><a href="/advert">ad</a>https://pan.baidu.com/s/mustnotfallback</div></div>"#;
    let row = parse_detail(body, entries[0].clone()).unwrap();
    assert_eq!(row.links.len(), 3);
    assert_eq!(row.links[0].password.as_deref(), Some("abcd"));
    assert_eq!(row.links[0].work_title.as_deref(), Some("Movie 1 第1集"));
    assert!(
        !row.links
            .iter()
            .any(|l| l.url.contains("mustnotfallback") || l.url.ends_with("/ad"))
    );
}
#[tokio::test]
async fn full_chain_three_page_cap_same_origin_loop_and_cache_clone() {
    let server = MockServer::start().await;
    for page in 1..=3 {
        let path_value = if page == 1 {
            "/index.php/vod/search.html".to_owned()
        } else {
            format!("/vod/search/page/{page}.html")
        };
        let next = if page == 3 { 1 } else { page + 1 };
        let body = format!(
            r#"{}<div id="page"><a class="page-next" title="下一页" href="/vod/search/page/{next}.html#discard">next</a></div>"#,
            card(page)
        );
        Mock::given(path(path_value))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(path(format!("/vod/detail/id/{page}.html")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(detail().replace("/first", &format!("/share{page}"))),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    let provider = Hjzhencai::with_base(Url::parse(&server.uri()).unwrap());
    let mut first = provider.search(&context(), "Movie").await.unwrap();
    assert_eq!(first.len(), 3);
    first[0].links[0].url = "modified".into();
    let second = provider.search(&context(), "Movie").await.unwrap();
    assert_eq!(second.len(), 3);
    assert_ne!(second[0].links[0].url, "modified");
    assert!(
        second
            .iter()
            .flat_map(|r| &r.links)
            .all(|l| !l.url.ends_with('?'))
    );
}
#[tokio::test]
async fn partial_failure_has_no_retry_and_keeps_completed_detail() {
    let server = MockServer::start().await;
    Mock::given(path("/index.php/vod/search.html"))
        .respond_with(ResponseTemplate::new(200).set_body_string(card(1) + &card(2)))
        .mount(&server)
        .await;
    Mock::given(path("/vod/detail/id/1.html"))
        .respond_with(ResponseTemplate::new(200).set_body_string(detail()))
        .mount(&server)
        .await;
    Mock::given(path("/vod/detail/id/2.html"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let provider = Hjzhencai::with_base(Url::parse(&server.uri()).unwrap());
    let ctx = context();
    assert!(provider.search(&ctx, "Movie").await.is_err());
    assert_eq!(ctx.finish(vec![]).len(), 1);
}
#[tokio::test]
async fn empty_malformed_non200_and_two_mib_limit() {
    for (status, body, ok) in [
        (
            200,
            "<div class=module-heading-search>0</div>".to_owned(),
            true,
        ),
        (200, "<html>challenge</html>".to_owned(), false),
        (403, String::new(), false),
        (200, "x".repeat((2 << 20) + 1), false),
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/index.php/vod/search.html"))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            Hjzhencai::with_base(Url::parse(&server.uri()).unwrap())
                .search(&context(), "Movie")
                .await
                .is_ok(),
            ok
        );
    }
}
#[tokio::test]
async fn deadline_retains_fast_detail_and_cancels_slow_owned_future() {
    let server = MockServer::start().await;
    Mock::given(path("/index.php/vod/search.html"))
        .respond_with(ResponseTemplate::new(200).set_body_string(card(1) + &card(2)))
        .mount(&server)
        .await;
    Mock::given(path("/vod/detail/id/1.html"))
        .respond_with(ResponseTemplate::new(200).set_body_string(detail()))
        .mount(&server)
        .await;
    Mock::given(path("/vod/detail/id/2.html"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(detail())
                .set_delay(Duration::from_secs(3)),
        )
        .mount(&server)
        .await;
    let provider = Hjzhencai::with_base(Url::parse(&server.uri()).unwrap());
    let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5))
        .for_source(tokio::time::Instant::now() + Duration::from_secs(1));
    assert!(provider.search(&ctx, "Movie").await.is_err());
    assert_eq!(ctx.finish(vec![]).len(), 1);
}
