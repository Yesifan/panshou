//! Woniu's authenticated MacCMS search protocol. No credentials are retained.
use crate::{
    core::ProviderError,
    http::{ClientOptions, HttpClientFactory, RedirectPolicy, Session},
    state::{StateError, StateStore},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};
use url::Url;
pub mod auth;
pub mod search;
#[cfg(test)]
mod tests;

pub const DEFAULT_BASE: &str = "https://wn4k.com";
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse(DEFAULT_BASE).unwrap(),
        }
    }
}
impl Endpoints {
    pub fn parse(raw: &str) -> Result<Self, ProviderError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(ProviderError::Protocol("woniu base URL is required".into()));
        }
        let raw = if raw.contains("://") {
            raw.to_owned()
        } else {
            format!("https://{raw}")
        };
        let base = Url::parse(&raw)
            .map_err(|_| ProviderError::Protocol("invalid woniu base URL".into()))?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(ProviderError::Protocol("invalid woniu base URL".into()));
        }
        Ok(Self { base })
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct WoniuProfile {
    pub username: String,
    pub cookie: String,
    pub base_url: String,
    pub status: String,
    pub expires_at: DateTime<Utc>,
    pub last_access_at: DateTime<Utc>,
    pub last_check_at: DateTime<Utc>,
}
impl WoniuProfile {
    pub fn is_ready(&self, now: DateTime<Utc>) -> bool {
        self.status == "active" && !self.cookie.is_empty() && self.expires_at > now
    }
}
impl std::fmt::Debug for WoniuProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WoniuProfile")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone)]
pub struct WoniuProvider {
    pub(crate) endpoints: Endpoints,
    pub(crate) options: ClientOptions,
    pub(crate) store: StateStore,
    pub(crate) profiles: Arc<RwLock<Vec<(String, WoniuProfile)>>>,
}
impl WoniuProvider {
    pub fn load(
        store: StateStore,
        endpoints: Endpoints,
        _factory: HttpClientFactory,
        options: ClientOptions,
    ) -> Result<Self, StateError> {
        let mut profiles = vec![];
        for name in store.list_profiles("woniu")? {
            if let Some(mut p) = store.load::<WoniuProfile>("woniu", &name)? {
                if p.base_url.trim_end_matches('/') != endpoints.base.as_str().trim_end_matches('/')
                    || p.expires_at <= Utc::now()
                {
                    p.status = "expired".into();
                    p.cookie.clear();
                    store.save("woniu", &name, &p)?;
                }
                profiles.push((name, p));
            }
        }
        Ok(Self {
            store,
            endpoints,
            options,
            profiles: Arc::new(RwLock::new(profiles)),
        })
    }
    pub fn profile_names(&self) -> Result<Vec<String>, ProviderError> {
        Ok(self
            .profiles
            .read()
            .map_err(lock)?
            .iter()
            .filter(|(_, p)| {
                p.status == "active" && !p.cookie.is_empty() && p.expires_at > Utc::now()
            })
            .map(|(n, _)| n.clone())
            .collect())
    }
    pub(crate) fn session(&self, cookie: &str) -> Result<Session, ProviderError> {
        let mut options = self.options.clone();
        options.redirect = RedirectPolicy::None;
        let session = HttpClientFactory::new()
            .session(&options)
            .map_err(|_| ProviderError::Unavailable("cannot create woniu HTTP session".into()))?;
        for part in cookie.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            session.add_cookie(&format!("{part}; Path=/"), &self.endpoints.base)
        }
        Ok(session)
    }
    pub(crate) fn save(&self, name: &str, p: WoniuProfile) -> Result<(), ProviderError> {
        self.store.save("woniu", name, &p).map_err(state)?;
        let mut all = self.profiles.write().map_err(lock)?;
        all.retain(|(n, _)| n != name);
        all.push((name.into(), p));
        Ok(())
    }
    pub(crate) fn selected(&self) -> Result<(String, WoniuProfile), ProviderError> {
        self.profiles
            .read()
            .map_err(lock)?
            .iter()
            .filter(|(_, p)| {
                p.status == "active" && !p.cookie.is_empty() && p.expires_at > Utc::now()
            })
            .max_by_key(|(_, p)| p.last_access_at)
            .cloned()
            .ok_or(ProviderError::AuthRequired)
    }
    /// Call after a real configuration change, before persisting the new base URL.
    pub fn expire_all(&self) -> Result<(), ProviderError> {
        let all = self.profiles.read().map_err(lock)?.clone();
        for (name, mut p) in all {
            p.status = "expired".into();
            p.cookie.clear();
            self.save(&name, p)?
        }
        Ok(())
    }
}
pub(crate) fn state(e: StateError) -> ProviderError {
    ProviderError::Unavailable(e.to_string())
}
fn lock<T>(_: T) -> ProviderError {
    ProviderError::Unavailable("woniu profile lock poisoned".into())
}
pub(crate) fn headers(r: reqwest::RequestBuilder, base: &Url) -> reqwest::RequestBuilder {
    r.header("user-agent", "Mozilla/5.0 Chrome/122.0.0.0 Safari/537.36")
        .header(
            "accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
        )
        .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("cache-control", "no-cache")
        .header("pragma", "no-cache")
        .header("referer", base.as_str())
}
/// Only same-origin redirects are followed: a hostile Location cannot receive
/// this profile's cookies, even on a sibling subdomain.
pub(crate) async fn get(
    session: &Session,
    base: &Url,
    mut url: Url,
    timeout: std::time::Duration,
    limit: usize,
) -> Result<(Url, Vec<u8>), ProviderError> {
    let deadline = tokio::time::Instant::now() + timeout;
    for _ in 0..=5 {
        if url.origin() != base.origin() {
            return Err(ProviderError::Protocol(
                "woniu cross-origin redirect rejected".into(),
            ));
        }
        let response = crate::http::execute(headers(
            session
                .client()
                .get(url.clone())
                .timeout(deadline.saturating_duration_since(tokio::time::Instant::now())),
            base,
        ))
        .await
        .map_err(super::c_support::network)?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get("location")
                .and_then(|x| x.to_str().ok())
                .ok_or_else(|| ProviderError::Protocol("woniu redirect without Location".into()))?;
            url = url
                .join(location)
                .map_err(|_| ProviderError::Protocol("invalid woniu redirect".into()))?;
            if url.path().contains("/user/login") {
                return Err(ProviderError::AuthRequired);
            }
            continue;
        }
        if response.status() != 200 {
            return Err(ProviderError::Protocol(format!(
                "HTTP {}",
                response.status().as_u16()
            )));
        }
        let body = crate::http::read_response(response, limit)
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        return Ok((url, body));
    }
    Err(ProviderError::Protocol("too many woniu redirects".into()))
}
