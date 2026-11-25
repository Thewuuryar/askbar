//! The MCP page's routes (P8.6a): the front's status, the egress rows, the approval key's rotation (session only, a sign-in within
//! 10 minutes, audited, pushed to the front and to `model-gateway` at once) and no key material anywhere.

mod common;
use askbar_admin::mcp_link::McpLink;
use askbar_proto::approval::ToolAction;
use askbar_service::approval::{sign, verify, OneTimeIds};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use common::serve;
use reqwest::Method as M;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const PASS: &str = "a long enough passphrase for tests";

/// What a fake service was last given: the front keeps its config, the gateway and the proxy their pushes.
#[derive(Default)]
struct Fake {
    config: Mutex<Option<Value>>,
    calls: Mutex<Value>,
}
type F = Arc<Fake>;

async fn status(State(f): State<F>) -> Json<Value> {
    let v = f.config.lock().unwrap().as_ref().map(|c| c["version"].clone());
    Json(json!({ "config_version": v, "boot_id": "b", "calls": f.calls.lock().unwrap().as_object().cloned().unwrap_or_default() }))
}
async fn config(State(f): State<F>, Json(body): Json<Value>) -> Json<Value> {
    *f.config.lock().unwrap() = Some(body);
    Json(json!({}))
}

struct Ids;
impl OneTimeIds for Ids {
    fn consume(&self, _: &str, _: i64) -> bool {
        true
    }
}

/// The front reports the version it computes from the config it was given, as the real one does.
fn front_router(f: &F) -> Router {
    let cfg = |State(f): State<F>, Json(body): Json<Value>| async move {
        let v = serde_json::from_value::<askbar_mcp_gateway::config::Config>(body.clone()).map(|c| c.version()).unwrap_or(0);
        let mut b = body;
        b["version"] = json!(v);
        config(State(f), Json(b)).await
    };
    Router::new().route("/api/mcp-gateway/v1/status", get(status)).route("/api/mcp-gateway/v1/config", put(cfg)).with_state(f.clone())
}

