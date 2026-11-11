use askbar_server::AppState;

async fn start(min: &str) -> String {
    let db = askbar_server::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    // Configured, as after the admin's first push, so a request past the version gate reaches auth.
    askbar_server::testing::update_snapshot(&db, |_| {});
    let state = AppState::new(db, min);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let app = askbar_server::client_router(state);
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
    for p in ["/api/client/v1/models", "/api/client/v1/spaces", "/api/client/v1/me", "/api/client/v1/settings"] {
        assert_eq!(status(&url, p, Some("0.2.9")).await, 426, "{p}");
    }
    let r = reqwest::Client::new()
        .post(format!("{url}/api/client/v1/pairings"))
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
    assert_eq!(status(&url, "/api/client/v1/me", Some("0.6.0")).await, 401);
    assert_eq!(status(&url, "/api/client/v1/me", Some("0.10.0")).await, 401);
    assert_eq!(status(&url, "/api/client/v1/me", Some("1.0.0-beta")).await, 401);
}

#[tokio::test]
async fn missing_or_garbled_header_is_400() {
    let url = start(askbar_server::MIN_CLIENT).await;
    assert_eq!(status(&url, "/api/client/v1/me", None).await, 400);
    assert_eq!(status(&url, "/api/client/v1/me", Some("banana")).await, 400);
}

#[tokio::test]
async fn health_needs_no_header_and_reports_versions() {
    let url = start(askbar_server::MIN_CLIENT).await;
    let r = reqwest::get(format!("{url}/api/client/v1/health")).await.unwrap();
    assert_eq!(r.status(), 200);
    let h: askbar_proto::Health = r.json().await.unwrap();
    assert_eq!((h.api_version, h.min_client.as_str()), (askbar_server::API_VERSION, "0.6.0"));
}

#[tokio::test]
async fn the_minimum_client_is_0_6_0() {
    assert_eq!(askbar_server::MIN_CLIENT, "0.6.0");
    let url = start(askbar_server::MIN_CLIENT).await;
    // v0.2.x clients speak the old wire format (numeric ids, provider names) and must be refused.
    assert_eq!(status(&url, "/api/client/v1/spaces", Some("0.2.0")).await, 426);
    // v0.3.x clients stop the reply when the connection drops and know nothing of queued messages or
    // resuming; since 0.4.0 the server keeps generating, so they must update too.
    assert_eq!(status(&url, "/api/client/v1/spaces", Some("0.3.0")).await, 426);
    // 0.4.x apps speak the old paths and error shapes; 0.5.0 moved the API to /api/client/v1 with problem details.
    assert_eq!(status(&url, "/api/client/v1/spaces", Some("0.4.0")).await, 426);
    // 0.5.x apps call the flat conversation, search and chat routes; 0.6.0 moved them under /spaces/{space_id}.
    assert_eq!(status(&url, "/api/client/v1/spaces", Some("0.5.0")).await, 426);
    assert_eq!(status(&url, "/api/client/v1/spaces", Some("0.6.0")).await, 401);
}

#[tokio::test]
async fn the_gate_answers_with_problems_a_program_can_branch_on() {
    let url = start(askbar_server::MIN_CLIENT).await;
    let http = reqwest::Client::new();
    let get = |v: Option<&str>| {
        let mut r = http.get(format!("{url}/api/client/v1/me")).bearer_auth("abt_not_a_real_token");
        if let Some(v) = v {
            r = r.header("X-Askbar-Client-Version", v);
        }
        r.send()
    };
    let r = get(Some("0.2.9")).await.unwrap();
    assert_eq!(r.status(), 426);
    assert_eq!(r.headers()["content-type"], "application/problem+json");
    let p: askbar_proto::Problem = r.json().await.unwrap();
    assert_eq!((p.error.as_str(), p.status), ("client_too_old", 426));
    assert_eq!(p.extra["min_client"], askbar_server::MIN_CLIENT, "the problem says what version is needed");
    let r = get(None).await.unwrap();
    assert_eq!(r.status(), 400);
    let p: askbar_proto::Problem = r.json().await.unwrap();
    assert_eq!(p.error, "client_version_missing");
}
