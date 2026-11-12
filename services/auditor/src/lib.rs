//! The auditor (ADR 0051, ADR 0010): the audit store as its own container, behind `/api/audit/v1`.
//!
//! - **Tokens.** Each link has its own token; the auditor holds only the hashes, in `auditor-token`, written by
//!   `bootstrap`. A link may call only its own routes, and a service may append only the kinds of record its link
//!   is for. The caller of a record is the one its token belongs to, never anything in the body.
//! - **Acknowledged appends.** An append is answered after the record is committed (group commit), and a retry of
//!   one whose answer was lost records it once.
//! - **Limits.** Past its backlog limit, or low on disk (for a new span), the auditor refuses with
//!   `503 audit_backlog` and records nothing, so model work stops rather than go unrecorded (ADR 0010 rule 3).
//! - **No content.** Prompts and replies arrive sealed and leave sealed. The auditor has no key.

pub mod alert_link;
pub mod disk;
pub mod ingest;
pub mod rules;

use askbar_audit::{AdminActionWrite, AlertDeliveryWrite, AppendError, PayloadWrite, Record, Recorder};
use askbar_proto::audit as wire;
use askbar_proto::http::ApiError;
use askbar_proto::paths::{self, AUDIT_V1};
use askbar_proto::ErrorCode;
use askbar_service::links::{Allow, Caller, Link, Links};
use askbar_service::status::BootId;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{Method, StatusCode};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The port the auditor listens on, on every interface (it joins only `audit-*` networks, one per caller).
pub const DEFAULT_PORT: u16 = 8083;
/// Sealed content is kept this long unless the admin says otherwise (never less than 7 days).
pub const DEFAULT_RETENTION_DAYS: u32 = 30;
pub const MAX_RETENTION_DAYS: u32 = 3650;
const RETENTION_KEY: &str = "retention_days";
/// The most one request may carry: a sealed prompt is at most 256 KB, and base64 makes it a third larger.
const MAX_BODY: usize = 4 * 1024 * 1024;
const MAX_ATTRS: usize = 16 * 1024;

/// When the auditor says no (ADR 0051's table). Tuned by the load test of plan step 7.2.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Records queued or being written, at most.
    pub max_queued: usize,
    /// How long the oldest queued record may have waited.
    pub max_wait: Duration,
    /// A new span is refused below this much free disk, leaving room for the finish and payload records of the
    /// spans already open.
    pub min_free_bytes: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { max_queued: 1000, max_wait: Duration::from_secs(5), min_free_bytes: 256 * 1024 * 1024 }
    }
}

pub struct Auditor {
    pub recorder: Recorder,
    pub limits: Limits,
    free_space: disk::FreeSpace,
    /// The boot ID each service last announced, to tag the spans it appends with it.
    boots: Mutex<HashMap<String, String>>,
    boot_id: BootId,
    /// When each link last made a request (unix seconds), by the caller its token stands for: what the alert rules read to
    /// tell a silent link from a quiet one (plan step 4.2, ADR 0055). In memory only: a restart starts it empty.
    last_request: Mutex<HashMap<&'static str, i64>>,
    /// Lines on the ingest listener that were not taken: not Caddy's, or too long (plan step 2.9).
    pub access_refused: std::sync::atomic::AtomicU64,
    /// What decides when to alert (plan step 4.2). Unset in the test rigs that do not look at alerts.
    rules: std::sync::OnceLock<rules::Rules>,
}

impl Auditor {
    pub fn new(recorder: Recorder, limits: Limits, free_space: disk::FreeSpace) -> Arc<Auditor> {
        Arc::new(Auditor {
            recorder,
            limits,
            free_space,
            boots: Mutex::new(HashMap::new()),
            boot_id: BootId::new(),
            last_request: Mutex::new(HashMap::new()),
            access_refused: Default::default(),
            rules: Default::default(),
        })
    }

