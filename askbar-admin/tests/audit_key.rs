//! The audit key: the admin derives an X25519 keypair from a passphrase, the server seals
//! prompts, replies and provider errors to the public half, and only the admin (with the
//! passphrase) can open them. Written before the code (step 5.1); each test names the step that
//! enables it.

mod common;
use common::T;
use reqwest::Method as M;
use serde_json::json;

/// The keypair for `passphrase` and `salt`: (secret, public).
fn derive_keypair(passphrase: &str, salt: &[u8; 16]) -> ([u8; 32], [u8; 32]) {
    let kp = askbar_admin::audit_key::derive(passphrase, salt, askbar_admin::audit_key::KdfParams::CURRENT).unwrap();
    (kp.secret.to_bytes(), kp.public_bytes())
}

/// What the server does with content: seal it to a public key.
fn server_seal(public_key: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
    askbar_server::seal::seal(public_key, plaintext).unwrap().bytes
}

/// What the admin does to read it: open a sealed box with the secret key.
fn admin_open(secret_key: &[u8; 32], sealed: &[u8]) -> Option<Vec<u8>> {
    askbar_admin::audit_key::open_sealed(&crypto_box::SecretKey::from(*secret_key), sealed)
}

#[test]
fn the_admin_can_decrypt_what_the_server_sealed() {
    let salt = [7u8; 16];
    let (secret, public) = derive_keypair("a long enough passphrase for tests", &salt);
    let sealed = server_seal(&public, b"the question and its answer");
    assert!(!String::from_utf8_lossy(&sealed).contains("question"), "ciphertext only");
    assert_eq!(admin_open(&secret, &sealed).as_deref(), Some(&b"the question and its answer"[..]));
    // The wrong passphrase gives a different key, which opens nothing.
    let (wrong_secret, _) = derive_keypair("another long enough passphrase!!", &salt);
    assert_eq!(admin_open(&wrong_secret, &sealed), None);
}

#[tokio::test]
async fn setup_refuses_a_passphrase_under_twenty_characters() {
    let t: T = common::start(true).await;
    let short = "only nineteen chars";
    assert_eq!(short.chars().count(), 19);
    let (st, _) = t.call(M::POST, "/admin/api/setup/audit-key", json!({ "passphrase": short, "confirm": short })).await;
    assert_eq!(st, 400, "a passphrase under 20 characters is refused");
    let (_, status) = t.call(M::GET, "/admin/api/setup-status", json!({})).await;
    assert_eq!(status["audit_key_set"], false, "and no key was made");

    // Exactly twenty is enough.
    let ok = "exactly twenty chars";
    assert_eq!(ok.chars().count(), 20);
    let (st, body) = t.call(M::POST, "/admin/api/setup/audit-key", json!({ "passphrase": ok, "confirm": ok })).await;
    assert_eq!(st, 200, "{body}");
    assert!(body["key_id"].as_str().is_some_and(|k| k.len() == 16), "an 8-byte hex key id: {body}");
}

