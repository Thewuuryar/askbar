//! `model-gateway` (ADR 0054, ADR 0014): provider calls and provider keys in their own container, behind
//! `/api/model-gateway/v1`.
//!
//! - **Keys in memory only.** The admin pushes them in `PUT /config`. Nothing here writes one to disk, a log, an
//!   error or a response, and a restart is how one leaves memory.
//! - **Nothing is called before it is on record.** `POST /chat` appends the turn span to the auditor and waits for
//!   the acknowledgment (one second at most); without one it answers `503 audit_backlog` and calls no provider.
//!   Each try at a provider is an attempt span, acknowledged the same way.
//! - **Two listeners, two links.** The server may call `/chat`; the admin may read the status, push the
//!   configuration and ask for a restart. A token of one is no good on the other's routes or network.
//! - **No provider words.** The server gets a code and a class. The provider's error text is sealed to the audit
//!   public key and goes to the auditor as a payload; the gateway holds no private key.

pub mod llm;
pub mod models;
pub mod provider_test;
pub mod router;
pub mod seal;
pub mod usage;

use askbar_proto::audit::{SpanFinish, SpanKind, SpanStart};
use askbar_proto::gateway::{ChatEvent, ChatRequest, FailureClass, GatewayConfig};
use askbar_proto::http::ApiError;
use askbar_proto::paths::{self, MODEL_GATEWAY_V1};
use askbar_proto::ErrorCode;
use askbar_service::audit_client::{AuditClient, Refused};
use askbar_service::links::{Allow, Caller, Link, Links};
use askbar_service::restart::Restarter;
use askbar_service::status::BootId;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::Method;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};
use llm::{ChatMessage, Provider, ProviderKind, StreamOutcome};
use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, watch};

/// The port both listeners use, on the address each caller's network gives them.
pub const DEFAULT_PORT: u16 = 8084;
const MAX_BODY: usize = 4 * 1024 * 1024;
const MAX_KEY_CHARS: usize = 4096;
const MAX_MODEL_CHARS: usize = 200;
const MAX_RETRIES: u32 = 2;
const RETRY_BACKOFF: [Duration; 2] = [Duration::from_millis(250), Duration::from_secs(1)];
/// Provider error text kept with an attempt is cut here (ADR 0011).
pub const MAX_ERROR_PAYLOAD_BYTES: usize = 4096;
/// A turn the server has not finished this long after the generation time limit is finished as interrupted.
pub const STALE_TURN_GRACE: Duration = Duration::from_secs(60);

pub use askbar_service::audit_client::now_parts;

/// What a `PUT /config` put in place.
struct Applied {
    version: u64,
    providers: BTreeMap<String, Provider>,
    audit_key: [u8; 32],
    timeout: Duration,
    /// The token budget, authoritative here (ADR 0014).
    budget: askbar_proto::Budget,
    /// The providers whose tokens do not count toward it (subscriptions).
    free: BTreeSet<String>,
    kill_switch: bool,
    aborts_running: bool,
    paused: BTreeSet<String>,
    /// The approved models, `(provider id, model)`; a reply asks for one of these or is refused (ADR 0014).
    catalog: BTreeSet<(String, String)>,
    /// The same, as the server is shown it, with the default.
    choices: askbar_proto::ModelList,
}

/// A reply that is running, so a stop can reach it: whose it is, and the channel that says why it must end.
struct Running {
    user_id: String,
    id: String,
    stop: watch::Sender<Option<&'static str>>,
}

struct OpenTurn {
    span_id: String,
    deadline: Instant,
}

pub struct Gateway {
    pub boot_id: BootId,
    audit: Arc<AuditClient>,
    http: llm::HttpClient,
    config: RwLock<Option<Arc<Applied>>>,
    turns: Mutex<Vec<OpenTurn>>,
    pub restarter: Restarter,
    /// What the budget is held against (plan step 3.2).
    usage: usage::Book,
    reader: std::sync::OnceLock<Arc<usage::Reader>>,
    /// The replies running now, for the kill switch and a user's pause to stop.
    running: Mutex<Vec<Running>>,
}

