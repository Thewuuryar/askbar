#![cfg(unix)]
//! The backup key (ADR 0013, D22, D21): made from a passphrase, kept as a public key and a passphrase-protected private key, pushed
//! to the server and the alerter, refused without a fresh sign-in or with a weak or audit passphrase, rotated into a new pair, and
//! recorded by fingerprint. The passphrase is stored, logged and returned nowhere.

mod common;
use askbar_service::backup;
use reqwest::Method as M;
use serde_json::{json, Value};

const PASS: &str = "a backup passphrase of its own 1";
const OTHER: &str = "a second backup passphrase 2222";
const AUDIT: &str = "the audit passphrase, used only here";

fn body(p: &str) -> Value {
    json!({ "passphrase": p, "confirm": p })
}

/// What the server holds from the admin's last push.
fn pushed_key(t: &common::T) -> Option<String> {
    let db = t.server.db.lock().unwrap();
    askbar_server::snapshot::current(&db).ok().flatten().and_then(|s| s.backup_public_key)
}

async fn stored(t: &common::T) -> Vec<(String, String, Vec<u8>, Option<i64>)> {
    let db = t.admin.db.lock().await;
    let mut stmt = db.conn().prepare("SELECT recipient, fingerprint, protected, retired_at FROM backup_keys ORDER BY id").unwrap();
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
    rows
}

/// The admin's own action records (the layer that records every change before it is made).
fn actions(t: &common::T) -> Vec<Value> {
    t.rig
        .rows("SELECT id, service, category, action, outcome, detail_json FROM admin_actions ORDER BY at, rowid")
        .into_iter()
        .map(|r| json!({ "id": r[0], "service": r[1], "category": r[2], "action": r[3], "outcome": r[4], "detail": serde_json::from_str::<Value>(r[5].as_str().unwrap_or("null")).unwrap() }))
        .collect()
}

#[tokio::test]
async fn setup_makes_a_key_pair_keeps_the_public_key_and_the_protected_private_key_and_pushes_the_public_key() {
    let t = common::start(true).await;
    let (_, before) = t.call(M::GET, "/api/admin/v1/setup", json!(null)).await;
    assert_eq!(before["backup_key_set"], false);
    assert_eq!(t.call(M::GET, "/api/admin/v1/backup/key", json!(null)).await.1["set"], false);
    assert_eq!(pushed_key(&t), None);

    let (st, v) = t.call(M::PUT, "/api/admin/v1/setup/backup-key", body(PASS)).await;
    assert_eq!(st, 200, "{v}");
    let recipient = v["recipient"].as_str().unwrap().to_string();
    assert!(v["set"] == true && recipient.starts_with("age1") && backup::is_recipient(&recipient), "{v}");
    assert_eq!(v["fingerprint"].as_str().unwrap(), backup::fingerprint(&recipient));
    assert_eq!(v["keys"], 1);
    assert_eq!(t.call(M::GET, "/api/admin/v1/setup", json!(null)).await.1["backup_key_set"], true);

    // The database holds the public key and the protected private key; the protected key opens with the passphrase and with nothing else.
    let rows = stored(&t).await;
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].0.as_str(), rows[0].3), (recipient.as_str(), None));
    let identity = backup::unlock(&rows[0].2, PASS).expect("the passphrase opens the protected key");
    assert_eq!(identity.to_public().to_string(), recipient);
    assert_eq!(backup::unlock(&rows[0].2, OTHER).err(), Some(backup::Error::WrongPassphrase));
    assert!(!rows[0].2.windows(14).any(|w| w == b"AGE-SECRET-KEY"), "no plaintext private key");

    // The public key is where it should be: in the server's snapshot and in what the alerter is given.
    assert_eq!(pushed_key(&t), Some(recipient.clone()));
    let config = {
        let db = t.admin.db.lock().await;
        askbar_admin::alerter_link::build_config(db.conn(), &t.admin.secrets)
    };
    assert_eq!(config.backup_public_key, Some(recipient.clone()));
    // Anything encrypted to it opens with the key the passphrase gives.
    let sealed = backup::encrypt(&recipient, b"an export").unwrap();
    assert_eq!(backup::decrypt(&identity, &sealed).unwrap(), b"an export");

    // A second setup is a 409 and changes nothing.
    let (st, _) = t.call(M::PUT, "/api/admin/v1/setup/backup-key", body(OTHER)).await;
    assert_eq!(st, 409);
    assert_eq!(stored(&t).await.len(), 1);
}

