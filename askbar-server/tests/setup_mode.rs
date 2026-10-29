use askbar_server::{admin, secrets, AppState};
use serde_json::json;

const PASSWORD: &str = "correct horse battery";

async fn serve(app: axum::Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr
}

fn headers() -> reqwest::header::HeaderMap {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert(askbar_proto::CLIENT_VERSION_HEADER, "0.1.0".parse().unwrap());
    h
}

#[tokio::test]
async fn without_a_master_key_only_health_and_admin_work() {
    let dir = std::env::temp_dir().join(format!("askbar-setup-mode-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let key_path = dir.join("master.key");

    let db = askbar_core::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    admin::init_password(&db, PASSWORD).unwrap();
    let state = AppState::build(db, None, "0.1.0", "http://localhost:8080", key_path.clone());
    assert!(!state.setup_complete());
    let (api, adm) = (serve(askbar_server::api_router(state.clone())).await, serve(askbar_server::admin_router(state.clone())).await);
    let http = reqwest::Client::builder().default_headers(headers()).build().unwrap();

    assert_eq!(http.get(format!("{api}/api/v1/health")).send().await.unwrap().status(), 200);
    for (m, p) in [("POST", "/api/v1/pair"), ("POST", "/api/v1/chat"), ("GET", "/api/v1/providers"), ("GET", "/api/v1/conversations"), ("GET", "/api/v1/whoami")] {
        let r = http.request(m.parse().unwrap(), format!("{api}{p}")).bearer_auth("abt_x").json(&json!({ "code": "x", "provider": "p", "text": "t" })).send().await.unwrap();
        assert_eq!(r.status(), 503, "{p}");
    }

    // The admin site works, but anything that needs the key says so.
    let r = http.post(format!("{adm}/admin/login")).header("x-askbar-admin", "1").json(&json!({ "password": PASSWORD })).send().await.unwrap();
    assert_eq!(r.status(), 204);
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    let admin_call = |m: reqwest::Method, p: &str, body: serde_json::Value| http.request(m, format!("{adm}{p}")).header("x-askbar-admin", "1").header("cookie", &cookie).json(&body).send();
    assert_eq!(admin_call(reqwest::Method::GET, "/admin/api/providers", json!({})).await.unwrap().status(), 200);
    let create = json!({ "name": "p", "type": "openai", "model": "m", "api_key": "sk-1" });
    let r = admin_call(reqwest::Method::POST, "/admin/api/providers", create.clone()).await.unwrap();
    assert_eq!(r.status(), 503);
    assert!(r.text().await.unwrap().contains("master key"));

    // Once the key exists the same server carries on without a restart.
    state.set_secrets(secrets::generate_key_file(&key_path).unwrap());
    assert!(state.setup_complete());
    assert_eq!(admin_call(reqwest::Method::POST, "/admin/api/providers", create).await.unwrap().status(), 201);
    assert_eq!(http.get(format!("{api}/api/v1/providers")).bearer_auth("abt_x").send().await.unwrap().status(), 401, "past the setup gate, auth applies");
    let _ = std::fs::remove_dir_all(&dir);
}
