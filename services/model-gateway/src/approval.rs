//! Sign one approved tool action (plan step P5.4). `/decide` (P5.6) and the quick search route (W5.2) call this; nothing
//! here touches a provider.
//!
//! The caller records the returned audit attributes on its span: the action hash and the one-time ID, never the
//! arguments, the key or the signature. Returning them, not taking a sink, keeps this a pure function.

use crate::{finish, gate_blocked, refused, start_of, valid_id, Failure, Gateway};
use askbar_proto::approval::{Approval, Provenance, QuickSearchApprovalRequest, ToolAction};
use askbar_proto::audit::SpanKind;
use askbar_proto::http::ApiError;
use askbar_proto::quick_search::QuickSearchLimits;
use askbar_proto::ErrorCode;
use askbar_service::approval::{sign, ApprovalError};
use axum::extract::State;
use axum::Json;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// Why no approval was made. Carries no data, so no key can leak through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApproveError {
    /// No signing key is held (none pushed yet): sign nothing.
    NoKey,
    /// The held key is unusable, or the action cannot be signed.
    Sign(ApprovalError),
}

/// A signed approval for `action`, with a fresh one-time ID and an expiry the approval TTL from now, plus the attributes
/// for the audit span.
pub fn approve(gw: &Gateway, action: &ToolAction, task_id: &str, user: &str) -> Result<(Approval, serde_json::Value), ApproveError> {
    let key = gw.approval_key().ok_or(ApproveError::NoKey)?;
    let pkcs8 = URL_SAFE_NO_PAD.decode(key.pkcs8).map_err(|_| ApproveError::Sign(ApprovalError::Key))?;
    let id = askbar_service::ids::new_id();
    let expires_at = askbar_service::audit_client::now_parts().0 + i64::from(gw.approval_ttl_secs());
    let a = sign(action, task_id, user, &id, expires_at, &pkcs8).map_err(ApproveError::Sign)?;
    let attrs = serde_json::json!({ "action_hash": a.action_hash, "one_time_id": a.one_time_id, "expires_at": a.expires_at });
    Ok((a, attrs))
}

// ------------------------------------------------------------------ POST /approvals/quick-search (plan W5.2)

/// Replies and users remembered; the oldest reply is forgotten first.
const MAX_REPLIES: usize = 10_000;
const MAX_USERS: usize = 10_000;
const HOUR_SECS: i64 = 3600;

/// What quick search approvals were given: per reply `(searches, fetches)`, and per user the times of the last hour's
/// searches. A refused request counts for nothing; an approved one counts once the span is acknowledged or not at all.
#[derive(Default)]
pub struct QuickCounts {
    replies: HashMap<String, (u32, u32)>,
    order: VecDeque<String>,
    hourly: HashMap<String, VecDeque<i64>>,
}

impl QuickCounts {
    /// Count one approval, or say which limit it would break.
    fn reserve(
        &mut self,
        reply: &str,
        user: &str,
        search: bool,
        l: &QuickSearchLimits,
        now: i64,
    ) -> Result<(), (ErrorCode, serde_json::Value)> {
        let (searches, fetches) = self.replies.get(reply).copied().unwrap_or_default();
        if search && searches >= l.max_queries {
            return Err((ErrorCode::QuickSearchQueryCount, serde_json::json!({ "searches": searches, "limit": l.max_queries })));
        }
        if !search && fetches >= l.max_pages {
            return Err((ErrorCode::QuickSearchFetchCount, serde_json::json!({ "fetches": fetches, "limit": l.max_pages })));
        }
        if search {
            if self.hourly.len() >= MAX_USERS && !self.hourly.contains_key(user) {
                self.hourly.retain(|_, t| t.back().is_some_and(|t| *t > now - HOUR_SECS));
            }
            // shortcut: more than MAX_USERS searching users in an hour are refused, not forgotten; raise the bound if it bites.
            if self.hourly.len() >= MAX_USERS && !self.hourly.contains_key(user) {
                return Err((ErrorCode::QuickSearchHourlyLimit, serde_json::json!({ "limit": l.searches_per_user_hour })));
            }
            let times = self.hourly.entry(user.into()).or_default();
            while times.front().is_some_and(|t| *t <= now - HOUR_SECS) {
                times.pop_front();
            }
            if times.len() >= l.searches_per_user_hour as usize {
                return Err((
                    ErrorCode::QuickSearchHourlyLimit,
                    serde_json::json!({ "searches": times.len(), "limit": l.searches_per_user_hour }),
                ));
            }
            times.push_back(now);
        }
        if !self.replies.contains_key(reply) {
            if self.order.len() >= MAX_REPLIES {
                if let Some(old) = self.order.pop_front() {
                    self.replies.remove(&old);
                }
            }
            self.order.push_back(reply.into());
        }
        let e = self.replies.entry(reply.into()).or_default();
        if search {
            e.0 += 1;
        } else {
            e.1 += 1;
        }
        Ok(())
    }

    /// Take back a `reserve` whose approval was not given.
    fn release(&mut self, reply: &str, user: &str, search: bool) {
        if let Some(e) = self.replies.get_mut(reply) {
            if search {
                e.0 = e.0.saturating_sub(1);
            } else {
                e.1 = e.1.saturating_sub(1);
            }
        }
        if search {
            if let Some(t) = self.hourly.get_mut(user) {
                t.pop_back();
            }
        }
    }
}

