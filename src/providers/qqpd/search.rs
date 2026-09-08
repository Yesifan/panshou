use std::{
    collections::{BTreeMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::{StreamExt, stream};
use reqwest::{Client, header};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    core::{ProviderError, SearchResult, Source, extract_links},
    providers::{AuthKind, KeywordFilterMode, Provider, ProviderMeta, SearchContext},
    state::StateStore,
};

const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120.0.0.0 Safari/537.36";
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(180);

#[derive(Debug, Clone)]
pub struct QqpdEndpoints {
    pub pd_origin: Url,
    pub qq_login_origin: Url,
    pub explore: Url,
    pub guild_page: Url,
    pub search: Url,
    pub qr_show: Url,
    pub qr_login: Url,
    pub check_sig: Url,
}

impl Default for QqpdEndpoints {
    fn default() -> Self {
        Self {
            pd_origin: static_url("https://pd.qq.com/"),
            qq_login_origin: static_url("https://xui.ptlogin2.qq.com/"),
            explore: static_url("https://pd.qq.com/explore"),
            guild_page: static_url("https://pd.qq.com/g/"),
            search: static_url(
                "https://pd.qq.com/qunng/guild/gotrpc/auth/trpc.group_pro.in_guild_search_svr.InGuildSearch/NewSearch",
            ),
            qr_show: static_url("https://xui.ptlogin2.qq.com/ssl/ptqrshow"),
            qr_login: static_url("https://xui.ptlogin2.qq.com/ssl/ptqrlogin"),
            check_sig: static_url("https://ptlogin2.pd.qq.com/check_sig"),
        }
    }
}
fn static_url(value: &str) -> Url {
    Url::parse(value).expect("built-in QQPD URL is valid")
}

#[derive(Clone, Serialize, Deserialize)]
pub struct QqpdProfile {
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub cookie: String,
    #[serde(default)]
    pub qq_masked: String,
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default)]
    pub channel_guild_ids: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_keepalive: Option<DateTime<Utc>>,
}

impl QqpdProfile {
    pub fn authenticated(cookie: impl Into<String>, qq_masked: impl Into<String>) -> Self {
        Self {
            active: true,
            cookie: cookie.into(),
            qq_masked: qq_masked.into(),
            channels: Vec::new(),
            channel_guild_ids: BTreeMap::new(),
            login_at: Some(Utc::now()),
            expires_at: None,
            last_keepalive: None,
        }
    }
    pub fn is_ready(&self, now: DateTime<Utc>) -> bool {
        self.active
            && !self.cookie.is_empty()
            && self.expires_at.is_none_or(|v| v > now)
            && self.channels.iter().all(|channel| {
                self.channel_guild_ids
                    .get(channel)
                    .is_some_and(|guild| valid_guild_id(guild))
            })
    }

    pub fn needs_keepalive(&self, now: DateTime<Utc>) -> bool {
        self.last_keepalive.is_none_or(|last| {
            now.signed_duration_since(last).num_seconds() >= KEEPALIVE_INTERVAL.as_secs() as i64
        })
    }
}

#[derive(Clone)]
pub struct QqpdProvider {
    store: StateStore,
    endpoints: QqpdEndpoints,
    cursor: Arc<AtomicUsize>,
}

impl QqpdProvider {
    pub fn new(store: StateStore) -> Self {
        Self::with_endpoints(store, QqpdEndpoints::default())
    }
    pub fn with_endpoints(store: StateStore, endpoints: QqpdEndpoints) -> Self {
        Self {
            store,
            endpoints,
            cursor: Arc::new(AtomicUsize::new(0)),
        }
    }
    pub fn endpoints(&self) -> &QqpdEndpoints {
        &self.endpoints
    }
    pub fn auth(&self, client: Client) -> super::QqpdAuth {
        super::QqpdAuth::new(client, self.endpoints.clone())
    }
    pub fn auth_session(&self, session: crate::http::Session) -> super::QqpdAuth {
        super::QqpdAuth::with_session(session, self.endpoints.clone())
    }
    pub fn profiles(&self) -> Result<Vec<String>, ProviderError> {
        self.store.list_profiles("qqpd").map_err(state_error)
    }
    pub fn load_profile(&self, name: &str) -> Result<Option<QqpdProfile>, ProviderError> {
        self.store.load("qqpd", name).map_err(state_error)
    }
    pub fn save_login(
        &self,
        name: &str,
        cookie: impl Into<String>,
        qq_masked: impl Into<String>,
    ) -> Result<(), ProviderError> {
        let mut state = self
            .load_profile(name)?
            .unwrap_or_else(|| QqpdProfile::authenticated("", ""));
        state.cookie = cookie.into();
        state.qq_masked = qq_masked.into();
        state.active = true;
        state.login_at = Some(Utc::now());
        state.expires_at = None;
        self.store.save("qqpd", name, &state).map_err(state_error)
    }
    pub fn logout(&self, name: &str) -> Result<bool, ProviderError> {
        self.store.delete("qqpd", name).map_err(state_error)
    }

