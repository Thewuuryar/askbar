//! JSON endpoints behind the admin site. Every route here sits behind the admin session.

use crate::{store, AppState};
use askbar_proto::DisplaySettings;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

type St = State<Arc<AppState>>;

fn internal<E>(_: E) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

/// What the admin page sees of a provider. There is deliberately no key field, only whether one is set.
#[derive(Serialize)]
pub struct ProviderRow {
    id: String,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    base_url: String,
    model: String,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    has_key: bool,
}

#[derive(Deserialize)]
pub struct ProviderInput {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    base_url: String,
    model: String,
    /// Write-only. Missing or empty on update means "keep the current key".
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    temperature: Option<f32>,
}

#[derive(Serialize)]
pub struct ApiError {
    error: String,
}

type Failure = (StatusCode, Json<ApiError>);

fn bad(msg: &str) -> Failure {
    (StatusCode::BAD_REQUEST, Json(ApiError { error: msg.into() }))
}

fn validate(i: &ProviderInput) -> Result<(), Failure> {
    if i.name.trim().is_empty() {
        return Err(bad("Name is required."));
    }
    if !matches!(i.kind.as_str(), "openai" | "anthropic") {
        return Err(bad("Type must be openai or anthropic."));
    }
    if i.model.trim().is_empty() {
        return Err(bad("Model is required."));
    }
    let url = i.base_url.trim();
    if !url.is_empty() && !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(bad("Base URL must start with http:// or https://."));
    }
    Ok(())
}

pub async fn list_providers(State(state): St) -> Result<Json<Vec<ProviderRow>>, StatusCode> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT id, name, type, base_url, model, max_tokens, temperature,
                    api_key_enc IS NOT NULL AND length(api_key_enc) > 0
             FROM providers ORDER BY name",
        )
        .map_err(internal)?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ProviderRow {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: r.get(2)?,
                base_url: r.get(3)?,
                model: r.get(4)?,
                max_tokens: r.get(5)?,
                temperature: r.get(6)?,
                has_key: r.get(7)?,
            })
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;
    Ok(Json(rows))
}

fn name_taken(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation)
}

pub async fn create_provider(State(state): St, Json(i): Json<ProviderInput>) -> Result<(StatusCode, Json<serde_json::Value>), Failure> {
    validate(&i)?;
    let id = ulid::Ulid::new().to_string();
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let secrets = state.secrets();
    let enc = i.api_key.as_deref().filter(|k| !k.is_empty()).map(|k| secrets.encrypt(k));
    db.conn()
        .execute(
            "INSERT INTO providers (id, name, type, base_url, model, api_key_enc, max_tokens, temperature, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, strftime('%s','now'))",
            params![id, i.name.trim(), i.kind, i.base_url.trim(), i.model.trim(), enc, i.max_tokens, i.temperature],
        )
        .map_err(|e| {
            if name_taken(&e) {
                (StatusCode::CONFLICT, Json(ApiError { error: "A provider with that name already exists.".into() }))
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "Could not save the provider.".into() }))
            }
        })?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({ "id": id }))))
}

