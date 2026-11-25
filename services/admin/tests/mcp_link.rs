//! The admin and the MCP gateway's front (ADR 0028): the push carries the bounded limits, the permissions and the approval public
//! key; a permission change or a key rotation is a new version and is pushed again; an unconfigured front gets it; the record of a
//! push holds no key and no user; no record, no push; and the version is the front's own.

mod common;
use askbar_admin::mcp_link::{McpLink, Sync};
use askbar_mcp_gateway::config::Config;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, put};
use axum::{Json, Router};
use common::{serve, T};
use reqwest::Method as M;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const TOKEN: &str = "admmcp_test_token";

/// A stand-in for the front's admin listener: it checks a push as the front does and reports the version (none until one is given).
#[derive(Default)]
struct FakeFront {
    running: Mutex<Option<u64>>,
    pushes: Mutex<Vec<Value>>,
}
type Fake = Arc<FakeFront>;

fn authorized(h: &HeaderMap) -> bool {
    h.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {TOKEN}"))
}

async fn status(State(f): State<Fake>, h: HeaderMap) -> Result<Json<Value>, StatusCode> {
    if !authorized(&h) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(Json(json!({ "config_version": *f.running.lock().unwrap(), "boot_id": "b", "calls": {} })))
}

async fn config(State(f): State<Fake>, h: HeaderMap, Json(body): Json<Value>) -> Result<Json<Value>, StatusCode> {
    if !authorized(&h) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let cfg: Config = serde_json::from_value(body.clone()).map_err(|_| StatusCode::BAD_REQUEST)?;
    cfg.validate().map_err(|_| StatusCode::BAD_REQUEST)?;
    *f.running.lock().unwrap() = Some(cfg.version());
    f.pushes.lock().unwrap().push(body);
    Ok(Json(json!({})))
}

async fn rig() -> (T, Fake, Arc<McpLink>) {
    let t = common::start(true).await;
    let fake = Fake::default();
    let url = serve(
        Router::new()
            .route("/api/mcp-gateway/v1/status", get(status))
            .route("/api/mcp-gateway/v1/config", put(config))
            .with_state(fake.clone()),
    )
    .await;
    let link = McpLink::new(&url, TOKEN.into());
    t.admin.mcp.set(link.clone()).ok().unwrap();
    (t, fake, link)
}

#[tokio::test]
async fn the_push_carries_limits_permissions_and_the_public_key_and_follows_changes() {
    let (t, fake, link) = rig().await;
    {
        let db = t.admin.db.lock().await;
        askbar_admin::approval_key::ensure(db.conn(), &t.admin.secrets).unwrap();
    }
    // A front with no config is given one (the fake takes it as the front does: unknown fields and bounds are refused).
    let Sync::Pushed(v1) = link.sync(&t.admin).await else { panic!("not pushed") };
    let push = fake.pushes.lock().unwrap().last().cloned().unwrap();
    assert_eq!(push["fetch_max_bytes"], askbar_proto::mcp::DEFAULT_FETCH_MAX_BYTES);
    assert_eq!(push["tool_result_max_bytes"], askbar_proto::mcp::DEFAULT_TOOL_RESULT_MAX_BYTES);
    assert_eq!(push["search_results"], askbar_proto::mcp::DEFAULT_SEARCH_RESULTS);
    assert_eq!(push["permissions"], json!({}));
    let (kv, raw) = askbar_admin::approval_key::public_key(t.admin.db.lock().await.conn()).unwrap();
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!((push["approval_public_key"].as_str(), push["approval_key_version"].as_u64()), (Some(hex.as_str()), Some(kv)));
    assert_eq!(link.sync(&t.admin).await, Sync::Current, "the admin's version is the front's own");

    // A permission change is a new version.
    let (st, _) = t.call(M::PUT, "/api/admin/v1/mcp/permissions/U1", json!({ "allowed": ["search"] })).await;
    // The user must exist for the route; seed one if the rig has none.
    if st == 404 {
        let db = t.admin.db.lock().await;
        db.conn().execute("INSERT INTO mcp_permissions (user_id, tool, state) VALUES ('U1', 'search', 'allow')", []).unwrap();
    }
    let Sync::Pushed(v2) = link.sync(&t.admin).await else { panic!("not re-pushed") };
    assert_ne!(v1, v2);
    assert_eq!(fake.pushes.lock().unwrap().last().unwrap()["permissions"], json!({ "U1": { "allowed": ["search"], "denied": [] } }));

    // A rotation of the approval key is a new version too.
    {
        let db = t.admin.db.lock().await;
        askbar_admin::approval_key::rotate(db.conn(), &t.admin.secrets).unwrap();
    }
    let Sync::Pushed(v3) = link.sync(&t.admin).await else { panic!("not re-pushed") };
    assert_ne!(v2, v3);
    assert_eq!(fake.pushes.lock().unwrap().last().unwrap()["approval_key_version"], kv + 1);

    // A restarted front has no config and is given it again.
    *fake.running.lock().unwrap() = None;
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(v) if v == v3));
}

#[tokio::test]
async fn a_push_is_recorded_without_key_or_user_and_is_not_made_when_it_cannot_be_recorded() {
    let (t, fake, link) = rig().await;
    {
        let db = t.admin.db.lock().await;
        askbar_admin::approval_key::ensure(db.conn(), &t.admin.secrets).unwrap();
        db.conn().execute("INSERT INTO mcp_permissions (user_id, tool, state) VALUES ('marker-user-41', 'search', 'allow')", []).unwrap();
    }
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    let rows = t.rig.rows("SELECT outcome, detail_json FROM admin_actions WHERE action = 'mcp-gateway' ORDER BY at, rowid");
    let outcomes: Vec<&str> = rows.iter().map(|r| r[0].as_str().unwrap()).collect();
    assert_eq!(outcomes, ["requested", "done"]);
    let detail: Value = serde_json::from_str(rows[0][1].as_str().unwrap()).unwrap();
    assert_eq!((detail["users"].as_u64(), detail["target"].as_str()), (Some(1), Some("mcp-gateway")));
    let (_, raw) = askbar_admin::approval_key::public_key(t.admin.db.lock().await.conn()).unwrap();
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    let text = format!("{rows:?}");
    assert!(!text.contains("marker-user") && !text.contains(&hex) && !text.contains(TOKEN), "{text}");

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
async fn a_front_that_is_down_or_refuses_is_reported_and_unset_url_means_no_link() {
    let t = common::start(true).await;
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let link = McpLink::new(&dead, TOKEN.into());
    assert_eq!(link.sync(&t.admin).await, Sync::Unreachable);
    assert_eq!(link.last_error().as_deref(), Some("The MCP gateway does not answer."));

    let (t2, _, bad) = rig().await;
    let wrong = McpLink::new(
        &serve(Router::new().route("/api/mcp-gateway/v1/status", get(|| async { StatusCode::UNAUTHORIZED }))).await,
        "x".into(),
    );
    assert_eq!(wrong.sync(&t2.admin).await, Sync::Unreachable);
    assert_eq!(wrong.last_error().as_deref(), Some("The MCP gateway does not accept this admin's token."));
    drop(bad);

    std::env::remove_var("ASKBAR_MCP_GATEWAY_URL");
    assert!(askbar_admin::mcp_link::front_url().is_none());
    assert!(t.admin.mcp.get().is_none(), "no link until main makes one");
}
