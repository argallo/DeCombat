//! Model objects (`obj_kind` 1) of `S2C_OBJ_GROUP`: the character object of every player and the weapon model
//! attached to its hand, in the create form the reference stack used (reference run `20261024_232007`, decoded by
//! `tests/reference_replay.rs`), plus the change records that make a character visible at its spawn and the
//! animation block of the stream.
//!
//! Wire layouts are the spec's (`UDP_SESSION_2009_B.md` §3.3, §3.4, §4.2, §4.4); the choice of fields, flag words
//! and blob values is REFERENCE evidence (`dserver-slice3_SPEC_ADDENDA.md`, items S3-1..);
//! the type-0x18 blob layout is the client's reader (`EVIDENCE: cshell+0x2b6aa0`).

use crate::objects::{write_record_head, PosBox, CF_FLAGS, CF_NEW, CF_POS, CF_ROT, OBJ_MODEL};
use crate::stream::pos48_encode;
use dc_wire::udp::{write_quat128, BitWriter, Vec72};

/// `cf` bits used here beyond those of [`crate::objects`].
// SPEC: UDP_SESSION_2009_B.md §4.1
pub const CF_SCALE: u16 = 0x0010;
pub const CF_ANIM: u16 = 0x0020;
pub const CF_ATTACH: u16 = 0x0100;
pub const CF_POS_SNAP: u16 = 0x0200;
pub const CF_ROT_SNAP: u16 = 0x0400;
pub const CF_FILES: u16 = 0x0800;
pub const CF_DIMS: u16 = 0x8000;
/// Server alias bits the reference stack leaves set on the wire: 0x2000 (animation, the reader tests 0x2020) on the
/// character's create record and 0x4000 (follow-up position, the reader never tests it) on its spawn record.
// SPEC: UDP_SESSION_2009_B.md §4.1 (aliases harmless if present); UDP_REFERENCE_FINDINGS_botclient.md reference run
// (create cf 0xa1ab, spawn cf 0x468a; REFERENCE)
pub const CF_ALIAS_ANIM: u16 = 0x2000;
pub const CF_ALIAS_FOLLOW_UP: u16 = 0x4000;

/// Shell object blob type of a character (the router's character client object, factory case 0x18).
// SPEC: GAME_MODES_2009.md §6.2 (type 0x18 = the type the roster looks characters up by, cshell+0x20fbe0(0x18, obj));
// EVIDENCE: cshell+0x220830 (factory), cshell+0x203c70 (slot +0x10: reads the record, then slot +0x0c init)
pub const BLOB_CHARACTER: u8 = 0x18;
/// The kind byte flags of a create record with a blob longer than 255 bits.
// SPEC: UDP_SESSION_2009_B.md §3.3 (bit 6 has_blob, bit 7 blob length 16 bits)
pub const KIND_HAS_BLOB: u8 = 0x40;
pub const KIND_BLOB_LEN16: u8 = 0x80;

/// The four flag words of a `CF_FLAGS` block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FlagWords {
    pub flags: u16,
    pub flags2: u16,
    pub uflags: u32,
    pub flags3: u32,
}

/// Flag words of a character before its first spawn and once spawned (visible), as the reference stack sent them. Bit
/// 0x0800 of `flags` makes the stream carry `yaw8` (spec), bit 0x0100 of `flags2` lets the dims block through
/// (spec); bits 0x0001 / 0x1000 are what the spawn adds (PLAUSIBLY "visible" and "solid", meanings UNPROVEN).
// SPEC: UDP_SESSION_2009_B.md §4.2 items 3 (0x0800 yaw only, 0x0100 full precision: clear here), 9 (dims need
// flags2 & 0x0100); UDP_REFERENCE_FINDINGS_botclient.md reference run (create 0x2802/0x0318/0x00401020/0, spawn 0x3803;
// REFERENCE)
pub const CHARACTER_FLAGS_HIDDEN: FlagWords = FlagWords {
    flags: 0x2802,
    flags2: 0x0318,
    uflags: 0x0040_1020,
    flags3: 0,
};
pub const CHARACTER_FLAGS_SPAWNED: FlagWords = FlagWords {
    flags: 0x3803,
    ..CHARACTER_FLAGS_HIDDEN
};
/// Flag words of the attached weapon model before and after the spawn.
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (weapon object 0x0002/0x0010/0x00000010/0, spawn 0x0003; REFERENCE)
pub const WEAPON_FLAGS_HIDDEN: FlagWords = FlagWords {
    flags: 0x0002,
    flags2: 0x0010,
    uflags: 0x0000_0010,
    flags3: 0,
};
pub const WEAPON_FLAGS_SPAWNED: FlagWords = FlagWords {
    flags: 0x0003,
    ..WEAPON_FLAGS_HIDDEN
};
/// Object flag bit: the stream sends `yaw8` instead of `quat128`.
// SPEC: UDP_SESSION_2009_B.md §4.2 item 3 (0x0800)
pub const FLAG_YAW_ONLY: u16 = 0x0800;
/// Number of (u32, u32) mask pairs after the attachment list of a model object.
// SPEC: UDP_SESSION_2009_B.md §4.2 item 8 (`masks: 32 x (u32, u32)`)
pub const ATTACH_MASK_WORDS: usize = 64;
/// Identity rotation `(x, y, z, w)`.
pub const IDENTITY_ROT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// One model of a model file block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelEntry {
    pub file: u16,
    /// Sub-model files after the model itself (written only for the first model).
    pub subs: Vec<u16>,
    pub textures: Vec<u16>,
    pub styles: Vec<u16>,
}

/// Caps of the client's model block arrays.
// SPEC: UDP_SESSION_2009_B.md §3.4 (`sub_count <= 32`, `tex_count <= 32`, `style_count <= 8`)
pub const MAX_SUBS: usize = 32;
pub const MAX_TEXTURES: usize = 32;
pub const MAX_STYLES: usize = 8;

/// Model file block: `[model_count:u6]`, per model `[file:u16]`, for the first `[sub_count:u6]` (counting the model
/// itself) and its sub files, `[tex_count:u6]` + ids, `[style_count:u4]` + ids.
// SPEC: UDP_SESSION_2009_B.md §3.4 (writer engine+0x90db0 / engine+0x90bc0)
pub fn write_models(w: &mut BitWriter, models: &[ModelEntry]) {
    let models = &models[..models.len().min(63)];
    w.write_bits(models.len() as u32, 6);
    for (m, e) in models.iter().enumerate() {
        w.write_u16(e.file);
        if m == 0 {
            let subs = &e.subs[..e.subs.len().min(MAX_SUBS - 1)];
            w.write_bits(subs.len() as u32 + 1, 6);
            for s in subs {
                w.write_u16(*s);
            }
        }
        let tex = &e.textures[..e.textures.len().min(MAX_TEXTURES)];
        w.write_bits(tex.len() as u32, 6);
        for t in tex {
            w.write_u16(*t);
        }
        let st = &e.styles[..e.styles.len().min(MAX_STYLES)];
        w.write_bits(st.len() as u32, 4);
        for s in st {
            w.write_u16(*s);
        }
    }
}

/// One animation tracker of the animation block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tracker {
    /// Animation index of the composed model (body clips, then the child models' clips).
    pub anim: u16,
    pub state0: bool,
    pub state1: bool,
    pub time_ms: u32,
    /// Length of the animation (selects the time field width).
    pub len_ms: u32,
    /// Secondary trackers: `(tracker id, weight set)`; `None` = the main tracker (written first).
    pub secondary: Option<(u8, u8)>,
}

/// Time field selector from the animation length: `n = (L + 7) >> 3`; 3 if `n >= 1024`, 2 if `n >= 64`, 1 if
/// `n > 8`, else 0. Widths 1, 6, 10, 32.
// SPEC: UDP_SESSION_2009_B.md §4.4 (writer engine+0x91600)
pub fn time_select(len_ms: u32) -> (u32, usize) {
    let n = (len_ms.saturating_add(7)) >> 3;
    let tsel = if n >= 1024 {
        3
    } else if n >= 64 {
        2
    } else if n > 8 {
        1
    } else {
        0
    };
    (tsel, [1usize, 6, 10, 32][tsel as usize])
}

/// Animation block: per tracker (main first) `[more=1][wide]`, `[anim:u14][state0]` or `[anim:u8]` (narrow when
/// `anim < 256` and `state0`), `[state1][tsel:u2][time8:W]`, secondary trackers add `[tracker:u8][wide_ws][wset:u8|u3]`;
/// then `[more=0]`. `time8 = (time_ms + 7) >> 3`, limited to its field.
// SPEC: UDP_SESSION_2009_B.md §4.4 (writer engine+0x91600, reader engine+0xa2530)
pub fn write_anim_block(w: &mut BitWriter, trackers: &[Tracker]) {
    for t in trackers {
        w.write_bit(true);
        let wide = t.anim >= 256 || !t.state0;
        w.write_bit(wide);
        if wide {
            w.write_bits(u32::from(t.anim) & 0x3FFF, 14);
            w.write_bit(t.state0);
        } else {
            w.write_bits(u32::from(t.anim), 8);
        }
        w.write_bit(t.state1);
        let (tsel, width) = time_select(t.len_ms);
        w.write_bits(tsel, 2);
        let time8 = (t.time_ms.saturating_add(7)) >> 3;
        let max = if width >= 32 {
            u32::MAX
        } else {
            (1u32 << width) - 1
        };
        w.write_bits(time8.min(max), width);
        if let Some((tracker, ws)) = t.secondary {
            w.write_u8(tracker);
            let wide_ws = ws > 7;
            w.write_bit(wide_ws);
            if wide_ws {
                w.write_u8(ws);
            } else {
                w.write_bits(u32::from(ws), 3);
            }
        }
    }
    w.write_bit(false);
}

