//! The world-file objects the server creates on every client (slice 4). In every mode: the objective zones (client
//! object type 0x37 on a kind-0 object) and the capture flags (type 0x17 pickup kind 2 on a kind-1 model object), in
//! the forms the reference stack sent and the real 2009 client accepted in the two-player capture. Behind
//! `-worldobjects all` also the world's model objects (kind 2 named brushes) with flag words derived from their
//! properties (UNPROVEN form). Kill volumes and ladders (kind 9 with a type-0x0b volume record), light sprites
//! (kind 3) and effect objects are not created yet. The world sounds are started as tracked sounds (special record
//! kind 1) after the creates (DSV-56).
// SPEC: UDP_SESSION_2009_B.md §3.3 (create record by kind), §4.2 (change fields); GAME_MODES_2009.md §4.3 (type 0x37
// layout), §4.4 (type 0x17 kinds); GAME_MESSAGES_2009_S2C.md §8.2; UDP_SESSION_2009_B.md §3.5.1 (the reference stack's
// initial group: zones, flags, brushes, volumes, sprites), §3.5.2 (the world-file objects as DeCombat creates them); CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §2.2 (the
// object group with the world objects, REFERENCE, ACCEPTED)

use crate::modelobj::{self, ModelEntry, ModelRecord, Tracker};
use crate::objects::{write_record_head, PosBox, CF_FLAGS, CF_NEW, CF_POS};
use crate::world::{
    BrushClass, BrushInfo, EffectInfo, FlagInfo, SoundInfo, SpriteInfo, VolumeInfo, VolumeKind,
    WorldScripts, ZoneInfo, ZoneKind, BLEND_NONE,
};
use dc_wire::udp::{BitWriter, Vec72};

/// Which world objects the server creates (`-worldobjects off|mode|world|all`).
// DC-DEFAULT: DSV-37 (extended, staged) `world`: the reference stack-equivalent set
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldObjects {
    Off,
    /// Objective zones and capture flags.
    Mode,
    /// The set the reference stack created at world load (OBSERVED tdm4 / tdm6): zones, flags, the model-linked objects
    /// (world models, breakables, sky models, spinning and rotating models) in the state the world's start scripts
    /// leave them, kill volumes and ladders, scaled sprites and placed effects that are switched on. Not doors,
    /// switches or water.
    World,
    /// Every model-linked object, volume, sprite and effect of the file, ignoring the start scripts and the
    /// switched-off state.
    All,
}

/// Which mode class a zone names (`z_class`): `Own` = the class the zone serves (bomb site 2, flag zone 7), so a
/// client of another mode ignores it; `Room` = the room's class (the reference stack's form: it sent 1 in a TDM room).
// SPEC: GAME_MODES_2009.md §4.3 (the zone tests compare `z_class` with the current class, 3 -> 2 and 8 -> 7);
// CAPTURE_tdm2_TWOPLAYER_VALIDATION.md (reference zones with `z_class` 1 in a TDM room, decoded by the ROM-gated test)
// DC-DEFAULT: GMD-4 (proposed) the zone's own class; DSV-37 (proposed)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneClass {
    Own,
    Room,
}

/// Client object type of a mode zone.
// SPEC: GAME_MODES_2009.md §4.3 (factory case 0x37, cshell+0x222480)
pub const OBJ_ZONE_TYPE: u8 = 0x37;
/// Client object type of a pickup.
// SPEC: GAME_MODES_2009.md §4.4 (OBJ_PICKUP 0x17)
pub const OBJ_PICKUP_TYPE: u8 = 0x17;
/// `pickup_kind` of a capture flag.
// SPEC: GAME_MODES_2009.md §4.4 (kind 2 `[state][team][at_base]`)
pub const PICKUP_FLAG: u32 = 2;
/// `obj_kind` of a named brush.
// SPEC: UDP_SESSION_2009_B.md §3.3 (OBJ_BRUSH `[name:zstr8]`, the reader keeps 127 characters)
pub const OBJ_BRUSH: u8 = 2;
pub const BRUSH_NAME_MAX: usize = 127;

/// `z_kind` values and the classes a zone serves.
// SPEC: GAME_MODES_2009.md §4.3 zone kinds (4 enemy objective area, 6 flag capture), §2 class table (bomb 2, flag 7)
pub const ZONE_KIND_BOMB: u32 = 4;
pub const ZONE_KIND_FLAG: u32 = 6;
pub const CLASS_BOMB: u8 = 2;
pub const CLASS_FLAG: u8 = 7;
/// `z_icon` none, `z_team` any team.
// SPEC: GAME_MODES_2009.md §4.3 (`z_icon` 0xff = none; `z_team` 0xfe = any team)
pub const ZONE_ICON_NONE: u8 = 0xff;
pub const ZONE_TEAM_ANY: u8 = 0xfe;
/// The first two zone words as the reference stack sent them (the float -1.0; meaning not traced).
// SPEC: GAME_MODES_2009.md §4.3 rows 1-2 (`z_a`, `z_b` not traced); reference zones (REFERENCE)
pub const TODO_ZONE_AB: u32 = 0xbf80_0000;

/// The type-0x37 record of a zone, after the type byte (GAME_MODES §4.3 order): `z_a`, `z_b`, `z_flag`, the extent
/// as `vec72` (the reference stack's field 4 carried the zone's box extent), `z_icon`, `z_team`, `z_class`, `z_c`, `z_value`
/// (the zone number), `z_kind`, `z_d`, `z_sub`, `z_e`, `z_text` (the object name), `z_text2`.
// SPEC: GAME_MODES_2009.md §4.3 (DISASM cshell+0x2b67b5..0x2b6896 widths); REFERENCE form (vec72 = the box extent,
// `z_value` = the zone number, `z_text` = the object name)
pub fn zone_blob(z: &ZoneInfo, team: u8, class: u8) -> BitWriter {
    zone_blob_as(z, team, class, 0)
}

/// `z_kind` of a use point (plant or defuse) and its two `z_sub` values.
// SPEC: GAME_MODES_2009.md §4.3 (kind 5: the use target; `z_sub` 1 plant point, 2 defuse point; cshell+0x5d610)
pub const ZONE_KIND_USE: u32 = 5;
pub const ZONE_SUB_PLANT: u8 = 1;
pub const ZONE_SUB_DEFUSE: u8 = 2;

/// [`zone_blob`], or with `point` 1 / 2 the plant / defuse point of a bomb site (`z_kind` 5, `z_sub` = `point`,
/// `z_team` any team): the 2009 client's use test makes it the use target (+0x1c9 plant, +0x1c8 defuse).
// SPEC: GAME_MODES_2009.md §4.3 (kind-5 test cshell+0x5d610: active, marked (`z_sub` != 0), class matches; no
// team test), §5.2 (plant point; the defuse point PLAUSIBLY made at the bomb by the 2009 server)
// DC-DEFAULT: DSV-200 (proposed) both points of every site are created with the world (the server checks the
// planted bomb's place)
pub fn zone_blob_as(z: &ZoneInfo, team: u8, class: u8, point: u8) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(OBJ_ZONE_TYPE)
        .write_u32(TODO_ZONE_AB)
        .write_u32(TODO_ZONE_AB)
        .write_bit(false);
    Vec72::encode(z.dims).write(&mut w);
    let kind = match (point, z.kind) {
        (1 | 2, _) => ZONE_KIND_USE,
        (_, ZoneKind::BombSite) => ZONE_KIND_BOMB,
        (_, ZoneKind::FlagZone) => ZONE_KIND_FLAG,
    };
    let team = if point != 0 { ZONE_TEAM_ANY } else { team };
    w.write_u8(ZONE_ICON_NONE)
        .write_u8(team)
        .write_u8(class)
        .write_u8(0)
        .write_u32(z.number.max(0) as u32)
        .write_u32(kind)
        .write_u32(0)
        .write_u8(point)
        .write_u32(0);
    let name: Vec<u8> = z.name.iter().copied().take(259).collect();
    w.write_zstr8(&name).write_zstr8(b"");
    w
}

/// `z_kind` of the use target (`UP_USE` 0x59) and the class it serves in Spy Hunt.
// SPEC: GAME_MODES_2009.md §4.3 (kind 8: active, `z_team` own or any; cshell+0x5e050)
pub const ZONE_KIND_USE_TARGET: u32 = 8;
pub const CLASS_SPY: u8 = 9;

/// The type-0x37 record of a Spy Hunt uplink box: the zone layout with `z_kind` 8, `z_class` 9, the box extent,
/// `z_team` the box's team (the spies' side), `z_d` the transmission time as a float, `z_text` the object name.
// SPEC: GAME_MODES_2009.md §4.3 (the reference stack's fifth tdm1 zone: kind 8, class 9, team 2, `z_d` 15.0 read as a float;
// REFERENCE-CONFIRMED); OBSERVED: ROM world files (one activation box per Spy Hunt world with a team text, a box extent
// and a time amount of 15.0)
// DC-DEFAULT: DSV-233 (proposed) created in Spy Hunt rooms only
pub fn uplink_blob(u: &crate::world::UplinkInfo, team: u8) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(OBJ_ZONE_TYPE)
        .write_u32(TODO_ZONE_AB)
        .write_u32(TODO_ZONE_AB)
        .write_bit(false);
    Vec72::encode(u.dims).write(&mut w);
    w.write_u8(ZONE_ICON_NONE)
        .write_u8(team)
        .write_u8(CLASS_SPY)
        .write_u8(0)
        .write_u32(0)
        .write_u32(ZONE_KIND_USE_TARGET)
        .write_u32(u.secs.unwrap_or(0.0).to_bits())
        .write_u8(0)
        .write_u32(0);
    let name: Vec<u8> = u.name.iter().copied().take(259).collect();
    w.write_zstr8(&name).write_zstr8(b"");
    w
}

/// The class a zone serves.
pub fn zone_own_class(z: &ZoneInfo) -> u8 {
    match z.kind {
        ZoneKind::BombSite => CLASS_BOMB,
        ZoneKind::FlagZone => CLASS_FLAG,
    }
}

