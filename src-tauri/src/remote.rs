//! `Remote`: the backend that talks to an Askbar server over HTTPS.

use crate::backend::{Backend, Emit, Settings, StreamEvent};
use crate::client_config::{merge_ui, ClientConfig};
use crate::tokens::TokenStore;
use askbar_core::{ConversationSummary, SseParser, StoredMessage};
use askbar_proto::{ChatEvent, ChatRequest, Conversation, ConversationDetail, ProviderInfo};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{Method, RequestBuilder, StatusCode, Url};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::oneshot;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

struct Inner {
    cfg: ClientConfig,
    base: Url,
    http: reqwest::Client,
    tokens: Box<dyn TokenStore>,
    /// The UI addresses conversations by number; the server uses ULIDs.
    /// Position + 1 in this list is the number, valid for this run.
    ids: Mutex<Vec<String>>,
}

pub struct Remote {
    inner: Result<Inner, String>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Remote {
    /// A bad `client.yaml` doesn't stop the app; every call reports the problem instead.
    pub fn new(cfg: Result<ClientConfig, String>, tokens: Box<dyn TokenStore>) -> Remote {
        let inner = cfg.and_then(|cfg| {
            let base = cfg.server()?;
            let http = reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .user_agent(format!("askbar/{VERSION}"))
                .build()
                .map_err(|e| e.to_string())?;
            Ok(Inner { cfg, base, http, tokens, ids: Mutex::new(Vec::new()) })
        });
        Remote { inner }
    }

    fn inner(&self) -> Result<&Inner, String> {
        self.inner.as_ref().map_err(Clone::clone)
    }
}

impl Inner {
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

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let resp = self.send(self.request(Method::GET, path)?.timeout(REQUEST_TIMEOUT)).await?;
        resp.json().await.map_err(|e| format!("The server sent an unexpected reply: {e}"))
    }

    fn id_for(&self, ulid: &str) -> i64 {
        let mut ids = lock(&self.ids);
        let pos = ids.iter().position(|x| x == ulid).unwrap_or_else(|| {
            ids.push(ulid.to_string());
            ids.len() - 1
        });
        pos as i64 + 1
    }

    fn ulid_for(&self, id: i64) -> Option<String> {
        let ids = lock(&self.ids);
        usize::try_from(id - 1).ok().and_then(|i| ids.get(i).cloned())
    }

    async fn active_provider(&self) -> Result<String, String> {
        if !self.cfg.active_provider.trim().is_empty() {
            return Ok(self.cfg.active_provider.trim().to_string());
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
        s => Err(format!("The server answered {s}.")),
    }
}

#[async_trait]
impl Backend for Remote {
    async fn settings(&self) -> Result<Settings, String> {
        let i = self.inner()?;
        let server: askbar_proto::Settings = i.get_json("/api/v1/settings").await?;
        let providers: Vec<ProviderInfo> = i.get_json("/api/v1/providers").await?;
        let wanted = i.cfg.active_provider.trim();
        let active = providers.iter().find(|p| wanted.is_empty() || p.name == wanted);
        Ok(Settings {
            ui: merge_ui(&server.display, &i.cfg.ui),
            hotkey: i.cfg.hotkey.clone(),
            provider: active.map(|p| p.name.clone()).unwrap_or_else(|| wanted.to_string()),
            model: active.map(|p| p.model.clone()).unwrap_or_default(),
        })
    }

    async fn list_providers(&self) -> Result<Vec<ProviderInfo>, String> {
        self.inner()?.get_json("/api/v1/providers").await
    }

    async fn list_conversations(&self) -> Result<Vec<ConversationSummary>, String> {
        let i = self.inner()?;
        let list: Vec<Conversation> = i.get_json("/api/v1/conversations").await?;
        Ok(list
            .into_iter()
            .map(|c| ConversationSummary { id: i.id_for(&c.id), title: c.title, updated_at: c.updated_at, message_count: c.message_count })
            .collect())
    }

