//! Mode rules beyond the two-team kill count: free-for-all (class 6) with one team value per player, and the side
//! switch of the clan classes (3, 4, 13) with `GS_TEAM_SWAP` 0x5e or `GS_CLAN_STATUS` 0x5f, always after a
//! `GS_ROUND_RESULT` (class 4 is played as two halves).
//!
//! What the 2009 client does is cited; when the server switches and how it scores free-for-all are DeCombat rules
//! (DSV-147..DSV-152, refining GMD-7 and GMD-10).
// SPEC: GAME_MODES_2009.md §4.6 (side switch), §5.4 (FFA counter rule trap); GAME_MESSAGES_2009_S2C.md §2.3 (0x5e,
// 0x5f), §3.2.1 (0x6b own record sets the own team)

use super::{Game, RoundPhase};
use crate::gs;
use crate::pipe::RosterEntry;
use dc_net_engine::time::since;
use dc_net_engine::Millis;
use std::collections::BTreeMap;

/// The FFA mode class.
// SPEC: GAME_MODES_2009.md §5.4 (`FFA` = class 6)
pub const MODE_CLASS_FFA: u8 = gs::MODE_CLASS_6;
/// Plain Search and Destroy.
// SPEC: GAME_MODES_2009.md §2 (`BOMB` = class 2)
pub const MODE_CLASS_BOMB: u8 = 2;
/// The clan classes whose client draws the "teams change sides" picture (cshell+0xcd210).
// EVIDENCE: cshell+0xcd210 (classes 3, 4, 8, 13)
pub const MODE_CLASS_BOMB_CLAN: u8 = 3;
pub const MODE_CLASS_TDM_CLAN: u8 = 4;
pub const MODE_CLASS_FLAG_CLAN: u8 = 8;
pub const MODE_CLASS_ELIM_CLAN: u8 = 13;
/// Highest FFA team value (`team:u8` of 0x6b; the 0x46 `win_team` is an `i8`).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 row 11 (`team:u8`), §2.1.2 (`win_team:i8`)
pub const FFA_TEAM_MAX: u8 = 127;

/// The side switch message (`-sideswitch off|5e|5f`).
// DC-DEFAULT: GMD-7 (proposed) on with 0x5e; DSV-147 (proposed) the 0x5f form as the alternative
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideSwitch {
    Off,
    /// `GS_TEAM_SWAP` 0x5e with the scores.
    TeamSwap,
    /// `GS_CLAN_STATUS` 0x5f kind 1 with the scores and `no_swap` 0.
    ClanStatus,
}

/// Per-match state of the mode rules.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModeState {
    /// FFA: foe kills of each player this match (by client id; cleared when a player is added).
    pub ffa_kills: BTreeMap<u8, u32>,
    /// FFA: two or more players were in the match at once, and since when only one is left.
    pub ffa_seen_two: bool,
    pub ffa_alone_since: Option<Millis>,
    /// The sides were switched (once per match); every team value 1 / 2 is the other one of the lobby's.
    pub swapped: bool,
    /// The round end that just went out reached the half: the switch follows with the between-round set.
    pub swap_pending: bool,
    /// Clan TDM halves (DSV-147): the first half is over, and the match clock it used.
    pub half_done: bool,
    pub half_clock_ms: u32,
    /// FFA: deaths by a foe kill of each player (by client id), for the 0x6b refresh of a player without a uid.
    pub ffa_deaths: BTreeMap<u8, u32>,
}

/// The half-time test of the 2009 client on the values of a `GS_ROUND_RESULT` it received: `round_limit`, the two
/// win counters, the stored `round_no`, and whether the picture was already shown this match (classes 4 and 8).
/// No picture when a counter equals the limit (the match is over).
// EVIDENCE: cshell+0xcd210 (half = limit / 2; class 3: half == wins1 + wins2; class 4: first time either counter >=
// half; class 8: first time; class 13: half == `match_ui`+0x9a58 (the round number of 0x44)); cshell+0x49a40 case
// 0x46 (called with the received counters while the 0x45 waiting flag is clear)
pub fn half_time_picture(
    mode_class: u8,
    round_limit: u32,
    wins: [u32; 2],
    round_no: u32,
    shown: bool,
) -> bool {
    let half = round_limit / 2;
    if wins[0] == round_limit || wins[1] == round_limit {
        return false;
    }
    match mode_class {
        MODE_CLASS_BOMB_CLAN => half == wins[0] + wins[1],
        MODE_CLASS_TDM_CLAN => !shown && (half <= wins[0] || half <= wins[1]),
        MODE_CLASS_FLAG_CLAN => !shown,
        MODE_CLASS_ELIM_CLAN => half == round_no,
        _ => false,
    }
}

