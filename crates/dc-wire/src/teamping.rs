//! Team pings (DSV-651, CLI-1950..1952, project decision): a player marks an enemy or a place for his team. Our own
//! design, no 2009 counterpart. Offered by a server entry with `features = ["pings"]` (LG-437).
//!
//! Both directions are shell messages with our own ids, ids no stage of the 2009 dispatch handles (dropped there,
//! SPEC: GAME_MESSAGES_2009_S2C.md §7.2, row 0x9d; the 2009 server side: GAME_MESSAGES_2009_C2S.md §1.3 has no such id):
//!
//! ```text
//! C->S [13][0xf4][kind:u8] kind 0 enemy: [obj:u16]          (the character under the crosshair)
//!                          kind 1 place: [x:f32][y:f32][z:f32] (the world point under the crosshair)
//! S->C [13][0xf5][kind:u8] kind 2 offer: [enemy_ms:u16][place_ms:u16]  (at world entry: pings work in this match)
//!                          kind 0 enemy: [from:u16][obj:u16][ms:u16]   (show obj's nametag for ms)
//!                          kind 1 place: [from:u16][x:f32][y:f32][z:f32][ms:u16] (a marker at the point for ms)
//! ```
//!
//! `from` is the pinging player's object id. A client that does not know 0xf5 drops it; a server that does not know
//! 0xf4 logs it once (DSV-9). A client sends 0xf4 only after an offer.
// DC-DEFAULT: DSV-651 (proposed, project decision) the messages; LG-437 the entry feature

use crate::udp::{BitReader, BitWriter};

/// The client's request (not a 2009 id).
pub const UP_ID: u8 = 0xf4;
/// The server's offer and relays (not a 2009 id).
pub const SHELL_ID: u8 = 0xf5;
/// The server entry feature name.
pub const FEATURE: &str = "pings";

const KIND_ENEMY: u8 = 0;
const KIND_PLACE: u8 = 1;
const KIND_OFFER: u8 = 2;

/// What a client asks to ping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Request {
    /// The character object under the crosshair.
    Enemy { obj: u16 },
    /// The world point under the crosshair.
    Place { pos: [f32; 3] },
}

/// What the server tells a client.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Notice {
    /// Pings work in this match; the shown times.
    Offer { enemy_ms: u16, place_ms: u16 },
    /// Show `obj`'s nametag for `ms` (pinged by `from`).
    Enemy { from: u16, obj: u16, ms: u16 },
    /// A marker at `pos` for `ms` (pinged by `from`).
    Place { from: u16, pos: [f32; 3], ms: u16 },
}

fn write_pos(w: &mut BitWriter, p: [f32; 3]) {
    w.write_f32(p[0]).write_f32(p[1]).write_f32(p[2]);
}

fn read_pos(r: &mut BitReader<'_>) -> Option<[f32; 3]> {
    if r.remaining_bits() < 96 {
        return None;
    }
    let p = [r.read_f32(), r.read_f32(), r.read_f32()];
    p.iter().all(|c| c.is_finite()).then_some(p)
}

/// The request body after the shell id.
pub fn write_request(w: &mut BitWriter, q: Request) {
    match q {
        Request::Enemy { obj } => {
            w.write_u8(KIND_ENEMY).write_u16(obj);
        }
        Request::Place { pos } => {
            w.write_u8(KIND_PLACE);
            write_pos(w, pos);
        }
    }
}

/// Reads a request; `None` when it is cut short, of an unknown kind, or not finite. Bits after the body are ignored
/// (as our readers of the 2009 messages do).
pub fn read_request(r: &mut BitReader<'_>) -> Option<Request> {
    if r.remaining_bits() < 8 {
        return None;
    }
    match r.read_u8() {
        KIND_ENEMY if r.remaining_bits() >= 16 => Some(Request::Enemy { obj: r.read_u16() }),
        KIND_PLACE => read_pos(r).map(|pos| Request::Place { pos }),
        _ => None,
    }
}

/// The notice body after the shell id.
pub fn write_notice(w: &mut BitWriter, n: Notice) {
    match n {
        Notice::Offer { enemy_ms, place_ms } => {
            w.write_u8(KIND_OFFER)
                .write_u16(enemy_ms)
                .write_u16(place_ms);
        }
        Notice::Enemy { from, obj, ms } => {
            w.write_u8(KIND_ENEMY)
                .write_u16(from)
                .write_u16(obj)
                .write_u16(ms);
        }
        Notice::Place { from, pos, ms } => {
            w.write_u8(KIND_PLACE).write_u16(from);
            write_pos(w, pos);
            w.write_u16(ms);
        }
    }
}

/// Reads a notice; `None` when it is cut short, of an unknown kind, or not finite.
pub fn read_notice(r: &mut BitReader<'_>) -> Option<Notice> {
    if r.remaining_bits() < 8 {
        return None;
    }
    match r.read_u8() {
        KIND_OFFER if r.remaining_bits() >= 32 => Some(Notice::Offer {
            enemy_ms: r.read_u16(),
            place_ms: r.read_u16(),
        }),
        KIND_ENEMY if r.remaining_bits() >= 48 => Some(Notice::Enemy {
            from: r.read_u16(),
            obj: r.read_u16(),
            ms: r.read_u16(),
        }),
        KIND_PLACE if r.remaining_bits() >= 16 + 96 + 16 => {
            let from = r.read_u16();
            let pos = read_pos(r)?;
            Some(Notice::Place {
                from,
                pos,
                ms: r.read_u16(),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_notices_round_trip_and_junk_is_refused() {
        for q in [
            Request::Enemy { obj: 0x1234 },
            Request::Place {
                pos: [1.5, -20.0, 300.25],
            },
        ] {
            let mut b = BitWriter::new();
            write_request(&mut b, q);
            assert_eq!(read_request(&mut b.reader()), Some(q));
        }
        for n in [
            Notice::Offer {
                enemy_ms: 3000,
                place_ms: 5000,
            },
            Notice::Enemy {
                from: 7,
                obj: 9,
                ms: 3000,
            },
            Notice::Place {
                from: 7,
                pos: [0.0, 1.0, -2.0],
                ms: 5000,
            },
        ] {
            let mut b = BitWriter::new();
            write_notice(&mut b, n);
            assert_eq!(read_notice(&mut b.reader()), Some(n));
        }
        assert_eq!(read_request(&mut BitWriter::new().reader()), None);
        let mut b = BitWriter::new();
        b.write_u8(9).write_u16(1);
        assert_eq!(read_request(&mut b.reader()), None);
        let mut b = BitWriter::new();
        b.write_u8(KIND_PLACE)
            .write_f32(f32::NAN)
            .write_f32(0.0)
            .write_f32(0.0);
        assert_eq!(read_request(&mut b.reader()), None);
        let mut b = BitWriter::new();
        b.write_u8(KIND_ENEMY).write_u8(1);
        assert_eq!(read_request(&mut b.reader()), None);
        assert_ne!(UP_ID, SHELL_ID);
    }
}
