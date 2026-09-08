use aes::Aes128;
use base64::{Engine, engine::general_purpose::STANDARD};
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use rand::RngCore;
use thiserror::Error;

type Encryptor = cbc::Encryptor<Aes128>;
type Decryptor = cbc::Decryptor<Aes128>;
const KEY: &[u8; 16] = b"PVGDwmcvfs1uV3d1";

#[derive(Debug, Error)]
pub enum MobileCryptoError {
    #[error("invalid base64 response: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("invalid encrypted response")]
    InvalidCiphertext,
    #[error("invalid response padding")]
    Padding,
}

pub fn encrypt(plaintext: &[u8]) -> String {
    let mut iv = [0u8; 16];
    rand::rng().fill_bytes(&mut iv);
    let mut buffer = vec![0; plaintext.len() + 16];
    buffer[..plaintext.len()].copy_from_slice(plaintext);
    let encrypted = Encryptor::new_from_slices(KEY, &iv)
        .expect("AES-128 key and IV lengths are fixed")
        .encrypt_padded_mut::<Pkcs7>(&mut buffer, plaintext.len())
        .expect("PKCS#7 output buffer always has one spare block");
    let mut joined = iv.to_vec();
    joined.extend_from_slice(encrypted);
    STANDARD.encode(joined)
}
pub fn decrypt(encoded: &str) -> Result<Vec<u8>, MobileCryptoError> {
    let raw = STANDARD.decode(encoded.trim().trim_matches('"'))?;
    if raw.len() < 32 || (raw.len() - 16) % 16 != 0 {
        return Err(MobileCryptoError::InvalidCiphertext);
    }
    let (iv, body) = raw.split_at(16);
    let mut body = body.to_vec();
    Decryptor::new_from_slices(KEY, iv)
        .map_err(|_| MobileCryptoError::InvalidCiphertext)?
        .decrypt_padded_mut::<Pkcs7>(&mut body)
        .map(|v| v.to_vec())
        .map_err(|_| MobileCryptoError::Padding)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip() {
        let input = br#"{"hello":"world"}"#;
        assert_eq!(decrypt(&encrypt(input)).unwrap(), input);
    }
}
