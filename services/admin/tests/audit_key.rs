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
    let (st, _) = t.call(M::PUT, "/api/admin/v1/setup/audit-key", json!({ "passphrase": short, "confirm": short })).await;
    assert_eq!(st, 400, "a passphrase under 20 characters is refused");
    let (_, status) = t.call(M::GET, "/api/admin/v1/setup", json!({})).await;
    assert_eq!(status["audit_key_set"], false, "and no key was made");

    // Exactly twenty is enough.
    let ok = "exactly twenty chars";
    assert_eq!(ok.chars().count(), 20);
    let (st, body) = t.call(M::PUT, "/api/admin/v1/setup/audit-key", json!({ "passphrase": ok, "confirm": ok })).await;
    assert_eq!(st, 200, "{body}");
    assert!(body["key_id"].as_str().is_some_and(|k| k.len() == 16), "an 8-byte hex key id: {body}");
}

#[tokio::test]
async fn the_audit_key_setup_checks_the_passphrase_makes_one_key_and_refuses_a_second() {
    let t: T = common::start(true).await;
    let call = |body: serde_json::Value| t.call(M::PUT, "/api/admin/v1/setup/audit-key", body);
    let good = "a passphrase long enough to use";

    // A mismatch is refused, and so is anything under twenty characters, and nothing is made.
    assert_eq!(call(json!({ "passphrase": good, "confirm": "something else entirely here" })).await.0, 400);
    assert_eq!(call(json!({ "passphrase": "too short", "confirm": "too short" })).await.0, 400);
    assert_eq!(t.call(M::GET, "/api/admin/v1/setup", json!({})).await.1["audit_key_set"], false);
    assert!(t.admin.db.lock().await.active_audit_key().unwrap().is_none());

    let (st, body) = call(json!({ "passphrase": good, "confirm": good })).await;
    assert_eq!(st, 200, "{body}");
    let key_id = body["key_id"].as_str().unwrap().to_string();
    assert_eq!(t.call(M::GET, "/api/admin/v1/setup", json!({})).await.1["audit_key_set"], true);

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
    let pushed = askbar_server::snapshot::current(&t.server.db.lock().unwrap())
        .unwrap()
        .unwrap()
        .audit_public_key
        .expect("the server has the public key");
    assert_eq!(pushed.key_id, key_id);
    assert_eq!(pushed.public_key.len(), 64);
    assert_eq!(pushed.public_key, row.public_key.iter().map(|b| format!("{b:02x}")).collect::<String>());

    // A second call is a conflict and changes nothing (rotation is a later step).
    let (st, _) = call(json!({ "passphrase": "another passphrase, long enough", "confirm": "another passphrase, long enough" })).await;
    assert_eq!(st, 409);
    assert_eq!(t.admin.db.lock().await.active_audit_key().unwrap().unwrap().key_id, key_id);

    // It needs a signed-in session like the rest of the admin API.
    let r = t
        .http
        .put(format!("{}/api/admin/v1/setup/audit-key", t.adm))
        .header("x-askbar-admin", "1")
        .json(&json!({ "passphrase": good, "confirm": good }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
}

// ------------------------------------------------- opening sealed content through the admin

const PASS: &str = "a passphrase long enough to use";

/// A trace on the server with a sealed reply, after the admin made its key from `PASS`.
/// Returns the span id that holds the payload and the text sealed into it.
async fn server_with_sealed_reply(t: &T) -> (String, String) {
    use askbar_server::audit::Kind;
    let (st, _) = t.call(M::PUT, "/api/admin/v1/setup/audit-key", json!({ "passphrase": PASS, "confirm": PASS })).await;
    assert_eq!(st, 200);
    // The server seals with the public key the admin pushed in the snapshot.
    let key = askbar_server::snapshot::current(&t.server.db.lock().unwrap()).unwrap().unwrap().audit_public_key.unwrap();
    let public = askbar_server::snapshot::parse_hex(&key.public_key).unwrap();
    let text = "The reply the server sealed and cannot read.";
    // A turn span, as the gateway opens it (the server may open gates only), with the reply sealed on it.
    let span = t
        .rig
        .seed("trace-1", Kind::Turn, "chat", Some(("reply", askbar_server::seal::seal(&public, text.as_bytes()).unwrap())), true)
        .await;
    (span, text.to_string())
}

async fn access_rows(t: &T) -> Vec<(String, String, bool)> {
    let db = t.admin.db.lock().await;
    let mut stmt = db.conn().prepare("SELECT span_id, key_id, ok FROM audit_access ORDER BY id").unwrap();
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? == 1))).unwrap().map(Result::unwrap).collect();
    rows
}