    pub async fn configure_channels<I, S>(
        &self,
        client: &Client,
        profile: &str,
        values: I,
    ) -> Result<Vec<String>, ProviderError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let channels = normalize_channels(values)?;
        let mut state = self
            .load_profile(profile)?
            .unwrap_or_else(|| QqpdProfile::authenticated("", ""));
        let existing = state.channel_guild_ids.clone();
        let base = self.endpoints.guild_page.clone();
        let resolutions = stream::iter(channels.iter().cloned().map(|channel| {
            let client = client.clone();
            let base = base.clone();
            let cached = existing
                .get(&channel)
                .filter(|guild| valid_guild_id(guild))
                .cloned();
            async move {
                if let Some(guild) = cached {
                    Ok((channel, guild))
                } else {
                    let guild = resolve_guild_id(&client, &base, &channel).await?;
                    Ok::<_, ProviderError>((channel, guild))
                }
            }
        }))
        .buffer_unordered(8)
        .collect::<Vec<_>>()
        .await;
        let resolutions = resolutions.into_iter().collect::<Result<Vec<_>, _>>()?;
        state.channels = channels.clone();
        state.channel_guild_ids = resolutions.into_iter().collect();
        self.store
            .save("qqpd", profile, &state)
            .map_err(state_error)?;
        Ok(channels)
    }

    fn active_profiles(&self) -> Result<Vec<(String, QqpdProfile)>, ProviderError> {
        let now = Utc::now();
        let mut output = Vec::new();
        for name in self.profiles()? {
            if let Some(profile) = self.load_profile(&name)?
                && profile.is_ready(now)
                && !profile.channels.is_empty()
            {
                output.push((name, profile));
            }
        }
        Ok(output)
    }

    async fn opportunistic_keepalive(
        &self,
        client: &Client,
        name: &str,
        profile: &mut QqpdProfile,
    ) {
        if !profile.needs_keepalive(Utc::now()) {
            return;
        }
        if let Ok(response) = client
            .get(self.endpoints.explore.clone())
            .header(header::COOKIE, &profile.cookie)
            .header(header::USER_AGENT, USER_AGENT)
            .send()
            .await
            && (response.status().is_success() || response.status().is_redirection())
        {
            profile.cookie = merge_set_cookies(&profile.cookie, response.headers());
            profile.last_keepalive = Some(Utc::now());
            let _ = self.store.save("qqpd", name, profile);
        }
    }
}

#[async_trait]
impl Provider for QqpdProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta {
            name: "qqpd",
            priority: 3,
            requires_auth: true,
            auth_kind: AuthKind::Qr,
            keyword_filter: KeywordFilterMode::Core,
        }
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let mut profiles = self.active_profiles()?;
        if profiles.is_empty() {
            return Err(ProviderError::AuthRequired);
        }
        for (name, profile) in &mut profiles {
            self.opportunistic_keepalive(&ctx.client, name, profile)
                .await;
        }
        let tasks = assign_channels(&profiles, self.cursor.fetch_add(1, Ordering::Relaxed));
        let client = ctx.client.clone();
        let endpoint = self.endpoints.search.clone();
        let query = query.to_owned();
        let outcomes = stream::iter(tasks.into_iter().map(|task| {
            let client = client.clone();
            let endpoint = endpoint.clone();
            let query = query.clone();
            async move { search_channel(&client, &endpoint, &task, &query).await }
        }))
        .buffered(10)
        .collect::<Vec<_>>()
        .await;
        let mut output = Vec::new();
        let mut error = None;
        for item in outcomes {
            match item {
                Ok(mut rows) => output.append(&mut rows),
                Err(e) if error.is_none() => error = Some(e),
                Err(_) => {}
            }
        }
        if output.is_empty()
            && let Some(e) = error
        {
            return Err(e);
        }
        Ok(output)
    }
}

