//! Fall damage (DSV-76): the server follows each living player's accepted 0x64 trailers, keeps the highest body
//! height since the last grounded trailer and hurts the player when a landing ends a fall of more than the lower
//! height limit.
// SPEC: MOVEMENT_2009.md §10.2 (the 2009 mover keeps the fall start as the highest y while not on ground; the
// surface is forced to 200 on a ladder), §10.4 (the four fall tunables have no client reader: the server applies
// the damage), §12.2 (trailer fields `on_ground`, `surface`); GAME_MESSAGES_2009_C2S.md §2.1
// DC-DEFAULT: DSV-76 (proposed)

use super::combat::DamageSource;
use super::{Game, Life, RoundPhase};
use dc_net_engine::Millis;

/// Trailer `surface` of a body on a ladder: a fall starts again at every ladder trailer.
// SPEC: MOVEMENT_2009.md §10.2 (forced to 200 on a ladder), §12.2 (200 on the ladder, PROVEN-CAPTURE tdm1);
// GAME_MESSAGES_2009_C2S.md §2.1 (69 ladder updates)
pub const LADDER_SURFACE: u8 = 200;

/// `zone` (and the unread byte before `mode`) of a fall's `GS_DAMAGE_TAKEN`.
// OBSERVED: capture 20261025_000014_tdm1 c5 seq 6660 / 7933 and 20261025_002236_tdm4 c6 seq
// 6695 (the three falls of the reference-stack captures: `0d74 <from_dir> 05 01 ffff 00 05 00 <damage>`; REFERENCE,
// accepted by the real client)
pub const FALL_DAMAGE_TYPE: u8 = 5;
/// `dmg_kind` of a fall's `GS_DAMAGE_TAKEN` (weapon hits carry 3).
// OBSERVED: the same three messages (`dmg_kind` 0 in all of them; REFERENCE)
pub const FALL_DAMAGE_KIND: u8 = 0;
/// Kill feed weapon of a lethal fall when the victim holds none.
// DC-DEFAULT: DSV-76 (proposed), as DSV-52
pub const FALL_DEATH_WEAPON: u16 = 0;

/// The fall rule values (`-combat fall=on|off,fall_h_lo=..,fall_h_hi=..,fall_d_lo=..,fall_d_hi=..`).
// EVIDENCE: cshell+0x1fdcb0 (the 2009 mover registers four fall-damage variables with the defaults 300.0, 700.0,
// 0.0, 200.0; MOVEMENT_2009.md §2, §10.4)
// OBSERVED: the three captured falls (h 301.877 / 342.414 / 303.999 -> health loss 0.93866 / 21.2074 / 1.99994) fit
// 0.5 x (h - 300) = the linear map (300, 0) -> (700, 200) with a residual below 0.001; every captured landing of less
// than 300 units got no damage (largest 265.5, tdm1 t 217.890)
// DC-DEFAULT: DSV-76 (proposed) the rule and its values
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FallTuning {
    pub enabled: bool,
    /// Falls of this height or less do no damage.
    pub h_lo: f32,
    /// Height of the largest damage (higher falls do no more).
    pub h_hi: f32,
    /// Damage just above `h_lo`.
    pub d_lo: f32,
    /// Damage at `h_hi` and above.
    pub d_hi: f32,
}

impl Default for FallTuning {
    fn default() -> Self {
        FallTuning {
            enabled: true,
            h_lo: 300.0,
            h_hi: 700.0,
            d_lo: 0.0,
            d_hi: 200.0,
        }
    }
}

/// The `combat.*` keys of the fall rule (handled before the other combat keys).
// DC-DEFAULT: DSV-76 (proposed)
pub const FALL_KEYS: [&str; 5] = ["fall", "fall_h_lo", "fall_h_hi", "fall_d_lo", "fall_d_hi"];

impl FallTuning {
    /// The damage of a fall of height `h`: none up to `h_lo`, then linear from `d_lo` to `d_hi` at `h_hi`, no more
    /// above it (the cap above 700 units is PLAUSIBLE: no capture has such a fall).
    // DC-DEFAULT: DSV-76 (proposed)
    pub fn damage(&self, h: f32) -> Option<f32> {
        if !self.enabled || !h.is_finite() || h <= self.h_lo {
            return None;
        }
        let span = self.h_hi - self.h_lo;
        let f = if span > 0.0 {
            ((h - self.h_lo) / span).min(1.0)
        } else {
            1.0
        };
        let d = self.d_lo + f * (self.d_hi - self.d_lo);
        (d.is_finite() && d > 0.0).then_some(d)
    }

