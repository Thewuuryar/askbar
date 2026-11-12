//! Read endpoints for a paired client. User-level reads are scoped to the token's user; everything
//! under `/spaces/{space_id}` goes through `MemberSpace`, which checks membership in the control
//! database before the space's file is opened (ADR 0009).

use crate::error::{ApiError, ApiJson};
use crate::{auth::ClientAuth, AppState};
use askbar_proto::problem::ErrorCode;
use askbar_proto::{
    Conversation, ConversationDetail, DisplaySettings, Message, PendingGeneration, Policy, Prefs, SearchHit, Settings, SpaceInfo,
};
use axum::{
    extract::{FromRequestParts, Path, Query, RawPathParams, State},
    http::{request::Parts, StatusCode},
    Extension, Json,
};
use rusqlite::{params, OptionalExtension};
use std::sync::Arc;

type St = State<Arc<AppState>>;
type Res<T> = Result<Json<T>, ApiError>;

fn internal<E>(_: E) -> ApiError {
    ApiError::code(ErrorCode::Internal)
}

/// The space named by the route's `{space_id}`, once the control database says the token's user is
/// a member of it. A space that does not exist and one the user is not in get the same 404, so the
/// answer says nothing about other people's spaces. The only way a handler reaches a space's file.
pub struct MemberSpace(pub Arc<crate::Space>);

impl FromRequestParts<Arc<AppState>> for MemberSpace {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &Arc<AppState>) -> Result<Self, ApiError> {
        let auth = parts.extensions.get::<ClientAuth>().cloned().ok_or_else(|| ApiError::code(ErrorCode::Unauthorized))?;
        let params = RawPathParams::from_request_parts(parts, state).await.map_err(internal)?;
        let space_id = params.iter().find(|(k, _)| *k == "space_id").map(|(_, v)| v.to_string()).ok_or_else(|| internal(()))?;
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        state
            .spaces
            .for_member(&db, &auth.user_id, &space_id)
            .map_err(internal)?
            .map(MemberSpace)
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "That space was not found."))
    }
}

/// `GET /spaces`: the spaces the caller is a member of, the personal one first.
pub async fn spaces(State(state): St, Extension(auth): Extension<ClientAuth>) -> Res<Vec<SpaceInfo>> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let rows = db.spaces_for(&auth.user_id).map_err(internal)?;
    Ok(Json(rows.into_iter().map(|s| SpaceInfo { id: s.id, kind: s.kind, name: s.name }).collect()))
}

/// The providers clients may use, from the snapshot: the default first, then the rest by name. A client with no choice of its own uses the first one.
/// `POST /me/pairing-keys`: a code to pair another client of this user (ADR 0008). The client builds the
/// pairing key from it with the server address and certificate pin it already has.
pub async fn new_pairing_key(
    State(state): St,
    Extension(auth): Extension<ClientAuth>,
    ApiJson(req): ApiJson<askbar_proto::PairingKeyRequest>,
) -> Result<(axum::http::StatusCode, Json<askbar_proto::PairingKeyResponse>), ApiError> {
    if req.name.chars().count() > 60 {
        return Err(ApiError::new(ErrorCode::BadRequest, "The client name is too long."));
    }
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    match crate::auth::new_client_pair_code(&db, &auth, &req.name).map_err(internal)? {
        Some(code) => Ok((
            axum::http::StatusCode::CREATED,
            Json(askbar_proto::PairingKeyResponse { code, expires_in: crate::auth::CLIENT_PAIR_CODE_TTL_SECS }),
        )),
        None => Err(ApiError::new(
            ErrorCode::TooManyAttempts,
            format!(
                "{} pairing keys are already waiting to be used. Use one, or wait 10 minutes.",
                crate::auth::MAX_OPEN_CLIENT_PAIR_CODES
            ),
        )
        .retry_after(crate::auth::CLIENT_PAIR_CODE_TTL_SECS as u64)),
    }
}

