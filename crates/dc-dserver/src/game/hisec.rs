//! The in-match HiSec case (HS-1..HS-5, DeCombat rules): a kill by a player may drop one case at the victim's place;
//! the killer has it to himself for a while (the other clients hide it), then everyone may take it; a living player
//! who touches it picks it up, every client is told, the object is removed and the lobby gets a `@hisec` line. A case
//! nobody takes is removed after its lifetime.
//!
//! The wire forms are the ones the real 2009 client accepted from the reference stack (create type 0x17 kind 5, 0xd6
//! sub 0 / 1 / 2, the 0xa6 countdown to the case object); when and how often a case drops, the reservation, the
//! lifetime and the pickup radius are ours (the client cannot show them, HISEC_2009 Q-2).
// SPEC: HISEC_2009.md §3.1, §3.2, §3.3, §6.1; GAME_MESSAGES_2009_S2C.md §3.5 row 0xd6, §8.2 row 0x17, §8.3
// OBSERVED: capture 20261025_065016_tdm6 c6 (six drops: seq 3006 / 3015 drop, 3034.. countdown,
// 4833 release; seq 10688 / 10691 pickup and removal; seq 11054 release when the reserving player died);
// 20261025_002236_tdm4 c6 seq 3124 / 3132 (drop), 3812 / 3815 (the real client walked to the case: pickup, removal);
// 20261025_013028_botduel4 c5 seq 6133 (pickup by the reserving killer); REFERENCE, accepted by the real client
// DC-DEFAULT: HS-1..HS-5 (project decision for the rate: about one case per match)

use super::{len3, sub3, Game, GameOut, Life, RoundPhase};
use crate::gs;
use dc_net_engine::time::later;
use dc_net_engine::Millis;

/// Mode class of the TDM rows (the room goal is the expected number of kills there) and of the clan TDM rows (no
/// drops in clan matches).
// SPEC: GAME_MODES_2009.md §2 (Mode0 class 1 `TDM`, ClanMode0 class 4 `TDM_CLAN`; OBSERVED ROM `GameType`)
pub const MODE_CLASS_TDM: u8 = 1;
pub const MODE_CLASS_CLAN_TDM: u8 = 4;
/// Height of the case above the floor below the victim (when the trace world finds one within [`CASE_FLOOR_SEARCH`]).
// OBSERVED: capture 20261025_065016_tdm6 c6 seq 6603 and 10134 (the reference stack's cases 20.1 units
// above the floor below them; a third, seq 3015, floated 100 units up, which DeCombat does not copy)
// DC-DEFAULT: HS-2 (amendment, proposed)
pub const CASE_FLOOR_LIFT: f32 = 20.0;
pub const CASE_FLOOR_SEARCH: f32 = 512.0;

/// The `hisec.*` rule values (`-hisec key=value[,key=value...]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HisecTuning {
    /// Cases drop at all (`hisec.enabled`).
    // DC-DEFAULT: HS-1 (project decision) on once the case path exists
    pub enabled: bool,
    /// Kills per match the drop chance is spread over outside TDM (`p = 1 / expected kills`; TDM uses the room goal).
    // DC-DEFAULT: HS-1 (project decision) 20
    pub expected_kills: u32,
    /// Cases in the world at once (`hisec.max_active`).
    // DC-DEFAULT: HS-1 (project decision) 1
    pub max_active: u32,
    /// Players in the match (introduced) needed for a drop (`hisec.min_players`).
    // DC-DEFAULT: HS-1 (project decision) 2 for the test phase
    pub min_players: u32,
    /// The killer's reservation (`hisec.reserve_ms`, 0 = none).
    // DC-DEFAULT: HS-3 (proposed) 10 s
    pub reserve_ms: u32,
    /// Lifetime of a case from its drop (`hisec.lifetime_ms`, 0 = until the match ends).
    // DC-DEFAULT: HS-4 (proposed) 30 s
    pub lifetime_ms: u32,
    /// Pickup radius around the case: a player closer than this takes it (`hisec.pickup_radius`, 0 = nobody).
    // DC-DEFAULT: HS-5 (proposed) 64 units
    pub pickup_radius: f32,
    /// The `[Hisec<n>]` row of the ROM case table whose model the case shows (`hisec.case_row`) when
    /// [`HisecTuning::case_rows`] is empty.
    // DC-DEFAULT: HS-2 (amendment, proposed) row 0
    pub case_row: u32,
    /// Which case drops: `(row, weight)` pairs, one drawn per drop by weight among the rows the ROM case table has
    /// (`hisec.case_rows=0:80/1:20`; empty = `case_row` only). The EU ROM: row 0 the Gray HiSec Case (its key
    /// sells for GP), row 1 the Black HiSec Case (its key sells for NX, the better pool), row 2 the Christmas case.
    // DC-DEFAULT: HS-15 (proposed, playtest report "only the gray cases drop in game") gray 80 %, black 20 %
    pub case_rows: [(u32, u32); CASE_ROWS_MAX],
    /// Test flag: every kill that may drop a case drops one (`hisec.force=on`).
    // DC-DEFAULT: HS-1 (test flag, proposed) off
    pub force: bool,
}

