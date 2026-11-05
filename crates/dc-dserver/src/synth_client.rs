//! A synthetic client that follows the client-side halves of the same specs (our own test tooling, not a model
//! of the real 2009 client's internals). It joins, sends `C2S_HELLO`, answers the file list, "loads" the world
//! after a delay during which it processes no session message (spec A §4.6: the load runs inside the handler),
//! answers the preload, tracks its object, the view object, the switching-worlds state and the stream time.
//!
//! Slice 2: it plays the game layer as the 2009 client does: answers `GS_IDENTIFY_REQUEST` with `UP_PLAYER_INFO`
//! and `UP_CLIENT_OPTIONS`, runs the full in-world test (condition d = a non-zero player state, `GS_PLAYER_STATE`),
//! sends `UP_WORLD_READY` once, applies the teleport / angles / status / weapon messages, sends `UP_PLAYER_UPDATE`
//! only in player state 1 (with the sequence of the last teleport), reads the roster, kill feed, scores, round
//! result and the remote objects of the object stream, and can fire (`UP_FIRE`), chat, leave and ask for a respawn.
//!
//! Its message readers and writers are written here from the client-side spec sections, independently of the
//! server's writers ([`crate::msg`], [`crate::gs`]) and readers ([`crate::up`]), so a symmetric codec mistake shows
//! up as a test failure.

use std::collections::{BTreeMap, VecDeque};

/// The cone inputs of [`SynthClient::fire_cone_at`]: spread byte, shot scale, zoom bit, the half-width the row gives
/// for them, and whether the direction is forged (the forward axis instead of the cone's).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ConeShot {
    pub frac: u8,
    pub scale: f32,
    pub zoom: bool,
    pub h: i64,
    pub forged: bool,
}

/// The cone fields one `UP_FIRE` carries.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FireCone {
    aim: [f32; 4],
    frac: u8,
    scale: f32,
    zoom: bool,
    stat: u32,
}

/// A shot rotation `(x, y, z, w)` whose forward axis (rotated +Z) is the unit vector `d`: yaw about +Y, then pitch
/// about +X.
// SPEC: MOVEMENT_2009.md §6.3 (forward axis of the yaw-then-pitch rotation)
pub fn look_quat(d: [f32; 3]) -> [f32; 4] {
    let yaw = d[0].atan2(d[2]);
    let pitch = -d[1].clamp(-1.0, 1.0).asin();
    let (sy, cy) = (yaw * 0.5).sin_cos();
    let (sp, cp) = (pitch * 0.5).sin_cos();
    [cy * sp, sy * cp, -sy * sp, cy * cp]
}

use dc_net_engine::time::{later, since};
use dc_net_engine::{Bye, JoinStatus, Joiner, Link, LinkEvent, Millis};
use dc_wire::udp::{
    read_quat128, BitReader, BitWriter, Pos48, Vec72, TODO_CLIENT_JOIN_RATE_BPS,
    TODO_GAME_APP_ID_GUID_LAYOUT,
};

/// Settings of the synthetic client.
#[derive(Debug, Clone, PartialEq)]
pub struct SynthConfig {
    pub name: String,
    /// LG-327: the seat ticket its hello carries.
    pub seat_ticket: Option<dc_wire::seatticket::Ticket>,
    pub app_id: [u8; 16],
    pub join_rate_bps: u32,
    /// Resource paths the client can resolve (ASCII case-insensitive); `None` resolves every name.
    pub resources: Option<Vec<String>>,
    /// Simulated world load time.
    pub load_ms: u32,
    /// Player updates per second once in the world.
    // SPEC: UDP_SESSION_2009_B.md §10.3 (default R = 15.0)
    pub update_hz: u32,
    /// The world box (the client knows it from the world file) to decode `pos48`; `None` = keep raw positions.
    pub box_bounds: Option<([f32; 3], [f32; 3])>,
    /// Answer `GS_IDENTIFY_REQUEST` (the 2009 client does when it is in a network game).
    pub answer_identify: bool,
    /// Send `UP_WORLD_READY` when the in-world test passes.
    pub send_world_ready: bool,
    /// Distance moved along +x per player update.
    pub walk_step: f32,
    /// Updates per leg of a back-and-forth walk (0 = always +x).
    pub walk_period: u32,
    /// `UP_PLAYER_INFO` head values.
    pub char_no: u32,
    pub room_slot: u32,
    /// The weapon command table (position -> weapon number) for `GS_WEAPON_SWITCH`; `None` = the weapon given in
    /// slot 0 is the one switched to.
    pub weapon_commands: Option<Vec<u16>>,
    /// Arm the respawn wait after a death and ask once it passed, as the 2009 client does.
    pub self_arm: bool,
    /// The wait; `None` = the 2009 client's wait of the mode class (7000 ms, 1500 ms in class 10).
    pub respawn_wait_ms: Option<u32>,
    /// Mode class used when the server sends no `GS_GAME_ENDPOINT` (the 2009 client then has the lobby's class).
    pub mode_class_hint: Option<u8>,
    /// Leave (`C2S_BYE`) this long after the result screen (PLAUSIBLE for the 2009 client, S2C §2.1: 0x3f sets the
    /// leaving latches); `None` = stay linked.
    pub leave_after_result_ms: Option<u32>,
    /// The control word of every player update; `None` = `FWD` while walking, 0 while standing.
    pub control: Option<u32>,
    /// The 15 look bytes of the `UP_PLAYER_INFO` (default 0, 1, .. 14).
    pub look: [i8; 15],
    /// The trailer's `on_ground` byte (default 1; 0 reports the body airborne).
    // SPEC: MOVEMENT_2009.md §12.2 (trailer `on_ground:u8`)
    pub on_ground: bool,
}

impl SynthConfig {
    pub fn named(name: &str) -> Self {
        SynthConfig {
            name: name.to_string(),
            seat_ticket: None,
            app_id: TODO_GAME_APP_ID_GUID_LAYOUT,
            join_rate_bps: TODO_CLIENT_JOIN_RATE_BPS,
            resources: None,
            load_ms: 1500,
            update_hz: 15,
            box_bounds: None,
            answer_identify: true,
            send_world_ready: true,
            walk_step: 1.0,
            walk_period: 0,
            char_no: 0,
            room_slot: 0,
            weapon_commands: None,
            self_arm: true,
            respawn_wait_ms: None,
            mode_class_hint: None,
            leave_after_result_ms: None,
            control: None,
            look: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14],
            on_ground: true,
        }
    }
}

/// The respawn wait the 2009 client arms for itself after a death, by mode class (`None` = it never asks; class 5
/// arms only after a server value this client does not model).
// DISASM: cshell+0x60b2e..0x60c0e (CAPTURE_tdm4_KILL_VALIDATION.md §4 item 3: 1, 4, 6, 7, 8 -> 7000; 5 when the own
// record +0x1f8 > 0; 9 when mode_rules+0xb4 = 0; 10 -> 1500; others nothing)
pub fn client_wait_ms(mode_class: u8) -> Option<u32> {
    match mode_class {
        1 | 4 | 6 | 7 | 8 | 9 => Some(7_000),
        10 => Some(1_500),
        _ => None,
    }
}

/// The respawn request of a mode class: shell id and whether it carries `[0:u8]`.
// SPEC: GAME_MESSAGES_2009_C2S.md §3.4 (0x89 for 1, 4, 5, 6, 9; 0xc5 for 7, 8; 0xd2 `[0:u8]` for 10)
pub fn client_request(mode_class: u8) -> Option<(u8, bool)> {
    match mode_class {
        1 | 4 | 5 | 6 | 9 => Some((0x89, false)),
        7 | 8 => Some((0xc5, false)),
        10 => Some((0xd2, true)),
        _ => None,
    }
}

/// How long [`Observed::trails`] keeps stream positions.
pub const TRAIL_KEEP_MS: u32 = 2_000;

/// How a synthetic shooter shows a remote object ([`SynthClient::view_of`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// The newest stream position.
    Raw,
    /// The stream positions interpolated this many ms in the past.
    Interpolate(u32),
    /// The newest stream position moved on by its velocity.
    Extrapolate,
}

/// Client states (spec A §8.1 names).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientState {
    Joining,
    Linked,
    LoadingWorld,
    WorldLoaded,
    InWorld,
    /// The engine start failed with this result code (join timeout 0x4F, refused 0x51, wrong app 0x52).
    Failed(u32),
    /// The session ended with this code (handler error, or the transport drop code).
    Dropped(u32),
}

/// An object the client created from a create record.
#[derive(Debug, Clone, PartialEq)]
pub struct SeenObject {
    pub kind: u8,
    pub create_pos: [f32; 3],
    pub pos: Option<[f32; 3]>,
    pub flags: Option<u16>,
    /// Shell blob type of the create record (0x18 = character) and the `pid` of a character blob.
    pub blob_type: Option<u8>,
    pub blob_pid: Option<u8>,
    /// Model file ids of the last model block (first model, its sub files, then the other models).
    pub models: Vec<u16>,
    /// `(child object, socket)` of the attachment list.
    pub attached: Vec<(u16, u32)>,
    pub dims: Option<[f32; 3]>,
    /// Animation indices of the last reliable animation block.
    pub anim: Vec<u16>,
    pub records: u32,
    /// `cf` of the last record on the object as read (the create's own bits, a change record's mask).
    pub last_cf: u16,
    /// The shell blob of the create record (type byte first), for checks of the record layout.
    pub blob: Option<BitWriter>,
    /// The user flag word of the last flag block (the world camera's live bit 0x20).
    // SPEC: UDP_SESSION_2009_B.md §4.2 item 3 (third word of `CF_FLAGS`)
    pub uflags: Option<u32>,
}

/// A roster record as the client stored it from `GS_PLAYER_INFO`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenRecord {
    pub name: Vec<u8>,
    pub account: u32,
    pub character: u16,
    pub kills: i16,
    pub deaths: u8,
    pub team: u8,
    pub dead: bool,
    pub kind: u8,
    /// The 15 look bytes.
    pub look: [i8; 15],
    /// Field 17 `h2`: the rank of the world mark.
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 field 17
    pub rank: u32,
    /// Field 16, the group block: (clan name, group number, [g1, g2, g3]); `None` without `has_group`.
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 field 16
    pub clan: Option<(String, u64, [u32; 3])>,
}

/// A remote object as seen in the object stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeenRemote {
    pub at: Millis,
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub yaw: Option<f32>,
    pub entries: u32,
    /// Entries with `yaw8` and with an animation block; the last three tracker animations.
    pub yaw8_entries: u32,
    pub anim_entries: u32,
    pub last_anim: [u16; 3],
    /// The main tracker of the last stream animation block: animation, state bits, time (ms).
    pub last_main: Option<(u16, bool, bool, u32)>,
    /// Tracker 1 (upper body) of the last stream animation block, in the same form.
    pub last_upper: Option<(u16, bool, bool, u32)>,
}

/// A kill feed line (`GS_KILL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeenKill {
    pub at: Millis,
    pub victim: u8,
    pub killer: u8,
    pub weapon: u16,
    pub zone: u8,
    pub kill_kind: u8,
    pub relation: bool,
    pub streak_flag: u8,
    pub feed_value: u8,
}

/// A tracked sound (special record kind 1) as the client read it, the cached fields filled in.
// SPEC: UDP_SESSION_2009_B.md §8.1 (per-file cache), §8.3 (layout; reader engine+0xa3070 / engine+0xa2e90)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeenSound {
    pub tmask: u8,
    pub track: u16,
    pub sfile: u16,
    pub dmask: u8,
    pub sflags: u16,
    pub prio: u8,
    pub outer: u16,
    pub ratio: u8,
    pub volume: Option<u8>,
    pub pitch: Option<f32>,
    pub tag: u32,
    /// The raw `pos48` and, with the configured box, the decoded position.
    pub pos48: Option<[u16; 3]>,
    pub pos: Option<[f32; 3]>,
    /// Records received for this track id.
    pub records: u32,
}

/// A `GS_DAMAGE_TAKEN` as the client read it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeenDamage {
    pub at: Millis,
    pub from_dir: [f32; 3],
    pub unused: u8,
    pub mode: u8,
    pub ammo_kind: u16,
    pub dmg_kind: u8,
    pub zone: u8,
    pub part: i8,
    pub damage: u32,
}

