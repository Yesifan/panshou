//! Miaosou's AES-128-CBC share URL search. Never embeds the upstream user's satoken.
use super::support::*;
use crate::{
    core::{Link, ProviderError, SearchResult},
    http::{ClientOptions, HttpClientFactory},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use std::{sync::Arc, time::Duration};
use tokio::sync::OnceCell;
use url::Url;
#[derive(Clone)]
pub struct Miaoso {
    endpoint: Url,
    token: Option<String>,
    options: ClientOptions,
    client: Arc<OnceCell<reqwest::Client>>,
}
impl std::fmt::Debug for Miaoso {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Miaoso").finish_non_exhaustive()
    }
}
impl Default for Miaoso {
    fn default() -> Self {
        Self::new(Url::parse("https://miaosou.fun/api/secendsearch").unwrap())
    }
}
#[derive(Deserialize)]
struct Response {
    code: i64,
    data: Data,
}
#[derive(Deserialize)]
struct Data {
    list: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    id: String,
    name: String,
    url: String,
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    from: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default, rename = "gmtShare")]
    shared: String,
}
fn decrypt(raw: &str) -> Result<String, ProviderError> {
    let mut bytes = STANDARD
        .decode(raw)
        .map_err(|_| protocol("miaoso invalid Base64 ciphertext"))?;
    let decrypted =
        cbc::Decryptor::<aes::Aes128>::new_from_slices(b"4OToScUFOaeVTrHE", b"9CLGao1vHKqm17Oz")
            .map_err(|_| protocol("miaoso invalid AES parameters"))?
            .decrypt_padded_mut::<Pkcs7>(&mut bytes)
            .map_err(|_| protocol("miaoso invalid CBC ciphertext or padding"))?;
    let raw = std::str::from_utf8(decrypted)
        .map_err(|_| protocol("miaoso decrypted URL is not UTF-8"))?;
    let url =
        Url::parse(raw.trim()).map_err(|_| protocol("miaoso decrypted value is not a URL"))?;
    if !["http", "https", "magnet", "ed2k"].contains(&url.scheme()) {
        return Err(protocol("miaoso decrypted URL has unsupported scheme"));
    }
    Ok(raw.trim().into())
}
impl Miaoso {
    pub fn new(endpoint: Url) -> Self {
        Self::with_options(endpoint, ClientOptions::default())
    }
    pub fn with_options(endpoint: Url, options: ClientOptions) -> Self {
        Self {
            endpoint,
            token: None,
            options,
            client: Arc::new(OnceCell::new()),
        }
    }
    /// Optional caller-owned session token; never logged or read from checked-in source.
    pub fn with_token(mut self, token: String) -> Self {
        self.token = Some(token);
        self
    }
    async fn run(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self.endpoint.clone();
        url.query_pairs_mut()
            .append_pair("name", query)
            .append_pair("pageNo", "1");
        let mut referer = self
            .endpoint
            .join("/info")
            .map_err(|_| protocol("miaoso invalid endpoint"))?;
        referer.query_pairs_mut().append_pair("searchKey", query);
        // A sensitive custom header is still forwarded by reqwest redirects.
        // Never follow redirects when sending the caller's session token.
        let client = self
            .client
            .get_or_try_init(|| async {
                HttpClientFactory::new()
                    .no_follow(&self.options)
                    .map_err(|e| protocol(e.to_string()))
            })
            .await?;
        let mut builder = browser(
            client.get(url),
            referer.as_str(),
            "application/json, text/plain, */*",
        );
        if let Some(token) = &self.token {
            let mut value = reqwest::header::HeaderValue::from_str(token)
                .map_err(|_| protocol("miaoso invalid session token header"))?;
            value.set_sensitive(true);
            builder = builder.header("satoken", value);
        }
        let bytes = tokio::time::timeout(
            ctx.request_timeout(Duration::from_secs(30)),
            request(ctx, builder, 3, Duration::from_secs(30), LIMIT),
        )
        .await
        .map_err(|_| ProviderError::Timeout)??;
        let response: Response = serde_json::from_slice(&bytes).map_err(parse)?;
        if response.code != 200 {
            return Err(protocol(format!("miaoso API code={}", response.code)));
        }
        let mut rows = Vec::new();
        let mut error = None;
        for item in response.data.list {
            if item.url.trim().is_empty() {
                continue;
            }
            match decrypt(&item.url) {
                Ok(url) => {
                    let mut row = SearchResult::provider(format!("miaoso-{}", item.id), "miaoso");
                    row.title = scraper::Html::parse_fragment(&item.name)
                        .root_element()
                        .text()
                        .collect::<String>()
                        .trim()
                        .into();
                    row.content = item.content.unwrap_or_default();
                    row.datetime = date(&item.shared);
                    if !item.from.is_empty() {
                        row.tags.push(item.from.clone());
                    }
                    if let Some(kind) = item.r#type.filter(|v| !v.is_empty()) {
                        row.tags.push(kind);
                    }
                    let mut link = Link::new(url);
                    link.cloud_type = cloud(&item.from);
                    row.links.push(link);
                    if let Some(row) = publish(ctx, row, query) {
                        rows.push(row);
                    }
                }
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
impl Provider for Miaoso {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("miaoso", 3, KeywordFilterMode::Core)
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
mod token_tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, path},
    };

    #[tokio::test]
    async fn caller_token_is_sent_but_redacted_from_debug() {
        let server = MockServer::start().await;
        let token = "caller-owned-fixture-token";
        Mock::given(path("/api/secendsearch"))
            .and(header("satoken", token))
            .respond_with(ResponseTemplate::new(200).set_body_string(include_str!(
                "../../tests/fixtures/providers/miaoso/empty.json"
            )))
            .expect(1)
            .mount(&server)
            .await;
        let source =
            Miaoso::new(Url::parse(&format!("{}/api/secendsearch", server.uri())).unwrap())
                .with_token(token.into());
        assert!(!format!("{source:?}").contains(token));
        let ctx = SearchContext::new(reqwest::Client::new(), Duration::from_secs(2));
        assert!(source.search(&ctx, "film").await.unwrap().is_empty());
        server.verify().await;
    }
}
