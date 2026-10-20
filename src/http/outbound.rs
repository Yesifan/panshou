use std::sync::{Arc, OnceLock};

use reqwest::{RequestBuilder, Response};
use tokio::sync::Semaphore;

pub const DEFAULT_OUTBOUND_CONCURRENCY: usize = 128;

fn shared_semaphore() -> &'static Arc<Semaphore> {
    static SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEMAPHORE.get_or_init(|| {
        let limit = std::env::var("OUTBOUND_MAX_CONCURRENCY")
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|limit| *limit > 0 && *limit <= Semaphore::MAX_PERMITS)
            .unwrap_or(DEFAULT_OUTBOUND_CONCURRENCY);
        Arc::new(Semaphore::new(limit))
    })
}

/// Shared actual-request admission. Waiting and sending are canceled by drop.
/// The permit lives with the response through chunked reads/read_response and
/// is released on EOF handling, overflow, error, or drop. Callers must use
/// read_response rather than reqwest's consuming text/json/bytes helpers (which
/// drop response extensions before collecting the body).
///
/// Raw Client::send/execute calls bypass this boundary: migrate every HTTP
/// entry point to this function to obtain command-wide request admission.
pub async fn execute(request: RequestBuilder) -> anyhow::Result<Response> {
    execute_with(shared_semaphore(), request).await
}

async fn execute_with(
    semaphore: &Arc<Semaphore>,
    request: RequestBuilder,
) -> anyhow::Result<Response> {
    let permit = semaphore.clone().acquire_owned().await?;
    let mut response = request.send().await.map_err(|error| error.without_url())?;
    response.extensions_mut().insert(Arc::new(permit));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

    #[tokio::test]
    async fn incremental_stream_keeps_permit_until_drop_or_eof() {
        use futures::StreamExt;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 128 * 1024]))
            .mount(&server)
            .await;
        let semaphore = Arc::new(Semaphore::new(1));
        let client = reqwest::Client::new();
        for drain in [false, true] {
            let response = execute_with(&semaphore, client.get(server.uri()))
                .await
                .unwrap();
            let mut stream = super::super::stream_response(response, 0).unwrap();
            assert!(!stream.next().await.unwrap().unwrap().is_empty());
            assert_eq!(semaphore.available_permits(), 0);
            if drain {
                while let Some(piece) = stream.next().await {
                    piece.unwrap();
                }
                assert_eq!(semaphore.available_permits(), 1);
            }
            drop(stream);
            assert_eq!(semaphore.available_permits(), 1);
        }
    }

    #[tokio::test]
    async fn holds_permit_until_body_dropped_and_wait_is_cancel_safe() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("hello"))
            .expect(2)
            .mount(&server)
            .await;
        let semaphore = Arc::new(Semaphore::new(1));
        let client = reqwest::Client::new();
        let response = execute_with(&semaphore, client.get(server.uri()))
            .await
            .unwrap();
        assert_eq!(semaphore.available_permits(), 0);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(10),
                execute_with(&semaphore, client.get(server.uri()))
            )
            .await
            .is_err()
        );
        assert_eq!(
            super::super::read_response(response, 5).await.unwrap(),
            b"hello"
        );
        assert_eq!(semaphore.available_permits(), 1);
        let response = execute_with(&semaphore, client.get(server.uri()))
            .await
            .unwrap();
        drop(response);
        assert_eq!(semaphore.available_permits(), 1);
    }
}
