//! The admin's link to `model-gateway` (ADR 0054, ADR 0028).
//!
//! The admin keeps the provider keys, encrypted with its own key, and pushes them in `PUT /config`; the gateway holds
//! them in memory only, so a restarted gateway is unconfigured until the admin pushes again. The admin checks the
//! gateway's `GET /status` every 12 seconds and pushes when it is unconfigured or runs another configuration. A
//! restart is the same case: the new process reports no configuration.
//!
//! A configuration's version is a hash of its content (the keys included), so the admin needs no counter and two
//! admins would agree. It is only compared, never shown. No key is logged, returned or put in an error here.

use crate::secrets::AdminSecrets;
use crate::server_link::build_snapshot;
use crate::AdminState;
use askbar_proto::gateway::{GatewayConfig, GatewayProvider};
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
pub const LINK: &str = "admin-model-gateway";
/// How often the admin reads the gateway's status (ADR 0028: every 10 to 15 seconds).
const CHECK_EVERY: Duration = Duration::from_secs(12);

/// The gateway's address: `ASKBAR_GATEWAY_URL`, default `http://gateway-admin-host:8084` (the alias exists on the
/// `gateway-admin` network only).
pub fn gateway_url() -> String {
    std::env::var("ASKBAR_GATEWAY_URL").ok().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| "http://gateway-admin-host:8084".into())
}

/// The token this admin sends the gateway, made by `bootstrap` (ADR 0049).
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

// ------------------------------------------------------------------ the keys

/// Keep each key of `keys` (provider id, key), encrypted, and drop the keys of providers that no longer exist.
/// Part of the caller's transaction.
pub(crate) fn keep_keys(conn: &Connection, secrets: &AdminSecrets, keys: &[(String, String)]) -> Result<(), ApiError> {
    let fail = || ApiError::new(ErrorCode::Internal, "The admin could not store the key.");
    for (id, key) in keys {
        conn.execute(
            "INSERT INTO provider_keys (provider_id, key_enc, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(provider_id) DO UPDATE SET key_enc = excluded.key_enc, updated_at = excluded.updated_at",
            params![id, secrets.encrypt(key), crate::auth::now()],
        )
        .map_err(|_| fail())?;
    }
    conn.execute("DELETE FROM provider_keys WHERE provider_id NOT IN (SELECT id FROM providers)", []).map_err(|_| fail())?;
    Ok(())
}

fn key_of(conn: &Connection, secrets: &AdminSecrets, id: &str) -> Option<String> {
    let blob: Vec<u8> =
        conn.query_row("SELECT key_enc FROM provider_keys WHERE provider_id = ?1", [id], |r| r.get(0)).optional().ok()??;
    secrets.decrypt(&blob)
}

/// What the gateway should run, from `admin.db`: the providers as the server has them, each with its key. The
/// managed provider's key is the one the admin keeps in its files (`managed`), not one that was entered.
pub fn build_config(conn: &Connection, secrets: &AdminSecrets, managed: Option<&(String, String)>) -> rusqlite::Result<GatewayConfig> {
    let snap = build_snapshot(conn)?;
    let providers = snap
        .providers
        .into_iter()
        .map(|p| {
            let api_key = match managed {
                Some((id, key)) if *id == p.id => key.clone(),
                _ => key_of(conn, secrets, &p.id).unwrap_or_default(),
            };
            GatewayProvider {
                id: p.id,
                name: p.name,
                kind: p.kind,
                base_url: p.base_url,
                model: p.model,
                max_tokens: p.max_tokens,
                temperature: p.temperature,
                headers: p.headers,
                api_key,
            }
        })
        .collect();
    let mut config = GatewayConfig {
        version: 0,
        providers,
        audit_public_key: snap.audit_public_key,
        generation_timeout_secs: snap.generation_timeout_secs,
    };
    config.version = version_of(&config);
    Ok(config)
}

/// The version of a configuration: the first 6 bytes of the SHA-256 of its content (version field zero), so it
/// fits an integer any client reads exactly. The same content has the same version.
pub fn version_of(config: &GatewayConfig) -> u64 {
    let mut content = config.clone();
    content.version = 0;
    let digest = Sha256::digest(serde_json::to_vec(&content).unwrap_or_default());
    digest[..6].iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b))
}

// ------------------------------------------------------------------ the link

/// How a check went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sync {
    /// The gateway runs this configuration already.
    Current,
    /// The configuration was pushed, under this version.
    Pushed(u64),
    /// The gateway did not answer.
    Unreachable,
    /// There is nothing to push yet, and why (a sentence for a person).
    NotReady(String),
    /// The gateway refused the configuration (a sentence for a person).
    Refused(String),
}

pub struct GatewayLink {
    http: reqwest::Client,
    base: String,
    token: String,
    online: AtomicBool,
    last_error: Mutex<Option<String>>,
    dirty: Notify,
}

impl GatewayLink {
    pub fn new(base: &str, token: String) -> Arc<GatewayLink> {
        Arc::new(GatewayLink {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest client"),
            base: base.trim_end_matches('/').to_string(),
            token,
            online: AtomicBool::new(false),
            last_error: Mutex::new(None),
            dirty: Notify::new(),
        })
    }

