use std::{sync::Arc, time::Duration};

use reqwest::{Client, cookie::Jar, header::HeaderMap};
use thiserror::Error;

use super::{ProxyUrl, Session, proxy::ProxyError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectPolicy {
    None,
    Limited(usize),
}

#[derive(Clone)]
pub struct ClientOptions {
    pub proxy: Option<ProxyUrl>,
    pub timeout: Duration,
    pub redirect: RedirectPolicy,
    pub headers: HeaderMap,
    pub user_agent: String,
    pub pool_max_idle_per_host: usize,
    pub pool_idle_timeout: Duration,
    /// Explicit bypass list. None reads NO_PROXY/no_proxy at client creation.
    pub no_proxy: Option<String>,
}

impl std::fmt::Debug for ClientOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientOptions")
            .field("proxy", &self.proxy)
            .field("timeout", &self.timeout)
            .field("redirect", &self.redirect)
            .field("pool_max_idle_per_host", &self.pool_max_idle_per_host)
            .field("pool_idle_timeout", &self.pool_idle_timeout)
            .finish_non_exhaustive()
    }
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            proxy: None,
            timeout: Duration::from_secs(30),
            redirect: RedirectPolicy::Limited(10),
            headers: HeaderMap::new(),
            user_agent: concat!("pansou/", env!("CARGO_PKG_VERSION")).into(),
            pool_max_idle_per_host: 110,
            pool_idle_timeout: Duration::from_secs(600),
            no_proxy: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum HttpError {
    #[error(transparent)]
    Proxy(#[from] ProxyError),
    #[error("HTTP timeout must be greater than zero")]
    ZeroTimeout,
    #[error("failed to build HTTP client")]
    Build,
}

impl From<reqwest::Error> for HttpError {
    fn from(_: reqwest::Error) -> Self {
        // Builder errors can contain user-controlled header/proxy values.
        Self::Build
    }
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

    /// A redirect-disabled client using the same transport/security options.
    /// Keep the returned clone for reuse instead of rebuilding per request.
    pub fn no_follow(&self, options: &ClientOptions) -> Result<Client, HttpError> {
        let mut options = options.clone();
        options.redirect = RedirectPolicy::None;
        self.client(&options)
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
            .connect_timeout(Duration::from_secs(30).min(options.timeout))
            .pool_max_idle_per_host(options.pool_max_idle_per_host)
            .pool_idle_timeout(options.pool_idle_timeout)
            .tcp_keepalive(Duration::from_secs(30))
            // read_response must see the encoded body to enforce both limits.
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .redirect(redirect)
            .default_headers(options.headers.clone())
            .user_agent(&options.user_agent);
        let environment_proxy = if options.proxy.is_none() {
            super::proxy::compatibility_proxy(|key| std::env::var(key).ok())
                .map(|value| ProxyUrl::parse(&value))
                .transpose()?
        } else {
            None
        };
        let bypass = options
            .no_proxy
            .clone()
            .or_else(|| super::proxy::no_proxy(|key| std::env::var(key).ok()));
        if let Some(proxy) = options.proxy.as_ref().or(environment_proxy.as_ref()) {
            builder = builder.no_proxy().proxy(
                reqwest::Proxy::all(proxy.as_url().as_str())?
                    .no_proxy(bypass.as_deref().and_then(reqwest::NoProxy::from_string)),
            );
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

    #[tokio::test]
    async fn explicit_proxy_respects_bypass_and_sends_credentials() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{header, method},
        };
        let target = MockServer::start().await;
        let proxy = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("direct"))
            .expect(1)
            .mount(&target)
            .await;
        Mock::given(method("GET"))
            .and(header("proxy-authorization", "Basic YWxpY2U6c2VjcmV0"))
            .respond_with(ResponseTemplate::new(200).set_body_string("proxy"))
            .expect(1)
            .mount(&proxy)
            .await;
        let proxy_url = proxy.uri().replace("http://", "http://alice:secret@");
        let mut options = ClientOptions {
            proxy: Some(ProxyUrl::parse(&proxy_url).unwrap()),
            no_proxy: Some("127.0.0.1".into()),
            ..ClientOptions::default()
        };
        let factory = HttpClientFactory::new();
        let direct = factory
            .client(&options)
            .unwrap()
            .get(target.uri())
            .send()
            .await
            .unwrap();
        assert_eq!(
            crate::http::read_response(direct, 100).await.unwrap(),
            b"direct"
        );
        options.no_proxy = Some(String::new());
        let proxied = factory
            .client(&options)
            .unwrap()
            .get(target.uri())
            .send()
            .await
            .unwrap();
        assert_eq!(
            crate::http::read_response(proxied, 100).await.unwrap(),
            b"proxy"
        );
        assert!(!format!("{options:?}").contains("alice"));
        assert!(!format!("{options:?}").contains("secret"));
    }

    #[tokio::test]
    async fn no_follow_does_not_follow_redirect() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        let server = MockServer::start().await;
        Mock::given(path("/redirect"))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", "/destination"))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/destination"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let client = HttpClientFactory::new()
            .no_follow(&ClientOptions::default())
            .unwrap();
        assert_eq!(
            client
                .get(format!("{}/redirect", server.uri()))
                .send()
                .await
                .unwrap()
                .status(),
            302
        );
    }

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
