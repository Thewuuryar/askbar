//! CLIProxyAPI (CPA): the admin makes its configuration and API key, connects Claude and ChatGPT accounts
//! through CPA's own control API (no terminal), and keeps one "managed" provider pointing at it.
//!
//! NOTE: the control API paths and shapes below are written from CPA's documentation as remembered and are
//! not yet checked against the pinned image (`eceasy/cli-proxy-api:v8.0.51`, the base of the `askbar-cpa` image). They are all in this file, and
//! `tests/mock_cpa` in `askbar-admin/tests/cpa.rs` states what is assumed.
//!
//! Secrets: the CPA API key and CPA's control secret are made here and kept as files in the admin's secrets
//! directory (mode 0600). The control secret goes into CPA's config in the shared `cpa-config` volume, which
//! only the admin and CPA mount. The browser never sees either.

use crate::server_link::write_atomic;
use crate::AdminState;
use askbar_proto::http::{ApiError, ApiJson};
use askbar_proto::problem::ErrorCode;
use axum::{
    extract::{Path, State},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chacha20poly1305::aead::{rand_core::RngCore, OsRng};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::Duration;

type St = State<Arc<AdminState>>;
type Failure = ApiError;

pub const API_KEY_FILE: &str = "cpa-api.key";
pub const SECRET_FILE: &str = "cpa-control.secret";
/// How long a login may take from "Connect" to the pasted address.
const LOGIN_TTL_SECS: i64 = 10 * 60;
const MAX_PENDING: usize = 8;

pub fn config_dir() -> PathBuf {
    std::env::var("ASKBAR_CPA_CONFIG_DIR").ok().filter(|p| !p.trim().is_empty()).map(PathBuf::from).unwrap_or_else(|| "/cpa-config".into())
}

/// Where this service reaches CPA's API: `ASKBAR_CPA_URL`, default `http://cpa:8317`.
pub fn cpa_url() -> String {
    std::env::var("ASKBAR_CPA_URL").ok().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| "http://cpa:8317".into())
}

#[derive(Clone)]
pub struct Keys {
    pub api_key: String,
    pub control_secret: String,
}

fn rand(n: usize) -> String {
    let mut raw = vec![0u8; n * 2];
    OsRng.fill_bytes(&mut raw);
    URL_SAFE_NO_PAD.encode(&raw).chars().filter(|c| c.is_ascii_alphanumeric()).take(n).collect()
}

fn config_yaml(api_key: &str, secret: &str) -> String {
    // The same file cpa/init.sh used to write.
    format!(
        "config-version: 8\nserver:\n  host: \"\"\n  port: 8317\noauth:\n  auth-dir: \"/root/.cli-proxy-api\"\naccess:\n  api-keys:\n    - \"{api_key}\"\nmanagement: # names-ok: CLIProxyAPI's own config key\n  # Reachable only on the compose networks: the API port is not published to the host.\n  allow-remote: true\n  secret-key: \"{secret}\"\n"
    )
}

fn read_trim(p: &FsPath) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn write_secret(p: &FsPath, v: &str) -> std::io::Result<()> {
    write_atomic(p, v, 0o600)
}

/// Make CPA's config if it is not there, or adopt the one that is (an install from before the admin managed it:
/// its API key is kept so the provider and any logins carry on). Returns the keys, and whether CPA's control
/// secret had to be replaced in an existing config (CPA then needs a restart to read it).
pub fn ensure_config(config_dir: &FsPath, secrets_dir: &FsPath) -> std::io::Result<(Keys, bool)> {
    std::fs::create_dir_all(secrets_dir)?;
    let (api_path, secret_path) = (secrets_dir.join(API_KEY_FILE), secrets_dir.join(SECRET_FILE));
    let config = config_dir.join("config.yaml");
    if !config.exists() {
        std::fs::create_dir_all(config_dir)?;
        let keys = Keys { api_key: read_trim(&api_path).unwrap_or_else(|| format!("sk-cpa-{}", rand(40))), control_secret: read_trim(&secret_path).unwrap_or_else(|| rand(40)) };
        write_atomic(&config, &config_yaml(&keys.api_key, &keys.control_secret), 0o644)?;
        write_secret(&api_path, &keys.api_key)?;
        write_secret(&secret_path, &keys.control_secret)?;
        return Ok((keys, false));
    }
    let text = std::fs::read_to_string(&config)?;
    let api_key = read_trim(&api_path)
        .or_else(|| read_trim(&config_dir.join("askbar-api-key")))
        .or_else(|| {
            let after = text.split("api-keys:").nth(1)?;
            let line = after.lines().find(|l| l.trim_start().starts_with('-'))?;
            Some(line.trim_start().trim_start_matches('-').trim().trim_matches('"').to_string()).filter(|k| !k.is_empty())
        })
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "CLIProxyAPI's config has no API key to adopt"))?;
    write_secret(&api_path, &api_key)?;
    if let Some(secret) = read_trim(&secret_path) {
        return Ok((Keys { api_key, control_secret: secret }, false));
    }
    // An existing config whose control secret we never saw (CPA may have hashed it): put a new one in.
    let secret = rand(40);
    let mut done = false;
    let rewritten: String = text
        .lines()
        .map(|l| {
            if !done && l.trim_start().starts_with("secret-key:") {
                done = true;
                format!("{}secret-key: \"{secret}\"", &l[..l.len() - l.trim_start().len()])
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !done {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "CLIProxyAPI's config has no secret-key line to replace"));
    }
    write_atomic(&config, &(rewritten + "\n"), 0o644)?;
    write_secret(&secret_path, &secret)?;
    Ok((Keys { api_key, control_secret: secret }, true))
}

