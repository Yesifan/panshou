use chrono::{TimeZone, Utc};
use pansou::core::{CloudType, Link, SearchResult, Source, merge_links, merge_search_results};

fn result(id: &str, source: Source, title: &str, urls: &[&str]) -> SearchResult {
    SearchResult {
        id: id.into(),
        source,
        datetime: None,
        title: title.into(),
        content: String::new(),
        links: urls.iter().map(|url| Link::new(*url)).collect(),
        tags: vec![],
        images: vec![],
    }
}

#[test]
fn duplicate_result_keeps_position_and_selects_more_complete() {
    let source = Source::provider("demo");
    let sparse = result("1", source.clone(), "短", &[]);
    let rich = result(
        "1",
        source,
        "更完整的搜索结果标题",
        &["https://pan.quark.cn/s/abc"],
    );
    let other = result("2", Source::provider("demo"), "第二", &[]);
    let merged = merge_search_results([vec![sparse, other.clone()], vec![rich.clone()]]);
    assert_eq!(merged, vec![rich, other]);
}

#[test]
fn ids_are_global_across_sources() {
    let merged = merge_search_results([vec![
        result("1", Source::provider("a"), "A", &[]),
        result(
            "1",
            Source::provider("b"),
            "a much more complete title",
            &[],
        ),
    ]]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].source, Source::provider("b"));
}

#[test]
fn duplicate_link_newest_metadata_wins_but_position_is_stable() {
    let old_time = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
    let new_time = Utc.with_ymd_and_hms(2025, 2, 1, 0, 0, 0).unwrap();
    let mut old = result(
        "1",
        Source::telegram("first"),
        "旧标题",
        &["https://pan.baidu.com/s/abc"],
    );
    old.datetime = Some(old_time);
    let mut new = result(
        "2",
        Source::provider("later"),
        "新标题",
        &["https://pan.baidu.com/s/abc?pwd=1234"],
    );
    new.datetime = Some(new_time);
    new.links[0].password = Some("1234".into());
    let last = result(
        "3",
        Source::provider("last"),
        "最后",
        &["https://pan.quark.cn/s/xyz"],
    );

    let merged = merge_links(&[old, new, last], None, &[]);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].note, "新标题");
    assert_eq!(merged[0].password.as_deref(), Some("1234"));
    assert_eq!(merged[0].source, Source::provider("later"));
    assert_eq!(merged[1].cloud_type, CloudType::Quark);
}

#[test]
fn work_title_beats_message_title_and_filters_per_link() {
    let mut item = result(
        "1",
        Source::provider("demo"),
        "大合集",
        &["https://pan.baidu.com/s/abc", "https://pan.quark.cn/s/xyz"],
    );
    item.links[0].work_title = Some("仙逆 全集".into());
    item.links[1].work_title = Some("凡人修仙传".into());
    let merged = merge_links(&[item], Some("仙逆"), &[CloudType::Baidu]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].note, "仙逆 全集");
}
