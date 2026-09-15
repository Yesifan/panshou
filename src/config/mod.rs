//! Application configuration and precedence handling.

mod paths;

pub use paths::AppPaths;

use std::{env, fs, path::Path};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const DEFAULT_PROVIDERS: &[&str] = &[
    "pansearch",
    "yunsou",
    "djgou",
    "hdmoli",
    "meitizy",
    "yulinshufa",
    "clxiong",
    "jsnoteclub",
    "duanjuw",
    "dyyj",
    "jupansou",
    "cldi",
    "clmao",
    "cyg",
    "susu",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub network: NetworkConfig,
    pub search: SearchConfig,
    pub check: CheckConfig,
    pub providers: ProviderConfigs,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkConfig {
    pub proxy: Option<String>,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SearchConfig {
    pub jobs: usize,
    pub all_timeout_secs: u64,
    #[serde(skip)]
    pub channels: Vec<String>,
    #[serde(rename = "channels", skip_serializing)]
    pub deprecated_channels: Option<Vec<String>>,
    pub providers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct CheckConfig {
    pub enabled_cache: bool,
    pub jobs: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ProviderConfigs {
    pub gying: GyingConfig,
    pub panlian: PanlianConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct GyingConfig {
    pub base_url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct PanlianConfig {
    pub blocked_clouds: Vec<String>,
}

/// Command-line values. Applying this last establishes the documented CLI
/// > environment > file > defaults precedence without coupling config to clap.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigOverrides {
    pub proxy: Option<String>,
    pub timeout_secs: Option<u64>,
    pub all_timeout_secs: Option<u64>,
    pub search_jobs: Option<usize>,
    pub channels: Option<Vec<String>>,
    pub providers: Option<Vec<String>>,
    pub check_jobs: Option<usize>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error(transparent)]
    Channels(#[from] crate::channel::ChannelError),
    #[error("failed to read config {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("invalid config {path}: {source}")]
    Parse {
        path: String,
        source: toml::de::Error,
    },
    #[error("invalid value for {name}: {value}")]
    InvalidEnvironment { name: &'static str, value: String },
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            proxy: None,
            timeout_secs: 30,
        }
    }
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            jobs: 8,
            all_timeout_secs: 600,
            channels: vec!["tgsearchers3".into()],
            deprecated_channels: None,
            providers: DEFAULT_PROVIDERS.iter().map(|v| (*v).to_owned()).collect(),
        }
    }
}

impl Default for CheckConfig {
    fn default() -> Self {
        Self {
            enabled_cache: true,
            jobs: 8,
        }
    }
}

impl Default for GyingConfig {
    fn default() -> Self {
        Self {
            base_url: "https://www.xn--wcv59z.com".into(),
        }
    }
}

impl Config {
    /// Network-only commands must remain usable with unrelated obsolete data.
    pub fn load_network(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct NetworkOnly {
            network: NetworkConfig,
        }
        let path = path.as_ref();
        let network = match fs::read_to_string(path) {
            Ok(input) => {
                toml::from_str::<NetworkOnly>(&input)
                    .map_err(|source| ConfigError::Parse {
                        path: path.display().to_string(),
                        source,
                    })?
                    .network
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => NetworkConfig::default(),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.display().to_string(),
                    source,
                });
            }
        };
        let mut config = Self {
            network,
            ..Self::default()
        };
        config.apply_network_environment()?;
        config.validate()?;
        Ok(config)
    }

    /// Load the platform config file, then apply process environment values.
    pub fn load(paths: &AppPaths) -> Result<Self, ConfigError> {
        Self::load_with_channels(&paths.config_file, Some(&paths.channels_file))
    }

    /// Load normal configuration and environment without opening TG data.
    /// Commands that do not use channels remain available when that file needs repair.
    pub fn load_without_channels(paths: &AppPaths) -> Result<Self, ConfigError> {
        Self::load_with_channels(&paths.config_file, None)
    }

    /// A missing file is equivalent to an empty config file.
    pub fn load_from(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        Self::load_with_channels(path, Some(&path.with_file_name("channels.toml")))
    }

    fn load_with_channels(path: &Path, channels_path: Option<&Path>) -> Result<Self, ConfigError> {
        let mut config = if path.exists() {
            let input = fs::read_to_string(path).map_err(|source| ConfigError::Read {
                path: path.display().to_string(),
                source,
            })?;
            toml::from_str(&input).map_err(|source| ConfigError::Parse {
                path: path.display().to_string(),
                source,
            })?
        } else {
            Self::default()
        };
        config.search.channels = match channels_path {
            Some(path) => crate::channel::ChannelStore::new(path)
                .load()?
                .enabled_names(),
            None => Vec::new(),
        };
        config.apply_environment()?;
        config.validate()?;
        Ok(config)
    }

    pub fn apply_overrides(&mut self, values: ConfigOverrides) -> Result<(), ConfigError> {
        if let Some(proxy) = values.proxy {
            self.network.proxy = nonempty(proxy);
        }
        if let Some(timeout) = values.timeout_secs {
            self.network.timeout_secs = timeout;
        }
        if let Some(timeout) = values.all_timeout_secs {
            self.search.all_timeout_secs = timeout;
        }
        if let Some(jobs) = values.search_jobs {
            self.search.jobs = jobs;
        }
        if let Some(channels) = values.channels {
            self.search.channels = normalized_list(channels);
        }
        if let Some(providers) = values.providers {
            self.search.providers = normalized_list(providers);
        }
        if let Some(jobs) = values.check_jobs {
            self.check.jobs = jobs;
        }
        self.validate()
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.network.timeout_secs == 0 {
            return Err(ConfigError::Invalid(
                "network.timeout_secs must be greater than zero".into(),
            ));
        }
        if self.search.all_timeout_secs == 0 {
            return Err(ConfigError::Invalid(
                "search.all_timeout_secs must be greater than zero".into(),
            ));
        }
        if self.search.jobs == 0 || self.check.jobs == 0 {
            return Err(ConfigError::Invalid(
                "job counts must be greater than zero".into(),
            ));
        }
        Ok(())
    }

    fn apply_environment(&mut self) -> Result<(), ConfigError> {
        self.apply_network_environment()?;
        // Compatibility variables are fallback-only.
        if let Some(channels) = env::var_os("CHANNELS").and_then(|v| v.into_string().ok()) {
            self.search.channels = csv(&channels);
        }
        if let Some(providers) = env::var_os("ENABLED_PLUGINS").and_then(|v| v.into_string().ok()) {
            self.search.providers = csv(&providers);
        }

        if let Some(value) = parse_env::<u64>("PANSOU_ALL_TIMEOUT_SECS")? {
            self.search.all_timeout_secs = value;
        }
        if let Some(value) = parse_env::<usize>("PANSOU_JOBS")? {
            self.search.jobs = value;
        }
        if let Ok(value) = env::var("PANSOU_CHANNELS") {
            self.search.channels = csv(&value);
        }
        if let Ok(value) = env::var("PANSOU_PROVIDERS") {
            self.search.providers = csv(&value);
        }
        if let Some(value) = parse_env::<usize>("PANSOU_CHECK_JOBS")? {
            self.check.jobs = value;
        }
        Ok(())
    }

    fn apply_network_environment(&mut self) -> Result<(), ConfigError> {
        // Specific proxy compatibility values precede generic ones.
        if let Some(proxy) = first_env(&["PROXY", "HTTPS_PROXY", "HTTP_PROXY"]) {
            self.network.proxy = nonempty(proxy);
        }
        if let Ok(proxy) = env::var("PANSOU_PROXY") {
            self.network.proxy = nonempty(proxy);
        }
        if let Some(value) = parse_env::<u64>("PANSOU_TIMEOUT_SECS")? {
            self.network.timeout_secs = value;
        }
        Ok(())
    }
}

