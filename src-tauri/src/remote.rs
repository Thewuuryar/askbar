//! `Remote`: the client for an Askbar server over HTTPS.

use crate::types::{ClientStatus, Emit, Settings, StreamEvent};
use crate::client_config::{check_server_url, merge_ui, ClientConfig};
use crate::tokens::TokenStore;
use askbar_proto::{ChatEvent, ChatRequest, Conversation, ConversationDetail, DisplaySettings, Message, ProviderInfo, SseParser};
use futures_util::StreamExt;
use reqwest::{Method, RequestBuilder, StatusCode, Url};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::oneshot;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Health and status checks must not make the window feel stuck.
const CHECK_TIMEOUT: Duration = Duration::from_secs(2);

struct Inner {
    cfg: Mutex<ClientConfig>,
    path: PathBuf,
    base: Url,
    http: reqwest::Client,
    tokens: Box<dyn TokenStore>,
}

pub struct Remote {
    inner: Result<Inner, String>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Remote {
    /// A bad `client.yaml` doesn't stop the app; every call reports the problem instead.
    pub fn new(cfg: Result<ClientConfig, String>, path: PathBuf, tokens: Box<dyn TokenStore>) -> Remote {
        let inner = cfg.and_then(|cfg| {
            let base = cfg.server()?;
            let http = reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .user_agent(format!("askbar/{VERSION}"))
                .build()
                .map_err(|e| e.to_string())?;
            Ok(Inner { cfg: Mutex::new(cfg), path, base, http, tokens })
        });
        Remote { inner }
    }

    fn inner(&self) -> Result<&Inner, String> {
        self.inner.as_ref().map_err(Clone::clone)
    }
}

impl Inner {
    fn cfg(&self) -> ClientConfig {
        lock(&self.cfg).clone()
    }

    fn url(&self, path: &str) -> Url {
        let mut u = self.base.clone();
        u.set_path(&format!("{}{path}", self.base.path().trim_end_matches('/')));
        u
    }

    fn request(&self, method: Method, path: &str) -> Result<RequestBuilder, String> {
        let token = self.tokens.get().ok_or("This device is not paired with the server yet.")?;
        Ok(self
            .http
            .request(method, self.url(path))
            .bearer_auth(token)
            .header(askbar_proto::CLIENT_VERSION_HEADER, VERSION))
    }

    async fn send(&self, req: RequestBuilder) -> Result<reqwest::Response, String> {
        let resp = req
            .send()
            .await
            .map_err(|e| format!("Could not reach the server at {}: {e}", self.base))?;
        check_status(resp.status())?;
        Ok(resp)
    }

    async fn http_status(&self, req: RequestBuilder) -> Result<StatusCode, reqwest::Error> {
        Ok(req.send().await?.status())
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let resp = self.send(self.request(Method::GET, path)?.timeout(REQUEST_TIMEOUT)).await?;
        resp.json().await.map_err(|e| format!("The server sent an unexpected reply: {e}"))
    }

    async fn active_provider(&self) -> Result<String, String> {
        let wanted = self.cfg().active_provider;
        if !wanted.trim().is_empty() {
            return Ok(wanted.trim().to_string());
        }
        let list: Vec<ProviderInfo> = self.get_json("/api/v1/providers").await?;
        list.into_iter().next().map(|p| p.name).ok_or_else(|| "The server has no providers configured yet.".into())
    }
}

fn check_status(status: StatusCode) -> Result<(), String> {
    match status {
        s if s.is_success() => Ok(()),
        StatusCode::UNAUTHORIZED => Err("The server rejected this device's token.".into()),
        StatusCode::UPGRADE_REQUIRED => Err("This version of Askbar is too old for the server. Please update.".into()),
        StatusCode::SERVICE_UNAVAILABLE => Err(SETUP_PENDING.into()),
        s => Err(format!("The server answered {s}.")),
    }
}

const SETUP_PENDING: &str = "The server's setup is not finished. Open its admin page to complete it.";

/// Redeem a pairing string: exchange its code for a device token, keep the
/// token in `tokens`, and record the server in `client.yaml` (creating it, or
/// updating `server_url` and leaving the rest alone).
pub async fn pair(pairing: &str, client_yaml: &Path, tokens: &dyn TokenStore) -> Result<(), String> {
    let (url, code) = askbar_proto::decode_pairing(pairing)
        .ok_or("That is not a pairing string. It should start with \"askbar-pair:\".")?;
    let mut cfg = if client_yaml.exists() { ClientConfig::load(client_yaml)? } else { ClientConfig::default() };
    let mut base = check_server_url(&url, cfg.allow_insecure)?;
    base.set_path(&format!("{}/api/v1/pair", base.path().trim_end_matches('/')));

    let resp = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?
        .post(base)
        .header(askbar_proto::CLIENT_VERSION_HEADER, VERSION)
        .json(&askbar_proto::PairRequest { code })
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|_| format!("Could not reach the server at {url}. Check the address and your network."))?;
    match resp.status() {
        s if s.is_success() => {}
        StatusCode::UNAUTHORIZED => return Err("The pairing code is wrong, expired (they last 10 minutes) or already used. Create a new one.".into()),
        StatusCode::UPGRADE_REQUIRED => return Err("This version of Askbar is too old for the server. Please update.".into()),
        s => return Err(format!("The server answered {s}.")),
    }
    let token = resp
        .json::<askbar_proto::PairResponse>()
        .await
        .map_err(|_| "The server sent an unexpected reply.".to_string())?
        .token;
    tokens.set(&token)?;
    cfg.server_url = url.trim_end_matches('/').to_string();
    cfg.save(client_yaml)
}

