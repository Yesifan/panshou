use std::io::{self, Write};

use comfy_table::{Cell, Table};
use serde::Serialize;

use crate::{
    check,
    search::{SearchOutcome, SearchSummary},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Table,
    Json,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchCheckSummary {
    pub enabled: bool,
    pub candidates: usize,
    pub checked: usize,
    pub valid: usize,
}

pub fn write_search<W: Write>(
    mut writer: W,
    outcome: &SearchOutcome,
    summary: &SearchSummary,
    checks: &SearchCheckSummary,
    format: OutputFormat,
) -> anyhow::Result<()> {
    match format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            struct SearchDocument<'a> {
                query: &'a str,
                total_results: usize,
                total_links: usize,
                links_by_type: &'a std::collections::BTreeMap<String, Vec<crate::core::MergedLink>>,
                source_errors: &'a [crate::search::SourceError],
                duration_ms: u128,
                summary: &'a SearchSummary,
                checks: &'a SearchCheckSummary,
            }

            serde_json::to_writer_pretty(
                &mut writer,
                &SearchDocument {
                    query: &outcome.query,
                    total_results: outcome.total_results,
                    total_links: outcome.total_links,
                    links_by_type: &outcome.links_by_type,
                    source_errors: &outcome.source_errors,
                    duration_ms: outcome.duration_ms,
                    summary,
                    checks,
                },
            )?;
            writeln!(writer)?;
        }
        OutputFormat::Table => {
            writeln!(writer, "Query: {}", outcome.query)?;
            for (cloud, links) in &outcome.links_by_type {
                writeln!(writer, "\n[{cloud}]")?;
                for (index, link) in links.iter().enumerate() {
                    writeln!(writer, "{}. {}", index + 1, link.note)?;
                    writeln!(writer, "   {}", link.url)?;
                    if let Some(password) = &link.password {
                        writeln!(writer, "   password: {password}")?;
                    }
                    writeln!(writer, "   source: {}", link.source)?;
                }
            }

            writeln!(writer)?;
            if checks.enabled {
                writeln!(
                    writer,
                    "Links: {} valid / {} candidates ({} checked)",
                    checks.valid, checks.candidates, checks.checked
                )?;
            } else {
                writeln!(writer, "Links: {} unchecked", outcome.total_links)?;
            }
            writeln!(
                writer,
                "Sources: {} completed, {} failed, {} cancelled, {} not started",
                summary.sources_completed,
                summary.sources_failed,
                summary.sources_cancelled,
                summary.sources_not_started
            )?;
            writeln!(
                writer,
                "Finish: {:?} · Partial: {} · Elapsed: {}ms",
                summary.finish_reason, summary.partial, summary.duration_ms
            )?;
        }
    }
    Ok(())
}

pub fn write_checks<W: Write>(
    mut writer: W,
    results: &[check::CheckResult],
    format: OutputFormat,
) -> anyhow::Result<()> {
    match format {
        OutputFormat::Json => serde_json::to_writer_pretty(&mut writer, results)?,
        OutputFormat::Table => {
            let mut table = Table::new();
            table.set_header(["TYPE", "STATE", "URL", "SUMMARY", "CACHE"]);
            for result in results {
                table.add_row([
                    Cell::new(result.cloud_type.to_string()),
                    Cell::new(format!("{:?}", result.state).to_ascii_lowercase()),
                    Cell::new(&result.url),
                    Cell::new(result.summary.as_deref().unwrap_or_default()),
                    Cell::new(if result.cache_hit { "yes" } else { "no" }),
                ]);
            }
            writeln!(writer, "{table}")?;
        }
    }
    Ok(())
}

pub fn stdout() -> io::Stdout {
    io::stdout()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::{CloudType, MergedLink, Source},
        search::{FinishReason, SearchOutcome, SearchSummary},
    };
    use std::collections::BTreeMap;

    fn outcome() -> SearchOutcome {
        let link = MergedLink {
            cloud_type: CloudType::Quark,
            url: "https://pan.quark.cn/s/example".into(),
            password: None,
            note: "仙逆 全集".into(),
            datetime: None,
            source: Source::provider("fixture"),
            images: vec![],
            check: None,
        };
        SearchOutcome {
            query: "仙逆".into(),
            total_results: 1,
            total_links: 1,
            results: vec![],
            links_by_type: BTreeMap::from([("quark".into(), vec![link])]),
            source_errors: vec![],
            duration_ms: 12,
            successful_sources: 1,
        }
    }

    fn summary() -> SearchSummary {
        SearchSummary {
            total_links: 1,
            sources_selected: 3,
            sources_completed: 1,
            sources_failed: 1,
            sources_cancelled: 1,
            sources_not_started: 0,
            partial: true,
            finish_reason: FinishReason::Deadline,
            search_duration_ms: 10,
            duration_ms: 12,
        }
    }

    fn checks(enabled: bool) -> SearchCheckSummary {
        SearchCheckSummary {
            enabled,
            candidates: 2,
            checked: 2,
            valid: 1,
        }
    }

    #[test]
    fn json_is_one_summary_document_without_raw_results() {
        let mut bytes = Vec::new();
        write_search(
            &mut bytes,
            &outcome(),
            &summary(),
            &checks(true),
            OutputFormat::Json,
        )
        .unwrap();
        let document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(document["query"], "仙逆");
        assert_eq!(document["summary"]["finish_reason"], "deadline");
        assert_eq!(document["checks"]["candidates"], 2);
        assert!(document.get("results").is_none());
    }

    #[test]
    fn table_is_human_readable() {
        let mut bytes = Vec::new();
        write_search(
            &mut bytes,
            &outcome(),
            &summary(),
            &checks(true),
            OutputFormat::Table,
        )
        .unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("Query: 仙逆"));
        assert!(text.contains("[quark]"));
        assert!(text.contains("Links: 1 valid / 2 candidates (2 checked)"));
        assert!(text.contains("Sources: 1 completed, 1 failed, 1 cancelled, 0 not started"));
        assert!(text.contains("Elapsed: 12ms"));
    }

    #[test]
    fn unchecked_table_labels_links() {
        let mut bytes = Vec::new();
        write_search(
            &mut bytes,
            &outcome(),
            &summary(),
            &checks(false),
            OutputFormat::Table,
        )
        .unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("Links: 1 unchecked"));
    }
}
