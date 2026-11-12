//! The server's restore (ADR 0013): `POST /api/server/v1/imports` takes what `POST /exports` made, opened by the admin, and makes
//! it the server's own: the control database and every space file.
//!
//! - **Only into an empty install.** The server starts with one seeded owner and an empty personal space; anything more (another
//!   person, a paired client, a conversation) is `409 not_empty` and nothing is changed or even read.
//! - **All or nothing.** The tar is written to a work directory on the data volume (the data is plain there, as it is in
//!   `control.db`), every file is opened and migrated as a copy, and only a set that opens and agrees with itself (every space the
//!   control database lists has its file, and no other) is swapped in. A backup from a newer release, or a file that is not one of
//!   ours, is refused before anything is touched.
//! - **A hot swap.** The control database is copied into the open connection with SQLite's backup API, the space files are moved into
//!   place and the seeded owner's are removed, so no restart is needed.
//! - **Work files** are in `<data>/.import-<id>/` and are removed however the import ends.

use crate::db::{Db, DbError};
use crate::error::ApiError;
use crate::space::{is_space_id, SpaceStore};
use crate::AppState;
use askbar_proto::problem::ErrorCode;
use futures_util::StreamExt;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const PREFIX: &str = ".import-";

fn internal<E: std::fmt::Display>(e: E) -> ApiError {
    tracing::warn!("an import failed: {e}");
    ApiError::new(ErrorCode::Internal, "Could not restore the backup.")
}

fn bad(detail: &str) -> ApiError {
    ApiError::new(ErrorCode::BadRequest, detail)
}

struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Whether this is an install nobody has used: at most the seeded owner, no paired client, no pairing code and no conversation.
pub fn is_empty(state: &AppState) -> Result<bool, DbError> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let count = |sql: &str| db.conn().query_row(sql, [], |r| r.get::<_, i64>(0));
    if count("SELECT COUNT(*) FROM users")? > 1
        || count("SELECT COUNT(*) FROM clients")? > 0
        || count("SELECT COUNT(*) FROM client_pair_codes")? > 0
    {
        return Ok(false);
    }
    for space in state.spaces.open_all(&db)? {
        if space.lock().conn().query_row("SELECT COUNT(*) FROM conversations", [], |r| r.get::<_, i64>(0))? > 0 {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Unpacks the plain tar at `tar` into `dir`: `control.db` and `spaces/<id>.db`, nothing else, none twice.
fn unpack(tar: &Path, dir: &Path) -> Result<BTreeSet<String>, ApiError> {
    let mut file = std::fs::File::open(tar).map_err(internal)?;
    let mut spaces = BTreeSet::new();
    let mut have_control = false;
    std::fs::create_dir_all(dir.join("spaces")).map_err(internal)?;
    let mut refused: Option<&'static str> = None;
    let read = askbar_service::tar::for_each(&mut file, |name, _size, data| {
        let target = if name == "control.db" {
            if std::mem::replace(&mut have_control, true) {
                refused = Some("The backup holds the control database twice.");
                return Err(std::io::Error::other("refused"));
            }
            dir.join("control.db")
        } else if let Some(id) = name.strip_prefix("spaces/").and_then(|n| n.strip_suffix(".db")).filter(|id| is_space_id(id)) {
            if !spaces.insert(id.to_string()) {
                refused = Some("The backup holds a space twice.");
                return Err(std::io::Error::other("refused"));
            }
            dir.join("spaces").join(format!("{id}.db"))
        } else {
            refused = Some("The backup holds a file this server did not make.");
            return Err(std::io::Error::other("refused"));
        };
        let mut out = std::fs::File::create(target)?;
        std::io::copy(data, &mut out)?;
        out.sync_all()
    });
    match (read, refused) {
        (_, Some(why)) => Err(bad(why)),
        (Err(_), None) => Err(bad("The backup is not whole.")),
        (Ok(()), None) if !have_control => Err(bad("The backup has no control database.")),
        (Ok(()), None) => Ok(spaces),
    }
}

/// A database error from opening a staged copy: a newer release made it, or it is not ours at all.
fn opened(e: DbError) -> ApiError {
    match e {
        DbError::UnexpectedVersion { found, expected } if found > expected as i64 => ApiError::code(ErrorCode::BackupNewerRelease),
        DbError::UnexpectedVersion { .. } | DbError::Sqlite(_) => bad("The backup holds a database this server cannot read."),
        other => internal(other),
    }
}

/// Opens every staged file (migrating the copy to this release) and checks they agree with each other.
fn check(dir: &Path, spaces: &BTreeSet<String>) -> Result<Db, ApiError> {
    let control = Db::open(&dir.join("control.db")).map_err(opened)?;
    let listed: BTreeSet<String> = control.space_ids().map_err(internal)?.into_iter().collect();
    if &listed != spaces {
        return Err(bad("The backup's spaces and the control database's list of them do not agree."));
    }
    for id in spaces {
        SpaceStore::open(&dir.join("spaces").join(format!("{id}.db"))).map_err(opened)?;
    }
    Ok(control)
}

/// Makes the checked files this server's own.
fn swap(state: &AppState, dir: &Path, staged: &Db, spaces: &BTreeSet<String>) -> Result<(), ApiError> {
    let mut db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let before = db.space_ids().map_err(internal)?;
    db.replace_with(staged).map_err(internal)?;
    state.spaces.remove(&before).map_err(internal)?;
    if let Some(live) = state.spaces.dir() {
        std::fs::create_dir_all(live).map_err(internal)?;
        for id in spaces {
            let from = dir.join("spaces").join(format!("{id}.db"));
            // Rename: the work directory and the space files are both on the data volume. A migrated copy has its own `-wal`/`-shm`
            // only while open, and it has been closed by now.
            std::fs::rename(&from, live.join(format!("{id}.db"))).map_err(internal)?;
        }
    }
    state.spaces.open_all(&db).map_err(internal)?;
    Ok(())
}

pub async fn response(state: Arc<AppState>, body: axum::body::Body) -> Result<axum::response::Response, ApiError> {
    use tokio::io::AsyncWriteExt;
    // Before anything is read: a used install refuses at once.
    let st = state.clone();
    if !tokio::task::spawn_blocking(move || is_empty(&st)).await.map_err(internal)?.map_err(internal)? {
        return Err(ApiError::code(ErrorCode::NotEmpty));
    }
    let parent = state.spaces.scratch_dir().unwrap_or_else(std::env::temp_dir);
    let dir = parent.join(format!("{PREFIX}{}", crate::ids::new_id()));
    std::fs::create_dir_all(&dir).map_err(internal)?;
    let _guard = TempDir(dir.clone());
    let tar = dir.join("import.tar");
    let mut file = tokio::fs::File::create(&tar).await.map_err(internal)?;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| bad("The upload stopped before the end."))?;
        file.write_all(&chunk).await.map_err(internal)?;
    }
    file.sync_all().await.map_err(internal)?;
    drop(file);

    let st = state.clone();
    let spaces = tokio::task::spawn_blocking(move || -> Result<usize, ApiError> {
        let unpacked = dir.join("unpacked");
        let ids = unpack(&tar, &unpacked)?;
        let staged = check(&unpacked, &ids)?;
        // The install may have been used while the upload ran: look again, now that it is the only thing that will happen.
        if !is_empty(&st).map_err(internal)? {
            return Err(ApiError::code(ErrorCode::NotEmpty));
        }
        swap(&st, &unpacked, &staged, &ids)?;
        Ok(ids.len())
    })
    .await
    .map_err(internal)??;
    tracing::info!(spaces, "a backup was restored");
    axum::response::Response::builder()
        .status(200)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(serde_json::json!({ "spaces": spaces }).to_string()))
        .map_err(internal)
}
