use std::{
    collections::{BTreeMap, HashSet},
    sync::{
        Arc, Mutex,
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

#[cfg(test)]
#[path = "delivery_tests.rs"]
mod delivery_tests;
#[path = "index.rs"]
mod index;
#[cfg(test)]
#[path = "upstream_tests.rs"]
mod upstream_tests;
use index::{request_api, validate_api};

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
            guild_page: static_url(
                "https://pd.qq.com/qunng/guild/gotrpc/auth/trpc.group_pro.cmd0x907e.Cmd0x907e/HandleProcess",
            ),
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
    #[serde(default)]
    pub dead_channel_failures: BTreeMap<String, u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_keepalive: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for QqpdProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QqpdProfile")
            .field("active", &self.active)
            .field("channel_count", &self.channels.len())
            .finish_non_exhaustive()
    }
}

impl QqpdProfile {
    pub fn authenticated(cookie: impl Into<String>, qq_masked: impl Into<String>) -> Self {
        Self {
            active: true,
            cookie: cookie.into(),
            qq_masked: qq_masked.into(),
            channels: Vec::new(),
            channel_guild_ids: BTreeMap::new(),
            dead_channel_failures: BTreeMap::new(),
            login_at: Some(Utc::now()),
            expires_at: None,
            last_keepalive: None,
        }
    }
    pub fn is_ready(&self, now: DateTime<Utc>) -> bool {
        self.active && !self.cookie.is_empty() && self.expires_at.is_none_or(|v| v > now)
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
    writes: Arc<Mutex<()>>,
    diagnostics: Arc<Mutex<Vec<String>>>,
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
            writes: Arc::default(),
            diagnostics: Arc::default(),
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
        let _lock = self
            .writes
            .lock()
            .map_err(|_| ProviderError::Unavailable("QQPD state lock poisoned".into()))?;
        self.load_clean(name)
    }
    pub fn save_login(
        &self,
        name: &str,
        cookie: impl Into<String>,
        qq_masked: impl Into<String>,
    ) -> Result<(), ProviderError> {
        let _lock = self
            .writes
            .lock()
            .map_err(|_| ProviderError::Unavailable("QQPD state lock poisoned".into()))?;
        let mut state = self
            .load_clean(name)?
            .unwrap_or_else(|| QqpdProfile::authenticated("", ""));
        state.cookie = cookie.into();
        state.qq_masked = qq_masked.into();
        state.active = true;
        state.login_at = Some(Utc::now());
        state.expires_at = None;
        self.store.save("qqpd", name, &state).map_err(state_error)
    }
    pub fn logout(&self, name: &str) -> Result<bool, ProviderError> {
        let _lock = self
            .writes
            .lock()
            .map_err(|_| ProviderError::Unavailable("QQPD state lock poisoned".into()))?;
        self.store.delete("qqpd", name).map_err(state_error)
    }

