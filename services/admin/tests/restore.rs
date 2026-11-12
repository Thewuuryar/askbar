#![cfg(unix)]
//! Restoring a backup into an empty install (ADR 0013, D21): the bundle is checked before the passphrase is asked for, a newer release is
//! refused naming the release, a used install, a wrong passphrase, a damaged bundle and a stale sign-in are each refused with nothing
//! changed, and what is restored reaches each component and, last, the admin's own pending set.

mod common;
use askbar_service::backup;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use reqwest::Method as M;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const PASS: &str = "a backup passphrase of its own 1";
const WRONG: &str = "not the backup passphrase at all";
const USER: &str = "marker-restored-person-4d9e";
const ADMIN_KEY: &str = "marker-restored-admin-key-81c3";
const ALERTER_STATE: &str = "marker alerter ledger 5e7a21f0";
const LOGIN: &str = "marker account login 90b4c7d2";
const ALERTER_TOKEN: &str = "admalr_test_token";

/// A source install with a backup key and a person, and its bundle.
async fn source_bundle() -> (common::T, Vec<u8>) {
    let t = common::start(true).await;
    let b = json!({ "passphrase": PASS, "confirm": PASS });
    assert_eq!(t.call(M::PUT, "/api/admin/v1/setup/backup-key", b).await.0, 200);
    let dir = t.dir.join("secret-files/admin-secrets");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("admin.key"), ADMIN_KEY).unwrap();
    *t.admin.admin_secrets_dir.lock().unwrap() = Some(dir);
    assert_eq!(t.call(M::POST, "/api/admin/v1/users", json!({ "name": USER })).await.0, 201);
    let r = t.http.get(format!("{}/api/admin/v1/backup", t.adm)).header("cookie", &t.cookie).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let bytes = r.bytes().await.unwrap().to_vec();
    (t, bytes)
}

/// An empty install: the rig's seeded person is removed, and work files have a directory of their own.
async fn empty_target() -> common::T {
    let t = common::start(true).await;
    t.admin.db.lock().await.conn().execute("DELETE FROM users", []).unwrap();
    std::fs::create_dir_all(t.dir.join("work")).unwrap();
    *t.admin.work_dir.lock().unwrap() = Some(t.dir.join("work"));
    t
}

impl common::T {
    async fn upload(&self, bundle: Vec<u8>) -> (u16, Value) {
        let r = self
            .http
            .post(format!("{}/api/admin/v1/restore/inspect", self.adm))
            .header("x-askbar-admin", "1")
            .header("cookie", &self.cookie)
            .body(bundle)
            .send()
            .await
            .unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }

    async fn restore(&self, id: &str, passphrase: &str) -> (u16, Value) {
        self.call(M::POST, "/api/admin/v1/restore", json!({ "restore_id": id, "passphrase": passphrase })).await
    }

