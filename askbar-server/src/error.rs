//! Every error the server's HTTP APIs answer with: `application/problem+json` (RFC 9457).
//! See `askbar_proto::problem` for the shape and the registry of codes.

use askbar_proto::problem::{ErrorCode, Problem, CONTENT_TYPE};
use axum::{
    extract::rejection::JsonRejection,
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};

/// A problem plus the header that goes with some of them.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub problem: Problem,
    retry_after: Option<u64>,
}

impl ApiError {
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> ApiError {
        let retry_after = (code == ErrorCode::AuditBacklog).then_some(crate::generation::AUDIT_BACKLOG_RETRY_SECS);
        ApiError { problem: Problem::new(code, detail), retry_after }
    }

    /// The code with its stock sentence.
    pub fn code(code: ErrorCode) -> ApiError {
        ApiError::new(code, default_detail(code))
    }

    /// Adds an extension member such as `period`.
    pub fn with(mut self, key: &str, value: impl Into<serde_json::Value>) -> ApiError {
        self.problem = self.problem.with(key, value);
        self
    }

    /// Sets `Retry-After` (seconds).
    pub fn retry_after(mut self, secs: u64) -> ApiError {
        self.retry_after = Some(secs);
        self
    }
}

/// The sentence for a person when a call site has nothing more specific to say.
pub fn default_detail(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::QueueFull => "Five messages are already waiting in this conversation.",
        ErrorCode::BudgetExceeded => "A usage limit has been reached.",
        ErrorCode::KillSwitch => "Replies have been switched off by the administrator.",
        ErrorCode::AuditKeyMissing => "The administrator has not set up the audit key yet.",
        ErrorCode::AuditBacklog => "The server is busy recording and asked for a short wait.",
        ErrorCode::ClientTooOld => "This version of Askbar is too old for the server. Update the app.",
        ErrorCode::ClientVersionMissing => "The request did not say which version of Askbar sent it.",
        ErrorCode::Unauthorized => "Missing or invalid credentials.",
        ErrorCode::Forbidden => "This is not allowed.",
        ErrorCode::NotFound => "There is nothing at this address.",
        ErrorCode::MethodNotAllowed => "This address does not accept that method.",
        ErrorCode::BadRequest => "The request could not be understood.",
        ErrorCode::PayloadTooLarge => "The request is too large.",
        ErrorCode::TooManyAttempts => "Too many attempts. Try again later.",
        ErrorCode::ApiKeyExists => "An API key already exists. Revoke it first.",
        ErrorCode::SetupRequired => "Setup is not finished.",
        ErrorCode::Conflict => "The request conflicts with the current state.",
        ErrorCode::Internal => "Something went wrong on the server.",
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.problem.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut res = (status, self.problem.to_json_bytes()).into_response();
        let h = res.headers_mut();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static(CONTENT_TYPE));
        if let Some(secs) = self.retry_after {
            h.insert(header::RETRY_AFTER, HeaderValue::from(secs));
        }
        if self.problem.status == 401 {
            h.insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        res
    }
}

/// Lets handlers that still say `StatusCode::X` become problems with `?` or `.into()`.
impl From<StatusCode> for ApiError {
    fn from(s: StatusCode) -> ApiError {
        let code = match s.as_u16() {
            400 => ErrorCode::BadRequest,
            401 => ErrorCode::Unauthorized,
            403 => ErrorCode::Forbidden,
            404 => ErrorCode::NotFound,
            405 => ErrorCode::MethodNotAllowed,
            409 => ErrorCode::Conflict,
            413 => ErrorCode::PayloadTooLarge,
            426 => ErrorCode::ClientTooOld,
            429 => ErrorCode::TooManyAttempts,
            503 => ErrorCode::SetupRequired,
            _ => ErrorCode::Internal,
        };
        ApiError::code(code)
    }
}

impl From<JsonRejection> for ApiError {
    fn from(r: JsonRejection) -> ApiError {
        let code = if r.status() == StatusCode::PAYLOAD_TOO_LARGE { ErrorCode::PayloadTooLarge } else { ErrorCode::BadRequest };
        // The body text names the field that failed; it holds nothing the caller did not send.
        ApiError::new(code, r.body_text())
    }
}

/// A JSON body whose parse failures are problems instead of plain text.
pub struct ApiJson<T>(pub T);

impl<S, T> axum::extract::FromRequest<S> for ApiJson<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, ApiError> {
        axum::Json::<T>::from_request(req, state).await.map(|j| ApiJson(j.0)).map_err(ApiError::from)
    }
}

impl<T: serde::Serialize> IntoResponse for ApiJson<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// Router fallback: no route matches.
pub async fn not_found() -> ApiError {
    ApiError::code(ErrorCode::NotFound)
}

/// Router fallback: the path exists, the method does not.
pub async fn method_not_allowed() -> ApiError {
    ApiError::code(ErrorCode::MethodNotAllowed)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let (s, p, _) = problem(http.post(format!("{url}/echo")).header("content-type", "application/json").body("{not json").send().await.unwrap()).await;
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
