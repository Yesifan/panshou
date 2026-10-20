use super::*;
use crate::core::ProviderError;
use chrono::Utc;
use serde::Deserialize;
use std::time::Duration;
impl WoniuProvider {
    /// The caller obtains the password from a TTY or stdin. It is never persisted.
    pub async fn login(
        &self,
        profile: &str,
        username: &str,
        password: &str,
    ) -> Result<WoniuProfile, ProviderError> {
        if username.trim().is_empty() || password.is_empty() {
            return Err(ProviderError::Protocol(
                "username and password are required".into(),
            ));
        }
        let session = self.session("")?;
        let url = self.endpoints.base.join("/user/login.html").unwrap();
        let response = crate::http::execute(
            headers(session.client().post(url), &self.endpoints.base)
                .timeout(Duration::from_secs(30).min(self.options.timeout))
                .header(
                    "content-type",
                    "application/x-www-form-urlencoded; charset=UTF-8",
                )
                .header("x-requested-with", "XMLHttpRequest")
                .form(&[("user_name", username.trim()), ("user_pwd", password)]),
        )
        .await
        .map_err(super::super::c_support::network)?;
        if response.status() != 200 {
            return Err(ProviderError::Protocol(format!(
                "HTTP {}",
                response.status().as_u16()
            )));
        }
        let body = crate::http::read_response(response, 16 * 1024 * 1024)
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        #[derive(Deserialize)]
        struct Login {
            code: i32,
            #[serde(default)]
            msg: String,
        }
        let result: Login =
            serde_json::from_slice(&body).map_err(|e| ProviderError::Parse(e.to_string()))?;
        if result.code != 1 {
            // Never reflect a server's credentials/HTML into diagnostics.
            let safe = scraper::Html::parse_fragment(&result.msg)
                .root_element()
                .text()
                .collect::<String>();
            let safe = safe
                .replace(password, "[redacted]")
                .replace(username, "[redacted]")
                .chars()
                .filter(|c| !c.is_control())
                .take(160)
                .collect::<String>();
            return Err(ProviderError::Protocol(format!(
                "woniu login rejected: {safe}"
            )));
        }
        let cookie = session
            .cookie_header(&self.endpoints.base)
            .filter(|c| !c.is_empty())
            .ok_or_else(|| {
                ProviderError::Protocol("woniu login succeeded without cookies".into())
            })?;
        let now = Utc::now();
        let p = WoniuProfile {
            username: username.trim().into(),
            cookie,
            base_url: self.endpoints.base.as_str().trim_end_matches('/').into(),
            status: "active".into(),
            expires_at: now + chrono::Duration::days(30),
            last_access_at: now,
            last_check_at: now,
        };
        self.save(profile, p.clone())?;
        Ok(p)
    }
    pub async fn verify(&self, profile: &str) -> Result<bool, ProviderError> {
        let p = self
            .profiles
            .read()
            .map_err(lock)?
            .iter()
            .find(|(n, _)| n == profile)
            .map(|(_, p)| p.clone())
            .ok_or(ProviderError::AuthRequired)?;
        self.verify_profile(
            profile,
            p,
            Duration::from_secs(20).min(self.options.timeout),
        )
        .await
    }
    pub(crate) async fn verify_profile(
        &self,
        name: &str,
        mut p: WoniuProfile,
        timeout: Duration,
    ) -> Result<bool, ProviderError> {
        let session = self.session(&p.cookie)?;
        let result = get(
            &session,
            &self.endpoints.base,
            self.endpoints.base.join("/user/index.html").unwrap(),
            timeout,
            64 * 1024,
        )
        .await;
        let valid = match result {
            Ok(_) => true,
            Err(ProviderError::AuthRequired) => false,
            Err(e) => return Err(e),
        };
        p.last_check_at = Utc::now();
        if valid {
            p.cookie = session
                .cookie_header(&self.endpoints.base)
                .unwrap_or_default();
        }
        if !valid {
            p.status = "expired".into();
            p.cookie.clear()
        }
        self.save(name, p)?;
        Ok(valid)
    }
    /// Notify the website, but always delete local state even if it is unavailable.
    pub async fn logout(&self, profile: &str) -> Result<bool, ProviderError> {
        let p = self
            .profiles
            .read()
            .map_err(lock)?
            .iter()
            .find(|(n, _)| n == profile)
            .map(|(_, p)| p.clone());
        if let Some(p) = p
            && let Ok(session) = self.session(&p.cookie)
        {
            let _ = get(
                &session,
                &self.endpoints.base,
                self.endpoints.base.join("/user/logout.html").unwrap(),
                Duration::from_secs(20).min(self.options.timeout),
                64 * 1024,
            )
            .await;
        }
        let deleted = self.store.delete("woniu", profile).map_err(state)?;
        self.profiles
            .write()
            .map_err(lock)?
            .retain(|(n, _)| n != profile);
        Ok(deleted)
    }
}
