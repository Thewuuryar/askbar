//! The admin's link to the `alerter` (ADR 0055, ADR 0028).
//!
//! The admin keeps the delivery settings (the ntfy server, topic and access token, and the mail server, login and addresses), each encrypted with its own key, and pushes
//! them in `PUT /config`; the alerter holds them in memory only, so a restarted alerter is unconfigured until the admin pushes
//! again. The admin checks the alerter's `GET /status` every 12 seconds and pushes when it is unconfigured or runs another
//! configuration; a restart is the same case, because the new process reports no configuration. A setting that no channel
//! holds is pushed too (an alerter with no channel is configured and drops what it is sent), so it answers events.
//!
//! A configuration's version is a hash of its content, so the admin needs no counter. No topic, token or server address is
//! logged, returned or put in an error here.

use crate::secrets::AdminSecrets;
use crate::AdminState;
use askbar_proto::alerter::{AlerterConfig, AlerterStatus, NtfyConfig, ReleaseStatus, SmtpConfig, SmtpTls, TestResult};
use askbar_proto::http::ApiError;
use askbar_proto::ErrorCode;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

/// The link's name, as `bootstrap` names it.
pub const LINK: &str = "admin-alerter";
/// How often the admin reads the alerter's status (ADR 0028: every 10 to 15 seconds).
const CHECK_EVERY: Duration = Duration::from_secs(12);
/// Delivery that has not worked for this long is a banner on every page (plan step 4.1).
pub const FAILING_FOR_SECS: i64 = 30 * 60;
/// An alerter that has not answered for this long is a banner too: nothing is delivering alerts.
pub const UNREACHABLE_FOR_SECS: i64 = 60;

/// The alerter's address: `ASKBAR_ALERTER_URL`, default `http://alerter-admin-host:8085` (the alias exists on the
/// `alerter-admin` network only).
pub fn alerter_url() -> String {
    std::env::var("ASKBAR_ALERTER_URL").ok().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| "http://alerter-admin-host:8085".into())
}

/// The token this admin sends the alerter, made by `bootstrap` (ADR 0049).
pub fn token_file(token_dir: &Path) -> PathBuf {
    token_dir.join("call").join(format!("{LINK}.token"))
}

/// Read it. A missing or empty file means `bootstrap` has not run, and the error says what to do.
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

// ------------------------------------------------------------------ the settings

/// The ntfy settings as stored, decrypted. `None` when no channel is set up (or a value cannot be read: the key changed).
pub fn read_ntfy(conn: &Connection, secrets: &AdminSecrets) -> Option<NtfyConfig> {
    let row: (Vec<u8>, Vec<u8>, Option<Vec<u8>>) = conn
        .query_row("SELECT ntfy_server_enc, ntfy_topic_enc, ntfy_token_enc FROM alert_settings WHERE id = 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()
        .ok()??;
    Some(NtfyConfig {
        server: secrets.decrypt(&row.0)?,
        topic: secrets.decrypt(&row.1)?,
        token: match row.2 {
            Some(blob) => Some(secrets.decrypt(&blob)?),
            None => None,
        },
    })
}

/// Keep `ntfy` (or remove the channel), each value encrypted.
pub fn write_ntfy(conn: &Connection, secrets: &AdminSecrets, ntfy: Option<&NtfyConfig>) -> rusqlite::Result<()> {
    match ntfy {
        None => conn.execute("DELETE FROM alert_settings WHERE id = 1", []).map(|_| ()),
        Some(n) => conn
            .execute(
                "INSERT INTO alert_settings (id, ntfy_server_enc, ntfy_topic_enc, ntfy_token_enc, updated_at) VALUES (1, ?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET ntfy_server_enc = excluded.ntfy_server_enc, ntfy_topic_enc = excluded.ntfy_topic_enc,
                   ntfy_token_enc = excluded.ntfy_token_enc, updated_at = excluded.updated_at",
                params![
                    secrets.encrypt(&n.server),
                    secrets.encrypt(&n.topic),
                    n.token.as_deref().map(|t| secrets.encrypt(t)),
                    crate::auth::now()
                ],
            )
            .map(|_| ()),
    }
}

/// The email settings as stored, decrypted. `None` when email is not set up (or a value cannot be read: the key changed).
pub fn read_email(conn: &Connection, secrets: &AdminSecrets) -> Option<SmtpConfig> {
    type Row = (Vec<u8>, i64, String, Option<Vec<u8>>, Option<Vec<u8>>, Vec<u8>, Vec<u8>);
    let row: Row = conn
        .query_row("SELECT host_enc, port, tls, user_enc, password_enc, from_enc, to_enc FROM alert_email_settings WHERE id = 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
        })
        .optional()
        .ok()??;
    let opt = |blob: Option<Vec<u8>>| -> Option<Option<String>> {
        match blob {
            Some(b) => secrets.decrypt(&b).map(Some),
            None => Some(None),
        }
    };
    Some(SmtpConfig {
        host: secrets.decrypt(&row.0)?,
        port: u16::try_from(row.1).ok()?,
        tls: match row.2.as_str() {
            "starttls" => SmtpTls::Starttls,
            "tls" => SmtpTls::Tls,
            "none" => SmtpTls::None,
            _ => return None,
        },
        user: opt(row.3)?,
        password: opt(row.4)?,
        from: secrets.decrypt(&row.5)?,
        to: secrets.decrypt(&row.6)?,
    })
}

