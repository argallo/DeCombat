//! Server -> client game messages (shell ids inside `S2C_SHELL_MSG`, engine id 13) that slice 2 sends: the spawn
//! set, the roster, combat, the TDM round flow, the ping table and chat. Every writer returns the whole message
//! `[13][shell_id][fields]`; fields are written in the bit order of the client's readers. Names are ours
//! (`GS_` prefix, `GAME_MESSAGES_2009_S2C.md` §12).
//!
//! Every message goes on the reliable channel in order.
// DC-DEFAULT: PD-S2C-1 (proposed) every server -> client game message is reliable and ordered

use crate::msg::s2c_shell_msg;
use dc_wire::udp::BitWriter;

// ------------------------------------------------------------------------------------------------- ids

/// `GS_ROUND_RESULT`: `[round_secs:u32][win_team:i8][wins_team1:u8][wins_team2:u8]` (+ co-op block).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1.2 (cshell+0x4a608)
pub const GS_ROUND_RESULT: u8 = 0x46;
/// `GS_MATCH_BEGIN`: round time, intro flags, countdown, optional intro block, round number.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1.1 (cshell+0x49de1)
pub const GS_MATCH_BEGIN: u8 = 0x45;
/// `GS_ROUND_TIMER_STOP`: `[round_secs:u32]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (cshell+0x4ac72)
pub const GS_ROUND_TIMER_STOP: u8 = 0x41;
/// `GS_MATCH_RESULT_SCREEN`: `[result_flag:bool1][result_value:u32]`; opens the result screen (screen 7).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (cshell+0x4ab58)
pub const GS_MATCH_RESULT_SCREEN: u8 = 0x3f;
/// `GS_TEAM_SCORE`: `[score_team1:u8][score_team2:u8]`. Never sent since slice 4: the reference stack carried the team kills
/// in 0xb5 only and the 2009 client counts its HUD totals from 0x6d itself.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (cshell+0x4ad9d); CAPTURE_tdm4_KILL_VALIDATION.md §3.2 item 2 (never 0x4a)
pub const GS_TEAM_SCORE: u8 = 0x4a;
/// `GS_ROUND_OBJECTS_END` `[unused:u32]` (objective map layers cleared) and `GS_ROUND_CLOSE` `[round_secs:u32]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1 rows 0x47 (cshell+0x4aa47), 0x48 (cshell+0x4aac1)
pub const GS_ROUND_OBJECTS_END: u8 = 0x47;
pub const GS_ROUND_CLOSE: u8 = 0x48;
/// Shell 0x9d `[u32]`: sent by the reference stack right after 0x9e (`0d 9d 00 00 00 00`); no 2009 handler is catalogued.
// SPEC: GAME_MESSAGES_2009_S2C.md §12 O2 note ("shell 0x9d has no row"); CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §2.2
// (the real client accepted it); SPEC: UNPROVEN meaning
pub const GS_SHELL_9D: u8 = 0x9d;
/// `GS_CHAT`: `[sender_name:zstr16(32)][text:zstr16(376)][sender_id:u32][team_only:u8][whisper:u32]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.4 (cshell+0x4b30e)
pub const GS_CHAT: u8 = 0x68;
/// `GS_PLAYER_SUMMARY`: per-player experience / GP summary for the result screen.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.4 (cshell+0x2e2fd)
pub const GS_PLAYER_SUMMARY: u8 = 0x65;
/// `GS_PLAYER_INFO`: one roster record (layout §3.2.1).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 (cshell+0x2c729)
pub const GS_PLAYER_INFO: u8 = 0x6b;
/// `GS_PLAYER_LEFT`: `[pid:u8][kicked:u8]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2 (cshell+0x2d4d5)
pub const GS_PLAYER_LEFT: u8 = 0x6c;
/// `GS_KILL`: kill feed / counters (layout §3.3.1).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 (cshell+0x2d922)
pub const GS_KILL: u8 = 0x6d;
/// `GS_MATCH_SETTINGS`: caption, own team, goal, team scores, round limit / number.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2 (cshell+0x2c4ff)
pub const GS_MATCH_SETTINGS: u8 = 0x7d;
/// `GS_PING_TABLE`: `{[pid:u16][ping:u16]}` ended by `pid = 0xFFFF`.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2 (cshell+0x2d73a)
pub const GS_PING_TABLE: u8 = 0xa2;
/// `GS_ROUND_WINS`: `[wins_team1:u8][wins_team2:u8]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.4 (cshell+0x2d894)
pub const GS_ROUND_WINS: u8 = 0xb5;
/// `GS_OBJECTIVE_STATE` (0xc6): a flag's state change in the flag modes
/// `[obj:objref16][state:u32][flag_team:u32][carrier:u32][actor:u32][event:u32][at_base:bool1][quiet:bool1]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §6.1 row 0xc6; GAME_MODES_2009.md §5.5 (C-GM-4; DISASM cshell+0x5aae0..0x5ac2f)
pub const GS_OBJECTIVE_STATE: u8 = 0xc6;
/// `GS_BOMB_EXPLODED` (0x4f, no body), `GS_BOMB_DEFUSED` (0x50, no body), `GS_BOMB_PLANTED` (0x51,
/// `[bomb_secs:u32]`): the bomb timer shown / hidden and the layout's notices (classes 2 / 3).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.2 rows 0x4f, 0x50, 0x51 (cshell+0x4adfa, 0x4aeab, 0x4afee); GAME_MODES_2009.md
// §5.2 step 3 / 4
pub const GS_BOMB_EXPLODED: u8 = 0x4f;
pub const GS_BOMB_DEFUSED: u8 = 0x50;
pub const GS_BOMB_PLANTED: u8 = 0x51;
/// `GS_OBJECTIVE_ITEM` (0x56): `[item:objref16]`, the radar marks the object as the own team's objective item.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.2 row 0x56 (cshell+0x4b2ca)
pub const GS_OBJECTIVE_ITEM: u8 = 0x56;
/// `GS_TRACK_OBJECT` (0xad): `[obj:objref16][kind:u8][a:u32][hold_secs:f32][c:u32]`, starts the client's
/// use-progress countdown (hold to plant / defuse).
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4 row 0xad; GAME_MODES_2009.md §4.5 (C-GM-5, DISASM cshell+0x275a40..0x275ac7)
pub const GS_TRACK_OBJECT: u8 = 0xad;
/// `GS_INFECTION_MODE` (0xd3): `[sub:u8]` then sub 0 `[team:u8]` (`[infector:u32]` when team 2) `[pid:u32]` + look;
/// sub 1 `[a:u8][b:u8][flag:bool1][secs:f32]`; subs 2 / 4 nothing; sub 3 `[e:u8]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.5 row 0xd3; GAME_MODES_2009.md §5.7 (C-GM-2, C-GM-3; cshell+0x2a1e0)
pub const GS_INFECTION_MODE: u8 = 0xd3;
/// `GS_BOUNTY_MODE` (class 9, Spy Hunt).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.5 row 0xcb; GAME_MODES_2009.md §5.6
pub const GS_BOUNTY_MODE: u8 = 0xcb;
/// `GS_PERF_STATS`: `[sub:u8][pid:u32]` + sub fields (statistics reports, echo, relay).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.6 (cshell+0x2acd0); GAME_MESSAGES_2009_C2S.md §5.8
pub const GS_PERF_STATS: u8 = 0xc3;
/// `GS_MULTI_KILL` (0xc8): `[kills:u16][head:bool1][combo:u32]`, a banner the server orders directly: `combo` > 1
/// the "N COMBO" banner, else the one-kill banner with `kills` (the head-shot banner when `head`). Not the
/// multi-kill banner of TDM, which the 0x6d `kill_kind` level selects; sent only per AI kill in co-op (DSV-602), never in
/// the kill modes (DSV-105).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3 (cshell+0x2d8ca)
// DISASM: cshell+0x2d8ca..0x2d918 (the reads and the two calls), cshell+0xa52a0, cshell+0xa57b0
pub const GS_MULTI_KILL: u8 = 0xc8;
/// `GS_PLAYER_STATUS`: `[kind:u8]` then by kind (health, armour, maxima, ammunition).
// SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (cshell+0x60dda, cshell+0x5cb80)
pub const GS_PLAYER_STATUS: u8 = 0x66;
/// `GS_PLAYER_STATE`: `[state:u8]`, the player-state carrier of the in-world test (S12).
// SPEC: GAME_MESSAGES_2009_S2C.md §4.1 (cshell+0x1f0470 -> cshell+0x61870 -> cshell+0x1f97d0);
// UDP_SESSION_2009_A_addendum_disasm.md A16, A17
pub const GS_PLAYER_STATE: u8 = 0x67;
/// `GS_VIEW_ANGLES`: `[kind:u8][angles:vec96]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.2 (cshell+0x5cca0 -> cshell+0x1f4ed0); MOVEMENT_2009.md §12.5
pub const GS_VIEW_ANGLES: u8 = 0x6a;
/// `GS_DAMAGE_TAKEN`: direction, ammunition, zone, amount (health itself is set by 0x66 kind 2).
// SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (cshell+0x5cd20 -> cshell+0x1f7d20)
pub const GS_DAMAGE_TAKEN: u8 = 0x74;
/// `GS_MOVER_UPDATE`: `[flags:u16]` then flag blocks (teleport and box 0x0020, body 0x0001, ...).
// SPEC: GAME_MESSAGES_2009_S2C.md §4.2 (cshell+0x1f27f0 -> cshell+0x1ff430); MOVEMENT_2009.md §12.4
pub const GS_MOVER_UPDATE: u8 = 0xa0;
/// `GS_WEAPON_GIVE`: a loadout weapon.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (cshell+0x60f20)
pub const GS_WEAPON_GIVE: u8 = 0xb2;
/// `GS_WEAPON_EQUIP`: the drawn weapon (with the clip).
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (cshell+0x61193)
pub const GS_WEAPON_EQUIP: u8 = 0xb3;
/// `GS_IDENTIFY_REQUEST`: no body; the client answers with its `UP_PLAYER_INFO` (C2S 0x72).
// SPEC: GAME_MESSAGES_2009_S2C.md §5.1 (cshell+0x23c5a5, slot +0x3c = cshell+0x23c620)
pub const GS_IDENTIFY_REQUEST: u8 = 0x72;
/// `GS_LEVEL_EXIT`: `[next_world:zstr8(260)][keep:bool1][now:bool1]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §6.1 (cshell+0x22f950)
pub const GS_LEVEL_EXIT: u8 = 0x99;
/// `GS_WEAPON_SWITCH`: `[command:u8][auto:u8][ammo:f32][last:u8]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (cshell+0x1f3400)
pub const GS_WEAPON_SWITCH: u8 = 0x7f;
/// `GS_SCORES_CLEAR` (no body).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (cshell+0x49b2a)
pub const GS_SCORES_CLEAR: u8 = 0x43;
/// `GS_ROUND_BEGIN`: `[round_secs:u32][round_no:u32][round_limit:u32][keep_team:bool1]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (cshell+0x49b81..0x49dc4)
pub const GS_ROUND_BEGIN: u8 = 0x44;
/// `GS_GAME_ENDPOINT`: `[mode_class:u8]` then, when not 0, `[host:zstr8(256)][port:u32]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §5.1 (slot +0x38 = cshell+0x2f860)
pub const GS_GAME_ENDPOINT: u8 = 0x9e;
/// `GS_SPRINT_ALLOWED` (formerly `GS_OWN_FLAG`): `[allowed:bool1]`, the own record byte the client's sprint test
/// requires.
// SPEC: CAPTURE_tdm1_GAME_VALIDATION.md §3.11 (PROVEN-CAPTURE by the client's reaction); MOVEMENT_2009.md §7
pub const GS_SPRINT_ALLOWED: u8 = 0xca;
/// `GS_OBJECT_NOTE`: `[note_kind:u8][obj:objref16]` then the class data of the client object.
// SPEC: GAME_MESSAGES_2009_S2C.md §7.2, §8.3 (cshell+0x20fd00)
pub const GS_OBJECT_NOTE: u8 = 0xa6;
/// Sub-id of a type-0x18 note that re-reads the whole character record.
// SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §1.7 (cshell+0x206260 sub 0x11; DISASM jump table)
pub const NOTE_RECORD: u8 = 0x11;
/// `GS_SHELL_FLAG_90`: `[flag:bool1]` (meaning UNPROVEN).
// SPEC: GAME_MESSAGES_2009_S2C.md §7.2 (cshell+0x1a58d0)
pub const GS_SHELL_FLAG_90: u8 = 0x90;

// ------------------------------------------------------------------------------------------------- field values

/// `GS_PLAYER_STATE` value: alive / spawned (movement on; the client sends `UP_PLAYER_UPDATE` only in it).
// SPEC: GAME_MESSAGES_2009_S2C.md §4.1 state table; DC-DEFAULT: PD-S2C-2' (proposed)
pub const STATE_ALIVE: u8 = 1;
/// `GS_PLAYER_STATE` value: dying (death camera).
// SPEC: GAME_MESSAGES_2009_S2C.md §4.1; DC-DEFAULT: PD-S2C-2' (proposed)
pub const STATE_DYING: u8 = 2;
/// `GS_PLAYER_STATE` value: waiting for a respawn (movement frozen; also the loading-screen state before 0x87).
// SPEC: GAME_MESSAGES_2009_S2C.md §4.1; DC-DEFAULT: PD-S2C-2' (proposed), GAME_MESSAGES_2009_C2S.md §12.3 DC-C2S-10' (proposed)
pub const STATE_WAITING: u8 = 3;
/// `GS_PLAYER_STATE` value: spectator.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.1; DC-DEFAULT: PD-S2C-2' (proposed)
pub const STATE_SPECTATOR: u8 = 6;

/// `GS_PLAYER_STATUS` kinds handled by cshell+0x5cb80 (`[kind][b1][b2][value:f32]`).
// SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (2 health, 3 armour, 0x0f maximum health, 0x10 maximum armour)
pub const STATUS_HEALTH: u8 = 2;
pub const STATUS_ARMOR: u8 = 3;
pub const STATUS_MAX_HEALTH: u8 = 0x0f;
pub const STATUS_MAX_ARMOR: u8 = 0x10;
/// `GS_PLAYER_STATUS` kind 1: ammunition of a weapon slot (`[weapon:u16][ammo_kind:u16][ammo:f32][slot:u8][extra:u16]`).
// SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (kind 1; status slot +0x38 records the weapon in `slot` and sets its ammo count)
pub const STATUS_AMMO: u8 = 1;
/// The `extra` of the grenade-count message the reference stack sent after a throw.
// SPEC: CAPTURE_tdm4_KILL_VALIDATION.md §2.6 (`[weapon 7][ammo kind 9][ammo 0.0][slot 3][1]`; REFERENCE); S2C §4.3.1
// (`extra` 1; its use is UNPROVEN)
pub const AMMO_EXTRA_THROW: u16 = 1;

/// `GS_MOVER_UPDATE` flag: teleport and new box `[seq:u8][pos:3 x f32][dims:3 x f32]`.
// SPEC: MOVEMENT_2009.md §12.4 (cshell+0x1feb90)
pub const MOVER_TELEPORT: u16 = 0x0020;
/// `GS_MOVER_UPDATE` flag: (re)create the body `[model:str][n:u8][n x skin:str][passable:1 bit]`.
// SPEC: MOVEMENT_2009.md §12.4
pub const MOVER_BODY: u16 = 0x0001;
/// `GS_MOVER_UPDATE` flag: container list `[n:u8]` + entries, then `[friction:f32]`.
// SPEC: MOVEMENT_2009.md §12.4 (0x0004)
pub const MOVER_CONTAINERS: u16 = 0x0004;
/// `GS_MOVER_UPDATE` flag: `[friction:f32]`.
// SPEC: MOVEMENT_2009.md §12.4 (0x0008)
pub const MOVER_FRICTION: u16 = 0x0008;
/// `GS_MOVER_UPDATE` flag: `[freeze:1 bit]`.
// SPEC: MOVEMENT_2009.md §12.4 (0x0080)
pub const MOVER_FREEZE: u16 = 0x0080;
/// The player's friction coefficient with no container: on a remote server the client takes `k` only from the
/// server's mover update, so the server sends the value the client computes locally with no container.
// SPEC: MOVEMENT_2009.md §8.4 (cshell+0x1fce10: `k = 5.0 * min(1, container factors)`; remote: from 0xa0 0x04 / 0x08
// only); UDP_REFERENCE_FINDINGS_botclient.md reference run (the reference stack sent 5.0 in both blocks; REFERENCE)
pub const FRICTION_NO_CONTAINER: f32 = 5.0;
/// `GS_PLAYER_STATUS` kind 0x13: status reset (`client_shell`+0xc3b0 := 5).
// SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (cshell+0x5cb80 kind 0x13 -> cshell+0x63160)
pub const STATUS_RESET: u8 = 0x13;

