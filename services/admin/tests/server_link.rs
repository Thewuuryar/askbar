#![cfg(unix)]
//! The admin side of the server link, against a real in-process server.

use askbar_admin::server_link::{self, ServerLink};
use askbar_admin::AdminDb;
use askbar_server::{secrets::Secrets, AppState};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("askbar-link-{tag}-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Dirs {
    root: PathBuf,
}

impl Dirs {
    fn new(tag: &str) -> Dirs {
        Dirs { root: temp(tag) }
    }

    /// What bootstrap makes (ADR 0049): the admin's token and the server's hash of it.
    fn install(&self) -> (String, PathBuf) {
        let (token, hash) = askbar_bootstrap::install_for_tests(&self.root);
        (std::fs::read_to_string(token).unwrap(), hash)
    }
}

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// ------------------------------------------------------------------ the token

#[test]
fn the_admin_reads_the_token_bootstrap_made_and_the_server_holds_only_its_hash() {
    let d = Dirs::new("token");
    let (token, hash_file) = d.install();
    let token_dir = d.root.join("admin-token");

    assert_eq!(server_link::read_token(&token_dir).unwrap(), token);
    assert_eq!(server_link::token_file(&token_dir), token_dir.join("call/admin-server.token"));
    assert!(token.starts_with("mgt_"));
    assert_eq!(URL_SAFE_NO_PAD.decode(token.trim_start_matches("mgt_")).unwrap().len(), 32, "32 random bytes");
    let hash = std::fs::read_to_string(&hash_file).unwrap();
    assert_eq!(hash.trim(), server_link::sha256_hex(&token));
    assert!(!hash.contains(token.trim_start_matches("mgt_")), "the hash file holds no raw token");
}

#[test]
fn without_a_token_file_the_admin_says_to_run_bootstrap_and_never_makes_one() {
    let d = Dirs::new("missing");
    let token_dir = d.root.join("admin-token");
    let e = server_link::read_token(&token_dir).unwrap_err();
    assert!(e.to_string().contains("docker compose up"), "{e}");
    assert!(!token_dir.exists(), "nothing was made");

    let (_, _) = d.install();
    let file = server_link::token_file(&token_dir);
    std::fs::set_permissions(file.parent().unwrap(), std::fs::Permissions::from_mode(0o700)).unwrap();
    // The file is 0400 as bootstrap makes it; the directory is open now, so replace the file.
    std::fs::remove_file(&file).unwrap();
    std::fs::write(&file, "  \n").unwrap();
    assert!(server_link::read_token(&token_dir).unwrap_err().to_string().contains("is empty"));
}

#[test]
fn the_old_token_file_is_removed_once() {
    let d = Dirs::new("old");
    let secrets = d.root.join("admin-secrets");
    std::fs::create_dir_all(&secrets).unwrap();
    std::fs::write(secrets.join("server.token"), "mgt_old").unwrap();
    std::fs::write(secrets.join("master.key"), "keep").unwrap();
    assert!(server_link::remove_old_token(&secrets).unwrap());
    assert!(!secrets.join("server.token").exists());
    assert!(secrets.join("master.key").exists(), "nothing else is touched");
    assert!(!server_link::remove_old_token(&secrets).unwrap(), "a second start has nothing to remove");
}

// ------------------------------------------------------------------ the server

fn server_state() -> Arc<AppState> {
    let db = askbar_server::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), askbar_server::MIN_CLIENT)
}

async fn serve_on(listener: tokio::net::TcpListener, state: Arc<AppState>, hash_file: PathBuf) {
    let app = askbar_server::server_api::router(state, hash_file);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
}

async fn free_port() -> std::net::SocketAddr {
    tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap()
}

fn admin_db_with_data() -> AdminDb {
    let db = AdminDb::open_in_memory().unwrap();
    db.conn()
        .execute_batch(
            "INSERT INTO providers (id, name, type, base_url, model, max_tokens, created_at) VALUES ('p1','mock','openai','http://x/v1','m1',100,0);
             INSERT INTO settings VALUES ('system_prompt', 'Be brief.');
             INSERT INTO settings VALUES ('default_provider_id', 'p1');
             INSERT INTO settings VALUES ('audit_retention_days', '14');
             INSERT INTO revoked_clients VALUES ('dev-9', 0);
             UPDATE budgets SET daily_tokens = 5000, kill_switch = 1;",
        )
        .unwrap();
    db
}

