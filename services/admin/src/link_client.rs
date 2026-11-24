//! What the alerter and gateway links share: the HTTP client, the token, the online state and the alerts about the
//! link. Each link keeps its own `sync` and `run`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

pub struct LinkClient {
    pub(crate) http: reqwest::Client,
    pub(crate) base: String,
    pub(crate) token: String,
    /// The service's API prefix, like `/api/alerter/v1`.
    pub(crate) prefix: &'static str,
    pub(crate) online: AtomicBool,
    last_error: Mutex<Option<String>>,
    pub(crate) dirty: Notify,
    token_watch: crate::notify::TokenWatch,
    pub(crate) restarts: crate::notify::RestartWatch,
}

impl LinkClient {
    /// `link` names the token for `TokenWatch`, `service` names the service for `RestartWatch`.
    pub fn new(
        base: &str,
        token: String,
        link: &'static str,
        service: &'static str,
        prefix: &'static str,
        timeout: Duration,
    ) -> LinkClient {
        LinkClient {
            http: reqwest::Client::builder().connect_timeout(Duration::from_secs(2)).timeout(timeout).build().expect("reqwest client"),
            base: base.trim_end_matches('/').to_string(),
            token,
            prefix,
            online: AtomicBool::new(false),
            last_error: Mutex::new(None),
            dirty: Notify::new(),
            token_watch: crate::notify::TokenWatch::new(link),
            restarts: crate::notify::RestartWatch::new(service),
        }
    }

    /// Where an alert about this link's token goes (set once by `main`).
    pub fn set_notifier(&self, n: Arc<crate::notify::Notifier>) {
        self.token_watch.set_notifier(n.clone());
        self.restarts.set_notifier(n);
    }

    pub fn online(&self) -> bool {
        self.online.load(Ordering::Relaxed)
    }

    /// A sentence about the last problem, for the Services page; never anything the service said.
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_error(&self, what: Option<String>) {
        *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = what;
    }

    /// Check the service now instead of at the next round: something it runs has changed.
    pub fn mark_dirty(&self) {
        self.dirty.notify_one();
    }

    pub async fn request(&self, method: reqwest::Method, path: &str, body: Option<&serde_json::Value>) -> Result<(u16, Vec<u8>), ()> {
        let url = format!("{}{}{path}", self.base, self.prefix);
        let mut req = self.http.request(method, url).bearer_auth(&self.token);
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await.map_err(|_| ())?;
        let status = resp.status().as_u16();
        self.token_watch.saw(status);
        let bytes = resp.bytes().await.map_err(|_| ())?;
        Ok((status, bytes.to_vec()))
    }
}
