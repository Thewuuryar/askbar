//! The route tables the server records while it builds its routers are the routers: every entry
//! answers (a 401 or a 400 counts, a 404 or 405 does not), and an unregistered method is a 405.
use askbar_server::routes::{client_routes, server_routes, RouteEntry};
use askbar_server::{secrets::Secrets, AppState};
use std::collections::BTreeSet;

fn concrete(path: &str) -> String {
    let mut out = String::new();
    let mut in_param = false;
    for c in path.chars() {
        match c {
            '{' => {
                in_param = true;
                out.push('x');
            }
            '}' => in_param = false,
            _ if !in_param => out.push(c),
            _ => {}
        }
    }
    out
}

async fn serve(app: axum::Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    url
}

fn state() -> std::sync::Arc<AppState> {
    let db = askbar_server::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), askbar_server::MIN_CLIENT)
}

fn assert_unique_under(table: &[RouteEntry], prefix: &str, also: &[&str]) {
    let mut seen = BTreeSet::new();
    for r in table {
        assert!(seen.insert((&r.method, &r.path)), "registered twice: {r:?}");
        assert!(r.path.starts_with(&format!("{prefix}/")) || also.iter().any(|a| r.path.starts_with(a)), "{r:?} is outside {prefix}");
    }
}

#[test]
fn the_tables_list_each_api_under_its_own_prefix_and_nothing_else() {
    let client = client_routes();
    assert_unique_under(&client, "/api/client/v1", &["/api/v1/"]);
    for (m, p) in [("POST", "/api/client/v1/chat"), ("POST", "/api/client/v1/pairings"), ("GET", "/api/client/v1/me"), ("GET", "/api/client/v1/health"), ("ANY", "/api/v1/{*rest}"), ("POST", "/api/client/v1/generations/{id}/cancel")] {
        assert!(client.iter().any(|r| r.method == m && r.path == p), "missing {m} {p}");
    }
    let server = server_routes();
    assert_unique_under(&server, "/api/server/v1", &[]);
    for (m, p) in [("PUT", "/api/server/v1/config"), ("GET", "/api/server/v1/clients"), ("GET", "/api/server/v1/audit/stats")] {
        assert!(server.iter().any(|r| r.method == m && r.path == p), "missing {m} {p}");
    }
    assert!(client.iter().all(|r| !r.path.starts_with("/api/server")) && server.iter().all(|r| !r.path.starts_with("/api/client")), "the two APIs share no prefix");
}

#[tokio::test]
async fn every_client_route_answers_and_a_method_that_is_not_registered_is_a_405() {
    let url = serve(askbar_server::client_router(state())).await;
    let http = reqwest::Client::new();
    let table = client_routes();
    for r in table.iter().filter(|r| r.method != "ANY") {
        let method = reqwest::Method::from_bytes(r.method.as_bytes()).unwrap();
        let res = http.request(method, format!("{url}{}", concrete(&r.path))).header("x-askbar-client-version", "0.5.0").send().await.unwrap();
        let s = res.status().as_u16();
        assert!(s != 404 && s != 405, "{} {} answered {s}", r.method, r.path);
    }
    for r in table.iter().filter(|r| r.method == "ANY") {
        let res = http.patch(format!("{url}{}", concrete(&r.path))).send().await.unwrap();
        assert_eq!(res.status(), 426, "any method on {}", r.path);
    }
    let paths: BTreeSet<&str> = table.iter().filter(|r| r.method != "ANY").map(|r| r.path.as_str()).collect();
    for p in paths {
        let res = http.patch(format!("{url}{}", concrete(p))).header("x-askbar-client-version", "0.5.0").send().await.unwrap();
        assert!(res.status() == 405 || res.status() == 401, "PATCH {p} answered {}", res.status());
    }
}

#[tokio::test]
async fn every_server_route_answers_behind_its_token_check() {
    let dir = std::env::temp_dir().join(format!("askbar-routes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("token.sha256"), "0".repeat(64)).unwrap();
    let url = serve(askbar_server::server_api::router(state(), dir.join("token.sha256"))).await;
    let http = reqwest::Client::new();
    for r in server_routes() {
        let method = reqwest::Method::from_bytes(r.method.as_bytes()).unwrap();
        let res = http.request(method, format!("{url}{}", concrete(&r.path))).send().await.unwrap();
        // The token check comes first, so a route that exists says 401.
        assert_eq!(res.status(), 401, "{} {}", r.method, r.path);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
