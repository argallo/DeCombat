//! The round end: by goal (from the kill), by the timer, or when a team has had no players for a while; the end set
//! with the reference stack's timing (`GS_ROUND_RESULT` at once, the summary set after a pause, the result screen after
//! another; a time-limit end sends the summary set with the result: `timeout_hold_ms`), the pipe report, and the
//! wait for the clients to leave (no server bye, no `GS_LEVEL_EXIT`).
//!
//! The round modes (classes 2, 3, 11, 12, 13: no respawn inside a round) play several rounds per match (GMD-20): a
//! round also ends when a team that had a living player has none left (GMD-22); a round that does not decide the match
//! is followed by the between-round set and, after the between-round pause, the next round with everyone respawned
//! and the breakables restored (GMD-21, GMD-24); the match ends when a side reaches the goal in round wins (GMD-5).
// SPEC: GAME_MESSAGES_2009_C2S.md Appendix A steps A12, A13; GAME_MESSAGES_2009_S2C.md §2.1, §3.4, §10 steps 19-21;
// REFERENCE_FINDINGS_botduel.md F7; CAPTURE_tdm4_KILL_VALIDATION.md §6; GAME_MODES_2009.md §4.1, §4.2, §5.2, §5.7, §5.8

use super::{Game, GameOut, Life, RoundPhase, TODO_SUMMARY_GAINS};
use crate::gs;
use dc_net_engine::time::since;
use dc_net_engine::Millis;

/// `GS_ROUND_OBJECTS_END` value (read and dropped by the client; the reference stack sent 0).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1 row 0x47 (`unused`); REFERENCE_FINDINGS_botduel.md F7 (`0x47 [0]`)
pub const ROUND_OBJECTS_END_VALUE: u32 = 0;
/// `GS_MATCH_RESULT_SCREEN` `result_flag` (meaning UNPROVEN): 0 in every reference result screen.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (0x3f); REFERENCE_FINDINGS_botduel.md F7 (`[0][win]`); SPEC: UNPROVEN meaning
pub const TODO_RESULT_FLAG: bool = false;

fn secs(ms: u32) -> u32 {
    ms.div_ceil(1000)
}

impl Game {
    /// Round end: `GS_ROUND_RESULT` `[pause in seconds][win team][round wins before this round]` to everyone at
    /// once, then `@player` / `@result` (the lobby computes the rewards and may send them back with `gain` lines
    /// before the summary). Never `GS_ROUND_TIMER_STOP` 0x41 or `GS_LEVEL_EXIT` 0x99 (the reference stack sent neither).
    // SPEC: REFERENCE_FINDINGS_botduel.md F7 (`0d4603000000010000` = [3][1][0][0] in the goal kill's tick; REFERENCE);
    // GAME_MESSAGES_2009_S2C.md §2.1.2
    // DC-DEFAULT: DC-C2S-13 (proposed) goal or timer, draw on equal score; DSV-31 (proposed) emptied team; DSV-35
    // (proposed) the end set
    pub(super) fn end_round(&mut self, now: Millis, win: u8) {
        self.end_round_with(now, win, self.cfg.end_pause_ms.0, false);
    }

