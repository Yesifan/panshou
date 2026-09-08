use std::{fmt, str::FromStr};

use thiserror::Error;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyUrl(Url);

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("invalid proxy URL: {0}")]
    Invalid(#[from] url::ParseError),
    #[error("unsupported proxy scheme {0:?}; expected http, https, socks5, or socks5h")]
    UnsupportedScheme(String),
    #[error("proxy URL must include a host")]
    MissingHost,
}

impl ProxyUrl {
    pub fn parse(value: &str) -> Result<Self, ProxyError> {
        let url = Url::parse(value)?;
        match url.scheme() {
            "http" | "https" | "socks5" | "socks5h" => {}
            other => return Err(ProxyError::UnsupportedScheme(other.to_owned())),
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
    fn display_hides_credentials() {
        let proxy = ProxyUrl::parse("http://alice:secret@localhost:8080").unwrap();
        assert!(!proxy.to_string().contains("secret"));
        assert!(!proxy.to_string().contains("alice"));
    }
}
