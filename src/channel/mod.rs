//! Persistent Telegram channel selection and candidate-list import.

use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_ENABLED_CHANNELS: usize = 128;
pub const BUILTIN_CATALOG: &str = include_str!("../../data/channels.txt");
const MAX_IMPORT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChannelList {
    pub version: u32,
    #[serde(default)]
    pub channels: Vec<Channel>,
}

impl Default for ChannelList {
    fn default() -> Self {
        Self {
            version: 1,
            channels: vec![Channel {
                name: "tgsearchers3".into(),
                enabled: true,
            }],
        }
    }
}

impl ChannelList {
    pub fn enabled_names(&self) -> Vec<String> {
        self.channels
            .iter()
            .filter(|c| c.enabled)
            .map(|c| c.name.clone())
            .collect()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ChangeSummary {
    pub added: usize,
    pub existing: usize,
    pub changed: usize,
    pub removed: usize,
    pub not_found: Vec<String>,
}

#[derive(Debug, Error)]
pub enum ChannelError {
    #[error("channel file operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid channels.toml: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("cannot serialize channels.toml: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("unsupported channels.toml version {0}; update pansou before using this file")]
    Version(u32),
    #[error("invalid public Telegram channel: {0}")]
    InvalidName(String),
    #[error("channel not found: {0}; add it with `pansou channel add` first")]
    NotFound(String),
    #[error(
        "enabled channel count {0} exceeds MAX_ENABLED_CHANNELS ({MAX_ENABLED_CHANNELS}); disable or remove channels"
    )]
    Limit(usize),
    #[error("invalid channel list at line {line}: {reason}")]
    ImportLine { line: usize, reason: String },
    #[error("channel import failed: {0}")]
    Import(String),
}

/// Normalize a public channel address. Short aliases are accepted for parity
/// with the CLI examples; Telegram determines whether a name actually exists.
pub fn normalize_channel(input: &str) -> Result<String, ChannelError> {
    let input = input.trim();
    let invalid = || ChannelError::InvalidName(input.to_owned());
    let name = if input.contains("://") {
        let url = url::Url::parse(input).map_err(|_| invalid())?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str() != Some("t.me")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(invalid());
        }
        let segments: Vec<_> = url
            .path()
            .trim_end_matches('/')
            .split('/')
            .skip(1)
            .collect();
        match segments.as_slice() {
            [name] => (*name).to_owned(),
            ["s", name] => (*name).to_owned(),
            _ => return Err(invalid()),
        }
    } else {
        input.strip_prefix('@').unwrap_or(input).to_owned()
    };
    if name.is_empty()
        || name.len() > 32
        || !name.as_bytes()[0].is_ascii_alphabetic()
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        || matches!(name.to_ascii_lowercase().as_str(), "joinchat" | "s" | "c")
    {
        return Err(invalid());
    }
    Ok(name.to_ascii_lowercase())
}

pub fn normalize_channels(names: &[String]) -> Result<Vec<String>, ChannelError> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for name in names {
        let name = normalize_channel(name)?;
        if seen.insert(name.clone()) {
            result.push(name);
        }
    }
    Ok(result)
}

pub fn validate_search_channels(names: &[String]) -> Result<Vec<String>, ChannelError> {
    let names = normalize_channels(names)?;
    if names.len() > MAX_ENABLED_CHANNELS {
        return Err(ChannelError::Limit(names.len()));
    }
    Ok(names)
}

pub fn parse_catalog(input: &str) -> Result<Vec<String>, ChannelError> {
    let mut names = Vec::new();
    for (index, line) in input.trim_start_matches('\u{feff}').lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        names.push(
            normalize_channel(line).map_err(|error| ChannelError::ImportLine {
                line: index + 1,
                reason: error.to_string(),
            })?,
        );
    }
    normalize_channels(&names)
}

#[derive(Debug, Clone)]
pub struct ChannelStore {
    path: PathBuf,
}

