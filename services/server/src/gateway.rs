//! The server's link to `model-gateway` (ADR 0054): `POST /chat` with the user, space, conversation and generation ids,
//! the provider, the system prompt and the history, and a stream of events back. The server has no provider keys and
//! no route to a provider; every model call goes through here.
//!
//! The gateway records the turn span in the auditor before it calls a provider and says so in the first event
//! (`span`). The server adds its sealed payloads and the turn's finish record to that span. Dropping the stream cancels
//! the request in flight.

use askbar_proto::gateway::{ChatEvent, ChatRequest};
use askbar_proto::{Problem, SseParser};
use futures_util::{Stream, StreamExt};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// The link the server's token for the gateway belongs to, as `bootstrap` names it.
pub const LINK: &str = "server-model-gateway";

/// Why the gateway took no request. A request it took ends in an `error` event instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayError {
    /// The gateway could not make the audit record in time, so it called no provider (`503 audit_backlog`).
    AuditBacklog,
    /// The gateway has no configuration yet (it just started, and the admin has not given it its keys).
    NotConfigured,
    /// The gateway refused: the kill switch is on there (the authoritative copy).
    KillSwitch,
    /// The gateway refused: this user is paused there.
    UserPaused,
    /// The gateway refused: a token budget is used up for its period (`daily` or `monthly`).
    BudgetExceeded(String),
    /// The gateway refused: the model is not in its catalog (the authoritative copy).
    ModelNotApproved,
    /// The gateway has no such provider (a key was never given to it for one that exists here).
    NoSuchProvider,
    /// The gateway does not accept this server's token, or this server could not read it.
    Rejected,
    /// Nothing answered.
    Unreachable,
    /// It answered something this client does not understand.
    Other(u16),
}

/// How long the catalog the gateway gave is used before it is asked again. The gateway checks every send itself, so a
/// change the administrator makes reaches the server's own check, and the clients' list, within this time.
const CATALOG_TTL: Duration = Duration::from_secs(5);

pub struct GatewayClient {
    http: reqwest::Client,
    base: String,
    token_dir: PathBuf,
    catalog: std::sync::Mutex<Option<(std::time::Instant, askbar_proto::ModelList)>>,
}

impl GatewayClient {
    pub fn new(base: &str, token_dir: PathBuf) -> Arc<GatewayClient> {
        Arc::new(GatewayClient {
            // No overall timeout: a reply streams for as long as the generation's own time limit allows.
            http: reqwest::Client::builder().connect_timeout(Duration::from_secs(3)).build().expect("reqwest client"),
            base: base.trim_end_matches('/').to_string(),
            token_dir,
            catalog: std::sync::Mutex::new(None),
        })
    }

