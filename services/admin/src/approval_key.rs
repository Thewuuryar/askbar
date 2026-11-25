//! The approval signing key (plan step P5.3): made once, kept in `admin.db` encrypted with `admin.key` (as the provider keys
//! are, so a backup holds it only encrypted, ADR 0013) and pushed to `model-gateway` in `PUT /config`, which holds it in memory.
//! The public key and version stay here for the MCP front's push (`mcp_link`). No key is logged, shown or returned by anything here.

use crate::secrets::AdminSecrets;
use askbar_proto::gateway::ApprovalSigningKey;
use askbar_service::approval::generate_keypair;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::{params, Connection, OptionalExtension};

/// Make the key if there is none.
pub fn ensure(conn: &Connection, secrets: &AdminSecrets) -> Result<(), String> {
    let held: bool = conn.query_row("SELECT EXISTS (SELECT 1 FROM approval_keys)", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    if held {
        return Ok(());
    }
    rotate(conn, secrets)
}

/// Replace the key with a new one under the next version. The caller re-pushes (the gateway's next sync does, since the
/// configuration changed) and, with a route, records the action in the auditor first (ADR 0010).
pub fn rotate(conn: &Connection, secrets: &AdminSecrets) -> Result<(), String> {
    let (private, public) = generate_keypair().map_err(|e| e.code().to_string())?;
    let enc = secrets.encrypt(&URL_SAFE_NO_PAD.encode(private));
    conn.execute(
        "INSERT INTO approval_keys (id, key_enc, public_key, version, created_at) VALUES (1, ?1, ?2, 1, ?3)
         ON CONFLICT(id) DO UPDATE SET key_enc = excluded.key_enc, public_key = excluded.public_key,
                                       version = approval_keys.version + 1, created_at = excluded.created_at",
        params![enc, public, crate::auth::now()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// What `model-gateway` is pushed: none before `ensure`, or if the stored key cannot be read.
pub fn signing_key(conn: &Connection, secrets: &AdminSecrets) -> Option<ApprovalSigningKey> {
    let (enc, version): (Vec<u8>, i64) =
        conn.query_row("SELECT key_enc, version FROM approval_keys WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?))).optional().ok()??;
    Some(ApprovalSigningKey { version: version as u64, pkcs8: secrets.decrypt(&enc)? })
}

/// The raw 32-byte public key and its version, for the MCP front.
pub fn public_key(conn: &Connection) -> Option<(u64, Vec<u8>)> {
    conn.query_row("SELECT version, public_key FROM approval_keys WHERE id = 1", [], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?)))
        .optional()
        .ok()?
}

/// The first 16 hex digits of the SHA-256 of the raw public key: safe to show.
pub fn fingerprint(public: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(public)[..8].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::AdminDb;

    #[test]
    fn made_once_rotated_to_a_new_version_and_never_shown() {
        let db = AdminDb::open_in_memory().unwrap();
        let s = AdminSecrets::ephemeral();
        let c = db.conn();
        assert!(signing_key(c, &s).is_none() && public_key(c).is_none(), "nothing before the first ensure");
        ensure(c, &s).unwrap();
        let (k1, (v1, p1)) = (signing_key(c, &s).unwrap(), public_key(c).unwrap());
        ensure(c, &s).unwrap();
        assert_eq!(signing_key(c, &s).unwrap(), k1, "ensure keeps the key");
        let private = URL_SAFE_NO_PAD.decode(&k1.pkcs8).unwrap();
        assert!(p1.len() == 32 && !private.is_empty());

        rotate(c, &s).unwrap();
        let (k2, (v2, p2)) = (signing_key(c, &s).unwrap(), public_key(c).unwrap());
        assert_eq!((v1, k2.version, v2), (1, 2, 2));
        assert!(p1 != p2 && k1.pkcs8 != k2.pkcs8);

        // The key is not in the database in the clear, nor in Debug output.
        let raw: Vec<u8> = c.query_row("SELECT key_enc FROM approval_keys", [], |r| r.get(0)).unwrap();
        for bytes in [&private[..], k1.pkcs8.as_bytes()] {
            assert!(!raw.windows(bytes.len()).any(|w| w == bytes));
        }
        assert!(!format!("{k2:?}").contains(&k2.pkcs8));
        let cfg = crate::gateway_link::build_config(c, &s, None).unwrap();
        assert_eq!(cfg.approval_key.as_ref(), Some(&k2));
        assert!(!format!("{cfg:?}").contains(&k2.pkcs8));
    }
}
