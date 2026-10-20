//! Modern Panlian protocol. Challenges intentionally do not implement Debug.
use super::*;
use futures::{StreamExt, stream};
use serde_json::{Value, json};
use std::time::Duration;

pub struct CaptchaChallenge {
    pub id: String,
    pub image: String,
}
impl CaptchaChallenge {
    /// The returned handle owns a private temporary image; dropping it removes the file.
    pub fn write_private_image(&self) -> Result<tempfile::NamedTempFile, ProviderError> {
        use base64::Engine;
        use std::io::Write;
        let (_, encoded) = self.image.split_once(";base64,").ok_or_else(|| {
            ProviderError::Protocol("Panlian captcha is not a Base64 image".into())
        })?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| ProviderError::Parse("Panlian invalid captcha image".into()))?;
        let mut file = tempfile::Builder::new()
            .prefix("pansou-panlian-")
            .suffix(".png")
            .tempfile()
            .map_err(|e| ProviderError::Unavailable(e.to_string()))?;
        file.write_all(&bytes)
            .map_err(|e| ProviderError::Unavailable(e.to_string()))?;
        Ok(file)
    }
}
pub struct EmailConfirmation {
    pub confirm_id: String,
    pub username: String,
    pub masked_email: String,
}
pub enum LoginOutcome {
    Active(PanlianProfile),
    EmailRequired(EmailConfirmation),
}

impl PanlianProvider {
    pub async fn captcha(&self, session: &Session) -> Result<CaptchaChallenge, ProviderError> {
        let url = self
            .endpoints
            .base
            .join("api/auth/captcha")
            .map_err(protocol)?;
        let value = request_get(
            session.client(),
            url,
            "",
            &self.endpoints.base,
            Duration::from_secs(20),
        )
        .await?;
        check_envelope(&value)?;
        let id = required_string(&value["data"], "id")?;
        let image = required_string(&value["data"], "image")?;
        if !image.starts_with("data:image/") {
            return Err(ProviderError::Protocol(
                "Panlian captcha omitted image data URL".into(),
            ));
        }
        Ok(CaptchaChallenge { id, image })
    }

    pub async fn login_with_captcha(
        &self,
        session: &Session,
        profile: &str,
        username: &str,
        password: &str,
        captcha_id: &str,
        captcha_code: &str,
    ) -> Result<LoginOutcome, ProviderError> {
        if [
            username.trim(),
            password,
            captcha_id.trim(),
            captcha_code.trim(),
        ]
        .iter()
        .any(|v| v.is_empty())
        {
            return Err(ProviderError::Protocol(
                "username, password and graphic captcha are required".into(),
            ));
        }
        let value = self
            .auth_form(
                session,
                "api/auth/login",
                &[
                    ("username", username.trim()),
                    ("password", password),
                    ("remember", "1"),
                    ("captcha_id", captcha_id),
                    ("captcha_code", captcha_code.trim()),
                ],
            )
            .await?;
        let details = &value["details"];
        if value["success"].as_bool() != Some(true)
            && (details["captcha_error"].as_bool() == Some(true)
                || value["message"].as_str().unwrap_or("").contains("验证码"))
        {
            return Err(ProviderError::Protocol(
                "Panlian graphic captcha incorrect or expired; request a new image".into(),
            ));
        }
        if details["email_verify_required"].as_bool() == Some(true) {
            return Ok(LoginOutcome::EmailRequired(EmailConfirmation {
                confirm_id: required_string(details, "confirm_id")?,
                username: username.trim().into(),
                masked_email: details["email"].as_str().unwrap_or("").into(),
            }));
        }
        check_envelope(&value)?;
        Ok(LoginOutcome::Active(
            self.save_session(session, profile, username)?,
        ))
    }

