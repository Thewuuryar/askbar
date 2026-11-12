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

/// A stand-in for any service on any admin link: answers every request with `status`.
async fn answering(status: Arc<Mutex<u16>>) -> String {
    let app = axum::Router::new().fallback(move || {
        let status = status.clone();
        async move { (StatusCode::from_u16(*status.lock().unwrap()).unwrap(), axum::Json(json!({}))) }
    });
    format!("http://{}", spawn(app).await)
}

#[tokio::test]
async fn a_token_that_is_refused_on_any_admin_link_is_one_alert_naming_that_link_for_each_episode() {
    use askbar_admin::{
        actions::ActionLog, alerter_link::AlerterLink, auditor_link::AuditorLink, gateway_link::GatewayLink, server_link::ServerLink,
    };
    let r = rig().await;
    let status = Arc::new(Mutex::new(401u16));
    let url = answering(status.clone()).await;
    let server = ServerLink::new(&url, "t".into());
    let gateway = GatewayLink::new(&url, "t".into());
    let alerter = AlerterLink::new(&url, "t".into());
    let auditor = AuditorLink::new(&url, "t".into());
    let actions = ActionLog::new(&url, "t".into());
    for n in [&server as &dyn Notified, &gateway, &alerter, &auditor, &actions] {
        n.attach(r.state.notifier.clone());
    }
    let record = askbar_proto::audit::AdminActionStart {
        id: "a".into(),
        at: 0,
        category: "settings".into(),
        action: "update".into(),
        outcome: "requested".into(),
        detail: None,
    };
    let touch = || async {
        server.check().await;
        gateway.status_now().await;
        alerter.status_now().await;
        auditor.get("/status").await;
        let _ = actions.record(&record).await;
    };
    // Three refusals on every link: one alert for each link.
    for _ in 0..3 {
        touch().await;
    }
    let events = r.events(5).await;
    assert_admin_events(&events);
    let mut links: Vec<String> = events.iter().map(|e| e["fields"]["link"].as_str().unwrap().to_string()).collect();
    links.sort();
    assert_eq!(links, ["admin-alerter", "admin-auditor-append", "admin-auditor-read", "admin-model-gateway", "admin-server"]);
    assert!(events.iter().all(|e| e["kind"] == "rejected_token" && e["severity"] == "warning"));
    assert_eq!(r.notes().await.len(), 5);
    // The token is accepted again, and refused again later: a new episode, a new alert (on each link).
    *status.lock().unwrap() = 200;
    touch().await;
    *status.lock().unwrap() = 401;
    touch().await;
    assert_eq!(r.events(10).await.len(), 10);
}

/// Each link takes the notifier the same way.
trait Notified {
    fn attach(&self, n: Arc<askbar_admin::notify::Notifier>);
}
macro_rules! notified {
    ($($t:ty),*) => {$(impl Notified for Arc<$t> { fn attach(&self, n: Arc<askbar_admin::notify::Notifier>) { self.set_notifier(n) } })*};
}
notified!(
    askbar_admin::server_link::ServerLink,
    askbar_admin::gateway_link::GatewayLink,
    askbar_admin::alerter_link::AlerterLink,
    askbar_admin::auditor_link::AuditorLink,
    askbar_admin::actions::ActionLog
);

/// A stand-in whose `GET /status` reports the boot ID in `boot` on every link a service's status has (ADR 0028).
async fn reporting(boot: Arc<Mutex<String>>) -> String {
    let app = axum::Router::new().fallback(move || {
        let boot = boot.clone();
        async move {
            axum::Json(json!({
                "config_version": 1, "boot_id": *boot.lock().unwrap(),
                "last_delivery_ok_at": null, "last_delivery_failed_at": null, "queued": 0, "dropped": 0
            }))
        }
    });
    format!("http://{}", spawn(app).await)
}

