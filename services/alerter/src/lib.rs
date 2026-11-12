//! `alerter` (ADR 0055, ADR 0006): alert delivery in its own container, behind `/api/alerter/v1`.
//!
//! - **It decides no rule.** The auditor and the admin send events: a kind from a fixed list, a severity, a time and typed
//!   fields. The alerter turns a kind into one of its own sentences, so a sender cannot make it say anything else.
//! - **Secrets in memory only.** The admin pushes the delivery settings in `PUT /config`; they are never written to disk, a log,
//!   an error or a response. Until they arrive every route but the status, the config and the restart answers `503 not_configured`.
//! - **Two listeners, two links.** The auditor may send its own kinds; the admin may send its own, push the configuration, read
//!   the status, test delivery and ask for a restart. A token of one is no good on the other's routes or network.
//! - **It checks for new releases once a day** ([`releases`]) and announces a newer one once.
//! - **Repeats are limited and remembered** ([`limits`], [`ledger`]), and each alert's outcome goes to the auditor as metadata.

pub mod deliver;
pub mod ledger;
pub mod limits;
pub mod releases;
pub mod templates;

use askbar_proto::alerter::{
    AlerterConfig, AlerterStatus, Channel, ChannelResult, Event, EventTaken, Fields, Kind, NtfyConfig, Refusal, ReleaseStatus, Sender,
    Severity, SmtpConfig, TestResult,
};
use askbar_proto::http::ApiError;
use askbar_proto::paths::{self, ALERTER_V1};
use askbar_proto::ErrorCode;
use askbar_service::audit_client::{now_parts, AuditClient};
use askbar_service::configured::Gate;
use askbar_service::links::{Allow, Caller, Link, Links};
use askbar_service::restart::Restarter;
use askbar_service::status::BootId;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::Method;
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};
use deliver::Problem;
use serde::de::DeserializeOwned;
use std::collections::VecDeque;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use templates::Rendered;
use tokio::sync::Notify;

/// The port both listeners use, on the address each caller's network gives them.
pub const DEFAULT_PORT: u16 = 8085;
const MAX_BODY: usize = 64 * 1024;
/// How many alerts wait to be delivered; past it the oldest is dropped, and the drops are counted.
pub const QUEUE_LIMIT: usize = 200;

/// How long the alerter waits between tries and how long it gives up after. Tests shorten it.
#[derive(Debug, Clone)]
pub struct Timing {
    /// The wait before each retry; one more try than there are entries.
    pub backoff: Vec<Duration>,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing { backoff: [2, 10, 30, 120, 300, 600].into_iter().map(Duration::from_secs).collect() }
    }
}

/// What a `PUT /config` put in place.
struct Applied {
    version: u64,
    ntfy: Option<NtfyConfig>,
    email: Option<SmtpConfig>,
    /// The key the export is encrypted to (public; ADR 0013). `None` until the admin has made one.
    backup_public_key: Option<String>,
}

impl Applied {
    /// The channels that are set up.
    fn channels(&self) -> Vec<Channel> {
        [self.ntfy.as_ref().map(|_| Channel::Ntfy), self.email.as_ref().map(|_| Channel::Email)].into_iter().flatten().collect()
    }
}

fn channel_name(c: Channel) -> &'static str {
    match c {
        Channel::Ntfy => "ntfy",
        Channel::Email => "email",
    }
}

/// One alert waiting to be delivered.
struct Pending {
    /// The kind's name, for the auditor's record (`alert_summary` for the cap's summary).
    kind: &'static str,
    severity: Severity,
    rendered: Rendered,
    /// Each channel has its own copy, so one that fails does not hold up or repeat the other.
    channel: Channel,
    attempts: u32,
    due: Instant,
}

#[derive(Default)]
struct Queue {
    items: VecDeque<Pending>,
    dropped: u64,
}

pub struct Alerter {
    pub boot_id: BootId,
    audit: Arc<AuditClient>,
    http: reqwest::Client,
    config: RwLock<Option<Arc<Applied>>>,
    state: Mutex<limits::State>,
    ledger: ledger::Ledger,
    data_dir: PathBuf,
    pub releases: releases::Checker,
    queue: Mutex<Queue>,
    wake: Notify,
    /// Wakes the release check when the settings change, so a channel set up after a release was found hears of it at once.
    release_wake: Notify,
    timing: Timing,
    pub restarter: Restarter,
}

