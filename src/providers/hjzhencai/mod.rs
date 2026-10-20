use super::html::{self, *};
use crate::{
    core::{ProviderError, SearchResult},
    providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use chrono::{FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc};
use scraper::Html;
use url::Url;

#[derive(Debug)]
pub struct Hjzhencai {
    site: Site,
}
impl Default for Hjzhencai {
    fn default() -> Self {
        Self::with_base(Url::parse("https://www.hjzhencai.top/").unwrap())
    }
}
impl Hjzhencai {
    pub fn with_base(base: Url) -> Self {
        Self {
            site: Site::new(base, 3, 48, 3, 2 << 20, 1),
        }
    }
}
#[async_trait]
impl Provider for Hjzhencai {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("hjzhencai", 3, KeywordFilterMode::Provider)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let mut url = self
            .site
            .base
            .join("index.php/vod/search.html")
            .map_err(|_| parse_error("invalid search URL"))?;
        url.query_pairs_mut().append_pair("wd", query);
        self.site
            .search(ctx, query, url, parse_search, parse_detail)
            .await
    }
}
fn parse_search(body: &str, base: &Url) -> Result<(Vec<Entry>, Option<Url>), ProviderError> {
    let doc = Html::parse_document(body);
    if doc
        .select(&selector(".module-heading-search, .module-card-item"))
        .next()
        .is_none()
    {
        return Err(parse_error("hjzhencai search structure missing"));
    }
    let mut entries = vec![];
    for card in doc.select(&selector(".module-card-item")) {
        let Some(anchor) = card.select(&selector(".module-card-item-title a")).next() else {
            continue;
        };
        let Some(mut item) = entry(base, "hjzhencai", anchor) else {
            continue;
        };
        item.row.content = card
            .select(&selector(
                ".module-card-item-class, .module-item-note, .module-info-item-content",
            ))
            .map(text)
            .collect::<Vec<_>>()
            .join(" | ");
        unique(&mut item.row.tags, first(card, ".module-card-item-class"));
        if let Some(url) = card
            .select(&selector("img"))
            .find_map(|node| image(node, &item.url))
        {
            item.row.images.push(url);
        }
        entries.push(item);
    }
    Ok((entries, next_page(&doc, base)))
}
fn parse_detail(body: &str, entry: Entry) -> Result<SearchResult, ProviderError> {
    let doc = Html::parse_document(body);
    if doc
        .select(&selector(".module-info-main, .down-wrap"))
        .next()
        .is_none()
    {
        return Err(parse_error("hjzhencai detail structure missing"));
    }
    let mut row = entry.row;
    let title = first(doc.root_element(), ".module-info-heading h1");
    if !title.is_empty() {
        row.title = title;
    }
    row.datetime = Some(Utc::now());
    let mut content = vec![row.content.clone()];
    unique(
        &mut content,
        first(doc.root_element(), ".module-info-introduction-content"),
    );
    for item in doc.select(&selector(".module-info-main .module-info-item")) {
        let label = first(item, ".module-info-item-title")
            .trim_end_matches([':', '：'])
            .to_owned();
        let value = first(item, ".module-info-item-content");
        if label == "更新" {
            let date = NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S")
                .ok()
                .or_else(|| {
                    NaiveDate::parse_from_str(&value, "%Y-%m-%d")
                        .ok()?
                        .and_hms_opt(0, 0, 0)
                });
            if let Some(date) = date {
                row.datetime = FixedOffset::east_opt(8 * 3600)
                    .unwrap()
                    .from_local_datetime(&date)
                    .single()
                    .map(|date| date.with_timezone(&Utc));
            }
        } else if !label.is_empty() && !value.is_empty() {
            unique(&mut content, format!("{label}：{value}"));
        }
    }
    unique(&mut content, format!("来源：{}", entry.url));
    row.content = content.join("\n");
    for a in doc.select(&selector(".module-info-tag a")) {
        unique(&mut row.tags, text(a));
    }
    if let Some(url) = doc
        .select(&selector(".module-info-poster img"))
        .find_map(|node| image(node, &entry.url))
    {
        row.images = vec![url];
    }
    let mut seen = std::collections::HashSet::new();
    for wrap in doc.select(&selector(".down-wrap")) {
        // down-src starts a new route; episode ambiguity is local to that route.
        let mut groups = std::collections::HashMap::<_, Vec<_>>::new();
        let mut order = vec![];
        for card in wrap.select(&selector(".down-card")) {
            let source = card
                .prev_siblings()
                .filter_map(scraper::ElementRef::wrap)
                .find(|node| {
                    node.value()
                        .has_class("down-src", scraper::CaseSensitivity::CaseSensitive)
                })
                .map(|node| node.id());
            if !groups.contains_key(&source) {
                order.push(source);
            }
            groups.entry(source).or_default().push(card);
        }
        for source in order {
            let group = &groups[&source];
            let group_size = if source.is_some() { group.len() } else { 0 };
            for card in group {
                let label = first(*card, ".down-card-name");
                let title = if !label.is_empty()
                    && label != row.title
                    && (label != "第1集" || group_size > 1)
                {
                    format!("{} {label}", row.title)
                } else {
                    row.title.clone()
                };
                let nearby = URLS.replace_all(&text(*card), " ").into_owned();
                let anchors = card.select(&selector("a[href]")).collect::<Vec<_>>();
                let urls = if anchors.is_empty() {
                    URLS.find_iter(&text(*card))
                        .map(|m| m.as_str().to_owned())
                        .collect::<Vec<_>>()
                } else {
                    anchors
                        .iter()
                        .filter_map(|a| a.value().attr("href").map(str::to_owned))
                        .collect()
                };
                for url in urls {
                    if let Some(mut link) = html::share(&url, &nearby, false)
                        && seen.insert((link.url.clone(), link.password.clone()))
                    {
                        link.work_title = Some(title.clone());
                        row.links.push(link);
                    }
                }
            }
        }
    }
    if row.links.is_empty() {
        return Err(parse_error(
            "hjzhencai detail contains no supported download shares",
        ));
    }
    Ok(row)
}
#[cfg(test)]
mod linkless_tests;
#[cfg(test)]
mod tests;
