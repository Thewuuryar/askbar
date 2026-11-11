//! `POST /models` and `POST /providers/{id}/test` on the admin's listener (moved from the server, ADR 0054): the
//! gateway asks a provider for its models and tries it with one short message, with the key only it holds.

use askbar_model_gateway::{admin_links, admin_router, Gateway};
use askbar_proto::gateway::{GatewayConfig, GatewayProvider};
use askbar_proto::AuditKey;
use askbar_service::audit_client::AuditClient;
use askbar_service::restart::Restarter;
use askbar_service::status::BootId;
use axum::{
    extract::Query,
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::Duration;

const ADM: &str = "tok_admgw_models";
const KEY: &str = "good";

/// A fake provider: `/v1/models` needs the bearer `good`; `/anthropic/v1/models` needs `x-api-key: good` and pages;
/// `/chat/completions` streams `ok` for the right key and 401s, echoing the key, for any other.
async fn provider() -> String {
    async fn openai(h: HeaderMap) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
        if h.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer good") {
            return Err((axum::http::StatusCode::UNAUTHORIZED, Json(json!({"error": {"message": "bad key good-not"}}))));
        }
        Ok(Json(json!({"data": [{"id": "m-b"}, {"id": "m-a"}]})))
    }
    async fn anthropic(h: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
        assert_eq!(h.get("x-api-key").and_then(|v| v.to_str().ok()), Some("good"));
        assert!(h.get("anthropic-version").is_some());
        if q.get("after_id").map(String::as_str) == Some("c-1") {
            Json(json!({"data": [{"id": "c-2", "display_name": "C Two"}], "has_more": false}))
        } else {
            Json(json!({"data": [{"id": "c-1"}], "has_more": true, "last_id": "c-1"}))
        }
    }
    async fn chat(h: HeaderMap) -> Result<String, (axum::http::StatusCode, String)> {
        let key = h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").trim_start_matches("Bearer ").to_string();
        if key != "good" {
            return Err((axum::http::StatusCode::UNAUTHORIZED, json!({"error": {"message": format!("invalid key {key}")}}).to_string()));
        }
        Ok("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n".to_string())
    }
    let app = Router::new()
        .route("/v1/models", get(openai))
        .route("/v1/chat/completions", post(chat))
        .route("/anthropic/v1/models", get(anthropic))
        .route("/empty/v1/models", get(|| async { Json(json!({"data": []})) }));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    url
}

struct G {
    url: String,
    http: reqwest::Client,
    dir: std::path::PathBuf,
}

