//! The Services tab without a controller (ADR 0005): each service's state comes from the service
//! itself, and a restart is session-only, recorded, rate limited, and limited to the five services.

mod common;

use askbar_admin::server_link::ServerLink;
use askbar_admin::{web, AdminDb, AdminState};
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use common::*;
use reqwest::Method as M;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn the_six_services_are_listed_from_the_services_themselves() {
    let t = start(true).await;
    let (s, v) = t.call(M::GET, "/api/admin/v1/services", json!(null)).await;
    assert_eq!(s, 200, "{v}");
    let by: Vec<(String, String)> = v["services"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["name"].as_str().unwrap().into(), c["state"].as_str().unwrap().into()))
        .collect();
    assert_eq!(
        by,
        [
            ("server".into(), "running".into()),
            ("model-gateway".into(), "running".into()),
            ("alerter".into(), "unreachable".into()),
            ("admin".into(), "running".into()),
            ("caddy".into(), "running".into()),
            ("cpa".into(), "unreachable".into())
        ],
        "the server and the gateway answer /status, Caddy its socket; the alerter and CLIProxyAPI are not set up in this test"
    );
    assert!(v["services"][0]["status"].as_str().unwrap().contains("not received its configuration"), "{v}");
    assert_eq!(v["recent"], json!([]));

    // With Caddy's socket silent, it reads as unreachable.
    let t = start(false).await;
    let (_, v) = t.call(M::GET, "/api/admin/v1/services", json!(null)).await;
    let caddy = v["services"].as_array().unwrap().iter().find(|c| c["name"] == "caddy").unwrap();
    assert_eq!(caddy["state"], "unreachable");
}

/// A server stand-in: boot ID "A" until it is asked to restart, then "B" (a new process). `refuse`
/// answers the restart with the cooldown's 429 instead.
#[derive(Default)]
struct FakeServer {
    restarted: AtomicBool,
    asked: AtomicU32,
    refuse: bool,
}

async fn status(State(f): State<Arc<FakeServer>>) -> Json<Value> {
    Json(json!({ "config_version": 1, "boot_id": if f.restarted.load(Ordering::SeqCst) { "B" } else { "A" } }))
}

async fn restart(State(f): State<Arc<FakeServer>>) -> Result<(StatusCode, Json<Value>), askbar_proto::http::ApiError> {
    f.asked.fetch_add(1, Ordering::SeqCst);
    if f.refuse {
        return Err(askbar_proto::http::ApiError::new(askbar_proto::problem::ErrorCode::TooManyAttempts, "This service started 3s ago.")
            .retry_after(27));
    }
    let f2 = f.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        f2.restarted.store(true, Ordering::SeqCst);
    });
    Ok((StatusCode::ACCEPTED, Json(json!({ "boot_id": "A" }))))
}

async fn admin_with_fake_server(refuse: bool) -> (String, String, Arc<FakeServer>, Arc<AdminState>) {
    let fake = Arc::new(FakeServer { refuse, ..Default::default() });
    let app =
        Router::new().route("/api/server/v1/status", get(status)).route("/api/server/v1/restart", post(restart)).with_state(fake.clone());
    let url = serve(app).await;
    let db = AdminDb::open_in_memory().unwrap();
    web::init_password(&db, PW).unwrap();
    let admin = AdminState::with_server(db, ServerLink::new(&url, "mgt_t".into()));
    let adm = serve(askbar_admin::router(admin.clone())).await;
    let r = reqwest::Client::new()
        .post(format!("{adm}/api/admin/v1/session"))
        .header("x-askbar-admin", "1")
        .json(&json!({ "password": PW }))
        .send()
        .await
        .unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    (adm, cookie, fake, admin)
}

async fn post_as(adm: &str, cookie: &str, path: &str) -> (u16, Value) {
    let r =
        reqwest::Client::new().post(format!("{adm}{path}")).header("x-askbar-admin", "1").header("cookie", cookie).send().await.unwrap();
    (r.status().as_u16(), r.json().await.unwrap_or(json!(null)))
}

