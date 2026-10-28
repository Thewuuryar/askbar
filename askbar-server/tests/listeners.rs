use std::net::SocketAddr;

fn state() -> std::sync::Arc<askbar_server::AppState> {
    let db = askbar_core::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    askbar_server::AppState::new(db, askbar_server::secrets::Secrets::new("0123456789abcdef0123456789abcdef").unwrap())
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
async fn routers_are_separate() {
    let api = spawn(askbar_server::api_router(state())).await;
    let admin = spawn(askbar_server::admin_router(state())).await;
    assert_eq!(status(api, "/api/v1/health").await, 200);
    assert_eq!(status(admin, "/admin/health").await, 200);
    assert_eq!(status(api, "/admin/health").await, 404);
    assert_eq!(status(admin, "/api/v1/health").await, 404);
}
