//! Gun Game (project decision): One Man Army's match (mode class 6: spawns, respawn, scoring screens, the
//! FFA count and its tie rule) under a gun ladder the lobby draws and hands over with `-gungame <seed>:<w,w,...>`.
//!
//! The FFA kill count of a player IS his rung ([`Game::ffa_kills_of`]): it drives the goal test, the time-limit leader
//! and the 0x6b counters every client's scoreboard shows. The account statistics keep the real kills.
//!
//! Provenance: ORIGINAL (no 2009 counterpart); the weapon messages are the Spy Hunt rearm's.
// DC-DEFAULT: DSV-481 (proposed, project decision) the Gun Game rules
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4 rows 0xb2 / 0xb4 / 0x7f, §4.3 (0x66 kind 1 sets the slot's ammunition count)

use super::Game;
use crate::gs;
use dc_net_engine::Millis;

/// What the lobby gave the match.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GunGameSetup {
    /// The seed the lobby drew the ladder from (logged; the lobby's pool and the seed reproduce the ladder).
    pub seed: u64,
    /// The weapon rows, rung 0 first.
    pub ladder: Vec<u16>,
}

/// The value of `-gungame`: `<seed>:<weapon>,<weapon>,...`.
pub fn parse_arg(v: &str) -> Result<GunGameSetup, String> {
    let (seed, list) = v
        .split_once(':')
        .ok_or_else(|| format!("-gungame: {v:?} is not seed:w,w,..."))?;
    let seed = seed
        .trim()
        .parse::<u64>()
        .map_err(|_| format!("-gungame: bad seed {seed:?}"))?;
    let mut ladder = Vec::new();
    for w in list.split(',').filter(|x| !x.trim().is_empty()) {
        ladder.push(
            w.trim()
                .parse::<u16>()
                .map_err(|_| format!("-gungame: bad weapon {w:?}"))?,
        );
    }
    if ladder.is_empty() {
        return Err("-gungame: an empty ladder".into());
    }
    Ok(GunGameSetup { seed, ladder })
}

/// The melee weapon type of the item table (the `slot_class` of `dc_bute::arms`).
const CLASS_PRIMARY: u8 = 0;
const CLASS_SECONDARY: u8 = 1;
const CLASS_MELEE: u8 = 2;
/// The knife's weapon slot.
const KNIFE_SLOT: u8 = 2;

impl Game {
    /// Whether this match plays the ladder: the flag on a free-for-all room.
    pub fn is_gun_game(&self) -> bool {
        self.cfg.gun_game.is_some() && self.is_ffa()
    }

    /// The ladder's weapon of rung `r` (the last one past the end).
    fn rung_weapon(&self, r: u32) -> Option<u16> {
        let l = &self.cfg.gun_game.as_ref()?.ladder;
        l.get(r as usize).or(l.last()).copied()
    }

    /// Fits the ladder to the ROM and the room once the room line is known: rows the weapon table lacks leave the
    /// ladder, the goal becomes its length, the room's weapon limit does not apply.
    pub(super) fn gun_game_apply_room(&mut self) {
        if self.cfg.gun_game.is_none() {
            return;
        }
        if !self.is_ffa() {
            self.log(format!(
                "gungame: mode class {} is not free-for-all: the flag is ignored",
                self.rules.mode_class
            ));
            return;
        }
        if !self.tables.weapons.weapons.is_empty() {
            let known: Vec<u16> = self.cfg.gun_game.as_ref().map_or(Vec::new(), |g| {
                g.ladder
                    .iter()
                    .copied()
                    .filter(|w| self.tables.weapons.weapons.contains_key(w))
                    .collect()
            });
            if let Some(g) = self.cfg.gun_game.as_mut() {
                g.ladder = known;
            }
        }
        let (seed, ladder) = self
            .cfg
            .gun_game
            .as_ref()
            .map_or((0, Vec::new()), |g| (g.seed, g.ladder.clone()));
        self.rules.goal = ladder.len() as u32;
        self.rules.arms = 0;
        self.log(format!(
            "gungame: seed {seed} ladder {ladder:?} goal {}",
            self.rules.goal
        ));
    }

    /// The default knife: the ROM's issue row of the melee class, else the lowest melee row of the item table.
    fn gun_game_knife(&self) -> Option<u16> {
        let melee = |w: &u16| {
            self.tables
                .items
                .weapon_types
                .get(w)
                .and_then(|t| dc_bute::arms::slot_class(*t))
                == Some(CLASS_MELEE)
        };
        self.tables
            .items
            .issue_weapons
            .iter()
            .flatten()
            .copied()
            .find(|w| melee(w))
            .or_else(|| {
                let mut all: Vec<u16> = self
                    .tables
                    .items
                    .weapon_types
                    .keys()
                    .copied()
                    .filter(|w| melee(w))
                    .collect();
                all.sort_unstable();
                all.first().copied()
            })
    }

    /// What a player holds at rung `r`: the rung's gun (primary slot 0, secondary slot 1) drawn first, then the knife.
    fn gun_game_kit(&self, r: u32) -> Vec<(u8, u16)> {
        let mut out = Vec::new();
        if let Some(w) = self.rung_weapon(r) {
            let class = self
                .tables
                .items
                .weapon_types
                .get(&w)
                .and_then(|t| dc_bute::arms::slot_class(*t));
            let slot = if class == Some(CLASS_SECONDARY) {
                CLASS_SECONDARY
            } else {
                CLASS_PRIMARY
            };
            out.push((slot, w));
        }
        if let Some(k) = self.gun_game_knife() {
            out.push((KNIFE_SLOT, k));
        }
        out
    }

