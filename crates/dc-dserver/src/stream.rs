//! The per-tick object stream of other players (`S2C_OBJ_STREAM`, id 10, unreliable) and the removal record of
//! `S2C_OBJ_GROUP` (id 8): position (`pos48` over the world box), velocity (`vec72`) and rotation (`quat128`) of
//! each remote player's object, then the time entry.
// SPEC: UDP_SESSION_2009_B.md §4.3, §5.1, §9.1, §12.2; DC-DEFAULT: CD-65 (approved) per-tick send policy

use crate::modelobj::{anim_block_bits, write_anim_block, Tracker};
use crate::msg::{S2C_OBJ_GROUP, S2C_OBJ_STREAM};
use crate::objects::{PosBox, TIME_ENTRY_OBJ};
use dc_wire::udp::{write_quat128, yaw8_from_radians_presumed, BitWriter, Pos48, Vec72};

/// `uf` bit: position (+ optional velocity).
// SPEC: UDP_SESSION_2009_B.md §4.3 (UF_POS)
pub const UF_POS: u8 = 0x4;
/// `uf` bit: yaw-only rotation (`yaw8`).
// SPEC: UDP_SESSION_2009_B.md §4.3 (UF_YAW)
pub const UF_YAW: u8 = 0x8;
/// `uf` bit: full rotation (`quat128`), read only when `UF_YAW` is clear.
// SPEC: UDP_SESSION_2009_B.md §4.3 (UF_ROT)
pub const UF_ROT: u8 = 0x2;
/// `uf` bit: animation block (model objects only).
// SPEC: UDP_SESSION_2009_B.md §4.3 (UF_ANIM), §4.4
pub const UF_ANIM: u8 = 0x1;
/// A velocity is sent when `|v|^2` exceeds this.
// SPEC: UDP_SESSION_2009_B.md §4.3 writer rules (`has_vel = |v|^2 > 1e-5`)
pub const HAS_VEL_MIN_SQ: f32 = 1e-5;
/// Special record kind of removed objects.
// SPEC: UDP_SESSION_2009_B.md §3.2, §5.1
pub const SPECIAL_REMOVED: u8 = 3;

/// `pos48` encoder of the 2009 position service: precise rounding (`trunc(t + 0.5)`), the world box, clamped to the
/// field range (the 2009 encoder wraps at the box edge; DeCombat clamps as the addendum advises).
// SPEC: UDP_TRANSPORT_2009_addendum_disasm.md T14 (engine+0x21820, service engine+0xcb720, precise = 1)
pub fn pos48_encode(pos: [f32; 3], b: &PosBox) -> Pos48 {
    let q = |v: f32, lo: f32, hi: f32, scale: f32, top: u32| -> u32 {
        let span = hi - lo;
        if span.is_nan() || span <= 0.0 || v.is_nan() {
            return 0;
        }
        let inv = 1.0f32 / span;
        let t = (inv * (v - lo)) * scale;
        let u = (t + 0.5).trunc();
        if u <= 0.0 {
            0
        } else if u >= top as f32 {
            top
        } else {
            u as u32
        }
    };
    let ux = q(pos[0], b.min[0], b.max[0], 524_288.0, (1 << 19) - 1);
    let uy = q(pos[1], b.min[1], b.max[1], 262_144.0, (1 << 18) - 1);
    let uz = q(pos[2], b.min[2], b.max[2], 524_288.0, (1 << 19) - 1);
    Pos48 {
        px: (ux >> 3) as u16,
        py: (uy >> 2) as u16,
        pz: (uz >> 3) as u16,
    }
}

/// The client's `pos48` decode (precise, extra byte 0).
// SPEC: UDP_TRANSPORT_2009_addendum_disasm.md T14 (engine+0x21910)
pub fn pos48_decode(p: Pos48, b: &PosBox) -> [f32; 3] {
    p.decode(b.min, b.max, true, 0)
}

/// Rotation about the vertical axis as `(x, y, z, w)`.
// SPEC: UDP_TRANSPORT_2009.md §15.3 (yaw of a quaternion `atan2(2 (x z + w y), 1 - 2 (x² + y²))`)
pub fn yaw_quat(yaw: f32) -> [f32; 4] {
    let h = yaw * 0.5;
    [0.0, h.sin(), 0.0, h.cos()]
}

/// Wraps a yaw angle into (-pi, pi] for `yaw8`.
pub fn wrap_yaw(y: f32) -> f32 {
    if !y.is_finite() {
        return 0.0;
    }
    let tau = std::f32::consts::TAU;
    let mut v = y % tau;
    if v > std::f32::consts::PI {
        v -= tau;
    } else if v <= -std::f32::consts::PI {
        v += tau;
    }
    v
}

/// One stream entry of a remote object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamEntry {
    pub obj: u16,
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    /// Rotation to send (only when it changed since the last send to this client).
    pub rot: Option<[f32; 4]>,
    /// Yaw-only objects (flag 0x0800): `yaw8` in every entry instead of `quat128`.
    pub yaw8: Option<f32>,
    /// Model objects: the animation trackers (main, upper, lower).
    pub anim: Option<[Tracker; 3]>,
}

