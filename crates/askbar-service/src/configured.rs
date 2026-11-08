//! `503 not_configured` until the admin has pushed configuration (ADR 0028 rule 3).

use askbar_proto::http::ApiError;
use askbar_proto::problem::ErrorCode;
use axum::{extract::Request, middleware::Next, response::Response};
use std::sync::Arc;

/// The gate: `is_configured` says whether configuration has arrived; `open` lists the paths that
/// answer anyway (health checks, and paths that only tell an old client to update).
#[derive(Clone)]
pub struct Gate {
    is_configured: Arc<dyn Fn() -> bool + Send + Sync>,
    open: Arc<dyn Fn(&str) -> bool + Send + Sync>,
}

impl Gate {
    pub fn new(is_configured: impl Fn() -> bool + Send + Sync + 'static, open: impl Fn(&str) -> bool + Send + Sync + 'static) -> Gate {
        Gate { is_configured: Arc::new(is_configured), open: Arc::new(open) }
    }
}

/// The middleware: `axum::middleware::from_fn_with_state(gate, configured::require)`.
pub async fn require(axum::extract::State(gate): axum::extract::State<Gate>, req: Request, next: Next) -> Result<Response, ApiError> {
    if (gate.open)(req.uri().path()) || (gate.is_configured)() {
        return Ok(next.run(req).await);
    }
    Err(ApiError::new(
        ErrorCode::NotConfigured,
        "The server is starting and has not received its settings yet. Try again in a few seconds.",
    )
    .retry_after(5))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn closed_until_configured_except_the_open_paths() {
        let ready = Arc::new(AtomicBool::new(false));
        let r = ready.clone();
        let gate = Gate::new(move || r.load(Ordering::Relaxed), |p| p == "/health");
        let app = axum::Router::new()
            .route("/health", axum::routing::get(|| async { "ok" }))
            .route("/work", axum::routing::get(|| async { "done" }))
            .layer(axum::middleware::from_fn_with_state(gate, require));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let get = |p: &'static str| {
            let url = url.clone();
            async move { reqwest::get(format!("{url}{p}")).await.unwrap() }
        };
        assert_eq!(get("/health").await.status(), 200);
        let r = get("/work").await;
        assert_eq!(r.status(), 503);
        assert_eq!(r.headers()["retry-after"], "5");
        let p: askbar_proto::Problem = r.json().await.unwrap();
        assert_eq!(p.error, "not_configured");
        ready.store(true, Ordering::Relaxed);
        assert_eq!(get("/work").await.status(), 200);
    }
}
