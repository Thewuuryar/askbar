//! The admin and the alerter (ADR 0055, ADR 0028): the admin keeps the delivery settings, encrypted, and pushes them in `PUT /config`;
//! it pushes again whenever the alerter reports no configuration or another one; a change goes to the alerter first and is saved only if
//! the alerter takes it; and no topic, token or server address is returned, logged or kept in the clear.

mod common;
use askbar_admin::alerter_link::{AlerterLink, Sync};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use common::{serve, T};
use reqwest::Method as M;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const TOPIC: &str = "marker-topic-8d3f";
const TOKEN: &str = "marker-token-52ab";
const SERVER: &str = "https://ntfy.marker-host.example";
const ALERTER_TOKEN: &str = "admalr_test_token";

/// A stand-in for the alerter's admin listener: it records each configuration it is given, reports the version it runs (none until
/// one is given), and takes only its own token.
#[derive(Default)]
struct FakeAlerter {
    running: Mutex<Option<u64>>,
    pushes: Mutex<Vec<Value>>,
    /// The status the next `PUT /config` is answered with (200 by default).
    answer: Mutex<Option<u16>>,
    boot: Mutex<u32>,
    last_ok: Mutex<Option<i64>>,
    last_failed: Mutex<Option<i64>>,
    /// What `POST /test` answers.
    test: Mutex<Option<Value>>,
}

type Fake = Arc<FakeAlerter>;

fn authorized(h: &HeaderMap) -> bool {
    h.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {ALERTER_TOKEN}"))
}

async fn fake_status(State(f): State<Fake>, h: HeaderMap) -> Result<Json<Value>, StatusCode> {
    if !authorized(&h) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(Json(json!({
        "config_version": *f.running.lock().unwrap(),
        "boot_id": format!("boot-{}", f.boot.lock().unwrap()),
        "last_delivery_ok_at": *f.last_ok.lock().unwrap(),
        "last_delivery_failed_at": *f.last_failed.lock().unwrap(),
        "queued": 0,
        "dropped": 0
    })))
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
    if let Some(code) = *f.answer.lock().unwrap() {
        return Err(StatusCode::from_u16(code).unwrap());
    }
    *f.running.lock().unwrap() = body["version"].as_u64();
    f.pushes.lock().unwrap().push(body.clone());
    Ok(Json(json!({ "version": body["version"] })))
}

async fn fake_test(State(f): State<Fake>, h: HeaderMap) -> Result<Json<Value>, StatusCode> {
    if !authorized(&h) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if f.running.lock().unwrap().is_none() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(Json(f.test.lock().unwrap().clone().unwrap_or_else(|| json!({ "results": [{ "channel": "ntfy", "ok": true }] }))))
}

async fn rig() -> (T, Fake, Arc<AlerterLink>) {
    let t = common::start_with(true, false).await;
    let fake = Fake::default();
    let app = Router::new()
        .route("/api/alerter/v1/status", get(fake_status))
        .route("/api/alerter/v1/config", put(fake_config))
        .route("/api/alerter/v1/restart", post(fake_restart))
        .route("/api/alerter/v1/test", post(fake_test))
        .with_state(fake.clone());
    let url = serve(app).await;
    let link = AlerterLink::new(&url, ALERTER_TOKEN.into());
    t.admin.alerter.set(link.clone()).ok().unwrap();
    (t, fake, link)
}

fn settings() -> Value {
    json!({ "ntfy": { "server": SERVER, "topic": TOPIC, "token": TOKEN } })
}

