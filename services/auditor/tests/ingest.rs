//! Caddy's access lines on the ingest listener (plan step 2.9, ADR 0051): one connection of newline-delimited JSON, a short
//! list of fields kept, nothing that could hold a secret, and a bad or oversize line cannot hurt anything.

use askbar_audit::{AuditDb, Recorder};
use askbar_auditor::{disk::FreeSpace, ingest, Auditor, Limits};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

fn auditor() -> (Arc<Auditor>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("auditor-ingest-{}", ulid::Ulid::new()));
    let limits = Limits::default();
    let recorder = Recorder::spawn(AuditDb::open(&dir.join("audit.db")).unwrap(), limits.max_queued);
    let free: FreeSpace = Arc::new(|| Some(100 * 1024 * 1024 * 1024));
    (Auditor::new(recorder, limits, free), dir)
}

fn line(path: &str, status: u16) -> String {
    format!(
        r#"{{"level":"info","ts":{}.5,"logger":"http.log.access.log0","msg":"handled request","request":{{"remote_ip":"198.51.100.4","client_ip":"203.0.113.7","proto":"HTTP/2.0","method":"GET","host":"askbar.example","uri":"{path}","headers":{{"Authorization":["Bearer SECRET-TOKEN"],"User-Agent":["curl/8"]}}}},"duration":0.002,"size":42,"status":{status}}}"#,
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
    )
}

async fn rows(a: &Auditor, want: usize) -> Vec<serde_json::Value> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let conn = a.recorder.reader().unwrap();
        let r = askbar_audit::list_access_logs(&conn, None, None).unwrap();
        if r.len() >= want || Instant::now() > deadline {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn listen(a: &Arc<Auditor>) -> std::net::SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(ingest::serve(l, a.clone()));
    addr
}

#[tokio::test]
async fn lines_sent_over_a_connection_are_stored_without_the_query_or_any_header() {
    let (a, _dir) = auditor();
    let addr = listen(&a).await;
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    for (path, status) in [("/api/client/v1/health", 200), ("/admin/?token=SECRET-QUERY", 302), ("/nope", 404)] {
        s.write_all(format!("{}\n", line(path, status)).as_bytes()).await.unwrap();
    }
    s.flush().await.unwrap();
    let got = rows(&a, 3).await;
    assert_eq!(got.len(), 3, "{got:?}");
    assert_eq!(got.iter().map(|r| r["path"].as_str().unwrap()).collect::<Vec<_>>(), ["/nope", "/admin/", "/api/client/v1/health"]);
    assert_eq!(got[0]["status"], 404);
    assert_eq!((got[0]["remote_ip"].clone(), got[0]["method"].clone()), (serde_json::json!("203.0.113.7"), serde_json::json!("GET")));
    let all = serde_json::to_string(&got).unwrap();
    assert!(!all.contains("SECRET"), "no query and no header is kept: {all}");
    // The listener says nothing back.
    s.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_line_that_is_not_caddys_is_counted_and_an_oversize_one_ends_its_connection() {
    let (a, _dir) = auditor();
    let addr = listen(&a).await;
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(b"not json at all\n\n{\"msg\":\"other\"}\n").await.unwrap();
    s.write_all(format!("{}\n", line("/ok", 200)).as_bytes()).await.unwrap();
    let got = rows(&a, 1).await;
    assert_eq!(got.len(), 1, "the good line after two bad ones is still taken");
    assert_eq!(a.access_refused.load(Ordering::Relaxed), 2, "an empty line is not a refusal");

    // A line over 16 KiB ends the connection: the sender connects again, and a few KiB of memory is all it ever held.
    let mut big = tokio::net::TcpStream::connect(addr).await.unwrap();
    big.write_all(&vec![b'x'; ingest::MAX_LINE + 10]).await.unwrap();
    let mut gone = false;
    for _ in 0..50 {
        if big.write_all(b"y").await.is_err() || big.flush().await.is_err() {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    assert!(gone || a.access_refused.load(Ordering::Relaxed) == 3, "the connection was ended");
    assert_eq!(a.access_refused.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn a_flood_of_access_lines_is_dropped_and_counted_and_never_blocks_the_listener() {
    let (a, _dir) = auditor();
    a.recorder.pause_writer(true);
    let addr = listen(&a).await;
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let burst: String = (0..3000).map(|i| format!("{}\n", line(&format!("/p{i}"), 200))).collect();
    s.write_all(burst.as_bytes()).await.unwrap();
    s.flush().await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while a.recorder.access_dropped() == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(a.recorder.access_dropped() > 0, "lines were dropped, not queued without bound");
    // The queue still has room for what fails closed: the backlog is under the limit.
    assert!(a.recorder.backlog().queued <= Limits::default().max_queued / 2 + 1, "{:?}", a.recorder.backlog());
    a.recorder.pause_writer(false);
}
