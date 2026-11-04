//! Session message ids, the client -> server message writers, and readers for the server -> client shell
//! messages the bot reacts to. Names are the specs' (`GLOSSARY_UDP.md` S.A / S.B, `GAME_MESSAGES_2009_S2C.md`
//! section 12, `GAME_MESSAGES_2009_C2S.md` Appendix B); every layout carries its section.
//!
//! The readers are written from the client-side spec sections, independently of the server writers in
//! `dc-dserver`, so a codec mistake on either side shows up when the two meet.

use dc_wire::udp::{read_quat128, write_quat128, BitReader, BitWriter};

/// Engine-level session message ids, server -> client.
// SPEC: UDP_SESSION_2009_A.md §4.1, UDP_SESSION_2009_B.md §2.4
pub mod s2c {
    pub const VERSION: u8 = 4;
    pub const LEAVE_WORLD: u8 = 5;
    pub const LOAD_WORLD: u8 = 6;
    pub const VIEW_OBJECT: u8 = 7;
    pub const OBJ_GROUP: u8 = 8;
    pub const OBJ_STREAM: u8 = 10;
    pub const CLIENT_ID: u8 = 12;
    pub const SHELL_MSG: u8 = 13;
    pub const BUNDLE: u8 = 14;
    pub const EVAR: u8 = 15;
    pub const SKY_TABLE: u8 = 16;
    pub const SHELL_OBJ_MSG: u8 = 17;
    pub const PRELOAD: u8 = 19;
    pub const FILE_TOUCH: u8 = 20;
    pub const MODEL_CACHE: u8 = 21;
    pub const DIR_LIGHT: u8 = 22;
    pub const MODEL_ADD_CHILD: u8 = 23;
    /// Handled through the client's fallback (no slot in the table).
    pub const FILE_LIST: u8 = 0x32;

    /// Our name of an id for logs.
    pub fn name(id: u8) -> &'static str {
        match id {
            VERSION => "S2C_VERSION",
            LEAVE_WORLD => "S2C_LEAVE_WORLD",
            LOAD_WORLD => "S2C_LOAD_WORLD",
            VIEW_OBJECT => "S2C_VIEW_OBJECT",
            OBJ_GROUP => "S2C_OBJ_GROUP",
            OBJ_STREAM => "S2C_OBJ_STREAM",
            CLIENT_ID => "S2C_CLIENT_ID",
            SHELL_MSG => "S2C_SHELL_MSG",
            BUNDLE => "S2C_BUNDLE",
            EVAR => "S2C_EVAR",
            SKY_TABLE => "S2C_SKY_TABLE",
            SHELL_OBJ_MSG => "S2C_SHELL_OBJ_MSG",
            PRELOAD => "S2C_PRELOAD",
            FILE_TOUCH => "S2C_FILE_TOUCH",
            MODEL_CACHE => "S2C_MODEL_CACHE",
            DIR_LIGHT => "S2C_DIR_LIGHT",
            MODEL_ADD_CHILD => "S2C_MODEL_ADD_CHILD",
            FILE_LIST => "S2C_FILE_LIST",
            _ => "unknown",
        }
    }
}

/// Engine-level session message ids, client -> server.
// SPEC: UDP_SESSION_2009_A.md §5.1, UDP_SESSION_2009_B.md §2.5
pub mod c2s {
    pub const HELLO: u8 = 5;
    pub const BYE: u8 = 6;
    pub const RATE: u8 = 7;
    pub const LOAD_STATE: u8 = 9;
    pub const SHELL_MSG: u8 = 11;
    pub const FILE_ACK: u8 = 0x36;
}

/// Shell message ids (inside `S2C_SHELL_MSG` / `C2S_SHELL_MSG`; the two directions are separate id spaces).
pub mod shell_id {
    /// S2C console text `[text:zstr8]`.
    // SPEC: UDP_SESSION_2009_B.md §9.4
    pub const CONSOLE_TEXT: u8 = 0x00;
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (0x45 match begin, 0x44 round begin)
    pub const GS_ROUND_BEGIN: u8 = 0x44;
    pub const GS_MATCH_BEGIN: u8 = 0x45;
    /// S2C `GS_PLAYER_STATE` `[state:u8]` (the S12 carrier); C2S `UP_PLAYER_UPDATE` is 0x64.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.1; UDP_SESSION_2009_A_addendum_disasm.md A16
    pub const GS_PLAYER_STATE: u8 = 0x67;
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.2
    pub const GS_VIEW_ANGLES: u8 = 0x6a;
    /// S2C `GS_SPRINT_ALLOWED` `[allowed:bool1]` (same id as the C2S `UP_SPRINT_STATE`).
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.2 (0xca, own record +0x1f0)
    pub const GS_SPRINT_ALLOWED: u8 = 0xca;
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.2, §3.2.1
    pub const GS_PLAYER_INFO: u8 = 0x6b;
    pub const GS_PLAYER_LEFT: u8 = 0x6c;
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1
    pub const GS_KILL: u8 = 0x6d;
    /// S2C identify request (no body) -> C2S `UP_PLAYER_INFO` 0x72.
    // SPEC: GAME_MESSAGES_2009_S2C.md §5.1; GAME_MESSAGES_2009_C2S.md §5.1
    pub const GS_IDENTIFY_REQUEST: u8 = 0x72;
    /// S2C weapon-table reload (no body) -> C2S `UP_CLIENT_OPTIONS` 0x85.
    // SPEC: GAME_MESSAGES_2009_S2C.md §5.1; GAME_MESSAGES_2009_C2S.md §5.2
    pub const GS_RELOAD_WEAPON_TABLE: u8 = 0x73;
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.2
    pub const GS_MATCH_SETTINGS: u8 = 0x7d;
    /// `GS_PLAYER_POINTS`: kind 1 advances the shot counter (`read_points_shot_seed`).
    // SPEC: GAME_MESSAGES_2009_S2C.md 0xda row
    pub const GS_PLAYER_POINTS: u8 = 0xda;
    /// S2C switching-worlds state `[state:u8]` (S11).
    // SPEC: UDP_SESSION_2009_A.md §4.11; addendum A12
    pub const SWITCH_STATE: u8 = 0x88;
    // SPEC: GAME_MESSAGES_2009_S2C.md §5.1
    pub const GS_GAME_ENDPOINT: u8 = 0x9e;
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.2; MOVEMENT_2009.md §12.4
    pub const GS_MOVER_UPDATE: u8 = 0xa0;
    // SPEC: GAME_MESSAGES_2009_S2C.md §5.1 (S2C 0xac), GAME_MESSAGES_2009_C2S.md §5.6 (C2S answer)
    pub const INTEGRITY: u8 = 0xac;
    // Match flow and HUD messages a client with its own HUD reads (server -> client).
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (0x3f result screen, 0x41 timer stop, 0x46 round result, 0x4a team score)
    pub const GS_MATCH_RESULT_SCREEN: u8 = 0x3f;
    pub const GS_ROUND_TIMER_STOP: u8 = 0x41;
    pub const GS_ROUND_RESULT: u8 = 0x46;
    pub const GS_TEAM_SCORE: u8 = 0x4a;
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.4 (0x65 player summary, 0xb5 round wins)
    pub const GS_PLAYER_SUMMARY: u8 = 0x65;
    pub const GS_ROUND_WINS: u8 = 0xb5;
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (0x66 health, armour, ammunition, money)
    pub const GS_PLAYER_STATUS: u8 = 0x66;
    // SPEC: GAME_MESSAGES_2009_S2C.md §6.1 (0x99 level exit, mode-rules stage)
    pub const GS_LEVEL_EXIT: u8 = 0x99;

