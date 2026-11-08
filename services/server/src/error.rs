//! Every error the server's HTTP APIs answer with: `application/problem+json` (RFC 9457).
//! The type lives in `askbar_proto::http` (the admin uses it too); the tests that need a real
//! listener live here.

pub use askbar_proto::http::*;

#[cfg(test)]
mod tests {
    use super::*;
    use askbar_proto::problem::{ErrorCode, Problem, CONTENT_TYPE};
    use axum::http::{header, StatusCode};
    use axum::{routing::post, Router};

    async fn serve() -> String {
        async fn echo(ApiJson(v): ApiJson<serde_json::Value>) -> ApiJson<serde_json::Value> {
            ApiJson(v)
        }
        async fn busy() -> ApiError {
            ApiError::code(ErrorCode::AuditBacklog)
        }
        async fn limit() -> ApiError {
            ApiError::code(ErrorCode::BudgetExceeded).with("period", "monthly")
        }
        async fn denied() -> ApiError {
            StatusCode::UNAUTHORIZED.into()
        }
        let app = Router::new()
            .route("/echo", post(echo))
            .route("/busy", axum::routing::get(busy))
            .route("/limit", axum::routing::get(limit))
            .route("/denied", axum::routing::get(denied))
            .fallback(not_found)
            .method_not_allowed_fallback(method_not_allowed);
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        url
    }

    async fn problem(r: reqwest::Response) -> (u16, Problem, reqwest::header::HeaderMap) {
        let status = r.status().as_u16();
        let headers = r.headers().clone();
        assert_eq!(headers[header::CONTENT_TYPE], CONTENT_TYPE, "every error is problem+json");
        (status, r.json().await.unwrap(), headers)
    }

    #[tokio::test]
    async fn unknown_paths_wrong_methods_and_bad_json_are_problems() {
        let url = serve().await;
        let http = reqwest::Client::new();
        let (s, p, _) = problem(http.get(format!("{url}/nope")).send().await.unwrap()).await;
        assert_eq!((s, p.error.as_str(), p.status), (404, "not_found", 404));
        let (s, p, _) = problem(http.get(format!("{url}/echo")).send().await.unwrap()).await;
        assert_eq!((s, p.error.as_str()), (405, "method_not_allowed"));
        let (s, p, _) =
            problem(http.post(format!("{url}/echo")).header("content-type", "application/json").body("{not json").send().await.unwrap())
                .await;
        assert_eq!((s, p.error.as_str()), (400, "bad_request"));
        assert!(!p.detail.is_empty());
        let (s, p, _) = problem(http.post(format!("{url}/echo")).body("{}").send().await.unwrap()).await;
        assert_eq!((s, p.error.as_str()), (400, "bad_request"), "a missing content type is a bad request, not plain text");
        let ok = http.post(format!("{url}/echo")).json(&serde_json::json!({"a": 1})).send().await.unwrap();
        assert_eq!(ok.status(), 200);
    }

    #[tokio::test]
    async fn refusals_keep_their_status_extension_members_and_headers() {
        let url = serve().await;
        let http = reqwest::Client::new();
        let (s, p, h) = problem(http.get(format!("{url}/busy")).send().await.unwrap()).await;
        assert_eq!((s, p.error.as_str(), h[header::RETRY_AFTER].to_str().unwrap()), (503, "audit_backlog", "5"));
        let (s, p, _) = problem(http.get(format!("{url}/limit")).send().await.unwrap()).await;
        assert_eq!((s, p.error.as_str(), p.extra["period"].as_str()), (429, "budget_exceeded", Some("monthly")));
        let (s, p, h) = problem(http.get(format!("{url}/denied")).send().await.unwrap()).await;
        assert_eq!((s, p.error.as_str(), h[header::WWW_AUTHENTICATE].to_str().unwrap()), (401, "unauthorized", "Bearer"));
    }
}
