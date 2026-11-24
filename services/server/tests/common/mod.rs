//! What the server's integration tests share: the mock LLM, serving a router on a free port, the
//! test client and `hex`. Each test file uses a different subset, hence the allow.
#![allow(dead_code)]

use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// `tests/e2e/mock_llm.py` on a free port, killed when dropped. `log` holds the lines it prints, one per request.
pub struct MockLlm {
    child: Child,
    pub port: u16,
    pub log: Arc<Mutex<Vec<String>>>,
}

impl MockLlm {
    pub fn start() -> MockLlm {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/e2e/mock_llm.py");
        let mut child =
            Command::new("python3").args(["-u", script, &port.to_string()]).stdout(Stdio::piped()).spawn().expect("python3 is needed");
        let log = Arc::new(Mutex::new(Vec::new()));
        let out = BufReader::new(child.stdout.take().unwrap());
        let l = log.clone();
        std::thread::spawn(move || {
            for line in out.lines().map_while(Result::ok) {
                l.lock().unwrap().push(line);
            }
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(Instant::now() < deadline, "mock did not start");
            std::thread::sleep(Duration::from_millis(50));
        }
        MockLlm { child, port, log }
    }

    /// How many chat requests it has seen.
    pub fn requests(&self) -> usize {
        self.log.lock().unwrap().iter().filter(|l| l.starts_with("POST")).count()
    }
}

impl Drop for MockLlm {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Serves `app` on a free local port in the background, and returns its address.
pub async fn serve(app: axum::Router) -> SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr
}

pub fn hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// A client that sends the client version header.
pub fn client_builder() -> reqwest::ClientBuilder {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert(askbar_proto::CLIENT_VERSION_HEADER, "0.6.0".parse().unwrap());
    reqwest::Client::builder().default_headers(h)
}

pub fn client() -> reqwest::Client {
    client_builder().build().unwrap()
}
