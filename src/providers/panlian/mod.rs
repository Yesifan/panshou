use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    core::{CloudType, Link, ProviderError, SearchResult, Source},
    http::Session,
    providers::{AuthKind, KeywordFilterMode, Provider, ProviderMeta, SearchContext},
    state::{StateError, StateStore},
};

const DEFAULT_BASE: &str = "https://pinglian.lol";
const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36";
const JSON_ACCEPT: &str = "application/json, text/plain, */*";
const HTML_ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
const ACCEPT_LANGUAGE: &str = "zh-TW,zh;q=0.9,zh-CN;q=0.8,en;q=0.7";

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse(DEFAULT_BASE).expect("static URL"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanlianProfile {
    pub username: String,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub cookie: String,
    #[serde(default = "active")]
    pub status: String,
}
fn active() -> String {
    "active".into()
}

#[derive(Debug, Clone)]
pub struct PanlianProvider {
    endpoints: Endpoints,
    store: StateStore,
    profiles: Arc<RwLock<Vec<(String, PanlianProfile)>>>,
    blocked: Arc<HashSet<CloudType>>,
}

impl PanlianProvider {
    pub fn load(
        store: StateStore,
        endpoints: Endpoints,
        blocked: impl IntoIterator<Item = CloudType>,
    ) -> Result<Self, StateError> {
        let mut profiles = Vec::new();
        for name in store.list_profiles("panlian")? {
            if let Some(profile) = store.load::<PanlianProfile>("panlian", &name)?
                && profile.status == "active"
                && !profile.cookie.is_empty()
            {
                profiles.push((name, profile));
            }
        }
        Ok(Self {
            endpoints,
            store,
            profiles: Arc::new(RwLock::new(profiles)),
            blocked: Arc::new(blocked.into_iter().collect()),
        })
    }

    pub fn profile_names(&self) -> Result<Vec<String>, ProviderError> {
        Ok(self
            .profiles
            .read()
            .map_err(|_| ProviderError::Unavailable("profile lock poisoned".into()))?
            .iter()
            .map(|(name, _)| name.clone())
            .collect())
    }

    pub fn logout(&self, profile: &str) -> Result<bool, ProviderError> {
        let deleted = self.store.delete("panlian", profile).map_err(state)?;
        self.profiles
            .write()
            .map_err(|_| ProviderError::Unavailable("profile lock poisoned".into()))?
            .retain(|(name, _)| name != profile);
        Ok(deleted)
    }

    pub async fn login(
        &self,
        session: &Session,
        profile: &str,
        username: &str,
        password: &str,
        remember_credentials: bool,
    ) -> Result<PanlianProfile, ProviderError> {
        if username.trim().is_empty() || password.is_empty() {
            return Err(ProviderError::Protocol(
                "username and password are required".into(),
            ));
        }
        let pre = self
            .endpoints
            .base
            .join("api/get_types.php")
            .map_err(protocol)?;
        response_ok(
            panlian_headers(
                session.client().get(pre),
                &self.endpoints.base,
                "",
                self.endpoints
                    .base
                    .join("all-videos.php")
                    .map_err(protocol)?
                    .as_str(),
                false,
            )
            .send()
            .await,
        )?;
        let login_url = self
            .endpoints
            .base
            .join("api/login.php")
            .map_err(protocol)?;
        let login_referer = self
            .endpoints
            .base
            .join("pages/login.php")
            .map_err(protocol)?;
        let response = panlian_headers(
            session.client().post(login_url),
            &self.endpoints.base,
            "",
            login_referer.as_str(),
            false,
        )
        .form(&[
            ("username", username.trim()),
            ("password", password),
            ("remember", "on"),
        ])
        .send()
        .await
        .map_err(network)?;
        let status = response.status();
        let body = response.text().await.map_err(network)?;
        if !status.is_success() {
            return Err(ProviderError::Network(format!("HTTP {status}")));
        }
        let login: LoginResponse = serde_json::from_str(&body).map_err(parse)?;
        if !login.success {
            return Err(ProviderError::AuthRequired);
        }
        let cookie = session
            .cookie_header(&self.endpoints.base)
            .unwrap_or_default();
        if cookie.is_empty() {
            return Err(ProviderError::Protocol(
                "login succeeded without a session cookie".into(),
            ));
        }
        let saved = PanlianProfile {
            username: username.trim().into(),
            password: remember_credentials.then(|| password.to_owned()),
            cookie,
            status: active(),
        };
        self.store.save("panlian", profile, &saved).map_err(state)?;
        let mut profiles = self
            .profiles
            .write()
            .map_err(|_| ProviderError::Unavailable("profile lock poisoned".into()))?;
        profiles.retain(|(name, _)| name != profile);
        profiles.push((profile.into(), saved.clone()));
        Ok(saved)
    }

    async fn relogin_with_context(
        &self,
        ctx: &SearchContext,
        profile_name: &str,
        profile: &PanlianProfile,
        password: &str,
    ) -> Result<PanlianProfile, ProviderError> {
        let pre = self
            .endpoints
            .base
            .join("api/get_types.php")
            .map_err(protocol)?;
        let pre_referer = self
            .endpoints
            .base
            .join("all-videos.php")
            .map_err(protocol)?;
        let pre_response = panlian_headers(
            ctx.client.get(pre),
            &self.endpoints.base,
            "",
            pre_referer.as_str(),
            false,
        )
        .send()
        .await
        .map_err(network)?;
        if !pre_response.status().is_success() {
            return Err(ProviderError::Network(format!(
                "HTTP {}",
                pre_response.status()
            )));
        }
        let mut cookies = collect_set_cookies(pre_response.headers());
        let cookie = render_cookies(&cookies);
        let login_url = self
            .endpoints
            .base
            .join("api/login.php")
            .map_err(protocol)?;
        let login_referer = self
            .endpoints
            .base
            .join("pages/login.php")
            .map_err(protocol)?;
        let response = panlian_headers(
            ctx.client.post(login_url),
            &self.endpoints.base,
            &cookie,
            login_referer.as_str(),
            false,
        )
        .form(&[
            ("username", profile.username.as_str()),
            ("password", password),
            ("remember", "on"),
        ])
        .send()
        .await
        .map_err(network)?;
        for (name, value) in collect_set_cookies(response.headers()) {
            cookies.insert(name, value);
        }
        let login: LoginResponse = response.json().await.map_err(network)?;
        if !login.success {
            return Err(ProviderError::AuthRequired);
        }
        let mut saved = profile.clone();
        saved.cookie = render_cookies(&cookies);
        if saved.cookie.is_empty() {
            return Err(ProviderError::AuthRequired);
        }
        self.store
            .save("panlian", profile_name, &saved)
            .map_err(state)?;
        let mut profiles = self
            .profiles
            .write()
            .map_err(|_| ProviderError::Unavailable("profile lock poisoned".into()))?;
        profiles.retain(|(name, _)| name != profile_name);
        profiles.push((profile_name.into(), saved.clone()));
        Ok(saved)
    }

    async fn search_profile(
        &self,
        ctx: &SearchContext,
        profile: &PanlianProfile,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut videos_url = self
            .endpoints
            .base
            .join("api/get_videos.php")
            .map_err(protocol)?;
        videos_url
            .query_pairs_mut()
            .append_pair("wd", query)
            .append_pair("pg", "1");
        let videos: VideoResponse =
            get_json(ctx, videos_url, &profile.cookie, &self.endpoints.base).await?;
        if videos.code == -1 || videos.msg.contains("登录") {
            return Err(ProviderError::AuthRequired);
        }
        if videos.code != 1 {
            return Err(ProviderError::Protocol(videos.msg));
        }
        let mut output = Vec::new();
        for video in videos.list.into_iter().take(10) {
            let mut link_url = self
                .endpoints
                .base
                .join("api/search_pan_links.php")
                .map_err(protocol)?;
            link_url
                .query_pairs_mut()
                .append_pair("keyword", query)
                .append_pair("vod_id", &video.vod_id.to_string());
            let mut response: PanResponse =
                get_json(ctx, link_url, &profile.cookie, &self.endpoints.base).await?;
            if !response.success && response.message.contains("登录") {
                return Err(ProviderError::AuthRequired);
            }
            if !response.success {
                continue;
            }
            self.resolve_tokens(ctx, &profile.cookie, &mut response.data)
                .await;
            let links = flatten_groups(&response.data, &self.blocked);
            if links.is_empty() {
                continue;
            }
            let datetime = links.iter().filter_map(|link| link.datetime).max();
            output.push(SearchResult {
                id: format!("panlian-{}", video.vod_id),
                source: Source::provider("panlian"),
                datetime,
                title: video.vod_name.trim().into(),
                content: video.content(),
                links,
                tags: vec![video.type_name, "panlian".into()],
                images: (!video.vod_pic.trim().is_empty())
                    .then_some(video.vod_pic)
                    .into_iter()
                    .collect(),
            });
        }
        output.sort_by_key(|result| std::cmp::Reverse(result.datetime));
        Ok(output)
    }

    async fn resolve_tokens(
        &self,
        ctx: &SearchContext,
        cookie: &str,
        groups: &mut HashMap<String, PanGroup>,
    ) {
        for group in groups.values_mut() {
            for item in &mut group.links {
                let candidate = if is_real_url(&item.url) {
                    continue;
                } else if !item.token.trim().is_empty() {
                    item.token.trim()
                } else {
                    item.url.trim()
                };
                if let Ok(url) = self.resolve_token(ctx, cookie, candidate).await {
                    item.url = url;
                }
            }
        }
    }

    async fn resolve_token(
        &self,
        ctx: &SearchContext,
        cookie: &str,
        token: &str,
    ) -> Result<String, ProviderError> {
        let api = self
            .endpoints
            .base
            .join("api/resolve_token.php")
            .map_err(protocol)?;
        let video_referer = self
            .endpoints
            .base
            .join("pages/video.php")
            .map_err(protocol)?;
        if let Ok(response) = panlian_headers(
            ctx.client.post(api),
            &self.endpoints.base,
            cookie,
            video_referer.as_str(),
            false,
        )
        .json(&serde_json::json!({"token":token}))
        .send()
        .await
            && let Ok(value) = response.json::<ResolveResponse>().await
            && value.success
            && is_real_url(&value.url)
        {
            return Ok(decode_url(&value.url));
        }
        let mut jump = self.endpoints.base.join("api/go.php").map_err(protocol)?;
        jump.query_pairs_mut().append_pair("t", token);
        let response = panlian_headers(
            ctx.client.get(jump),
            &self.endpoints.base,
            cookie,
            video_referer.as_str(),
            true,
        )
        .send()
        .await
        .map_err(network)?;
        let final_url = decode_url(response.url().as_str());
        if response.url().origin() != self.endpoints.base.origin() && is_real_url(&final_url) {
            return Ok(final_url);
        }
        if let Some(location) = response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(decode_url)
            .filter(|v| is_real_url(v))
        {
            return Ok(location);
        }
        let body = response.text().await.map_err(network)?;
        extract_jump_url(&body)
            .ok_or_else(|| ProviderError::Parse("panlian token did not resolve".into()))
    }
}

#[async_trait]
impl Provider for PanlianProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta {
            name: "panlian",
            priority: 3,
            requires_auth: true,
            auth_kind: AuthKind::Password,
            keyword_filter: KeywordFilterMode::Core,
        }
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let profiles = self
            .profiles
            .read()
            .map_err(|_| ProviderError::Unavailable("profile lock poisoned".into()))?
            .clone();
        if profiles.is_empty() {
            return Err(ProviderError::AuthRequired);
        }
        let mut last = None;
        for (name, profile) in profiles {
            let first = self.search_profile(ctx, &profile, query).await;
            let result = if matches!(&first, Err(ProviderError::AuthRequired)) {
                if let Some(password) = profile.password.as_deref() {
                    match self
                        .relogin_with_context(ctx, &name, &profile, password)
                        .await
                    {
                        Ok(refreshed) => self.search_profile(ctx, &refreshed, query).await,
                        Err(error) => Err(error),
                    }
                } else {
                    first
                }
            } else {
                first
            };
            match result {
                Ok(v) => return Ok(v),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or(ProviderError::AuthRequired))
    }
}

