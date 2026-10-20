//! Shared HTTP infrastructure.

mod body;
mod client;
mod encoding;
mod outbound;
mod proxy;
pub mod retry;
mod session;
mod stream;

pub use body::{
    DEFAULT_RESPONSE_LIMIT, MAX_DECOMPRESSED_BYTES, ResponseTooLarge, decompression_limit,
    read_limited, read_response,
};
pub use client::{ClientOptions, HttpClientFactory, HttpError, RedirectPolicy};
pub use encoding::{DecodeError, decode_body, response_text};
pub use outbound::{DEFAULT_OUTBOUND_CONCURRENCY, execute};
pub use proxy::{ProxyUrl, compatibility_proxy, no_proxy};
pub use session::Session;
pub use stream::stream_response;
