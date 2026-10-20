use serde_json::Value;

fn response_error(error: anyhow::Error) -> super::CheckError {
    match error.downcast::<reqwest::Error>() {
        Ok(error) => super::CheckError::Network(error.without_url()),
        Err(_) => super::CheckError::Protocol("response body read failed or exceeded limit".into()),
    }
}

pub(crate) trait BoundedRequest {
    fn gated_send(
        self,
    ) -> impl std::future::Future<Output = Result<reqwest::Response, super::CheckError>> + Send;
}

impl BoundedRequest for reqwest::RequestBuilder {
    async fn gated_send(self) -> Result<reqwest::Response, super::CheckError> {
        crate::http::execute(self).await.map_err(response_error)
    }
}

pub(crate) trait BoundedResponse {
    fn bounded_bytes(
        self,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, super::CheckError>> + Send;
}

impl BoundedResponse for reqwest::Response {
    async fn bounded_bytes(self) -> Result<Vec<u8>, super::CheckError> {
        crate::http::read_response(self, 16 * 1024 * 1024)
            .await
            .map_err(response_error)
    }
}

pub(crate) fn contains_any(text: &str, words: &[&str]) -> bool {
    let text = text.to_lowercase();
    words.iter().any(|word| text.contains(&word.to_lowercase()))
}
pub(crate) fn string<'a>(v: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter()
        .find_map(|k| {
            v.get(k)
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or("")
}
pub(crate) fn number(v: &Value, keys: &[&str]) -> i64 {
    keys.iter()
        .find_map(|k| v.get(k).and_then(Value::as_i64))
        .unwrap_or_default()
}
pub(crate) fn json(body: &[u8]) -> Result<Value, super::CheckError> {
    serde_json::from_slice(body).map_err(|e| super::CheckError::Parse(e.to_string()))
}

#[cfg(test)]
mod body_tests {
    use super::{BoundedRequest, BoundedResponse};
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

    #[tokio::test]
    async fn checker_body_accepts_exact_limit_and_rejects_overflow() {
        let server = MockServer::start().await;
        const LIMIT: usize = 16 * 1024 * 1024;
        for (route, size) in [("exact", LIMIT), ("overflow", LIMIT + 1)] {
            Mock::given(path(format!("/{route}")))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; size]))
                .mount(&server)
                .await;
        }
        let client = reqwest::Client::new();
        let exact = client
            .get(format!("{}/exact", server.uri()))
            .gated_send()
            .await
            .unwrap()
            .bounded_bytes()
            .await
            .unwrap();
        assert_eq!(exact.len(), LIMIT);
        let overflow = client
            .get(format!("{}/overflow", server.uri()))
            .gated_send()
            .await
            .unwrap()
            .bounded_bytes()
            .await;
        assert!(matches!(
            overflow,
            Err(crate::check::CheckError::Protocol(_))
        ));
    }
}
