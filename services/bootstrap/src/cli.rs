//! The command line (ADR 0049): create what is missing, `list`, and `rotate`.

use crate::links::{Link, LINKS};
use crate::write::{apply, Ctx};
use std::collections::BTreeSet;
use std::io::{Result, Write};

pub const USAGE: &str = "usage: askbar-bootstrap [list | rotate [--service <name> | <link>]]";

#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    All,
    Link(String),
    Service(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// No arguments: make what is missing.
    Create,
    List,
    Rotate(Target),
}

pub fn parse(args: &[String]) -> std::result::Result<Command, String> {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match a.as_slice() {
        [] => Ok(Command::Create),
        ["list"] => Ok(Command::List),
        ["rotate"] => Ok(Command::Rotate(Target::All)),
        ["rotate", "--service", name] => Ok(Command::Rotate(Target::Service(name.to_string()))),
        ["rotate", link] if !link.starts_with('-') => Ok(Command::Rotate(Target::Link(link.to_string()))),
        _ => Err(USAGE.to_string()),
    }
}

/// The links a target names, from `table`. An unknown link or service is an error.
pub fn select(table: &'static [Link], target: &Target) -> std::result::Result<Vec<&'static Link>, String> {
    let names = |f: &dyn Fn(&Link) -> String| table.iter().map(f).collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>().join(", ");
    match target {
        Target::All => Ok(table.iter().collect()),
        Target::Link(name) => match table.iter().find(|l| l.name == name) {
            Some(l) => Ok(vec![l]),
            None => Err(format!("no link named {name}; the links are: {}", names(&|l| l.name.to_string()))),
        },
        Target::Service(name) => {
            let picked: Vec<&Link> = table.iter().filter(|l| l.caller == name || l.receiver == name).collect();
            if picked.is_empty() {
                let services = names(&|l| format!("{}, {}", l.caller, l.receiver));
                return Err(format!("no link uses a service named {name}; the services are: {services}"));
            }
            Ok(picked)
        }
    }
}

/// The services to restart after `links` changed: the caller reads its token again, the receiver its hash.
pub fn restart(links: &[&Link]) -> Vec<&'static str> {
    links.iter().flat_map(|l| [l.caller, l.receiver]).collect::<BTreeSet<_>>().into_iter().collect()
}

/// Runs `cmd` on `table` and prints what happened to `out`. The error is what to tell the owner.
pub fn run(ctx: &Ctx, table: &'static [Link], cmd: Command, out: &mut impl Write) -> std::result::Result<(), String> {
    let io = |e: std::io::Error| e.to_string();
    let say = |r: Result<()>| r.map_err(io);
    match cmd {
        Command::List => {
            for l in table {
                say(writeln!(out, "{}", l.name))?;
            }
        }
        Command::Create => {
            let links = select(table, &Target::All)?;
            let report = apply(ctx, &links, false).map_err(io)?;
            for n in &report.written {
                say(writeln!(out, "created {n}"))?;
            }
            for n in &report.kept {
                say(writeln!(out, "kept {n}"))?;
            }
        }
        Command::Rotate(target) => {
            let links = select(table, &target)?;
            let report = apply(ctx, &links, true).map_err(io)?;
            for n in &report.written {
                say(writeln!(out, "rotated {n}"))?;
            }
            say(writeln!(out, "Restart from the admin: {}", restart(&links).join(", ")))?;
        }
    }
    Ok(())
}