    /// Sets one fall key; `None` when `key` is not a fall key.
    pub fn set(&mut self, key: &str, value: &str) -> Option<Result<(), String>> {
        if !FALL_KEYS.contains(&key) {
            return None;
        }
        let bad = || format!("combat.{key}: bad value {value:?}");
        if key == "fall" {
            return Some(if value.eq_ignore_ascii_case("on") {
                self.enabled = true;
                Ok(())
            } else if value.eq_ignore_ascii_case("off") {
                self.enabled = false;
                Ok(())
            } else {
                Err(bad())
            });
        }
        let Some(x) = value
            .parse::<f32>()
            .ok()
            .filter(|x| x.is_finite() && *x >= 0.0)
        else {
            return Some(Err(bad()));
        };
        match key {
            "fall_h_lo" => self.h_lo = x,
            "fall_h_hi" => self.h_hi = x,
            "fall_d_lo" => self.d_lo = x,
            _ => self.d_hi = x,
        }
        Some(Ok(()))
    }
}

/// A player's fall in progress: the highest height since the last grounded (or ladder) trailer of the teleport
/// sequence `seq`, and whether a trailer since then was airborne.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FallTrack {
    pub seq: u8,
    pub top: f32,
    pub airborne: bool,
}

/// Feeds one accepted trailer (teleport sequence `seq`, height `y`) into the fall track and returns the height of a
/// fall that this trailer ends (the first grounded trailer after an airborne one). A trailer of another teleport
/// sequence (a spawn, a teleport back) starts a new track; a ladder trailer restarts the top at its own height.
// SPEC: MOVEMENT_2009.md §10.2 (fall start = max y while not on ground; ladder surface 200), §12.4 (`move_seq`)
// DC-DEFAULT: DSV-76 (proposed) the top includes the last grounded trailer's height; the ladder restart is ours (no
// capture has a ladder descent)
pub fn track(
    slot: &mut Option<FallTrack>,
    seq: u8,
    y: f32,
    on_ground: bool,
    surface: u8,
) -> Option<f32> {
    if slot.is_some_and(|t| t.seq != seq) {
        *slot = None;
    }
    if surface == LADDER_SURFACE {
        *slot = Some(FallTrack {
            seq,
            top: y,
            airborne: false,
        });
        return None;
    }
    if on_ground {
        let fell = slot.filter(|t| t.airborne).map(|t| t.top - y);
        *slot = Some(FallTrack {
            seq,
            top: y,
            airborne: false,
        });
        return fell;
    }
    let top = slot.map_or(y, |t| t.top.max(y));
    *slot = Some(FallTrack {
        seq,
        top,
        airborne: true,
    });
    None
}

