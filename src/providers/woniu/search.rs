use super::*;
use crate::{
    core::{Link, ProviderError, SearchResult},
    providers::{AuthKind, KeywordFilterMode, Provider, ProviderMeta, SearchContext},
};
use async_trait::async_trait;
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use std::{collections::HashSet, sync::LazyLock, time::Duration};
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^/voddetail/(\d+)/?$").unwrap());
static PASSWORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:提取码|密码|访问码|password|pwd)\s*[:：]?\s*([a-z0-9]{3,12})").unwrap()
});
fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}
fn text(e: ElementRef<'_>) -> String {
    e.text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn first(e: ElementRef<'_>, s: &str) -> String {
    e.select(&sel(s)).next().map(text).unwrap_or_default()
}
#[derive(Debug, Clone)]
struct Item {
    id: String,
    title: String,
    url: Url,
    poster: Option<String>,
    note: String,
    meta: String,
}
fn items(body: &str, base: &Url) -> Vec<Item> {
    let doc = Html::parse_document(body);
    let mut seen = HashSet::new();
    doc.select(&sel("a.video-card"))
        .filter_map(|e| {
            let url = base.join(e.value().attr("href")?).ok()?;
            if url.origin() != base.origin() {
                return None;
            }
            let id = ID.captures(url.path())?[1].to_owned();
            let title = e
                .value()
                .attr("title")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| first(e, ".video-title"));
            if title.is_empty() || !seen.insert(id.clone()) {
                return None;
            }
            let poster = e
                .select(&sel(".video-poster img"))
                .next()
                .and_then(|i| i.value().attr("src"))
                .filter(|s| !s.trim().is_empty())
                .and_then(|s| base.join(s).ok())
                .map(|u| u.to_string());
            Some(Item {
                id,
                title,
                url,
                poster,
                note: first(e, ".video-episode"),
                meta: first(e, ".video-meta"),
            })
        })
        .take(24)
        .collect()
}
fn work_title(movie: &str, note: &str) -> String {
    let movie = movie.trim();
    let note = note.trim();
    if movie.is_empty() || note.contains(movie) {
        note.into()
    } else if note.is_empty() {
        movie.into()
    } else {
        format!("{movie} {note}")
    }
}
fn normalize(raw: &str, title: &str) -> Option<Link> {
    let decoded = Html::parse_fragment(raw)
        .root_element()
        .text()
        .collect::<String>();
    let decoded = decoded.trim();
    if decoded.is_empty() || decoded.contains("******") {
        return None;
    }
    let end = decoded
        .find(|c: char| c.is_whitespace() || "\"'<>".contains(c))
        .unwrap_or(decoded.len());
    let raw =
        decoded[..end].trim_end_matches(['.', ',', ';', '，', '。', '；', '！', '!', '）', ')']);
    if !raw.starts_with("magnet:?xt=urn:btih:") && !raw.starts_with("ed2k://") {
        let url = Url::parse(raw).ok()?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return None;
        }
    }
    let mut link = Link::new(raw);
    link.password = super::super::c_support::password(raw)
        .or_else(|| PASSWORD.captures(decoded).map(|c| c[1].to_owned()));
    link.work_title = Some(title.into());
    Some(link)
}
fn detail(body: &str, item: &Item, base: &Url) -> Result<SearchResult, ProviderError> {
    let doc = Html::parse_document(body);
    let root = doc.root_element();
    let full = text(root);
    if doc.select(&sel(".pan-lock-box")).next().is_some()
        || full.contains("登录后可见") && full.contains("******")
    {
        return Err(ProviderError::AuthRequired);
    }
    let mut row = SearchResult::provider(format!("woniu-{}", item.id), "woniu");
    row.title = first(root, "h1.premium-title");
    if row.title.is_empty() {
        row.title = item.title.clone()
    }
    let mut seen = HashSet::new();
    for e in doc.select(&sel(".pan-link-item")) {
        let mut note = first(e, ".pan-link-title");
        if note.is_empty() {
            note = first(e, ".pan-link-name")
        }
        let title = work_title(&item.title, &note);
        let mut candidates = vec![];
        if let Some(raw) = e.value().attr("data-pan-item") {
            candidates.push(raw.to_owned())
        }
        if let Some(a) = e.select(&sel("a.pan-link-btn[href]")).next() {
            candidates.push(a.value().attr("href").unwrap().into())
        }
        if let Some(a) = e.select(&sel("[data-copy]")).next() {
            candidates.push(a.value().attr("data-copy").unwrap().into())
        }
        candidates.extend(e.select(&sel(".pan-link-meta")).map(text));
        // The first valid candidate belongs to this record, even if already seen.
        if let Some(link) = candidates.iter().find_map(|s| normalize(s, &title))
            && seen.insert(link.url.clone())
        {
            row.links.push(link)
        }
        if row.links.len() >= 200 {
            break;
        }
    }
    if row.links.is_empty() {
        return Err(ProviderError::Protocol(
            "woniu detail has no usable links; session may have expired or site schema changed"
                .into(),
        ));
    }
    let mut parts = vec![];
    if !item.note.is_empty() {
        parts.push(format!("【{}】", item.note))
    }
    if !item.meta.is_empty() {
        parts.push(item.meta.clone())
    }
    let mut plot = first(root, "#detailDescText");
    if plot.is_empty() {
        plot = first(root, ".detail-desc-text")
    }
    if !plot.is_empty() && plot != "暂无简介" {
        if plot.chars().count() > 300 {
            plot = plot.chars().take(300).collect::<String>() + "..."
        }
        parts.push(plot)
    }
    row.content = parts.join(" ");
    row.tags = doc
        .select(&sel(".premium-tags-top .p-tag"))
        .map(text)
        .filter(|s| !s.is_empty() && !s.starts_with('★'))
        .collect();
    row.datetime = doc
        .select(&sel(".premium-meta-grid .meta-item"))
        .find_map(|e| {
            first(e, ".m-label")
                .contains("时间")
                .then(|| super::super::c_support::date(&first(e, ".m-val")))
                .flatten()
        });
    let poster = item.poster.clone().or_else(|| {
        doc.select(&sel(".premium-poster img"))
            .next()?
            .value()
            .attr("src")
            .filter(|s| !s.is_empty())
            .and_then(|s| base.join(s).ok())
            .map(|u| u.to_string())
    });
    row.images = poster.into_iter().collect();
    Ok(row)
}
#[async_trait]
impl Provider for WoniuProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta {
            name: "woniu",
            priority: 2,
            requires_auth: true,
            auth_kind: AuthKind::Password,
            keyword_filter: KeywordFilterMode::Core,
        }
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let (name, mut p) = self.selected()?;
        let timeout = Duration::from_secs(20).min(ctx.remaining());
        if !self.verify_profile(&name, p.clone(), timeout).await? {
            return Err(ProviderError::AuthRequired);
        }
        p = self
            .profiles
            .read()
            .map_err(lock)?
            .iter()
            .find(|(n, _)| n == &name)
            .map(|(_, p)| p.clone())
            .ok_or(ProviderError::AuthRequired)?;
        p.last_access_at = Utc::now();
        self.save(&name, p.clone())?;
        let session = self.session(&p.cookie)?;
        let mut url = self
            .endpoints
            .base
            .join("/vodsearch/-------------/")
            .unwrap();
        url.query_pairs_mut().append_pair("wd", query.trim());
        let (_, body) = get(
            &session,
            &self.endpoints.base,
            url,
            Duration::from_secs(20).min(ctx.remaining()),
            8 * 1024 * 1024,
        )
        .await?;
        let candidates = items(&String::from_utf8_lossy(&body), &self.endpoints.base);
        if candidates.is_empty() {
            return Ok(vec![]);
        }
        let base = &self.endpoints.base;
        let futures = candidates.into_iter().map(|item| {
            let session = &session;
            async move {
                if ctx.remaining().is_zero() {
                    return Err(ProviderError::Timeout);
                }
                let (_, body) = get(
                    session,
                    base,
                    item.url.clone(),
                    Duration::from_secs(20).min(ctx.remaining()),
                    8 * 1024 * 1024,
                )
                .await?;
                detail(&String::from_utf8_lossy(&body), &item, base)
            }
        });
        let mut rows = super::super::c_support::collect_details(ctx, futures, 6).await?;
        rows.sort_by(|a, b| a.title.cmp(&b.title));
        Ok(rows)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn item() -> Item {
        Item {
            id: "1".into(),
            title: "庆余年".into(),
            url: Url::parse(DEFAULT_BASE).unwrap(),
            poster: None,
            note: "全集".into(),
            meta: "剧集".into(),
        }
    }
    #[test]
    fn cards() {
        let base = Url::parse(DEFAULT_BASE).unwrap();
        let h = r#"<a class="video-card" href="/voddetail/1/"><span class="video-title">test</span><div class="video-poster"><img src="/a.jpg"></div></a><a class="video-card" href="/voddetail/1/" title="duplicate"></a><a class="video-card" href="https://evil.test/voddetail/2/" title="evil"></a>"#;
        let r = items(h, &base);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].poster.as_deref(), Some("https://wn4k.com/a.jpg"));
        assert!(items("bad", &base).is_empty());
    }
    #[test]
    fn links_and_metadata() {
        let base = Url::parse(DEFAULT_BASE).unwrap();
        let row=detail(r#"<h1 class="premium-title">庆余年 第二季</h1><div class="pan-link-item" data-pan-item="https://pan.baidu.com/s/test?pwd=url1 提取码abcd"><div class="pan-link-title">S02 4K</div><div class="pan-link-meta">https://pan.quark.cn/s/other</div></div><div class="pan-link-item"><a class="pan-link-btn" href="https://drive.uc.cn/s/test"></a></div><div class="pan-link-item"><div class="pan-link-meta">https://pan.baidu.com/s/test?pwd=url1</div></div><div id="detailDescText">简介</div><div class="premium-tags-top"><span class="p-tag">★ 8</span><span class="p-tag">2026</span></div><div class="premium-meta-grid"><div class="meta-item"><span class="m-label">更新时间</span><span class="m-val">2026-10-01 12:30</span></div></div>"#,&item(),&base).unwrap();
        assert_eq!(row.links.len(), 2);
        assert_eq!(row.links[0].password.as_deref(), Some("url1"));
        assert_eq!(row.links[0].work_title.as_deref(), Some("庆余年 S02 4K"));
        assert_eq!(row.tags, vec!["2026"]);
        assert!(row.datetime.is_some());
        assert_eq!(row.content, "【全集】 剧集 简介");
    }
    #[test]
    fn passwords_and_invalid() {
        let l = normalize("https://pan.baidu.com/s/a 提取码：abcd", "movie").unwrap();
        assert_eq!(l.password.as_deref(), Some("abcd"));
        assert!(normalize("https://******", "x").is_none());
        assert!(normalize("https://", "x").is_none());
        assert!(normalize("javascript:alert(1)", "x").is_none());
        assert_eq!(
            normalize("https://evil.test/?u=115.com", "x")
                .unwrap()
                .cloud_type,
            crate::core::CloudType::Others
        );
        assert_eq!(work_title("庆余年", "庆余年 S02"), "庆余年 S02");
    }
    #[test]
    fn locked_or_missing() {
        let base = Url::parse(DEFAULT_BASE).unwrap();
        assert_eq!(
            detail("<div class=pan-lock-box></div>", &item(), &base),
            Err(ProviderError::AuthRequired)
        );
        assert!(detail("bad", &item(), &base).is_err());
    }
}
