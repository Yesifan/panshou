use super::{KeywordFilterMode, Provider, ProviderMeta, SearchContext, c_support::*};
use crate::{
    core::{CloudType, Link, ProviderError, SearchResult},
    http::{ClientOptions, HttpClientFactory},
};
use async_trait::async_trait;
use regex::Regex;
use scraper::{Html, Selector};
use std::{collections::HashSet, sync::LazyLock, time::Duration};
use url::Url;
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/subject/(\d+)\.html").unwrap());
static PWD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:提取码|密码)[:：]?\s*([A-Za-z0-9]+)").unwrap());
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: Url,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: Url::parse("https://xzys.fun/").unwrap(),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct Xiaozhang {
    endpoints: Endpoints,
    options: ClientOptions,
}
impl Xiaozhang {
    pub fn new(endpoints: Endpoints, options: ClientOptions) -> Self {
        Self { endpoints, options }
    }
}
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn list(body: &str, base: &Url) -> Vec<(Url, SearchResult)> {
    let d = Html::parse_document(body);
    d.select(&sel(".list-boxes"))
        .filter_map(|e| {
            let a = e.select(&sel("a.text_title_p")).next()?;
            let title = a.text().collect::<String>().trim().to_owned();
            if title.is_empty() {
                return None;
            }
            let u = base.join(a.value().attr("href")?).ok()?;
            let id = ID
                .captures(u.path())
                .map(|c| c[1].to_owned())
                .unwrap_or_else(|| format!("{:x}", md5::compute(u.as_str())));
            let mut r = SearchResult::provider(format!("xiaozhang-{id}"), "xiaozhang");
            r.title = title;
            r.content = e
                .select(&sel("p.text_p"))
                .next()
                .map(|e| e.text().collect::<String>().trim().into())
                .unwrap_or_default();
            r.datetime = e
                .select(&sel(".list-actions span"))
                .next()
                .map(|e| e.text().collect::<String>())
                .and_then(|s| date(s.trim()))
                .or_else(|| Some(chrono::Utc::now()));
            Some((u, r))
        })
        .collect()
}
fn detail(body: &str) -> Vec<Link> {
    let d = Html::parse_document(body);
    let mut out = vec![];
    let mut seen = HashSet::new();
    for p in d.select(&sel("p")) {
        let text = p.text().collect::<String>();
        let code = PWD.captures(&text).map(|c| c[1].to_owned());
        for a in p.select(&sel("a[href]")) {
            let raw = a.value().attr("href").unwrap();
            let mut l = Link::new(raw);
            if !matches!(
                l.cloud_type,
                CloudType::Baidu
                    | CloudType::Quark
                    | CloudType::Aliyun
                    | CloudType::Uc
                    | CloudType::Xunlei
                    | CloudType::Tianyi
                    | CloudType::One15
                    | CloudType::Pan123
                    | CloudType::Mobile
                    | CloudType::Pikpak
            ) || !seen.insert(l.url.clone())
            {
                continue;
            }
            l.password = code.clone().or_else(|| password(raw));
            out.push(l)
        }
    }
    out
}
impl Xiaozhang {
    async fn get(
        &self,
        ctx: &SearchContext,
        url: Url,
        referer: &str,
        follow: bool,
    ) -> Result<reqwest::Response, ProviderError> {
        let client = if follow {
            ctx.client.clone()
        } else {
            HttpClientFactory::new()
                .no_follow(&self.options)
                .map_err(|_| {
                    ProviderError::Unavailable("cannot construct xiaozhang no-follow client".into())
                })?
        };
        crate::http::execute(
            client
                .get(url)
                .timeout(ctx.remaining())
                .header("referer", referer)
                .header("accept-encoding", "gzip, deflate")
                .header("user-agent", "Mozilla/5.0 Chrome/138.0.0.0 Safari/537.36")
                .header(
                    "accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
                ),
        )
        .await
        .map_err(network)
    }
    async fn fetch_detail(
        &self,
        ctx: &SearchContext,
        url: Url,
    ) -> Result<Vec<Link>, ProviderError> {
        let mut response = self
            .get(ctx, url.clone(), self.endpoints.base.as_str(), false)
            .await?;
        if let Some(location) = response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
        {
            let next = url
                .join(location)
                .map_err(|e| ProviderError::Protocol(e.to_string()))?;
            drop(response);
            response = self.get(ctx, next, url.as_str(), true).await?;
        }
        if response.status() != 200 {
            return Err(ProviderError::Protocol(format!(
                "HTTP {}",
                response.status().as_u16()
            )));
        }
        let body = crate::http::read_response(response, BODY_LIMIT)
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        Ok(detail(&String::from_utf8_lossy(&body)))
    }
}
#[async_trait]
impl Provider for Xiaozhang {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("xiaozhang", 3, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut url = self.endpoints.base.join("/search.html").unwrap();
        url.query_pairs_mut().append_pair("keyword", query);
        let r = self
            .get(ctx, url, self.endpoints.base.as_str(), true)
            .await?;
        if r.status() != 200 {
            return Err(ProviderError::Protocol(format!(
                "HTTP {}",
                r.status().as_u16()
            )));
        }
        let body = crate::http::read_response(r, BODY_LIMIT)
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        let rows = list(&String::from_utf8_lossy(&body), &self.endpoints.base);
        collect_details(
            ctx,
            rows.into_iter()
                .enumerate()
                .map(|(index, (url, mut row))| async move {
                    tokio::time::sleep(Duration::from_millis(index as u64 * 50)).await;
                    if ctx.is_cancelled() {
                        return Err(ProviderError::Timeout);
                    }
                    row.links = self.fetch_detail(ctx, url).await?;
                    Ok(row)
                }),
            20,
        )
        .await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paragraph_passwords() {
        let r = detail(
            r#"<p><a href="https://pan.baidu.com/s/a">open</a>提取码: abcd</p><p><a href="https://pan.quark.cn/s/b?pwd=efgh">q</a></p><a href="https://pan.quark.cn/s/ignored">outside</a>"#,
        );
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].password.as_deref(), Some("abcd"));
        assert_eq!(r[1].password.as_deref(), Some("efgh"));
        assert!(detail("bad").is_empty());
        assert!(list("bad", &Endpoints::default().base).is_empty());
    }
}
