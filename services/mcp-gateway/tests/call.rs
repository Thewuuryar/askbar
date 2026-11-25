//! `POST /tools/{tool}/call`: the signed approval is checked first, each failure has its own code and is counted, a valid
//! approval reaches the next check (a 501 for now), and one approval raced by many calls lets exactly one through.

use askbar_mcp_gateway::{links, router, Gateway};
use askbar_proto::approval::{Provenance, ToolAction};
use askbar_service::approval::{generate_keypair, sign};
use askbar_service::status::BootId;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

const ADMIN: &str = "admmcp_secret";
const SERVER: &str = "srvmcp_secret";

struct Rig {
    url: String,
    g: Arc<Gateway>,
    sk: Vec<u8>,
    pk: Vec<u8>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

async fn start(push_key: bool) -> Rig {
    let dir = std::env::temp_dir().join(format!("askbar-mcp-call-{}", BootId::new().as_str()));
    std::fs::create_dir_all(dir.join("accept")).unwrap();
    for (name, t) in [("admin-mcp-gateway", ADMIN), ("server-mcp-gateway", SERVER)] {
        std::fs::write(dir.join("accept").join(format!("{name}.sha256")), format!("{}\n", hex(&Sha256::digest(t.as_bytes())))).unwrap();
    }
    let g = Gateway::new(BootId::new(), None);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api/mcp-gateway/v1", l.local_addr().unwrap());
    let app = router(g.clone(), links(&dir));
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let (sk, pk) = generate_keypair().unwrap();
    let rig = Rig { url, g, sk, pk };
    if push_key {
        let cfg = json!({"fetch_max_bytes": 1048576, "fetch_timeout_secs": 20, "fetch_redirects": 3, "tool_call_timeout_secs": 30,
            "one_time_id_window_secs": 120, "search_results": 8, "remembered_urls": 50,
            "approval_public_key": hex(&rig.pk), "approval_key_version": 1});
        let r = reqwest::Client::new().put(format!("{}/config", rig.url)).bearer_auth(ADMIN).json(&cfg).send().await.unwrap();
        assert_eq!(r.status(), 200);
    }
    rig
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

fn action(url: &str) -> ToolAction {
    ToolAction {
        tool: "fetch".into(),
        arguments: json!({"url": url}),
        destination: "example.com".into(),
        provenance: Provenance::User,
        query_data: false,
    }
}

/// The body for `url`, approved with `sk`.
fn body(rig_sk: &[u8], signed_for: &str, sent: &str, id: &str, expires_at: i64) -> Value {
    let a = sign(&action(signed_for), "task1", "alice", id, expires_at, rig_sk).unwrap();
    json!({"arguments": {"url": sent}, "destination": "example.com", "provenance": "user", "approval": a})
}

async fn post(rig: &Rig, token: &str, body: &Value) -> (u16, Value) {
    let r = reqwest::Client::new().post(format!("{}/tools/fetch/call", rig.url)).bearer_auth(token).json(body).send().await.unwrap();
    (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
}

fn code(r: &(u16, Value)) -> (u16, &str) {
    (r.0, r.1["error"].as_str().unwrap_or(""))
}

#[tokio::test]
async fn each_bad_approval_is_refused_with_its_own_code_and_counted() {
    let rig = start(true).await;
    let u = "https://example.com/a";
    let mut unsigned = body(&rig.sk, u, u, "o1", now() + 60);
    unsigned["approval"]["signature"] = json!("");
    let swapped = body(&rig.sk, u, "https://example.com/other", "o2", now() + 60);
    let expired = body(&rig.sk, u, u, "o3", now() - 1);
    let (other_sk, _) = generate_keypair().unwrap();
    let wrong_key = body(&other_sk, u, u, "o4", now() + 60);
    let mut floaty = body(&rig.sk, u, u, "o5", now() + 60);
    floaty["arguments"] = json!({"url": u, "n": 1.5});

    assert_eq!(code(&post(&rig, SERVER, &unsigned).await), (403, "approval_unsigned"));
    assert_eq!(code(&post(&rig, SERVER, &swapped).await), (403, "approval_action_swapped"));
    assert_eq!(code(&post(&rig, SERVER, &expired).await), (403, "approval_expired"));
    assert_eq!(code(&post(&rig, SERVER, &wrong_key).await), (403, "approval_signature_invalid"));
    assert_eq!(code(&post(&rig, SERVER, &floaty).await), (400, "approval_unsignable"));
    assert_eq!(code(&post(&rig, SERVER, &json!({"arguments": {}})).await), (400, "bad_request"));

    let ok = body(&rig.sk, u, u, "o6", now() + 60);
    assert_eq!(code(&post(&rig, SERVER, &ok).await), (501, "tool_not_available"), "a valid approval reaches the next check");
    assert_eq!(code(&post(&rig, SERVER, &ok).await), (409, "approval_replayed"));

    let st = rig.g.status().calls;
    for (k, n) in [
        ("refused_approval_unsigned", 1),
        ("refused_approval_action_swapped", 1),
        ("refused_approval_expired", 1),
        ("refused_approval_signature_invalid", 1),
        ("refused_approval_unsignable", 1),
        ("refused_approval_replayed", 1),
        ("refused_tool_not_available", 1),
    ] {
        assert_eq!(st.get(k), Some(&n), "{k}");
    }
    let text = serde_json::to_string(&st).unwrap();
    let (pk, sig) = (hex(&rig.pk), ok["approval"]["signature"].as_str().unwrap());
    for secret in [u, "example.com", "o6", &pk, sig] {
        assert!(!text.contains(secret), "status leaked {secret}");
    }
}

#[tokio::test]
async fn fifty_racing_calls_with_one_approval_let_exactly_one_through() {
    let rig = Arc::new(start(true).await);
    let u = "https://example.com/race";
    let b = Arc::new(body(&rig.sk, u, u, "race", now() + 60));
    let tasks: Vec<_> = (0..50)
        .map(|_| {
            let (rig, b) = (rig.clone(), b.clone());
            tokio::spawn(async move { post(&rig, SERVER, &b).await })
        })
        .collect();
    let mut codes = Vec::new();
    for t in tasks {
        let r = t.await.unwrap();
        codes.push((r.0, r.1["error"].as_str().unwrap().to_string()));
    }
    assert_eq!(codes.iter().filter(|c| c.0 == 501).count(), 1, "exactly one passes to the next check");
    assert_eq!(codes.iter().filter(|c| **c == (409, "approval_replayed".to_string())).count(), 49);
}

#[tokio::test]
async fn the_admin_link_gets_403_and_a_call_without_config_or_key_is_refused() {
    let rig = start(true).await;
    let u = "https://example.com/a";
    let b = body(&rig.sk, u, u, "adm", now() + 60);
    assert_eq!(code(&post(&rig, ADMIN, &b).await), (403, "forbidden"));
    assert_eq!(code(&post(&rig, "nope", &b).await), (401, "unauthorized"));

    let none = start(false).await;
    assert_eq!(code(&post(&none, SERVER, &b).await), (503, "not_configured"), "before the first push");
    let cfg = json!({"fetch_max_bytes": 1048576, "fetch_timeout_secs": 20, "fetch_redirects": 3, "tool_call_timeout_secs": 30,
        "one_time_id_window_secs": 120, "search_results": 8, "remembered_urls": 50});
    let r = reqwest::Client::new().put(format!("{}/config", none.url)).bearer_auth(ADMIN).json(&cfg).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(code(&post(&none, SERVER, &b).await), (503, "not_configured"), "a config without a key still fails closed");
    let bad = json!({"approval_public_key": "zz"});
    let r = reqwest::Client::new().put(format!("{}/config", none.url)).bearer_auth(ADMIN).json(&bad).send().await.unwrap();
    assert_eq!(r.status(), 400);
}
