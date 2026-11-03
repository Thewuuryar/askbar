//! `POST /api/server/v1/models/list`: the server asks a provider for its models.

use axum::{extract::Query, http::HeaderMap, routing::get, Json, Router};
use askbar_server::{secrets::Secrets, server_api, AppState};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

const TOKEN: &str = "mgt_models-test-token";

/// A fake provider: `/v1/models` needs the bearer `good`; `/anthropic/v1/models` needs `x-api-key: good` and pages.
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
    let app = Router::new().route("/v1/models", get(openai)).route("/anthropic/v1/models", get(anthropic)).route("/empty/v1/models", get(|| async { Json(json!({"data": []})) }));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    url
}

async fn server() -> (String, reqwest::Client, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("models-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let hash_file = dir.join("token.sha256");
    let hex: String = Sha256::digest(TOKEN.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(&hash_file, hex).unwrap();
    let db = askbar_server::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    let state = AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), askbar_server::MIN_CLIENT);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let app = server_api::router(state, hash_file);
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (url, reqwest::Client::new(), dir)
}

async fn list(url: &str, http: &reqwest::Client, body: Value) -> (u16, Value) {
    let r = http.post(format!("{url}/api/server/v1/models/list")).bearer_auth(TOKEN).json(&body).send().await.unwrap();
    (r.status().as_u16(), r.json().await.unwrap())
}

#[tokio::test]
async fn an_openai_list_comes_back_sorted() {
    let (up, (url, http, dir)) = (provider().await, server().await);
    let (s, v) = list(&url, &http, json!({"type": "openai", "base_url": format!("{up}/v1"), "api_key": "good"})).await;
    assert_eq!(s, 200);
    assert_eq!(v["ok"], true);
    assert_eq!(v["models"], json!([{"id": "m-a"}, {"id": "m-b"}]));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_bad_key_is_ok_false_and_the_key_is_not_echoed() {
    let (up, (url, http, dir)) = (provider().await, server().await);
    let (s, v) = list(&url, &http, json!({"type": "openai", "base_url": format!("{up}/v1"), "api_key": "good-not"})).await;
    assert_eq!(s, 200);
    assert_eq!(v["ok"], false);
    assert!(v["message"].as_str().unwrap().contains("401"));
    assert!(!v["message"].as_str().unwrap().contains("good-not"), "{v}");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn anthropic_pages_are_followed_and_labels_kept() {
    let (up, (url, http, dir)) = (provider().await, server().await);
    let (_, v) = list(&url, &http, json!({"type": "anthropic", "base_url": format!("{up}/anthropic"), "api_key": "good"})).await;
    assert_eq!(v["models"], json!([{"id": "c-1"}, {"id": "c-2", "label": "C Two"}]));
    let (_, v) = list(&url, &http, json!({"type": "anthropic", "base_url": format!("{up}/anthropic")})).await;
    assert_eq!(v["ok"], false, "Anthropic needs a key");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn bad_input_and_empty_lists_are_ok_false_and_the_token_is_required() {
    let (up, (url, http, dir)) = (provider().await, server().await);
    assert_eq!(list(&url, &http, json!({"type": "gemini"})).await.1["ok"], false);
    assert_eq!(list(&url, &http, json!({"type": "openai", "base_url": "ftp://x"})).await.1["ok"], false);
    assert_eq!(list(&url, &http, json!({"type": "openai", "base_url": format!("{up}/empty/v1")})).await.1["ok"], false);
    assert_eq!(list(&url, &http, json!({"type": "openai", "provider_id": "nope"})).await.1["message"], "No such provider.");
    let r = http.post(format!("{url}/api/server/v1/models/list")).json(&json!({"type": "openai"})).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let _ = std::fs::remove_dir_all(dir);
}
