//! Keys, sign and verify for one approved tool action (feature `approval`).
//!
//! `action_hash` is hex SHA-256 over [`askbar_proto::approval::canonical_bytes`]. The signature (Ed25519, base64) is over the
//! message built by [`message`]. Verify checks, in order: the action hash (swapped), expiry, signature, then consumes the
//! one-time ID. The ID is consumed only when every other check has passed. Keys and signatures never appear in an error.

use askbar_proto::approval::{canonical_bytes, Approval, ToolAction};
use base64::{engine::general_purpose::STANDARD, Engine};
use ring::{
    digest::{digest, SHA256},
    rand::SystemRandom,
    signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519},
};
use subtle::ConstantTimeEq;

/// The caller's store of used one-time IDs.
pub trait OneTimeIds {
    /// Record `id` (kept at least until `expires_at`). True if it was new, false if it was already used.
    fn consume(&self, id: &str, expires_at: i64) -> bool;
}

/// Why an approval was refused. Carries no data, so nothing secret can leak through `Debug` or `Display`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalError {
    /// The action cannot be canonicalised (a float, say), so it cannot be signed.
    Unsignable,
    /// The private key is not a valid Ed25519 PKCS#8 key, or the random source failed.
    Key,
    /// The approval was made for a different action.
    Swapped,
    Expired,
    /// The signature is empty.
    Unsigned,
    /// The signature is malformed, or not by this key over these fields.
    BadSignature,
    /// The one-time ID was used before.
    Replayed,
}

impl ApprovalError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unsignable => "approval_unsignable",
            Self::Key => "approval_key_invalid",
            Self::Swapped => "approval_action_swapped",
            Self::Expired => "approval_expired",
            Self::Unsigned => "approval_unsigned",
            Self::BadSignature => "approval_signature_invalid",
            Self::Replayed => "approval_replayed",
        }
    }
}

impl std::fmt::Display for ApprovalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for ApprovalError {}

/// A new Ed25519 key: (PKCS#8 private key, raw 32-byte public key).
pub fn generate_keypair() -> Result<(Vec<u8>, Vec<u8>), ApprovalError> {
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).map_err(|_| ApprovalError::Key)?;
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).map_err(|_| ApprovalError::Key)?;
    Ok((pkcs8.as_ref().to_vec(), pair.public_key().as_ref().to_vec()))
}

fn action_hash(action: &ToolAction) -> Result<String, ApprovalError> {
    let bytes = canonical_bytes(action).map_err(|_| ApprovalError::Unsignable)?;
    Ok(digest(&SHA256, &bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect())
}

/// The signed message: a fixed tag, then each of action_hash, task_id, user and one_time_id as a 4-byte big-endian length and its
/// bytes, then expires_at as 8 bytes big-endian. No field can run into the next.
fn message(a: &Approval) -> Vec<u8> {
    let mut m = b"askbar-approval-v1".to_vec();
    for f in [&a.action_hash, &a.task_id, &a.user, &a.one_time_id] {
        m.extend_from_slice(&(f.len() as u32).to_be_bytes());
        m.extend_from_slice(f.as_bytes());
    }
    m.extend_from_slice(&a.expires_at.to_be_bytes());
    m
}

/// An approval for `action`, signed with the PKCS#8 key.
pub fn sign(
    action: &ToolAction,
    task_id: &str,
    user: &str,
    one_time_id: &str,
    expires_at: i64,
    pkcs8: &[u8],
) -> Result<Approval, ApprovalError> {
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8).map_err(|_| ApprovalError::Key)?;
    let mut a = Approval {
        action_hash: action_hash(action)?,
        task_id: task_id.into(),
        user: user.into(),
        one_time_id: one_time_id.into(),
        expires_at,
        signature: String::new(),
    };
    a.signature = STANDARD.encode(pair.sign(&message(&a)).as_ref());
    Ok(a)
}

