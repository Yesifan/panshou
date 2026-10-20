//! Request-boundary regressions shared by the nine final group-B additions.
fn partial_response(case: usize, request: &wiremock::Request) -> ResponseTemplate {
    let path = request.url.path();
    let param = |key: &str| {
        request
            .url
            .query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    };
    let html = |body: String| ResponseTemplate::new(200).set_body_string(body);
    let json = |body: serde_json::Value| ResponseTemplate::new(200).set_body_json(body);
    match case {
        0 if path=="/"=>html((1..=2).map(|i|format!("<article class='excerpt'><h2><a href='/{i}'>Film {i}</a></h2></article>")).collect()),
        0 if path=="/1"=>html("<article class='article-content'><a href='https://pan.quark.cn/s/one'>share</a></article>".into()),
        1 if path.contains("/vod/search/")=>html((1..=2).map(|i|format!("<div class='module-search-item'><div class='video-info-header'><h3><a href='/index.php/vod/detail/id/{i}.html'>Film {i}</a></h3></div></div>")).collect()),
        1 if path=="/index.php/vod/detail/id/1.html"=>html("<div id='download-list'><div class='module-row-one'><a href='https://pan.quark.cn/s/one'>share</a></div></div>".into()),
        2 if path=="/search"=>html("<div class='topicItem'><a href='thread?topicId=1'>Film 1</a><a href='https://pan.quark.cn/s/one'>share</a></div><div class='topicItem'><a href='thread?topicId=2'>Film 2</a></div>".into()),
        3 if path=="/search.php"&&param("srchtxt").is_some()=>ResponseTemplate::new(302).insert_header("location","/search.php?searchid=12"),
        3 if path=="/search.php"=>html(format!("<div class='slst'><ul>{}</ul></div>",(1..=2).map(|i|format!("<li class='pbw'><h3 class='xs3'><a href='/forum.php?tid={i}'>Film {i}</a></h3><p>stats</p><p>summary</p></li>")).collect::<String>())),
        3 if path=="/forum.php"&&param("tid").as_deref()==Some("1")=>html("<div id='postmessage_1' class='t_f'><a href='https://pan.quark.cn/s/one'>share</a></div>".into()),
        4 if path=="/"=>html(format!(r#"0:{{"sign":"signed","sha":"{}","hash","hashval"}}"#,"a".repeat(64))),
        4 if path=="/api/search"=>json(serde_json::json!({"data":{"hits":[{"eid":"e1","desc":"Film 1"},{"eid":"e2","desc":"Film 2"}],"maxPageNum":1}})),
        4 if path=="/search/hashval"&&String::from_utf8_lossy(&request.body).contains("e1")=>html("ok".into()),
        4 if path=="/go/e1"=>html("[null,{\"url\":\"https://pan.quark.cn/s/one\"}]".into()),
        5 if path=="/api/discussions"=>json(serde_json::json!({"data":[{"id":"1","attributes":{"title":"Film 1"}},{"id":"2","attributes":{"title":"Film 2"}}],"links":{}})),
        5 if path=="/api/discussions/1"=>json(serde_json::json!({"data":{"id":"1","attributes":{},"relationships":{"posts":{"data":[{"id":"p1"}]}}},"included":[{"type":"posts","id":"p1","attributes":{"contentHtml":"Film https://pan.quark.cn/s/one"}}]})),
        6 if path=="/search/-------------.html"=>html(format!("<div class='sr_lists'>{}</div>",(1..=2).map(|i|format!("<dl><dt><a href='/movie/{i}.html'></a></dt><dd><p><strong><a>Film {i}</a></strong></p></dd></dl>")).collect::<String>())),
        6 if path=="/movie/1.html"=>html("<div id='donLink'><div class='down-list2'><div class='down-list3'><a href='https://pan.quark.cn/s/abcdef'>share</a></div></div></div>".into()),
        7 if path=="/vodsearch/-------------.html"=>html((1..=2).map(|i|format!("<div class='module-search-item'><div class='video-info'><div class='video-info-header'><h3><a href='/voddetail/{i}.html'>Film {i}</a></h3></div></div></div>")).collect()),
        7 if path=="/voddetail/1.html"=>html("<div class='module-heading'><h2 class='module-title'>123云盘</h2></div><div class='module-downlist'><div class='module-row-text' data-clipboard-text='https://www.123684.com/s/one'></div></div>".into()),
        8 if path=="/index.php/ajax/suggest"=>json(serde_json::json!({"code":1,"list":[{"id":1,"name":"Film 1"},{"id":2,"name":"Film 2"}]})),
        8 if path=="/mv/1.html"=>html("<div class='main-ui-meta'><h1>Film 1</h1></div><a href='https://pan.quark.cn/s/one'>share</a>".into()),
        _=>ResponseTemplate::new(503),
    }
}
#[tokio::test]
async fn partial_detail_failure_fails_source_but_retains_success_for_all_nine() {
    let server = MockServer::start().await;
    let base = Url::parse(&server.uri()).unwrap();
    for (case, (mut provider, _)) in providers(&base).into_iter().enumerate() {
        server.reset().await;
        if case == 4 {
            provider = Arc::new(
                Panyq::new(base.clone()).with_actions(super::panyq::ActionIds {
                    credential: "cred".into(),
                    intermediate: "inter".into(),
                    final_link: "final".into(),
                }),
            );
        }
        Mock::given(|_: &wiremock::Request| true)
            .respond_with(move |request: &wiremock::Request| partial_response(case, request))
            .mount(&server)
            .await;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let context = SearchContext::new(client, Duration::from_secs(5));
        let error = provider.search(&context, "Film").await.unwrap_err();
        assert!(
            error.to_string().contains("503"),
            "{}: {error}",
            provider.meta().name
        );
        assert!(
            context.has_results(),
            "{} discarded its successful sibling",
            provider.meta().name
        );
        let rows = context.finish(vec![]);
        assert_eq!(rows.len(), 1, "{}", provider.meta().name);
        assert!(!rows[0].links.is_empty());
    }
}
use super::{
    mizixing::Mizixing, muou::Muou, panta::Panta, panwiki::Panwiki, panyq::Panyq, panzun::Panzun,
    pianku::Pianku, qingying::Qingying, qiwei::Qiwei,
};
use crate::providers::{Provider, SearchContext};
use std::{sync::Arc, time::Duration};
use url::Url;
use wiremock::{Mock, MockServer, ResponseTemplate};
fn providers(base: &Url) -> Vec<(Arc<dyn Provider>, u64)> {
    vec![
        (Arc::new(Mizixing::new(base.clone())), 3),
        (Arc::new(Muou::new(vec![base.clone()])), 3),
        (Arc::new(Panta::new(base.clone())), 1),
        (Arc::new(Panwiki::new(vec![base.clone()])), 1),
        (Arc::new(Panyq::new(base.clone())), 1),
        (Arc::new(Panzun::new(base.clone())), 1),
        (Arc::new(Pianku::new(base.clone())), 3),
        (Arc::new(Qingying::new(base.clone())), 3),
        (Arc::new(Qiwei::new(vec![base.clone()])), 3),
    ]
}
#[tokio::test]
async fn exact_http_403_attempts_at_provider_boundary() {
    let server = MockServer::start().await;
    let base = Url::parse(&server.uri()).unwrap();
    for (provider, attempts) in providers(&base) {
        let context = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
        server.reset().await;
        Mock::given(|_: &wiremock::Request| true)
            .respond_with(ResponseTemplate::new(403))
            .expect(attempts)
            .mount(&server)
            .await;
        let error = provider.search(&context, "Film").await.unwrap_err();
        assert!(
            error.to_string().contains("403"),
            "{}: {error}",
            provider.meta().name
        );
        server.verify().await;
    }
}
#[tokio::test]
async fn cancellation_drops_each_adapter_without_background_requests() {
    let server = MockServer::start().await;
    let base = Url::parse(&server.uri()).unwrap();
    for (provider, _) in providers(&base) {
        server.reset().await;
        Mock::given(|_: &wiremock::Request| true)
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(2))
                    .set_body_string("not reached"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let context = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
        assert!(
            tokio::time::timeout(
                Duration::from_millis(100),
                provider.search(&context, "Film")
            )
            .await
            .is_err(),
            "{}",
            provider.meta().name
        );
        assert!(!context.has_results());
        server.verify().await;
    }
}