    // client -> server
    // SPEC: GAME_MESSAGES_2009_C2S.md §2.1
    pub const UP_PLAYER_UPDATE: u8 = 0x64;
    pub const UP_PLAYER_INFO: u8 = 0x72;
    // SPEC: GAME_MESSAGES_2009_C2S.md §5.2
    pub const UP_CLIENT_OPTIONS: u8 = 0x85;
    // SPEC: GAME_MESSAGES_2009_C2S.md §5.4
    pub const UP_WORLD_READY: u8 = 0x87;
    // SPEC: GAME_MESSAGES_2009_C2S.md §5.7
    pub const UP_LEAVE_MATCH: u8 = 0xbd;
    /// `[pid:u32]`: the client misses the record (object) of this player.
    // SPEC: GAME_MESSAGES_2009_C2S.md §5.5
    pub const UP_ROSTER_QUERY: u8 = 0x8c;
    /// `[sprinting:bool1][stopped:bool1]`, sent when the pair changes.
    // SPEC: GAME_MESSAGES_2009_C2S.md §2.3
    pub const UP_SPRINT_STATE: u8 = 0xca;
}

fn with_id(id: u8) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(id);
    w
}

// ------------------------------------------------------------------------------------------------- engine writers

/// Size of the hello blob the 2009 shell attaches.
// SPEC: UDP_SESSION_2009_A.md §2.3 (0x102 bytes), §5.2
pub const HELLO_BLOB_BYTES: usize = 0x102;

/// The hello blob: 8-bit name at 0x00 (63 bytes + NUL), a second 8-bit string at 0x40, zeros at 0x80 / 0x81, the
/// UTF-16 name at 0x82 (NUL-terminated). Characters outside 8 bits become `?` in the 8-bit copy (the 2009 client
/// converts with the system ANSI code page; the server must identify by the UTF-16 copy, addendum A5).
// SPEC: UDP_SESSION_2009_A.md §2.3; UDP_SESSION_2009_A_addendum_disasm.md A5
pub fn hello_blob(name: &str, tag: &str) -> Vec<u8> {
    let mut blob = vec![0u8; HELLO_BLOB_BYTES];
    let eight = |s: &str| -> Vec<u8> {
        s.chars()
            .map(|c| if (c as u32) < 0x100 { c as u8 } else { b'?' })
            .take(63)
            .collect()
    };
    let n8 = eight(name);
    blob[..n8.len()].copy_from_slice(&n8);
    let t8 = eight(tag);
    blob[0x40..0x40 + t8.len()].copy_from_slice(&t8);
    for (i, u) in name.encode_utf16().take(63).enumerate() {
        blob[0x82 + 2 * i..0x84 + 2 * i].copy_from_slice(&u.to_le_bytes());
    }
    blob
}

/// `C2S_HELLO`: `[5][blob_len:u16][blob]`, reliable, the first message on the link.
// SPEC: UDP_SESSION_2009_A.md §5.2, §9.2 S1 (`05`, `02 01`, 258 bytes)
pub fn hello(name: &str, tag: &str, ticket: Option<&dc_wire::seatticket::Ticket>) -> BitWriter {
    let mut blob = hello_blob(name, tag);
    // DC-DEFAULT: LG-327 (proposed, project decision) our seat ticket after the 2009 blob
    if let Some(t) = ticket {
        dc_wire::seatticket::append_to_blob(&mut blob, t);
    }
    let mut w = with_id(c2s::HELLO);
    w.write_u16(blob.len() as u16).write_raw(&blob);
    w
}

/// `C2S_FILE_ACK`: one u16 per announced entry, bit 15 set (the 2009 client accepts every entry).
// SPEC: UDP_SESSION_2009_A.md §4.4, §5.4
pub fn file_ack(ids: &[u16]) -> BitWriter {
    let mut w = with_id(c2s::FILE_ACK);
    for &id in ids {
        w.write_u16(id | 0x8000);
    }
    w
}

/// `C2S_LOAD_STATE`: 0 = world loaded, 1 = preload done.
// SPEC: UDP_SESSION_2009_A.md §5.3, §9.2 S6a (`09 00`) / S8a (`09 01`)
pub fn load_state(state: u8) -> BitWriter {
    let mut w = with_id(c2s::LOAD_STATE);
    w.write_u8(state);
    w
}

/// `C2S_RATE`: `rate_8k = clamp(rate_bps / 8000, 0, 65535)`, no sound acknowledgements.
// SPEC: UDP_SESSION_2009_A.md §5.5; addendum A14; UDP_SESSION_2009_B.md §10.2
pub fn rate(rate_bps: u32) -> BitWriter {
    let mut w = with_id(c2s::RATE);
    w.write_u16((rate_bps / 8000).min(65_535) as u16);
    w
}

/// `C2S_BYE`: no body.
// SPEC: UDP_SESSION_2009_A.md §5.6
pub fn bye() -> BitWriter {
    with_id(c2s::BYE)
}

/// `C2S_SHELL_MSG`: `[11][shell_id][body]`.
// SPEC: UDP_SESSION_2009_B.md §6.1, §11.1; GAME_MESSAGES_2009_C2S.md §1.2
pub fn shell_msg(shell: u8, body: &BitWriter) -> BitWriter {
    let mut w = with_id(c2s::SHELL_MSG);
    w.write_u8(shell).append(body);
    w
}

// ------------------------------------------------------------------------------------------------- game writers

/// Clan part of `UP_PLAYER_INFO`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClanPart {
    pub band_name: String,
    pub clan_id: u32,
    pub mark_a: u32,
    pub mark_b: u32,
    pub word_c: u32,
}

/// The look bytes the bot sends in `UP_PLAYER_INFO`: no overridable-model row (byte 14 = -1). All zeros would select
/// override row 0, an infected face whose looping eye FX the real client shows on the bot.
// OBSERVED: capture 20261025_065016_tdm6 seq 219 (the look the reference stack sent for the bot, accepted
// by the real client); 20261025_125432_rust6 conn 6 seq 271 (the bot's all-zero look); ae-triage-reddot2-blast §1.3
// DC-DEFAULT: BOT-29 (proposed)
pub const BOT_LOOK: [i8; 15] = [0, 0, 0, 0, 0, 1, 0, 0, 0, 0, -1, -1, 0, 0, -1];

/// The fields of `UP_PLAYER_INFO` (0x72). The 2009 client fills them from its lobby-side profile; the bot fills
/// what the lobby told it, [`BOT_LOOK`] and empty look strings, and 0 for the profile values it does not have
/// (BOT-3, BOT-29, proposed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfo {
    /// Own slot in the room (0x383 +0x12 / 0x385 +0xe).
    pub room_slot: u32,
    /// 0x303 +0xe.
    pub char_no: u32,
    /// 0x301 +0x1e.
    pub session_idx: u16,
    pub exp: u32,
    pub look: [i8; 15],
    pub look_a: Vec<u8>,
    pub look_b: Vec<u8>,
    pub r: [i8; 4],
    pub r5: u32,
    /// 1 or 2.
    pub team: i8,
    pub r6: i8,
    pub r7: u32,
    pub gp: u32,
    /// `[item_no:u16][e1:u8][e2:u8][e3:u8]` entries.
    pub loadout: Vec<(u16, u8, u8, u8)>,
    pub clan: Option<ClanPart>,
    pub word_d: u32,
    pub nation: u32,
    pub flag_e: bool,
    pub flag_f: bool,
}

