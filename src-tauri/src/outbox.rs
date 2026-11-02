//! The outbox: messages this device has sent (or tried to send) and has not yet seen the server
//! take. It keeps the text only; there is no offline chat. An entry is added when a message is
//! sent and removed when the server's `meta` arrives, so a message that never got there is
//! still here to Send, Edit or Discard. The file lives in the app data directory.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxEntry {
    /// Sent with the chat request; the server uses it to recognise a repeat of the same send.
    pub client_request_id: String,
    pub conversation_id: Option<String>,
    /// Id of the provider the message was sent to.
    pub provider: String,
    pub text: String,
    pub created_at: i64,
}

pub struct Outbox {
    path: PathBuf,
    entries: Mutex<Vec<OutboxEntry>>,
}

/// Where the outbox file goes by default.
pub fn default_path() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("askbar").join("outbox.json")
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

impl Outbox {
    /// An outbox that is never saved to disk (tests, and the default until a path is given).
    pub fn in_memory() -> Outbox {
        Outbox { path: PathBuf::new(), entries: Mutex::new(Vec::new()) }
    }

    /// Loads the file at `path`. A missing file is an empty outbox; so is one that cannot be
    /// read as an outbox (it is left alone until the next change replaces it).
    pub fn load(path: PathBuf) -> Outbox {
        let entries = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Outbox { path, entries: Mutex::new(entries) }
    }

    fn entries(&self) -> MutexGuard<'_, Vec<OutboxEntry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Adds a message with a fresh random request id and saves the file. Returns the entry.
    pub fn add(&self, conversation_id: Option<String>, provider: &str, text: &str) -> OutboxEntry {
        let entry = OutboxEntry {
            client_request_id: uuid::Uuid::new_v4().to_string(),
            conversation_id,
            provider: provider.to_string(),
            text: text.to_string(),
            created_at: now(),
        };
        let mut entries = self.entries();
        entries.push(entry.clone());
        self.save(&entries);
        entry
    }

    /// Removes the entry with this request id. False if there was none.
    pub fn remove(&self, client_request_id: &str) -> bool {
        let mut entries = self.entries();
        let before = entries.len();
        entries.retain(|e| e.client_request_id != client_request_id);
        let removed = entries.len() != before;
        if removed {
            self.save(&entries);
        }
        removed
    }

    pub fn get(&self, client_request_id: &str) -> Option<OutboxEntry> {
        self.entries().iter().find(|e| e.client_request_id == client_request_id).cloned()
    }

    /// Everything unsent, oldest first.
    pub fn list(&self) -> Vec<OutboxEntry> {
        self.entries().clone()
    }

    /// Writes to a temporary file and renames it over the real one, so a crash cannot leave half a file.
    /// Failing to save is not fatal: the message is still in memory and is being sent.
    fn save(&self, entries: &[OutboxEntry]) {
        if self.path.as_os_str().is_empty() {
            return;
        }
        let write = || -> std::io::Result<()> {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = self.path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(entries).map_err(std::io::Error::other)?)?;
            restrict(&tmp);
            std::fs::rename(&tmp, &self.path)
        };
        if let Err(e) = write() {
            eprintln!("could not save the outbox to {}: {e}", self.path.display());
        }
    }
}

/// The file holds what the person typed: readable by them only.
#[cfg(unix)]
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("askbar-outbox-{name}-{}", uuid::Uuid::new_v4()));
        dir.join("outbox.json")
    }

    #[test]
    fn add_gives_each_message_its_own_request_id() {
        let path = temp_path("add");
        let o = Outbox::load(path.clone());
        assert!(o.list().is_empty());
        let a = o.add(None, "p1", "first");
        let b = o.add(Some("c1".into()), "p1", "second");
        assert_ne!(a.client_request_id, b.client_request_id);
        assert_eq!(a.client_request_id.len(), 36, "a UUID: {}", a.client_request_id);
        assert_eq!(uuid::Uuid::parse_str(&a.client_request_id).unwrap().get_version_num(), 4);
        assert_eq!(o.list(), [a.clone(), b.clone()]);
        assert_eq!(o.get(&b.client_request_id), Some(b));
        assert_eq!((a.conversation_id, a.provider.as_str(), a.text.as_str()), (None, "p1", "first"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn remove_takes_one_entry_and_reports_whether_it_was_there() {
        let path = temp_path("remove");
        let o = Outbox::load(path.clone());
        let a = o.add(None, "p1", "first");
        let b = o.add(None, "p1", "second");
        assert!(o.remove(&a.client_request_id));
        assert!(!o.remove(&a.client_request_id), "already gone");
        assert_eq!(o.list(), [b]);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn entries_survive_a_restart_and_removals_do_too() {
        let path = temp_path("reload");
        let (a, b) = {
            let o = Outbox::load(path.clone());
            (o.add(Some("c9".into()), "p2", "keep me"), o.add(None, "p2", "drop me"))
        };
        let reloaded = Outbox::load(path.clone());
        assert_eq!(reloaded.list(), [a.clone(), b.clone()], "read back from disk, in order");
        reloaded.remove(&b.client_request_id);
        assert_eq!(Outbox::load(path.clone()).list(), [a], "the removal was saved");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_missing_or_unreadable_file_is_an_empty_outbox() {
        let path = temp_path("bad");
        assert!(Outbox::load(path.clone()).list().is_empty());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "this is not json").unwrap();
        let o = Outbox::load(path.clone());
        assert!(o.list().is_empty());
        let e = o.add(None, "p", "after a bad file");
        assert_eq!(Outbox::load(path.clone()).list(), [e], "the next change replaces the bad file");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
