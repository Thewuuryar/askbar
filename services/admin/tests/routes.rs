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
                out.push('x');
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
    assert!(api.iter().all(|r| r.path.starts_with("/api/admin/v1/")), "{api:?}");
    for (m, p) in [
        ("GET", "/api/admin/v1/providers"),
        ("POST", "/api/admin/v1/providers"),
        ("PUT", "/api/admin/v1/password"),
        ("POST", "/api/admin/v1/session"),
        ("GET", "/api/admin/v1/session"),
        ("DELETE", "/api/admin/v1/session"),
        ("GET", "/api/admin/v1/health"),
        ("GET", "/api/admin/v1/users"),
        ("POST", "/api/admin/v1/users"),
        ("POST", "/api/admin/v1/users/{id}/pairing-keys"),
        ("GET", "/api/admin/v1/clients"),
    ] {
        assert!(api.iter().any(|r| r.method == m && r.path == p), "missing {m} {p}");
    }
    // Nothing from before the move is left.
    assert!(
        table.iter().all(|r| !r.path.starts_with("/admin/api/devices") && r.path != "/admin/login" && r.path != "/health" && r.path != "/"),
        "{table:?}"
    ); // names-ok: proves the old path is gone
}

#[test]
fn every_admin_page_has_a_route_and_a_nav_link() {
    // Plan step 1.16: `PAGES` is the one list the routes and the nav come from.
    let table = routes::route_table();
    for p in askbar_admin::pages::PAGES {
        let path = askbar_admin::pages::path(p);
        assert!(table.iter().any(|r| r.method == "GET" && r.path == path), "no route for {path}");
    }
    // The redirects are not pages: /admin/ (to the landing page) and /admin/audit/ (to Model calls, plan step 2.1).
    const REDIRECTS: [&str; 2] = ["/admin/", "/admin/audit/"];
    let pages =
        table.iter().filter(|r| r.path.starts_with("/admin/") && r.path.ends_with('/') && !REDIRECTS.contains(&r.path.as_str())).count();
    assert_eq!(pages, askbar_admin::pages::PAGES.len(), "every page route comes from PAGES");
}

#[tokio::test]
async fn the_old_audit_address_opens_model_calls_and_keeps_its_query() {
    let url = start().await;
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    // The landing page (plan step 4.1): /admin and /admin/ open Home.
    for (from, to) in [("/admin", "/admin/home/"), ("/admin/", "/admin/home/")] {
        let res = http.get(format!("{url}{from}")).send().await.unwrap();
        assert!(res.status().is_redirection(), "{from}: {}", res.status());
        assert_eq!(res.headers()["location"], to, "{from}");
    }
    for (from, to) in
        [("/admin/audit/", "/admin/audit/calls/"), ("/admin/audit/?view=trace&id=abc", "/admin/audit/calls/?view=trace&id=abc")]
    {
        let res = http.get(format!("{url}{from}")).send().await.unwrap();
        assert!(res.status().is_redirection(), "{from}: {}", res.status());
        assert_eq!(res.headers()["location"], to, "{from}");
    }
    for slug in ["home", "audit/calls", "audit/admin", "audit/sealed", "audit/routing", "reference"] {
        assert_eq!(http.get(format!("{url}/admin/{slug}/")).send().await.unwrap().status(), 200, "{slug}");
    }
}

#[tokio::test]
async fn an_unknown_admin_page_is_a_404() {
    let url = start().await;
    for path in ["/admin/x/", "/admin/users/", "/admin/providers/x/"] {
        assert_eq!(reqwest::get(format!("{url}{path}")).await.unwrap().status(), 404, "{path}");
    }
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
    let res = http.get(format!("{url}/api/admin/v1/no-such-thing")).send().await.unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn constants_json_is_public_and_cached_by_etag() {
    let url = start().await;
    let http = reqwest::Client::new();
    let res = http.get(format!("{url}/admin/constants.json")).send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert!(res.headers()["content-type"].to_str().unwrap().starts_with("application/json"));
    let etag = res.headers()["etag"].to_str().unwrap().to_string();
    let v: serde_json::Value = res.json().await.unwrap();
    assert_eq!(v["MIN_PASSPHRASE_CHARS"], 20);
    let again = http.get(format!("{url}/admin/constants.json")).header("if-none-match", &etag).send().await.unwrap();
    assert_eq!(again.status(), 304);
}
