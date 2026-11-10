//! The per-link bearer check (ADR 0004, 0028). Each link is one caller of this service: the caller
//! holds a token, this service holds only its SHA-256 (hex) in a file, and the caller may call only
//! the routes listed for its link.
//!
//! - The hash file is read on every request, so a replaced hash works at once; a missing,
//!   unreadable or malformed file accepts nothing (fail closed).
//! - No token, or one that matches no link: `401 unauthorized`.
//! - A token that matches a link, on a route that link may not call: `403 forbidden`.
//! - On success the caller's name is put in the request's extensions as [`Caller`].
//! - Repeated tokens that match no link are counted and logged once per episode ([`InvalidTokens`],
//!   ADR 0024). The log line names the links this service accepts and never holds a token.

use askbar_proto::http::ApiError;
use askbar_proto::problem::ErrorCode;
use axum::{
    extract::{Request, State},
    http::{header, Method},
    middleware::Next,
    response::Response,
};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;

/// Who called, once the token has been checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caller(pub &'static str);

/// One route a link may call: a method (or any) and a path pattern, where `*` stands for one
/// path segment and a trailing `/**` for any number of them.
#[derive(Debug, Clone)]
pub struct Allow {
    method: Option<Method>,
    pattern: String,
}

impl Allow {
    pub fn any(pattern: &str) -> Allow {
        Allow { method: None, pattern: pattern.to_string() }
    }
    pub fn method(method: Method, pattern: &str) -> Allow {
        Allow { method: Some(method), pattern: pattern.to_string() }
    }
    fn matches(&self, method: &Method, path: &str) -> bool {
        if self.method.as_ref().is_some_and(|m| m != method) {
            return false;
        }
        let want: Vec<&str> = self.pattern.trim_start_matches('/').split('/').collect();
        let got: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        if want.last() == Some(&"**") {
            let head = &want[..want.len() - 1];
            return got.len() >= head.len() && head.iter().zip(&got).all(|(w, g)| *w == "*" || w == g);
        }
        want.len() == got.len() && want.iter().zip(&got).all(|(w, g)| *w == "*" || w == g)
    }
}

/// One caller of this service.
#[derive(Debug, Clone)]
pub struct Link {
    pub caller: &'static str,
    pub hash_file: PathBuf,
    pub allow: Vec<Allow>,
}

/// Invalid tokens seen by one listener in a sliding window, for the repeated-token alert (ADR 0024).
///
/// It reports a count once when the window first holds `THRESHOLD` of them, then stays quiet until the
/// count has dropped below the threshold again, so one fault or one attack is one alert, not a flood.
#[derive(Debug)]
pub struct InvalidTokens {
    window: Duration,
    threshold: usize,
    times: VecDeque<Instant>,
    armed: bool,
}

impl InvalidTokens {
    /// Five invalid tokens within ten minutes.
    pub const THRESHOLD: usize = 5;
    pub const WINDOW: Duration = Duration::from_secs(600);
    /// What is remembered at most, so a flood cannot grow it.
    const KEEP: usize = 1024;

    pub fn new(window: Duration, threshold: usize) -> InvalidTokens {
        InvalidTokens { window, threshold, times: VecDeque::new(), armed: true }
    }

    fn prune(&mut self, now: Instant) {
        while self.times.front().is_some_and(|t| now.saturating_duration_since(*t) > self.window) {
            self.times.pop_front();
        }
    }

    /// Notes one invalid token at `now`. `Some(count)` when this one should be reported.
    pub fn record(&mut self, now: Instant) -> Option<usize> {
        self.times.push_back(now);
        if self.times.len() > Self::KEEP {
            self.times.pop_front();
        }
        self.prune(now);
        let count = self.times.len();
        if count < self.threshold {
            self.armed = true;
            return None;
        }
        if self.armed {
            self.armed = false;
            return Some(count);
        }
        None
    }

    /// How many are in the window at `now`.
    pub fn count(&mut self, now: Instant) -> usize {
        self.prune(now);
        self.times.len()
    }
}

