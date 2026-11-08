//! The client token lives in the OS keychain, never in a config file.

use std::sync::Mutex;

pub trait TokenStore: Send + Sync {
    fn get(&self) -> Option<String>;
    fn set(&self, token: &str) -> Result<(), String>;
}

/// OS keychain entry. `profile` (the client.yaml path) keeps separate
/// profiles on one machine from sharing a token.
pub struct Keychain {
    profile: String,
}

impl Keychain {
    pub fn new(client_yaml: &std::path::Path) -> Keychain {
        Keychain { profile: client_yaml.display().to_string() }
    }

    fn entry(&self) -> Result<keyring::Entry, keyring::Error> {
        // The entry name predates the word "client"; renaming it would drop every stored token.
        keyring::Entry::new("askbar", &format!("device-token:{}", self.profile))
        // names-ok
    }
}

impl TokenStore for Keychain {
    fn get(&self) -> Option<String> {
        self.entry().ok()?.get_password().ok().filter(|t| !t.is_empty())
    }

    fn set(&self, token: &str) -> Result<(), String> {
        self.entry().and_then(|e| e.set_password(token)).map_err(|e| format!("Could not save the client token in the system keychain: {e}"))
    }
}

/// Reads the token from `inner` once and keeps it in memory. On macOS each keychain read can ask the
/// person for their password, and the window starts a dozen requests at once; the lock is held during
/// the read, so they all wait for that one read and see at most one prompt. A missing token is not
/// remembered, so pairing later is noticed.
pub struct CachedTokens<S: TokenStore> {
    inner: S,
    token: Mutex<Option<String>>,
}

impl<S: TokenStore> CachedTokens<S> {
    pub fn new(inner: S) -> CachedTokens<S> {
        CachedTokens { inner, token: Mutex::new(None) }
    }
}

impl<S: TokenStore> TokenStore for CachedTokens<S> {
    fn get(&self) -> Option<String> {
        let mut cached = self.token.lock().unwrap_or_else(|e| e.into_inner());
        if cached.is_none() {
            *cached = self.inner.get();
        }
        cached.clone()
    }

    fn set(&self, token: &str) -> Result<(), String> {
        let mut cached = self.token.lock().unwrap_or_else(|e| e.into_inner());
        self.inner.set(token)?;
        *cached = Some(token.to_string());
        Ok(())
    }
}

#[cfg(test)]
#[derive(Default)]
pub struct MemoryTokens(Mutex<Option<String>>);

#[cfg(test)]
impl MemoryTokens {
    pub fn with(token: &str) -> MemoryTokens {
        MemoryTokens(Mutex::new(Some(token.to_string())))
    }
}

#[cfg(test)]
impl TokenStore for MemoryTokens {
    fn get(&self) -> Option<String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn set(&self, token: &str) -> Result<(), String> {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(token.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Counts reads, and is slow to answer, like a keychain waiting on a password prompt.
    #[derive(Default)]
    struct Counting {
        reads: AtomicUsize,
        token: Mutex<Option<String>>,
    }

    impl TokenStore for Arc<Counting> {
        fn get(&self) -> Option<String> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(20));
            self.token.lock().unwrap().clone()
        }
        fn set(&self, token: &str) -> Result<(), String> {
            *self.token.lock().unwrap() = Some(token.to_string());
            Ok(())
        }
    }

    #[test]
    fn many_requests_at_once_read_the_keychain_once() {
        let inner = Arc::new(Counting::default());
        *inner.token.lock().unwrap() = Some("tok".into());
        let cached = Arc::new(CachedTokens::new(inner.clone()));
        let threads: Vec<_> = (0..12)
            .map(|_| {
                let c = cached.clone();
                std::thread::spawn(move || c.get())
            })
            .collect();
        for t in threads {
            assert_eq!(t.join().unwrap().as_deref(), Some("tok"));
        }
        assert_eq!(inner.reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_token_saved_at_pairing_is_used_without_reading_it_back() {
        let inner = Arc::new(Counting::default());
        let cached = CachedTokens::new(inner.clone());
        cached.set("new").unwrap();
        assert_eq!(cached.get().as_deref(), Some("new"));
        assert_eq!(inner.token.lock().unwrap().as_deref(), Some("new"), "the keychain holds it too");
        assert_eq!(inner.reads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_missing_token_is_looked_for_again() {
        let inner = Arc::new(Counting::default());
        let cached = CachedTokens::new(inner.clone());
        assert_eq!(cached.get(), None);
        assert_eq!(cached.get(), None);
        assert_eq!(inner.reads.load(Ordering::SeqCst), 2);
    }
}
