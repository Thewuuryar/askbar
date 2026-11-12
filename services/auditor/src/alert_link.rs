//! The auditor's link to the alerter (`auditor-alerter`, ADR 0055): events go to `POST /events` on the network the two share.
//! The token is `call/auditor-alerter.token` in `auditor-token`, made by `bootstrap`. Nothing is sent when there is no token:
//! the alerter then has no way to know the sender, and the auditor does not hold events it cannot send.

use askbar_service::alert_client::AlertClient;
use std::path::Path;
use std::sync::Arc;

/// The link's name, as `bootstrap` names it.
pub const LINK: &str = "auditor-alerter";

/// The alerter's address on `audit-alerter`: `ASKBAR_ALERTER_URL`, default `http://alerter-auditor-host:8085` (the alias exists
/// on that network only).
pub fn alerter_url() -> String {
    std::env::var("ASKBAR_ALERTER_URL").ok().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| "http://alerter-auditor-host:8085".into())
}

/// The client for events, reading its token from `token_dir` each time it sends.
pub fn client(token_dir: &Path) -> Arc<AlertClient> {
    AlertClient::new(&alerter_url(), token_dir.to_path_buf(), LINK)
}
