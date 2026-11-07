//! Capture the Flag (mode classes 7 / 8): the two flags of the world file (created by the session as type-0x17 kind-2
//! pickups, DSV-37), taken by touch, carried, captured in the carrier's own capture zone while his own flag is home,
//! dropped where the carrier dies or leaves, returned by touch of a team mate or after a timer; every change goes to
//! everybody as `GS_OBJECTIVE_STATE` 0xc6, the captures are the 0xb5 team counters and the goal.
//!
//! The 2009 client only displays: it has no pickup request (the server decides from positions, GAME_MODES_2009.md
//! §5.5), shows the capture prompt itself (cshell+0x5dad0) and counts captures / returns from the 0xc6 events. The
//! radii, the timer and the order of the tests are DeCombat rules (DSV-192).
// SPEC: GAME_MODES_2009.md §5.5 (flags, zones, 0xc6 field roles and events, 0x7d carrier records, 0xc5 respawn),
// §4.3 (kind-6 capture zones), §4.4 (kind-2 `[state][team][at_base]`); GAME_MESSAGES_2009_S2C.md §6.1 row 0xc6

use super::{Game, GameOut, Life, RoundPhase};
use crate::gs;
use crate::world::{WorldInfo, ZoneKind};
use dc_net_engine::time::since;
use dc_net_engine::Millis;

/// 0xc6 events (PLAUSIBLE meanings from the client's effects, GAME_MODES_2009.md §5.5).
pub const EVENT_CAPTURED: u32 = 1;
pub const EVENT_TAKEN: u32 = 2;
pub const EVENT_RETURNED: u32 = 3;
pub const EVENT_DROPPED: u32 = 4;
/// `carrier` / `actor` of nobody.
pub const NOBODY: u32 = u32::MAX;
/// `state` of a flag lying away from its base: the 2009 top bar draws a flag whose state is 1 on the first team's
/// side and 2 on the second team's (cshell+0x86f10: +0x100 / +0x104 per team) and nothing for any other value.
// DC-DEFAULT: DSV-192 (proposed) state = the team whose side holds the flag (own team at home, the carrier's team
// while carried), 0 while it lies dropped
pub const STATE_DROPPED: u32 = 0;

/// Touch: a player whose centre is within this horizontal distance and height of a flag takes / returns it.
// DC-DEFAULT: DSV-192 (proposed) 64 units across, 96 up or down
pub const TOUCH_RADIUS: f32 = 64.0;
pub const TOUCH_HEIGHT: f32 = 96.0;
/// A dropped flag nobody touched goes home after this time.
// DC-DEFAULT: DSV-192 (proposed) 30 s
pub const RETURN_MS: u32 = 30_000;
/// Without a capture zone of his team the carrier captures within this distance of his own flag's base.
// DC-DEFAULT: DSV-192 (proposed) 128 units
pub const BASE_RADIUS: f32 = 128.0;
/// A capture zone is at least this large in each half extent.
// DC-DEFAULT: DSV-192 (proposed) 48 units
pub const ZONE_MIN_HALF: f32 = 48.0;
/// A carried flag follows its carrier: a move goes out when the carrier moved this far, at most every
/// [`FOLLOW_MS`].
// DC-DEFAULT: DSV-193 (proposed) 16 units, 100 ms
pub const FOLLOW_DIST: f32 = 16.0;
pub const FOLLOW_MS: u32 = 100;
/// A living player this close to a flag (horizontally and up / down) who is not handed it or does not return it is
/// logged once per life and flag, with both distances and the reason: what the server saw of a player who reports
/// that the flag did not react to him.
// DC-DEFAULT: DSV-192 (amendment 2026-11-06, proposed) diagnostic only, twice the touch distances
pub const NEAR_RADIUS: f32 = 2.0 * TOUCH_RADIUS;
pub const NEAR_HEIGHT: f32 = 2.0 * TOUCH_HEIGHT;
/// Height of a flag object above the floor when the world gives none to measure: the ROM world files place their
/// flag objects 18..42 units above the floor below them, most at 28.
// DC-DEFAULT: DSV-252 (proposed)
pub const FLAG_LIFT: f32 = 28.0;