impl Gateway {
    pub fn new(audit: Arc<AuditClient>, boot_id: BootId, restarter: Restarter) -> Arc<Gateway> {
        Arc::new(Gateway {
            boot_id,
            audit,
            http: llm::http_client(),
            config: RwLock::new(None),
            turns: Mutex::new(Vec::new()),
            restarter,
            usage: usage::Book::default(),
            reader: std::sync::OnceLock::new(),
            running: Mutex::new(Vec::new()),
        })
    }

    /// Where the gateway reads the day's and month's token counts. Without one a budget can never be read, so a request
    /// that a budget applies to is refused.
    pub fn set_usage_reader(&self, reader: usage::Reader) {
        let _ = self.reader.set(Arc::new(reader));
    }

    /// Read the auditor's totals for the day and the month now. Returns whether it did.
    pub async fn refresh_usage(&self) -> bool {
        let (Some(reader), Some(cfg)) = (self.reader.get(), self.applied()) else { return false };
        let now = askbar_service::audit_client::now_parts().0;
        let (daily, monthly) = tokio::join!(
            reader.metered_total(askbar_service::periods::day_start(now), &cfg.free),
            reader.metered_total(askbar_service::periods::month_start(now), &cfg.free)
        );
        match (daily, monthly) {
            (Some(d), Some(m)) => {
                self.usage.set_view(now, d, m);
                true
            }
            _ => false,
        }
    }

    /// Keep the view fresh for as long as the process runs: soon after start (and again until it works), then every minute.
    pub async fn watch_usage(self: Arc<Self>) {
        loop {
            let ok = self.refresh_usage().await;
            tokio::time::sleep(if ok { usage::REFRESH_EVERY } else { Duration::from_secs(3) }).await;
        }
    }

    pub fn audit(&self) -> &Arc<AuditClient> {
        &self.audit
    }

    fn applied(&self) -> Option<Arc<Applied>> {
        self.config.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The provider with this id, as configured (key included), if there is one.
    pub fn provider(&self, id: &str) -> Option<Provider> {
        self.applied()?.providers.get(id).cloned()
    }

    pub fn http(&self) -> &llm::HttpClient {
        &self.http
    }

    pub fn config_version(&self) -> Option<u64> {
        self.applied().map(|a| a.version)
    }

    /// Finish the turn spans the server never finished, once their time has passed (ADR 0054). The auditor takes
    /// the first finish of a span, so one the server did send is the one that counts.
    pub fn finish_stale_turns(&self, now: Instant) -> usize {
        let stale: Vec<String> = {
            let mut turns = self.turns.lock().unwrap_or_else(|e| e.into_inner());
            let (stale, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut *turns).into_iter().partition(|t| t.deadline <= now);
            *turns = keep;
            stale.into_iter().map(|t| t.span_id).collect()
        };
        for id in &stale {
            let mut f = finish("interrupted");
            f.error_code = Some("stale_turn".into());
            self.audit.finish(id, f);
        }
        stale.len()
    }

    /// Check for stale turns every few seconds, for as long as the process runs.
    pub async fn watch_turns(self: Arc<Self>) {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            self.finish_stale_turns(Instant::now());
        }
    }
}

fn finish(status: &str) -> SpanFinish {
    let (at, ms) = now_parts();
    SpanFinish {
        status: status.into(),
        ended_at: at,
        ended_ms: ms,
        provider_id: None,
        model: None,
        tokens_in: None,
        tokens_out: None,
        tokens_cached: None,
        ttft_ms: None,
        error_code: None,
        attrs: None,
    }
}

// ------------------------------------------------------------------ the links

/// What one link may call: methods and paths after `/api/model-gateway/v1`.
type Routes = &'static [(&'static str, &'static str)];

const SERVER_ROUTES: Routes = &[("POST", "/chat"), ("GET", "/catalog")];
const ADMIN_ROUTES: Routes =
    &[("GET", "/status"), ("PUT", "/config"), ("POST", "/restart"), ("POST", "/models"), ("POST", "/providers/*/test")];

/// The two links: name as `bootstrap` names it (the file `accept/<name>.sha256`), the caller, and the routes.
pub const LINKS: &[(&str, &str, Routes)] =
    &[("server-model-gateway", "server", SERVER_ROUTES), ("admin-model-gateway", "admin", ADMIN_ROUTES)];

