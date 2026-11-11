//! Connecting Claude and ChatGPT accounts to the bundled CLIProxyAPI from the admin, and the provider it makes.
//! `mock_cpa` states what is assumed about CPA's control API; check it against the pinned image.

mod common;

use askbar_admin::cpa::{CpaLink, Keys};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    routing::{delete, get, post},
    Json, Router,
};
use common::*;
use reqwest::Method as M;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const API_KEY: &str = "sk-cpa-TESTKEY";
const SECRET: &str = "control-secret-xyz";

#[derive(Default)]
struct Cpa {
    accounts: Mutex<Vec<Value>>,
    callbacks: Mutex<Vec<Value>>,
    pending: Mutex<Option<String>>,
    removed: Mutex<Vec<String>>,
}

fn authed(h: &HeaderMap) -> bool {
    h.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {SECRET}"))
}

async fn mock_cpa() -> (String, Arc<Cpa>) {
    let cpa = Arc::new(Cpa::default());
    async fn url(State(c): State<Arc<Cpa>>, h: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
        assert!(authed(&h) && q.get("is_webui").map(String::as_str) == Some("true"));
        *c.pending.lock().unwrap() = Some("STATE-1".into());
        Json(
            json!({ "status": "ok", "url": "https://claude.example/authorize?x=1&redirect_uri=http%3A%2F%2Flocalhost%3A54545%2Fcallback", "state": "STATE-1" }),
        )
    }
    async fn status(State(c): State<Arc<Cpa>>, h: HeaderMap) -> Json<Value> {
        assert!(authed(&h));
        if c.callbacks.lock().unwrap().is_empty() {
            Json(json!({ "status": "wait" }))
        } else {
            Json(json!({ "status": "ok" }))
        }
    }
    async fn callback(State(c): State<Arc<Cpa>>, h: HeaderMap, Json(b): Json<Value>) -> Json<Value> {
        assert!(authed(&h));
        c.callbacks.lock().unwrap().push(b);
        c.accounts
            .lock()
            .unwrap()
            .push(json!({ "name": "claude-me@example.com.json", "provider": "anthropic", "email": "me@example.com" }));
        Json(json!({ "status": "ok" }))
    }
    async fn files(State(c): State<Arc<Cpa>>, h: HeaderMap) -> Json<Value> {
        assert!(authed(&h));
        Json(json!({ "files": *c.accounts.lock().unwrap() }))
    }
    async fn rm(State(c): State<Arc<Cpa>>, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
        c.removed.lock().unwrap().push(q["name"].clone());
        c.accounts.lock().unwrap().clear();
        Json(json!({ "status": "ok" }))
    }
    async fn models(h: HeaderMap) -> Result<Json<Value>, axum::http::StatusCode> {
        if h.get("authorization").and_then(|v| v.to_str().ok()) != Some(&format!("Bearer {API_KEY}")) {
            return Err(axum::http::StatusCode::UNAUTHORIZED);
        }
        Ok(Json(json!({ "data": [{ "id": "gpt-5" }, { "id": "claude-sonnet-x" }] })))
    }
    let app = Router::new()
        .route("/v0/management/anthropic-auth-url", get(url)) // names-ok: CLIProxyAPI's own path
        .route("/v0/management/codex-auth-url", get(url)) // names-ok: CLIProxyAPI's own path
        .route("/v0/management/get-auth-status", get(status)) // names-ok: CLIProxyAPI's own path
        .route("/v0/management/oauth-callback", post(callback)) // names-ok: CLIProxyAPI's own path
        .route("/v0/management/auth-files", get(files)) // names-ok: CLIProxyAPI's own path
        .route("/v0/management/auth-files", delete(rm)) // names-ok: CLIProxyAPI's own path
        .route("/v1/models", get(models))
        .with_state(cpa.clone());
    (serve(app).await, cpa)
}

async fn with_cpa() -> (T, Arc<Cpa>) {
    let t = start(true).await;
    let (url, cpa) = mock_cpa().await;
    t.admin.set_cpa(CpaLink::new(&url, Keys { api_key: API_KEY.into(), control_secret: SECRET.into() }));
    (t, cpa)
}

const GOOD: &str = "http://localhost:54545/callback?code=THECODE&state=STATE-1";

