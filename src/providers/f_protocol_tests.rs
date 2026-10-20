// Additional protocol boundary regressions, included by f_tests.rs.
struct ForumPostRetry {
    calls: Arc<AtomicUsize>,
}
impl Respond for ForumPostRetry {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        if self.calls.fetch_add(1, Ordering::SeqCst) < 2 {
            ResponseTemplate::new(503)
        } else {
            ResponseTemplate::new(302).insert_header("location", "/search.php?searchid=7")
        }
    }
}
#[tokio::test]
async fn qupanshe_replays_post_and_keeps_session_during_503_retries() {
    let server = MockServer::start().await;
    setup(&server, Site::Qupanshe, false).await;
    let calls = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/search.php"))
        .and(header("cookie", "session=fixture"))
        .and(body_string_contains("formhash=fixturehash"))
        .and(body_string_contains("srchtxt=movie"))
        .respond_with(ForumPostRetry {
            calls: calls.clone(),
        })
        .with_priority(1)
        .expect(3)
        .mount(&server)
        .await;
    let rows = provider(Site::Qupanshe, &format!("{}/", server.uri()))
        .search(&context(), "movie")
        .await
        .unwrap();
    assert!(!rows.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    server.verify().await;
}
#[tokio::test]
async fn qupanshe_rejects_cross_origin_session_redirect() {
    let server = MockServer::start().await;
    let other = MockServer::start().await;
    setup(&server, Site::Qupanshe, false).await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("{}/results", other.uri())),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    let answer = provider(Site::Qupanshe, &format!("{}/", server.uri()))
        .search(&context(), "movie")
        .await;
    assert!(answer.unwrap_err().to_string().contains("cross-origin"));
    assert!(other.received_requests().await.unwrap().is_empty());
}
#[tokio::test]
async fn qupanshe_decodes_gzip_bootstrap_before_formhash() {
    use std::io::Write;
    let server = MockServer::start().await;
    setup(&server, Site::Qupanshe, false).await;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(b"<html><input name='formhash' value='fixturehash'></html>")
        .unwrap();
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-encoding", "gzip")
                .insert_header("set-cookie", "session=fixture; Path=/")
                .set_body_bytes(encoder.finish().unwrap()),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    assert!(
        !provider(Site::Qupanshe, &format!("{}/", server.uri()))
            .search(&context(), "movie")
            .await
            .unwrap()
            .is_empty()
    );
}
#[tokio::test]
async fn piratebay_stops_at_thirty_pages() {
    let server = MockServer::start().await;
    let first = TPB.replace(
        "<div></div>",
        "<div><a href='/search/movie/999/99/0'>999</a></div>",
    );
    get(&server, "/search/movie/1/99/0", first).await;
    Mock::given(wiremock::matchers::path_regex(
        r"^/search/movie/(?:[2-9]|[12][0-9]|30)/99/0$",
    ))
    .respond_with(
        ResponseTemplate::new(200)
            .set_body_string("<html><table id='searchResult'></table></html>"),
    )
    .expect(29)
    .mount(&server)
    .await;
    let rows = provider(Site::Tpb, &format!("{}/", server.uri()))
        .search(&context(), "movie")
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(server.received_requests().await.unwrap().len(), 30);
    server.verify().await;
}
#[tokio::test]
async fn sdso_queries_each_type_and_both_default_pages() {
    let server = MockServer::start().await;
    setup(&server, Site::Sdso, false).await;
    let rows = provider(Site::Sdso, &format!("{}/", server.uri()))
        .search(&context(), "movie")
        .await
        .unwrap();
    assert_eq!(rows.len(), 8);
    assert!(
        rows.iter()
            .all(|r| r.title == "movie 4K"
                && r.links[0].url == "https://pan.quark.cn/s/sdso?pwd=a1b2")
    );
    let mut pairs = std::collections::HashSet::new();
    for request in server.received_requests().await.unwrap() {
        let query: std::collections::HashMap<_, _> = request.url.query_pairs().collect();
        assert_eq!(query.get("name").map(|s| s.as_ref()), Some("movie"));
        pairs.insert((query["from"].to_string(), query["pageNo"].to_string()));
    }
    assert_eq!(pairs.len(), 8);
}
#[tokio::test]
async fn shandian_selects_one_successful_mirror_and_drops_others() {
    let fast = MockServer::start().await;
    let slow = MockServer::start().await;
    setup(&fast, Site::Shandian, false).await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(SHANDIAN_SEARCH)
                .set_delay(Duration::from_secs(60)),
        )
        .mount(&slow)
        .await;
    let source = shandian::Shandian {
        bases: vec![
            format!("{}/", slow.uri()).parse().unwrap(),
            format!("{}/", fast.uri()).parse().unwrap(),
        ],
    };
    let rows = tokio::time::timeout(Duration::from_secs(1), source.search(&context(), "movie"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(slow.received_requests().await.unwrap().len() <= 1);
}