fn full(rest: &str) -> String {
    paths::join(MODEL_GATEWAY_V1, rest)
}

fn links_for(token_dir: &FsPath, link: &'static (&'static str, &'static str, Routes)) -> Links {
    let (name, caller, routes) = link;
    Links::new(vec![Link {
        caller,
        hash_file: token_dir.join("accept").join(format!("{name}.sha256")),
        allow: routes.iter().map(|(m, p)| Allow::method(Method::from_bytes(m.as_bytes()).expect("a method"), &full(p))).collect(),
    }])
    .named("model-gateway")
}

/// The check on the server's listener: only the server's token, only `/chat`.
pub fn server_links(token_dir: &FsPath) -> Links {
    links_for(token_dir, &LINKS[0])
}

/// The check on the admin's listener: only the admin's token, only status, config and restart.
pub fn admin_links(token_dir: &FsPath) -> Links {
    links_for(token_dir, &LINKS[1])
}

/// The credential volume, read-only: `ASKBAR_GATEWAY_TOKEN_DIR`, default `/model-gateway-token`.
pub fn token_dir() -> PathBuf {
    std::env::var("ASKBAR_GATEWAY_TOKEN_DIR")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map_or_else(|| "/model-gateway-token".into(), PathBuf::from)
}

// ------------------------------------------------------------------ the routes

/// One route, for the table the OpenAPI test reads.
pub struct RouteEntry {
    pub method: &'static str,
    pub path: String,
}

/// Every route, method and full path, across both listeners. The OpenAPI file describes exactly these.
pub fn routes() -> Vec<RouteEntry> {
    let mut out = vec![RouteEntry { method: "GET", path: full("/health") }];
    for (_, _, rs) in LINKS {
        out.extend(rs.iter().map(|(m, p)| RouteEntry { method: m, path: full(p) }));
    }
    out
}

/// The server's listener.
pub fn server_router(gw: Arc<Gateway>, links: Links) -> Router {
    let protected = Router::new()
        .route(&full("/chat"), post(chat))
        .route(&full("/catalog"), get(catalog))
        .layer(axum::middleware::from_fn_with_state(links, askbar_service::links::require));
    shell(gw, protected)
}

/// The admin's listener.
pub fn admin_router(gw: Arc<Gateway>, links: Links) -> Router {
    let restart = gw.restarter.clone();
    let protected = Router::new()
        .route(&full("/status"), get(status))
        .route(&full("/config"), put(put_config))
        .route(&full("/models"), post(list_models))
        .route(&full("/providers/{id}/test"), post(test_provider))
        .route(&full("/restart"), post(askbar_service::restart::handler).with_state(restart))
        .layer(axum::middleware::from_fn_with_state(links, askbar_service::links::require));
    shell(gw, protected)
}

fn shell(gw: Arc<Gateway>, protected: Router<Arc<Gateway>>) -> Router {
    Router::new()
        .route(&full("/health"), get(|| async { "ok" }))
        .merge(protected)
        .fallback(|| async { ApiError::code(ErrorCode::NotFound) })
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(gw)
}

type Failure = ApiError;

fn bad(detail: impl Into<String>) -> Failure {
    ApiError::new(ErrorCode::BadRequest, detail)
}

async fn status(State(gw): State<Arc<Gateway>>) -> Json<serde_json::Value> {
    Json(askbar_service::status::body(gw.config_version(), &gw.boot_id))
}

// ------------------------------------------------------------------ models and the provider test

/// `GET /catalog`: the approved models and the default, for the server to offer clients and to check a send against.
async fn catalog(State(gw): State<Arc<Gateway>>) -> Result<Json<askbar_proto::ModelList>, Failure> {
    let Some(cfg) = gw.applied() else {
        return Err(ApiError::new(ErrorCode::NotConfigured, "The gateway has not received its settings yet. Try again in a few seconds.")
            .retry_after(5));
    };
    Ok(Json(cfg.choices.clone()))
}

/// `POST /models`: the models a provider offers. Always `200`; `ok: false` and a sentence when it could not say.
async fn list_models(State(gw): State<Arc<Gateway>>, Json(req): Json<models::ModelsRequest>) -> Json<models::ModelsResult> {
    Json(models::run(&gw, req).await)
}