/// Keep `email` (or remove the channel), each secret value encrypted.
pub fn write_email(conn: &Connection, secrets: &AdminSecrets, email: Option<&SmtpConfig>) -> rusqlite::Result<()> {
    let Some(m) = email else {
        return conn.execute("DELETE FROM alert_email_settings WHERE id = 1", []).map(|_| ());
    };
    let tls = match m.tls {
        SmtpTls::Starttls => "starttls",
        SmtpTls::Tls => "tls",
        SmtpTls::None => "none",
    };
    conn.execute(
        "INSERT INTO alert_email_settings (id, host_enc, port, tls, user_enc, password_enc, from_enc, to_enc, updated_at)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET host_enc = excluded.host_enc, port = excluded.port, tls = excluded.tls,
           user_enc = excluded.user_enc, password_enc = excluded.password_enc, from_enc = excluded.from_enc,
           to_enc = excluded.to_enc, updated_at = excluded.updated_at",
        params![
            secrets.encrypt(&m.host),
            m.port,
            tls,
            m.user.as_deref().map(|u| secrets.encrypt(u)),
            m.password.as_deref().map(|p| secrets.encrypt(p)),
            secrets.encrypt(&m.from),
            secrets.encrypt(&m.to),
            crate::auth::now()
        ],
    )
    .map(|_| ())
}

/// What the alerter should run, from `admin.db`.
pub fn build_config(conn: &Connection, secrets: &AdminSecrets) -> AlerterConfig {
    let mut config = AlerterConfig {
        version: 0,
        ntfy: read_ntfy(conn, secrets),
        email: read_email(conn, secrets),
        backup_public_key: crate::db::active_backup_key(conn).ok().flatten().map(|k| k.recipient),
    };
    config.version = version_of(&config);
    config
}

/// The version of a configuration: the first 6 bytes of the SHA-256 of its content (version field zero), so it fits an
/// integer any client reads exactly. The same content has the same version.
pub fn version_of(config: &AlerterConfig) -> u64 {
    let mut content = config.clone();
    content.version = 0;
    let digest = Sha256::digest(serde_json::to_vec(&content).unwrap_or_default());
    digest[..6].iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b))
}

// ------------------------------------------------------------------ the link

/// How a check went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sync {
    /// The alerter runs this configuration already.
    Current,
    /// The configuration was pushed, under this version.
    Pushed(u64),
    /// The alerter did not answer.
    Unreachable,
    /// It was not pushed, and why (a sentence for a person).
    NotReady(String),
    /// The alerter refused the configuration.
    Refused(u16),
}

/// Why a push did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushError {
    Unreachable,
    /// The alerter answered with this status.
    Refused(u16),
    /// The record of the push could not be made, so it was not made (a sentence).
    NotRecorded(String),
}

/// What the banner says (plan step 4.1): delivery works, is not set up, has been failing, or the alerter does not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Banner {
    Ok,
    NotSetUp,
    Failing,
    Unreachable,
}

impl Banner {
    pub fn as_str(self) -> &'static str {
        match self {
            Banner::Ok => "ok",
            Banner::NotSetUp => "not_set_up",
            Banner::Failing => "failing",
            Banner::Unreachable => "unreachable",
        }
    }
}

#[derive(Default)]
struct Watch {
    /// The last status the alerter gave.
    last: Option<AlerterStatus>,
    /// Since when delivery has been failing (a failure later than the last success), as this admin saw it.
    failing_since: Option<i64>,
    /// Since when the alerter has not answered.
    offline_since: Option<i64>,
}

