use askbar_server::AppState;
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
    start_with(true).await
}

/// `configured`: whether the admin has pushed a snapshot yet.
async fn start_with(configured: bool) -> Server {
    let db = askbar_server::Db::open_in_memory().unwrap();
    let user = db.seed_server().unwrap();
    if configured {
        askbar_server::testing::update_snapshot(&db, |_| {});
    }
    let state = AppState::new(db, askbar_server::MIN_CLIENT);
    let app: Router = askbar_server::client_router(state.clone());
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
        self.http.post(self.url("/api/client/v1/pairings")).json(&serde_json::json!({ "code": code })).send().await.unwrap()
    }
    async fn probe(&self, token: Option<&str>) -> u16 {
        let mut r = self.http.get(self.url("/api/client/v1/me"));
        if let Some(t) = token {
            r = r.bearer_auth(t);
        }
        r.send().await.unwrap().status().as_u16()
    }
    fn code(&self, ttl: i64) -> String {
        askbar_server::testing::add_pair_code_expiring(&self.state.db.lock().unwrap(), &self.user, "laptop", ttl)
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
    let r = s.http.get(s.url("/api/client/v1/me")).header("authorization", "Basic abc").send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn valid_token_works_and_updates_last_seen() {
    let s = start().await;
    let token = s.token().await;
    let seen = |s: &Server| -> Option<i64> {
        s.state.db.lock().unwrap().conn().query_row("SELECT last_seen FROM clients", [], |r| r.get(0)).unwrap()
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
    s.state.db.lock().unwrap().conn().execute("UPDATE clients SET revoked_at = 1", []).unwrap();
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
    let stored: String = db.conn().query_row("SELECT hash FROM clients", [], |r| r.get(0)).unwrap();
    assert_ne!(stored, token);
    assert_eq!(stored.len(), 64);
    let name: String = db.conn().query_row("SELECT name FROM clients", [], |r| r.get(0)).unwrap();
    assert_eq!(name, "laptop");
}

#[tokio::test]
async fn health_stays_public() {
    let s = start().await;
    assert_eq!(s.http.get(s.url("/api/client/v1/health")).send().await.unwrap().status(), 200);
}

fn client() -> reqwest::Client {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert(askbar_proto::CLIENT_VERSION_HEADER, "0.6.0".parse().unwrap());
    reqwest::Client::builder().default_headers(h).build().unwrap()
}

#[tokio::test]
async fn a_refused_token_and_a_bad_pairing_code_are_problems() {
    let s = start().await;
    let ask = |token: Option<&str>| {
        let mut r = s.http.get(s.url("/api/client/v1/me"));
        if let Some(t) = token {
            r = r.bearer_auth(t);
        }
        r.send()
    };
    for token in [None, Some("abt_wrong")] {
        let r = ask(token).await.unwrap();
        assert_eq!(r.status(), 401);
        assert_eq!(r.headers()["www-authenticate"], "Bearer");
        assert_eq!(r.headers()["content-type"], "application/problem+json");
        let p: askbar_proto::Problem = r.json().await.unwrap();
        assert_eq!(p.error, "unauthorized");
    }
    let r = s.pair("not-a-code").await;
    assert_eq!(r.status(), 401);
    let p: askbar_proto::Problem = r.json().await.unwrap();
    assert_eq!(p.error, "unauthorized");
    assert!(p.detail.contains("pairing code"));
    // A body that is not JSON is a bad request problem, not plain text.
    let r = s.http.post(s.url("/api/client/v1/pairings")).header("content-type", "application/json").body("nope").send().await.unwrap();
    assert_eq!(r.status(), 400);
    assert_eq!(r.json::<askbar_proto::Problem>().await.unwrap().error, "bad_request");
}

#[tokio::test]
async fn before_the_first_push_every_client_route_but_health_is_503_not_configured() {
    // ADR 0028 rule 3. The stored snapshot survives a restart, so only a never-configured server is closed.
    let s = start_with(false).await;
    let version = |r: reqwest::RequestBuilder| r.header("x-askbar-client-version", askbar_server::MIN_CLIENT);
    assert_eq!(s.http.get(s.url("/api/client/v1/health")).send().await.unwrap().status(), 200);
    for r in [
        version(s.http.get(s.url("/api/client/v1/me"))).bearer_auth("abt_anything"),
        version(s.http.post(s.url("/api/client/v1/pairings"))).json(&serde_json::json!({ "code": "x" })),
    ] {
        let r = r.send().await.unwrap();
        assert_eq!(r.status(), 503);
        assert_eq!(r.headers()["retry-after"], "5");
        let p: askbar_proto::Problem = r.json().await.unwrap();
        assert_eq!(p.error, "not_configured");
    }
    assert_eq!(s.http.get(s.url("/api/v1/chat")).send().await.unwrap().status(), 426, "the old prefix still says update the app");
    assert_eq!(s.http.get(s.url("/api/client/v1/no-such-route")).send().await.unwrap().status(), 404, "an unknown path is still a 404");
    // The first push opens it.
    askbar_server::testing::update_snapshot(&s.state.db.lock().unwrap(), |_| {});
    assert_eq!(version(s.http.get(s.url("/api/client/v1/me"))).bearer_auth("abt_anything").send().await.unwrap().status(), 401);
}