impl Default for HisecTuning {
    fn default() -> Self {
        HisecTuning {
            enabled: true,
            expected_kills: 20,
            max_active: 1,
            min_players: 2,
            reserve_ms: 10_000,
            lifetime_ms: 30_000,
            pickup_radius: 64.0,
            case_row: 0,
            case_rows: [(0, 80), (1, 20), (0, 0), (0, 0)],
            force: false,
        }
    }
}

/// The `hisec.*` keys.
/// Weighted case rows the tuning can hold (HS-15); a pair with weight 0 is unused.
pub const CASE_ROWS_MAX: usize = 4;

pub const HISEC_KEYS: [&str; 10] = [
    "enabled",
    "expected_kills",
    "max_active",
    "min_players",
    "reserve_ms",
    "lifetime_ms",
    "pickup_radius",
    "case_row",
    "case_rows",
    "force",
];

impl HisecTuning {
    /// Sets one `hisec.*` key (the prefix is optional).
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let k = key.trim();
        let k = k.strip_prefix("hisec.").unwrap_or(k);
        let v = value.trim();
        let bad = || format!("hisec.{k}: bad value {v:?}");
        let int = || v.parse::<u32>().map_err(|_| bad());
        let on = || match v.to_ascii_lowercase().as_str() {
            "on" | "1" | "true" => Ok(true),
            "off" | "0" | "false" => Ok(false),
            _ => Err(bad()),
        };
        match k {
            "enabled" => self.enabled = on()?,
            "expected_kills" => self.expected_kills = int()?.max(1),
            "max_active" => self.max_active = int()?,
            "min_players" => self.min_players = int()?,
            "reserve_ms" => self.reserve_ms = int()?.min(3_600_000),
            "lifetime_ms" => self.lifetime_ms = int()?.min(3_600_000),
            "pickup_radius" => {
                self.pickup_radius = v
                    .parse::<f32>()
                    .ok()
                    .filter(|x| x.is_finite() && *x >= 0.0)
                    .ok_or_else(bad)?
            }
            "case_row" => self.case_row = int()?,
            // `row:weight` pairs separated by `/` (the `-hisec` list itself is comma-separated); `none` = empty
            "case_rows" => {
                let mut rows = [(0u32, 0u32); CASE_ROWS_MAX];
                if !v.eq_ignore_ascii_case("none") {
                    let pairs: Vec<(u32, u32)> = v
                        .split('/')
                        .map(|pair| {
                            let (r, w) = pair.split_once(':').ok_or_else(bad)?;
                            let r = r.trim().parse::<u32>().map_err(|_| bad())?;
                            let w = w.trim().parse::<u32>().map_err(|_| bad())?;
                            Ok((r, w))
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    if pairs.len() > CASE_ROWS_MAX || pairs.iter().all(|&(_, w)| w == 0) {
                        return Err(bad());
                    }
                    rows[..pairs.len()].copy_from_slice(&pairs);
                }
                self.case_rows = rows;
            }
            "force" => self.force = on()?,
            _ => {
                return Err(format!(
                    "unknown key hisec.{k} (known: {})",
                    HISEC_KEYS.join(", ")
                ))
            }
        }
        Ok(())
    }
}

/// The model of the case object (ROM `[Hisec<n>]` row).
#[derive(Debug, Clone, PartialEq)]
pub struct CaseLook {
    pub model: String,
    pub textures: Vec<String>,
    pub styles: Vec<String>,
    pub scale: [f32; 3],
}

/// A case the session creates on every client in the world.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseOut {
    /// The case key (0xd6 and the blob address the case by it; the session maps it to an object id).
    pub key: u32,
    pub look: CaseLook,
    pub pos: [f32; 3],
    pub yaw: f32,
}

