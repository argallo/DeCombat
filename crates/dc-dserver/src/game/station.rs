//! The Spy Hunt upload station (stage 2): the world's scripts around the uplink box, run by a small statement
//! interpreter. The mode file names the trigger that brings the station when a player becomes the agent; that
//! trigger's commands switch the station's effects on and, after their delays, unlock the uplink box. The agent
//! working the unlocked box for its time runs the box's activate command, which fires the finish trigger and tells
//! the mode controller that the defence (the agent's side) won.
//!
//! Only the statements the stage-2 scripts of the ROM worlds use are run: switching an effect on or off (`on`,
//! `off`, `remove`, `visible 0 / 1`), firing a trigger, locking and unlocking a box, the defence win; each may be
//! delayed. The station's solid / kill boxes and other statements are skipped. Statement words are compared by hash
//! (no ROM text in the source).
// OBSERVED: ROM world WARHEAD (modes-r13 survey: the activation box `Activate_TRG01` starts locked, its work /
// release / activate commands; the trigger the GMS `[AgentModelSetting] AgentTrigger` names switches the station
// model and particles on and, `delay 40`, the ready effect on and the box unlocked; the finish trigger switches the
// finished effect on); ROM GMS `[AgentModelSetting] AgentTrigger`
// DC-DEFAULT: DSV-323 (proposed) the stage-2 station flow

use super::{Game, GameOut, Life, RoundPhase};
use crate::world::PropHash;
use dc_net_engine::time::since;
use dc_net_engine::Millis;
use std::collections::{BTreeMap, BTreeSet};

/// Statement words of the world scripts (hash keys of the lower-case word).
// OBSERVED: the ROM worlds' trigger and activation-box command texts (hashes only)
const W_MSG: PropHash = ([0xe4, 0x6b, 0x32, 0x01, 0x65, 0xee, 0xc9, 0x1e], 3);
const W_ON: PropHash = ([0xb8, 0xd3, 0x1e, 0x85, 0x27, 0x25, 0xaf, 0xb1], 2);
const W_OFF: PropHash = ([0xb4, 0xdc, 0x66, 0xdd, 0xe8, 0x06, 0x26, 0x1b], 3);
const W_REMOVE: PropHash = ([0x7e, 0x56, 0x08, 0xab, 0x61, 0x00, 0x17, 0xaf], 6);
const W_VISIBLE: PropHash = ([0xd4, 0x2e, 0xf1, 0x49, 0x79, 0x00, 0xbc, 0x6e], 7);
const W_DELAY: PropHash = ([0x6e, 0x16, 0xdb, 0x10, 0x22, 0x67, 0xe6, 0xe7], 5);
const W_TRIGGER: PropHash = ([0x68, 0x32, 0x59, 0xfe, 0xab, 0xbf, 0x5e, 0xb3], 7);
const W_LOCK: PropHash = ([0x0c, 0x03, 0x05, 0x86, 0x94, 0x5f, 0xe5, 0x04], 4);
const W_UNLOCK: PropHash = ([0x78, 0x76, 0x00, 0xeb, 0xe6, 0xd6, 0xc7, 0x5b], 6);
const W_SIDE_WIN: PropHash = ([0x87, 0x51, 0xff, 0xc4, 0x50, 0xa8, 0x49, 0x3f], 10);

/// Effect keys of the station (the session maps each to a placed-effect object), apart from the breakables' keys.
pub const FX_KEY_BASE: u32 = 0x5354_0000;
/// Triggers fired from triggers stop at this depth.
pub const MAX_DEPTH: u32 = 4;

/// One statement of a world script.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    FxOn(Vec<u8>),
    FxOff(Vec<u8>),
    Trigger(Vec<u8>),
    Lock(Vec<u8>),
    Unlock(Vec<u8>),
    SideWin,
}

fn key_of(w: &[u8]) -> PropHash {
    (dc_world::hash16(&w.to_ascii_lowercase()), w.len())
}

