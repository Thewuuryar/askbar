//! The links `bootstrap` makes tokens for (ADR 0049). The Links table in `docs/architecture.md` is the
//! source of truth; a unit test checks every row here against it.

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

/// Only links whose receiver exists: the auditor links come in step 2.4, `model-gateway` in phase 3,
/// `alerter` in phase 4.
pub const LINKS: &[Link] = &[
    Link { name: "admin-server", caller: "admin", receiver: "server", prefix: "mgt_" }, // names-ok: the token prefix already issued
];

/// A service with a credential volume `<name>-token`, and the uid that reads it (ADR 0003).
pub const SERVICES: &[(&str, u32)] = &[("admin", 10001), ("server", 10002)];

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
