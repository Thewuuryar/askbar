#![cfg(unix)]
//! The approved model catalog (plan step 3.3, ADR 0014): the admin keeps it, refuses one that has no default, and gives
//! it to the gateway, which serves it to the server.

mod common;
use reqwest::Method as M;
use serde_json::{json, Value};

async fn provider(t: &common::T, name: &str) -> String {
    let (st, made) = t
        .call(M::POST, "/api/admin/v1/providers", json!({ "name": name, "type": "openai", "base_url": "http://x/v1", "model": "m" }))
        .await;
    assert_eq!(st, 201, "{made}");
    made["id"].as_str().unwrap().to_string()
}

async fn catalog(t: &common::T) -> Value {
    let (st, c) = t.call(M::GET, "/api/admin/v1/catalog", json!(null)).await;
    assert_eq!(st, 200);
    c
}

async fn config(t: &common::T) -> askbar_proto::gateway::GatewayConfig {
    let db = t.admin.db.lock().await;
    askbar_admin::gateway_link::build_config(db.conn(), &t.admin.secrets, None).unwrap()
}

#[tokio::test]
async fn a_new_install_approves_nothing_and_the_gateway_is_told_so() {
    let t = common::start(true).await;
    provider(&t, "one").await;
    assert_eq!(catalog(&t).await, json!({ "models": [], "default": null }));
    let cfg = config(&t).await;
    assert_eq!((cfg.catalog.len(), cfg.default_model), (0, None));
}

#[tokio::test]
async fn a_catalog_needs_a_default_that_is_one_of_its_models() {
    let t = common::start(true).await;
    let a = provider(&t, "one").await;
    let b = provider(&t, "two").await;
    let put = |body: Value| t.call(M::PUT, "/api/admin/v1/catalog", body);

    for (why, body) in [
        ("no default", json!({ "models": [{ "provider_id": a, "model": "x" }], "default": null })),
        ("a default outside the list", json!({ "models": [{ "provider_id": a, "model": "x" }], "default": format!("{b}/x") })),
        ("an unknown provider", json!({ "models": [{ "provider_id": "nobody", "model": "x" }], "default": "nobody/x" })),
        (
            "a model twice",
            json!({ "models": [{ "provider_id": a, "model": "x" }, { "provider_id": a, "model": "x" }], "default": format!("{a}/x") }),
        ),
        ("a blank model", json!({ "models": [{ "provider_id": a, "model": "  " }], "default": format!("{a}/ ") })),
        ("a default with nothing approved", json!({ "models": [], "default": format!("{a}/x") })),
    ] {
        let (st, p) = put(body).await;
        assert_eq!(st, 400, "{why}: {p}");
    }
    assert_eq!(catalog(&t).await, json!({ "models": [], "default": null }), "a refusal changes nothing");

    let good = json!({
        "models": [{ "provider_id": a, "model": "gpt-x" }, { "provider_id": b, "model": "meta/llama" }],
        "default": format!("{b}/meta/llama"),
    });
    let (st, saved) = put(good.clone()).await;
    assert_eq!(st, 200, "{saved}");
    assert_eq!(saved, good);
    assert_eq!(catalog(&t).await, good, "in the order it was saved, with its default");

    // Replacing it replaces all of it, and an empty catalog with no default is allowed.
    assert_eq!(put(json!({ "models": [], "default": null })).await.0, 200);
    assert_eq!(catalog(&t).await, json!({ "models": [], "default": null }));
}

#[tokio::test]
async fn the_gateway_is_given_the_catalog_and_a_change_is_a_new_version() {
    let t = common::start(true).await;
    let a = provider(&t, "one").await;
    let before = config(&t).await.version;
    let body = json!({ "models": [{ "provider_id": a, "model": "gpt-x" }, { "provider_id": a, "model": "gpt-y" }], "default": format!("{a}/gpt-y") });
    assert_eq!(t.call(M::PUT, "/api/admin/v1/catalog", body).await.0, 200);
    let cfg = config(&t).await;
    assert_eq!(cfg.catalog.iter().map(|m| m.id()).collect::<Vec<_>>(), [format!("{a}/gpt-x"), format!("{a}/gpt-y")]);
    assert_eq!(cfg.default_model, Some(format!("{a}/gpt-y")));
    assert_ne!(cfg.version, before, "so the gateway is pushed again");
}

#[tokio::test]
async fn deleting_a_provider_takes_its_models_out_but_not_while_it_holds_the_default() {
    let t = common::start(true).await;
    let a = provider(&t, "one").await;
    let b = provider(&t, "two").await;
    let body = json!({ "models": [{ "provider_id": a, "model": "x" }, { "provider_id": b, "model": "y" }], "default": format!("{a}/x") });
    assert_eq!(t.call(M::PUT, "/api/admin/v1/catalog", body).await.0, 200);

    let (st, p) = t.call(M::DELETE, &format!("/api/admin/v1/providers/{a}"), json!(null)).await;
    assert_eq!(st, 409, "{p}");
    assert_eq!(catalog(&t).await["models"].as_array().unwrap().len(), 2, "nothing changed");

    assert_eq!(t.call(M::DELETE, &format!("/api/admin/v1/providers/{b}"), json!(null)).await.0, 204);
    assert_eq!(catalog(&t).await, json!({ "models": [{ "provider_id": a, "model": "x" }], "default": format!("{a}/x") }));
    // The last provider can go: nothing is left approved, so no default is needed.
    assert_eq!(t.call(M::DELETE, &format!("/api/admin/v1/providers/{a}"), json!(null)).await.0, 204);
    assert_eq!(catalog(&t).await, json!({ "models": [], "default": null }));
}

#[tokio::test]
async fn saving_the_catalog_is_an_admin_action_that_is_recorded_first() {
    let t = common::start(true).await;
    let a = provider(&t, "one").await;
    t.call(M::PUT, "/api/admin/v1/catalog", json!({ "models": [{ "provider_id": a, "model": "x" }], "default": format!("{a}/x") })).await;
    let rows =
        t.rig.rows("SELECT outcome FROM admin_actions WHERE category = 'providers' AND action = 'catalog_update' ORDER BY at, rowid");
    assert_eq!(rows.iter().map(|r| r[0].as_str().unwrap()).collect::<Vec<_>>(), ["requested", "done"]);
}