/// Create record of a kind-0 object with a shell blob: `cf` = NEW | POS (| ROT_SNAP), zero create position, the
/// blob, then `pos48` + `vec72` 0 (no flag block: compact position) and the rotation.
// SPEC: UDP_SESSION_2009_B.md §3.3, §4.2 items 6-7; reference zones (`cf` 0x0003, 0x0483 with a rotation; REFERENCE)
pub fn blob_object_record(
    obj: u16,
    blob: &BitWriter,
    pos: [f32; 3],
    rot: Option<[f32; 4]>,
    pbox: &PosBox,
) -> BitWriter {
    let mut cf = CF_NEW | CF_POS;
    if rot.is_some() {
        cf |= modelobj::CF_ROT_SNAP;
    }
    let mut w = BitWriter::new();
    write_record_head(&mut w, cf, obj);
    let bits = blob.bit_len();
    let mut kb = modelobj::KIND_HAS_BLOB;
    if bits > 255 {
        kb |= modelobj::KIND_BLOB_LEN16;
    }
    w.write_u8(kb);
    for _ in 0..3 {
        w.write_f32(0.0);
    }
    if bits > 255 {
        w.write_u16(bits.min(u16::MAX as usize) as u16);
    } else {
        w.write_u8(bits as u8);
    }
    w.append(blob);
    crate::stream::pos48_encode(pos, pbox).write(&mut w);
    Vec72::encode([0.0; 3]).write(&mut w);
    if let Some(q) = rot {
        dc_wire::udp::write_quat128(&mut w, q);
    }
    w
}

/// The type-0x17 record of a capture flag: `[1][0][0]`, empty name, kind 2, `[state][team][at_base]` with the
/// reference stack's values (state = the flag's team, not at base).
// SPEC: GAME_MODES_2009.md §4.4 (kind 2 layout); reference flags (bits 1 0 0, name "", state = team, at_base 0;
// REFERENCE); SPEC: UNPROVEN meanings of the bits and of `state`
pub fn flag_blob(team: u32) -> BitWriter {
    flag_blob_at(team, false)
}

/// [`flag_blob`] with `at_base`: a flag-mode room creates its flags at home (the 2009 client shows the capture
/// prompt only while no flag of the carrier's team has `at_base` 0, cshell+0x5dad0).
// SPEC: GAME_MODES_2009.md §5.5 (capture prompt); DC-DEFAULT: DSV-192 (proposed) `at_base` 1 in classes 7 / 8
pub fn flag_blob_at(team: u32, at_base: bool) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(OBJ_PICKUP_TYPE)
        .write_bit(true)
        .write_bit(false)
        .write_bit(false)
        .write_zstr8(b"")
        .write_u32(PICKUP_FLAG)
        .write_u32(team)
        .write_u32(team)
        .write_bit(at_base);
    w
}

/// Flag words of a capture flag in a flag-mode room: the engine's visible bit (an object created without flag words
/// keeps flags 0 and is not drawn; the reference stack's TDM flags had none).
// EVIDENCE: engine+0x16b4b0, engine+0x16b580 (an object is drawn only with bit 0x01 of byte +0x84)
// DC-DEFAULT: CLI-584 (proposed) the words of a shown flag; the TDM form keeps the reference stack's (no flag words)
pub const FLAG_SHOWN_FLAGS: modelobj::FlagWords = modelobj::FlagWords {
    flags: 0x0001,
    flags2: 0,
    uflags: 0,
    flags3: 0,
};

/// True for the room classes whose capture flags are shown: the flag modes.
// SPEC: GAME_MODES_2009.md §5.5 (`FLAG` class 7, `FLAG_CLAN` class 8), §4.4 (kind 2: flag classes 7 / 8)
// DC-DEFAULT: CLI-584 (proposed)
pub fn flags_shown(room_class: u8) -> bool {
    matches!(room_class, 7 | 8)
}

/// The model file id of a capture flag with no model (a `dc.capture_flag` whose `model` is empty, MAP-23): not an
/// entry of the session file table (the id of "not in the table", `Session::file_id_of`), so the client's model
/// block names no file and the client draws its own flag marker; the ROM flag row's textures and styles are still
/// sent when a ROM is at hand.
// DC-DEFAULT: MAP-23
pub const NO_MODEL_FILE: u16 = 0xFFFF;

/// Animation length of the flag's single tracker (the reference stack sent the 10-bit time field, time 0).
// SPEC: reference flags (one tracker: animation 0, state bits 1, time select 2; REFERENCE); UDP_SESSION_2009_B.md §4.4
pub const TODO_FLAG_ANIM_LEN_MS: u32 = 1_000;

/// Create record of a capture flag: a kind-1 model object (`cf` NEW | POS | ANIM | ATTACH) with the pickup blob,
/// the model block, one tracker, the position and an empty attachment list.
// SPEC: UDP_SESSION_2009_B.md §3.3, §3.4, §4.2; reference flags (`cf` 0x01a3 on the wire; REFERENCE)
pub fn flag_record(
    obj: u16,
    team: u32,
    model: ModelEntry,
    pos: [f32; 3],
    shown: bool,
    pbox: &PosBox,
) -> BitWriter {
    ModelRecord {
        obj,
        create: true,
        blob: Some(flag_blob_at(team, shown)),
        models: Some(vec![model]),
        flags: shown.then_some(FLAG_SHOWN_FLAGS),
        anim: Some(vec![Tracker {
            anim: 0,
            state0: true,
            state1: true,
            time_ms: 0,
            len_ms: TODO_FLAG_ANIM_LEN_MS,
            secondary: None,
        }]),
        pos: Some((pos, [0.0; 3])),
        attach: Some(Vec::new()),
        ..ModelRecord::default()
    }
    .write(pbox)
}

/// Change record of a capture flag's model block (`CF_FILES`): the carried form on a carrier, the ground form again
/// when it lies or is home.
// SPEC: UDP_SESSION_2009_B.md §4.2 item 1 (kind 1: the model file block again)
// DC-DEFAULT: DSV-217 (proposed)
pub fn flag_model_record(obj: u16, model: ModelEntry, pbox: &PosBox) -> BitWriter {
    ModelRecord {
        obj,
        create: false,
        models: Some(vec![model]),
        ..ModelRecord::default()
    }
    .write(pbox)
}

/// Change record of a capture flag that moved: the shown flag words and the position as a cut (no smoothing).
// SPEC: UDP_SESSION_2009_B.md §4.1 row 0x0200, §4.2 items 3, 6
// DC-DEFAULT: DSV-193 (proposed)
pub fn flag_move_record(obj: u16, pos: [f32; 3], pbox: &PosBox) -> BitWriter {
    ModelRecord {
        obj,
        create: false,
        flags: Some(FLAG_SHOWN_FLAGS),
        pos: Some((pos, [0.0; 3])),
        pos_snap: true,
        ..ModelRecord::default()
    }
    .write(pbox)
}

/// `pickup_kind` of the planted bomb: a kind without extra fields (no 2009 bomb pickup kind is known).
// SPEC: GAME_MODES_2009.md §4.4 (kinds 2, 3, 5, 6 carry extra fields; the reference stack's parked kind-0 pickups none)
// DC-DEFAULT: DSV-220 (proposed)
pub const PICKUP_BOMB: u32 = 0;

/// The type-0x17 blob of the planted bomb: no spin, no fall, no client FX (the indicator is its own placed effect),
/// kind [`PICKUP_BOMB`].
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 (type 0x17 `[3 x bool1][name:zstr8(256)][kind:u32]`)
// DC-DEFAULT: DSV-220 (proposed)
pub fn bomb_blob() -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(OBJ_PICKUP_TYPE)
        .write_bit(false)
        .write_bit(false)
        .write_bit(false)
        .write_zstr8(b"")
        .write_u32(PICKUP_BOMB);
    w
}

/// Create record of the planted bomb: a kind-1 model object like a shown capture flag (the engine's visible bit, one
/// tracker, an empty attachment list) with the bomb blob, lying at `pos` turned by `yaw`.
// SPEC: UDP_SESSION_2009_B.md §3.3, §3.4, §4.2
// DC-DEFAULT: DSV-220 (proposed)
pub fn bomb_record(
    obj: u16,
    model: ModelEntry,
    pos: [f32; 3],
    yaw: f32,
    pbox: &PosBox,
) -> BitWriter {
    ModelRecord {
        obj,
        create: true,
        blob: Some(bomb_blob()),
        models: Some(vec![model]),
        flags: Some(FLAG_SHOWN_FLAGS),
        anim: Some(vec![Tracker {
            anim: 0,
            state0: true,
            state1: true,
            time_ms: 0,
            len_ms: TODO_FLAG_ANIM_LEN_MS,
            secondary: None,
        }]),
        pos: Some((pos, [0.0; 3])),
        rot: Some(crate::stream::yaw_quat(yaw)),
        rot_snap: true,
        attach: Some(Vec::new()),
        ..ModelRecord::default()
    }
    .write(pbox)
}

/// Flag words of a named brush from its properties: full precision (0x0100), visible 0x0001, ray-hit 0x1000, solid
/// 0x2000; second word 0x0040 translucent / 0x0042 additive; the sky model 0x0001 / 0x0020; user word 1 when visible.
// SPEC: UDP_SESSION_2009_B.md §4.2 item 3 (0x0100 full precision); reference brushes (tdm2: these words for the
// plain world models, compared in tests/rom_world.rs; REFERENCE); SPEC: UNPROVEN the meaning of each bit, the user
// words of breakables and doors
// DC-DEFAULT: DSV-37 (proposed)
pub fn brush_flags(b: &BrushInfo) -> (u16, u16, u32, u32) {
    if b.role == Some(dc_world::ObjectRole::SkyWorldModel) || b.class == BrushClass::Sky {
        return (0x0001, 0x0020, 0, 0);
    }
    let mut f = 0x0100u16;
    if b.visible {
        f |= 0x0001;
    }
    // OBSERVED: tdm4 (a world model with the fog-disable property had flags 0x0181; REFERENCE)
    if b.fog_disable {
        f |= 0x0080;
    }
    if b.rayhit {
        f |= 0x1000;
    }
    if b.solid {
        f |= 0x2000;
    }
    let mut f2 = match b.blend {
        1 => 0x0040,
        2 => 0x0042,
        _ => {
            let _ = BLEND_NONE;
            0
        }
    };
    // OBSERVED: tdm4 (the spinning model: flags2 0x0080, user word 0x00400121; breakables 0x0121, doors 0x00200121;
    // the high byte of the user word (a surface code, PLAUSIBLY) is not derived here; REFERENCE)
    let mut uf = u32::from(b.visible);
    match b.class {
        BrushClass::Breakable => uf |= 0x0120,
        BrushClass::Spinning => {
            f2 |= 0x0080;
            uf |= 0x0040_0120;
        }
        BrushClass::Door => uf |= 0x0020_0120,
        _ => {}
    }
    (f, f2, uf, 0)
}