struct ChannelTask {
    channel: String,
    guild_id: String,
    cookie: String,
}
fn assign_channels(profiles: &[(String, QqpdProfile)], seed: usize) -> Vec<ChannelTask> {
    let mut channels = Vec::new();
    for (_, p) in profiles {
        for ch in &p.channels {
            if !channels.contains(ch) {
                channels.push(ch.clone())
            }
        }
    }
    let mut counts = vec![0usize; profiles.len()];
    channels
        .into_iter()
        .filter_map(|channel| {
            let eligible = profiles
                .iter()
                .enumerate()
                .filter(|(_, (_, p))| p.channels.contains(&channel))
                .map(|(i, _)| i)
                .collect::<Vec<_>>();
            let i = eligible.into_iter().min_by_key(|&i| {
                (
                    counts[i],
                    (i + profiles.len() - seed % profiles.len()) % profiles.len(),
                )
            })?;
            counts[i] += 1;
            let p = &profiles[i].1;
            Some(ChannelTask {
                guild_id: p
                    .channel_guild_ids
                    .get(&channel)
                    .cloned()
                    .unwrap_or_else(|| channel.clone()),
                channel,
                cookie: p.cookie.clone(),
            })
        })
        .collect()
}

async fn resolve_guild_id(
    client: &Client,
    base: &Url,
    channel: &str,
) -> Result<String, ProviderError> {
    if channel.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(channel.into());
    }
    let url = base
        .join(channel)
        .map_err(|e| ProviderError::Protocol(e.to_string()))?;
    let body = client
        .get(url)
        .send()
        .await
        .map_err(network)?
        .error_for_status()
        .map_err(network)?
        .text()
        .await
        .map_err(network)?;
    parse_guild_id(&body).ok_or_else(|| {
        ProviderError::Protocol(format!("QQPD channel {channel} page omitted guild_id"))
    })
}
pub fn parse_guild_id(html: &str) -> Option<String> {
    let marker = "https://groupprohead.gtimg.cn/";
    let rest = &html[html.find(marker)? + marker.len()..];
    let id: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (!id.is_empty()).then_some(id)
}
fn valid_guild_id(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

async fn search_channel(
    client: &Client,
    endpoint: &Url,
    task: &ChannelTask,
    query: &str,
) -> Result<Vec<SearchResult>, ProviderError> {
    let cookies = parse_cookies(&task.cookie);
    let p_skey = cookies.get("p_skey").ok_or(ProviderError::AuthRequired)?;
    let response=client.post(endpoint.clone()).query(&[("bkn",bkn(p_skey).to_string())])
        .header("x-oidb",r#"{"uint32_command":"0x9287","uint32_service_type":"2"}"#).header(header::CONTENT_TYPE,"application/json")
        .header(header::USER_AGENT,USER_AGENT).header(header::REFERER,"https://pd.qq.com/").header(header::ORIGIN,"https://pd.qq.com").header(header::COOKIE,&task.cookie)
        .json(&serde_json::json!({"guild_id":task.guild_id,"query":query,"cookie":"","member_cookie":"","search_type":{"type":0,"feed_type":0},"cond":{"channel_ids":[],"feed_rank_type":0,"type_list":[2,3]}}))
        .send().await.map_err(network)?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED
        || response.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err(ProviderError::AuthRequired);
    }
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(ProviderError::RateLimited);
    }
    let body = response
        .error_for_status()
        .map_err(network)?
        .text()
        .await
        .map_err(network)?;
    parse_search_response(&body, &task.channel)
}

