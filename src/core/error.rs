use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum ProviderError {
    #[error("provider timed out")]
    Timeout,
    #[error("network error: {0}")]
    Network(String),
    #[error("response parse error: {0}")]
    Parse(String),
    #[error("authentication required")]
    AuthRequired,
    #[error("provider rate limited the request")]
    RateLimited,
    #[error("provider blocked the request")]
    Blocked,
    #[error("provider protocol error: {0}")]
    Protocol(String),
    #[error("provider is unavailable: {0}")]
    Unavailable(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum CheckError {
    #[error("unsupported cloud type")]
    Unsupported,
    #[error("check timed out")]
    Timeout,
    #[error("network error: {0}")]
    Network(String),
    #[error("response parse error: {0}")]
    Parse(String),
    #[error("checker protocol error: {0}")]
    Protocol(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum ConfigError {
    #[error("invalid configuration: {0}")]
    Invalid(String),
    #[error("failed to read configuration: {0}")]
    Read(String),
    #[error("failed to write configuration: {0}")]
    Write(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum StateError {
    #[error("state is locked")]
    Locked,
    #[error("state encryption error: {0}")]
    Crypto(String),
    #[error("failed to read state: {0}")]
    Read(String),
    #[error("failed to write state: {0}")]
    Write(String),
    #[error("state data is invalid: {0}")]
    Invalid(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum HttpError {
    #[error("request timed out")]
    Timeout,
    #[error("network error: {0}")]
    Network(String),
    #[error("HTTP status {0}")]
    Status(u16),
    #[error("invalid proxy: {0}")]
    InvalidProxy(String),
    #[error("failed to decode response: {0}")]
    Decode(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum ParseError {
    #[error("unknown cloud type: {0}")]
    UnknownCloudType(String),
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
}
