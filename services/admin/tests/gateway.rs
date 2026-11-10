//! The admin and `model-gateway` (ADR 0054, ADR 0028): the admin keeps the provider keys, encrypted, and pushes them
//! in `PUT /config`; it pushes again whenever the gateway reports no configuration or another one (a restart is the
//! first); and no key is returned, logged or kept in the clear.

mod common;
use askbar_admin::gateway_link::{build_config, version_of, GatewayLink, Sync};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, put};
use axum::{Json, Router};
use common::{serve, T};
use reqwest::Method as M;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const KEY: &str = "sk-marker-key-in-the-admin";
const PASS: &str = "a long enough passphrase for tests";
const GATEWAY_TOKEN: &str = "admgw_test_token";

/// A stand-in for the gateway's admin listener: it records each configuration it is given, reports the version it
/// runs (none until one is given), and takes only its own token.
#[derive(Default)]
struct FakeGateway {
    running: Mutex<Option<u64>>,
    pushes: Mutex<Vec<Value>>,
    refuse: Mutex<bool>,
    /// Which process this is; a restart makes a new one that runs no configuration.
    boot: Mutex<u32>,
}

type Fake = Arc<FakeGateway>;

fn authorized(h: &HeaderMap) -> bool {
    h.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {GATEWAY_TOKEN}"))
}

async fn fake_status(State(f): State<Fake>, h: HeaderMap) -> Result<Json<Value>, StatusCode> {
    if !authorized(&h) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(Json(json!({ "config_version": *f.running.lock().unwrap(), "boot_id": format!("boot-{}", f.boot.lock().unwrap()) })))
}

async fn fake_restart(State(f): State<Fake>, h: HeaderMap) -> Result<(StatusCode, Json<Value>), StatusCode> {
    if !authorized(&h) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let old = format!("boot-{}", f.boot.lock().unwrap());
    *f.boot.lock().unwrap() += 1;
    *f.running.lock().unwrap() = None;
    Ok((StatusCode::ACCEPTED, Json(json!({ "boot_id": old }))))
}

async fn fake_config(State(f): State<Fake>, h: HeaderMap, Json(body): Json<Value>) -> Result<Json<Value>, StatusCode> {
    if !authorized(&h) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if *f.refuse.lock().unwrap() {
        return Err(StatusCode::BAD_REQUEST);
    }
    *f.running.lock().unwrap() = body["version"].as_u64();
    f.pushes.lock().unwrap().push(body.clone());
    Ok(Json(json!({ "version": body["version"] })))
}

async fn rig() -> (T, Fake, Arc<GatewayLink>) {
    let t = common::start_with(true, false).await;
    let fake = Fake::default();
    let app = Router::new()
        .route("/api/model-gateway/v1/status", get(fake_status))
        .route("/api/model-gateway/v1/config", put(fake_config))
        .route("/api/model-gateway/v1/restart", axum::routing::post(fake_restart))
        .with_state(fake.clone());
    let url = serve(app).await;
    let link = GatewayLink::new(&url, GATEWAY_TOKEN.into());
    t.admin.gateway.set(link.clone()).ok().unwrap();
    (t, fake, link)
}

async fn set_audit_key(t: &T) {
    let (st, _) = t.call(M::PUT, "/api/admin/v1/setup/audit-key", json!({ "passphrase": PASS, "confirm": PASS })).await;
    assert_eq!(st, 200);
}

async fn add_provider(t: &T, name: &str, key: &str) -> String {
    let (st, body) = t
        .call(
            M::POST,
            "/api/admin/v1/providers",
            json!({ "name": name, "type": "openai", "base_url": "http://mock:8765/v1", "model": "m", "api_key": key }),
        )
        .await;
    assert_eq!(st, 201, "{body}");
    body["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn a_key_entered_in_the_admin_is_pushed_to_the_gateway_and_kept_only_encrypted() {
    let (t, fake, link) = rig().await;
    set_audit_key(&t).await;
    let id = add_provider(&t, "Mock", KEY).await;

    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)), "an unconfigured gateway is given its configuration");
    let pushed = fake.pushes.lock().unwrap().last().cloned().unwrap();
    let providers = pushed["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!((providers[0]["id"].as_str().unwrap(), providers[0]["api_key"].as_str().unwrap()), (id.as_str(), KEY));
    assert!(pushed["audit_public_key"]["key_id"].is_string(), "the audit key the gateway seals errors to");

    // Kept in admin.db, but not in the clear.
    let db = t.admin.db.lock().await;
    let blob: Vec<u8> = db.conn().query_row("SELECT key_enc FROM provider_keys WHERE provider_id = ?1", [&id], |r| r.get(0)).unwrap();
    assert!(!blob.windows(KEY.len()).any(|w| w == KEY.as_bytes()), "the key is encrypted at rest");
    assert_eq!(t.admin.secrets.decrypt(&blob).as_deref(), Some(KEY));
    drop(db);

    // No answer of the admin holds it.
    for path in ["/api/admin/v1/providers", "/api/admin/v1/services", "/api/admin/v1/status"] {
        let (st, body) = t.call(M::GET, path, json!(null)).await;
        assert_eq!(st, 200, "{path}");
        assert!(!body.to_string().contains(KEY), "{path} returned the key");
    }
}

