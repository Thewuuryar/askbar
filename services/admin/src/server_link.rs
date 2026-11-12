//! The admin service's side of the server API: the token, the snapshot it builds from
//! `admin.db`, and the pusher that keeps the server's copy current.
//!
//! The token comes from `bootstrap` (ADR 0049): the admin reads it from its own credential volume,
//! mounted read-only, and never makes one. The server holds only the SHA-256, in its own volume, so it
//! can check a caller but can never act as one.

use crate::db::AdminDb;
use askbar_proto::{Budget, DisplaySettings, Snapshot, SnapshotPairCode, SnapshotProvider};
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::sync::Notify;

/// The link this module is the caller of, as `bootstrap` names it (ADR 0049).
pub const LINK: &str = "admin-server";

/// Where the secrets of the admin's own are kept: `ASKBAR_ADMIN_SECRETS_DIR`, default `/admin-secrets`.
pub fn secrets_dir() -> PathBuf {
    std::env::var("ASKBAR_ADMIN_SECRETS_DIR")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| "/admin-secrets".into())
}

/// The admin's credential volume, read-only: `ASKBAR_ADMIN_TOKEN_DIR`, default `/admin-token`.
pub fn token_dir() -> PathBuf {
    std::env::var("ASKBAR_ADMIN_TOKEN_DIR")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| "/admin-token".into())
}

/// The token this admin sends to the server, made by `bootstrap`.
pub fn token_file(token_dir: &Path) -> PathBuf {
    token_dir.join("call").join(format!("{LINK}.token"))
}

/// The server's server API: `ASKBAR_SERVER_API_URL`, default `http://server:8082`.
pub fn server_url() -> String {
    std::env::var("ASKBAR_SERVER_API_URL").ok().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| "http://server:8082".into())
}

pub fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The one log line when the server stops accepting the token (ADR 0024). Names the link, never a token.
fn rejection_line(previously_accepted: bool) -> String {
    format!(
        "the server rejected this admin's token on the link {LINK} ({}); rotate it with `docker compose run --rm bootstrap rotate {LINK}`, then restart the server and the admin",
        if previously_accepted { "it accepted it before" } else { "it has not accepted it since this admin started" }
    )
}

fn io_err(path: &Path, e: std::io::Error) -> std::io::Error {
    std::io::Error::new(e.kind(), format!("{}: {e}", path.display()))
}

/// Write `contents` to `path` through a temporary file, so a reader never sees half of it.
pub(crate) fn write_atomic(path: &Path, contents: &str, mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp).map_err(|e| io_err(&tmp, e))?;
    f.write_all(contents.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| io_err(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| io_err(path, e))
}

/// The token `bootstrap` made for this link. The admin never makes one: a missing or empty file means
/// `bootstrap` has not run, and the error says what to do (ADR 0049).
pub fn read_token(token_dir: &Path) -> std::io::Result<String> {
    let path = token_file(token_dir);
    let missing = |why: &str| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} {why}; run `docker compose up -d`, which runs bootstrap first", path.display()),
        )
    };
    match std::fs::read_to_string(&path) {
        Ok(t) if !t.trim().is_empty() => Ok(t.trim().to_string()),
        Ok(_) => Err(missing("is empty")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(missing("is missing")),
        Err(e) => Err(io_err(&path, e)),
    }
}

/// The file the admin kept the server token in before `bootstrap` (ADR 0049): removed on start, so no
/// copy of a token that no longer works stays on the volume. Returns whether there was one.
pub fn remove_old_token(secrets_dir: &Path) -> std::io::Result<bool> {
    let path = secrets_dir.join("server.token");
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io_err(&path, e)),
    }
}

// ------------------------------------------------------------------ the snapshot

fn active_audit_key(conn: &rusqlite::Connection) -> rusqlite::Result<Option<crate::db::AuditKeyRow>> {
    crate::db::active_audit_key(conn)
}

fn setting(conn: &rusqlite::Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()
}