pub struct AlerterLink {
    http: reqwest::Client,
    base: String,
    token: String,
    online: AtomicBool,
    last_error: Mutex<Option<String>>,
    watch: Mutex<Watch>,
    dirty: Notify,
    token_watch: crate::notify::TokenWatch,
    restarts: crate::notify::RestartWatch,
}

impl AlerterLink {
    pub fn new(base: &str, token: String) -> Arc<AlerterLink> {
        Arc::new(AlerterLink {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(15))
                .build()
                .expect("reqwest client"),
            base: base.trim_end_matches('/').to_string(),
            token,
            online: AtomicBool::new(false),
            last_error: Mutex::new(None),
            watch: Mutex::new(Watch::default()),
            dirty: Notify::new(),
            token_watch: crate::notify::TokenWatch::new(LINK),
            restarts: crate::notify::RestartWatch::new("alerter"),
        })
    }

    /// Where an alert about this link's token goes (set once by `main`).
    pub fn set_notifier(&self, n: Arc<crate::notify::Notifier>) {
        self.token_watch.set_notifier(n.clone());
        self.restarts.set_notifier(n);
    }

    pub fn online(&self) -> bool {
        self.online.load(Ordering::Relaxed)
    }

    /// A sentence about the last problem, for the Services page; never anything the alerter said.
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_error(&self, what: Option<String>) {
        *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = what;
    }

    /// Check the alerter now instead of at the next round: something it runs has changed.
    pub fn mark_dirty(&self) {
        self.dirty.notify_one();
    }

    async fn request(&self, method: reqwest::Method, path: &str, body: Option<&serde_json::Value>) -> Result<(u16, Vec<u8>), ()> {
        let url = format!("{}{}{path}", self.base, askbar_proto::paths::ALERTER_V1);
        let mut req = self.http.request(method, url).bearer_auth(&self.token);
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await.map_err(|_| ())?;
        let status = resp.status().as_u16();
        self.token_watch.saw(status);
        let bytes = resp.bytes().await.map_err(|_| ())?;
        Ok((status, bytes.to_vec()))
    }

    /// `GET /status`: what the alerter reports. `None` if it did not answer or did not take the token.
    pub async fn status_now(&self) -> Option<AlerterStatus> {
        let result = self.request(reqwest::Method::GET, askbar_service::routes::STATUS, None).await;
        let ok = match &result {
            Ok((200, body)) => serde_json::from_slice::<AlerterStatus>(body).ok(),
            _ => None,
        };
        self.online.store(ok.is_some(), Ordering::Relaxed);
        if let Some(st) = &ok {
            self.restarts.saw(&st.boot_id);
        }
        let now = crate::auth::now();
        {
            let mut w = self.watch.lock().unwrap_or_else(|e| e.into_inner());
            match &ok {
                Some(st) => {
                    w.offline_since = None;
                    let failing = match (st.last_delivery_failed_at, st.last_delivery_ok_at) {
                        (Some(failed), Some(good)) => failed > good,
                        (Some(_), None) => true,
                        _ => false,
                    };
                    if failing {
                        // The failure's own time, if it is the first this admin has seen, so a long outage is counted from when
                        // delivery stopped working and not from when this admin started.
                        w.failing_since.get_or_insert(st.last_delivery_ok_at.unwrap_or(st.last_delivery_failed_at.unwrap_or(now)));
                    } else {
                        w.failing_since = None;
                    }
                    w.last = Some(st.clone());
                }
                None => {
                    w.offline_since.get_or_insert(now);
                }
            }
        }
        if ok.is_none() {
            self.set_error(Some(match result {
                Ok((401, _)) => "The alerter does not accept this admin's token.".to_string(),
                Ok(_) => "The alerter answered with something this admin does not understand.".to_string(),
                Err(()) => "The alerter does not answer.".to_string(),
            }));
        }
        ok
    }

    /// What the banner should say at `now`, given whether a channel is set up.
    pub fn banner(&self, channel_set_up: bool, now: i64) -> Banner {
        let w = self.watch.lock().unwrap_or_else(|e| e.into_inner());
        if w.offline_since.is_some_and(|t| now - t >= UNREACHABLE_FOR_SECS) {
            Banner::Unreachable
        } else if w.failing_since.is_some_and(|t| now - t >= FAILING_FOR_SECS) {
            Banner::Failing
        } else if !channel_set_up {
            Banner::NotSetUp
        } else {
            Banner::Ok
        }
    }

    /// `GET /releases`: what the alerter's daily release check found. `None` if it did not answer or has no settings yet.
    pub async fn releases(&self) -> Option<ReleaseStatus> {
        match self.request(reqwest::Method::GET, "/releases", None).await {
            Ok((200, body)) => serde_json::from_slice(&body).ok(),
            _ => None,
        }
    }

    /// Ask the alerter to restart. Returns the boot ID of the process that is about to stop, or why not.
    pub async fn restart(&self) -> Result<String, ApiError> {
        match self.request(reqwest::Method::POST, askbar_service::routes::RESTART, None).await {
            Ok((202, body)) => Ok(serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["boot_id"].as_str().map(String::from))
                .unwrap_or_default()),
            Ok((429, _)) => Err(ApiError::new(
                ErrorCode::TooManyAttempts,
                "The alerter started less than 30 seconds ago. Restart it again in a moment.",
            )
            .retry_after(askbar_service::restart::COOLDOWN.as_secs())),
            Ok(_) | Err(()) => Err(ApiError::new(ErrorCode::ServerUnavailable, "The alerter did not take the restart request.")),
        }
    }

    /// `POST /exports`: the alerter's files, encrypted to the backup public key (ADR 0013). The admin cannot read them.
    pub async fn export(&self) -> Result<Vec<u8>, ApiError> {
        match self.request(reqwest::Method::POST, "/exports", None).await {
            Ok((200, body)) if !body.is_empty() => Ok(body),
            Ok((503, body))
                if serde_json::from_slice::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v["error"].as_str().map(String::from))
                    .as_deref()
                    == Some("backup_key_missing") =>
            {
                Err(ApiError::new(ErrorCode::BackupKeyMissing, "The alerter has not been given the backup key yet. Try again in a moment."))
            }
            Ok(_) | Err(()) => {
                Err(ApiError::new(ErrorCode::ServerUnavailable, "The alerter could not make its export. Nothing was backed up."))
            }
        }
    }

    /// `PUT /config`, as an action of its own: recorded before it is made, and not made when it cannot be (ADR 0010).
    pub async fn put_config(&self, state: &AdminState, config: &AlerterConfig) -> Result<(), PushError> {
        let body = serde_json::to_value(config).unwrap_or_default();
        let send = || self.request(reqwest::Method::PUT, askbar_service::routes::CONFIG, Some(&body));
        let sent = match state.actions.get() {
            None => Ok(send().await),
            Some(log) => {
                log.around(
                    "config_push",
                    "alerter",
                    serde_json::json!({ "target": "alerter", "version": config.version }),
                    send(),
                    |r| serde_json::json!({ "status": r.as_ref().ok().map(|(s, _)| *s) }),
                )
                .await
            }
        };
        match sent {
            Err(why) => Err(PushError::NotRecorded(why.sentence().to_string())),
            Ok(Ok((200, _))) => Ok(()),
            Ok(Ok((status, _))) => Err(PushError::Refused(status)),
            Ok(Err(())) => {
                self.online.store(false, Ordering::Relaxed);
                Err(PushError::Unreachable)
            }
        }
    }

    /// `POST /test`: what each channel really did with a test alert, after giving the alerter its settings if it has none.
    pub async fn test(&self, state: &AdminState) -> Result<TestResult, ApiError> {
        let unreachable = || ApiError::new(ErrorCode::ServerUnavailable, "The alerter did not answer. Check it on the Services page.");
        if matches!(self.sync(state).await, Sync::Unreachable) {
            return Err(unreachable());
        }
        match self.request(reqwest::Method::POST, "/test", None).await {
            Ok((200, body)) => serde_json::from_slice(&body).map_err(|_| {
                ApiError::new(ErrorCode::ServerUnavailable, "The alerter answered with something this admin does not understand.")
            }),
            Ok((503, _)) => {
                Err(ApiError::new(ErrorCode::ServerUnavailable, "The alerter has no settings yet. Try again in a few seconds."))
            }
            Ok(_) | Err(()) => Err(unreachable()),
        }
    }

    /// Read the alerter's status and push the configuration if it needs one.
    pub async fn sync(&self, state: &AdminState) -> Sync {
        let config = {
            let db = state.db.lock().await;
            build_config(db.conn(), &state.secrets)
        };
        let Some(status) = self.status_now().await else { return Sync::Unreachable };
        if status.config_version == Some(config.version) {
            self.set_error(None);
            return Sync::Current;
        }
        match self.put_config(state, &config).await {
            Ok(()) => {
                self.set_error(None);
                tracing::info!(
                    version = config.version,
                    ntfy = config.ntfy.is_some(),
                    email = config.email.is_some(),
                    "pushed the settings to the alerter"
                );
                Sync::Pushed(config.version)
            }
            Err(PushError::NotRecorded(why)) => {
                self.set_error(Some(why.clone()));
                Sync::NotReady(why)
            }
            Err(PushError::Refused(status)) => {
                self.set_error(Some(format!("The alerter refused the settings (it answered {status}).")));
                Sync::Refused(status)
            }
            Err(PushError::Unreachable) => {
                self.set_error(Some("The alerter did not answer the settings.".into()));
                Sync::Unreachable
            }
        }
    }

    /// Check every 12 seconds, and at once when something changed, for as long as the process runs.
    pub async fn run(self: Arc<Self>, state: Arc<AdminState>) {
        let mut was_online = None;
        loop {
            let outcome = self.sync(&state).await;
            let online = !matches!(outcome, Sync::Unreachable);
            if was_online != Some(online) {
                if online {
                    tracing::info!("the alerter answers");
                } else {
                    tracing::warn!("the alerter does not answer; settings cannot be pushed and no alert is delivered until it does");
                }
                was_online = Some(online);
            }
            tokio::select! {
                _ = tokio::time::sleep(CHECK_EVERY) => {}
                _ = self.dirty.notified() => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ntfy() -> NtfyConfig {
        NtfyConfig { server: "https://ntfy.example.com".into(), topic: "marker-topic".into(), token: Some("marker-token".into()) }
    }

    #[test]
    fn the_settings_are_stored_encrypted_and_come_back_and_none_means_no_channel() {
        let db = crate::AdminDb::open_in_memory().unwrap();
        let secrets = AdminSecrets::ephemeral();
        assert_eq!(read_ntfy(db.conn(), &secrets), None);
        write_ntfy(db.conn(), &secrets, Some(&ntfy())).unwrap();
        assert_eq!(read_ntfy(db.conn(), &secrets), Some(ntfy()));
        // What is in the table is ciphertext: none of the three values appears in it.
        let (server, topic, token): (Vec<u8>, Vec<u8>, Vec<u8>) = db
            .conn()
            .query_row("SELECT ntfy_server_enc, ntfy_topic_enc, ntfy_token_enc FROM alert_settings", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        for (blob, plain) in [(&server, "ntfy.example.com"), (&topic, "marker-topic"), (&token, "marker-token")] {
            assert!(!String::from_utf8_lossy(blob).contains(plain), "{plain} is in the clear");
        }
        // A token is optional, and another key cannot read what was stored.
        let no_token = NtfyConfig { token: None, ..ntfy() };
        write_ntfy(db.conn(), &secrets, Some(&no_token)).unwrap();
        assert_eq!(read_ntfy(db.conn(), &secrets), Some(no_token));
        assert_eq!(read_ntfy(db.conn(), &AdminSecrets::ephemeral()), None);
        write_ntfy(db.conn(), &secrets, None).unwrap();
        assert_eq!(read_ntfy(db.conn(), &secrets), None);
    }

    #[test]
    fn the_version_is_a_hash_of_the_content_so_the_same_settings_have_the_same_version() {
        let db = crate::AdminDb::open_in_memory().unwrap();
        let secrets = AdminSecrets::ephemeral();
        let empty = build_config(db.conn(), &secrets);
        assert_eq!(empty, build_config(db.conn(), &secrets));
        write_ntfy(db.conn(), &secrets, Some(&ntfy())).unwrap();
        let set = build_config(db.conn(), &secrets);
        assert_ne!(empty.version, set.version);
        assert_eq!(set.version, version_of(&set));
        assert!(set.version < 1 << 48, "it fits an integer any client reads exactly");
    }

    #[test]
    fn the_banner_says_what_is_wrong_in_the_order_that_matters() {
        let link = AlerterLink::new("http://127.0.0.1:9", "t".into());
        assert_eq!(link.banner(true, 1_000), Banner::Ok);
        assert_eq!(link.banner(false, 1_000), Banner::NotSetUp);
        {
            let mut w = link.watch.lock().unwrap();
            w.failing_since = Some(1_000);
        }
        assert_eq!(link.banner(true, 1_000 + FAILING_FOR_SECS - 1), Banner::Ok, "not yet half an hour");
        assert_eq!(link.banner(true, 1_000 + FAILING_FOR_SECS), Banner::Failing);
        {
            let mut w = link.watch.lock().unwrap();
            w.offline_since = Some(5_000);
        }
        assert_eq!(link.banner(true, 5_000 + UNREACHABLE_FOR_SECS - 1), Banner::Failing);
        assert_eq!(link.banner(true, 5_000 + UNREACHABLE_FOR_SECS), Banner::Unreachable, "an alerter that does not answer comes first");
    }
}
