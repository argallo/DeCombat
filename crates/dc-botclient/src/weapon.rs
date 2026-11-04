//! Weapon messages of the 2009 client: the writers the shooter bot sends (`UP_WEAPON_CHANGE` 0x7f, `UP_WEAPON_EVENT`
//! 0x76 category 3, `UP_FIRE` 0x82 one per pellet) with readers for each (the capture decoder uses them on the real
//! client's traffic), and readers of the server's weapon and damage messages (0xb2 / 0xb3 give / equip, 0x74
//! damage, 0x66 status, 0x6d kill) as the bot uses them.
//!
//! Field order and widths are those of the C2S / S2C specs; the real 2009 client's own messages of capture
//! `20261025_002236_tdm4` were decoded with these readers (forms in the staged `bot-shooter_WEAPON_FORMS.md`).
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (0x64), §4.1 (0x82), §4.4 (0x7f), §4.5 (0x76 category 3)
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 (0x6d), §4.3 (0x66, 0x74), §4.4 (0xb2, 0xb3)
// OBSERVED: capture 20261025_002236_tdm4/capture.jsonl (the real client's link, conn 6)

use dc_wire::udp::{read_quat128, write_quat128, BitReader, BitWriter};

/// Client -> server shell ids of this module.
pub mod up {
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.1
    pub const FIRE: u8 = 0x82;
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.4
    pub const WEAPON_CHANGE: u8 = 0x7f;
    // SPEC: GAME_MESSAGES_2009_C2S.md §2.2, §4.5 (category 1 movement, 3 weapon, 5 deflect)
    pub const EVENT: u8 = 0x76;
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.2
    pub const RELOAD: u8 = 0x83;
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.3
    pub const WEAPON_SOUND: u8 = 0x80;
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.6 (`UP_THROW_HOLD`, no fields; the release is the next 0x82)
    pub const THROW_HOLD: u8 = 0x4d;
}

/// Server -> client shell ids of this module.
pub mod gs {
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.4
    pub const WEAPON_GIVE: u8 = 0xb2;
    pub const WEAPON_EQUIP: u8 = 0xb3;
    pub const WEAPON_REMOVE: u8 = 0xb4;
    pub const WEAPON_SWITCH: u8 = 0x7f;
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.3
    pub const PLAYER_STATUS: u8 = 0x66;
    pub const DAMAGE_TAKEN: u8 = 0x74;
}

/// `UP_WEAPON_EVENT` (0x76) category of the weapon notices; event 2 follows the select animation.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.5 (category 3, event 2 = draw / select, PLAUSIBLE)
// OBSERVED: tdm4 conn 6: every `UP_WEAPON_CHANGE` is preceded in the same instant by `76 03 02`
pub const EVENT_WEAPON: u8 = 3;
pub const WEAPON_EVENT_SELECT: u8 = 2;

fn vec96(r: &mut BitReader<'_>) -> [f32; 3] {
    [r.read_f32(), r.read_f32(), r.read_f32()]
}

fn put96(w: &mut BitWriter, v: [f32; 3]) {
    for c in v {
        w.write_f32(c);
    }
}

// --------------------------------------------------------------------------------------------- 0x7f, 0x76

/// `UP_WEAPON_CHANGE` (0x7f): `[slot:u8][weapon_id:u16][aux:u16]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeaponChange {
    pub slot: u8,
    pub weapon_id: u16,
    /// A value from the new weapon (UNPROVEN; the real client sent the ammunition id there for its guns).
    pub aux: u16,
}

// SPEC: GAME_MESSAGES_2009_C2S.md §4.4 (DISASM cshell+0x1f8ffa..0x1f902d)
pub fn weapon_change_body(c: &WeaponChange) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(c.slot).write_u16(c.weapon_id).write_u16(c.aux);
    w
}

pub fn read_weapon_change(r: &mut BitReader<'_>) -> WeaponChange {
    WeaponChange {
        slot: r.read_u8(),
        weapon_id: r.read_u16(),
        aux: r.read_u16(),
    }
}

/// `UP_WEAPON_EVENT` (0x76 category 3): `[3][event]` (events 2, 3, 5).
// SPEC: GAME_MESSAGES_2009_C2S.md §4.5
pub fn weapon_event_body(event: u8) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(EVENT_WEAPON).write_u8(event);
    w
}

