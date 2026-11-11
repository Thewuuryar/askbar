//! The check of the audit chain (ADR 0010 rule 7, plan step 2.6): every few minutes the admin asks the auditor for the
//! head of its hash chain and compares it with the one it stored last time. The admin is the only one that remembers it,
//! so it is the only one that can tell a rollback (the chain is shorter than it was, as after an older copy of the file
//! was put back) or a rewrite (the entry it stored is not there, or has another hash) from a chain that merely grew.
//! Now and then it also asks the auditor for a full check of every record against its entry, which finds an edit, an
//! insertion or a deletion inside the file.
//!
//! What it finds is shown as a banner and logged once when it begins (never what a record held), until the alerter
//! delivers it as well (phase 4). A finding is worked out again at every check, so it goes away when the auditor's chain
//! is the one the admin stored again, and stays while it is not. The stored head moves forward only after a check that
//! found the chain as it was.

use crate::auditor_link::AuditorLink;
use crate::AdminState;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the head is compared.
pub const CHECK_EVERY: Duration = Duration::from_secs(5 * 60);
/// How often the whole store is checked (it reads every record).
pub const FULL_EVERY: Duration = Duration::from_secs(60 * 60);

/// Where the chain is, as far as the last check could tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Not checked yet, or the auditor has not been reachable since this process started.
    Unknown,
    Ok,
    /// The auditor's chain is shorter than the one the admin stored.
    Rollback {
        stored: i64,
        current: i64,
    },
    /// The entry the admin stored is not in the chain, or has another hash.
    Rewritten {
        seq: i64,
    },
    /// The chain is as it was, but records in the file do not match it.
    Edited {
        problems: usize,
        first: String,
    },
}

#[derive(Default)]
struct Inner {
    /// What the head comparison found.
    head: Option<State>,
    /// What the last full check found, if it found anything.
    edited: Option<State>,
    last_full: Option<Instant>,
    /// What was last logged, so a finding is one line, not one for every check.
    logged: Option<State>,
}

#[derive(Default)]
pub struct ChainWatch {
    inner: Mutex<Inner>,
}

impl ChainWatch {
    /// What to show: a rollback or a rewrite before an edit, an edit before all clear.
    pub fn state(&self) -> State {
        let i = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match (&i.head, &i.edited) {
            (Some(s @ (State::Rollback { .. } | State::Rewritten { .. })), _) => s.clone(),
            (_, Some(e)) => e.clone(),
            (Some(State::Ok), None) => State::Ok,
            _ => State::Unknown,
        }
    }

    /// The part of the status the banner reads: `state` and a sentence for the person. The sentence names numbers and
    /// the kind of record, never what a record held.
    pub fn status(&self) -> serde_json::Value {
        let (state, detail) = match self.state() {
            State::Unknown => ("unknown", String::new()),
            State::Ok => ("ok", String::new()),
            State::Rollback { stored, current } => (
                "rollback",
                format!(
                    "The audit trail is shorter than it was: the auditor's chain ends at entry {current}, and this admin last saw entry {stored}. An older copy of the file may have been put back, or records may have been removed."
                ),
            ),
            State::Rewritten { seq } => (
                "rewritten",
                format!("The audit trail was rewritten: entry {seq} is no longer the one this admin stored. Records may have been changed or removed."),
            ),
            State::Edited { problems, first } => (
                "edited",
                format!(
                    "Records in the audit trail do not match their entries in the chain: {problems} problem{}, the first: {first}. They may have been changed, added or removed after they were written.",
                    if problems == 1 { "" } else { "s" }
                ),
            ),
        };
        serde_json::json!({ "state": state, "detail": detail })
    }

