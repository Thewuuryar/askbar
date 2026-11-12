//! The real binary, run as the owner of a throwaway directory (`ASKBAR_BOOTSTRAP_ROOT`, development only).
//! Running it here also builds `target/debug/askbar-bootstrap`, which `scripts/smoke.sh` and the browser test use.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("askbar-bootstrap-bin-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_askbar-bootstrap")).args(args).env("ASKBAR_BOOTSTRAP_ROOT", root).output().unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn it_creates_keeps_lists_and_rotates() {
    let root = temp("flow");
    let token_file = root.join("admin-token/call/admin-server.token");
    let hash_file = root.join("server-token/accept/admin-server.sha256");

    let first = run(&root, &[]);
    assert!(first.status.success());
    assert_eq!(out(&first), "created admin-server\ncreated admin-auditor-append\ncreated admin-auditor-read\ncreated server-model-gateway\ncreated admin-model-gateway\ncreated model-gateway-auditor\ncreated server-auditor\ncreated model-gateway-auditor-read\ncreated auditor-alerter\ncreated admin-alerter\ncreated alerter-auditor\ncreated admin-auditor-restore\n");
    let token = std::fs::read_to_string(&token_file).unwrap();
    assert!(token.starts_with("mgt_"));
    assert_eq!(std::fs::read_to_string(&hash_file).unwrap().trim().len(), 64);

    let again = run(&root, &[]);
    assert_eq!(out(&again), "kept admin-server\nkept admin-auditor-append\nkept admin-auditor-read\nkept server-model-gateway\nkept admin-model-gateway\nkept model-gateway-auditor\nkept server-auditor\nkept model-gateway-auditor-read\nkept auditor-alerter\nkept admin-alerter\nkept alerter-auditor\nkept admin-auditor-restore\n");
    assert_eq!(std::fs::read_to_string(&token_file).unwrap(), token, "a rerun keeps the token");

    assert_eq!(
        out(&run(&root, &["list"])),
        "admin-server\nadmin-auditor-append\nadmin-auditor-read\nserver-model-gateway\nadmin-model-gateway\nmodel-gateway-auditor\nserver-auditor\nmodel-gateway-auditor-read\nauditor-alerter\nadmin-alerter\nalerter-auditor\nadmin-auditor-restore\n"
    );

    let rotated = run(&root, &["rotate", "admin-server"]);
    assert!(rotated.status.success());
    assert_eq!(out(&rotated), "rotated admin-server\nRestart from the admin: admin, server\n");
    assert_ne!(std::fs::read_to_string(&token_file).unwrap(), token);
}

#[test]
fn an_unknown_link_exits_1_with_a_sentence_and_prints_no_token() {
    let root = temp("unknown");
    assert!(run(&root, &[]).status.success());
    let token = std::fs::read_to_string(root.join("admin-token/call/admin-server.token")).unwrap();
    let bad = run(&root, &["rotate", "admin-nowhere"]);
    assert_eq!(bad.status.code(), Some(1));
    let err = String::from_utf8_lossy(&bad.stderr).into_owned();
    assert!(err.contains("no link named admin-nowhere"), "{err}");
    assert!(!err.contains(&token) && !out(&bad).contains(&token), "a token is never printed");
    assert_eq!(run(&root, &["frobnicate"]).status.code(), Some(1));
}
