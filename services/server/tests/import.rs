//! The server's restore (ADR 0013): what an export made goes into an empty install, all or nothing, with no restart; a used install, a
//! backup from a newer release and a backup that is not whole are each refused and change nothing.

use askbar_server::{server_api, AppState, Db, Spaces};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;

const TOKEN: &str = "mgt_this-is-the-raw-server-token";
const PASS: &str = "a backup passphrase of its own";

struct Srv {
    url: String,
    dir: PathBuf,
    state: Arc<AppState>,
    http: reqwest::Client,
}

impl Drop for Srv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A server with its files on disk, as a stack's is: `control.db` and `spaces/` under one data directory.
async fn start() -> Srv {
    let dir = std::env::temp_dir().join(format!("server-import-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let hash_file = dir.join("token.sha256");
    let hash: String = Sha256::digest(TOKEN.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(&hash_file, hash).unwrap();
    let db = Db::open(&dir.join("control.db")).unwrap();
    db.seed_server().unwrap();
    let state = AppState::new(db, askbar_server::MIN_CLIENT).with_spaces(Spaces::on_disk(dir.join("spaces")));
    state.spaces.open_all(&state.db.lock().unwrap()).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let app = server_api::router(state.clone(), hash_file);
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    Srv { url, dir, state, http: reqwest::Client::new() }
}

impl Srv {
    fn owner(&self) -> String {
        self.state.db.lock().unwrap().conn().query_row("SELECT id FROM users ORDER BY created_at, id LIMIT 1", [], |r| r.get(0)).unwrap()
    }

    /// A conversation in the owner's space, so the install is used.
    fn write_history(&self, title: &str, text: &str) {
        let user = self.owner();
        let space = askbar_server::testing::personal_space(&self.state, &user);
        let c = space.lock().create_conversation(&user, title).unwrap();
        space.lock().add_message(&user, &c, "user", text).unwrap();
    }

    async fn post(&self, path: &str, body: Vec<u8>) -> reqwest::Response {
        self.http.post(format!("{}/api/server/v1{path}", self.url)).bearer_auth(TOKEN).body(body).send().await.unwrap()
    }

    /// The server's export, opened with the key: the plain tar `POST /imports` takes.
    async fn plain_export(&self, key: &askbar_service::backup::Key) -> Vec<u8> {
        askbar_server::testing::update_snapshot(&self.state.db.lock().unwrap(), |s| s.backup_public_key = Some(key.recipient.clone()));
        let sealed = self.post("/exports", vec![]).await.bytes().await.unwrap();
        let identity = askbar_service::backup::unlock(&key.protected, PASS).unwrap();
        askbar_service::backup::decrypt(&identity, &sealed).unwrap()
    }

    fn leftovers(&self) -> usize {
        std::fs::read_dir(&self.dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".import-"))
            .count()
    }

    fn space_files(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(self.dir.join("spaces"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".db"))
            .collect();
        v.sort();
        v
    }
}

async fn problem(r: reqwest::Response) -> (u16, String) {
    let status = r.status().as_u16();
    let body: serde_json::Value = r.json().await.unwrap_or_default();
    (status, body["error"].as_str().unwrap_or_default().to_string())
}

#[tokio::test]
async fn an_export_restores_into_an_empty_server_without_a_restart() {
    let source = start().await;
    let key = askbar_service::backup::generate_with(PASS, 10).unwrap();
    source.write_history("kept conversation", "question one");
    let owner = source.owner();
    source
        .state
        .db
        .lock()
        .unwrap()
        .conn()
        .execute("INSERT INTO clients (id, user_id, name, hash, created_at) VALUES ('C1', ?1, 'laptop', 'token-hash-marker', 0)", [owner])
        .unwrap();
    let tar = source.plain_export(&key).await;
    let target = start().await;
    let (old_owner, old_files) = (target.owner(), target.space_files());
    assert_ne!(old_owner, source.owner(), "two fresh installs seed different owners");
    let r = target.post("/imports", tar).await;
    assert_eq!(r.status(), 200, "{:?}", r.text().await);
    // The restored server is the backed-up one: its owner, its client, its history, with the seeded space gone.
    assert_eq!(target.owner(), source.owner());
    let one = |sql: &str| target.state.db.lock().unwrap().conn().query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(one("SELECT COUNT(*) FROM clients WHERE hash = 'token-hash-marker'"), 1);
    let space = askbar_server::testing::personal_space(&target.state, &target.owner());
    let titles: Vec<String> = space.lock().list_conversations().unwrap().into_iter().map(|c| c.title).collect();
    assert_eq!(titles, ["kept conversation"]);
    assert_eq!(target.space_files(), source.space_files());
    assert!(old_files.iter().all(|f| !target.space_files().contains(f)), "the seeded owner's space file is gone");
    assert_eq!(target.leftovers(), 0, "no work files");
    // It is a used install now: a second import is refused.
    assert_eq!(problem(target.post("/imports", source.plain_export(&key).await).await).await, (409, "not_empty".into()));
}

#[tokio::test]
async fn a_used_install_refuses_before_reading_anything_and_changes_nothing() {
    let source = start().await;
    let key = askbar_service::backup::generate_with(PASS, 10).unwrap();
    let tar = source.plain_export(&key).await;
    let target = start().await;
    target.write_history("mine", "do not lose me");
    let before = target.owner();
    assert_eq!(problem(target.post("/imports", tar.clone()).await).await, (409, "not_empty".into()));
    assert_eq!(target.owner(), before);
    let space = askbar_server::testing::personal_space(&target.state, &before);
    assert_eq!(space.lock().list_conversations().unwrap().len(), 1);
    // A second person, or a client, makes it used as well.
    let target = start().await;
    target.state.db.lock().unwrap().conn().execute("INSERT INTO users (id, name, created_at) VALUES ('U2', 'second', 0)", []).unwrap();
    assert_eq!(problem(target.post("/imports", tar).await).await.1, "not_empty");
    assert_eq!(target.leftovers(), 0);
}

#[tokio::test]
async fn a_backup_from_a_newer_release_or_one_that_is_not_whole_is_refused_and_changes_nothing() {
    let source = start().await;
    let key = askbar_service::backup::generate_with(PASS, 10).unwrap();
    let tar = source.plain_export(&key).await;
    let target = start().await;
    let (owner, files) = (target.owner(), target.space_files());
    let unchanged = |t: &Srv| assert_eq!((t.owner(), t.space_files(), t.leftovers()), (owner.clone(), files.clone(), 0));

    // Cut anywhere: a download that stopped early is not a backup.
    for cut in [0, 300, tar.len() / 2, tar.len() - 600] {
        let (status, code) = problem(target.post("/imports", tar[..cut].to_vec()).await).await;
        assert_eq!((status, code.as_str()), (400, "bad_request"), "cut at {cut}");
        unchanged(&target);
    }
    // A control database from a newer release (a higher schema version).
    let mut entries = askbar_service::tar::read(&tar).unwrap();
    let scratch = target.dir.join("edit.db");
    std::fs::write(&scratch, &entries[0].1).unwrap();
    rusqlite::Connection::open(&scratch).unwrap().execute_batch("PRAGMA user_version = 999").unwrap();
    entries[0].1 = std::fs::read(&scratch).unwrap();
    let mut newer = Vec::new();
    for (n, d) in &entries {
        askbar_service::tar::append(&mut newer, n, d, 0);
    }
    askbar_service::tar::finish(&mut newer);
    assert_eq!(problem(target.post("/imports", newer).await).await, (409, "backup_newer_release".into()));
    unchanged(&target);
    // A file that is not ours, a missing space, and an extra one.
    let build = |f: &dyn Fn(&mut Vec<(String, Vec<u8>)>)| {
        let mut e = askbar_service::tar::read(&tar).unwrap();
        f(&mut e);
        let mut t = Vec::new();
        for (n, d) in &e {
            askbar_service::tar::append(&mut t, n, d, 0);
        }
        askbar_service::tar::finish(&mut t);
        t
    };
    for (what, bad) in [
        ("a stranger", build(&|e| e.push(("admin.db".into(), b"x".to_vec())))),
        ("no spaces", build(&|e| e.truncate(1))),
        ("no control database", build(&|e| e.remove(0).1.clear())),
        ("not a database", build(&|e| e[0].1 = b"this is not sqlite".to_vec())),
    ] {
        let (status, code) = problem(target.post("/imports", bad).await).await;
        assert_eq!((status, code.as_str()), (400, "bad_request"), "{what}");
        unchanged(&target);
    }
    // It still takes the good one afterwards: nothing was half done.
    assert_eq!(target.post("/imports", tar).await.status(), 200);
}

#[tokio::test]
async fn only_the_admins_token_reaches_it() {
    let t = start().await;
    let r = t.http.post(format!("{}/api/server/v1/imports", t.url)).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let r = t.http.post(format!("{}/api/server/v1/imports", t.url)).bearer_auth("mgt_wrong").send().await.unwrap();
    assert_eq!(r.status(), 401);
}