impl Game {
    /// Whether the room plays free-for-all with one team value per player (GMD-10).
    // DC-DEFAULT: GMD-10 (proposed), DSV-148 (proposed)
    pub fn is_ffa(&self) -> bool {
        self.rules.mode_class == MODE_CLASS_FFA && self.cfg.ffa_teams
    }

    /// The team value of a joining player in a free-for-all room: the lowest value from 1 that no other player in the
    /// match holds (join order 1, 2, 3, ...; a leaver's value is given again).
    // SPEC: GAME_MODES_2009.md §5.4 (the client counts a kill only between different team values, cshell+0x2c430)
    // DC-DEFAULT: GMD-10 (proposed) distinct values in join order; DSV-148 (proposed) the lowest free value
    pub(super) fn ffa_team(&self, pid: u8) -> u8 {
        (1..=FFA_TEAM_MAX)
            .find(|t| !self.players.values().any(|p| p.pid != pid && p.team == *t))
            .unwrap_or(FFA_TEAM_MAX)
    }

    /// The lobby team of a roster entry after the side switch of this match (1 <-> 2 once the sides switched).
    // DC-DEFAULT: DSV-149 (proposed) a joiner after the switch plays on his side's new team value
    pub(super) fn sided(&self, team: u8) -> u8 {
        if self.modes.swapped && (team == 1 || team == 2) {
            3 - team
        } else {
            team
        }
    }

    /// Team of a player without the co-op rule: FFA value or the lobby team on the current sides.
    pub(super) fn team_without_coop(&self, entry: Option<&RosterEntry>, pid: u8) -> u8 {
        // DC-DEFAULT: DSV-210 (proposed) Quarantine: everybody joins as a human (team 1)
        if self.is_infect() {
            return super::infect::HUMANS;
        }
        // DC-DEFAULT: DSV-236 (proposed) Spy Hunt: everybody joins as a hunter (team 1)
        if self.is_spy() {
            return super::spy::HUNTERS;
        }
        if self.is_ffa() {
            self.ffa_team(pid)
        } else {
            self.sided(Self::team_for(entry, pid))
        }
    }

    /// FFA foe kills of a player.
    pub fn ffa_kills_of(&self, pid: u8) -> u32 {
        self.modes.ffa_kills.get(&pid).copied().unwrap_or(0)
    }

    /// The FFA leader among the players in the match: the team value of the one player with the most foe kills, 0
    /// when two or more share the lead (or nobody is there).
    // DC-DEFAULT: DSV-148 (proposed) time limit: the most kills wins, a shared lead is a draw
    pub(super) fn ffa_leader(&self) -> u8 {
        let mut best: Option<(u32, u8)> = None;
        let mut shared = false;
        for p in self.players.values().filter(|p| p.introduced) {
            let k = self.ffa_kills_of(p.pid);
            match best {
                Some((b, _)) if k < b => {}
                Some((b, _)) if k == b => shared = true,
                _ => {
                    best = Some((k, p.team));
                    shared = false;
                }
            }
        }
        match best {
            Some((_, t)) if !shared => t,
            _ => 0,
        }
    }

    /// A foe kill in FFA: the killer's count, then the absolute counters of killer and victim to everyone (0x6b
    /// kind 2), so the scoreboard is right whichever way the client counts.
    // SPEC: GAME_MODES_2009.md §5.4 (V-GM-3: which way the 2009 server went is UNPROVEN)
    // EVIDENCE: cshell+0x2b820 (0x6b kind 2 writes the record's counters and team, `team` at record +0x70)
    // DC-DEFAULT: GMD-10 (proposed) the 0x6b kind 2 refresh after each kill
    pub(super) fn ffa_on_kill(&mut self, killer: u8, victim: u8, weapon: u16, now: Millis) {
        // DC-DEFAULT: DSV-481 (proposed) Gun Game: the count is the rung
        if self.is_gun_game() {
            self.gun_game_on_kill(killer, victim, weapon, now);
        } else {
            *self.modes.ffa_kills.entry(killer).or_insert(0) += 1;
        }
        *self.modes.ffa_deaths.entry(victim).or_insert(0) += 1;
        let infos: Vec<gs::PlayerInfo> = [killer, victim]
            .iter()
            .filter_map(|pid| self.players.get(pid))
            .filter(|p| p.introduced)
            .map(|p| self.info_of(p, gs::INFO_UPDATE))
            .collect();
        for i in &infos {
            self.broadcast(&gs::player_info(i));
        }
    }