#[tokio::test]
async fn the_audit_key_setup_checks_the_passphrase_makes_one_key_and_refuses_a_second() {
    let t: T = common::start(true).await;
    let call = |body: serde_json::Value| t.call(M::POST, "/admin/api/setup/audit-key", body);
    let good = "a passphrase long enough to use";

    // A mismatch is refused, and so is anything under twenty characters, and nothing is made.
    assert_eq!(call(json!({ "passphrase": good, "confirm": "something else entirely here" })).await.0, 400);
    assert_eq!(call(json!({ "passphrase": "too short", "confirm": "too short" })).await.0, 400);
    assert_eq!(t.call(M::GET, "/admin/api/setup-status", json!({})).await.1["audit_key_set"], false);
    assert!(t.admin.db.lock().await.active_audit_key().unwrap().is_none());

    let (st, body) = call(json!({ "passphrase": good, "confirm": good })).await;
    assert_eq!(st, 200, "{body}");
    let key_id = body["key_id"].as_str().unwrap().to_string();
    assert_eq!(t.call(M::GET, "/admin/api/setup-status", json!({})).await.1["audit_key_set"], true);

    // What was kept is public: the key id matches the stored public key, and nothing holds the passphrase.
    let row = t.admin.db.lock().await.active_audit_key().unwrap().unwrap();
    assert_eq!(row.key_id, key_id);
    assert_eq!(askbar_admin::audit_key::key_id(&row.public_key), key_id);
    assert_eq!(row.argon_params, askbar_admin::audit_key::KdfParams::CURRENT.to_text());
    let again = askbar_admin::audit_key::derive(good, &row.salt, askbar_admin::audit_key::KdfParams::CURRENT).unwrap();
    assert_eq!(again.public_bytes(), row.public_key, "the passphrase and the salt give back the same public key");
    let dump: Vec<u8> = {
        let db = t.admin.db.lock().await;
        let mut stmt = db.conn().prepare("SELECT value FROM settings").unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0)).unwrap().flat_map(|v| v.unwrap().into_bytes()).collect()
    };
    assert!(!String::from_utf8_lossy(&dump).contains(good));

    // The public half reached the server in the snapshot; nothing else of the key did.
    let pushed = askbar_server::snapshot::current(&t.server.db.lock().unwrap()).unwrap().unwrap().audit_public_key.expect("the server has the public key");
    assert_eq!(pushed.key_id, key_id);
    assert_eq!(pushed.public_key.len(), 64);
    assert_eq!(pushed.public_key, row.public_key.iter().map(|b| format!("{b:02x}")).collect::<String>());

    // A second call is a conflict and changes nothing (rotation is a later step).
    let (st, _) = call(json!({ "passphrase": "another passphrase, long enough", "confirm": "another passphrase, long enough" })).await;
    assert_eq!(st, 409);
    assert_eq!(t.admin.db.lock().await.active_audit_key().unwrap().unwrap().key_id, key_id);

    // It needs a signed-in session like the rest of the admin API.
    let r = t.http.post(format!("{}/admin/api/setup/audit-key", t.adm)).header("x-askbar-admin", "1").json(&json!({ "passphrase": good, "confirm": good })).send().await.unwrap();
    assert_eq!(r.status(), 401);
}

// ------------------------------------------------- opening sealed content through the admin

const PASS: &str = "a passphrase long enough to use";

/// A trace on the server with a sealed reply, after the admin made its key from `PASS`.
/// Returns the span id that holds the payload and the text sealed into it.
async fn server_with_sealed_reply(t: &T) -> (String, String) {
    use askbar_server::audit::{Kind, SpanFinish, SpanStart};
    let (st, _) = t.call(M::POST, "/admin/api/setup/audit-key", json!({ "passphrase": PASS, "confirm": PASS })).await;
    assert_eq!(st, 200);
    // The server seals with the public key the admin pushed in the snapshot.
    let key = askbar_server::snapshot::current(&t.server.db.lock().unwrap()).unwrap().unwrap().audit_public_key.unwrap();
    let public = askbar_server::snapshot::parse_hex(&key.public_key).unwrap();
    let text = "The reply the server sealed and cannot read.";
    let span = t.server.audit.start_span(SpanStart::new("trace-1", Kind::Turn, "chat"));
    t.server.audit.payload(&span, "reply", askbar_server::seal::seal(&public, text.as_bytes()).unwrap());
    t.server.audit.finish_span(SpanFinish::new(&span, "done"));
    t.server.audit.flush().await;
    (span, text.to_string())
}

async fn access_rows(t: &T) -> Vec<(String, String, bool)> {
    let db = t.admin.db.lock().await;
    let mut stmt = db.conn().prepare("SELECT span_id, key_id, ok FROM audit_access ORDER BY id").unwrap();
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? == 1))).unwrap().map(Result::unwrap).collect();
    rows
}

