//! `Remote`: the client for an Askbar server over HTTPS.

use crate::client_config::{check_server_url, merge_ui, ClientConfig};
use crate::outbox::{Outbox, OutboxEntry};
use crate::tokens::TokenStore;
use crate::types::{ClientStatus, Emit, PendingReply, ProviderChoice, Settings, StreamEvent};
use askbar_proto::{
    ChatEvent, ChatRequest, Conversation, ConversationDetail, DisplaySettings, Message, PendingGeneration, Prefs, ProviderInfo, SpaceInfo,
    SseParser,
};
use futures_util::{FutureExt, StreamExt};
use reqwest::{Method, RequestBuilder, StatusCode, Url};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::oneshot;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Health and status checks must not make the window feel stuck.
const CHECK_TIMEOUT: Duration = Duration::from_secs(2);

struct Inner {
    cfg: Mutex<ClientConfig>,
    path: PathBuf,
    base: Url,
    http: reqwest::Client,
    tokens: Box<dyn TokenStore>,
    /// One "second" of the resume backoff; tests shrink it.
    backoff_unit: Duration,
    /// Messages sent and not yet seen by the server.
    outbox: Outbox,
    /// The space this app works in: the user's personal space (ADR 0009), found with `GET /spaces`
    /// the first time it is needed and kept for this pairing.
    space: Mutex<Option<String>>,
}

/// Wait before resume attempt `n` (from 0): 1, 2, 4, 8, 16, then 30 seconds.
fn backoff_secs(n: u32) -> u64 {
    [1, 2, 4, 8, 16].get(n as usize).copied().unwrap_or(30)
}

pub struct Remote {
    inner: Result<Inner, String>,
}

enum Read {
    /// done or an error event was reported to the caller.
    Finished,
    /// Stop was pressed and the server's generation id is known: ask the server to stop it.
    Stop,
    /// The stream ended or broke before done; the reason, for when there is nothing to resume.
    Lost(String),
}

type Cancel = futures_util::future::Fuse<oneshot::Receiver<()>>;

/// Reads one response of a reply until it ends, reporting events as they arrive.
async fn read_reply(resp: reqwest::Response, reader: &mut ReplyReader, cancel: &mut Cancel, stop: &mut bool, emit: Emit<'_>) -> Read {
    let mut stream = Box::pin(resp.bytes_stream());
    loop {
        if *stop && reader.generation_id.is_some() {
            return Read::Stop;
        }
        let chunk = tokio::select! {
            c = stream.next() => c,
            _ = &mut *cancel => {
                *stop = true;
                continue;
            }
        };
        let bytes = match chunk {
            None => return Read::Lost("The connection to the server ended before the reply was complete.".into()),
            Some(Err(e)) => return Read::Lost(format!("The connection to the server was lost: {e}")),
            Some(Ok(bytes)) => bytes,
        };
        for step in reader.feed(&bytes) {
            match step {
                Step::Emit(e) => emit(e),
                Step::Done => {
                    emit(StreamEvent::Done);
                    return Read::Finished;
                }
                Step::Fail { message, code } => {
                    emit(server_error(message, code));
                    return Read::Finished;
                }
            }
        }
    }
}

/// The server's last word on a failed reply. Its `code` (timed_out, interrupted, kill_switch,
/// provider_error, internal) becomes the event's kind; the words are only for display. A reply that
/// ran out of time reads "Stopped after N ..." and the window keeps the text that arrived and offers
/// to try again.
fn server_error(message: String, code: Option<String>) -> StreamEvent {
    match code.as_deref() {
        Some("timed_out") => {
            let took = message.strip_prefix("Timed out after ").filter(|t| !t.trim().is_empty());
            let words = took.map(|t| format!("Stopped after {t}")).unwrap_or(message);
            StreamEvent::error_kind("timed_out", words)
        }
        Some(other) => StreamEvent::error_kind(other, message),
        None => StreamEvent::error(message),
    }
}

/// What one chunk of a reply stream means for the caller.
#[derive(Debug, PartialEq)]
enum Step {
    Emit(StreamEvent),
    /// The reply is complete.
    Done,
    /// The server ended the reply with an error; `code` says why, when the server says.
    Fail {
        message: String,
        code: Option<String>,
    },
}

/// Reads a reply stream and remembers where it got to, so a dropped stream can be resumed
/// with `?after=<last_id>`. Deltas carry `id: 1, 2, 3...`, so the last id seen is the number
/// of deltas received.
#[derive(Default)]
struct ReplyReader {
    parser: SseParser,
    /// Deltas received so far; the `after` to resume from.
    last_id: u64,
    generation_id: Option<String>,
}

impl ReplyReader {
    fn feed(&mut self, bytes: &[u8]) -> Vec<Step> {
        let mut steps = Vec::new();
        for ev in self.parser.feed(bytes) {
            match serde_json::from_str::<ChatEvent>(&ev.data) {
                Ok(ChatEvent::Meta { conversation_id, generation_id, queued }) => {
                    if generation_id.is_some() {
                        self.generation_id = generation_id.clone();
                    }
                    steps.push(Step::Emit(StreamEvent::Started { conversation_id, generation_id, queued }));
                }
                Ok(ChatEvent::Provider { id, name }) => steps.push(Step::Emit(StreamEvent::Provider { id, name })),
                Ok(ChatEvent::Delta { text }) => {
                    self.last_id += 1;
                    steps.push(Step::Emit(StreamEvent::Delta { text }));
                }
                Ok(ChatEvent::Done) => steps.push(Step::Done),
                Ok(ChatEvent::Error { message, code }) => steps.push(Step::Fail { message, code }),
                Err(_) => {} // unknown event from a newer server: ignore
            }
        }
        steps
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Remote {
    /// A bad `client.yaml` doesn't stop the app; every call reports the problem instead.
    pub fn new(cfg: Result<ClientConfig, String>, path: PathBuf, tokens: Box<dyn TokenStore>) -> Remote {
        let inner = cfg.and_then(|cfg| {
            let base = cfg.server()?;
            let http = crate::pinned_tls::client_builder(cfg.cert_sha256.as_deref())
                .connect_timeout(Duration::from_secs(5))
                .user_agent(format!("askbar/{VERSION}"))
                .build()
                .map_err(|e| e.to_string())?;
            Ok(Inner {
                cfg: Mutex::new(cfg),
                path,
                base,
                http,
                tokens,
                backoff_unit: Duration::from_secs(1),
                outbox: Outbox::in_memory(),
                space: Mutex::new(None),
            })
        });
        Remote { inner }
    }

    /// Keeps the outbox in `outbox` (a file) instead of in memory only.
    pub fn with_outbox(mut self, outbox: Outbox) -> Remote {
        if let Ok(i) = self.inner.as_mut() {
            i.outbox = outbox;
        }
        self
    }

    /// Tests only: make the resume backoff run in milliseconds.
    #[cfg(test)]
    fn with_backoff_unit(mut self, unit: Duration) -> Remote {
        if let Ok(i) = self.inner.as_mut() {
            i.backoff_unit = unit;
        }
        self
    }

    fn inner(&self) -> Result<&Inner, String> {
        self.inner.as_ref().map_err(Clone::clone)
    }
}

impl Inner {
    fn cfg(&self) -> ClientConfig {
        lock(&self.cfg).clone()
    }

    /// Pick up hand edits to the hotkey and the display overrides in client.yaml, so
    /// removing an override takes effect the next time the window opens. The server
    /// address stays as it was paired.
    fn refresh_local(&self) {
        if let Ok(on_disk) = ClientConfig::load(&self.path) {
            let mut cfg = lock(&self.cfg);
            cfg.hotkey = on_disk.hotkey;
            cfg.ui = on_disk.ui;
        }
    }

    fn url(&self, path: &str) -> Url {
        let mut u = self.base.clone();
        u.set_path(&format!("{}{path}", self.base.path().trim_end_matches('/')));
        u
    }

    /// The client API path `rest` in this app's space: `/api/client/v1/spaces/<id><rest>`. The
    /// personal space is used unless the server lists none, then the first one.
    async fn space_path(&self, rest: &str) -> Result<String, String> {
        let known = lock(&self.space).clone();
        let id = match known {
            Some(id) => id,
            None => {
                let spaces: Vec<SpaceInfo> = self.get_json(&askbar_proto::paths::join(askbar_proto::paths::CLIENT_V1, "/spaces")).await?;
                let id = spaces
                    .iter()
                    .find(|s| s.kind == "personal")
                    .or(spaces.first())
                    .map(|s| s.id.clone())
                    .ok_or("The server lists no space for this app's user.")?;
                *lock(&self.space) = Some(id.clone());
                id
            }
        };
        Ok(format!("{}/spaces/{id}{rest}", askbar_proto::paths::CLIENT_V1))
    }

    fn request(&self, method: Method, path: &str) -> Result<RequestBuilder, String> {
        let token = self.tokens.get().ok_or("This app is not paired with the server yet.")?;
        Ok(self.http.request(method, self.url(path)).bearer_auth(token).header(askbar_proto::CLIENT_VERSION_HEADER, VERSION))
    }

    async fn send(&self, req: RequestBuilder) -> Result<reqwest::Response, String> {
        let resp = req.send().await.map_err(|e| format!("Could not reach the server at {}: {e}", self.base))?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status();
        Err(explain(status, &resp.bytes().await.unwrap_or_default()))
    }

    /// Sends the chat request. A refusal the server explains (see `refusal_for`) comes back with
    /// its kind and words; a busy server (503 with Retry-After) is asked again once after the wait.
    async fn send_chat(&self, mut req: RequestBuilder) -> Result<reqwest::Response, Refusal> {
        let mut retried = false;
        loop {
            let again = req.try_clone();
            let resp = req.send().await.map_err(|e| Refusal::plain(format!("Could not reach the server at {}: {e}", self.base)))?;
            let status = resp.status();
            if status.is_success() {
                return Ok(resp);
            }
            let retry_after = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok());
            let body = resp.bytes().await.unwrap_or_default();
            let Some(refusal) = refusal_for(&problem_of(&body), retry_after) else {
                return Err(Refusal::plain(explain(status, &body)));
            };
            match (refusal.kind, again) {
                (Some("busy"), Some(next)) if !retried => {
                    retried = true;
                    tokio::time::sleep(self.backoff_unit * retry_after.unwrap_or(1).min(30) as u32).await;
                    req = next;
                }
                _ => return Err(refusal),
            }
        }
    }

    /// Follows a reply to its end: reads `resp` if there is one, and when a stream ends without
    /// done, asks the server for the rest (see `backoff_secs`). Reports everything through `emit`.
    async fn drive(&self, resp: Option<reqwest::Response>, reader: &mut ReplyReader, cancel: &mut Cancel, stop: &mut bool, emit: Emit<'_>) {
        let fail = |message: String| emit(StreamEvent::error(message));
        let mut resp = resp;
        loop {
            let lost = match resp.take() {
                Some(r) => match read_reply(r, reader, cancel, stop, emit).await {
                    Read::Finished => return,
                    Read::Stop => return self.stop_reply(reader.generation_id.as_deref(), emit).await,
                    Read::Lost(why) => why,
                },
                None => String::new(), // attaching: nothing read yet, ask for the stream straight away
            };
            // The stream ended without done. The server keeps generating, so ask for the rest,
            // quietly: the person sees nothing unless the reply can no longer be had.
            let Some(generation_id) = reader.generation_id.clone() else { return fail(lost) };
            if *stop {
                return self.stop_reply(Some(&generation_id), emit).await;
            }
            let mut attempt = 0u32;
            let mut immediately = lost.is_empty();
            resp = Some(loop {
                let wait = if std::mem::take(&mut immediately) {
                    Duration::ZERO
                } else {
                    let w = self.backoff_unit * backoff_secs(attempt) as u32;
                    attempt += 1;
                    w
                };
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = &mut *cancel => return self.stop_reply(Some(&generation_id), emit).await,
                }
                let path = match self.space_path(&format!("/generations/{generation_id}/stream")).await {
                    Ok(p) => p,
                    Err(e) => return fail(e),
                };
                let req = match self.request(Method::GET, &path) {
                    Ok(r) => r.header("accept", "text/event-stream").query(&[("after", reader.last_id)]),
                    Err(e) => return fail(e),
                };
                let sent = tokio::select! {
                    r = req.send() => r,
                    _ = &mut *cancel => return self.stop_reply(Some(&generation_id), emit).await,
                };
                match sent {
                    Ok(r) if r.status() == StatusCode::NOT_FOUND => {
                        return fail("The reply is no longer on the server.".into());
                    }
                    Ok(r) if r.status() == StatusCode::UNAUTHORIZED => {
                        return fail(explain(r.status(), &[]));
                    }
                    Ok(r) if r.status().is_success() => break r,
                    _ => {} // unreachable or a server hiccup: try again after the next wait
                }
            });
        }
    }

