//! Object records of `S2C_OBJ_GROUP` (id 8) and the `S2C_OBJ_STREAM` (id 10) time entry, as far as slice 1 needs
//! them: the create record of the player's own object and the periodic time signal.

use crate::msg::{S2C_OBJ_GROUP, S2C_OBJ_STREAM};
use dc_wire::udp::{BitWriter, Pos48, Vec72};

/// `cf` bit: create record.
// SPEC: UDP_SESSION_2009_B.md §4.1 (CF_NEW)
pub const CF_NEW: u16 = 0x0001;
/// `cf` bit: position + velocity block.
// SPEC: UDP_SESSION_2009_B.md §4.1 (CF_POS)
pub const CF_POS: u16 = 0x0002;
/// `cf` bit: rotation block.
// SPEC: UDP_SESSION_2009_B.md §4.1 (CF_ROT)
pub const CF_ROT: u16 = 0x0004;
/// `cf` bit: flag block.
// SPEC: UDP_SESSION_2009_B.md §4.1 (CF_FLAGS)
pub const CF_FLAGS: u16 = 0x0008;
/// `cf` bits that need the second mask byte (bit 7 of the low byte is the "high byte follows" marker).
// SPEC: UDP_SESSION_2009_B.md §3.2 (writer engine+0x917c0: `cf & 0x8F00`)
pub const CF_HIGH_MASK: u16 = 0x8F00;
/// Create normalisation: these bits are cleared on a create record (`CF_FILES`, `CF_POS_SNAP`).
// SPEC: UDP_SESSION_2009_B.md §3.3
pub const CF_CREATE_CLEAR: u16 = 0x0800 | 0x0200;

/// `obj_kind` 0: plain / point object (no create extras).
// SPEC: UDP_SESSION_2009_B.md §3.3 table (OBJ_PLAIN)
pub const OBJ_PLAIN: u8 = 0;
/// `obj_kind` 1: model object (model file block).
// SPEC: UDP_SESSION_2009_B.md §3.3 table (OBJ_MODEL)
pub const OBJ_MODEL: u8 = 1;

/// Object `flags` bit: full precision (change records carry raw floats for position / velocity).
// SPEC: UDP_SESSION_2009_B.md §4.2 item 3 (0x0100)
pub const FLAG_FULL_PRECISION: u16 = 0x0100;

/// Object id of the stream's time entry.
// SPEC: UDP_SESSION_2009_B.md §4.3, §9.1
pub const TIME_ENTRY_OBJ: u16 = 0xFFFF;

/// The box the `pos48` position service decodes against. The 2009 service and its bounds are not decompiled;
/// the spec presumes the world's bounding box.
// SPEC: UDP_TRANSPORT_2009.md §15.1 (service VA 0x6e0770, bounds UNPROVEN, V17)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PosBox {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// How a create record carries the position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosForm {
    /// `CF_FLAGS` with `FLAG_FULL_PRECISION`, then raw floats: exact, independent of the UNPROVEN `pos48` bounds.
    FullPrecision,
    /// `pos48` + `vec72` over [`PosBox`] (the encoder's scale is UNPROVEN).
    Compact,
}

/// What the player's own object is created as.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateSpec {
    pub obj: u16,
    pub kind: u8,
    pub pos: [f32; 3],
    pub pos_form: PosForm,
    /// Model file ids (kind 1 only): `(model, textures)`; empty = none (a kind-1 object needs at least one).
    pub models: Vec<(u16, Vec<u16>)>,
}

/// Writes the record head: `cf` (one byte, or two with the marker bit 7) then the object id.
// SPEC: UDP_SESSION_2009_B.md §3.2 (writer engine+0x917c0)
pub fn write_record_head(w: &mut BitWriter, cf: u16, obj: u16) {
    if cf & CF_HIGH_MASK != 0 {
        w.write_u8(((cf & 0xFF) as u8) | 0x80);
        w.write_u8((cf >> 8) as u8);
    } else {
        w.write_u8((cf & 0x7F) as u8);
    }
    w.write_u16(obj);
}