/// `GET /models`: the approved models and the default, as the gateway serves them (ADR 0014). Nothing approved yet is an
/// empty list; a gateway that has no settings or cannot be reached is `503`.
pub async fn models(State(state): St) -> Res<askbar_proto::ModelList> {
    Ok(Json(catalog(&state).await?))
}

/// The catalog from the gateway, or why it cannot be had.
pub(crate) async fn catalog(state: &AppState) -> Result<askbar_proto::ModelList, ApiError> {
    use crate::gateway::GatewayError as E;
    let Some(gateway) = state.gateway.clone() else {
        return Err(ApiError::new(ErrorCode::NotConfigured, "No model gateway is set up on this server."));
    };
    gateway.catalog().await.map_err(|e| match e {
        E::NotConfigured => ApiError::new(
            ErrorCode::NotConfigured,
            "The model gateway is starting up and has not received its settings yet. Try again in a few seconds.",
        )
        .retry_after(5),
        E::Rejected => ApiError::new(ErrorCode::ServerUnavailable, "The model gateway did not accept this server."),
        _ => ApiError::new(ErrorCode::ServerUnavailable, "The model gateway does not answer. Try again in a moment."),
    })
}

pub async fn conversations(MemberSpace(space): MemberSpace) -> Res<Vec<Conversation>> {
    let db = space.lock();
    let rows = db
        .list_conversations()
        .map_err(internal)?
        .into_iter()
        .map(|c| Conversation { id: c.id, title: c.title, updated_at: c.updated_at, message_count: c.message_count, model: c.model })
        .collect();
    Ok(Json(rows))
}

pub async fn conversation(MemberSpace(space): MemberSpace, Path((_, id)): Path<(String, String)>) -> Res<ConversationDetail> {
    let db = space.lock();
    let c = db
        .get_conversation(&id)
        .map_err(internal)?
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "That conversation was not found."))?;
    let messages = db
        .get_messages(&id)
        .map_err(internal)?
        .into_iter()
        .map(|m| Message {
            id: m.id,
            role: m.role,
            content: m.content,
            created_at: m.created_at,
            provider: m.by.map(|b| askbar_proto::ReplyProvider { id: b.provider_id, name: b.provider_name, model: b.model }),
            failed: m.failed.map(|f| askbar_proto::MessageFailure { code: f.code, message: f.message }),
        })
        .collect();
    let pending = db.list_active_generations().map_err(internal)?.into_iter().filter(|g| g.conversation_id == id).map(pending_of).collect();
    Ok(Json(ConversationDetail {
        conversation: Conversation { id: c.id, title: c.title, updated_at: c.updated_at, message_count: c.message_count, model: c.model },
        messages,
        pending,
    }))
}

fn pending_of(g: crate::Generation) -> PendingGeneration {
    PendingGeneration {
        generation_id: g.id,
        conversation_id: g.conversation_id,
        status: g.status,
        text: g.text,
        prompt: g.prompt,
        client_request_id: g.client_request_id,
    }
}

#[derive(serde::Deserialize)]
pub struct GenerationsQuery {
    status: Option<String>,
}

/// `GET /api/client/v1/spaces/{space_id}/generations?status=running`: the space's running and queued
/// generations, across its conversations, so a client that just started (or reconnected) can find
/// what is still going.
pub async fn generations(MemberSpace(space): MemberSpace, Query(q): Query<GenerationsQuery>) -> Res<Vec<PendingGeneration>> {
    if q.status.as_deref().is_some_and(|s| s != "running") {
        return Err(ApiError::new(ErrorCode::BadRequest, "Only status=running is supported."));
    }
    let db = space.lock();
    Ok(Json(db.list_active_generations().map_err(internal)?.into_iter().map(pending_of).collect()))
}