    pub async fn confirm_email(
        &self,
        session: &Session,
        profile: &str,
        pending: &EmailConfirmation,
        email: &str,
    ) -> Result<PanlianProfile, ProviderError> {
        if email.trim().is_empty() {
            return Err(ProviderError::Protocol(
                "account's bound email is required (not an OTP)".into(),
            ));
        }
        let value = self
            .auth_form(
                session,
                "api/auth/login/confirm-email",
                &[
                    ("confirm_id", pending.confirm_id.as_str()),
                    ("email", email.trim()),
                    ("remember", "1"),
                ],
            )
            .await?;
        if value["success"].as_bool() != Some(true) {
            return Err(ProviderError::Protocol(
                "Panlian email mismatch or expired confirmation; retry or restart login".into(),
            ));
        }
        self.save_session(session, profile, &pending.username)
    }

    async fn auth_form(
        &self,
        session: &Session,
        path: &str,
        form: &[(&str, &str)],
    ) -> Result<Value, ProviderError> {
        let response = crate::http::execute(
            panlian_headers(
                session
                    .client()
                    .post(self.endpoints.base.join(path).map_err(protocol)?),
                &self.endpoints.base,
                "",
                self.endpoints
                    .base
                    .join("login")
                    .map_err(protocol)?
                    .as_str(),
                false,
            )
            .timeout(Duration::from_secs(20))
            .form(form),
        )
        .await
        .map_err(network)?;
        let status = response.status();
        let body = bounded_body(response, 16 * 1024 * 1024).await?;
        if status.as_u16() != 200 && status.as_u16() != 400 {
            return Err(ProviderError::Network(format!(
                "Panlian auth HTTP {status}"
            )));
        }
        serde_json::from_slice(&body).map_err(parse)
    }
    fn save_session(
        &self,
        session: &Session,
        profile: &str,
        username: &str,
    ) -> Result<PanlianProfile, ProviderError> {
        let cookie = session
            .cookie_header(&self.endpoints.base)
            .unwrap_or_default();
        if !cookie
            .split(';')
            .any(|v| v.trim().starts_with("admin_session=") && v.trim() != "admin_session=")
        {
            return Err(ProviderError::Protocol(
                "Panlian login omitted admin_session cookie".into(),
            ));
        }
        let saved = PanlianProfile {
            username: username.trim().into(),
            password: None,
            cookie,
            status: active(),
            last_access: Some(Utc::now()),
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

    pub(super) async fn modern_search(
        &self,
        ctx: &SearchContext,
        profile: &PanlianProfile,
        query: &str,
    ) -> Result<Option<Vec<SearchResult>>, ProviderError> {
        let mut url = self.endpoints.base.join("api/videos").map_err(protocol)?;
        url.query_pairs_mut().extend_pairs([
            ("search", query),
            ("sort", "year_desc"),
            ("page", "1"),
            ("page_size", "10"),
        ]);
        let root = match request_get(
            &ctx.client,
            url,
            &profile.cookie,
            &self.endpoints.base,
            ctx.request_timeout(Duration::from_secs(20)),
        )
        .await
        {
            Err(ProviderError::Unavailable(_)) => return Ok(None),
            other => other?,
        };
        if root.get("success").is_none() {
            return Ok(None);
        }
        check_envelope(&root)?;
        let list = root
            .pointer("/data/list")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ProviderError::Protocol("Panlian modern response omitted list".into())
            })?;
        let videos = list.iter().take(10).cloned().collect::<Vec<_>>();
        let outcomes = stream::iter(videos.into_iter().map(|video| async move {
            let row = self.modern_video(ctx, profile, query, &video).await?;
            if let Some(ref row) = row {
                ctx.publish(vec![row.clone()]);
            }
            Ok::<_, ProviderError>(row)
        }))
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await;
        let mut rows = Vec::new();
        let mut failure = None;
        for result in outcomes {
            match result {
                Ok(Some(row)) => rows.push(row),
                Ok(None) => {}
                Err(ProviderError::AuthRequired) => return Err(ProviderError::AuthRequired),
                Err(e) => failure = Some(e),
            }
        }
        if let Some(e) = failure {
            return Err(e);
        }
        Ok(Some(rows))
    }
    async fn modern_video(
        &self,
        ctx: &SearchContext,
        profile: &PanlianProfile,
        query: &str,
        raw: &Value,
    ) -> Result<Option<SearchResult>, ProviderError> {
        let video = normalize_video(raw)?;
        if video.vod_id <= 0 {
            return Err(ProviderError::Protocol("Panlian invalid video ID".into()));
        }
        let root = match request_get(
            &ctx.client,
            self.endpoints
                .base
                .join(&format!("api/videos/{}", video.vod_id))
                .map_err(protocol)?,
            &profile.cookie,
            &self.endpoints.base,
            ctx.request_timeout(Duration::from_secs(20)),
        )
        .await
        {
            Err(ProviderError::Unavailable(_)) => {
                return self.legacy_video(ctx, profile, query, video).await;
            }
            other => other?,
        };
        if root.get("success").is_none() {
            return self.legacy_video(ctx, profile, query, video).await;
        }
        check_envelope(&root)?;
        let metadata = root
            .pointer("/data/links")
            .and_then(Value::as_array)
            .ok_or_else(|| ProviderError::Protocol("Panlian detail omitted links".into()))?;
        let mut links = Vec::new();
        for item in select_links(metadata) {
            let opened = self.open_link(ctx, profile, item).await;
            match opened {
                Ok(mut link) if !self.blocked.contains(&link.cloud_type) => {
                    if link.work_title.is_none() {
                        link.work_title = Some(video.vod_name.clone());
                    }
                    links.push(link);
                }
                Err(ProviderError::AuthRequired) => return Err(ProviderError::AuthRequired),
                _ => {}
            }
        }
        Ok(make_row(video, links))
    }
    async fn legacy_video(
        &self,
        ctx: &SearchContext,
        profile: &PanlianProfile,
        query: &str,
        video: Video,
    ) -> Result<Option<SearchResult>, ProviderError> {
        let mut url = self
            .endpoints
            .base
            .join("api/search_pan_links.php")
            .map_err(protocol)?;
        url.query_pairs_mut()
            .append_pair("keyword", query)
            .append_pair("vod_id", &video.vod_id.to_string());
        let mut response: PanResponse =
            get_json(ctx, url, &profile.cookie, &self.endpoints.base).await?;
        if !response.success && response.message.contains("登录") {
            return Err(ProviderError::AuthRequired);
        }
        if !response.success {
            return Err(ProviderError::Protocol(
                "Panlian legacy links failed".into(),
            ));
        }
        self.resolve_tokens(ctx, &profile.cookie, &mut response.data)
            .await;
        Ok(make_row(
            video,
            flatten_groups(&response.data, &self.blocked),
        ))
    }
    async fn open_link(
        &self,
        ctx: &SearchContext,
        profile: &PanlianProfile,
        item: &Value,
    ) -> Result<Link, ProviderError> {
        let id = item["id"]
            .as_i64()
            .or_else(|| item["id"].as_str()?.parse().ok())
            .filter(|id| *id > 0)
            .ok_or_else(|| ProviderError::Protocol("Panlian invalid link ID".into()))?;
        let response = crate::http::execute(
            panlian_headers(
                ctx.client.post(
                    self.endpoints
                        .base
                        .join("api/videos/link-ticket")
                        .map_err(protocol)?,
                ),
                &self.endpoints.base,
                &profile.cookie,
                self.endpoints.base.as_str(),
                false,
            )
            .timeout(ctx.request_timeout(Duration::from_secs(20)))
            .json(&json!({"link_id":id})),
        )
        .await
        .map_err(network)?;
        if response.status().as_u16() == 401 {
            return Err(ProviderError::AuthRequired);
        }
        let status = response.status();
        let bytes = bounded_body(response, 1024 * 1024).await?;
        let text = String::from_utf8_lossy(&bytes);
        if text.contains("请先登录") || text.contains("admin_auth_required") {
            return Err(ProviderError::AuthRequired);
        }
        if status.as_u16() != 200 {
            return Err(ProviderError::Network(format!(
                "Panlian ticket HTTP {status}"
            )));
        }
        let root: Value = serde_json::from_slice(&bytes).map_err(parse)?;
        check_envelope(&root)?;
        let ticket = required_string(&root["data"], "ticket")?;
        let mut url = self
            .endpoints
            .base
            .join(&format!("api/videos/link-open/{id}"))
            .map_err(protocol)?;
        url.query_pairs_mut().append_pair("t", &ticket);
        let opened = request_get(
            &ctx.client,
            url,
            &profile.cookie,
            &self.endpoints.base,
            ctx.request_timeout(Duration::from_secs(20)),
        )
        .await?;
        check_envelope(&opened)?;
        let url = required_string(&opened["data"], "url")?;
        if !is_real_url(&url) {
            return Err(ProviderError::Protocol(
                "Panlian link-open omitted real URL".into(),
            ));
        }
        let kind =
            if item["is_magnet"].as_bool() == Some(true) || item["is_magnet"].as_i64() == Some(1) {
                CloudType::Magnet
            } else {
                normalize_kind(item["pan_type"].as_str().unwrap_or(""), &url)
            };
        Ok(Link {
            cloud_type: kind,
            url,
            password: opened["data"]["code"]
                .as_str()
                .filter(|v| !v.is_empty())
                .map(str::to_owned),
            datetime: ["updated_at", "created_at"]
                .iter()
                .find_map(|k| parse_time(item[*k].as_str().unwrap_or(""))),
            work_title: item["title"]
                .as_str()
                .filter(|v| !v.trim().is_empty())
                .or_else(|| item["video_title"].as_str())
                .filter(|v| !v.is_empty())
                .map(str::to_owned),
        })
    }
}

