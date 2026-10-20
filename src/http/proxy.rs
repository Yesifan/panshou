use std::{fmt, str::FromStr};

use thiserror::Error;
use url::Url;

#[derive(Clone, PartialEq, Eq)]
pub struct ProxyUrl(Url);

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("invalid proxy URL: {0}")]
    Invalid(#[from] url::ParseError),
    #[error("unsupported proxy scheme; expected http, https, socks5, or socks5h")]
    UnsupportedScheme,
    #[error("proxy URL must include a host")]
    MissingHost,
}

impl ProxyUrl {
    pub fn parse(value: &str) -> Result<Self, ProxyError> {
        let url = Url::parse(value)?;
        match url.scheme() {
            "http" | "https" | "socks5" | "socks5h" => {}
            _ => return Err(ProxyError::UnsupportedScheme),
        }
        if url.host_str().is_none() {
            return Err(ProxyError::MissingHost);
        }
        Ok(Self(url))
    }

    pub fn as_url(&self) -> &Url {
        &self.0
    }

    /// Safe diagnostic form: proxy credentials are never exposed.
    pub fn redacted(&self) -> String {
        let mut url = self.0.clone();
        let _ = url.set_username("");
        let _ = url.set_password(None);
        url.to_string()
    }
}

impl fmt::Debug for ProxyUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ProxyUrl")
            .field(&self.redacted())
            .finish()
    }
}

/// Compatibility proxy fallback, after CLI/PANSOU_* overrides. Empty values
/// are ignored. Accepting a lookup keeps precedence tests environment-safe.
pub fn compatibility_proxy(mut lookup: impl FnMut(&str) -> Option<String>) -> Option<String> {
    [
        "PROXY",
        "proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ]
    .into_iter()
    .find_map(|key| {
        lookup(key)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
}

pub fn no_proxy(mut lookup: impl FnMut(&str) -> Option<String>) -> Option<String> {
    ["NO_PROXY", "no_proxy"].into_iter().find_map(|key| {
        lookup(key)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
}

impl fmt::Display for ProxyUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.redacted())
    }
}

impl FromStr for ProxyUrl {
    type Err = ProxyError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_all_documented_schemes() {
        for scheme in ["http", "https", "socks5", "socks5h"] {
            assert!(ProxyUrl::parse(&format!("{scheme}://127.0.0.1:1080")).is_ok());
        }
    }

    #[test]
    fn invalid_urls_never_echo_input() {
        for value in [
            "ftp://alice:secret@localhost",
            "http://alice:secret@",
            "alice:secret@localhost",
            "http://alice:secret@[invalid",
        ] {
            let error = ProxyUrl::parse(value).unwrap_err();
            assert!(!format!("{error:?} {error}").contains("alice"));
            assert!(!format!("{error:?} {error}").contains("secret"));
        }
    }

    #[test]
    fn compatibility_precedence_and_casing() {
        for key in [
            "PROXY",
            "proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            assert_eq!(
                compatibility_proxy(
                    |name| (name == key).then(|| "  http://localhost:8080  ".into())
                ),
                Some("http://localhost:8080".into())
            );
        }
        assert_eq!(
            compatibility_proxy(|name| match name {
                "PROXY" => Some(" ".into()),
                "https_proxy" => Some("first".into()),
                "ALL_PROXY" => Some("second".into()),
                _ => None,
            }),
            Some("first".into())
        );
        assert_eq!(
            no_proxy(|name| Some(
                if name == "NO_PROXY" {
                    " upper "
                } else {
                    "lower"
                }
                .into()
            )),
            Some("upper".into())
        );
    }

    #[test]
    fn display_hides_credentials() {
        let proxy = ProxyUrl::parse("http://alice:secret@localhost:8080").unwrap();
        assert!(!proxy.to_string().contains("secret"));
        assert!(!proxy.to_string().contains("alice"));
        assert!(!format!("{proxy:?}").contains("secret"));
        assert!(!format!("{proxy:?}").contains("alice"));
    }
}
