//! The backup key (ADR 0013, D22, D21): the age key pair every backup is encrypted to, and its passphrase.
//!
//! - **The passphrase is the owner's alone.** The admin turns it into a key pair, keeps the public key (`age1…`) and the private
//!   key encrypted with the passphrase (`backup-key.age`, which every backup carries), and forgets the passphrase: it is in no
//!   database, no log, no audit record and no response. Losing it loses the backups for good.
//! - **At least 20 characters, and not the audit passphrase.** The two passphrases protect different things, and one must not open
//!   the other's (ADR 0013 rule 1). The audit passphrase is checked against every audit key, the retired ones too.
//! - **A fresh sign-in** (D21): a browser session whose sign-in is at most 10 minutes old. An API key cannot set or rotate it.
//! - **The public key goes everywhere that encrypts to it**: into the snapshot for the server and into the alerter's settings.
//!   The two other recipients, the auditor and `cpa`, are plan steps 6.4 and 6.3. A key that cannot be pushed to the server is
//!   not kept (the rule for every setting).
//! - **Each change is on record**: the admin's action record names the new key by its fingerprint, and the old one, never a key
//!   and never a passphrase.
//!
//! Rotating makes a new key pair and retires the old one, which stays listed: each backup carries the protected key of the
//! passphrase in force when it was made, so an older backup opens with its own, older passphrase.

use crate::actions::ActionNotes;
use crate::admin_api::{mutate, St};
use crate::db::BackupKeyRow;
use crate::web::{err, Failure};
use askbar_proto::http::ApiJson;
use askbar_service::backup;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};

/// What `GET /backup/key` says: whether a key is set, and its public name. Nothing secret: the public key is what every component
/// encrypts to, and the protected private key is not returned.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct KeyView {
    pub set: bool,
    pub fingerprint: Option<String>,
    /// The public key, `age1…`.
    pub recipient: Option<String>,
    pub created_at: Option<i64>,
    /// How many keys there have been (a rotation adds one).
    pub keys: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PassphraseInput {
    pub passphrase: String,
    pub confirm: String,
}

impl std::fmt::Debug for PassphraseInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PassphraseInput").finish_non_exhaustive()
    }
}

fn bad(msg: &str) -> Failure {
    err(StatusCode::BAD_REQUEST, msg)
}