// DC-DEFAULT: BOT-3, BOT-29 (proposed)
impl Default for PlayerInfo {
    fn default() -> Self {
        PlayerInfo {
            room_slot: 0,
            char_no: 0,
            session_idx: 0,
            exp: 0,
            look: BOT_LOOK,
            look_a: Vec::new(),
            look_b: Vec::new(),
            r: [0; 4],
            r5: 0,
            team: 0,
            r6: 0,
            r7: 0,
            gp: 0,
            loadout: Vec::new(),
            clan: None,
            word_d: 0,
            nation: 0,
            flag_e: false,
            flag_f: false,
        }
    }
}

/// `UP_PLAYER_INFO` body (after the shell id), field order and widths as written by cshell+0x23ccb0.
// SPEC: GAME_MESSAGES_2009_C2S.md §5.1 (DISASM cshell+0x23cd2a..0x23d1ea)
pub fn player_info_body(p: &PlayerInfo) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u32(p.room_slot)
        .write_u32(p.char_no)
        .write_u16(p.session_idx)
        .write_u32(p.exp);
    // look: 15 signed bytes, two zstr8
    for b in p.look {
        w.write_s8(b);
    }
    w.write_zstr8(&p.look_a).write_zstr8(&p.look_b);
    for b in p.r {
        w.write_s8(b);
    }
    w.write_u32(p.r5)
        .write_s8(p.team)
        .write_s8(p.r6)
        .write_u32(p.r7)
        .write_u32(p.gp);
    let n = p.loadout.len().min(255);
    w.write_u8(n as u8);
    for &(item, e1, e2, e3) in &p.loadout[..n] {
        w.write_u16(item).write_u8(e1).write_u8(e2).write_u8(e3);
    }
    w.write_bit(p.clan.is_some());
    if let Some(c) = &p.clan {
        let units: Vec<u16> = c.band_name.encode_utf16().collect();
        w.write_zstr16_presumed(&units);
        // clan_id:u64, low 32 bits from the clan id, high 32 bits written as 0
        w.write_u64(c.clan_id as u64)
            .write_u32(c.mark_a)
            .write_u32(c.mark_b)
            .write_u32(c.word_c);
    }
    w.write_u32(p.word_d)
        .write_u32(p.nation)
        .write_bit(p.flag_e)
        .write_bit(p.flag_f);
    w
}

/// `UP_CLIENT_OPTIONS` (0x85) body: `[opt_a:u8][opt_b:bool1]`.
// SPEC: GAME_MESSAGES_2009_C2S.md §5.2
pub fn client_options_body(opt_a: u8, opt_b: bool) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(opt_a).write_bit(opt_b);
    w
}

/// Bits of `pu_flags` used by the bot.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (flag table)
pub mod pu {
    /// View block `[yaw8c:u8][body_angles:vec96]`, then `[aux: 6 x u32]`.
    pub const VIEW8: u16 = 0x0020;
    /// The reliable state form `[0x0010][state:u8]`.
    pub const STATE_FORM: u16 = 0x0010;
    /// `[aim_q:quat128]` after the view block (and the offset block).
    pub const AIM_QUAT: u16 = 0x0100;
    /// `[eye_offset:vec96]` after the aux words: eye point minus body position.
    // OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.2 (d), (g) (set in 2,338 of 2,338 updates of the 2009 client)
    pub const EYE_OFFSET: u16 = 0x0001;
    /// `[weapon_f:f32]` after the aim block (the current weapon's float, sent when it changed).
    pub const WEAPON_F: u16 = 0x0200;
    /// `[cam_point:vec96]` after the weapon float: the absolute camera position, sent when it moved.
    // OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.2 (g) (cam_point - trailer position = eye offset standing)
    pub const CAM_POINT: u16 = 0x0400;
    /// No field; set in every update of the 2009 client (`local_player`+0x1c4, meaning UNPROVEN).
    // OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.2 (d) (2,338 of 2,338), CAPTURE_tdm4_KILL_VALIDATION.md §5
    pub const FLAG_1000: u16 = 0x1000;
    /// The flag bits without a field (0x0800, 0x1000, 0x2000) a caller may add.
    pub const NO_FIELD_MASK: u16 = 0x3800;
}

/// The optional blocks of a 0x64 update that the bot does not send but a playing client does: the eye offset
/// (0x0001), the weapon float (0x0200), the camera point (0x0400), and flag bits without a field.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (field order: view block, aux, eye_offset, aim_q, weapon_f, cam_point)
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UpdateExtras {
    pub eye_offset: Option<[f32; 3]>,
    pub weapon_f: Option<f32>,
    pub cam_point: Option<[f32; 3]>,
    /// Only the bits of [`pu::NO_FIELD_MASK`] are used.
    pub flag_bits: u16,
}

/// `yaw8c = trunc(yaw * 40.425354)` (254 / 2π, toward zero), yaw wrapped to (-π, π] as `atan2` returns it.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (yaw8c = yaw(q) * 40.425354); UDP_SESSION_2009_B.md §10.3
// OBSERVED: tdm4 conn 6, 1,103 of 1,103 updates truncate toward zero (capval-kill §5; capval-transport V18, tdm1)
pub fn yaw8c(yaw: f32) -> u8 {
    let tau = std::f32::consts::TAU;
    let mut y = yaw % tau;
    if y > std::f32::consts::PI {
        y -= tau;
    } else if y <= -std::f32::consts::PI {
        y += tau;
    }
    ((y * 40.425_354).trunc().clamp(-128.0, 127.0) as i8) as u8
}

/// The movement trailer of 0x64 (312 bits, nothing follows).
// SPEC: MOVEMENT_2009.md §12.2; UDP_SESSION_2009_B_addendum_disasm.md B18
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveTrailer {
    pub move_seq: u8,
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    /// Client time in seconds.
    pub time: f32,
    pub on_ground: u8,
    pub surface: u8,
    /// The standing ids (low 32 bits, high 32 bits); `u64::MAX` = -1 / -1 (nothing).
    pub standing: u64,
}

/// One unreliable player update of the live (derived) builder cshell+0x5f7f0 with the `0x0020` view block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveReport {
    /// Pitch, yaw, roll in radians (body angles, controller +0x68..+0x70).
    pub angles: [f32; 3],
    /// `local_player`+0x40c .. +0x420 (two 3-float vectors; meaning UNPROVEN).
    pub aux: [u32; 6],
    /// The aim rotation (`pu_flags` 0x0100, `quat128` x, y, z, w), sent when it changed since the last update.
    pub aim: Option<[f32; 4]>,
    /// The frame's control word (mover+0x24: FWD 0x1, BACK 0x2, JUMP 0x10, CROUCH 0x20, RUN 0x200, ...).
    pub control: u32,
    pub trailer: MoveTrailer,
}

/// `UP_PLAYER_UPDATE` (0x64) unreliable body: `[pu_flags][yaw8c][body_angles][aux][control][trailer]`.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (layout, DISASM cshell+0x5fc0a..0x5ff80); UDP_SESSION_2009_B_addendum_disasm.md B16
pub fn player_update_body(u: &MoveReport) -> BitWriter {
    player_update_body_ext(u, &UpdateExtras::default())
}

