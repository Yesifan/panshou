//! 盘友圈 Next server-action discovery and credential → search → open chain.
use super::markup::matches;
use crate::{
    core::{CloudType, Link, ProviderError, SearchResult},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use reqwest::cookie::{CookieStore, Jar};
use scraper::Html;
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};
use url::Url;
#[derive(Clone, Debug)]
pub struct ActionIds {
    pub credential: String,
    pub intermediate: String,
    pub final_link: String,
}
pub struct Panyq {
    base: Url,
    jar: Arc<Jar>,
    actions: tokio::sync::Mutex<Option<ActionIds>>,
    memo: Mutex<HashMap<String, (Instant, String)>>,
}
impl Default for Panyq {
    fn default() -> Self {
        Self::new(Url::parse("https://panyq.com/").unwrap())
    }
}
static SCRIPTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<script src="(/_next/static/[^"]+\.js)""#).unwrap());
static ACTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"["']([a-f0-9]{40})["']{1}"#).unwrap());
static SIGN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""sign":"([^"]+)""#).unwrap());
static SHA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""sha":"([a-f0-9]{64})""#).unwrap());
static HASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""hash","([^"]+)""#).unwrap());
static URLS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(https?://[^\s"'<>]+|magnet:\?[^\s"'<>]+)"#).unwrap());
static TITLES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [r"《([^》]+)》", r"【([^】]+)】"]
        .into_iter()
        .map(|s| Regex::new(s).unwrap())
        .collect()
});
#[derive(Clone)]
struct Credentials {
    sign: String,
    hash: String,
    sha: String,
}
#[derive(Deserialize, Clone)]
struct Hit {
    eid: String,
    desc: String,
    #[serde(default)]
    size_str: String,
}
#[derive(Deserialize)]
struct Search {
    data: SearchData,
}
#[derive(Deserialize)]
struct SearchData {
    hits: Vec<Hit>,
    #[serde(rename = "maxPageNum")]
    max_page_num: usize,
}
fn clean(s: &str) -> String {
    let mut s = s
        .replace("\\u003c", "<")
        .replace("\\u003e", ">")
        .replace("\\u0026", "&")
        .replace("\\\"", "\"")
        .replace("\\n", "\n");
    for _ in 0..2 {
        s = Html::parse_fragment(&s)
            .root_element()
            .text()
            .collect::<String>();
    }
    s.trim().to_owned()
}
fn title(desc: &str) -> String {
    let desc = clean(desc);
    TITLES
        .iter()
        .find_map(|r| r.captures(&desc).map(|c| c[1].to_owned()))
        .unwrap_or_else(|| desc.split('✔').next().unwrap_or(&desc).trim().to_owned())
}
fn final_link(body: &str) -> Result<String, ProviderError> {
    for line in body.lines().rev() {
        if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(line)
            && let Some(raw) = items
                .get(1)
                .and_then(|v| v["url"].as_str())
                .filter(|s| !s.is_empty())
        {
            return Ok(raw.to_owned());
        }
    }
    URLS.find(body)
        .map(|m| m.as_str().replace("\\/", "/").replace("&amp;", "&"))
        .ok_or_else(|| ProviderError::Parse("panyq final action omitted share URL".into()))
}
fn credentials(body: &str) -> Result<Credentials, ProviderError> {
    let take = |r: &Regex| {
        r.captures(body).map(|c| c[1].to_owned()).ok_or_else(|| {
            ProviderError::Parse("panyq credential action missing sign/hash/sha".into())
        })
    };
    Ok(Credentials {
        sign: take(&SIGN)?,
        sha: take(&SHA)?,
        hash: take(&HASH)?,
    })
}
fn router(section: &str, key: &str, value: &str) -> String {
    let state = serde_json::json!(["",{"children":[section,{"children":[[key,value,"d"],{"children":["__PAGE__",{},format!("/{section}/{value}"),"refresh"]}]}]},null,null,true]);
    let raw = state.to_string();
    let mut encoded = url::form_urlencoded::Serializer::new(String::new());
    encoded.append_pair("t", &raw);
    encoded.finish()[2..].to_owned()
}
impl Panyq {
    pub fn new(base: Url) -> Self {
        Self {
            base,
            jar: Arc::new(Jar::default()),
            actions: tokio::sync::Mutex::new(None),
            memo: Mutex::new(HashMap::new()),
        }
    }
    pub fn with_actions(mut self, ids: ActionIds) -> Self {
        self.actions = tokio::sync::Mutex::new(Some(ids));
        self
    }
    async fn request(
        &self,
        ctx: &SearchContext,
        url: Url,
        payload: Option<Value>,
        action: Option<&str>,
        tree: Option<String>,
        allow_error: bool,
    ) -> Result<String, ProviderError> {
        tokio::time::timeout(ctx.request_timeout(Duration::from_secs(15)), async {
            let mut request = if let Some(body) = payload {
                ctx.client.post(url.clone()).header("content-type", "text/plain;charset=UTF-8").body(body.to_string())
            } else {
                ctx.client.get(url.clone())
            };
            request = request
                .header("user-agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/138.0.0.0 Safari/537.36")
                .header("origin", self.base.origin().ascii_serialization())
                .header("referer", if tree.is_some() { url.as_str() } else { self.base.as_str() })
                .header("sec-ch-ua", r#""Not)A;Brand";v="8", "Chromium";v="138", "Google Chrome";v="138""#);
            if let Some(action) = action {
                request = request.header("next-action", action);
            }
            if let Some(tree) = tree {
                request = request.header("next-router-state-tree", tree);
            }
            if let Some(cookie) = self.jar.cookies(&url) {
                request = request.header("cookie", cookie);
            }
            let response = crate::http::execute(request).await.map_err(|e| ProviderError::Network(e.to_string()))?;
            let status = response.status().as_u16();
            for value in response.headers().get_all("set-cookie") {
                if let Ok(cookie) = value.to_str() {
                    self.jar.add_cookie_str(cookie, response.url());
                }
            }
            let bytes = crate::http::read_response(response, 16 << 20).await.map_err(|e| ProviderError::Network(e.to_string()))?;
            if status != 200 && !allow_error {
                return Err(ProviderError::Protocol(format!("panyq HTTP {status}")));
            }
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        }).await.map_err(|_| ProviderError::Timeout)?
    }
    async fn creds(
        &self,
        ctx: &SearchContext,
        query: &str,
        action: &str,
    ) -> Result<Credentials, ProviderError> {
        let body = self
            .request(
                ctx,
                self.base.clone(),
                Some(serde_json::json!([{"cat":"all","query":query,"pageNum":1}])),
                Some(action),
                None,
                false,
            )
            .await?;
        credentials(&body)
    }
    async fn page(
        &self,
        ctx: &SearchContext,
        creds: &Credentials,
        action: &str,
        page: usize,
    ) -> Result<SearchData, ProviderError> {
        let mut url = self.base.join("api/search").unwrap();
        url.query_pairs_mut()
            .append_pair("sign", &creds.sign)
            .append_pair("page", &page.to_string());
        let body = self
            .request(ctx, url, None, Some(action), None, false)
            .await?;
        serde_json::from_str::<Search>(&body)
            .map(|s| s.data)
            .map_err(|_| {
                ProviderError::Parse("panyq search response missing data.hits/maxPageNum".into())
            })
    }
    async fn intermediate(
        &self,
        ctx: &SearchContext,
        creds: &Credentials,
        action: &str,
        eid: &str,
    ) -> Result<(), ProviderError> {
        let mut url = self.base.join("search/").unwrap();
        url.path_segments_mut()
            .unwrap()
            .pop_if_empty()
            .push(&creds.hash);
        self.request(
            ctx,
            url,
            Some(serde_json::json!([{"eid":eid,"sha":creds.sha,"page_num":"1"}])),
            Some(action),
            Some(router("search", "hash", &creds.hash)),
            false,
        )
        .await?;
        Ok(())
    }
    async fn raw_final(
        &self,
        ctx: &SearchContext,
        action: &str,
        eid: &str,
    ) -> Result<String, ProviderError> {
        let mut url = self.base.join("go/").unwrap();
        url.path_segments_mut().unwrap().pop_if_empty().push(eid);
        self.request(
            ctx,
            url,
            Some(serde_json::json!([{"eid":eid}])),
            Some(action),
            Some(router("go", "eid", eid)),
            false,
        )
        .await
    }
    async fn discover(&self, ctx: &SearchContext) -> Result<ActionIds, ProviderError> {
        let body = self
            .request(ctx, self.base.clone(), None, None, None, false)
            .await?;
        let scripts = SCRIPTS
            .captures_iter(&body)
            .filter_map(|c| self.base.join(&c[1]).ok())
            .collect::<Vec<_>>();
        if scripts.is_empty() {
            return Err(ProviderError::Parse(
                "panyq discovery omitted Next static scripts".into(),
            ));
        }
        let mut potential = Vec::new();
        let mut seen = HashSet::new();
        for script in scripts {
            let js = self.request(ctx, script, None, None, None, true).await?;
            for c in ACTION.captures_iter(&js) {
                if seen.insert(c[1].to_owned()) {
                    potential.push(c[1].to_owned());
                }
            }
        }
        if potential.is_empty() {
            return Err(ProviderError::Parse(
                "panyq static scripts omitted action IDs".into(),
            ));
        }
        let mut candidates = stream::iter(potential.iter().cloned().map(|id| async move {
            let result = self.creds(ctx, "test", &id).await;
            (id, result)
        }))
        .buffered(100);
        let mut credential = None;
        while let Some((id, result)) = candidates.next().await {
            if result.is_ok() {
                credential = Some(id);
                break;
            }
        }
        drop(candidates);
        let credential = credential.ok_or_else(|| {
            ProviderError::Protocol("panyq could not validate credential action".into())
        })?;
        let creds = self.creds(ctx, "test", &credential).await?;
        let remaining = potential
            .into_iter()
            .filter(|id| id != &credential)
            .collect::<Vec<_>>();
        let mut intermediate = None;
        for id in remaining.iter().rev() {
            if self
                .intermediate(ctx, &creds, id, "fake_eid_for_validation")
                .await
                .is_ok()
            {
                intermediate = Some(id.clone());
                break;
            }
        }
        let mut intermediate = intermediate.ok_or_else(|| {
            ProviderError::Protocol("panyq could not validate intermediate action".into())
        })?;
        let hits = self.page(ctx, &creds, &credential, 1).await?.hits;
        let eid = hits.first().map(|h| h.eid.as_str()).ok_or_else(|| {
            ProviderError::Protocol("panyq action discovery test query returned no EID".into())
        })?;
        let final_candidates = remaining
            .iter()
            .filter(|id| *id != &intermediate)
            .cloned()
            .collect::<Vec<_>>();
        for id in &final_candidates {
            if self
                .intermediate(ctx, &creds, &intermediate, eid)
                .await
                .is_ok()
                && let Ok(raw) = self.raw_final(ctx, id, eid).await
                && ["http", "magnet", "aliyundrive", "\"url\""]
                    .iter()
                    .any(|s| raw.contains(s))
            {
                return Ok(ActionIds {
                    credential,
                    intermediate,
                    final_link: id.clone(),
                });
            }
        }
        if final_candidates.len() == 1 && remaining.len() == 2 {
            let final_link = intermediate;
            intermediate = final_candidates[0].clone();
            self.intermediate(ctx, &creds, &intermediate, eid).await?;
            let raw = self.raw_final(ctx, &final_link, eid).await?;
            if ["http", "magnet", "aliyundrive", "\"url\""]
                .iter()
                .any(|s| raw.contains(s))
            {
                return Ok(ActionIds {
                    credential,
                    intermediate,
                    final_link,
                });
            }
        }
        Err(ProviderError::Protocol(
            "panyq could not validate final-link action".into(),
        ))
    }
    async fn actions(
        &self,
        ctx: &SearchContext,
        refresh: bool,
    ) -> Result<ActionIds, ProviderError> {
        let mut state = self.actions.lock().await;
        if !refresh && let Some(ids) = state.as_ref() {
            return Ok(ids.clone());
        }
        let ids = self.discover(ctx).await?;
        *state = Some(ids.clone());
        Ok(ids)
    }
    async fn hit(
        &self,
        ctx: &SearchContext,
        ids: &ActionIds,
        creds: &Credentials,
        index: usize,
        hit: Hit,
        query: &str,
    ) -> Result<Option<SearchResult>, ProviderError> {
        let memo_key = format!("{}\0{}", ids.final_link, hit.eid);
        let cached = self
            .memo
            .lock()
            .unwrap()
            .get(&memo_key)
            .filter(|(t, _)| Instant::now() < *t)
            .map(|(_, s)| s.clone());
        let raw = if let Some(raw) = cached {
            raw
        } else {
            self.intermediate(ctx, creds, &ids.intermediate, &hit.eid)
                .await?;
            let raw = final_link(&self.raw_final(ctx, &ids.final_link, &hit.eid).await?)?;
            let mut memo = self.memo.lock().unwrap();
            if memo.len() >= 256
                && let Some(oldest) = memo
                    .iter()
                    .min_by_key(|(_, (t, _))| *t)
                    .map(|(k, _)| k.clone())
            {
                memo.remove(&oldest);
            }
            memo.insert(
                memo_key,
                (Instant::now() + Duration::from_secs(1800), raw.clone()),
            );
            raw
        };
        let url = Url::parse(&raw)
            .map_err(|_| ProviderError::Parse("panyq final link was not a URL".into()))?;
        if !matches!(url.scheme(), "http" | "https" | "magnet" | "ed2k") {
            return Err(ProviderError::Protocol(
                "panyq unsupported final link scheme".into(),
            ));
        }
        let mut r = SearchResult::provider(format!("panyq-{index}"), "panyq");
        r.title = title(&hit.desc);
        r.content = clean(&hit.desc);
        if !hit.size_str.is_empty() {
            r.tags.push(hit.size_str);
        }
        if !matches(&format!("{} {}", r.title, r.content), query) {
            return Ok(None);
        }
        let mut l = Link::new(raw);
        if l.cloud_type == CloudType::Baidu {
            l.password = url
                .query_pairs()
                .find(|(k, _)| k == "pwd")
                .map(|(_, v)| v.chars().take(4).collect());
        }
        if l.cloud_type == CloudType::Aliyun {
            l.password = url
                .query_pairs()
                .find(|(k, _)| k == "password")
                .map(|(_, v)| v.into_owned());
        }
        l.work_title = Some(r.title.clone());
        r.links.push(l);
        ctx.publish(vec![r.clone()]);
        Ok(Some(r))
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut ids = self.actions(ctx, false).await?;
        let creds = match self.creds(ctx, query, &ids.credential).await {
            Ok(c) => c,
            Err(_) => {
                ids = self.actions(ctx, true).await?;
                self.creds(ctx, query, &ids.credential).await?
            }
        };
        let first = self.page(ctx, &creds, &ids.credential, 1).await?;
        let mut hits = first.hits;
        if hits.is_empty() {
            return Ok(vec![]);
        }
        let mut failure = None;
        let mut pages = stream::iter(
            (2..=first.max_page_num.min(3)).map(|p| self.page(ctx, &creds, &ids.credential, p)),
        )
        .buffered(2);
        while let Some(page) = pages.next().await {
            match page {
                Ok(page) => hits.extend(page.hits),
                Err(e) => failure = Some(e),
            }
        }
        drop(pages);
        let mut jobs = stream::iter(
            hits.into_iter()
                .enumerate()
                .map(|(index, hit)| self.hit(ctx, &ids, &creds, index, hit, query)),
        )
        .buffered(100);
        let mut out = Vec::new();
        while let Some(result) = jobs.next().await {
            match result {
                Ok(Some(r)) => out.push(r),
                Ok(None) => {}
                Err(e) => failure = Some(e),
            }
        }
        if let Some(e) = failure {
            return Err(e);
        }
        Ok(out)
    }
}
#[async_trait]
impl Provider for Panyq {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("panyq", 2, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        tokio::time::timeout(ctx.remaining(), self.run(ctx, query))
            .await
            .map_err(|_| ProviderError::Timeout)?
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_partial_json, header, method, path, query_param},
    };
    #[tokio::test]
    async fn complete_discovery_and_open_chain() {
        let s = MockServer::start().await;
        let cred = "1111111111111111111111111111111111111111";
        let inter = "2222222222222222222222222222222222222222";
        let final_id = "3333333333333333333333333333333333333333";
        let sha = "a".repeat(64);
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"<script src="/_next/static/app.js"></script>"#),
            )
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(path("/_next/static/app.js"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(format!("'{cred}','{inter}','{final_id}'")),
            )
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(header("next-action", cred))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                r#"0:{{"sign":"signed","sha":"{sha}","hash","hashval"}}"#
            )))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(header("next-action", inter))
            .and(path("/search/hashval"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(header("next-action", final_id))
            .and(path("/search/hashval"))
            .respond_with(ResponseTemplate::new(400))
            .mount(&s)
            .await;
        Mock::given(path("/api/search")).and(query_param("sign","signed")).and(query_param("page","1")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data":{"hits":[{"eid":"e1","desc":"《Film》✔1080p","size_str":"1GB"}],"maxPageNum":1}}))).expect(2).mount(&s).await;
        Mock::given(method("POST"))
            .and(header("next-action", final_id))
            .and(header("content-type", "text/plain;charset=UTF-8"))
            .and(path("/go/e1"))
            .and(body_partial_json(serde_json::json!([{"eid":"e1"}])))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "0:metadata\n[null,{\"url\":\"https://pan.baidu.com/s/abc?pwd=abcd\"}]",
            ))
            .mount(&s)
            .await;
        let p = Panyq::new(Url::parse(&s.uri()).unwrap());
        let c = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
        let r = p.search(&c, "Film").await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "Film");
        assert_eq!(r[0].links[0].password.as_deref(), Some("abcd"));
        assert!(
            serde_json::from_str::<Search>(r#"{"data":{"hits":[],"maxPageNum":0}}"#)
                .unwrap()
                .data
                .hits
                .is_empty()
        );
        assert!(serde_json::from_str::<Search>("{}").is_err());
        assert!(final_link("malformed").is_err());
    }
}