impl Drop for G {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn audit_key() -> AuditKey {
    let public = *crypto_box::SecretKey::from([3u8; 32]).public_key().as_bytes();
    AuditKey { key_id: askbar_model_gateway::seal::key_id(&public), public_key: public.iter().map(|b| format!("{b:02x}")).collect() }
}

/// A gateway with the providers `up` serves, saved under the id `saved`.
async fn gateway(up: &str) -> G {
    let dir = std::env::temp_dir().join(format!("gw-models-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(dir.join("accept")).unwrap();
    let hex: String = Sha256::digest(ADM.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(dir.join("accept/admin-model-gateway.sha256"), hex).unwrap();
    let boot = BootId::new();
    let audit = AuditClient::new("http://127.0.0.1:1", dir.clone(), "model-gateway-auditor");
    let gw = Gateway::new(audit, boot.clone(), Restarter::for_tests(boot, Duration::ZERO, Duration::from_secs(1), || {}, || {}));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let app = admin_router(gw, admin_links(&dir));
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let http = reqwest::Client::new();
    let mk = |id: &str, name: &str, key: &str, base: String| GatewayProvider {
        id: id.into(),
        name: name.into(),
        kind: "openai".into(),
        base_url: base,
        model: "m".into(),
        max_tokens: None,
        temperature: None,
        headers: Default::default(),
        api_key: key.into(),
        billing: Default::default(),
    };
    let cfg = GatewayConfig {
        version: 1,
        providers: vec![mk("saved", "Saved", KEY, format!("{up}/v1")), mk("wrong", "Wrong key", "good-not", format!("{up}/v1"))],
        audit_public_key: Some(audit_key()),
        generation_timeout_secs: 300,
        budget: Default::default(),
        kill_switch: false,
        kill_switch_aborts_running: true,
        paused_users: vec![],
    };
    let r = http.put(format!("{url}/api/model-gateway/v1/config")).bearer_auth(ADM).json(&cfg).send().await.unwrap();
    assert_eq!(r.status(), 200);
    G { url, http, dir }
}

impl G {
    async fn list(&self, body: Value) -> (u16, Value) {
        let r = self.http.post(format!("{}/api/model-gateway/v1/models", self.url)).bearer_auth(ADM).json(&body).send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap())
    }

    async fn test(&self, id: &str) -> (u16, Value) {
        let r = self.http.post(format!("{}/api/model-gateway/v1/providers/{id}/test", self.url)).bearer_auth(ADM).send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap())
    }
}

#[tokio::test]
async fn an_openai_list_comes_back_sorted() {
    let up = provider().await;
    let g = gateway(&up).await;
    let (s, v) = g.list(json!({"type": "openai", "base_url": format!("{up}/v1"), "api_key": "good"})).await;
    assert_eq!(s, 200);
    assert_eq!(v["ok"], true);
    assert_eq!(v["models"], json!([{"id": "m-a"}, {"id": "m-b"}]));
}

#[tokio::test]
async fn a_saved_providers_key_is_used_only_for_the_address_saved_with_it() {
    let up = provider().await;
    let g = gateway(&up).await;
    // No key in the request: the saved one, for the saved address.
    let (_, v) = g.list(json!({"type": "openai", "base_url": format!("{up}/v1"), "provider_id": "saved"})).await;
    assert_eq!(v["ok"], true, "{v}");
    // Another address: the saved key is not sent there, so the provider says no.
    let (_, v) = g.list(json!({"type": "openai", "base_url": format!("{up}/v1/"), "provider_id": "saved"})).await;
    assert_eq!(v["ok"], true, "a trailing slash is the same address");
    let (_, v) = g.list(json!({"type": "openai", "base_url": format!("{up}/empty/v1"), "provider_id": "saved"})).await;
    assert_eq!(v["ok"], false, "the empty list, not the saved key's answer");
}

#[tokio::test]
async fn a_bad_key_is_ok_false_and_the_key_is_not_echoed() {
    let up = provider().await;
    let g = gateway(&up).await;
    let (s, v) = g.list(json!({"type": "openai", "base_url": format!("{up}/v1"), "api_key": "good-not"})).await;
    assert_eq!(s, 200);
    assert_eq!(v["ok"], false);
    assert!(v["message"].as_str().unwrap().contains("401"));
    assert!(!v["message"].as_str().unwrap().contains("good-not"), "{v}");
}

#[tokio::test]
async fn anthropic_pages_are_followed_and_labels_kept() {
    let up = provider().await;
    let g = gateway(&up).await;
    let (_, v) = g.list(json!({"type": "anthropic", "base_url": format!("{up}/anthropic"), "api_key": "good"})).await;
    assert_eq!(v["models"], json!([{"id": "c-1"}, {"id": "c-2", "label": "C Two"}]));
    let (_, v) = g.list(json!({"type": "anthropic", "base_url": format!("{up}/anthropic")})).await;
    assert_eq!(v["ok"], false, "Anthropic needs a key");
}

#[tokio::test]
async fn bad_input_and_empty_lists_are_ok_false_and_the_admin_token_is_required() {
    let up = provider().await;
    let g = gateway(&up).await;
    assert_eq!(g.list(json!({"type": "gemini"})).await.1["ok"], false);
    assert_eq!(g.list(json!({"type": "openai", "base_url": "ftp://x"})).await.1["ok"], false);
    assert_eq!(g.list(json!({"type": "openai", "base_url": format!("{up}/empty/v1")})).await.1["ok"], false);
    assert_eq!(g.list(json!({"type": "openai", "provider_id": "nope"})).await.1["message"], "No such provider.");
    let r = g.http.post(format!("{}/api/model-gateway/v1/models", g.url)).json(&json!({"type": "openai"})).send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn the_provider_test_says_ok_with_the_right_key_and_never_repeats_a_wrong_one() {
    let up = provider().await;
    let g = gateway(&up).await;
    let (s, v) = g.test("saved").await;
    assert_eq!((s, v["ok"].as_bool(), v["message"].as_str()), (200, Some(true), Some("ok")), "{v}");
    let (s, v) = g.test("wrong").await;
    assert_eq!((s, v["ok"].as_bool()), (200, Some(false)), "{v}");
    assert!(v["message"].as_str().unwrap().contains("401"), "{v}");
    assert!(!v["message"].as_str().unwrap().contains("good-not"), "the provider echoed the key; the answer must not: {v}");
    assert_eq!(g.test("nobody").await.0, 404);
}
