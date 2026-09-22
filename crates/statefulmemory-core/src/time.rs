//! Time helpers used by storage and gRPC layers.
//!
//! All persistent timestamps are stored as RFC-3339 UTC strings, matching the
//! `datetime('now')` SQLite default in PRD §5.3 and the wire format in PRD §6.

use chrono::{DateTime, Utc};

/// Current wall-clock time as a UTC RFC-3339 string with second precision.
///
/// Matches `datetime('now')` in SQLite. Used wherever the application layer
/// needs to compute a timestamp before SQLite assigns its own (e.g., for
/// `review_after`, `last_seen_at`).
pub fn now_rfc3339() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

/// `now() + months` as RFC-3339 UTC. Used to derive `review_after` (FR4.6).
pub fn months_from_now(months: i32) -> String {
    let now = Utc::now();
    add_months(now, months)
        .format("%Y-%m-%dT%H:%M:%S%:z")
        .to_string()
}

/// Naive month addition: keeps the same day-of-month when possible, clamps to
/// last day otherwise. Sufficient for `review_after` derivation; tests assert
/// "approximately now+N months ± 1 minute" so calendar arithmetic edge cases
/// don't matter.
fn add_months(ts: DateTime<Utc>, months: i32) -> DateTime<Utc> {
    use chrono::{Datelike, NaiveDate, Timelike};

    let total_months = ts.year() * 12 + ts.month0() as i32 + months;
    let new_year = total_months.div_euclid(12);
    let new_month0 = total_months.rem_euclid(12) as u32;
    let mut day = ts.day();
    // Clamp day to last day of the target month if it doesn't exist (e.g., Feb 30).
    loop {
        if NaiveDate::from_ymd_opt(new_year, new_month0 + 1, day).is_some() {
            break;
        }
        day -= 1;
        if day == 0 {
            day = 1;
            break;
        }
    }
    let date = NaiveDate::from_ymd_opt(new_year, new_month0 + 1, day).unwrap();
    let dt = date
        .and_hms_opt(ts.hour(), ts.minute(), ts.second())
        .unwrap();
    DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn months_addition_preserves_day() {
        let ts = Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap();
        let plus6 = add_months(ts, 6);
        assert_eq!(plus6.format("%Y-%m-%d").to_string(), "2026-07-15");
    }

    #[test]
    fn months_addition_clamps_to_last_day() {
        // Jan 31 + 1 month should clamp to Feb 28 (or 29 in leap years).
        let ts = Utc.with_ymd_and_hms(2027, 1, 31, 12, 0, 0).unwrap(); // 2027 is not a leap year
        let plus1 = add_months(ts, 1);
        assert_eq!(plus1.format("%Y-%m-%d").to_string(), "2027-02-28");
    }

    #[test]
    fn months_addition_crosses_year() {
        let ts = Utc.with_ymd_and_hms(2026, 11, 15, 12, 0, 0).unwrap();
        let plus3 = add_months(ts, 3);
        assert_eq!(plus3.format("%Y-%m-%d").to_string(), "2027-02-15");
    }
}
