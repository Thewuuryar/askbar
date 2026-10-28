//! Encryption of provider API keys at rest (ChaCha20-Poly1305).
//!
//! The cipher key is derived from `ASKBAR_MASTER_KEY` with HKDF-SHA256.
//! Stored form: 12-byte random nonce followed by ciphertext and tag.

use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;

pub const MIN_MASTER_KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const HKDF_INFO: &[u8] = b"askbar provider key encryption v1";

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum SecretsError {
    #[error("ASKBAR_MASTER_KEY is not set")]
    Missing,
    #[error("ASKBAR_MASTER_KEY must be at least 32 characters")]
    TooShort,
    #[error("stored secret could not be decrypted (wrong master key or corrupted data)")]
    Decrypt,
}

pub struct Secrets {
    cipher: ChaCha20Poly1305,
}

impl Secrets {
    pub fn new(master_key: &str) -> Result<Secrets, SecretsError> {
        if master_key.chars().count() < MIN_MASTER_KEY_LEN {
            return Err(SecretsError::TooShort);
        }
        let mut key = [0u8; 32];
        Hkdf::<Sha256>::new(None, master_key.as_bytes())
            .expand(HKDF_INFO, &mut key)
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        Ok(Secrets { cipher: ChaCha20Poly1305::new(Key::from_slice(&key)) })
    }

    /// Read `ASKBAR_MASTER_KEY`; the server refuses to start without a valid one.
    pub fn from_env() -> Result<Secrets, SecretsError> {
        match std::env::var("ASKBAR_MASTER_KEY") {
            Ok(v) => Secrets::new(&v),
            Err(_) => Err(SecretsError::Missing),
        }
    }

    pub fn encrypt(&self, plaintext: &str) -> Vec<u8> {
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let mut out = nonce.to_vec();
        out.extend(self.cipher.encrypt(&nonce, plaintext.as_bytes()).expect("encryption cannot fail for in-memory input"));
        out
    }

    pub fn decrypt(&self, stored: &[u8]) -> Result<String, SecretsError> {
        if stored.len() < NONCE_LEN {
            return Err(SecretsError::Decrypt);
        }
        let (nonce, ct) = stored.split_at(NONCE_LEN);
        let plain = self.cipher.decrypt(Nonce::from_slice(nonce), ct).map_err(|_| SecretsError::Decrypt)?;
        String::from_utf8(plain).map_err(|_| SecretsError::Decrypt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn round_trip() {
        let s = Secrets::new(KEY).unwrap();
        let ct = s.encrypt("sk-test-123");
        assert!(!ct.windows(11).any(|w| w == b"sk-test-123"));
        assert_eq!(s.decrypt(&ct).unwrap(), "sk-test-123");
        assert_ne!(s.encrypt("sk-test-123"), ct, "fresh nonce each time");
        assert_eq!(s.decrypt(&s.encrypt("")).unwrap(), "");
    }

    #[test]
    fn wrong_key_fails() {
        let ct = Secrets::new(KEY).unwrap().encrypt("secret");
        let other = Secrets::new("ffffffffffffffffffffffffffffffff").unwrap();
        assert_eq!(other.decrypt(&ct), Err(SecretsError::Decrypt));
    }

    #[test]
    fn tampering_fails() {
        let s = Secrets::new(KEY).unwrap();
        let ct = s.encrypt("secret");
        for i in 0..ct.len() {
            let mut bad = ct.clone();
            bad[i] ^= 1;
            assert_eq!(s.decrypt(&bad), Err(SecretsError::Decrypt), "byte {i}");
        }
        assert_eq!(s.decrypt(&ct[..ct.len() - 1]), Err(SecretsError::Decrypt));
        assert_eq!(s.decrypt(&ct[..5]), Err(SecretsError::Decrypt));
    }

    #[test]
    fn short_key_rejected() {
        assert!(matches!(Secrets::new("too short"), Err(SecretsError::TooShort)));
        assert!(Secrets::new(&"x".repeat(31)).is_err());
        assert!(Secrets::new(&"x".repeat(32)).is_ok());
    }
}
