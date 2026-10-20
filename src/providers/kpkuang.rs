//! 看片狂人 JSONP search, mirror fallback, and encrypted detail links.
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::{Html, Selector};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
    time::Duration,
};
use url::Url;
const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/138.0.0.0 Safari/537.36";
static ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/voddetail/(\d+)(?:/|\.html)?").unwrap());
static COVER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)var\s+def_cover\s*=\s*["']([^"']+)"#).unwrap());
static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\(\d{4}\)\s*$").unwrap());
static PASSWORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:提取码|访问码|密码|取件码|password|pwd)\s*[:：]?\s*([0-9a-zA-Z_-]{3,32})")
        .unwrap()
});
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub beta: Url,
    pub hosts: Vec<Url>,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            beta: Url::parse("https://kpdata.flixfiend.top/esearch/index").unwrap(),
            hosts: [
                "https://www.kpkuang.org/",
                "https://www.kpkuang.fun/",
                "https://www.kpkuang.sbs/",
            ]
            .into_iter()
            .map(|s| Url::parse(s).unwrap())
            .collect(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Kpkuang {
    endpoints: Endpoints,
}
#[derive(Debug, Default, Clone)]
struct Item {
    id: String,
    title: String,
    detail: Option<Url>,
    image: String,
    content: String,
    tags: Vec<String>,
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn resolve(raw: &str, base: &Url) -> String {
    if raw.trim().is_empty() {
        String::new()
    } else {
        base.join(raw.trim())
            .map(|u| u.to_string())
            .unwrap_or_default()
    }
}
fn b64(s: &str) -> Option<Vec<u8>> {
    STANDARD
        .decode(s.trim())
        .or_else(|_| STANDARD_NO_PAD.decode(s.trim().trim_end_matches('=')))
        .ok()
}
fn is_link(s: &str) -> bool {
    let s = s.trim().to_lowercase();
    ["http://", "https://", "magnet:"]
        .iter()
        .any(|p| s.starts_with(p))
}
fn is_password(s: &str) -> bool {
    (3..=32).contains(&s.len()) && !s.contains([' ', '/', '\\', '\t', '\r', '\n'])
}
fn sanitize(s: &str) -> String {
    s.replace(['\u{200b}', '\u{200c}', '\u{200d}', '\u{feff}'], "")
        .trim()
        .to_owned()
}
fn decrypt(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() || s == "@@@encrypted@@@" {
        return String::new();
    }
    let Some(payload) = s.strip_prefix("@@@encrypted@@@") else {
        return if is_link(s) {
            sanitize(s)
        } else {
            b64(s)
                .map(|b| sanitize(&String::from_utf8_lossy(&b)))
                .unwrap_or_default()
        };
    };
    if !payload.is_ascii() || payload.len() < 5 {
        return String::new();
    }
    let n = payload.len() - 5;
    let rotated = format!("{}{}", &payload[n..], &payload[..n]);
    let Some(bytes) = b64(&rotated) else {
        return String::new();
    };
    let decode = |key: &[u8]| {
        let mut b = bytes
            .iter()
            .enumerate()
            .map(|(i, c)| c ^ key[i % key.len()])
            .collect::<Vec<_>>();
        b.reverse();
        String::from_utf8_lossy(&b).trim().to_owned()
    };
    let mut value = decode(b"your_secret_key_here");
    let useful = is_link(&value)
        || is_password(&value)
        || (!value.is_empty()
            && !value
                .chars()
                .any(|c| c < '\u{20}' && !matches!(c, '\t' | '\r' | '\n')));
    if !useful {
        value = decode(b"DYWNn");
    }
    sanitize(&value)
}
fn parse_link(raw: &str, password: &str) -> Option<Link> {
    let raw = sanitize(raw);
    let u = Url::parse(&raw).ok()?;
    if !matches!(u.scheme(), "http" | "https" | "magnet") {
        return None;
    }
    let mut link = Link::new(&raw);
    if u.host_str() == Some("115cdn.com") {
        link.cloud_type = CloudType::One15;
    }
    if link.cloud_type == CloudType::Others {
        return None;
    }
    link.password = if !password.is_empty() {
        Some(password.to_owned())
    } else {
        ["pwd", "password", "passcode", "code"]
            .into_iter()
            .find_map(|key| {
                u.query_pairs()
                    .find(|(k, v)| k == key && !v.trim().is_empty())
                    .map(|(_, v)| v.into_owned())
            })
    };
    Some(link)
}
fn parse_beta(body: &str, base: &Url) -> Result<Vec<Item>, ProviderError> {
    let (start, end) = body
        .find('{')
        .zip(body.rfind('}'))
        .filter(|(a, b)| a < b)
        .ok_or_else(|| ProviderError::Parse("kpkuang invalid JSONP".into()))?;
    let v: Value = serde_json::from_str(&body[start..=end])
        .map_err(|e| ProviderError::Parse(e.to_string()))?;
    if v["code"] != 1 || v["js"].as_str().is_none_or(|s| s.trim().is_empty()) {
        return Ok(vec![]);
    }
    let bytes = b64(v["js"].as_str().unwrap())
        .ok_or_else(|| ProviderError::Parse("kpkuang invalid beta base64".into()))?;
    let values: Vec<Value> =
        serde_json::from_slice(&bytes).map_err(|e| ProviderError::Parse(e.to_string()))?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for v in values {
        let id = match &v["id"] {
            Value::String(s) => s.trim().to_owned(),
            Value::Number(n) => n.to_string(),
            _ => continue,
        };
        let d = &v["data"];
        let field = |k: &str| d[k].as_str().unwrap_or("");
        let title = [field("vod_name"), field("vod_name_org")]
            .into_iter()
            .map(clean)
            .find(|s| !s.is_empty())
            .unwrap_or_default();
        if id.is_empty() || title.is_empty() || !seen.insert(id.clone()) {
            continue;
        }
        let image = [
            field("vod_douban_cover"),
            field("vod_imdb_poster"),
            field("vod_pic"),
        ]
        .into_iter()
        .find(|s| !s.trim().is_empty() && s.trim() != "null")
        .map(|s| resolve(s, base))
        .unwrap_or_default();
        let content = [
            ("主演", "vod_actor"),
            ("导演", "vod_director"),
            ("编剧", "vod_writer"),
            ("地区", "vod_area"),
            ("年份", "vod_year"),
        ]
        .into_iter()
        .filter_map(|(label, k)| {
            let value = clean(field(k));
            (!value.is_empty()).then(|| format!("{label}：{value}"))
        })
        .collect::<Vec<_>>()
        .join(" | ");
        let tags = ["看片狂人", field("vod_area"), field("vod_year")]
            .into_iter()
            .map(clean)
            .filter(|s| !s.is_empty())
            .collect();
        out.push(Item {
            detail: base.join(&format!("voddetail/{id}/")).ok(),
            id,
            title,
            image,
            content,
            tags,
        });
    }
    Ok(out)
}
fn parse_html(body: &str, base: &Url) -> Vec<Item> {
    let doc = Html::parse_document(body);
    let mut nodes = doc
        .select(&sel("a.fed-list-pics[href*='/voddetail/']"))
        .collect::<Vec<_>>();
    if nodes.is_empty() {
        nodes = doc.select(&sel("a[href*='/voddetail/']")).collect();
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for a in nodes {
        let Some(detail) = a.value().attr("href").and_then(|s| base.join(s).ok()) else {
            continue;
        };
        let Some(id) = ID.captures(detail.as_str()).map(|c| c[1].to_owned()) else {
            continue;
        };
        let title = clean(
            a.value()
                .attr("title")
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(&a.text().collect::<String>()),
        );
        if title.is_empty() || !seen.insert(id.clone()) {
            continue;
        }
        let scope = a.parent().and_then(scraper::ElementRef::wrap).unwrap_or(a);
        let image = scope
            .select(&sel("img"))
            .next()
            .and_then(|i| {
                i.value()
                    .attr("data-original")
                    .or_else(|| i.value().attr("src"))
            })
            .map(|s| resolve(s, base))
            .unwrap_or_default();
        out.push(Item {
            id,
            title,
            detail: Some(detail),
            image,
            content: clean(&scope.text().collect::<String>()),
            tags: vec!["看片狂人".into()],
        });
    }
    out
}
fn parse_detail(body: &str, page: &Url, item: &Item) -> SearchResult {
    let doc = Html::parse_document(body);
    let mut result = SearchResult::provider(format!("kpkuang-{}", item.id), "kpkuang");
    let title = doc
        .select(&sel("h1.uk-card-title"))
        .next()
        .map(|n| clean(&n.text().collect::<String>()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| item.title.clone());
    result.title = YEAR.replace(&title, "").trim().to_owned();
    let image = doc
        .select(&sel("#cover_showbox[data-original]"))
        .next()
        .and_then(|n| n.value().attr("data-original"))
        .map(|s| resolve(s, page))
        .filter(|s| !s.is_empty())
        .or_else(|| COVER.captures(body).map(|c| resolve(&c[1], page)))
        .unwrap_or_else(|| item.image.clone());
    if !image.is_empty() {
        result.images.push(image);
    }
    let mut meta = HashMap::new();
    for li in doc.select(&sel(".vodbox li")) {
        let text = clean(&li.text().collect::<String>());
        for label in [
            "主演", "导演", "编剧", "分类", "地区", "年份", "更新", "TAG", "简介",
        ] {
            if let Some(value) = text.strip_prefix(&format!("{label}：")) {
                meta.insert(label, clean(value));
                break;
            }
        }
    }
    let mut parts = ["主演", "导演", "编剧", "分类", "地区", "年份", "TAG"]
        .into_iter()
        .filter_map(|k| {
            meta.get(k)
                .filter(|s| !s.is_empty())
                .map(|v| format!("{k}：{v}"))
        })
        .collect::<Vec<_>>();
    if let Some(intro) = meta.get("简介") {
        parts.push(intro.clone());
    }
    result.content = if parts.is_empty() {
        item.content.clone()
    } else {
        parts.join(" | ")
    };
    result.tags = item.tags.clone();
    result.tags.push("看片狂人".into());
    for key in ["分类", "地区", "年份", "TAG"] {
        if let Some(v) = meta.get(key) {
            result.tags.push(v.clone());
        }
    }
    let mut tags = HashSet::new();
    result
        .tags
        .retain(|s| !s.is_empty() && tags.insert(s.clone()));
    let mut seen = HashSet::new();
    for row in doc.select(&sel(
        "td[id^='td-'],li[id^='magnet-'],li[id^='pan-'],li[id^='xunlei-']",
    )) {
        let mut values = Vec::new();
        let mut password = String::new();
        for n in row.select(&sel("[data-pan-url],[data-clipboard-text]")) {
            for attr in ["data-pan-url", "data-clipboard-text"] {
                if let Some(value) = n.value().attr(attr) {
                    let value = decrypt(value);
                    if is_link(&value) {
                        values.push(value);
                    } else if password.is_empty() && is_password(&value) {
                        password = value;
                    }
                }
            }
        }
        if password.is_empty() {
            password = PASSWORD
                .captures(&clean(&row.text().collect::<String>()))
                .map(|c| c[1].to_owned())
                .unwrap_or_default();
        }
        for value in values {
            if let Some(mut link) = parse_link(&value, &password) {
                let key = (link.cloud_type, link.url.clone(), link.password.clone());
                if seen.insert(key) {
                    link.work_title = Some(result.title.clone());
                    result.links.push(link);
                    if result.links.len() == 24 {
                        return result;
                    }
                }
            }
        }
    }
    result
}
impl Kpkuang {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
    async fn fetch(
        &self,
        ctx: &SearchContext,
        url: Url,
        referer: &Url,
        seconds: u64,
        beta: bool,
    ) -> Result<String, ProviderError> {
        let mut request = ctx
            .client
            .get(url)
            .header("user-agent", UA)
            .header(
                "accept",
                "text/html,application/xhtml+xml,application/json;q=0.9,*/*;q=0.8",
            )
            .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
            .header("referer", referer.as_str());
        if beta {
            request = request
                .header("origin", referer.origin().ascii_serialization())
                .header("sec-fetch-dest", "empty")
                .header("sec-fetch-mode", "cors");
        }
        tokio::time::timeout(Duration::from_secs(seconds), async {
            let response = crate::http::execute(request)
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?;
            if response.status().as_u16() != 200 {
                return Err(ProviderError::Protocol(format!(
                    "kpkuang HTTP {}",
                    response.status()
                )));
            }
            let body = crate::http::read_response(response, 8 << 20)
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?;
            Ok(String::from_utf8_lossy(&body).into_owned())
        })
        .await
        .map_err(|_| ProviderError::Timeout)?
    }
    fn hosts(&self, item: &Item) -> Vec<Url> {
        let mut hosts = Vec::new();
        if let Some(incoming) = &item.detail {
            for host in self.endpoints.hosts.iter().skip(1) {
                if host.origin() != incoming.origin() {
                    hosts.push(host.clone());
                }
            }
            if let Ok(u) = Url::parse(&format!("{}/", incoming.origin().ascii_serialization())) {
                hosts.push(u);
            }
        }
        for h in &self.endpoints.hosts {
            if !hosts.iter().any(|u| u.origin() == h.origin()) {
                hosts.push(h.clone());
            }
        }
        hosts
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let base = self
            .endpoints
            .hosts
            .first()
            .ok_or_else(|| ProviderError::Protocol("kpkuang no hosts".into()))?;
        let mut url = self.endpoints.beta.clone();
        url.query_pairs_mut()
            .append_pair("kw", query)
            .append_pair("ts", &chrono::Utc::now().timestamp_millis().to_string())
            .append_pair("callback", "__kpkuang_cb");
        let beta = match self.fetch(ctx, url, base, 15, true).await {
            Ok(body) => parse_beta(&body, base),
            Err(e) => Err(e),
        };
        let mut error = None;
        let mut items = match beta {
            Ok(v) => v,
            Err(e) => {
                error = Some(e);
                vec![]
            }
        };
        if items.is_empty() {
            for host in &self.endpoints.hosts {
                let mut url = host
                    .join("vodsearch/-------------.html")
                    .map_err(|e| ProviderError::Protocol(e.to_string()))?;
                url.query_pairs_mut()
                    .append_pair("wd", query)
                    .append_pair("limit", "500");
                match self.fetch(ctx, url, host, 15, false).await {
                    Ok(body) => {
                        items = parse_html(&body, host);
                        if !items.is_empty() {
                            break;
                        }
                    }
                    Err(e) => error = Some(e),
                }
            }
        }
        if items.is_empty() {
            return if let Some(e) = error {
                Err(e)
            } else {
                Ok(vec![])
            };
        }
        let mut jobs = stream::iter(items.into_iter().take(10).map(|item| async move {
            let mut last = None;
            for host in self.hosts(&item) {
                let url = host.join(&format!("voddetail/{}/", item.id)).unwrap();
                match self.fetch(ctx, url.clone(), &host, 6, false).await {
                    Ok(body) => {
                        let r = parse_detail(&body, &url, &item);
                        if !r.links.is_empty() {
                            let hay = format!("{} {}", r.title, r.content).to_lowercase();
                            if query
                                .to_lowercase()
                                .split_whitespace()
                                .all(|w| hay.contains(w))
                            {
                                ctx.publish(vec![r.clone()]);
                            }
                            return Ok(r);
                        }
                        last = Some(ProviderError::Parse(
                            "kpkuang detail contains no supported links".into(),
                        ));
                    }
                    Err(e) => last = Some(e),
                }
            }
            Err(last.unwrap_or_else(|| ProviderError::Protocol("kpkuang no detail host".into())))
        }))
        .buffered(4);
        let mut out = Vec::new();
        let mut last = None;
        while let Some(r) = jobs.next().await {
            match r {
                Ok(r) => {
                    let hay = format!("{} {}", r.title, r.content).to_lowercase();
                    if query
                        .to_lowercase()
                        .split_whitespace()
                        .all(|w| hay.contains(w))
                    {
                        out.push(r);
                    }
                }
                Err(e) => last = Some(e),
            }
        }
        if let Some(e) = last {
            return Err(e);
        }
        Ok(out)
    }
}
#[async_trait]
impl Provider for Kpkuang {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("kpkuang", 3, KeywordFilterMode::Provider)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        tokio::time::timeout(ctx.timeout, self.run(ctx, query.trim()))
            .await
            .map_err(|_| ProviderError::Timeout)?
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const ALI: &str = "@@@encrypted@@@ydCHw0QMzBwGEoUMAtLFxMQHREcKgoMDxNLAygcSlZlGxUGERE=MwYsF";
    const BAIDU: &str = "@@@encrypted@@@GIXEhNNJAxpHxY8LgoiOiNOAkwxABAWPCAyA25EFlYyBwZcEB0GFBBxHQQTXUpOLBsRDTc=TQEHB";
    #[test]
    fn real_decryption_vectors() {
        assert_eq!(decrypt(ALI), "https://www.aliyundrive.com/s/DVbnz1xeYiJ");
        assert_eq!(
            decrypt(BAIDU),
            "https://pan.baidu.com/s/1wWR_sc_C9m7FHGbqEst6xA?pwd=vrn4"
        );
        assert_eq!(decrypt("@@@encrypted@@@"), "");
        assert_eq!(sanitize("a\u{200b}b"), "ab");
    }
    #[test]
    fn metadata_and_duplicate_rows() {
        let page = Url::parse("https://www.kpkuang.fun/voddetail/1/").unwrap();
        let body = format!(
            r#"<a id="cover_showbox" data-original="/poster.jpg"></a><h1 class="uk-card-title">测试片 (2024)</h1><div class="vodbox"><li>导演：张三</li><li>地区：中国</li><li>简介：内容</li></div><table><td id="td-pan-0"><a data-pan-url="{ALI}"></a></td><td id="td-pan-1"><a data-clipboard-text="{ALI}"></a></td><td id="td-pan-2"><a data-pan-url="{BAIDU}"></a><span>提取码：abcd</span></td></table>"#
        );
        let r = parse_detail(&body, &page, &Item::default());
        assert_eq!(r.title, "测试片");
        assert_eq!(r.images, ["https://www.kpkuang.fun/poster.jpg"]);
        assert!(r.content.contains("导演：张三"));
        assert_eq!(r.links.len(), 2);
        assert_eq!(r.links[1].password.as_deref(), Some("abcd"));
    }
    #[test]
    fn beta_html_empty_and_malformed() {
        let base = Url::parse("https://www.kpkuang.org/").unwrap();
        let data = serde_json::json!([{"id":625161,"data":{"vod_name":"杀手","vod_year":"2023","vod_pic":"upload/a.jpg","vod_actor":"演员"}},{"id":"625161","data":{"vod_name":"重复"}}]);
        let encoded = STANDARD_NO_PAD.encode(serde_json::to_vec(&data).unwrap());
        let r = parse_beta(&format!(r#"cb({{"code":1,"js":"{encoded}"}})"#), &base).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, "625161");
        assert_eq!(r[0].image, "https://www.kpkuang.org/upload/a.jpg");
        assert!(r[0].content.contains("主演：演员"));
        assert!(parse_beta("broken", &base).is_err());
        assert!(
            parse_beta(r#"cb({"code":1,"js":"W10="})"#, &base)
                .unwrap()
                .is_empty()
        );
        let r = parse_html(
            r#"<ul><li><a class="fed-list-pics" href="/voddetail/123/" title="测试"><img data-original="/x.jpg"></a></li></ul>"#,
            &base,
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, "123");
        assert!(parse_html("<html/>", &base).is_empty());
    }
    #[tokio::test]
    async fn beta_failure_html_and_mirror_detail_full_chain() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{header, method, path, query_param},
        };
        let primary = MockServer::start().await;
        let mirror = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/beta"))
            .and(query_param("kw", "Film"))
            .and(query_param("callback", "__kpkuang_cb"))
            .and(header("origin", primary.uri()))
            .respond_with(ResponseTemplate::new(503))
            .expect(1)
            .mount(&primary)
            .await;
        Mock::given(path("/vodsearch/-------------.html")).and(query_param("wd","Film")).and(query_param("limit","500")).respond_with(ResponseTemplate::new(200).set_body_string(r#"<li><a class="fed-list-pics" href="/voddetail/1/" title="Film Search">Search</a></li>"#)).expect(1).mount(&primary).await;
        Mock::given(path("/voddetail/1/")).respond_with(ResponseTemplate::new(200).set_body_string(format!(r#"<h1 class="uk-card-title">Film Detail (2024)</h1><a id="cover_showbox" data-original="/poster.jpg"></a><table><tr><td id="td-1"><a data-pan-url="{ALI}"></a></td></tr></table>"#))).expect(1).mount(&mirror).await;
        Mock::given(path("/voddetail/1/"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&primary)
            .await;
        let p = Kpkuang::new(Endpoints {
            beta: Url::parse(&format!("{}/beta", primary.uri())).unwrap(),
            hosts: vec![
                Url::parse(&primary.uri()).unwrap(),
                Url::parse(&mirror.uri()).unwrap(),
            ],
        });
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(5));
        let rows = p.search(&ctx, "Film").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Film Detail");
        assert_eq!(rows[0].links[0].work_title.as_deref(), Some("Film Detail"));
        assert_eq!(rows[0].images, [format!("{}/poster.jpg", mirror.uri())]);
        assert!(ctx.has_results());
    }
    #[test]
    fn mirror_order() {
        let p = Kpkuang::default();
        let a = Item {
            detail: Some(p.endpoints.hosts[0].join("voddetail/1/").unwrap()),
            ..Item::default()
        };
        let h = p.hosts(&a);
        assert!(h[0].as_str().contains(".fun"));
        assert!(h[1].as_str().contains(".sbs"));
        let a = Item {
            detail: Some(p.endpoints.hosts[1].clone()),
            ..a
        };
        assert!(p.hosts(&a)[0].as_str().contains(".sbs"));
    }
}
