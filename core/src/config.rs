//! YAML configuration: loading, defaults, provider/key resolution.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::{env, fs};

pub const DEFAULT_CONFIG: &str = include_str!("default_config.yaml");

const DEFAULT_SYSTEM_PROMPT: &str = "You are a fast, concise assistant in a quick-question popup. \
Answer directly. Use Markdown when it helps. You cannot access the user's computer, files, or the internet.";

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read {path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error("{path} is not valid: {message}")]
    Parse { path: String, message: String },
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// Any OpenAI-compatible `/chat/completions` endpoint.
    #[serde(alias = "openai-compatible", alias = "openai_compatible")]
    OpenAi,
    /// The Anthropic Messages API.
    Anthropic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ProviderKind,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_key_env: String,
    pub model: String,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

impl Provider {
    pub fn effective_base_url(&self) -> String {
        let url = self.base_url.trim();
        if !url.is_empty() {
            return url.trim_end_matches('/').to_string();
        }
        match self.kind {
            ProviderKind::OpenAi => "https://api.openai.com/v1".into(),
            ProviderKind::Anthropic => "https://api.anthropic.com".into(),
        }
    }

    /// `api_key` wins; otherwise the environment variable named in `api_key_env`.
    pub fn resolve_key(&self) -> Option<String> {
        let direct = self.api_key.trim();
        if !direct.is_empty() {
            return Some(direct.to_string());
        }
        let var = self.api_key_env.trim();
        if var.is_empty() {
            return None;
        }
        env::var(var)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Auto,
    Light,
    Dark,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Ui {
    pub theme: Theme,
    pub width: u32,
    pub max_height: u32,
    pub history_width: u32,
    pub top_percent: f64,
    pub hide_on_blur: bool,
    pub macos_hide_dock_icon: bool,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            theme: Theme::Auto,
            width: 720,
            max_height: 640,
            history_width: 260,
            top_percent: 0.22,
            hide_on_blur: true,
            macos_hide_dock_icon: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryCfg {
    /// Send only the most recent N messages to the model. `None` sends all.
    pub context_messages: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    #[serde(default)]
    pub active_provider: String,
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,
    #[serde(default = "default_providers")]
    pub providers: Vec<Provider>,
    #[serde(default)]
    pub history: HistoryCfg,
    #[serde(default)]
    pub ui: Ui,
}

fn default_hotkey() -> String {
    "Ctrl+Shift+Space".into()
}
fn default_system_prompt() -> String {
    DEFAULT_SYSTEM_PROMPT.into()
}
fn default_providers() -> Vec<Provider> {
    Config::builtin().providers
}

impl Config {
    /// The bundled default config (what a first run writes to disk).
    pub fn builtin() -> Config {
        serde_yaml_ng::from_str(DEFAULT_CONFIG).expect("bundled default_config.yaml must be valid")
    }

    pub fn from_yaml(text: &str, origin: &str) -> Result<Config, ConfigError> {
        let mut cfg: Config = serde_yaml_ng::from_str(text).map_err(|e| ConfigError::Parse {
            path: origin.to_string(),
            message: e.to_string(),
        })?;
        if cfg.providers.is_empty() {
            return Err(ConfigError::Invalid(format!(
                "{origin}: `providers` is empty; add at least one provider"
            )));
        }
        if cfg.active_provider.trim().is_empty() {
            cfg.active_provider = cfg.providers[0].name.clone();
        }
        cfg.ui.top_percent = cfg.ui.top_percent.clamp(0.0, 0.8);
        Ok(cfg)
    }

    /// Load the config at `path`, writing the commented default first if the
    /// file does not exist. Returns `(config, created)`.
    pub fn load_or_create(path: &Path) -> Result<(Config, bool), ConfigError> {
        let origin = path.display().to_string();
        let mut created = false;
        if !path.exists() {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir).map_err(|e| ConfigError::Io {
                    path: dir.display().to_string(),
                    source: e,
                })?;
            }
            fs::write(path, DEFAULT_CONFIG).map_err(|e| ConfigError::Io {
                path: origin.clone(),
                source: e,
            })?;
            created = true;
        }
        let text = fs::read_to_string(path).map_err(|e| ConfigError::Io {
            path: origin.clone(),
            source: e,
        })?;
        Ok((Config::from_yaml(&text, &origin)?, created))
    }

    pub fn active(&self) -> Result<&Provider, ConfigError> {
        self.providers
            .iter()
            .find(|p| p.name == self.active_provider)
            .ok_or_else(|| {
                let names: Vec<&str> = self.providers.iter().map(|p| p.name.as_str()).collect();
                ConfigError::Invalid(format!(
                    "active_provider \"{}\" is not in `providers` (found: {})",
                    self.active_provider,
                    names.join(", ")
                ))
            })
    }
}

/// `ASKBAR_CONFIG` overrides the default location.
pub fn config_path() -> PathBuf {
    if let Ok(p) = env::var("ASKBAR_CONFIG") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("askbar")
        .join("config.yaml")
}

/// `ASKBAR_DB` overrides the default location.
pub fn db_path() -> PathBuf {
    if let Ok(p) = env::var("ASKBAR_DB") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("askbar")
        .join("history.db")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_default_is_valid_and_complete() {
        let cfg = Config::builtin();
        assert_eq!(cfg.hotkey, "Ctrl+Shift+Space");
        assert!(cfg.providers.len() >= 4);
        assert_eq!(cfg.active().unwrap().kind, ProviderKind::Anthropic);
        let ollama = cfg.providers.iter().find(|p| p.name == "ollama").unwrap();
        assert_eq!(ollama.kind, ProviderKind::OpenAi);
        assert!(ollama.resolve_key().is_none());
        assert_eq!(cfg.history.context_messages, None);
    }

    #[test]
    fn minimal_yaml_gets_defaults() {
        let cfg = Config::from_yaml(
            "providers:\n  - name: x\n    type: openai\n    model: m\n",
            "test",
        )
        .unwrap();
        assert_eq!(cfg.active_provider, "x");
        assert_eq!(cfg.hotkey, "Ctrl+Shift+Space");
        assert_eq!(cfg.ui.width, 720);
        assert!(cfg.ui.hide_on_blur);
        assert!(cfg.system_prompt.contains("concise"));
        assert_eq!(cfg.providers[0].effective_base_url(), "https://api.openai.com/v1");
    }

    #[test]
    fn empty_file_falls_back_to_builtin_providers() {
        let cfg = Config::from_yaml("hotkey: \"Super+A\"\n", "test").unwrap();
        assert_eq!(cfg.hotkey, "Super+A");
        assert!(!cfg.providers.is_empty());
        assert_eq!(cfg.active_provider, cfg.providers[0].name);
    }

    #[test]
    fn parse_errors_name_the_file() {
        let err = Config::from_yaml("providers: [", "my/config.yaml").unwrap_err();
        assert!(err.to_string().contains("my/config.yaml"), "{err}");
    }

    #[test]
    fn unknown_active_provider_lists_options() {
        let cfg = Config::from_yaml("active_provider: nope\n", "t").unwrap();
        let msg = cfg.active().unwrap_err().to_string();
        assert!(msg.contains("nope") && msg.contains("anthropic"), "{msg}");
    }

    #[test]
    fn key_resolution_prefers_inline_then_env() {
        let mut p = Config::builtin().providers[0].clone();
        p.api_key = "  sk-inline ".into();
        p.api_key_env = "ASKBAR_TEST_KEY_A".into();
        assert_eq!(p.resolve_key().as_deref(), Some("sk-inline"));

        p.api_key = String::new();
        assert_eq!(p.resolve_key(), None);
        env::set_var("ASKBAR_TEST_KEY_A", " from-env ");
        assert_eq!(p.resolve_key().as_deref(), Some("from-env"));
        env::remove_var("ASKBAR_TEST_KEY_A");
    }

    #[test]
    fn top_percent_is_clamped() {
        let cfg = Config::from_yaml("ui:\n  top_percent: 5.0\n", "t").unwrap();
        assert_eq!(cfg.ui.top_percent, 0.8);
    }

    #[test]
    fn load_or_create_writes_default_once() {
        let dir = std::env::temp_dir().join(format!("askbar-cfg-test-{}", std::process::id()));
        let path = dir.join("nested").join("config.yaml");
        let _ = fs::remove_dir_all(&dir);
        let (_, created) = Config::load_or_create(&path).unwrap();
        assert!(created);
        assert!(fs::read_to_string(&path).unwrap().contains("active_provider"));
        let (_, created2) = Config::load_or_create(&path).unwrap();
        assert!(!created2);
        let _ = fs::remove_dir_all(&dir);
    }
}
