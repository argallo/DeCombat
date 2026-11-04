//! The end of a match the 2009 way, after the round result 0x46:
//!
//! 1. 0x46: the announcer plays at once; the scoreboard with the win / lose / draw header shows 3000 ms later
//!    ([`crate::hud::state::RESULT_BOARD_DELAY_MS`]); the world camera shot runs (CLI-240).
//! 2. 0x3f (the server's result screen message): the in-game frame ends. No player update, no respawn request, no
//!    weapon or movement message goes out any more, and the in-game HUD is gone; the result board stays.
//! 3. About 1 s later ([`crate::net::RESULT_HOLD_MS`], CLI-1124; the 2009 client opened its result page at the 0x3f
//!    and left about 8 s later) the client leaves the game server: the lobby location 0x306 {0xb} and the plain
//!    transport bye. The result screen (screen 7, [`crate::menu::result`]) has the focus from then on.
//! 4. The result screen goes back to the waiting room by its exit button or by itself 60 s after it filled its
//!    rows ([`RESULT_AUTO_EXIT_MS`]): the lobby location 0x306 {1}, then 0x388 and 0x38a.
//!
//! [`MatchResult`] is what the result screen shows, taken from the match's HUD inputs (roster records, kill
//! messages, the 0x65 summary rows, 0x46 and 0x3f) by the network side.
// DISASM: cshell+0x4ac5c (0x3f: interface state 5, screen 7 through cshell+0x1bb140), cshell+0x137dd0 (screen 7's
// focus sends the lobby location 0xb), cshell+0x1343b0 (screen 7's frame: 60000 ms after its rows were filled the
// exit command 6), cshell+0x132400 (commands: 6 -> screen 9, the waiting room; 0x3b -> lobby 0x386)
// OBSERVED: capture 20261025_125432_rust6, 20261025_153705_rust7, 20261025_204341_realclient
// (0x3f -> 0x306 {0xb} + bye after 7657 / 8172 / 9266 ms; 0x306 {0xb} -> 0x306 {1}, 0x388, 0x38a after 61547 /
// 62000 ms with nobody touching the screen in rust7 and realclient, 16406 ms after a click in rust6)

use crate::hud::board::LevelTable;
use crate::hud::state::HudState;
use std::time::Instant;

/// The result screen goes back to the waiting room by itself this long after it filled its rows.
// DISASM: cshell+0x1343b0 (`60000 < GetTickCount() - stamp` -> command 6; the stamp is taken in the frame after the
// rows were filled, cshell+0x134b80 sets the fill flag)
pub const RESULT_AUTO_EXIT_MS: u64 = 60_000;
/// Result screen commands: back to the waiting room, leave the room.
// DISASM: cshell+0x132400 (6: screen 9; 0x3b: lobby room leave 0x386, cshell+0x18b380)
pub const RESULT_CMD_EXIT: i32 = 6;
pub const RESULT_CMD_LEAVE_ROOM: i32 = 0x3b;

/// One player row of the result screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultRow {
    pub pid: u8,
    pub name: String,
    pub team: u8,
    pub kills: i32,
    pub deaths: i32,
    pub exp_gain: u32,
    pub gp_gain: u32,
    /// client-scoreboard: the rest of the 0x65 summary (experience before the match, the bonus parts of the two
    /// gains) and the 0x6b rank field.
    pub exp_before: u32,
    pub exp_bonus: u32,
    pub gp_bonus: u32,
    pub rank: u32,
    /// The record's `v64`, `v68`, `v6c`: the mission column (captures / returns in the flag classes).
    pub mission: [u8; 3],
}

