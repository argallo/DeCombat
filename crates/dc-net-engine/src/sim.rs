//! A deterministic simulated datagram channel (loss, duplication, corruption, random delay and therefore
//! reordering) for tests of anything built on links. Our own test tooling; the random source is SplitMix64
//! (public algorithm, Steele / Lea / Flood 2014).

use crate::time::{later, Millis};

/// SplitMix64 pseudo-random generator.
#[derive(Debug, Clone)]
pub struct SimRng(u64);

impl SimRng {
    pub fn new(seed: u64) -> Self {
        SimRng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (`n > 0`).
    pub fn below(&mut self, n: u32) -> u32 {
        (self.next_u64() % n.max(1) as u64) as u32
    }

    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: u32, hi: u32) -> u32 {
        if hi <= lo {
            lo
        } else {
            lo + self.below(hi - lo + 1)
        }
    }

    /// True with probability `permille / 1000`.
    pub fn chance(&mut self, permille: u32) -> bool {
        permille > 0 && self.below(1000) < permille
    }
}

/// Fault settings of one direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Faults {
    pub loss_permille: u32,
    pub dup_permille: u32,
    /// Flips one random bit of the datagram.
    pub corrupt_permille: u32,
    pub min_delay_ms: u32,
    pub max_delay_ms: u32,
}

impl Faults {
    /// A perfect channel with a fixed delay.
    pub fn clean(delay_ms: u32) -> Self {
        Faults {
            min_delay_ms: delay_ms,
            max_delay_ms: delay_ms,
            ..Faults::default()
        }
    }
}

/// One direction of a simulated network path.
#[derive(Debug, Clone)]
pub struct SimChannel {
    pub faults: Faults,
    /// A path that loses every datagram longer than this many bytes (a small MTU with fragments dropped).
    pub drop_over_bytes: Option<usize>,
    pub oversize_lost: u64,
    rng: SimRng,
    flight: Vec<(Millis, u64, Vec<u8>)>,
    order: u64,
    pub sent: u64,
    pub lost: u64,
    pub duplicated: u64,
    pub corrupted: u64,
    pub delivered: u64,
}

impl SimChannel {
    pub fn new(seed: u64, faults: Faults) -> Self {
        SimChannel {
            faults,
            drop_over_bytes: None,
            oversize_lost: 0,
            rng: SimRng::new(seed),
            flight: Vec::new(),
            order: 0,
            sent: 0,
            lost: 0,
            duplicated: 0,
            corrupted: 0,
            delivered: 0,
        }
    }

    /// Puts a datagram on the wire at `now`.
    pub fn send(&mut self, now: Millis, mut bytes: Vec<u8>) {
        self.sent += 1;
        if self.drop_over_bytes.is_some_and(|m| bytes.len() > m) {
            self.oversize_lost += 1;
            return;
        }
        if self.rng.chance(self.faults.loss_permille) {
            self.lost += 1;
            return;
        }
        if !bytes.is_empty() && self.rng.chance(self.faults.corrupt_permille) {
            let bit = self.rng.below(bytes.len() as u32 * 8);
            bytes[(bit / 8) as usize] ^= 1 << (bit % 8);
            self.corrupted += 1;
        }
        let copies = if self.rng.chance(self.faults.dup_permille) {
            self.duplicated += 1;
            2
        } else {
            1
        };
        for _ in 0..copies {
            let delay = self
                .rng
                .range(self.faults.min_delay_ms, self.faults.max_delay_ms);
            self.order += 1;
            self.flight
                .push((now.wrapping_add(delay), self.order, bytes.clone()));
        }
    }

    /// Datagrams whose delivery time has come, in delivery order.
    pub fn deliver(&mut self, now: Millis) -> Vec<Vec<u8>> {
        let mut due: Vec<(Millis, u64, Vec<u8>)> = Vec::new();
        let mut keep = Vec::with_capacity(self.flight.len());
        for f in self.flight.drain(..) {
            if later(f.0, now) {
                keep.push(f);
            } else {
                due.push(f);
            }
        }
        self.flight = keep;
        due.sort_by_key(|f| (f.0, f.1));
        self.delivered += due.len() as u64;
        due.into_iter().map(|f| f.2).collect()
    }

    pub fn in_flight(&self) -> usize {
        self.flight.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_reordering() {
        let f = Faults {
            loss_permille: 100,
            dup_permille: 100,
            corrupt_permille: 0,
            min_delay_ms: 0,
            max_delay_ms: 50,
        };
        let run = || {
            let mut c = SimChannel::new(42, f);
            let mut out = Vec::new();
            for t in 0..200u32 {
                c.send(t, vec![t as u8]);
                out.extend(c.deliver(t));
            }
            out.extend(c.deliver(1000));
            out
        };
        let a = run();
        assert_eq!(a, run());
        assert!(a.windows(2).any(|w| w[0][0] > w[1][0]), "some reordering");
        assert!(a.len() < 220 && a.len() > 150);
    }
}
