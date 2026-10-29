//! Encryption of provider API keys at rest (ChaCha20-Poly1305).
//!
//! The master key lives in a file on its own volume (see `key_path`), never in the
//! database, an env file or the repo. The cipher key is derived from it with HKDF-SHA256.
//! Stored form: 12-byte random nonce followed by ciphertext and tag.

use chacha20poly1305::aead::{rand_core::RngCore, Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;

pub const MIN_MASTER_KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const HKDF_INFO: &[u8] = b"askbar provider key encryption v1";

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum SecretsError {
    #[error("the master key must be at least 32 characters")]
    TooShort,
    #[error("master key file {path}: {message}")]
    File { path: String, message: String },
    #[error("stored secret could not be decrypted (wrong master key or corrupted data)")]
    Decrypt,
}

/// Where the master key file is: `ASKBAR_MASTER_KEY_FILE`, else `master.key` inside
/// `ASKBAR_SECRETS_DIR` (default `secrets`; `/secrets` in the container image).
pub fn key_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("ASKBAR_MASTER_KEY_FILE") {
        if !p.trim().is_empty() {
            return p.into();
        }
    }
    let dir = std::env::var("ASKBAR_SECRETS_DIR").unwrap_or_else(|_| "secrets".into());
    std::path::Path::new(&dir).join("master.key")
}

fn file_err(path: &std::path::Path, e: impl std::fmt::Display) -> SecretsError {
    SecretsError::File { path: path.display().to_string(), message: e.to_string() }
}

/// `Ok(None)` when there is no key file yet (the server then runs in setup mode).
pub fn load_key_file(path: &std::path::Path) -> Result<Option<Secrets>, SecretsError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Secrets::new(text.trim()).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(file_err(path, e)),
    }
}

/// Create a fresh random master key at `path` (mode 0600) and return the secrets built from it.
/// Refuses to overwrite an existing file.
pub fn generate_key_file(path: &std::path::Path) -> Result<Secrets, SecretsError> {
    generate_key(path).map(|(s, _)| s)
}

/// Like `generate_key_file`, but also returns the key text so setup can show it once.
pub fn generate_key(path: &std::path::Path) -> Result<(Secrets, String), SecretsError> {
    let mut raw = [0u8; 32];
    OsRng.fill_bytes(&mut raw);
    let key = base64_url(&raw);
    write_key_file(path, &key).map(|s| (s, key))
}

/// Store a master key the user already has (for example from a password manager, when
/// restoring onto a fresh secrets volume). Refuses to overwrite an existing file.
pub fn write_key_file(path: &std::path::Path, key: &str) -> Result<Secrets, SecretsError> {
    use std::io::Write;
    let secrets = Secrets::new(key.trim())?;
    let key = key.trim();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| file_err(path, e))?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(path).map_err(|e| file_err(path, e))?;
    f.write_all(key.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| file_err(path, e))?;
    Ok(secrets)
}

fn base64_url(bytes: &[u8]) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    URL_SAFE_NO_PAD.encode(bytes)
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
    fn key_file_lifecycle() {
        let dir = std::env::temp_dir().join(format!("askbar-keyfile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("master.key");
        assert!(load_key_file(&path).unwrap().is_none(), "no file means setup mode");

        let made = generate_key_file(&path).unwrap();
        let loaded = load_key_file(&path).unwrap().expect("key is there now");
        assert_eq!(loaded.decrypt(&made.encrypt("hello")).unwrap(), "hello", "same key after reload");
        assert!(matches!(generate_key_file(&path), Err(SecretsError::File { .. })), "never overwrites");
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions()) & 0o777, 0o600);

        std::fs::write(&path, "too short").unwrap();
        assert!(matches!(load_key_file(&path), Err(SecretsError::TooShort)));
        std::fs::write(&path, format!("{}\n", KEY)).unwrap();
        assert!(load_key_file(&path).unwrap().is_some(), "trailing newline is fine");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn short_key_rejected() {
        assert!(matches!(Secrets::new("too short"), Err(SecretsError::TooShort)));
        assert!(Secrets::new(&"x".repeat(31)).is_err());
        assert!(Secrets::new(&"x".repeat(32)).is_ok());
    }
}