/// Bits of an animation block.
pub fn anim_block_bits(trackers: &[Tracker]) -> usize {
    let mut w = BitWriter::new();
    write_anim_block(&mut w, trackers);
    w.bit_len()
}

/// One attachment of a model object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Attachment {
    pub child: u16,
    /// Socket index of the parent's model (the writer keeps the low 16 bits).
    pub socket: u32,
    pub offset: [f32; 3],
    pub rot: [f32; 4],
}

/// The type-0x18 character record (the blob of a character's create record). Field names are ours, by record
/// offset of the client's reader; values the reference stack sent are the defaults of [`CharacterBlob::reference`].
// EVIDENCE: cshell+0x2b6aa0 (reader: widths 1, 1, 8 x 6, 64, 32 x 6, 96, 96, 32, 32, 1, 8, 1 and, when the second
// bit is set, 8, 8, 8, 32 and a string of at most 0x101 bytes); DISASM: cshell+0x2b6ac9..0x2b6ccf (widths pushed)
#[derive(Debug, Clone, PartialEq)]
pub struct CharacterBlob {
    /// rec+0x08 (object +0x40): a roster player; the init then binds the character to the roster
    /// (cshell+0x205f40: field +0x40 set and the roster check passes). PLAUSIBLE meaning.
    pub roster_bound: bool,
    /// rec+0x0c: the NPC block follows (name, team; the init adds a marker by itself). PLAUSIBLE meaning.
    pub npc: Option<NpcBlock>,
    /// rec+0x10, +0x14, +0x20, +0x2c, +0x2d (u8; +0x14 / +0x20 = 0xff short-cut the lookup of rec+0x24).
    pub rec_10: u8,
    pub rec_14: u8,
    pub rec_20: u8,
    pub rec_2c: u8,
    pub rec_2d: u8,
    /// rec+0x34: the player id (the reference stack sent the engine client id of the player).
    pub pid: u8,
    pub rec_38: u64,
    pub rec_40: u32,
    pub rec_44: [u32; 5],
    /// rec+0x58: the reference stack sent the player box half extents here.
    pub dims: [f32; 3],
    pub rec_64: [f32; 3],
    /// rec+0x70 / +0x74 (the init copies their low halves to object +0x5ae / +0x5b0).
    pub rec_70: u32,
    pub rec_74: u32,
    pub rec_78: bool,
    pub rec_79: u8,
    /// rec+0xa4 -> object +0x5bc (copied as the infected flag, GAME_MODES_2009.md §6.2).
    pub infected: bool,
}

/// The NPC tail of a character record (read only when `has_tail`, after the infected bit), in reader order.
// SPEC: FIRETEAM_2009.md §1.2 (T1-T5: u8 +0x28, u8 +0x98, u8 +0xa0, u32 +0x9c, zstr8 of at most 0x101 bytes +0x7c;
// PROVEN order, DISASM widths cshell+0x2b6c79..0x2b6cda)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NpcBlock {
    /// T1 `npc_rank` (object +0x60): the rank-icon index of the marker entry (FIRETEAM §1.4).
    pub npc_rank: u8,
    /// T2 `npc_team` (object +0xd0): the team of the marker, radar and map entries.
    pub team: u8,
    /// T3 `npc_kind` (object +0xd8): 1 or 2 show the health bar, 2 = boss icons, 4 = infected (FIRETEAM §1.3, §1.5).
    pub kind: u8,
    /// T4 `npc_health` (object +0xd4): f32 bit pattern of the health fraction (x 100, truncated, FIRETEAM §1.3).
    pub health_bits: u32,
    /// T5 `npc_caption` (object +0xb4): the marker caption; empty = no marker entry (no caption, no bar).
    pub caption: Vec<u8>,
}

/// `pid` byte of an AI character's record (an AI has no roster id).
// DC-DEFAULT: FT-1 (proposed) `pid` 0xff
pub const NPC_PID: u8 = 0xff;
/// Longest caption the client's string reader keeps (its limit is 0x101 bytes with the terminator).
// SPEC: FIRETEAM_2009.md §1.2 T5
pub const NPC_CAPTION_MAX: usize = 0x100;

/// Values the reference stack put into the character record of a player (only `pid` and `dims` vary).
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run, both player objects (REFERENCE): roster_bound 1, no NPC block,
// u8 fields 0, 0, 0, 3, 2, pid, u64 0, rec_40 0x3f000000, five zero words, dims, zero vector, zero words, flags 0
pub const REFERENCE_REC_2C: u8 = 3;
pub const REFERENCE_REC_2D: u8 = 2;
/// The same two bytes before the player's first spawn (the record is completed by a 0xa6 sub 0x11 note then).
// SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §1.5 ((1, 255) before the spawn, (3, 2) after it; REFERENCE, ACCEPTED)
pub const PRE_SPAWN_REC_2C: u8 = 1;
pub const PRE_SPAWN_REC_2D: u8 = 0xff;
pub const REFERENCE_REC_40: u32 = 0x3f00_0000;

impl CharacterBlob {
    pub fn reference(pid: u8, dims: [f32; 3]) -> Self {
        CharacterBlob {
            roster_bound: true,
            npc: None,
            rec_10: 0,
            rec_14: 0,
            rec_20: 0,
            rec_2c: REFERENCE_REC_2C,
            rec_2d: REFERENCE_REC_2D,
            pid,
            rec_38: 0,
            rec_40: REFERENCE_REC_40,
            rec_44: [0; 5],
            dims,
            rec_64: [0.0; 3],
            rec_70: 0,
            rec_74: 0,
            rec_78: false,
            rec_79: 0,
            infected: false,
        }
    }

    /// The record of a player before (`complete` false) or after its first spawn.
    pub fn for_player(pid: u8, dims: [f32; 3], complete: bool) -> Self {
        let mut b = Self::reference(pid, dims);
        if !complete {
            b.rec_2c = PRE_SPAWN_REC_2C;
            b.rec_2d = PRE_SPAWN_REC_2D;
        }
        b
    }

    /// The record of an AI character: `roster_bound` 0 and the NPC tail (the client's NPC branch of the init), the
    /// spawned record bytes (3, 2), `pid` 0xff, the box in rec+0x58, `infected` 0.
    // SPEC: FIRETEAM_2009.md §1.2 (path selection: roster_bound 0, has_tail 1), §1.9
    // DC-DEFAULT: FT-C1 (proposed); FT-1 (proposed) pid 0xff, record bytes (3, 2)
    pub fn for_npc(dims: [f32; 3], tail: NpcBlock) -> Self {
        let mut b = Self::reference(NPC_PID, dims);
        b.roster_bound = false;
        b.npc = Some(tail);
        b
    }

    /// `[type:u8 = 0x18]` then the record in reader order.
    pub fn write(&self) -> BitWriter {
        let mut w = BitWriter::new();
        w.write_u8(BLOB_CHARACTER);
        w.write_bit(self.roster_bound).write_bit(self.npc.is_some());
        for b in [
            self.rec_10,
            self.rec_14,
            self.rec_20,
            self.rec_2c,
            self.rec_2d,
            self.pid,
        ] {
            w.write_u8(b);
        }
        w.write_u64(self.rec_38).write_u32(self.rec_40);
        for x in self.rec_44 {
            w.write_u32(x);
        }
        for v in self.dims.iter().chain(self.rec_64.iter()) {
            w.write_f32(*v);
        }
        w.write_u32(self.rec_70).write_u32(self.rec_74);
        w.write_bit(self.rec_78)
            .write_u8(self.rec_79)
            .write_bit(self.infected);
        if let Some(n) = &self.npc {
            w.write_u8(n.npc_rank)
                .write_u8(n.team)
                .write_u8(n.kind)
                .write_u32(n.health_bits);
            let cut: Vec<u8> = n
                .caption
                .iter()
                .copied()
                .take_while(|&c| c != 0)
                .take(NPC_CAPTION_MAX)
                .collect();
            w.write_zstr8(&cut);
        }
        w
    }
}

/// One object record of a model object: a create record (`blob` / `models` / `create_pos` used) or a change record.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelRecord {
    pub obj: u16,
    pub create: bool,
    /// Create: raw position of the create head (the reference stack sent 0 and placed the object by the position block).
    pub create_pos: [f32; 3],
    pub blob: Option<BitWriter>,
    /// Create: the model block; change: `CF_FILES` with the block again.
    pub models: Option<Vec<ModelEntry>>,
    pub anim: Option<Vec<Tracker>>,
    pub flags: Option<FlagWords>,
    /// `CF_SCALE`: `[sx][sy][sz]` (kind 1 writes all three).
    pub scale: Option<[f32; 3]>,
    /// Position and velocity (`pos48` / `vec72` over the world box; the full-precision flag is never set here).
    pub pos: Option<([f32; 3], [f32; 3])>,
    pub rot: Option<[f32; 4]>,
    /// Rotation as `CF_ROT_SNAP` instead of `CF_ROT`.
    pub rot_snap: bool,
    /// Position as `CF_POS_SNAP` alone instead of `CF_POS` (the same fields; the reader tests 0x0202).
    // SPEC: UDP_SESSION_2009_B.md §4.1 row 0x0200, §4.2 item 6
    pub pos_snap: bool,
    /// `CF_ATTACH`: the list (then the 64 zero mask words of a model object).
    pub attach: Option<Vec<Attachment>>,
    pub dims: Option<[f32; 3]>,
    /// Alias bits copied onto the wire ([`CF_ALIAS_ANIM`], [`CF_ALIAS_FOLLOW_UP`]).
    pub cf_alias: u16,
}

