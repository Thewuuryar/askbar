//! Master key rotation: re-encrypt everything under a new key, then swap the key file.
//!
//! Order matters for crash safety:
//!   1. write `master.key.new` (durable)
//!   2. commit the re-encrypted rows
//!   3. `master.key` -> `master.key.prev`, `master.key.new` -> `master.key`
//! `recover` finishes or discards a rotation that was interrupted between those steps.

use crate::secrets::{self, Secrets};
use crate::AppState;
use askbar_core::Db;
use std::path::Path;

/// Settings rows that hold a ciphertext (base64) and must move to the new key too.
pub const ENCRYPTED_SETTINGS: &[&str] = &["cloudflare_token"];

#[derive(Debug, thiserror::Error)]
pub enum RotateError {
    #[error("Setup is not finished: there is no master key to rotate.")]
    NoKey,
    #[error("The stored key for {0} can't be read with the current master key. Re-enter it or delete that provider first; nothing was changed.")]
    Undecryptable(String),
    #[error("Rotation failed and nothing was changed: {0}")]
    Failed(String),
}

fn failed(e: impl std::fmt::Display) -> RotateError {
    RotateError::Failed(e.to_string())
}

/// Rotate to a fresh key. Returns the new key text (shown to the user once).
pub fn rotate(state: &AppState) -> Result<String, RotateError> {
    // Hold the database for the whole operation: chat and admin code read the
    // secrets while holding it, so nobody sees a half-rotated state.
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let old = state.secrets().ok_or(RotateError::NoKey)?;
    let key_path = &state.key_path;
    let new_text = secrets::random_key();
    let new = Secrets::new(&new_text).map_err(failed)?;

    let tx = db.conn().unchecked_transaction().map_err(failed)?;
    {
        let mut stmt = tx.prepare("SELECT id, name, api_key_enc FROM providers WHERE api_key_enc IS NOT NULL").map_err(failed)?;
        let rows: Vec<(String, String, Vec<u8>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(failed)?
            .collect::<Result<_, _>>()
            .map_err(failed)?;
        for (id, name, enc) in rows {
            if enc.is_empty() {
                continue;
            }
            let plain = old.decrypt(&enc).map_err(|_| RotateError::Undecryptable(name))?;
            tx.execute("UPDATE providers SET api_key_enc = ?1 WHERE id = ?2", rusqlite::params![new.encrypt(&plain), id]).map_err(failed)?;
        }
    }
    for key in ENCRYPTED_SETTINGS {
        let stored: Option<String> = tx
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
            .map(Some)
            .or_else(|e| if matches!(e, rusqlite::Error::QueryReturnedNoRows) { Ok(None) } else { Err(e) })
            .map_err(failed)?;
        if let Some(v) = stored {
            let plain = old.decrypt_b64(&v).map_err(|_| RotateError::Undecryptable((*key).to_string()))?;
            tx.execute("UPDATE settings SET value = ?1 WHERE key = ?2", rusqlite::params![new.encrypt_b64(&plain), key]).map_err(failed)?;
        }
    }

    // 1. The new key hits the disk before the rows commit, so it can't be lost.
    let new_path = secrets::new_path(key_path);
    let _ = std::fs::remove_file(&new_path);
    secrets::write_key_file(&new_path, &new_text).map_err(failed)?;
    // 2. Commit.
    if let Err(e) = tx.commit() {
        let _ = std::fs::remove_file(&new_path);
        return Err(failed(e));
    }
    // 3. Swap the files. The data is already on the new key, so from here on use it in memory
    //    no matter what; `recover` finishes a swap that didn't complete.
    state.set_secrets(new);
    if let Err(e) = swap_files(key_path) {
        tracing::error!("master key rotated in the database, but the key file swap failed ({e}); it will be completed at the next start");
    }
    Ok(new_text)
}

fn swap_files(key_path: &Path) -> std::io::Result<()> {
    let (new_path, prev) = (secrets::new_path(key_path), secrets::prev_path(key_path));
    if key_path.exists() {
        std::fs::rename(key_path, &prev)?;
    }
    std::fs::rename(&new_path, key_path)
}

/// Does the previous-key file from an earlier rotation exist?
pub fn previous_key_exists(key_path: &Path) -> bool {
    secrets::prev_path(key_path).exists()
}

pub fn delete_previous_key(key_path: &Path) -> std::io::Result<bool> {
    match std::fs::remove_file(secrets::prev_path(key_path)) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Run at start-up, before the key is loaded. Finishes or discards a rotation that was interrupted.
pub fn recover(db: &Db, key_path: &Path) {
    let new_path = secrets::new_path(key_path);
    if !new_path.exists() {
        return;
    }
    let Ok(Some(new)) = secrets::load_key_file(&new_path) else {
        let _ = std::fs::remove_file(&new_path);
        return;
    };
    // Crash between the two renames: the data is on the new key already.
    if !key_path.exists() {
        tracing::warn!("finishing an interrupted master key rotation");
        let _ = std::fs::rename(&new_path, key_path);
        return;
    }
    // Otherwise see which key the stored ciphertext belongs to.
    let sample: Option<Vec<u8>> = db
        .conn()
        .query_row("SELECT api_key_enc FROM providers WHERE api_key_enc IS NOT NULL AND length(api_key_enc) > 0 LIMIT 1", [], |r| r.get(0))
        .ok();
    let committed = match sample {
        Some(enc) => new.decrypt(&enc).is_ok(),
        None => ENCRYPTED_SETTINGS.iter().any(|k| {
            db.conn()
                .query_row("SELECT value FROM settings WHERE key = ?1", [k], |r| r.get::<_, String>(0))
                .map(|v| new.decrypt_b64(&v).is_ok())
                .unwrap_or(false)
        }),
    };
    if committed {
        tracing::warn!("finishing an interrupted master key rotation");
        let _ = swap_files(key_path);
    } else {
        let _ = std::fs::remove_file(&new_path);
    }
}
