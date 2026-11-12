//! The daily release check (ADR 0055, ADR 0017, D11): ask GitHub's latest-release API for the newest version, and remember the
//! answer for the admin to read and for one `release_available` alert per version.
//!
//! The request carries a fixed `User-Agent` and nothing about the install: no version, no host name, no identifier. The answer is
//! one version, checked to be plain digits before it is kept, so nothing GitHub says can reach an alert or a page as text. The
//! state is a small JSON file on the alerter's volume, written atomically; a file that cannot be read starts the state empty.

use askbar_proto::alerter::ReleaseStatus;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// The repository is public (ADR 0017).
pub const DEFAULT_URL: &str = "https://api.github.com/repos/Thewuuryar/askbar/releases/latest";
/// What `ASKBAR_ALERTER_RELEASES_URL` is set to for no check at all.
pub const OFF: &str = "off";
/// The same for every install, and never anything about it.
pub const USER_AGENT: &str = "askbar-release-check";
pub const FILE: &str = "releases.json";
const TIMEOUT: Duration = Duration::from_secs(15);
const MAX_BODY: usize = 256 * 1024;
/// A check that worked is repeated after a day, and one that failed after an hour.
pub const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
pub const RETRY: Duration = Duration::from_secs(60 * 60);

/// What is kept between starts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub checked_at: Option<i64>,
    pub failed_at: Option<i64>,
    pub latest: Option<String>,
    /// The version an alert was queued for, so a version is announced once and a restart does not repeat it.
    pub notified: Option<String>,
}

/// Where to ask, and what this install is.
#[derive(Debug, Clone)]
pub struct Source {
    pub url: String,
    pub current: String,
    pub every: Duration,
    pub retry: Duration,
}

pub struct Checker {
    path: PathBuf,
    source: Mutex<Source>,
    saved: Mutex<Saved>,
}

/// `1.2.3` or `v1.2.3` as numbers. A pre-release (`1.0.0-rc1`), a build suffix or anything else is not a release.
pub fn parse(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.strip_prefix('v').unwrap_or(v);
    let mut parts = v.split('.');
    let mut next = || {
        let p = parts.next()?;
        (!p.is_empty() && p.len() <= 9 && p.bytes().all(|b| b.is_ascii_digit())).then(|| p.parse::<u64>().ok()).flatten()
    };
    let out = (next()?, next()?, next()?);
    parts.next().is_none().then_some(out)
}

/// Whether `latest` is a newer release than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse(latest), parse(current)), (Some(l), Some(c)) if l > c)
}

/// The version in a latest-release answer: `tag_name`, without a leading `v`, if it is a plain release and not a draft or a
/// pre-release.
pub fn version_in(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
        return None;
    }
    let tag = v["tag_name"].as_str()?;
    parse(tag)?;
    Some(tag.strip_prefix('v').unwrap_or(tag).to_string())
}

