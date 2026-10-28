//! `client.yaml`: where the server is and how this client looks. Secrets
//! (the device token) are not stored here; see `tokens`.

use askbar_core::{Theme, Ui};
use askbar_proto::DisplaySettings;
use reqwest::Url;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Where `client.yaml` lives. `ASKBAR_CLIENT_CONFIG` overrides it (used by tests and second profiles).
pub fn path() -> PathBuf {
    if let Ok(p) = std::env::var("ASKBAR_CLIENT_CONFIG") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("askbar").join("client.yaml")
}

/// Display fields a user may override locally; anything left out follows the server.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct UiOverrides {
    pub theme: Option<Theme>,
    pub width: Option<u32>,
    pub max_height: Option<u32>,
    pub history_width: Option<u32>,
    pub top_percent: Option<f64>,
    pub hide_on_blur: Option<bool>,
    pub macos_hide_dock_icon: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct ClientConfig {
    pub server_url: String,
    pub hotkey: String,
    pub active_provider: String,
    pub ui: UiOverrides,
    pub allow_insecure: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            hotkey: "Ctrl+Shift+Space".into(),
            active_provider: String::new(),
            ui: UiOverrides::default(),
            allow_insecure: false,
        }
    }
}

impl ClientConfig {
    pub fn load(path: &Path) -> Result<ClientConfig, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        if text.trim().is_empty() {
            return Ok(ClientConfig::default());
        }
        serde_yaml_ng::from_str(&text).map_err(|e| format!("Could not parse {}: {e}", path.display()))
    }

    /// The validated server URL, or why it can't be used.
    pub fn server(&self) -> Result<Url, String> {
        check_server_url(&self.server_url, self.allow_insecure)
    }
}

/// `https://` is always fine. `http://` only for a loopback host (this machine),
/// or when `allow_insecure` is set.
pub fn check_server_url(raw: &str, allow_insecure: bool) -> Result<Url, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("`server_url` is not set in client.yaml.".into());
    }
    let url = Url::parse(raw).map_err(|e| format!("`server_url` \"{raw}\" is not a valid URL: {e}"))?;
    match url.scheme() {
        "https" => Ok(url),
        "http" if allow_insecure || is_loopback(&url) => Ok(url),
        "http" => Err(format!(
            "Refusing to talk to {raw} over plain http. Use https, or set `allow_insecure: true` in client.yaml if you trust this network."
        )),
        other => Err(format!("`server_url` must start with https:// (found {other}://).")),
    }
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost") || d.to_ascii_lowercase().ends_with(".localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Server defaults with this client's overrides on top.
pub fn merge_ui(server: &DisplaySettings, o: &UiOverrides) -> Ui {
    let theme = o.theme.unwrap_or(match server.theme.as_str() {
        "light" => Theme::Light,
        "dark" => Theme::Dark,
        _ => Theme::Auto,
    });
    Ui {
        theme,
        width: o.width.unwrap_or(server.width),
        max_height: o.max_height.unwrap_or(server.max_height),
        history_width: o.history_width.unwrap_or(server.history_width),
        top_percent: o.top_percent.unwrap_or(server.top_percent).clamp(0.0, 0.8),
        hide_on_blur: o.hide_on_blur.unwrap_or(server.hide_on_blur),
        macos_hide_dock_icon: o.macos_hide_dock_icon.unwrap_or(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_is_always_allowed() {
        assert!(check_server_url("https://askbar.example.com:8443", false).is_ok());
        assert!(check_server_url("https://192.168.1.10", false).is_ok());
    }

    #[test]
    fn http_is_allowed_for_loopback_only() {
        for ok in ["http://localhost:8080", "http://127.0.0.1:8080", "http://127.5.5.5", "http://[::1]:8080", "http://app.localhost"] {
            assert!(check_server_url(ok, false).is_ok(), "{ok}");
        }
        for bad in ["http://192.168.1.10:8080", "http://nas.local:8080", "http://askbar.example.com", "http://10.0.0.5", "http://localhost.evil.com"] {
            let e = check_server_url(bad, false).unwrap_err();
            assert!(e.contains("allow_insecure"), "{bad}: {e}");
        }
    }

    #[test]
    fn allow_insecure_overrides_the_lan_refusal() {
        assert!(check_server_url("http://192.168.1.10:8080", true).is_ok());
    }

    #[test]
    fn other_schemes_and_junk_are_refused() {
        assert!(check_server_url("ftp://localhost", true).is_err());
        assert!(check_server_url("localhost:8080", true).is_err());
        assert!(check_server_url("", true).is_err());
        assert!(check_server_url("not a url", true).is_err());
    }

    #[test]
    fn display_overrides_merge_over_server_defaults() {
        let server = DisplaySettings { theme: "dark".into(), width: 800, max_height: 500, history_width: 300, top_percent: 0.3, hide_on_blur: true };
        let none = merge_ui(&server, &UiOverrides::default());
        assert_eq!((none.theme, none.width, none.max_height, none.history_width, none.hide_on_blur), (Theme::Dark, 800, 500, 300, true));
        assert!(none.macos_hide_dock_icon);

        let o = UiOverrides { theme: Some(Theme::Light), width: Some(640), hide_on_blur: Some(false), top_percent: Some(5.0), ..Default::default() };
        let m = merge_ui(&server, &o);
        assert_eq!((m.theme, m.width, m.max_height, m.hide_on_blur), (Theme::Light, 640, 500, false));
        assert_eq!(m.top_percent, 0.8, "clamped like the local config");
    }

    #[test]
    fn yaml_holds_only_the_documented_fields() {
        let dir = std::env::temp_dir().join(format!("askbar-client-yaml-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("client.yaml");
        std::fs::write(&p, "server_url: https://x.example.com\nactive_provider: mock\nui:\n  width: 600\n  theme: dark\n").unwrap();
        let c = ClientConfig::load(&p).unwrap();
        assert_eq!((c.server_url.as_str(), c.active_provider.as_str(), c.allow_insecure), ("https://x.example.com", "mock", false));
        assert_eq!(c.hotkey, "Ctrl+Shift+Space");
        assert_eq!((c.ui.width, c.ui.theme), (Some(600), Some(Theme::Dark)));
        std::fs::write(&p, "server_url: [oops").unwrap();
        assert!(ClientConfig::load(&p).unwrap_err().contains("client.yaml"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
