//! Camera recoil of the local player's view, the way the 2009 client computes it.
//!
//! The 2009 client keeps the recoil on the view controller, not on the weapon: a pending vertical amount `a`
//! (controller +0x54), a pending horizontal amount `w` (+0x58), the side of the horizontal kick (+0x5c), the tick
//! of the last kick (+0x60) and a kick-back distance (+0x64). Every shot:
//! - **pitch kick**: `k = 0.5 x (rad(BaseCamRecoilPitch) x (1 - a / m) + BaseCamRecoilAccelPitch x a)` (the accel term
//!   only while the first part is positive), `m = rad(MaxCamRecoilPitch)`, `k` capped so `a + k <= m`; `a += k`,
//!   the view pitch `-= 2k` (up);
//! - **kick-back**: `back := max(back, 0) + BaseCamBackJitter`;
//! - **yaw kick** (the "wag"), only once `a` exceeds `MaxRandomPitch` (a raw row value compared with radians): `f =
//!   rad(BaseCamRecoilWagL or BaseCamRecoilWagR) x (1 - |w| / rad(MaxCamRecoilWag))`, `w += f / 2`, the view yaw
//!   `-= f`; then, when `|w|` exceeds `MaxRandomWag` (raw, radians) and the millisecond tick clock reads an odd
//!   value, the side flips. The ROM rows give the two sides opposite signs (L positive, R negative), so the view
//!   walks one way for a few shots and turns back at a random flip. The tick clock's low bit is the 2009 client's
//!   only random source here (`GetTickCount() & 1`).
//!
//! Every frame both pending amounts drain back into the view: `r = dt x rate x (1 + |p| / max)`, `r <= |p|`, `p -=
//! r / 2` (signed), the angle `+= r`, with `rate` = `JitterWaitRecover` while the tick clock says the last kick is
//! younger than `JitterDecDelay` ms and `JitterRecover` after; `max` is `MaxCamRecoilPitch` for the pitch and
//! `MaxCamRecoilWag` for the yaw (degrees to radians). Since the kick moved the angle by `2 x` what it added to the
//! pending amount and the drain moves it by `2 x` what it takes, the view's offset from the aim before the burst is
//! always `-2 x` the pending amount: the view comes back exactly where it was. The kick-back distance falls by `dt
//! x BackJitterRecover x 5` and pushes the eye back along the view by `10 x back` units.
//!
//! DeCombat runs the drain on the session clock in steps of at most 1 ms (the 2009 client used the frame time), and
//! a shot first drains up to its own time and then kicks (the 2009 frame drained before the fire check), so the view
//! is a function of the session clock and the shot times only, never of the frame rate (project decision: no
//! frame-rate cap, frame-rate independence by design).
//!
//! Numbers come from the user's ROM at run time ([`crate::spread::RecoilRow`]: the weapon row and its ammunition
//! row); a row without a key takes the 2009 client's code default ([`crate::spread::recoil_default`]).
// EVIDENCE: cshell+0x1f8b40 (camera update: the drain cshell+0x1f3070 first; on a fired shot the kick cshell+0x1f5150,
// then the wag cshell+0x1f2f60 when the mods' MinPerturb-row term + MaxRandomPitch < |a|)
// EVIDENCE: cshell+0x1f5150 (pitch kick, kick tick, kick-back), cshell+0x1f2f60 (wag: side flag +0x5c, flip on
// MaxRandomWag < |w| and an odd GetTickCount), cshell+0x1f3070 (drain of a, back, w; DISASM cshell+0x1f312d..0x1f3176
// for the pitch factor the clean C loses, DISASM cshell+0x1f3286..0x1f3365 for the yaw drain)
// DISASM: cshell+0x17dd12..0x17def9 (the row keys of +0x2f0..+0x32c: MaxCamRecoilPitch, BaseCamRecoilPitch,
// BaseCamRecoilAccelPitch, MaxCamRecoilYaw, BaseCamRecoilYaw, MaxCamRecoilWag, BaseCamRecoilWagR, BaseCamRecoilWagL,
// MaxRandomPitch, MaxRandomWag, two screen keys, TripleShotMaxCamRecoilPitch, TripleShotBaseCamRecoilPitch,
// BaseCamBackJitter, BackJitterRecover)
// EVIDENCE: cshell+0x1f9c40 (eye -= view forward x 10 x back)

