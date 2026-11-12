//! The manual backup (ADR 0013): one `.tar` bundle of encrypted exports, made to the backup public key, that the person downloads.
//!
//! - **The admin cannot read what it bundles.** Each component encrypts its own export to the backup public key and gives it over its
//!   own authenticated link; the admin encrypts its own the same way, and holds neither the passphrase nor the private key. It only
//!   counts and hashes the bytes it passes on.
//! - **What is in the bundle.** `manifest.json` (in the clear: the release that made it, the key's fingerprint, and each export's
//!   SHA-256 and size), `backup-key.age` (the private key protected by the passphrase), then the exports: the admin's own first
//!   (`admin.age`) and the server's (`server.age`), then the alerter's (`alerter.age`: its ledger and release state) and `cpa`'s (`cpa.age`:
//!   the account login files). The auditor's, last, follows in a later step. With the file and the passphrase alone, `age` opens it.
//! - **The admin's own export** is `admin.db` (a `VACUUM INTO` copy, complete: it is encrypted), `admin-secrets/` (`admin.key` and the
//!   CLIProxyAPI keys it keeps), and `caddy-secrets/` (the install's own certificate and key).
//! - **Streamed.** Each export goes to a work file on the admin's data volume as it arrives (never `/tmp`, never memory), then the
//!   bundle is streamed from them with a known length. The work files are removed however the download ends.
//! - **Nothing without a key.** With no backup key set, nothing is made: `503 backup_key_missing`. A server export that ends early
//!   (the connection breaks, no bytes) is a failure, never a short bundle.

use crate::web::{err, Failure};
use crate::AdminState;
use askbar_proto::http::ApiError;
use askbar_proto::ErrorCode;
use askbar_service::backup::{self, Manifest, Part};
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::Response;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

type St = State<Arc<AdminState>>;

fn internal() -> Failure {
    err(StatusCode::INTERNAL_SERVER_ERROR, "The backup could not be made.")
}

