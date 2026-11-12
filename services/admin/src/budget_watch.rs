//! The token budget alerts (ADR 0055, ADR 0014, D5): once a minute the admin reads what the auditor has counted for the day and
//! the month (`GET /token-counts`), leaves out the subscription providers, and tells the alerter when a limit is 80% and when it is
//! 100% used (`budget_threshold`, `warning` and `critical`). Each threshold of each period is sent once for that period: the next day
//! or month is a new one. `model-gateway` holds the budget and refuses the work; this only says so. The count and the limits are the
//! same as the gateway's: the day and the month start at midnight UTC, and a limit that is not set is not watched.

use crate::auditor_link::AuditorLink;
use crate::AdminState;
use askbar_proto::alerter::{Fields, Kind, Period, Severity};
use askbar_service::periods::{day_start, month_start};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often the counts are read.
pub const EVERY: Duration = Duration::from_secs(60);
/// The thresholds, in percent of a limit.
pub const THRESHOLDS: [u8; 2] = [80, 100];

#[derive(Default)]
pub struct BudgetWatch {
    /// `(period, the period's start, threshold)` already sent. Kept in memory: a restart may repeat a threshold that was
    /// already crossed, and the alerter's own repeat window then absorbs it.
    sent: Mutex<BTreeSet<(Period, i64, u8)>>,
}

/// The limits (tokens a day, tokens a month), if set.
fn limits(conn: &rusqlite::Connection) -> (Option<u64>, Option<u64>) {
    conn.query_row("SELECT daily_tokens, monthly_tokens FROM budgets WHERE id = 1", [], |r| {
        Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?))
    })
    .map(|(d, m)| (d.filter(|n| *n > 0).map(|n| n as u64), m.filter(|n| *n > 0).map(|n| n as u64)))
    .unwrap_or((None, None))
}

/// The providers whose tokens are not counted (a subscription costs the same however much is used).
fn subscriptions(conn: &rusqlite::Connection) -> BTreeSet<String> {
    let Ok(mut stmt) = conn.prepare("SELECT id FROM providers WHERE billing = 'subscription'") else { return BTreeSet::new() };
    stmt.query_map([], |r| r.get::<_, String>(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// The highest threshold `used` has reached of `limit`, if any.
pub fn reached(used: u64, limit: u64) -> Option<u8> {
    THRESHOLDS.into_iter().rev().find(|t| u128::from(used) * 100 >= u128::from(limit) * u128::from(*t))
}

impl BudgetWatch {
    /// Read the counts and send what is due. An auditor that does not answer changes nothing.
    pub async fn check(&self, state: &AdminState, link: &AuditorLink) {
        self.check_at(state, link, crate::auth::now()).await;
    }

    pub async fn check_at(&self, state: &AdminState, link: &AuditorLink, now: i64) {
        let (daily, monthly, free) = {
            let db = state.db.lock().await;
            let (d, m) = limits(db.conn());
            (d, m, subscriptions(db.conn()))
        };
        for (period, limit, start) in [(Period::Day, daily, day_start(now)), (Period::Month, monthly, month_start(now))] {
            let Some(limit) = limit else { continue };
            // A period that is over is forgotten, so the sent list does not grow.
            self.sent.lock().unwrap_or_else(|e| e.into_inner()).retain(|(p, s, _)| *p != period || *s >= start);
            let Some((200, rows)) = link.get(&format!("/token-counts?since={start}")).await else { continue };
            let Some(rows) = rows.as_array() else { continue };
            let used: u64 = rows
                .iter()
                .filter(|r| !r["provider_id"].as_str().is_some_and(|p| free.contains(p)))
                .map(|r| (r["tokens_in"].as_i64().unwrap_or(0).max(0) + r["tokens_out"].as_i64().unwrap_or(0).max(0)) as u64)
                .sum();
            let Some(top) = reached(used, limit) else { continue };
            let due = {
                let mut sent = self.sent.lock().unwrap_or_else(|e| e.into_inner());
                // Crossing 100% at once also crosses 80%: the higher one is said, and the lower is not said after it.
                let new = !sent.contains(&(period, start, top));
                for t in THRESHOLDS.into_iter().filter(|t| *t <= top) {
                    sent.insert((period, start, t));
                }
                new
            };
            if due {
                let severity = if top == 100 { Severity::Critical } else { Severity::Warning };
                state.notifier.notify(
                    Kind::BudgetThreshold,
                    severity,
                    Fields { period: Some(period), percent: Some(top), ..Fields::default() },
                );
            }
        }
    }
}

/// Check once a minute for as long as the process runs.
pub async fn run(state: Arc<AdminState>, link: Arc<AuditorLink>) {
    loop {
        state.budget.check(&state, &link).await;
        tokio::time::sleep(EVERY).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eighty_and_a_hundred_percent_are_the_thresholds_and_the_higher_one_wins() {
        assert_eq!(reached(79, 100), None);
        assert_eq!(reached(80, 100), Some(80));
        assert_eq!(reached(99, 100), Some(80));
        assert_eq!(reached(100, 100), Some(100));
        assert_eq!(reached(5_000, 100), Some(100));
        assert_eq!(reached(u64::MAX, u64::MAX), Some(100), "no overflow");
        assert_eq!(reached(0, 1), None);
    }
}
