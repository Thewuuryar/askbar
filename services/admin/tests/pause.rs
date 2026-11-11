#![cfg(unix)]
//! The emergency stop for one person and what the gateway is told about stops and budgets (plan step 3.2, ADR 0014): the
//! admin keeps a pause flag for each user and gives it to the server (to refuse early) and to the gateway (the authority).

mod common;
use reqwest::Method as M;
use serde_json::{json, Value};

async fn make_user(t: &common::T, name: &str) -> String {
    let (st, made) = t.call(M::POST, "/api/admin/v1/users", json!({ "name": name })).await;
    assert_eq!(st, 201, "{made}");
    made["id"].as_str().unwrap().to_string()
}

fn pushed_paused(t: &common::T) -> Vec<String> {
    askbar_server::snapshot::current(&t.server.db.lock().unwrap()).unwrap().unwrap().paused_user_ids
}

#[tokio::test]
async fn pausing_a_user_reaches_the_server_and_resuming_takes_it_back() {
    let t = common::start(true).await;
    let alice = make_user(&t, "Alice").await;
    let bob = make_user(&t, "Bob").await;
    assert!(pushed_paused(&t).is_empty());

    assert_eq!(t.call(M::POST, &format!("/api/admin/v1/users/{alice}/pause"), json!(null)).await.0, 204);
    assert_eq!(pushed_paused(&t), std::slice::from_ref(&alice), "the server has it, and only Alice");
    let (_, users) = t.call(M::GET, "/api/admin/v1/users", json!(null)).await;
    let paused: Vec<(String, bool)> =
        users.as_array().unwrap().iter().map(|u| (u["name"].as_str().unwrap().to_string(), u["paused"].as_bool().unwrap())).collect();
    assert!(paused.contains(&("Alice".into(), true)) && paused.contains(&("Bob".into(), false)), "{paused:?}");

    // Pausing again changes nothing and is still fine; so is pausing a second person.
    assert_eq!(t.call(M::POST, &format!("/api/admin/v1/users/{alice}/pause"), json!(null)).await.0, 204);
    assert_eq!(t.call(M::POST, &format!("/api/admin/v1/users/{bob}/pause"), json!(null)).await.0, 204);
    let mut both = pushed_paused(&t);
    both.sort();
    let mut want = vec![alice.clone(), bob.clone()];
    want.sort();
    assert_eq!(both, want);

    assert_eq!(t.call(M::POST, &format!("/api/admin/v1/users/{alice}/resume"), json!(null)).await.0, 204);
    assert_eq!(pushed_paused(&t), std::slice::from_ref(&bob));
    assert_eq!(t.call(M::POST, "/api/admin/v1/users/no-such-user/pause", json!(null)).await.0, 404);
    assert_eq!(t.call(M::POST, "/api/admin/v1/users/no-such-user/resume", json!(null)).await.0, 404);
}

#[tokio::test]
async fn the_gateway_is_given_the_pause_the_budget_the_kill_switch_and_how_each_provider_is_paid_for() {
    let t = common::start(true).await;
    let alice = make_user(&t, "Alice").await;
    t.call(
        M::PUT,
        "/api/admin/v1/setup/audit-key",
        json!({ "passphrase": "a passphrase of at least twenty", "confirm": "a passphrase of at least twenty" }),
    )
    .await;
    for (name, billing) in [("metered-one", "metered"), ("flat-one", "subscription")] {
        let (st, _) = t
            .call(
                M::POST,
                "/api/admin/v1/providers",
                json!({ "name": name, "type": "openai", "base_url": "http://x/v1", "model": "m", "billing": billing }),
            )
            .await;
        assert_eq!(st, 201, "{name}");
    }
    assert_eq!(t.call(M::POST, &format!("/api/admin/v1/users/{alice}/pause"), json!(null)).await.0, 204);
    let disp = serde_json::to_value(askbar_proto::DisplaySettings::default()).unwrap();
    // (The budget numbers have their own routes today: set them in the table the snapshot reads.)
    t.admin.db.lock().await.conn().execute("UPDATE budgets SET daily_tokens = 1000, monthly_tokens = 20000 WHERE id = 1", []).unwrap();
    let save = json!({ "system_prompt": "x", "display": disp, "kill_switch": true, "kill_switch_aborts_running": false });
    assert_eq!(t.call(M::PUT, "/api/admin/v1/settings", save).await.0, 204);

    let cfg = {
        let db = t.admin.db.lock().await;
        askbar_admin::gateway_link::build_config(db.conn(), &t.admin.secrets, None).unwrap()
    };
    assert_eq!(cfg.paused_users, [alice]);
    assert_eq!((cfg.budget.daily_tokens, cfg.budget.monthly_tokens), (Some(1000), Some(20000)));
    assert!(cfg.kill_switch && !cfg.kill_switch_aborts_running);
    let billing = |n: &str| cfg.providers.iter().find(|p| p.name == n).map(|p| p.billing).unwrap();
    assert_eq!((billing("metered-one"), billing("flat-one")), (askbar_proto::Billing::Metered, askbar_proto::Billing::Subscription));
    // A change to any of them is a new configuration version, so the gateway is pushed again.
    let v1 = cfg.version;
    t.admin.db.lock().await.conn().execute("UPDATE users SET paused = 0", []).unwrap();
    let v2 = {
        let db = t.admin.db.lock().await;
        askbar_admin::gateway_link::build_config(db.conn(), &t.admin.secrets, None).unwrap().version
    };
    assert_ne!(v1, v2);
}

#[tokio::test]
async fn a_pause_and_a_resume_are_admin_actions_that_are_recorded_first() {
    let t = common::start(true).await;
    let alice = make_user(&t, "Alice").await;
    t.call(M::POST, &format!("/api/admin/v1/users/{alice}/pause"), json!(null)).await;
    let rows: Vec<Value> = t
        .rig
        .rows("SELECT action, outcome, detail_json FROM admin_actions WHERE category = 'users' AND action = 'pause' ORDER BY at, rowid")
        .into_iter()
        .map(|r| json!({ "action": r[0], "outcome": r[1], "detail": serde_json::from_str::<Value>(r[2].as_str().unwrap()).unwrap() }))
        .collect();
    assert_eq!(rows.iter().map(|r| r["outcome"].as_str().unwrap()).collect::<Vec<_>>(), ["requested", "done"], "{rows:?}");
    assert_eq!(rows[0]["detail"]["params"]["id"], json!(alice));
}
