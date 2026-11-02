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

    assert_eq!(status(api, "/api/v1/health").await, 200);
    // The server API answers 401 (no token) to everything, including paths that are not its own;
    // the client API answers 404 to server API paths and to the admin site that used to live here.
    assert_eq!(status(server_api, "/mgmt/v1/health").await, 401);
    assert_eq!(status(server_api, "/api/v1/health").await, 401, "the client API is not served here");
    assert_eq!(status(api, "/mgmt/v1/health").await, 404);
    for gone in ["/admin/", "/admin/health", "/admin/login", "/admin/api/providers", "/"] {
        assert_eq!(status(api, gone).await, 404, "{gone}: the admin site is not part of the server");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
