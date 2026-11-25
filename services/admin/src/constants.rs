//! The one home of the admin's behaviour values: limits, intervals, names and links (plan step 1.1).
//! The server's checks read them from here, and `/admin/constants.json` serves the same values to the
//! pages, so a limit is never typed twice and the UI cannot drift from what the server enforces.
//! Visual values (colours, sizes, spacing) are not here: they live in `ui/tokens.css`.
//!
//! Nothing secret belongs here, because the JSON is served to anyone who can reach the sign-in page:
//! limits, intervals, names and links only. (The published first-run password stays in `web`.)

use serde::Serialize;
use std::sync::OnceLock;

// Identity and links.
/// The one admin account's name (ADR 0007: v1 has one admin account).
pub const ADMIN_USERNAME: &str = "admin";
/// What every admin API key starts with, so one can be recognised (and a secret scanner can find it).
pub const API_KEY_PREFIX: &str = "askbar-admin-";
/// Where the source is (the AGPL notice links to it).
pub const SOURCE_URL: &str = "https://github.com/Thewuuryar/askbar";
pub const LICENSE: &str = "AGPL-3.0-or-later";
/// The licence text, in the source repository.
pub const LICENSE_URL: &str = "https://github.com/Thewuuryar/askbar/blob/main/LICENSE";
/// The branch the source links point at (the repository's default branch).
pub const SOURCE_BRANCH: &str = "main";
/// Where the reference articles live, relative to the repository root.
pub const REFERENCE_ROOT: &str = "docs/reference";

// Sign-in and secrets.
pub const MIN_PASSWORD_CHARS: usize = 10;
pub const MAX_PASSWORD_CHARS: usize = 200;
/// A passphrase for the audit key must be at least this long, counted in characters.
pub const MIN_PASSPHRASE_CHARS: usize = 20;
/// A sign-in session lasts this long.
pub const SESSION_TTL_SECS: i64 = 12 * 60 * 60;
/// This many wrong passwords in a row lock the sign-in.
pub const MAX_FAILURES: usize = 5;
/// How long the sign-in stays locked.
pub const LOCKOUT_SECS: i64 = 15 * 60;
/// A sign-in older than this does not start a sealed-content session.
pub const FRESH_SIGNIN_SECS: i64 = 10 * 60;

// Settings ranges.
/// The same bounds the server enforces on the snapshot.
pub const MIN_GENERATION_TIMEOUT_SECS: u32 = 30;
pub const MAX_GENERATION_TIMEOUT_SECS: u32 = 3600;
pub const DEFAULT_GENERATION_TIMEOUT_SECS: u32 = askbar_proto::DEFAULT_GENERATION_TIMEOUT_SECS;
/// How long a signed tool approval lasts. Pushed to the gateway; bounds in `askbar-proto`.
pub const APPROVAL_TTL_SECS: u32 = askbar_proto::DEFAULT_APPROVAL_TTL_SECS;
/// The same floor the server enforces (and the audit database's own trigger): a week.
pub const MIN_AUDIT_RETENTION_DAYS: u32 = 7;
pub const MAX_AUDIT_RETENTION_DAYS: u32 = 3650;
pub const DEFAULT_UNSEAL_IDLE_MINUTES: u32 = askbar_proto::audit::DEFAULT_UNSEAL_IDLE_MINUTES;
pub const DEFAULT_UNSEAL_TOTAL_MINUTES: u32 = askbar_proto::audit::DEFAULT_UNSEAL_TOTAL_MINUTES;
pub const MIN_UNSEAL_MINUTES: u32 = 1;
pub const MAX_UNSEAL_MINUTES: u32 = 12 * 60;
pub const MIN_HISTORY_TURNS: u32 = 0;
pub const MAX_HISTORY_TURNS: u32 = 1000;
pub const MAX_NOTICE_CHARS: usize = 2000;
pub const MAX_WARNING_CHARS: usize = 300;
// The display defaults' ranges (General). The server checks the same bounds on save.
pub const MIN_DISPLAY_WIDTH_PX: u32 = 300;
pub const MAX_DISPLAY_WIDTH_PX: u32 = 4000;
pub const MIN_DISPLAY_MAX_HEIGHT_PX: u32 = 200;
pub const MAX_DISPLAY_MAX_HEIGHT_PX: u32 = 4000;
pub const MIN_DISPLAY_HISTORY_WIDTH_PX: u32 = 120;
pub const MAX_DISPLAY_HISTORY_WIDTH_PX: u32 = 1000;
pub const MIN_DISPLAY_TOP_PERCENT: f64 = 0.0;
pub const MAX_DISPLAY_TOP_PERCENT: f64 = 0.8;
pub const API_KEY_MAX_NAME_CHARS: usize = 60;
/// The longest name of a user or a client (the API refuses more; the forms stop typing at it).
pub const NAME_MAX_CHARS: usize = 60;

