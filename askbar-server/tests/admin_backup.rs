use askbar_server::{admin, secrets::Secrets, AppState};
use serde_json::json;

// Both tests look at the shared temp directory, so they take turns.
static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const PASSWORD: &str = "correct horse battery";
const KEY: &str = "sk-backup-secret-456";

async fn start() -> (String, String, std::sync::Arc<AppState>, Vec<u8>) {
    let db = askbar_server::Db::open_in_memory().unwrap();
    let user = db.seed_server().unwrap();
    admin::init_password(&db, PASSWORD).unwrap();
    let secrets = Secrets::new("0123456789abcdef0123456789abcdef").unwrap();
    let enc = secrets.encrypt(KEY);
    db.conn()
        .execute("INSERT INTO providers (id, name, type, base_url, model, api_key_enc, created_at) VALUES ('p','mock','openai','http://x/v1','m',?1,0)", [&enc])
        .unwrap();
    db.conn().execute("INSERT INTO settings VALUES ('system_prompt', 'Be brief.')", []).unwrap();
    let c = db.create_conversation(&user, "kept conversation").unwrap();
    db.add_message(&user, &c, "user", "question one").unwrap();
    db.add_message(&user, &c, "assistant", "answer one").unwrap();
    askbar_server::auth::create_pair_code(&db, &user, "pending", 600).unwrap();
    let state = AppState::new(db, secrets, askbar_server::MIN_CLIENT, "http://localhost:8080");
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    let app = askbar_server::admin_router(state.clone());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let r = reqwest::Client::new().post(format!("{addr}/admin/login")).header("x-askbar-admin", "1").json(&json!({ "password": PASSWORD })).send().await.unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    (addr, cookie, state, enc)
}

#[tokio::test]
async fn backup_has_the_data_and_none_of_the_secrets() {
    let _turn = TURN.lock().await;
    let (addr, cookie, state, enc) = start().await;
    let r = reqwest::Client::new().get(format!("{addr}/admin/api/backup")).header("cookie", &cookie).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "application/vnd.sqlite3");
    let disp = r.headers()["content-disposition"].to_str().unwrap().to_string();
    assert!(disp.starts_with("attachment; filename=\"askbar-backup-") && disp.ends_with(".db\""), "{disp}");
    let bytes = r.bytes().await.unwrap().to_vec();
    assert!(bytes.starts_with(b"SQLite format 3\0"));

    // Neither the plaintext key, the encrypted key, nor the admin hash is anywhere in the file.
    let contains = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    assert!(!contains(KEY.as_bytes()) && !contains(&enc) && !contains(b"$argon2") && !contains(b"admin_password_hash"));

    let dir = std::env::temp_dir().join(format!("askbar-backup-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("restored.db");
    std::fs::write(&file, &bytes).unwrap();
    let c = rusqlite::Connection::open(&file).unwrap();
    let one = |sql: &str| c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(c.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0)).unwrap(), "ok");
    assert_eq!(one("SELECT COUNT(*) FROM conversations WHERE title = 'kept conversation'"), 1);
    assert_eq!(one("SELECT COUNT(*) FROM messages"), 2);
    assert_eq!(c.query_row("SELECT content FROM messages ORDER BY id LIMIT 1", [], |r| r.get::<_, String>(0)).unwrap(), "question one");
    assert_eq!(one("SELECT COUNT(*) FROM providers"), 1, "the provider entry itself is kept");
    assert_eq!(one("SELECT COUNT(*) FROM providers WHERE api_key_enc IS NOT NULL"), 0);
    assert_eq!(one("SELECT COUNT(*) FROM settings WHERE key = 'admin_password_hash'"), 0);
    assert_eq!(one("SELECT COUNT(*) FROM settings WHERE key = 'system_prompt'"), 1);
    assert_eq!(one("SELECT COUNT(*) FROM pair_codes"), 0);
    drop(c);

    // The live database is untouched.
    let db = state.db.lock().unwrap();
    let live: Vec<u8> = db.conn().query_row("SELECT api_key_enc FROM providers", [], |r| r.get(0)).unwrap();
    assert_eq!(live, enc);
    assert_eq!(db.conn().query_row("SELECT COUNT(*) FROM pair_codes", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    assert!(db.conn().query_row("SELECT 1 FROM settings WHERE key = 'admin_password_hash'", [], |_| Ok(())).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn backup_needs_a_session_and_leaves_no_temp_files() {
    let _turn = TURN.lock().await;
    let (addr, cookie, _state, _) = start().await;
    let before = count_snapshots();
    let http = reqwest::Client::new();
    assert_eq!(http.get(format!("{addr}/admin/api/backup")).send().await.unwrap().status(), 401);
    let r = http.get(format!("{addr}/admin/api/backup")).header("cookie", &cookie).send().await.unwrap();
    assert_eq!(r.status(), 200);
    r.bytes().await.unwrap();
    for _ in 0..50 {
        if count_snapshots() <= before {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the snapshot file was left behind");
}

fn count_snapshots() -> usize {
    std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("askbar-backup-") && e.file_name().to_string_lossy().ends_with(".db"))
        .count()
}
