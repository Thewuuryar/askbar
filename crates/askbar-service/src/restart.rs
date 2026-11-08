//! Self-restart (ADR 0005): there is no controller and no service has the Docker socket. A service
//! asked to restart answers, finishes what it must within a time limit, and exits; Docker's restart
//! policy (`unless-stopped`) starts it again with a new boot ID.
//!
//! - **Cooldown:** a restart is refused for `cooldown` after the process started (`429
//!   too_many_attempts` with `Retry-After`), so a script or a loop cannot keep a service down.
//! - **Shutdown limit:** the service's own stop work (`on_stop`) gets at most `grace`; the process
//!   exits when it is done or when the time is up, whichever is first.
//! - **Watchdog:** Docker marks a failed health check unhealthy but does not restart the container,
//!   so each service checks itself on its own thread and exits after `failures` failed checks in a
//!   row.

use crate::status::BootId;
use askbar_proto::http::ApiError;
use askbar_proto::problem::ErrorCode;
use axum::{extract::State, http::StatusCode, Json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What a service does when it is asked to restart.
#[derive(Clone)]
pub struct Restarter {
    started: Instant,
    boot_id: BootId,
    cooldown: Duration,
    grace: Duration,
    on_stop: Arc<dyn Fn() + Send + Sync>,
    exit: Arc<dyn Fn() + Send + Sync>,
    asked: Arc<AtomicBool>,
}

/// Defaults: no restart in the first 30 seconds; at most 10 seconds to stop.
pub const COOLDOWN: Duration = Duration::from_secs(30);
pub const GRACE: Duration = Duration::from_secs(10);

impl Restarter {
    /// `on_stop` is the service's own stop work (save running replies, flush records). The process
    /// exits with status 0 afterwards.
    pub fn new(boot_id: BootId, on_stop: impl Fn() + Send + Sync + 'static) -> Restarter {
        Restarter {
            started: Instant::now(),
            boot_id,
            cooldown: COOLDOWN,
            grace: GRACE,
            on_stop: Arc::new(on_stop),
            exit: Arc::new(|| std::process::exit(0)),
            asked: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Tests only: other limits, and something other than exiting the process.
    #[doc(hidden)]
    pub fn for_tests(
        boot_id: BootId,
        cooldown: Duration,
        grace: Duration,
        on_stop: impl Fn() + Send + Sync + 'static,
        exit: impl Fn() + Send + Sync + 'static,
    ) -> Restarter {
        Restarter {
            started: Instant::now(),
            boot_id,
            cooldown,
            grace,
            on_stop: Arc::new(on_stop),
            exit: Arc::new(exit),
            asked: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Stop now (a refused cooldown aside): run `on_stop` for at most `grace`, then exit. Returns
    /// at once; the work happens on its own thread. A second call while one is under way does nothing.
    pub fn stop_soon(&self) {
        if self.asked.swap(true, Ordering::SeqCst) {
            return;
        }
        let (on_stop, exit, grace) = (self.on_stop.clone(), self.exit.clone(), self.grace);
        std::thread::spawn(move || {
            // Let the answer reach the caller first.
            std::thread::sleep(Duration::from_millis(200));
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                on_stop();
                let _ = tx.send(());
            });
            if rx.recv_timeout(grace).is_err() {
                eprintln!("restart: the service did not stop within {}s; exiting anyway", grace.as_secs());
            }
            exit();
        });
    }
}

/// The `POST /restart` handler: `202 {"boot_id"}` (the ID of the process that is about to stop), or
/// `429 too_many_attempts` during the cooldown.
pub async fn handler(State(r): State<Restarter>) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let up = r.started.elapsed();
    if up < r.cooldown {
        let wait = (r.cooldown - up).as_secs().max(1);
        return Err(ApiError::new(
            ErrorCode::TooManyAttempts,
            format!("This service started {}s ago. Restart it again in {wait}s.", up.as_secs()),
        )
        .retry_after(wait));
    }
    eprintln!("restart: asked to restart; stopping (boot {})", r.boot_id.as_str());
    r.stop_soon();
    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({ "boot_id": r.boot_id.as_str() }))))
}