    /// Gives the auditor the rules that send alerts, over `alerts` (set once, by `main`). `Auditor::new` keeps its signature,
    /// so every rig that does not care about alerts is as it was.
    pub fn with_alerts(self: Arc<Self>, alerts: Arc<dyn rules::Alerts>) -> Arc<Self> {
        let rules = rules::Rules::new(alerts);
        // What the admin last recorded for how long sealed content stays open, so a restart is not a change.
        if let Some(conn) = self.recorder.reader() {
            let (idle, total) = latest_unseal_limits(&conn);
            rules.seed_unseal_limits(idle, total);
        }
        let _ = self.rules.set(rules);
        self
    }

    /// The rules, if there are any.
    pub fn rules(&self) -> Option<&rules::Rules> {
        self.rules.get()
    }

    /// Notes a request on a link, at `at` (unix seconds).
    pub fn note_request(&self, caller: &'static str, at: i64) {
        self.last_request.lock().unwrap_or_else(|e| e.into_inner()).insert(caller, at);
        if let Some(r) = self.rules() {
            r.seen(caller, at);
        }
    }

    /// When each link last made a request, by caller.
    pub fn last_requests(&self) -> std::collections::BTreeMap<&'static str, i64> {
        self.last_request.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, v)| (*k, *v)).collect()
    }

    /// The retention setting now, as stored (default 30).
    pub fn retention_days(&self) -> u32 {
        self.recorder
            .reader()
            .and_then(|conn| askbar_audit::get_setting(&conn, RETENTION_KEY).ok().flatten())
            .and_then(|v| v.parse().ok())
            .map_or(DEFAULT_RETENTION_DAYS, |d: u32| d.max(askbar_audit::MIN_RETENTION_DAYS))
    }

    /// Runs the time-based rules once: disk, and links that have gone quiet. Nothing happens without rules.
    pub fn tick_rules(&self, now: i64) {
        if let Some(rules) = self.rules() {
            rules.tick(now, (self.free_space)(), &self.last_requests());
        }
    }

    /// Deletes what is past retention, and records the prune. Returns (spans, payloads).
    pub async fn prune_once(&self, now: i64) -> Result<(usize, usize), String> {
        self.recorder.prune(self.retention_days(), now).await
    }
}

/// The limits on how long sealed content stays open that the admin last recorded in a saved change (idle, total), in minutes.
fn latest_unseal_limits(conn: &askbar_audit::rusqlite::Connection) -> (Option<u32>, Option<u32>) {
    let Ok(mut stmt) = conn.prepare(
        "SELECT detail_json FROM admin_actions WHERE category = 'settings' AND action = 'update' AND outcome = 'done' \
         AND detail_json IS NOT NULL ORDER BY at DESC, rowid DESC LIMIT 200",
    ) else {
        return (None, None);
    };
    let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) else { return (None, None) };
    let (mut idle, mut total) = (None, None);
    for detail in rows.flatten() {
        let Ok(d) = serde_json::from_str::<serde_json::Value>(&detail) else { continue };
        idle = idle.or(d.get("unseal_idle_minutes").and_then(|v| v.as_u64()).map(|v| v as u32));
        total = total.or(d.get("unseal_total_minutes").and_then(|v| v.as_u64()).map(|v| v as u32));
        if idle.is_some() && total.is_some() {
            break;
        }
    }
    (idle, total)
}

/// How often the time-based rules run.
pub const RULES_EVERY: Duration = Duration::from_secs(30);

/// Runs the time-based alert rules every [`RULES_EVERY`], for as long as the process runs.
pub async fn rules_every_30_seconds(auditor: Arc<Auditor>) {
    loop {
        tokio::time::sleep(RULES_EVERY).await;
        auditor.tick_rules(now());
    }
}

