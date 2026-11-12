//! What every Askbar service the project builds shares (ADR 0028, plan step 1.3):
//!
//! - [`links`]: the per-link bearer check. Each caller has its own token; the service holds only
//!   its SHA-256 in a file, and each caller may call only its own routes.
//! - [`status`]: the boot ID and the `GET /status` body the admin polls.
//! - [`configured`]: `503 not_configured` until the admin has pushed configuration.
//! - [`restart`]: `POST /restart` (exit cleanly; Docker's restart policy starts the service again)
//!   and the watchdog that exits a service whose own health check keeps failing (ADR 0005).
//! - [`ids`]: server-generated ULIDs.
//! - [`audit_client`]: the link to the auditor, with its queue of records to send (ADR 0010, ADR 0054).
//! - [`tar`]: the archive a backup export is written in.
//!
//! The common internal routes are the same in every service (ADR 0022): `GET /health`,
//! `GET /status`, `PUT /config`, `POST /restart` (and later `POST /exports`). Their paths are in
//! [`routes`].

pub mod alert_client;
pub mod audit_client;
#[cfg(feature = "backup")]
pub mod backup;
pub mod configured;
pub mod ids;
pub mod links;
pub mod periods;
pub mod restart;
pub mod restart_loop;
pub mod status;
pub mod tar;

/// The common internal routes, after a service's own `/api/{name}/v1` prefix.
pub mod routes {
    pub const HEALTH: &str = "/health";
    pub const STATUS: &str = "/status";
    pub const CONFIG: &str = "/config";
    pub const RESTART: &str = "/restart";
}
