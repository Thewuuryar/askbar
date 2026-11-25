//! The Search page's routes (P8.6f): searxng shown as down when the front's last search failed, the engines against the egress
//! hosts, and the usage figures read from the auditor (never the server's tables) with no query, URL or page text.

mod common;
use askbar_admin::auditor_link::AuditorLink;
use askbar_admin::constants::{engine_drift, SEARXNG_EGRESS_HOSTS, SEARXNG_ENGINES};
use askbar_admin::mcp_link::McpLink;
use askbar_admin::{server_link::ServerLink, web, AdminDb, AdminState};
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use common::serve;
use reqwest::Method as M;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const SECRET: &str = "very private question";

// ------------------------------------------------------------------ engines and egress hosts

fn enabled_engines() -> Vec<String> {
    let path = format!("{}/../../docker/searxng/settings.yml", env!("CARGO_MANIFEST_DIR"));
    let v: Value = serde_saphyr::from_str(&std::fs::read_to_string(path).unwrap()).expect("settings.yml is YAML");
    let keep = v["use_default_settings"]["engines"]["keep_only"].as_array().expect("the engines are a keep_only list").clone();
    keep.iter().map(|e| e.as_str().unwrap().to_string()).collect()
}

#[test]
fn the_engines_in_the_searxng_settings_and_the_egress_hosts_agree() {
    let engines = enabled_engines();
    let engines: Vec<&str> = engines.iter().map(String::as_str).collect();
    assert_eq!(engine_drift(&engines, &SEARXNG_EGRESS_HOSTS, &SEARXNG_ENGINES), Ok(()));
    assert_eq!(engines.len(), SEARXNG_ENGINES.len(), "the page lists exactly the enabled engines");
}

#[test]
fn the_drift_check_fails_on_a_missing_host_or_engine() {
    let pairs = [("a", "a.example"), ("b", "b.example")];
    assert_eq!(engine_drift(&["a", "b"], &["a.example", "b.example"], &pairs), Ok(()));
    assert!(engine_drift(&["a", "b"], &["a.example"], &pairs).unwrap_err().contains("engine b has no egress host"), "a host is missing");
    assert!(engine_drift(&["a", "b", "c"], &["a.example", "b.example"], &pairs).unwrap_err().contains("engine c"), "an engine has no pair");
    assert!(engine_drift(&["a"], &["a.example", "b.example"], &pairs).unwrap_err().contains("host b.example has no engine"));
}

// ------------------------------------------------------------------ searxng health in /mcp/status

#[derive(Default)]
struct Front {
    last_search: Mutex<Option<Value>>,
}

async fn front_status(State(f): State<Arc<Front>>) -> Json<Value> {
    let mut v = json!({ "config_version": 1, "boot_id": "b", "calls": {} });
    if let Some(l) = f.last_search.lock().unwrap().clone() {
        v["last_search"] = l;
    }
    Json(v)
}

#[tokio::test]
async fn searxng_that_does_not_answer_shows_as_down_at_the_next_poll() {
    let t = common::start_with(true, false).await;
    let st = |b: &Value| (b["searxng"]["state"].clone(), b["searxng"]["last_at"].clone(), b["searxng"]["outcome"].clone());
    let (_, b) = t.call(M::GET, "/api/admin/v1/mcp/status", json!(null)).await;
    assert_eq!(st(&b).0, "unknown", "no link to the front: not known");

    let front = Arc::new(Front::default());
    let url = serve(Router::new().route("/api/mcp-gateway/v1/status", get(front_status)).with_state(front.clone())).await;
    let link = McpLink::new(&url, "t".into());
    t.admin.mcp.set(link.clone()).ok().unwrap();
    link.sync(&t.admin).await;
    let (_, b) = t.call(M::GET, "/api/admin/v1/mcp/status", json!(null)).await;
    assert_eq!(st(&b), (json!("no_search_yet"), json!(null), json!(null)));

    *front.last_search.lock().unwrap() = Some(json!({ "at": 1_700_000_000, "outcome": "ok" }));
    link.sync(&t.admin).await;
    let (_, b) = t.call(M::GET, "/api/admin/v1/mcp/status", json!(null)).await;
    assert_eq!(st(&b), (json!("ok"), json!(1_700_000_000), json!("ok")));

    *front.last_search.lock().unwrap() = Some(json!({ "at": 1_700_000_100, "outcome": "tool_upstream_failed" }));
    link.sync(&t.admin).await;
    let (_, b) = t.call(M::GET, "/api/admin/v1/mcp/status", json!(null)).await;
    assert_eq!(st(&b), (json!("down"), json!(1_700_000_100), json!("tool_upstream_failed")), "the failed search marks it down");
}

// ------------------------------------------------------------------ usage

fn span(kind: &str, name: &str, status: &str, at: i64, role: Option<&str>, tokens: Option<(i64, i64)>) -> Value {
    json!({
        "kind": kind, "name": name, "status": status, "started_at": at, "provider_id": null,
        "tokens_in": tokens.map(|t| t.0), "tokens_out": tokens.map(|t| t.1), "tokens_cached": tokens.map(|_| 0),
        // What a real span may carry: a query must never come out of the admin.
        "attrs": { "role": role, "query": SECRET, "url": "https://r.example/secret" },
    })
}