/// The statements of a command text with their delays (seconds); statements this server does not run are left
/// out.
pub fn parse(text: &[u8]) -> Vec<(f32, Stmt)> {
    let mut out = Vec::new();
    for st in text.split(|&b| b == b';' || b == b'\n' || b == b'\r') {
        let words: Vec<&[u8]> = st
            .split(|b| b.is_ascii_whitespace() || *b == b'(' || *b == b')')
            .filter(|w| !w.is_empty())
            .collect();
        let (delay, rest) = match words.as_slice() {
            [d, secs, rest @ ..] if key_of(d) == W_DELAY => {
                let Some(s) = std::str::from_utf8(secs)
                    .ok()
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|s| s.is_finite() && *s >= 0.0)
                else {
                    continue;
                };
                (s, rest)
            }
            rest => (0.0, rest),
        };
        let [m, target, arg, more @ ..] = rest else {
            continue;
        };
        if key_of(m) != W_MSG {
            continue;
        }
        let t = target.to_vec();
        let a = key_of(arg);
        let stmt = if a == W_ON {
            Stmt::FxOn(t)
        } else if a == W_OFF || a == W_REMOVE {
            Stmt::FxOff(t)
        } else if a == W_VISIBLE {
            if more.first().is_some_and(|v| *v == b"0") {
                Stmt::FxOff(t)
            } else {
                Stmt::FxOn(t)
            }
        } else if a == W_TRIGGER {
            Stmt::Trigger(t)
        } else if a == W_LOCK {
            Stmt::Lock(t)
        } else if a == W_UNLOCK {
            Stmt::Unlock(t)
        } else if a == W_SIDE_WIN {
            Stmt::SideWin
        } else {
            continue;
        };
        out.push((delay, stmt));
    }
    out
}

/// The station's state in a room.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Station {
    /// Delayed statements and when they run.
    pub queue: Vec<(Millis, Stmt)>,
    /// Effects switched on (lower-case name -> effect key).
    pub fx: BTreeMap<Vec<u8>, u32>,
    /// Locked boxes (lower-case names).
    pub locked: BTreeSet<Vec<u8>>,
    /// The box the agent works (uplink index) and since when.
    pub working: Option<(usize, Millis)>,
    /// The upload is done (the match is decided).
    pub done: bool,
    /// The world's locks were read.
    pub armed: bool,
    next_key: u32,
}

impl Game {
    /// The boxes' start locks (once per stage).
    fn station_arm(&mut self) {
        if self.spy.station.armed {
            return;
        }
        self.spy.station.armed = true;
        self.spy.station.locked = self
            .spy
            .world
            .uplinks
            .iter()
            .filter(|u| u.locked)
            .map(|u| u.name.to_ascii_lowercase())
            .collect();
    }

    /// Runs a command text now (its delayed statements later).
    pub(super) fn station_run(&mut self, text: &[u8], now: Millis, depth: u32) {
        for (delay, st) in parse(text) {
            if delay > 0.0 {
                let at = now.wrapping_add((delay * 1000.0) as u32);
                self.spy.station.queue.push((at, st));
            } else {
                self.station_exec(st, now, depth);
            }
        }
    }

    fn station_exec(&mut self, st: Stmt, now: Millis, depth: u32) {
        match st {
            Stmt::FxOn(n) => {
                let k = n.to_ascii_lowercase();
                if self.spy.station.fx.contains_key(&k) {
                    return;
                }
                let key = FX_KEY_BASE + self.spy.station.next_key;
                self.spy.station.next_key += 1;
                self.spy.station.fx.insert(k, key);
                self.out.push(GameOut::WorldFx { key, name: n });
            }
            Stmt::FxOff(n) => {
                if let Some(key) = self.spy.station.fx.remove(&n.to_ascii_lowercase()) {
                    self.out.push(GameOut::WorldFxGone { key });
                }
            }
            Stmt::Trigger(n) => {
                if depth >= MAX_DEPTH {
                    return;
                }
                let cmds: Vec<Vec<u8>> = self
                    .spy
                    .world
                    .triggers
                    .iter()
                    .find(|(t, _)| t.eq_ignore_ascii_case(&n))
                    .map(|(_, c)| c.clone())
                    .unwrap_or_default();
                for c in cmds {
                    self.station_run(&c, now, depth + 1);
                }
            }
            Stmt::Lock(n) => {
                self.spy.station.locked.insert(n.to_ascii_lowercase());
            }
            Stmt::Unlock(n) => {
                if self.spy.station.locked.remove(&n.to_ascii_lowercase()) {
                    self.log(format!(
                        "spy: the uplink {} is open",
                        String::from_utf8_lossy(&n)
                    ));
                }
            }
            Stmt::SideWin => self.spy_upload_done(now),
        }
    }

    /// Stage 2 starts: the trigger the mode file names brings the station; without it every box opens at once.
    // DC-DEFAULT: DSV-323 (proposed) a world without the named trigger opens its boxes when the agent appears
    pub(super) fn station_start(&mut self, now: Millis) {
        self.station_arm();
        let name = self.spy.gms.agent_trigger.clone().unwrap_or_default();
        let found = !name.is_empty()
            && self
                .spy
                .world
                .triggers
                .iter()
                .any(|(t, _)| t.eq_ignore_ascii_case(name.as_bytes()));
        if found {
            self.station_exec(Stmt::Trigger(name.into_bytes()), now, 0);
        } else {
            self.spy.station.locked.clear();
        }
    }