/// Every link of one listener.
#[derive(Debug, Clone)]
pub struct Links {
    links: Arc<Vec<Link>>,
    /// The service that holds this listener, to name its links (`<caller>-<service>`) in the alert.
    service: &'static str,
    invalid: Arc<Mutex<InvalidTokens>>,
}

impl Links {
    pub fn new(links: Vec<Link>) -> Links {
        Links {
            links: Arc::new(links),
            service: "",
            invalid: Arc::new(Mutex::new(InvalidTokens::new(InvalidTokens::WINDOW, InvalidTokens::THRESHOLD))),
        }
    }

    /// Names the service this listener belongs to, so the alert names its links as `bootstrap` does.
    pub fn named(mut self, service: &'static str) -> Links {
        self.service = service;
        self
    }

    /// The names of the links this listener accepts, as `bootstrap` names them.
    fn link_names(&self) -> String {
        let name = |l: &Link| if self.service.is_empty() { l.caller.to_string() } else { format!("{}-{}", l.caller, self.service) };
        self.links.iter().map(name).collect::<Vec<_>>().join(", ")
    }

    /// Counts one token that matched no link. The line to log, when this one crosses the threshold
    /// (ADR 0024); it names the links and the count and never holds the token.
    pub fn note_invalid(&self, now: Instant) -> Option<String> {
        let (count, window) = {
            let mut invalid = self.invalid.lock().unwrap_or_else(|e| e.into_inner());
            (invalid.record(now)?, invalid.window)
        };
        Some(format!(
            "{count} invalid tokens in {} minutes on the link(s) {}: the caller may hold an old token, or something is guessing",
            window.as_secs() / 60,
            self.link_names()
        ))
    }

    /// How many invalid tokens this listener has seen in the window.
    pub fn invalid_count(&self) -> usize {
        self.invalid.lock().unwrap_or_else(|e| e.into_inner()).count(Instant::now())
    }

    /// The link whose hash the token matches, if any. Every link is checked, in constant time per
    /// link, whatever matched before.
    fn caller_for(&self, token: &str) -> Option<&Link> {
        let hash: String = Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        let mut found = None;
        for link in self.links.iter() {
            if stored_hash(&link.hash_file).is_some_and(|stored| bool::from(hash.as_bytes().ct_eq(stored.as_bytes()))) && found.is_none() {
                found = Some(link);
            }
        }
        found
    }
}

/// The well-formed hash in `file`, or `None` (missing, unreadable, malformed: accept nothing).
fn stored_hash(file: &std::path::Path) -> Option<String> {
    let stored = std::fs::read_to_string(file).ok()?.trim().to_ascii_lowercase();
    (stored.len() == 64 && stored.bytes().all(|b| b.is_ascii_hexdigit())).then_some(stored)
}

fn bearer(req: &Request) -> Option<String> {
    req.headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, t)| scheme.eq_ignore_ascii_case("bearer") && !t.trim().is_empty())
        .map(|(_, t)| t.trim().to_string())
}

