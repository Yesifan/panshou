use pansou::{
    http::{ClientOptions, HttpClientFactory},
    providers::{
        Provider, SearchContext,
        a_registry::{
            aikanzy::AikanzyProvider, btbtlb::BtbtlbProvider, feikuai::FeikuaiProvider,
            gaoqing888::Gaoqing888Provider, huban::HubanProvider,
        },
    },
};
use std::time::Duration;
use url::Url;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};
fn ctx() -> SearchContext {
    SearchContext::new(
        HttpClientFactory::new()
            .client(&ClientOptions::default())
            .unwrap(),
        Duration::from_secs(10),
    )
}
#[tokio::test]
async fn huban_search_detail_empty_and_malformed() {
    for (search, expected) in [
        (
            include_str!("fixtures/providers/huban/success.html"),
            Some(1),
        ),
        (include_str!("fixtures/providers/huban/empty.html"), Some(0)),
        (
            include_str!("fixtures/providers/huban/malformed.html"),
            None,
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/index.php/vod/search/wd/test.html"))
            .and(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(search))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/index.php/vod/detail/id/123.html"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(include_str!("fixtures/providers/huban/detail.html")),
            )
            .mount(&server)
            .await;
        let p = HubanProvider::new(vec![Url::parse(&server.uri()).unwrap()]);
        let result = p.search(&ctx(), "test").await;
        match expected {
            Some(n) => assert_eq!(result.unwrap().len(), n),
            None => assert!(result.is_err()),
        }
    }
}
#[tokio::test]
async fn article_retry_counts_and_status_are_preserved() {
    let server = MockServer::start().await;
    Mock::given(path("/search"))
        .and(query_param("molds", "article"))
        .respond_with(ResponseTemplate::new(404))
        .expect(4)
        .mount(&server)
        .await;
    let e = AikanzyProvider::new(Url::parse(&server.uri()).unwrap())
        .search(&ctx(), "test")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("404"));
    let server = MockServer::start().await;
    Mock::given(path("/search"))
        .and(query_param("kw", "test"))
        .respond_with(ResponseTemplate::new(503))
        .expect(3)
        .mount(&server)
        .await;
    let e = Gaoqing888Provider::new(Url::parse(&server.uri()).unwrap())
        .search(&ctx(), "test")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("503"));
}
#[tokio::test]
async fn btbtlb_fanout_caps_ten_movies_and_forty_resources() {
    let server = MockServer::start().await;
    let cards=(1..=11).map(|i|format!("<div class='module-item'><a class='module-item-title' href='/detail/{i}.html'>测试影片</a></div>")).collect::<String>();
    Mock::given(path("/search/test"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(format!("<div class='module-items'>{cards}</div>")),
        )
        .expect(1)
        .mount(&server)
        .await;
    for movie in 1..=10 {
        let mut resources = String::new();
        for i in 1..=14 {
            resources.push_str(&format!(
                "<a class='module-row-text' href='/tdown/{movie}-{i}.html'></a>"
            ));
        }
        for i in 1..=6 {
            resources.push_str(&format!(
                "<a class='module-row-text' href='/pdown/{movie}-{i}.html'></a>"
            ));
        }
        Mock::given(path(format!("/detail/{movie}.html"))).respond_with(ResponseTemplate::new(200).set_body_string(format!("<div class='video-info-header'><h1 class='page-title'>test</h1></div><div class='module-row-info'>{resources}</div>"))).expect(1).mount(&server).await;
    }
    Mock::given(path("/detail/11.html"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let mut wanted = Vec::new();
    for movie in 1..=3 {
        for i in 1..=12 {
            wanted.push(format!("/tdown/{movie}-{i}.html"));
        }
        for i in 1..=4 {
            wanted.push(format!("/pdown/{movie}-{i}.html"));
        }
    }
    wanted.truncate(40);
    for (i, path_) in wanted.iter().enumerate() {
        Mock::given(path(path_))
            .respond_with(if i == 0 {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(200)
                    .set_body_string(format!("<a href='magnet:?xt=urn:btih:{i:040x}'>magnet</a>"))
            })
            .expect(1)
            .mount(&server)
            .await;
    }
    let context = ctx();
    assert!(
        BtbtlbProvider::new(Url::parse(&server.uri()).unwrap())
            .search(&context, "test")
            .await
            .is_err()
    );
    assert!(
        context.has_results(),
        "successful resource pages must survive a sibling 503"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 51);
}
#[tokio::test]
async fn feikuai_failed_api_falls_back_to_real_web_protocol() {
    let api = MockServer::start().await;
    let web = MockServer::start().await;
    Mock::given(path("/t_search/bm_search.php"))
        .and(query_param("kw", "测试"))
        .respond_with(ResponseTemplate::new(200).set_body_string("invalid JSON"))
        .expect(1)
        .mount(&api)
        .await;
    Mock::given(path("/vodsearch/-------------.html"))
        .and(query_param("wd", "测试"))
        .and(query_param("ext", "1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(include_str!("fixtures/providers/feikuai/web.html")),
        )
        .expect(1)
        .mount(&web)
        .await;
    let rows = FeikuaiProvider::new(
        Url::parse(&api.uri()).unwrap(),
        Url::parse(&web.uri()).unwrap(),
    )
    .search(&ctx(), "测试")
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].links[0].password.as_deref(), Some("abcd"));
}

fn new_source(name: &str, base: &Url) -> Box<dyn Provider> {
    use pansou::providers::a_registry::*;
    match name {
        "aikanzy" => Box::new(aikanzy::AikanzyProvider::new(base.clone())),
        "aipan" => Box::new(aipan::AipanProvider::new(base.clone())),
        "ash" => Box::new(ash::AshProvider::new(base.clone())),
        "btbtlb" => Box::new(btbtlb::BtbtlbProvider::new(base.clone())),
        "dy4k" => Box::new(dy4k::Dy4kProvider::new(base.clone())),
        "dyyjpro" => Box::new(dyyjpro::DyyjproProvider::new(base.clone())),
        "erxiao" => Box::new(erxiao::ErxiaoProvider::new(vec![base.clone()])),
        "feikuai" => Box::new(feikuai::FeikuaiProvider::new(base.clone(), base.clone())),
        "gaoqing888" => Box::new(gaoqing888::Gaoqing888Provider::new(base.clone())),
        "haisou" => Box::new(haisou::HaisouProvider::new(base.clone())),
        "hdr4k" => Box::new(hdr4k::Hdr4kProvider::new(base.clone())),
        "huban" => Box::new(huban::HubanProvider::new(vec![base.clone()])),
        "hunhepan" => Box::new(hunhepan::HunhepanProvider::new(vec![
            base.join("open/search/disk").unwrap(),
        ])),
        "ikantv" => Box::new(ikantv::IkantvProvider::new(base.clone())),
        "javdb" => Box::new(javdb::JavdbProvider::new(base.clone())),
        "488run" => Box::new(run488::Run488Provider::new(base.clone())),
        _ => unreachable!(),
    }
}
fn new_fixture(name: &str, scenario: &str) -> &'static str {
    match (name, scenario) {
        ("aikanzy", "success") => include_str!("fixtures/providers/aikanzy/success.html"),
        ("aikanzy", "empty") => include_str!("fixtures/providers/aikanzy/empty.html"),
        ("aikanzy", "malformed") => include_str!("fixtures/providers/aikanzy/malformed.html"),
        ("aipan", "success") => include_str!("fixtures/providers/aipan/success.json"),
        ("aipan", "empty") => include_str!("fixtures/providers/aipan/empty.json"),
        ("aipan", "malformed") => include_str!("fixtures/providers/aipan/malformed.json"),
        ("ash", "success") => include_str!("fixtures/providers/ash/success.html"),
        ("ash", "empty") => include_str!("fixtures/providers/ash/empty.html"),
        ("ash", "malformed") => include_str!("fixtures/providers/ash/malformed.html"),
        ("btbtlb", "success") => include_str!("fixtures/providers/btbtlb/success.html"),
        ("btbtlb", "empty") => include_str!("fixtures/providers/btbtlb/empty.html"),
        ("btbtlb", "malformed") => include_str!("fixtures/providers/btbtlb/malformed.html"),
        ("dy4k", "success") => include_str!("fixtures/providers/dy4k/success.html"),
        ("dy4k", "empty") => include_str!("fixtures/providers/dy4k/empty.html"),
        ("dy4k", "malformed") => include_str!("fixtures/providers/dy4k/malformed.html"),
        ("dyyjpro", "success") => include_str!("fixtures/providers/dyyjpro/success.html"),
        ("dyyjpro", "empty") => include_str!("fixtures/providers/dyyjpro/empty.html"),
        ("dyyjpro", "malformed") => include_str!("fixtures/providers/dyyjpro/malformed.html"),
        ("erxiao", "success") => include_str!("fixtures/providers/erxiao/success.html"),
        ("erxiao", "empty") => include_str!("fixtures/providers/erxiao/empty.html"),
        ("erxiao", "malformed") => include_str!("fixtures/providers/erxiao/malformed.html"),
        ("feikuai", "success") => include_str!("fixtures/providers/feikuai/success.json"),
        ("feikuai", "empty") => include_str!("fixtures/providers/feikuai/empty.json"),
        ("feikuai", "malformed") => include_str!("fixtures/providers/feikuai/malformed.json"),
        ("gaoqing888", "success") => include_str!("fixtures/providers/gaoqing888/success.html"),
        ("gaoqing888", "empty") => include_str!("fixtures/providers/gaoqing888/empty.html"),
        ("gaoqing888", "malformed") => include_str!("fixtures/providers/gaoqing888/malformed.html"),
        ("haisou", "success") => include_str!("fixtures/providers/haisou/success.json"),
        ("haisou", "empty") => include_str!("fixtures/providers/haisou/empty.json"),
        ("haisou", "malformed") => include_str!("fixtures/providers/haisou/malformed.json"),
        ("hdr4k", "success") => include_str!("fixtures/providers/hdr4k/success.html"),
        ("hdr4k", "empty") => include_str!("fixtures/providers/hdr4k/empty.html"),
        ("hdr4k", "malformed") => include_str!("fixtures/providers/hdr4k/malformed.html"),
        ("huban", "success") => include_str!("fixtures/providers/huban/success.html"),
        ("huban", "empty") => include_str!("fixtures/providers/huban/empty.html"),
        ("huban", "malformed") => include_str!("fixtures/providers/huban/malformed.html"),
        ("hunhepan", "success") => include_str!("fixtures/providers/hunhepan/success.json"),
        ("hunhepan", "empty") => include_str!("fixtures/providers/hunhepan/empty.json"),
        ("hunhepan", "malformed") => include_str!("fixtures/providers/hunhepan/malformed.json"),
        ("ikantv", "success") => include_str!("fixtures/providers/ikantv/success.json"),
        ("ikantv", "empty") => include_str!("fixtures/providers/ikantv/empty.json"),
        ("ikantv", "malformed") => include_str!("fixtures/providers/ikantv/malformed.json"),
        ("javdb", "success") => include_str!("fixtures/providers/javdb/success.html"),
        ("javdb", "empty") => include_str!("fixtures/providers/javdb/empty.html"),
        ("javdb", "malformed") => include_str!("fixtures/providers/javdb/malformed.html"),
        ("488run", "success") => include_str!("fixtures/providers/488run/success.json"),
        ("488run", "empty") => include_str!("fixtures/providers/488run/empty.json"),
        ("488run", "malformed") => include_str!("fixtures/providers/488run/malformed.json"),
        _ => unreachable!(),
    }
}
fn new_search_route(name: &str) -> (&'static str, &'static str) {
    match name {
        "aikanzy" | "gaoqing888" | "javdb" => ("GET", "/search"),
        "aipan" => ("GET", "/api/movies/search"),
        "ash" => ("GET", "/s/%E6%B5%8B%E8%AF%95.html"),
        "btbtlb" => ("GET", "/search/%E6%B5%8B%E8%AF%95"),
        "dy4k" => ("GET", "/4K-search/-------------.html"),
        "dyyjpro" => ("GET", "/"),
        "erxiao" | "huban" => ("GET", "/index.php/vod/search/wd/%E6%B5%8B%E8%AF%95.html"),
        "feikuai" => ("GET", "/t_search/bm_search.php"),
        "haisou" => ("GET", "/api/pan/share/search"),
        "hdr4k" => ("POST", "/search.php"),
        "hunhepan" => ("POST", "/open/search/disk"),
        "ikantv" => ("GET", "/api/open/pansou/search"),
        "488run" => ("GET", "/api/frontend/search"),
        _ => unreachable!(),
    }
}
async fn new_detail_routes(name: &str, server: &MockServer) {
    let routes: Vec<(&str, &str)> = match name {
        "aikanzy" => vec![(
            "/article/123.html",
            include_str!("fixtures/providers/aikanzy/detail.html"),
        )],
        "aipan" => vec![(
            "/api/movies/detail/m1",
            include_str!("fixtures/providers/aipan/detail.json"),
        )],
        "btbtlb" => vec![
            (
                "/detail/123.html",
                include_str!("fixtures/providers/btbtlb/movie.html"),
            ),
            (
                "/tdown/1.html",
                include_str!("fixtures/providers/btbtlb/resource.html"),
            ),
            (
                "/pdown/2.html",
                include_str!("fixtures/providers/btbtlb/hash.html"),
            ),
        ],
        "dy4k" => vec![(
            "/4K-detail/123.html",
            include_str!("fixtures/providers/dy4k/detail.html"),
        )],
        "dyyjpro" => vec![(
            "/123.html",
            include_str!("fixtures/providers/dyyjpro/detail.html"),
        )],
        "erxiao" | "huban" => vec![(
            "/index.php/vod/detail/id/123.html",
            include_str!("fixtures/providers/erxiao/detail.html"),
        )],
        "gaoqing888" => vec![(
            "/123/detail",
            include_str!("fixtures/providers/gaoqing888/detail.html"),
        )],
        "haisou" => vec![(
            "/api/pan/share/opaque-id/fetch",
            include_str!("fixtures/providers/haisou/share.json"),
        )],
        "hdr4k" => vec![(
            "/thread-123-1-1.html",
            include_str!("fixtures/providers/hdr4k/detail.html"),
        )],
        "javdb" => vec![(
            "/v/abc",
            include_str!("fixtures/providers/javdb/detail.html"),
        )],
        _ => vec![],
    };
    for (route, body) in routes {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(server)
            .await;
    }
    if name == "488run" {
        Mock::given(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Set-Cookie", "yd_frontend=fixture; Path=/")
                    .set_body_string("ok"),
            )
            .mount(server)
            .await;
    }
    if name == "feikuai" {
        Mock::given(path("/vodsearch/-------------.html"))
            .respond_with(ResponseTemplate::new(503))
            .mount(server)
            .await;
    }
}
#[tokio::test]
async fn all_sixteen_new_sources_have_request_boundary_search_contracts() {
    for name in [
        "aikanzy",
        "aipan",
        "ash",
        "btbtlb",
        "dy4k",
        "dyyjpro",
        "erxiao",
        "feikuai",
        "gaoqing888",
        "haisou",
        "hdr4k",
        "huban",
        "hunhepan",
        "ikantv",
        "javdb",
        "488run",
    ] {
        for scenario in ["success", "empty", "malformed", "schema", "http503"] {
            let server = MockServer::start().await;
            new_detail_routes(name, &server).await;
            let (verb, route) = new_search_route(name);
            let response = if scenario == "http503" {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(200).set_body_string(if scenario == "schema" {
                    "{}"
                } else {
                    new_fixture(name, scenario)
                })
            };
            Mock::given(method(verb))
                .and(path(route))
                .respond_with(response)
                .mount(&server)
                .await;
            let provider = new_source(name, &Url::parse(&server.uri()).unwrap());
            let result = provider.search(&ctx(), "测试").await;
            match scenario {
                "success" => assert!(
                    !result
                        .unwrap_or_else(|e| panic!("{name} success: {e}"))
                        .is_empty(),
                    "{name} success"
                ),
                "empty" => assert!(
                    result
                        .unwrap_or_else(|e| panic!("{name} empty: {e}"))
                        .is_empty(),
                    "{name} empty"
                ),
                _ => assert!(
                    result.is_err(),
                    "{name} {scenario} should fail, not succeed empty"
                ),
            }
            let requests = server.received_requests().await.unwrap();
            assert!(
                requests
                    .iter()
                    .any(|r| r.method.as_str() == verb && r.url.path() == route),
                "{name}: actual search route"
            );
            if scenario == "http503" {
                let attempts = match name {
                    "aikanzy" => 4,
                    "ash" => 2,
                    "dy4k" | "dyyjpro" | "erxiao" | "feikuai" | "gaoqing888" | "ikantv"
                    | "huban" => 3,
                    "haisou" => 30,
                    "hunhepan" => 3,
                    _ => 1,
                };
                assert_eq!(
                    requests.iter().filter(|r| r.url.path() == route).count(),
                    attempts,
                    "{name}: HTTP retry budget"
                );
            }
        }
    }
}

fn existing_source(name: &str, base: &Url) -> Box<dyn Provider> {
    use pansou::providers::*;
    match name {
        "cldi" => Box::new(cldi::CldiProvider::new(cldi::Endpoints {
            base: base.clone(),
        })),
        "clmao" => Box::new(clmao::Clmao::new(clmao::Endpoints { base: base.clone() })),
        "clxiong" => Box::new(clxiong::ClxiongProvider::new(clxiong::Endpoints {
            base: base.clone(),
            search: base.join("e/search/index.php").unwrap(),
        })),
        "cyg" => Box::new(cyg::CygProvider::new(cyg::Endpoints { base: base.clone() })),
        "djgou" => Box::new(djgou::Djgou::new(djgou::Endpoints { base: base.clone() })),
        "duanjuw" => Box::new(duanjuw::DuanjuwProvider::new(duanjuw::Endpoints {
            base: base.clone(),
        })),
        "dyyj" => Box::new(dyyj::DyyjProvider::new(dyyj::Endpoints {
            base: base.clone(),
        })),
        "hdmoli" => Box::new(hdmoli::HdmoliProvider::new(hdmoli::Endpoints {
            base: base.clone(),
        })),
        _ => unreachable!(),
    }
}
fn existing_fixture(name: &str, kind: &str) -> &'static str {
    match (name, kind) {
        ("cldi", "success") => include_str!("fixtures/providers/cldi/success.html"),
        ("cldi", "empty") => include_str!("fixtures/providers/cldi/empty.html"),
        ("cldi", "malformed") => include_str!("fixtures/providers/cldi/malformed.html"),
        ("clmao", "success") => include_str!("fixtures/providers/clmao/success-search.html"),
        ("clmao", "empty") => include_str!("fixtures/providers/clmao/empty.html"),
        ("clmao", "malformed") => include_str!("fixtures/providers/clmao/malformed.html"),
        ("clxiong", "success") => include_str!("fixtures/providers/clxiong/success.html"),
        ("clxiong", "empty") => include_str!("fixtures/providers/clxiong/empty.html"),
        ("clxiong", "malformed") => include_str!("fixtures/providers/clxiong/malformed.html"),
        ("cyg", "success") => include_str!("fixtures/providers/cyg/success.json"),
        ("cyg", "empty") => include_str!("fixtures/providers/cyg/empty.json"),
        ("cyg", "malformed") => include_str!("fixtures/providers/cyg/malformed.json"),
        ("djgou", "success") => include_str!("fixtures/providers/djgou/success.html"),
        ("djgou", "empty") => include_str!("fixtures/providers/djgou/empty.html"),
        ("djgou", "malformed") => include_str!("fixtures/providers/djgou/malformed.html"),
        ("duanjuw", "success") => include_str!("fixtures/providers/duanjuw/success.html"),
        ("duanjuw", "empty") => include_str!("fixtures/providers/duanjuw/empty.html"),
        ("duanjuw", "malformed") => include_str!("fixtures/providers/duanjuw/malformed.html"),
        ("dyyj", "success") => include_str!("fixtures/providers/dyyj/success.json"),
        ("dyyj", "empty") => include_str!("fixtures/providers/dyyj/empty.json"),
        ("dyyj", "malformed") => include_str!("fixtures/providers/dyyj/malformed.json"),
        ("hdmoli", "success") => include_str!("fixtures/providers/hdmoli/success.html"),
        ("hdmoli", "empty") => include_str!("fixtures/providers/hdmoli/empty.html"),
        ("hdmoli", "malformed") => include_str!("fixtures/providers/hdmoli/malformed.html"),
        _ => unreachable!(),
    }
}
fn existing_search_route(name: &str) -> (&'static str, &'static str) {
    match name {
        "cldi" => ("GET", "/search-"),
        "clmao" => ("GET", "/search"),
        "clxiong" => ("POST", "/e/search/index.php"),
        "cyg" => ("GET", "/wp-json/wp/v2/posts"),
        "djgou" | "hdmoli" => ("GET", "/search.php"),
        "duanjuw" => ("GET", "/so/search.php"),
        "dyyj" => ("GET", "/api/discussions"),
        _ => unreachable!(),
    }
}
async fn existing_detail_routes(name: &str, server: &MockServer) {
    let routes = match name {
        "clmao" => vec![(
            "/detail/movie-42",
            include_str!("fixtures/providers/clmao/success.html"),
        )],
        "clxiong" => vec![(
            "/drama/4466.html",
            include_str!("fixtures/providers/clxiong/detail.html"),
        )],
        "cyg" => vec![(
            "/wp-json/acg-studio/v1/download",
            include_str!("fixtures/providers/cyg/download.json"),
        )],
        "djgou" => vec![(
            "/a/1",
            include_str!("fixtures/providers/djgou/success-detail.html"),
        )],
        "hdmoli" => vec![(
            "/movie/xiann.html",
            include_str!("fixtures/providers/hdmoli/detail.html"),
        )],
        _ => vec![],
    };
    for (route, body) in routes {
        Mock::given(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(server)
            .await;
    }
}
#[tokio::test]
async fn all_eight_updated_sources_keep_request_success_empty_and_error_contracts() {
    for name in [
        "cldi", "clmao", "clxiong", "cyg", "djgou", "duanjuw", "dyyj", "hdmoli",
    ] {
        for scenario in ["success", "empty", "malformed", "http503"] {
            let server = MockServer::start().await;
            existing_detail_routes(name, &server).await;
            let (verb, route) = existing_search_route(name);
            let response = if scenario == "http503" {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(200).set_body_string(existing_fixture(name, scenario))
            };
            let matcher = Mock::given(method(verb));
            let matcher = if name == "cldi" {
                matcher.and(wiremock::matchers::path_regex(
                    r"^/search-.*-0-2-[1-5]\.html$",
                ))
            } else {
                matcher.and(path(route))
            };
            if name == "clxiong" && scenario != "http503" {
                matcher
                    .respond_with(
                        ResponseTemplate::new(200)
                            .insert_header("Location", "/e/search/result/?searchid=7"),
                    )
                    .expect(1)
                    .mount(&server)
                    .await;
                Mock::given(method("GET"))
                    .and(path("/e/search/result/"))
                    .and(query_param("searchid", "7"))
                    .respond_with(response)
                    .expect(1)
                    .mount(&server)
                    .await;
            } else {
                matcher.respond_with(response).mount(&server).await;
            }
            let result = existing_source(name, &Url::parse(&server.uri()).unwrap())
                .search(&ctx(), "仙逆")
                .await;
            match scenario {
                "success" => assert!(
                    !result.unwrap_or_else(|e| panic!("{name}: {e}")).is_empty(),
                    "{name} success"
                ),
                "empty" => assert!(
                    result.unwrap_or_else(|e| panic!("{name}: {e}")).is_empty(),
                    "{name} empty"
                ),
                "http503" => assert!(result.is_err(), "{name} HTTP error"),
                "malformed" if matches!(name, "cyg" | "dyyj") => {
                    assert!(result.is_err(), "{name} malformed JSON")
                }
                // Existing HTML parsers intentionally discard malformed cards. This
                // verifies they cannot fabricate links, NOT schema-error parity.
                "malformed" => assert!(result.unwrap().is_empty(), "{name} malformed cards"),
                _ => unreachable!(),
            }
            if scenario == "http503" {
                let requests = server.received_requests().await.unwrap();
                let expected = match name {
                    "cldi" => 15,
                    "cyg" | "djgou" | "duanjuw" | "hdmoli" => 3,
                    _ => 1,
                };
                assert_eq!(requests.len(), expected, "{name} exact retry count");
            }
        }
    }
}

fn sibling_search(name: &str) -> (String, &'static str) {
    match name {
        "aipan" => (serde_json::json!({"movies":[{"id":"m1","title":"测试影片"},{"id":"bad","title":"测试影片"}]}).to_string(), "/api/movies/detail/bad"),
        "haisou" => (serde_json::json!({"code":0,"data":{"list":[{"hsid":"opaque-id","platform":"quark","share_name":"测试影片"},{"hsid":"bad","platform":"quark","share_name":"测试影片"}]}}).to_string(), "/api/pan/share/bad/fetch"),
        "dy4k" => ("<div class='space-y-4'><div><div><a href='/4K-detail/123.html'>测试影片</a></div><a href='/4K-vodplay/123.html'>播放</a></div><div><div><a href='/4K-detail/124.html'>测试影片</a></div><a href='/4K-vodplay/124.html'>播放</a></div></div>".into(), "/4K-detail/124.html"),
        "javdb" => (format!("{}{}",new_fixture(name,"success"),new_fixture(name,"success").replace("/v/abc","/v/bad")), "/v/bad"),
        "aikanzy" => (format!("{}{}",new_fixture(name,"success"),new_fixture(name,"success").replace("123","124")), "/article/124.html"),
        "btbtlb" => (format!("{}{}",new_fixture(name,"success"),new_fixture(name,"success").replace("123","124")), "/detail/124.html"),
        "dyyjpro" => (format!("{}{}",new_fixture(name,"success"),new_fixture(name,"success").replace("123","124")), "/124.html"),
        "erxiao"|"huban" => (format!("{}{}",new_fixture(name,"success"),new_fixture(name,"success").replace("123","124")), "/index.php/vod/detail/id/124.html"),
        "gaoqing888" => (format!("{}{}",new_fixture(name,"success"),new_fixture(name,"success").replace("123","124")), "/124/detail"),
        "hdr4k" => (format!("{}{}",new_fixture(name,"success"),new_fixture(name,"success").replace("123","124")), "/thread-124-1-1.html"),
        _=>unreachable!(),
    }
}
#[tokio::test]
async fn sibling_detail_failure_is_incomplete_and_keeps_published_rows() {
    for name in [
        "aikanzy",
        "aipan",
        "btbtlb",
        "dy4k",
        "dyyjpro",
        "erxiao",
        "huban",
        "gaoqing888",
        "haisou",
        "hdr4k",
        "javdb",
    ] {
        let server = MockServer::start().await;
        new_detail_routes(name, &server).await;
        let (body, bad) = sibling_search(name);
        let (verb, search) = new_search_route(name);
        Mock::given(method(verb))
            .and(path(search))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;
        let attempts = match name {
            "aikanzy" => 4,
            "dy4k" | "dyyjpro" | "erxiao" | "huban" | "gaoqing888" | "haisou" => 3,
            _ => 1,
        };
        Mock::given(path(bad))
            .respond_with(ResponseTemplate::new(503))
            .expect(attempts)
            .mount(&server)
            .await;
        let context = ctx();
        let result = new_source(name, &Url::parse(&server.uri()).unwrap())
            .search(&context, "测试")
            .await;
        let error = result.expect_err(&format!(
            "{name}: a failed sibling must not complete the source"
        ));
        assert!(
            error.to_string().contains("503"),
            "{name}: preserve actual sibling HTTP status: {error}"
        );
        assert!(
            context.has_results(),
            "{name}: valid sibling must remain in source collector"
        );
    }
}
#[tokio::test]
async fn federated_page_failure_keeps_fast_page_links_but_is_incomplete() {
    use pansou::providers::a_registry::hunhepan::HunhepanProvider;
    let server = MockServer::start().await;
    Mock::given(path("/open/search/disk"))
        .respond_with(|request: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["q"], "测试");
            assert_eq!(body["size"], 30);
            if body["page"] == 2 {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(200)
                    .set_body_string(include_str!("fixtures/providers/hunhepan/success.json"))
            }
        })
        .expect(3)
        .mount(&server)
        .await;
    let context = ctx();
    let p = HunhepanProvider::new(vec![
        Url::parse(&format!("{}/open/search/disk", server.uri())).unwrap(),
    ]);
    assert!(p.search(&context, "测试").await.is_err());
    assert!(context.has_results());
}

