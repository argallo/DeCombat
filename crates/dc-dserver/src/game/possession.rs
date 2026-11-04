//! Possession (project decision): `-possession` from the lobby. A foe kill moves the killer in a straight line
//! to the victim's place and gives him the victim's guns. The server drives the body for the move: `Player::pos` follows
//! the line each tick (the other clients' stream), the client's reports of the old `move_seq` are ignored, nothing
//! hits and nothing fires, and a teleport at the end point closes it.
//!
//! Provenance: ORIGINAL (no 2009 counterpart); the weapon messages are the Spy Hunt rearm's.
// DC-DEFAULT: DSV-483 (proposed, project decision) the Possession rules
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4 rows 0xb2 / 0xb4 / 0x7f; MOVEMENT_2009.md §12.4 (the teleport and `move_seq`)

use super::{since, Game, Life, RoundPhase, WeaponState};
use crate::gs;
use dc_net_engine::Millis;
use dc_sim::Collider;

/// Move speed in world units per second, below the cap.
pub const SPEED: f32 = 3000.0;
/// The shortest move, so a kill at arm's length still reads as a move.
pub const MIN_MS: u32 = 120;
/// project decision: the move takes at most 0.5 s at ANY distance, the speed adapts.
pub const MAX_MS: u32 = 500;
/// Weapon slots from here up are the mode's (the bomb): they do not change hands.
const MODE_SLOT: u8 = 9;

/// The move's length for a distance.
pub fn move_ms(dist: f32) -> u32 {
    if !dist.is_finite() {
        return MAX_MS;
    }
    ((dist / SPEED * 1000.0) as u32).clamp(MIN_MS, MAX_MS)
}

/// One running move.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flight {
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub start: Millis,
    pub ms: u32,
}

impl Game {
    /// Whether this match plays Possession: the flag, not in Fireteam, Spy Hunt or Quarantine, not with Gun Game.
    pub fn is_possession(&self) -> bool {
        self.cfg.possession
            && !self.is_coop()
            && !self.is_spy()
            && !self.is_infect()
            && !self.is_gun_game()
    }

    /// Whether `pid` is in a move: no damage, no fire.
    pub fn in_flight(&self, pid: u8) -> bool {
        self.players.get(&pid).is_some_and(|p| p.flight.is_some())
    }

    /// Logs a flag this room's mode ignores.
    pub(super) fn possession_apply_room(&mut self) {
        if self.cfg.possession && !self.is_possession() {
            self.log(format!(
                "possession: mode class {} does not play it: the flag is ignored",
                self.rules.mode_class
            ));
        }
    }

    /// Where a move onto the dead `victim` ends: where he stood, on the floor; his last grounded standing report when
    /// he died in the air or that spot has no room for a standing box (he crouched under something).
    fn possession_spot(&self, victim: u8, dims: [f32; 3]) -> Option<[f32; 3]> {
        let v = self.players.get(&victim)?;
        let fallback = v.stand_pos.unwrap_or(v.pos);
        let spot = if v.on_ground_report {
            self.floor_point(v.pos, dims)
        } else {
            fallback
        };
        let fits = self.trace.as_deref().is_none_or(|w| {
            !w.overlaps(
                &dc_sim::Cylinder::from_dims(dc_sim::V3::from_array(dims)),
                dc_sim::V3::from_array(spot),
            )
        });
        let spot = if fits { spot } else { self.floor_point(fallback, dims) };
        spot.iter().all(|c| c.is_finite()).then_some(spot)
    }

    /// A foe kill (the caller's test: another player, another team). A killer who is dead, already moving, in a plant
    /// or defuse hold, or a kill outside the running round moves nobody.
    pub(super) fn possession_on_kill(&mut self, killer: u8, victim: u8, now: Millis) {
        if !self.is_possession()
            || !matches!(self.phase, RoundPhase::Running { .. })
            || self.bomb.hold.is_some_and(|h| h.pid == killer)
        {
            return;
        }
        let dims = self.dims_of(killer);
        let Some(k) = self.players.get(&killer) else {
            return;
        };
        if k.life != Life::Alive || k.flight.is_some() {
            return;
        }
        let from = k.pos;
        let Some(to) = self.possession_spot(victim, dims) else {
            return;
        };
        let dist = (0..3).map(|i| (to[i] - from[i]).powi(2)).sum::<f32>().sqrt();
        let ms = move_ms(dist);
        let guns: Vec<(u8, u16, [i8; 4])> = self.players.get(&victim).map_or(Vec::new(), |v| {
            v.weapons
                .iter()
                .filter(|w| w.slot < MODE_SLOT)
                .map(|w| (w.slot, w.weapon, w.parts))
                .collect()
        });
        let Some(k) = self.players.get_mut(&killer) else {
            return;
        };
        // reports of the old sequence are ignored from here to the closing teleport
        k.move_seq = k.move_seq.wrapping_add(1);
        k.flight = Some(Flight { from, to, start: now, ms });
        k.fall = None;
        k.sprinting = None;
        self.log(format!(
            "possession: client {killer} takes client {victim}: {dist:.0} units in {ms} ms to {to:?}"
        ));
        self.possession_rearm(killer, &guns);
        let mut b = dc_wire::udp::BitWriter::new();
        dc_wire::possession::write_move(&mut b, to, ms as u16);
        self.send(killer, crate::msg::s2c_shell_msg(dc_wire::possession::SHELL_ID, &b));
    }

