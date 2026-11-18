//! How an activity entry is written to the log file: one line per entry,
//! starting with a readable UTC date and time.

use std::time::{SystemTime, UNIX_EPOCH};

/// One log-file line, without the newline, such as
/// `2026-11-18 14:03:22Z warning could not reach the overlay`.
pub fn log_line(time: SystemTime, level: &str, message: &str) -> String {
    let seconds = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, of_day) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil_date(days);
    let (hour, minute, second) = (of_day / 3600, of_day / 60 % 60, of_day % 60);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}Z {level} {message}")
}

/// The year, month and day of a count of days since 1970-01-01, by Howard
/// Hinnant's `civil_from_days`.
fn civil_date(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_line_starts_with_the_utc_date_and_time() {
        // 2026-11-18 14:03:22 UTC, computed independently.
        let time = UNIX_EPOCH + Duration::from_secs(1_795_010_602);
        assert_eq!(
            log_line(time, "warning", "could not reach the overlay"),
            "2026-11-18 14:03:22Z warning could not reach the overlay"
        );
    }

    #[test]
    fn a_leap_day_is_dated_correctly() {
        // 2024-02-29 23:59:59 UTC.
        let time = UNIX_EPOCH + Duration::from_secs(1_709_251_199);
        assert_eq!(
            log_line(time, "info", "match 42 recorded"),
            "2024-02-29 23:59:59Z info match 42 recorded"
        );
    }
}