impl Alerter {
    pub fn new(audit: Arc<AuditClient>, boot_id: BootId, restarter: Restarter, data_dir: &FsPath, timing: Timing) -> Arc<Alerter> {
        let ledger = ledger::Ledger::new(data_dir);
        let state = ledger.load();
        Arc::new(Alerter {
            boot_id,
            audit,
            http: reqwest::Client::builder().connect_timeout(Duration::from_secs(5)).build().expect("reqwest client"),
            config: RwLock::new(None),
            state: Mutex::new(state),
            ledger,
            data_dir: data_dir.to_path_buf(),
            releases: releases::Checker::new(data_dir, release_url(), env!("CARGO_PKG_VERSION").to_string()),
            queue: Mutex::new(Queue::default()),
            wake: Notify::new(),
            release_wake: Notify::new(),
            timing,
            restarter,
        })
    }

    fn applied(&self) -> Option<Arc<Applied>> {
        self.config.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn is_configured(&self) -> bool {
        self.applied().is_some()
    }

    pub fn config_version(&self) -> Option<u64> {
        self.applied().map(|a| a.version)
    }

    /// Replaces the delivery settings. They are checked first, and nothing changes if they are refused.
    pub fn apply_config(&self, c: &AlerterConfig) -> Result<(), &'static str> {
        if let Some(n) = &c.ntfy {
            deliver::ntfy::validate(n)?;
        }
        if let Some(m) = &c.email {
            deliver::smtp::validate(m)?;
        }
        *self.config.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(Applied {
            version: c.version,
            ntfy: c.ntfy.clone(),
            email: c.email.clone(),
            backup_public_key: c.backup_public_key.clone().filter(|k| askbar_service::backup::is_recipient(k)),
        }));
        tracing::info!(version = c.version, ntfy = c.ntfy.is_some(), email = c.email.is_some(), "configuration applied");
        self.audit.config_applied(c.version as i64);
        self.wake.notify_one();
        self.release_wake.notify_one();
        Ok(())
    }

    /// Saves the limiter's state; a failure is logged and never stops an alert.
    fn save(&self, state: &limits::State) {
        if let Err(e) = self.ledger.save(state) {
            tracing::warn!("the repeat ledger could not be saved: {}", e.kind());
        }
    }

    /// Queues the alert for every channel that is set up now. With none, nothing is queued and the log says so.
    fn enqueue(&self, kind: &'static str, severity: Severity, rendered: Rendered) {
        let channels = self.applied().map(|a| a.channels()).unwrap_or_default();
        if channels.is_empty() {
            // The admin's banner says delivery is not set up.
            tracing::info!(kind, "an alert was not delivered: no delivery channel is set up");
            return;
        }
        {
            let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
            for channel in channels {
                if q.items.len() >= QUEUE_LIMIT {
                    q.items.pop_front();
                    q.dropped += 1;
                }
                q.items.push_back(Pending { kind, severity, rendered: rendered.clone(), channel, attempts: 0, due: Instant::now() });
            }
        }
        self.wake.notify_one();
    }

    /// Takes an event from `from`: checks it against its kind, limits repeats, and queues it for delivery. A repeat is
    /// counted and recorded as suppressed, and is not an error.
    pub fn submit(&self, event: &Event, from: Sender) -> Result<EventTaken, Refusal> {
        event.check(from)?;
        let now = now_parts().0;
        let (admit, snapshot) = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let admit = st.admit(event, now);
            (admit, st.clone())
        };
        self.save(&snapshot);
        let channels = self.applied().map(|a| a.channels()).unwrap_or_default();
        match admit {
            limits::Admit::Send { suppressed } => {
                let mut r = templates::render(event);
                if suppressed > 0 {
                    r.body.push_str(&format!(" ({suppressed} more like this were held back.)"));
                }
                self.enqueue(event.kind.name(), event.severity, r);
                Ok(EventTaken { suppressed: false })
            }
            limits::Admit::Summary { held } => {
                let body = format!(
                    "Alerts are coming faster than {} an hour, so later ones are held back until the hour ends. {held} have been held back so far.",
                    limits::HOURLY_CAP
                );
                self.enqueue("alert_summary", Severity::Warning, Rendered { title: "Many alerts".into(), body });
                Ok(EventTaken { suppressed: true })
            }
            limits::Admit::Suppress => {
                for c in channels {
                    self.audit.alert_delivery(event.kind.name(), channel_name(c), "suppressed", 0);
                }
                Ok(EventTaken { suppressed: true })
            }
            limits::Admit::Drop => Ok(EventTaken { suppressed: true }),
        }
    }

    fn note_result(&self, ok: bool) {
        let snapshot = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let now = Some(now_parts().0);
            if ok {
                st.last_ok = now;
            } else {
                st.last_failed = now;
            }
            st.clone()
        };
        self.save(&snapshot);
    }

    /// Delivers alerts for as long as the process runs: each as soon as it is due, retried with backoff, and recorded with the
    /// auditor when it has been sent or given up on.
    pub async fn deliver_forever(self: Arc<Self>) {
        loop {
            let Some(cfg) = self.applied() else {
                let _ = tokio::time::timeout(Duration::from_secs(1), self.wake.notified()).await;
                continue;
            };
            let next = {
                let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
                let now = Instant::now();
                match q.items.iter().position(|p| p.due <= now) {
                    Some(i) => Ok(q.items.remove(i).expect("an item at that position")),
                    None => Err(q.items.iter().map(|p| p.due).min()),
                }
            };
            let mut item = match next {
                Ok(item) => item,
                Err(due) => {
                    let wait =
                        due.map_or(Duration::from_secs(1), |d| d.saturating_duration_since(Instant::now()).min(Duration::from_secs(1)));
                    let _ = tokio::time::timeout(wait.max(Duration::from_millis(5)), self.wake.notified()).await;
                    continue;
                }
            };
            if !cfg.channels().contains(&item.channel) {
                // The channel was taken away since the alert was queued: there is nowhere to send it now.
                tracing::info!(kind = item.kind, "an alert was not delivered: its channel is no longer set up");
                continue;
            }
            let channel = channel_name(item.channel);
            item.attempts += 1;
            match self.send(&cfg, item.channel, item.severity, &item.rendered).await {
                Ok(()) => {
                    self.note_result(true);
                    self.audit.alert_delivery(item.kind, channel, "sent", item.attempts);
                }
                Err(problem) => {
                    self.note_result(false);
                    tracing::warn!(
                        kind = item.kind,
                        channel,
                        attempt = item.attempts,
                        problem = problem.as_str(),
                        "an alert was not delivered"
                    );
                    match self.timing.backoff.get(item.attempts as usize - 1) {
                        Some(wait) => {
                            item.due = Instant::now() + *wait;
                            let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
                            if q.items.len() >= QUEUE_LIMIT {
                                q.items.pop_front();
                                q.dropped += 1;
                            }
                            q.items.push_back(item);
                        }
                        None => self.audit.alert_delivery(item.kind, channel, "failed", item.attempts),
                    }
                }
            }
        }
    }

    /// One delivery through one channel of `cfg`.
    async fn send(&self, cfg: &Applied, channel: Channel, severity: Severity, r: &Rendered) -> Result<(), Problem> {
        match (channel, &cfg.ntfy, &cfg.email) {
            (Channel::Ntfy, Some(n), _) => deliver::ntfy::send(&self.http, n, severity, r).await,
            (Channel::Email, _, Some(m)) => deliver::smtp::send(m, severity, r).await,
            _ => Err(Problem::Unreachable),
        }
    }

    /// Sends a test alert now, through every channel that is set up, and says what each did. It is not limited and not queued.
    pub async fn test(&self) -> TestResult {
        let mut results = Vec::new();
        let Some(cfg) = self.applied() else { return TestResult { results } };
        let event = Event { kind: Kind::Test, severity: Severity::Info, at: now_parts().0, fields: Default::default() };
        let rendered = templates::render(&event);
        for channel in cfg.channels() {
            let outcome = self.send(&cfg, channel, event.severity, &rendered).await;
            self.note_result(outcome.is_ok());
            self.audit.alert_delivery("test", channel_name(channel), if outcome.is_ok() { "sent" } else { "failed" }, 1);
            results.push(ChannelResult {
                channel,
                ok: outcome.is_ok(),
                problem: outcome.err().map(Problem::as_str).unwrap_or_default().to_string(),
            });
        }
        TestResult { results }
    }

    /// One round of the release check: ask if a check is due, then announce a newer version that has not been announced, once,
    /// when a channel is there to carry it (so a channel set up later still hears of it).
    pub async fn release_round(&self) {
        if !self.releases.enabled() {
            return;
        }
        let now = now_parts().0;
        if self.releases.due_in(now) == 0 {
            self.releases.check(&self.http, now).await;
        }
        let Some(version) = self.releases.to_announce() else { return };
        if self.applied().is_none_or(|a| a.channels().is_empty()) {
            return;
        }
        let event = Event {
            kind: Kind::ReleaseAvailable,
            severity: Severity::Info,
            at: now,
            fields: Fields { version: Some(version.clone()), ..Default::default() },
        };
        if self.submit(&event, Sender::Alerter).is_ok() {
            self.releases.mark_announced(&version);
        }
    }

    /// Checks for new releases for as long as the process runs: a round every hour at most, and a check when one is due.
    pub async fn releases_forever(self: Arc<Self>) {
        loop {
            self.release_round().await;
            let wait = self.releases.due_in(now_parts().0).clamp(1, 3600);
            let wait = Duration::from_secs(wait.min(self.releases.source().retry.as_secs().max(1)));
            let _ = tokio::time::timeout(wait, self.release_wake.notified()).await;
        }
    }

    pub fn release_status(&self) -> ReleaseStatus {
        self.releases.status()
    }

    pub fn status(&self) -> AlerterStatus {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        AlerterStatus {
            config_version: self.config_version(),
            boot_id: self.boot_id.as_str().to_string(),
            last_delivery_ok_at: st.last_ok,
            last_delivery_failed_at: st.last_failed,
            queued: q.items.len() as u32,
            dropped: q.dropped,
        }
    }
}

