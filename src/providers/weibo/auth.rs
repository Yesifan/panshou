use std::{collections::BTreeMap, path::Path};

use reqwest::{Client, Response, header};
use url::Url;

use crate::core::ProviderError;
use crate::http::Session;

use super::WeiboEndpoints;

const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120.0.0.0 Safari/537.36";

#[derive(Clone)]
pub struct QrChallenge {
    pub qrid: String,
    pub image_png: Vec<u8>,
}

impl QrChallenge {
    /// Persist the original QR bitmap for terminals which cannot render images.
    pub fn write_png(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        std::fs::write(path, &self.image_png)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QrStatus {
    Waiting,
    Scanned,
    Expired,
    Success,
}

#[derive(Clone)]
pub struct QrPoll {
    pub status: QrStatus,
    pub message: String,
    pub cookie: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WeiboAuth {
    client: Client,
    session: Option<Session>,
    endpoints: WeiboEndpoints,
}

impl WeiboAuth {
    pub fn new(client: Client, endpoints: WeiboEndpoints) -> Self {
        Self {
            client,
            session: None,
            endpoints,
        }
    }

    /// Prefer this constructor for login so every profile gets an isolated jar.
    pub fn with_session(session: Session, endpoints: WeiboEndpoints) -> Self {
        Self {
            client: session.client().clone(),
            session: Some(session),
            endpoints,
        }
    }

    pub async fn begin(&self) -> Result<QrChallenge, ProviderError> {
        let callback = format!("STK_{}", chrono::Utc::now().timestamp_millis());
        let response = crate::http::execute(
            self.client
                .get(self.endpoints.qr_info.clone())
                .query(&[
                    ("entry", "miniblog"),
                    ("size", "180"),
                    ("callback", callback.as_str()),
                ])
                .header(header::USER_AGENT, USER_AGENT)
                .timeout(std::time::Duration::from_secs(30))
                .header(header::REFERER, self.endpoints.web_origin.as_str()),
        )
        .await
        .map_err(network)?
        .error_for_status()
        .map_err(network)?;
        let body = super::search::read_text(response).await?;
        let value = jsonp_value(&body)?;
        let qrid = value
            .pointer("/data/qrid")
            .and_then(|v| v.as_str())
            .or_else(|| value.get("qrid").and_then(|v| v.as_str()))
            .ok_or_else(|| ProviderError::Protocol("Weibo QR response omitted qrid".into()))?;
        let image_url = value
            .pointer("/data/image")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::Protocol("Weibo QR response omitted image URL".into()))?;
        let image_url = qr_image_url(&self.endpoints.qr_image, image_url)?;
        let image_response = crate::http::execute(
            self.client
                .get(image_url)
                .header(header::USER_AGENT, USER_AGENT)
                .timeout(std::time::Duration::from_secs(30))
                .header(header::REFERER, self.endpoints.web_origin.as_str()),
        )
        .await
        .map_err(network)?
        .error_for_status()
        .map_err(network)?;
        let image_png = crate::http::read_response(image_response, 16 * 1024 * 1024)
            .await
            .map_err(|_| {
                ProviderError::Network("Weibo QR image read failed or exceeded limit".into())
            })?;
        if !image_png.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(ProviderError::Protocol(
                "Weibo QR image endpoint did not return PNG data".into(),
            ));
        }
        Ok(QrChallenge {
            qrid: qrid.to_owned(),
            image_png,
        })
    }

    pub async fn poll(&self, qrid: &str) -> Result<QrPoll, ProviderError> {
        let callback = format!("STK_{}", chrono::Utc::now().timestamp_millis());
        let response = crate::http::execute(
            self.client
                .get(self.endpoints.qr_check.clone())
                .query(&[
                    ("entry", "sso"),
                    ("qrid", qrid),
                    ("callback", callback.as_str()),
                ])
                .header(header::USER_AGENT, USER_AGENT)
                .timeout(std::time::Duration::from_secs(30))
                .header(header::REFERER, self.endpoints.web_origin.as_str()),
        )
        .await
        .map_err(network)?
        .error_for_status()
        .map_err(network)?;
        let value = jsonp_value(&super::search::read_text(response).await?)?;
        let (status, message, alt) = classify_poll(&value)?;
        match status {
            QrStatus::Success => {
                let cookie = self
                    .initialize_cookie(alt.as_deref().unwrap_or_default())
                    .await?;
                Ok(QrPoll {
                    status,
                    message,
                    cookie: Some(cookie),
                })
            }
            _ => Ok(QrPoll {
                status,
                message,
                cookie: None,
            }),
        }
    }

    async fn initialize_cookie(&self, start: &str) -> Result<String, ProviderError> {
        if let Some(session) = &self.session {
            let start = Url::parse(start).map_err(|e| ProviderError::Protocol(e.to_string()))?;
            crate::http::execute(
                session
                    .client()
                    .get(start)
                    .header(header::USER_AGENT, USER_AGENT)
                    .timeout(std::time::Duration::from_secs(30)),
            )
            .await
            .map_err(network)?
            .error_for_status()
            .map_err(network)?;
            for url in [&self.endpoints.web_origin, &self.endpoints.mobile_origin] {
                crate::http::execute(
                    session
                        .client()
                        .get(url.clone())
                        .header(header::USER_AGENT, USER_AGENT)
                        .timeout(std::time::Duration::from_secs(30)),
                )
                .await
                .map_err(network)?
                .error_for_status()
                .map_err(network)?;
            }
            let mut values = Vec::new();
            for url in [&self.endpoints.web_origin, &self.endpoints.mobile_origin] {
                if let Some(value) = session.cookie_header(url) {
                    values.push(value);
                }
            }
            let combined = merge_cookie_headers(&values);
            if combined.is_empty() {
                return Err(ProviderError::Protocol(
                    "Weibo login completed without cookies".into(),
                ));
            }
            return Ok(combined);
        }
        let mut next = Url::parse(start).map_err(|e| ProviderError::Protocol(e.to_string()))?;
        let mut cookies = BTreeMap::<String, String>::new();
        for _ in 0..10 {
            let response = crate::http::execute(
                self.client
                    .get(next.clone())
                    .header(header::USER_AGENT, USER_AGENT)
                    .timeout(std::time::Duration::from_secs(30)),
            )
            .await
            .map_err(network)?;
            collect_cookies(&response, &mut cookies);
            if !response.status().is_redirection() {
                break;
            }
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    ProviderError::Protocol("Weibo login redirect omitted Location".into())
                })?;
            next = next
                .join(location)
                .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        }
        // Seed both sites and collect their short-lived cookies.
        for url in [&self.endpoints.web_origin, &self.endpoints.mobile_origin] {
            let response = crate::http::execute(
                self.client
                    .get(url.clone())
                    .header(header::USER_AGENT, USER_AGENT)
                    .timeout(std::time::Duration::from_secs(30))
                    .header(header::COOKIE, cookie_header(&cookies)),
            )
            .await
            .map_err(network)?;
            collect_cookies(&response, &mut cookies);
        }
        if cookies.is_empty() {
            return Err(ProviderError::Protocol(
                "Weibo login completed without cookies".into(),
            ));
        }
        Ok(cookie_header(&cookies))
    }
}

