//! `bootstrap`: creates the per-link tokens and exits (ADR 0049, ADR 0004).
//!
//! - [`cli`]: create, list and rotate.
//! - [`links`]: the table of links it makes tokens for.
//! - [`write`]: the token and hash files, with the owner and mode each reader needs.

pub mod cli;
pub mod links;
pub mod write;

/// For other crates' tests: the files bootstrap makes for the `admin-server` link, under `root`, owned by
/// the current user. Returns the token file (the admin's) and the hash file (the server's).
pub fn install_for_tests(root: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let ctx = write::Ctx::current_user(root).expect("the test directory is usable");
    // The admin starts only with its token for each service it talks to: the server's, the gateway's and the auditor's.
    let links = [
        links::link("admin-server").expect("admin-server is a link"),
        links::link("admin-model-gateway").expect("a link"),
        links::link("admin-auditor-read").expect("a link"),
    ];
    write::apply(&ctx, &links, false).expect("bootstrap writes its files");
    (root.join("admin-token/call/admin-server.token"), root.join("server-token/accept/admin-server.sha256"))
}