// ------------------------------------------------------------------ the links

/// What one link may call: methods and paths after `/api/alerter/v1`.
type Routes = &'static [(&'static str, &'static str)];

const AUDITOR_ROUTES: Routes = &[("POST", "/events")];
const ADMIN_ROUTES: Routes = &[
    ("GET", "/status"),
    ("PUT", "/config"),
    ("POST", "/restart"),
    ("POST", "/events"),
    ("POST", "/test"),
    ("GET", "/releases"),
    ("POST", "/exports"),
    ("POST", "/imports"),
];

/// The two links: name as `bootstrap` names it (the file `accept/<name>.sha256`), the caller, and the routes.
pub const LINKS: &[(&str, &str, Routes)] = &[("auditor-alerter", "auditor", AUDITOR_ROUTES), ("admin-alerter", "admin", ADMIN_ROUTES)];

fn full(rest: &str) -> String {
    paths::join(ALERTER_V1, rest)
}

fn links_for(token_dir: &FsPath, link: &'static (&'static str, &'static str, Routes)) -> Links {
    let (name, caller, routes) = link;
    Links::new(vec![Link {
        caller,
        hash_file: token_dir.join("accept").join(format!("{name}.sha256")),
        allow: routes.iter().map(|(m, p)| Allow::method(Method::from_bytes(m.as_bytes()).expect("a method"), &full(p))).collect(),
    }])
    .named("alerter")
}