/// Once a day, a minute after start-up: the first run comes soon enough that a long-stopped auditor catches up.
pub async fn retention_daily(auditor: Arc<Auditor>) {
    tokio::time::sleep(Duration::from_secs(60)).await;
    loop {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        match auditor.prune_once(now).await {
            Ok((0, 0)) => {}
            Ok((spans, payloads)) => tracing::info!(spans, payloads, "pruned records past retention"),
            Err(e) => tracing::warn!("could not prune: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
    }
}

// ------------------------------------------------------------------ the links

fn full(rest: &str) -> String {
    paths::join(AUDIT_V1, rest)
}

/// What one link may call: methods and paths after `/api/audit/v1`.
type Routes = &'static [(&'static str, &'static str)];

/// A service that appends records: its own spans, their finish and payload records, and its boot ID.
const SERVICE_APPEND: Routes = &[
    ("POST", "/hello"),
    ("POST", "/spans"),
    ("POST", "/spans/*/finish"),
    ("POST", "/spans/*/payloads"),
    ("POST", "/config-applied"),
    ("GET", "/status"),
];
/// The alerter's append token (plan step 4.1, ADR 0055): the outcome of each alert, and the same hello, status and
/// configuration record any service has. It opens no span and reads nothing.
const ALERTER_APPEND: Routes = &[("POST", "/hello"), ("POST", "/config-applied"), ("POST", "/alert-deliveries"), ("GET", "/status")];
/// The admin's append token: admin action records, and its boot ID. No read, no change, no delete.
const ADMIN_APPEND: Routes = &[("POST", "/hello"), ("POST", "/admin-actions"), ("GET", "/status")];
/// The admin's read token: records, metadata and retention. It appends nothing.
const ADMIN_READ: Routes = &[
    ("GET", "/status"),
    ("GET", "/traces"),
    ("GET", "/traces/*"),
    ("GET", "/spans/*/payloads"),
    ("GET", "/stats"),
    ("GET", "/token-counts"),
    ("GET", "/admin-actions"),
    ("GET", "/alert-deliveries"),
    ("GET", "/prunes"),
    ("GET", "/access-logs"),
    ("GET", "/chain/head"),
    ("GET", "/chain/entries/*"),
    ("GET", "/chain/verify"),
    ("GET", "/retention"),
    ("PUT", "/retention"),
];

/// The gateway's read token (plan step 3.2, ADR 0014): the token counts it holds the budget against, and nothing else. It
/// appends nothing with it and reads no trace, no content and no admin record.
const GATEWAY_READ: Routes = &[("GET", "/status"), ("GET", "/token-counts")];

/// Every link the auditor knows: its name as `bootstrap` names it (the file `accept/<name>.sha256`), the
/// caller it stands for, and what it may call. A link whose hash file is not there accepts nothing, so a link
/// is live from the day `bootstrap` makes its file.
pub const LINKS: &[(&str, &str, Routes)] = &[
    ("server-auditor", "server", SERVICE_APPEND),
    ("model-gateway-auditor", "model-gateway", SERVICE_APPEND),
    ("admin-auditor-append", "admin", ADMIN_APPEND),
    ("admin-auditor-read", "admin-read", ADMIN_READ),
    ("model-gateway-auditor-read", "model-gateway-read", GATEWAY_READ),
    ("alerter-auditor", "alerter", ALERTER_APPEND),
];

fn method(m: &str) -> Method {
    Method::from_bytes(m.as_bytes()).expect("a method")
}

/// The token check for the hashes in `token_dir/accept` (see `LINKS`).
pub fn links(token_dir: &FsPath) -> Links {
    Links::new(
        LINKS
            .iter()
            .map(|(name, caller, routes)| Link {
                caller,
                hash_file: token_dir.join("accept").join(format!("{name}.sha256")),
                allow: routes.iter().map(|(m, p)| Allow::method(method(m), &full(p))).collect(),
            })
            .collect(),
    )
}

/// Which kinds of span a caller may open. Only `model-gateway` opens model-call spans (ADR 0010 rule 2); the
/// server may open the checks that come before one.
fn may_start(caller: &str, kind: wire::SpanKind) -> bool {
    match caller {
        "model-gateway" => true,
        "server" => kind == wire::SpanKind::Gate,
        _ => false,
    }
}

// ------------------------------------------------------------------ the routes

/// One route, for the table the OpenAPI test reads.
pub struct RouteEntry {
    pub method: &'static str,
    pub path: String,
}

/// Every route, method and full path. The OpenAPI file describes exactly these.
pub fn routes() -> Vec<RouteEntry> {
    let mut out = vec![RouteEntry { method: "GET", path: full("/health") }];
    let mut seen = std::collections::BTreeSet::new();
    for (_, _, rs) in LINKS {
        for (m, p) in rs.iter() {
            if seen.insert((*m, *p)) {
                out.push(RouteEntry { method: m, path: full(p) });
            }
        }
    }
    out
}

pub fn router(auditor: Arc<Auditor>, links: Links) -> Router {
    let protected = Router::new()
        .route(&full("/hello"), post(hello))
        .route(&full("/spans"), post(start_span))
        .route(&full("/spans/{id}/finish"), post(finish_span))
        .route(&full("/spans/{id}/payloads"), post(add_payload).get(get_payloads))
        .route(&full("/admin-actions"), post(add_admin_action).get(list_admin_actions))
        .route(&full("/config-applied"), post(add_config_applied))
        .route(&full("/alert-deliveries"), post(add_alert_delivery).get(list_alert_deliveries))
        .route(&full("/traces"), get(traces))
        .route(&full("/traces/{id}"), get(trace))
        .route(&full("/stats"), get(stats))
        .route(&full("/token-counts"), get(token_counts))
        .route(&full("/prunes"), get(prunes))
        .route(&full("/access-logs"), get(list_access_logs))
        .route(&full("/chain/head"), get(chain_head))
        .route(&full("/chain/entries/{seq}"), get(chain_entry))
        .route(&full("/chain/verify"), get(chain_verify))
        .route(&full("/retention"), get(get_retention).put(put_retention))
        .route(&full("/status"), get(status))
        // Inside the token check, so only a request that passed it is noted, and with the caller the token stands for.
        .layer(axum::middleware::from_fn_with_state(auditor.clone(), note_request))
        .layer(axum::middleware::from_fn_with_state(links, askbar_service::links::require));
    Router::new()
        .route(&full("/health"), get(|| async { "ok" }))
        .merge(protected)
        .fallback(|| async { ApiError::code(ErrorCode::NotFound) })
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(auditor)
}

/// Notes the time of each request that passed the token check, by link (plan step 4.2, D2).
async fn note_request(State(a): St, req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    if let Some(Caller(caller)) = req.extensions().get::<Caller>().copied() {
        a.note_request(caller, now());
    }
    next.run(req).await
}

type St = State<Arc<Auditor>>;
type Failure = ApiError;

fn bad(detail: impl Into<String>) -> Failure {
    ApiError::new(ErrorCode::BadRequest, detail)
}

fn internal<E>(_: E) -> Failure {
    // Never the database's words: they can name a value.
    ApiError::new(ErrorCode::Internal, "The audit store could not do that.")
}

fn parse<T: DeserializeOwned>(body: &Bytes) -> Result<T, Failure> {
    serde_json::from_slice(body).map_err(|e| bad(format!("The body is not what this route takes ({}).", e.classify_name())))
}

/// `serde_json::Error` names its class without quoting the input.
trait ClassName {
    fn classify_name(&self) -> &'static str;
}

impl ClassName for serde_json::Error {
    fn classify_name(&self) -> &'static str {
        match self.classify() {
            serde_json::error::Category::Io => "unreadable",
            serde_json::error::Category::Syntax => "not JSON",
            serde_json::error::Category::Data => "wrong fields or types",
            serde_json::error::Category::Eof => "cut short",
        }
    }
}

fn ident(value: &str, what: &str) -> Result<(), Failure> {
    let ok = !value.is_empty() && value.len() <= 64 && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    ok.then_some(()).ok_or_else(|| bad(format!("{what} must be 1 to 64 letters, digits, - or _.")))
}

fn short(value: &str, what: &str, max: usize) -> Result<(), Failure> {
    (!value.is_empty() && value.len() <= max).then_some(()).ok_or_else(|| bad(format!("{what} must be 1 to {max} characters.")))
}

fn attrs_json(attrs: &Option<serde_json::Value>) -> Result<Option<String>, Failure> {
    let Some(v) = attrs else { return Ok(None) };
    let s = v.to_string();
    if s.len() > MAX_ATTRS || !v.is_object() {
        return Err(bad(format!("Attributes must be a JSON object of at most {} KB.", MAX_ATTRS / 1024)));
    }
    Ok(Some(s))
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn refuse(detail: &str) -> Failure {
    ApiError::new(ErrorCode::AuditBacklog, detail).retry_after(1)
}

/// The limits (ADR 0051): at the backlog limit every append is refused, and below the disk floor a new span is.
fn gate(a: &Auditor, new_span: bool) -> Result<(), Failure> {
    let backlog = a.recorder.backlog();
    if backlog.queued >= a.limits.max_queued || backlog.oldest > a.limits.max_wait {
        return Err(refuse("The audit store is behind; nothing was recorded. Try again in a moment."));
    }
    if new_span && (a.free_space)().is_some_and(|free| free < a.limits.min_free_bytes) {
        return Err(refuse("The audit volume is almost full, so no new span is accepted. Nothing was recorded."));
    }
    Ok(())
}

async fn append(a: &Auditor, record: Record) -> Result<u64, Failure> {
    // The rules hear of a record once it is committed, never before: an alert about something that was not recorded is false.
    let fed = a.rules().and_then(|_| rules::fed_of(&record));
    let appended = a.recorder.append(record).await.map_err(|e| match e {
        AppendError::Full | AppendError::Stopped => refuse("The audit store is behind; nothing was recorded. Try again in a moment."),
        AppendError::NoSuchSpan => ApiError::new(ErrorCode::NotFound, "No such span."),
        AppendError::Db(_) => internal(()),
    })?;
    if let (Some(rules), Some(fed)) = (a.rules(), fed) {
        rules.fed(&fed, now());
    }
    Ok(appended)
}

fn created(id: String) -> (StatusCode, Json<wire::Appended>) {
    (StatusCode::CREATED, Json(wire::Appended { id }))
}

// ------------------------------------------------------------------ appends

async fn hello(State(a): St, Extension(Caller(caller)): Extension<Caller>, body: Bytes) -> Result<Json<wire::HelloResult>, Failure> {
    let h: wire::Hello = parse(&body)?;
    ident(&h.boot_id, "The boot ID")?;
    gate(&a, false)?;
    a.boots.lock().unwrap_or_else(|e| e.into_inner()).insert(caller.to_string(), h.boot_id.clone());
    let interrupted = append(&a, Record::Hello { service: caller.to_string(), boot_id: h.boot_id }).await?;
    Ok(Json(wire::HelloResult { interrupted }))
}

async fn start_span(
    State(a): St,
    Extension(Caller(caller)): Extension<Caller>,
    body: Bytes,
) -> Result<(StatusCode, Json<wire::Appended>), Failure> {
    let s: wire::SpanStart = parse(&body)?;
    if !may_start(caller, s.kind) {
        return Err(ApiError::new(ErrorCode::Forbidden, "This link may not open a span of this kind."));
    }
    ident(&s.id, "The span id")?;
    ident(&s.trace_id, "The trace id")?;
    if let Some(p) = &s.parent_id {
        ident(p, "The parent id")?;
    }
    short(&s.name, "The name", 200)?;
    let attrs = attrs_json(&s.attrs)?;
    gate(&a, true)?;
    let boot_id = a.boots.lock().unwrap_or_else(|e| e.into_inner()).get(caller).cloned();
    let id = s.id.clone();
    append(
        &a,
        Record::Start(askbar_audit::SpanStart {
            id: s.id,
            trace_id: s.trace_id,
            parent_id: s.parent_id,
            kind: s.kind.as_str().to_string(),
            name: s.name,
            started_at: s.started_at,
            provider_id: s.provider_id,
            model: s.model,
            user_id: s.user_id,
            client_id: s.client_id,
            attrs_json: attrs,
            started_ms: s.started_ms,
            // From the token, never from the body.
            service: Some(caller.to_string()),
            boot_id,
        }),
    )
    .await?;
    Ok(created(id))
}

async fn finish_span(State(a): St, Path(id): Path<String>, body: Bytes) -> Result<(StatusCode, Json<wire::Appended>), Failure> {
    let f: wire::SpanFinish = parse(&body)?;
    ident(&id, "The span id")?;
    short(&f.status, "The status", 32)?;
    if f.status == "running" {
        return Err(bad("A span is finished with a status other than running."));
    }
    let attrs = attrs_json(&f.attrs)?;
    gate(&a, false)?;
    append(
        &a,
        Record::Finish(askbar_audit::SpanFinish {
            id: id.clone(),
            status: f.status,
            ended_at: f.ended_at,
            provider_id: f.provider_id,
            model: f.model,
            tokens_in: f.tokens_in,
            tokens_out: f.tokens_out,
            tokens_cached: f.tokens_cached,
            ttft_ms: f.ttft_ms,
            error_code: f.error_code,
            attrs_json: attrs,
            ended_ms: f.ended_ms,
        }),
    )
    .await?;
    Ok(created(id))
}

async fn add_payload(State(a): St, Path(id): Path<String>, body: Bytes) -> Result<(StatusCode, Json<wire::Appended>), Failure> {
    let p: wire::SealedPayload = parse(&body)?;
    ident(&id, "The span id")?;
    short(&p.role, "The role", 32)?;
    ident(&p.key_id, "The key id")?;
    let ciphertext = STANDARD.decode(p.ciphertext_b64.as_bytes()).map_err(|_| bad("The ciphertext is not base64."))?;
    if ciphertext.is_empty() {
        return Err(bad("A sealed payload is not empty."));
    }
    gate(&a, false)?;
    append(&a, Record::Payload(PayloadWrite { span_id: id.clone(), role: p.role, key_id: p.key_id, ciphertext, created_at: now() }))
        .await?;
    Ok(created(id))
}

async fn add_admin_action(
    State(a): St,
    Extension(Caller(caller)): Extension<Caller>,
    body: Bytes,
) -> Result<(StatusCode, Json<wire::Appended>), Failure> {
    let r: wire::AdminActionStart = parse(&body)?;
    ident(&r.id, "The record id")?;
    short(&r.category, "The category", 64)?;
    short(&r.action, "The action", 200)?;
    if !["requested", "done", "refused", "failed"].contains(&r.outcome.as_str()) {
        return Err(bad("The outcome is requested, done, refused or failed."));
    }
    let detail = attrs_json(&r.detail)?;
    gate(&a, false)?;
    let id = r.id.clone();
    append(
        &a,
        Record::AdminAction(AdminActionWrite {
            id: r.id,
            at: r.at,
            service: caller.to_string(),
            category: r.category,
            action: r.action,
            outcome: r.outcome,
            detail_json: detail,
        }),
    )
    .await?;
    Ok(created(id))
}

/// A service says which configuration version it now runs. It is stored as an admin action record whose service is the
/// caller (from the token), so the admin's claim of a push and the service's record of applying it sit side by side.
async fn add_config_applied(
    State(a): St,
    Extension(Caller(caller)): Extension<Caller>,
    body: Bytes,
) -> Result<(StatusCode, Json<wire::Appended>), Failure> {
    let r: wire::ConfigApplied = parse(&body)?;
    ident(&r.id, "The record id")?;
    gate(&a, false)?;
    let id = r.id.clone();
    append(
        &a,
        Record::AdminAction(AdminActionWrite {
            id: r.id,
            at: r.at,
            service: caller.to_string(),
            category: "config".into(),
            action: "applied".into(),
            outcome: "done".into(),
            detail_json: Some(serde_json::json!({ "version": r.version }).to_string()),
        }),
    )
    .await?;
    Ok(created(id))
}

/// The alerter says how an alert ended. Stored with the caller from the token, chained with the record. Only the
/// outcome is kept: the kind from a fixed shape, the channel and outcome from short lists, and a count.
async fn add_alert_delivery(
    State(a): St,
    Extension(Caller(caller)): Extension<Caller>,
    body: Bytes,
) -> Result<(StatusCode, Json<wire::Appended>), Failure> {
    let r: wire::AlertDeliveryStart = parse(&body)?;
    ident(&r.id, "The record id")?;
    short(&r.kind, "The kind", 64)?;
    if !r.kind.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
        return Err(bad("The kind is lower case letters, digits and underscores."));
    }
    if !["ntfy", "email"].contains(&r.channel.as_str()) {
        return Err(bad("The channel is ntfy or email."));
    }
    if !["sent", "failed", "suppressed"].contains(&r.outcome.as_str()) {
        return Err(bad("The outcome is sent, failed or suppressed."));
    }
    if r.attempts > 1000 {
        return Err(bad("The attempts are at most 1,000."));
    }
    gate(&a, false)?;
    let id = r.id.clone();
    append(
        &a,
        Record::AlertDelivery(AlertDeliveryWrite {
            id: r.id,
            at: r.at,
            caller: caller.to_string(),
            kind: r.kind,
            channel: r.channel,
            outcome: r.outcome,
            attempts: i64::from(r.attempts),
        }),
    )
    .await?;
    Ok(created(id))
}

// ------------------------------------------------------------------ reads

/// Runs `f` on a read-only connection, off the async threads.
async fn read<T: Send + 'static>(
    a: &Auditor,
    f: impl FnOnce(&rusqlite_conn::Connection) -> rusqlite_conn::Result<T> + Send + 'static,
) -> Result<T, Failure> {
    let recorder = a.recorder.clone();
    tokio::task::spawn_blocking(move || {
        let conn = recorder.reader().ok_or(())?;
        f(&conn).map_err(|_| ())
    })
    .await
    .map_err(internal)?
    .map_err(internal)
}

