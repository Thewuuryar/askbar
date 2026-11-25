//! Per-user MCP tool permissions (P5A.17): off by default, set/list/delete round trip, validated, audited, gone with the user.

mod common;
use reqwest::Method as M;
use serde_json::json;

#[tokio::test]
async fn permissions_round_trip_default_off_validated_audited_and_deleted_with_the_user() {
    let t = common::start(true).await;
    let p = "/api/admin/v1/mcp/permissions";
    // No row: nothing held, and the pushed config is empty.
    assert_eq!(t.call(M::GET, p, json!(null)).await, (200, json!({})));
    assert!(askbar_admin::mcp::permissions_config(t.admin.db.lock().await.conn()).unwrap().is_empty());

    let (st, b) = t.call(M::PUT, &format!("{p}/U1"), json!({ "allowed": ["search"], "denied": ["fetch"] })).await;
    assert_eq!((st, &b), (200, &json!({ "allowed": ["search"], "denied": ["fetch"] })), "{b}");
    let (_, list) = t.call(M::GET, p, json!(null)).await;
    assert_eq!(list, json!({ "U1": { "allowed": ["search"], "denied": ["fetch"] } }));
    // The pushed shape is what mcp-gateway accepts: user id to { allowed, denied }.
    let cfg = askbar_admin::mcp::permissions_config(t.admin.db.lock().await.conn()).unwrap();
    assert_eq!(serde_json::to_value(&cfg).unwrap(), list);

    for bad in [json!({ "allowed": ["shell"] }), json!({ "denied": ["x"] }), json!({ "allowed": ["fetch"], "denied": ["fetch"] })] {
        assert_eq!(t.call(M::PUT, &format!("{p}/U1"), bad).await.0, 400);
    }
    assert_eq!(t.call(M::PUT, &format!("{p}/nobody"), json!({ "allowed": ["search"] })).await.0, 404);
    assert_eq!(t.call(M::GET, p, json!(null)).await.1, list, "a refused change changes nothing");

    // Audited, with the user and the counts, no more.
    let rows = t.rig.rows("SELECT outcome, detail_json FROM admin_actions WHERE category = 'mcp' ORDER BY at, rowid");
    assert!(rows.len() >= 2 && rows[0][0] == "requested" && rows[1][0] == "done");
    let d = rows[1][1].as_str().unwrap();
    assert!(d.contains("U1") && !d.contains("search"), "{d}");

    assert_eq!(t.call(M::DELETE, &format!("{p}/U1"), json!(null)).await.0, 204);
    assert_eq!(t.call(M::DELETE, &format!("{p}/U1"), json!(null)).await.0, 404);
    assert_eq!(t.call(M::GET, p, json!(null)).await.1, json!({}));

    // The rows go with the user (the user-delete handler needs the server link; check its statement's effect via the table).
    t.call(M::PUT, &format!("{p}/U1"), json!({ "allowed": ["fetch"] })).await;
    let (st, _) = t.call(M::DELETE, "/api/admin/v1/users/U1", json!(null)).await;
    assert_eq!(st, 204);
    assert_eq!(t.call(M::GET, p, json!(null)).await.1, json!({}));
}
