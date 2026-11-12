#![cfg(unix)]
//! The manual backup (ADR 0013, plan step 6.2): a bundle of encrypted exports. The server's export and the admin's own are encrypted to
//! the backup public key; the admin holds no passphrase and no private key, so it cannot read what it hands over; the manifest says
//! what is in the bundle; and one file and the passphrase open everything.

mod common;
use askbar_service::backup;
use reqwest::Method as M;
use serde_json::json;
use sha2::{Digest, Sha256};

const PASS: &str = "a backup passphrase of its own 1";
// Markers: each is put where it would be if something leaked, and must be absent from the bundle's bytes.
const ADMIN_KEY: &str = "marker-admin-key-5c3e1f9a";
const CPA_API: &str = "marker-cpa-api-key-77d0e2b1";
const CPA_SECRET: &str = "marker-cpa-secret-4a8f6c20";
const CERT: &str = "marker-certificate-b61d9e07";
const CERT_KEY: &str = "marker-certificate-key-0c52a7d3";
const USER: &str = "marker-user-name-e8b4f1a6";
const PROMPT: &str = "marker system prompt 3d7a90c5";
const CONVERSATION: &str = "marker conversation text 91f2b6e4";

fn hex(d: impl AsRef<[u8]>) -> String {
    d.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// Put every marker where the admin or the server holds it.
async fn mark(t: &common::T) {
    let dir = t.dir.join("secret-files");
    let (admin, caddy) = (dir.join("admin-secrets"), dir.join("caddy-secrets"));
    std::fs::create_dir_all(&admin).unwrap();
    std::fs::create_dir_all(&caddy).unwrap();
    for (d, name, text) in [
        (&admin, "admin.key", ADMIN_KEY),
        (&admin, "cpa-api.key", CPA_API),
        (&admin, "cpa-control.secret", CPA_SECRET),
        (&caddy, "self.crt", CERT),
        (&caddy, "self.key", CERT_KEY),
    ] {
        std::fs::write(d.join(name), text).unwrap();
    }
    *t.admin.admin_secrets_dir.lock().unwrap() = Some(admin);
    t.admin.caddy.lock().unwrap().secrets_dir = Some(caddy);
    assert_eq!(t.call(M::POST, "/api/admin/v1/users", json!({ "name": USER })).await.0, 201);
    assert_eq!(
        t.call(M::PUT, "/api/admin/v1/settings", json!({ "system_prompt": PROMPT, "display": askbar_proto::DisplaySettings::default() }))
            .await
            .0,
        204
    );
    let user: String = t.server.db.lock().unwrap().conn().query_row("SELECT id FROM users WHERE name = ?1", [USER], |r| r.get(0)).unwrap();
    let space = askbar_server::testing::personal_space(&t.server, &user);
    let c = space.lock().create_conversation(&user, "a title").unwrap();
    space.lock().add_message(&user, &c, "user", CONVERSATION).unwrap();
}

async fn set_key(t: &common::T) {
    let b = json!({ "passphrase": PASS, "confirm": PASS });
    assert_eq!(t.call(M::PUT, "/api/admin/v1/setup/backup-key", b).await.0, 200);
}

async fn download(t: &common::T) -> reqwest::Response {
    t.http.get(format!("{}/api/admin/v1/backup", t.adm)).header("cookie", &t.cookie).send().await.unwrap()
}

#[tokio::test]
async fn the_bundle_holds_a_manifest_the_protected_key_and_the_encrypted_exports_admin_first_and_none_of_it_is_readable() {
    let t = common::start(true).await;
    set_key(&t).await;
    mark(&t).await;
    let r = download(&t).await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "application/x-tar");
    assert!(r.headers()["content-disposition"].to_str().unwrap().contains("askbar-backup-"));
    let length: usize = r.headers()["content-length"].to_str().unwrap().parse().unwrap();
    let bytes = r.bytes().await.unwrap();
    assert_eq!(bytes.len(), length, "the length it announced is the length it sent");

    // The entries, in order: the manifest and the protected key, then the admin's own export first and the server's.
    let files = askbar_service::tar::read(&bytes).expect("a tar");
    assert_eq!(files.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["manifest.json", "backup-key.age", "admin.age", "server.age"]);
    let get = |name: &str| files.iter().find(|(n, _)| n == name).unwrap().1.clone();

    // The manifest names the release and the key and says what each part is, by hash and size; it holds nothing secret.
    let manifest: backup::Manifest = serde_json::from_slice(&get("manifest.json")).unwrap();
    let (_, key) = t.call(M::GET, "/api/admin/v1/backup/key", json!(null)).await;
    assert_eq!((manifest.format, manifest.release.as_str()), (backup::BUNDLE_FORMAT, env!("CARGO_PKG_VERSION")));
    assert_eq!(manifest.key_fingerprint, key["fingerprint"].as_str().unwrap());
    assert_eq!(manifest.parts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["admin.age", "server.age"]);
    for p in &manifest.parts {
        let data = get(&p.name);
        assert_eq!((p.bytes, p.sha256.as_str()), (data.len() as u64, hex(Sha256::digest(&data)).as_str()), "{}", p.name);
    }
    assert!(get("backup-key.age").starts_with(b"age-encryption.org/v1\n-> scrypt "), "the protected key is an age file");

    // Nothing the admin or the server holds is in the bundle in the clear.
    for marker in [ADMIN_KEY, CPA_API, CPA_SECRET, CERT, CERT_KEY, USER, PROMPT, CONVERSATION, "SQLite format 3", "AGE-SECRET-KEY", PASS] {
        assert!(!bytes.windows(marker.len()).any(|w| w == marker.as_bytes()), "{marker} is in the bundle in the clear");
    }
    // The admin keeps no way to open it: its database holds the public key and the passphrase-protected private key only.
    let stored: Vec<(String, Vec<u8>)> = {
        let db = t.admin.db.lock().await;
        let mut stmt = db.conn().prepare("SELECT recipient, protected FROM backup_keys").unwrap();
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        rows
    };
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].1, get("backup-key.age"), "the bundle carries the protected key as the admin keeps it");
    assert_eq!(backup::unlock(&stored[0].1, "the admin does not know the passphrase").err(), Some(backup::Error::WrongPassphrase));
}