/// Create record of a named brush (kind 2): name, flag words, the position (raw floats with the full-precision
/// flag). No rotation: the model's geometry is stored rotated in the world file.
// SPEC: UDP_SESSION_2009_B.md §3.3 (kind 2 `[name:zstr8]`), §4.2 items 3, 6, 7; OBSERVED: tdm4 / tdm6 (every brush
// create `cf` 0x000b without a rotation, also for objects with a yaw property; only a spinning model carried its
// current spin, `cf` 0x048f; REFERENCE)
pub fn brush_record(obj: u16, b: &BrushInfo, pbox: &PosBox) -> BitWriter {
    brush_record_with(obj, b, pbox, None)
}

/// [`brush_record`] with a door's own create blob (its activate entry now, DSV-340) in place of the reference stack's.
pub fn brush_record_with(
    obj: u16,
    b: &BrushInfo,
    pbox: &PosBox,
    door: Option<BitWriter>,
) -> BitWriter {
    let (f, f2, uf, f3) = brush_flags(b);
    let rot: Option<[f32; 4]> = None;
    let cf = CF_NEW | CF_POS | CF_FLAGS;
    let mut w = BitWriter::new();
    write_record_head(&mut w, cf, obj);
    // a door carries its client object (type 0x39) as the create blob
    // OBSERVED: tdm4 (the doors' kind-2 creates with the 25-bit blob; REFERENCE); DC-DEFAULT: DSV-46 (proposed)
    // DC-DEFAULT: DSV-561 (proposed) a switch players use carries its entry the same way
    let blob = if b.class == BrushClass::Door {
        Some(door.unwrap_or_else(crate::doors::door_blob))
    } else {
        door
    };
    match &blob {
        Some(_) => w.write_u8(OBJ_BRUSH | modelobj::KIND_HAS_BLOB),
        None => w.write_u8(OBJ_BRUSH),
    };
    for _ in 0..3 {
        w.write_f32(0.0);
    }
    if let Some(bl) = &blob {
        w.write_u8(bl.bit_len() as u8);
        w.append(bl);
    }
    let name: Vec<u8> = b.name.iter().copied().take(BRUSH_NAME_MAX).collect();
    w.write_zstr8(&name);
    w.write_u16(f & !0x0024)
        .write_u16(f2)
        .write_u32(uf)
        .write_u32(f3);
    if f & 0x0100 != 0 {
        for c in b.pos {
            w.write_f32(c);
        }
        for _ in 0..3 {
            w.write_f32(0.0);
        }
    } else {
        crate::stream::pos48_encode(b.pos, pbox).write(&mut w);
        Vec72::encode([0.0; 3]).write(&mut w);
    }
    if let Some(q) = rot {
        dc_wire::udp::write_quat128(&mut w, q);
    }
    w
}

/// The `obj_kind` of a named volume and the client object type of its blob.
// SPEC: UDP_SESSION_2009_B.md §3.3 (kind 9 `[name:zstr8][code:u16]`); GAME_MESSAGES_2009_S2C.md §8.2 row 0x0b
// (`OBJ_VOLUME`, the shared block of cshell+0x2163d0)
pub const OBJ_VOLUME_KIND: u8 = 9;
pub const OBJ_VOLUME_TYPE: u8 = 0x0b;
/// Container codes of the two volume kinds as the reference stack sent them.
// OBSERVED: capture 20261025_065016_tdm6 (36 kill volumes, code 7) and
// capture 20261025_002236_tdm4 (48 kill volumes code 7, 10 ladders code 11); REFERENCE, accepted
pub const VOLUME_CODE_KILL: u16 = 7;
pub const VOLUME_CODE_LADDER: u16 = 11;
/// The eleventh field of the shared volume block, constant in every reference volume (meaning not traced).
// OBSERVED: tdm4 / tdm6 (f32 -200.0 in all 94 volume blobs; REFERENCE); SPEC: UNPROVEN meaning
pub const TODO_VOLUME_FIELD_11: f32 = -200.0;
/// `cf` bit of the colour block.
// SPEC: UDP_SESSION_2009_B.md §4.1 (0x0040 `CF_COLOR`)
pub const CF_COLOR: u16 = 0x0040;
/// `obj_kind` of a billboard (sprite) record and the flag words the reference stack gave scaled sprites.
// SPEC: UDP_SESSION_2009_B.md §3.3 (kind 3 `[file:u16]`, scale of 2 components); OBSERVED: tdm4 / tdm6 (87 sprite
// creates `cf` 0x049b, flags 0x0081 / 0x0002 / 1 / 0; REFERENCE)
pub const OBJ_SPRITE_KIND: u8 = 3;
pub const SPRITE_FLAGS: modelobj::FlagWords = modelobj::FlagWords {
    flags: 0x0081,
    flags2: 0x0002,
    uflags: 1,
    flags3: 0,
};
/// Client object type of a placed effect (attaches the named effect to the object) and its flag words.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.4 (type 0x64 `[name:zstr8][value:u32][has_target:u8]`, cshell+0x26e070);
// OBSERVED: tdm4 / tdm6 (17 effect creates on kind-0 objects, flags 0 / 0 / 0x00800000 / 0, value 1, no target;
// REFERENCE)
pub const OBJ_EFFECT_TYPE: u8 = 0x64;
pub const EFFECT_FLAGS: modelobj::FlagWords = modelobj::FlagWords {
    flags: 0,
    flags2: 0,
    uflags: 0x0080_0000,
    flags3: 0,
};

/// The type-0x0b blob of a named volume (the shared block of cshell+0x2163d0 in its read order): fog on, fog far,
/// fog near, fog colour, tint, light add, a byte (0), movement sounds, a byte (0), current, the constant field,
/// viscosity, friction, a byte (0).
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 row 0x0b (widths); OBSERVED: tdm4 / tdm6 (every reference volume blob equals
// this form filled from the volume's properties: kill volumes viscosity 0, ladders 0.4; REFERENCE); SPEC: UNPROVEN the
// three byte fields (0 for the properties' default texts) and the eleventh field
pub fn volume_blob(v: &VolumeInfo) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(OBJ_VOLUME_TYPE)
        .write_bit(v.fog_enable)
        .write_f32(v.fog_far)
        .write_f32(v.fog_near);
    for c in v.fog_rgb.iter().chain(&v.tint).chain(&v.light_add) {
        w.write_f32(*c);
    }
    w.write_u8(0).write_bit(v.movement_sounds).write_u8(0);
    for c in v.current {
        w.write_f32(c);
    }
    w.write_f32(TODO_VOLUME_FIELD_11)
        .write_f32(v.viscosity)
        .write_f32(v.friction)
        .write_u8(0);
    w
}

/// The `uflags` word of a volume: 0x4 always, 0x1 while on, 0x400000 for a kill volume with a delay.
// OBSERVED: tdm4 / tdm6 (kill volumes 0x400005 with a delay property above 0, 0x5 without; ladders 0x5; a kill volume
// switched off by the start script went to 0x4 and was removed; REFERENCE); SPEC: UNPROVEN meanings
pub fn volume_uflags(v: &VolumeInfo) -> u32 {
    0x4 | 0x1 | if v.pending { 0x0040_0000 } else { 0 }
}

/// Create record of a named volume: kind 9 with the type-0x0b blob, the name and the container code, flag block,
/// compact position (`cf` 0x000b).
// SPEC: UDP_SESSION_2009_B.md §3.3, §4.2; OBSERVED: tdm4 / tdm6 (REFERENCE)
pub fn volume_record(obj: u16, v: &VolumeInfo, pbox: &PosBox) -> BitWriter {
    let blob = volume_blob(v);
    let mut w = BitWriter::new();
    write_record_head(&mut w, CF_NEW | CF_POS | CF_FLAGS, obj);
    let bits = blob.bit_len();
    let mut kb = OBJ_VOLUME_KIND | modelobj::KIND_HAS_BLOB;
    if bits > 255 {
        kb |= modelobj::KIND_BLOB_LEN16;
    }
    w.write_u8(kb);
    for _ in 0..3 {
        w.write_f32(0.0);
    }
    if bits > 255 {
        w.write_u16(bits as u16);
    } else {
        w.write_u8(bits as u8);
    }
    w.append(&blob);
    let name: Vec<u8> = v.name.iter().copied().take(BRUSH_NAME_MAX).collect();
    w.write_zstr8(&name);
    w.write_u16(match v.kind {
        VolumeKind::KillVolume => VOLUME_CODE_KILL,
        VolumeKind::Ladder => VOLUME_CODE_LADDER,
    });
    w.write_u16(0)
        .write_u16(0)
        .write_u32(volume_uflags(v))
        .write_u32(0);
    crate::stream::pos48_encode(v.pos, pbox).write(&mut w);
    Vec72::encode([0.0; 3]).write(&mut w);
    w
}

/// The rotation of a world object from its Euler property (pitch about X, yaw about Y, roll about Z): yaw, then
/// pitch, then roll.
// SPEC: FMT_DAT85.md §6.1 (property 2: three radians); UDP_TRANSPORT_2009.md §15.3 (the yaw of a quaternion);
// SPEC: UNPROVEN the order for objects with pitch and roll (every observed sprite and effect has yaw only)
pub fn euler_quat(r: [f32; 3]) -> [f32; 4] {
    let (sp, cp) = (r[0] * 0.5).sin_cos();
    let (sy, cy) = (r[1] * 0.5).sin_cos();
    let (sr, cr) = (r[2] * 0.5).sin_cos();
    // q = q_yaw * q_pitch * q_roll, components (x, y, z, w)
    let [x, y, z, w] = [cy * sp, sy * cp, -sy * sp, cy * cp];
    [
        x * cr + y * sr,
        y * cr - x * sr,
        w * sr + z * cr,
        w * cr - z * sr,
    ]
}

/// The colour block of a sprite that is not plain white and opaque: `[r][g][b][a][extra 0]`.
// SPEC: UDP_SESSION_2009_B.md §4.2 item 4 (`CF_COLOR` 0x0040); OBSERVED: tdm4 (81 sprites with a grey or red colour
// property sent `cf` 0x04db with the colour, alpha 255, extra 0), tdm6 (6 white sprites without the block); REFERENCE
pub fn sprite_color(s: &SpriteInfo) -> Option<[u8; 5]> {
    let ch = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    let c = [
        ch(s.color[0]),
        ch(s.color[1]),
        ch(s.color[2]),
        ch(s.alpha * 255.0),
        0,
    ];
    (c != [255, 255, 255, 255, 0]).then_some(c)
}

