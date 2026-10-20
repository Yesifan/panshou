//! Panwiki Discuz search redirects and per-work grouped post links.
use super::markup::{crc32, matches, sel, text};
use crate::{
    core::{CloudType, Link, ProviderError, SearchResult},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use chrono::NaiveDateTime;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::Html;
use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
    time::Duration,
};
use url::Url;
#[derive(Clone, Debug)]
pub struct Panwiki {
    hosts: Vec<Url>,
}
impl Default for Panwiki {
    fn default() -> Self {
        Self::new(vec![
            Url::parse("https://www.panwiki.com/").unwrap(),
            Url::parse("https://pan666.net/").unwrap(),
        ])
    }
}
static TID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"tid=(\d+)").unwrap());
static SINGLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^丨]*丨[^：]*：https?://[^\s]+").unwrap());
static LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([^丨]+)丨([^：]+)：(https?://[a-zA-Z0-9\.\-\_\?\=\&\/]+)").unwrap()
});
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").unwrap());
static AD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"【[^】]*(?:论坛|网站|\.com|\.net|\.cn)[^】]*】|\[[^\]]*(?:论坛|网站|\.com|\.net|\.cn)[^\]]*\]").unwrap()
});
static TITLE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"\(\d{4}\)",
        r"\[[^\]]*\]|【[^\]]*】",
        r"【[全\d]+[集\d]*】|【\d+[全集]】|\[\d+[全集]\]|【完结】",
        r"^[A-Za-z]*[^\s]*(?:传|剧|版|之|的|与|和|：|丨|\s)+",
        r"^[\u{4e00}-\u{9fff}\w\s\-\(\)（）]+$",
    ]
    .into_iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});
static PASSWORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:提取码|密码|pwd|password)[：:=]\s*([A-Za-z0-9]+)").unwrap());
static TEXT_LINKS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https://(?:pan\.quark\.cn/s/|pan\.baidu\.com/s/|www\.alipan\.com/s/|pan\.xunlei\.com/s/|cloud\.189\.cn/|pan\.uc\.cn/s/|www\.123pan\.com/s/|www\.123684\.com/s/|115cdn\.com/s/|pan\.pikpak\.com/s/|caiyun\.139\.cn/s/)[a-zA-Z0-9_-]+").unwrap()
});
fn clean(s: &str) -> String {
    TAG.replace_all(s, "")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .trim()
        .to_owned()
}
fn relevant(title: &str, query: &str) -> bool {
    let norm = |s: &str| s.to_lowercase().replace([' ', '.'], "");
    let title = norm(title);
    let query = norm(query);
    if query == "凡人修仙传" {
        return [
            "凡人修仙传",
            "凡人修仙",
            "修仙传",
            "fanrenxiuxianchuan",
            "fanren",
            "xiuxian",
        ]
        .iter()
        .any(|s| title.contains(s));
    }
    title.contains(&query)
}
fn is_title(s: &str) -> bool {
    let s = s.trim();
    if s.len() < 3 {
        return false;
    }
    if TITLE_PATTERNS[..3].iter().any(|r| r.is_match(s))
        || [
            "4K持续更新",
            "集完结",
            "完结",
            "4K高码",
            "持续更新",
            "全集",
            "集】",
            "更新",
            "剧版",
            "真人版",
            "动画版",
        ]
        .iter()
        .any(|p| s.contains(p))
    {
        return true;
    }
    if [
        "导演:",
        "编剧:",
        "主演:",
        "类型:",
        "制片国家",
        "语言:",
        "首播:",
        "集数:",
        "单集片长:",
        "评分:",
        "简介:",
        "链接：",
        "链接:",
        "夸克网盘：",
        "百度网盘：",
        "阿里云盘：",
        "迅雷网盘：",
    ]
    .iter()
    .any(|p| s.starts_with(p))
    {
        return false;
    }
    if s.contains("http") || s.contains(['<', '>']) {
        return false;
    }
    let n = s.chars().count();
    let chinese = s
        .chars()
        .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
        .count();
    ((3..=6).contains(&n) && chinese * 5 >= n * 4)
        || TITLE_PATTERNS[3].is_match(s)
        || ((7..=50).contains(&n) && TITLE_PATTERNS[4].is_match(s))
}
fn kind(raw: &str) -> Option<CloudType> {
    let u = Url::parse(raw).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    Some(match u.host_str()? {
        "pan.quark.cn" => CloudType::Quark,
        "pan.baidu.com" => CloudType::Baidu,
        "www.alipan.com" => CloudType::Aliyun,
        "pan.xunlei.com" => CloudType::Xunlei,
        "cloud.189.cn" => CloudType::Tianyi,
        "pan.uc.cn" => CloudType::Uc,
        "www.123pan.com" | "www.123684.com" => CloudType::Pan123,
        "115cdn.com" => CloudType::One15,
        "pan.pikpak.com" => CloudType::Pikpak,
        "caiyun.139.cn" => CloudType::Mobile,
        _ => return None,
    })
}
fn link(raw: &str, context: &str) -> Option<Link> {
    let mut l = Link::new(raw);
    l.cloud_type = kind(raw)?;
    let url = Url::parse(raw).ok()?;
    l.password = ["pwd", "password", "pass", "code"]
        .into_iter()
        .find_map(|key| {
            url.query_pairs()
                .find(|(k, v)| k == key && !v.is_empty())
                .map(|(_, v)| v.into_owned())
        });
    if l.password.is_none()
        && let Some(pos) = context.find(raw)
    {
        let start = context[..pos]
            .char_indices()
            .rev()
            .nth(19)
            .map(|(i, _)| i)
            .unwrap_or(0);
        let end = context[pos + raw.len()..]
            .char_indices()
            .nth(100)
            .map(|(i, _)| pos + raw.len() + i)
            .unwrap_or(context.len());
        l.password = PASSWORD
            .captures(&context[start..end])
            .map(|c| c[1].to_owned());
    }
    Some(l)
}
fn dedup(links: Vec<Link>) -> Vec<Link> {
    let mut out = Vec::<Link>::new();
    let mut indices = HashMap::<String, usize>::new();
    for l in links {
        let mut url = match Url::parse(&l.url) {
            Ok(u) => u,
            Err(_) => continue,
        };
        let pairs = url
            .query_pairs()
            .filter(|(k, _)| !["pwd", "password", "pass", "code"].contains(&k.as_ref()))
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect::<Vec<_>>();
        url.set_query(None);
        if !pairs.is_empty() {
            url.query_pairs_mut().extend_pairs(pairs);
        }
        let key = url.to_string();
        if let Some(&i) = indices.get(&key) {
            if (l.password.is_some() && out[i].password.is_none())
                || (l.password.is_some() == out[i].password.is_some()
                    && l.url.len() > out[i].url.len())
            {
                out[i] = l;
            }
        } else {
            indices.insert(key, out.len());
            out.push(l);
        }
    }
    out
}
fn all_links(html: &str) -> Vec<Link> {
    let doc = Html::parse_fragment(html);
    let plain = text(doc.root_element());
    let mut links = doc
        .select(&sel("a[href]"))
        .filter_map(|a| a.value().attr("href").and_then(|s| link(s, &plain)))
        .collect::<Vec<_>>();
    links.extend(
        TEXT_LINKS
            .find_iter(&plain)
            .filter_map(|m| link(m.as_str(), &plain)),
    );
    dedup(links)
}
fn details(body: &str, query: &str) -> Result<Vec<Link>, ProviderError> {
    let doc = Html::parse_document(body);
    let content = doc
        .select(&sel(".t_f[id^='postmessage_']"))
        .next()
        .or_else(|| {
            doc.select(&sel(".t_msgfont,.plhin,.message,[id^='postmessage_']"))
                .next()
        })
        .ok_or_else(|| ProviderError::Parse("panwiki missing post content".into()))?;
    let html = content.inner_html();
    let all = all_links(&html);
    if all.len() <= 4 {
        return Ok(all);
    }
    let lines = html.lines().map(clean).collect::<Vec<_>>();
    let valid = lines
        .iter()
        .filter(|s| s.len() >= 10 && SINGLE.is_match(s))
        .count();
    let matching = lines
        .iter()
        .flat_map(|s| LINE.captures_iter(s))
        .any(|c| relevant(&c[1], query));
    let mut links = Vec::new();
    if valid >= 2 && matching {
        for s in lines {
            for c in LINE.captures_iter(&s) {
                if relevant(&c[1], query)
                    && let Some(mut l) = link(&c[3], &s)
                {
                    l.work_title = Some(c[1].trim().into());
                    links.push(l);
                }
            }
        }
        return Ok(dedup(links));
    }
    let mut group = Vec::new();
    let mut title = String::new();
    let mut flush = |group: &mut Vec<String>, title: &str| {
        if relevant(title, query) {
            for mut l in all_links(&group.join("\n")) {
                l.work_title = Some(title.into());
                links.push(l);
            }
        }
        group.clear();
    };
    for raw in html.lines() {
        let line = clean(raw);
        if line.trim().len() < 5 {
            continue;
        }
        if is_title(&line) {
            flush(&mut group, &title);
            title = line;
        }
        if !title.is_empty() {
            group.push(raw.to_owned());
        }
    }
    flush(&mut group, &title);
    Ok(dedup(links))
}
fn search_items(body: &str, base: &Url) -> Result<Vec<(Url, SearchResult)>, ProviderError> {
    let doc = Html::parse_document(body);
    if doc.select(&sel(".slst")).next().is_none() {
        return Err(ProviderError::Parse("panwiki result list missing".into()));
    }
    let mut out = Vec::new();
    for row in doc.select(&sel(".slst ul li.pbw")) {
        let Some(a) = row.select(&sel("h3.xs3 a")).next() else {
            continue;
        };
        let title = AD.replace_all(&text(a), "").trim().to_owned();
        if title.is_empty() {
            continue;
        }
        let Some(url) = a.value().attr("href").and_then(|s| base.join(s).ok()) else {
            continue;
        };
        let id = TID
            .captures(url.as_str())
            .map(|c| c[1].to_owned())
            .unwrap_or_else(|| crc32(url.as_str().as_bytes()).to_string());
        let mut r = SearchResult::provider(format!("panwiki-{id}"), "panwiki");
        r.title = title;
        r.content = row.select(&sel("p")).nth(1).map(text).unwrap_or_default();
        let last = row.select(&sel("p")).last();
        let spans = last
            .map(|p| p.select(&sel("span")).collect::<Vec<_>>())
            .unwrap_or_default();
        if spans.len() >= 3 {
            let raw = text(spans[0]);
            r.datetime = ["%Y-%m-%d %H:%M", "%Y-%m-%d %H:%M:%S"]
                .into_iter()
                .find_map(|fmt| NaiveDateTime::parse_from_str(&raw, fmt).ok())
                .map(|t| t.and_utc());
            let author = spans[1].select(&sel("a")).map(text).collect::<String>();
            let category = spans[2].select(&sel("a")).map(text).collect::<String>();
            if !author.is_empty() || !category.is_empty() {
                r.content = format!("{} | 作者: {author} | 分类: {category}", r.content);
            }
        }
        r.content = format!("{} | 详情: {url}", r.content);
        out.push((url, r));
    }
    Ok(out)
}
impl Panwiki {
    pub fn new(hosts: Vec<Url>) -> Self {
        Self { hosts }
    }
    async fn request(
        &self,
        ctx: &SearchContext,
        url: Url,
        base: &Url,
    ) -> Result<(String, Url, Option<Url>), ProviderError> {
        let r = crate::http::execute(
            ctx.client
                .get(url.clone())
                .timeout(ctx.request_timeout(Duration::from_secs(30)))
                .header("user-agent", "Mozilla/5.0 Chrome/138.0.0.0 Safari/537.36")
                .header("referer", base.as_str())
                .header(
                    "accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
                )
                .header("cache-control", "no-cache")
                .header("pragma", "no-cache"),
        )
        .await
        .map_err(|e| ProviderError::Network(e.to_string()))?;
        let status = r.status().as_u16();
        let final_url = r.url().clone();
        let next = r
            .headers()
            .get("location")
            .and_then(|s| s.to_str().ok())
            .and_then(|s| final_url.join(s).ok());
        if status != 200 && !matches!(status, 301 | 302 | 303 | 307 | 308) {
            return Err(ProviderError::Protocol(format!("panwiki HTTP {status}")));
        }
        if status != 200 && next.is_none() {
            return Err(ProviderError::Protocol(format!(
                "panwiki HTTP {status} missing Location"
            )));
        }
        let bytes = crate::http::read_response(r, 16 << 20)
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        Ok((
            String::from_utf8_lossy(&bytes).into_owned(),
            final_url,
            next,
        ))
    }
    async fn page(
        &self,
        ctx: &SearchContext,
        base: &Url,
        query: &str,
        page: u8,
    ) -> Result<Vec<(Url, SearchResult)>, ProviderError> {
        let mut url = base.join("search.php").unwrap();
        url.query_pairs_mut()
            .append_pair("mod", "forum")
            .append_pair("srchtxt", query)
            .append_pair("searchsubmit", "yes")
            .append_pair("orderby", "lastpost");
        if page > 1 {
            url.query_pairs_mut().append_pair("page", &page.to_string());
        }
        let (mut body, final_url, next) = self.request(ctx, url, base).await?;
        let mut target = next.unwrap_or(final_url);
        if page > 1 && target.query_pairs().any(|(k, _)| k == "searchid") {
            let searchid = target
                .query_pairs()
                .find(|(k, _)| k == "searchid")
                .map(|(_, v)| v.into_owned())
                .unwrap();
            target = base.join("search.php").unwrap();
            target
                .query_pairs_mut()
                .append_pair("mod", "forum")
                .append_pair("searchid", &searchid)
                .append_pair("orderby", "lastpost")
                .append_pair("ascdesc", "desc")
                .append_pair("searchsubmit", "yes")
                .append_pair("page", &page.to_string());
            body = self.request(ctx, target, base).await?.0;
        } else if Html::parse_document(&body)
            .select(&sel(".slst"))
            .next()
            .is_none()
        {
            body = self.request(ctx, target, base).await?.0;
        }
        search_items(&body, base)
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut selected = None;
        let mut last = ProviderError::Unavailable("panwiki no hosts".into());
        for base in &self.hosts {
            match self.page(ctx, base, query, 1).await {
                Ok(v) => {
                    selected = Some((base.clone(), v));
                    break;
                }
                Err(e) => last = e,
            }
        }
        let Some((base, mut items)) = selected else {
            return Err(last);
        };
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut failure = None;
        match self.page(ctx, &base, query, 2).await {
            Ok(v) => items.extend(v),
            Err(e) => failure = Some(e),
        }
        let mut seen = HashSet::new();
        items.retain(|(_, r)| seen.insert(r.id.clone()));
        let mut jobs = stream::iter(items.into_iter().map(|(url, mut r)| {
            let base = base.clone();
            async move {
                let body = self.request(ctx, url, &base).await?.0;
                r.links = details(&body, query)?;
                if r.links.is_empty() || !matches(&format!("{} {}", r.title, r.content), query) {
                    return Ok(None);
                }
                for l in &mut r.links {
                    if l.work_title.is_none() {
                        l.work_title = Some(r.title.clone());
                    }
                }
                ctx.publish(vec![r.clone()]);
                Ok(Some(r))
            }
        }))
        .buffer_unordered(40);
        let mut out = Vec::new();
        while let Some(r) = jobs.next().await {
            match r {
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
impl Provider for Panwiki {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("panwiki", 3, KeywordFilterMode::Provider)
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
    #[test]
    fn multi_work_lines_and_password_dedup() {
        let body = "<div id='postmessage_1' class='t_f'>Film丨夸克：https://pan.quark.cn/s/a\nOther丨夸克：https://pan.quark.cn/s/b\nFilm丨百度：https://pan.baidu.com/s/c?pwd=abcd\nOther丨夸克：https://pan.quark.cn/s/d\nFilm丨夸克：https://pan.quark.cn/s/e</div>";
        let links = details(body, "Film").unwrap();
        assert_eq!(links.len(), 3);
        assert!(
            links
                .iter()
                .all(|l| l.work_title.as_deref() == Some("Film"))
        );
        assert!(links.iter().any(|l| l.password.as_deref() == Some("abcd")));
    }
    #[tokio::test]
    async fn nofollow_full_chain_and_schema() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{path, query_param},
        };
        let s = MockServer::start().await;
        Mock::given(path("/search.php"))
            .and(query_param("srchtxt", "Film"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", "/search.php?mod=forum&searchid=12"),
            )
            .expect(2)
            .mount(&s)
            .await;
        Mock::given(path("/search.php")).and(query_param("searchid","12")).respond_with(ResponseTemplate::new(200).set_body_string("<div class='slst'><ul><li class='pbw'><h3 class='xs3'><a href='/forum.php?mod=viewthread&amp;tid=7'>Film</a></h3><p>stats</p><p>summary</p></li></ul></div>")).expect(2).mount(&s).await;
        Mock::given(path("/forum.php")).and(query_param("tid","7")).respond_with(ResponseTemplate::new(200).set_body_string("<div class='t_f' id='postmessage_7'><a href='https://pan.baidu.com/s/abc?pwd=abcd'>share</a></div>")).expect(1).mount(&s).await;
        let p = Panwiki::new(vec![Url::parse(&s.uri()).unwrap()]);
        let c = SearchContext::new(
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            Duration::from_secs(4),
        );
        let r = p.search(&c, "Film").await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].links[0].password.as_deref(), Some("abcd"));
        assert!(
            search_items("<div class='slst'></div>", &p.hosts[0])
                .unwrap()
                .is_empty()
        );
        assert!(search_items("bad", &p.hosts[0]).is_err());
    }
}