fn internal() -> Failure {
    err(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong on the admin side.")
}

pub fn view_of(conn: &rusqlite::Connection) -> rusqlite::Result<KeyView> {
    let active = crate::db::active_backup_key(conn)?;
    Ok(KeyView {
        set: active.is_some(),
        fingerprint: active.as_ref().map(|k| k.fingerprint.clone()),
        recipient: active.as_ref().map(|k| k.recipient.clone()),
        created_at: active.as_ref().map(|k| k.created_at),
        keys: crate::db::backup_key_count(conn)?,
    })
}

/// `GET /api/admin/v1/backup/key`.
pub async fn get_key(State(state): St) -> Result<Json<KeyView>, Failure> {
    let db = state.db.lock().await;
    view_of(db.conn()).map(Json).map_err(|_| internal())
}

/// A passphrase that can be used: the two match, it is long enough, and it is not an audit passphrase.
async fn check_passphrase(state: &crate::AdminState, b: &PassphraseInput) -> Result<(), Failure> {
    use crate::audit_key::{derive, KdfParams};
    if b.passphrase != b.confirm {
        return Err(bad("The two passphrases do not match."));
    }
    if b.passphrase.chars().count() < backup::MIN_PASSPHRASE_CHARS {
        return Err(bad("The backup passphrase must be at least 20 characters."));
    }
    // It must open no audit key, retired ones included: one passphrase must not open the other's secrets (ADR 0013 rule 1).
    let rows = state.db.lock().await.audit_keys().map_err(|_| internal())?;
    for row in rows {
        let (pass, salt, params) = (b.passphrase.clone(), row.salt, KdfParams::parse(&row.argon_params).unwrap_or(KdfParams::CURRENT));
        let kp =
            tokio::task::spawn_blocking(move || derive(&pass, &salt, params)).await.map_err(|_| internal())?.map_err(|_| internal())?;
        if crate::sealed::is_that_key(&row.key_id, &kp) {
            return Err(bad("The backup passphrase must be different from the audit passphrase."));
        }
    }
    Ok(())
}

/// A new key pair from `passphrase` (about a second of scrypt, so off the request's thread).
async fn make_key(passphrase: String) -> Result<BackupKeyRow, Failure> {
    let key = tokio::task::spawn_blocking(move || backup::generate(&passphrase)).await.map_err(|_| internal())?.map_err(|e| match e {
        backup::Error::TooShort => bad("The backup passphrase must be at least 20 characters."),
        _ => internal(),
    })?;
    Ok(BackupKeyRow {
        recipient: key.recipient,
        fingerprint: key.fingerprint,
        protected: key.protected,
        created_at: crate::auth::now(),
        retired_at: None,
    })
}

/// Tell the alerter the new public key at its next look (it is in the settings the admin pushes it).
fn tell_alerter(state: &crate::AdminState) {
    if let Some(link) = state.alerter.get() {
        link.mark_dirty();
    }
}

/// `PUT /api/admin/v1/setup/backup-key`: make the first backup key from a passphrase. A second call is a 409: rotation is its own step.
pub async fn setup_key(
    State(state): St,
    headers: HeaderMap,
    notes: Option<Extension<ActionNotes>>,
    ApiJson(b): ApiJson<PassphraseInput>,
) -> Result<Json<KeyView>, Failure> {
    crate::web::require_fresh_signin(&state, &headers)?;
    if state.db.lock().await.conn().query_row("SELECT 1 FROM backup_keys WHERE retired_at IS NULL", [], |_| Ok(())).is_ok() {
        return Err(err(StatusCode::CONFLICT, "A backup key already exists. Rotate it instead."));
    }
    check_passphrase(&state, &b).await?;
    let row = make_key(b.passphrase).await?;
    let fingerprint = row.fingerprint.clone();
    // Kept and pushed to the server together, and the unique index on the active key settles two calls at once.
    mutate(&state, vec![], move |c| {
        c.execute(
            "INSERT INTO backup_keys (recipient, fingerprint, protected, created_at, retired_at) VALUES (?1, ?2, ?3, ?4, NULL)",
            rusqlite::params![row.recipient, row.fingerprint, row.protected, row.created_at],
        )
        .map(|_| ())
        .map_err(|_| err(StatusCode::CONFLICT, "A backup key already exists. Rotate it instead."))
    })
    .await?;
    if let Some(Extension(n)) = &notes {
        n.add("fingerprint", serde_json::json!(fingerprint));
    }
    tell_alerter(&state);
    let db = state.db.lock().await;
    view_of(db.conn()).map(Json).map_err(|_| internal())
}

/// `POST /api/admin/v1/backup/rotate-key`: a new key pair from a new passphrase; the old key is retired but kept listed.
/// The old passphrase is not asked for, and not checked against the new one: nothing here may be a way to guess it.
pub async fn rotate_key(
    State(state): St,
    headers: HeaderMap,
    notes: Option<Extension<ActionNotes>>,
    ApiJson(b): ApiJson<PassphraseInput>,
) -> Result<Json<KeyView>, Failure> {
    crate::web::require_fresh_signin(&state, &headers)?;
    let Some(old) = state
        .db
        .lock()
        .await
        .conn()
        .query_row("SELECT fingerprint FROM backup_keys WHERE retired_at IS NULL", [], |r| r.get::<_, String>(0))
        .ok()
    else {
        return Err(err(StatusCode::CONFLICT, "There is no backup key to rotate yet."));
    };
    check_passphrase(&state, &b).await?;
    let row = make_key(b.passphrase).await?;
    let (new_fingerprint, now) = (row.fingerprint.clone(), row.created_at);
    mutate(&state, vec![], move |c| {
        crate::db::replace_active_backup_key_in(c, &row, now).map_err(|_| err(StatusCode::CONFLICT, "That key already exists."))
    })
    .await?;
    if let Some(Extension(n)) = &notes {
        n.add("fingerprint", serde_json::json!(new_fingerprint));
        n.add("retired_fingerprint", serde_json::json!(old));
    }
    tell_alerter(&state);
    let db = state.db.lock().await;
    view_of(db.conn()).map(Json).map_err(|_| internal())
}
