#![cfg(unix)]
//! The model pinned to each role (plan W4.2): set, list, clear; only an approved model; audited; a pin that stops being
//! approved is listed as not approved and is not given to the gateway.

mod common;
use askbar_proto::gateway::Role;
use reqwest::Method as M;
use serde_json::json;

async fn config(t: &common::T) -> askbar_proto::gateway::GatewayConfig {
    let db = t.admin.db.lock().await;
    askbar_admin::gateway_link::build_config(db.conn(), &t.admin.secrets, None).unwrap()
}

#[tokio::test]
async fn pins_round_trip_need_an_approved_model_are_audited_and_only_approved_ones_are_pushed() {
    let t = common::start(true).await;
    let (_, p) = t
        .call(M::POST, "/api/admin/v1/providers", json!({ "name": "one", "type": "openai", "base_url": "http://x/v1", "model": "m" }))
        .await;
    let pid = p["id"].as_str().unwrap().to_string();
    let roles = "/api/admin/v1/roles";
    let none = |r: &str| json!({ "role": r, "pin": null, "approved": false });
    let (st, list) = t.call(M::GET, roles, json!(null)).await;
    assert_eq!((st, &list), (200, &json!([none("chat.manual"), none("search.query"), none("search.writer")])));
    assert!(config(&t).await.role_pins.is_empty());

    // An unapproved model is refused, an unknown role is a 404, and nothing changes.
    let pin = json!({ "provider_id": pid, "model": "gpt-x" });
    let (st, b) = t.call(M::PUT, &format!("{roles}/search.query"), pin.clone()).await;
    assert_eq!((st, b["error"].as_str()), (400, Some("model_not_approved")), "{b}");
    assert_eq!(t.call(M::PUT, &format!("{roles}/nope"), pin.clone()).await.0, 404);
    assert_eq!(t.call(M::DELETE, &format!("{roles}/search.query"), json!(null)).await.0, 404, "nothing to clear");
    assert_eq!(t.call(M::GET, roles, json!(null)).await.1, list);

    let cat = json!({ "models": [{ "provider_id": pid, "model": "gpt-x" }], "default": format!("{pid}/gpt-x") });
    assert_eq!(t.call(M::PUT, "/api/admin/v1/catalog", cat).await.0, 200);
    let (st, b) = t.call(M::PUT, &format!("{roles}/search.query"), pin.clone()).await;
    assert_eq!((st, &b), (200, &pin));
    let (_, list) = t.call(M::GET, roles, json!(null)).await;
    assert_eq!(list[1], json!({ "role": "search.query", "pin": pin, "approved": true }));
    let before = config(&t).await;
    assert_eq!(before.role_pins.get(&Role::SearchQuery).map(String::as_str), Some(format!("{pid}/gpt-x").as_str()));

    // Audited with the role and the model, no more.
    let rows = t.rig.rows("SELECT outcome, detail_json FROM admin_actions WHERE category = 'roles' ORDER BY at, rowid");
    let done = rows.last().unwrap();
    assert!(rows.iter().any(|r| r[0] == "refused") && done[0] == "done", "{rows:?}");
    assert!(done[1].as_str().unwrap().contains("search.query"));

    // The model leaves the catalog: the pin stays listed, marked not approved, and is not pushed.
    let off = json!({ "models": [], "default": null });
    assert_eq!(t.call(M::PUT, "/api/admin/v1/catalog", off).await.0, 200);
    let (_, list) = t.call(M::GET, roles, json!(null)).await;
    assert_eq!(list[1], json!({ "role": "search.query", "pin": pin, "approved": false }));
    let after = config(&t).await;
    assert!(after.role_pins.is_empty());
    assert_ne!(after.version, before.version, "the push carries the change");

    assert_eq!(t.call(M::DELETE, &format!("{roles}/search.query"), json!(null)).await.0, 204);
    assert_eq!(t.call(M::GET, roles, json!(null)).await.1[1], none("search.query"));
}