    /// [`Game::end_round`] with the first pause given: `end_pause_ms.0` for a goal or forfeit end, `timeout_hold_ms`
    /// for a time-limit end. The wire values of 0x46 and 0x48 stay the reference stack's (`end_pause_ms.0` in seconds); only
    /// the pause before the summary set changes.
    ///
    /// In a round mode the round's end either decides the match (a side reached the goal in round wins, the round
    /// cap, or `forfeit`: a team left the match) or the next round follows (`last` false, GMD-21); 0xb5 carries the
    /// round wins after this round. Only the match's end reports to the lobby.
    // project decision (the scoreboard as soon as the clock runs out); DC-DEFAULT: DSV-35 (amendment, proposed)
    // DC-DEFAULT: GMD-5 (proposed) the match ends at the goal in rounds; GMD-21, GMD-23 (staged)
    pub(super) fn end_round_with(
        &mut self,
        now: Millis,
        win: u8,
        first_hold_ms: u32,
        forfeit: bool,
    ) {
        if !matches!(self.phase, RoundPhase::Running { .. }) {
            return;
        }
        self.end_hold_ms = first_hold_ms;
        let wins_before = self.round_wins;
        // DC-DEFAULT: DSV-148 (proposed) no team round win in FFA (the winner's value is a player's)
        if (win == 1 || win == 2) && !self.is_ffa() {
            self.round_wins[(win - 1) as usize] += 1;
        }
        let looped = self.round_loop();
        // DC-DEFAULT: DSV-567 (proposed; USER DECISION 2026-11-04) the warm-up's end takes this path too
        // (`warmup::tick_warmup`): never the match's end, no side switch
        let warmup_end = self.in_warmup();
        let last = !looped || forfeit || (!warmup_end && self.match_decided());
        // the 0x3f winner and the lobby's result: the side with more round wins in a round mode (a forfeit goes to
        // the side that stayed)
        let match_win = if looped && !forfeit {
            lead_of(self.round_wins)
        } else {
            win
        };
        self.phase = RoundPhase::Over {
            at: now,
            win: if last { match_win } else { win },
            stage: 0,
            last,
        };
        // DC-DEFAULT: DSV-95 (proposed) the round end removes every dropped weapon
        self.drops_clear();
        // DC-DEFAULT: DSV-361 (proposed) a bomb still planted at the round end is cancelled
        self.bomb_cancel_at_round_end(now);
        let mc = self.rules.mode_class;
        let pause = secs(self.cfg.end_pause_ms.0);
        if self.is_coop() {
            // SPEC: FIRETEAM_2009.md §2.6 (class-5 block: stage banner and six values), §2.2 row 4 (timer frozen)
            // DC-DEFAULT: FT-15 (proposed) 0xd7 sub 4 [0][0] at the end
            let c = self.coop_result(now);
            self.broadcast(&gs::coop_round_clock(false, 0));
            self.broadcast(&gs::round_result_coop(pause, win as i8, wins_before, &c));
        } else {
            // DC-DEFAULT: DSV-147 (proposed) the clan TDM halves carry the team kills in the win counters, so the
            // client's match-over guard (a counter equal to `round_limit` = goal) keeps the picture off at the goal
            // EVIDENCE: cshell+0xcd210 (`limit == wins1 || limit == wins2`: no picture)
            // DC-DEFAULT: DSV-400 (proposed; amends DSV-35 / GMD-21, playtest report "S&D didn't update the scoreboard
            // when the bomb detonates but still counted to the win limit") a round mode's 0x46 carries the round wins
            // AFTER this round: the 2009 client's top counters of classes 2, 3, 12, 13 take only the 0x46 pair
            // (cshell+0x4a608..0x4aa42: `hud_scoreboard`+0x138 / +0x13c), and its half-time check treats a counter
            // equal to the round limit as the match's end (cshell+0xcd210 `nMaxRound == nJSFWinCount ||
            // nMaxRound == nCreedWinCount`: no picture), which only the deciding round's after-value can reach
            // DISASM cshell+0xcd210 (FUN_371cd210 "ShowTeamChangeSprite nMaxRound, nHalfRound, nJSFWinCount,
            // nCreedWinCount"); single-round classes keep the before-value of DSV-35
            let sent = if self.clan_halves() {
                self.half_counters()
            } else if looped {
                self.round_wins
            } else {
                wins_before
            };
            if self.is_spy() {
                // DC-DEFAULT: DSV-280 (proposed) one win value per receiver in class 9 (see `spy_result_for`)
                let leader = self.spy_leader_pid();
                for pid in self.introduced() {
                    let w = self.spy_result_for(pid, leader);
                    self.send(pid, gs::round_result(pause, w, sent, mc));
                }
            } else {
                self.broadcast(&gs::round_result(pause, win as i8, sent, mc));
            }
        }
        if looped {
            // SPEC: GAME_MODES_2009.md §4.1 (0xb5 round wins after each round; PLAUSIBLE, V-GM-1)
            // DC-DEFAULT: GMD-21 (staged) 0xb5 with the round wins right after 0x46
            self.broadcast(&gs::round_wins(self.round_wins));
            if !last && !warmup_end {
                // DC-DEFAULT: GMD-7 (proposed), DSV-147 (proposed) the half-time switch of 3 / 13; DSV-400: tested on
                // the pair the 0x46 just carried (the round wins after this round)
                self.side_switch_at_round_end(self.round_wins);
            }
        }
        // project decision; DC-DEFAULT: DSV-77 (reinstated, proposed) the players stop with the result
        self.freeze_at_round_end(now);
        self.log(format!(
            "round {} over: win {win}, score {}:{}, round wins {}:{}{}",
            self.round_no.max(1),
            self.scores[0],
            self.scores[1],
            self.round_wins[0],
            self.round_wins[1],
            match (looped, last) {
                (false, _) => String::new(),
                (true, true) => format!(", match over (win {match_win})"),
                (true, false) => format!(
                    ", next round in {} ms",
                    first_hold_ms.saturating_add(self.cfg.between_round_ms)
                ),
            }
        ));
        // DC-DEFAULT: DSV-42 (proposed) the hit verdict counters at every round end
        let hits = self.hit_summary();
        self.log(hits);
        // DC-DEFAULT: DSV-102 (proposed) the shot-direction counters at every round end
        let cone = self.cone_summary();
        self.log(cone);
        if last {
            self.report(match_win);
        }
    }

