//! How long an acknowledged append takes, measured and printed (ADR 0010 rule 4: a 20 ms 95th percentile target
//! for the acknowledgment, inside the 200 ms budget). **Not a gate.** Plan step 7.2 makes it one, on the
//! reference machine; this test only says what it is today, so the number can be watched from now on. It fails
//! only if an append fails or the acknowledgment is slower than the 1 s a caller waits for it.
//!
//! Run it with `cargo test -p askbar-auditor --test latency -- --nocapture` to see the figures.

use askbar_audit::{AuditDb, Recorder};
use askbar_auditor::{Auditor, Limits};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{Duration, Instant};

const TOKEN: &str = "tok_latency_secret";

async fn start() -> (String, Arc<Auditor>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("askbar-auditor-latency-{}", ulid::Ulid::new()));
    let accept = dir.join("tokens/accept");
    std::fs::create_dir_all(&accept).unwrap();
    let hash: String = Sha256::digest(TOKEN.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(accept.join("model-gateway-auditor.sha256"), hash).unwrap();
    let limits = Limits::default();
    let recorder = Recorder::spawn(AuditDb::open(&dir.join("data/audit.db")).unwrap(), limits.max_queued);
    let auditor = Auditor::new(recorder, limits, Arc::new(|| Some(100 * 1024 * 1024 * 1024)));
    let app = askbar_auditor::router(auditor.clone(), askbar_auditor::links(&dir.join("tokens")));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (url, auditor, dir)
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[((p / 100.0 * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1]
}

/// `clients` callers, each sending `each` span starts one after the other (one persistent connection each),
/// timing every acknowledgment.
async fn measure(url: &str, clients: usize, each: usize) -> Vec<Duration> {
    let tasks: Vec<_> = (0..clients)
        .map(|c| {
            let url = url.to_string();
            tokio::spawn(async move {
                let http = reqwest::Client::new();
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
                let mut times = Vec::with_capacity(each);
                for i in 0..each {
                    let body = json!({
                        "id": format!("c{c}s{i}"), "trace_id": format!("t{c}"), "kind": "turn", "name": "turn",
                        "started_at": now, "started_ms": now * 1000, "user_id": "u1", "provider_id": "p1", "model": "m1",
                        "attrs": { "space": "sp1", "conversation": "cv1" }
                    });
                    let t0 = Instant::now();
                    let r = http.post(format!("{url}/api/audit/v1/spans")).bearer_auth(TOKEN).json(&body).send().await.unwrap();
                    let took = t0.elapsed();
                    assert_eq!(r.status().as_u16(), 201, "an append failed");
                    times.push(took);
                }
                times
            })
        })
        .collect();
    let mut all = Vec::new();
    for t in tasks {
        all.extend(t.await.unwrap());
    }
    all.sort();
    all
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_acknowledgment_of_an_append_is_measured_and_printed() {
    let (url, auditor, dir) = start().await;
    // Warm up: the first connections and the first transaction are not what a model call sees.
    measure(&url, 1, 20).await;
    for (label, clients, each) in [("one caller", 1, 400), ("four callers at once", 4, 150)] {
        let all = measure(&url, clients, each).await;
        let (p50, p95, p99, max) = (percentile(&all, 50.0), percentile(&all, 95.0), percentile(&all, 99.0), *all.last().unwrap());
        println!(
            "acknowledgment of an append, {label}: {} requests, p50 {:.2} ms, p95 {:.2} ms (target 20 ms, not yet a gate), p99 {:.2} ms, max {:.2} ms",
            all.len(),
            p50.as_secs_f64() * 1000.0,
            p95.as_secs_f64() * 1000.0,
            p99.as_secs_f64() * 1000.0,
            max.as_secs_f64() * 1000.0
        );
        assert!(p95 < Duration::from_secs(1), "{label}: the 95th percentile is over the 1 s a caller waits");
    }
    assert!(auditor.recorder.backlog().queued == 0, "everything acknowledged is written");
    let _ = std::fs::remove_dir_all(dir);
}
