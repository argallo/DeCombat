//! Timestamped log lines on stderr (UTC, millisecond resolution). Under systemd the journal keeps them.

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

/// `2026-10-31T12:34:56.789Z` for a time in milliseconds since the Unix epoch.
pub fn stamp(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    let (y, mo, d) = civil(secs / 86400);
    format!(
        "{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}.{:03}Z",
        ms % 1000
    )
}

/// Year, month, day of a day count since 1970-01-01 (Howard Hinnant's days-to-civil).
fn civil(days: u64) -> (i64, u32, u32) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Writes one line `<time> dc-host: <text>` to stderr (one write, so lines of several threads do not interleave).
pub fn line(text: &str) {
    let l = format!("{} dc-host: {text}\n", stamp(now_ms()));
    let _ = std::io::stderr().lock().write_all(l.as_bytes());
}

/// `log!("...", args)`: [`line`] with `format!` arguments.
#[macro_export]
macro_rules! log {
    ($($t:tt)*) => {
        $crate::log::line(&format!($($t)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_are_utc_dates() {
        assert_eq!(stamp(0), "1970-01-01T00:00:00.000Z");
        // 2026-10-31T12:34:56.789Z
        assert_eq!(stamp(1_793_450_096_789), "2026-10-31T12:34:56.789Z");
        // a leap day
        assert_eq!(stamp(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }
}
