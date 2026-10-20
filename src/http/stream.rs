//! Incremental, cancellable response decoding with independent wire/stage limits.
use std::{io, io::Cursor, pin::Pin};

use anyhow::{Context, bail};
use async_compression::tokio::bufread::{BrotliDecoder, GzipDecoder, ZlibDecoder};
use futures::{
    StreamExt, TryStreamExt,
    stream::{self, BoxStream},
};
use reqwest::{Response, header::CONTENT_ENCODING};
use tokio::io::{AsyncRead, AsyncReadExt, BufReader};
use tokio_util::io::StreamReader;

use super::body::{DEFAULT_RESPONSE_LIMIT, ResponseTooLarge, decompression_limit};

const PIECE: usize = 32 * 1024;
type Reader = Pin<Box<dyn AsyncRead + Send>>;

/// Decode on the caller's task, yielding between bounded pieces. Dropping the
/// stream drops the original response and its outbound admission permit.
/// Zero selects the default wire limit. Smaller source limits also cap every
/// decoded stage; ordinary responses use the shared decompression/ratio cap.
pub fn stream_response(
    response: Response,
    limit: usize,
) -> anyhow::Result<BoxStream<'static, anyhow::Result<Vec<u8>>>> {
    let limit = if limit == 0 {
        DEFAULT_RESPONSE_LIMIT
    } else {
        limit
    };
    let known_length = response.content_length();
    if known_length.is_some_and(|length| length > limit as u64) {
        return Err(ResponseTooLarge { limit }.into());
    }
    let mut codings = Vec::new();
    for header in response.headers().get_all(CONTENT_ENCODING) {
        for coding in header
            .to_str()
            .context("invalid response content encoding")?
            .split(',')
        {
            let coding = coding.trim().to_ascii_lowercase();
            if !matches!(
                coding.as_str(),
                "" | "identity" | "gzip" | "x-gzip" | "deflate" | "br"
            ) {
                bail!("unsupported HTTP content encoding");
            }
            if !matches!(coding.as_str(), "" | "identity") {
                // Nested AsyncRead decoders recurse while polling. Bound this
                // untrusted header's depth as well as every stage's bytes.
                if codings.len() == 8 {
                    bail!("too many HTTP content encoding layers");
                }
                codings.push(coding);
            }
        }
    }
    let mut decoded_limit = decompression_limit(known_length);
    if limit < DEFAULT_RESPONSE_LIMIT {
        decoded_limit = decoded_limit.min(limit);
    }

    // Do not use Response::bytes_stream: the response extensions own the shared
    // outbound permit. Keep the complete Response in this stream's state.
    let raw = stream::try_unfold(
        (response, None, 0usize, false),
        move |(mut response, mut pending, count, mut overflow)| async move {
            tokio::task::yield_now().await;
            if overflow {
                return Err(io::Error::other(ResponseTooLarge { limit }));
            }
            if pending.is_none() {
                pending = response
                    .chunk()
                    .await
                    .map_err(|error| io::Error::other(error.without_url()))?;
            }
            let Some(chunk) = pending.as_mut() else {
                return Ok(None);
            };
            let remaining = limit - count;
            if !chunk.is_empty() && remaining == 0 {
                return Err(io::Error::other(ResponseTooLarge { limit }));
            }
            // Bound input scheduling too: a decoder consuming empty members or
            // large headers must yield even when it produces no output.
            let accepted = chunk.len().min(remaining).min(PIECE);
            let piece = chunk.split_to(accepted);
            let count = count + accepted;
            if chunk.is_empty() {
                pending = None;
            } else if count == limit {
                // Observe overflow, but first deliver the accepted prefix so
                // completed events are not lost with the oversized chunk.
                pending = None;
                overflow = true;
            }
            Ok(Some((piece, (response, pending, count, overflow))))
        },
    )
    .boxed();
    let mut reader: Reader = Box::pin(StreamReader::new(raw));
    for coding in codings.into_iter().rev() {
        // Each decoder has its own bounded output, including intermediate
        // encodings: a small final output cannot hide an oversized inner stage.
        reader = match coding.as_str() {
            "" | "identity" => continue,
            "gzip" | "x-gzip" => {
                let mut decoder = GzipDecoder::new(BufReader::with_capacity(PIECE, reader));
                decoder.multiple_members(true);
                Box::pin(decoder)
            }
            "deflate" => {
                // HTTP deflate denotes the zlib wrapper, not raw DEFLATE.
                let mut decoder = ZlibDecoder::new(BufReader::with_capacity(PIECE, reader));
                decoder.multiple_members(true);
                Box::pin(decoder)
            }
            "br" => {
                let mut decoder = BrotliDecoder::new(BufReader::with_capacity(PIECE, reader));
                decoder.multiple_members(true);
                Box::pin(decoder)
            }
            _ => unreachable!("validated content coding"),
        };
        reader = Box::pin(StreamReader::new(
            bounded(reader, decoded_limit).map_ok(Cursor::new),
        ));
    }
    // Also cap identity reads and their scheduling pieces. Raw counting remains
    // in the response stream, before any decoder can buffer wire bytes.
    Ok(bounded(reader, limit.max(decoded_limit))
        .map_err(restore_error)
        .boxed())
}

fn bounded(reader: Reader, limit: usize) -> BoxStream<'static, io::Result<Vec<u8>>> {
    stream::try_unfold(
        (reader, 0usize, false),
        move |(mut reader, count, overflow)| async move {
            tokio::task::yield_now().await;
            if overflow {
                return Err(io::Error::other(ResponseTooLarge { limit }));
            }
            let mut bytes = vec![0; PIECE.min((limit - count).saturating_add(1))];
            let length = reader.read(&mut bytes).await?;
            if length == 0 {
                return Ok(None);
            }
            let remaining = limit - count;
            let overflow = length > remaining;
            if overflow && remaining == 0 {
                return Err(io::Error::other(ResponseTooLarge { limit }));
            }
            bytes.truncate(length.min(remaining));
            let count = count + bytes.len();
            Ok(Some((bytes, (reader, count, overflow))))
        },
    )
    .boxed()
}

fn restore_error(error: io::Error) -> anyhow::Error {
    // Preserve shared size-error and reqwest timeout/status downcasts across
    // the AsyncRead adapters, rather than reducing them to formatted strings.
    if let Some(inner) = error.get_ref() {
        if let Some(error) = inner.downcast_ref::<ResponseTooLarge>() {
            return ResponseTooLarge { limit: error.limit }.into();
        }
        if inner.is::<reqwest::Error>() {
            return (*error
                .into_inner()
                .expect("checked inner")
                .downcast::<reqwest::Error>()
                .expect("checked type"))
            .into();
        }
    }
    error.into()
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