/// `GS_VIEW_ANGLES` kind 0: pitch, yaw and roll are set.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.2; MOVEMENT_2009.md §12.5
pub const ANGLES_ALL: u8 = 0;

/// Ends the ping table.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2 (`pid = 0xFFFF` ends the list)
pub const PING_TABLE_END: u16 = 0xFFFF;
/// `killer` value of `GS_KILL` for a death without a killer (world, fall).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 field 2
pub const NO_KILLER: u8 = 0xff;
/// `hit_zone` value of `GS_KILL` for a head shot.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 field 5 (1 = head)
pub const KILL_ZONE_HEAD: u8 = 1;
/// `GS_CHAT` sender id of a system notice.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.4 (`sender_id` = 0xFFFFFFFF)
pub const CHAT_SYSTEM: u32 = 0xFFFF_FFFF;

/// Mode classes (the client's `mode_class`) with their own message layouts.
// SPEC: GAME_MESSAGES_2009_S2C.md §1.1, §2.1.1, §2.1.2, §3.2, §3.4 (2, 3, 5, 6, 7, 8, 9, 11 change layouts)
pub const MODE_COOP: u8 = 5;
pub const MODE_CLASS_6: u8 = 6;
pub const MODE_CLASS_7: u8 = 7;
pub const MODE_CLASS_8: u8 = 8;
pub const MODE_CLASS_9: u8 = 9;
pub const MODE_CLASS_11: u8 = 11;

// ------------------------------------------------------------------------------------------------- helpers

fn shell(id: u8, f: impl FnOnce(&mut BitWriter)) -> BitWriter {
    let mut b = BitWriter::new();
    f(&mut b);
    s2c_shell_msg(id, &b)
}

/// `zstr8` for a buffer of `n` bytes: the client keeps at most `n - 1` bytes, so longer text is cut here.
// SPEC: GAME_MESSAGES_2009_S2C.md §1.1 (`zstr8(N)`)
fn zstr8n(w: &mut BitWriter, s: &[u8], n: usize) {
    let cut: Vec<u8> = s
        .iter()
        .copied()
        .take_while(|&c| c != 0)
        .take(n.saturating_sub(1))
        .collect();
    w.write_zstr8(&cut);
}

/// `zstr16` for a buffer of `n` units (at most `n - 1` kept).
// SPEC: GAME_MESSAGES_2009_S2C.md §1.1 (`zstr16(N)`); UDP_TRANSPORT_2009_addendum_disasm.md T3 (writer)
fn zstr16n(w: &mut BitWriter, s: &[u16], n: usize) {
    let cut: Vec<u16> = s
        .iter()
        .copied()
        .take_while(|&c| c != 0)
        .take(n.saturating_sub(1))
        .collect();
    w.write_zstr16_presumed(&cut);
}

fn vec96(w: &mut BitWriter, v: [f32; 3]) {
    for c in v {
        w.write_f32(c);
    }
}

fn clamp_u8(v: u32) -> u8 {
    v.min(255) as u8
}

fn clamp_i8(v: i64) -> i8 {
    v.clamp(-128, 127) as i8
}

// ------------------------------------------------------------------------------------------------- writers

/// `GS_PLAYER_STATE` `[state:u8]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.1; UDP_SESSION_2009_A_addendum_disasm.md A17 (`0D 67 01`)
pub fn player_state(state: u8) -> BitWriter {
    shell(GS_PLAYER_STATE, |w| {
        w.write_u8(state);
    })
}

/// `GS_IDENTIFY_REQUEST` (no body).
// SPEC: GAME_MESSAGES_2009_S2C.md §5.1
pub fn identify_request() -> BitWriter {
    shell(GS_IDENTIFY_REQUEST, |_| {})
}

/// `GS_MOVER_UPDATE` with any set of blocks, written in the reader's order (0x0020, 0x0004, 0x0001, 0x0008,
/// 0x0080). The container list is always empty here.
// SPEC: MOVEMENT_2009.md §12.4 (block order and layouts)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MoverUpdate<'a> {
    pub teleport: Option<(u8, [f32; 3], [f32; 3])>,
    /// 0x0004 with no container and this friction.
    pub containers_friction: Option<f32>,
    pub body: Option<&'a BodySpec>,
    pub friction: Option<f32>,
    pub freeze: Option<bool>,
}

pub fn mover_update(m: &MoverUpdate<'_>) -> BitWriter {
    let mut flags = 0u16;
    if m.teleport.is_some() {
        flags |= MOVER_TELEPORT;
    }
    if m.containers_friction.is_some() {
        flags |= MOVER_CONTAINERS;
    }
    if m.body.is_some() {
        flags |= MOVER_BODY;
    }
    if m.friction.is_some() {
        flags |= MOVER_FRICTION;
    }
    if m.freeze.is_some() {
        flags |= MOVER_FREEZE;
    }
    shell(GS_MOVER_UPDATE, |w| {
        w.write_u16(flags);
        if let Some((seq, pos, dims)) = m.teleport {
            w.write_u8(seq);
            vec96(w, pos);
            vec96(w, dims);
        }
        if let Some(f) = m.containers_friction {
            w.write_u8(0).write_f32(f);
        }
        if let Some(b) = m.body {
            write_body(w, b);
        }
        if let Some(f) = m.friction {
            w.write_f32(f);
        }
        if let Some(z) = m.freeze {
            w.write_bit(z);
        }
    })
}

fn write_body(w: &mut BitWriter, b: &BodySpec) {
    w.write_zstr8(&b.model);
    let skins = &b.skins[..b.skins.len().min(255)];
    w.write_u8(skins.len() as u8);
    for s in skins {
        w.write_zstr8(s);
    }
    w.write_bit(b.passable);
}

/// `GS_WEAPON_SWITCH` `[command:u8][auto:u8][ammo:f32][last:u8]`: the client looks the weapon up as command table
/// [`command` - 0x14] and, with `auto` set (and its auto-switch option on, default on), switches to it; `ammo` < 0
/// = the weapon row's default.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (0x7f); UDP_REFERENCE_FINDINGS_botclient.md reference run (auto switch after
// the 0xb2 gives, ammo -1.0; REFERENCE)
pub fn weapon_switch(command: u8, auto: bool, ammo: f32, last: u8) -> BitWriter {
    shell(GS_WEAPON_SWITCH, |w| {
        w.write_u8(command)
            .write_u8(auto as u8)
            .write_f32(ammo)
            .write_u8(last);
    })
}

/// `GS_SCORES_CLEAR` (no body).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1
pub fn scores_clear() -> BitWriter {
    shell(GS_SCORES_CLEAR, |_| {})
}

/// `GS_ROUND_BEGIN` `[round_secs:u32][round_no:u32][round_limit:u32][keep_team:bool1]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1
pub fn round_begin(round_secs: u32, round_no: u32, round_limit: u32, keep_team: bool) -> BitWriter {
    shell(GS_ROUND_BEGIN, |w| {
        w.write_u32(round_secs)
            .write_u32(round_no)
            .write_u32(round_limit)
            .write_bit(keep_team);
    })
}

/// `GS_GAME_ENDPOINT` `[mode_class:u8]`, then `[host:zstr8(256)][port:u32]` when `mode_class` is not 0.
// SPEC: GAME_MESSAGES_2009_S2C.md §5.1 (0x9e), §10 step 1
pub fn game_endpoint(mode_class: u8, host: &[u8], port: u32) -> BitWriter {
    shell(GS_GAME_ENDPOINT, |w| {
        w.write_u8(mode_class);
        if mode_class != 0 {
            zstr8n(w, host, 256);
            w.write_u32(port);
        }
    })
}

/// `GS_SHELL_FLAG_90` `[flag:bool1]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §7.2 (0x90, meaning UNPROVEN); UDP_REFERENCE_FINDINGS_botclient.md O6 (REFERENCE: 1 at
// link creation, 0 after the client's world-ready)
pub fn shell_flag_90(flag: bool) -> BitWriter {
    shell(GS_SHELL_FLAG_90, |w| {
        w.write_bit(flag);
    })
}

/// `GS_SPRINT_ALLOWED` `[allowed:bool1]`.
// SPEC: CAPTURE_tdm1_GAME_VALIDATION.md §3.11 (`0d ca 01` / `0d ca 00`)
pub fn sprint_allowed(allowed: bool) -> BitWriter {
    shell(GS_SPRINT_ALLOWED, |w| {
        w.write_bit(allowed);
    })
}

/// `GS_OBJECT_NOTE` to a type-0x18 character: sub 0x11 with the 572-bit character record (the blob without its
/// type byte).
// SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §1.7 (0x11 re-reads the record); GAME_MESSAGES_2009_S2C.md §8.3
pub fn character_record_note(obj: u16, blob: &BitWriter) -> BitWriter {
    shell(GS_OBJECT_NOTE, |w| {
        w.write_u8(crate::modelobj::BLOB_CHARACTER)
            .write_u16(obj)
            .write_u8(NOTE_RECORD);
        let mut r = blob.reader();
        r.skip_bits(8);
        let n = r.remaining_bits();
        w.append(&r.read_bit_string(n));
    })
}

/// Sub-id of a type-0x18 note sent at a death: the client clears the character's attached client FX (and its
/// infected flag).
// SPEC: GAME_MESSAGES_2009_S2C.md §8.3 (cshell+0x206260 sub 0x1d -> cshell+0x205df0(0), cshell+0x202ed0; PROVEN)
pub const NOTE_DEATH: u8 = 0x1d;

/// `GS_OBJECT_NOTE` `[0x18][obj:objref16][0x1d]` (no body): the death note to a dead player's character.
// OBSERVED: capture 20261025_065016_tdm6 seq 10134 (`0da618fc001d`, character 252, unreliable,
// after the lethal shot effects in the kill tick) and seq 11052 (`0da6188c001d`, character 140); 20261025_002236_tdm4
// seq 3135 (`0da61898011d`, character 408); one per death on both links (REFERENCE, accepted by the real client)
// DC-DEFAULT: DSV-91 (proposed)
pub fn death_note(obj: u16) -> BitWriter {
    shell(GS_OBJECT_NOTE, |w| {
        w.write_u8(crate::modelobj::BLOB_CHARACTER)
            .write_u16(obj)
            .write_u8(NOTE_DEATH);
    })
}

/// Sub-id of a type-0x18 note with a player's aim: six f32 (view pitch, view yaw offset, their rates, their
/// accelerations).
// EVIDENCE: cshell+0x66c4b (type 0x18 client objects are the subclass of constructor cshell+0x28a30); DISASM:
// cshell+0x26f60 (its message slot: sub 0x80 -> cshell+0x259b0, six 32-bit reads to object +0x78..+0x8c)
pub const NOTE_AIM: u8 = 0x80;

/// `GS_OBJECT_NOTE` `[0x18][obj:objref16][0x80][aux:6 x u32]`: the aim of a player's character, the six aux words
/// of the player's own `UP_PLAYER_UPDATE` copied as they came.
// OBSERVED: capture 20261025_001409_tdm2 (the reference server: 593 sub 0x80 notes to object 189
// on both links, 240 bits each, unreliable, the words equal to the sender's 0x64 aux words, REFERENCE)
// DC-DEFAULT: DSV-153 (proposed)
pub fn aim_note(obj: u16, aux: [u32; 6]) -> BitWriter {
    shell(GS_OBJECT_NOTE, |w| {
        w.write_u8(crate::modelobj::BLOB_CHARACTER)
            .write_u16(obj)
            .write_u8(NOTE_AIM);
        for x in aux {
            w.write_u32(x);
        }
    })
}

/// Subs of a type-0x18 note with a character's health and armour as the HUD of a spectator shows them.
// DISASM: cshell+0x26f60 (the type-0x18 subclass's message slot: sub 0x14 `read(16)` -> object +0x5ae, sub 0x15
// `read(16)` -> +0x5b0, each kept only while object +0x40 is set and the match state is not 4 / 5), cshell+0x28970 /
// cshell+0x28980 (its slots +0x74 / +0x78 return +0x5ae / +0x5b0), cshell+0xcbea0 (the death panel's HP and AP digits
// of the followed player from those two slots)
pub const NOTE_HEALTH: u8 = 0x14;
pub const NOTE_ARMOUR: u8 = 0x15;

/// `GS_OBJECT_NOTE` `[0x18][obj:objref16][sub:u8][value:u16]`: a character's health (sub 0x14) or armour (0x15).
// DC-DEFAULT: DSV-521 (proposed, project decision) the value is the number the player's own HUD shows
pub fn vitals_note(obj: u16, sub: u8, value: u16) -> BitWriter {
    shell(GS_OBJECT_NOTE, |w| {
        w.write_u8(crate::modelobj::BLOB_CHARACTER)
            .write_u16(obj)
            .write_u8(sub)
            .write_u16(value);
    })
}

/// Subs of a type-0x18 note that mark a spawn: the start (unreliable, at the spawn) and the end (reliable, later).
// SPEC: GAME_MESSAGES_2009_S2C.md §8.3 (subs 0x1f / 0x25, no body; PLAUSIBLY a spawn-protection window)
// DC-DEFAULT: DSV-63 (proposed)
pub const NOTE_SPAWN_START: u8 = 0x1f;
pub const NOTE_SPAWN_END: u8 = 0x25;

/// `GS_OBJECT_NOTE` to a type-0x18 character with a body-less sub (the spawn notes).
// SPEC: GAME_MESSAGES_2009_S2C.md §8.3
// OBSERVED: capture 20261025_065016_tdm6 c6 seq 733 (`0da6188c001f`), seq 1162
// (`0da6188c0025`); REFERENCE
pub fn spawn_note(obj: u16, sub: u8) -> BitWriter {
    shell(GS_OBJECT_NOTE, |w| {
        w.write_u8(crate::modelobj::BLOB_CHARACTER)
            .write_u16(obj)
            .write_u8(sub);
    })
}

/// The object note of a door's activate entry (shell 0xa6, type 0x39): sub 1 `[disabled:bool1]`, sub 2
/// `[state:u8]` (DSV-340).
// SPEC: GAME_MESSAGES_2009_S2C.md §7.2 row 0xa6 (`[note_kind:u8][obj:objref16]` + class data, the object's slot +0x30)
// DISASM: cshell+0x2e1a70 (type 0x39 slot +0x30: `[sub:u8]`, sub 1 `read(1)` -> +0x39, sub 2 `read(8)` -> +0x3c)
pub fn door_note(obj: u16, sub: u8, value: u8) -> BitWriter {
    shell(GS_OBJECT_NOTE, |w| {
        w.write_u8(crate::doors::OBJ_DOOR_TYPE)
            .write_u16(obj)
            .write_u8(sub);
        if sub == crate::doors::NOTE_DISABLED {
            w.write_bit(value != 0);
        } else {
            w.write_u8(value);
        }
    })
}

/// `GS_HISEC_CASE` 0xd6: `[sub:u8]`, sub 0 nothing (the "a case has dropped" line), sub 1 `[case_key:u32][player:u32]`
/// (picked up: line, kill-feed note, HUD entry removed; a player not in the roster makes the client do nothing), sub
/// 2 `[case_key:u32][owner:u32]` (owner 0xFFFFFFFF = nobody: everyone sees the case).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.5 row 0xd6; HISEC_2009.md §3.2 (DISASM cshell+0x2e6a4..0x2e834)
// OBSERVED: capture 20261025_065016_tdm6 c6 seq 3006 (`0dd600`, `0dd6020100000000000000`),
// seq 4833 (`0dd60201000000ffffffff`), seq 10688 (`0dd6010200000001000000`); REFERENCE, accepted by the real client
pub const GS_HISEC_CASE: u8 = 0xd6;
pub const HISEC_DROPPED: u8 = 0;
pub const HISEC_TAKEN: u8 = 1;
pub const HISEC_RESERVE: u8 = 2;
/// `owner` of an unreserved case.
pub const HISEC_NOBODY: u32 = 0xFFFF_FFFF;
/// Note kind and sub of the case object's countdown (`GS_OBJECT_NOTE` to a type-0x17 object, sub 2).
// SPEC: GAME_MESSAGES_2009_S2C.md §8.3 (type 0x17 kind 5: sub 2 `[remaining:u32]`, cshell+0x2828e0 PROVEN)
pub const NOTE_PICKUP: u8 = 0x17;
pub const NOTE_PICKUP_REMAINING: u8 = 2;

/// 0xd6 sub 0: a case has dropped.
pub fn hisec_dropped() -> BitWriter {
    shell(GS_HISEC_CASE, |w| {
        w.write_u8(HISEC_DROPPED);
    })
}

