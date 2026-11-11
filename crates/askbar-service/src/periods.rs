//! The periods a token budget is counted over, in UTC: the day and the month (plan step 3.2, ADR 0014). The server counts
//! against its copy and the gateway against the authoritative one, and both must start a period at the same moment.

/// Midnight UTC at the start of the day containing `t` (unix seconds).
pub fn day_start(t: i64) -> i64 {
    t - t.rem_euclid(86_400)
}

/// Midnight UTC on the first of the month containing `t` (unix seconds).
pub fn month_start(t: i64) -> i64 {
    // Civil date from days since 1970-01-01 and back (Howard Hinnant's algorithms).
    let z = t.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    // Days since 1970-01-01 of day 1 of that month.
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) * 86_400
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_period_starts_at_midnight_utc_and_a_month_on_the_first_across_leap_days_and_years() {
        assert_eq!((day_start(1793262600), month_start(1793262600)), (1793232000, 1790812800));
        assert_eq!((day_start(1772323200), month_start(1772323200)), (1772323200, 1772323200));
        assert_eq!((day_start(1772323199), month_start(1772323199)), (1772236800, 1769904000));
        assert_eq!((day_start(1835438400), month_start(1835438400)), (1835395200, 1832976000));
        assert_eq!((day_start(1767225601), month_start(1767225601)), (1767225600, 1767225600));
        assert_eq!((day_start(1767225599), month_start(1767225599)), (1767139200, 1764547200));
    }
}
