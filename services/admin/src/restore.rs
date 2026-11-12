//! Restoring a backup into an empty install (ADR 0013, D21): the admin opens the bundle with the owner's passphrase, in memory, and gives each
//! component its own part.
//!
//! - **Two steps.** `POST /restore/inspect` takes the bundle (a stream, kept as ciphertext in a work directory on the admin's data volume),
//!   checks its manifest and the hash of every part, and refuses a backup from a newer release (`409 backup_newer_release`, naming the
//!   release to install) *before the passphrase is asked for*. `POST /restore` takes the passphrase and does the work.
//! - **A fresh sign-in, never a key.** Both are a person's own session, signed in within ten minutes (D21).
//! - **Only into an empty, unconfigured install**: no provider, no person, no API key and no audit key here, and each component says
//!   `409 not_empty` if it has data. A used install is refused before anything is read.
//! - **In memory.** The passphrase opens the protected private key, which lives only for this call. Each part is decrypted as it is read and
//!   handed on; nothing is written decrypted except what a component keeps itself and the admin's own pending set.
//! - **Everything is checked first.** Before any component is touched, every part is decrypted to the end and read as a tar, so a damaged
//!   bundle or a wrong key fails with nothing changed. What can still fail after that is a component that is not there.
//! - **Guesses are limited** like the audit passphrase's (`audit_key::Limiter`): five wrong in fifteen minutes locks it for fifteen.
//! - **The admin restores itself last**: its decrypted export goes to a pending directory next to `admin.db`, then the admin restarts and
//!   [`apply_pending`] swaps it in before the database is opened. The old database and secrets are kept beside as `*.before-restore`.
//!
//! The auditor's part (`audit.age`) is checked but not restored here: the auditor takes it through a link of its own (plan step 6.3, the next
//! step), and the answer says so.

use crate::actions::ActionNotes;
use crate::web::{err, Failure};
use crate::AdminState;
use askbar_proto::http::{ApiError, ApiJson};
use askbar_proto::ErrorCode;
use askbar_service::backup::{self, Manifest};
use askbar_service::tar::{self, Entry};
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

type St = State<Arc<AdminState>>;

/// A bundle uploaded and checked is kept this long for the passphrase to come.
const STAGED_FOR_SECS: i64 = 30 * 60;
/// What `manifest.json` and `backup-key.age` may weigh: both are small, and a restore reads them into memory.
const MAX_SMALL: u64 = 1024 * 1024;
/// The parts the admin hands on in memory (the rest are streamed): the alerter's and `cpa`'s hold a few small files.
const MAX_IN_MEMORY: u64 = 256 * 1024 * 1024;
/// The directory next to `admin.db` that holds the decrypted admin export until the restart.
pub const PENDING_DIR: &str = ".restore-pending";
/// The file that says the pending set is whole (written last).
const READY: &str = "READY";
/// Left by the swap for `main` to report, then removed: a restore happened.
pub const DONE_MARKER: &str = ".restore-done";

const ADMIN_SECRETS: [&str; 3] = [crate::secrets::KEY_FILE, crate::cpa::API_KEY_FILE, crate::cpa::SECRET_FILE];
const CADDY_SECRETS: [&str; 2] = [crate::self_cert::CERT_FILE, crate::self_cert::KEY_FILE];

/// A bundle uploaded and read: where it is and what it says.
struct Staged {
    id: String,
    dir: PathBuf,
    bundle: PathBuf,
    entries: Vec<Entry>,
    manifest: Manifest,
    protected: Vec<u8>,
    at: i64,
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// What the admin holds for a restore.
#[derive(Default)]
pub struct Restore {
    staged: Mutex<Option<Staged>>,
    /// Limits guesses at the backup passphrase.
    pub limiter: crate::audit_key::Limiter,
    busy: AtomicBool,
}

fn internal<E: std::fmt::Display>(e: E) -> Failure {
    tracing::warn!("a restore step failed: {e}");
    err(StatusCode::INTERNAL_SERVER_ERROR, "The restore could not be done.")
}

fn bad(msg: &str) -> Failure {
    err(StatusCode::BAD_REQUEST, msg)
}

/// `1.2.3` as numbers; anything else is not a release.
fn version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    let (a, b, c) = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some((a, b, c))
}

