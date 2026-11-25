//! The quick search and MCP front limits the administrator may change (plan P8.6d). Ranges come from `askbar-proto`, defaults
//! from `constants.rs`; an override is a `limit.<name>` row in `settings`, and a limit without one runs on its default. A change
//! goes to the server in the snapshot (refused there, nothing is kept) and to the gateway and the front by their links.
//! Audited as `limits/set` (names with old and new numbers), refused when the record cannot be appended.

use crate::constants::*;
use crate::web::Failure;
use crate::AdminState;
use askbar_proto::http::{ApiError, ApiJson};
use askbar_proto::quick_search::{self as qs, QuickSearchLimits};
use askbar_proto::{mcp, ErrorCode};
use axum::{extract::State, Json};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct Limit {
    pub name: &'static str,
    pub group: &'static str,
    pub default: u64,
    pub min: u64,
    pub max: u64,
}

const fn l(name: &'static str, group: &'static str, default: u64, min: u64, max: u64) -> Limit {
    Limit { name, group, default, min, max }
}

const Q: QuickSearchLimits = QUICK_SEARCH_LIMITS;

pub const LIMITS: [Limit; 12] = [
    l("max_queries", "quick_search", Q.max_queries as u64, qs::MIN_QUERIES as u64, qs::MAX_QUERIES as u64),
    l("max_query_chars", "quick_search", Q.max_query_chars as u64, qs::MIN_QUERY_CHARS as u64, qs::MAX_QUERY_CHARS as u64),
    l("max_pages", "quick_search", Q.max_pages as u64, qs::MIN_PAGES as u64, qs::MAX_PAGES as u64),
    l(
        "searches_per_user_hour",
        "quick_search",
        Q.searches_per_user_hour as u64,
        qs::MIN_SEARCHES_PER_HOUR as u64,
        qs::MAX_SEARCHES_PER_HOUR as u64,
    ),
    l("fetch_max_bytes", "mcp", MCP_FETCH_MAX_BYTES, mcp::MIN_FETCH_MAX_BYTES, mcp::MAX_FETCH_MAX_BYTES),
    l("fetch_timeout_secs", "mcp", MCP_FETCH_TIMEOUT_SECS as u64, mcp::MIN_FETCH_TIMEOUT_SECS as u64, mcp::MAX_FETCH_TIMEOUT_SECS as u64),
    l("fetch_redirects", "mcp", MCP_FETCH_REDIRECTS as u64, mcp::MIN_FETCH_REDIRECTS as u64, mcp::MAX_FETCH_REDIRECTS as u64),
    l(
        "tool_call_timeout_secs",
        "mcp",
        MCP_TOOL_CALL_TIMEOUT_SECS as u64,
        mcp::MIN_TOOL_CALL_TIMEOUT_SECS as u64,
        mcp::MAX_TOOL_CALL_TIMEOUT_SECS as u64,
    ),
    l(
        "one_time_id_window_secs",
        "mcp",
        MCP_ONE_TIME_ID_WINDOW_SECS as u64,
        mcp::MIN_ONE_TIME_ID_WINDOW_SECS as u64,
        mcp::MAX_ONE_TIME_ID_WINDOW_SECS as u64,
    ),
    l("search_results", "mcp", MCP_SEARCH_RESULTS as u64, mcp::MIN_SEARCH_RESULTS as u64, mcp::MAX_SEARCH_RESULTS as u64),
    l("remembered_urls", "mcp", MCP_REMEMBERED_URLS as u64, mcp::MIN_REMEMBERED_URLS as u64, mcp::MAX_REMEMBERED_URLS as u64),
    l("tool_result_max_bytes", "mcp", MCP_TOOL_RESULT_MAX_BYTES, mcp::MIN_TOOL_RESULT_MAX_BYTES, mcp::MAX_TOOL_RESULT_MAX_BYTES),
];

fn key(l: &Limit) -> String {
    format!("limit.{}", l.name)
}

/// The value a limit runs on: its override, or its default.
fn value_of(conn: &Connection, l: &Limit) -> u64 {
    let stored = conn.query_row("SELECT value FROM settings WHERE key = ?1", [key(l)], |r| r.get::<_, String>(0)).ok();
    stored.and_then(|s| s.trim().parse().ok()).filter(|v| (l.min..=l.max).contains(v)).unwrap_or(l.default)
}