/// The files the `askbar-cpa` supervisor watches and writes (ADR 0037), in the config directory.
pub const RESTART_REQUEST: &str = "restart-request";
pub const RESTART_DONE: &str = "restart-done";

/// Make `restart-done` writable by the supervisor: mode 0660 in the admin's group. The supervisor runs as
/// root in group 10001 with every capability dropped (ADR 0032), so it can write a file of the admin's
/// only through the group bit, and cannot create one in the admin's directory.
pub fn prepare_restart_done(config_dir: &FsPath) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let p = config_dir.join(RESTART_DONE);
    if !p.exists() {
        std::fs::write(&p, "")?;
    }
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o660))
}

/// Ask the supervisor to restart CLIProxyAPI: write a new random value to `restart-request`. Returns
/// the value; the supervisor writes it to `restart-done` once CLIProxyAPI has started again.
pub fn request_restart(config_dir: &FsPath) -> std::io::Result<String> {
    prepare_restart_done(config_dir)?;
    let value = rand(24);
    write_atomic(&config_dir.join(RESTART_REQUEST), &value, 0o644)?;
    Ok(value)
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Account {
    pub name: String,
    /// `anthropic` or `openai`.
    pub provider: String,
    pub label: String,
}

pub struct CpaLink {
    http: reqwest::Client,
    base: String,
    keys: Keys,
}

/// The admin's name for a provider, as CPA's endpoints spell it.
fn cpa_name(provider: &str) -> Option<(&'static str, &'static str)> {
    match provider {
        "anthropic" => Some(("anthropic", "anthropic-auth-url")),
        "openai" => Some(("codex", "codex-auth-url")),
        _ => None,
    }
}

const API: &str = "/v0/management"; // names-ok: CLIProxyAPI's own path

impl CpaLink {
    pub fn new(base: &str, keys: Keys) -> Arc<CpaLink> {
        let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(3)).timeout(Duration::from_secs(20)).build().expect("reqwest client");
        Arc::new(CpaLink { http, base: base.trim_end_matches('/').to_string(), keys })
    }

    pub fn api_key(&self) -> &str {
        &self.keys.api_key
    }

    /// Where the server reaches CPA's OpenAI-compatible API.
    pub fn provider_url(&self) -> String {
        format!("{}/v1", self.base)
    }

    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, String> {
        let mut req = self.http.request(method, format!("{}{API}{path}", self.base)).bearer_auth(&self.keys.control_secret);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let r = req.send().await.map_err(|e| format!("CLIProxyAPI did not answer: {}", e.without_url()))?;
        let status = r.status();
        let text = r.text().await.map_err(|e| e.without_url().to_string())?;
        if !status.is_success() {
            let why = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"].as_str().or(v["message"].as_str()).map(String::from)).unwrap_or_else(|| format!("status {}", status.as_u16()));
            return Err(format!("CLIProxyAPI refused: {why}"));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// Start a login: the address to open, and the state that ties the answer to it.
    pub async fn auth_url(&self, provider: &str) -> Result<(String, String), String> {
        let (_, endpoint) = cpa_name(provider).ok_or("Unknown provider.")?;
        let v = self.call(reqwest::Method::GET, &format!("/{endpoint}?is_webui=true"), None).await?;
        match (v["url"].as_str(), v["state"].as_str()) {
            (Some(u), Some(s)) if !u.is_empty() && !s.is_empty() => Ok((u.to_string(), s.to_string())),
            _ => Err("CLIProxyAPI gave no login address.".into()),
        }
    }

    pub async fn callback(&self, provider: &str, redirect_url: &str, code: &str, state: &str) -> Result<(), String> {
        let (name, _) = cpa_name(provider).ok_or("Unknown provider.")?;
        self.call(reqwest::Method::POST, "/oauth-callback", Some(json!({ "provider": name, "redirect_url": redirect_url, "code": code, "state": state }))).await.map(|_| ())
    }

    /// `("wait" | "ok" | "error", message)`.
    pub async fn login_status(&self, state: &str) -> Result<(String, String), String> {
        let v = self.call(reqwest::Method::GET, &format!("/get-auth-status?state={}", urlencode(state)), None).await?;
        Ok((v["status"].as_str().unwrap_or("wait").to_string(), v["error"].as_str().unwrap_or("").to_string()))
    }

    pub async fn accounts(&self) -> Result<Vec<Account>, String> {
        let v = self.call(reqwest::Method::GET, "/auth-files", None).await?;
        Ok(v["files"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|f| {
                        let name = f["name"].as_str()?.to_string();
                        let kind = f["provider"].as_str().or(f["type"].as_str()).unwrap_or("");
                        let provider = match kind {
                            "anthropic" | "claude" => "anthropic",
                            "codex" | "openai" => "openai",
                            _ => return None,
                        };
                        let label = f["email"].as_str().or(f["label"].as_str()).unwrap_or(&name).to_string();
                        Some(Account { name, provider: provider.to_string(), label })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    pub async fn remove(&self, name: &str) -> Result<(), String> {
        self.call(reqwest::Method::DELETE, &format!("/auth-files?name={}", urlencode(name)), None).await.map(|_| ())
    }
}

fn urlencode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

// ---------------------------------------------------------------- the managed provider

/// The managed provider's id, if it exists.
fn managed_id(conn: &rusqlite::Connection) -> Option<String> {
    conn.query_row("SELECT id FROM providers WHERE managed = 1", [], |r| r.get(0)).ok()
}

/// Make the managed provider once CPA lists a model (so only after an account is connected): named `CPA`, type
/// `openai`, pointing at CPA with the API key the admin made, billed as a subscription. The model starts as
/// the first one CPA lists (Claude's, if any) and is changed from the dropdown like any other.
pub async fn ensure_managed_provider(state: &AdminState, link: &CpaLink) -> Result<Option<String>, String> {
    if let Some(id) = managed_id(state.db.lock().await.conn()) {
        return Ok(Some(id));
    }
    let server = state.server.as_ref().ok_or("The server is not linked.")?;
    let body = json!({ "type": "openai", "base_url": link.provider_url(), "api_key": link.api_key() });
    let (status, bytes) = server.request(reqwest::Method::POST, &askbar_proto::paths::join(askbar_proto::paths::SERVER_V1, "/models"), Some(&body)).await.map_err(|_| "The server is offline.".to_string())?;
    let list: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let ids: Vec<&str> = list["models"].as_array().map(|a| a.iter().filter_map(|m| m["id"].as_str()).collect()).unwrap_or_default();
    if status != 200 || ids.is_empty() {
        return Ok(None); // nothing to choose yet; try again after the next login
    }
    let model = ids.iter().find(|m| m.starts_with("claude")).unwrap_or(&ids[0]).to_string();
    let id = ulid::Ulid::new().to_string();
    let (new_id, url, key) = (id.clone(), link.provider_url(), link.api_key().to_string());
    crate::admin_api::mutate(state, vec![(id.clone(), key)], move |c| {
        c.execute(
            "INSERT INTO providers (id, name, type, base_url, model, created_at, billing, managed) VALUES (?1, 'CPA', 'openai', ?2, ?3, ?4, 'subscription', 1)",
            rusqlite::params![new_id, url, model, crate::auth::now()],
        )
        .map(|_| ())
        .map_err(|_| crate::web::err(axum::http::StatusCode::CONFLICT, "A provider named CPA already exists."))
    })
    .await
    .map_err(|_| "Could not save the CPA provider.".to_string())?;
    Ok(Some(id))
}

// ---------------------------------------------------------------- the API

fn link(state: &AdminState) -> Result<Arc<CpaLink>, Failure> {
    state.cpa.lock().unwrap_or_else(|e| e.into_inner()).clone().ok_or_else(|| ApiError::new(ErrorCode::ServerUnavailable, "CLIProxyAPI is not set up here."))
}

fn down(e: String) -> Failure {
    ApiError::new(ErrorCode::ServerUnavailable, e)
}

/// `GET /api/admin/v1/cpa`: whether CPA answers, the accounts connected to it, and its provider.
pub async fn status(State(state): St) -> Json<Value> {
    let Ok(l) = link(&state) else { return Json(json!({ "configured": false, "reachable": false, "accounts": [], "provider_id": null })) };
    let accounts = l.accounts().await;
    let reachable = accounts.is_ok();
    let accounts = accounts.unwrap_or_default();
    // Catches an account that was connected some other way, too.
    let provider_id = if reachable && !accounts.is_empty() { ensure_managed_provider(&state, &l).await.ok().flatten() } else { managed_id(state.db.lock().await.conn()) };
    Json(json!({ "configured": true, "reachable": reachable, "accounts": accounts, "provider_id": provider_id }))
}

#[derive(Deserialize)]
pub struct LoginInput {
    provider: String,
}

/// `POST /api/admin/v1/cpa/logins`: start a login. Open `url` in a browser, approve, then paste the address the
/// browser ends up on into `POST .../logins/{state}/callback`.
pub async fn start_login(State(state): St, ApiJson(i): ApiJson<LoginInput>) -> Result<Json<Value>, Failure> {
    if cpa_name(&i.provider).is_none() {
        return Err(ApiError::new(ErrorCode::BadRequest, "Provider must be anthropic or openai."));
    }
    let l = link(&state)?;
    let now = crate::auth::now();
    {
        let mut p = state.cpa_logins.lock().unwrap_or_else(|e| e.into_inner());
        p.retain(|_, (_, exp)| *exp > now);
        if p.len() >= MAX_PENDING {
            return Err(ApiError::new(ErrorCode::TooManyAttempts, "Too many logins are waiting. Finish or wait for them to expire."));
        }
    }
    let (url, st) = l.auth_url(&i.provider).await.map_err(down)?;
    state.cpa_logins.lock().unwrap_or_else(|e| e.into_inner()).insert(st.clone(), (i.provider, now + LOGIN_TTL_SECS));
    Ok(Json(json!({ "url": url, "state": st, "expires_in": LOGIN_TTL_SECS })))
}

#[derive(Deserialize)]
pub struct CallbackInput {
    redirect_url: String,
}

/// What a pasted address must be: a `localhost` or `127.0.0.1` address carrying the `code` and the `state` of this login.
fn check_redirect(raw: &str, state: &str) -> Result<String, &'static str> {
    let u = reqwest::Url::parse(raw.trim()).map_err(|_| "That is not an address. Paste the whole address from the browser's address bar.")?;
    if !matches!(u.scheme(), "http" | "https") || !matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")) {
        return Err("That address does not start with http://localhost. Paste the address the browser ended up on after you approved.");
    }
    let q: std::collections::HashMap<_, _> = u.query_pairs().collect();
    if q.get("state").map(|s| s.as_ref()) != Some(state) {
        return Err("That address belongs to a different login.");
    }
    match q.get("code") {
        Some(c) if !c.is_empty() => Ok(c.to_string()),
        _ => Err("That address has no code. Was the login approved?"),
    }
}

/// `POST /api/admin/v1/cpa/logins/{state}/callback`.
pub async fn finish_login(State(state): St, Path(st): Path<String>, ApiJson(i): ApiJson<CallbackInput>) -> Result<Json<Value>, Failure> {
    let l = link(&state)?;
    let now = crate::auth::now();
    // Single use: the login is taken out now, so a pasted address cannot be replayed.
    let (provider, exp) = state.cpa_logins.lock().unwrap_or_else(|e| e.into_inner()).remove(&st).ok_or_else(|| ApiError::new(ErrorCode::NotFound, "No such login, or it was already used."))?;
    if exp <= now {
        return Err(ApiError::new(ErrorCode::NotFound, "This login expired. Start it again."));
    }
    let code = match check_redirect(&i.redirect_url, &st) {
        Ok(c) => c,
        Err(why) => {
            // A typo should not cost the login: put it back.
            state.cpa_logins.lock().unwrap_or_else(|e| e.into_inner()).insert(st, (provider, exp));
            return Err(ApiError::new(ErrorCode::BadRequest, why));
        }
    };
    l.callback(&provider, i.redirect_url.trim(), &code, &st).await.map_err(down)?;
    Ok(Json(json!({ "ok": true })))
}

/// `GET /api/admin/v1/cpa/logins/{state}`: `wait`, `ok` or `error`. On `ok` the CPA provider is made if it was not.
pub async fn login_status(State(state): St, Path(st): Path<String>) -> Result<Json<Value>, Failure> {
    let l = link(&state)?;
    let (status, message) = l.login_status(&st).await.map_err(down)?;
    let mut provider_id = None;
    if status == "ok" {
        provider_id = ensure_managed_provider(&state, &l).await.ok().flatten();
    }
    Ok(Json(json!({ "status": status, "message": message, "provider_id": provider_id })))
}

/// `DELETE /api/admin/v1/cpa/accounts/{name}`: disconnect an account.
pub async fn remove_account(State(state): St, Path(name): Path<String>) -> Result<axum::http::StatusCode, Failure> {
    if name.is_empty() || name.len() > 200 || name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(ApiError::new(ErrorCode::NotFound, "No such account."));
    }
    let l = link(&state)?;
    if !l.accounts().await.map_err(down)?.iter().any(|a| a.name == name) {
        return Err(ApiError::new(ErrorCode::NotFound, "No such account."));
    }
    l.remove(&name).await.map_err(down)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restart_request_is_a_new_value_and_restart_done_is_group_writable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("askbar-cpa-req-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = request_restart(&dir).unwrap();
        let b = request_restart(&dir).unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read_to_string(dir.join(RESTART_REQUEST)).unwrap(), b);
        let done = dir.join(RESTART_DONE);
        assert_eq!(std::fs::metadata(&done).unwrap().permissions().mode() & 0o777, 0o660, "the supervisor writes it through the group");
        std::fs::write(&done, "x").unwrap();
        prepare_restart_done(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(&done).unwrap(), "x", "an existing file is kept, only its mode is set");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pasted_address_must_be_local_and_carry_this_logins_state_and_a_code() {
        assert_eq!(check_redirect("http://localhost:54545/callback?code=abc&state=S1", "S1"), Ok("abc".into()));
        assert_eq!(check_redirect(" http://127.0.0.1:1455/auth/callback?state=S1&code=xyz ", "S1"), Ok("xyz".into()));
        assert!(check_redirect("http://localhost:1/cb?code=abc&state=OTHER", "S1").is_err(), "another login's state");
        assert!(check_redirect("http://localhost:1/cb?state=S1", "S1").is_err(), "no code");
        assert!(check_redirect("https://evil.example/cb?code=abc&state=S1", "S1").is_err(), "not local");
        assert!(check_redirect("http://localhost.evil.example/cb?code=abc&state=S1", "S1").is_err(), "a look-alike host");
        assert!(check_redirect("not an address", "S1").is_err());
    }

    #[test]
    fn a_fresh_config_has_the_key_and_secret_and_an_old_one_keeps_its_key() {
        let d = std::env::temp_dir().join(format!("cpa-{}", ulid::Ulid::new()));
        let (cfg, sec) = (d.join("cfg"), d.join("sec"));
        let (k, changed) = ensure_config(&cfg, &sec).unwrap();
        assert!(!changed && k.api_key.starts_with("sk-cpa-") && k.control_secret.len() == 40);
        let yaml = std::fs::read_to_string(cfg.join("config.yaml")).unwrap();
        assert!(yaml.contains(&k.api_key) && yaml.contains(&k.control_secret));
        // Again: nothing changes.
        let (k2, changed) = ensure_config(&cfg, &sec).unwrap();
        assert!(!changed && k2.api_key == k.api_key && k2.control_secret == k.control_secret);
        // An install from before: a config written by cpa/init.sh, and the key in askbar-api-key.
        let (cfg2, sec2) = (d.join("cfg2"), d.join("sec2"));
        std::fs::create_dir_all(&cfg2).unwrap();
        std::fs::write(cfg2.join("config.yaml"), config_yaml("sk-cpa-OLDKEY", "$2a$10$hashedbycpa")).unwrap();
        std::fs::write(cfg2.join("askbar-api-key"), "sk-cpa-OLDKEY").unwrap();
        let (k3, changed) = ensure_config(&cfg2, &sec2).unwrap();
        assert!(changed, "its control secret was replaced, so CPA needs a restart");
        assert_eq!(k3.api_key, "sk-cpa-OLDKEY");
        let yaml = std::fs::read_to_string(cfg2.join("config.yaml")).unwrap();
        assert!(yaml.contains("sk-cpa-OLDKEY") && yaml.contains(&k3.control_secret) && !yaml.contains("hashedbycpa"));
        let _ = std::fs::remove_dir_all(d);
    }
}
