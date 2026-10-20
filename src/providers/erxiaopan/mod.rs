use super::html::{self, *};
use crate::{
    core::{ProviderError, SearchResult},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use scraper::{ElementRef, Html};
use url::Url;

#[derive(Debug)]
pub struct Erxiaopan {
    site: Site,
}
impl Default for Erxiaopan {
    fn default() -> Self {
        Self::with_base(Url::parse("https://www.2xiaopan.one/").unwrap())
    }
}
impl Erxiaopan {
    pub fn with_base(base: Url) -> Self {
        Self {
            site: Site::new(base, 2, 20, 5, 4 << 20, 3),
        }
    }
}
#[async_trait]
impl Provider for Erxiaopan {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("erxiaopan", 2, KeywordFilterMode::Provider)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let encoded = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
        let url = self
            .site
            .base
            .join(&format!("index.php/vod/search/wd/{encoded}.html"))
            .map_err(|_| parse_error("invalid search URL"))?;
        self.site
            .search(ctx, query, url, parse_search, parse_detail)
            .await
    }
}
fn metadata(scope: ElementRef<'_>, row: &mut SearchResult) {
    let mut lines = vec![];
    for a in scope.select(&selector(".video-info-aux a")) {
        let href = a.value().attr("href").unwrap_or("");
        let value = text(a);
        let label = if href.contains("/year/") {
            "年份"
        } else if href.contains("/area/") {
            "地区"
        } else if href.contains("/vod/type/") {
            "分类"
        } else if href.contains("/class/") {
            "类型"
        } else {
            continue;
        };
        unique(&mut lines, format!("{label}：{value}"));
        for value in value.split(['/', ',', '，', '、']) {
            unique(&mut row.tags, clean(value));
        }
    }
    for item in scope.select(&selector(".video-info-items")) {
        let label = first(item, ".video-info-itemtitle")
            .trim_end_matches([':', '：'])
            .to_owned();
        if !matches!(label.as_str(), "导演" | "主演" | "剧情" | "简介") {
            continue;
        }
        let mut values = item
            .select(&selector(".video-info-item a"))
            .map(text)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        if values.is_empty() && label == "主演" {
            values = first(item, ".video-info-item")
                .split(['/', ',', '，', '、'])
                .map(clean)
                .filter(|v| !v.is_empty())
                .collect();
        }
        if label == "主演" {
            values.truncate(6);
        }
        let value = if values.is_empty() {
            first(item, ".video-info-item").replace('/', " ")
        } else {
            values.join(" / ")
        };
        if !value.is_empty() {
            unique(&mut lines, format!("{label}：{value}"));
        }
    }
    let alias = first(scope, "h2.video-subtitle");
    if !alias.is_empty() {
        unique(&mut lines, alias);
    }
    unique(
        &mut row.tags,
        first(scope, ".video-info-header .video-serial"),
    );
    if !lines.is_empty() {
        row.content = lines.join("\n");
    }
}
fn parse_search(body: &str, base: &Url) -> Result<(Vec<Entry>, Option<Url>), ProviderError> {
    let doc = Html::parse_document(body);
    if doc
        .select(&selector(".module-search-item, .mac_total"))
        .next()
        .is_none()
    {
        return Err(parse_error("erxiaopan search structure missing"));
    }
    let mut entries = vec![];
    for card in doc.select(&selector(".module-search-item")) {
        let Some(anchor) = card.select(&selector(".video-info-header h3 a")).next() else {
            continue;
        };
        let Some(mut item) = entry(base, "erxiaopan", anchor) else {
            continue;
        };
        metadata(card, &mut item.row);
        item.row.content.push_str(&format!("\n来源：{}", item.url));
        if let Some(url) = card
            .select(&selector(".module-item-pic img"))
            .next()
            .and_then(|node| image(node, base))
        {
            item.row.images.push(url);
        }
        entries.push(item);
    }
    Ok((entries, next_page(&doc, base)))
}
fn suffix(label: &str, title: &str) -> String {
    let label = clean(label);
    if label == title {
        return String::new();
    }
    if let Some(rest) = label.strip_prefix(title).filter(|_| !title.is_empty()) {
        return clean(rest.trim_start_matches(['-', '–', '—', '_', '~', '·', ':', '：', ' ']));
    }
    if label.contains(" - ") {
        String::new()
    } else {
        label
    }
}
fn parse_detail(body: &str, entry: Entry) -> Result<SearchResult, ProviderError> {
    let doc = Html::parse_document(body);
    if doc
        .select(&selector("#download-list, .video-info-header h1"))
        .next()
        .is_none()
    {
        return Err(parse_error("erxiaopan detail structure missing"));
    }
    let mut row = entry.row;
    let title = first(doc.root_element(), ".video-info-header h1");
    if !title.is_empty() {
        row.title = title;
    }
    let scope = doc
        .select(&selector(".view-heading"))
        .next()
        .filter(|scope| {
            scope.select(&selector(".video-info-aux a")).any(|a| {
                let href = a.value().attr("href").unwrap_or("");
                ["/year/", "/area/", "/vod/type/"]
                    .iter()
                    .any(|part| href.contains(*part))
            })
        })
        .unwrap_or_else(|| doc.root_element());
    metadata(scope, &mut row);
    let alias = first(doc.root_element(), "h2.video-subtitle");
    if !alias.is_empty() && !row.content.contains(&alias) {
        row.content.push_str(&format!("\n{alias}"));
    }
    unique(
        &mut row.tags,
        first(doc.root_element(), ".video-info-header .video-serial"),
    );
    row.content.push_str(&format!("\n来源：{}", entry.url));
    row.datetime = Some(chrono::Utc::now());
    if let Some(url) = doc
        .select(&selector(".mobile-play img, .video-cover img"))
        .find_map(|node| image(node, &entry.url))
    {
        row.images = vec![url];
    }
    let mut raw = vec![];
    let nodes = doc
        .select(&selector("#download-list .module-row-one"))
        .collect::<Vec<_>>();
    if nodes.is_empty() {
        for node in doc.select(&selector("#download-list a[data-clipboard-text]")) {
            raw.push((
                first(node, "h4"),
                node.value()
                    .attr("data-clipboard-text")
                    .unwrap_or("")
                    .to_owned(),
                text(node),
            ));
        }
    } else {
        for node in nodes {
            let url = node
                .select(&selector("a.copy[data-clipboard-text]"))
                .filter_map(|a| a.value().attr("data-clipboard-text"))
                .find(|s| !s.trim().is_empty())
                .or_else(|| {
                    node.select(&selector(".module-row-shortcuts a[href]"))
                        .find_map(|a| a.value().attr("href"))
                })
                .unwrap_or("");
            raw.push((
                first(node, ".module-row-title h4"),
                url.to_owned(),
                text(node),
            ));
        }
    }
    let mut resolved = vec![];
    let mut seen = std::collections::HashSet::new();
    for (label, raw, nearby) in raw {
        if let Some(link) = html::share(&raw, &format!("{raw} {nearby}"), true)
            && seen.insert((link.url.clone(), link.password.clone()))
        {
            resolved.push((suffix(&label, &row.title), link));
        }
    }
    let suffixes = resolved
        .iter()
        .map(|(s, _)| s)
        .filter(|s| !s.is_empty())
        .collect::<std::collections::HashSet<_>>();
    let ambiguous = suffixes.len() > 1;
    row.links = resolved
        .into_iter()
        .map(|(suffix, mut link)| {
            link.work_title = Some(if ambiguous && !suffix.is_empty() {
                format!("{} {suffix}", row.title)
            } else {
                row.title.clone()
            });
            link
        })
        .collect();
    Ok(row)
}
#[cfg(test)]
mod tests;