    /// Whether the round wins so far end a round-mode match: a side reached the goal (0 = one round), or the round
    /// cap (`max_rounds`, default twice the goal) was reached; Quarantine: the goal in rounds played (DSV-565).
    // DC-DEFAULT: GMD-5 (proposed) the goal in rounds; GMD-23 (staged) the cap
    fn match_decided(&self) -> bool {
        let goal = self.rules.goal;
        // DC-DEFAULT: DSV-565 (proposed; playtest report "Quarantine went to 8 rounds even though it was out of 7
        // rounds, because humans won a round") Quarantine plays exactly `goal` rounds, whoever wins each one: its
        // HUD draws the goal as the round limit beside the current round (round / limit with the round word), not
        // as a win target
        // EVIDENCE: cshell+0xb29c0 (class-11 element: current round and round limit digits, slash, round word),
        // cshell+0xb0e30 / cshell+0xb0f70 (the two counters set by 0xd3 sub 1 and 0x44 with round_no / round_limit)
        if self.is_infect() {
            return goal == 0 || self.round_no >= goal;
        }
        if goal == 0 || self.round_wins.iter().any(|&w| w >= goal) {
            return true;
        }
        let cap = self
            .cfg
            .max_rounds
            .unwrap_or_else(|| goal.saturating_mul(2))
            .max(1);
        self.round_no >= cap
    }

    /// The living introduced players of team 1 and team 2; marks the teams that had one in this round.
    // DC-DEFAULT: GMD-22 (staged) alive = introduced and in life state Alive
    fn alive_counts(&mut self) -> [u32; 2] {
        let mut n = [0u32; 2];
        for p in self.players.values() {
            if !p.introduced || p.life != Life::Alive {
                continue;
            }
            if let Some(i) = (p.team as usize).checked_sub(1).filter(|i| *i < 2) {
                n[i] += 1;
            }
        }
        for (seen, &k) in self.round_alive_seen.iter_mut().zip(&n) {
            *seen |= k > 0;
        }
        n
    }

    /// GMD-22: in a round mode a team that had a living player in this round and has none left loses the round; the
    /// other team wins when it had a living player in this round, both out (or the other never alive) is a draw.
    // SPEC: GAME_MODES_2009.md §5.2 (all enemies dead), §5.7 (no human left), §5.8 (the other team has no living
    // player); EVIDENCE: cshell+0x61e40 (no respawn request in these classes, so a dead player stays out)
    // DC-DEFAULT: GMD-9, GMD-12, GMD-13 (proposed); GMD-22 (staged)
    pub(super) fn check_elimination(&mut self, now: Millis) {
        if !self.round_loop() || !matches!(self.phase, RoundPhase::Running { .. }) {
            return;
        }
        // DC-DEFAULT: DSV-512 (proposed) the warm-up never ends by elimination
        if self.in_warmup() {
            return;
        }
        let alive = self.alive_counts();
        let out = [0, 1].map(|i| self.round_alive_seen[i] && alive[i] == 0);
        // DC-DEFAULT: GMD-9 (proposed) a planted bomb still has to be defused: the attackers being out does not end
        // the round (both out: the bomb decides); DSV-203 (proposed)
        if out[0] && self.bomb_holds_round() {
            return;
        }
        // DC-DEFAULT: DSV-213 (amended 2026-11-03, proposed) Quarantine: the dead infected stay out, so both sides can
        // run out: no human left = the infected win, every infected dead after the outbreak = the humans win
        let win = match out {
            [false, false] => return,
            [true, true] => 0,
            [true, false] => {
                if self.round_alive_seen[1] {
                    2
                } else {
                    0
                }
            }
            [false, true] => {
                if self.round_alive_seen[0] {
                    1
                } else {
                    0
                }
            }
        };
        self.log(format!(
            "round {}: team(s) {} without a living player: win {win}",
            self.round_no,
            match out {
                [true, true] => "1 and 2",
                [true, false] => "1",
                _ => "2",
            }
        ));
        self.end_round(now, win);
    }

