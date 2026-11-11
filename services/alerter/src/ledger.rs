//! The repeat ledger: the limiter's state in a JSON file on the alerter's volume, written atomically, so a restart does not
//! start the count again (ADR 0055). It holds keys, times and counts: no secret, no sentence, no content. A file that cannot
//! be read, or is not what this writes, starts the ledger empty; the alerter never stops for it.

use crate::limits::State;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The file inside the data directory.
pub const FILE: &str = "ledger.json";

pub struct Ledger {
    path: PathBuf,
}

impl Ledger {
    pub fn new(dir: &Path) -> Ledger {
        Ledger { path: dir.join(FILE) }
    }

    /// The state the file holds, or an empty one if there is none or it is corrupt.
    pub fn load(&self) -> State {
        let Ok(text) = std::fs::read_to_string(&self.path) else { return State::default() };
        serde_json::from_str(&text).unwrap_or_else(|_| {
            tracing::warn!("the repeat ledger is not readable; starting it empty");
            State::default()
        })
    }

    /// Writes `state` to a temporary file in the same directory, syncs it and renames it over the ledger, so a crash leaves
    /// the old file or the new one and never half of one.
    pub fn save(&self, state: &State) -> std::io::Result<()> {
        let dir = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".{FILE}.tmp"));
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(serde_json::to_string(state).map_err(std::io::Error::other)?.as_bytes())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::KeyState;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("askbar-ledger-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_saved_ledger_comes_back_after_a_restart_and_leaves_no_temporary_file() {
        let d = dir();
        let l = Ledger::new(&d);
        assert_eq!(l.load(), State::default(), "no file: empty");
        let mut s = State::default();
        s.keys.insert("rejected_token|link=admin-server".into(), KeyState { last_sent: 5, suppressed: 2 });
        s.recent.push(5);
        l.save(&s).unwrap();
        assert_eq!(Ledger::new(&d).load(), s);
        let names: Vec<String> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, [FILE]);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_corrupt_ledger_starts_empty_and_is_replaced_by_the_next_save() {
        let d = dir();
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(FILE), "{ not json").unwrap();
        let l = Ledger::new(&d);
        assert_eq!(l.load(), State::default());
        l.save(&State::default()).unwrap();
        assert_eq!(l.load(), State::default());
        std::fs::write(d.join(FILE), r#"{"keys": 5}"#).unwrap();
        assert_eq!(l.load(), State::default(), "the wrong shape is corrupt too");
        let _ = std::fs::remove_dir_all(d);
    }
}