/// Create record of a scaled sprite: kind 3 with the sprite file, the sprite flag words, the colour when it is not
/// white, the scale (two components), compact position and the rotation (`cf` 0x049b / 0x04db).
// SPEC: UDP_SESSION_2009_B.md §3.3, §4.2 items 3, 4, 5, 6, 7; OBSERVED: tdm4 / tdm6 (REFERENCE)
pub fn sprite_record(obj: u16, s: &SpriteInfo, file: u16, pbox: &PosBox) -> BitWriter {
    let color = sprite_color(s);
    let mut cf = CF_NEW | CF_POS | CF_FLAGS | modelobj::CF_SCALE | modelobj::CF_ROT_SNAP;
    if color.is_some() {
        cf |= CF_COLOR;
    }
    let mut w = BitWriter::new();
    write_record_head(&mut w, cf, obj);
    w.write_u8(OBJ_SPRITE_KIND);
    for _ in 0..3 {
        w.write_f32(0.0);
    }
    w.write_u16(file);
    let f = SPRITE_FLAGS;
    w.write_u16(f.flags & !0x0024)
        .write_u16(f.flags2)
        .write_u32(f.uflags)
        .write_u32(f.flags3);
    if let Some(c) = color {
        for b in c {
            w.write_u8(b);
        }
    }
    w.write_f32(s.scale[0]).write_f32(s.scale[1]);
    crate::stream::pos48_encode(s.pos, pbox).write(&mut w);
    Vec72::encode([0.0; 3]).write(&mut w);
    dc_wire::udp::write_quat128(&mut w, euler_quat(s.rot));
    w
}

/// The type-0x64 blob of a placed effect: the effect name, the loop value, no target.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.4; OBSERVED: tdm4 / tdm6 (value 1 on every effect, all of them looping; REFERENCE);
// SPEC: UNPROVEN that the value is the loop switch
pub fn effect_blob(e: &EffectInfo) -> BitWriter {
    let mut w = BitWriter::new();
    let name: Vec<u8> = e.fx.iter().copied().take(255).collect();
    w.write_u8(OBJ_EFFECT_TYPE)
        .write_zstr8(&name)
        .write_u32(u32::from(e.looped))
        .write_u8(0);
    w
}

/// Create record of a placed effect: kind 0 with the type-0x64 blob, the effect flag words, compact position and,
/// when it has one, the rotation (`cf` 0x000b / 0x048b).
// SPEC: UDP_SESSION_2009_B.md §3.3, §4.2; OBSERVED: tdm4 / tdm6 (REFERENCE)
pub fn effect_record(obj: u16, e: &EffectInfo, pbox: &PosBox) -> BitWriter {
    let blob = effect_blob(e);
    let rot = (e.rot != [0.0; 3]).then(|| euler_quat(e.rot));
    let mut cf = CF_NEW | CF_POS | CF_FLAGS;
    if rot.is_some() {
        cf |= modelobj::CF_ROT_SNAP;
    }
    let mut w = BitWriter::new();
    write_record_head(&mut w, cf, obj);
    let bits = blob.bit_len();
    let mut kb = modelobj::KIND_HAS_BLOB;
    if bits > 255 {
        kb |= modelobj::KIND_BLOB_LEN16;
    }
    w.write_u8(kb);
    for _ in 0..3 {
        w.write_f32(0.0);
    }
    if bits > 255 {
        w.write_u16(bits as u16);
    } else {
        w.write_u8(bits as u8);
    }
    w.append(&blob);
    let f = EFFECT_FLAGS;
    w.write_u16(f.flags)
        .write_u16(f.flags2)
        .write_u32(f.uflags)
        .write_u32(f.flags3);
    crate::stream::pos48_encode(e.pos, pbox).write(&mut w);
    Vec72::encode([0.0; 3]).write(&mut w);
    if let Some(q) = rot {
        dc_wire::udp::write_quat128(&mut w, q);
    }
    w
}

/// Special record kind of a tracked sound inside `S2C_OBJ_GROUP`.
// SPEC: UDP_SESSION_2009_B.md §3.2 (special kinds: 0 one-shot sound, 1 tracked sound, 3 removed objects)
pub const RECORD_TRACKED_SOUND: u8 = 1;
/// `tmask` of a tracked sound's first record: start (bit 0) and position (bit 1).
// SPEC: UDP_SESSION_2009_B.md §8.3 (tmask bits 0 / 1); OBSERVED: capture 20261025_002236_tdm4
// (every world-sound record with tmask 3; REFERENCE)
pub const TMASK_START_AND_POS: u8 = 3;
/// `dmask` of a tracked sound: every cached field is sent (flags, priority, radii), so the client's per-file cache
/// never supplies a value. The 2009 server sent 7 on the first use of a file and 0 / 4 / 5 on repeats (its cache).
// SPEC: UDP_SESSION_2009_B.md §8.1 (the client copies what dmask carries; always 7 is always correct)
// DC-DEFAULT: DSV-57 (proposed) always 7
pub const DMASK_ALL: u8 = 7;
/// The `sflags` bits of a world sound: positional with direction 0x0002, ambient (no direction) 0x0001, looping
/// 0x0004, volume byte present 0x0040, pitch present 0x0080; bits 0x0010 / 0x0020 / 0x0100 are set on every world
/// sound the reference stack sent (0x0010 keep the handle and 0x0100 the reverb key per AUDIO_2009.md §3.2, 0x0020 UNPROVEN).
// SPEC: UDP_SESSION_2009_B.md §8.2 (sflags bits 0-1, 0x40, 0x80); AUDIO_2009.md §3.2
// OBSERVED: capture 20261025_002236_tdm4 c6 seq 807 (11 world sounds: 0x135 ambient loop,
// 0x136 positional loop, 0x175 ambient loop at volume 50; the same on every link of tdm1 / tdm2 / tdm4 /
// menu-frida6; REFERENCE)
pub const SFLAGS_WORLD_BASE: u16 = 0x0130;
pub const SFLAG_AMBIENT: u16 = 0x0001;
pub const SFLAG_POSITIONAL: u16 = 0x0002;
pub const SFLAG_LOOP: u16 = 0x0004;
pub const SFLAG_VOLUME: u16 = 0x0040;
pub const SFLAG_PITCH: u16 = 0x0080;

/// The `sflags` word of a world sound.
// OBSERVED: tdm4 c6 seq 807 (11 of 11 records equal this rule from the ROM objects' properties; REFERENCE)
// DC-DEFAULT: DSV-56 (proposed)
pub fn world_sound_bits(s: &SoundInfo) -> u16 {
    let mut f = SFLAGS_WORLD_BASE;
    f |= if s.ambient {
        SFLAG_AMBIENT
    } else {
        SFLAG_POSITIONAL
    };
    if s.looped {
        f |= SFLAG_LOOP;
    }
    if s.volume != 100 {
        f |= SFLAG_VOLUME;
    }
    if (s.pitch - 1.0).abs() > 1e-6 && s.pitch.is_finite() {
        f |= SFLAG_PITCH;
    }
    f
}

/// The inner radius as a fraction of the outer radius in 1/255, truncated (the reference stack's 159 for 800 / 1280, not
/// 160).
// SPEC: UDP_SESSION_2009_B.md §8.2 (inner = ratio * outer / 255); OBSERVED: tdm4 c6 seq 807 (ratios 159, 63, 81,
// 127, 255 for 800/1280, 64/256, 256/800, 512/1024, 10000/10000; REFERENCE)
pub fn inner_ratio(s: &SoundInfo) -> u8 {
    if s.outer <= 0.0 {
        return 255;
    }
    ((255.0 * s.inner / s.outer).clamp(0.0, 255.0)) as u8
}

/// The tracked-sound special record of a world sound that starts at once:
/// `[0][1][tmask 3][track:u16][sfile:u16][dmask 7][sflags:u16][prio:u8][outer:u16][ratio:u8]([volume:u8])
/// ([pitch:f32])[tag:u32 = 0][pos:pos48]`.
// SPEC: UDP_SESSION_2009_B.md §8.3 (record layout; reader engine+0xa3070 / engine+0xa2e90)
// OBSERVED: capture 20261025_002236_tdm4 c6 seq 807 (the wind record
// `000103d6000800073501001027ff00000000` + pos48; tag 0 on all; REFERENCE)
pub fn tracked_sound_record(track: u16, sfile: u16, s: &SoundInfo, pbox: &PosBox) -> BitWriter {
    let flags = world_sound_bits(s);
    let mut w = BitWriter::new();
    w.write_u8(0)
        .write_u8(RECORD_TRACKED_SOUND)
        .write_u8(TMASK_START_AND_POS)
        .write_u16(track)
        .write_u16(sfile)
        .write_u8(DMASK_ALL)
        .write_u16(flags)
        .write_u8(s.priority);
    if flags & (SFLAG_AMBIENT | SFLAG_POSITIONAL) != 0 {
        w.write_u16(s.outer.clamp(0.0, u16::MAX as f32) as u16)
            .write_u8(inner_ratio(s));
    }
    if flags & SFLAG_VOLUME != 0 {
        w.write_u8(s.volume.min(u8::MAX as u32) as u8);
    }
    if flags & SFLAG_PITCH != 0 {
        w.write_f32(s.pitch);
    }
    w.write_u32(0);
    crate::stream::pos48_encode(s.pos, pbox).write(&mut w);
    w
}

/// The tracked-sound record of an event sound (pain, head shot, grenade impact) that starts at once:
/// `[0][1][tmask 3][track:u16][sfile:u16][dmask 7][sflags 0x0132][prio:u8][outer:u16][ratio:u8][tag:u32 = 0]
/// [pos:pos48]`.
// SPEC: UDP_SESSION_2009_B.md §8.3
// OBSERVED: capture 20261025_002236_tdm4 c6 seq 2968 (`0001039d01180007320102f4011100000000` +
// pos48), seq 7690 (`000103a601210007320101e8031100000000` + pos48); REFERENCE
// DC-DEFAULT: DSV-64 (proposed); DSV-57 (proposed) dmask always 7
pub fn event_sound_record(
    track: u16,
    sfile: u16,
    form: crate::sounds::SoundForm,
    pos: [f32; 3],
    pbox: &PosBox,
) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(0)
        .write_u8(RECORD_TRACKED_SOUND)
        .write_u8(TMASK_START_AND_POS)
        .write_u16(track)
        .write_u16(sfile)
        .write_u8(DMASK_ALL)
        .write_u16(crate::sounds::SFLAGS_EVENT)
        .write_u8(form.prio)
        .write_u16(form.outer)
        .write_u8(form.ratio)
        .write_u32(0);
    crate::stream::pos48_encode(pos, pbox).write(&mut w);
    w
}

