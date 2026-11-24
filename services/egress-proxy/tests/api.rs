//! The admin API: the token on every route but `/health`, the config round trip, the refusals, the restart.

use askbar_egress_proxy::{admin_links, admin_router, system_connector, system_resolver, Proxy};
use askbar_proto::egress::EgressStatus;
use askbar_service::restart::Restarter;
use askbar_service::status::BootId;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TOKEN: &str = "admegr_test-token";
const V1: &str = "/api/egress-proxy/v1";

struct Api {
    addr: std::net::SocketAddr,
    proxy: Arc<Proxy>,
    exited: Arc<AtomicBool>,
}

async fn api() -> Api {
    let dir = std::env::temp_dir().join(format!("askbar-egress-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(dir.join("accept")).unwrap();
    let hash: String = Sha256::digest(TOKEN.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(dir.join("accept/admin-egress-proxy.sha256"), hash).unwrap();
    let (boot, exited) = (BootId::new(), Arc::new(AtomicBool::new(false)));
    let e = exited.clone();
    let restarter =
        Restarter::for_tests(boot.clone(), Duration::ZERO, Duration::from_millis(50), || {}, move || e.store(true, Ordering::SeqCst));
    let proxy = Proxy::new(vec!["a".into(), "b".into()], system_resolver(), system_connector(), boot, restarter);
    let app = admin_router(proxy.clone(), admin_links(&dir));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    Api { addr, proxy, exited }
}

/// One request, `Connection: close`: the status and the body.
async fn call(a: &Api, method: &str, path: &str, token: Option<&str>, body: &str) -> (u16, String) {
    let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    let req = format!("{method} {V1}{path} HTTP/1.1\r\nHost: x\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let mut s = TcpStream::connect(a.addr).await.unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap();
    (head.split(' ').nth(1).unwrap().parse().unwrap(), body.to_string())
}

#[tokio::test]
async fn the_token_is_required_on_every_route_but_health() {
    let a = api().await;
    assert_eq!(call(&a, "GET", "/health", None, "").await.0, 200);
    for (m, p) in [("GET", "/status"), ("PUT", "/config"), ("POST", "/restart")] {
        assert_eq!(call(&a, m, p, None, "{}").await.0, 401, "{m} {p} without a token");
        assert_eq!(call(&a, m, p, Some("wrong"), "{}").await.0, 401, "{m} {p} with a wrong token");
    }
    assert!(!a.exited.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_pushed_config_is_in_force_and_the_status_says_so() {
    let a = api().await;
    let (code, body) = call(&a, "GET", "/status", Some(TOKEN), "").await;
    let s: EgressStatus = serde_json::from_str(&body).unwrap();
    assert_eq!((code, s.configured, s.version), (200, false, None));
    assert_eq!(s.sources.keys().collect::<Vec<_>>(), ["a", "b"]);
    let cfg = r#"{"sources":{"a":{"entries":[{"host":"api.example.com"}]}}}"#;
    let (code, body) = call(&a, "PUT", "/config", Some(TOKEN), cfg).await;
    assert_eq!(code, 200, "{body}");
    let version = serde_json::from_str::<serde_json::Value>(&body).unwrap()["version"].as_u64().unwrap();
    // The same config is the same version; another is not.
    let again = call(&a, "PUT", "/config", Some(TOKEN), cfg).await.1;
    assert_eq!(serde_json::from_str::<serde_json::Value>(&again).unwrap()["version"], version);
    let other = call(&a, "PUT", "/config", Some(TOKEN), r#"{"sources":{"b":{"entries":[]}}}"#).await.1;
    assert_ne!(serde_json::from_str::<serde_json::Value>(&other).unwrap()["version"], version);
    let s: EgressStatus = serde_json::from_str(&call(&a, "GET", "/status", Some(TOKEN), "").await.1).unwrap();
    assert!(s.configured && s.version.is_some() && !s.boot_id.is_empty());
    assert!(a.proxy.status().configured);
}

#[tokio::test]
async fn a_refused_config_changes_nothing_and_echoes_nothing() {
    let a = api().await;
    let bad = [
        r#"{"sources":{"nope":{"entries":[]}}}"#,
        r#"{"sources":{"a":{"entries":[{"host":"Bad Host!"}]}}}"#,
        r#"{"limits":{"head_bytes":1}}"#,
        r#"{"limits":{"max_tunnels":2,"max_tunnels_per_source":3}}"#,
        r#"{"unknown_field":1}"#,
        r#"not json"#,
    ];
    for b in bad {
        let (code, body) = call(&a, "PUT", "/config", Some(TOKEN), b).await;
        assert_eq!(code, 400, "{b}");
        assert!(body.contains("bad_request") && !body.contains("nope") && !body.contains("Bad Host"), "{body}");
    }
    assert!(!a.proxy.status().configured);
}

#[tokio::test]
async fn restart_stops_the_process() {
    let a = api().await;
    assert_eq!(call(&a, "POST", "/restart", Some(TOKEN), "").await.0, 202);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(a.exited.load(Ordering::SeqCst));
}
