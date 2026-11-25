#![cfg(unix)]
//! The quick search and MCP limits (plan P8.6d): listed with default and range, changed in part, reset with null, refused
//! outside their range with nothing saved or pushed, audited, and carried to the server, the gateway config and the front.

mod common;
use askbar_admin::mcp_link::{McpLink, Sync};
use askbar_proto::mcp as m;
use askbar_proto::quick_search as q;
use axum::{
    extract::State,
    routing::{get, put},
    Json, Router,
};
use reqwest::Method as M;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const URL: &str = "/api/admin/v1/limits";
const TOKEN: &str = "admmcp_test_token";

#[derive(Default)]
struct Front(Mutex<Vec<Value>>);

async fn status(State(f): State<Arc<Front>>) -> Json<Value> {
    let v = f.0.lock().unwrap().last().map(|c| {
        let cfg: askbar_mcp_gateway::config::Config = serde_json::from_value(c.clone()).unwrap();
        cfg.version()
    });
    Json(json!({ "config_version": v, "boot_id": "b", "calls": {} }))
}

async fn config(State(f): State<Arc<Front>>, Json(b): Json<Value>) -> Json<Value> {
    f.0.lock().unwrap().push(b);
    Json(json!({}))
}

async fn server_limits(t: &common::T) -> q::QuickSearchLimits {
    askbar_server::snapshot::current(&t.server.db.lock().unwrap()).unwrap().unwrap().quick_search
}

async fn gateway_limits(t: &common::T) -> q::QuickSearchLimits {
    let db = t.admin.db.lock().await;
    askbar_admin::gateway_link::build_config(db.conn(), &t.admin.secrets, None).unwrap().quick_search
}

#[tokio::test]
async fn limits_list_change_reset_and_reach_the_server_the_gateway_and_the_front() {
    let t = common::start(true).await;
    let front = Arc::new(Front::default());
    let url = common::serve(
        Router::new()
            .route("/api/mcp-gateway/v1/status", get(status))
            .route("/api/mcp-gateway/v1/config", put(config))
            .with_state(front.clone()),
    )
    .await;
    let link = McpLink::new(&url, TOKEN.into());
    t.admin.mcp.set(link.clone()).ok().unwrap();

    // Listed with value, default and range.
    let (st, list) = t.call(M::GET, URL, json!(null)).await;
    assert_eq!(st, 200);
    assert_eq!(
        list["quick_search"][0],
        json!({ "name": "max_queries", "value": 3, "default": q::DEFAULT_QUERIES, "min": q::MIN_QUERIES, "max": q::MAX_QUERIES })
    );
    assert_eq!(list["quick_search"].as_array().unwrap().len(), 4);
    let results = list["mcp"].as_array().unwrap().iter().find(|r| r["name"] == "search_results").unwrap();
    assert_eq!(
        results,
        &json!({ "name": "search_results", "value": 8, "default": m::DEFAULT_SEARCH_RESULTS, "min": m::MIN_SEARCH_RESULTS, "max": m::MAX_SEARCH_RESULTS })
    );
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));

    // A change reaches the server before the answer, the gateway config and the front's next push.
    let (st, body) = t.call(M::PUT, URL, json!({ "max_queries": 5, "search_results": 12 })).await;
    assert_eq!(st, 200, "{body}");
    assert_eq!(body["quick_search"][0]["value"], 5);
    assert_eq!(server_limits(&t).await.max_queries, 5);
    assert_eq!(gateway_limits(&t).await.max_queries, 5);
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    assert_eq!(front.0.lock().unwrap().last().unwrap()["search_results"], 12);
    let rows = t.rig.rows("SELECT outcome, detail_json FROM admin_actions WHERE category = 'limits' AND action = 'set' ORDER BY at, rowid");
    assert_eq!(rows.last().unwrap()[0], "done", "{rows:?}");
    let detail = rows.last().unwrap()[1].as_str().unwrap();
    assert!(detail.contains("max_queries") && detail.contains("search_results"), "{detail}");

    // Out of range, unknown or not a number: refused with an error code, nothing saved or pushed.
    let pushes = front.0.lock().unwrap().len();
    for bad in [
        json!({ "max_queries": 6 }),
        json!({ "max_pages": 0 }),
        json!({ "fetch_redirects": 99, "max_pages": 2 }),
        json!({ "nope": 1 }),
        json!({ "max_pages": -1 }),
    ] {
        let (st, b) = t.call(M::PUT, URL, bad.clone()).await;
        assert_eq!((st, b["error"].as_str()), (400, Some("bad_request")), "{bad} {b}");
    }
    let (_, b) = t.call(M::PUT, URL, json!({ "max_queries": 6 })).await;
    assert!(b["detail"].as_str().unwrap().contains("max_queries"), "{b}");
    assert_eq!(t.call(M::GET, URL, json!(null)).await.1["quick_search"][2]["value"], q::DEFAULT_PAGES);
    assert_eq!(server_limits(&t).await.max_pages, q::DEFAULT_PAGES);
    assert_eq!(link.sync(&t.admin).await, Sync::Current);
    assert_eq!(front.0.lock().unwrap().len(), pushes, "the front saw nothing new");

    // null resets to the default, and the push returns to it.
    let (st, body) = t.call(M::PUT, URL, json!({ "max_queries": null, "search_results": null })).await;
    assert_eq!(st, 200, "{body}");
    assert_eq!(server_limits(&t).await, q::QuickSearchLimits::DEFAULT);
    assert_eq!(gateway_limits(&t).await, q::QuickSearchLimits::DEFAULT);
    assert!(matches!(link.sync(&t.admin).await, Sync::Pushed(_)));
    assert_eq!(front.0.lock().unwrap().last().unwrap()["search_results"], m::DEFAULT_SEARCH_RESULTS);
}