/// Model file block: `[model_count:u6]`, per model `[model_file:u16]`, for the first `[sub_count:u6]` (counts
/// itself), `[tex_count:u6]` + ids, `[style_count:u4]` + ids. Counts are capped at 32 / 32 / 8.
// SPEC: UDP_SESSION_2009_B.md §3.4 (writer engine+0x90db0 / engine+0x90bc0)
pub fn write_model_block(w: &mut BitWriter, models: &[(u16, Vec<u16>)]) {
    let models = &models[..models.len().min(63)];
    w.write_bits(models.len() as u32, 6);
    for (m, (file, textures)) in models.iter().enumerate() {
        w.write_u16(*file);
        if m == 0 {
            w.write_bits(1, 6);
        }
        let tex = &textures[..textures.len().min(32)];
        w.write_bits(tex.len() as u32, 6);
        for &t in tex {
            w.write_u16(t);
        }
        w.write_bits(0, 4);
    }
}

/// The create record of one object: head, `kind_byte`, raw `create_pos`, the kind-specific part, then the change
/// fields of the remaining `cf` bits in the fixed order (flags before position). No blob, no rotation.
// SPEC: UDP_SESSION_2009_B.md §3.3 (writer engine+0x917c0, reader engine+0xa3cb0), §4.2 items 3 and 6
pub fn create_record(spec: &CreateSpec, pbox: &PosBox) -> BitWriter {
    let mut cf = (CF_NEW | CF_POS) & !CF_CREATE_CLEAR;
    if spec.pos_form == PosForm::FullPrecision {
        cf |= CF_FLAGS;
    }
    let mut w = BitWriter::new();
    write_record_head(&mut w, cf, spec.obj);
    w.write_u8(spec.kind & 0x3F);
    for c in spec.pos {
        w.write_f32(c);
    }
    if spec.kind == OBJ_MODEL {
        write_model_block(&mut w, &spec.models);
    }
    if cf & CF_FLAGS != 0 {
        // [flags:u16][flags2:u16][uflags:u32][flags3:u32]
        w.write_u16(FLAG_FULL_PRECISION)
            .write_u16(0)
            .write_u32(0)
            .write_u32(0);
    }
    match spec.pos_form {
        PosForm::FullPrecision => {
            for c in spec.pos {
                w.write_f32(c);
            }
            for _ in 0..3 {
                w.write_f32(0.0);
            }
        }
        PosForm::Compact => {
            let (p, _extra) = Pos48::encode_presumed(spec.pos, pbox.min, pbox.max);
            p.write(&mut w);
            Vec72::encode([0.0; 3]).write(&mut w);
        }
    }
    w
}

/// `S2C_OBJ_GROUP`: `[8]` then every record as `[rec_bits:u32][record]`. `None` when there is no record (the
/// 2009 host sends the message only when it carries more than its id byte).
// SPEC: UDP_SESSION_2009_B.md §3.1 (engine+0x8d200, engine+0x8fe60)
pub fn obj_group(records: &[BitWriter]) -> Option<BitWriter> {
    if records.is_empty() {
        return None;
    }
    let mut w = BitWriter::new();
    w.write_u8(S2C_OBJ_GROUP);
    for r in records {
        w.write_u32(r.bit_len() as u32).append(r);
    }
    Some(w)
}