/// One world object to create: its id and what it is.
#[derive(Debug, Clone, PartialEq)]
pub enum WorldObject {
    Zone {
        obj: u16,
        zone: ZoneInfo,
        team: u8,
        class: u8,
        /// 0 = the zone itself, 1 / 2 = its plant / defuse point ([`zone_blob_as`]).
        point: u8,
    },
    /// A Spy Hunt uplink box (DSV-233).
    Uplink {
        obj: u16,
        uplink: crate::world::UplinkInfo,
        team: u8,
    },
    Flag {
        obj: u16,
        flag: FlagInfo,
        team: u32,
        /// Created with the visible flag words (a flag-mode room), else without flag words (not drawn).
        shown: bool,
        /// Model file, textures, styles (resource paths).
        model: String,
        textures: Vec<String>,
        styles: Vec<String>,
    },
    Brush {
        obj: u16,
        brush: BrushInfo,
    },
    Volume {
        obj: u16,
        volume: VolumeInfo,
    },
    Sprite {
        obj: u16,
        sprite: SpriteInfo,
    },
    Effect {
        obj: u16,
        effect: EffectInfo,
    },
    /// A world sound: a tracked sound (its id from the object id space), not an object.
    Sound {
        obj: u16,
        sound: SoundInfo,
    },
}

impl WorldObject {
    pub fn obj(&self) -> u16 {
        match self {
            WorldObject::Zone { obj, .. }
            | WorldObject::Uplink { obj, .. }
            | WorldObject::Flag { obj, .. }
            | WorldObject::Brush { obj, .. }
            | WorldObject::Volume { obj, .. }
            | WorldObject::Sprite { obj, .. }
            | WorldObject::Effect { obj, .. }
            | WorldObject::Sound { obj, .. } => *obj,
        }
    }

    /// Resource paths the record names (announced in the file table before it).
    pub fn files(&self) -> Vec<String> {
        match self {
            WorldObject::Flag {
                model,
                textures,
                styles,
                ..
            } => std::iter::once(model.clone())
                .filter(|m| !m.is_empty())
                .chain(textures.iter().cloned())
                .chain(styles.iter().cloned())
                .collect(),
            WorldObject::Sprite { sprite, .. } => vec![sprite.file.clone()],
            WorldObject::Sound { sound, .. } => vec![sound.file.clone()],
            _ => Vec::new(),
        }
    }

    /// The create record; `file_id` maps a resource path to its id in the session file table.
    pub fn record(&self, pbox: &PosBox, file_id: &dyn Fn(&str) -> u16) -> BitWriter {
        match self {
            WorldObject::Zone {
                obj,
                zone,
                team,
                class,
                point,
            } => {
                let rot = (zone.rot[1] != 0.0).then(|| crate::stream::yaw_quat(zone.rot[1]));
                let blob = zone_blob_as(zone, *team, *class, *point);
                blob_object_record(*obj, &blob, zone.pos, rot, pbox)
            }
            WorldObject::Uplink { obj, uplink, team } => {
                blob_object_record(*obj, &uplink_blob(uplink, *team), uplink.pos, None, pbox)
            }
            WorldObject::Flag {
                obj,
                flag,
                team,
                shown,
                model,
                textures,
                styles,
            } => {
                // DC-DEFAULT: MAP-23 a flag without a model is created with the no-model file id (the client draws
                // its own marker for it)
                let m = ModelEntry {
                    file: if model.is_empty() {
                        NO_MODEL_FILE
                    } else {
                        file_id(model)
                    },
                    subs: Vec::new(),
                    textures: textures.iter().map(|t| file_id(t)).collect(),
                    styles: styles.iter().map(|s| file_id(s)).collect(),
                };
                flag_record(*obj, *team, m, flag.pos, *shown, pbox)
            }
            WorldObject::Brush { obj, brush } => brush_record(*obj, brush, pbox),
            WorldObject::Volume { obj, volume } => volume_record(*obj, volume, pbox),
            WorldObject::Sprite { obj, sprite } => {
                sprite_record(*obj, sprite, file_id(&sprite.file), pbox)
            }
            WorldObject::Effect { obj, effect } => effect_record(*obj, effect, pbox),
            WorldObject::Sound { obj, sound } => {
                tracked_sound_record(*obj, file_id(&sound.file), sound, pbox)
            }
        }
    }

    /// A tracked sound, not an object create (its record follows the creates of the group).
    pub fn is_sound(&self) -> bool {
        matches!(self, WorldObject::Sound { .. })
    }
}

/// The world sounds to start at world load: with `World` those switched on (the start switch, or the start scripts
/// of the room's `trig`), with `All` every one, none with `Off` / `Mode`; in world-file order, ids from `next_id`.
// OBSERVED: capture 20261025_002236_tdm4, 20261025_001409_tdm2, 20261025_000014_tdm1,
// 20261025_095156_menu-frida6 (the reference stack started the 11 world sounds of that world, all switched on, on every link
// in the id 8 message with the world creates; never removed; REFERENCE)
// DC-DEFAULT: DSV-56 (proposed)
pub fn plan_sounds(
    world: &crate::world::WorldInfo,
    which: WorldObjects,
    trig: Option<&str>,
    mut next_id: impl FnMut() -> u16,
) -> Vec<WorldObject> {
    let ov = match which {
        WorldObjects::Off | WorldObjects::Mode => return Vec::new(),
        WorldObjects::World => Some(run_start_scripts(&world.scripts, trig)),
        WorldObjects::All => None,
    };
    world
        .sounds
        .iter()
        .filter(|s| {
            ov.as_ref().is_none_or(|ov| {
                ov.get(&s.name.to_ascii_lowercase())
                    .and_then(|o| o.on)
                    .unwrap_or(s.start_on)
            })
        })
        .map(|s| WorldObject::Sound {
            obj: next_id(),
            sound: s.clone(),
        })
        .collect()
}

/// Command words of the world's command texts (hash and length of the lower-case word).
// OBSERVED: command texts of the ROM world files (hashes only): the message statement, its trigger / on / off /
// visible / solid arguments and the delayed statement
const WORD_MSG: crate::world::PropHash = ([0xe4, 0x6b, 0x32, 0x01, 0x65, 0xee, 0xc9, 0x1e], 3);
const WORD_TRIGGER: crate::world::PropHash = ([0x68, 0x32, 0x59, 0xfe, 0xab, 0xbf, 0x5e, 0xb3], 7);
const WORD_ON: crate::world::PropHash = ([0xb8, 0xd3, 0x1e, 0x85, 0x27, 0x25, 0xaf, 0xb1], 2);
const WORD_OFF: crate::world::PropHash = ([0xb4, 0xdc, 0x66, 0xdd, 0xe8, 0x06, 0x26, 0x1b], 3);
const WORD_VISIBLE: crate::world::PropHash = ([0xd4, 0x2e, 0xf1, 0x49, 0x79, 0x00, 0xbc, 0x6e], 7);
const WORD_SOLID: crate::world::PropHash = ([0x4b, 0x3d, 0x4a, 0x0d, 0xd7, 0x32, 0xf7, 0x91], 5);

fn word_key(w: &[u8]) -> crate::world::PropHash {
    let l = w.to_ascii_lowercase();
    (dc_world::hash16(&l), l.len())
}

/// What the start scripts did to one named object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScriptOverride {
    pub visible: Option<bool>,
    pub solid: Option<bool>,
    pub on: Option<bool>,
}

/// Limits of the script evaluation (nesting of trigger messages, statements in all).
const SCRIPT_DEPTH: u32 = 8;
const SCRIPT_STATEMENTS: u32 = 4096;

struct ScriptRun<'a> {
    scripts: &'a WorldScripts,
    out: std::collections::BTreeMap<Vec<u8>, ScriptOverride>,
    statements: u32,
    /// The triggers fired, one entry per fire (DSV-562 seeds the run-time counts with them).
    fired: Vec<Vec<u8>>,
}

impl ScriptRun<'_> {
    fn fire(&mut self, name: &[u8], depth: u32) {
        if depth > SCRIPT_DEPTH {
            return;
        }
        let Some(cmds) = self
            .scripts
            .triggers
            .iter()
            .find(|t| t.0.eq_ignore_ascii_case(name))
            .map(|t| t.1.clone())
        else {
            return;
        };
        self.fired.push(name.to_vec());
        for c in &cmds {
            self.run(c, depth + 1);
        }
    }

    fn run(&mut self, text: &[u8], depth: u32) {
        for st in text.split(|&b| b == b';' || b == b'\n' || b == b'\r') {
            self.statements += 1;
            if self.statements > SCRIPT_STATEMENTS {
                return;
            }
            let words: Vec<&[u8]> = st
                .split(|b| b.is_ascii_whitespace() || *b == b'(' || *b == b')')
                .filter(|w| !w.is_empty())
                .collect();
            // only immediate message statements; delayed ones (and every other statement) do not run at the start
            if words.len() < 3 || word_key(words[0]) != WORD_MSG {
                continue;
            }
            let target = words[1].to_ascii_lowercase();
            let arg = word_key(words[2]);
            let value = words.get(3).map(|v| crate::doors::script_flag(v));
            if arg == WORD_TRIGGER {
                self.fire(&target, depth);
                continue;
            }
            let o = self.out.entry(target).or_default();
            if arg == WORD_ON {
                o.on = Some(true);
            } else if arg == WORD_OFF {
                o.on = Some(false);
            } else if arg == WORD_VISIBLE {
                if let Some(v) = value {
                    o.visible = Some(v);
                }
            } else if arg == WORD_SOLID {
                if let Some(v) = value {
                    o.solid = Some(v);
                }
            }
        }
    }
}

/// Evaluates the world's start scripts: every start-up command object, then the room's start triggers (the first two
/// fields of the room's `trig` setting: the mission's and the map's start trigger) and the object-start command of
/// the mode controller it names (the fourth field). Only immediate `msg <object> <argument>` statements count:
/// `trigger` fires a trigger object's numbered commands, `on` / `off`, `visible 0|1` and `solid 0|1` change the
/// object; delayed statements and the end trigger (third field) do not run. Names compare without case.
// OBSERVED: capture 20261025_065016_tdm6 (the reference stack's objects of that world at load are those
// this evaluation leaves: the two objects the start-up command made non-solid or switched off and the model the start
// trigger made invisible and non-solid were removed before the real client joined, the effect switched off by it was
// never created, the breakables made invisible by the mission start trigger and visible again by the map's start
// trigger were created visible; REFERENCE); DSERVER_PIPE.md §3.2 (`trig` field order of the room line, our lobby's
// writer in dc-bute)
// DC-DEFAULT: DSV-37 (extended, staged) the start-script evaluation
pub fn run_start_scripts(
    scripts: &WorldScripts,
    trig: Option<&str>,
) -> std::collections::BTreeMap<Vec<u8>, ScriptOverride> {
    start_scripts_run(scripts, trig).0
}

