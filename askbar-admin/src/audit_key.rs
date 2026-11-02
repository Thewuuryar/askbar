//! The audit key: a keypair derived from a passphrase the administrator types, so the private
//! half exists only while they are using it. The server gets the public key (in the snapshot)
//! and seals prompts, replies and provider errors to it; opening them takes the passphrase here.
//!
//! The passphrase is stretched with Argon2id and a random 16-byte salt into 32 bytes, which are
//! used as an X25519 secret key (the key type of NaCl sealed boxes). The salt and the Argon2
//! parameters are public and kept with the public key; the passphrase and the secret key are
//! never stored.

use argon2::{Algorithm, Argon2, Params, Version};
use crypto_box::{PublicKey, SecretKey};
use sha2::{Digest, Sha256};

/// A passphrase for the audit key must be at least this long, counted in characters.
pub const MIN_PASSPHRASE_CHARS: usize = 20;

/// Argon2id cost in the form it is stored: memory in KiB, passes, lanes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    pub memory_kib: u32,
    pub passes: u32,
    pub lanes: u32,
}

impl KdfParams {
    /// 64 MiB, 3 passes, 1 lane: well above the minimum OWASP recommends, and about a tenth of a
    /// second on a current machine (opening content is an occasional, deliberate act).
    pub const CURRENT: KdfParams = KdfParams { memory_kib: 65_536, passes: 3, lanes: 1 };

    /// "m=65536,t=3,p=1", as kept in the keyring so a later build can change the default and
    /// still open older keys.
    pub fn to_text(self) -> String {
        format!("m={},t={},p={}", self.memory_kib, self.passes, self.lanes)
    }

