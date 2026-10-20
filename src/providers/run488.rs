//! 488.run frontend cookie, incremental search queue, and instance-local cooldown.
use super::UA;
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
    time::Duration,
};
use tokio::{sync::Mutex, time::Instant};
use url::Url;
#[derive(Default)]
struct State {
    cookie: String,
    cookie_at: Option<Instant>,
    cooldown: Option<Instant>,
}
pub struct Run488Provider {
    base: Url,
    state: Mutex<State>,
    refresh: Mutex<()>,
}
impl Default for Run488Provider {
    fn default() -> Self {
        Self::new(Url::parse("https://488.run/").unwrap())
    }
}
impl Run488Provider {
    pub fn new(base: Url) -> Self {
        Self {
            base,
            state: Mutex::new(State::default()),
            refresh: Mutex::new(()),
        }
    }
}
#[derive(Deserialize)]
struct Envelope {
    code: i64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    data: Data,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Data {
    complete: bool,
    blocked: bool,
    list: Vec<Value>,
    items: Vec<Value>,
    search_id: String,
    #[serde(rename = "searchId")]
    search_id_alt: String,
}
fn field<'a>(v: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter()
        .find_map(|k| {
            v.get(*k)
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or("")
}
fn cookies(response: &reqwest::Response) -> Option<String> {
    response
        .cookies()
        .find(|c| c.name() == "yd_frontend" && !c.value().is_empty())
        .map(|c| format!("yd_frontend={}", c.value()))
}
fn remaining(deadline: Instant) -> Duration {
    deadline
        .saturating_duration_since(Instant::now())
        .min(Duration::from_secs(10))
}
impl Run488Provider {
    async fn cookie(
        &self,
        ctx: &SearchContext,
        deadline: Instant,
        force: bool,
        previous: &str,
    ) -> String {
        let _guard = self.refresh.lock().await;
        {
            let s = self.state.lock().await;
            if !s.cookie.is_empty()
                && s.cookie_at
                    .is_some_and(|at| at.elapsed() < Duration::from_secs(6 * 3600))
                && (!force || s.cookie != previous)
            {
                return s.cookie.clone();
            }
        }
        let result = tokio::time::timeout(remaining(deadline), async {
            let r =
                crate::http::execute(ctx.client.get(self.base.clone()).header("User-Agent", UA))
                    .await
                    .ok()?;
            let status = r.status();
            let cookie = cookies(&r);
            crate::http::read_response(r, 4 << 20).await.ok()?;
            if !status.is_success() {
                return None;
            }
            cookie
        })
        .await
        .ok()
        .flatten();
        if let Some(cookie) = result {
            let mut s = self.state.lock().await;
            s.cookie = cookie.clone();
            s.cookie_at = Some(Instant::now());
            cookie
        } else {
            String::new()
        }
    }
    async fn api(
        &self,
        ctx: &SearchContext,
        query: &str,
        id: Option<&str>,
        cookie: &str,
        deadline: Instant,
    ) -> Result<Envelope, ProviderError> {
        if self
            .state
            .lock()
            .await
            .cooldown
            .is_some_and(|d| d > Instant::now())
        {
            return Err(ProviderError::Unavailable(
                "488run: rate limited (cooldown)".into(),
            ));
        }
        let mut url = self
            .base
            .join(if id.is_some() {
                "api/frontend/search/poll"
            } else {
                "api/frontend/search"
            })
            .unwrap();
        url.query_pairs_mut()
            .append_pair(if id.is_some() { "id" } else { "q" }, id.unwrap_or(query))
            .append_pair("limit", "24")
            .append_pair("theme", "wechat-cinehunt");
        let escaped =
            percent_encoding::utf8_percent_encode(query, percent_encoding::NON_ALPHANUMERIC);
        let referer = self.base.join(&format!("oeol/{escaped}.htm")).unwrap();
        tokio::time::timeout(remaining(deadline), async {
            let mut request = ctx
                .client
                .get(url)
                .header("User-Agent", UA)
                .header("Accept", "application/json, text/plain, */*")
                .header("Referer", referer.as_str());
            if !cookie.is_empty() {
                request = request.header("Cookie", cookie);
            }
            let response = crate::http::execute(request)
                .await
                .map_err(|_| ProviderError::Network("488run frontend: transport error".into()))?;
            let status = response.status();
            let new_cookie = cookies(&response);
            let bytes = crate::http::read_response(response, 4 << 20)
                .await
                .map_err(|e| {
                    ProviderError::Protocol(format!("488run frontend HTTP {status}: {e}"))
                })?;
            if let Some(cookie) = new_cookie {
                let mut s = self.state.lock().await;
                s.cookie = cookie;
                s.cookie_at = Some(Instant::now());
            }
            let body = String::from_utf8_lossy(&bytes);
            if status.as_u16() == 429 || (status.as_u16() == 403 && body.contains("访问过于频繁"))
            {
                self.state.lock().await.cooldown = Some(Instant::now() + Duration::from_secs(20));
                return Err(ProviderError::Unavailable("488run: rate limited".into()));
            }
            if matches!(status.as_u16(), 401 | 403) {
                return Err(ProviderError::Protocol(format!(
                    "488run session HTTP {}",
                    status.as_u16()
                )));
            }
            if status.as_u16() != 200 {
                return Err(ProviderError::Protocol(format!(
                    "488run frontend HTTP {status}"
                )));
            }
            let env: Envelope = serde_json::from_slice(&bytes)
                .map_err(|_| ProviderError::Parse("488run frontend: malformed JSON".into()))?;
            if env.code != 0 && !(id.is_some() && matches!(env.code, 404 | 1004)) {
                if env.message.contains("频繁") {
                    self.state.lock().await.cooldown =
                        Some(Instant::now() + Duration::from_secs(20));
                    return Err(ProviderError::Unavailable("488run: rate limited".into()));
                }
                return Err(ProviderError::Protocol(format!(
                    "488run frontend API code {}",
                    env.code
                )));
            }
            Ok(env)
        })
        .await
        .map_err(|_| ProviderError::Timeout)?
    }
}
fn merge(order: &mut Vec<String>, items: &mut HashMap<String, Value>, data: Data) {
    for row in data.list.into_iter().chain(data.items) {
        if field(&row, &["status"]).eq_ignore_ascii_case("invalid") {
            continue;
        }
        let key = field(
            &row,
            &[
                "share_link",
                "link",
                "url",
                "magnet",
                "id",
                "resource_id",
                "slug",
                "title",
                "name",
            ],
        )
        .to_owned();
        if key.is_empty() {
            continue;
        }
        if let Some(old) = items.get_mut(&key) {
            if let (Some(old), Some(new)) = (old.as_object_mut(), row.as_object()) {
                for (k, v) in new {
                    let missing = old
                        .get(k)
                        .is_none_or(|v| v.is_null() || v.as_str() == Some(""));
                    let longer = matches!(k.as_str(), "desc" | "description")
                        && v.as_str().map(str::len)
                            > old.get(k).and_then(Value::as_str).map(str::len);
                    if missing || longer {
                        old.insert(k.clone(), v.clone());
                    }
                }
            }
        } else {
            order.push(key.clone());
            items.insert(key, row);
        }
    }
}
fn date(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|d| d.with_timezone(&chrono::Utc))
        .or_else(|| {
            let date = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S")
                .ok()
                .or_else(|| {
                    chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                        .ok()
                        .and_then(|d| d.and_hms_opt(0, 0, 0))
                })?;
            Some(date.and_utc() - chrono::Duration::hours(8))
        })
}
fn results(
    order: &[String],
    items: &HashMap<String, Value>,
    base: &Url,
    query: &str,
) -> Vec<SearchResult> {
    static URL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)(?:https?://[a-zA-Z0-9._~:/?#\[\]@!$&'*+,;=%-]+|magnet:\?xt=urn:btih:[^\s<>\"']+|ed2k://\|file\|[^\s<>\"']+\|/)"#).unwrap()
    });
    static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9]{4,8}").unwrap());
    static PWD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(?:提取码|密码|访问码|验证码|口令|pwd|password|passcode)\s*[:：=]\s*([A-Za-z0-9]{4})\b").unwrap()
    });
    static NOISE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\s*提取码\s*[:：]?\s*(?:\d+\s*(?:天|小时|分钟|秒|个月|周|年)前)?\s*$").unwrap()
    });
    static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^A-Za-z0-9_-]+").unwrap());
    let mut rows = Vec::new();
    let mut ids = HashSet::new();
    for (index, key) in order.iter().enumerate() {
        let Some(row) = items.get(key) else {
            continue;
        };
        let title = NOISE
            .replace_all(field(row, &["title", "name"]), "")
            .trim()
            .to_owned();
        let content = field(row, &["description", "desc"]).to_owned();
        let text = format!("{title} {content}");
        if !query
            .split_whitespace()
            .all(|w| text.to_lowercase().contains(&w.to_lowercase()))
        {
            continue;
        }
        let all = format!(
            "{} {} {} {} {text}",
            field(row, &["share_link"]),
            field(row, &["link"]),
            field(row, &["url"]),
            field(row, &["magnet"])
        );
        let mut links = Vec::new();
        let mut seen = HashSet::new();
        for m in URL.find_iter(&all) {
            let raw = m.as_str().trim_end_matches(['.', ',', ';']);
            let Ok(mut url) = Url::parse(raw) else {
                continue;
            };
            let host = url.host_str().unwrap_or("");
            if url.origin() == base.origin()
                || ["488.run", "b46.cn", "weserv.nl"]
                    .iter()
                    .any(|h| host == *h || host.ends_with(&format!(".{h}")))
            {
                continue;
            }
            let mut link = Link::new(raw);
            if url.host_str() == Some("fast.uc.cn") {
                link.cloud_type = CloudType::Uc;
            }
            if link.cloud_type == CloudType::Others {
                let host = url.host_str().unwrap_or("");
                let path = url.path().to_ascii_lowercase();
                let image = host.starts_with("img.")
                    || host.starts_with("image.")
                    || [".jpg", ".jpeg", ".png", ".gif", ".webp", ".svg"]
                        .iter()
                        .any(|ext| path.ends_with(ext));
                if image
                    || !matches!(url.scheme(), "http" | "https")
                    || !(path.starts_with("/s/")
                        || path.starts_with("/t/")
                        || path.contains("share"))
                    || path.len() <= 3
                {
                    continue;
                }
                link.cloud_type = match field(row, &["disk_type", "drive_type"])
                    .trim()
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "quark" | "夸克" | "夸克网盘" => CloudType::Quark,
                    "baidu" | "bd" | "百度" | "百度网盘" => CloudType::Baidu,
                    "uc" | "uc网盘" => CloudType::Uc,
                    "xunlei" | "xl" | "迅雷" | "迅雷网盘" => CloudType::Xunlei,
                    "aliyun" | "ali" | "alipan" | "阿里" | "阿里云盘" => CloudType::Aliyun,
                    "tianyi" | "189" | "天翼" | "天翼云盘" => CloudType::Tianyi,
                    "115" | "115网盘" => CloudType::One15,
                    "123" | "123pan" | "123网盘" => CloudType::Pan123,
                    "mobile" | "139" | "caiyun" | "移动" | "移动云盘" => CloudType::Mobile,
                    "pikpak" => CloudType::Pikpak,
                    "guangya" | "光鸭" | "光鸭云盘" => CloudType::Guangya,
                    _ => CloudType::Others,
                };
                if link.cloud_type == CloudType::Others {
                    continue;
                }
            }
            if matches!(url.scheme(), "http" | "https") {
                let mut pwd = None;
                let mut pairs = Vec::new();
                for (k, v) in url.query_pairs() {
                    if ["pwd", "password", "passcode", "code"].contains(&k.as_ref()) {
                        if let Some(m) = TOKEN.find(&v) {
                            let value = m.as_str().to_owned();
                            pwd.get_or_insert(value.clone());
                            pairs.push((k.into_owned(), value));
                        }
                    } else {
                        pairs.push((k.into_owned(), v.into_owned()));
                    }
                }
                if !pairs.is_empty() {
                    url.set_query(None);
                    url.query_pairs_mut().extend_pairs(pairs);
                    link.url = url.to_string();
                }
                link.password = pwd.or_else(|| PWD.captures(&text).map(|c| c[1].to_owned()));
            }
            link.work_title = Some(title.clone());
            if seen.insert((link.url.clone(), link.password.clone())) {
                links.push(link);
            }
        }
        if links.is_empty() || title.is_empty() {
            continue;
        }
        let raw_id = field(row, &["id", "resource_id", "slug"]);
        let id = ID
            .replace_all(if raw_id.is_empty() { key } else { raw_id }, "-")
            .chars()
            .take(64)
            .collect::<String>();
        let id = if id.is_empty() { index.to_string() } else { id };
        if !ids.insert(id.clone()) {
            continue;
        }
        let mut r = SearchResult::provider(format!("488run-{id}"), "488run");
        r.title = title;
        r.content = content;
        r.links = links;
        r.datetime = date(field(row, &["share_time", "updated_at", "created_at"]))
            .or_else(|| Some(chrono::Utc::now()));
        let image = field(row, &["poster", "cover"]);
        if !image.is_empty()
            && let Ok(url) = base.join(image)
        {
            r.images.push(url.to_string());
        }
        let mut tags = HashSet::new();
        for key in ["source_label", "category", "year", "area"] {
            let tag = field(row, &[key]);
            if !tag.is_empty() && !matches!(tag, "资源" | "全部") && tags.insert(tag.to_owned())
            {
                r.tags.push(tag.to_owned());
            }
        }
        rows.push(r);
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.datetime));
    rows
}
#[async_trait]
impl Provider for Run488Provider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("488run", 2, KeywordFilterMode::Provider)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        if self
            .state
            .lock()
            .await
            .cooldown
            .is_some_and(|d| d > Instant::now())
        {
            return Err(ProviderError::Unavailable(
                "488run: rate limited (cooldown)".into(),
            ));
        }
        let deadline = ctx
            .deadline()
            .unwrap_or_else(|| Instant::now() + ctx.timeout);
        let mut cookie = self.cookie(ctx, deadline, false, "").await;
        let mut initial = self.api(ctx, query, None, &cookie, deadline).await;
        if matches!(&initial,Err(ProviderError::Protocol(s)) if s=="488run session HTTP 401"||s=="488run session HTTP 403")
        {
            cookie = self.cookie(ctx, deadline, true, &cookie).await;
            initial = self.api(ctx, query, None, &cookie, deadline).await;
        }
        let initial = initial?;
        if initial.data.blocked {
            return Ok(vec![]);
        }
        let mut complete = initial.data.complete;
        let id = if initial.data.search_id.is_empty() {
            initial.data.search_id_alt.clone()
        } else {
            initial.data.search_id.clone()
        };
        let mut order = Vec::new();
        let mut items = HashMap::new();
        merge(&mut order, &mut items, initial.data);
        ctx.publish(results(&order, &items, &self.base, query));
        for _ in 0..6 {
            if complete || id.is_empty() {
                break;
            }
            if deadline.saturating_duration_since(Instant::now()) <= Duration::from_millis(520) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(420)).await;
            let polled = self.api(ctx, query, Some(&id), &cookie, deadline).await;
            match polled {
                Ok(env) => {
                    complete = env.data.complete || matches!(env.code, 404 | 1004);
                    merge(&mut order, &mut items, env.data);
                    ctx.publish(results(&order, &items, &self.base, query));
                }
                Err(e) => return Err(e),
            }
        }
        if !complete && !id.is_empty() {
            return Err(ProviderError::Timeout);
        }
        Ok(results(&order, &items, &self.base, query))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hinted_shares_accept_aliases_but_reject_site_and_image_proxies() {
        let rows = [
            ("fast", "https://fast.uc.cn/s/abc", ""),
            ("hint", "https://share.example/share/abc", "夸克"),
            ("site", "https://b46.cn/s/ad", "quark"),
            ("proxy", "https://images.weserv.nl/share/abc", "quark"),
            ("image", "https://img.example/s/image.png", "quark"),
        ];
        let mut order = Vec::new();
        let mut items = HashMap::new();
        for (id, url, ty) in rows {
            let row = serde_json::json!({"id":id,"title":"测试","url":url,"disk_type":ty});
            order.push(id.to_owned());
            items.insert(id.to_owned(), row);
        }
        let rows = results(
            &order,
            &items,
            &Url::parse("https://488.run/").unwrap(),
            "测试",
        );
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.links[0].cloud_type == CloudType::Uc));
        assert!(
            rows.iter()
                .any(|r| r.links[0].cloud_type == CloudType::Quark)
        );
    }
    #[test]
    fn fixtures() {
        let env: Envelope = serde_json::from_str(include_str!(
            "../../tests/fixtures/providers/488run/success.json"
        ))
        .unwrap();
        let mut order = Vec::new();
        let mut items = HashMap::new();
        merge(&mut order, &mut items, env.data);
        let r = results(
            &order,
            &items,
            &Url::parse("https://488.run/").unwrap(),
            "测试",
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].links[0].password.as_deref(), Some("623f"));
        assert_eq!(r[0].title, "测试影片");
        assert!(r[0].images[0].starts_with("https://488.run/"));
        let e: Envelope = serde_json::from_str(include_str!(
            "../../tests/fixtures/providers/488run/empty.json"
        ))
        .unwrap();
        assert!(e.data.list.is_empty());
        assert!(
            serde_json::from_str::<Envelope>(include_str!(
                "../../tests/fixtures/providers/488run/malformed.json"
            ))
            .is_err()
        );
    }
}