/// Check `approval` for `action` at `now` (UTC unix seconds) and consume its one-time ID.
pub fn verify(approval: &Approval, action: &ToolAction, public_key: &[u8], now: i64, ids: &dyn OneTimeIds) -> Result<(), ApprovalError> {
    let want = action_hash(action)?;
    if !bool::from(want.as_bytes().ct_eq(approval.action_hash.as_bytes())) {
        return Err(ApprovalError::Swapped);
    }
    if now >= approval.expires_at {
        return Err(ApprovalError::Expired);
    }
    if approval.signature.is_empty() {
        return Err(ApprovalError::Unsigned);
    }
    let sig = STANDARD.decode(&approval.signature).map_err(|_| ApprovalError::BadSignature)?;
    UnparsedPublicKey::new(&ED25519, public_key).verify(&message(approval), &sig).map_err(|_| ApprovalError::BadSignature)?;
    if !ids.consume(&approval.one_time_id, approval.expires_at) {
        return Err(ApprovalError::Replayed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use askbar_proto::approval::Provenance;
    use std::{cell::RefCell, collections::HashSet};

    #[derive(Default)]
    struct Ids(RefCell<HashSet<String>>);
    impl OneTimeIds for Ids {
        fn consume(&self, id: &str, _expires_at: i64) -> bool {
            self.0.borrow_mut().insert(id.into())
        }
    }

    const NOW: i64 = 1_700_000_000;

    fn action(dest: &str) -> ToolAction {
        ToolAction {
            tool: "fetch".into(),
            arguments: serde_json::json!({"url": "https://example.com"}),
            destination: dest.into(),
            provenance: Provenance::User,
            query_data: false,
        }
    }

    fn fixture() -> (Approval, Vec<u8>, Vec<u8>) {
        let (sk, pk) = generate_keypair().unwrap();
        (sign(&action("example.com"), "task1", "alice", "once1", NOW + 60, &sk).unwrap(), sk, pk)
    }

    fn check(a: &Approval, pk: &[u8], ids: &Ids) -> Result<(), ApprovalError> {
        verify(a, &action("example.com"), pk, NOW, ids)
    }

    #[test]
    fn a_valid_approval_passes() {
        let (a, _, pk) = fixture();
        assert_eq!(check(&a, &pk, &Ids::default()), Ok(()));
    }

    #[test]
    fn each_failure_has_its_own_code() {
        let (a, _, pk) = fixture();
        let ids = Ids::default();
        let mut unsigned = a.clone();
        unsigned.signature.clear();
        let mut garbage = a.clone();
        garbage.signature = "!!not base64!!".into();
        let (_, other_pk) = generate_keypair().unwrap();

        assert_eq!(check(&unsigned, &pk, &ids), Err(ApprovalError::Unsigned));
        assert_eq!(check(&garbage, &pk, &ids), Err(ApprovalError::BadSignature));
        assert_eq!(check(&a, &other_pk, &ids), Err(ApprovalError::BadSignature));
        assert_eq!(verify(&a, &action("evil.com"), &pk, NOW, &ids), Err(ApprovalError::Swapped));
        assert_eq!(verify(&a, &action("example.com"), &pk, NOW + 60, &ids), Err(ApprovalError::Expired));
        // none of the failures above used the id
        assert_eq!(check(&a, &pk, &ids), Ok(()));
        assert_eq!(check(&a, &pk, &ids), Err(ApprovalError::Replayed));

        let codes = [
            ApprovalError::Unsignable,
            ApprovalError::Key,
            ApprovalError::Swapped,
            ApprovalError::Expired,
            ApprovalError::Unsigned,
            ApprovalError::BadSignature,
            ApprovalError::Replayed,
        ]
        .map(ApprovalError::code);
        assert_eq!(codes.iter().collect::<HashSet<_>>().len(), codes.len());
    }

    #[test]
    fn a_tampered_field_fails() {
        let (a, _, pk) = fixture();
        let mut t = a.clone();
        t.task_id = "task2".into();
        let mut u = a.clone();
        u.user = "bob".into();
        let mut o = a.clone();
        o.one_time_id = "once2".into();
        let mut e = a.clone();
        e.expires_at += 1;
        for bad in [t, u, o, e] {
            assert_eq!(check(&bad, &pk, &Ids::default()), Err(ApprovalError::BadSignature));
        }
    }

    #[test]
    fn a_field_boundary_cannot_move() {
        let (sk, pk) = generate_keypair().unwrap();
        let act = action("example.com");
        let a = sign(&act, "ab", "c", "id", NOW + 60, &sk).unwrap();
        let mut moved = a.clone();
        moved.task_id = "a".into();
        moved.user = "bc".into();
        assert_eq!(verify(&moved, &act, &pk, NOW, &Ids::default()), Err(ApprovalError::BadSignature));
    }

    #[test]
    fn a_bad_key_and_a_float_are_refused() {
        assert_eq!(sign(&action("x"), "t", "u", "o", NOW, b"nope"), Err(ApprovalError::Key));
        let mut f = action("x");
        f.arguments = serde_json::json!({"n": 1.5});
        let (sk, _) = generate_keypair().unwrap();
        assert_eq!(sign(&f, "t", "u", "o", NOW, &sk), Err(ApprovalError::Unsignable));
    }

    #[test]
    fn no_key_bytes_in_debug_output() {
        let (a, sk, pk) = fixture();
        let shown = format!("{:?}{}", ApprovalError::BadSignature, ApprovalError::Key);
        for secret in [&sk, &pk] {
            assert!(!shown.contains(&format!("{secret:?}")));
            assert!(!shown.contains(&STANDARD.encode(secret)));
        }
        assert!(!shown.contains(&a.signature));
    }
}