impl ModelRecord {
    /// The record's `cf`.
    pub fn cf(&self) -> u16 {
        let mut cf = self.cf_alias;
        if self.create {
            cf |= CF_NEW | CF_POS;
        } else if self.models.is_some() {
            cf |= CF_FILES;
        }
        if self.anim.is_some() {
            cf |= CF_ANIM;
        }
        if self.flags.is_some() {
            cf |= CF_FLAGS;
        }
        if self.scale.is_some() {
            cf |= CF_SCALE;
        }
        if self.pos.is_some() {
            cf |= if self.pos_snap && !self.create {
                CF_POS_SNAP
            } else {
                CF_POS
            };
        }
        if self.rot.is_some() {
            cf |= if self.rot_snap { CF_ROT_SNAP } else { CF_ROT };
        }
        if self.attach.is_some() {
            cf |= CF_ATTACH;
        }
        if self.dims.is_some() {
            cf |= CF_DIMS;
        }
        if !self.create && self.pos.is_some() && self.cf_alias & CF_ALIAS_FOLLOW_UP != 0 {
            cf |= CF_POS_SNAP;
        }
        cf
    }

    /// Writes the record (head, create part, then the change fields in the fixed order).
    // SPEC: UDP_SESSION_2009_B.md §3.2 (head), §3.3 (create), §4.2 items 1-9 (field order)
    pub fn write(&self, pbox: &PosBox) -> BitWriter {
        let cf = self.cf();
        let mut w = BitWriter::new();
        write_record_head(&mut w, cf, self.obj);
        if self.create {
            let blob_bits = self.blob.as_ref().map_or(0, |b| b.bit_len());
            let mut kb = OBJ_MODEL;
            if blob_bits > 0 {
                kb |= KIND_HAS_BLOB;
                if blob_bits > 255 {
                    kb |= KIND_BLOB_LEN16;
                }
            }
            w.write_u8(kb);
            for c in self.create_pos {
                w.write_f32(c);
            }
            if let Some(b) = self.blob.as_ref().filter(|b| b.bit_len() > 0) {
                if blob_bits > 255 {
                    w.write_u16(blob_bits.min(u16::MAX as usize) as u16);
                } else {
                    w.write_u8(blob_bits as u8);
                }
                w.append(b);
            }
            write_models(&mut w, self.models.as_deref().unwrap_or(&[]));
        } else if let Some(m) = &self.models {
            write_models(&mut w, m);
        }
        if let Some(a) = &self.anim {
            write_anim_block(&mut w, a);
        }
        if let Some(f) = self.flags {
            w.write_u16(f.flags & !0x0024)
                .write_u16(f.flags2)
                .write_u32(f.uflags)
                .write_u32(f.flags3);
        }
        // SPEC: UDP_SESSION_2009_B.md §4.2 item 5 (after the colour block, which model records never carry here)
        if let Some(sc) = self.scale {
            for c in sc {
                w.write_f32(c);
            }
        }
        if let Some((p, v)) = self.pos {
            pos48_encode(p, pbox).write(&mut w);
            Vec72::encode(v).write(&mut w);
        }
        if let Some(q) = self.rot {
            write_quat128(&mut w, q);
        }
        if let Some(list) = &self.attach {
            for a in list {
                w.write_u16(a.child).write_u32(a.socket & 0xFFFF);
                for c in a.offset {
                    w.write_f32(c);
                }
                write_quat128(&mut w, a.rot);
            }
            w.write_u16(0xFFFF);
            for _ in 0..ATTACH_MASK_WORDS {
                w.write_u32(0);
            }
        }
        if let Some(d) = self.dims {
            for c in d {
                w.write_f32(c);
            }
        }
        w
    }
}

/// The create records of a player's character and its weapon model.
#[derive(Debug, Clone, PartialEq)]
pub struct CharacterSpec {
    pub obj: u16,
    pub weapon_obj: Option<u16>,
    pub pid: u8,
    pub pos: [f32; 3],
    pub dims: [f32; 3],
    pub models: Vec<ModelEntry>,
    pub anim: Vec<Tracker>,
    /// Visible (flag words of a spawned character).
    pub spawned: bool,
    /// The character record after the first spawn (S3 `for_player`).
    pub record_complete: bool,
    /// Rotation of the create record (`CF_ROT_SNAP`) for viewers other than the owner.
    pub rot: Option<[f32; 4]>,
    pub socket: u32,
    pub weapon_model: Option<ModelEntry>,
    pub weapon_anim_len_ms: u32,
}

/// The reference stack's create form of a player character: kind 1 with the type-0x18 blob, model block, animation block,
/// flag block, position, attachment of the weapon model and dims (`cf` 0xa1ab).
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (REFERENCE); UDP_SESSION_2009_B.md §3.3, §4.2
pub fn character_create(s: &CharacterSpec) -> ModelRecord {
    ModelRecord {
        obj: s.obj,
        create: true,
        create_pos: [0.0; 3],
        blob: Some(CharacterBlob::for_player(s.pid, s.dims, s.record_complete).write()),
        models: Some(s.models.clone()),
        anim: Some(s.anim.clone()),
        flags: Some(if s.spawned {
            CHARACTER_FLAGS_SPAWNED
        } else {
            CHARACTER_FLAGS_HIDDEN
        }),
        scale: None,
        pos: Some((s.pos, [0.0; 3])),
        // SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §1.2 (remote viewers: CF_ROT_SNAP in the create; REFERENCE, ACCEPTED)
        rot: s.rot,
        rot_snap: s.rot.is_some(),
        pos_snap: false,
        attach: Some(
            s.weapon_obj
                .filter(|_| s.weapon_model.is_some())
                .map(|child| Attachment {
                    child,
                    socket: s.socket,
                    offset: [0.0; 3],
                    rot: IDENTITY_ROT,
                })
                .into_iter()
                .collect(),
        ),
        dims: Some(s.dims),
        cf_alias: CF_ALIAS_ANIM,
    }
}

/// The reference stack's create form of the attached weapon model (`cf` 0x01af): model block, a main tracker on
/// animation 0, flags, position, identity rotation, an empty attachment list.
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (REFERENCE)
pub fn weapon_create(s: &CharacterSpec) -> Option<ModelRecord> {
    let obj = s.weapon_obj?;
    let m = s.weapon_model.clone()?;
    Some(ModelRecord {
        obj,
        create: true,
        create_pos: [0.0; 3],
        blob: None,
        models: Some(vec![m]),
        anim: Some(vec![Tracker {
            anim: 0,
            state0: true,
            state1: true,
            time_ms: 0,
            len_ms: s.weapon_anim_len_ms,
            secondary: None,
        }]),
        flags: Some(if s.spawned {
            WEAPON_FLAGS_SPAWNED
        } else {
            WEAPON_FLAGS_HIDDEN
        }),
        scale: None,
        pos: Some((s.pos, [0.0; 3])),
        rot: Some(IDENTITY_ROT),
        rot_snap: false,
        pos_snap: false,
        attach: Some(Vec::new()),
        dims: None,
        cf_alias: 0,
    })
}