    pub fn online(&self) -> bool {
        self.online.load(Ordering::Relaxed)
    }

    /// A sentence about the last problem, for the Services page; never anything the gateway said.
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_error(&self, what: Option<String>) {
        *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = what;
    }

    /// Check the gateway now instead of at the next round: something it runs has changed.
    pub fn mark_dirty(&self) {
        self.dirty.notify_one();
    }

    async fn request(&self, method: reqwest::Method, path: &str, body: Option<&serde_json::Value>) -> Result<(u16, Vec<u8>), ()> {
        let url = format!("{}{}{path}", self.base, askbar_proto::paths::MODEL_GATEWAY_V1);
        let mut req = self.http.request(method, url).bearer_auth(&self.token);
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await.map_err(|_| ())?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|_| ())?;
        Ok((status, bytes.to_vec()))
    }

    /// A call on the gateway for the admin API: what it answered, after giving it the configuration if it has none (a
    /// provider saved a moment ago is not in its memory yet). A gateway that does not answer is a `502` with a sentence.
    pub async fn call(
        &self,
        state: &AdminState,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
        needs_config: bool,
    ) -> Result<(u16, Vec<u8>), ApiError> {
        let unreachable = || ApiError::new(ErrorCode::ServerUnavailable, "The gateway did not answer. Check it on the Services page.");
        match self.sync(state).await {
            Sync::Unreachable => return Err(unreachable()),
            // A saved provider is only in the gateway once it has a configuration, and that needs the audit key.
            Sync::NotReady(why) if needs_config => return Err(ApiError::new(ErrorCode::AuditKeyMissing, why)),
            _ => {}
        }
        self.request(method, path, body).await.map_err(|()| unreachable())
    }

    /// `GET /status`: the configuration version the gateway runs (none while unconfigured) and its boot ID. `None`
    /// if it did not answer or did not take the token.
    pub async fn status_now(&self) -> Option<(Option<u64>, String)> {
        let result = self.request(reqwest::Method::GET, askbar_service::routes::STATUS, None).await;
        let ok = match &result {
            Ok((200, body)) => serde_json::from_slice::<serde_json::Value>(body)
                .ok()
                .map(|v| (v["config_version"].as_u64(), v["boot_id"].as_str().unwrap_or_default().to_string())),
            _ => None,
        };
        self.online.store(ok.is_some(), Ordering::Relaxed);
        if ok.is_none() {
            self.set_error(Some(match result {
                Ok((401, _)) => "The gateway does not accept this admin's token.".to_string(),
                Ok(_) => "The gateway answered with something this admin does not understand.".to_string(),
                Err(()) => "The gateway does not answer.".to_string(),
            }));
        }
        ok
    }

    /// Ask the gateway to restart. Returns the boot ID of the process that is about to stop, or why not.
    pub async fn restart(&self) -> Result<String, ApiError> {
        match self.request(reqwest::Method::POST, askbar_service::routes::RESTART, None).await {
            Ok((202, body)) => Ok(serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["boot_id"].as_str().map(String::from))
                .unwrap_or_default()),
            Ok((429, _)) => Err(ApiError::new(
                ErrorCode::TooManyAttempts,
                "The gateway started less than 30 seconds ago. Restart it again in a moment.",
            )
            .retry_after(askbar_service::restart::COOLDOWN.as_secs())),
            Ok(_) | Err(()) => Err(ApiError::new(ErrorCode::ServerUnavailable, "The gateway did not take the restart request.")),
        }
    }

    /// Read the gateway's status and push the configuration if it needs one.
    pub async fn sync(&self, state: &AdminState) -> Sync {
        let managed = crate::cpa::managed_key(state).await;
        let config = {
            let db = state.db.lock().await;
            match build_config(db.conn(), &state.secrets, managed.as_ref()) {
                Ok(c) => c,
                Err(_) => return Sync::NotReady("The admin could not read its settings.".into()),
            }
        };
        let Some((running, _boot)) = self.status_now().await else { return Sync::Unreachable };
        if running == Some(config.version) {
            self.set_error(None);
            return Sync::Current;
        }
        if config.audit_public_key.is_none() {
            let why = "The audit key is not set up yet, so the gateway has nothing to run.".to_string();
            self.set_error(Some(why.clone()));
            return Sync::NotReady(why);
        }
        let body = serde_json::to_value(&config).unwrap_or_default();
        match self.request(reqwest::Method::PUT, askbar_service::routes::CONFIG, Some(&body)).await {
            Ok((200, _)) => {
                self.set_error(None);
                tracing::info!(version = config.version, providers = config.providers.len(), "pushed the configuration to the gateway");
                Sync::Pushed(config.version)
            }
            Ok((status, _)) => {
                let why = format!("The gateway refused the configuration (it answered {status}).");
                self.set_error(Some(why.clone()));
                Sync::Refused(why)
            }
            Err(()) => {
                self.online.store(false, Ordering::Relaxed);
                self.set_error(Some("The gateway did not answer the configuration.".into()));
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
                    tracing::info!("the gateway answers");
                } else {
                    tracing::warn!("the gateway does not answer; keys cannot be pushed until it does");
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