    /// The FFA goal: a player with `goal` foe kills ends the match; his team value wins.
    // DC-DEFAULT: GMD-1 (proposed) the goal counts player kills in class 6; DSV-148 (proposed) the winner's team value
    // in 0x46 / 0x3f
    pub(super) fn ffa_check_goal(&mut self, now: Millis) {
        let goal = self.rules.goal;
        if goal == 0 {
            return;
        }
        let win = self
            .players
            .values()
            .find(|p| self.ffa_kills_of(p.pid) >= goal)
            .map(|p| p.team);
        if let Some(w) = win {
            self.log(format!(
                "ffa: team value {w} reached the goal of {goal} kills"
            ));
            self.end_round(now, w);
        }
    }

    /// DSV-31 in FFA: after two or more players were in the match, one left alone for `team_empty_ms` wins.
    // DC-DEFAULT: DSV-31 (proposed) the delay; DSV-148 (proposed) the FFA form
    pub(super) fn tick_ffa_alone(&mut self, now: Millis) {
        let here: Vec<u8> = self
            .players
            .values()
            .filter(|p| p.introduced)
            .map(|p| p.team)
            .collect();
        if here.len() >= 2 {
            self.modes.ffa_seen_two = true;
            self.modes.ffa_alone_since = None;
            return;
        }
        if here.len() != 1 || !self.modes.ffa_seen_two || self.cfg.team_empty_ms == 0 {
            self.modes.ffa_alone_since = None;
            return;
        }
        let t0 = *self.modes.ffa_alone_since.get_or_insert(now);
        if since(now, t0) >= self.cfg.team_empty_ms {
            self.log(format!(
                "ffa: one player left for {} ms: team value {} wins",
                since(now, t0),
                here[0]
            ));
            let hold = self.cfg.end_pause_ms.0;
            self.end_round_with(now, here[0], hold, true);
        }
    }

    /// The side switch of a round mode, decided at a round end that does not end the match: when the
    /// `GS_ROUND_RESULT` just sent makes the 2009 client draw the half-time picture, the sides switch with the
    /// between-round set.
    // EVIDENCE: cshell+0xcd210 (the picture), cshell+0x49a40 case 0x46
    // DC-DEFAULT: GMD-7 (proposed) classes 3 and 13 at half the rounds; DSV-147 (proposed) the switch at the very
    // round end the client shows the picture for
    pub(super) fn side_switch_at_round_end(&mut self, sent_wins: [u32; 2]) {
        if self.cfg.side_switch == SideSwitch::Off || self.modes.swapped {
            return;
        }
        let rule = self.rules.mode_class;
        // DC-DEFAULT: DSV-242 (proposed; withdraws DSV-204's class-2 switch, playtest report) plain Search and Destroy
        // (class 2) keeps its sides for the whole match: the 2009 client draws the half-time picture only for the clan
        // classes 3, 4, 8 and 13 (cshell+0xcd210) and no class-2 path of it exists
        // project decision; DC-DEFAULT: DSV-471 (proposed) Elimination Pro (12 and 13) keeps each team on the
        // side it started the match on: class 13 does not switch either (the client's half-time test for 13,
        // cshell+0xcd210, still exists, but the user chose no switch), so the spawn sets never swap between rounds
        if rule != MODE_CLASS_BOMB_CLAN {
            return;
        }
        if half_time_picture(rule, self.round_limit(), sent_wins, self.round_no, false) {
            self.modes.swap_pending = true;
            self.log(format!(
                "round {}: half of {} rounds played: the sides switch before the next round",
                self.round_no,
                self.round_limit()
            ));
        }
    }