/// What `main` runs.
pub fn main_with(args: &[String]) -> std::result::Result<(), String> {
    let cmd = parse(args)?;
    let ctx = match std::env::var("ASKBAR_BOOTSTRAP_ROOT").ok().filter(|r| !r.is_empty()) {
        Some(root) => Ctx { root: root.into(), ..Ctx::container() },
        None => Ctx::container(),
    };
    run(&ctx, LINKS, cmd, &mut std::io::stdout())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    static TABLE: [Link; 3] = [
        Link { name: "admin-server", caller: "admin", receiver: "server", prefix: "mgt_" }, // names-ok: test fixture
        Link { name: "admin-auditor-read", caller: "admin", receiver: "auditor", prefix: "adr_" }, // names-ok: test fixture
        Link { name: "server-auditor", caller: "server", receiver: "auditor", prefix: "sar_" }, // names-ok: test fixture
    ];

    fn s(a: &[&str]) -> Vec<String> {
        a.iter().map(|x| x.to_string()).collect()
    }

    fn ctx(root: &Path) -> Ctx {
        let me = std::fs::metadata(root).unwrap().uid();
        Ctx { root: root.to_path_buf(), taker: me, uid_of: Box::new(move |_| Some(me)) }
    }

    fn temp() -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("askbar-bootstrap-cli-{}-{}", std::process::id(), crate::write::new_token("t_").unwrap()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Every token and hash, by file name.
    fn files(root: &Path) -> std::collections::BTreeMap<String, String> {
        let mut out = std::collections::BTreeMap::new();
        for vol in std::fs::read_dir(root).unwrap() {
            for sub in ["call", "accept"] {
                if let Ok(dir) = std::fs::read_dir(vol.as_ref().unwrap().path().join(sub)) {
                    for f in dir {
                        let p = f.unwrap().path();
                        out.insert(p.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read_to_string(&p).unwrap());
                    }
                }
            }
        }
        out
    }

    fn changed(before: &std::collections::BTreeMap<String, String>, after: &std::collections::BTreeMap<String, String>) -> Vec<String> {
        after.iter().filter(|(k, v)| before.get(*k) != Some(v)).map(|(k, _)| k.clone()).collect()
    }

    #[test]
    fn arguments_parse() {
        assert_eq!(parse(&[]), Ok(Command::Create));
        assert_eq!(parse(&s(&["list"])), Ok(Command::List));
        assert_eq!(parse(&s(&["rotate"])), Ok(Command::Rotate(Target::All)));
        assert_eq!(parse(&s(&["rotate", "admin-server"])), Ok(Command::Rotate(Target::Link("admin-server".into()))));
        assert_eq!(parse(&s(&["rotate", "--service", "server"])), Ok(Command::Rotate(Target::Service("server".into()))));
        for bad in [&["rotate", "--service"][..], &["rotate", "a", "b"], &["nope"], &["rotate", "--all"]] {
            assert_eq!(parse(&s(bad)), Err(USAGE.to_string()), "{bad:?}");
        }
    }

    #[test]
    fn rotate_changes_exactly_the_named_link() {
        let root = temp();
        let mut sink = Vec::new();
        run(&ctx(&root), &TABLE, Command::Create, &mut sink).unwrap();
        let before = files(&root);
        assert_eq!(before.len(), 6);
        sink.clear();
        run(&ctx(&root), &TABLE, Command::Rotate(Target::Link("admin-auditor-read".into())), &mut sink).unwrap();
        let after = files(&root);
        assert_eq!(changed(&before, &after), ["admin-auditor-read.sha256", "admin-auditor-read.token"]);
        assert_eq!(String::from_utf8(sink).unwrap(), "rotated admin-auditor-read\nRestart from the admin: admin, auditor\n");
    }

    #[test]
    fn rotate_service_picks_every_link_of_that_service() {
        let picked: Vec<&str> = select(&TABLE, &Target::Service("server".into())).unwrap().iter().map(|l| l.name).collect();
        assert_eq!(picked, ["admin-server", "server-auditor"]);
        let root = temp();
        let mut sink = Vec::new();
        run(&ctx(&root), &TABLE, Command::Create, &mut sink).unwrap();
        let before = files(&root);
        run(&ctx(&root), &TABLE, Command::Rotate(Target::Service("server".into())), &mut sink).unwrap();
        let after = files(&root);
        assert_eq!(
            changed(&before, &after),
            ["admin-server.sha256", "admin-server.token", "server-auditor.sha256", "server-auditor.token"]
        );
        assert_eq!(restart(&select(&TABLE, &Target::Service("server".into())).unwrap()), ["admin", "auditor", "server"]);
    }

    #[test]
    fn rotate_all_changes_every_link() {
        let root = temp();
        let mut sink = Vec::new();
        run(&ctx(&root), &TABLE, Command::Create, &mut sink).unwrap();
        let before = files(&root);
        run(&ctx(&root), &TABLE, Command::Rotate(Target::All), &mut sink).unwrap();
        assert_eq!(changed(&before, &files(&root)).len(), 6);
    }

    #[test]
    fn an_unknown_link_or_service_is_an_error_and_changes_nothing() {
        let root = temp();
        let mut sink = Vec::new();
        run(&ctx(&root), &TABLE, Command::Create, &mut sink).unwrap();
        let before = files(&root);
        let e = run(&ctx(&root), &TABLE, Command::Rotate(Target::Link("admin-nowhere".into())), &mut sink).unwrap_err();
        assert!(e.contains("no link named admin-nowhere") && e.contains("admin-server"), "{e}");
        let e = run(&ctx(&root), &TABLE, Command::Rotate(Target::Service("nowhere".into())), &mut sink).unwrap_err();
        assert!(e.contains("nowhere"), "{e}");
        assert_eq!(files(&root), before);
    }

    #[test]
    fn list_prints_the_link_names() {
        let mut out = Vec::new();
        run(&ctx(&temp()), &TABLE, Command::List, &mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "admin-server\nadmin-auditor-read\nserver-auditor\n");
    }
}
