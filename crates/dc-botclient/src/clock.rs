//! The local wall clock as a FILETIME, the time base of the lobby's local-time values (the chat mute end of 0x303
//! +0xec and 0x3053, item end times): 100 ns ticks since 1601-01-01 of the LOCAL clock, compared against the local
//! clock (FILETIME and SYSTEMTIME as the public Win32 documentation defines them).
// SPEC: PROTOCOL_2009.md §1.2 "Time values" (FILETIME values of the local clock), §6.4 0x3053
// EVIDENCE: cshell+0x1b1640 (the mute end FILETIME goes through FileTimeToSystemTime into calendar fields that are
// compared with the local clock)

use std::time::{SystemTime, UNIX_EPOCH};

/// 100 ns ticks between 1601-01-01 and 1970-01-01 (public FILETIME definition).
pub const UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;
/// 100 ns ticks per second.
pub const TICKS_PER_SEC: u64 = 10_000_000;

#[cfg(windows)]
mod sys {
    /// Calendar fields in SYSTEMTIME order (public Win32 layout).
    #[repr(C)]
    #[derive(Default)]
    struct Calendar {
        f: [u16; 8],
    }

    #[repr(C)]
    #[derive(Default)]
    struct Ticks {
        low: u32,
        high: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(out: *mut Calendar);
        fn SystemTimeToFileTime(input: *const Calendar, out: *mut Ticks) -> i32;
    }

    /// The local clock as FILETIME ticks (`None` when the conversion fails).
    pub fn local_ticks() -> Option<u64> {
        let mut c = Calendar::default();
        let mut t = Ticks::default();
        // SAFETY: both pointers refer to live, correctly sized #[repr(C)] locals for the duration of the calls.
        let ok = unsafe {
            GetLocalTime(&mut c);
            SystemTimeToFileTime(&c, &mut t)
        };
        (ok != 0).then(|| (u64::from(t.high) << 32) | u64::from(t.low))
    }
}

/// The UTC clock as FILETIME ticks.
pub fn utc_ticks() -> u64 {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    UNIX_EPOCH_TICKS + d.as_secs() * TICKS_PER_SEC + u64::from(d.subsec_nanos() / 100)
}

/// The local wall clock as FILETIME ticks (UTC where the local clock cannot be read).
pub fn local_ticks() -> u64 {
    #[cfg(windows)]
    {
        if let Some(t) = sys::local_ticks() {
            return t;
        }
    }
    utc_ticks()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_clock_is_within_a_day_of_utc() {
        let (l, u) = (local_ticks(), utc_ticks());
        let day = 86_400 * TICKS_PER_SEC;
        assert!(l.abs_diff(u) <= day + 60 * TICKS_PER_SEC, "{l} vs {u}");
        assert!(u > UNIX_EPOCH_TICKS);
    }
}
