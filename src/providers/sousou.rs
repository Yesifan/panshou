use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, support::*};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use serde::Deserialize;
use std::sync::LazyLock;
use url::Url;
static DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d{4}-\d{2}-\d{2}(?: \d{2}:\d{2}:\d{2})?").unwrap());
static CODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:提取码|密码|pwd)[:：]?\s*([a-z0-9]{4})(?:[^a-z0-9]|$)").unwrap()
});
#[derive(Debug, Clone)]
pub struct Sousou {
    pub api: Url,
    pub web: Url,
}
impl Default for Sousou {
    fn default() -> Self {
        Self {
            api: Url::parse("https://sousou.pro/api.php").unwrap(),
            web: Url::parse("https://www.panso.vip/search").unwrap(),
        }
    }
}
struct Document {
    url: Url,
    title: String,
    content: String,
    datetime: Option<chrono::DateTime<chrono::Utc>>,
    password: Option<String>,
}
fn code(raw: &str) -> Option<String> {
    CODE.captures(raw)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().into())
}
fn title(raw: &str) -> String {
    raw.split(" - 网盘搜索").next().unwrap_or(raw).trim().into()
}
fn documents(bytes: &[u8], base: &Url) -> Result<Vec<Document>, ProviderError> {
    let doc = html(bytes)?;
    let mut items = vec![];
    for node in doc.select(&sel("div.search-item")) {
        let Some(href) = attr(node, "a.search-item-title[href]", "href") else {
            continue;
        };
        let Ok(url) = base.join(&href) else { continue };
        // Adjacent metadata elements are separate text fields, not a suffix
        // of the four-character access code in the description.
        let raw = node.text().collect::<Vec<_>>().join(" ");
        let meta = text(node, ".search-item-meta-item");
        items.push(Document {
            url,
            title: text(node, "a.search-item-title"),
            content: text(node, ".search-item-info"),
            datetime: DATE.find(&meta).and_then(|m| date(m.as_str())),
            password: code(&raw)
                .or_else(|| attr(node, "a[href*='pwd=']", "href").and_then(|s| password(&s))),
        });
    }
    Ok(items)
}
fn document(bytes: &[u8], item: Document) -> Result<Vec<SearchResult>, ProviderError> {
    let doc = html(bytes)?;
    let root = doc.root_element();
    let raw = attr(root, "a.jump-link[href]", "href")
        .ok_or_else(|| ProviderError::Protocol("sousou document missing jump link".into()))?;
    let mut link = Link::new(&raw);
    if link.cloud_type == CloudType::Others {
        return Err(ProviderError::Protocol(
            "sousou unsupported jump link".into(),
        ));
    }
    let heading = title(&text(root, ".resource-box h1"));
    let heading = if heading.is_empty() {
        title(&item.title)
    } else {
        heading
    };
    let mut published = item.datetime;
    for label in doc.select(&sel(".description-label")) {
        if clean(&label.text().collect::<String>()) == "分享时间"
            && let Some(value) = label
                .next_siblings()
                .filter_map(scraper::ElementRef::wrap)
                .next()
        {
            published = date(&clean(&value.text().collect::<String>())).or(published);
        }
    }
    link.password = password(&raw)
        .or(item.password)
        .or_else(|| code(&root.text().collect::<Vec<_>>().join(" ")));
    link.datetime = published;
    link.work_title = Some(heading.clone());
    let mut hash = 2166136261u32;
    for b in item.url.path().bytes() {
        hash = (hash ^ u32::from(b)).wrapping_mul(16777619);
    }
    let mut row = SearchResult::provider(format!("sousou-{hash:x}"), "sousou");
    row.title = heading;
    row.datetime = published;
    row.content = format!("{}\n{}", item.content, text(root, ".resource-files"))
        .trim()
        .into();
    row.links = vec![link];
    Ok(vec![row])
}
#[derive(Deserialize)]
struct Response {
    code: i32,
    data: Data,
}
#[derive(Deserialize)]
struct Data {
    list: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    #[serde(default)]
    disk_id: String,
    disk_name: String,
    link: String,
    #[serde(default)]
    disk_pass: String,
    #[serde(default)]
    files: String,
    #[serde(default)]
    shared_time: String,
    #[serde(default)]
    tags: Option<Vec<serde_json::Value>>,
}
fn api_rows(bytes: &[u8]) -> Result<Vec<SearchResult>, ProviderError> {
    let response: Response =
        serde_json::from_slice(bytes).map_err(|e| ProviderError::Parse(e.to_string()))?;
    if response.code != 200 {
        return Err(ProviderError::Protocol(format!(
            "sousou API code {}",
            response.code
        )));
    }
    Ok(response
        .data
        .list
        .into_iter()
        .filter_map(|item| {
            if item.link.trim().is_empty() {
                return None;
            }
            let mut link = Link::new(&item.link);
            if link.cloud_type == CloudType::Others {
                return None;
            }
            link.password = password(&item.link)
                .or_else(|| (!item.disk_pass.is_empty()).then_some(item.disk_pass));
            let mut row = SearchResult::provider(
                format!(
                    "sousou-{}",
                    if item.disk_id.is_empty() {
                        item.link
                    } else {
                        item.disk_id
                    }
                ),
                "sousou",
            );
            row.title = item.disk_name;
            row.content = item.files;
            row.datetime = date(&item.shared_time);
            row.tags = item
                .tags
                .unwrap_or_default()
                .into_iter()
                .filter_map(|v| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned))
                .collect();
            row.links = vec![link];
            Some(row)
        })
        .collect())
}
fn deduplicate(rows: Vec<SearchResult>) -> Vec<SearchResult> {
    let mut unique: Vec<SearchResult> = vec![];
    for mut row in rows {
        if let Some(old) = unique.iter_mut().find(|old| old.id == row.id) {
            let score = row.content.len()
                + if old.links[0].password.is_none() && row.links[0].password.is_some() {
                    5
                } else {
                    0
                }
                + if old.datetime.is_none() && row.datetime.is_some() {
                    3
                } else {
                    0
                }
                + if old.tags.is_empty() && !row.tags.is_empty() {
                    2
                } else {
                    0
                };
            if score > old.content.len() {
                row.links[0].password = row.links[0]
                    .password
                    .clone()
                    .or_else(|| old.links[0].password.clone());
                *old = row;
            } else if old.links[0].password.is_none() {
                old.links[0].password = row.links[0].password.take();
            }
        } else {
            unique.push(row);
        }
    }
    unique
}
impl Sousou {
    async fn search_web(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self.web.clone();
        url.query_pairs_mut().append_pair("q", query);
        let bytes = body(
            ctx,
            || {
                ctx.client
                    .get(url.clone())
                    .header("referer", self.web.as_str())
            },
            30,
            ONCE,
        )
        .await?;
        let items = documents(&bytes, &self.web)?;
        collect(
            ctx,
            stream::iter(items.into_iter().map(|item| async move {
                let bytes = body(
                    ctx,
                    || {
                        ctx.client
                            .get(item.url.clone())
                            .header("referer", self.web.as_str())
                    },
                    20,
                    ONCE,
                )
                .await?;
                document(&bytes, item)
            }))
            .buffer_unordered(8),
        )
        .await
    }
}
#[async_trait]
impl Provider for Sousou {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("sousou", 3, KeywordFilterMode::Core)
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
        match self.search_web(ctx, query).await {
            Ok(rows) if !rows.is_empty() => return Ok(rows),
            Err(error) if ctx.has_results() => return Err(error),
            _ => {}
        }
        let tasks: Vec<_> = ["QUARK", "BDY", "ALY", "XUNLEI", "UC", "115"]
            .into_iter()
            .flat_map(|kind| (1..=3).map(move |page| (kind.to_owned(), page)))
            .collect();
        collect(
            ctx,
            stream::iter(tasks.into_iter().map(|(kind, page)| async move {
                let mut url = self.api.clone();
                url.query_pairs_mut()
                    .append_pair("action", "search")
                    .append_pair("q", query)
                    .append_pair("page", &page.to_string())
                    .append_pair("per_size", "30")
                    .append_pair("type", &kind);
                let bytes = body(
                    ctx,
                    || {
                        ctx.client
                            .get(url.clone())
                            .header("referer", self.api.as_str())
                    },
                    30,
                    ONCE,
                )
                .await?;
                api_rows(&bytes)
            }))
            .buffer_unordered(6),
        )
        .await
        .map(deduplicate)
    }
}