fn front_key(f: &F) -> Option<Vec<u8>> {
    let c = f.config.lock().unwrap();
    let hex = c.as_ref()?["approval_public_key"].as_str()?.to_string();
    Some((0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect())
}

fn action() -> ToolAction {
    serde_json::from_value(json!({ "tool": "fetch", "arguments": { "url": "https://example.com/" }, "destination": "example.com" }))
        .unwrap()
}

async fn until(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..100 {
        if ok() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("never happened: {what}");
}

#[tokio::test]
async fn status_reports_the_front_and_the_egress_rows_and_an_unconfigured_front_says_so() {
    let t = common::start_with(true, false).await;
    // No links at all: said, not an error.
    let (st, b) = t.call(M::GET, "/api/admin/v1/mcp/status", json!(null)).await;
    assert_eq!((st, b["online"].clone(), b["configured"].clone()), (200, json!(false), json!(false)));
    assert!(b["error"].is_string() && b["reported_version"].is_null());

    let front = F::default();
    *front.calls.lock().unwrap() = json!({ "refused_not_configured": 2 });
    let link = McpLink::new(&serve(front_router(&front)).await, "t".into());
    t.admin.mcp.set(link.clone()).ok().unwrap();
    // Before the first answer the front is unconfigured.
    let (_, b) = t.call(M::GET, "/api/admin/v1/mcp/status", json!(null)).await;
    assert_eq!(b["configured"], json!(false));
    link.sync(&t.admin).await; // pushes
    link.sync(&t.admin).await; // reads the status of the configured front, with the counts
    let (_, b) = t.call(M::GET, "/api/admin/v1/mcp/status", json!(null)).await;
    assert_eq!(
        (b["online"].clone(), b["configured"].clone(), b["reported_version"].clone()),
        (json!(true), json!(true), b["pushed_version"].clone())
    );
    assert_eq!(b["calls"], json!({ "refused_not_configured": 2 }));

    // The egress rows: what was pushed per source, and the proxy's counts.
    let proxy = serve(Router::new().route(
        "/api/egress-proxy/v1/status",
        get(|| async {
            Json(json!({ "configured": true, "version": 1, "boot_id": "b", "open_tunnels": 0, "recent_denied": [],
                "sources": { "searxng": { "allowed": 7, "denied": { "not_listed": 2 } }, "fetch": { "allowed": 3, "denied": {} } } }))
        }),
    ))
    .await;
    let eg = askbar_admin::egress_link::EgressLink::new(&proxy, "t".into());
    t.admin.egress.set(eg.clone()).ok().unwrap();
    eg.status_now().await;
    let (st, b) = t.call(M::GET, "/api/admin/v1/egress/status", json!(null)).await;
    assert_eq!((st, b["online"].clone()), (200, json!(true)), "{b}");
    let s = &b["sources"];
    assert_eq!((s["searxng"]["allowed"].clone(), s["searxng"]["denied"].clone()), (json!(7), json!({ "not_listed": 2 })));
    assert_eq!(s["searxng"]["entries"].as_array().unwrap().len(), askbar_admin::constants::SEARXNG_EGRESS_HOSTS.len());
    let fetch = s["fetch"]["entries"].as_array().unwrap();
    assert!(fetch.len() == 2 && fetch.iter().all(|e| e["any_public"] == json!(true)));
    assert_eq!(s["fetch"]["allowed"], json!(3));
}

#[tokio::test]
async fn a_rotation_is_pushed_to_the_front_and_the_gateway_and_the_old_key_is_refused_with_no_key_material_shown() {
    let t = common::start_with(true, false).await;
    let (front, gw) = (F::default(), F::default());
    let link = McpLink::new(&serve(front_router(&front)).await, "t".into());
    let gw_url = serve(
        Router::new()
            .route(
                "/api/model-gateway/v1/status",
                get(|State(f): State<F>| async move {
                    let v = f.config.lock().unwrap().as_ref().map(|c| c["version"].clone());
                    Json(json!({ "config_version": v, "boot_id": "b" }))
                }),
            )
            .route("/api/model-gateway/v1/config", put(config))
            .with_state(gw.clone()),
    )
    .await;
    let glink = askbar_admin::gateway_link::GatewayLink::new(&gw_url, "t".into());
    t.admin.mcp.set(link.clone()).ok().unwrap();
    t.admin.gateway.set(glink.clone()).ok().unwrap();
    let (st, _) = t.call(M::PUT, "/api/admin/v1/setup/audit-key", json!({ "passphrase": PASS, "confirm": PASS })).await;
    assert_eq!(st, 200);
    askbar_admin::approval_key::ensure(t.admin.db.lock().await.conn(), &t.admin.secrets).unwrap();
    tokio::spawn(link.run(t.admin.clone()));
    tokio::spawn(glink.run(t.admin.clone()));
    until("the first pushes", || front_key(&front).is_some() && gw.config.lock().unwrap().is_some()).await;

    let old_private = {
        let db = t.admin.db.lock().await;
        URL_SAFE_NO_PAD.decode(askbar_admin::approval_key::signing_key(db.conn(), &t.admin.secrets).unwrap().pkcs8).unwrap()
    };
    let old_public = front_key(&front).unwrap();
    let a = sign(&action(), "task", "U1", "one", i64::MAX, &old_private).unwrap();
    assert!(verify(&a, &action(), &old_public, 0, &Ids).is_ok(), "the front accepts the old key before the rotation");

    let (st, b) = t.call(M::POST, "/api/admin/v1/mcp/approval-key/rotate", json!(null)).await;
    assert_eq!((st, b["version"].clone()), (200, json!(2)), "{b}");
    until("the new keys are pushed", || front_key(&front).is_some_and(|k| k != old_public)).await;
    until("the gateway has the new key", || gw.config.lock().unwrap().as_ref().is_some_and(|c| c["approval_key"]["version"] == json!(2)))
        .await;
    let new_private = gw.config.lock().unwrap().as_ref().unwrap()["approval_key"]["pkcs8"].as_str().unwrap().to_string();

    let new_public = front_key(&front).unwrap();
    assert!(verify(&a, &action(), &new_public, 0, &Ids).is_err(), "a call signed with the old key is refused after the push");
    let b2 = sign(&action(), "task", "U1", "two", i64::MAX, &URL_SAFE_NO_PAD.decode(&new_private).unwrap()).unwrap();
    assert!(verify(&b2, &action(), &new_public, 0, &Ids).is_ok(), "the gateway's new key matches the front's");

    // Nothing shows a private key, a public key or a token: the answer, the status and the audit records.
    let (_, status) = t.call(M::GET, "/api/admin/v1/mcp/status", json!(null)).await;
    let hex: String = new_public.iter().map(|x| format!("{x:02x}")).collect();
    let rows = t.rig.rows("SELECT detail_json FROM admin_actions");
    let all = format!("{b} {status} {rows:?}");
    for secret in [new_private.as_str(), &URL_SAFE_NO_PAD.encode(&old_private), hex.as_str(), "\"t\""] {
        assert!(!all.contains(secret), "{secret} shown");
    }
    assert_eq!(status["approval_key"]["fingerprint"], b["fingerprint"]);
    assert_eq!(b["fingerprint"].as_str().unwrap().len(), 16);
    let done = t.rig.rows("SELECT outcome FROM admin_actions WHERE action = 'approval_key_rotate'");
    assert!(done.len() == 2 && done[0][0] == "requested" && done[1][0] == "done", "{done:?}");
}

#[tokio::test]
async fn the_session_only_routes_refuse_an_api_key_and_a_rotation_needs_a_recent_sign_in() {
    let t = common::start(true).await;
    let (_, made) = t.call(M::POST, "/api/admin/v1/keys", json!({ "name": "ci" })).await;
    let key = made["key"].as_str().unwrap();
    for (m, p) in [(M::GET, "/mcp/status"), (M::GET, "/egress/status"), (M::POST, "/mcp/approval-key/rotate")] {
        let r =
            t.http.request(m, format!("{}/api/admin/v1{p}", t.adm)).header("x-askbar-admin", "1").bearer_auth(key).send().await.unwrap();
        assert_eq!(r.status(), 401, "{p}");
    }
    let before = askbar_admin::approval_key::public_key(t.admin.db.lock().await.conn());
    t.admin.auth.backdate_session(t.cookie.split_once('=').unwrap().1, 11 * 60);
    let (st, b) = t.call(M::POST, "/api/admin/v1/mcp/approval-key/rotate", json!(null)).await;
    assert_eq!(st, 403, "{b}");
    assert_eq!(askbar_admin::approval_key::public_key(t.admin.db.lock().await.conn()), before, "nothing changed");
}

#[tokio::test]
async fn a_rotation_is_refused_when_the_record_cannot_be_appended() {
    let db = askbar_admin::AdminDb::open_in_memory().unwrap();
    askbar_admin::web::init_password(&db, common::PW).unwrap();
    let admin = askbar_admin::AdminState::new(db);
    askbar_admin::approval_key::ensure(admin.db.lock().await.conn(), &admin.secrets).unwrap();
    let auditor =
        serve(Router::new().route("/api/audit/v1/admin-actions", axum::routing::post(|| async { StatusCode::SERVICE_UNAVAILABLE }))).await;
    admin.actions.set(askbar_admin::actions::ActionLog::new(&auditor, "t".into())).ok().unwrap();
    let adm = serve(askbar_admin::router(admin.clone())).await;
    let http = reqwest::Client::new();
    let r = http
        .post(format!("{adm}/api/admin/v1/session"))
        .header("x-askbar-admin", "1")
        .json(&json!({ "password": common::PW }))
        .send()
        .await
        .unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    let before = askbar_admin::approval_key::public_key(admin.db.lock().await.conn());
    let r = http
        .post(format!("{adm}/api/admin/v1/mcp/approval-key/rotate"))
        .header("x-askbar-admin", "1")
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 503);
    assert_eq!(askbar_admin::approval_key::public_key(admin.db.lock().await.conn()), before, "the key was not rotated");
}