/// Whether this install has nothing in it yet: no provider, no person, no API key and no audit key (the wizard's first screens come
/// before any of them). A restore replaces everything the admin holds, so it is for the start of an install only.
async fn is_empty(state: &AdminState) -> Result<bool, Failure> {
    let db = state.db.lock().await;
    let count = |sql: &str| db.conn().query_row(sql, [], |r| r.get::<_, i64>(0)).map_err(internal);
    Ok(count("SELECT COUNT(*) FROM providers")? == 0
        && count("SELECT COUNT(*) FROM users")? == 0
        && count("SELECT COUNT(*) FROM admin_api_keys WHERE revoked_at IS NULL")? == 0
        && db.active_audit_key().map_err(internal)?.is_none())
}

fn hex(d: impl AsRef<[u8]>) -> String {
    d.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// The data of `entry`, read from the bundle on disk.
fn part_reader(bundle: &Path, entry: &Entry) -> std::io::Result<impl Read> {
    let mut f = std::fs::File::open(bundle)?;
    f.seek(SeekFrom::Start(entry.offset))?;
    Ok(f.take(entry.size))
}

/// The SHA-256 of `entry`'s data.
fn hash_of(bundle: &Path, entry: &Entry) -> std::io::Result<String> {
    let mut sha = Sha256::new();
    let mut r = part_reader(bundle, entry)?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            return Ok(hex(sha.finalize()));
        }
        sha.update(&buf[..n]);
    }
}

/// Reads an uploaded bundle: its entries, its manifest and its protected key. Refuses a newer release first, then a bundle that is not
/// whole (a part that is missing, or whose hash is not the manifest's).
fn read_bundle(dir: PathBuf, bundle: PathBuf, now: i64) -> Result<Staged, Failure> {
    let mut file = std::fs::File::open(&bundle).map_err(internal)?;
    let entries = tar::index(&mut file).map_err(|_| bad("That is not a backup this version of Askbar can read, or it is not whole."))?;
    let small = |name: &str| -> Result<Vec<u8>, Failure> {
        let e = entries.iter().find(|e| e.name == name).ok_or_else(|| bad("That is not an Askbar backup: it has no manifest."))?;
        if e.size > MAX_SMALL {
            return Err(bad("That is not an Askbar backup."));
        }
        let mut v = Vec::new();
        part_reader(&bundle, e).and_then(|mut r| r.read_to_end(&mut v)).map_err(internal)?;
        Ok(v)
    };
    let manifest: Manifest = serde_json::from_slice(&small(backup::MANIFEST_FILE)?)
        .map_err(|_| bad("That is not an Askbar backup: its manifest cannot be read."))?;
    // The release comes first: a backup from a newer release is refused before anything else is asked of it, naming the release.
    let newer = manifest.format > backup::BUNDLE_FORMAT
        || match (version(&manifest.release), version(env!("CARGO_PKG_VERSION"))) {
            (Some(theirs), Some(ours)) => theirs > ours,
            _ => false,
        };
    if newer {
        return Err(ApiError::new(
            ErrorCode::BackupNewerRelease,
            format!("This backup was made by Askbar {}. Install that release (or a later one), then restore it.", manifest.release),
        )
        .with("release", manifest.release.clone()));
    }
    let protected = small(backup::KEY_FILE)?;
    for p in &manifest.parts {
        let e = entries.iter().find(|e| e.name == p.name).ok_or_else(|| bad("The backup is missing a part its manifest lists."))?;
        if e.size != p.bytes || hash_of(&bundle, e).map_err(internal)? != p.sha256 {
            return Err(bad("The backup is damaged: a part is not the one its manifest describes."));
        }
    }
    if !manifest.parts.iter().any(|p| p.name == "admin.age") {
        return Err(bad("The backup has no part for the admin."));
    }
    Ok(Staged { id: ulid::Ulid::new().to_string(), dir, bundle, entries, manifest, protected, at: now })
}

