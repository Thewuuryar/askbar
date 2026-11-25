//! The admin and the egress proxy (ADR 0056): the admin derives the allowlists from the providers, ntfy and SMTP settings and a fixed
//! `cpa` list, pushes them when the proxy is unconfigured or runs another version, records each push without a host, does not push
//! when it cannot record, and puts no token, key or topic in a push.

mod common;
use askbar_admin::egress_link::{EgressLink, Sync};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, put};
use axum::{Json, Router};
use common::{serve, T};
use reqwest::Method as M;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const TOKEN: &str = "admegr_test_token";
const TOPIC: &str = "marker-topic-8d3f";
const NTFY_TOKEN: &str = "marker-token-52ab";
const KEY: &str = "sk-marker-key-77";

/// A stand-in for the proxy's admin listener: it keeps the config it is given and reports its version (none until one is given).
#[derive(Default)]
struct FakeProxy {
    running: Mutex<Option<u64>>,
    pushes: Mutex<Vec<Value>>,
    denied: Mutex<Vec<Value>>,
    /// Extra source names `GET /status` reports (a proxy that has the `searxng` and `fetch` listeners).
    listeners: Mutex<Vec<&'static str>>,
}
type Fake = Arc<FakeProxy>;

fn authorized(h: &HeaderMap) -> bool {
    h.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {TOKEN}"))
}

async fn status(State(f): State<Fake>, h: HeaderMap) -> Result<Json<Value>, StatusCode> {
    if !authorized(&h) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let v = *f.running.lock().unwrap();
    let sources: serde_json::Map<String, Value> =
        f.listeners.lock().unwrap().iter().map(|s| (s.to_string(), json!({ "allowed": 0, "denied": {} }))).collect();
    Ok(Json(
        json!({ "configured": v.is_some(), "version": v, "boot_id": "b", "open_tunnels": 0, "sources": sources, "recent_denied": *f.denied.lock().unwrap() }),
    ))
}

async fn config(State(f): State<Fake>, h: HeaderMap, Json(body): Json<Value>) -> Result<Json<Value>, StatusCode> {
    if !authorized(&h) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let cfg: askbar_proto::egress::EgressConfig = serde_json::from_value(body.clone()).map_err(|_| StatusCode::BAD_REQUEST)?;
    *f.running.lock().unwrap() = Some(cfg.version());
    f.pushes.lock().unwrap().push(body);
    Ok(Json(json!({})))
}

async fn rig() -> (T, Fake, Arc<EgressLink>) {
    let t = common::start(true).await;
    let fake = Fake::default();
    let url = serve(
        Router::new()
            .route("/api/egress-proxy/v1/status", get(status))
            .route("/api/egress-proxy/v1/config", put(config))
            .with_state(fake.clone()),
    )
    .await;
    let link = EgressLink::new(&url, TOKEN.into());
    t.admin.egress.set(link.clone()).ok().unwrap();
    (t, fake, link)
}

fn hosts(push: &Value, source: &str) -> Vec<(String, u64, bool, bool)> {
    push["sources"][source]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["host"].as_str().unwrap().to_string(),
                e["port"].as_u64().unwrap_or(443),
                e["plain_http"].as_bool().unwrap_or(false),
                e["relay"].as_bool().unwrap_or(false),
            )
        })
        .collect()
}

async fn add_provider(t: &T, base_url: &str) {
    let (st, body) = t
        .call(
            M::POST,
            "/api/admin/v1/providers",
            json!({ "name": base_url, "type": "openai", "base_url": base_url, "model": "m", "api_key": KEY }),
        )
        .await;
    assert_eq!(st, 201, "{body}");
}