/// `POST /providers/{id}/test`: one short message through the provider, and how it went.
async fn test_provider(
    State(gw): State<Arc<Gateway>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<provider_test::TestResult>, Failure> {
    provider_test::run(&gw, &id).await.map(Json).ok_or_else(|| ApiError::new(ErrorCode::NotFound, "No such provider."))
}

// ------------------------------------------------------------------ PUT /config

fn check_config(c: &GatewayConfig) -> Result<Applied, String> {
    let key = c.audit_public_key.as_ref().ok_or("audit_public_key is missing: the gateway could not record what a provider says")?;
    let bytes = seal::parse_hex_key(&key.public_key).ok_or("audit_public_key.public_key must be 32 bytes in hex")?;
    if key.key_id != seal::key_id(&bytes) {
        return Err("audit_public_key.key_id does not match the key".into());
    }
    let mut providers = BTreeMap::new();
    let mut free = BTreeSet::new();
    for p in &c.providers {
        if p.id.is_empty() || p.id.len() > 64 {
            return Err("A provider id must be 1 to 64 characters.".into());
        }
        if p.api_key.chars().count() > MAX_KEY_CHARS {
            return Err(format!("A provider's key must be at most {MAX_KEY_CHARS} characters."));
        }
        let kind = match p.kind.as_str() {
            "openai" => ProviderKind::OpenAi,
            "anthropic" => ProviderKind::Anthropic,
            _ => return Err("A provider's type is openai or anthropic.".into()),
        };
        let provider = Provider {
            name: p.name.clone(),
            kind,
            base_url: p.base_url.clone(),
            api_key: p.api_key.clone(),
            model: p.model.clone(),
            max_tokens: p.max_tokens,
            temperature: p.temperature,
            headers: p.headers.clone(),
        };
        if providers.insert(p.id.clone(), provider).is_some() {
            return Err("Two providers have the same id.".into());
        }
        if p.billing == askbar_proto::Billing::Subscription {
            free.insert(p.id.clone());
        }
    }
    if c.paused_users.len() > 10_000 || c.paused_users.iter().any(|u| !valid_id(u)) {
        return Err("A paused user is an id of 1 to 64 letters, digits, - or _ (at most 10,000 of them).".into());
    }
    let mut catalog = BTreeSet::new();
    let mut choices = Vec::new();
    for e in &c.catalog {
        let Some(p) = providers.get(&e.provider_id) else {
            return Err("The catalog names a provider that is not in the configuration.".into());
        };
        if e.model.trim().is_empty() || e.model.chars().count() > MAX_MODEL_CHARS || e.model.chars().any(char::is_control) {
            return Err(format!("A model name is 1 to {MAX_MODEL_CHARS} characters, with no control characters."));
        }
        if !catalog.insert((e.provider_id.clone(), e.model.clone())) {
            return Err("The catalog lists a model twice.".into());
        }
        choices.push(askbar_proto::ModelChoice {
            id: e.id(),
            provider_id: e.provider_id.clone(),
            provider_name: p.name.clone(),
            model: e.model.clone(),
        });
    }
    // One default, in the catalog, whenever there is a catalog (ADR 0014); none for an empty one.
    match (&c.default_model, catalog.is_empty()) {
        (None, true) => {}
        (None, false) => return Err("The catalog needs a default model.".into()),
        (Some(_), true) => return Err("An empty catalog has no default model.".into()),
        (Some(d), false) => {
            if !choices.iter().any(|m| &m.id == d) {
                return Err("The default model is not in the catalog.".into());
            }
        }
    }
    Ok(Applied {
        version: c.version,
        providers,
        audit_key: bytes,
        timeout: Duration::from_secs(u64::from(c.generation_timeout_secs)),
        budget: c.budget.clone(),
        free,
        kill_switch: c.kill_switch,
        aborts_running: c.kill_switch_aborts_running,
        paused: c.paused_users.iter().cloned().collect(),
        catalog,
        choices: askbar_proto::ModelList { models: choices, default: c.default_model.clone() },
    })
}

impl Gateway {
    /// Put `c` in place of the configuration, or say why not (a sentence for a person). The keys it carries are held in
    /// memory and nowhere else.
    pub fn apply_config(&self, c: &GatewayConfig) -> Result<(), String> {
        let applied = check_config(c)?;
        let version = applied.version;
        // The stops reach the replies already running (the kill switch if it is set to, and each paused user's), the
        // moment the configuration is in place, so a stop does not wait for the next request.
        let stop_all = applied.kill_switch && applied.aborts_running;
        let paused = applied.paused.clone();
        *self.config.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(applied));
        for r in self.running.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            if stop_all {
                let _ = r.stop.send(Some("kill_switch"));
            } else if paused.contains(&r.user_id) {
                let _ = r.stop.send(Some("user_paused"));
            }
        }
        tracing::info!(version, providers = c.providers.len(), "configuration applied");
        self.audit.config_applied(version as i64);
        Ok(())
    }
}