// Pairing and services.
pub const PAIR_CODE_TTL_SECS: i64 = 10 * 60;
/// The services the Services page shows and may restart.
pub const SERVICES: [&str; 6] = ["server", "model-gateway", "alerter", "admin", "caddy", "cpa"];
/// What `cpa` may reach through the egress proxy (ADR 0056): the hosts behind the two subscription logins, nothing else.
pub const CPA_EGRESS_HOSTS: [&str; 5] = ["api.anthropic.com", "platform.claude.com", "auth.openai.com", "chatgpt.com", "api.openai.com"];
/// The limits the admin pushes to the egress proxy; `askbar-proto` bounds them.
pub const EGRESS_LIMITS: askbar_proto::egress::Limits = askbar_proto::egress::Limits {
    head_bytes: 8 * 1024,
    header_lines: 64,
    connect_secs: 10,
    head_read_secs: 10,
    idle_secs: 300,
    max_tunnels: 256,
    max_tunnels_per_source: 128,
};
// MCP gateway tool bounds (phase 5A). Pushed to the gateway; the bounds live in `askbar-proto::mcp`.
pub const MCP_FETCH_MAX_BYTES: u64 = askbar_proto::mcp::DEFAULT_FETCH_MAX_BYTES;
pub const MCP_FETCH_TIMEOUT_SECS: u32 = askbar_proto::mcp::DEFAULT_FETCH_TIMEOUT_SECS;
pub const MCP_FETCH_REDIRECTS: u32 = askbar_proto::mcp::DEFAULT_FETCH_REDIRECTS;
pub const MCP_TOOL_CALL_TIMEOUT_SECS: u32 = askbar_proto::mcp::DEFAULT_TOOL_CALL_TIMEOUT_SECS;
pub const MCP_ONE_TIME_ID_WINDOW_SECS: u32 = askbar_proto::mcp::DEFAULT_ONE_TIME_ID_WINDOW_SECS;
pub const MCP_SEARCH_RESULTS: u32 = askbar_proto::mcp::DEFAULT_SEARCH_RESULTS;
pub const MCP_REMEMBERED_URLS: u32 = askbar_proto::mcp::DEFAULT_REMEMBERED_URLS;
/// At most this many service actions in a minute.
pub const MAX_RESTARTS_PER_MINUTE: usize = 6;

// Backups.
/// 03:30 UTC, in seconds after midnight.
pub const BACKUP_RUN_AT_SECS: i64 = 3 * 3600 + 30 * 60;
/// No successful backup for this long is a missed one.
pub const BACKUP_MISSED_AFTER_SECS: i64 = 26 * 3600;
/// After a failed run, the next try is this much later (until the next day's slot).
pub const BACKUP_RETRY_AFTER_SECS: i64 = 3600;
/// The restore drill is due this often.
pub const DRILL_EVERY_SECS: i64 = 90 * 24 * 3600;
/// The Backup page's keep limits: how many backups, and how many days. The server checks the same ranges.
pub const MIN_BACKUP_KEEP_FILES: u32 = 1;
pub const MAX_BACKUP_KEEP_FILES: u32 = 365;
pub const MIN_BACKUP_KEEP_DAYS: u32 = 1;
pub const MAX_BACKUP_KEEP_DAYS: u32 = 3650;

