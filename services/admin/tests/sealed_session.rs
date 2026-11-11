#![cfg(unix)]
//! Sealed-content sessions (ADR 0011 rules 8 to 13): a passphrase after a fresh sign-in opens content for a while, the
//! keys live in memory only, a session ends on Seal, a timeout, a sign-out, and what happens is recorded and alerted.

mod common;
use common::T;
use reqwest::Method as M;
use serde_json::{json, Value};
use std::time::Duration;

const PASS: &str = "a passphrase long enough to use";

async fn with_sealed_reply() -> (T, String) {
    let t = common::start(true).await;
    let (st, _) = t.call(M::PUT, "/api/admin/v1/setup/audit-key", json!({ "passphrase": PASS, "confirm": PASS })).await;
    assert_eq!(st, 200);
    let key = askbar_server::snapshot::current(&t.server.db.lock().unwrap()).unwrap().unwrap().audit_public_key.unwrap();
    let public = askbar_server::snapshot::parse_hex(&key.public_key).unwrap();
    let span = t
        .rig
        .seed(
            "trace-1",
            askbar_server::audit::Kind::Turn,
            "chat",
            Some(("reply", askbar_server::seal::seal(&public, b"sealed reply text").unwrap())),
            true,
        )
        .await;
    (t, span)
}

async fn unseal(t: &T) -> (u16, Value) {
    t.call(M::POST, "/api/admin/v1/audit/session", json!({ "passphrase": PASS })).await
}

async fn view(t: &T, span: &str) -> (u16, Value) {
    t.call(M::GET, &format!("/api/admin/v1/audit/payloads/{span}"), json!(null)).await
}

fn token(t: &T) -> String {
    t.cookie.split_once('=').unwrap().1.to_string()
}

fn records(t: &T) -> Vec<(String, String)> {
    t.rig
        .rows("SELECT category, action || ':' || outcome FROM admin_actions WHERE category = 'sealed_content' ORDER BY at, rowid")
        .into_iter()
        .map(|r| (r[0].as_str().unwrap().to_string(), r[1].as_str().unwrap().to_string()))
        .collect()
}

