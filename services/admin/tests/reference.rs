#![cfg(unix)]
//! The reference pages in the admin (plan step 7.2): every page is served in the admin's frame with the same CSP as the
//! other pages, an unknown path is the admin's 404, the menu marks Reference as current, and "Edit on GitHub" points at
//! the source.
use askbar_admin::constants::{SOURCE_BRANCH, SOURCE_URL};
use askbar_admin::{reference, AdminDb, AdminState};

async fn start() -> String {
    let state = AdminState::new(AdminDb::open_in_memory().unwrap());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let app = askbar_admin::router(state);
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    url
}

#[tokio::test]
async fn every_reference_page_is_served_in_the_frame() {
    let url = start().await;
    let other_csp = reqwest::get(format!("{url}/admin/home/")).await.unwrap().headers()["content-security-policy"].clone();
    for p in reference::pages() {
        let res = reqwest::get(format!("{url}{}", p.path())).await.unwrap();
        assert_eq!(res.status(), 200, "{}", p.path());
        assert_eq!(res.headers()["content-security-policy"], other_csp, "{}: the CSP is the one every page has", p.path());
        assert!(res.headers()["content-type"].to_str().unwrap().starts_with("text/html"));
        let html = res.text().await.unwrap();
        // The menu's Reference item is the current one, and the only current item in the menu.
        let nav = &html[html.find(r#"<nav id="nav""#).unwrap()..];
        let nav = &nav[..nav.find("</nav>").unwrap()];
        assert_eq!(nav.matches(r#"aria-current="page""#).count(), 1, "{}", p.path());
        assert!(nav.contains(r#"data-tab="reference" data-tip="Reference" aria-current="page""#), "{}: {nav}", p.path());
        assert!(html.contains(&format!(r#"<a class="ref-edit" href="{SOURCE_URL}/edit/{SOURCE_BRANCH}/{}""#, p.source)), "{}", p.path());
        assert!(!html.contains(" style=") && !html.contains("<script>"), "{}: no inline style or script", p.path());
        assert!(html.contains(&format!("<title>{} ", p.title.replace('&', "&#38;"))) || p.is_index(), "{}", p.path());
    }
}

#[tokio::test]
async fn an_unknown_reference_path_is_the_admin_404() {
    let url = start().await;
    for path in [
        "/admin/reference/nope/",
        "/admin/reference/settings/",
        "/admin/reference/settings/network",
        "/admin/reference/settings/network.md",
        "/admin/reference/index/",
        "/admin/reference/settings/nope/",
        "/admin/reference/../home/x/",
    ] {
        let res = reqwest::get(format!("{url}{path}")).await.unwrap();
        assert_eq!(res.status(), 404, "{path}");
    }
    let known = reqwest::get(format!("{url}/admin/reference/settings/network/")).await.unwrap();
    assert_eq!(known.status(), 200);
}

#[tokio::test]
async fn the_index_is_served_as_json_for_the_search() {
    let url = start().await;
    let res = reqwest::get(format!("{url}/admin/reference.json")).await.unwrap();
    assert_eq!(res.status(), 200);
    let v: serde_json::Value = res.json().await.unwrap();
    let net = v.as_array().unwrap().iter().find(|p| p["id"] == "settings/network").expect("the network page");
    assert!(net["headings"].as_array().unwrap().iter().any(|h| h["id"] == "host-name"));
    assert_eq!(net["source"], "docs/reference/settings/network.md");
}
