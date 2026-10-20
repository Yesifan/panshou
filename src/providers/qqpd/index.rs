//! Command-scoped, serialized profile repair; no background warmup.
use super::*;

impl QqpdProvider {
    pub(super) fn load_clean(&self, name: &str) -> Result<Option<QqpdProfile>, ProviderError> {
        let Some(mut state) = self
            .store
            .load::<QqpdProfile>("qqpd", name)
            .map_err(state_error)?
        else {
            return Ok(None);
        };
        let old = state.channel_guild_ids.len();
        let channel_count = state.channels.len();
        let mut seen = HashSet::new();
        state
            .channels
            .retain(|channel| seen.insert(channel.clone()));
        state
            .channel_guild_ids
            .retain(|channel, guild| state.channels.contains(channel) && valid_guild_id(guild));
        if state.channel_guild_ids.len() != old || state.channels.len() != channel_count {
            self.store.save("qqpd", name, &state).map_err(state_error)?;
        }
        Ok(Some(state))
    }
    /// Warnings from channel configuration/repair, consumable by CLI integration.
    pub fn take_diagnostics(&self) -> Vec<String> {
        std::mem::take(&mut *self.diagnostics.lock().expect("diagnostic lock"))
    }
    fn warn(&self, message: String) {
        self.diagnostics
            .lock()
            .expect("diagnostic lock")
            .push(message);
    }
    pub(super) fn persist_resolution(
        &self,
        name: &str,
        cookie: &str,
        channel: String,
        outcome: Result<String, ResolveError>,
    ) -> Result<(), ProviderError> {
        // Reload under one lock: never overwrite sibling writes or revive deleted channels.
        let _lock = self
            .writes
            .lock()
            .map_err(|_| ProviderError::Unavailable("QQPD state lock poisoned".into()))?;
        let Some(mut latest) = self.load_clean(name)? else {
            return Ok(());
        };
        if !latest.channels.contains(&channel) || latest.cookie != cookie {
            return Ok(());
        }
        match outcome {
            Ok(guild) => {
                latest.channel_guild_ids.insert(channel.clone(), guild);
                latest.dead_channel_failures.remove(&channel);
            }
            Err(ResolveError::Missing) => {
                let count = latest
                    .dead_channel_failures
                    .entry(channel.clone())
                    .or_default();
                *count = count.saturating_add(1);
                self.warn(format!(
                    "QQPD profile {name}: channel {channel} explicitly absent (10003){}",
                    if *count >= 2 {
                        "; isolated, re-add to restore"
                    } else {
                        "; retry on next command"
                    }
                ));
            }
            Err(ResolveError::Other(error)) => {
                // Only consecutive explicit missing failures count, not unrelated errors.
                latest.dead_channel_failures.remove(&channel);
                self.warn(format!(
                    "QQPD profile {name}: channel {channel} index repair failed: {error}"
                ));
            }
        }
        self.store
            .save("qqpd", name, &latest)
            .map_err(state_error)?;
        Ok(())
    }
    pub(super) async fn repair_profile(
        &self,
        client: &Client,
        name: &str,
    ) -> Result<(), ProviderError> {
        let Some(snapshot) = self.load_profile(name)? else {
            return Ok(());
        };
        let tasks = snapshot
            .channels
            .iter()
            .filter(|channel| {
                !snapshot.channel_guild_ids.contains_key(*channel)
                    && snapshot
                        .dead_channel_failures
                        .get(*channel)
                        .copied()
                        .unwrap_or(0)
                        < 2
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut outcomes = stream::iter(tasks.into_iter().map(|channel| {
            let cookie = &snapshot.cookie;
            async move {
                let result =
                    resolve_guild_id(client, &self.endpoints.guild_page, &channel, cookie).await;
                (channel, result)
            }
        }))
        .buffer_unordered(8);
        while let Some((channel, outcome)) = outcomes.next().await {
            self.persist_resolution(name, &snapshot.cookie, channel, outcome)?;
        }
        Ok(())
    }
}

pub(super) async fn request_api(
    client: &Client,
    endpoint: &Url,
    cookie: &str,
    oidb: &str,
    payload: serde_json::Value,
    lookup: bool,
) -> Result<serde_json::Value, ProviderError> {
    let cookies = parse_cookies(cookie);
    let skey = cookies
        .get("p_skey")
        .filter(|v| !v.is_empty())
        .ok_or(ProviderError::AuthRequired)?;
    let mut request = client
        .post(endpoint.clone())
        .query(&[("bkn", bkn(skey).to_string())]);
    if lookup {
        request = request.query(&[("forGuildId", "1")]);
    }
    let response = crate::http::execute(
        request
            .timeout(Duration::from_secs(15))
            .header("X-QQ-Client-AppId", "537246381")
            .header("x-oidb", oidb)
            .header(header::ACCEPT, "application/json")
            .header(header::USER_AGENT, USER_AGENT)
            .header(header::ORIGIN, "https://pd.qq.com")
            .header(header::REFERER, "https://pd.qq.com/")
            .header(header::COOKIE, cookie)
            .json(&payload),
    )
    .await
    .map_err(network)?;
    let status = response.status();
    let body = crate::http::read_response(response, 4 * 1024 * 1024)
        .await
        .map_err(|_| {
            ProviderError::Protocol("QQPD response read failed or exceeded 4MiB".into())
        })?;
    if body
        .windows(b"EO-Bot-Js-Token".len())
        .any(|w| w == b"EO-Bot-Js-Token")
    {
        return Err(ProviderError::Protocol(
            "QQPD EdgeOne challenge required".into(),
        ));
    }
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(ProviderError::AuthRequired);
    }
    if status.as_u16() == 429 {
        return Err(ProviderError::RateLimited);
    }
    if status.as_u16() != 200 {
        return Err(ProviderError::Network(format!("QQPD HTTP {status}")));
    }
    validate_api(&String::from_utf8_lossy(&body))
}
fn safe_error_message(message: &str) -> String {
    use std::sync::LazyLock;
    static SECRETS: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)(?:https?://\S+|(?:authorization|cookie)\s*[:=][^\r\n]*|(?:bkn|p_skey|skey|token)\s*[:=]\s*[^\s,;]+)").expect("static redaction regex")
    });
    SECRETS
        .replace_all(message, "[redacted]")
        .chars()
        .filter(|c| !c.is_control())
        .take(256)
        .collect()
}

pub(super) fn validate_api(body: &str) -> Result<serde_json::Value, ProviderError> {
    if body.contains("EO-Bot-Js-Token") {
        return Err(ProviderError::Protocol(
            "QQPD EdgeOne challenge required".into(),
        ));
    }
    let root: serde_json::Value =
        serde_json::from_str(body).map_err(|_| ProviderError::Parse("QQPD invalid JSON".into()))?;
    let retcode = root["retcode"]
        .as_i64()
        .ok_or_else(|| ProviderError::Protocol("QQPD omitted retcode".into()))?;
    let code = root
        .pointer("/error/code")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if retcode != 0 || code != 0 {
        // Do not echo arbitrary sensitive upstream payloads.
        let message = root
            .pointer("/error/message")
            .or_else(|| root.pointer("/error/msg"))
            .or_else(|| root.get("message"))
            .or_else(|| root.get("msg"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        return Err(ProviderError::Protocol(format!(
            "QQPD retcode={retcode}, error.code={code}: {}",
            safe_error_message(message)
        )));
    }
    Ok(root)
}
