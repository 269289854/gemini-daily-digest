//! AES-256-GCM `v1:` envelope encryption (`APP_MASTER_KEY_B64`) and
//! deterministic secret masking (`mask_secret`).

use std::fmt;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use rand::rngs::OsRng;
use rand::RngCore;

use crate::constants::{
    CRYPTO_ENVELOPE_PREFIX, CRYPTO_KEY_BYTE_LEN, CRYPTO_NONCE_BYTE_LEN,
    SECRET_MASK_MIN_PARTIAL_LEN, SECRET_MASK_REDACTED_PLACEHOLDER, SECRET_MASK_VISIBLE_PREFIX_LEN,
    SECRET_MASK_VISIBLE_SUFFIX_LEN,
};
use crate::error::CoreError;

/// Decodes a Base64-encoded 256-bit master key for AES-256-GCM envelope encryption.
pub fn parse_master_key_b64(b64: &str) -> Result<[u8; CRYPTO_KEY_BYTE_LEN], CoreError> {
    let trimmed = b64.trim();
    if trimmed.is_empty() {
        return Err(CoreError::Crypto("master key base64 is empty".to_owned()));
    }
    let decoded = STANDARD
        .decode(trimmed)
        .map_err(|err| CoreError::Crypto(format!("invalid master key base64: {err}")))?;
    decoded.try_into().map_err(|bytes: Vec<u8>| {
        CoreError::Crypto(format!(
            "invalid master key length: expected {CRYPTO_KEY_BYTE_LEN} bytes, got {}",
            bytes.len()
        ))
    })
}

/// Authenticated AES-256-GCM cipher producing and verifying `v1:` envelopes.
#[derive(Clone)]
pub struct EnvelopeCipher {
    cipher: Aes256Gcm,
}

impl fmt::Debug for EnvelopeCipher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnvelopeCipher")
            .field("cipher", &SECRET_MASK_REDACTED_PLACEHOLDER)
            .finish()
    }
}

impl EnvelopeCipher {
    /// Constructs an `EnvelopeCipher` from a Base64-encoded 32-byte key.
    pub fn from_base64(b64: &str) -> Result<Self, CoreError> {
        let key = parse_master_key_b64(b64)?;
        Ok(Self::from_bytes(key))
    }

    /// Constructs an `EnvelopeCipher` from raw 32-byte key material.
    #[must_use]
    pub fn from_bytes(key: [u8; CRYPTO_KEY_BYTE_LEN]) -> Self {
        Self {
            cipher: Aes256Gcm::new(&key.into()),
        }
    }

    /// Encrypts `plaintext` with a fresh random 12-byte nonce into a `v1:<b64>` envelope.
    pub fn encrypt(&self, plaintext: &str) -> Result<String, CoreError> {
        let mut nonce_bytes = [0u8; CRYPTO_NONCE_BYTE_LEN];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from(nonce_bytes);

        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|err| CoreError::Crypto(format!("AEAD encryption failed: {err}")))?;

        let mut payload = Vec::with_capacity(CRYPTO_NONCE_BYTE_LEN + ciphertext.len());
        payload.extend_from_slice(&nonce_bytes);
        payload.extend_from_slice(&ciphertext);

        let encoded = STANDARD.encode(&payload);
        Ok(format!("{CRYPTO_ENVELOPE_PREFIX}{encoded}"))
    }

    /// Verifies and decrypts a `v1:<b64>` envelope back to UTF-8 plaintext.
    pub fn decrypt(&self, envelope: &str) -> Result<String, CoreError> {
        let encoded = envelope
            .trim()
            .strip_prefix(CRYPTO_ENVELOPE_PREFIX)
            .ok_or_else(|| {
                CoreError::Crypto(format!(
                    "envelope missing required '{CRYPTO_ENVELOPE_PREFIX}' prefix"
                ))
            })?;

        let raw = STANDARD
            .decode(encoded)
            .map_err(|err| CoreError::Crypto(format!("invalid envelope base64: {err}")))?;

        if raw.len() <= CRYPTO_NONCE_BYTE_LEN {
            return Err(CoreError::Crypto(
                "envelope payload shorter than nonce and authentication tag".to_owned(),
            ));
        }

        let (nonce_slice, ciphertext) = raw.split_at(CRYPTO_NONCE_BYTE_LEN);
        let nonce_bytes: [u8; CRYPTO_NONCE_BYTE_LEN] = nonce_slice
            .try_into()
            .map_err(|_| CoreError::Crypto("invalid nonce length".to_owned()))?;
        let nonce = Nonce::from(nonce_bytes);

        let plaintext = self
            .cipher
            .decrypt(&nonce, ciphertext)
            .map_err(|_| CoreError::Crypto("envelope authentication failed".to_owned()))?;

        String::from_utf8(plaintext)
            .map_err(|err| CoreError::Crypto(format!("decrypted plaintext is not UTF-8: {err}")))
    }
}