/// The database types the reads use, re-exported by the audit crate's `Connection` type alias.
mod rusqlite_conn {
    pub use askbar_audit::rusqlite::{Connection, Result};
}

#[derive(serde::Deserialize)]
struct TracesQuery {
    status: Option<String>,
    provider: Option<String>,
    before: Option<String>,
    limit: Option<usize>,
}

async fn traces(State(a): St, Query(q): Query<TracesQuery>) -> Result<Json<serde_json::Value>, Failure> {
    let filter = askbar_audit::TraceFilter { status: q.status, provider: q.provider, before: q.before, limit: q.limit };
    let rows = read(&a, move |conn| askbar_audit::list_traces(conn, &filter)).await?;
    Ok(Json(serde_json::Value::Array(rows)))
}

async fn trace(State(a): St, Path(id): Path<String>) -> Result<Json<serde_json::Value>, Failure> {
    let rows = read(&a, move |conn| askbar_audit::trace_spans(conn, &id)).await?;
    if rows.is_empty() {
        return Err(ApiError::new(ErrorCode::NotFound, "No such trace."));
    }
    Ok(Json(serde_json::Value::Array(rows)))
}

async fn get_payloads(State(a): St, Path(id): Path<String>) -> Result<Json<serde_json::Value>, Failure> {
    let rows = read(&a, move |conn| askbar_audit::span_payloads(conn, &id)).await?;
    match rows {
        Some(list) => Ok(Json(serde_json::Value::Array(list))),
        None => Err(ApiError::new(ErrorCode::NotFound, "No such span.")),
    }
}

