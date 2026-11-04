//! Fixed-timestep driver: an exact integer accumulator that turns wall-clock time into a whole number of
//! simulation ticks, so the server and every client run the same steps with the same `dt`.
//!
//! The 2009 client steps once per rendered frame with a variable `dt` (MOVEMENT_2009.md §4); a fixed step
//! is a DeCombat decision and differs from the 2009 feel by design in the frame-rate-dependent details the
//! spec lists in §15 item 3-4 (the jump apex, for example, is 50.88 at 60 Hz and 57.75 at 10 Hz).
// SPEC: MOVEMENT_2009.md §4 (no fixed-step sub-stepping on the client; a shared crate may run a fixed step),
// §15 item 3

use crate::collide::Collider;
use crate::params::MovementParams;
use crate::player::{PlayerState, StepEvents, TickInput};

/// Default simulation rate. PROPOSED default (staged for DC_DEFAULTS.md as a new row, not yet approved):
/// 60 Hz, the Rust client's target rate (CD-63 note); the network tick rate is separate (CD-63).
pub const DEFAULT_TICK_HZ: u32 = 60;

/// Exact fixed-step accumulator. Time is accumulated in microseconds times the rate, so no rounding drift
/// builds up: after `T` microseconds exactly `floor(T * hz / 1e6)` ticks have run (minus any dropped
/// backlog, see [`FixedStep::max_steps`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedStep {
    hz: u32,
    /// Accumulated `microseconds * hz`, below 1e6 after every [`FixedStep::advance`].
    acc: u64,
    /// Upper bound of ticks per `advance` call; a longer backlog is dropped (no spiral of death).
    pub max_steps: u32,
    ticks: u64,
}

impl FixedStep {
    pub fn new(hz: u32) -> FixedStep {
        FixedStep {
            hz: hz.max(1),
            acc: 0,
            max_steps: 8,
            ticks: 0,
        }
    }

    pub fn hz(&self) -> u32 {
        self.hz
    }

    /// The step length every tick uses.
    pub fn dt(&self) -> f32 {
        1.0 / self.hz as f32
    }

    /// Ticks run so far.
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// Adds elapsed wall time and returns how many ticks to run now.
    pub fn advance(&mut self, elapsed_us: u64) -> u32 {
        self.acc = self
            .acc
            .saturating_add(elapsed_us.saturating_mul(self.hz as u64));
        let due = self.acc / 1_000_000;
        let run = if due > self.max_steps as u64 {
            self.max_steps as u64
        } else {
            due
        };
        self.acc -= due * 1_000_000; // a backlog beyond max_steps is dropped
        self.ticks += run;
        run as u32
    }

    /// Fraction of the next tick already elapsed, for render interpolation (0..1).
    pub fn alpha(&self) -> f32 {
        self.acc as f32 / 1_000_000.0
    }
}

impl Default for FixedStep {
    fn default() -> Self {
        FixedStep::new(DEFAULT_TICK_HZ)
    }
}

/// One player simulated at a fixed rate against a world: the same type runs on the server (authoritative
/// re-simulation of received inputs) and on the client (prediction).
pub struct Simulation<'w, W: Collider + ?Sized> {
    pub params: MovementParams,
    pub world: &'w W,
    pub state: PlayerState,
    pub clock: FixedStep,
}

impl<'w, W: Collider + ?Sized> Simulation<'w, W> {
    pub fn new(params: MovementParams, world: &'w W, state: PlayerState, hz: u32) -> Self {
        Simulation {
            params,
            world,
            state,
            clock: FixedStep::new(hz),
        }
    }

    /// Runs exactly one tick with `input`.
    pub fn tick(&mut self, input: &TickInput<'_>) -> StepEvents {
        let dt = self.clock.dt();
        self.state.step(&self.params, self.world, input, dt)
    }

    /// Advances by wall time; `input_for(tick_index)` supplies each tick's input. Returns the ticks run and
    /// the union of their events.
    pub fn advance<'i, F>(&mut self, elapsed_us: u64, mut input_for: F) -> (u32, StepEvents)
    where
        F: FnMut(u64) -> TickInput<'i>,
    {
        let first = self.clock.ticks();
        let n = self.clock.advance(elapsed_us);
        let mut all = StepEvents::default();
        for i in 0..n as u64 {
            let input = input_for(first + i);
            let e = self.tick(&input);
            all.jumped |= e.jumped;
            all.fall_started |= e.fall_started;
            all.landed_from_fall |= e.landed_from_fall;
            all.landed_from_jump |= e.landed_from_jump;
            all.moved |= e.moved;
        }
        (n, all)
    }
}

/// Re-simulates `inputs` from `state` at a fixed `dt` (server validation, client replay after a
/// correction).
pub fn replay<W: Collider + ?Sized>(
    params: &MovementParams,
    world: &W,
    state: &mut PlayerState,
    inputs: &[TickInput<'_>],
    dt: f32,
) {
    for i in inputs {
        state.step(params, world, i, dt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulator_is_exact() {
        let mut c = FixedStep::new(60);
        c.max_steps = u32::MAX;
        let mut total = 0u64;
        // 1 s in uneven chunks
        for us in [16_000u64, 17_000, 1, 999, 333_333, 632_667] {
            total += c.advance(us) as u64;
        }
        assert_eq!(total, 60);
        assert_eq!(c.ticks(), 60);
        assert_eq!(c.alpha(), 0.0);
        assert_eq!(c.dt(), 1.0 / 60.0);
    }

    #[test]
    fn backlog_is_capped() {
        let mut c = FixedStep::new(60);
        assert_eq!(c.advance(10_000_000), 8);
        assert_eq!(c.advance(0), 0);
    }

    #[test]
    fn alpha_tracks_the_partial_tick() {
        let mut c = FixedStep::new(100);
        assert_eq!(c.advance(15_000), 1);
        assert!((c.alpha() - 0.5).abs() < 1e-6);
    }
}