/// The middleware: `axum::middleware::from_fn_with_state(links, links::require)`.
pub async fn require(State(links): State<Links>, mut req: Request, next: Next) -> Result<Response, ApiError> {
    let token =
        bearer(&req).ok_or_else(|| ApiError::new(ErrorCode::Unauthorized, "Send this link's token as `Authorization: Bearer <token>`."))?;
    let l = links.clone();
    let found =
        tokio::task::spawn_blocking(move || l.caller_for(&token).map(|link| (link.caller, link.allow.clone()))).await.ok().flatten();
    let Some((caller, allow)) = found else {
        if let Some(line) = links.note_invalid(Instant::now()) {
            tracing::warn!("{line}");
        }
        return Err(ApiError::new(ErrorCode::Unauthorized, "This service does not accept this token."));
    };
    if !allow.iter().any(|a| a.matches(req.method(), req.uri().path())) {
        return Err(ApiError::new(ErrorCode::Forbidden, "This link may not call this route."));
    }
    req.extensions_mut().insert(Caller(caller));
    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> String {
        Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn patterns_match_one_segment_per_star_and_any_depth_after_double_star() {
        let a = Allow::any("/api/x/v1/providers/*/key");
        assert!(a.matches(&Method::PUT, "/api/x/v1/providers/p1/key"));
        assert!(!a.matches(&Method::PUT, "/api/x/v1/providers/p1/key/more"));
        assert!(!a.matches(&Method::PUT, "/api/x/v1/providers/key"));
        let all = Allow::any("/api/x/v1/**");
        assert!(all.matches(&Method::GET, "/api/x/v1/status") && all.matches(&Method::GET, "/api/x/v1/a/b/c"));
        assert!(!all.matches(&Method::GET, "/api/y/v1/status"));
        assert!(Allow::any("/**").matches(&Method::DELETE, "/anything/at/all") && Allow::any("/**").matches(&Method::GET, "/"));
        let get = Allow::method(Method::GET, "/api/x/v1/status");
        assert!(get.matches(&Method::GET, "/api/x/v1/status") && !get.matches(&Method::PUT, "/api/x/v1/status"));
    }

    #[tokio::test]
    async fn each_caller_gets_its_own_routes_and_a_bad_or_missing_hash_accepts_nothing() {
        let dir = std::env::temp_dir().join(format!("askbar-links-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let (fa, fb, fc) = (dir.join("a.sha256"), dir.join("b.sha256"), dir.join("c.sha256"));
        std::fs::write(&fa, hex("token-a")).unwrap();
        std::fs::write(&fb, format!("{}\n", hex("token-b").to_uppercase())).unwrap();
        std::fs::write(&fc, "not a hash").unwrap();
        let links = Links::new(vec![
            Link { caller: "admin", hash_file: fa.clone(), allow: vec![Allow::any("/v1/**")] },
            Link { caller: "reader", hash_file: fb, allow: vec![Allow::method(Method::GET, "/v1/status")] },
            Link { caller: "broken", hash_file: fc, allow: vec![Allow::any("/v1/**")] },
            Link { caller: "missing", hash_file: dir.join("nope"), allow: vec![Allow::any("/v1/**")] },
        ]);
        async fn who(axum::Extension(c): axum::Extension<Caller>) -> String {
            c.0.to_string()
        }
        let app = axum::Router::new()
            .route("/v1/status", axum::routing::get(who).put(who))
            .route("/v1/config", axum::routing::put(who))
            .layer(axum::middleware::from_fn_with_state(links, require));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let http = reqwest::Client::new();
        let call = |m: reqwest::Method, p: &str, t: Option<&str>| {
            let mut r = http.request(m, format!("{url}{p}"));
            if let Some(t) = t {
                r = r.bearer_auth(t);
            }
            r.send()
        };
        use reqwest::Method as M;
        let r = call(M::PUT, "/v1/config", Some("token-a")).await.unwrap();
        assert_eq!((r.status().as_u16(), r.text().await.unwrap()), (200, "admin".to_string()));
        let r = call(M::GET, "/v1/status", Some("token-b")).await.unwrap();
        assert_eq!(
            (r.status().as_u16(), r.text().await.unwrap()),
            (200, "reader".to_string()),
            "an upper-case hash with a newline is fine"
        );
        assert_eq!(call(M::PUT, "/v1/config", Some("token-b")).await.unwrap().status(), 403, "not this link's route");
        assert_eq!(call(M::PUT, "/v1/status", Some("token-b")).await.unwrap().status(), 403, "nor this method");
        for bad in [None, Some("wrong"), Some("not a hash"), Some(hex("token-a").as_str())] {
            assert_eq!(call(M::GET, "/v1/status", bad).await.unwrap().status(), 401, "{bad:?}");
        }
        // A replaced hash works at once; a removed one accepts nothing.
        std::fs::write(&fa, hex("token-a2")).unwrap();
        assert_eq!(call(M::GET, "/v1/status", Some("token-a")).await.unwrap().status(), 401);
        assert_eq!(call(M::GET, "/v1/status", Some("token-a2")).await.unwrap().status(), 200);
        std::fs::remove_file(&fa).unwrap();
        assert_eq!(call(M::GET, "/v1/status", Some("token-a2")).await.unwrap().status(), 401);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn five_invalid_tokens_in_ten_minutes_report_once_and_then_stay_quiet_until_it_calms_down() {
        let mut inv = InvalidTokens::new(Duration::from_secs(600), 5);
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let got: Vec<Option<usize>> = (0..4).map(|i| inv.record(at(i))).collect();
        assert_eq!(got, [None, None, None, None], "below the threshold nothing is reported");
        assert_eq!(inv.record(at(4)), Some(5), "the fifth in the window is reported with its count");
        for s in 5..60 {
            assert_eq!(inv.record(at(s)), None, "an episode is one report, not one per token");
        }
        // Ten quiet minutes later the window is empty; a new burst is a new episode.
        assert_eq!(inv.count(at(60 + 700)), 0);
        let again: Vec<Option<usize>> = (0..5).map(|i| inv.record(at(800 + i))).collect();
        assert_eq!(again, [None, None, None, None, Some(5)]);
    }

    #[test]
    fn tokens_spread_over_more_than_the_window_never_reach_the_threshold() {
        let mut inv = InvalidTokens::new(Duration::from_secs(600), 5);
        let t0 = Instant::now();
        for i in 0..40 {
            assert_eq!(inv.record(t0 + Duration::from_secs(i * 200)), None, "one every 200 s is three in a window");
        }
    }

    #[test]
    fn a_flood_cannot_grow_what_is_remembered() {
        let mut inv = InvalidTokens::new(Duration::from_secs(600), 5);
        let t0 = Instant::now();
        for _ in 0..5000 {
            inv.record(t0);
        }
        assert_eq!(inv.count(t0), InvalidTokens::KEEP);
    }

    #[test]
    fn the_line_names_the_links_and_the_count() {
        let links = Links::new(vec![Link { caller: "admin", hash_file: PathBuf::from("/nowhere"), allow: vec![] }]).named("server");
        let now = Instant::now();
        let lines: Vec<String> = (0..9).filter_map(|i| links.note_invalid(now + Duration::from_secs(i))).collect();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("5 invalid tokens in 10 minutes on the link(s) admin-server"), "{}", lines[0]);
    }

    #[tokio::test]
    async fn only_a_presented_token_that_matches_no_link_is_counted() {
        let dir = std::env::temp_dir().join(format!("askbar-links-inv-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let fa = dir.join("a.sha256");
        std::fs::write(&fa, hex("token-a")).unwrap();
        let links = Links::new(vec![Link { caller: "admin", hash_file: fa, allow: vec![Allow::method(Method::GET, "/v1/status")] }])
            .named("server");
        let app = axum::Router::new()
            .route("/v1/status", axum::routing::get(|| async { "ok" }))
            .route("/v1/other", axum::routing::get(|| async { "no" }))
            .layer(axum::middleware::from_fn_with_state(links.clone(), require));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let http = reqwest::Client::new();
        let get = |p: &str, t: Option<&str>| {
            let mut r = http.get(format!("{url}{p}"));
            if let Some(t) = t {
                r = r.bearer_auth(t);
            }
            async move { r.send().await.unwrap().status().as_u16() }
        };
        assert_eq!(get("/v1/status", Some("token-a")).await, 200);
        assert_eq!(get("/v1/other", Some("token-a")).await, 403, "a valid token on the wrong route");
        assert_eq!(get("/v1/status", None).await, 401, "no token at all");
        assert_eq!(links.invalid_count(), 0, "none of those is an invalid token");
        for i in 0..3 {
            assert_eq!(get("/v1/status", Some(&format!("wrong-{i}"))).await, 401);
        }
        assert_eq!(links.invalid_count(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
