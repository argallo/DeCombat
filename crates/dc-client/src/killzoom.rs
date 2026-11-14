//! CLI-1956: the kill zoom. On a server that offers kill cards (`dc_wire::killcard`, an entry with the `killcard`
//! feature), a death by another player starts the death camera with a fast zoom from the own body to the killer,
//! then holds on him; the killer stands frozen as he was at the kill the whole time, and the HUD shows his card
//! (CLI-1957: name, health, armour). After the hold the follow camera of the 2009 rules takes over
//! ([`crate::deathcam`]). Without an offer or a card the 2009 killer camera runs as before.
//!
//! Provenance: ORIGINAL (no 2009 counterpart; owner request 2026-11-10).
// DC-DEFAULT: CLI-1956 (proposed, owner request 2026-11-10 "after getting killed the camera would do an epic zoom to
// the enemy that killed you before switching to the respawn cam ... freeze on them for one second with their name
// ... and show their HP"), see DC_DEFAULTS.md

use dc_sim::V3;
use dc_wire::killcard::{Card, Notice};

/// How long the camera flies from the own body to the killer.
pub const ZOOM_MS: u32 = 450;
/// The longest hold the client takes from an offer (the server's value is used up to this).
pub const MAX_HOLD_MS: u16 = 3000;
/// How far in front of the killer's head the camera stops (world units: head and shoulders fill the view).
pub const STANDOFF: f32 = 80.0;
/// The camera never stops closer to the head than this, even with a wall right behind the killer's line.
pub const MIN_STANDOFF: f32 = 24.0;
/// The aim point: the killer's body centre raised by this share of its half height (about the eyes).
pub const HEAD_SHARE: f32 = 0.75;
/// A card that arrives this long after the death still starts the zoom (the card and the death come in one burst;
/// a late packet must not start a zoom halfway through the 2009 killer camera).
pub const LATE_MS: u32 = 300;

/// The card of the last own death on an offering server, as the death camera takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zoomable {
    pub card: Card,
    pub hold_ms: u16,
}

/// The kill cards of a match (client side): the offer and the last card.
#[derive(Debug, Clone, Default)]
pub struct KillCards {
    hold_ms: Option<u16>,
    card: Option<Card>,
}

impl KillCards {
    pub fn on_notice(&mut self, n: Notice) {
        match n {
            Notice::Offer { hold_ms } => self.hold_ms = Some(hold_ms.min(MAX_HOLD_MS)),
            Notice::Card(c) => self.card = Some(c),
        }
    }

    /// A new world: no offer until the server sends one.
    pub fn reset(&mut self) {
        *self = KillCards::default();
    }

    /// The own player lives again: the last card is spent.
    pub fn alive(&mut self) {
        self.card = None;
    }

    /// The card for the death camera (none without an offer).
    pub fn view(&self) -> Option<Zoomable> {
        Some(Zoomable {
            card: self.card?,
            hold_ms: self.hold_ms?,
        })
    }
}

/// Where the camera aims on a body: its centre raised to about the eyes.
pub fn aim_point(pos: V3, half_height: f32) -> V3 {
    pos + V3::new(0.0, half_height.max(0.0) * HEAD_SHARE, 0.0)
}

/// The flight's end: [`STANDOFF`] in front of `aim` on the line back toward `from`, pulled in by a wall on that
/// line (`free`: how far the line is free from `aim`, `None`: no wall), never closer than [`MIN_STANDOFF`]. `None`
/// when `from` and `aim` are the same point.
pub fn end_point(from: V3, aim: V3, free: Option<f32>) -> Option<V3> {
    let d = from - aim;
    let len = d.len();
    if !len.is_finite() || len < 1e-3 {
        return None;
    }
    let back = d * (1.0 / len);
    let reach = STANDOFF
        .min(len)
        .min(free.unwrap_or(f32::MAX))
        .max(MIN_STANDOFF.min(len));
    Some(aim + back * reach)
}