    /// Stop pressed: ask the server to stop generation `id`, then report Cancelled. With no id
    /// (an older server) the reply cannot be stopped there; the connection is simply let go.
    async fn stop_reply(&self, id: Option<&str>, emit: Emit<'_>) {
        if let Some(id) = id {
            let path = self.space_path(&format!("/generations/{id}/cancel")).await;
            let sent = match path.and_then(|p| self.request(Method::POST, &p)) {
                Ok(req) => req.timeout(REQUEST_TIMEOUT).send().await.map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            match sent {
                Ok(r) if r.status().is_success() || r.status() == StatusCode::NOT_FOUND => {}
                Ok(r) => return emit(StreamEvent::error(format!("Could not stop the reply: the server answered {}.", r.status()))),
                Err(e) => return emit(StreamEvent::error(format!("Could not reach the server to stop the reply: {e}"))),
            }
        }
        emit(StreamEvent::Cancelled);
    }

    async fn http_status(&self, req: RequestBuilder) -> Result<StatusCode, reqwest::Error> {
        Ok(req.send().await?.status())
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let resp = self.send(self.request(Method::GET, path)?.timeout(REQUEST_TIMEOUT)).await?;
        resp.json().await.map_err(|e| format!("The server sent an unexpected reply: {e}"))
    }

    async fn active_provider(&self) -> Result<String, String> {
        let prefs: Prefs = self.get_json(&askbar_proto::paths::join(askbar_proto::paths::CLIENT_V1, "/me/prefs")).await?;
        if let Some(id) = prefs.active_provider_id.filter(|id| !id.trim().is_empty()) {
            return Ok(id);
        }
        let list: Vec<ProviderInfo> = self.get_json(&askbar_proto::paths::join(askbar_proto::paths::CLIENT_V1, "/providers")).await?;
        list.into_iter().next().map(|p| p.id).ok_or_else(|| "The server has no providers configured yet.".into())
    }
}

/// A send the server did not take, told the way the window can act on it.
struct Refusal {
    kind: Option<&'static str>,
    message: String,
}

impl Refusal {
    fn plain(message: String) -> Refusal {
        Refusal { kind: None, message }
    }
}

/// The parts of an `application/problem+json` body the app uses.
#[derive(Default)]
struct Seen {
    /// The stable code (`queue_full`, `unauthorized`, ...). Programs branch on this, never on the words.
    code: String,
    detail: String,
    period: String,
}

fn problem_of(body: &[u8]) -> Seen {
    let v: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
    let text = |k: &str| v[k].as_str().unwrap_or("").to_string();
    Seen { code: text("error"), detail: text("detail"), period: text("period") }
}

/// The server's refusals of a send, in words for a person, by code. `None` for anything else.
fn refusal_for(p: &Seen, retry_after: Option<u64>) -> Option<Refusal> {
    let (kind, message) = match p.code.as_str() {
        "queue_full" => ("queue_full", "Five messages are already waiting in this conversation. Wait for one to finish, or remove a waiting message.".to_string()),
        "budget_exceeded" => (
            "budget_exceeded",
            if p.period == "monthly" {
                "The usage limit for this month has been reached. Try again next month, or ask the administrator to raise it.".to_string()
            } else {
                "The usage limit for today has been reached. Try again tomorrow, or ask the administrator to raise it.".to_string()
            },
        ),
        "kill_switch" => ("kill_switch", "Replies have been switched off by the administrator for now. Your message is kept; send it again when they are back.".to_string()),
        "audit_key_missing" => (
            "audit_key_missing",
            "The server is not ready to answer yet: the administrator has to finish setting up the audit key in the admin site. Your message is kept.".to_string(),
        ),
        "audit_backlog" => (
            "busy",
            format!("The server is busy recording and asked for a wait of {} seconds. Your message is kept; send it again in a moment.", retry_after.unwrap_or(0)),
        ),
        _ => return None,
    };
    Some(Refusal { kind: Some(kind), message })
}

/// A failed response, in words for a person: by the problem's `error` code, then (for a proxy or an
/// older server that sends no body) by status.
fn explain(status: StatusCode, body: &[u8]) -> String {
    let p = problem_of(body);
    let rejected = || "The server rejected this app's token.".to_string();
    let too_old = || "This version of Askbar is too old for the server. Please update.".to_string();
    match (p.code.as_str(), status) {
        ("unauthorized", _) | (_, StatusCode::UNAUTHORIZED) => rejected(),
        ("client_too_old", _) | (_, StatusCode::UPGRADE_REQUIRED) => too_old(),
        ("setup_required", _) | (_, StatusCode::SERVICE_UNAVAILABLE) if p.detail.is_empty() || p.code == "setup_required" => {
            SETUP_PENDING.into()
        }
        _ if !p.detail.is_empty() => p.detail,
        _ => format!("The server answered {status}."),
    }
}

const SETUP_PENDING: &str = "The server's setup is not finished. Open its admin page to complete it.";

/// Redeem a pairing key: exchange its code for a client token, keep the
/// token in `tokens`, and record the server in `client.yaml` (creating it, or
/// updating `server_url` and leaving the rest alone).
pub async fn pair(pairing: &str, client_yaml: &Path, tokens: &dyn TokenStore) -> Result<(), String> {
    let askbar_proto::Pairing { url, code, cert_sha256 } =
        askbar_proto::Pairing::decode(pairing).ok_or("That is not a pairing key. It should start with \"askbar-pair:\".")?;
    let mut cfg = if client_yaml.exists() { ClientConfig::load(client_yaml)? } else { ClientConfig::default() };
    let mut base = check_server_url(&url, cfg.allow_insecure)?;
    base.set_path(&format!(
        "{}{}",
        base.path().trim_end_matches('/'),
        askbar_proto::paths::join(askbar_proto::paths::CLIENT_V1, "/pairings")
    ));

    let resp = crate::pinned_tls::client_builder(cert_sha256.as_deref())
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?
        .post(base)
        .header(askbar_proto::CLIENT_VERSION_HEADER, VERSION)
        .json(&askbar_proto::PairRequest { code })
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| {
            if crate::pinned_tls::is_pin_mismatch(&e) {
                format!("The server at {url} shows a different certificate than the one in this pairing key. Create a new pairing key in its admin site.")
            } else {
                format!("Could not reach the server at {url}. Check the address and your network.")
            }
        })?;
    match resp.status() {
        s if s.is_success() => {}
        StatusCode::UNAUTHORIZED => {
            return Err("The pairing key is wrong, expired (they last 10 minutes) or already used. Create a new one.".into())
        }
        StatusCode::UPGRADE_REQUIRED => return Err("This version of Askbar is too old for the server. Please update.".into()),
        s => return Err(format!("The server answered {s}.")),
    }
    let token = resp.json::<askbar_proto::PairResponse>().await.map_err(|_| "The server sent an unexpected reply.".to_string())?.token;
    tokens.set(&token)?;
    cfg.server_url = url.trim_end_matches('/').to_string();
    cfg.cert_sha256 = cert_sha256;
    cfg.save(client_yaml)
}

impl Remote {
    pub async fn settings(&self) -> Result<Settings, String> {
        let i = self.inner()?;
        i.refresh_local();
        let cfg = i.cfg();
        // Offline is not an error here: the window still opens with the local
        // look and feel, and `status` tells the user what is wrong.
        let quick = |path: &str| i.request(Method::GET, path).map(|r| r.timeout(CHECK_TIMEOUT));
        let display: DisplaySettings = match quick(&askbar_proto::paths::join(askbar_proto::paths::CLIENT_V1, "/me/display-defaults")) {
            Ok(req) => match i.send(req).await {
                Ok(r) => r.json().await.unwrap_or_default(),
                Err(_) => DisplaySettings::default(),
            },
            Err(_) => DisplaySettings::default(),
        };
        let prefs: Prefs = match quick(&askbar_proto::paths::join(askbar_proto::paths::CLIENT_V1, "/me/prefs")) {
            Ok(req) => match i.send(req).await {
                Ok(r) => r.json().await.unwrap_or_default(),
                Err(_) => Prefs::default(),
            },
            Err(_) => Prefs::default(),
        };
        let providers: Vec<ProviderInfo> = match quick(&askbar_proto::paths::join(askbar_proto::paths::CLIENT_V1, "/providers")) {
            Ok(req) => match i.send(req).await {
                Ok(r) => r.json().await.unwrap_or_default(),
                Err(_) => Vec::new(),
            },
            Err(_) => Vec::new(),
        };
        let wanted = prefs.active_provider_id.as_deref().unwrap_or("").trim();
        let active = providers.iter().find(|p| wanted.is_empty() || p.id == wanted);
        Ok(Settings {
            ui: merge_ui(&display, &cfg.ui),
            hotkey: cfg.hotkey.clone(),
            provider: active.map(|p| p.name.clone()).unwrap_or_default(),
            model: active.map(|p| p.model.clone()).unwrap_or_default(),
        })
    }

