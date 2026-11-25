//! No call runs unrecorded (ADR 0010 rule 3): an auditor that is down, slow (past the acknowledgement wait) or answers an
//! error refuses the call with `503 audit_backlog`, and the tool runs in none of those cases. The allowed case passes.

mod common;

use common::*;
use serde_json::json;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn an_auditor_that_is_down_slow_or_failing_refuses_the_call_and_the_tool_never_runs() {
    for (name, mode) in [("down", Auditor::Down), ("slow", Auditor::Slow), ("error", Auditor::Fail), ("backlog", Auditor::Busy)] {
        let (registry, ran) = counting();
        let rig = start(mode, registry, json!({"alice": {"allowed": ["fetch"]}})).await;
        let t = std::time::Instant::now();
        let r = rig.post("fetch", &signed(&rig.sk, "fetch", "https://example.com/a", "alice", "t1", "id1", now() + 60)).await;
        assert_eq!(code(&r), (503, "audit_backlog"), "{name}");
        assert!(t.elapsed() < std::time::Duration::from_millis(2500), "{name}: the call does not wait for a 3 s answer");
        assert_eq!((ran.load(Ordering::SeqCst), rig.g.tool_runs.load(Ordering::SeqCst)), (0, 0), "{name}: the tool did not run");
        assert_eq!(rig.outcome("refused_audit_unavailable").await, 1, "{name}");
        assert_eq!(rig.outcome("tool_ok").await, 0, "{name}");
    }
}

#[tokio::test]
async fn an_auditor_that_acknowledges_lets_the_tool_run_after_the_span() {
    let (registry, ran) = counting();
    let rig = start(Auditor::Up, registry, json!({"alice": {"allowed": ["fetch"]}})).await;
    let r = rig.post("fetch", &signed(&rig.sk, "fetch", "https://example.com/a", "alice", "t1", "id1", now() + 60)).await;
    assert_eq!(r, (200, json!({"result": {"ran": true}, "untrusted": true})));
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    assert_eq!(rig.spans()[0].2, 0, "the tool had not run when the auditor was asked for the span");
    assert_eq!(rig.outcome("tool_ok").await, 1);
}