    pub fn parse(text: &str) -> Option<KdfParams> {
        let mut m = None;
        let mut t = None;
        let mut p = None;
        for part in text.split(',') {
            let (k, v) = part.trim().split_once('=')?;
            let v: u32 = v.parse().ok()?;
            match k {
                "m" => m = Some(v),
                "t" => t = Some(v),
                "p" => p = Some(v),
                _ => return None,
            }
        }
        Some(KdfParams { memory_kib: m?, passes: t?, lanes: p? })
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KeyError {
    #[error("The passphrase must be at least {MIN_PASSPHRASE_CHARS} characters.")]
    TooShort,
    #[error("The key could not be derived: {0}")]
    Kdf(String),
}

pub struct AuditKeypair {
    pub secret: SecretKey,
    pub public: PublicKey,
}

impl AuditKeypair {
    /// The 32 bytes of the public key, as sent to the server.
    pub fn public_bytes(&self) -> [u8; 32] {
        *self.public.as_bytes()
    }

    pub fn key_id(&self) -> String {
        key_id(&self.public_bytes())
    }
}

/// Stretches `passphrase` and `salt` into a keypair. No length rule here: this is also how a
/// passphrase typed to open content is turned into a key to try (a wrong one simply opens nothing).
pub fn derive(passphrase: &str, salt: &[u8; 16], params: KdfParams) -> Result<AuditKeypair, KeyError> {
    let p = Params::new(params.memory_kib, params.passes, params.lanes, Some(32)).map_err(|e| KeyError::Kdf(e.to_string()))?;
    let mut out = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, p)
        .hash_password_into(passphrase.as_bytes(), salt, &mut out)
        .map_err(|e| KeyError::Kdf(e.to_string()))?;
    let secret = SecretKey::from(out);
    let public = secret.public_key();
    Ok(AuditKeypair { secret, public })
}

/// Derives the key for a new passphrase, refusing one that is too short.
pub fn derive_for_setup(passphrase: &str, salt: &[u8; 16], params: KdfParams) -> Result<AuditKeypair, KeyError> {
    if passphrase.chars().count() < MIN_PASSPHRASE_CHARS {
        return Err(KeyError::TooShort);
    }
    derive(passphrase, salt, params)
}

/// The first 8 bytes of SHA-256 of the public key, in hex (16 characters): names a key without
/// revealing anything, and tells which key sealed a payload.
pub fn key_id(public_key: &[u8; 32]) -> String {
    Sha256::digest(public_key)[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// A fresh random 16-byte salt.
pub fn new_salt() -> [u8; 16] {
    let mut salt = [0u8; 16];
    let mut os = argon2::password_hash::rand_core::OsRng;
    argon2::password_hash::rand_core::RngCore::fill_bytes(&mut os, &mut salt);
    salt
}

/// A sealed payload opened with `secret`, or `None` if it does not open (wrong key, damaged).
pub fn open_sealed(secret: &SecretKey, sealed: &[u8]) -> Option<Vec<u8>> {
    secret.unseal(sealed).ok()
}

/// Limits guesses at the passphrase: `MAX_WRONG` wrong ones within `WINDOW_SECS` lock opening out
/// for `LOCKOUT_SECS`. In memory only, and separate from the login lockout (a wrong passphrase
/// must not lock the administrator out of the site, nor the reverse).
#[derive(Default)]
pub struct Limiter {
    state: std::sync::Mutex<LimiterState>,
}

#[derive(Default)]
struct LimiterState {
    wrong: Vec<i64>,
    locked_until: i64,
}

pub const MAX_WRONG: usize = 5;
pub const WINDOW_SECS: i64 = 15 * 60;
pub const LOCKOUT_SECS: i64 = 15 * 60;

impl Limiter {
    /// Seconds left on a lockout at time `now`, if one is active.
    pub fn locked_for(&self, now: i64) -> Option<i64> {
        let until = self.state.lock().unwrap_or_else(|e| e.into_inner()).locked_until;
        (until > now).then(|| until - now)
    }

    pub fn record_wrong(&self, now: i64) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.wrong.retain(|t| *t > now - WINDOW_SECS);
        st.wrong.push(now);
        if st.wrong.len() >= MAX_WRONG {
            st.locked_until = now + LOCKOUT_SECS;
            st.wrong.clear();
        }
    }

    pub fn record_right(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).wrong.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_wrong_passphrases_lock_opening_for_fifteen_minutes_and_a_right_one_resets_the_count() {
        let l = Limiter::default();
        for i in 0..4 {
            l.record_wrong(1000 + i);
            assert_eq!(l.locked_for(1000 + i), None);
        }
        l.record_right();
        for i in 0..4 {
            l.record_wrong(2000 + i);
        }
        assert_eq!(l.locked_for(2004), None, "the right one in between reset the count");
        l.record_wrong(2004);
        assert_eq!(l.locked_for(2005), Some(LOCKOUT_SECS - 1));
        assert_eq!(l.locked_for(2004 + LOCKOUT_SECS), None, "it ends");
        // Old failures outside the window do not count toward a new lockout.
        let l = Limiter::default();
        for i in 0..4 {
            l.record_wrong(i);
        }
        l.record_wrong(WINDOW_SECS + 100);
        assert_eq!(l.locked_for(WINDOW_SECS + 101), None);
    }

    #[test]
    fn a_payload_opens_only_with_the_key_derived_from_the_right_passphrase() {
        let salt = [4u8; 16];
        let right = derive_for_setup("the right passphrase, long enough", &salt, FAST).unwrap();
        let wrong = derive_for_setup("the wrong passphrase, long enough", &salt, FAST).unwrap();
        let sealed = right.public.seal(&mut crypto_box::aead::OsRng, b"reply text").unwrap();
        assert_eq!(open_sealed(&right.secret, &sealed).as_deref(), Some(&b"reply text"[..]));
        assert_eq!(open_sealed(&wrong.secret, &sealed), None);
        assert_eq!(open_sealed(&right.secret, b"garbage"), None);
    }

    // Cheap parameters keep these tests quick; the point is the wiring, not the cost.
    const FAST: KdfParams = KdfParams { memory_kib: 64, passes: 1, lanes: 1 };
    const PASS: &str = "a passphrase of enough length";

    #[test]
    fn the_same_passphrase_and_salt_give_the_same_key() {
        let a = derive_for_setup(PASS, &[1; 16], FAST).unwrap();
        let b = derive_for_setup(PASS, &[1; 16], FAST).unwrap();
        assert_eq!(a.public_bytes(), b.public_bytes());
        assert_eq!(a.key_id(), b.key_id());
        assert_eq!(a.secret.to_bytes(), b.secret.to_bytes());
        assert_eq!(a.key_id().len(), 16);
    }

    #[test]
    fn a_different_salt_or_passphrase_gives_a_different_key() {
        let base = derive_for_setup(PASS, &[1; 16], FAST).unwrap();
        assert_ne!(base.public_bytes(), derive_for_setup(PASS, &[2; 16], FAST).unwrap().public_bytes());
        assert_ne!(base.public_bytes(), derive_for_setup("a different passphrase of length", &[1; 16], FAST).unwrap().public_bytes());
        // Different cost, different key: the parameters are part of what must be stored.
        assert_ne!(base.public_bytes(), derive_for_setup(PASS, &[1; 16], KdfParams { passes: 2, ..FAST }).unwrap().public_bytes());
    }

    #[test]
    fn passphrases_under_twenty_characters_are_rejected_at_setup() {
        assert_eq!(derive_for_setup("only nineteen chars", &[0; 16], FAST).err(), Some(KeyError::TooShort));
        assert_eq!(derive_for_setup("", &[0; 16], FAST).err(), Some(KeyError::TooShort));
        assert!(derive_for_setup("exactly twenty chars", &[0; 16], FAST).is_ok());
        // Characters, not bytes: twenty multi-byte characters are enough.
        assert!(derive_for_setup(&"é".repeat(20), &[0; 16], FAST).is_ok());
        assert_eq!(derive_for_setup(&"é".repeat(19), &[0; 16], FAST).err(), Some(KeyError::TooShort));
        // Opening content may try any passphrase; a wrong one is just a wrong key.
        assert!(derive("short", &[0; 16], FAST).is_ok());
    }

    #[test]
    fn params_round_trip_through_text_and_garbage_is_refused() {
        assert_eq!(KdfParams::CURRENT.to_text(), "m=65536,t=3,p=1");
        assert_eq!(KdfParams::parse("m=65536,t=3,p=1"), Some(KdfParams::CURRENT));
        assert_eq!(KdfParams::parse(" m=64 , t=1 , p=2 "), Some(KdfParams { memory_kib: 64, passes: 1, lanes: 2 }));
        for bad in ["", "m=1,t=2", "m=a,t=1,p=1", "m=1,t=1,p=1,x=3", "garbage"] {
            assert_eq!(KdfParams::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_key_id_is_the_first_eight_bytes_of_the_sha256_of_the_public_key() {
        let pk = [9u8; 32];
        let full: String = Sha256::digest(pk).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(key_id(&pk), full[..16]);
    }

    #[test]
    fn salts_are_random() {
        assert_ne!(new_salt(), new_salt());
    }
}