use crate::spread::{tick_of, RecoilRow};

/// Degrees to radians the way the client multiplies (`x 3.1415927 x 0.0055555557`).
// EVIDENCE: cshell+0x1f5150 (every row angle is scaled by the f32 constants pi and 1/180)
pub fn rad(deg: f32) -> f32 {
    deg * std::f32::consts::PI * (1.0 / 180.0)
}

/// Largest drain step (session ms).
// DC-DEFAULT: CLI-64 (proposed) 1 ms integration steps
pub const DRAIN_STEP_MS: f64 = 1.0;

/// A change of the view angles (radians): pitch positive looks down, yaw is added to the view yaw.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ViewKick {
    pub pitch: f32,
    pub yaw: f32,
}

impl ViewKick {
    pub fn add(&mut self, o: ViewKick) {
        self.pitch += o.pitch;
        self.yaw += o.yaw;
    }
}

/// The recoil state of the view (the 2009 controller fields +0x54..+0x64).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CameraRecoil {
    /// Pending vertical recoil `a` (radians, >= 0).
    pub pitch_acc: f32,
    /// Pending horizontal recoil `w` (radians, signed).
    pub wag_acc: f32,
    /// The side of the next yaw kick: `false` = `BaseCamRecoilWagL`, `true` = `BaseCamRecoilWagR`.
    pub wag_right: bool,
    /// Tick clock value of the last kick.
    kick_tick: Option<u64>,
    /// Kick-back distance (the eye moves back by `10 x back` units).
    pub back: f32,
    last_drain: Option<f64>,
    /// Counters for tests and reports: yaw kicks, side flips.
    pub wags: u32,
    pub flips: u32,
}

/// The recoil terms of the mods fitted to the held weapon, combined the way the 2009 kick combines its three sockets.
/// [`Default`] is "no mod": the row's own kick.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct KickMods {
    /// The pitch bound's factor less 1 (`MaxCamRecoilPitchPer` of the last filled socket).
    pub max_per: f32,
    /// The base's factor less 1 (`BaseCamRecoilPitch` + `BaseCamRecoilPitchPer` of the last filled socket).
    pub base_per: f32,
    /// `BaseCamRecoilAccelPitch` summed over the sockets.
    pub accel: f32,
    /// `MaxRandomPitch` summed over the sockets.
    pub random_pitch: f32,
}

impl KickMods {
    /// The terms of the mods in sockets 0, 1, 2. The two factors are not sums: the 2009 loop stores each filled
    /// socket's value over the last, so the highest filled socket alone counts (a suppressor's -0.1 base factor is
    /// lost once a magazine mod sits in socket 2, whose row holds 0); the acceleration and the threshold are summed.
    // DISASM: cshell+0x1f5190..0x1f51b4 (per filled socket: `mov` of +0x238 and `fstp` of +0x258 + +0x23c into the
    // two locals, `fadd` of +0x25c into the third); EVIDENCE: cshell+0x1f8b40 (+0x254 summed)
    pub fn of_sockets(mods: [Option<&crate::zoom::ModRecoil>; 3]) -> KickMods {
        let mut out = KickMods::default();
        for m in mods.into_iter().flatten() {
            out.max_per = m.max_per;
            out.base_per = m.base + m.base_per;
            out.accel += m.accel;
            out.random_pitch += m.random_pitch;
        }
        out
    }
}