#[tokio::test]
async fn detail_fanout_deadline_keeps_fast_links_and_cancels_retry_and_pending_io() {
    for name in [
        "aikanzy",
        "aipan",
        "dy4k",
        "dyyjpro",
        "erxiao",
        "huban",
        "gaoqing888",
        "haisou",
        "hdr4k",
        "javdb",
    ] {
        let server = MockServer::start().await;
        new_detail_routes(name, &server).await;
        let (body, bad) = sibling_search(name);
        let (verb, route) = new_search_route(name);
        Mock::given(method(verb))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;
        Mock::given(path(bad))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(3))
                    .set_body_string("{}"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(250);
        let context = ctx().for_source(deadline);
        let provider = new_source(name, &Url::parse(&server.uri()).unwrap());
        // The actual source deadline belongs to the engine. All requests and
        // retry futures are owned by this search future and dropped together.
        let outcome = tokio::time::timeout_at(deadline, provider.search(&context, "测试")).await;
        assert!(
            outcome.is_err() || outcome.as_ref().is_ok_and(|r| r.is_err()),
            "{name}: incomplete at deadline"
        );
        assert!(
            context.has_results(),
            "{name}: fast detail must publish before slow sibling"
        );
        let count = server.received_requests().await.unwrap().len();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            count,
            "{name}: dropped search must not retry in background"
        );
    }
}

