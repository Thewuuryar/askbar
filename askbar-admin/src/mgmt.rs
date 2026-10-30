//! The admin service's side of the management API: the token, the snapshot it builds from
//! `admin.db`, and the pusher that keeps the server's copy current.
//!
//! The raw token lives only in the admin's own secrets directory (mode 0600). The server is
//! given only its SHA-256, in a separate directory it mounts read-only, so it can check a
//! caller but can never act as one.

use crate::db::AdminDb;
use askbar_proto::{Budget, DisplaySettings, Snapshot, SnapshotPairCode, SnapshotProvider};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chacha20poly1305::aead::{rand_core::RngCore, OsRng};
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

pub const TOKEN_FILE: &str = "mgmt.token";
pub const HASH_FILE: &str = "token.sha256";

/// Where the raw token is kept: `ASKBAR_ADMIN_SECRETS_DIR`, default `/admin-secrets`.
pub fn secrets_dir() -> PathBuf {
    std::env::var("ASKBAR_ADMIN_SECRETS_DIR").ok().filter(|p| !p.trim().is_empty()).map(PathBuf::from).unwrap_or_else(|| "/admin-secrets".into())
}

/// Where the token's hash is written for the server: `ASKBAR_MGMT_DIR`, default `/mgmt`.
pub fn mgmt_dir() -> PathBuf {
    std::env::var("ASKBAR_MGMT_DIR").ok().filter(|p| !p.trim().is_empty()).map(PathBuf::from).unwrap_or_else(|| "/mgmt".into())
}

/// The server's management API: `ASKBAR_SERVER_MGMT_URL`, default `http://server:8082`.
pub fn server_url() -> String {
    std::env::var("ASKBAR_SERVER_MGMT_URL").ok().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| "http://server:8082".into())
}

pub fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn io_err(path: &Path, e: std::io::Error) -> std::io::Error {
    std::io::Error::new(e.kind(), format!("{}: {e}", path.display()))
}

/// Write `contents` to `path` through a temporary file, so a reader never sees half of it.
fn write_atomic(path: &Path, contents: &str, mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp).map_err(|e| io_err(&tmp, e))?;
    f.write_all(contents.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| io_err(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| io_err(path, e))
}

/// Make sure the token exists (created once, 32 random bytes, mode 0600, never overwritten)
/// and that the server's hash file matches it. Returns the raw token.
pub fn ensure_token(secrets_dir: &Path, mgmt_dir: &Path) -> std::io::Result<String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    std::fs::create_dir_all(secrets_dir).map_err(|e| io_err(secrets_dir, e))?;
    std::fs::set_permissions(secrets_dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| io_err(secrets_dir, e))?;
    let path = secrets_dir.join(TOKEN_FILE);
    let token = match std::fs::read_to_string(&path) {
        Ok(t) if !t.trim().is_empty() => t.trim().to_string(),
        Ok(_) => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{} is empty; delete it to make a new token", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut raw = [0u8; 32];
            OsRng.fill_bytes(&mut raw);
            let token = format!("mgt_{}", URL_SAFE_NO_PAD.encode(raw));
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).map_err(|e| io_err(&path, e))?;
            std::io::Write::write_all(&mut f, token.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| io_err(&path, e))?;
            token
        }
        Err(e) => return Err(io_err(&path, e)),
    };
    std::fs::create_dir_all(mgmt_dir).map_err(|e| io_err(mgmt_dir, e))?;
    let hash_path = mgmt_dir.join(HASH_FILE);
    let hash = sha256_hex(&token);
    if std::fs::read_to_string(&hash_path).map(|h| h.trim() != hash).unwrap_or(true) {
        // Readable by the server's user, which is not ours.
        write_atomic(&hash_path, &format!("{hash}\n"), 0o644)?;
    }
    Ok(token)
}

// ------------------------------------------------------------------ the snapshot

fn setting(db: &AdminDb, key: &str) -> rusqlite::Result<Option<String>> {
    db.conn().query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()
}

/// The snapshot of everything in `admin.db` the server needs. `user_id` is the one user a
/// pairing code belongs to; the admin keeps codes per user already.
pub fn build_snapshot(db: &AdminDb) -> rusqlite::Result<Snapshot> {
    let mut providers = Vec::new();
    {
        let mut stmt = db.conn().prepare("SELECT id, name, type, base_url, model, max_tokens, temperature, headers FROM providers ORDER BY name")?;
        let rows = stmt.query_map([], |r| {
            Ok(SnapshotProvider {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: r.get(2)?,
                base_url: r.get(3)?,
                model: r.get(4)?,
                max_tokens: r.get(5)?,
                temperature: r.get(6)?,
                headers: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or_default(),
            })
        })?;
        for p in rows {
            providers.push(p?);
        }
    }
    let known = |id: &String| providers.iter().any(|p| &p.id == id);
    let default_provider_id = setting(db, "default_provider_id")?.filter(known);
    let fallback_order: Vec<String> =
        setting(db, "fallback_order")?.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()).unwrap_or_default().into_iter().filter(known).collect();
    let display: DisplaySettings = setting(db, "display")?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let mut user_display_defaults = std::collections::BTreeMap::new();
    {
        let mut stmt = db.conn().prepare("SELECT user_id, value FROM display_defaults")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (user, value) = row?;
            if let Ok(d) = serde_json::from_str(&value) {
                user_display_defaults.insert(user, d);
            }
        }
    }
    let mut pair_codes = Vec::new();
    {
        let mut stmt = db.conn().prepare("SELECT hash, user_id, device_name, expires_at FROM pair_codes WHERE expires_at > ?1 ORDER BY created_at")?;
        for c in stmt.query_map([crate::auth::now()], |r| {
            Ok(SnapshotPairCode { hash: r.get(0)?, user_id: r.get(1)?, device_name: r.get(2)?, expires_at: r.get(3)? })
        })? {
            pair_codes.push(c?);
        }
    }
    let revoked_device_ids = {
        let mut stmt = db.conn().prepare("SELECT device_id FROM revoked_devices ORDER BY device_id")?;
        let ids = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        ids
    };
    let (daily, monthly, kill): (Option<i64>, Option<i64>, i64) =
        db.conn().query_row("SELECT daily_tokens, monthly_tokens, kill_switch FROM budgets WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(Snapshot {
        version: 0,
        providers,
        default_provider_id,
        fallback_order,
        system_prompt: setting(db, "system_prompt")?,
        display,
        user_display_defaults,
        pair_codes,
        revoked_device_ids,
        budget: Budget { daily_tokens: daily.map(|v| v as u64), monthly_tokens: monthly.map(|v| v as u64) },
        kill_switch: kill != 0,
        audit_retention_days: setting(db, "audit_retention_days")?.and_then(|s| s.parse().ok()).unwrap_or(30),
    })
}