#[tokio::test]
async fn the_right_passphrase_opens_the_content_and_leaves_an_access_record() {
    let t = common::start(true).await;
    let (span, text) = server_with_sealed_reply(&t).await;
    assert!(access_rows(&t).await.is_empty());

    let (st, body) = t.call(M::POST, &format!("/admin/api/audit/payloads/{span}/open"), json!({ "passphrase": PASS })).await;
    assert_eq!(st, 200, "{body}");
    assert_eq!(body["payloads"][0]["role"], "reply");
    assert_eq!(body["payloads"][0]["text"], text.as_str());
    let key_id = body["payloads"][0]["key_id"].as_str().unwrap().to_string();

    let rows = access_rows(&t).await;
    assert_eq!(rows, [(span.clone(), key_id, true)], "one record, written for the successful open");
    // Nothing the admin keeps holds the passphrase or the opened text.
    let db = t.admin.db.lock().await;
    let mut all = String::new();
    for table in ["audit_access", "audit_keys", "settings"] {
        let mut stmt = db.conn().prepare(&format!("SELECT * FROM {table}")).unwrap();
        let n = stmt.column_count();
        let mut rows = stmt.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            for i in 0..n {
                if let Ok(v) = r.get::<_, String>(i) {
                    all.push_str(&v);
                }
            }
        }
    }
    assert!(!all.contains(PASS) && !all.contains("sealed and cannot read"));
}

#[tokio::test]
async fn a_wrong_passphrase_is_403_is_recorded_and_returns_no_content() {
    let t = common::start(true).await;
    let (span, text) = server_with_sealed_reply(&t).await;
    let wrong = "another passphrase of good length";
    let r = t.http.post(format!("{}/admin/api/audit/payloads/{span}/open", t.adm)).header("x-askbar-admin", "1").header("cookie", &t.cookie).json(&json!({ "passphrase": wrong })).send().await.unwrap();
    assert_eq!(r.status(), 403);
    let body = r.text().await.unwrap();
    assert!(!body.contains(&text) && !body.contains(wrong) && !body.contains("\"text\""), "{body}");
    let rows = access_rows(&t).await;
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].2, "recorded as a failure");
}

#[tokio::test]
async fn five_wrong_passphrases_lock_opening_out_even_for_the_right_one_and_unknown_spans_are_404() {
    let t = common::start(true).await;
    let (span, _) = server_with_sealed_reply(&t).await;
    let open = |pass: &str| {
        let (path, pass) = (format!("/admin/api/audit/payloads/{span}/open"), pass.to_string());
        let t = &t;
        async move { t.call(M::POST, &path, json!({ "passphrase": pass })).await }
    };
    for i in 0..5 {
        assert_eq!(open(&format!("wrong passphrase number {i}, long enough")).await.0, 403);
    }
    let (st, body) = open(PASS).await;
    assert_eq!(st, 429, "locked out, even with the right one: {body}");
    assert!(body["error"].as_str().unwrap_or("").contains("Try again"), "{body}");
    assert_eq!(access_rows(&t).await.len(), 5, "a refused attempt is not an open, so nothing more is recorded");
    // The login is a different lock: signing in still works.
    let r = t.http.post(format!("{}/admin/login", t.adm)).header("x-askbar-admin", "1").json(&json!({ "password": common::PW })).send().await.unwrap();
    assert_eq!(r.status(), 204);

    let t2 = common::start(true).await;
    server_with_sealed_reply(&t2).await;
    assert_eq!(t2.call(M::POST, "/admin/api/audit/payloads/no-such-span/open", json!({ "passphrase": PASS })).await.0, 404);
}

#[tokio::test]
async fn a_span_with_nothing_sealed_opens_to_an_empty_list_without_a_record() {
    use askbar_server::audit::{Kind, SpanStart};
    let t = common::start(true).await;
    server_with_sealed_reply(&t).await;
    let bare = t.server.audit.start_span(SpanStart::new("trace-2", Kind::Gate, "queue"));
    t.server.audit.flush().await;
    let (st, body) = t.call(M::POST, &format!("/admin/api/audit/payloads/{bare}/open"), json!({ "passphrase": "x" })).await;
    assert_eq!((st, body["payloads"].as_array().map(Vec::len)), (200, Some(0)));
    assert!(access_rows(&t).await.is_empty());
}
