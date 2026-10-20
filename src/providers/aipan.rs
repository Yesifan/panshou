//! 爱盼 JSON catalog: search -> movie detail, not a generic pan search API.
use super::{UA, clean, fetch, password};
use crate::core::{CloudType, Link, ProviderError, SearchResult};
use crate::providers::{KeywordFilterMode, Provider, ProviderMeta, SearchContext};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use serde::Deserialize;
use std::{collections::HashSet, time::Duration};
use url::Url;

#[derive(Debug, Clone)]
pub struct AipanProvider {
    base: Url,
}
impl Default for AipanProvider {
    fn default() -> Self {
        Self::new(Url::parse("https://www.aipan.me/").unwrap())
    }
}
impl AipanProvider {
    pub fn new(base: Url) -> Self {
        Self { base }
    }
}
#[derive(Deserialize)]
struct Search {
    movies: Vec<Movie>,
}
#[derive(Deserialize)]
struct Movie {
    id: String,
    title: String,
    #[serde(default)]
    cover: String,
}
#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Detail {
    title: String,
    aka: String,
    year: u32,
    synopsis: String,
    genres: Vec<String>,
    region: String,
    directors: Vec<String>,
    actors: Vec<String>,
    release_label: String,
    language: String,
    #[serde(rename = "imdbRating")]
    imdb_rating: f64,
    pan_links: Vec<Resource>,
    resources: Vec<Resource>,
    poster_url: String,
}
#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Resource {
    kind: String,
    url: String,
    title: String,
    name: String,
    size_label: String,
}
fn kind(kind: &str, raw: &str) -> CloudType {
    match kind.trim().to_ascii_lowercase().as_str() {
        "ali" | "alipan" | "aliyundrive" | "aliyun" => CloudType::Aliyun,
        "guangya_pan" | "guangya" => CloudType::Guangya,
        "189cloud" | "tianyi" => CloudType::Tianyi,
        "123pan" | "123" => CloudType::Pan123,
        "139" | "mobile" => CloudType::Mobile,
        other => other.parse().unwrap_or_else(|_| Link::new(raw).cloud_type),
    }
}
fn convert(movie: Movie, detail: Detail) -> Option<SearchResult> {
    let title = if detail.title.trim().is_empty() {
        clean(&movie.title)
    } else {
        clean(&detail.title)
    };
    let mut seen = HashSet::new();
    let mut links = Vec::new();
    for row in detail.pan_links.iter().chain(&detail.resources) {
        let raw = row
            .url
            .trim()
            .trim_matches(|c| "\"'<>(),.;".contains(c))
            .replace("&amp;", "&");
        // ED2K's pipe-delimited `//|file|...` is not an HTTP authority and
        // is rejected by Url::parse. Validate these native schemes before
        // requiring host-bearing URL syntax for ordinary HTTP(S) shares.
        let lower = raw.to_ascii_lowercase();
        let native_link = lower.starts_with("magnet:") || lower.starts_with("ed2k:");
        if !native_link
            && !Url::parse(&raw).ok().is_some_and(|url| {
                matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
            })
        {
            continue;
        }
        let ty = kind(&row.kind, &raw);
        if ty == CloudType::Others {
            continue;
        }
        let mut link = Link::new(raw);
        link.cloud_type = ty;
        link.password = password(&link.url, &format!("{} {}", row.title, row.name));
        link.work_title = Some(title.clone());
        if seen.insert((ty, link.url.clone(), link.password.clone())) {
            links.push(link);
        }
        if links.len() == 64 {
            break;
        }
    }
    if links.is_empty() {
        return None;
    }
    let mut result = SearchResult::provider(format!("aipan-{}", movie.id), "aipan");
    result.title = title;
    result.links = links;
    let mut content = Vec::new();
    for (label, value) in [
        ("别名", detail.aka),
        ("上映", detail.release_label),
        ("地区", detail.region.clone()),
        ("类型", detail.genres.join(", ")),
        ("导演", detail.directors.join(" / ")),
        ("主演", detail.actors.join(" / ")),
        ("语言", detail.language.clone()),
    ] {
        if !value.is_empty() {
            content.push(format!("{label}：{}", clean(&value)));
        }
    }
    if detail.year > 0 {
        content.push(format!("年份：{}", detail.year));
    }
    if detail.imdb_rating > 0.0 {
        content.push(format!("评分：{:.1}", detail.imdb_rating));
    }
    if !detail.synopsis.is_empty() {
        content.push(clean(&detail.synopsis));
    }
    for row in detail.pan_links.iter().chain(&detail.resources) {
        let name = if row.name.is_empty() {
            &row.title
        } else {
            &row.name
        };
        if !name.is_empty() {
            content.push(format!(
                "资源：{} [{}]",
                clean(name),
                clean(&row.size_label)
            ));
        }
    }
    result.content = content.join(" | ");
    let mut tags = vec!["爱盼".into()];
    tags.extend(detail.genres);
    tags.extend([detail.region, detail.language]);
    if detail.year > 0 {
        tags.push(detail.year.to_string());
    }
    let mut seen = HashSet::new();
    result.tags = tags
        .into_iter()
        .filter(|s| !s.is_empty() && seen.insert(s.clone()))
        .collect();
    let cover = if detail.poster_url.is_empty() {
        movie.cover
    } else {
        detail.poster_url
    };
    if !cover.is_empty() {
        result.images.push(cover);
    }
    Some(result)
}
#[async_trait]
impl Provider for AipanProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("aipan", 2, KeywordFilterMode::Core)
    }
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut url = self.base.join("api/movies/search").unwrap();
        url.query_pairs_mut().append_pair("q", query);
        let request = |url: Url, referer: Url| {
            ctx.client
                .get(url)
                .header("User-Agent", UA)
                .header("Accept", "application/json, text/plain, */*")
                .header("Origin", self.base.origin().ascii_serialization())
                .header("Referer", referer.as_str())
        };
        let body = fetch(
            request(url, self.base.clone()),
            "aipan",
            "search",
            ctx.timeout.min(Duration::from_secs(12)),
            8 << 20,
            &[],
        )
        .await?;
        let response: Search = serde_json::from_str(&body)
            .map_err(|_| ProviderError::Parse("aipan search: malformed JSON".into()))?;
        let mut seen = HashSet::new();
        let movies = response
            .movies
            .into_iter()
            .filter_map(|mut m| {
                m.id = m.id.trim().to_owned();
                m.title = clean(&m.title);
                (!m.id.is_empty() && !m.title.is_empty() && seen.insert(m.id.clone())).then_some(m)
            })
            .take(12);
        let failures = &std::sync::Mutex::new(None);
        let rows = stream::iter(movies.map(|m| async move {
            let mut url = self.base.join("api/movies/detail/").ok()?;
            url.path_segments_mut().ok()?.pop_if_empty().push(&m.id);
            let mut referer = self.base.join("movie/").ok()?;
            referer.path_segments_mut().ok()?.pop_if_empty().push(&m.id);
            let body = fetch(
                request(url, referer),
                "aipan",
                "detail",
                ctx.timeout.min(Duration::from_secs(12)),
                8 << 20,
                &[],
            )
            .await;
            let body = super::remember_failure(body, failures)?;
            let detail: Detail = serde_json::from_str(&body).ok()?;
            let result = convert(m, detail);
            if let Some(row) = &result {
                ctx.publish(vec![row.clone()]);
            }
            result
        }))
        .buffered(6)
        .collect::<Vec<_>>()
        .await;
        if let Some(error) = failures.lock().unwrap().take() {
            return Err(error);
        }
        if rows.iter().any(Option::is_none) {
            return Err(ProviderError::Unavailable(
                "aipan: one or more details failed or contained no usable links".into(),
            ));
        }
        Ok(rows.into_iter().flatten().collect())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pipe_delimited_ed2k_links_preserve_the_native_protocol() {
        let raw = "ed2k://|file|test|1|01234567890123456789012345678901|/";
        assert!(Url::parse(raw).is_err());
        for kind in ["ed2k", ""] {
            let movie = Movie {
                id: "native".into(),
                title: "Native resource".into(),
                cover: String::new(),
            };
            let detail = Detail {
                resources: vec![
                    Resource {
                        kind: kind.into(),
                        url: raw.into(),
                        ..Default::default()
                    },
                    Resource {
                        kind: "ed2k".into(),
                        url: "https://|file|invalid/".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            };
            let row = convert(movie, detail).unwrap();
            assert_eq!(row.links.len(), 1);
            assert_eq!(row.links[0].url, raw);
            assert_eq!(row.links[0].cloud_type, CloudType::Ed2k);
        }
    }

    #[test]
    fn fixtures() {
        let search: Search = serde_json::from_str(include_str!(
            "../../tests/fixtures/providers/aipan/success.json"
        ))
        .unwrap();
        let detail: Detail = serde_json::from_str(include_str!(
            "../../tests/fixtures/providers/aipan/detail.json"
        ))
        .unwrap();
        let row = convert(search.movies.into_iter().next().unwrap(), detail).unwrap();
        assert_eq!(row.links.len(), 5);
        assert_eq!(
            row.links
                .iter()
                .map(|link| link.cloud_type)
                .collect::<Vec<_>>(),
            [
                CloudType::Quark,
                CloudType::Guangya,
                CloudType::Aliyun,
                CloudType::Magnet,
                CloudType::Ed2k
            ]
        );
        assert_eq!(row.links[2].password.as_deref(), Some("Ab12"));
        assert!(row.content.contains("导演：测试导演"));
        assert!(row.content.contains("2 GB"));
        assert_eq!(row.links[0].work_title.as_deref(), Some("测试影片"));
        assert!(
            serde_json::from_str::<Search>(include_str!(
                "../../tests/fixtures/providers/aipan/empty.json"
            ))
            .unwrap()
            .movies
            .is_empty()
        );
        assert!(
            serde_json::from_str::<Search>(include_str!(
                "../../tests/fixtures/providers/aipan/malformed.json"
            ))
            .is_err()
        );
    }
}
