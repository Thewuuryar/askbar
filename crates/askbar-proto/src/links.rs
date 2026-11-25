//! The names of the authenticated links (ADR 0049). `bootstrap` makes one token file per name, and each
//! receiver looks its caller up by the same name. The Links table in docs/architecture.md is the source of
//! truth. A new link is added here once.

pub const ADMIN_SERVER: &str = "admin-server";
pub const ADMIN_AUDITOR_APPEND: &str = "admin-auditor-append";
pub const ADMIN_AUDITOR_READ: &str = "admin-auditor-read";
pub const SERVER_MODEL_GATEWAY: &str = "server-model-gateway";
pub const ADMIN_MODEL_GATEWAY: &str = "admin-model-gateway";
pub const MODEL_GATEWAY_AUDITOR: &str = "model-gateway-auditor";
pub const SERVER_AUDITOR: &str = "server-auditor";
pub const MODEL_GATEWAY_AUDITOR_READ: &str = "model-gateway-auditor-read";
pub const AUDITOR_ALERTER: &str = "auditor-alerter";
pub const ADMIN_ALERTER: &str = "admin-alerter";
pub const ALERTER_AUDITOR: &str = "alerter-auditor";
pub const ADMIN_AUDITOR_RESTORE: &str = "admin-auditor-restore";
pub const ADMIN_EGRESS_PROXY: &str = "admin-egress-proxy";
pub const SERVER_MCP_GATEWAY: &str = "server-mcp-gateway";
pub const ADMIN_MCP_GATEWAY: &str = "admin-mcp-gateway";