/// Spawn change of a character: visible flag words, position, and (not for the player's own client, CD-67) the
/// rotation as `CF_ROT_SNAP` (`cf` 0x468a as the reference stack sent it, 0x428a without the rotation).
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (REFERENCE); UDP_SESSION_2009_B.md §4.2 (own view object: no
// rotation)
// DC-DEFAULT: CD-67 (approved) the own object never gets reliable rotation changes
pub fn character_spawned(obj: u16, pos: [f32; 3], rot: Option<[f32; 4]>) -> ModelRecord {
    ModelRecord {
        obj,
        flags: Some(CHARACTER_FLAGS_SPAWNED),
        pos: Some((pos, [0.0; 3])),
        rot,
        rot_snap: true,
        cf_alias: CF_ALIAS_FOLLOW_UP,
        ..ModelRecord::default()
    }
}

/// Hide change of a character that stays in the world (an AI between two lives, FT-R1): the flag words of a
/// character before its spawn and a snapped position (the park point), no rotation.
// SPEC: FIRETEAM_2009.md §1.7 (no client path removes an AI's HUD entries; hide and reuse)
// DC-DEFAULT: FT-R1 (proposed)
pub fn character_hidden(obj: u16, park: [f32; 3]) -> ModelRecord {
    ModelRecord {
        obj,
        flags: Some(CHARACTER_FLAGS_HIDDEN),
        pos: Some((park, [0.0; 3])),
        cf_alias: CF_ALIAS_FOLLOW_UP,
        ..ModelRecord::default()
    }
}

/// Hide change of the attached weapon model (flag words before the spawn, the park point).
// DC-DEFAULT: FT-R1 (proposed)
pub fn weapon_hidden(obj: u16, park: [f32; 3]) -> ModelRecord {
    ModelRecord {
        obj,
        flags: Some(WEAPON_FLAGS_HIDDEN),
        pos: Some((park, [0.0; 3])),
        ..ModelRecord::default()
    }
}

/// A new body on a reused character object (`CF_FILES` with the model block again).
// SPEC: UDP_SESSION_2009_B.md §4.2 item 1 (kind 1: model file block again)
// DC-DEFAULT: FT-R1 (proposed) a reused AI object gets the next body by `CF_FILES`
pub fn character_model_change(obj: u16, models: Vec<ModelEntry>) -> ModelRecord {
    ModelRecord {
        obj,
        models: Some(models),
        ..ModelRecord::default()
    }
}

/// Spawn change of the weapon model (`cf` 0x000a).
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (REFERENCE)
pub fn weapon_spawned(obj: u16, pos: [f32; 3]) -> ModelRecord {
    ModelRecord {
        obj,
        flags: Some(WEAPON_FLAGS_SPAWNED),
        pos: Some((pos, [0.0; 3])),
        ..ModelRecord::default()
    }
}

/// A weapon model created again at a respawn (its old object was removed at the death): the create form of
/// [`weapon_create`] with the visible flag words.
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (weapon create `cf` 0x01af; REFERENCE); OBSERVED: tdm6 (the
// reference stack's respawn creates of the weapon model carried 0x0003 / 0x0010 / 0x00000010 / 0; REFERENCE)
// DC-DEFAULT: DSV-44 (proposed)
pub fn weapon_recreate(obj: u16, m: ModelEntry, pos: [f32; 3], anim_len_ms: u32) -> ModelRecord {
    ModelRecord {
        obj,
        create: true,
        create_pos: [0.0; 3],
        blob: None,
        models: Some(vec![m]),
        anim: Some(vec![Tracker {
            anim: 0,
            state0: true,
            state1: true,
            time_ms: 0,
            len_ms: anim_len_ms,
            secondary: None,
        }]),
        flags: Some(WEAPON_FLAGS_SPAWNED),
        scale: None,
        pos: Some((pos, [0.0; 3])),
        rot: Some(IDENTITY_ROT),
        rot_snap: false,
        pos_snap: false,
        attach: Some(Vec::new()),
        dims: None,
        cf_alias: 0,
    }
}

/// The attachment list of a character changed (`CF_ATTACH`: the list, then the 64 mask words of a model object).
// SPEC: UDP_SESSION_2009_B.md §4.2 item 8 (the reader clears the attachments, then adds one per entry)
// DC-DEFAULT: DSV-25 (proposed) a weapon model attached once the drawn weapon is known
pub fn character_attach(obj: u16, child: u16, socket: u32) -> ModelRecord {
    ModelRecord {
        obj,
        attach: Some(vec![Attachment {
            child,
            socket,
            offset: [0.0; 3],
            rot: IDENTITY_ROT,
        }]),
        ..ModelRecord::default()
    }
}

/// A new weapon model on the attached weapon object (`CF_FILES`, the model block again): what other players see
/// when the player draws another weapon.
// SPEC: UDP_SESSION_2009_B.md §4.2 item 1 (kind 1: model file block again); CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.5
// (U5 decided: 0x7f -> CF_FILES on the weapon child; REFERENCE, ACCEPTED)
// DC-DEFAULT: DSV-25 (proposed)
pub fn weapon_model_change(obj: u16, m: ModelEntry) -> ModelRecord {
    ModelRecord {
        obj,
        models: Some(vec![m]),
        ..ModelRecord::default()
    }
}

/// Shell object blob type of a corpse (the router's corpse client object, factory case 0x26).
// EVIDENCE: cshell+0x222480 (router factory, case 0x26 -> cshell+0x220770 -> constructor cshell+0x2da880, class
// table VA 0x374faa84: slot +0x10 = cshell+0x2da9b0 reads the record with cshell+0x2b6470, slot +0x0c applies it)
pub const BLOB_CORPSE: u8 = 0x26;

/// The type-0x26 corpse record. Reader order (cshell+0x2b6470): seven u8 (rec +0x08, +0x0c, +0x10, +0x14, +0x1c,
/// +0x20, +0x24), three bits (rec +0x28, +0x2a, +0x29), one u8 (rec +0x18), then two vectors through the reader's
/// compact vector slot +0x58 (rec +0x2c, +0x38). Field names are ours.
// EVIDENCE: cshell+0x2b6470 (widths 8 x 7, 1 x 3, 8, slot +0x58 twice); the slot +0x58 vectors are 72 bits each: the
// reference stack's 219-bit corpse blobs end exactly after two `vec72` fields (OBSERVED:
// capture 20261025_065016_tdm6, seven corpses); cshell+0x2daae0 / cshell+0x2dacc0 skip their
// effect lookups when rec +0x0c / +0x10 / +0x14 hold 0xff, and run at all only when the rec +0x29 bit is 0
// (cshell+0x2db050)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CorpseBlob {
    pub rec_08: u8,
    /// rec +0x0c, +0x10, +0x14: effect selectors (0xff = none).
    pub fx: [u8; 3],
    pub rec_1c: u8,
    pub rec_20: u8,
    /// rec +0x24 (the apply slot makes a bit mask `1 << value` of it; meaning UNPROVEN).
    pub rec_24: u8,
    pub bits: [bool; 3],
    pub rec_18: u8,
    /// rec +0x2c: the reference stack sent the victim's box half extents here; rec +0x38: zero.
    pub dims: [f32; 3],
    pub offset: [f32; 3],
}

impl CorpseBlob {
    /// The values the reference stack sent in its corpse records (dims = the victim's box): rec +0x08 0, the three
    /// selectors 0xff, rec +0x1c 0, rec +0x20 1 (0 once, after a head-shot kill), rec +0x24 2, the three bits 0, 0,
    /// 0 (the third 1 on the first three corpses of the match), rec +0x18 0, offset 0.
    // OBSERVED: capture 20261025_065016_tdm6 (seven corpses; REFERENCE, accepted by the real client);
    // SPEC: UNPROVEN meanings
    // DC-DEFAULT: DSV-44 (proposed) the majority values
    pub fn reference(dims: [f32; 3]) -> Self {
        CorpseBlob {
            rec_08: 0,
            fx: [0xff; 3],
            rec_1c: 0,
            rec_20: TODO_CORPSE_REC_20,
            rec_24: TODO_CORPSE_REC_24,
            bits: [false; 3],
            rec_18: 0,
            dims,
            offset: [0.0; 3],
        }
    }

    /// `[type:u8 = 0x26]` then the record in reader order (219 bits).
    pub fn write(&self) -> BitWriter {
        let mut w = BitWriter::new();
        w.write_u8(BLOB_CORPSE).write_u8(self.rec_08);
        for b in self.fx {
            w.write_u8(b);
        }
        w.write_u8(self.rec_1c)
            .write_u8(self.rec_20)
            .write_u8(self.rec_24);
        for b in self.bits {
            w.write_bit(b);
        }
        w.write_u8(self.rec_18);
        Vec72::encode(self.dims).write(&mut w);
        Vec72::encode(self.offset).write(&mut w);
        w
    }
}

/// rec +0x20 and rec +0x24 of the corpse record as the reference stack sent them (meanings not traced).
// OBSERVED: capture 20261025_065016_tdm6 (rec +0x20 1 on six of seven corpses, rec +0x24 2 on all)
pub const TODO_CORPSE_REC_20: u8 = 1;
pub const TODO_CORPSE_REC_24: u8 = 2;

/// Flag words of a corpse object as the reference stack created it.
// OBSERVED: capture 20261025_065016_tdm6 (0x1803 / 0 / 0x00600820 / 0 on all seven; REFERENCE)
pub const CORPSE_FLAGS: FlagWords = FlagWords {
    flags: 0x1803,
    flags2: 0,
    uflags: 0x0060_0820,
    flags3: 0,
};
/// Flag words of a dead player's character: the hidden form (visible and solid bits clear) with `flags2` / `uflags`
/// unchanged.
// OBSERVED: capture 20261025_065016_tdm6 (the victim's character got 0x0802 / 0x0318 /
// 0x00401020 / 0 with an empty attachment list in the tick of every kill, on both links; REFERENCE)
pub const CHARACTER_FLAGS_DEAD: FlagWords = FlagWords {
    flags: 0x0802,
    ..CHARACTER_FLAGS_HIDDEN
};

/// The change record of a player's character at its death: the dead flag words and an empty attachment list (the
/// weapon model is removed in the same group).
// SPEC: UDP_SESSION_2009_B.md §4.2 items 3 and 8 (the reader clears the attachments, then adds none)
// DC-DEFAULT: DSV-44 (proposed); OBSERVED: tdm6 form `cf` 0x0188 to the other client (REFERENCE)
pub fn character_died(obj: u16) -> ModelRecord {
    ModelRecord {
        obj,
        flags: Some(CHARACTER_FLAGS_DEAD),
        attach: Some(Vec::new()),
        ..ModelRecord::default()
    }
}

/// The create record of a corpse: kind 1 with the type-0x26 blob, the model entries of the victim (body with child
/// models, textures, styles, then the face: CLI-585; the reference stack sent the body entry only), the death clip on the main tracker (not looping), the corpse flag words,
/// scale 1, the victim's position and facing (`CF_ROT_SNAP`), an empty attachment list (`cf` 0x25bb).
// OBSERVED: capture 20261025_065016_tdm6 (corpse creates `cf` 0x25bb, 3,073 bits: one model
// entry, main tracker `state1` 0 on a death clip, scale (1, 1, 1); REFERENCE, accepted by the real client)
// DC-DEFAULT: DSV-44 (proposed)
pub fn corpse_create(
    obj: u16,
    models: Vec<ModelEntry>,
    clip: (u16, u32),
    dims: [f32; 3],
    pos: [f32; 3],
    rot: [f32; 4],
) -> ModelRecord {
    ModelRecord {
        obj,
        create: true,
        create_pos: [0.0; 3],
        blob: Some(CorpseBlob::reference(dims).write()),
        models: Some(models),
        anim: Some(vec![Tracker {
            anim: clip.0,
            state0: true,
            state1: false,
            time_ms: 0,
            len_ms: clip.1,
            secondary: None,
        }]),
        flags: Some(CORPSE_FLAGS),
        scale: Some([1.0; 3]),
        pos: Some((pos, [0.0; 3])),
        rot: Some(rot),
        rot_snap: true,
        pos_snap: false,
        attach: Some(Vec::new()),
        dims: None,
        cf_alias: CF_ALIAS_ANIM,
    }
}

/// The respawn change of a character after a death, in the reference stack's form (`cf` 0x2ba8 on the wire): the model block
/// again (`CF_FILES`), the three trackers from their start (`CF_ANIM` with its alias), the visible flag words, the
/// position through `CF_POS_SNAP` alone, no rotation (the stream carries the yaw), and the attachment list naming
/// the re-created weapon model (empty without one). The same record goes to the owner and to the other viewers.
// OBSERVED: capture 20261025_065016_tdm6 (every respawn, both links, owner included: character
// change `cf` 0x2ba8 = alias 0x2000 | CF_FILES | CF_POS_SNAP | CF_ATTACH | CF_ANIM | CF_FLAGS with the body and face
// entries, three trackers, flags 0x3803, position, attachment [weapon, socket 2], then the weapon create; seq 3758,
// 7359, 10964, 11897; REFERENCE, accepted by the real client)
// SPEC: UDP_SESSION_2009_B.md §4.2 items 1, 2, 3, 6, 8
// DC-DEFAULT: DSV-44 (amendment, proposed) the trackers' clips are ours (the main clip, then the stand clips)
pub fn character_respawned(
    obj: u16,
    pos: [f32; 3],
    models: Vec<ModelEntry>,
    trackers: Vec<Tracker>,
    weapon: Option<(u16, u32)>,
) -> ModelRecord {
    ModelRecord {
        obj,
        models: Some(models),
        anim: Some(trackers),
        flags: Some(CHARACTER_FLAGS_SPAWNED),
        pos: Some((pos, [0.0; 3])),
        pos_snap: true,
        attach: Some(
            weapon
                .map(|(child, socket)| Attachment {
                    child,
                    socket,
                    offset: [0.0; 3],
                    rot: IDENTITY_ROT,
                })
                .into_iter()
                .collect(),
        ),
        cf_alias: CF_ALIAS_ANIM,
        ..ModelRecord::default()
    }
}

/// Shell object blob type of a thrown object (the router's projectile client object, factory case 0x13).
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 row 0x13 (class-read; blob `[0x13][weapon:u8][ammo:u8][1:u8]`, REFERENCE tdm2)
pub const BLOB_PROJECTILE: u8 = 0x13;
/// The last byte of the projectile blob as the reference stack sent it (meaning not traced).
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 row 0x13; UDP_SESSION_2009_B.md §3.5 (`13 07 09 01`; REFERENCE, ACCEPTED)
pub const TODO_PROJECTILE_BLOB_FLAG: u8 = 1;
/// Flag words of a thrown object as the reference stack created it (compact position: `flags` bit 0x0100 clear; `flags2` bit
/// 0x0100 lets the dims through). `flags3` was not reported by the capture and is written 0.
// SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.4 (create 413: flags 0x1201 / 0x0100 / 0x00400020; REFERENCE, ACCEPTED);
// UDP_SESSION_2009_B.md §4.2 items 3, 9
pub const PROJECTILE_FLAGS: FlagWords = FlagWords {
    flags: 0x1201,
    flags2: 0x0100,
    uflags: 0x0040_0020,
    flags3: 0,
};
/// Box of a thrown object (no ROM key names it; the reference stack's create carried this one).
// SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.4 (`dims` (6, 6, 8); REFERENCE, ACCEPTED)
// DC-DEFAULT: DSV-39 (proposed)
pub const PROJECTILE_DIMS: [f32; 3] = [6.0, 6.0, 8.0];

/// The `[0x13][weapon:u8][ammo:u8][flag:u8]` blob of a thrown object (the low bytes of the ids: the class reader's
/// widths are not traced, the observed form has one byte each).
pub fn projectile_blob(weapon: u16, ammo: u16) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(BLOB_PROJECTILE)
        .write_u8(weapon as u8)
        .write_u8(ammo as u8)
        .write_u8(TODO_PROJECTILE_BLOB_FLAG);
    w
}

