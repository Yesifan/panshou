mod challenge;

pub use challenge::{
    Challenge, ChallengeError, Solution, solve_inline_pow, solve_legacy, solve_pow,
    solve_remote_pow,
};

use async_trait::async_trait;
use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    sync::{Arc, LazyLock, RwLock},
    time::Duration,
};
use url::Url;

use crate::{
    core::{Link, ProviderError, SearchResult, Source},
    http::{ClientOptions, HttpClientFactory, Session},
    providers::{AuthKind, KeywordFilterMode, Provider, ProviderMeta, SearchContext},
    state::{StateError, StateStore},
};

const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36";
const BROWSER_ACCEPT: &str = "text/html,application/xhtml+xml,application/json;q=0.9,*/*;q=0.8";
const ACCEPT_LANGUAGE: &str = "zh-CN,zh;q=0.9,en;q=0.8";

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Endpoints {
    pub fn parse(raw: &str) -> Result<Self, ProviderError> {
        let mut base = Url::parse(raw).map_err(protocol)?;
        if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
            return Err(ProviderError::Protocol(
                "gying base_url must be HTTP(S)".into(),
            ));
        }
        base.set_query(None);
        base.set_fragment(None);
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path().trim_end_matches('/')))
        }
        Ok(Self { base })
    }
}
impl Default for Endpoints {
    fn default() -> Self {
        Self::parse("https://www.xn--wcv59z.com").expect("static URL")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GyingProfile {
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
#[derive(Clone)]
struct RuntimeProfile {
    name: String,
    data: GyingProfile,
    session: Session,
}
impl std::fmt::Debug for RuntimeProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeProfile")
            .field("name", &self.name)
            .field("username", &self.data.username)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct GyingProvider {
    endpoints: Endpoints,
    store: StateStore,
    factory: HttpClientFactory,
    options: ClientOptions,
    profiles: Arc<RwLock<Vec<RuntimeProfile>>>,
}
impl GyingProvider {
    pub fn load(
        store: StateStore,
        endpoints: Endpoints,
        factory: HttpClientFactory,
        options: ClientOptions,
    ) -> Result<Self, StateError> {
        let mut profiles = Vec::new();
        for name in store.list_profiles("gying")? {
            if let Some(data) = store.load::<GyingProfile>("gying", &name)?
                && data.status == "active"
                && !data.cookie.is_empty()
            {
                let session = factory.session(&options).map_err(|e| StateError::Io {
                    path: "gying session".into(),
                    source: std::io::Error::other(e.to_string()),
                })?;
                restore_cookie(&session, &endpoints.base, &data.cookie);
                profiles.push(RuntimeProfile {
                    name,
                    data,
                    session,
                })
            }
        }
        Ok(Self {
            endpoints,
            store,
            factory,
            options,
            profiles: Arc::new(RwLock::new(profiles)),
        })
    }

    pub fn profile_names(&self) -> Result<Vec<String>, ProviderError> {
        Ok(self
            .profiles
            .read()
            .map_err(|_| ProviderError::Unavailable("profile lock poisoned".into()))?
            .iter()
            .map(|profile| profile.name.clone())
            .collect())
    }

    pub fn logout(&self, profile: &str) -> Result<bool, ProviderError> {
        let deleted = self.store.delete("gying", profile).map_err(state)?;
        self.profiles
            .write()
            .map_err(|_| ProviderError::Unavailable("profile lock poisoned".into()))?
            .retain(|candidate| candidate.name != profile);
        Ok(deleted)
    }

    pub async fn login(
        &self,
        profile: &str,
        username: &str,
        password: &str,
        remember_credentials: bool,
    ) -> Result<GyingProfile, ProviderError> {
        if username.trim().is_empty() || password.is_empty() {
            return Err(ProviderError::Protocol(
                "username and password are required".into(),
            ));
        }
        let session = self
            .factory
            .session(&self.options)
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        let login_page = self.endpoints.base.join("user/login/").map_err(protocol)?;
        self.request_challenged(&session, reqwest::Method::GET, login_page, None)
            .await?;
        let login_api = self.endpoints.base.join("user/login").map_err(protocol)?;
        let form = [
            ("code", ""),
            ("siteid", "1"),
            ("dosubmit", "1"),
            ("cookietime", "10506240"),
            ("username", username.trim()),
            ("password", password),
        ];
        let response = self
            .request_challenged(&session, reqwest::Method::POST, login_api, Some(&form))
            .await?;
        let value: serde_json::Value = serde_json::from_slice(&response).map_err(parse)?;
        let code = value
            .get("code")
            .and_then(|v| v.as_i64())
            .or_else(|| value.get("code").and_then(|v| v.as_str()?.parse().ok()))
            .unwrap_or_default();
        if code != 200 {
            return Err(ProviderError::AuthRequired);
        }
        let warm = self.endpoints.base.join("mv/wkMn").map_err(protocol)?;
        let _ = self
            .request_challenged(&session, reqwest::Method::GET, warm, None)
            .await;
        let cookie = session
            .cookie_header(&self.endpoints.base)
            .unwrap_or_default();
        if cookie.is_empty() {
            return Err(ProviderError::Protocol(
                "login succeeded without cookies".into(),
            ));
        }
        let data = GyingProfile {
            username: username.trim().into(),
            password: remember_credentials.then(|| password.into()),
            cookie,
            status: active(),
        };
        self.store.save("gying", profile, &data).map_err(state)?;
        let mut profiles = self
            .profiles
            .write()
            .map_err(|_| ProviderError::Unavailable("profile lock poisoned".into()))?;
        profiles.retain(|p| p.name != profile);
        profiles.push(RuntimeProfile {
            name: profile.into(),
            data: data.clone(),
            session,
        });
        Ok(data)
    }

    async fn request_challenged(
        &self,
        session: &Session,
        method: reqwest::Method,
        url: Url,
        form: Option<&[(&str, &str)]>,
    ) -> Result<Vec<u8>, ProviderError> {
        for attempt in 0..2 {
            let referer = gying_referer(&self.endpoints.base, &method, &url)?;
            let mut request = gying_headers(
                session.client().request(method.clone(), url.clone()),
                &self.endpoints.base,
                Some(referer.as_str()),
                method == reqwest::Method::POST,
            );
            if let Some(form) = form {
                request = request.form(form)
            }
            let response = request.send().await.map_err(network)?;
            let status = response.status();
            let body = response.bytes().await.map_err(network)?.to_vec();
            if !is_challenge(&body) {
                if status == reqwest::StatusCode::FORBIDDEN {
                    return Err(ProviderError::AuthRequired);
                }
                if !status.is_success() {
                    return Err(ProviderError::Network(format!("HTTP {status}")));
                }
                return Ok(body);
            }
            if attempt == 1 {
                return Err(ProviderError::Blocked);
            }
            self.solve_challenge(session, &url, &body).await?
        }
        Err(ProviderError::Blocked)
    }

    async fn solve_challenge(
        &self,
        session: &Session,
        request_url: &Url,
        body: &[u8],
    ) -> Result<(), ProviderError> {
        let text = String::from_utf8_lossy(body);
        if let Some(raw) = capture_json(&text) {
            let challenge: Challenge = serde_json::from_str(raw).map_err(parse)?;
            let solution = if !challenge.modulus.is_empty() {
                solve_inline_pow(&challenge, Duration::from_secs(3))
                    .await
                    .map_err(challenge_error)?
            } else {
                solve_legacy(&challenge).map_err(challenge_error)?
            };
            return submit_solution(session, &self.endpoints.base, request_url, solution).await;
        }
        let pow_url = self.endpoints.base.join("res/pow").map_err(protocol)?;
        let response = gying_headers(
            session.client().get(pow_url.clone()),
            &self.endpoints.base,
            Some(request_url.as_str()),
            false,
        )
        .send()
        .await
        .map_err(network)?;
        let challenge: Challenge = response.json().await.map_err(network)?;
        let solution = solve_remote_pow(&challenge).map_err(challenge_error)?;
        submit_solution(session, &self.endpoints.base, &pow_url, solution).await
    }

    async fn search_profile(
        &self,
        profile: &RuntimeProfile,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self.endpoints.base.join("search").map_err(protocol)?;
        url.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("type", "0")
            .append_pair("mode", "2");
        let body = tokio::time::timeout(
            ctx.timeout,
            self.request_challenged(&profile.session, reqwest::Method::GET, url, None),
        )
        .await
        .map_err(|_| ProviderError::Timeout)??;
        let text = String::from_utf8_lossy(&body);
        if text.contains("user/login") && !text.contains("_obj.search") {
            return Err(ProviderError::AuthRequired);
        }
        let raw = Regex::new(r"(?s)_obj\s*\.\s*search\s*=\s*(\{.*?\})\s*;")
            .expect("regex")
            .captures(&text)
            .and_then(|c| c.get(1))
            .ok_or_else(|| ProviderError::Parse("gying search payload missing".into()))?
            .as_str();
        let search: SearchData = serde_json::from_str(raw).map_err(parse)?;
        let mut output = Vec::new();
        for index in 0..search.l.i.len() {
            let title = search.l.title.get(index).cloned().unwrap_or_default();
            if !title.to_lowercase().contains(&query.to_lowercase()) {
                continue;
            }
            let kind = search.l.d.get(index).map(String::as_str).unwrap_or("mv");
            let id = &search.l.i[index];
            let detail_url = self
                .endpoints
                .base
                .join(&format!("res/downurl/{kind}/{id}"))
                .map_err(protocol)?;
            let body = self
                .request_challenged(&profile.session, reqwest::Method::GET, detail_url, None)
                .await?;
            let detail: DetailData = serde_json::from_slice(&body).map_err(parse)?;
            if detail.code == 403 {
                return Err(ProviderError::AuthRequired);
            }
            let mut links = extract_links(&detail, &title);
            if links.is_empty() {
                continue;
            }
            let year = search.l.year.get(index).copied().unwrap_or_default();
            let display = if year > 0 {
                format!("{title}（{year}）")
            } else {
                title
            };
            for link in &mut links {
                if link.work_title.is_none() {
                    link.work_title = Some(display.clone())
                }
            }
            output.push(SearchResult {
                id: format!("gying-{kind}-{id}"),
                source: Source::provider("gying"),
                datetime: Some(Utc::now()),
                title: display,
                content: search.l.info.get(index).cloned().unwrap_or_default(),
                links,
                tags: (year > 0).then(|| year.to_string()).into_iter().collect(),
                images: vec![],
            })
        }
        Ok(output)
    }
}

#[async_trait]
impl Provider for GyingProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta {
            name: "gying",
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
        let mut all = Vec::new();
        let mut seen = HashSet::new();
        let mut last = None;
        for profile in profiles {
            let first = self.search_profile(&profile, ctx, query).await;
            let result = if matches!(&first, Err(ProviderError::AuthRequired)) {
                if let Some(password) = profile.data.password.as_deref() {
                    match self
                        .login(&profile.name, &profile.data.username, password, true)
                        .await
                    {
                        Ok(_) => {
                            let refreshed = self
                                .profiles
                                .read()
                                .map_err(|_| {
                                    ProviderError::Unavailable("profile lock poisoned".into())
                                })?
                                .iter()
                                .find(|candidate| candidate.name == profile.name)
                                .cloned();
                            match refreshed {
                                Some(refreshed) => {
                                    self.search_profile(&refreshed, ctx, query).await
                                }
                                None => Err(ProviderError::AuthRequired),
                            }
                        }
                        Err(error) => Err(error),
                    }
                } else {
                    first
                }
            } else {
                first
            };
            match result {
                Ok(rows) => {
                    for row in rows {
                        if seen.insert(row.id.clone()) {
                            all.push(row)
                        }
                    }
                }
                Err(e) => last = Some(e),
            }
        }
        if all.is_empty()
            && let Some(error) = last
        {
            return Err(error);
        }
        Ok(all)
    }
}

#[derive(Deserialize)]
struct SearchData {
    l: SearchList,
}
#[derive(Deserialize)]
struct SearchList {
    #[serde(default)]
    title: Vec<String>,
    #[serde(default)]
    year: Vec<i32>,
    #[serde(default)]
    d: Vec<String>,
    #[serde(default)]
    i: Vec<String>,
    #[serde(default)]
    info: Vec<String>,
}
#[derive(Deserialize)]
struct DetailData {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    panlist: PanList,
    #[serde(default)]
    downlist: DownList,
}
#[derive(Default, Deserialize)]
struct PanList {
    #[serde(default)]
    name: Vec<String>,
    #[serde(default)]
    p: Vec<String>,
    #[serde(default)]
    url: Vec<String>,
}
#[derive(Default, Deserialize)]
struct DownList {
    #[serde(default)]
    list: DownloadList,
}
#[derive(Default, Deserialize)]
struct DownloadList {
    #[serde(default)]
    m: Vec<String>,
    #[serde(default)]
    t: Vec<String>,
}
fn extract_links(detail: &DetailData, title: &str) -> Vec<Link> {
    static URL_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"https?://[^\s<>"'（）()]+"#).expect("valid URL regex"));

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (index, raw) in detail.panlist.url.iter().enumerate() {
        let url = URL_RE
            .find(raw)
            .map(|m| m.as_str())
            .unwrap_or(raw)
            .trim()
            .to_owned();
        if url.is_empty() || !seen.insert(url.to_lowercase()) {
            continue;
        }
        let mut link = Link::new(url.clone());
        link.password = extract_password(raw).or_else(|| {
            detail
                .panlist
                .p
                .get(index)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        });
        link.work_title = detail.panlist.name.get(index).map(|v| {
            if v.trim().is_empty() {
                title.into()
            } else {
                format!("{title} - {}", v.trim())
            }
        });
        out.push(link)
    }
    for (index, hash) in detail.downlist.list.m.iter().enumerate() {
        let hash = hash.trim().to_ascii_lowercase();
        if hash.len() != 40
            || !hash.bytes().all(|b| b.is_ascii_hexdigit())
            || !seen.insert(format!("magnet:{hash}"))
        {
            continue;
        }
        let name = detail
            .downlist
            .list
            .t
            .get(index)
            .map(|v| v.trim())
            .unwrap_or("");
        let url = if name.is_empty() {
            format!("magnet:?xt=urn:btih:{hash}")
        } else {
            format!(
                "magnet:?xt=urn:btih:{hash}&dn={}",
                percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC)
            )
        };
        let mut link = Link::new(url);
        link.work_title = Some(if name.is_empty() {
            title.into()
        } else {
            format!("{title} - {name}")
        });
        out.push(link)
    }
    out
}
fn extract_password(v: &str) -> Option<String> {
    Regex::new(
        r"(?i)(?:[?&](?:pwd|password)=|访问码[:：]|提取码[:：]|密码[:：])\s*([A-Za-z0-9]{4,8})",
    )
    .ok()?
    .captures(v)?
    .get(1)
    .map(|m| m.as_str().into())
}
fn capture_json(text: &str) -> Option<&str> {
    Regex::new(r"(?s)const\s+json\s*=\s*(\{.*?\})\s*;\s*const\s+jss\s*=")
        .ok()?
        .captures(text)?
        .get(1)
        .map(|m| m.as_str())
}
fn is_challenge(body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body);
    text.contains("const json") || text.contains("/res/pow") || text.contains("action=verify")
}
async fn submit_solution(
    session: &Session,
    base: &Url,
    url: &Url,
    solution: Solution,
) -> Result<(), ProviderError> {
    let mut form = Vec::<(String, String)>::new();
    match solution {
        Solution::Pow { id, y } => {
            if let Some(id) = id {
                form.push(("action".into(), "verify".into()));
                form.push(("id".into(), id))
            }
            form.push(("y".into(), y))
        }
        Solution::Legacy { id, nonces } => {
            form.push(("action".into(), "verify".into()));
            form.push(("id".into(), id));
            for nonce in nonces {
                form.push(("nonce[]".into(), nonce.to_string()))
            }
        }
    }
    let response = gying_headers(
        session.client().post(url.clone()),
        base,
        Some(url.as_str()),
        true,
    )
    .form(&form)
    .send()
    .await
    .map_err(network)?;
    let value: serde_json::Value = response.json().await.map_err(network)?;
    if value.get("success").and_then(|v| v.as_bool()) == Some(true) {
        Ok(())
    } else {
        Err(ProviderError::Blocked)
    }
}
fn gying_headers(
    request: reqwest::RequestBuilder,
    base: &Url,
    referer: Option<&str>,
    include_origin: bool,
) -> reqwest::RequestBuilder {
    let mut request = request
        .header(reqwest::header::USER_AGENT, BROWSER_UA)
        .header(reqwest::header::ACCEPT, BROWSER_ACCEPT)
        .header(reqwest::header::ACCEPT_LANGUAGE, ACCEPT_LANGUAGE);
    if let Some(referer) = referer {
        request = request.header(reqwest::header::REFERER, referer);
    }
    if include_origin {
        request = request.header(reqwest::header::ORIGIN, base.origin().ascii_serialization());
    }
    request
}
fn gying_referer(base: &Url, method: &reqwest::Method, target: &Url) -> Result<Url, ProviderError> {
    if method == reqwest::Method::POST && target.path().trim_end_matches('/') == "/user/login" {
        base.join("user/login/").map_err(protocol)
    } else {
        Ok(base.clone())
    }
}
fn restore_cookie(session: &Session, origin: &Url, cookie: &str) {
    for pair in cookie.split(';').map(str::trim).filter(|v| v.contains('=')) {
        session.add_cookie(&format!("{pair}; Path=/"), origin)
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
fn challenge_error(e: ChallengeError) -> ProviderError {
    ProviderError::Protocol(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracts_fixture_links() {
        let detail:DetailData=serde_json::from_str(r#"{"code":200,"panlist":{"name":["全集"],"p":["a1b2"],"url":["https://pan.quark.cn/s/abc"],"time":[]},"downlist":{"list":{"m":["0123456789abcdef0123456789abcdef01234567"],"t":["资源"]}}}"#).unwrap();
        let links = extract_links(&detail, "仙逆");
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].password.as_deref(), Some("a1b2"));
    }
    #[test]
    fn base_url_validation() {
        assert!(Endpoints::parse("file:///tmp/x").is_err());
    }
    #[test]
    fn fixture_matrix_parses() {
        let search_text = include_str!("../../../tests/fixtures/providers/gying/search.html");
        let raw = Regex::new(r"(?s)_obj\s*\.\s*search\s*=\s*(\{.*?\})\s*;")
            .unwrap()
            .captures(search_text)
            .unwrap()
            .get(1)
            .unwrap()
            .as_str();
        let search: SearchData = serde_json::from_str(raw).unwrap();
        assert_eq!(search.l.i, ["abc"]);
        let detail: DetailData = serde_json::from_str(include_str!(
            "../../../tests/fixtures/providers/gying/detail.json"
        ))
        .unwrap();
        assert_eq!(extract_links(&detail, "仙逆").len(), 2);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(include_str!(
                "../../../tests/fixtures/providers/gying/login.json"
            ))
            .unwrap()["code"],
            200
        );
        assert!(
            include_str!("../../../tests/fixtures/providers/gying/no_login.html")
                .contains("user/login")
        );
    }
    #[test]
    fn challenge_fixtures_cover_all_paths() {
        let inline: Challenge = serde_json::from_str(
            capture_json(include_str!(
                "../../../tests/fixtures/providers/gying/inline.html"
            ))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(solve_pow(&inline).unwrap(), "4");
        let legacy: Challenge = serde_json::from_str(
            capture_json(include_str!(
                "../../../tests/fixtures/providers/gying/legacy.html"
            ))
            .unwrap(),
        )
        .unwrap();
        assert!(matches!(solve_legacy(&legacy), Ok(Solution::Legacy { .. })));
        let remote: Challenge = serde_json::from_str(include_str!(
            "../../../tests/fixtures/providers/gying/remote.json"
        ))
        .unwrap();
        assert!(matches!(
            solve_remote_pow(&remote),
            Ok(Solution::Pow { id: None, .. })
        ));
    }

    #[test]
    fn request_headers_match_browser_protocol() {
        let base = Url::parse("https://gying.test/").unwrap();
        let request = gying_headers(
            reqwest::Client::new().post(base.join("user/login").unwrap()),
            &base,
            Some("https://gying.test/user/login/"),
            true,
        )
        .build()
        .unwrap();
        assert_eq!(request.headers()[reqwest::header::USER_AGENT], BROWSER_UA);
        assert_eq!(request.headers()[reqwest::header::ACCEPT], BROWSER_ACCEPT);
        assert_eq!(
            request.headers()[reqwest::header::ACCEPT_LANGUAGE],
            ACCEPT_LANGUAGE
        );
        assert_eq!(
            request.headers()[reqwest::header::REFERER],
            "https://gying.test/user/login/"
        );
        assert_eq!(
            request.headers()[reqwest::header::ORIGIN],
            "https://gying.test"
        );

        let get = gying_headers(
            reqwest::Client::new().get(base.join("search").unwrap()),
            &base,
            Some(base.as_str()),
            false,
        )
        .build()
        .unwrap();
        assert!(!get.headers().contains_key(reqwest::header::ORIGIN));
        assert_eq!(
            gying_referer(
                &base,
                &reqwest::Method::POST,
                &base.join("user/login").unwrap()
            )
            .unwrap()
            .as_str(),
            "https://gying.test/user/login/"
        );
    }
}