/// The check on the auditor's listener: only the auditor's token, only `POST /events`.
pub fn auditor_links(token_dir: &FsPath) -> Links {
    links_for(token_dir, &LINKS[0])
}

/// The check on the admin's listener: only the admin's token.
pub fn admin_links(token_dir: &FsPath) -> Links {
    links_for(token_dir, &LINKS[1])
}

/// Where the release check asks: `ASKBAR_ALERTER_RELEASES_URL` (an address, for a mock or a mirror; `off` for no check),
/// otherwise GitHub's latest-release API for the public repository.
fn release_url() -> String {
    std::env::var("ASKBAR_ALERTER_RELEASES_URL").ok().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| releases::DEFAULT_URL.into())
}

/// The credential volume, read-only: `ASKBAR_ALERTER_TOKEN_DIR`, default `/alerter-token`.
pub fn token_dir() -> PathBuf {
    std::env::var("ASKBAR_ALERTER_TOKEN_DIR").ok().filter(|p| !p.trim().is_empty()).map_or_else(|| "/alerter-token".into(), PathBuf::from)
}

/// The data volume: `ASKBAR_ALERTER_DATA_DIR`, default `/alerter-data`.
pub fn data_dir() -> PathBuf {
    std::env::var("ASKBAR_ALERTER_DATA_DIR").ok().filter(|p| !p.trim().is_empty()).map_or_else(|| "/alerter-data".into(), PathBuf::from)
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

/// The routes that answer before the alerter has been configured.
fn open_before_config(path: &str) -> bool {
    [full("/health"), full("/status"), full("/config"), full("/restart"), full("/imports")].iter().any(|p| p == path)
}

/// The auditor's listener.
pub fn auditor_router(a: Arc<Alerter>, links: Links) -> Router {
    let gate = gate(&a);
    let protected = Router::new()
        .route(&full("/events"), post(events))
        .layer(axum::middleware::from_fn_with_state(gate, askbar_service::configured::require))
        .layer(axum::middleware::from_fn_with_state(links, askbar_service::links::require));
    shell(a, protected)
}

/// The admin's listener.
pub fn admin_router(a: Arc<Alerter>, links: Links) -> Router {
    let restart = a.restarter.clone();
    let gate = gate(&a);
    let protected = Router::new()
        .route(&full("/status"), get(status))
        .route(&full("/config"), put(put_config))
        .route(&full("/restart"), post(askbar_service::restart::handler).with_state(restart))
        .route(&full("/events"), post(events))
        .route(&full("/test"), post(test))
        .route(&full("/releases"), get(releases))
        .route(&full("/exports"), post(export))
        .route(&full("/imports"), post(import))
        .layer(axum::middleware::from_fn_with_state(gate, askbar_service::configured::require))
        .layer(axum::middleware::from_fn_with_state(links, askbar_service::links::require));
    shell(a, protected)
}

fn gate(a: &Arc<Alerter>) -> Gate {
    let a = a.clone();
    Gate::new(move || a.is_configured(), open_before_config)
}

fn shell(a: Arc<Alerter>, protected: Router<Arc<Alerter>>) -> Router {
    Router::new()
        .route(&full("/health"), get(|| async { "ok" }))
        .merge(protected)
        .fallback(|| async { ApiError::code(ErrorCode::NotFound) })
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(a)
}

type Failure = ApiError;
type St = State<Arc<Alerter>>;

fn bad(detail: impl Into<String>) -> Failure {
    ApiError::new(ErrorCode::BadRequest, detail)
}

/// A body of JSON, refused with a sentence that names no value (a serde message can quote one).
fn parse<T: DeserializeOwned>(body: &Bytes) -> Result<T, Failure> {
    serde_json::from_slice(body)
        .map_err(|_| bad("The body is not what this route takes: an unknown kind, severity or field, or a value out of range."))
}

async fn status(State(a): St) -> Json<AlerterStatus> {
    Json(a.status())
}

/// Replace the delivery settings. The answer holds none of them.
async fn put_config(State(a): St, body: Bytes) -> Result<Json<serde_json::Value>, Failure> {
    let c: AlerterConfig = parse(&body)?;
    a.apply_config(&c).map_err(bad)?;
    Ok(Json(serde_json::json!({ "version": c.version })))
}

fn sender(caller: &str) -> Sender {
    if caller == "auditor" {
        Sender::Auditor
    } else {
        Sender::Admin
    }
}

/// An event from the auditor or the admin, checked against what that link may send.
async fn events(
    State(a): St,
    Extension(Caller(caller)): Extension<Caller>,
    body: Bytes,
) -> Result<(axum::http::StatusCode, Json<EventTaken>), Failure> {
    let e: Event = parse(&body)?;
    match a.submit(&e, sender(caller)) {
        Ok(taken) => Ok((axum::http::StatusCode::ACCEPTED, Json(taken))),
        Err(Refusal::NotYours) => Err(ApiError::new(ErrorCode::Forbidden, Refusal::NotYours.to_string())),
        Err(r) => Err(bad(r.to_string())),
    }
}

/// Send a test alert through every channel that is set up and say what each did.
async fn test(State(a): St) -> Json<TestResult> {
    Json(a.test().await)
}

/// `POST /exports` (ADR 0013): what the alerter keeps on its volume (the repeat ledger and the release state), as a tar encrypted
/// to the backup public key the admin pushed. Both files are small, so this is built in memory. It holds no secret: the delivery
/// settings are never on disk. With no key, nothing is made: `503 backup_key_missing`.
async fn export(State(a): St) -> Result<axum::response::Response, Failure> {
    let Some(recipient) = a.applied().and_then(|c| c.backup_public_key.clone()) else {
        return Err(ApiError::code(ErrorCode::BackupKeyMissing));
    };
    let dir = a.data_dir.clone();
    let now = now_parts().0;
    let made = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, ApiError> {
        let internal = || ApiError::code(ErrorCode::Internal);
        let mut tar = Vec::new();
        for name in [ledger::FILE, releases::FILE] {
            if let Ok(data) = std::fs::read(dir.join(name)) {
                askbar_service::tar::append(&mut tar, name, &data, now);
            }
        }
        askbar_service::tar::finish(&mut tar);
        askbar_service::backup::encrypt(&recipient, &tar).map_err(|_| internal())
    })
    .await
    .map_err(|_| ApiError::code(ErrorCode::Internal))??;
    axum::response::Response::builder()
        .header(axum::http::header::CONTENT_TYPE, "application/octet-stream")
        .body(axum::body::Body::from(made))
        .map_err(|_| ApiError::code(ErrorCode::Internal))
}

