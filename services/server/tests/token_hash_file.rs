//! The start-up check for the hash file `bootstrap` makes (ADR 0049).

use askbar_server::server_api::{check_token_hash_file, token_hash_path};

fn temp() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("askbar-hashfile-{}", askbar_server::ids::new_id()));
    std::fs::create_dir_all(d.join("accept")).unwrap();
    d
}

#[test]
fn the_default_is_the_file_bootstrap_makes_for_the_admin_link() {
    if std::env::var_os("ASKBAR_SERVER_TOKEN_HASH_FILE").is_none() {
        assert_eq!(token_hash_path(), std::path::PathBuf::from("/server-token/accept/admin-server.sha256"));
    }
}

#[test]
fn the_file_from_bootstrap_passes() {
    let d = temp();
    let f = d.join("accept/admin-server.sha256");
    std::fs::write(&f, "0".repeat(64)).unwrap();
    assert_eq!(check_token_hash_file(&f), Ok(()));
}

#[test]
fn an_install_with_only_the_old_file_is_told_to_run_compose_up() {
    let d = temp();
    std::fs::write(d.join("token.sha256"), "0".repeat(64)).unwrap();
    let e = check_token_hash_file(&d.join("accept/admin-server.sha256")).unwrap_err();
    assert!(e.contains("old token.sha256") && e.contains("docker compose up"), "{e}");
}

#[test]
fn with_no_file_at_all_it_says_bootstrap_makes_it() {
    let d = temp();
    let e = check_token_hash_file(&d.join("accept/admin-server.sha256")).unwrap_err();
    assert!(e.contains("bootstrap") && e.contains("docker compose up"), "{e}");
    assert!(!e.contains("old token.sha256"), "{e}");
}

#[test]
fn repeated_invalid_tokens_on_the_server_api_are_reported_once_naming_its_link_and_never_a_token() {
    let d = temp();
    let links = askbar_server::server_api::links(d.join("accept/admin-server.sha256"));
    let now = std::time::Instant::now();
    let lines: Vec<String> = (0..12).filter_map(|i| links.note_invalid(now + std::time::Duration::from_secs(i))).collect();
    assert_eq!(lines.len(), 1, "one report for the episode: {lines:?}");
    assert!(lines[0].contains("5 invalid tokens in 10 minutes") && lines[0].contains("admin-server"), "{}", lines[0]);
}
