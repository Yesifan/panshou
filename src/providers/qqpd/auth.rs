use std::{collections::BTreeMap, path::Path};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::{Client, Response, header};
use url::Url;

use crate::{core::ProviderError, http::Session};

use super::QqpdEndpoints;

const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120.0.0.0 Safari/537.36";

#[derive(Clone)]
pub struct QqQrChallenge {
    pub qrsig: String,
    pub image_png: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QqQrTerminalProtocol {
    Iterm2,
    Kitty,
}

impl QqQrChallenge {
    pub fn terminal_escape(&self, protocol: QqQrTerminalProtocol) -> String {
        let image = STANDARD.encode(&self.image_png);
        match protocol {
            QqQrTerminalProtocol::Iterm2 => {
                format!("\x1b]1337;File=inline=1;preserveAspectRatio=1:{image}\x07")
            }
            QqQrTerminalProtocol::Kitty => format!("\x1b_Gf=100,a=T;{image}\x1b\\"),
        }
    }
    pub fn terminal_escape_from_env(&self) -> Option<String> {
        if std::env::var_os("KITTY_WINDOW_ID").is_some() {
            return Some(self.terminal_escape(QqQrTerminalProtocol::Kitty));
        }
        if std::env::var("TERM_PROGRAM").is_ok_and(|v| v == "iTerm.app") {
            return Some(self.terminal_escape(QqQrTerminalProtocol::Iterm2));
        }
        None
    }
    pub fn write_png(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        std::fs::write(path, &self.image_png)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QqQrStatus {
    Waiting,
    Scanned,
    Expired,
    Success,
}

#[derive(Clone)]
pub struct QqQrPoll {
    pub status: QqQrStatus,
    pub cookie: Option<String>,
    pub qq_masked: Option<String>,
}

#[derive(Debug, Clone)]
pub struct QqpdAuth {
    client: Client,
    session: Option<Session>,
    endpoints: QqpdEndpoints,
}

impl QqpdAuth {
    pub fn new(client: Client, endpoints: QqpdEndpoints) -> Self {
        Self {
            client,
            session: None,
            endpoints,
        }
    }
    pub fn with_session(session: Session, endpoints: QqpdEndpoints) -> Self {
        Self {
            client: session.client().clone(),
            session: Some(session),
            endpoints,
        }
    }

    pub async fn begin(&self) -> Result<QqQrChallenge, ProviderError> {
        let cache_buster = format!("0.{}", chrono::Utc::now().timestamp_millis());
        let response = self
            .client
            .get(self.endpoints.qr_show.clone())
            .query(&[
                ("appid", "1600001587"),
                ("e", "2"),
                ("l", "M"),
                ("s", "3"),
                ("d", "72"),
                ("v", "4"),
                ("t", cache_buster.as_str()),
                ("daid", "823"),
                ("pt_3rd_aid", "0"),
            ])
            .header(header::USER_AGENT, USER_AGENT)
            .send()
            .await
            .map_err(network)?
            .error_for_status()
            .map_err(network)?;
        let qrsig = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(extract_qrsig)
            .ok_or_else(|| ProviderError::Protocol("QQ QR response omitted qrsig".into()))?;
        let image_png = response.bytes().await.map_err(network)?.to_vec();
        if image_png.is_empty() {
            return Err(ProviderError::Protocol(
                "QQ returned an empty QR image".into(),
            ));
        }
        Ok(QqQrChallenge { qrsig, image_png })
    }

    pub async fn poll(&self, qrsig: &str) -> Result<QqQrPoll, ProviderError> {
        let token = ptqrtoken(qrsig).to_string();
        let action = format!("0-0-{}", chrono::Utc::now().timestamp_millis());
        let response = self
            .client
            .get(self.endpoints.qr_login.clone())
            .query(&[
                ("u1", self.endpoints.explore.as_str()),
                ("ptqrtoken", token.as_str()),
                ("ptredirect", "1"),
                ("h", "1"),
                ("t", "1"),
                ("g", "1"),
                ("from_ui", "1"),
                ("ptlang", "2052"),
                ("action", action.as_str()),
                ("js_ver", "25100115"),
                ("js_type", "1"),
                ("pt_uistyle", "40"),
                ("aid", "1600001587"),
                ("daid", "823"),
                ("o1vId", "11f3315cde61b7b5da200e4a09fe308c"),
                ("pt_js_version", "28d22679"),
            ])
            .header(header::COOKIE, format!("qrsig={qrsig}"))
            .header(header::USER_AGENT, USER_AGENT)
            .send()
            .await
            .map_err(network)?
            .error_for_status()
            .map_err(network)?;
        let set_cookies = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let body = response.text().await.map_err(network)?;
        if body.contains("二维码已失效") {
            return Ok(QqQrPoll {
                status: QqQrStatus::Expired,
                cookie: None,
                qq_masked: None,
            });
        }
        if body.contains("二维码认证中") || body.contains("已扫描") {
            return Ok(QqQrPoll {
                status: QqQrStatus::Scanned,
                cookie: None,
                qq_masked: None,
            });
        }
        let Some((redirect, uin, ptsigx)) = parse_success(&body)? else {
            return Ok(QqQrPoll {
                status: QqQrStatus::Waiting,
                cookie: None,
                qq_masked: None,
            });
        };
        let cookie = self
            .finish_login(&redirect, &uin, &ptsigx, &set_cookies)
            .await?;
        Ok(QqQrPoll {
            status: QqQrStatus::Success,
            cookie: Some(cookie),
            qq_masked: Some(mask_qq(&uin)),
        })
    }

    async fn finish_login(
        &self,
        redirect: &Url,
        uin: &str,
        ptsigx: &str,
        initial: &[String],
    ) -> Result<String, ProviderError> {
        if let Some(session) = &self.session {
            for cookie in initial {
                session.add_cookie(cookie, &self.endpoints.qq_login_origin);
            }
            session
                .client()
                .get(redirect.clone())
                .header(header::USER_AGENT, USER_AGENT)
                .send()
                .await
                .map_err(network)?
                .error_for_status()
                .map_err(network)?;
            session
                .client()
                .get(self.endpoints.check_sig.clone())
                .query(&[
                    ("pttype", "1"),
                    ("uin", uin),
                    ("service", "ptqrlogin"),
                    ("nodirect", "1"),
                    ("ptsigx", ptsigx),
                    ("s_url", self.endpoints.explore.as_str()),
                    ("aid", "1600001587"),
                    ("daid", "823"),
                ])
                .send()
                .await
                .map_err(network)?
                .error_for_status()
                .map_err(network)?;
            let mut cookies = BTreeMap::new();
            for origin in [&self.endpoints.pd_origin, &self.endpoints.qq_login_origin] {
                if let Some(value) = session.cookie_header(origin) {
                    for pair in value.split(';') {
                        collect_cookie(pair, &mut cookies);
                    }
                }
            }
            let cookie = cookie_header(&cookies);
            if cookie.is_empty() {
                return Err(ProviderError::Protocol(
                    "QQ login completed without cookies".into(),
                ));
            }
            return Ok(cookie);
        }
        let mut cookies = BTreeMap::new();
        for value in initial {
            collect_cookie(value, &mut cookies);
        }
        let response = self
            .client
            .get(self.endpoints.check_sig.clone())
            .query(&[
                ("pttype", "1"),
                ("uin", uin),
                ("service", "ptqrlogin"),
                ("nodirect", "1"),
                ("ptsigx", ptsigx),
                ("s_url", self.endpoints.explore.as_str()),
                ("aid", "1600001587"),
                ("daid", "823"),
            ])
            .header(header::COOKIE, cookie_header(&cookies))
            .send()
            .await
            .map_err(network)?
            .error_for_status()
            .map_err(network)?;
        collect_response_cookies(&response, &mut cookies);
        cookies
            .entry("uin".into())
            .or_insert_with(|| format!("o0{uin}"));
        let cookie = cookie_header(&cookies);
        if cookie.is_empty() {
            return Err(ProviderError::Protocol(
                "QQ login completed without cookies".into(),
            ));
        }
        Ok(cookie)
    }
}

fn parse_success(body: &str) -> Result<Option<(Url, String, String)>, ProviderError> {
    if !body.contains("登录成功") && !body.starts_with("ptuiCB('0','0'") {
        return Ok(None);
    }
    let marker = "ptuiCB('0','0','";
    let start = body
        .find(marker)
        .ok_or_else(|| ProviderError::Protocol("invalid QQ login callback".into()))?
        + marker.len();
    let end = body[start..]
        .find('\'')
        .ok_or_else(|| ProviderError::Protocol("invalid QQ login callback URL".into()))?
        + start;
    let url = Url::parse(&body[start..end]).map_err(|e| ProviderError::Protocol(e.to_string()))?;
    let uin = url
        .query_pairs()
        .find(|(k, _)| k == "uin")
        .map(|(_, v)| v.into_owned())
        .ok_or_else(|| ProviderError::Protocol("QQ login callback omitted uin".into()))?;
    let ptsigx = url
        .query_pairs()
        .find(|(k, _)| k == "ptsigx")
        .map(|(_, v)| v.into_owned())
        .ok_or_else(|| ProviderError::Protocol("QQ login callback omitted ptsigx".into()))?;
    Ok(Some((url, uin, ptsigx)))
}

fn extract_qrsig(value: &str) -> Option<String> {
    value
        .split(';')
        .find_map(|part| part.trim().strip_prefix("qrsig=").map(str::to_owned))
}

pub fn ptqrtoken(qrsig: &str) -> i64 {
    let mut value = 0i64;
    for byte in qrsig.bytes() {
        value = value
            .wrapping_add(value.wrapping_shl(5))
            .wrapping_add(i64::from(byte));
    }
    value & 0x7fff_ffff
}

pub fn mask_qq(uin: &str) -> String {
    let chars = uin.chars().collect::<Vec<_>>();
    match chars.len() {
        0..=4 => uin.to_owned(),
        5..=6 => format!(
            "{}****{}",
            chars[..2].iter().collect::<String>(),
            chars[chars.len() - 2..].iter().collect::<String>()
        ),
        _ => format!(
            "{}****{}",
            chars[..4].iter().collect::<String>(),
            chars[chars.len() - 2..].iter().collect::<String>()
        ),
    }
}

fn collect_response_cookies(response: &Response, output: &mut BTreeMap<String, String>) {
    for value in response.headers().get_all(header::SET_COOKIE) {
        if let Ok(value) = value.to_str() {
            collect_cookie(value, output);
        }
    }
}
fn collect_cookie(value: &str, output: &mut BTreeMap<String, String>) {
    if let Some((name, value)) = value.split(';').next().and_then(|v| v.split_once('='))
        && !name.trim().is_empty()
    {
        output.insert(name.trim().into(), value.trim().into());
    }
}
fn cookie_header(values: &BTreeMap<String, String>) -> String {
    values
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ")
}
fn network(error: reqwest::Error) -> ProviderError {
    ProviderError::Network(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn token_and_mask_match_oracle() {
        assert_eq!(ptqrtoken("abc"), 108966);
        assert_eq!(mask_qq("123456789"), "1234****89");
    }
    #[test]
    fn parses_success_callback() {
        let body = include_str!("../../../tests/fixtures/providers/qqpd/login_success.txt");
        let (_, uin, sig) = parse_success(body).unwrap().unwrap();
        assert_eq!(uin, "123456789");
        assert_eq!(sig, "abcXYZ123");
    }
    #[test]
    fn extracts_qrsig_cookie() {
        assert_eq!(
            extract_qrsig("qrsig=secret; Path=/; HttpOnly").as_deref(),
            Some("secret")
        );
    }

    #[test]
    fn waiting_and_expired_fixtures_are_distinct() {
        let waiting = include_str!("../../../tests/fixtures/providers/qqpd/login_waiting.txt");
        let expired = include_str!("../../../tests/fixtures/providers/qqpd/login_expired.txt");
        assert!(parse_success(waiting).unwrap().is_none());
        assert!(!waiting.contains("二维码已失效"));
        assert!(expired.contains("二维码已失效"));
    }
}