    pub async fn status(&self) -> Option<ClientStatus> {
        let i = match self.inner() {
            Ok(i) => i,
            Err(e) => return Some(ClientStatus::new("problem", e)),
        };
        let offline = || {
            ClientStatus::new(
                "offline",
                format!("Server offline. Could not reach {}. Check that it is running and that you are on the right network.", i.base),
            )
        };
        // Reachable at all? The health endpoint needs neither token nor version.
        match i.http.get(i.url(&askbar_proto::paths::join(askbar_proto::paths::CLIENT_V1, "/health"))).timeout(CHECK_TIMEOUT).send().await {
            Ok(r) if r.status().is_success() => {}
            Err(e) if crate::pinned_tls::is_pin_mismatch(&e) => return Some(ClientStatus::new(
                "rejected",
                "The server's certificate changed, so this app no longer trusts it. Pair again with a new pairing key from the admin site.",
            )),
            _ => return Some(offline()),
        }
        if i.tokens.get().is_none() {
            return Some(ClientStatus::new("not_paired", "This app is not paired with the server yet."));
        }
        let req = i.request(Method::GET, &askbar_proto::paths::join(askbar_proto::paths::CLIENT_V1, "/me")).ok()?.timeout(CHECK_TIMEOUT);
        match i.http_status(req).await {
            Err(_) => return Some(offline()),
            Ok(StatusCode::UNAUTHORIZED) => {
                return Some(ClientStatus::new(
                    "rejected",
                    "Token rejected. The server no longer accepts this app; it may have been revoked.",
                ))
            }
            Ok(StatusCode::UPGRADE_REQUIRED) => {
                return Some(ClientStatus::new("update_needed", "Update needed. This version of Askbar is too old for the server."))
            }
            Ok(s) if s.is_success() => {}
            Ok(StatusCode::SERVICE_UNAVAILABLE) => return Some(ClientStatus::new("problem", SETUP_PENDING)),
            Ok(s) => return Some(ClientStatus::new("problem", format!("The server answered {s}."))),
        }
        let providers: Vec<ProviderInfo> =
            i.get_json(&askbar_proto::paths::join(askbar_proto::paths::CLIENT_V1, "/providers")).await.ok()?;
        let prefs: Prefs = i.get_json(&askbar_proto::paths::join(askbar_proto::paths::CLIENT_V1, "/me/prefs")).await.ok()?;
        let wanted = prefs.active_provider_id.unwrap_or_default();
        let wanted = wanted.trim();
        if providers.is_empty() {
            return Some(ClientStatus::new("problem", "The server has no providers yet. Add one on its admin page."));
        }
        if !wanted.is_empty() && !providers.iter().any(|p| p.id == wanted) {
            let mut st = ClientStatus::new("provider_missing", "The provider you picked no longer exists on the server. Pick another.");
            st.providers = providers.into_iter().map(|p| ProviderChoice { id: p.id, name: p.name }).collect();
            return Some(st);
        }
        None
    }

    pub async fn set_active_provider(&self, id: String) -> Result<(), String> {
        let i = self.inner()?;
        let req = i
            .request(Method::PUT, &askbar_proto::paths::join(askbar_proto::paths::CLIENT_V1, "/me/prefs"))?
            .timeout(REQUEST_TIMEOUT)
            .json(&Prefs { active_provider_id: Some(id) });
        i.send(req).await.map(|_| ())
    }

    #[cfg(test)]
    pub async fn list_providers(&self) -> Result<Vec<ProviderInfo>, String> {
        self.inner()?.get_json(&askbar_proto::paths::join(askbar_proto::paths::CLIENT_V1, "/providers")).await
    }

    /// "Pair another app" (ADR 0008): ask the server for a one-time code for this user, and build the
    /// pairing key from it with this app's own server address and certificate pin, which the server
    /// does not know. The key works once, for ten minutes.
    pub async fn new_pairing_key(&self, name: String) -> Result<String, String> {
        let i = self.inner()?;
        let req = i
            .request(Method::POST, &askbar_proto::paths::join(askbar_proto::paths::CLIENT_V1, "/me/pairing-keys"))?
            .timeout(REQUEST_TIMEOUT)
            .json(&askbar_proto::PairingKeyRequest { name });
        let resp = i.send(req).await?;
        let key: askbar_proto::PairingKeyResponse =
            resp.json().await.map_err(|e| format!("The server's answer was not understood: {e}"))?;
        let cert_sha256 = lock(&i.cfg).cert_sha256.clone();
        Ok(askbar_proto::Pairing { url: i.base.as_str().trim_end_matches('/').to_string(), code: key.code, cert_sha256 }.encode())
    }

    pub async fn list_conversations(&self) -> Result<Vec<Conversation>, String> {
        let i = self.inner()?;
        i.get_json(&i.space_path("/conversations").await?).await
    }

    pub async fn load_conversation(&self, id: String) -> Result<Vec<Message>, String> {
        let i = self.inner()?;
        let d: ConversationDetail = i.get_json(&i.space_path(&format!("/conversations/{id}")).await?).await?;
        Ok(d.messages)
    }

    /// Replies in this conversation that have not finished: the one running, and the messages
    /// waiting behind it.
    pub async fn load_pending(&self, id: String) -> Result<Vec<PendingReply>, String> {
        let i = self.inner()?;
        let d: ConversationDetail = i.get_json(&i.space_path(&format!("/conversations/{id}")).await?).await?;
        Ok(d.pending.into_iter().map(PendingReply::from).collect())
    }