/// A flag ring stands this far above the floor, flat, as the ROM worlds place theirs under their flags.
// OBSERVED: ROM world flag indicators (12 units below a flag that stands 28 above the floor; this rotation)
// DC-DEFAULT: MAP-115 (proposed)
pub const FLAG_RING_LIFT: f32 = 16.0;
pub const FLAG_RING_ROT: [f32; 3] = [std::f32::consts::FRAC_PI_2, 0.0, 0.0];

/// A community map's capture flags get the looping ground ring a ROM world places by hand, in their team's group;
/// it stays while the flag is away, as it marks the base. Returns how many; a ROM world is left alone.
// OBSERVED: ROM world placed effects at every capture flag
// DC-DEFAULT: MAP-115 (proposed; USER REQUEST 2026-11-04 "should have a circle around it on the ground too")
pub fn add_flag_rings(
    world: &mut WorldInfo,
    team_of: impl Fn(Option<&[u8]>) -> Option<u8>,
    team1: &[u8],
    team2: &[u8],
) -> usize {
    let rings: Vec<crate::world::EffectInfo> = world
        .flags
        .iter()
        .filter(|f| f.dc)
        .filter_map(|f| {
            let team = team_of(f.team_text.as_deref())?;
            let fx = match team {
                1 => team1,
                2 => team2,
                _ => return None,
            };
            let y = standing_height(world, f.pos) - FLAG_LIFT + FLAG_RING_LIFT;
            Some(crate::world::EffectInfo {
                name: format!("dc_flag_ring_{team}").into_bytes(),
                pos: [f.pos[0], y, f.pos[2]],
                rot: FLAG_RING_ROT,
                fx: fx.to_vec(),
                looped: true,
                start_on: true,
            })
        })
        .collect();
    let n = rings.len();
    world.effects.extend(rings);
    n
}

/// Where a community map's default flag stands above the floor below `at` (MAP-114).
// DC-DEFAULT: MAP-114 (proposed) a `dc.capture_flag` is placed on the floor, a ROM flag's model is drawn from FLAG_LIFT
// above it
pub fn standing_height(world: &WorldInfo, at: [f32; 3]) -> f32 {
    let from = [at[0], at[1] + 1.0, at[2]];
    let to = [at[0], at[1] - 512.0, at[2]];
    let floor = world
        .trace
        .as_ref()
        .and_then(|t| t.ray(dc_sim::V3::from_array(from), dc_sim::V3::from_array(to)))
        .map_or(at[1], |h| from[1] + (to[1] - from[1]) * h.t.clamp(0.0, 1.0));
    floor + FLAG_LIFT
}

/// One flag on the server.
#[derive(Debug, Clone, PartialEq)]
pub struct FlagSlot {
    pub obj: u16,
    pub team: u8,
    pub base: [f32; 3],
    pub pos: [f32; 3],
    pub at_base: bool,
    pub carrier: Option<u8>,
    pub dropped_at: Option<Millis>,
    sent_at: Option<Millis>,
    /// Height of the flag object above the floor at its world-file base (DSV-252).
    pub lift: f32,
    /// The carrier the clients were last told of (DSV-217).
    shown_carrier: Option<u8>,
    /// The return countdown the clients were last told of, in whole seconds (DSV-290).
    shown_left: u32,
}

/// A team's capture area (axis-aligned box round the zone object).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaptureZone {
    pub team: u8,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl CaptureZone {
    pub fn contains(&self, p: [f32; 3]) -> bool {
        (0..3).all(|i| p[i] >= self.min[i] && p[i] <= self.max[i])
    }
}

/// The flag-mode state of a room.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CtfState {
    pub flags: Vec<FlagSlot>,
    pub zones: Vec<CaptureZone>,
    /// Captures of team 1 and team 2 in this match.
    pub captures: [u32; 2],
    /// Per player: his captures and his returns of the own flag in this match (the board's mission column).
    // DC-DEFAULT: DSV-253 (proposed)
    pub scored: std::collections::BTreeMap<u8, [u32; 2]>,
    /// The near-flag notes already logged: (player, flag index) -> the player's spawn count then (DSV-192).
    near_noted: std::collections::BTreeMap<(u8, usize), u32>,
}