// Pages.
/// Every admin page's slug (`/admin/<slug>/`), in menu order. `pages::PAGES` lists the same, which a test checks.
pub const PAGE_SLUGS: [&str; 15] = [
    "home",
    "alerts",
    "audit/calls",
    "audit/admin",
    "audit/sealed",
    "audit/routing",
    "general",
    "providers",
    "people",
    "services",
    "network",
    "security",
    "backup",
    "notifications",
    "reference",
];
/// The page `/admin` opens.
pub const LANDING_PAGE: &str = "home";
// Help topics (plan steps 7.1a and 7.3, and R8: every page title and section heading has one): each is `<reference page id>#<heading>`, the heading the ? button opens, or a
// bare `<reference page id>`, the page's own ? button (it shows the text before the page's first heading). The reference
// pages under docs/reference hold them, and `scripts/check-reference.sh` fails on a topic with no such page or heading.
pub const HELP_TOPICS: &[&str] = &[
    "settings/providers",
    "settings/people",
    "settings/services",
    "settings/network",
    "settings/security",
    "settings/backup",
    "settings/notifications",
    "settings/general",
    "settings/providers#providers",
    "settings/providers#claude-and-chatgpt-accounts",
    "settings/providers#models-people-can-use",
    "settings/people#users",
    "settings/people#paired-clients",
    "settings/services#running-services",
    "settings/services#blocked-outbound-requests",
    "settings/network#host-name",
    "settings/network#certificate",
    "settings/network#allowed-addresses-and-networks",
    "settings/security#change-admin-password",
    "settings/security#passkey",
    "settings/security#recovery-codes",
    "settings/security#admin-api-key",
    "settings/security#sealed-content-and-audit-records",
    "settings/security#rotate-the-audit-key",
    "settings/security#backup-key",
    "settings/security#danger-zone",
    "settings/backup#back-up-now",
    "settings/backup#scheduled-backups",
    "settings/backup#how-long-backups-are-kept",
    "settings/backup#download-a-backup",
    "settings/backup#restore-from-a-backup",
    "settings/notifications#ntfy",
    "settings/notifications#email",
    "settings/notifications#test-alert",
    "settings/notifications#page-wide-banners",
    "settings/general#system-prompt",
    "settings/general#reply-time-limit",
    "settings/general#history-sent-with-each-question",
    "settings/general#what-people-are-told",
    "settings/general#display-defaults",
    "home",
    "home#needs-attention",
    "home#request-numbers",
    "home#requests-per-day",
    "home#spend-by-person",
    "home#models-used",
    "home#services",
    "home#backup",
    "home#clients",
    "alerts",
    "alerts#critical",
    "alerts#warning",
    "alerts#info",
    "audit/model-calls",
    "audit/admin-actions",
    "audit/sealed-access",
    "audit/sealed-access#session",
    "audit/sealed-access#who-opened-sealed-content",
    "audit/routing",
    "audit/routing#this-request",
    "audit/routing#providers-in-the-chosen-range",
    "index",
];

