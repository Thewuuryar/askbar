//! The device token lives in the OS keychain, never in a config file.

#[cfg(test)]
use std::sync::Mutex;

pub trait TokenStore: Send + Sync {
    fn get(&self) -> Option<String>;
    #[allow(dead_code)] // used by the pairing screen
    fn set(&self, token: &str) -> Result<(), String>;
    #[allow(dead_code)] // used when the server rejects the token
    fn clear(&self);
}

/// OS keychain entry. `profile` (the client.yaml path) keeps separate
/// profiles on one machine from sharing a token.
pub struct Keychain {
    profile: String,
}

impl Keychain {
    pub fn new(profile: &str) -> Keychain {
        Keychain { profile: profile.to_string() }
    }

    fn entry(&self) -> Result<keyring::Entry, keyring::Error> {
        keyring::Entry::new("askbar", &format!("device-token:{}", self.profile))
    }
}

impl TokenStore for Keychain {
    fn get(&self) -> Option<String> {
        self.entry().ok()?.get_password().ok().filter(|t| !t.is_empty())
    }

    fn set(&self, token: &str) -> Result<(), String> {
        self.entry()
            .and_then(|e| e.set_password(token))
            .map_err(|e| format!("Could not save the device token in the system keychain: {e}"))
    }

    fn clear(&self) {
        if let Ok(e) = self.entry() {
            let _ = e.delete_credential();
        }
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
    fn clear(&self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}
