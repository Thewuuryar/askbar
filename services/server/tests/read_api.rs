use askbar_proto::{Conversation, ConversationDetail, Prefs, ProviderInfo, SearchHit, Settings};
use askbar_server::{auth, AppState};
use std::net::SocketAddr;
use std::sync::Arc;

const PROVIDER_SECRET: &str = "sk-live-VERY-SECRET-123";

struct T {
    addr: SocketAddr,
    state: Arc<AppState>,
    user: String,
    token: String,
    http: reqwest::Client,
    /// The user's personal space.
    space: String,
}

/// A conversation in `user`'s personal space.
fn add_conversation(state: &AppState, user: &str, title: &str, msgs: &[(&str, &str)]) -> String {
    let space = askbar_server::testing::personal_space(state, user);
    let db = space.lock();
    let id = db.create_conversation(user, title).unwrap();
    for (role, content) in msgs {
        db.add_message(user, &id, role, content).unwrap();
    }
    id
}

async fn start() -> T {
    let db = askbar_server::Db::open_in_memory().unwrap();
    let user = db.seed_server().unwrap();
    askbar_server::testing::add_provider(&db, askbar_server::testing::provider("p1", "mock", "http://x/v1", "m1"));
    let code = askbar_server::testing::add_pair_code(&db, &user, "t");
    let state = AppState::new(db, askbar_server::MIN_CLIENT);
    let token = auth::redeem_pair_code(&state.db.lock().unwrap(), &code).unwrap().unwrap();
    let app = askbar_server::client_router(state.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let space = askbar_server::testing::personal_space(&state, &user).id.clone();
    T { addr, state, user, token, http: client(), space }
}

impl T {
    /// A client API path in the user's personal space: `sp("/search?q=x")`.
    fn sp(&self, rest: &str) -> String {
        format!("/api/client/v1/spaces/{}{rest}", self.space)
    }
    fn req(&self, m: reqwest::Method, p: &str) -> reqwest::RequestBuilder {
        self.http.request(m, format!("http://{}{p}", self.addr)).bearer_auth(&self.token)
    }
    async fn get_text(&self, p: &str) -> (u16, String) {
        let r = self.req(reqwest::Method::GET, p).send().await.unwrap();
        (r.status().as_u16(), r.text().await.unwrap())
    }
}

#[tokio::test]
async fn endpoints_return_scoped_data() {
    let t = start().await;
    let c1 = add_conversation(&t.state, &t.user, "mine", &[("user", "hi"), ("assistant", "hello")]);
    // Another user's conversation must be invisible.
    t.state.db.lock().unwrap().conn().execute("INSERT INTO users (id, name, created_at) VALUES ('U2','other',0)", []).unwrap();
    let c2 = add_conversation(&t.state, "U2", "theirs", &[("user", "secret chat")]);

    let list: Vec<Conversation> = serde_json::from_str(&t.get_text(&t.sp("/conversations")).await.1).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].id.as_str(), list[0].title.as_str(), list[0].message_count), (c1.as_str(), "mine", 2));

    let (st, body) = t.get_text(&t.sp(&format!("/conversations/{c1}"))).await;
    assert_eq!(st, 200);
    let d: ConversationDetail = serde_json::from_str(&body).unwrap();
    assert_eq!(d.messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(), ["hi", "hello"]);

    assert_eq!(t.get_text(&t.sp(&format!("/conversations/{c2}"))).await.0, 404);
    assert_eq!(t.req(reqwest::Method::DELETE, &t.sp(&format!("/conversations/{c2}"))).send().await.unwrap().status(), 404);
    assert_eq!(t.req(reqwest::Method::DELETE, &t.sp(&format!("/conversations/{c1}"))).send().await.unwrap().status(), 204);
    assert_eq!(t.get_text(&t.sp(&format!("/conversations/{c1}"))).await.0, 404);
    let theirs = askbar_server::testing::personal_space(&t.state, "U2");
    let n: i64 = theirs.lock().conn().query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1, "other user's rows untouched");

    let providers: Vec<ProviderInfo> = serde_json::from_str(&t.get_text("/api/client/v1/providers").await.1).unwrap();
    assert_eq!(providers, [ProviderInfo { id: "p1".into(), name: "mock".into(), kind: "openai".into(), model: "m1".into() }]);
}

