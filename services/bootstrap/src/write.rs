//! Writing the token and hash files with the owner and mode each reader needs (ADR 0049).
//!
//! Every container drops all capabilities, so `bootstrap` (root, with `CHOWN` only) cannot read or
//! write a directory that belongs to another uid. So for each credential volume it takes the directory
//! over (owner: itself, mode 0700), works, then gives every file and directory to the service that
//! reads them. A volume that was left taken over (a crash) is taken again by the next run.
//!
//! Layout, under `<root>/<service>-token/`:
//! - `call/<link>.token`: a token this service sends. Owner: this service, mode 0400.
//! - `accept/<link>.sha256`: the hash of a token this service checks. Owner: this service, mode 0444.
//! - each directory: owner this service, mode 0500.

use crate::links::{self, Link};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::{Error, ErrorKind, Read, Result, Write};
use std::os::unix::fs::{chown, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const TOKEN_MODE: u32 = 0o400;
pub const HASH_MODE: u32 = 0o444;
pub const DIR_MODE: u32 = 0o500;

/// The uid that reads a service's volume, by service name.
pub type UidOf = Box<dyn Fn(&str) -> Option<u32>>;

/// Where the volumes are and whose they become.
pub struct Ctx {
    /// The directory holding `<service>-token`: `/` in the container.
    pub root: PathBuf,
    /// The owner of a directory while `bootstrap` works in it: root in the container.
    pub taker: u32,
    /// The uid that reads a service's volume.
    pub uid_of: UidOf,
}

impl Ctx {
    pub fn container() -> Ctx {
        Ctx { root: "/".into(), taker: 0, uid_of: Box::new(links::uid_of) }
    }

    fn volume(&self, service: &str) -> PathBuf {
        self.root.join(format!("{service}-token"))
    }

    fn reader(&self, service: &str) -> Result<u32> {
        (self.uid_of)(service).ok_or_else(|| Error::new(ErrorKind::InvalidInput, format!("no uid for service {service}")))
    }
}

/// What a run did, by link name.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub written: Vec<&'static str>,
    pub kept: Vec<&'static str>,
}

fn io_err(path: &Path, e: Error) -> Error {
    Error::new(e.kind(), format!("{}: {e}", path.display()))
}

/// A new token: the prefix, then 32 bytes from the OS in unpadded base64url.
pub fn new_token(prefix: &str) -> Result<String> {
    let mut raw = [0u8; 32];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut raw)).map_err(|e| io_err(Path::new("/dev/urandom"), e))?;
    Ok(format!("{prefix}{}", URL_SAFE_NO_PAD.encode(raw)))
}

pub fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn token_name(l: &Link) -> String {
    format!("{}.token", l.name)
}

fn hash_name(l: &Link) -> String {
    format!("{}.sha256", l.name)
}

/// Makes `dir` if it is missing and takes it over.
fn take(ctx: &Ctx, dir: &Path) -> Result<()> {
    match std::fs::create_dir(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(e) => return Err(io_err(dir, e)),
    }
    chown(dir, Some(ctx.taker), None).map_err(|e| io_err(dir, e))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| io_err(dir, e))
}

/// Makes `path` `uid`'s with `mode`, leaving it alone when it already is. Otherwise it is taken over first:
/// with every capability but `CHOWN` dropped, `bootstrap` can change the mode of a file only while it owns it
/// (a rerun finds the files already given to their readers). Then the mode is set, then the file is given:
/// after the last `chown` it is no longer ours to change.
fn set(ctx: &Ctx, path: &Path, mode: u32, uid: u32) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).map_err(|e| io_err(path, e))?;
    if meta.uid() == uid && meta.mode() & 0o7777 == mode {
        return Ok(());
    }
    chown(path, Some(ctx.taker), None).map_err(|e| io_err(path, e))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|e| io_err(path, e))?;
    chown(path, Some(uid), None).map_err(|e| io_err(path, e))
}