/// One case in the world.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Case {
    pub key: u32,
    /// The ROM case table row it shows and delivers as (HS-15).
    pub row: u32,
    pub pos: [f32; 3],
    /// Reserved for this player until `reserved_until`.
    pub owner: Option<u8>,
    pub reserved_until: Millis,
    pub expires: Option<Millis>,
    /// Last countdown value sent.
    pub shown: u32,
}

fn whole_secs_left(now: Millis, until: Millis) -> u32 {
    if later(until, now) {
        until.wrapping_sub(now).div_ceil(1000)
    } else {
        0
    }
}

impl Game {
    /// Cases in the world (tests and tools): `(key, position, reserved for)`.
    pub fn hisec_cases(&self) -> Vec<(u32, [f32; 3], Option<u8>)> {
        self.cases.iter().map(|c| (c.key, c.pos, c.owner)).collect()
    }

    /// The reserved cases and their last countdown value (for a client that enters the world, HS-3).
    pub fn hisec_countdowns(&self) -> Vec<(u32, u32)> {
        self.cases
            .iter()
            .filter(|c| c.owner.is_some())
            .map(|c| (c.key, c.shown))
            .collect()
    }

    /// The reservation state of every case to one client (its introduction, HS-3): 0xd6 sub 2 with the owner or
    /// nobody.
    // DC-DEFAULT: HS-3 (amendment, proposed)
    pub(super) fn hisec_introduce(&mut self, pid: u8) {
        let st: Vec<(u32, Option<u8>)> = self.cases.iter().map(|c| (c.key, c.owner)).collect();
        for (key, owner) in st {
            self.send(pid, gs::hisec_reserve(key, owner));
        }
    }

    /// The drop chance of one kill in this room: `1 / expected kills` (the room goal in TDM, else
    /// `hisec.expected_kills`), 1 with the test flag.
    // DC-DEFAULT: HS-1 (project decision)
    pub fn hisec_drop_chance(&self) -> f64 {
        let h = &self.cfg.hisec;
        if h.force {
            return 1.0;
        }
        let expected = if self.rules.mode_class == MODE_CLASS_TDM && self.rules.goal > 0 {
            self.rules.goal
        } else {
            h.expected_kills
        };
        1.0 / f64::from(expected.max(1))
    }

    fn case_look_of(&self, case_row: u32) -> Option<CaseLook> {
        let row = self.tables.hisec.rows.get(&case_row)?;
        Some(CaseLook {
            model: row.model.clone()?,
            textures: row.skins.clone(),
            styles: row.styles.clone(),
            scale: row.scale.unwrap_or([1.0; 3]),
        })
    }

    /// The case table row of the next drop: one of `hisec.case_rows` drawn by weight among the rows the ROM table
    /// has a model for; `hisec.case_row` when the list is empty or none of its rows exists.
    // DC-DEFAULT: HS-15 (proposed)
    pub(crate) fn pick_case_row(&mut self) -> u32 {
        let pool: Vec<(u32, u32)> = self
            .cfg
            .hisec
            .case_rows
            .iter()
            .copied()
            .filter(|&(r, w)| {
                w > 0
                    && self
                        .tables
                        .hisec
                        .rows
                        .get(&r)
                        .is_some_and(|x| x.model.is_some())
            })
            .collect();
        let total: u64 = pool.iter().map(|&(_, w)| u64::from(w)).sum();
        if total == 0 {
            return self.cfg.hisec.case_row;
        }
        let mut pick = self.next_rand() % total;
        for (r, w) in pool {
            if pick < u64::from(w) {
                return r;
            }
            pick -= u64::from(w);
        }
        self.cfg.hisec.case_row
    }

    /// Cases in the world with their rows (tests and tools): `(key, row)`.
    pub fn hisec_case_rows(&self) -> Vec<(u32, u32)> {
        self.cases.iter().map(|c| (c.key, c.row)).collect()
    }

    /// Where a case lands: on the floor below the victim plus [`CASE_FLOOR_LIFT`], else the victim's position.
    // DC-DEFAULT: HS-2 (amendment, proposed)
    fn case_place(&self, at: [f32; 3]) -> [f32; 3] {
        match self.wall_along(at, [0.0, -1.0, 0.0], CASE_FLOOR_SEARCH) {
            Some(d) => [at[0], at[1] - d + CASE_FLOOR_LIFT, at[2]],
            None => at,
        }
    }