    /// The winner when the round clock runs out: the leading side in team kills (single-round classes); in the round
    /// modes the defenders (team 2) in 2 / 3, the humans (team 1) in 11 while one lives, else the side with more
    /// living players (equal = draw).
    // DC-DEFAULT: DC-C2S-13 (proposed) score, draw on equal; GMD-9 (defenders on time), GMD-12 (humans survive),
    // GMD-13 (more living players) (proposed); GMD-22 (staged)
    fn timeout_winner(&mut self) -> u8 {
        // DC-DEFAULT: DSV-148 (proposed) FFA: the player with the most kills
        if self.is_ffa() {
            return self.ffa_leader();
        }
        // DC-DEFAULT: DSV-235 (proposed) Spy Hunt: the player with the most points
        if self.is_spy() {
            return self.spy_leader();
        }
        // DC-DEFAULT: DSV-192 (proposed) the flag modes: more captures, then more team kills, else a draw
        if self.is_flag() {
            return match lead_of(self.ctf.captures) {
                0 => lead_of(self.scores),
                w => w,
            };
        }
        if !self.round_loop() {
            return lead_of(self.scores);
        }
        let alive = self.alive_counts();
        match self.rules.mode_class {
            2 | 3 => 2,
            11 => {
                if alive[0] > 0 {
                    1
                } else {
                    2
                }
            }
            _ => lead_of(alive),
        }
    }

