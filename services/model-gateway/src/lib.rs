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
pub mod router;
pub mod seal;

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
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// The port both listeners use, on the address each caller's network gives them.
pub const DEFAULT_PORT: u16 = 8084;
const MAX_BODY: usize = 4 * 1024 * 1024;
const MAX_KEY_CHARS: usize = 4096;
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
}

impl Gateway {
    pub fn new(audit: Arc<AuditClient>, boot_id: BootId, restarter: Restarter) -> Arc<Gateway> {
        Arc::new(Gateway { boot_id, audit, http: llm::http_client(), config: RwLock::new(None), turns: Mutex::new(Vec::new()), restarter })
    }

    pub fn audit(&self) -> &Arc<AuditClient> {
        &self.audit
    }

    fn applied(&self) -> Option<Arc<Applied>> {
        self.config.read().unwrap_or_else(|e| e.into_inner()).clone()
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

const SERVER_ROUTES: Routes = &[("POST", "/chat")];
const ADMIN_ROUTES: Routes = &[("GET", "/status"), ("PUT", "/config"), ("POST", "/restart")];

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
    let protected =
        Router::new().route(&full("/chat"), post(chat)).layer(axum::middleware::from_fn_with_state(links, askbar_service::links::require));
    shell(gw, protected)
}

/// The admin's listener.
pub fn admin_router(gw: Arc<Gateway>, links: Links) -> Router {
    let restart = gw.restarter.clone();
    let protected = Router::new()
        .route(&full("/status"), get(status))
        .route(&full("/config"), put(put_config))
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

// ------------------------------------------------------------------ PUT /config

fn check_config(c: &GatewayConfig) -> Result<Applied, String> {
    let key = c.audit_public_key.as_ref().ok_or("audit_public_key is missing: the gateway could not record what a provider says")?;
    let bytes = seal::parse_hex_key(&key.public_key).ok_or("audit_public_key.public_key must be 32 bytes in hex")?;
    if key.key_id != seal::key_id(&bytes) {
        return Err("audit_public_key.key_id does not match the key".into());
    }
    let mut providers = BTreeMap::new();
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
    }
    Ok(Applied { version: c.version, providers, audit_key: bytes, timeout: Duration::from_secs(u64::from(c.generation_timeout_secs)) })
}

/// Replace the configuration. The keys it carries are held in memory and nowhere else; the answer holds none.
async fn put_config(
    State(gw): State<Arc<Gateway>>,
    Extension(Caller(_)): Extension<Caller>,
    Json(c): Json<GatewayConfig>,
) -> Result<Json<serde_json::Value>, Failure> {
    let applied = check_config(&c).map_err(bad)?;
    let version = applied.version;
    *gw.config.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(applied));
    tracing::info!(version, providers = c.providers.len(), "configuration applied");
    Ok(Json(serde_json::json!({ "version": version })))
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
    let Some(provider) = cfg.providers.get(&req.provider_id).cloned() else {
        return Err(ApiError::new(ErrorCode::NotFound, "No such provider."));
    };
    let messages: Vec<ChatMessage> = req.messages.iter().map(|m| ChatMessage::new(&m.role, &m.content)).collect();
    if llm::normalize(&messages).is_empty() {
        return Err(bad("There is nothing to send."));
    }

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
    tokio::spawn(run_attempts(gw.clone(), cfg, provider, req, turn_id, messages, tx));
    let stream = futures_util::StreamExt::map(tokio_stream::wrappers::UnboundedReceiverStream::new(rx), |e: ChatEvent| {
        Ok::<_, Infallible>(Event::default().event(e.name()).json_data(&e).expect("an event is JSON"))
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

/// Tries the provider, and up to `MAX_RETRIES` more times when the failure is worth it. Nothing is retried once
/// text has gone to the caller. The caller going away (its stream closing) cancels the request in flight.
async fn run_attempts(
    gw: Arc<Gateway>,
    cfg: Arc<Applied>,
    provider: Provider,
    req: ChatRequest,
    turn_id: String,
    messages: Vec<ChatMessage>,
    tx: mpsc::UnboundedSender<ChatEvent>,
) {
    let mut sent_text = false;
    let mut retry_index = 0u32;
    let system = req.system.clone();
    loop {
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
        let watcher = {
            let tx = tx.clone();
            tokio::spawn(async move {
                tx.closed().await;
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
                gw.audit.finish(&attempt_id, end);
                if r.outcome == StreamOutcome::Completed {
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