#[test]
fn the_snapshot_is_built_from_admin_db() {
    let db = admin_db_with_data();
    db.conn()
        .execute(
            "INSERT INTO pair_codes (hash, user_id, client_name, expires_at, created_at) VALUES ('h1','u1','laptop', 4000000000, 0)",
            [],
        )
        .unwrap();
    db.conn()
        .execute("INSERT INTO pair_codes (hash, user_id, client_name, expires_at, created_at) VALUES ('old','u1','x', 1, 0)", [])
        .unwrap();
    let s = server_link::build_snapshot(db.conn()).unwrap();
    assert_eq!(s.providers.len(), 1);
    assert_eq!((s.providers[0].id.as_str(), s.providers[0].model.as_str(), s.providers[0].max_tokens), ("p1", "m1", Some(100)));
    assert_eq!((s.default_provider_id.as_deref(), s.system_prompt.as_deref(), s.audit_retention_days), (Some("p1"), Some("Be brief."), 14));
    assert_eq!((s.budget.daily_tokens, s.budget.monthly_tokens, s.kill_switch), (Some(5000), None, true));
    assert_eq!(s.revoked_client_ids, ["dev-9"]);
    assert_eq!(s.pair_codes.len(), 1, "an expired code is not sent");
    assert_eq!(s.pair_codes[0].hash, "h1");
    // A fresh database gives a valid, empty snapshot.
    let empty = server_link::build_snapshot(AdminDb::open_in_memory().unwrap().conn()).unwrap();
    assert_eq!((empty.providers.len(), empty.audit_retention_days, empty.kill_switch), (0, 30, false));
}

#[test]
fn the_snapshot_carries_the_active_audit_public_key_and_only_that() {
    let db = AdminDb::open_in_memory().unwrap();
    assert_eq!(server_link::build_snapshot(db.conn()).unwrap().audit_public_key, None, "no key made yet");
    db.insert_audit_key(&askbar_admin::db::AuditKeyRow {
        key_id: "0011223344556677".into(),
        public_key: [0xab; 32],
        salt: [9; 16],
        argon_params: "m=65536,t=3,p=1".into(),
        created_at: 1,
        retired_at: None,
    })
    .unwrap();
    let s = server_link::build_snapshot(db.conn()).unwrap();
    let json = serde_json::to_string(&s).unwrap();
    let k = s.audit_public_key.expect("the active key");
    assert_eq!((k.key_id.as_str(), k.public_key.as_str()), ("0011223344556677", "ab".repeat(32).as_str()));
    assert!(!json.contains("salt") && !json.contains("argon"), "the salt and cost stay in the admin: {json}");
}

#[test]
fn the_history_limit_defaults_to_the_whole_conversation_and_follows_the_setting() {
    let db = AdminDb::open_in_memory().unwrap();
    assert_eq!(server_link::build_snapshot(db.conn()).unwrap().history_turns, 0);
    db.conn().execute("INSERT INTO settings VALUES ('history_turns', '3')", []).unwrap();
    assert_eq!(server_link::build_snapshot(db.conn()).unwrap().history_turns, 3);
    db.conn().execute("UPDATE settings SET value = 'some' WHERE key = 'history_turns'", []).unwrap();
    assert_eq!(server_link::build_snapshot(db.conn()).unwrap().history_turns, 0);
}

#[test]
fn the_generation_timeout_defaults_to_300_when_never_set_and_follows_the_setting() {
    let db = AdminDb::open_in_memory().unwrap();
    // An install that predates the setting: no key, so the default, with no migration.
    assert_eq!(server_link::build_snapshot(db.conn()).unwrap().generation_timeout_secs, 300);
    db.conn().execute("INSERT INTO settings VALUES ('generation_timeout_secs', '120')", []).unwrap();
    assert_eq!(server_link::build_snapshot(db.conn()).unwrap().generation_timeout_secs, 120);
    // A stored value that is not a number falls back to the default rather than breaking the push.
    db.conn().execute("UPDATE settings SET value = 'soon' WHERE key = 'generation_timeout_secs'", []).unwrap();
    assert_eq!(server_link::build_snapshot(db.conn()).unwrap().generation_timeout_secs, 300);
}

#[tokio::test]
async fn a_push_applies_on_the_server() {
    let d = Dirs::new("push");
    let (token, hash_file) = d.install();
    let state = server_state();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    serve_on(l, state.clone(), hash_file).await;

    let link = ServerLink::new(&format!("http://{addr}"), token);
    assert!(!link.server_online(), "unknown until it has been in touch");
    let db = admin_db_with_data();
    let v = link.push(&server_link::build_snapshot(db.conn()).unwrap()).await.unwrap();
    assert_eq!(v, 1);
    assert!(link.server_online());
    assert_eq!(link.last_version(), 1);
    let stored = askbar_server::snapshot::current(&state.db.lock().unwrap()).unwrap().unwrap();
    assert_eq!((stored.version, stored.providers.len(), stored.audit_retention_days, stored.kill_switch), (1, 1, 14, true));
    assert_eq!(link.push(&server_link::build_snapshot(db.conn()).unwrap()).await.unwrap(), 2, "the server numbers each one");
}

