//! Win32 `FILETIME` (u64, 100-ns ticks since 1601-01-01) and 16-byte `SYSTEMTIME` (8 x u16) helpers.
//!
//! SPEC: PROTOCOL_2009.md §1.2 (time values: FILETIME fields are 8 bytes in **local wall-clock time**,
//! 0 = permanent; SYSTEMTIME is 8 x u16) and §3.3 (`lobby_login_reply` 0x301 carries a SYSTEMTIME server time
//! at +0xe). The layouts themselves are public Win32 knowledge.

use std::time::{SystemTime as StdSystemTime, UNIX_EPOCH};

/// 100-ns ticks between `1601-01-01T00:00:00` and `1970-01-01T00:00:00` (`116444736000000000`).
pub const EPOCH_DIFF: u64 = 116_444_736_000_000_000;

/// 100-ns ticks per second (`10_000_000`).
pub const HNS_PER_SEC: u64 = 10_000_000;

/// 100-ns ticks per 24-hour day (`864_000_000_000`).
pub const FT_DAY: u64 = 864_000_000_000;

/// FILETIME value that means "permanent" (no end time).
// SPEC: PROTOCOL_2009.md §1.2 (time values: 0 = permanent)
pub const NO_END_FILETIME: u64 = 0;

/// Win32 `SYSTEMTIME` layout (8 x u16 = 16 bytes little-endian), field order as in the public Win32 record.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SystemTime {
    pub year: u16,
    pub month: u16,
    pub weekday: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub milliseconds: u16,
}

impl SystemTime {
    /// Packs this record into 16 little-endian bytes (the +0xe field of `lobby_login_reply` 0x301).
    #[inline]
    pub fn to_bytes(self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[0..2].copy_from_slice(&self.year.to_le_bytes());
        out[2..4].copy_from_slice(&self.month.to_le_bytes());
        out[4..6].copy_from_slice(&self.weekday.to_le_bytes());
        out[6..8].copy_from_slice(&self.day.to_le_bytes());
        out[8..10].copy_from_slice(&self.hour.to_le_bytes());
        out[10..12].copy_from_slice(&self.minute.to_le_bytes());
        out[12..14].copy_from_slice(&self.second.to_le_bytes());
        out[14..16].copy_from_slice(&self.milliseconds.to_le_bytes());
        out
    }

    /// Unpacks a 16-byte `<8H` little-endian array into a `SystemTime`.
    #[inline]
    pub fn from_bytes(b: [u8; 16]) -> Self {
        Self {
            year: u16::from_le_bytes([b[0], b[1]]),
            month: u16::from_le_bytes([b[2], b[3]]),
            weekday: u16::from_le_bytes([b[4], b[5]]),
            day: u16::from_le_bytes([b[6], b[7]]),
            hour: u16::from_le_bytes([b[8], b[9]]),
            minute: u16::from_le_bytes([b[10], b[11]]),
            second: u16::from_le_bytes([b[12], b[13]]),
            milliseconds: u16::from_le_bytes([b[14], b[15]]),
        }
    }
}