/// Writes one entry `[obj:u16][uf:u4][pos:pos48][has_vel:u1]([vel:vec72])([yaw8] | [rot:quat128])([anim])`.
// SPEC: UDP_SESSION_2009_B.md §4.3 (field order position, rotation, animation; reader engine+0xa4160; never both
// UF_YAW and UF_ROT)
pub fn write_entry(w: &mut BitWriter, e: &StreamEntry, b: &PosBox) {
    let mut uf = UF_POS;
    if e.yaw8.is_some() {
        uf |= UF_YAW;
    } else if e.rot.is_some() {
        uf |= UF_ROT;
    }
    if e.anim.is_some() {
        uf |= UF_ANIM;
    }
    w.write_u16(e.obj).write_bits(uf as u32, 4);
    pos48_encode(e.pos, b).write(w);
    let v2: f32 = e.vel.iter().map(|c| c * c).sum();
    let has_vel = v2 > HAS_VEL_MIN_SQ;
    w.write_bit(has_vel);
    if has_vel {
        Vec72::encode(e.vel).write(w);
    }
    if let Some(y) = e.yaw8 {
        // SPEC: UDP_TRANSPORT_2009.md §15.3 (yaw8 write scale UNPROVEN, dc-wire TODO_YAW8_WRITE_SCALE)
        w.write_s8(yaw8_from_radians_presumed(wrap_yaw(y)));
    } else if let Some(q) = e.rot {
        write_quat128(w, q);
    }
    if let Some(a) = &e.anim {
        write_anim_block(w, a);
    }
}

/// Bits of one entry (for the budget).
pub fn entry_bits(e: &StreamEntry) -> usize {
    let v2: f32 = e.vel.iter().map(|c| c * c).sum();
    16 + 4
        + 48
        + 1
        + if v2 > HAS_VEL_MIN_SQ { 72 } else { 0 }
        + if e.yaw8.is_some() {
            8
        } else if e.rot.is_some() {
            128
        } else {
            0
        }
        + e.anim.as_ref().map_or(0, |a| anim_block_bits(a))
}

/// Bits of the message head plus the time entry.
pub const STREAM_FIXED_BITS: usize = 8 + 16 + 4 + 32;

/// `S2C_OBJ_STREAM` with the given entries (in order, until `budget_bits`) and the time entry last. Returns the
/// message and the number of entries written.
// SPEC: UDP_SESSION_2009_B.md §4.3 (budget, time entry last), §9.1; DC-DEFAULT: CD-65 (approved)
pub fn obj_stream(
    entries: &[StreamEntry],
    server_time: f32,
    b: &PosBox,
    budget_bits: usize,
) -> (BitWriter, usize) {
    let mut w = BitWriter::new();
    w.write_u8(S2C_OBJ_STREAM);
    let mut used = STREAM_FIXED_BITS;
    let mut n = 0;
    for e in entries {
        let eb = entry_bits(e);
        if used + eb > budget_bits {
            break;
        }
        write_entry(&mut w, e, b);
        used += eb;
        n += 1;
    }
    w.write_u16(TIME_ENTRY_OBJ)
        .write_bits(0, 4)
        .write_f32(server_time);
    (w, n)
}

/// `S2C_OBJ_GROUP` with one removed-objects record `[0][3]{obj:u16}*`.
// SPEC: UDP_SESSION_2009_B.md §5.1 (writer engine+0x8d3e0, reader engine+0xa2980), §3.1 (framing)
pub fn removal_group(objs: &[u16]) -> Option<BitWriter> {
    if objs.is_empty() {
        return None;
    }
    let mut rec = BitWriter::new();
    rec.write_u8(0).write_u8(SPECIAL_REMOVED);
    for &o in objs {
        rec.write_u16(o);
    }
    let mut w = BitWriter::new();
    w.write_u8(S2C_OBJ_GROUP)
        .write_u32(rec.bit_len() as u32)
        .append(&rec);
    Some(w)
}