#[tokio::test]
async fn settings_go_to_the_alerter_first_then_are_saved_encrypted_and_no_answer_returns_them() {
    let (t, fake, _link) = rig().await;
    let (st, view) = t.call(M::PUT, "/api/admin/v1/alerts", settings()).await;
    assert_eq!(st, 200, "{view}");
    assert_eq!(view["ntfy"], json!({ "server": true, "topic": true, "token": true }));
    // The alerter was given exactly what was entered.
    let pushed = fake.pushes.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        (pushed["ntfy"]["server"].as_str(), pushed["ntfy"]["topic"].as_str(), pushed["ntfy"]["token"].as_str()),
        (Some(SERVER), Some(TOPIC), Some(TOKEN))
    );
    // Kept in admin.db, but not in the clear.
    {
        let db = t.admin.db.lock().await;
        let (a, b, c): (Vec<u8>, Vec<u8>, Vec<u8>) = db
            .conn()
            .query_row("SELECT ntfy_server_enc, ntfy_topic_enc, ntfy_token_enc FROM alert_settings", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        for (blob, plain) in [(&a, SERVER), (&b, TOPIC), (&c, TOKEN)] {
            assert!(!blob.windows(plain.len()).any(|w| w == plain.as_bytes()), "{plain} is in the clear");
            assert_eq!(t.admin.secrets.decrypt(blob).as_deref(), Some(plain));
        }
    }
    // No answer of the admin holds any of it.
    for path in ["/api/admin/v1/alerts", "/api/admin/v1/services", "/api/admin/v1/status", "/api/admin/v1/settings"] {
        let (st, body) = t.call(M::GET, path, json!(null)).await;
        assert_eq!(st, 200, "{path}");
        let text = body.to_string();
        assert!(!text.contains(TOPIC) && !text.contains(TOKEN) && !text.contains("marker-host"), "{path} returned a secret: {text}");
    }
}

#[tokio::test]
async fn a_token_that_is_left_out_is_kept_an_empty_one_removes_it_and_null_removes_the_channel() {
    let (t, fake, _link) = rig().await;
    t.call(M::PUT, "/api/admin/v1/alerts", settings()).await;
    // Change the topic and say nothing about the token: it stays.
    let (st, view) = t.call(M::PUT, "/api/admin/v1/alerts", json!({ "ntfy": { "server": SERVER, "topic": "another-topic" } })).await;
    assert_eq!((st, view["ntfy"]["token"].clone()), (200, json!(true)));
    let pushed = fake.pushes.lock().unwrap().last().cloned().unwrap();
    assert_eq!((pushed["ntfy"]["topic"].as_str(), pushed["ntfy"]["token"].as_str()), (Some("another-topic"), Some(TOKEN)));
    // An empty token removes it.
    let (_, view) = t.call(M::PUT, "/api/admin/v1/alerts", json!({ "ntfy": { "server": SERVER, "topic": TOPIC, "token": "" } })).await;
    assert_eq!(view["ntfy"], json!({ "server": true, "topic": true, "token": false }));
    assert!(fake.pushes.lock().unwrap().last().unwrap()["ntfy"].get("token").is_none());
    // Null removes the channel.
    let (_, view) = t.call(M::PUT, "/api/admin/v1/alerts", json!({ "ntfy": null })).await;
    assert_eq!(view["ntfy"], json!({ "server": false, "topic": false, "token": false }));
    assert!(fake.pushes.lock().unwrap().last().unwrap().get("ntfy").is_none());
}

#[tokio::test]
async fn settings_the_alerter_refuses_or_cannot_be_asked_about_are_not_saved_and_the_answer_names_no_value() {
    let (t, fake, _link) = rig().await;
    *fake.answer.lock().unwrap() = Some(400);
    let (st, body) = t.call(M::PUT, "/api/admin/v1/alerts", settings()).await;
    assert_eq!((st, body["error"].as_str()), (400, Some("bad_request")), "{body}");
    assert!(!body.to_string().contains(TOPIC) && !body.to_string().contains(TOKEN), "{body}");
    let (_, view) = t.call(M::GET, "/api/admin/v1/alerts", json!(null)).await;
    assert_eq!(view["ntfy"]["server"], false, "nothing was saved");
    // An alerter that fails some other way: 502, nothing saved.
    *fake.answer.lock().unwrap() = Some(500);
    let (st, _) = t.call(M::PUT, "/api/admin/v1/alerts", settings()).await;
    assert_eq!(st, 502);
    // One that does not answer at all.
    let t2 = common::start_with(true, false).await;
    t2.admin.alerter.set(AlerterLink::new("http://127.0.0.1:9", ALERTER_TOKEN.into())).ok().unwrap();
    let (st, body) = t2.call(M::PUT, "/api/admin/v1/alerts", settings()).await;
    assert_eq!(st, 502, "{body}");
    assert!(body["detail"].as_str().unwrap().contains("Nothing was changed"), "{body}");
    assert_eq!(t2.call(M::GET, "/api/admin/v1/alerts", json!(null)).await.1["ntfy"]["server"], false);
    // Fields a settings body does not have, or values that make no sense, are 400s.
    for bad in [
        json!({ "ntfy": { "server": "", "topic": "t" } }),
        json!({ "ntfy": { "server": SERVER, "topic": "" } }),
        json!({ "smtp": {} }),
        json!({ "ntfy": { "server": SERVER, "topic": "t", "extra": 1 } }),
    ] {
        let (st, _) = t.call(M::PUT, "/api/admin/v1/alerts", bad.clone()).await;
        assert!(st == 400 || st == 422, "{bad} gave {st}");
    }
}

