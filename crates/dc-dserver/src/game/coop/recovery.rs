//! The recovery between the rounds of a co-op round world (DSV-593) and at a chapter's start (DSV-594): the living
//! share the round's health (`healinpart`, DSV-578), every human's reserve is refilled to a new kit's count with the
//! clip untouched, and the dead come back at the round's spawn points; a new chapter heals the living fully.
//!
//! No 2009 server evidence exists for any of it: the amounts come from the ROM's difficulty rows, the rest is ours,
//! shaped by the owner's description of a December 2009 recording (DC_DEFAULTS.md DSV-593 / DSV-594).

use super::super::packs::RefillSet;
use super::super::{Game, Life, RoundPhase};
use crate::coop_rom::CoopPlan;
use crate::gs;
use dc_bute::ButeDoc;
use dc_net_engine::Millis;

/// One living human's share of the round's recovery `total` when `living` humans share it: an even split in whole
/// points (all of it for a lone survivor; nothing below 0).
// OBSERVED: owner description of a December 2009 Cabin Fever recording (Normal), 2026-11-05: 33 -> 55 health at a
// round end; the ROM's Normal row gives 180 (`RecoverRoundTotalHP`, read at run time), 180 / 8 = 22.5
// DC-DEFAULT: DSV-593 (proposed) the even split among the living, rounded down
pub fn heal_share(total: f32, living: usize) -> f32 {
    let total = total.max(0.0);
    if living <= 1 {
        return total;
    }
    (total / living as f32).floor()
}

/// The last rounds of the difficulties easier than `goal` (their health-rate rows, DSV-584): where a chapter of the
/// harder difficulty ends (10 and 15 on the round map of the ROM).
// DC-DEFAULT: DSV-594 (proposed)
pub(super) fn chapter_ends(doc: &ButeDoc, goal: u32) -> Vec<u32> {
    let mut ends: Vec<u32> = (0..goal.min(2))
        .map(|g| CoopPlan::from_gms(doc, g).last_round() as u32)
        .filter(|n| *n > 0)
        .collect();
    ends.sort_unstable();
    ends.dedup();
    ends
}

impl Game {
    /// The mission timer of a co-op room: the room line's `time=` (the lobby sends the difficulty row's
    /// `TimeLimit` there) or `-roundsecs`; a line without `time=` takes the row's `TimeLimit` itself. The round
    /// clock running out ends the mission as timed out (FT-15).
    // DC-DEFAULT: DSV-595 (proposed)
    pub(super) fn coop_mission_clock(&mut self, plan: &CoopPlan) {
        if self.cfg.round_secs.is_some() || self.room.get("time").is_some() {
            return;
        }
        if let Some(t) = plan.difficulty.time_limit_s.filter(|t| *t > 0) {
            self.rules.round_secs = super::super::room_time_secs(i64::from(t));
        }
    }

    /// The round world's recovery: every living human's reserve to a new kit's count (the clip untouched; with
    /// `chapter` also his health to the maximum), every dead human spawned again at the round's spawn points with
    /// the spawn set (a full kit, the start health; his lives stay as they are). Paid once per cleared round.
    // DC-DEFAULT: DSV-593 (proposed) round end; DSV-594 (proposed) chapter
    pub(super) fn coop_recover(&mut self, now: Millis, chapter: bool) {
        if !matches!(self.phase, RoundPhase::Running { .. }) {
            return;
        }
        let humans: Vec<(u8, Life)> = self
            .players
            .values()
            .filter(|p| p.introduced && p.spawns > 0)
            .map(|p| (p.pid, p.life))
            .collect();
        let max = self.cfg.max_health;
        let (mut refilled, mut healed, mut revived) = (0u32, 0u32, 0u32);
        for (pid, life) in humans {
            match life {
                Life::Alive => {
                    if chapter {
                        if let Some(p) = self.players.get_mut(&pid).filter(|p| p.health < max) {
                            p.health = max;
                            self.send(pid, gs::player_status(gs::STATUS_HEALTH, max));
                            healed += 1;
                        }
                    }
                    if self.refill_kit(pid, RefillSet::All) > 0 {
                        refilled += 1;
                    }
                }
                Life::Dying { .. } | Life::Waiting { .. } => {
                    self.spawn(pid, now);
                    self.respawn_match_begin(pid, now);
                    revived += 1;
                }
                _ => {}
            }
        }
        let Some(c) = self.coop.as_mut() else {
            return;
        };
        c.recovery_owed = false;
        c.recoveries += 1;
        if chapter {
            c.chapter_recoveries += 1;
        }
        self.log(format!(
            "co-op: {} recovery ({refilled} refilled, {healed} healed fully, {revived} back)",
            if chapter { "chapter" } else { "round" }
        ));
    }

