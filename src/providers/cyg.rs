use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use futures::future::join_all;
use serde::Deserialize;
use url::Url;

use crate::core::{Link, ProviderError, SearchResult, Source};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};

const USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 16_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.6 Mobile/15E148 Safari/604.1";

fn request(client: &reqwest::Client, url: Url, referer: &Url) -> reqwest::RequestBuilder {
    client
        .get(url)
        .header(reqwest::header::REFERER, referer.as_str())
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::ACCEPT, "application/json, text/plain, */*")
        .header(reqwest::header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
        .header(reqwest::header::CONNECTION, "keep-alive")
}

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://www.acgndog.com/").expect("static URL"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CygProvider {
    endpoints: Endpoints,
}
impl CygProvider {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}

#[derive(Debug, Default, Deserialize)]
struct Rendered {
    #[serde(default)]
    rendered: String,
}
#[derive(Debug, Deserialize)]
struct Post {
    id: i64,
    #[serde(default)]
    date: String,
    #[serde(default)]
    title: Rendered,
    #[serde(default)]
    excerpt: Rendered,
    #[serde(default)]
    category_name: String,
}
#[derive(Debug, Deserialize)]
struct Download {
    #[serde(default)]
    name: String,
    #[serde(default)]
    url: String,
    #[serde(default, rename = "downloadPwd")]
    download_pwd: String,
}

fn parse_posts(body: &str) -> Result<Vec<Post>, ProviderError> {
    serde_json::from_str(body).map_err(|e| ProviderError::Parse(e.to_string()))
}
fn parse_downloads(body: &str) -> Result<Vec<Link>, ProviderError> {
    let rows: Vec<Download> =
        serde_json::from_str(body).map_err(|e| ProviderError::Parse(e.to_string()))?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let raw = row.url.trim();
            if raw.is_empty() {
                return None;
            }
            let mut link = Link::new(raw);
            if link.cloud_type == crate::core::CloudType::Others {
                link.cloud_type = cloud_from_name(&row.name);
            }
            link.password = (!row.download_pwd.trim().is_empty()).then_some(row.download_pwd);
            Some(link)
        })
        .collect())
}
fn cloud_from_name(name: &str) -> crate::core::CloudType {
    match name.trim().to_ascii_lowercase().as_str() {
        "夸克" | "夸克网盘" => crate::core::CloudType::Quark,
        "uc" | "uc网盘" => crate::core::CloudType::Uc,
        "百度" | "百度网盘" | "baidu" => crate::core::CloudType::Baidu,
        "阿里" | "阿里云盘" | "阿里网盘" | "aliyun" => crate::core::CloudType::Aliyun,
        "迅雷" | "迅雷网盘" | "xunlei" => crate::core::CloudType::Xunlei,
        "天翼" | "天翼云盘" | "189" | "189云盘" => crate::core::CloudType::Tianyi,
        "115" | "115网盘" => crate::core::CloudType::One15,
        "123" | "123网盘" | "123pan" => crate::core::CloudType::Pan123,
        "pikpak" | "pikpak网盘" => crate::core::CloudType::Pikpak,
        "磁力链接" | "magnet" => crate::core::CloudType::Magnet,
        "ed2k" => crate::core::CloudType::Ed2k,
        _ => crate::core::CloudType::Others,
    }
}
fn clean_html(value: &str) -> String {
    let fragment = scraper::Html::parse_fragment(value);
    fragment
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn convert(post: Post, links: Vec<Link>) -> SearchResult {
    let datetime = DateTime::parse_from_rfc3339(&post.date)
        .ok()
        .map(|v| v.with_timezone(&Utc))
        .or_else(|| {
            NaiveDateTime::parse_from_str(&post.date, "%Y-%m-%dT%H:%M:%S")
                .ok()
                .map(|v| v.and_utc())
        });
    SearchResult {
        id: format!("cyg-{}", post.id),
        source: Source::provider("cyg"),
        datetime,
        title: clean_html(&post.title.rendered),
        content: clean_html(&post.excerpt.rendered),
        links,
        tags: (!post.category_name.is_empty())
            .then_some(vec![post.category_name])
            .unwrap_or_default(),
        images: vec![],
    }
}

#[async_trait]
impl Provider for CygProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("cyg", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self
            .endpoints
            .base
            .join("wp-json/wp/v2/posts")
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("per_page", "20")
            .append_pair("orderby", "date")
            .append_pair("order", "desc")
            .append_pair("page", "1")
            .append_pair("search", query);
        let response = tokio::time::timeout(
            ctx.timeout,
            request(&ctx.client, url, &self.endpoints.base).send(),
        )
        .await
        .map_err(|_| ProviderError::Timeout)?
        .map_err(|e| ProviderError::Network(e.to_string()))?;
        if !response.status().is_success() {
            return Err(ProviderError::Protocol(format!(
                "HTTP {}",
                response.status()
            )));
        }
        let posts = parse_posts(
            &response
                .text()
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?,
        )?;
        let jobs = posts.into_iter().map(|post| async move {
            let mut url = self
                .endpoints
                .base
                .join("wp-json/acg-studio/v1/download")
                .ok()?;
            url.query_pairs_mut()
                .append_pair("id", &post.id.to_string());
            let response = request(&ctx.client, url, &self.endpoints.base)
                .send()
                .await
                .ok()?;
            if !response.status().is_success() {
                return None;
            }
            let links = parse_downloads(&response.text().await.ok()?).ok()?;
            (!links.is_empty()).then(|| convert(post, links))
        });
        Ok(join_all(jobs).await.into_iter().flatten().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn success() {
        let p = parse_posts(include_str!(
            "../../tests/fixtures/providers/cyg/success.json"
        ))
        .unwrap();
        let l = parse_downloads(include_str!(
            "../../tests/fixtures/providers/cyg/download.json"
        ))
        .unwrap();
        assert_eq!(
            convert(p.into_iter().next().unwrap(), l).links[0]
                .password
                .as_deref(),
            Some("1iq9")
        );
    }
    #[test]
    fn empty() {
        assert!(
            parse_posts(include_str!(
                "../../tests/fixtures/providers/cyg/empty.json"
            ))
            .unwrap()
            .is_empty()
        );
    }
    #[test]
    fn malformed() {
        assert!(
            parse_posts(include_str!(
                "../../tests/fixtures/providers/cyg/malformed.json"
            ))
            .is_err()
        );
    }
}
