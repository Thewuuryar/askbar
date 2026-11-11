//! The approved model catalog (ADR 0014, plan step 3.3): which models of which providers people may use, and the one
//! that answers when a send names none.
//!
//! The admin keeps it in `admin.db` (`catalog_models`, and the `default_model` setting) and pushes it to
//! `model-gateway` in `PUT /config`; the gateway serves it to the server, which offers it to clients and checks every
//! send against it. The catalog can be empty (an install starts that way, and nothing answers until the administrator
//! approves a model), but once it holds a model it needs a default.

use crate::web::{err, Failure};
use crate::AdminState;
use askbar_proto::http::ApiJson;
use askbar_proto::CatalogEntry;
use axum::{extract::State, http::StatusCode, Json};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The most models approved at once, and the longest model name.
const MAX_MODELS: usize = 500;
const MAX_MODEL_CHARS: usize = 200;

/// The catalog as the admin API shows and takes it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    pub models: Vec<CatalogEntry>,
    /// `provider/model`; present exactly when `models` is not empty.
    pub default: Option<String>,
}

/// The catalog from `admin.db`, in the order it was saved. A default that is missing or no longer in the list reads as
/// the first model, so a half-edited database still gives the gateway a usable catalog.
pub fn read(conn: &Connection) -> rusqlite::Result<Catalog> {
    let models: Vec<CatalogEntry> = conn
        .prepare("SELECT provider_id, model FROM catalog_models ORDER BY position")?
        .query_map([], |r| Ok(CatalogEntry { provider_id: r.get(0)?, model: r.get(1)? }))?
        .collect::<Result<_, _>>()?;
    let saved: Option<String> =
        rusqlite::OptionalExtension::optional(conn.query_row("SELECT value FROM settings WHERE key = 'default_model'", [], |r| r.get(0)))?;
    let default = saved.filter(|d| models.iter().any(|m| &m.id() == d)).or_else(|| models.first().map(CatalogEntry::id));
    Ok(Catalog { models, default })
}

/// Why a catalog cannot be saved, as a sentence for a person.
pub fn check(conn: &Connection, c: &Catalog) -> Result<(), String> {
    if c.models.len() > MAX_MODELS {
        return Err(format!("At most {MAX_MODELS} models can be approved."));
    }
    let mut seen = std::collections::BTreeSet::new();
    for m in &c.models {
        let known: bool = conn
            .query_row("SELECT 1 FROM providers WHERE id = ?1", [&m.provider_id], |_| Ok(()))
            .map(|_| true)
            .or_else(|e| if e == rusqlite::Error::QueryReturnedNoRows { Ok(false) } else { Err(e) })
            .map_err(|_| "The catalog could not be checked.".to_string())?;
        if !known {
            return Err("A model names a provider that does not exist.".into());
        }
        if m.model.trim().is_empty() || m.model.chars().count() > MAX_MODEL_CHARS || m.model.chars().any(char::is_control) {
            return Err(format!("A model name is 1 to {MAX_MODEL_CHARS} characters, with no control characters."));
        }
        if !seen.insert((m.provider_id.as_str(), m.model.as_str())) {
            return Err("A model is listed twice.".into());
        }
    }
    match (&c.default, c.models.is_empty()) {
        (None, true) => Ok(()),
        (None, false) => Err("Choose a default model: the one that answers when a message names none.".into()),
        (Some(_), true) => Err("There is no default model while no model is approved.".into()),
        (Some(d), false) if c.models.iter().any(|m| &m.id() == d) => Ok(()),
        (Some(_), false) => Err("The default model has to be one of the approved models.".into()),
    }
}

/// Replace the catalog with `c`, which `check` has passed. Part of the caller's transaction.
pub fn write(conn: &Connection, c: &Catalog) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM catalog_models", [])?;
    for (i, m) in c.models.iter().enumerate() {
        conn.execute(
            "INSERT INTO catalog_models (provider_id, model, position) VALUES (?1, ?2, ?3)",
            params![m.provider_id, m.model, i as i64],
        )?;
    }
    match &c.default {
        Some(d) => conn.execute(
            "INSERT INTO settings (key, value) VALUES ('default_model', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [d],
        )?,
        None => conn.execute("DELETE FROM settings WHERE key = 'default_model'", [])?,
    };
    Ok(())
}

/// A provider is going: take its models out of the catalog. Refused while it holds the default and other models remain,
/// because the catalog would be left with no default; choose another one first. Part of the caller's transaction.
pub fn drop_provider(conn: &Connection, provider_id: &str) -> Result<(), Failure> {
    let internal = |_| err(StatusCode::INTERNAL_SERVER_ERROR, "The catalog could not be read.");
    let before = read(conn).map_err(internal)?;
    let left: Vec<CatalogEntry> = before.models.iter().filter(|m| m.provider_id != provider_id).cloned().collect();
    if left.len() == before.models.len() {
        return Ok(());
    }
    let default = before.default.filter(|d| left.iter().any(|m| &m.id() == d));
    if !left.is_empty() && default.is_none() {
        return Err(err(
            StatusCode::CONFLICT,
            "This provider holds the default model. Choose another default under Approved models first.",
        ));
    }
    write(conn, &Catalog { models: left, default }).map_err(internal)
}

pub async fn get_catalog(State(state): State<Arc<AdminState>>) -> Result<Json<Catalog>, Failure> {
    let db = state.db.lock().await;
    read(db.conn()).map(Json).map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "The catalog could not be read."))
}

/// `PUT /catalog`: replace the approved models and the default, in one change. Recorded like every change; the gateway
/// has it within a moment.
pub async fn put_catalog(State(state): State<Arc<AdminState>>, ApiJson(c): ApiJson<Catalog>) -> Result<Json<Catalog>, Failure> {
    let saved = c.clone();
    crate::admin_api::mutate(&state, vec![], move |conn| {
        check(conn, &c).map_err(|why| err(StatusCode::BAD_REQUEST, &why))?;
        write(conn, &c).map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "The catalog could not be saved."))
    })
    .await?;
    Ok(Json(saved))
}