/// `POST /api/admin/v1/restore/inspect`: take the bundle (the body, a stream) and say what it is.
pub async fn inspect(
    State(state): St,
    headers: HeaderMap,
    notes: Option<Extension<ActionNotes>>,
    body: Body,
) -> Result<Json<Value>, Failure> {
    use tokio::io::AsyncWriteExt;
    crate::web::require_fresh_signin(&state, &headers)?;
    if !is_empty(&state).await? {
        return Err(ApiError::code(ErrorCode::NotEmpty));
    }
    let dir = crate::backup_bundle::work_parent(&state).await.join(format!(".restore-upload-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).map_err(internal)?;
    // Whatever way this ends, a directory that was not kept goes (a kept one removes itself when it is replaced or used).
    struct Cleanup(Option<PathBuf>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Some(d) = self.0.take() {
                let _ = std::fs::remove_dir_all(d);
            }
        }
    }
    let mut cleanup = Cleanup(Some(dir.clone()));
    let bundle = dir.join("bundle.tar");
    let mut file = tokio::fs::File::create(&bundle).await.map_err(internal)?;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| bad("The upload stopped before the end."))?;
        file.write_all(&chunk).await.map_err(internal)?;
    }
    file.sync_all().await.map_err(internal)?;
    drop(file);
    let (d, b, now) = (dir.clone(), bundle.clone(), crate::auth::now());
    let staged = tokio::task::spawn_blocking(move || read_bundle(d, b, now)).await.map_err(internal)??;
    cleanup.0 = None; // the Staged owns the directory now
    let view = json!({
        "restore_id": staged.id,
        "release": staged.manifest.release,
        "created_at": staged.manifest.created_at,
        "key_fingerprint": staged.manifest.key_fingerprint,
        "parts": staged.manifest.parts.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
        "valid_for_secs": STAGED_FOR_SECS,
    });
    if let Some(Extension(n)) = &notes {
        n.add("release", json!(staged.manifest.release));
    }
    *state.restore.staged.lock().unwrap_or_else(|e| e.into_inner()) = Some(staged);
    Ok(Json(view))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreInput {
    pub restore_id: String,
    pub passphrase: String,
}

impl std::fmt::Debug for RestoreInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestoreInput").field("restore_id", &self.restore_id).finish_non_exhaustive()
    }
}

/// What a restore needs from the staged bundle, copied out so the lock is not held across the work.
struct Job {
    bundle: PathBuf,
    entries: Vec<Entry>,
    parts: Vec<String>,
    protected: Vec<u8>,
    fingerprint: String,
}

fn take_job(state: &AdminState, id: &str) -> Result<Job, Failure> {
    let guard = state.restore.staged.lock().unwrap_or_else(|e| e.into_inner());
    let s = guard.as_ref().filter(|s| s.id == id && crate::auth::now() - s.at <= STAGED_FOR_SECS);
    let s = s.ok_or_else(|| err(StatusCode::CONFLICT, "Upload the backup again: the one checked earlier is gone or has expired."))?;
    Ok(Job {
        bundle: s.bundle.clone(),
        entries: s.entries.clone(),
        parts: s.manifest.parts.iter().map(|p| p.name.clone()).collect(),
        protected: s.protected.clone(),
        fingerprint: s.manifest.key_fingerprint.clone(),
    })
}

impl Job {
    fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// A reader of the part `name`, decrypted as it is read.
    fn open(&self, identity: &Arc<backup::Identity>, name: &str) -> Result<Box<dyn Read + Send>, Failure> {
        let entry = self.entry(name).ok_or_else(|| bad("The backup is missing a part."))?;
        let raw = part_reader(&self.bundle, entry).map_err(internal)?;
        Ok(Box::new(backup::decrypt_stream(identity, raw).map_err(|_| bad("The backup passphrase opens the key, but not this backup."))?))
    }

