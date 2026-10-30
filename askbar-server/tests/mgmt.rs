use askbar_server::{mgmt, secrets::Secrets, AppState};
use sha2::{Digest, Sha256};

const TOKEN: &str = "mgt_this-is-the-raw-management-token";

fn hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

struct T {
    url: String,
    hash_file: std::path::PathBuf,
    dir: std::path::PathBuf,
    http: reqwest::Client,
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn start(write_hash: bool) -> T {
    let dir = std::env::temp_dir().join(format!("askbar-mgmt-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let hash_file = dir.join("token.sha256");
    if write_hash {
        std::fs::write(&hash_file, format!("{}\n", hex(TOKEN))).unwrap();
    }
    let db = askbar_server::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    let state = AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), askbar_server::MIN_CLIENT, "http://localhost:8080");
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let app = mgmt::router(state, hash_file.clone());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    T { url, hash_file, dir, http: reqwest::Client::new() }
}

impl T {
    async fn health(&self, auth: Option<&str>) -> u16 {
        let mut r = self.http.get(format!("{}/mgmt/v1/health", self.url));
        if let Some(a) = auth {
            r = r.header("authorization", a);
        }
        r.send().await.unwrap().status().as_u16()
    }
}

#[tokio::test]
async fn the_right_token_gets_through() {
    let t = start(true).await;
    assert_eq!(t.health(Some(&format!("Bearer {TOKEN}"))).await, 200);
    assert_eq!(t.health(Some(&format!("bearer {TOKEN}"))).await, 200, "the scheme is case-insensitive");
}

#[tokio::test]
async fn no_token_a_wrong_token_or_the_stored_hash_is_401() {
    let t = start(true).await;
    assert_eq!(t.health(None).await, 401);
    assert_eq!(t.health(Some("Bearer ")).await, 401);
    assert_eq!(t.health(Some(&format!("Basic {TOKEN}"))).await, 401);
    assert_eq!(t.health(Some("Bearer mgt_wrong")).await, 401);
    assert_eq!(t.health(Some(&format!("Bearer {}", hex(TOKEN)))).await, 401, "the hash the server holds is not a credential");
    // Unknown routes do not get past the check either.
    let r = t.http.get(format!("{}/mgmt/v1/nope", t.url)).send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn it_fails_closed_when_the_hash_file_is_missing_or_unusable() {
    let t = start(false).await;
    assert_eq!(t.health(Some(&format!("Bearer {TOKEN}"))).await, 401, "no hash file");

    // Appears later: read on each request, so it works without a restart.
    std::fs::write(&t.hash_file, hex(TOKEN)).unwrap();
    assert_eq!(t.health(Some(&format!("Bearer {TOKEN}"))).await, 200);

    // Replaced (rotated): the old token stops working at once.
    std::fs::write(&t.hash_file, hex("mgt_a-newer-token")).unwrap();
    assert_eq!(t.health(Some(&format!("Bearer {TOKEN}"))).await, 401);
    assert_eq!(t.health(Some("Bearer mgt_a-newer-token")).await, 200);

    // Garbage or empty content never matches, whatever is sent.
    for junk in ["", "not a hash", "abc"] {
        std::fs::write(&t.hash_file, junk).unwrap();
        assert_eq!(t.health(Some(&format!("Bearer {junk}"))).await, 401, "{junk:?}");
        assert_eq!(t.health(Some(&format!("Bearer {TOKEN}"))).await, 401, "{junk:?}");
    }

    // And gone again.
    std::fs::remove_file(&t.hash_file).unwrap();
    assert_eq!(t.health(Some("Bearer mgt_a-newer-token")).await, 401);
}

#[test]
fn the_listen_address_is_never_a_wildcard() {
    assert!(mgmt::parse_addr("172.30.0.2:8082").is_ok());
    assert!(mgmt::parse_addr("127.0.0.1:8082").is_ok());
    for bad in ["0.0.0.0:8082", "[::]:8082", "8082", "mgmt:8082", ""] {
        assert!(mgmt::parse_addr(bad).is_err(), "{bad:?}");
    }
    assert!(mgmt::parse_addr("0.0.0.0:8082").unwrap_err().contains("every network"));
}