/// [`run_start_scripts`] and the triggers it fired (one entry per fire, in order).
pub fn start_scripts_run(
    scripts: &WorldScripts,
    trig: Option<&str>,
) -> (
    std::collections::BTreeMap<Vec<u8>, ScriptOverride>,
    Vec<Vec<u8>>,
) {
    let mut r = ScriptRun {
        scripts,
        out: Default::default(),
        statements: 0,
        fired: Vec::new(),
    };
    for c in &scripts.startup {
        r.run(c, 1);
    }
    let fields: Vec<&str> = trig.map(|t| t.split('|').collect()).unwrap_or_default();
    for f in fields.iter().take(2) {
        if !f.trim().is_empty() {
            r.fire(f.trim().as_bytes(), 1);
        }
    }
    if let Some(ctrl) = fields.get(3).map(|f| f.trim()).filter(|f| !f.is_empty()) {
        if let Some(cmd) = scripts
            .controls
            .iter()
            .find(|c| c.0.eq_ignore_ascii_case(ctrl.as_bytes()))
            .map(|c| c.1.clone())
        {
            r.run(&cmd, 1);
        }
    }
    (r.out, r.fired)
}

/// A named brush in the state the start scripts leave it: their `visible` / `solid` values replace the file's (a
/// model made invisible also loses its ray-hit flag); `None` when it is left with neither visible, solid nor ray-hit
/// flag (a sky model always stays).
// OBSERVED: tdm6 (a model made invisible by the script lost its ray-hit flag as well; models with neither visible,
// solid nor ray-hit flag were removed at once; REFERENCE)
// DC-DEFAULT: DSV-37 (extended, staged); DSV-501 (proposed) the doors take the same values
pub fn scripted_brush(b: &BrushInfo, o: ScriptOverride) -> Option<BrushInfo> {
    let mut b = b.clone();
    if let Some(v) = o.visible {
        b.visible = v;
        b.rayhit = b.rayhit && v;
    }
    if let Some(s) = o.solid {
        b.solid = s;
    }
    (b.class == BrushClass::Sky || b.visible || b.solid || b.rayhit).then_some(b)
}

