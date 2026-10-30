use askbar_admin::{auth, db::DbError, AdminDb, AdminState};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("askbar-admin-{tag}-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ---------------------------------------------------------------- database

#[test]
fn migrations_apply_and_the_defaults_are_safe() {
    let db = AdminDb::open_in_memory().unwrap();
    let tables: Vec<String> = {
        let mut stmt = db.conn().prepare("SELECT name FROM sqlite_master WHERE type = 'table'").unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    for t in ["admin_credentials", "providers", "settings", "display_defaults", "pair_codes", "budgets", "network_settings", "revoked_devices"] {
        assert!(tables.iter().any(|x| x == t), "missing {t}");
    }
    let v: i64 = db.conn().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(v, 3);
    // No password until setup; no limits; the IP allowlist and HTTPS start switched off.
    let creds: i64 = db.conn().query_row("SELECT COUNT(*) FROM admin_credentials", [], |r| r.get(0)).unwrap();
    assert_eq!(creds, 0);
    let (allow, tls, kill): (i64, i64, i64) = db
        .conn()
        .query_row("SELECT n.allowlist_enabled, n.tls_enabled, b.kill_switch FROM network_settings n, budgets b", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    assert_eq!((allow, tls, kill), (0, 0, 0));
}

#[test]
fn it_persists_and_refuses_databases_it_did_not_make() {
    let dir = temp("db");
    let path = dir.join("nested").join("admin.db");
    {
        let db = AdminDb::open(&path).unwrap();
        db.conn().execute("INSERT INTO settings VALUES ('k', 'v')", []).unwrap();
    }
    let db = AdminDb::open(&path).unwrap();
    let v: String = db.conn().query_row("SELECT value FROM settings WHERE key = 'k'", [], |r| r.get(0)).unwrap();
    assert_eq!(v, "v", "applying migrations again keeps the data");
    drop(db);

    // The server's database (a different application id) and a newer version are both refused.
    let foreign = dir.join("foreign.db");
    rusqlite::Connection::open(&foreign).unwrap().execute_batch("CREATE TABLE t (x); PRAGMA application_id = 1095062338; PRAGMA user_version = 1;").unwrap();
    assert!(matches!(AdminDb::open(&foreign), Err(DbError::UnexpectedVersion { .. })));
    let newer = dir.join("newer.db");
    drop(AdminDb::open(&newer).unwrap());
    rusqlite::Connection::open(&newer).unwrap().execute_batch("PRAGMA user_version = 99").unwrap();
    assert!(matches!(AdminDb::open(&newer), Err(DbError::UnexpectedVersion { .. })));
    let _ = std::fs::remove_dir_all(&dir);
}

// ----------------------------------------------------------- sessions, limiting

#[test]
fn repeated_failed_logins_are_blocked_and_a_success_clears_them() {
    let a = auth::AdminAuth::default();
    for _ in 0..auth::MAX_FAILURES - 1 {
        a.record_failure();
    }
    assert!(a.lockout_remaining().is_none(), "one short of the limit");
    a.record_failure();
    let left = a.lockout_remaining().expect("locked after the fifth failure");
    assert!(left > 0 && left <= auth::LOCKOUT_SECS);

    let b = auth::AdminAuth::default();
    for _ in 0..auth::MAX_FAILURES - 1 {
        b.record_failure();
    }
    b.clear_failures();
    b.record_failure();
    assert!(b.lockout_remaining().is_none(), "a success in between starts the count again");

    // Failures spread over more than the window never add up.
    let c = auth::AdminAuth::default();
    let t = auth::now();
    for i in 0..auth::MAX_FAILURES as i64 * 2 {
        c.record_failure_at(t - i * (auth::FAILURE_WINDOW_SECS / 2 + 1));
    }
    assert!(c.lockout_remaining().is_none());
}

#[test]
fn sessions_are_in_memory_tokens_that_can_end() {
    let a = auth::AdminAuth::default();
    let (one, two) = (a.new_session(), a.new_session());
    assert_ne!(one, two);
    assert!(a.valid_session(&one) && a.valid_session(&two));
    assert!(!a.valid_session("made-up"));
    a.keep_only(Some(&one));
    assert!(a.valid_session(&one) && !a.valid_session(&two));
    a.end_session(&one);
    assert!(!a.valid_session(&one));
}

// ------------------------------------------------------------------ the socket

fn get_health(path: &std::path::Path) -> String {
    let mut s = UnixStream::connect(path).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    s.write_all(b"GET /health HTTP/1.0\r\nHost: admin\r\n\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

async fn wait_for(path: &std::path::Path) {
    for _ in 0..100 {
        if path.exists() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("no socket at {}", path.display());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_socket_answers_health() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp("sock");
    let sock = dir.join("run").join("admin.sock");
    std::fs::create_dir_all(sock.parent().unwrap()).unwrap();
    std::fs::write(&sock, "stale file from a crashed run").unwrap();
    let state = AdminState::new(AdminDb::open_in_memory().unwrap());
    let app = askbar_admin::router(state);
    let p = sock.clone();
    tokio::spawn(async move { askbar_admin::serve_unix(&p, app).await.unwrap() });
    wait_for(&sock).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let reply = get_health(&sock);
    assert!(reply.starts_with("HTTP/1.") && reply.contains(" 200 "), "{reply}");
    assert!(reply.ends_with("ok"));
    assert_eq!(std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777, 0o660, "owner and group only");
    assert!(askbar_admin::healthcheck(&sock));
    assert!(!askbar_admin::healthcheck(&dir.join("nothing-here.sock")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The real binary: the socket answers, `healthcheck` agrees, and the process has no TCP listener.
#[test]
fn the_binary_serves_a_socket_and_opens_no_tcp_port() {
    let dir = temp("bin");
    let sock = dir.join("admin.sock");
    let bin = env!("CARGO_BIN_EXE_askbar-admin");
    let envs = [("ASKBAR_ADMIN_SOCKET", sock.to_str().unwrap()), ("ASKBAR_ADMIN_DATA_DIR", dir.to_str().unwrap())];
    let mut child = std::process::Command::new(bin).envs(envs).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    let reply = get_health(&sock);
    assert!(reply.starts_with("HTTP/1.") && reply.contains(" 200 "), "{reply}");
    let hc = std::process::Command::new(bin).arg("healthcheck").envs(envs).status().unwrap();
    assert!(hc.success(), "healthcheck exits 0 while it is up");
    assert!(dir.join("admin.db").exists(), "admin.db is created in the data directory");

    // Every socket the process holds: none may be a TCP listener.
    #[cfg(target_os = "linux")]
    {
        let inodes: Vec<String> = std::fs::read_dir(format!("/proc/{}/fd", child.id()))
            .unwrap()
            .filter_map(|e| std::fs::read_link(e.unwrap().path()).ok())
            .filter_map(|l| l.to_str().and_then(|s| s.strip_prefix("socket:[")).map(|s| s.trim_end_matches(']').to_string()))
            .collect();
        assert!(!inodes.is_empty(), "it does hold its unix socket");
        for table in ["/proc/net/tcp", "/proc/net/tcp6", "/proc/net/udp", "/proc/net/udp6"] {
            let text = std::fs::read_to_string(table).unwrap_or_default();
            for line in text.lines().skip(1) {
                let cols: Vec<&str> = line.split_whitespace().collect();
                if let Some(inode) = cols.get(9) {
                    assert!(!inodes.iter().any(|i| i == inode), "{table} lists one of its sockets: {line}");
                }
            }
        }
    }

    child.kill().unwrap();
    child.wait().unwrap();
    let down = std::process::Command::new(bin).arg("healthcheck").envs(envs).status().unwrap();
    assert!(!down.success(), "healthcheck fails once it is gone");
    let _ = std::fs::remove_dir_all(&dir);
}