// --------------------------------------------------------------------------------------------- 0x82

/// The optional `has_extra` block of `UP_FIRE` (meanings UNPROVEN).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FireExtra {
    pub obj: u16,
    pub kind: u8,
    pub a: [f32; 3],
    pub b: [f32; 3],
}

/// `UP_FIRE` (0x82), one per pellet, field names of C2S §4.1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FireReport {
    pub wkind: u8,
    pub weapon_id: u16,
    pub ammo_id: u16,
    pub weapon_pos: [f32; 3],
    pub origin: [f32; 3],
    /// Four floats (PLAUSIBLY the shot rotation as a quaternion x, y, z, w).
    pub aim: [f32; 4],
    pub yaw: f32,
    pub dir: [f32; 3],
    pub seed: u8,
    pub moving: bool,
    pub wflag: bool,
    pub mode1: bool,
    pub frac: u8,
    pub shot_no: u32,
    pub time_ms: u32,
    pub extra: Option<FireExtra>,
    pub flag_c: bool,
    pub last: bool,
    /// `(hit_dist, hit_obj)`: the hit point's distance along the ray and the object hit.
    pub hit: Option<(f32, u16)>,
    pub stat: u32,
}

/// `UP_FIRE` body after the shell id.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.1 (DISASM cshell+0x238b7c..0x238e41, order and widths PROVEN)
pub fn fire_body(f: &FireReport) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(f.wkind)
        .write_u16(f.weapon_id)
        .write_u16(f.ammo_id);
    put96(&mut w, f.weapon_pos);
    put96(&mut w, f.origin);
    write_quat128(&mut w, f.aim);
    w.write_f32(f.yaw);
    put96(&mut w, f.dir);
    w.write_u8(f.seed)
        .write_bit(f.moving)
        .write_bit(f.wflag)
        .write_bit(f.mode1)
        .write_u8(f.frac)
        .write_u32(f.shot_no)
        .write_u32(f.time_ms)
        .write_bit(f.extra.is_some());
    if let Some(x) = f.extra {
        w.write_u16(x.obj).write_u8(x.kind);
        put96(&mut w, x.a);
        put96(&mut w, x.b);
    }
    w.write_bit(f.flag_c)
        .write_bit(f.last)
        .write_bit(f.hit.is_some());
    if let Some((d, o)) = f.hit {
        w.write_f32(d).write_u16(o);
    }
    w.write_u32(f.stat);
    w
}

/// Reads `UP_FIRE` after the shell id; `None` when the message is shorter than its layout.
pub fn read_fire(r: &mut BitReader<'_>) -> Option<FireReport> {
    // fixed part up to and including has_extra
    if r.remaining_bits() < 572 {
        return None;
    }
    let wkind = r.read_u8();
    let weapon_id = r.read_u16();
    let ammo_id = r.read_u16();
    let weapon_pos = vec96(r);
    let origin = vec96(r);
    let aim = read_quat128(r);
    let yaw = r.read_f32();
    let dir = vec96(r);
    let seed = r.read_u8();
    let moving = r.read_bit();
    let wflag = r.read_bit();
    let mode1 = r.read_bit();
    let frac = r.read_u8();
    let shot_no = r.read_u32();
    let time_ms = r.read_u32();
    let extra = if r.read_bit() {
        if r.remaining_bits() < 216 {
            return None;
        }
        Some(FireExtra {
            obj: r.read_u16(),
            kind: r.read_u8(),
            a: vec96(r),
            b: vec96(r),
        })
    } else {
        None
    };
    if r.remaining_bits() < 3 {
        return None;
    }
    let flag_c = r.read_bit();
    let last = r.read_bit();
    let hit = if r.read_bit() {
        if r.remaining_bits() < 48 {
            return None;
        }
        Some((r.read_f32(), r.read_u16()))
    } else {
        None
    };
    if r.remaining_bits() < 32 {
        return None;
    }
    let stat = r.read_u32();
    Some(FireReport {
        wkind,
        weapon_id,
        ammo_id,
        weapon_pos,
        origin,
        aim,
        yaw,
        dir,
        seed,
        moving,
        wflag,
        mode1,
        frac,
        shot_no,
        time_ms,
        extra,
        flag_c,
        last,
        hit,
        stat,
    })
}

