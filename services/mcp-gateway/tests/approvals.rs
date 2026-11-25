//! ADR 0016 "Defend": a signed approval (made by the real signer, `askbar_service::approval::sign`) that is replayed,
//! swapped, expired, stripped, signed by another key or issued for another user, task or tool is refused before the tool
//! runs. An approval failure comes before the span, so what is recorded is the `/status` outcome counter, and no span.
//! The one-time ID is consumed by a call that passed the approval checks (even one that fails later), never by one that did not.

mod common;

use common::*;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;

fn perms() -> Value {
    json!({"alice": {"allowed": ["fetch", "search"]}, "bob": {"allowed": ["fetch"]}})
}

const U: &str = "https://example.com/a";

async fn rig() -> (Rig, Arc<std::sync::atomic::AtomicU64>) {
    let (registry, ran) = counting();
    (start(Auditor::Up, registry, perms()).await, ran)
}

#[tokio::test]
async fn each_attack_is_refused_before_the_tool_runs_and_counted_by_outcome() {
    let (rig, ran) = rig().await;
    let (t, sk) = (now() + 60, &rig.sk);
    let (other_sk, _) = askbar_service::approval::generate_keypair().unwrap();
    // An edit of a signed field after signing: the signature no longer matches.
    let edited = |field: &str, v: Value| {
        let mut b = signed(sk, "fetch", U, "alice", "task1", &format!("e-{field}"), t);
        b["approval"][field] = v;
        b
    };
    let mut stripped = signed(sk, "fetch", U, "alice", "task1", "stripped", t);
    stripped["approval"]["signature"] = json!("");
    let mut garbled = signed(sk, "fetch", U, "alice", "task1", "garbled", t);
    garbled["approval"]["signature"] = json!("not base64!");
    let mut swapped = signed(sk, "fetch", U, "alice", "task1", "swapped", t);
    swapped["arguments"] = json!({"url": "https://example.com/other"});
    let mut dest = signed(sk, "fetch", U, "alice", "task1", "dest", t);
    dest["destination"] = json!("evil.example");
    let mut query = signed(sk, "fetch", U, "alice", "task1", "query", t);
    query["query_data"] = json!(true);
    let mut prov = signed(sk, "fetch", U, "alice", "task1", "prov", t);
    prov["provenance"] = json!("search");
    let cases = [
        ("unsigned", "refused_approval_unsigned", stripped),
        ("garbled", "refused_approval_signature_invalid", garbled),
        ("swapped arguments", "refused_approval_action_swapped", swapped),
        ("swapped destination", "refused_approval_action_swapped", dest),
        ("swapped query_data", "refused_approval_action_swapped", query),
        ("swapped provenance", "refused_approval_action_swapped", prov),
        ("expired", "refused_approval_expired", signed(sk, "fetch", U, "alice", "task1", "expired", now() - 1)),
        ("another key", "refused_approval_signature_invalid", signed(&other_sk, "fetch", U, "alice", "task1", "key", t)),
        ("another user", "refused_approval_signature_invalid", edited("user", json!("bob"))),
        ("another task", "refused_approval_signature_invalid", edited("task_id", json!("task2"))),
        ("another one-time id", "refused_approval_signature_invalid", edited("one_time_id", json!("fresh"))),
        ("a later expiry", "refused_approval_signature_invalid", edited("expires_at", json!(t + 3600))),
        // The approval was made for `search`; the path says `fetch`.
        ("another tool", "refused_approval_action_swapped", signed(sk, "search", U, "alice", "task1", "tool", t)),
    ];
    for (what, outcome, body) in &cases {
        let before = rig.outcome(outcome).await;
        let r = rig.post("fetch", body).await;
        assert_eq!(r.0, 403, "{what}: {r:?}");
        assert_eq!(rig.outcome(outcome).await, before + 1, "{what}: counted as {outcome}");
        assert_eq!(ran.load(Ordering::SeqCst), 0, "{what}: the tool did not run");
    }
    let r = rig.post("search", &signed(sk, "fetch", U, "alice", "task1", "tool2", t)).await;
    assert_eq!(code(&r), (403, "approval_action_swapped"), "an approval for fetch does not open search");
    let r = rig.post("fetch", &json!({"arguments": {"url": U}, "destination": "example.com", "provenance": "user"})).await;
    assert_eq!(code(&r), (400, "bad_request"), "no approval at all");
    assert_eq!((ran.load(Ordering::SeqCst), rig.g.tool_runs.load(Ordering::SeqCst)), (0, 0));
    assert!(rig.spans().is_empty(), "an approval failure comes before the span: nothing is recorded but the outcome count");
}