    /// The part `name`, decrypted into memory.
    fn open_all(&self, identity: &Arc<backup::Identity>, name: &str) -> Result<Vec<u8>, Failure> {
        let mut out = Vec::new();
        self.open(identity, name)?.take(MAX_IN_MEMORY + 1).read_to_end(&mut out).map_err(|_| bad("The backup is damaged."))?;
        if out.len() as u64 > MAX_IN_MEMORY {
            return Err(bad("A part of the backup is larger than a restore takes in memory."));
        }
        Ok(out)
    }

    /// Reads the part to the end as a tar: it decrypts, and it is whole.
    fn verify(&self, identity: &Arc<backup::Identity>, name: &str) -> Result<(), Failure> {
        let mut r = self.open(identity, name)?;
        tar::for_each(&mut r, |_, _, data| std::io::copy(data, &mut std::io::sink()).map(|_| ()))
            .map_err(|_| bad("The backup is damaged: a part does not open to the end."))
    }
}

/// A body for a request to the server, fed by decrypting the part on a blocking thread as the request is sent.
fn streamed(job: &Job, identity: &Arc<backup::Identity>, name: &str) -> Result<reqwest::Body, Failure> {
    let mut reader = job.open(identity, name)?;
    let (mut writer, body) = askbar_service::stream::body();
    tokio::task::spawn_blocking(move || {
        let done = std::io::copy(&mut reader, &mut writer).and_then(|_| writer.flush());
        if done.is_err() {
            writer.fail(); // the request ends badly, so the server takes nothing
        }
    });
    Ok(reqwest::Body::wrap_stream(body.into_data_stream()))
}

/// Writes the decrypted admin export to the pending directory next to `admin.db`: `admin.db` (opened once, so a database from a newer
/// release is refused here, not after the restart) and the secret files, then `READY`.
fn write_pending(parent: &Path, mut export: impl Read) -> Result<(), Failure> {
    let pending = parent.join(PENDING_DIR);
    let _ = std::fs::remove_dir_all(&pending);
    let result = (|| -> Result<(), Failure> {
        std::fs::create_dir_all(pending.join("admin-secrets")).map_err(internal)?;
        std::fs::create_dir_all(pending.join("caddy-secrets")).map_err(internal)?;
        let mut seen_db = false;
        let mut refused: Option<&'static str> = None;
        let read = tar::for_each(&mut export, |name, _, data| {
            let target = match name.split_once('/') {
                None if name == "admin.db" => {
                    seen_db = true;
                    pending.join("admin.db")
                }
                Some(("admin-secrets", f)) if ADMIN_SECRETS.contains(&f) => pending.join("admin-secrets").join(f),
                Some(("caddy-secrets", f)) if CADDY_SECRETS.contains(&f) => pending.join("caddy-secrets").join(f),
                _ => {
                    refused = Some("The backup holds a file the admin did not make.");
                    return Err(std::io::Error::other("refused"));
                }
            };
            let mut out = std::fs::File::create(target)?;
            std::io::copy(data, &mut out)?;
            out.sync_all()
        });
        if let Some(why) = refused {
            return Err(bad(why));
        }
        read.map_err(|_| bad("The backup is damaged: the admin's part does not open to the end."))?;
        if !seen_db {
            return Err(bad("The admin's part of the backup has no database."));
        }
        match crate::db::AdminDb::open(&pending.join("admin.db")) {
            Ok(db) => drop(db),
            Err(crate::db::DbError::UnexpectedVersion { found, expected }) if found > expected as i64 => {
                return Err(ApiError::code(ErrorCode::BackupNewerRelease))
            }
            Err(_) => return Err(bad("The admin's database in the backup cannot be read.")),
        }
        // `admin.db` is consolidated on close; the write-ahead files of the check go.
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(pending.join(format!("admin.db{suffix}")));
        }
        std::fs::write(pending.join(READY), b"").map_err(internal)
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&pending);
    }
    result
}

