use std::sync::Arc;

use reqwest::{
    Client,
    cookie::{CookieStore, Jar},
};
use url::Url;

/// An HTTP client and its profile-local cookie jar.
#[derive(Clone)]
pub struct Session {
    client: Client,
    jar: Arc<Jar>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately omit the jar: Debug output must never leak cookies.
        formatter.debug_struct("Session").finish_non_exhaustive()
    }
}

impl Session {
    pub(crate) fn new(client: Client, jar: Arc<Jar>) -> Self {
        Self { client, jar }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Restore a Set-Cookie style string previously persisted by a provider.
    pub fn add_cookie(&self, cookie: &str, origin: &Url) {
        self.jar.add_cookie_str(cookie, origin);
    }

    /// Returns a Cookie request-header value suitable for profile persistence.
    pub fn cookie_header(&self, url: &Url) -> Option<String> {
        self.jar
            .cookies(url)
            .and_then(|value| value.to_str().ok().map(str::to_owned))
    }
}
