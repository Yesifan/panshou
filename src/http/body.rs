use std::io::Read;

use anyhow::{Context, bail};
use reqwest::{Response, header::CONTENT_ENCODING};
use thiserror::Error;

pub const DEFAULT_RESPONSE_LIMIT: usize = 16 * 1024 * 1024;
pub const MAX_DECOMPRESSED_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Error)]
#[error("HTTP response exceeds {limit} byte limit")]
pub struct ResponseTooLarge {
    pub limit: usize,
}

/// Read at most limit+1 bytes, reporting overflow rather than truncating input.
/// Factory clients disable automatic decompression so both wire and decoded
/// lengths can be bounded. A zero limit selects the 16 MiB default.
pub async fn read_response(mut response: Response, limit: usize) -> anyhow::Result<Vec<u8>> {
    let limit = if limit == 0 {
        DEFAULT_RESPONSE_LIMIT
    } else {
        limit
    };
    let encoding = response
        .headers()
        .get(CONTENT_ENCODING)
        .map(|value| value.to_str().map(str::to_owned))
        .transpose()
        .context("invalid response content encoding")?;
    let known_length = response.content_length();
    if known_length.is_some_and(|length| length > limit as u64) {
        return Err(ResponseTooLarge { limit }.into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| error.without_url())?
    {
        let remaining = limit.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            return Err(ResponseTooLarge { limit }.into());
        }
        bytes.extend_from_slice(&chunk);
    }
    // Drop the response (and any outbound permit) before CPU decoding.
    drop(response);
    let mut decoded_limit = decompression_limit(known_length);
    // Preserve smaller source-specific limits (for example TG's 2 MiB) on
    // decoded content as well as the encoded wire body.
    if limit < DEFAULT_RESPONSE_LIMIT {
        decoded_limit = decoded_limit.min(limit);
    }
    decode_encoded(bytes, encoding.as_deref(), decoded_limit).await
}

pub fn decompression_limit(compressed_length: Option<u64>) -> usize {
    match compressed_length.filter(|length| *length > 0) {
        Some(length) => length
            .saturating_mul(100)
            .clamp(400 * 1024, MAX_DECOMPRESSED_BYTES as u64) as usize,
        None => MAX_DECOMPRESSED_BYTES,
    }
}

async fn decode_encoded(
    mut bytes: Vec<u8>,
    encoding: Option<&str>,
    limit: usize,
) -> anyhow::Result<Vec<u8>> {
    // Content codings are applied in order and must be decoded in reverse.
    for coding in encoding.unwrap_or("identity").split(',').rev() {
        let reader: Box<dyn Read + Send + '_> = match coding.trim().to_ascii_lowercase().as_str() {
            "" | "identity" => continue,
            "gzip" | "x-gzip" => Box::new(flate2::read::MultiGzDecoder::new(bytes.as_slice())),
            "deflate" => Box::new(flate2::read::ZlibDecoder::new(bytes.as_slice())),
            "br" => Box::new(brotli::Decompressor::new(bytes.as_slice(), 4096)),
            _ => bail!("unsupported HTTP content encoding"),
        };
        bytes = read_limited_cooperative(reader, limit).await?;
    }
    Ok(bytes)
}

// Decode bounded pieces on the owning task, yielding before each piece. This
// keeps command cancellation/deadlines effective even with in-memory input;
// there is no detached spawn_blocking work after the caller drops its future.
async fn read_limited_cooperative(
    mut reader: impl Read + Send,
    limit: usize,
) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        tokio::task::yield_now().await;
        let remaining = limit.saturating_sub(bytes.len()).saturating_add(1);
        let length = buffer.len().min(remaining);
        let count = reader
            .read(&mut buffer[..length])
            .context("failed to decode bounded response")?;
        if count == 0 {
            return Ok(bytes);
        }
        if count > limit.saturating_sub(bytes.len()) {
            return Err(ResponseTooLarge { limit }.into());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}