/// 0xd6 sub 1: `player` (roster id) has picked up the case `key`.
pub fn hisec_taken(key: u32, player: u8) -> BitWriter {
    shell(GS_HISEC_CASE, |w| {
        w.write_u8(HISEC_TAKEN)
            .write_u32(key)
            .write_u32(u32::from(player));
    })
}

/// 0xd6 sub 2: the case `key` is reserved for `owner` (roster id), or for nobody.
pub fn hisec_reserve(key: u32, owner: Option<u8>) -> BitWriter {
    shell(GS_HISEC_CASE, |w| {
        w.write_u8(HISEC_RESERVE)
            .write_u32(key)
            .write_u32(owner.map_or(HISEC_NOBODY, u32::from));
    })
}

/// `GS_OBJECT_NOTE` `[0x17][obj:objref16][2][remaining:u32]`: the countdown the HUD draws at the case (red at <= 5,
/// nothing at 0).
// SPEC: GAME_MESSAGES_2009_S2C.md §8.3; HISEC_2009.md §3.1, §3.3 (cshell+0x2823b0)
// OBSERVED: capture 20261025_065016_tdm6 c6 seq 3034 (`0da6170201020f000000`: object 0x102,
// 15), seq 4833 (`0da61702010200000000`, 0 with the release); REFERENCE, accepted by the real client
pub fn hisec_countdown(obj: u16, remaining: u32) -> BitWriter {
    shell(GS_OBJECT_NOTE, |w| {
        w.write_u8(NOTE_PICKUP)
            .write_u16(obj)
            .write_u8(NOTE_PICKUP_REMAINING)
            .write_u32(remaining);
    })
}

/// Effect type 0x19 (`OBJ_PLAYER_SOUND`) of `S2C_SHELL_OBJ_MSG`: `[0x19][k:u8][u16][player:u32][pos:vec96]` for
/// `k` != 0x12. A weapon sound is `k` = the client's sound number with the weapon id; a jump is `k` 0x0f with 0.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 (type 0x19 inline record); CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.5 (U5:
// 0x80 -> type 0x19 `[sound_no][weapon_id][player:u32][pos]`, `k` 0x0f; REFERENCE, ACCEPTED); the S2C §8.2 relay table
// and GLOSSARY_UDP.md S.F read `k` 0x0f as the jump (relayed on the jump event), not a landing
pub const FX_PLAYER_SOUND: u8 = 0x19;
pub const FX_SOUND_JUMP: u8 = 0x0f;
/// `k` value whose record has another layout (`[u8][u32][vec96][u8][u8]`, the throw sound); never in the short form.
pub const FX_SOUND_LONG_FORM: u8 = 0x12;

pub fn sound_effect(k: u8, value: u16, player: u32, pos: [f32; 3]) -> BitWriter {
    let mut b = BitWriter::new();
    b.write_u8(FX_PLAYER_SOUND)
        .write_u8(k)
        .write_u16(value)
        .write_u32(player);
    vec96(&mut b, pos);
    crate::msg::s2c_shell_obj_msg(&b)
}

/// Effect type 0x19 with `k` 0x12: `[0x19][0x12][a:u8][player:u32][pos:vec96][b:u8][c:u8]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 (type 0x19, `k` = 0x12 form); CAPTURE_tdm4_KILL_VALIDATION.md §8.1 S8
pub fn throw_sound(a: u8, player: u32, pos: [f32; 3], b: u8, c: u8) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(FX_PLAYER_SOUND)
        .write_u8(FX_SOUND_LONG_FORM)
        .write_u8(a)
        .write_u32(player);
    vec96(&mut w, pos);
    w.write_u8(b).write_u8(c);
    crate::msg::s2c_shell_obj_msg(&w)
}

/// Effect type 0x06 of `S2C_SHELL_OBJ_MSG`: the shot effect the other clients draw for one `UP_FIRE`.
// SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.3 (reader cshell+0x2c6770, DISASM; 8 + 416 bits; REFERENCE, ACCEPTED)
pub const FX_SHOT: u8 = 0x06;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShotEffect {
    /// What the shot hit: an object, 0 = the world, `None` = nothing (written 0xFFFF).
    pub first: Option<u16>,
    pub shooter: u16,
    pub weapon: u16,
    pub ammo: u16,
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub normal: [f32; 3],
    /// `a`: the ROM surface id (`ATTRIBUTES\SURFACE.TXT` row `Id`) of what the shot hit; the client plays that
    /// surface's impact and, where the row allows, its mark.
    pub surface: u8,
    /// `b`: the effect suppress mask ([`SHOT_MASK_MISS`], [`SHOT_MASK_HIT`], [`SHOT_MASK_EXIT`]).
    // DC-DEFAULT: DSV-86 (proposed)
    pub mask: u16,
    /// `c`: the client id of the shooter (the id its own link got in `S2C_CLIENT_ID`); [`SHOT_CLIENT_NONE`] for a
    /// shooter without a client (a co-op AI character).
    // DC-DEFAULT: DSV-85 (proposed)
    pub client: u8,
    /// `e`, `f`, `g`: the `[Mod<n>]` numbers in the shooter weapon's muzzle, sight and magazine sockets
    /// ([`SHOT_MOD_NONE`] for an empty socket); [`NO_SHOT_MODS`] for a shot of no player weapon.
    // DC-DEFAULT: DSV-167 (proposed)
    pub mods: [u8; 3],
}

/// Surface ids of a hit on a character: the head row and the body row of the ROM surface table.
// DISASM: cshell+0x2c6770 (the reader replaces `a` 4 and 6 by 0 under the low-gore option), cshell+0x2c8540 ->
// cshell+0x37f0f0 (`a` selects the ROM surface row; triage aa-triage-red-dot §2.1)
// OBSERVED: capture 20261025_002236_tdm4 (4 of 4 body hits on bot 408 carried 6, seq 2968 .. 3132)
// and 20261025_065016_tdm6 (14 body hits 6; the one head-shot kill, seq 10134, 4; REFERENCE, accepted by the real client)
// DC-DEFAULT: DSV-29 (amendment, proposed)
pub const SURFACE_HEAD: u8 = 4;
pub const SURFACE_BODY: u8 = 6;
/// Surface id of a world hit, a miss and a blast: the one surface the reference stack's world records carried in the first
/// captures (the polygon's own surface is not traced: open question V-SURF-1).
// SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.3, §7 CV2-3 (proposed: a 71)
pub const TODO_SURFACE_WORLD: u8 = 71;
/// Surface id of a shot stopped by a sky face (MAP-24): the client's sky guard, which plays no impact, lays no mark
/// and makes no sound for it. The one shared number of dc-world (the ROM's sky faces carry it as their `texflags`,
/// dc-mapkit writes it on the sky faces it builds), narrowed to the record's byte.
// DC-DEFAULT: MAP-24
pub const SURFACE_SKY: u8 = {
    assert!(dc_world::SURFACE_SKY <= u8::MAX as u16);
    dc_world::SURFACE_SKY as u8
};

/// Opaque byte `d` of the shot effect as the reference stack sent it on every record; its meaning is not traced.
// SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.3, §7 CV2-3 (proposed: d 0)
// OBSERVED: capture 20261025_065016_tdm6 and 20261025_002236_tdm4 (every type-0x06 record of
// both links ends `00 ff ff ff`, players without mods; REFERENCE)
pub const TODO_SHOT_D: u8 = 0;
/// `e`/`f`/`g` of an empty mod socket: no `[Mod<n>]` row has this number, so the client's mod lookup finds nothing.
// OBSERVED: capture 20261025_065016_tdm6 and 20261025_002236_tdm4 (`ff ff ff` on every record of
// players without mods; REFERENCE)
// DISASM: cshell+0x2c6a7e..0x2c6a96 (each byte looked up as a mod row by cshell+0x1724e0, which returns none outside
// 0..count)
// DC-DEFAULT: DSV-167 (proposed)
pub const SHOT_MOD_NONE: u8 = 0xff;
pub const NO_SHOT_MODS: [u8; 3] = [SHOT_MOD_NONE; 3];

/// `b` of a miss, a world hit and a blast (the bullet form the reference stack sent most).
// SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.3, §7 CV2-3 (proposed: b 0x4f10)
// OBSERVED: capture 20261025_065016_tdm6 c6 (174 + 1 + 1 world records of a bullet 0x4f10;
// REFERENCE)
pub const SHOT_MASK_MISS: u16 = 0x4f10;
/// `b` of a hit on a character: bit 0x4000 clear, so the shooter's own client plays its hit feedback.
// OBSERVED: capture 20261025_002236_tdm4 c5 / c6 seq 2968, 3019, 3073 (0x0f11; the kill shot
// 3132 0x0f01), 20261025_065016_tdm6 c5 / c6 (the user's two hits 0x0f11, the bot's eight hits on the user 6 x
// 0x0f11 / 2 x 0x0f01); REFERENCE, accepted by the real client. Bit 0x0001 (0x0f11 vs 0x0f01) not traced: the majority
// form
// DISASM: cshell+0x2c8b36..0x2c8b57 (FX flags := (ammo row | 0x1810) & !b, extra flags := (row | 0x4000) & !b),
// cshell+0x2c8b8e (own shot: the hit feedback only when extra flag 0x4000 survives)
// DC-DEFAULT: DSV-86 (proposed)
pub const SHOT_MASK_HIT: u16 = 0x0f11;
/// `b` of the continuation record that follows a character hit (`first` 0, the end of the shot behind the victim).
// OBSERVED: capture 20261025_002236_tdm4 c6 seq 2968 (`first` 0, `a` 20, b 0x4bf0 after the
// 0x0f11 hit record in the same datagram; 0x4be0 after the 0x0f01 hit, seq 3135), 20261025_065016_tdm6 (8 x 0x4bf0,
// 2 x 0x4be0, one per hit record); REFERENCE
// DC-DEFAULT: DSV-86 (proposed)
pub const SHOT_MASK_EXIT: u16 = 0x4bf0;
/// `c` of a shot by a shooter that has no client (co-op AI); no client has this id, so every client draws the shot
/// as another player's.
// DC-DEFAULT: DSV-85 (proposed) our value, no capture
pub const SHOT_CLIENT_NONE: u8 = 0xff;

// SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.3 (reader cshell+0x2c6770, 8 + 416 bits)
// OBSERVED: capture 20261025_065016_tdm6 (`c` = the shooter's client id: 1 on the 140 records of
// the user, client id 1 at c6 seq 158, 0 on the bot's, client id 0 at c5 seq 159; REFERENCE)
// DISASM: cshell+0x2c6a66 (`c` -> FX +0xdc), cshell+0x2c85b8 (own client id -> FX +0xdd), cshell+0x2c6580 (a foreign
// shot plays the weapon's third-person fire sound)
// DC-DEFAULT: DSV-85 (proposed)
pub fn shot_effect(s: &ShotEffect) -> BitWriter {
    let (b, c) = (s.mask, s.client);
    // DC-DEFAULT: DSV-167 (proposed) `e`, `f`, `g` = the shooter weapon's mods by socket (muzzle, sight, magazine)
    // DISASM: cshell+0x2c6a7e..0x2c6a96 (`e` / `f` / `g` -> shot +0xfc / +0x100 / +0x104, each a mod row number)
    // EVIDENCE: cshell+0x2c6580 (the mod at +0xfc picks the silenced fire sound: `e` is the muzzle mod)
    let tail = [TODO_SHOT_D, s.mods[0], s.mods[1], s.mods[2]];
    let a = s.surface;
    let mut w = BitWriter::new();
    w.write_u8(FX_SHOT)
        .write_objref16(s.first)
        .write_u16(s.shooter)
        .write_u16(s.weapon)
        .write_u16(s.ammo)
        .write_u8(a)
        .write_u16(b)
        .write_u8(c);
    vec96(&mut w, s.from);
    vec96(&mut w, s.to);
    vec96(&mut w, s.normal);
    for x in tail {
        w.write_u8(x);
    }
    crate::msg::s2c_shell_obj_msg(&w)
}

/// `GS_PLAYER_STATUS` generic form with the two sub-bytes.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (cshell+0x5cb80)
pub fn player_status_b(kind: u8, b1: u8, b2: u8, value: f32) -> BitWriter {
    shell(GS_PLAYER_STATUS, |w| {
        w.write_u8(kind).write_u8(b1).write_u8(b2).write_f32(value);
    })
}

/// The body block of `GS_MOVER_UPDATE` flag 0x0001: a model file name, skins, and the non-solid bit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BodySpec {
    pub model: Vec<u8>,
    pub skins: Vec<Vec<u8>>,
    pub passable: bool,
}

/// `GS_MOVER_UPDATE` with the teleport block (new `seq`, position, half extents) and, when given, the body block.
/// Blocks are written in the reader's order (0x0020, 0x0004, 0x0001, 0x0008, 0x0080, 0x0100).
// SPEC: MOVEMENT_2009.md §12.4 (block order and layouts); GAME_MESSAGES_2009_S2C.md §4.2
pub fn mover_teleport(
    seq: u8,
    pos: [f32; 3],
    dims: [f32; 3],
    body: Option<&BodySpec>,
) -> BitWriter {
    let flags = MOVER_TELEPORT | if body.is_some() { MOVER_BODY } else { 0 };
    shell(GS_MOVER_UPDATE, |w| {
        w.write_u16(flags);
        w.write_u8(seq);
        vec96(w, pos);
        vec96(w, dims);
        if let Some(b) = body {
            write_body(w, b);
        }
    })
}

/// `GS_VIEW_ANGLES` `[kind:u8][pitch, yaw, roll:vec96]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.2; MOVEMENT_2009.md §12.5
pub fn view_angles(kind: u8, angles: [f32; 3]) -> BitWriter {
    shell(GS_VIEW_ANGLES, |w| {
        w.write_u8(kind);
        vec96(w, angles);
    })
}

/// `GS_PLAYER_STATUS` kind 1: the ammunition count of a weapon slot.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (kind 1 layout, cshell+0x60dda..0x60f1b); CAPTURE_tdm4_KILL_VALIDATION.md §2.6
pub fn player_ammo(weapon: u16, ammo_kind: u16, ammo: f32, slot: u8, extra: u16) -> BitWriter {
    shell(GS_PLAYER_STATUS, |w| {
        w.write_u8(STATUS_AMMO)
            .write_u16(weapon)
            .write_u16(ammo_kind)
            .write_f32(ammo)
            .write_u8(slot)
            .write_u16(extra);
    })
}

/// `GS_PLAYER_STATUS` of the generic form `[kind:u8][b1:u8][b2:u8][value:f32]` (health, armour, maxima).
// SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (cshell+0x5cb80)
pub fn player_status(kind: u8, value: f32) -> BitWriter {
    shell(GS_PLAYER_STATUS, |w| {
        w.write_u8(kind).write_u8(0).write_u8(0).write_f32(value);
    })
}

/// One loadout weapon for `GS_WEAPON_GIVE` / `GS_WEAPON_EQUIP`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeaponSpec {
    /// `weapon_db` index (the client's weapon table row).
    pub weapon: u16,
    pub ammo_kind: u16,
    /// Ammunition count outside the clip.
    pub ammo: f32,
    pub clip: u32,
    pub class_pos: u8,
    pub slot: u8,
    /// Part bytes: three attachment mod indices and the finish; -1 = none (see [`NO_PARTS`]).
    // DISASM: cshell+0x60d90 (0xb2 / 0xb3 empty the weapon's three sockets, then attach each part != -1 through
    // cshell+0x235ec0, which puts the mod into the socket its Kinds names)
    // DC-DEFAULT: DSV-80 (proposed) the server sends the weapon row's default attachments
    pub parts: [i8; 4],
}

/// Attachment parts and finish: -1 = none.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (`+0x104(part)` only for parts != -1, finish only when != -1)
pub const NO_PART: i8 = -1;

/// All four part bytes -1 (no attachment, no finish).
pub const NO_PARTS: [i8; 4] = [NO_PART; 4];

fn parts(w: &mut BitWriter, parts: &[i8; 4]) {
    for p in parts {
        w.write_s8(*p);
    }
}

/// `GS_WEAPON_GIVE` `[weapon:u16][ammo_kind:u16][ammo:f32][class_pos:u8][slot:u8][4 x part:i8]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4
pub fn weapon_give(s: &WeaponSpec) -> BitWriter {
    shell(GS_WEAPON_GIVE, |w| {
        w.write_u16(s.weapon)
            .write_u16(s.ammo_kind)
            .write_f32(s.ammo)
            .write_u8(s.class_pos)
            .write_u8(s.slot);
        parts(w, &s.parts);
    })
}

