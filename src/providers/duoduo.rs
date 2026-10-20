//! 多多影视's MacCMS search and clipboard download rows.
use super::{anchors, clean, fetch, headers, matches, sel, text};
use crate::core::{ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use regex::Regex;
use scraper::Html;
use std::{collections::HashSet, sync::LazyLock, time::Duration};
use url::Url;
#[derive(Debug, Clone)]
pub struct DuoduoProvider {
    base: Url,
}
impl Default for DuoduoProvider {
    fn default() -> Self {
        Self::new(Url::parse("https://tv.yydsys.top/").unwrap())
    }
}
impl DuoduoProvider {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
}
fn items(body: &str) -> Result<Vec<SearchResult>, ProviderError> {
    static ID: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"/vod/detail/id/(\d+)\.html").unwrap());
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    if !body.contains("<html") && !body.contains("<body") && !body.contains("module-search-item") {
        return Err(ProviderError::Parse(
            "duoduo: malformed search document".into(),
        ));
    }
    for node in doc.select(&sel(".module-search-item")) {
        let Some(a) = node.select(&sel(".video-info-header h3 a")).next() else {
            continue;
        };
        let Some(id) = a
            .value()
            .attr("href")
            .and_then(|s| ID.captures(s))
            .map(|c| c[1].to_owned())
        else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        let mut row = SearchResult::provider(format!("duoduo-{id}"), "duoduo");
        row.title = clean(&a.inner_html());
        row.tags = node
            .select(&sel(".video-info-aux .tag-link a"))
            .map(|n| clean(&n.inner_html()))
            .collect();
        row.content = format!(
            "{} {}",
            text(node, ".video-serial"),
            text(node, ".video-info-items")
        );
        row.images = node
            .select(&sel(".module-item-pic > img"))
            .filter_map(|n| n.value().attr("data-src").map(str::to_owned))
            .collect();
        out.push(row);
    }
    Ok(out)
}
#[async_trait]
impl Provider for DuoduoProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("duoduo", 2, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let encoded = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
        let url = self
            .base
            .join(&format!("index.php/vod/search/wd/{encoded}.html"))
            .unwrap();
        let body = fetch(
            ctx,
            headers(ctx.client.get(url), self.base.as_str()),
            "duoduo",
            "search",
            Duration::from_secs(8),
            16 << 20,
            &[200, 400],
        )
        .await?;
        let results=stream::iter(items(&body)?.into_iter().map(|mut row|async move{
   let id=row.id.strip_prefix("duoduo-").unwrap();let url=self.base.join(&format!("index.php/vod/detail/id/{id}.html")).unwrap();
   let body=fetch(ctx,headers(ctx.client.get(url),self.base.as_str()),"duoduo","detail",Duration::from_secs(6),16<<20,&[200,400]).await?;
   row.links=anchors(&body,"#download-list .module-row-one [data-clipboard-text], #download-list .module-row-one a[href]");
   let doc=Html::parse_document(&body);
   // Jianguoyun has no shared CloudType/checker; retain its real URL as Others.
   for node in doc.select(&sel("#download-list .module-row-one [data-clipboard-text], #download-list .module-row-one a[href]")) {
    let Some(raw)=node.value().attr("data-clipboard-text").or_else(||node.value().attr("href"))else{continue;};
    let Ok(url)=Url::parse(raw)else{continue;};let host=url.host_str().unwrap_or("");
    if matches!(url.scheme(),"http"|"https") && matches!(host,"jianguoyun.com"|"www.jianguoyun.com") && url.path().starts_with("/p/") && !row.links.iter().any(|l|l.url==raw){row.links.push(crate::core::Link::new(raw));}
   }
   if row.links.is_empty(){return Err(ProviderError::Parse("duoduo: missing download links".into()));}
   if let Some(image)=doc.select(&sel(".mobile-play .lazyload")).find_map(|n|n.value().attr("data-src")){row.images=vec![image.to_owned()];}
   for link in &mut row.links{link.work_title=Some(row.title.clone());}
   if matches(&row,query){ctx.publish(vec![row.clone()]);Ok(Some(row))}else{Ok(None)}
  })).buffer_unordered(20).collect::<Vec<Result<Option<SearchResult>,ProviderError>>>().await;
        let mut out = Vec::new();
        let mut error = None;
        for result in results {
            match result {
                Ok(Some(row)) => out.push(row),
                Ok(None) => {}
                Err(e) => {
                    error.get_or_insert(e);
                }
            }
        }
        if let Some(error) = error {
            return Err(error);
        }
        Ok(out)
    }
}