#[tokio::test]
async fn settings_have_defaults_and_no_system_prompt() {
    let t = start().await;
    let (st, body) = t.get_text("/api/client/v1/settings").await;
    assert_eq!(st, 200);
    assert!(!body.contains("system_prompt") && !body.contains("prompt"));
    let s: Settings = serde_json::from_str(&body).unwrap();
    assert_eq!(s, Settings::default());
    // What the admin pushed is served; the system prompt in the same snapshot is not.
    askbar_server::testing::update_snapshot(&t.state.db.lock().unwrap(), |s| {
        s.display.width = 900;
        s.system_prompt = Some("TOP SECRET PROMPT".into());
    });
    let (_, body) = t.get_text("/api/client/v1/settings").await;
    let s: Settings = serde_json::from_str(&body).unwrap();
    assert_eq!((s.display.width, s.display.max_height), (900, 640));
    assert!(!body.contains("TOP SECRET"));
}

#[tokio::test]
async fn no_response_contains_provider_key_material() {
    let t = start().await;
    let c1 = add_conversation(&t.state, &t.user, "c", &[("user", "hi")]);
    // The server holds no provider key at all now (ADR 0054); nothing it serves may name one or look like one.
    let needles = [PROVIDER_SECRET.to_string(), "api_key".to_string()];
    for p in [
        "/api/client/v1/health",
        "/api/client/v1/me",
        "/api/client/v1/providers",
        &t.sp("/conversations"),
        &t.sp(&format!("/conversations/{c1}")),
        &t.sp("/conversations/nope"),
        "/api/client/v1/settings",
    ] {
        let r = t.req(reqwest::Method::GET, p).send().await.unwrap();
        let headers = format!("{:?}", r.headers());
        let body = r.text().await.unwrap();
        for n in &needles {
            assert!(!body.contains(n.as_str()) && !headers.contains(n.as_str()), "{p} leaks {n}");
        }
    }
}

#[tokio::test]
async fn read_endpoints_require_a_token() {
    let t = start().await;
    for p in ["/api/client/v1/providers", &t.sp("/conversations"), &t.sp("/conversations/C1"), "/api/client/v1/settings"] {
        let r = t.http.get(format!("http://{}{p}", t.addr)).send().await.unwrap();
        assert_eq!(r.status(), 401, "{p}");
    }
}

fn client() -> reqwest::Client {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert(askbar_proto::CLIENT_VERSION_HEADER, "0.6.0".parse().unwrap());
    reqwest::Client::builder().default_headers(h).build().unwrap()
}

#[tokio::test]
async fn search_returns_only_my_matching_messages() {
    let t = start().await;
    let c1 = add_conversation(&t.state, &t.user, "mine", &[("user", "feed the zebra"), ("assistant", "done")]);
    t.state.db.lock().unwrap().conn().execute("INSERT INTO users (id, name, created_at) VALUES ('U2','other',0)", []).unwrap();
    add_conversation(&t.state, "U2", "theirs", &[("user", "zebra secrets")]);

    let (st, body) = t.get_text(&t.sp("/search?q=zebra")).await;
    assert_eq!(st, 200);
    let hits: Vec<SearchHit> = serde_json::from_str(&body).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].conversation_id.as_str(), hits[0].role.as_str()), (c1.as_str(), "user"));
    assert!(!body.contains("secrets"));

    assert_eq!(t.get_text(&t.sp("/search?q=")).await, (200, "[]".to_string()));
    assert_eq!(t.get_text(&t.sp(&format!("/search?q={}", "x".repeat(201)))).await.0, 400);
    let r = t.http.get(format!("http://{}{}", t.addr, t.sp("/search?q=zebra"))).send().await.unwrap();
    assert_eq!(r.status(), 401, "search needs a token");
}