/// A `GS_MUSIC` play order: file, loop, fade seconds.
pub type MusicPlay = (Vec<u8>, bool, f32);

/// Everything the client observed, for assertions.
#[derive(Debug, Clone, Default)]
pub struct Observed {
    /// Server -> client message ids in processing order, with the time.
    pub messages: Vec<(Millis, u8)>,
    pub version: Option<(u32, u32)>,
    pub client_id: Option<(u16, u8)>,
    pub files: BTreeMap<u16, (u32, Vec<u8>)>,
    pub file_acks_sent: usize,
    pub load_world: Option<(f32, u16)>,
    pub preload_subs: Vec<u8>,
    pub load_states_sent: Vec<(Millis, u8)>,
    pub sky_count: Option<u16>,
    /// The sky table's block as 12 f32 and its object ids.
    pub sky_block: Option<[f32; 12]>,
    pub sky_ids: Vec<u16>,
    /// `S2C_DIR_LIGHT` (id 22): time, direction, colour, extra; the last one received.
    pub dir_light: Option<(Millis, [f32; 3], [f32; 3], f32)>,
    /// `GS_PLAYER_STATUS` kind 1: time, weapon, ammo kind, ammo, slot, extra.
    pub ammo_notes: Vec<(Millis, u16, u16, f32, u8, u16)>,
    pub objects: BTreeMap<u16, SeenObject>,
    pub view_object: Option<u16>,
    pub switch_state: Option<u8>,
    pub server_times: Vec<(Millis, f32)>,
    pub stale_times: u32,
    pub in_world_at: Option<Millis>,
    pub updates_sent: u32,
    pub errors: Vec<String>,
    pub link_rate: Option<u32>,
    // ---- game layer
    /// Shell ids of every `S2C_SHELL_MSG`, in processing order.
    pub shell_ids: Vec<(Millis, u8)>,
    /// `GS_PLAYER_STATE` values.
    pub life_states: Vec<(Millis, u8)>,
    pub identify_requests: u32,
    pub world_ready_sent: Option<Millis>,
    pub roster: BTreeMap<u8, SeenRecord>,
    pub own_team: Option<u8>,
    /// `GS_MATCH_SETTINGS`: own team, goal, round limit.
    pub settings: Option<(i8, u8, i8)>,
    /// `GS_MATCH_BEGIN`: round seconds, the waiting-banner flag (the second head bit).
    pub match_begin: Option<(u32, bool)>,
    /// Every `GS_MATCH_BEGIN` taken in player state 1, where the 2009 client plays its round-start voice: time, the
    /// waiting flag (set: the first-spawn key instead of the round-start key), and whether the last shell 0x90 flag
    /// was still set.
    // DISASM: cshell+0x49f00..0x49f7b (the `local_player`+0x30 == 1 test and the two interface sound keys)
    pub round_start_cues: Vec<(Millis, bool, bool)>,
    /// Every `GS_MATCH_BEGIN` received (time).
    pub match_begins: Vec<Millis>,
    /// The intro block of `GS_MATCH_BEGIN` when it had one: mode caption, notice head, the two map pictures, the radar
    /// picture.
    pub intro: Option<[Vec<u8>; 5]>,
    /// `S2C_EVAR` names and values in arrival order, with the time.
    pub evars: Vec<(Millis, Vec<u8>, Vec<u8>)>,
    pub team_scores: Vec<(Millis, u8, u8)>,
    /// `GS_MOVER_UPDATE` teleports: time, seq, position, half extents.
    pub teleports: Vec<(Millis, u8, [f32; 3], [f32; 3])>,
    pub body_blocks: u32,
    pub angles: Option<[f32; 3]>,
    pub health: Option<f32>,
    pub armor: Option<f32>,
    pub max_health: Option<f32>,
    /// `(shell id, weapon, ammo kind)` of 0xb2 / 0xb3.
    pub weapons: Vec<(u8, u16, u16)>,
    /// `(weapon, ammo)` of every 0xb2 / 0xb3, in the order received.
    pub weapon_amounts: Vec<(u16, f32)>,
    /// `(weapon, part bytes)` of every 0xb2 / 0xb3, in the order received.
    // DISASM: cshell+0x60d90 (three attachment parts and the finish, -1 = none)
    pub weapon_parts: Vec<(u16, [i8; 4])>,
    pub damage: Vec<(Millis, u32)>,
    pub kills: Vec<SeenKill>,
    pub multi_kills: Vec<u32>,
    /// `GS_ROUND_RESULT`: win team, team values.
    pub round_result: Option<(i8, u8, u8)>,
    pub summaries: Vec<u8>,
    pub round_wins: Option<(u8, u8)>,
    pub result_screen: Option<Millis>,
    pub level_exit: Option<Millis>,
    pub ping_tables: Vec<(Millis, Vec<(u16, u16)>)>,
    pub players_left: Vec<u8>,
    pub chat: Vec<(String, String, u32, bool)>,
    pub perf_enabled: bool,
    pub remote: BTreeMap<u16, SeenRemote>,
    /// Stream positions of each remote object with their arrival time (the last [`TRAIL_KEEP_MS`]).
    pub trails: BTreeMap<u16, VecDeque<(Millis, [f32; 3])>>,
    pub removed: Vec<u16>,
    /// Tracked sounds by track id (the last record of each), and the per-file description cache.
    pub sounds: BTreeMap<u16, SeenSound>,
    pub sound_cache: BTreeMap<u16, (u16, u8, u16, u8)>,
    pub fires_sent: u32,
    /// `GS_MATCH_SETTINGS` `own_value`: the shot counter the server set (the 0x82 `stat` base, DSV-102).
    pub shot_seed: Option<u32>,
    /// The client's shot counter (`local_player + 0x1fc`): set by 0x7d, two table draws per cone pellet.
    pub shot_counter: u32,
    // ---- slice 3
    /// `GS_MOVER_UPDATE` friction values (blocks 0x0004 and 0x0008) and freeze values.
    pub frictions: Vec<f32>,
    pub freezes: Vec<bool>,
    /// Body blocks: model name and skins.
    pub bodies: Vec<(Vec<u8>, Vec<Vec<u8>>)>,
    /// `GS_WEAPON_SWITCH`: (command, auto).
    pub weapon_switches: Vec<(u8, bool)>,
    /// `GS_GAME_ENDPOINT` mode class.
    pub endpoint: Option<u8>,
    /// Shell 0x90 flags.
    pub flag90: Vec<bool>,
    /// `GS_ROUND_BEGIN` round seconds.
    pub round_begin: Option<u32>,
    /// Every `GS_ROUND_BEGIN`: (time, round seconds, round number, round limit).
    pub round_begins: Vec<(Millis, u32, u32, u32)>,
    /// Every `GS_ROUND_RESULT` head: (time, win team, the two win counters).
    pub round_results: Vec<(Millis, i8, u8, u8)>,
    /// Every `S2C_OBJ_GROUP` body (the bits after the id byte), for cross-checks with other readers.
    pub groups: Vec<BitWriter>,
    /// `S2C_SHELL_OBJ_MSG` (id 17) effects: time, effect type, the record after the type byte.
    pub effects: Vec<(Millis, u8, BitWriter)>,
    // ---- slice 4
    /// Respawn waits armed after deaths: (time armed, wait).
    pub respawn_armed: Vec<(Millis, u32)>,
    /// Respawn requests sent: (time, shell id).
    pub respawn_requests: Vec<(Millis, u8)>,
    /// Every `GS_DAMAGE_TAKEN`.
    pub damage_forms: Vec<SeenDamage>,
    /// Every `GS_ROUND_WINS` (0xb5): time and the two values (team kills in TDM).
    pub team_kills: Vec<(Millis, u8, u8)>,
    /// `GS_TEAM_SWAP` 0x5e and `GS_CLAN_STATUS` 0x5f kind 1 with `no_swap` 0: (time, id, scores, own team after).
    pub team_swaps: Vec<(Millis, u8, [u8; 2], Option<u8>)>,
    /// `GS_ROUND_RESULT` `round_secs`, `GS_ROUND_CLOSE` `round_secs`, `GS_ROUND_OBJECTS_END` time.
    pub round_result_secs: Option<u32>,
    pub round_close: Option<(Millis, u32)>,
    pub objects_end: Option<Millis>,
    /// `GS_PLAYER_SUMMARY` rows: (pid, object, exp before, exp gain, GP gain).
    pub summary_rows: Vec<(u8, u16, u32, u32, u32)>,
    /// `GS_PLAYER_SUMMARY` bonus pairs: (pid, exp bonus, GP bonus).
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.4 row 0x65 (`exp_bonus`, `gp_bonus`)
    pub summary_bonus: Vec<(u8, u32, u32)>,
    /// `GS_MATCH_RESULT_SCREEN` (flag, value).
    pub result_value: Option<(bool, u32)>,
    /// Shell 0x9d values.
    pub shell_9d: Vec<u32>,
    /// Every shell 0x88 value in order.
    pub switch_states: Vec<u8>,
    /// Every roster record in order: (time, pid, kind, dead).
    pub roster_log: Vec<(Millis, u8, u8, bool)>,
    // ---- co-op (mode class 5)
    /// The own record's +0x1f8 (0x6b `m1` of the own record, 0xd7 sub 2 `b`): lives counting the current one, and
    /// every write in order (time, source shell id, value).
    pub own_lives: Option<u8>,
    pub lives_log: Vec<(Millis, u8, u8)>,
    /// `GS_COOP` sub 1 (pid, obj, x, points), sub 3 (pid, x, points), sub 4 (running, secs).
    pub coop_points: Vec<(Millis, u32, u16, u32, u32)>,
    pub coop_notes: Vec<(Millis, u32, u8, u32)>,
    pub coop_timers: Vec<(bool, u32)>,
    /// `GS_COOP_MISSION` sub 1 (goal count, timers, a, b) and sub 2 banner texts.
    pub missions: Vec<(u32, Vec<f32>, u8, u8)>,
    pub banners: Vec<Vec<u8>>,
    /// `GS_COOP_MISSION` sub 3 (the mission caption): (time, text, seconds).
    pub captions: Vec<(Millis, Vec<u8>, f32)>,
    /// `GS_MUSIC` 0xd9: (time, `Some((file, loop, fade))` for a play, `None` for a stop).
    pub music: Vec<(Millis, Option<MusicPlay>)>,
    /// `GS_OBJECT_NOTE` type 0x0d sub 1 (a camera's field of view): (time, object, degrees, blend seconds).
    pub camera_fov: Vec<(Millis, u16, [f32; 2], f32)>,
    /// `GS_OBJECT_NOTE` type 0x18 sub 0x24: (time, object, health fraction); notes to objects without the NPC tail
    /// are an error (the client leaves the bytes unread).
    pub npc_health: Vec<(Millis, u16, f32)>,
    /// `GS_HISEC_CASE` 0xd6: (time, sub, case key, player / owner); sub 0 has key and player 0.
    pub hisec: Vec<(Millis, u8, u32, u32)>,
    /// `GS_OBJECT_NOTE` type 0x18 sub 0x1d (the death note, no body): (time, object).
    pub death_notes: Vec<(Millis, u16)>,
    /// Aim notes (type 0x18 sub 0x80): arrival, object, the six words.
    pub aim_notes: Vec<(Millis, u16, [u32; 6])>,
    /// `GS_OBJECT_NOTE` type 0x17 sub 2 (the case countdown): (time, object, remaining).
    pub case_notes: Vec<(Millis, u16, u32)>,
    /// `GS_OBJECT_NOTE` type 0x18 subs 0x1f / 0x25 (the spawn notes, no body): (time, object, sub).
    pub spawn_notes: Vec<(Millis, u16, u8)>,
    /// `GS_OBJECT_NOTE` type 0x39 (a door's activate entry): (time, object, sub, value).
    pub door_notes: Vec<(Millis, u16, u8, u8)>,
    /// Tracked-sound starts in arrival order: (track, sound file id).
    pub sound_starts: Vec<(u16, u16)>,
    /// Tracked sounds removed by a removed-object record.
    pub sounds_removed: Vec<u16>,
    /// Removal records for ids this client never had (an object or a sound it was not sent).
    pub removed_unknown: Vec<u16>,
    /// `S2C_PRELOAD` sub 5 file lists.
    pub preload_sounds: Vec<Vec<u16>>,
    /// `GS_ROUND_RESULT` co-op block (stage, six values, names).
    pub coop_result: Option<(u8, [u32; 6], Vec<Vec<u8>>)>,
    /// `GS_PLAYER_SUMMARY` co-op blocks (pid, c1, c2, c3, c4).
    pub coop_summaries: Vec<(u8, [u32; 4])>,
    /// The own lives value at each `GS_PLAYER_STATE` 2 (what the class-5 client decides its respawn on).
    pub lives_at_death: Vec<Option<u8>>,
}