impl ChannelStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reading does not enforce the enabled limit, so management commands can
    /// repair a hand-edited, over-limit list.
    pub fn load(&self) -> Result<ChannelList, ChannelError> {
        let input = match fs::read_to_string(&self.path) {
            Ok(input) => input,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ChannelList::default());
            }
            Err(error) => return Err(error.into()),
        };
        let mut list: ChannelList = toml::from_str(&input)?;
        if list.version != 1 {
            return Err(ChannelError::Version(list.version));
        }
        let mut channels: Vec<Channel> = Vec::new();
        for channel in list.channels {
            let name = normalize_channel(&channel.name)?;
            if let Some(existing) = channels.iter_mut().find(|c| c.name == name) {
                existing.enabled |= channel.enabled;
            } else {
                channels.push(Channel {
                    name,
                    enabled: channel.enabled,
                });
            }
        }
        list.channels = channels;
        Ok(list)
    }

    fn modify(
        &self,
        enforce_limit: bool,
        operation: impl FnOnce(&mut ChannelList) -> Result<ChangeSummary, ChannelError>,
    ) -> Result<ChangeSummary, ChannelError> {
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.path.with_extension("lock"))?;
        lock.lock()?;
        let mut list = self.load()?;
        let summary = operation(&mut list)?;
        let enabled = list.channels.iter().filter(|c| c.enabled).count();
        if enforce_limit && enabled > MAX_ENABLED_CHANNELS {
            return Err(ChannelError::Limit(enabled));
        }
        let content = toml::to_string_pretty(&list)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(content.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
        Ok(summary)
    }

    pub fn add(&self, names: &[String], enabled: bool) -> Result<ChangeSummary, ChannelError> {
        let names = normalize_channels(names)?;
        self.modify(enabled, |list| {
            let mut summary = ChangeSummary::default();
            for name in names {
                if list.channels.iter().any(|c| c.name == name) {
                    summary.existing += 1;
                } else {
                    list.channels.push(Channel { name, enabled });
                    summary.added += 1;
                }
            }
            Ok(summary)
        })
    }

    pub fn remove(&self, names: &[String]) -> Result<ChangeSummary, ChannelError> {
        let names = normalize_channels(names)?;
        self.modify(false, |list| {
            let mut summary = ChangeSummary::default();
            for name in names {
                let before = list.channels.len();
                list.channels.retain(|c| c.name != name);
                if before == list.channels.len() {
                    summary.not_found.push(name);
                } else {
                    summary.removed += 1;
                }
            }
            Ok(summary)
        })
    }

    pub fn set_enabled(
        &self,
        names: &[String],
        enabled: bool,
    ) -> Result<ChangeSummary, ChannelError> {
        let names = normalize_channels(names)?;
        self.modify(enabled, |list| {
            let mut summary = ChangeSummary::default();
            for name in names {
                let channel = list
                    .channels
                    .iter_mut()
                    .find(|c| c.name == name)
                    .ok_or(ChannelError::NotFound(name))?;
                if channel.enabled != enabled {
                    channel.enabled = enabled;
                    summary.changed += 1;
                }
            }
            Ok(summary)
        })
    }

    pub fn set_all_enabled(&self, enabled: bool) -> Result<ChangeSummary, ChannelError> {
        self.modify(enabled, |list| {
            let mut summary = ChangeSummary::default();
            for channel in &mut list.channels {
                if channel.enabled != enabled {
                    channel.enabled = enabled;
                    summary.changed += 1;
                }
            }
            Ok(summary)
        })
    }

    pub fn import_text(&self, input: &str, enabled: bool) -> Result<ChangeSummary, ChannelError> {
        self.add(&parse_catalog(input)?, enabled)
    }

    pub async fn import_source(
        &self,
        source: &str,
        client: &reqwest::Client,
        timeout: Duration,
        enabled: bool,
    ) -> Result<ChangeSummary, ChannelError> {
        let bytes = if source.contains("://") {
            let url =
                url::Url::parse(source).map_err(|_| ChannelError::Import("invalid URL".into()))?;
            if !matches!(url.scheme(), "http" | "https") {
                return Err(ChannelError::Import(
                    "only HTTP(S) URLs are supported".into(),
                ));
            }
            let mut response = client
                .get(url)
                .timeout(timeout)
                .send()
                .await
                .map_err(|error| ChannelError::Import(error.without_url().to_string()))?
                .error_for_status()
                .map_err(|error| ChannelError::Import(error.without_url().to_string()))?;
            if response
                .content_length()
                .is_some_and(|n| n > MAX_IMPORT_BYTES as u64)
            {
                return Err(ChannelError::Import("list exceeds 1 MiB".into()));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| ChannelError::Import(error.without_url().to_string()))?
            {
                if bytes.len() + chunk.len() > MAX_IMPORT_BYTES {
                    return Err(ChannelError::Import("list exceeds 1 MiB".into()));
                }
                bytes.extend_from_slice(&chunk);
            }
            bytes
        } else {
            fs::read(source)?
        };
        let input = String::from_utf8(bytes)
            .map_err(|_| ChannelError::Import("list must be UTF-8 text".into()))?;
        self.import_text(&input, enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).into()).collect()
    }

    #[test]
    fn normalizes_and_rejects_non_channel_addresses() {
        assert_eq!(
            normalize_channels(&names(&["@Foo", "https://t.me/foo", "https://t.me/s/BAR/"]))
                .unwrap(),
            ["foo", "bar"]
        );
        for input in [
            "",
            "@",
            "foo bar",
            "https://example.com/foo",
            "https://t.me/+private",
            "https://t.me/joinchat/token",
            "https://t.me/foo/123",
            "123foo",
        ] {
            assert!(normalize_channel(input).is_err(), "{input}");
        }
    }

    #[test]
    fn mutations_preserve_states_and_explicit_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChannelStore::new(dir.path().join("channels.toml"));
        assert_eq!(store.load().unwrap().enabled_names(), ["tgsearchers3"]);
        store.add(&names(&["Foo"]), false).unwrap();
        store.add(&names(&["@foo"]), true).unwrap();
        assert_eq!(store.load().unwrap().enabled_names(), ["tgsearchers3"]);
        store.set_enabled(&names(&["foo"]), true).unwrap();
        assert_eq!(
            store.load().unwrap().enabled_names(),
            ["tgsearchers3", "foo"]
        );
        let summary = store
            .remove(&names(&["foo", "tgsearchers3", "missing"]))
            .unwrap();
        assert_eq!(summary.not_found, ["missing"]);
        assert!(store.load().unwrap().channels.is_empty());
    }

    #[test]
    fn failed_batch_does_not_modify_file_and_over_limit_is_repairable() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChannelStore::new(dir.path().join("channels.toml"));
        store.add(&names(&["foo"]), false).unwrap();
        let before = fs::read(store.path()).unwrap();
        assert!(
            store
                .set_enabled(&names(&["foo", "missing"]), true)
                .is_err()
        );
        assert_eq!(fs::read(store.path()).unwrap(), before);
        let too_many = (0..MAX_ENABLED_CHANNELS)
            .map(|i| format!("channel{i}"))
            .collect::<Vec<_>>();
        assert!(store.add(&too_many, true).is_err());
        assert_eq!(fs::read(store.path()).unwrap(), before);
        let list = ChannelList {
            version: 1,
            channels: (0..130)
                .map(|i| Channel {
                    name: format!("channel{i}"),
                    enabled: true,
                })
                .collect(),
        };
        fs::write(store.path(), toml::to_string(&list).unwrap()).unwrap();
        store.set_enabled(&names(&["channel0"]), false).unwrap();
        assert_eq!(store.load().unwrap().enabled_names().len(), 129);
        store.remove(&names(&["channel1"])).unwrap();
        assert_eq!(store.load().unwrap().enabled_names().len(), 128);
    }

    #[test]
    fn imports_are_atomic_idempotent_and_preserve_existing_states() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChannelStore::new(dir.path().join("channels.toml"));
        let summary = store
            .import_text("# comment\n\nFoo\n@foo\nhttps://t.me/bar\n", true)
            .unwrap();
        assert_eq!(summary.added, 2);
        assert_eq!(
            store.load().unwrap().enabled_names(),
            ["tgsearchers3", "foo", "bar"]
        );
        assert_eq!(
            store.import_text("foo\nbar\nbaz", false).unwrap().existing,
            2
        );
        assert_eq!(store.import_text("baz", true).unwrap().existing, 1);
        assert_eq!(
            store.load().unwrap().enabled_names(),
            ["tgsearchers3", "foo", "bar"]
        );
        assert!(matches!(
            store.import_text("qux\ninvalid value", true),
            Err(ChannelError::ImportLine { line: 2, .. })
        ));
        assert_eq!(store.load().unwrap().channels.len(), 4);
        assert_eq!(parse_catalog(BUILTIN_CATALOG).unwrap().len(), 110);
    }

    #[test]
    fn enabled_import_and_enable_all_enforce_limit_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChannelStore::new(dir.path().join("channels.toml"));
        store.add(&names(&["foo"]), false).unwrap();
        let before = fs::read(store.path()).unwrap();
        let catalog = (0..MAX_ENABLED_CHANNELS)
            .map(|i| format!("channel{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(matches!(
            store.import_text(&catalog, true),
            Err(ChannelError::Limit(129))
        ));
        assert_eq!(fs::read(store.path()).unwrap(), before);

        store.import_text(&catalog, false).unwrap();
        let before = fs::read(store.path()).unwrap();
        assert!(matches!(
            store.set_all_enabled(true),
            Err(ChannelError::Limit(130))
        ));
        assert_eq!(fs::read(store.path()).unwrap(), before);
        store.remove(&names(&["foo", "channel0"])).unwrap();
        assert_eq!(store.set_all_enabled(true).unwrap().changed, 127);
        assert_eq!(
            store.load().unwrap().enabled_names().len(),
            MAX_ENABLED_CHANNELS
        );
        assert_eq!(store.set_all_enabled(true).unwrap().changed, 0);
    }

    #[test]
    fn disable_all_repairs_over_limit_and_preserves_empty_lists() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChannelStore::new(dir.path().join("channels.toml"));
        let list = ChannelList {
            version: 1,
            channels: (0..130)
                .map(|i| Channel {
                    name: format!("channel{i}"),
                    enabled: true,
                })
                .collect(),
        };
        fs::write(store.path(), toml::to_string(&list).unwrap()).unwrap();
        assert_eq!(store.set_all_enabled(false).unwrap().changed, 130);
        assert!(store.load().unwrap().enabled_names().is_empty());
        assert_eq!(store.set_all_enabled(false).unwrap().changed, 0);
        store.remove(&list.enabled_names()).unwrap();
        assert_eq!(store.set_all_enabled(true).unwrap().changed, 0);
        assert!(store.load().unwrap().channels.is_empty());
    }

    #[test]
    fn concurrent_writers_do_not_lose_updates() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChannelStore::new(dir.path().join("channels.toml"));
        let handles: Vec<_> = (0..12)
            .map(|i| {
                let store = store.clone();
                std::thread::spawn(move || store.add(&[format!("channel{i}")], true).unwrap())
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(store.load().unwrap().channels.len(), 13);
    }

    #[tokio::test]
    async fn imports_http_and_local_files_without_changing_state_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChannelStore::new(dir.path().join("channels.toml"));
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/good"))
            .respond_with(ResponseTemplate::new(200).set_body_string("@foo\nbar"))
            .mount(&server)
            .await;
        Mock::given(path("/failure"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        Mock::given(path("/large"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("a".repeat(MAX_IMPORT_BYTES + 1)),
            )
            .mount(&server)
            .await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let timeout = Duration::from_secs(2);
        assert_eq!(
            store
                .import_source(&format!("{}/good", server.uri()), &client, timeout, true)
                .await
                .unwrap()
                .added,
            2
        );
        assert_eq!(
            store.load().unwrap().enabled_names(),
            ["tgsearchers3", "foo", "bar"]
        );
        let before = fs::read(store.path()).unwrap();
        for endpoint in ["failure", "large"] {
            assert!(
                store
                    .import_source(
                        &format!("{}/{endpoint}", server.uri()),
                        &client,
                        timeout,
                        true
                    )
                    .await
                    .is_err()
            );
            assert_eq!(fs::read(store.path()).unwrap(), before);
        }
        let file = dir.path().join("input.txt");
        fs::write(&file, "baz").unwrap();
        assert_eq!(
            store
                .import_source(file.to_str().unwrap(), &client, timeout, false)
                .await
                .unwrap()
                .added,
            1
        );
        assert_eq!(
            store.load().unwrap().enabled_names(),
            ["tgsearchers3", "foo", "bar"]
        );
    }

    #[test]
    fn unsupported_schema_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChannelStore::new(dir.path().join("channels.toml"));
        fs::write(store.path(), "version = 2\n").unwrap();
        assert!(matches!(
            store.add(&names(&["foo"]), true),
            Err(ChannelError::Version(2))
        ));
        assert_eq!(fs::read_to_string(store.path()).unwrap(), "version = 2\n");
    }
}