#[tokio::test]
async fn a_passphrase_that_is_short_does_not_match_or_is_the_audit_passphrase_makes_no_key() {
    let t = common::start(true).await;
    let (st, _) = t.call(M::PUT, "/api/admin/v1/setup/audit-key", body(AUDIT)).await;
    assert_eq!(st, 200);
    for (what, b) in [
        ("under 20 characters", body("only nineteen chars")),
        ("not repeated the same", json!({ "passphrase": PASS, "confirm": OTHER })),
        ("the audit passphrase", body(AUDIT)),
        ("an unknown field", json!({ "passphrase": PASS, "confirm": PASS, "extra": 1 })),
    ] {
        let (st, v) = t.call(M::PUT, "/api/admin/v1/setup/backup-key", b).await;
        assert!(st == 400 || st == 422, "{what}: {st} {v}");
        assert!(!v.to_string().contains(PASS) && !v.to_string().contains(AUDIT), "{what}: an answer repeated a passphrase: {v}");
    }
    assert!(stored(&t).await.is_empty(), "no key was made");
    assert_eq!(pushed_key(&t), None);
    // Exactly twenty characters is enough.
    let twenty = "exactly twenty chars";
    assert_eq!(twenty.chars().count(), 20);
    assert_eq!(t.call(M::PUT, "/api/admin/v1/setup/backup-key", body(twenty)).await.0, 200);
}

#[tokio::test]
async fn the_audit_passphrase_is_refused_even_when_that_audit_key_has_been_retired() {
    let t = common::start(true).await;
    assert_eq!(t.call(M::PUT, "/api/admin/v1/setup/audit-key", body(AUDIT)).await.0, 200);
    let new_audit = "a newer audit passphrase, 2";
    let (st, v) = t.call(M::POST, "/api/admin/v1/audit/rotate-key", json!({ "current": AUDIT, "new": new_audit })).await;
    assert_eq!(st, 200, "{v}");
    for audit in [AUDIT, new_audit] {
        let (st, v) = t.call(M::PUT, "/api/admin/v1/setup/backup-key", body(audit)).await;
        assert_eq!(st, 400, "{v}");
        assert!(v["detail"].as_str().unwrap().contains("audit passphrase"), "{v}");
    }
    assert!(stored(&t).await.is_empty());
}

#[tokio::test]
async fn setting_or_rotating_needs_a_fresh_sign_in_from_a_browser_session_and_an_api_key_cannot() {
    let t = common::start(true).await;
    // An old sign-in.
    t.admin.auth.backdate_session(t.cookie.split_once('=').unwrap().1, 11 * 60);
    for (m, path) in [(M::PUT, "/api/admin/v1/setup/backup-key"), (M::POST, "/api/admin/v1/backup/rotate-key")] {
        let (st, v) = t.call(m.clone(), path, body(PASS)).await;
        assert_eq!(st, 403, "{m} {path}: {v}");
        assert!(v["detail"].as_str().unwrap().contains("fresh sign-in"), "{v}");
    }
    assert!(stored(&t).await.is_empty(), "nothing was made");
    // An API key, however fresh, is refused: these are session-only.
    let key = {
        let db = t.admin.db.lock().await;
        askbar_admin::api_key::create(db.conn(), "ci", askbar_admin::auth::now()).unwrap().1
    };
    for (m, path) in [(reqwest::Method::PUT, "/api/admin/v1/setup/backup-key"), (reqwest::Method::POST, "/api/admin/v1/backup/rotate-key")]
    {
        let r = t.http.request(m.clone(), format!("{}{path}", t.adm)).bearer_auth(&key).json(&body(PASS)).send().await.unwrap();
        assert_eq!(r.status().as_u16(), 401, "{m} {path}");
    }
    assert!(stored(&t).await.is_empty());
    // A key may read the public name.
    let r = t.http.get(format!("{}/api/admin/v1/backup/key", t.adm)).bearer_auth(&key).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200);
}