/// What a thrown object's create needs.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectileSpec {
    pub obj: u16,
    pub weapon: u16,
    pub ammo: u16,
    pub model: ModelEntry,
    pub scale: [f32; 3],
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub rot: [f32; 4],
    pub anim_len_ms: u32,
}

/// The create of a thrown object in the form the real client accepted from the reference stack: kind 1 with the type-0x13
/// blob, one model (one texture, no style), a main animation tracker, the flag words above, scale, position with
/// the launch velocity, rotation (`CF_ROT` and `CF_ROT_SNAP` both set, one quaternion), an empty attachment list
/// and the dims (`cf` 0x85bf on the wire).
// SPEC: UDP_SESSION_2009_B.md §3.3, §3.5 (thrown grenade), §4.1, §4.2; CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.4
// (create 413, `cf` 0x85bf; REFERENCE, ACCEPTED); GAME_MESSAGES_2009_S2C.md §8.2 row 0x13
// DC-DEFAULT: DSV-39 (proposed)
pub fn projectile_create(s: &ProjectileSpec) -> ModelRecord {
    ModelRecord {
        obj: s.obj,
        create: true,
        create_pos: [0.0; 3],
        blob: Some(projectile_blob(s.weapon, s.ammo)),
        models: Some(vec![s.model.clone()]),
        anim: Some(vec![Tracker {
            anim: 0,
            state0: true,
            state1: true,
            time_ms: 0,
            len_ms: s.anim_len_ms,
            secondary: None,
        }]),
        flags: Some(PROJECTILE_FLAGS),
        scale: Some(s.scale),
        pos: Some((s.pos, s.vel)),
        rot: Some(s.rot),
        rot_snap: false,
        pos_snap: false,
        attach: Some(Vec::new()),
        dims: Some(PROJECTILE_DIMS),
        // the second rotation bit of the reference stack's record (the reader tests 0x0404 and reads one quaternion)
        cf_alias: CF_ROT_SNAP,
    }
}

/// Shell object blob type of a pickup (the router's pickup client object, factory case 0x17) and the pickup kind of
/// a HiSec case.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 row 0x17 (`[3 x bool1][name:zstr8(256)][kind:u32]`, kind 5 `[case_key:u32]`);
// HISEC_2009.md §3.1 (DISASM cshell+0x2181c8..0x218317)
pub const BLOB_PICKUP: u8 = 0x17;
pub const PICKUP_KIND_HISEC: u32 = 5;
/// The three leading flags of the case's pickup blob as the reference stack sent them (the HiSec code does not read them).
// SPEC: HISEC_2009.md §3.1 rows 1-3 (`flag_a`..`flag_c`, not used by the HiSec code; DISASM)
// OBSERVED: capture 20261025_065016_tdm6 c6 seq 3015 (blob `1701280000000800000000`, 83 bits:
// flags 1, 0, 0, empty FX name, kind 5, key 1); REFERENCE, accepted by the real client
pub const TODO_CASE_BLOB_FLAGS: [bool; 3] = [true, false, false];
/// Flag words of the case object as the reference stack created it (meanings of the bits not traced).
// OBSERVED: capture 20261025_065016_tdm6 c6 seq 3015, 6603, 10134 and
// 20261025_002236_tdm4 c6 seq 3132 (every case create: 0x0001 / 0x0000 / 0x004000a0 / 0); REFERENCE, accepted
pub const CASE_FLAGS: FlagWords = FlagWords {
    flags: 0x0001,
    flags2: 0,
    uflags: 0x0040_00a0,
    flags3: 0,
};
/// The case's single animation tracker: animation 0 with the time field of width 10 (`tsel` 2) and time step 5,
/// as in the reference stack's creates (the case model's clip length is not read; any length of 505..8184 ms gives `tsel` 2).
// OBSERVED: as [`CASE_FLAGS`] (tracker anim 0, state bits 1 / 1, tsel 2, time8 5)
pub const TODO_CASE_ANIM_LEN_MS: u32 = 1_000;
pub const TODO_CASE_ANIM_TIME_MS: u32 = 40;

/// The type-0x17 pickup blob of a HiSec case: `[0x17][1][0][0][""][5][case_key]` (83 bits).
// SPEC: HISEC_2009.md §3.1; GAME_MESSAGES_2009_S2C.md §8.2 row 0x17
pub fn case_blob(case_key: u32) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(BLOB_PICKUP);
    for f in TODO_CASE_BLOB_FLAGS {
        w.write_bit(f);
    }
    // empty FX name: no client FX on the case (HISEC_2009 §3.1 row 4)
    w.write_u8(0);
    w.write_u32(PICKUP_KIND_HISEC).write_u32(case_key);
    w
}

/// What the create of a HiSec case needs.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseSpec {
    pub obj: u16,
    pub case_key: u32,
    /// The case model with its skin and render style (`[Hisec<n>]` row of the ROM case table).
    pub model: ModelEntry,
    pub scale: [f32; 3],
    pub pos: [f32; 3],
    pub rot: [f32; 4],
}

/// The create of a HiSec case in the form the real client accepted from the reference stack: kind 1 with the type-0x17 kind-5
/// blob, one model (one skin, one render style), one animation tracker, the flag words above, scale, position with
/// no velocity, rotation (`CF_ROT_SNAP`), an empty attachment list, no dims (`cf` 0x053b on the wire).
// SPEC: UDP_SESSION_2009_B.md §3.3, §4.2; HISEC_2009.md §3.1
// OBSERVED: capture 20261025_065016_tdm6 c6 seq 3015 (`cf` 0x053b, kind byte 0x41, create
// position 0, the fields in this order; REFERENCE, accepted by the real client, which hid, showed and drew the case)
// DC-DEFAULT: HS-2 (proposed)
pub fn case_create(s: &CaseSpec) -> ModelRecord {
    ModelRecord {
        obj: s.obj,
        create: true,
        create_pos: [0.0; 3],
        blob: Some(case_blob(s.case_key)),
        models: Some(vec![s.model.clone()]),
        anim: Some(vec![Tracker {
            anim: 0,
            state0: true,
            state1: true,
            time_ms: TODO_CASE_ANIM_TIME_MS,
            len_ms: TODO_CASE_ANIM_LEN_MS,
            secondary: None,
        }]),
        flags: Some(CASE_FLAGS),
        scale: Some(s.scale),
        pos: Some((s.pos, [0.0; 3])),
        rot: Some(s.rot),
        rot_snap: true,
        pos_snap: false, // the case create uses CF_POS (dserver-reddot's CF_POS_SNAP form is for respawn records)
        attach: Some(Vec::new()),
        dims: None,
        cf_alias: 0,
    }
}