/// The pitch bounds of a kick: the triple-shot pair when the mode is on and the row has both, else the plain pair.
// EVIDENCE: cshell+0x1f5150, cshell+0x1f3070 (weapon method +0xf8 true and row +0x320 / +0x324 non-zero: the
// TripleShot* pair; PLAUSIBLE that +0xf8 is the triple-shot mode, by the key names of +0x320 / +0x324)
fn pitch_pair(r: &RecoilRow, triple: bool) -> (f32, f32) {
    if triple && r.triple_max_deg != 0.0 && r.triple_base_deg != 0.0 {
        (r.triple_max_deg, r.triple_base_deg)
    } else {
        (r.max_deg, r.base_deg)
    }
}

impl CameraRecoil {
    /// Back to rest (spawn).
    // EVIDENCE: cshell+0x1f20b0 (controller reset: +0x54, +0x58, +0x60, +0x64 := 0; the side flag +0x5c is not reset)
    pub fn reset(&mut self) {
        let side = self.wag_right;
        let (wags, flips) = (self.wags, self.flips);
        *self = CameraRecoil::default();
        self.wag_right = side;
        self.wags = wags;
        self.flips = flips;
    }

    /// The kick of a shot fired at session time `now_ms`: the drain up to the shot, then the pitch kick, the
    /// kick-back and (past `MaxRandomPitch`) the yaw kick. Returns the view change.
    pub fn kick(&mut self, r: &RecoilRow, now_ms: f64, tick_ms: f64, triple: bool) -> ViewKick {
        self.kick_with(r, &KickMods::default(), now_ms, tick_ms, triple)
    }

    /// [`CameraRecoil::kick`] of a weapon with mods fitted: the pitch bound and the base scaled by the mods' factors,
    /// their acceleration and yaw-kick threshold terms added (the drain keeps the row's own bound).
    // EVIDENCE: cshell+0x1f5150 (bound = (1 + mod +0x238) x row +0x2f0, base = (1 + mod +0x258 + mod +0x23c) x row
    // +0x2f4, accel = sum of mod +0x25c + row +0x2f8; the same factors on the TripleShot pair), cshell+0x1f8b40 (the yaw
    // kick when the three mods' +0x254 summed + row +0x310 < |a|), cshell+0x1f3070 (the drain reads the row bound only)
    pub fn kick_with(
        &mut self,
        r: &RecoilRow,
        m: &KickMods,
        now_ms: f64,
        tick_ms: f64,
        triple: bool,
    ) -> ViewKick {
        let mut out = self.drain(r, now_ms, tick_ms, triple);
        let (max_deg, base_deg) = pitch_pair(r, triple);
        let max_deg = max_deg * (1.0 + m.max_per);
        let base_deg = base_deg * (1.0 + m.base_per);
        let max = rad(max_deg);
        let tick = tick_of(now_ms, tick_ms);
        self.kick_tick = Some(tick);
        // DC-DEFAULT: CLI-310 (proposed) a row whose MaxCamRecoilPitch is 0 or less kicks no pitch (the 2009 client
        // divides by it)
        if max > 0.0 {
            let a = self.pitch_acc.max(0.0).min(max);
            let mut k = rad(base_deg) * (1.0 - a / max);
            if k > 0.0 {
                k += (r.accel + m.accel) * a;
            }
            k *= 0.5;
            if a + k > max {
                k = max - a;
            }
            self.pitch_acc = a + k;
            out.pitch -= 2.0 * k;
        }
        if self.back < 0.0 {
            self.back = 0.0;
        }
        self.back += r.back_jitter;
        if r.pitch_threshold + m.random_pitch < self.pitch_acc.abs() {
            out.yaw += self.wag(r, tick);
        }
        out
    }

