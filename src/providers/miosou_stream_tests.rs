//! Real chunked HTTP: successful snapshots must survive a stalled or broken body.
use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
const SNAPSHOT: &str = "event: snapshot\ndata: {\"merged_by_type\":{\"quark\":[{\"stream_key\":\"one\",\"url\":\"https://pan.quark.cn/s/fixture\",\"note\":\"电影 film\"}]}}\n\n";
async fn headers(socket: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
        let mut part = [0; 1024];
        let count = socket.read(&mut part).await.unwrap();
        assert!(count > 0 && bytes.len() < 8192);
        bytes.extend_from_slice(&part[..count]);
    }
    String::from_utf8(bytes).unwrap()
}
async fn serve(
    listener: TcpListener,
    chunks: Vec<Vec<u8>>,
    finish: bool,
    stall: Duration,
    detail: bool,
) {
    let (mut gate, _) = listener.accept().await.unwrap();
    assert!(headers(&mut gate).await.starts_with("GET / HTTP/1.1"));
    gate.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nready")
        .await
        .unwrap();
    drop(gate);
    let (mut socket, _) = listener.accept().await.unwrap();
    let request = headers(&mut socket).await;
    assert!(request.starts_with("GET /miov1/search?keyword=film "));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("accept-encoding: identity")
    );
    socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n")
        .await.unwrap();
    for chunk in chunks {
        // An overflow test may drop its response before the entire tail is sent.
        if socket
            .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
            .await
            .is_err()
            || socket.write_all(&chunk).await.is_err()
            || socket.write_all(b"\r\n").await.is_err()
        {
            return;
        }
    }
    if detail {
        let (mut socket, _) = listener.accept().await.unwrap();
        assert!(
            headers(&mut socket)
                .await
                .starts_with("GET /miov1/share/detail?share_ref=fixture ")
        );
        let body = r#"{"display_url":"https://pan.quark.cn/s/resolved"}"#;
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    }
    tokio::time::sleep(stall).await;
    if finish {
        let _ = socket.write_all(b"0\r\n\r\n").await;
    }
}
async fn boundary(chunks: Vec<Vec<u8>>, finish: bool, stalled: bool, detail: bool) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let source = Miosou::new(base);
    let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(3)).for_source(
        tokio::time::Instant::now()
            + if stalled {
                Duration::from_millis(300)
            } else {
                Duration::from_secs(3)
            },
    );
    let work = async {
        let result = source.search(&ctx, "film").await;
        let error = result
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default();
        if stalled {
            assert!(matches!(result, Err(ProviderError::Timeout)));
        } else if finish {
            assert_eq!(result.unwrap().len(), 1);
        } else {
            assert!(result.is_err());
        }
        let rows = ctx.finish(Vec::new());
        assert_eq!(rows.len(), 1, "completed snapshot lost before EOF/error");
        if detail {
            assert!(rows[0].links[0].url.ends_with("/resolved"));
        }
        assert!(!ctx.publish(Vec::new()));
        error
    };
    // Both server and provider are owned futures; cancellation cannot leave a task.
    let (_, error) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            serve(
                listener,
                chunks,
                finish,
                if stalled {
                    Duration::from_millis(600)
                } else {
                    Duration::ZERO
                },
                detail
            ),
            work
        )
    })
    .await
    .unwrap();
    error
}
#[tokio::test]
async fn snapshot_is_published_before_stalled_stream_eof_and_deadline() {
    boundary(vec![SNAPSHOT.as_bytes().to_vec()], false, true, false).await;
}
#[tokio::test]
async fn share_detail_can_finish_while_stream_is_still_open() {
    let snapshot = "event: snapshot\ndata: {\"merged_by_type\":{\"quark\":[{\"share_ref\":\"fixture\",\"note\":\"film\"}]}}\n\n";
    boundary(vec![snapshot.as_bytes().to_vec()], false, true, true).await;
}
#[tokio::test]
async fn fragmented_utf8_lines_and_crlf_are_reassembled() {
    let snapshot = SNAPSHOT.replace('\n', "\r\n");
    let split = snapshot.find('电').unwrap() + 1;
    boundary(
        vec![
            snapshot.as_bytes()[..split].to_vec(),
            snapshot.as_bytes()[split..].to_vec(),
        ],
        true,
        false,
        false,
    )
    .await;
}
#[tokio::test]
async fn final_snapshot_without_blank_line_is_flushed_at_eof() {
    boundary(
        vec![SNAPSHOT.trim_end_matches('\n').as_bytes().to_vec()],
        true,
        false,
        false,
    )
    .await;
}
#[tokio::test]
async fn broken_http_body_retains_complete_snapshot() {
    boundary(vec![SNAPSHOT.as_bytes().to_vec()], false, false, false).await;
}
#[tokio::test]
async fn invalid_utf8_tail_retains_complete_snapshot() {
    let error = boundary(
        vec![SNAPSHOT.as_bytes().to_vec(), vec![0xff, b'\n']],
        false,
        false,
        false,
    )
    .await;
    assert!(error.contains("not UTF-8"), "{error}");
}
#[tokio::test]
async fn overlong_unterminated_line_retains_complete_snapshot() {
    let error = boundary(
        vec![SNAPSHOT.as_bytes().to_vec(), vec![b'x'; SSE_LINE_LIMIT + 1]],
        false,
        false,
        false,
    )
    .await;
    assert!(error.contains("line exceeds 2MiB"), "{error}");
}
#[tokio::test]
async fn raw_limit_counts_ignored_comments_and_retains_complete_snapshot() {
    let mut chunks = vec![SNAPSHOT.as_bytes().to_vec()];
    let mut comment = vec![b':'; 64 * 1024 - 1];
    comment.push(b'\n');
    for _ in 0..256 {
        chunks.push(comment.clone());
    }
    let error = boundary(chunks, false, false, false).await;
    assert!(error.contains("body exceeds 16MiB"), "{error}");
}