impl CtfState {
    /// The kind-6 capture zones of the world file with a team (the zone object's extent property as half extents).
    // SPEC: GAME_MODES_2009.md §4.3 (kind 6 = flag capture zone, `z_team` the team)
    // DC-DEFAULT: DSV-192 (proposed) the extent property as half extents, at least ZONE_MIN_HALF
    pub fn from_world(world: &WorldInfo, team_of: impl Fn(Option<&[u8]>) -> Option<u8>) -> Self {
        let zones = world
            .zones
            .iter()
            .filter(|z| z.kind == ZoneKind::FlagZone)
            .filter_map(|z| {
                let team = team_of(z.team_text.as_deref())?;
                let h = z.dims.map(|d| d.abs().max(ZONE_MIN_HALF));
                Some(CaptureZone {
                    team,
                    min: [z.pos[0] - h[0], z.pos[1] - h[1], z.pos[2] - h[2]],
                    max: [z.pos[0] + h[0], z.pos[1] + h[1], z.pos[2] + h[2]],
                })
            })
            .collect();
        CtfState {
            zones,
            ..CtfState::default()
        }
    }

    fn flag_of(&self, team: u8) -> Option<&FlagSlot> {
        self.flags.iter().find(|f| f.team == team)
    }

    /// The carrier records of 0x7d: `[flag_team][carrier][carrier_team]` per carried flag.
    pub fn carrier_records(&self, team_of: impl Fn(u8) -> u8) -> Vec<[u32; 3]> {
        self.flags
            .iter()
            .filter_map(|f| {
                let c = f.carrier?;
                Some([u32::from(f.team), u32::from(c), u32::from(team_of(c))])
            })
            .collect()
    }
}

fn touching(p: [f32; 3], f: [f32; 3]) -> bool {
    let dx = p[0] - f[0];
    let dz = p[2] - f[2];
    (dx * dx + dz * dz).sqrt() <= TOUCH_RADIUS && (p[1] - f[1]).abs() <= TOUCH_HEIGHT
}

