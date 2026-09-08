use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
};
use base64::{Engine as _, engine::general_purpose};
use thiserror::Error;

const KEY_BYTES: usize = 32;

#[derive(Clone)]
pub struct StateCipher(Aes256Gcm);

impl std::fmt::Debug for StateCipher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("StateCipher([REDACTED])")
    }
}

#[derive(Debug, Error)]
pub enum StateKeyError {
    #[error(
        "PANSOU_STATE_KEY must be 32 raw bytes, 64 hex characters, or base64 encoding 32 bytes"
    )]
    InvalidLength,
    #[error("state ciphertext is malformed")]
    MalformedCiphertext,
    #[error("state decryption failed (wrong PANSOU_STATE_KEY or corrupt state)")]
    Decryption,
    #[error("state encryption failed")]
    Encryption,
}

impl StateCipher {
    pub fn parse(value: &str) -> Result<Self, StateKeyError> {
        let bytes = decode_key(value)?;
        Ok(Self(
            Aes256Gcm::new_from_slice(&bytes).expect("validated AES-256 key length"),
        ))
    }

    pub fn from_bytes(bytes: [u8; KEY_BYTES]) -> Self {
        Self(Aes256Gcm::new_from_slice(&bytes).expect("AES-256 key length"))
    }

    pub(crate) fn encrypt(
        &self,
        plaintext: &[u8],
        aad: &[u8],
    ) -> Result<(String, String), StateKeyError> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = self
            .0
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| StateKeyError::Encryption)?;
        Ok((
            general_purpose::STANDARD_NO_PAD.encode(nonce),
            general_purpose::STANDARD_NO_PAD.encode(ciphertext),
        ))
    }

    pub(crate) fn decrypt(
        &self,
        nonce: &str,
        ciphertext: &str,
        aad: &[u8],
    ) -> Result<Vec<u8>, StateKeyError> {
        let nonce = general_purpose::STANDARD_NO_PAD
            .decode(nonce)
            .map_err(|_| StateKeyError::MalformedCiphertext)?;
        if nonce.len() != 12 {
            return Err(StateKeyError::MalformedCiphertext);
        }
        let ciphertext = general_purpose::STANDARD_NO_PAD
            .decode(ciphertext)
            .map_err(|_| StateKeyError::MalformedCiphertext)?;
        let nonce = Nonce::from_slice(&nonce);
        self.0
            .decrypt(
                nonce,
                Payload {
                    msg: &ciphertext,
                    aad,
                },
            )
            .map_err(|_| StateKeyError::Decryption)
    }
}

fn decode_key(value: &str) -> Result<[u8; KEY_BYTES], StateKeyError> {
    let value = value.trim();
    let decoded = if value.len() == KEY_BYTES {
        value.as_bytes().to_vec()
    } else if value.len() == KEY_BYTES * 2 && value.bytes().all(|v| v.is_ascii_hexdigit()) {
        value
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                let high = from_hex(pair[0]);
                let low = from_hex(pair[1]);
                (high << 4) | low
            })
            .collect()
    } else {
        general_purpose::STANDARD
            .decode(value)
            .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(value))
            .or_else(|_| general_purpose::URL_SAFE_NO_PAD.decode(value))
            .map_err(|_| StateKeyError::InvalidLength)?
    };
    decoded.try_into().map_err(|_| StateKeyError::InvalidLength)
}

fn from_hex(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        b'A'..=b'F' => value - b'A' + 10,
        _ => unreachable!("validated hex"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_aad_binding() {
        let cipher = StateCipher::from_bytes([7; 32]);
        let (nonce, ciphertext) = cipher.encrypt(b"cookie", b"cookie").unwrap();
        assert_eq!(
            cipher.decrypt(&nonce, &ciphertext, b"cookie").unwrap(),
            b"cookie"
        );
        assert!(cipher.decrypt(&nonce, &ciphertext, b"token").is_err());
    }

    #[test]
    fn accepts_hex_and_base64_keys() {
        assert!(StateCipher::parse(&"ab".repeat(32)).is_ok());
        assert!(StateCipher::parse(&general_purpose::STANDARD.encode([1; 32])).is_ok());
    }
}