#[tokio::test]
async fn it_pushes_again_when_the_gateway_has_none_and_not_while_it_runs_the_current_one() {
    let (t, fake, link) = rig().await;
    set_audit_key(&t).await;
    add_provider(&t, "Mock", KEY).await;

    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    assert_eq!(link.sync(&t.admin).await, Sync::Current, "it runs this configuration: nothing to push");
    assert_eq!(fake.pushes.lock().unwrap().len(), 1);

    // A restart: the new process has no configuration (its keys were in memory).
    *fake.running.lock().unwrap() = None;
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    assert_eq!(fake.pushes.lock().unwrap().len(), 2);
    let pushes = fake.pushes.lock().unwrap().clone();
    assert_eq!(pushes[0], pushes[1], "the same configuration, keys and all");

    // Another configuration (a changed key) is pushed too.
    let (_, list) = t.call(M::GET, "/api/admin/v1/providers", json!(null)).await;
    let id = list[0]["id"].as_str().unwrap().to_string();
    let (st, _) = t
        .call(
            M::PUT,
            &format!("/api/admin/v1/providers/{id}"),
            json!({ "name": "Mock", "type": "openai", "base_url": "http://mock:8765/v1", "model": "m", "api_key": "sk-new-key-value" }),
        )
        .await;
    assert_eq!(st, 204);
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    assert_eq!(fake.pushes.lock().unwrap().last().unwrap()["providers"][0]["api_key"], "sk-new-key-value");
}

#[tokio::test]
async fn a_deleted_provider_takes_its_key_out_of_the_admin_and_the_next_configuration() {
    let (t, fake, link) = rig().await;
    set_audit_key(&t).await;
    let keep = add_provider(&t, "Keep", "sk-keep").await;
    let gone = add_provider(&t, "Gone", KEY).await;
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    assert_eq!(fake.pushes.lock().unwrap()[0]["providers"].as_array().unwrap().len(), 2);

    let (st, _) = t.call(M::DELETE, &format!("/api/admin/v1/providers/{gone}"), json!(null)).await;
    assert_eq!(st, 204);
    let db = t.admin.db.lock().await;
    let rows: Vec<String> = db
        .conn()
        .prepare("SELECT provider_id FROM provider_keys")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows, std::slice::from_ref(&keep), "the key of a provider that is gone is gone too");
    drop(db);
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    let last = fake.pushes.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last["providers"].as_array().unwrap().len(), 1);
    assert!(!last.to_string().contains(KEY));
}

#[tokio::test]
async fn without_an_audit_key_nothing_is_pushed_and_it_says_why() {
    let (t, fake, link) = rig().await;
    add_provider(&t, "Mock", KEY).await;
    let Sync::NotReady(why) = link.sync(&t.admin).await else { panic!("nothing can be pushed without the audit key") };
    assert!(why.contains("audit key"), "{why}");
    assert!(fake.pushes.lock().unwrap().is_empty());
    assert!(link.last_error().unwrap().contains("audit key"));
    set_audit_key(&t).await;
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)), "it goes as soon as there is a key");
}

#[tokio::test]
async fn a_gateway_that_does_not_answer_or_refuses_is_reported_and_never_stops_a_change() {
    let t = common::start_with(true, false).await;
    // Nothing listens here.
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let link = GatewayLink::new(&format!("http://127.0.0.1:{dead}"), GATEWAY_TOKEN.into());
    t.admin.gateway.set(link.clone()).ok().unwrap();
    set_audit_key(&t).await;
    // The change is made, with the server, as it was before the gateway existed; the gateway catches up.
    add_provider(&t, "Mock", KEY).await;
    assert_eq!(link.sync(&t.admin).await, Sync::Unreachable);
    assert!(!link.online());
    assert!(link.last_error().unwrap().contains("does not answer"));

    let (_, fake, link) = rig().await;
    *fake.refuse.lock().unwrap() = true;
    let t2 = common::start_with(true, false).await;
    t2.admin.gateway.set(link.clone()).ok().unwrap();
    set_audit_key(&t2).await;
    let Sync::Refused(why) = link.sync(&t2.admin).await else { panic!("a refusal is reported") };
    assert!(why.contains("refused"), "{why}");
    assert!(!why.contains(KEY), "the sentence never holds a key");
}

