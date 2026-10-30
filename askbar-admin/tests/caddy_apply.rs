#![cfg(unix)]
mod common;

use common::*;
use askbar_admin::caddy::{CaddyLink, STAGING_CA};
use reqwest::Method as M;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;

fn settings(tls: bool) -> Value {
    json!({ "tls_enabled": tls, "hostname": "askbar.orraitu.com", "admin_cidr": "192.168.1.0/24", "acme_staging": true, "allowlist_enabled": true, "cloudflare_token": TOKEN })
}

#[tokio::test]
async fn saving_pushes_the_config_through_the_socket_and_writes_the_token_file() {
    let t = start(true).await;
    let (st, v) = t.save(settings(true)).await;
    assert_eq!(st, 200);
    assert_eq!(v["proxy"]["ok"], true, "{v}");

    let loads = t.fake.loads.lock().unwrap().clone();
    assert_eq!(loads.len(), 1);
    assert_eq!(loads[0].0, "text/caddyfile");
    let body = &loads[0].1;
    assert!(body.contains("\naskbar.orraitu.com {") && body.contains("\nadmin.askbar.orraitu.com {"), "{body}");
    assert!(body.contains("remote_ip 192.168.1.0/24") && body.contains(STAGING_CA));
    assert!(body.contains("admin unix//run/caddy/admin.sock") && !body.contains("origins"));
    assert!(!body.contains(TOKEN), "the token never goes into the config");
    assert_eq!(t.token_file().as_deref(), Some(TOKEN));
    assert_eq!(std::fs::metadata(t.secrets_dir.join("cf_token")).unwrap().permissions().mode() & 0o777, 0o600);
    // The status is remembered and served back.
    assert_eq!(t.call(M::GET, "/admin/api/network", json!({})).await.1["proxy"]["ok"], true);
}

#[tokio::test]
async fn the_remote_ip_matcher_is_in_the_config_only_when_the_allowlist_is_on() {
    let t = start(true).await;
    t.save(settings(true)).await;
    let on = t.fake.loads.lock().unwrap().last().unwrap().1.clone();
    assert!(on.contains("@outside not remote_ip 192.168.1.0/24") && on.contains("abort @outside"), "{on}");

    // Allowlist off: the addresses stay saved but no matcher is sent, so any device reaches the login page.
    let mut off = settings(true);
    off["allowlist_enabled"] = json!(false);
    off.as_object_mut().unwrap().remove("cloudflare_token");
    let (_, v) = t.save(off).await;
    assert_eq!((v["proxy"]["ok"].as_bool(), v["admin_cidr"].as_str()), (Some(true), Some("192.168.1.0/24")));
    let off_body = t.fake.loads.lock().unwrap().last().unwrap().1.clone();
    assert!(!off_body.contains("remote_ip") && !off_body.contains("abort"), "{off_body}");
    assert!(off_body.contains("admin.askbar.orraitu.com"), "the admin site is still served");

    // And on again, with several single addresses and a range.
    let mut again = settings(true);
    again["admin_cidr"] = json!("10.0.0.5 fd00::1 172.16.0.0/12");
    again.as_object_mut().unwrap().remove("cloudflare_token");
    t.save(again).await;
    let body = t.fake.loads.lock().unwrap().last().unwrap().1.clone();
    assert!(body.contains("remote_ip 10.0.0.5/32 fd00::1/128 172.16.0.0/12"), "{body}");
}

#[tokio::test]
async fn turning_https_off_serves_nothing_and_removes_the_token_file() {
    let t = start(true).await;
    t.save(settings(true)).await;
    assert!(t.token_file().is_some());
    let mut off = settings(false);
    off.as_object_mut().unwrap().remove("cloudflare_token");
    let (_, v) = t.save(off).await;
    assert_eq!(v["proxy"]["ok"], true);
    assert!(t.token_file().is_none(), "no plaintext token lying around while HTTPS is off");
    let last = t.fake.loads.lock().unwrap().last().unwrap().1.clone();
    assert!(!last.contains("askbar.orraitu.com"));
    assert!(last.contains("admin unix//run/caddy/admin.sock"), "Caddy keeps its admin socket");
    // The encrypted copy is still stored, so turning HTTPS back on needs no re-entry.
    assert_eq!(t.call(M::GET, "/admin/api/network", json!({})).await.1["has_token"], true);
}

#[tokio::test]
async fn a_refusal_from_caddy_is_reported_but_the_settings_stay_saved() {
    let t = start(true).await;
    *t.fake.refuse.lock().unwrap() = true;
    let (st, v) = t.save(settings(true)).await;
    assert_eq!(st, 200);
    assert_eq!(v["proxy"]["ok"], false);
    let msg = v["proxy"]["message"].as_str().unwrap();
    assert!(msg.contains("Caddy refused") && msg.contains("unknown directive"), "{msg}");
    assert_eq!(v["hostname"], "askbar.orraitu.com", "saved anyway");

    // Fixing Caddy and pressing Apply works without re-entering anything.
    *t.fake.refuse.lock().unwrap() = false;
    let (st, v) = t.call(M::POST, "/admin/api/network/apply", json!({})).await;
    assert_eq!((st, v["ok"].as_bool()), (200, Some(true)));
}

#[tokio::test]
async fn caddy_being_down_is_a_message_not_an_error() {
    let t = start(false).await;
    let (st, v) = t.save(settings(true)).await;
    assert_eq!(st, 200);
    assert_eq!(v["proxy"]["ok"], false);
    assert!(v["proxy"]["message"].as_str().unwrap().contains("Could not reach Caddy"));
}

#[tokio::test]
async fn without_a_caddy_link_the_status_says_so() {
    let t = start(true).await;
    t.admin.set_caddy(CaddyLink::default());
    let (_, v) = t.call(M::POST, "/admin/api/network/apply", json!({})).await;
    assert_eq!(v["ok"], false);
    assert!(v["message"].as_str().unwrap().contains("not set up"));
    assert!(t.fake.loads.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_token_that_cannot_be_read_is_not_pushed() {
    let t = start(true).await;
    t.save(settings(true)).await;
    t.admin.db.lock().await.conn().execute("UPDATE network_settings SET cloudflare_token = x'00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff'", []).unwrap();
    t.fake.loads.lock().unwrap().clear();
    let (_, v) = t.call(M::POST, "/admin/api/network/apply", json!({})).await;
    assert_eq!(v["ok"], false);
    assert!(v["message"].as_str().unwrap().contains("Cloudflare token"));
    assert!(t.fake.loads.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_ui_is_served_with_its_security_headers() {
    let t = start(true).await;
    for (path, ctype) in [("/admin/", "text/html"), ("/admin/app.js", "text/javascript"), ("/admin/style.css", "text/css")] {
        let r = t.http.get(format!("{}{path}", t.adm)).send().await.unwrap();
        assert_eq!(r.status(), 200, "{path}");
        assert!(r.headers()["content-type"].to_str().unwrap().starts_with(ctype), "{path}");
        assert!(r.headers()["content-security-policy"].to_str().unwrap().contains("frame-ancestors 'none'"));
    }
    let page = t.http.get(format!("{}/admin/", t.adm)).send().await.unwrap().text().await.unwrap();
    assert!(page.contains("id=\"offline\"") && page.contains("id=\"n-allow\""));
    assert!(!page.contains("setup-key") && !page.contains("Rotate") && !page.contains("rotate-"), "no master-key screens here");
    let redirect = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap().get(format!("{}/", t.adm)).send().await.unwrap();
    assert_eq!(redirect.status(), 303);
}
