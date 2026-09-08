use std::collections::HashMap;

use chrono::{DateTime, Utc};

use super::{SearchResult, Source};

pub const PRIORITY_KEYWORDS: [&str; 7] = ["合集", "系列", "全", "完", "最新", "附", "complete"];

pub fn rank_results(
    results: &mut [SearchResult],
    now: DateTime<Utc>,
    provider_priorities: &HashMap<String, i32>,
) {
    // `sort_by` is stable, so equal scores preserve source order.
    results.sort_by(|left, right| {
        score_result(right, now, provider_priorities).cmp(&score_result(
            left,
            now,
            provider_priorities,
        ))
    });
}

pub fn score_result(
    result: &SearchResult,
    now: DateTime<Utc>,
    provider_priorities: &HashMap<String, i32>,
) -> i64 {
    let provider = match &result.source {
        Source::Provider { name } => provider_score(*provider_priorities.get(name).unwrap_or(&3)),
        Source::Telegram { .. } => 0,
    };
    time_score(result.datetime, now) + i64::from(keyword_score(&result.title)) + i64::from(provider)
}

pub fn keyword_score(title: &str) -> i32 {
    let lower = title.to_lowercase();
    PRIORITY_KEYWORDS
        .iter()
        .position(|word| lower.contains(word))
        .map_or(0, |position| {
            ((PRIORITY_KEYWORDS.len() - position) * 70) as i32
        })
}

pub fn provider_score(priority: i32) -> i32 {
    match priority {
        1 => 1000,
        2 => 500,
        3 => 0,
        _ if priority >= 4 => -200,
        _ => 0,
    }
}

pub fn time_score(datetime: Option<DateTime<Utc>>, now: DateTime<Utc>) -> i64 {
    let Some(datetime) = datetime else { return 0 };
    let age_days = now.signed_duration_since(datetime).num_seconds().max(0) / 86_400;
    match age_days {
        0..=1 => 500,
        2..=3 => 400,
        4..=7 => 300,
        8..=30 => 200,
        31..=90 => 100,
        91..=365 => 50,
        _ => 20,
    }
}