/// What the result screen shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MatchResult {
    /// The 0x46 winner (1, 2; 0 = draw).
    pub winner: i8,
    pub own_pid: Option<u8>,
    pub own_team: Option<u8>,
    /// The team scores of the top counters: the 0xb5 pair, or the round wins in the round classes.
    pub score: [u32; 2],
    pub rows: Vec<ResultRow>,
    pub mode_class: Option<u8>,
    /// The 0x3f fields.
    pub flag: bool,
    pub value: u32,
    /// w1-result: the moment the 2009 result screen would fill its rows (its focus, the lobby location 0xb); its
    /// 60 s exit timer counts from here. `None`: from the moment the screen opens.
    pub filled_at: Option<Instant>,
}

/// Mode classes that count round wins instead of kills.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1.2 (the round classes 2, 3, 12, 13 show the 0x46 wins)
const ROUND_CLASSES: [u8; 4] = [2, 3, 12, 13];

/// Mode classes whose result is one ranking list.
// EVIDENCE: cshell+0x137720, cshell+0x134b80 (classes 6, 9, 0xb)
pub const SINGLE_LIST_CLASSES: [u8; 3] = [6, 9, 11];

impl MatchResult {
    /// The result of a match whose result screen 0x3f arrived (`None` before it).
    pub fn from_hud(s: &HudState) -> Option<MatchResult> {
        let screen = s.result_screen?;
        let own_team = s
            .own_pid
            .and_then(|p| s.roster.get(&p))
            .map(|r| r.team)
            .filter(|t| matches!(t, 1 | 2));
        let score = if s.mode_class.is_some_and(|c| ROUND_CLASSES.contains(&c)) {
            s.round_wins
        } else {
            s.team_pair
        };
        let rows = s
            .roster
            .values()
            .map(|r| {
                let sum = s.summary.get(&r.pid);
                ResultRow {
                    pid: r.pid,
                    name: r.name.clone(),
                    team: r.team,
                    kills: r.kills,
                    deaths: r.deaths,
                    exp_gain: sum.map_or(0, |x| x.exp_gain),
                    gp_gain: sum.map_or(0, |x| x.gp_gain),
                    exp_before: sum.map_or(0, |x| x.exp_before),
                    exp_bonus: sum.map_or(0, |x| x.exp_bonus),
                    gp_bonus: sum.map_or(0, |x| x.gp_bonus),
                    rank: s.ranks.get(&r.pid).copied().unwrap_or(0),
                    mission: s.row_marks.get(&r.pid).map_or([0; 3], |m| m.mission),
                }
            })
            .collect();
        Some(MatchResult {
            winner: s.round_winner.unwrap_or(0),
            own_pid: s.own_pid,
            own_team,
            score,
            rows,
            mode_class: s.mode_class,
            flag: screen.flag,
            value: screen.value,
            filled_at: None,
        })
    }

    /// The result with the moment its rows were filled (the lobby location 0xb went out).
    // DISASM: cshell+0x137dd0 (screen 7's focus sends 0xb and fills the rows, cshell+0x137720 -> 0x134b80),
    // cshell+0x1343b0 (the 60000 ms run from the frame after the fill)
    pub fn filled(mut self, at: Instant) -> MatchResult {
        self.filled_at = Some(at);
        self
    }

    /// The team of the left list and of the right one: team 1 (alpha) left, team 2 (bravo) right, whoever won.
    // DC-DEFAULT: CLI-271 (proposed) the result lists by team (the result window's art names alpha on the left and
    // bravo on the right; the 2009 fill of the two lists, cshell+0x137720, was not traced to its team order)
    pub fn sides(&self) -> (u8, u8) {
        (1, 2)
    }

    /// The rows of `team`: most kills first, then fewer deaths, then more experience gained, then the client id.
    // DISASM: cshell+0x135dd0 (the result lists' sort: three mode keys of the record (0x6b fields 10 and 11, field
    // 21 `m2`), then kills greater, deaths smaller, experience gain greater), cshell+0x137720 (team 1 into the left
    // list, other teams > 0 except 0xff into the right one)
    // DC-DEFAULT: CLI-271 (amended, proposed) the three mode keys are 0 in DeCombat's 0x6b (not kept), the client id
    // breaks the remaining ties
    pub fn rows_of(&self, team: u8) -> Vec<&ResultRow> {
        let mut v: Vec<&ResultRow> = self.rows.iter().filter(|r| r.team == team).collect();
        Self::rank(&mut v);
        v
    }