#[tokio::test]
async fn an_unreachable_server_is_reported_as_offline_and_a_wrong_token_as_a_refusal() {
    let d = Dirs::new("offline");
    let (token, hash_file) = d.install();
    let dead = free_port().await;
    let link = ServerLink::new(&format!("http://{dead}"), token.clone());
    let snap = server_link::build_snapshot(AdminDb::open_in_memory().unwrap().conn()).unwrap();
    assert!(matches!(link.push(&snap).await, Err(server_link::PushError::Unreachable(_))));
    assert!(!link.server_online());
    assert!(link.last_error().is_some());
    assert!(!link.check().await);

    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    serve_on(l, server_state(), hash_file).await;
    let wrong = ServerLink::new(&format!("http://{addr}"), "mgt_not-the-token".into());
    match wrong.push(&snap).await {
        Err(server_link::PushError::Refused { status, .. }) => assert_eq!(status, 401),
        other => panic!("{other:?}"),
    }
    assert!(wrong.server_online(), "it answered, so it is there");
    assert!(!wrong.check().await, "but this token is no good");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pushing_retries_until_the_server_is_up_and_follows_later_changes() {
    let d = Dirs::new("retry");
    let (token, hash_file) = d.install();
    let addr = free_port().await; // nothing listens here yet
    let link = ServerLink::new(&format!("http://{addr}"), token);
    let db = Arc::new(Mutex::new(admin_db_with_data()));
    tokio::spawn(link.clone().run(db.clone(), server_link::no_resend()));

    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!link.server_online() && link.pending(), "still trying, and says so");

    // The server comes up on that address.
    let state = server_state();
    let l = tokio::net::TcpListener::bind(addr).await.unwrap();
    serve_on(l, state.clone(), hash_file).await;
    let applied =
        |want: usize| askbar_server::snapshot::current(&state.db.lock().unwrap()).ok().flatten().is_some_and(|s| s.providers.len() == want);
    for _ in 0..80 {
        if applied(1) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(applied(1), "the snapshot arrived once the server was there");
    assert!(link.server_online() && !link.pending());

    // A change in admin.db reaches it after mark_dirty.
    db.lock()
        .await
        .conn()
        .execute("INSERT INTO providers (id, name, type, model, created_at) VALUES ('p2','second','anthropic','m2',0)", [])
        .unwrap();
    link.mark_dirty();
    for _ in 0..50 {
        if applied(2) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(applied(2), "the change was pushed");
}

#[tokio::test]
async fn the_status_check_asks_for_a_push_when_the_server_is_unconfigured_or_out_of_date() {
    // ADR 0028 rule 4: read GET /status, and push when unconfigured, out of date, or a new process.
    let d = Dirs::new("status");
    let (token, hash) = d.install();
    let db = admin_db_with_data();
    let snap = || server_link::build_snapshot(db.conn()).unwrap();
    let state = server_state();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    serve_on(l, state.clone(), hash.clone()).await;
    let link = ServerLink::new(&format!("http://{addr}"), token.clone());

    assert_eq!(link.poll().await, Some(true), "never configured: push");
    link.push(&snap()).await.unwrap();
    assert_eq!(link.poll().await, Some(false), "in step: nothing to do");
    assert_eq!(link.poll().await, Some(false));

    // Another admin (or a restore) pushed a different version: this one's is no longer current.
    askbar_server::testing::update_snapshot(&state.db.lock().unwrap(), |_| {});
    assert_eq!(link.poll().await, Some(true), "out of date: push");
    link.push(&snap()).await.unwrap();
    assert_eq!(link.poll().await, Some(false));

    // The config is lost (a new volume): the version reads null.
    state.db.lock().unwrap().conn().execute("DELETE FROM config_snapshot", []).unwrap();
    assert_eq!(link.poll().await, Some(true), "unconfigured again: push");
    link.push(&snap()).await.unwrap();
    assert_eq!(link.poll().await, Some(false));

    // A new process (another boot ID) is covered by the unit tests of `needs_push` in server_link.rs.

    // Offline: no answer.
    let dead = ServerLink::new(&format!("http://{}", free_port().await), token);
    assert_eq!(dead.poll().await, None);
}