    /// Harness entry of the recovery tests: sets a player's health and, with `gun`, the clip and reserve of his
    /// first gun (a weapon with a clip that is not its own ammunition); the server's view only, his client is not
    /// told.
    pub fn harness_vitals(&mut self, pid: u8, health: f32, gun: Option<(u32, u32)>) {
        let Some(p) = self.players.get_mut(&pid) else {
            return;
        };
        p.health = health;
        let first_gun = p
            .weapons
            .iter_mut()
            .find(|w| w.clip_size > 0 && !w.own_ammo);
        if let (Some((clip, stock)), Some(w)) = (gun, first_gun) {
            w.clip = clip;
            w.stock = stock;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::{Game, GameOut, GameTuning, Life, RoundPhase, WeaponState};
    use super::super::WavePhase;
    use super::*;
    use crate::pipe::RoomSettings;
    use crate::rom_tables::RomTables;
    use crate::world::{synthetic_world_bytes, world_from_bytes};

    fn gun(clip: u32, stock: u32) -> WeaponState {
        WeaponState {
            weapon: 3,
            ammo: 3,
            clip_size: 30,
            clip,
            stock,
            slot: 0,
            own_ammo: false,
            parts: gs::NO_PARTS,
            shot_open: false,
            shot_ok: false,
        }
    }

    /// A running co-op room (class 5) on the synthetic world, marked a round world, with `living` introduced living
    /// humans (clients 1..) at 33 health with 9 + 20 in the gun (clip 30, the stock cap without a ROM row: 90) and
    /// one dead human after them; the difficulty row's round total `total`.
    fn game(living: u8, total: f32) -> (Game, u8) {
        let w = world_from_bytes(&synthetic_world_bytes(), "Worlds\\S.dat", "S").unwrap();
        let mut g = Game::new(GameTuning::default(), &w, RomTables::default());
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "5".into());
        g.set_room(&room);
        let dead = living + 1;
        for pid in 1..=dead {
            g.prepare_player(
                pid,
                None,
                None,
                10 + u16::from(pid),
                [0.0; 3],
                [0.0; 3],
                None,
                0,
            );
            let p = g.players.get_mut(&pid).unwrap();
            p.introduced = true;
            p.spawns = 1;
            p.life = if pid == dead {
                Life::Waiting { died: 100 }
            } else {
                Life::Alive
            };
            p.health = if pid == dead { 0.0 } else { 33.0 };
            p.weapons = vec![gun(9, 20)];
        }
        let c = g.coop.as_mut().unwrap();
        c.round_world = true;
        c.plan.difficulty.recover_hp = Some(total);
        c.chapter_ends = vec![2];
        g.phase = RoundPhase::Running { start: 0 };
        g.take_out();
        (g, dead)
    }

    /// DSV-593: the even split in whole points; a lone survivor takes all.
    #[test]
    fn the_living_share_the_round_total() {
        assert_eq!(heal_share(180.0, 8), 22.0);
        assert_eq!(heal_share(120.0, 8), 15.0);
        assert_eq!(heal_share(80.0, 8), 10.0);
        assert_eq!(heal_share(180.0, 3), 60.0);
        assert_eq!(heal_share(180.0, 1), 180.0);
        assert_eq!(heal_share(180.0, 0), 180.0);
        assert_eq!(heal_share(-5.0, 4), 0.0);
    }

    /// DSV-593: a cleared round owes its recovery; `healinpart` heals the eight living from 33 to 55 (180 / 8), fills
    /// the reserve to 111 beside the 9 in the clip (a new kit's 120), and brings the dead human back; paid once.
    #[test]
    fn healinpart_pays_the_round_recovery() {
        let (mut g, dead) = game(8, 180.0);
        g.coop.as_mut().unwrap().recovery_owed = true;
        g.coop_heal_in_part(1_000);
        for pid in 1..dead {
            let p = &g.players[&pid];
            assert_eq!(p.health, 55.0, "client {pid}");
            assert_eq!((p.weapons[0].clip, p.weapons[0].stock), (9, 111));
        }
        let d = &g.players[&dead];
        assert_eq!(d.life, Life::Alive, "the dead human is back");
        assert_eq!(d.health, g.cfg.start_health);
        let c = g.coop.as_ref().unwrap();
        assert!(!c.recovery_owed);
        assert_eq!((c.heals, c.recoveries), (1, 1));
        let out = g.take_out();
        assert!(out.iter().any(|o| matches!(o, GameOut::To(1, m)
            if *m == gs::player_status(gs::STATUS_HEALTH, 55.0))));
        // a second `healinpart` heals again, with nothing owed it refills nobody
        g.players.get_mut(&1).unwrap().weapons[0].stock = 0;
        g.coop_heal_in_part(2_000);
        assert_eq!(g.players[&1].weapons[0].stock, 0);
        assert_eq!(g.coop.as_ref().unwrap().recoveries, 1);
    }

    /// DSV-593: a round end without `healinpart` pays at the next round's start (no heal); DSV-594: the round after a
    /// chapter's end starts with the full recovery (health to the maximum).
    #[test]
    fn the_next_round_pays_an_owed_recovery_and_a_chapter_heals_fully() {
        let (mut g, dead) = game(2, 180.0);
        {
            let c = g.coop.as_mut().unwrap();
            c.recovery_owed = true;
            c.phase = WavePhase::Idle;
        }
        g.coop_round_counted(2, 1_000);
        assert_eq!(g.players[&1].health, 33.0, "no heal at a round start");
        assert_eq!(g.players[&1].weapons[0].stock, 111);
        assert_eq!(g.players[&dead].life, Life::Alive);
        assert_eq!(g.coop.as_ref().unwrap().chapter_recoveries, 0);
        // round 2 ends a chapter: round 3 starts with the full heal (owed or not)
        g.coop.as_mut().unwrap().phase = WavePhase::Idle;
        g.coop_round_counted(3, 2_000);
        assert_eq!(g.players[&1].health, g.cfg.max_health);
        let c = g.coop.as_ref().unwrap();
        assert_eq!((c.recoveries, c.chapter_recoveries), (2, 1));
        // a round that starts nothing (no owed recovery, no chapter) changes nobody
        g.players.get_mut(&1).unwrap().health = 40.0;
        g.coop.as_mut().unwrap().phase = WavePhase::Idle;
        g.coop_round_counted(4, 3_000);
        assert_eq!(g.players[&1].health, 40.0);
    }

    /// DSV-594: the chapter ends are the easier difficulties' last rounds.
    #[test]
    fn chapter_ends_from_the_easier_rows() {
        let doc = dc_bute::ButeDoc::parse(concat!(
            "[AIHealthRateRoundTable0]\nRound0 = 1\nRound1 = 1\nRound2 = 1\n",
            "[AIHealthRateRoundTable1]\nRound0 = 1\nRound1 = 1\nRound2 = 1\nRound3 = 1\n",
            "[AISpawnTable0]\nRound = 1\nSpawnAIName0 = \"a\"\nSpawnNum0 = 1\n",
            "[AISpawnTable1]\nRound = 2\nSpawnAIName0 = \"a\"\nSpawnNum0 = 1\n",
            "[AISpawnTable2]\nRound = 3\nSpawnAIName0 = \"a\"\nSpawnNum0 = 1\n",
            "[AISpawnTable3]\nRound = 4\nSpawnAIName0 = \"a\"\nSpawnNum0 = 1\n",
        ));
        assert_eq!(chapter_ends(&doc, 0), Vec::<u32>::new());
        assert_eq!(chapter_ends(&doc, 1), vec![2]);
        assert_eq!(chapter_ends(&doc, 2), vec![2, 3]);
    }

    /// DSV-595: a co-op room line without `time=` takes the difficulty row's `TimeLimit`; a given time stays.
    #[test]
    fn the_mission_timer_falls_back_to_the_rows_time_limit() {
        let w = world_from_bytes(&synthetic_world_bytes(), "Worlds\\S.dat", "S").unwrap();
        let mut g = Game::new(GameTuning::default(), &w, RomTables::default());
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "5".into());
        g.set_room(&room);
        assert_eq!(g.rules.round_secs, 0, "no ROM row: no timer");
        let mut plan = CoopPlan::default();
        plan.difficulty.time_limit_s = Some(1800);
        g.coop_mission_clock(&plan);
        assert_eq!(g.rules.round_secs, 1800);
        room.pairs.insert("time".into(), "600".into());
        g.set_room(&room);
        g.coop_mission_clock(&plan);
        assert_eq!(g.rules.round_secs, 600, "the room's own time wins");
    }

    /// FT-15 / DSV-595: the mission timer running out ends the co-op match as timed out (stage 5, a loss).
    #[test]
    fn the_mission_timer_ends_the_match_as_timed_out() {
        let (mut g, _) = game(2, 180.0);
        g.rules.round_secs = 1800;
        g.tick_round(1_799_000);
        assert_eq!(g.coop.as_ref().unwrap().stage, None);
        g.tick_round(1_800_000);
        assert_eq!(
            g.coop.as_ref().unwrap().stage,
            Some(super::super::STAGE_TIMEOUT)
        );
        assert!(!matches!(g.phase, RoundPhase::Running { .. }));
    }
}
