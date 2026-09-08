use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::{StreamExt, stream};
use reqwest::{Client, header};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    core::{Link, ProviderError, SearchResult, Source, canonical_url_key, extract_links},
    providers::{AuthKind, KeywordFilterMode, Provider, ProviderMeta, SearchContext},
    state::StateStore,
};

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/120.0.0.0 Safari/537.36";

#[derive(Debug, Clone)]
pub struct WeiboEndpoints {
    pub search: Url,
    pub comments: Url,
    pub qr_info: Url,
    pub qr_image: Url,
    pub qr_check: Url,
    pub web_origin: Url,
    pub mobile_origin: Url,
}

impl Default for WeiboEndpoints {
    fn default() -> Self {
        Self {
            search: static_url("https://weibo.com/ajax/profile/searchblog"),
            comments: static_url("https://m.weibo.cn/comments/hotflow"),
            qr_info: static_url("https://passport.weibo.com/sso/v2/qrcode/image"),
            qr_image: static_url("https://v2.qr.weibo.cn/inf/gen"),
            qr_check: static_url("https://passport.weibo.com/sso/v2/qrcode/check"),
            web_origin: static_url("https://weibo.com/"),
            mobile_origin: static_url("https://m.weibo.cn/"),
        }
    }
}

fn static_url(value: &str) -> Url {
    Url::parse(value).expect("built-in Weibo URL is valid")
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WeiboProfile {
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub cookie: String,
    #[serde(default)]
    pub user_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

impl WeiboProfile {
    pub fn authenticated(cookie: impl Into<String>) -> Self {
        Self {
            active: true,
            cookie: cookie.into(),
            user_ids: Vec::new(),
            login_at: Some(Utc::now()),
            expires_at: None,
        }
    }

    pub fn is_ready(&self, now: DateTime<Utc>) -> bool {
        self.active
            && !self.cookie.trim().is_empty()
            && self.expires_at.is_none_or(|expiry| expiry > now)
    }
}

#[derive(Clone)]
pub struct WeiboProvider {
    store: StateStore,
    endpoints: WeiboEndpoints,
    cursor: Arc<AtomicUsize>,
}

impl WeiboProvider {
    pub fn new(store: StateStore) -> Self {
        Self::with_endpoints(store, WeiboEndpoints::default())
    }

    pub fn with_endpoints(store: StateStore, endpoints: WeiboEndpoints) -> Self {
        Self {
            store,
            endpoints,
            cursor: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn endpoints(&self) -> &WeiboEndpoints {
        &self.endpoints
    }

    pub fn auth(&self, client: Client) -> super::WeiboAuth {
        super::WeiboAuth::new(client, self.endpoints.clone())
    }

    pub fn auth_session(&self, session: crate::http::Session) -> super::WeiboAuth {
        super::WeiboAuth::with_session(session, self.endpoints.clone())
    }

    pub fn profiles(&self) -> Result<Vec<String>, ProviderError> {
        self.store.list_profiles("weibo").map_err(state_error)
    }

    pub fn load_profile(&self, profile: &str) -> Result<Option<WeiboProfile>, ProviderError> {
        self.store.load("weibo", profile).map_err(state_error)
    }

    pub fn save_login(
        &self,
        profile: &str,
        cookie: impl Into<String>,
    ) -> Result<(), ProviderError> {
        let mut state = self
            .load_profile(profile)?
            .unwrap_or_else(|| WeiboProfile::authenticated(""));
        state.cookie = cookie.into();
        state.active = true;
        state.login_at = Some(Utc::now());
        state.expires_at = None;
        self.store
            .save("weibo", profile, &state)
            .map_err(state_error)
    }

    pub fn configure_users<I, S>(
        &self,
        profile: &str,
        values: I,
    ) -> Result<Vec<String>, ProviderError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut state = self
            .load_profile(profile)?
            .unwrap_or_else(|| WeiboProfile::authenticated(""));
        state.user_ids = normalize_user_ids(values)?;
        self.store
            .save("weibo", profile, &state)
            .map_err(state_error)?;
        Ok(state.user_ids)
    }

    pub fn logout(&self, profile: &str) -> Result<bool, ProviderError> {
        self.store.delete("weibo", profile).map_err(state_error)
    }

    fn active_profiles(&self) -> Result<Vec<(String, WeiboProfile)>, ProviderError> {
        let now = Utc::now();
        let mut profiles = Vec::new();
        for name in self.profiles()? {
            if let Some(profile) = self.load_profile(&name)?
                && profile.is_ready(now)
                && !profile.user_ids.is_empty()
            {
                profiles.push((name, profile));
            }
        }
        Ok(profiles)
    }
}

#[async_trait]
impl Provider for WeiboProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta {
            name: "weibo",
            priority: 3,
            requires_auth: true,
            auth_kind: AuthKind::Qr,
            keyword_filter: KeywordFilterMode::Provider,
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
        let profiles = self.active_profiles()?;
        if profiles.is_empty() {
            return Err(ProviderError::AuthRequired);
        }
        let tasks = assign_users(&profiles, self.cursor.fetch_add(1, Ordering::Relaxed));
        let endpoints = self.endpoints.clone();
        let client = ctx.client.clone();
        let query = query.to_owned();
        let outcomes =
            stream::iter(tasks.into_iter().map(|task| {
                let endpoints = endpoints.clone();
                let client = client.clone();
                let query = query.clone();
                async move {
                    search_user(&client, &endpoints, &task.user_id, &task.cookie, &query).await
                }
            }))
            .buffered(10)
            .collect::<Vec<_>>()
            .await;
        let mut results = Vec::new();
        let mut first_error = None;
        for outcome in outcomes {
            match outcome {
                Ok(mut values) => results.append(&mut values),
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        if results.is_empty()
            && let Some(error) = first_error
        {
            return Err(error);
        }
        dedupe_results(&mut results);
        Ok(results)
    }
}

struct UserTask {
    user_id: String,
    cookie: String,
}

fn assign_users(profiles: &[(String, WeiboProfile)], seed: usize) -> Vec<UserTask> {
    let mut users = Vec::<String>::new();
    for (_, profile) in profiles {
        for id in &profile.user_ids {
            if !users.contains(id) {
                users.push(id.clone());
            }
        }
    }
    let mut counts = vec![0usize; profiles.len()];
    users
        .into_iter()
        .filter_map(|user_id| {
            let eligible = profiles
                .iter()
                .enumerate()
                .filter(|(_, (_, p))| p.user_ids.contains(&user_id))
                .map(|(i, _)| i)
                .collect::<Vec<_>>();
            let selected = eligible.into_iter().min_by_key(|&i| {
                (
                    counts[i],
                    (i + profiles.len() - seed % profiles.len()) % profiles.len(),
                )
            })?;
            counts[selected] += 1;
            Some(UserTask {
                user_id,
                cookie: profiles[selected].1.cookie.clone(),
            })
        })
        .collect()
}

async fn search_user(
    client: &Client,
    endpoints: &WeiboEndpoints,
    uid: &str,
    cookie: &str,
    query: &str,
) -> Result<Vec<SearchResult>, ProviderError> {
    let mut output = Vec::new();
    for page in 1..=3 {
        let response = client
            .get(endpoints.search.clone())
            .query(&[
                ("uid", uid),
                ("feature", "0"),
                ("q", query),
                ("page", &page.to_string()),
            ])
            .header(header::USER_AGENT, USER_AGENT)
            .header(header::REFERER, endpoints.web_origin.as_str())
            .header(header::ACCEPT, "application/json, text/plain, */*")
            .header(header::COOKIE, cookie)
            .send()
            .await
            .map_err(network)?;
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
        let posts = parse_search_page(&body, uid)?;
        if posts.is_empty() {
            break;
        }
        for mut post in posts {
            for url in &post.external_urls {
                if let Ok(found) = fetch_external_links(client, url).await {
                    merge_unique_links(&mut post.result.links, found);
                }
            }
            if post.result.links.is_empty() {
                let comments = fetch_comments(client, endpoints, &post.weibo_id, cookie)
                    .await
                    .unwrap_or_default();
                for comment in comments {
                    let direct = extract_links(&comment);
                    if direct.is_empty() {
                        for url in http_urls(&comment) {
                            if let Ok(found) = fetch_external_links(client, &url).await {
                                merge_unique_links(&mut post.result.links, found);
                            }
                        }
                    } else {
                        merge_unique_links(&mut post.result.links, direct);
                    }
                }
            }
            for link in &mut post.result.links {
                if link.datetime.is_none() {
                    link.datetime = post.result.datetime;
                }
            }
            if !post.result.links.is_empty() {
                output.push(post.result);
            }
        }
    }
    Ok(output)
}

#[derive(Debug, Clone)]
pub struct ParsedPost {
    pub result: SearchResult,
    pub weibo_id: String,
    pub external_urls: Vec<String>,
}

pub fn parse_search_page(body: &str, uid: &str) -> Result<Vec<ParsedPost>, ProviderError> {
    let root: serde_json::Value =
        serde_json::from_str(body).map_err(|e| ProviderError::Parse(e.to_string()))?;
    let ok = root.get("ok").is_some_and(|v| {
        v == 1 || v == true || v.as_str().is_some_and(|s| s == "1" || s == "true")
    });
    if !ok {
        let message = root.get("msg").and_then(|v| v.as_str()).unwrap_or("");
        if message.contains("登录") || message.to_ascii_lowercase().contains("login") {
            return Err(ProviderError::AuthRequired);
        }
        if message.contains("频繁") || message.contains("限流") {
            return Err(ProviderError::RateLimited);
        }
        return Ok(Vec::new());
    }
    let list = root
        .pointer("/data/list")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ProviderError::Protocol("Weibo search response omitted data.list".into()))?;
    list.iter().map(|post| parse_post(post, uid)).collect()
}

fn parse_post(post: &serde_json::Value, uid: &str) -> Result<ParsedPost, ProviderError> {
    let text_raw = post
        .get("text_raw")
        .or_else(|| post.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let content = clean_html(text_raw);
    let weibo_id = post
        .get("idstr")
        .or_else(|| post.get("id"))
        .map(value_string)
        .unwrap_or_default();
    if weibo_id.is_empty() {
        return Err(ProviderError::Protocol("Weibo post omitted id".into()));
    }
    let datetime = post
        .get("created_at")
        .and_then(|v| v.as_str())
        .and_then(parse_datetime);
    let mut links = extract_links(&content);
    let mut external_urls = Vec::new();
    if let Some(items) = post.get("url_struct").and_then(|v| v.as_array()) {
        for item in items {
            if let Some(url) = item.get("long_url").and_then(|v| v.as_str()) {
                let direct = extract_links(url);
                if direct.is_empty() {
                    if Url::parse(url).is_ok_and(|v| matches!(v.scheme(), "http" | "https")) {
                        external_urls.push(url.to_owned());
                    }
                } else {
                    merge_unique_links(&mut links, direct);
                }
            }
        }
    }
    for link in &mut links {
        link.datetime = datetime;
    }
    let title = if content.chars().count() > 100 {
        format!("{}...", content.chars().take(100).collect::<String>())
    } else {
        content.clone()
    };
    Ok(ParsedPost {
        weibo_id: weibo_id.clone(),
        external_urls,
        result: SearchResult {
            id: format!("weibo-{uid}-{weibo_id}"),
            source: Source::provider("weibo"),
            datetime,
            title,
            content,
            links,
            tags: Vec::new(),
            images: Vec::new(),
        },
    })
}

async fn fetch_external_links(client: &Client, value: &str) -> Result<Vec<Link>, ProviderError> {
    let url = Url::parse(value).map_err(|e| ProviderError::Protocol(e.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Ok(Vec::new());
    }
    let body = client
        .get(url)
        .header(header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .map_err(network)?
        .error_for_status()
        .map_err(network)?
        .text()
        .await
        .map_err(network)?;
    Ok(extract_links(&body))
}

fn http_urls(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .filter(|part| part.starts_with("http://") || part.starts_with("https://"))
        .map(|part| {
            part.trim_matches(|c: char| matches!(c, '"' | '\'' | '<' | '>' | ')' | '）'))
                .to_owned()
        })
        .collect()
}

async fn fetch_comments(
    client: &Client,
    endpoints: &WeiboEndpoints,
    id: &str,
    cookie: &str,
) -> Result<Vec<String>, ProviderError> {
    let response = client
        .get(endpoints.comments.clone())
        .query(&[
            ("id", id),
            ("mid", id),
            ("max_id", "0"),
            ("max_id_type", "0"),
        ])
        .header(header::USER_AGENT, USER_AGENT)
        .header(header::REFERER, endpoints.mobile_origin.as_str())
        .header(header::COOKIE, cookie)
        .send()
        .await
        .map_err(network)?
        .error_for_status()
        .map_err(network)?;
    parse_comments(&response.text().await.map_err(network)?)
}

pub fn parse_comments(body: &str) -> Result<Vec<String>, ProviderError> {
    let root: serde_json::Value =
        serde_json::from_str(body).map_err(|e| ProviderError::Parse(e.to_string()))?;
    let list = root
        .pointer("/data/data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(list
        .into_iter()
        .take(1)
        .map(|comment| {
            let raw = comment.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let mut text = clean_html(raw);
            for decoded in extract_sina_urls(raw) {
                text.push(' ');
                text.push_str(&decoded);
            }
            text
        })
        .collect())
}

fn extract_sina_urls(text: &str) -> Vec<String> {
    let needle = "https://weibo.cn/sinaurl?u=";
    let mut output = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(needle) {
        rest = &rest[start + needle.len()..];
        let end = rest.find(['"', '&', '<', ' ', '\'']).unwrap_or(rest.len());
        let decoded = percent_encoding::percent_decode_str(&rest[..end])
            .decode_utf8_lossy()
            .into_owned();
        if !decoded.is_empty() {
            output.push(decoded);
        }
        rest = &rest[end..];
    }
    output
}

pub fn normalize_user_id(value: &str) -> Result<String, ProviderError> {
    let value = value.trim();
    let candidate = if value.bytes().all(|b| b.is_ascii_digit()) {
        value.to_owned()
    } else {
        let url = Url::parse(value)
            .map_err(|_| ProviderError::Protocol(format!("invalid Weibo user: {value}")))?;
        if !url
            .host_str()
            .is_some_and(|host| host == "weibo.com" || host.ends_with(".weibo.com"))
        {
            return Err(ProviderError::Protocol(format!(
                "invalid Weibo user host: {value}"
            )));
        }
        let segments = url
            .path_segments()
            .map(|s| s.collect::<Vec<_>>())
            .unwrap_or_default();
        match segments.as_slice() {
            ["u", id, ..] => (*id).to_owned(),
            [id, ..] => (*id).to_owned(),
            _ => String::new(),
        }
    };
    if candidate.is_empty() || !candidate.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ProviderError::Protocol(format!(
            "invalid Weibo user id: {value}"
        )));
    }
    Ok(candidate)
}

pub fn normalize_user_ids<I, S>(values: I) -> Result<Vec<String>, ProviderError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for value in values {
        for part in value.as_ref().split(',') {
            let id = normalize_user_id(part)?;
            if seen.insert(id.clone()) {
                output.push(id);
            }
        }
    }
    Ok(output)
}

fn clean_html(value: &str) -> String {
    let fragment = scraper::Html::parse_fragment(value);
    fragment
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_datetime(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_str(value, "%a %b %d %H:%M:%S %z %Y")
        .ok()
        .map(|v| v.with_timezone(&Utc))
}

fn value_string(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|v| v.to_string()))
        .unwrap_or_default()
}

fn merge_unique_links(destination: &mut Vec<Link>, incoming: Vec<Link>) {
    let mut seen = destination
        .iter()
        .map(|l| canonical_url_key(&l.url))
        .collect::<HashSet<_>>();
    destination.extend(
        incoming
            .into_iter()
            .filter(|l| seen.insert(canonical_url_key(&l.url))),
    );
}

fn dedupe_results(results: &mut Vec<SearchResult>) {
    let mut seen = HashSet::new();
    results.retain(|result| seen.insert(result.id.clone()));
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
    fn user_ids_accept_urls_and_dedupe_stably() {
        assert_eq!(
            normalize_user_ids(["123,456", "https://weibo.com/u/123"]).unwrap(),
            ["123", "456"]
        );
        assert!(normalize_user_id("https://example.com/u/123").is_err());
        assert!(normalize_user_id("not-a-user").is_err());
    }

    #[test]
    fn parses_post_body_and_url_struct_links() {
        let posts = parse_search_page(
            include_str!("../../../tests/fixtures/providers/weibo/search.json"),
            "7788",
        )
        .unwrap();
        assert_eq!(posts.len(), 2);
        assert_eq!(posts[0].result.id, "weibo-7788-501234567890");
        assert_eq!(
            posts[0].result.links[0].cloud_type,
            crate::core::CloudType::Quark
        );
        assert_eq!(posts[1].result.links[0].password.as_deref(), Some("a1b2"));
        assert!(
            parse_search_page(
                include_str!("../../../tests/fixtures/providers/weibo/malformed.json"),
                "1"
            )
            .is_err()
        );
        assert!(
            parse_search_page(
                include_str!("../../../tests/fixtures/providers/weibo/empty.json"),
                "1"
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn parses_comment_redirect_links() {
        let comments = parse_comments(include_str!(
            "../../../tests/fixtures/providers/weibo/comments.json"
        ))
        .unwrap();
        let links = extract_links(&comments[0]);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].password.as_deref(), Some("z9y8"));
    }

    #[test]
    fn assignments_dedupe_targets_and_balance_profiles() {
        let profile = |cookie: &str, ids: &[&str]| WeiboProfile {
            active: true,
            cookie: cookie.into(),
            user_ids: ids.iter().map(|v| (*v).into()).collect(),
            login_at: None,
            expires_at: None,
        };
        let profiles = vec![
            ("a".into(), profile("a=1", &["1", "2", "3"])),
            ("b".into(), profile("b=1", &["1", "2", "3"])),
        ];
        let tasks = assign_users(&profiles, 0);
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks.iter().filter(|t| t.cookie == "a=1").count(), 2);
        assert_eq!(tasks.iter().filter(|t| t.cookie == "b=1").count(), 1);
    }

    #[test]
    fn profile_roundtrips_through_state_store() {
        let temp = tempfile::tempdir().unwrap();
        let provider = WeiboProvider::new(StateStore::with_cipher(temp.path(), None));
        provider.save_login("main", "SUB=x; SUBP=y").unwrap();
        assert_eq!(
            provider
                .configure_users("main", ["123", "https://weibo.com/u/456", "123"])
                .unwrap(),
            ["123", "456"]
        );
        let state = provider.load_profile("main").unwrap().unwrap();
        assert!(state.is_ready(Utc::now()));
        assert_eq!(state.user_ids, ["123", "456"]);
    }
}