#[tokio::test]
async fn the_file_and_the_passphrase_open_everything_the_admins_export_and_the_servers() {
    let t = common::start(true).await;
    set_key(&t).await;
    mark(&t).await;
    let bytes = download(&t).await.bytes().await.unwrap();
    let files = askbar_service::tar::read(&bytes).unwrap();
    let get = |name: &str| files.iter().find(|(n, _)| n == name).unwrap().1.clone();
    // One file and one passphrase: the protected key opens with the passphrase, and the private key opens each export.
    let identity = backup::unlock(&get("backup-key.age"), PASS).expect("the passphrase opens the key in the bundle");

    let admin = askbar_service::tar::read(&backup::decrypt(&identity, &get("admin.age")).unwrap()).unwrap();
    assert_eq!(
        admin.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        [
            "admin.db",
            "admin-secrets/admin.key",
            "admin-secrets/cpa-api.key",
            "admin-secrets/cpa-control.secret",
            "caddy-secrets/self.crt",
            "caddy-secrets/self.key"
        ]
    );
    let file = |name: &str| String::from_utf8_lossy(&admin.iter().find(|(n, _)| n == name).unwrap().1).into_owned();
    assert_eq!(file("admin-secrets/admin.key"), ADMIN_KEY);
    assert_eq!(file("admin-secrets/cpa-api.key"), CPA_API);
    assert_eq!(file("admin-secrets/cpa-control.secret"), CPA_SECRET);
    assert_eq!(file("caddy-secrets/self.crt"), CERT);
    assert_eq!(file("caddy-secrets/self.key"), CERT_KEY);
    // The admin's database is whole, secrets included (it is encrypted): its users, its settings and its keyring.
    std::fs::write(t.dir.join("restored-admin.db"), &admin[0].1).unwrap();
    let c = rusqlite::Connection::open(t.dir.join("restored-admin.db")).unwrap();
    let n = |sql: &str| c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(n(&format!("SELECT COUNT(*) FROM users WHERE name = '{USER}'")), 1);
    assert_eq!(n(&format!("SELECT COUNT(*) FROM settings WHERE value = '{PROMPT}'")), 1);
    assert_eq!(n("SELECT COUNT(*) FROM backup_keys WHERE retired_at IS NULL"), 1);
    assert!(n("SELECT COUNT(*) FROM admin_credentials") >= 0);

    let server = askbar_service::tar::read(&backup::decrypt(&identity, &get("server.age")).unwrap()).unwrap();
    assert_eq!(server[0].0, "control.db");
    assert!(server.iter().any(|(n, _)| n.starts_with("spaces/")));
    assert!(
        server.iter().any(|(_, d)| d.windows(CONVERSATION.len()).any(|w| w == CONVERSATION.as_bytes())),
        "the conversation is in a space file"
    );
    // A different key opens nothing.
    let other = backup::generate_with(PASS, 10).unwrap();
    let wrong = backup::unlock(&other.protected, PASS).unwrap();
    assert!(backup::decrypt(&wrong, &get("admin.age")).is_err() && backup::decrypt(&wrong, &get("server.age")).is_err());
}