/// The synthetic client.
pub struct SynthClient {
    cfg: SynthConfig,
    state: ClientState,
    joiner: Joiner,
    link: Option<Link>,
    pub seen: Observed,
    world_loaded: bool,
    load_done_at: Option<Millis>,
    held: VecDeque<BitWriter>,
    next_update: Millis,
    pos: [f32; 3],
    /// View pitch sent as `aux[0]` of the player update (radians).
    pitch: f32,
    move_seq: u8,
    life_state: u8,
    weapon: (u16, u16),
    /// Weapons given by `GS_WEAPON_GIVE`, by slot (weapon, ammo kind).
    given: BTreeMap<u8, (u16, u16)>,
    quit_sent: bool,
    start: Option<Millis>,
    /// The client's respawn wait: death time (set by state 2, cleared by state 1), whether it ran for this death,
    /// and the armed wait (start, length).
    death_time: Option<Millis>,
    self_armed: bool,
    armed: Option<(Millis, u32)>,
    /// When the client leaves after the result screen.
    leave_at: Option<Millis>,
    /// A walk to this point, at most this far per update (tests of proximity rules); replaces the walk step.
    goal: Option<([f32; 3], f32)>,
}

const SHELL_SWITCH: u8 = 0x88;

fn vec3(r: &mut BitReader<'_>) -> [f32; 3] {
    [r.read_f32(), r.read_f32(), r.read_f32()]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

impl SynthClient {
    pub fn new(cfg: SynthConfig) -> Self {
        SynthClient {
            joiner: Joiner::new(cfg.app_id, cfg.join_rate_bps),
            cfg,
            state: ClientState::Joining,
            link: None,
            seen: Observed::default(),
            world_loaded: false,
            load_done_at: None,
            held: VecDeque::new(),
            next_update: 0,
            pos: [0.0; 3],
            pitch: 0.0,
            move_seq: 0,
            life_state: 0,
            weapon: (0, 0),
            given: BTreeMap::new(),
            quit_sent: false,
            start: None,
            death_time: None,
            self_armed: false,
            armed: None,
            leave_at: None,
            goal: None,
        }
    }

    /// The mode class the client plays: `GS_GAME_ENDPOINT`, else the configured hint.
    pub fn mode_class(&self) -> Option<u8> {
        self.seen.endpoint.or(self.cfg.mode_class_hint)
    }

    /// The respawn wait: armed once per death in the first poll after state 2 while not playing, then one request
    /// strictly after the wait.
    // DISASM: cshell+0x62467..0x62494 (every frame while the state is 2, 3, 5 or 6 and the death time is set),
    // cshell+0x6203e..0x62132 (`now - start > wait`, one reliable request, disarm)
    fn respawn_wait(&mut self, now: Millis) {
        if !self.cfg.self_arm || !matches!(self.life_state, 2 | 3 | 5 | 6) {
            return;
        }
        let Some(class) = self.mode_class() else {
            return;
        };
        if self.death_time.is_some() && !self.self_armed {
            self.self_armed = true;
            // DISASM: cshell+0x60ac0..0x60bcc (class 5: armed only while the own record +0x1f8 > 0, FIRETEAM_2009.md
            // §2.3)
            let coop = (class == 5).then(|| self.seen.own_lives.unwrap_or(0) > 0);
            let wait = match coop {
                Some(false) => None,
                Some(true) => self.cfg.respawn_wait_ms.or(Some(7_000)),
                None => self.cfg.respawn_wait_ms.or(client_wait_ms(class)),
            };
            if let Some(w) = wait {
                self.armed = Some((now, w));
                self.seen.respawn_armed.push((now, w));
            }
        }
        if let Some((start, wait)) = self.armed {
            if since(now, start) > wait {
                self.armed = None;
                if let Some((id, zero)) = client_request(class) {
                    let mut b = BitWriter::new();
                    if zero {
                        b.write_u8(0);
                    }
                    self.send_shell(id, &b);
                    self.seen.respawn_requests.push((now, id));
                }
            }
        }
    }

    pub fn state(&self) -> ClientState {
        self.state
    }

    pub fn link(&self) -> Option<&Link> {
        self.link.as_ref()
    }

    /// The current player state (`GS_PLAYER_STATE`, 0 before the first one).
    pub fn life_state(&self) -> u8 {
        self.life_state
    }

    /// Own body position.
    pub fn position(&self) -> [f32; 3] {
        self.pos
    }

    /// Moves the own body to height `y` from the next player update on (a test lift or drop; x and z stay).
    pub fn set_height(&mut self, y: f32) {
        self.pos[1] = y;
    }

    /// Reports the body grounded or airborne from the next player update on (`SynthConfig::on_ground`).
    pub fn set_on_ground(&mut self, on: bool) {
        self.cfg.on_ground = on;
    }

    /// Walks to `target` (x and z; the height stays), at most `step` units per player update; `None` stops.
    pub fn walk_to(&mut self, target: Option<[f32; 3]>, step: f32) {
        self.goal = target.map(|t| (t, step));
    }

    /// Sends `pitch` as the view pitch (`aux[0]`) from the next player update on.
    pub fn set_pitch(&mut self, pitch: f32) {
        self.pitch = pitch;
    }

    /// Sets the back-and-forth walk (`SynthConfig::walk_period`).
    pub fn walk_period(&mut self, updates: u32) {
        self.cfg.walk_period = updates;
    }

    /// The in-world test: the view object exists, the switching-worlds state is 2 or 3, the world is loaded and
    /// the player state is non-zero.
    // SPEC: UDP_SESSION_2009_A.md §8.3 a-c; UDP_SESSION_2009_A_addendum_disasm.md A16 (d: shell 0x67)
    pub fn in_world_test(&self) -> bool {
        let a = self
            .seen
            .view_object
            .is_some_and(|o| self.seen.objects.contains_key(&o));
        let b = matches!(self.seen.switch_state, Some(2) | Some(3));
        a && b && self.world_loaded && self.life_state != 0
    }

    /// Asks the server to end the session (`C2S_BYE`, reliable).
    // SPEC: UDP_SESSION_2009_A.md §5.6
    pub fn quit(&mut self) {
        if let Some(l) = self.link.as_mut() {
            if !self.quit_sent {
                self.quit_sent = true;
                l.send_reliable(BitWriter::from_bytes(&[6]));
                // quiet after the C2S_BYE block has left (UDP_TRANSPORT_2009.md §9.1)
                l.quiesce_when_flushed();
            }
        }
    }

    fn send_shell(&mut self, shell_id: u8, body: &BitWriter) {
        // SPEC: GAME_MESSAGES_2009_C2S.md §1.2 (`[11][shell_id][fields]`, reliable for these senders)
        let mut m = BitWriter::new();
        m.write_u8(11).write_u8(shell_id).append(body);
        if let Some(l) = self.link.as_mut() {
            l.send_reliable(m);
        }
    }

    /// `C2S_CONSOLE` `[10][command:zstr8]`, reliable (tests of the remote console rule).
    // SPEC: UDP_SESSION_2009_B.md §9.4 (client console command to the server)
    pub fn send_console(&mut self, command: &str) {
        let mut m = BitWriter::new();
        m.write_u8(10).write_zstr8(command.as_bytes());
        if let Some(l) = self.link.as_mut() {
            l.send_reliable(m);
        }
    }

    /// Sends any shell message (tests of unhandled ids).
    pub fn send_raw_shell(&mut self, shell_id: u8, body: &BitWriter) {
        self.send_shell(shell_id, body);
    }

    /// `UP_LEAVE_MATCH` (no fields), then `C2S_BYE` as the 2009 client does.
    // SPEC: GAME_MESSAGES_2009_C2S.md §5.7; OBSERVED: capture 20261025_065016_tdm6 and
    // 20261025_081704_rust3 (the real client's `C2S_BYE` 15-16 ms after its 0xbd)
    pub fn leave_match(&mut self) {
        self.leave_match_only();
        self.quit();
    }

    /// `UP_LEAVE_MATCH` without the bye (tests of the server's wait for it).
    pub fn leave_match_only(&mut self) {
        self.send_shell(0xbd, &BitWriter::new());
    }

    /// `UP_RESPAWN_REQUEST` 0x89 (no fields).
    // SPEC: GAME_MESSAGES_2009_C2S.md §3.4
    pub fn request_respawn(&mut self) {
        self.send_shell(0x89, &BitWriter::new());
    }

    /// `UP_CHAT` `[text:zstr16][tag:zstr16][scope:u8]`.
    // SPEC: GAME_MESSAGES_2009_C2S.md §6.1
    pub fn chat(&mut self, text: &str, scope: u8) {
        let mut b = BitWriter::new();
        let t: Vec<u16> = text.encode_utf16().collect();
        b.write_zstr16_presumed(&t).write_zstr16_presumed(&[]);
        b.write_u8(scope);
        self.send_shell(0x68, &b);
    }

    /// `UP_RELOAD` `[ammo_id:u16]` for the drawn weapon.
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.2
    pub fn reload(&mut self) {
        let ammo = self.weapon.1;
        self.reload_ammo(ammo);
    }

    /// `UP_RELOAD` `[ammo_id:u16]` for any ammunition (a throwable reloads its hand from its stock right after the
    /// release, in the same block).
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.2
    // OBSERVED: capture 20261025_095358_rust4 (0x82 then 0x83 [143] in one block)
    pub fn reload_ammo(&mut self, ammo: u16) {
        let mut b = BitWriter::new();
        b.write_u16(ammo);
        self.send_shell(0x83, &b);
    }

    /// One `UP_FIRE` pellet aimed at a remote object (its stream position plus `offset`), reported as a hit on it
    /// when `hit` (the client's own trace result), else as a miss. Returns false when the object is unknown.
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.1 (layout; `hit_dist = dot(hit - origin, dir)`)
    pub fn fire_at(&mut self, obj: u16, offset: [f32; 3], hit: bool) -> bool {
        let Some(t) = self.seen.remote.get(&obj).map(|r| r.pos) else {
            return false;
        };
        let target = [t[0] + offset[0], t[1] + offset[1], t[2] + offset[2]];
        let origin = self.pos;
        let d = sub(target, origin);
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-3);
        let dir = [d[0] / len, d[1] / len, d[2] / len];
        let weapon = self.weapon;
        self.send_fire(weapon, origin, dir, hit.then_some((len, obj)));
        true
    }

    /// One `UP_FIRE` pellet aimed at a world point, reported as a hit on `obj` (a world object such as a breakable
    /// model) when `hit`, else as a miss.
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.1 (layout; `hit_dist = dot(hit - origin, dir)`)
    pub fn fire_at_point(&mut self, obj: u16, target: [f32; 3], hit: bool) {
        let origin = self.pos;
        let d = sub(target, origin);
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-3);
        let dir = [d[0] / len, d[1] / len, d[2] / len];
        let weapon = self.weapon;
        self.send_fire(weapon, origin, dir, hit.then_some((len, obj)));
    }

    /// Where this client shows a remote object at `now`: with `View::Interpolate(d)` the stream positions
    /// interpolated at `now - d` (clamped to the oldest and newest), with `View::Extrapolate` the newest position
    /// moved on by its streamed velocity, with `View::Raw` the newest position.
    pub fn view_of(&self, obj: u16, now: Millis, view: View) -> Option<[f32; 3]> {
        let r = self.seen.remote.get(&obj)?;
        match view {
            View::Raw => Some(r.pos),
            View::Extrapolate => {
                let dt = since(now, r.at) as f32 / 1000.0;
                Some([
                    r.pos[0] + r.vel[0] * dt,
                    r.pos[1] + r.vel[1] * dt,
                    r.pos[2] + r.vel[2] * dt,
                ])
            }
            View::Interpolate(delay) => {
                let trail = self.seen.trails.get(&obj)?;
                let at = now.wrapping_sub(delay);
                let mut prev: Option<(Millis, [f32; 3])> = None;
                for &(t, p) in trail {
                    if later(t, at) {
                        return Some(match prev {
                            Some((tp, pp)) => {
                                let f = since(at, tp) as f32 / since(t, tp).max(1) as f32;
                                [
                                    pp[0] + (p[0] - pp[0]) * f,
                                    pp[1] + (p[1] - pp[1]) * f,
                                    pp[2] + (p[2] - pp[2]) * f,
                                ]
                            }
                            None => p,
                        });
                    }
                    prev = Some((t, p));
                }
                prev.map(|x| x.1)
            }
        }
    }

    /// One `UP_FIRE` hit claim on a remote object as this client shows it (`view`), at that position plus `offset`.
    pub fn fire_at_view(&mut self, obj: u16, offset: [f32; 3], now: Millis, view: View) -> bool {
        let Some(t) = self.view_of(obj, now, view) else {
            return false;
        };
        let target = [t[0] + offset[0], t[1] + offset[1], t[2] + offset[2]];
        let origin = self.pos;
        let d = sub(target, origin);
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-3);
        let dir = [d[0] / len, d[1] / len, d[2] / len];
        let weapon = self.weapon;
        self.send_fire(weapon, origin, dir, Some((len, obj)));
        true
    }

    /// One `UP_FIRE` pellet the way the 2009 client builds it (DSV-102 tests): the shot rotation looks at the remote
    /// object's stream position plus `offset`, the pellet direction is that rotation's forward axis tilted by the
    /// cone ([`crate::cone::pellet_dir`]) with two draws of `table` from the client's own counter and the half-width
    /// `shot.h`, and the message carries the counter after the draws as `stat` together with the spread byte, the
    /// shot scale and the zoom bit. `shot.forged` replaces the direction by the forward axis (a "no spread" client) and
    /// keeps everything else. A hit claim (`hit`) reports the distance to the aimed point along the pellet.
    // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-2.4, AC-3.3, AC-3.4; GAME_MESSAGES_2009_C2S.md §4.1
    pub fn fire_cone_at(
        &mut self,
        obj: u16,
        offset: [f32; 3],
        table: &[f32],
        shot: ConeShot,
        hit: bool,
    ) -> bool {
        let Some(t) = self.seen.remote.get(&obj).map(|r| r.pos) else {
            return false;
        };
        let target = [t[0] + offset[0], t[1] + offset[1], t[2] + offset[2]];
        let origin = self.pos;
        let d = sub(target, origin);
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-3);
        let aim = look_quat([d[0] / len, d[1] / len, d[2] / len]);
        let (r1, r2, stat) = crate::cone::draw_pair(table, self.seen.shot_counter);
        self.seen.shot_counter = stat;
        let dir = if shot.forged {
            crate::cone::axes(aim).2
        } else {
            crate::cone::pellet_dir(aim, r1, r2, shot.h)
        };
        let weapon = self.weapon;
        let cone = FireCone {
            aim,
            frac: shot.frac,
            scale: shot.scale,
            zoom: shot.zoom,
            stat,
        };
        self.send_fire_with(weapon, origin, dir, hit.then_some((len, obj)), Some(cone));
        true
    }

    /// A throw: `UP_THROW_HOLD` 0x4d, then the release, one `UP_FIRE` with the throwable's ids from the own
    /// position along `dir`, no hit.
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.6 (0x4d repeats while held; the release is one 0x82 with no extra block and
    // no hit, `last` 1)
    pub fn throw(&mut self, weapon: (u16, u16), dir: [f32; 3]) {
        self.throw_hold();
        self.throw_release(weapon, dir);
    }

    /// One `UP_THROW_HOLD` 0x4d (the throwable held).
    pub fn throw_hold(&mut self) {
        self.send_shell(0x4d, &BitWriter::new());
    }

    /// The release alone: one `UP_FIRE` with the throwable's ids from the own position along `dir`, no hit.
    pub fn throw_release(&mut self, weapon: (u16, u16), dir: [f32; 3]) {
        let origin = self.pos;
        self.send_fire(weapon, origin, dir, None);
    }

    /// `UP_FIRE` with `origin` as both the eye and the weapon position.
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.1 (layout)
    fn send_fire(
        &mut self,
        weapon: (u16, u16),
        origin: [f32; 3],
        dir: [f32; 3],
        hit: Option<(f32, u16)>,
    ) {
        self.send_fire_with(weapon, origin, dir, hit, None);
    }

    /// [`SynthClient::send_fire`] with the cone fields of a 2009-style pellet (`None`: the identity rotation, spread
    /// byte 0, `shot_scale` = the message count, `stat` 0).
    fn send_fire_with(
        &mut self,
        weapon: (u16, u16),
        origin: [f32; 3],
        dir: [f32; 3],
        hit: Option<(f32, u16)>,
        cone: Option<FireCone>,
    ) {
        let mut b = BitWriter::new();
        b.write_u8(0).write_u16(weapon.0).write_u16(weapon.1);
        for v in origin.iter().chain(origin.iter()) {
            b.write_f32(*v);
        }
        let aim = cone.map_or([0.0, 0.0, 0.0, 1.0], |c| c.aim);
        for v in aim {
            b.write_f32(v);
        }
        b.write_f32(0.0);
        for v in dir {
            b.write_f32(v);
        }
        b.write_u8(9)
            .write_bit(cone.is_some_and(|c| c.zoom))
            .write_bit(false)
            .write_bit(false);
        let shot_word = cone.map_or(self.seen.fires_sent, |c| c.scale.to_bits());
        b.write_u8(cone.map_or(0, |c| c.frac))
            .write_u32(shot_word)
            .write_u32(0);
        b.write_bit(false); // has_extra
        b.write_bit(false).write_bit(true); // flag_c, last
        b.write_bit(hit.is_some());
        if let Some((len, obj)) = hit {
            b.write_f32(len).write_u16(obj);
        }
        b.write_u32(cone.map_or(0, |c| c.stat));
        self.send_shell(0x82, &b);
        self.seen.fires_sent += 1;
    }

    /// Datagrams to send now.
    pub fn poll(&mut self, now: Millis) -> Vec<Vec<u8>> {
        self.start.get_or_insert(now);
        let mut out = Vec::new();
        if self.state == ClientState::Joining {
            if let Some(d) = self.joiner.poll(now) {
                out.push(d);
            }
            if let Some(code) = self.joiner.status().result_code() {
                self.state = ClientState::Failed(code);
            }
            return out;
        }
        // finishing a world load: report it, then process what arrived meanwhile
        if let Some(t) = self.load_done_at {
            if !later(t, now) {
                self.load_done_at = None;
                self.world_loaded = true;
                self.state = ClientState::WorldLoaded;
                // SPEC: UDP_SESSION_2009_A.md §4.6 step 7 (`[9][0]`, reliable)
                self.send_rel(now, &[9, 0]);
                while let Some(m) = self.held.pop_front() {
                    if self.load_done_at.is_some() {
                        self.held.push_front(m);
                        break;
                    }
                    self.on_message(now, &m);
                }
            }
        }
        if self.state == ClientState::WorldLoaded && self.in_world_test() {
            self.state = ClientState::InWorld;
            self.seen.in_world_at = Some(now);
            self.next_update = now;
            if self.cfg.send_world_ready && self.seen.world_ready_sent.is_none() {
                // SPEC: GAME_MESSAGES_2009_C2S.md §5.4 (sent once when the loading screen closes)
                self.seen.world_ready_sent = Some(now);
                self.send_shell(0x87, &BitWriter::new());
            }
        }
        if self.state == ClientState::InWorld {
            self.respawn_wait(now);
        }
        if self.leave_at.is_some_and(|t| !later(t, now)) {
            self.leave_at = None;
            self.quit();
        }
        // SPEC: GAME_MESSAGES_2009_C2S.md §2.1 gate (nothing is sent unless the player state is 1)
        if self.state == ClientState::InWorld
            && self.life_state == 1
            && !later(self.next_update, now)
        {
            self.send_update(now);
            self.next_update = now.wrapping_add(1000 / self.cfg.update_hz.max(1));
        }
        let mut events = Vec::new();
        if let Some(l) = self.link.as_mut() {
            events = l.poll(now);
        }
        for e in events {
            match e {
                LinkEvent::Transmit(d) => out.push(d),
                LinkEvent::Message { bits, .. } => {
                    if self.load_done_at.is_some() {
                        self.held.push_back(bits);
                    } else {
                        self.on_message(now, &bits);
                    }
                }
                LinkEvent::Closed { code, .. } => {
                    if !matches!(self.state, ClientState::Dropped(_) | ClientState::Failed(_)) {
                        self.state = ClientState::Dropped(code);
                    }
                }
            }
        }
        out
    }

    /// A datagram from the server.
    pub fn handle_datagram(&mut self, now: Millis, bytes: &[u8]) {
        if self.state == ClientState::Joining {
            match self.joiner.handle_datagram(bytes) {
                JoinStatus::Accepted => {
                    let mut link = self.joiner.clone().into_link(now).expect("accepted");
                    // SPEC: UDP_SESSION_2009_A.md §5.2 (C2S_HELLO first, reliable, blob 258 bytes)
                    let mut blob = crate::msg::build_hello_blob(&self.cfg.name, b"");
                    if let Some(t) = &self.cfg.seat_ticket {
                        dc_wire::seatticket::append_to_blob(&mut blob, t);
                    }
                    let mut hello = BitWriter::new();
                    hello
                        .write_u8(5)
                        .write_u16(blob.len() as u16)
                        .write_raw(&blob);
                    link.send_reliable(hello);
                    // SPEC: UDP_SESSION_2009_A.md §5.5 (`07 04 00` with the default rate)
                    let mut rate = BitWriter::new();
                    rate.write_u8(7)
                        .write_u16((self.cfg.join_rate_bps / 8000).min(65_535) as u16);
                    link.send_reliable(rate);
                    self.link = Some(link);
                    self.state = ClientState::Linked;
                }
                JoinStatus::Pending => {}
                other => self.state = ClientState::Failed(other.result_code().unwrap_or(0)),
            }
            return;
        }
        if let Some(l) = self.link.as_mut() {
            l.handle_datagram(now, bytes);
        }
    }

    fn send_rel(&mut self, now: Millis, bytes: &[u8]) {
        if let Some(l) = self.link.as_mut() {
            l.send_reliable(BitWriter::from_bytes(bytes));
        }
        if bytes.first() == Some(&9) {
            self.seen.load_states_sent.push((now, bytes[1]));
        }
    }

    fn fail(&mut self, now: Millis, code: u32, why: String) {
        self.seen.errors.push(why);
        self.state = ClientState::Dropped(code);
        if let Some(l) = self.link.as_mut() {
            l.close(now, Bye::Plain);
        }
    }

    /// `UP_PLAYER_UPDATE` with the 0x0020 view block and the movement trailer (sequence of the last teleport,
    /// position, velocity, time, ground state).
    // SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (live layout); MOVEMENT_2009.md §12.2 (trailer)
    fn send_update(&mut self, now: Millis) {
        let t = since(now, self.start.unwrap_or(now)) as f32 / 1000.0;
        let period = self.cfg.walk_period;
        let leg = if period > 0 && (self.seen.updates_sent / period) % 2 == 1 {
            -1.0
        } else {
            1.0
        };
        let mut step = self.cfg.walk_step * leg;
        if let Some((g, max)) = self.goal {
            let (dx, dz) = (g[0] - self.pos[0], g[2] - self.pos[2]);
            let d = (dx * dx + dz * dz).sqrt();
            let k = if d > max && d > 0.0 { max / d } else { 1.0 };
            self.pos[0] += dx * k;
            self.pos[2] += dz * k;
            step = d.min(max);
        } else {
            self.pos[0] += step;
        }
        let speed = step * self.cfg.update_hz as f32;
        let mut b = BitWriter::new();
        b.write_u16(0x0020).write_u8(0);
        for c in [0.0, t, 0.0] {
            b.write_f32(c);
        }
        b.write_u32(self.pitch.to_bits());
        for _ in 0..5 {
            b.write_u32(0);
        }
        // control word: FWD while walking, nothing while standing (MOVEMENT_2009.md §5.1)
        b.write_u32(
            self.cfg
                .control
                .unwrap_or(if self.cfg.walk_step != 0.0 { 0x1 } else { 0 }),
        );
        b.write_u8(self.move_seq);
        for c in self.pos {
            b.write_f32(c);
        }
        for c in [speed, 0.0, 0.0] {
            b.write_f32(c);
        }
        b.write_f32(t)
            .write_u8(u8::from(self.cfg.on_ground))
            .write_u8(0)
            .write_u64(u64::MAX);
        let mut m = BitWriter::new();
        m.write_u8(11).write_u8(0x64).append(&b);
        if let Some(l) = self.link.as_mut() {
            // SPEC: UDP_SESSION_2009_B.md §10.3 (the update is unreliable)
            l.send_unreliable(now, m);
            self.seen.updates_sent += 1;
        }
    }

    /// `UP_PLAYER_INFO` (§5.1) with zero profile values, no loadout entries and no clan, then
    /// `UP_CLIENT_OPTIONS`.
    // SPEC: GAME_MESSAGES_2009_C2S.md §5.1, §5.2
    fn send_player_info(&mut self) {
        let mut b = BitWriter::new();
        b.write_u32(self.cfg.room_slot)
            .write_u32(self.cfg.char_no)
            .write_u16(0)
            .write_u32(0);
        for i in self.cfg.look {
            b.write_s8(i);
        }
        b.write_zstr8(b"synth-a").write_zstr8(b"synth-b");
        for _ in 0..4 {
            b.write_s8(0);
        }
        b.write_u32(0)
            .write_s8(0)
            .write_s8(0)
            .write_u32(0)
            .write_u32(0);
        b.write_u8(0); // n loadout entries
        b.write_bit(false); // in_clan
        b.write_u32(0)
            .write_u32(0)
            .write_bit(false)
            .write_bit(false);
        self.send_shell(0x72, &b);
        let mut o = BitWriter::new();
        o.write_u8(0).write_bit(false);
        self.send_shell(0x85, &o);
    }

    fn resolves(&self, name: &[u8]) -> bool {
        match &self.cfg.resources {
            None => true,
            Some(list) => list.iter().any(|r| r.as_bytes().eq_ignore_ascii_case(name)),
        }
    }

    fn on_message(&mut self, now: Millis, msg: &BitWriter) {
        let mut r = msg.reader();
        let id = r.read_u8();
        self.seen.messages.push((now, id));
        // NET-24: like our client, take the server's link move secret (a 2009 client drops this unknown shell id)
        // DC-DEFAULT: NET-24 (proposed, project decision)
        if id == 13 && msg.as_bytes().get(1) == Some(&dc_wire::linkmove::SHELL_ID) {
            let mut b = msg.reader();
            b.seek_bits(16);
            if let (Some(s), Some(l)) = (dc_wire::linkmove::read_body(&mut b), self.link.as_mut()) {
                l.set_move_trailer(s);
            }
            return;
        }
        match id {
            // SPEC: UDP_SESSION_2009_A.md §4.2 (net_version 7 else 0x4E; lower rate is applied)
            4 => {
                let v = r.read_u32();
                let rate = r.read_u32();
                self.seen.version = Some((v, rate));
                if v != 7 {
                    self.fail(now, 0x4E, format!("version {v}"));
                    return;
                }
                if let Some(l) = self.link.as_mut() {
                    if rate < l.rate_bps() {
                        l.change_rate(now, rate);
                    }
                    self.seen.link_rate = Some(l.rate_bps());
                }
            }
            // SPEC: UDP_SESSION_2009_A.md §4.3
            12 => {
                let cid = r.read_u16();
                let local = r.read_u8();
                self.seen.client_id = Some((cid, local));
            }
            // SPEC: UDP_SESSION_2009_A.md §4.4 (entries to the end; one C2S_FILE_ACK per list, bit 15 always set)
            0x32 => {
                let mut ack = BitWriter::new();
                ack.write_u8(0x36);
                while r.remaining_bits() >= 16 + 32 + 8 {
                    let fid = r.read_u16();
                    let info = r.read_u32();
                    let name = r.read_zstr8();
                    self.seen.files.entry(fid).or_insert((info, name));
                    ack.write_u16(fid | 0x8000);
                    self.seen.file_acks_sent += 1;
                }
                if let Some(l) = self.link.as_mut() {
                    l.send_reliable(ack);
                }
            }
            // SPEC: UDP_SESSION_2009_A.md §4.5
            15 => {
                let n = r.read_zstr8();
                let v = r.read_zstr8();
                self.seen.evars.push((now, n, v));
            }
            // SPEC: UDP_SESSION_2009_A.md §4.6 (unknown id or unresolvable name -> 0x29)
            6 => {
                let clock = r.read_f32();
                let file = r.read_u16();
                self.seen.load_world = Some((clock, file));
                self.seen.view_object = None;
                let ok = self
                    .seen
                    .files
                    .get(&file)
                    .is_some_and(|f| self.resolves(&f.1));
                if !ok {
                    self.fail(now, 0x29, format!("world file {file} not found"));
                    return;
                }
                self.seen.server_times.push((now, clock));
                self.world_loaded = false;
                self.state = ClientState::LoadingWorld;
                self.load_done_at = Some(now.wrapping_add(self.cfg.load_ms));
            }
            // SPEC: UDP_SESSION_2009_A.md §4.7 (sub 1 -> `[9][1]`)
            19 => {
                let sub = r.read_u8();
                self.seen.preload_subs.push(sub);
                if sub == 1 {
                    self.send_rel(now, &[9, 1]);
                }
                // SPEC: UDP_SESSION_2009_A.md §4.7 (sub 5: u16 file ids to the end of the message)
                if sub == 5 {
                    let mut ids = Vec::new();
                    while r.remaining_bits() >= 16 {
                        ids.push(r.read_u16());
                    }
                    self.seen.preload_sounds.push(ids);
                }
            }
            // SPEC: UDP_SESSION_2009_A.md §4.8 (count > 30 -> result 1)
            16 => {
                let mut block = [0f32; 12];
                for v in &mut block {
                    *v = r.read_f32();
                }
                self.seen.sky_block = Some(block);
                let count = r.read_u16();
                self.seen.sky_count = Some(count);
                if count > 30 {
                    self.fail(now, 1, format!("sky table count {count}"));
                } else {
                    self.seen.sky_ids = (0..count).map(|_| r.read_u16()).collect();
                }
            }
            // SPEC: UDP_SESSION_2009_B.md §8.4 (`[22][dir][color][extra]`, 7 x f32)
            22 => {
                let mut f = [0.0f32; 7];
                for v in &mut f {
                    *v = r.read_f32();
                }
                if !r.at_end() {
                    self.seen
                        .errors
                        .push(format!("dir light: {} bits left", r.remaining_bits()));
                }
                self.seen.dir_light = Some((now, [f[0], f[1], f[2]], [f[3], f[4], f[5]], f[6]));
            }
            // SPEC: UDP_SESSION_2009_A.md §4.9
            7 => self.seen.view_object = Some(r.read_u16()),
            5 => {
                self.seen.view_object = None;
                self.world_loaded = false;
                self.state = ClientState::Linked;
            }
            // SPEC: UDP_SESSION_2009_B.md §3.1 (records `[rec_bits:u32][record]` to the end)
            8 => {
                let mut copy = msg.reader();
                copy.skip_bits(8);
                let n = copy.remaining_bits();
                self.seen.groups.push(copy.read_bit_string(n));
                while r.remaining_bits() >= 32 {
                    let n = r.read_u32() as usize;
                    if n > r.remaining_bits() {
                        self.fail(now, 0x2c, "record past the end".into());
                        return;
                    }
                    let rec = r.read_bit_string(n);
                    if let Err(e) = self.on_record(&rec) {
                        self.fail(now, 0x2c, e);
                        return;
                    }
                }
            }
            // SPEC: UDP_SESSION_2009_B.md §4.3, §9.1 (entries to the end; time entry; only a later time is accepted)
            10 => self.on_stream(now, &mut r),
            // SPEC: UDP_SESSION_2009_B.md §6.3 (id 17: an effect record without an object)
            17 => {
                let t = r.read_u8();
                let n = r.remaining_bits();
                self.seen.effects.push((now, t, r.read_bit_string(n)));
            }
            // SPEC: UDP_SESSION_2009_B.md §6.2 (shell messages)
            13 => {
                let shell = r.read_u8();
                self.seen.shell_ids.push((now, shell));
                if let Err(e) = self.on_shell(now, shell, &mut r) {
                    self.seen.errors.push(format!("shell {shell:#04x}: {e}"));
                }
            }
            _ => {}
        }
    }

    /// The stream: remote objects with position / velocity / rotation, and the time entry.
    // SPEC: UDP_SESSION_2009_B.md §4.3 (reader engine+0xa4160)
    fn on_stream(&mut self, now: Millis, r: &mut BitReader<'_>) {
        while r.remaining_bits() >= 20 {
            let obj = r.read_u16();
            let uf = r.read_bits(4);
            if obj == 0xFFFF && uf == 0 {
                let t = r.read_f32();
                let last = self.seen.server_times.last().map(|x| x.1);
                if last.is_none_or(|l| t > l) {
                    self.seen.server_times.push((now, t));
                } else {
                    self.seen.stale_times += 1;
                }
                continue;
            }
            let mut e = self.seen.remote.get(&obj).copied().unwrap_or(SeenRemote {
                at: now,
                pos: [0.0; 3],
                vel: [0.0; 3],
                yaw: None,
                entries: 0,
                yaw8_entries: 0,
                anim_entries: 0,
                last_anim: [0; 3],
                last_main: None,
                last_upper: None,
            });
            if uf & 0x4 != 0 {
                let p = Pos48::read(r);
                e.pos = match self.cfg.box_bounds {
                    // SPEC: UDP_TRANSPORT_2009_addendum_disasm.md T14 (decode, precise = 1, extra 0)
                    Some((lo, hi)) => p.decode(lo, hi, true, 0),
                    None => [p.px as f32, p.py as f32, p.pz as f32],
                };
                e.vel = if r.read_bit() {
                    Vec72::read(r).decode_presumed()
                } else {
                    [0.0; 3]
                };
                let trail = self.seen.trails.entry(obj).or_default();
                trail.push_back((now, e.pos));
                while trail
                    .front()
                    .is_some_and(|x| since(now, x.0) > TRAIL_KEEP_MS)
                {
                    trail.pop_front();
                }
            }
            if uf & 0x8 != 0 {
                e.yaw = Some(dc_wire::udp::yaw8_to_radians(r.read_s8()));
                e.yaw8_entries += 1;
            } else if uf & 0x2 != 0 {
                e.yaw = Some(dc_wire::udp::quat_yaw(read_quat128(r)));
            }
            if uf & 0x1 != 0 {
                // SPEC: UDP_SESSION_2009_B.md §4.3 (UF_ANIM on a known object that is not a model: the reader returns)
                if self.seen.objects.get(&obj).is_some_and(|o| o.kind != 1) {
                    self.seen.errors.push(format!(
                        "stream animation block for the non-model object {obj}"
                    ));
                    return;
                }
                let a = read_anim_trackers(r);
                e.anim_entries += 1;
                for (i, v) in a.iter().take(3).enumerate() {
                    e.last_anim[i] = v.0;
                }
                e.last_main = a.first().copied();
                e.last_upper = a.get(1).copied();
            }
            e.at = now;
            e.entries += 1;
            self.seen.remote.insert(obj, e);
        }
    }

    /// Shell messages the client handles; layouts from the client-side readers of `GAME_MESSAGES_2009_S2C.md`.
    /// Fully known layouts must be consumed exactly.
    fn on_shell(&mut self, now: Millis, shell: u8, r: &mut BitReader<'_>) -> Result<(), String> {
        let exact = |r: &BitReader<'_>| {
            if r.at_end() {
                Ok(())
            } else {
                Err(format!("{} bits left", r.remaining_bits()))
            }
        };
        match shell {
            // SPEC: UDP_SESSION_2009_A.md §4.11
            SHELL_SWITCH => {
                let v = r.read_u8();
                self.seen.switch_state = Some(v);
                self.seen.switch_states.push(v);
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §4.1
            0x67 => {
                let s = r.read_u8();
                self.life_state = s;
                self.seen.life_states.push((now, s));
                // DISASM: cshell+0x619d5..0x619de (state 2 stores the death time), cshell+0x618e0 (state 1 clears it);
                // state 3 does not touch it (cshell+0x61975)
                if s == 2 {
                    self.seen.lives_at_death.push(self.seen.own_lives);
                }
                if s == 2 && self.death_time.is_none() {
                    self.death_time = Some(now);
                    self.self_armed = false;
                }
                if s == 1 {
                    self.death_time = None;
                    self.armed = None;
                }
                exact(r)?;
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §5.1 (answered with UP_PLAYER_INFO, body ignored)
            0x72 => {
                self.seen.identify_requests += 1;
                if self.cfg.answer_identify {
                    self.send_player_info();
                }
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1
            0x6b => {
                let name = r.read_zstr8();
                let pid = r.read_u8();
                let account = r.read_u32();
                r.read_bit();
                let character = r.read_u16();
                let mut look = [0i8; 15];
                for b in &mut look {
                    *b = r.read_s8();
                }
                r.read_zstr8();
                r.read_zstr8();
                let kills = r.read_s16();
                let deaths = r.read_u8();
                r.skip_bits(24);
                let team = r.read_u8();
                let dead = r.read_bit();
                let kind = r.read_u8();
                let clan = if r.read_bit() {
                    let n = String::from_utf16_lossy(&r.read_zstr16());
                    let no = r.read_u64();
                    Some((n, no, [r.read_u32(), r.read_u32(), r.read_u32()]))
                } else {
                    None
                };
                r.skip_bits(32);
                let rank = r.read_u32();
                r.skip_bits(3 + 8);
                let m1 = r.read_u8();
                r.skip_bits(32 + 32 + 16);
                exact(r)?;
                // SPEC: FIRETEAM_2009.md §2.1 (field 21 `m1` = record +0x1f8)
                if Some(pid as u16) == self.seen.client_id.map(|c| c.0)
                    && self.mode_class() == Some(5)
                {
                    self.seen.own_lives = Some(m1);
                    self.seen.lives_log.push((now, 0x6b, m1));
                }
                if !self.seen.objects.contains_key(&character) {
                    return Err(format!("record of {pid} names unknown object {character}"));
                }
                if Some(pid as u16) == self.seen.client_id.map(|c| c.0) {
                    self.seen.own_team = Some(team);
                }
                self.seen.roster_log.push((now, pid, kind, dead));
                self.seen.roster.insert(
                    pid,
                    SeenRecord {
                        name,
                        account,
                        character,
                        kills,
                        deaths,
                        team,
                        dead,
                        kind,
                        look,
                        rank,
                        clan,
                    },
                );
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §3.2 (0x7d; mode classes 7 / 8 not modelled here)
            0x7d => {
                r.read_zstr8();
                r.read_u8();
                let own = r.read_s8();
                // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1.5 (the client's counter := own_value)
                let seed = r.read_u32();
                self.seen.shot_seed = Some(seed);
                self.seen.shot_counter = seed;
                let goal = r.read_u8();
                r.skip_bits(16);
                let limit = r.read_s8();
                r.read_s8();
                exact(r)?;
                self.seen.settings = Some((own, goal, limit));
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §2.1.1 (the intro block by mode class: caption for classes 6, 9, 11 and 5,
            // team bases unless class 6, three more fields for class 5)
            0x45 => {
                let secs = r.read_u32();
                let intro = r.read_bit();
                let banner = r.read_bit();
                r.read_u32();
                self.seen.match_begins.push(now);
                // DISASM: cshell+0x49f00..0x49f2d (after the countdown read: the voice only in player state 1)
                if self.life_state == 1 {
                    let flag90 = self.seen.flag90.last().copied().unwrap_or(false);
                    self.seen.round_start_cues.push((now, banner, flag90));
                }
                if intro {
                    let class = self.mode_class().unwrap_or(1);
                    let caption = if matches!(class, 5 | 6 | 9 | 11) {
                        r.read_zstr8()
                    } else {
                        Vec::new()
                    };
                    let head = r.read_zstr8();
                    let _body = r.read_zstr8();
                    let map_a = r.read_zstr8();
                    r.skip_bits(192);
                    let radar = r.read_zstr8();
                    let map_b = r.read_zstr8();
                    r.skip_bits(192 + 128);
                    if class != 6 {
                        r.skip_bits(192);
                    }
                    if class == 5 {
                        r.skip_bits(17);
                    }
                    self.seen.intro = Some([caption, head, map_a, map_b, radar]);
                }
                r.read_u32();
                exact(r)?;
                self.seen.match_begin = Some((secs, banner));
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (0x4a)
            0x4a => {
                let (a, b) = (r.read_u8(), r.read_u8());
                exact(r)?;
                self.seen.team_scores.push((now, a, b));
            }
            // SPEC: MOVEMENT_2009.md §12.4 (blocks 0x0020, 0x0004, 0x0001, 0x0008, 0x0080, 0x0100 in this order)
            0xa0 => {
                let flags = r.read_u16();
                if flags & !0x01ad != 0 {
                    return Err(format!("mover flags {flags:#06x} not modelled"));
                }
                if flags & 0x0020 != 0 {
                    let seq = r.read_u8();
                    let pos = vec3(r);
                    let dims = vec3(r);
                    self.move_seq = seq;
                    self.pos = pos;
                    self.seen.teleports.push((now, seq, pos, dims));
                }
                if flags & 0x0004 != 0 {
                    let n = r.read_u8();
                    r.skip_bits(n as usize * (8 + 96 + 32 + 32 + 8 + 16 + 8));
                    self.seen.frictions.push(r.read_f32());
                }
                if flags & 0x0001 != 0 {
                    let model = r.read_zstr8();
                    let n = r.read_u8();
                    let skins = (0..n).map(|_| r.read_zstr8()).collect();
                    r.read_bit();
                    self.seen.body_blocks += 1;
                    self.seen.bodies.push((model, skins));
                }
                if flags & 0x0008 != 0 {
                    self.seen.frictions.push(r.read_f32());
                }
                if flags & 0x0080 != 0 {
                    self.seen.freezes.push(r.read_bit());
                }
                if flags & 0x0100 != 0 {
                    r.read_bit();
                }
                exact(r)?;
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §7.2 (0x90 `[flag:bool1]`)
            0x90 => {
                self.seen.flag90.push(r.read_bit());
                exact(r)?;
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §5.1 (0x9e)
            0x9e => {
                let mode = r.read_u8();
                if mode != 0 {
                    r.read_zstr8();
                    r.read_u32();
                }
                exact(r)?;
                self.seen.endpoint = Some(mode);
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (0x43 no body, 0x44)
            0x43 => exact(r)?,
            0x44 => {
                let secs = r.read_u32();
                let no = r.read_u32();
                let limit = r.read_u32();
                r.read_bit();
                exact(r)?;
                self.seen.round_begin = Some(secs);
                self.seen.round_begins.push((now, secs, no, limit));
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (0x7f: weapon = command table [command - 0x14]; auto switches)
            0x7f => {
                let cmd = r.read_u8();
                let auto = r.read_u8() != 0;
                r.read_f32();
                r.read_u8();
                exact(r)?;
                self.seen.weapon_switches.push((cmd, auto));
                if auto {
                    let w = match &self.cfg.weapon_commands {
                        Some(t) => t.get((cmd as usize).wrapping_sub(0x14)).copied(),
                        None => self.given.get(&0).map(|g| g.0),
                    };
                    if let Some(w) = w {
                        let ammo = self.given.values().find(|g| g.0 == w).map_or(0, |g| g.1);
                        self.weapon = (w, ammo);
                    }
                }
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §4.2 (0x6a)
            0x6a => {
                r.read_u8();
                self.seen.angles = Some(vec3(r));
                exact(r)?;
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (0x66 generic kinds)
            0x66 => {
                let kind = r.read_u8();
                if kind == 1 {
                    // SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (kind 1 `[weapon:u16][ammo_kind:u16][ammo:f32][slot:u8][extra:u16]`)
                    let (w, a, v, s, x) = (
                        r.read_u16(),
                        r.read_u16(),
                        r.read_f32(),
                        r.read_u8(),
                        r.read_u16(),
                    );
                    exact(r)?;
                    self.seen.ammo_notes.push((now, w, a, v, s, x));
                    return Ok(());
                }
                if matches!(kind, 0x0c | 0x14) {
                    return Err(format!("status kind {kind} not modelled"));
                }
                r.skip_bits(16);
                let v = r.read_f32();
                exact(r)?;
                match kind {
                    2 => self.seen.health = Some(v),
                    3 => self.seen.armor = Some(v),
                    0x0f => self.seen.max_health = Some(v),
                    _ => {}
                }
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (0xb2 / 0xb3)
            0xb2 | 0xb3 => {
                let w = r.read_u16();
                let a = r.read_u16();
                let amount = r.read_f32();
                if shell == 0xb3 {
                    r.read_u32();
                    self.weapon = (w, a);
                }
                r.read_u8();
                let slot = r.read_u8();
                let parts = [r.read_s8(), r.read_s8(), r.read_s8(), r.read_s8()];
                exact(r)?;
                self.seen.weapon_parts.push((w, parts));
                if shell == 0xb2 {
                    self.given.insert(slot, (w, a));
                }
                self.seen.weapons.push((shell, w, a));
                self.seen.weapon_amounts.push((w, amount));
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (0x74)
            0x74 => {
                let from_dir = vec3(r);
                let f = SeenDamage {
                    at: now,
                    from_dir,
                    unused: r.read_u8(),
                    mode: r.read_u8(),
                    ammo_kind: r.read_u16(),
                    dmg_kind: r.read_u8(),
                    zone: r.read_u8(),
                    part: r.read_s8(),
                    damage: r.read_u32(),
                };
                exact(r)?;
                self.seen.damage.push((now, f.damage));
                self.seen.damage_forms.push(f);
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 (no revenge block expected)
            0x6d => {
                let victim = r.read_u8();
                let killer = r.read_u8();
                let kill_kind = r.read_u8();
                let weapon = r.read_u16();
                let zone = r.read_u8();
                let relation = r.read_bit();
                r.skip_bits(3);
                let streak_flag = r.read_u8();
                let feed_value = r.read_u8();
                r.read_bit();
                if r.read_bit() {
                    r.read_u32();
                }
                r.read_bit();
                r.skip_bits(64);
                exact(r)?;
                self.seen.kills.push(SeenKill {
                    at: now,
                    victim,
                    killer,
                    weapon,
                    zone,
                    kill_kind,
                    relation,
                    streak_flag,
                    feed_value,
                });
                // the client counts kills and deaths itself (§3.3.1 step 6): a foe kill counts the killer's kill and
                // the victim's death, a team kill nothing, a death without a killer (or by the own hand) the death
                // EVIDENCE: cshell+0x2dd9a..0x2dda0 (same team: no counting), cshell+0x2df08..0x2df3a (no killer)
                let teams = (
                    self.seen.roster.get(&killer).map(|k| k.team),
                    self.seen.roster.get(&victim).map(|v| v.team),
                );
                let by_other = killer != 0xff && killer != victim;
                let counted = !by_other || teams.0.is_none() || teams.0 != teams.1;
                if let Some(v) = self.seen.roster.get_mut(&victim) {
                    if counted {
                        v.deaths = v.deaths.wrapping_add(1);
                    }
                    v.dead = true;
                }
                if by_other && counted {
                    if let Some(k) = self.seen.roster.get_mut(&killer) {
                        k.kills = k.kills.wrapping_add(1);
                    }
                }
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §3.3 (0xc8)
            0xc8 => {
                r.read_u16();
                r.read_bit();
                self.seen.multi_kills.push(r.read_u32());
                exact(r)?;
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (0x41, 0x3f), §2.1.2 (0x46, not the co-op class)
            0x41 => {
                r.read_u32();
                exact(r)?;
            }
            0x46 => {
                let secs = r.read_u32();
                let w = r.read_s8();
                let (a, b) = (r.read_u8(), r.read_u8());
                // SPEC: GAME_MESSAGES_2009_S2C.md §2.1.2 (class 5: stage, six values; stage 2: names, title, text,
                // panel time)
                if self.mode_class() == Some(5) {
                    let stage = r.read_u8();
                    let mut v = [0u32; 6];
                    for x in v.iter_mut() {
                        *x = r.read_u32();
                    }
                    let mut names = Vec::new();
                    if stage == 2 {
                        let n = r.read_u32();
                        if n > 64 {
                            return Err(format!("co-op result with {n} names"));
                        }
                        for _ in 0..n {
                            names.push(r.read_zstr8());
                        }
                        r.read_zstr8();
                        r.read_zstr8();
                        r.read_u32();
                    }
                    self.seen.coop_result = Some((stage, v, names));
                }
                exact(r)?;
                self.seen.round_result = Some((w, a, b));
                self.seen.round_results.push((now, w, a, b));
                self.seen.round_result_secs = Some(secs);
            }
            0x3f => {
                let flag = r.read_bit();
                let value = r.read_u32();
                exact(r)?;
                self.seen.result_screen = Some(now);
                self.seen.result_value = Some((flag, value));
                if let Some(ms) = self.cfg.leave_after_result_ms {
                    self.leave_at = Some(now.wrapping_add(ms));
                }
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §2.1 rows 0x47 / 0x48 (`[u32]` each)
            0x47 => {
                r.read_u32();
                exact(r)?;
                self.seen.objects_end = Some(now);
            }
            0x48 => {
                let secs = r.read_u32();
                exact(r)?;
                self.seen.round_close = Some((now, secs));
            }
            // shell 0x9d `[u32]` (the reference stack's form; no client handler catalogued)
            0x9d => {
                let v = r.read_u32();
                exact(r)?;
                self.seen.shell_9d.push(v);
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §3.4 (0x65 not the co-op class; 0xb5)
            0x65 => {
                let pid = r.read_u8();
                let obj = r.read_u16();
                let before = r.read_u32();
                let exp = r.read_u32();
                let exp_bonus = r.read_u32();
                let gp = r.read_u32();
                let gp_bonus = r.read_u32();
                r.skip_bits(1);
                // SPEC: GAME_MESSAGES_2009_S2C.md §3.4 (class 5: [c1][n] n x [bool1] [c2][c3][c4])
                if self.mode_class() == Some(5) {
                    let c1 = r.read_u32();
                    let n = r.read_u32();
                    if n > 1024 {
                        return Err(format!("co-op summary with {n} flags"));
                    }
                    r.skip_bits(n as usize);
                    let (c2, c3, c4) = (r.read_u32(), r.read_u32(), r.read_u32());
                    self.seen.coop_summaries.push((pid, [c1, c2, c3, c4]));
                }
                exact(r)?;
                self.seen.summaries.push(pid);
                self.seen.summary_rows.push((pid, obj, before, exp, gp));
                self.seen.summary_bonus.push((pid, exp_bonus, gp_bonus));
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §3.5 (0xd7); FIRETEAM_2009.md §2.2 (sub 2 re-arm: a dead state and a == b)
            0xd7 => {
                match r.read_u8() {
                    1 => {
                        let (pid, obj, x, pts) =
                            (r.read_u32(), r.read_u16(), r.read_u32(), r.read_u32());
                        self.seen.coop_points.push((now, pid, obj, x, pts));
                    }
                    2 => {
                        let (a, b) = (r.read_u8(), r.read_u8());
                        self.seen.own_lives = Some(b);
                        self.seen.lives_log.push((now, 0xd7, b));
                        // DISASM: cshell+0x5df50 (re-arm the 7000 ms wait when dead and a == the stored b)
                        if matches!(self.life_state, 2 | 3 | 5 | 6)
                            && a == b
                            && b > 0
                            && self.cfg.self_arm
                        {
                            let w = self.cfg.respawn_wait_ms.unwrap_or(7_000);
                            self.armed = Some((now, w));
                            self.self_armed = true;
                            self.seen.respawn_armed.push((now, w));
                        }
                    }
                    3 => {
                        let (pid, x, pts) = (r.read_u32(), r.read_u8(), r.read_u32());
                        self.seen.coop_notes.push((now, pid, x, pts));
                    }
                    4 => {
                        let (run, secs) = (r.read_bit(), r.read_u32());
                        self.seen.coop_timers.push((run, secs));
                    }
                    5 => {}
                    other => return Err(format!("0xd7 sub {other}")),
                }
                exact(r)?;
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §6.2 (0xc9 subs 1 and 2; the others are not sent by the server)
            0xc9 => {
                match r.read_u8() {
                    1 => {
                        let n = r.read_u32();
                        if n > 256 {
                            return Err(format!("0xc9 sub 1 with {n} goals"));
                        }
                        for _ in 0..n {
                            r.read_u32();
                            r.read_zstr8();
                            r.read_u8();
                        }
                        let m = r.read_u32();
                        if m > 256 {
                            return Err(format!("0xc9 sub 1 with {m} timers"));
                        }
                        let timers = (0..m).map(|_| r.read_f32()).collect();
                        let (a, b) = (r.read_u8(), r.read_u8());
                        self.seen.missions.push((n, timers, a, b));
                    }
                    2 => {
                        let t = r.read_zstr8();
                        r.read_f32();
                        self.seen.banners.push(t);
                    }
                    // DISASM: cshell+0x81fe0 (the caption: three `|` fields, else nothing is shown)
                    3 => {
                        let t = r.read_zstr8();
                        let secs = r.read_f32();
                        let fields = t.split(|&b| b == b'|').filter(|f| !f.is_empty()).count();
                        if !(t.iter().filter(|&&b| b == b'|').count() == 2 && fields == 3) {
                            return Err("0xc9 sub 3 caption without three fields".into());
                        }
                        self.seen.captions.push((now, t, secs));
                    }
                    other => return Err(format!("0xc9 sub {other} not modelled")),
                }
                exact(r)?;
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §4.2 row 0xd9 (op 0 stop; op 1 loop bit, 32-bit value, file)
            0xd9 => {
                match r.read_u8() {
                    0 => self.seen.music.push((now, None)),
                    1 => {
                        let looped = r.read_bit();
                        let fade = r.read_f32();
                        let file = r.read_zstr8();
                        self.seen.music.push((now, Some((file, looped, fade))));
                    }
                    other => return Err(format!("0xd9 op {other}")),
                }
                exact(r)?;
            }
            // SPEC: FIRETEAM_2009.md §1.5 (0xa6 type 0x18 sub 0x24 reads 32 bits only for objects with the NPC tail)
            0xa6 => {
                let kind = r.read_u8();
                let obj = r.read_u16();
                let sub = r.read_u8();
                // DISASM: cshell+0x20fd00 (note kind 0x0d to the cameras), cshell+0x216800 (sub 1: three f32)
                if kind == crate::camshot::OBJ_CAMERA_TYPE {
                    if !self.seen.objects.contains_key(&obj)
                        || sub != crate::camshot::CAMERA_NOTE_FOV
                    {
                        return Err(format!("0xa6 camera note sub {sub} to object {obj}"));
                    }
                    let fov = [r.read_f32(), r.read_f32()];
                    let secs = r.read_f32();
                    self.seen.camera_fov.push((now, obj, fov, secs));
                    exact(r)?;
                }
                if kind == 0x18 && sub == 0x24 {
                    let has_tail = self
                        .seen
                        .objects
                        .get(&obj)
                        .and_then(|o| o.blob.as_ref())
                        .is_some_and(|b| {
                            let mut br = b.reader();
                            br.read_u8() == 0x18 && {
                                br.read_bit();
                                br.read_bit()
                            }
                        });
                    if !has_tail {
                        return Err(format!(
                            "0xa6 sub 0x24 to object {obj} without the NPC tail"
                        ));
                    }
                    let f = f32::from_bits(r.read_u32());
                    self.seen.npc_health.push((now, obj, f));
                    exact(r)?;
                }
                // SPEC: GAME_MESSAGES_2009_S2C.md §8.3 (type 0x18 sub 0x1d has no body, cshell+0x206260)
                // DISASM: cshell+0x26f60 (sub 0x80: six 32-bit words, cshell+0x259b0)
                if kind == 0x18 && sub == crate::gs::NOTE_AIM {
                    if !self.seen.objects.contains_key(&obj) {
                        return Err(format!("0xa6 sub 0x80 to unknown object {obj}"));
                    }
                    let mut w = [0u32; 6];
                    for x in &mut w {
                        *x = r.read_u32();
                    }
                    self.seen.aim_notes.push((now, obj, w));
                    exact(r)?;
                }
                if kind == 0x18 && sub == crate::gs::NOTE_DEATH {
                    self.seen.death_notes.push((now, obj));
                    exact(r)?;
                }
                // SPEC: GAME_MESSAGES_2009_S2C.md §8.3 (type 0x17 sub 2 `[remaining:u32]`, cshell+0x2828e0); a note to
                // an object the client has not created is dropped (cshell+0x20fbe0), which a server must avoid
                // SPEC: GAME_MESSAGES_2009_S2C.md §8.3 (type 0x18 subs 0x1f / 0x25 have no body)
                if kind == 0x18 && (sub == 0x1f || sub == 0x25) {
                    if !self.seen.objects.contains_key(&obj) {
                        return Err(format!("0xa6 sub {sub:#x} to unknown object {obj}"));
                    }
                    self.seen.spawn_notes.push((now, obj, sub));
                    exact(r)?;
                }
                // DISASM: cshell+0x2e1a70 (type 0x39: sub 1 `read(1)`, sub 2 `read(8)`)
                if kind == crate::doors::OBJ_DOOR_TYPE {
                    if !self.seen.objects.contains_key(&obj) {
                        return Err(format!("0xa6 type 0x39 to unknown object {obj}"));
                    }
                    let v = match sub {
                        crate::doors::NOTE_DISABLED => u8::from(r.read_bit()),
                        crate::doors::NOTE_STATE => r.read_u8(),
                        _ => return Err(format!("0xa6 type 0x39 sub {sub}")),
                    };
                    self.seen.door_notes.push((now, obj, sub, v));
                    exact(r)?;
                }
                if kind == 0x17 && sub == 2 {
                    if !self.seen.objects.contains_key(&obj) {
                        return Err(format!("0xa6 type 0x17 to unknown object {obj}"));
                    }
                    let left = r.read_u32();
                    self.seen.case_notes.push((now, obj, left));
                    exact(r)?;
                }
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §3.5 row 0xd6; HISEC_2009.md §3.2 (sub 1 / 2 read two u32, sub 0 none)
            0xd6 => {
                let sub = r.read_u8();
                let (key, who) = if sub == 1 || sub == 2 {
                    (r.read_u32(), r.read_u32())
                } else {
                    (0, 0)
                };
                exact(r)?;
                self.seen.hisec.push((now, sub, key, who));
            }
            0xb5 => {
                let (a, b) = (r.read_u8(), r.read_u8());
                exact(r)?;
                self.seen.round_wins = Some((a, b));
                self.seen.team_kills.push((now, a, b));
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §2.3 (0x5e: scores, the own team 1 <-> 2; 0x5f kind 1 with `no_swap` 0:
            // the same; kinds 2 / 3: chat notices)
            0x5e | 0x5f => {
                let swap = if shell == 0x5e {
                    let sc = [r.read_u8(), r.read_u8()];
                    Some(sc)
                } else {
                    let kind = r.read_u8();
                    if kind == 1 {
                        let sc = [r.read_u8(), r.read_u8()];
                        let no_swap = r.read_bit();
                        (!no_swap).then_some(sc)
                    } else {
                        None
                    }
                };
                exact(r)?;
                if let Some(sc) = swap {
                    if let Some(t) = self.seen.own_team.filter(|t| *t == 1 || *t == 2) {
                        self.seen.own_team = Some(3 - t);
                    }
                    self.seen
                        .team_swaps
                        .push((now, shell, sc, self.seen.own_team));
                }
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §6.1 (0x99)
            0x99 => {
                r.read_zstr8();
                r.skip_bits(2);
                exact(r)?;
                self.seen.level_exit = Some(now);
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §3.2 (0xa2 ends with pid 0xFFFF; 0x6c)
            0xa2 => {
                let mut rows = Vec::new();
                loop {
                    let pid = r.read_u16();
                    if pid == 0xFFFF {
                        break;
                    }
                    rows.push((pid, r.read_u16()));
                    if r.at_end() {
                        return Err("ping table without end marker".into());
                    }
                }
                exact(r)?;
                self.seen.ping_tables.push((now, rows));
            }
            0x6c => {
                let pid = r.read_u8();
                r.read_u8();
                exact(r)?;
                self.seen.roster.remove(&pid);
                self.seen.players_left.push(pid);
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §2.4 (0x68)
            0x68 => {
                let name = String::from_utf16_lossy(&r.read_zstr16());
                let text = String::from_utf16_lossy(&r.read_zstr16());
                let sender = r.read_u32();
                let team = r.read_u8() != 0;
                r.read_u32();
                exact(r)?;
                self.seen.chat.push((name, text, sender, team));
            }
            // SPEC: GAME_MESSAGES_2009_C2S.md §5.8 (0xc3 sub 1 enable; sub 2 / 3 handled by the client)
            0xc3 => {
                let sub = r.read_u8();
                r.read_u32();
                if sub == 1 {
                    self.seen.perf_enabled = r.read_u8() == 1;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// One object record: create records (any kind; kind 1 with blob, model block, animation, attachments and
    /// dims), change records for known ids, the removed-objects special record. Colour and scale blocks are read
    /// past.
    // SPEC: UDP_SESSION_2009_B.md §3.2, §3.3, §3.4, §4.2, §4.4, §5.1 (readers engine+0xa3cb0 / engine+0xa32f0 /
    // engine+0xa2530 / engine+0xa2980)
    /// Reads one `S2C_OBJ_GROUP` message (`[8]` + records) as the client would (tests of record writers).
    pub fn read_group_for_test(&mut self, group: &BitWriter) -> Result<(), String> {
        let mut r = group.reader();
        if r.read_u8() != 8 {
            return Err("not an object group".into());
        }
        while r.remaining_bits() >= 32 {
            let n = r.read_u32() as usize;
            if n > r.remaining_bits() {
                return Err("record past the end".into());
            }
            let rec = r.read_bit_string(n);
            self.on_record(&rec)?;
        }
        Ok(())
    }

    /// Tracked-sound record body after `[0][1]`: the fields `dmask` carries go into the per-file cache, the others
    /// come from it; the record must end exactly after its fields.
    // SPEC: UDP_SESSION_2009_B.md §8.1, §8.3
    fn on_tracked_sound(&mut self, r: &mut BitReader<'_>) -> Result<(), String> {
        let tmask = r.read_u8();
        let track = r.read_u16();
        let mut s = self.seen.sounds.get(&track).copied().unwrap_or(SeenSound {
            tmask,
            track,
            sfile: 0,
            dmask: 0,
            sflags: 0,
            prio: 0,
            outer: 0,
            ratio: 0,
            volume: None,
            pitch: None,
            tag: 0,
            pos48: None,
            pos: None,
            records: 0,
        });
        s.tmask = tmask;
        s.records += 1;
        if tmask & 1 != 0 {
            s.sfile = r.read_u16();
            s.dmask = r.read_u8();
            let (mut fl, mut pr, mut ou, mut ra) = self
                .seen
                .sound_cache
                .get(&s.sfile)
                .copied()
                .unwrap_or_default();
            if s.dmask & 1 != 0 {
                fl = r.read_u16();
            }
            if s.dmask & 2 != 0 {
                pr = r.read_u8();
            }
            if fl & 3 != 0 && s.dmask & 4 != 0 {
                ou = r.read_u16();
                ra = r.read_u8();
            }
            self.seen.sound_cache.insert(s.sfile, (fl, pr, ou, ra));
            self.seen.sound_starts.push((track, s.sfile));
            (s.sflags, s.prio, s.outer, s.ratio) = (fl, pr, ou, ra);
            s.volume = (fl & 0x40 != 0).then(|| r.read_u8());
            s.pitch = (fl & 0x80 != 0).then(|| r.read_f32());
            if fl & 0x400 != 0 {
                let b = r.read_u8();
                if b == 0xFF {
                    r.read_u32();
                }
            }
            s.tag = r.read_u32();
        }
        if tmask & 2 != 0 {
            let p = Pos48::read(r);
            s.pos48 = Some([p.px, p.py, p.pz]);
            s.pos = self
                .cfg
                .box_bounds
                .map(|(lo, hi)| p.decode(lo, hi, true, 0));
        }
        if r.remaining_bits() != 0 {
            return Err(format!(
                "tracked sound {track}: {} bits left over",
                r.remaining_bits()
            ));
        }
        self.seen.sounds.insert(track, s);
        Ok(())
    }

    fn on_record(&mut self, rec: &BitWriter) -> Result<(), String> {
        let mut r = rec.reader();
        let lo = r.read_u8();
        if lo == 0 {
            let kind = r.read_u8();
            if kind == 1 {
                return self.on_tracked_sound(&mut r);
            }
            if kind == 3 {
                while r.remaining_bits() >= 16 {
                    let o = r.read_u16();
                    if self.seen.objects.remove(&o).is_some() {
                        self.seen.remote.remove(&o);
                        self.seen.removed.push(o);
                    } else if self.seen.sounds.contains_key(&o) {
                        self.seen.sounds_removed.push(o);
                    } else {
                        self.seen.removed_unknown.push(o);
                    }
                }
            }
            return Ok(());
        }
        let mut cf = lo as u16;
        if lo & 0x80 != 0 {
            cf = (cf & 0x7F) | ((r.read_u8() as u16) << 8);
        }
        let obj = r.read_u16();
        let mut seen = if cf & 1 != 0 {
            let kind_byte = r.read_u8();
            let kind = kind_byte & 0x3F;
            let create_pos = [r.read_f32(), r.read_f32(), r.read_f32()];
            let mut blob = None;
            if kind_byte & 0x40 != 0 {
                let n = if kind_byte & 0x80 != 0 {
                    r.read_u16() as usize
                } else {
                    r.read_u8() as usize
                };
                blob = Some(r.read_bit_string(n));
            }
            let mut models = Vec::new();
            match kind {
                1 => models = read_model_block(&mut r)?,
                2 => {
                    r.read_zstr8();
                }
                9 => {
                    r.read_zstr8();
                    r.read_u16();
                }
                3 => {
                    r.read_u16();
                }
                _ => {}
            }
            cf |= 0x0200;
            SeenObject {
                kind,
                create_pos,
                pos: None,
                flags: None,
                blob_type: blob.as_ref().map(|b| b.reader().read_u8()),
                blob_pid: blob.as_ref().and_then(character_blob_pid),
                models,
                attached: Vec::new(),
                dims: None,
                anim: Vec::new(),
                records: 0,
                last_cf: 0,
                blob,
                uflags: None,
            }
        } else {
            self.seen
                .objects
                .get(&obj)
                .cloned()
                .ok_or_else(|| format!("change record for unknown object {obj}"))?
        };
        seen.records += 1;
        seen.last_cf = cf;
        if cf & 0x0800 != 0 {
            match seen.kind {
                1 => seen.models = read_model_block(&mut r)?,
                3 => {
                    r.read_u16();
                }
                _ => {}
            }
        }
        if cf & 0x2020 != 0 {
            match seen.kind {
                1 => seen.anim = read_anim_block(&mut r),
                3 => {
                    r.read_u64();
                }
                _ => {}
            }
        }
        if cf & 0x0008 != 0 {
            let flags = r.read_u16();
            r.skip_bits(16);
            seen.uflags = Some(r.read_u32());
            r.skip_bits(32);
            seen.flags = Some(flags);
        }
        // SPEC: UDP_SESSION_2009_B.md §4.2 item 4 (`[r][g][b][a][extra]`, kind 4 adds `[radius:u16]`; the writer
        // drops the block for kinds 0, 5 and 9)
        if cf & 0x0040 != 0 && !matches!(seen.kind, 0 | 5 | 9) {
            r.skip_bits(40 + if seen.kind == 4 { 16 } else { 0 });
        }
        // SPEC: UDP_SESSION_2009_B.md §4.2 item 5 (`[sx][sy]`, then `[sz]` except for kind 3)
        if cf & 0x0010 != 0 {
            r.skip_bits(if seen.kind == 3 { 64 } else { 96 });
        }
        if cf & 0x0202 != 0 {
            let full = seen.flags.is_some_and(|f| f & 0x0100 != 0);
            if full {
                seen.pos = Some([r.read_f32(), r.read_f32(), r.read_f32()]);
                r.skip_bits(96);
            } else {
                let p = Pos48::read(&mut r);
                r.skip_bits(72);
                seen.pos = Some(match self.cfg.box_bounds {
                    // SPEC: UDP_TRANSPORT_2009_addendum_disasm.md T14 (decode, precise = 1, extra 0)
                    Some((lo, hi)) => p.decode(lo, hi, true, 0),
                    None => seen.create_pos,
                });
            }
        }
        if cf & 0x0404 != 0 {
            r.skip_bits(128);
        }
        if cf & 0x0100 != 0 {
            seen.attached.clear();
            loop {
                if r.remaining_bits() < 16 {
                    return Err("attachment list without its end".into());
                }
                let child = r.read_u16();
                if child == 0xFFFF {
                    break;
                }
                let socket = r.read_u32();
                r.skip_bits(96 + 128);
                seen.attached.push((child, socket));
            }
            if seen.kind == 1 {
                r.skip_bits(64 * 32);
            }
        }
        if cf & 0x8000 != 0 {
            seen.dims = Some([r.read_f32(), r.read_f32(), r.read_f32()]);
        }
        if !r.at_end() {
            return Err(format!(
                "{} bits left in the record of object {obj}",
                r.remaining_bits()
            ));
        }
        self.seen.objects.insert(obj, seen);
        Ok(())
    }
}

/// The `pid` field of a type-0x18 character record (`[0x18][a:1][b:1][5 x u8][pid:u8]...`).
// EVIDENCE: cshell+0x2b6aa0 (field order and widths)
fn character_blob_pid(b: &BitWriter) -> Option<u8> {
    let mut r = b.reader();
    if b.bit_len() < 8 + 2 + 48 || r.read_u8() != 0x18 {
        return None;
    }
    r.skip_bits(2 + 40);
    Some(r.read_u8())
}

/// Model block: the model file ids (first model's sub files after it).
// SPEC: UDP_SESSION_2009_B.md §3.4 (reader engine+0xa2870 / engine+0xa2700; model_count 0 -> code 1)
fn read_model_block(r: &mut BitReader<'_>) -> Result<Vec<u16>, String> {
    let models = r.read_bits(6);
    if models == 0 {
        return Err("model block with no model".into());
    }
    let mut files = Vec::new();
    for m in 0..models {
        files.push(r.read_u16());
        if m == 0 {
            let subs = r.read_bits(6);
            for _ in 0..subs.saturating_sub(1) {
                files.push(r.read_u16());
            }
        }
        let tex = r.read_bits(6);
        r.skip_bits(16 * tex as usize);
        let styles = r.read_bits(4);
        r.skip_bits(16 * styles as usize);
    }
    Ok(files)
}

/// Animation block: the animation index of every tracker (main first).
// SPEC: UDP_SESSION_2009_B.md §4.4 (reader engine+0xa2530)
fn read_anim_block(r: &mut BitReader<'_>) -> Vec<u16> {
    read_anim_trackers(r).into_iter().map(|t| t.0).collect()
}

/// Animation block: `(animation, state0, state1, time ms)` of every tracker (main first; a narrow tracker has
/// `state0` 1; `time = time8 << 3`).
// SPEC: UDP_SESSION_2009_B.md §4.4 (reader engine+0xa2530)
fn read_anim_trackers(r: &mut BitReader<'_>) -> Vec<(u16, bool, bool, u32)> {
    let mut main = true;
    let mut out = Vec::new();
    while !r.at_end() && r.read_bit() {
        let (anim, state0) = if r.read_bit() {
            let a = r.read_bits(14) as u16;
            (a, r.read_bit())
        } else {
            (r.read_bits(8) as u16, true)
        };
        let state1 = r.read_bit();
        let tsel = r.read_bits(2) as usize;
        let time8 = r.read_bits([1, 6, 10, 32][tsel]);
        if !main {
            r.read_u8();
            if r.read_bit() {
                r.read_u8();
            } else {
                r.read_bits(3);
            }
        }
        main = false;
        out.push((anim, state0, state1, time8.wrapping_shl(3)));
    }
    out
}
