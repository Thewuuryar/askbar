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
#[ignore = "enabled in 5.4"]
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
