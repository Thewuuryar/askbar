#![cfg(unix)]
//! The events the admin sends the alerter itself (ADR 0055, plan step 4.1): each is a log line, an event with typed fields only, and a
//! note on the pages. The alerter here is a stand-in that notes what it is sent.

use askbar_admin::{web, AdminDb, AdminState};
use askbar_proto::alerter::{Event, Fields, Kind, Sender, Severity};
use axum::http::StatusCode;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const PASSWORD: &str = "correct horse battery";

/// What the stand-in alerter was sent: `(authorization, body)` for each `POST /events`.
type Seen = Arc<Mutex<Vec<(String, Value)>>>;

async fn spawn(app: axum::Router) -> SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr
}

async fn alerter(seen: Seen) -> String {
    let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
        let seen = seen.clone();
        async move {
            let auth = req.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            let body = axum::body::to_bytes(req.into_body(), 1 << 16).await.unwrap();
            seen.lock().unwrap().push((auth, serde_json::from_slice(&body).unwrap_or_default()));
            (StatusCode::ACCEPTED, axum::Json(json!({ "suppressed": false })))
        }
    });
    format!("http://{}", spawn(app).await)
}

pub struct R {
    pub state: Arc<AdminState>,
    pub seen: Seen,
    pub admin: SocketAddr,
    pub dir: PathBuf,
}

impl Drop for R {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// An admin whose notifier talks to a stand-in alerter, with a token for the link.
pub async fn rig() -> R {
    let dir = std::env::temp_dir().join(format!("askbar-alertevents-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(dir.join("call")).unwrap();
    std::fs::write(dir.join("call").join("admin-alerter.token"), "admalr_test\n").unwrap();
    let seen = Seen::default();
    let url = alerter(seen.clone()).await;
    let db = AdminDb::open_in_memory().unwrap();
    web::init_password(&db, PASSWORD).unwrap();
    let state = AdminState::new(db);
    let client = askbar_service::alert_client::AlertClient::new(&url, dir.clone(), "admin-alerter");
    state.notifier.set_client(client.clone());
    tokio::spawn(client.drain_forever());
    let admin = spawn(askbar_admin::router(state.clone())).await;
    R { state, seen, admin, dir }
}

impl R {
    /// Wait until the alerter has been sent `n` events, and return them.
    pub async fn events(&self, n: usize) -> Vec<Value> {
        for _ in 0..200 {
            if self.seen.lock().unwrap().len() >= n {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let seen = self.seen.lock().unwrap();
        assert_eq!(seen.len(), n, "{seen:?}");
        assert!(seen.iter().all(|(auth, _)| auth == "Bearer admalr_test"), "the link's own token");
        seen.iter().map(|(_, b)| b.clone()).collect()
    }

    /// The notes the pages show, from `GET /status`.
    pub async fn notes(&self) -> Vec<Value> {
        let http = reqwest::Client::new();
        let r = http
            .post(format!("http://{}/api/admin/v1/session", self.admin))
            .header("x-askbar-admin", "1")
            .json(&json!({ "password": PASSWORD }))
            .send()
            .await
            .unwrap();
        let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
        let s: Value = http
            .get(format!("http://{}/api/admin/v1/status", self.admin))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        s["sealed_content"]["alerts"].as_array().unwrap().clone()
    }
}

/// Every event in `events` is one the alerter takes from the admin's link.
pub fn assert_admin_events(events: &[Value]) {
    for e in events {
        let e: Event = serde_json::from_value(e.clone()).unwrap_or_else(|err| panic!("{e}: {err}"));
        assert_eq!(e.check(Sender::Admin), Ok(()), "{e:?}");
    }
}

#[tokio::test]
async fn an_alert_is_an_event_with_typed_fields_and_a_note_on_the_pages() {
    let r = rig().await;
    r.state.notifier.notify(
        Kind::RejectedToken,
        Severity::Warning,
        Fields { link: Some("admin-model-gateway".into()), ..Fields::default() },
    );
    let events = r.events(1).await;
    assert_eq!(events[0]["kind"], "rejected_token");
    assert_eq!(events[0]["severity"], "warning");
    assert_eq!(events[0]["fields"], json!({ "link": "admin-model-gateway" }));
    assert_admin_events(&events);
    let notes = r.notes().await;
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["kind"], "rejected_token");
    assert!(notes[0]["text"].as_str().unwrap().contains("admin-model-gateway"));
}
