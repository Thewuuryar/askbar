//! The setup the auditor's integration tests share: a token per link, an in-process auditor on a temporary directory,
//! and the handle that removes the directory. Each test file's `start` says what it differs in (its limits, its links,
//! whether the alert rules run).

#![allow(dead_code)] // each test file uses a different part of this

use askbar_audit::{AuditDb, Recorder};
use askbar_auditor::rules::Alerts;
use askbar_auditor::{disk::FreeSpace, Auditor, Limits};
use askbar_proto::alerter::{Event, Fields, Kind, Severity};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// What the rules sent, in order.
#[derive(Default)]
pub struct Collector(pub Mutex<Vec<Event>>);

impl Alerts for Collector {
    fn send(&self, kind: Kind, severity: Severity, fields: Fields) {
        self.0.lock().unwrap().push(Event { kind, severity, at: 0, fields });
    }
}

pub struct T {
    pub url: String,
    pub dir: PathBuf,
    pub auditor: Arc<Auditor>,
    pub http: reqwest::Client,
    /// What the volume has free, by the byte.
    pub free: Arc<AtomicU64>,
    /// What the alert rules sent. Stays empty unless the auditor was started with alerts.
    pub sent: Arc<Collector>,
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn token(link: &str) -> String {
    format!("tok_{link}_secret")
}

pub fn hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// An auditor on `dir` that accepts `links` (their tokens are written under `dir/tokens/accept`), with `limits`, and
/// with the alert rules over `sent` when `alerts` is set. The volume has 100 GiB free until a test says otherwise.
pub async fn start_on(dir: PathBuf, limits: Limits, links: &[&str], alerts: bool) -> T {
    let accept = dir.join("tokens/accept");
    std::fs::create_dir_all(&accept).unwrap();
    for link in links {
        std::fs::write(accept.join(format!("{link}.sha256")), format!("{}\n", hex(&token(link)))).unwrap();
    }
    let recorder = Recorder::spawn(AuditDb::open(&dir.join("data/audit.db")).unwrap(), limits.max_queued);
    let free = Arc::new(AtomicU64::new(100 * 1024 * 1024 * 1024));
    let f = free.clone();
    let free_space: FreeSpace = Arc::new(move || Some(f.load(Ordering::Relaxed)));
    let sent = Arc::new(Collector::default());
    let auditor = Auditor::new(recorder, limits, free_space);
    let auditor = if alerts { auditor.with_alerts(sent.clone()) } else { auditor };
    let app = askbar_auditor::router(auditor.clone(), askbar_auditor::links(&dir.join("tokens")));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    T { url, dir, auditor, http: reqwest::Client::new(), free, sent }
}