#[derive(serde::Deserialize)]
struct SinceQuery {
    since: Option<i64>,
}

fn since_or_day(q: Option<i64>) -> i64 {
    q.unwrap_or_else(|| now() - 24 * 3600)
}

/// Counts and timings, with no cost: the auditor holds no prices, so every cost is null.
async fn stats(State(a): St, Query(q): Query<SinceQuery>) -> Result<Json<serde_json::Value>, Failure> {
    let since = since_or_day(q.since);
    Ok(Json(read(&a, move |conn| askbar_audit::stats(conn, since, &|_, _, _, _| None)).await?))
}

async fn token_counts(State(a): St, Query(q): Query<SinceQuery>) -> Result<Json<serde_json::Value>, Failure> {
    let since = since_or_day(q.since);
    Ok(Json(serde_json::Value::Array(read(&a, move |conn| askbar_audit::token_counts(conn, since)).await?)))
}

#[derive(serde::Deserialize)]
struct ActionsQuery {
    before: Option<String>,
    limit: Option<usize>,
}

async fn list_admin_actions(State(a): St, Query(q): Query<ActionsQuery>) -> Result<Json<serde_json::Value>, Failure> {
    let rows = read(&a, move |conn| askbar_audit::list_admin_actions(conn, q.before.as_deref(), q.limit)).await?;
    Ok(Json(serde_json::Value::Array(rows)))
}