fn required_string(value: &Value, key: &str) -> Result<String, ProviderError> {
    value[key]
        .as_str()
        .filter(|v| !v.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ProviderError::Protocol(format!("Panlian omitted {key}")))
}
fn check_envelope(value: &Value) -> Result<(), ProviderError> {
    let message = value["message"].as_str().unwrap_or("");
    if value["success"].as_bool() != Some(true)
        && (value["code"].as_str() == Some("admin_auth_required")
            || message.contains("登录")
            || message.to_lowercase().contains("login"))
    {
        return Err(ProviderError::AuthRequired);
    }
    if value["success"].as_bool() != Some(true) {
        return Err(ProviderError::Protocol(
            "Panlian business request failed".into(),
        ));
    }
    Ok(())
}
fn normalize_video(value: &Value) -> Result<Video, ProviderError> {
    let mut value = value.clone();
    for (old, new) in [
        ("vod_id", "id"),
        ("alias", "alias"),
        ("vod_name", "title"),
        ("vod_pic", "cover"),
        ("vod_content", "intro"),
        ("vod_year", "year"),
        ("vod_area", "area"),
        ("vod_lang", "lang"),
        ("vod_score", "score"),
        ("vod_remarks", "remarks"),
        ("vod_actor", "actor"),
        ("vod_director", "director"),
        ("type_name", "type"),
    ] {
        if value.get(old).is_none_or(|v| {
            v.is_null()
                || v.as_str().is_some_and(|v| v.trim().is_empty())
                || (old == "vod_id" && v.as_i64() == Some(0))
        }) {
            value[old] = value[new].clone();
        }
        if old != "vod_id" && !value[old].is_string() {
            value[old] = Value::String(if value[old].is_null() {
                String::new()
            } else {
                value[old].to_string()
            });
        }
    }
    if let Some(id) = value["vod_id"].as_str().and_then(|s| s.parse::<i64>().ok()) {
        value["vod_id"] = json!(id);
    }
    serde_json::from_value(value).map_err(parse)
}
fn select_links(items: &[Value]) -> Vec<&Value> {
    let mut chosen = Vec::new();
    let mut kinds = HashSet::new();
    for (i, item) in items.iter().enumerate() {
        if kinds.insert(normalize_kind(item["pan_type"].as_str().unwrap_or(""), "")) {
            chosen.push(i);
            if chosen.len() == 3 {
                break;
            }
        }
    }
    for i in 0..items.len() {
        if chosen.len() == 3 {
            break;
        }
        if !chosen.contains(&i) {
            chosen.push(i);
        }
    }
    chosen.into_iter().map(|i| &items[i]).collect()
}
fn make_row(video: Video, links: Vec<Link>) -> Option<SearchResult> {
    if links.is_empty() {
        return None;
    }
    Some(SearchResult {
        id: format!("panlian-{}", video.vod_id),
        source: Source::provider("panlian"),
        datetime: links.iter().filter_map(|l| l.datetime).max(),
        title: video.vod_name.trim().into(),
        content: video.content(),
        links,
        tags: vec![video.type_name, "panlian".into()],
        images: (!video.vod_pic.is_empty())
            .then_some(video.vod_pic)
            .into_iter()
            .collect(),
    })
}
pub(super) async fn bounded_body(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, ProviderError> {
    crate::http::read_response(response, limit)
        .await
        .map_err(|_| {
            ProviderError::Protocol("Panlian response read failed or exceeded limit".into())
        })
}
pub(super) async fn request_get(
    client: &reqwest::Client,
    url: Url,
    cookie: &str,
    base: &Url,
    timeout: Duration,
) -> Result<Value, ProviderError> {
    let mut last = ProviderError::Timeout;
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(200 * attempt)).await;
        }
        let response = crate::http::execute(
            panlian_headers(client.get(url.clone()), base, cookie, base.as_str(), false)
                .timeout(timeout),
        )
        .await;
        let response = match response {
            Ok(r) => r,
            Err(e) => {
                if e.downcast_ref::<reqwest::Error>()
                    .is_some_and(|e| e.is_builder())
                {
                    return Err(network(e));
                }
                last = network(e);
                continue;
            }
        };
        let status = response.status();
        if status.as_u16() == 401 {
            return Err(ProviderError::AuthRequired);
        }
        let body = match bounded_body(response, 16 * 1024 * 1024).await {
            Ok(b) => b,
            Err(e) => {
                last = e;
                continue;
            }
        };
        let text = String::from_utf8_lossy(&body);
        if text.contains("请先登录")
            || text.contains("admin_auth_required")
            || (status.as_u16() == 200
                && serde_json::from_slice::<Value>(&body).is_err()
                && text.to_lowercase().contains("login"))
        {
            return Err(ProviderError::AuthRequired);
        }
        if status.as_u16() == 404 || status.as_u16() == 405 {
            return Err(ProviderError::Unavailable(
                "Panlian endpoint unavailable".into(),
            ));
        }
        if status.as_u16() != 200 {
            last = ProviderError::Network(format!("Panlian HTTP {status}"));
            continue;
        }
        return serde_json::from_slice(&body).map_err(parse);
    }
    Err(last)
}

