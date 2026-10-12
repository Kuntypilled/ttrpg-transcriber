//! Local dates and times without a time-zone database: the offset comes from `date`
//! once, and the calendar arithmetic is done here. Used for the log file's timestamps
//! and for the date written into chunks (what `date '+...'` and `date.today()` gave
//! the original).

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Days since 1970-01-01 to (year, month, day). Howard Hinnant's civil-from-days.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// `YYYY-MM-DD HH:MM:SS` for a Unix time shifted by `offset_secs`.
pub fn format_timestamp(unix_secs: i64, offset_secs: i64) -> String {
    let t = unix_secs + offset_secs;
    let (y, m, d) = civil_from_days(t.div_euclid(86_400));
    let rem = t.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// The local UTC offset in seconds, from `date +%z` (`+0200`, `-0530`); 0 if unknown.
pub fn local_offset_secs() -> i64 {
    let Ok(out) = Command::new("date").arg("+%z").output() else {
        return 0;
    };
    parse_offset(String::from_utf8_lossy(&out.stdout).trim()).unwrap_or(0)
}

fn parse_offset(z: &str) -> Option<i64> {
    let sign = match z.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let digits = &z[1..];
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (h, m): (i64, i64) = (digits[..2].parse().ok()?, digits[2..].parse().ok()?);
    Some(sign * (h * 3600 + m * 60))
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Today's date, `YYYY-MM-DD`, in the local time zone.
pub fn today_local() -> String {
    format_timestamp(unix_now(), local_offset_secs())[..10].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_arithmetic_is_right_around_the_awkward_dates() {
        assert_eq!(format_timestamp(0, 0), "1970-01-01 00:00:00");
        assert_eq!(format_timestamp(951_782_400, 0), "2000-02-29 00:00:00");
        assert_eq!(format_timestamp(1_791_849_600 + 3_661, 0), "2026-10-13 01:01:01");
        assert_eq!(
            format_timestamp(1_791_849_600, -3600),
            "2026-10-12 23:00:00",
            "the offset can change the day"
        );
        assert_eq!(format_timestamp(-1, 0), "1969-12-31 23:59:59");
    }

    #[test]
    fn offsets_read_the_way_date_prints_them() {
        assert_eq!(parse_offset("+0000"), Some(0));
        assert_eq!(parse_offset("+0200"), Some(7200));
        assert_eq!(parse_offset("-0530"), Some(-19_800));
        assert_eq!(parse_offset("0200"), None);
        assert_eq!(parse_offset("+02"), None);
        assert_eq!(today_local().len(), 10);
    }
}
