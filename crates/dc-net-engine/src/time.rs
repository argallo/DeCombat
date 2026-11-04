//! Millisecond clock arithmetic. Times are `timeGetTime`-style milliseconds: `u32`, wrapping at 2^32, and are
//! always injected by the caller, so every state machine in this crate is deterministic.

/// A point in time in milliseconds (wraps at 2^32).
// SPEC: UDP_TRANSPORT_2009.md conventions (times compared with unsigned 32-bit subtraction)
pub type Millis = u32;

/// Milliseconds from `then` to `now` (unsigned 32-bit subtraction).
#[inline]
pub fn since(now: Millis, then: Millis) -> u32 {
    now.wrapping_sub(then)
}

/// True when `a` lies after `b` (within half the clock range).
#[inline]
pub fn later(a: Millis, b: Millis) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

/// The earlier of two stamps (wrap-aware `min`).
#[inline]
pub fn earliest(a: Millis, b: Millis) -> Millis {
    if later(a, b) {
        b
    } else {
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_aware_order() {
        assert_eq!(since(5, u32::MAX - 4), 10);
        assert!(later(5, u32::MAX - 4));
        assert!(!later(u32::MAX - 4, 5));
        assert_eq!(earliest(5, u32::MAX - 4), u32::MAX - 4);
        assert_eq!(earliest(7, 7), 7);
    }
}
