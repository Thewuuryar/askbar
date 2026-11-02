//! The database backup: a consistent copy with everything secret taken out.
//! Shared by the admin site and the management API.

use crate::AppState;
use axum::http::StatusCode;
use std::sync::Arc;

fn internal_error<E>(_: E) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

/// Deletes the snapshot file when the download finishes or the client goes away.
struct TempFile(std::path::PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
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

/// A consistent snapshot of the database (`VACUUM INTO`) with everything secret taken out:
/// provider API keys and pairing-code hashes (unredeemed or used).
/// Conversations, settings and paired-device records (token hashes only) stay.
fn make_snapshot(state: &AppState, path: &std::path::Path) -> rusqlite::Result<()> {
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

pub async fn response(state: Arc<AppState>) -> Result<axum::response::Response, StatusCode> {
    use axum::{body::Body, http::header};
    let mut raw = [0u8; 8];
    chacha20poly1305::aead::rand_core::RngCore::fill_bytes(&mut chacha20poly1305::aead::OsRng, &mut raw);
    let name: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    let path = std::env::temp_dir().join(format!("askbar-backup-{name}.db"));
    let guard = TempFile(path.clone());
    let st = state.clone();
    let p = path.clone();
    tokio::task::spawn_blocking(move || make_snapshot(&st, &p)).await.map_err(internal_error)?.map_err(internal_error)?;
    let file = tokio::fs::File::open(&path).await.map_err(internal_error)?;
    let len = file.metadata().await.map_err(internal_error)?.len();
    // The guard rides along in the stream, so the file goes when the body is dropped.
    let stream = tokio_stream::StreamExt::map(tokio_util::io::ReaderStream::new(file), move |chunk| {
        let _keep = &guard;
        chunk
    });
    let filename = format!("askbar-backup-{}.db", stamp(crate::auth::now()));
    Ok(axum::response::Response::builder()
        .header(header::CONTENT_TYPE, "application/vnd.sqlite3")
        .header(header::CONTENT_LENGTH, len)
        .header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\""))
        .body(Body::from_stream(stream))
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