    /// The yaw kick (cshell+0x1f2f60) at tick clock value `tick`: the view yaw change.
    fn wag(&mut self, r: &RecoilRow, tick: u64) -> f32 {
        let mw = rad(r.wag_max_deg);
        // DC-DEFAULT: CLI-310 (proposed) a row whose MaxCamRecoilWag is 0 or less kicks no yaw (the 2009 client
        // divides by it)
        if mw <= 0.0 || !mw.is_finite() {
            return 0.0;
        }
        let share = 1.0 - self.wag_acc.abs() / mw;
        let side = if self.wag_right {
            r.wag_right_deg
        } else {
            r.wag_left_deg
        };
        let f = rad(side) * share;
        self.wag_acc += f * 0.5;
        self.wags += 1;
        if r.wag_threshold < self.wag_acc.abs() && tick & 1 == 1 {
            self.wag_right = !self.wag_right;
            self.flips += 1;
        }
        -f
    }

    /// The drain up to session time `now_ms`: the view change (radians). Integrated in steps of at most
    /// [`DRAIN_STEP_MS`] that end on whole multiples of it, each with its own tick for the rate test.
    pub fn drain(&mut self, r: &RecoilRow, now_ms: f64, tick_ms: f64, triple: bool) -> ViewKick {
        let mut out = ViewKick::default();
        let Some(from) = self.last_drain else {
            self.last_drain = Some(now_ms);
            return out;
        };
        self.last_drain = Some(now_ms.max(from));
        if now_ms <= from {
            return out;
        }
        let pmax = rad(pitch_pair(r, triple).0);
        let wmax = rad(r.wag_max_deg);
        let mut t = from;
        while t < now_ms && (self.pitch_acc > 0.0 || self.wag_acc != 0.0 || self.back > 0.0) {
            // steps end on the session clock's whole milliseconds, so any split of the time into frames gives the
            // same steps
            let next = ((t / DRAIN_STEP_MS).floor() + 1.0) * DRAIN_STEP_MS;
            let dt = next.min(now_ms) - t;
            t += dt;
            let dts = (dt / 1000.0) as f32;
            let since = self
                .kick_tick
                .map_or(u64::MAX, |k| tick_of(t, tick_ms).saturating_sub(k));
            let rate = if since < r.delay_ms as u64 {
                r.wait_rate
            } else {
                r.rate
            };
            if self.pitch_acc > 0.0 && pmax > 0.0 {
                let rec = (dts * rate * (self.pitch_acc / pmax + 1.0)).min(self.pitch_acc);
                self.pitch_acc -= rec * 0.5;
                out.pitch += rec;
            }
            if self.pitch_acc < 0.0 {
                self.pitch_acc = 0.0;
            }
            // DC-DEFAULT: CLI-312 (proposed) the kick-back stops at 0 (the 2009 client could step below 0 within one
            // frame and keep that until the next kick)
            if self.back > 0.0 {
                self.back = (self.back - dts * r.back_recover * 5.0).max(0.0);
            }
            let w = self.wag_acc.abs();
            if w > 0.0 && wmax > 0.0 {
                let mut rec = (dts * rate * (w / wmax + 1.0)).min(w);
                if self.wag_acc < 0.0 {
                    rec = -rec;
                }
                self.wag_acc -= rec * 0.5;
                out.yaw += rec;
            } else if wmax <= 0.0 {
                self.wag_acc = 0.0;
            }
        }
        out
    }

