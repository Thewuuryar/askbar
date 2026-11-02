//! The device token lives in the OS keychain, never in a config file.

#[cfg(test)]
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
