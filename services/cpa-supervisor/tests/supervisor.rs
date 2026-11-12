//! The supervisor against a stand-in child: a shell loop that logs each start and each SIGTERM.

use askbar_cpa_supervisor::{run, Settings};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct T {
    dir: PathBuf,
    log: PathBuf,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    settings: Settings,
}

impl Drop for T {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// `script` gets `$LOG`. Timings are short so the tests run in seconds.
fn start(script: &str, grace: Duration) -> T {
    let dir = std::env::temp_dir().join(format!("askbar-sup-{}-{}", std::process::id(), Instant::now().elapsed().as_nanos() ^ rand_u64()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("log");
    std::fs::write(dir.join("config.yaml"), "port: 1\n").unwrap();
    let mut s = Settings::for_dir(dir.to_str().unwrap(), vec!["sh".into(), "-c".into(), script.replace("$LOG", log.to_str().unwrap())]);
    s.transfer = askbar_cpa_supervisor::transfer::Paths::for_dirs(&dir, &dir.join("auth"));
    s.poll = Duration::from_millis(50);
    s.min_gap = Duration::from_millis(400);
    s.grace = grace;
    s.backoff_start = Duration::from_millis(100);
    s.backoff_max = Duration::from_millis(400);
    s.stable_after = Duration::from_secs(60);
    let stop = Arc::new(AtomicBool::new(false));
    let (s2, st) = (s.clone(), stop.clone());
    let handle = std::thread::spawn(move || run(&s2, &st));
    T { dir, log, stop, handle: Some(handle), settings: s }
}

fn rand_u64() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

const LOOP: &str = r#"echo start >> $LOG; trap 'echo term >> $LOG; exit 0' TERM; while true; do sleep 0.02; done"#;

impl T {
    fn starts(&self) -> usize {
        std::fs::read_to_string(&self.log).unwrap_or_default().lines().filter(|l| *l == "start").count()
    }
    fn terms(&self) -> usize {
        std::fs::read_to_string(&self.log).unwrap_or_default().lines().filter(|l| *l == "term").count()
    }
    fn wait_for(&self, what: &str, f: impl Fn(&T) -> bool) {
        let t = Instant::now();
        while !f(self) {
            assert!(t.elapsed() < Duration::from_secs(5), "timed out waiting for {what}; log: {:?}", std::fs::read_to_string(&self.log));
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn a_config_change_and_a_restart_request_each_restart_the_child_and_done_echoes_the_request() {
    let t = start(LOOP, Duration::from_secs(2));
    t.wait_for("the first start", |t| t.starts() == 1);
    std::thread::sleep(Duration::from_millis(450)); // past the minimum gap
    std::fs::write(&t.settings.config, "port: 2\n").unwrap();
    t.wait_for("a restart on the config change", |t| t.starts() == 2);
    assert_eq!(t.terms(), 1, "the old child was asked to stop first");
    std::thread::sleep(Duration::from_millis(450));
    std::fs::write(&t.settings.request, "req-123").unwrap();
    t.wait_for("a restart on the request", |t| t.starts() == 3);
    t.wait_for("done to echo the request", |t| std::fs::read_to_string(&t.settings.done).unwrap_or_default() == "req-123");
    // Nothing changed: no more restarts.
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(t.starts(), 3);
}

#[test]
fn at_most_one_restart_per_gap_and_a_change_inside_it_waits() {
    let t = start(LOOP, Duration::from_secs(2));
    t.wait_for("the first start", |t| t.starts() == 1);
    std::fs::write(&t.settings.request, "a").unwrap(); // inside the gap after the start
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(t.starts(), 1, "too soon after the start");
    t.wait_for("the restart once the gap is over", |t| t.starts() == 2);
}

#[test]
fn a_child_that_exits_is_started_again_with_a_growing_wait() {
    let t = start("echo start >> $LOG; exit 1", Duration::from_secs(1));
    t.wait_for("several starts", |t| t.starts() >= 4);
    // Waits of 100, 200, 400, 400 ms: four starts take at least 700 ms.
    let t0 = Instant::now();
    let n = t.starts();
    t.wait_for("one more start", |t| t.starts() > n);
    assert!(t0.elapsed() >= Duration::from_millis(250), "the wait has grown to the maximum: {:?}", t0.elapsed());
}

#[test]
fn a_stop_passes_sigterm_on_and_returns_once_the_child_has_exited() {
    let mut t = start(LOOP, Duration::from_secs(2));
    t.wait_for("the first start", |t| t.starts() == 1);
    let t0 = Instant::now();
    t.stop.store(true, Ordering::SeqCst);
    t.handle.take().unwrap().join().unwrap();
    assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
    assert_eq!(t.terms(), 1, "the child got SIGTERM");
}

#[test]
fn a_child_that_ignores_sigterm_is_killed_after_the_grace_time() {
    let mut t = start(r#"echo start >> $LOG; trap 'echo term >> $LOG' TERM; while true; do sleep 0.02; done"#, Duration::from_millis(300));
    t.wait_for("the first start", |t| t.starts() == 1);
    let t0 = Instant::now();
    t.stop.store(true, Ordering::SeqCst);
    t.handle.take().unwrap().join().unwrap();
    let took = t0.elapsed();
    assert!(took >= Duration::from_millis(280) && took < Duration::from_secs(2), "killed at the limit: {took:?}");
}

// ---- export and import (ADR 0013, ADR 0037)

const PASS: &str = "a long backup passphrase 1";

fn wait_done(t: &T, path: &std::path::Path, id: &str) -> serde_json::Value {
    let t0 = Instant::now();
    loop {
        if let Ok(b) = std::fs::read(path) {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&b) {
                if v["id"] == id {
                    return v;
                }
            }
        }
        assert!(t0.elapsed() < Duration::from_secs(5), "timed out; log: {:?}", std::fs::read_to_string(&t.log));
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn an_export_is_the_auth_files_encrypted_to_the_key_and_the_child_is_not_restarted() {
    use askbar_service::{backup, tar};
    let t = start(LOOP, Duration::from_secs(2));
    t.wait_for("the first start", |t| t.starts() == 1);
    let p = &t.settings.transfer;
    std::fs::create_dir_all(&p.auth).unwrap();
    std::fs::write(p.auth.join("claude-a.json"), r#"{"token":"first"}"#).unwrap();
    std::fs::write(p.auth.join("codex-b.json"), r#"{"token":"second"}"#).unwrap();
    std::fs::create_dir_all(p.auth.join("subdir")).unwrap(); // not a login file: left out
    let key = backup::generate_with(PASS, 10).unwrap();
    std::fs::write(&p.export_request, serde_json::json!({"id": "e1", "recipient": key.recipient}).to_string()).unwrap();
    let done = wait_done(&t, &p.export_done, "e1");
    assert_eq!(done["ok"], true);
    let bytes = std::fs::read(&p.export_file).unwrap();
    assert_eq!(done["bytes"], bytes.len() as u64);
    assert!(!String::from_utf8_lossy(&bytes).contains("first"), "encrypted");
    let identity = backup::unlock(&key.protected, PASS).unwrap();
    let files = tar::read(&backup::decrypt(&identity, &bytes).unwrap()).unwrap();
    assert_eq!(
        files,
        vec![
            ("claude-a.json".to_string(), br#"{"token":"first"}"#.to_vec()),
            ("codex-b.json".to_string(), br#"{"token":"second"}"#.to_vec())
        ]
    );
    // Acted on once: a new file is not exported again for the same request id.
    std::fs::write(p.auth.join("later.json"), "{}").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(std::fs::read(&p.export_file).unwrap(), bytes);
    assert_eq!(t.starts(), 1, "an export does not restart CLIProxyAPI");
}

#[test]
fn an_export_to_a_bad_key_fails_and_leaves_no_file() {
    let t = start(LOOP, Duration::from_secs(2));
    let p = &t.settings.transfer;
    std::fs::write(&p.export_request, r#"{"id":"e2","recipient":"not-a-key"}"#).unwrap();
    assert_eq!(wait_done(&t, &p.export_done, "e2")["ok"], false);
    assert_eq!(std::fs::read(&p.export_file).unwrap_or_default().len(), 0, "no export left");
    // A request with no recipient fails the same way.
    std::fs::write(&p.export_request, r#"{"id":"e3"}"#).unwrap();
    assert_eq!(wait_done(&t, &p.export_done, "e3")["ok"], false);
}

#[test]
fn an_import_unpacks_the_files_restarts_the_child_and_removes_the_archive() {
    let t = start(LOOP, Duration::from_secs(2));
    t.wait_for("the first start", |t| t.starts() == 1);
    let p = &t.settings.transfer;
    std::fs::create_dir_all(&p.auth).unwrap();
    std::fs::write(p.auth.join("old.json"), "old").unwrap();
    let mut archive = Vec::new();
    askbar_service::tar::append(&mut archive, "claude-a.json", br#"{"token":"first"}"#, 0);
    askbar_service::tar::finish(&mut archive);
    std::fs::write(&p.import_file, &archive).unwrap();
    std::fs::write(&p.import_request, r#"{"id":"i1"}"#).unwrap();
    assert_eq!(wait_done(&t, &p.import_done, "i1")["ok"], true);
    assert_eq!(std::fs::read_to_string(p.auth.join("claude-a.json")).unwrap(), r#"{"token":"first"}"#);
    assert_eq!(std::fs::read_to_string(p.auth.join("old.json")).unwrap(), "old", "files not in the archive are left alone");
    assert_eq!(std::fs::read(&p.import_file).unwrap().len(), 0, "the plain archive does not stay on the volume");
    t.wait_for("a restart after the import", |t| t.starts() == 2);
    assert_eq!(t.terms(), 1);
}

#[test]
fn a_bad_import_changes_nothing_and_says_so() {
    let t = start(LOOP, Duration::from_secs(2));
    let p = &t.settings.transfer;
    std::fs::create_dir_all(&p.auth).unwrap();
    std::fs::write(p.auth.join("keep.json"), "keep").unwrap();
    let mut archive = Vec::new();
    askbar_service::tar::append(&mut archive, "sub/x.json", b"{}", 0); // a directory part: not an auth file
    askbar_service::tar::finish(&mut archive);
    std::fs::write(&p.import_file, &archive).unwrap();
    std::fs::write(&p.import_request, r#"{"id":"i2"}"#).unwrap();
    assert_eq!(wait_done(&t, &p.import_done, "i2")["ok"], false);
    assert_eq!(std::fs::read_to_string(p.auth.join("keep.json")).unwrap(), "keep");
    assert!(!p.auth.join("sub").exists());
    assert_eq!(std::fs::read(&p.import_file).unwrap().len(), 0);
    // No archive at all: also a failure.
    std::fs::write(&p.import_request, r#"{"id":"i3"}"#).unwrap();
    assert_eq!(wait_done(&t, &p.import_done, "i3")["ok"], false);
}
