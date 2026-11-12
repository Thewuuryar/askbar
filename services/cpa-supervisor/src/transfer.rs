//! Export and import of the account login files (ADR 0013, ADR 0037; plan step 6.2).
//!
//! The admin asks by writing a request file to `cpa-config`; the supervisor answers with a `-done` file naming the request,
//! as it does for a restart. No port, no token.
//!
//! - **Export.** `export-request` is JSON: `{"id": "<unique>", "recipient": "age1…"}`. The supervisor archives the regular
//!   files directly in the auth directory (`tar`), encrypts the archive to the recipient and writes it to `export.age`
//!   (a temporary file, then a rename), then writes `export-done`: `{"id": "<id>", "ok": true, "bytes": n}`, or
//!   `{"id": "<id>", "ok": false}`. The supervisor never holds the private key.
//! - **Import.** `import-request` is JSON: `{"id": "<unique>"}`, with the plain archive of login files in `import.tar`
//!   (the admin has opened it with the owner's key). The supervisor stops CLIProxyAPI, unpacks the files into the auth
//!   directory, restarts it, removes `import.tar`, and writes `import-done` the same way.
//! - A request is acted on once: it is done when its `id` is the one in the done file. Nothing about the files is logged.

use std::path::{Path, PathBuf};

/// The files the two requests use, all in `cpa-config` except the auth directory.
#[derive(Clone, Debug)]
pub struct Paths {
    pub auth: PathBuf,
    pub export_request: PathBuf,
    pub export_done: PathBuf,
    pub export_file: PathBuf,
    pub import_request: PathBuf,
    pub import_done: PathBuf,
    pub import_file: PathBuf,
}

impl Paths {
    pub fn for_dirs(config: &Path, auth: &Path) -> Paths {
        Paths {
            auth: auth.to_path_buf(),
            export_request: config.join("export-request"),
            export_done: config.join("export-done"),
            export_file: config.join("export.age"),
            import_request: config.join("import-request"),
            import_done: config.join("import-done"),
            import_file: config.join("import.tar"),
        }
    }
}

/// The `id` in a request or done file.
fn id_of(bytes: &[u8]) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(bytes).ok()?.get("id")?.as_str().filter(|s| !s.is_empty()).map(String::from)
}

/// A request not yet answered: its id, and the request itself.
pub fn pending(request: &Path, done: &Path) -> Option<serde_json::Value> {
    let bytes = std::fs::read(request).ok()?;
    let id = id_of(&bytes)?;
    if std::fs::read(done).ok().and_then(|d| id_of(&d)).as_deref() == Some(id.as_str()) {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn answer(done: &Path, id: &str, ok: bool, bytes: Option<u64>) {
    let mut v = serde_json::json!({ "id": id, "ok": ok });
    if let Some(b) = bytes {
        v["bytes"] = b.into();
    }
    let tmp = done.with_extension("tmp");
    if std::fs::write(&tmp, v.to_string()).and_then(|_| std::fs::rename(&tmp, done)).is_err() {
        eprintln!("askbar-cpa-supervisor: could not write {}", done.display());
    }
}

/// The regular files directly in `dir`, by name.
fn files_in(dir: &Path) -> std::io::Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if e.file_type()?.is_file() && askbar_service::tar::is_plain_name(&name) {
            out.push((name, e.path()));
        }
    }
    out.sort();
    Ok(out)
}

fn export_to(p: &Paths, recipient: &str, now: i64) -> Result<u64, Box<dyn std::error::Error>> {
    let tmp = p.export_file.with_extension("tmp");
    let file = std::fs::File::create(&tmp)?;
    let mut enc = askbar_service::backup::encrypt_to(recipient, file)?;
    for (name, path) in files_in(&p.auth)? {
        let size = std::fs::metadata(&path)?.len();
        askbar_service::tar::write_file(&mut enc, &name, size, &mut std::fs::File::open(&path)?, now)?;
    }
    askbar_service::tar::write_end(&mut enc)?;
    let file = enc.finish()?;
    file.sync_all()?;
    let bytes = file.metadata()?.len();
    std::fs::rename(&tmp, &p.export_file)?;
    Ok(bytes)
}

/// Acts on a pending export request. Returns whether there was one.
pub fn export(p: &Paths, now: i64) -> bool {
    let Some(req) = pending(&p.export_request, &p.export_done) else { return false };
    let id = req["id"].as_str().unwrap_or_default().to_string();
    let _ = std::fs::remove_file(&p.export_file);
    match req["recipient"].as_str().map(|r| export_to(p, r, now)) {
        Some(Ok(bytes)) => answer(&p.export_done, &id, true, Some(bytes)),
        _ => {
            eprintln!("askbar-cpa-supervisor: the export failed");
            answer(&p.export_done, &id, false, None);
        }
    }
    true
}

/// A pending import request, if there is one.
pub fn import_pending(p: &Paths) -> Option<String> {
    pending(&p.import_request, &p.import_done).and_then(|r| r["id"].as_str().map(String::from))
}

/// Unpacks `import.tar` into the auth directory (the child is stopped by the caller). Returns whether it worked, and answers.
pub fn import(p: &Paths, id: &str) -> bool {
    let ok = (|| -> Option<()> {
        let files = askbar_service::tar::read(&std::fs::read(&p.import_file).ok()?)?;
        // Plain names with no directory: the archive is the auth directory's own files.
        if files.iter().any(|(n, _)| n.contains('/')) {
            return None;
        }
        std::fs::create_dir_all(&p.auth).ok()?;
        for (name, data) in &files {
            let tmp = p.auth.join(format!(".{name}.tmp"));
            std::fs::write(&tmp, data).ok()?;
            std::fs::rename(&tmp, p.auth.join(name)).ok()?;
        }
        Some(())
    })()
    .is_some();
    let _ = std::fs::remove_file(&p.import_file);
    if !ok {
        eprintln!("askbar-cpa-supervisor: the import failed");
    }
    answer(&p.import_done, id, ok, None);
    ok
}