    /// Whether the room plays the clan TDM class (4) as two halves with the side switch between them: on with the
    /// side switch, a kill goal of at least 2 and one round per match. The first half ends when either side's team
    /// kills reach half the goal; the second half plays to the goal.
    // EVIDENCE: cshell+0xcd210 (class 4: the picture the first time a 0x46 counter reaches `round_limit / 2`, never
    // when a counter equals `round_limit`), called only from cshell+0x49a40 case 0x46
    // DC-DEFAULT: GMD-7 (proposed) class 4 at half the goal of either side; DSV-147 (proposed) two halves
    pub fn clan_halves(&self) -> bool {
        // DC-DEFAULT: DSV-291 (proposed) the clan flag class (8) is played in the same two halves, by captures
        self.cfg.side_switch != SideSwitch::Off
            && matches!(
                self.rules.mode_class,
                MODE_CLASS_TDM_CLAN | MODE_CLASS_FLAG_CLAN
            )
            && self.rules.goal >= 2
            && !self.round_loop()
            && !self.is_coop()
    }

    /// `round_limit` of `GS_ROUND_BEGIN` 0x44: the kill goal in the clan TDM halves (the client's half-time test
    /// halves it), else [`Game::round_limit`].
    // EVIDENCE: cshell+0xcd210 (`half = round_limit / 2`; match over when a counter equals `round_limit`)
    // DC-DEFAULT: DSV-147 (proposed) round_limit = goal in the 0x44 of both halves; the 0x7d settings unchanged
    pub(super) fn round_begin_limit(&self) -> u32 {
        if self.clan_halves() {
            self.rules.goal
        } else {
            self.round_limit()
        }
    }

    /// The first half of the clan TDM class ends in the tick of the kill that brings either side's team kills to
    /// half the goal (and not to the goal itself, which ends the match).
    // DC-DEFAULT: GMD-7 (proposed); DSV-147 (proposed) the half by the kill; the time limit in the first half ends the
    // match
    pub(super) fn clan_half_on_kill(&mut self, now: Millis) {
        if !self.clan_halves()
            || self.modes.half_done
            || !matches!(self.phase, RoundPhase::Running { .. })
        {
            return;
        }
        let half = self.rules.goal / 2;
        if self.scores.iter().all(|&s| s < half) {
            return;
        }
        self.end_first_half(now);
    }

    /// What the clan halves count: the captures in the flag class, else the team kills.
    pub(super) fn half_counters(&self) -> [u32; 2] {
        if self.is_flag() {
            self.ctf.captures
        } else {
            self.scores
        }
    }

    /// The end of the first half: `GS_ROUND_RESULT` `[pause][win team][the two team kills]` (the client draws the
    /// half-time picture with the 0x44 `round_limit` = goal), `GS_ROUND_WINS` with the team kills, the players
    /// stop; after the first pause the between-round set with the switch message, after the between-round pause the
    /// second half: everyone who spawned before respawned at his side's new base, 0x44 with round number 2. The
    /// match clock stops for the break: the second half gets what the first half left of it.
    // EVIDENCE: cshell+0x49a40 case 0x46 (cshell+0xcd210 with the received counters while the 0x45 waiting flag is
    // clear); cshell+0xcd210 (class 4 branch)
    // SPEC: GAME_MODES_2009.md §4.6; GAME_MESSAGES_2009_S2C.md §2.1 rows 0x44, 0x46
    // DC-DEFAULT: GMD-7 (proposed) 0x5e after the round result; DSV-147 (proposed) the win team of the half is the
    // side that reached it, the clock paused for the break
    pub(super) fn end_first_half(&mut self, now: Millis) {
        let RoundPhase::Running { start } = self.phase else {
            return;
        };
        let counters = self.half_counters();
        let win = super::round::lead_of(counters);
        self.modes.half_done = true;
        self.modes.half_clock_ms = since(now, start);
        self.modes.swap_pending = true;
        self.end_hold_ms = self.cfg.end_pause_ms.0;
        self.phase = RoundPhase::Over {
            at: now,
            win,
            stage: 0,
            last: false,
        };
        self.drops_clear();
        let pause = self.cfg.end_pause_ms.0.div_ceil(1000);
        self.broadcast(&gs::round_result(
            pause,
            win as i8,
            counters,
            self.rules.mode_class,
        ));
        self.broadcast(&gs::round_wins(counters));
        // DC-DEFAULT: DSV-291 (proposed) the flags go home for the second half (nobody carries one over the break)
        for i in 0..self.ctf.flags.len() {
            let f = &self.ctf.flags[i];
            if f.carrier.is_some() || !f.at_base {
                self.flag_home(i, now);
            }
        }
        self.freeze_at_round_end(now);
        self.log(format!(
            "first half over: team {} {}:{} reached half the goal of {} (win {win}): the sides switch, second              half in {} ms",
            if self.is_flag() { "captures" } else { "kills" },
            counters[0],
            counters[1],
            self.rules.goal,
            self.end_hold_ms.saturating_add(self.cfg.between_round_ms)
        ));
    }

