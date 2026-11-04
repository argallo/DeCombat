//! Flow control: the two-counter token bucket of the 2009 link.

use crate::time::{since, Millis};
use dc_wire::udp::{flow_blocked, flow_window_bits, FLOW_TICK_MIN_MS};

/// Token bucket with a fill counter `used <= W` and an overflow counter `debt`.
///
/// `W = rate_bps * 200 / 1000` bits; a cost is blocked when `2W < used + debt + cost`. Draining happens only when
/// at least 17 ms passed since the last tick and empties `debt` first, then `used`.
// SPEC: UDP_TRANSPORT_2009.md §12.2, §12.3 (engine+0xad650, engine+0xad7c0, engine+0xad5e0)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowBucket {
    rate_bps: u32,
    window: u32,
    used: u32,
    debt: u32,
    last: Millis,
}

impl FlowBucket {
    /// A bucket for `rate_bps`, reset at `now` (the first use resets, spec §12.3).
    pub fn new(rate_bps: u32, now: Millis) -> Self {
        let mut b = FlowBucket {
            rate_bps,
            window: 0,
            used: 0,
            debt: 0,
            last: now,
        };
        b.reset(now);
        b
    }

    /// Reset: `W` from the rate, `debt = min(debt, W)`, `used = min(used, W)`, `last = now`.
    // SPEC: UDP_TRANSPORT_2009.md §12.3 (engine+0xad5e0)
    pub fn reset(&mut self, now: Millis) {
        self.window = flow_window_bits(self.rate_bps);
        self.debt = self.debt.min(self.window);
        self.used = self.used.min(self.window);
        self.last = now;
    }

    /// Changes the rate and resets the window.
    pub fn change_rate(&mut self, rate_bps: u32, now: Millis) {
        self.rate_bps = rate_bps;
        self.reset(now);
    }

    /// Drains `(now - last) * rate / 1000` bits when at least 17 ms passed. When `debt >= drain` the drain moves
    /// from `debt` to `used` and back out of `used`, which leaves `used` unchanged; otherwise `debt = 0` and
    /// `used` drains by the full amount.
    // SPEC: UDP_TRANSPORT_2009.md §12.3 (engine+0xad650)
    pub fn tick(&mut self, now: Millis) {
        let dt = since(now, self.last);
        if dt < FLOW_TICK_MIN_MS {
            return;
        }
        let drain = ((dt as u64 * self.rate_bps as u64) / 1000).min(u32::MAX as u64) as u32;
        if self.debt < drain {
            self.debt = 0;
            self.used = self.used.saturating_sub(drain);
        } else {
            self.debt -= drain;
        }
        self.last = now;
    }

    /// Adds a cost: `used` fills up to `W`, the rest goes to `debt`.
    // SPEC: UDP_TRANSPORT_2009.md §12.3 (engine+0xad650 add path)
    pub fn add(&mut self, cost: u32) {
        let room = self.window.saturating_sub(self.used);
        let into_used = cost.min(room);
        self.used += into_used;
        self.debt = self.debt.saturating_add(cost - into_used);
    }

    /// `2W < used + debt + cost`.
    // SPEC: UDP_TRANSPORT_2009.md §12.3 (engine+0xad7c0)
    pub fn blocked(&self, cost: u32) -> bool {
        flow_blocked(self.window, self.used, self.debt, cost)
    }

    pub fn rate_bps(&self) -> u32 {
        self.rate_bps
    }

    /// `W` in bits.
    pub fn window_bits(&self) -> u32 {
        self.window
    }

    pub fn used(&self) -> u32 {
        self.used
    }

    pub fn debt(&self) -> u32 {
        self.debt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_fill_debt_and_drain() {
        // 40000 bps -> W = 8000 bits per 200 ms, blocked above 2W = 16000.
        let mut b = FlowBucket::new(40_000, 0);
        assert_eq!(b.window_bits(), 8000);
        assert!(!b.blocked(16_000));
        assert!(b.blocked(16_001));
        b.add(10_000);
        assert_eq!((b.used(), b.debt()), (8000, 2000));
        assert!(!b.blocked(6000));
        assert!(b.blocked(6001));
        // under 17 ms nothing drains
        b.tick(16);
        assert_eq!((b.used(), b.debt()), (8000, 2000));
        // 25 ms -> 1000 bits: debt 2000 >= 1000, debt drains first, used unchanged
        b.tick(25);
        assert_eq!((b.used(), b.debt()), (8000, 1000));
        // 100 ms later -> 4000 bits: debt < drain -> debt 0, used - 4000
        b.tick(125);
        assert_eq!((b.used(), b.debt()), (4000, 0));
        b.tick(1125);
        assert_eq!((b.used(), b.debt()), (0, 0));
    }

    #[test]
    fn rate_change_resets_window_and_clamps() {
        let mut b = FlowBucket::new(1_000_000, 0);
        b.add(150_000);
        b.change_rate(40_000, 10);
        assert_eq!(b.window_bits(), 8000);
        assert!(b.used() <= 8000 && b.debt() <= 8000);
    }

    #[test]
    fn initial_link_rate_never_overflows() {
        let mut b = FlowBucket::new(dc_wire::udp::INITIAL_LINK_RATE_BPS, 0);
        for _ in 0..1000 {
            b.add(u32::MAX / 2);
        }
        assert!(b.blocked(1));
        b.tick(1_000_000);
        b.tick(4_000_000_000);
    }
}
