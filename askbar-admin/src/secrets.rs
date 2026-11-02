//! Encryption at rest for the one secret the admin keeps: the Cloudflare token.
//!
//! The key is a file in the admin's own secrets directory (mode 0600), made on first start
//! and never overwritten. It is unrelated to the server's master key: the server's
//! provider keys stay unreadable to the admin, and the admin's token to the server.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chacha20poly1305::aead::{rand_core::RngCore, Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use std::path::Path;

pub const KEY_FILE: &str = "admin.key";
const NONCE_LEN: usize = 12;

pub struct AdminSecrets {
    cipher: ChaCha20Poly1305,
}

impl AdminSecrets {
    fn from_bytes(raw: &[u8; 32]) -> AdminSecrets {
        AdminSecrets { cipher: ChaCha20Poly1305::new(Key::from_slice(raw)) }
    }

    /// A fresh key that is never written anywhere. For tests.
    pub fn ephemeral() -> AdminSecrets {
        let mut raw = [0u8; 32];
        OsRng.fill_bytes(&mut raw);
        AdminSecrets::from_bytes(&raw)
    }

    /// Load `admin.key` from `dir`, creating it (0600) on the first start.
    pub fn load_or_create(dir: &Path) -> std::io::Result<AdminSecrets> {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        let path = dir.join(KEY_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut raw = [0u8; 32];
                OsRng.fill_bytes(&mut raw);
                let text = URL_SAFE_NO_PAD.encode(raw);
                let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
                f.write_all(text.as_bytes())?;
                f.sync_all()?;
                text
            }
            Err(e) => return Err(e),
        };
        let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{} is not a valid key", path.display()));
        let raw: [u8; 32] = URL_SAFE_NO_PAD.decode(text.trim()).map_err(|_| bad())?.try_into().map_err(|_| bad())?;
        Ok(AdminSecrets::from_bytes(&raw))
    }

    /// Nonce followed by ciphertext and tag.
    pub fn encrypt(&self, plaintext: &str) -> Vec<u8> {
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let mut out = nonce.to_vec();
        out.extend(self.cipher.encrypt(&nonce, plaintext.as_bytes()).expect("encryption cannot fail for in-memory input"));
        out
    }

    pub fn decrypt(&self, blob: &[u8]) -> Option<String> {
        if blob.len() < NONCE_LEN + 16 {
            return None;
        }
        let (nonce, ct) = blob.split_at(NONCE_LEN);
        String::from_utf8(self.cipher.decrypt(Nonce::from_slice(nonce), ct).ok()?).ok()
    }
}