/// `GS_WEAPON_REMOVE`: a carried weapon leaves its slot.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4 row 0xb4 (cshell+0x61427..0x61486)
pub const GS_WEAPON_REMOVE: u8 = 0xb4;

/// `GS_WEAPON_REMOVE` `[slot:u8][weapon:u16][ammo_kind:u16][unused:u32]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4 row 0xb4
pub fn weapon_remove(slot: u8, weapon: u16, ammo_kind: u16) -> BitWriter {
    shell(GS_WEAPON_REMOVE, |w| {
        w.write_u8(slot)
            .write_u16(weapon)
            .write_u16(ammo_kind)
            .write_u32(0);
    })
}

/// `GS_WEAPON_EQUIP` `[weapon:u16][ammo_kind:u16][ammo:f32][clip:u32][class_pos:u8][slot:u8][4 x part:i8]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4
pub fn drawn_weapon(s: &WeaponSpec) -> BitWriter {
    shell(GS_WEAPON_EQUIP, |w| {
        w.write_u16(s.weapon)
            .write_u16(s.ammo_kind)
            .write_f32(s.ammo)
            .write_u32(s.clip)
            .write_u8(s.class_pos)
            .write_u8(s.slot);
        parts(w, &s.parts);
    })
}

/// The character look block: 15 signed bytes and two 8-bit strings (the client's `UP_PLAYER_INFO` sends the same
/// shape; the server forwards it after [`Look::sanitised`]).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 field 6; GAME_MESSAGES_2009_C2S.md §5.1 (`look`, PLAUSIBLE pairing)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Look {
    pub bytes: [i8; 15],
    pub text_a: Vec<u8>,
    pub text_b: Vec<u8>,
}

/// Index of the look byte the client reads as its overridable-model row (look object +0x13c): a row >= 0 whose
/// `FaceIndex` is set replaces the face of the character's model row, and the face row's `ClientFX` keys are
/// attached to the character as looping client FX.
// EVIDENCE: cshell+0x1c2a90 (the 15th look byte -> look +0x13c); cshell+0x206730 (type-0x18 slot +0x3c: face row
// cshell+0x1e3430(model, override), FX list cshell+0x1e1820, attach cshell+0x2065f0); DISASM cshell+0x2ce24..0x2ce58
// (the 0x6b handler calls slot +0x3c with the parsed look); ae-triage-reddot2-blast §1.3
pub const LOOK_OVERRIDE_BYTE: usize = 14;
/// The overridable-model row value "none".
// OBSERVED: capture 20261025_065016_tdm6 (the real client's own look has -1 in byte 14 on both
// stacks; the reference stack's look for the bot too)
pub const LOOK_NO_OVERRIDE: i8 = -1;
/// The look the server sends for a client whose look bytes are all 0 (a client without a character look, such as a
/// test bot).
// OBSERVED: capture 20261025_001409_tdm2 seq 209, 20261025_002236_tdm4 seq 212,
// 20261025_065016_tdm6 seq 219 (the reference stack's 0x6b for the bot whose 0x72 look was all 0 (tdm6 conn 5 seq 217)
// carried these bytes; REFERENCE, accepted by the real client)
// DC-DEFAULT: DSV-90 (proposed)
pub const DEFAULT_LOOK_BYTES: [i8; 15] = [0, 0, 0, 0, 0, 1, 0, 0, 0, 0, -1, -1, 0, 0, -1];

impl Look {
    /// The look as the server forwards it in `GS_PLAYER_INFO`: an all-zero byte block becomes
    /// [`DEFAULT_LOOK_BYTES`], and byte [`LOOK_OVERRIDE_BYTE`] is always [`LOOK_NO_OVERRIDE`] (no current mode uses
    /// an overridable model; row 0 is an infected face whose looping eye FX the client keeps on a hidden dead
    /// character: the red dot at the death spot). The two strings pass unchanged.
    // DC-DEFAULT: DSV-90 (proposed)
    // OBSERVED: capture 20261025_125432_rust6 conn 5 seq 810, conn 6 seq 275 (our dserver
    // forwarded the bot's all-zero look; the user saw a red dot at the bot's death spot); ae-triage-reddot2-blast §1.3
    pub fn sanitised(&self) -> Look {
        let mut l = self.clone();
        if l.bytes.iter().all(|b| *b == 0) {
            l.bytes = DEFAULT_LOOK_BYTES;
        }
        l.bytes[LOOK_OVERRIDE_BYTE] = LOOK_NO_OVERRIDE;
        l
    }
}

/// Buffer sizes of the two look strings in the client's reader.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 field 6 (`look_a:zstr8(128)`, `look_b:zstr8(128)`)
pub const LOOK_TEXT_BYTES: usize = 128;

/// Writes a look block.
pub fn write_look(w: &mut BitWriter, l: &Look) {
    for b in l.bytes {
        w.write_s8(b);
    }
    zstr8n(w, &l.text_a, LOOK_TEXT_BYTES);
    zstr8n(w, &l.text_b, LOOK_TEXT_BYTES);
}

/// `GS_PLAYER_INFO` record kinds: 0 = new player with the join notice, 1 = add or refresh, 2 = add or update without
/// a notice (cshell+0x2b820 falls back to the add when the pid has no record).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 field 14; CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §2.5 (kind 2 correction)
pub const INFO_NEW: u8 = 0;
pub const INFO_REFRESH: u8 = 1;
pub const INFO_UPDATE: u8 = 2;

/// One roster record of `GS_PLAYER_INFO`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfo {
    /// 8-bit display name.
    pub name: Vec<u8>,
    /// The player's engine client id (< 256).
    pub pid: u8,
    pub account_no: u32,
    /// The player's world object (must already exist on the receiving client).
    pub character: Option<u16>,
    pub look: Look,
    pub kills: i16,
    pub deaths: u8,
    /// 1 or 2.
    pub team: u8,
    pub dead: bool,
    pub kind: u8,
    /// Field 21 `m1` (record +0x1f8): in the co-op class the player's lives counting the current one, the same
    /// field 0xd7 sub 2 `b` writes (0 elsewhere).
    // SPEC: FIRETEAM_2009.md §2.1 (0x6b m1 = record +0x1f8, cshell+0x2a950), §2.3
    pub mode_m1: u8,
    /// Field 21 `m2` (record +0x1f4): in the co-op class the player's points, the field 0xd7 sub 1 writes.
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 row 21, §3.5 (0xd7 sub 1 record +0x1f4 := points)
    pub mode_m2: u32,
    /// Fields 9-11 `v64`, `v68`, `v6c` (record +0x64 / +0x68 / +0x6c): the board's mission column. The 2009 board
    /// shows `v68/v6c` in the flag classes 7 / 8, else `v64` (cshell+0xc0ac0, cshell+0xbe7c0).
    // DC-DEFAULT: DSV-253 (proposed, playtest report "mission scoreboard not updating") flag classes: v68 = the
    // player's captures, v6c = his returns of the own flag; 0 elsewhere
    pub mission: [u8; 3],
    /// Fields 15-17: rank (`h2`) and clan group block.
    // DC-DEFAULT: DSV-141 (proposed)
    pub mark: PlayerMark,
}

/// The clan group block of `GS_PLAYER_INFO` (field 16).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClanMark {
    /// Clan name (record +0x1b0), at most 127 UTF-16 units on the wire.
    pub name: Vec<u16>,
    /// `group_no` (record +0x1cc, low half): non-zero means "in a clan"; the target-name panel shows the clan name only
    /// then. We send the lobby's clan number.
    // EVIDENCE: cshell+0xcbea0 (record +0x1cc == 0 hides the clan text, else the text of +0x1b0 is set and shown)
    pub group_no: u64,
    /// `g1` / `g2` (record +0x1d0 / +0x1d4): the two clan mark layer ids, layer 0 then layer 1, each looked up in the
    /// ROM clan mark table (an id below 1 draws nothing). We send the lobby's emblem, then its background.
    // EVIDENCE: cshell+0xcbea0 (with record +0x1af set: cshell+0x2811e0 / cshell+0x280570 layer 0 with +0x1d0, layer
    // 1 with +0x1d4); cshell+0x280570 (id < 1 or past the list: no image)
    pub mark: [u32; 2],
    /// `g3` (record +0x1dc): the clan name's text colour, 0 = the panel's default colour.
    // EVIDENCE: cshell+0xcbea0 (+0x1dc == 0 -> the panel's default colour, else the value is the colour)
    pub name_colour: u32,
}

/// Fields 15-17 of `GS_PLAYER_INFO`: the clan group block and the words `h1` / `h2`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlayerMark {
    /// `h2` (record +0x210): the rank number; the world mark shows the level table's small icon for it (0 = trainee).
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 field 17 (CORRECTION 2026-10-26: h2 = the world mark's rank)
    pub rank: u32,
    /// `h1` (record +0x1d8): the player name's text colour, 0 = the panel's default colour.
    // EVIDENCE: cshell+0xcbea0 (+0x1d8 == 0 -> the panel's default colour, else the value is the colour)
    pub name_colour: u32,
    /// `has_group` (record +0x1af) and the group block; `None` = no clan.
    pub clan: Option<ClanMark>,
}

impl PlayerMark {
    /// The mark of a player from the lobby's `user` line tokens (`rank=`, `clan=`, `clanlabel=`, `mark=`,
    /// `namecolour=`, `clancolour=`): no tokens give rank 0, no clan and the default colours (0). The colour tokens
    /// carry the colour words the lobby looked up for the player's colour items.
    // DC-DEFAULT: DSV-141 (proposed); DSV-606 (proposed) the colour words of the lobby's colour tokens
    pub fn of_roster(e: Option<&crate::pipe::RosterEntry>) -> PlayerMark {
        let Some(e) = e else {
            return PlayerMark::default();
        };
        PlayerMark {
            rank: e.rank().unwrap_or(0),
            name_colour: e.name_colour(),
            clan: e.clan().map(|c| ClanMark {
                name: c.name.encode_utf16().collect(),
                group_no: c.id as u64,
                mark: [c.emblem, c.background],
                name_colour: c.colour,
            }),
        }
    }
}

/// Clan name buffer of the group block (`zstr16(128)`).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 field 16
const CLAN_NAME_UNITS: usize = 128;

/// `GS_PLAYER_INFO`, fields 1-21 of §3.2.1 with the record's own mark (see [`player_info_marked`]).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 (DISASM cshell+0x2c75f..0x2c9fa)
pub fn player_info(p: &PlayerInfo) -> BitWriter {
    player_info_marked(p, &p.mark)
}

/// `GS_PLAYER_INFO`, fields 1-21 of §3.2.1, with the rank and clan of `mark` (fields 15-17); other unknown values 0.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 (DISASM cshell+0x2c75f..0x2c9fa)
pub fn player_info_marked(p: &PlayerInfo, mark: &PlayerMark) -> BitWriter {
    shell(GS_PLAYER_INFO, |w| {
        zstr8n(w, &p.name, 64);
        w.write_u8(p.pid).write_u32(p.account_no);
        w.write_bit(false); // flag_a
        w.write_objref16(p.character);
        write_look(w, &p.look);
        w.write_s16(p.kills).write_u8(p.deaths);
        w.write_u8(p.mission[0])
            .write_u8(p.mission[1])
            .write_u8(p.mission[2]); // v64, v68, v6c (DSV-253)
        w.write_u8(p.team);
        w.write_bit(p.dead); // flag_b -> record +0x74 (dead flag)
        w.write_u8(p.kind);
        w.write_bit(mark.clan.is_some()); // has_group
        if let Some(c) = &mark.clan {
            zstr16n(w, &c.name, CLAN_NAME_UNITS);
            w.write_u64(c.group_no);
            w.write_u32(c.mark[0])
                .write_u32(c.mark[1])
                .write_u32(c.name_colour);
        }
        w.write_u32(mark.name_colour).write_u32(mark.rank); // h1, h2
        w.write_bit(false).write_bit(false).write_bit(false); // e1, e2, e3
        w.write_u8(0); // unused
        w.write_u8(p.mode_m1)
            .write_u32(p.mode_m2)
            .write_u32(0)
            .write_u16(0); // m1..m4 mode values
    })
}

/// `GS_PLAYER_LEFT` `[pid:u8][kicked:u8]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2
pub fn player_left(pid: u8, kicked: bool) -> BitWriter {
    shell(GS_PLAYER_LEFT, |w| {
        w.write_u8(pid).write_u8(kicked as u8);
    })
}

/// `GS_MATCH_SETTINGS` content (TDM: `round_limit` 1, `round_no` 0).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MatchSettings {
    pub caption: Vec<u8>,
    pub own_team: u8,
    pub goal: u32,
    pub scores: [u32; 2],
    pub round_limit: u32,
    pub round_no: u32,
    /// `own_value`: the client stores it as its shot counter (the 0x82 `stat`, the cone's table index).
    // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1.5 (`local_player`+0x1fc := own_value)
    pub shot_seed: u32,
    /// Classes 7 / 8: the flag carrier records `[flag_team][carrier][carrier_team]` (late joiners learn who carries
    /// which flag).
    // SPEC: GAME_MODES_2009.md §5.5 (C-GM-6, PROVEN cshell+0x5bcd0)
    pub carriers: Vec<[u32; 3]>,
}

/// `GS_MATCH_SETTINGS` `[lobby_caption:zstr8(64)][unused:u8][own_team:i8][own_value:u32][goal:u8][score_team1:i8]
/// [score_team2:i8][round_limit:i8][round_no:i8]` and, for mode classes 7 / 8, `[n:u32 = 0]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2 (cshell+0x2c4ff..0x2c724)
pub fn match_settings(s: &MatchSettings, mode_class: u8) -> BitWriter {
    shell(GS_MATCH_SETTINGS, |w| {
        zstr8n(w, &s.caption, 64);
        w.write_u8(0);
        w.write_s8(clamp_i8(s.own_team as i64));
        w.write_u32(s.shot_seed); // own_value
        w.write_u8(clamp_u8(s.goal));
        w.write_s8(clamp_i8(s.scores[0] as i64));
        w.write_s8(clamp_i8(s.scores[1] as i64));
        w.write_s8(clamp_i8(s.round_limit as i64));
        w.write_s8(clamp_i8(s.round_no as i64));
        if mode_class == MODE_CLASS_7 || mode_class == MODE_CLASS_8 {
            w.write_u32(s.carriers.len() as u32);
            for c in &s.carriers {
                for v in c {
                    w.write_u32(*v);
                }
            }
        }
    })
}

/// One flag event of [`GS_OBJECTIVE_STATE`]. `carrier` and `actor` are client ids (`u32::MAX` = none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectiveState {
    pub obj: u16,
    pub state: u32,
    pub flag_team: u32,
    pub carrier: u32,
    pub actor: u32,
    pub event: u32,
    pub at_base: bool,
    pub quiet: bool,
}

/// [`GS_OBJECTIVE_STATE`]: the client gives the flag object of `flag_team` the `state` and `at_base`, tells the own
/// player whether he carries it (`carrier` = own id), counts captures (event 1) and returns (event 3) for `actor`
/// unless `quiet`, and shows the event's notice and sound.
// SPEC: GAME_MODES_2009.md §5.5 (C-GM-4: field roles; event 1 captured, 2 taken, 3 returned, 4 dropped / reset,
// PLAUSIBLE); DISASM cshell+0x5aae0..0x5ac2f
pub fn objective_state(o: &ObjectiveState) -> BitWriter {
    shell(GS_OBJECTIVE_STATE, |w| {
        w.write_u16(o.obj)
            .write_u32(o.state)
            .write_u32(o.flag_team)
            .write_u32(o.carrier)
            .write_u32(o.actor)
            .write_u32(o.event)
            .write_bit(o.at_base)
            .write_bit(o.quiet);
    })
}

/// [`GS_BOMB_PLANTED`] with the bomb timer in seconds (0 shows a plain notice instead of the timer).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.2 row 0x51
pub fn planted_notice(bomb_secs: u32) -> BitWriter {
    shell(GS_BOMB_PLANTED, |w| {
        w.write_u32(bomb_secs);
    })
}

/// [`GS_BOUNTY_MODE`] with the sub-message `body` (sub byte first).
pub fn bounty_mode(body: &BitWriter) -> BitWriter {
    s2c_shell_msg(GS_BOUNTY_MODE, body)
}

/// [`GS_INFECTION_MODE`] with the sub-message `body` (sub byte first).
pub fn infection_mode(body: &BitWriter) -> BitWriter {
    s2c_shell_msg(GS_INFECTION_MODE, body)
}

/// [`GS_BOMB_DEFUSED`].
pub fn defused_notice() -> BitWriter {
    shell(GS_BOMB_DEFUSED, |_| {})
}