    /// Stage 2 ends: every station effect leaves, the boxes are locked again, nothing is pending.
    pub(super) fn station_reset(&mut self) {
        let fx = std::mem::take(&mut self.spy.station.fx);
        for key in fx.into_values() {
            self.out.push(GameOut::WorldFxGone { key });
        }
        self.spy.station = Station::default();
    }

    /// The box index the position is in (`Some(None)` when the world has no box: anywhere counts).
    fn station_box_at(&self, pos: [f32; 3]) -> Option<Option<usize>> {
        if self.spy.world.uplinks.is_empty() {
            return Some(None);
        }
        self.spy
            .world
            .uplinks
            .iter()
            .position(|u| {
                let h = u.dims.map(|d| d.abs().max(32.0));
                (0..3).all(|i| (pos[i] - u.pos[i]).abs() <= h[i] + 16.0)
            })
            .map(Some)
    }

    /// One tick of stage 2: the delayed statements, then the agent working an open box (its work command when he
    /// starts, its release command when he leaves, its activate command when the time is done).
    pub(super) fn station_tick(&mut self, now: Millis) {
        let due: Vec<Stmt> = {
            let q = &mut self.spy.station.queue;
            let mut due = Vec::new();
            q.retain(|(at, st)| {
                if now.wrapping_sub(*at) < u32::MAX / 2 {
                    due.push(st.clone());
                    false
                } else {
                    true
                }
            });
            due
        };
        for st in due {
            self.station_exec(st, now, 0);
            if !matches!(self.phase, RoundPhase::Running { .. }) {
                return;
            }
        }
        if self.spy.station.done {
            return;
        }
        let Some(agent) = self.spy.agent else {
            return;
        };
        let at = self
            .players
            .get(&agent)
            .filter(|p| p.life == Life::Alive)
            .and_then(|p| self.station_box_at(p.pos));
        let open = at.filter(|b| {
            b.is_none_or(|i| {
                !self
                    .spy
                    .station
                    .locked
                    .contains(&self.spy.world.uplinks[i].name.to_ascii_lowercase())
            })
        });
        let idx = open.map(|b| b.unwrap_or(usize::MAX));
        match (self.spy.station.working, idx) {
            (Some((w, t0)), Some(i)) if w == i => {
                let secs = self
                    .spy
                    .world
                    .uplinks
                    .get(i)
                    .and_then(|u| u.secs)
                    .filter(|s| *s > 0.0)
                    .unwrap_or(super::spy::DEFAULT_TRANSMIT_SECS);
                if since(now, t0) as f32 >= secs * 1000.0 {
                    self.spy.station.working = None;
                    self.log(format!(
                        "spy: the agent (client {agent}) uploaded the intel"
                    ));
                    let act = self.spy.world.uplinks.get(i).map(|u| u.activate.clone());
                    if let Some(a) = act {
                        self.station_run(&a, now, 0);
                    }
                    // a box whose activate command names no defence win still ends the mission
                    self.spy_upload_done(now);
                }
            }
            (w, i) => {
                if let Some((wi, _)) = w {
                    self.spy.station.working = None;
                    let rel = self.spy.world.uplinks.get(wi).map(|u| u.release.clone());
                    if let Some(r) = rel {
                        self.station_run(&r, now, 0);
                    }
                }
                if let Some(i) = i {
                    self.spy.station.working = Some((i, now));
                    self.log(format!("spy: the agent (client {agent}) uploads the intel"));
                    let work = self.spy.world.uplinks.get(i).map(|u| u.work.clone());
                    if let Some(wk) = work {
                        self.station_run(&wk, now, 0);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The station statements of a command text: switches, triggers, locks, delays and the defence win; others are
    /// skipped.
    #[test]
    fn station_statements_parse() {
        let s = parse(
            concat!(
                "msg A on;\r\nmsg B (visible 0);\r\ndelay 7 (msg C (solid 1));\r\ndelay 40 (msg Box unlock);\r\n",
                "msg F trigger;\r\nmsg Box lock;\r\nmsg Ctl defence",
                "win;\r\nmsg D remove;"
            )
            .as_bytes(),
        );
        assert_eq!(
            s,
            vec![
                (0.0, Stmt::FxOn(b"A".to_vec())),
                (0.0, Stmt::FxOff(b"B".to_vec())),
                (40.0, Stmt::Unlock(b"Box".to_vec())),
                (0.0, Stmt::Trigger(b"F".to_vec())),
                (0.0, Stmt::Lock(b"Box".to_vec())),
                (0.0, Stmt::SideWin),
                (0.0, Stmt::FxOff(b"D".to_vec())),
            ]
        );
    }
}