#[tokio::test]
async fn with_no_backup_key_nothing_is_made_and_a_rotated_key_is_the_one_used_afterwards() {
    let t = common::start(true).await;
    let r = download(&t).await;
    assert_eq!(r.status(), 503);
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["error"], "backup_key_missing");
    set_key(&t).await;
    let first = askbar_service::tar::read(&download(&t).await.bytes().await.unwrap()).unwrap();
    let first_fp = serde_json::from_slice::<backup::Manifest>(&first[0].1).unwrap().key_fingerprint;
    let other = "a second backup passphrase 2222";
    assert_eq!(t.call(M::POST, "/api/admin/v1/backup/rotate-key", json!({ "passphrase": other, "confirm": other })).await.0, 200);
    let second = askbar_service::tar::read(&download(&t).await.bytes().await.unwrap()).unwrap();
    let m = serde_json::from_slice::<backup::Manifest>(&second[0].1).unwrap();
    assert_ne!(m.key_fingerprint, first_fp, "the new key is used");
    // The new bundle carries the new protected key: the old passphrase does not open it, the new one does.
    assert!(backup::unlock(&second[1].1, PASS).is_err());
    let identity = backup::unlock(&second[1].1, other).unwrap();
    assert!(backup::decrypt(&identity, &second[2].1).is_ok() && backup::decrypt(&identity, &second[3].1).is_ok());
    // The older bundle still opens with the older passphrase (ADR 0013).
    assert!(backup::unlock(&first[1].1, PASS).is_ok());
}

#[tokio::test]
async fn the_bundle_holds_no_audit_store_and_leaves_no_work_file_behind() {
    let t = common::start(true).await;
    set_key(&t).await;
    let leftovers = || -> usize {
        std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".backup-"))
            .count()
    };
    let before = leftovers();
    for _ in 0..2 {
        let files = askbar_service::tar::read(&download(&t).await.bytes().await.unwrap()).unwrap();
        assert!(files.iter().all(|(n, _)| !n.contains("audit")), "audit.db is the auditor's own volume, not in this bundle");
    }
    // One the client drops after its first bytes.
    let mut r = download(&t).await;
    assert!(r.chunk().await.unwrap().is_some());
    drop(r);
    for _ in 0..60 {
        if leftovers() <= before {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(leftovers() <= before, "work files were left behind");
}

#[tokio::test]
async fn an_api_key_may_download_it_since_it_is_encrypted_and_the_download_is_recorded_with_the_key_and_the_parts() {
    let t = common::start(true).await;
    set_key(&t).await;
    let key = {
        let db = t.admin.db.lock().await;
        askbar_admin::api_key::create(db.conn(), "ci", askbar_admin::auth::now()).unwrap().1
    };
    let r = t.http.get(format!("{}/api/admin/v1/backup", t.adm)).bearer_auth(&key).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let bytes = r.bytes().await.unwrap();
    assert!(askbar_service::tar::read(&bytes).is_some());
    let rows = t.rig.rows("SELECT outcome, detail_json FROM admin_actions WHERE category = 'backup' ORDER BY at, rowid");
    let text = format!("{rows:?}");
    assert!(text.contains("requested") && text.contains("done") && text.contains("admin.age") && text.contains("fingerprint"), "{text}");
    assert!(!text.contains("AGE-SECRET-KEY") && !text.contains(PASS));
}