#[tokio::test]
async fn connecting_an_account_from_the_page_makes_the_cpa_provider_with_the_key_the_admin_made() {
    let (t, cpa) = with_cpa().await;
    let (s, v) = t.call(M::GET, "/api/admin/v1/cpa", json!(null)).await;
    assert_eq!(
        (s, v["configured"].clone(), v["reachable"].clone(), v["accounts"].clone(), v["provider_id"].clone()),
        (200, json!(true), json!(true), json!([]), json!(null))
    );
    assert!(
        t.call(M::GET, "/api/admin/v1/providers", json!(null)).await.1.as_array().unwrap().is_empty(),
        "no provider before an account is connected"
    );

    let (s, login) = t.call(M::POST, "/api/admin/v1/cpa/logins", json!({ "provider": "anthropic" })).await;
    assert_eq!((s, login["state"].as_str()), (200, Some("STATE-1")));
    assert!(login["url"].as_str().unwrap().starts_with("https://claude.example/"));

    // Waiting until the address is pasted.
    assert_eq!(t.call(M::GET, "/api/admin/v1/cpa/logins/STATE-1", json!(null)).await.1["status"], "wait");

    let (s, _) = t.call(M::POST, "/api/admin/v1/cpa/logins/STATE-1/callback", json!({ "redirect_url": GOOD })).await;
    assert_eq!(s, 200);
    // The pasted address holds the one-time code: it is on no record the admin keeps (ADR 0010 rule on content).
    let actions: String = t.rig.rows("SELECT detail_json FROM admin_actions").iter().map(|r| r[0].to_string()).collect();
    assert!(actions.contains("cpa_login_finish") && !actions.contains("THECODE"), "the code is in no admin record: {actions}");
    let sent = cpa.callbacks.lock().unwrap()[0].clone();
    assert_eq!(
        (sent["provider"].as_str(), sent["code"].as_str(), sent["state"].as_str()),
        (Some("anthropic"), Some("THECODE"), Some("STATE-1"))
    );

    let (_, v) = t.call(M::GET, "/api/admin/v1/cpa/logins/STATE-1", json!(null)).await;
    assert_eq!(v["status"], "ok");
    let id = v["provider_id"].as_str().expect("the provider is made once an account is connected").to_string();

    let (_, list) = t.call(M::GET, "/api/admin/v1/providers", json!(null)).await;
    let p = &list[0];
    assert_eq!(
        (p["name"].as_str(), p["type"].as_str(), p["model"].as_str(), p["billing"].as_str(), p["managed"].clone(), p["has_key"].clone()),
        (Some("CPA"), Some("openai"), Some("claude-sonnet-x"), Some("subscription"), json!(true), json!(true))
    );
    assert!(p["base_url"].as_str().unwrap().ends_with("/v1"));
    // The key is the admin's own file, handed to the gateway with its configuration (the server holds none, ADR 0054);
    // the list never shows it.
    assert_eq!(gateway_key(&t, &id).await.as_deref(), Some(API_KEY));
    assert!(!list.to_string().contains(API_KEY));
    // Asking again does not make a second one.
    let (_, v) = t.call(M::GET, "/api/admin/v1/cpa", json!(null)).await;
    assert_eq!((v["provider_id"].as_str(), v["accounts"][0]["label"].as_str()), (Some(id.as_str()), Some("me@example.com")));
    assert_eq!(t.call(M::GET, "/api/admin/v1/providers", json!(null)).await.1.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_bad_address_is_explained_and_costs_nothing_but_a_good_one_works_once() {
    let (t, cpa) = with_cpa().await;
    t.call(M::POST, "/api/admin/v1/cpa/logins", json!({ "provider": "anthropic" })).await;
    let path = "/api/admin/v1/cpa/logins/STATE-1/callback";
    for bad in [
        "nonsense",
        "https://evil.example/cb?code=c&state=STATE-1",
        "http://localhost:1/cb?code=c&state=OTHER",
        "http://localhost:1/cb?state=STATE-1",
        // A local address with this login's state, but not the one the login sends the browser back to.
        "http://localhost:1/cb?code=c&state=STATE-1",
        "http://localhost:54545/elsewhere?code=c&state=STATE-1",
    ] {
        let (s, v) = t.call(M::POST, path, json!({ "redirect_url": bad })).await;
        assert_eq!((s, v["error"].as_str()), (400, Some("bad_request")), "{bad}");
    }
    assert!(cpa.callbacks.lock().unwrap().is_empty(), "nothing reached CPA");
    assert_eq!(t.call(M::POST, path, json!({ "redirect_url": GOOD })).await.0, 200, "the login survived the typos");
    assert_eq!(t.call(M::POST, path, json!({ "redirect_url": GOOD })).await.0, 404, "an address cannot be replayed");
    assert_eq!(t.call(M::POST, "/api/admin/v1/cpa/logins/NEVER-STARTED/callback", json!({ "redirect_url": GOOD })).await.0, 404);
    assert_eq!(t.call(M::POST, "/api/admin/v1/cpa/logins", json!({ "provider": "gemini" })).await.0, 400);
}

/// What the gateway holds for the provider after the admin has given it its configuration (which needs the audit key).
async fn gateway_key(t: &T, id: &str) -> Option<String> {
    let pass = "a long enough passphrase for tests";
    t.call(M::PUT, "/api/admin/v1/setup/audit-key", json!({ "passphrase": pass, "confirm": pass })).await;
    t.admin.gateway.get().unwrap().sync(&t.admin).await;
    t.gateway.provider(id).map(|p| p.api_key)
}

#[tokio::test]
async fn the_cpa_provider_cannot_be_renamed_repointed_re_keyed_or_deleted_but_its_model_can_change() {
    let (t, _cpa) = with_cpa().await;
    t.call(M::POST, "/api/admin/v1/cpa/logins", json!({ "provider": "anthropic" })).await;
    t.call(M::POST, "/api/admin/v1/cpa/logins/STATE-1/callback", json!({ "redirect_url": GOOD })).await;
    let id = t.call(M::GET, "/api/admin/v1/cpa/logins/STATE-1", json!(null)).await.1["provider_id"].as_str().unwrap().to_string();
    let attempt = json!({ "name": "other", "type": "anthropic", "base_url": "http://evil.example/v1", "model": "gpt-5", "api_key": "stolen-key-123", "billing": "subscription" });
    assert_eq!(t.call(M::PUT, &format!("/api/admin/v1/providers/{id}"), attempt).await.0, 204);
    let (_, list) = t.call(M::GET, "/api/admin/v1/providers", json!(null)).await;
    let p = &list[0];
    assert_eq!((p["name"].as_str(), p["type"].as_str(), p["model"].as_str()), (Some("CPA"), Some("openai"), Some("gpt-5")));
    assert!(p["base_url"].as_str().unwrap().contains("127.0.0.1"), "still pointing at CPA");
    assert_eq!(gateway_key(&t, &id).await.as_deref(), Some(API_KEY), "the key was not replaced");
    assert_eq!(t.call(M::DELETE, &format!("/api/admin/v1/providers/{id}"), json!(null)).await.0, 409);
}

#[tokio::test]
async fn disconnecting_removes_the_account_and_only_a_known_one() {
    let (t, cpa) = with_cpa().await;
    t.call(M::POST, "/api/admin/v1/cpa/logins", json!({ "provider": "openai" })).await;
    t.call(M::POST, "/api/admin/v1/cpa/logins/STATE-1/callback", json!({ "redirect_url": GOOD })).await;
    assert_eq!(t.call(M::DELETE, "/api/admin/v1/cpa/accounts/nope.json", json!(null)).await.0, 404);
    assert_eq!(t.call(M::DELETE, "/api/admin/v1/cpa/accounts/..%2Fescape", json!(null)).await.0, 404);
    assert!(cpa.removed.lock().unwrap().is_empty());
    assert_eq!(t.call(M::DELETE, "/api/admin/v1/cpa/accounts/claude-me@example.com.json", json!(null)).await.0, 204);
    assert_eq!(*cpa.removed.lock().unwrap(), vec!["claude-me@example.com.json".to_string()]);
}

#[tokio::test]
async fn the_routes_are_session_only_and_say_so_when_cpa_is_not_set_up() {
    let t = start(true).await;
    let (_, v) = t.call(M::GET, "/api/admin/v1/cpa", json!(null)).await;
    assert_eq!((v["configured"].clone(), v["reachable"].clone()), (json!(false), json!(false)));
    assert_eq!(t.call(M::POST, "/api/admin/v1/cpa/logins", json!({ "provider": "anthropic" })).await.0, 502);
    let (_, k) = t.call(M::POST, "/api/admin/v1/keys", json!({ "name": "script" })).await;
    let r = reqwest::Client::new().get(format!("{}/api/admin/v1/cpa", t.adm)).bearer_auth(k["key"].as_str().unwrap()).send().await.unwrap();
    assert_eq!(r.status(), 401, "an admin API key cannot connect or read accounts");
}
