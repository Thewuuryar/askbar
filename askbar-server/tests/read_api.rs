use askbar_proto::{Conversation, ConversationDetail, Prefs, ProviderInfo, SearchHit, Settings};
use askbar_server::{auth, secrets::Secrets, AppState};
use std::net::SocketAddr;
use std::sync::Arc;

const KEY: &str = "0123456789abcdef0123456789abcdef";
const PROVIDER_SECRET: &str = "sk-live-VERY-SECRET-123";

struct T {
    addr: SocketAddr,
    state: Arc<AppState>,
    user: String,
    token: String,
    http: reqwest::Client,
}

fn add_conversation(state: &AppState, user: &str, title: &str, msgs: &[(&str, &str)]) -> String {
    let db = state.db.lock().unwrap();
    let id = db.create_conversation(user, title).unwrap();
    for (role, content) in msgs {
        db.add_message(user, &id, role, content).unwrap();
    }
    id
}

async fn start() -> T {
    let db = askbar_server::Db::open_in_memory().unwrap();
    let user = db.seed_server().unwrap();
    let secrets = Secrets::new(KEY).unwrap();
    askbar_server::testing::add_provider(&db, askbar_server::testing::provider("p1", "mock", "http://x/v1", "m1"));
    askbar_server::testing::store_key(&db, &secrets, "p1", PROVIDER_SECRET);
    let code = askbar_server::testing::add_pair_code(&db, &user, "t");
    let state = AppState::new(db, secrets, askbar_server::MIN_CLIENT);
    let token = auth::redeem_pair_code(&state.db.lock().unwrap(), &code).unwrap().unwrap();
    let app = askbar_server::client_router(state.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    T { addr, state, user, token, http: client() }
}

impl T {
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

    let list: Vec<Conversation> = serde_json::from_str(&t.get_text("/api/client/v1/conversations").await.1).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].id.as_str(), list[0].title.as_str(), list[0].message_count), (c1.as_str(), "mine", 2));

    let (st, body) = t.get_text(&format!("/api/client/v1/conversations/{c1}")).await;
    assert_eq!(st, 200);
    let d: ConversationDetail = serde_json::from_str(&body).unwrap();
    assert_eq!(d.messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(), ["hi", "hello"]);

    assert_eq!(t.get_text(&format!("/api/client/v1/conversations/{c2}")).await.0, 404);
    assert_eq!(t.req(reqwest::Method::DELETE, &format!("/api/client/v1/conversations/{c2}")).send().await.unwrap().status(), 404);
    assert_eq!(t.req(reqwest::Method::DELETE, &format!("/api/client/v1/conversations/{c1}")).send().await.unwrap().status(), 204);
    assert_eq!(t.get_text(&format!("/api/client/v1/conversations/{c1}")).await.0, 404);
    let n: i64 = t.state.db.lock().unwrap().conn().query_row("SELECT COUNT(*) FROM messages WHERE user_id = 'U2'", [], |r| r.get(0)).unwrap();
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
    let enc: Vec<u8> = t.state.db.lock().unwrap().conn().query_row("SELECT api_key_enc FROM providers", [], |r| r.get(0)).unwrap();
    use base64::{engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}, Engine};
    let hex: String = enc.iter().map(|b| format!("{b:02x}")).collect();
    let needles = [PROVIDER_SECRET.to_string(), STANDARD.encode(&enc), URL_SAFE_NO_PAD.encode(&enc), hex, "api_key".to_string()];
    for p in ["/api/client/v1/health", "/api/client/v1/me", "/api/client/v1/providers", "/api/client/v1/conversations", &format!("/api/client/v1/conversations/{c1}"), "/api/client/v1/conversations/nope", "/api/client/v1/settings"] {
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
    for p in ["/api/client/v1/providers", "/api/client/v1/conversations", "/api/client/v1/conversations/C1", "/api/client/v1/settings"] {
        let r = t.http.get(format!("http://{}{p}", t.addr)).send().await.unwrap();
        assert_eq!(r.status(), 401, "{p}");
    }
}

fn client() -> reqwest::Client {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert(askbar_proto::CLIENT_VERSION_HEADER, "0.5.0".parse().unwrap());
    reqwest::Client::builder().default_headers(h).build().unwrap()
}

