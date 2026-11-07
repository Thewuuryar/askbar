use std::net::SocketAddr;

fn state() -> std::sync::Arc<askbar_server::AppState> {
    let db = askbar_server::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    askbar_server::AppState::new(db, askbar_server::secrets::Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), askbar_server::MIN_CLIENT)
}

async fn spawn(app: axum::Router) -> SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr
}

async fn status(addr: SocketAddr, path: &str) -> u16 {
    reqwest::get(format!("http://{addr}{path}")).await.unwrap().status().as_u16()
}

#[tokio::test]
async fn the_client_and_server_api_routers_share_no_routes() {
    let dir = std::env::temp_dir().join(format!("askbar-listeners-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let hash = dir.join("token.sha256");
    std::fs::write(&hash, "0".repeat(64)).unwrap();
    let server_api = spawn(askbar_server::server_api::router(state(), hash)).await;
    let api = spawn(askbar_server::client_router(state())).await;

    assert_eq!(status(api, "/api/client/v1/health").await, 200);
    // The server API answers 401 (no token) to everything, including paths that are not its own;
    // the client API answers 404 to server API paths and to the admin site that used to live here.
    assert_eq!(status(server_api, "/api/server/v1/health").await, 401);
    assert_eq!(status(server_api, "/api/client/v1/health").await, 401, "the client API is not served here");
    assert_eq!(status(api, "/api/server/v1/health").await, 404);
    for gone in ["/admin/", "/admin/health", "/api/admin/v1/session", "/api/admin/v1/providers", "/"] {
        assert_eq!(status(api, gone).await, 404, "{gone}: the admin site is not part of the server");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn even_with_the_server_token_the_server_api_listener_serves_no_client_api_and_the_old_path_is_gone() {
    use sha2::{Digest, Sha256};
    let dir = std::env::temp_dir().join(format!("askbar-listeners2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let token = "mgt_listener-test-token";
    let hash: String = Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(dir.join("token.sha256"), hash).unwrap();
    let server_api = spawn(askbar_server::server_api::router(state(), dir.join("token.sha256"))).await;
    let get = |path: &'static str| async move {
        reqwest::Client::new().get(format!("http://{server_api}{path}")).bearer_auth(token).send().await.unwrap().status().as_u16()
    };
    assert_eq!(get("/api/server/v1/health").await, 200);
    for not_here in ["/api/client/v1/health", "/api/client/v1/conversations", "/api/v1/health", "/api/v1/conversations", "/mgmt/v1/health", "/admin/", "/"] { // names-ok: the old path is the point
        assert_eq!(get(not_here).await, 404, "{not_here}: the server API listener serves only /api/server/v1");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn every_client_api_answer_says_nosniff() {
    // ADR 0029 rule 4: JSON or the event stream, never content a browser may guess is HTML or script.
    let api = spawn(askbar_server::client_router(state())).await;
    let http = reqwest::Client::new();
    for (path, version, token) in [
        ("/api/client/v1/health", false, false),             // 200
        ("/api/client/v1/conversations", true, false),       // 401, no token
        ("/api/client/v1/conversations", false, true),       // 400, no version header
        ("/api/client/v1/nothing-here", false, false),       // 404
        ("/api/v1/chat", false, false),                      // 426, the old prefix
    ] {
        let mut r = http.get(format!("http://{api}{path}"));
        if version {
            r = r.header("x-askbar-client-version", askbar_server::MIN_CLIENT);
        }
        if token {
            r = r.bearer_auth("abt_not-a-real-token");
        }
        let r = r.send().await.unwrap();
        assert_eq!(r.headers().get("x-content-type-options").map(|v| v.to_str().unwrap()), Some("nosniff"), "{path} ({})", r.status());
    }
}
