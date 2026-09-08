use std::collections::HashSet;

use super::{CloudType, SearchResult};

#[derive(Debug, Clone, Default)]
pub struct FilterOptions {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub cloud_types: HashSet<CloudType>,
}

pub fn filter_results(results: Vec<SearchResult>, options: &FilterOptions) -> Vec<SearchResult> {
    let includes: Vec<String> = options.include.iter().map(|s| s.to_lowercase()).collect();
    let excludes: Vec<String> = options.exclude.iter().map(|s| s.to_lowercase()).collect();
    results
        .into_iter()
        .filter_map(|mut result| {
            let title = result.title.to_lowercase();
            if (!includes.is_empty() && !includes.iter().any(|word| title.contains(word)))
                || excludes.iter().any(|word| title.contains(word))
            {
                return None;
            }
            result.links.retain(|link| {
                let link_title = link
                    .work_title
                    .as_deref()
                    .unwrap_or(&result.title)
                    .to_lowercase();
                (includes.is_empty() || includes.iter().any(|word| link_title.contains(word)))
                    && !excludes.iter().any(|word| link_title.contains(word))
            });
            if !options.cloud_types.is_empty() {
                result
                    .links
                    .retain(|link| options.cloud_types.contains(&link.cloud_type));
            }
            if result.links.is_empty() {
                return None;
            }
            Some(result)
        })
        .collect()
}