fn classify_poll(
    value: &serde_json::Value,
) -> Result<(QrStatus, String, Option<String>), ProviderError> {
    let retcode = value.get("retcode").and_then(number).unwrap_or_default();
    let message = value
        .get("msg")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let (status, alt) = match retcode {
        20_000_000 => {
            let url = value
                .pointer("/data/url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ProviderError::Protocol(
                        "successful Weibo QR response omitted redirect URL".into(),
                    )
                })?;
            (QrStatus::Success, Some(url.to_owned()))
        }
        50_114_002 => (QrStatus::Scanned, None),
        50_114_004 => (QrStatus::Expired, None),
        _ => (QrStatus::Waiting, None),
    };
    Ok((status, message, alt))
}

fn merge_cookie_headers(values: &[String]) -> String {
    let mut cookies = BTreeMap::new();
    for value in values {
        for pair in value.split(';') {
            if let Some((name, value)) = pair.trim().split_once('=') {
                cookies.insert(name.to_owned(), value.to_owned());
            }
        }
    }
    cookie_header(&cookies)
}

fn collect_cookies(response: &Response, cookies: &mut BTreeMap<String, String>) {
    for value in response.headers().get_all(header::SET_COOKIE) {
        let Ok(value) = value.to_str() else { continue };
        let Some(pair) = value.split(';').next() else {
            continue;
        };
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        if !name.trim().is_empty() {
            cookies.insert(name.trim().to_owned(), value.trim().to_owned());
        }
    }
}

