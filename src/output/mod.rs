use std::io::{self, Write};

use comfy_table::{Cell, Table};
use serde::Serialize;

use crate::{check, search::SearchOutcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Table,
    Json,
    Jsonl,
}

pub fn write_search<W: Write>(
    mut writer: W,
    outcome: &SearchOutcome,
    format: OutputFormat,
) -> anyhow::Result<()> {
    match format {
        OutputFormat::Json => serde_json::to_writer_pretty(&mut writer, outcome)?,
        OutputFormat::Jsonl => {
            for links in outcome.links_by_type.values() {
                for link in links {
                    serde_json::to_writer(&mut writer, link)?;
                    writeln!(writer)?;
                }
            }
        }
        OutputFormat::Table => {
            writeln!(writer, "Query: {}", outcome.query)?;
            writeln!(writer, "Sources succeeded: {}", outcome.successful_sources)?;
            writeln!(writer, "Links: {}", outcome.total_links)?;
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
        OutputFormat::Jsonl => write_jsonl(&mut writer, results)?,
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

fn write_jsonl<W: Write, T: Serialize>(writer: &mut W, values: &[T]) -> anyhow::Result<()> {
    for value in values {
        serde_json::to_writer(&mut *writer, value)?;
        writeln!(writer)?;
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
        search::SearchOutcome,
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

    #[test]
    fn json_uses_stable_envelope() {
        let mut bytes = Vec::new();
        write_search(&mut bytes, &outcome(), OutputFormat::Json).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["query"], "仙逆");
        assert_eq!(value["total_links"], 1);
        assert!(value.get("successful_sources").is_none());
    }

    #[test]
    fn jsonl_has_one_link_per_line() {
        let mut bytes = Vec::new();
        write_search(&mut bytes, &outcome(), OutputFormat::Jsonl).unwrap();
        let lines = String::from_utf8(bytes).unwrap();
        assert_eq!(lines.lines().count(), 1);
        assert!(lines.contains("pan.quark.cn"));
    }

    #[test]
    fn table_is_human_readable() {
        let mut bytes = Vec::new();
        write_search(&mut bytes, &outcome(), OutputFormat::Table).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("Query: 仙逆"));
        assert!(text.contains("[quark]"));
    }
}
