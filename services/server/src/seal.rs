//! Sealing audit content to the administrator's public key.
//!
//! A sealed box (NaCl `crypto_box_seal`) lets anyone holding the public key encrypt something
//! that only the holder of the matching private key can read, not even the sender afterwards.
//! The private key is derived from the administrator's passphrase and exists nowhere else, so
//! the server, which only ever has the public key, can write audit content but never read it
//! back. This module can seal and nothing more: there is deliberately no function here that
//! opens a sealed box.

use crypto_box::{aead::OsRng, PublicKey};
use sha2::{Digest, Sha256};

pub use askbar_audit::Sealed;

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

/// Seals `plaintext` to `public_key` (32 bytes). Every call uses a fresh ephemeral key, so the
/// same plaintext gives different bytes each time.
pub fn seal(public_key: &[u8], plaintext: &[u8]) -> Result<Sealed, SealError> {
    let key: [u8; 32] = public_key.try_into().map_err(|_| SealError::BadKey)?;
    let bytes = PublicKey::from(key).seal(&mut OsRng, plaintext).map_err(|_| SealError::Seal)?;
    Ok(Sealed { key_id: key_id(&key), bytes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto_box::SecretKey;

    fn keypair(seed: u8) -> (SecretKey, [u8; 32]) {
        let secret = SecretKey::from([seed; 32]);
        let public = *secret.public_key().as_bytes();
        (secret, public)
    }

    #[test]
    fn a_sealed_payload_opens_with_the_matching_private_key() {
        let (secret, public) = keypair(1);
        let sealed = seal(&public, b"the prompt and the reply").unwrap();
        assert_eq!(sealed.key_id, key_id(&public));
        assert_eq!(sealed.key_id.len(), 16);
        assert_eq!(secret.unseal(&sealed.bytes).unwrap(), b"the prompt and the reply");
        assert!(!String::from_utf8_lossy(&sealed.bytes).contains("prompt"), "ciphertext, not text");
    }

    #[test]
    fn the_same_plaintext_gives_different_ciphertext_each_time() {
        let (secret, public) = keypair(2);
        let a = seal(&public, b"same words").unwrap();
        let b = seal(&public, b"same words").unwrap();
        assert_ne!(a.bytes, b.bytes);
        assert_eq!(secret.unseal(&a.bytes).unwrap(), secret.unseal(&b.bytes).unwrap());
    }

    #[test]
    fn the_wrong_key_opens_nothing() {
        let (_, public) = keypair(3);
        let (wrong, _) = keypair(4);
        let sealed = seal(&public, b"secret").unwrap();
        assert!(wrong.unseal(&sealed.bytes).is_err());
        // And tampering is noticed.
        let (right, _) = keypair(3);
        let mut tampered = sealed.bytes.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        assert!(right.unseal(&tampered).is_err());
    }

    #[test]
    fn empty_and_large_payloads_seal_and_a_bad_key_is_refused() {
        let (secret, public) = keypair(5);
        for len in [0usize, 1, 4096, 300_000] {
            let data = vec![b'x'; len];
            assert_eq!(secret.unseal(&seal(&public, &data).unwrap().bytes).unwrap(), data, "{len} bytes");
        }
        assert_eq!(seal(&public[..31], b"x").err(), Some(SealError::BadKey));
        assert_eq!(seal(&[], b"x").err(), Some(SealError::BadKey));
    }

    #[test]
    fn different_keys_have_different_ids() {
        assert_ne!(key_id(&keypair(6).1), key_id(&keypair(7).1));
    }
}