    /// One check. `full` also asks for the whole-store check; otherwise it runs when the last one is [`FULL_EVERY`] old.
    pub async fn check(&self, state: &AdminState, link: &AuditorLink, full: bool) {
        // 1. The head. An auditor that does not answer is not a finding: nothing was seen, so nothing changes.
        let Some((200, head)) = link.get("/chain/head").await else { return };
        let (Some(seq), Some(hash)) = (head["seq"].as_i64(), head["hash"].as_str().map(str::to_string)) else { return };
        let stored = {
            let db = state.db.lock().await;
            stored_head(db.conn())
        };

        // 2. Against the one stored.
        let found = match &stored {
            None => State::Ok,
            Some((s_seq, _)) if seq < *s_seq => State::Rollback { stored: *s_seq, current: seq },
            Some((s_seq, s_hash)) => match link.get(&format!("/chain/entries/{s_seq}")).await {
                Some((200, e)) if e["hash"].as_str() == Some(s_hash.as_str()) => State::Ok,
                Some((200 | 404, _)) => State::Rewritten { seq: *s_seq },
                // An answer that says nothing about the entry (an error, nothing): not seen.
                _ => return,
            },
        };

        // 3. The whole store, now and then, and only while the chain itself is as it was.
        let due = {
            let i = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            full || i.last_full.is_none_or(|t| t.elapsed() >= FULL_EVERY)
        };
        let mut edited = None;
        let mut ran_full = false;
        if found == State::Ok && due {
            if let Some((200, report)) = link.get("/chain/verify").await {
                ran_full = true;
                if report["ok"].as_bool() == Some(false) {
                    let problems = report["problems"].as_array().map_or(0, Vec::len) + usize::from(report["more"].as_bool() == Some(true));
                    let first = report["problems"][0]["reason"]
                        .as_str()
                        .map(|r| format!("{} {}", report["problems"][0]["kind"].as_str().unwrap_or("record"), r));
                    edited = Some(State::Edited { problems, first: first.unwrap_or_else(|| "a record does not match".into()) });
                }
            }
        }

        // 4. Forward only when the chain was as it was.
        if found == State::Ok {
            let db = state.db.lock().await;
            store_head(db.conn(), seq, &hash);
        }

        let now_state = {
            let mut i = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            i.head = Some(found);
            if ran_full {
                i.edited = edited;
                i.last_full = Some(Instant::now());
            }
            drop(i);
            self.state()
        };
        self.log_change(now_state);
    }

    /// One log line when a finding begins (and one when it ends), so one fault is one line.
    fn log_change(&self, now: State) {
        let mut i = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if i.logged.as_ref() == Some(&now) {
            return;
        }
        match &now {
            State::Rollback { stored, current } => {
                tracing::error!(
                    stored,
                    current,
                    "ALERT audit chain: the auditor's chain is shorter than the one stored: a rollback, or records removed"
                )
            }
            State::Rewritten { seq } => {
                tracing::error!(seq, "ALERT audit chain: the stored entry is not in the auditor's chain: it was rewritten")
            }
            State::Edited { problems, first } => tracing::error!(problems, first, "ALERT audit chain: records do not match their entries"),
            State::Ok if matches!(i.logged, Some(State::Rollback { .. } | State::Rewritten { .. } | State::Edited { .. })) => {
                tracing::info!("audit chain: the chain is the one stored again")
            }
            _ => {}
        }
        i.logged = Some(now);
    }
}

/// Check for as long as the process runs: the first time at once, then every [`CHECK_EVERY`].
pub async fn run(state: Arc<AdminState>, link: Arc<AuditorLink>) {
    loop {
        state.chain.check(&state, &link, false).await;
        tokio::time::sleep(CHECK_EVERY).await;
    }
}

fn stored_head(conn: &rusqlite::Connection) -> Option<(i64, String)> {
    conn.query_row("SELECT seq, hash FROM audit_chain_head WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?))).ok()
}

fn store_head(conn: &rusqlite::Connection, seq: i64, hash: &str) {
    let now = crate::auth::now();
    let saved = conn.execute(
        "INSERT INTO audit_chain_head (id, seq, hash, checked_at) VALUES (1, ?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET seq = excluded.seq, hash = excluded.hash, checked_at = excluded.checked_at",
        rusqlite::params![seq, hash, now],
    );
    if let Err(e) = saved {
        tracing::warn!("could not store the audit chain head: {e}");
    }
}
