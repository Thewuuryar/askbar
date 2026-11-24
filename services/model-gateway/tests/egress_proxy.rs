//! Provider calls go through the egress proxy; `cpa` calls do not (plan step P4.3a).

use askbar_model_gateway::llm::client;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A fake proxy: records the first line of each request it gets and answers 403.
async fn fake_proxy() -> (String, Arc<Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    tokio::spawn(async move {
        loop {
            let (mut c, _) = l.accept().await.unwrap();
            let s = s.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let n = c.read(&mut buf).await.unwrap_or(0);
                if let Some(line) = String::from_utf8_lossy(&buf[..n]).lines().next() {
                    s.lock().unwrap().push(line.to_string());
                }
                let _ = c.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            });
        }
    });
    (url, seen)
}

#[tokio::test]
async fn provider_calls_use_the_proxy_and_cpa_calls_do_not() {
    let (url, seen) = fake_proxy().await;
    let c = client(Some(&url));
    // Plain HTTP arrives in absolute form, HTTPS as CONNECT.
    let r = c.get("http://provider.test:8080/v1/models").send().await.unwrap();
    assert_eq!(r.status(), 403);
    assert!(c.get("https://provider.test/v1/models").send().await.is_err());
    // cpa goes direct (the name does not resolve here), so the proxy sees nothing.
    assert!(c.get("http://cpa:8317/v1/models").send().await.is_err());
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen, ["GET http://provider.test:8080/v1/models HTTP/1.1", "CONNECT provider.test:443 HTTP/1.1"]);
}