// --------------------------------------------------------------------------------------------- 0x64 (read side)

/// The head of an unreliable `UP_PLAYER_UPDATE` as the capture decoder reads it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UpdateHead {
    pub flags: u16,
    pub yaw8c: Option<u8>,
    pub body_angles: Option<[f32; 3]>,
    pub aim_q: Option<[f32; 4]>,
    pub weapon_f: Option<f32>,
    pub control: u32,
    pub pos: [f32; 3],
    pub move_seq: u8,
    /// False when the flag word announced a block this reader does not size (`yaw16c`, `view_q`).
    pub complete: bool,
}

/// Reads the live (derived) form of 0x64 after the shell id; the reliable state form returns `None`.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (layout, DISASM cshell+0x5fc0a..0x5ff80); MOVEMENT_2009.md §12.2 (trailer)
pub fn read_update_head(r: &mut BitReader<'_>) -> Option<UpdateHead> {
    if r.remaining_bits() < 16 {
        return None;
    }
    let flags = r.read_u16();
    if flags == 0x0010 && r.remaining_bits() == 8 {
        return None;
    }
    let mut u = UpdateHead {
        flags,
        complete: true,
        ..UpdateHead::default()
    };
    if flags & 0x0020 != 0 {
        u.yaw8c = Some(r.read_u8());
        u.body_angles = Some(vec96(r));
    } else if flags & 0x0040 != 0 {
        r.read_u16();
    } else if flags & 0x0080 != 0 {
        read_quat128(r);
    }
    if flags & 0x0060 != 0 {
        r.skip_bits(6 * 32);
    }
    if flags & 0x0001 != 0 {
        vec96(r);
    }
    if flags & 0x0100 != 0 {
        u.aim_q = Some(read_quat128(r));
    }
    if flags & 0x0200 != 0 {
        u.weapon_f = Some(r.read_f32());
    }
    if flags & 0x0400 != 0 {
        vec96(r);
    }
    if r.remaining_bits() < 32 + 8 + 96 {
        u.complete = false;
        return Some(u);
    }
    u.control = r.read_u32();
    u.move_seq = r.read_u8();
    u.pos = vec96(r);
    Some(u)
}

// --------------------------------------------------------------------------------------------- server readers

/// `GS_WEAPON_GIVE` (0xb2) / `GS_WEAPON_EQUIP` (0xb3) as the bot keeps them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeaponGrant {
    pub weapon: u16,
    pub ammo_kind: u16,
    pub ammo: f32,
    /// 0xb3 only.
    pub clip: Option<u32>,
    pub class_pos: u8,
    pub slot: u8,
    /// True for 0xb3 (the drawn weapon).
    pub equip: bool,
    /// The part bytes `[part1][part2][part3][finish]` (-1 = none; all -1 when the message ends before them).
    pub parts: [i8; 4],
}

/// The part bytes of a grant that carries none.
pub const NO_PARTS: [i8; 4] = [-1; 4];

/// Reads 0xb2 (`equip` = false) or 0xb3 after the shell id, with its four part bytes when the message holds them.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (0xb2 `[weapon:u16][ammo_kind:u16][ammo:f32][class_pos:u8][slot:u8]`,
// 0xb3 with `[clip:u32]` before `class_pos`; then `[part1:i8][part2:i8][part3:i8][finish:i8]`)
pub fn read_weapon_grant(r: &mut BitReader<'_>, equip: bool) -> WeaponGrant {
    let weapon = r.read_u16();
    let ammo_kind = r.read_u16();
    let ammo = r.read_f32();
    let clip = equip.then(|| r.read_u32());
    WeaponGrant {
        weapon,
        ammo_kind,
        ammo,
        clip,
        class_pos: r.read_u8(),
        slot: r.read_u8(),
        equip,
        parts: if r.remaining_bits() >= 32 {
            [0; 4].map(|_| r.read_u8() as i8)
        } else {
            NO_PARTS
        },
    }
}

