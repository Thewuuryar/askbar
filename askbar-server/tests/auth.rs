use askbar_server::{auth, secrets::Secrets, AppState};
use axum::Router;
use std::net::SocketAddr;
use std::sync::Arc;

struct Server {
    addr: SocketAddr,
    state: Arc<AppState>,
    user: String,
    http: reqwest::Client,
}

async fn start() -> Server {
    let db = askbar_server::Db::open_in_memory().unwrap();
    let user = db.seed_server().unwrap();
    let state = AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), askbar_server::MIN_CLIENT, "http://localhost:8080");
    let app: Router = askbar_server::api_router(state.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    Server { addr, state, user, http: client() }
}

impl Server {
    fn url(&self, p: &str) -> String {
        format!("http://{}{p}", self.addr)
    }
    async fn pair(&self, code: &str) -> reqwest::Response {
        self.http.post(self.url("/api/v1/pair")).json(&serde_json::json!({ "code": code })).send().await.unwrap()
    }
    async fn probe(&self, token: Option<&str>) -> u16 {
        let mut r = self.http.get(self.url("/api/v1/whoami"));
        if let Some(t) = token {
            r = r.bearer_auth(t);
        }
        r.send().await.unwrap().status().as_u16()
    }
    fn code(&self, ttl: i64) -> String {
        let db = self.state.db.lock().unwrap();
        let code = auth::create_pair_code(&db, &self.user, "laptop", ttl).unwrap();
        askbar_server::bridge::rebuild(&db).unwrap();
        code
    }
    async fn token(&self) -> String {
        let r = self.pair(&self.code(600)).await;
        assert_eq!(r.status(), 200);
        r.json::<askbar_proto::PairResponse>().await.unwrap().token
    }
}

#[tokio::test]
async fn missing_and_bad_tokens_are_401() {
    let s = start().await;
    assert_eq!(s.probe(None).await, 401);
    assert_eq!(s.probe(Some("abt_nope")).await, 401);
    let r = s.http.get(s.url("/api/v1/whoami")).header("authorization", "Basic abc").send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn valid_token_works_and_updates_last_seen() {
    let s = start().await;
    let token = s.token().await;
    let seen = |s: &Server| -> Option<i64> {
        s.state.db.lock().unwrap().conn().query_row("SELECT last_seen FROM devices", [], |r| r.get(0)).unwrap()
    };
    assert_eq!(seen(&s), None);
    assert_eq!(s.probe(Some(&token)).await, 204);
    assert!(seen(&s).is_some());
}

#[tokio::test]
async fn revoked_token_is_401() {
    let s = start().await;
    let token = s.token().await;
    assert_eq!(s.probe(Some(&token)).await, 204);
    s.state.db.lock().unwrap().conn().execute("UPDATE devices SET revoked_at = 1", []).unwrap();
    assert_eq!(s.probe(Some(&token)).await, 401);
}

#[tokio::test]
async fn expired_code_is_rejected() {
    let s = start().await;
    let code = s.code(-1);
    assert_eq!(s.pair(&code).await.status(), 401);
    assert_eq!(s.pair("not-a-code").await.status(), 401);
}

#[tokio::test]
async fn code_works_once() {
    let s = start().await;
    let code = s.code(600);
    assert_eq!(s.pair(&code).await.status(), 200);
    assert_eq!(s.pair(&code).await.status(), 401);
}

#[tokio::test]
async fn tokens_are_stored_hashed() {
    let s = start().await;
    let token = s.token().await;
    let db = s.state.db.lock().unwrap();
    let stored: String = db.conn().query_row("SELECT hash FROM devices", [], |r| r.get(0)).unwrap();
    assert_ne!(stored, token);
    assert_eq!(stored.len(), 64);
    let name: String = db.conn().query_row("SELECT name FROM devices", [], |r| r.get(0)).unwrap();
    assert_eq!(name, "laptop");
}

#[tokio::test]
async fn health_stays_public() {
    let s = start().await;
    assert_eq!(s.http.get(s.url("/api/v1/health")).send().await.unwrap().status(), 200);
}

fn client() -> reqwest::Client {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert(askbar_proto::CLIENT_VERSION_HEADER, "0.3.0".parse().unwrap());
    reqwest::Client::builder().default_headers(h).build().unwrap()
}
