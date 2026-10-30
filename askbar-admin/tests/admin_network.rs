mod common;

use common::*;
use reqwest::Method as M;
use serde_json::{json, Value};

fn full() -> Value {
    json!({ "tls_enabled": true, "hostname": "Askbar.Orraitu.com", "admin_cidr": "192.168.1.0/24, 10.0.0.5", "acme_staging": true, "allowlist_enabled": true, "cloudflare_token": TOKEN })
}

#[tokio::test]
async fn saving_normalises_and_encrypts_the_token() {
    let t = start(true).await;
    let (st, v) = t.call(M::GET, "/admin/api/network", json!({})).await;
    assert_eq!(st, 200);
    assert_eq!((v["tls_enabled"].as_bool(), v["has_token"].as_bool(), v["allowlist_enabled"].as_bool()), (Some(false), Some(false), Some(false)), "everything starts off");
    assert_eq!(v["public_url"], "http://localhost:8080");

    let (st, v) = t.save(full()).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["hostname"], "askbar.orraitu.com", "lower-cased");
    assert_eq!(v["admin_cidr"], "192.168.1.0/24 10.0.0.5/32", "normalised");
    assert_eq!((v["tls_enabled"].as_bool(), v["acme_staging"].as_bool(), v["has_token"].as_bool(), v["allowlist_enabled"].as_bool()), (Some(true), Some(true), Some(true), Some(true)));
    assert_eq!(v["public_url"], "https://askbar.orraitu.com");
    assert!(!v.to_string().contains(TOKEN) && !v.to_string().contains("cloudflare_token"), "write-only");

    // At rest the token is ciphertext that only the admin's own key opens.
    let stored = t.stored_token().await.unwrap();
    assert!(!stored.windows(TOKEN.len()).any(|w| w == TOKEN.as_bytes()));
    assert_eq!(t.admin.secrets.decrypt(&stored).as_deref(), Some(TOKEN));
    let (_, again) = t.call(M::GET, "/admin/api/network", json!({})).await;
    assert!(!again.to_string().contains(TOKEN));
}

#[tokio::test]
async fn leaving_the_token_blank_keeps_it_and_clear_removes_it() {
    let t = start(true).await;
    t.save(full()).await;
    let before = t.stored_token().await.unwrap();
    let mut keep = full();
    keep.as_object_mut().unwrap().remove("cloudflare_token");
    keep["acme_staging"] = json!(false);
    let (st, v) = t.save(keep.clone()).await;
    assert_eq!((st, v["has_token"].as_bool(), v["acme_staging"].as_bool()), (200, Some(true), Some(false)));
    assert_eq!(t.stored_token().await.unwrap(), before);
    keep["cloudflare_token"] = json!("");
    assert_eq!(t.save(keep.clone()).await.0, 200);
    assert_eq!(t.stored_token().await.unwrap(), before, "empty string also keeps it");

    // Removing it while HTTPS is on is refused; with HTTPS off it works.
    keep["clear_token"] = json!(true);
    let (st, e) = t.save(keep.clone()).await;
    assert_eq!(st, 400);
    assert!(e["error"].as_str().unwrap().contains("token"));
    assert_eq!(t.stored_token().await.unwrap(), before);
    keep["tls_enabled"] = json!(false);
    let (st, v) = t.save(keep).await;
    assert_eq!((st, v["has_token"].as_bool()), (200, Some(false)));
    assert!(t.stored_token().await.is_none());
}

#[tokio::test]
async fn invalid_input_is_refused_and_changes_nothing() {
    let t = start(true).await;
    for (label, patch) in [
        ("url as hostname", json!({ "hostname": "https://askbar.orraitu.com" })),
        ("port in hostname", json!({ "hostname": "askbar.orraitu.com:8443" })),
        ("no dot", json!({ "hostname": "localhost" })),
        ("bad cidr", json!({ "admin_cidr": "192.168.1.0/99" })),
        ("junk cidr", json!({ "admin_cidr": "everyone" })),
        ("token with space", json!({ "cloudflare_token": "abc def abc def abc def abc def" })),
        ("token with newline", json!({ "cloudflare_token": "abcdefghijklmnopqrstuvwxyz\nimport evil" })),
        ("token with brace", json!({ "cloudflare_token": "abcdefghijklmnopqrstuvwxyz}{" })),
        ("short token", json!({ "cloudflare_token": "abc" })),
        ("tls without hostname", json!({ "hostname": "" })),
        ("allowlist on with nothing in it", json!({ "admin_cidr": "" })),
    ] {
        let mut body = full();
        for (k, v) in patch.as_object().unwrap() {
            body[k] = v.clone();
        }
        let (st, e) = t.save(body).await;
        assert_eq!(st, 400, "{label}");
        assert!(e["error"].is_string(), "{label}");
    }
    let (_, v) = t.call(M::GET, "/admin/api/network", json!({})).await;
    assert_eq!((v["tls_enabled"].as_bool(), v["hostname"].as_str(), v["has_token"].as_bool()), (Some(false), Some(""), Some(false)));
    assert!(t.stored_token().await.is_none());
}

#[tokio::test]
async fn http_mode_saves_without_a_token_and_pairing_follows_the_setting() {
    let t = start(true).await;
    let (st, _) = t.save(json!({ "tls_enabled": false, "hostname": "", "admin_cidr": "", "acme_staging": false })).await;
    assert_eq!(st, 200);
    let pairing = |t: &T| {
        let url = format!("{}/admin/api/pair", t.adm);
        let (http, cookie) = (t.http.clone(), t.cookie.clone());
        async move {
            let v: Value = http.post(url).header("x-askbar-admin", "1").header("cookie", cookie).json(&json!({})).send().await.unwrap().json().await.unwrap();
            askbar_proto::decode_pairing(v["pairing"].as_str().unwrap()).unwrap().0
        }
    };
    assert_eq!(pairing(&t).await, "http://localhost:8080");
    assert_eq!(t.save(full()).await.0, 200);
    assert_eq!(pairing(&t).await, "https://askbar.orraitu.com");
}

#[tokio::test]
async fn the_backup_leaves_out_the_token_but_keeps_the_rest() {
    let t = start(true).await;
    t.save(full()).await;
    let r = t.http.get(format!("{}/admin/api/backup", t.adm)).header("cookie", &t.cookie).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let bytes = r.bytes().await.unwrap();
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    let stored = t.stored_token().await.unwrap();
    assert!(!has(&stored) && !has(TOKEN.as_bytes()), "neither the ciphertext nor the token is in the backup");
    assert!(has(b"askbar.orraitu.com"), "non-secret network settings are in the backup");
}

#[tokio::test]
async fn network_needs_a_session() {
    let t = start(true).await;
    for (m, p) in [(M::GET, "/admin/api/network"), (M::PUT, "/admin/api/network"), (M::POST, "/admin/api/network/apply")] {
        let r = t.http.request(m, format!("{}{p}", t.adm)).header("x-askbar-admin", "1").json(&json!({})).send().await.unwrap();
        assert_eq!(r.status(), 401, "{p}");
    }
}