/// The sky table of the planned objects: per sky slot the object id of the last sky model of that slot (file
/// order), and the 48-byte block of the last sky model: its sky box (position -/+ the sky dims) and its inner box
/// (position -/+ the sky dims x the inner fraction) as 12 f32. `None` without a sky model.
// SPEC: UDP_SESSION_2009_A.md §4.8 (block, 30 slots), RENDER_2009.md §4.4 (floats 6..11 = the sky box of the sky
// camera); OBSERVED: tdm4 / tdm6 (the reference stack's tables equal this rule from the world files' sky models: the tdm6
// world's slot 0 = the second of its two slot-0 sky models, block from it; the tdm4 world's slot 0 = its one sky
// model; REFERENCE, accepted by the real client)
// DC-DEFAULT: DSV-45 (proposed)
pub fn sky_table(objs: &[WorldObject]) -> Option<([u8; 48], Vec<u16>)> {
    let mut ids = vec![0xFFFFu16; crate::msg::SKY_TABLE_MAX];
    let mut block = None;
    for o in objs {
        let WorldObject::Brush { obj, brush } = o else {
            continue;
        };
        let Some(sky) = brush.sky else {
            continue;
        };
        if let Some(slot) = usize::try_from(sky.index)
            .ok()
            .filter(|&s| s < crate::msg::SKY_TABLE_MAX)
        {
            ids[slot] = *obj;
        }
        let p = brush.pos;
        let mut f = [0f32; 12];
        for i in 0..3 {
            f[i] = p[i] - sky.dims[i];
            f[3 + i] = p[i] + sky.dims[i];
            f[6 + i] = p[i] - sky.dims[i] * sky.inner[i];
            f[9 + i] = p[i] + sky.dims[i] * sky.inner[i];
        }
        let mut b = [0u8; 48];
        for (i, v) in f.iter().enumerate() {
            b[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
        block = Some(b);
    }
    block.map(|b| (b, ids))
}

/// A named brush as the start scripts leave it (`None`: not created). A sky model the scripts switch off stays an
/// object but leaves the sky table (no sky slot), so a world with two sky models of one slot shows the one its start
/// trigger keeps on (Two Towers: the start triggers send `SkypointerBox on` and `SkypointerBox_Q off`; the `_Q` box
/// is the red sky of the quarantine variant and, last in the file, took slot 0).
// OBSERVED: ROM world TWOTOWERS (two sky models of slot 0; every LevelNNStartTrigger switches the second one off)
// DISASM: engine+0x8b7d0 (the host's remove-from-sky: clears the object's in-sky bit 0x100 and the slot that holds
// it), engine+0x96ef0 (add-to-sky: writes the object into its slot, the last one written wins)
// DC-DEFAULT: DSV-520 (proposed, project decision) `off` on a sky model is a removal from the sky table
fn scene_brush(b: &BrushInfo, o: ScriptOverride) -> Option<BrushInfo> {
    let mut b = b.clone();
    if let Some(v) = o.visible {
        b.visible = v;
        // OBSERVED: tdm6 (a model made invisible by the script lost its ray-hit flag as well; REFERENCE)
        b.rayhit = b.rayhit && v;
    }
    if let Some(s) = o.solid {
        b.solid = s;
    }
    if b.class == BrushClass::Sky {
        if o.on == Some(false) {
            b.sky = None;
        }
        return Some(b);
    }
    // OBSERVED: tdm6 (models with neither visible, solid nor ray-hit flag were removed at once; REFERENCE)
    if !(b.visible || b.solid || b.rayhit) || o.on == Some(false) {
        return None;
    }
    Some(b)
}

/// The world objects of a world, with ids from `next_id` (lowest free, CD-66); no start scripts (no room `trig`).
// DC-DEFAULT: DSV-37 (proposed); GMD-4 (proposed) zones from the world's objects
pub fn plan(
    world: &crate::world::WorldInfo,
    tables: &crate::rom_tables::RomTables,
    which: WorldObjects,
    zone_class: ZoneClass,
    room_class: u8,
    next_id: impl FnMut() -> u16,
) -> Vec<WorldObject> {
    plan_scene(world, tables, which, zone_class, room_class, None, next_id)
}

/// The world objects of a world in the state the start scripts of the room's `trig` setting leave them.
// DC-DEFAULT: DSV-37 (extended, staged)
pub fn plan_scene(
    world: &crate::world::WorldInfo,
    tables: &crate::rom_tables::RomTables,
    which: WorldObjects,
    zone_class: ZoneClass,
    room_class: u8,
    trig: Option<&str>,
    mut next_id: impl FnMut() -> u16,
) -> Vec<WorldObject> {
    let mut out = Vec::new();
    if which == WorldObjects::Off {
        return out;
    }
    let team_of = |t: &Option<Vec<u8>>| tables.teams.team_of(t.as_deref());
    for z in &world.zones {
        let class = match zone_class {
            ZoneClass::Own => zone_own_class(z),
            ZoneClass::Room => room_class,
        };
        out.push(WorldObject::Zone {
            obj: next_id(),
            zone: z.clone(),
            team: team_of(&z.team_text).unwrap_or(ZONE_TEAM_ANY),
            class,
            point: 0,
        });
        // DC-DEFAULT: DSV-200 (amended by DSV-222, proposed) a bomb room gets the plant point of every site; the
        // defuse point is made at the planted bomb only (session `on_bomb_shown`)
        if z.kind == ZoneKind::BombSite && matches!(room_class, 2 | 3) {
            {
                let point = ZONE_SUB_PLANT;
                out.push(WorldObject::Zone {
                    obj: next_id(),
                    zone: z.clone(),
                    team: ZONE_TEAM_ANY,
                    class: CLASS_BOMB,
                    point,
                });
            }
        }
    }
    // DC-DEFAULT: DSV-233 (proposed) a Spy Hunt room gets the world's uplink boxes (the spies' use target)
    if room_class == CLASS_SPY {
        for u in &world.spy.uplinks {
            out.push(WorldObject::Uplink {
                obj: next_id(),
                uplink: u.clone(),
                team: team_of(&u.team_text).unwrap_or(crate::game::spy::SPIES),
            });
        }
    }
    for f in &world.flags {
        let Some(team) = team_of(&f.team_text) else {
            continue;
        };
        // a ROM flag without a model is not created (as before); a `dc.capture_flag` without one takes the ROM's ground
        // flag, standing as high above the floor as the ROM worlds' flags (MAP-114), and without a ROM no model file
        // (MAP-23); the ROM flag row's textures and styles when a ROM is at hand
        let mut flag = f.clone();
        let model = match (f.model.clone(), f.dc, tables.flag_looks.ground.clone()) {
            (Some(m), _, _) => m,
            (None, true, Some(ground)) => {
                flag.pos[1] = crate::game::ctf::standing_height(world, f.pos);
                ground
            }
            (None, true, None) => String::new(),
            (None, false, _) => continue,
        };
        // SPEC: reference flags (capture tdm2: team 1's flag carried the files of [Flag1], team 2's those of [Flag0],
        // compared by sha256 in tests/rom_world.rs; REFERENCE); SPEC: UNPROVEN why the rows are in this order
        let (textures, styles) = tables
            .flag_looks
            .rows
            .get(&(u32::from(team) % 2))
            .cloned()
            .unwrap_or_default();
        out.push(WorldObject::Flag {
            obj: next_id(),
            flag,
            team: u32::from(team),
            shown: flags_shown(room_class),
            model,
            textures,
            styles,
        });
    }
    if which == WorldObjects::All {
        for b in &world.brushes {
            if b.class == BrushClass::Water {
                continue;
            }
            out.push(WorldObject::Brush {
                obj: next_id(),
                brush: b.clone(),
            });
        }
        for v in &world.volumes {
            out.push(WorldObject::Volume {
                obj: next_id(),
                volume: v.clone(),
            });
        }
        for s in &world.sprites {
            out.push(WorldObject::Sprite {
                obj: next_id(),
                sprite: s.clone(),
            });
        }
        for e in &world.effects {
            out.push(WorldObject::Effect {
                obj: next_id(),
                effect: e.clone(),
            });
        }
    }
    if which == WorldObjects::World {
        let ov = run_start_scripts(&world.scripts, trig);
        let get = |name: &[u8]| {
            ov.get(&name.to_ascii_lowercase())
                .copied()
                .unwrap_or_default()
        };
        for b in &world.brushes {
            if !matches!(
                b.class,
                BrushClass::Plain
                    | BrushClass::Breakable
                    | BrushClass::Sky
                    | BrushClass::Spinning
                    | BrushClass::Rotating
            ) {
                continue;
            }
            let Some(b) = scene_brush(b, get(&b.name)) else {
                continue;
            };
            out.push(WorldObject::Brush {
                obj: next_id(),
                brush: b,
            });
        }
        for v in &world.volumes {
            if get(&v.name).on == Some(false) {
                continue;
            }
            out.push(WorldObject::Volume {
                obj: next_id(),
                volume: v.clone(),
            });
        }
        for s in &world.sprites {
            if !get(&s.name).on.unwrap_or(s.start_on) {
                continue;
            }
            out.push(WorldObject::Sprite {
                obj: next_id(),
                sprite: s.clone(),
            });
        }
        for e in &world.effects {
            if !get(&e.name).on.unwrap_or(e.start_on) {
                continue;
            }
            out.push(WorldObject::Effect {
                obj: next_id(),
                effect: e.clone(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone() -> ZoneInfo {
        ZoneInfo {
            kind: ZoneKind::BombSite,
            pos: [10.0, 20.0, 30.0],
            rot: [0.0; 3],
            dims: [115.0, 60.0, 220.0],
            name: b"ZoneA".to_vec(),
            team_text: None,
            number: 1,
            planted_command: Vec::new(),
        }
    }

    #[test]
    fn zone_and_flag_blob_layouts() {
        let b = zone_blob(&zone(), 2, CLASS_BOMB);
        // type 8 + 2 x 32 + 1 + 72 + 4 x 8 + 3 x 32 + 8 + 32 + (5 + 1) x 8 + 8
        assert_eq!(b.bit_len(), 8 + 64 + 1 + 72 + 32 + 96 + 8 + 32 + 48 + 8);
        let mut r = b.reader();
        assert_eq!(r.read_u8(), OBJ_ZONE_TYPE);
        assert_eq!((r.read_u32(), r.read_u32()), (TODO_ZONE_AB, TODO_ZONE_AB));
        assert!(!r.read_bit());
        let d = Vec72::read(&mut r).decode_presumed();
        assert!(
            (d[0] - 115.0).abs() < 0.5 && (d[2] - 220.0).abs() < 0.5,
            "{d:?}"
        );
        assert_eq!(
            (r.read_u8(), r.read_u8(), r.read_u8(), r.read_u8()),
            (0xff, 2, 2, 0)
        );
        assert_eq!(
            (r.read_u32(), r.read_u32(), r.read_u32()),
            (1, ZONE_KIND_BOMB, 0)
        );
        assert_eq!((r.read_u8(), r.read_u32()), (0, 0));
        assert_eq!(r.read_zstr8(), b"ZoneA");
        assert_eq!(r.read_zstr8(), b"");
        assert!(r.at_end());
        // DSV-200: the plant point of the same site: kind 5, any team, `z_sub` 1
        let p = zone_blob_as(&zone(), 2, CLASS_BOMB, ZONE_SUB_PLANT);
        let mut r = p.reader();
        r.skip_bits(8 + 64 + 1 + 72);
        assert_eq!(
            (r.read_u8(), r.read_u8(), r.read_u8(), r.read_u8()),
            (0xff, ZONE_TEAM_ANY, 2, 0)
        );
        assert_eq!(
            (r.read_u32(), r.read_u32(), r.read_u32()),
            (1, ZONE_KIND_USE, 0)
        );
        assert_eq!((r.read_u8(), r.read_u32()), (ZONE_SUB_PLANT, 0));
        // the flag blob is the reference stack's 116 bits
        let f = flag_blob(2);
        assert_eq!(f.bit_len(), 116);
        let mut r = f.reader();
        assert_eq!(r.read_u8(), OBJ_PICKUP_TYPE);
        assert_eq!(
            (r.read_bit(), r.read_bit(), r.read_bit()),
            (true, false, false)
        );
        assert_eq!(r.read_zstr8(), b"");
        assert_eq!((r.read_u32(), r.read_u32(), r.read_u32()), (2, 2, 2));
        assert!(!r.read_bit() && r.at_end());
    }

    /// The start-script evaluation: start-up commands, the two start triggers of `trig` (nested trigger messages),
    /// the controller's start command; delayed statements and the end trigger do not run.
    #[test]
    fn start_scripts_follow_the_room_triggers() {
        let scripts = WorldScripts {
            startup: vec![b"msg Box1 (solid 0);
msg Vol1 off;"
                .to_vec()],
            triggers: vec![
                (
                    b"MissionStart".to_vec(),
                    vec![b"msg Hide trigger;".to_vec()],
                ),
                (
                    b"Hide".to_vec(),
                    vec![b"msg CrateA (visible 0);
msg CrateA (Solid 0)"
                        .to_vec()],
                ),
                (
                    b"LevelStart".to_vec(),
                    vec![
                        b"msg cratea (visible 1); msg CrateA (solid 1)".to_vec(),
                        b"msg Fx1 off;
delay 7 (msg Box1 (solid 1))"
                            .to_vec(),
                    ],
                ),
                (b"LevelEnd".to_vec(), vec![b"msg Fx2 off".to_vec()]),
            ],
            controls: vec![(b"Ctrl".to_vec(), b"msg Fx3 on;".to_vec())],
            ..WorldScripts::default()
        };
        let ov = run_start_scripts(&scripts, Some("MissionStart|LevelStart|LevelEnd|Ctrl"));
        let get = |n: &[u8]| ov.get(n).copied().unwrap_or_default();
        assert_eq!(
            get(b"box1").solid,
            Some(false),
            "the delayed statement did not run"
        );
        assert_eq!(get(b"vol1").on, Some(false));
        assert_eq!(
            (get(b"cratea").visible, get(b"cratea").solid),
            (Some(true), Some(true))
        );
        assert_eq!(get(b"fx1").on, Some(false));
        assert_eq!(get(b"fx2").on, None, "the end trigger does not run");
        assert_eq!(get(b"fx3").on, Some(true));
        // a trigger that fires itself stops at the depth limit
        let looped = WorldScripts {
            triggers: vec![(b"T".to_vec(), vec![b"msg T trigger; msg A off".to_vec()])],
            ..WorldScripts::default()
        };
        let ov = run_start_scripts(&looped, Some("T"));
        assert_eq!(ov[&b"a".to_vec()].on, Some(false));
    }

    /// DSV-520: a sky model switched off by the start scripts leaves the sky table; the slot goes to the one kept on
    /// (the user's "Two Towers: the sky is red instead of daytime").
    #[test]
    fn a_sky_model_switched_off_leaves_the_sky_table() {
        let sky = |name: &[u8], pos| BrushInfo {
            name: name.to_vec(),
            role: None,
            pos,
            rot: [0.0; 3],
            visible: true,
            solid: false,
            rayhit: false,
            blend: 0,
            fog_disable: false,
            class: BrushClass::Sky,
            sky: Some(crate::world::SkyInfo {
                index: 0,
                dims: [256.0; 3],
                inner: [0.1; 3],
            }),
        };
        let scripts = WorldScripts {
            triggers: vec![(
                b"Level01StartTrigger".to_vec(),
                vec![b"msg SkypointerBox on;
msg SkypointerBox_Q off;"
                    .to_vec()],
            )],
            ..WorldScripts::default()
        };
        let ov = run_start_scripts(&scripts, Some("|Level01StartTrigger||"));
        let get = |n: &[u8]| ov.get(&n.to_ascii_lowercase()).copied().unwrap_or_default();
        let day = sky(b"SkyPointerBox", [5376.0, 0.0, 1472.0]);
        let red = sky(b"SkypointerBox_Q", [6338.0, -9.0, 1561.0]);
        let objs: Vec<WorldObject> = [(&day, 300u16), (&red, 301)]
            .into_iter()
            .map(|(b, obj)| WorldObject::Brush {
                obj,
                brush: scene_brush(b, get(&b.name)).expect("a sky model stays an object"),
            })
            .collect();
        let (block, ids) = sky_table(&objs).unwrap();
        assert_eq!(ids[0], 300, "the day sky holds slot 0");
        let x0 = f32::from_le_bytes([block[0], block[1], block[2], block[3]]);
        assert_eq!(x0, 5376.0 - 256.0, "the block is the day sky's box");
        // without the trigger the later model keeps the slot, as before
        let objs: Vec<WorldObject> = [(&day, 300u16), (&red, 301)]
            .into_iter()
            .map(|(b, obj)| WorldObject::Brush {
                obj,
                brush: scene_brush(b, ScriptOverride::default()).unwrap(),
            })
            .collect();
        assert_eq!(sky_table(&objs).unwrap().1[0], 301);
    }

    /// The sky table from a sky model: its slot names it, the block is the sky box and the inner box.
    #[test]
    fn sky_table_from_the_sky_models() {
        let sky = |name: &[u8], index, pos| WorldObject::Brush {
            obj: 0,
            brush: BrushInfo {
                name: name.to_vec(),
                role: None,
                pos,
                rot: [0.0; 3],
                visible: true,
                solid: false,
                rayhit: false,
                blend: 0,
                fog_disable: false,
                class: BrushClass::Sky,
                sky: Some(crate::world::SkyInfo {
                    index,
                    dims: [256.0; 3],
                    inner: [0.001; 3],
                }),
            },
        };
        let mut objs = vec![
            sky(b"S1", 0, [0.0; 3]),
            sky(b"S2", 0, [5376.0, 256.0, -6400.0]),
        ];
        if let WorldObject::Brush { obj, .. } = &mut objs[1] {
            *obj = 251;
        }
        let (block, ids) = sky_table(&objs).unwrap();
        assert_eq!(ids.len(), 30);
        assert_eq!(ids[0], 251, "the last sky model of the slot");
        assert!(ids[1..].iter().all(|&i| i == 0xFFFF));
        let f: Vec<f32> = block
            .chunks(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        assert_eq!(&f[..6], &[5120.0, 0.0, -6656.0, 5632.0, 512.0, -6144.0]);
        assert!((f[6] - 5375.744).abs() < 1e-3 && (f[11] + 6399.744).abs() < 1e-3);
        assert!(sky_table(&[]).is_none());
        assert_eq!(
            brush_flags(&match &objs[0] {
                WorldObject::Brush { brush, .. } => brush.clone(),
                _ => unreachable!(),
            }),
            (0x0001, 0x0020, 0, 0)
        );
    }

    #[test]
    fn records_parse_with_the_synthetic_reader() {
        let pbox = PosBox {
            min: [-1000.0; 3],
            max: [1000.0; 3],
        };
        let z = blob_object_record(
            5,
            &zone_blob(&zone(), 2, 2),
            [10.0, 20.0, 30.0],
            None,
            &pbox,
        );
        // head: cf 0x03, obj; kind byte 0xc0 (blob, 16-bit length since the blob is above 255 bits)
        assert_eq!(&z.as_bytes()[..4], &[0x03, 5, 0, 0xc0]);
        let b = BrushInfo {
            name: b"Wm1".to_vec(),
            role: Some(dc_world::ObjectRole::PlainWorldModel),
            pos: [1.0, 2.0, 3.0],
            rot: [0.0; 3],
            visible: true,
            solid: false,
            rayhit: false,
            blend: 2,
            fog_disable: false,
            class: BrushClass::Plain,
            sky: None,
        };
        assert_eq!(brush_flags(&b), (0x0101, 0x0042, 1, 0));
        let r = brush_record(6, &b, &pbox);
        assert_eq!(r.as_bytes()[0], 0x0b);
        let f = flag_record(
            7,
            1,
            ModelEntry {
                file: 9,
                subs: vec![],
                textures: vec![10, 11],
                styles: vec![12],
            },
            [0.0; 3],
            false,
            &pbox,
        );
        // cf NEW | POS | ANIM | ATTACH (0x0123: the marker byte form 0xa3 0x01)
        assert_eq!(&f.as_bytes()[..2], &[0xa3, 0x01]);
        let mut c =
            crate::synth_client::SynthClient::new(crate::synth_client::SynthConfig::named("x"));
        let mut g = BitWriter::new();
        g.write_u8(8);
        for rec in [&z, &r, &f] {
            g.write_u32(rec.bit_len() as u32).append(rec);
        }
        assert!(c.read_group_for_test(&g).is_ok());
        assert_eq!(c.seen.objects.len(), 3);
        assert_eq!(c.seen.objects[&5].blob_type, Some(OBJ_ZONE_TYPE));
        assert_eq!(c.seen.objects[&7].blob_type, Some(OBJ_PICKUP_TYPE));
        assert_eq!(c.seen.objects[&6].kind, OBJ_BRUSH);
        assert_eq!(
            c.seen.objects[&7].flags, None,
            "TDM form: no flag words (not drawn)"
        );
        // CLI-584: a flag-mode room's flag carries the visible flag word
        assert!(!flags_shown(1) && flags_shown(7) && flags_shown(8));
        let shown = flag_record(
            9,
            1,
            ModelEntry {
                file: 9,
                subs: vec![],
                textures: vec![10, 11],
                styles: vec![12],
            },
            [0.0; 3],
            true,
            &pbox,
        );
        let mut g = BitWriter::new();
        g.write_u8(8);
        g.write_u32(shown.bit_len() as u32).append(&shown);
        assert!(c.read_group_for_test(&g).is_ok());
        assert_eq!(c.seen.objects[&9].flags, Some(0x0001));
    }

    fn sound(ambient: bool, inner: f32, outer: f32, volume: u32, prio: u8) -> SoundInfo {
        SoundInfo {
            name: b"S1".to_vec(),
            pos: [0.0, 100.0, 0.0],
            file: "SND\\a.wav".into(),
            priority: prio,
            inner,
            outer,
            volume,
            pitch: 1.0,
            ambient,
            looped: true,
            start_on: true,
            attached: false,
        }
    }

    /// The tracked-sound record bytes against the reference stack's records of tdm4 c6 seq 807 (the bytes before pos48):
    /// the wind loop (ambient, 10000 / 10000, priority 0), a positional loop (800 / 1280 -> ratio 159, flags 0x136)
    /// and an ambient loop at volume 50 (64 / 256 -> ratio 63, flags 0x175, the volume byte); the synthetic client
    /// reads each to its end and gets the fields and the position back.
    #[test]
    fn tracked_sound_records_match_the_reference_bytes() {
        let pbox = PosBox {
            min: [-4096.0; 3],
            max: [4096.0; 3],
        };
        let cases: [(u16, u16, SoundInfo, &str); 3] = [
            (
                214,
                8,
                sound(true, 10000.0, 10000.0, 100, 0),
                "000103d6000800073501001027ff00000000",
            ),
            (
                212,
                7,
                sound(false, 800.0, 1280.0, 100, 2),
                "000103d40007000736010200059f00000000",
            ),
            (
                208,
                6,
                sound(true, 64.0, 256.0, 50, 2),
                "000103d00006000775010200013f3200000000",
            ),
        ];
        let mut sc = crate::synth_client::SynthConfig::named("x");
        sc.box_bounds = Some((pbox.min, pbox.max));
        let mut c = crate::synth_client::SynthClient::new(sc);
        let mut g = BitWriter::new();
        g.write_u8(8);
        for (track, file, s, hex) in &cases {
            let r = tracked_sound_record(*track, *file, s, &pbox);
            let head: String = r.as_bytes()[..hex.len() / 2]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            assert_eq!(&head, hex, "track {track}");
            assert_eq!(
                r.bit_len(),
                hex.len() * 4 + 48,
                "then pos48 and nothing else"
            );
            g.write_u32(r.bit_len() as u32).append(&r);
        }
        c.read_group_for_test(&g).unwrap();
        assert!(c.seen.objects.is_empty(), "sounds are not objects");
        let w = c.seen.sounds[&208];
        assert_eq!(
            (w.tmask, w.sfile, w.dmask, w.sflags, w.prio, w.outer, w.ratio, w.volume, w.tag),
            (3, 6, 7, 0x175, 2, 256, 63, Some(50), 0)
        );
        let p = w.pos.unwrap();
        assert!((p[1] - 100.0).abs() < 0.2 && p[0].abs() < 0.2, "{p:?}");
        assert_eq!(c.seen.sounds[&212].sflags, 0x136);
        assert_eq!(c.seen.sounds[&214].volume, None);
    }

    /// The pain and bounce event-sound record bytes (DSV-64) against the reference stack's records of tdm4 c6 seq 2968
    /// and seq 7690 up to their pos48 (the head of each record); the synthetic client reads each back.
    #[test]
    fn event_sound_records_match_the_reference_bytes() {
        // DSV-64: the pain record of tdm4 c6 seq 2968 and the bounce record of seq 7690 up to their pos48
        let pbox = PosBox {
            min: [-4096.0; 3],
            max: [4096.0; 3],
        };
        for (track, file, form, hex) in [
            (
                0x019d,
                0x0018,
                crate::sounds::PAIN_FORM,
                "0001039d01180007320102f4011100000000",
            ),
            (
                0x01a6,
                0x0021,
                crate::sounds::BOUNCE_FORM,
                "000103a601210007320101e8031100000000",
            ),
        ] {
            let r = event_sound_record(track, file, form, [1.0, 2.0, 3.0], &pbox);
            let head: String = r.as_bytes()[..hex.len() / 2]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            assert_eq!(head, hex);
            assert_eq!(r.bit_len(), hex.len() * 4 + 48);
            let mut sc = crate::synth_client::SynthConfig::named("x");
            sc.box_bounds = Some((pbox.min, pbox.max));
            let mut c = crate::synth_client::SynthClient::new(sc);
            let mut g = BitWriter::new();
            g.write_u8(8);
            g.write_u32(r.bit_len() as u32).append(&r);
            c.read_group_for_test(&g).unwrap();
            let seen = c.seen.sounds[&track];
            assert_eq!(
                (seen.sflags, seen.prio, seen.outer, seen.ratio),
                (0x132, form.prio, form.outer, 17)
            );
        }
    }

    /// Flags and ratio rules: pitch adds 0x80 and the f32, the ratio truncates, a zero outer radius sends 255.
    #[test]
    fn sound_flag_and_ratio_rules() {
        let mut s = sound(false, 512.0, 1024.0, 150, 2);
        assert_eq!(inner_ratio(&s), 127, "127.5 truncated");
        assert_eq!(world_sound_bits(&s), 0x0130 | 0x2 | 0x4 | 0x40);
        s.pitch = 1.5;
        s.looped = false;
        assert_eq!(world_sound_bits(&s), 0x0130 | 0x2 | 0x40 | 0x80);
        let pbox = PosBox {
            min: [-10.0; 3],
            max: [10.0; 3],
        };
        let r = tracked_sound_record(5, 1, &s, &pbox);
        // head 3 + track 2 + file 2 + dmask 1 + flags 2 + prio 1 + radius 3 + volume 1 + pitch 4 + tag 4 + pos 6
        assert_eq!(r.bit_len(), 29 * 8);
        s.outer = 0.0;
        s.inner = 0.0;
        assert_eq!(inner_ratio(&s), 255);
    }

    /// World sounds at load: switched-on ones in file order with `World` (a start script may switch one on or
    /// off), all with `All`, none with `Mode` / `Off`.
    #[test]
    fn plan_sounds_follows_the_start_switch_and_scripts() {
        let mut w = crate::world::world_from_bytes(
            &crate::world::synthetic_world_bytes(),
            "Worlds\\S.dat",
            "S",
        )
        .unwrap();
        let mut a = sound(true, 1.0, 2.0, 100, 0);
        a.name = b"SndA".to_vec();
        let mut b = a.clone();
        b.name = b"SndB".to_vec();
        b.start_on = false;
        let mut c = a.clone();
        c.name = b"SndC".to_vec();
        w.sounds = vec![a, b, c];
        let ids = |w: &crate::world::WorldInfo, which, trig: Option<&str>| {
            let mut n = 100u16;
            plan_sounds(w, which, trig, || {
                n += 1;
                n
            })
            .iter()
            .map(|o| match o {
                WorldObject::Sound { obj, sound } => (*obj, sound.name.clone()),
                _ => unreachable!(),
            })
            .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(&w, WorldObjects::World, None),
            vec![(101, b"SndA".to_vec()), (102, b"SndC".to_vec())]
        );
        assert_eq!(ids(&w, WorldObjects::All, None).len(), 3);
        assert!(ids(&w, WorldObjects::Mode, None).is_empty());
        assert!(ids(&w, WorldObjects::Off, None).is_empty());
        w.scripts.triggers = vec![(
            b"start".to_vec(),
            vec![b"msg sndb on; msg SndA off".to_vec()],
        )];
        assert_eq!(
            ids(&w, WorldObjects::World, Some("start")),
            vec![(101, b"SndB".to_vec()), (102, b"SndC".to_vec())]
        );
        let o = WorldObject::Sound {
            obj: 9,
            sound: w.sounds[0].clone(),
        };
        assert!(o.is_sound());
        assert_eq!(o.files(), vec!["SND\\a.wav".to_string()]);
    }
}
