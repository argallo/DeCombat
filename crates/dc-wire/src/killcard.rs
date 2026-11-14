//! Kill zoom and kill card (DSV-652, CLI-1956..1958, owner request 2026-11-10): after a death the victim's camera
//! zooms to the killer and holds on him with a card (name, health, armour). Our own design, no 2009 counterpart.
//! Offered by a server entry with `features = ["killcard"]` (LG-439).
//!
//! One shell message with our own id, an id no stage of the 2009 dispatch handles (dropped there, SPEC:
//! GAME_MESSAGES_2009_S2C.md §7.2, row 0x9d):
//!
//! ```text
//! S->C [13][0xf6][kind:u8] kind 0 offer: [hold_ms:u16]          (at world entry: kill cards in this match)
//!                          kind 1 card:  [killer:u8][obj:u16][health:u16][health_max:u16][armor:u16][armor_max:u16]
//! ```
//!
//! A card goes to the victim only, at the kill: `killer` is the killer's roster id, `obj` his object, the values his
//! health and armour after the kill, rounded up (a living killer has at least 1 health). A client that does not know
//! 0xf6 drops it.
// DC-DEFAULT: DSV-652 (proposed, owner request 2026-11-10) the messages; LG-439 the entry feature

use crate::udp::{BitReader, BitWriter};

/// The server's offer and cards (not a 2009 id).
pub const SHELL_ID: u8 = 0xf6;
/// The server entry feature name.
pub const FEATURE: &str = "killcard";

const KIND_OFFER: u8 = 0;
const KIND_CARD: u8 = 1;

/// The killer as the victim's card shows him.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Card {
    /// Roster id.
    pub killer: u8,
    /// World object.
    pub obj: u16,
    pub health: u16,
    pub health_max: u16,
    pub armor: u16,
    pub armor_max: u16,
}

/// What the server tells a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// Kill cards work in this match; the card's hold time.
    Offer { hold_ms: u16 },
    /// The local player was killed by this player.
    Card(Card),
}

/// The notice body after the shell id.
pub fn write_notice(w: &mut BitWriter, n: Notice) {
    match n {
        Notice::Offer { hold_ms } => {
            w.write_u8(KIND_OFFER).write_u16(hold_ms);
        }
        Notice::Card(c) => {
            w.write_u8(KIND_CARD)
                .write_u8(c.killer)
                .write_u16(c.obj)
                .write_u16(c.health)
                .write_u16(c.health_max)
                .write_u16(c.armor)
                .write_u16(c.armor_max);
        }
    }
}

/// Reads a notice; `None` when it is cut short or of an unknown kind. Bits after the body are ignored (as our
/// readers of the 2009 messages do).
pub fn read_notice(r: &mut BitReader<'_>) -> Option<Notice> {
    if r.remaining_bits() < 8 {
        return None;
    }
    match r.read_u8() {
        KIND_OFFER if r.remaining_bits() >= 16 => Some(Notice::Offer {
            hold_ms: r.read_u16(),
        }),
        KIND_CARD if r.remaining_bits() >= 8 + 5 * 16 => Some(Notice::Card(Card {
            killer: r.read_u8(),
            obj: r.read_u16(),
            health: r.read_u16(),
            health_max: r.read_u16(),
            armor: r.read_u16(),
            armor_max: r.read_u16(),
        })),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_round_trip_and_junk_is_refused() {
        for n in [
            Notice::Offer { hold_ms: 1000 },
            Notice::Card(Card {
                killer: 7,
                obj: 0x1234,
                health: 37,
                health_max: 100,
                armor: 0,
                armor_max: 100,
            }),
        ] {
            let mut b = BitWriter::new();
            write_notice(&mut b, n);
            assert_eq!(read_notice(&mut b.reader()), Some(n));
        }
        assert_eq!(read_notice(&mut BitWriter::new().reader()), None);
        let mut b = BitWriter::new();
        b.write_u8(9).write_u16(1);
        assert_eq!(read_notice(&mut b.reader()), None);
        let mut b = BitWriter::new();
        b.write_u8(KIND_CARD).write_u8(1).write_u16(2);
        assert_eq!(read_notice(&mut b.reader()), None);
    }
}