/// `UP_PLAYER_UPDATE` (0x64) unreliable body with the optional blocks of `x`: `[pu_flags][yaw8c][body_angles][aux]
/// [eye_offset][aim_q][weapon_f][cam_point][control][trailer]`, each optional block announced by its flag bit.
/// With `UpdateExtras::default()` the result equals [`player_update_body`].
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (layout and field order, DISASM cshell+0x5fc0a..0x5ff80)
// OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.2 (c) (every 2009 update parses with this order, 0 bits left)
pub fn player_update_body_ext(u: &MoveReport, x: &UpdateExtras) -> BitWriter {
    let mut w = BitWriter::new();
    let mut flags = pu::VIEW8 | if u.aim.is_some() { pu::AIM_QUAT } else { 0 };
    if x.eye_offset.is_some() {
        flags |= pu::EYE_OFFSET;
    }
    if x.weapon_f.is_some() {
        flags |= pu::WEAPON_F;
    }
    if x.cam_point.is_some() {
        flags |= pu::CAM_POINT;
    }
    flags |= x.flag_bits & pu::NO_FIELD_MASK;
    w.write_u16(flags)
        .write_u8(yaw8c(u.angles[1]))
        .write_f32(u.angles[0])
        .write_f32(u.angles[1])
        .write_f32(u.angles[2]);
    // pu_flags & 0x0060 -> aux
    for a in u.aux {
        w.write_u32(a);
    }
    // pu_flags & 0x0001 -> eye_offset
    if let Some(e) = x.eye_offset {
        for c in e {
            w.write_f32(c);
        }
    }
    // pu_flags & 0x0100 -> aim_q (writer slot +0x4c = quat128, x, y, z, w)
    // SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (0x0100 aim_q:quat128); UDP_SESSION_2009_B_addendum_disasm.md B16
    if let Some(q) = u.aim {
        write_quat128(&mut w, q);
    }
    // pu_flags & 0x0200 -> weapon_f, 0x0400 -> cam_point
    if let Some(f) = x.weapon_f {
        w.write_f32(f);
    }
    if let Some(c) = x.cam_point {
        for v in c {
            w.write_f32(v);
        }
    }
    w.write_u32(u.control);
    let t = &u.trailer;
    w.write_u8(t.move_seq);
    for c in t.pos {
        w.write_f32(c);
    }
    for c in t.vel {
        w.write_f32(c);
    }
    w.write_f32(t.time)
        .write_u8(t.on_ground)
        .write_u8(t.surface)
        .write_u64(t.standing);
    w
}

/// The reliable state form of 0x64: `[pu_flags = 0x0010][state:u8]`.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (reliable state form, cshell+0x5f8b5..0x5f96f)
pub fn player_state_form_body(state: u8) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u16(pu::STATE_FORM).write_u8(state);
    w
}

/// Answer to the server's 0xac sub 0 with `check = 0x100`: `[0][version:u16][word:u32]`.
// SPEC: GAME_MESSAGES_2009_C2S.md §5.6 (sub 0 reply; `word` source UNPROVEN)
pub fn integrity_reply_body(version: u16, word: u32) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(0).write_u16(version).write_u32(word);
    w
}

// ------------------------------------------------------------------------------------------------- readers

fn vec96(r: &mut BitReader<'_>) -> [f32; 3] {
    [r.read_f32(), r.read_f32(), r.read_f32()]
}

/// A roster record from `GS_PLAYER_INFO` (0x6b).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RosterRecord {
    pub name: String,
    pub pid: u8,
    pub account_no: u32,
    /// The player's world object (`objref16`, 0xFFFF = none).
    pub character: u16,
    pub kills: i16,
    pub deaths: u8,
    pub team: u8,
    pub kind: u8,
    /// Field 13 `flag_b`: the player's dead state (record +0x74). The 2009 roster handler hands the state to the marker,
    /// the scoreboard and the maps on every record, so a refresh after a respawn clears the grey look.
    // EVIDENCE: cshell+0x8a760 (marker add / refresh: an existing entry of the client id takes the passed flag at
    // marker +0x30), DISASM cshell+0x2ce9c (the 0x6b handler calls it for every record with a team); SPEC:
    // GAME_MESSAGES_2009_S2C.md §3.2.1 field 13 (record +0x74, set to 1 by 0x6d); that the pushed slot is field 13
    // is PLAUSIBLE (the dserver sends kind 2 `dead` 0 after a spawn, CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §2.5)
    pub dead: bool,
    /// False when the message ended before the last field (a layout mismatch).
    pub complete: bool,
    /// Field 17b `h2` (record +0x210): the rank the world mark shows as an icon (client-hudgaps).
    // DISASM: cshell+0x2c983..0x2c990 (stored to the slot the marker call reads at cshell+0x2ce8e)
    pub rank: u32,
    /// Field 6: the 15 look bytes in wire order (w2-board: byte 4 is the board's and the death panel's level index,
    /// record +0x1a4).
    // EVIDENCE: cshell+0x1c2a90 (the fifth byte read lands at look +0x10, clamped to >= 0), cshell+0x2a950 (record
    // +0x1a4 := that slot)
    pub look: [i8; 15],
    /// Fields 9-11 `v64`, `v68`, `v6c` (record +0x64 / +0x68 / +0x6c): the board's mission column values.
    // DISASM: cshell+0x2c7db..0x2c7fe (the three bytes), cshell+0x2cce4..0x2ccef (passed to the board row as its
    // mission and `%d/%d` values)
    pub mission: [u8; 3],
    /// Field 17a `h1` (record +0x1d8): the player name's colour word (`0xAARRGGBB`), 0 = the element's default.
    // DISASM: cshell+0x2c97c..0x2c97f (read into edi), cshell+0x2ccaf (passed to the board row); EVIDENCE:
    // cshell+0xcbea0 (the death panel's name colour)
    pub name_colour: u32,
    /// Fields 15-16: the clan block (`None` when `has_group` is clear).
    pub clan: Option<RosterClan>,
}

/// The clan block of `GS_PLAYER_INFO` (field 16).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 field 16 (record +0x1b0, +0x1cc low half, +0x1d0, +0x1d4, +0x1dc)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RosterClan {
    /// The clan name (record +0x1b0).
    pub name: String,
    /// `group_no` (record +0x1cc holds its low half): 0 hides the clan name on the death panel.
    pub group_no: u64,
    /// `g1`, `g2` (record +0x1d0 / +0x1d4): the two clan mark layer ids.
    pub mark: [u32; 2],
    /// `g3` (record +0x1dc): the clan name's colour word (`0xAARRGGBB`), 0 = the element's default.
    pub colour: u32,
}

/// Reads `GS_PLAYER_INFO` (0x6b) after the shell id.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 (fields 1-21, DISASM cshell+0x2c75f..0x2c9fa)
pub fn read_player_info(r: &mut BitReader<'_>) -> RosterRecord {
    let mut p = RosterRecord {
        name: String::from_utf8_lossy(&r.read_zstr8()).into_owned(),
        pid: r.read_u8(),
        account_no: r.read_u32(),
        ..RosterRecord::default()
    };
    let _flag_a = r.read_bit();
    p.character = r.read_u16();
    for b in p.look.iter_mut() {
        *b = r.read_s8();
    }
    r.read_zstr8();
    r.read_zstr8();
    p.kills = r.read_s16();
    p.deaths = r.read_u8();
    for v in p.mission.iter_mut() {
        *v = r.read_u8();
    }
    p.team = r.read_u8();
    p.dead = r.read_bit();
    p.kind = r.read_u8();
    if r.read_bit() {
        let name = r.read_zstr16();
        p.clan = Some(RosterClan {
            name: String::from_utf16_lossy(&name),
            group_no: r.read_u64(),
            mark: [r.read_u32(), r.read_u32()],
            colour: r.read_u32(),
        });
    }
    p.name_colour = r.read_u32();
    p.rank = r.read_u32();
    r.read_bit();
    r.read_bit();
    r.read_bit();
    r.read_u8();
    r.read_u8();
    r.read_u32();
    r.read_u32();
    if r.remaining_bits() >= 16 {
        r.read_u16();
        p.complete = true;
    }
    p
}