#[tokio::test]
async fn a_server_restart_asks_the_server_and_waits_for_a_new_process() {
    let (adm, cookie, fake, _) = admin_with_fake_server(false).await;
    let t0 = std::time::Instant::now();
    let (s, v) = post_as(&adm, &cookie, "/api/admin/v1/services/server/restart").await;
    assert_eq!((s, v["state"].as_str()), (200, Some("running")), "{v}");
    assert!(v["status"].as_str().unwrap().starts_with("Restarted"), "{v}");
    assert!(t0.elapsed() < Duration::from_secs(60), "ADR 0002: back within a minute");
    assert_eq!(fake.asked.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn the_servers_cooldown_refusal_is_passed_on() {
    let (adm, cookie, _, admin) = admin_with_fake_server(true).await;
    let (s, v) = post_as(&adm, &cookie, "/api/admin/v1/services/server/restart").await;
    assert_eq!((s, v["error"].as_str()), (429, Some("too_many_attempts")), "{v}");
    // The attempt is on record, and marked as not done.
    let db = admin.db.lock().await;
    let (service, action, ok): (String, String, i64) =
        db.conn().query_row("SELECT service, action, ok FROM service_actions", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!((service.as_str(), action.as_str(), ok), ("server", "restart", 0));
}

#[tokio::test]
async fn the_admin_restarts_itself_by_answering_and_exiting() {
    let t = start(true).await;
    let exited = Arc::new(AtomicBool::new(false));
    let e = exited.clone();
    let _ = t.admin.restarter.set(askbar_service::restart::Restarter::for_tests(
        askbar_service::status::BootId::new(),
        Duration::ZERO,
        Duration::from_secs(1),
        || {},
        move || e.store(true, Ordering::SeqCst),
    ));
    let (s, v) = t.call(M::POST, "/api/admin/v1/services/admin/restart", json!(null)).await;
    assert_eq!((s, v["state"].as_str()), (202, Some("restarting")), "{v}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(exited.load(Ordering::SeqCst), "it exits after answering");
}

#[tokio::test]
async fn caddy_is_restarted_by_pushing_its_configuration_again() {
    let t = start(true).await;
    let before = t.fake.loads.lock().unwrap().len();
    let (s, v) = t.call(M::POST, "/api/admin/v1/services/caddy/restart", json!(null)).await;
    assert_eq!((s, v["state"].as_str()), (200, Some("running")), "{v}");
    assert_eq!(t.fake.loads.lock().unwrap().len(), before + 1, "one config push");
    let (_, v) = t.call(M::GET, "/api/admin/v1/services", json!(null)).await;
    assert_eq!(
        (v["recent"][0]["service"].as_str(), v["recent"][0]["action"].as_str(), v["recent"][0]["ok"].clone()),
        (Some("caddy"), Some("restart"), json!(true))
    );
}

#[tokio::test]
async fn only_the_six_services_can_be_restarted_and_start_and_stop_are_gone() {
    let t = start(true).await;
    for name in ["controller", "caddy%2F..", "mock-llm", "auditor"] {
        assert_eq!(t.call(M::POST, &format!("/api/admin/v1/services/{name}/restart"), json!(null)).await.0, 404, "{name}");
    }
    for action in ["start", "stop", "kill"] {
        assert_eq!(t.call(M::POST, &format!("/api/admin/v1/services/server/{action}"), json!(null)).await.0, 404, "{action}");
    }
}

#[tokio::test]
async fn an_admin_api_key_cannot_restart_services() {
    let t = start(true).await;
    let (_, k) = t.call(M::POST, "/api/admin/v1/keys", json!({ "name": "script" })).await;
    let key = k["key"].as_str().unwrap();
    for (m, path) in [(M::GET, "/api/admin/v1/services"), (M::POST, "/api/admin/v1/services/server/restart")] {
        let r = reqwest::Client::new()
            .request(m, format!("{}{path}", t.adm))
            .bearer_auth(key)
            .header("X-Askbar-Admin", "1")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401, "{path}");
    }
    let r = reqwest::Client::new()
        .post(format!("{}/api/admin/v1/services/server/restart", t.adm))
        .header("X-Askbar-Admin", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401, "and no session at all");
}

#[tokio::test]
async fn at_most_six_actions_a_minute() {
    let t = start(true).await;
    for _ in 0..6 {
        assert_eq!(t.call(M::POST, "/api/admin/v1/services/caddy/restart", json!(null)).await.0, 200);
    }
    let (s, v) = t.call(M::POST, "/api/admin/v1/services/caddy/restart", json!(null)).await;
    assert_eq!((s, v["error"].as_str()), (429, Some("too_many_attempts")));
}

#[tokio::test]
async fn cpa_is_restarted_through_its_supervisor_files() {
    // ADR 0037: the admin writes restart-request and waits for the supervisor to echo it in restart-done.
    // A thread stands in for the supervisor. (Only this test sets ASKBAR_CPA_CONFIG_DIR.)
    let dir = std::env::temp_dir().join(format!("askbar-cpa-restart-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("ASKBAR_CPA_CONFIG_DIR", &dir);
    let d = dir.clone();
    let supervisor = std::thread::spawn(move || {
        for _ in 0..200 {
            if let Ok(v) = std::fs::read_to_string(d.join("restart-request")) {
                std::thread::sleep(Duration::from_millis(300)); // CLIProxyAPI starting again
                std::fs::write(d.join("restart-done"), v).unwrap();
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    });
    let t = start(true).await;
    let (s, v) = t.call(M::POST, "/api/admin/v1/services/cpa/restart", json!(null)).await;
    assert!(supervisor.join().unwrap(), "the admin wrote a restart request");
    assert_eq!((s, v["state"].as_str()), (200, Some("running")), "{v}");
    let (_, v) = t.call(M::GET, "/api/admin/v1/services", json!(null)).await;
    assert_eq!((v["recent"][0]["service"].as_str(), v["recent"][0]["ok"].clone()), (Some("cpa"), json!(true)));
    let _ = std::fs::remove_dir_all(&dir);
}