#[tokio::test]
async fn it_pushes_again_when_the_alerter_has_none_and_not_while_it_runs_the_current_one() {
    let (t, fake, link) = rig().await;
    // An alerter with no channel is given a configuration all the same, so that it answers.
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    assert_eq!(link.sync(&t.admin).await, Sync::Current);
    assert_eq!(fake.pushes.lock().unwrap().len(), 1);
    assert!(fake.pushes.lock().unwrap()[0].get("ntfy").is_none());
    t.call(M::PUT, "/api/admin/v1/alerts", settings()).await;
    let before = fake.pushes.lock().unwrap().len();
    assert_eq!(link.sync(&t.admin).await, Sync::Current, "the change was pushed when it was made");
    assert_eq!(fake.pushes.lock().unwrap().len(), before);
    // A restart: the new process has no configuration (its secrets were in memory).
    *fake.running.lock().unwrap() = None;
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    let pushes = fake.pushes.lock().unwrap().clone();
    assert_eq!(pushes[pushes.len() - 1], pushes[pushes.len() - 2], "the same settings, secrets and all");
}

#[tokio::test]
async fn the_test_alert_returns_what_the_alerter_says_and_a_refusal_is_a_sentence() {
    let (t, fake, _link) = rig().await;
    t.call(M::PUT, "/api/admin/v1/alerts", settings()).await;
    let (st, body) = t.call(M::POST, "/api/admin/v1/alerts/test", json!(null)).await;
    assert_eq!((st, body["results"][0]["ok"].clone()), (200, json!(true)), "{body}");
    *fake.test.lock().unwrap() = Some(json!({ "results": [{ "channel": "ntfy", "ok": false, "problem": "unauthorized" }] }));
    let (_, body) = t.call(M::POST, "/api/admin/v1/alerts/test", json!(null)).await;
    assert_eq!((body["results"][0]["ok"].clone(), body["results"][0]["problem"].as_str()), (json!(false), Some("unauthorized")));
    // An alerter that does not answer is a 502 with a sentence.
    let t2 = common::start_with(true, false).await;
    t2.admin.alerter.set(AlerterLink::new("http://127.0.0.1:9", ALERTER_TOKEN.into())).ok().unwrap();
    let (st, body) = t2.call(M::POST, "/api/admin/v1/alerts/test", json!(null)).await;
    assert_eq!((st, body["error"].as_str()), (502, Some("server_unavailable")));
}

#[tokio::test]
async fn the_banner_says_not_set_up_then_ok_then_failing_after_half_an_hour() {
    let (t, fake, link) = rig().await;
    let alerts = |t: &T| {
        let t = t.admin.clone();
        async move { askbar_admin::alerts::view(&t).await.banner }
    };
    link.sync(&t.admin).await;
    assert_eq!(alerts(&t).await, "not_set_up", "no channel is set up");
    let (_, st) = t.call(M::GET, "/api/admin/v1/status", json!(null)).await;
    assert_eq!(st["alerts"], "not_set_up", "the status carries it for the banner");
    t.call(M::PUT, "/api/admin/v1/alerts", settings()).await;
    link.sync(&t.admin).await;
    assert_eq!(alerts(&t).await, "ok");
    // Delivery has been failing since the last time it worked, an hour ago.
    let now = askbar_admin::auth::now();
    *fake.last_ok.lock().unwrap() = Some(now - 3600);
    *fake.last_failed.lock().unwrap() = Some(now - 5);
    link.sync(&t.admin).await;
    assert_eq!(alerts(&t).await, "failing");
    // It works again.
    *fake.last_ok.lock().unwrap() = Some(now);
    link.sync(&t.admin).await;
    assert_eq!(alerts(&t).await, "ok");
}