/// Gives `dir` and the files in `call` and `accept` to `uid`, with their modes.
fn give(ctx: &Ctx, volume: &Path, uid: u32) -> Result<()> {
    for (sub, mode) in [("call", TOKEN_MODE), ("accept", HASH_MODE)] {
        let dir = volume.join(sub);
        for entry in std::fs::read_dir(&dir).map_err(|e| io_err(&dir, e))? {
            set(ctx, &entry.map_err(|e| io_err(&dir, e))?.path(), mode, uid)?;
        }
        set(ctx, &dir, DIR_MODE, uid)?;
    }
    set(ctx, volume, DIR_MODE, uid)
}

/// Takes `service`'s volume over, runs `f` in it, and gives it back, also when `f` fails.
fn with_volume<T>(ctx: &Ctx, service: &str, f: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
    let uid = ctx.reader(service)?;
    let volume = ctx.volume(service);
    take(ctx, &volume)?;
    let prepared = take(ctx, &volume.join("call")).and_then(|_| take(ctx, &volume.join("accept")));
    let result = prepared.and_then(|_| f(&volume));
    let given = give(ctx, &volume, uid);
    let out = result?;
    given?;
    Ok(out)
}

fn names_in(dir: &Path) -> Result<BTreeSet<String>> {
    std::fs::read_dir(dir)
        .map_err(|e| io_err(dir, e))?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()).map_err(|e| io_err(dir, e)))
        .collect()
}

fn put(path: &Path, contents: &str, mode: u32) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(io_err(path, e)),
    }
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(path).map_err(|e| io_err(path, e))?;
    f.write_all(contents.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| io_err(path, e))
}