    /// A kill by `killer` of a foe `vpid`: release the cases reserved for the victim, then roll a drop (HS-1): only
    /// while the round runs, not in co-op or clan matches, with fewer than `max_active` cases in the world and at
    /// least `min_players` players in the match, and not on the kill that reaches the goal.
    // DC-DEFAULT: HS-1 (project decision), HS-2, HS-3 (proposed)
    pub(super) fn hisec_on_kill(&mut self, vpid: u8, killer: Option<u8>, now: Millis) {
        self.hisec_release_owner(vpid);
        let h = self.cfg.hisec;
        let Some(k) = killer.filter(|&k| k != vpid) else {
            return;
        };
        if !h.enabled
            || !matches!(self.phase, RoundPhase::Running { .. })
            || self.is_coop()
            || self.rules.mode_class == MODE_CLASS_CLAN_TDM
            || self.cases.len() as u32 >= h.max_active
            || (self.introduced().len() as u32) < h.min_players
        {
            return;
        }
        // the kill that reaches the goal ends the round: no case nobody could take
        let goal = self.rules.goal;
        if goal > 0 && self.scores.iter().any(|&s| s >= goal) {
            return;
        }
        let p = self.hisec_drop_chance();
        // 53 random bits as a fraction in [0, 1)
        let roll = (self.next_rand() >> 11) as f64 / (1u64 << 53) as f64;
        if roll >= p {
            return;
        }
        let row = self.pick_case_row();
        let Some(look) = self.case_look_of(row) else {
            self.log(format!(
                "hisec: no case model (ROM row {row} of the case table), no drop"
            ));
            return;
        };
        let Some((at, yaw)) = self.players.get(&vpid).map(|v| (v.pos, v.yaw)) else {
            return;
        };
        let pos = self.case_place(at);
        let key = self.next_case_key;
        self.next_case_key = self.next_case_key.wrapping_add(1).max(1);
        self.out.push(GameOut::CaseCreate(Box::new(CaseOut {
            key,
            look,
            pos,
            yaw,
        })));
        self.broadcast(&gs::hisec_dropped());
        let reserved = h.reserve_ms > 0;
        self.broadcast(&gs::hisec_reserve(key, reserved.then_some(k)));
        let until = now.wrapping_add(h.reserve_ms);
        let shown = if reserved {
            whole_secs_left(now, until)
        } else {
            0
        };
        if reserved {
            self.out.push(GameOut::CaseNote {
                key,
                remaining: shown,
            });
        }
        self.cases.push(Case {
            key,
            row,
            pos,
            owner: reserved.then_some(k),
            reserved_until: until,
            expires: (h.lifetime_ms > 0).then(|| now.wrapping_add(h.lifetime_ms)),
            shown,
        });
        self.log(format!(
            "hisec: case {key} (row {row}) dropped at ({:.0}, {:.0}, {:.0}) for client {k} (chance {p:.3})",
            pos[0], pos[1], pos[2]
        ));
    }

    /// The reservations of `pid` end at once (death or leave): everyone may take the case.
    // OBSERVED: capture 20261025_065016_tdm6 c6 seq 11045 / 11054 (the reserving player died:
    // 0xd6 sub 2 with -1 and the countdown 0 right after the kill; REFERENCE, accepted by the real client)
    // DC-DEFAULT: HS-3 (amendment, proposed)
    pub(super) fn hisec_release_owner(&mut self, pid: u8) {
        let keys: Vec<u32> = self
            .cases
            .iter()
            .filter(|c| c.owner == Some(pid))
            .map(|c| c.key)
            .collect();
        for key in keys {
            self.hisec_release(key);
        }
    }

    fn hisec_release(&mut self, key: u32) {
        if let Some(c) = self.cases.iter_mut().find(|c| c.key == key) {
            c.owner = None;
            c.shown = 0;
        } else {
            return;
        }
        self.broadcast(&gs::hisec_reserve(key, None));
        self.out.push(GameOut::CaseNote { key, remaining: 0 });
    }