#[tokio::test]
async fn restarting_the_alerter_from_the_services_page_gives_it_its_settings_again() {
    let (t, fake, _link) = rig().await;
    t.call(M::PUT, "/api/admin/v1/alerts", settings()).await;
    let (st, list) = t.call(M::GET, "/api/admin/v1/services", json!(null)).await;
    assert_eq!(st, 200);
    let names: Vec<&str> = list["services"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"alerter"), "{names:?}");
    let card = list["services"].as_array().unwrap().iter().find(|s| s["name"] == "alerter").unwrap().clone();
    assert_eq!(card["state"], "running", "{card}");
    let before = fake.pushes.lock().unwrap().len();
    let (st, body) = t.call(M::POST, "/api/admin/v1/services/alerter/restart", json!(null)).await;
    assert_eq!(st, 200, "{body}");
    assert!(body["status"].as_str().unwrap().contains("with its settings"), "{body}");
    assert!(fake.pushes.lock().unwrap().len() > before, "the settings were pushed to the new process");
    assert_eq!(*fake.boot.lock().unwrap(), 1);
}

const MAIL_HOST: &str = "smtp.marker-host.example";
const MAIL_USER: &str = "marker-mail-user-3e1c";
const MAIL_PASSWORD: &str = "marker-mail-password-77d0";
const MAIL_TO: &str = "marker-owner@marker-host.example";

fn email() -> Value {
    json!({ "email": { "host": MAIL_HOST, "port": 587, "tls": "starttls", "user": MAIL_USER, "password": MAIL_PASSWORD,
                       "from": "askbar@marker-host.example", "to": MAIL_TO } })
}

#[tokio::test]
async fn email_settings_go_to_the_alerter_are_saved_encrypted_and_no_answer_returns_them() {
    let (t, fake, _link) = rig().await;
    let (st, view) = t.call(M::PUT, "/api/admin/v1/alerts", email()).await;
    assert_eq!(st, 200, "{view}");
    // Booleans for the secrets, and the two settings that are not secret.
    assert_eq!(
        view["email"],
        json!({ "host": true, "port": 587, "tls": "starttls", "user": true, "password": true, "from": true, "to": true })
    );
    let pushed = fake.pushes.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        (pushed["email"]["host"].as_str(), pushed["email"]["password"].as_str(), pushed["email"]["to"].as_str()),
        (Some(MAIL_HOST), Some(MAIL_PASSWORD), Some(MAIL_TO))
    );
    {
        let db = t.admin.db.lock().await;
        let row: Vec<Vec<u8>> = db
            .conn()
            .query_row("SELECT host_enc, user_enc, password_enc, from_enc, to_enc FROM alert_email_settings", [], |r| {
                Ok((0..5).map(|i| r.get::<_, Vec<u8>>(i).unwrap()).collect())
            })
            .unwrap();
        for (blob, plain) in row.iter().zip([MAIL_HOST, MAIL_USER, MAIL_PASSWORD, "askbar@marker-host.example", MAIL_TO]) {
            assert!(!blob.windows(plain.len()).any(|w| w == plain.as_bytes()), "{plain} is in the clear");
            assert_eq!(t.admin.secrets.decrypt(blob).as_deref(), Some(plain));
        }
    }
    for path in ["/api/admin/v1/alerts", "/api/admin/v1/services", "/api/admin/v1/status", "/api/admin/v1/settings"] {
        let text = t.call(M::GET, path, json!(null)).await.1.to_string();
        for m in [MAIL_HOST, MAIL_USER, MAIL_PASSWORD, MAIL_TO] {
            assert!(!text.contains(m), "{path} returned a secret: {text}");
        }
    }
    // The banner counts email as a channel.
    let (_, status) = t.call(M::GET, "/api/admin/v1/status", json!(null)).await;
    assert_ne!(status["alerts"], "not_set_up");
}

