//! The admin side of the management link, against a real in-process server.

use askbar_admin::mgmt::{self, ServerLink};
use askbar_admin::AdminDb;
use askbar_server::{secrets::Secrets, AppState};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::time::Duration;

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("askbar-link-{tag}-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn mode(p: &std::path::Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

struct Dirs {
    root: PathBuf,
    secrets: PathBuf,
    mgmt: PathBuf,
}

impl Dirs {
    fn new(tag: &str) -> Dirs {
        let root = temp(tag);
        Dirs { secrets: root.join("admin-secrets"), mgmt: root.join("askbar-mgmt"), root }
    }
}

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// ------------------------------------------------------------------ the token

#[test]
fn the_token_is_private_the_server_gets_only_its_hash_and_nothing_is_overwritten() {
    let d = Dirs::new("token");
    let token = mgmt::ensure_token(&d.secrets, &d.mgmt).unwrap();

    let token_file = d.secrets.join(mgmt::TOKEN_FILE);
    assert_eq!(mode(&token_file), 0o600, "the raw token is private to the admin");
    assert_eq!(mode(&d.secrets), 0o700);
    assert_eq!(std::fs::read_to_string(&token_file).unwrap(), token);
    let raw = URL_SAFE_NO_PAD.decode(token.strip_prefix("mgt_").expect("prefixed")).unwrap();
    assert_eq!(raw.len(), 32, "32 random bytes");

    let hash_file = d.mgmt.join(mgmt::HASH_FILE);
    let hash = std::fs::read_to_string(&hash_file).unwrap();
    assert_eq!(hash.trim(), mgmt::sha256_hex(&token));
    assert_eq!(hash.trim().len(), 64);
    assert!(!hash.contains(&token) && !hash.contains(token.trim_start_matches("mgt_")), "the hash file holds no raw token");
    assert!(std::fs::read_dir(&d.mgmt).unwrap().all(|e| e.unwrap().file_name() == mgmt::HASH_FILE), "only the hash is in the server's directory");
    assert_ne!(mode(&hash_file) & 0o044, 0, "the server's user can read the hash");

    // A restart keeps the token; a damaged hash file is put right; a second token is never made.
    assert_eq!(mgmt::ensure_token(&d.secrets, &d.mgmt).unwrap(), token);
    std::fs::write(&hash_file, "tampered").unwrap();
    assert_eq!(mgmt::ensure_token(&d.secrets, &d.mgmt).unwrap(), token);
    assert_eq!(std::fs::read_to_string(&hash_file).unwrap().trim(), mgmt::sha256_hex(&token));
    std::fs::remove_file(&hash_file).unwrap();
    mgmt::ensure_token(&d.secrets, &d.mgmt).unwrap();
    assert!(hash_file.exists(), "a lost hash file is written again");

    // An empty token file is an error, not a reason to make a new token silently.
    std::fs::write(&token_file, "").unwrap();
    assert!(mgmt::ensure_token(&d.secrets, &d.mgmt).is_err());
}

// ------------------------------------------------------------------ the server

fn server_state() -> Arc<AppState> {
    let db = askbar_server::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), askbar_server::MIN_CLIENT, "http://localhost:8080")
}

async fn serve_on(listener: tokio::net::TcpListener, state: Arc<AppState>, hash_file: PathBuf) {
    let app = askbar_server::mgmt::router(state, hash_file);
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
             INSERT INTO revoked_devices VALUES ('dev-9', 0);
             UPDATE budgets SET daily_tokens = 5000, kill_switch = 1;",
        )
        .unwrap();
    db
}

#[test]
fn the_snapshot_is_built_from_admin_db() {
    let db = admin_db_with_data();
    db.conn().execute("INSERT INTO pair_codes (hash, user_id, device_name, expires_at, created_at) VALUES ('h1','u1','laptop', 4000000000, 0)", []).unwrap();
    db.conn().execute("INSERT INTO pair_codes (hash, user_id, device_name, expires_at, created_at) VALUES ('old','u1','x', 1, 0)", []).unwrap();
    let s = mgmt::build_snapshot(db.conn()).unwrap();
    assert_eq!(s.providers.len(), 1);
    assert_eq!((s.providers[0].id.as_str(), s.providers[0].model.as_str(), s.providers[0].max_tokens), ("p1", "m1", Some(100)));
    assert_eq!((s.default_provider_id.as_deref(), s.system_prompt.as_deref(), s.audit_retention_days), (Some("p1"), Some("Be brief."), 14));
    assert_eq!((s.budget.daily_tokens, s.budget.monthly_tokens, s.kill_switch), (Some(5000), None, true));
    assert_eq!(s.revoked_device_ids, ["dev-9"]);
    assert_eq!(s.pair_codes.len(), 1, "an expired code is not sent");
    assert_eq!(s.pair_codes[0].hash, "h1");
    // A fresh database gives a valid, empty snapshot.
    let empty = mgmt::build_snapshot(AdminDb::open_in_memory().unwrap().conn()).unwrap();
    assert_eq!((empty.providers.len(), empty.audit_retention_days, empty.kill_switch), (0, 30, false));
}

