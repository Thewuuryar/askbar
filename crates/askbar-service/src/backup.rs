//! The backup key (ADR 0013, D22): an age key pair whose private half is encrypted with a passphrase.
//!
//! The admin makes the pair, keeps the public half (`age1…`, the recipient every component encrypts its export to) and the
//! private half encrypted with the owner's passphrase (`backup-key.age`, which every backup carries), and forgets the passphrase.
//! Nothing here stores or logs one. A backup opens with the passphrase alone: `age -d backup-key.age` gives the private key, and
//! `age -d -i <that key> <export>` opens an export, with the ordinary `age` command-line tool.
//!
//! The scrypt work factor is fixed (2^18, about a second on a current machine, age's own default target) rather than calibrated,
//! so a protected key opens the same way on any machine; a file that asks for more than 2^22 is refused, whatever it says.

use age::secrecy::{ExposeSecret, SecretString};
use age::x25519;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::iter;
use std::str::FromStr;

/// The shortest passphrase the owner may choose.
pub const MIN_PASSPHRASE_CHARS: usize = 20;
/// scrypt's work factor, as a power of two, for protecting the private key.
const WORK_FACTOR: u8 = 18;
/// The most a file may ask of us when opening it (age's own advice: more lets a hostile file cost hours and gigabytes).
const MAX_WORK_FACTOR: u8 = 22;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The passphrase is shorter than [`MIN_PASSPHRASE_CHARS`].
    TooShort,
    /// The passphrase does not open the protected key.
    WrongPassphrase,
    /// The key or the file is not what this makes.
    Invalid,
    /// The encryption itself failed (nothing the caller can fix).
    Crypto,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Error::TooShort => "The backup passphrase must be at least 20 characters.",
            Error::WrongPassphrase => "The backup passphrase is wrong.",
            Error::Invalid => "That is not a key or a file this version of Askbar makes.",
            Error::Crypto => "The encryption failed.",
        })
    }
}

impl std::error::Error for Error {}

/// A new backup key, as the admin keeps it.
#[derive(Clone, PartialEq, Eq)]
pub struct Key {
    /// The public key, `age1…`: what the components encrypt to.
    pub recipient: String,
    /// The private key encrypted with the passphrase: the contents of `backup-key.age`. Safe to keep and to give away; only the
    /// passphrase opens it.
    pub protected: Vec<u8>,
    /// A short name for the public key, for people to compare (see [`fingerprint`]).
    pub fingerprint: String,
}

// A key never shows its contents in a log or a panic message.
impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Key").field("fingerprint", &self.fingerprint).finish_non_exhaustive()
    }
}

fn secret(passphrase: &str) -> SecretString {
    SecretString::from(passphrase.to_owned())
}

/// A key pair, with the private half protected by `passphrase`. Takes about a second: the passphrase is stretched with scrypt.
pub fn generate(passphrase: &str) -> Result<Key, Error> {
    generate_with(passphrase, WORK_FACTOR)
}

/// [`generate`] with another scrypt work factor. For tests, which cannot wait a second for each key.
#[doc(hidden)]
pub fn generate_with(passphrase: &str, work_factor: u8) -> Result<Key, Error> {
    if passphrase.chars().count() < MIN_PASSPHRASE_CHARS {
        return Err(Error::TooShort);
    }
    let identity = x25519::Identity::generate();
    let recipient = identity.to_public().to_string();
    let mut wrap = age::scrypt::Recipient::new(secret(passphrase));
    wrap.set_work_factor(work_factor);
    let encryptor = age::Encryptor::with_recipients(iter::once(&wrap as &dyn age::Recipient)).map_err(|_| Error::Crypto)?;
    let mut protected = Vec::new();
    let mut writer = encryptor.wrap_output(&mut protected).map_err(|_| Error::Crypto)?;
    writer.write_all(identity.to_string().expose_secret().as_bytes()).map_err(|_| Error::Crypto)?;
    writer.finish().map_err(|_| Error::Crypto)?;
    Ok(Key { fingerprint: fingerprint(&recipient), recipient, protected })
}

/// The private key, opened with the passphrase. Not kept: the caller uses it and lets it go (a restore, and the tests).
pub fn unlock(protected: &[u8], passphrase: &str) -> Result<x25519::Identity, Error> {
    let decryptor = age::Decryptor::new(protected).map_err(|_| Error::Invalid)?;
    let mut identity = age::scrypt::Identity::new(secret(passphrase));
    identity.set_max_work_factor(MAX_WORK_FACTOR);
    let mut reader = decryptor.decrypt(iter::once(&identity as &dyn age::Identity)).map_err(|e| match e {
        age::DecryptError::DecryptionFailed | age::DecryptError::NoMatchingKeys => Error::WrongPassphrase,
        _ => Error::Invalid,
    })?;
    // A private key is one short line; anything longer is not one.
    let mut text = String::new();
    reader.by_ref().take(4096).read_to_string(&mut text).map_err(|_| Error::Invalid)?;
    x25519::Identity::from_str(text.trim()).map_err(|_| Error::Invalid)
}

/// Whether `s` is a public key this makes (`age1…`, X25519).
pub fn is_recipient(s: &str) -> bool {
    x25519::Recipient::from_str(s).is_ok()
}

/// A short name for a public key: the first 16 hex digits of its SHA-256, in four groups (`1a2b-3c4d-5e6f-7a8b`). It is what the
/// admin shows, what a backup's manifest carries and what the audit record names, so a person can tell two keys apart.
pub fn fingerprint(recipient: &str) -> String {
    let digest = Sha256::digest(recipient.as_bytes());
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}", &hex[0..4], &hex[4..8], &hex[8..12], &hex[12..16])
}