#[tokio::test]
async fn the_allowlists_come_from_providers_ntfy_and_smtp_and_a_change_or_a_restart_pushes_again() {
    let (t, fake, link) = rig().await;
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    let first = fake.pushes.lock().unwrap().last().cloned().unwrap();
    assert!(hosts(&first, "model-gateway").is_empty());
    assert_eq!(hosts(&first, "alerter"), [("api.github.com".to_string(), 443, false, false)]);
    let cpa = hosts(&first, "cpa");
    assert!(
        cpa.len() == 5
            && cpa
                .iter()
                .all(|h| h.0.contains("anthropic") || h.0.contains("openai") || h.0 == "platform.claude.com" || h.0 == "chatgpt.com")
    );
    assert_eq!(link.sync(&t.admin).await, Sync::Current);

    // A provider: one entry for its host and port, plain only for http.
    add_provider(&t, "http://mock:8765/v1").await;
    add_provider(&t, "https://api.example.com/v1").await;
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    let push = fake.pushes.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        hosts(&push, "model-gateway"),
        [("api.example.com".to_string(), 443, false, false), ("mock".to_string(), 8765, true, false)]
    );

    // ntfy and SMTP.
    {
        use askbar_proto::alerter::{NtfyConfig, SmtpConfig, SmtpTls};
        let db = t.admin.db.lock().await;
        let ntfy = NtfyConfig { server: "http://ntfy.lan:8080".into(), topic: TOPIC.into(), token: Some(NTFY_TOKEN.into()) };
        askbar_admin::alerter_link::write_ntfy(db.conn(), &t.admin.secrets, Some(&ntfy)).unwrap();
        let mail = SmtpConfig {
            host: "Smtp.Example.com".into(),
            port: 587,
            tls: SmtpTls::Starttls,
            user: None,
            password: None,
            from: "a@example.com".into(),
            to: "b@example.com".into(),
        };
        askbar_admin::alerter_link::write_email(db.conn(), &t.admin.secrets, Some(&mail)).unwrap();
    }
    link.sync(&t.admin).await;
    let push = fake.pushes.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        hosts(&push, "alerter"),
        [
            ("api.github.com".to_string(), 443, false, false),
            ("ntfy.lan".to_string(), 8080, true, false),
            ("smtp.example.com".to_string(), 587, false, true)
        ]
    );
    assert!(!push.to_string().contains("allow_private\":true"));
    let text = push.to_string();
    for secret in [TOPIC, NTFY_TOKEN, KEY] {
        assert!(!text.contains(secret), "{secret} is in the push");
    }

    // A restarted proxy has no config and is given it again.
    let n = fake.pushes.lock().unwrap().len();
    *fake.running.lock().unwrap() = None;
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    assert_eq!(fake.pushes.lock().unwrap().len(), n + 1);
    assert_eq!(link.sync(&t.admin).await, Sync::Current);
}

#[tokio::test]
async fn each_push_is_recorded_without_a_host_and_is_not_made_when_it_cannot_be_recorded() {
    let (t, fake, link) = rig().await;
    add_provider(&t, "https://marker-host.example/v1").await;
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    let rows = t.rig.rows("SELECT outcome, detail_json FROM admin_actions WHERE action = 'egress-proxy' ORDER BY at, rowid");
    let outcomes: Vec<&str> = rows.iter().map(|r| r[0].as_str().unwrap()).collect();
    assert_eq!(outcomes, ["requested", "done"]);
    let detail: Value = serde_json::from_str(rows[0][1].as_str().unwrap()).unwrap();
    assert_eq!(detail["sources"], json!({ "alerter": 1, "cpa": 5, "model-gateway": 1 }));
    assert!(detail["version"].as_u64().is_some());
    assert!(!format!("{rows:?}").contains("marker-host"), "a host is in the record");

    // An auditor that takes nothing: the push is not made.
    let auditor =
        serve(Router::new().route("/api/audit/v1/admin-actions", axum::routing::post(|| async { StatusCode::SERVICE_UNAVAILABLE }))).await;
    let admin = askbar_admin::AdminState::new(askbar_admin::AdminDb::open_in_memory().unwrap());
    admin.actions.set(askbar_admin::actions::ActionLog::new(&auditor, "t".into())).ok().unwrap();
    let n = fake.pushes.lock().unwrap().len();
    assert!(matches!(link.sync(&admin).await, Sync::NotReady(_)));
    assert_eq!(fake.pushes.lock().unwrap().len(), n, "nothing was pushed");
}