async fn auditor_stub(now: i64) -> String {
    let list = move || async move {
        Json(json!([
            { "trace_id": "t1", "started_at": now - 60 },
            { "trace_id": "t-old", "started_at": now - 40 * 86_400 },
        ]))
    };
    let trace = move |axum::extract::Path(id): axum::extract::Path<String>| async move {
        assert_eq!(id, "t1", "a trace outside the window is not read");
        let mut attempt = span("attempt", "provider", "done", now - 60, None, None);
        attempt["provider_id"] = json!("p1");
        Json(json!([
            span("turn", "chat", "done", now - 60, Some("search.query"), Some((1_000_000, 250_000))),
            attempt,
            span("turn", "chat", "done", now - 30, Some("search.writer"), Some((500_000, 0))),
            span("turn", "chat", "done", now - 25, Some("judge.review"), Some((200_000, 0))),
            span("turn", "chat", "done", now - 20, None, Some((9_000_000, 9_000_000))),
            span("tool", "search", "ok", now - 50, None, None),
            span("tool", "search", "error", now - 49, None, None),
            span("tool", "search", "refused", now - 48, None, None),
            span("tool", "fetch", "ok", now - 40, None, None),
            span("gate", "quick_search_refused", "blocked", now - 10, None, None),
        ]))
    };
    serve(Router::new().route("/api/audit/v1/traces", get(list)).route("/api/audit/v1/traces/{id}", get(trace))).await
}

async fn admin_for(auditor_url: Option<&str>) -> (String, reqwest::Client, String) {
    let db = AdminDb::open_in_memory().unwrap();
    web::init_password(&db, common::PW).unwrap();
    // p1: 2.0 per million in, 4.0 per million out.
    db.conn()
        .execute(
            "INSERT INTO providers (id, name, type, base_url, model, created_at, billing, price_in, price_out) VALUES ('p1', 'p1', 'openai', 'http://x/v1', 'm', 0, 'metered', 2.0, 4.0)",
            [],
        )
        .unwrap();
    let state = AdminState::with_server(db, ServerLink::new("http://127.0.0.1:1", "mgt_stub-token".into()));
    if let Some(u) = auditor_url {
        state.auditor.set(AuditorLink::new(u, "admrd_stub-token".into())).ok().unwrap();
    }
    let adm = common::serve(askbar_admin::router(state)).await;
    let http = reqwest::Client::new();
    let r = http
        .post(format!("{adm}/api/admin/v1/session"))
        .header("x-askbar-admin", "1")
        .json(&json!({ "password": common::PW }))
        .send()
        .await
        .unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    (adm, http, cookie)
}

fn close(v: &Value, want: f64) -> bool {
    v.as_f64().is_some_and(|x| (x - want).abs() < 1e-9)
}

#[tokio::test]
async fn usage_counts_per_day_and_prices_each_role_from_the_auditor_with_no_text() {
    let now = askbar_admin::auth::now();
    let url = auditor_stub(now).await;
    let (adm, http, cookie) = admin_for(Some(&url)).await;
    let get = |p: &'static str, cookie: Option<&str>| {
        let mut r = http.get(format!("{adm}/api/admin/v1/usage/roles{p}"));
        if let Some(c) = cookie {
            r = r.header("cookie", c);
        }
        async move {
            let r = r.send().await.unwrap();
            (r.status().as_u16(), r.text().await.unwrap())
        }
    };
    assert_eq!(get("", None).await.0, 401, "a session is needed");
    for bad in ["?days=0", "?days=31"] {
        assert_eq!(get(bad, Some(&cookie)).await.0, 400, "{bad}");
    }
    let (st, text) = get("?days=7", Some(&cookie)).await;
    assert_eq!(st, 200, "{text}");
    for leak in [SECRET, "r.example", "attrs"] {
        assert!(!text.contains(leak), "the usage holds {leak}");
    }
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!((v["days"].clone(), v["traces_read"].clone(), v["truncated"].clone()), (json!(7), json!(1), json!(false)));
    let day = askbar_admin::usage::day_of(now - 30);
    // The refused search and the gate span are refusals; the failed search still counts as a search.
    assert_eq!(v["per_day"], json!([{ "day": day, "searches": 2, "fetches": 1, "refusals": 2 }]));
    let roles = v["roles"].as_array().unwrap();
    assert_eq!(roles.len(), 3, "the turn without a role is left out, the research role is counted");
    assert!(close(&roles.iter().find(|x| x["role"] == "judge.review").unwrap()["cost"], 0.4), "{roles:?}");
    let by = |r: &str| roles.iter().find(|x| x["role"] == r).unwrap();
    // search.query: 1M in x 2.0 + 250k out x 4.0 = 3.0; search.writer: 500k in x 2.0 = 1.0 (priced by the attempt's provider).
    assert_eq!((by("search.query")["tokens_in"].clone(), by("search.query")["tokens_out"].clone()), (json!(1_000_000), json!(250_000)));
    assert!(close(&by("search.query")["cost"], 3.0) && close(&by("search.writer")["cost"], 1.0), "{roles:?}");
}

#[tokio::test]
async fn usage_says_so_when_the_auditor_cannot_be_reached() {
    let (adm, http, cookie) = admin_for(None).await;
    let r = http.get(format!("{adm}/api/admin/v1/usage/roles")).header("cookie", &cookie).send().await.unwrap();
    assert_eq!(r.status(), 503);
    let (adm, http, cookie) = admin_for(Some("http://127.0.0.1:1")).await;
    let r = http.get(format!("{adm}/api/admin/v1/usage/roles")).header("cookie", &cookie).send().await.unwrap();
    assert_eq!(r.status(), 503);
}
