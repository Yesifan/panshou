use std::io::{self, Write};

use comfy_table::{Cell, Table};
use serde::Serialize;

use crate::{
    check,
    search::{SearchEvent, SearchOutcome},
};

/// Write one append-only event and make it immediately visible to pipes.
/// Progress is handled by the caller on stderr, never in result output.
pub fn write_search_event<W: Write>(
    mut writer: W,
    event: &SearchEvent,
    format: OutputFormat,
) -> anyhow::Result<()> {
    if matches!(format, OutputFormat::Json) {
        anyhow::bail!("search JSON output was removed; use --format jsonl");
    }
    if matches!(event, SearchEvent::Progress { .. }) {
        return Ok(());
    }
    if matches!(format, OutputFormat::Jsonl) {
        serde_json::to_writer(&mut writer, event)?;
        writeln!(writer)?;
    } else {
        match event {
            SearchEvent::Result { link, .. } | SearchEvent::ResultUpdate { link, .. } => {
                let kind = if matches!(event, SearchEvent::ResultUpdate { .. }) {
                    "UPDATE"
                } else {
                    "RESULT"
                };
                writeln!(writer, "[{kind}] [{}] {}", link.cloud_type, link.note)?;
                writeln!(writer, "  {}", link.url)?;
                if let Some(password) = &link.password {
                    writeln!(writer, "  password: {password}")?;
                }
                writeln!(writer, "  source: {}", link.source)?;
                if let Some(check) = &link.check {
                    writeln!(writer, "  check: {:?}", check.state)?;
                }
            }
            SearchEvent::Summary { summary } => {
                writeln!(
                    writer,
                    "Links: {} · Sources: {} completed, {} failed, {} cancelled, {} not started",
                    summary.total_links,
                    summary.sources_completed,
                    summary.sources_failed,
                    summary.sources_cancelled,
                    summary.sources_not_started
                )?;
                writeln!(
                    writer,
                    "Finish: {:?} · Partial: {} · {}ms",
                    summary.finish_reason, summary.partial, summary.duration_ms
                )?;
            }
            SearchEvent::SourceError { .. } | SearchEvent::Progress { .. } => {}
        }
    }
    writer.flush()?;
    Ok(())
}

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
        OutputFormat::Json => anyhow::bail!("search JSON output was removed; use --format jsonl"),
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
    fn collected_search_also_rejects_removed_json_format() {
        let mut bytes = Vec::new();
        assert!(write_search(&mut bytes, &outcome(), OutputFormat::Json).is_err());
        assert!(bytes.is_empty());
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

    #[test]
    fn streaming_jsonl_is_tagged_and_progress_does_not_pollute_stdout() {
        let mut bytes = Vec::new();
        let link = outcome().links_by_type["quark"][0].clone();
        write_search_event(
            &mut bytes,
            &SearchEvent::Result {
                id: "stable".into(),
                link: link.clone(),
            },
            OutputFormat::Jsonl,
        )
        .unwrap();
        write_search_event(
            &mut bytes,
            &SearchEvent::Progress {
                completed: 1,
                selected: 2,
                links: 1,
            },
            OutputFormat::Jsonl,
        )
        .unwrap();
        write_search_event(
            &mut bytes,
            &SearchEvent::ResultUpdate {
                id: "stable".into(),
                link,
            },
            OutputFormat::Jsonl,
        )
        .unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let events: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["event"], "result");
        assert_eq!(events[1]["event"], "result_update");
        assert_eq!(events[0]["id"], events[1]["id"]);
    }

    #[test]
    fn streaming_rejects_json_and_flushes_each_result() {
        #[derive(Default)]
        struct Writer {
            bytes: Vec<u8>,
            flushes: usize,
        }
        impl Write for Writer {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.bytes.write(buf)
            }
            fn flush(&mut self) -> io::Result<()> {
                self.flushes += 1;
                Ok(())
            }
        }
        let event = SearchEvent::Result {
            id: "stable".into(),
            link: outcome().links_by_type["quark"][0].clone(),
        };
        let mut writer = Writer::default();
        assert!(write_search_event(&mut writer, &event, OutputFormat::Json).is_err());
        write_search_event(&mut writer, &event, OutputFormat::Jsonl).unwrap();
        assert_eq!(writer.flushes, 1);
    }
}