/// The passphrase opens a sealed-content session (ADR 0011 rule 8).
async fn unseal(t: &T, pass: &str) -> (u16, serde_json::Value) {
    t.call(M::POST, "/api/admin/v1/audit/session", json!({ "passphrase": pass })).await
}

/// The content of one span, opened with the session's keys.
async fn view(t: &T, span: &str) -> (u16, serde_json::Value) {
    t.call(M::GET, &format!("/api/admin/v1/audit/payloads/{span}"), json!(null)).await
}

#[tokio::test]
async fn the_right_passphrase_opens_a_session_and_each_view_leaves_an_access_record() {
    let t = common::start(true).await;
    let (span, text) = server_with_sealed_reply(&t).await;
    assert!(access_rows(&t).await.is_empty());
    assert_eq!(view(&t, &span).await.0, 403, "sealed until the passphrase is entered");

    let (st, status) = unseal(&t, PASS).await;
    assert_eq!(st, 201, "{status}");
    assert_eq!(status["open"], true);
    assert_eq!(status["keys"][0]["open"], true);
    let (st, body) = view(&t, &span).await;
    assert_eq!(st, 200, "{body}");
    assert_eq!(body["payloads"][0]["role"], "reply");
    assert_eq!(body["payloads"][0]["text"], text.as_str());
    let key_id = body["payloads"][0]["key_id"].as_str().unwrap().to_string();
    // No more prompts while it is open.
    assert_eq!(view(&t, &span).await.0, 200);

    let rows = access_rows(&t).await;
    assert_eq!(
        rows,
        [("(session)".into(), key_id.clone(), true), (span.clone(), key_id.clone(), true), (span.clone(), key_id, true)],
        "the start, then each view"
    );
    // Nothing the admin keeps holds the passphrase, a key or the opened text.
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
async fn a_wrong_passphrase_is_403_is_recorded_and_opens_nothing() {
    let t = common::start(true).await;
    let (span, text) = server_with_sealed_reply(&t).await;
    let wrong = "another passphrase of good length";
    let r = t
        .http
        .post(format!("{}/api/admin/v1/audit/session", t.adm))
        .header("x-askbar-admin", "1")
        .header("cookie", &t.cookie)
        .json(&json!({ "passphrase": wrong }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let body = r.text().await.unwrap();
    assert!(!body.contains(&text) && !body.contains(wrong) && !body.contains("\"text\""), "{body}");
    let rows = access_rows(&t).await;
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].2, "recorded as a failure");
    assert_eq!(view(&t, &span).await.0, 403, "still sealed");
    assert_eq!(t.call(M::GET, "/api/admin/v1/audit/session", json!(null)).await.1["open"], false);
}

#[tokio::test]
async fn five_wrong_passphrases_lock_unsealing_out_even_for_the_right_one_and_unknown_spans_are_404() {
    let t = common::start(true).await;
    server_with_sealed_reply(&t).await;
    for i in 0..5 {
        assert_eq!(unseal(&t, &format!("wrong passphrase number {i}, long enough")).await.0, 403);
    }
    let (st, body) = unseal(&t, PASS).await;
    assert_eq!(st, 429, "locked out, even with the right one: {body}");
    assert!(body["detail"].as_str().unwrap_or("").contains("Try again"), "{body}");
    assert_eq!(body["error"], "too_many_attempts");
    assert_eq!(access_rows(&t).await.len(), 5, "a refused attempt is not an unseal, so nothing more is recorded");
    // The login is a different lock: signing in still works.
    let r = t
        .http
        .post(format!("{}/api/admin/v1/session", t.adm))
        .header("x-askbar-admin", "1")
        .json(&json!({ "password": common::PW }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);

    let t2 = common::start(true).await;
    server_with_sealed_reply(&t2).await;
    assert_eq!(unseal(&t2, PASS).await.0, 201);
    assert_eq!(view(&t2, "no-such-span").await.0, 404);
    assert_eq!(view(&t2, "bad%20id").await.0, 404);
}

#[tokio::test]
async fn a_span_with_nothing_sealed_opens_to_an_empty_list_without_an_access_record() {
    use askbar_server::audit::Kind;
    let t = common::start(true).await;
    server_with_sealed_reply(&t).await;
    unseal(&t, PASS).await;
    let bare = t.rig.seed("trace-2", Kind::Gate, "queue", None, false).await;
    let (st, body) = view(&t, &bare).await;
    assert_eq!((st, body["payloads"].as_array().map(Vec::len)), (200, Some(0)));
    assert_eq!(access_rows(&t).await.len(), 1, "only the start");
}

#[tokio::test]
async fn rotation_adds_a_key_retires_the_old_one_and_each_passphrase_opens_what_its_own_key_sealed() {
    use askbar_server::audit::Kind;
    let t = common::start(true).await;
    let (old_span, old_text) = server_with_sealed_reply(&t).await;
    let old_key = t.admin.db.lock().await.active_audit_key().unwrap().unwrap();
    let new_pass = "a brand new passphrase, also long";

    // The refusals first: wrong current, same passphrase, too short, mismatch.
    let rotate = |cur: &str, new: &str, confirm: &str| {
        let body = json!({ "current": cur, "new": new, "confirm": confirm });
        let t = &t;
        async move { t.call(M::POST, "/api/admin/v1/audit/rotate-key", body).await }
    };
    assert_eq!(rotate("not the current one at all!!", new_pass, new_pass).await.0, 403);
    assert_eq!(rotate(PASS, PASS, PASS).await.0, 400, "must differ");
    assert_eq!(rotate(PASS, "too short", "too short").await.0, 400);
    assert_eq!(rotate(PASS, new_pass, "does not match the new one").await.0, 400);
    assert_eq!(t.admin.db.lock().await.active_audit_key().unwrap().unwrap().key_id, old_key.key_id, "nothing changed yet");

    let (st, body) = rotate(PASS, new_pass, new_pass).await;
    assert_eq!(st, 200, "{body}");
    let new_id = body["key_id"].as_str().unwrap().to_string();
    assert_ne!(new_id, old_key.key_id);
    assert_eq!(body["retired_key_id"], old_key.key_id.as_str());

    // One active key, the old one retired but kept.
    let db = t.admin.db.lock().await;
    assert_eq!(db.active_audit_key().unwrap().unwrap().key_id, new_id);
    assert!(db.audit_key(&old_key.key_id).unwrap().unwrap().retired_at.is_some());
    drop(db);
    // The server was pushed the new public key; it seals with that from now on.
    let pushed = askbar_server::snapshot::current(&t.server.db.lock().unwrap()).unwrap().unwrap().audit_public_key.unwrap();
    assert_eq!(pushed.key_id, new_id);

    // New content is sealed with the new key. Old content stays sealed with the old one.
    let public = askbar_server::snapshot::parse_hex(&pushed.public_key).unwrap();
    let new_text = "A reply sealed after the rotation.";
    let new_span = t
        .rig
        .seed("trace-9", Kind::Turn, "chat", Some(("reply", askbar_server::seal::seal(&public, new_text.as_bytes()).unwrap())), false)
        .await;

    // The new passphrase opens a session that holds the new key only: the old content names the key it needs.
    let (st, s) = unseal(&t, new_pass).await;
    assert_eq!(st, 201, "{s}");
    let (_, b) = view(&t, &new_span).await;
    assert_eq!((b["payloads"][0]["text"].as_str(), b["payloads"][0]["key_id"].as_str()), (Some(new_text), Some(new_id.as_str())));
    let (st, b) = view(&t, &old_span).await;
    assert_eq!(st, 200);
    assert_eq!(b["payloads"][0]["text"], json!(null), "this session has no key for it");
    assert_eq!(b["payloads"][0]["key_id"], old_key.key_id.as_str());
    assert!(b["payloads"][0]["key_retired_at"].as_i64().is_some(), "it says the key is retired, so the person knows which passphrase");

    // The old passphrase adds the old key to the same session (it is not restarted).
    let (st, s) = unseal(&t, PASS).await;
    assert_eq!(st, 200, "added, not a new session: {s}");
    assert_eq!(s["keys"].as_array().unwrap().iter().filter(|k| k["open"] == true).count(), 2);
    let (_, b) = view(&t, &old_span).await;
    assert_eq!(b["payloads"][0]["text"].as_str(), Some(old_text.as_str()), "the old content opens with the old passphrase's key");
}

#[tokio::test]
async fn rotation_needs_an_existing_key_and_a_reachable_server() {
    let t = common::start(true).await;
    let (st, _) = t
        .call(
            M::POST,
            "/api/admin/v1/audit/rotate-key",
            json!({ "current": PASS, "new": "another long enough passphrase", "confirm": "another long enough passphrase" }),
        )
        .await;
    assert_eq!(st, 409, "no key yet");
}

/// Collects everything the process logs on this thread.
#[derive(Clone, Default)]
struct LogBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for LogBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuf {
    type Writer = LogBuf;
    fn make_writer(&'a self) -> LogBuf {
        self.clone()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn no_passphrase_reaches_the_admin_logs_when_unsealing_content_or_rotating() {
    let logs = LogBuf::default();
    let sub = tracing_subscriber::fmt().with_max_level(tracing::Level::TRACE).with_writer(logs.clone()).finish();
    let _guard = tracing::subscriber::set_default(sub);

    let t = common::start(true).await;
    let (span, _) = server_with_sealed_reply(&t).await;
    let wrong = "a wrong passphrase that is long enough";
    let new_pass = "the new passphrase after rotating";
    for pass in [PASS, wrong] {
        unseal(&t, pass).await;
    }
    view(&t, &span).await;
    t.call(M::POST, "/api/admin/v1/audit/rotate-key", json!({ "current": PASS, "new": new_pass, "confirm": new_pass })).await;
    t.call(M::POST, "/api/admin/v1/audit/rotate-key", json!({ "current": wrong, "new": new_pass, "confirm": new_pass })).await;
    let log = String::from_utf8_lossy(&logs.0.lock().unwrap()).into_owned();
    for p in [PASS, wrong, new_pass] {
        assert!(!log.contains(p), "a passphrase is in the logs");
    }
}

#[tokio::test]
async fn the_access_history_lists_every_unseal_and_view_newest_first_without_content_and_needs_a_session() {
    let t = common::start(true).await;
    let (span, text) = server_with_sealed_reply(&t).await;
    let (st, empty) = t.call(M::GET, "/api/admin/v1/audit/access", json!({})).await;
    assert_eq!((st, empty["access"].as_array().map(Vec::len)), (200, Some(0)));

    assert_eq!(unseal(&t, "a wrong passphrase of enough length").await.0, 403);
    assert_eq!(unseal(&t, PASS).await.0, 201);
    assert_eq!(view(&t, &span).await.0, 200);
    assert_eq!(view(&t, &span).await.0, 200);

    let (st, list) = t.call(M::GET, "/api/admin/v1/audit/access", json!({})).await;
    assert_eq!(st, 200);
    let rows = list["access"].as_array().unwrap();
    assert_eq!(
        rows.iter().map(|r| r["ok"].as_bool().unwrap()).collect::<Vec<_>>(),
        [true, true, true, false],
        "newest first; the wrong guess is there too"
    );
    assert_eq!(
        rows.iter().map(|r| r["span_id"].as_str().unwrap()).collect::<Vec<_>>(),
        [span.as_str(), span.as_str(), "(session)", "(session)"]
    );
    assert!(rows.iter().all(|r| r["key_id"].as_str().unwrap().len() == 16 && r["opened_at"].as_i64().unwrap() > 0));
    let body = list.to_string();
    assert!(!body.contains(&text) && !body.contains(PASS) && !body.contains("passphrase"), "no content, no passphrase: {body}");
    assert!(rows[0]["id"].as_i64().unwrap() > rows[1]["id"].as_i64().unwrap());
    // A limit asks for fewer.
    let (_, one) = t.call(M::GET, "/api/admin/v1/audit/access?limit=1", json!({})).await;
    assert_eq!(one["access"].as_array().unwrap().len(), 1);
    // And it is behind the session.
    let r = t.http.get(format!("{}/api/admin/v1/audit/access", t.adm)).send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn the_settings_show_the_active_key_id_and_keep_the_retention_between_7_and_3650_days() {
    let t = common::start(true).await;
    let (_, s) = t.call(M::GET, "/api/admin/v1/settings", json!({})).await;
    assert_eq!((s["audit_retention_days"].clone(), s["audit_key_id"].clone()), (json!(30), json!(null)), "the default, and no key yet");
    server_with_sealed_reply(&t).await;
    let active = t.admin.db.lock().await.active_audit_key().unwrap().unwrap().key_id;
    let (_, s) = t.call(M::GET, "/api/admin/v1/settings", json!({})).await;
    assert_eq!(s["audit_key_id"], active.as_str());

    let disp = serde_json::to_value(askbar_proto::DisplaySettings::default()).unwrap();
    let save = |days: serde_json::Value| json!({ "system_prompt": "x", "display": disp, "audit_retention_days": days });
    for (days, status) in [(7, 204), (90, 204), (3650, 204), (6, 400), (0, 400), (3651, 400)] {
        assert_eq!(t.call(M::PUT, "/api/admin/v1/settings", save(json!(days))).await.0, status, "{days}");
    }
    let (_, s) = t.call(M::PUT, "/api/admin/v1/settings", save(json!(6))).await;
    assert!(s["detail"].as_str().unwrap().contains("at least 7 days"), "{s}");
    let (_, s) = t.call(M::GET, "/api/admin/v1/settings", json!({})).await;
    assert_eq!(s["audit_retention_days"], 3650, "refused values changed nothing; the last accepted one stands");
    assert_eq!(
        askbar_server::snapshot::current(&t.server.db.lock().unwrap()).unwrap().unwrap().audit_retention_days,
        3650,
        "and the server has it"
    );
    // A save that leaves it out keeps it.
    let without = json!({ "system_prompt": "y", "display": disp });
    assert_eq!(t.call(M::PUT, "/api/admin/v1/settings", without).await.0, 204);
    assert_eq!(t.call(M::GET, "/api/admin/v1/settings", json!({})).await.1["audit_retention_days"], 3650);
    // The key id is read-only: sending one changes nothing.
    let mut sneaky = save(json!(30));
    sneaky["audit_key_id"] = json!("ffffffffffffffff");
    assert_eq!(t.call(M::PUT, "/api/admin/v1/settings", sneaky).await.0, 204);
    assert_eq!(t.call(M::GET, "/api/admin/v1/settings", json!({})).await.1["audit_key_id"], active.as_str());
}
