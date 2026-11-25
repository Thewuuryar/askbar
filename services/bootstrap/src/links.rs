//! The links `bootstrap` makes tokens for (ADR 0049). The Links table in `docs/architecture.md` is the
//! source of truth; a unit test checks every row here against it.

use askbar_proto::links::*;

/// One authenticated path from a caller to a receiver. Its token is sent by the caller and its hash
/// is checked by the receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    /// `<caller>-<receiver>`, with a suffix where a caller has two links to one receiver.
    pub name: &'static str,
    pub caller: &'static str,
    pub receiver: &'static str,
    /// What every token of this link starts with, so a leaked token names its link.
    pub prefix: &'static str,
}

/// Only links whose receiver exists: `admin-server` (step 2.1), the auditor's two admin links (2.4), and the
/// three `model-gateway` links (3.1, ADR 0054), the server's link to the auditor (3.1, second half), and the
/// alerter's three links (4.1, ADR 0055) the auditor's restore link (6.3, ADR 0013) the egress proxy's link (4.1c, ADR 0056) and the MCP gateway's two (5A.6a, ADR 0057).
pub const LINKS: &[Link] = &[
    Link { name: ADMIN_SERVER, caller: "admin", receiver: "server", prefix: "mgt_" }, // names-ok: the token prefix already issued
    Link { name: ADMIN_AUDITOR_APPEND, caller: "admin", receiver: "auditor", prefix: "audapp_" },
    Link { name: ADMIN_AUDITOR_READ, caller: "admin", receiver: "auditor", prefix: "audrd_" },
    Link { name: SERVER_MODEL_GATEWAY, caller: "server", receiver: "model-gateway", prefix: "srvgw_" },
    Link { name: ADMIN_MODEL_GATEWAY, caller: "admin", receiver: "model-gateway", prefix: "admgw_" },
    Link { name: MODEL_GATEWAY_AUDITOR, caller: "model-gateway", receiver: "auditor", prefix: "gwaud_" },
    Link { name: SERVER_AUDITOR, caller: "server", receiver: "auditor", prefix: "srvaud_" },
    // The gateway reads the day's and month's token counts to hold the budget (plan step 3.2, ADR 0014): `GET /token-counts`
    // and nothing else.
    Link { name: MODEL_GATEWAY_AUDITOR_READ, caller: "model-gateway", receiver: "auditor", prefix: "gwrd_" },
    // The alerter (plan step 4.1, ADR 0055): the auditor sends it events, the admin configures it and sends its own events,
    // and it appends the outcome of each alert to the auditor.
    Link { name: AUDITOR_ALERTER, caller: "auditor", receiver: "alerter", prefix: "audalr_" },
    Link { name: ADMIN_ALERTER, caller: "admin", receiver: "alerter", prefix: "admalr_" },
    Link { name: ALERTER_AUDITOR, caller: "alerter", receiver: "auditor", prefix: "alraud_" },
    // The one way records reach the auditor other than an append (plan step 6.3, ADR 0013, ADR 0010): `POST /restore` and nothing else,
    // into an empty store. The admin holds this token and uses it only for a restore.
    Link { name: ADMIN_AUDITOR_RESTORE, caller: "admin", receiver: "auditor", prefix: "audrs_" },
    // The egress proxy (plan step 4.1c, ADR 0056): the admin pushes its allowlists and reads its status.
    Link { name: ADMIN_EGRESS_PROXY, caller: "admin", receiver: "egress-proxy", prefix: "admegr_" },
    // The MCP gateway (plan step 5A.6a, ADR 0057): the server calls its tools, the admin pushes its config.
    Link { name: SERVER_MCP_GATEWAY, caller: "server", receiver: "mcp-gateway", prefix: "srvmcp_" },
    Link { name: ADMIN_MCP_GATEWAY, caller: "admin", receiver: "mcp-gateway", prefix: "admmcp_" },
];

/// A service with a credential volume `<name>-token`, and the uid that reads it (ADR 0003).
pub const SERVICES: &[(&str, u32)] = &[
    ("admin", 10001),
    ("server", 10002),
    ("auditor", 10003),
    ("model-gateway", 10004),
    ("alerter", 10005),
    ("egress-proxy", 10006),
    ("mcp-gateway", 10007),
];

pub fn link(name: &str) -> Option<&'static Link> {
    LINKS.iter().find(|l| l.name == name)
}

pub fn uid_of(service: &str) -> Option<u32> {
    SERVICES.iter().find(|(n, _)| *n == service).map(|(_, uid)| *uid)
}

/// Every service named in `LINKS`, once, in table order.
pub fn services() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for l in LINKS {
        for s in [l.caller, l.receiver] {
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_file(path: &str) -> String {
        std::fs::read_to_string(format!("{}/../../{path}", env!("CARGO_MANIFEST_DIR"))).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    /// The names in backticks in one table cell.
    fn names(cell: &str) -> Vec<String> {
        cell.split('`').skip(1).step_by(2).map(str::to_string).collect()
    }

    #[test]
    fn every_link_is_a_row_of_the_links_table_in_architecture_md() {
        let doc = repo_file("docs/architecture.md");
        let section = doc.split("\n## Links").nth(1).expect("architecture.md has a Links section");
        let section = section.split("\n## ").next().unwrap();
        let rows: Vec<(Vec<String>, Vec<String>)> = section
            .lines()
            .filter(|l| l.starts_with('|'))
            .map(|l| {
                let cells: Vec<&str> = l.split('|').map(str::trim).collect();
                (names(cells[1]), names(cells[2]))
            })
            .collect();
        assert!(rows.len() > 5, "the Links table was not found");
        for l in LINKS {
            assert!(
                rows.iter().any(|(from, to)| from.iter().any(|f| f == l.caller) && to.iter().any(|t| t == l.receiver)),
                "link {} ({} to {}) has no row in the Links table of docs/architecture.md",
                l.name,
                l.caller,
                l.receiver
            );
        }
    }

    #[test]
    fn names_are_unique_and_follow_the_pattern() {
        for (i, l) in LINKS.iter().enumerate() {
            assert!(l.name.starts_with(&format!("{}-{}", l.caller, l.receiver)), "{}", l.name);
            assert!(l.prefix.ends_with('_') && l.prefix.len() >= 3, "{}", l.prefix);
            assert!(LINKS[..i].iter().all(|o| o.name != l.name && o.prefix != l.prefix), "{} repeats a name or prefix", l.name);
            assert!(uid_of(l.caller).is_some() && uid_of(l.receiver).is_some(), "{} names a service with no uid", l.name);
        }
    }

    #[test]
    fn uids_match_the_dockerfile() {
        let dockerfile = repo_file("Dockerfile");
        for (name, id) in SERVICES {
            // The message names the service; the number is in the table above.
            assert!(dockerfile.contains(&format!("--uid {id} ")), "no stage of the Dockerfile runs {name} as the user the table gives it");
        }
    }
}
