use askbar_server::{rotate, secrets, AppState};
use reqwest::Method as M;
use serde_json::{json, Value};
use std::sync::Arc;

const PW: &str = "first password 123";

struct T {
    adm: String,
    state: Arc<AppState>,
    key_path: std::path::PathBuf,
    dir: std::path::PathBuf,
    cookie: String,
    http: reqwest::Client,
}
impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A set-up server with a key file, one provider with a key, and a Cloudflare-style encrypted setting.
async fn start() -> T {
    let dir = std::env::temp_dir().join(format!("askbar-rotate-{}", ulid::Ulid::new()));
    let key_path = dir.join("master.key");
    let (old, _) = secrets::generate_key(&key_path).unwrap();
    let db = askbar_core::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    db.conn().execute("INSERT INTO providers (id,name,type,model,api_key_enc,created_at) VALUES ('p1','mock','openai','m',?1,0)", [old.encrypt("sk-provider-1")]).unwrap();
    db.conn().execute("INSERT INTO providers (id,name,type,model,api_key_enc,created_at) VALUES ('p2','nokey','openai','m',NULL,0)", []).unwrap();
    db.conn().execute("INSERT INTO settings VALUES ('cloudflare_token', ?1)", [old.encrypt_b64("cf-token-xyz")]).unwrap();
    askbar_server::admin::init_password(&db, PW).unwrap();
    let state = AppState::build(db, Some(old), "0.1.0", "http://localhost:8080", key_path.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let adm = format!("http://{}", l.local_addr().unwrap());
    let app = askbar_server::admin_router(state.clone());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let http = reqwest::Client::new();
    let r = http.post(format!("{adm}/admin/login")).header("x-askbar-admin", "1").json(&json!({ "password": PW })).send().await.unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    T { adm, state, key_path, dir, cookie, http }
}

impl T {
    async fn call(&self, m: M, path: &str, body: Value) -> (u16, Value) {
        let r = self.http.request(m, format!("{}{path}", self.adm)).header("x-askbar-admin", "1").header("cookie", &self.cookie).json(&body).send().await.unwrap();
        let st = r.status().as_u16();
        (st, serde_json::from_str(&r.text().await.unwrap()).unwrap_or(Value::Null))
    }
    fn provider_blob(&self) -> Vec<u8> {
        self.state.db.lock().unwrap().conn().query_row("SELECT api_key_enc FROM providers WHERE id='p1'", [], |r| r.get(0)).unwrap()
    }
    fn setting(&self) -> String {
        self.state.db.lock().unwrap().conn().query_row("SELECT value FROM settings WHERE key='cloudflare_token'", [], |r| r.get(0)).unwrap()
    }
}

#[tokio::test]
async fn rotation_moves_everything_to_the_new_key() {
    let t = start().await;
    let old_text = std::fs::read_to_string(&t.key_path).unwrap();
    let old = secrets::Secrets::new(&old_text).unwrap();
    let (before_blob, before_setting) = (t.provider_blob(), t.setting());

    let (st, body) = t.call(M::POST, "/admin/api/security/rotate-key", json!({ "password": PW })).await;
    assert_eq!(st, 200);
    let new_text = body["key"].as_str().unwrap().to_string();
    assert_ne!(new_text, old_text);

    // Key file holds the new key; the old one is kept as .prev.
    assert_eq!(std::fs::read_to_string(&t.key_path).unwrap().trim(), new_text);
    assert_eq!(std::fs::read_to_string(secrets::prev_path(&t.key_path)).unwrap().trim(), old_text);
    assert!(!secrets::new_path(&t.key_path).exists());
    #[cfg(unix)]
    assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&t.key_path).unwrap().permissions()) & 0o777, 0o600);

    // Data re-encrypted: new key reads it, old key no longer does; plaintext unchanged.
    let new = secrets::Secrets::new(&new_text).unwrap();
    let (blob, setting) = (t.provider_blob(), t.setting());
    assert_ne!(blob, before_blob);
    assert_eq!(new.decrypt(&blob).unwrap(), "sk-provider-1");
    assert_eq!(new.decrypt_b64(&setting).unwrap(), "cf-token-xyz");
    assert!(old.decrypt(&blob).is_err() && old.decrypt_b64(&setting).is_err());
    assert!(old.decrypt_b64(&before_setting).is_ok());
    // The running server already uses it, so providers keep working with no restart.
    let live = askbar_server::store::load_provider(&t.state.db.lock().unwrap(), &t.state.secrets().unwrap(), "mock").unwrap().unwrap();
    assert_eq!(live.api_key, "sk-provider-1");
    // A provider without a key stays without one.
    let nokey: Option<Vec<u8>> = t.state.db.lock().unwrap().conn().query_row("SELECT api_key_enc FROM providers WHERE id='p2'", [], |r| r.get(0)).unwrap();
    assert!(nokey.is_none());

    // The previous key file can be listed and deleted.
    assert_eq!(t.call(M::GET, "/admin/api/security", json!({})).await.1, json!({ "previous_key": true }));
    assert_eq!(t.call(M::DELETE, "/admin/api/security/previous-key", json!({})).await.0, 204);
    assert_eq!(t.call(M::DELETE, "/admin/api/security/previous-key", json!({})).await.0, 404);
    assert_eq!(t.call(M::GET, "/admin/api/security", json!({})).await.1, json!({ "previous_key": false }));

    // And it can be rotated again.
    assert_eq!(t.call(M::POST, "/admin/api/security/rotate-key", json!({ "password": PW })).await.0, 200);
    assert_eq!(new.decrypt(&blob).unwrap(), "sk-provider-1");
    let live = askbar_server::store::load_provider(&t.state.db.lock().unwrap(), &t.state.secrets().unwrap(), "mock").unwrap().unwrap();
    assert_eq!(live.api_key, "sk-provider-1");
}