#[tokio::test]
async fn a_proxy_that_is_down_or_refuses_is_reported_and_does_not_block_saving_a_provider() {
    let t = common::start(true).await;
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let link = EgressLink::new(&dead, TOKEN.into());
    t.admin.egress.set(link.clone()).ok().unwrap();
    add_provider(&t, "https://api.example.com/v1").await;
    assert_eq!(link.sync(&t.admin).await, Sync::Unreachable);
    assert!(!link.online());
    assert_eq!(link.last_error().as_deref(), Some("The egress proxy does not answer."));

    // A wrong token is a refusal to read, not a push.
    let (_, _fake, wrong) = {
        let (_, fake, _) = rig().await;
        let url = serve(Router::new().route("/api/egress-proxy/v1/status", get(status)).with_state(fake.clone())).await;
        (0, fake, EgressLink::new(&url, "other".into()))
    };
    assert_eq!(wrong.sync(&t.admin).await, Sync::Unreachable);
    assert!(wrong.last_error().unwrap().contains("token"));
}

#[tokio::test]
async fn an_approval_is_an_allow_private_entry_for_its_source_only_and_is_audited_and_removable() {
    let (t, fake, link) = rig().await;
    let a = "/api/admin/v1/egress/approvals";
    let (st, body) =
        t.call(M::PUT, a, json!({ "source": "model-gateway", "host": "Mock-LLM.", "port": 8765, "plain_http": true, "note": "dev" })).await;
    assert_eq!(st, 200, "{body}");
    assert_eq!(body["host"], "mock-llm");
    // A derived entry of the same destination keeps its other flags and becomes private; another source is untouched.
    add_provider(&t, "http://lan-llm:9000/v1").await;
    t.call(M::PUT, a, json!({ "source": "model-gateway", "host": "lan-llm", "port": 9000 })).await;
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    let push = fake.pushes.lock().unwrap().last().cloned().unwrap();
    let private = |src: &str| -> Vec<String> {
        push["sources"][src]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["allow_private"] == true)
            .map(|e| format!("{}:{}:{}", e["host"].as_str().unwrap(), e["port"], e["plain_http"]))
            .collect()
    };
    assert_eq!(private("model-gateway"), ["lan-llm:9000:true", "mock-llm:8765:true"]);
    assert!(private("alerter").is_empty() && private("cpa").is_empty());
    let (st, list) = t.call(M::GET, a, json!(null)).await;
    assert_eq!((st, list.as_array().unwrap().len()), (200, 2));

    // Audited, with the destination (not a secret).
    let rows = t.rig.rows("SELECT outcome, detail_json FROM admin_actions WHERE category = 'egress' ORDER BY at, rowid");
    assert!(rows.len() >= 4 && rows[0][0] == "requested" && rows[1][0] == "done");
    assert!(rows[1][1].as_str().unwrap().contains("mock-llm"));

    // Delete removes it, and the proxy is given the change.
    let (st, _) = t.call(M::DELETE, &format!("{a}?source=model-gateway&host=mock-llm&port=8765"), json!(null)).await;
    assert_eq!(st, 204);
    let (st, _) = t.call(M::DELETE, &format!("{a}?source=model-gateway&host=mock-llm&port=8765"), json!(null)).await;
    assert_eq!(st, 404);
    link.sync(&t.admin).await;
    let push = fake.pushes.lock().unwrap().last().cloned().unwrap();
    assert!(!push.to_string().contains("mock-llm"));
}