/// [`GS_BOMB_EXPLODED`].
pub fn exploded_notice() -> BitWriter {
    shell(GS_BOMB_EXPLODED, |_| {})
}

/// [`GS_OBJECTIVE_ITEM`] for `item` (`None` = no object).
pub fn carried_mark(item: Option<u16>) -> BitWriter {
    shell(GS_OBJECTIVE_ITEM, |w| {
        w.write_objref16(item);
    })
}

/// [`GS_TRACK_OBJECT`]: the use progress on `obj` with `hold_secs` to go; `kind` must be non-zero for a client
/// outside its in-match state to take it.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4 row 0xad (`kind` != 0 or the in-match flag passes it on); `a` / `c` UNPROVEN
pub fn track_object(obj: u16, kind: u8, hold_secs: f32) -> BitWriter {
    shell(GS_TRACK_OBJECT, |w| {
        w.write_objref16(Some(obj))
            .write_u8(kind)
            .write_u32(0)
            .write_f32(hold_secs)
            .write_u32(0);
    })
}

/// The intro block of `GS_MATCH_BEGIN` (read by the client only when its two intro images loaded).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Intro {
    pub mode_caption: Vec<u8>,
    pub notice_head: Vec<u8>,
    pub notice_body: Vec<u8>,
    pub map_image_a: Vec<u8>,
    pub map_min: [f32; 3],
    pub map_max: [f32; 3],
    pub radar_image: Vec<u8>,
    pub map_image_b: Vec<u8>,
    pub radar_min: [f32; 3],
    pub radar_max: [f32; 3],
    pub radar_zoom: f32,
    pub radar_rot: f32,
    pub radar_p3: u32,
    pub radar_p4: u32,
    pub base_team1: [f32; 3],
    pub base_team2: [f32; 3],
}

/// `GS_MATCH_BEGIN` content.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MatchBegin {
    pub round_secs: u32,
    /// The second flag of the head (named `first_round` in older notes). Set, the 2009 client latches its
    /// "waiting for players" training banner (HUD side-switch element +0x6a, picture of the HUD layout key
    /// `TrainingModInfoTex`) and draws it every frame until a later 0x45 with the flag clear or a 0x46.
    // DISASM: cshell+0x49dff (+0x6a cleared at the top of every 0x45), cshell+0x49e9b (the second read(1)),
    // cshell+0x49eab (call cshell+0xcd340: +0x6a := 1), cshell+0xcd170 (per-frame draw of +0x54 while +0x6a),
    // cshell+0xcd4d0 (+0x54 loaded from `TrainingModInfoTex`); case 0x46 of cshell+0x49a40 (cshell+0xcd350: clear)
    // PROVEN-CAPTURE: reference 0x45 flag 0 (tdm1 c5, tdm2 c6, tdm4 c5 / c6, menu-frida6 c3); rust4 c5 flag 1 + the
    // user's report (Wave AA item 7)
    pub waiting_banner: bool,
    pub countdown_secs: u32,
    /// `Some` = `show_intro` 1 with this block.
    pub intro: Option<Intro>,
    pub round_no: u32,
}

/// `GS_MATCH_BEGIN`: `[round_secs:u32][show_intro:bool1][waiting_banner:bool1][countdown_secs:u32]`, the intro block
/// when `show_intro` (mode-dependent parts per §2.1.1 rows a, i, j), then `[round_no:u32]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1.1 (cshell+0x49de1..0x4a5f0)
pub fn match_begin(m: &MatchBegin, mode_class: u8) -> BitWriter {
    shell(GS_MATCH_BEGIN, |w| {
        w.write_u32(m.round_secs);
        w.write_bit(m.intro.is_some());
        w.write_bit(m.waiting_banner);
        w.write_u32(m.countdown_secs);
        if let Some(i) = &m.intro {
            if matches!(
                mode_class,
                MODE_CLASS_6 | MODE_CLASS_9 | MODE_CLASS_11 | MODE_COOP
            ) {
                zstr8n(w, &i.mode_caption, 256);
            }
            zstr8n(w, &i.notice_head, 256);
            zstr8n(w, &i.notice_body, 256);
            zstr8n(w, &i.map_image_a, 256);
            vec96(w, i.map_min);
            vec96(w, i.map_max);
            zstr8n(w, &i.radar_image, 256);
            zstr8n(w, &i.map_image_b, 256);
            vec96(w, i.radar_min);
            vec96(w, i.radar_max);
            w.write_f32(i.radar_zoom)
                .write_f32(i.radar_rot)
                .write_u32(i.radar_p3)
                .write_u32(i.radar_p4);
            if mode_class != MODE_CLASS_6 {
                vec96(w, i.base_team1);
                vec96(w, i.base_team2);
            }
            if mode_class == MODE_COOP {
                w.write_u8(0).write_u8(0).write_bit(false);
            }
        }
        w.write_u32(m.round_no);
    })
}

/// `GS_TEAM_SCORE` `[score_team1:u8][score_team2:u8]` (scores above 255 are sent as 255).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1
pub fn team_score(scores: [u32; 2]) -> BitWriter {
    shell(GS_TEAM_SCORE, |w| {
        w.write_u8(clamp_u8(scores[0]))
            .write_u8(clamp_u8(scores[1]));
    })
}

/// `GS_ROUND_TIMER_STOP` `[round_secs:u32]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1
pub fn round_timer_stop(round_secs: u32) -> BitWriter {
    shell(GS_ROUND_TIMER_STOP, |w| {
        w.write_u32(round_secs);
    })
}

/// `GS_ROUND_RESULT` `[round_secs:u32][win_team:i8][wins_team1:u8][wins_team2:u8]`; for the co-op class a
/// stage-0 block of zeros follows (the client reads it unconditionally for that class).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1.2 (cshell+0x4a608..0x4aa42)
pub fn round_result(round_secs: u32, win_team: i8, wins: [u32; 2], mode_class: u8) -> BitWriter {
    shell(GS_ROUND_RESULT, |w| {
        w.write_u32(round_secs).write_s8(win_team);
        w.write_u8(clamp_u8(wins[0])).write_u8(clamp_u8(wins[1]));
        if mode_class == MODE_COOP {
            w.write_u8(0);
            for _ in 0..6 {
                w.write_u32(0);
            }
        }
    })
}

/// One `GS_PLAYER_SUMMARY` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Summary {
    pub pid: u8,
    /// The player's character object (the reference stack filled the `objref16`).
    pub obj: Option<u16>,
    pub exp_before: u32,
    pub exp_gain: u32,
    pub exp_bonus: u32,
    pub gp_gain: u32,
    pub gp_bonus: u32,
    /// The co-op block (class 5 only): `c1`, `c2`, `c3`, `c4` (record +0x208, +0x204, +0x1f4, +0x1fc); `n` is 0.
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.4 row 0x65 (co-op block)
    pub coop: [u32; 4],
}

/// `GS_PLAYER_SUMMARY` `[pid:u8][obj:objref16][exp_before:u32][exp_gain:u32][exp_bonus:u32][gp_gain:u32]
/// [gp_bonus:u32][flag:bool1]` (+ an empty co-op block for the co-op class).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.4 (cshell+0x2e2fd..0x2e4c6); CAPTURE_tdm2_TWOPLAYER_VALIDATION.md (0x65 with the
// character object 408; REFERENCE)
pub fn player_summary(s: &Summary, mode_class: u8) -> BitWriter {
    shell(GS_PLAYER_SUMMARY, |w| {
        w.write_u8(s.pid).write_objref16(s.obj);
        w.write_u32(s.exp_before)
            .write_u32(s.exp_gain)
            .write_u32(s.exp_bonus)
            .write_u32(s.gp_gain)
            .write_u32(s.gp_bonus);
        w.write_bit(false);
        if mode_class == MODE_COOP {
            w.write_u32(s.coop[0]).write_u32(0); // c1, n = 0 flags
            w.write_u32(s.coop[1])
                .write_u32(s.coop[2])
                .write_u32(s.coop[3]);
        }
    })
}

/// `GS_ROUND_WINS` `[wins_team1:u8][wins_team2:u8]`. In the TDM classes the two values are the team kill totals:
/// the reference stack sent them after every kill and [0][0] at the round start; the client stores them as its scoreboard
/// values (cshell+0x92bb0).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.4; CAPTURE_tdm4_KILL_VALIDATION.md §3.2 item 2 (REFERENCE, weakly CONFIRMED)
// DC-DEFAULT: DC-KILL-3 (proposed) 0xb5 after every kill
pub fn round_wins(wins: [u32; 2]) -> BitWriter {
    shell(GS_ROUND_WINS, |w| {
        w.write_u8(clamp_u8(wins[0])).write_u8(clamp_u8(wins[1]));
    })
}

/// `GS_TEAM_SWAP`: `[score_team1:u8][score_team2:u8]`. The client sets its scoreboard scores and switches its own
/// team to the other one (1 -> 2, 2 -> 1); the other roster records keep their team until a `GS_PLAYER_INFO` names
/// a new one.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.3 row 0x5e
// EVIDENCE: cshell+0x49a40 case 0x5e (two `read(8)`, scoreboard cshell+0x92bb0, own team flipped through
// cshell+0x1b1330 only when it is 1 or 2), cshell+0x1b1330 (writes `session_info`+0xf8)
pub const GS_TEAM_SWAP: u8 = 0x5e;
/// `GS_CLAN_STATUS`: `[kind:u8]`, kind 1 `[score_team1:u8][score_team2:u8][no_swap:bool1]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.3 row 0x5f
// EVIDENCE: cshell+0x49a40 case 0x5f (kind 1: team counters, scoreboard; `no_swap` 0 = the 0x5e swap plus the
// team-score, spectator, scoreboard and roster refreshes)
pub const GS_CLAN_STATUS: u8 = 0x5f;
/// `GS_CLAN_STATUS` kind of the score and side form.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.3 row 0x5f (kind 1)
pub const CLAN_STATUS_SCORES: u8 = 1;

/// `GS_TEAM_SWAP` `[score_team1:u8][score_team2:u8]` (scores above 255 are sent as 255).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.3 row 0x5e
pub fn team_swap(scores: [u32; 2]) -> BitWriter {
    shell(GS_TEAM_SWAP, |w| {
        w.write_u8(clamp_u8(scores[0]))
            .write_u8(clamp_u8(scores[1]));
    })
}

/// `GS_CLAN_STATUS` kind 1 `[1][score_team1:u8][score_team2:u8][no_swap:bool1]`; `no_swap` false makes the client
/// switch its own team as with 0x5e.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.3 row 0x5f
pub fn clan_status_scores(scores: [u32; 2], no_swap: bool) -> BitWriter {
    shell(GS_CLAN_STATUS, |w| {
        w.write_u8(CLAN_STATUS_SCORES)
            .write_u8(clamp_u8(scores[0]))
            .write_u8(clamp_u8(scores[1]));
        w.write_bit(no_swap);
    })
}

/// `GS_ROUND_OBJECTS_END` `[unused:u32]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1 row 0x47
pub fn round_objects_end(unused: u32) -> BitWriter {
    shell(GS_ROUND_OBJECTS_END, |w| {
        w.write_u32(unused);
    })
}

/// `GS_ROUND_CLOSE` `[round_secs:u32]` (the banner timer, hidden round timer).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1 row 0x48
pub fn round_close(round_secs: u32) -> BitWriter {
    shell(GS_ROUND_CLOSE, |w| {
        w.write_u32(round_secs);
    })
}

/// Shell 0x9d `[value:u32]` (the reference stack's form; meaning UNPROVEN).
// SPEC: GAME_MESSAGES_2009_S2C.md §12 O2 note; CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §2.2 (`0d 9d 00 00 00 00`)
pub fn shell_9d(value: u32) -> BitWriter {
    shell(GS_SHELL_9D, |w| {
        w.write_u32(value);
    })
}

/// `GS_MATCH_RESULT_SCREEN` `[result_flag:bool1][result_value:u32]` (field meanings UNPROVEN, V-S2C-3).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1
pub fn result_screen(flag: bool, value: u32) -> BitWriter {
    shell(GS_MATCH_RESULT_SCREEN, |w| {
        w.write_bit(flag).write_u32(value);
    })
}

/// `GS_LEVEL_EXIT` `[next_world:zstr8(260)][keep:bool1][now:bool1]`; an empty world with `now` takes the client
/// back to its menu screens.
// SPEC: GAME_MESSAGES_2009_S2C.md §6.1 (cshell+0x22f950), §10 step 22
pub fn level_exit(next_world: &[u8], keep: bool, now: bool) -> BitWriter {
    shell(GS_LEVEL_EXIT, |w| {
        zstr8n(w, next_world, 260);
        w.write_bit(keep).write_bit(now);
    })
}

/// `GS_PING_TABLE` `{[pid:u16][ping:u16]}` then `[0xFFFF:u16]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2 (cshell+0x2d73a..0x2d798)
pub fn ping_table(rows: &[(u8, u16)]) -> BitWriter {
    shell(GS_PING_TABLE, |w| {
        for &(pid, ping) in rows {
            w.write_u16(pid as u16).write_u16(ping);
        }
        w.write_u16(PING_TABLE_END);
    })
}

/// `GS_PERF_STATS` sub 1 `[1][pid:u32][enable:u8][ping_ms:u16][stats_ms:u16]`: enables the client's reports.
// SPEC: GAME_MESSAGES_2009_C2S.md §5.8 (handler cshell+0x2acd0 -> cshell+0x29300)
pub fn perf_enable(pid: u32, enable: bool, ping_ms: u16, stats_ms: u16) -> BitWriter {
    shell(GS_PERF_STATS, |w| {
        w.write_u8(1)
            .write_u32(pid)
            .write_u8(enable as u8)
            .write_u16(ping_ms)
            .write_u16(stats_ms);
    })
}

/// `GS_PERF_STATS` sub 3 echo `[3][pid:u32][t:f32][3 x u32]` (the client turns `t` into its round-trip time).
// SPEC: GAME_MESSAGES_2009_C2S.md §5.8; GAME_MESSAGES_2009_S2C.md §3.6
pub fn perf_echo(pid: u32, t: f32, words: [u32; 3]) -> BitWriter {
    shell(GS_PERF_STATS, |w| {
        w.write_u8(3).write_u32(pid).write_f32(t);
        for x in words {
            w.write_u32(x);
        }
    })
}

/// `GS_PERF_STATS` sub 2 relay `[2][pid:u32][n:u16][3 x u16]`.
// SPEC: GAME_MESSAGES_2009_C2S.md §5.8; GAME_MESSAGES_2009_S2C.md §3.6
pub fn perf_relay(pid: u32, values: [u16; 4]) -> BitWriter {
    shell(GS_PERF_STATS, |w| {
        w.write_u8(2).write_u32(pid);
        for x in values {
            w.write_u16(x);
        }
    })
}

/// `GS_KILL` content (§3.3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Kill {
    pub victim: u8,
    /// [`NO_KILLER`] for a death without a killer.
    pub killer: u8,
    pub kill_kind: u8,
    pub weapon: u16,
    pub hit_zone: u8,
    pub relation: bool,
    pub keep_hud: bool,
    /// `streak_flag` (u8 at bit 68) and `feed_value` (u8 at bit 76).
    pub streak_flag: u8,
    pub feed_value: u8,
    pub no_count: bool,
}

/// `GS_KILL` with the fields of §3.3.1 in order (no revenge block; co-op values 0).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 (cshell+0x2d922..0x2e2ed)
pub fn kill(k: &Kill) -> BitWriter {
    shell(GS_KILL, |w| {
        w.write_u8(k.victim)
            .write_u8(k.killer)
            .write_u8(k.kill_kind)
            .write_u16(k.weapon)
            .write_u8(k.hit_zone);
        w.write_bit(k.relation); // f_relation
        w.write_bit(false); // f_feed2
        w.write_bit(k.keep_hud);
        w.write_bit(false); // f_feed6
        w.write_u8(k.streak_flag);
        w.write_u8(k.feed_value);
        w.write_bit(k.no_count);
        w.write_bit(false); // has_revenge
        w.write_bit(false); // f_revenge_done
        w.write_u32(0).write_u32(0); // coop_a, coop_b
    })
}

/// `GS_MULTI_KILL` `[kills:u16][head:bool1][combo:u32]`: `combo` > 1 shows the client's "N COMBO" banner with the
/// digits of `combo` (a sound only when `head` is set); else the head-shot banner when `head`, else the one-kill
/// banner with the digits of `kills` (nothing for 0). The banner shows on the receiving client whoever it is.
/// Not used by the TDM rule (DSV-105): the multi-kill banner with its voice line is the 0x6d level; co-op sends it per
/// AI kill (DSV-602).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3
// DISASM: cshell+0x2d8ef..0x2d918 (`combo` > 1 -> cshell+0xa52a0(`head`, `combo`, 0), else
// cshell+0xa57b0(`head`, `kills`, 0)); cshell+0xa52e0 / cshell+0xa57f9 (`head` is the head-shot flag), cshell+0xa588d
// (the one-kill form needs `kills` > 0)
pub fn streak_banner(kills: u16, head: bool, combo: u32) -> BitWriter {
    shell(GS_MULTI_KILL, |w| {
        w.write_u16(kills).write_bit(head).write_u32(combo);
    })
}