impl Remote {
    pub async fn settings(&self) -> Result<Settings, String> {
        let i = self.inner()?;
        let cfg = i.cfg();
        // Offline is not an error here: the window still opens with the local
        // look and feel, and `status` tells the user what is wrong.
        let quick = |path: &str| i.request(Method::GET, path).map(|r| r.timeout(CHECK_TIMEOUT));
        let display = match quick("/api/v1/settings") {
            Ok(req) => match i.send(req).await {
                Ok(r) => r.json::<askbar_proto::Settings>().await.map(|s| s.display).unwrap_or_default(),
                Err(_) => DisplaySettings::default(),
            },
            Err(_) => DisplaySettings::default(),
        };
        let providers: Vec<ProviderInfo> = match quick("/api/v1/providers") {
            Ok(req) => match i.send(req).await {
                Ok(r) => r.json().await.unwrap_or_default(),
                Err(_) => Vec::new(),
            },
            Err(_) => Vec::new(),
        };
        let wanted = cfg.active_provider.trim();
        let active = providers.iter().find(|p| wanted.is_empty() || p.name == wanted);
        Ok(Settings {
            ui: merge_ui(&display, &cfg.ui),
            hotkey: cfg.hotkey.clone(),
            provider: active.map(|p| p.name.clone()).unwrap_or_else(|| wanted.to_string()),
            model: active.map(|p| p.model.clone()).unwrap_or_default(),
        })
    }

    pub async fn status(&self) -> Option<ClientStatus> {
        let i = match self.inner() {
            Ok(i) => i,
            Err(e) => return Some(ClientStatus::new("problem", e)),
        };
        let offline = || {
            ClientStatus::new(
                "offline",
                format!("Server offline. Could not reach {}. Check that it is running and that you are on the right network.", i.base),
            )
        };
        // Reachable at all? The health endpoint needs neither token nor version.
        match i.http.get(i.url("/api/v1/health")).timeout(CHECK_TIMEOUT).send().await {
            Ok(r) if r.status().is_success() => {}
            _ => return Some(offline()),
        }
        if i.tokens.get().is_none() {
            return Some(ClientStatus::new("not_paired", "This device is not paired with the server yet."));
        }
        let req = i.request(Method::GET, "/api/v1/whoami").ok()?.timeout(CHECK_TIMEOUT);
        match i.http_status(req).await {
            Err(_) => return Some(offline()),
            Ok(StatusCode::UNAUTHORIZED) => {
                return Some(ClientStatus::new("rejected", "Token rejected. The server no longer accepts this device; it may have been revoked."))
            }
            Ok(StatusCode::UPGRADE_REQUIRED) => {
                return Some(ClientStatus::new("update_needed", "Update needed. This version of Askbar is too old for the server."))
            }
            Ok(s) if s.is_success() => {}
            Ok(StatusCode::SERVICE_UNAVAILABLE) => return Some(ClientStatus::new("problem", SETUP_PENDING)),
            Ok(s) => return Some(ClientStatus::new("problem", format!("The server answered {s}."))),
        }
        let providers: Vec<ProviderInfo> = i.get_json("/api/v1/providers").await.ok()?;
        let wanted = i.cfg().active_provider;
        let wanted = wanted.trim();
        if providers.is_empty() {
            return Some(ClientStatus::new("problem", "The server has no providers yet. Add one on its admin page."));
        }
        if !wanted.is_empty() && !providers.iter().any(|p| p.name == wanted) {
            let mut st = ClientStatus::new("provider_missing", format!("Provider \"{wanted}\" no longer exists on the server. Pick another."));
            st.providers = providers.into_iter().map(|p| p.name).collect();
            return Some(st);
        }
        None
    }