impl Checker {
    pub fn new(dir: &Path, url: String, current: String) -> Checker {
        let path = dir.join(FILE);
        let saved = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| {
            if path.exists() {
                tracing::warn!("the release state is not readable; starting it empty");
            }
            Saved::default()
        });
        Checker { path, source: Mutex::new(Source { url, current, every: EVERY, retry: RETRY }), saved: Mutex::new(saved) }
    }

    pub fn source(&self) -> Source {
        self.source.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Replaces where to ask and what this install is (tests point it at a mock and shorten the waits).
    pub fn set_source(&self, s: Source) {
        *self.source.lock().unwrap_or_else(|e| e.into_inner()) = s;
    }

    pub fn saved(&self) -> Saved {
        self.saved.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn enabled(&self) -> bool {
        self.source().url != OFF
    }

    /// Makes `s` the state, kept and in memory (a restore, ADR 0013).
    pub fn replace(&self, s: Saved) {
        self.update(|saved| *saved = s);
    }

    fn update(&self, f: impl FnOnce(&mut Saved)) {
        let snapshot = {
            let mut s = self.saved.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut s);
            s.clone()
        };
        if let Err(e) = self.write(&snapshot) {
            tracing::warn!("the release state could not be saved: {}", e.kind());
        }
    }

    /// A temporary file in the same directory, synced and renamed over the state, so a crash leaves the old file or the new.
    fn write(&self, s: &Saved) -> std::io::Result<()> {
        let dir = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".{FILE}.tmp"));
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(serde_json::to_string(s).map_err(std::io::Error::other)?.as_bytes())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &self.path)
    }

    /// Seconds until the next check is due at `now`; 0 when it is due.
    pub fn due_in(&self, now: i64) -> u64 {
        let (src, s) = (self.source(), self.saved());
        let (last, wait) = match (s.checked_at, s.failed_at) {
            (None, None) => return 0,
            (Some(ok), Some(failed)) if failed > ok => (failed, src.retry),
            (Some(ok), _) => (ok, src.every),
            (None, Some(failed)) => (failed, src.retry),
        };
        (last.saturating_add(wait.as_secs() as i64) - now).max(0) as u64
    }

    /// Asks the API once and keeps what it said, or that it failed. Never says why: the answer is not trusted with a sentence.
    pub async fn check(&self, http: &reqwest::Client, now: i64) -> bool {
        let url = self.source().url;
        let got = async {
            let resp = http
                .get(&url)
                .timeout(TIMEOUT)
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .header(reqwest::header::ACCEPT, "application/vnd.github+json")
                .send()
                .await
                .ok()?;
            if !resp.status().is_success() || resp.content_length().is_some_and(|n| n as usize > MAX_BODY) {
                return None;
            }
            let body = resp.bytes().await.ok()?;
            (body.len() <= MAX_BODY).then(|| version_in(&body)).flatten()
        }
        .await;
        match got {
            Some(v) => {
                self.update(|s| {
                    s.checked_at = Some(now);
                    s.latest = Some(v);
                });
                true
            }
            None => {
                tracing::warn!("the release check did not work");
                self.update(|s| s.failed_at = Some(now));
                false
            }
        }
    }

    /// The newest version, if it is newer than this install and has not been announced.
    pub fn to_announce(&self) -> Option<String> {
        let (src, s) = (self.source(), self.saved());
        s.latest.filter(|l| is_newer(l, &src.current) && s.notified.as_deref() != Some(l.as_str()))
    }

    pub fn mark_announced(&self, version: &str) {
        self.update(|s| s.notified = Some(version.to_string()));
    }

    pub fn status(&self) -> ReleaseStatus {
        let (src, s) = (self.source(), self.saved());
        ReleaseStatus {
            newer: s.latest.as_deref().is_some_and(|l| is_newer(l, &src.current)),
            current: src.current,
            latest: s.latest,
            checked_at: s.checked_at,
            failed_at: s.failed_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_release_versions_are_versions_and_they_compare_as_numbers() {
        assert_eq!(parse("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse("v10.0.1"), Some((10, 0, 1)));
        for bad in ["", "1.2", "1.2.3.4", "1.2.x", "1.0.0-rc1", "1.0.0+build", "v", "1..3", " 1.2.3", "1.2.3\n", "1.2.99999999999"] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
        assert!(is_newer("1.0.0", "0.6.0") && is_newer("0.10.0", "0.9.9") && is_newer("1.0.1", "1.0.0"));
        assert!(!is_newer("1.0.0", "1.0.0") && !is_newer("0.5.0", "0.6.0") && !is_newer("junk", "0.6.0") && !is_newer("1.0.0", "junk"));
    }

    #[test]
    fn the_version_is_the_tag_of_a_plain_release_and_nothing_else_in_the_answer() {
        assert_eq!(version_in(br#"{"tag_name":"v1.2.3","name":"<b>hi</b>","body":"x"}"#), Some("1.2.3".into()));
        assert_eq!(version_in(br#"{"tag_name":"1.2.3"}"#), Some("1.2.3".into()));
        assert_eq!(version_in(br#"{"tag_name":"v1.2.3","prerelease":true}"#), None);
        assert_eq!(version_in(br#"{"tag_name":"v1.2.3","draft":true}"#), None);
        assert_eq!(version_in(br#"{"tag_name":"v1.2.3 <script>"}"#), None);
        assert_eq!(version_in(br#"{"name":"v1.2.3"}"#), None);
        assert_eq!(version_in(b"not json"), None);
    }

    #[test]
    fn a_check_is_due_after_a_day_and_a_failed_one_after_an_hour() {
        let d = std::env::temp_dir().join(format!("askbar-releases-{}", ulid::Ulid::new()));
        let c = Checker::new(&d, "http://x".into(), "0.6.0".into());
        assert_eq!(c.due_in(1000), 0, "never checked");
        c.update(|s| s.checked_at = Some(1000));
        assert_eq!(c.due_in(1000 + 3600), 86_400 - 3600);
        assert_eq!(c.due_in(1000 + 86_400), 0);
        c.update(|s| s.failed_at = Some(2000));
        assert_eq!(c.due_in(2000 + 60), 3600 - 60);
        assert_eq!(c.due_in(2000 + 3600), 0);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_state_survives_a_restart_and_a_corrupt_file_starts_empty() {
        let d = std::env::temp_dir().join(format!("askbar-releases-{}", ulid::Ulid::new()));
        let c = Checker::new(&d, "http://x".into(), "0.6.0".into());
        c.update(|s| {
            s.latest = Some("1.0.0".into());
            s.checked_at = Some(5);
        });
        c.mark_announced("1.0.0");
        let again = Checker::new(&d, "http://x".into(), "0.6.0".into());
        assert_eq!(again.saved(), c.saved());
        assert_eq!(again.to_announce(), None, "a version is announced once");
        let names: Vec<String> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, [FILE]);
        std::fs::write(d.join(FILE), "{ nope").unwrap();
        assert_eq!(Checker::new(&d, "http://x".into(), "0.6.0".into()).saved(), Saved::default());
        let _ = std::fs::remove_dir_all(d);
    }
}