/// `GS_MOVER_UPDATE` (0xa0) as far as the bot uses it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MoverUpdate {
    pub flags: u16,
    /// Flag 0x0020: `move_seq`, position, box.
    pub teleport: Option<(u8, [f32; 3], [f32; 3])>,
    pub friction: Option<f32>,
    pub freeze: Option<bool>,
    pub release: Option<bool>,
    pub complete: bool,
}

/// Reads `GS_MOVER_UPDATE` (0xa0): `[flags:u16]` then the blocks 0x0020, 0x0004, 0x0001, 0x0008, 0x0080, 0x0100.
// SPEC: MOVEMENT_2009.md §12.4 (blocks in this order); GAME_MESSAGES_2009_S2C.md §4.2
pub fn read_mover_update(r: &mut BitReader<'_>) -> MoverUpdate {
    let mut short = r.remaining_bits() < 16;
    let mut m = MoverUpdate {
        flags: r.read_u16(),
        ..MoverUpdate::default()
    };
    let mut need = |r: &BitReader<'_>, n: usize| {
        if r.remaining_bits() < n {
            short = true;
        }
    };
    let f = m.flags;
    if f & 0x0020 != 0 {
        need(r, 8 + 96 + 96);
        let seq = r.read_u8();
        let pos = vec96(r);
        let dims = vec96(r);
        m.teleport = Some((seq, pos, dims));
    }
    if f & 0x0004 != 0 {
        need(r, 8);
        let n = r.read_u8();
        for _ in 0..n {
            need(r, 8 + 96 + 32 + 32 + 8 + 16 + 8);
            r.read_u8();
            vec96(r);
            r.read_f32();
            r.read_f32();
            r.read_u8();
            r.read_u16();
            r.read_u8();
        }
        need(r, 32);
        m.friction = Some(r.read_f32());
    }
    if f & 0x0001 != 0 {
        r.read_zstr8();
        need(r, 8);
        let n = r.read_u8();
        for _ in 0..n {
            r.read_zstr8();
        }
        need(r, 1);
        r.read_bit();
    }
    if f & 0x0008 != 0 {
        need(r, 32);
        m.friction = Some(r.read_f32());
    }
    if f & 0x0080 != 0 {
        need(r, 1);
        m.freeze = Some(r.read_bit());
    }
    if f & 0x0100 != 0 {
        need(r, 1);
        m.release = Some(r.read_bit());
    }
    m.complete = !short;
    m
}

/// `GS_VIEW_ANGLES` (0x6a): `[kind:u8][angles:vec96]` (kind 0 = pitch, yaw, roll; kind 1 = yaw only).
// SPEC: GAME_MESSAGES_2009_S2C.md §4.2; MOVEMENT_2009.md §12.5
pub fn read_view_angles(r: &mut BitReader<'_>) -> (u8, [f32; 3]) {
    let k = r.read_u8();
    (k, vec96(r))
}

/// `GS_GAME_ENDPOINT` (0x9e): `[mode_class:u8]`, then `[host:zstr8][port:u32]` when non-zero.
// SPEC: GAME_MESSAGES_2009_S2C.md §5.1
pub fn read_game_endpoint(r: &mut BitReader<'_>) -> (u8, Option<(String, u32)>) {
    let mc = r.read_u8();
    if mc == 0 {
        return (0, None);
    }
    let host = String::from_utf8_lossy(&r.read_zstr8()).into_owned();
    (mc, Some((host, r.read_u32())))
}

/// `GS_INTEGRITY` (0xac) request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityAsk {
    /// Sub 0 with its check word (0x100 -> the client answers sub 0).
    Version(u16),
    /// Sub 1: a nonce challenge (answer needs the client's own checksums; not reproduced).
    Nonce(u32),
    /// Subs 2 and 4 make the client record a disconnect reason; 3 does nothing.
    Other(u8),
}

/// Reads `GS_INTEGRITY` (0xac).
// SPEC: GAME_MESSAGES_2009_S2C.md §5.1 (sub 0: [version:u16]; sub 1: [nonce:u32][flag:bool1])
pub fn read_integrity(r: &mut BitReader<'_>) -> IntegrityAsk {
    match r.read_u8() {
        0 => IntegrityAsk::Version(r.read_u16()),
        1 => {
            let n = r.read_u32();
            r.read_bit();
            IntegrityAsk::Nonce(n)
        }
        s => IntegrityAsk::Other(s),
    }
}

/// `GS_KILL` (0x6d) head: victim, killer, kill kind, weapon, hit zone.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 (fields 1-5)
pub fn read_kill_head(r: &mut BitReader<'_>) -> (u8, u8, u8, u16, u8) {
    (
        r.read_u8(),
        r.read_u8(),
        r.read_u8(),
        r.read_u16(),
        r.read_u8(),
    )
}

/// `GS_MATCH_SETTINGS` (0x7d) head: own team, goal, two scores, round limit, round number.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2 (0x7d layout)
pub fn read_match_settings(r: &mut BitReader<'_>) -> (i8, u8, i8, i8, i8, i8) {
    r.read_zstr8();
    r.read_u8();
    let own_team = r.read_s8();
    r.read_u32();
    (
        own_team,
        r.read_u8(),
        r.read_s8(),
        r.read_s8(),
        r.read_s8(),
        r.read_s8(),
    )
}

/// The `GS_PLAYER_POINTS` 0xda kind that advances the shot counter.
// SPEC: GAME_MESSAGES_2009_S2C.md 0xda row (kind 1: local_player+0x1fc += delta)
pub const POINTS_SHOT_SEED: u8 = 1;

/// `own_value` of a `GS_MATCH_SETTINGS` 0x7d body (after the shell id):
/// `[lobby_caption:zstr8][unused:u8][own_team:i8][own_value:u32]...`, the shot counter's new value; `None` when the
/// body ends before it.
// SPEC: GAME_MESSAGES_2009_S2C.md 0x7d row (cshell+0x2c4ff..0x2c724: the third read, 32 bits, := local_player+0x1fc);
// WEAPON_FIRE_2009_addendum_cone.md AC-1.5
pub fn read_match_shot_seed(r: &mut BitReader<'_>) -> Option<u32> {
    r.read_zstr8();
    if r.remaining_bits() < 48 {
        return None;
    }
    r.read_u8();
    r.read_s8();
    Some(r.read_u32())
}

/// `delta` of a `GS_PLAYER_POINTS` 0xda body of kind 1 (`[kind:u8][delta:u32]`); `None` for another kind.
// SPEC: GAME_MESSAGES_2009_S2C.md 0xda row (kind 1: local_player+0x1fc += delta, cshell+0x61521..0x61539)
pub fn read_points_shot_seed(r: &mut BitReader<'_>) -> Option<u32> {
    if r.remaining_bits() < 8 || r.read_u8() != POINTS_SHOT_SEED || r.remaining_bits() < 32 {
        return None;
    }
    Some(r.read_u32())
}

