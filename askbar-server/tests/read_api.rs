use askbar_proto::{Conversation, ConversationDetail, ProviderInfo, Settings};
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

fn add_conversation(state: &AppState, user: &str, pid: &str, title: &str, msgs: &[(&str, &str)]) {
    let db = state.db.lock().unwrap();
    let id = db.create_conversation(title).unwrap();
    db.conn().execute("UPDATE conversations SET public_id = ?1, user_id = ?2 WHERE id = ?3", rusqlite::params![pid, user, id]).unwrap();
    for (i, (role, content)) in msgs.iter().enumerate() {
        let m = db.add_message(id, role, content).unwrap();
        db.conn().execute("UPDATE messages SET public_id = ?1, user_id = ?2 WHERE id = ?3", rusqlite::params![format!("{pid}-m{i}"), user, m]).unwrap();
    }
}

async fn start() -> T {
    let db = askbar_core::Db::open_in_memory().unwrap();
    let user = db.seed_server().unwrap();
    let secrets = Secrets::new(KEY).unwrap();
    let enc = secrets.encrypt(PROVIDER_SECRET);
    db.conn()
        .execute(
            "INSERT INTO providers (id, name, type, base_url, model, api_key_enc, created_at) VALUES ('p1','mock','openai','http://x/v1','m1',?1,0)",
            [enc],
        )
        .unwrap();
    let code = auth::create_pair_code(&db, &user, "t", 600).unwrap();
    let state = AppState::new(db, secrets);
    let token = auth::redeem_pair_code(&state.db.lock().unwrap(), &code).unwrap().unwrap();
    let app = askbar_server::api_router(state.clone());
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
    add_conversation(&t.state, &t.user, "C1", "mine", &[("user", "hi"), ("assistant", "hello")]);
    // Another user's conversation must be invisible.
    t.state.db.lock().unwrap().conn().execute("INSERT INTO users (id, name, created_at) VALUES ('U2','other',0)", []).unwrap();
    add_conversation(&t.state, "U2", "C2", "theirs", &[("user", "secret chat")]);

    let list: Vec<Conversation> = serde_json::from_str(&t.get_text("/api/v1/conversations").await.1).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].id.as_str(), list[0].title.as_str(), list[0].message_count), ("C1", "mine", 2));

    let (st, body) = t.get_text("/api/v1/conversations/C1").await;
    assert_eq!(st, 200);
    let d: ConversationDetail = serde_json::from_str(&body).unwrap();
    assert_eq!(d.messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(), ["hi", "hello"]);

    assert_eq!(t.get_text("/api/v1/conversations/C2").await.0, 404);
    assert_eq!(t.req(reqwest::Method::DELETE, "/api/v1/conversations/C2").send().await.unwrap().status(), 404);
    assert_eq!(t.req(reqwest::Method::DELETE, "/api/v1/conversations/C1").send().await.unwrap().status(), 204);
    assert_eq!(t.get_text("/api/v1/conversations/C1").await.0, 404);
    let n: i64 = t.state.db.lock().unwrap().conn().query_row("SELECT COUNT(*) FROM messages WHERE user_id = 'U2'", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1, "other user's rows untouched");

    let providers: Vec<ProviderInfo> = serde_json::from_str(&t.get_text("/api/v1/providers").await.1).unwrap();
    assert_eq!(providers, [ProviderInfo { name: "mock".into(), kind: "openai".into(), model: "m1".into() }]);
}

#[tokio::test]
async fn settings_have_defaults_and_no_system_prompt() {
    let t = start().await;
    let (st, body) = t.get_text("/api/v1/settings").await;
    assert_eq!(st, 200);
    assert!(!body.contains("system_prompt") && !body.contains("prompt"));
    let s: Settings = serde_json::from_str(&body).unwrap();
    assert_eq!(s, Settings::default());
    // A partial stored override is merged over the defaults.
    t.state.db.lock().unwrap().conn().execute("INSERT INTO settings VALUES ('display', '{\"width\": 900}')", []).unwrap();
    t.state.db.lock().unwrap().conn().execute("INSERT INTO settings VALUES ('system_prompt', 'TOP SECRET PROMPT')", []).unwrap();
    let (_, body) = t.get_text("/api/v1/settings").await;
    let s: Settings = serde_json::from_str(&body).unwrap();
    assert_eq!((s.display.width, s.display.max_height), (900, 640));
    assert!(!body.contains("TOP SECRET"));
}

#[tokio::test]
async fn no_response_contains_provider_key_material() {
    let t = start().await;
    add_conversation(&t.state, &t.user, "C1", "c", &[("user", "hi")]);
    let enc: Vec<u8> = t.state.db.lock().unwrap().conn().query_row("SELECT api_key_enc FROM providers", [], |r| r.get(0)).unwrap();
    use base64::{engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}, Engine};
    let hex: String = enc.iter().map(|b| format!("{b:02x}")).collect();
    let needles = [PROVIDER_SECRET.to_string(), STANDARD.encode(&enc), URL_SAFE_NO_PAD.encode(&enc), hex, "api_key".to_string()];
    for p in ["/api/v1/health", "/api/v1/whoami", "/api/v1/providers", "/api/v1/conversations", "/api/v1/conversations/C1", "/api/v1/conversations/nope", "/api/v1/settings"] {
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
    for p in ["/api/v1/providers", "/api/v1/conversations", "/api/v1/conversations/C1", "/api/v1/settings"] {
        let r = t.http.get(format!("http://{}{p}", t.addr)).send().await.unwrap();
        assert_eq!(r.status(), 401, "{p}");
    }
}

fn client() -> reqwest::Client {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert(askbar_proto::CLIENT_VERSION_HEADER, "0.1.0".parse().unwrap());
    reqwest::Client::builder().default_headers(h).build().unwrap()
}
