use super::super::html::{parse_error, words_match};
use super::{Buerchen, Candidate};
use crate::core::ProviderError;
use serde::Deserialize;
use std::collections::HashSet;

pub(super) const LINE_LIMIT: usize = 256 << 10;
#[derive(Default)]
pub(super) struct Parser {
    data: Vec<String>,
    seen: HashSet<String>,
    pub entries: Vec<Candidate>,
    pub done: bool,
}
#[derive(Deserialize)]
struct Event {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    is_type: Option<usize>,
    #[serde(default)]
    is_invalid: i64,
    #[serde(default)]
    code: i64,
    #[serde(default, rename = "type")]
    kind: String,
}
impl Parser {
    pub fn line(
        &mut self,
        line: &str,
        query: &str,
        kind: usize,
        site: &Buerchen,
    ) -> Result<(), ProviderError> {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            return self.consume(query, kind, site);
        }
        if let Some(data) = line.strip_prefix("data:") {
            self.data
                .push(data.strip_prefix(' ').unwrap_or(data).to_owned());
        } else if ![":", "event:", "id:", "retry:", "线路："]
            .iter()
            .any(|prefix| line.starts_with(*prefix))
        {
            if let Ok(value) = serde_json::from_str::<Event>(line)
                && value.code != 0
                && value.code != 200
            {
                return Err(site.api_error(value.code));
            }
            return Err(parse_error("buerchen response is not an SSE stream"));
        }
        Ok(())
    }
    pub fn consume(
        &mut self,
        query: &str,
        kind: usize,
        site: &Buerchen,
    ) -> Result<(), ProviderError> {
        let data = std::mem::take(&mut self.data).join("\n");
        let data = data.trim();
        if data.is_empty() {
            return Ok(());
        }
        if data.starts_with("[DONE]") {
            if data.contains("暂无可用线路") {
                return Err(parse_error("buerchen has no usable search route"));
            }
            self.done = true;
            return Ok(());
        }
        let event: Event =
            serde_json::from_str(data).map_err(|_| parse_error("malformed buerchen SSE event"))?;
        if event.code != 0 && event.code != 200 {
            return Err(site.api_error(event.code));
        }
        if event.kind == "error" {
            return Err(parse_error("buerchen SSE route failed"));
        }
        let title = super::clean_text(&event.title);
        let url = event.url.trim();
        if event.is_invalid != 0
            || event.is_type.is_some_and(|k| k != kind)
            || url.is_empty()
            || url.len() > 8192
            || !words_match(&title, query)
            || !self.seen.insert(url.to_owned())
        {
            return Ok(());
        }
        self.entries.push(Candidate {
            title,
            url: url.to_owned(),
            description: super::clean_text(&event.description),
            kind,
            daily: false,
        });
        if self.entries.len() == 6 {
            self.done = true;
        }
        Ok(())
    }
}