#[derive(Deserialize)]
struct LoginResponse {
    success: bool,
}
#[derive(Deserialize)]
struct VideoResponse {
    code: i64,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    list: Vec<Video>,
}
#[derive(Deserialize)]
struct Video {
    vod_id: i64,
    #[serde(default)]
    vod_name: String,
    #[serde(default)]
    vod_pic: String,
    #[serde(default)]
    vod_remarks: String,
    #[serde(default)]
    vod_year: String,
    #[serde(default)]
    vod_area: String,
    #[serde(default)]
    vod_lang: String,
    #[serde(default)]
    type_name: String,
    #[serde(default)]
    vod_actor: String,
    #[serde(default)]
    vod_director: String,
    #[serde(default)]
    vod_content: String,
}
impl Video {
    fn content(&self) -> String {
        [
            self.type_name.as_str(),
            self.vod_remarks.as_str(),
            self.vod_year.as_str(),
            self.vod_area.as_str(),
            self.vod_lang.as_str(),
            self.vod_actor.as_str(),
            self.vod_director.as_str(),
            self.vod_content.as_str(),
        ]
        .into_iter()
        .filter(|v| !v.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" / ")
    }
}
#[derive(Deserialize)]
struct PanResponse {
    success: bool,
    #[serde(default)]
    message: String,
    #[serde(default)]
    data: HashMap<String, PanGroup>,
}
#[derive(Deserialize)]
struct PanGroup {
    #[serde(default)]
    name: String,
    #[serde(default)]
    links: Vec<PanItem>,
}
#[derive(Deserialize)]
struct PanItem {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    password: String,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    time: String,
}
#[derive(Deserialize)]
struct ResolveResponse {
    success: bool,
    #[serde(default)]
    url: String,
}