    async fn load_conversation(&self, id: i64) -> Result<Vec<StoredMessage>, String> {
        let i = self.inner()?;
        let ulid = i.ulid_for(id).ok_or("That conversation is no longer available.")?;
        let d: ConversationDetail = i.get_json(&format!("/api/v1/conversations/{ulid}")).await?;
        Ok(d.messages
            .into_iter()
            .enumerate()
            .map(|(n, m)| StoredMessage { id: n as i64 + 1, role: m.role, content: m.content, created_at: m.created_at })
            .collect())
    }

    async fn delete_conversation(&self, id: i64) -> Result<(), String> {
        let i = self.inner()?;
        let Some(ulid) = i.ulid_for(id) else { return Ok(()) };
        let resp = i.request(Method::DELETE, &format!("/api/v1/conversations/{ulid}"))?.timeout(REQUEST_TIMEOUT);
        match i.send(resp).await {
            Ok(_) => Ok(()),
            Err(e) if e.ends_with("404 Not Found.") => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn ask(&self, conversation_id: Option<i64>, text: String, mut cancel: oneshot::Receiver<()>, emit: Emit<'_>) {
        let fail = |message: String| emit(StreamEvent::Error { message });
        let i = match self.inner() {
            Ok(i) => i,
            Err(e) => return fail(e),
        };
        let provider = match i.active_provider().await {
            Ok(p) => p,
            Err(e) => return fail(e),
        };
        let conv = match conversation_id {
            Some(id) => match i.ulid_for(id) {
                Some(u) => Some(u),
                None => return fail("That conversation is no longer available.".into()),
            },
            None => None,
        };
        let req = match i.request(Method::POST, "/api/v1/chat") {
            Ok(r) => r.header("accept", "text/event-stream").json(&ChatRequest { conversation_id: conv, provider, text }),
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
                        emit(StreamEvent::Started { conversation_id: i.id_for(&conversation_id) })
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
        let db = askbar_core::Db::open_in_memory().unwrap();
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
        let remote = Remote::new(Ok(cfg), Box::new(MemoryTokens::with(&token)));

        let s = remote.settings().await.unwrap();
        assert_eq!((s.provider.as_str(), s.model.as_str()), ("mock", "mock-model"));
        assert_eq!(remote.list_providers().await.unwrap().len(), 1);
        assert!(remote.list_conversations().await.unwrap().is_empty());

        let events = Mutex::new(Vec::new());
        let (_tx, rx) = oneshot::channel();
        remote.ask(None, "hello".into(), rx, &|e| events.lock().unwrap().push(e)).await;
        let events = events.into_inner().unwrap();
        let StreamEvent::Started { conversation_id } = events[0] else { panic!("first event must be Started") };
        assert!(matches!(events.last(), Some(StreamEvent::Done)));
        let reply: String = events.iter().filter_map(|e| if let StreamEvent::Delta { text } = e { Some(text.as_str()) } else { None }).collect();
        assert!(reply.ends_with("Anything else?"));

        let list = remote.list_conversations().await.unwrap();
        assert_eq!((list.len(), list[0].id, list[0].message_count), (1, conversation_id, 2));
        let msgs = remote.load_conversation(conversation_id).await.unwrap();
        assert_eq!((msgs[0].role.as_str(), msgs[0].content.as_str()), ("user", "hello"));
        assert_eq!(msgs[1].content, reply);

        // A follow-up goes into the same conversation.
        let (_tx, rx) = oneshot::channel();
        remote.ask(Some(conversation_id), "more".into(), rx, &|_| {}).await;
        assert_eq!(remote.load_conversation(conversation_id).await.unwrap().len(), 4);

        remote.delete_conversation(conversation_id).await.unwrap();
        assert!(remote.list_conversations().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn unpaired_or_misconfigured_clients_explain_themselves() {
        let cfg = ClientConfig { server_url: "http://127.0.0.1:1".into(), ..Default::default() };
        let r = Remote::new(Ok(cfg), Box::new(MemoryTokens::default()));
        assert!(r.list_conversations().await.unwrap_err().contains("not paired"));

        let bad = ClientConfig { server_url: "http://192.168.1.5:8080".into(), ..Default::default() };
        let r = Remote::new(Ok(bad), Box::new(MemoryTokens::default()));
        assert!(r.settings().await.unwrap_err().contains("allow_insecure"));
    }
}