/// `GS_KILL` (0x6d), every field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KillRecord {
    pub victim: u8,
    /// 0xff = no killer (world, fall).
    pub killer: u8,
    pub kill_kind: u8,
    pub weapon: u16,
    /// 1 = head; 5 = the special zone.
    pub hit_zone: u8,
    pub relation: bool,
    pub feed2: bool,
    pub keep_hud: bool,
    pub feed6: bool,
    pub streak_flag: u8,
    pub feed_value: u8,
    /// Kills / deaths are not counted for this event.
    pub no_count: bool,
    pub revenge: Option<u32>,
    pub revenge_done: bool,
    pub coop: (u32, u32),
    /// False when the message ended before the last field.
    pub complete: bool,
}

/// "No killer" value of [`KillRecord::killer`].
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 field 2 (0xff = no killer)
pub const NO_KILLER: u8 = 0xff;

/// Reads `GS_KILL` (0x6d) after the shell id: fields 1-16 in order.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 (cshell+0x2d922..0x2e2ed)
pub fn read_kill(r: &mut BitReader<'_>) -> KillRecord {
    let mut k = KillRecord {
        victim: r.read_u8(),
        killer: r.read_u8(),
        kill_kind: r.read_u8(),
        weapon: r.read_u16(),
        hit_zone: r.read_u8(),
        ..KillRecord::default()
    };
    k.relation = r.read_bit();
    k.feed2 = r.read_bit();
    k.keep_hud = r.read_bit();
    k.feed6 = r.read_bit();
    k.streak_flag = r.read_u8();
    k.feed_value = r.read_u8();
    k.no_count = r.read_bit();
    if r.read_bit() {
        k.revenge = Some(r.read_u32());
    }
    k.revenge_done = r.read_bit();
    let need = 64;
    let enough = r.remaining_bits() >= need;
    k.coop = (r.read_u32(), r.read_u32());
    k.complete = enough;
    k
}

/// `GS_PLAYER_STATUS` (0x66) by its kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlayerStatus {
    /// Kind 1: ammunition of an inventory slot.
    Ammo {
        weapon: u16,
        ammo_kind: u16,
        ammo: f32,
        slot: u8,
        extra: u16,
    },
    /// Kind 0x14: the player's money.
    Money { b1: u8, b2: u8, v: u32, money: u32 },
    /// Every other kind: `[b1][b2][value:f32]` (2 health, 3 armour, 0x0f / 0x10 their maxima, 4 selection, 0x0c the
    /// screen flash of stage A).
    Value {
        kind: u8,
        b1: u8,
        b2: u8,
        value: f32,
    },
}

/// Kinds of [`PlayerStatus::Value`] the client acts on.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (cshell+0x5cb80: 2 health, 3 armour, 0x0f max health, 0x10 max armour)
pub mod status_kind {
    pub const AMMO: u8 = 1;
    pub const HEALTH: u8 = 2;
    pub const ARMOR: u8 = 3;
    pub const SCREEN_FLASH: u8 = 0x0c;
    pub const MAX_HEALTH: u8 = 0x0f;
    pub const MAX_ARMOR: u8 = 0x10;
    pub const MONEY: u8 = 0x14;
}

/// Reads `GS_PLAYER_STATUS` (0x66) after the shell id.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (cshell+0x60dda..0x60f1b), §2.2 (kind 0x0c: `[fade_out:u8][unused:u8]
// [flash_secs:f32]`, the same widths as the generic form)
pub fn read_player_status(r: &mut BitReader<'_>) -> PlayerStatus {
    match r.read_u8() {
        status_kind::AMMO => PlayerStatus::Ammo {
            weapon: r.read_u16(),
            ammo_kind: r.read_u16(),
            ammo: r.read_f32(),
            slot: r.read_u8(),
            extra: r.read_u16(),
        },
        status_kind::MONEY => PlayerStatus::Money {
            b1: r.read_u8(),
            b2: r.read_u8(),
            v: r.read_u32(),
            money: r.read_u32(),
        },
        kind => PlayerStatus::Value {
            kind,
            b1: r.read_u8(),
            b2: r.read_u8(),
            value: r.read_f32(),
        },
    }
}

/// `GS_MATCH_BEGIN` (0x45) head: `round_secs`, `show_intro`, `waiting_banner` (the 2009 client's "waiting for
/// players" banner), `countdown_secs`. The rest depends on client state (intro block) and is not read.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1.1 items 1-4
pub fn read_match_begin_head(r: &mut BitReader<'_>) -> (u32, bool, bool, u32) {
    (r.read_u32(), r.read_bit(), r.read_bit(), r.read_u32())
}

/// `GS_ROUND_BEGIN` (0x44): `round_secs`, `round_no`, `round_limit`, `keep_team`.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (0x44)
pub fn read_round_begin(r: &mut BitReader<'_>) -> (u32, u32, u32, bool) {
    (r.read_u32(), r.read_u32(), r.read_u32(), r.read_bit())
}

/// `GS_ROUND_RESULT` (0x46) head: `round_secs`, `win_team`, the two round-win counts (the co-op block that follows
/// for mode class 5 is not read).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1.2
pub fn read_round_result_head(r: &mut BitReader<'_>) -> (u32, i8, u8, u8) {
    (r.read_u32(), r.read_s8(), r.read_u8(), r.read_u8())
}

/// `GS_MATCH_RESULT_SCREEN` (0x3f): `[result_flag:bool1][result_value:u32]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (0x3f)
pub fn read_result_screen(r: &mut BitReader<'_>) -> (bool, u32) {
    (r.read_bit(), r.read_u32())
}

/// `GS_LEVEL_EXIT` (0x99): `[next_world:zstr8(260)][keep:bool1][now:bool1]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §6.1 (cshell+0x22f950)
pub fn read_level_exit(r: &mut BitReader<'_>) -> (String, bool, bool) {
    let w = String::from_utf8_lossy(&r.read_zstr8()).into_owned();
    (w, r.read_bit(), r.read_bit())
}

/// Two `u8` team values: `GS_TEAM_SCORE` (0x4a) and `GS_ROUND_WINS` (0xb5).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (0x4a), §3.4 (0xb5)
pub fn read_team_pair(r: &mut BitReader<'_>) -> [u8; 2] {
    [r.read_u8(), r.read_u8()]
}

/// One `GS_PLAYER_SUMMARY` (0x65) row (head; the co-op block of mode class 5 is not read).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SummaryRow {
    pub pid: u8,
    pub exp_before: u32,
    pub exp_gain: u32,
    pub exp_bonus: u32,
    pub gp_gain: u32,
    pub gp_bonus: u32,
    pub flag: bool,
}

/// Reads `GS_PLAYER_SUMMARY` (0x65): `[pid:u8][unused:objref16][exp_before:u32][exp_gain:u32][exp_bonus:u32]
/// [gp_gain:u32][gp_bonus:u32][flag:bool1]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.4 (cshell+0x2e2fd..0x2e4c6)
pub fn read_player_summary(r: &mut BitReader<'_>) -> SummaryRow {
    let pid = r.read_u8();
    let _unused = r.read_u16();
    SummaryRow {
        pid,
        exp_before: r.read_u32(),
        exp_gain: r.read_u32(),
        exp_bonus: r.read_u32(),
        gp_gain: r.read_u32(),
        gp_bonus: r.read_u32(),
        flag: r.read_bit(),
    }
}