#[tokio::test]
async fn a_channel_left_out_is_kept_a_login_left_out_is_kept_and_null_removes_email() {
    let (t, fake, _link) = rig().await;
    t.call(M::PUT, "/api/admin/v1/alerts", settings()).await;
    t.call(M::PUT, "/api/admin/v1/alerts", email()).await;
    // Saving email did not touch ntfy.
    assert_eq!(fake.pushes.lock().unwrap().last().unwrap()["ntfy"]["topic"], TOPIC);
    // Change the port and say nothing about the login: it stays.
    let mut change = email();
    change["email"]["port"] = json!(465);
    change["email"]["tls"] = json!("tls");
    change["email"].as_object_mut().unwrap().remove("user");
    change["email"].as_object_mut().unwrap().remove("password");
    let (st, view) = t.call(M::PUT, "/api/admin/v1/alerts", change).await;
    assert_eq!((st, view["email"]["port"].clone(), view["email"]["password"].clone()), (200, json!(465), json!(true)), "{view}");
    let pushed = fake.pushes.lock().unwrap().last().cloned().unwrap();
    assert_eq!((pushed["email"]["user"].as_str(), pushed["email"]["password"].as_str()), (Some(MAIL_USER), Some(MAIL_PASSWORD)));
    // An empty user and password remove the login.
    let mut none = email();
    none["email"]["user"] = json!("");
    none["email"]["password"] = json!("");
    let (_, view) = t.call(M::PUT, "/api/admin/v1/alerts", none).await;
    assert_eq!((view["email"]["user"].clone(), view["email"]["password"].clone()), (json!(false), json!(false)));
    // ntfy alone, with no mention of email, leaves email as it is.
    t.call(M::PUT, "/api/admin/v1/alerts", json!({ "ntfy": { "server": SERVER, "topic": "t2" } })).await;
    assert_eq!(t.call(M::GET, "/api/admin/v1/alerts", json!(null)).await.1["email"]["host"], true);
    // Null removes email, and only email.
    let (_, view) = t.call(M::PUT, "/api/admin/v1/alerts", json!({ "email": null })).await;
    assert_eq!((view["email"]["host"].clone(), view["ntfy"]["server"].clone()), (json!(false), json!(true)));
    assert!(fake.pushes.lock().unwrap().last().unwrap().get("email").is_none());
    // An unknown TLS mode, a missing address and a port out of range are refused.
    for (k, v) in [("tls", json!("ssl")), ("to", json!("")), ("port", json!(70000)), ("extra", json!(1))] {
        let mut bad = email();
        bad["email"][k] = v;
        let (st, _) = t.call(M::PUT, "/api/admin/v1/alerts", bad).await;
        assert!(st == 400 || st == 422, "{k} gave {st}");
    }
}

#[tokio::test]
async fn email_the_alerter_refuses_is_not_saved_and_the_answer_names_no_value() {
    let (t, fake, _link) = rig().await;
    *fake.answer.lock().unwrap() = Some(400);
    let (st, body) = t.call(M::PUT, "/api/admin/v1/alerts", email()).await;
    assert_eq!((st, body["error"].as_str()), (400, Some("bad_request")), "{body}");
    for m in [MAIL_HOST, MAIL_USER, MAIL_PASSWORD, MAIL_TO] {
        assert!(!body.to_string().contains(m), "{body}");
    }
    assert_eq!(t.call(M::GET, "/api/admin/v1/alerts", json!(null)).await.1["email"]["host"], false, "nothing was saved");
}

#[tokio::test]
async fn the_test_alert_names_the_email_channel_too() {
    let (t, fake, _link) = rig().await;
    t.call(M::PUT, "/api/admin/v1/alerts", email()).await;
    *fake.test.lock().unwrap() = Some(json!({ "results": [{ "channel": "email", "ok": false, "problem": "tls" }] }));
    let (st, body) = t.call(M::POST, "/api/admin/v1/alerts/test", json!(null)).await;
    assert_eq!((st, body["results"][0]["channel"].clone(), body["results"][0]["problem"].clone()), (200, json!("email"), json!("tls")));
}