fn cookie_header(cookies: &BTreeMap<String, String>) -> String {
    cookies
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn qr_image_url(endpoint: &Url, returned: &str) -> Result<Url, ProviderError> {
    let returned = Url::parse(returned)
        .map_err(|error| ProviderError::Parse(format!("invalid Weibo QR image URL: {error}")))?;
    let query = returned.query().ok_or_else(|| {
        ProviderError::Protocol("Weibo QR image URL omitted its signed query".into())
    })?;
    let mut image_url = endpoint.clone();
    image_url.set_query(Some(query));
    Ok(image_url)
}

pub(crate) fn jsonp_value(body: &str) -> Result<serde_json::Value, ProviderError> {
    let trimmed = body.trim().trim_end_matches(';');
    let json = if trimmed.starts_with('{') {
        trimmed
    } else {
        let start = trimmed
            .find('(')
            .ok_or_else(|| ProviderError::Parse("invalid JSONP response".into()))?;
        let end = trimmed
            .rfind(')')
            .ok_or_else(|| ProviderError::Parse("invalid JSONP response".into()))?;
        &trimmed[start + 1..end]
    };
    serde_json::from_str(json).map_err(|e| ProviderError::Parse(e.to_string()))
}

fn number(value: &serde_json::Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}

fn network(_: impl std::fmt::Display) -> ProviderError {
    ProviderError::Network("Weibo HTTP request failed".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn qr_image_over_limit_is_rejected_without_signed_data_in_error() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        let server = MockServer::start().await;
        Mock::given(path("/info")).respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"data":{"qrid":"fixture-qr","image":"https://example.invalid/image?sign=fixture-signature"}}"#
        )).mount(&server).await;
        Mock::given(path("/image"))
            .respond_with(
                ResponseTemplate::new(200).set_body_bytes(vec![0u8; 16 * 1024 * 1024 + 1]),
            )
            .mount(&server)
            .await;
        let endpoints = WeiboEndpoints {
            qr_info: Url::parse(&format!("{}/info", server.uri())).unwrap(),
            qr_image: Url::parse(&format!("{}/image", server.uri())).unwrap(),
            ..WeiboEndpoints::default()
        };
        let auth = WeiboAuth::new(Client::new(), endpoints);
        let error = auth
            .begin()
            .await
            .err()
            .expect("oversized QR image must fail")
            .to_string();
        assert!(error.contains("exceeded limit"));
        assert!(!error.contains("fixture-signature"));
        assert!(!error.contains("fixture-qr"));
    }

    #[test]
    fn jsonp_is_accepted_and_malformed_input_is_rejected() {
        let value = jsonp_value(include_str!(
            "../../../tests/fixtures/providers/weibo/qr.jsonp"
        ))
        .unwrap();
        assert_eq!(
            value.pointer("/data/qrid").and_then(|v| v.as_str()),
            Some("qr-fixture")
        );
        assert!(jsonp_value("callback(not-json)").is_err());
    }

    #[test]
    fn qr_image_keeps_the_complete_signed_query_on_the_configured_endpoint() {
        let endpoint = Url::parse("http://127.0.0.1:1234/qr-image").unwrap();
        let image = qr_image_url(
            &endpoint,
            "https://v2.qr.weibo.cn/inf/gen?api_key=key&data=payload&sign=signature&size=180",
        )
        .unwrap();
        assert_eq!(
            image.as_str(),
            "http://127.0.0.1:1234/qr-image?api_key=key&data=payload&sign=signature&size=180"
        );
    }

    #[test]
    fn qr_fixture_status_codes_are_stable() {
        for (fixture, expected) in [
            (
                include_str!("../../../tests/fixtures/providers/weibo/qr_waiting.jsonp"),
                QrStatus::Waiting,
            ),
            (
                include_str!("../../../tests/fixtures/providers/weibo/qr_scanned.jsonp"),
                QrStatus::Scanned,
            ),
            (
                include_str!("../../../tests/fixtures/providers/weibo/qr_expired.jsonp"),
                QrStatus::Expired,
            ),
            (
                include_str!("../../../tests/fixtures/providers/weibo/qr_success.jsonp"),
                QrStatus::Success,
            ),
        ] {
            assert_eq!(
                classify_poll(&jsonp_value(fixture).unwrap()).unwrap().0,
                expected
            );
        }
    }
}
