//! What the gateway holds the token budget against (plan step 3.2, ADR 0014).
//!
//! The gateway has no data volume, so its count would start again at every restart, and the server's count is the
//! server's word, not the gateway's. It therefore keeps two numbers for the day and for the month, in memory, and reads the
//! third from the auditor, which holds what was really recorded:
//!
//! - **the view:** the auditor's total for the period, read at start and then every minute with the gateway's own read-only
//!   link (`GET /token-counts` and nothing else);
//! - **what it counted itself** since it started, for the replies the auditor has not been told the end of yet.
//!
//! The figure it enforces is `max(view, the first view + what it counted since that view)`: it never counts a reply twice
//! (the larger of the two sources, not their sum) and it never counts less than either source knows. Until it has read a
//! view it knows nothing about the past, and says so: a budgeted request is then refused with `audit_backlog` (try again in
//! a few seconds), because a budget it cannot read is one it cannot hold. A subscription provider's tokens are not counted,
//! as at the server.

use askbar_proto::audit::TokenCount;
use askbar_service::periods::{day_start, month_start};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

/// The link the read token belongs to, as `bootstrap` names it.
pub const READ_LINK: &str = "model-gateway-auditor-read";
/// How often the view is read again.
pub const REFRESH_EVERY: Duration = Duration::from_secs(60);

/// One period (a day or a month) of one count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Period {
    start: i64,
    /// The auditor's latest total for this period.
    view: Option<u64>,
    /// The first view this process read for this period, and what it had counted itself by then.
    first: Option<(u64, u64)>,
    /// Tokens counted by this process in this period.
    local: u64,
}

impl Period {
    fn roll(&mut self, start: i64) {
        if self.start != start {
            *self = Period { start, ..Period::default() };
        }
    }

    fn effective(&self) -> Option<u64> {
        let view = self.view?;
        let (first, local_then) = self.first?;
        Some(view.max(first + self.local.saturating_sub(local_then)))
    }
}

/// The day's and the month's counts.
#[derive(Default)]
pub struct Book {
    inner: Mutex<(Period, Period)>,
}

/// How much was used in each period, or `None` for one the gateway has not read yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Used {
    pub daily: Option<u64>,
    pub monthly: Option<u64>,
}

impl Book {
    fn with<R>(&self, now: i64, f: impl FnOnce(&mut Period, &mut Period) -> R) -> R {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (day, month) = &mut *g;
        day.roll(day_start(now));
        month.roll(month_start(now));
        f(day, month)
    }

    /// A reply that was served and used `tokens` (metered providers only).
    pub fn count(&self, now: i64, tokens: u64) {
        self.with(now, |d, m| {
            d.local += tokens;
            m.local += tokens;
        });
    }

    /// The auditor's totals for the current day and month, as just read.
    pub fn set_view(&self, now: i64, daily: u64, monthly: u64) {
        self.with(now, |d, m| {
            for (p, total) in [(d, daily), (m, monthly)] {
                p.view = Some(total);
                if p.first.is_none() {
                    p.first = Some((total, p.local));
                }
            }
        });
    }

    pub fn used(&self, now: i64) -> Used {
        self.with(now, |d, m| Used { daily: d.effective(), monthly: m.effective() })
    }
}

/// The gateway's read of the token counts, with its own token.
pub struct Reader {
    http: reqwest::Client,
    base: String,
    token_dir: PathBuf,
}

impl Reader {
    pub fn new(base: &str, token_dir: PathBuf) -> Reader {
        Reader {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest client"),
            base: base.trim_end_matches('/').to_string(),
            token_dir,
        }
    }

    /// `GET /token-counts?since=`: one row for each provider and model, or `None` if the auditor did not answer.
    pub async fn counts(&self, since: i64) -> Option<Vec<TokenCount>> {
        let token = std::fs::read_to_string(self.token_dir.join("call").join(format!("{READ_LINK}.token"))).ok()?;
        let url = format!("{}{}/token-counts?since={since}", self.base, askbar_proto::paths::AUDIT_V1);
        let resp = self.http.get(url).bearer_auth(token.trim()).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.json().await.ok()
    }

    /// The tokens in and out since `since` by every provider that is not in `free` (the subscription providers).
    pub async fn metered_total(&self, since: i64, free: &BTreeSet<String>) -> Option<u64> {
        let rows = self.counts(since).await?;
        Some(
            rows.iter()
                .filter(|r| !r.provider_id.as_ref().is_some_and(|p| free.contains(p)))
                .map(|r| (r.tokens_in.max(0) + r.tokens_out.max(0)) as u64)
                .sum(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_793_262_600; // 2026-10-29 08:30 UTC

    #[test]
    fn nothing_is_known_until_a_view_is_read_and_then_the_larger_of_the_two_sources_counts() {
        let b = Book::default();
        assert_eq!(b.used(T), Used { daily: None, monthly: None }, "no view, no knowledge");
        b.count(T, 40); // served before the first view: the view may or may not include it
        b.set_view(T, 100, 1000);
        assert_eq!(b.used(T), Used { daily: Some(100), monthly: Some(1000) }, "the view is the floor");
        b.count(T, 30);
        assert_eq!(b.used(T).daily, Some(130), "100 from before, and 30 the auditor has not been told of");
        // The auditor catches up with those 30 and more: its view is larger, and wins.
        b.set_view(T, 150, 1050);
        assert_eq!(b.used(T), Used { daily: Some(150), monthly: Some(1050) });
        // A view that lags (a read just before records landed) never lowers what was counted since the first view.
        b.set_view(T, 120, 1020);
        assert_eq!(b.used(T).daily, Some(130), "max of the view and the first view plus what was counted since");
    }

    #[test]
    fn a_new_day_and_a_new_month_start_their_counts_again() {
        let b = Book::default();
        b.set_view(T, 500, 5000);
        b.count(T, 100);
        let next_day = T + 86_400;
        assert_eq!(
            b.used(next_day),
            Used { daily: None, monthly: Some(5100) },
            "the day rolled, the month did not: 5000 read plus the 100 counted since"
        );
        b.set_view(next_day, 7, 5007);
        assert_eq!(
            b.used(next_day),
            Used { daily: Some(7), monthly: Some(5100) },
            "the 100 counted here are not yet in the auditor's 5007, so the larger figure stands"
        );
        let b2 = Book::default();
        b2.set_view(T, 10, 20);
        assert_eq!(b2.used(T + 2 * 86_400).monthly, Some(20), "two days later is still October");
        let november = T + 5 * 86_400; // 2026-11-03
        assert_eq!(b2.used(november), Used { daily: None, monthly: None }, "a new month: unknown until read");
    }
}