#[tokio::test]
async fn an_approval_with_a_bad_source_host_or_port_is_refused() {
    let t = common::start(true).await;
    let a = "/api/admin/v1/egress/approvals";
    for bad in [
        json!({ "source": "nobody", "host": "ok.lan" }),
        json!({ "source": "alerter", "host": "ünï.lan" }),
        json!({ "source": "alerter", "host": "a.lan:25" }),
        json!({ "source": "alerter", "host": "*.lan" }),
        json!({ "source": "alerter", "host": "0x7f.1" }),
        json!({ "source": "alerter", "host": "ok.lan", "port": 0 }),
        json!({ "source": "alerter", "host": "ok.lan", "note": "x".repeat(201) }),
    ] {
        let (st, body) = t.call(M::PUT, a, bad.clone()).await;
        assert_eq!(st, 400, "{bad} {body}");
    }
    let (_, list) = t.call(M::GET, a, json!(null)).await;
    assert!(list.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn the_denied_attempts_are_the_last_status_and_a_proxy_that_is_down_is_not_an_empty_list() {
    let (t, fake, link) = rig().await;
    let path = "/api/admin/v1/egress/denied";
    let (st, body) = t.call(M::GET, path, json!(null)).await;
    assert_eq!((st, body["denied"].as_array().unwrap().len()), (200, 0));
    let one = json!({ "source": "alerter", "host": "<img src=x>.example", "port": 443, "reason": "not_listed", "at": 1700000000 });
    *fake.denied.lock().unwrap() = vec![one.clone()];
    link.sync(&t.admin).await;
    let (_, body) = t.call(M::GET, path, json!(null)).await;
    assert_eq!((body["online"].clone(), body["denied"].clone()), (json!(true), json!([one])));

    let down = EgressLink::new("http://127.0.0.1:1", TOKEN.into());
    let t2 = common::start(true).await;
    t2.admin.egress.set(down.clone()).ok().unwrap();
    down.sync(&t2.admin).await;
    let (_, body) = t2.call(M::GET, path, json!(null)).await;
    assert_eq!((body["online"].clone(), body["error"].clone()), (json!(false), json!("The egress proxy does not answer.")));
}

#[tokio::test]
async fn searxng_and_fetch_are_pushed_only_to_a_proxy_that_reports_them_and_none_is_private() {
    let (t, fake, link) = rig().await;
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    let push = fake.pushes.lock().unwrap().last().cloned().unwrap();
    assert!(push["sources"].get("searxng").is_none() && push["sources"].get("fetch").is_none());

    *fake.listeners.lock().unwrap() = vec!["searxng", "fetch"];
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    let push = fake.pushes.lock().unwrap().last().cloned().unwrap();
    let want: Vec<String> = askbar_admin::constants::SEARXNG_EGRESS_HOSTS.iter().map(|h| h.to_string()).collect();
    let mut got = hosts(&push, "searxng");
    assert!(got.iter().all(|e| e.1 == 443 && !e.2 && !e.3));
    got.sort();
    assert_eq!(got.into_iter().map(|e| e.0).collect::<Vec<_>>(), {
        let mut w = want.clone();
        w.sort();
        w
    });
    let fetch = push["sources"]["fetch"]["entries"].as_array().unwrap();
    assert_eq!(fetch.len(), 2);
    for (port, plain) in [(443, false), (80, true)] {
        let e = fetch.iter().find(|e| e["port"].as_u64().unwrap_or(443) == port).unwrap();
        assert_eq!(
            (e["any_public"].as_bool(), e["plain_http"].as_bool().unwrap_or(false), e["host"].as_str().unwrap_or("")),
            (Some(true), plain, "")
        );
    }
    assert!(!push.to_string().contains("\"allow_private\":true"));
    let sources = push["sources"].as_object().unwrap();
    assert_eq!(sources.len(), 5);

    // A proxy that loses the listeners gets the sources dropped again (no error).
    fake.listeners.lock().unwrap().retain(|s| *s != "searxng");
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    let push = fake.pushes.lock().unwrap().last().cloned().unwrap();
    assert!(push["sources"].get("searxng").is_none() && push["sources"].get("fetch").is_some());
}