/// Removes the work directory, whatever way the download ends.
struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Passes bytes on, counting and hashing them.
struct Hashing<W: Write> {
    inner: W,
    sha: Sha256,
    bytes: u64,
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.sha.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// An export staged in a work file.
struct Staged {
    name: &'static str,
    path: PathBuf,
    sha256: String,
    bytes: u64,
}

impl Staged {
    fn part(&self) -> Part {
        Part { name: self.name.to_string(), sha256: self.sha256.clone(), bytes: self.bytes }
    }
}

fn hex(d: impl AsRef<[u8]>) -> String {
    d.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// The admin's secret files its own export holds, by directory: the admin's secrets directory and Caddy's.
const ADMIN_SECRET_FILES: [&str; 3] = [crate::secrets::KEY_FILE, crate::cpa::API_KEY_FILE, crate::cpa::SECRET_FILE];
const CADDY_SECRET_FILES: [&str; 2] = [crate::self_cert::CERT_FILE, crate::self_cert::KEY_FILE];

/// The admin's own export, encrypted to `recipient` and written to `out`: `admin.db`, `admin-secrets/…` and `caddy-secrets/…`.
fn export_admin(
    db: tokio::sync::OwnedMutexGuard<crate::AdminDb>,
    work: &Path,
    recipient: &str,
    out: &Path,
    secrets: Option<PathBuf>,
    caddy: Option<PathBuf>,
    now: i64,
) -> Result<Staged, Box<dyn std::error::Error + Send + Sync>> {
    let copy = work.join("admin.db");
    db.conn().execute("VACUUM INTO ?1", [copy.to_string_lossy().as_ref()])?;
    drop(db);
    let sink = Hashing { inner: std::fs::File::create(out)?, sha: Sha256::new(), bytes: 0 };
    let mut tar = backup::encrypt_to(recipient, sink).map_err(|e| e.to_string())?;
    let size = std::fs::metadata(&copy)?.len();
    askbar_service::tar::write_file(&mut tar, "admin.db", size, &mut std::fs::File::open(&copy)?, now)?;
    std::fs::remove_file(&copy)?;
    for (dir, prefix, names) in [(secrets, "admin-secrets", &ADMIN_SECRET_FILES[..]), (caddy, "caddy-secrets", &CADDY_SECRET_FILES[..])] {
        let Some(dir) = dir else { continue };
        for name in names {
            let path = dir.join(name);
            if path.is_file() {
                let size = std::fs::metadata(&path)?.len();
                askbar_service::tar::write_file(&mut tar, &format!("{prefix}/{name}"), size, &mut std::fs::File::open(&path)?, now)?;
            }
        }
    }
    askbar_service::tar::write_end(&mut tar)?;
    let sink = tar.finish()?;
    sink.inner.sync_all()?;
    Ok(Staged { name: "admin.age", path: out.to_path_buf(), sha256: hex(sink.sha.finalize()), bytes: sink.bytes })
}

/// The server's export, read from its link into a work file as it arrives. Not the admin's to open: it is already encrypted.
async fn stage_server(state: &AdminState, out: &Path) -> Result<Staged, Failure> {
    use tokio::io::AsyncWriteExt;
    let link = state.server.as_ref().ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "No link to the server is set up."))?;
    let path = askbar_proto::paths::join(askbar_proto::paths::SERVER_V1, "/exports");
    let offline = || err(StatusCode::BAD_GATEWAY, "The server is offline. Nothing was backed up.");
    let mut resp = link.request_stream(reqwest::Method::POST, &path).await.map_err(|_| offline())?;
    let status = resp.status().as_u16();
    if status == 503 {
        let body = resp.bytes().await.unwrap_or_default();
        if serde_json::from_slice::<serde_json::Value>(&body).ok().and_then(|v| v["error"].as_str().map(String::from)).as_deref()
            == Some("backup_key_missing")
        {
            // The server has not been given the key yet (the push is in flight, or it failed).
            return Err(ApiError::new(
                ErrorCode::BackupKeyMissing,
                "The server has not been given the backup key yet. Try again in a moment.",
            ));
        }
        return Err(err(StatusCode::BAD_GATEWAY, "The server could not make its export."));
    }
    if status != 200 {
        return Err(err(StatusCode::BAD_GATEWAY, &format!("The server answered {status}.")));
    }
    let cut = || err(StatusCode::BAD_GATEWAY, "The server's export was cut short. Nothing was backed up.");
    let mut file = tokio::fs::File::create(out).await.map_err(|_| internal())?;
    let (mut sha, mut bytes) = (Sha256::new(), 0u64);
    while let Some(chunk) = resp.chunk().await.map_err(|_| cut())? {
        sha.update(&chunk);
        bytes += chunk.len() as u64;
        file.write_all(&chunk).await.map_err(|_| internal())?;
    }
    file.sync_all().await.map_err(|_| internal())?;
    if bytes == 0 {
        return Err(cut());
    }
    Ok(Staged { name: "server.age", path: out.to_path_buf(), sha256: hex(sha.finalize()), bytes })
}

/// Bytes already in memory (an export that is small, like the alerter's), written to a work file and hashed.
fn stage_bytes(name: &'static str, data: &[u8], out: &Path) -> Result<Staged, Failure> {
    std::fs::write(out, data).map_err(|_| internal())?;
    Ok(Staged { name, path: out.to_path_buf(), sha256: hex(Sha256::digest(data)), bytes: data.len() as u64 })
}

/// The alerter's export, over its link. Left out only where the install has no alerter link at all (a stack always has one).
async fn stage_alerter(state: &AdminState, out: &Path) -> Result<Option<Staged>, Failure> {
    let Some(link) = state.alerter.get() else { return Ok(None) };
    let bytes = link.export().await?;
    stage_bytes("alerter.age", &bytes, out).map(Some)
}

/// `cpa`'s export: the account login files, which its supervisor encrypts to the key on request (ADR 0037). Left out only where
/// CLIProxyAPI is not managed here.
async fn stage_cpa(state: &AdminState, recipient: &str, out: &Path) -> Result<Option<Staged>, Failure> {
    let link = state.cpa.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(link) = link else { return Ok(None) };
    let bytes = crate::cpa::export_logins(link.config_dir(), recipient, std::time::Duration::from_secs(60))
        .await
        .map_err(|why| err(StatusCode::BAD_GATEWAY, &format!("{why} Nothing was backed up.")))?;
    stage_bytes("cpa.age", &bytes, out).map(Some)
}