    /// Stops (or, for a message still waiting, removes) one generation. Already finished is fine.
    pub async fn cancel_generation(&self, id: String) -> Result<(), String> {
        let i = self.inner()?;
        let req = i.request(Method::POST, &i.space_path(&format!("/generations/{id}/cancel")).await?)?.timeout(REQUEST_TIMEOUT);
        match i.send(req).await {
            Ok(_) => Ok(()),
            Err(e) if e.ends_with("404 Not Found.") => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Joins a reply that is already running on the server (started here earlier, or elsewhere):
    /// replays it from the start, then follows it live, reporting through `emit` like `ask`.
    pub async fn attach(&self, generation_id: String, cancel: oneshot::Receiver<()>, emit: Emit<'_>) {
        let i = match self.inner() {
            Ok(i) => i,
            Err(e) => return emit(StreamEvent::error(e)),
        };
        let mut reader = ReplyReader { generation_id: Some(generation_id), ..Default::default() };
        let (mut cancel, mut stop) = (cancel.fuse(), false);
        i.drive(None, &mut reader, &mut cancel, &mut stop, emit).await;
    }

    pub async fn delete_conversation(&self, id: String) -> Result<(), String> {
        let i = self.inner()?;
        let resp = i.request(Method::DELETE, &i.space_path(&format!("/conversations/{id}")).await?)?.timeout(REQUEST_TIMEOUT);
        match i.send(resp).await {
            Ok(_) => Ok(()),
            Err(e) if e.ends_with("404 Not Found.") => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Sends a new message. It goes into the outbox first and leaves it when the server's `meta`
    /// arrives, so whatever happens before that, the text is not lost.
    pub async fn ask(&self, conversation_id: Option<String>, text: String, cancel: oneshot::Receiver<()>, emit: Emit<'_>) {
        let i = match self.inner() {
            Ok(i) => i,
            Err(e) => return emit(StreamEvent::error(e)),
        };
        // If the provider cannot be looked up (server unreachable), the entry still keeps the text;
        // sending it later looks the provider up again.
        let provider = i.active_provider().await;
        let entry = i.outbox.add(conversation_id, provider.as_deref().unwrap_or(""), &text);
        if let Err(e) = provider {
            return emit(StreamEvent::error(e));
        }
        self.send_entry(entry, cancel, emit).await;
    }

    /// Sends an outbox entry again, with the same `client_request_id`, so the server can tell a
    /// repeat from a new message.
    pub async fn resend(&self, client_request_id: String, cancel: oneshot::Receiver<()>, emit: Emit<'_>) {
        let i = match self.inner() {
            Ok(i) => i,
            Err(e) => return emit(StreamEvent::error(e)),
        };
        match i.outbox.get(&client_request_id) {
            Some(entry) => self.send_entry(entry, cancel, emit).await,
            None => emit(StreamEvent::error("That message is no longer waiting to be sent.".into())),
        }
    }

    /// The messages still waiting to be sent: the outbox, less those the server already has
    /// (a running or queued generation carrying the same request id), which are dropped from it.
    pub async fn unsent(&self) -> Result<Vec<OutboxEntry>, String> {
        let i = self.inner()?;
        if i.outbox.list().is_empty() {
            return Ok(Vec::new());
        }
        // Without an answer (offline) nothing can be compared; the outbox is shown as it is.
        let Ok(path) = i.space_path("/generations").await else { return Ok(i.outbox.list()) };
        let req = i.request(Method::GET, &path)?.query(&[("status", "running")]).timeout(REQUEST_TIMEOUT);
        let known: Vec<PendingGeneration> = match i.send(req).await {
            Ok(r) => match r.json().await {
                Ok(v) => v,
                Err(_) => return Ok(i.outbox.list()),
            },
            Err(_) => return Ok(i.outbox.list()),
        };
        for g in known {
            if let Some(id) = g.client_request_id {
                i.outbox.remove(&id);
            }
        }
        Ok(i.outbox.list())
    }

    /// Throws one waiting message away.
    pub fn discard_unsent(&self, client_request_id: &str) -> Result<bool, String> {
        Ok(self.inner()?.outbox.remove(client_request_id))
    }

    async fn send_entry(&self, entry: OutboxEntry, cancel: oneshot::Receiver<()>, emit: Emit<'_>) {
        let i = match self.inner() {
            Ok(i) => i,
            Err(e) => return emit(StreamEvent::error(e)),
        };
        // The server has it once `meta` arrives; from then on it is the server's to keep.
        let forget = entry.client_request_id.clone();
        let emit = |e: StreamEvent| {
            if matches!(e, StreamEvent::Started { .. }) {
                i.outbox.remove(&forget);
            }
            emit(e)
        };
        let emit: Emit<'_> = &emit;
        let fail = |message: String| emit(StreamEvent::error(message));
        let provider = if entry.provider.is_empty() {
            match i.active_provider().await {
                Ok(p) => p,
                Err(e) => return fail(e),
            }
        } else {
            entry.provider.clone()
        };
        let req = match i.space_path("/chat").await.and_then(|p| i.request(Method::POST, &p)) {
            Ok(r) => r.header("accept", "text/event-stream").json(&ChatRequest {
                conversation_id: entry.conversation_id.clone(),
                provider,
                text: entry.text.clone(),
                client_request_id: Some(entry.client_request_id.clone()),
            }),
            Err(e) => return fail(e),
        };
        // Stop does not drop the request: it asks the server to stop the reply, which needs the
        // generation id from `meta`. A Stop that comes sooner waits for it.
        let mut cancel = cancel.fuse();
        let mut stop = false;
        let send = i.send_chat(req);
        tokio::pin!(send);
        let resp = loop {
            tokio::select! {
                r = &mut send => match r {
                    Ok(r) => break r,
                    Err(Refusal { kind: Some(kind), message }) => return emit(StreamEvent::error_kind(kind, message)),
                    Err(Refusal { message, .. }) => return fail(message),
                },
                _ = &mut cancel => stop = true,
            }
        };

        let mut reader = ReplyReader::default();
        i.drive(Some(resp), &mut reader, &mut cancel, &mut stop, emit).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_config::Theme;
    use crate::tokens::MemoryTokens;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::sync::Arc;

    struct Mock(Child);
    impl Drop for Mock {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn start_mock() -> (Mock, u16) {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../tests/e2e/mock_llm.py");
        let mut child = Command::new("python3").args(["-u", script, &port.to_string()]).stdout(Stdio::piped()).spawn().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let out = BufReader::new(child.stdout.take().unwrap());
        // Keep draining so the mock never hits a closed pipe.
        std::thread::spawn(move || {
            for line in out.lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
        rx.recv_timeout(Duration::from_secs(10)).expect("mock did not start");
        (Mock(child), port)
    }

    /// Server with one mock provider; returns its URL and a fresh client token.
    async fn start_server(mock_port: u16) -> (String, String) {
        use askbar_server::{auth, secrets::Secrets, AppState};
        let db = askbar_server::Db::open_in_memory().unwrap();
        let user = db.seed_server().unwrap();
        askbar_server::testing::add_provider(
            &db,
            askbar_server::testing::provider("p", "mock", &format!("http://127.0.0.1:{mock_port}/v1"), "mock-model"),
        );
        let code = askbar_server::testing::add_pair_code(&db, &user, "test");
        let state: Arc<AppState> = AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), "0.1.0");
        let token = auth::redeem_pair_code(&state.db.lock().unwrap(), &code).unwrap().unwrap();
        let app = askbar_server::client_router(state);
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (url, token)
    }

    fn sse(name: &str, id: Option<u64>, data: &str) -> String {
        let id = id.map(|n| format!("id: {n}\n")).unwrap_or_default();
        format!("event: {name}\n{id}data: {data}\n\n")
    }

    #[test]
    fn the_reply_reader_parses_the_new_meta_fields_and_counts_delta_ids() {
        let mut r = ReplyReader::default();
        let stream = [
            sse("meta", None, r#"{"event":"meta","conversation_id":"c1","generation_id":"g1","queued":true}"#),
            sse("delta", Some(1), r#"{"event":"delta","text":"Hel"}"#),
            sse("provider", None, r#"{"event":"provider","id":"p1","name":"OpenAI"}"#), // who is answering; passed on, not counted as text
            sse("brand_new", None, r#"{"event":"brand_new","x":1}"#),                   // from a newer server: ignored
            sse("delta", Some(2), r#"{"event":"delta","text":"lo"}"#),
            sse("done", None, r#"{"event":"done"}"#),
        ]
        .concat();
        let steps = r.feed(stream.as_bytes());
        assert_eq!(
            steps,
            [
                Step::Emit(StreamEvent::Started { conversation_id: "c1".into(), generation_id: Some("g1".into()), queued: true }),
                Step::Emit(StreamEvent::Delta { text: "Hel".into() }),
                Step::Emit(StreamEvent::Provider { id: "p1".into(), name: "OpenAI".into() }),
                Step::Emit(StreamEvent::Delta { text: "lo".into() }),
                Step::Done,
            ]
        );
        assert_eq!((r.last_id, r.generation_id.as_deref()), (2, Some("g1")));
    }

    #[test]
    fn the_reply_reader_copes_with_split_chunks_old_servers_and_errors() {
        let mut r = ReplyReader::default();
        // A server that does not send the new fields: no generation id, not queued.
        let meta = sse("meta", None, r#"{"event":"meta","conversation_id":"c1"}"#);
        let (a, b) = meta.as_bytes().split_at(10);
        assert!(r.feed(a).is_empty());
        assert_eq!(r.feed(b), [Step::Emit(StreamEvent::Started { conversation_id: "c1".into(), generation_id: None, queued: false })]);
        assert_eq!((r.last_id, r.generation_id.as_deref()), (0, None));
        assert_eq!(
            r.feed(sse("error", None, r#"{"event":"error","message":"boom"}"#).as_bytes()),
            [Step::Fail { message: "boom".into(), code: None }]
        );
    }

    /// What the fake server saw.
    #[derive(Default)]
    struct Seen {
        plan: Mutex<Vec<(u16, String)>>,
        /// The `after` value of every resume request, in order.
        resumes: Mutex<Vec<String>>,
        /// The id of every cancel request.
        cancels: Mutex<Vec<String>>,
        /// The JSON body of every chat request.
        chats: Mutex<Vec<serde_json::Value>>,
        /// What `GET /api/client/v1/spaces/{space_id}/generations?status=running` answers.
        running: Mutex<String>,
        /// When set, the answer to `POST /api/client/v1/spaces/{space_id}/chat` instead of the usual stream:
        /// status, extra headers and body.
        chat_answer: Mutex<Option<(u16, Vec<(String, String)>, String)>>,
    }

    struct Fake {
        url: String,
        seen: Arc<Seen>,
    }

    /// The one space the fake server lists.
    const FAKE_SPACE: &str = "01JFAKESPACE0000000000000S";

    /// A server that drops the first stream after three deltas, with no `done`. `resume` answers
    /// each resume request in turn with a status and a body (the last one repeats).
    async fn fake_server(resume: Vec<(u16, String)>) -> Fake {
        use axum::{
            extract::{Path as P, Query, State},
            http::{header, StatusCode as S},
            routing::{get, post},
            Router,
        };
        let seen = Arc::new(Seen { plan: Mutex::new(resume), running: Mutex::new("[]".into()), ..Default::default() });
        let first: String = [
            (None, "meta", r#"{"event":"meta","conversation_id":"c1","generation_id":"g1"}"#),
            (Some(1), "delta", r#"{"event":"delta","text":"a"}"#),
            (Some(2), "delta", r#"{"event":"delta","text":"b"}"#),
            (Some(3), "delta", r#"{"event":"delta","text":"c"}"#),
        ]
        .iter()
        .map(|(id, name, data)| sse(name, *id, data))
        .collect();
        let app = Router::new()
            .route("/api/client/v1/me/prefs", get(|| async { r#"{"active_provider_id":"p"}"# }))
            .route(
                "/api/client/v1/spaces",
                get(|| async {
                    ([(header::CONTENT_TYPE, "application/json")], format!(r#"[{{"id":"{FAKE_SPACE}","kind":"personal","name":""}}]"#))
                }),
            )
            .route(
                "/api/client/v1/spaces/{space_id}/chat",
                post(move |State(seen): State<Arc<Seen>>, axum::Json(body): axum::Json<serde_json::Value>| {
                    let b = first.clone();
                    async move {
                        seen.chats.lock().unwrap().push(body);
                        let custom = seen.chat_answer.lock().unwrap().clone();
                        match custom {
                            Some((status, headers, text)) => {
                                let mut r = axum::response::Response::new(axum::body::Body::from(text));
                                *r.status_mut() = S::from_u16(status).unwrap();
                                r.headers_mut().insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
                                for (k, v) in headers {
                                    r.headers_mut().insert(header::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
                                }
                                r
                            }
                            None => axum::response::IntoResponse::into_response(([(header::CONTENT_TYPE, "text/event-stream")], b)),
                        }
                    }
                }),
            )
            .route(
                "/api/client/v1/spaces/{space_id}/generations",
                get(|State(seen): State<Arc<Seen>>| async move {
                    ([(header::CONTENT_TYPE, "application/json")], seen.running.lock().unwrap().clone())
                }),
            )
            .route(
                "/api/client/v1/spaces/{space_id}/generations/{id}/stream",
                get(|State(seen): State<Arc<Seen>>, Query(q): Query<std::collections::HashMap<String, String>>| async move {
                    seen.resumes.lock().unwrap().push(q.get("after").cloned().unwrap_or_default());
                    let (status, body) = {
                        let mut plan = seen.plan.lock().unwrap();
                        if plan.len() > 1 {
                            plan.remove(0)
                        } else {
                            plan[0].clone()
                        }
                    };
                    (S::from_u16(status).unwrap(), [(header::CONTENT_TYPE, "text/event-stream")], body)
                }),
            )
            .route(
                "/api/client/v1/spaces/{space_id}/generations/{id}/cancel",
                post(|State(seen): State<Arc<Seen>>, P((_, id)): P<(String, String)>| async move {
                    seen.cancels.lock().unwrap().push(id);
                    S::NO_CONTENT
                }),
            )
            .with_state(seen.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        Fake { url, seen }
    }

    async fn ask_fake(fake: &Fake) -> Vec<StreamEvent> {
        let remote = remote_for(&fake.url, Some("t"), PathBuf::new()).with_backoff_unit(Duration::from_millis(2));
        let events = Mutex::new(Vec::new());
        let (_tx, rx) = oneshot::channel();
        remote.ask(None, "hi".into(), rx, &|e| events.lock().unwrap().push(e)).await;
        events.into_inner().unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_dropped_stream_is_resumed_quietly_and_the_reply_arrives_once_in_order() {
        let rest = sse("delta", Some(4), r#"{"event":"delta","text":"d"}"#)
            + &sse("delta", Some(5), r#"{"event":"delta","text":"e"}"#)
            + &sse("done", None, r#"{"event":"done"}"#);
        // The first resume attempt meets a server hiccup; the second gets the rest.
        let fake = fake_server(vec![(503, String::new()), (200, rest)]).await;
        let events = ask_fake(&fake).await;

        let reply: String =
            events.iter().filter_map(|e| if let StreamEvent::Delta { text } = e { Some(text.as_str()) } else { None }).collect();
        assert_eq!(reply, "abcde", "every delta once, in order");
        assert!(matches!(events.first(), Some(StreamEvent::Started { generation_id: Some(g), .. }) if g == "g1"));
        assert!(matches!(events.last(), Some(StreamEvent::Done)));
        assert!(!events.iter().any(|e| matches!(e, StreamEvent::Error { .. })), "no error while retrying: {events:?}");
        assert_eq!(*fake.seen.resumes.lock().unwrap(), ["3", "3"], "both attempts ask for everything after delta 3");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resuming_stops_on_an_error_event_a_404_or_a_401() {
        let err = sse("error", None, r#"{"event":"error","message":"Timed out after 5 minutes","code":"timed_out"}"#);
        let events = ask_fake(&fake_server(vec![(200, err)]).await).await;
        assert!(matches!(events.last(), Some(StreamEvent::Error { message, .. }) if message == "Stopped after 5 minutes"), "{events:?}");

        let events = ask_fake(&fake_server(vec![(404, String::new())]).await).await;
        assert!(
            matches!(events.last(), Some(StreamEvent::Error { message, .. }) if message.contains("no longer on the server")),
            "{events:?}"
        );

        let events = ask_fake(&fake_server(vec![(401, String::new())]).await).await;
        assert!(matches!(events.last(), Some(StreamEvent::Error { message, .. }) if message.contains("rejected")), "{events:?}");
        for e in [&events[..]] {
            assert_eq!(e.iter().filter(|x| matches!(x, StreamEvent::Delta { .. })).count(), 3, "what arrived stays");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_posts_to_the_cancel_endpoint_with_the_generation_id_then_reports_cancelled() {
        let fake = fake_server(vec![(200, sse("done", None, r#"{"event":"done"}"#))]).await;
        let remote = remote_for(&fake.url, Some("t"), PathBuf::new()).with_backoff_unit(Duration::from_millis(2));
        let events = Mutex::new(Vec::new());
        let (tx, rx) = oneshot::channel();
        let tx = Mutex::new(Some(tx));
        remote
            .ask(None, "hi".into(), rx, &|e| {
                // Stop is pressed as soon as the reply has begun.
                if matches!(e, StreamEvent::Started { .. }) {
                    if let Some(tx) = tx.lock().unwrap().take() {
                        let _ = tx.send(());
                    }
                }
                events.lock().unwrap().push(e)
            })
            .await;
        let events = events.into_inner().unwrap();
        assert!(matches!(events.last(), Some(StreamEvent::Cancelled)), "{events:?}");
        assert_eq!(*fake.seen.cancels.lock().unwrap(), ["g1"], "the server was asked to stop that generation");
        assert!(!events.iter().any(|e| matches!(e, StreamEvent::Done | StreamEvent::Error { .. })), "{events:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_pressed_before_the_reply_begins_still_reaches_the_server() {
        let fake = fake_server(vec![(200, String::new())]).await;
        let remote = remote_for(&fake.url, Some("t"), PathBuf::new()).with_backoff_unit(Duration::from_millis(2));
        let events = Mutex::new(Vec::new());
        let (tx, rx) = oneshot::channel();
        tx.send(()).unwrap(); // already pressed
        remote.ask(None, "hi".into(), rx, &|e| events.lock().unwrap().push(e)).await;
        assert!(matches!(events.into_inner().unwrap().last(), Some(StreamEvent::Cancelled)));
        assert_eq!(*fake.seen.cancels.lock().unwrap(), ["g1"], "it waited for the generation id, then cancelled");
    }

    /// Runs `remote.ask` in the background, returning its events and Stop's sender.
    fn ask_in_background(
        remote: Arc<Remote>,
        conversation: Option<String>,
        text: &str,
    ) -> (Arc<Mutex<Vec<StreamEvent>>>, oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = oneshot::channel();
        let (ev, text) = (events.clone(), text.to_string());
        let task = tokio::spawn(async move {
            remote.ask(conversation, text, rx, &|e| ev.lock().unwrap().push(e)).await;
        });
        (events, tx, task)
    }

    async fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
        for _ in 0..400 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("timed out waiting for {what}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn another_window_can_attach_to_a_running_reply_and_remove_a_queued_message() {
        let (_mock, mock_port) = start_mock();
        let (url, token) = start_server(mock_port).await;
        let a = Arc::new(remote_for(&url, Some(&token), PathBuf::new()));
        let b = Arc::new(remote_for(&url, Some(&token), PathBuf::new()));

        // A starts a slow reply; B did not start it.
        let (a_events, a_stop, a_task) = ask_in_background(a.clone(), None, "please be slow");
        wait_until("A's reply to begin", || {
            a_events.lock().unwrap().iter().filter(|e| matches!(e, StreamEvent::Delta { .. })).count() >= 3
        })
        .await;
        let (conversation, generation) = match a_events.lock().unwrap()[0].clone() {
            StreamEvent::Started { conversation_id, generation_id: Some(g), .. } => (conversation_id, g),
            other => panic!("expected Started, got {other:?}"),
        };

        // B sends into the same conversation: it waits its turn.
        let (b_events, _b_stop, b_task) = ask_in_background(b.clone(), Some(conversation.clone()), "second question");
        let queued = {
            let mut found = None;
            for _ in 0..200 {
                let p = b.load_pending(conversation.clone()).await.unwrap();
                if let Some(q) = p.iter().find(|p| p.status == "queued") {
                    found = Some(q.clone());
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            found.expect("the second message shows as queued")
        };
        assert_eq!(queued.prompt, "second question");
        let pending = b.load_pending(conversation.clone()).await.unwrap();
        assert_eq!(
            pending.iter().map(|p| (p.status.as_str(), p.prompt.as_str())).collect::<Vec<_>>(),
            [("running", "please be slow"), ("queued", "second question")]
        );
        assert_eq!(pending[0].generation_id, generation);

        // Attaching replays from the start, then follows.
        let attached = Arc::new(Mutex::new(Vec::new()));
        let (att_stop, att_rx) = oneshot::channel();
        let (att, remote_b) = (attached.clone(), b.clone());
        let att_task = tokio::spawn(async move { remote_b.attach(generation, att_rx, &|e| att.lock().unwrap().push(e)).await });
        wait_until("the attached stream to catch up", || attached.lock().unwrap().len() >= 6).await;
        let text: String = attached
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| if let StreamEvent::Delta { text } = e { Some(text.clone()) } else { None })
            .collect();
        assert!(text.starts_with("Streaming slowly so"), "replayed from the start: {text:?}");

        // Removing the queued message: it never runs and leaves nothing in the conversation.
        b.cancel_generation(queued.generation_id.clone()).await.unwrap();
        b.cancel_generation(queued.generation_id).await.unwrap(); // repeat is fine
        b_task.await.unwrap();
        assert!(matches!(b_events.lock().unwrap().last(), Some(StreamEvent::Done)), "its waiting connection ends");

        // Stop from the attached window ends the reply for everyone.
        att_stop.send(()).unwrap();
        att_task.await.unwrap();
        assert!(matches!(attached.lock().unwrap().last(), Some(StreamEvent::Cancelled)));
        a_task.await.unwrap();
        assert!(matches!(a_events.lock().unwrap().last(), Some(StreamEvent::Done)), "A's stream ends cleanly with the partial text kept");
        drop(a_stop);
        assert!(b.load_pending(conversation.clone()).await.unwrap().is_empty());
        let msgs = b.load_conversation(conversation).await.unwrap();
        assert_eq!(msgs.iter().map(|m| m.role.as_str()).collect::<Vec<_>>(), ["user", "assistant"], "the removed message left no trace");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unsent_messages_the_server_already_has_are_dropped_and_the_rest_resent_with_their_request_id() {
        let fake = fake_server(vec![(200, sse("done", None, r#"{"event":"done"}"#))]).await;
        let remote = remote_for(&fake.url, Some("t"), PathBuf::new()).with_backoff_unit(Duration::from_millis(2));
        let i = remote.inner().unwrap();
        let had = i.outbox.add(None, "p", "the server has this one");
        let missing = i.outbox.add(Some("c5".into()), "p", "this one never arrived");
        *fake.seen.running.lock().unwrap() = format!(
            r#"[{{"generation_id":"g7","conversation_id":"c1","status":"queued","text":"","prompt":"x","client_request_id":"{}"}},
                {{"generation_id":"g8","conversation_id":"c1","status":"running","text":"","prompt":"y"}}]"#,
            had.client_request_id
        );

        let unsent = remote.unsent().await.unwrap();
        assert_eq!(unsent, [missing.clone()], "only the message the server lacks is left");
        assert_eq!(remote.inner().unwrap().outbox.list(), [missing.clone()], "and the other left the outbox for good");

        // Sending it again carries its own request id (and nothing else changes), and it leaves
        // the outbox once the server says it has it.
        let events = Mutex::new(Vec::new());
        let (_tx, rx) = oneshot::channel();
        remote.resend(missing.client_request_id.clone(), rx, &|e| events.lock().unwrap().push(e)).await;
        let chats = fake.seen.chats.lock().unwrap().clone();
        assert_eq!(chats.len(), 1);
        assert_eq!(chats[0]["client_request_id"], missing.client_request_id.as_str());
        assert_eq!(
            (chats[0]["text"].as_str(), chats[0]["conversation_id"].as_str(), chats[0]["provider"].as_str()),
            (Some("this one never arrived"), Some("c5"), Some("p"))
        );
        assert!(remote.unsent().await.unwrap().is_empty(), "the server took it (meta arrived)");

        // An id that is not in the outbox says so.
        let events2 = Mutex::new(Vec::new());
        let (_tx2, rx2) = oneshot::channel();
        remote.resend("nope".into(), rx2, &|e| events2.lock().unwrap().push(e)).await;
        assert!(
            matches!(events2.into_inner().unwrap().last(), Some(StreamEvent::Error { message, .. }) if message.contains("no longer waiting"))
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unsent_messages_are_listed_as_they_are_when_the_server_cannot_be_asked() {
        let remote = remote_for("http://127.0.0.1:9", Some("t"), PathBuf::new());
        let e = remote.inner().unwrap().outbox.add(None, "p", "still here");
        assert_eq!(remote.unsent().await.unwrap(), [e]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_new_message_is_kept_until_the_server_takes_it() {
        // A server that is not there at all: the text must stay in the outbox.
        let remote = remote_for("http://127.0.0.1:9", Some("t"), PathBuf::new());
        let events = Mutex::new(Vec::new());
        let (_tx, rx) = oneshot::channel();
        remote.ask(None, "do not lose me".into(), rx, &|e| events.lock().unwrap().push(e)).await;
        assert!(matches!(events.into_inner().unwrap().last(), Some(StreamEvent::Error { .. })));
        let kept = remote.inner().unwrap().outbox.list();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].text, "do not lose me");
        assert_eq!(remote.discard_unsent(&kept[0].client_request_id), Ok(true));
        assert!(remote.inner().unwrap().outbox.list().is_empty());

        // A server that takes it: the entry is gone after meta.
        let fake = fake_server(vec![(200, sse("done", None, r#"{"event":"done"}"#))]).await;
        let remote = remote_for(&fake.url, Some("t"), PathBuf::new());
        let (_tx, rx) = oneshot::channel();
        remote.ask(None, "take me".into(), rx, &|_| {}).await;
        assert!(remote.inner().unwrap().outbox.list().is_empty());
        let chats = fake.seen.chats.lock().unwrap().clone();
        assert!(chats[0]["client_request_id"].as_str().is_some_and(|id| id.len() == 36), "every send carries a request id");
    }

    /// Sends "my precious words" against a server that answers the chat with `status`, `headers` and
    /// `body`; returns the events, the outbox afterwards and what the server saw.
    async fn refused(status: u16, headers: &[(&str, &str)], body: &str) -> (Vec<StreamEvent>, Vec<OutboxEntry>, usize) {
        let fake = fake_server(vec![(200, sse("done", None, r#"{"event":"done"}"#))]).await;
        *fake.seen.chat_answer.lock().unwrap() =
            Some((status, headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), body.to_string()));
        let remote = remote_for(&fake.url, Some("t"), PathBuf::new()).with_backoff_unit(Duration::from_millis(2));
        let events = Mutex::new(Vec::new());
        let (_tx, rx) = oneshot::channel();
        remote.ask(None, "my precious words".into(), rx, &|e| events.lock().unwrap().push(e)).await;
        let sent = fake.seen.chats.lock().unwrap().len();
        (events.into_inner().unwrap(), remote.inner().unwrap().outbox.list(), sent)
    }

    fn last_error(events: &[StreamEvent]) -> (String, Option<String>) {
        match events.last() {
            Some(e @ StreamEvent::Error { message, .. }) => {
                (message.clone(), serde_json::to_value(e).unwrap()["kind"].as_str().map(str::to_string))
            }
            other => panic!("expected an error, got {other:?}"),
        }
    }

    /// The refusals the server can answer a send with, and the kind each should be told as.
    const REFUSALS: [(&str, u16, &str, &str); 5] = [
        ("queue_full", 409, "", r#"{"error":"queue_full"}"#), // the 409 now carries its code; a bare 409 is no longer read as a full queue
        ("budget_exceeded", 429, "", r#"{"error":"budget_exceeded","period":"daily"}"#),
        ("kill_switch", 503, "", r#"{"error":"kill_switch"}"#),
        ("audit_key_missing", 503, "", r#"{"error":"audit_key_missing"}"#),
        ("busy", 503, "5", r#"{"error":"audit_backlog"}"#),
    ];

    #[tokio::test]
    async fn each_refusal_has_its_own_kind_and_words_and_the_typed_text_is_kept() {
        let mut seen_messages = Vec::new();
        for (kind, status, retry_after, body) in REFUSALS {
            let headers: Vec<(&str, &str)> = if retry_after.is_empty() { vec![] } else { vec![("retry-after", retry_after)] };
            let (events, outbox, sent) = refused(status, &headers, body).await;
            let (message, got_kind) = last_error(&events);
            assert_eq!(got_kind.as_deref(), Some(kind), "{status} {body}: {message}");
            assert!(message.len() > 20 && !message.contains("answered"), "a sentence for a person, not a status code: {message}");
            assert!(!events.iter().any(|e| matches!(e, StreamEvent::Started { .. })), "{kind}: the reply never began");
            assert_eq!(outbox.len(), 1, "{kind}: the message is still waiting to be sent");
            assert_eq!(outbox[0].text, "my precious words", "{kind}: the typed text is kept as typed");
            assert!(sent >= 1);
            seen_messages.push(message);
        }
        let unique: std::collections::BTreeSet<_> = seen_messages.iter().collect();
        assert_eq!(unique.len(), seen_messages.len(), "every refusal reads differently: {seen_messages:?}");
        assert!(seen_messages[0].contains("waiting"), "{}", seen_messages[0]);
        assert!(seen_messages[1].to_lowercase().contains("limit"), "{}", seen_messages[1]);
        assert!(seen_messages[2].to_lowercase().contains("switched off"), "{}", seen_messages[2]);
        assert!(seen_messages[3].to_lowercase().contains("audit"), "{}", seen_messages[3]);
        assert!(seen_messages[4].contains('5'), "the wait is named: {}", seen_messages[4]);
    }

    #[tokio::test]
    async fn a_busy_server_is_retried_once_after_its_retry_after_and_the_rest_wait_for_the_person() {
        // 503 with Retry-After: the message is sent again once (after the delay) and no more.
        let (events, outbox, sent) = refused(503, &[("retry-after", "1")], r#"{"error":"audit_backlog"}"#).await;
        assert_eq!(sent, 2, "one retry, not a loop");
        assert_eq!(last_error(&events).1.as_deref(), Some("busy"));
        assert_eq!(outbox.len(), 1, "still waiting for the person after that");
        // The others are not retried by themselves.
        for (kind, status, _, body) in REFUSALS.iter().filter(|r| r.0 != "busy") {
            let (_, _, sent) = refused(*status, &[], body).await;
            assert_eq!(sent, 1, "{kind} is sent once");
        }
    }

    #[tokio::test]
    async fn a_timed_out_reply_keeps_its_text_and_says_how_long_it_ran() {
        // The first stream (the fake's usual one) delivers "a", "b", "c" and drops; the resume brings
        // the server's last word: it ran out of time.
        let stream = sse("error", None, r#"{"event":"error","message":"Timed out after 5 minutes","code":"timed_out"}"#);
        let fake = fake_server(vec![(200, stream)]).await;
        let remote = remote_for(&fake.url, Some("t"), PathBuf::new()).with_backoff_unit(Duration::from_millis(2));
        let events = Mutex::new(Vec::new());
        let (_tx, rx) = oneshot::channel();
        remote.ask(None, "hi".into(), rx, &|e| events.lock().unwrap().push(e)).await;
        let events = events.into_inner().unwrap();
        let text: String =
            events.iter().filter_map(|e| if let StreamEvent::Delta { text } = e { Some(text.as_str()) } else { None }).collect();
        assert_eq!(text, "abc", "the text that arrived stays");
        let (message, kind) = last_error(&events);
        assert_eq!(kind.as_deref(), Some("timed_out"));
        assert_eq!(message, "Stopped after 5 minutes");
    }

    /// Serves `state` on a fresh port; returns the URL and a task that stops it.
    async fn serve_state(state: Arc<askbar_server::AppState>) -> (String, tokio::task::JoinHandle<()>) {
        let app = askbar_server::client_router(state);
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        (url, tokio::spawn(async move { axum::serve(l, app).await.unwrap() }))
    }

    /// Reads a chat or resume response into `reader` until `until` says enough, or the stream ends.
    /// Returns the text read and the terminal step, if one arrived.
    async fn read_some(resp: reqwest::Response, reader: &mut ReplyReader, until: impl Fn(&ReplyReader) -> bool) -> (String, Option<Step>) {
        let (mut text, mut stream) = (String::new(), resp.bytes_stream());
        while let Some(chunk) = stream.next().await {
            for step in reader.feed(&chunk.unwrap()) {
                match step {
                    Step::Emit(StreamEvent::Delta { text: t }) => text.push_str(&t),
                    Step::Emit(_) => {}
                    end => return (text, Some(end)),
                }
            }
            if until(reader) {
                break;
            }
        }
        (text, None)
    }

    async fn stored_reply(http: &reqwest::Client, url: &str, token: &str, conversation_id: &str) -> String {
        let detail: ConversationDetail =
            http.get(format!("{url}/conversations/{conversation_id}")).bearer_auth(token).send().await.unwrap().json().await.unwrap();
        detail.messages.iter().rev().find(|m| m.role == "assistant").map(|m| m.content.clone()).unwrap_or_default()
    }

    /// A reply that is dropped by the client and picked up again with `after=N` arrives whole: no
    /// delta twice, none missing. With a server restart in between the person gets what was
    /// written, the reply ends `interrupted`, and the stored text is that same partial text.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_dropped_client_resumes_with_after_and_a_restart_ends_the_reply_interrupted() {
        use askbar_server::{auth, secrets::Secrets, AppState};
        let (_mock, mock_port) = start_mock();
        let dir = std::env::temp_dir().join(format!("askbar-8-5-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("control.db");
        let db = askbar_server::Db::open(&db_path).unwrap();
        let user = db.seed_server().unwrap();
        askbar_server::testing::add_provider(
            &db,
            askbar_server::testing::provider("p", "mock", &format!("http://127.0.0.1:{mock_port}/v1"), "mock-model"),
        );
        let code = askbar_server::testing::add_pair_code(&db, &user, "test");
        let state: Arc<AppState> = AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), "0.1.0")
            .with_spaces(askbar_server::Spaces::on_disk(dir.join("spaces")));
        let token = auth::redeem_pair_code(&state.db.lock().unwrap(), &code).unwrap().unwrap();
        let space = askbar_server::testing::personal_space(&state, &user);
        let (url, server) = serve_state(state.clone()).await;
        // Everything below is in the user's personal space.
        let sp = format!("{url}/api/client/v1/spaces/{}", space.id);
        let mut version = reqwest::header::HeaderMap::new();
        version.insert(askbar_proto::CLIENT_VERSION_HEADER, VERSION.parse().unwrap());
        let http = reqwest::Client::builder().default_headers(version).build().unwrap();
        let chat = |text: &str| {
            http.post(format!("{sp}/chat"))
                .bearer_auth(&token)
                .header(askbar_proto::CLIENT_VERSION_HEADER, VERSION)
                .header("accept", "text/event-stream")
                .json(&ChatRequest { conversation_id: None, provider: "p".into(), text: text.into(), client_request_id: None })
        };

        // 1. Drop the client after a few deltas, resume after what was received (twice, to be sure),
        // then stop the reply: the three pieces joined are the stored reply.
        let resume = |gid: String, after: u64| {
            http.get(format!("{sp}/generations/{gid}/stream"))
                .bearer_auth(&token)
                .header(askbar_proto::CLIENT_VERSION_HEADER, VERSION)
                .header("accept", "text/event-stream")
                .query(&[("after", after)])
        };
        let mut reader = ReplyReader::default();
        let first = chat("slow hello").send().await.unwrap();
        let (head, end) = read_some(first, &mut reader, |r| r.last_id >= 3).await;
        assert!(
            end.is_none() && reader.last_id >= 3,
            "the reply was still going when the client left: {end:?} {} {head:?}",
            reader.last_id
        );
        let gid = reader.generation_id.clone().expect("meta names the generation");
        let at_drop = reader.last_id;
        let second = resume(gid.clone(), at_drop).send().await.unwrap();
        assert!(second.status().is_success());
        let (middle, end) = read_some(second, &mut reader, |r| r.last_id >= at_drop + 5).await;
        assert!(end.is_none() && reader.last_id >= at_drop + 5, "the resume carried on from delta {at_drop}");
        let stop = http.post(format!("{sp}/generations/{gid}/cancel")).bearer_auth(&token).send().await.unwrap();
        assert!(stop.status().is_success());
        let third = resume(gid.clone(), reader.last_id).send().await.unwrap();
        let (tail, _) = read_some(third, &mut reader, |_| false).await;
        let all = format!("{head}{middle}{tail}");
        let convs: Vec<Conversation> =
            http.get(format!("{sp}/conversations")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
        assert_eq!(all, stored_reply(&http, &sp, &token, &convs[0].id).await, "no delta twice, none missing");
        assert!(all.starts_with("Streaming slowly"), "{all}");

        // 2. A server restart while a reply is running.
        let mut reader = ReplyReader::default();
        let first = chat("slow reply please").send().await.unwrap();
        let (head, end) = read_some(first, &mut reader, |r| r.last_id >= 3).await;
        assert!(end.is_none());
        let gid = reader.generation_id.clone().unwrap();
        askbar_server::generation::shutdown(&state);
        server.abort();
        drop(space);
        let restarted_db = askbar_server::Db::open(&db_path).unwrap();
        // As the server starts: every space file opened, then swept.
        let spaces = askbar_server::Spaces::on_disk(dir.join("spaces"));
        for s in spaces.open_all(&restarted_db).unwrap() {
            askbar_server::generation::sweep_on_start(&s.lock()).unwrap();
        }
        let state2: Arc<AppState> =
            AppState::new(restarted_db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), "0.1.0").with_spaces(spaces);
        let (url2, _server2) = serve_state(state2.clone()).await;
        let space2 = askbar_server::testing::personal_space(&state2, &user);
        let resumed = http
            .get(format!("{url2}/api/client/v1/spaces/{}/generations/{gid}/stream", space2.id))
            .bearer_auth(&token)
            .header(askbar_proto::CLIENT_VERSION_HEADER, VERSION)
            .header("accept", "text/event-stream")
            .query(&[("after", reader.last_id)])
            .send()
            .await
            .unwrap();
        assert!(resumed.status().is_success());
        let (tail, end) = read_some(resumed, &mut reader, |_| false).await;
        let Some(Step::Fail { message: why, .. }) = end else { panic!("an interrupted reply ends with an error event, got {end:?}") };
        assert!(!why.is_empty());
        let (status, stored_text): (String, String) = space2
            .lock()
            .conn()
            .query_row("SELECT status, text FROM generations WHERE id = ?1", [&gid], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(status, "interrupted");
        let all = format!("{head}{tail}");
        assert!(stored_text.starts_with("Streaming sl") && stored_text.len() >= 12, "{stored_text}");
        assert_eq!(all, stored_text, "the person got exactly the partial text that was kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_servers_code_decides_the_kind_and_the_words_are_only_for_display() {
        let some = |c: &str| Some(c.to_string());
        assert_eq!(
            server_error("Timed out after 90 seconds".into(), some("timed_out")),
            StreamEvent::error_kind("timed_out", "Stopped after 90 seconds".into())
        );
        assert_eq!(
            server_error("Timed out after 1 minute".into(), some("timed_out")),
            StreamEvent::error_kind("timed_out", "Stopped after 1 minute".into())
        );
        // The code decides, not the sentence: other words, same kind.
        assert_eq!(
            server_error("The reply ran out of time.".into(), some("timed_out")),
            StreamEvent::error_kind("timed_out", "The reply ran out of time.".into())
        );
        // The sentence alone proves nothing.
        assert_eq!(server_error("Timed out after 5 minutes".into(), None), StreamEvent::error("Timed out after 5 minutes".into()));
        assert_eq!(
            server_error("The server restarted.".into(), some("interrupted")),
            StreamEvent::error_kind("interrupted", "The server restarted.".into())
        );
        assert_eq!(server_error("boom".into(), some("provider_error")), StreamEvent::error_kind("provider_error", "boom".into()));
    }

    #[test]
    fn a_failed_response_is_explained_by_its_code_then_its_status_then_its_sentence() {
        let body = |code: &str, detail: &str| format!(r#"{{"error":"{code}","detail":"{detail}"}}"#).into_bytes();
        let st = |n: u16| StatusCode::from_u16(n).unwrap();
        assert!(explain(st(401), &body("unauthorized", "x")).contains("rejected"));
        assert!(explain(st(401), b"").contains("rejected"), "a proxy that sends no body still gets the status right");
        assert!(explain(st(426), &body("client_too_old", "x")).contains("too old"));
        assert!(explain(st(503), &body("setup_required", "x")).contains("setup is not finished"));
        assert_eq!(explain(st(404), &body("not_found", "That conversation was not found.")), "That conversation was not found.");
        assert_eq!(explain(st(500), b"<html>"), "The server answered 500 Internal Server Error.");
    }

    #[test]
    fn the_resume_backoff_is_1_2_4_8_16_then_30_seconds() {
        assert_eq!((0..8).map(backoff_secs).collect::<Vec<_>>(), [1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_talks_to_a_real_server() {
        let (_mock, mock_port) = start_mock();
        let (url, token) = start_server(mock_port).await;
        let cfg = ClientConfig { server_url: url, ..Default::default() };
        let remote = Remote::new(Ok(cfg), PathBuf::new(), Box::new(MemoryTokens::with(&token)));

        let s = remote.settings().await.unwrap();
        assert_eq!((s.provider.as_str(), s.model.as_str()), ("mock", "mock-model"));
        assert_eq!(remote.list_providers().await.unwrap().len(), 1);
        assert!(remote.list_conversations().await.unwrap().is_empty());

        let events = Mutex::new(Vec::new());
        let (_tx, rx) = oneshot::channel();
        remote.ask(None, "hello".into(), rx, &|e| events.lock().unwrap().push(e)).await;
        let events = events.into_inner().unwrap();
        let StreamEvent::Started { conversation_id, .. } = events[0].clone() else { panic!("first event must be Started") };
        assert!(matches!(events.last(), Some(StreamEvent::Done)));
        let reply: String =
            events.iter().filter_map(|e| if let StreamEvent::Delta { text } = e { Some(text.as_str()) } else { None }).collect();
        assert!(reply.ends_with("Anything else?"));

        let list = remote.list_conversations().await.unwrap();
        assert_eq!((list.len(), list[0].id.as_str(), list[0].message_count), (1, conversation_id.as_str(), 2));
        let msgs = remote.load_conversation(conversation_id.clone()).await.unwrap();
        assert_eq!((msgs[0].role.as_str(), msgs[0].content.as_str()), ("user", "hello"));
        assert_eq!(msgs[1].content, reply);

        // A follow-up goes into the same conversation.
        let (_tx, rx) = oneshot::channel();
        remote.ask(Some(conversation_id.clone()), "more".into(), rx, &|_| {}).await;
        assert_eq!(remote.load_conversation(conversation_id.clone()).await.unwrap().len(), 4);

        remote.delete_conversation(conversation_id).await.unwrap();
        assert!(remote.list_conversations().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn unpaired_or_misconfigured_clients_explain_themselves() {
        let cfg = ClientConfig { server_url: "http://127.0.0.1:1".into(), ..Default::default() };
        let r = Remote::new(Ok(cfg), PathBuf::new(), Box::new(MemoryTokens::default()));
        assert!(r.list_conversations().await.unwrap_err().contains("not paired"));

        let bad = ClientConfig { server_url: "http://192.168.1.5:8080".into(), ..Default::default() };
        let r = Remote::new(Ok(bad), PathBuf::new(), Box::new(MemoryTokens::default()));
        assert!(r.settings().await.unwrap_err().contains("allow_insecure"));
    }

    /// A real server with a mock-less provider list; `min_client` lets a test make the client too old.
    async fn status_server(min_client: &str) -> (String, String, Arc<askbar_server::AppState>) {
        use askbar_server::{auth, secrets::Secrets, AppState};
        let db = askbar_server::Db::open_in_memory().unwrap();
        let user = db.seed_server().unwrap();
        for (n, name) in ["alpha", "beta"].iter().enumerate() {
            askbar_server::testing::add_provider(&db, askbar_server::testing::provider(&format!("p{n}"), name, "http://x/v1", "m"));
        }
        let code = askbar_server::testing::add_pair_code(&db, &user, "test");
        let state = AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), min_client);
        let token = auth::redeem_pair_code(&state.db.lock().unwrap(), &code).unwrap().unwrap();
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let app = askbar_server::client_router(state.clone());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (url, token, state)
    }

    /// The signed-in user's chosen provider lives on the server, so tests set it there.
    fn choose_provider(state: &Arc<askbar_server::AppState>, id: &str) {
        state
            .db
            .lock()
            .unwrap()
            .conn()
            .execute("INSERT OR REPLACE INTO user_prefs (user_id, active_provider_id) SELECT id, ?1 FROM users", [id])
            .unwrap();
    }

    fn remote_for(url: &str, token: Option<&str>, path: PathBuf) -> Remote {
        let cfg = ClientConfig { server_url: url.into(), ..Default::default() };
        let tokens: Box<dyn crate::tokens::TokenStore> = match token {
            Some(t) => Box::new(MemoryTokens::with(t)),
            None => Box::new(MemoryTokens::default()),
        };
        Remote::new(Ok(cfg), path, tokens)
    }

    async fn kind(r: &Remote) -> Option<String> {
        r.status().await.map(|s| s.kind)
    }

    #[tokio::test]
    async fn pair_another_app_gives_a_key_with_this_apps_address_that_pairs_once() {
        let (url, token, state) = status_server("0.1.0").await;
        let r = remote_for(&url, Some(&token), PathBuf::new());
        let key = r.new_pairing_key("Work laptop".into()).await.unwrap();
        let p = askbar_proto::Pairing::decode(&key).expect("a pairing key");
        assert_eq!((p.url.as_str(), p.cert_sha256), (url.as_str(), None), "the key points at the server this app uses");
        let db = state.db.lock().unwrap();
        assert!(askbar_server::auth::redeem_pair_code(&db, &p.code).unwrap().is_some());
        assert!(askbar_server::auth::redeem_pair_code(&db, &p.code).unwrap().is_none(), "once only");
        drop(db);
        assert!(r.new_pairing_key("x".repeat(61)).await.is_err(), "a name over 60 characters is refused");
        assert!(remote_for(&url, None, PathBuf::new()).new_pairing_key(String::new()).await.unwrap_err().contains("not paired"));
    }

    #[tokio::test]
    async fn status_reports_each_problem_with_its_kind() {
        let (url, token, state) = status_server("0.1.0").await;

        assert_eq!(kind(&remote_for(&url, Some(&token), PathBuf::new())).await, None);
        choose_provider(&state, "p0");
        assert_eq!(kind(&remote_for(&url, Some(&token), PathBuf::new())).await, None, "a provider that exists is fine");
        assert_eq!(kind(&remote_for(&url, None, PathBuf::new())).await.as_deref(), Some("not_paired"));
        assert_eq!(kind(&remote_for(&url, Some("abt_wrong"), PathBuf::new())).await.as_deref(), Some("rejected"));

        choose_provider(&state, "gone");
        let missing = remote_for(&url, Some(&token), PathBuf::new()).status().await.unwrap();
        assert_eq!(missing.kind, "provider_missing");
        assert!(missing.message.contains("no longer exists") && missing.message.contains("Pick another"));
        assert_eq!(
            missing.providers,
            [ProviderChoice { id: "p0".into(), name: "alpha".into() }, ProviderChoice { id: "p1".into(), name: "beta".into() }]
        );

        // Revoking the client turns "ok" into "rejected".
        choose_provider(&state, "p0");
        let r = remote_for(&url, Some(&token), PathBuf::new());
        assert_eq!(kind(&r).await, None);
        state.db.lock().unwrap().conn().execute("UPDATE clients SET revoked_at = 1", []).unwrap();
        assert_eq!(kind(&r).await.as_deref(), Some("rejected"));
    }

    #[tokio::test]
    async fn status_reports_update_needed_and_offline() {
        let (url, token, _state) = status_server("99.0.0").await;
        assert_eq!(kind(&remote_for(&url, Some(&token), PathBuf::new())).await.as_deref(), Some("update_needed"));

        // Nothing listens here: refused immediately, well inside the 2 second budget.
        let started = std::time::Instant::now();
        let st = remote_for("http://127.0.0.1:1", Some("t"), PathBuf::new()).status().await.unwrap();
        assert_eq!(st.kind, "offline");
        assert!(st.message.starts_with("Server offline"));
        assert!(started.elapsed() < Duration::from_secs(3));

        // A bad client.yaml is reported, not a crash.
        let broken = Remote::new(Err("Could not parse client.yaml".into()), PathBuf::new(), Box::new(MemoryTokens::default()));
        assert_eq!(broken.status().await.unwrap().kind, "problem");
    }

    #[tokio::test]
    async fn no_reachable_server_shows_server_offline() {
        // Paired or not, an unreachable server is reported as offline before anything else.
        for token in [Some("t"), None] {
            let r = remote_for("http://127.0.0.1:1", token, PathBuf::new());
            let st = r.status().await.unwrap();
            assert_eq!(st.kind, "offline", "token: {token:?}");
            assert!(st.message.starts_with("Server offline. Could not reach http://127.0.0.1:1"), "{}", st.message);

            // The window still opens with the built-in look and the saved hotkey.
            let settings = r.settings().await.unwrap();
            assert_eq!(settings.hotkey, ClientConfig::default().hotkey);
            assert_eq!(settings.ui.width, crate::client_config::Ui::default().width);
            assert!(settings.model.is_empty());
        }
        // Asking for history fails with a message instead of hanging.
        let r = remote_for("http://127.0.0.1:1", Some("t"), PathBuf::new());
        assert!(r.list_conversations().await.is_err());
    }

    #[tokio::test]
    async fn picking_another_provider_is_saved_on_the_server_and_clears_the_state() {
        let (url, token, state) = status_server("0.1.0").await;
        let dir = std::env::temp_dir().join(format!("askbar-pick-{}", std::process::id()));
        let path = dir.join("client.yaml");
        choose_provider(&state, "gone");
        let r = remote_for(&url, Some(&token), path.clone());
        assert_eq!(kind(&r).await.as_deref(), Some("provider_missing"));
        r.set_active_provider("p1".into()).await.unwrap();
        assert_eq!(kind(&r).await, None);
        let stored: String =
            state.db.lock().unwrap().conn().query_row("SELECT active_provider_id FROM user_prefs", [], |r| r.get(0)).unwrap();
        assert_eq!(stored, "p1");
        // Nothing about the choice is written to client.yaml (it isn't even created).
        assert!(!path.exists());
        assert!(r.set_active_provider("nope".into()).await.is_err(), "an unknown provider is refused");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_active_provider_follows_the_user_across_clients() {
        let (url, token, state) = status_server("0.1.0").await;
        let second_token = {
            let db = state.db.lock().unwrap();
            let user: String = db.conn().query_row("SELECT id FROM users", [], |r| r.get(0)).unwrap();
            let code = askbar_server::testing::add_pair_code(&db, &user, "second");
            askbar_server::auth::redeem_pair_code(&db, &code).unwrap().unwrap()
        };
        let (laptop, desktop) = (remote_for(&url, Some(&token), PathBuf::new()), remote_for(&url, Some(&second_token), PathBuf::new()));
        assert_eq!(desktop.settings().await.unwrap().provider, "alpha", "the first provider until one is picked");
        laptop.set_active_provider("p1".into()).await.unwrap();
        assert_eq!(desktop.settings().await.unwrap().provider, "beta", "picked on one client, used on the other");
    }

    #[tokio::test]
    async fn display_defaults_come_from_the_server_and_a_local_override_wins() {
        let (url, token, state) = status_server("0.1.0").await;
        {
            let db = state.db.lock().unwrap();
            let user: String = db.conn().query_row("SELECT id FROM users", [], |r| r.get(0)).unwrap();
            let dark: DisplaySettings = serde_json::from_str(
                r#"{"theme":"dark","width":900,"max_height":700,"history_width":300,"top_percent":0.3,"hide_on_blur":false}"#,
            )
            .unwrap();
            askbar_server::testing::update_snapshot(&db, |s| {
                s.user_display_defaults.insert(user, dark);
            });
        }
        let dir = std::env::temp_dir().join(format!("askbar-overrides-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("client.yaml");
        let r = remote_for(&url, Some(&token), path.clone());

        // No override: the server's defaults.
        let ui = r.settings().await.unwrap().ui;
        assert_eq!((ui.theme, ui.width, ui.hide_on_blur), (Theme::Dark, 900, false));

        // A local override wins for its field only.
        std::fs::write(&path, format!("server_url: {url}\nui:\n  theme: light\n")).unwrap();
        let ui = r.settings().await.unwrap().ui;
        assert_eq!((ui.theme, ui.width), (Theme::Light, 900));

        // Resetting it (removing it) returns the server's default.
        std::fs::write(&path, format!("server_url: {url}\n")).unwrap();
        assert_eq!(r.settings().await.unwrap().ui.theme, Theme::Dark);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn new_pairing(state: &Arc<askbar_server::AppState>, url: &str) -> String {
        let db = state.db.lock().unwrap();
        let user: String = db.conn().query_row("SELECT id FROM users", [], |r| r.get(0)).unwrap();
        let code = askbar_server::testing::add_pair_code(&db, &user, "second profile");
        askbar_proto::encode_pairing(url, &code)
    }

    #[tokio::test]
    async fn pairing_stores_the_token_and_the_server_and_keeps_other_settings() {
        let (url, _first_token, state) = status_server("0.1.0").await;
        let dir = std::env::temp_dir().join(format!("askbar-pair-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("profile2").join("client.yaml");
        let tokens = MemoryTokens::default();

        let s = new_pairing(&state, &url);
        pair(&s, &path, &tokens).await.unwrap();
        let token = tokens.get().expect("token stored");
        let yaml = std::fs::read_to_string(&path).unwrap();
        assert!(yaml.contains(&url) && !yaml.contains(&token) && !yaml.to_lowercase().contains("token"));
        let cfg = ClientConfig::load(&path).unwrap();
        assert_eq!(cfg.server_url, url);

        // The new profile works end to end, and the code can't be used twice.
        let r = Remote::new(Ok(cfg), path.clone(), Box::new(MemoryTokens::with(&token)));
        assert_eq!(kind(&r).await, None);
        assert!(r.list_providers().await.unwrap().len() == 2);
        assert!(pair(&s, &path, &MemoryTokens::default()).await.unwrap_err().contains("already used"));

        // Re-pairing updates the server but keeps hand-edited settings.
        std::fs::write(&path, format!("server_url: http://old.invalid\nhotkey: Alt+Space\nallow_insecure: true\n")).unwrap();
        pair(&new_pairing(&state, &url), &path, &tokens).await.unwrap();
        let cfg = ClientConfig::load(&path).unwrap();
        assert_eq!((cfg.server_url.as_str(), cfg.hotkey.as_str(), cfg.allow_insecure), (url.as_str(), "Alt+Space", true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn bad_pairing_attempts_explain_themselves_and_store_nothing() {
        let dir = std::env::temp_dir().join(format!("askbar-pair-bad-{}", std::process::id()));
        let path = dir.join("client.yaml");
        let tokens = MemoryTokens::default();
        let err = |r: Result<(), String>| r.unwrap_err();

        assert!(err(pair("hello", &path, &tokens).await).contains("askbar-pair:"));
        let lan = askbar_proto::encode_pairing("http://192.168.1.5:8080", "c");
        assert!(err(pair(&lan, &path, &tokens).await).contains("allow_insecure"));
        let gone = askbar_proto::encode_pairing("http://127.0.0.1:1", "c");
        assert!(err(pair(&gone, &path, &tokens).await).contains("Could not reach"));

        let (url, _t, _state) = status_server("0.1.0").await;
        let wrong = askbar_proto::encode_pairing(&url, "not-a-real-code");
        assert!(err(pair(&wrong, &path, &tokens).await).contains("wrong, expired"));
        let (old_url, _t, _state) = status_server("99.0.0").await;
        let too_old = askbar_proto::encode_pairing(&old_url, "x");
        assert!(err(pair(&too_old, &path, &tokens).await).contains("too old"));

        assert!(tokens.get().is_none() && !path.exists());
    }

    /// A pairing key names the server's own certificate: the app reaches that server over HTTPS by
    /// address, and refuses one that shows another certificate.
    #[tokio::test]
    async fn pairing_trusts_exactly_the_certificate_in_the_pairing_string() {
        let dir = std::env::temp_dir().join(format!("askbar-pair-pin-{}", std::process::id()));
        let path = dir.join("client.yaml");
        let tokens = MemoryTokens::default();
        let (url, fp) = crate::pinned_tls::tests::self_signed_server().await;
        let with = |pin: Option<String>| askbar_proto::Pairing { url: url.clone(), code: "c".into(), cert_sha256: pin }.encode();

        let other = pair(&with(Some("0".repeat(64))), &path, &tokens).await.unwrap_err();
        assert!(other.contains("different certificate"), "{other}");
        let unpinned = pair(&with(None), &path, &tokens).await.unwrap_err();
        assert!(unpinned.contains("Could not reach"), "{unpinned}");
        // The right pin gets through TLS; this stub server then answers something that is not a token.
        let pinned = pair(&with(Some(fp)), &path, &tokens).await.unwrap_err();
        assert!(pinned.contains("unexpected reply"), "{pinned}");
        assert!(tokens.get().is_none() && !path.exists());
    }

    #[test]
    fn the_pin_is_kept_in_client_yaml_and_left_out_when_there_is_none() {
        let dir = std::env::temp_dir().join(format!("askbar-pin-yaml-{}", std::process::id()));
        let path = dir.join("client.yaml");
        let mut cfg = ClientConfig { server_url: "https://192.168.1.20".into(), ..Default::default() };
        cfg.save(&path).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("cert_sha256"));
        cfg.cert_sha256 = Some("ab".repeat(32));
        cfg.save(&path).unwrap();
        assert_eq!(ClientConfig::load(&path).unwrap().cert_sha256, Some("ab".repeat(32)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Pair with a code the admin listed in its snapshot, have the admin revoke the client
    /// (again through the snapshot), and the client says "rejected".
    #[tokio::test]
    async fn revoking_a_client_shows_token_rejected() {
        use askbar_server::{secrets::Secrets, testing, AppState};
        let db = askbar_server::Db::open_in_memory().unwrap();
        let user = db.seed_server().unwrap();
        testing::add_provider(&db, testing::provider("p", "alpha", "http://x/v1", "m"));
        let code = testing::add_pair_code(&db, &user, "test client");
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api_url = format!("http://{}", l.local_addr().unwrap());
        let state = AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), "0.1.0");
        let app = askbar_server::client_router(state.clone());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });

        let dir = std::env::temp_dir().join(format!("askbar-revoke-{}", std::process::id()));
        let path = dir.join("client.yaml");
        let tokens = MemoryTokens::default();
        pair(&askbar_proto::encode_pairing(&api_url, &code), &path, &tokens).await.unwrap();

        let r = Remote::new(ClientConfig::load(&path), path.clone(), Box::new(MemoryTokens::with(&tokens.get().unwrap())));
        assert_eq!(kind(&r).await, None);

        let client_id: String =
            state.db.lock().unwrap().conn().query_row("SELECT id FROM clients WHERE name = 'test client'", [], |r| r.get(0)).unwrap();
        testing::update_snapshot(&state.db.lock().unwrap(), |s| s.revoked_client_ids.push(client_id));

        let status = r.status().await.unwrap();
        assert_eq!(status.kind, "rejected");
        assert!(status.message.starts_with("Token rejected"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Manual end-to-end check against a running server, using the real keychain
    /// and the same `client.yaml` path logic as the app. Run with:
    ///   ASKBAR_E2E_PAIRING='askbar-pair:…' ASKBAR_CLIENT_CONFIG=/tmp/profile2/client.yaml \
    ///     cargo test -p askbar e2e -- --ignored --nocapture
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "needs a running server and a keychain"]
    async fn e2e_pair_and_ask_against_a_running_server() {
        let pairing = std::env::var("ASKBAR_E2E_PAIRING").expect("set ASKBAR_E2E_PAIRING");
        let path = crate::client_config::path();
        pair(&pairing, &path, &crate::tokens::Keychain::new(&path)).await.unwrap();

        // A fresh Remote, as the app builds it at start-up.
        let remote = Remote::new(ClientConfig::load(&path), path.clone(), Box::new(crate::tokens::Keychain::new(&path)));
        assert_eq!(remote.status().await, None);
        let events = Mutex::new(Vec::new());
        let (_tx, rx) = oneshot::channel();
        remote.ask(None, "hello from the second profile".into(), rx, &|e| events.lock().unwrap().push(e)).await;
        let events = events.into_inner().unwrap();
        assert!(matches!(events.last(), Some(StreamEvent::Done)), "{}", serde_json::to_string(&events.last()).unwrap());
        let reply: String =
            events.iter().filter_map(|e| if let StreamEvent::Delta { text } = e { Some(text.as_str()) } else { None }).collect();
        println!("reply: {}", reply.lines().next().unwrap_or(""));
        assert!(!reply.is_empty());
    }
}
