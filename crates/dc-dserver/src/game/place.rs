//! GATE-PLACE: a test-only server feature that puts a named player at a scripted position and facing so a gate
//! scenario reaches its state in seconds instead of walking there. ORIGINAL tooling (no 2009 counterpart).
//!
//! Off by default: without `-place keyed=<KEY>` the config is empty, scripted entries are a launch error, and a
//! client's `place ...` console request is logged and ignored. A placement is the server moving a player the way
//! it already corrects a bad update (a new teleport sequence, `GS_MOVER_UPDATE` teleport, view angles), so the
//! client's next player update starts from the spot.
// SPEC: MOVEMENT_2009.md §12.4 (teleport sequence and mover update), GAME_MESSAGES_2009_S2C.md §2.1 (0x46)
// DC-DEFAULT: DSV-146 (gate-place) test-only placement, keyed and bounded

use super::{fold_yaw, Game, Life};
use crate::gs;
use dc_net_engine::time::since;
use dc_net_engine::Millis;

/// The most scripted placements one launch accepts.
// DC-DEFAULT: DSV-146 16 entries
pub const MAX_ENTRIES: usize = 16;

/// One scripted placement: where the named player goes, `at_ms` after he first stands alive in the world.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaceEntry {
    /// Case-insensitive; `_` and a space compare equal.
    pub nick: String,
    pub pos: [f32; 3],
    /// Degrees.
    pub yaw_deg: f32,
    pub at_ms: u32,
}

/// The placement configuration of a launch. `key` is `None` when the feature is off.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlaceConfig {
    pub key: Option<String>,
    pub entries: Vec<PlaceEntry>,
}

/// Runtime state: which entries have fired, and when each player was first seen alive.
#[derive(Debug, Default)]
pub struct PlaceState {
    done: Vec<bool>,
    first_alive: std::collections::BTreeMap<u8, Millis>,
}

fn norm(s: &[u8]) -> String {
    s.iter()
        .map(|&b| match b.to_ascii_lowercase() {
            b'_' => ' ',
            c => c as char,
        })
        .collect()
}

/// Parses `NICK,x,y,z,yaw[,at_ms]`.
pub fn parse_entry(v: &str) -> Result<PlaceEntry, String> {
    let t: Vec<&str> = v.split(',').map(str::trim).collect();
    if !(5..=6).contains(&t.len()) || t[0].is_empty() {
        return Err(format!("-place: want NICK,x,y,z,yaw[,at_ms], got {v:?}"));
    }
    let f = |i: usize| -> Result<f32, String> {
        t[i].parse::<f32>()
            .ok()
            .filter(|x| x.is_finite())
            .ok_or_else(|| format!("-place: bad number {:?}", t[i]))
    };
    let at_ms = match t.get(5) {
        Some(s) => s
            .parse::<u32>()
            .map_err(|_| format!("-place: bad at_ms {s:?}"))?,
        None => 0,
    };
    Ok(PlaceEntry {
        nick: t[0].to_string(),
        pos: [f(1)?, f(2)?, f(3)?],
        yaw_deg: f(4)?,
        at_ms,
    })
}

/// Parses a console request `place <KEY> x y z yaw`.
pub fn parse_request(text: &str) -> Option<(String, [f32; 3], f32)> {
    let t: Vec<&str> = text.split_whitespace().collect();
    if t.len() != 6 || !t[0].eq_ignore_ascii_case("place") {
        return None;
    }
    let n: Vec<f32> = t[2..]
        .iter()
        .filter_map(|s| s.parse::<f32>().ok().filter(|x| x.is_finite()))
        .collect();
    (n.len() == 4).then(|| (t[1].to_string(), [n[0], n[1], n[2]], n[3]))
}

impl Game {
    /// Moves `pid` to `pos` facing `yaw_deg` with a new teleport sequence. False if he is not alive.
    fn place_player(&mut self, pid: u8, pos: [f32; 3], yaw_deg: f32, now: Millis) -> bool {
        let dims = self.dims_of(pid);
        let Some(p) = self.players.get_mut(&pid) else {
            return false;
        };
        if p.life != Life::Alive {
            return false;
        }
        let yaw = fold_yaw(yaw_deg.to_radians());
        p.move_seq = p.move_seq.wrapping_add(1);
        p.pos = pos;
        p.track.restart(now, pos);
        p.vel = [0.0; 3];
        p.yaw = yaw;
        p.last_move = Some(now);
        let seq = p.move_seq;
        self.send(pid, gs::mover_teleport(seq, pos, dims, None));
        self.send(pid, gs::view_angles(gs::ANGLES_ALL, [0.0, yaw, 0.0]));
        true
    }

    /// The tick hook: fires each scripted entry once, `at_ms` after its player first stands alive.
    pub(super) fn tick_place(&mut self, now: Millis) {
        if self.cfg.place.key.is_none() || self.cfg.place.entries.is_empty() {
            return;
        }
        let n = self.cfg.place.entries.len();
        self.place.done.resize(n, false);
        let alive: Vec<(u8, String)> = self
            .players
            .values()
            .filter(|p| p.life == Life::Alive && p.introduced && p.spawns > 0)
            .map(|p| (p.pid, norm(&p.name8)))
            .collect();
        for (pid, _) in &alive {
            self.place.first_alive.entry(*pid).or_insert(now);
        }
        for i in 0..n {
            if self.place.done[i] {
                continue;
            }
            let e = self.cfg.place.entries[i].clone();
            let want = norm(e.nick.as_bytes());
            let Some(&(pid, _)) = alive.iter().find(|(_, nm)| *nm == want) else {
                continue;
            };
            let t0 = self.place.first_alive.get(&pid).copied().unwrap_or(now);
            if since(now, t0) < e.at_ms {
                continue;
            }
            if self.place_player(pid, e.pos, e.yaw_deg, now) {
                self.place.done[i] = true;
                self.log(format!(
                    "place: {} (client {pid}) to {:?} yaw {} at +{} ms (scripted)",
                    e.nick, e.pos, e.yaw_deg, e.at_ms
                ));
            }
        }
    }

    /// A client's console request `place <KEY> x y z yaw`: moves only the sender, only with the feature on and the
    /// key right. Returns true when the text was a placement request (applied or refused), so the caller stops.
    pub fn place_request(&mut self, pid: u8, text: &str, now: Millis) -> bool {
        let Some((key, pos, yaw)) = parse_request(text) else {
            return false;
        };
        let ok = self.cfg.place.key.as_deref().is_some_and(|k| k == key);
        if !ok {
            let why = if self.cfg.place.key.is_some() {
                "wrong key"
            } else {
                "feature off"
            };
            self.log(format!("place: request from client {pid} refused ({why})"));
            return true;
        }
        if self.place_player(pid, pos, yaw, now) {
            self.log(format!(
                "place: client {pid} to {pos:?} yaw {yaw} (keyed request)"
            ));
        } else {
            self.log(format!("place: client {pid} not alive, request dropped"));
        }
        true
    }
}