/// `S2C_OBJ_STREAM` with only the time entry `[0xFFFF:u16][0:u4][server_time:f32]` (sent every tick to every
/// in-world client: it is the periodic time signal).
// SPEC: UDP_SESSION_2009_B.md §4.3, §9.1 (engine+0x8fe60)
pub fn obj_stream_time(server_time: f32) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(S2C_OBJ_STREAM)
        .write_u16(TIME_ENTRY_OBJ)
        .write_bits(0, 4)
        .write_f32(server_time);
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_uses_the_marker_bit_only_for_high_bits() {
        let mut w = BitWriter::new();
        write_record_head(&mut w, 0x000B, 5);
        assert_eq!(w.as_bytes(), [0x0B, 5, 0]);
        let mut w = BitWriter::new();
        write_record_head(&mut w, 0x8003, 5);
        assert_eq!(w.as_bytes(), [0x83, 0x80, 5, 0]);
    }

    #[test]
    fn full_precision_create_layout() {
        let spec = CreateSpec {
            obj: 1,
            kind: OBJ_PLAIN,
            pos: [1.0, 2.0, 3.0],
            pos_form: PosForm::FullPrecision,
            models: vec![],
        };
        let pbox = PosBox {
            min: [-10.0; 3],
            max: [10.0; 3],
        };
        let rec = create_record(&spec, &pbox);
        // head 3 bytes, kind 1, create_pos 12, flags block 12, pos 12, vel 12
        assert_eq!(rec.bit_len(), (3 + 1 + 12 + 12 + 12 + 12) * 8);
        let mut r = rec.reader();
        assert_eq!(r.read_u8(), 0x0B);
        assert_eq!(r.read_u16(), 1);
        assert_eq!(r.read_u8(), 0);
        assert_eq!([r.read_f32(), r.read_f32(), r.read_f32()], [1.0, 2.0, 3.0]);
        assert_eq!(r.read_u16(), FLAG_FULL_PRECISION);
        r.skip_bits(16 + 32 + 32);
        assert_eq!([r.read_f32(), r.read_f32(), r.read_f32()], [1.0, 2.0, 3.0]);
        assert_eq!([r.read_f32(), r.read_f32(), r.read_f32()], [0.0; 3]);
        assert!(r.at_end());
    }

    #[test]
    fn compact_create_layout_and_group_framing() {
        let spec = CreateSpec {
            obj: 9,
            kind: OBJ_PLAIN,
            pos: [0.0, 0.0, 0.0],
            pos_form: PosForm::Compact,
            models: vec![],
        };
        let pbox = PosBox {
            min: [-10.0; 3],
            max: [10.0; 3],
        };
        let rec = create_record(&spec, &pbox);
        assert_eq!(rec.bit_len(), (3 + 1 + 12) * 8 + 48 + 72);
        assert_eq!(rec.as_bytes()[0], 0x03);
        let g = obj_group(std::slice::from_ref(&rec)).unwrap();
        let mut r = g.reader();
        assert_eq!(r.read_u8(), 8);
        assert_eq!(r.read_u32() as usize, rec.bit_len());
        assert_eq!(r.read_bit_string(rec.bit_len()), rec);
        assert!(r.at_end());
        assert!(obj_group(&[]).is_none());
    }

    #[test]
    fn model_block_counts() {
        let mut w = BitWriter::new();
        write_model_block(&mut w, &[(7, vec![8, 9])]);
        let mut r = w.reader();
        assert_eq!(r.read_bits(6), 1);
        assert_eq!(r.read_u16(), 7);
        assert_eq!(r.read_bits(6), 1, "sub_count counts the entry itself");
        assert_eq!(r.read_bits(6), 2);
        assert_eq!((r.read_u16(), r.read_u16()), (8, 9));
        assert_eq!(r.read_bits(4), 0);
        assert!(r.at_end());
    }

    #[test]
    fn time_entry() {
        let w = obj_stream_time(1.5);
        assert_eq!(w.bit_len(), 8 + 16 + 4 + 32);
        let mut r = w.reader();
        assert_eq!(r.read_u8(), 10);
        assert_eq!(r.read_u16(), 0xFFFF);
        assert_eq!(r.read_bits(4), 0);
        assert_eq!(r.read_f32(), 1.5);
    }
}
