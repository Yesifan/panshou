use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::CloudType;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Source {
    Telegram { channel: String },
    Provider { name: String },
}

impl Source {
    pub fn telegram(channel: impl Into<String>) -> Self {
        Self::Telegram {
            channel: channel.into(),
        }
    }

    pub fn provider(name: impl Into<String>) -> Self {
        Self::Provider { name: name.into() }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Telegram { channel } => channel,
            Self::Provider { name } => name,
        }
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Telegram { channel } => write!(f, "tg:{channel}"),
            Self::Provider { name } => write!(f, "provider:{name}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub cloud_type: CloudType,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_title: Option<String>,
}

impl Link {
    pub fn new(url: impl Into<String>) -> Self {
        let url = url.into();
        Self {
            cloud_type: super::cloud::CloudType::Others,
            url,
            password: None,
            datetime: None,
            work_title: None,
        }
        .detect_type()
    }

    pub fn detect_type(mut self) -> Self {
        self.cloud_type = super::link::detect_cloud_type(&self.url);
        self
    }

    pub fn with_password(mut self, password: impl Into<String>) -> Self {
        self.password = Some(password.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: String,
    pub source: Source,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<DateTime<Utc>>,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub links: Vec<Link>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}

impl SearchResult {
    pub fn new(id: impl Into<String>, source: Source) -> Self {
        Self {
            id: id.into(),
            source,
            datetime: None,
            title: String::new(),
            content: String::new(),
            links: Vec::new(),
            tags: Vec::new(),
            images: Vec::new(),
        }
    }

    pub fn provider(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self::new(id, Source::provider(name))
    }

    pub fn telegram(id: impl Into<String>, channel: impl Into<String>) -> Self {
        Self::new(id, Source::telegram(channel))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckState {
    Ok,
    Bad,
    Locked,
    Unsupported,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub state: CheckState,
    #[serde(default)]
    pub cache_hit: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergedLink {
    pub cloud_type: CloudType,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<DateTime<Utc>>,
    pub source: Source,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<CheckResult>,
}
