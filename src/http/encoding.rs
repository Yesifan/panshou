use encoding_rs::{Encoding, UTF_8};
use reqwest::{Response, header::CONTENT_TYPE};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("failed to read HTTP response: {0}")]
    Http(#[from] reqwest::Error),
    #[error("response is not valid {encoding} text")]
    Malformed { encoding: &'static str },
}

/// Decode bytes using a Content-Type charset. UTF-8 is the standards default;
/// providers with a known legacy encoding (for example yulinshufa) can pass GBK.
pub fn decode_body(
    bytes: &[u8],
    content_type: Option<&str>,
    fallback: Option<&'static Encoding>,
) -> Result<String, DecodeError> {
    let encoding = content_type
        .and_then(charset_label)
        .and_then(|label| Encoding::for_label(label.as_bytes()))
        .or(fallback)
        .unwrap_or(UTF_8);
    let (decoded, _, malformed) = encoding.decode(bytes);
    if malformed {
        return Err(DecodeError::Malformed {
            encoding: encoding.name(),
        });
    }
    Ok(decoded.into_owned())
}

pub async fn response_text(
    response: Response,
    fallback: Option<&'static Encoding>,
) -> Result<String, DecodeError> {
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = response.bytes().await?;
    decode_body(&bytes, content_type.as_deref(), fallback)
}

fn charset_label(content_type: &str) -> Option<&str> {
    content_type.split(';').skip(1).find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches(['\'', '"']))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use encoding_rs::GBK;

    #[test]
    fn decodes_gbk_fallback() {
        let (bytes, _, _) = GBK.encode("仙逆");
        assert_eq!(decode_body(&bytes, None, Some(GBK)).unwrap(), "仙逆");
    }

    #[test]
    fn header_wins_over_fallback() {
        assert_eq!(
            decode_body(
                "hello".as_bytes(),
                Some("text/plain; charset=utf-8"),
                Some(GBK)
            )
            .unwrap(),
            "hello"
        );
    }
}
