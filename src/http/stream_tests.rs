use super::*;
use std::{io::Write, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}
fn zlib(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}
async fn fixture_response(bytes: Vec<u8>, encoding: &str) -> (MockServer, Response) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-encoding", encoding)
                .set_body_bytes(bytes),
        )
        .mount(&server)
        .await;
    let client = super::super::HttpClientFactory::new()
        .client(&Default::default())
        .unwrap();
    let response = super::super::execute(client.get(server.uri()))
        .await
        .unwrap();
    (server, response)
}
async fn collect(response: Response, limit: usize) -> anyhow::Result<Vec<u8>> {
    let chunks: Vec<Vec<u8>> = stream_response(response, limit)?.try_collect().await?;
    Ok(chunks.concat())
}

#[tokio::test]
async fn chunked_wire_limit_passes_exactly_and_rejects_one_extra_byte() {
    for limit in [4, 5] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let serve = async {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                let mut buffer = [0; 1024];
                let length = socket.read(&mut buffer).await.unwrap();
                assert_ne!(length, 0);
                request.extend_from_slice(&buffer[..length]);
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\n12345\r\n0\r\n\r\n").await.unwrap();
        };
        let consume = async {
            let response = reqwest::Client::new().get(url).send().await.unwrap();
            assert_eq!(response.content_length(), None);
            let mut stream = stream_response(response, limit).unwrap();
            let mut bytes = Vec::new();
            let mut failed = false;
            while let Some(piece) = stream.next().await {
                match piece {
                    Ok(piece) => bytes.extend(piece),
                    Err(error) => {
                        assert!(error.is::<ResponseTooLarge>(), "{error:?}");
                        failed = true;
                    }
                }
            }
            assert_eq!(bytes, &b"12345"[..limit]);
            assert_eq!(failed, limit == 4);
        };
        tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(serve, consume)
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn supported_codings_and_reverse_order() {
    let plain = b"incremental response";
    let mut br = Vec::new();
    {
        let mut encoder = brotli::CompressorWriter::new(&mut br, 4096, 5, 22);
        encoder.write_all(plain).unwrap();
    }
    for (coding, encoded) in [
        ("identity", plain.to_vec()),
        ("gzip", gzip(plain)),
        ("x-gzip", gzip(plain)),
        ("deflate", zlib(plain)),
        ("br", br),
        ("deflate, gzip", gzip(&zlib(plain))),
    ] {
        let (_server, response) = fixture_response(encoded, coding).await;
        assert_eq!(collect(response, 0).await.unwrap(), plain, "{coding}");
    }
}

#[tokio::test]
async fn concatenated_members_and_invalid_encodings() {
    let mut members = gzip(b"first");
    members.extend(gzip(b"second"));
    let (_server, response) = fixture_response(members, "gzip").await;
    assert_eq!(collect(response, 0).await.unwrap(), b"firstsecond");
    let mut corrupt = gzip(b"valid prefix");
    corrupt.truncate(corrupt.len() - 4);
    let (_server, response) = fixture_response(corrupt, "gzip").await;
    assert!(collect(response, 0).await.is_err());
    for coding in [
        "unknown".to_owned(),
        std::iter::repeat_n("gzip", 9).collect::<Vec<_>>().join(","),
    ] {
        let (_server, response) = fixture_response(Vec::new(), &coding).await;
        assert!(stream_response(response, 0).is_err());
    }
}

#[tokio::test]
async fn known_wire_and_decoded_limits_are_exact() {
    let (_server, response) = fixture_response(b"12345".to_vec(), "identity").await;
    assert!(
        stream_response(response, 4)
            .err()
            .unwrap()
            .is::<ResponseTooLarge>()
    );
    let (_server, response) = fixture_response(b"1234".to_vec(), "identity").await;
    assert_eq!(collect(response, 4).await.unwrap(), b"1234");
    for length in [1024, 1025] {
        let (_server, response) = fixture_response(gzip(&vec![b'x'; length]), "gzip").await;
        let result = collect(response, 1024).await;
        if length == 1024 {
            assert_eq!(result.unwrap().len(), 1024);
        } else {
            assert!(result.unwrap_err().is::<ResponseTooLarge>());
        }
    }
    let (_server, response) = fixture_response(gzip(&vec![0; 400 * 1024 + 1]), "gzip").await;
    assert!(
        collect(response, 0)
            .await
            .unwrap_err()
            .is::<ResponseTooLarge>()
    );
}

#[tokio::test]
async fn nested_encoding_cannot_hide_oversized_intermediate_output() {
    let inner = gzip(b"small final output");
    // A long legal gzip comment produces a huge intermediate representation,
    // even though the innermost decoded payload is tiny.
    let mut padded = inner[..10].to_vec();
    padded[3] |= 0x10;
    padded.extend(std::iter::repeat_n(b'x', 450 * 1024));
    padded.push(0);
    padded.extend_from_slice(&inner[10..]);
    let (_server, response) = fixture_response(gzip(&padded), "gzip, gzip").await;
    assert!(
        collect(response, 0)
            .await
            .unwrap_err()
            .is::<ResponseTooLarge>()
    );
}

#[tokio::test]
async fn compressed_prefix_is_available_before_http_eof() {
    for coding in ["gzip", "deflate"] {
        let plain = b"event: snapshot\ndata: {\"ready\":true}\n\n";
        let (prefix, tail) = if coding == "gzip" {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(plain).unwrap();
            encoder.flush().unwrap();
            let prefix = encoder.get_ref().clone();
            let all = encoder.finish().unwrap();
            (prefix.clone(), all[prefix.len()..].to_vec())
        } else {
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(plain).unwrap();
            encoder.flush().unwrap();
            let prefix = encoder.get_ref().clone();
            let all = encoder.finish().unwrap();
            (prefix.clone(), all[prefix.len()..].to_vec())
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let (ready, acknowledged) = tokio::sync::oneshot::channel();
        let serve = async {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                let mut buffer = [0; 1024];
                let length = socket.read(&mut buffer).await.unwrap();
                assert_ne!(length, 0);
                request.extend_from_slice(&buffer[..length]);
            }
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Encoding: {coding}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            socket
                .write_all(format!("{:x}\r\n", prefix.len()).as_bytes())
                .await
                .unwrap();
            socket.write_all(&prefix).await.unwrap();
            socket.write_all(b"\r\n").await.unwrap();
            acknowledged.await.unwrap();
            socket
                .write_all(format!("{:x}\r\n", tail.len()).as_bytes())
                .await
                .unwrap();
            socket.write_all(&tail).await.unwrap();
            socket.write_all(b"\r\n0\r\n\r\n").await.unwrap();
        };
        let consume = async {
            let client = super::super::HttpClientFactory::new()
                .client(&Default::default())
                .unwrap();
            let response = super::super::execute(client.get(url)).await.unwrap();
            let mut stream = stream_response(response, 0).unwrap();
            let mut bytes = stream.next().await.unwrap().unwrap();
            assert!(!bytes.is_empty());
            ready.send(()).unwrap();
            while let Some(piece) = stream.next().await {
                bytes.extend(piece.unwrap());
            }
            assert_eq!(bytes, plain);
        };
        tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(serve, consume)
        })
        .await
        .unwrap();
    }
}