/// The flight's progress at `t` of [`ZOOM_MS`]: a fast start that settles (ease-out cubic), 0..=1.
pub fn ease(t_ms: u32) -> f32 {
    let t = (t_ms as f32 / ZOOM_MS as f32).clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// The frame's bodies with the frozen killer in place of his live body (added when the stream no longer has him).
pub fn freeze_into(remotes: &mut Vec<crate::world::RemoteBody>, frozen: &crate::world::RemoteBody) {
    match remotes.iter_mut().find(|r| r.pid == frozen.pid) {
        Some(r) => *r = frozen.clone(),
        None => remotes.push(frozen.clone()),
    }
}

/// The HUD's card of the death camera's shown card.
pub fn hud_card(shown: Option<(Zoomable, &str)>) -> Option<crate::hud::killcard::KillCardShown> {
    shown.map(|(z, name)| crate::hud::killcard::KillCardShown {
        name: name.to_string(),
        card: z.card,
    })
}

/// Whether a death this old with this killer may start the zoom from `z`.
pub fn starts(z: Option<&Zoomable>, killer: Option<u8>, since_ms: u32) -> bool {
    since_ms <= LATE_MS && z.is_some_and(|z| Some(z.card.killer) == killer && z.hold_ms > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(killer: u8) -> Card {
        Card {
            killer,
            obj: 9,
            health: 37,
            health_max: 100,
            armor: 0,
            armor_max: 100,
        }
    }

    /// CLI-1956: the session hands the kill cards' shell to the client whole (else the offer never arrives, as with
    /// the pings before CLI-1952).
    #[test]
    fn the_session_hands_the_kill_card_shell_to_the_client() {
        assert!(dc_botclient::session::RAW_SHELL_EVENTS.contains(&dc_wire::killcard::SHELL_ID));
    }

    #[test]
    fn a_card_counts_only_with_an_offer_and_until_the_next_life() {
        let mut k = KillCards::default();
        k.on_notice(Notice::Card(card(3)));
        assert_eq!(k.view(), None, "no offer");
        k.on_notice(Notice::Offer { hold_ms: 60_000 });
        assert_eq!(
            k.view(),
            Some(Zoomable {
                card: card(3),
                hold_ms: MAX_HOLD_MS
            })
        );
        k.alive();
        assert_eq!(k.view(), None);
        k.on_notice(Notice::Card(card(4)));
        k.reset();
        assert_eq!(k.view(), None);
    }

    #[test]
    fn the_flight_ends_in_front_of_the_head_and_a_wall_pulls_it_in() {
        let aim = V3::new(0.0, 60.0, 0.0);
        let from = V3::new(0.0, 60.0, 1000.0);
        let e = end_point(from, aim, None).unwrap();
        assert!((e.z - STANDOFF).abs() < 1e-3 && e.x.abs() < 1e-3);
        let e = end_point(from, aim, Some(40.0)).unwrap();
        assert!((e.z - 40.0).abs() < 1e-3);
        let e = end_point(from, aim, Some(2.0)).unwrap();
        assert!((e.z - MIN_STANDOFF).abs() < 1e-3);
        // a killer closer than the standoff: the camera stays where it is
        let near = V3::new(0.0, 60.0, 30.0);
        assert!((end_point(near, aim, None).unwrap().z - 30.0).abs() < 1e-3);
        assert_eq!(end_point(aim, aim, None), None);
        assert_eq!(ease(0), 0.0);
        assert_eq!(ease(ZOOM_MS), 1.0);
        assert!(ease(ZOOM_MS / 4) > 0.5, "fast start");
    }

    #[test]
    fn only_a_fresh_death_by_the_cards_killer_starts_the_zoom() {
        let z = Zoomable {
            card: card(3),
            hold_ms: 1000,
        };
        assert!(starts(Some(&z), Some(3), 0));
        assert!(!starts(Some(&z), Some(4), 0));
        assert!(!starts(Some(&z), Some(3), LATE_MS + 1));
        assert!(!starts(None, Some(3), 0));
    }
}