    /// The approved models and the default, as the gateway serves them (ADR 0014), kept for `CATALOG_TTL`.
    pub async fn catalog(&self) -> Result<askbar_proto::ModelList, GatewayError> {
        if let Some((at, list)) = self.catalog.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            if at.elapsed() < CATALOG_TTL {
                return Ok(list.clone());
            }
        }
        let token = self.token().ok_or(GatewayError::Rejected)?;
        let url = format!("{}{}/catalog", self.base, askbar_proto::paths::MODEL_GATEWAY_V1);
        let resp =
            self.http.get(url).bearer_auth(token).timeout(Duration::from_secs(5)).send().await.map_err(|_| GatewayError::Unreachable)?;
        let status = resp.status().as_u16();
        if status != 200 {
            let code = resp.json::<Problem>().await.ok().and_then(|p| p.code());
            return Err(match (status, code) {
                (503, Some(askbar_proto::ErrorCode::NotConfigured)) => GatewayError::NotConfigured,
                (401 | 403, _) => GatewayError::Rejected,
                (other, _) => GatewayError::Other(other),
            });
        }
        let list: askbar_proto::ModelList = resp.json().await.map_err(|_| GatewayError::Other(status))?;
        *self.catalog.lock().unwrap_or_else(|e| e.into_inner()) = Some((std::time::Instant::now(), list.clone()));
        Ok(list)
    }

    /// Forget the catalog held, so the next question asks the gateway (it said a model is not approved).
    pub fn forget_catalog(&self) {
        *self.catalog.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn token(&self) -> Option<String> {
        let t = std::fs::read_to_string(self.token_dir.join("call").join(format!("{LINK}.token"))).ok()?;
        Some(t.trim().to_string()).filter(|t| !t.is_empty())
    }

    /// Start one generation. On success, the events, first `span`. The stream ends after `done` or `error` (or when the
    /// connection breaks, which the caller treats as a failure).
    pub async fn chat(&self, req: &ChatRequest) -> Result<impl Stream<Item = ChatEvent> + Send + Unpin, GatewayError> {
        let token = self.token().ok_or(GatewayError::Rejected)?;
        let url = format!("{}{}/chat", self.base, askbar_proto::paths::MODEL_GATEWAY_V1);
        let resp = self
            .http
            .post(url)
            .bearer_auth(token)
            .header("accept", "text/event-stream")
            .json(req)
            .send()
            .await
            .map_err(|_| GatewayError::Unreachable)?;
        let status = resp.status().as_u16();
        if status != 200 {
            let problem: Option<Problem> = resp.json().await.ok();
            let period = problem.as_ref().and_then(|p| p.extra.get("period")).and_then(|v| v.as_str()).unwrap_or("daily").to_string();
            return Err(match (status, problem.and_then(|p| p.code())) {
                (503, Some(askbar_proto::ErrorCode::KillSwitch)) => GatewayError::KillSwitch,
                (403, Some(askbar_proto::ErrorCode::UserPaused)) => GatewayError::UserPaused,
                (400, Some(askbar_proto::ErrorCode::ModelNotApproved)) => GatewayError::ModelNotApproved,
                (429, Some(askbar_proto::ErrorCode::BudgetExceeded)) => GatewayError::BudgetExceeded(period),
                (503, Some(askbar_proto::ErrorCode::AuditBacklog)) => GatewayError::AuditBacklog,
                (503, Some(askbar_proto::ErrorCode::NotConfigured)) => GatewayError::NotConfigured,
                (404, _) => GatewayError::NoSuchProvider,
                (401 | 403, _) => GatewayError::Rejected,
                (other, _) => GatewayError::Other(other),
            });
        }
        let bytes = Box::pin(resp.bytes_stream());
        Ok(Box::pin(futures_util::stream::unfold(
            (bytes, SseParser::default(), VecDeque::<ChatEvent>::new(), false),
            |(mut bytes, mut parser, mut ready, mut ended)| async move {
                loop {
                    if let Some(e) = ready.pop_front() {
                        return Some((e, (bytes, parser, ready, ended)));
                    }
                    if ended {
                        return None;
                    }
                    match bytes.next().await {
                        Some(Ok(chunk)) => {
                            for ev in parser.feed(&chunk) {
                                // A keep-alive comment is not an event; anything that is not one of ours is skipped.
                                if let Ok(e) = serde_json::from_str::<ChatEvent>(&ev.data) {
                                    ready.push_back(e);
                                }
                            }
                        }
                        Some(Err(_)) | None => {
                            for ev in parser.finish() {
                                if let Ok(e) = serde_json::from_str::<ChatEvent>(&ev.data) {
                                    ready.push_back(e);
                                }
                            }
                            ended = true;
                        }
                    }
                }
            },
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use askbar_proto::gateway::{ChatMessage, FailureClass};
    use axum::response::IntoResponse;
    use axum::routing::post;

    fn request() -> ChatRequest {
        ChatRequest {
            provider_id: "p".into(),
            model: "m".into(),
            user_id: "u".into(),
            client_id: None,
            space_id: None,
            conversation_id: None,
            generation_id: "g".into(),
            system: String::new(),
            messages: vec![ChatMessage { role: "user".into(), content: "hi".into() }],
        }
    }

    async fn client_for(app: axum::Router) -> Arc<GatewayClient> {
        let dir = std::env::temp_dir().join(format!("gw-client-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(dir.join("call")).unwrap();
        std::fs::write(dir.join("call").join(format!("{LINK}.token")), "srvgw_test\n").unwrap();
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        GatewayClient::new(&url, dir)
    }

    #[tokio::test]
    async fn events_come_back_in_order_with_the_token_sent_and_a_comment_skipped() {
        let app = axum::Router::new().route(
            "/api/model-gateway/v1/chat",
            post(|h: axum::http::HeaderMap| async move {
                assert_eq!(h["authorization"], "Bearer srvgw_test");
                let body = ": keep-alive\n\nevent: span\ndata: {\"event\":\"span\",\"span_id\":\"s\",\"trace_id\":\"g\"}\n\nevent: delta\ndata: {\"event\":\"delta\",\"text\":\"hi\"}\n\nevent: done\ndata: {\"event\":\"done\",\"finish_reason\":null,\"ttft_ms\":1,\"attempts\":1}\n\n";
                ([("content-type", "text/event-stream")], body).into_response()
            }),
        );
        let c = client_for(app).await;
        let events: Vec<ChatEvent> = c.chat(&request()).await.unwrap().collect().await;
        assert_eq!(events.len(), 3, "{events:?}");
        assert!(matches!(&events[0], ChatEvent::Span { span_id, .. } if span_id == "s"));
        assert!(matches!(&events[1], ChatEvent::Delta { text } if text == "hi"));
        assert!(matches!(&events[2], ChatEvent::Done { attempts: 1, .. }));
        let _ = FailureClass::Fatal;
    }

    #[tokio::test]
    async fn the_gateways_refusals_have_their_own_errors() {
        use askbar_proto::http::ApiError;
        use askbar_proto::ErrorCode;
        let app = axum::Router::new()
            .route("/api/model-gateway/v1/chat", post(|| async { ApiError::new(ErrorCode::AuditBacklog, "busy").retry_after(5) }));
        assert_eq!(client_for(app).await.chat(&request()).await.err(), Some(GatewayError::AuditBacklog));
        let app =
            axum::Router::new().route("/api/model-gateway/v1/chat", post(|| async { ApiError::new(ErrorCode::NotConfigured, "wait") }));
        assert_eq!(client_for(app).await.chat(&request()).await.err(), Some(GatewayError::NotConfigured));
        let app = axum::Router::new().route("/api/model-gateway/v1/chat", post(|| async { ApiError::new(ErrorCode::NotFound, "no") }));
        assert_eq!(client_for(app).await.chat(&request()).await.err(), Some(GatewayError::NoSuchProvider));
        let app = axum::Router::new().route("/api/model-gateway/v1/chat", post(|| async { ApiError::new(ErrorCode::Unauthorized, "no") }));
        assert_eq!(client_for(app).await.chat(&request()).await.err(), Some(GatewayError::Rejected));
        // The gateway's own copy of the stops and the budget (plan step 3.2).
        let app = axum::Router::new().route("/api/model-gateway/v1/chat", post(|| async { ApiError::new(ErrorCode::KillSwitch, "off") }));
        assert_eq!(client_for(app).await.chat(&request()).await.err(), Some(GatewayError::KillSwitch));
        let app =
            axum::Router::new().route("/api/model-gateway/v1/chat", post(|| async { ApiError::new(ErrorCode::UserPaused, "paused") }));
        assert_eq!(client_for(app).await.chat(&request()).await.err(), Some(GatewayError::UserPaused));
        let app = axum::Router::new().route(
            "/api/model-gateway/v1/chat",
            post(|| async { ApiError::new(ErrorCode::BudgetExceeded, "used up").with("period", "monthly") }),
        );
        assert_eq!(client_for(app).await.chat(&request()).await.err(), Some(GatewayError::BudgetExceeded("monthly".into())));
        let dead = GatewayClient::new("http://127.0.0.1:1", std::env::temp_dir());
        assert_eq!(dead.chat(&request()).await.err(), Some(GatewayError::Rejected), "no token file here");
    }
}
