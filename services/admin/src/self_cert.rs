//! This install's own HTTPS certificate, used until a host name with a real certificate is set
//! ([ADR 0007](../../../docs/adr/0007-first-run-and-admin-sign-in.md)).
//!
//! The admin makes it once, on the volume it shares with Caddy for secrets, and never replaces it:
//! clients paired while it is in use pin its SHA-256, so a new one would mean pairing every client
//! again. Caddy reads both files; nothing else does.

use sha2::{Digest, Sha256};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub const CERT_FILE: &str = "self.crt";
pub const KEY_FILE: &str = "self.key";
/// The name in the certificate. Caddy hands the certificate out for this name when a client sends
/// no name (an address) or one it has no certificate for (`default_sni`, `fallback_sni`).
pub const NAME: &str = "askbar.internal";

/// Make the certificate and key in `dir` unless both are there. Returns the certificate's SHA-256.
pub fn ensure(dir: &Path) -> std::io::Result<String> {
    if let Some(fp) = fingerprint(dir) {
        if dir.join(KEY_FILE).exists() {
            return Ok(fp);
        }
    }
    let made = rcgen::generate_simple_self_signed(vec![NAME.to_string(), "localhost".to_string()]).map_err(std::io::Error::other)?;
    std::fs::create_dir_all(dir)?;
    write_new(dir, KEY_FILE, made.signing_key.serialize_pem().as_bytes(), 0o600)?;
    write_new(dir, CERT_FILE, made.cert.pem().as_bytes(), 0o644)?;
    Ok(hex(&Sha256::digest(made.cert.der())))
}

/// The SHA-256 (lowercase hex) of the certificate in `dir`, if there is a readable one.
pub fn fingerprint(dir: &Path) -> Option<String> {
    let pem = std::fs::read_to_string(dir.join(CERT_FILE)).ok()?;
    Some(hex(&Sha256::digest(der_of(&pem)?)))
}

/// The DER bytes of the first certificate in a PEM text.
fn der_of(pem: &str) -> Option<Vec<u8>> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let body = pem.split("-----BEGIN CERTIFICATE-----").nth(1)?.split("-----END CERTIFICATE-----").next()?;
    let b64: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    STANDARD.decode(b64).ok().filter(|d| !d.is_empty())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Write `name` atomically with `mode`.
fn write_new(dir: &Path, name: &str, data: &[u8], mode: u32) -> std::io::Result<()> {
    let tmp = dir.join(format!(".{name}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    std::fs::rename(&tmp, dir.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("askbar-self-cert-{}", ulid::Ulid::new()))
    }

    #[test]
    fn the_certificate_is_made_once_and_kept() {
        let d = dir();
        let first = ensure(&d).unwrap();
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        let cert = std::fs::read(d.join(CERT_FILE)).unwrap();
        let key = std::fs::read(d.join(KEY_FILE)).unwrap();
        assert_eq!(ensure(&d).unwrap(), first, "a second start keeps the same certificate");
        assert_eq!(std::fs::read(d.join(CERT_FILE)).unwrap(), cert);
        assert_eq!(std::fs::read(d.join(KEY_FILE)).unwrap(), key);
        assert_eq!(fingerprint(&d).as_deref(), Some(first.as_str()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_key_is_private_and_the_certificate_readable() {
        let d = dir();
        ensure(&d).unwrap();
        let mode = |f: &str| std::fs::metadata(d.join(f)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(KEY_FILE), 0o600);
        assert_eq!(mode(CERT_FILE), 0o644);
        assert!(std::fs::read_to_string(d.join(KEY_FILE)).unwrap().contains("PRIVATE KEY"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_fingerprint_is_the_sha256_of_the_certificate_der() {
        let d = dir();
        let fp = ensure(&d).unwrap();
        let der = der_of(&std::fs::read_to_string(d.join(CERT_FILE)).unwrap()).unwrap();
        assert_eq!(fp, hex(&Sha256::digest(&der)));
        assert_eq!(der[0], 0x30, "DER starts with a SEQUENCE");
        assert_eq!(fingerprint(&dir()), None, "no file, no fingerprint");
        let _ = std::fs::remove_dir_all(&d);
    }
}
