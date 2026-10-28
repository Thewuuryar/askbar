//! JSON endpoints behind the admin site. Every route here sits behind the admin session.

use crate::{store, AppState};
use askbar_core::{stream_chat, ChatMessage};
use askbar_proto::DisplaySettings;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

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
    let enc = i.api_key.as_deref().filter(|k| !k.is_empty()).map(|k| state.secrets.encrypt(k));
    let id = ulid::Ulid::new().to_string();
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
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
    let new_key = i.api_key.as_deref().filter(|k| !k.is_empty()).map(|k| state.secrets.encrypt(k));
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
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

#[derive(Serialize)]
pub struct TestResult {
    ok: bool,
    /// The start of the model's reply on success, or the error.
    message: String,
}

/// Send one short message through the provider and report what happened.
pub async fn test_provider(State(state): St, Path(id): Path<String>) -> Result<Json<TestResult>, StatusCode> {
    let provider = {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let name: Option<String> =
            db.conn().query_row("SELECT name FROM providers WHERE id = ?1", [&id], |r| r.get(0)).optional().map_err(internal)?;
        let name = name.ok_or(StatusCode::NOT_FOUND)?;
        store::load_provider(&db, &state.secrets, &name).map_err(internal)?.ok_or(StatusCode::NOT_FOUND)?
    };
    let key = provider.api_key.clone();
    let (_keep, cancel) = oneshot::channel::<()>();
    let mut reply = String::new();
    let prompt = [ChatMessage::new("user", "Reply with the single word: ok")];
    let run = stream_chat(
        &state.http,
        &provider,
        "",
        &prompt,
        cancel,
        |d| {
            if reply.len() < 200 {
                reply.push_str(d);
            }
        },
    );
    let outcome = tokio::time::timeout(Duration::from_secs(30), run).await;
    let result = match outcome {
        Ok(Ok(_)) if !reply.trim().is_empty() => TestResult { ok: true, message: reply.trim().chars().take(120).collect() },
        Ok(Ok(_)) => TestResult { ok: false, message: "The provider answered but sent no text.".into() },
        Ok(Err(e)) => TestResult { ok: false, message: redact(&e.to_string(), &key) },
        Err(_) => TestResult { ok: false, message: "Timed out after 30 seconds.".into() },
    };
    Ok(Json(result))
}

/// Errors sometimes echo the credential back; never pass that on.
fn redact(msg: &str, key: &str) -> String {
    if key.len() >= 6 {
        msg.replace(key, "[key hidden]")
    } else {
        msg.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn redaction_hides_the_key() {
        assert_eq!(redact("bad key sk-abcdef123 given", "sk-abcdef123"), "bad key [key hidden] given");
        assert_eq!(redact("nothing here", ""), "nothing here");
    }
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

fn failure(_: &rusqlite::Error) -> Failure {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "Could not save the settings.".into() }))
}