    /// The switch itself: every team value 1 <-> 2 and every per-team counter with it (so each side keeps its own
    /// score), the switch message with the switched scoreboard values, then (with `refresh`) every roster record
    /// with its new team (kind 2). The message goes first: 0x5e flips the client's own team, the own 0x6b then sets
    /// it to the new value, so both together leave it right.
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.3 (0x5e flips the own team only; 0x5f kind 1 `no_swap` 0 the same), §3.2.1
    // effects (an own 0x6b sets the own team; kind 2 sets every record's team, cshell+0x2b820)
    // DC-DEFAULT: GMD-7 (proposed) scores kept; DSV-147 (proposed) the counters follow the sides, message first
    pub(super) fn switch_sides(&mut self, refresh: bool) {
        self.modes.swapped = true;
        self.modes.swap_pending = false;
        for p in self.players.values_mut() {
            if p.team == 1 || p.team == 2 {
                p.team = 3 - p.team;
            }
        }
        self.scores.swap(0, 1);
        self.ctf.captures.swap(0, 1);
        self.round_wins.swap(0, 1);
        self.round_alive_seen.swap(0, 1);
        self.team_seen.swap(0, 1);
        self.team_empty_since.swap(0, 1);
        let shown = self.team_counters();
        let m = match self.cfg.side_switch {
            SideSwitch::ClanStatus => gs::clan_status_scores(shown, false),
            _ => gs::team_swap(shown),
        };
        self.broadcast(&m);
        if refresh {
            let infos: Vec<gs::PlayerInfo> = self
                .players
                .values()
                .filter(|p| p.introduced)
                .map(|p| self.info_of(p, gs::INFO_UPDATE))
                .collect();
            for i in &infos {
                self.broadcast(&gs::player_info(i));
            }
        }
        self.log(format!(
            "sides switched: scoreboard {}:{}",
            shown[0], shown[1]
        ));
    }

    /// The lobby's view of a result after a side switch: the winner and the per-team values in the lobby's team
    /// order (team 1 of the `user` lines), whatever the clients show now.
    // DC-DEFAULT: DSV-149 (proposed) `@result` keeps the lobby's sides (CLN-4 home / away)
    pub(super) fn lobby_sides(&self, win: u8, pair: [u32; 2]) -> (u8, [u32; 2]) {
        if self.modes.swapped {
            let w = if win == 1 || win == 2 { 3 - win } else { win };
            (w, [pair[1], pair[0]])
        } else {
            (win, pair)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The client's half-time test, case by case (cshell+0xcd210).
    #[test]
    fn half_time_picture_follows_the_client() {
        // class 3: exactly half the rounds played (sum of the counters), not when a counter is the limit
        assert!(half_time_picture(3, 6, [2, 1], 0, false));
        assert!(!half_time_picture(3, 6, [2, 0], 0, false));
        assert!(!half_time_picture(3, 6, [2, 2], 0, false));
        assert!(half_time_picture(3, 2, [1, 0], 0, false));
        // class 13: the round number is half the limit
        assert!(half_time_picture(13, 4, [1, 0], 2, false));
        assert!(!half_time_picture(13, 4, [1, 0], 3, false));
        assert!(!half_time_picture(13, 4, [4, 0], 2, false));
        // class 4: first time a counter reaches half
        assert!(half_time_picture(4, 10, [5, 1], 0, false));
        assert!(!half_time_picture(4, 10, [5, 1], 0, true));
        assert!(!half_time_picture(4, 10, [4, 4], 0, false));
        // class 8 once; other classes never
        assert!(half_time_picture(8, 6, [0, 0], 0, false));
        assert!(!half_time_picture(8, 6, [0, 0], 0, true));
        for mc in [1u8, 2, 6, 12] {
            assert!(!half_time_picture(mc, 6, [3, 0], 3, false));
        }
    }
}