    fn leftovers(&self) -> Vec<String> {
        std::fs::read_dir(self.dir.join("work"))
            .map(|d| d.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default()
    }
}

/// A stand-in alerter that keeps what it is given in `POST /imports`.
async fn fake_alerter(t: &common::T) -> Arc<Mutex<Vec<Vec<u8>>>> {
    let got: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
    let handler = |State(got): State<Arc<Mutex<Vec<Vec<u8>>>>>, h: HeaderMap, body: axum::body::Bytes| async move {
        if h.get("authorization").and_then(|v| v.to_str().ok()) != Some(&format!("Bearer {ALERTER_TOKEN}")) {
            return StatusCode::UNAUTHORIZED;
        }
        got.lock().unwrap().push(body.to_vec());
        StatusCode::OK
    };
    let url =
        common::serve(axum::Router::new().route("/api/alerter/v1/imports", axum::routing::post(handler)).with_state(got.clone())).await;
    t.admin.alerter.set(askbar_admin::alerter_link::AlerterLink::new(&url, ALERTER_TOKEN.into())).ok().unwrap();
    got
}

/// Stand-ins for the source's alerter and supervisor, so the bundle has the parts and the target can be checked against them.
async fn with_alerter_and_cpa(t: &common::T) {
    let recipient = {
        let db = t.admin.db.lock().await;
        askbar_admin::db::active_backup_key(db.conn()).unwrap().unwrap().recipient
    };
    let handler = move |State(recipient): State<String>| async move {
        let mut tar = Vec::new();
        askbar_service::tar::append(&mut tar, "ledger.json", ALERTER_STATE.as_bytes(), 0);
        askbar_service::tar::finish(&mut tar);
        (StatusCode::OK, backup::encrypt(&recipient, &tar).unwrap())
    };
    let url = common::serve(axum::Router::new().route("/api/alerter/v1/exports", axum::routing::post(handler)).with_state(recipient)).await;
    t.admin.alerter.set(askbar_admin::alerter_link::AlerterLink::new(&url, ALERTER_TOKEN.into())).ok().unwrap();
    // `cpa`: a thread answers each request file, as the supervisor does.
    let dir = t.dir.join("cpa-config");
    std::fs::create_dir_all(&dir).unwrap();
    t.admin.set_cpa(askbar_admin::cpa::CpaLink::in_dir(
        "http://127.0.0.1:1",
        askbar_admin::cpa::Keys { api_key: "k".into(), control_secret: "s".into() },
        dir.clone(),
    ));
    std::thread::spawn(move || {
        let mut seen = (String::new(), String::new());
        for _ in 0..1200 {
            std::thread::sleep(std::time::Duration::from_millis(25));
            if let Ok(text) = std::fs::read_to_string(dir.join("export-request")) {
                let req: Value = serde_json::from_str(&text).unwrap();
                if req["id"].as_str().unwrap() != seen.0 {
                    seen.0 = req["id"].as_str().unwrap().to_string();
                    let mut tar = Vec::new();
                    askbar_service::tar::append(&mut tar, "claude-a.json", LOGIN.as_bytes(), 0);
                    askbar_service::tar::finish(&mut tar);
                    std::fs::write(dir.join("export.age"), backup::encrypt(req["recipient"].as_str().unwrap(), &tar).unwrap()).unwrap();
                    std::fs::write(dir.join("export-done"), json!({ "id": seen.0, "ok": true }).to_string()).unwrap();
                }
            }
            if let Ok(text) = std::fs::read_to_string(dir.join("import-request")) {
                let req: Value = serde_json::from_str(&text).unwrap();
                if req["id"].as_str().unwrap() != seen.1 {
                    seen.1 = req["id"].as_str().unwrap().to_string();
                    // Keep what was handed over, as the supervisor does with the files, then empty the archive.
                    let tar = std::fs::read(dir.join("import.tar")).unwrap();
                    std::fs::write(dir.join("imported-copy"), tar).unwrap();
                    std::fs::write(dir.join("import.tar"), []).unwrap();
                    std::fs::write(dir.join("import-done"), json!({ "id": seen.1, "ok": true }).to_string()).unwrap();
                }
            }
        }
    });
}

#[tokio::test]
async fn a_backup_restores_part_by_part_and_the_admins_own_export_waits_for_the_restart() {
    let (source, bundle) = source_bundle().await;
    // The source's alerter and cpa have something to give.
    with_alerter_and_cpa(&source).await;
    let bundle = {
        let r = source.http.get(format!("{}/api/admin/v1/backup", source.adm)).header("cookie", &source.cookie).send().await.unwrap();
        assert_eq!(r.status(), 200);
        let _ = bundle;
        r.bytes().await.unwrap().to_vec()
    };
    let source_owner: String = source
        .server
        .db
        .lock()
        .unwrap()
        .conn()
        .query_row("SELECT id FROM users ORDER BY created_at, id LIMIT 1", [], |r| r.get(0))
        .unwrap();

    let target = empty_target().await;
    let got = fake_alerter(&target).await;
    with_cpa_target(&target);
    let (status, view) = target.upload(bundle).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["release"], env!("CARGO_PKG_VERSION"));
    assert_eq!(view["parts"], json!(["admin.age", "server.age", "alerter.age", "cpa.age", "audit.age"]));
    assert!(view["key_fingerprint"].as_str().unwrap().len() == 19);
    let id = view["restore_id"].as_str().unwrap().to_string();

