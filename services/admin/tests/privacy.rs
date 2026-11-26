#![cfg(unix)]
//! The PII settings (plan PII-P3.1): defaults, round trip, refusals that echo no term, the audit record (counts only), and the
//! gateway config and the front's config carrying `pii`, with the admin's front version equal to the front's own.

mod common;
use askbar_admin::mcp_link::{McpLink, Sync};
use axum::{
    extract::State,
    routing::{get, put},
    Json, Router,
};
use reqwest::Method as M;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const URL: &str = "/api/admin/v1/privacy";
const MARKER: &str = "zq-marker-term-7731";

#[derive(Default)]
struct Front(Mutex<Vec<Value>>);

async fn status(State(f): State<Arc<Front>>) -> Json<Value> {
    let v = f.0.lock().unwrap().last().map(|c| serde_json::from_value::<askbar_mcp_gateway::config::Config>(c.clone()).unwrap().version());
    Json(json!({ "config_version": v, "boot_id": "b", "calls": {} }))
}

async fn config(State(f): State<Arc<Front>>, Json(b): Json<Value>) -> Json<Value> {
    f.0.lock().unwrap().push(b);
    Json(json!({}))
}

async fn gateway(t: &common::T) -> askbar_proto::gateway::GatewayConfig {
    let db = t.admin.db.lock().await;
    askbar_admin::gateway_link::build_config(db.conn(), &t.admin.secrets, None).unwrap()
}

#[tokio::test]
async fn privacy_defaults_round_trip_and_reach_the_gateway_config_and_the_front() {
    let t = common::start(true).await;
    let front = Arc::new(Front::default());
    let url = common::serve(
        Router::new()
            .route("/api/mcp-gateway/v1/status", get(status))
            .route("/api/mcp-gateway/v1/config", put(config))
            .with_state(front.clone()),
    )
    .await;
    let link = McpLink::new(&url, "admmcp_test_token".into());
    t.admin.mcp.set(link.clone()).ok().unwrap();

    // Defaults, with each type's default action; CUSTOM is not listed.
    let (st, v) = t.call(M::GET, URL, json!(null)).await;
    assert_eq!((st, v["enabled"].clone(), v["mode"].clone()), (200, json!(false), json!("detect_only")));
    assert_eq!(v["types"].as_array().unwrap().len(), 10);
    assert!(v["types"].as_array().unwrap().contains(&json!({ "name": "CARD", "default": "block" })));
    assert!(v["types"].as_array().unwrap().contains(&json!({ "name": "EMAIL", "default": "pseudonymize" })));
    assert!(!v["types"].to_string().contains("CUSTOM"));

    // An unchanged default keeps today's version and leaves `pii` out of both pushes.
    let g0 = gateway(&t).await;
    assert!(serde_json::to_value(&g0).unwrap().get("pii").is_none());
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    let first = front.0.lock().unwrap().last().unwrap().clone();
    assert!(first.get("pii").is_none());

    // A save round-trips, and both configs carry it under a new version.
    let body =
        json!({ "enabled": true, "mode": "enforce", "entities": { "EMAIL": "allow" }, "allow_terms": ["a@b.c"], "deny_terms": [MARKER] });
    let (st, saved) = t.call(M::PUT, URL, body).await;
    assert_eq!(st, 200, "{saved}");
    assert_eq!(saved["deny_terms"], json!([MARKER]));
    assert_eq!(t.call(M::GET, URL, json!(null)).await.1["entities"], json!({ "EMAIL": "allow" }));
    let g1 = gateway(&t).await;
    assert!(g1.pii.enabled && g1.pii.deny_terms == [MARKER] && g1.pii.allow_terms == ["a@b.c"]);
    assert_ne!(g1.version, g0.version);
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    let pushed = front.0.lock().unwrap().last().unwrap().clone();
    assert_eq!(pushed["pii"]["mode"], "enforce");
    let mine = askbar_admin::mcp_link::build_config(t.admin.db.lock().await.conn());
    let theirs: askbar_mcp_gateway::config::Config = serde_json::from_value(pushed).unwrap();
    assert_eq!(mine.version(), theirs.version(), "the admin hashes what the front hashes");
    assert_eq!(theirs.pii.deny_terms, [MARKER]);

    // The record names counts and field names only; no term is in the records or the status.
    let rows = t.rig.rows("SELECT detail_json FROM admin_actions WHERE category = 'privacy' AND action = 'set' AND outcome = 'done'");
    let d = rows[0][0].as_str().unwrap();
    for want in ["\"enabled\":true", "\"mode\":\"enforce\"", "\"entity_overrides\":1", "\"allow_terms\":1", "\"deny_terms\":1"] {
        assert!(d.contains(want), "{d}");
    }
    let all = t.rig.rows("SELECT detail_json FROM admin_actions");
    assert!(!format!("{all:?}").contains(MARKER) && !format!("{all:?}").contains("a@b.c"));
    assert!(!t.call(M::GET, "/api/admin/v1/status", json!(null)).await.1.to_string().contains(MARKER));

    // Back to the default: the key leaves both pushes and the versions return.
    assert_eq!(t.call(M::PUT, URL, json!({})).await.0, 200);
    let g2 = gateway(&t).await;
    assert_eq!(g2.version, g0.version);
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    assert!(front.0.lock().unwrap().last().unwrap().get("pii").is_none());
}

#[tokio::test]
async fn a_bad_save_is_refused_without_echoing_a_term_and_nothing_is_saved() {
    let t = common::start(true).await;
    let many: Vec<String> = (0..501).map(|i| format!("{MARKER}{i}")).collect();
    for bad in [
        json!({ "entities": { "NOPE": "allow" } }),
        json!({ "entities": { "CUSTOM": "allow" } }),
        json!({ "deny_terms": [MARKER, "  "] }),
        json!({ "allow_terms": [format!("{MARKER}\n")] }),
        json!({ "deny_terms": [MARKER.repeat(10)] }),
        json!({ "allow_terms": many }),
    ] {
        let (st, b) = t.call(M::PUT, URL, bad.clone()).await;
        assert_eq!((st, b["error"].as_str()), (400, Some("bad_request")), "{b}");
        assert!(!b.to_string().contains(MARKER), "{b}");
    }
    assert_eq!(t.call(M::GET, URL, json!(null)).await.1["deny_terms"], json!([]));
    let all = t.rig.rows("SELECT detail_json FROM admin_actions");
    assert!(!format!("{all:?}").contains(MARKER));
}