/// The one argument of the tool, as the MCP front takes it: `search` a non-blank `query` of at most `max_chars`
/// characters, `fetch` a `url` string. The `destination` to sign is the URL's host for a fetch and empty for a search.
fn shape(tool: &str, args: &serde_json::Value, max_chars: u32) -> Result<String, &'static str> {
    let one = args.as_object().filter(|o| o.len() == 1).ok_or("shape")?;
    if tool == "search" {
        let q = one.get("query").and_then(|q| q.as_str()).ok_or("shape")?;
        if q.trim().is_empty() {
            return Err("shape");
        }
        return if q.chars().count() > max_chars as usize { Err("too_long") } else { Ok(String::new()) };
    }
    let url = one.get("url").and_then(|u| u.as_str()).ok_or("shape")?;
    let (_, rest) = url.split_once("://").ok_or("shape")?;
    let auth = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // The front decides whether the URL may be fetched; here the host only names where the approval is for.
    let host = auth.rsplit('@').next().unwrap_or_default().split(':').next().unwrap_or_default().to_ascii_lowercase();
    if host.is_empty() {
        Err("shape")
    } else {
        Ok(host)
    }
}

/// `POST /approvals/quick-search`: sign a quick search call by fixed rules, or refuse it with its own code. The span is on
/// record (acknowledged by the auditor) before the approval is returned. Never calls a model.
pub async fn quick_search(
    State(gw): State<Arc<Gateway>>,
    Json(req): Json<QuickSearchApprovalRequest>,
) -> Result<Json<serde_json::Value>, Failure> {
    let Some(cfg) = gw.applied() else {
        return Err(ApiError::new(ErrorCode::NotConfigured, "The gateway has not received its settings yet. Try again in a few seconds.")
            .retry_after(5));
    };
    if !valid_id(&req.generation_id) || !valid_id(&req.user) {
        return Err(ApiError::new(ErrorCode::BadRequest, "generation_id and user are 1 to 64 letters, digits, - or _."));
    }
    const NAME: &str = "quick_search_approval";
    // Only counts and flags go on a refusal's span: never the query or the URL.
    let deny = |code: ErrorCode, attrs: serde_json::Value| {
        gate_blocked(&gw, &req.generation_id, &req.user, None, NAME, code.as_str(), attrs);
        ApiError::new(code, askbar_proto::http::default_detail(code))
    };
    if !cfg.quick_search_enabled {
        return Err(deny(ErrorCode::QuickSearchDisabled, serde_json::json!({ "enabled": false })));
    }
    if cfg.kill_switch {
        return Err(deny(ErrorCode::KillSwitch, serde_json::json!({ "kill_switch": true })));
    }
    if cfg.paused.contains(&req.user) {
        return Err(deny(ErrorCode::UserPaused, serde_json::json!({ "paused": true })));
    }
    if !askbar_proto::mcp::TOOLS.contains(&req.tool.as_str()) {
        return Err(deny(ErrorCode::QuickSearchTool, serde_json::json!({ "allowed": false })));
    }
    let search = req.tool == "search";
    let limits = cfg.quick_search;
    let destination = shape(&req.tool, &req.arguments, limits.max_query_chars)
        .map_err(|why| deny(ErrorCode::QuickSearchArguments, serde_json::json!({ "tool": req.tool, "reason": why })))?;
    let now = askbar_service::audit_client::now_parts().0;
    gw.quick
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .reserve(&req.generation_id, &req.user, search, &limits, now)
        .map_err(|(code, attrs)| deny(code, serde_json::json!({ "tool": req.tool, "counts": attrs })))?;
    let release = || gw.quick.lock().unwrap_or_else(|e| e.into_inner()).release(&req.generation_id, &req.user, search);
    // Signed in memory first: its hash and one-time ID go on the span. It is returned only once the span is acknowledged.
    let action =
        ToolAction { tool: req.tool.clone(), arguments: req.arguments, destination, provenance: Provenance::Search, query_data: false };
    let (approval, mut attrs) = match approve(&gw, &action, &req.generation_id, &req.user) {
        Ok(a) => a,
        Err(e) => {
            release();
            return Err(match e {
                ApproveError::NoKey => {
                    ApiError::new(ErrorCode::NotConfigured, "The gateway holds no approval key yet. Try again in a few seconds.")
                        .retry_after(5)
                }
                ApproveError::Sign(_) => {
                    ApiError::new(ErrorCode::ApprovalUnsignable, askbar_proto::http::default_detail(ErrorCode::ApprovalUnsignable))
                }
            });
        }
    };
    attrs["tool"] = req.tool.clone().into();
    let span_id = askbar_service::ids::new_id();
    let mut start = start_of(&span_id, &req.generation_id, None, SpanKind::Gate, NAME);
    start.user_id = Some(req.user.clone());
    start.attrs = Some(attrs);
    if let Err(why) = gw.audit.open(&start).await {
        release();
        let mut f = finish("refused");
        f.error_code = Some("audit_backlog".into());
        gw.audit.refusal(start, f);
        return Err(refused(why));
    }
    gw.audit.finish(&span_id, finish("ok"));
    Ok(Json(serde_json::json!({
        "approval": approval,
        "destination": action.destination,
        "provenance": action.provenance,
        "query_data": action.query_data,
    })))
}