pub fn parse_search_response(
    body: &str,
    channel: &str,
) -> Result<Vec<SearchResult>, ProviderError> {
    let root: serde_json::Value =
        serde_json::from_str(body).map_err(|e| ProviderError::Parse(e.to_string()))?;
    let feeds = root
        .pointer("/data/union_result/guild_feeds")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            let message = root
                .get("message")
                .or_else(|| root.get("msg"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if message.contains("登录") || message.to_ascii_lowercase().contains("login") {
                ProviderError::AuthRequired
            } else {
                ProviderError::Protocol("QQPD response omitted guild_feeds".into())
            }
        })?;
    Ok(feeds
        .iter()
        .enumerate()
        .filter_map(|(index, item)| parse_feed(item, channel, index))
        .collect())
}
fn parse_feed(item: &serde_json::Value, channel: &str, index: usize) -> Option<SearchResult> {
    let raw_title = item.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let title = raw_title
        .strip_prefix("名称：")
        .unwrap_or(raw_title)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_owned();
    let content = item
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let mut links = extract_links(&content);
    if title.is_empty() || links.is_empty() {
        return None;
    }
    let datetime = item
        .get("create_time")
        .and_then(value_i64)
        .and_then(|v| DateTime::from_timestamp(v, 0));
    for link in &mut links {
        link.datetime = datetime;
    }
    let images = item
        .get("images")
        .and_then(|v| v.as_array())
        .map(|v| {
            v.iter()
                .filter_map(|i| i.get("url").and_then(|u| u.as_str()).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let feed_id = item
        .get("id")
        .or_else(|| item.get("feed_id"))
        .map(value_string)
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| index.to_string());
    Some(SearchResult {
        id: format!("qqpd-{channel}-{feed_id}"),
        source: Source::provider("qqpd"),
        datetime,
        title,
        content,
        links,
        tags: Vec::new(),
        images,
    })
}

pub fn normalize_channel(value: &str) -> Result<String, ProviderError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ProviderError::Protocol("empty QQPD channel".into()));
    }
    let candidate = if value.contains("://") {
        let url = Url::parse(value)
            .map_err(|_| ProviderError::Protocol(format!("invalid QQPD channel: {value}")))?;
        if url.host_str() != Some("pd.qq.com") {
            return Err(ProviderError::Protocol(format!(
                "invalid QQPD channel host: {value}"
            )));
        }
        let segments = url
            .path_segments()
            .map(|s| s.collect::<Vec<_>>())
            .unwrap_or_default();
        match segments.as_slice() {
            ["g", id, ..] => (*id).to_owned(),
            _ => String::new(),
        }
    } else {
        value.to_owned()
    };
    if candidate.is_empty()
        || !candidate
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(ProviderError::Protocol(format!(
            "invalid QQPD channel: {value}"
        )));
    }
    Ok(candidate)
}
pub fn normalize_channels<I, S>(values: I) -> Result<Vec<String>, ProviderError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for value in values {
        for part in value.as_ref().split(',') {
            let channel = normalize_channel(part)?;
            if seen.insert(channel.clone()) {
                output.push(channel)
            }
        }
    }
    Ok(output)
}
pub fn bkn(skey: &str) -> i64 {
    let mut value = 5381i64;
    for byte in skey.bytes() {
        value = value
            .wrapping_add(value.wrapping_shl(5))
            .wrapping_add(i64::from(byte));
    }
    value & 0x7fff_ffff
}
fn parse_cookies(value: &str) -> BTreeMap<String, String> {
    value
        .split(';')
        .filter_map(|v| v.trim().split_once('='))
        .map(|(k, v)| (k.into(), v.into()))
        .collect()
}
fn merge_set_cookies(existing: &str, headers: &header::HeaderMap) -> String {
    let mut values = parse_cookies(existing);
    for raw in headers.get_all(header::SET_COOKIE) {
        if let Ok(raw) = raw.to_str()
            && let Some((k, v)) = raw.split(';').next().and_then(|v| v.split_once('='))
        {
            values.insert(k.trim().into(), v.trim().into());
        }
    }
    values
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ")
}
fn value_i64(value: &serde_json::Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}
fn value_string(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|v| v.to_string()))
        .unwrap_or_default()
}
fn network(error: reqwest::Error) -> ProviderError {
    ProviderError::Network(error.to_string())
}
fn state_error(error: crate::state::StateError) -> ProviderError {
    ProviderError::Unavailable(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn channel_normalization_and_dedupe() {
        assert_eq!(
            normalize_channels(["pd97631607,languan8K115", "https://pd.qq.com/g/pd97631607"])
                .unwrap(),
            ["pd97631607", "languan8K115"]
        );
        assert!(normalize_channel("https://evil.test/g/a").is_err());
    }
    #[test]
    fn guild_id_parser_has_fallback_signal() {
        assert_eq!(
            parse_guild_id(include_str!(
                "../../../tests/fixtures/providers/qqpd/channel.html"
            ))
            .as_deref(),
            Some("592843764045681811")
        );
        assert_eq!(parse_guild_id("changed"), None);
    }
    #[test]
    fn response_parser_extracts_links_password_time_and_images() {
        let rows = parse_search_response(
            include_str!("../../../tests/fixtures/providers/qqpd/search.json"),
            "pd97631607",
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "仙逆 最新合集");
        assert_eq!(rows[0].links[0].password.as_deref(), Some("a1b2"));
        assert_eq!(rows[0].images, ["https://img.test/a.jpg"]);
        assert!(
            parse_search_response(
                include_str!("../../../tests/fixtures/providers/qqpd/malformed.json"),
                "x"
            )
            .is_err()
        );
        assert!(
            parse_search_response(
                include_str!("../../../tests/fixtures/providers/qqpd/empty.json"),
                "x"
            )
            .unwrap()
            .is_empty()
        );
    }
    #[test]
    fn profile_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = QqpdProvider::new(StateStore::with_cipher(tmp.path(), None));
        provider
            .save_login("main", "p_skey=x", "1234****89")
            .unwrap();
        let state = provider.load_profile("main").unwrap().unwrap();
        assert!(state.is_ready(Utc::now()));
        assert_eq!(state.qq_masked, "1234****89");
    }

    #[test]
    fn shared_channels_are_balanced_and_keepalive_is_opportunistic() {
        let profile = |cookie: &str| QqpdProfile {
            active: true,
            cookie: cookie.into(),
            qq_masked: String::new(),
            channels: vec!["one".into(), "two".into(), "three".into()],
            channel_guild_ids: BTreeMap::new(),
            login_at: None,
            expires_at: None,
            last_keepalive: Some(Utc::now() - chrono::Duration::seconds(181)),
        };
        let profiles = vec![
            ("a".into(), profile("p_skey=a")),
            ("b".into(), profile("p_skey=b")),
        ];
        let tasks = assign_channels(&profiles, 0);
        assert_eq!(tasks.len(), 3);
        assert_eq!(
            tasks
                .iter()
                .filter(|task| task.cookie == "p_skey=a")
                .count(),
            2
        );
        assert!(profiles[0].1.needs_keepalive(Utc::now()));
    }

    #[tokio::test]
    async fn configure_numeric_channels_persists_guild_cache_without_network() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = QqpdProvider::new(StateStore::with_cipher(tmp.path(), None));
        provider.save_login("main", "p_skey=x", "masked").unwrap();
        let channels = provider
            .configure_channels(&Client::new(), "main", ["12345", "12345"])
            .await
            .unwrap();
        assert_eq!(channels, ["12345"]);
        let state = provider.load_profile("main").unwrap().unwrap();
        assert_eq!(
            state.channel_guild_ids.get("12345").map(String::as_str),
            Some("12345")
        );
    }

    #[tokio::test]
    async fn failed_guild_resolution_does_not_overwrite_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let endpoints = QqpdEndpoints {
            guild_page: Url::parse("http://127.0.0.1:1/g/").unwrap(),
            ..QqpdEndpoints::default()
        };
        let provider =
            QqpdProvider::with_endpoints(StateStore::with_cipher(tmp.path(), None), endpoints);
        provider.save_login("main", "p_skey=x", "masked").unwrap();
        provider
            .configure_channels(&Client::new(), "main", ["12345"])
            .await
            .unwrap();
        assert!(
            provider
                .configure_channels(&Client::new(), "main", ["named-channel"])
                .await
                .is_err()
        );
        let state = provider.load_profile("main").unwrap().unwrap();
        assert_eq!(state.channels, ["12345"]);
        assert_eq!(
            state.channel_guild_ids.get("12345").map(String::as_str),
            Some("12345")
        );
    }
}
