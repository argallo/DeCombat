//! Readers of the object-less shell object messages (engine id 17) that carry sounds: the player sound record
//! (type 0x19) and the shot effect (type 0x06). The bot only counts them; a client plays what they name.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.1 (engine id 17 -> effect dispatcher), §8.2 (type 0x19 inline record: `[k]` then
// k = 0x12 `[u8][u32][vec96][u8][u8]`, else `[u16][u32][vec96]`; type 0x06 relay form: 424 bits after the engine id byte, the type
// byte included)

use dc_wire::udp::{BitReader, BitWriter};

/// Effect type of the player sound record.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 (0x19 `OBJ_PLAYER_SOUND`)
pub const FX_PLAYER_SOUND: u8 = 0x19;
/// Effect type of the shot effect.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 (0x06 `OBJ_SHOT_EFFECT`)
pub const FX_SHOT: u8 = 0x06;
/// `k` of the jump relay.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 relay table (a jump -> type 0x19 `k` 0x0f)
pub const SOUND_JUMP: u8 = 0x0f;
/// `k` of the throw relay (its own layout).
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 relay table (the release of a throwable -> type 0x19 `k` 0x12)
pub const SOUND_THROW: u8 = 0x12;

/// A type 0x19 record.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundRelay {
    /// The weapon sound number 1..14, [`SOUND_JUMP`] or [`SOUND_THROW`].
    pub k: u8,
    /// The weapon id (0 for a jump); for [`SOUND_THROW`] the first byte widened.
    pub weapon: u16,
    /// The player's engine client id.
    pub player: u32,
    pub pos: [f32; 3],
    /// The two trailing bytes of the throw form.
    pub extra: Option<(u8, u8)>,
}

/// A type 0x06 record (the fields a sound needs; the rest is skipped).
#[derive(Debug, Clone, PartialEq)]
pub struct ShotEffect {
    pub first: u16,
    /// The shooter's character object.
    pub shooter: u16,
    pub weapon: u16,
    pub ammo: u16,
    /// The ROM surface id of what the shot hit.
    // SPEC: GAME_MESSAGES_2009_S2C.md §8.2 shot-effect notes (1) (`a` = ROM surface id)
    pub surface: u8,
    /// `b`: bits the record clears from the client's effect flags; the 2009 client plays the fire sound only when a
    /// flag of 0x1900 survives, so a record with 0x1000 and 0x0800 set here plays no fire sound (the reference stack's blasts
    /// and the impact record of a hitting shot).
    // DISASM: cshell+0x2c696b..0x2c696f (record +0x16 -> object +0x54), cshell+0x2c8b25..0x2c8b46 (object +0x50 =
    // (ammo-row field | 0x1810) & ~object +0x54), cshell+0x2c8eeb..0x2c8ef6 (fire sound only if +0x50 & 0x1900)
    pub clear: u16,
    /// `c`: the shooter's client id; the 2009 client plays no fire sound when it equals its own id.
    // DISASM: cshell+0x2c6a63..0x2c6a66 (record +0x3c -> object +0xdc), cshell+0x2c85b2..0x2c85c3 (object +0xdd := the
    // local client id), cshell+0x2c6580 (returns when +0xdd == +0xdc)
    pub client: u8,
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub normal: [f32; 3],
    /// `e`, `f`, `g`: the `[Mod<n>]` numbers in the shooter weapon's three mod sockets (0xff = none); the 2009 client
    /// picks the silenced fire sound from the first one.
    // DISASM: cshell+0x2c68cc..0x2c68fd (`d` read as a word, `e` / `f` / `g` as bytes at create +0x48..+0x4a),
    // cshell+0x2c6a7e..0x2c6a96 (+0x48 / +0x49 / +0x4a -> shot +0xfc / +0x100 / +0x104, each looked up as a mod row by
    // cshell+0x1724e0); EVIDENCE: cshell+0x2c6580 (the mod at +0xfc picks weapon sound 9)
    pub mods: [u8; 3],
}

/// What an engine id 17 message carried.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    SoundRelay(SoundRelay),
    Shot(ShotEffect),
    /// Any other effect type (not read).
    Other(u8),
}

fn vec96(r: &mut BitReader<'_>) -> [f32; 3] {
    [r.read_f32(), r.read_f32(), r.read_f32()]
}

