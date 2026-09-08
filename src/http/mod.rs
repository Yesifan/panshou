//! Shared HTTP infrastructure.

mod client;
mod encoding;
mod proxy;
mod session;

pub use client::{ClientOptions, HttpClientFactory, HttpError, RedirectPolicy};
pub use encoding::{DecodeError, decode_body, response_text};
pub use proxy::ProxyUrl;
pub use session::Session;