/// One `S2C_OBJ_GROUP` with object records and, last, a removed-objects record (left out when `gone` is empty).
// SPEC: UDP_SESSION_2009_B.md §3.1 (framing), §5.1 (the removal record is the last record of its message)
pub fn group_with_removal(records: &[BitWriter], gone: &[u16]) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(S2C_OBJ_GROUP);
    for r in records {
        w.write_u32(r.bit_len() as u32).append(r);
    }
    if !gone.is_empty() {
        let mut rec = BitWriter::new();
        rec.write_u8(0).write_u8(SPECIAL_REMOVED);
        for &o in gone {
            rec.write_u16(o);
        }
        w.write_u32(rec.bit_len() as u32).append(&rec);
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx() -> PosBox {
        PosBox {
            min: [-512.0, -64.0, -512.0],
            max: [512.0, 256.0, 512.0],
        }
    }

    #[test]
    fn pos48_round_trip_and_clamp() {
        let b = bx();
        for p in [
            [0.0, 0.0, 0.0],
            [100.5, -20.25, 300.0],
            [-511.0, 255.0, 511.0],
        ] {
            let d = pos48_decode(pos48_encode(p, &b), &b);
            // x / z cells are 1024 * 8 / 2^19 units wide, y cells 320 * 4 / 2^18
            assert!((d[0] - p[0]).abs() <= 0.02, "{p:?} -> {d:?}");
            assert!((d[1] - p[1]).abs() <= 0.006, "{p:?} -> {d:?}");
            assert!((d[2] - p[2]).abs() <= 0.02, "{p:?} -> {d:?}");
        }
        let top = pos48_encode([512.0, 256.0, 512.0], &b);
        assert_eq!(
            (top.px, top.py, top.pz),
            (0xFFFF, 0xFFFF, 0xFFFF),
            "no wrap at the edge"
        );
        let low = pos48_encode([-9999.0, -9999.0, f32::NAN], &b);
        assert_eq!((low.px, low.py, low.pz), (0, 0, 0));
    }

    #[test]
    fn stream_entries_budget_and_time_last() {
        let b = bx();
        let e = StreamEntry {
            obj: 2,
            pos: [1.0, 2.0, 3.0],
            vel: [0.0; 3],
            rot: None,
            yaw8: None,
            anim: None,
        };
        let moving = StreamEntry {
            obj: 3,
            vel: [100.0, 0.0, 0.0],
            rot: Some(yaw_quat(1.0)),
            ..e
        };
        assert_eq!(entry_bits(&e), 69);
        assert_eq!(entry_bits(&moving), 69 + 72 + 128);
        let (w, n) = obj_stream(&[e, moving], 2.5, &b, 4000);
        assert_eq!(n, 2);
        assert_eq!(w.bit_len(), STREAM_FIXED_BITS + 69 + 269);
        let mut r = w.reader();
        assert_eq!(r.read_u8(), 10);
        assert_eq!((r.read_u16(), r.read_bits(4)), (2, UF_POS as u32));
        let p = Pos48::read(&mut r);
        assert!(!r.read_bit());
        let d = pos48_decode(p, &b);
        assert!((d[0] - 1.0).abs() < 0.02);
        assert_eq!(
            (r.read_u16(), r.read_bits(4)),
            (3, (UF_POS | UF_ROT) as u32)
        );
        r.skip_bits(48);
        assert!(r.read_bit());
        let v = Vec72::read(&mut r).decode_presumed();
        assert!((v[0] - 100.0).abs() < 0.01);
        let q = dc_wire::udp::read_quat128(&mut r);
        assert!((dc_wire::udp::quat_yaw(q) - 1.0).abs() < 1e-5);
        assert_eq!(
            (r.read_u16(), r.read_bits(4), r.read_f32()),
            (0xFFFF, 0, 2.5)
        );
        assert!(r.at_end());
        let (_, n) = obj_stream(&[e, moving], 2.5, &b, STREAM_FIXED_BITS + 100);
        assert_eq!(n, 1, "the budget cuts after the first entry");
    }

    #[test]
    fn character_entries_carry_yaw8_and_the_animation_block() {
        let b = bx();
        let t = |anim, secondary| Tracker {
            anim,
            state0: true,
            state1: true,
            time_ms: 400,
            len_ms: 7200,
            secondary,
        };
        let e = StreamEntry {
            obj: 9,
            pos: [0.0; 3],
            vel: [0.0; 3],
            rot: Some(yaw_quat(1.0)),
            yaw8: Some(1.583),
            anim: Some([t(0, None), t(96, Some((1, 0))), t(6, Some((2, 1)))]),
        };
        let (w, n) = obj_stream(&[e], 1.0, &b, 4000);
        assert_eq!(n, 1);
        assert_eq!(w.bit_len(), STREAM_FIXED_BITS + entry_bits(&e));
        let mut r = w.reader();
        r.read_u8();
        // UDP_REFERENCE_FINDINGS_botclient.md reference run: player entries are `uf` 0xd (REFERENCE)
        assert_eq!((r.read_u16(), r.read_bits(4)), (9, 0xd));
        r.skip_bits(48);
        assert!(!r.read_bit());
        assert_eq!(r.read_s8(), 63);
        // the animation block follows (main tracker first)
        assert!(r.read_bit());
        assert!((wrap_yaw(7.0) - (7.0 - std::f32::consts::TAU)).abs() < 1e-5);
        assert!((wrap_yaw(-4.0) - (-4.0 + std::f32::consts::TAU)).abs() < 1e-5);
    }

    #[test]
    fn removal_record() {
        let g = removal_group(&[4, 5]).unwrap();
        let mut r = g.reader();
        assert_eq!(r.read_u8(), 8);
        assert_eq!(r.read_u32(), 48);
        assert_eq!(
            (r.read_u8(), r.read_u8(), r.read_u16(), r.read_u16()),
            (0, 3, 4, 5)
        );
        assert!(r.at_end());
        assert!(removal_group(&[]).is_none());
    }
}