impl Game {
    /// A landing after a fall of `h` units: the DSV-76 damage through the DSV-30 path with damage type 5 (no armour
    /// slot in the gear rows: health only), no attacker, `from_dir` up; a lethal fall dies as a kill-volume death
    /// (DSV-52: no killer, the drawn weapon in the kill feed). Nothing while the round is not running.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (fall damage: 0x74 + 0x66 kind 2 after the landing, `zone` 5)
    // OBSERVED: capture 20261025_002236_tdm4 c6 seq 6695 (`from_dir` (0, 0.0199994, 0) = up x
    // the health loss / 100, armour unchanged, no 0x66 kind 3; REFERENCE)
    // DC-DEFAULT: DSV-76 (proposed)
    pub(super) fn fall_damage(&mut self, pid: u8, h: f32, now: Millis) {
        if !matches!(self.phase, RoundPhase::Running { .. }) {
            return;
        }
        let Some(p) = self.players.get(&pid) else {
            return;
        };
        if p.life != Life::Alive {
            return;
        }
        let Some(d) = self.cfg.fall.damage(h) else {
            return;
        };
        let weapon = p.hand_weapon.unwrap_or(FALL_DEATH_WEAPON);
        self.log(format!("client {pid}: fell {h:.1} units, damage {d:.2}"));
        let src = DamageSource {
            weapon,
            dtype: FALL_DAMAGE_TYPE,
        };
        // the landing's voice is chosen inside the damage call, once it is known whether the fall kills: a hurting
        // landing plays the fall-hurt file before the 0x74, a lethal one the fall-death sound
        // OBSERVED: tdm4 c6 seq 6695, tdm1 c5 seq 6660 (the fall-hurt file in the tick of the landing; REFERENCE)
        // DC-DEFAULT: DSV-596 (proposed), DSV-152 (proposed)
        self.plunge = Some((pid, true));
        self.apply_hit_as(
            pid,
            None,
            d,
            d,
            [0.0, 1.0, 0.0],
            src,
            super::TODO_KILL_ZONE_BODY,
            FALL_DAMAGE_KIND,
            now,
        );
        self.plunge = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(seq: u8, samples: &[(f32, bool, u8)]) -> Vec<f32> {
        let mut slot = None;
        samples
            .iter()
            .filter_map(|&(y, g, s)| track(&mut slot, seq, y, g, s))
            .collect()
    }

    /// The rule reproduces the three captured falls within 1e-3 and spares the largest captured harmless one.
    #[test]
    fn rule_fits_the_captured_falls() {
        let t = FallTuning::default();
        // (top, landing, health loss) of tdm1 c5 seq 6660, tdm1 c5 seq 7933, tdm4 c6 seq 6695
        for (top, land, loss) in [
            (1124.579f32, 822.702f32, 0.93866f32),
            (716.441, 374.027, 21.2074),
            (1122.735, 818.736, 1.99994),
        ] {
            let d = t.damage(top - land).expect("damage");
            assert!((d - loss).abs() < 1e-2, "{top} -> {land}: {d} vs {loss}");
        }
        assert_eq!(t.damage(265.531), None);
        assert_eq!(t.damage(300.0), None);
        assert_eq!(t.damage(800.0), Some(200.0));
        assert_eq!(t.damage(f32::NAN), None);
        let off = FallTuning {
            enabled: false,
            ..t
        };
        assert_eq!(off.damage(500.0), None);
    }

    /// Track: the top is the highest airborne height (the last grounded height included), the landing ends it.
    #[test]
    fn track_jump_and_walk_off() {
        // a jump off a ledge (tdm1 seq 6660 shape): grounded 1077.203, apex 1124.579, landing 822.702
        let falls = run(
            1,
            &[
                (1077.203, true, 71),
                (1100.0, false, 0),
                (1124.579, false, 0),
                (1000.0, false, 0),
                (822.702, true, 71),
            ],
        );
        assert_eq!(falls.len(), 1);
        assert!((falls[0] - 301.877).abs() < 1e-3);
        // walking off: the grounded height is the top
        let falls = run(
            1,
            &[(716.441, true, 10), (700.0, false, 0), (374.027, true, 10)],
        );
        assert!((falls[0] - 342.414).abs() < 1e-3);
        // standing still: no fall
        assert!(run(1, &[(5.0, true, 10), (5.0, true, 10)]).is_empty());
    }

    /// A ladder descent of 400 units ends with a short drop: only that drop counts.
    #[test]
    fn ladder_restarts_the_top() {
        let falls = run(
            2,
            &[
                (900.0, true, 71),
                (880.0, false, LADDER_SURFACE),
                (700.0, false, LADDER_SURFACE),
                (520.0, false, LADDER_SURFACE),
                (510.0, false, 0),
                (500.0, true, 71),
            ],
        );
        assert_eq!(falls.len(), 1);
        assert!((falls[0] - 20.0).abs() < 1e-3, "{falls:?}");
    }

    /// A new teleport sequence (spawn, teleport back) drops the top.
    #[test]
    fn teleport_drops_the_top() {
        let mut slot = None;
        assert_eq!(track(&mut slot, 1, 1200.0, false, 0), None);
        // spawned (seq 2) far below while the old fall was open: the first grounded trailer ends nothing
        assert_eq!(track(&mut slot, 2, 100.0, true, 71), None);
        assert_eq!(slot.unwrap().top, 100.0);
    }

    #[test]
    fn fall_keys() {
        let mut t = FallTuning::default();
        assert_eq!(t.set("fall", "off"), Some(Ok(())));
        assert!(!t.enabled);
        assert_eq!(t.set("fall_h_lo", "250"), Some(Ok(())));
        assert_eq!(t.h_lo, 250.0);
        assert!(t.set("fall_d_hi", "-1").unwrap().is_err());
        assert!(t.set("fall", "maybe").unwrap().is_err());
        assert_eq!(t.set("head_factor", "1"), None);
    }
}
