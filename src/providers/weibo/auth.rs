use std::{collections::BTreeMap, path::Path};

use base64::{Engine as _, engine::general_purpose::STANDARD};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QrTerminalProtocol {
    Iterm2,
    Kitty,
}

impl QrChallenge {
    /// Render the QR bitmap with a terminal's native inline-image protocol.
    pub fn terminal_escape(&self, protocol: QrTerminalProtocol) -> String {
        let image = STANDARD.encode(&self.image_png);
        match protocol {
            QrTerminalProtocol::Iterm2 => {
                format!("\x1b]1337;File=inline=1;preserveAspectRatio=1:{image}\x07")
            }
            QrTerminalProtocol::Kitty => format!("\x1b_Gf=100,a=T;{image}\x1b\\"),
        }
    }

    pub fn terminal_escape_from_env(&self) -> Option<String> {
        if std::env::var_os("KITTY_WINDOW_ID").is_some() {
            return Some(self.terminal_escape(QrTerminalProtocol::Kitty));
        }
        if std::env::var("TERM_PROGRAM").is_ok_and(|v| v == "iTerm.app") {
            return Some(self.terminal_escape(QrTerminalProtocol::Iterm2));
        }
        None
    }

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
        let response = self
            .client
            .get(self.endpoints.qr_info.clone())
            .query(&[
                ("entry", "miniblog"),
                ("size", "180"),
                ("callback", callback.as_str()),
            ])
            .header(header::USER_AGENT, USER_AGENT)
            .header(header::REFERER, self.endpoints.web_origin.as_str())
            .send()
            .await
            .map_err(network)?
            .error_for_status()
            .map_err(network)?;
        let body = response.text().await.map_err(network)?;
        let value = jsonp_value(&body)?;
        let qrid = value
            .pointer("/data/qrid")
            .and_then(|v| v.as_str())
            .or_else(|| value.get("qrid").and_then(|v| v.as_str()))
            .ok_or_else(|| ProviderError::Protocol("Weibo QR response omitted qrid".into()))?;
        let api_key = value
            .pointer("/data/image")
            .and_then(|v| v.as_str())
            .and_then(api_key_from_image)
            .or_else(|| find_api_key(&body).map(str::to_owned))
            .ok_or_else(|| ProviderError::Protocol("Weibo QR response omitted api_key".into()))?;
        let image_png = self
            .client
            .get(self.endpoints.qr_image.clone())
            .query(&[("api_key", api_key.as_str())])
            .header(header::USER_AGENT, USER_AGENT)
            .header(header::REFERER, self.endpoints.web_origin.as_str())
            .send()
            .await
            .map_err(network)?
            .error_for_status()
            .map_err(network)?
            .bytes()
            .await
            .map_err(network)?
            .to_vec();
        if image_png.is_empty() {
            return Err(ProviderError::Protocol(
                "Weibo returned an empty QR image".into(),
            ));
        }
        Ok(QrChallenge {
            qrid: qrid.to_owned(),
            image_png,
        })
    }

    pub async fn poll(&self, qrid: &str) -> Result<QrPoll, ProviderError> {
        let callback = format!("STK_{}", chrono::Utc::now().timestamp_millis());
        let response = self
            .client
            .get(self.endpoints.qr_check.clone())
            .query(&[
                ("entry", "sso"),
                ("qrid", qrid),
                ("callback", callback.as_str()),
            ])
            .header(header::USER_AGENT, USER_AGENT)
            .header(header::REFERER, self.endpoints.web_origin.as_str())
            .send()
            .await
            .map_err(network)?
            .error_for_status()
            .map_err(network)?;
        let value = jsonp_value(&response.text().await.map_err(network)?)?;
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
            session
                .client()
                .get(start)
                .header(header::USER_AGENT, USER_AGENT)
                .send()
                .await
                .map_err(network)?
                .error_for_status()
                .map_err(network)?;
            for url in [&self.endpoints.web_origin, &self.endpoints.mobile_origin] {
                session
                    .client()
                    .get(url.clone())
                    .header(header::USER_AGENT, USER_AGENT)
                    .send()
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
            let response = self
                .client
                .get(next.clone())
                .header(header::USER_AGENT, USER_AGENT)
                .send()
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
            let response = self
                .client
                .get(url.clone())
                .header(header::USER_AGENT, USER_AGENT)
                .header(header::COOKIE, cookie_header(&cookies))
                .send()
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

fn api_key_from_image(image: &str) -> Option<String> {
    Url::parse(image)
        .ok()?
        .query_pairs()
        .find(|(key, _)| key == "api_key")
        .map(|(_, value)| value.into_owned())
}

fn find_api_key(body: &str) -> Option<&str> {
    let start = body.find("api_key=")? + "api_key=".len();
    let rest = &body[start..];
    let end = rest.find(['"', '&', '\\']).unwrap_or(rest.len());
    (!rest[..end].is_empty()).then_some(&rest[..end])
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

fn network(error: reqwest::Error) -> ProviderError {
    ProviderError::Network(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            find_api_key(include_str!(
                "../../../tests/fixtures/providers/weibo/qr.jsonp"
            )),
            Some("fixture-key")
        );
        assert!(jsonp_value("callback(not-json)").is_err());
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
