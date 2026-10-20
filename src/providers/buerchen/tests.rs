use super::*;
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{header, method, path, query_param},
};
fn context() -> SearchContext {
    SearchContext::new(reqwest::Client::new(), Duration::from_secs(5))
        .for_source(Instant::now() + Duration::from_secs(5))
}
async fn sources(server: &MockServer, streams: [String; 3], daily: Value) {
    for (index, body) in streams.into_iter().enumerate() {
        Mock::given(path("/api/other/web_search"))
            .and(query_param("is_type", (index * 2).to_string()))
            .and(query_param("title", "Movie"))
            .and(header("accept", "text/event-stream"))
            .and(header("accept-encoding", "identity"))
            .and(header("referer", format!("{}/s/Movie.html", server.uri())))
            .and(header("user-agent", UA))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .mount(server)
            .await;
    }
    Mock::given(path("/api2/content"))
        .and(query_param("keyword", "Movie"))
        .and(header("accept", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(daily))
        .mount(server)
        .await;
}
fn empty_daily() -> Value {
    json!({"success":true,"data":{"data":[]}})
}
fn event(kind: usize, url: &str) -> String {
    format!(
        "data: {}\n\n",
        json!({"title":"Movie","url":url,"is_type":kind})
    )
}
#[test]
fn fixture_framing_multiline_filtering_and_component_encoding() {
    let provider = Buerchen::default();
    let mut parser = stream::Parser::default();
    for line in include_str!("fixtures/search.sse").lines() {
        parser.line(line, "流浪地球", 0, &provider).unwrap();
    }
    assert_eq!(parser.entries.len(), 2);
    assert!(parser.done);
    let mut parser = stream::Parser::default();
    for line in [
        ":heartbeat\r",
        "event: message\r",
        "id: 1\r",
        "retry: 100\r",
        "线路：quark\r",
        "data: {\r",
        "data: \"title\":\"Movie\",\"url\":\"0+/1==\",\"is_type\":0}\r",
        "\r",
        "data: [DONE]\r",
        "\r",
    ] {
        parser.line(line, "Movie", 0, &provider).unwrap();
    }
    assert_eq!(parser.entries.len(), 1);
    assert!(parser.done);
    assert_eq!(component("0+/1== !'()*~"), "0%2B%2F1%3D%3D%20!'()*~");
    assert!(normalize("https://pan.quark.cn.evil/s/x", "").is_none());
    assert!(normalize("https://pan.quark.cn:443/s/x", "").is_none());
    assert_eq!(
        normalize("https://pan.baidu.com/s/share?pwd=abcd", "zzzz")
            .unwrap()
            .password
            .as_deref(),
        Some("abcd")
    );
}
#[tokio::test]
async fn full_chain_quotas_daily_decrypt_encoding_stable_ids_cache_and_partial_errors() {
    let server = MockServer::start().await;
    let streams = std::array::from_fn(|index| {
        let mut body = String::from(": heartbeat\r\n\r\n");
        for n in 0..9 {
            let raw = if index == 2 && n == 0 {
                "https://pan.xunlei.com/s/direct?pwd=d4e5".into()
            } else {
                format!("{}+/{n}==", index * 2)
            };
            body += &event(index * 2, &raw);
            body += &event(index * 2, &raw);
        }
        body + "data: [DONE]\n\n"
    });
    sources(&server,streams,json!({"success":true,"data":{"data":[{"title":"Movie collection","quarkLink":"dailyQ","baiduLink":"dailyB"}]}})).await;
    let calls = Arc::new(Mutex::new(HashMap::<String, usize>::new()));
    let log = calls.clone();
    Mock::given(path("/api2/decrypt")).and(method("POST")).and(header("origin",server.uri())).respond_with(move|request:&Request|{
        let payload:Value=serde_json::from_slice(&request.body).unwrap();let raw=payload["encryptUrl"].as_str().unwrap();assert!(matches!(raw,"dailyQ"|"dailyB"));
        *log.lock().unwrap().entry(format!("decrypt:{raw}")).or_default()+=1;
        let host=if raw=="dailyQ"{"pan.quark.cn"}else{"pan.baidu.com"};ResponseTemplate::new(200).set_body_json(json!({"success":true,"data":{"url":format!("https://{host}/s/original?pwd=old1")}}))
    }).mount(&server).await;
    let log = calls.clone();
    Mock::given(path("/api/other/save_url"))
        .and(method("POST"))
        .and(header("origin", server.uri()))
        .and(header("content-type", "application/json"))
        .respond_with(move |request: &Request| {
            let payload: Value = serde_json::from_slice(&request.body).unwrap();
            let encoded = payload["url"].as_str().unwrap();
            let raw = percent_encoding::percent_decode_str(encoded)
                .decode_utf8()
                .unwrap()
                .into_owned();
            assert_eq!(component(&raw), encoded);
            let attempt = {
                let mut calls = log.lock().unwrap();
                let count = calls.entry(format!("save:{raw}")).or_default();
                *count += 1;
                *count
            };
            if raw == "0+/1==" {
                return ResponseTemplate::new(503);
            }
            let kind = if raw.starts_with("https://pan.baidu") {
                2
            } else if raw.starts_with("https://") {
                0
            } else {
                raw.split('+').next().unwrap().parse::<usize>().unwrap()
            };
            let id = if raw.starts_with("https://") {
                "daily".to_owned()
            } else {
                raw.split('/')
                    .nth(1)
                    .unwrap()
                    .trim_end_matches('=')
                    .to_owned()
            };
            let host = match kind {
                0 => "pan.quark.cn",
                2 => "pan.baidu.com",
                _ => "pan.xunlei.com",
            };
            let url = if raw == "4+/2==" {
                "https://buerchen.top/s/advert.html".to_owned()
            } else {
                format!("https://{host}/s/share{id}_{attempt}?pwd=6666")
            };
            ResponseTemplate::new(200).set_body_json(json!({"code":200,"data":{"url":url}}))
        })
        .mount(&server)
        .await;
    let provider = Buerchen::with_base(Url::parse(&server.uri()).unwrap());
    let mut first_ids = vec![];
    for run in 0..2 {
        let ctx = context();
        assert!(provider.search(&ctx, "Movie").await.is_err());
        let mut rows = ctx.finish(vec![]);
        assert_eq!(rows.len(), 16);
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        let ids = rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>();
        if run == 0 {
            first_ids = ids;
        } else {
            assert_eq!(ids, first_ids);
        }
        assert_eq!(
            rows.iter()
                .filter(|row| row.content.contains("每日更新"))
                .count(),
            2
        );
        rows[0].links[0].url = "caller mutation".into();
    }
    let calls = calls.lock().unwrap();
    assert_eq!(calls["decrypt:dailyQ"], 1);
    assert_eq!(calls["decrypt:dailyB"], 1);
    assert_eq!(calls["save:0+/0=="], 1);
    assert_eq!(calls["save:0+/1=="], 2);
    assert!(!calls.keys().any(|key| key.contains("xunlei.com/s/direct")));
}
#[tokio::test]
async fn daily_always_saves_and_keeps_original_identity_and_password() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::body_json;
    let server = MockServer::start().await;
    sources(
        &server,
        std::array::from_fn(|_| "data: [DONE]\n\n".into()),
        json!({"success":true,"data":{"data":[{"title":"Movie","quarkLink":"daily+/=="}]}}),
    )
    .await;
    Mock::given(path("/api2/decrypt"))
        .and(body_json(json!({"encryptUrl":"daily+/=="})))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"success":true,"data":{"url":"https://pan.quark.cn/s/original?pwd=old1"}}),
        ))
        .expect(2)
        .mount(&server)
        .await;
    let attempt = Arc::new(AtomicUsize::new(0));
    let calls = attempt.clone();
    Mock::given(path("/api/other/save_url"))
        .and(body_json(
            json!({"url":component("https://pan.quark.cn/s/original?pwd=old1"),"title":"Movie"}),
        ))
        .respond_with(move |_: &Request| {
            let url = if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                "https://pan.quark.cn/s/original"
            } else {
                "https://pan.quark.cn/s/new_generated"
            };
            ResponseTemplate::new(200).set_body_json(json!({"code":200,"data":{"url":url}}))
        })
        .expect(2)
        .mount(&server)
        .await;
    let provider = Buerchen::with_base(Url::parse(&server.uri()).unwrap());
    let first = provider.search(&context(), "Movie").await.unwrap();
    assert_eq!(first[0].links[0].password.as_deref(), Some("old1"));
    provider.state.lock().unwrap().cache.clear();
    let second = provider.search(&context(), "Movie").await.unwrap();
    assert_ne!(first[0].links[0].url, second[0].links[0].url);
    assert_eq!(first[0].id, second[0].id);
    assert_eq!(attempt.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn legal_empty_is_success_but_truncated_malformed_json_and_non200_fail() {
    for bad in [
        None,
        Some("data: not-json\n\n"),
        Some("data: [DONE] 暂无可用线路\n\n"),
        Some("<html>verification</html>"),
        Some("data: {\"title\":\"Movie\",\"url\":\"token\",\"is_type\":0}\n\n"),
    ] {
        let server = MockServer::start().await;
        sources(
            &server,
            [
                bad.unwrap_or("data: [DONE]\n\n").to_owned(),
                "data: [DONE]\n\n".into(),
                "data: [DONE]\n\n".into(),
            ],
            empty_daily(),
        )
        .await;
        let result = Buerchen::with_base(Url::parse(&server.uri()).unwrap())
            .search(&context(), "Movie")
            .await;
        assert_eq!(result.is_ok(), bad.is_none());
    }
    let server = MockServer::start().await;
    Mock::given(path("/api/other/web_search"))
        .respond_with(ResponseTemplate::new(503))
        .expect(3)
        .mount(&server)
        .await;
    Mock::given(path("/api2/content"))
        .respond_with(ResponseTemplate::new(200).set_body_json(empty_daily()))
        .mount(&server)
        .await;
    assert!(
        Buerchen::with_base(Url::parse(&server.uri()).unwrap())
            .search(&context(), "Movie")
            .await
            .is_err()
    );
}
#[tokio::test]
async fn gzip_sse_uses_shared_bounded_decoder_when_server_ignores_identity() {
    use std::io::Write;
    let server = MockServer::start().await;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all((event(0, "https://pan.quark.cn/s/gzip") + "data: [DONE]\n\n").as_bytes())
        .unwrap();
    let compressed = encoder.finish().unwrap();
    for index in 0..3 {
        let response = if index == 0 {
            ResponseTemplate::new(200)
                .insert_header("content-encoding", "gzip")
                .set_body_bytes(compressed.clone())
        } else {
            ResponseTemplate::new(200).set_body_string("data: [DONE]\n\n")
        };
        Mock::given(path("/api/other/web_search"))
            .and(query_param("is_type", (index * 2).to_string()))
            .respond_with(response.insert_header("content-type", "text/event-stream"))
            .mount(&server)
            .await;
    }
    Mock::given(path("/api2/content"))
        .respond_with(ResponseTemplate::new(200).set_body_json(empty_daily()))
        .mount(&server)
        .await;
    let client = crate::http::HttpClientFactory::new()
        .client(&crate::http::ClientOptions::default())
        .unwrap();
    let ctx = SearchContext::new(client, Duration::from_secs(5));
    let rows = Buerchen::with_base(Url::parse(&server.uri()).unwrap())
        .search(&ctx, "Movie")
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn json_challenge_starts_instance_cooldown_without_retries() {
    let server = MockServer::start().await;
    Mock::given(path("/api/other/web_search"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({"code":42901})),
        )
        .mount(&server)
        .await;
    Mock::given(path("/api2/content"))
        .respond_with(ResponseTemplate::new(200).set_body_json(empty_daily()))
        .mount(&server)
        .await;
    let provider = Buerchen::with_base(Url::parse(&server.uri()).unwrap());
    assert!(provider.search(&context(), "Movie").await.is_err());
    let count = server.received_requests().await.unwrap().len();
    assert!(matches!(
        provider.search(&context(), "Movie").await,
        Err(ProviderError::Blocked)
    ));
    assert_eq!(count, server.received_requests().await.unwrap().len());
}
#[tokio::test]
async fn stream_errors_preserve_completed_direct_share_and_limits_fail() {
    let server = MockServer::start().await;
    let body = event(0, "https://pan.quark.cn/s/fast") + "data: malformed\n\n";
    sources(
        &server,
        [body, "data: [DONE]\n\n".into(), "data: [DONE]\n\n".into()],
        empty_daily(),
    )
    .await;
    let provider = Buerchen::with_base(Url::parse(&server.uri()).unwrap());
    let ctx = context();
    assert!(provider.search(&ctx, "Movie").await.is_err());
    assert_eq!(ctx.finish(vec![]).len(), 1);
    for body in [
        format!(":{}\n", "x".repeat((256 << 10) + 1)),
        ": heartbeat\n".repeat(200_000),
    ] {
        let server = MockServer::start().await;
        sources(
            &server,
            [body, "data: [DONE]\n\n".into(), "data: [DONE]\n\n".into()],
            empty_daily(),
        )
        .await;
        assert!(
            Buerchen::with_base(Url::parse(&server.uri()).unwrap())
                .search(&context(), "Movie")
                .await
                .is_err()
        );
    }
}
#[tokio::test]
async fn deadline_drops_slow_resolves_and_shared_slots_never_exceed_three() {
    let server = MockServer::start().await;
    let streams = std::array::from_fn(|index| {
        (0..6)
            .map(|n| event(index * 2, &format!("{}+/{n}==", index * 2)))
            .collect::<String>()
            + "data: [DONE]\n\n"
    });
    sources(&server, streams, empty_daily()).await;
    Mock::given(path("/api/other/save_url"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"code":200,"data":{"url":"https://pan.quark.cn/s/slow"}}))
                .set_delay(Duration::from_secs(3)),
        )
        .mount(&server)
        .await;
    let provider = Buerchen::with_base(Url::parse(&server.uri()).unwrap());
    let ctx1 = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5))
        .for_source(Instant::now() + Duration::from_millis(250));
    let ctx2 = ctx1.clone();
    let (first, second) = tokio::join!(
        provider.search(&ctx1, "Movie"),
        provider.search(&ctx2, "Movie")
    );
    assert!(first.is_err() && second.is_err());
    let count = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/api/other/save_url")
        .count();
    assert!(count > 0 && count <= 3);
    assert_eq!(provider.slots.available_permits(), 3);
}
