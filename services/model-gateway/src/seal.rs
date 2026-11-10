//! Sealing provider errors to the audit public key (ADR 0011, ADR 0054).
//!
//! A sealed box (NaCl `crypto_box_seal`) lets anyone holding the public key encrypt something that only the
//! holder of the private key can read. The gateway has the public key only, so this module can seal and
//! nothing more: there is deliberately no function here that opens a sealed box.

use crypto_box::{aead::OsRng, PublicKey};
use sha2::{Digest, Sha256};

/// A sealed payload: the id of the key and the ciphertext.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub key_id: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SealError {
    #[error("the audit public key is not 32 bytes")]
    BadKey,
    #[error("sealing failed")]
    Seal,
}

/// The id of a public key: the first 8 bytes of its SHA-256, in hex (16 characters).
pub fn key_id(public_key: &[u8]) -> String {
    Sha256::digest(public_key)[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Seals `plaintext` to `public_key` (32 bytes). Every call uses a fresh ephemeral key.
pub fn seal(public_key: &[u8], plaintext: &[u8]) -> Result<Sealed, SealError> {
    let key: [u8; 32] = public_key.try_into().map_err(|_| SealError::BadKey)?;
    let bytes = PublicKey::from(key).seal(&mut OsRng, plaintext).map_err(|_| SealError::Seal)?;
    Ok(Sealed { key_id: key_id(&key), bytes })
}

/// The 32 bytes of a key given as 64 hex characters, as a configuration carries it.
pub fn parse_hex_key(hex: &str) -> Option<[u8; 32]> {
    let hex = hex.trim();
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto_box::SecretKey;

    #[test]
    fn a_sealed_payload_opens_only_with_the_matching_private_key() {
        let secret = SecretKey::from([1u8; 32]);
        let public = *secret.public_key().as_bytes();
        let sealed = seal(&public, b"the provider said no").unwrap();
        assert_eq!(sealed.key_id, key_id(&public));
        assert_eq!(secret.unseal(&sealed.bytes).unwrap(), b"the provider said no");
        assert!(SecretKey::from([2u8; 32]).unseal(&sealed.bytes).is_err());
        assert_ne!(sealed.bytes, seal(&public, b"the provider said no").unwrap().bytes, "a fresh key each time");
    }

    #[test]
    fn a_bad_key_is_refused_and_hex_keys_parse_exactly() {
        assert_eq!(seal(&[0u8; 31], b"x").err(), Some(SealError::BadKey));
        let hex: String = (0..32).map(|i| format!("{i:02x}")).collect();
        assert_eq!(parse_hex_key(&hex).unwrap()[31], 31);
        assert!(parse_hex_key(&hex[..62]).is_none());
        assert!(parse_hex_key(&format!("{}zz", &hex[..62])).is_none());
    }
}
