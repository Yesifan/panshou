//! 灵迹公开影视 API: getVideoList → getVideoDetail, online/cloud/magnet seeds.
use super::support::*;
use crate::{
    core::{CloudType, Link, ProviderError, SearchResult},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    time::Duration,
};
use url::Url;
const APP_ID: &str = "83768d9ad4";
const IDENTITY: &str = "23734adac0301bccdcb107c4aa21f96c";
#[derive(Debug, Clone)]
pub struct Lingjisp {
    base: Url,
}
impl Default for Lingjisp {
    fn default() -> Self {
        Self::new(Url::parse("https://web5.mukaku.com/prod/api/v1/").unwrap())
    }
}
fn id(item: &Value) -> Option<u64> {
    item["doub_id"]
        .as_u64()
        .filter(|v| *v > 0)
        .or_else(|| item["id"].as_u64().filter(|v| *v > 0))
}
fn string<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn strings(v: &Value, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .map(|k| string(v, k).trim())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
impl Lingjisp {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
    async fn get(
        &self,
        ctx: &SearchContext,
        path: &str,
        query: &[(&str, String)],
        seconds: u64,
    ) -> Result<Value, ProviderError> {
        let mut url = self
            .base
            .join(path)
            .map_err(|_| protocol("lingjisp invalid base"))?;
        url.query_pairs_mut()
            .append_pair("app_id", APP_ID)
            .append_pair("identity", IDENTITY)
            .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())));
        let bytes = request(
            ctx,
            browser(
                ctx.client.get(url),
                self.base.as_str(),
                "application/json,text/plain,*/*",
            ),
            3,
            Duration::from_secs(seconds),
            LIMIT,
        )
        .await?;
        let value: Value = serde_json::from_slice(&bytes).map_err(parse)?;
        if value["success"].as_bool() != Some(true) || value["code"].as_i64() != Some(200) {
            return Err(protocol(format!(
                "lingjisp API success={}, code={}",
                value["success"], value["code"]
            )));
        }
        Ok(value)
    }
    fn convert(&self, item: Value) -> Result<SearchResult, ProviderError> {
        let id = id(&item).ok_or_else(|| protocol("lingjisp detail omitted positive ID"))?;
        let mut row = SearchResult::provider(format!("lingjisp-{id}"), "lingjisp");
        row.title = string(&item, "title").trim().into();
        if row.title.is_empty() {
            return Err(protocol("lingjisp detail omitted title"));
        }
        let mut parts = strings(
            &item,
            &["years", "production_area", "director", "performer"],
        );
        let summary = string(&item, "abstract")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if !summary.is_empty() {
            parts.push(summary);
        }
        row.content = short(&parts.join(" | "), 300);
        row.tags = strings(&item, &["years", "class", "zqxd", "ejs"]);
        row.datetime = ["seed_updated_at", "updated_at"]
            .iter()
            .find_map(|k| date(string(&item, k)));
        if !string(&item, "image").trim().is_empty() {
            row.images.push(string(&item, "image").trim().into());
        }
        let mut seen = HashSet::new();
        let mut add = |raw: &str, code: &str, fallback: &str| {
            let raw = raw.trim();
            if raw.is_empty() {
                return;
            }
            let mut link = Link::new(raw);
            if link.cloud_type == CloudType::Others {
                link.cloud_type = cloud(fallback);
            }
            link.password = (!code.trim().is_empty())
                .then(|| code.trim().to_owned())
                .or_else(|| query_password(raw));
            if seen.insert((link.cloud_type, link.url.clone(), link.password.clone())) {
                row.links.push(link);
            }
        };
        if let Some(groups) = item["ecca"].as_object() {
            let ordered = groups.iter().collect::<BTreeMap<_, _>>();
            for seeds in ordered.values() {
                if let Some(seeds) = seeds.as_array() {
                    for seed in seeds {
                        add(string(seed, "zlink"), "", "magnet");
                    }
                }
            }
        }
        if let Some(seeds) = item["all_seeds"].as_array() {
            for seed in seeds {
                add(string(seed, "zlink"), "", "magnet");
            }
        }
        if let Some(groups) = item["movies_online_seed"].as_object() {
            let ordered = groups.iter().collect::<BTreeMap<_, _>>();
            for (kind, seeds) in ordered {
                if let Some(seeds) = seeds.as_array() {
                    for seed in seeds {
                        let fallback = if string(seed, "type").is_empty() {
                            kind.as_str()
                        } else {
                            string(seed, "type")
                        };
                        add(string(seed, "link"), string(seed, "code"), fallback);
                    }
                }
            }
        }
        Ok(row)
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let value = self
            .get(
                ctx,
                "getVideoList",
                &[
                    ("sb", query.into()),
                    ("page", "1".into()),
                    ("limit", "20".into()),
                ],
                20,
            )
            .await?;
        let items = value
            .pointer("/data/data")
            .and_then(Value::as_array)
            .filter(|v| !v.is_empty())
            .or_else(|| value.pointer("/data/list").and_then(Value::as_array))
            .or_else(|| value.pointer("/data/data").and_then(Value::as_array))
            .ok_or_else(|| protocol("lingjisp response omitted data/list"))?;
        let mut seen = HashSet::new();
        let ids = items
            .iter()
            .filter_map(id)
            .filter(|id| seen.insert(*id))
            .collect::<Vec<_>>();
        let mut jobs = stream::iter(ids.into_iter().map(|id| async move {
            let detail = self
                .get(ctx, "getVideoDetail", &[("id", id.to_string())], 12)
                .await?;
            let row = self.convert(detail["data"].clone())?;
            Ok::<_, ProviderError>(publish(ctx, row, query))
        }))
        .buffer_unordered(6);
        let mut rows = Vec::new();
        let mut error = None;
        while let Some(result) = jobs.next().await {
            match result {
                Ok(Some(row)) => rows.push(row),
                Ok(None) => {}
                Err(e) => error = Some(e),
            }
        }
        if let Some(e) = error {
            return Err(e);
        }
        Ok(rows)
    }
}
#[async_trait]
impl Provider for Lingjisp {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("lingjisp", 2, KeywordFilterMode::Core)
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