/// Replace the configuration. The keys it carries are held in memory and nowhere else; the answer holds none.
async fn put_config(
    State(gw): State<Arc<Gateway>>,
    Extension(Caller(_)): Extension<Caller>,
    Json(c): Json<GatewayConfig>,
) -> Result<Json<serde_json::Value>, Failure> {
    gw.apply_config(&c).map_err(bad)?;
    Ok(Json(serde_json::json!({ "version": c.version })))
}

// ------------------------------------------------------------------ POST /chat

fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn refused(reason: Refused) -> Failure {
    let why = match reason {
        Refused::Unreachable => "The audit record could not be made in time, so no model was called. Try again in a few seconds.",
        Refused::Backlog => "The auditor is busy recording, so no model was called. Try again in a few seconds.",
        Refused::Rejected => "The audit record was refused, so no model was called. The administrator should look at the gateway's log.",
    };
    ApiError::new(ErrorCode::AuditBacklog, why).retry_after(5)
}

fn start_of(id: &str, trace: &str, parent: Option<&str>, kind: SpanKind, name: &str) -> SpanStart {
    let (at, ms) = now_parts();
    SpanStart {
        id: id.into(),
        trace_id: trace.into(),
        parent_id: parent.map(str::to_string),
        kind,
        name: name.into(),
        started_at: at,
        started_ms: ms,
        provider_id: None,
        model: None,
        user_id: None,
        client_id: None,
        attrs: None,
    }
}

/// A refusal at a gate, on record when the auditor is back: a span that began and ended at once, `blocked`.
fn refuse_at_gate(gw: &Gateway, req: &ChatRequest, name: &str, error_code: &str, attrs: serde_json::Value) {
    let id = askbar_service::ids::new_id();
    let mut start = start_of(&id, &req.generation_id, None, SpanKind::Gate, name);
    start.user_id = Some(req.user_id.clone());
    start.client_id = req.client_id.clone();
    start.attrs = Some(attrs);
    let mut f = finish("blocked");
    f.error_code = Some(error_code.into());
    gw.audit.refusal(start, f);
}