/// `GS_DAMAGE_TAKEN` content.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Damage {
    pub from_dir: [f32; 3],
    /// The byte the client does not read (the reference stack repeated `zone` there).
    pub unused: u8,
    pub mode: u8,
    pub ammo_kind: u16,
    pub dmg_kind: u8,
    pub zone: u8,
    pub part: i8,
    pub damage: u32,
}

/// `GS_DAMAGE_TAKEN` `[from_dir:vec96][unused:u8][mode:u8][ammo_kind:u16][dmg_kind:u8][zone:u8][part:i8]
/// [damage:u32]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (cshell+0x1f7d20); CAPTURE_tdm4_KILL_VALIDATION.md §3.1 (observed form)
pub fn damage_taken(d: &Damage) -> BitWriter {
    shell(GS_DAMAGE_TAKEN, |w| {
        vec96(w, d.from_dir);
        w.write_u8(d.unused)
            .write_u8(d.mode)
            .write_u16(d.ammo_kind)
            .write_u8(d.dmg_kind)
            .write_u8(d.zone)
            .write_s8(d.part)
            .write_u32(d.damage);
    })
}

/// `GS_CHAT` `[sender_name:zstr16(32)][text:zstr16(376)][sender_id:u32][team_only:u8][whisper:u32]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.4 (cshell+0x4b30e..0x4b4cb)
pub fn chat(sender_name: &[u16], text: &[u16], sender_id: u32, team_only: bool) -> BitWriter {
    shell(GS_CHAT, |w| {
        zstr16n(w, sender_name, 32);
        zstr16n(w, text, 376);
        w.write_u32(sender_id)
            .write_u8(team_only as u8)
            .write_u32(0);
    })
}

// ------------------------------------------------------------------------------------------------- co-op (class 5)

/// `GS_COOP`: `[sub:u8]` then the sub's fields.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.5 row 0xd7 (cshell+0x2d2e4..0x2d4d0); FIRETEAM_2009.md §2.2
pub const GS_COOP: u8 = 0xd7;
pub const COOP_POINTS: u8 = 1;
pub const COOP_LIVES: u8 = 2;
pub const COOP_POINTS_NOTE: u8 = 3;
pub const COOP_CLOCK: u8 = 4;
/// `GS_COOP_MISSION`: `[sub:u8]` then the sub's fields.
// SPEC: GAME_MESSAGES_2009_S2C.md §6.2 (cshell+0x31490); FIRETEAM_2009.md §2.4
pub const GS_COOP_MISSION: u8 = 0xc9;
pub const MISSION_LIST: u8 = 1;
pub const MISSION_BANNER: u8 = 2;
pub const MISSION_CAPTION: u8 = 3;
pub const MISSION_PICKUP: u8 = 6;
/// `GS_COOP_MISSION` sub 6 `pickup` values: the HUD's drop-item note with the co-op layout's health, ammunition or
/// ammunition-box picture (and one of the three pickup sounds).
// DISASM: cshell+0x80090 (1 -> note object +0xfc, 2 -> +0xf4, 3 -> +0xf8, other values nothing), cshell+0x7f290 (the
// layout section's three drop-item picture keys load +0xf4 (ammunition), +0xf8 (ammunition box), +0xfc (health))
pub const PICKUP_NOTE_HEALTH: u8 = 1;
pub const PICKUP_NOTE_AMMO: u8 = 2;
pub const PICKUP_NOTE_AMMO_BOX: u8 = 3;
/// `GS_MUSIC` 0xd9: `[op:u8]`, op 0 stop, op 1 `[loop:bool1][fade:f32][file:zstr8(261)]` (the file relative to the
/// client's game folder).
// SPEC: GAME_MESSAGES_2009_S2C.md §4.2 row 0xd9 (cshell+0x5de30)
// DISASM: cshell+0x5de63..0x5def2 (`read(8)` op; op 1: `read(1)`, `read(32)`, a 0x105-byte string; the game folder
// prefixed; engine client slot +0x33c called with the path, the loop flag and the 32-bit value)
pub const GS_MUSIC: u8 = 0xd9;
pub const MUSIC_STOP: u8 = 0;
pub const MUSIC_PLAY: u8 = 1;
pub const MUSIC_FILE_BYTES: usize = 261;
/// `GS_OBJECT_NOTE` sub of a type-0x18 object with the NPC tail: the health fraction.
// SPEC: FIRETEAM_2009.md §1.5 (handler cshell+0x206260 case 0x24: read only when object +0x44 is set)
pub const NOTE_NPC_HEALTH: u8 = 0x24;
/// Text buffers of `GS_COOP_MISSION` and of the co-op result block.
// SPEC: GAME_MESSAGES_2009_S2C.md §6.2 (`zstr8(1000)`), §2.1.2 (`zstr8(260)`)
pub const MISSION_TEXT_BYTES: usize = 1000;
pub const RESULT_TEXT_BYTES: usize = 260;

/// `GS_COOP` sub 1 `[pid:u32][obj:objref16][x:u32][points:u32]`: the player's points total (record +0x1f4) and, for
/// the own pid, the popup with `x`.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.5; FIRETEAM_2009.md §2.2 row 1
pub fn coop_points(pid: u32, obj: Option<u16>, x: u32, points: u32) -> BitWriter {
    shell(GS_COOP, |w| {
        w.write_u8(COOP_POINTS)
            .write_u32(pid)
            .write_objref16(obj)
            .write_u32(x)
            .write_u32(points);
    })
}

/// `GS_COOP` sub 2 `[a:u8][b:u8]`: own record +0x1f8 := `b` (the lives counting the current one); a dead client
/// with `a == b` re-arms its respawn wait.
// SPEC: FIRETEAM_2009.md §2.2 row 2, re-arm (cshell+0x5df50), §2.3
pub fn coop_lives(a: u8, b: u8) -> BitWriter {
    shell(GS_COOP, |w| {
        w.write_u8(COOP_LIVES).write_u8(a).write_u8(b);
    })
}

/// `GS_COOP` sub 3 `[pid:u32][x:u8][points:u32]`: as sub 1 without the record write.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.5; FIRETEAM_2009.md §2.2 row 3
pub fn coop_points_note(pid: u32, x: u8, points: u32) -> BitWriter {
    shell(GS_COOP, |w| {
        w.write_u8(COOP_POINTS_NOTE)
            .write_u32(pid)
            .write_u8(x)
            .write_u32(points);
    })
}

/// `GS_COOP` sub 4 `[running:bool1][secs:u32]`: the round timer shown with `secs`, running or frozen.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.5; FIRETEAM_2009.md §2.2 row 4
pub fn coop_round_clock(running: bool, secs: u32) -> BitWriter {
    shell(GS_COOP, |w| {
        w.write_u8(COOP_CLOCK).write_bit(running).write_u32(secs);
    })
}

/// One goal of `GS_COOP_MISSION` sub 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissionGoal {
    pub id: u32,
    pub label: Vec<u8>,
    pub state: u8,
}

/// `GS_COOP_MISSION` sub 1 `[n:u32] n x [goal_id:u32][goal_label:zstr8(1000)][goal_state:u8] [m:u32] m x [timer:f32]
/// [a:u8][b:u8]`: the goal list, the mission timer (first positive timer), scoreboard `a` / `b` (in class 5 with
/// `a` > 0 the stage elements are shown).
// SPEC: GAME_MESSAGES_2009_S2C.md §6.2; FIRETEAM_2009.md §2.4 (`a` PLAUSIBLY the stage or wave, `b` the goal)
pub fn coop_mission_list(goals: &[MissionGoal], timers: &[f32], a: u8, b: u8) -> BitWriter {
    shell(GS_COOP_MISSION, |w| {
        w.write_u8(MISSION_LIST).write_u32(goals.len() as u32);
        for g in goals {
            w.write_u32(g.id);
            zstr8n(w, &g.label, MISSION_TEXT_BYTES);
            w.write_u8(g.state);
        }
        w.write_u32(timers.len() as u32);
        for t in timers {
            w.write_f32(*t);
        }
        w.write_u8(a).write_u8(b);
    })
}

/// `GS_COOP_MISSION` sub 6 `[pickup:u8]`: the drop-item note of the own HUD ([`PICKUP_NOTE_HEALTH`] ..
/// [`PICKUP_NOTE_AMMO_BOX`]).
// SPEC: GAME_MESSAGES_2009_S2C.md §6.2 (sub 6 `[pickup:u8]`, cshell+0x80090); FIRETEAM_2009.md row 0xc9 sub 6
pub fn coop_pickup_note(pickup: u8) -> BitWriter {
    shell(GS_COOP_MISSION, |w| {
        w.write_u8(MISSION_PICKUP).write_u8(pickup);
    })
}

/// `GS_COOP_MISSION` sub 2 `[text:zstr8(1000)][secs:f32]`: a banner text for `secs` with the mode sound.
// SPEC: GAME_MESSAGES_2009_S2C.md §6.2; FIRETEAM_2009.md §2.4
pub fn coop_banner(text: &[u8], secs: f32) -> BitWriter {
    shell(GS_COOP_MISSION, |w| {
        w.write_u8(MISSION_BANNER);
        zstr8n(w, text, MISSION_TEXT_BYTES);
        w.write_f32(secs);
    })
}

/// `GS_COOP_MISSION` sub 3 `[text:zstr8(1000)][secs:f32]`: the mission caption. The client splits `text` at `|`
/// into exactly three fields (it appends a last `|` itself): a picture, the speaker and the caption, which it types
/// out letter by letter and keeps for `secs` seconds once it is complete, with the mode sound; any other field count
/// shows nothing.
// SPEC: GAME_MESSAGES_2009_S2C.md §6.2 (sub 3 `[text][v]`, cshell+0x81fe0); FIRETEAM_2009.md §2.4 (the mission
// captions with a speaker in the co-op HUD layout)
// DISASM: cshell+0x31490 case 3 (`read_zstr(1000)`, `read(32)` passed on, then the mode sound); cshell+0x81fe0 (split at
// `|`, the caption only for three fields; the 32-bit value stored at +0xd4); cshell+0x81930 (+0xd4 read as a float of
// seconds: the caption stays `+0xd4 x 1000` ms after the last letter, then fades over 1000 ms)
pub fn coop_caption(text: &[u8], secs: f32) -> BitWriter {
    shell(GS_COOP_MISSION, |w| {
        w.write_u8(MISSION_CAPTION);
        // the client appends `|` to the 1000-byte buffer when the text lacks it: room for it
        zstr8n(w, text, MISSION_TEXT_BYTES - 1);
        w.write_f32(secs);
    })
}

/// `GS_MUSIC` op 1: play `file` (relative to the game folder), looping or once, fading in over `fade` seconds.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.2 row 0xd9
// DISASM: cshell+0x5de30 (op 1 read order: loop bit, 32-bit value, file); cshell+0x1fba00 (the shell's own music call
// passes the name, 1 and 0x3f800000 = 1.0 to the screen manager's music slot: PLAUSIBLY the same third argument, a fade in
// seconds as a float)
pub fn music_play(file: &[u8], looped: bool, fade: f32) -> BitWriter {
    shell(GS_MUSIC, |w| {
        w.write_u8(MUSIC_PLAY).write_bit(looped).write_f32(fade);
        zstr8n(w, file, MUSIC_FILE_BYTES);
    })
}

/// `GS_MUSIC` op 0: the music stops.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.2 row 0xd9 (op 0: engine client slot +0x340)
pub fn music_stop() -> BitWriter {
    shell(GS_MUSIC, |w| {
        w.write_u8(MUSIC_STOP);
    })
}

/// `GS_OBJECT_NOTE` `[0x0d][obj:objref16][1:u8][fov_x:f32][fov_y:f32][secs:f32]`: a world camera's field of view
/// (degrees) blends from its current one to the new one over `secs` (0: a cut).
// DISASM: cshell+0x20fd00 (note kind 0x0d: every camera of the camera list whose engine object is `obj` reads the rest
// through its slot +0x30); camera method table VA 0x374f04c4 slot +0x30 = cshell+0x216800 (sub 1: the old field of
// view kept at +0x4c / +0x50, the new one read into +0x44 / +0x48, the blend time into +0x54, elapsed +0x58 := 0);
// cshell+0x1f8aab..0x1f8b22 (each frame: elapsed += frame time; past the blend time the new value, else the linear
// blend)
pub fn camera_fov_note(obj: u16, fov: [f32; 2], secs: f32) -> BitWriter {
    shell(GS_OBJECT_NOTE, |w| {
        w.write_u8(crate::camshot::OBJ_CAMERA_TYPE)
            .write_u16(obj)
            .write_u8(crate::camshot::CAMERA_NOTE_FOV)
            .write_f32(fov[0])
            .write_f32(fov[1])
            .write_f32(secs);
    })
}

/// `GS_OBJECT_NOTE` `[0x18][obj:objref16][0x24][fraction:f32 bits]`: the NPC health bar of an object with the NPC
/// tail (never send it to a player character: the client then leaves the four bytes unread).
// SPEC: FIRETEAM_2009.md §1.5 (DISASM cshell+0x206548..0x206569)
pub fn npc_health_note(obj: u16, fraction: f32) -> BitWriter {
    shell(GS_OBJECT_NOTE, |w| {
        w.write_u8(crate::modelobj::BLOB_CHARACTER)
            .write_u16(obj)
            .write_u8(NOTE_NPC_HEALTH)
            .write_u32(fraction.to_bits());
    })
}

/// The health fraction sent for `health` of `max`: the client shows `trunc(fraction x 100)` percent, so a living NPC
/// gets `(p + 0.5) / 100` for its percent `p` (rounded up, at least 1) and a dead one 0.
// SPEC: FIRETEAM_2009.md §1.3 step 3, §1.5 (the conversion truncates toward zero)
// DC-DEFAULT: FT-W1 (proposed)
pub fn npc_health_fraction(health: f32, max: f32) -> f32 {
    if !(health.is_finite() && max.is_finite() && max > 0.0) || health <= 0.0 {
        return 0.0;
    }
    // a small tolerance keeps an exact percent (15 of 100 is 15.000001 in f32) from rounding up
    let p = (health * 100.0 / max - 1e-3).ceil().clamp(1.0, 100.0);
    (p + 0.5) / 100.0
}

/// The co-op block of `GS_ROUND_RESULT`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CoopResult {
    /// 2 = mission complete banner, 3 / 4 = failed, 5 = timed out (other values keep the previous banner).
    pub stage: u8,
    pub values: [u32; 6],
    /// Stage 2 only: the name list, title, text and the panel time.
    pub names: Vec<Vec<u8>>,
    pub title: Vec<u8>,
    pub text: Vec<u8>,
    pub panel_ms: u32,
}

/// `GS_ROUND_RESULT` of the co-op class: `[round_secs:u32][win_team:i8][wins_team1:u8][wins_team2:u8][stage:u8]
/// [c1..c6:u32]`, for `stage` 2 then `[n:u32] n x [name:zstr8(260)][title:zstr8(260)][text:zstr8(260)]
/// [panel_ms:u32]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1.2 (cshell+0x4a734..0x4a780); FIRETEAM_2009.md §2.6 (stage banners)
pub fn round_result_coop(
    round_secs: u32,
    win_team: i8,
    wins: [u32; 2],
    c: &CoopResult,
) -> BitWriter {
    shell(GS_ROUND_RESULT, |w| {
        w.write_u32(round_secs).write_s8(win_team);
        w.write_u8(clamp_u8(wins[0])).write_u8(clamp_u8(wins[1]));
        w.write_u8(c.stage);
        for v in c.values {
            w.write_u32(v);
        }
        if c.stage == 2 {
            w.write_u32(c.names.len() as u32);
            for n in &c.names {
                zstr8n(w, n, RESULT_TEXT_BYTES);
            }
            zstr8n(w, &c.title, RESULT_TEXT_BYTES);
            zstr8n(w, &c.text, RESULT_TEXT_BYTES);
            w.write_u32(c.panel_ms);
        }
    })
}

#[cfg(test)]
mod coop_tests {
    use super::*;