    pub async fn add_channels<I, S>(
        &self,
        client: &Client,
        profile: &str,
        values: I,
    ) -> Result<Vec<String>, ProviderError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let additions = normalize_channels(values)?;
        {
            let _lock = self
                .writes
                .lock()
                .map_err(|_| ProviderError::Unavailable("QQPD state lock poisoned".into()))?;
            let mut state = self
                .load_clean(profile)?
                .unwrap_or_else(|| QqpdProfile::authenticated("", ""));
            for channel in additions {
                state.dead_channel_failures.remove(&channel);
                if !state.channels.contains(&channel) {
                    state.channels.push(channel);
                }
            }
            self.store
                .save("qqpd", profile, &state)
                .map_err(state_error)?;
        }
        self.repair_profile(client, profile).await?;
        self.target_channels(profile)
    }

    pub fn delete_channels<I, S>(
        &self,
        profile: &str,
        values: I,
    ) -> Result<Vec<String>, ProviderError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let removals = normalize_channels(values)?
            .into_iter()
            .collect::<HashSet<_>>();
        let _lock = self
            .writes
            .lock()
            .map_err(|_| ProviderError::Unavailable("QQPD state lock poisoned".into()))?;
        let mut state = self
            .load_clean(profile)?
            .unwrap_or_else(|| QqpdProfile::authenticated("", ""));
        state.channels.retain(|channel| !removals.contains(channel));
        state
            .channel_guild_ids
            .retain(|channel, _| state.channels.contains(channel));
        self.store
            .save("qqpd", profile, &state)
            .map_err(state_error)?;
        Ok(state.channels)
    }

    pub fn target_channels(&self, profile: &str) -> Result<Vec<String>, ProviderError> {
        Ok(self
            .load_profile(profile)?
            .map(|state| state.channels)
            .unwrap_or_default())
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
        profiles.truncate(10);
        let tasks = assign_channels(&profiles, self.cursor.fetch_add(1, Ordering::Relaxed));
        if tasks.is_empty() {
            return Err(ProviderError::Unavailable(
                "QQPD channels are isolated; re-add channels to restore".into(),
            ));
        }
        let client = ctx.client.clone();
        let endpoint = self.endpoints.search.clone();
        let query = query.to_owned();
        let outcomes = stream::iter(tasks.into_iter().map(|mut task| {
            let client = client.clone();
            let endpoint = endpoint.clone();
            let query = query.clone();
            async move {
                if !valid_guild_id(&task.guild_id) {
                    let outcome = resolve_guild_id(
                        &client,
                        &self.endpoints.guild_page,
                        &task.channel,
                        &task.cookie,
                    )
                    .await;
                    let failure = match &outcome {
                        Ok(_) => None,
                        Err(ResolveError::Missing) => Some(ProviderError::Protocol(
                            "QQPD channel explicitly absent (10003)".into(),
                        )),
                        Err(ResolveError::Other(e)) => {
                            Some(ProviderError::Unavailable(e.to_string()))
                        }
                    };
                    if let Ok(ref guild) = outcome {
                        task.guild_id = guild.clone();
                    }
                    self.persist_resolution(
                        &task.profile_name,
                        &task.cookie,
                        task.channel.clone(),
                        outcome,
                    )?;
                    if let Some(error) = failure {
                        return Err(error);
                    }
                }
                if ctx.is_cancelled() {
                    return Err(ProviderError::Timeout);
                }
                let rows = search_channel(&client, &endpoint, &task, &query).await?;
                ctx.publish(rows.clone());
                Ok::<_, ProviderError>(rows)
            }
        }))
        .buffer_unordered(10)
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
        let warnings = self.take_diagnostics();
        if let Some(e) = error {
            return Err(e);
        }
        if !warnings.is_empty() {
            return Err(ProviderError::Unavailable(warnings.join("; ")));
        }
        Ok(output)
    }
}

struct ChannelTask {
    profile_name: String,
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
                .filter(|(_, (_, p))| {
                    p.channels.contains(&channel)
                        && p.dead_channel_failures.get(&channel).copied().unwrap_or(0) < 2
                })
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
                profile_name: profiles[i].0.clone(),
                guild_id: p
                    .channel_guild_ids
                    .get(&channel)
                    .cloned()
                    .unwrap_or_default(),
                channel,
                cookie: p.cookie.clone(),
            })
        })
        .collect()
}

async fn resolve_guild_id(
    client: &Client,
    endpoint: &Url,
    channel: &str,
    cookie: &str,
) -> Result<String, ResolveError> {
    if valid_guild_id(channel) {
        return Ok(channel.into());
    }
    if channel.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ResolveError::Other(ProviderError::Protocol(
            "QQPD invalid numeric guild ID".into(),
        )));
    }
    let root = request_api(client, endpoint, cookie, r#"{"uint32_service_type":1}"#, serde_json::json!({
        "guild_num":channel,
        "cmd0xf57_req":{"msg_field_filter":{"msg_guild_info_filter":{"uint32_guild_number":1}},"rpt_req_guild_info_list":[{}]}
    }), true).await.map_err(ResolveError::Other)?;
    let result = root
        .pointer("/data/cmd0xf57_rsp/rpt_rsp_guild_info_list/0/uint32_result")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| {
            ResolveError::Other(ProviderError::Protocol(
                "QQPD lookup omitted uint32_result".into(),
            ))
        })?;
    if result == 10003 {
        return Err(ResolveError::Missing);
    }
    if result != 0 {
        return Err(ResolveError::Other(ProviderError::Protocol(format!(
            "QQPD lookup uint32_result={result}"
        ))));
    }
    let id = root
        .pointer("/data/cmd0xf57_rsp/rpt_rsp_guild_info_list/0/uint64_guild_id")
        .map(value_string)
        .filter(|v| valid_guild_id(v))
        .ok_or_else(|| {
            ResolveError::Other(ProviderError::Protocol(
                "QQPD lookup omitted valid guild ID".into(),
            ))
        })?;
    Ok(id)
}
enum ResolveError {
    Missing,
    Other(ProviderError),
}