fn parse_env<T>(name: &'static str) -> Result<Option<T>, ConfigError>
where
    T: std::str::FromStr,
{
    let Ok(value) = env::var(name) else {
        return Ok(None);
    };
    value
        .parse()
        .map(Some)
        .map_err(|_| ConfigError::InvalidEnvironment { name, value })
}

fn first_env(names: &[&str]) -> Option<String> {
    names
        .iter()
        .filter_map(|name| env::var(name).ok())
        .find(|value| !value.trim().is_empty())
}

fn nonempty(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn csv(value: &str) -> Vec<String> {
    normalized_list(value.split(',').map(str::to_owned).collect())
}

fn normalized_list(values: Vec<String>) -> Vec<String> {
    let mut result = Vec::new();
    for value in values {
        let value = value.trim();
        if !value.is_empty() && !result.iter().any(|known| known == value) {
            result.push(value.to_owned());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let config = Config::default();
        assert_eq!(config.network.timeout_secs, 30);
        assert_eq!(config.search.jobs, 8);
        assert_eq!(config.search.all_timeout_secs, 600);
        assert_eq!(config.search.providers.len(), 15);
    }

    #[test]
    fn legacy_channels_are_detected_but_never_used_or_serialized() {
        let config: Config = toml::from_str("[search]\nchannels = ['legacy']\n").unwrap();
        assert_eq!(
            config.search.deprecated_channels,
            Some(vec!["legacy".into()])
        );
        assert_eq!(config.search.channels, ["tgsearchers3"]);
        let serialized = toml::to_string(&config).unwrap();
        assert!(!serialized.contains("channels"));
        assert!(!serialized.contains("legacy"));
    }

    #[test]
    fn all_timeout_can_be_overridden_and_must_be_positive() {
        let mut config = Config::default();
        config
            .apply_overrides(ConfigOverrides {
                all_timeout_secs: Some(42),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(config.search.all_timeout_secs, 42);
        assert!(
            config
                .apply_overrides(ConfigOverrides {
                    all_timeout_secs: Some(0),
                    ..Default::default()
                })
                .is_err()
        );
    }

    #[test]
    fn network_only_loading_ignores_unrelated_config_and_channel_schema() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "[search]\nchannels = 123\njobs = 'obsolete'\n",
        )
        .unwrap();
        fs::write(dir.path().join("channels.toml"), "version = 999\n").unwrap();
        assert!(Config::load_network(config_path).is_ok());
    }

    #[test]
    fn loading_without_channels_keeps_full_config_and_ignores_broken_channel_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_dirs(
            dir.path(),
            dir.path().join("state"),
            dir.path().join("cache"),
        );
        fs::write(&paths.config_file, "[search]\nchannels = ['legacy']\n[providers.gying]\nbase_url = 'https://configured.example'\n[check]\nenabled_cache = false\n").unwrap();
        for input in ["version = 999\n", "this is not TOML"] {
            fs::write(&paths.channels_file, input).unwrap();
            let config = Config::load_without_channels(&paths).unwrap();
            assert_eq!(
                config.providers.gying.base_url,
                "https://configured.example"
            );
            assert!(!config.check.enabled_cache);
            assert_eq!(
                config.search.deprecated_channels,
                Some(vec!["legacy".into()])
            );
            assert!(Config::load(&paths).is_err());
        }
        fs::write(&paths.config_file, "[check]\nenabled_cache = 'invalid'").unwrap();
        assert!(Config::load_without_channels(&paths).is_err());
    }

    #[test]
    fn overrides_are_normalized() {
        let mut config = Config::default();
        config
            .apply_overrides(ConfigOverrides {
                channels: Some(vec![" one ".into(), "one".into(), "two".into()]),
                ..ConfigOverrides::default()
            })
            .unwrap();
        assert_eq!(config.search.channels, ["one", "two"]);
    }
}