    /// The between-round set after the first pause (GMD-21): `GS_ROUND_OBJECTS_END` [0], every roster record refreshed
    /// (kind 2), `GS_ROUND_CLOSE` with the between-round seconds (the client's banner timer); no `GS_PLAYER_SUMMARY`
    /// and no result screen.
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.1 rows 0x47 (objective layers off), 0x48 (`hud_banner` timed state with
    // `round_secs`, timer hidden); GAME_MODES_2009.md §4.1 (round close between rounds, objectives off after each
    // round; PLAUSIBLE, V-GM-1)
    // DC-DEFAULT: GMD-5 (proposed) 5 s; GMD-21 (staged) the order of the match-end set without 0x65
    fn between_set(&mut self) {
        self.broadcast(&gs::round_objects_end(ROUND_OBJECTS_END_VALUE));
        // DC-DEFAULT: GMD-7 (proposed), DSV-147 (proposed) the switch message before the roster refresh, which then
        // carries every player's new team
        if self.modes.swap_pending {
            self.switch_sides(false);
        }
        let infos: Vec<gs::PlayerInfo> = self
            .players
            .values()
            .filter(|p| p.introduced)
            .map(|p| self.info_of(p, gs::INFO_UPDATE))
            .collect();
        for i in &infos {
            self.broadcast(&gs::player_info(i));
        }
        self.broadcast(&gs::round_close(secs(self.cfg.between_round_ms)));
    }

    /// The next round of a round mode (GMD-20, GMD-21, GMD-24): the round number up, the clock from now, the breakables
    /// restored, every player who has spawned before respawned (spawn set, the release of the round-end freeze first),
    /// then `GS_ROUND_BEGIN` `[round_secs][round_no][round_limit][keep_team 0]` and `GS_ROUND_WINS` to each of them.
    /// No `GS_MATCH_BEGIN` (once per match) and no `GS_SCORES_CLEAR`.
    // SPEC: GAME_MODES_2009.md §4.1 (0x44 once per round, everybody respawns at the next 0x44), §4.2 (the dead player
    // observes until the next round's spawn); GAME_MESSAGES_2009_S2C.md §2.1 row 0x44 (layout PROVEN, per-round use
    // PLAUSIBLE, V-GM-1)
    // DC-DEFAULT: GMD-5 (proposed); GMD-21 (staged) the order: spawn sets, then 0x44 / 0xb5 per player
    /// A new round of a round mode (GMD-24): every breakable has its full hit points again, the death effects still
    /// on the clients go (each one's removal as when its time is up), and the dead ones are kept for the session,
    /// which switches their models on again on every client (`take_restored_breakables`).
    // OBSERVED: the ROM worlds' state save / state restore statements in the mode controller and start triggers
    // (hashes only; world-objects.md §1.2: they restore the breakables at a round start)
    // DC-DEFAULT: GMD-24 (staged) `destruct.restore` on
    fn restore_breakables(&mut self, now: Millis) {
        if !self.cfg.destruct.restore {
            return;
        }
        let mut back = 0;
        for b in &mut self.destruct.items {
            if b.dead {
                back += 1;
                if !self.restored_breakables.contains(&b.obj) {
                    self.restored_breakables.push(b.obj);
                }
            }
            b.dead = false;
            b.left = b.hit_points;
        }
        // every effect ends no later than effect_ms from now: the regular expiry removes them all at once
        self.tick_destruct(now.wrapping_add(self.cfg.destruct.effect_ms));
        if back > 0 {
            self.log(format!(
                "destructibles: {back} breakable(s) restored for the round"
            ));
        }
    }

    pub(super) fn next_round(&mut self, now: Millis) {
        self.round_no = self.round_no.saturating_add(1);
        // DC-DEFAULT: DSV-147 (proposed) the clan TDM second half gets what the first half left of the match clock
        let start = if self.clan_halves() {
            now.wrapping_sub(self.modes.half_clock_ms)
        } else {
            now
        };
        self.phase = RoundPhase::Running { start };
        self.round_alive_seen = [false; 2];
        self.team_empty_since = [None; 2];
        self.log(format!(
            "round {} started (mode class {}, round wins {}:{}, {} s)",
            self.round_no,
            self.rules.mode_class,
            self.round_wins[0],
            self.round_wins[1],
            self.rules.round_secs
        ));
        // DC-DEFAULT: GMD-24 (staged) the world's breakables come back at each round start
        self.restore_breakables(now);
        // DC-DEFAULT: DSV-210 (proposed) the infected are humans again before the spawn set; DSV-511 (proposed) the
        // countdown follows each player's 0x44 below
        self.infect_round_start(now, false);
        let pids: Vec<u8> = self
            .players
            .values()
            .filter(|p| p.introduced && p.spawns > 0)
            .map(|p| p.pid)
            .collect();
        for &pid in &pids {
            self.spawn(pid, now);
        }
        let left = self.round_secs_left(now);
        let (no, limit) = (self.round_no, self.round_begin_limit());
        let counters = self.team_counters();
        // DC-DEFAULT: DSV-243 (proposed, playtest report "round start audio is not played each new round") every round
        // of a round mode also gets `GS_MATCH_BEGIN` without the intro and with the waiting banner off, after the
        // spawn set: the 2009 client plays its round-start voice only in the 0x45 handler (own life state 1), the
        // 0x44 handler plays none
        // EVIDENCE: cshell+0x49a40 case 0x45 (cshell+0x49f00..0x49f7b: the round-start voice when alive and the
        // waiting flag is clear), case 0x44 (resets only, no sound call)
        let mb = gs::MatchBegin {
            round_secs: left,
            waiting_banner: false,
            countdown_secs: 0,
            intro: None,
            round_no: no,
        };
        let mc = self.rules.mode_class;
        // DC-DEFAULT: DSV-321 (proposed, playtest report "in quarantine the audio for round start is not needed")
        // Quarantine rounds after the first get no 0x45: the 2009 0x45 voice has no class test (cshell+0x49f00), so
        // the server's choice decides; the class-11 round cue is the 0xd3 sub 1 intro voice and the countdown voices
        // (cshell+0xb1190, cshell+0xb27b0)
        let per_round_begin = mc != gs::MODE_CLASS_11;
        for &pid in &pids {
            self.send(pid, gs::round_begin(left, no, limit, false));
            self.send(pid, gs::round_wins(counters));
            if per_round_begin {
                self.send(pid, gs::match_begin(&mb, mc));
            }
        }
        // DC-DEFAULT: DSV-511 (proposed) the outbreak countdown after the round-start messages, to every player in
        // the world (also the ones who never spawned yet)
        for pid in self.introduced() {
            self.infect_countdown_to(pid, now);
        }
        self.alive_counts();
    }

    /// The summary set after the first pause: `GS_ROUND_OBJECTS_END`, every player's roster record refreshed (kind
    /// 2, absolute kills and deaths), one `GS_PLAYER_SUMMARY` per player (its character object, the experience
    /// before the match, the lobby's gains when they arrived, else 0), `GS_ROUND_CLOSE`.
    // SPEC: REFERENCE_FINDINGS_botduel.md F7 (0x47 [0], 0x6b x 2, 0x65 x 2, 0x48 [3]; REFERENCE); CAPTURE_tdm2 §2.5 (0x65 with
    // the character `objref16`); GAME_MESSAGES_2009_S2C.md §3.4
    // DC-DEFAULT: DC-KILL-5 (proposed) the refresh before 0x65; CD-54 (approved) the lobby's gains; DSV-35 (proposed)
    fn summary_set(&mut self) {
        let mc = self.rules.mode_class;
        self.broadcast(&gs::round_objects_end(ROUND_OBJECTS_END_VALUE));
        let infos: Vec<gs::PlayerInfo> = self
            .players
            .values()
            .filter(|p| p.introduced)
            .map(|p| self.info_of(p, gs::INFO_UPDATE))
            .collect();
        for i in &infos {
            self.broadcast(&gs::player_info(i));
        }
        let rows: Vec<gs::Summary> = self
            .players
            .values()
            .filter(|p| p.introduced)
            .map(|p| {
                let gain = p.uid.and_then(|u| self.gains.get(&u)).copied();
                gs::Summary {
                    pid: p.pid,
                    obj: Some(p.obj),
                    exp_before: p
                        .roster
                        .as_ref()
                        .map_or(0, |e| e.exp.clamp(0, u32::MAX as i64) as u32),
                    // TODO: without a `gain` line from the lobby the summary shows 0 (TODO_SUMMARY_GAINS)
                    exp_gain: gain.map_or(TODO_SUMMARY_GAINS, |g| g.0),
                    // DC-DEFAULT: DSV-142 (proposed) the lobby's bonus pair
                    exp_bonus: gain.map_or(0, |g| g.2),
                    gp_gain: gain.map_or(TODO_SUMMARY_GAINS, |g| g.1),
                    gp_bonus: gain.map_or(0, |g| g.3),
                    coop: self.coop_summary(p.pid),
                }
            })
            .collect();
        for r in &rows {
            self.broadcast(&gs::player_summary(r, mc));
        }
        self.broadcast(&gs::round_close(secs(self.cfg.end_pause_ms.0)));
    }

    /// The round clock, the team-empty rule and the end set's later stages; after the result screen the session
    /// ends once every client has left (or the linger limit passed).
    // DC-DEFAULT: DC-C2S-13 (proposed) timer, draw on equal score; DSV-31 (proposed); DSV-35 (proposed)
    pub(super) fn tick_round(&mut self, now: Millis) {
        if let RoundPhase::Running { start } = self.phase {
            let secs = self.rules.round_secs;
            if secs > 0 && since(now, start) >= secs.saturating_mul(1000) && self.is_coop() {
                // DC-DEFAULT: FT-15 (proposed) the mission timer ran out: the time-out banner
                self.coop_finish(now, super::coop::STAGE_TIMEOUT);
            } else if secs > 0
                && since(now, start) >= secs.saturating_mul(1000)
                // DC-DEFAULT: DSV-203 (proposed) the round clock stops at the plant (the bomb timer decides)
                && !self.bomb_holds_round()
                // DC-DEFAULT: DSV-512 (proposed) the warm-up ends by its own time only
                && !self.in_warmup()
            {
                let win = self.timeout_winner();
                // project decision: the scoreboard set follows the result after `timeout_hold_ms` (0: the
                // same tick) when the clock ran out; DC-DEFAULT: DSV-35 (amendment, proposed)
                let hold = self.cfg.timeout_hold_ms;
                self.end_round_with(now, win, hold, false);
            }
        }
        // DC-DEFAULT: GMD-22 (staged) a leave or any death outside a kill can empty a team too
        self.check_elimination(now);
        if matches!(self.phase, RoundPhase::Running { .. }) && !self.is_coop() {
            if self.is_ffa() {
                self.tick_ffa_alone(now);
            } else if !self.is_infect() && !self.is_spy() && !self.in_warmup() {
                // DC-DEFAULT: DSV-512 (proposed) no team-empty forfeit while the players load in the warm-up
                // DC-DEFAULT: DSV-236 (proposed) Spy Hunt: an empty spy side is the normal state
                // DC-DEFAULT: DSV-213 (proposed) Quarantine: an empty infected side is the normal round start
                self.tick_team_empty(now);
            }
        }
        // DC-DEFAULT: DSV-77 (reinstated, proposed) an airborne player is frozen when his landing wait ran out
        self.tick_freeze_wait(now);
        let (p1, p2) = (self.end_hold_ms, self.cfg.end_pause_ms.1);
        if let RoundPhase::Over {
            at,
            win,
            stage,
            last: false,
        } = self.phase
        {
            if self.players.is_empty() {
                self.log("every client left between two rounds".into());
                self.close();
                return;
            }
            let gone = since(now, at);
            if stage == 0 && gone >= p1 {
                self.between_set();
                self.phase = RoundPhase::Over {
                    at,
                    win,
                    stage: 1,
                    last: false,
                };
            } else if stage == 1 && gone >= p1.saturating_add(self.cfg.between_round_ms) {
                self.next_round(now);
            }
            return;
        }
        if let RoundPhase::Over { at, win, stage, .. } = self.phase {
            if self.players.is_empty() {
                // every client left: nothing more to show
                self.log("every client left after the round end".into());
                self.close();
                return;
            }
            let gone = since(now, at);
            if stage == 0 && gone >= p1 {
                self.summary_set();
                self.phase = RoundPhase::Over {
                    at,
                    win,
                    stage: 1,
                    last: true,
                };
            } else if stage == 1 && gone >= p1.saturating_add(p2) {
                // SPEC: REFERENCE_FINDINGS_botduel.md F7 (`0d3f0200000000` = [0][winning team]; REFERENCE)
                self.broadcast(&gs::result_screen(TODO_RESULT_FLAG, u32::from(win)));
                self.phase = RoundPhase::Over {
                    at,
                    win,
                    stage: 2,
                    last: true,
                };
            } else if stage == 2
                && self.cfg.linger_ms > 0
                && gone >= p1.saturating_add(p2).saturating_add(self.cfg.linger_ms)
            {
                self.log(format!(
                    "clients still linked {} s after the result screen: ending the session",
                    self.cfg.linger_ms / 1000
                ));
                self.close();
            }
        }
    }

    fn close(&mut self) {
        self.phase = RoundPhase::Closed;
        self.out.push(GameOut::Finish);
    }

    /// DSV-31: a team that had a player in this round and has had none for `team_empty_ms` loses the round to the
    /// other team (when that one still has players); in a round mode it loses the match (GMD-22).
    // DC-DEFAULT: DSV-31 (proposed); SPEC: CAPTURE_tdm4_KILL_VALIDATION.md §6 (the reference stack ended 0.344 s after the
    // only team-1 player left, `win_team` 2; the delay is ours)
    fn tick_team_empty(&mut self, now: Millis) {
        let mut present = [false; 2];
        for p in self.players.values().filter(|p| p.introduced) {
            if let Some(i) = (p.team as usize).checked_sub(1).filter(|i| *i < 2) {
                present[i] = true;
                self.team_seen[i] = true;
            }
        }
        if self.cfg.team_empty_ms == 0 {
            return;
        }
        for i in 0..2 {
            if present[i] || !self.team_seen[i] {
                self.team_empty_since[i] = None;
                continue;
            }
            let since_t = *self.team_empty_since[i].get_or_insert(now);
            let other = 1 - i;
            if present[other] && since(now, since_t) >= self.cfg.team_empty_ms {
                self.log(format!(
                    "team {} has had no players for {} ms: team {} wins",
                    i + 1,
                    since(now, since_t),
                    other + 1
                ));
                // DC-DEFAULT: GMD-22 (staged) in a round mode the emptied team forfeits the match
                let hold = self.cfg.end_pause_ms.0;
                self.end_round_with(now, other as u8 + 1, hold, true);
                return;
            }
        }
    }
}

/// The side with the larger value: 1, 2, or 0 when equal.
pub(super) fn lead_of(v: [u32; 2]) -> u8 {
    match v[0].cmp(&v[1]) {
        std::cmp::Ordering::Greater => 1,
        std::cmp::Ordering::Less => 2,
        std::cmp::Ordering::Equal => 0,
    }
}
