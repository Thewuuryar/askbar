//! Askbar server. Two separate routers: the device API and the admin site.

use askbar_proto::Health;
use axum::{extract::Request, middleware, middleware::Next, response::Response, routing::get, Json, Router};

pub const API_VERSION: u32 = 1;
pub const MIN_CLIENT: &str = "0.1.0";

pub fn api_router() -> Router {
    Router::new().route("/api/v1/health", get(health)).layer(middleware::from_fn(log_requests))
}

pub fn admin_router() -> Router {
    Router::new().route("/admin/health", get(admin_health)).layer(middleware::from_fn(log_requests))
}

async fn health() -> Json<Health> {
    Json(Health { api_version: API_VERSION, min_client: MIN_CLIENT.into() })
}

async fn admin_health() -> &'static str {
    "ok"
}

/// Logs method, path and status only: no query string, headers or bodies.
async fn log_requests(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let res = next.run(req).await;
    tracing::info!(%method, %path, status = res.status().as_u16(), "request");
    res
}
