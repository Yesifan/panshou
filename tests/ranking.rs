use std::collections::HashMap;

use chrono::{Duration, TimeZone, Utc};
use pansou::core::{FilterOptions, SearchResult, filter_results, keyword_score, rank_results};

fn result(id: &str, provider: &str, title: &str) -> SearchResult {
    let mut result = SearchResult::provider(id, provider);
    result.title = title.into();
    result
}

#[test]
fn priority_keywords_retain_go_order() {
    assert!(keyword_score("资源合集") > keyword_score("完整资源"));
    assert!(keyword_score("完整资源") > keyword_score("complete resource"));
    assert_eq!(keyword_score("ordinary"), 0);
}

#[test]
fn provider_then_freshness_and_keywords_rank_stably() {
    let now = Utc.with_ymd_and_hms(2026, 9, 8, 0, 0, 0).unwrap();
    let mut high = result("high", "high", "普通");
    high.datetime = Some(now - Duration::days(500));
    let mut fresh = result("fresh", "normal", "最新合集");
    fresh.datetime = Some(now);
    let tie1 = result("tie1", "normal", "普通");
    let tie2 = result("tie2", "normal", "普通");
    let mut priorities = HashMap::new();
    priorities.insert("high".into(), 1);
    let mut values = vec![tie1, tie2, fresh, high];
    rank_results(&mut values, now, &priorities);
    assert_eq!(
        values.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
        ["high", "fresh", "tie1", "tie2"]
    );
}

#[test]
fn include_exclude_and_cloud_filters_compose() {
    let mut keep = result("1", "p", "仙逆 最新");
    keep.content = "高清".into();
    keep.links
        .push(pansou::core::Link::new("https://pan.quark.cn/s/abc"));
    keep.links
        .push(pansou::core::Link::new("https://pan.baidu.com/s/def"));
    let mut excluded = result("2", "p", "仙逆 枪版");
    excluded.content = "枪版".into();
    excluded
        .links
        .push(pansou::core::Link::new("https://pan.quark.cn/s/xyz"));
    let options = FilterOptions {
        include: vec!["仙逆".into(), "高清".into()],
        exclude: vec!["枪版".into()],
        cloud_types: [pansou::core::CloudType::Quark].into_iter().collect(),
    };
    let output = filter_results(vec![keep, excluded], &options);
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].links.len(), 1);
}
