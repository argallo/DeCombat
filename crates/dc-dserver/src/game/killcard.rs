//! Kill zoom and kill card (owner request 2026-11-10): `-killcard` from the lobby on an entry with
//! `features = ["killcard"]` (LG-439). At world entry the client hears that kill cards work in this match; at every
//! kill by another player the victim (only the victim) gets the killer's health and armour as they are after the
//! kill, so the client's kill zoom can show how close the fight was. Nothing on a death by the world or by the own
//! hand; any other player's kill has a card, a team kill included (the card shows who it was).
//!
//! Provenance: ORIGINAL (no 2009 counterpart). Messages: `dc_wire::killcard`.
// DC-DEFAULT: DSV-652 (proposed, owner request 2026-11-10) the kill card rules

use super::Game;
use dc_wire::killcard::{self, Card, Notice};
use dc_wire::udp::BitWriter;

/// How long the client holds the zoom on the killer (the client may shorten it, never lengthen it).
pub const HOLD_MS: u16 = 1000;

/// The shell message of a notice.
pub fn message(n: Notice) -> BitWriter {
    let mut b = BitWriter::new();
    killcard::write_notice(&mut b, n);
    crate::msg::s2c_shell_msg(killcard::SHELL_ID, &b)
}

fn round_up(v: f32) -> u16 {
    if v.is_finite() {
        v.max(0.0).ceil().min(u16::MAX as f32) as u16
    } else {
        0
    }
}

impl Game {
    /// At world entry: tells the client that kill cards work here (nothing on other matches).
    pub(super) fn kill_card_offer(&mut self, pid: u8) {
        if !self.cfg.killcard {
            return;
        }
        self.send(pid, message(Notice::Offer { hold_ms: HOLD_MS }));
    }

    /// At a kill: the killer's card to the victim (none for a death without another player as its killer, and none
    /// in Spy Hunt, where a spy's state is hidden information; the client then runs the 2009 killer camera).
    pub(super) fn kill_card(&mut self, vpid: u8, killer: Option<u8>) {
        // DC-DEFAULT: DSV-652 (amended 2026-11-10, review) no card in Spy Hunt
        if !self.cfg.killcard || self.is_spy() {
            return;
        }
        let Some(k) = killer.filter(|&k| k != vpid) else {
            return;
        };
        let Some(p) = self.players.get(&k) else {
            return;
        };
        let (obj, health, armor) = (p.obj, p.health, p.armor);
        let armour = self.armour_of(k);
        let card = Card {
            killer: k,
            obj,
            health: round_up(health),
            health_max: round_up(self.cfg.max_health.max(health)).max(1),
            armor: round_up(armor),
            armor_max: round_up(armour.max.max(armor)),
        };
        self.send(vpid, message(Notice::Card(card)));
    }
}