    /// The result is one ranking list (free for all, bounty, infection) instead of two team lists.
    // EVIDENCE: cshell+0x137720 (mode class 6, 9 or 0xb: every record but team 0xff into one sorted list),
    // cshell+0x134b80 (that list's places run on from the first list's count)
    pub fn one_ranking(&self) -> bool {
        self.mode_class
            .is_some_and(|c| SINGLE_LIST_CLASSES.contains(&c))
    }

    /// Every player but the spectators (team 0xff), best first ([`MatchResult::rows_of`]'s order).
    // EVIDENCE: cshell+0x137720 (single list: team != 0xff), cshell+0x137240 (the sort)
    pub fn ranking(&self) -> Vec<&ResultRow> {
        let mut v: Vec<&ResultRow> = self.rows.iter().filter(|r| r.team != 0xff).collect();
        Self::rank(&mut v);
        v
    }

    fn rank(v: &mut [&ResultRow]) {
        v.sort_by(|a, b| {
            b.kills
                .cmp(&a.kills)
                .then(a.deaths.cmp(&b.deaths))
                .then(b.exp_gain.cmp(&a.exp_gain))
                .then(a.pid.cmp(&b.pid))
        });
    }

    pub fn own_row(&self) -> Option<&ResultRow> {
        self.own_pid
            .and_then(|p| self.rows.iter().find(|r| r.pid == p))
    }

    /// The own team won (`None` on a draw or without a team).
    pub fn own_won(&self) -> Option<bool> {
        // a single list: the own player on top won (win mark, win sound); anyone else gets the draw mark and the
        // lose sound
        // EVIDENCE: cshell+0x137720 (single list: first row's name equal to the own name -> the FFA winner mark and
        // sound 0xf, else the FFA draw mark and sound 0x10)
        if self.one_ranking() {
            let top = self.ranking().first().map(|r| r.pid);
            return (top.is_some() && top == self.own_pid).then_some(true);
        }
        match (self.winner, self.own_team) {
            (1 | 2, Some(t)) => Some(self.winner as u8 == t),
            _ => None,
        }
    }

    /// The score of team `t` (1 or 2).
    pub fn score_of(&self, t: u8) -> u32 {
        match t {
            1 => self.score[0],
            2 => self.score[1],
            _ => 0,
        }
    }

    /// A made-up result for previews and tests (no ROM data).
    pub fn sample() -> MatchResult {
        let row =
            |pid: u8, name: &str, team: u8, kills: i32, deaths: i32, exp: u32, gp: u32| ResultRow {
                pid,
                name: name.into(),
                team,
                kills,
                deaths,
                exp_gain: exp,
                gp_gain: gp,
                exp_before: 0,
                exp_bonus: 0,
                gp_bonus: 0,
                rank: 0,
                mission: [0; 3],
            };
        MatchResult {
            winner: 2,
            own_pid: Some(0),
            own_team: Some(1),
            score: [1, 3],
            rows: vec![
                row(0, "Player", 1, 1, 3, 43, 34),
                row(1, "Walker", 1, 0, 1, 12, 10),
                row(2, "Shooter", 2, 3, 0, 95, 66),
                row(3, "Runner", 2, 1, 1, 40, 30),
            ],
            mode_class: Some(1),
            flag: false,
            value: 2,
            filled_at: None,
        }
    }
}

impl ResultRow {
    /// The player's level changed in the match: the level of the experience before it and the level of that plus
    /// the gain differ (the row's level-up mark; for the own row, the level-up window).
    // EVIDENCE: cshell+0x137720 (per record: cshell+0x267f10 of +0x18c + +0x190 against cshell+0x267f10 of +0x18c ->
    // the row's level-up flag; the own row then calls cshell+0x136e40 with the stored experience + gain)
    pub fn levelled_up(&self, levels: &LevelTable) -> bool {
        levels.level_of(self.exp_before.saturating_add(self.exp_gain))
            != levels.level_of(self.exp_before)
    }
}