/// Horizontal distance and height of `p` over `f`.
fn offset(p: [f32; 3], f: [f32; 3]) -> (f32, f32) {
    let dx = p[0] - f[0];
    let dz = p[2] - f[2];
    ((dx * dx + dz * dz).sqrt(), p[1] - f[1])
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

impl Game {
    /// A flag mode room (classes 7 / 8).
    pub fn is_flag(&self) -> bool {
        matches!(self.rules.mode_class, gs::MODE_CLASS_7 | gs::MODE_CLASS_8)
    }

    /// The flag objects the session created: object id, team, base position.
    pub fn bind_flags(&mut self, created: &[(u16, u8, [f32; 3])]) {
        self.ctf.flags = created
            .iter()
            .map(|&(obj, team, base)| FlagSlot {
                obj,
                team,
                base,
                pos: base,
                at_base: true,
                carrier: None,
                dropped_at: None,
                sent_at: None,
                lift: self
                    .floor_below(base)
                    .map_or(FLAG_LIFT, |y| (base[1] - y).clamp(0.0, 64.0)),
                shown_carrier: None,
                shown_left: 0,
            })
            .collect();
    }

    pub fn ctf(&self) -> &CtfState {
        &self.ctf
    }

    /// The 0x6b mission values of a player (`v64`, `v68`, `v6c`): in the flag classes his captures and returns.
    // DC-DEFAULT: DSV-253 (proposed)
    pub(super) fn flag_mission_of(&self, pid: u8) -> [u8; 3] {
        if !self.is_flag() {
            return [0; 3];
        }
        let [c, r] = self.ctf.scored.get(&pid).copied().unwrap_or_default();
        [0, c.min(255) as u8, r.min(255) as u8]
    }

    /// Counts a capture (`k` 0) or a return (`k` 1) for `pid` and sends his roster record again so every board shows
    /// it.
    // DC-DEFAULT: DSV-253 (proposed) a 0x6b refresh after each capture / return by a player
    fn flag_scored(&mut self, pid: u8, k: usize) {
        self.ctf.scored.entry(pid).or_default()[k] += 1;
        let Some(info) = self
            .players
            .get(&pid)
            .map(|p| self.info_of(p, gs::INFO_UPDATE))
        else {
            return;
        };
        self.broadcast(&gs::player_info(&info));
    }

    /// The 0x7d carrier records.
    pub(super) fn flag_carriers(&self) -> Vec<[u32; 3]> {
        self.ctf
            .carrier_records(|pid| self.players.get(&pid).map_or(0, |p| p.team))
    }

    fn flag_event(&mut self, i: usize, event: u32, actor: Option<u8>) {
        let Some(f) = self.ctf.flags.get(i) else {
            return;
        };
        let state = match (f.carrier, f.at_base) {
            (Some(c), _) => self.players.get(&c).map_or(0, |p| u32::from(p.team)),
            (None, true) => u32::from(f.team),
            (None, false) => STATE_DROPPED,
        };
        let m = gs::objective_state(&gs::ObjectiveState {
            obj: f.obj,
            state,
            flag_team: u32::from(f.team),
            carrier: f.carrier.map_or(NOBODY, u32::from),
            actor: actor.map_or(NOBODY, u32::from),
            event,
            at_base: f.at_base,
            quiet: false,
        });
        self.broadcast(&m);
    }

    fn flag_move(&mut self, i: usize, pos: [f32; 3], now: Millis) {
        let Some(f) = self.ctf.flags.get_mut(i) else {
            return;
        };
        f.pos = pos;
        f.sent_at = Some(now);
        let obj = f.obj;
        self.out.push(GameOut::FlagMove { obj, pos });
    }

    /// The floor's height within 512 units below `p` (the world trace), if any.
    pub(super) fn floor_below(&self, p: [f32; 3]) -> Option<f32> {
        let w = self.trace.as_ref()?;
        let from = [p[0], p[1] + 1.0, p[2]];
        let to = [p[0], p[1] - 512.0, p[2]];
        let h = w.ray(dc_sim::V3::from_array(from), dc_sim::V3::from_array(to))?;
        Some(from[1] + (to[1] - from[1]) * h.t.clamp(0.0, 1.0))
    }

    /// Where flag `i` lies when its carrier `pid` drops it at his centre: as high above the floor below him as the
    /// flag stands above the floor at its world-file base (else at his feet plus [`FLAG_LIFT`]).
    // DC-DEFAULT: DSV-252 (proposed, playtest report "flag locations are not flat on the ground": the flag was left at
    // the carrier's centre, about 20 units higher than a flag stands at its base)
    fn flag_ground(&self, i: usize, pid: u8, centre: [f32; 3]) -> [f32; 3] {
        let lift = self.ctf.flags.get(i).map_or(FLAG_LIFT, |f| f.lift);
        let floor = self
            .floor_below(centre)
            .unwrap_or(centre[1] - self.dims_of(pid)[1]);
        [centre[0], floor + lift, centre[2]]
    }

    pub(super) fn flag_home(&mut self, i: usize, now: Millis) {
        let Some(f) = self.ctf.flags.get_mut(i) else {
            return;
        };
        f.carrier = None;
        f.at_base = true;
        f.dropped_at = None;
        let base = f.base;
        self.flag_move(i, base, now);
    }

    /// The flag rules of one tick (classes 7 / 8, round running).
    // DC-DEFAULT: DSV-192 (proposed) the order: lost carriers drop, the timer returns, then each living player in
    // id order takes, returns and captures by touch; DSV-193 (proposed) a carried flag follows its carrier
    pub(super) fn tick_ctf(&mut self, now: Millis) {
        self.tick_ctf_rules(now);
        self.sync_flag_carriers();
        self.tick_flag_countdowns(now);
    }

    /// A dropped flag's seconds to its return, told to every client each time the number changes (0 once it is taken
    /// or home): the clients draw it over the flag's marker.
    // DC-DEFAULT: DSV-290 (proposed) the countdown note the HiSec cases use (0xa6 type 0x17 sub 2, object +0x90) is
    // sent for a dropped flag too, the whole seconds left of RETURN_MS rounded up; the 2009 client shows the number
    // above a flag's marker (cshell+0x86f10), the 2009 server's rule for the number is not known
    fn tick_flag_countdowns(&mut self, now: Millis) {
        for i in 0..self.ctf.flags.len() {
            let f = &self.ctf.flags[i];
            let left = match f.dropped_at {
                Some(t) if f.carrier.is_none() && !f.at_base => {
                    RETURN_MS.saturating_sub(since(now, t)).div_ceil(1000)
                }
                _ => 0,
            };
            if left != f.shown_left {
                let obj = f.obj;
                self.ctf.flags[i].shown_left = left;
                self.out.push(GameOut::FlagNote {
                    obj,
                    remaining: left,
                });
            }
        }
    }

    /// A flag whose carrier changed is attached to the new carrier's body on every client and taken off the last
    /// one (DSV-217).
    // DC-DEFAULT: DSV-217 (proposed)
    fn sync_flag_carriers(&mut self) {
        for f in &mut self.ctf.flags {
            if f.carrier != f.shown_carrier {
                self.out.push(GameOut::FlagCarry {
                    obj: f.obj,
                    team: f.team,
                    carrier: f.carrier,
                    before: f.shown_carrier,
                });
                f.shown_carrier = f.carrier;
            }
        }
    }

    fn tick_ctf_rules(&mut self, now: Millis) {
        if !self.is_flag() || !matches!(self.phase, RoundPhase::Running { .. }) {
            return;
        }
        // a carrier who died or left drops the flag where he was
        for i in 0..self.ctf.flags.len() {
            let Some(c) = self.ctf.flags[i].carrier else {
                continue;
            };
            let at = match self.players.get(&c) {
                Some(p) if p.life == Life::Alive => continue,
                Some(p) => self.flag_ground(i, c, p.pos),
                None => self.ctf.flags[i].pos,
            };
            let f = &mut self.ctf.flags[i];
            f.carrier = None;
            f.at_base = false;
            f.dropped_at = Some(now);
            let team = f.team;
            self.log(format!("ctf: team {team} flag dropped by client {c}"));
            self.flag_move(i, at, now);
            self.flag_event(i, EVENT_DROPPED, Some(c));
        }
        for i in 0..self.ctf.flags.len() {
            let f = &self.ctf.flags[i];
            if f.carrier.is_none()
                && !f.at_base
                && f.dropped_at.is_some_and(|t| since(now, t) >= RETURN_MS)
            {
                let team = f.team;
                self.flag_home(i, now);
                self.log(format!("ctf: team {team} flag returned by the timer"));
                self.flag_event(i, EVENT_RETURNED, None);
            }
        }
        let alive: Vec<(u8, u8, [f32; 3])> = self
            .players
            .values()
            .filter(|p| p.introduced && p.life == Life::Alive)
            .map(|p| (p.pid, p.team, p.pos))
            .collect();
        for (pid, team, pos) in alive {
            for i in 0..self.ctf.flags.len() {
                let f = &self.ctf.flags[i];
                if f.carrier.is_some() {
                    continue;
                }
                if !touching(pos, f.pos) {
                    self.note_near_flag(pid, i, pos, "outside the touch");
                    continue;
                }
                if f.team != team {
                    if self.ctf.flags.iter().any(|g| g.carrier == Some(pid)) {
                        self.note_near_flag(pid, i, pos, "carries a flag already");
                        continue;
                    }
                    let fteam = f.team;
                    let f = &mut self.ctf.flags[i];
                    f.carrier = Some(pid);
                    f.at_base = false;
                    f.dropped_at = None;
                    self.log(format!("ctf: team {fteam} flag taken by client {pid}"));
                    self.flag_event(i, EVENT_TAKEN, Some(pid));
                } else if !f.at_base {
                    self.flag_home(i, now);
                    self.log(format!("ctf: team {team} flag returned by client {pid}"));
                    self.flag_event(i, EVENT_RETURNED, Some(pid));
                    self.flag_scored(pid, 1);
                } else {
                    self.note_near_flag(pid, i, pos, "his own flag, at its base");
                }
            }
            self.try_capture(pid, team, pos, now);
            if !matches!(self.phase, RoundPhase::Running { .. }) {
                return;
            }
        }
        // carried flags follow their carriers
        for i in 0..self.ctf.flags.len() {
            let f = &self.ctf.flags[i];
            let Some(c) = f.carrier else {
                continue;
            };
            let Some(p) = self.players.get(&c).map(|p| p.pos) else {
                continue;
            };
            if dist(p, f.pos) >= FOLLOW_DIST && f.sent_at.is_none_or(|t| since(now, t) >= FOLLOW_MS)
            {
                self.flag_move(i, p, now);
            }
        }
    }

    /// Logs once per life a player within [`NEAR_RADIUS`] / [`NEAR_HEIGHT`] of flag `i` whom the tick's rules did not
    /// hand it or let return it: his team, the flag's, both distances, the reason (DSV-192 amendment, diagnostic).
    // DC-DEFAULT: DSV-192 (amendment 2026-11-06, proposed)
    fn note_near_flag(&mut self, pid: u8, i: usize, pos: [f32; 3], why: &str) {
        let Some(f) = self.ctf.flags.get(i) else {
            return;
        };
        let (across, up) = offset(pos, f.pos);
        if across > NEAR_RADIUS || up.abs() > NEAR_HEIGHT {
            return;
        }
        let Some((team, life)) = self.players.get(&pid).map(|p| (p.team, p.spawns)) else {
            return;
        };
        if self.ctf.near_noted.insert((pid, i), life) == Some(life) {
            return;
        }
        let fteam = f.team;
        let place = if f.at_base { "at its base" } else { "away" };
        self.log(format!(
            "ctf: client {pid} (team {team}) near the team {fteam} flag ({place}): {across:.0} across, {up:+.0} up, \
             not taken: {why}"
        ));
    }

    /// A carrier inside his team's capture zone (or near his flag's base when the world has no zone of his team)
    /// while his own flag is home scores: the enemy flag goes home, the captures go out as 0xb5, the goal ends the
    /// match.
    // SPEC: GAME_MODES_2009.md §5.5 (capture prompt: carrying, own kind-6 zone, own flag at base; the capture is the
    // server's decision); DC-DEFAULT: DSV-192 (proposed)
    fn try_capture(&mut self, pid: u8, team: u8, pos: [f32; 3], now: Millis) {
        let Some(i) = self.ctf.flags.iter().position(|f| f.carrier == Some(pid)) else {
            return;
        };
        let Some(own) = self.ctf.flag_of(team) else {
            return;
        };
        if !own.at_base {
            return;
        }
        let zones: Vec<&CaptureZone> = self.ctf.zones.iter().filter(|z| z.team == team).collect();
        let inside = if zones.is_empty() {
            dist(pos, own.base) <= BASE_RADIUS
        } else {
            zones.iter().any(|z| z.contains(pos))
        };
        if !inside {
            return;
        }
        let Some(k) = (team as usize).checked_sub(1).filter(|k| *k < 2) else {
            return;
        };
        self.ctf.captures[k] += 1;
        let fteam = self.ctf.flags[i].team;
        // the event carries the carrier before the flag goes home (the client clears his carrying mark on it)
        self.ctf.flags[i].at_base = true;
        self.flag_event(i, EVENT_CAPTURED, Some(pid));
        self.flag_home(i, now);
        self.log(format!(
            "ctf: client {pid} captured the team {fteam} flag, captures {}:{}",
            self.ctf.captures[0], self.ctf.captures[1]
        ));
        self.broadcast(&gs::round_wins(self.team_counters()));
        self.flag_scored(pid, 0);
        let goal = self.rules.goal;
        if goal > 0 && self.ctf.captures[k] >= goal {
            self.end_round(now, team);
            return;
        }
        // DC-DEFAULT: DSV-291 (proposed) the clan flag class (8) plays two halves: the first ends when a team's
        // captures reach half the goal (the 2009 client draws its half-time picture on the first round result,
        // cshell+0xcd210), the sides switch and the second half plays to the goal
        if self.clan_halves() && !self.modes.half_done && self.ctf.captures[k] >= (goal / 2).max(1)
        {
            self.end_first_half(now);
        }
    }
}
