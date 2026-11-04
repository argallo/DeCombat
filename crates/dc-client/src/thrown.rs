//! Where a thrown object (a grenade in flight) is drawn between the server's position updates.
//!
//! The server streams a thrown object's position and velocity about ten times a second. Drawing the latest streamed
//! position as it is makes the object hop from sample to sample (the user's 2026-10-27 report: "grenade throws through
//! the air don't look smooth"). The 2009 engine keeps, per moved object, the last network position, velocity and the
//! client time it arrived, and each frame:
//!
//! 1. `t` = the time since that arrival, clamped to `0 ..= T` (`T` = 0.5 s, an engine data value);
//! 2. with a velocity and `t < T`: the target is `pos + vel * t` (no acceleration term: the gravity bit lives in the
//!    client-local upper half of the flag word, which the network flag block does not write); with no velocity the
//!    target is `pos`;
//! 3. the drawn position moves half-way from where it was drawn to the target;
//! 4. once `t` reaches `T` the object is drawn at `pos` and leaves the list (no further smoothing until the next
//!    update).
//!
//! Step 3 is per rendered frame in 2009, so the 2009 path depends on the frame rate (at 60 fps a correction is halved
//! every 16.7 ms, at 300 fps almost at once, and the drawn object trails its target by one frame's blend). DeCombat
//! draws `target(now) + offset`, where `offset` is the jump an update made (drawn position just before it minus the
//! new target) and halves every [`HALF_LIFE_S`] of clock time: the 2009 blend of a correction at 60 fps, with no
//! frame-rate trail, so every frame rate draws the same path (the user's frame-rate independence rule).
// EVIDENCE: engine+0x890a0 (per-frame pass over the moved-object list: clock advance, `t` clamped to the engine value at
// 0x6cb120, target from engine+0x88e10 `pos + vel * t + a * t * t / 2` with `a` = 0 unless object flag 0x20000, drawn =
// (previous drawn + target) * 0.5, at `t == T` the object is set to its network position and unlinked),
// engine+0x89590 (a network position: stores pos, vel and the arrival time, links the object into the list),
// engine+0xa4160 (the stream reader: no move when position and velocity changed by at most 0.1 units)
// DISASM: Engine.exe .data 0x6cb118 = 1 (smoothing on), 0x6cb120 = 0.5 (`T`, seconds)
// SPEC: UDP_SESSION_2009_B.md §4.2 item 3 (`CF_FLAGS`: the reader keeps the upper half of its 32-bit flag word), §4.3
// reader rules (0.1-unit test, client-side prediction)

use std::collections::BTreeMap;

/// The longest time a network position is extrapolated along its velocity (seconds).
// DISASM: Engine.exe .data 0x6cb120 (0.5)
pub const EXTRAPOLATE_MAX_S: f64 = 0.5;

/// A new network sample moves the object only when its position or velocity differs by more than this (squared).
// EVIDENCE: engine+0xa4160 (0.010000001 on both squared distances)
const SAME_SQ: f32 = 0.01;

/// The drawn position closes half of its distance to the target in this time.
// DC-DEFAULT: CLI-574 (proposed) an update's correction halves every 1/60 s of clock time (the 2009 per-frame half-way
// blend at 60 fps), added to the extrapolated target
pub const HALF_LIFE_S: f64 = 1.0 / 60.0;

#[derive(Debug, Clone, Copy)]
struct Flight {
    pos: [f32; 3],
    vel: [f32; 3],
    /// Arrival of `pos` / `vel` (seconds of the client clock).
    at: f64,
    /// The drawn position minus the target at `at` (decays with [`HALF_LIFE_S`]).
    offset: [f32; 3],
}

impl Flight {
    fn target(&self, now: f64) -> [f32; 3] {
        let t = (now - self.at).clamp(0.0, EXTRAPOLATE_MAX_S);
        if t >= EXTRAPOLATE_MAX_S {
            // the 2009 list drops the object: it is set to its network position
            return self.pos;
        }
        let t = t as f32;
        [
            self.pos[0] + self.vel[0] * t,
            self.pos[1] + self.vel[1] * t,
            self.pos[2] + self.vel[2] * t,
        ]
    }