pub async fn update_provider(State(state): St, Path(id): Path<String>, Json(i): Json<ProviderInput>) -> Result<StatusCode, Failure> {
    validate(&i)?;
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let secrets = state.secrets();
    let new_key = i.api_key.as_deref().filter(|k| !k.is_empty()).map(|k| secrets.encrypt(k));
    let changed = db
        .conn()
        .execute(
            // COALESCE keeps the stored key unless a replacement came in.
            "UPDATE providers SET name = ?2, type = ?3, base_url = ?4, model = ?5,
                    api_key_enc = COALESCE(?6, api_key_enc), max_tokens = ?7, temperature = ?8
             WHERE id = ?1",
            params![id, i.name.trim(), i.kind, i.base_url.trim(), i.model.trim(), new_key, i.max_tokens, i.temperature],
        )
        .map_err(|e| {
            if name_taken(&e) {
                (StatusCode::CONFLICT, Json(ApiError { error: "A provider with that name already exists.".into() }))
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "Could not save the provider.".into() }))
            }
        })?;
    if changed == 0 {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "No such provider.".into() })));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_provider(State(state): St, Path(id): Path<String>) -> StatusCode {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    match db.conn().execute("DELETE FROM providers WHERE id = ?1", [id]) {
        Ok(0) => StatusCode::NOT_FOUND,
        Ok(_) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Send one short message through the provider and report what happened.
pub async fn test_provider(State(state): St, Path(id): Path<String>) -> Result<Json<crate::provider_test::TestResult>, StatusCode> {
    crate::provider_test::run(&state, &id).await.map_err(internal)?.map(Json).ok_or(StatusCode::NOT_FOUND)
}

// ---------------------------------------------------------------- settings

const MAX_PROMPT_CHARS: usize = 20_000;

#[derive(Serialize, Deserialize)]
pub struct AdminSettings {
    system_prompt: String,
    display: DisplaySettings,
}

fn check_display(d: &DisplaySettings) -> Result<(), Failure> {
    if !matches!(d.theme.as_str(), "auto" | "light" | "dark") {
        return Err(bad("Theme must be auto, light or dark."));
    }
    let in_range = |v: u32, lo: u32, hi: u32| (lo..=hi).contains(&v);
    if !in_range(d.width, 300, 4000) || !in_range(d.max_height, 200, 4000) || !in_range(d.history_width, 120, 1000) {
        return Err(bad("Width must be 300-4000, max height 200-4000 and history width 120-1000."));
    }
    if !(0.0..=0.8).contains(&d.top_percent) {
        return Err(bad("Top percent must be between 0 and 0.8."));
    }
    Ok(())
}

pub async fn get_settings(State(state): St) -> Result<Json<AdminSettings>, StatusCode> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    Ok(Json(AdminSettings {
        system_prompt: store::system_prompt(&db).map_err(internal)?,
        display: crate::api::load_display(&db).map_err(internal)?,
    }))
}

pub async fn put_settings(State(state): St, Json(s): Json<AdminSettings>) -> Result<StatusCode, Failure> {
    if s.system_prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(bad("The system prompt is too long."));
    }
    check_display(&s.display)?;
    let display = serde_json::to_string(&s.display).map_err(|_| bad("Invalid display settings."))?;
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let tx = db.conn().unchecked_transaction().map_err(|e| failure(&e))?;
    for (k, v) in [("system_prompt", s.system_prompt.as_str()), ("display", display.as_str())] {
        tx.execute("INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![k, v])
            .map_err(|e| failure(&e))?;
    }
    tx.commit().map_err(|e| failure(&e))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Interim: set one user's display defaults until the admin moves out of the server.
pub async fn put_user_display(State(state): St, Path(user_id): Path<String>, Json(d): Json<DisplaySettings>) -> Result<StatusCode, Failure> {
    check_display(&d)?;
    let value = serde_json::to_string(&d).map_err(|_| bad("Invalid display settings."))?;
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let known = db.conn().query_row("SELECT 1 FROM users WHERE id = ?1", [&user_id], |_| Ok(())).is_ok();
    if !known {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "No such user.".into() })));
    }
    db.conn()
        .execute(
            "INSERT INTO display_defaults (user_id, value) VALUES (?1, ?2)
             ON CONFLICT(user_id) DO UPDATE SET value = excluded.value",
            params![user_id, value],
        )
        .map_err(|e| failure(&e))?;
    Ok(StatusCode::NO_CONTENT)
}

fn failure(_: &rusqlite::Error) -> Failure {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "Could not save the settings.".into() }))
}

// ----------------------------------------------------------------- devices

pub async fn list_devices(State(state): St) -> Result<Json<Vec<crate::auth::DeviceRow>>, StatusCode> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    crate::auth::list_devices(&db).map(Json).map_err(internal)
}

pub async fn revoke_device(State(state): St, Path(id): Path<String>) -> StatusCode {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    match db.conn().execute("UPDATE devices SET revoked_at = COALESCE(revoked_at, ?2) WHERE id = ?1", rusqlite::params![id, crate::auth::now()]) {
        Ok(0) => StatusCode::NOT_FOUND,
        Ok(_) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[derive(Deserialize)]
pub struct PairInput {
    #[serde(default)]
    name: String,
}

#[derive(Serialize)]
pub struct PairingOut {
    pairing: String,
    expires_in: i64,
}

/// A one-time pairing string for a new device, good for ten minutes.
pub async fn create_pairing(State(state): St, Json(i): Json<PairInput>) -> Result<Json<PairingOut>, Failure> {
    let name = i.name.trim();
    if name.chars().count() > 60 {
        return Err(bad("The device name is too long."));
    }
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let fail = || (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "Could not create a pairing code.".into() }));
    let user: String = db.conn().query_row("SELECT id FROM users ORDER BY created_at, id LIMIT 1", [], |r| r.get(0)).map_err(|_| fail())?;
    let code = crate::auth::create_pair_code(&db, &user, name, crate::auth::PAIR_CODE_TTL_SECS).map_err(|_| fail())?;
    Ok(Json(PairingOut { pairing: askbar_proto::encode_pairing(&crate::network::effective_public_url(&db, &state), &code), expires_in: crate::auth::PAIR_CODE_TTL_SECS }))
}

// ------------------------------------------------------------------ backup

pub async fn backup(State(state): St) -> Result<axum::response::Response, StatusCode> {
    crate::backup::response(state).await
}