async fn get_json<T: for<'de> Deserialize<'de>>(
    ctx: &SearchContext,
    url: Url,
    cookie: &str,
    base: &Url,
) -> Result<T, ProviderError> {
    let referer = base.join("all-videos.php").map_err(protocol)?;
    let r = tokio::time::timeout(
        ctx.timeout,
        panlian_headers(ctx.client.get(url), base, cookie, referer.as_str(), false).send(),
    )
    .await
    .map_err(|_| ProviderError::Timeout)?
    .map_err(network)?;
    if r.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(ProviderError::AuthRequired);
    }
    let text = r.text().await.map_err(network)?;
    serde_json::from_str(&text).map_err(parse)
}
fn panlian_headers(
    request: reqwest::RequestBuilder,
    base: &Url,
    cookie: &str,
    referer: &str,
    html: bool,
) -> reqwest::RequestBuilder {
    let request = request
        .header(reqwest::header::USER_AGENT, BROWSER_UA)
        .header(
            reqwest::header::ACCEPT,
            if html { HTML_ACCEPT } else { JSON_ACCEPT },
        )
        .header(reqwest::header::ACCEPT_LANGUAGE, ACCEPT_LANGUAGE)
        .header(reqwest::header::ORIGIN, base.origin().ascii_serialization())
        .header(reqwest::header::REFERER, referer)
        .header("x-requested-with", "XMLHttpRequest");
    if cookie.is_empty() {
        request
    } else {
        request.header(reqwest::header::COOKIE, cookie)
    }
}
fn response_ok(response: Result<reqwest::Response, reqwest::Error>) -> Result<(), ProviderError> {
    let response = response.map_err(network)?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(ProviderError::Network(format!(
            "HTTP {}",
            response.status()
        )))
    }
}
fn network(e: reqwest::Error) -> ProviderError {
    ProviderError::Network(e.to_string())
}
fn parse(e: serde_json::Error) -> ProviderError {
    ProviderError::Parse(e.to_string())
}
fn protocol(e: url::ParseError) -> ProviderError {
    ProviderError::Protocol(e.to_string())
}
fn state(e: StateError) -> ProviderError {
    ProviderError::Unavailable(e.to_string())
}
fn decode_url(v: &str) -> String {
    v.trim().replace("\\/", "/").replace("&amp;", "&")
}
fn collect_set_cookies(headers: &reqwest::header::HeaderMap) -> HashMap<String, String> {
    let mut cookies = HashMap::new();
    for header in headers.get_all(reqwest::header::SET_COOKIE).iter() {
        if let Ok(value) = header.to_str()
            && let Some((name, value)) = value.split(';').next().and_then(|v| v.split_once('='))
        {
            cookies.insert(name.trim().into(), value.trim().into());
        }
    }
    cookies
}
fn render_cookies(cookies: &HashMap<String, String>) -> String {
    let mut entries = cookies.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    entries
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}
fn extract_jump_url(body: &str) -> Option<String> {
    let patterns = [
        r#"id=["']jumpBtn["'][^>]*href=["']([^"']+)"#,
        r#"href=["']([^"']+)["'][^>]*id=["']jumpBtn"#,
        r#"(?:targetUrl|(?:window\.)?location\.href)\s*=\s*["']([^"']+)"#,
    ];
    patterns
        .iter()
        .find_map(|p| {
            Regex::new(p)
                .ok()?
                .captures(body)?
                .get(1)
                .map(|m| decode_url(m.as_str()))
        })
        .filter(|v| is_real_url(v))
}
fn is_real_url(v: &str) -> bool {
    let v = v.trim().to_ascii_lowercase();
    ["http://", "https://", "magnet:", "ed2k:", "thunder:"]
        .iter()
        .any(|p| v.starts_with(p))
}
fn normalize_kind(kind: &str, url: &str) -> CloudType {
    let kind = kind.trim().to_ascii_lowercase();
    match kind.as_str() {
        "夸克" | "夸克网盘" | "quark" => CloudType::Quark,
        "百度" | "百度网盘" | "baidu" => CloudType::Baidu,
        "uc" | "uc网盘" => CloudType::Uc,
        "迅雷" | "xunlei" => CloudType::Xunlei,
        "123" | "123pan" | "a123" => CloudType::Pan123,
        "天翼" | "a189" => CloudType::Tianyi,
        "115" | "a115" => CloudType::One15,
        "阿里" | "aliyun" | "ali" => CloudType::Aliyun,
        "pikpak" => CloudType::Pikpak,
        "磁力" | "magnet" => CloudType::Magnet,
        _ => crate::core::detect_cloud_type(url),
    }
}
pub fn normalize_pan_url(raw: &str, password: &str, kind: CloudType) -> String {
    let mut clean = raw.trim().trim_end_matches('#').to_owned();
    if kind == CloudType::Pan123
        && let Ok(url) = Url::parse(&clean)
    {
        let host = url.host_str().unwrap_or("").to_ascii_lowercase();
        let parts = url.path().trim_matches('/').split('/').collect::<Vec<_>>();
        if host.starts_with("share.") && parts.len() == 2 && parts[0].eq_ignore_ascii_case("123pan")
        {
            clean = format!("https://123865.com/s/{}", parts[1]);
            let pwd = ["pwd", "pass", "password", "code"].iter().find_map(|k| {
                url.query_pairs()
                    .find(|(key, _)| key == *k)
                    .map(|(_, v)| v.into_owned())
            });
            if let Some(pwd) = pwd {
                clean.push_str(&format!("?pwd={pwd}"));
            }
        }
    }
    if password.trim().is_empty() || clean.contains("pwd=") || clean.contains("password=") {
        return clean;
    }
    match kind {
        CloudType::Baidu | CloudType::Xunlei | CloudType::Pan123 => format!(
            "{clean}{}pwd={}",
            if clean.contains('?') { "&" } else { "?" },
            password.trim()
        ),
        CloudType::One15 => format!(
            "{clean}{}password={}",
            if clean.contains('?') { "&" } else { "?" },
            password.trim()
        ),
        _ => clean,
    }
}
fn parse_time(v: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(v)
        .ok()
        .map(|v| v.with_timezone(&Utc))
        .or_else(|| {
            NaiveDateTime::parse_from_str(v, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|v| v.and_utc())
        })
}
fn flatten_groups(groups: &HashMap<String, PanGroup>, blocked: &HashSet<CloudType>) -> Vec<Link> {
    let order = [
        CloudType::Quark,
        CloudType::Uc,
        CloudType::Baidu,
        CloudType::Xunlei,
        CloudType::Pan123,
        CloudType::Tianyi,
        CloudType::One15,
        CloudType::Aliyun,
        CloudType::Guangya,
        CloudType::Mobile,
        CloudType::Pikpak,
        CloudType::Magnet,
        CloudType::Others,
    ];
    let mut links = Vec::new();
    let mut seen = HashSet::new();
    for kind in order {
        if blocked.contains(&kind) {
            continue;
        }
        for (key, group) in groups {
            let group_kind = normalize_kind(
                if group.name.is_empty() {
                    key
                } else {
                    &group.name
                },
                "",
            );
            if group_kind != kind {
                continue;
            }
            let mut items = group.links.iter().collect::<Vec<_>>();
            items.sort_by_key(|i| std::cmp::Reverse(parse_time(&i.time)));
            for item in items {
                let item_kind = normalize_kind(
                    if item.kind.is_empty() {
                        key
                    } else {
                        &item.kind
                    },
                    &item.url,
                );
                let url = normalize_pan_url(&item.url, &item.password, item_kind);
                if !is_real_url(&url) || !seen.insert((url.clone(), item.password.clone())) {
                    continue;
                }
                links.push(Link {
                    cloud_type: item_kind,
                    url,
                    password: (!item.password.trim().is_empty())
                        .then(|| item.password.trim().into()),
                    datetime: parse_time(&item.time),
                    work_title: (!item.title.trim().is_empty()).then(|| item.title.trim().into()),
                });
                if links.len() >= 200 {
                    return links;
                }
            }
        }
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_legacy_123() {
        assert_eq!(
            normalize_pan_url(
                "https://share.123865.com/123pan/Ab_C?code=x9y8",
                "",
                CloudType::Pan123
            ),
            "https://123865.com/s/Ab_C?pwd=x9y8"
        );
    }
    #[test]
    fn jump_fallback() {
        assert_eq!(
            extract_jump_url(r#"<a id="jumpBtn" href="https:\/\/pan.quark.cn\/s\/abc">go</a>"#)
                .as_deref(),
            Some("https://pan.quark.cn/s/abc")
        );
    }
    #[test]
    fn password_is_appended_for_supported_clouds() {
        assert_eq!(
            normalize_pan_url("https://pan.baidu.com/s/abc", "a1b2", CloudType::Baidu),
            "https://pan.baidu.com/s/abc?pwd=a1b2"
        );
        assert_eq!(
            normalize_pan_url("https://115.com/s/abc", "a1b2", CloudType::One15),
            "https://115.com/s/abc?password=a1b2"
        );
    }

    #[test]
    fn blocked_clouds_and_duplicate_links_are_filtered() {
        let groups: HashMap<String, PanGroup> = serde_json::from_str(
            r#"{"quark":{"name":"夸克","links":[{"url":"https://pan.quark.cn/s/a"}]},"baidu":{"name":"百度","links":[{"url":"https://pan.baidu.com/s/b"},{"url":"https://pan.baidu.com/s/b"}]}}"#,
        )
        .unwrap();
        let links = flatten_groups(&groups, &[CloudType::Quark].into_iter().collect());
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].cloud_type, CloudType::Baidu);
    }
    #[test]
    fn fixture_matrix_parses() {
        assert!(
            serde_json::from_str::<LoginResponse>(include_str!(
                "../../../tests/fixtures/providers/panlian/login.json"
            ))
            .unwrap()
            .success
        );
        assert_eq!(
            serde_json::from_str::<VideoResponse>(include_str!(
                "../../../tests/fixtures/providers/panlian/search.json"
            ))
            .unwrap()
            .list
            .len(),
            1
        );
        let token: ResolveResponse = serde_json::from_str(include_str!(
            "../../../tests/fixtures/providers/panlian/token.json"
        ))
        .unwrap();
        assert!(token.success && is_real_url(&token.url));
        assert_eq!(
            extract_jump_url(include_str!(
                "../../../tests/fixtures/providers/panlian/jump.html"
            ))
            .as_deref(),
            Some("https://pan.quark.cn/s/abc")
        );
        let links: PanResponse = serde_json::from_str(include_str!(
            "../../../tests/fixtures/providers/panlian/links.json"
        ))
        .unwrap();
        let flattened = flatten_groups(&links.data, &HashSet::new());
        assert_eq!(flattened.len(), 2);
        assert_eq!(flattened[1].url, "https://123865.com/s/Ab_C?pwd=x9y8");
    }

    #[test]
    fn request_headers_match_browser_protocol() {
        let base = Url::parse("https://pinglian.test/").unwrap();
        let request = panlian_headers(
            reqwest::Client::new().get(base.join("api/get_videos.php").unwrap()),
            &base,
            "PHPSESSID=secret",
            "https://pinglian.test/all-videos.php",
            false,
        )
        .build()
        .unwrap();
        assert_eq!(request.headers()[reqwest::header::USER_AGENT], BROWSER_UA);
        assert_eq!(request.headers()[reqwest::header::ACCEPT], JSON_ACCEPT);
        assert_eq!(
            request.headers()[reqwest::header::ACCEPT_LANGUAGE],
            ACCEPT_LANGUAGE
        );
        assert_eq!(
            request.headers()[reqwest::header::ORIGIN],
            "https://pinglian.test"
        );
        assert_eq!(
            request.headers()[reqwest::header::REFERER],
            "https://pinglian.test/all-videos.php"
        );
        assert_eq!(request.headers()["x-requested-with"], "XMLHttpRequest");
        assert_eq!(
            request.headers()[reqwest::header::COOKIE],
            "PHPSESSID=secret"
        );
    }
}