    /// Per tick: reservation countdown and its end, lifetime, pickups (while the round runs).
    // DC-DEFAULT: HS-3, HS-4, HS-5 (proposed)
    pub(super) fn tick_hisec(&mut self, now: Millis) {
        if self.cases.is_empty() {
            return;
        }
        let running = matches!(self.phase, RoundPhase::Running { .. });
        let keys: Vec<u32> = self.cases.iter().map(|c| c.key).collect();
        for key in keys {
            let Some(c) = self.cases.iter().find(|c| c.key == key).cloned() else {
                continue;
            };
            if c.owner.is_some() {
                let left = whole_secs_left(now, c.reserved_until);
                if left == 0 {
                    self.hisec_release(key);
                } else if left != c.shown {
                    if let Some(m) = self.cases.iter_mut().find(|m| m.key == key) {
                        m.shown = left;
                    }
                    self.out.push(GameOut::CaseNote {
                        key,
                        remaining: left,
                    });
                }
            }
            if c.expires.is_some_and(|t| !later(t, now)) {
                self.cases.retain(|m| m.key != key);
                self.out.push(GameOut::CaseGone { key });
                self.log(format!("hisec: case {key} expired"));
                continue;
            }
            if !running {
                continue;
            }
            let owner = self
                .cases
                .iter()
                .find(|m| m.key == key)
                .and_then(|m| m.owner);
            let r = self.cfg.hisec.pickup_radius;
            let taker = self
                .players
                .values()
                .filter(|p| p.life == Life::Alive && p.introduced)
                .filter(|p| owner.is_none_or(|o| o == p.pid))
                .map(|p| (p.pid, len3(sub3(p.pos, c.pos))))
                .filter(|(_, d)| d.is_finite() && *d < r)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(pid, _)| pid);
            if let Some(pid) = taker {
                self.hisec_take(key, pid);
            }
        }
    }

    /// `pid` picks up the case: 0xd6 sub 1 to everyone, the object removed, the `@hisec` line to the lobby.
    // SPEC: HISEC_2009.md §3.2 sub 1, §6.1 (sub 1, then remove the object); DSERVER_PIPE.md §4 (server -> lobby
    // event lines; `@hisec` is a DeCombat addition, the lobby ignores lines it does not know)
    // DC-DEFAULT: HS-5 (proposed); HS-6 (proposed) delivery is the lobby's
    fn hisec_take(&mut self, key: u32, pid: u8) {
        let row = self
            .cases
            .iter()
            .find(|c| c.key == key)
            .map_or(self.cfg.hisec.case_row, |c| c.row);
        self.cases.retain(|c| c.key != key);
        self.broadcast(&gs::hisec_taken(key, pid));
        self.out.push(GameOut::CaseGone { key });
        let (uid, nick) = self
            .players
            .get(&pid)
            .map(|p| {
                (
                    p.uid.unwrap_or(0),
                    p.roster
                        .as_ref()
                        .map(|r| r.nick.clone())
                        .unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        // HS-13 / HS-15: the row the case dropped as, so the lobby delivers that case
        self.out
            .push(GameOut::Pipe(crate::pipe::hisec_line(uid, row, key, &nick)));
        self.log(format!("hisec: case {key} picked up by client {pid}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_set_and_refuse() {
        let mut h = HisecTuning::default();
        h.set("hisec.force", "on").unwrap();
        h.set("pickup_radius", "5000").unwrap();
        h.set("reserve_ms", "0").unwrap();
        assert!(h.force);
        assert_eq!((h.pickup_radius, h.reserve_ms), (5000.0, 0));
        assert!(h.set("nope", "1").is_err());
        assert!(h.set("force", "maybe").is_err());
        h.set("expected_kills", "0").unwrap();
        assert_eq!(h.expected_kills, 1);
        // HS-15: the weighted case rows
        assert_eq!(
            HisecTuning::default().case_rows,
            [(0, 80), (1, 20), (0, 0), (0, 0)]
        );
        h.set("case_rows", "0:50/1:30/2:20").unwrap();
        assert_eq!(h.case_rows, [(0, 50), (1, 30), (2, 20), (0, 0)]);
        h.set("hisec.case_rows", "none").unwrap();
        assert!(h.case_rows.iter().all(|&(_, w)| w == 0));
        assert!(h.set("case_rows", "0:0/1:0").is_err(), "no weight at all");
        assert!(h.set("case_rows", "0-80").is_err());
    }

    #[test]
    fn countdown_rounds_up() {
        assert_eq!(whole_secs_left(1_000, 11_000), 10);
        assert_eq!(whole_secs_left(1_001, 11_000), 10);
        assert_eq!(whole_secs_left(10_001, 11_000), 1);
        assert_eq!(whole_secs_left(11_000, 11_000), 0);
        assert_eq!(whole_secs_left(12_000, 11_000), 0);
    }
}
