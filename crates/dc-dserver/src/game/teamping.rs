//! Team pings (project decision): `-pings` from the lobby on an entry with `features = ["pings"]` (LG-437). A living
//! player pings the enemy under his crosshair (the enemy's nametag shows for the pinger's team for [`ENEMY_MS`], also
//! through walls: culling lets the team see him for that time) or the place under it (a marker for the team for
//! [`PLACE_MS`]). Off in free-for-all rooms (no team to tell).
//!
//! Provenance: ORIGINAL (no 2009 counterpart). Messages: `dc_wire::teamping`.
// DC-DEFAULT: DSV-651 (proposed, project decision; owner request 2026-11-08) the team ping rules

use super::{Game, Life};
use dc_net_engine::time::{later, since};
use dc_net_engine::Millis;
use dc_wire::teamping::{self, Notice, Request};
use dc_wire::udp::BitWriter;
use std::collections::BTreeMap;

/// How long a pinged enemy's nametag shows (and culling lets the team see him).
pub const ENEMY_MS: u16 = 3000;
/// How long a place marker shows (slightly longer than an enemy tag, owner request).
pub const PLACE_MS: u16 = 5000;
/// The shortest time between two pings of one player.
pub const COOLDOWN_MS: u32 = 1000;
/// The pinger's last clear sight of the enemy must be at most this old (culling's own verdict).
pub const SEEN_MS: u32 = 1000;
/// The farthest place a player may ping, in world units from his position.
pub const PLACE_REACH: f32 = 30000.0;

/// The match's ping state.
#[derive(Debug, Default)]
pub struct TeamPings {
    /// The last accepted ping per player.
    last: BTreeMap<u8, Millis>,
    /// (team, enemy pid) -> until: the team sees the enemy unculled.
    spotted: BTreeMap<(u8, u8), Millis>,
}

/// The shell message of a notice.
pub fn message(n: Notice) -> BitWriter {
    let mut b = BitWriter::new();
    teamping::write_notice(&mut b, n);
    crate::msg::s2c_shell_msg(teamping::SHELL_ID, &b)
}

impl Game {
    /// Whether this match plays team pings: the flag, and not a free-for-all room.
    pub fn plays_team_pings(&self) -> bool {
        self.cfg.pings && self.rules.mode_class != crate::visibility::MODE_CLASS_FFA
    }

    /// At world entry: tells the client that pings work here (nothing on other matches).
    pub(super) fn team_ping_offer(&mut self, pid: u8) {
        if !self.plays_team_pings() {
            return;
        }
        self.send(
            pid,
            message(Notice::Offer {
                enemy_ms: ENEMY_MS,
                place_ms: PLACE_MS,
            }),
        );
    }

    /// The teams that see `enemy` unculled because of a ping, as a bit mask (team values 0..15).
    pub fn spotted_mask(&self, enemy: u8) -> u16 {
        self.team_pings
            .spotted
            .keys()
            .filter(|(t, e)| *e == enemy && *t < 16)
            .fold(0, |m, (t, _)| m | 1 << t)
    }

    /// A ping of `pid`. `seen(enemy_pid, enemy_obj)` = the pinger's own client had a clear sight of the enemy just
    /// now (the session asks its culling); without it an enemy ping is refused.
    pub fn team_ping(&mut self, pid: u8, q: Request, seen: &dyn Fn(u8, u16) -> bool, now: Millis) {
        if !self.plays_team_pings() {
            return;
        }
        let Some(p) = self.players.get(&pid) else {
            return;
        };
        if !p.introduced || p.life != Life::Alive {
            return;
        }
        if self
            .team_pings
            .last
            .get(&pid)
            .is_some_and(|t| since(now, *t) < COOLDOWN_MS)
        {
            return;
        }
        // every attempt counts for the cooldown, refused ones too: a flood costs one lookup and one log line a second
        self.team_pings.last.insert(pid, now);
        let (team, from, at) = (p.team, p.obj, p.pos);
        let notice = match q {
            Request::Enemy { obj } => {
                let Some(e) = self.players.values().find(|e| e.obj == obj && e.pid != pid) else {
                    return;
                };
                if !e.introduced || e.life != Life::Alive || e.team == team {
                    self.log(format!(
                        "ping: client {pid}: object {obj} is no living enemy: refused"
                    ));
                    return;
                }
                let epid = e.pid;
                if !seen(epid, obj) {
                    self.log(format!(
                        "ping: client {pid}: client {epid} not in clear sight: refused"
                    ));
                    return;
                }
                self.team_pings
                    .spotted
                    .insert((team, epid), now.wrapping_add(u32::from(ENEMY_MS)));
                self.log(format!(
                    "ping: client {pid} spots client {epid} for team {team}"
                ));
                Notice::Enemy {
                    from,
                    obj,
                    ms: ENEMY_MS,
                }
            }
            Request::Place { pos } => {
                let d = (0..3).map(|i| (pos[i] - at[i]).powi(2)).sum::<f32>().sqrt();
                if d > PLACE_REACH {
                    self.log(format!(
                        "ping: client {pid}: place {d:.0} units away: refused"
                    ));
                    return;
                }
                self.log(format!("ping: client {pid} marks {pos:?} for team {team}"));
                Notice::Place {
                    from,
                    pos,
                    ms: PLACE_MS,
                }
            }
        };
        let to: Vec<u8> = self
            .players
            .values()
            .filter(|q| q.introduced && q.team == team)
            .map(|q| q.pid)
            .collect();
        let m = message(notice);
        for q in to {
            self.send(q, m.clone());
        }
    }

    /// A player left: his cooldown and every spot on him go (a reused player number inherits nothing).
    pub(super) fn team_ping_forget(&mut self, pid: u8) {
        self.team_pings.last.remove(&pid);
        self.team_pings.spotted.retain(|(_, e), _| *e != pid);
    }

    /// Ends the spots whose time is up and those of players who left or died. A spot outlives its pinger's death
    /// or team change by at most its time (3 s).
    pub(super) fn tick_team_pings(&mut self, now: Millis) {
        if self.team_pings.spotted.is_empty() {
            return;
        }
        let players = &self.players;
        self.team_pings.spotted.retain(|(_, e), until| {
            later(*until, now) && players.get(e).is_some_and(|p| p.life == Life::Alive)
        });
    }
}
