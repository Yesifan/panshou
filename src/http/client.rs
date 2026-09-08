use std::{sync::Arc, time::Duration};

use reqwest::{Client, cookie::Jar, header::HeaderMap};
use thiserror::Error;

use super::{ProxyUrl, Session, proxy::ProxyError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectPolicy {
    None,
    Limited(usize),
}

#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub proxy: Option<ProxyUrl>,
    pub timeout: Duration,
    pub redirect: RedirectPolicy,
    pub headers: HeaderMap,
    pub user_agent: String,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            proxy: None,
            timeout: Duration::from_secs(30),
            redirect: RedirectPolicy::Limited(10),
            headers: HeaderMap::new(),
            user_agent: concat!("pansou/", env!("CARGO_PKG_VERSION")).into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum HttpError {
    #[error(transparent)]
    Proxy(#[from] ProxyError),
    #[error("HTTP timeout must be greater than zero")]
    ZeroTimeout,
    #[error("failed to build HTTP client: {0}")]
    Build(#[from] reqwest::Error),
}

#[derive(Debug, Clone, Default)]
pub struct HttpClientFactory;

impl HttpClientFactory {
    pub fn new() -> Self {
        Self
    }

    /// A stateless client. Callers may share this between stateless providers.
    pub fn client(&self, options: &ClientOptions) -> Result<Client, HttpError> {
        self.build(options, None)
    }

    /// A stateful client with a cookie jar dedicated to this session/profile.
    pub fn session(&self, options: &ClientOptions) -> Result<Session, HttpError> {
        let jar = Arc::new(Jar::default());
        let client = self.build(options, Some(jar.clone()))?;
        Ok(Session::new(client, jar))
    }

    fn build(&self, options: &ClientOptions, jar: Option<Arc<Jar>>) -> Result<Client, HttpError> {
        if options.timeout.is_zero() {
            return Err(HttpError::ZeroTimeout);
        }
        let redirect = match options.redirect {
            RedirectPolicy::None => reqwest::redirect::Policy::none(),
            RedirectPolicy::Limited(max) => reqwest::redirect::Policy::limited(max),
        };
        let mut builder = Client::builder()
            .use_rustls_tls()
            .timeout(options.timeout)
            .redirect(redirect)
            .default_headers(options.headers.clone())
            .user_agent(&options.user_agent);
        if let Some(proxy) = &options.proxy {
            builder = builder.proxy(reqwest::Proxy::all(proxy.as_url().as_str())?);
        }
        if let Some(jar) = jar {
            builder = builder.cookie_provider(jar);
        }
        Ok(builder.build()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_timeout() {
        let options = ClientOptions {
            timeout: Duration::ZERO,
            ..ClientOptions::default()
        };
        assert!(matches!(
            HttpClientFactory::new().client(&options),
            Err(HttpError::ZeroTimeout)
        ));
    }

    #[test]
    fn sessions_have_distinct_cookie_jars() {
        let factory = HttpClientFactory::new();
        let one = factory.session(&ClientOptions::default()).unwrap();
        let two = factory.session(&ClientOptions::default()).unwrap();
        one.add_cookie("sid=one; Path=/", &"https://example.com/".parse().unwrap());
        assert_eq!(
            one.cookie_header(&"https://example.com/".parse().unwrap())
                .as_deref(),
            Some("sid=one")
        );
        assert_eq!(
            two.cookie_header(&"https://example.com/".parse().unwrap()),
            None
        );
    }
}