// UI timing.
/// How often the status bar refreshes while the tab is visible.
pub const STATUS_EVERY_SECS: u32 = 5;
/// How often a live list refreshes while the tab is visible.
pub const LIVE_EVERY_SECS: u32 = 10;
/// How often the bell and tab title refresh while the tab is hidden.
pub const HIDDEN_EVERY_SECS: u32 = 60;
/// The waits between retries of a failed check, the last one repeating.
pub const BACKOFF_SECS: [u32; 4] = [10, 20, 40, 60];
/// How long a new row stays highlighted.
pub const HIGHLIGHT_SECS: u32 = 3;
/// How long a toast stays.
pub const TOAST_SECS: u32 = 5;
/// How many days of requests Home's numbers and usage chart cover.
pub const HOME_STATS_WINDOW_DAYS: u32 = 7;
/// The same window in seconds (the `since` of the stats read).
pub const HOME_STATS_WINDOW_SECS: i64 = HOME_STATS_WINDOW_DAYS as i64 * 24 * 3600;
/// The ranges Home's range selector offers, in days (the default is `HOME_STATS_WINDOW_DAYS`; the stats read takes any `since`).
pub const HOME_STATS_RANGE_DAYS: [u32; 3] = [1, 7, 30];
/// How many model calls the Audit list loads at a time (and each time Load more is chosen); the auditor's own cap is higher.
pub const AUDIT_PAGE_SIZE: u32 = 25;
/// How many rows each group of the global search shows; typing more narrows it.
pub const SEARCH_RESULTS_PER_GROUP: usize = 5;
/// How many times the Providers page asks CLIProxyAPI whether a Claude or ChatGPT login finished.
pub const CPA_LOGIN_POLLS: u32 = 30;
/// The wait between those asks.
pub const CPA_LOGIN_POLL_SECS: u32 = 2;

#[derive(Serialize)]
struct Range<T> {
    min: T,
    max: T,
}

#[derive(Serialize)]
struct UnsealMinutes {
    default_idle: u32,
    default_total: u32,
    min: u32,
    max: u32,
}

