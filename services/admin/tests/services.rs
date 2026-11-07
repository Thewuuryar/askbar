//! Starting, stopping and restarting services from the admin: session only, recorded, rate limited, and
//! refused for anything but the three services.

mod common;

use askbar_admin::control_link::ControlLink;
use axum::{extract::{Path, State}, http::HeaderMap, routing::{get, post}, Json, Router};
use common::*;
use reqwest::Method as M;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const CTL_TOKEN: &str = "ctl_test-token";

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<String>>,
    tokens: Mutex<Vec<String>>,
}

async fn list(State(f): State<Arc<Fake>>, h: HeaderMap) -> Json<Value> {
    f.tokens.lock().unwrap().push(h["authorization"].to_str().unwrap().to_string());
    Json(json!({ "services": [
        { "name": "server", "state": "running", "status": "Up 1 hour (healthy)", "health": "healthy" },
        { "name": "caddy", "state": "running", "status": "Up 1 hour", "health": null },
        { "name": "cpa", "state": "exited", "status": "Exited (0)", "health": null } ] }))
}

async fn act(State(f): State<Arc<Fake>>, Path((n, a)): Path<(String, String)>) -> Json<Value> {
    f.calls.lock().unwrap().push(format!("{a} {n}"));
    Json(json!({ "name": n, "state": "running", "status": "Up", "health": null }))
}

async fn start_with_control() -> (T, Arc<Fake>) {
    let t = start(true).await;
    let fake = Arc::new(Fake::default());
    let app = Router::new()
        .route("/api/control/v1/services", get(list))
        .route("/api/control/v1/services/{name}/{action}", post(act))
        .with_state(fake.clone());
    t.admin.set_control(ControlLink::new(&serve(app).await, CTL_TOKEN.into()));
    (t, fake)
}

#[tokio::test]
async fn services_are_listed_and_actions_reach_the_controller_with_its_token() {
    let (t, fake) = start_with_control().await;
    let (s, v) = t.call(M::GET, "/api/admin/v1/services", json!(null)).await;
    assert_eq!(s, 200);
    assert_eq!(v["services"][2]["state"], "exited");
    assert_eq!(v["recent"], json!([]));
    assert_eq!(fake.tokens.lock().unwrap()[0], format!("Bearer {CTL_TOKEN}"));

    let (s, v) = t.call(M::POST, "/api/admin/v1/services/cpa/start", json!(null)).await;
    assert_eq!((s, v["name"].as_str()), (200, Some("cpa")));
    assert_eq!(*fake.calls.lock().unwrap(), vec!["start cpa".to_string()]);

    // The attempt is on record.
    let (_, v) = t.call(M::GET, "/api/admin/v1/services", json!(null)).await;
    assert_eq!((v["recent"][0]["service"].as_str(), v["recent"][0]["action"].as_str(), v["recent"][0]["ok"].clone()), (Some("cpa"), Some("start"), json!(true)));
}

#[tokio::test]
async fn only_the_three_services_and_three_actions_can_be_named() {
    let (t, fake) = start_with_control().await;
    for name in ["admin", "controller", "caddy%2F..", "mock-llm"] {
        assert_eq!(t.call(M::POST, &format!("/api/admin/v1/services/{name}/stop"), json!(null)).await.0, 404, "{name}");
    }
    for action in ["kill", "exec", "rm"] {
        assert_eq!(t.call(M::POST, &format!("/api/admin/v1/services/server/{action}"), json!(null)).await.0, 404, "{action}");
    }
    assert!(fake.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_admin_api_key_cannot_control_services() {
    let (t, fake) = start_with_control().await;
    let (_, k) = t.call(M::POST, "/api/admin/v1/keys", json!({ "name": "script" })).await;
    let key = k["key"].as_str().unwrap();
    for (m, path) in [(M::GET, "/api/admin/v1/services"), (M::POST, "/api/admin/v1/services/server/restart")] {
        let r = reqwest::Client::new().request(m, format!("{}{path}", t.adm)).bearer_auth(key).header("X-Askbar-Admin", "1").send().await.unwrap();
        assert_eq!(r.status(), 401, "{path}");
    }
    // And no session at all.
    let r = reqwest::Client::new().post(format!("{}/api/admin/v1/services/server/restart", t.adm)).header("X-Askbar-Admin", "1").send().await.unwrap();
    assert_eq!(r.status(), 401);
    assert!(fake.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn at_most_six_actions_a_minute() {
    let (t, fake) = start_with_control().await;
    for _ in 0..6 {
        assert_eq!(t.call(M::POST, "/api/admin/v1/services/caddy/restart", json!(null)).await.0, 200);
    }
    let (s, v) = t.call(M::POST, "/api/admin/v1/services/caddy/restart", json!(null)).await;
    assert_eq!((s, v["error"].as_str()), (429, Some("too_many_attempts")));
    assert_eq!(fake.calls.lock().unwrap().len(), 6);
}

#[tokio::test]
async fn without_a_controller_the_answer_is_a_clear_502() {
    let t = start(true).await;
    let (s, v) = t.call(M::GET, "/api/admin/v1/services", json!(null)).await;
    assert_eq!((s, v["error"].as_str()), (502, Some("server_unavailable")));
    assert_eq!(t.call(M::POST, "/api/admin/v1/services/cpa/start", json!(null)).await.0, 502);
}