#[tokio::test]
async fn a_push_applies_on_the_server() {
    let d = Dirs::new("push");
    let token = mgmt::ensure_token(&d.secrets, &d.mgmt).unwrap();
    let state = server_state();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    serve_on(l, state.clone(), d.mgmt.join(mgmt::HASH_FILE)).await;

    let link = ServerLink::new(&format!("http://{addr}"), token);
    assert!(!link.server_online(), "unknown until it has been in touch");
    let db = admin_db_with_data();
    let v = link.push(&mgmt::build_snapshot(db.conn()).unwrap()).await.unwrap();
    assert_eq!(v, 1);
    assert!(link.server_online());
    assert_eq!(link.last_version(), 1);
    let stored = askbar_server::snapshot::current(&state.db.lock().unwrap()).unwrap().unwrap();
    assert_eq!((stored.version, stored.providers.len(), stored.audit_retention_days, stored.kill_switch), (1, 1, 14, true));
    assert_eq!(link.push(&mgmt::build_snapshot(db.conn()).unwrap()).await.unwrap(), 2, "the server numbers each one");
}

#[tokio::test]
async fn an_unreachable_server_is_reported_as_offline_and_a_wrong_token_as_a_refusal() {
    let d = Dirs::new("offline");
    let token = mgmt::ensure_token(&d.secrets, &d.mgmt).unwrap();
    let dead = free_port().await;
    let link = ServerLink::new(&format!("http://{dead}"), token.clone());
    let snap = mgmt::build_snapshot(AdminDb::open_in_memory().unwrap().conn()).unwrap();
    assert!(matches!(link.push(&snap).await, Err(mgmt::PushError::Unreachable(_))));
    assert!(!link.server_online());
    assert!(link.last_error().is_some());
    assert!(!link.check().await);

    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    serve_on(l, server_state(), d.mgmt.join(mgmt::HASH_FILE)).await;
    let wrong = ServerLink::new(&format!("http://{addr}"), "mgt_not-the-token".into());
    match wrong.push(&snap).await {
        Err(mgmt::PushError::Refused { status, .. }) => assert_eq!(status, 401),
        other => panic!("{other:?}"),
    }
    assert!(wrong.server_online(), "it answered, so it is there");
    assert!(!wrong.check().await, "but this token is no good");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pushing_retries_until_the_server_is_up_and_follows_later_changes() {
    let d = Dirs::new("retry");
    let token = mgmt::ensure_token(&d.secrets, &d.mgmt).unwrap();
    let addr = free_port().await; // nothing listens here yet
    let link = ServerLink::new(&format!("http://{addr}"), token);
    let db = Arc::new(Mutex::new(admin_db_with_data()));
    tokio::spawn(link.clone().run(db.clone()));

    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!link.server_online() && link.pending(), "still trying, and says so");

    // The server comes up on that address.
    let state = server_state();
    let l = tokio::net::TcpListener::bind(addr).await.unwrap();
    serve_on(l, state.clone(), d.mgmt.join(mgmt::HASH_FILE)).await;
    let applied = |want: usize| {
        askbar_server::snapshot::current(&state.db.lock().unwrap()).ok().flatten().is_some_and(|s| s.providers.len() == want)
    };
    for _ in 0..80 {
        if applied(1) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(applied(1), "the snapshot arrived once the server was there");
    assert!(link.server_online() && !link.pending());

    // A change in admin.db reaches it after mark_dirty.
    db.lock().await.conn().execute("INSERT INTO providers (id, name, type, model, created_at) VALUES ('p2','second','anthropic','m2',0)", []).unwrap();
    link.mark_dirty();
    for _ in 0..50 {
        if applied(2) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(applied(2), "the change was pushed");
}
