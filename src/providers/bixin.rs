//! 比心 Flarum search (two offset pages); only Mobile cloud shares are supported.
use super::{UA, clean, fetch, password};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
    time::Duration,
};
use url::Url;
#[derive(Debug, Clone)]
pub struct BixinProvider {
    base: Url,
}
impl Default for BixinProvider {
    fn default() -> Self {
        Self::new(Url::parse("https://www.bixbiy.com/").unwrap())
    }
}
impl BixinProvider {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
}
fn parse(body: &str) -> Result<Vec<SearchResult>, ProviderError> {
    let data: Value = serde_json::from_str(body)
        .map_err(|_| ProviderError::Parse("bixin page: malformed JSON".into()))?;
    let rows = data
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| ProviderError::Parse("bixin page: missing data".into()))?;
    let posts: HashMap<&str, &str> = data
        .get("included")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| {
            Some((
                p.get("id")?.as_str()?,
                p.pointer("/attributes/contentHtml")?.as_str()?,
            ))
        })
        .collect();
    static URL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"https?://caiyun\.139\.com/[^\s\"'<>]+"#).unwrap());
    let mut out = Vec::new();
    for row in rows {
        let Some(id) = row.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(post_id) = row
            .pointer("/relationships/mostRelevantPost/data/id")
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Some(html) = posts.get(post_id) else {
            continue;
        };
        let content = html
            .replace("<br>", "\n")
            .replace("<br/>", "\n")
            .replace("<br />", "\n");
        let fragment = scraper::Html::parse_fragment(&content);
        let text = fragment.root_element().text().collect::<String>();
        let lines: Vec<_> = text.lines().collect();
        let mut links = Vec::new();
        let mut seen = HashSet::new();
        for (i, line) in lines.iter().enumerate() {
            for m in URL.find_iter(line) {
                let raw = m.as_str().replace("&amp;", "&");
                if !seen.insert(raw.clone()) {
                    continue;
                }
                let Ok(url) = Url::parse(&raw) else {
                    continue;
                };
                if url.host_str() != Some("caiyun.139.com") {
                    continue;
                }
                let mut l = Link::new(raw);
                l.cloud_type = CloudType::Mobile;
                l.password = password(&l.url, "").or_else(|| {
                    lines
                        .iter()
                        .enumerate()
                        .filter_map(|(j, line)| {
                            if i.abs_diff(j) > 3
                                || !(line.contains("访问码") || line.contains("密码"))
                            {
                                return None;
                            }
                            let value = line
                                .split_once(':')
                                .or_else(|| line.split_once('：'))?
                                .1
                                .trim();
                            (!value.is_empty() && value.len() <= 10)
                                .then_some((i.abs_diff(j), value))
                        })
                        .min_by_key(|(distance, _)| *distance)
                        .map(|(_, value)| value.to_owned())
                });
                links.push(l);
            }
        }
        if links.is_empty() {
            continue;
        }
        let mut r = SearchResult::provider(format!("bixin-{id}"), "bixin");
        r.title = row
            .pointer("/attributes/title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into();
        r.content = clean(html);
        r.links = links;
        r.datetime = row
            .pointer("/attributes/createdAt")
            .and_then(Value::as_str)
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&chrono::Utc));
        out.push(r);
    }
    Ok(out)
}
#[async_trait]
impl Provider for BixinProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("bixin", 3, KeywordFilterMode::Provider)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let pages = stream::iter([0, 50].into_iter().map(|offset| async move {
            let mut url = self.base.join("api/discussions").unwrap();
            url.query_pairs_mut()
                .append_pair("filter[q]", query)
                .append_pair("include", "mostRelevantPost")
                .append_pair("page[offset]", &offset.to_string())
                .append_pair("page[limit]", "50");
            let body = fetch(
                ctx,
                ctx.client
                    .get(url)
                    .header("User-Agent", UA)
                    .header("Accept", "application/json, text/plain, */*"),
                "bixin",
                "page",
                Duration::from_secs(30),
                16 << 20,
                &[500, 500],
            )
            .await?;
            let mut rows = parse(&body)?;
            rows.retain(|r| {
                let text = format!("{} {}", r.title, r.content).to_lowercase();
                query
                    .split_whitespace()
                    .all(|w| text.contains(&w.to_lowercase()))
            });
            ctx.publish(rows.clone());
            Ok::<_, ProviderError>(rows)
        }))
        .buffer_unordered(2)
        .collect::<Vec<_>>()
        .await;
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut error = None;
        for page in pages {
            match page {
                Ok(rows) => {
                    out.extend(rows.into_iter().filter(|r| seen.insert(r.id.clone())));
                }
                Err(e) => {
                    error.get_or_insert(e);
                }
            }
        }
        if let Some(error) = error {
            return Err(error);
        }
        out.sort_by_key(|row| std::cmp::Reverse(row.datetime));
        Ok(out)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixtures() {
        let rows = parse(include_str!(
            "../../tests/fixtures/providers/bixin/success.json"
        ))
        .unwrap();
        assert_eq!(rows[0].links[0].cloud_type, CloudType::Mobile);
        assert_eq!(rows[0].links[0].password.as_deref(), Some("abcd"));
        assert!(
            parse(include_str!(
                "../../tests/fixtures/providers/bixin/empty.json"
            ))
            .unwrap()
            .is_empty()
        );
        assert!(
            parse(include_str!(
                "../../tests/fixtures/providers/bixin/malformed.json"
            ))
            .is_err()
        );
    }
}
