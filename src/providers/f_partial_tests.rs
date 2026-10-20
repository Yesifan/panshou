// Included by f_tests.rs: partial batches survive both late failure and drop.
async fn partial_setup(server: &MockServer, site: Site, delay: Duration) {
    setup(server, site, false).await;
    let (search_path, search_body, failed_path) = match site {
        Site::Quarktv => (
            "/",
            format!(
                "{QUARKTV_SEARCH}<article class='excerpt'><header><h2><a href='/2.html'>movie slow</a></h2></header></article>"
            ),
            "/2.html",
        ),
        Site::Shandian => (
            "/index.php/vod/search/wd/movie.html",
            format!(
                "{SHANDIAN_SEARCH}{}",
                SHANDIAN_SEARCH.replace("/id/1.html", "/id/2.html")
            ),
            "/index.php/vod/detail/id/2.html",
        ),
        Site::Sousou => (
            "/search",
            format!(
                "{SOUSOU_SEARCH}{}",
                SOUSOU_SEARCH.replace("/doc/1", "/doc/2")
            ),
            "/doc/2",
        ),
        Site::Tpb => (
            "/search/movie/1/99/0",
            TPB.replace(
                "<div></div>",
                "<div><a href='/search/movie/2/99/0'>2</a></div>",
            ),
            "/search/movie/2/99/0",
        ),
        Site::Wuji => (
            "/search",
            WUJI_SEARCH.replace(
                "</tbody>",
                "<tr><td><a href='/hash/2'>movie slow</a></td></tr></tbody>",
            ),
            "/hash/2",
        ),
        Site::Xb6v => (
            "/e/search/result/",
            XB6V_SEARCH.replace(
                "</ul>",
                "<li class='post'><a href='/movie/2.html'>movie slow</a></li></ul>",
            ),
            "/movie/2.html",
        ),
        Site::Sdso => {
            Mock::given(path("/api/sd/search"))
                .and(query_param("from", "baidu"))
                .respond_with(ResponseTemplate::new(503).set_delay(delay))
                .with_priority(1)
                .mount(server)
                .await;
            return;
        }
        Site::Qupanshe => unreachable!("single result-page protocol has no follow-up task"),
    };
    Mock::given(method("GET"))
        .and(path(search_path))
        .respond_with(ResponseTemplate::new(200).set_body_string(search_body))
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(failed_path))
        .respond_with(ResponseTemplate::new(503).set_delay(delay))
        .with_priority(1)
        .mount(server)
        .await;
}
#[tokio::test]
async fn completed_batches_survive_late_detail_or_page_failure() {
    for site in SITES.into_iter().filter(|s| !matches!(s, Site::Qupanshe)) {
        let server = MockServer::start().await;
        partial_setup(&server, site, Duration::ZERO).await;
        let ctx = context();
        let answer = provider(site, &format!("{}/", server.uri()))
            .search(&ctx, "movie")
            .await;
        assert!(answer.is_err(), "{site:?} must expose incomplete source");
        assert!(ctx.has_results(), "{site:?} lost completed rows");
    }
}
#[tokio::test]
async fn completed_batches_survive_cancelling_slow_followups() {
    for site in SITES.into_iter().filter(|s| !matches!(s, Site::Qupanshe)) {
        let server = MockServer::start().await;
        partial_setup(&server, site, Duration::from_secs(60)).await;
        let ctx = context();
        let source = provider(site, &format!("{}/", server.uri()));
        assert!(
            tokio::time::timeout(Duration::from_millis(150), source.search(&ctx, "movie"))
                .await
                .is_err(),
            "{site:?}"
        );
        assert!(ctx.has_results(), "{site:?} lost completed rows on drop");
    }
}
