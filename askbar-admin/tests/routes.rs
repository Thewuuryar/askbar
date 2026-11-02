#![cfg(unix)]
//! The route table the admin records while it builds its router is the router: every entry answers
//! (a 401 or a 400 counts, a 404 or 405 does not), and nothing answers that is not in the table.
use askbar_admin::{routes, AdminDb, AdminState};

async fn start() -> String {
    let state = AdminState::new(AdminDb::open_in_memory().unwrap());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let app = askbar_admin::router(state);
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    url
}

fn concrete(path: &str) -> String {
    // {id} and {span_id} become a plain word.
    let mut out = String::new();
    let mut in_param = false;
    for c in path.chars() {
        match c {
            '{' => {
                in_param = true;
                out.push_str("x");
            }
            '}' => in_param = false,
            _ if !in_param => out.push(c),
            _ => {}
        }
    }
    out
}

#[test]
fn the_table_lists_the_api_under_v1_without_duplicates() {
    let table = routes::route_table();
    let mut seen = std::collections::BTreeSet::new();
    for r in &table {
        assert!(seen.insert((r.method.clone(), r.path.clone())), "registered twice: {r:?}");
    }
    let api = routes::api_route_table();
    assert!(api.len() >= 25, "{}", api.len());
    assert!(api.iter().all(|r| r.path.starts_with("/admin/api/v1/")), "{api:?}");
    for (m, p) in [
        ("GET", "/admin/api/v1/providers"),
        ("POST", "/admin/api/v1/providers"),
        ("PUT", "/admin/api/v1/password"),
        ("POST", "/admin/api/v1/session"),
        ("GET", "/admin/api/v1/session"),
        ("DELETE", "/admin/api/v1/session"),
        ("GET", "/admin/api/v1/health"),
        ("POST", "/admin/api/v1/pairings"),
        ("GET", "/admin/api/v1/clients"),
    ] {
        assert!(api.iter().any(|r| r.method == m && r.path == p), "missing {m} {p}");
    }
    // Nothing from before the move is left.
    assert!(table.iter().all(|r| !r.path.starts_with("/admin/api/devices") && r.path != "/admin/login" && r.path != "/health" && r.path != "/"), "{table:?}");
}

#[tokio::test]
async fn every_registered_route_answers_and_a_method_that_is_not_registered_is_a_405() {
    let url = start().await;
    let http = reqwest::Client::new();
    let table = routes::route_table();
    for r in &table {
        let method = reqwest::Method::from_bytes(r.method.as_bytes()).unwrap();
        let res = http.request(method, format!("{url}{}", concrete(&r.path))).header("x-askbar-admin", "1").send().await.unwrap();
        let s = res.status().as_u16();
        assert!(s != 404 && s != 405, "{} {} answered {s}", r.method, r.path);
    }
    let paths: std::collections::BTreeSet<&str> = table.iter().map(|r| r.path.as_str()).collect();
    for p in paths {
        let res = http.patch(format!("{url}{}", concrete(p))).header("x-askbar-admin", "1").send().await.unwrap();
        assert_eq!(res.status(), 405, "PATCH {p} is not registered");
    }
    let res = http.get(format!("{url}/admin/api/v1/no-such-thing")).send().await.unwrap();
    assert_eq!(res.status(), 404);
}
