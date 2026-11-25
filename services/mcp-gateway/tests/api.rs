//! The front's API: the config push (a bad one keeps the old, the version follows the content), a status with no
//! secret, and a tool call refused before the first push.

use askbar_mcp_gateway::{links, router, Gateway};
use askbar_service::links::Links;
use askbar_service::status::BootId;
use reqwest::Method as M;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

const TOKEN: &str = "admmcp_secret";
const SERVER_TOKEN: &str = "srvmcp_secret";

async fn start(links: impl FnOnce(&std::path::Path) -> Links) -> (String, Arc<Gateway>) {
    let dir = std::env::temp_dir().join(format!("askbar-mcp-gateway-{}", BootId::new().as_str()));
    std::fs::create_dir_all(&dir).unwrap();
    let g = Gateway::new(BootId::new(), Some("searxng-marker-host:8080".into()));
    let app = router(g.clone(), links(&dir));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api/mcp-gateway/v1", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (url, g)
}

/// The real links over hash files in `dir/accept`, as `bootstrap` writes them: the admin's, and the server's when `server` is set.
fn test_links(dir: &std::path::Path) -> Links {
    write_hash(dir, "admin-mcp-gateway", TOKEN);
    write_hash(dir, "server-mcp-gateway", SERVER_TOKEN);
    links(dir)
}

fn write_hash(dir: &std::path::Path, name: &str, token: &str) {
    let hash: String = Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    std::fs::create_dir_all(dir.join("accept")).unwrap();
    std::fs::write(dir.join("accept").join(format!("{name}.sha256")), format!("{hash}\n")).unwrap();
}

async fn call(url: &str, m: M, path: &str, token: Option<&str>, body: Option<Value>) -> (u16, Value) {
    let mut r = reqwest::Client::new().request(m, format!("{url}{path}"));
    if let Some(t) = token {
        r = r.bearer_auth(t);
    }
    if let Some(b) = body {
        r = r.json(&b);
    }
    let r = r.send().await.unwrap();
    (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
}

fn cfg(search_results: u32) -> Value {
    json!({"fetch_max_bytes": 1048576, "fetch_timeout_secs": 20, "fetch_redirects": 3, "tool_call_timeout_secs": 30,
           "one_time_id_window_secs": 120, "search_results": search_results, "remembered_urls": 50})
}

#[tokio::test]
async fn a_bad_push_leaves_the_old_config_live_and_the_version_follows_the_content() {
    let (url, g) = start(test_links).await;
    let put = |b: Value| call(&url, M::PUT, "/config", Some(TOKEN), Some(b));
    let (s, v1) = put(cfg(8)).await;
    assert_eq!(s, 200);
    assert_eq!(put(cfg(8)).await.1, v1, "equal content, equal version");
    let (s, v2) = put(cfg(9)).await;
    assert_eq!(s, 200);
    assert_ne!(v1, v2, "other content, other version");
    for bad in [cfg(21), json!({"search_results": 5}), json!({"extra": 1})] {
        let (s, p) = put(bad).await;
        assert_eq!((s, p["error"].as_str()), (400, Some("bad_request")));
    }
    assert_eq!(g.config().search_results, 9);
    let (_, st) = call(&url, M::GET, "/status", Some(TOKEN), None).await;
    assert_eq!(st["config_version"], v2["version"]);
}

#[tokio::test]
async fn status_holds_counts_and_no_secret_and_a_call_before_the_first_push_is_refused() {
    let (url, g) = start(test_links).await;
    assert_eq!(g.config().search_results, 8, "the askbar-proto defaults apply before a push");
    assert!(g.admit().is_err(), "a call before the first push is refused");
    let (s, st) = call(&url, M::GET, "/status", Some(TOKEN), None).await;
    assert_eq!(s, 200, "status answers before the first push");
    assert_eq!(st["config_version"], Value::Null);
    assert_eq!(st["calls"], json!({"refused_not_configured": 1}));
    let text = st.to_string();
    for secret in [TOKEN, "searxng-marker-host", "http"] {
        assert!(!text.contains(secret), "status leaked {secret}");
    }
    call(&url, M::PUT, "/config", Some(TOKEN), Some(cfg(8))).await;
    assert!(g.admit().is_ok(), "a call is admitted after the first push");
}

#[tokio::test]
async fn health_needs_no_token_and_a_missing_or_wrong_token_is_401() {
    let (url, _) = start(test_links).await;
    assert_eq!(reqwest::get(format!("{url}/health")).await.unwrap().status(), 200);
    for token in [None, Some("admmcp_wrong")] {
        assert_eq!(call(&url, M::GET, "/status", token, None).await.0, 401);
        assert_eq!(call(&url, M::PUT, "/config", token, Some(cfg(8))).await.0, 401);
    }
    assert_eq!(call(&url, M::GET, "/status", Some(TOKEN), None).await.0, 200);
}

#[tokio::test]
async fn the_server_link_cannot_read_status_or_push_config() {
    let (url, g) = start(test_links).await;
    assert_eq!(call(&url, M::PUT, "/config", Some(SERVER_TOKEN), Some(cfg(9))).await.0, 403);
    assert_eq!(call(&url, M::GET, "/status", Some(SERVER_TOKEN), None).await.0, 403);
    assert!(!g.is_configured(), "the refused push changed nothing");
}

#[tokio::test]
async fn without_hash_files_nothing_but_health_is_reachable() {
    let (url, _) = start(links).await;
    assert_eq!(call(&url, M::GET, "/status", Some(TOKEN), None).await.0, 401);
    assert_eq!(call(&url, M::PUT, "/config", Some(TOKEN), Some(cfg(8))).await.0, 401);
}
