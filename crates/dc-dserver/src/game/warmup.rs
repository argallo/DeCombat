//! The warm-up of a Search and Destroy match (mode classes 2 / 3): the match opens with a timed warm-up while the
//! players load and get into the world; players move and shoot but nobody takes player damage (DSV-566: shots,
//! melee, grenades and blasts are refused; falls and kill volumes still hurt) and nothing counts: no bomb, no
//! round win, no team kills, no kills or deaths for the lobby, no case drop, no team-empty forfeit. At its end the
//! scores and counters are cleared and the warm-up ends like any round (DSV-567): the round result, the between-round
//! set and pause, then round 1 with everyone spawned again (the next-round set of GMD-21).
//!
//! 2009 evidence: the room line's `train` (the ROM mode files' `TrainingTimeLimit`, 10 s for the bomb maps) and the
//! 0x45 `waiting_banner` flag that draws the client's training picture (layout key `TrainingModInfoTex`) until a
//! 0x45 with the flag clear. What the 2009 server did in that time is not known: the rules here are ours.
// SPEC: DSERVER_PIPE.md §room line (`train` seconds, "warm-up period before play", meaning D); DC_DEFAULTS.md DSV-20
// (revision: a DeCombat warm-up may send 0x45 with the banner flag 1 and must clear it with the round start)
// OBSERVED: ROM table key GMS `TrainingTimeLimit` / `TrainingComment` (the bomb maps' files: 10 and a training
// picture; values read at run time by the lobby's room line)
// DC-DEFAULT: DSV-512 (proposed, user playtest 2026-11-03 "add a warmup round before round 1")

use super::{Game, RoundPhase};
use crate::gs;
use dc_net_engine::Millis;

/// The shortest warm-up: the room's `train` seconds when longer (loading a map takes longer than the ROM's 10 s on
/// a slow PC).
// DC-DEFAULT: DSV-512 (proposed) 20 s
pub const WARMUP_MIN_SECS: u32 = 20;
/// The longest warm-up an operator or a room may ask for.
pub const WARMUP_MAX_SECS: u32 = 300;

impl Game {
    /// A warm-up is running (its end time).
    pub fn warmup_until(&self) -> Option<Millis> {
        self.warmup_until
    }

    pub fn in_warmup(&self) -> bool {
        self.warmup_until.is_some()
    }

    /// The warm-up holds player damage: it is running and `-combat warmup_damage` is off (the default). Every mode
    /// with a warm-up gets the rule; today only the bomb classes have one ([`Game::warmup_secs`]).
    // DC-DEFAULT: DSV-566 (proposed; USER DECISION 2026-11-04 "nobody can kill or damage anyone" while waiting
    // for players)
    pub fn warmup_holds_damage(&self) -> bool {
        self.in_warmup() && !self.cfg.warmup_damage
    }

    /// The warm-up length of this room: 0 outside the bomb modes or without the round loop; else `-warmup <s>`
    /// when the operator gave it (0 = none), else the room's `train` seconds, at least [`WARMUP_MIN_SECS`].
    pub fn warmup_secs(&self) -> u32 {
        if !self.is_bomb() || !self.round_loop() {
            return 0;
        }
        if let Some(s) = self.cfg.warmup_secs {
            return s.min(WARMUP_MAX_SECS);
        }
        let train = self
            .room
            .get("train")
            .and_then(|v| v.trim().parse::<i64>().ok())
            .unwrap_or(0)
            .clamp(0, i64::from(WARMUP_MAX_SECS)) as u32;
        train.max(WARMUP_MIN_SECS)
    }

    /// The first round of the match begins: in a bomb room it is the warm-up first.
    pub(super) fn warmup_begin(&mut self, now: Millis) {
        let secs = self.warmup_secs();
        if secs == 0 || self.warmup_done {
            return;
        }
        self.warmup_until = Some(now.wrapping_add(secs.saturating_mul(1000)));
        self.log(format!(
            "warm-up: {secs} s (nothing counts; round 1 follows)"
        ));
    }

    /// The seconds the round clock shows during the warm-up (its time left, at least 1).
    pub(super) fn warmup_secs_left(&self, now: Millis) -> Option<u32> {
        let until = self.warmup_until?;
        let left = until.wrapping_sub(now);
        Some(if left >= u32::MAX / 2 {
            1
        } else {
            left.div_ceil(1000).max(1)
        })
    }

    /// The warm-up's end: scores, round wins, kills and deaths cleared (0x43), then the round end of every round
    /// (DSV-567): `GS_ROUND_RESULT` as a draw with the round wins 0:0, 0xb5, the round-end freeze; after the first
    /// pause the between-round set (0x47, every roster record again, 0x48), after the between-round pause round 1
    /// with everyone spawned (the next-round set: 0x44, 0xb5, 0x45 with the banner flag clear).
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.1.2 (0x46 clears the banner element +0x6a; with the 0x45 waiting flag set
    // the client skips its half-time picture check; win 0 shows no team)
    // DC-DEFAULT: DSV-567 (proposed; USER DECISION 2026-11-04 "after waiting for players ends it should go to the
    // between-round image, not jump you right into the next round")
    pub(super) fn tick_warmup(&mut self, now: Millis) {
        let Some(until) = self.warmup_until else {
            return;
        };
        if !matches!(self.phase, RoundPhase::Running { .. }) {
            return;
        }
        if now.wrapping_sub(until) >= u32::MAX / 2 {
            return;
        }
        self.scores = [0; 2];
        self.round_wins = [0; 2];
        self.first_kill_done = false;
        for s in self.stats.values_mut() {
            s.kills = 0;
            s.deaths = 0;
            s.team_kills = 0;
            s.headshots = 0;
        }
        self.broadcast(&gs::scores_clear());
        self.log("warm-up over: the between-round pause, then round 1".into());
        // the round-end path with the warm-up still on (no match end, no side switch); `next_round` then numbers
        // the first counted round 1
        self.round_no = 0;
        self.end_round(now, 0);
        self.warmup_until = None;
        self.warmup_done = true;
    }
}