/// The effective value of a limit by name; the names are the ones in `LIMITS`.
pub fn get(conn: &Connection, name: &str) -> u64 {
    value_of(conn, LIMITS.iter().find(|l| l.name == name).expect("a known limit"))
}

pub fn quick_search(conn: &Connection) -> QuickSearchLimits {
    let g = |n| get(conn, n) as u32;
    QuickSearchLimits {
        max_queries: g("max_queries"),
        max_query_chars: g("max_query_chars"),
        max_pages: g("max_pages"),
        searches_per_user_hour: g("searches_per_user_hour"),
    }
}

#[derive(Serialize)]
pub struct LimitRow {
    name: &'static str,
    value: u64,
    default: u64,
    min: u64,
    max: u64,
}

#[derive(Serialize)]
pub struct LimitsView {
    quick_search: Vec<LimitRow>,
    mcp: Vec<LimitRow>,
}

fn view(conn: &Connection) -> LimitsView {
    let rows = |group| {
        LIMITS
            .iter()
            .filter(|l| l.group == group)
            .map(|l| LimitRow { name: l.name, value: value_of(conn, l), default: l.default, min: l.min, max: l.max })
            .collect()
    };
    LimitsView { quick_search: rows("quick_search"), mcp: rows("mcp") }
}

fn internal<E>(_: E) -> Failure {
    crate::web::err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, "The admin could not read or store the limits.")
}

/// `GET /api/admin/v1/limits`.
pub async fn list(State(state): State<Arc<AdminState>>) -> Json<LimitsView> {
    Json(view(state.db.lock().await.conn()))
}

/// `PUT /api/admin/v1/limits`: a partial map, name to number, or `null` to go back to the default.
pub async fn put(
    State(state): State<Arc<AdminState>>,
    notes: Option<axum::Extension<crate::actions::ActionNotes>>,
    ApiJson(body): ApiJson<BTreeMap<String, Option<u64>>>,
) -> Result<Json<LimitsView>, Failure> {
    let mut changes = Vec::new();
    {
        let db = state.db.lock().await;
        for (name, v) in &body {
            let Some(l) = LIMITS.iter().find(|l| l.name == name) else {
                return Err(ApiError::new(ErrorCode::BadRequest, format!("There is no limit called {name}.")));
            };
            if v.is_some_and(|v| !(l.min..=l.max).contains(&v)) {
                return Err(ApiError::new(ErrorCode::BadRequest, format!("The limit {name} must be between {} and {}.", l.min, l.max)));
            }
            changes.push((l, *v, value_of(db.conn(), l)));
        }
        // The whole quick-search set must still be valid with the new values in it.
        let mut next = quick_search(db.conn());
        for (l, v, _) in &changes {
            let v = v.unwrap_or(l.default) as u32;
            match l.name {
                "max_queries" => next.max_queries = v,
                "max_query_chars" => next.max_query_chars = v,
                "max_pages" => next.max_pages = v,
                "searches_per_user_hour" => next.searches_per_user_hour = v,
                _ => {}
            }
        }
        next.validate().map_err(|why| ApiError::new(ErrorCode::BadRequest, why))?;
    }
    if let Some(axum::Extension(n)) = &notes {
        let shown: BTreeMap<_, _> =
            changes.iter().map(|(l, v, old)| (l.name, serde_json::json!({ "old": old, "new": v.unwrap_or(l.default) }))).collect();
        n.add("limits", serde_json::json!(shown));
    }
    crate::admin_api::mutate(&state, vec![], move |c| {
        for (l, v, _) in &changes {
            match v {
                Some(v) => c.execute(
                    "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key(l), v.to_string()],
                ),
                None => c.execute("DELETE FROM settings WHERE key = ?1", [key(l)]),
            }
            .map_err(internal)?;
        }
        Ok(())
    })
    .await?;
    if let Some(m) = state.mcp.get() {
        m.mark_dirty();
    }
    Ok(Json(view(state.db.lock().await.conn())))
}