#[tokio::test]
async fn the_gateways_own_token_is_the_only_one_it_takes() {
    let (t, _fake, _) = rig().await;
    let wrong = GatewayLink::new(&format!("http://{}", "127.0.0.1:1"), "x".into());
    let _ = wrong;
    // A link with the wrong token is told 401, and says so without showing a token.
    let app = Router::new().route("/api/model-gateway/v1/status", get(fake_status)).with_state(Fake::default());
    let url = serve(app).await;
    let link = GatewayLink::new(&url, "not-the-token".into());
    assert!(link.status_now().await.is_none());
    assert_eq!(link.last_error().as_deref(), Some("The gateway does not accept this admin's token."));
    drop(t);
}

#[tokio::test]
async fn the_services_page_lists_the_gateway() {
    let (t, _fake, link) = rig().await;
    let (st, body) = t.call(M::GET, "/api/admin/v1/services", json!(null)).await;
    assert_eq!(st, 200);
    let card = body["services"].as_array().unwrap().iter().find(|s| s["name"] == "model-gateway").expect("a card for the gateway");
    assert_eq!(card["state"], "running");
    assert!(card["status"].as_str().unwrap().contains("not received its configuration"), "{card}");

    set_audit_key(&t).await;
    add_provider(&t, "Mock", KEY).await;
    link.sync(&t.admin).await;
    let (_, body) = t.call(M::GET, "/api/admin/v1/services", json!(null)).await;
    let card = body["services"].as_array().unwrap().iter().find(|s| s["name"] == "model-gateway").unwrap();
    assert!(card["status"].as_str().unwrap().contains("runs the configuration"), "{card}");
    assert!(!body.to_string().contains(KEY));
}

#[tokio::test]
async fn a_version_is_the_same_for_the_same_content_and_changes_with_any_key() {
    let (t, _fake, _) = rig().await;
    set_audit_key(&t).await;
    let id = add_provider(&t, "Mock", KEY).await;
    let build = |t: &T| {
        let db = futures_util::FutureExt::now_or_never(t.admin.db.lock()).unwrap();
        build_config(db.conn(), &t.admin.secrets, None).unwrap()
    };
    let (a, b) = (build(&t), build(&t));
    assert_eq!(a.version, b.version);
    assert_eq!(a.version, version_of(&a));
    assert!(a.version < (1u64 << 53), "an integer any client reads exactly");
    let mut other = a.clone();
    other.providers[0].api_key = "sk-another".into();
    assert_ne!(version_of(&other), a.version);
    // The managed provider's key is the admin's own, not one entered.
    let managed = (id, "sk-cpa-managed".to_string());
    let db = t.admin.db.lock().await;
    let c = build_config(db.conn(), &t.admin.secrets, Some(&managed)).unwrap();
    assert_eq!(c.providers[0].api_key, "sk-cpa-managed");
}

#[tokio::test]
async fn restarting_the_gateway_waits_for_a_new_process_and_gives_it_its_keys_again() {
    let (t, fake, link) = rig().await;
    set_audit_key(&t).await;
    add_provider(&t, "Mock", KEY).await;
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));

    let (st, card) = t.call(M::POST, "/api/admin/v1/services/model-gateway/restart", json!(null)).await;
    assert_eq!(st, 200, "{card}");
    assert_eq!(card["name"], "model-gateway");
    assert!(card["status"].as_str().unwrap().contains("with its keys"), "{card}");
    let pushes = fake.pushes.lock().unwrap().clone();
    assert_eq!(pushes.len(), 2, "pushed again after the restart");
    assert_eq!(pushes[1]["providers"][0]["api_key"], KEY);
    assert!(!card.to_string().contains(KEY));
    // The restart is on the admin's own record of service actions, like the others.
    let db = t.admin.db.lock().await;
    let n: i64 =
        db.conn().query_row("SELECT COUNT(*) FROM service_actions WHERE service = 'model-gateway' AND ok = 1", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}
