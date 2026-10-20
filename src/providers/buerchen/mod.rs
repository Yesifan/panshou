mod stream;
use super::html::{self, UA, network, parse_error, valid_password, words_match};
use crate::{
    core::{CloudType, Link, ProviderError, SearchResult, Source},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use futures::{FutureExt, StreamExt, stream::FuturesUnordered};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::Semaphore, time::Instant};
use url::Url;
const LIMIT: usize = 2 << 20;
#[derive(Clone, Debug)]
struct Candidate {
    title: String,
    url: String,
    description: String,
    kind: usize,
    daily: bool,
}
#[derive(Clone, Debug)]
struct Share {
    link: Link,
    identity: String,
}
#[derive(Debug, Default)]
struct State {
    blocked: Option<(Instant, ProviderError)>,
    cache: HashMap<String, (Instant, Share)>,
}
#[derive(Debug)]
pub struct Buerchen {
    base: Url,
    slots: Semaphore,
    state: Mutex<State>,
}
impl Default for Buerchen {
    fn default() -> Self {
        Self::with_base(Url::parse("https://buerchen.top/").unwrap())
    }
}
impl Buerchen {
    pub fn with_base(base: Url) -> Self {
        Self {
            base,
            slots: Semaphore::new(3),
            state: Mutex::default(),
        }
    }
    fn cooldown(&self) -> Result<(), ProviderError> {
        if let Some((until, error)) = &self.state.lock().unwrap().blocked
            && *until > Instant::now()
        {
            return Err(error.clone());
        }
        Ok(())
    }
    fn api_error(&self, code: i64) -> ProviderError {
        let error = match code {
            403 | 42901 => ProviderError::Blocked,
            429 => ProviderError::RateLimited,
            _ => ProviderError::Protocol(format!("buerchen API failed (code={code})")),
        };
        if matches!(code, 403 | 429 | 42901) {
            self.state.lock().unwrap().blocked =
                Some((Instant::now() + Duration::from_secs(60), error.clone()));
        }
        error
    }
    async fn request(
        &self,
        ctx: &SearchContext,
        path: &str,
        payload: Option<Value>,
    ) -> Result<Value, ProviderError> {
        self.cooldown()?;
        if ctx.is_cancelled() {
            return Err(ProviderError::Timeout);
        }
        let url = self
            .base
            .join(path)
            .map_err(|_| parse_error("invalid buerchen endpoint"))?;
        let request = if let Some(payload) = payload {
            ctx.client
                .post(url)
                .json(&payload)
                .header("origin", self.base.origin().ascii_serialization())
        } else {
            ctx.client.get(url)
        };
        let response = crate::http::execute(
            request
                .timeout(ctx.request_timeout(Duration::from_secs(12)))
                .header("user-agent", UA)
                .header("referer", self.base.as_str())
                .header("accept", "application/json")
                .header("accept-language", "zh-CN,zh;q=0.9"),
        )
        .await
        .map_err(network)?;
        if response.status().as_u16() != 200 {
            return Err(self.api_error(response.status().as_u16() as i64));
        }
        let bytes = crate::http::read_response(response, LIMIT)
            .await
            .map_err(|_| parse_error("buerchen response read failed or exceeds 2 MiB"))?;
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| parse_error("buerchen response is not valid JSON"))?;
        if let Some(code) = value.get("code").and_then(Value::as_i64)
            && matches!(code, 403 | 429 | 42901)
        {
            return Err(self.api_error(code));
        }
        Ok(value)
    }
    async fn daily(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<Candidate>, ProviderError> {
        let mut path = Url::parse("https://placeholder.invalid/api2/content").unwrap();
        path.query_pairs_mut().append_pair("keyword", query);
        let value = self
            .request(
                ctx,
                &format!("{}?{}", path.path(), path.query().unwrap_or("")),
                None,
            )
            .await?;
        if value.get("success").and_then(Value::as_bool) != Some(true) {
            return Err(self.api_error(value.get("code").and_then(Value::as_i64).unwrap_or(-1)));
        }
        let rows = value
            .pointer("/data/data")
            .and_then(Value::as_array)
            .ok_or_else(|| parse_error("buerchen daily resource list missing"))?;
        let mut candidates = vec![];
        let mut counts = [0; 3];
        let mut seen = HashSet::new();
        for row in rows {
            let title = clean_text(row.get("title").and_then(Value::as_str).unwrap_or(""));
            if title.is_empty() || !words_match(&title, query) {
                continue;
            }
            for (index, key) in ["quarkLink", "baiduLink", "xunleiLink"].iter().enumerate() {
                let raw = row.get(*key).and_then(Value::as_str).unwrap_or("").trim();
                if !raw.is_empty()
                    && raw.len() <= 8192
                    && counts[index] < 2
                    && seen.insert((index, raw.to_owned()))
                {
                    counts[index] += 1;
                    candidates.push(Candidate {
                        title: title.clone(),
                        url: raw.into(),
                        description: String::new(),
                        kind: index * 2,
                        daily: true,
                    });
                }
            }
        }
        Ok(candidates)
    }
    async fn web(
        &self,
        ctx: &SearchContext,
        query: &str,
        kind: usize,
        progress: Arc<Mutex<Vec<Candidate>>>,
    ) -> Result<Vec<Candidate>, ProviderError> {
        self.cooldown()?;
        if ctx.is_cancelled() {
            return Err(ProviderError::Timeout);
        }
        let mut url = self.base.join("api/other/web_search").unwrap();
        url.query_pairs_mut()
            .append_pair("title", query)
            .append_pair("is_type", &kind.to_string());
        let escaped = component(query);
        let mut response = crate::http::execute(
            ctx.client
                .get(url)
                .timeout(ctx.request_timeout(Duration::from_secs(12)))
                .header("user-agent", UA)
                .header("accept", "text/event-stream")
                .header("accept-language", "zh-CN,zh;q=0.9")
                .header("accept-encoding", "identity")
                .header(
                    "referer",
                    self.base
                        .join(&format!("s/{escaped}.html"))
                        .unwrap()
                        .as_str(),
                ),
        )
        .await
        .map_err(network)?;
        if response.status().as_u16() != 200 {
            return Err(self.api_error(response.status().as_u16() as i64));
        }
        if response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("application/json"))
        {
            let bytes = crate::http::read_response(response, LIMIT)
                .await
                .map_err(|_| parse_error("buerchen search response exceeds limit"))?;
            if let Ok(value) = serde_json::from_slice::<Value>(&bytes)
                && let Some(code) = value.get("code").and_then(Value::as_i64)
                && code != 0
                && code != 200
            {
                return Err(self.api_error(code));
            }
            return Err(parse_error("buerchen search endpoint did not return SSE"));
        }
        // Prefer incremental identity SSE. Servers ignoring that preference
        // still use the bounded shared decompressor before framing is parsed.
        if response
            .headers()
            .get("content-encoding")
            .is_some_and(|v| v != "identity")
        {
            let bytes = crate::http::read_response(response, LIMIT)
                .await
                .map_err(|_| parse_error("buerchen compressed SSE read failed or exceeds limit"))?;
            return self.buffered_web(ctx, query, kind, &bytes, &progress);
        }
        let mut parser = stream::Parser::default();
        let mut carry = Vec::new();
        let mut total = 0;
        let mut delivered = 0;
        let mut processed_lines = 0;
        while let Some(chunk) = response.chunk().await.map_err(network)? {
            total += chunk.len();
            if total > LIMIT {
                return Err(parse_error("buerchen SSE exceeds 2 MiB"));
            }
            carry.extend_from_slice(&chunk);
            let mut consumed = 0;
            while let Some(offset) = carry[consumed..].iter().position(|b| *b == b'\n') {
                if offset > stream::LINE_LIMIT {
                    return Err(parse_error("buerchen SSE line exceeds 256 KiB"));
                }
                let end = consumed + offset;
                let line = std::str::from_utf8(&carry[consumed..end])
                    .map_err(|_| parse_error("invalid SSE UTF-8"))?;
                let result = parser.line(line, query, kind, self);
                *progress.lock().unwrap() = parser.entries.clone();
                // Reserve two slots for daily candidates. Already-valid direct
                // shares can be retained even if a different route stalls.
                for candidate in parser.entries.iter().take(4).skip(delivered) {
                    if let Some(share) = direct(candidate) {
                        ctx.publish(vec![make_result(candidate, share)]);
                    }
                    delivered += 1;
                }
                result?;
                if parser.done {
                    return Ok(parser.entries);
                }
                consumed = end + 1;
                processed_lines += 1;
                if processed_lines % 256 == 0 {
                    tokio::task::yield_now().await;
                }
            }
            carry.drain(..consumed);
            if carry.len() > stream::LINE_LIMIT {
                return Err(parse_error("buerchen SSE line exceeds 256 KiB"));
            }
        }
        if !carry.is_empty() {
            let line = std::str::from_utf8(&carry).map_err(|_| parse_error("invalid SSE UTF-8"))?;
            parser.line(line, query, kind, self)?;
        }
        let result = parser.consume(query, kind, self);
        *progress.lock().unwrap() = parser.entries.clone();
        result?;
        if parser.done {
            Ok(parser.entries)
        } else {
            Err(parse_error("buerchen SSE ended without DONE"))
        }
    }
    fn buffered_web(
        &self,
        ctx: &SearchContext,
        query: &str,
        kind: usize,
        bytes: &[u8],
        progress: &Mutex<Vec<Candidate>>,
    ) -> Result<Vec<Candidate>, ProviderError> {
        let mut parser = stream::Parser::default();
        let mut delivered = 0;
        for line in bytes.split(|b| *b == b'\n') {
            if line.len() > stream::LINE_LIMIT {
                return Err(parse_error("buerchen SSE line exceeds 256 KiB"));
            }
            let line = std::str::from_utf8(line).map_err(|_| parse_error("invalid SSE UTF-8"))?;
            let result = parser.line(line, query, kind, self);
            *progress.lock().unwrap() = parser.entries.clone();
            for candidate in parser.entries.iter().take(4).skip(delivered) {
                if let Some(share) = direct(candidate) {
                    ctx.publish(vec![make_result(candidate, share)]);
                }
                delivered += 1;
            }
            result?;
            if parser.done {
                return Ok(parser.entries);
            }
        }
        let result = parser.consume(query, kind, self);
        *progress.lock().unwrap() = parser.entries.clone();
        result?;
        if parser.done {
            Ok(parser.entries)
        } else {
            Err(parse_error("buerchen SSE ended without DONE"))
        }
    }
    async fn resolve(
        &self,
        ctx: &SearchContext,
        candidate: &Candidate,
    ) -> Result<Share, ProviderError> {
        let key = format!("{}\0{}\0{}", candidate.kind, candidate.daily, candidate.url);
        let cached = || {
            self.state
                .lock()
                .unwrap()
                .cache
                .get(&key)
                .filter(|(until, _)| *until > Instant::now())
                .map(|(_, share)| share.clone())
        };
        if let Some(share) = cached() {
            return Ok(share);
        }
        let _slot = self
            .slots
            .acquire()
            .await
            .map_err(|_| ProviderError::Timeout)?;
        if ctx.is_cancelled() {
            return Err(ProviderError::Timeout);
        }
        if let Some(share) = cached() {
            return Ok(share);
        }
        let original = if candidate.daily {
            let value = self
                .request(
                    ctx,
                    "api2/decrypt",
                    Some(json!({"encryptUrl":candidate.url})),
                )
                .await?;
            if value.get("success").and_then(Value::as_bool) != Some(true) {
                return Err(self.api_error(value.get("code").and_then(Value::as_i64).unwrap_or(-1)));
            }
            value
                .pointer("/data/url")
                .and_then(Value::as_str)
                .ok_or_else(|| parse_error("buerchen decrypt data.url missing"))?
                .to_owned()
        } else {
            candidate.url.clone()
        };
        let original_share = normalize(&original, &candidate.description);
        if original_share
            .as_ref()
            .is_some_and(|link| link.cloud_type != cloud(candidate.kind))
        {
            return Err(parse_error(
                "buerchen original share type does not match route",
            ));
        }
        if !candidate.daily
            && let Some(link) = original_share.clone()
        {
            return Ok(Share {
                identity: identity(&link),
                link,
            });
        }
        if original_share.is_none()
            && (candidate.daily || original.trim().to_ascii_lowercase().starts_with("http"))
        {
            return Err(parse_error("buerchen original share is invalid"));
        }
        let value = self
            .request(
                ctx,
                "api/other/save_url",
                Some(json!({"url":component(&original),"title":candidate.title})),
            )
            .await?;
        if value.get("code").and_then(Value::as_i64) != Some(200) {
            return Err(self.api_error(value.get("code").and_then(Value::as_i64).unwrap_or(-1)));
        }
        let data = value
            .get("data")
            .ok_or_else(|| parse_error("buerchen save data missing"))?;
        let pass = data
            .get("code")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .or_else(|| data.get("password").and_then(Value::as_str))
            .unwrap_or("");
        let mut link = normalize(data.get("url").and_then(Value::as_str).unwrap_or(""), pass)
            .ok_or_else(|| parse_error("buerchen save did not return a valid share"))?;
        if link.cloud_type != cloud(candidate.kind) {
            return Err(parse_error(
                "buerchen saved share type does not match route",
            ));
        }
        let id = if let Some(original) = original_share {
            let id = identity(&original);
            if identity(&link) == id && link.password.is_none() {
                link.password = original.password;
            }
            id
        } else {
            format!("{}\0{}", candidate.kind, candidate.url)
        };
        let share = Share { link, identity: id };
        let mut state = self.state.lock().unwrap();
        if state.cache.len() >= 256
            && let Some(old) = state
                .cache
                .iter()
                .min_by_key(|(_, v)| v.0)
                .map(|(k, _)| k.clone())
        {
            state.cache.remove(&old);
        }
        state.cache.insert(
            key,
            (Instant::now() + Duration::from_secs(1800), share.clone()),
        );
        Ok(share)
    }
    async fn inner(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        self.cooldown()?;
        let progress = (0..3)
            .map(|_| Arc::new(Mutex::new(vec![])))
            .collect::<Vec<_>>();
        let mut sources = FuturesUnordered::new();
        for index in 0..3 {
            let progress = progress[index].clone();
            sources.push(
                async move { (index, self.web(ctx, query, index * 2, progress).await) }.boxed(),
            );
        }
        sources.push(async move { (3, self.daily(ctx, query).await) }.boxed());
        let mut web: [Vec<Candidate>; 3] = std::array::from_fn(|_| vec![]);
        let mut daily = vec![];
        let mut first_error = None;
        let phase = tokio::time::sleep(ctx.remaining().min(Duration::from_secs(10)));
        tokio::pin!(phase);
        while !sources.is_empty() {
            tokio::select! {biased;
                _=&mut phase=>{first_error.get_or_insert(ProviderError::Timeout);break;}
                value=sources.next()=>{if let Some((index,result))=value{match result{Ok(rows)=>{if index==3{daily=rows;}else{web[index]=rows;}},Err(error)=>{first_error.get_or_insert(error);}}}}
            }
        }
        drop(sources);
        for index in 0..3 {
            if web[index].is_empty() {
                web[index] = progress[index].lock().unwrap().clone();
            }
        }
        let groups: [Vec<Candidate>; 3] = std::array::from_fn(|index| {
            daily
                .iter()
                .filter(|c| c.kind == index * 2)
                .cloned()
                .chain(web[index].clone())
                .take(6)
                .collect()
        });
        let mut entries = vec![];
        for offset in 0..6 {
            for group in &groups {
                if let Some(candidate) = group.get(offset) {
                    entries.push(candidate.clone());
                }
            }
        }
        let mut pending = FuturesUnordered::new();
        for (index, candidate) in entries.into_iter().enumerate() {
            pending.push(async move {
                let result = self.resolve(ctx, &candidate).await;
                (index, candidate, result)
            });
        }
        let mut rows = vec![];
        let mut ids = HashSet::new();
        let mut urls = HashSet::new();
        while let Some((index, candidate, result)) = pending.next().await {
            match result {
                Ok(share) => {
                    if !ids.insert(share.identity.clone())
                        || !urls.insert((share.link.url.clone(), share.link.password.clone()))
                    {
                        continue;
                    }
                    let row = make_result(&candidate, share);
                    ctx.publish(vec![row.clone()]);
                    rows.push((index, row));
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        rows.sort_by_key(|(index, _)| *index);
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(rows.into_iter().map(|(_, row)| row).collect())
    }
}
#[async_trait]
impl Provider for Buerchen {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("buerchen", 3, KeywordFilterMode::Provider)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        tokio::time::timeout(
            ctx.remaining().min(Duration::from_secs(28)),
            self.inner(ctx, query.trim()),
        )
        .await
        .map_err(|_| ProviderError::Timeout)?
    }
}
fn clean_text(value: &str) -> String {
    html::text(scraper::Html::parse_fragment(value).root_element())
}

fn cloud(kind: usize) -> CloudType {
    match kind {
        0 => CloudType::Quark,
        2 => CloudType::Baidu,
        _ => CloudType::Xunlei,
    }
}
fn normalize(raw: &str, hint: &str) -> Option<Link> {
    let nearby = format!("{raw} {hint}");
    let raw = raw.split_whitespace().next()?;
    let url = Url::parse(raw).ok()?;
    if !matches!(
        url.host_str()?,
        "pan.quark.cn" | "pan.baidu.com" | "pan.xunlei.com"
    ) {
        return None;
    }
    let hint = if valid_password(hint) {
        format!("密码：{hint}")
    } else {
        nearby
    };
    let mut link = html::share(raw, &hint, false)?;
    let mut url = Url::parse(&link.url).ok()?;
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(&path);
    link.url = url.to_string();
    Some(link)
}
fn identity(link: &Link) -> String {
    format!(
        "{}\0{}",
        link.cloud_type,
        Url::parse(&link.url).unwrap().path()
    )
}
fn direct(candidate: &Candidate) -> Option<Share> {
    let link = normalize(&candidate.url, &candidate.description)?;
    if link.cloud_type != cloud(candidate.kind) {
        return None;
    }
    Some(Share {
        identity: identity(&link),
        link,
    })
}
fn make_result(candidate: &Candidate, mut share: Share) -> SearchResult {
    let hash = format!("{:x}", Sha256::digest(share.identity.as_bytes()));
    let mut row = SearchResult::new(
        format!("buerchen-{}", &hash[..32]),
        Source::provider("buerchen"),
    );
    row.title = candidate.title.clone();
    row.content = format!(
        "来源：不二资源搜索站{}\n{}",
        if candidate.daily {
            " · 每日更新"
        } else {
            ""
        },
        candidate.description
    );
    row.datetime = Some(chrono::Utc::now());
    share.link.work_title = Some(row.title.clone());
    row.links.push(share.link);
    row
}
/// Browser encodeURIComponent, not form/query encoding.
fn component(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            use std::fmt::Write;
            write!(&mut out, "%{byte:02X}").unwrap();
        }
    }
    out
}
#[cfg(test)]
mod edge_tests;
#[cfg(test)]
mod tests;
