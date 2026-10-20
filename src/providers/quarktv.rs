use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, support::*};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use serde::Deserialize;
use url::Url;
#[derive(Debug, Clone)]
pub struct Quarktv {
    pub base: Url,
}
impl Default for Quarktv {
    fn default() -> Self {
        Self {
            base: Url::parse("https://www.quarktv.com/").unwrap(),
        }
    }
}
#[derive(Clone)]
struct Article {
    id: String,
    title: String,
    url: Url,
    image: String,
    summary: String,
    category: String,
    published: String,
}
fn articles(bytes: &[u8], base: &Url) -> Result<Vec<Article>, ProviderError> {
    let doc = html(bytes)?;
    let mut rows = vec![];
    for node in doc.select(&sel("article.excerpt")) {
        let Some(href) = attr(node, "header h2 a", "href") else {
            continue;
        };
        let url = base
            .join(&href)
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        let Some(id) = id(url.as_str()) else { continue };
        let title = text(node, "header h2 a");
        if title.is_empty() {
            continue;
        }
        rows.push(Article {
            id,
            title,
            url,
            image: attr(node, "a.focus img", "data-src")
                .or_else(|| attr(node, "a.focus img", "src"))
                .and_then(|s| base.join(&s).ok())
                .map(|u| u.to_string())
                .unwrap_or_default(),
            summary: text(node, "p.note"),
            category: text(node, "div.meta a.cat"),
            published: text(node, "div.meta time"),
        });
    }
    Ok(rows)
}
#[derive(Deserialize)]
struct Resources {
    resources: Vec<Resource>,
}
#[derive(Deserialize)]
struct Resource {
    url: String,
    #[serde(default)]
    qrcode: String,
    #[serde(default)]
    cls: String,
    #[serde(default)]
    is_bt: bool,
}
impl Quarktv {
    async fn detail(
        &self,
        ctx: &SearchContext,
        item: Article,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let bytes = body(
            ctx,
            || {
                ctx.client
                    .get(item.url.clone())
                    .header("referer", self.base.as_str())
            },
            10,
            QUARKTV,
        )
        .await?;
        // Parse all DOM-owned data before the next await (scraper is not Send).
        let (post, content) = {
            let doc = html(&bytes)?;
            let root = doc.root_element();
            let post = attr(root, "div.postkit", "data-postid")
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| item.id.clone());
            let content = text(root, "div.postkit_art div.intro");
            (
                post,
                if content.is_empty() {
                    text(root, "article.article-content")
                } else {
                    content
                },
            )
        };
        let mut api = self.base.join("wp-json/postkit/v1/pan_view").unwrap();
        api.query_pairs_mut()
            .append_pair("post_id", &post)
            .append_pair("visitor_id", &format!("pk_{}", rand::random::<u64>()));
        let bytes = body(
            ctx,
            || {
                ctx.client
                    .get(api.clone())
                    .header("referer", item.url.as_str())
            },
            10,
            QUARKTV,
        )
        .await?;
        let resources: Resources =
            serde_json::from_slice(&bytes).map_err(|e| ProviderError::Parse(e.to_string()))?;
        let mut row = SearchResult::provider(format!("quarktv-{}", item.id), "quarktv");
        row.title = item.title;
        row.content = if content.is_empty() {
            item.summary
        } else {
            content
        };
        row.datetime = date(&item.published);
        if !item.category.is_empty() {
            row.tags.push(item.category);
        }
        if !item.image.is_empty() {
            row.images.push(item.image);
        }
        for resource in resources.resources {
            let raw = resource.url.trim();
            if raw.is_empty() {
                continue;
            }
            let mut link = Link::new(raw);
            link.password = password(raw);
            if resource.is_bt && raw.starts_with("magnet:") {
                link.cloud_type = CloudType::Magnet;
            }
            if link.cloud_type == CloudType::Others {
                link.cloud_type = match resource.cls.as_str() {
                    "quark" => CloudType::Quark,
                    "baidu" => CloudType::Baidu,
                    "uc" => CloudType::Uc,
                    "xunlei" => CloudType::Xunlei,
                    "ali" | "alipan" | "aliyun" => CloudType::Aliyun,
                    _ => CloudType::Others,
                };
            }
            if !row.links.iter().any(|l| l.url == link.url) {
                row.links.push(link);
            }
            if !resource.qrcode.trim().is_empty() && !row.images.contains(&resource.qrcode) {
                row.images.push(resource.qrcode);
            }
        }
        if row.links.is_empty() {
            return Err(ProviderError::Protocol(
                "quarktv postkit returned no resources".into(),
            ));
        }
        Ok(vec![row])
    }
}
#[async_trait]
impl Provider for Quarktv {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("quarktv", 2, KeywordFilterMode::Core)
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
        let mut url = self.base.clone();
        url.query_pairs_mut().append_pair("s", query);
        let bytes = body(
            ctx,
            || {
                ctx.client
                    .get(url.clone())
                    .header("referer", self.base.as_str())
            },
            15,
            QUARKTV,
        )
        .await?;
        let items = articles(&bytes, &self.base)?;
        collect(
            ctx,
            stream::iter(items.into_iter().map(|item| self.detail(ctx, item))).buffer_unordered(10),
        )
        .await
    }
}
