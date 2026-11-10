//! How much room the audit volume has left. The standard library has no call for it and `unsafe` is denied, so
//! this asks `df`, which every Debian image has, and remembers the answer for a few seconds.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Bytes free where the audit store is, or `None` when it cannot be told.
pub type FreeSpace = Arc<dyn Fn() -> Option<u64> + Send + Sync>;

/// `df -Pk <dir>`: the fourth column of the second line is the available kilobytes.
pub fn parse_df(output: &str) -> Option<u64> {
    let line = output.lines().nth(1)?;
    let kb: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
    kb.checked_mul(1024)
}

fn ask_df(dir: &Path) -> Option<u64> {
    let out = std::process::Command::new("df").arg("-Pk").arg(dir).output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_df(&String::from_utf8_lossy(&out.stdout))
}

/// The real thing for `dir`, asked at most once every `every`.
pub fn free_space_of(dir: &Path, every: Duration) -> FreeSpace {
    let dir = dir.to_path_buf();
    let last: Mutex<Option<(Instant, Option<u64>)>> = Mutex::new(None);
    Arc::new(move || {
        let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, value)) = *last {
            if at.elapsed() < every {
                return value;
            }
        }
        let value = ask_df(&dir);
        *last = Some((Instant::now(), value));
        value
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn df_output_gives_the_available_bytes() {
        let out = "Filesystem     1024-blocks      Used Available Capacity Mounted on\n/dev/vda        263112020  32043408   8123456      80% /\n";
        assert_eq!(parse_df(out), Some(8_123_456 * 1024));
        assert_eq!(parse_df("nothing useful"), None);
        assert_eq!(parse_df("header only\n"), None);
    }

    #[test]
    fn a_real_directory_has_some_room_and_the_answer_is_remembered() {
        let free = free_space_of(&std::env::temp_dir(), Duration::from_secs(60));
        let a = free();
        assert!(a.is_some_and(|n| n > 0), "{a:?}");
        assert_eq!(free(), a, "asked once, then remembered");
    }
}