/// For synchronous explicit streaming decoders and caches. Reads only limit+1 bytes.
pub fn read_limited(reader: impl Read, limit: usize) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((limit as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .context("failed to decode bounded response")?;
    if bytes.len() > limit {
        return Err(ResponseTooLarge { limit }.into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    #[test]
    fn exact_and_overflow() {
        assert_eq!(read_limited(Cursor::new(b"1234"), 4).unwrap(), b"1234");
        assert!(
            read_limited(Cursor::new(b"12345"), 4)
                .unwrap_err()
                .is::<ResponseTooLarge>()
        );
        assert!(
            read_limited(std::io::repeat(0), 4)
                .unwrap_err()
                .is::<ResponseTooLarge>()
        );
    }

    #[tokio::test]
    async fn compressed_limits() {
        assert_eq!(decompression_limit(None), MAX_DECOMPRESSED_BYTES);
        assert_eq!(decompression_limit(Some(0)), MAX_DECOMPRESSED_BYTES);
        assert_eq!(decompression_limit(Some(10)), 400 * 1024);
        assert_eq!(decompression_limit(Some(5000)), 500_000);
        assert_eq!(decompression_limit(Some(u64::MAX)), MAX_DECOMPRESSED_BYTES);
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&vec![0; DEFAULT_RESPONSE_LIMIT]).unwrap();
        let encoded = encoder.finish().unwrap();
        let limit = decompression_limit(Some(encoded.len() as u64));
        assert!(
            decode_encoded(encoded, Some("gzip"), limit)
                .await
                .unwrap_err()
                .is::<ResponseTooLarge>()
        );
    }

    #[tokio::test]
    async fn wire_body_limits_and_decoding() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        let server = MockServer::start().await;
        Mock::given(path("/exact"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"1234".to_vec()))
            .mount(&server)
            .await;
        Mock::given(path("/overflow"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"12345".to_vec()))
            .mount(&server)
            .await;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(b"hello compressed").unwrap();
        let compressed = encoder.finish().unwrap();
        Mock::given(path("/gzip"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Encoding", "gzip")
                    .set_body_bytes(compressed),
            )
            .mount(&server)
            .await;
        let client = crate::http::HttpClientFactory::new()
            .client(&crate::http::ClientOptions::default())
            .unwrap();
        assert_eq!(
            read_response(
                client
                    .get(format!("{}/exact", server.uri()))
                    .send()
                    .await
                    .unwrap(),
                4
            )
            .await
            .unwrap(),
            b"1234"
        );
        assert!(
            read_response(
                client
                    .get(format!("{}/overflow", server.uri()))
                    .send()
                    .await
                    .unwrap(),
                4
            )
            .await
            .unwrap_err()
            .is::<ResponseTooLarge>()
        );
        assert_eq!(
            read_response(
                client
                    .get(format!("{}/gzip", server.uri()))
                    .send()
                    .await
                    .unwrap(),
                100
            )
            .await
            .unwrap(),
            b"hello compressed"
        );
    }

    async fn chunked_response(body: &'static [u8]) -> Response {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            stream.write_all(body).unwrap();
        });
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn unknown_length_and_mid_stream_error() {
        let response = chunked_response(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n12345\r\n0\r\n\r\n",
        )
        .await;
        assert!(
            read_response(response, 4)
                .await
                .unwrap_err()
                .is::<ResponseTooLarge>()
        );
        let response = chunked_response(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n1234\r\n0\r\n\r\n",
        )
        .await;
        assert_eq!(read_response(response, 4).await.unwrap(), b"1234");
        let response =
            chunked_response(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort").await;
        assert!(read_response(response, 10).await.is_err());
    }

    #[tokio::test]
    async fn absolute_decompression_boundary() {
        assert!(
            read_limited(std::io::repeat(0), MAX_DECOMPRESSED_BYTES)
                .unwrap_err()
                .is::<ResponseTooLarge>()
        );
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(b"12345").unwrap();
        let encoded = encoder.finish().unwrap();
        assert_eq!(
            decode_encoded(encoded.clone(), Some("deflate"), 5)
                .await
                .unwrap(),
            b"12345"
        );
        assert!(
            decode_encoded(encoded, Some("deflate"), 4)
                .await
                .unwrap_err()
                .is::<ResponseTooLarge>()
        );
    }

    #[tokio::test]
    async fn tg_limit_applies_to_decoded_content() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        let server = MockServer::start().await;
        let limit = 2 * 1024 * 1024;
        let mut data = vec![0u8; limit + 1];
        let mut state = 42u32;
        for byte in &mut data[..40_000] {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            *byte = (state >> 24) as u8;
        }
        for (path_name, length) in [("/exact", limit), ("/overflow", limit + 1)] {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(&data[..length]).unwrap();
            let encoded = encoder.finish().unwrap();
            assert!(encoded.len() < limit);
            assert!(decompression_limit(Some(encoded.len() as u64)) > limit + 1);
            Mock::given(path(path_name))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("Content-Encoding", "gzip")
                        .set_body_bytes(encoded),
                )
                .mount(&server)
                .await;
        }
        let client = crate::http::HttpClientFactory::new()
            .client(&crate::http::ClientOptions::default())
            .unwrap();
        let exact = client
            .get(format!("{}/exact", server.uri()))
            .send()
            .await
            .unwrap();
        assert_eq!(read_response(exact, limit).await.unwrap().len(), limit);
        let overflow = client
            .get(format!("{}/overflow", server.uri()))
            .send()
            .await
            .unwrap();
        assert_eq!(
            read_response(overflow, limit)
                .await
                .unwrap_err()
                .downcast_ref::<ResponseTooLarge>()
                .unwrap()
                .limit,
            limit
        );
    }

    #[tokio::test]
    async fn decoding_yields_and_drop_stops_work() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct Counted(Arc<AtomicUsize>);
        impl Read for Counted {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                self.0.fetch_add(output.len(), Ordering::SeqCst);
                output.fill(0);
                Ok(output.len())
            }
        }
        let count = Arc::new(AtomicUsize::new(0));
        let mut future = Box::pin(read_limited_cooperative(
            Counted(count.clone()),
            MAX_DECOMPRESSED_BYTES,
        ));
        assert!(futures::poll!(future.as_mut()).is_pending());
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert!(futures::poll!(future.as_mut()).is_pending());
        assert_eq!(count.load(Ordering::SeqCst), 8192);
        // The reader is owned by the future, not a detached worker.
        drop(future);
        tokio::task::yield_now().await;
        assert_eq!(count.load(Ordering::SeqCst), 8192);
    }

    #[tokio::test]
    async fn every_content_coding_layer_is_bounded() {
        let mut inner = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        inner.write_all(&vec![b'x'; 100_000]).unwrap();
        let mut outer = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        outer.write_all(&inner.finish().unwrap()).unwrap();
        let encoded = outer.finish().unwrap();
        assert_eq!(
            decode_encoded(encoded.clone(), Some("gzip, deflate"), 100_000)
                .await
                .unwrap()
                .len(),
            100_000
        );
        assert!(
            decode_encoded(encoded, Some("gzip, deflate"), 99_999)
                .await
                .unwrap_err()
                .is::<ResponseTooLarge>()
        );
    }

    #[tokio::test]
    async fn small_repetitive_payload_is_allowed() {
        let data = vec![b'a'; 100_000];
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&data).unwrap();
        let encoded = encoder.finish().unwrap();
        let limit = decompression_limit(Some(encoded.len() as u64));
        assert_eq!(
            decode_encoded(encoded, Some("gzip"), limit).await.unwrap(),
            data
        );
    }
}