    pub async fn set_active_provider(&self, name: String) -> Result<(), String> {
        let i = self.inner()?;
        let mut cfg = i.cfg();
        cfg.active_provider = name;
        cfg.save(&i.path)?;
        *lock(&i.cfg) = cfg;
        Ok(())
    }

    #[cfg(test)]
    pub async fn list_providers(&self) -> Result<Vec<ProviderInfo>, String> {
        self.inner()?.get_json("/api/v1/providers").await
    }

    pub async fn list_conversations(&self) -> Result<Vec<Conversation>, String> {
        self.inner()?.get_json("/api/v1/conversations").await
    }

    pub async fn load_conversation(&self, id: String) -> Result<Vec<Message>, String> {
        let d: ConversationDetail = self.inner()?.get_json(&format!("/api/v1/conversations/{id}")).await?;
        Ok(d.messages)
    }

    pub async fn delete_conversation(&self, id: String) -> Result<(), String> {
        let i = self.inner()?;
        let resp = i.request(Method::DELETE, &format!("/api/v1/conversations/{id}"))?.timeout(REQUEST_TIMEOUT);
        match i.send(resp).await {
            Ok(_) => Ok(()),
            Err(e) if e.ends_with("404 Not Found.") => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub async fn ask(&self, conversation_id: Option<String>, text: String, mut cancel: oneshot::Receiver<()>, emit: Emit<'_>) {
        let fail = |message: String| emit(StreamEvent::Error { message });
        let i = match self.inner() {
            Ok(i) => i,
            Err(e) => return fail(e),
        };
        let provider = match i.active_provider().await {
            Ok(p) => p,
            Err(e) => return fail(e),
        };
        let req = match i.request(Method::POST, "/api/v1/chat") {
            Ok(r) => r.header("accept", "text/event-stream").json(&ChatRequest { conversation_id, provider, text }),
            Err(e) => return fail(e),
        };
        let resp = tokio::select! {
            r = i.send(req) => match r { Ok(r) => r, Err(e) => return fail(e) },
            _ = &mut cancel => return emit(StreamEvent::Cancelled),
        };

        let mut stream = Box::pin(resp.bytes_stream());
        let mut parser = SseParser::default();
        loop {
            let chunk = tokio::select! {
                c = stream.next() => c,
                // Dropping the stream closes the connection; the server keeps the partial reply.
                _ = &mut cancel => return emit(StreamEvent::Cancelled),
            };
            let events = match chunk {
                None => break,
                Some(Err(e)) => return fail(format!("The connection to the server was lost: {e}")),
                Some(Ok(bytes)) => parser.feed(&bytes),
            };
            for ev in events {
                match serde_json::from_str::<ChatEvent>(&ev.data) {
                    Ok(ChatEvent::Meta { conversation_id }) => {
                        emit(StreamEvent::Started { conversation_id })
                    }
                    Ok(ChatEvent::Delta { text }) => emit(StreamEvent::Delta { text }),
                    Ok(ChatEvent::Done) => return emit(StreamEvent::Done),
                    Ok(ChatEvent::Error { message }) => return fail(message),
                    Err(_) => {} // unknown event from a newer server: ignore
                }
            }
        }
        fail("The connection to the server ended before the reply was complete.".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::MemoryTokens;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::sync::Arc;

    struct Mock(Child);
    impl Drop for Mock {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn start_mock() -> (Mock, u16) {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/mock_llm.py");
        let mut child = Command::new("python3").args(["-u", script, &port.to_string()]).stdout(Stdio::piped()).spawn().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let out = BufReader::new(child.stdout.take().unwrap());
        // Keep draining so the mock never hits a closed pipe.
        std::thread::spawn(move || {
            for line in out.lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
        rx.recv_timeout(Duration::from_secs(10)).expect("mock did not start");
        (Mock(child), port)
    }

    /// Server with one mock provider; returns its URL and a fresh device token.
    async fn start_server(mock_port: u16) -> (String, String) {
        use askbar_server::{auth, secrets::Secrets, AppState};
        let db = askbar_server::Db::open_in_memory().unwrap();
        let user = db.seed_server().unwrap();
        db.conn()
            .execute(
                "INSERT INTO providers (id, name, type, base_url, model, created_at) VALUES ('p','mock','openai',?1,'mock-model',0)",
                [format!("http://127.0.0.1:{mock_port}/v1")],
            )
            .unwrap();
        let code = auth::create_pair_code(&db, &user, "test", 600).unwrap();
        let state: Arc<AppState> = AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap());
        let token = auth::redeem_pair_code(&state.db.lock().unwrap(), &code).unwrap().unwrap();
        let app = askbar_server::api_router(state);
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (url, token)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_talks_to_a_real_server() {
        let (_mock, mock_port) = start_mock();
        let (url, token) = start_server(mock_port).await;
        let cfg = ClientConfig { server_url: url, ..Default::default() };
        let remote = Remote::new(Ok(cfg), PathBuf::new(), Box::new(MemoryTokens::with(&token)));

        let s = remote.settings().await.unwrap();
        assert_eq!((s.provider.as_str(), s.model.as_str()), ("mock", "mock-model"));
        assert_eq!(remote.list_providers().await.unwrap().len(), 1);
        assert!(remote.list_conversations().await.unwrap().is_empty());

        let events = Mutex::new(Vec::new());
        let (_tx, rx) = oneshot::channel();
        remote.ask(None, "hello".into(), rx, &|e| events.lock().unwrap().push(e)).await;
        let events = events.into_inner().unwrap();
        let StreamEvent::Started { conversation_id } = events[0].clone() else { panic!("first event must be Started") };
        assert!(matches!(events.last(), Some(StreamEvent::Done)));
        let reply: String = events.iter().filter_map(|e| if let StreamEvent::Delta { text } = e { Some(text.as_str()) } else { None }).collect();
        assert!(reply.ends_with("Anything else?"));

        let list = remote.list_conversations().await.unwrap();
        assert_eq!((list.len(), list[0].id.as_str(), list[0].message_count), (1, conversation_id.as_str(), 2));
        let msgs = remote.load_conversation(conversation_id.clone()).await.unwrap();
        assert_eq!((msgs[0].role.as_str(), msgs[0].content.as_str()), ("user", "hello"));
        assert_eq!(msgs[1].content, reply);

        // A follow-up goes into the same conversation.
        let (_tx, rx) = oneshot::channel();
        remote.ask(Some(conversation_id.clone()), "more".into(), rx, &|_| {}).await;
        assert_eq!(remote.load_conversation(conversation_id.clone()).await.unwrap().len(), 4);

        remote.delete_conversation(conversation_id).await.unwrap();
        assert!(remote.list_conversations().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn unpaired_or_misconfigured_clients_explain_themselves() {
        let cfg = ClientConfig { server_url: "http://127.0.0.1:1".into(), ..Default::default() };
        let r = Remote::new(Ok(cfg), PathBuf::new(), Box::new(MemoryTokens::default()));
        assert!(r.list_conversations().await.unwrap_err().contains("not paired"));

        let bad = ClientConfig { server_url: "http://192.168.1.5:8080".into(), ..Default::default() };
        let r = Remote::new(Ok(bad), PathBuf::new(), Box::new(MemoryTokens::default()));
        assert!(r.settings().await.unwrap_err().contains("allow_insecure"));
    }

    /// A real server with a mock-less provider list; `min_client` lets a test make the client too old.
    async fn status_server(min_client: &str) -> (String, String, Arc<askbar_server::AppState>) {
        use askbar_server::{auth, secrets::Secrets, AppState};
        let db = askbar_server::Db::open_in_memory().unwrap();
        let user = db.seed_server().unwrap();
        for (n, name) in ["alpha", "beta"].iter().enumerate() {
            db.conn()
                .execute(
                    "INSERT INTO providers (id, name, type, base_url, model, created_at) VALUES (?1,?2,'openai','http://x/v1','m',0)",
                    rusqlite::params![format!("p{n}"), name],
                )
                .unwrap();
        }
        let code = auth::create_pair_code(&db, &user, "test", 600).unwrap();
        let state = AppState::with_min_client(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), min_client);
        let token = auth::redeem_pair_code(&state.db.lock().unwrap(), &code).unwrap().unwrap();
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let app = askbar_server::api_router(state.clone());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (url, token, state)
    }

    fn remote_for(url: &str, token: Option<&str>, active: &str, path: PathBuf) -> Remote {
        let cfg = ClientConfig { server_url: url.into(), active_provider: active.into(), ..Default::default() };
        let tokens: Box<dyn crate::tokens::TokenStore> = match token {
            Some(t) => Box::new(MemoryTokens::with(t)),
            None => Box::new(MemoryTokens::default()),
        };
        Remote::new(Ok(cfg), path, tokens)
    }

    async fn kind(r: &Remote) -> Option<String> {
        r.status().await.map(|s| s.kind)
    }

    #[tokio::test]
    async fn status_reports_each_problem_with_its_kind() {
        let (url, token, state) = status_server("0.1.0").await;

        assert_eq!(kind(&remote_for(&url, Some(&token), "alpha", PathBuf::new())).await, None);
        assert_eq!(kind(&remote_for(&url, Some(&token), "", PathBuf::new())).await, None, "no active provider means the first one");
        assert_eq!(kind(&remote_for(&url, None, "alpha", PathBuf::new())).await.as_deref(), Some("not_paired"));
        assert_eq!(kind(&remote_for(&url, Some("abt_wrong"), "alpha", PathBuf::new())).await.as_deref(), Some("rejected"));

        let missing = remote_for(&url, Some(&token), "gone", PathBuf::new()).status().await.unwrap();
        assert_eq!(missing.kind, "provider_missing");
        assert!(missing.message.contains("gone") && missing.message.contains("Pick another"));
        assert_eq!(missing.providers, ["alpha", "beta"]);

        // Revoking the device turns "ok" into "rejected".
        let r = remote_for(&url, Some(&token), "alpha", PathBuf::new());
        assert_eq!(kind(&r).await, None);
        state.db.lock().unwrap().conn().execute("UPDATE devices SET revoked_at = 1", []).unwrap();
        assert_eq!(kind(&r).await.as_deref(), Some("rejected"));
    }

    #[tokio::test]
    async fn status_reports_update_needed_and_offline() {
        let (url, token, _state) = status_server("99.0.0").await;
        assert_eq!(kind(&remote_for(&url, Some(&token), "alpha", PathBuf::new())).await.as_deref(), Some("update_needed"));

        // Nothing listens here: refused immediately, well inside the 2 second budget.
        let started = std::time::Instant::now();
        let st = remote_for("http://127.0.0.1:1", Some("t"), "alpha", PathBuf::new()).status().await.unwrap();
        assert_eq!(st.kind, "offline");
        assert!(st.message.starts_with("Server offline"));
        assert!(started.elapsed() < Duration::from_secs(3));

        // A bad client.yaml is reported, not a crash.
        let broken = Remote::new(Err("Could not parse client.yaml".into()), PathBuf::new(), Box::new(MemoryTokens::default()));
        assert_eq!(broken.status().await.unwrap().kind, "problem");
    }

    #[tokio::test]
    async fn no_reachable_server_shows_server_offline() {
        // Paired or not, an unreachable server is reported as offline before anything else.
        for token in [Some("t"), None] {
            let r = remote_for("http://127.0.0.1:1", token, "alpha", PathBuf::new());
            let st = r.status().await.unwrap();
            assert_eq!(st.kind, "offline", "token: {token:?}");
            assert!(st.message.starts_with("Server offline. Could not reach http://127.0.0.1:1"), "{}", st.message);

            // The window still opens with the built-in look and the saved hotkey.
            let settings = r.settings().await.unwrap();
            assert_eq!(settings.hotkey, ClientConfig::default().hotkey);
            assert_eq!(settings.ui.width, crate::client_config::Ui::default().width);
            assert!(settings.model.is_empty());
        }
        // Asking for history fails with a message instead of hanging.
        let r = remote_for("http://127.0.0.1:1", Some("t"), "alpha", PathBuf::new());
        assert!(r.list_conversations().await.is_err());
    }

    #[tokio::test]
    async fn picking_another_provider_is_saved_and_clears_the_state() {
        let (url, token, _state) = status_server("0.1.0").await;
        let dir = std::env::temp_dir().join(format!("askbar-pick-{}", std::process::id()));
        let path = dir.join("client.yaml");
        let r = remote_for(&url, Some(&token), "gone", path.clone());
        assert_eq!(kind(&r).await.as_deref(), Some("provider_missing"));
        r.set_active_provider("beta".into()).await.unwrap();
        assert_eq!(kind(&r).await, None);
        let saved = ClientConfig::load(&path).unwrap();
        assert_eq!((saved.active_provider.as_str(), saved.server_url.as_str()), ("beta", url.as_str()));
        assert!(!std::fs::read_to_string(&path).unwrap().contains("token"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn new_pairing(state: &Arc<askbar_server::AppState>, url: &str) -> String {
        let db = state.db.lock().unwrap();
        let user: String = db.conn().query_row("SELECT id FROM users", [], |r| r.get(0)).unwrap();
        let code = askbar_server::auth::create_pair_code(&db, &user, "second profile", 600).unwrap();
        askbar_proto::encode_pairing(url, &code)
    }

    #[tokio::test]
    async fn pairing_stores_the_token_and_the_server_and_keeps_other_settings() {
        let (url, _first_token, state) = status_server("0.1.0").await;
        let dir = std::env::temp_dir().join(format!("askbar-pair-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("profile2").join("client.yaml");
        let tokens = MemoryTokens::default();

        let s = new_pairing(&state, &url);
        pair(&s, &path, &tokens).await.unwrap();
        let token = tokens.get().expect("token stored");
        let yaml = std::fs::read_to_string(&path).unwrap();
        assert!(yaml.contains(&url) && !yaml.contains(&token) && !yaml.to_lowercase().contains("token"));
        let cfg = ClientConfig::load(&path).unwrap();
        assert_eq!(cfg.server_url, url);

        // The new profile works end to end, and the code can't be used twice.
        let r = Remote::new(Ok(cfg), path.clone(), Box::new(MemoryTokens::with(&token)));
        assert_eq!(kind(&r).await, None);
        assert!(r.list_providers().await.unwrap().len() == 2);
        assert!(pair(&s, &path, &MemoryTokens::default()).await.unwrap_err().contains("already used"));

        // Re-pairing updates the server but keeps hand-edited settings.
        std::fs::write(&path, format!("server_url: http://old.invalid\nhotkey: Alt+Space\nactive_provider: beta\nallow_insecure: true\n")).unwrap();
        pair(&new_pairing(&state, &url), &path, &tokens).await.unwrap();
        let cfg = ClientConfig::load(&path).unwrap();
        assert_eq!((cfg.server_url.as_str(), cfg.hotkey.as_str(), cfg.active_provider.as_str(), cfg.allow_insecure), (url.as_str(), "Alt+Space", "beta", true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn bad_pairing_attempts_explain_themselves_and_store_nothing() {
        let dir = std::env::temp_dir().join(format!("askbar-pair-bad-{}", std::process::id()));
        let path = dir.join("client.yaml");
        let tokens = MemoryTokens::default();
        let err = |r: Result<(), String>| r.unwrap_err();

        assert!(err(pair("hello", &path, &tokens).await).contains("askbar-pair:"));
        let lan = askbar_proto::encode_pairing("http://192.168.1.5:8080", "c");
        assert!(err(pair(&lan, &path, &tokens).await).contains("allow_insecure"));
        let gone = askbar_proto::encode_pairing("http://127.0.0.1:1", "c");
        assert!(err(pair(&gone, &path, &tokens).await).contains("Could not reach"));

        let (url, _t, _state) = status_server("0.1.0").await;
        let wrong = askbar_proto::encode_pairing(&url, "not-a-real-code");
        assert!(err(pair(&wrong, &path, &tokens).await).contains("wrong, expired"));
        let (old_url, _t, _state) = status_server("99.0.0").await;
        let too_old = askbar_proto::encode_pairing(&old_url, "x");
        assert!(err(pair(&too_old, &path, &tokens).await).contains("too old"));

        assert!(tokens.get().is_none() && !path.exists());
    }

    /// Pair with a string made by the admin site, revoke the device there, and the client says "rejected".
    #[tokio::test]
    async fn revoking_a_device_in_the_admin_site_shows_token_rejected() {
        use askbar_server::{admin, secrets::Secrets, AppState};
        let db = askbar_server::Db::open_in_memory().unwrap();
        db.seed_server().unwrap();
        admin::init_password(&db, "pw-for-test").unwrap();
        db.conn()
            .execute("INSERT INTO providers (id, name, type, base_url, model, created_at) VALUES ('p','alpha','openai','http://x/v1','m',0)", [])
            .unwrap();
        let listen = || async {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", l.local_addr().unwrap());
            (l, url)
        };
        let (api_l, api_url) = listen().await;
        let (admin_l, admin_url) = listen().await;
        let state = AppState::with_config(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), "0.1.0", &api_url);
        let (a, b) = (askbar_server::api_router(state.clone()), askbar_server::admin_router(state));
        tokio::spawn(async move { axum::serve(api_l, a).await.unwrap() });
        tokio::spawn(async move { axum::serve(admin_l, b).await.unwrap() });

        let http = reqwest::Client::new();
        let login = http.post(format!("{admin_url}/admin/login")).header("x-askbar-admin", "1").json(&serde_json::json!({ "password": "pw-for-test" })).send().await.unwrap();
        let cookie = login.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
        let admin_call = |method: Method, path: String, body: serde_json::Value| {
            http.request(method, format!("{admin_url}{path}")).header("x-askbar-admin", "1").header("cookie", &cookie).json(&body).send()
        };

        let out: serde_json::Value = admin_call(Method::POST, "/admin/api/pair".into(), serde_json::json!({ "name": "test client" })).await.unwrap().json().await.unwrap();
        let dir = std::env::temp_dir().join(format!("askbar-revoke-{}", std::process::id()));
        let path = dir.join("client.yaml");
        let tokens = MemoryTokens::default();
        pair(out["pairing"].as_str().unwrap(), &path, &tokens).await.unwrap();

        let r = Remote::new(ClientConfig::load(&path), path.clone(), Box::new(MemoryTokens::with(&tokens.get().unwrap())));
        assert_eq!(kind(&r).await, None);

        let devices: serde_json::Value = admin_call(Method::GET, "/admin/api/devices".into(), serde_json::json!({})).await.unwrap().json().await.unwrap();
        assert_eq!(devices[0]["name"], "test client");
        let id = devices[0]["id"].as_str().unwrap();
        let st = admin_call(Method::POST, format!("/admin/api/devices/{id}/revoke"), serde_json::json!({})).await.unwrap().status();
        assert_eq!(st, 204);

        let status = r.status().await.unwrap();
        assert_eq!(status.kind, "rejected");
        assert!(status.message.starts_with("Token rejected"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Manual end-to-end check against a running server, using the real keychain
    /// and the same `client.yaml` path logic as the app. Run with:
    ///   ASKBAR_E2E_PAIRING='askbar-pair:…' ASKBAR_CLIENT_CONFIG=/tmp/profile2/client.yaml \
    ///     cargo test -p askbar e2e -- --ignored --nocapture
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "needs a running server and a keychain"]
    async fn e2e_pair_and_ask_against_a_running_server() {
        let pairing = std::env::var("ASKBAR_E2E_PAIRING").expect("set ASKBAR_E2E_PAIRING");
        let path = crate::client_config::path();
        pair(&pairing, &path, &crate::tokens::Keychain::new(&path)).await.unwrap();

        // A fresh Remote, as the app builds it at start-up.
        let remote = Remote::new(ClientConfig::load(&path), path.clone(), Box::new(crate::tokens::Keychain::new(&path)));
        assert_eq!(remote.status().await, None);
        let events = Mutex::new(Vec::new());
        let (_tx, rx) = oneshot::channel();
        remote.ask(None, "hello from the second profile".into(), rx, &|e| events.lock().unwrap().push(e)).await;
        let events = events.into_inner().unwrap();
        assert!(matches!(events.last(), Some(StreamEvent::Done)), "{}", serde_json::to_string(&events.last()).unwrap());
        let reply: String = events.iter().filter_map(|e| if let StreamEvent::Delta { text } = e { Some(text.as_str()) } else { None }).collect();
        println!("reply: {}", reply.lines().next().unwrap_or(""));
        assert!(!reply.is_empty());
    }
}