/// The size of a tar entry for `size` bytes of data: its header and the data, padded to a block.
fn entry_len(size: u64) -> u64 {
    512 + size.div_ceil(512) * 512
}

/// `GET /api/admin/v1/backup`: the encrypted bundle, downloaded. Recorded like every backup (ADR 0010).
pub async fn download(State(state): St, notes: Option<axum::Extension<crate::actions::ActionNotes>>) -> Result<Response, Failure> {
    let key = state
        .db
        .lock()
        .await
        .conn()
        .query_row("SELECT recipient, fingerprint, protected FROM backup_keys WHERE retired_at IS NULL", [], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Vec<u8>>(2)?))
        })
        .ok();
    let Some((recipient, fingerprint, protected)) = key else {
        return Err(ApiError::new(
            ErrorCode::BackupKeyMissing,
            "Set a backup passphrase first (the Backup page): nothing is backed up without one.",
        ));
    };
    // Work files on the admin's data volume, next to admin.db (the volume is the admin's own and the root is read-only, ADR 0032).
    let parent = {
        let db = state.db.lock().await;
        let at = db.conn().path().map(|p| p.to_string()).unwrap_or_default();
        Path::new(&at)
            .parent()
            .filter(|p| !at.is_empty() && !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir)
    };
    let work = parent.join(format!(".backup-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&work).map_err(|_| internal())?;
    let guard = TempDir(work.clone());
    let now = crate::auth::now();

    let admin_part = {
        let db = state.db.clone().lock_owned().await;
        let secrets = state.admin_secrets_dir.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let caddy = state.caddy.lock().unwrap_or_else(|e| e.into_inner()).secrets_dir.clone();
        let (w, rec, out) = (work.clone(), recipient.clone(), work.join("admin.age"));
        tokio::task::spawn_blocking(move || export_admin(db, &w, &rec, &out, secrets, caddy, now))
            .await
            .map_err(|_| internal())?
            .map_err(|_| internal())?
    };
    let server_part = stage_server(&state, &work.join("server.age")).await?;

    // The admin's first and the auditor's last (a later step); the others in between.
    let mut parts = vec![admin_part, server_part];
    parts.extend(stage_alerter(&state, &work.join("alerter.age")).await?);
    parts.extend(stage_cpa(&state, &recipient, &work.join("cpa.age")).await?);
    let manifest = Manifest {
        format: backup::BUNDLE_FORMAT,
        release: env!("CARGO_PKG_VERSION").to_string(),
        created_at: now,
        key_fingerprint: fingerprint.clone(),
        parts: parts.iter().map(Staged::part).collect(),
    };
    let manifest_json = serde_json::to_vec_pretty(&manifest).map_err(|_| internal())?;
    let length = entry_len(manifest_json.len() as u64)
        + entry_len(protected.len() as u64)
        + parts.iter().map(|p| entry_len(p.bytes)).sum::<u64>()
        + 1024;

    if let Some(axum::Extension(n)) = &notes {
        n.add("fingerprint", serde_json::json!(fingerprint));
        n.add("parts", serde_json::json!(manifest.parts.iter().map(|p| p.name.clone()).collect::<Vec<_>>()));
    }

    let (mut writer, body) = askbar_service::stream::body();
    tokio::task::spawn_blocking(move || {
        let _guard = guard; // the work files go when the bundle has been written, or the client has gone
        let result = (|| -> std::io::Result<()> {
            askbar_service::tar::write_bytes(&mut writer, backup::MANIFEST_FILE, &manifest_json, now)?;
            askbar_service::tar::write_bytes(&mut writer, backup::KEY_FILE, &protected, now)?;
            for p in &parts {
                askbar_service::tar::write_file(&mut writer, p.name, p.bytes, &mut std::fs::File::open(&p.path)?, now)?;
            }
            askbar_service::tar::write_end(&mut writer)?;
            writer.flush()
        })();
        if result.is_err() {
            writer.fail();
        }
    });
    Response::builder()
        .header(header::CONTENT_TYPE, "application/x-tar")
        .header(header::CONTENT_LENGTH, length)
        .header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"askbar-backup-{now}.tar\""))
        .body(body)
        .map_err(|_| internal())
}