    let (status, done) = target.restore(&id, PASS).await;
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["restored"], json!(["server", "alerter", "cpa", "auditor", "admin"]));
    assert_eq!(done["not_restored"], json!([]));
    assert_eq!(done["restarting"], json!(false), "no restarter in this rig");
    // The server is the backed-up one; the alerter and cpa were handed their parts, opened.
    let owner: String = target
        .server
        .db
        .lock()
        .unwrap()
        .conn()
        .query_row("SELECT id FROM users ORDER BY created_at, id LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(owner, source_owner);
    let alerter = askbar_service::tar::read(&got.lock().unwrap()[0]).unwrap();
    assert_eq!(alerter, vec![("ledger.json".to_string(), ALERTER_STATE.as_bytes().to_vec())]);
    let cpa = askbar_service::tar::read(&std::fs::read(target.dir.join("cpa-config/imported-copy")).unwrap()).unwrap();
    assert_eq!(cpa, vec![("claude-a.json".to_string(), LOGIN.as_bytes().to_vec())]);
    assert_eq!(std::fs::read(target.dir.join("cpa-config/import.tar")).unwrap().len(), 0, "the opened logins do not stay on the volume");
    // The auditor's store is the backup's: the source's own record of its download is in it.
    let downloads: i64 =
        target.rig.rows("SELECT COUNT(*) FROM admin_actions WHERE category = 'backup' AND action = 'download'").len() as i64;
    assert!(downloads >= 1, "the source's records came over");
    // The admin's own set is pending, whole, next to where its database is, and nothing else of the upload is kept.
    let pending = target.dir.join("work").join(askbar_admin::restore::PENDING_DIR);
    assert!(pending.join("admin.db").is_file() && pending.join("READY").is_file());
    assert_eq!(std::fs::read_to_string(pending.join("admin-secrets/admin.key")).unwrap(), ADMIN_KEY);
    assert!(target.leftovers().iter().all(|n| !n.starts_with(".restore-upload-")), "{:?}", target.leftovers());
    // The restart's swap: the old database and key are kept, the backup's are in place, and the person is there.
    let (db, secrets) = (target.dir.join("work/admin.db"), target.dir.join("live-secrets"));
    std::fs::create_dir_all(&secrets).unwrap();
    std::fs::write(&db, b"the install's own database").unwrap();
    std::fs::write(secrets.join("admin.key"), "the install's own key").unwrap();
    assert!(askbar_admin::restore::apply_pending(&db, &secrets, None).unwrap());
    assert_eq!(std::fs::read(target.dir.join("work/admin.db.before-restore")).unwrap(), b"the install's own database");
    assert_eq!(std::fs::read_to_string(secrets.join("admin.key.before-restore")).unwrap(), "the install's own key");
    assert_eq!(std::fs::read_to_string(secrets.join("admin.key")).unwrap(), ADMIN_KEY);
    let restored = askbar_admin::AdminDb::open(&db).unwrap();
    let people: i64 = restored.conn().query_row("SELECT COUNT(*) FROM users WHERE name = ?1", [USER], |r| r.get(0)).unwrap();
    assert_eq!(people, 1, "the admin's database is the backup's");
    assert!(!pending.exists());
    assert!(askbar_admin::restore::take_done_marker(&db) && !askbar_admin::restore::take_done_marker(&db), "reported once");
    // Nothing the passphrase or the key could be is in the answers or the records.
    assert!(!done.to_string().contains(PASS));
    let rows = target.rig.rows("SELECT outcome, detail_json FROM admin_actions WHERE category = 'backup' ORDER BY at, rowid");
    let text = format!("{rows:?}");
    assert!(text.contains("restore") && !text.contains(PASS) && !text.contains("AGE-SECRET-KEY"), "{text}");
}

/// The target's `cpa`: the same stand-in as the source's, for the import request.
fn with_cpa_target(t: &common::T) {
    let dir = t.dir.join("cpa-config");
    std::fs::create_dir_all(&dir).unwrap();
    t.admin.set_cpa(askbar_admin::cpa::CpaLink::in_dir(
        "http://127.0.0.1:1",
        askbar_admin::cpa::Keys { api_key: "k".into(), control_secret: "s".into() },
        dir.clone(),
    ));
    std::thread::spawn(move || {
        let mut seen = String::new();
        for _ in 0..1200 {
            std::thread::sleep(std::time::Duration::from_millis(25));
            let Ok(text) = std::fs::read_to_string(dir.join("import-request")) else { continue };
            let req: Value = serde_json::from_str(&text).unwrap();
            if req["id"].as_str().unwrap() != seen {
                seen = req["id"].as_str().unwrap().to_string();
                let tar = std::fs::read(dir.join("import.tar")).unwrap();
                std::fs::write(dir.join("imported-copy"), tar).unwrap();
                std::fs::write(dir.join("import.tar"), []).unwrap();
                std::fs::write(dir.join("import-done"), json!({ "id": seen, "ok": true }).to_string()).unwrap();
            }
        }
    });
}