/// Makes the files of `links`. With `replace`, every link gets a new token; without it, only a link
/// that lacks its token or its hash does, and a complete one is left as it is.
pub fn apply(ctx: &Ctx, links: &[&'static Link], replace: bool) -> Result<Report> {
    let services: BTreeSet<&str> = links.iter().flat_map(|l| [l.caller, l.receiver]).collect();
    // What is there now. Every volume of every service named is made and given to its reader.
    let mut have: std::collections::BTreeMap<&str, (BTreeSet<String>, BTreeSet<String>)> = Default::default();
    for s in &services {
        let sets = with_volume(ctx, s, |v| Ok((names_in(&v.join("call"))?, names_in(&v.join("accept"))?)))?;
        have.insert(s, sets);
    }
    let mut report = Report::default();
    let mut todo: Vec<(&'static Link, String)> = Vec::new();
    for l in links {
        let complete = have[l.caller].0.contains(&token_name(l)) && have[l.receiver].1.contains(&hash_name(l));
        if complete && !replace {
            report.kept.push(l.name);
        } else {
            todo.push((l, new_token(l.prefix)?));
            report.written.push(l.name);
        }
    }
    for s in &services {
        let mine: Vec<&(&'static Link, String)> = todo.iter().filter(|(l, _)| l.caller == *s || l.receiver == *s).collect();
        if mine.is_empty() {
            continue;
        }
        with_volume(ctx, s, |v| {
            for (l, token) in &mine {
                if l.receiver == *s {
                    put(&v.join("accept").join(hash_name(l)), &format!("{}\n", sha256_hex(token)), HASH_MODE)?;
                }
                if l.caller == *s {
                    put(&v.join("call").join(token_name(l)), token, TOKEN_MODE)?;
                }
            }
            Ok(())
        })?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::links::LINKS;
    use std::os::unix::fs::MetadataExt;

    fn temp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("askbar-bootstrap-{}-{}", std::process::id(), new_token("t_").unwrap()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// An ordinary user cannot give files to another uid, so the tests give them to themselves.
    fn ctx(root: &Path) -> Ctx {
        let me = std::fs::metadata(root).unwrap().uid();
        Ctx { root: root.to_path_buf(), taker: me, uid_of: Box::new(move |_| Some(me)) }
    }

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o7777
    }

    fn all(root: &Path) -> Vec<(PathBuf, u32, Vec<u8>)> {
        fn walk(p: &Path, out: &mut Vec<(PathBuf, u32, Vec<u8>)>) {
            let m = mode(p);
            if p.is_dir() {
                out.push((p.to_path_buf(), m, vec![]));
                // Directories are 0500: read them without needing write access.
                let mut kids: Vec<_> = std::fs::read_dir(p).unwrap().map(|e| e.unwrap().path()).collect();
                kids.sort();
                for k in kids {
                    walk(&k, out);
                }
            } else {
                out.push((p.to_path_buf(), m, std::fs::read(p).unwrap_or_default()));
            }
        }
        let mut out = vec![];
        walk(root, &mut out);
        out
    }

    fn links() -> Vec<&'static Link> {
        LINKS.iter().collect()
    }

    #[test]
    fn a_token_has_its_prefix_and_32_random_bytes() {
        let a = new_token("mgt_").unwrap();
        let b = new_token("mgt_").unwrap();
        assert_ne!(a, b);
        let body = a.strip_prefix("mgt_").unwrap();
        assert_eq!(body.len(), 43, "32 bytes in unpadded base64url");
        assert!(body.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'));
        assert_eq!(URL_SAFE_NO_PAD.decode(body).unwrap().len(), 32);
    }

    #[test]
    fn the_hash_file_holds_the_sha256_of_the_token() {
        let root = temp();
        apply(&ctx(&root), &links(), false).unwrap();
        for l in LINKS {
            let token = std::fs::read_to_string(root.join(format!("{}-token/call/{}.token", l.caller, l.name))).unwrap();
            let hash = std::fs::read_to_string(root.join(format!("{}-token/accept/{}.sha256", l.receiver, l.name))).unwrap();
            assert!(token.starts_with(l.prefix));
            assert_eq!(hash, format!("{}\n", sha256_hex(&token)));
            assert_eq!(hash.trim().len(), 64);
        }
    }

    #[test]
    fn files_and_directories_have_the_modes_their_readers_need() {
        let root = temp();
        apply(&ctx(&root), &links(), false).unwrap();
        for l in LINKS {
            let caller = root.join(format!("{}-token", l.caller));
            let receiver = root.join(format!("{}-token", l.receiver));
            assert_eq!(mode(&caller.join(format!("call/{}.token", l.name))), 0o400);
            assert_eq!(mode(&receiver.join(format!("accept/{}.sha256", l.name))), 0o444);
            for dir in [&caller, &receiver] {
                for d in [dir.clone(), dir.join("call"), dir.join("accept")] {
                    assert_eq!(mode(&d), 0o500, "{}", d.display());
                }
            }
        }
    }

    #[test]
    fn a_rerun_changes_nothing() {
        let root = temp();
        let first = apply(&ctx(&root), &links(), false).unwrap();
        assert_eq!(first.written, ["admin-server"]);
        let before = all(&root);
        let again = apply(&ctx(&root), &links(), false).unwrap();
        assert_eq!(again, Report { written: vec![], kept: vec!["admin-server"] });
        assert_eq!(all(&root), before);
    }

    #[test]
    fn a_link_missing_its_hash_gets_a_new_token_and_hash() {
        let root = temp();
        apply(&ctx(&root), &links(), false).unwrap();
        let hash = root.join("server-token/accept/admin-server.sha256");
        let old = std::fs::read(root.join("admin-token/call/admin-server.token")).unwrap();
        std::fs::set_permissions(root.join("server-token/accept"), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::remove_file(&hash).unwrap();
        apply(&ctx(&root), &links(), false).unwrap();
        let new = std::fs::read_to_string(root.join("admin-token/call/admin-server.token")).unwrap();
        assert_ne!(new.as_bytes(), old);
        assert_eq!(std::fs::read_to_string(&hash).unwrap().trim(), sha256_hex(&new));
    }
}