/// `GS_DAMAGE_TAKEN` (0x74) fields.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageTaken {
    pub from_dir: [f32; 3],
    /// The byte before `mode`: the damage type the client's camera jar tests (the reference stack and dc-dserver repeat
    /// `zone` there).
    // EVIDENCE: cshell+0x1f7da8 (first u8 after the direction, handed to the damage-type lookup cshell+0x282c20)
    pub damage_type: u8,
    pub mode: u8,
    pub ammo_kind: u16,
    pub dmg_kind: u8,
    pub zone: u8,
    pub part: i8,
    pub damage: u32,
}

// SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (0x74 `[from_dir:vec96][unused:u8][mode:u8][ammo_kind:u16][dmg_kind:u8]
// [zone:u8][part:i8][damage:u32]`)
pub fn read_damage(r: &mut BitReader<'_>) -> DamageTaken {
    let from_dir = vec96(r);
    let damage_type = r.read_u8();
    DamageTaken {
        from_dir,
        damage_type,
        mode: r.read_u8(),
        ammo_kind: r.read_u16(),
        dmg_kind: r.read_u8(),
        zone: r.read_u8(),
        part: r.read_s8(),
        damage: r.read_u32(),
    }
}

/// `GS_PLAYER_STATUS` (0x66) value kinds the bot reads (`[kind:u8][b1:u8][b2:u8][value:f32]`; kind 1 and 0x14
/// have other layouts).
// SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (2 health, 3 armour, 0x0f maximum health, 0x10 maximum armour)
pub mod status {
    pub const AMMO: u8 = 1;
    pub const HEALTH: u8 = 2;
    pub const ARMOUR: u8 = 3;
    pub const MAX_HEALTH: u8 = 0x0f;
    pub const MAX_ARMOUR: u8 = 0x10;
    pub const MONEY: u8 = 0x14;
}

/// A decoded 0x66.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Status {
    /// kind 1: `[weapon:u16][ammo_kind:u16][ammo:f32][slot:u8][extra:u16]`.
    Ammo {
        weapon: u16,
        ammo_kind: u16,
        ammo: f32,
        slot: u8,
    },
    /// Every kind but 1 and 0x14: `[b1:u8][b2:u8][value:f32]`.
    Value {
        kind: u8,
        b1: u8,
        b2: u8,
        value: f32,
    },
    Other(u8),
}

pub fn read_status(r: &mut BitReader<'_>) -> Status {
    match r.read_u8() {
        status::AMMO => Status::Ammo {
            weapon: r.read_u16(),
            ammo_kind: r.read_u16(),
            ammo: r.read_f32(),
            slot: r.read_u8(),
        },
        status::MONEY => Status::Other(status::MONEY),
        kind => Status::Value {
            kind,
            b1: r.read_u8(),
            b2: r.read_u8(),
            value: r.read_f32(),
        },
    }
}

/// `GS_KILL` (0x6d) fields 1-13 (the revenge and co-op tail is not needed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KillNote {
    pub victim: u8,
    pub killer: u8,
    pub kill_kind: u8,
    pub weapon: u16,
    pub hit_zone: u8,
    pub f_relation: bool,
    pub f_feed2: bool,
    pub keep_hud: bool,
    pub f_feed6: bool,
    pub streak_flag: u8,
    pub feed_value: u8,
    pub no_count: bool,
    pub revenge: Option<u32>,
}

// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 (fields 1-14)
pub fn read_kill(r: &mut BitReader<'_>) -> KillNote {
    let mut k = KillNote {
        victim: r.read_u8(),
        killer: r.read_u8(),
        kill_kind: r.read_u8(),
        weapon: r.read_u16(),
        hit_zone: r.read_u8(),
        f_relation: r.read_bit(),
        f_feed2: r.read_bit(),
        keep_hud: r.read_bit(),
        f_feed6: r.read_bit(),
        streak_flag: r.read_u8(),
        feed_value: r.read_u8(),
        no_count: r.read_bit(),
        revenge: None,
    };
    if r.read_bit() {
        k.revenge = Some(r.read_u32());
    }
    k
}