/// `POST /api/admin/v1/restore`: open the staged bundle with the passphrase and restore it.
pub async fn restore(
    State(state): St,
    headers: HeaderMap,
    notes: Option<Extension<ActionNotes>>,
    ApiJson(input): ApiJson<RestoreInput>,
) -> Result<Json<Value>, Failure> {
    crate::web::require_fresh_signin(&state, &headers)?;
    let now = crate::auth::now();
    if let Some(secs) = state.restore.limiter.locked_for(now) {
        return Err(ApiError::code(ErrorCode::TooManyAttempts).retry_after(secs as u64));
    }
    if !is_empty(&state).await? {
        return Err(ApiError::code(ErrorCode::NotEmpty));
    }
    if state.restore.busy.swap(true, Ordering::SeqCst) {
        return Err(err(StatusCode::CONFLICT, "A restore is already running."));
    }
    struct Busy<'a>(&'a AtomicBool);
    impl Drop for Busy<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _busy = Busy(&state.restore.busy);
    let job = take_job(&state, &input.restore_id)?;

    // The passphrase opens the protected key (about a second of scrypt), here and nowhere else; the key goes when this call ends.
    let (protected, passphrase) = (job.protected.clone(), input.passphrase);
    let identity = match tokio::task::spawn_blocking(move || backup::unlock(&protected, &passphrase)).await.map_err(internal)? {
        Ok(i) => Arc::new(i),
        Err(backup::Error::WrongPassphrase) => {
            state.restore.limiter.record_wrong(now);
            return Err(err(StatusCode::FORBIDDEN, "The backup passphrase is wrong."));
        }
        Err(_) => return Err(bad("The key in that backup cannot be opened.")),
    };
    state.restore.limiter.record_right();
    if backup::fingerprint(&identity.to_public().to_string()) != job.fingerprint {
        return Err(bad("The key in the backup is not the one its manifest names."));
    }
    let job = Arc::new(job);

    // Everything is read to the end first, so a damaged bundle fails with nothing changed.
    for name in &job.parts {
        let (j, id, n) = (job.clone(), identity.clone(), name.clone());
        tokio::task::spawn_blocking(move || j.verify(&id, &n)).await.map_err(internal)??;
    }

    let mut restored: Vec<&str> = Vec::new();
    // A step that fails after others worked says which did, so the person knows what state the install is in.
    let stopped = |restored: &[&str], e: Failure| -> Failure {
        tracing::warn!("a restore stopped after {restored:?}");
        e.with("restored", json!(restored))
    };

    // The server: the plain tar, streamed as it is decrypted.
    if job.entry("server.age").is_some() {
        let link = state.server.as_ref().ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "No link to the server is set up."))?;
        let body = streamed(&job, &identity, "server.age")?;
        let path = askbar_proto::paths::join(askbar_proto::paths::SERVER_V1, "/imports");
        let resp = link
            .post_stream(&path, body)
            .await
            .map_err(|_| stopped(&restored, err(StatusCode::BAD_GATEWAY, "The server is offline. Nothing was restored.")))?;
        match resp.status().as_u16() {
            200 => restored.push("server"),
            409 => {
                let code = resp.json::<Value>().await.ok().and_then(|v| v["error"].as_str().map(String::from));
                return Err(match code.as_deref() {
                    Some("backup_newer_release") => ApiError::code(ErrorCode::BackupNewerRelease),
                    _ => ApiError::code(ErrorCode::NotEmpty),
                });
            }
            _ => return Err(stopped(&restored, err(StatusCode::BAD_GATEWAY, "The server did not take its part of the backup."))),
        }
    }
    // The alerter's ledger and release state.
    if job.entry("alerter.age").is_some() {
        if let Some(link) = state.alerter.get() {
            let (j, id) = (job.clone(), identity.clone());
            let tar = tokio::task::spawn_blocking(move || j.open_all(&id, "alerter.age")).await.map_err(internal)??;
            link.import(tar).await.map_err(|e| stopped(&restored, e))?;
            restored.push("alerter");
        }
    }
    // `cpa`'s account logins, through its supervisor.
    if job.entry("cpa.age").is_some() {
        let link = state.cpa.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(link) = link {
            let (j, id) = (job.clone(), identity.clone());
            let tar = tokio::task::spawn_blocking(move || j.open_all(&id, "cpa.age")).await.map_err(internal)??;
            crate::cpa::import_logins(link.config_dir(), &tar, std::time::Duration::from_secs(60))
                .await
                .map_err(|why| stopped(&restored, err(StatusCode::BAD_GATEWAY, why)))?;
            restored.push("cpa");
        }
    }
    // The admin last: its own export becomes a pending set, and the swap happens at the restart.
    let parent = crate::backup_bundle::work_parent(&state).await;
    {
        let (j, id) = (job.clone(), identity.clone());
        tokio::task::spawn_blocking(move || {
            let reader = j.open(&id, "admin.age")?;
            write_pending(&parent, reader)
        })
        .await
        .map_err(internal)?
        .map_err(|e| stopped(&restored, e))?;
    }
    restored.push("admin");
    // The staged bundle has done its work.
    *state.restore.staged.lock().unwrap_or_else(|e| e.into_inner()) = None;
    if let Some(Extension(n)) = &notes {
        n.add("restored", json!(restored));
    }
    // Restart so the pending set is swapped in before the database is opened. Without a restarter (tests) there is nothing to restart.
    let restarting = match state.restarter.get().cloned() {
        Some(r) => askbar_service::restart::handler(State(r)).await.is_ok(),
        None => false,
    };
    Ok(Json(json!({
        "restored": restored,
        // The auditor's store is restored through a link of its own (the next step of plan step 6.3).
        "not_restored": job.parts.iter().filter(|p| p.as_str() == "audit.age").map(|_| "audit").collect::<Vec<_>>(),
        "restarting": restarting,
    })))
}