#[tokio::test]
async fn three_restarts_in_ten_minutes_of_a_polled_service_are_one_restart_loop_alert_for_it() {
    use askbar_admin::{alerter_link::AlerterLink, gateway_link::GatewayLink, server_link::ServerLink};
    let r = rig().await;
    let boot = Arc::new(Mutex::new("boot-0".to_string()));
    let url = reporting(boot.clone()).await;
    let server = ServerLink::new(&url, "t".into());
    let gateway = GatewayLink::new(&url, "t".into());
    let alerter = AlerterLink::new(&url, "t".into());
    for n in [&server as &dyn Notified, &gateway, &alerter] {
        n.attach(r.state.notifier.clone());
    }
    let poll = || async {
        server.check().await;
        gateway.status_now().await;
        alerter.status_now().await;
    };
    poll().await;
    // Two restarts are not a loop. The same boot ID polled again is not a restart.
    for id in ["boot-1", "boot-1", "boot-2"] {
        *boot.lock().unwrap() = id.into();
        poll().await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(r.seen.lock().unwrap().is_empty(), "two restarts are not a loop");
    *boot.lock().unwrap() = "boot-3".into();
    poll().await;
    let events = r.events(3).await;
    assert_admin_events(&events);
    let mut services: Vec<&str> = events.iter().map(|e| e["fields"]["service"].as_str().unwrap()).collect();
    services.sort();
    assert_eq!(services, ["alerter", "model-gateway", "server"]);
    assert!(events.iter().all(|e| e["kind"] == "restart_loop" && e["fields"]["count"] == 3 && e["fields"]["minutes"] == 10), "{events:?}");
    // The loop going on is not another alert.
    *boot.lock().unwrap() = "boot-4".into();
    poll().await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(r.seen.lock().unwrap().len(), 3);
}

// ------------------------------------------------------------------ the budget

#[derive(Default)]
struct Counts {
    /// What `GET /token-counts?since=` answers for the day and for the month: `(provider, tokens_in, tokens_out)` rows.
    day: Mutex<Vec<(String, i64, i64)>>,
    month: Mutex<Vec<(String, i64, i64)>>,
}

const DAY_START: i64 = 1_793_232_000; // 2026-10-29 00:00 UTC
const NOW: i64 = DAY_START + 8 * 3600;

async fn auditor_counts(c: Arc<Counts>) -> String {
    let app = axum::Router::new().route(
        "/api/audit/v1/token-counts",
        axum::routing::get(move |axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, i64>>| {
            let c = c.clone();
            async move {
                let rows = if q["since"] >= DAY_START { c.day.lock().unwrap().clone() } else { c.month.lock().unwrap().clone() };
                axum::Json(
                    rows.into_iter()
                        .map(|(p, i, o)| json!({ "provider_id": p, "model": "m", "requests": 1, "tokens_in": i, "tokens_out": o, "tokens_cached": 0 }))
                        .collect::<Vec<_>>(),
                )
            }
        }),
    );
    format!("http://{}", spawn(app).await)
}

#[tokio::test]
async fn a_budget_at_eighty_and_at_a_hundred_percent_is_one_alert_each_for_the_period_and_a_subscription_is_not_counted() {
    use askbar_admin::auditor_link::AuditorLink;
    let r = rig().await;
    let counts = Arc::new(Counts::default());
    let link = AuditorLink::new(&auditor_counts(counts.clone()).await, "t".into());
    {
        let db = r.state.db.lock().await;
        db.conn().execute("UPDATE budgets SET daily_tokens = 1000, monthly_tokens = 100000 WHERE id = 1", []).unwrap();
        db.conn()
            .execute(
                "INSERT INTO providers (id, name, type, base_url, model, created_at, billing) VALUES ('sub', 'Subscription', 'openai', 'http://x', 'm', 0, 'subscription')",
                [],
            )
            .unwrap();
    }
    let check = |now: i64| {
        let (state, link) = (r.state.clone(), link.clone());
        async move { state.budget.check_at(&state, &link, now).await }
    };
    // Under 80%: nothing. A subscription's tokens are not counted, however many.
    *counts.day.lock().unwrap() = vec![("p".into(), 300, 400), ("sub".into(), 9_000_000, 9_000_000)];
    *counts.month.lock().unwrap() = vec![("p".into(), 300, 400), ("sub".into(), 9_000_000, 9_000_000)];
    check(NOW).await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert!(r.seen.lock().unwrap().is_empty());
    // 80% of the day: one warning, and not again at the next check.
    *counts.day.lock().unwrap() = vec![("p".into(), 400, 400)];
    check(NOW).await;
    check(NOW + 60).await;
    let events = r.events(1).await;
    assert_eq!(events[0]["kind"], "budget_threshold");
    assert_eq!((events[0]["severity"].clone(), events[0]["fields"].clone()), (json!("warning"), json!({ "period": "day", "percent": 80 })));
    // 100%: one critical.
    *counts.day.lock().unwrap() = vec![("p".into(), 700, 400)];
    check(NOW + 120).await;
    check(NOW + 180).await;
    let events = r.events(2).await;
    assert_eq!(
        (events[1]["severity"].clone(), events[1]["fields"].clone()),
        (json!("critical"), json!({ "period": "day", "percent": 100 }))
    );
    assert_admin_events(&events);
    // The next day starts at nothing, so nothing is sent; use over the limit again that day is a new alert, once.
    *counts.day.lock().unwrap() = vec![];
    check(NOW + 86_400).await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(r.seen.lock().unwrap().len(), 2);
    *counts.day.lock().unwrap() = vec![("p".into(), 700, 400)];
    check(NOW + 86_400 + 60).await;
    check(NOW + 86_400 + 120).await;
    let events = r.events(3).await;
    assert_eq!(events[2]["fields"], json!({ "period": "day", "percent": 100 }));
    // The month is watched the same way: 80% of 100000 is a warning for the month.
    *counts.month.lock().unwrap() = vec![("p".into(), 50_000, 31_000)];
    check(NOW + 86_400 + 180).await;
    let events = r.events(4).await;
    assert_eq!(
        (events[3]["severity"].clone(), events[3]["fields"].clone()),
        (json!("warning"), json!({ "period": "month", "percent": 80 }))
    );
}
