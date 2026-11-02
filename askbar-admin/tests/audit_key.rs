//! The audit key: the admin derives an X25519 keypair from a passphrase, the server seals
//! prompts, replies and provider errors to the public half, and only the admin (with the
//! passphrase) can open them. Written before the code (step 5.1); each test names the step that
//! enables it. The helpers below stand in for code that does not exist yet.

mod common;
use common::T;
use reqwest::Method as M;
use serde_json::json;

/// The keypair for `passphrase` and `salt`: (secret, public). Step 5.2 (askbar-admin's audit_key).
#[allow(dead_code)]
fn derive_keypair(_passphrase: &str, _salt: &[u8; 16]) -> ([u8; 32], [u8; 32]) {
    unimplemented!("step 5.2")
}

/// What the server does with content: seal it to a public key. Step 5.7 (askbar-server's seal).
#[allow(dead_code)]
fn server_seal(_public_key: &[u8; 32], _plaintext: &[u8]) -> Vec<u8> {
    unimplemented!("step 5.7")
}

/// What the admin does to read it: open a sealed box with the secret key. Step 5.12.
#[allow(dead_code)]
fn admin_open(_secret_key: &[u8; 32], _sealed: &[u8]) -> Option<Vec<u8>> {
    unimplemented!("step 5.12")
}

#[test]
#[ignore = "enabled in 5.12"]
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