    /// The spawn set of `pid` in Gun Game; `None` in other matches.
    pub(super) fn gun_game_spawn_kit(&self, pid: u8) -> Option<Vec<(u8, u16)>> {
        self.is_gun_game()
            .then(|| self.gun_game_kit(self.ffa_kills_of(pid)))
    }

    /// Whether `w` is a melee weapon.
    fn is_melee(&self, w: u16) -> bool {
        self.tables
            .items
            .weapon_types
            .get(&w)
            .and_then(|t| dc_bute::arms::slot_class(*t))
            == Some(CLASS_MELEE)
    }

    /// A foe kill: a kill with the current rung's gun moves the killer up one rung and gives him the next gun in the
    /// same life (the last gun's kill reaches the goal, the caller's goal test ends the match); a knife kill moves
    /// the victim down one rung (floor 0), the knifer stays. Any other weapon (a round still flying from the gun
    /// before) moves nobody.
    // DC-DEFAULT: DSV-481 (proposed) knife: the victim down one, the knifer not up; the user may change it
    pub(super) fn gun_game_on_kill(&mut self, killer: u8, victim: u8, weapon: u16, now: Millis) {
        let rung = self.ffa_kills_of(killer);
        if self.is_melee(weapon) {
            let v = self.ffa_kills_of(victim);
            if v > 0 {
                self.modes.ffa_kills.insert(victim, v - 1);
            }
            self.log(format!(
                "gungame: client {killer} knifed client {victim}: rung {v} -> {}",
                v.saturating_sub(1)
            ));
            return;
        }
        if self.rung_weapon(rung) != Some(weapon) {
            self.log(format!(
                "gungame: client {killer} kill with weapon {weapon} on rung {rung}: no promotion"
            ));
            return;
        }
        let next = rung + 1;
        self.modes.ffa_kills.insert(killer, next);
        self.log(format!(
            "gungame: client {killer} rung {rung} -> {next} of {}",
            self.rules.goal
        ));
        if next < self.rules.goal {
            self.gun_game_rearm(killer, now);
        }
    }

    /// The new rung's kit in the same life: the old weapons leave (0xb4), the new ones come full (0xb2) with their
    /// default attachments, and the gun is drawn at once (0x7f). A dead killer (his own round or a trade) gets it at
    /// the respawn.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.4 rows 0xb2 / 0xb4 / 0x7f
    fn gun_game_rearm(&mut self, pid: u8, _now: Millis) {
        if !self
            .players
            .get(&pid)
            .is_some_and(|p| matches!(p.life, super::Life::Alive))
        {
            return;
        }
        let kit = self.gun_game_kit(self.ffa_kills_of(pid));
        let parts_on = self.cfg.weapon_parts;
        let states: Vec<super::WeaponState> = kit
            .iter()
            .map(|&(slot, w)| super::WeaponState {
                slot,
                parts: if parts_on {
                    self.tables.weapons.default_parts(w)
                } else {
                    gs::NO_PARTS
                },
                ..self.new_weapon(w, None)
            })
            .collect();
        let Some(p) = self.players.get_mut(&pid) else {
            return;
        };
        let old = std::mem::replace(&mut p.weapons, states.clone());
        p.current = (!states.is_empty()).then_some(0);
        for w in &old {
            self.send(pid, gs::weapon_remove(w.slot, w.weapon, w.ammo));
        }
        for w in &states {
            self.send(
                pid,
                gs::weapon_give(&gs::WeaponSpec {
                    weapon: w.weapon,
                    ammo_kind: w.ammo,
                    ammo: w.told_amount(),
                    clip: w.clip,
                    class_pos: 0,
                    slot: w.slot,
                    parts: w.parts,
                }),
            );
        }
        if let Some(w) = states.first() {
            match self.tables.weapons.command_of(w.weapon) {
                Some(c) => self.send(pid, gs::weapon_switch(c, true, -1.0, 0)),
                None => {
                    let spec = gs::WeaponSpec {
                        weapon: w.weapon,
                        ammo_kind: w.ammo,
                        ammo: w.told_amount(),
                        clip: w.clip,
                        class_pos: 0,
                        slot: w.slot,
                        parts: w.parts,
                    };
                    self.send(pid, gs::drawn_weapon(&spec));
                }
            }
            self.hand_weapon_changed(pid, w.weapon);
        }
    }

    /// A gun that ran dry gets its reserve again (0x66 kind 1): missing every shot must not end a player's match at
    /// the knife. A promotion and a respawn bring a full gun anyway.
    // DC-DEFAULT: DSV-481 (proposed) refill at clip 0 and stock 0
    pub(super) fn gun_game_refill(&mut self, pid: u8, weapon: u16) {
        if !self.is_gun_game() || self.is_melee(weapon) {
            return;
        }
        let granted = self.new_weapon(weapon, None).stock;
        let Some(w) = self
            .players
            .get_mut(&pid)
            .and_then(|p| p.weapons.iter_mut().find(|w| w.weapon == weapon))
        else {
            return;
        };
        if w.clip_size == 0 || w.clip > 0 || w.stock > 0 || w.shot_open {
            return;
        }
        w.stock = granted.max(w.clip_size);
        let m = gs::player_ammo(
            w.weapon,
            w.ammo,
            w.told_amount(),
            w.slot,
            super::arrival::TODO_SLOT_STATUS_EXTRA,
        );
        self.send(pid, m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_parses_and_refuses_junk() {
        assert_eq!(
            parse_arg("9:4, 12,7"),
            Ok(GunGameSetup {
                seed: 9,
                ladder: vec![4, 12, 7]
            })
        );
        assert!(parse_arg("9").is_err());
        assert!(parse_arg("x:1").is_err());
        assert!(parse_arg("9:").is_err());
        assert!(parse_arg("9:a").is_err());
    }
}