/// Start the watchdog: on its own thread (so it still runs if the async runtime is stuck), call
/// `healthy` every `every`, starting after `every`; after `failures` failures in a row, run `exit`.
pub fn watchdog(every: Duration, failures: u32, healthy: impl Fn() -> bool + Send + 'static, exit: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        let mut bad = 0;
        loop {
            std::thread::sleep(every);
            if healthy() {
                bad = 0;
                continue;
            }
            bad += 1;
            eprintln!("watchdog: health check failed ({bad} of {failures})");
            if bad >= failures {
                eprintln!("watchdog: exiting so the container restarts");
                exit();
                return;
            }
        }
    });
}

/// The watchdog with the defaults: a check every 30 seconds, exit (status 1) after 3 failures.
pub fn start_watchdog(healthy: impl Fn() -> bool + Send + 'static) {
    watchdog(Duration::from_secs(30), 3, healthy, || std::process::exit(1));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    async fn serve(r: Restarter) -> String {
        let app = axum::Router::new().route("/restart", axum::routing::post(handler)).with_state(r);
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        url
    }

    #[tokio::test]
    async fn refused_during_the_cooldown_then_answers_stops_and_exits_once() {
        let (stopped, exited) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        let (s, e) = (stopped.clone(), exited.clone());
        let boot = BootId::new();
        let r = Restarter::for_tests(
            boot.clone(),
            Duration::from_millis(300),
            Duration::from_secs(2),
            move || {
                s.fetch_add(1, Ordering::SeqCst);
            },
            move || {
                e.fetch_add(1, Ordering::SeqCst);
            },
        );
        let url = serve(r).await;
        let http = reqwest::Client::new();
        let refused = http.post(format!("{url}/restart")).send().await.unwrap();
        assert_eq!(refused.status(), 429);
        assert!(refused.headers().contains_key("retry-after"));
        assert_eq!(refused.json::<askbar_proto::Problem>().await.unwrap().error, "too_many_attempts");
        tokio::time::sleep(Duration::from_millis(350)).await;
        let ok = http.post(format!("{url}/restart")).send().await.unwrap();
        assert_eq!(ok.status(), 202);
        assert_eq!(ok.json::<serde_json::Value>().await.unwrap()["boot_id"], boot.as_str());
        let again = http.post(format!("{url}/restart")).send().await.unwrap();
        assert_eq!(again.status(), 202, "a second ask while stopping is answered and changes nothing");
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!((stopped.load(Ordering::SeqCst), exited.load(Ordering::SeqCst)), (1, 1));
    }

    #[test]
    fn a_stop_that_hangs_is_cut_off_at_the_limit() {
        let exited = Arc::new(AtomicU32::new(0));
        let e = exited.clone();
        let r = Restarter::for_tests(
            BootId::new(),
            Duration::ZERO,
            Duration::from_millis(300),
            || std::thread::sleep(Duration::from_secs(60)),
            move || {
                e.fetch_add(1, Ordering::SeqCst);
            },
        );
        let t = Instant::now();
        r.stop_soon();
        while exited.load(Ordering::SeqCst) == 0 {
            assert!(t.elapsed() < Duration::from_secs(3), "did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    }

    #[test]
    fn the_watchdog_exits_after_failures_in_a_row_and_a_success_resets_the_count() {
        let calls = Arc::new(AtomicU32::new(0));
        let exited = Arc::new(AtomicBool::new(false));
        let (c, e) = (calls.clone(), exited.clone());
        // fail, ok, fail, fail, fail -> exit on the fifth check
        watchdog(Duration::from_millis(20), 3, move || c.fetch_add(1, Ordering::SeqCst) == 1, move || e.store(true, Ordering::SeqCst));
        let t = Instant::now();
        while !exited.load(Ordering::SeqCst) {
            assert!(t.elapsed() < Duration::from_secs(3), "never exited");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 5);
    }
}