/// The snapshot of everything in `admin.db` the server needs. (The `providers.allow_as_fallback`
/// column and a `fallback_order` setting from earlier versions are no longer read: v1 has no
/// provider fallback, ADR 0014. Migrations are append-only, so the column stays.)
/// `user_id` is the one user a
/// pairing code belongs to; the admin keeps codes per user already.
pub fn build_snapshot(conn: &rusqlite::Connection) -> rusqlite::Result<Snapshot> {
    let mut providers = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT id, name, type, base_url, model, max_tokens, temperature, headers, billing, price_in, price_out, price_cached_in FROM providers ORDER BY name")?;
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
                billing: if r.get::<_, String>(8)? == "subscription" {
                    askbar_proto::Billing::Subscription
                } else {
                    askbar_proto::Billing::Metered
                },
                price_in: r.get(9)?,
                price_out: r.get(10)?,
                price_cached_in: r.get(11)?,
            })
        })?;
        for p in rows {
            providers.push(p?);
        }
    }
    let known = |id: &String| providers.iter().any(|p| &p.id == id);
    let default_provider_id = setting(conn, "default_provider_id")?.filter(known);
    let display: DisplaySettings = setting(conn, "display")?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let mut user_display_defaults = std::collections::BTreeMap::new();
    {
        let mut stmt = conn.prepare("SELECT user_id, value FROM display_defaults")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (user, value) = row?;
            if let Ok(d) = serde_json::from_str(&value) {
                user_display_defaults.insert(user, d);
            }
        }
    }
    let mut pair_codes = Vec::new();
    {
        let mut stmt =
            conn.prepare("SELECT hash, user_id, client_name, expires_at FROM pair_codes WHERE expires_at > ?1 ORDER BY created_at")?;
        for c in stmt.query_map([crate::auth::now()], |r| {
            Ok(SnapshotPairCode { hash: r.get(0)?, user_id: r.get(1)?, client_name: r.get(2)?, expires_at: r.get(3)? })
        })? {
            pair_codes.push(c?);
        }
    }
    let revoked_client_ids = {
        let mut stmt = conn.prepare("SELECT client_id FROM revoked_clients ORDER BY client_id")?;
        let ids = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        ids
    };
    let paused_user_ids = {
        let mut stmt = conn.prepare("SELECT id FROM users WHERE paused = 1 ORDER BY id")?;
        let ids = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        ids
    };
    let (daily, monthly, kill): (Option<i64>, Option<i64>, i64) =
        conn.query_row("SELECT daily_tokens, monthly_tokens, kill_switch FROM budgets WHERE id = 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
    // How long audit records are kept: the notice says so, and its version changes with it.
    let audit_retention_days: u32 = setting(conn, "audit_retention_days")?.and_then(|s| s.parse().ok()).unwrap_or(30);
    // The privacy notice and the AI warning (ADR 0012): on unless switched off, with the default wording unless the administrator
    // wrote their own. The version of the notice is worked out here, from the wording and the retention.
    let flag = |key: &str| -> rusqlite::Result<bool> { Ok(setting(conn, key)?.is_none_or(|v| v.trim() != "0")) };
    let policy = askbar_proto::Policy::new(
        flag("privacy_notice_enabled")?,
        &setting(conn, "privacy_notice_text")?.unwrap_or_default(),
        flag("ai_warning_enabled")?,
        &setting(conn, "ai_warning_text")?.unwrap_or_default(),
        audit_retention_days,
    );
    Ok(Snapshot {
        policy: Some(policy),
        backup_public_key: crate::db::active_backup_key(conn)?.map(|k| k.recipient),
        version: 0,
        providers,
        default_provider_id,
        users: {
            let mut stmt = conn.prepare("SELECT id, name FROM users ORDER BY created_at, name")?;
            let rows = stmt
                .query_map([], |r| Ok(askbar_proto::SnapshotUser { id: r.get(0)?, name: r.get(1)? }))?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        },
        system_prompt: setting(conn, "system_prompt")?,
        display,
        user_display_defaults,
        pair_codes,
        revoked_client_ids,
        paused_user_ids,
        budget: Budget { daily_tokens: daily.map(|v| v as u64), monthly_tokens: monthly.map(|v| v as u64) },
        kill_switch: kill != 0,
        // Absent means the safer default: the switch really stops running replies.
        kill_switch_aborts_running: setting(conn, "kill_switch_aborts_running")?
            .map(|v| !matches!(v.trim(), "0" | "false"))
            .unwrap_or(true),
        audit_retention_days,
        // Absent on installs that never set it: the default applies, no migration needed.
        generation_timeout_secs: setting(conn, "generation_timeout_secs")?
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(askbar_proto::DEFAULT_GENERATION_TIMEOUT_SECS),
        // Absent or unreadable: 0, the whole conversation, as before the setting existed.
        history_turns: setting(conn, "history_turns")?.and_then(|s| s.trim().parse().ok()).unwrap_or(0),
        // Public half only; the keyring holds nothing else.
        audit_public_key: active_audit_key(conn)?
            .map(|k| askbar_proto::AuditKey { key_id: k.key_id, public_key: k.public_key.iter().map(|b| format!("{b:02x}")).collect() }),
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

/// How often the admin reads the server's status (ADR 0028: every 10 to 15 seconds).
const CHECK_EVERY: Duration = Duration::from_secs(12);

/// What `GET /api/server/v1/status` answers.
#[derive(serde::Deserialize, Debug)]
struct ServerStatus {
    config_version: Option<u64>,
    boot_id: String,
}

/// Whether the server should be sent the snapshot again: it was away (`!was_online`), it is a new
/// process (a boot ID other than the last one seen), or it does not run the version this admin
/// last pushed (none at all, or another one).
fn needs_push(was_online: bool, last_boot: Option<&str>, last_version: u64, st: &ServerStatus) -> bool {
    let restarted = last_boot.is_some_and(|b| b != st.boot_id);
    let stale = st.config_version != Some(last_version);
    !was_online || restarted || stale
}

/// Talks to the server's server API and remembers whether it is there.
pub struct ServerLink {
    http: reqwest::Client,
    base: String,
    token: String,
    online: AtomicBool,
    last_version: AtomicU64,
    last_error: StdMutex<Option<String>>,
    dirty: AtomicBool,
    wake: Notify,
    /// The server process the last status came from (its boot ID), to see a restart.
    last_boot: StdMutex<Option<String>>,
    /// The server has accepted this admin's token at least once since this process started.
    accepted: AtomicBool,
    /// The server's last answer to a call was `401`: it does not accept this token (ADR 0024).
    rejected: AtomicBool,
    /// Turns the first `401` of an episode into an alert (`rejected_token`).
    token_watch: crate::notify::TokenWatch,
    /// Turns three boot-ID changes within ten minutes into an alert (`restart_loop`).
    restarts: crate::notify::RestartWatch,
}

impl ServerLink {
    pub fn new(base: &str, token: String) -> Arc<ServerLink> {
        Arc::new(ServerLink {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(15))
                .build()
                .expect("a plain HTTP client builds"),
            base: base.trim_end_matches('/').to_string(),
            token,
            online: AtomicBool::new(false),
            last_version: AtomicU64::new(0),
            last_error: StdMutex::new(None),
            // A fresh admin has never pushed, so it starts out needing to.
            dirty: AtomicBool::new(true),
            wake: Notify::new(),
            last_boot: StdMutex::new(None),
            accepted: AtomicBool::new(false),
            rejected: AtomicBool::new(false),
            token_watch: crate::notify::TokenWatch::new(LINK),
            restarts: crate::notify::RestartWatch::new("server"),
        })
    }

    /// Where an alert about this link's token goes (set once by `main`).
    pub fn set_notifier(&self, n: Arc<crate::notify::Notifier>) {
        self.token_watch.set_notifier(n.clone());
        self.restarts.set_notifier(n);
    }

    /// True while the server answers `401` to this admin's token. Cleared by the next answer that is not.
    pub fn token_rejected(&self) -> bool {
        self.rejected.load(Ordering::Relaxed)
    }

    /// What the server's answer to a call says about the token (ADR 0024). `401` is the only answer that
    /// means the token was not accepted: any other status, an error included, came after the check. The
    /// first `401` of an episode logs one line naming the link, never the token; the next answer that is
    /// not a `401` ends the episode.
    fn saw(&self, status: u16) {
        self.token_watch.saw(status);
        if status != 401 {
            self.accepted.store(true, Ordering::Relaxed);
            self.rejected.store(false, Ordering::Relaxed);
            return;
        }
        if !self.rejected.swap(true, Ordering::Relaxed) {
            tracing::warn!("{}", rejection_line(self.accepted.load(Ordering::Relaxed)));
        }
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
                .put(format!("{}{}", self.base, askbar_proto::paths::join(askbar_proto::paths::SERVER_V1, "/config")))
                .bearer_auth(&self.token)
                .json(snapshot)
                .send()
                .await
                .map_err(|e| PushError::Unreachable(e.to_string()))?;
            let status = resp.status();
            self.saw(status.as_u16());
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

    /// Any server call. Returns the status and body of whatever the server answered, or
    /// `Unreachable` if it did not answer at all (which also marks it offline).
    pub async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<(u16, Vec<u8>), PushError> {
        let mut req = self.http.request(method, format!("{}{path}", self.base)).bearer_auth(&self.token);
        if let Some(b) = body {
            req = req.json(b);
        }
        match req.send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                self.saw(status);
                let bytes = resp.bytes().await.map_err(|e| PushError::Unreachable(e.to_string()))?;
                self.online.store(true, Ordering::Relaxed);
                Ok((status, bytes.to_vec()))
            }
            Err(e) => {
                self.online.store(false, Ordering::Relaxed);
                *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(e.to_string());
                Err(PushError::Unreachable(e.to_string()))
            }
        }
    }

    /// Is the server there, and does it run what was pushed? Reads `GET /status` (ADR 0028
    /// rule 4). `None` if the server did not answer (or refused the token); otherwise whether the
    /// snapshot should be pushed again (see `needs_push`). Updates the online state.
    pub async fn poll(&self) -> Option<bool> {
        let was_online = self.server_online();
        let status = self.fetch_status().await;
        self.online.store(status.is_some(), Ordering::Relaxed);
        let st = status?;
        let mut boot = self.last_boot.lock().unwrap_or_else(|e| e.into_inner());
        let push = needs_push(was_online, boot.as_deref(), self.last_version(), &st);
        *boot = Some(st.boot_id);
        Some(push)
    }

    /// The server's `GET /status` as it is now: `(config_version, boot_id)`, or `None` if it did not
    /// answer. Changes nothing here (the push decision is `poll`'s).
    pub async fn status_now(&self) -> Option<(Option<u64>, String)> {
        self.fetch_status().await.map(|s| (s.config_version, s.boot_id))
    }

    async fn fetch_status(&self) -> Option<ServerStatus> {
        async {
            let r = self
                .http
                .get(format!("{}{}", self.base, askbar_proto::paths::join(askbar_proto::paths::SERVER_V1, "/status")))
                .bearer_auth(&self.token)
                .timeout(Duration::from_secs(3))
                .send()
                .await
                .ok()?;
            self.saw(r.status().as_u16());
            if !r.status().is_success() {
                return None;
            }
            r.json::<ServerStatus>().await.ok()
        }
        .await
        .inspect(|st| self.restarts.saw(&st.boot_id))
    }

    /// `poll`, and mark the snapshot to be pushed when it should be. True if the server answered.
    pub async fn check(&self) -> bool {
        match self.poll().await {
            Some(push) => {
                if push {
                    self.mark_dirty();
                }
                true
            }
            None => false,
        }
    }

    /// Keep the server's snapshot current for as long as the process runs: push at start-up
    /// and after each `mark_dirty`, retrying with a growing pause while the server is away,
    /// and check on the server now and then so the online state stays true.
    pub async fn run(self: Arc<Self>, db: Arc<Mutex<AdminDb>>) {
        let mut pause = Duration::from_secs(1);
        loop {
            if self.dirty.swap(false, Ordering::Relaxed) {
                let snapshot = { build_snapshot(db.lock().await.conn()) };
                let result = match snapshot {
                    Ok(s) => self.push(&s).await,
                    Err(e) => Err(PushError::Build(e.to_string())),
                };
                if let Ok(v) = &result {
                    tracing::info!(version = v, "snapshot pushed");
                }
                match result {
                    Ok(_) => pause = Duration::from_secs(1),
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
                _ = tokio::time::sleep(CHECK_EVERY) => { self.check().await; }
            }
        }
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;

    fn st(version: Option<u64>, boot: &str) -> ServerStatus {
        ServerStatus { config_version: version, boot_id: boot.into() }
    }

    #[test]
    fn a_push_is_needed_when_away_restarted_unconfigured_or_out_of_date_and_only_then() {
        assert!(!needs_push(true, Some("A"), 3, &st(Some(3), "A")), "in step");
        assert!(!needs_push(true, None, 3, &st(Some(3), "A")), "the first status seen is not a restart");
        assert!(needs_push(true, Some("A"), 3, &st(Some(3), "B")), "a new process, even with the same version");
        assert!(needs_push(true, Some("A"), 3, &st(None, "A")), "no configuration");
        assert!(needs_push(true, Some("A"), 3, &st(Some(4), "A")), "another version");
        assert!(needs_push(true, Some("A"), 0, &st(Some(3), "A")), "nothing pushed by this admin yet");
        assert!(needs_push(false, Some("A"), 3, &st(Some(3), "A")), "back after being away");
    }
}