/// The level text of the MY SCORE window: the progress through the level of `exp` in per cent, two decimals.
// EVIDENCE: cshell+0x136920 (`%3.2f %%` of cshell+0x268040 x 100 / cshell+0x267ff0 for the stored experience + the
// gain + the bonus; 0 when the span is 0), cshell+0x267ff0 (span: the first bound for level 0, else the bound
// difference to the level below; 0 at the level count), cshell+0x268040 (the part: the experience above the level
// below), DISASM cshell+0x136bc2..0x136bfe (the part x 100 as an unsigned 32-bit value rounded to a float, divided
// by the unsigned span)
pub fn level_progress_text(levels: &LevelTable, exp: u32) -> String {
    let b = &levels.bounds;
    let level = levels.level_of(exp) as usize;
    let (into, span) = if level >= b.len() {
        (0, 0)
    } else if level == 0 {
        (exp, b[0])
    } else {
        (
            exp.wrapping_sub(b[level - 1]),
            b[level].wrapping_sub(b[level - 1]),
        )
    };
    let pct = if span == 0 {
        0.0
    } else {
        f64::from(into.wrapping_mul(100) as f32) / f64::from(span)
    };
    format!("{pct:3.2} %")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::state::HudEvent;
    use dc_botclient::shell::{RosterRecord, SummaryRow};

    fn rec(pid: u8, name: &str, team: u8) -> RosterRecord {
        RosterRecord {
            name: name.into(),
            pid,
            team,
            complete: true,
            ..RosterRecord::default()
        }
    }

    #[test]
    fn the_result_comes_from_the_match_inputs_and_only_after_0x3f() {
        let mut s = HudState::default();
        for e in [
            HudEvent::Own {
                pid: Some(0),
                mode_class: Some(1),
            },
            HudEvent::Roster(rec(0, "me", 1)),
            HudEvent::Roster(rec(1, "foe", 2)),
            HudEvent::TeamPair([0, 1]),
            HudEvent::RoundResult {
                round_secs: 0,
                win_team: 2,
                wins: [0, 1],
            },
            HudEvent::Summary(SummaryRow {
                pid: 0,
                exp_gain: 43,
                gp_gain: 34,
                ..SummaryRow::default()
            }),
        ] {
            s.apply(&e, 100, 0.0);
        }
        assert_eq!(MatchResult::from_hud(&s), None, "no result before 0x3f");
        s.apply(
            &HudEvent::ResultScreen {
                flag: false,
                value: 2,
            },
            6100,
            0.0,
        );
        let r = MatchResult::from_hud(&s).expect("result");
        assert_eq!((r.winner, r.own_team, r.score), (2, Some(1), [0, 1]));
        assert_eq!(r.sides(), (1, 2), "alpha left, bravo right");
        assert_eq!(r.own_won(), Some(false));
        let me = r.own_row().unwrap();
        assert_eq!((me.exp_gain, me.gp_gain), (43, 34));
        assert_eq!(r.rows_of(2)[0].name, "foe");
    }

    /// CLI-688: an FFA result (class 6, team values 1..7) ranks every player in one list; the top player wins, the
    /// others get no win.
    #[test]
    fn ffa_result_is_one_ranking_list_of_every_player() {
        let mut r = MatchResult::sample();
        r.mode_class = Some(6);
        for (i, row) in r.rows.iter_mut().enumerate() {
            row.team = i as u8 + 1;
        }
        r.own_pid = Some(2);
        assert!(r.one_ranking());
        let names: Vec<&str> = r.ranking().iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names.len(), r.rows.len(), "{names:?}");
        assert_eq!(names[0], "Shooter");
        assert_eq!(r.own_won(), Some(true));
        r.own_pid = Some(0);
        assert_eq!(r.own_won(), None);
        // a team result keeps its two team lists
        r.mode_class = Some(1);
        assert!(!r.one_ranking());
    }

    #[test]
    fn rows_by_kills_and_no_winner_on_a_draw() {
        let mut r = MatchResult::sample();
        r.winner = 0;
        assert_eq!(r.own_won(), None);
        let s = MatchResult::sample();
        let names: Vec<&str> = s.rows_of(2).iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Shooter", "Runner"]);
    }

    #[test]
    fn players_who_leave_after_0x3f_keep_their_result_rows() {
        // client-scoreboard: the user's round had only the own row (the bots left 3 s after 0x3f, the client 8 s)
        let mut s = HudState::default();
        let mut feed = vec![HudEvent::Own {
            pid: Some(3),
            mode_class: Some(1),
        }];
        for (pid, name, team) in [(0, "bot2", 1), (1, "bot3", 2), (2, "bot1", 2), (3, "me", 1)] {
            feed.push(HudEvent::Roster(rec(pid, name, team)));
            feed.push(HudEvent::Summary(SummaryRow {
                pid,
                exp_before: 1000,
                exp_gain: 40 + u32::from(pid),
                exp_bonus: 4,
                gp_gain: 30,
                ..SummaryRow::default()
            }));
        }
        for e in &feed {
            s.apply(e, 100, 0.0);
        }
        // a departure before 0x3f still removes the row
        s.apply(&HudEvent::Left(0, "bot2".into()), 200, 0.0);
        s.apply(&HudEvent::Roster(rec(0, "bot2", 1)), 300, 0.0);
        s.apply(
            &HudEvent::ResultScreen {
                flag: false,
                value: 1,
            },
            6000,
            0.0,
        );
        for (pid, name) in [(0, "bot2"), (1, "bot3"), (2, "bot1")] {
            s.apply(&HudEvent::Left(pid, name.into()), 9000, 0.0);
        }
        let r = MatchResult::from_hud(&s).expect("result");
        assert_eq!(r.rows.len(), 4, "every player of the match: {:?}", r.rows);
        let me = r.own_row().unwrap();
        assert_eq!((me.exp_before, me.exp_gain, me.exp_bonus), (1000, 43, 4));
        // same kills and deaths: more experience first
        let right: Vec<&str> = r.rows_of(2).iter().map(|x| x.name.as_str()).collect();
        assert_eq!(right, ["bot1", "bot3"]);
    }

    fn table(bounds: &[u32]) -> LevelTable {
        LevelTable {
            bounds: bounds.to_vec(),
            small: Vec::new(),
        }
    }

    #[test]
    fn the_level_text_is_the_progress_through_the_level_in_per_cent() {
        // w1-result: cshell+0x136920 `%3.2f %%` of the part over the span of the level
        let t = table(&[100, 300, 600]);
        assert_eq!(level_progress_text(&t, 0), "0.00 %");
        assert_eq!(level_progress_text(&t, 50), "50.00 %");
        assert_eq!(level_progress_text(&t, 100), "0.00 %");
        assert_eq!(level_progress_text(&t, 250), "75.00 %");
        assert_eq!(level_progress_text(&t, 401), "33.67 %");
        // past the last bound the level reads 0 (cshell+0x267f10), as in 2009
        assert_eq!(level_progress_text(&t, 700), "700.00 %");
        assert_eq!(level_progress_text(&table(&[]), 5), "0.00 %");
    }

    #[test]
    fn a_row_levelled_up_when_its_gain_crossed_a_bound() {
        let t = table(&[100, 300]);
        let mut r = MatchResult::sample().rows[0].clone();
        (r.exp_before, r.exp_gain) = (90, 9);
        assert!(!r.levelled_up(&t));
        r.exp_gain = 10;
        assert!(r.levelled_up(&t));
        assert!(MatchResult::sample().filled_at.is_none());
        let at = Instant::now();
        assert_eq!(MatchResult::sample().filled(at).filled_at, Some(at));
    }
}