    /// How far the eye sits behind its place (units along the view direction).
    // EVIDENCE: cshell+0x1f9c40 (eye -= forward x back x 10)
    pub fn eye_back(&self) -> f32 {
        self.back.max(0.0) * 10.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spread::clock::TICK_MS;

    fn row() -> RecoilRow {
        RecoilRow {
            base_deg: 4.0,
            accel: 0.0,
            max_deg: 10.0,
            wag_max_deg: 15.0,
            wag_left_deg: 1.2,
            wag_right_deg: -1.2,
            pitch_threshold: 0.02,
            wag_threshold: 0.01,
            back_jitter: 0.0,
            back_recover: 1.0,
            ..RecoilRow::default()
        }
    }

    /// The mods' recoil terms (cshell+0x1f5150, cshell+0x1f8b40): the two factors come from the last filled socket
    /// only, the acceleration and the yaw threshold add up; the factors scale the first kick's base and bound.
    #[test]
    fn mod_terms_scale_the_kick_with_the_last_socket_factors() {
        use crate::zoom::ModRecoil;
        let silencer = ModRecoil {
            base: -0.1,
            random_pitch: -2.0,
            accel: 0.1,
            ..ModRecoil::default()
        };
        let magazine = ModRecoil {
            accel: 0.2,
            ..ModRecoil::default()
        };
        let m = KickMods::of_sockets([Some(&silencer), None, None]);
        assert_eq!((m.base_per, m.random_pitch), (-0.1, -2.0));
        let both = KickMods::of_sockets([Some(&silencer), None, Some(&magazine)]);
        assert_eq!(both.base_per, 0.0, "socket 2 overwrites the suppressor's factor");
        assert!((both.accel - 0.3).abs() < 1e-6);
        assert_eq!(both.random_pitch, -2.0);
        // the first kick: base 4 deg x 0.9 -> half of it in pending pitch, twice that on the view
        let r = RecoilRow {
            pitch_threshold: 0.05,
            ..row()
        };
        let mut plain = CameraRecoil::default();
        let mut modded = CameraRecoil::default();
        let k0 = plain.kick(&r, 0.0, TICK_MS, false);
        let k1 = modded.kick_with(&r, &m, 0.0, TICK_MS, false);
        assert!((k1.pitch / k0.pitch - 0.9).abs() < 1e-5, "{k0:?} {k1:?}");
        // the threshold 0.05 - 2 is below 0: the suppressed kick wags on the first shot, the plain one does not
        assert_eq!((k0.yaw, modded.wags), (0.0, 1));
        // the bound factor: MaxCamRecoilPitchPer -0.5 halves the 10 deg bound the pending pitch may reach
        let tight = KickMods {
            max_per: -0.5,
            ..KickMods::default()
        };
        let mut s = CameraRecoil::default();
        for i in 0..40 {
            s.kick_with(&r, &tight, i as f64, TICK_MS, false);
        }
        assert!(s.pitch_acc <= rad(5.0) + 1e-6 && s.pitch_acc > rad(4.0), "{}", s.pitch_acc);
    }

    #[test]
    fn recoil_kicks_up_and_recovers() {
        let r = row();
        let mut s = CameraRecoil::default();
        let kick = s.kick(&r, 0.0, TICK_MS, false);
        assert!((kick.pitch + 2.0 * rad(2.0)).abs() < 1e-6, "{kick:?}");
        let back = s.drain(&r, 1000.0, TICK_MS, false);
        assert!(back.pitch > 0.0 && back.pitch <= -kick.pitch + 1e-6);
        // 1 ms integration: two halves give the same as one call
        let mut a = CameraRecoil::default();
        let mut b = CameraRecoil::default();
        a.kick(&r, 0.0, TICK_MS, false);
        b.kick(&r, 0.0, TICK_MS, false);
        let x = a.drain(&r, 300.0, TICK_MS, false);
        let mut y = b.drain(&r, 150.0, TICK_MS, false);
        y.add(b.drain(&r, 300.0, TICK_MS, false));
        assert!((x.pitch - y.pitch).abs() < 1e-6);
    }

    #[test]
    fn the_view_offset_is_twice_the_pending_amount_and_drains_to_zero() {
        let r = row();
        let mut s = CameraRecoil::default();
        let (mut p, mut y) = (0.0f32, 0.0f32);
        for i in 0..20 {
            let k = s.kick(&r, i as f64 * 100.0, TICK_MS, false);
            p += k.pitch;
            y += k.yaw;
            assert!(
                (p + 2.0 * s.pitch_acc).abs() < 1e-5,
                "shot {i}: {p} vs {}",
                s.pitch_acc
            );
            assert!(
                (y + 2.0 * s.wag_acc).abs() < 1e-5,
                "shot {i}: {y} vs {}",
                s.wag_acc
            );
        }
        assert!(
            s.wags > 0,
            "the yaw kicks once the pitch passes MaxRandomPitch"
        );
        let d = s.drain(&r, 60_000.0, TICK_MS, false);
        p += d.pitch;
        y += d.yaw;
        assert!(p.abs() < 1e-5 && y.abs() < 1e-5, "{p} {y}");
    }

    #[test]
    fn the_yaw_waits_for_the_pitch_threshold_and_flips_on_odd_ticks() {
        let r = row();
        let mut s = CameraRecoil::default();
        // the first kick: a = rad(2) = 0.0349 > 0.02, so the wag runs on the first shot already
        let k = s.kick(&r, 0.0, TICK_MS, false);
        assert!((k.yaw + rad(1.2)).abs() < 1e-6, "{k:?}");
        // a weapon with a high threshold never wags
        let mut q = CameraRecoil::default();
        let hi = RecoilRow {
            pitch_threshold: 1.0,
            ..row()
        };
        for i in 0..10 {
            assert_eq!(q.kick(&hi, i as f64 * 90.0, TICK_MS, false).yaw, 0.0);
        }
        // flips only when |w| > MaxRandomWag and the tick value is odd: tick 0 (0 ms) is even
        let mut f = CameraRecoil::default();
        f.kick(&r, 0.0, TICK_MS, false);
        assert!(!f.wag_right, "tick value 0 is even");
        let mut g = CameraRecoil::default();
        g.kick(&r, 16.0, TICK_MS, false);
        assert!(
            g.wag_right,
            "tick value 15 is odd and |w| = rad(0.6) > 0.01"
        );
    }

    #[test]
    fn the_drain_does_not_depend_on_the_frame_rate() {
        let r = row();
        let run = |period: f64| {
            let mut s = CameraRecoil::default();
            let (mut p, mut y) = (0.0f64, 0.0f64);
            let mut t = 0.0;
            let mut next_shot = 0.0;
            while t < 3000.0 {
                if t >= next_shot && next_shot < 1500.0 {
                    let k = s.kick(&r, next_shot, TICK_MS, false);
                    p += k.pitch as f64;
                    y += k.yaw as f64;
                    next_shot += 100.0;
                }
                let d = s.drain(&r, t, TICK_MS, false);
                p += d.pitch as f64;
                y += d.yaw as f64;
                t += period;
            }
            (p, y, s.pitch_acc, s.wag_acc)
        };
        let a = run(1.0);
        for fps in [30.0, 60.0, 144.0, 1000.0 / 7.0] {
            let b = run(1000.0 / fps);
            assert!(
                (a.2 - b.2).abs() < 1e-5 && (a.3 - b.3).abs() < 1e-5,
                "{fps}: {a:?} {b:?}"
            );
        }
    }

    #[test]
    fn kick_back_rises_per_shot_and_falls() {
        let r = RecoilRow {
            back_jitter: 0.2,
            back_recover: 3.0,
            ..row()
        };
        let mut s = CameraRecoil::default();
        s.kick(&r, 0.0, TICK_MS, false);
        assert!((s.eye_back() - 2.0).abs() < 1e-6);
        s.drain(&r, 10.0, TICK_MS, false);
        assert!((s.back - (0.2 - 0.01 * 15.0)).abs() < 1e-5, "{}", s.back);
        s.drain(&r, 100.0, TICK_MS, false);
        assert_eq!(s.back, 0.0);
    }

    #[test]
    fn reset_keeps_the_side() {
        let mut s = CameraRecoil {
            pitch_acc: 0.1,
            wag_acc: 0.2,
            wag_right: true,
            back: 1.0,
            ..CameraRecoil::default()
        };
        s.reset();
        assert_eq!(
            (s.pitch_acc, s.wag_acc, s.back, s.wag_right),
            (0.0, 0.0, 0.0, true)
        );
    }
}
