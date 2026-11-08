//! The server's backup export: consistent copies of the control database (with everything secret
//! taken out) and of every space's file, in one tar. The admin fetches it over the server API.

use crate::error::ApiError;
use crate::AppState;
use askbar_proto::problem::ErrorCode;
use std::sync::Arc;

fn internal_error<E>(_: E) -> ApiError {
    ApiError::new(ErrorCode::Internal, "Could not make the backup.")
}

/// Deletes the working directory, whatever way the export ends.
struct TempDir(std::path::PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `YYYYMMDD-HHMMSS` in UTC for a unix time (civil-from-days, no date crate needed).
fn stamp(unix: i64) -> String {
    let (days, secs) = (unix.div_euclid(86_400), unix.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}-{:02}{:02}{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
}

/// A consistent copy (`VACUUM INTO`) of the control database with everything secret taken out:
/// provider API keys and pairing-code hashes (unredeemed or used). Settings and paired-client
/// records (token hashes only) stay.
fn control_copy(state: &AppState, path: &std::path::Path) -> rusqlite::Result<()> {
    {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db.conn().execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])?;
    }
    let snap = rusqlite::Connection::open(path)?;
    // The snapshot document lists unredeemed pairing-code hashes; take those out too.
    let body: Option<String> = snap.query_row("SELECT body FROM config_snapshot WHERE id = 1", [], |r| r.get(0)).ok();
    if let Some(mut doc) = body.and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok()) {
        doc["pair_codes"] = serde_json::json!([]);
        snap.execute("UPDATE config_snapshot SET body = ?1 WHERE id = 1", [doc.to_string()])?;
    }
    snap.execute_batch(
        "UPDATE providers SET api_key_enc = NULL;
         DELETE FROM redeemed_codes;
         PRAGMA journal_mode = DELETE;
         VACUUM;", // rewrite the file so the removed values don't linger in free pages
    )?;
    Ok(())
}

/// A consistent copy of one space's file. It holds no secrets.
fn space_copy(space: &crate::Space, path: &std::path::Path) -> rusqlite::Result<()> {
    space.lock().conn().execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])?;
    rusqlite::Connection::open(path)?.execute_batch("PRAGMA journal_mode = DELETE;")?;
    Ok(())
}

/// The export: a tar with `control.db` and `spaces/<space id>.db` for every space (ADR 0009
/// mechanism 4). Copies are made one at a time under `dir`, read in, and deleted.
fn make_export(state: &AppState, dir: &std::path::Path) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    std::fs::create_dir_all(dir)?;
    let now = crate::auth::now();
    let mut tar = Vec::new();
    let control = dir.join("control.db");
    control_copy(state, &control)?;
    askbar_service::tar::append(&mut tar, "control.db", &std::fs::read(&control)?, now);
    std::fs::remove_file(&control)?;
    let spaces = state.spaces.open_all(&state.db.lock().unwrap_or_else(|e| e.into_inner()))?;
    for space in spaces {
        let path = dir.join(format!("{}.db", space.id));
        space_copy(&space, &path)?;
        askbar_service::tar::append(&mut tar, &format!("spaces/{}.db", space.id), &std::fs::read(&path)?, now);
        std::fs::remove_file(&path)?;
    }
    askbar_service::tar::finish(&mut tar);
    Ok(tar)
}

pub async fn response(state: Arc<AppState>) -> Result<axum::response::Response, ApiError> {
    use axum::{body::Body, http::header};
    let mut raw = [0u8; 8];
    chacha20poly1305::aead::rand_core::RngCore::fill_bytes(&mut chacha20poly1305::aead::OsRng, &mut raw);
    let name: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    let dir = std::env::temp_dir().join(format!("askbar-backup-{name}"));
    let guard = TempDir(dir.clone());
    let st = state.clone();
    let tar = tokio::task::spawn_blocking(move || make_export(&st, &guard.0)).await.map_err(internal_error)?.map_err(internal_error)?;
    let filename = format!("askbar-server-backup-{}.tar", stamp(crate::auth::now()));
    Ok(axum::response::Response::builder()
        .header(header::CONTENT_TYPE, "application/x-tar")
        .header(header::CONTENT_LENGTH, tar.len())
        .header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\""))
        .body(Body::from(tar))
        .map_err(internal_error)?)
}

#[cfg(test)]
mod tests {
    use super::stamp;

    #[test]
    fn timestamps() {
        assert_eq!(stamp(0), "19700101-000000");
        assert_eq!(stamp(951_782_400 + 86_399), "20000229-235959"); // leap day
        assert_eq!(stamp(1_793_227_200), "20261028-224000");
    }
}
