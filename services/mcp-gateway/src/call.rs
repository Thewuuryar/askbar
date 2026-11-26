//! `POST /tools/{tool}/call`: the signed approval is checked first (a problem on any failure, ADR 0023, counted by
//! outcome), then the user's permission for the tool, then the PII check of a query or URL (`pii.rs`, counted as `refused_pii`), then the `tool` span is on record before the registry's handler runs
//! (ADR 0010 rules 1 to 3). The handler is bounded by the tool-call time limit; its result goes back marked untrusted.

use crate::approval::At;
use crate::permissions;
use crate::pii;
use crate::registry::ToolCall;
use crate::Gateway;
use askbar_proto::approval::{Approval, Provenance, ToolAction};
use askbar_proto::audit::{SpanFinish, SpanKind, SpanStart};
use askbar_proto::http::ApiError;
use askbar_proto::ErrorCode;
use askbar_service::approval::{verify, ApprovalError};
use askbar_service::audit_client::now_parts;
use askbar_service::ids::new_id;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
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

pub async fn call(State(g): State<Arc<Gateway>>, Path(tool): Path<String>, body: Bytes) -> Result<Json<serde_json::Value>, ApiError> {
    let config = g.admit()?;
    let b: CallBody = serde_json::from_slice(&body)
        .map_err(|_| ApiError::new(ErrorCode::BadRequest, "The body is not a tool call: a missing or unknown field or a wrong type."))?;
    let Some(key) = config.public_key_bytes() else {
        g.count("refused_not_configured");
        return Err(ApiError::new(ErrorCode::NotConfigured, "The gateway has not received its settings yet.").retry_after(5));
    };
    let mut action =
        ToolAction { tool, arguments: b.arguments, destination: b.destination, provenance: b.provenance, query_data: b.query_data };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let ids = At { store: &g.one_time_ids, now, window: i64::from(config.one_time_id_window_secs) };
    if let Err(e) = verify(&b.approval, &action, &key, now, &ids) {
        let (outcome, code) = refusal(e);
        g.count(outcome);
        return Err(ApiError::new(code, "The approval was refused."));
    }
    if !permissions::holds(&config.permissions, &b.approval.user, &action.tool) {
        g.count("refused_permission");
        return Err(ApiError::new(ErrorCode::ToolNotPermitted, "The user does not hold this tool."));
    }
    if !g.registry.has(&action.tool) {
        g.count("refused_tool_not_available");
        return Err(ApiError::new(ErrorCode::ToolNotAvailable, "The tool is not available."));
    }
    // The approval covered the original arguments; from here on the cleaned ones run. Counts only go on the span.
    let checked = match pii::check(&config.pii, &action.tool, &mut action.arguments) {
        Ok(c) => c,
        Err(b) => {
            g.count("refused_pii");
            return Err(ApiError::new(ErrorCode::PiiBlocked, b.detail).with("types", b.types));
        }
    };
    // The span carries the approval's hash (which covers the arguments) and ID, never the
    // arguments, a URL or the signature; sealed arguments are not recorded.
    let Some(audit) = g.audit() else {
        g.count("refused_audit_unavailable");
        return Err(audit_down());
    };
    let (at, ms) = now_parts();
    let (id, a) = (new_id(), &b.approval);
    let start = SpanStart {
        id: id.clone(),
        trace_id: a.task_id.clone(),
        parent_id: None,
        kind: SpanKind::Tool,
        name: action.tool.clone(),
        started_at: at,
        started_ms: ms,
        provider_id: None,
        model: None,
        user_id: Some(a.user.clone()),
        client_id: None,
        attrs: Some(match checked.attrs() {
            Some(serde_json::Value::Object(mut o)) => {
                o.insert("action_hash".into(), a.action_hash.clone().into());
                o.insert("one_time_id".into(), a.one_time_id.clone().into());
                o.into()
            }
            _ => serde_json::json!({ "action_hash": a.action_hash, "one_time_id": a.one_time_id }),
        }),
    };
    // Nothing runs until the auditor has acknowledged the span.
    if let Err(why) = audit.open(&start).await {
        audit.refusal(start, finish("refused", Some("audit_backlog")));
        g.count("refused_audit_unavailable");
        tracing::warn!(?why, "the tool span was not acknowledged; the call is refused");
        return Err(audit_down());
    }
    g.tool_runs.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tool_call = ToolCall {
        action,
        task_id: a.task_id.clone(),
        user: a.user.clone(),
        config: config.clone(),
        audit: Some(audit.clone()),
        span_id: id.clone(),
    };
    let run = (g.registry.get(&tool_call.action.tool).expect("checked above"))(tool_call);
    let left_out = checked.left_out;
    let run = async move {
        let mut r = run.await;
        if let (Ok(serde_json::Value::Object(o)), true) = (&mut r, left_out > 0) {
            o.insert("left_out".into(), left_out.into());
        }
        r
    };
    let limit = std::time::Duration::from_secs(u64::from(config.tool_call_timeout_secs));
    // Only the outcome goes to the span; the result is not recorded.
    let (status, code, out) = match tokio::time::timeout(limit, run).await {
        Ok(Ok(result)) if serde_json::to_vec(&result).map_or(usize::MAX, |v| v.len()) as u64 > config.tool_result_max_bytes => {
            ("error", Some(ErrorCode::ToolResultTooLarge), Err(ErrorCode::ToolResultTooLarge.into()))
        }
        Ok(Ok(result)) => ("ok", None, Ok(result)),
        Ok(Err(e)) => ("error", Some(e.code), Err(e)),
        Err(_) => ("timeout", Some(ErrorCode::ToolTimeout), Err(ErrorCode::ToolTimeout.into())),
    };
    audit.finish(&id, finish(status, code.map(ErrorCode::as_str)));
    g.count(match (status, code) {
        ("ok", _) => "tool_ok",
        (_, Some(ErrorCode::ToolNotAvailable)) => "refused_tool_not_available",
        ("timeout", _) => "tool_timeout",
        _ => "tool_error",
    });
    match out {
        Ok(result) => Ok(Json(serde_json::json!({ "result": result, "untrusted": true }))),
        Err(e) => {
            let mut err = ApiError::new(e.code, "The tool did not return a result.");
            if let Some(reason) = e.reason {
                err.problem = err.problem.with("reason", reason);
            }
            Err(err)
        }
    }
}

fn audit_down() -> ApiError {
    ApiError::new(ErrorCode::AuditBacklog, "The call could not be recorded, so nothing was run. Try again in a few seconds.").retry_after(5)
}

pub(crate) fn finish(status: &str, error_code: Option<&str>) -> SpanFinish {
    let (at, ms) = now_parts();
    SpanFinish {
        status: status.into(),
        ended_at: at,
        ended_ms: ms,
        provider_id: None,
        model: None,
        tokens_in: None,
        tokens_out: None,
        tokens_cached: None,
        ttft_ms: None,
        error_code: error_code.map(str::to_string),
        attrs: None,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolsQuery {
    user: String,
}

/// `GET /tools?user=`: the registered tools the user holds, by name (no tool has a description yet). Nothing for an unknown user.
pub async fn tools(State(g): State<Arc<Gateway>>, Query(q): Query<ToolsQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    let config = g.admit()?;
    let tools: Vec<_> = permissions::permitted(&config.permissions, &q.user)
        .into_iter()
        .filter_map(|name| Some(serde_json::json!({ "name": name, "schema": g.registry.schema(name)? })))
        .collect();
    Ok(Json(serde_json::json!({ "tools": tools })))
}