#[tokio::test]
async fn prefs_are_shared_by_a_users_clients_and_not_by_other_users() {
    let t = start().await;
    let get = |token: String| {
        let (http, addr) = (t.http.clone(), t.addr);
        async move { http.get(format!("http://{addr}/api/client/v1/me/prefs")).bearer_auth(token).send().await.unwrap() }
    };
    // Nothing chosen yet.
    assert_eq!(serde_json::from_str::<Prefs>(&get(t.token.clone()).await.text().await.unwrap()).unwrap(), Prefs::default());

    // A second client of the same user, and a client of another user.
    let (second, other) = {
        let db = t.state.db.lock().unwrap();
        db.conn().execute("INSERT INTO users (id, name, created_at) VALUES ('U2','other',0)", []).unwrap();
        let c1 = askbar_server::testing::add_pair_code(&db, &t.user, "second client");
        let c2 = askbar_server::testing::add_pair_code(&db, "U2", "their client");
        (auth::redeem_pair_code(&db, &c1).unwrap().unwrap(), auth::redeem_pair_code(&db, &c2).unwrap().unwrap())
    };

    let put =
        t.req(reqwest::Method::PUT, "/api/client/v1/me/prefs").json(&Prefs { active_provider_id: Some("p1".into()) }).send().await.unwrap();
    assert_eq!(put.status(), 200);
    for token in [t.token.clone(), second] {
        let p: Prefs = serde_json::from_str(&get(token).await.text().await.unwrap()).unwrap();
        assert_eq!(p.active_provider_id.as_deref(), Some("p1"), "every client of the user sees it");
    }
    let p: Prefs = serde_json::from_str(&get(other.clone()).await.text().await.unwrap()).unwrap();
    assert_eq!(p, Prefs::default(), "another user does not");

    // An unknown provider is refused and changes nothing; null clears the choice.
    let bad = t
        .req(reqwest::Method::PUT, "/api/client/v1/me/prefs")
        .json(&Prefs { active_provider_id: Some("nope".into()) })
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 404);
    let still: Prefs = serde_json::from_str(&get(t.token.clone()).await.text().await.unwrap()).unwrap();
    assert_eq!(still.active_provider_id.as_deref(), Some("p1"));
    t.req(reqwest::Method::PUT, "/api/client/v1/me/prefs").json(&Prefs::default()).send().await.unwrap();
    let cleared: Prefs = serde_json::from_str(&get(t.token.clone()).await.text().await.unwrap()).unwrap();
    assert_eq!(cleared, Prefs::default());
    let unauth = t.http.get(format!("http://{}/api/client/v1/me/prefs", t.addr)).send().await.unwrap();
    assert_eq!(unauth.status(), 401);
}

#[tokio::test]
async fn read_api_failures_are_problems_with_a_code_and_a_sentence() {
    let t = start().await;
    let long = "x".repeat(201);
    let cases: Vec<(reqwest::Method, String, u16, &str)> = vec![
        (reqwest::Method::GET, t.sp("/conversations/nope"), 404, "not_found"),
        (reqwest::Method::DELETE, t.sp("/conversations/nope"), 404, "not_found"),
        (reqwest::Method::GET, t.sp(&format!("/search?q={long}")), 400, "bad_request"),
        (reqwest::Method::GET, t.sp("/generations?status=done"), 400, "bad_request"),
        (reqwest::Method::GET, t.sp("/generations/nope/stream"), 404, "not_found"),
        (reqwest::Method::POST, t.sp("/generations/nope/cancel"), 404, "not_found"),
        (reqwest::Method::GET, "/api/client/v1/no-such-endpoint".into(), 404, "not_found"),
        (reqwest::Method::DELETE, "/api/client/v1/providers".into(), 405, "method_not_allowed"),
    ];
    for (method, path, status, code) in cases {
        let r = t.req(method.clone(), &path).send().await.unwrap();
        assert_eq!(r.status().as_u16(), status, "{method} {path}");
        assert_eq!(r.headers()["content-type"], "application/problem+json", "{method} {path}");
        let p: askbar_proto::Problem = r.json().await.unwrap();
        assert_eq!((p.error.as_str(), p.status), (code, status), "{method} {path}");
        assert!(!p.detail.is_empty());
    }
    let r = t
        .req(reqwest::Method::PUT, "/api/client/v1/me/prefs")
        .json(&Prefs { active_provider_id: Some("no-such-provider".into()) })
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(r.json::<askbar_proto::Problem>().await.unwrap().error, "not_found");
}