/// Reads a quaternion (for logs of the stream).
pub fn read_rotation(r: &mut BitReader<'_>) -> [f32; 4] {
    read_quat128(r)
}

/// Writes a quaternion (tests build server messages with it).
pub fn write_rotation(w: &mut BitWriter, q: [f32; 4]) {
    write_quat128(w, q)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_writers_match_the_slice_byte_strings() {
        // SPEC: UDP_SESSION_2009_A.md §9.2 (S1 `05 02 01`, S6a `09 00`, S8a `09 01`); A18 (`07 80 02`)
        let h = hello("Bot1", "bot1", None);
        assert_eq!(&h.as_bytes()[..3], &[0x05, 0x02, 0x01]);
        assert_eq!(h.byte_len(), 3 + 258);
        let b = &h.as_bytes()[3..];
        assert_eq!(&b[..5], b"Bot1\0");
        assert_eq!(&b[0x40..0x45], b"bot1\0");
        assert_eq!(&b[0x82..0x8a], &[b'B', 0, b'o', 0, b't', 0, b'1', 0]);
        assert_eq!(b[0x8a], 0);
        assert_eq!(load_state(0).as_bytes(), [0x09, 0x00]);
        assert_eq!(load_state(1).as_bytes(), [0x09, 0x01]);
        assert_eq!(rate(5_120_000).as_bytes(), [0x07, 0x80, 0x02]);
        assert_eq!(rate(35_840).as_bytes(), [0x07, 0x04, 0x00]);
        assert_eq!(file_ack(&[1, 2]).as_bytes(), [0x36, 0x01, 0x80, 0x02, 0x80]);
        assert_eq!(bye().as_bytes(), [0x06]);
        assert_eq!(shell_msg(0x87, &BitWriter::new()).as_bytes(), [0x0B, 0x87]);
    }

    /// A 0x6b body after the shell id with the given dead bit (field 13), team and kind (fields 12, 14).
    fn roster_record_body(dead: bool, team: u8, kind: u8) -> BitWriter {
        let mut w = BitWriter::new();
        w.write_zstr8(b"Ab");
        w.write_u8(7).write_u32(1002);
        w.write_bit(false); // flag_a
        w.write_u16(408);
        for _ in 0..15 {
            w.write_u8(0);
        }
        w.write_zstr8(b"").write_zstr8(b"");
        w.write_s16(3).write_u8(2);
        w.write_u8(0).write_u8(0).write_u8(0);
        w.write_u8(team);
        w.write_bit(dead);
        w.write_u8(kind);
        w.write_bit(false); // has_group
        w.write_u32(0).write_u32(0);
        w.write_bit(false).write_bit(false).write_bit(false);
        w.write_u8(0);
        w.write_u8(0).write_u32(0).write_u32(0).write_u16(0);
        w
    }

    #[test]
    fn player_info_carries_the_dead_flag_of_field_13() {
        // SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 (field 12 `team`, field 13 `flag_b` = record +0x74, field 14 `kind`)
        for dead in [false, true] {
            let w = roster_record_body(dead, 2, 2);
            let p = read_player_info(&mut w.reader());
            assert_eq!(
                (p.pid, p.team, p.kind, p.dead, p.complete),
                (7, 2, 2, dead, true)
            );
            assert_eq!((p.kills, p.deaths, p.account_no), (3, 2, 1002));
        }
    }

    #[test]
    fn non_latin_name_keeps_utf16() {
        let b = hello_blob("Ä€x", "");
        assert_eq!(&b[..4], &[0xC4, b'?', b'x', 0]);
        assert_eq!(&b[0x82..0x88], &[0xC4, 0x00, 0xAC, 0x20, b'x', 0]);
    }

    #[test]
    fn player_update_layout_is_352_plus_312_bits() {
        let mut u = MoveReport {
            angles: [0.1, 1.0, 0.0],
            aux: [0; 6],
            aim: None,
            control: 0x201,
            trailer: MoveTrailer {
                move_seq: 3,
                pos: [1.0, 2.0, 3.0],
                vel: [0.0; 3],
                time: 4.5,
                on_ground: 1,
                surface: 0,
                standing: u64::MAX,
            },
        };
        let b = player_update_body(&u);
        // u16 + u8 + 96 + 192 + u32 = 344 bits, then the 312-bit trailer
        assert_eq!(b.bit_len(), 344 + 312);
        let mut r = b.reader();
        assert_eq!(r.read_u16(), 0x0020);
        assert_eq!(r.read_u8(), 40); // round(1.0 * 40.425354)
        assert_eq!(r.read_f32(), 0.1);
        r.skip_bits(64 + 192);
        assert_eq!(r.read_u32(), 0x201);
        assert_eq!(r.read_u8(), 3);
        assert_eq!(vec96(&mut r), [1.0, 2.0, 3.0]);
        r.skip_bits(96);
        assert_eq!(r.read_f32(), 4.5);
        assert_eq!(r.read_u8(), 1);
        assert_eq!(r.read_u8(), 0);
        assert_eq!(r.read_u64(), u64::MAX);
        assert!(r.at_end());
        assert_eq!(player_state_form_body(1).as_bytes(), [0x10, 0x00, 0x01]);
        // with the aim rotation: flag 0x0100 and 128 more bits right after the aux words
        u.aim = Some([0.0, 0.5, 0.0, 0.75]);
        let b = player_update_body(&u);
        assert_eq!(b.bit_len(), 344 + 128 + 312);
        let mut r = b.reader();
        assert_eq!(r.read_u16(), 0x0120);
        r.skip_bits(8 + 96 + 192);
        assert_eq!(read_quat128(&mut r), [0.0, 0.5, 0.0, 0.75]);
        assert_eq!(r.read_u32(), 0x201);
    }

    #[test]
    fn update_with_extras_has_the_live_field_order() {
        let u = MoveReport {
            angles: [0.0, 0.5, 0.0],
            aux: [0.25f32.to_bits(), 0, 0, 0, 0, 0],
            aim: Some([0.0, 0.5, 0.0, 0.75]),
            control: 0x201,
            trailer: MoveTrailer {
                move_seq: 3,
                pos: [1.0, 2.0, 3.0],
                vel: [0.0; 3],
                time: 4.0,
                on_ground: 1,
                surface: 0,
                standing: u64::MAX,
            },
        };
        // no extras: the bot's body, bit for bit
        let plain = player_update_body(&u);
        assert_eq!(
            player_update_body_ext(&u, &UpdateExtras::default()).as_bytes(),
            plain.as_bytes()
        );
        let x = UpdateExtras {
            eye_offset: Some([0.0, 33.0, 0.0]),
            weapon_f: None,
            cam_point: Some([1.0, 35.0, 3.0]),
            flag_bits: pu::FLAG_1000 | 0x0001,
        };
        let b = player_update_body_ext(&u, &x);
        assert_eq!(b.bit_len(), plain.bit_len() + 96 + 96);
        let mut r = b.reader();
        // 0x0001 | 0x0020 | 0x0100 | 0x0400 | 0x1000 (a field bit in flag_bits is ignored)
        assert_eq!(r.read_u16(), 0x1521);
        r.skip_bits(8 + 96);
        assert_eq!(r.read_u32(), 0.25f32.to_bits());
        r.skip_bits(5 * 32);
        assert_eq!([r.read_f32(), r.read_f32(), r.read_f32()], [0.0, 33.0, 0.0]);
        assert_eq!(read_quat128(&mut r), [0.0, 0.5, 0.0, 0.75]);
        assert_eq!([r.read_f32(), r.read_f32(), r.read_f32()], [1.0, 35.0, 3.0]);
        assert_eq!(r.read_u32(), 0x201);
        assert_eq!(r.read_u8(), 3);
    }

    #[test]
    fn yaw8c_rounds_and_wraps() {
        assert_eq!(yaw8c(0.0), 0);
        assert_eq!(yaw8c(std::f32::consts::PI) as i8, 127);
        assert_eq!(yaw8c(-0.5) as i8, -20);
        assert_eq!(yaw8c(std::f32::consts::TAU + 0.5), yaw8c(0.5));
    }

    #[test]
    fn player_info_round_trip_of_the_head() {
        let p = PlayerInfo {
            room_slot: 1,
            char_no: 7,
            session_idx: 3,
            exp: 100,
            team: 2,
            gp: 10_000,
            loadout: vec![(1001, 0, 0, 0)],
            nation: 4,
            ..PlayerInfo::default()
        };
        let b = player_info_body(&p);
        let mut r = b.reader();
        assert_eq!(r.read_u32(), 1);
        assert_eq!(r.read_u32(), 7);
        assert_eq!(r.read_u16(), 3);
        assert_eq!(r.read_u32(), 100);
        // DC-DEFAULT: BOT-29 (proposed) the default look, then two empty zstr8
        let look: Vec<i8> = (0..15).map(|_| r.read_s8()).collect();
        assert_eq!(look, BOT_LOOK);
        assert_eq!(look[14], -1);
        r.skip_bits(16);
        r.skip_bits(32 + 32); // r1..r4, r5
        assert_eq!(r.read_s8(), 2);
        r.skip_bits(8 + 32);
        assert_eq!(r.read_u32(), 10_000);
        assert_eq!(r.read_u8(), 1);
        assert_eq!(r.read_u16(), 1001);
        r.skip_bits(24);
        assert!(!r.read_bit());
        r.skip_bits(32);
        assert_eq!(r.read_u32(), 4);
        r.skip_bits(2);
        assert!(r.at_end());
    }

    #[test]
    fn mover_update_teleport_block() {
        let mut w = BitWriter::new();
        w.write_u16(0x0020 | 0x0008).write_u8(9);
        for v in [1.0f32, 2.0, 3.0, 16.0, 40.0, 16.0, 0.5] {
            w.write_f32(v);
        }
        let m = read_mover_update(&mut w.reader());
        assert_eq!(m.teleport, Some((9, [1.0, 2.0, 3.0], [16.0, 40.0, 16.0])));
        assert_eq!(m.friction, Some(0.5));
        assert!(m.complete);
    }

    #[test]
    fn kill_status_and_match_end_readers() {
        // GS_KILL fields 1-16 (§3.3.1) with the revenge block present
        let mut w = BitWriter::new();
        w.write_u8(3)
            .write_u8(5)
            .write_u8(2)
            .write_u16(41)
            .write_u8(1);
        w.write_bit(true)
            .write_bit(false)
            .write_bit(true)
            .write_bit(false);
        w.write_u8(0)
            .write_u8(7)
            .write_bit(false)
            .write_bit(true)
            .write_u32(99);
        w.write_bit(false).write_u32(1).write_u32(2);
        let k = read_kill(&mut w.reader());
        assert_eq!(
            (k.victim, k.killer, k.kill_kind, k.weapon, k.hit_zone),
            (3, 5, 2, 41, 1)
        );
        assert!(k.relation && k.keep_hud && !k.feed2 && !k.no_count);
        assert_eq!((k.feed_value, k.revenge, k.coop), (7, Some(99), (1, 2)));
        assert!(k.complete);
        // a kill without its two co-op words is reported incomplete
        let mut w = BitWriter::new();
        w.write_u8(3)
            .write_u8(NO_KILLER)
            .write_u8(0)
            .write_u16(0)
            .write_u8(0);
        w.write_bits(0, 4).write_u8(0).write_u8(0).write_bits(0, 3);
        assert!(!read_kill(&mut w.reader()).complete);
        // GS_PLAYER_STATUS: generic, ammunition and money forms (§4.3)
        let mut w = BitWriter::new();
        w.write_u8(status_kind::HEALTH)
            .write_u8(0)
            .write_u8(0)
            .write_f32(75.0);
        assert_eq!(
            read_player_status(&mut w.reader()),
            PlayerStatus::Value {
                kind: 2,
                b1: 0,
                b2: 0,
                value: 75.0
            }
        );
        let mut w = BitWriter::new();
        w.write_u8(status_kind::AMMO)
            .write_u16(9)
            .write_u16(4)
            .write_f32(60.0)
            .write_u8(1)
            .write_u16(0);
        assert!(matches!(
            read_player_status(&mut w.reader()),
            PlayerStatus::Ammo {
                weapon: 9,
                ammo_kind: 4,
                slot: 1,
                ..
            }
        ));
        let mut w = BitWriter::new();
        w.write_u8(status_kind::MONEY)
            .write_u8(0)
            .write_u8(0)
            .write_u32(0)
            .write_u32(1234);
        assert!(matches!(
            read_player_status(&mut w.reader()),
            PlayerStatus::Money { money: 1234, .. }
        ));
        // result screen, level exit, team pair, summary
        let mut w = BitWriter::new();
        w.write_bit(true).write_u32(6);
        assert_eq!(read_result_screen(&mut w.reader()), (true, 6));
        let mut w = BitWriter::new();
        w.write_zstr8(b"").write_bit(false).write_bit(true);
        assert_eq!(
            read_level_exit(&mut w.reader()),
            (String::new(), false, true)
        );
        let mut w = BitWriter::new();
        w.write_u8(4).write_u8(2);
        assert_eq!(read_team_pair(&mut w.reader()), [4, 2]);
        let mut w = BitWriter::new();
        w.write_u8(1).write_u16(0xffff);
        for v in [10u32, 20, 0, 30, 0] {
            w.write_u32(v);
        }
        w.write_bit(false);
        let s = read_player_summary(&mut w.reader());
        assert_eq!(
            (s.pid, s.exp_before, s.exp_gain, s.gp_gain),
            (1, 10, 20, 30)
        );
        let mut w = BitWriter::new();
        w.write_u32(0).write_s8(-1).write_u8(1).write_u8(0);
        assert_eq!(read_round_result_head(&mut w.reader()), (0, -1, 1, 0));
        let mut w = BitWriter::new();
        w.write_u32(300)
            .write_bit(false)
            .write_bit(true)
            .write_u32(0)
            .write_u32(0);
        assert_eq!(
            read_match_begin_head(&mut w.reader()),
            (300, false, true, 0)
        );
    }

    #[test]
    fn integrity_and_endpoint_readers() {
        let mut w = BitWriter::new();
        w.write_u8(0).write_u16(0x100);
        assert_eq!(
            read_integrity(&mut w.reader()),
            IntegrityAsk::Version(0x100)
        );
        let mut w = BitWriter::new();
        w.write_u8(1).write_zstr8(b"127.0.0.1").write_u32(27888);
        assert_eq!(
            read_game_endpoint(&mut w.reader()),
            (1, Some(("127.0.0.1".to_string(), 27888)))
        );
        assert_eq!(
            integrity_reply_body(0x1234, 0).as_bytes(),
            [0x00, 0x34, 0x12, 0, 0, 0, 0]
        );
    }
}
