//! Sign one approved tool action (plan step P5.4). `/decide` (P5.6) calls this; nothing here touches the network.
//!
//! The caller records the returned audit attributes on its span: the action hash and the one-time ID, never the
//! arguments, the key or the signature. Returning them, not taking a sink, keeps this a pure function.

use crate::Gateway;
use askbar_proto::approval::{Approval, ToolAction};
use askbar_service::approval::{sign, ApprovalError};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

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
