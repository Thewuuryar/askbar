//! What the admin pushes in `PUT /config` (ADR 0028): the tool limits, each within the bounds in `askbar_proto::mcp`.
//! Until the first push the front runs on [`Config::default`] (the `askbar_proto` defaults) and refuses every call.

use crate::permissions::{self, Permissions};
use askbar_proto::mcp::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub fetch_max_bytes: u64,
    pub fetch_timeout_secs: u32,
    pub fetch_redirects: u32,
    pub tool_call_timeout_secs: u32,
    pub one_time_id_window_secs: u32,
    pub search_results: u32,
    pub remembered_urls: u32,
    /// The approval public key: 64 hex characters (32 raw bytes). None until the admin pushes one; every call is refused then.
    #[serde(default)]
    pub approval_public_key: Option<String>,
    /// The version of that key, for the admin's records. Not checked against anything.
    #[serde(default)]
    pub approval_key_version: u64,
    /// Per user, the tools held. A user with no entry holds none (ADR 0019).
    #[serde(default)]
    pub permissions: Permissions,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            fetch_max_bytes: DEFAULT_FETCH_MAX_BYTES,
            fetch_timeout_secs: DEFAULT_FETCH_TIMEOUT_SECS,
            fetch_redirects: DEFAULT_FETCH_REDIRECTS,
            tool_call_timeout_secs: DEFAULT_TOOL_CALL_TIMEOUT_SECS,
            one_time_id_window_secs: DEFAULT_ONE_TIME_ID_WINDOW_SECS,
            search_results: DEFAULT_SEARCH_RESULTS,
            remembered_urls: DEFAULT_REMEMBERED_URLS,
            approval_public_key: None,
            approval_key_version: 0,
            permissions: Default::default(),
        }
    }
}

impl Config {
    /// Every value within its bounds; the sentence names no value.
    pub fn validate(&self) -> Result<(), &'static str> {
        let ok = (MIN_FETCH_MAX_BYTES..=MAX_FETCH_MAX_BYTES).contains(&self.fetch_max_bytes)
            && (MIN_FETCH_TIMEOUT_SECS..=MAX_FETCH_TIMEOUT_SECS).contains(&self.fetch_timeout_secs)
            && (MIN_FETCH_REDIRECTS..=MAX_FETCH_REDIRECTS).contains(&self.fetch_redirects)
            && (MIN_TOOL_CALL_TIMEOUT_SECS..=MAX_TOOL_CALL_TIMEOUT_SECS).contains(&self.tool_call_timeout_secs)
            && (MIN_ONE_TIME_ID_WINDOW_SECS..=MAX_ONE_TIME_ID_WINDOW_SECS).contains(&self.one_time_id_window_secs)
            && (MIN_SEARCH_RESULTS..=MAX_SEARCH_RESULTS).contains(&self.search_results)
            && (MIN_REMEMBERED_URLS..=MAX_REMEMBERED_URLS).contains(&self.remembered_urls);
        let ok = ok && permissions::valid(&self.permissions);
        let ok = ok && self.approval_public_key.as_ref().is_none_or(|k| k.len() == 64 && self.public_key_bytes().is_some());
        if ok {
            Ok(())
        } else {
            Err("A value is outside its bounds (askbar-proto, mcp).")
        }
    }

    /// The approval public key as raw bytes, if one is set and is hex.
    pub fn public_key_bytes(&self) -> Option<Vec<u8>> {
        let k = self.approval_public_key.as_deref()?;
        if !k.is_ascii() || k.len() % 2 != 0 {
            return None;
        }
        (0..k.len()).step_by(2).map(|i| u8::from_str_radix(&k[i..i + 2], 16).ok()).collect()
    }

    /// The version: the first 8 bytes of the SHA-256 of the content, so equal content has an equal version.
    pub fn version(&self) -> u64 {
        let h = Sha256::digest(serde_json::to_vec(self).expect("a config serializes"));
        u64::from_be_bytes(h[..8].try_into().expect("8 bytes"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_valid_and_the_version_follows_the_content() {
        let a = Config::default();
        assert!(a.validate().is_ok());
        let b = Config { search_results: 9, ..a.clone() };
        assert_ne!(a.version(), b.version());
        assert_eq!(a.version(), Config::default().version());
        assert!(Config { fetch_redirects: MAX_FETCH_REDIRECTS + 1, ..a.clone() }.validate().is_err());
        assert!(Config { approval_public_key: Some("ab".repeat(32)), ..a.clone() }.validate().is_ok());
        for bad in ["ab", &"zz".repeat(32), &"é".repeat(32)] {
            assert!(Config { approval_public_key: Some(bad.to_string()), ..a.clone() }.validate().is_err(), "{bad}");
        }
    }
}