/// `pickup_kind` of a Spy Hunt intel case.
// SPEC: GAME_MODES_2009.md §4.4 (kind 3 `[flag:bool1]`, shown only in class 9)
pub const PICKUP_KIND_INTEL: u32 = 3;
/// The type-0x17 pickup blob of an intel case: `[0x17][0][0][1][""][3][flag]`. The third leading flag lets the case
/// fall to the floor below its spawn point (as a dropped weapon); `flag` (+0x8c) is not read by the class-9 radar
/// and map icons.
// SPEC: GAME_MODES_2009.md §4.4 (kind 3); EVIDENCE: cshell+0xb8990 (the radar icon for every kind-3 pickup in class 9)
// DC-DEFAULT: DSV-230 (proposed) the falling form, `flag` 0
pub fn intel_blob() -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(BLOB_PICKUP);
    for f in DROP_BLOB_FLAGS {
        w.write_bit(f);
    }
    w.write_u8(0);
    w.write_u32(PICKUP_KIND_INTEL).write_bit(false);
    w
}

/// The create of an intel case: the HiSec case's form (one model, one tracker, its flag words) with the kind-3 blob.
// DC-DEFAULT: DSV-230 (proposed)
pub fn intel_create(s: &CaseSpec) -> ModelRecord {
    ModelRecord {
        blob: Some(intel_blob()),
        ..case_create(s)
    }
}

/// `pickup_kind` of a weapon pickup: the record goes on with `[weapon:u16][rounds:u16]`; the client looks the weapon
/// up in its weapon table, shows the row's third-person model and hides one model piece the row names when
/// `rounds` is 0.
// SPEC: GAME_MODES_2009.md §4.4 (kind 6 `[item:u16][b:u16]`); GAME_MESSAGES_2009_S2C.md §8.2 row 0x17
// DISASM: cshell+0x2824ff..0x282581 (kind 6: the weapon row by the first u16 through cshell+0x172300; the piece named
// by row +0x464, read in cshell+0x17bd20 at cshell+0x17e623, is hidden through the model interface slot +0x2c when
// the second u16 is 0)
pub const PICKUP_KIND_WEAPON: u32 = 6;
/// The three leading flags of a dropped weapon's blob (`[0][0][1]`): the third is the client object's "falling"
/// flag (object +0x40): while it is set the client moves the object down each frame, turning it, until a segment
/// test hits the world, then clears it; the first (object +0x38) would spin the object (the HiSec case sets it).
// DISASM: cshell+0x2824ba..0x2824c9 (record +0x08 / +0x0c / +0x10 -> object +0x38 / +0x3c / +0x40),
// cshell+0x2826a6..0x28271e (+0x38: rotation by the frame time), cshell+0x282721..0x282872 (+0x40: the fall, the
// segment test at cshell+0x2827d5, cleared at cshell+0x2827e3)
// OBSERVED: capture 20261025_065016_tdm6 c5 / c6 and 20261025_002236_tdm4 c5 / c6 (every
// dropped weapon placed at the body carried 0, 0, 1; the reference stack's parked pickups 0, 0, 0); REFERENCE, accepted by the
// real client
pub const DROP_BLOB_FLAGS: [bool; 3] = [false, false, true];
/// Flag words of a dropped weapon as the reference stack created it (meanings of the bits not traced).
// OBSERVED: capture 20261025_065016_tdm6 c6 seq 3015, 6603, 10134, 11052, 16308, 20679, 23319
// and 20261025_002236_tdm4 c6 seq 3132 (every dropped weapon at the body: 0x1001 / 0x0000 / 0x006000a0 / 0); REFERENCE
pub const DROP_FLAGS: FlagWords = FlagWords {
    flags: 0x1001,
    flags2: 0,
    uflags: 0x0060_00a0,
    flags3: 0,
};
/// The dropped weapon's single animation tracker: animation 1 of the third-person weapon model at time 0, the
/// narrowest time field (meaning of animation 1 UNPROVEN; every reference drop used it).
// OBSERVED: as [`DROP_FLAGS`] (tracker anim 1, state bits 1 / 1, tsel 0, time 0)
pub const TODO_DROP_ANIM: u16 = 1;
pub const TODO_DROP_ANIM_LEN_MS: u32 = 0;

/// The type-0x17 pickup blob of a dropped weapon: `[0x17][0][0][1][""][6][weapon:u16][rounds:u16]` (83 bits).
// SPEC: GAME_MODES_2009.md §4.4; GAME_MESSAGES_2009_S2C.md §8.2 row 0x17
pub fn weapon_drop_blob(weapon: u16, rounds: u16) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(BLOB_PICKUP);
    for f in DROP_BLOB_FLAGS {
        w.write_bit(f);
    }
    // empty FX name (no client FX on the weapon)
    w.write_u8(0);
    w.write_u32(PICKUP_KIND_WEAPON)
        .write_u16(weapon)
        .write_u16(rounds);
    w
}

/// The pickup kind of a co-op AI pack (DSV-591): a kind without fields, so the client keeps the record's fields and
/// draws the object by its model block only (no weapon row is looked up).
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 row 0x17 (fields only for kinds 2, 3, 5, 6)
// DISASM: cshell+0x2824ff (only kind 6 looks a weapon row up; every kind keeps record +0x30..+0x48)
// OBSERVED: capture 20261025_065016_tdm6 c5 / c6 (the reference stack's kind-0 pickups were accepted by the real
// client; REFERENCE)
// DC-DEFAULT: DSV-591 (proposed)
pub const PICKUP_KIND_PACK: u32 = 0;

/// The type-0x17 blob of an AI pack: `[0x17][0][0][1][""][0]` (51 bits).
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 row 0x17
// DC-DEFAULT: DSV-591 (proposed) the fall flag of a dropped weapon ([`DROP_BLOB_FLAGS`])
pub fn pack_drop_blob() -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(BLOB_PICKUP);
    for f in DROP_BLOB_FLAGS {
        w.write_bit(f);
    }
    w.write_u8(0);
    w.write_u32(PICKUP_KIND_PACK);
    w
}

/// The pack model's tracker: its first animation (a prop model has no weapon animation 1).
// DC-DEFAULT: DSV-591 (proposed)
pub const PACK_DROP_ANIM: u16 = 0;

/// The create of an AI pack: the dropped weapon's form ([`weapon_drop_create`]) with the kind-0 blob and the
/// model's first animation; `s.weapon` and `s.rounds` are not sent.
// DC-DEFAULT: DSV-591 (proposed)
pub fn pack_drop_create(s: &DropSpec) -> ModelRecord {
    let mut r = weapon_drop_create(s);
    r.blob = Some(pack_drop_blob());
    if let Some(t) = r.anim.as_mut().and_then(|a| a.first_mut()) {
        t.anim = PACK_DROP_ANIM;
    }
    r
}

/// What the create of a dropped weapon needs.
#[derive(Debug, Clone, PartialEq)]
pub struct DropSpec {
    pub obj: u16,
    pub weapon: u16,
    /// Rounds the pickup carries (0 hides the weapon's piece on the client).
    pub rounds: u16,
    /// The weapon's third-person model with its skin and render style (ROM weapon row).
    pub model: ModelEntry,
    pub pos: [f32; 3],
    pub rot: [f32; 4],
}

