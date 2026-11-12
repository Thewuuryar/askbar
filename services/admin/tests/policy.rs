#![cfg(unix)]
//! The privacy notice and the AI warning (ADR 0012, plan step 5.1): the administrator's choices, how they reach the server in the
//! snapshot, and the version of the notice, which changes when its wording or the audit retention does.

mod common;
use askbar_proto::{notice_version, Policy, DEFAULT_AI_WARNING, DEFAULT_PRIVACY_NOTICE};
use reqwest::Method as M;
use serde_json::{json, Value};

fn settings(extra: Value) -> Value {
    let mut base = json!({ "system_prompt": "p", "display": askbar_proto::DisplaySettings::default() });
    base.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    base
}

/// The policy the server holds from the admin's last push.
fn pushed(t: &common::T) -> Policy {
    let db = t.server.db.lock().unwrap();
    askbar_server::snapshot::current(&db).unwrap().expect("a snapshot").policy.expect("the admin sends a policy")
}

#[tokio::test]
async fn both_are_on_with_the_default_wording_until_the_administrator_chooses() {
    let t = common::start(true).await;
    let (st, s) = t.call(M::GET, "/api/admin/v1/settings", json!(null)).await;
    assert_eq!(st, 200);
    assert_eq!(
        (&s["privacy_notice_enabled"], s["privacy_notice_text"].as_str(), &s["ai_warning_enabled"], s["ai_warning_text"].as_str()),
        (&json!(true), Some(DEFAULT_PRIVACY_NOTICE), &json!(true), Some(DEFAULT_AI_WARNING))
    );
    // Any save pushes a snapshot, and the server holds the default policy for the retention in force.
    assert_eq!(t.save_settings(json!({})).await, 204);
    assert_eq!(pushed(&t), Policy::default_for(30));
}

#[tokio::test]
async fn the_wording_and_whether_each_is_shown_are_kept_pushed_and_trimmed() {
    let t = common::start(true).await;
    let st = t
        .save_settings(json!({
            "privacy_notice_enabled": false,
            "privacy_notice_text": "  We keep what you ask for 30 days.  ",
            "ai_warning_text": "Check the answers.",
        }))
        .await;
    assert_eq!(st, 204);
    let p = pushed(&t);
    assert_eq!((p.notice.enabled, p.notice.text.as_str()), (false, "We keep what you ask for 30 days."));
    assert_eq!((p.ai_warning.enabled, p.ai_warning.text.as_str()), (true, "Check the answers."), "left out keeps it (on)");
    // Read back: what was written, and a save that leaves the fields out keeps them.
    assert_eq!(t.save_settings(json!({ "audit_retention_days": 30 })).await, 204);
    let (_, s) = t.call(M::GET, "/api/admin/v1/settings", json!(null)).await;
    assert_eq!(
        (&s["privacy_notice_enabled"], s["privacy_notice_text"].as_str(), s["ai_warning_text"].as_str()),
        (&json!(false), Some("We keep what you ask for 30 days."), Some("Check the answers."))
    );
    // Writing the default wording, or nothing, goes back to the default.
    assert_eq!(t.save_settings(json!({ "privacy_notice_text": DEFAULT_PRIVACY_NOTICE, "ai_warning_text": "   " })).await, 204);
    let p = pushed(&t);
    assert_eq!((p.notice.text.as_str(), p.ai_warning.text.as_str()), (DEFAULT_PRIVACY_NOTICE, DEFAULT_AI_WARNING));
    let (_, s) = t.call(M::GET, "/api/admin/v1/settings", json!(null)).await;
    assert_eq!(s["ai_warning_text"], DEFAULT_AI_WARNING);
}

#[tokio::test]
async fn the_version_changes_with_the_wording_and_with_the_retention_and_with_nothing_else() {
    let t = common::start(true).await;
    t.save_settings(json!({ "audit_retention_days": 30 })).await;
    let base = pushed(&t).notice;
    assert_eq!((base.retention_days, base.version.clone()), (30, notice_version(DEFAULT_PRIVACY_NOTICE, 30)));
    // Something else changes: the version does not.
    t.save_settings(
        json!({ "audit_retention_days": 30, "system_prompt": "another prompt", "ai_warning_text": "Other.", "history_turns": 5 }),
    )
    .await;
    assert_eq!(pushed(&t).notice.version, base.version);
    // The retention changes: a new version, and the notice says the new number.
    t.save_settings(json!({ "audit_retention_days": 90 })).await;
    let after_retention = pushed(&t).notice;
    assert_eq!(after_retention.retention_days, 90);
    assert_ne!(after_retention.version, base.version);
    // The wording changes: a new one again.
    t.save_settings(json!({ "privacy_notice_text": "New words." })).await;
    let after_words = pushed(&t).notice;
    assert_ne!(after_words.version, after_retention.version);
    assert_eq!(after_words.version, notice_version("New words.", 90));
    // Turning it off and on again does not change what it says, so it does not make people read it again.
    t.save_settings(json!({ "privacy_notice_enabled": false })).await;
    t.save_settings(json!({ "privacy_notice_enabled": true })).await;
    assert_eq!(pushed(&t).notice.version, after_words.version);
}

#[tokio::test]
async fn wording_that_is_too_long_is_refused_and_changes_nothing() {
    let t = common::start(true).await;
    t.save_settings(json!({ "ai_warning_text": "Kept." })).await;
    assert_eq!(t.save_settings(json!({ "privacy_notice_text": "x".repeat(2001) })).await, 400);
    assert_eq!(t.save_settings(json!({ "ai_warning_text": "y".repeat(301) })).await, 400);
    assert_eq!(pushed(&t).ai_warning.text, "Kept.");
    assert_eq!(
        t.save_settings(json!({ "privacy_notice_text": "x".repeat(2000), "ai_warning_text": "y".repeat(300) })).await,
        204,
        "the limits are inclusive"
    );
}

impl common::T {
    /// `PUT /settings` with the given fields over a plain prompt and the default display; the status.
    async fn save_settings(&self, extra: Value) -> u16 {
        self.call(M::PUT, "/api/admin/v1/settings", settings(extra)).await.0
    }
}