#[tokio::test]
async fn an_admin_api_key_can_neither_unseal_nor_look_into_a_session() {
    let (t, span) = with_sealed_reply().await;
    let (_, made) = t.call(M::POST, "/api/admin/v1/keys", json!({ "name": "ci" })).await;
    let key = made["key"].as_str().unwrap().to_string();
    for (m, path, body) in [
        (M::POST, "/api/admin/v1/audit/session".to_string(), Some(json!({ "passphrase": PASS }))),
        (M::GET, "/api/admin/v1/audit/session".to_string(), None),
        (M::DELETE, "/api/admin/v1/audit/session".to_string(), None),
        (M::GET, format!("/api/admin/v1/audit/payloads/{span}"), None),
    ] {
        let mut r = t.http.request(m.clone(), format!("{}{path}", t.adm)).bearer_auth(&key);
        if let Some(b) = &body {
            r = r.json(b);
        }
        assert_eq!(r.send().await.unwrap().status(), 401, "{m} {path}");
    }
    // And a session opened by the person is not the key's to use.
    assert_eq!(unseal(&t).await.0, 201);
    let r = t.http.get(format!("{}/api/admin/v1/audit/payloads/{span}", t.adm)).bearer_auth(&key).send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn a_sign_in_older_than_ten_minutes_does_not_start_a_session() {
    let (t, _) = with_sealed_reply().await;
    t.admin.auth.backdate_session(&token(&t), 11 * 60);
    let (st, body) = unseal(&t).await;
    assert_eq!(st, 403);
    assert!(body["detail"].as_str().unwrap().contains("fresh sign-in"), "{body}");
    assert_eq!(t.call(M::GET, "/api/admin/v1/audit/session", json!(null)).await.1["open"], false);
    assert!(
        t.admin.db.lock().await.conn().query_row("SELECT COUNT(*) FROM audit_access", [], |r| r.get::<_, i64>(0)).unwrap() == 0,
        "no guess was made"
    );
}

#[tokio::test]
async fn seal_ends_the_session_and_the_keys_are_gone() {
    let (t, span) = with_sealed_reply().await;
    let (st, s) = unseal(&t).await;
    assert_eq!((st, s["open"].clone()), (201, json!(true)));
    assert!(s["idle_left_secs"].as_u64().unwrap() <= 900 && s["total_left_secs"].as_u64().unwrap() <= 3600);
    assert_eq!(view(&t, &span).await.1["payloads"][0]["text"], "sealed reply text");
    assert_eq!(t.call(M::DELETE, "/api/admin/v1/audit/session", json!(null)).await.0, 204);
    assert_eq!(view(&t, &span).await.0, 403, "sealed again");
    assert_eq!(t.call(M::GET, "/api/admin/v1/audit/session", json!(null)).await.1["open"], false);
    // Every step is on record in the auditor, each before it was done, and so is the view that was refused.
    let kinds: Vec<String> = records(&t).into_iter().map(|r| r.1).collect();
    assert_eq!(
        kinds,
        ["unseal:requested", "unseal:done", "view:requested", "view:done", "seal:requested", "seal:done", "view:requested", "view:refused"],
        "{kinds:?}"
    );
}

#[tokio::test]
async fn a_session_seals_itself_after_its_idle_time_or_its_total_time_and_the_end_is_recorded() {
    let (t, span) = with_sealed_reply().await;
    unseal(&t).await;
    // Fourteen minutes of nothing, then a view: still open, and the idle time starts again.
    t.admin.sealed.rewind(Duration::from_secs(14 * 60));
    assert_eq!(view(&t, &span).await.0, 200);
    t.admin.sealed.rewind(Duration::from_secs(14 * 60));
    assert_eq!(view(&t, &span).await.0, 200);
    // Sixteen idle minutes: sealed, and a view is refused.
    t.admin.sealed.rewind(Duration::from_secs(16 * 60));
    assert_eq!(view(&t, &span).await.0, 403);
    let ends: Vec<Value> = t
        .rig
        .rows("SELECT detail_json FROM admin_actions WHERE category = 'sealed_content' AND action = 'session_end'")
        .into_iter()
        .map(|r| serde_json::from_str(r[0].as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(ends.len(), 1);
    // (Rewinding moved both clocks, so the total ran out first, which is also a timeout.)
    assert!(["idle_timeout", "total_timeout"].contains(&ends[0]["reason"].as_str().unwrap()), "{ends:?}");

    // An hour in all, however it is used.
    unseal(&t).await;
    t.admin.sealed.rewind(Duration::from_secs(61 * 60));
    askbar_admin::sealed::sweep(&t.admin).await;
    assert_eq!(t.call(M::GET, "/api/admin/v1/audit/session", json!(null)).await.1["open"], false);
}

#[tokio::test]
async fn signing_out_seals_what_that_sign_in_unsealed_and_another_sign_in_does_not_see_it() {
    let (t, span) = with_sealed_reply().await;
    unseal(&t).await;
    // A second sign-in is another browser: it has no session.
    let r = t
        .http
        .post(format!("{}/api/admin/v1/session", t.adm))
        .header("x-askbar-admin", "1")
        .json(&json!({ "password": common::PW }))
        .send()
        .await
        .unwrap();
    let other = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    let r = t.http.get(format!("{}/api/admin/v1/audit/payloads/{span}", t.adm)).header("cookie", &other).send().await.unwrap();
    assert_eq!(r.status(), 403);
    // Signing the first one out ends the session and records why.
    let r = t
        .http
        .delete(format!("{}/api/admin/v1/session", t.adm))
        .header("x-askbar-admin", "1")
        .header("cookie", &t.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let status =
        t.http.get(format!("{}/api/admin/v1/status", t.adm)).header("cookie", &other).send().await.unwrap().json::<Value>().await.unwrap();
    assert_eq!(status["sealed_content"]["open"], false);
    let ends = t.rig.rows("SELECT detail_json FROM admin_actions WHERE action = 'session_end'");
    assert_eq!(ends.len(), 1);
    assert!(ends[0][0].as_str().unwrap().contains("sign_out"));
    // Even the browser session that survives cannot reach the keys: they are gone.
    let r = t.http.get(format!("{}/api/admin/v1/audit/payloads/{span}", t.adm)).header("cookie", &other).send().await.unwrap();
    assert_eq!(r.status(), 403);
}

#[tokio::test]
async fn an_unseal_and_a_change_of_the_timeouts_are_alerts_in_the_banner_status() {
    let (t, _) = with_sealed_reply().await;
    let status = || async { t.call(M::GET, "/api/admin/v1/status", json!(null)).await.1["sealed_content"].clone() };
    assert_eq!(status().await["alerts"], json!([]));
    unseal(&t).await;
    let s = status().await;
    assert_eq!(s["open"], true);
    assert_eq!(s["alerts"][0]["kind"], "unsealed");
    assert!(s["alerts"][0]["text"].as_str().unwrap().contains("15 minutes"));
    assert!(!s.to_string().contains(PASS));

    let disp = serde_json::to_value(askbar_proto::DisplaySettings::default()).unwrap();
    let save = |idle: u32, total: u32| json!({ "system_prompt": "x", "display": disp, "unseal_idle_minutes": idle, "unseal_total_minutes": total });
    assert_eq!(t.call(M::PUT, "/api/admin/v1/settings", save(5, 120)).await.0, 204);
    let s = status().await;
    assert_eq!(s["alerts"].as_array().unwrap().last().unwrap()["kind"], "timeouts_changed");
    assert!(s["alerts"][1]["text"].as_str().unwrap().contains("5 minutes without use, 120 in all (it was 15 and 60)"));
    // The new limits apply to the session that is open, and the change is on record with the numbers.
    assert!(s["idle_left_secs"].as_u64().unwrap() <= 300);
    let rows = t.rig.rows("SELECT detail_json FROM admin_actions WHERE category = 'settings' AND outcome = 'done'");
    assert!(rows.iter().any(|r| r[0].as_str().unwrap().contains("\"unseal_idle_minutes\":5")), "{rows:?}");
    // Saving the same values again is not a change, so not an alert.
    let before = status().await["alerts"].as_array().unwrap().len();
    assert_eq!(t.call(M::PUT, "/api/admin/v1/settings", save(5, 120)).await.0, 204);
    assert_eq!(status().await["alerts"].as_array().unwrap().len(), before);
}

#[tokio::test]
async fn the_timeouts_are_bounded_and_the_idle_time_cannot_exceed_the_total() {
    let (t, _) = with_sealed_reply().await;
    let disp = serde_json::to_value(askbar_proto::DisplaySettings::default()).unwrap();
    let save = |idle: u32, total: u32| json!({ "system_prompt": "x", "display": disp, "unseal_idle_minutes": idle, "unseal_total_minutes": total });
    for (idle, total, status) in [(1, 1, 204), (720, 720, 204), (0, 60, 400), (15, 721, 400), (721, 721, 400), (90, 60, 400)] {
        assert_eq!(t.call(M::PUT, "/api/admin/v1/settings", save(idle, total)).await.0, status, "{idle}/{total}");
    }
    let (_, s) = t.call(M::GET, "/api/admin/v1/settings", json!(null)).await;
    assert_eq!(
        (s["unseal_idle_minutes"].clone(), s["unseal_total_minutes"].clone()),
        (json!(720), json!(720)),
        "refused values changed nothing"
    );
}

#[tokio::test]
async fn an_unseal_and_a_view_are_refused_when_their_record_cannot_be_written() {
    use std::sync::atomic::{AtomicU16, Ordering};
    use std::sync::Arc;
    let (t, span) = with_sealed_reply().await;
    // The same admin, its record link in front of an auditor whose answer the test chooses.
    let status = Arc::new(AtomicU16::new(201));
    let s = status.clone();
    let stub = common::serve(axum::Router::new().route(
        "/api/audit/v1/admin-actions",
        axum::routing::post(move || {
            let s = s.clone();
            async move { axum::http::StatusCode::from_u16(s.load(Ordering::Relaxed)).unwrap() }
        }),
    ))
    .await;
    let server_token = askbar_admin::server_link::read_token(&t.dir.join("admin-token")).unwrap();
    let admin = askbar_admin::AdminState::build(
        askbar_admin::AdminDb::open_in_memory().unwrap(),
        Some(askbar_admin::server_link::ServerLink::new(&t.server_api_url, server_token)),
        askbar_admin::secrets::AdminSecrets::ephemeral(),
    );
    askbar_admin::web::init_password(&*admin.db.lock().await, common::PW).unwrap();
    admin
        .auditor
        .set(askbar_admin::auditor_link::AuditorLink::new(
            &t.rig.audit_url,
            askbar_admin::auditor_link::read_token(&t.rig.admin_tokens).unwrap(),
        ))
        .ok()
        .unwrap();
    admin.actions.set(askbar_admin::actions::ActionLog::new(&stub, "t".into())).ok().unwrap();
    let adm = common::serve(askbar_admin::router(admin.clone())).await;
    let http = reqwest::Client::new();
    let r = http
        .post(format!("{adm}/api/admin/v1/session"))
        .header("x-askbar-admin", "1")
        .json(&json!({ "password": common::PW }))
        .send()
        .await
        .unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    let call = |m: M, path: String, body: Option<Value>| {
        let (http, adm, cookie) = (http.clone(), adm.clone(), cookie.clone());
        async move {
            let mut r = http.request(m, format!("{adm}{path}")).header("x-askbar-admin", "1").header("cookie", cookie);
            if let Some(b) = body {
                r = r.json(&b);
            }
            r.send().await.unwrap().status().as_u16()
        }
    };
    // The admin needs an audit key of its own to unseal with.
    let key = json!({ "passphrase": PASS, "confirm": PASS });
    assert_eq!(call(M::PUT, "/api/admin/v1/setup/audit-key".into(), Some(key)).await, 200);

    status.store(503, Ordering::Relaxed);
    assert_eq!(call(M::POST, "/api/admin/v1/audit/session".into(), Some(json!({ "passphrase": PASS }))).await, 503, "no record, no unseal");
    assert!(!admin.sealed.belongs_to(&askbar_admin::auth::AdminAuth::hash_token(cookie.split_once('=').unwrap().1)));
    status.store(201, Ordering::Relaxed);
    assert_eq!(call(M::POST, "/api/admin/v1/audit/session".into(), Some(json!({ "passphrase": PASS }))).await, 201);
    status.store(503, Ordering::Relaxed);
    assert_eq!(call(M::GET, format!("/api/admin/v1/audit/payloads/{span}"), None).await, 503, "no record, no view");
    drop(t);
}
