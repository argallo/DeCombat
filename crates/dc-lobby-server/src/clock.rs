//! Local wall-clock time as the 2009 client uses it: item expiry and mute ends are FILETIME values of the
//! LOCAL clock (SPEC: PROTOCOL_2009.md §1.2 "Time values"), and 0x301 carries a 16-byte SYSTEMTIME.
//! FILETIME = 100 ns ticks since 1601-01-01; SYSTEMTIME = 8 x u16 (public Win32 documentation).

use std::time::{SystemTime, UNIX_EPOCH};

/// 100 ns ticks between 1601-01-01 and 1970-01-01 (public FILETIME definition).
pub const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;
/// 100 ns ticks per second.
pub const TICKS_PER_SEC: i64 = 10_000_000;
/// 100 ns ticks per day.
pub const TICKS_PER_DAY: i64 = 86_400 * TICKS_PER_SEC;

/// Calendar fields in the order of the 16-byte wire SYSTEMTIME.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct WallClock {
    pub year: u16,
    pub month: u16,
    pub weekday: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub millis: u16,
}

impl WallClock {
    pub fn to_bytes(self) -> [u8; 16] {
        let mut out = [0u8; 16];
        let f = [
            self.year,
            self.month,
            self.weekday,
            self.day,
            self.hour,
            self.minute,
            self.second,
            self.millis,
        ];
        for (i, v) in f.iter().enumerate() {
            out[i * 2..i * 2 + 2].copy_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// `YYYY-MM-DD HH:MM:SS.mmm`
    pub fn stamp(&self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
            self.year, self.month, self.day, self.hour, self.minute, self.second, self.millis
        )
    }
}

#[cfg(windows)]
mod sys {
    use super::WallClock;

    #[repr(C)]
    #[derive(Default)]
    pub struct Ticks64 {
        pub low: u32,
        pub high: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetLocalTime(out: *mut WallClock);
        pub fn SystemTimeToFileTime(input: *const WallClock, out: *mut Ticks64) -> i32;
    }

    pub fn local_now() -> Option<(WallClock, i64)> {
        let mut wc = WallClock::default();
        let mut ft = Ticks64::default();
        // SAFETY: both pointers refer to live, correctly sized #[repr(C)] locals.
        let ok = unsafe {
            GetLocalTime(&mut wc);
            SystemTimeToFileTime(&wc, &mut ft)
        };
        if ok == 0 {
            return None;
        }
        Some((wc, ((ft.high as i64) << 32) | ft.low as i64))
    }
}

/// Days since 1970-01-01 -> (year, month, day) (public civil-from-days algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn utc_now() -> (WallClock, i64) {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, dd) = civil(days);
    let wc = WallClock {
        year: y as u16,
        month: m as u16,
        weekday: ((days + 4).rem_euclid(7)) as u16,
        day: dd as u16,
        hour: (rem / 3600) as u16,
        minute: (rem / 60 % 60) as u16,
        second: (rem % 60) as u16,
        millis: d.subsec_millis() as u16,
    };
    let ticks = UNIX_EPOCH_TICKS + secs * TICKS_PER_SEC + (d.subsec_nanos() / 100) as i64;
    (wc, ticks)
}

/// Current local wall clock and its FILETIME value.
pub fn local_now() -> (WallClock, i64) {
    #[cfg(windows)]
    {
        if let Some(v) = sys::local_now() {
            return v;
        }
    }
    utc_now()
}

/// Current local FILETIME (the value item end times are compared against).
pub fn now_ticks() -> i64 {
    local_now().1
}

/// Seconds since the Unix epoch (UTC), for DB bookkeeping columns.
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_723), (2024, 1, 1));
        assert_eq!(civil(-1), (1969, 12, 31));
    }

    #[test]
    fn local_now_is_plausible() {
        let (wc, t) = local_now();
        assert!(wc.year >= 2024 && (1..=12).contains(&wc.month) && (1..=31).contains(&wc.day));
        let utc = UNIX_EPOCH_TICKS + unix_now() * TICKS_PER_SEC;
        // local clock is within 15 h of UTC
        assert!((t - utc).abs() < 15 * 3600 * TICKS_PER_SEC, "{t} vs {utc}");
        assert_eq!(wc.to_bytes().len(), 16);
        assert_eq!(&wc.to_bytes()[0..2], &wc.year.to_le_bytes());
        assert_eq!(wc.stamp().len(), 23);
    }
}