pub async fn delete_conversation(
    State(state): St,
    MemberSpace(space): MemberSpace,
    Path((_, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    match crate::generation::delete_conversation(&state, &space, &id) {
        Ok(false) => Err(ApiError::new(ErrorCode::NotFound, "That conversation was not found.")),
        Ok(true) => Ok(StatusCode::NO_CONTENT),
        Err(_) => Err(ApiError::code(ErrorCode::Internal)),
    }
}

pub async fn get_prefs(State(state): St, Extension(auth): Extension<ClientAuth>) -> Res<Prefs> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let active: Option<Option<String>> = db
        .conn()
        .query_row("SELECT active_model FROM user_prefs WHERE user_id = ?1", [&auth.user_id], |r| r.get(0))
        .optional()
        .map_err(internal)?;
    Ok(Json(Prefs { active_model: active.flatten() }))
}

/// Replaces the caller's preferences. A model id must be one the catalog approves (`model_not_approved` otherwise).
pub async fn put_prefs(State(state): St, Extension(auth): Extension<ClientAuth>, ApiJson(p): ApiJson<Prefs>) -> Res<Prefs> {
    if let Some(id) = &p.active_model {
        if !catalog(&state).await?.models.iter().any(|m| &m.id == id) {
            return Err(ApiError::new(ErrorCode::ModelNotApproved, "That model is not approved. Pick another one."));
        }
    }
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    db.conn()
        .execute(
            "INSERT INTO user_prefs (user_id, active_model) VALUES (?1, ?2)
             ON CONFLICT(user_id) DO UPDATE SET active_model = excluded.active_model",
            params![auth.user_id, p.active_model],
        )
        .map_err(internal)?;
    Ok(Json(p))
}

#[derive(serde::Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    q: String,
}

/// Full-text search over one space's messages.
pub async fn search(MemberSpace(space): MemberSpace, Query(q): Query<SearchQuery>) -> Res<Vec<SearchHit>> {
    if q.q.chars().count() > 200 {
        return Err(ApiError::new(ErrorCode::BadRequest, "A search can be at most 200 characters."));
    }
    let db = space.lock();
    let hits = db
        .search_messages(&q.q, 20)
        .map_err(internal)?
        .into_iter()
        .map(|h| SearchHit {
            conversation_id: h.conversation_id,
            message_id: h.message_id,
            role: h.role,
            snippet: h.snippet,
            created_at: h.created_at,
        })
        .collect();
    Ok(Json(hits))
}

/// What clients are served, from the snapshot: the user's own defaults if there are any,
/// else the server-wide ones, else the built-ins.
pub fn served_display(db: &crate::Db, user_id: Option<&str>) -> rusqlite::Result<DisplaySettings> {
    let Some(s) = crate::snapshot::current(db)? else { return Ok(DisplaySettings::default()) };
    Ok(user_id.and_then(|u| s.user_display_defaults.get(u).cloned()).unwrap_or(s.display))
}

/// Read-only: what this user's clients should look like unless the person overrides it locally.
pub async fn display_defaults(State(state): St, Extension(auth): Extension<ClientAuth>) -> Res<DisplaySettings> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    Ok(Json(served_display(&db, Some(&auth.user_id)).map_err(internal)?))
}

/// The privacy notice and the AI warning as the admin pushed them (ADR 0012). A snapshot from an admin that did not send a policy
/// gets the defaults for its retention, and no snapshot at all gets the defaults for 30 days: a person is told what is recorded
/// before the administrator has chosen anything.
pub fn served_policy(db: &crate::Db) -> rusqlite::Result<Policy> {
    Ok(match crate::snapshot::current(db)? {
        Some(s) => s.policy.unwrap_or_else(|| Policy::default_for(s.audit_retention_days)),
        None => Policy::default(),
    })
}

pub async fn settings(State(state): St) -> Res<Settings> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    Ok(Json(Settings { display: served_display(&db, None).map_err(internal)?, policy: served_policy(&db).map_err(internal)? }))
}