#[cfg(test)]
mod normalization_tests {
    use super::*;
    #[test]
    fn legacy_metadata_wins_and_type_keys_remain_independent() {
        let video = normalize_video(&json!({"id":12,"title":"new","vod_name":"old","type":"movie","type_name":"电影","year":2026,"alias":"alias","score":8.5})).unwrap();
        assert_eq!(video.vod_id, 12);
        assert_eq!(video.vod_name, "old");
        assert_eq!(video.type_name, "电影");
        assert_eq!(video.vod_year, "2026");
        assert_eq!(video.vod_score, "8.5");
        let fallback = normalize_video(&json!({"id":12,"title":"film","type":"movie"})).unwrap();
        assert_eq!(fallback.type_name, "movie");
        assert_eq!(
            normalize_kind("", "https://pan.uc.cn/s/fixture"),
            CloudType::Uc
        );
        assert_eq!(
            normalize_kind("", "https://share.123pan.cn/s/fixture"),
            CloudType::Pan123
        );
        assert!(check_envelope(&json!({"success":true,"message":"登录成功"})).is_ok());
        let selection = select_links(&[
            json!({"id":1,"pan_type":"百度"}),
            json!({"id":2,"pan_type":"baidu"}),
            json!({"id":3,"pan_type":"quark"}),
            json!({"id":4,"pan_type":"uc"}),
        ])
        .into_iter()
        .map(|v| v["id"].as_i64().unwrap())
        .collect::<Vec<_>>();
        assert_eq!(selection, [1, 3, 4]);
    }
}