async fn list_alert_deliveries(State(a): St, Query(q): Query<ActionsQuery>) -> Result<Json<serde_json::Value>, Failure> {
    let rows = read(&a, move |conn| askbar_audit::list_alert_deliveries(conn, q.before.as_deref(), q.limit)).await?;
    Ok(Json(serde_json::Value::Array(rows)))
}

/// The newest entry of the hash chain: what the admin keeps and checks later.
async fn chain_head(State(a): St) -> Result<Json<askbar_audit::chain::Head>, Failure> {
    Ok(Json(read(&a, askbar_audit::chain::head).await?))
}

/// The hash of one entry, so the admin can check that the entry it stored is still where it was.
async fn chain_entry(State(a): St, Path(seq): Path<i64>) -> Result<Json<askbar_audit::chain::Head>, Failure> {
    let hash = read(&a, move |conn| askbar_audit::chain::entry_hash(conn, seq)).await?;
    match hash {
        Some(hash) => Ok(Json(askbar_audit::chain::Head { seq, hash })),
        None => Err(ApiError::new(ErrorCode::NotFound, "No such entry.")),
    }
}

/// A full check of the chain: every link, every record against its digest. It reads the whole store, so the admin
/// asks for it now and then, not every few minutes.
async fn chain_verify(State(a): St) -> Result<Json<askbar_audit::chain::Report>, Failure> {
    Ok(Json(read(&a, askbar_audit::chain::verify).await?))
}