// ------------------------------------------------------------------- the pusher

#[derive(Debug, thiserror::Error)]
pub enum PushError {
    #[error("the server could not be reached: {0}")]
    Unreachable(String),
    #[error("the server refused the snapshot ({status}): {body}")]
    Refused { status: u16, body: String },
    #[error("could not build the snapshot: {0}")]
    Build(String),
}

/// Talks to the server's management API and remembers whether it is there.
pub struct ServerLink {
    http: reqwest::Client,
    base: String,
    token: String,
    online: AtomicBool,
    last_version: AtomicU64,
    last_error: Mutex<Option<String>>,
    dirty: AtomicBool,
    wake: Notify,
}

impl ServerLink {
    pub fn new(base: &str, token: String) -> Arc<ServerLink> {
        Arc::new(ServerLink {
            http: reqwest::Client::builder().connect_timeout(Duration::from_secs(3)).timeout(Duration::from_secs(15)).build().expect("a plain HTTP client builds"),
            base: base.trim_end_matches('/').to_string(),
            token,
            online: AtomicBool::new(false),
            last_version: AtomicU64::new(0),
            last_error: Mutex::new(None),
            // A fresh admin has never pushed, so it starts out needing to.
            dirty: AtomicBool::new(true),
            wake: Notify::new(),
        })
    }

    /// True if the last contact with the server worked.
    pub fn server_online(&self) -> bool {
        self.online.load(Ordering::Relaxed)
    }

    /// The version the server assigned to the last snapshot it accepted (0 if none yet).
    pub fn last_version(&self) -> u64 {
        self.last_version.load(Ordering::Relaxed)
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// True while a change has not reached the server yet.
    pub fn pending(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    /// Something changed in `admin.db`: push it as soon as possible.
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
        self.wake.notify_one();
    }

    fn note(&self, result: &Result<u64, PushError>) {
        match result {
            Ok(v) => {
                self.online.store(true, Ordering::Relaxed);
                self.last_version.store(*v, Ordering::Relaxed);
                *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
            Err(PushError::Unreachable(m)) => {
                self.online.store(false, Ordering::Relaxed);
                *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(m.clone());
            }
            Err(e) => {
                // The server answered, so it is online; what it said is the problem.
                self.online.store(true, Ordering::Relaxed);
                *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(e.to_string());
            }
        }
    }

    /// Push one snapshot now. Returns the version the server assigned.
    pub async fn push(&self, snapshot: &Snapshot) -> Result<u64, PushError> {
        let result = async {
            let resp = self
                .http
                .put(format!("{}/mgmt/v1/snapshot", self.base))
                .bearer_auth(&self.token)
                .json(snapshot)
                .send()
                .await
                .map_err(|e| PushError::Unreachable(e.to_string()))?;
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(PushError::Refused { status: status.as_u16(), body });
            }
            serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["version"].as_u64())
                .ok_or(PushError::Refused { status: status.as_u16(), body })
        }
        .await;
        self.note(&result);
        result
    }

    /// Is the server there? Updates the online state; a server that is back after being
    /// away is sent the snapshot again.
    pub async fn check(&self) -> bool {
        let was_online = self.server_online();
        let ok = self
            .http
            .get(format!("{}/mgmt/v1/health", self.base))
            .bearer_auth(&self.token)
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false);
        self.online.store(ok, Ordering::Relaxed);
        if ok && !was_online {
            self.mark_dirty();
        }
        ok
    }

    /// Keep the server's snapshot current for as long as the process runs: push at start-up
    /// and after each `mark_dirty`, retrying with a growing pause while the server is away,
    /// and check on the server now and then so the online state stays true.
    pub async fn run(self: Arc<Self>, db: Arc<Mutex<AdminDb>>) {
        let mut pause = Duration::from_secs(1);
        loop {
            if self.dirty.swap(false, Ordering::Relaxed) {
                let snapshot = { build_snapshot(&db.lock().unwrap_or_else(|e| e.into_inner())) };
                let result = match snapshot {
                    Ok(s) => self.push(&s).await,
                    Err(e) => Err(PushError::Build(e.to_string())),
                };
                match result {
                    Ok(v) => {
                        tracing::info!(version = v, "snapshot pushed");
                        pause = Duration::from_secs(1);
                    }
                    Err(e) => {
                        tracing::warn!("{e}; will retry");
                        self.dirty.store(true, Ordering::Relaxed);
                        tokio::time::sleep(pause).await;
                        pause = (pause * 2).min(Duration::from_secs(30));
                        continue;
                    }
                }
            }
            tokio::select! {
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(10)) => { self.check().await; }
            }
        }
    }
}