    fn body(m: &BitWriter) -> dc_wire::udp::BitReader<'_> {
        let mut r = m.reader();
        assert_eq!(r.read_u8(), 13);
        r
    }

    #[test]
    fn coop_forms_bit_exact() {
        let m = coop_lives(3, 3);
        let mut r = body(&m);
        assert_eq!(
            (r.read_u8(), r.read_u8(), r.read_u8(), r.read_u8()),
            (GS_COOP, 2, 3, 3)
        );
        assert!(r.at_end());
        let m = coop_points(5, Some(300), 40, 120);
        let mut r = body(&m);
        assert_eq!((r.read_u8(), r.read_u8()), (GS_COOP, 1));
        assert_eq!(
            (r.read_u32(), r.read_u16(), r.read_u32(), r.read_u32()),
            (5, 300, 40, 120)
        );
        assert!(r.at_end());
        let m = coop_points_note(5, 10, 130);
        let mut r = body(&m);
        assert_eq!(
            (
                r.read_u8(),
                r.read_u8(),
                r.read_u32(),
                r.read_u8(),
                r.read_u32()
            ),
            (GS_COOP, 3, 5, 10, 130)
        );
        assert!(r.at_end());
        let m = coop_round_clock(true, 600);
        let mut r = body(&m);
        assert_eq!(
            (r.read_u8(), r.read_u8(), r.read_bit(), r.read_u32()),
            (GS_COOP, 4, true, 600)
        );
        assert!(r.at_end());
        let m = npc_health_note(301, npc_health_fraction(29.0, 100.0));
        let mut r = body(&m);
        assert_eq!(
            (r.read_u8(), r.read_u8(), r.read_u16(), r.read_u8()),
            (GS_OBJECT_NOTE, 0x18, 301, 0x24)
        );
        let f = f32::from_bits(r.read_u32());
        // the client's truncation of fraction x 100 shows the intended percent
        assert_eq!((f as f64 * 100.0) as i32, 29);
        assert!(r.at_end());
    }

    #[test]
    fn health_fraction_survives_the_truncation() {
        // the client multiplies on the x87 stack (extended precision) and truncates toward zero
        let shown = |f: f32| (f as f64 * 100.0) as u32;
        for p in 1..=100u32 {
            assert_eq!(
                shown(npc_health_fraction(p as f32, 100.0)),
                p,
                "percent {p}"
            );
            assert_eq!(
                shown(npc_health_fraction(p as f32 * 2.5, 250.0)),
                p,
                "percent {p} of 250"
            );
        }
        assert_eq!(npc_health_fraction(0.0, 100.0), 0.0);
        assert_eq!(npc_health_fraction(-3.0, 100.0), 0.0);
        // a sliver of health still shows 1 %
        assert_eq!(shown(npc_health_fraction(0.01, 250.0)), 1);
        // a plain fraction 0.29 would show 28 (the trap of FIRETEAM_2009.md §1.5)
        assert_eq!(shown(0.29), 28);
    }

    #[test]
    fn caption_music_and_camera_fov_forms() {
        // DSV-579: 0xc9 sub 3, the three-field caption and its seconds as f32
        let m = coop_caption(b"pic.dtx|-Speaker-| Hold on.", 10.0);
        let mut r = body(&m);
        assert_eq!(
            (r.read_u8(), r.read_u8(), r.read_zstr8(), r.read_f32()),
            (
                GS_COOP_MISSION,
                MISSION_CAPTION,
                b"pic.dtx|-Speaker-| Hold on.".to_vec(),
                10.0
            )
        );
        assert!(r.at_end());
        // the client appends `|` in its 1000-byte buffer: at most 998 bytes go out
        let long = vec![b'a'; 1200];
        let m = coop_caption(&long, 1.0);
        let mut r = body(&m);
        r.read_u8();
        r.read_u8();
        assert_eq!(r.read_zstr8().len(), MISSION_TEXT_BYTES - 2);
        // DSV-581: 0xd9 op 1 `[loop:bool1][fade:f32][file]`, op 0 alone
        let m = music_play(b"BGM/x.mp3", true, 4.0);
        let mut r = body(&m);
        assert_eq!(
            (
                r.read_u8(),
                r.read_u8(),
                r.read_bit(),
                r.read_f32(),
                r.read_zstr8()
            ),
            (GS_MUSIC, MUSIC_PLAY, true, 4.0, b"BGM/x.mp3".to_vec())
        );
        assert!(r.at_end());
        let m = music_stop();
        let mut r = body(&m);
        assert_eq!((r.read_u8(), r.read_u8()), (GS_MUSIC, MUSIC_STOP));
        assert!(r.at_end());
        // DSV-580: 0xa6 note kind 0x0d, sub 1, two degrees and the blend seconds
        let m = camera_fov_note(77, [100.0, 84.0], 0.0);
        let mut r = body(&m);
        assert_eq!(
            (r.read_u8(), r.read_u8(), r.read_u16(), r.read_u8()),
            (GS_OBJECT_NOTE, 0x0d, 77, 1)
        );
        assert_eq!(
            (r.read_f32(), r.read_f32(), r.read_f32()),
            (100.0, 84.0, 0.0)
        );
        assert!(r.at_end());
    }

    #[test]
    fn mission_and_result_forms() {
        let goals = [MissionGoal {
            id: 1,
            label: b"wave".to_vec(),
            state: 0,
        }];
        let m = coop_mission_list(&goals, &[300.0], 2, 1);
        let mut r = body(&m);
        assert_eq!(
            (r.read_u8(), r.read_u8(), r.read_u32()),
            (GS_COOP_MISSION, 1, 1)
        );
        assert_eq!(
            (r.read_u32(), r.read_zstr8(), r.read_u8()),
            (1, b"wave".to_vec(), 0)
        );
        assert_eq!((r.read_u32(), r.read_f32()), (1, 300.0));
        assert_eq!((r.read_u8(), r.read_u8()), (2, 1));
        assert!(r.at_end());
        let m = coop_banner(b"Wave 2", 3.0);
        let mut r = body(&m);
        assert_eq!(
            (r.read_u8(), r.read_u8(), r.read_zstr8(), r.read_f32()),
            (GS_COOP_MISSION, 2, b"Wave 2".to_vec(), 3.0)
        );
        assert!(r.at_end());
        // stage 3: six values and nothing more; stage 2: the name list, title, text and panel time
        let c = CoopResult {
            stage: 3,
            values: [1, 2, 3, 4, 5, 6],
            ..CoopResult::default()
        };
        assert_eq!(
            round_result_coop(3, 2, [0, 0], &c).bit_len(),
            16 + 32 + 8 + 16 + 8 + 6 * 32
        );
        let c = CoopResult {
            stage: 2,
            names: vec![b"A".to_vec()],
            panel_ms: 4000,
            ..c
        };
        let m = round_result_coop(3, 1, [0, 0], &c);
        let mut r = body(&m);
        assert_eq!(r.read_u8(), GS_ROUND_RESULT);
        r.skip_bits(32 + 8 + 16);
        assert_eq!(r.read_u8(), 2);
        r.skip_bits(6 * 32);
        assert_eq!((r.read_u32(), r.read_zstr8()), (1, b"A".to_vec()));
        assert_eq!(
            (r.read_zstr8(), r.read_zstr8(), r.read_u32()),
            (vec![], vec![], 4000)
        );
        assert!(r.at_end());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The HiSec case messages equal the reference stack's bytes on the real client's link of tdm6 (seq 3006, 4833, 10688,
    /// 3034, 4833).
    // OBSERVED: capture 20261025_065016_tdm6 c6 (REFERENCE, accepted by the real client)
    #[test]
    fn hisec_case_messages_have_the_observed_bytes() {
        let hx = |m: &BitWriter| {
            m.as_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        assert_eq!(hx(&hisec_dropped()), "0dd600");
        assert_eq!(hisec_dropped().bit_len(), 24);
        assert_eq!(hx(&hisec_reserve(1, Some(0))), "0dd6020100000000000000");
        assert_eq!(hx(&hisec_reserve(1, None)), "0dd60201000000ffffffff");
        assert_eq!(hx(&hisec_taken(2, 1)), "0dd6010200000001000000");
        assert_eq!(hisec_taken(2, 1).bit_len(), 88);
        assert_eq!(hx(&hisec_countdown(0x0102, 15)), "0da6170201020f000000");
        assert_eq!(hx(&hisec_countdown(0x0102, 0)), "0da61702010200000000");
        assert_eq!(hisec_countdown(0x0102, 0).bit_len(), 80);
    }

    // OBSERVED: capture 20261025_065016_tdm6 seq 10134 / 11052, 20261025_002236_tdm4 seq 3135
    #[test]
    fn death_note_has_the_reference_bytes() {
        let hx = |m: &BitWriter| {
            m.as_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        assert_eq!(hx(&death_note(252)), "0da618fc001d");
        assert_eq!(hx(&death_note(140)), "0da6188c001d");
        assert_eq!(hx(&death_note(408)), "0da61898011d");
        assert_eq!(death_note(408).bit_len(), 48);
    }

    /// DSV-63: the spawn notes equal the reference stack's bytes bit for bit (tdm6 c6 seq 733 / 1162, character 0x8c).
    // OBSERVED: capture 20261025_065016_tdm6 c6 (REFERENCE, accepted by the real client)
    #[test]
    fn spawn_notes_have_the_observed_bytes() {
        let hx = |m: &BitWriter| {
            m.as_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        assert_eq!(hx(&spawn_note(0x8c, NOTE_SPAWN_START)), "0da6188c001f");
        assert_eq!(hx(&spawn_note(0x8c, NOTE_SPAWN_END)), "0da6188c0025");
        assert_eq!(spawn_note(0x8c, NOTE_SPAWN_END).bit_len(), 48);
    }

    // DC-DEFAULT: DSV-90 (proposed); OBSERVED: rust6 / tdm6 looks (ae-triage-reddot2-blast §1.3)
    #[test]
    fn look_is_sanitised() {
        let zero = Look {
            bytes: [0; 15],
            text_a: Vec::new(),
            text_b: Vec::new(),
        };
        assert_eq!(zero.sanitised().bytes, DEFAULT_LOOK_BYTES);
        // the real client's own look (rust6 conn 5) passes unchanged
        let user = Look {
            bytes: [0, 0, 0, 0, 18, 1, 0, 0, 0, 1, 1, -1, 1, 0, -1],
            text_a: b"a".to_vec(),
            text_b: b"b".to_vec(),
        };
        assert_eq!(user.sanitised(), user);
        // a look with override row 0 loses it, the rest stays
        let mut row0 = user.clone();
        row0.bytes[LOOK_OVERRIDE_BYTE] = 0;
        assert_eq!(row0.sanitised(), user);
        let mut row7 = user.clone();
        row7.bytes[LOOK_OVERRIDE_BYTE] = 7;
        assert_eq!(row7.sanitised().bytes[LOOK_OVERRIDE_BYTE], -1);
    }

    fn body(m: &BitWriter) -> (u8, u8, dc_wire::udp::BitReader<'_>) {
        let mut r = m.reader();
        let id = r.read_u8();
        let shell = r.read_u8();
        (id, shell, r)
    }

    /// Wave AA item 7: the second head bit of 0x45 latches the client's "waiting for players to connect" banner.
    /// rust4 c5 (t=104.828, s2c seq 807-811) carried `0d 45 00000000 02 ..` (114 bits: banner set); every reference
    /// 0x45 carried byte 6 = `01` (intro, banner clear: tdm4 c6 seq 815-834, tdm2 c6, tdm1 c5, menu-frida6 c3).
    // PROVEN-CAPTURE: rust4 / tdm1 / tdm2 / tdm4 / menu-frida6 0x45 heads (aa-triage-start-state §2.1)
    #[test]
    fn match_begin_waiting_banner_bit() {
        let off = MatchBegin::default();
        let w = match_begin(&off, 1);
        assert_eq!(w.bit_len(), 114, "the rust4 0x45 length without the intro");
        assert_eq!(
            &w.as_bytes()[..7],
            [0x0D, 0x45, 0, 0, 0, 0, 0x00],
            "banner clear"
        );
        let on = MatchBegin {
            waiting_banner: true,
            ..MatchBegin::default()
        };
        assert_eq!(
            &match_begin(&on, 1).as_bytes()[..7],
            [0x0D, 0x45, 0, 0, 0, 0, 0x02],
            "the rust4 head byte (banner set)"
        );
        let reference = MatchBegin {
            intro: Some(Intro::default()),
            ..MatchBegin::default()
        };
        assert_eq!(
            match_begin(&reference, 1).as_bytes()[6] & 0x03,
            0x01,
            "the reference stack form: intro, no banner"
        );
    }

    #[test]
    fn s12_and_switch_bytes() {
        // SPEC: UDP_SESSION_2009_A_addendum_disasm.md A17 (S12 `0D 67 01`)
        assert_eq!(player_state(STATE_ALIVE).as_bytes(), [0x0D, 0x67, 0x01]);
        assert_eq!(player_state(STATE_WAITING).as_bytes(), [0x0D, 0x67, 0x03]);
        assert_eq!(identify_request().as_bytes(), [0x0D, 0x72]);
    }

    #[test]
    fn mover_teleport_layout() {
        let m = mover_teleport(7, [1.0, 2.0, 3.0], [4.0, 5.0, 6.0], None);
        let (id, s, mut r) = body(&m);
        assert_eq!((id, s), (13, GS_MOVER_UPDATE));
        assert_eq!(r.read_u16(), MOVER_TELEPORT);
        assert_eq!(r.read_u8(), 7);
        let v: Vec<f32> = (0..6).map(|_| r.read_f32()).collect();
        assert_eq!(v, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert!(r.at_end());
        let b = BodySpec {
            model: b"m".to_vec(),
            skins: vec![b"s".to_vec()],
            passable: true,
        };
        let m = mover_teleport(1, [0.0; 3], [1.0; 3], Some(&b));
        let (_, _, mut r) = body(&m);
        assert_eq!(r.read_u16(), MOVER_TELEPORT | MOVER_BODY);
        r.skip_bits(8 + 192);
        assert_eq!(r.read_zstr8(), b"m");
        assert_eq!(r.read_u8(), 1);
        assert_eq!(r.read_zstr8(), b"s");
        assert!(r.read_bit());
        assert!(r.at_end());
    }

    #[test]
    fn slice3_writers_match_the_reference_bytes() {
        // UDP_REFERENCE_FINDINGS_botclient.md reference run: entry bytes after the shell id (REFERENCE)
        let body = |m: &BitWriter| m.as_bytes()[2..].to_vec();
        assert_eq!(
            body(&weapon_switch(0x69, false, -1.0, 0)),
            [0x69, 0, 0, 0, 0x80, 0xbf, 0]
        );
        assert_eq!(weapon_switch(0x69, true, -1.0, 0).bit_len(), 16 + 56);
        assert_eq!(scores_clear().bit_len(), 16);
        let rb = round_begin(0, 1, 1, false);
        assert_eq!(rb.bit_len(), 16 + 97);
        assert_eq!(body(&rb)[..12], [0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(shell_flag_90(true).as_bytes(), [0x0d, 0x90, 0x01]);
        assert_eq!(shell_flag_90(false).bit_len(), 17);
        assert_eq!(game_endpoint(0, b"h", 1).bit_len(), 24);
        assert_eq!(game_endpoint(1, b"h", 2).bit_len(), 24 + 16 + 32);
        assert_eq!(
            body(&player_status_b(STATUS_RESET, 1, 0, 0.0)),
            [0x13, 1, 0, 0, 0, 0, 0]
        );
        // the reference stack's first mover update: containers + friction, body, friction (flags 0x000d)
        let b = BodySpec {
            model: b"M\\B.LTB".to_vec(),
            skins: vec![b"T\\U.DTX".to_vec(), b"T\\L.DTX".to_vec()],
            passable: false,
        };
        let m = mover_update(&MoverUpdate {
            containers_friction: Some(FRICTION_NO_CONTAINER),
            body: Some(&b),
            friction: Some(FRICTION_NO_CONTAINER),
            ..MoverUpdate::default()
        });
        let mut r = m.reader();
        r.skip_bits(16);
        assert_eq!(r.read_u16(), 0x000d);
        assert_eq!((r.read_u8(), r.read_f32()), (0, 5.0));
        assert_eq!(r.read_zstr8(), b.model);
        assert_eq!(r.read_u8(), 2);
        r.read_zstr8();
        r.read_zstr8();
        assert!(!r.read_bit());
        assert_eq!(r.read_f32(), 5.0);
        assert!(r.at_end());
        // freeze 1 = `0d a0 80 00 01` (17 bits after the id)
        let f = mover_update(&MoverUpdate {
            freeze: Some(true),
            ..MoverUpdate::default()
        });
        assert_eq!(f.bit_len(), 16 + 17);
        assert_eq!(&f.as_bytes()[2..], [0x80, 0x00, 0x01]);
        // the teleport block alone equals the slice-2 writer
        assert_eq!(
            mover_update(&MoverUpdate {
                teleport: Some((1, [1.0; 3], [2.0; 3])),
                ..MoverUpdate::default()
            }),
            mover_teleport(1, [1.0; 3], [2.0; 3], None)
        );
    }

    #[test]
    fn relay_writers_match_the_capture_forms() {
        // CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.5 head `11 19 03 91 00 01 00 00 00 ..` (168 bits)
        let s = sound_effect(3, 0x91, 1, [1.0, 2.0, 3.0]);
        assert_eq!(&s.as_bytes()[..9], [0x11, 0x19, 3, 0x91, 0, 1, 0, 0, 0]);
        assert_eq!(s.bit_len(), 168, "id 17 + type 0x19 + 160 bits");
        // §3.3: 8 + 416 bits after the id byte, head `11 06 ff ff bd 00 ..`
        let e = shot_effect(&ShotEffect {
            first: None,
            shooter: 0xbd,
            weapon: 0,
            ammo: 0,
            from: [0.0; 3],
            to: [0.0; 3],
            normal: [0.0, 1.0, 0.0],
            surface: TODO_SURFACE_WORLD,
            mask: SHOT_MASK_MISS,
            client: 1,
            mods: NO_SHOT_MODS,
        });
        assert_eq!(e.bit_len(), 8 + 8 + 416);
        assert_eq!(&e.as_bytes()[..4], [0x11, 0x06, 0xff, 0xff]);
        assert_eq!(sprint_allowed(true).as_bytes(), [0x0d, 0xca, 0x01]);
        let blob = crate::modelobj::CharacterBlob::reference(1, [15.0, 50.0, 15.0]).write();
        // 0xa6 sub 0x11: 8 type + 16 obj + 8 sub + 572 record bits after the shell id (604, as the reference stack)
        assert_eq!(character_record_note(252, &blob).bit_len(), 16 + 604);
    }

    #[test]
    fn player_info_bit_count_and_order() {
        let p = PlayerInfo {
            name: b"Dave".to_vec(),
            pid: 3,
            account_no: 7,
            character: Some(2),
            look: Look::default(),
            kills: 5,
            deaths: 1,
            team: 2,
            dead: false,
            kind: INFO_REFRESH,
            mode_m1: 0,
            mission: [0; 3],
            mode_m2: 0,
            mark: PlayerMark::default(),
        };
        let m = player_info(&p);
        let (_, s, mut r) = body(&m);
        assert_eq!(s, GS_PLAYER_INFO);
        assert_eq!(r.read_zstr8(), b"Dave");
        assert_eq!(r.read_u8(), 3);
        assert_eq!(r.read_u32(), 7);
        assert!(!r.read_bit());
        assert_eq!(r.read_u16(), 2);
        r.skip_bits(15 * 8);
        assert_eq!(r.read_zstr8(), b"");
        assert_eq!(r.read_zstr8(), b"");
        assert_eq!(r.read_s16(), 5);
        assert_eq!(r.read_u8(), 1);
        r.skip_bits(24);
        assert_eq!(r.read_u8(), 2);
        assert!(!r.read_bit());
        assert_eq!(r.read_u8(), INFO_REFRESH);
        assert!(!r.read_bit());
        // h1 h2, e1 e2 e3, unused, m1 m2 m3 m4
        assert_eq!(r.remaining_bits(), 64 + 3 + 8 + 8 + 32 + 32 + 16);
    }

    #[test]
    fn player_info_with_rank_and_clan_block() {
        // SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 fields 15-17 (has_group, group block, h1, h2)
        let p = PlayerInfo {
            name: b"Mo".to_vec(),
            pid: 1,
            account_no: 9,
            character: Some(4),
            look: Look::default(),
            kills: 0,
            deaths: 0,
            team: 1,
            dead: false,
            kind: INFO_UPDATE,
            mode_m1: 0,
            mission: [0; 3],
            mode_m2: 0,
            mark: PlayerMark::default(),
        };
        let plain = player_info(&p);
        let mark = PlayerMark {
            rank: 17,
            name_colour: 0,
            clan: Some(ClanMark {
                name: "Owls".encode_utf16().collect(),
                group_no: 77,
                mark: [5, 9],
                name_colour: 0,
            }),
        };
        let m = player_info_marked(&p, &mark);
        // the group block adds the name (4 units + terminator), u64, 3 x u32
        assert_eq!(m.bit_len(), plain.bit_len() + 5 * 16 + 64 + 3 * 32);
        assert_eq!(
            player_info_marked(&p, &PlayerMark::default()).as_bytes(),
            plain.as_bytes()
        );
        let (_, s, mut r) = body(&m);
        assert_eq!(s, GS_PLAYER_INFO);
        // the same fixed head as the plain form up to `kind`
        let head = plain.bit_len() - 16 - (1 + 64 + 3 + 8 + 8 + 32 + 32 + 16);
        r.skip_bits(head);
        assert!(r.read_bit(), "has_group");
        assert_eq!(String::from_utf16_lossy(&r.read_zstr16()), "Owls");
        assert_eq!(r.read_u64(), 77);
        assert_eq!((r.read_u32(), r.read_u32(), r.read_u32()), (5, 9, 0));
        assert_eq!((r.read_u32(), r.read_u32()), (0, 17), "h1 colour, h2 rank");
        assert_eq!(r.remaining_bits(), 3 + 8 + 8 + 32 + 32 + 16);
        // a 200-unit clan name is cut to 127 units
        let long = PlayerMark {
            clan: Some(ClanMark {
                name: vec![b'x' as u16; 200],
                ..ClanMark::default()
            }),
            ..PlayerMark::default()
        };
        let lm = player_info_marked(&p, &long);
        let (_, _, mut r) = body(&lm);
        r.skip_bits(head + 1);
        assert_eq!(r.read_zstr16().len(), 127);
    }

    #[test]
    fn player_mark_from_the_roster_tokens() {
        // DC-DEFAULT: DSV-141 (proposed)
        let e = match crate::pipe::parse_control(
            "user 9 5 1 0 3570 Mo rank=4 clan=77 clanlabel=Night%20Owls mark=5/9",
        ) {
            crate::pipe::Control::User(u) => u,
            other => panic!("{other:?}"),
        };
        let m = PlayerMark::of_roster(Some(&e));
        assert_eq!(m.rank, 4);
        let c = m.clan.unwrap();
        assert_eq!(String::from_utf16_lossy(&c.name), "Night Owls");
        assert_eq!((c.group_no, c.mark, c.name_colour), (77, [5, 9], 0));
        assert_eq!(PlayerMark::of_roster(None), PlayerMark::default());
    }

    /// DSV-606: the lobby's colour tokens reach `h1` / `g3` of the 0x6b record, as the client's roster reader (the
    /// scoreboard's and the death panel's source) reads them.
    #[test]
    fn colour_tokens_reach_the_clients_roster_record() {
        // DC-DEFAULT: DSV-606 (proposed)
        let e = match crate::pipe::parse_control(
            "user 9 5 1 0 0 Mo clan=77 clanlabel=Owls mark=5/9 namecolour=4278255360 clancolour=4294901760",
        ) {
            crate::pipe::Control::User(u) => u,
            other => panic!("{other:?}"),
        };
        let p = PlayerInfo {
            name: b"Mo".to_vec(),
            pid: 1,
            account_no: 9,
            character: Some(4),
            look: Look::default(),
            kills: 0,
            deaths: 0,
            team: 1,
            dead: false,
            kind: INFO_UPDATE,
            mode_m1: 0,
            mission: [0; 3],
            mode_m2: 0,
            mark: PlayerMark::of_roster(Some(&e)),
        };
        let m = player_info(&p);
        let (_, s, mut r) = body(&m);
        assert_eq!(s, GS_PLAYER_INFO);
        let rec = dc_botclient::shell::read_player_info(&mut r);
        assert!(rec.complete);
        assert_eq!(rec.name_colour, 0xff00_ff00);
        assert_eq!(
            rec.clan.map(|c| (c.name, c.colour)),
            Some(("Owls".into(), 0xffff_0000))
        );
    }

    #[test]
    fn kill_and_damage_layouts() {
        let k = kill(&Kill {
            victim: 1,
            killer: 0,
            weapon: 0x0102,
            hit_zone: KILL_ZONE_HEAD,
            ..Kill::default()
        });
        let (_, s, mut r) = body(&k);
        assert_eq!(s, GS_KILL);
        assert_eq!((r.read_u8(), r.read_u8(), r.read_u8()), (1, 0, 0));
        assert_eq!(r.read_u16(), 0x0102);
        assert_eq!(r.read_u8(), 1);
        // 4 bits, 2 bytes, 3 bits, 2 words
        assert_eq!(r.remaining_bits(), 4 + 16 + 3 + 64);

        let d = damage_taken(&Damage {
            from_dir: [0.0, 0.0, 1.0],
            ammo_kind: 9,
            damage: 25,
            part: 0,
            ..Damage::default()
        });
        let (_, s, r) = body(&d);
        assert_eq!(s, GS_DAMAGE_TAKEN);
        assert_eq!(r.remaining_bits(), 96 + 8 + 8 + 16 + 8 + 8 + 8 + 32);
    }

    /// DSV-33 / DC-KILL-2: our writers produce the reference stack's 0x6d and the 0x74 tail byte for byte (heads of
    /// CAPTURE_tdm4_KILL_VALIDATION.md §3.1 / §3.2, REFERENCE forms the real client accepted).
    #[test]
    fn kill_and_damage_match_the_observed_forms() {
        let hex = |m: &BitWriter| {
            m.as_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        let k = kill(&Kill {
            victim: 0,
            killer: 1,
            kill_kind: 1,
            weapon: 145,
            hit_zone: 3,
            streak_flag: 1,
            feed_value: 2,
            ..Kill::default()
        });
        assert_eq!(k.bit_len(), 151);
        assert_eq!(hex(&k), "0d6d0001019100031020000000000000000000");
        let k = kill(&Kill {
            victim: 0,
            killer: 1,
            kill_kind: 1,
            weapon: 145,
            hit_zone: 3,
            streak_flag: 0,
            feed_value: 2,
            ..Kill::default()
        });
        assert_eq!(hex(&k), "0d6d0001019100030020000000000000000000");
        let d = damage_taken(&Damage {
            from_dir: [0.2314, -0.0035, 0.1347],
            unused: 2,
            mode: 1,
            ammo_kind: 0xFFFF,
            dmg_kind: 3,
            zone: 2,
            part: 0,
            damage: 27,
        });
        assert_eq!(&hex(&d)[2 * 14..], "0201ffff0302001b000000");
        assert_eq!(
            hex(&round_wins([1, 0])),
            "0db50100",
            "team kills after the first kill"
        );
        assert_eq!(hex(&round_objects_end(0)), "0d4700000000");
        assert_eq!(hex(&round_close(3)), "0d4803000000");
        assert_eq!(hex(&shell_9d(0)), "0d9d00000000");
    }

    #[test]
    fn mode_dependent_layouts() {
        let s = MatchSettings {
            own_team: 1,
            goal: 300,
            round_limit: 1,
            ..MatchSettings::default()
        };
        let a = match_settings(&s, 1);
        let b = match_settings(&s, MODE_CLASS_7);
        assert_eq!(b.bit_len(), a.bit_len() + 32);
        let (_, _, mut r) = body(&a);
        assert_eq!(r.read_zstr8(), b"");
        r.skip_bits(8);
        assert_eq!(r.read_s8(), 1);
        r.skip_bits(32);
        assert_eq!(r.read_u8(), 255, "goal clamped to u8");
        // DSV-191: the carrier records of a flag room follow the count
        let c = MatchSettings {
            carriers: vec![[2, 4, 1]],
            ..s.clone()
        };
        let cb = match_settings(&c, MODE_CLASS_7);
        assert_eq!(cb.bit_len(), b.bit_len() + 96);
        let (_, _, mut r) = body(&cb);
        r.read_zstr8();
        r.skip_bits(8 + 8 + 32 + 8 * 5);
        assert_eq!(
            [r.read_u32(), r.read_u32(), r.read_u32(), r.read_u32()],
            [1, 2, 4, 1]
        );
        // DSV-190: 0xc6 field order, the two bits last
        let o = objective_state(&ObjectiveState {
            obj: 0x1234,
            state: 2,
            flag_team: 1,
            carrier: 3,
            actor: 3,
            event: 2,
            at_base: false,
            quiet: true,
        });
        let (_, id, mut r) = body(&o);
        assert_eq!(id, GS_OBJECTIVE_STATE);
        assert_eq!(r.read_u16(), 0x1234);
        assert_eq!(
            [
                r.read_u32(),
                r.read_u32(),
                r.read_u32(),
                r.read_u32(),
                r.read_u32()
            ],
            [2, 1, 3, 3, 2]
        );
        assert!(!r.read_bit());
        assert!(r.read_bit());

        let m = MatchBegin {
            round_secs: 600,
            waiting_banner: true,
            intro: None,
            round_no: 0,
            countdown_secs: 0,
        };
        let w = match_begin(&m, 1);
        let (_, _, mut r) = body(&w);
        assert_eq!(r.read_u32(), 600);
        assert!(!r.read_bit());
        assert!(r.read_bit());
        assert_eq!(r.read_u32(), 0);
        assert_eq!(r.read_u32(), 0);
        assert!(r.at_end());
        let with = MatchBegin {
            intro: Some(Intro::default()),
            ..m
        };
        let tdm = match_begin(&with, 1).bit_len();
        let six = match_begin(&with, MODE_CLASS_6).bit_len();
        let nine = match_begin(&with, MODE_CLASS_9).bit_len();
        assert_eq!(tdm + 8 - six, 192, "class 6: caption, no team bases");
        assert_eq!(nine - tdm, 8, "empty mode caption for class 9");

        assert_eq!(round_result(0, 1, [3, 1], 1).bit_len(), 16 + 32 + 24);
        assert_eq!(
            round_result(0, 1, [3, 1], MODE_COOP).bit_len(),
            16 + 32 + 24 + 8 + 192
        );
        let sm = Summary::default();
        assert_eq!(player_summary(&sm, 1).bit_len(), 16 + 8 + 16 + 160 + 1);
    }

    #[test]
    fn ping_table_and_chat() {
        let p = ping_table(&[(0, 40), (3, 120)]);
        let (_, _, mut r) = body(&p);
        assert_eq!((r.read_u16(), r.read_u16()), (0, 40));
        assert_eq!((r.read_u16(), r.read_u16()), (3, 120));
        assert_eq!(r.read_u16(), PING_TABLE_END);
        assert!(r.at_end());
        let name: Vec<u16> = "Dave".encode_utf16().collect();
        let text: Vec<u16> = std::iter::repeat_n(b'x' as u16, 500).collect();
        let c = chat(&name, &text, 2, true);
        let (_, _, mut r) = body(&c);
        assert_eq!(String::from_utf16_lossy(&r.read_zstr16()), "Dave");
        assert_eq!(
            r.read_zstr16().len(),
            375,
            "text cut to the client's buffer"
        );
        assert_eq!((r.read_u32(), r.read_u8(), r.read_u32()), (2, 1, 0));
    }

    /// DSV-80: the part bytes of `GS_WEAPON_GIVE` / `GS_WEAPON_EQUIP` are written in order. With -1 x 4 the grant
    /// equals our rust6 grant of the L96A1 bit for bit; with the scope mod 27 in part 1 only that byte changes.
    // OBSERVED: capture 20261025_125432_rust6 c5 seq 810 (`0db21b001b000000a0410104ffffffff`)
    // DISASM: cshell+0x60d90 (four i8 parts after the slot)
    #[test]
    fn weapon_grants_carry_the_part_bytes() {
        let hx = |m: &BitWriter| {
            m.as_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        let mut s = WeaponSpec {
            weapon: 27,
            ammo_kind: 27,
            ammo: 20.0,
            clip: 5,
            class_pos: 1,
            slot: 4,
            parts: NO_PARTS,
        };
        assert_eq!(hx(&weapon_give(&s)), "0db21b001b000000a0410104ffffffff");
        s.parts = [27, NO_PART, NO_PART, NO_PART];
        assert_eq!(hx(&weapon_give(&s)), "0db21b001b000000a04101041bffffff");
        assert_eq!(weapon_give(&s).bit_len(), 128);
        assert_eq!(
            hx(&drawn_weapon(&s)),
            "0db31b001b000000a041050000000104 1bffffff".replace(' ', "")
        );
        s.parts = [1, 2, 3, 4];
        assert!(hx(&weapon_give(&s)).ends_with("01020304"));
    }
}
