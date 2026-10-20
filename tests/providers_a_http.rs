use pansou::{
    http::{ClientOptions, HttpClientFactory},
    providers::{
        Provider, SearchContext,
        a_registry::{aipan::AipanProvider, btbtlb::BtbtlbProvider, run488::Run488Provider},
    },
};
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use url::Url;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path, query_param},
};
fn context() -> SearchContext {
    SearchContext::new(
        HttpClientFactory::new()
            .client(&ClientOptions::default())
            .unwrap(),
        Duration::from_secs(5),
    )
}
async fn home(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Set-Cookie", "yd_frontend=fixture; Path=/")
                .set_body_string("ok"),
        )
        .mount(server)
        .await;
}
fn item(id: &str, drive: &str, time: &str) -> serde_json::Value {
    json!({"id":id,"title":"测试影片","share_link":format!("https://{drive}/s/{id}"),"share_time":time})
}
#[tokio::test]
async fn run488_accumulates_incremental_polls() {
    let server = MockServer::start().await;
    home(&server).await;
    Mock::given(path("/api/frontend/search")).and(query_param("q","测试")).and(query_param("limit","24")).and(query_param("theme","wechat-cinehunt")).and(header("cookie","yd_frontend=fixture")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"code":0,"data":{"complete":false,"search_id":"queue","list":[item("one","pan.quark.cn","2026-01-01")]}}))).expect(1).mount(&server).await;
    let count = Arc::new(AtomicUsize::new(0));
    let n = count.clone();
    Mock::given(path("/api/frontend/search/poll")).and(query_param("id","queue")).respond_with(move|_:&wiremock::Request|{let i=n.fetch_add(1,Ordering::SeqCst);ResponseTemplate::new(200).set_body_json(json!({"code":0,"data":{"complete":i==1,"items":[item(if i==0{"two"}else{"three"},if i==0{"pan.baidu.com"}else{"pan.xunlei.com"},if i==0{"2026-02-01"}else{"2026-03-01"})]}}))}).expect(2).mount(&server).await;
    let rows = Run488Provider::new(Url::parse(&server.uri()).unwrap())
        .search(&context(), "测试")
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].id, "488run-three");
    assert_eq!(count.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn run488_refreshes_stale_cookie_once() {
    let server = MockServer::start().await;
    let homes = Arc::new(AtomicUsize::new(0));
    let n = homes.clone();
    Mock::given(path("/"))
        .respond_with(move |_: &wiremock::Request| {
            let i = n.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(200)
                .insert_header(
                    "Set-Cookie",
                    if i == 0 {
                        "yd_frontend=stale; Path=/"
                    } else {
                        "yd_frontend=fresh; Path=/"
                    },
                )
                .set_body_string("ok")
        })
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path("/api/frontend/search"))
        .and(header("cookie", "yd_frontend=stale"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/api/frontend/search")).and(header("cookie","yd_frontend=fresh")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"code":0,"data":{"complete":true,"list":[item("one","pan.quark.cn","2026-01-01")]}}))).expect(1).mount(&server).await;
    assert_eq!(
        Run488Provider::new(Url::parse(&server.uri()).unwrap())
            .search(&context(), "测试")
            .await
            .unwrap()
            .len(),
        1
    );
}
#[tokio::test]
async fn run488_poll_failure_preserves_collected_results() {
    let server = MockServer::start().await;
    home(&server).await;
    Mock::given(path("/api/frontend/search")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"code":0,"data":{"complete":false,"searchId":"queue","list":[item("one","pan.quark.cn","2026-01-01")]}}))).mount(&server).await;
    Mock::given(path("/api/frontend/search/poll"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let ctx = context();
    assert!(
        Run488Provider::new(Url::parse(&server.uri()).unwrap())
            .search(&ctx, "测试")
            .await
            .is_err()
    );
    assert!(ctx.has_results());
}
#[tokio::test]
async fn run488_cancellation_drops_poll_queue() {
    let server = MockServer::start().await;
    home(&server).await;
    Mock::given(path("/api/frontend/search")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"code":0,"data":{"complete":false,"search_id":"queue","list":[item("one","pan.quark.cn","2026-01-01")]}}))).mount(&server).await;
    Mock::given(path("/api/frontend/search/poll"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let provider = Run488Provider::new(Url::parse(&server.uri()).unwrap());
    let ctx = context();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), provider.search(&ctx, "测试"))
            .await
            .is_err()
    );
    assert!(ctx.has_results());
    tokio::time::sleep(Duration::from_millis(450)).await;
}
#[tokio::test]
async fn aipan_caps_search_and_marks_partial_detail_failure() {
    let server = MockServer::start().await;
    let movies: Vec<_> = (0..14)
        .map(|i| json!({"id":i.to_string(),"title":"测试影片"}))
        .collect();
    Mock::given(path("/api/movies/search"))
        .and(query_param("q", "测试"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"movies":movies})))
        .expect(1)
        .mount(&server)
        .await;
    for i in 0..12 {
        let response = if i == 1 {
            ResponseTemplate::new(503)
        } else {
            ResponseTemplate::new(200).set_body_json(json!({"title":"测试影片","panLinks":[{"kind":"quark","url":format!("https://pan.quark.cn/s/{i}")}]}))
        };
        Mock::given(path(format!("/api/movies/detail/{i}")))
            .and(header("Referer", format!("{}/movie/{i}", server.uri())))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
    }
    let ctx = context();
    assert!(
        AipanProvider::new(Url::parse(&server.uri()).unwrap())
            .search(&ctx, "测试")
            .await
            .is_err()
    );
    assert!(
        ctx.has_results(),
        "eleven valid details must remain in the source collector"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 13);
}
#[tokio::test]
async fn btbtlb_resolves_real_resource_pages_and_filters_work_title() {
    let server = MockServer::start().await;
    Mock::given(path("/search/%E6%B5%8B%E8%AF%95"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(include_str!("fixtures/providers/btbtlb/success.html")),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/detail/123.html"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(include_str!("fixtures/providers/btbtlb/movie.html")),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/tdown/1.html")).respond_with(ResponseTemplate::new(200).set_body_string("<div class='tinfo'><h1 class='page-title'>English.Release.1080p.torrent</h1></div><a href='magnet:?xt=urn:btih:0123456789012345678901234567890123456789'>magnet</a>")).expect(1).mount(&server).await;
    Mock::given(path("/pdown/2.html"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let ctx = context();
    assert!(
        BtbtlbProvider::new(Url::parse(&server.uri()).unwrap())
            .search(&ctx, "测试")
            .await
            .is_err()
    );
    assert!(
        ctx.has_results(),
        "Chinese WorkTitle must retain the English-named successful resource"
    );
}

#[tokio::test]
async fn aipan_caps_links_at_sixty_four_before_resources() {
    let server = MockServer::start().await;
    Mock::given(path("/api/movies/search"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"movies":[{"id":"one","title":"测试影片"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let pans: Vec<_> = (0..70)
        .map(|i| json!({"kind":"quark","url":format!("https://pan.quark.cn/s/{i}")}))
        .collect();
    Mock::given(path("/api/movies/detail/one")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"title":"测试影片","panLinks":pans,"resources":[{"kind":"baidu","url":"https://pan.baidu.com/s/not-in-quota"}]}))).expect(1).mount(&server).await;
    let rows = AipanProvider::new(Url::parse(&server.uri()).unwrap())
        .search(&context(), "测试")
        .await
        .unwrap();
    assert_eq!(rows[0].links.len(), 64);
    assert_eq!(rows[0].links[0].url, "https://pan.quark.cn/s/0");
    assert_eq!(rows[0].links[63].url, "https://pan.quark.cn/s/63");
}
#[tokio::test]
async fn aipan_bounds_six_inflight_details_and_cancellation_never_starts_queued_items() {
    let server = MockServer::start().await;
    let movies: Vec<_> = (0..14)
        .map(|i| json!({"id":i.to_string(),"title":"测试影片"}))
        .collect();
    Mock::given(path("/api/movies/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"movies":movies})))
        .expect(1)
        .mount(&server)
        .await;
    let started = Arc::new(AtomicUsize::new(0));
    let n = started.clone();
    Mock::given(wiremock::matchers::path_regex(r"^/api/movies/detail/")).respond_with(move|_:&wiremock::Request|{
        n.fetch_add(1,Ordering::SeqCst);
        ResponseTemplate::new(200).set_delay(Duration::from_secs(3)).set_body_json(json!({"title":"测试影片","panLinks":[{"kind":"quark","url":"https://pan.quark.cn/s/one"}]}))
    }).mount(&server).await;
    let provider = AipanProvider::new(Url::parse(&server.uri()).unwrap());
    let task = tokio::spawn(async move { provider.search(&context(), "测试").await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while started.load(Ordering::SeqCst) < 6 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(started.load(Ordering::SeqCst), 6);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        started.load(Ordering::SeqCst),
        6,
        "no queued detail may start after cancellation"
    );
}
#[tokio::test]
async fn aipan_accepts_exact_eight_mib_and_rejects_limit_plus_one() {
    for extra in [0, 1] {
        let server = MockServer::start().await;
        let mut body = String::from(r#"{"movies":[]}"#);
        body.extend(std::iter::repeat_n(' ', (8 << 20) - body.len() + extra));
        Mock::given(path("/api/movies/search"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .expect(1)
            .mount(&server)
            .await;
        let result = AipanProvider::new(Url::parse(&server.uri()).unwrap())
            .search(&context(), "测试")
            .await;
        if extra == 0 {
            assert!(result.unwrap().is_empty());
        } else {
            assert!(result.unwrap_err().to_string().contains("limit"));
        }
    }
}
#[tokio::test]
async fn run488_shared_deadline_is_incomplete_without_starting_a_late_poll() {
    let server = MockServer::start().await;
    home(&server).await;
    Mock::given(path("/api/frontend/search")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"code":0,"data":{"complete":false,"search_id":"queue","list":[item("one","pan.quark.cn","2026-01-01")]}}))).expect(1).mount(&server).await;
    Mock::given(path("/api/frontend/search/poll"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(250);
    let ctx = context().for_source(deadline);
    assert!(
        Run488Provider::new(Url::parse(&server.uri()).unwrap())
            .search(&ctx, "测试")
            .await
            .is_err()
    );
    assert!(ctx.has_results());
    assert!(tokio::time::Instant::now() < deadline);
}
#[tokio::test]
async fn btbtlb_resource_deadline_preserves_fast_resource_and_drops_slow_sibling() {
    let server = MockServer::start().await;
    Mock::given(path("/search/%E6%B5%8B%E8%AF%95"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(include_str!("fixtures/providers/btbtlb/success.html")),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/detail/123.html"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(include_str!("fixtures/providers/btbtlb/movie.html")),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/tdown/1.html"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<a href='magnet:?xt=urn:btih:0123456789012345678901234567890123456789'>test</a>",
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/pdown/2.html"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(3))
                .set_body_string(include_str!("fixtures/providers/btbtlb/resource.html")),
        )
        .expect(1)
        .mount(&server)
        .await;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(250);
    let ctx = context().for_source(deadline);
    let p = BtbtlbProvider::new(Url::parse(&server.uri()).unwrap());
    assert!(
        tokio::time::timeout_at(deadline, p.search(&ctx, "测试"))
            .await
            .is_err()
    );
    assert!(
        ctx.has_results(),
        "fast resource must publish before slow resource returns"
    );
}