#[tokio::test]
async fn wrong_password_changes_nothing() {
    let t = start().await;
    let (key, blob) = (std::fs::read(&t.key_path).unwrap(), t.provider_blob());
    let (st, e) = t.call(M::POST, "/admin/api/security/rotate-key", json!({ "password": "nope" })).await;
    assert_eq!(st, 403);
    assert!(e["error"].is_string());
    assert_eq!(std::fs::read(&t.key_path).unwrap(), key);
    assert_eq!(t.provider_blob(), blob);
    assert!(!secrets::prev_path(&t.key_path).exists());
}

#[tokio::test]
async fn an_unreadable_stored_key_aborts_and_rolls_everything_back() {
    let t = start().await;
    // p0 sorts first and re-encrypts fine; p9's blob is garbage and fails afterwards.
    let good = t.state.secrets().unwrap().encrypt("sk-other");
    {
        let db = t.state.db.lock().unwrap();
        db.conn().execute("INSERT INTO providers (id,name,type,model,api_key_enc,created_at) VALUES ('p0','a-first','openai','m',?1,0)", [&good]).unwrap();
        db.conn().execute("INSERT INTO providers (id,name,type,model,api_key_enc,created_at) VALUES ('p9','z-broken','openai','m',?1,0)", [vec![1u8; 40]]).unwrap();
    }
    let (key, blob, setting) = (std::fs::read(&t.key_path).unwrap(), t.provider_blob(), t.setting());
    let (st, e) = t.call(M::POST, "/admin/api/security/rotate-key", json!({ "password": PW })).await;
    assert_eq!(st, 409);
    assert!(e["error"].as_str().unwrap().contains("z-broken") && e["error"].as_str().unwrap().contains("nothing was changed"));
    assert_eq!(std::fs::read(&t.key_path).unwrap(), key, "key file untouched");
    assert!(!secrets::new_path(&t.key_path).exists() && !secrets::prev_path(&t.key_path).exists());
    assert_eq!(t.provider_blob(), blob, "no row was half-converted");
    assert_eq!(t.setting(), setting);
    let still: Vec<u8> = t.state.db.lock().unwrap().conn().query_row("SELECT api_key_enc FROM providers WHERE id='p0'", [], |r| r.get(0)).unwrap();
    assert_eq!(still, good);
    assert_eq!(t.state.secrets().unwrap().decrypt(&blob).unwrap(), "sk-provider-1", "still on the old key");
}

