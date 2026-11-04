//! Other players' health for the x-ray health bars (DSV-480): only where culling is off (`-visibility off`, a room
//! with wallhacks, DSV-479 / MOD-22). A culled match sends nothing of it, byte for byte as before.
//!
//! Each tick the players' health (rounded up, 0 while not alive) and maximum are compared with what was sent; the
//! changed entries go to every introduced client in one reliable shell message (our id 0xf1, `dc_wire::remotehealth`),
//! and a client that is new to the list gets every entry once. The maximum is the room's `combat.max_health`, raised
//! to the highest health the player had since his last spawn (an agent's or an infected player's larger pool).
// DC-DEFAULT: DSV-480 (proposed, USER REQUEST 2026-11-01) remote health in wallhacks matches

use super::{Game, Life};
use dc_wire::remotehealth::{self, HealthEntry};
use dc_wire::udp::BitWriter;
use std::collections::{BTreeMap, BTreeSet};

/// The sync state (`None` on a culled match).
#[derive(Debug, Default)]
pub struct RemoteHealth {
    /// Last sent (health, max) by player.
    sent: BTreeMap<u8, (u16, u16)>,
    /// Per player: the spawn count and the highest health seen in that life.
    peak: BTreeMap<u8, (u32, f32)>,
    /// Clients that hold the whole list.
    told: BTreeSet<u8>,
}

/// The message for `entries`.
pub fn message(entries: &[HealthEntry]) -> BitWriter {
    let mut b = BitWriter::new();
    remotehealth::write_body(&mut b, entries);
    crate::msg::s2c_shell_msg(remotehealth::SHELL_ID, &b)
}

fn round_up(v: f32) -> u16 {
    if v.is_finite() {
        v.max(0.0).ceil().min(u16::MAX as f32) as u16
    } else {
        0
    }
}

impl Game {
    /// DSV-480: sends other players' health from now on (the session turns it on when culling is off).
    pub fn set_remote_health(&mut self, on: bool) {
        self.remote_health = on.then(RemoteHealth::default);
    }

    /// DSV-480: whether this match sends other players' health.
    pub fn sends_remote_health(&self) -> bool {
        self.remote_health.is_some()
    }

    pub(super) fn tick_remote_health(&mut self) {
        let Some(mut st) = self.remote_health.take() else {
            return;
        };
        let base = self.cfg.max_health;
        let mut all = Vec::with_capacity(self.players.len());
        let mut changed = Vec::new();
        for p in self.players.values() {
            let alive = matches!(p.life, Life::Alive);
            let hp = if alive { p.health.max(0.0) } else { 0.0 };
            let peak = st.peak.entry(p.pid).or_insert((p.spawns, 0.0));
            if peak.0 != p.spawns {
                *peak = (p.spawns, 0.0);
            }
            if hp.is_finite() {
                peak.1 = peak.1.max(hp);
            }
            let e = HealthEntry {
                pid: p.pid,
                health: round_up(hp),
                max: round_up(base.max(peak.1)).max(1),
            };
            if st.sent.get(&p.pid) != Some(&(e.health, e.max)) {
                st.sent.insert(p.pid, (e.health, e.max));
                changed.push(e);
            }
            all.push(e);
        }
        st.sent.retain(|pid, _| self.players.contains_key(pid));
        st.peak.retain(|pid, _| self.players.contains_key(pid));
        let to = self.introduced();
        st.told.retain(|pid| to.contains(pid));
        for pid in to {
            if st.told.insert(pid) {
                if !all.is_empty() {
                    self.send(pid, message(&all));
                }
            } else if !changed.is_empty() {
                self.send(pid, message(&changed));
            }
        }
        self.remote_health = Some(st);
    }
}
