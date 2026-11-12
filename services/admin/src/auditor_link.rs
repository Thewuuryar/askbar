//! The admin's link to the auditor (ADR 0051): the reads of the Audit pages (traces, a trace's spans, sealed payloads,
//! stats) with the `admin-auditor-read` token over `audit-admin`, and the audit retention the admin sets. The admin
//! appends its own action records with a different token (plan step 2.7).
//!
//! Retention lives in the auditor; the admin's setting (`audit_retention_days`) is pushed to it every half minute and
//! at once after a change, so an auditor that was away gets it back.

use crate::AdminState;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

/// The link's name, as `bootstrap` names it.
pub const READ_LINK: &str = "admin-auditor-read";
const CHECK_EVERY: Duration = Duration::from_secs(30);

/// The auditor's address: `ASKBAR_AUDIT_API_URL`, default `http://auditor:8083`.
pub fn auditor_url() -> String {
    std::env::var("ASKBAR_AUDIT_API_URL").ok().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| "http://auditor:8083".into())
}

pub fn token_file(token_dir: &Path) -> PathBuf {
    token_dir.join("call").join(format!("{READ_LINK}.token"))
}

/// The read token `bootstrap` made. A missing or empty file means `bootstrap` has not run.
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
        Err(e) => Err(e),
    }
}

pub struct AuditorLink {
    http: reqwest::Client,
    base: String,
    token: String,
    online: AtomicBool,
    last_error: Mutex<Option<String>>,
    dirty: Notify,
    token_watch: crate::notify::TokenWatch,
}

impl AuditorLink {
    pub fn new(base: &str, token: String) -> Arc<AuditorLink> {
        Arc::new(AuditorLink {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(15))
                .build()
                .expect("reqwest client"),
            base: base.trim_end_matches('/').to_string(),
            token,
            online: AtomicBool::new(false),
            last_error: Mutex::new(None),
            dirty: Notify::new(),
            token_watch: crate::notify::TokenWatch::new(READ_LINK),
        })
    }

    /// Where an alert about this link's token goes (set once by `main`).
    pub fn set_notifier(&self, n: Arc<crate::notify::Notifier>) {
        self.token_watch.set_notifier(n);
    }

    pub fn online(&self) -> bool {
        self.online.load(Ordering::Relaxed)
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Push the retention setting now instead of at the next round.
    pub fn mark_dirty(&self) {
        self.dirty.notify_one();
    }

    fn note(&self, reached: bool, status: Option<u16>) {
        self.online.store(reached, Ordering::Relaxed);
        if let Some(status) = status {
            self.token_watch.saw(status);
        }
        *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = match (reached, status) {
            (false, _) => Some("The auditor does not answer.".into()),
            (true, Some(401)) => Some("The auditor does not accept this admin's read token.".into()),
            _ => None,
        };
    }

    /// `GET /api/audit/v1{path_and_query}`: the status and the JSON body, or `None` if nothing answered.
    pub async fn get(&self, path_and_query: &str) -> Option<(u16, serde_json::Value)> {
        let url = format!("{}{}{path_and_query}", self.base, askbar_proto::paths::AUDIT_V1);
        let resp = self.http.get(url).bearer_auth(&self.token).send().await;
        let Ok(resp) = resp else {
            self.note(false, None);
            return None;
        };
        let status = resp.status().as_u16();
        self.note(true, Some(status));
        Some((status, resp.json().await.unwrap_or(serde_json::Value::Null)))
    }

    /// `PUT /backup-key`: give the auditor the backup public key its export is encrypted to (ADR 0013). The admin gives it with each
    /// export, so a rotation or an auditor that lost its volume leaves no old key behind. Nothing is read back but the status.
    pub async fn put_backup_key(&self, recipient: &str) -> Result<(), u16> {
        let url = format!("{}{}/backup-key", self.base, askbar_proto::paths::AUDIT_V1);
        match self.http.put(url).bearer_auth(&self.token).json(&serde_json::json!({ "public_key": recipient })).send().await {
            Ok(r) if r.status().is_success() => {
                self.note(true, Some(r.status().as_u16()));
                Ok(())
            }
            Ok(r) => {
                self.note(true, Some(r.status().as_u16()));
                Err(r.status().as_u16())
            }
            Err(_) => {
                self.note(false, None);
                Err(0)
            }
        }
    }

    /// `GET /export`, answered as it arrives (the store can be large): the auditor's store, encrypted to the backup public key. The
    /// status is checked by the caller.
    pub async fn export_stream(&self) -> Result<reqwest::Response, ()> {
        let url = format!("{}{}/export", self.base, askbar_proto::paths::AUDIT_V1);
        // The client's own limit is 15 seconds for the whole call, which a large store outlasts: this call may take an hour.
        match self.http.get(url).bearer_auth(&self.token).timeout(Duration::from_secs(60 * 60)).send().await {
            Ok(r) => {
                self.note(true, Some(r.status().as_u16()));
                Ok(r)
            }
            Err(_) => {
                self.note(false, None);
                Err(())
            }
        }
    }

    /// Set the auditor's retention to `days`. `Ok(())` once it has taken it.
    pub async fn set_retention(&self, days: u32) -> Result<(), u16> {
        let url = format!("{}{}/retention", self.base, askbar_proto::paths::AUDIT_V1);
        let sent = self.http.put(url).bearer_auth(&self.token).json(&serde_json::json!({ "days": days })).send().await;
        match sent {
            Ok(r) if r.status().is_success() => {
                self.note(true, Some(r.status().as_u16()));
                Ok(())
            }
            Ok(r) => {
                self.note(true, Some(r.status().as_u16()));
                Err(r.status().as_u16())
            }
            Err(_) => {
                self.note(false, None);
                Err(0)
            }
        }
    }

    /// Make the auditor keep records as long as the admin's setting says. Returns whether it already did or now does.
    pub async fn sync_retention(&self, state: &AdminState) -> bool {
        let wanted = {
            let db = state.db.lock().await;
            crate::admin_api::audit_retention_days(db.conn())
        };
        let have = self.get("/retention").await.and_then(|(s, v)| (s == 200).then(|| v["days"].as_u64())).flatten();
        if have == Some(u64::from(wanted)) {
            return true;
        }
        match self.set_retention(wanted).await {
            Ok(()) => {
                tracing::info!(days = wanted, "set the auditor's retention");
                true
            }
            Err(_) => false,
        }
    }

    /// Check every half minute, and at once after a change, for as long as the process runs.
    pub async fn run(self: Arc<Self>, state: Arc<AdminState>) {
        let mut was_ok = None;
        loop {
            let ok = self.sync_retention(&state).await;
            if was_ok != Some(ok) {
                if ok {
                    tracing::info!("the auditor answers");
                } else {
                    tracing::warn!("the auditor does not take the retention setting; the Audit pages cannot be read until it answers");
                }
                was_ok = Some(ok);
            }
            tokio::select! {
                _ = tokio::time::sleep(CHECK_EVERY) => {}
                _ = self.dirty.notified() => {}
            }
        }
    }
}