#[tokio::test]
async fn a_backup_from_a_newer_release_is_refused_naming_the_release_before_the_passphrase_is_asked_for() {
    let (_source, bundle) = source_bundle().await;
    let mut entries = askbar_service::tar::read(&bundle).unwrap();
    let mut manifest: backup::Manifest = serde_json::from_slice(&entries[0].1).unwrap();
    manifest.release = "99.1.0".into();
    entries[0].1 = serde_json::to_vec(&manifest).unwrap();
    let mut newer = Vec::new();
    for (n, d) in &entries {
        askbar_service::tar::append(&mut newer, n, d, 0);
    }
    askbar_service::tar::finish(&mut newer);
    let target = empty_target().await;
    let (status, body) = target.upload(newer).await;
    assert_eq!((status, body["error"].as_str(), body["release"].as_str()), (409, Some("backup_newer_release"), Some("99.1.0")), "{body}");
    assert!(body["detail"].as_str().unwrap().contains("99.1.0"));
    // Nothing was kept, so there is nothing to give a passphrase to.
    assert_eq!(target.restore("anything", PASS).await.0, 409);
    assert!(target.leftovers().is_empty(), "{:?}", target.leftovers());
    // An older or the same release is taken.
    manifest.release = "0.0.1".into();
    entries[0].1 = serde_json::to_vec(&manifest).unwrap();
    let mut older = Vec::new();
    for (n, d) in &entries {
        askbar_service::tar::append(&mut older, n, d, 0);
    }
    askbar_service::tar::finish(&mut older);
    assert_eq!(target.upload(older).await.0, 200);
}

#[tokio::test]
async fn a_wrong_passphrase_is_refused_five_lock_it_and_nothing_is_changed() {
    let (_source, bundle) = source_bundle().await;
    let target = empty_target().await;
    let (_, view) = target.upload(bundle).await;
    let id = view["restore_id"].as_str().unwrap().to_string();
    let before: i64 = target.server.db.lock().unwrap().conn().query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0)).unwrap();
    for _ in 0..5 {
        let (status, body) = target.restore(&id, WRONG).await;
        assert_eq!(status, 403, "{body}");
        assert!(!body.to_string().contains(WRONG), "the guess is not repeated");
    }
    // Locked now, even for the right one.
    let (status, body) = target.restore(&id, PASS).await;
    assert_eq!((status, body["error"].as_str()), (429, Some("too_many_attempts")), "{body}");
    assert!(!target.dir.join("work").join(askbar_admin::restore::PENDING_DIR).exists());
    let after: i64 = target.server.db.lock().unwrap().conn().query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0)).unwrap();
    assert_eq!(before, after);
}

#[tokio::test]
async fn a_used_install_a_damaged_bundle_and_a_stale_sign_in_are_each_refused() {
    let (_source, bundle) = source_bundle().await;
    // The admin has a person: not empty, refused before the upload is read.
    let used = common::start(true).await;
    let (status, body) = used.upload(bundle.clone()).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("not_empty")), "{body}");
    // The server has a conversation: the admin side is empty, so the bundle is taken, and the server says no.
    let target = empty_target().await;
    {
        let owner: String = target.server.db.lock().unwrap().conn().query_row("SELECT id FROM users LIMIT 1", [], |r| r.get(0)).unwrap();
        let space = askbar_server::testing::personal_space(&target.server, &owner);
        let c = space.lock().create_conversation(&owner, "mine").unwrap();
        space.lock().add_message(&owner, &c, "user", "keep me").unwrap();
    }
    let (_, view) = target.upload(bundle.clone()).await;
    let (status, body) = target.restore(view["restore_id"].as_str().unwrap(), PASS).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("not_empty")), "{body}");
    assert!(!target.dir.join("work").join(askbar_admin::restore::PENDING_DIR).exists(), "the admin's own set waits for the rest to work");
    // Damaged: a byte flipped in a part, or the upload cut short.
    let target = empty_target().await;
    let mut flipped = bundle.clone();
    let at = flipped.len() - 5000;
    flipped[at] ^= 0xff;
    let (status, body) = target.upload(flipped).await;
    assert_eq!((status, body["error"].as_str()), (400, Some("bad_request")), "{body}");
    assert_eq!(target.upload(bundle[..bundle.len() / 2].to_vec()).await.0, 400);
    assert_eq!(target.upload(b"not a backup at all".to_vec()).await.0, 400);
    assert!(target.leftovers().is_empty(), "{:?}", target.leftovers());
    // A sign-in older than ten minutes, and an API key, cannot.
    let token = target.cookie.split_once('=').unwrap().1.to_string();
    target.admin.auth.backdate_session(&token, 11 * 60);
    let (status, body) = target.upload(bundle.clone()).await;
    assert_eq!(status, 403, "{body}");
    let key = {
        let db = target.admin.db.lock().await;
        askbar_admin::api_key::create(db.conn(), "ci", askbar_admin::auth::now()).unwrap().1
    };
    for path in ["/api/admin/v1/restore/inspect", "/api/admin/v1/restore"] {
        let r = target
            .http
            .post(format!("{}{path}", target.adm))
            .bearer_auth(&key)
            .header("x-askbar-admin", "1")
            .body(bundle.clone())
            .send()
            .await
            .unwrap();
        assert!(matches!(r.status().as_u16(), 401 | 403), "{path}: {}", r.status());
    }
}