/// `POST /imports` (ADR 0013): what `POST /exports` made, opened by the admin, into an alerter that has kept nothing yet (an empty
/// ledger and no release state), else `409 not_empty`. The body is the plain tar of `ledger.json` and `releases.json` (those that were
/// there). Both are read and checked before either is kept, so a refused import changes nothing. It answers before the settings arrive:
/// a restore comes first, and the admin pushes the settings after.
async fn import(State(a): St, body: Bytes) -> Result<Json<serde_json::Value>, Failure> {
    let files = askbar_service::tar::read(&body).ok_or_else(|| bad("The import is not a backup this alerter made."))?;
    let (mut ledger_state, mut release_state) = (None, None);
    for (name, data) in &files {
        match name.as_str() {
            ledger::FILE if ledger_state.is_none() => {
                ledger_state = Some(
                    serde_json::from_slice::<limits::State>(data)
                        .map_err(|_| bad("The ledger in the import is not one this alerter made."))?,
                )
            }
            releases::FILE if release_state.is_none() => {
                release_state = Some(
                    serde_json::from_slice::<releases::Saved>(data)
                        .map_err(|_| bad("The release state in the import is not one this alerter made."))?,
                )
            }
            _ => return Err(bad("The import holds a file this alerter did not make.")),
        }
    }
    let mut state = a.state.lock().unwrap_or_else(|e| e.into_inner());
    if *state != limits::State::default() || a.releases.saved() != releases::Saved::default() {
        return Err(ApiError::code(ErrorCode::NotEmpty));
    }
    if let Some(l) = ledger_state {
        a.ledger.save(&l).map_err(|_| ApiError::code(ErrorCode::Internal))?;
        *state = l;
    }
    if let Some(r) = release_state {
        a.releases.replace(r);
    }
    Ok(Json(serde_json::json!({ "files": files.len() })))
}

/// What the daily release check found.
async fn releases(State(a): St) -> Json<ReleaseStatus> {
    Json(a.release_status())
}
