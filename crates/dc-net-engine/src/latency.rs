//! Round-trip estimate of a link and the truncated latency `L_ms` its timers use.

use crate::time::{since, Millis};
use dc_wire::udp::{INITIAL_RTT_MS, RTT_SAMPLES, TODO_LATENCY_USES_EFFECTIVE};

/// Mean of the last up to 16 round-trip samples (initially 200 ms) and the effective latency
/// `L_eff = waiting ? max(rtt_mean, now - wait_start) : rtt_mean`.
// SPEC: UDP_TRANSPORT_2009.md §12.1 (engine+0xabb40, engine+0xad580, engine+0xb48b0)
#[derive(Debug, Clone, PartialEq)]
pub struct Latency {
    ring: [f32; RTT_SAMPLES],
    count: usize,
    next: usize,
    mean: f32,
    waiting: bool,
    wait_start: Millis,
    effective: f32,
    /// Samples taken since the link opened (diagnostics: lets a reader pick up each new sample once).
    total: u64,
}

impl Default for Latency {
    fn default() -> Self {
        Latency {
            ring: [0.0; RTT_SAMPLES],
            count: 0,
            next: 0,
            mean: INITIAL_RTT_MS,
            waiting: false,
            wait_start: 0,
            effective: INITIAL_RTT_MS,
            total: 0,
        }
    }
}

impl Latency {
    /// Records one round trip in milliseconds; a new sample clears `waiting`.
    // SPEC: UDP_TRANSPORT_2009.md §12.1 (engine+0xabb40)
    pub fn sample(&mut self, rtt_ms: u32) {
        self.ring[self.next] = rtt_ms as f32;
        self.next = (self.next + 1) % RTT_SAMPLES;
        self.count = (self.count + 1).min(RTT_SAMPLES);
        let sum: f32 = self.ring[..self.count].iter().sum();
        self.mean = sum / self.count as f32;
        self.waiting = false;
        self.effective = self.mean;
        self.total += 1;
    }

    /// First reliable send while no record is outstanding: stamp `wait_start` and set `waiting`.
    // SPEC: UDP_TRANSPORT_2009.md §11.2 (link+0x144 / link+0x140)
    pub fn start_waiting(&mut self, now: Millis) {
        self.waiting = true;
        self.wait_start = now;
    }

    /// Refreshes `L_eff` (every link update).
    // SPEC: UDP_TRANSPORT_2009.md §12.1 (engine+0xad580)
    pub fn refresh(&mut self, now: Millis) {
        self.effective = if self.waiting {
            self.mean.max(since(now, self.wait_start) as f32)
        } else {
            self.mean
        };
    }

    /// `L_ms`: the truncated latency the timers compare with. Which estimate feeds the truncation is UNPROVEN;
    /// `dc_wire::udp::TODO_LATENCY_USES_EFFECTIVE` picks `L_eff` (the spec's likelier reading).
    // SPEC: UDP_TRANSPORT_2009.md §12.1 (truncation engine+0x1de5e4, input UNPROVEN, V7)
    pub fn l_ms(&self) -> u32 {
        let v = if TODO_LATENCY_USES_EFFECTIVE {
            self.effective
        } else {
            self.mean
        };
        v.max(0.0) as u32
    }

    pub fn rtt_mean(&self) -> f32 {
        self.mean
    }

    pub fn effective(&self) -> f32 {
        self.effective
    }

    pub fn waiting(&self) -> bool {
        self.waiting
    }

    pub fn sample_count(&self) -> usize {
        self.count
    }

    /// Samples taken since the link opened and the newest one (ms), for link statistics.
    // DC-DEFAULT: DSV-412 (proposed) the dserver's link window reads every raw round-trip sample
    pub fn newest(&self) -> (u64, Option<f32>) {
        if self.count == 0 {
            return (self.total, None);
        }
        let i = (self.next + self.ring.len() - 1) % self.ring.len();
        (self.total, Some(self.ring[i]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_of_last_sixteen() {
        let mut l = Latency::default();
        assert_eq!(l.l_ms(), 200);
        for v in 0..20u32 {
            l.sample(v * 10);
        }
        // samples 40..190 step 10 remain
        assert_eq!(l.rtt_mean(), 115.0);
        assert_eq!(l.l_ms(), 115);
    }

    #[test]
    fn waiting_raises_effective_latency() {
        let mut l = Latency::default();
        l.sample(20);
        l.start_waiting(1000);
        l.refresh(1010);
        assert_eq!(l.effective(), 20.0);
        l.refresh(1500);
        assert_eq!(l.effective(), 500.0);
        l.sample(30);
        assert!(!l.waiting());
        l.refresh(5000);
        assert_eq!(l.effective(), 25.0);
    }
}