#[test]
fn an_incomplete_pending_set_is_removed_and_nothing_is_swapped() {
    let dir = std::env::temp_dir().join(format!("restore-pending-{}", ulid::Ulid::new()));
    let pending = dir.join(askbar_admin::restore::PENDING_DIR);
    std::fs::create_dir_all(&pending).unwrap();
    std::fs::write(pending.join("admin.db"), b"half").unwrap();
    let db = dir.join("admin.db");
    std::fs::write(&db, b"live").unwrap();
    assert!(!askbar_admin::restore::apply_pending(&db, &dir.join("secrets"), None).unwrap(), "no READY: not applied");
    assert_eq!(std::fs::read(&db).unwrap(), b"live");
    assert!(!pending.exists());
    assert!(!askbar_admin::restore::apply_pending(&db, &dir.join("secrets"), None).unwrap(), "nothing pending is fine");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_admin_expects_the_restored_chain_so_a_restore_raises_no_rollback_or_rewrite() {
    let (_source, bundle) = source_bundle().await;
    let target = empty_target().await;
    // The install's own earlier records: the chain the admin has seen is the one that is about to be replaced.
    let link = target.admin.auditor.get().cloned().unwrap();
    target.admin.chain.check(&target.admin, &link, true).await;
    let (_, view) = target.upload(bundle).await;
    let (status, done) = target.restore(view["restore_id"].as_str().unwrap(), PASS).await;
    assert_eq!(status, 200, "{done}");
    assert!(done["restored"].as_array().unwrap().iter().any(|p| p == "auditor"), "{done}");
    // The stored head is the restored chain's: the next check finds it as it was left, and raises nothing.
    target.admin.chain.check(&target.admin, &link, true).await;
    assert_eq!(target.admin.chain.status()["state"], "ok", "{}", target.admin.chain.status());
    assert!(target.admin.notifier.notes().iter().all(|n| n.kind != "chain_mismatch"), "{:?}", target.admin.notifier.notes());
    // The set the admin swaps in at its restart expects it too.
    let pending = target.dir.join("work").join(askbar_admin::restore::PENDING_DIR).join("admin.db");
    let db = askbar_admin::AdminDb::open(&pending).unwrap();
    let (seq, hash): (i64, String) =
        db.conn().query_row("SELECT seq, hash FROM audit_chain_head WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    // It is an entry of the auditor's chain as it stands (the chain has grown since: the restore's own record).
    let (code, entry) = link.get(&format!("/chain/entries/{seq}")).await.unwrap();
    assert_eq!((code, entry["hash"].as_str()), (200, Some(hash.as_str())), "the pending database expects an entry the auditor has");
    // And it is the anchor that does it: a head the admin expected from before is a rewrite.
    target.admin.db.lock().await.conn().execute("UPDATE audit_chain_head SET hash = 'a-hash-from-the-old-chain' WHERE id = 1", []).unwrap();
    target.admin.chain.check(&target.admin, &link, true).await;
    assert_eq!(target.admin.chain.status()["state"], "rewritten");
}
