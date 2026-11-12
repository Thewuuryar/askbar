//! The server's backup export (ADR 0013): consistent copies of the control database and of every space's file, in one tar, encrypted
//! to the backup public key from the snapshot and streamed to the admin as it is made.
//!
//! - **Encrypted, so complete.** The export holds everything a restore needs, secrets included (client token hashes, pairing codes),
//!   because only the owner's passphrase opens it; the admin that receives it cannot. With no backup key in the snapshot it is
//!   refused (`503 backup_key_missing`): nothing is ever exported in the clear.
//! - **Streamed.** The tar is written through the encryption into the response as each file is read, so no database is held in
//!   memory. The response has no length: a transfer that ends early (the server stopping, an error) ends the connection badly, which
//!   is how the admin knows it is incomplete, since it cannot decrypt to check.
//! - **Work files on the data volume**, in `<data>/.export-<id>/`, one copy at a time, removed when the export ends however it ends
//!   (a client that goes away included). Leftovers from a crash are removed at the next export.

use crate::error::ApiError;
use crate::AppState;
use askbar_proto::problem::ErrorCode;
use askbar_service::backup;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn internal_error<E>(_: E) -> ApiError {
    ApiError::new(ErrorCode::Internal, "Could not make the backup.")
}

/// Deletes the working directory, whatever way the export ends.
struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Work directories older than this are leftovers of a crash.
const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(60 * 60);
const PREFIX: &str = ".export-";

/// Removes leftover work directories from an export that did not end cleanly.
fn clear_stale(parent: &Path) {
    let Ok(entries) = std::fs::read_dir(parent) else { return };
    for e in entries.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > STALE_AFTER);
        if old && e.file_name().to_string_lossy().starts_with(PREFIX) {
            let _ = std::fs::remove_dir_all(e.path());
        }
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

/// A consistent copy (`VACUUM INTO`) of the control database.
fn control_copy(state: &AppState, path: &Path) -> rusqlite::Result<()> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    db.conn().execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])?;
    Ok(())
}

/// A consistent copy of one space's file.
fn space_copy(space: &crate::Space, path: &Path) -> rusqlite::Result<()> {
    space.lock().conn().execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])?;
    Ok(())
}

/// One file of the export, streamed into the tar, then deleted.
fn add_file<W: Write>(tar: &mut W, name: &str, path: &Path, mtime: i64) -> std::io::Result<()> {
    let size = std::fs::metadata(path)?.len();
    askbar_service::tar::write_file(tar, name, size, &mut std::fs::File::open(path)?, mtime)?;
    std::fs::remove_file(path)
}

/// Writes the export: a tar with `control.db` and `spaces/<space id>.db` for every space (ADR 0009 mechanism 4), through the
/// encryption, into `out`. Copies are made one at a time under `dir`.
fn write_export<W: Write>(state: &AppState, dir: &Path, recipient: &str, out: W) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    std::fs::create_dir_all(dir)?;
    let now = crate::auth::now();
    let mut tar = backup::encrypt_to(recipient, out).map_err(|e| e.to_string())?;
    let control = dir.join("control.db");
    control_copy(state, &control)?;
    add_file(&mut tar, "control.db", &control, now)?;
    let spaces = state.spaces.open_all(&state.db.lock().unwrap_or_else(|e| e.into_inner()))?;
    for space in spaces {
        let path = dir.join(format!("{}.db", space.id));
        space_copy(&space, &path)?;
        add_file(&mut tar, &format!("spaces/{}.db", space.id), &path, now)?;
    }
    askbar_service::tar::write_end(&mut tar)?;
    tar.finish()?;
    Ok(())
}

/// The backup public key the admin gave in the snapshot, if there is one that is a key.
fn backup_recipient(state: &AppState) -> Option<String> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    crate::snapshot::current(&db).ok().flatten()?.backup_public_key.filter(|k| backup::is_recipient(k))
}

pub async fn response(state: Arc<AppState>) -> Result<axum::response::Response, ApiError> {
    use axum::http::header;
    let recipient = backup_recipient(&state).ok_or_else(|| ApiError::code(ErrorCode::BackupKeyMissing))?;
    let parent = state.spaces.scratch_dir().unwrap_or_else(std::env::temp_dir);
    clear_stale(&parent);
    let dir = parent.join(format!("{PREFIX}{}", crate::ids::new_id()));
    let guard = TempDir(dir.clone());
    let (mut writer, body) = askbar_service::stream::body();
    let st = state.clone();
    tokio::task::spawn_blocking(move || {
        let _guard = guard; // the work directory goes when this ends, a failure or a vanished client included
        let done = write_export(&st, &dir, &recipient, &mut writer).and_then(|_| writer.flush().map_err(Into::into));
        if let Err(e) = done {
            tracing::warn!("a backup export did not finish: {e}");
            // An error in the body ends the connection badly: the admin sees an export that is not whole.
            writer.fail();
        }
    });
    let filename = format!("askbar-server-backup-{}.tar.age", stamp(crate::auth::now()));
    axum::response::Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\""))
        .body(body)
        .map_err(internal_error)
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
