use std::collections::HashMap;

use super::{CloudType, MergedLink, SearchResult, canonical_url_key};

/// Merge result batches without destroying source order. A duplicate keeps the
/// position of its first occurrence, while its most complete representation wins.
pub fn merge_search_results<I>(batches: I) -> Vec<SearchResult>
where
    I: IntoIterator,
    I::Item: IntoIterator<Item = SearchResult>,
{
    let mut positions = HashMap::<String, usize>::new();
    let mut merged = Vec::new();
    for result in batches.into_iter().flatten() {
        let key = result_key(&result);
        if let Some(&position) = positions.get(&key) {
            if completeness_score(&result) > completeness_score(&merged[position]) {
                merged[position] = result;
            }
        } else {
            positions.insert(key, merged.len());
            merged.push(result);
        }
    }
    merged
}

pub fn result_key(result: &SearchResult) -> String {
    if !result.id.trim().is_empty() {
        result.id.clone()
    } else {
        format!(
            "title:{}:{}",
            result.source,
            result.title.trim().to_lowercase()
        )
    }
}

pub fn completeness_score(result: &SearchResult) -> usize {
    usize::from(!result.id.is_empty()) * 10
        + usize::from(!result.links.is_empty()) * 5
        + result.links.len()
        + usize::from(!result.content.is_empty()) * 3
        + result.title.chars().count() / 10
        + result.tags.len()
}

/// Flatten links while retaining their first source position. For duplicate
/// URLs, newer metadata replaces older metadata in that same position.
pub fn merge_links(
    results: &[SearchResult],
    keyword: Option<&str>,
    clouds: &[CloudType],
) -> Vec<MergedLink> {
    let cloud_filter: std::collections::HashSet<_> = clouds.iter().copied().collect();
    let keyword = keyword.map(str::to_lowercase).filter(|s| !s.is_empty());
    let mut positions = HashMap::<String, usize>::new();
    let mut merged: Vec<MergedLink> = Vec::new();

    for result in results {
        for link in &result.links {
            if !cloud_filter.is_empty() && !cloud_filter.contains(&link.cloud_type) {
                continue;
            }
            let note = link
                .work_title
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(&result.title);
            if keyword
                .as_ref()
                .is_some_and(|word| !note.to_lowercase().contains(word))
            {
                continue;
            }
            let candidate = MergedLink {
                cloud_type: link.cloud_type,
                url: link.url.clone(),
                password: link.password.clone(),
                note: trim_description(note),
                datetime: link.datetime.or(result.datetime),
                source: result.source.clone(),
                images: result.images.clone(),
                check: None,
            };
            let key = canonical_url_key(&link.url);
            if let Some(&position) = positions.get(&key) {
                let existing = &merged[position];
                if candidate.datetime > existing.datetime {
                    merged[position] = candidate;
                } else if existing.password.is_none() && candidate.password.is_some() {
                    merged[position].password = candidate.password;
                }
            } else {
                positions.insert(key, merged.len());
                merged.push(candidate);
            }
        }
    }
    merged
}

fn trim_description(value: &str) -> String {
    let end = ["简介", "描述"]
        .iter()
        .filter_map(|marker| value.find(marker))
        .min()
        .unwrap_or(value.len());
    value[..end].trim().to_owned()
}