/// Reads the body of an engine id 17 message (the bits after the id byte). `None` when it is too short for its type.
pub fn read_effect(body: &BitWriter) -> Option<Effect> {
    let mut r = body.reader();
    if r.remaining_bits() < 8 {
        return None;
    }
    let ty = r.read_u8();
    let need = match ty {
        FX_PLAYER_SOUND => 8 + 16 + 32 + 96,
        FX_SHOT => 416,
        _ => return Some(Effect::Other(ty)),
    };
    if r.remaining_bits() < need {
        return None;
    }
    match ty {
        FX_PLAYER_SOUND => {
            let k = r.read_u8();
            if k == SOUND_THROW {
                let a = r.read_u8();
                let player = r.read_u32();
                let pos = vec96(&mut r);
                if r.remaining_bits() < 16 {
                    return None;
                }
                let b = r.read_u8();
                let c = r.read_u8();
                Some(Effect::SoundRelay(SoundRelay {
                    k,
                    weapon: a as u16,
                    player,
                    pos,
                    extra: Some((b, c)),
                }))
            } else {
                let weapon = r.read_u16();
                let player = r.read_u32();
                let pos = vec96(&mut r);
                Some(Effect::SoundRelay(SoundRelay {
                    k,
                    weapon,
                    player,
                    pos,
                    extra: None,
                }))
            }
        }
        _ => {
            let first = r.read_u16();
            let shooter = r.read_u16();
            let weapon = r.read_u16();
            let ammo = r.read_u16();
            let surface = r.read_u8();
            let clear = r.read_u16();
            let client = r.read_u8();
            let from = vec96(&mut r);
            let to = vec96(&mut r);
            let normal = vec96(&mut r);
            let _d = r.read_u8();
            let mods = [r.read_u8(), r.read_u8(), r.read_u8()];
            Some(Effect::Shot(ShotEffect {
                first,
                shooter,
                weapon,
                ammo,
                surface,
                clear,
                client,
                from,
                to,
                normal,
                mods,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> BitWriter {
        let mut w = BitWriter::new();
        for i in (0..s.len()).step_by(2) {
            w.write_u8(u8::from_str_radix(&s[i..i + 2], 16).unwrap());
        }
        w
    }

    #[test]
    fn the_reference_weapon_sound_relay_reads_bit_for_bit() {
        // OBSERVED: CAPTURE_tdm4_KILL_VALIDATION.md §8.1 (`111903910001000000b4d7ccc3518cca431e0b1ec4`, the id byte 0x11
        // removed): sound 3 of weapon 145 by player 1
        let e = read_effect(&hex("1903910001000000b4d7ccc3518cca431e0b1ec4")).unwrap();
        let Effect::SoundRelay(p) = e else {
            panic!("{e:?}")
        };
        assert_eq!((p.k, p.weapon, p.player), (3, 145, 1));
        assert!(p.pos.iter().all(|v| v.is_finite()));
        assert_eq!(p.extra, None);
    }

    #[test]
    fn throw_form_and_shot_effect_read_their_fields() {
        let mut w = BitWriter::new();
        w.write_u8(FX_PLAYER_SOUND)
            .write_u8(SOUND_THROW)
            .write_u8(6)
            .write_u32(2);
        for v in [1.0f32, 2.0, 3.0] {
            w.write_f32(v);
        }
        w.write_u8(1).write_u8(0);
        let Some(Effect::SoundRelay(p)) = read_effect(&w) else {
            panic!()
        };
        assert_eq!(
            (p.k, p.weapon, p.player, p.pos, p.extra),
            (0x12, 6, 2, [1.0, 2.0, 3.0], Some((1, 0)))
        );
        let mut w = BitWriter::new();
        w.write_u8(FX_SHOT)
            .write_u16(0xFFFF)
            .write_u16(190)
            .write_u16(145)
            .write_u16(145);
        w.write_u8(71).write_u16(0x4be0).write_u8(1);
        for v in [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 0.0, 1.0, 0.0] {
            w.write_f32(v);
        }
        w.write_u8(0).write_u8(0xff).write_u8(0xff).write_u8(0xff);
        assert_eq!(w.bit_len(), 424, "424 bits after the engine id byte");
        let Some(Effect::Shot(s)) = read_effect(&w) else {
            panic!()
        };
        assert_eq!(
            (s.shooter, s.weapon, s.surface, s.clear, s.client),
            (190, 145, 71, 0x4be0, 1)
        );
        assert_eq!(
            (s.from, s.to, s.normal),
            ([1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [0.0, 1.0, 0.0])
        );
        assert_eq!(s.mods, [0xff; 3], "the test records carry no mods");
        assert_eq!(read_effect(&hex("19")), None);
        assert_eq!(read_effect(&hex("2d00")), Some(Effect::Other(0x2d)));
    }
}