#[derive(serde::Deserialize)]
struct AccessQuery {
    before: Option<i64>,
    limit: Option<usize>,
}

/// Caddy's access lines, newest first.
async fn list_access_logs(State(a): St, Query(q): Query<AccessQuery>) -> Result<Json<serde_json::Value>, Failure> {
    let rows = read(&a, move |conn| askbar_audit::list_access_logs(conn, q.before, q.limit)).await?;
    Ok(Json(serde_json::Value::Array(rows)))
}

async fn prunes(State(a): St) -> Result<Json<serde_json::Value>, Failure> {
    Ok(Json(serde_json::Value::Array(read(&a, askbar_audit::list_prunes).await?)))
}

async fn get_retention(State(a): St) -> Json<wire::Retention> {
    Json(wire::Retention { days: a.retention_days(), floor_days: Some(askbar_audit::MIN_RETENTION_DAYS) })
}

async fn put_retention(State(a): St, body: Bytes) -> Result<Json<wire::Retention>, Failure> {
    let r: wire::Retention = parse(&body)?;
    if r.days < askbar_audit::MIN_RETENTION_DAYS {
        return Err(bad(format!("Records are kept at least {} days.", askbar_audit::MIN_RETENTION_DAYS)));
    }
    if r.days > MAX_RETENTION_DAYS {
        return Err(bad(format!("Records are kept at most {MAX_RETENTION_DAYS} days.")));
    }
    gate(&a, false)?;
    let old = a.retention_days();
    append(&a, Record::Setting { key: RETENTION_KEY.into(), value: r.days.to_string() }).await?;
    if let Some(rules) = a.rules() {
        rules.retention_changed(old, r.days);
    }
    Ok(Json(wire::Retention { days: r.days, floor_days: Some(askbar_audit::MIN_RETENTION_DAYS) }))
}

async fn status(State(a): St) -> Json<serde_json::Value> {
    let mut body = askbar_service::status::body(None, &a.boot_id);
    // How many of Caddy's lines were not kept: dropped because the queue was busy, or not an access line (plan step 2.9).
    body["access_lines_dropped"] = a.recorder.access_dropped().into();
    body["access_lines_refused"] = a.access_refused.load(std::sync::atomic::Ordering::Relaxed).into();
    // When each link last called, by caller, so the rules can tell a silent link from a quiet one (D2).
    body["last_requests"] = serde_json::json!(a.last_requests());
    Json(body)
}

/// Where the hash files are: `ASKBAR_AUDITOR_TOKEN_DIR`, default `/auditor-token`.
pub fn token_dir() -> PathBuf {
    std::env::var("ASKBAR_AUDITOR_TOKEN_DIR").ok().filter(|p| !p.trim().is_empty()).map_or_else(|| "/auditor-token".into(), PathBuf::from)
}