fn existing_siblings(name: &str) -> (String, &'static str) {
    match name {
        "clmao"=>(format!("{}{}",existing_fixture(name,"success"),existing_fixture(name,"success").replace("movie-42","movie-43")),"/detail/movie-43"),
        "clxiong"=>(format!("{}{}",existing_fixture(name,"success"),existing_fixture(name,"success").replace("4466","4467")),"/drama/4467.html"),
        "cyg"=>{
            let mut posts:serde_json::Value=serde_json::from_str(existing_fixture(name,"success")).unwrap();
            let mut second=posts[0].clone();second["id"]=serde_json::json!(43);
            posts.as_array_mut().unwrap().push(second);(posts.to_string(),"/wp-json/acg-studio/v1/download")
        },
        "djgou"=>(format!("{}{}",existing_fixture(name,"success"),existing_fixture(name,"success").replace("/a/1","/a/2")),"/a/2"),
        "hdmoli"=>(format!("{}{}",existing_fixture(name,"success"),existing_fixture(name,"success").replace("xiann.html","other.html")),"/movie/other.html"),
        "duanjuw"=>("<li class='col-6'><h3 class='f-14'><a href='/detail/one'>仙逆</a></h3></li><li class='col-6'><h3 class='f-14'><a href='/detail/two'>仙逆</a></h3></li>".into(),"/detail/two"),
        _=>unreachable!(),
    }
}
async fn existing_sibling_search(name: &str, server: &MockServer) {
    existing_detail_routes(name, server).await;
    let (body, bad) = existing_siblings(name);
    let (verb, route) = existing_search_route(name);
    if name == "clxiong" {
        Mock::given(method(verb))
            .and(path(route))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Location", "/e/search/result/?searchid=7"),
            )
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/e/search/result/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(server)
            .await;
    } else {
        Mock::given(method(verb))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(server)
            .await;
    }
    if name == "duanjuw" {
        Mock::given(path("/detail/one"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(include_str!("fixtures/providers/duanjuw/detail_scope.html")),
            )
            .mount(server)
            .await;
    }
    let bad_mock = Mock::given(method("GET")).and(path(bad));
    let bad_mock = if name == "cyg" {
        bad_mock.and(query_param("id", "43"))
    } else {
        bad_mock
    };
    bad_mock
        .respond_with(ResponseTemplate::new(503))
        .with_priority(1)
        .mount(server)
        .await;
}
#[tokio::test]
async fn updated_html_and_wordpress_sources_mark_failed_sibling_incomplete() {
    for name in ["clmao", "clxiong", "cyg", "djgou", "duanjuw", "hdmoli"] {
        let server = MockServer::start().await;
        existing_sibling_search(name, &server).await;
        let context = ctx();
        let result = existing_source(name, &Url::parse(&server.uri()).unwrap())
            .search(&context, "仙逆")
            .await;
        assert!(
            result.is_err(),
            "{name}: failed detail must make source incomplete"
        );
        assert!(
            context.has_results(),
            "{name}: completed detail must remain in collector"
        );
    }
}
#[tokio::test]
async fn cldi_partial_search_pages_are_incomplete_but_keep_valid_page() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::path_regex(r"^/search-.*-0-2-1\.html$"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(include_str!("fixtures/providers/cldi/success.html")),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(wiremock::matchers::path_regex(
        r"^/search-.*-0-2-[2-5]\.html$",
    ))
    .respond_with(ResponseTemplate::new(503))
    .expect(12)
    .mount(&server)
    .await;
    let context = ctx();
    assert!(
        existing_source("cldi", &Url::parse(&server.uri()).unwrap())
            .search(&context, "仙逆")
            .await
            .is_err()
    );
    assert!(context.has_results());
}
