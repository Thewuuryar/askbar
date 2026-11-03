//! Every URL path prefix, in one place. The rule is `/<audience>/<...>/v<N>/<resource>`: the
//! audience first and the version after it, so each API versions on its own. The three audiences
//! are the words in docs/architecture.md: client, server, admin.

/// The admin site, its files and its API. Served by `askbar-admin` through Caddy.
pub const ADMIN: &str = "/admin";
/// The admin API: the admin site's own calls, and anyone's script.
pub const ADMIN_V1: &str = "/admin/api/v1";
/// What an Askbar app (or any client) calls on the server. Served by `askbar-server`.
pub const CLIENT_V1: &str = "/api/client/v1";
/// What the admin calls on the server. Served only on the internal listener; Caddy answers 404.
pub const SERVER_V1: &str = "/api/server/v1";
/// What the admin calls on the controller (start, stop, restart). Internal network only; never routed.
pub const CONTROL_V1: &str = "/api/control/v1";
/// Every version of the server API, as Caddy blocks it.
pub const SERVER: &str = "/api/server";
/// Every version of the client API.
pub const CLIENT: &str = "/api/client";
/// Where the first generation of the client API lived; answered with 426 "update the app".
pub const TOMBSTONE_V1: &str = "/api/v1";

/// `prefix` plus `rest` (which starts with a slash): `join(CLIENT_V1, "/chat")`.
pub fn join(prefix: &str, rest: &str) -> String {
    debug_assert!(rest.starts_with('/') && !prefix.ends_with('/'));
    format!("{prefix}{rest}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_prefix_is_audience_first_then_the_version_and_has_no_trailing_slash() {
        for p in [ADMIN, ADMIN_V1, CLIENT_V1, SERVER_V1, CONTROL_V1, SERVER, CLIENT, TOMBSTONE_V1] {
            assert!(p.starts_with('/') && !p.ends_with('/'), "{p}");
        }
        // The versioned prefixes sit under their audience prefix, with the version after it.
        assert!(CLIENT_V1.starts_with(&format!("{CLIENT}/v")));
        assert!(SERVER_V1.starts_with(&format!("{SERVER}/v")));
        assert!(ADMIN_V1.starts_with(&format!("{ADMIN}/api/v")));
    }

    #[test]
    fn the_server_api_is_never_under_a_prefix_the_client_api_uses() {
        assert!(!SERVER_V1.starts_with(CLIENT) && !CLIENT_V1.starts_with(SERVER));
        assert!(!ADMIN_V1.starts_with("/api"));
    }

    #[test]
    fn join_puts_the_resource_after_the_version() {
        assert_eq!(join(CLIENT_V1, "/chat"), "/api/client/v1/chat");
        assert_eq!(join(SERVER_V1, "/snapshot"), "/api/server/v1/snapshot");
        assert_eq!(join(ADMIN_V1, "/providers"), "/admin/api/v1/providers");
    }
}