#[tokio::test]
async fn rotating_makes_a_new_pair_retires_the_old_and_every_older_key_opens_only_with_its_own_passphrase() {
    let t = common::start(true).await;
    // There is nothing to rotate yet.
    assert_eq!(t.call(M::POST, "/api/admin/v1/backup/rotate-key", body(PASS)).await.0, 409);
    assert_eq!(t.call(M::PUT, "/api/admin/v1/setup/backup-key", body(PASS)).await.0, 200);
    let (st, v) = t.call(M::POST, "/api/admin/v1/backup/rotate-key", body(OTHER)).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["keys"], 2);
    let rows = stored(&t).await;
    assert_eq!(rows.len(), 2);
    assert!(rows[0].3.is_some() && rows[1].3.is_none(), "the old one is retired, the new one active");
    assert_ne!(rows[0].0, rows[1].0, "a new pair, not the same key again");
    assert_eq!(v["recipient"].as_str(), Some(rows[1].0.as_str()));
    // Each protected key opens with the passphrase that was in force for it, and not with the other.
    assert!(backup::unlock(&rows[0].2, PASS).is_ok() && backup::unlock(&rows[0].2, OTHER).is_err());
    assert!(backup::unlock(&rows[1].2, OTHER).is_ok() && backup::unlock(&rows[1].2, PASS).is_err());
    // The server and the alerter now hold the new public key.
    assert_eq!(pushed_key(&t).as_deref(), Some(rows[1].0.as_str()));
    let config = {
        let db = t.admin.db.lock().await;
        askbar_admin::alerter_link::build_config(db.conn(), &t.admin.secrets)
    };
    assert_eq!(config.backup_public_key.as_deref(), Some(rows[1].0.as_str()));
    // The same rules apply to the new passphrase.
    assert_eq!(t.call(M::POST, "/api/admin/v1/backup/rotate-key", body("too short")).await.0, 400);
    assert_eq!(stored(&t).await.len(), 2);
}

#[tokio::test]
async fn each_change_is_on_record_by_fingerprint_and_no_passphrase_or_key_is_in_a_record_a_response_or_the_database() {
    let t = common::start(true).await;
    let (_, v) = t.call(M::PUT, "/api/admin/v1/setup/backup-key", body(PASS)).await;
    let first = v["fingerprint"].as_str().unwrap().to_string();
    let (_, v) = t.call(M::POST, "/api/admin/v1/backup/rotate-key", body(OTHER)).await;
    let second = v["fingerprint"].as_str().unwrap().to_string();
    assert_ne!(first, second);
    let recorded = actions(&t);
    for (action, fingerprints) in [("backup_key_setup", vec![&first]), ("backup_key_rotate", vec![&second, &first])] {
        let ours: Vec<&Value> = recorded.iter().filter(|r| r["action"] == action).collect();
        assert_eq!(ours.len(), 2, "{action}: requested and done: {recorded:?}");
        let text = serde_json::to_string(&ours).unwrap();
        for f in fingerprints {
            assert!(text.contains(f.as_str()), "{action} names {f}: {text}");
        }
    }
    // Nowhere a passphrase or a private key.
    let everything = format!("{recorded:?}");
    assert!(!everything.contains(PASS) && !everything.contains(OTHER) && !everything.contains("AGE-SECRET-KEY"), "{everything}");
    let db_text = {
        let db = t.admin.db.lock().await;
        let path = db.conn().path().map(|p| p.to_string()).unwrap_or_default();
        let _ = db.conn().execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
        path
    };
    if !db_text.is_empty() {
        let bytes = std::fs::read(&db_text).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains(PASS) && !text.contains(OTHER) && !text.contains("AGE-SECRET-KEY"), "a secret is in admin.db");
    }
}
