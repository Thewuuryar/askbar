use askbar_server::{secrets::Secrets, AppState};

async fn start(min: &str) -> String {
    let db = askbar_server::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    let state = AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), min, "http://localhost:8080");
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let app = askbar_server::api_router(state);
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{addr}")
}

async fn status(url: &str, path: &str, version: Option<&str>) -> u16 {
    let mut r = reqwest::Client::new().get(format!("{url}{path}")).bearer_auth("abt_not_a_real_token");
    if let Some(v) = version {
        r = r.header("X-Askbar-Client-Version", v);
    }
    r.send().await.unwrap().status().as_u16()
}

#[tokio::test]
async fn too_old_is_426_before_auth_is_even_checked() {
    let url = start(askbar_server::MIN_CLIENT).await;
    for p in ["/api/v1/providers", "/api/v1/conversations", "/api/v1/whoami", "/api/v1/settings"] {
        assert_eq!(status(&url, p, Some("0.2.9")).await, 426, "{p}");
    }
    let r = reqwest::Client::new()
        .post(format!("{url}/api/v1/pair"))
        .header("X-Askbar-Client-Version", "0.1.0")
        .json(&serde_json::json!({"code": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 426);
}

#[tokio::test]
async fn current_or_newer_passes_the_gate() {
    let url = start(askbar_server::MIN_CLIENT).await;
    // Past the gate the bogus token is rejected by auth instead.
    assert_eq!(status(&url, "/api/v1/whoami", Some("0.3.0")).await, 401);
    assert_eq!(status(&url, "/api/v1/whoami", Some("0.10.0")).await, 401);
    assert_eq!(status(&url, "/api/v1/whoami", Some("1.0.0-beta")).await, 401);
}

#[tokio::test]
async fn missing_or_garbled_header_is_400() {
    let url = start(askbar_server::MIN_CLIENT).await;
    assert_eq!(status(&url, "/api/v1/whoami", None).await, 400);
    assert_eq!(status(&url, "/api/v1/whoami", Some("banana")).await, 400);
}

#[tokio::test]
async fn health_needs_no_header_and_reports_versions() {
    let url = start(askbar_server::MIN_CLIENT).await;
    let r = reqwest::get(format!("{url}/api/v1/health")).await.unwrap();
    assert_eq!(r.status(), 200);
    let h: askbar_proto::Health = r.json().await.unwrap();
    assert_eq!((h.api_version, h.min_client.as_str()), (askbar_server::API_VERSION, "0.3.0"));
}

#[tokio::test]
async fn the_minimum_client_is_0_3_0() {
    assert_eq!(askbar_server::MIN_CLIENT, "0.3.0");
    let url = start(askbar_server::MIN_CLIENT).await;
    // v0.2.x clients speak the old wire format (numeric ids, provider names) and must be refused.
    assert_eq!(status(&url, "/api/v1/conversations", Some("0.2.0")).await, 426);
    assert_eq!(status(&url, "/api/v1/conversations", Some("0.3.0")).await, 401);
}