    fn drawn(&self, now: f64) -> [f32; 3] {
        let p = self.target(now);
        if now - self.at >= EXTRAPOLATE_MAX_S {
            return p;
        }
        let k = 0.5f64.powf((now - self.at).max(0.0) / HALF_LIFE_S) as f32;
        [
            p[0] + self.offset[0] * k,
            p[1] + self.offset[1] * k,
            p[2] + self.offset[2] * k,
        ]
    }
}

/// The drawn positions of the thrown objects of a session.
#[derive(Debug, Clone, Default)]
pub struct ThrownMotion {
    flights: BTreeMap<u16, Flight>,
}

fn dist_sq(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

impl ThrownMotion {
    /// A position / velocity of `obj` as the network has it, arrived at `at` seconds. A first sample places the
    /// object; a sample equal to the current one (within 0.1 units) changes nothing.
    pub fn observe(&mut self, obj: u16, pos: [f32; 3], vel: [f32; 3], at: f64) {
        if !pos.iter().chain(vel.iter()).all(|c| c.is_finite()) {
            return;
        }
        match self.flights.get_mut(&obj) {
            None => {
                self.flights.insert(
                    obj,
                    Flight {
                        pos,
                        vel,
                        at,
                        offset: [0.0; 3],
                    },
                );
            }
            Some(f) => {
                // the same sample seen again (the session keeps the latest values) is not a new arrival
                if dist_sq(pos, f.pos) <= SAME_SQ && dist_sq(vel, f.vel) <= SAME_SQ {
                    return;
                }
                let was = f.drawn(at);
                f.pos = pos;
                f.vel = vel;
                f.at = at;
                f.offset = [was[0] - pos[0], was[1] - pos[1], was[2] - pos[2]];
            }
        }
    }

    /// The position to draw `obj` at, at `now` seconds (a function of the clock only, not of the frame rate).
    pub fn draw(&self, obj: u16, now: f64) -> Option<[f32; 3]> {
        Some(self.flights.get(&obj)?.drawn(now))
    }

    /// The objects with a flight.
    pub fn objects(&self) -> Vec<u16> {
        self.flights.keys().copied().collect()
    }

    /// Forgets the objects `live` says are gone.
    pub fn retain(&mut self, live: impl Fn(u16) -> bool) {
        self.flights.retain(|&k, _| live(k));
    }

    pub fn len(&self) -> usize {
        self.flights.len()
    }

    pub fn is_empty(&self) -> bool {
        self.flights.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    const G: f32 = -1100.0;

    fn arc(t: f32) -> ([f32; 3], [f32; 3]) {
        let p0 = [0.0f32, 100.0, 400.0];
        let v0 = [300.0f32, 500.0, 900.0];
        (
            [
                p0[0] + v0[0] * t,
                p0[1] + v0[1] * t + 0.5 * G * t * t,
                p0[2] + v0[2] * t,
            ],
            [v0[0], v0[1] + G * t, v0[2]],
        )
    }

    fn screen(cam: &dc_render::Camera, p: [f32; 3]) -> glam::Vec2 {
        let c = cam.view_projection() * Vec3::from_array(p).extend(1.0);
        let n = c.truncate() / c.w;
        glam::Vec2::new((n.x * 0.5 + 0.5) * 1024.0, (0.5 - n.y * 0.5) * 768.0)
    }

    /// The user's 2026-10-27 report: a thrown grenade hops through the air. A 10 Hz stream of an exact ballistic arc,
    /// drawn for 60 frames at 240 fps at 1024x768: no screen step is larger than the per-frame ballistic delta (plus
    /// the half-way blend of the 5.5-unit gravity correction each update brings). Drawing the streamed position as it
    /// is (the old way) steps once per update by the whole 0.1 s of flight.
    #[test]
    fn a_streamed_grenade_moves_smoothly_at_240_fps() {
        let (fx, fy) =
            dc_render::fov_pair_2009(dc_render::fov_y_deg_for_reference_x(90.0), 1024, 768);
        let cam = dc_render::Camera::looking(Vec3::new(-300.0, 150.0, 0.0), 0.6, 0.0, fx, fy);
        let mut m = ThrownMotion::default();
        let frame = 1.0 / 240.0;
        let mut new_steps = Vec::new();
        let mut old_steps = Vec::new();
        let mut true_steps = Vec::new();
        let mut last_net = None;
        let (mut prev_new, mut prev_old, mut prev_true): (
            Option<glam::Vec2>,
            Option<glam::Vec2>,
            Option<glam::Vec2>,
        ) = (None, None, None);
        for i in 0..90 {
            let now = 0.05 + i as f64 * frame;
            // the stream: one sample per 100 ms
            let tick = (now * 10.0).floor() / 10.0;
            if last_net != Some(tick) {
                last_net = Some(tick);
                let (p, v) = arc(tick as f32);
                m.observe(10, p, v, now);
            }
            let drawn = m.draw(10, now).unwrap();
            let (truth, _) = arc(now as f32);
            let (old, _) = arc(tick as f32);
            let (sn, so, st) = (screen(&cam, drawn), screen(&cam, old), screen(&cam, truth));
            // the first 30 frames settle the start; 60 frames are measured
            if i >= 30 {
                new_steps.push((sn - prev_new.unwrap()).length());
                old_steps.push((so - prev_old.unwrap()).length());
                true_steps.push((st - prev_true.unwrap()).length());
            }
            (prev_new, prev_old, prev_true) = (Some(sn), Some(so), Some(st));
        }
        assert_eq!(new_steps.len(), 60);
        let max_true = true_steps.iter().cloned().fold(0.0f32, f32::max);
        let max_new = new_steps.iter().cloned().fold(0.0f32, f32::max);
        let max_old = old_steps.iter().cloned().fold(0.0f32, f32::max);
        for (k, (n, t)) in new_steps.iter().zip(&true_steps).enumerate() {
            assert!(
                *n <= t * 1.25 + 0.5,
                "frame {k}: step {n:.2} px vs ballistic {t:.2} px"
            );
        }
        // the largest step comes in the frames after an update (half of the 5.5-unit gravity correction per 1/60 s,
        // as the 2009 half-way blend does per frame): within 1.5 x the largest ballistic step
        assert!(max_new <= max_true * 1.5, "{max_new} vs {max_true}");
        // the old path held still for 23 frames, then jumped by the whole 0.1 s
        assert!(max_old > 5.0 * max_true, "{max_old} vs {max_true}");
    }

    #[test]
    fn the_same_frame_rate_free_path_at_60_and_240_fps() {
        let run = |fps: f64| -> [f32; 3] {
            let mut m = ThrownMotion::default();
            let (p, v) = arc(0.0);
            m.observe(1, p, v, 0.0);
            let mut out = p;
            let n = (0.3 * fps).round() as usize;
            for i in 1..=n {
                let now = i as f64 / fps;
                // the network thread stamps a sample when it arrives, not at the next frame
                if (now * 10.0).floor() > ((now - 1.0 / fps) * 10.0).floor() {
                    let tk = (now * 10.0).floor() / 10.0;
                    let (p, v) = arc(tk as f32);
                    m.observe(1, p, v, tk);
                }
                out = m.draw(1, now).unwrap();
            }
            out
        };
        let a = run(60.0);
        let b = run(240.0);
        assert!(dist_sq(a, b) < 1e-6, "{a:?} vs {b:?}");
    }

    #[test]
    fn after_half_a_second_without_an_update_it_rests_on_the_network_position() {
        let mut m = ThrownMotion::default();
        m.observe(3, [0.0, 0.0, 0.0], [100.0, 0.0, 0.0], 0.0);
        let mid = m.draw(3, 0.25).unwrap();
        assert!((mid[0] - 25.0).abs() < 1e-3);
        assert!(mid[0] > 0.0);
        assert_eq!(m.draw(3, 0.6), Some([0.0, 0.0, 0.0]));
        assert_eq!(m.draw(3, 0.7), Some([0.0, 0.0, 0.0]));
    }
}