impl Gateway {
    /// The kill switch, the user's pause and the budget, in that order. `Err` is the refusal to answer with.
    fn gates(&self, cfg: &Applied, req: &ChatRequest) -> Result<(), Failure> {
        if cfg.kill_switch {
            refuse_at_gate(self, req, "kill_switch", "kill_switch", serde_json::json!({ "kill_switch": true }));
            return Err(ApiError::new(ErrorCode::KillSwitch, "Replies have been switched off by the administrator for now."));
        }
        if cfg.paused.contains(&req.user_id) {
            refuse_at_gate(self, req, "user_pause", "user_paused", serde_json::json!({ "paused": true }));
            return Err(ApiError::new(ErrorCode::UserPaused, "Replies are paused for this user by the administrator."));
        }
        // Only an approved model is called, whatever the server asked (ADR 0014). The model's name is not recorded in the
        // gate: it is the caller's text, and the catalog already says what is approved.
        if !cfg.catalog.contains(&(req.provider_id.clone(), req.model.clone())) {
            refuse_at_gate(self, req, "model_catalog", "model_not_approved", serde_json::json!({ "approved": false }));
            return Err(ApiError::new(ErrorCode::ModelNotApproved, "That model is not approved. Pick another one."));
        }
        let (daily, monthly) = (cfg.budget.daily_tokens, cfg.budget.monthly_tokens);
        if (daily.is_none() && monthly.is_none()) || cfg.free.contains(&req.provider_id) {
            return Ok(());
        }
        let used = self.usage.used(now_parts().0);
        // A budget that cannot be read cannot be held: ask again in a few seconds, with no provider called.
        if (daily.is_some() && used.daily.is_none()) || (monthly.is_some() && used.monthly.is_none()) {
            return Err(ApiError::new(
                ErrorCode::AuditBacklog,
                "The gateway has not read how many tokens have been used yet, so no model was called. Try again in a few seconds.",
            )
            .retry_after(5));
        }
        let note = serde_json::json!({
            "daily_used": used.daily, "daily_limit": daily, "monthly_used": used.monthly, "monthly_limit": monthly,
        });
        for (period, limit, used) in [("daily", daily, used.daily), ("monthly", monthly, used.monthly)] {
            if limit.zip(used).is_some_and(|(limit, used)| used >= limit) {
                refuse_at_gate(self, req, "budget", "budget_exceeded", note);
                return Err(ApiError::new(
                    ErrorCode::BudgetExceeded,
                    if period == "monthly" {
                        "The usage limit for this month has been reached."
                    } else {
                        "The usage limit for today has been reached."
                    },
                )
                .with("period", period));
            }
        }
        Ok(())
    }

    /// Registers a reply that is about to run, so a stop can reach it. A stop that came in since the gate is delivered at
    /// once, so there is no moment in which a reply is neither refused nor stoppable.
    fn register(self: &Arc<Self>, user_id: &str, id: &str) -> (Registered, watch::Receiver<Option<&'static str>>) {
        let (stop, rx) = watch::channel(None);
        self.running.lock().unwrap_or_else(|e| e.into_inner()).push(Running { user_id: user_id.to_string(), id: id.to_string(), stop });
        if let Some(cfg) = self.applied() {
            let reason = if cfg.kill_switch {
                Some("kill_switch")
            } else if cfg.paused.contains(user_id) {
                Some("user_paused")
            } else {
                None
            };
            if let Some(reason) = reason {
                if let Some(r) = self.running.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|r| r.id == id) {
                    let _ = r.stop.send(Some(reason));
                }
            }
        }
        (Registered { gw: self.clone(), id: id.to_string() }, rx)
    }
}

/// Takes a reply off the running list when it ends, however it ends.
struct Registered {
    gw: Arc<Gateway>,
    id: String,
}

impl Drop for Registered {
    fn drop(&mut self) {
        self.gw.running.lock().unwrap_or_else(|e| e.into_inner()).retain(|r| r.id != self.id);
    }
}

fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