/// Deterministically masks a secret without exposing its full value.
#[must_use]
pub fn mask_secret(secret: &str) -> String {
    let trimmed = secret.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let char_count = trimmed.chars().count();
    if char_count < SECRET_MASK_MIN_PARTIAL_LEN {
        return SECRET_MASK_REDACTED_PLACEHOLDER.to_owned();
    }

    let prefix: String = trimmed
        .chars()
        .take(SECRET_MASK_VISIBLE_PREFIX_LEN)
        .collect();
    let suffix: String = trimmed
        .chars()
        .skip(char_count.saturating_sub(SECRET_MASK_VISIBLE_SUFFIX_LEN))
        .collect();
    format!("{prefix}{SECRET_MASK_REDACTED_PLACEHOLDER}{suffix}")
}

/// Deterministically masks an optional secret, returning `None` for empty inputs.
#[must_use]
pub fn mask_optional_secret(secret: Option<&str>) -> Option<String> {
    secret.map(mask_secret).filter(|masked| !masked.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_key_b64() -> String {
        STANDARD.encode([0x42u8; CRYPTO_KEY_BYTE_LEN])
    }

    #[test]
    fn parse_master_key_accepts_valid_32_byte_base64_and_rejects_invalid() {
        let key = parse_master_key_b64(&sample_key_b64()).unwrap();
        assert_eq!(key, [0x42u8; CRYPTO_KEY_BYTE_LEN]);

        assert!(matches!(
            parse_master_key_b64(""),
            Err(CoreError::Crypto(_))
        ));
        assert!(matches!(
            parse_master_key_b64("not-valid-base64!!!"),
            Err(CoreError::Crypto(_))
        ));
        let short_b64 = STANDARD.encode([0x11u8; 16]);
        assert!(matches!(
            parse_master_key_b64(&short_b64),
            Err(CoreError::Crypto(_))
        ));
    }

    #[test]
    fn envelope_round_trip_and_distinct_random_nonces() {
        let cipher = EnvelopeCipher::from_base64(&sample_key_b64()).unwrap();
        let plaintext = "test-key-placeholder";

        let env1 = cipher.encrypt(plaintext).unwrap();
        let env2 = cipher.encrypt(plaintext).unwrap();

        assert!(env1.starts_with(CRYPTO_ENVELOPE_PREFIX));
        assert!(env2.starts_with(CRYPTO_ENVELOPE_PREFIX));
        assert_ne!(env1, env2);

        assert_eq!(cipher.decrypt(&env1).unwrap(), plaintext);
        assert_eq!(cipher.decrypt(&env2).unwrap(), plaintext);
    }

    #[test]
    fn envelope_decrypt_rejects_wrong_key_tampering_and_missing_prefix() {
        let cipher = EnvelopeCipher::from_bytes([0x24u8; CRYPTO_KEY_BYTE_LEN]);
        let wrong_cipher = EnvelopeCipher::from_bytes([0x99u8; CRYPTO_KEY_BYTE_LEN]);

        let envelope = cipher.encrypt("secret-payload").unwrap();
        assert!(matches!(
            wrong_cipher.decrypt(&envelope),
            Err(CoreError::Crypto(_))
        ));

        let without_prefix = envelope.strip_prefix(CRYPTO_ENVELOPE_PREFIX).unwrap();
        assert!(matches!(
            cipher.decrypt(without_prefix),
            Err(CoreError::Crypto(_))
        ));

        let mut raw = STANDARD.decode(without_prefix).unwrap();
        let last_idx = raw.len() - 1;
        raw[last_idx] ^= 0xFF;
        let tampered = format!("{CRYPTO_ENVELOPE_PREFIX}{}", STANDARD.encode(raw));
        assert!(matches!(
            cipher.decrypt(&tampered),
            Err(CoreError::Crypto(_))
        ));

        let truncated = format!("{CRYPTO_ENVELOPE_PREFIX}{}", STANDARD.encode([0u8; 8]));
        assert!(matches!(
            cipher.decrypt(&truncated),
            Err(CoreError::Crypto(_))
        ));
    }

    #[test]
    fn mask_secret_handles_empty_short_and_normal_values() {
        assert_eq!(mask_secret(""), "");
        assert_eq!(mask_secret("   "), "");
        assert_eq!(mask_secret("a"), SECRET_MASK_REDACTED_PLACEHOLDER);
        assert_eq!(mask_secret("abcd"), SECRET_MASK_REDACTED_PLACEHOLDER);
        assert_eq!(mask_secret("123456789"), SECRET_MASK_REDACTED_PLACEHOLDER);
        assert_eq!(mask_secret("test-key-placeholder"), "test****lder");
        assert_eq!(mask_optional_secret(None), None);
        assert_eq!(mask_optional_secret(Some("")), None);
        assert_eq!(
            mask_optional_secret(Some("test-key-placeholder")).as_deref(),
            Some("test****lder")
        );
    }

    #[test]
    fn envelope_cipher_debug_redacts_internal_state() {
        let cipher = EnvelopeCipher::from_bytes([0x11u8; CRYPTO_KEY_BYTE_LEN]);
        let debug = format!("{cipher:?}");
        assert!(debug.contains(SECRET_MASK_REDACTED_PLACEHOLDER));
    }
}