pub fn parse_guild_id(html: &str) -> Option<String> {
    let marker = "https://groupprohead.gtimg.cn/";
    let rest = &html[html.find(marker)? + marker.len()..];
    let id: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (!id.is_empty()).then_some(id)
}
fn valid_guild_id(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(|b| matches!(b, b'1'..=b'9'))
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<u64>().is_ok()
}

async fn search_channel(
    client: &Client,
    endpoint: &Url,
    task: &ChannelTask,
    query: &str,
) -> Result<Vec<SearchResult>, ProviderError> {
    let root = request_api(client, endpoint, &task.cookie, r#"{"uint32_command":"0x9287","uint32_service_type":"2"}"#,
        serde_json::json!({"guild_id":task.guild_id,"query":query,"cookie":"","member_cookie":"","search_type":{"type":0,"feed_type":0},"cond":{"channel_ids":[],"feed_rank_type":0,"type_list":[2,3]}}), false).await?;
    let body = root.to_string();
    parse_search_response(&body, &task.channel)
}

pub fn parse_search_response(
    body: &str,
    channel: &str,
) -> Result<Vec<SearchResult>, ProviderError> {
    let root = validate_api(body)?;
    let data = root
        .get("data")
        .filter(|v| v.is_object())
        .ok_or_else(|| ProviderError::Protocol("QQPD response omitted data".into()))?;
    if data.get("union_result").is_some_and(|v| v.is_null()) {
        return Ok(Vec::new());
    }
    let feeds = data
        .pointer("/union_result/guild_feeds")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ProviderError::Protocol("QQPD response omitted guild_feeds".into()))?;
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
    if (candidate.bytes().all(|b| b.is_ascii_digit()) && !valid_guild_id(&candidate))
        || (candidate.starts_with('-') && candidate[1..].bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(ProviderError::Protocol(
            "QQPD numeric guild ID must be a positive canonical u64".into(),
        ));
    }
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
fn network(error: impl Into<anyhow::Error>) -> ProviderError {
    let error = error.into();
    match error.downcast::<reqwest::Error>() {
        Ok(error) => ProviderError::Network(error.without_url().to_string()),
        Err(error) => ProviderError::Network(error.to_string()),
    }
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
            channel_guild_ids: [
                ("one".into(), "1".into()),
                ("two".into(), "2".into()),
                ("three".into(), "3".into()),
            ]
            .into(),
            dead_channel_failures: BTreeMap::new(),
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
            .add_channels(&Client::new(), "main", ["12345", "12345"])
            .await
            .unwrap();
        assert_eq!(channels, ["12345"]);
        let state = provider.load_profile("main").unwrap().unwrap();
        assert_eq!(
            state.channel_guild_ids.get("12345").map(String::as_str),
            Some("12345")
        );
        assert_eq!(
            provider
                .add_channels(&Client::new(), "main", ["12345", "67890"])
                .await
                .unwrap(),
            ["12345", "67890"]
        );
        assert_eq!(
            provider
                .delete_channels("main", ["12345", "missing"])
                .unwrap(),
            ["67890"]
        );
        assert_eq!(provider.target_channels("main").unwrap(), ["67890"]);
        let state = provider.load_profile("main").unwrap().unwrap();
        assert!(!state.channel_guild_ids.contains_key("12345"));
        assert_eq!(
            state.channel_guild_ids.get("67890").map(String::as_str),
            Some("67890")
        );
    }

    #[tokio::test]
    async fn failed_guild_resolution_preserves_channels_and_warns() {
        let tmp = tempfile::tempdir().unwrap();
        let endpoints = QqpdEndpoints {
            guild_page: Url::parse("http://127.0.0.1:1/g/").unwrap(),
            ..QqpdEndpoints::default()
        };
        let provider =
            QqpdProvider::with_endpoints(StateStore::with_cipher(tmp.path(), None), endpoints);
        provider.save_login("main", "p_skey=x", "masked").unwrap();
        provider
            .add_channels(&Client::new(), "main", ["12345"])
            .await
            .unwrap();
        assert!(
            provider
                .add_channels(&Client::new(), "main", ["named-channel"])
                .await
                .is_ok()
        );
        let state = provider.load_profile("main").unwrap().unwrap();
        assert_eq!(state.channels, ["12345", "named-channel"]);
        assert!(!state.channel_guild_ids.contains_key("named-channel"));
        assert!(!provider.take_diagnostics().is_empty());
        assert_eq!(
            state.channel_guild_ids.get("12345").map(String::as_str),
            Some("12345")
        );
    }
}