#[tokio::test]
async fn search_returns_only_my_matching_messages() {
    let t = start().await;
    let c1 = add_conversation(&t.state, &t.user, "mine", &[("user", "feed the zebra"), ("assistant", "done")]);
    t.state.db.lock().unwrap().conn().execute("INSERT INTO users (id, name, created_at) VALUES ('U2','other',0)", []).unwrap();
    add_conversation(&t.state, "U2", "theirs", &[("user", "zebra secrets")]);

    let (st, body) = t.get_text("/api/client/v1/search?q=zebra").await;
    assert_eq!(st, 200);
    let hits: Vec<SearchHit> = serde_json::from_str(&body).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].conversation_id.as_str(), hits[0].role.as_str()), (c1.as_str(), "user"));
    assert!(!body.contains("secrets"));

    assert_eq!(t.get_text("/api/client/v1/search?q=").await, (200, "[]".to_string()));
    assert_eq!(t.get_text(&format!("/api/client/v1/search?q={}", "x".repeat(201))).await.0, 400);
    let r = t.http.get(format!("http://{}/api/client/v1/search?q=zebra", t.addr)).send().await.unwrap();
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

    let put = t.req(reqwest::Method::PUT, "/api/client/v1/me/prefs").json(&Prefs { active_provider_id: Some("p1".into()), active_model: None }).send().await.unwrap();
    assert_eq!(put.status(), 200);
    for token in [t.token.clone(), second] {
        let p: Prefs = serde_json::from_str(&get(token).await.text().await.unwrap()).unwrap();
        assert_eq!(p.active_provider_id.as_deref(), Some("p1"), "every client of the user sees it");
    }
    let p: Prefs = serde_json::from_str(&get(other.clone()).await.text().await.unwrap()).unwrap();
    assert_eq!(p, Prefs::default(), "another user does not");

    // An unknown provider is refused and changes nothing; null clears the choice.
    let bad = t.req(reqwest::Method::PUT, "/api/client/v1/me/prefs").json(&Prefs { active_provider_id: Some("nope".into()), active_model: None }).send().await.unwrap();
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
        (reqwest::Method::GET, "/api/client/v1/conversations/nope".into(), 404, "not_found"),
        (reqwest::Method::DELETE, "/api/client/v1/conversations/nope".into(), 404, "not_found"),
        (reqwest::Method::GET, format!("/api/client/v1/search?q={long}"), 400, "bad_request"),
        (reqwest::Method::GET, "/api/client/v1/generations?status=done".into(), 400, "bad_request"),
        (reqwest::Method::GET, "/api/client/v1/generations/nope/stream".into(), 404, "not_found"),
        (reqwest::Method::POST, "/api/client/v1/generations/nope/cancel".into(), 404, "not_found"),
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
    let r = t.req(reqwest::Method::PUT, "/api/client/v1/me/prefs").json(&Prefs { active_provider_id: Some("no-such-provider".into()), active_model: None }).send().await.unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(r.json::<askbar_proto::Problem>().await.unwrap().error, "not_found");
}

#[tokio::test]
async fn a_conversation_stores_its_model_and_only_accepts_ones_the_provider_offers() {
    let t = start().await;
    askbar_server::testing::update_snapshot(&t.state.db.lock().unwrap(), |s| s.providers[0].models = vec!["m2".into()]);
    let conversation = {
        let db = t.state.db.lock().unwrap();
        let user: String = db.conn().query_row("SELECT id FROM users LIMIT 1", [], |r| r.get(0)).unwrap();
        db.create_conversation(&user, "t").unwrap()
    };
    let path = format!("/api/client/v1/conversations/{conversation}");
    let put = |path: String, provider: &str, model: Option<&str>| {
        t.req(reqwest::Method::PUT, &format!("{path}/model")).json(&askbar_proto::ConversationModel { provider_id: provider.into(), model: model.map(String::from) }).send()
    };
    let detail: askbar_proto::ConversationDetail = serde_json::from_str(&t.get_text(&path).await.1).unwrap();
    assert_eq!((detail.provider_id, detail.model), (None, None));

    assert_eq!(put(path.clone(), "p1", Some("m2")).await.unwrap().status(), 200);
    let detail: askbar_proto::ConversationDetail = serde_json::from_str(&t.get_text(&path).await.1).unwrap();
    assert_eq!((detail.provider_id.as_deref(), detail.model.as_deref()), (Some("p1"), Some("m2")));

    assert_eq!(put(path.clone(), "p1", Some("zzz")).await.unwrap().status(), 404);
    assert_eq!(put(path.clone(), "nope", None).await.unwrap().status(), 404);
    assert_eq!(put("/api/client/v1/conversations/nope".into(), "p1", None).await.unwrap().status(), 404);
    let detail: askbar_proto::ConversationDetail = serde_json::from_str(&t.get_text(&path).await.1).unwrap();
    assert_eq!(detail.model.as_deref(), Some("m2"), "a refused change changes nothing");
}

#[tokio::test]
async fn a_provider_lists_its_models_and_prefs_only_accept_ones_it_offers() {
    let t = start().await;
    let (st, body) = t.get_text("/api/client/v1/providers/p1/models").await;
    let m: askbar_proto::ProviderModels = serde_json::from_str(&body).unwrap();
    assert_eq!((st, m.default.as_str(), m.models.clone()), (200, "m1", vec!["m1".to_string()]), "a provider with no list offers its default");

    askbar_server::testing::update_snapshot(&t.state.db.lock().unwrap(), |s| s.providers[0].models = vec!["m2".into(), "m1".into(), "m2".into()]);
    let m: askbar_proto::ProviderModels = serde_json::from_str(&t.get_text("/api/client/v1/providers/p1/models").await.1).unwrap();
    assert_eq!(m.models, ["m1", "m2"], "the default first, no repeats");
    assert_eq!(t.get_text("/api/client/v1/providers/nope/models").await.0, 404);

    let put = |p: Prefs| t.req(reqwest::Method::PUT, "/api/client/v1/me/prefs").json(&p).send();
    let ok = put(Prefs { active_provider_id: Some("p1".into()), active_model: Some("m2".into()) }).await.unwrap();
    assert_eq!(ok.status(), 200);
    let got: Prefs = serde_json::from_str(&t.get_text("/api/client/v1/me/prefs").await.1).unwrap();
    assert_eq!((got.active_provider_id.as_deref(), got.active_model.as_deref()), (Some("p1"), Some("m2")));

    let bad = put(Prefs { active_provider_id: Some("p1".into()), active_model: Some("zzz".into()) }).await.unwrap();
    assert_eq!(bad.status(), 404);
    let orphan = put(Prefs { active_provider_id: None, active_model: Some("m2".into()) }).await.unwrap();
    assert_eq!(orphan.status(), 400, "a request that makes no sense, not a missing thing");
    let still: Prefs = serde_json::from_str(&t.get_text("/api/client/v1/me/prefs").await.1).unwrap();
    assert_eq!(still.active_model.as_deref(), Some("m2"), "a refused change changes nothing");
}