async fn chat(
    State(gw): State<Arc<Gateway>>,
    Json(req): Json<ChatRequest>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, Failure> {
    let Some(cfg) = gw.applied() else {
        return Err(ApiError::new(ErrorCode::NotConfigured, "The gateway has not received its settings yet. Try again in a few seconds.")
            .retry_after(5));
    };
    if !valid_id(&req.generation_id) || !valid_id(&req.user_id) {
        return Err(bad("generation_id and user_id are 1 to 64 letters, digits, - or _."));
    }
    let Some(mut provider) = cfg.providers.get(&req.provider_id).cloned() else {
        return Err(ApiError::new(ErrorCode::NotFound, "No such provider."));
    };
    // The model asked for; `gates` below refuses it unless the catalog approves it for this provider.
    provider.model = req.model.clone();
    let messages: Vec<ChatMessage> = req.messages.iter().map(|m| ChatMessage::new(&m.role, &m.content)).collect();
    if llm::normalize(&messages).is_empty() {
        return Err(bad("There is nothing to send."));
    }
    // The stops and the budget, from this gateway's own copy: the authoritative one (ADR 0014). Each refusal is a gate span
    // in the generation's trace, and nothing of a call is on record before it.
    gw.gates(&cfg, &req)?;

    // The turn span, on record before any provider is called (ADR 0010 rules 1 to 3).
    let turn_id = askbar_service::ids::new_id();
    let mut turn = start_of(&turn_id, &req.generation_id, None, SpanKind::Turn, "chat");
    turn.provider_id = Some(req.provider_id.clone());
    turn.model = Some(provider.model.clone());
    turn.user_id = Some(req.user_id.clone());
    turn.client_id = req.client_id.clone();
    turn.attrs = Some(serde_json::json!({ "space_id": req.space_id, "conversation_id": req.conversation_id }));
    if let Err(why) = gw.audit.open(&turn).await {
        // Record the refusal when the auditor is back, and call no provider.
        let mut f = finish("refused");
        f.error_code = Some("audit_backlog".into());
        gw.audit.refusal(turn, f);
        return Err(refused(why));
    }
    gw.turns
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(OpenTurn { span_id: turn_id.clone(), deadline: Instant::now() + cfg.timeout + STALE_TURN_GRACE });

    let (tx, rx) = mpsc::unbounded_channel::<ChatEvent>();
    let _ = tx.send(ChatEvent::Span { span_id: turn_id.clone(), trace_id: req.generation_id.clone() });
    let (registered, stop_rx) = gw.register(&req.user_id, &turn_id);
    tokio::spawn(run_attempts(gw.clone(), cfg, provider, req, turn_id, messages, tx, stop_rx, registered));
    let stream = futures_util::StreamExt::map(tokio_stream::wrappers::UnboundedReceiverStream::new(rx), |e: ChatEvent| {
        Ok::<_, Infallible>(Event::default().event(e.name()).json_data(&e).expect("an event is JSON"))
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

/// Tries the provider, and up to `MAX_RETRIES` more times when the failure is worth it. Nothing is retried once
/// text has gone to the caller. The caller going away (its stream closing) cancels the request in flight.
#[allow(clippy::too_many_arguments)]
async fn run_attempts(
    gw: Arc<Gateway>,
    cfg: Arc<Applied>,
    provider: Provider,
    req: ChatRequest,
    turn_id: String,
    messages: Vec<ChatMessage>,
    tx: mpsc::UnboundedSender<ChatEvent>,
    stop_rx: watch::Receiver<Option<&'static str>>,
    _registered: Registered,
) {
    let mut sent_text = false;
    let mut retry_index = 0u32;
    let system = req.system.clone();
    loop {
        // A stop that came while the last try waited, or before the first: no more tries (and no provider is called).
        if let Some(reason) = *stop_rx.borrow() {
            let _ = tx.send(ChatEvent::Error {
                code: reason.into(),
                class: FailureClass::Fatal,
                retry_after_secs: None,
                attempts: retry_index,
            });
            return;
        }
        let attempt_id = askbar_service::ids::new_id();
        let mut start = start_of(&attempt_id, &req.generation_id, Some(&turn_id), SpanKind::Attempt, &provider.name);
        start.provider_id = Some(req.provider_id.clone());
        start.model = Some(provider.model.clone());
        start.user_id = Some(req.user_id.clone());
        start.client_id = req.client_id.clone();
        start.attrs = Some(serde_json::json!({ "retry_index": retry_index }));
        // Every try is on record before it is made.
        if gw.audit.open(&start).await.is_err() {
            let _ = tx.send(ChatEvent::Error {
                code: "audit_backlog".into(),
                class: FailureClass::Retryable,
                retry_after_secs: Some(5),
                attempts: retry_index,
            });
            return;
        }

        // The caller going away cancels the request: a watcher holds the cancel sender until then.
        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
        // ...and so does a stop from the gateway's own copy (the kill switch, or this user's pause), which says why.
        let stopped: Arc<Mutex<Option<&'static str>>> = Arc::default();
        let watcher = {
            let tx = tx.clone();
            let mut rx = stop_rx.clone();
            let stopped = stopped.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = tx.closed() => break,
                        changed = rx.changed() => {
                            if changed.is_err() {
                                // The sender is gone: nothing will stop this reply but its caller.
                                tx.closed().await;
                                break;
                            }
                            let reason = *rx.borrow();
                            if reason.is_some() {
                                *stopped.lock().unwrap_or_else(|e| e.into_inner()) = reason;
                                break;
                            }
                        }
                    }
                }
                let _ = cancel_tx.send(());
            })
        };
        let delta_tx = tx.clone();
        let mut text_this_try = false;
        let result = llm::stream_chat_report(&gw.http, &provider, &system, &messages, cancel_rx, |d| {
            text_this_try = true;
            let _ = delta_tx.send(ChatEvent::Delta { text: d.to_string() });
        })
        .await;
        watcher.abort();
        sent_text |= text_this_try;
        let stopped_by = *stopped.lock().unwrap_or_else(|e| e.into_inner());

        let mut end = finish("done");
        end.provider_id = Some(req.provider_id.clone());
        end.model = Some(provider.model.clone());
        match result {
            Ok(r) => {
                end.status = match r.outcome {
                    StreamOutcome::Completed => "done",
                    StreamOutcome::Cancelled => "cancelled",
                }
                .into();
                end.ttft_ms = r.time_to_first_token.map(|d| d.as_millis() as u64);
                if let Some(u) = r.usage {
                    (end.tokens_in, end.tokens_out, end.tokens_cached) = (u.input, u.output, u.cache_read);
                }
                end.attrs = Some(attempt_attrs(&r, retry_index));
                if let (StreamOutcome::Cancelled, Some(reason)) = (r.outcome, stopped_by) {
                    // Stopped by the kill switch or this user's pause: say why, here and to the caller.
                    end.error_code = Some(reason.into());
                    gw.audit.finish(&attempt_id, end);
                    let _ = tx.send(ChatEvent::Error {
                        code: reason.into(),
                        class: FailureClass::Fatal,
                        retry_after_secs: None,
                        attempts: retry_index + 1,
                    });
                    return;
                }
                gw.audit.finish(&attempt_id, end);
                if r.outcome == StreamOutcome::Completed {
                    // What this reply used counts toward the budget, unless the provider is a subscription.
                    if let Some(u) = r.usage.filter(|_| !cfg.free.contains(&req.provider_id)) {
                        gw.usage.count(now_parts().0, u.input.unwrap_or(0) + u.output.unwrap_or(0));
                    }
                    if let Some(u) = r.usage {
                        let _ = tx.send(ChatEvent::Usage {
                            input: u.input,
                            output: u.output,
                            cache_read: u.cache_read,
                            cache_write: u.cache_write,
                            estimated: u.estimated,
                        });
                    }
                    let _ = tx.send(ChatEvent::Done {
                        finish_reason: r.finish_reason,
                        ttft_ms: r.time_to_first_token.map(|d| d.as_millis() as u64),
                        attempts: retry_index + 1,
                    });
                }
                return;
            }
            Err(e) => {
                // The provider's own words go to the auditor sealed, cut at 4 KB, and nowhere else.
                if let Ok(sealed) = seal::seal(&cfg.audit_key, cut(&e.error.to_string(), MAX_ERROR_PAYLOAD_BYTES).as_bytes()) {
                    gw.audit.payload(&attempt_id, "error", &sealed.key_id, &sealed.bytes);
                }
                let code = router::error_code(&e.error);
                end.status = "error".into();
                end.error_code = Some(code.clone());
                end.ttft_ms = e.report.time_to_first_token.map(|d| d.as_millis() as u64);
                end.attrs = Some(attempt_attrs(&e.report, retry_index));
                gw.audit.finish(&attempt_id, end);

                let class = router::classify(&e.error);
                let out_of_tries = retry_index >= MAX_RETRIES;
                if sent_text || class != FailureClass::Retryable || out_of_tries {
                    let _ = tx.send(ChatEvent::Error {
                        code,
                        class,
                        retry_after_secs: e.report.retry_after.map(|d| d.as_secs()),
                        attempts: retry_index + 1,
                    });
                    return;
                }
                let wait = match e.report.retry_after {
                    Some(ra) if ra <= Duration::from_secs(5) => ra,
                    _ => RETRY_BACKOFF[retry_index as usize],
                };
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = tx.closed() => return,
                }
            }
        }
        retry_index += 1;
    }
}

fn attempt_attrs(r: &llm::StreamReport, retry_index: u32) -> serde_json::Value {
    serde_json::json!({
        "retry_index": retry_index,
        "latency_ms": r.total.as_millis() as u64,
        "http_status": r.http_status,
        "usage_estimated": r.usage.map(|u| u.estimated),
        "finish_reason": r.finish_reason,
    })
}