/// What `/admin/constants.json` holds. Each key is the constant's name.
#[derive(Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
struct Constants {
    admin_username: &'static str,
    api_key_prefix: &'static str,
    source_url: &'static str,
    source_branch: &'static str,
    license: &'static str,
    license_url: &'static str,
    reference_root: &'static str,
    min_password_chars: usize,
    max_password_chars: usize,
    min_passphrase_chars: usize,
    session_ttl_secs: i64,
    max_failures: usize,
    lockout_secs: i64,
    fresh_signin_secs: i64,
    generation_timeout_secs: Range<u32>,
    default_generation_timeout_secs: u32,
    audit_retention_days: Range<u32>,
    unseal_minutes: UnsealMinutes,
    min_history_turns: u32,
    max_history_turns: u32,
    max_notice_chars: usize,
    max_warning_chars: usize,
    display_width_px: Range<u32>,
    display_max_height_px: Range<u32>,
    display_history_width_px: Range<u32>,
    display_top_percent: Range<f64>,
    api_key_max_name_chars: usize,
    name_max_chars: usize,
    pair_code_ttl_secs: i64,
    services: [&'static str; 6],
    max_restarts_per_minute: usize,
    backup_run_at_secs: i64,
    backup_missed_after_secs: i64,
    backup_retry_after_secs: i64,
    drill_every_secs: i64,
    backup_keep_files: Range<u32>,
    backup_keep_days: Range<u32>,
    status_every_secs: u32,
    live_every_secs: u32,
    hidden_every_secs: u32,
    backoff_secs: [u32; 4],
    highlight_secs: u32,
    toast_secs: u32,
    search_results_per_group: usize,
    audit_page_size: u32,
    cpa_login_polls: u32,
    cpa_login_poll_secs: u32,
    home_stats_window_days: u32,
    home_stats_window_secs: i64,
    home_stats_range_days: [u32; 3],
    page_slugs: [&'static str; 15],
    landing_page: &'static str,
    help_topics: &'static [&'static str],
}

fn current() -> Constants {
    Constants {
        admin_username: ADMIN_USERNAME,
        api_key_prefix: API_KEY_PREFIX,
        source_url: SOURCE_URL,
        source_branch: SOURCE_BRANCH,
        license: LICENSE,
        license_url: LICENSE_URL,
        reference_root: REFERENCE_ROOT,
        min_password_chars: MIN_PASSWORD_CHARS,
        max_password_chars: MAX_PASSWORD_CHARS,
        min_passphrase_chars: MIN_PASSPHRASE_CHARS,
        session_ttl_secs: SESSION_TTL_SECS,
        max_failures: MAX_FAILURES,
        lockout_secs: LOCKOUT_SECS,
        fresh_signin_secs: FRESH_SIGNIN_SECS,
        generation_timeout_secs: Range { min: MIN_GENERATION_TIMEOUT_SECS, max: MAX_GENERATION_TIMEOUT_SECS },
        default_generation_timeout_secs: DEFAULT_GENERATION_TIMEOUT_SECS,
        audit_retention_days: Range { min: MIN_AUDIT_RETENTION_DAYS, max: MAX_AUDIT_RETENTION_DAYS },
        unseal_minutes: UnsealMinutes {
            default_idle: DEFAULT_UNSEAL_IDLE_MINUTES,
            default_total: DEFAULT_UNSEAL_TOTAL_MINUTES,
            min: MIN_UNSEAL_MINUTES,
            max: MAX_UNSEAL_MINUTES,
        },
        min_history_turns: MIN_HISTORY_TURNS,
        max_history_turns: MAX_HISTORY_TURNS,
        max_notice_chars: MAX_NOTICE_CHARS,
        max_warning_chars: MAX_WARNING_CHARS,
        display_width_px: Range { min: MIN_DISPLAY_WIDTH_PX, max: MAX_DISPLAY_WIDTH_PX },
        display_max_height_px: Range { min: MIN_DISPLAY_MAX_HEIGHT_PX, max: MAX_DISPLAY_MAX_HEIGHT_PX },
        display_history_width_px: Range { min: MIN_DISPLAY_HISTORY_WIDTH_PX, max: MAX_DISPLAY_HISTORY_WIDTH_PX },
        display_top_percent: Range { min: MIN_DISPLAY_TOP_PERCENT, max: MAX_DISPLAY_TOP_PERCENT },
        api_key_max_name_chars: API_KEY_MAX_NAME_CHARS,
        name_max_chars: NAME_MAX_CHARS,
        pair_code_ttl_secs: PAIR_CODE_TTL_SECS,
        services: SERVICES,
        max_restarts_per_minute: MAX_RESTARTS_PER_MINUTE,
        backup_run_at_secs: BACKUP_RUN_AT_SECS,
        backup_missed_after_secs: BACKUP_MISSED_AFTER_SECS,
        backup_retry_after_secs: BACKUP_RETRY_AFTER_SECS,
        drill_every_secs: DRILL_EVERY_SECS,
        backup_keep_files: Range { min: MIN_BACKUP_KEEP_FILES, max: MAX_BACKUP_KEEP_FILES },
        backup_keep_days: Range { min: MIN_BACKUP_KEEP_DAYS, max: MAX_BACKUP_KEEP_DAYS },
        status_every_secs: STATUS_EVERY_SECS,
        live_every_secs: LIVE_EVERY_SECS,
        hidden_every_secs: HIDDEN_EVERY_SECS,
        backoff_secs: BACKOFF_SECS,
        highlight_secs: HIGHLIGHT_SECS,
        toast_secs: TOAST_SECS,
        search_results_per_group: SEARCH_RESULTS_PER_GROUP,
        audit_page_size: AUDIT_PAGE_SIZE,
        cpa_login_polls: CPA_LOGIN_POLLS,
        cpa_login_poll_secs: CPA_LOGIN_POLL_SECS,
        home_stats_window_days: HOME_STATS_WINDOW_DAYS,
        home_stats_window_secs: HOME_STATS_WINDOW_SECS,
        home_stats_range_days: HOME_STATS_RANGE_DAYS,
        page_slugs: PAGE_SLUGS,
        landing_page: LANDING_PAGE,
        help_topics: HELP_TOPICS,
    }
}

/// The JSON served at `/admin/constants.json`, built once.
pub fn json() -> &'static str {
    static JSON: OnceLock<String> = OnceLock::new();
    JSON.get_or_init(|| serde_json::to_string(&current()).expect("constants serialize"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn the_json_holds_every_constant() {
        let v: Value = serde_json::from_str(json()).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut want = vec![
            "ADMIN_USERNAME",
            "API_KEY_PREFIX",
            "API_KEY_MAX_NAME_CHARS",
            "NAME_MAX_CHARS",
            "AUDIT_PAGE_SIZE",
            "AUDIT_RETENTION_DAYS",
            "BACKOFF_SECS",
            "BACKUP_KEEP_DAYS",
            "BACKUP_KEEP_FILES",
            "BACKUP_MISSED_AFTER_SECS",
            "BACKUP_RETRY_AFTER_SECS",
            "BACKUP_RUN_AT_SECS",
            "CPA_LOGIN_POLLS",
            "CPA_LOGIN_POLL_SECS",
            "DRILL_EVERY_SECS",
            "FRESH_SIGNIN_SECS",
            "GENERATION_TIMEOUT_SECS",
            "HIDDEN_EVERY_SECS",
            "HELP_TOPICS",
            "HIGHLIGHT_SECS",
            "HOME_STATS_WINDOW_DAYS",
            "HOME_STATS_WINDOW_SECS",
            "HOME_STATS_RANGE_DAYS",
            "LANDING_PAGE",
            "LICENSE",
            "LICENSE_URL",
            "LIVE_EVERY_SECS",
            "LOCKOUT_SECS",
            "MAX_FAILURES",
            "DEFAULT_GENERATION_TIMEOUT_SECS",
            "DISPLAY_HISTORY_WIDTH_PX",
            "DISPLAY_MAX_HEIGHT_PX",
            "DISPLAY_TOP_PERCENT",
            "DISPLAY_WIDTH_PX",
            "MAX_HISTORY_TURNS",
            "MIN_HISTORY_TURNS",
            "MAX_NOTICE_CHARS",
            "MAX_PASSWORD_CHARS",
            "MAX_RESTARTS_PER_MINUTE",
            "MAX_WARNING_CHARS",
            "MIN_PASSPHRASE_CHARS",
            "MIN_PASSWORD_CHARS",
            "PAGE_SLUGS",
            "PAIR_CODE_TTL_SECS",
            "REFERENCE_ROOT",
            "SEARCH_RESULTS_PER_GROUP",
            "SERVICES",
            "SESSION_TTL_SECS",
            "SOURCE_BRANCH",
            "SOURCE_URL",
            "STATUS_EVERY_SECS",
            "TOAST_SECS",
            "UNSEAL_MINUTES",
        ];
        want.sort_unstable();
        assert_eq!(keys, want);
    }

    #[test]
    fn the_json_carries_the_values_the_server_enforces() {
        let v: Value = serde_json::from_str(json()).unwrap();
        assert_eq!(v["MIN_PASSWORD_CHARS"], 10);
        assert_eq!(v["MIN_PASSPHRASE_CHARS"], 20);
        assert_eq!(v["GENERATION_TIMEOUT_SECS"]["min"], MIN_GENERATION_TIMEOUT_SECS);
        assert_eq!(v["AUDIT_RETENTION_DAYS"]["max"], MAX_AUDIT_RETENTION_DAYS);
        assert_eq!(v["UNSEAL_MINUTES"]["max"], 720);
        assert_eq!(v["UNSEAL_MINUTES"]["min"], MIN_UNSEAL_MINUTES);
        assert_eq!(v["DISPLAY_WIDTH_PX"]["min"], MIN_DISPLAY_WIDTH_PX);
        assert_eq!(v["DISPLAY_TOP_PERCENT"]["max"], MAX_DISPLAY_TOP_PERCENT);
        assert_eq!(v["SERVICES"].as_array().unwrap().len(), SERVICES.len());
        assert_eq!(v["BACKOFF_SECS"], serde_json::json!([10, 20, 40, 60]));
        assert_eq!(v["REFERENCE_ROOT"], "docs/reference");
        assert!(LICENSE_URL.starts_with(SOURCE_URL), "the licence is in the source repository");
    }

    #[test]
    fn the_json_holds_no_secret() {
        // The published first-run password stays in `web` and is never served.
        let upper = json().to_uppercase();
        for word in ["DEFAULT_PASSWORD", "SECRET", "TOKEN", "PASSPHRASE\":", "PRIVATE"] {
            assert!(!upper.contains(word), "{word}");
        }
    }
}
