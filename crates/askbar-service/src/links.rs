//! The per-link bearer check (ADR 0004, 0028). Each link is one caller of this service: the caller
//! holds a token, this service holds only its SHA-256 (hex) in a file, and the caller may call only
//! the routes listed for its link.
//!
//! - The hash file is read on every request, so a replaced hash works at once; a missing,
//!   unreadable or malformed file accepts nothing (fail closed).
//! - No token, or one that matches no link: `401 unauthorized`.
//! - A token that matches a link, on a route that link may not call: `403 forbidden`.
//! - On success the caller's name is put in the request's extensions as [`Caller`].

use askbar_proto::http::ApiError;
use askbar_proto::problem::ErrorCode;
use axum::{
    extract::{Request, State},
    http::{header, Method},
    middleware::Next,
    response::Response,
};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;
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

/// Every link of one listener.
#[derive(Debug, Clone)]
pub struct Links(Arc<Vec<Link>>);

impl Links {
    pub fn new(links: Vec<Link>) -> Links {
        Links(Arc::new(links))
    }

    /// The link whose hash the token matches, if any. Every link is checked, in constant time per
    /// link, whatever matched before.
    fn caller_for(&self, token: &str) -> Option<&Link> {
        let hash: String = Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        let mut found = None;
        for link in self.0.iter() {
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
    let token = bearer(&req).ok_or_else(|| ApiError::new(ErrorCode::Unauthorized, "Send this link's token as `Authorization: Bearer <token>`."))?;
    let l = links.clone();
    let found = tokio::task::spawn_blocking(move || l.caller_for(&token).map(|link| (link.caller, link.allow.clone()))).await.ok().flatten();
    let Some((caller, allow)) = found else {
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
        assert_eq!((r.status().as_u16(), r.text().await.unwrap()), (200, "reader".to_string()), "an upper-case hash with a newline is fine");
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
}