/// The create of a dropped weapon in the form the real client accepted from the reference stack: kind 1 with the type-0x17
/// kind-6 blob, one model (one skin, one render style), one tracker, the flag words above, scale 1, position with
/// no velocity, rotation (`CF_ROT_SNAP`), an empty attachment list, no dims (`cf` 0x053b, 0x05bb on the wire).
// SPEC: UDP_SESSION_2009_B.md §3.3, §4.2; GAME_MODES_2009.md §4.4
// OBSERVED: capture 20261025_065016_tdm6 c6 seq 3015 (object 259, weapon 145) and the drops
// listed at [`DROP_FLAGS`] (REFERENCE, accepted by the real client; rebuilt bit for bit by the capture-gated
// `round2_rom::reference_weapon_drop_records_rebuild_bit_for_bit`)
// DC-DEFAULT: DSV-93 (proposed)
pub fn weapon_drop_create(s: &DropSpec) -> ModelRecord {
    ModelRecord {
        obj: s.obj,
        create: true,
        create_pos: [0.0; 3],
        blob: Some(weapon_drop_blob(s.weapon, s.rounds)),
        models: Some(vec![s.model.clone()]),
        anim: Some(vec![Tracker {
            anim: TODO_DROP_ANIM,
            state0: true,
            state1: true,
            time_ms: 0,
            len_ms: TODO_DROP_ANIM_LEN_MS,
            secondary: None,
        }]),
        flags: Some(DROP_FLAGS),
        scale: Some([1.0; 3]),
        pos: Some((s.pos, [0.0; 3])),
        rot: Some(s.rot),
        rot_snap: true,
        pos_snap: false,
        attach: Some(Vec::new()),
        dims: None,
        cf_alias: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The case create equals the reference stack's record of tdm6 c6 seq 3015 (object 258, key 1) up to its position and
    /// rotation, which are placed by the rule (HS-2): head, kind byte, blob, model block, tracker, flag words, scale,
    /// the zero velocity, the empty attachment list.
    // OBSERVED: capture 20261025_065016_tdm6 c6 seq 3015 (REFERENCE, accepted by the real client)
    #[test]
    fn case_create_has_the_observed_record() {
        let hx = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(hx(case_blob(1).as_bytes()), "1701280000000800000000");
        assert_eq!(case_blob(1).bit_len(), 83);
        assert_eq!(hx(case_blob(6).as_bytes()), "1701280000003000000000");
        let pbox = PosBox {
            min: [-1000.0; 3],
            max: [1000.0; 3],
        };
        let rec = case_create(&CaseSpec {
            obj: 258,
            case_key: 1,
            model: ModelEntry {
                file: 24,
                subs: Vec::new(),
                textures: vec![23],
                styles: vec![11],
            },
            scale: [1.0; 3],
            pos: [0.0; 3],
            rot: IDENTITY_ROT,
        });
        assert_eq!(rec.cf(), 0x053b);
        let w = rec.write(&pbox);
        // the reference stack's record up to the position: head bb 05, object 0x0102, kind 0x41, zero create position, blob
        // length 0x53, the blob, then model / tracker / flags / scale
        let reference_head = "bb05020141000000000000000000000000531701280000000800000008300082e0022016000268010200000040018000000000000000007f0000007f0000007f";
        let ours = hx(w.as_bytes());
        assert_eq!(&ours[..reference_head.len()], reference_head);
        // the scale ends at bit 513 (its last bit, the sign of z, is 0 as the hex prefix's next bit); after the
        // position (48 bits) the zero velocity (72), the rotation (128), the attachment end 0xFFFF and 64 zero masks
        let mut r = w.reader();
        r.skip_bits(512);
        assert!(!r.read_bit());
        r.skip_bits(48);
        assert_eq!(r.read_bit_string(72).as_bytes(), &[0u8; 9]);
        dc_wire::udp::read_quat128(&mut r);
        assert_eq!(r.read_u16(), 0xFFFF);
        for _ in 0..ATTACH_MASK_WORDS {
            assert_eq!(r.read_u32(), 0);
        }
        assert_eq!(r.remaining_bits(), 0);
    }

    /// DSV-93: the dropped weapon's blob is `[0x17][0][0][1][""][6][weapon][rounds]` (83 bits) and its create uses
    /// the case's field set (`cf` 0x053b) with the drop's flag words and tracker, read back field by field.
    #[test]
    fn pack_blob_is_kind_0_without_fields() {
        let b = pack_drop_blob();
        assert_eq!(b.bit_len(), 8 + 3 + 8 + 32);
        let mut r = b.reader();
        assert_eq!(r.read_u8(), BLOB_PICKUP);
        assert_eq!([r.read_bit(), r.read_bit(), r.read_bit()], DROP_BLOB_FLAGS);
        assert_eq!(r.read_u8(), 0, "no FX name");
        assert_eq!(r.read_u32(), PICKUP_KIND_PACK);
        assert!(r.at_end());
    }

    #[test]
    fn weapon_drop_create_has_the_kind_6_blob() {
        let b = weapon_drop_blob(145, 120);
        assert_eq!(b.bit_len(), 83);
        let mut r = b.reader();
        assert_eq!(r.read_u8(), BLOB_PICKUP);
        assert_eq!(
            (r.read_bit(), r.read_bit(), r.read_bit()),
            (false, false, true)
        );
        assert_eq!(r.read_u8(), 0, "empty FX name");
        assert_eq!(r.read_u32(), PICKUP_KIND_WEAPON);
        assert_eq!((r.read_u16(), r.read_u16()), (145, 120));
        assert_eq!(r.remaining_bits(), 0);
        let rec = weapon_drop_create(&DropSpec {
            obj: 259,
            weapon: 145,
            rounds: 120,
            model: ModelEntry {
                file: 21,
                subs: Vec::new(),
                textures: vec![20],
                styles: vec![11],
            },
            pos: [1.0, 2.0, 3.0],
            rot: IDENTITY_ROT,
        });
        assert_eq!(rec.cf(), 0x053b);
        let pbox = PosBox {
            min: [-1000.0; 3],
            max: [1000.0; 3],
        };
        let w = rec.write(&pbox);
        let mut r = w.reader();
        // head: cf (two bytes), object, kind byte with the blob flag, zero create position, blob length
        r.skip_bits(16);
        assert_eq!(r.read_u16(), 259);
        assert_eq!(r.read_u8(), 0x41);
        r.skip_bits(96);
        assert_eq!(r.read_u8(), 83);
        assert_eq!(r.read_bit_string(83).as_bytes(), b.as_bytes());
        // one model, no sub, one texture, one style
        assert_eq!(r.read_bits(6), 1);
        assert_eq!(r.read_u16(), 21);
        assert_eq!(r.read_bits(6), 1);
        assert_eq!(r.read_bits(6), 1);
        assert_eq!(r.read_u16(), 20);
        assert_eq!(r.read_bits(4), 1);
        assert_eq!(r.read_u16(), 11);
        // tracker: more, narrow, anim 1, state1, tsel 0 (one time bit), end
        assert!(r.read_bit());
        assert!(!r.read_bit());
        assert_eq!(r.read_bits(8), u32::from(TODO_DROP_ANIM));
        assert!(r.read_bit());
        assert_eq!(r.read_bits(2), 0);
        r.skip_bits(1);
        assert!(!r.read_bit());
        assert_eq!(
            (r.read_u16(), r.read_u16(), r.read_u32(), r.read_u32()),
            (0x1001, 0, 0x0060_00a0, 0)
        );
    }

    #[test]
    fn projectile_create_has_the_observed_head_and_blob() {
        let s = ProjectileSpec {
            obj: 413,
            weapon: 7,
            ammo: 9,
            model: ModelEntry {
                file: 25,
                subs: Vec::new(),
                textures: vec![24],
                styles: Vec::new(),
            },
            scale: [2.0; 3],
            pos: [10.0, 20.0, 30.0],
            vel: [300.0, -1500.0, 450.0],
            rot: IDENTITY_ROT,
            anim_len_ms: 0,
        };
        let rec = projectile_create(&s);
        assert_eq!(
            rec.cf() | 0x0080,
            0x85bf,
            "the reference stack's create mask"
        );
        let b = projectile_blob(7, 9);
        assert_eq!(b.bit_len(), 32);
        let mut r = b.reader();
        assert_eq!(
            (r.read_u8(), r.read_u8(), r.read_u8(), r.read_u8()),
            (0x13, 7, 9, 1)
        );
        let w = rec.write(&pbox());
        let mut r = w.reader();
        // head: cf low byte with the marker, high byte, id; kind byte 0x41 (kind 1, 8-bit blob length 32)
        assert_eq!((r.read_u8(), r.read_u8(), r.read_u16()), (0xbf, 0x85, 413));
        assert_eq!(r.read_u8(), 0x41);
        for _ in 0..3 {
            assert_eq!(r.read_f32(), 0.0);
        }
        assert_eq!(r.read_u8(), 32);
    }

    fn pbox() -> PosBox {
        PosBox {
            min: [-1000.0; 3],
            max: [1000.0; 3],
        }
    }

    #[test]
    fn character_blob_is_580_bits() {
        let b = CharacterBlob::reference(1, [15.0, 50.0, 15.0]).write();
        // EVIDENCE: cshell+0x2b6aa0 read widths without the NPC block = 572 bits, plus the type byte
        assert_eq!(b.bit_len(), 580);
        let mut r = b.reader();
        assert_eq!(r.read_u8(), BLOB_CHARACTER);
        assert!(r.read_bit());
        assert!(!r.read_bit());
        let u: Vec<u8> = (0..6).map(|_| r.read_u8()).collect();
        assert_eq!(u, vec![0, 0, 0, 3, 2, 1]);
        let npc = CharacterBlob {
            npc: Some(NpcBlock {
                caption: b"x".to_vec(),
                ..NpcBlock::default()
            }),
            ..CharacterBlob::reference(0, [1.0; 3])
        }
        .write();
        assert_eq!(npc.bit_len(), 580 + 24 + 32 + 16);
    }

    /// The AI record against the field table of FIRETEAM_2009.md §1.2: `roster_bound` 0, `has_tail` 1, the fixed
    /// part as for a player (572 bits) with `pid` 0xff, then T1 `npc_rank` u8, T2 `npc_team` u8, T3 `npc_kind` u8,
    /// T4 `npc_health` u32 (f32 bits), T5 the caption as zstr8: `572 + 56 + 8 x (len + 1)` bits after the type byte.
    #[test]
    fn npc_record_follows_the_fireteam_field_table() {
        let tail = NpcBlock {
            npc_rank: 4,
            team: 2,
            kind: 1,
            health_bits: 1.005f32.to_bits(),
            caption: b"Grunt".to_vec(),
        };
        let b = CharacterBlob::for_npc([15.0, 50.0, 15.0], tail).write();
        assert_eq!(b.bit_len(), 8 + 572 + 56 + 8 * (5 + 1));
        let mut r = b.reader();
        assert_eq!(r.read_u8(), BLOB_CHARACTER);
        assert!(!r.read_bit(), "roster_bound 0");
        assert!(r.read_bit(), "has_tail 1");
        let u: Vec<u8> = (0..6).map(|_| r.read_u8()).collect();
        assert_eq!(u, vec![0, 0, 0, 3, 2, NPC_PID]);
        r.skip_bits(64 + 6 * 32);
        let dims = [r.read_f32(), r.read_f32(), r.read_f32()];
        assert_eq!(dims, [15.0, 50.0, 15.0]);
        r.skip_bits(96 + 32 + 32 + 1 + 8);
        assert!(!r.read_bit(), "infected 0");
        assert_eq!((r.read_u8(), r.read_u8(), r.read_u8()), (4, 2, 1));
        let health = f32::from_bits(r.read_u32());
        assert_eq!((health * 100.0) as i32, 100);
        assert_eq!(r.read_zstr8(), b"Grunt".to_vec());
        assert!(r.at_end());
        // the caption is cut to the reader's limit
        let long = CharacterBlob::for_npc(
            [1.0; 3],
            NpcBlock {
                caption: vec![b'a'; 400],
                ..NpcBlock::default()
            },
        )
        .write();
        assert_eq!(long.bit_len(), 8 + 572 + 56 + 8 * (NPC_CAPTION_MAX + 1));
    }

    #[test]
    fn hidden_and_model_change_records() {
        let h = character_hidden(300, [0.0, -900.0, 0.0]);
        // flags and a snapped position, no rotation
        assert_eq!(h.cf(), CF_FLAGS | CF_POS | CF_POS_SNAP | CF_ALIAS_FOLLOW_UP);
        assert_eq!(h.flags, Some(CHARACTER_FLAGS_HIDDEN));
        let w = weapon_hidden(301, [0.0; 3]);
        assert_eq!(w.cf(), CF_FLAGS | CF_POS);
        assert_eq!(w.flags, Some(WEAPON_FLAGS_HIDDEN));
        let m = character_model_change(300, vec![ModelEntry::default()]);
        assert_eq!(m.cf(), CF_FILES);
        let _ = (h.write(&pbox()), w.write(&pbox()), m.write(&pbox()));
    }

    #[test]
    fn anim_block_forms() {
        assert_eq!(time_select(0), (0, 1));
        assert_eq!(time_select(64), (0, 1));
        assert_eq!(time_select(72), (1, 6));
        assert_eq!(time_select(505), (2, 10));
        assert_eq!(time_select(8185), (3, 32));
        let t = |anim, time_ms, secondary| Tracker {
            anim,
            state0: true,
            state1: true,
            time_ms,
            len_ms: 7200,
            secondary,
        };
        let mut w = BitWriter::new();
        write_anim_block(
            &mut w,
            &[
                t(0, 0, None),
                t(300, 80, Some((1, 0))),
                t(5, 9000, Some((2, 9))),
            ],
        );
        let mut r = w.reader();
        // main: more, narrow, anim 0, state1, tsel 2, time 0
        assert!(r.read_bit() && !r.read_bit());
        assert_eq!(r.read_bits(8), 0);
        assert!(r.read_bit());
        assert_eq!((r.read_bits(2), r.read_bits(10)), (2, 0));
        // wide anim 300 + state0, tracker 1, narrow weight set 0
        assert!(r.read_bit() && r.read_bit());
        assert_eq!(r.read_bits(14), 300);
        assert!(r.read_bit() && r.read_bit());
        assert_eq!((r.read_bits(2), r.read_bits(10)), (2, 10));
        assert_eq!((r.read_u8(), r.read_bit(), r.read_bits(3)), (1, false, 0));
        // narrow anim 5, time clamped to the field, wide weight set 9
        assert!(r.read_bit() && !r.read_bit());
        assert_eq!(r.read_bits(8), 5);
        assert!(r.read_bit());
        assert_eq!((r.read_bits(2), r.read_bits(10)), (2, 1023));
        assert_eq!((r.read_u8(), r.read_bit(), r.read_u8()), (2, true, 9));
        assert!(!r.read_bit());
        assert!(r.at_end());
    }

    fn spec() -> CharacterSpec {
        let t = |secondary| Tracker {
            anim: 0,
            state0: true,
            state1: true,
            time_ms: 0,
            len_ms: 7200,
            secondary,
        };
        CharacterSpec {
            obj: 252,
            weapon_obj: Some(253),
            pid: 1,
            pos: [10.0, 20.0, 30.0],
            dims: [15.0, 50.0, 15.0],
            models: vec![
                ModelEntry {
                    file: 14,
                    subs: vec![15],
                    textures: vec![8, 9, 10, 10, 10],
                    styles: vec![11, 12, 13],
                },
                ModelEntry {
                    file: 18,
                    subs: vec![],
                    textures: vec![19],
                    styles: vec![11, 11],
                },
            ],
            anim: vec![t(None), t(Some((1, 0))), t(Some((2, 1)))],
            spawned: false,
            record_complete: true,
            rot: None,
            socket: 2,
            weapon_model: Some(ModelEntry {
                file: 21,
                subs: vec![],
                textures: vec![20],
                styles: vec![11],
            }),
            weapon_anim_len_ms: 0,
        }
    }

    /// The mask as it reads on the wire (the low byte carries the "high byte follows" marker 0x80).
    fn wire(cf: u16) -> u16 {
        if cf & crate::objects::CF_HIGH_MASK != 0 {
            cf | 0x80
        } else {
            cf
        }
    }

    /// DSV-44: the corpse blob is the reference stack's 219-bit form (dims (15, 50, 15) encode as the reference stack's `vec72`), the
    /// death record is `cf` 0x0188 with the dead flag words and no attachment, the corpse create is `cf` 0x25bb with
    /// one model entry and a non-looping main tracker.
    #[test]
    fn corpse_and_death_records_have_the_observed_forms() {
        let b = CorpseBlob::reference([15.0, 50.0, 15.0]).write();
        assert_eq!(b.bit_len(), 219);
        // OBSERVED: capture 20261025_065016_tdm6 (the corpse blob of a body kill)
        let want: [u8; 28] = [
            0x26, 0x00, 0xff, 0xff, 0xff, 0x00, 0x01, 0x02, 0x00, 0x00, 0x00, 0x40, 0x12, 0x92,
            0x99, 0x91, 0x99, 0x49, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(b.as_bytes(), &want[..]);
        let d = character_died(252);
        assert_eq!(wire(d.cf()), 0x0188);
        let dw = d.write(&pbox());
        let mut r = dw.reader();
        assert_eq!((r.read_u8(), r.read_u8(), r.read_u16()), (0x88, 0x01, 252));
        assert_eq!(r.read_u16(), 0x0802);
        r.skip_bits(16 + 32 + 32);
        assert_eq!(r.read_u16(), 0xFFFF, "empty attachment list");
        let body = ModelEntry {
            file: 14,
            subs: vec![15],
            textures: vec![8, 9, 10, 10, 10],
            styles: vec![11, 12, 13],
        };
        let c = corpse_create(
            265,
            vec![body],
            (86, 1766),
            [15.0, 50.0, 15.0],
            [1.0, 2.0, 3.0],
            IDENTITY_ROT,
        );
        assert_eq!(wire(c.cf()), 0x25bb);
        let w = c.write(&pbox());
        let mut g = BitWriter::new();
        g.write_u8(8).write_u32(w.bit_len() as u32).append(&w);
        let mut sc =
            crate::synth_client::SynthClient::new(crate::synth_client::SynthConfig::named("x"));
        sc.read_group_for_test(&g).unwrap();
        let o = &sc.seen.objects[&265];
        assert_eq!(
            (o.kind, o.blob_type, o.flags, o.anim.clone()),
            (1, Some(0x26), Some(0x1803), vec![86])
        );
        // DSV-44 amendment: the reference stack's respawn form `cf` 0x2ba8 (tdm6 seq 10964: body and face entries, three
        // trackers, flags 0x3803, position by CF_POS_SNAP alone, the weapon attached; no rotation)
        let face = ModelEntry {
            file: 18,
            subs: Vec::new(),
            textures: vec![19],
            styles: vec![11, 11],
        };
        let trackers = vec![
            Tracker {
                anim: 0,
                state0: true,
                state1: true,
                time_ms: 0,
                len_ms: 7200,
                secondary: None,
            },
            Tracker {
                anim: 96,
                state0: true,
                state1: true,
                time_ms: 0,
                len_ms: 4000,
                secondary: Some((1, 0)),
            },
            Tracker {
                anim: 6,
                state0: true,
                state1: true,
                time_ms: 0,
                len_ms: 4000,
                secondary: Some((2, 1)),
            },
        ];
        let rs = character_respawned(
            252,
            [410.0, 50.0, -1460.0],
            vec![spec().models[0].clone(), face],
            trackers,
            Some((253, 2)),
        );
        assert_eq!(wire(rs.cf()), 0x2ba8, "the reference stack's respawn mask");
        let mut sc2 =
            crate::synth_client::SynthClient::new(crate::synth_client::SynthConfig::named("y"));
        let mut g = BitWriter::new();
        for rec in [
            character_create(&spec()).write(&pbox()),
            d.write(&pbox()),
            rs.write(&pbox()),
        ] {
            g.write_u8(8).write_u32(rec.bit_len() as u32).append(&rec);
            sc2.read_group_for_test(&g).unwrap();
            g = BitWriter::new();
        }
        assert_eq!(sc2.seen.objects[&252].attached, vec![(253, 2)]);
        assert_eq!(sc2.seen.objects[&252].flags, Some(0x3803));
    }

    #[test]
    fn create_masks_and_lengths_match_the_reference_form() {
        let s = spec();
        let c = character_create(&s);
        assert_eq!(wire(c.cf()), 0xa1ab);
        let w = weapon_create(&s).unwrap();
        assert_eq!(wire(w.cf()), 0x01af);
        // record lengths of the reference run's character and weapon creates (same file ids, counts and blob)
        assert_eq!(c.write(&pbox()).bit_len(), 3730);
        assert_eq!(w.write(&pbox()).bit_len(), 2629);
        assert_eq!(
            wire(character_spawned(252, s.pos, Some(IDENTITY_ROT)).cf()),
            0x468a
        );
        assert_eq!(wire(character_spawned(252, s.pos, None).cf()), 0x428a);
        assert_eq!(weapon_spawned(253, s.pos).cf(), 0x000a);
        assert_eq!(weapon_spawned(253, s.pos).write(&pbox()).bit_len(), 240);
        assert_eq!(
            character_spawned(252, s.pos, Some(IDENTITY_ROT))
                .write(&pbox())
                .bit_len(),
            376
        );
        assert_eq!(
            weapon_model_change(253, s.weapon_model.clone().unwrap()).cf(),
            CF_FILES
        );
    }
}
