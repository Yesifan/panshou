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
    pub channels: Vec<String>,
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
    pub search_jobs: Option<usize>,
    pub channels: Option<Vec<String>>,
    pub providers: Option<Vec<String>>,
    pub check_jobs: Option<usize>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
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
            channels: vec!["tgsearchers3".into()],
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
    /// Load the platform config file, then apply process environment values.
    pub fn load(paths: &AppPaths) -> Result<Self, ConfigError> {
        Self::load_from(&paths.config_file)
    }

    /// A missing file is equivalent to an empty config file.
    pub fn load_from(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
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
        if self.search.jobs == 0 || self.check.jobs == 0 {
            return Err(ConfigError::Invalid(
                "job counts must be greater than zero".into(),
            ));
        }
        Ok(())
    }

    fn apply_environment(&mut self) -> Result<(), ConfigError> {
        // Compatibility variables are fallback-only. More specific proxy env
        // values precede generic ones within that compatibility tier.
        if let Some(proxy) = first_env(&["PROXY", "HTTPS_PROXY", "HTTP_PROXY"]) {
            self.network.proxy = nonempty(proxy);
        }
        if let Some(channels) = env::var_os("CHANNELS").and_then(|v| v.into_string().ok()) {
            self.search.channels = csv(&channels);
        }
        if let Some(providers) = env::var_os("ENABLED_PLUGINS").and_then(|v| v.into_string().ok()) {
            self.search.providers = csv(&providers);
        }

        if let Ok(proxy) = env::var("PANSOU_PROXY") {
            self.network.proxy = nonempty(proxy);
        }
        if let Some(value) = parse_env::<u64>("PANSOU_TIMEOUT_SECS")? {
            self.network.timeout_secs = value;
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
        assert_eq!(config.search.providers.len(), 15);
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