#[tokio::test]
async fn a_replay_is_refused_sequentially_and_exactly_one_of_many_racing_calls_runs() {
    let (rig, ran) = rig().await;
    let b = signed(&rig.sk, "fetch", U, "alice", "task1", "once", now() + 60);
    assert_eq!(rig.post("fetch", &b).await.0, 200);
    for _ in 0..3 {
        assert_eq!(code(&rig.post("fetch", &b).await), (409, "approval_replayed"));
    }
    assert_eq!((ran.load(Ordering::SeqCst), rig.outcome("refused_approval_replayed").await), (1, 3));

    let rig = Arc::new(rig);
    let b = Arc::new(signed(&rig.sk, "fetch", U, "alice", "task1", "race", now() + 60));
    let tasks: Vec<_> = (0..30)
        .map(|_| {
            let (rig, b) = (rig.clone(), b.clone());
            tokio::spawn(async move { rig.post("fetch", &b).await })
        })
        .collect();
    let mut answers = Vec::new();
    for t in tasks {
        answers.push(t.await.unwrap());
    }
    assert_eq!(answers.iter().filter(|r| r.0 == 200).count(), 1);
    assert_eq!(answers.iter().filter(|r| code(r) == (409, "approval_replayed")).count(), 29);
    assert_eq!(ran.load(Ordering::SeqCst), 2, "one run for the sequential id, one for the race");
}

#[tokio::test]
async fn an_id_is_kept_only_by_a_call_that_passed_every_approval_check() {
    let (rig, ran) = rig().await;
    let t = now() + 60;
    // A call that failed an approval check did not consume the ID: the corrected retry passes.
    let mut swapped = signed(&rig.sk, "fetch", U, "alice", "task1", "c1", t);
    swapped["arguments"] = json!({"url": "https://example.com/other"});
    assert_eq!(code(&rig.post("fetch", &swapped).await), (403, "approval_action_swapped"));
    let mut bad_sig = signed(&rig.sk, "fetch", U, "alice", "task1", "c2", t);
    bad_sig["approval"]["signature"] = json!("");
    assert_eq!(code(&rig.post("fetch", &bad_sig).await), (403, "approval_unsigned"));
    assert_eq!(code(&rig.post("fetch", &signed(&rig.sk, "fetch", U, "alice", "task1", "c3", now() - 1)).await).1, "approval_expired");
    for id in ["c1", "c2", "c3"] {
        assert_eq!(rig.post("fetch", &signed(&rig.sk, "fetch", U, "alice", "task1", id, t)).await.0, 200, "{id}: the retry passes");
    }
    assert_eq!(ran.load(Ordering::SeqCst), 3);

    // A call that passed the approval but failed the permission has consumed it (call.rs checks the permission after
    // `verify`): the same approval is replayed after the permission is granted. A fresh approval then passes.
    let denied = signed(&rig.sk, "fetch", U, "carol", "task1", "c4", t);
    assert_eq!(code(&rig.post("fetch", &denied).await), (403, "tool_not_permitted"));
    rig.push(json!({"carol": {"allowed": ["fetch"]}})).await;
    assert_eq!(code(&rig.post("fetch", &denied).await), (409, "approval_replayed"));
    assert_eq!(rig.post("fetch", &signed(&rig.sk, "fetch", U, "carol", "task1", "c5", t)).await.0, 200);
    assert_eq!(ran.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn an_approval_consumed_by_a_call_the_audit_refused_is_not_reusable() {
    let (registry, ran) = counting();
    let rig = start(Auditor::Busy, registry, perms()).await;
    let b = signed(&rig.sk, "fetch", U, "alice", "task1", "a1", now() + 60);
    assert_eq!(code(&rig.post("fetch", &b).await), (503, "audit_backlog"));
    assert_eq!(code(&rig.post("fetch", &b).await), (409, "approval_replayed"), "the audit check is after the ID is consumed");
    assert_eq!(ran.load(Ordering::SeqCst), 0);
}
