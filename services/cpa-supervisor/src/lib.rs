//! The supervisor in the `askbar-cpa` image ([ADR 0037](../../../docs/adr/0037-askbar-cpa-image-and-supervisor.md)).
//!
//! CLIProxyAPI reads its config only when it starts. This program runs it as a child and restarts it:
//! - when `config.yaml` changes (by content, checked every `poll`);
//! - when the admin writes a new value to `restart-request`;
//! - when it exits on its own, after a wait that starts at 1 s, doubles up to 30 s, and resets once
//!   the child has run for a minute.
//!
//! At most one restart every `min_gap` (5 s); a change inside the gap waits for it. After each start
//! it writes the request value it acted on to `restart-done`. On a stop (SIGTERM or SIGINT in the
//! binary, `stop` here) it passes SIGTERM to the child, waits up to `grace`, kills it if needed, and
//! returns. It opens no port and holds no secret. It also answers export and import requests ([`transfer`], ADR 0013).

pub mod transfer;

use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// What to run and how.
#[derive(Clone, Debug)]
pub struct Settings {
    /// The child's program and arguments.
    pub command: Vec<String>,
    /// CLIProxyAPI's config file; a change to its content restarts the child.
    pub config: PathBuf,
    /// Written by the admin; a new value restarts the child.
    pub request: PathBuf,
    /// Written here after each start: the request value acted on.
    pub done: PathBuf,
    /// The export and import request files (ADR 0013).
    pub transfer: transfer::Paths,
    pub poll: Duration,
    pub min_gap: Duration,
    pub grace: Duration,
    pub backoff_start: Duration,
    pub backoff_max: Duration,
    /// A child that ran this long resets the backoff.
    pub stable_after: Duration,
}

impl Settings {
    /// The image's settings: the config and the trigger files under `dir`.
    pub fn for_dir(dir: &str, command: Vec<String>) -> Settings {
        let d = PathBuf::from(dir);
        let auth =
            std::env::var("ASKBAR_CPA_AUTH_DIR").ok().filter(|a| !a.trim().is_empty()).unwrap_or_else(|| "/root/.cli-proxy-api".into());
        Settings {
            command,
            config: d.join("config.yaml"),
            request: d.join("restart-request"),
            done: d.join("restart-done"),
            transfer: transfer::Paths::for_dirs(&d, std::path::Path::new(&auth)),
            poll: Duration::from_secs(2),
            min_gap: Duration::from_secs(5),
            grace: Duration::from_secs(10),
            backoff_start: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            stable_after: Duration::from_secs(60),
        }
    }
}

/// A file's content, or empty if it cannot be read (a missing file is "no value").
fn read(p: &std::path::Path) -> Vec<u8> {
    std::fs::read(p).unwrap_or_default()
}

/// Tell the admin which request was acted on. A failure is logged: the admin then waits in vain.
fn write_done(s: &Settings, request: &[u8]) {
    if let Err(e) = std::fs::write(&s.done, request) {
        log(&format!("could not write {}: {e}", s.done.display()));
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn log(msg: &str) {
    eprintln!("askbar-cpa-supervisor: {msg}");
}

fn spawn(s: &Settings) -> Option<Child> {
    let (prog, args) = s.command.split_first()?;
    match Command::new(prog).args(args).spawn() {
        Ok(c) => Some(c),
        Err(e) => {
            log(&format!("could not start {prog}: {e}"));
            None
        }
    }
}

/// SIGTERM, then up to `grace` for it to exit, then SIGKILL.
#[allow(unsafe_code)] // kill(2): std can send only SIGKILL (ADR 0037)
fn stop_child(child: &mut Child, grace: Duration) {
    if let Ok(Some(_)) = child.try_wait() {
        return;
    }
    // SAFETY: kill(2) with our own child's pid and a constant signal.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
    let until = Instant::now() + grace;
    while Instant::now() < until {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    log("CLIProxyAPI did not stop in time; killing it");
    let _ = child.kill();
    let _ = child.wait();
}

/// Run until `stop` is set. Returns after the child has stopped.
pub fn run(s: &Settings, stop: &AtomicBool) {
    let mut config = read(&s.config);
    let mut request = read(&s.request);
    let mut child = spawn(s);
    let mut started = Instant::now();
    write_done(s, &request);
    let mut backoff = s.backoff_start;
    let mut crashed_at: Option<Instant> = None;
    loop {
        // Sleep in small steps so a stop is acted on quickly.
        let until = Instant::now() + s.poll;
        while Instant::now() < until && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(20).min(s.poll));
        }
        if stop.load(Ordering::SeqCst) {
            if let Some(c) = child.as_mut() {
                log("stopping CLIProxyAPI");
                stop_child(c, s.grace);
            }
            return;
        }

        // The child exited on its own: start it again after the backoff.
        let exited = match child.as_mut() {
            Some(c) => matches!(c.try_wait(), Ok(Some(_))),
            None => true,
        };
        if exited {
            let at = *crashed_at.get_or_insert_with(Instant::now);
            if at.elapsed() < backoff {
                continue;
            }
            if started.elapsed() >= s.stable_after {
                backoff = s.backoff_start;
            }
            log(&format!("CLIProxyAPI exited; starting it again (next wait {}s)", (backoff * 2).min(s.backoff_max).as_secs()));
            child = spawn(s);
            started = Instant::now();
            crashed_at = None;
            backoff = (backoff * 2).min(s.backoff_max);
            write_done(s, &request);
            continue;
        }

        // An export is answered at once; it does not touch the child.
        transfer::export(&s.transfer, unix_now());

        // An import stops the child, unpacks the files and starts it again (no waiting for the gap: the owner asked for it).
        if let Some(id) = transfer::import_pending(&s.transfer) {
            log("import requested; restarting CLIProxyAPI");
            if let Some(c) = child.as_mut() {
                stop_child(c, s.grace);
            }
            transfer::import(&s.transfer, &id);
            child = spawn(s);
            started = Instant::now();
            write_done(s, &request);
            continue;
        }

        // A change asks for a restart; at most one per `min_gap`.
        let (new_config, new_request) = (read(&s.config), read(&s.request));
        if new_config == config && new_request == request {
            continue;
        }
        if started.elapsed() < s.min_gap {
            continue;
        }
        log(if new_request != request { "restart requested; restarting CLIProxyAPI" } else { "config changed; restarting CLIProxyAPI" });
        if let Some(c) = child.as_mut() {
            stop_child(c, s.grace);
        }
        config = new_config;
        request = new_request;
        child = spawn(s);
        started = Instant::now();
        write_done(s, &request);
    }
}

/// The flag the signal handler sets.
pub static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// Set `STOP` on SIGTERM and SIGINT.
#[allow(unsafe_code)] // signal(2): std has no signal API (ADR 0037)
pub fn install_signal_handlers() {
    // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
}