#[tokio::test]
async fn rotation_needs_a_session() {
    let t = start().await;
    let r = t.http.post(format!("{}/admin/api/security/rotate-key", t.adm)).header("x-askbar-admin", "1").json(&json!({ "password": PW })).send().await.unwrap();
    assert_eq!(r.status(), 401);
}

// ---- crash recovery: the server was killed part-way through a rotation ----

fn recovery_fixture() -> (askbar_core::Db, std::path::PathBuf, std::path::PathBuf, secrets::Secrets, secrets::Secrets) {
    let dir = std::env::temp_dir().join(format!("askbar-recover-{}", ulid::Ulid::new()));
    let key_path = dir.join("master.key");
    let (old, _) = secrets::generate_key(&key_path).unwrap();
    let (new, _) = secrets::generate_key(&secrets::new_path(&key_path)).unwrap();
    let db = askbar_core::Db::open_in_memory().unwrap();
    (db, dir, key_path, old, new)
}

fn insert(db: &askbar_core::Db, blob: Vec<u8>) {
    db.conn().execute("INSERT INTO providers (id,name,type,model,api_key_enc,created_at) VALUES ('p','x','openai','m',?1,0)", [blob]).unwrap();
}

#[test]
fn recovery_discards_a_new_key_that_never_committed() {
    let (db, dir, key_path, old, _new) = recovery_fixture();
    insert(&db, old.encrypt("sk"));
    let before = std::fs::read(&key_path).unwrap();
    rotate::recover(&db, &key_path);
    assert!(!secrets::new_path(&key_path).exists());
    assert_eq!(std::fs::read(&key_path).unwrap(), before);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn recovery_finishes_a_rotation_that_committed_but_did_not_swap() {
    let (db, dir, key_path, old, new) = recovery_fixture();
    insert(&db, new.encrypt("sk"));
    let old_text = std::fs::read_to_string(&key_path).unwrap();
    let new_text = std::fs::read_to_string(secrets::new_path(&key_path)).unwrap();
    rotate::recover(&db, &key_path);
    assert_eq!(std::fs::read_to_string(&key_path).unwrap(), new_text);
    assert_eq!(std::fs::read_to_string(secrets::prev_path(&key_path)).unwrap(), old_text);
    assert!(!secrets::new_path(&key_path).exists());
    let _ = (old, std::fs::remove_dir_all(dir));
}

#[test]
fn recovery_handles_a_crash_between_the_two_renames() {
    let (db, dir, key_path, old, new) = recovery_fixture();
    insert(&db, new.encrypt("sk"));
    let new_text = std::fs::read_to_string(secrets::new_path(&key_path)).unwrap();
    std::fs::rename(&key_path, secrets::prev_path(&key_path)).unwrap(); // first rename done, second not
    rotate::recover(&db, &key_path);
    assert_eq!(std::fs::read_to_string(&key_path).unwrap(), new_text);
    assert!(!secrets::new_path(&key_path).exists());
    let _ = (old, std::fs::remove_dir_all(dir));
}

#[test]
fn recovery_ignores_a_garbage_leftover() {
    let (db, dir, key_path, _old, _new) = recovery_fixture();
    std::fs::write(secrets::new_path(&key_path), "short").unwrap();
    let before = std::fs::read(&key_path).unwrap();
    rotate::recover(&db, &key_path);
    assert!(!secrets::new_path(&key_path).exists());
    assert_eq!(std::fs::read(&key_path).unwrap(), before);
    let _ = std::fs::remove_dir_all(dir);
}
