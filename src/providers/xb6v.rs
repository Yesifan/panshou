use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, support::*};
use crate::core::{Link, ProviderError, SearchResult};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use std::{sync::LazyLock, time::Duration};
use url::Url;
static REDIRECT: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r#"(?:window\.)?location\.href\s*=\s*["']([^"']+)["']"#,
        r#"(?:href|url)=["']([^"']*searchid[^"']*)["']"#,
        r"result/\?searchid=\d+",
    ]
    .into_iter()
    .map(|s| Regex::new(s).unwrap())
    .collect()
});
#[derive(Debug, Clone)]
pub struct Xb6v {
    pub base: Url,
    pub options: crate::http::ClientOptions,
}
impl Default for Xb6v {
    fn default() -> Self {
        Self {
            base: Url::parse("https://www.66ss.org/").unwrap(),
            options: crate::http::ClientOptions::default(),
        }
    }
}
impl Xb6v {
    pub fn with_options(base: Url, options: crate::http::ClientOptions) -> Self {
        Self { base, options }
    }
}
fn clean_title(raw: &str) -> String {
    let mut s = raw.to_owned();
    for name in ["6v电影-新版", "6v电影", "新版6v", "新版6V", "6V电影"] {
        s = s.replace(name, " ");
    }
    let s = clean(&s);
    if s.is_empty() {
        "未知标题".into()
    } else {
        s
    }
}
type DetailPage = (Url, Option<chrono::DateTime<chrono::Utc>>);
fn items(bytes: &[u8], base: &Url) -> Result<Vec<DetailPage>, ProviderError> {
    let doc = html(bytes)?;
    let mut rows = vec![];
    for node in doc.select(&sel("ul#post_container li.post")) {
        let Some(href) = attr(node, "a[href*='.html']", "href") else {
            continue;
        };
        let Ok(url) = base.join(&href) else { continue };
        let segments: Vec<_> = url.path().trim_matches('/').split('/').collect();
        let Some(last) = segments.last() else {
            continue;
        };
        if segments.len() < 2
            || !last.ends_with(".html")
            || !last.chars().any(|c| c.is_ascii_digit())
        {
            continue;
        }
        if rows.iter().any(|(known, _): &DetailPage| *known == url) {
            continue;
        }
        rows.push((url, date(&text(node, ".info .info_date"))));
        if rows.len() == 50 {
            break;
        }
    }
    Ok(rows)
}
fn detail(
    bytes: &[u8],
    url: &Url,
    published: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<Vec<SearchResult>, ProviderError> {
    let doc = html(bytes)?;
    let root = doc.root_element();
    let title = clean_title(&text(root, "h1"));
    let category = text(root, ".info_category a");
    let mut anchors = vec![];
    for td in doc.select(&sel("td")) {
        if td.text().collect::<String>().contains("磁力：") {
            anchors.extend(td.select(&sel("a[href^='magnet:']")));
        }
    }
    if anchors.is_empty() {
        anchors.extend(doc.select(&sel("a[href^='magnet:']")));
    }
    let mut rows = vec![];
    let mut seen = std::collections::HashSet::new();
    for anchor in anchors {
        let raw = anchor.value().attr("href").unwrap();
        if !magnet(raw) || !seen.insert(raw.to_owned()) {
            continue;
        }
        let subtitle = clean(&anchor.text().collect::<String>());
        let subtitle = if subtitle.is_empty() {
            "磁力链接".into()
        } else {
            subtitle
        };
        let mut row = SearchResult::provider(
            format!(
                "xb6v-{}-{}",
                id(url.as_str()).unwrap_or_else(|| url.path().into()),
                rows.len()
            ),
            "xb6v",
        );
        row.title = format!("{title}-{subtitle}");
        row.content = format!("分类：{category}\n磁力链接：{subtitle}");
        row.datetime = published;
        row.tags = vec![category.clone()];
        let mut link = Link::new(raw);
        link.work_title = Some(row.title.clone());
        row.links = vec![link];
        rows.push(row);
    }
    if rows.is_empty() {
        return Err(ProviderError::Protocol("xb6v detail has no magnets".into()));
    }
    Ok(rows)
}
#[async_trait]
impl Provider for Xb6v {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("xb6v", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if ctx.is_cancelled() {
            return Err(ProviderError::Timeout);
        }
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let decoded = percent_encoding::percent_decode_str(query)
            .decode_utf8_lossy()
            .replace('+', " ");
        let query = decoded.split_whitespace().next().unwrap_or("");
        let url = self.base.join("e/search/11index.php").unwrap();
        let response = crate::http::execute(
            crate::http::HttpClientFactory::new()
                .no_follow(&self.options)
                .map_err(|e| ProviderError::Protocol(e.to_string()))?
                .post(url)
                .timeout(ctx.request_timeout(Duration::from_secs(30)))
                .header("referer", self.base.as_str())
                .header("user-agent", UA)
                .form(&[
                    ("show", "title"),
                    ("tempid", "1"),
                    ("tbname", "article"),
                    ("mid", "1"),
                    ("dopost", "search"),
                    ("submit", ""),
                    ("keyboard", query),
                ]),
        )
        .await
        .map_err(network)?;
        let status = response.status();
        if status.as_u16() != 200 && !status.is_redirection() {
            return Err(ProviderError::Protocol(format!("HTTP {}", status.as_u16())));
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|s| s.to_str().ok())
            .map(str::to_owned);
        let bytes = crate::http::read_response(response, LIMIT)
            .await
            .map_err(network)?;
        let body_text = String::from_utf8_lossy(&bytes);
        let location = location
            .or_else(|| {
                REDIRECT.iter().find_map(|re| {
                    re.captures(&body_text)
                        .and_then(|c| c.get(1).or_else(|| c.get(0)))
                        .map(|m| m.as_str().to_owned())
                })
            })
            .ok_or_else(|| {
                ProviderError::Protocol("xb6v missing result searchid redirect".into())
            })?;
        let result = if location.starts_with("result/") {
            self.base.join(&format!("e/search/{location}"))
        } else {
            self.base.join(&location)
        }
        .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        if result.origin() != self.base.origin() {
            return Err(ProviderError::Protocol(
                "xb6v cross-origin search redirect rejected".into(),
            ));
        }
        let bytes = body(
            ctx,
            || {
                ctx.client
                    .get(result.clone())
                    .header("referer", self.base.as_str())
            },
            30,
            ONCE,
        )
        .await?;
        let tasks = items(&bytes, &self.base)?;
        collect(
            ctx,
            stream::iter(tasks.into_iter().map(|(url, published)| async move {
                let bytes = body(
                    ctx,
                    || {
                        ctx.client
                            .get(url.clone())
                            .header("referer", self.base.as_str())
                    },
                    30,
                    ONCE,
                )
                .await?;
                detail(&bytes, &url, published)
            }))
            .buffer_unordered(50),
        )
        .await
    }
}
