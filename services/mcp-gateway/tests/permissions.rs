//! A tool call needs the user's permission from the pushed config (ADR 0019): off by default, deny wins, a push takes effect
//! at the next call, and a front that has had no push refuses every call. A refusal opens no span and runs no tool.

mod common;

use common::*;
use serde_json::json;
use std::sync::atomic::Ordering;

const U: &str = "https://example.com/a";

#[tokio::test]
async fn only_a_user_who_holds_the_tool_gets_it() {
    let (registry, ran) = counting();
    let perms = json!({"alice": {"allowed": ["fetch"]}, "bob": {"allowed": ["fetch"], "denied": ["fetch"]}});
    let rig = start(Auditor::Up, registry, perms).await;
    let call = |tool: &'static str, user: &'static str, id: &'static str| signed(&rig.sk, tool, U, user, "t1", id, now() + 60);
    for (tool, user, id) in [("fetch", "bob", "1"), ("fetch", "carol", "2"), ("search", "alice", "3")] {
        let r = rig.post(tool, &call(tool, user, id)).await;
        assert_eq!(code(&r), (403, "tool_not_permitted"), "{user} {tool}");
    }
    assert_eq!((ran.load(Ordering::SeqCst), rig.outcome("refused_permission").await), (0, 3));
    assert!(rig.spans().is_empty(), "a permission refusal opens no span");
    assert_eq!(rig.post("fetch", &call("fetch", "alice", "4")).await.0, 200);
    assert_eq!(ran.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_permission_removed_by_a_push_between_two_calls_refuses_the_second() {
    let (registry, ran) = counting();
    let rig = start(Auditor::Up, registry, json!({"alice": {"allowed": ["fetch"]}})).await;
    let call = |id: &str| signed(&rig.sk, "fetch", U, "alice", "t1", id, now() + 60);
    assert_eq!(rig.post("fetch", &call("1")).await.0, 200);
    rig.push(json!({"alice": {"allowed": []}})).await;
    assert_eq!(code(&rig.post("fetch", &call("2")).await), (403, "tool_not_permitted"));
    assert_eq!(ran.load(Ordering::SeqCst), 1, "the second call did not run");
    rig.push(json!({"alice": {"allowed": ["fetch"]}})).await;
    assert_eq!(rig.post("fetch", &call("3")).await.0, 200, "and a push that grants it again takes effect too");
}

#[tokio::test]
async fn a_front_before_its_first_push_refuses_every_call() {
    let (registry, ran) = counting();
    let rig = start_unpushed(Auditor::Up, registry).await;
    let b = signed(&rig.sk, "fetch", U, "alice", "t1", "1", now() + 60);
    assert_eq!(code(&rig.post("fetch", &b).await), (503, "not_configured"));
    let r = reqwest::Client::new().get(format!("{}/tools?user=alice", rig.url)).bearer_auth(SERVER).send().await.unwrap();
    assert_eq!(r.status(), 503);
    // The configured gate answers before the handler, so the outcome counter is not bumped on this route.
    assert_eq!((ran.load(Ordering::SeqCst), rig.g.status().config_version), (0, None));
    assert!(rig.spans().is_empty());
    // The refused approval was not consumed (nothing was checked); once pushed, it passes.
    rig.push(json!({"alice": {"allowed": ["fetch"]}})).await;
    assert_eq!(rig.post("fetch", &b).await.0, 200);
}