/// Returns current Unix timestamp in seconds.
#[inline]
pub fn now_unix_secs() -> i64 {
    StdSystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Converts a Unix timestamp in seconds to a UTC `FILETIME` (64-bit 100-ns ticks since 1601-01-01).
#[inline]
pub fn to_filetime_utc(unix_secs: i64) -> u64 {
    let ticks_from_1970 = (unix_secs as i128) * (HNS_PER_SEC as i128);
    let total = (EPOCH_DIFF as i128) + ticks_from_1970;
    if total <= 0 {
        0
    } else {
        total as u64
    }
}

/// Converts a UTC `FILETIME` to a Unix timestamp in seconds.
#[inline]
pub fn from_filetime_utc(ft: u64) -> i64 {
    let diff = (ft as i128) - (EPOCH_DIFF as i128);
    (diff.div_euclid(HNS_PER_SEC as i128)) as i64
}

#[cfg(windows)]
mod win_ffi {
    use super::SystemTime;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct FileTimeStruct {
        pub low: u32,
        pub high: u32,
    }

    impl FileTimeStruct {
        #[inline]
        pub fn from_u64(v: u64) -> Self {
            Self {
                low: v as u32,
                high: (v >> 32) as u32,
            }
        }

        #[inline]
        pub fn to_u64(self) -> u64 {
            (self.low as u64) | ((self.high as u64) << 32)
        }
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn SystemTimeToFileTime(
            lp_system_time: *const SystemTime,
            lp_file_time: *mut FileTimeStruct,
        ) -> i32;
        pub fn FileTimeToSystemTime(
            lp_file_time: *const FileTimeStruct,
            lp_system_time: *mut SystemTime,
        ) -> i32;
        pub fn SystemTimeToTzSpecificLocalTime(
            lp_time_zone_information: *const core::ffi::c_void,
            lp_universal_time: *const SystemTime,
            lp_local_time: *mut SystemTime,
        ) -> i32;
        pub fn TzSpecificLocalTimeToSystemTime(
            lp_time_zone_information: *const core::ffi::c_void,
            lp_local_time: *const SystemTime,
            lp_universal_time: *mut SystemTime,
        ) -> i32;
    }
}

/// Converts a civil date/time into a UTC `FILETIME` using Howard Hinnant's civil calendar algorithm.
pub fn systemtime_to_filetime_civil(st: SystemTime) -> u64 {
    let y = (st.year as i64) - if st.month <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m = st.month as u64;
    let d = st.day as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days_since_1970 = era * 146097 + (doe as i64) - 719468;
    let secs = days_since_1970 * 86_400
        + (st.hour as i64) * 3_600
        + (st.minute as i64) * 60
        + (st.second as i64);
    let base = to_filetime_utc(secs);
    base + (st.milliseconds as u64) * 10_000
}

/// Converts a `FILETIME` (ticks since 1601-01-01) into a civil `SystemTime`.
pub fn filetime_to_systemtime_civil(ft: u64) -> SystemTime {
    let total_ms = ft / 10_000;
    let milliseconds = (total_ms % 1_000) as u16;
    let unix_secs = from_filetime_utc(ft);
    let days = unix_secs.div_euclid(86_400);
    let secs_of_day = unix_secs.rem_euclid(86_400) as u64;

    let hour = (secs_of_day / 3_600) as u16;
    let minute = ((secs_of_day % 3_600) / 60) as u16;
    let second = (secs_of_day % 60) as u16;
    // 1970-01-01 was a Thursday (4 in 0=Sun..6=Sat)
    let weekday = (days + 4).rem_euclid(7) as u16;

    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = (y + if m <= 2 { 1 } else { 0 }) as u16;

    SystemTime {
        year,
        month: m as u16,
        weekday,
        day: d as u16,
        hour,
        minute,
        second,
        milliseconds,
    }
}

/// Converts a Unix timestamp in seconds to a local wall-clock `SystemTime`.
pub fn systemtime_local(unix_secs: i64) -> SystemTime {
    #[cfg(windows)]
    unsafe {
        use win_ffi::*;
        let utc_ft = FileTimeStruct::from_u64(to_filetime_utc(unix_secs));
        let mut utc_st = SystemTime::default();
        if FileTimeToSystemTime(&utc_ft, &mut utc_st) != 0 {
            let mut local_st = SystemTime::default();
            if SystemTimeToTzSpecificLocalTime(core::ptr::null(), &utc_st, &mut local_st) != 0 {
                return local_st;
            }
        }
        filetime_to_systemtime_civil(to_filetime_utc(unix_secs))
    }
    #[cfg(not(windows))]
    {
        filetime_to_systemtime_civil(to_filetime_utc(unix_secs))
    }
}

/// Packs a Unix timestamp in seconds into the 16-byte local `SYSTEMTIME` layout
/// (`lobby_login_reply` 0x301 +0xe, SPEC: PROTOCOL_2009.md §3.3).
#[inline]
pub fn systemtime_bytes_local(unix_secs: i64) -> [u8; 16] {
    systemtime_local(unix_secs).to_bytes()
}

/// Packs the current local wall-clock time into the 16-byte `SYSTEMTIME` layout
/// (`lobby_login_reply` 0x301 +0xe, SPEC: PROTOCOL_2009.md §3.3).
#[inline]
pub fn systemtime_bytes_now() -> [u8; 16] {
    systemtime_bytes_local(now_unix_secs())
}

/// Converts a Unix timestamp in seconds to a local wall-clock `FILETIME`.
// SPEC: PROTOCOL_2009.md §1.2 (FILETIME fields are local wall-clock time, LIVE)
pub fn to_filetime_local(unix_secs: i64) -> u64 {
    let local_st = systemtime_local(unix_secs);
    #[cfg(windows)]
    unsafe {
        use win_ffi::*;
        let mut local_ft = FileTimeStruct::default();
        if SystemTimeToFileTime(&local_st, &mut local_ft) != 0 {
            return local_ft.to_u64();
        }
    }
    systemtime_to_filetime_civil(local_st)
}

/// Converts a local wall-clock `FILETIME` back to a Unix timestamp in seconds.
pub fn from_filetime_local(ft: u64) -> i64 {
    #[cfg(windows)]
    unsafe {
        use win_ffi::*;
        let local_ft = FileTimeStruct::from_u64(ft);
        let mut local_st = SystemTime::default();
        if FileTimeToSystemTime(&local_ft, &mut local_st) != 0 {
            let mut utc_st = SystemTime::default();
            if TzSpecificLocalTimeToSystemTime(core::ptr::null(), &local_st, &mut utc_st) != 0 {
                let mut utc_ft = FileTimeStruct::default();
                if SystemTimeToFileTime(&utc_st, &mut utc_ft) != 0 {
                    return from_filetime_utc(utc_ft.to_u64());
                }
            }
        }
    }
    from_filetime_utc(ft)
}

/// Returns the current local wall-clock `FILETIME` plus `days * FT_DAY` (negative `days` go back).
// SPEC: PROTOCOL_2009.md §1.2 (FILETIME fields are local wall-clock time)
#[inline]
pub fn local_filetime_after_days(days: i64) -> u64 {
    let base = to_filetime_local(now_unix_secs());
    if days >= 0 {
        base.saturating_add((days as u64).saturating_mul(FT_DAY))
    } else {
        base.saturating_sub(((-days) as u64).saturating_mul(FT_DAY))
    }
}