/// The shooter's view of a full [`crate::shell::KillRecord`] (the session decodes 0x6d once with
/// [`crate::shell::read_kill`]).
impl From<&crate::shell::KillRecord> for KillNote {
    fn from(k: &crate::shell::KillRecord) -> Self {
        KillNote {
            victim: k.victim,
            killer: k.killer,
            kill_kind: k.kill_kind,
            weapon: k.weapon,
            hit_zone: k.hit_zone,
            f_relation: k.relation,
            f_feed2: k.feed2,
            keep_hud: k.keep_hud,
            f_feed6: k.feed6,
            streak_flag: k.streak_flag,
            feed_value: k.feed_value,
            no_count: k.no_count,
            revenge: k.revenge,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(hit: Option<(f32, u16)>, extra: bool) -> FireReport {
        FireReport {
            wkind: 0,
            weapon_id: 0x91,
            ammo_id: 0x91,
            weapon_pos: [1.0, 2.0, 3.0],
            origin: [4.0, 5.0, 6.0],
            aim: [0.0, 0.5, 0.0, 0.75],
            yaw: 1.25,
            dir: [0.0, 0.0, 1.0],
            seed: 77,
            moving: false,
            wflag: true,
            mode1: false,
            frac: 9,
            shot_no: 3,
            time_ms: 12_345,
            extra: extra.then_some(FireExtra {
                obj: 0xFFFF,
                kind: 0,
                a: [0.0; 3],
                b: [0.0; 3],
            }),
            flag_c: false,
            last: true,
            hit,
            stat: 0,
        }
    }

    #[test]
    fn fire_sizes_match_the_observed_message_lengths() {
        // OBSERVED: tdm4 conn 6: 887-bit messages (rifle, extra block + hit) and a 623-bit one (no extra, no hit),
        // both counted with the 16 bits of `[11][0x82]`
        assert_eq!(
            fire_body(&sample(Some((100.0, 5)), true)).bit_len() + 16,
            887
        );
        assert_eq!(fire_body(&sample(None, false)).bit_len() + 16, 623);
        for f in [
            sample(Some((812.5, 7)), true),
            sample(None, false),
            sample(Some((1.0, 2)), false),
        ] {
            let b = fire_body(&f);
            let mut r = b.reader();
            assert_eq!(read_fire(&mut r), Some(f));
            assert!(r.at_end());
        }
        let short = BitWriter::from_bytes(&[0; 10]);
        assert_eq!(read_fire(&mut short.reader()), None);
    }

    #[test]
    fn weapon_change_and_event_bytes() {
        // OBSERVED: tdm4 conn 6 `0b 7f 00 91 00 91 00` and `0b 76 03 02`
        let c = WeaponChange {
            slot: 0,
            weapon_id: 0x91,
            aux: 0x91,
        };
        assert_eq!(
            weapon_change_body(&c).as_bytes(),
            [0x00, 0x91, 0x00, 0x91, 0x00]
        );
        assert_eq!(read_weapon_change(&mut weapon_change_body(&c).reader()), c);
        assert_eq!(weapon_event_body(2).as_bytes(), [0x03, 0x02]);
    }

    #[test]
    fn server_readers() {
        let mut w = BitWriter::new();
        w.write_u16(0x91)
            .write_u16(0x91)
            .write_f32(120.0)
            .write_u8(0)
            .write_u8(0);
        let g = read_weapon_grant(&mut w.reader(), false);
        assert_eq!(
            (g.weapon, g.ammo_kind, g.ammo, g.slot, g.clip),
            (0x91, 0x91, 120.0, 0, None)
        );
        let mut w = BitWriter::new();
        w.write_u8(2).write_u8(0).write_u8(0).write_f32(45.0);
        assert_eq!(
            read_status(&mut w.reader()),
            Status::Value {
                kind: 2,
                b1: 0,
                b2: 0,
                value: 45.0
            }
        );
        let mut w = BitWriter::new();
        w.write_u8(0)
            .write_u8(1)
            .write_u8(1)
            .write_u16(0x91)
            .write_u8(3);
        w.write_bit(true)
            .write_bit(false)
            .write_bit(false)
            .write_bit(false);
        w.write_u8(0).write_u8(0).write_bit(false).write_bit(false);
        let k = read_kill(&mut w.reader());
        assert_eq!(
            (k.victim, k.killer, k.weapon, k.hit_zone, k.f_relation),
            (0, 1, 0x91, 3, true)
        );
        // the session decodes 0x6d once with the full reader; the shooter's note is the same fields 1-14
        assert_eq!(KillNote::from(&crate::shell::read_kill(&mut w.reader())), k);
    }
}
