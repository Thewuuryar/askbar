//! `POST /tools/{tool}/call`: the signed approval is checked first (a problem on any failure, ADR 0023, counted by
//! outcome), and nothing after it runs yet.

use crate::approval::At;
use crate::Gateway;
use askbar_proto::approval::{Approval, Provenance, ToolAction};
use askbar_proto::http::ApiError;
use askbar_proto::ErrorCode;
use askbar_service::approval::{verify, ApprovalError};
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use std::sync::Arc;

/// The action's fields but the tool (that is the path), and the approval for it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallBody {
    arguments: serde_json::Value,
    destination: String,
    #[serde(default)]
    provenance: Provenance,
    #[serde(default)]
    query_data: bool,
    approval: Approval,
}

fn refusal(e: ApprovalError) -> (&'static str, ErrorCode) {
    match e {
        ApprovalError::Unsignable => ("refused_approval_unsignable", ErrorCode::ApprovalUnsignable),
        ApprovalError::Swapped => ("refused_approval_action_swapped", ErrorCode::ApprovalActionSwapped),
        ApprovalError::Expired => ("refused_approval_expired", ErrorCode::ApprovalExpired),
        ApprovalError::Unsigned => ("refused_approval_unsigned", ErrorCode::ApprovalUnsigned),
        ApprovalError::Replayed => ("refused_approval_replayed", ErrorCode::ApprovalReplayed),
        // `Key` is for signing; a verify never returns it.
        ApprovalError::BadSignature | ApprovalError::Key => ("refused_approval_signature_invalid", ErrorCode::ApprovalSignatureInvalid),
    }
}

pub async fn call(State(g): State<Arc<Gateway>>, Path(tool): Path<String>, body: Bytes) -> Result<Json<()>, ApiError> {
    let config = g.admit()?;
    let b: CallBody = serde_json::from_slice(&body)
        .map_err(|_| ApiError::new(ErrorCode::BadRequest, "The body is not a tool call: a missing or unknown field or a wrong type."))?;
    let Some(key) = config.public_key_bytes() else {
        g.count("refused_not_configured");
        return Err(ApiError::new(ErrorCode::NotConfigured, "The gateway has not received its settings yet.").retry_after(5));
    };
    let action =
        ToolAction { tool, arguments: b.arguments, destination: b.destination, provenance: b.provenance, query_data: b.query_data };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let ids = At { store: &g.one_time_ids, now, window: i64::from(config.one_time_id_window_secs) };
    if let Err(e) = verify(&b.approval, &action, &key, now, &ids) {
        let (outcome, code) = refusal(e);
        g.count(outcome);
        return Err(ApiError::new(code, "The approval was refused."));
    }
    // The next checks (permission, audit span, the tool) are P5A.9 to P5A.11.
    g.count("refused_tool_not_available");
    Err(ApiError::new(ErrorCode::ToolNotAvailable, "The tool is not available yet."))
}
