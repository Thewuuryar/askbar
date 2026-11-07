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