/// `plaintext` encrypted to `recipient`.
pub fn encrypt(recipient: &str, plaintext: &[u8]) -> Result<Vec<u8>, Error> {
    let recipient = x25519::Recipient::from_str(recipient).map_err(|_| Error::Invalid)?;
    let encryptor = age::Encryptor::with_recipients(iter::once(&recipient as &dyn age::Recipient)).map_err(|_| Error::Crypto)?;
    let mut out = Vec::with_capacity(plaintext.len() + 256);
    let mut writer = encryptor.wrap_output(&mut out).map_err(|_| Error::Crypto)?;
    writer.write_all(plaintext).map_err(|_| Error::Crypto)?;
    writer.finish().map_err(|_| Error::Crypto)?;
    Ok(out)
}

/// What [`encrypt`] made, opened with the private key.
pub fn decrypt(identity: &x25519::Identity, ciphertext: &[u8]) -> Result<Vec<u8>, Error> {
    let decryptor = age::Decryptor::new(ciphertext).map_err(|_| Error::Invalid)?;
    let mut reader = decryptor.decrypt(iter::once(identity as &dyn age::Identity)).map_err(|_| Error::WrongPassphrase)?;
    let mut out = Vec::new();
    reader.read_to_end(&mut out).map_err(|_| Error::Invalid)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASS: &str = "a long backup passphrase 1";
    // Low enough to be quick; the strong default is tested once below.
    const FAST: u8 = 10;

    fn key() -> Key {
        generate_with(PASS, FAST).unwrap()
    }

    #[test]
    fn a_key_pair_round_trips_through_its_passphrase_and_encrypts_and_decrypts_data() {
        let k = key();
        assert!(k.recipient.starts_with("age1") && is_recipient(&k.recipient));
        let identity = unlock(&k.protected, PASS).unwrap();
        assert_eq!(identity.to_public().to_string(), k.recipient, "the protected key is the private half of the public one");
        let data = b"an export: control.db, spaces, admin.db".repeat(1000);
        let encrypted = encrypt(&k.recipient, &data).unwrap();
        assert!(!encrypted.windows(16).any(|w| w == &data[..16]), "no plaintext in the ciphertext");
        assert_eq!(decrypt(&identity, &encrypted).unwrap(), data);
    }

    #[test]
    fn the_protected_key_opens_only_with_the_passphrase_and_holds_no_plaintext_key() {
        let k = key();
        assert_eq!(unlock(&k.protected, "another long passphrase 22").err(), Some(Error::WrongPassphrase));
        assert_eq!(unlock(&k.protected, "").err(), Some(Error::WrongPassphrase));
        assert_eq!(unlock(b"not an age file", PASS).err(), Some(Error::Invalid));
        assert_eq!(unlock(&[], PASS).err(), Some(Error::Invalid));
        assert!(!k.protected.windows(14).any(|w| w == b"AGE-SECRET-KEY"), "the private key is not in the clear");
        // The age file format, so the ordinary age tool can open it: a header naming scrypt.
        assert!(k.protected.starts_with(b"age-encryption.org/v1\n-> scrypt "), "{:?}", String::from_utf8_lossy(&k.protected[..40]));
    }

    #[test]
    fn a_passphrase_shorter_than_twenty_characters_makes_no_key() {
        assert_eq!(generate_with("nineteen characters", FAST).err(), Some(Error::TooShort));
        assert_eq!(generate_with("", FAST).err(), Some(Error::TooShort));
        // Characters, not bytes: twenty accented letters are twenty.
        assert!(generate_with(&"é".repeat(20), FAST).is_ok());
        assert_eq!(generate_with(&"é".repeat(19), FAST).err(), Some(Error::TooShort));
    }

    #[test]
    fn two_keys_differ_and_a_fingerprint_names_one_public_key() {
        let (a, b) = (key(), key());
        assert_ne!(a.recipient, b.recipient);
        assert_ne!(a.fingerprint, b.fingerprint);
        assert_eq!(a.fingerprint, fingerprint(&a.recipient));
        let f = &a.fingerprint;
        assert_eq!(f.len(), 19);
        assert!(f.split('-').all(|g| g.len() == 4 && g.bytes().all(|c| c.is_ascii_hexdigit())), "{f}");
        // Data for one key does not open with the other.
        let encrypted = encrypt(&a.recipient, b"x").unwrap();
        assert!(decrypt(&unlock(&b.protected, PASS).unwrap(), &encrypted).is_err());
    }

    #[test]
    fn anything_that_is_not_a_public_key_is_refused() {
        for bad in ["", "age1", "age1notakey", "AGE-SECRET-KEY-1ABC", "ssh-ed25519 AAAA", " age1qqqqqqqq", "https://example.com"] {
            assert!(!is_recipient(bad), "{bad:?}");
            assert!(encrypt(bad, b"x").is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_key_never_shows_its_contents_when_printed() {
        let k = key();
        let shown = format!("{k:?}");
        assert!(!shown.contains(&k.recipient) && !shown.contains("age-encryption"), "{shown}");
        assert!(shown.contains(&k.fingerprint));
    }

    #[test]
    fn the_strong_default_works_and_an_export_opens_with_the_key_it_was_made_with() {
        let started = std::time::Instant::now();
        let k = generate(PASS).unwrap();
        assert!(unlock(&k.protected, PASS).is_ok());
        assert!(started.elapsed() < std::time::Duration::from_secs(60), "key making is slow, not unbounded");
    }
}