/// At the start of the admin, before its database is opened: if a restore left a whole pending set, put it in place. `db` is
/// `admin.db`; `secrets` and `caddy_secrets` are the two secret directories. The old database and secrets stay as `*.before-restore`.
/// Returns whether a restore was applied (and leaves [`DONE_MARKER`] for `main` to report). A pending set that is not whole is removed.
pub fn apply_pending(db: &Path, secrets: &Path, caddy_secrets: Option<&Path>) -> std::io::Result<bool> {
    let Some(parent) = db.parent() else { return Ok(false) };
    let pending = parent.join(PENDING_DIR);
    if !pending.is_dir() {
        return Ok(false);
    }
    if !pending.join(READY).is_file() {
        std::fs::remove_dir_all(&pending)?;
        return Ok(false);
    }
    let keep = |p: &Path| -> std::io::Result<()> {
        if p.exists() {
            let mut name = p.file_name().unwrap_or_default().to_os_string();
            name.push(".before-restore");
            std::fs::rename(p, p.with_file_name(name))?;
        }
        Ok(())
    };
    // The secrets first, so a crash between the steps leaves the old database with the old key, or the new with the new.
    for (dir, names, from) in [
        (Some(secrets), &ADMIN_SECRETS[..], pending.join("admin-secrets")),
        (caddy_secrets, &CADDY_SECRETS[..], pending.join("caddy-secrets")),
    ] {
        let Some(dir) = dir else { continue };
        std::fs::create_dir_all(dir)?;
        for name in names {
            let src = from.join(name);
            if src.is_file() {
                keep(&dir.join(name))?;
                std::fs::copy(&src, dir.join(name))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(0o600))?;
                }
            }
        }
    }
    keep(db)?;
    for suffix in ["-wal", "-shm"] {
        let mut old = db.as_os_str().to_os_string();
        old.push(suffix);
        let _ = std::fs::remove_file(old);
    }
    std::fs::rename(pending.join("admin.db"), db)?;
    std::fs::remove_dir_all(&pending)?;
    std::fs::write(parent.join(DONE_MARKER), crate::auth::now().to_string())?;
    Ok(true)
}

/// Whether a restore was applied at the last start (and forgets it): `main` sends `restore_performed` once.
pub fn take_done_marker(db: &Path) -> bool {
    db.parent().map(|p| p.join(DONE_MARKER)).is_some_and(|m| std::fs::remove_file(m).is_ok())
}