    /// A full copy of a weapon with its attachments (a magazine mod as at a spawn).
    fn possession_weapon(&self, slot: u8, weapon: u16, parts: [i8; 4]) -> WeaponState {
        let mut w = WeaponState { slot, parts, ..self.new_weapon(weapon, None) };
        let (clip_plus, max_plus) = self.tables.weapons.magazine_bonus(weapon, &parts);
        if w.clip_size > 0 && !w.own_ammo {
            let size = (i64::from(w.clip_size) + i64::from(clip_plus)).max(1);
            let size = u32::try_from(size).unwrap_or(u32::MAX);
            w.stock = w.stock.saturating_add(w.clip.saturating_sub(size));
            w.clip = size;
            w.clip_size = size;
            w.stock = u32::try_from((i64::from(w.stock) + i64::from(max_plus)).max(0))
                .unwrap_or(u32::MAX);
        }
        w
    }

    /// The victim's guns in the same life: the killer's leave (0xb4, no pickup), the new ones come full (0xb2), the
    /// first is drawn (0x7f); the mode's items stay. A victim without guns leaves the killer his own.
    fn possession_rearm(&mut self, pid: u8, guns: &[(u8, u16, [i8; 4])]) {
        if guns.is_empty() {
            return;
        }
        let mut states: Vec<WeaponState> = guns
            .iter()
            .map(|&(slot, w, parts)| self.possession_weapon(slot, w, parts))
            .collect();
        let new_count = states.len();
        let Some(p) = self.players.get(&pid) else {
            return;
        };
        let (old, kept): (Vec<WeaponState>, Vec<WeaponState>) =
            p.weapons.iter().copied().partition(|w| w.slot < MODE_SLOT);
        states.extend(kept);
        // the weapon's position in its class list, as at a spawn
        let class_pos: Vec<u8> = states
            .iter()
            .enumerate()
            .map(|(i, w)| {
                self.tables.weapons.class_of(w.weapon).map_or(0, |c| {
                    states[..i]
                        .iter()
                        .filter(|x| self.tables.weapons.class_of(x.weapon) == Some(c))
                        .count()
                        .min(32) as u8
                })
            })
            .collect();
        if let Some(p) = self.players.get_mut(&pid) {
            p.weapons = states.clone();
            p.current = Some(0);
        }
        for w in &old {
            self.send(pid, gs::weapon_remove(w.slot, w.weapon, w.ammo));
        }
        let spec = |i: usize, w: &WeaponState| gs::WeaponSpec {
            weapon: w.weapon,
            ammo_kind: w.ammo,
            ammo: w.told_amount(),
            clip: w.clip,
            class_pos: class_pos[i],
            slot: w.slot,
            parts: w.parts,
        };
        for (i, w) in states.iter().enumerate().take(new_count) {
            self.send(pid, gs::weapon_give(&spec(i, w)));
        }
        self.slot_status(pid, &states[..new_count]);
        let first = states[0];
        match self.tables.weapons.command_of(first.weapon) {
            Some(c) => self.send(pid, gs::weapon_switch(c, true, -1.0, 0)),
            None => self.send(pid, gs::drawn_weapon(&spec(0, &first))),
        }
        self.hand_weapon_changed(pid, first.weapon);
    }

    /// The running moves: the body along the line (constant speed), the closing teleport at the end.
    pub(super) fn tick_possession(&mut self, now: Millis) {
        let flying: Vec<(u8, Flight)> = self
            .players
            .values()
            .filter_map(|p| p.flight.map(|f| (p.pid, f)))
            .collect();
        for (pid, f) in flying {
            let elapsed = since(now, f.start);
            let dims = self.dims_of(pid);
            let Some(p) = self.players.get_mut(&pid) else {
                continue;
            };
            p.last_move = Some(now);
            if elapsed < f.ms {
                let t = elapsed as f32 / f.ms as f32;
                let mut pos = [0.0; 3];
                for i in 0..3 {
                    pos[i] = f.from[i] + (f.to[i] - f.from[i]) * t;
                    p.vel[i] = (f.to[i] - f.from[i]) / (f.ms as f32 / 1000.0);
                }
                p.pos = pos;
                p.track.push(now, pos);
                continue;
            }
            p.flight = None;
            p.move_seq = p.move_seq.wrapping_add(1);
            p.pos = f.to;
            p.vel = [0.0; 3];
            p.track.restart(now, f.to);
            p.stand_pos = Some(f.to);
            p.on_ground_report = true;
            let seq = p.move_seq;
            self.send(pid, gs::mover_teleport(seq, f.to, dims, None));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// project decision: distance / speed, hard cap 0.5 s at any distance.
    #[test]
    fn the_move_time_follows_the_distance_under_a_hard_cap() {
        assert_eq!(move_ms(0.0), MIN_MS);
        assert_eq!(move_ms(900.0), 300);
        assert_eq!(move_ms(1500.0), 500);
        assert_eq!(move_ms(1_000_000.0), MAX_MS);
        assert_eq!(move_ms(f32::INFINITY), MAX_MS);
    }
}
