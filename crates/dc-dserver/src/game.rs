//! The game layer of slice 2: one Team Deathmatch round per process. Sans-IO and deterministic like the session:
//! the session hands it identified players, their parsed game messages and the time; it answers with game messages
//! per player, pipe lines and log lines ([`GameOut`]).
//!
//! Flow per player (`GAME_MESSAGES_2009_C2S.md` Appendix A, `GAME_MESSAGES_2009_S2C.md` §10):
//! placed in the world (S9-S11) -> `GS_IDENTIFY_REQUEST` -> the client's `UP_PLAYER_INFO` (or a deadline) -> roster
//! `GS_PLAYER_INFO` both ways and `GS_MATCH_SETTINGS` -> `GS_PLAYER_STATE` 3 (S12, ends the loading screen) ->
//! `UP_WORLD_READY` -> `GS_MATCH_BEGIN`, `GS_TEAM_SCORE`, statistics enable -> spawn set 0xa0, 0x6a, 0xb2 / 0xb3,
//! 0x66, 0x67 state 1 -> play (`UP_PLAYER_UPDATE`, `UP_FIRE`) -> death: state 3 then 2 in one block; the client
//! arms its own respawn wait and asks (0x89 / 0xc5 / 0xd2), the server answers with the spawn set (slice 4, DSV-32)
//! -> round end by goal, timer or an emptied team -> result messages (slice 4: the reference stack's end timing, no
//! `GS_LEVEL_EXIT`; the client leaves by itself).

use std::collections::BTreeMap;

use crate::chars::{self, CharacterAssets, CharacterChoice, RigCache};
use crate::gs::{self, BodySpec, Look, PlayerInfo, WeaponSpec};
use crate::modelobj::Tracker;
use crate::msg::Hello;
use crate::objects::PosBox;
use crate::pipe::{self, RoomSettings, RosterEntry};
use crate::rom_tables::RomTables;
use crate::up::{Up, UpPerf, UpPlayerInfo};
use crate::world::WorldInfo;
use dc_net_engine::time::{later, since};
use dc_net_engine::Millis;
use dc_wire::udp::BitWriter;

mod anim;
mod combat;
pub mod coop;
mod coop_show;
// fall damage (DSV-76)
pub mod fall;
pub mod gungame;
pub mod hisec;
pub mod possession;
// weapons dropped at a death (DSV-93..DSV-95)
pub mod drops;
pub mod packs;
// DC-DEFAULT: DSV-115..DSV-120 (proposed) destructible world objects (world-objects)
// DC-DEFAULT: DSV-200..DSV-204 (proposed) Search and Destroy: bomb carrier, plant, defuse, explosion
pub mod bomb;
pub mod ctf;
// DC-DEFAULT: DSV-210..DSV-213 (proposed) Quarantine: outbreak, infection by the claws, infected respawn
pub mod destruct;
pub mod hits;
pub mod infect;
// DC-DEFAULT: DSV-230..DSV-235 (proposed) Spy Hunt: intel cases, spies, the uplink, the agent, points
mod projectile;
pub mod spy;
pub mod station;
pub use projectile::{blast_factor, BLAST_INNER_FRACTION};
mod relay;
mod round;
// the Search and Destroy warm-up before round 1 (DSV-512)
mod warmup;
pub use warmup::WARMUP_MIN_SECS;
// the players stop at the round result (DSV-77)
mod freeze;
pub mod place;
pub mod remotehp;
pub mod teamping;
// FFA team values and the clan side switch (GMD-7, GMD-10; DSV-147..DSV-152)
pub mod modes;
pub use freeze::{FREEZE_WAIT_MS, ROUND_END_FREEZE};
// event sounds: pain, head shot, grenade impacts (DSV-64)
mod events;
// round 2: spawn on the floor, slot status, match intro, kill volumes (DSV-48..50, DSV-52)
mod arrival;
pub use arrival::{dc_mode_list, SPAWN_LIFT, SPAWN_MAX_DROP};

// ------------------------------------------------------------------------------------------------- policy values

/// Life-state timeline of the old death order (`-deathstates spec`): state 3 after 2000 ms in state 2.
// SPEC: GAME_MESSAGES_2009_C2S.md §3.3, Appendix A step A11
// DC-DEFAULT: DC-C2S-10 (superseded by GAME_MESSAGES_2009_C2S.md §12.3 DC-C2S-10'; kept behind `-deathstates spec`)
pub const DYING_MS: u32 = 2_000;
/// The respawn wait the 2009 client arms for itself once per death, in the first frame after it processed
/// `GS_PLAYER_STATE` 2 (7000 ms in mode classes 1, 4, 6, 7, 8; 5 and 9 only under a client state; 1500 ms in class
/// 10; nothing in 2, 3, 11, 12, 13 and every other class). It then sends its respawn request once, strictly more
/// than the wait after arming.
// DISASM: cshell+0x60b2e..0x60c0e, cshell+0x62467..0x62494, cshell+0x6203e..0x62132 (CAPTURE_tdm4_KILL_VALIDATION.md
// §4 items 1-5); OBSERVED: botduel3 / botduel4 (the bot's self-armed 0x89 7001-7016 ms after state 2, REFERENCE_FINDINGS
// F3)
pub const CLIENT_RESPAWN_WAIT_MS: u32 = 7_000;
pub const CLIENT_RESPAWN_WAIT_CLASS10_MS: u32 = 1_500;
/// The server answers a respawn request no earlier than the class wait minus this margin after the death (a request
/// before it is a client that did not wait: logged and ignored).
// DC-DEFAULT: DSV-32 (proposed, refines DC-KILL-1) 250 ms
pub const RESPAWN_GUARD_MS: u32 = 250;
/// Respawn on the server's own timer this long after the death, for the classes whose client arms its wait only
/// under a client state the server does not control (5, 9) and for classes the client never asks in but that are
/// not round modes (the generic class of a room line without a type).
// DC-DEFAULT: DSV-32 (proposed; DC-KILL-1 proposed `respawn_fallback_ms` 10000)
pub const RESPAWN_FALLBACK_MS: u32 = 10_000;
/// Death states the server sends: `Reference` = `GS_PLAYER_STATE` 3 then 2 in one block (the client arms its wait on
/// the 2; state 3 does not clear the death time); `Spec` = 2 at once and 3 after [`DYING_MS`] (the superseded DC-C2S-10; the reference stack order is DC-C2S-10').
// SPEC: CAPTURE_tdm4_KILL_VALIDATION.md §3.3 (REFERENCE: 3 then 2 in one block; the 2009 client arms on 2, DISASM
// cshell+0x61975 / 0x619b9), DC_DEFAULTS capture notes 2026-10-25; GAME_MESSAGES_2009_C2S.md §12.3
// DC-C2S-10' (proposed), GAME_MESSAGES_2009_S2C.md §4.1 PD-S2C-2' (proposed)
// DC-DEFAULT: DSV-32 (proposed) reference order by default, `-deathstates spec` for the old order
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeathStates {
    Reference,
    Spec,
}

/// How a dead player of a mode class comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RespawnRule {
    /// The client arms its own wait and asks; the server answers the request (and, with `fallback_ms`, respawns on
    /// its own that long after the death when no request came).
    ClientRequest {
        wait_ms: u32,
        fallback_ms: Option<u32>,
    },
    /// Round modes: no respawn inside a round; the next round respawns everybody (`game::round`, GMD-20).
    NextRound,
    /// The client never asks: the server respawns this long after the death.
    ServerTimer { after_ms: u32 },
}

/// The respawn request shell id the 2009 client sends in a mode class (0x89; 0xc5 in 7 / 8; 0xd2 in 10).
// SPEC: GAME_MESSAGES_2009_C2S.md §3.4 (request by class, cshell+0x61e40); CAPTURE_tdm4_KILL_VALIDATION.md §4 item 5
pub fn respawn_request_id(mode_class: u8) -> Option<u8> {
    match mode_class {
        1 | 4 | 5 | 6 | 9 => Some(crate::up::UP_RESPAWN_REQUEST),
        7 | 8 => Some(crate::up::UP_RESPAWN_REQUEST_B),
        10 => Some(crate::up::UP_RESPAWN_REQUEST_C),
        _ => None,
    }
}
/// Ping table period.
// DC-DEFAULT: PD-S2C-4 (proposed) every 5 s
pub const PING_PERIOD_MS: u32 = 5_000;
/// Statistics report periods the server enables after `UP_WORLD_READY`.
// DC-DEFAULT: DC-C2S-7 (proposed) ping 2000 ms, statistics 5000 ms
pub const PERF_PING_MS: u16 = 2_000;
pub const PERF_STATS_MS: u16 = 5_000;
/// Multi-kill chain: a foe kill at most 4 s after the killer's previous foe kill (same life) raises the chain level
/// that 0x6d `kill_kind` carries; level 2 is the first multi-kill form.
// DC-DEFAULT: DC-C2S-13 (amended, staged DSV-105)
pub const MULTI_KILL_WINDOW_MS: u32 = 4_000;
pub const MULTI_KILL_MIN: u32 = 2;

/// `pid` of the statistics enable message. The client shows its own statistics overlay only when the `pid` is its
/// own id, so the enable is addressed to nobody in particular.
// SPEC: GAME_MESSAGES_2009_C2S.md §5.8 (cshell+0x29300: enable from `x == 1`, overlay only for the own id)
// SPEC: UNPROVEN that the reports start without the overlay
pub const TODO_PERF_ENABLE_PID: u32 = 0xFFFF_FFFF;
/// `kill_kind` of `GS_KILL` for a foe kill that is no multi-kill: the client's announcement level 1 (the killer's own
/// kill-count banner, or the head-shot banner on a head hit). The field is the multi-kill level, see
/// `combat::KILL_LEVEL_MAX` and `combat::KILL_KIND_NO_ANNOUNCE`.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 field 3; CAPTURE_tdm4_KILL_VALIDATION.md §3.2, REFERENCE_FINDINGS_botduel.md F6
// (`kill_kind` 1 on 30 + 8 kills; REFERENCE)
// DISASM: cshell+0x2e12a..0x2e1d6 (`kill_kind` is the second argument of cshell+0xa5fe0), cshell+0xa628b (its
// level switch: 1 one kill, 2..6 the multi-kill forms, 7 and more the last form, 0 nothing)
pub const TODO_KILL_KIND: u8 = 1;
/// `hit_zone` of `GS_KILL` when the hit names no model node: 1 = head (PROVEN), else the torso. With a node the
/// zone is the node's location code from the ROM skeleton (1 head, 2 torso, 3 arms, 4 legs), which is what the reference stack
/// sent for the real client's shoulder hits (3).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 field 5 (1 = head); CAPTURE_tdm4_KILL_VALIDATION.md §3.2 (`hit_zone` 3 for
// hits with `extra_kind` 24 / 11; REFERENCE); OBSERVED: ROM table key MODELBUTES.TXT [Skeleton<n>] Node<k>Location (the
// nodes 24 and 11 of the body rows' skeleton carry location 3, the head nodes 1; values read at run time)
// DC-DEFAULT: DSV-33 (proposed) torso when no node is named
pub const TODO_KILL_ZONE_BODY: u8 = 2;
/// `f_relation` of `GS_KILL` (the kill feed gets the victim's relation to the own team): the reference stack sent 0.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 field 6 (meaning PLAUSIBLE); CAPTURE_tdm4_KILL_VALIDATION.md §3.2 (0; REFERENCE)
// DC-DEFAULT: DSV-33 (proposed)
pub const TODO_KILL_RELATION: bool = false;
/// `feed_value` of `GS_KILL` (meaning UNPROVEN): 2 on every kill the reference stack sent.
// SPEC: CAPTURE_tdm4_KILL_VALIDATION.md §3.2 field 11 (2; REFERENCE); SPEC: UNPROVEN meaning
// DC-DEFAULT: DSV-33 (proposed)
pub const TODO_KILL_FEED_VALUE: u8 = 2;
/// `GS_DAMAGE_TAKEN` constants of a weapon hit as the reference stack sent them and the real client accepted: `mode` 1,
/// `dmg_kind` 3, `ammo_kind` 0xFFFF (no ammunition row: the client's row-based camera shake does not run), `part` 0;
/// `zone` = the ROM damage type of the ammunition (`TODO_DAMAGE_ZONE` when the ROM has none) and the unread byte
/// before `mode` repeats it.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (0x74 fields); CAPTURE_tdm4_KILL_VALIDATION.md §3.1 (REFERENCE); SPEC: UNPROVEN
// meanings of `mode` / `dmg_kind` / `part`
// DC-DEFAULT: DC-KILL-2 (proposed); DSV-33 (proposed) the constants
pub const TODO_DAMAGE_ZONE: u8 = 0;
pub const TODO_DAMAGE_KIND: u8 = 3;
pub const TODO_DAMAGE_MODE: u8 = 1;
pub const TODO_DAMAGE_PART: i8 = 0;
pub const DAMAGE_NO_AMMO_ROW: u16 = 0xFFFF;
/// `GS_MATCH_RESULT_SCREEN` `result_flag` (meaning UNPROVEN, V-S2C-3); `result_value` is the winning team (the
/// reference stack's form, round module).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (0x3f), §11.2 item 2; SPEC: UNPROVEN
pub use round::TODO_RESULT_FLAG;
/// `GS_PLAYER_SUMMARY` gains without a `gain` line: the lobby computes rewards (CD-54) after `@result` and may send
/// them back (`gain <uid> <exp> <gp>`) during the first end pause; without one the summary shows zero gains.
// SPEC: GAME_MESSAGES_2009_C2S.md Appendix A step A13 (order with the lobby UNPROVEN); SPEC: UNPROVEN
// DC-DEFAULT: CD-54 (approved) rewards computed by the lobby
pub const TODO_SUMMARY_GAINS: u32 = 0;
/// `GS_MATCH_BEGIN` with the intro block (from the room line's map and radar data, DSV-50). The client reads the
/// block only when its two intro images loaded (client state the server cannot see; on a normal install they do).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1.1 step 5 (intro read only when two image handles are valid; PLAUSIBLE always)
// OBSERVED: tdm4 / tdm6 (the reference stack's 0x45 with the intro, accepted by the real client)
// DC-DEFAULT: DSV-50 (proposed) on (`-nointro` off)
pub const TODO_SHOW_INTRO: bool = true;
/// Weapon slots of a loadout (primary, secondary, melee, throwable).
// SPEC: PROTOCOL_2009.md §3.3 (0x323 equipment slots 0..3 = the weapon slots); EVIDENCE: cshell+0x259f00
pub const WEAPON_SLOTS: u8 = 4;
/// The lobby's backpack equipment slots (10..13 hold weapons carried in the backpack, CD-77) and the client weapon
/// slots they are given in: backpack slot 10 + k -> weapon slot 4 + k.
// OBSERVED: capture 20261025_065016_tdm6 (the reference stack gave the backpack shotgun as 0xb2 slot 4,
// `class_pos` 1, and the real client fired it; REFERENCE); capture 20261025_081704_rust3 (our lobby
// stored the bought shotgun in equipment slot 10 and the dserver dropped it: the user could not switch to it);
// DISASM: cshell+0x22e930 (the client's weapon change accepts slot indices 0..8)
// DC-DEFAULT: DSV-38 (extended, staged) backpack slots 10..13 -> weapon slots 4..7
pub const BACKPACK_EQUIP_FIRST: u8 = 10;
pub const BACKPACK_WEAPON_FIRST: u8 = 4;
pub const BACKPACK_SLOTS: u8 = 4;

/// The client weapon slot of a lobby equipment slot (`None` for gear slots).
pub fn weapon_slot_of_equip(slot: u8) -> Option<u8> {
    if slot < WEAPON_SLOTS {
        Some(slot)
    } else if (BACKPACK_EQUIP_FIRST..BACKPACK_EQUIP_FIRST + BACKPACK_SLOTS).contains(&slot) {
        Some(BACKPACK_WEAPON_FIRST + slot - BACKPACK_EQUIP_FIRST)
    } else {
        None
    }
}
/// Mode class used for the message layouts when the room line names none (no special layout branch).
// SPEC: GAME_MESSAGES_2009.md §1.2 (the TDM row is none of the special values 2, 3, 5, 6, 9, 10, 11)
pub const GENERIC_MODE_CLASS: u8 = 0;

/// Order of the start-up and spawn messages.
///
/// `Reference` (default) follows the order observed from the reference client where our specs mark the order only
/// PLAUSIBLE: shell 0x90 and 0x88 state 2 at link creation before `S2C_VERSION` (O6), the identify request, the game
/// endpoint and the match settings before the view object (O2), the player spawned with state 1 directly right
/// after its player info, before `UP_WORLD_READY` (O9: 0x6b, 0x6a, 0x67, 0x66, 0xb2, 0x7f, 0xa0, then 0x43, 0x44,
/// 0xb5, 0x45), and after `UP_WORLD_READY` shell 0x90 / 0x88 state 3 and a second teleport. `Spec` keeps the
/// slice-2 order (0x88 at S11, state 3 before the 0x87 and the spawn set after it; the superseded DC-C2S-10, which DC-C2S-10' still
/// allows).
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md O2, O6, O9 (REFERENCE); UDP_SESSION_2009_A.md §9.2 (S11, order PLAUSIBLE);
// GAME_MESSAGES_2009_S2C.md §10 (a consistent proposal, not observed)
// DC-DEFAULT: DSV-22 (proposed) reference order by default, spec order behind `-startorder spec`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartOrder {
    Reference,
    Spec,
}

/// How a spawn point is drawn (`-spawnpick`).
// DC-DEFAULT: DSV-121 (staged, arena-bots; renumber at fold if taken) `stream` by default; `keyed` for the arena comparison harness
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpawnPick {
    /// One random stream for the whole match, after the out-of-enemy-sight filter: the point a player gets depends
    /// on how many draws came before it (the spawn order of everybody) and on where the enemies stand.
    #[default]
    Stream,
    /// A hash of the seed, the player's hello name (case-insensitive), its team and its spawn number picks among
    /// the team's candidate points, without the enemy filter and without touching the stream: the same seed and
    /// the same names give the same points whatever the load order (a test-harness option).
    Keyed,
}

/// The `-spawnpick keyed` key of one spawn: FNV-1a over the lower-case name, then the team and the spawn number.
// DC-DEFAULT: DSV-121 (staged, arena-bots; renumber at fold if taken); FNV-1a and the splitmix64 finaliser are public algorithms
pub fn spawn_key(name: &[u8], team: u8, spawn_no: u32) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.iter().map(u8::to_ascii_lowercase) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^ (u64::from(team) << 32) ^ u64::from(spawn_no)
}

/// splitmix64 finaliser (public algorithm, Steele / Lea / Flood).
fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Control word bits that make a remote player walk (any horizontal movement command).
// SPEC: MOVEMENT_2009.md §5.1 (FWD 0x1, BACK 0x2, LEFT 0x80, RIGHT 0x100)
pub const CONTROL_MOVING: u32 = 0x1 | 0x2 | 0x80 | 0x100;
/// Control word bits of the gait: crouched, running (MOVEMENT_2009.md §5.1; `RUN` is never set while crouched).
// SPEC: MOVEMENT_2009.md §5.1 (CROUCH 0x20, RUN 0x200)
pub const CROUCHED_BIT: u32 = 0x20;

/// Crouch box half height used when the rig has no crouch clip (the ROM player models give 40; standing 50).
// DC-DEFAULT: DSV-169 (proposed); SIM-25
pub const CROUCH_HALF_HEIGHT: f32 = 40.0;
pub const RUNNING_BIT: u32 = 0x200;
/// Default of [`GameTuning::directional_anims`]: the lower-body clip follows the control word's gait and direction
/// (run / walk / crouch sets, eight directions, DSV-28 refinement); off = the one walk pair for every movement (the
/// first slice-3 form). The 2009 client plays footsteps only from the run clips' animation keys, so with the walk
/// pair a running character is silent.
// OBSERVED: capture 20261025_000014_tdm1, 20261025_002236_tdm4 (the reference stack's clip per control
// word; REFERENCE); capture 20261025_095358_rust4 (our walk clip 47 for every moving state, the user
// heard no footsteps); DISASM: cshell+0x2057a0 / cshell+0x204f60 (footsteps from the clip keys)
// DC-DEFAULT: DSV-28 (refinement, proposed) on (`-directionalanims off` restores the walk pair)
pub const DIRECTIONAL_ANIMS: bool = true;
/// Animation length of the weapon model's single tracker: the reference stack sent it with the time field of width 1.
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (weapon object tracker `tsel` 0, time 0; REFERENCE)
pub const TODO_WEAPON_ANIM_LEN_MS: u32 = 0;
/// Client events that are not relayed to the other players: `UP_WEAPON_LOOP` 0x81, `UP_WEAPON_EVENT` / `UP_DEFLECT`
/// 0x76 categories 3 and 5, `UP_WEAPON_TOGGLE` 0xcc (the reference stack relayed no 0x76 category 3; 0x81 / 0xcc / category 5
/// were never observed). Relayed: 0x80 and the 0x76 category 1 event 3 (type 0x19), 0x82 (type 0x06), 0x7f (the hand
/// model, DSV-25).
// SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.5 (U5 decided: "0x76 cat 3 -> no relay; 0x81 / 0xcc -> not observed")
// DC-DEFAULT: DSV-26 (proposed)
pub const TODO_RELAY_UNOBSERVED_EVENTS: bool = false;

/// `UP_MOVE_EVENT` category and the event relayed as the jump sound (`k` 0x0f): event 1, the jump that plays the
/// jump sound locally. CAPTURE_tdm2 §3.5 reads its 13 relays as landings, but the round had 13 jumps (event 1) and
/// 16 events 3; CAPTURE_tdm4 §8.1 S8 maps 0x76 [1][1] to `k` 0x0f, and CAPTURE_tdm1 §7 item 3 shows event 3 is no
/// landing marker.
// SPEC: CAPTURE_tdm4_KILL_VALIDATION.md §8.1 S8; CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.5; GAME_MESSAGES_2009_C2S.md
// §2.2 (event 1 = jump with the jump sound)
pub const EVENT_CATEGORY_MOVE: u8 = 1;
pub const EVENT_RELAYED_JUMP: u8 = 1;
/// The throw sound relayed with the throw's `UP_FIRE`: type 0x19 `k` 0x12 `[u8][player:u32][pos:vec96][u8][u8]`
/// with the byte values the reference stack sent (meanings not traced).
// SPEC: CAPTURE_tdm4_KILL_VALIDATION.md §8.1 S8; CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.4 (`[u8 6][u32 1][vec96][u8 1]
// [u8 0]`; REFERENCE, ACCEPTED); GAME_MESSAGES_2009_S2C.md §8.2 (type 0x19 reader, `k` 0x12 form)
pub const TODO_THROW_SOUND_BYTES: (u8, u8, u8) = (6, 1, 0);

/// The throw sound's bytes for a thrower of `team`: the callout row and the voice template as the reference stack sent them,
/// the team byte the thrower's team. The 2009 client plays the callout only when that byte equals its own team (the
/// thrower hears his own "fire in the hole" from this relay); the reference stack's 1 was its thrower's team, so with a fixed
/// 1 a team-2 thrower and his team never heard it and team 1 heard the enemy's.
// EVIDENCE: cshell+0x2cbd30 (k 0x12: plays only when the record's +0x48, the trailing `b`, equals the local player's
// team, slot +0xb0); DISASM: cshell+0x217845..0x217865 (`b` -> +0x48, `c` -> +0x49, the leading byte -> +0x46)
// DC-DEFAULT: DSV-260 (proposed; amends CV2-3) `b` = the thrower's team (playtest report: "fire in the hole" missing)
pub fn throw_sound_bytes(team: u8) -> (u8, u8, u8) {
    let (row, _, template) = TODO_THROW_SOUND_BYTES;
    (row, team, template)
}
/// An `UP_FIRE` within this time since the last hold (`UP_THROW_HOLD`, repeated while held) is the throw (about
/// 200 ms observed).
// SPEC: GAME_MESSAGES_2009_C2S.md §4.6 (the release is the 0x82 sent 0.2 s after the last 0x4d);
// CAPTURE_tdm1_GAME_VALIDATION.md §7 item 2
// DC-DEFAULT: DSV-29 (proposed)
pub const THROW_FIRE_WINDOW_MS: u32 = 1_000;
/// Distance of a missed shot's end point (the server has no world trace yet).
// DC-DEFAULT: DSV-29 (proposed)
pub const TODO_MISS_TRACE_UNITS: f32 = 4096.0;
/// View-angle repair: a reported yaw beyond this magnitude (radians) is folded back with `GS_VIEW_ANGLES` kind 0, at
/// most once per interval.
// SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §5 (a client stuck at yaw 2.4e6 rad); GAME_MESSAGES_2009_S2C.md §4.2
// DC-DEFAULT: CV2-5 (proposed) 1000 rad, 2 s
pub const VIEW_REPAIR_MAX_RAD: f64 = 1000.0;
pub const VIEW_REPAIR_INTERVAL_MS: u32 = 2_000;

/// A yaw folded into (-pi, pi], computed in f64 (the client's yaw is an unwrapped accumulator).
// SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §8 D5 (fold in f64 before storing or streaming)
pub fn fold_yaw(y: f32) -> f32 {
    if !y.is_finite() {
        return 0.0;
    }
    let tau = std::f64::consts::TAU;
    let mut v = (y as f64).rem_euclid(tau);
    if v > std::f64::consts::PI {
        v -= tau;
    }
    v as f32
}

/// Weight sets and tracker ids of the two secondary trackers (upper body, lower body).
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (tracker 1 weight set 0, tracker 2 weight set 1; REFERENCE)
pub const TRACKER_UPPER: (u8, u8) = (1, 0);
pub const TRACKER_LOWER: (u8, u8) = (2, 1);

/// Movement direction of a character from the control word's direction bits (opposite bits cancel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Forward,
    Back,
    Left,
    Right,
    ForwardLeft,
    ForwardRight,
    BackLeft,
    BackRight,
}

impl Dir {
    /// All directions in the order of the clip sets (`chars::CLIP_LOWER_RUN` and its siblings).
    pub const ALL: [Dir; 8] = [
        Dir::Forward,
        Dir::Back,
        Dir::Left,
        Dir::Right,
        Dir::ForwardLeft,
        Dir::ForwardRight,
        Dir::BackLeft,
        Dir::BackRight,
    ];

    /// The direction of a control word, `None` without horizontal movement.
    // SPEC: MOVEMENT_2009.md §5.1 (FWD 0x1, BACK 0x2, LEFT 0x80, RIGHT 0x100)
    pub fn from_control(c: u32) -> Option<Dir> {
        let fb = i32::from(c & 0x1 != 0) - i32::from(c & 0x2 != 0);
        let lr = i32::from(c & 0x100 != 0) - i32::from(c & 0x80 != 0);
        Some(match (fb, lr) {
            (1, 0) => Dir::Forward,
            (-1, 0) => Dir::Back,
            (0, -1) => Dir::Left,
            (0, 1) => Dir::Right,
            (1, -1) => Dir::ForwardLeft,
            (1, 1) => Dir::ForwardRight,
            (-1, -1) => Dir::BackLeft,
            (-1, 1) => Dir::BackRight,
            _ => return None,
        })
    }

    fn index(self) -> usize {
        Dir::ALL.iter().position(|d| *d == self).unwrap_or(0)
    }
}

/// The gait of a moving character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gait {
    Walk,
    Run,
    Crouch,
}

/// How a character moves, for its animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Stand,
    /// The one walk pair (co-op AI bodies, and every movement with `directional_anims` off).
    Walk,
    /// Crouched without moving.
    Crouch,
    /// A gait in a direction.
    Step(Gait, Dir),
}

impl Motion {
    /// The motion a control word asks for: `CROUCH` first (the crouch set), then `RUN` (the run set), else the
    /// walk set; the clip by the direction bits. `directional` off: the walk pair for any movement.
    // SPEC: MOVEMENT_2009.md §5.1 (control bits; `RUN` only when not crouched)
    // DC-DEFAULT: DSV-28 (refinement, proposed)
    pub fn from_control(c: u32, directional: bool) -> Motion {
        if !directional {
            return if c & CONTROL_MOVING != 0 {
                Motion::Walk
            } else {
                Motion::Stand
            };
        }
        let crouch = c & CROUCHED_BIT != 0;
        match (Dir::from_control(c), crouch) {
            (None, true) => Motion::Crouch,
            (None, false) => Motion::Stand,
            (Some(d), true) => Motion::Step(Gait::Crouch, d),
            (Some(d), false) if c & RUNNING_BIT != 0 => Motion::Step(Gait::Run, d),
            (Some(d), false) => Motion::Step(Gait::Walk, d),
        }
    }

    /// Whether the character moves (the upper body uses the walk clip).
    pub fn moving(self) -> bool {
        matches!(self, Motion::Walk | Motion::Step(..))
    }
}

/// Animation indices of a character (combined index, length in ms): main tracker, then `[stand, walk]` for the
/// upper and the lower body, the lower-body direction sets and the crouched stand.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnimSet {
    pub main: (u16, u32),
    pub upper: [(u16, u32); 2],
    pub lower: [(u16, u32); 2],
    /// Lower-body clips by gait (walk, run, crouch) and [`Dir`]; a clip the rig lacks is the walk clip.
    pub steps: [[(u16, u32); 8]; 3],
    /// Lower-body clip crouched without moving (the stand clip where the rig has none).
    pub crouch: (u16, u32),
    /// Death clips of the corpse: body hit left / right, head hit left / right (the main clip where the rig has
    /// none).
    pub death: [(u16, u32); 4],
    /// The upper-body table of the rig (DSV-125): aim / action clips by weapon family, hit and jump clips.
    pub table: crate::anims::UpperTable,
    /// Tracker id and weight set of the upper-body and the lower-body tracker on this rig (DSV-475).
    pub secondary: [(u8, u8); 2],
}

impl AnimSet {
    /// From a rig: the main tracker on the composed model's first clip (the reference stack's animation 0), the four
    /// player clips by name key (missing ones fall back to the main clip).
    // SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (main tracker animation 0; REFERENCE)
    pub fn from_rig(rig: &chars::Rig) -> AnimSet {
        let main = rig.clips.first().map_or((0, 0), |c| (0, c.len_ms));
        let pick = |k| rig.clip(k).unwrap_or(main);
        let lower = [pick(chars::CLIP_LOWER_STAND), pick(chars::CLIP_LOWER_WALK)];
        // DC-DEFAULT: DSV-28 (refinement, proposed) a rig without a direction clip falls back to the walk clip
        let set = |keys: &[chars::NameKey; 8]| {
            let mut out = [lower[1]; 8];
            for (o, k) in out.iter_mut().zip(keys.iter()) {
                if let Some(c) = rig.clip(*k) {
                    *o = c;
                }
            }
            out
        };
        AnimSet {
            main,
            upper: [pick(chars::CLIP_UPPER_STAND), pick(chars::CLIP_UPPER_WALK)],
            lower,
            steps: [
                set(&chars::CLIP_LOWER_WALK_DIRS),
                set(&chars::CLIP_LOWER_RUN),
                set(&chars::CLIP_LOWER_CROUCH),
            ],
            crouch: rig.clip(chars::CLIP_LOWER_CROUCH_STAND).unwrap_or(lower[0]),
            death: [
                pick(chars::CLIP_DEATH_BODY_LEFT),
                pick(chars::CLIP_DEATH_BODY_RIGHT),
                pick(chars::CLIP_DEATH_HEAD_LEFT),
                pick(chars::CLIP_DEATH_HEAD_RIGHT),
            ],
            table: crate::anims::UpperTable::from_rig(rig),
            // DC-DEFAULT: DSV-475 (proposed) the rig's own upper / lower weight set (EVIDENCE cshell+0x15ef0: looked
            // up by name), else the reference stack's index
            secondary: [
                (
                    TRACKER_UPPER.0,
                    rig.tracker_masks[0].unwrap_or(TRACKER_UPPER.1),
                ),
                (
                    TRACKER_LOWER.0,
                    rig.tracker_masks[1].unwrap_or(TRACKER_LOWER.1),
                ),
            ],
        }
    }

    /// The corpse's death clip: head or body by the lethal hit's zone, left or right by the side of the victim the
    /// shot travelled toward (`dir` the shot direction, `yaw` the victim's facing; no direction = right).
    // DC-DEFAULT: DSV-44 (proposed) head kill -> a head clip, else a body clip; the side by the shot direction
    pub fn death_clip(&self, head: bool, dir: [f32; 3], yaw: f32) -> (u16, u32) {
        // the victim's right-hand vector for a facing `yaw` about +Y (forward (sin, 0, cos))
        let right = [yaw.cos(), 0.0, -yaw.sin()];
        let s = dir[0] * right[0] + dir[2] * right[2];
        let i = usize::from(head) * 2 + usize::from(s >= 0.0 || s.is_nan());
        self.death[i]
    }

    /// The three trackers at `t_main` ms since the spawn and `t_motion` ms since the motion began.
    pub fn trackers(&self, motion: Motion, t_main: u32, t_motion: u32) -> [Tracker; 3] {
        let upper = self.upper[usize::from(motion.moving())];
        let lower = self.lower_clip(motion);
        let tr = |(anim, len): (u16, u32), t: u32, secondary| Tracker {
            anim,
            // SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (state bits 1, 1 on every tracker; REFERENCE)
            state0: true,
            state1: true,
            time_ms: if len > 0 { t % len } else { 0 },
            len_ms: len,
            secondary,
        };
        [
            tr(self.main, t_main, None),
            tr(upper, t_motion, Some(self.secondary[0])),
            tr(lower, t_motion, Some(self.secondary[1])),
        ]
    }

    /// The lower-body clip of a motion.
    // DC-DEFAULT: DSV-28 (refinement, proposed)
    pub fn lower_clip(&self, motion: Motion) -> (u16, u32) {
        match motion {
            Motion::Stand => self.lower[0],
            Motion::Walk => self.lower[1],
            Motion::Crouch => self.crouch,
            Motion::Step(g, d) => {
                let gi = match g {
                    Gait::Walk => 0,
                    Gait::Run => 1,
                    Gait::Crouch => 2,
                };
                self.steps[gi][d.index()]
            }
        }
    }
}

/// The weapon model other players see in a character's hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeaponLook {
    pub weapon: u16,
    pub model: String,
    pub textures: Vec<String>,
    pub styles: Vec<String>,
}

/// What other clients see of a player, resolved from the ROM when the player is placed.
#[derive(Debug, Clone, PartialEq)]
pub struct CharacterLook {
    pub assets: CharacterAssets,
    pub weapon: Option<WeaponLook>,
    pub anims: AnimSet,
}

/// Hit validation input for the `combat.on_hit` hook.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitInfo {
    pub shooter: u8,
    pub victim: u8,
    pub weapon_id: u16,
    pub ammo_id: u16,
    pub base_damage: f32,
    pub head: bool,
    pub distance: f32,
    /// The zone multiplier of DSV-30 and the kill-feed zone (1 head, 2 torso, 3 arms, 4 legs).
    pub zone_factor: f32,
    pub zone: u8,
}

/// A player's armour for one life.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Armour {
    pub start: f32,
    pub max: f32,
    pub absorb: Absorb,
}

/// How a player's armour shares a hit while it lasts (DSV-30).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Absorb {
    /// The same share of every hit (the operator's `combat.armor_absorb`, or the armour items' grade).
    Flat(f32),
    /// The ROM armour row `[Gear<n>]` of the weapon table: the share by the hit's damage type and location.
    Gear(u16),
    /// The armour rows the player wears (body, head gear, goggle; DSV-430): the sum of their shares, at most 1.
    Worn([Option<u16>; 3]),
}

/// Where a player's armour comes from (`combat.armor_gear`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmorGear {
    /// Every player wears the ROM armour row `[Gear<n>]`: start armour = its `Armor`, shares by damage type and hit
    /// location from its protection slots.
    Row(u16),
    /// The armour rows of what the player wears (DSV-430): the body row's `GearId` (row 0 when the body names none),
    /// the head gear's and the goggle's; start armour = the sum of their `Armor`.
    Worn,
    /// The loadout's armour items (`ITEMS.TXT` `Armor` summed, the largest `ArmorGrade` / 100 as a flat share).
    Items,
    /// No armour.
    Off,
}

/// The armour row every player wears by default.
// project decision: calibrated to the captured 2009 per-hit values (CAPTURE_tdm6 §2.1 / §3 damage rows T1,
// T2; tdm4 §3.1): with no armour item in the loadout the 2009 server started every life with armour 100 and took the
// same fixed share of the ammunition's InstDamage from it per torso or arm rifle hit (5.25 each), nothing per head shot
// or blast; the first ROM armour row `[Gear0]` reproduces exactly that (its `Armor`, a slot for the rifle's damage
// type with head share 0 and torso / arm / leg shares; values read at run time, never copied)
// DC-DEFAULT: DSV-30 (calibration staged, DC_DEFAULTS.md)
// EVIDENCE: cshell+0x1e4760, cshell+0x1dfc0a (body, head-gear and goggle rows carry a `GearId`); the default body
// row 0 names `[Gear0]` and head row 0 none, so a player without gear wears exactly the calibrated row
// DC-DEFAULT: DSV-430 (proposed) the worn rows replace the single row for everyone
pub const DEFAULT_ARMOR_GEAR: ArmorGear = ArmorGear::Worn;

/// The worn rows with a repeated armour row dropped: a body and a head gear naming the same row protect once
/// (review of audit-gear, 2026-10-30), so neither the start armour nor the share counts a row twice.
fn dedup_worn(mut rows: [Option<u16>; 3]) -> [Option<u16>; 3] {
    for i in 1..3 {
        if rows[i].is_some() && rows[..i].contains(&rows[i]) {
            rows[i] = None;
        }
    }
    rows
}

/// Default armour maximum without a ROM body row value.
// DC-DEFAULT: DSV-30 (proposed) 100 (the value of the 0x66 kind 0x10 the reference stack sent; S2C §4.3)
pub const DEFAULT_MAX_ARMOR: f32 = 100.0;

/// The `combat.on_hit` hook: returns the damage to apply, or `None` to refuse the hit.
// DC-DEFAULT: DC-C2S-11 (proposed) hit validation and damage hook `combat.on_hit`
#[derive(Debug, Clone, Copy)]
pub struct HitHook(pub fn(&HitInfo) -> Option<f32>);

impl PartialEq for HitHook {
    /// Settings compare equal when the same hook is installed (address comparison; only used by config tests).
    fn eq(&self, other: &Self) -> bool {
        std::ptr::fn_addr_eq(self.0, other.0)
    }
}

/// Game rule settings. Every value is a DeCombat default (row in `DC_DEFAULTS.md` or a staged DSV row) and can be
/// overridden by the operator.
#[derive(Debug, Clone, PartialEq)]
pub struct GameTuning {
    /// Health at spawn and its maximum (`combat.start_health`, `combat.max_health`).
    // DC-DEFAULT: DSV-30 (proposed) 100 / 100
    pub start_health: f32,
    pub max_health: f32,
    /// Armour at spawn: `None` = the worn armour's value ([`ArmorGear`]: the gear row's `Armor`, or the sum of the
    /// loadout's armour items), capped by the maximum (`combat.start_armor = rom|<n>`).
    // DC-DEFAULT: DSV-30 (proposed)
    pub start_armor: Option<f32>,
    /// Armour maximum: `None` = the body row's `MaxArmor`, else 100 (`combat.max_armor = rom|<n>`).
    // DC-DEFAULT: DSV-30 (proposed)
    pub max_armor: Option<f32>,
    /// Fraction of a hit the armour takes while it lasts: `None` = the worn armour's share ([`ArmorGear`]: the gear
    /// row's share for the hit's damage type and location, or the largest `ArmorGrade` / 100 of the armour items),
    /// else this share for every hit (`combat.armor_absorb = rom|<fraction>`).
    // DC-DEFAULT: DSV-30 (proposed)
    pub armor_absorb: Option<f32>,
    /// What the players wear (`combat.armor_gear = worn|<row>|items|off`).
    // project decision: calibrated to the captured 2009 per-hit values (CAPTURE_tdm6 §2.1, tdm4 §3.1); see
    // [`DEFAULT_ARMOR_GEAR`]
    pub armor_gear: ArmorGear,
    /// What a hit costs the armour: `true` = the share x the ammunition's damage before the zone multiplier (the 2009
    /// observation: the same 5.25 per hit on the arm and on the torso), `false` = the share x the zoned damage
    /// (`combat.armor_cost = base|hit`).
    // project decision: calibrated to the captured 2009 per-hit values (CAPTURE_tdm6 §2.1 T2: a torso and
    // an arm hit of one rifle cost the armour the same; tdm4 §3.1: the same on the bot)
    pub armor_cost_from_base: bool,
    /// Half extents of the player box sent with every teleport.
    // SPEC: UNPROVEN (the standing box is ROM model data, MOVEMENT_2009.md §9.2; the crouched box is not set by
    // any client code found, V-M7)
    // DC-DEFAULT: DSV-10 (staged proposal) until the box is read from the ROM player model
    pub player_dims: [f32; 3],
    /// Use the character's stand box from the ROM model instead of `player_dims` (off once `-playerbox` is given).
    // SPEC: MOVEMENT_2009.md §9.2 (stand box = user dims of the stand clip); UDP_REFERENCE_FINDINGS_botclient.md reference
    // run (the reference stack's box in the teleport, the 0x18 record and the dims block equals that clip's dims, compared
    // at run time in tests/reference_replay.rs; REFERENCE)
    // DC-DEFAULT: DSV-27 (proposed)
    pub player_dims_from_rom: bool,
    /// Body of the first teleport (`GS_MOVER_UPDATE` 0x0001); `None` = no body block.
    // SPEC: UNPROVEN which model and skins the 2009 server named (MOVEMENT_2009.md §12.4 block 0x0001)
    // DC-DEFAULT: DSV-10 (staged proposal)
    pub body: Option<BodySpec>,
    /// How long the server waits for `UP_PLAYER_INFO` before it introduces the player anyway.
    // DC-DEFAULT: DSV-12 (staged proposal) 3000 ms
    pub identify_wait_ms: u32,
    // DC-DEFAULT: DC-C2S-10 (superseded by GAME_MESSAGES_2009_C2S.md §12.3 DC-C2S-10'; kept behind
    // `-deathstates spec`), with `death_states` Spec
    pub dying_ms: u32,
    /// Death state order (`-deathstates reference|spec`).
    // DC-DEFAULT: DSV-32 (proposed)
    pub death_states: DeathStates,
    /// The client's respawn wait the server expects; `None` = the 2009 client's wait of the mode class
    /// ([`CLIENT_RESPAWN_WAIT_MS`]). Tests and modded clients with another wait set it (`-respawnwait`).
    // DC-DEFAULT: DSV-32 (proposed)
    pub respawn_wait_ms: Option<u32>,
    /// Margin before the wait in which a request is still accepted.
    // DC-DEFAULT: DSV-32 (proposed) 250 ms
    pub respawn_guard_ms: u32,
    /// Server-side respawn for the classes whose client may not ask (see [`RESPAWN_FALLBACK_MS`]).
    // DC-DEFAULT: DSV-32 (proposed) 10000 ms
    pub respawn_fallback_ms: u32,
    /// Round time in seconds overriding the room's `time=` (0 = no timer).
    pub round_secs: Option<u32>,
    /// Kill goal overriding the room's `goal=` (0 = no goal).
    pub goal: Option<u32>,
    /// Spy Hunt: intel cases overriding the GMS count (`-spyintel`, a test knob for short bot matches).
    // DC-DEFAULT: DSV-230 (proposed) none: the GMS count
    pub spy_intel: Option<u32>,
    /// Spy Hunt: the agent's health (and its maximum) and armour (and its maximum) while he is the agent
    /// (`spy.agent_health`, `spy.agent_armor`).
    // DC-DEFAULT: DSV-451 (proposed, amended by project decision "the super spy did not have enough extra hp")
    // 1000 / 500
    pub agent_health: f32,
    pub agent_armor: f32,
    /// Spy Hunt: multiplier of the burn and blast damage of the agent's own rounds (`spy.agent_damage_scale`).
    // DC-DEFAULT: DSV-456 (proposed, USER REQUEST 2026-10-31) 2.0
    pub agent_damage_scale: f32,
    pub show_intro: bool,
    /// Spawns rest on the floor below the spawn point (`-rawspawn`: at the point's own height).
    // DC-DEFAULT: DSV-48 (proposed) on
    pub spawn_on_floor: bool,
    /// The second flag of `GS_MATCH_BEGIN` (`-waitbanner on|off`): set, the 2009 client shows its "waiting for
    /// players to connect" banner until a 0x46 or a 0x45 with the flag clear. A running round sends it clear.
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.1.1 step 3; DISASM: cshell+0x49eab / cshell+0xcd170 (see gs::MatchBegin)
    // REFERENCE: flag 0 in every 0x45 of tdm1 / tdm2 / tdm4 / menu-frida6
    // DC-DEFAULT: DSV-20 (revision, proposed) off
    pub waiting_banner: bool,
    /// After each respawn's spawn set also send `GS_MATCH_BEGIN` without the intro (`-respawnbegin on|off`). The
    /// 2009 client plays its round-start voice (key `RoundStartSound` of the ROM interface table) for every 0x45
    /// that arrives while its own player state is 1, and its intro banner only once per round, so this replays the
    /// voice at each respawn. The user remembers the 2009 game saying "round start" at every respawn of a respawn
    /// mode, and 0x45 is the only client path to that voice, so it is on (the reference stack sent one 0x45 per link
    /// per round; `-respawnbegin off` restores that).
    // DISASM: cshell+0x49f00..0x49f7b (state +0x30 == 1 -> key at VA 0x374d0dc4, or VA 0x374d0dd4 with the waiting
    // flag, looked up by cshell+0x1b2c00, played by cshell+0x1b20c0 / +0x1b1f00); cshell+0x4a0ff / +0x4a12e (banner latch
    // `match_ui`+0x9a50, cleared only by 0x43 and the round-flow ids); cshell+0x49f3c the only reference to the
    // `RoundStartSound` key
    // OBSERVED: playtest report (the 2009 client says "round start" at each respawn in Elimination and One Man
    // Army)
    // DC-DEFAULT: DSV-473 (USER REPORT 2026-10-31; settles DSV-98) on
    pub respawn_match_begin: bool,
    /// The two pauses of the round-end set: `GS_ROUND_RESULT` at once, after the first pause `GS_ROUND_OBJECTS_END`,
    /// the roster refresh, `GS_PLAYER_SUMMARY` per player and `GS_ROUND_CLOSE`, after the second the result screen
    /// (`-holds <first_ms>,<second_ms>`). The 2009 client draws the banner and the summary from these; the pauses are
    /// the reference stack's (a client-facing timing the task asked to align, not a reward or limit value).
    // SPEC: REFERENCE_FINDINGS_botduel.md F7 (+3.000 s, +3.000 s; REFERENCE); CAPTURE_tdm4_KILL_VALIDATION.md §6 (the same
    // order after a leave)
    // DC-DEFAULT: DSV-35 (proposed) 3000 / 3000 ms
    pub end_pause_ms: (u32, u32),
    /// The first pause when the round ends by the time limit (`-timeout-hold <ms>`): the summary set (the
    /// scoreboard) follows `GS_ROUND_RESULT` after this long, 0 = in the same tick. Goal and forfeit ends keep
    /// `end_pause_ms.0`; the second pause is the same for every end.
    // project decision (the scoreboard as soon as the clock runs out)
    // DC-DEFAULT: DSV-35 (amendment, proposed) 0 ms
    pub timeout_hold_ms: u32,
    /// With `GS_ROUND_RESULT` every living player gets the mover freeze (`-roundfreeze on|off`); an airborne one at
    /// his landing, at the latest `freeze_wait_ms` later.
    // project decision (rust7: frozen at the win banner, as the user remembers the original)
    // DC-DEFAULT: DSV-77 (reinstated, proposed) on, 1000 ms
    pub round_end_freeze: bool,
    pub freeze_wait_ms: u32,
    /// After the result screen the server keeps every link until its client leaves (no server bye); this limit
    /// ends a session whose clients stay (`-linger <secs>`, 0 = wait for ever).
    // SPEC: REFERENCE_FINDINGS_botduel.md F7 (no server bye after 0x3f), CAPTURE_tdm4_KILL_VALIDATION.md §6 (the reference stack
    // kept the bot's link 38 s)
    // DC-DEFAULT: DSV-35 (proposed) 120 s
    pub linger_ms: u32,
    /// Round modes (the classes whose client never asks for a respawn: 2, 3, 11, 12, 13) play several rounds per
    /// match (`round_loop`; off = one round per match, the old form) with `between_round_ms` between the round
    /// close and the next round begin.
    // SPEC: GAME_MODES_2009.md §4.1 (round modes row), §4.2 (no request for 2, 3, 11, 12, 13; cshell+0x61e40)
    // DC-DEFAULT: GMD-5 (proposed) 5 s; GMD-20 (staged) the loop
    pub round_loop: bool,
    pub between_round_ms: u32,
    /// The most rounds a round-mode match plays (`None`: twice the goal); at this count the side with more round
    /// wins takes the match, equal = draw. Only draws can make a match reach it.
    // DC-DEFAULT: GMD-23 (staged) 2 x goal
    pub max_rounds: Option<u32>,
    /// The side switch of the clan classes 3, 4 and 13 (`-sideswitch off|5e|5f`).
    // DC-DEFAULT: GMD-7 (proposed) on with 0x5e; DSV-147 (proposed)
    pub side_switch: modes::SideSwitch,
    /// Free-for-all (class 6) gives every player his own team value (`-ffateams on|off`; off = the lobby's two
    /// teams, the old form).
    // DC-DEFAULT: GMD-10 (proposed); DSV-148 (proposed)
    pub ffa_teams: bool,
    /// A team that had players in this round and has none left for this long loses it (`-teamempty <secs>`, 0 = off).
    // DC-DEFAULT: DSV-31 (proposed) 10 s (the reference stack ended 0.3 s after the leave: REFERENCE timing, our own rule)
    pub team_empty_ms: u32,
    // DC-DEFAULT: PD-S2C-4 (proposed)
    pub ping_period_ms: u32,
    // DC-DEFAULT: DC-C2S-7 (proposed)
    pub perf_enable: bool,
    /// Damage, clip and stock when the ROM table has no value.
    // DC-DEFAULT: DSV-11 (staged proposal) 25 damage, 30 rounds, 90 in stock
    pub default_damage: f32,
    pub default_clip: u32,
    pub default_stock: u32,
    /// Damage multipliers: the ROM skeleton node's factor for the model part the client reported (when
    /// `zone_factors_from_rom`), else `head_factor` for the top `head_fraction` of the victim's box and `body_factor`
    /// below it (`combat.head_factor`, `combat.body_factor`, `combat.zone_factors = rom|off`).
    // DC-DEFAULT: DSV-30 (proposed) ROM node factors, else 2.0 head / 1.0 body; DC-C2S-11 (proposed) factors are
    // config values
    pub head_fraction: f32,
    pub head_factor: f32,
    pub body_factor: f32,
    pub zone_factors_from_rom: bool,
    /// Hit point tolerance around the victim's box and shot origin tolerance around the shooter.
    // DC-DEFAULT: DC-C2S-11 (proposed) tolerance; values DSV-11 (staged proposal)
    pub hit_tolerance: f32,
    pub origin_tolerance: f32,
    /// Trace each reported hit from the shot origin to the hit point against the world and refuse it when a wall
    /// comes first (more than `trace_slack` units before the hit point); `combat.trace = on|off`.
    // SPEC: REFERENCE_FINDINGS_botduel.md F1 (the reference stack refused hits through walls; REFERENCE)
    // DC-DEFAULT: DSV-34 (proposed) on, 16 units
    pub trace: bool,
    pub trace_slack: f32,
    /// Bounded rewind: a hit point outside the victim's box at its latest position is tested against the victim's
    /// own reported path of the last `rewind_ms` (0 = off; `combat.rewind_ms`). The shooter saw the victim where the
    /// stream showed it, which lags the victim's latest update by the transport, the update interval and the client's
    /// interpolation.
    // DC-DEFAULT: DSV-42 (proposed) 200 ms
    pub rewind_ms: u32,
    /// Log every refused hit claim with its verdict (`-verbose`); the `hits:` counter line is logged at the round
    /// end and at the session end either way.
    // DC-DEFAULT: DSV-42 (proposed)
    pub verbose_hits: bool,
    /// Thrown objects (DSV-39): launch speed, fuse, gravity and bounce when the ROM projectile row has no value, and
    /// the blast when the ammunition row has none.
    // DC-DEFAULT: DSV-39 (proposed) 1500 u/s, 3000 ms, -1100 u/s² (MOVEMENT_2009.md §2 `GRAVITY`), bounce 0.35,
    // stop below 100 u/s; blast 100 at the centre, radius 400, linear falloff
    pub projectile: ProjectileTuning,
    /// Movement validation of `UP_PLAYER_UPDATE` (speed, world box); violations get a teleport back.
    // SPEC: MOVEMENT_2009.md §12.4 (correct only by 0xa0 / 0x0020 with a new seq)
    // DC-DEFAULT: DSV-14 (staged proposal) speed 1500 u/s + 64 slack, box margin 256
    pub move_check: bool,
    pub max_speed: f32,
    pub move_slack: f32,
    pub box_margin: f32,
    /// Loadout for players whose `user` line carries no `loadout=` token (weapon table indices, first = drawn).
    // DC-DEFAULT: DSV-15 (staged proposal)
    pub default_loadout: Vec<u16>,
    /// A spawn point counts as out of enemy sight when no living enemy is closer than this.
    // DC-DEFAULT: DC-C2S-12 (proposed); radius DSV-16 (staged proposal)
    pub spawn_safe_radius: f32,
    /// A spawn point is occupied while a living player stands closer than this (horizontally) to it.
    // DC-DEFAULT: DSV-159 (proposed) 64 units = two stand boxes (2 x 24) plus 16
    pub spawn_occupied_radius: f32,
    pub rng_seed: u64,
    // DC-DEFAULT: DC-C2S-13 (proposed)
    pub multi_kill_window_ms: u32,
    pub multi_kill_min: u32,
    /// Bits of remote-object entries per stream message and tick.
    // DC-DEFAULT: CD-65 (approved) budget = transport window share; value DSV-17 (staged proposal)
    pub stream_budget_bits: usize,
    // DC-DEFAULT: DC-C2S-11 (proposed) `combat.on_hit`
    pub hit_hook: Option<HitHook>,
    /// The fall damage rule (`-combat fall=on|off,fall_h_lo=,fall_h_hi=,fall_d_lo=,fall_d_hi=`).
    // DC-DEFAULT: DSV-76 (proposed) on, 300 / 700 / 0 / 200
    pub fall: fall::FallTuning,
    /// Character rows of the ROM model table for players whose `user` line names none (`-character`).
    // DC-DEFAULT: DSV-24 (proposed)
    pub character: crate::chars::CharacterChoice,
    /// Start-up and spawn order (`-startorder`).
    // DC-DEFAULT: DSV-22 (proposed)
    pub start_order: StartOrder,
    /// Spawn point draw (`-spawnpick stream|keyed`).
    // DC-DEFAULT: DSV-121 (staged, arena-bots; renumber at fold if taken)
    pub spawn_pick: SpawnPick,
    /// Test-only placement (`-place`), off by default.
    pub place: place::PlaceConfig,
    /// Stream the player's own character to its own client as well (`-streamown on|off`; the reference stack did). The 2009
    /// client plays the local player's footsteps from the animation keys of its own character object, so an own
    /// object that never gets an animation never steps; the client does not adopt the streamed position.
    // SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (both characters in every bot's stream; REFERENCE);
    // UDP_SESSION_2009_B.md §4.2 (the client keeps its own position)
    // OBSERVED: capture 20261025_000014_tdm1 (4,652 entries of the user's own object on its
    // link; REFERENCE); capture 20261025_095358_rust4 (0 own entries, the user heard no own steps)
    // DISASM: cshell+0x204f60 (footstep player: the branch for the client's own object takes the surface from the
    // mover)
    // DC-DEFAULT: DSV-23 (change, proposed) on; off = DSV-21
    pub stream_own: bool,
    /// The lower-body clip follows the control word's gait and direction (`-directionalanims on|off`).
    // DC-DEFAULT: DSV-28 (refinement, proposed) on
    pub directional_anims: bool,
    /// Stream a thrown object's position every tick from its create to its removal (`-projectile-stream on|off`,
    /// on by default: the 2009 client does not move a type-0x13 object by itself).
    // SPEC: UDP_SESSION_2009_B.md §3.5.4 (Q6: the reference stack streamed the object in every stream message)
    // OBSERVED: capture 20261025_095358_rust4 (stream off: the user saw no throw, only the blast)
    // DC-DEFAULT: DSV-41 (amended, proposed) on
    pub projectile_stream: bool,
    /// Send shell 0x9d `[0]` after `GS_GAME_ENDPOINT` as the reference stack did (`-no9d` omits it).
    // DC-DEFAULT: DSV-36 (proposed) on
    pub send_9d: bool,
    /// Co-op (mode class 5) rule values (`-coop key=value,...`).
    pub coop: coop::CoopTuning,
    /// The in-match HiSec case rules (`-hisec key=value,...`).
    // DC-DEFAULT: HS-1..HS-5 (HS-1 project decision)
    pub hisec: hisec::HisecTuning,
    /// Hardcore (`combat.health_scale`, DSV-482): effective health as a share of normal; 1 = vanilla. The shown scale
    /// stays: [`combat`]'s one damage point divides incoming damage by it.
    pub health_scale: f32,
    /// Gun Game (`-gungame seed:w,w,...`; DSV-481).
    pub gun_game: Option<gungame::GunGameSetup>,
    /// The Search and Destroy warm-up in seconds (`-warmup <s>`, 0 = none; `None`: the room's `train`, at least
    /// [`warmup::WARMUP_MIN_SECS`]; DSV-512).
    pub warmup_secs: Option<u32>,
    /// Player damage in the warm-up (`-combat warmup_damage=on|off`): `false` = every damage with a player source
    /// (shots, melee, grenades, launchers, flames, blasts a player set off, the own grenade) is refused while the
    /// warm-up runs; damage without a player source (falls, kill volumes) stays.
    // DC-DEFAULT: DSV-566 (proposed; USER DECISION 2026-11-04) off
    pub warmup_damage: bool,
    /// Possession (`-possession`; DSV-483).
    pub possession: bool,
    /// Team pings (`-pings`; DSV-651).
    pub pings: bool,
    /// The weapon drop rules (`-drops key=value,...`).
    // DC-DEFAULT: DSV-93..DSV-95 (proposed)
    pub drops: drops::DropTuning,
    /// The destructible world object rules (`-destruct key=value,...`).
    // DC-DEFAULT: DSV-115..DSV-120 (proposed)
    pub destruct: crate::destructibles::DestructTuning,
    /// How long a corpse stays on the clients (`-combat corpse_ms=<ms>`, 0 = until the session ends).
    // DC-DEFAULT: DSV-44 (proposed) 15000 ms (our value; the reference stack's corpses stayed about 18.8 s, OBSERVED only)
    pub corpse_ms: u32,
    /// The part bytes of the weapon grants: `true` = the weapon row's default attachments from the ROM (the L96A1's
    /// scope; without it the 2009 client cannot zoom), `false` = -1 x 4 (`-combat weapon_parts=default|none`).
    // DISASM: cshell+0x60d90 (0xb2 / 0xb3 empty the sockets, attach the parts != -1), cshell+0x1f4180 (zoom reads the
    // socket-1 mod)
    // DC-DEFAULT: DSV-80 (proposed) on
    pub weapon_parts: bool,
    /// The player's own attachments (the lobby's `parts=` token) in the grants, socket by socket over the row's
    /// defaults (`-combat player_parts=on|off`; only with `weapon_parts` on).
    // DISASM: cshell+0x60d90, cshell+0x235ec0 (the client files each part by the mod row's Kinds)
    // DC-DEFAULT: DSV-99 (proposed) on
    pub player_parts: bool,
    /// `b` of a shot effect record that hit a character (`-combat hit_mask=<u16>`, decimal or `0x..`).
    // DC-DEFAULT: DSV-86 (proposed) 0x0f11
    pub hit_mask: u16,
    /// After a character hit, the continuation record of the shot to the wall behind the victim
    /// (`-combat hit_continuation=on|off`).
    // DC-DEFAULT: DSV-86 (proposed) on
    pub hit_continuation: bool,
    /// The shot effect of a hit claim the server refuses by its victim (a dead character, or a team mate without
    /// friendly fire): `true` = the miss form (no hit record, so no client plays a hit on that character), `false` =
    /// the hit form as for an accepted hit (`-combat refused_hits=miss|hit`).
    // OBSERVED: capture 20261025_013028_botduel4 (40 of 40 claims on a dead character got the
    // miss form; tests/refused_cap.rs; REFERENCE)
    // DC-DEFAULT: DSV-101 (proposed) miss
    pub refused_hits_miss: bool,
    /// A character inside its spawn-note window (DSV-63) takes no hit (`-combat spawn_shield=on|off`).
    // EVIDENCE: cshell+0x238eb0 (the shooter's client claims no vector hit on a character whose spawn flag +0x53a is
    // set), cshell+0x206260 (sub 0x1f sets it, sub 0x25 clears it), cshell+0x203e20 (cleared 3500 ms after 0x1f)
    // DC-DEFAULT: DSV-156 (proposed) on: the server refuses such claims too (the 2009 check is the shooter's)
    pub spawn_shield: bool,
    /// Time from a spawn's start note (0xa6 sub 0x1f) to its end note (sub 0x25) (`-combat spawn_note_ms=<ms>`, 0 =
    /// no spawn notes).
    // DC-DEFAULT: DSV-63 (proposed; project decision) 1750 ms
    pub spawn_note_ms: u32,
    /// Pain, fall, head-shot and bounce sounds as tracked sounds (`-combat event_sounds=on|off`).
    // DC-DEFAULT: DSV-64 (proposed) on
    pub event_sounds: bool,
    /// The shot-direction check (`-combat cone_check=log|refuse|off`): every `UP_FIRE` pellet's direction is
    /// rebuilt from the 2009 cone and compared with the claimed one; `log` counts and logs, `refuse` also refuses an
    /// offending pellet's damage and throw.
    // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-6
    // DC-DEFAULT: DSV-102 (proposed; project decision pending on `refuse`, no 2009 server evidence) log
    pub cone_check: crate::cone::ConeMode,
    /// Largest angle in radians between a claimed and a rebuilt pellet direction that still passes
    /// (`-combat cone_tolerance=<rad>`).
    // DC-DEFAULT: DSV-102 (proposed) 2e-5
    pub cone_tolerance: f64,
    /// `own_value` of `GS_MATCH_SETTINGS` 0x7d: the shot counter the client starts from (`-combat shot_seed=<u32>`).
    // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1.5 (both captured servers sent 0)
    // DC-DEFAULT: DSV-103 (proposed) 0
    pub shot_seed: u32,
    /// The hit reaction on the victim's upper body (`-combat hit_clip=on|off`): `on` (the default since the user's
    /// decision of 2026-10-26) streams the ROM body's hit clip once on tracker 1 when a hit takes health and does not
    /// kill. The reference stack never streamed one (tdm1, tdm6); whether the 2009 server did is UNPROVEN (the 2009 client's
    /// own animation table has the two hit clips as actions 6 / 7); `off` gives the reference stack's stream.
    // DISASM: cshell+0x1980 (actions 6 / 7)
    // OBSERVED: capture 20261025_065016_tdm6, 20261025_000014_tdm1 (no hit clip in any stream
    // entry; REFERENCE)
    // DC-DEFAULT: DSV-127 (proposed; project decision: on)
    pub hit_clip: bool,
    /// `combat.mod_damage = on|off`: a hit is scaled by the shooter's granted mods' `Instdamage` (DSV-440).
    // DC-DEFAULT: DSV-440 (proposed)
    pub mod_damage: bool,
    /// `combat.infected_head_scale = <x>`: Quarantine's head factor on an infected victim (DSV-608): a head hit's
    /// zone factor above 1 is scaled by `x`, never below 1 (1 = the human factor).
    // DC-DEFAULT: DSV-608 (proposed; OWNER REPORT 2026-11-05) 0.5
    pub infected_head_scale: f32,
}

/// Default corpse lifetime.
// DC-DEFAULT: DSV-44 (proposed)
pub const CORPSE_MS: u32 = 15_000;

/// Default time between the two spawn notes.
// OBSERVED: capture 20261025_065016_tdm6 c5 / c6 (8 spawn-note pairs on each link: sub 0x25
// 3.47 .. 3.53 s after sub 0x1f), 20261025_002236_tdm4 c6 (3.55 s, 3.53 s); the reference stack's 3500 ms, halved by USER
// DECISION 2026-10-31
// DC-DEFAULT: DSV-63 (proposed)
pub const SPAWN_NOTE_MS: u32 = 1_750;

/// A fire that arrives within this time of the player's spawn does not end its spawn window (DSV-477): it is taken
/// for a shot that was in flight at the death (one-way latency), not a shot of the new life.
// DC-DEFAULT: DSV-477 (amendment, proposed) 250 ms
pub const SPAWN_FIRE_GRACE_MS: u32 = 250;

/// An open spawn-note window (DSV-63): the character object of `pid`, when its end note is due, and the clients that
/// were sent its start note (the end note goes to those alone).
#[derive(Debug, Clone)]
struct SpawnNote {
    pid: u8,
    obj: u16,
    due: Millis,
    to: Vec<u8>,
}

/// The `combat.*` keys an operator may set (`-combat key=value[,key=value...]`, later a `[combat]` table of
/// `server.toml`); every damage multiplier of DSV-30 is one of them.
// DC-DEFAULT: DSV-30 (proposed) the multipliers are config keys; DSV-34 (proposed) the trace keys
pub const COMBAT_KEYS: [&str; 40] = [
    "health_scale",
    "head_factor",
    "body_factor",
    "head_fraction",
    "zone_factors",
    "armor_absorb",
    "armor_gear",
    "armor_cost",
    "start_armor",
    "max_armor",
    "start_health",
    "max_health",
    "default_damage",
    "hit_tolerance",
    "origin_tolerance",
    "trace",
    "trace_slack",
    "rewind_ms",
    "corpse_ms",
    "blast_inner",
    "weapon_parts",
    "player_parts",
    "blast_los",
    "hit_mask",
    "hit_continuation",
    "refused_hits",
    "spawn_note_ms",
    "event_sounds",
    "cone_check",
    "cone_tolerance",
    "shot_seed",
    "hit_clip",
    "mod_damage",
    "warmup_damage",
    "infected_head_scale",
    "flash_secs",
    "flash_behind",
    "flash_damage",
    "hazards",
    "hazard_self",
];

impl GameTuning {
    /// Sets one `combat.*` key (the prefix is optional). `rom` selects the ROM-derived value where a key has one.
    pub fn set_combat(&mut self, key: &str, value: &str) -> Result<(), String> {
        let k = key.trim();
        let k = k.strip_prefix("combat.").unwrap_or(k);
        let v = value.trim();
        // DC-DEFAULT: DSV-76 (proposed) the fall keys (fall::FALL_KEYS)
        if let Some(r) = self.fall.set(k, v) {
            return r;
        }
        let bad = || format!("combat.{k}: bad value {v:?}");
        let num = || {
            v.parse::<f32>()
                .ok()
                .filter(|x| x.is_finite() && *x >= 0.0)
                .ok_or_else(bad)
        };
        let rom_or = || -> Result<Option<f32>, String> {
            if v.eq_ignore_ascii_case("rom") {
                Ok(None)
            } else {
                num().map(Some)
            }
        };
        let switch = |on: &str, off: &str| -> Result<bool, String> {
            if v.eq_ignore_ascii_case(on) {
                Ok(true)
            } else if v.eq_ignore_ascii_case(off) {
                Ok(false)
            } else {
                Err(bad())
            }
        };
        match k {
            "head_factor" => self.head_factor = num()?,
            "body_factor" => self.body_factor = num()?,
            "head_fraction" => self.head_fraction = num()?.min(1.0),
            "zone_factors" => self.zone_factors_from_rom = switch("rom", "off")?,
            "armor_absorb" => self.armor_absorb = rom_or()?.map(|x| x.min(1.0)),
            "armor_gear" => {
                self.armor_gear = if v.eq_ignore_ascii_case("worn") {
                    ArmorGear::Worn
                } else if v.eq_ignore_ascii_case("items") {
                    ArmorGear::Items
                } else if v.eq_ignore_ascii_case("off") {
                    ArmorGear::Off
                } else {
                    ArmorGear::Row(v.parse::<u16>().map_err(|_| bad())?)
                }
            }
            "armor_cost" => self.armor_cost_from_base = switch("base", "hit")?,
            "start_armor" => self.start_armor = rom_or()?,
            "max_armor" => self.max_armor = rom_or()?,
            "start_health" => self.start_health = num()?.max(1.0),
            "max_health" => self.max_health = num()?.max(1.0),
            "default_damage" => self.default_damage = num()?,
            "hit_tolerance" => self.hit_tolerance = num()?,
            "origin_tolerance" => self.origin_tolerance = num()?,
            "trace" => self.trace = switch("on", "off")?,
            "trace_slack" => self.trace_slack = num()?,
            "rewind_ms" => {
                self.rewind_ms = (num()?.round() as u32).min(hits::TRACK_KEEP_MS);
            }
            "corpse_ms" => self.corpse_ms = num()?.min(3_600_000.0).round() as u32,
            // DC-DEFAULT: DSV-39 (amendment, proposed)
            "blast_inner" => self.projectile.inner_fraction = num()?.min(1.0),
            // DC-DEFAULT: DSV-80 (proposed)
            "weapon_parts" => self.weapon_parts = switch("default", "none")?,
            // DC-DEFAULT: DSV-99 (proposed)
            "player_parts" => self.player_parts = switch("on", "off")?,
            // DC-DEFAULT: DSV-92 (proposed)
            "blast_los" => self.projectile.blast_los = switch("on", "off")?,
            // DC-DEFAULT: DSV-86 (proposed)
            "hit_mask" => {
                self.hit_mask = match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
                    Some(h) => u16::from_str_radix(h, 16),
                    None => v.parse::<u16>(),
                }
                .map_err(|_| bad())?
            }
            // DC-DEFAULT: DSV-86 (proposed)
            "hit_continuation" => self.hit_continuation = switch("on", "off")?,
            // DC-DEFAULT: DSV-101 (proposed)
            "refused_hits" => self.refused_hits_miss = switch("miss", "hit")?,
            // DC-DEFAULT: DSV-156 (proposed)
            "spawn_shield" => self.spawn_shield = switch("on", "off")?,
            // DC-DEFAULT: DSV-63 (proposed)
            "spawn_note_ms" => self.spawn_note_ms = num()?.min(3_600_000.0).round() as u32,
            // DC-DEFAULT: DSV-64 (proposed)
            "event_sounds" => self.event_sounds = switch("on", "off")?,
            // DC-DEFAULT: DSV-102 (proposed)
            "cone_check" => self.cone_check = crate::cone::ConeMode::parse(v).ok_or_else(bad)?,
            "cone_tolerance" => self.cone_tolerance = f64::from(num()?),
            // DC-DEFAULT: DSV-103 (proposed)
            "shot_seed" => self.shot_seed = v.parse::<u32>().map_err(|_| bad())?,
            // DC-DEFAULT: DSV-127 (proposed)
            "hit_clip" => self.hit_clip = switch("on", "off")?,
            "mod_damage" => self.mod_damage = switch("on", "off")?,
            // DC-DEFAULT: DSV-566 (proposed; USER DECISION 2026-11-04)
            "warmup_damage" => self.warmup_damage = switch("on", "off")?,
            // DC-DEFAULT: DSV-608 (proposed; OWNER REPORT 2026-11-05)
            "infected_head_scale" => self.infected_head_scale = num()?,
            // DC-DEFAULT: DSV-630 (proposed; OWNER PLAYTEST 2026-11-07) the flash blast's screen flash
            "flash_secs" => self.projectile.flash_secs = num()?.min(60.0),
            "flash_behind" => self.projectile.flash_behind = num()?.min(1.0),
            // DC-DEFAULT: DSV-631 (amended)
            "flash_damage" => self.projectile.flash_damage = switch("on", "off")?,
            // DC-DEFAULT: DSV-635 (proposed; OWNER REPORT 2026-11-07) the thrown rounds' fire and gas
            "hazards" => self.projectile.hazards = switch("on", "off")?,
            "hazard_self" => self.projectile.hazard_self = switch("on", "off")?,
            // DC-DEFAULT: DSV-482 (proposed) Hardcore
            "health_scale" => {
                let s = num()?;
                if !(s > 0.0 && s <= 1.0) {
                    return Err(bad());
                }
                self.health_scale = s;
            }
            _ => {
                return Err(format!(
                    "unknown key combat.{k} (known: {})",
                    COMBAT_KEYS.join(", ")
                ))
            }
        }
        Ok(())
    }
}

impl Default for GameTuning {
    fn default() -> Self {
        GameTuning {
            start_health: 100.0,
            max_health: 100.0,
            start_armor: None,
            max_armor: None,
            armor_absorb: None,
            armor_gear: DEFAULT_ARMOR_GEAR,
            armor_cost_from_base: true,
            player_dims: [24.0, 53.0, 24.0],
            player_dims_from_rom: true,
            body: None,
            identify_wait_ms: 3_000,
            dying_ms: DYING_MS,
            death_states: DeathStates::Reference,
            respawn_wait_ms: None,
            respawn_guard_ms: RESPAWN_GUARD_MS,
            respawn_fallback_ms: RESPAWN_FALLBACK_MS,
            round_secs: None,
            goal: None,
            spy_intel: None,
            agent_health: spy::AGENT_HEALTH,
            agent_armor: spy::AGENT_ARMOR,
            agent_damage_scale: spy::AGENT_DAMAGE_SCALE,
            show_intro: TODO_SHOW_INTRO,
            spawn_on_floor: true,
            waiting_banner: false,
            respawn_match_begin: true,
            end_pause_ms: (END_PAUSE_MS, END_PAUSE_MS),
            timeout_hold_ms: TIMEOUT_HOLD_MS,
            round_end_freeze: ROUND_END_FREEZE,
            freeze_wait_ms: FREEZE_WAIT_MS,
            linger_ms: LINGER_MS,
            round_loop: true,
            between_round_ms: BETWEEN_ROUND_MS,
            max_rounds: None,
            side_switch: modes::SideSwitch::TeamSwap,
            ffa_teams: true,
            team_empty_ms: TEAM_EMPTY_MS,
            ping_period_ms: PING_PERIOD_MS,
            perf_enable: true,
            default_damage: 25.0,
            default_clip: 30,
            default_stock: 90,
            head_fraction: 0.2,
            head_factor: 2.0,
            body_factor: 1.0,
            zone_factors_from_rom: true,
            hit_tolerance: 48.0,
            origin_tolerance: 160.0,
            trace: true,
            trace_slack: 16.0,
            rewind_ms: 200,
            verbose_hits: false,
            projectile: ProjectileTuning::default(),
            move_check: true,
            max_speed: 1500.0,
            move_slack: 64.0,
            box_margin: 256.0,
            default_loadout: Vec::new(),
            spawn_safe_radius: 512.0,
            spawn_occupied_radius: 64.0,
            rng_seed: 0x0DEC_0BA7_5EED,
            multi_kill_window_ms: MULTI_KILL_WINDOW_MS,
            multi_kill_min: MULTI_KILL_MIN,
            stream_budget_bits: 4_000,
            hit_hook: None,
            fall: fall::FallTuning::default(),
            character: crate::chars::CharacterChoice::default(),
            start_order: StartOrder::Reference,
            spawn_pick: SpawnPick::Stream,
            place: place::PlaceConfig::default(),
            stream_own: true,
            directional_anims: DIRECTIONAL_ANIMS,
            projectile_stream: true,
            send_9d: true,
            coop: coop::CoopTuning::default(),
            corpse_ms: CORPSE_MS,
            hit_mask: gs::SHOT_MASK_HIT,
            hit_continuation: true,
            // DC-DEFAULT: DSV-101 (proposed)
            refused_hits_miss: true,
            spawn_shield: true,
            spawn_note_ms: SPAWN_NOTE_MS,
            event_sounds: true,
            // DC-DEFAULT: DSV-102 (proposed)
            cone_check: crate::cone::ConeMode::Log,
            cone_tolerance: crate::cone::DEFAULT_TOLERANCE_RAD,
            // DC-DEFAULT: DSV-103 (proposed)
            shot_seed: 0,
            // DC-DEFAULT: DSV-127 (proposed; project decision: on)
            hit_clip: true,
            mod_damage: true,
            // DC-DEFAULT: DSV-608 (proposed; OWNER REPORT 2026-11-05)
            infected_head_scale: infect::INFECTED_HEAD_SCALE,
            hisec: hisec::HisecTuning::default(),
            health_scale: 1.0,
            gun_game: None,
            warmup_secs: None,
            // DC-DEFAULT: DSV-566 (proposed; USER DECISION 2026-11-04)
            warmup_damage: false,
            possession: false,
            pings: false,
            drops: drops::DropTuning::default(),
            destruct: crate::destructibles::DestructTuning::default(),
            // DC-DEFAULT: DSV-80 (proposed)
            weapon_parts: true,
            // DC-DEFAULT: DSV-99 (proposed)
            player_parts: true,
        }
    }
}

// ------------------------------------------------------------------------------------------------- state

/// Projectile defaults of DSV-39 (see [`GameTuning::projectile`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectileTuning {
    pub speed: f32,
    pub fuse_ms: u32,
    /// Acceleration along +Y (negative: down).
    pub gravity: f32,
    /// Fraction of the speed kept by a bounce.
    pub bounce: f32,
    /// Below this speed after a bounce the object rests.
    pub stop_speed: f32,
    pub area_damage: f32,
    pub area_radius: f32,
    /// Fraction of the blast radius inside which a blast does its full damage; beyond it the damage falls linearly
    /// to 0 at the radius (0 = the plain linear falloff; `-combat blast_inner=<fraction>`).
    // DC-DEFAULT: DSV-39 (amendment, proposed; PLAUSIBLE fit of the three captured 2009 blasts) 0.25
    pub inner_fraction: f32,
    /// A blast damages a player in its radius only when the trace world does not hide the player from the blast
    /// point (`-combat blast_los=on`); `off` = every living player in the radius. Our rule: no 2009 capture has a
    /// wall between a blast and its victim.
    // DC-DEFAULT: DSV-92 (proposed; the default is a project decision pending V-BLAST-LOS) on
    pub blast_los: bool,
    /// Multiplier of a blast's damage to players (`room.blast_damage_scale`, env `DC_ROOM__BLAST_DAMAGE_SCALE`);
    /// 1.0 = the ROM row's `AreaDamage` with the falloff, as the 2009 captures show it.
    // DC-DEFAULT: DSV-165 (gameplay choice, user-tunable) 1.0
    pub blast_damage_scale: f32,
    /// The screen flash of a flash blast (`-combat flash_secs`): its seconds for a player facing the blast in the
    /// full-damage part of the radius, before the falloff ([`crate::game::projectile::flash_secs`]).
    // DC-DEFAULT: DSV-630 (proposed) 6.0
    pub flash_secs: f32,
    /// Factor of the flash for a player whose view points away from the blast (`-combat flash_behind`).
    // DC-DEFAULT: DSV-630 (proposed) 0.5
    pub flash_behind: f32,
    /// A flash blast also hurts by its row's area damage through the blast path (`-combat flash_damage=off`: it only
    /// whites out the screens).
    // OBSERVED: ROM table WEAPONS.TXT (the flash grenade's ammunition row: `AreaDamage` with `AreaDamageType` 34; gear
    // rows' `ProtectionType<k>` 34: armour protects against it; 2026-11-07)
    // DC-DEFAULT: DSV-631 (amended 2026-11-07) on
    pub flash_damage: bool,
    /// A thrown round with progressive damage and a lifetime leaves its fire or gas where it went off
    /// (`-combat hazards=off`: the one-shot flame burst of DSV-455 instead).
    // DC-DEFAULT: DSV-635 (proposed) on
    pub hazards: bool,
    /// The thrower's own fire or gas hurts him (`-combat hazard_self=off`: it does not).
    // DC-DEFAULT: DSV-635 (proposed) on, as his own blast does (DSV-39)
    pub hazard_self: bool,
}

impl Default for ProjectileTuning {
    fn default() -> Self {
        ProjectileTuning {
            speed: 1500.0,
            fuse_ms: 3_000,
            // SPEC: MOVEMENT_2009.md §2 (`GRAVITY` -1100.0, the player's gravity; CAPTURE_tdm1 V-M3)
            gravity: -1100.0,
            bounce: 0.35,
            stop_speed: 100.0,
            area_damage: 100.0,
            area_radius: 400.0,
            inner_fraction: projectile::BLAST_INNER_FRACTION,
            blast_los: projectile::BLAST_LOS,
            blast_damage_scale: 1.0,
            flash_secs: projectile::FLASH_SECS,
            flash_behind: projectile::FLASH_BEHIND,
            flash_damage: true,
            hazards: true,
            hazard_self: true,
        }
    }
}

/// A thrown object the session creates on every client in the world.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectileOut {
    /// The game's key of the flight (the session maps it to an object id).
    pub key: u32,
    pub weapon: u16,
    pub ammo: u16,
    /// Model and skin (`None`: no model resolves, the session creates no object).
    pub look: Option<WeaponLook>,
    pub scale: [f32; 3],
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub rot: [f32; 4],
}

/// What the session has to do for the game layer.
#[derive(Debug, Clone, PartialEq)]
pub enum GameOut {
    /// Send this game message reliably to the player.
    To(u8, BitWriter),
    Pipe(String),
    Log(String),
    /// Drop the player's link with this bye code.
    Kick(u8, u32),
    /// A milestone of the player (for the session's step list).
    Mark(u8, Milestone),
    /// The match is over and the hold times passed: end the session.
    Finish,
    /// The player spawned; `first` = its first spawn (the session completes the character record on every client).
    Spawned {
        pid: u8,
        pos: [f32; 3],
        yaw: f32,
        first: bool,
    },
    /// The character becomes visible at `pos` on every client (spawn flag words).
    Visible {
        pid: u8,
        pos: [f32; 3],
        yaw: f32,
    },
    /// Send this game message unreliably to the player (effect relays).
    Unreliable(u8, BitWriter),
    /// The weapon model in the player's hand changed (the session sends the new model to every client).
    HandWeapon {
        pid: u8,
        weapon: u16,
    },
    /// Create a thrown object on every client in the world (DSV-39).
    Projectile(Box<ProjectileOut>),
    /// Remove the thrown object of this key from every client (its blast, if any, follows as effects).
    ProjectileGone {
        key: u32,
    },
    /// Co-op: create the objects of a new AI slot on every client in the world (and bind them, [`Game::bind_ai`]).
    AiCreate {
        slot: usize,
    },
    /// Co-op: a reused AI slot becomes visible at its spawn.
    AiShow {
        slot: usize,
    },
    /// Co-op: an AI died: its objects are hidden at the park point and a corpse object at `pos` facing `yaw` plays
    /// `clip` (combined clip index, length in ms) for the co-op corpse time (DSV-585).
    AiDied {
        slot: usize,
        pos: [f32; 3],
        yaw: f32,
        clip: (u16, u32),
    },
    /// Co-op: an AI left the world without a death (its slot went to a waiting spawn, DSV-642): its objects are
    /// hidden at the park point, no corpse.
    AiHidden {
        slot: usize,
    },
    /// Co-op: a reused AI slot gets another body and weapon model (`CF_FILES`).
    AiLook {
        slot: usize,
    },
    /// Co-op: run this command text with the world's statements (a mission AI's death command, DSV-571).
    WorldScript(Vec<u8>),
    /// The player died: on every client its character is hidden and loses its weapon model, and a corpse object at
    /// `pos` facing `yaw` plays `clip` (combined clip index, length in ms) (DSV-44).
    Died {
        pid: u8,
        pos: [f32; 3],
        yaw: f32,
        clip: (u16, u32),
    },
    /// Create a HiSec case on every client in the world (HS-2); the 0xd6 messages follow as `To`.
    CaseCreate(Box<hisec::CaseOut>),
    /// The countdown to the case object of this key (0xa6 sub 2, HS-3).
    CaseNote {
        key: u32,
        remaining: u32,
    },
    /// Remove the case object of this key from every client (pickup or lifetime end).
    CaseGone {
        key: u32,
    },
    /// Start an event sound on every client in the world and remove it after its life (DSV-64).
    Sound(Box<crate::sounds::SoundOut>),
    /// Create a dropped weapon on every client in the world (DSV-93).
    DropCreate(Box<drops::DropOut>),
    /// Remove the dropped weapon of this key from every client (taken, lifetime, round end).
    DropGone {
        key: u32,
    },
    /// A breakable died: switch its model off on every client and create its death effects (DSV-117).
    WorldBreak(Box<destruct::BreakOut>),
    /// Remove the death effect object of this key from every client (DSV-118).
    WorldFxGone {
        key: u32,
    },
    /// Switch the world's placed effect of this name on (a placed-effect object on every client) under `key`, until
    /// its [`GameOut::WorldFxGone`] (the Spy Hunt station, DSV-323).
    WorldFx {
        key: u32,
        name: Vec<u8>,
    },
    /// A dropped capture flag's seconds to its return (0: none), as the countdown note to its object (DSV-290).
    FlagNote {
        obj: u16,
        remaining: u32,
    },
    /// A capture flag moved (taken along, dropped, home again): its position on every client (DSV-193).
    FlagMove {
        obj: u16,
        pos: [f32; 3],
    },
    /// A bomb was planted at `pos` in the site of zone number `site`: its model, its indicator effect and the defuse
    /// point at it on every client (DSV-220).
    BombShown {
        site: i32,
        pos: [f32; 3],
    },
    /// The planted bomb's objects leave every client (defused, blew up, new round).
    BombHidden,
    /// Switch the world's placed effect of this name on or off on every client (the bomb site's planted command,
    /// CLI-1340): a world effect object that is already planned is removed / created again, another one is created
    /// / removed; until [`GameOut::WorldFxRestore`].
    WorldFxSwitch {
        name: Vec<u8>,
        on: bool,
    },
    /// Every effect switched by [`GameOut::WorldFxSwitch`] goes back to the world's planned state (the next round).
    WorldFxRestore,
    /// The bomb blew up at `pos`: the blast effect on every client (DSV-221).
    BombBlast {
        pos: [f32; 3],
    },
    /// The blast effect object leaves every client.
    BombBlastGone,
    /// The player's character look changed (the infected look and back, DSV-214): its model block on every client.
    CharacterLook {
        pid: u8,
    },
    /// Create a Spy Hunt intel case on every client in the world (DSV-230).
    IntelCreate(Box<spy::IntelOut>),
    /// Remove the intel case of this key from every client (picked up, dropped elsewhere, back at a spawn point).
    IntelGone {
        key: u32,
    },
    /// A capture flag's carrier changed (DSV-217): `carrier` = the player who carries it now (`None`: it lies or is
    /// home), `before` = the one who carried it.
    FlagCarry {
        obj: u16,
        team: u8,
        carrier: Option<u8>,
        before: Option<u8>,
    },
}

/// Game-layer milestones of one player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Milestone {
    IdentifyRequest,
    PlayerInfo,
    Introduced,
    WaitingState,
    WorldReady,
    Spawned,
}

/// Life of a player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Life {
    /// Placed, not yet introduced (waiting for `UP_PLAYER_INFO`).
    Joining,
    /// State 3 sent, waiting for `UP_WORLD_READY` (or for the round).
    Loading,
    Alive,
    /// State 2 since the death.
    Dying {
        died: Millis,
    },
    /// State 3 after the dying time.
    Waiting {
        died: Millis,
    },
}

/// Server mirror of one carried weapon.
// DC-DEFAULT: DC-C2S-4 (proposed) the server mirrors clip and stock
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeaponState {
    pub weapon: u16,
    pub ammo: u16,
    /// 0 = no clip limit (weapons without ammunition).
    pub clip_size: u32,
    pub clip: u32,
    pub stock: u32,
    /// The weapon slot it was granted in (0..3; adopted weapons: 0).
    pub slot: u8,
    /// The weapon is its own ammunition (ROM `IsAmmo`: grenades, mines, kits): the granted amount counts every
    /// round the player carries, the one in the hand included, so the clip is taken out of it.
    // OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] IsAmmo, ShotsPerClip; [Ammo<n>] MaxAmount
    // DC-DEFAULT: DSV-39 (amended, proposed)
    pub own_ammo: bool,
    /// The part bytes the weapon was granted with (`GS_WEAPON_GIVE` / `GS_WEAPON_EQUIP`); the shot record's mods
    /// come from them.
    // DC-DEFAULT: DSV-167 (proposed)
    pub parts: [i8; 4],
    shot_open: bool,
    shot_ok: bool,
}

impl WeaponState {
    /// Every round the player carries: clip and stock.
    pub fn carried(&self) -> u32 {
        self.clip.saturating_add(self.stock)
    }

    /// The amount the client is told (`GS_WEAPON_GIVE` / `GS_WEAPON_EQUIP` `ammo`, `GS_PLAYER_STATUS` kind 1): every
    /// carried round for a weapon that is its own ammunition (the grant's count, as the reference stack's 1.0 for a single
    /// grenade), the stock outside the clip for the others.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (kind 1 sets the slot's ammunition count), §4.4 (0xb2 `ammo`)
    // OBSERVED: reference tdm4 / tdm6 (weapon 7, one grenade: give 1.0, status 1.0, after the throw 0.0; REFERENCE)
    // DC-DEFAULT: DSV-39 (amended, proposed); DC-C2S-4 (proposed) other weapons unchanged
    pub fn told_amount(&self) -> f32 {
        if self.own_ammo {
            self.carried() as f32
        } else {
            self.stock as f32
        }
    }
}

/// One player of the match.
#[derive(Debug, Clone)]
pub struct Player {
    pub pid: u8,
    pub uid: Option<u32>,
    pub roster: Option<RosterEntry>,
    pub name8: Vec<u8>,
    pub name16: Vec<u16>,
    /// 1 or 2.
    pub team: u8,
    pub obj: u16,
    pub look: Look,
    pub info: Option<UpPlayerInfo>,
    pub introduced: bool,
    intro_deadline: Millis,
    pub ready: bool,
    pub life: Life,
    pub health: f32,
    pub armor: f32,
    /// How the armour shares a hit while it lasts (the worn armour, DSV-30).
    pub armor_absorb: Absorb,
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub yaw: f32,
    pub move_seq: u8,
    last_move: Option<Millis>,
    pub updates: u64,
    pub stale_updates: u64,
    pub corrections: u64,
    pub weapons: Vec<WeaponState>,
    pub current: Option<usize>,
    /// The multi-kill chain: time and level of this player's last foe kill in this life (DC-C2S-13 / DSV-105).
    kill_chain: Option<(Millis, u8)>,
    /// Who killed this player in its current death (cleared on spawn); the visibility layer keeps him shown (VIS-14).
    pub killed_by: Option<u8>,
    /// Kills of this player since its last spawn.
    pub kills_this_life: u32,
    pub client_ping_ms: Option<u16>,
    pub link_rtt_ms: Option<u16>,
    pub spawns: u32,
    /// Shell 0x90 [0] / 0x88 [3] already went out at the link (a joiner of a running round).
    pub switch_sent: bool,
    /// Per remote object: last stream send time and last rotation sent to this player.
    stream_seen: BTreeMap<u16, (Millis, Option<[f32; 4]>)>,
    /// What other clients see (slice 3); `None` = a plain object (no ROM character tables).
    pub character: Option<CharacterLook>,
    pub motion: Motion,
    motion_since: Millis,
    spawned_at: Millis,
    /// The corpse's death clip (combined index, length in ms) the dead character's main tracker plays until the next
    /// spawn (DSV-44 amendment); `None` while alive.
    pub death_clip: Option<(u16, u32)>,
    /// The character has been made visible (first spawn).
    pub visible: bool,
    /// Weapon whose model is in the character's hand.
    pub hand_weapon: Option<u16>,
    /// Last `sprinting` bit of `UP_SPRINT_STATE` (the echo is sent on a change).
    pub sprinting: Option<bool>,
    /// The client switched its heartbeat sensor on (`UP_SCAN` sub 1; cleared at the spawn and the round end),
    /// DSV-615. It counts only while [`Game::scan_reach`] says so (introduced, alive, a sensor row drawn).
    pub scanning: bool,
    /// The last radar switch answered to the client (`None`: none since the spawn); an answer goes out only when
    /// the verdict changes.
    scan_answered: Option<bool>,
    /// Time of the last `UP_THROW_HOLD`: the next `UP_FIRE` is a throw, not a bullet.
    last_throw: Option<Millis>,
    /// Last view-angle repair (CV2-5).
    view_repair_at: Option<Millis>,
    /// The aux words of the last relayed aim note (DSV-153).
    aim_sent: Option<[u32; 6]>,
    /// The health and armour of the last vitals notes and when they went out (DSV-521).
    vitals_sent: Option<([u16; 2], Millis)>,
    pub view_repairs: u32,
    /// Recent positions for the bounded rewind (DSV-42).
    track: hits::Track,
    /// The fall in progress (DSV-76).
    fall: Option<fall::FallTrack>,
    /// The round-end freeze went out and no release since (DSV-77).
    pub frozen: bool,
    /// An airborne player at the round result: frozen at his landing or at this time (DSV-77).
    freeze_due: Option<Millis>,
    /// The shot-direction check's state of this link (DSV-102).
    pub cone: crate::cone::LinkCone,
    /// The character's upper-body action and ground state (DSV-125).
    pub pose: crate::anims::Pose,
    /// The running Possession move (DSV-483).
    pub flight: Option<possession::Flight>,
    /// The last grounded report without the crouch bit, or the last placement (DSV-483's safe end point).
    pub stand_pos: Option<[f32; 3]>,
    /// The last accepted report was grounded.
    pub on_ground_report: bool,
}

/// Match counters of one account's current stay (a rejoin starts them again, DSV-621).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stats {
    pub entry: Option<RosterEntry>,
    pub kills: u32,
    pub deaths: u32,
    pub team_kills: u32,
    pub headshots: u32,
}

/// Round phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundPhase {
    /// No player is ready yet.
    Waiting,
    Running {
        start: Millis,
    },
    /// The round ended at `at` (`GS_ROUND_RESULT` sent); `win` 1, 2 or 0 (draw); `stage` 1 once the summary set went
    /// out, 2 once the result screen went out (then the server waits for the clients to leave). `last` is false for
    /// a round of a round mode that the next round follows (GMD-21): `win` is then that round's winner, stage 1 the
    /// between-round set, and the next round starts after the between-round pause; when `last` is true `win` is the
    /// match winner.
    Over {
        at: Millis,
        win: u8,
        stage: u8,
        last: bool,
    },
    Closed,
}

/// End-set pause of DSV-35.
// DC-DEFAULT: DSV-35 (proposed) 3000 ms
pub const END_PAUSE_MS: u32 = 3_000;
/// First pause of a time-limit end (`-timeout-hold`). The 2009 client draws the scoreboard panel itself 3 s after
/// the 0x46 (cshell+0xc02e0), so a shorter hold only moves the result screen onto that moment; with the DSV-77 freeze
/// the original sequence (3 s frozen banner, 3 s scoreboard, result) needs the reference stack's 3 s here too.
// EVIDENCE: cshell+0xc02e0 (af-endfreeze); OBSERVED: rust7 c5 (hold 0: 0x3f at 0x46 + 3.016 s, the panel never showed)
// DC-DEFAULT: DSV-35 (amendment, proposed) 3000 ms (coordinator 2026-10-25, pending the user's play check)
pub const TIMEOUT_HOLD_MS: u32 = 3_000;
/// Linger limit after the result screen.
// DC-DEFAULT: DSV-35 (proposed) 120 s
pub const LINGER_MS: u32 = 120_000;
/// DSV-31: an emptied team loses the round after this long.
// DC-DEFAULT: DSV-31 (proposed) 10 s
pub const TEAM_EMPTY_MS: u32 = 10_000;
/// The pause between the between-round set (`GS_ROUND_CLOSE` with this many seconds) and the next round's
/// `GS_ROUND_BEGIN` in the round modes.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1 row 0x48 (`hud_banner` timed state with `round_secs`); GAME_MODES_2009.md §4.1
// DC-DEFAULT: GMD-5 (proposed) 5 s; GMD-21 (staged) the between-round order
pub const BETWEEN_ROUND_MS: u32 = 5_000;

/// The room rules taken from the lobby's `room` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomRules {
    pub mode_class: u8,
    pub goal: u32,
    pub round_secs: u32,
    pub team_damage: bool,
    /// The room's weapon limit (`arms=`, the waiting room's weapon list row; 0 = all permitted).
    pub arms: u32,
}

/// The game layer.
pub struct Game {
    cfg: GameTuning,
    tables: RomTables,
    pbox: PosBox,
    centre: [f32; 3],
    /// The world geometry shots are traced against (slice 4, DSV-34).
    trace: Option<std::sync::Arc<dc_sim::CollisionWorld>>,
    /// The geometry and the navigation grid of the co-op AI (FT-S11).
    coop_nav: coop::NavSlot,
    /// The surface id of each world polygon (V-SURF-1, DSV-62).
    surfaces: Option<std::sync::Arc<crate::surface::SurfaceProbe>>,
    /// Spawn points with their team (None = any team).
    spawns: Vec<([f32; 3], [f32; 3], Option<u8>)>,
    /// The mode filter text of each spawn point (index-aligned with `spawns`; the intro's team bases, DSV-50).
    spawn_modes: Vec<Option<Vec<u8>>>,
    /// Whether each spawn point is a DeCombat-vocabulary object (index-aligned with `spawns`; MAP-21).
    spawn_dc: Vec<bool>,
    /// The name of each spawn point and whether it is locked now (index-aligned with `spawns`; DSV-569), with the
    /// file's locks for a round reset.
    spawn_names: Vec<Vec<u8>>,
    spawn_locked: Vec<bool>,
    spawn_locked_file: Vec<bool>,
    room: RoomSettings,
    rules: RoomRules,
    players: BTreeMap<u8, Player>,
    stats: BTreeMap<u32, Stats>,
    phase: RoundPhase,
    scores: [u32; 2],
    round_wins: [u32; 2],
    /// The round number of the match (1 from the first round start; GMD-20) and whether each team had a living player
    /// in the current round (the elimination test, GMD-22).
    round_no: u32,
    round_alive_seen: [bool; 2],
    /// The dead breakables the last round starts brought back (object ids, GMD-24), until the session takes them to
    /// switch their brushes on again on the clients (`take_restored_breakables`). At most one entry per breakable.
    restored_breakables: Vec<u16>,
    /// The lobby's computed gains per account (`gain` control lines) for the summary (exp, GP).
    gains: BTreeMap<u32, (u32, u32, u32, u32)>,
    /// DSV-31: whether each team had a player in this round, and since when it has none.
    team_seen: [bool; 2],
    team_empty_since: [Option<Millis>; 2],
    /// DSV-36: when each account's `user` line arrived, the round's start, and the accounts that had their first
    /// spawn of the round (`GS_MATCH_BEGIN` goes only to the round's first spawn).
    roster_since: BTreeMap<u32, Millis>,
    round_started_at: Option<Millis>,
    round_spawned: std::collections::BTreeSet<u32>,
    next_ping: Option<Millis>,
    rng: u64,
    reported: bool,
    out: Vec<GameOut>,
    /// Body model files read so far (slice 3).
    rigs: RigCache,
    vfs: Option<dc_rez::RezVfs>,
    /// The game server's address for `GS_GAME_ENDPOINT` (host text, port).
    endpoint: (Vec<u8>, u32),
    /// Thrown objects in flight (DSV-39) and the next key.
    flights: Vec<projectile::Flight>,
    next_flight: u32,
    /// Burning players (DSV-455).
    burns: Vec<projectile::Burn>,
    /// Lingering fire and gas of thrown rounds (DSV-635).
    hazards: Vec<projectile::Hazard>,
    /// Co-op state of a class-5 room (slice 1), the world's co-op objects and the world's announced path (the GMS
    /// of the room is the one naming this world).
    coop: Option<coop::Coop>,
    coop_world: crate::world::CoopWorld,
    world_announce: String,
    /// Verdicts of the hit claims (DSV-42).
    hit_counts: hits::HitCounters,
    /// Verdicts of the shot-direction check (DSV-102) and whether the pellet in hand is refused by it.
    cone_counts: crate::cone::ConeCounters,
    cone_refused: bool,
    /// Player and weapon pairs whose refusal under the room's weapon limit was logged (DSV-476).
    barred_logged: std::collections::BTreeSet<(u8, u16)>,
    /// The match's first kill went out (its 0x6d carried `streak_flag` 1, DSV-33).
    first_kill_done: bool,
    /// FFA counters and the side switch (DSV-147..DSV-149).
    modes: modes::ModeState,
    /// HiSec cases in the world and the next case key (HS-1..HS-5; keys start at 1 as in the reference stack's creates).
    cases: Vec<hisec::Case>,
    /// Open spawn-note windows (DSV-63).
    spawn_notes: Vec<SpawnNote>,
    place: place::PlaceState,
    /// The first pause of the current round end (DSV-35): `end_pause_ms.0`, or `timeout_hold_ms` after a time-limit
    /// end.
    end_hold_ms: u32,
    /// Play times of the ROM sound files used so far (DSV-64).
    play_ms_cache: BTreeMap<String, u32>,
    /// Until when each player's last pain sound plays (DSV-64).
    pain_until: BTreeMap<u8, Millis>,
    /// The player whose damage is being applied is falling to his end (DSV-596): `(pid, true)` for the damage of a
    /// landing, `(pid, false)` for a drop through a kill volume. Set around the one damage call, read by its sound.
    plunge: Option<(u8, bool)>,
    next_case_key: u32,
    /// Weapons dropped at deaths and the next drop key (DSV-93).
    drops: Vec<drops::Drop>,
    next_drop_key: u32,
    /// The packs dead co-op AI dropped (DSV-591) and their models, resolved from the ROM on first use.
    packs: Vec<packs::Pack>,
    pack_looks: Option<packs::PackLooks>,
    /// The breakables and their death effects (DSV-115).
    destruct: destruct::DestructState,
    /// The flags, capture zones and captures of a flag-mode room (DSV-190..DSV-193).
    ctf: ctf::CtfState,
    /// The bomb sites, carrier, hold and planted bomb of a Search and Destroy room (DSV-200..DSV-204).
    bomb: bomb::SndState,
    /// The infected, the outbreak and the respawns of a Quarantine room (DSV-210..DSV-213).
    infect: infect::InfectState,
    /// The intel, spies, agent and points of a Spy Hunt room (DSV-230..DSV-235).
    spy: spy::SpyState,
    /// The end of the running warm-up (DSV-512), and whether this match had it.
    warmup_until: Option<Millis>,
    warmup_done: bool,
    /// Other players' health for every client (DSV-480; `None`: a culled match sends none).
    remote_health: Option<remotehp::RemoteHealth>,
    /// Team pings (DSV-651).
    team_pings: teamping::TeamPings,
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn len3(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn finite3(a: [f32; 3]) -> bool {
    a.iter().all(|c| c.is_finite())
}

/// Seconds of the lobby's `time=` token. The ROM mode files give most limits in seconds (180, 600, 1080) but the
/// Quarantine and Spy Hunt files give minutes (`TimeLimit` 3 / 10); a value below one minute is read as minutes.
// DC-DEFAULT: DSV-250 (proposed) `time=` values 1..59 are minutes (ROM mode files: CTF / TMM / TSV / CMM / CTSV 180,
// CCTF 600, COOP 1080 / 2700 are seconds; QRT 3, BNT 10 cannot be seconds); 0 = no timer (DSV-20)
pub fn room_time_secs(v: i64) -> u32 {
    match v {
        i64::MIN..=0 => 0,
        1..=59 => v as u32 * 60,
        _ => v.min(u32::MAX as i64) as u32,
    }
}

impl Game {
    pub fn new(cfg: GameTuning, world: &WorldInfo, tables: RomTables) -> Self {
        let spawns = world
            .spawns
            .iter()
            .map(|s| (s.pos, s.rot, tables.teams.team_of(s.team_filter.as_deref())))
            .collect();
        let rng = cfg.rng_seed | 1;
        let mut g = Game {
            pbox: PosBox {
                min: world.extent_min,
                max: world.extent_max,
            },
            centre: world.centre(),
            trace: world.trace.clone(),
            coop_nav: coop::NavSlot::new(world),
            surfaces: world.surfaces.clone(),
            spawns,
            spawn_modes: world.spawn_modes.clone(),
            spawn_dc: world.spawn_dc.clone(),
            spawn_names: world.spawn_names.clone(),
            spawn_locked: world.spawn_locked.clone(),
            spawn_locked_file: world.spawn_locked.clone(),
            room: RoomSettings::default(),
            rules: RoomRules {
                mode_class: GENERIC_MODE_CLASS,
                goal: 0,
                round_secs: 0,
                team_damage: false,
                arms: 0,
            },
            players: BTreeMap::new(),
            stats: BTreeMap::new(),
            phase: RoundPhase::Waiting,
            scores: [0; 2],
            round_wins: [0; 2],
            round_no: 0,
            round_alive_seen: [false; 2],
            restored_breakables: Vec::new(),
            gains: BTreeMap::new(),
            team_seen: [false; 2],
            team_empty_since: [None; 2],
            roster_since: BTreeMap::new(),
            round_started_at: None,
            first_kill_done: false,
            modes: modes::ModeState::default(),
            round_spawned: std::collections::BTreeSet::new(),
            next_ping: None,
            rng,
            reported: false,
            out: Vec::new(),
            rigs: RigCache::default(),
            vfs: tables.rom.as_ref().map(dc_rez::RezVfs::new),
            endpoint: (
                b"127.0.0.1".to_vec(),
                u32::from(dc_wire::udp::DEFAULT_GAME_PORT),
            ),
            flights: Vec::new(),
            next_flight: 1,
            burns: Vec::new(),
            hazards: Vec::new(),
            coop: None,
            coop_world: world.coop.clone(),
            world_announce: world.announce.clone(),
            hit_counts: hits::HitCounters::default(),
            cone_counts: crate::cone::ConeCounters::default(),
            cone_refused: false,
            barred_logged: Default::default(),
            cases: Vec::new(),
            spawn_notes: Vec::new(),
            place: place::PlaceState::default(),
            end_hold_ms: 0,
            play_ms_cache: BTreeMap::new(),
            pain_until: BTreeMap::new(),
            plunge: None,
            next_case_key: 1,
            drops: Vec::new(),
            next_drop_key: 1,
            packs: Vec::new(),
            pack_looks: None,
            destruct: destruct::DestructState::new(world.destructibles.clone()),
            ctf: ctf::CtfState::from_world(world, |t| tables.teams.team_of(t)),
            bomb: bomb::SndState::from_world(world),
            infect: infect::InfectState::default(),
            spy: spy::SpyState::from_world(world),
            warmup_until: None,
            warmup_done: false,
            remote_health: None,
            team_pings: teamping::TeamPings::default(),
            tables,
            cfg,
        };
        g.apply_room();
        g
    }

    // ------------------------------------------------------------------------------------------- accessors

    pub fn config(&self) -> &GameTuning {
        &self.cfg
    }

    pub fn tables(&self) -> &RomTables {
        &self.tables
    }

    pub fn rules(&self) -> &RoomRules {
        &self.rules
    }

    /// How a dead player comes back in the room's mode class.
    // DISASM: cshell+0x60b2e..0x60c0e (the client's own wait by class); GAME_MODES_2009.md §4.2, GMD-13 (proposed: no
    // respawn inside a round of the round modes)
    // DC-DEFAULT: DSV-32 (proposed) request-driven respawn, fallback only where the client may not ask
    pub fn respawn_rule(&self) -> RespawnRule {
        let wait = |w: u32| self.cfg.respawn_wait_ms.unwrap_or(w);
        match self.rules.mode_class {
            1 | 4 | 6 | 7 | 8 => RespawnRule::ClientRequest {
                wait_ms: wait(CLIENT_RESPAWN_WAIT_MS),
                fallback_ms: None,
            },
            10 => RespawnRule::ClientRequest {
                wait_ms: wait(CLIENT_RESPAWN_WAIT_CLASS10_MS),
                fallback_ms: None,
            },
            5 | 9 => RespawnRule::ClientRequest {
                wait_ms: wait(CLIENT_RESPAWN_WAIT_MS),
                fallback_ms: Some(self.cfg.respawn_fallback_ms),
            },
            2 | 3 | 11 | 12 | 13 => RespawnRule::NextRound,
            _ => RespawnRule::ServerTimer {
                after_ms: wait(CLIENT_RESPAWN_WAIT_MS),
            },
        }
    }

    pub fn phase(&self) -> RoundPhase {
        self.phase
    }

    /// The round number of the match: 0 before the first round, then 1, 2, ... (GMD-20).
    pub fn round_no(&self) -> u32 {
        self.round_no
    }

    /// The dead breakables that round starts restored since the last call (GMD-24): the session switches each brush's
    /// created flag words on again on every client (`destructibles::restored`, `brush_flags_record`).
    // DC-DEFAULT: GMD-24 (staged)
    pub fn take_restored_breakables(&mut self) -> Vec<u16> {
        std::mem::take(&mut self.restored_breakables)
    }

    /// The restored breakables the session has not taken yet (GMD-24).
    pub fn restored_breakables(&self) -> &[u16] {
        &self.restored_breakables
    }

    /// The round wins of the two teams so far.
    pub fn round_wins(&self) -> [u32; 2] {
        self.round_wins
    }

    /// Whether the room's class plays several rounds per match: the classes without a respawn inside a round
    /// (2, 3, 11, 12, 13), unless `round_loop` is off.
    // EVIDENCE: cshell+0x61e40 (no respawn request in classes 2, 3, 11, 12, 13; GAME_MODES_2009.md §4.2, PROVEN)
    // DC-DEFAULT: GMD-20 (staged)
    pub fn round_loop(&self) -> bool {
        self.cfg.round_loop && self.respawn_rule() == RespawnRule::NextRound
    }

    /// `round_limit` of `GS_ROUND_BEGIN` / `GS_MATCH_SETTINGS`: the goal (rounds to win) in a round mode, else 1.
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.2 row 0x7d (`goal` drawn as the round limit for 2, 3, 12, 13), §2.1 row 0x44
    // (`round_limit` stored; the half-time check compares `round_limit / 2`; PLAUSIBLE)
    // DC-DEFAULT: GMD-23 (staged) round_limit = goal
    pub fn round_limit(&self) -> u32 {
        if self.round_loop() {
            self.rules.goal.max(1)
        } else {
            1
        }
    }

    /// The team values of `GS_ROUND_WINS` 0xb5 and the 0x7d scores: the round wins in a round mode, the team kills
    /// otherwise.
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.2 row 0xb5 (the scoreboard values; the TDM kill totals REFERENCE-CONFIRMED);
    // GAME_MODES_2009.md §4.1 (0xb5 round wins after each round, PLAUSIBLE)
    // DC-DEFAULT: DC-KILL-3 (proposed) kills in TDM; GMD-21 (staged) round wins in the round modes
    pub(crate) fn team_counters(&self) -> [u32; 2] {
        // DC-DEFAULT: DSV-192 (proposed) the flag modes count captures (the room goal is in captures)
        if self.is_flag() {
            return self.ctf.captures;
        }
        if self.round_loop() {
            self.round_wins
        } else {
            self.scores
        }
    }

    pub fn scores(&self) -> [u32; 2] {
        self.scores
    }

    pub fn players(&self) -> &BTreeMap<u8, Player> {
        &self.players
    }

    pub fn stats(&self) -> &BTreeMap<u32, Stats> {
        &self.stats
    }

    /// Verdicts of the hit claims so far (DSV-42).
    pub fn hit_counters(&self) -> &hits::HitCounters {
        &self.hit_counts
    }

    /// The `hits:` counter line (DSV-42).
    pub fn hit_summary(&self) -> String {
        self.hit_counts.summary()
    }

    /// Verdicts of the shot-direction check so far (DSV-102).
    pub fn cone_counters(&self) -> &crate::cone::ConeCounters {
        &self.cone_counts
    }

    /// The `cone:` counter line (DSV-102).
    pub fn cone_summary(&self) -> String {
        self.cone_counts.summary(self.cfg.cone_check)
    }

    pub fn reported(&self) -> bool {
        self.reported
    }

    pub fn take_out(&mut self) -> Vec<GameOut> {
        std::mem::take(&mut self.out)
    }

    /// Current position of a player's object (for create records sent to later joiners).
    pub fn position_of(&self, pid: u8) -> Option<[f32; 3]> {
        self.players.get(&pid).map(|p| p.pos)
    }

    // ------------------------------------------------------------------------------------------- room

    /// The lobby's `room` line.
    // SPEC: DSERVER_PIPE.md §3.2 (`key type goal ff time`); GAME_MESSAGES_2009.md §1.2 (`type` = the mode row's
    // game type, read by the lobby from the ROM mode table at run time)
    pub fn set_room(&mut self, room: &RoomSettings) {
        self.room = room.clone();
        self.apply_room();
    }

    fn apply_room(&mut self) {
        let num = |k: &str| self.room.get(k).and_then(|v| v.trim().parse::<i64>().ok());
        self.rules = RoomRules {
            mode_class: num("type")
                .map(|v| v.clamp(0, 255) as u8)
                .unwrap_or(GENERIC_MODE_CLASS),
            goal: self
                .cfg
                .goal
                .unwrap_or_else(|| num("goal").unwrap_or(0).max(0) as u32),
            round_secs: self
                .cfg
                .round_secs
                .unwrap_or_else(|| room_time_secs(num("time").unwrap_or(0))),
            team_damage: num("ff").unwrap_or(0) != 0,
            arms: num("arms").unwrap_or(0).clamp(0, u32::MAX as i64) as u32,
        };
        // DC-DEFAULT: DSV-481 (proposed) the goal is the ladder's length, no weapon limit
        self.gun_game_apply_room();
        self.possession_apply_room();
        // DC-DEFAULT: DSV-393 (proposed) Spy Hunt ends only at the upload or the time limit (DSV-392), so a room time
        // of 0 ("unlimited") falls back to `spy::FALLBACK_SECS`, the class-9 entry of the GMD-2 round-time policy
        // (ROM mode table GAMEROOM.TXT [Mode<n>] has no time key; the BNT GMS TimeLimit 10 is already the lobby's
        // `time=` when the file gives one): 10 minutes
        if self.rules.mode_class == gs::MODE_CLASS_9 && self.rules.round_secs == 0 {
            self.rules.round_secs = spy::FALLBACK_SECS;
        }
        // DC-DEFAULT: DSV-236 (proposed) Spy Hunt is everyone against everyone: hunters (all team 1) hurt each other
        if self.rules.mode_class == gs::MODE_CLASS_9 {
            self.rules.team_damage = true;
        }
        // DC-DEFAULT: FT-20 (proposed) a class-5 room line turns the co-op rules on (the lobby decides whether it
        // offers the row)
        if self.rules.mode_class == gs::MODE_COOP && self.phase == RoundPhase::Waiting {
            let goal = self.rules.goal.min(2);
            if self.coop.as_ref().is_none_or(|c| c.goal != goal) {
                self.coop_setup();
            }
        } else if self.rules.mode_class != gs::MODE_COOP {
            self.coop = None;
        }
    }

    // ------------------------------------------------------------------------------------------- helpers

    fn send(&mut self, pid: u8, m: BitWriter) {
        self.out.push(GameOut::To(pid, m));
    }

    fn log(&mut self, s: String) {
        self.out.push(GameOut::Log(s));
    }

    fn introduced(&self) -> Vec<u8> {
        self.players
            .values()
            .filter(|p| p.introduced)
            .map(|p| p.pid)
            .collect()
    }

    fn broadcast(&mut self, m: &BitWriter) {
        for pid in self.introduced() {
            self.send(pid, m.clone());
        }
    }

    fn next_rand(&mut self) -> u64 {
        // xorshift64* (public algorithm, Marsaglia / Vigna)
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Team of a new player in this room: every human is on team 1 in a co-op room (the AIs are team 2), else
    /// [`Game::team_for`].
    // DC-DEFAULT: GMD-15 (proposed)
    // DC-DEFAULT: GMD-10 (proposed) one team value per player in FFA; DSV-149 (proposed) the switched sides
    pub fn team_in_room(&self, entry: Option<&RosterEntry>, pid: u8) -> u8 {
        if self.coop.is_some() {
            coop::HUMAN_TEAM
        } else {
            self.team_without_coop(entry, pid)
        }
    }

    /// Team (1 or 2) of a roster entry, else alternating by client id.
    // SPEC: DSERVER_PIPE.md §3.1 (`team` 0 = Alpha, 1 = Bravo); GAME_MESSAGES_2009.md §1.2 (`team` 1 / 2 on the wire)
    // DC-DEFAULT: DC-C2S-5 (proposed) team from the lobby
    pub fn team_for(entry: Option<&RosterEntry>, pid: u8) -> u8 {
        match entry {
            Some(e) => e.team.min(1) + 1,
            None => pid % 2 + 1,
        }
    }

    /// Spawn point for a team: a random point of the team (team filter = the ROM team name) whose mode filter names
    /// the room's mode (the capital letters of the filter text equal the mode key, the rule of the intro's team bases)
    /// that no living enemy is near; without such a point, the same among all points of the team, else any point
    /// without a team filter, else any point, else the box centre. Returns the position and the Euler angles.
    // OBSERVED: reports/dserver-round2.md findings (a TDM respawn landed on a spawn point of another
    // mode); capture 20261025_002236_tdm4, 20261025_065016_tdm6 (the reference stack's spawn teleports
    // lay on points of the room's mode only: tests/round2_rom.rs)
    // DC-DEFAULT: DSV-59 (proposed) the mode filter first, the earlier rule as the fallback
    // DC-DEFAULT: DC-C2S-12 (proposed) random team spawn point out of enemy sight, else any team spawn point;
    // "out of sight" approximated by the radius of DSV-16 (staged proposal: no visibility test yet)
    pub fn choose_spawn(&mut self, team: u8) -> ([f32; 3], [f32; 3]) {
        self.choose_spawn_keyed(team, None)
    }

    /// [`Game::choose_spawn`]; with `key` (`-spawnpick keyed`, [`spawn_key`]) the point is the keyed pick among the
    /// same candidates, without the enemy filter and without a draw from the match stream.
    // DC-DEFAULT: DSV-121 (staged, arena-bots; renumber at fold if taken)
    pub fn choose_spawn_keyed(&mut self, team: u8, key: Option<u64>) -> ([f32; 3], [f32; 3]) {
        self.choose_spawn_for(team, key, None)
    }

    /// Which spawn points are candidates (index-aligned with `spawns`): the unlocked ones; every point when the
    /// statements locked them all (a world whose scripts we do not follow must still spawn its players).
    // DC-DEFAULT: DSV-569 (proposed)
    fn spawn_open(&self) -> Vec<bool> {
        let open: Vec<bool> = (0..self.spawns.len())
            .map(|i| !self.spawn_locked.get(i).copied().unwrap_or(false))
            .collect();
        if open.iter().any(|o| *o) {
            open
        } else {
            vec![true; self.spawns.len()]
        }
    }

    /// A world statement locks (`true`) or unlocks the spawn points named `name` (ASCII case ignored); the number of
    /// points whose lock changed.
    // OBSERVED: ROM world SANDHOG_CP1 (each checkpoint trigger locks the start group behind the players and unlocks
    // the next with one `msg <point> lock|unlock` statement per point)
    // DC-DEFAULT: DSV-569 (proposed)
    pub fn set_spawn_lock(&mut self, name: &[u8], locked: bool) -> usize {
        let mut changed = 0;
        for (i, n) in self.spawn_names.iter().enumerate() {
            if n.is_empty() || !n.eq_ignore_ascii_case(name) {
                continue;
            }
            if let Some(l) = self.spawn_locked.get_mut(i) {
                if *l != locked {
                    *l = locked;
                    changed += 1;
                }
            }
        }
        changed
    }

    /// Whether each spawn point is locked now (index-aligned with the world's spawn points; DSV-569).
    pub fn spawn_locks(&self) -> &[bool] {
        &self.spawn_locked
    }

    /// The spawn points' locks as the file has them (a new round, DSV-564).
    // DC-DEFAULT: DSV-569 (proposed)
    pub fn reset_spawn_locks(&mut self) {
        self.spawn_locked = self.spawn_locked_file.clone();
    }

    /// Whether a living player other than `who` stands on the spawn point `at`: horizontal distance below
    /// [`GameTuning::spawn_occupied_radius`] and the feet within one stand box height of the point.
    // DC-DEFAULT: DSV-159 (proposed) a point is occupied while a living player stands on it
    fn spawn_occupied(&self, at: [f32; 3], who: Option<u8>) -> bool {
        let r = self.cfg.spawn_occupied_radius;
        self.players.values().any(|p| {
            Some(p.pid) != who
                && p.life == Life::Alive
                && (p.pos[0] - at[0]).hypot(p.pos[2] - at[2]) < r
                && (p.pos[1] - at[1]).abs() < 2.0 * self.cfg.player_dims[1] + r
        })
    }

    /// [`Game::choose_spawn_keyed`] for the player `who` (whose own body never occupies a point): the candidates a
    /// living player stands on are dropped first (DSV-159); when every candidate is occupied, the one farthest from
    /// the nearest living player.
    // DC-DEFAULT: DSV-159 (proposed)
    pub fn choose_spawn_for(
        &mut self,
        team: u8,
        key: Option<u64>,
        who: Option<u8>,
    ) -> ([f32; 3], [f32; 3]) {
        let pick = |want: &dyn Fn(Option<u8>) -> bool,
                    spawns: &[([f32; 3], [f32; 3], Option<u8>)]| {
            spawns
                .iter()
                .filter(|s| want(s.2))
                .map(|s| (s.0, s.1))
                .collect::<Vec<_>>()
        };
        // DC-DEFAULT: DSV-569 (proposed) a locked point is no candidate; every point when all are locked
        let open = self.spawn_open();
        let all: Vec<([f32; 3], [f32; 3], Option<u8>)> = self
            .spawns
            .iter()
            .zip(&open)
            .filter(|(_, o)| **o)
            .map(|(s, _)| *s)
            .collect();
        // DC-DEFAULT: DSV-470 (project decision) classes 12 / 13 on the Search and Destroy set
        let in_mode: Vec<([f32; 3], [f32; 3], Option<u8>)> = self
            .spawns
            .iter()
            .zip(self.spawn_set())
            .zip(&open)
            .filter(|((_, m), o)| *m && **o)
            .map(|((s, _), _)| *s)
            .collect();
        // DC-DEFAULT: DSV-150 (proposed) FFA: any spawn point of the room's mode, else any point (no team filter)
        let ffa = self.is_ffa();
        let mut cands = if ffa {
            pick(&|_| true, &in_mode)
        } else {
            pick(&|t| t == Some(team), &in_mode)
        };
        if cands.is_empty() && !ffa {
            cands = pick(&|t| t == Some(team), &all);
        }
        if cands.is_empty() {
            cands = pick(&|t| t.is_none(), &all);
        }
        if cands.is_empty() {
            cands = pick(&|_| true, &all);
        }
        if cands.is_empty() {
            return (self.centre, [0.0; 3]);
        }
        let free: Vec<([f32; 3], [f32; 3])> = cands
            .iter()
            .copied()
            .filter(|c| !self.spawn_occupied(c.0, who))
            .collect();
        if free.is_empty() {
            // every candidate is stood on: the one with the most room around it
            let gap = |c: &([f32; 3], [f32; 3])| {
                self.players
                    .values()
                    .filter(|p| Some(p.pid) != who && p.life == Life::Alive)
                    .map(|p| (p.pos[0] - c.0[0]).hypot(p.pos[2] - c.0[2]))
                    .fold(f32::INFINITY, f32::min)
            };
            return cands
                .iter()
                .copied()
                .max_by(|a, b| gap(a).total_cmp(&gap(b)))
                .unwrap_or(cands[0]);
        }
        let cands = free;
        if let Some(k) = key {
            let i = (mix64(self.cfg.rng_seed ^ k) % cands.len() as u64) as usize;
            return cands[i];
        }
        let enemies: Vec<[f32; 3]> = self
            .players
            .values()
            .filter(|p| p.team != team && p.life == Life::Alive)
            .map(|p| p.pos)
            .collect();
        let r = self.cfg.spawn_safe_radius;
        let safe: Vec<([f32; 3], [f32; 3])> = cands
            .iter()
            .copied()
            .filter(|c| enemies.iter().all(|e| len3(sub3(c.0, *e)) >= r))
            .collect();
        let from = if safe.is_empty() { &cands } else { &safe };
        let i = (self.next_rand() % from.len() as u64) as usize;
        from[i]
    }

    // ------------------------------------------------------------------------------------------- players

    /// An account was identified by its hello (it gets a `@player` line at the end).
    ///
    /// A later hello of the same account (a rejoin after its link closed, or a new link that replaced the old one,
    /// DSV-2) starts a new stay: the board counters and the `@player` numbers count from 0 again, the team score keeps
    /// what the first stay added, and the co-op lives stay (FT-S9).
    // DC-DEFAULT: DSV-7 (staged proposal) `@player` per identified player
    // OBSERVED: owner playtest 2026-11-07 (a player who was dropped and came back kept his board row)
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.2 (0x6c deletes the record; a new 0x6b carries absolute kills and deaths)
    // DC-DEFAULT: DSV-621 (proposed) a rejoin is a new stay
    pub fn note_identified(&mut self, e: &RosterEntry) {
        let s = self.stats.entry(e.uid).or_default();
        let rejoin = s.entry.is_some();
        if rejoin {
            *s = Stats::default();
        }
        s.entry = Some(e.clone());
        if rejoin {
            self.coop_new_stay(e.uid);
            self.log(format!(
                "uid {}: identified again, a new stay (board counters from 0)",
                e.uid
            ));
        }
    }

    /// A `user` line for `uid` arrived (the first one counts): a player announced before the round started is one
    /// of its first spawn.
    // DC-DEFAULT: DSV-36 (proposed)
    pub fn note_roster(&mut self, uid: u32, now: Millis) {
        self.roster_since.entry(uid).or_insert(now);
    }

    /// The session already sent shell 0x90 [0] / 0x88 [3] at the link (a joiner of a running round).
    pub fn mark_switch_sent(&mut self, pid: u8) {
        if let Some(p) = self.players.get_mut(&pid) {
            p.switch_sent = true;
        }
    }

    /// Whether this player's next spawn is part of the round's first spawn: an account announced before the round
    /// started that has not spawned in it yet (a player without an account: its first spawn). The others joined a
    /// running round (late join or rejoin).
    // SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §2.3, §2.5 (a joiner of the running round got 0x43, 0x44, 0xb5 and no
    // 0x45; REFERENCE, ACCEPTED)
    // DC-DEFAULT: DSV-36 (proposed)
    fn first_spawn_member(&self, pid: u8) -> bool {
        let Some(p) = self.players.get(&pid) else {
            return false;
        };
        match p.uid {
            Some(u) => {
                let announced = self.roster_since.get(&u).copied();
                let started = self.round_started_at;
                let early = match (announced, started) {
                    (Some(a), Some(s)) => !later(a, s),
                    (None, _) | (_, None) => true,
                };
                early && !self.round_spawned.contains(&u)
            }
            None => p.spawns == 0,
        }
    }

    /// The lobby's rewards of an account (`gain <uid> <exp> <gp>`): `GS_PLAYER_SUMMARY` shows them when they
    /// arrive before it.
    // SPEC: DSERVER_PIPE.md §3 (control lines; the `gain` line is a staged DeCombat addition, see the dserver-slice4
    // addendum); DC-DEFAULT: CD-54 (approved) the lobby computes the rewards
    pub fn set_gain(&mut self, uid: u32, gain: (u32, u32, u32, u32)) {
        self.gains.insert(uid, gain);
    }

    /// The game server's address for `GS_GAME_ENDPOINT` (the runner knows it once the socket is bound).
    pub fn set_endpoint(&mut self, host: &[u8], port: u16) {
        self.endpoint = (host.to_vec(), u32::from(port));
    }

    /// Half extents of a player's box: the ROM stand box of its character, else the configured box.
    // DC-DEFAULT: DSV-27 (proposed); DSV-10 (proposed) the configured box
    pub fn dims_of(&self, pid: u8) -> [f32; 3] {
        let rom = self
            .players
            .get(&pid)
            .and_then(|p| p.character.as_ref())
            .and_then(|c| c.assets.rig.stand_extent)
            .filter(|_| self.cfg.player_dims_from_rom);
        rom.unwrap_or(self.cfg.player_dims)
    }

    /// Half extents of the box a shot is tested against: [`Game::dims_of`] while standing, the crouch box while the
    /// player's last stream state says crouched (the `CROUCH` control bit). The crouch box is the extent of the
    /// crouched stand clip of the player's rig, else the standing box with the half height capped at
    /// [`CROUCH_HALF_HEIGHT`]; the centre is the reported position (the client keeps the feet on the floor).
    // DC-DEFAULT: DSV-169 (proposed) hit tests follow the stance, same numbers as SIM-25 (crouch half height 40 vs 50)
    pub fn hit_dims_of(&self, pid: u8) -> [f32; 3] {
        let stand = self.dims_of(pid);
        let Some(p) = self.players.get(&pid) else {
            return stand;
        };
        let crouched = matches!(p.motion, Motion::Crouch | Motion::Step(Gait::Crouch, _));
        if !crouched {
            return stand;
        }
        let rom = p
            .character
            .as_ref()
            .and_then(|c| {
                c.assets
                    .rig
                    .clips
                    .iter()
                    .find(|k| k.key == chars::CLIP_LOWER_CROUCH_STAND)
            })
            .map(|k| k.extent)
            .filter(|_| self.cfg.player_dims_from_rom);
        match rom {
            Some(e) if e[1] < stand[1] => e,
            Some(_) => stand,
            None => [stand[0], stand[1].min(CROUCH_HALF_HEIGHT), stand[2]],
        }
    }

    /// The box [`Game::dims_of`] gives a player with this character.
    pub fn dims_of_look(&self, look: &CharacterLook) -> [f32; 3] {
        look.assets
            .rig
            .stand_extent
            .filter(|_| self.cfg.player_dims_from_rom)
            .unwrap_or(self.cfg.player_dims)
    }

    /// Size in bytes of a ROM resource (0 without a ROM), the `file_info` of the file list.
    // DC-DEFAULT: CD-59 (approved)
    pub fn resource_size(&self, path: &str) -> u32 {
        self.vfs
            .as_ref()
            .and_then(|v| chars::resource_size(v, path))
            .unwrap_or(0)
    }

    /// True when the user's ROM has the resource, or when no ROM is open (tests).
    pub fn has_resource(&self, path: &str) -> bool {
        self.vfs
            .as_ref()
            .is_none_or(|v| chars::resource_size(v, path).is_some())
    }

    /// Puts a rig into the cache (tests and tools without a ROM).
    pub fn insert_rig(&mut self, body: &str, rig: chars::Rig) {
        self.rigs.insert(body, rig);
    }

    /// Replaces the shot-trace geometry (tests with synthetic walls).
    pub fn set_trace_world(&mut self, w: Option<dc_sim::CollisionWorld>) {
        self.trace = w.map(std::sync::Arc::new);
    }

    /// Distance from `from` along the unit direction `dir` to the first solid of the trace world, when one lies
    /// within `len`.
    // SPEC: MOVEMENT_2009.md §9.6 (the sweep of dc-sim, with a zero-size shape a ray; `t` = the free fraction)
    // DC-DEFAULT: DSV-34 (proposed)
    pub fn wall_along(&self, from: [f32; 3], dir: [f32; 3], len: f32) -> Option<f32> {
        self.wall_hit_along(from, dir, len).map(|(d, _)| d)
    }

    /// [`Game::wall_along`] with the kind of the solid: whether the first solid is a sky face (surface flag
    /// [`crate::world::SURF_SKY`]): a shot stops there like at any wall, but draws no decal (MAP-24).
    // DC-DEFAULT: MAP-24
    pub fn wall_hit_along(&self, from: [f32; 3], dir: [f32; 3], len: f32) -> Option<(f32, bool)> {
        if !self.cfg.trace || !(len.is_finite() && len > 0.0) || !finite3(from) {
            return None;
        }
        let w = self.trace.as_ref()?;
        let to = [
            from[0] + dir[0] * len,
            from[1] + dir[1] * len,
            from[2] + dir[2] * len,
        ];
        let h = w.ray(dc_sim::V3::from_array(from), dc_sim::V3::from_array(to))?;
        Some((
            (h.t.clamp(0.0, 1.0)) * len,
            h.surface_flags & crate::world::SURF_SKY != 0,
        ))
    }

    /// The ROM surface id of the world polygon at the end point `at` of a shot along `dir` (unit), else the fallback
    /// surface.
    // DC-DEFAULT: DSV-62 (proposed)
    pub(crate) fn world_surface_at(&self, at: [f32; 3], dir: [f32; 3]) -> u8 {
        self.surfaces
            .as_ref()
            .filter(|_| finite3(at) && finite3(dir))
            .and_then(|p| p.at_shot_end(at, dir))
            .unwrap_or(gs::TODO_SURFACE_WORLD)
    }

    /// The ROM surface id of the world polygon below `at` (a blast point, a bounce), else the fallback surface.
    // DC-DEFAULT: DSV-62 (proposed)
    pub(crate) fn world_surface_below(&self, at: [f32; 3]) -> u8 {
        self.surfaces
            .as_ref()
            .filter(|_| finite3(at))
            .and_then(|p| p.below(at))
            .unwrap_or(gs::TODO_SURFACE_WORLD)
    }

    /// The model other players see in a hand for `weapon` (ROM weapon row, third-person model).
    // OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] HHModel, HHSkin<k>, HHRenderStyle<k>
    pub fn weapon_look(&self, weapon: u16) -> Option<WeaponLook> {
        let w = self.tables.weapons.hand_model(weapon)?;
        Some(WeaponLook {
            weapon,
            model: w.hh_model.clone()?,
            textures: w.hh_skins.clone(),
            styles: w.hh_styles.clone(),
        })
    }

    /// Resolves a player's character from the ROM tables: the rows named by the `user` line (`model=`, `skin=`,
    /// `face=`) or the operator default, the body model file's rig, the weapon model of the first loadout weapon.
    /// `None` (a plain object as in slice 2) when the ROM has no character tables or the body does not resolve;
    /// a face that does not resolve is left out.
    // DC-DEFAULT: DSV-24 (proposed)
    pub fn character_for(
        &mut self,
        roster: Option<&RosterEntry>,
    ) -> Result<Option<CharacterLook>, String> {
        if self.tables.chars.is_empty() {
            return Ok(None);
        }
        let d = self.cfg.character;
        let (m, s, f) = roster
            .map(|e| e.character_rows())
            .unwrap_or((None, None, None));
        let face_row = f.unwrap_or(d.face_row);
        // DC-DEFAULT: DSV-271 (proposed) the equipment's body row and skin when the line names none
        let (lm, ls) = roster
            .and_then(|e| e.loadout_items())
            .map(|l| chars::loadout_look(&self.tables.chars, &l, face_row))
            .unwrap_or((None, None));
        let choice = CharacterChoice {
            model_row: m.or(lm).unwrap_or(d.model_row),
            skin: s.or(ls).unwrap_or(d.skin),
            face_row,
        };
        let fallback = CharacterChoice {
            model_row: m.unwrap_or(d.model_row),
            skin: s.unwrap_or(d.skin),
            face_row,
        };
        let assets = self.assets_for(choice, Some(fallback))?;
        let weapon = self
            .loadout_weapons(roster)
            .first()
            .and_then(|w| self.weapon_look(w.1));
        let anims = AnimSet::from_rig(&assets.rig);
        Ok(Some(CharacterLook {
            assets,
            weapon,
            anims,
        }))
    }

    /// The assets of the character rows `choice` (body, skins, face, rig): a missing body resource is an error,
    /// missing styles are left out, a face with a missing resource is dropped. A `choice` whose body does not resolve
    /// falls back to `fallback` when given.
    pub(crate) fn assets_for(
        &mut self,
        choice: CharacterChoice,
        fallback: Option<CharacterChoice>,
    ) -> Result<chars::CharacterAssets, String> {
        let vfs = self.vfs.as_ref();
        let rigs = &mut self.rigs;
        let mut rig_of = |body: &str| match vfs {
            Some(v) => rigs.rig(v, body),
            None => rigs
                .cached(body)
                .ok_or_else(|| "no ROM for the body model".to_string()),
        };
        // an equipment body that does not resolve falls back to the line's / the operator's choice
        let mut assets = match chars::character_assets(&self.tables.chars, choice, &mut rig_of) {
            Ok(a) => a,
            Err(_) if fallback.is_some_and(|f| f != choice) => chars::character_assets(
                &self.tables.chars,
                fallback.expect("checked"),
                &mut rig_of,
            )?,
            Err(e) => return Err(e),
        };
        if let Some(v) = vfs {
            let missing = |p: &String| chars::resource_size(v, p).is_none();
            if let Some(p) = assets
                .body
                .iter()
                .chain(assets.body_textures.iter())
                .find(|p| missing(p))
            {
                return Err(format!(
                    "body resource {} not in the ROM",
                    chars::short_hash(p)
                ));
            }
            assets.body_styles.retain(|p| !missing(p));
            let face_missing = assets.face.as_ref().is_some_and(|(m, sk, st)| {
                std::iter::once(m)
                    .chain(sk.iter())
                    .chain(st.iter())
                    .any(missing)
            });
            if face_missing {
                assets.face = None;
            }
        }
        Ok(assets)
    }

    /// Reads the default character's body model once (the large shared child model takes a moment), so the first
    /// player's placement does not wait for it. Returns a log line.
    pub fn preload_character(&mut self) -> Option<String> {
        if self.tables.chars.is_empty() {
            return None;
        }
        Some(match self.character_for(None) {
            Ok(Some(c)) => format!(
                "default character: {} model file(s), {} clips, stand box {:?}, weapon model {}",
                c.assets.body.len() + usize::from(c.assets.face.is_some()),
                c.assets.rig.clips.len(),
                c.assets.rig.stand_extent,
                if c.weapon.is_some() { "yes" } else { "no" }
            ),
            Ok(None) => "default character: none".into(),
            Err(e) => format!("default character: {e} (players will be plain objects)"),
        })
    }

    /// A player's object exists on every in-world client: ask for its player info (A2).
    // SPEC: GAME_MESSAGES_2009_C2S.md Appendix A step A2; GAME_MESSAGES_2009_S2C.md §10 step 2
    #[allow(clippy::too_many_arguments)]
    pub fn add_player(
        &mut self,
        pid: u8,
        roster: Option<&RosterEntry>,
        hello: Option<&Hello>,
        obj: u16,
        pos: [f32; 3],
        rot: [f32; 3],
        now: Millis,
    ) {
        let character = match self.character_for(roster) {
            Ok(c) => c,
            Err(e) => {
                self.log(format!(
                    "client {pid}: no character model ({e}); a plain object instead"
                ));
                None
            }
        };
        self.prepare_player(pid, roster, hello, obj, pos, rot, character, now);
        self.ask_identity(pid);
    }

    /// The identify request (A2); in the reference stack order also the game endpoint and the match settings, all before
    /// the view object (O2).
    // SPEC: GAME_MESSAGES_2009_S2C.md §10 steps 1, 2, 4; UDP_REFERENCE_FINDINGS_botclient.md O2 (0x72, 0x9e, 0x7d
    // before id 7; REFERENCE)
    pub fn ask_identity(&mut self, pid: u8) {
        if !self.players.contains_key(&pid) {
            return;
        }
        self.send(pid, gs::identify_request());
        self.out
            .push(GameOut::Mark(pid, Milestone::IdentifyRequest));
        if self.cfg.start_order == StartOrder::Reference {
            let (host, port) = self.endpoint.clone();
            self.send(pid, gs::game_endpoint(self.rules.mode_class, &host, port));
            // SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §2.2 (0x72, 0x9e, 0x9d, 0x7d; the real client accepted it);
            // SPEC: UNPROVEN meaning of 0x9d
            // DC-DEFAULT: DSV-36 (proposed) the reference stack's `[0]`, `-no9d` omits it
            if self.cfg.send_9d {
                self.send(pid, gs::shell_9d(0));
            }
            self.send_match_settings(pid);
        }
    }

    fn send_match_settings(&mut self, pid: u8) {
        let Some(team) = self.players.get(&pid).map(|p| p.team) else {
            return;
        };
        let settings = gs::MatchSettings {
            caption: Vec::new(),
            own_team: team,
            goal: self.rules.goal,
            scores: self.team_counters(),
            // DC-DEFAULT: GMD-23 (staged) the round limit and the current round in a round mode (1 / 0 otherwise)
            round_limit: self.round_limit(),
            round_no: if self.round_loop() { self.round_no } else { 0 },
            shot_seed: self.cfg.shot_seed,
            // DC-DEFAULT: DSV-191 (proposed) a late joiner learns the carriers
            carriers: if self.is_flag() {
                self.flag_carriers()
            } else {
                Vec::new()
            },
        };
        self.send(pid, gs::match_settings(&settings, self.rules.mode_class));
        // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1.5 (the client's counter := own_value)
        if let Some(p) = self.players.get_mut(&pid) {
            p.cone.set_seed(self.cfg.shot_seed);
        }
    }

    /// Creates the player record (no message yet).
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_player(
        &mut self,
        pid: u8,
        roster: Option<&RosterEntry>,
        hello: Option<&Hello>,
        obj: u16,
        pos: [f32; 3],
        rot: [f32; 3],
        character: Option<CharacterLook>,
        now: Millis,
    ) {
        // DC-DEFAULT: DSV-148 (proposed) a new player's FFA count starts at 0 (a client id may be reused)
        self.modes.ffa_kills.remove(&pid);
        self.modes.ffa_deaths.remove(&pid);
        // DC-DEFAULT: DSV-621 (proposed) the other board counters kept by client id start at 0 too (a rejoin or
        // another player may get the id of one who left)
        self.ctf.scored.remove(&pid);
        self.spy.points.remove(&pid);
        self.spy.kills.remove(&pid);
        self.spy.deaths.remove(&pid);
        self.infect.infections.remove(&pid);
        let name16 = hello
            .map(|h| h.name16.clone())
            .filter(|n| !n.is_empty())
            .or_else(|| roster.map(|e| e.nick.encode_utf16().collect()))
            .unwrap_or_default();
        let name8 = hello
            .map(|h| h.name8.clone())
            .filter(|n| !n.is_empty())
            .or_else(|| {
                roster.map(|e| {
                    e.nick
                        .chars()
                        .map(|c| if (c as u32) < 0x100 { c as u8 } else { b'?' })
                        .collect()
                })
            })
            .unwrap_or_default();
        let p = Player {
            pid,
            uid: roster.map(|e| e.uid),
            roster: roster.cloned(),
            name8,
            name16,
            team: self.team_in_room(roster, pid),
            obj,
            look: Look::default(),
            info: None,
            introduced: false,
            intro_deadline: now.wrapping_add(self.cfg.identify_wait_ms),
            ready: false,
            life: Life::Joining,
            health: self.cfg.start_health,
            armor: 0.0,
            armor_absorb: Absorb::Flat(0.0),
            pos,
            vel: [0.0; 3],
            yaw: rot[1],
            move_seq: 0,
            last_move: None,
            updates: 0,
            stale_updates: 0,
            corrections: 0,
            weapons: Vec::new(),
            current: None,
            kill_chain: None,
            killed_by: None,
            kills_this_life: 0,
            client_ping_ms: None,
            link_rtt_ms: None,
            spawns: 0,
            switch_sent: false,
            stream_seen: BTreeMap::new(),
            hand_weapon: character
                .as_ref()
                .and_then(|c| c.weapon.as_ref().map(|w| w.weapon)),
            character,
            motion: Motion::Stand,
            motion_since: now,
            spawned_at: now,
            death_clip: None,
            visible: false,
            sprinting: None,
            scanning: false,
            scan_answered: None,
            last_throw: None,
            view_repair_at: None,
            aim_sent: None,
            vitals_sent: None,
            view_repairs: 0,
            track: hits::Track::default(),
            fall: None,
            frozen: false,
            freeze_due: None,
            flight: None,
            stand_pos: None,
            on_ground_report: true,
            cone: crate::cone::LinkCone::default(),
            pose: crate::anims::Pose::default(),
        };
        self.players.insert(pid, p);
        if self.coop.is_some() {
            self.coop_on_player(pid);
        }
    }

    /// The player's link closed: the others get `GS_PLAYER_LEFT`.
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.2 (0x6c); GAME_MESSAGES_2009_C2S.md Appendix A step A14
    pub fn remove_player(&mut self, pid: u8, kicked: bool) {
        let Some(p) = self.players.remove(&pid) else {
            return;
        };
        if p.introduced {
            self.broadcast(&gs::player_left(pid, kicked));
        }
        // DC-DEFAULT: HS-3 (amendment, proposed) a reservation ends when its player leaves
        self.hisec_release_owner(pid);
        // DC-DEFAULT: DSV-612 (proposed) the mines of a player who left go with him
        self.drop_mines_of(pid);
        // DC-DEFAULT: DSV-651 (proposed) his ping cooldown and the spots on him go with him
        self.team_ping_forget(pid);
        for q in self.players.values_mut() {
            q.stream_seen.remove(&p.obj);
        }
    }

    /// The transport's round-trip estimate (feeds the ping table when the client reports none).
    pub fn set_link_rtt(&mut self, pid: u8, rtt_ms: u16) {
        if let Some(p) = self.players.get_mut(&pid) {
            p.link_rtt_ms = Some(rtt_ms);
        }
    }

    fn info_of(&self, p: &Player, kind: u8) -> PlayerInfo {
        let dead = !matches!(p.life, Life::Alive | Life::Joining | Life::Loading);
        self.info_with(p, kind, dead)
    }

    fn info_with(&self, p: &Player, kind: u8, dead: bool) -> PlayerInfo {
        let st = p.uid.and_then(|u| self.stats.get(&u));
        // DC-DEFAULT: DSV-148 (proposed) a FFA player without a uid (no statistics row): his foe kills and deaths
        // by foe kills of this match, so the per-kill refresh never writes 0 over the client's own count
        let (kills, deaths) = match st {
            // DC-DEFAULT: DSV-481 (proposed) Gun Game: the kill column shows the rung
            Some(s) if self.is_gun_game() => (self.ffa_kills_of(p.pid), s.deaths),
            Some(s) => (s.kills, s.deaths),
            None if self.is_ffa() => (
                self.ffa_kills_of(p.pid),
                self.modes.ffa_deaths.get(&p.pid).copied().unwrap_or(0),
            ),
            None => (0, 0),
        };
        // DC-DEFAULT: DSV-322 (proposed) Spy Hunt: kills in the kill column, the points in the mission column
        let (kills, deaths, mission) = if self.is_spy() {
            self.spy_board(p.pid)
        } else {
            (kills, deaths, self.flag_mission_of(p.pid))
        };
        PlayerInfo {
            name: p.name8.clone(),
            pid: p.pid,
            account_no: p.uid.unwrap_or(0),
            character: Some(p.obj),
            look: p.look.clone(),
            kills: kills.min(i16::MAX as u32) as i16,
            deaths: deaths.min(255) as u8,
            team: p.team,
            dead,
            kind,
            // SPEC: FIRETEAM_2009.md §2.1 (0x6b m1 = record +0x1f8, the class-5 lives; before any 0x67 state 2)
            // DC-DEFAULT: FT-L1 (proposed)
            mode_m1: self.coop_lives_of(p.pid).min(255) as u8,
            mode_m2: self.coop_points_of(p.pid),
            mission,
            // DC-DEFAULT: DSV-141 (proposed) rank and clan from the lobby's user line
            mark: gs::PlayerMark::of_roster(p.roster.as_ref()),
        }
    }

    /// Roster both ways, match settings, then player state 3 (S12: the loading screen can close). In the reference stack
    /// order the settings went out with the identify request and the player spawns at once (state 1), then the
    /// round messages follow.
    // SPEC: GAME_MESSAGES_2009_C2S.md Appendix A steps A3, A4; GAME_MESSAGES_2009_S2C.md §10 steps 4, 5, 10
    // DC-DEFAULT: DC-C2S-10' (proposed, GAME_MESSAGES_2009_C2S.md §12.3: state 3 first stays allowed) for the spec
    // order; DSV-22 (proposed) reference order
    // SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §2.5 (roster kinds: the joiner gets kind 2 for itself, `dead` 1, and
    // kind 2 for every other player; every other client gets the joiner's kind 0, `dead` 1, then kind 2, `dead` 0 after
    // the spawn set; REFERENCE, ACCEPTED)
    // DC-DEFAULT: CV2-7 (proposed) roster kinds
    fn introduce(&mut self, pid: u8, now: Millis) {
        let Some(me) = self.players.get(&pid).cloned() else {
            return;
        };
        if me.introduced {
            return;
        }
        // own record first (not yet spawned), then everyone already introduced, all kind 2, to the newcomer
        let mut list: Vec<PlayerInfo> = vec![self.info_with(&me, gs::INFO_UPDATE, true)];
        list.extend(
            self.players
                .values()
                .filter(|p| p.introduced)
                .map(|p| self.info_of(p, gs::INFO_UPDATE)),
        );
        for i in &list {
            self.send(pid, gs::player_info(i));
        }
        // the newcomer to everyone else: kind 0 (the join notice), not yet spawned
        let mine = gs::player_info(&self.info_with(&me, gs::INFO_NEW, true));
        self.broadcast(&mine);
        if self.cfg.start_order == StartOrder::Reference {
            if let Some(p) = self.players.get_mut(&pid) {
                p.introduced = true;
                p.life = Life::Loading;
            }
            self.spawn_notes_catch_up(pid);
            self.out.push(GameOut::Mark(pid, Milestone::Introduced));
            self.start_round(now);
            let member = self.first_spawn_member(pid);
            self.spawn(pid, now);
            self.send_round_start(pid, now, member);
            // DC-DEFAULT: DSV-511 (proposed) Quarantine: the running outbreak countdown to a later player
            self.infect_countdown_to(pid, now);
            // DC-DEFAULT: HS-3 (amendment, proposed) the cases' reservation state to a client that joins later
            self.hisec_introduce(pid);
            return;
        }
        self.send_match_settings(pid);
        self.send(pid, gs::player_state(gs::STATE_WAITING));
        if let Some(p) = self.players.get_mut(&pid) {
            p.introduced = true;
            p.life = Life::Loading;
        }
        self.spawn_notes_catch_up(pid);
        self.out.push(GameOut::Mark(pid, Milestone::Introduced));
        self.out.push(GameOut::Mark(pid, Milestone::WaitingState));
        self.hisec_introduce(pid);
    }

    /// The round clock starts with the first player who is ready (spec order: `UP_WORLD_READY`) or spawned
    /// (reference order: the player info).
    // DC-DEFAULT: DSV-20 (proposed); DSV-22 (proposed) reference order
    fn start_round(&mut self, now: Millis) {
        if self.phase == RoundPhase::Waiting {
            self.phase = RoundPhase::Running { start: now };
            self.round_started_at = Some(now);
            // DC-DEFAULT: GMD-20 (staged) the first round of the match
            self.round_no = 1;
            self.round_alive_seen = [false; 2];
            self.next_ping = Some(now);
            let line = format!(
                "round started (mode class {}, goal {}, {} s)",
                self.rules.mode_class, self.rules.goal, self.rules.round_secs
            );
            self.log(line);
            self.coop_round_started(now);
            // DC-DEFAULT: DSV-512 (proposed) a bomb match opens with the warm-up
            self.warmup_begin(now);
        }
    }

    fn round_secs_left(&self, now: Millis) -> u32 {
        // DC-DEFAULT: DSV-512 (proposed) the clock shows the warm-up's time left
        if let Some(left) = self.warmup_secs_left(now) {
            return left;
        }
        match self.phase {
            RoundPhase::Running { start } if self.rules.round_secs != 0 => {
                let gone = since(now, start) / 1000;
                self.rules.round_secs.saturating_sub(gone).max(1)
            }
            _ => 0,
        }
    }

    /// Reference order, after the spawn set: scores clear, round begin (remaining seconds), the team kills, match begin
    /// only for the round's first spawn, then the statistics enable of slice 2.
    // SPEC: UDP_REFERENCE_FINDINGS_botclient.md O2 (0x43, 0x44, 0xb5, 0x45 in this order; REFERENCE); GAME_MESSAGES_2009_S2C.md
    // §2.1 (0x43, 0x44 layouts), §10 steps 11-13; CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §2.3 (a joiner of a running
    // round: 0x43, 0x44, 0xb5, no 0x45)
    // DC-DEFAULT: DSV-36 (proposed)
    fn send_round_start(&mut self, pid: u8, now: Millis, member: bool) {
        let left = self.round_secs_left(now);
        self.send(pid, gs::scores_clear());
        // DC-DEFAULT: DSV-22 (proposed) one round of a single-round match: round_no 1, round_limit 1; GMD-23 (staged)
        // the round number and the goal in a round mode
        let (no, limit) = (self.round_no.max(1), self.round_begin_limit());
        self.send(pid, gs::round_begin(left, no, limit, false));
        // DC-DEFAULT: DC-KILL-3 (proposed) 0xb5 carries the team kills ([0][0] at the start); GMD-21 (staged) the round
        // wins in a round mode
        self.send(pid, gs::round_wins(self.team_counters()));
        if member {
            let intro = self.cfg.show_intro.then(|| self.intro_block());
            let mb = gs::MatchBegin {
                round_secs: left,
                // DC-DEFAULT: DSV-20 (revision, proposed) the waiting banner off in a running round
                // DC-DEFAULT: DSV-512 (proposed) the training banner during the warm-up
                waiting_banner: self.cfg.waiting_banner || self.in_warmup(),
                countdown_secs: 0,
                intro,
                round_no: 0,
            };
            self.send(pid, gs::match_begin(&mb, self.rules.mode_class));
        }
        if self.cfg.perf_enable {
            self.send(
                pid,
                gs::perf_enable(TODO_PERF_ENABLE_PID, true, PERF_PING_MS, PERF_STATS_MS),
            );
        }
        // SPEC: FIRETEAM_2009.md §2.2 row 4 (0xd7 sub 4: the round timer shown running)
        // DC-DEFAULT: FT-S7 (proposed) the co-op round timer at the round start
        if self.coop.is_some() && left > 0 {
            self.send(pid, gs::coop_round_clock(true, left));
        }
        // DC-DEFAULT: DSV-584 (proposed) the running mission's goal list or round, and its music, to this player
        if self.coop.is_some() {
            self.coop_catch_up(pid, now);
        }
    }

    // ------------------------------------------------------------------------------------------- client messages

    /// A parsed client game message.
    pub fn on_up(&mut self, pid: u8, up: Up, now: Millis) {
        if !self.players.contains_key(&pid) {
            return;
        }
        match up {
            Up::PlayerInfo(info) => self.on_player_info(pid, *info, now),
            Up::ClientOptions { .. } => {}
            Up::WorldReady => self.on_world_ready(pid, now),
            Up::Fire(f) => {
                // DC-DEFAULT: DSV-483 (proposed) no firing during a Possession move
                if self.in_flight(pid) {
                    return;
                }
                // DC-DEFAULT: DSV-477 (proposed) a refused shot ends the window too
                self.end_spawn_note_of(pid, now);
                if self.refuse_barred(pid, f.weapon_id, "shot") {
                    if f.hit.and_then(|h| h.1).is_some() {
                        self.hit_counts.add(hits::HitVerdict::WeaponLimit);
                    }
                    return;
                }
                self.pose_on_fire(pid, now);
                self.on_fire(pid, &f, now);
                // DC-DEFAULT: DSV-481 (proposed)
                self.gun_game_refill(pid, f.weapon_id);
            }
            Up::Reload { ammo_id } => {
                self.pose_action(pid, crate::anims::Act::Reload, now);
                self.on_reload(pid, ammo_id)
            }
            Up::WeaponChange { weapon_id, .. } => {
                if self.refuse_barred(pid, weapon_id, "weapon change") {
                    return;
                }
                let i = self.carried_slot(pid, weapon_id, None);
                if let Some(p) = self.players.get_mut(&pid) {
                    p.current = Some(i);
                }
                self.hand_weapon_changed(pid, weapon_id);
                self.pose_weapon_changed(pid, weapon_id, now);
                // DSV-615: a sensor drawn after the switch arrived takes it now (and a weapon change ends it)
                self.scan_sync(pid);
            }
            Up::RespawnRequest { id } => self.on_respawn_request(pid, id, now),
            Up::Chat { text, scope, .. } => self.on_chat(pid, &text, scope),
            Up::RosterQuery { pid: q } => self.on_roster_query(pid, q),
            Up::Perf(p) => self.on_perf(pid, p),
            Up::Sprint { sprinting, .. } => self.on_sprint(pid, sprinting),
            Up::Scan { on, .. } => self.on_scan(pid, on),
            Up::WeaponSound {
                sound,
                weapon_id,
                looped: false,
                pos: Some(pos),
                ..
            } => self.relay_sound(pid, sound, weapon_id, pos),
            Up::Event {
                category: EVENT_CATEGORY_MOVE,
                event: EVENT_RELAYED_JUMP,
            } => {
                let pos = self.players.get(&pid).map(|p| p.pos);
                if let Some(pos) = pos {
                    self.relay_sound(pid, gs::FX_SOUND_JUMP, 0, pos);
                }
                // DC-DEFAULT: DSV-126 (proposed) the jump clip from the jump event
                if let Some(p) = self.players.get_mut(&pid) {
                    p.pose.jumped(now);
                }
            }
            Up::ThrowHold => {
                if let Some(p) = self.players.get_mut(&pid) {
                    p.last_throw = Some(now);
                }
                self.pose_action(pid, crate::anims::Act::ThrowHold, now);
            }
            // DC-DEFAULT: DSV-202 (proposed) hold to plant / defuse (classes 2 / 3)
            Up::UseHold { phase, target } => self.on_use_hold(pid, phase, target, now),
            Up::UseProgressEnd { target, remaining } => {
                self.on_use_progress_end(pid, target, remaining, now)
            }
            Up::LeaveMatch
            | Up::Use { .. }
            | Up::Event { .. }
            | Up::ObserveTarget { .. }
            | Up::ObserveFree
            | Up::WeaponSound { .. }
            | Up::Developer
            // DSV-651: the session checks the sight and calls `team_ping`
            | Up::TeamPing(_)
            | Up::Unknown => {}
        }
    }

    /// `UP_SPRINT_STATE`: a change of the `sprinting` bit is answered with `GS_SPRINT_ALLOWED` carrying it (the
    /// client reaches sprint speed only once the server allowed it).
    // SPEC: CAPTURE_tdm1_GAME_VALIDATION.md §3.11, §6 (new row: always allow; PROVEN-CAPTURE by the client's reaction)
    // DC-DEFAULT: CV2-4 (proposed) sprint acknowledgement
    /// `UP_SCAN` sub 1: the heartbeat sensor of `pid` switched on or off (a player not in the match is ignored). The
    /// switch counts while he is alive with a heartbeat-sensor row drawn ([`Game::scan_reach`]); the verdict is
    /// answered to the sender with the radar switch whenever it changes ([`Game::scan_sync`]): "off" for a refused
    /// "on" (a 2009 client needs the "off" to clear its marks; a DeCombat client stops its scan, CLI-1912), "on" when
    /// a later draw of the sensor makes a waiting switch count. A repeat of the same switch gets no answer.
    // DC-DEFAULT: DSV-615 (proposed) the switch counts with the drawn sensor row only; the answer goes to the sender
    // on a change of the verdict (2009 server's rule not in the allowed sources)
    fn on_scan(&mut self, pid: u8, on: bool) {
        let Some(p) = self.players.get_mut(&pid).filter(|p| p.introduced) else {
            return;
        };
        p.scanning = on;
        self.scan_sync(pid);
    }

    /// Answers `pid`'s radar switch when its verdict differs from the last answer (DSV-615).
    fn scan_sync(&mut self, pid: u8) {
        let verdict = self.scan_reach(pid).is_some();
        let weapon = self.drawn_weapon(pid).unwrap_or(0);
        let Some(p) = self.players.get_mut(&pid) else {
            return;
        };
        // nothing to answer before the first switch since the spawn
        if p.scan_answered.is_none() && !p.scanning {
            return;
        }
        if p.scan_answered == Some(verdict) {
            return;
        }
        p.scan_answered = Some(verdict);
        self.send(pid, gs::radar_switch(verdict, weapon));
    }

    /// Round end (and intermission): every heartbeat sensor is switched off; a client whose scan counted is told.
    // DC-DEFAULT: DSV-615 (proposed)
    pub(super) fn scan_stop_all(&mut self) {
        let pids: Vec<u8> = self.players.keys().copied().collect();
        for pid in pids {
            if let Some(p) = self.players.get_mut(&pid) {
                p.scanning = false;
            }
            self.scan_sync(pid);
        }
    }

    /// The weapon `pid` holds now (the slot of the last `UP_WEAPON_CHANGE` or the spawn's draw).
    fn drawn_weapon(&self, pid: u8) -> Option<u16> {
        self.players
            .get(&pid)
            .and_then(|p| p.current.and_then(|i| p.weapons.get(i)).map(|w| w.weapon))
    }

    /// While `pid`'s heartbeat sensor is on, alive and drawn: its row's scan distance (world units, horizontal), the
    /// reach within which the visibility layer shows him every enemy (DSV-616).
    // DC-DEFAULT: DSV-616 (proposed) the reach is the row's `ScanDist` in world units (the client compares it with the
    // radar distance, world distance x the radar's character scale, which is at least the world distance in the ROM)
    pub fn scan_reach(&self, pid: u8) -> Option<f32> {
        let p = self.players.get(&pid)?;
        if !p.scanning || !p.introduced || p.life != Life::Alive {
            return None;
        }
        let w = self.drawn_weapon(pid)?;
        self.tables
            .weapons
            .weapons
            .get(&self.tables.weapons.base_of(w))
            .and_then(|r| r.scan_dist)
    }

    fn on_sprint(&mut self, pid: u8, sprinting: bool) {
        let Some(p) = self.players.get_mut(&pid) else {
            return;
        };
        if p.sprinting == Some(sprinting) {
            return;
        }
        p.sprinting = Some(sprinting);
        self.send(pid, gs::sprint_allowed(sprinting));
    }

    /// A weapon sound (0x80) or a jump of `pid` to every introduced player, the sender included, unreliable:
    /// effect type 0x19 with the player id and the position.
    // SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.5 (U5 decided; REFERENCE, ACCEPTED); GAME_MESSAGES_2009_S2C.md §8.2
    // DC-DEFAULT: CV2-3 (proposed) relays
    fn relay_sound(&mut self, pid: u8, k: u8, value: u16, pos: [f32; 3]) {
        if k == gs::FX_SOUND_LONG_FORM {
            return;
        }
        let m = gs::sound_effect(k, value, u32::from(pid), pos);
        for to in self.introduced() {
            self.out.push(GameOut::Unreliable(to, m.clone()));
        }
    }

    /// The shot effect of one `UP_FIRE` to every introduced player, the shooter included, unreliable (not for the
    /// `UP_FIRE` that releases a throw: a thrown object has no shot effect, [`Game::throw_release`]). The end point is the client's
    /// reported hit point; a reported object hit with a wall of the trace world more than the slack before it ends
    /// at the wall instead (and the hit is refused: the returned distance); a miss ends at the first wall, else a
    /// fixed distance along the shot.
    // SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §3.3 (type 0x06 after each bullet; REFERENCE, ACCEPTED);
    // GAME_MESSAGES_2009_C2S.md §4.1 (hit point = origin + hit_dist * dir); REFERENCE_FINDINGS_botduel.md F1 (through a
    // wall the reference stack sent only the world record, ending at the wall)
    // DC-DEFAULT: CV2-3 (proposed); DSV-29 (proposed) miss distance and normal; DSV-34 (proposed) the trace
    /// The shot record's mods of `pid` firing `weapon`: the held weapon's granted parts by socket (the current weapon
    /// when it is that weapon, else the first carried one), [`gs::NO_SHOT_MODS`] for a weapon the player does not hold.
    // DC-DEFAULT: DSV-167 (proposed)
    fn shot_mods_of(&self, pid: u8, weapon: u16) -> [u8; 3] {
        let Some(p) = self.players.get(&pid) else {
            return gs::NO_SHOT_MODS;
        };
        let held = p
            .current
            .and_then(|i| p.weapons.get(i))
            .filter(|w| w.weapon == weapon)
            .or_else(|| p.weapons.iter().find(|w| w.weapon == weapon));
        held.map_or(gs::NO_SHOT_MODS, |w| {
            self.tables.weapons.shot_mods(&w.parts)
        })
    }

    /// The damage of one hit of `pid`'s shot `f` before the zone factor: the row's ([`rom_tables::WeaponTable::
    /// shot_damage`], else `default_damage`) times the mod factor of the parts the server granted the fired weapon
    /// (DSV-440; 1 with `combat.mod_damage=off`, for a weapon the player was never granted, or a mod-less grant).
    // DC-DEFAULT: DSV-440 (proposed)
    pub(crate) fn hit_damage(&self, pid: u8, f: &crate::up::UpFire) -> f32 {
        let base = self
            .tables
            .weapons
            .shot_damage(f.weapon_id, f.ammo_id, f.mode1)
            .unwrap_or(self.cfg.default_damage);
        if !self.cfg.mod_damage {
            return base;
        }
        let parts = self.players.get(&pid).and_then(|p| {
            p.current
                .and_then(|i| p.weapons.get(i))
                .filter(|w| w.weapon == f.weapon_id)
                .or_else(|| p.weapons.iter().find(|w| w.weapon == f.weapon_id))
                .map(|w| w.parts)
        });
        parts.map_or(base, |pp| base * self.tables.weapons.mod_damage_factor(&pp))
    }

    pub(crate) fn shot_effect(&mut self, pid: u8, f: &crate::up::UpFire) -> Option<f32> {
        let p = self.players.get(&pid)?;
        let shooter = p.obj;
        let dl = len3(f.dir);
        if !(dl.is_finite() && dl > 0.0) {
            return None;
        }
        let dir = [f.dir[0] / dl, f.dir[1] / dl, f.dir[2] / dl];
        let along = |d: f32| {
            [
                f.origin[0] + d * dir[0],
                f.origin[1] + d * dir[1],
                f.origin[2] + d * dir[2],
            ]
        };
        let mut blocked = None;
        // a claim refused by its victim is drawn as the miss it is: no record names the character
        // DC-DEFAULT: DSV-101 (proposed)
        // DC-DEFAULT: DSV-102 (proposed) a pellet the cone check refuses is drawn as a miss too
        let refused = self.cone_refused
            || (self.cfg.refused_hits_miss
                && matches!(f.hit, Some((_, Some(o))) if self.target_refusal(pid, o).is_some()));
        // DC-DEFAULT: MAP-24 a shot whose first solid is a sky face (surface flag 0x10) stops there but answers the
        // no-hit record (`first` none) with the sky surface id (`dc_world::SURFACE_SKY`, the client's sky guard):
        // no decal, impact or sound lands on the sky, whatever `texflags` the face carries
        let wall_first = |w: f32, sky: bool| ((!sky).then_some(0), along(w), sky);
        let (first, to, sky_stop) = match (f.hit, f.shot_end()) {
            (Some((dist, Some(obj))), Some(end)) if !refused => {
                match self
                    .wall_hit_along(f.origin, dir, dist)
                    .filter(|(w, _)| *w + self.cfg.trace_slack < dist)
                {
                    Some((w, sky)) => {
                        blocked = Some(w);
                        wall_first(w, sky)
                    }
                    None => (Some(obj), end, false),
                }
            }
            (Some((_, None)), Some(end)) if !refused => (Some(0), end, false),
            // DC-DEFAULT: DSV-122 (proposed) a living breakable on the ray ends it (the surface is its polygon's)
            _ => match self
                .cfg
                .destruct
                .enabled
                .then(|| self.breakable_on_ray(f.origin, dir, TODO_MISS_TRACE_UNITS))
                .flatten()
            {
                Some((_, d)) => (Some(0), along(d), false),
                None => match self.wall_hit_along(f.origin, dir, TODO_MISS_TRACE_UNITS) {
                    Some((w, sky)) => wall_first(w, sky),
                    None => (None, along(TODO_MISS_TRACE_UNITS), false),
                },
            },
        };
        let surface = if sky_stop {
            gs::SURFACE_SKY
        } else {
            self.shot_surface(pid, first, to, dir, f)
        };
        // a hit on a character: the hit mask, then the continuation record to the wall behind the victim
        // DC-DEFAULT: DSV-86 (proposed)
        let hit_obj = matches!(first, Some(o) if o != 0);
        let mods = self.shot_mods_of(pid, f.weapon_id);
        let shot = gs::ShotEffect {
            first,
            shooter,
            weapon: f.weapon_id,
            ammo: f.ammo_id,
            from: f.weapon_pos,
            to,
            normal: [-dir[0], -dir[1], -dir[2]],
            surface,
            mask: if hit_obj {
                self.cfg.hit_mask
            } else {
                gs::SHOT_MASK_MISS
            },
            // DC-DEFAULT: DSV-85 (proposed) `c` = the shooter's client id
            client: pid,
            mods,
        };
        let mut msgs = vec![gs::shot_effect(&shot)];
        if hit_obj && self.cfg.hit_continuation {
            if let Some((end, normal)) = self.shot_exit(f.origin, dir) {
                msgs.push(gs::shot_effect(&gs::ShotEffect {
                    first: Some(0),
                    to: end,
                    normal,
                    surface: self.shot_surface(pid, Some(0), end, dir, f),
                    mask: gs::SHOT_MASK_EXIT,
                    ..shot
                }));
            }
        }
        for to in self.introduced() {
            for m in &msgs {
                self.out.push(GameOut::Unreliable(to, m.clone()));
            }
        }
        blocked
    }

    /// The end of a shot behind what it hit: the first wall of the trace world along `dir` from `from` within the
    /// miss distance, and that wall's normal.
    // OBSERVED: capture 20261025_002236_tdm4 c6 seq 2968 (the continuation's `to` lies on the
    // shot line 1.063 x as far as the hit point, its normal is a wall normal (0.0, 0.717, -0.697), `from` is the hit
    // record's; REFERENCE)
    // SPEC: MOVEMENT_2009.md §9.6 (the dc-sim sweep as a ray)
    // DC-DEFAULT: DSV-86 (proposed) no wall within the miss distance = no continuation; DSV-34 (proposed) the trace;
    // MAP-24 a sky face behind the victim is no wall for the continuation (no decal on the sky)
    fn shot_exit(&self, from: [f32; 3], dir: [f32; 3]) -> Option<([f32; 3], [f32; 3])> {
        if !self.cfg.trace || !finite3(from) || !finite3(dir) {
            return None;
        }
        let w = self.trace.as_ref()?;
        let len = TODO_MISS_TRACE_UNITS;
        let along = |d: f32| {
            [
                from[0] + d * dir[0],
                from[1] + d * dir[1],
                from[2] + d * dir[2],
            ]
        };
        let h = w.ray(
            dc_sim::V3::from_array(from),
            dc_sim::V3::from_array(along(len)),
        )?;
        if h.surface_flags & crate::world::SURF_SKY != 0 {
            return None;
        }
        let n = h.normal.to_array();
        finite3(n).then(|| (along(h.t.clamp(0.0, 1.0) * len), n))
    }

    /// The surface byte of a shot effect: a hit on another player's character is a head hit or a body hit by the
    /// zone the damage uses (the reported skeleton node, else the head band of the box at the victim's latest
    /// position); a hit on another object (a co-op AI character) is a body hit; a world hit is the ROM surface id of
    /// the world polygon at the end point (V-SURF-1); a miss with no polygon keeps the fallback surface.
    // DC-DEFAULT: DSV-29 (amendment, proposed); DSV-62 (proposed) the world polygon's surface
    fn shot_surface(
        &self,
        pid: u8,
        first: Option<u16>,
        to: [f32; 3],
        dir: [f32; 3],
        f: &crate::up::UpFire,
    ) -> u8 {
        let obj = match first {
            Some(o) if o != 0 => o,
            Some(_) => return self.world_surface_at(to, dir),
            None => return gs::TODO_SURFACE_WORLD,
        };
        let Some(v) = self.players.values().find(|v| v.obj == obj && v.pid != pid) else {
            return gs::SURFACE_BODY;
        };
        let d = self.hit_dims_of(v.pid);
        let head = to[1] >= v.pos[1] + d[1] - 2.0 * d[1] * self.cfg.head_fraction;
        if self.hit_zone(v.pid, f, head).1 == gs::KILL_ZONE_HEAD {
            gs::SURFACE_HEAD
        } else {
            gs::SURFACE_BODY
        }
    }

    /// `UP_PLAYER_INFO`: identity check against the lobby's roster (when the lobby passed `char=` / `slot=`), the
    /// look block for the roster records, then the introduction.
    // SPEC: GAME_MESSAGES_2009_C2S.md §5.1, Appendix A step A2
    // DC-DEFAULT: DC-C2S-5 (proposed) identity, team and loadout from the lobby; CD-60 (approved) refuse a mismatch
    fn on_player_info(&mut self, pid: u8, info: UpPlayerInfo, now: Millis) {
        let Some(p) = self.players.get(&pid) else {
            return;
        };
        if p.info.is_some() {
            return;
        }
        // DC-DEFAULT: DSV-19 (proposed) optional `char=` / `slot=` tokens of the `user` line are compared
        let want = (
            p.roster.as_ref().and_then(|e| e.char_no()),
            p.roster.as_ref().and_then(|e| e.slot()),
        );
        let mismatch = [
            (pipe::TOKEN_CHAR, want.0, info.char_no),
            (pipe::TOKEN_SLOT, want.1, info.room_slot),
        ]
        .iter()
        .find(|(_, w, v)| w.is_some_and(|w| w != *v))
        .map(|(k, _, v)| format!("{k}={v}"));
        // the client's own option fields per weapon (0xff = none), logged only: the lobby's record is authoritative
        // SPEC: GAME_MESSAGES_2009_C2S.md §5.1 (`e1..e3`); EVIDENCE: cshell+0x226700 (fields by kind 0x11 / 0x12 / 0x13)
        let options: Vec<String> = info
            .loadout
            .iter()
            .filter(|e| [e.1, e.2, e.3] != [0xff; 3])
            .map(|e| format!("{}:{}/{}/{}", e.0, e.1, e.2, e.3))
            .collect();
        let line = format!(
            "client {pid}: player info char {} slot {} team {} exp {} gp {} loadout {} clan {}{}",
            info.char_no,
            info.room_slot,
            info.team,
            info.exp,
            info.gp,
            info.loadout.len(),
            info.clan.is_some(),
            if options.is_empty() {
                String::new()
            } else {
                format!(" options {}", options.join(","))
            }
        );
        self.log(line);
        if let Some(m) = mismatch {
            self.log(format!("client {pid}: identity mismatch ({m}), refused"));
            self.out.push(GameOut::Kick(pid, CD60_REFUSE_CODE));
            return;
        }
        if let Some(p) = self.players.get_mut(&pid) {
            // DC-DEFAULT: DSV-90 (proposed) the roster look without an overridable-model row, a default for none
            p.look = info.look.sanitised();
            // DC-DEFAULT: DSV-341 (proposed) the equipment's gear in the look the other clients get
            if let Some(l) = p.roster.as_ref().and_then(|e| e.loadout_items()) {
                chars::gear_look(&mut p.look.bytes, &l);
            }
            p.info = Some(info);
        }
        self.out.push(GameOut::Mark(pid, Milestone::PlayerInfo));
        self.introduce(pid, now);
    }

    /// `UP_WORLD_READY`: the round starts with the first ready player; the player gets the match start and spawns.
    // SPEC: GAME_MESSAGES_2009_C2S.md §5.4, Appendix A steps A5, A6; GAME_MESSAGES_2009_S2C.md §10 steps 11-13
    // DC-DEFAULT: DSV-20 (staged proposal) the round clock starts with the first UP_WORLD_READY
    fn on_world_ready(&mut self, pid: u8, now: Millis) {
        let Some(p) = self.players.get(&pid) else {
            return;
        };
        if p.ready {
            return;
        }
        if !p.introduced {
            self.introduce(pid, now);
        }
        if let Some(p) = self.players.get_mut(&pid) {
            p.ready = true;
        }
        self.out.push(GameOut::Mark(pid, Milestone::WorldReady));
        // DC-DEFAULT: DSV-651 (proposed) the offer at world entry
        self.team_ping_offer(pid);
        if self.cfg.start_order == StartOrder::Reference {
            self.after_world_ready(pid, now);
            // DC-DEFAULT: DSV-511 (proposed) Quarantine: the running outbreak countdown again once the client is in the
            // world (the round started at the first player's info, while this client was loading)
            self.infect_countdown_to(pid, now);
            return;
        }
        self.start_round(now);
        let RoundPhase::Running { .. } = self.phase else {
            return;
        };
        let left = self.round_secs_left(now);
        // DC-DEFAULT: DSV-36 (proposed) 0x45 only to the round's first spawn
        if self.first_spawn_member(pid) {
            let intro = self.cfg.show_intro.then(|| self.intro_block());
            let mb = gs::MatchBegin {
                round_secs: left,
                // DC-DEFAULT: DSV-20 (revision, proposed) the waiting banner off in a running round
                // DC-DEFAULT: DSV-512 (proposed) the training banner during the warm-up
                waiting_banner: self.cfg.waiting_banner || self.in_warmup(),
                countdown_secs: 0,
                intro,
                round_no: 0,
            };
            self.send(pid, gs::match_begin(&mb, self.rules.mode_class));
        } else {
            self.send(pid, gs::scores_clear());
            // DC-DEFAULT: GMD-23 (staged) the round number and the goal in a round mode
            let (no, limit) = (self.round_no.max(1), self.round_begin_limit());
            self.send(pid, gs::round_begin(left, no, limit, false));
        }
        // DC-DEFAULT: DC-KILL-3 (proposed) team kills in 0xb5, never 0x4a; GMD-21 (staged) round wins in a round mode
        self.send(pid, gs::round_wins(self.team_counters()));
        if self.cfg.perf_enable {
            self.send(
                pid,
                gs::perf_enable(TODO_PERF_ENABLE_PID, true, PERF_PING_MS, PERF_STATS_MS),
            );
        }
        self.spawn(pid, now);
        // DC-DEFAULT: DSV-511 (proposed) Quarantine: the running outbreak countdown to a player who enters the world
        // after the round start (the round's first ready player started it; bots load first)
        self.infect_countdown_to(pid, now);
    }

    /// Reference order after `UP_WORLD_READY`: shell 0x90 [0] and 0x88 state 3, a second teleport to the same place
    /// (new `move_seq`), health and armour, and the friction again.
    // SPEC: UDP_REFERENCE_FINDINGS_botclient.md O2 (`0d 90 00`, `0d 88 03` after 0x87), O9 (teleport seq 1 then seq 2 at
    // the same position; REFERENCE); MOVEMENT_2009.md §8.4, §12.4
    fn after_world_ready(&mut self, pid: u8, now: Millis) {
        let dims = self.dims_of(pid);
        // SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §2.3 (a joiner of a running round got 0x90 [0] / 0x88 [3] at the
        // link and not again)
        if !self.players.get(&pid).is_some_and(|p| p.switch_sent) {
            self.send(pid, gs::shell_flag_90(false));
            self.send(
                pid,
                crate::msg::s2c_switch_state(REFERENCE_SWITCH_STATE_READY),
            );
        }
        let Some(p) = self.players.get_mut(&pid) else {
            return;
        };
        if p.life != Life::Alive {
            return;
        }
        p.move_seq = p.move_seq.wrapping_add(1);
        p.last_move = Some(now);
        let became_visible = !p.visible;
        p.visible = true;
        let (seq, pos, health, armor, yaw) = (p.move_seq, p.pos, p.health, p.armor, p.yaw);
        if became_visible {
            self.out.push(GameOut::Visible { pid, pos, yaw });
        }
        self.send(
            pid,
            gs::mover_update(&gs::MoverUpdate {
                teleport: Some((seq, pos, dims)),
                ..gs::MoverUpdate::default()
            }),
        );
        self.send(pid, gs::player_status(gs::STATUS_HEALTH, health));
        self.send(pid, gs::player_status(gs::STATUS_ARMOR, armor));
        self.send(
            pid,
            gs::mover_update(&gs::MoverUpdate {
                friction: Some(gs::FRICTION_NO_CONTAINER),
                ..gs::MoverUpdate::default()
            }),
        );
    }

    /// The mover body of a player: the operator's `-bodymodel` when given, else the character's body model and its
    /// two skins from the ROM.
    // SPEC: MOVEMENT_2009.md §12.4 (0x0001 `[model:str][n:u8][n x skin:str][passable:1 bit]`)
    // OBSERVED: ROM table key MODELBUTES.TXT [Model<n>] ModelFile, UpperSkin<k>, LowerSkin<k> (the reference stack's body block
    // named exactly these, compared by sha256 in tests/reference_replay.rs)
    fn body_of(&self, pid: u8) -> Option<BodySpec> {
        if let Some(b) = &self.cfg.body {
            return Some(b.clone());
        }
        let c = self.players.get(&pid)?.character.as_ref()?;
        Some(BodySpec {
            model: chars::announce_spelling(c.assets.body.first()?).into_bytes(),
            skins: c
                .assets
                .mover_skins
                .iter()
                .map(|s| chars::announce_spelling(s).into_bytes())
                .collect(),
            passable: false,
        })
    }

    /// `UP_WEAPON_CHANGE`: the character's hand model follows the drawn weapon.
    // DC-DEFAULT: DSV-25 (proposed) the attached weapon model shows the drawn weapon
    fn hand_weapon_changed(&mut self, pid: u8, weapon_id: u16) {
        let drawn = self
            .players
            .get(&pid)
            .and_then(|p| p.current.and_then(|i| p.weapons.get(i)).map(|w| w.weapon))
            .unwrap_or(weapon_id);
        let Some(p) = self.players.get(&pid) else {
            return;
        };
        if p.character.is_none() || p.hand_weapon == Some(drawn) {
            return;
        }
        let Some(look) = self.weapon_look(drawn) else {
            return;
        };
        if let Some(p) = self.players.get_mut(&pid) {
            p.hand_weapon = Some(drawn);
            // a character that had no loadout weapon gets its weapon model now (the session creates the object)
            if let Some(c) = p.character.as_mut() {
                c.weapon.get_or_insert(look);
            }
        }
        self.out.push(GameOut::HandWeapon { pid, weapon: drawn });
    }

    /// Intro block: the room line's pictures and corners (DSV-50), else the world box; the team spawn centres.
    fn intro_block(&self) -> gs::Intro {
        let centre = |team: u8| {
            let pts: Vec<[f32; 3]> = self
                .spawns
                .iter()
                .filter(|s| s.2 == Some(team))
                .map(|s| s.0)
                .collect();
            if pts.is_empty() {
                return self.centre;
            }
            let mut c = [0.0; 3];
            for p in &pts {
                for i in 0..3 {
                    c[i] += p[i] / pts.len() as f32;
                }
            }
            c
        };
        let mut intro = gs::Intro {
            map_min: self.pbox.min,
            map_max: self.pbox.max,
            radar_min: self.pbox.min,
            radar_max: self.pbox.max,
            radar_zoom: 1.0,
            base_team1: centre(1),
            base_team2: centre(2),
            ..gs::Intro::default()
        };
        // DC-DEFAULT: DSV-50 (proposed) pictures, corners, zoom, rotation and sizes from the room line
        self.apply_room_intro(&mut intro);
        intro
    }

    /// The armour of a player's next life: the worn armour ([`ArmorGear`]: the ROM armour row every player wears,
    /// or the armour items its loadout names with their ROM `Armor` / `ArmorGrade`), the body row's maximum, or the
    /// operator's fixed values.
    // project decision: calibrated to the captured 2009 per-hit values (CAPTURE_tdm6 §2.1, tdm4 §3.1: armour
    // 100 at every spawn without an armour item)
    // DC-DEFAULT: DSV-30 (proposed)
    pub fn armour_of(&self, pid: u8) -> Armour {
        let p = self.players.get(&pid);
        let max = self.cfg.max_armor.unwrap_or_else(|| {
            p.and_then(|p| p.character.as_ref())
                .and_then(|c| c.assets.max_armor)
                .map_or(DEFAULT_MAX_ARMOR, |m| m as f32)
        });
        let (rom_start, rom_absorb) = match self.cfg.armor_gear {
            ArmorGear::Row(n) => (
                self.tables
                    .weapons
                    .gear
                    .get(&n)
                    .and_then(|g| g.armor)
                    .unwrap_or(0.0),
                Absorb::Gear(n),
            ),
            ArmorGear::Worn => {
                let body = p.and_then(|p| p.character.as_ref()).map(|c| c.assets.gear);
                // DC-DEFAULT: DSV-481 (proposed) Gun Game: no shop armour, the body's gear alone
                let equipped = p
                    .and_then(|p| p.roster.as_ref())
                    .filter(|_| !self.is_gun_game())
                    .and_then(|e| e.loadout_items())
                    .unwrap_or_default();
                // the body row's gear is looked up on the assets (the body may come from a `model=` token)
                let mut rows = self.tables.chars.worn_gear(None, &equipped);
                rows[0] = Some(body.flatten().unwrap_or(0));
                let rows = dedup_worn(rows.map(|r| r.and_then(|g| u16::try_from(g).ok())));
                (
                    rows.iter()
                        .flatten()
                        .filter_map(|g| self.tables.weapons.gear.get(g).and_then(|r| r.armor))
                        .fold(0.0, |acc, a| acc + a),
                    Absorb::Worn(rows),
                )
            }
            ArmorGear::Items => {
                let worn: Vec<crate::rom_tables::GearRow> = p
                    .and_then(|p| p.roster.as_ref())
                    .and_then(|e| e.loadout_items())
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|(_, item)| self.tables.items.gear_of(*item))
                    .filter(|g| g.armor > 0.0)
                    .collect();
                (
                    // a fold from +0.0: an empty float `sum` is -0.0, which would go on the wire with its sign bit
                    worn.iter().fold(0.0, |acc, g| acc + g.armor),
                    Absorb::Flat(worn.iter().map(|g| g.grade / 100.0).fold(0.0, f32::max)),
                )
            }
            ArmorGear::Off => (0.0, Absorb::Flat(0.0)),
        };
        let start = self
            .cfg
            .start_armor
            .unwrap_or(rom_start)
            .clamp(0.0, max.max(0.0));
        let absorb = match (self.cfg.armor_absorb, rom_absorb) {
            (Some(x), _) | (None, Absorb::Flat(x)) => Absorb::Flat(x.clamp(0.0, 1.0)),
            (None, g) => g,
        };
        Armour { start, max, absorb }
    }

    /// The zone multiplier and the kill-feed zone of a hit: the victim's ROM skeleton node the client named
    /// (`UP_FIRE` extra block `extra_kind`, when its object is the hit object), else the box geometry (head =
    /// the top `head_fraction`).
    // OBSERVED: CAPTURE_tdm4_KILL_VALIDATION.md §2.1 (`extra_obj` = `hit_obj`, `extra_kind` 24 / 11 for hits 24-27
    // units above the centre: the shoulder nodes 24 / 11 of the ROM body rows' skeleton, location 3, the reference stack's
    // `hit_zone` 3); ROM table key MODELBUTES.TXT [Skeleton<s>] Node<k>DamageFactor, Node<k>Location
    // SPEC: UNPROVEN that `extra_kind` is the node index (PLAUSIBLE by the two observations above)
    // DC-DEFAULT: DSV-30 (proposed)
    pub fn hit_zone(&self, vpid: u8, f: &crate::up::UpFire, head: bool) -> (f32, u8) {
        let (factor, zone) = self.victim_zone(vpid, f, head);
        // DC-DEFAULT: DSV-608 (proposed; OWNER REPORT 2026-11-05) an infected victim's head factor is reduced
        (self.infected_zone_factor(vpid, factor, zone), zone)
    }

    /// [`Game::hit_zone`] before the Quarantine rule: the victim's own node factor or the box factor.
    fn victim_zone(&self, vpid: u8, f: &crate::up::UpFire, head: bool) -> (f32, u8) {
        let geometry = if head {
            (self.cfg.head_factor, gs::KILL_ZONE_HEAD)
        } else {
            (self.cfg.body_factor, TODO_KILL_ZONE_BODY)
        };
        if !self.cfg.zone_factors_from_rom {
            return geometry;
        }
        let hit_obj = f.hit.and_then(|h| h.1);
        let node = f
            .extra
            .filter(|x| x.obj.is_some() && x.obj == hit_obj)
            .and_then(|x| {
                self.players
                    .get(&vpid)?
                    .character
                    .as_ref()?
                    .assets
                    .hit_nodes
                    .get(x.kind as usize)
                    .copied()
            });
        match node {
            Some(n) => (
                n.factor,
                if n.location == 0 {
                    geometry.1
                } else {
                    n.location
                },
            ),
            None => geometry,
        }
    }

    /// The weapons a player spawns with, by weapon slot (0..3, ascending; the lowest slot is drawn): the `user`
    /// line's `loadout=` weapon items (`slot:item` with ROM item numbers below 1000 = weapon rows; a bare number takes
    /// the next free slot, the slice-2 form), else the operator's `-loadout`, else the ROM default-issue weapon of
    /// every slot. A player never spawns unarmed while the ROM has weapons.
    // OBSERVED: CAPTURE_tdm4_WEAPON_FORMS.md §2 (equipment slot i = weapon slot i, item number = weapon number; the
    // client drew the lowest granted slot)
    // DC-DEFAULT: DSV-38 (proposed, refines DSV-15); CD-18 (approved) the ROM default weapons by slot kind; CD-18 /
    // CD-52 (approved) which items are equipped stays the lobby's rule
    pub fn loadout_weapons(&self, roster: Option<&RosterEntry>) -> Vec<(u8, u16)> {
        // DC-DEFAULT: DSV-481 (proposed) Gun Game: no shop loadout (rung 0's kit; `spawn` takes the player's rung)
        if self.is_gun_game() {
            return self.gun_game_spawn_kit(u8::MAX).unwrap_or_default();
        }
        let mut out: Vec<(u8, u16)> = Vec::new();
        // `slot` is the lobby's equipment slot (0..3 weapon slots, 10..13 backpack slots, DSV-38)
        let put = |out: &mut Vec<(u8, u16)>, slot: Option<u8>, w: u16| {
            let slot = match slot {
                Some(s) => match weapon_slot_of_equip(s) {
                    Some(s) => s,
                    None => return,
                },
                None => match (0..WEAPON_SLOTS).find(|s| !out.iter().any(|x| x.0 == *s)) {
                    Some(s) => s,
                    None => return,
                },
            };
            if !out.iter().any(|x| x.0 == slot) {
                out.push((slot, w));
            }
        };
        if let Some(items) = roster.and_then(|e| e.loadout_items()) {
            for (slot, item) in items {
                if let crate::rom_tables::ItemNo::Weapon(w) = crate::rom_tables::item_no(item) {
                    if self.arms_grants(slot, w) {
                        put(&mut out, slot, w);
                    }
                }
            }
        }
        if out.is_empty() {
            for &w in &self.cfg.default_loadout {
                if self.arms_grants(None, w) {
                    put(&mut out, None, w);
                }
            }
        }
        if out.is_empty() {
            for (s, w) in self.tables.items.issue_weapons.iter().enumerate() {
                if let Some(w) = w.filter(|w| self.arms_grants(None, *w)) {
                    put(&mut out, Some(s as u8), w);
                }
            }
        }
        if out.is_empty() {
            for (s, w) in self.tables.items.issue_weapons.iter().enumerate() {
                if let Some(w) = w {
                    put(&mut out, Some(s as u8), *w);
                }
            }
        }
        out.sort();
        out
    }

    /// Whether a client message of `pid` naming weapon `w` is refused under the room's weapon limit: the weapon is not
    /// one the server gave the player (spawn set, mode item, pickup) and the limit keeps its kind out of the match, so
    /// a client cannot draw or fire it by naming it (the server would otherwise adopt it, DSV-15). Logged once per
    /// player and weapon.
    // DC-DEFAULT: DSV-476 (proposed, USER REPORT 2026-10-31 "pistols are available to use in a sniper-only match") the
    // 2009 client's only weapon-limit check is the one before Ready / Start (cshell+0x160f70 -> cshell+0x226dd0, no
    // other caller), so the match side of the rule is the server's: such a shot gets no record and no damage, such a
    // weapon change is not taken
    pub(crate) fn refuse_barred(&mut self, pid: u8, w: u16, what: &str) -> bool {
        // DC-DEFAULT: DSV-481 (proposed) Gun Game: only what the server gave, whatever the weapon
        // DC-DEFAULT: DSV-483 (proposed) Possession too: the killer's old guns are gone
        if self.is_gun_game() || self.is_possession() {
            let carried = self
                .players
                .get(&pid)
                .is_some_and(|p| p.weapons.iter().any(|x| x.weapon == w));
            if !carried && self.barred_logged.insert((pid, w)) {
                self.log(format!(
                    "client {pid}: {what} with weapon {w} refused (gun game)"
                ));
            }
            return !carried;
        }
        if self.rules.arms == 0 {
            return false;
        }
        let Some(p) = self.players.get(&pid) else {
            return false;
        };
        // DC-DEFAULT: DSV-619 (proposed) a controller passes with the carried weapon it controls (the client draws it
        // after the claymore's placement)
        let controls = self.tables.weapons.controllee(w);
        if p.weapons
            .iter()
            .any(|x| x.weapon == w || controls == Some(x.weapon))
        {
            return false;
        }
        // DC-DEFAULT: DSV-610 (proposed) a weapon of the player's own lobby loadout is judged in its equipment slot
        // (a weapon slot before a backpack slot), so "No Backpack" refuses the backpack weapon a client names
        let slot = p
            .roster
            .as_ref()
            .and_then(|r| r.loadout_items())
            .and_then(|items| {
                items
                    .into_iter()
                    .filter(|(_, item)| *item == u32::from(w))
                    .filter_map(|(s, _)| s)
                    .min()
            });
        if self.arms_grants(slot, w) {
            return false;
        }
        if self.barred_logged.insert((pid, w)) {
            self.log(format!(
                "client {pid}: {what} with weapon {w} refused (room weapon limit {})",
                self.rules.arms
            ));
        }
        true
    }

    /// Whether the room's weapon limit lets the lobby's equipped weapon `w` (equipment slot `slot`) into the match. A
    /// variant has its base gun's type; a weapon whose type stays unknown goes by its slot's class
    /// (`dc_bute::arms::match_grants_unknown`). Without the item table (no weapon type known at all) every weapon
    /// passes.
    // DC-DEFAULT: DSV-180 (proposed), DSV-476 (proposed) the grant rule of `dc_bute::arms::match_grants`; backpack =
    // equipment slots 10..13
    // DC-DEFAULT: DSV-610 (proposed, OWNER REPORT 2026-11-05 "no backpack option for a game mode still lets you bring
    // your backpack item, same with pistols only") before, a weapon without an item row (every Vanilla++ camo
    // variant, 500..899) passed every limit
    fn arms_grants(&self, slot: Option<u8>, w: u16) -> bool {
        let limit = self.rules.arms;
        if limit == 0 || self.tables.items.weapon_types.is_empty() {
            return true;
        }
        dc_bute::arms::match_grants_in_slot(limit, self.tables.weapon_type(w), slot.map(i64::from))
    }

    /// The mod index per socket (muzzle, sight, magazine) the lobby attached to the weapon granted in `granted_slot`:
    /// the `parts=` entry of the equipment slot whose `loadout=` item is that weapon and maps to that weapon slot
    /// (the first such slot, like the loadout's first claim). Option item numbers `1000 + ItemID` give the mod index
    /// ItemID; other numbers give nothing.
    // EVIDENCE: cshell+0x25c010 (options are 1000 + ItemID; the client keeps n % 1000), cshell+0x287930 / cshell+0x226700
    // (the client files that number under the weapon by kind 0x11 / 0x12 / 0x13)
    // OBSERVED: capture 20261025_153705_rust7 (item 1000 + 35 attached to the M4A1 -> 35 in the
    // sight field of the client's own `UP_PLAYER_INFO` entry)
    // DC-DEFAULT: DSV-99 (proposed)
    pub fn player_options(
        &self,
        roster: Option<&RosterEntry>,
        granted_slot: u8,
        weapon: u16,
    ) -> [Option<u16>; 3] {
        let Some(r) = roster else {
            return [None; 3];
        };
        // DC-DEFAULT: DSV-481 (proposed) Gun Game: no weapon mods
        if self.is_gun_game() {
            return [None; 3];
        }
        let options = r.weapon_options();
        if options.is_empty() {
            return [None; 3];
        }
        let equip = r.loadout_items().and_then(|items| {
            items.into_iter().find_map(|(s, item)| {
                let s = s?;
                (weapon_slot_of_equip(s) == Some(granted_slot) && item == u32::from(weapon))
                    .then_some(s)
            })
        });
        let Some(set) = equip.and_then(|s| options.iter().find(|o| o.0 == s).map(|o| o.1)) else {
            return [None; 3];
        };
        set.map(|n| match n.map(crate::rom_tables::item_no) {
            Some(crate::rom_tables::ItemNo::Option(m)) => Some(m),
            _ => None,
        })
    }

    fn new_weapon(&self, weapon: u16, ammo: Option<u16>) -> WeaponState {
        let t = &self.tables.weapons;
        let ammo = ammo.or_else(|| t.ammo_of(weapon)).unwrap_or(0);
        let clip_size = t.clip(weapon).unwrap_or(self.cfg.default_clip);
        let granted = t.max_amount(ammo).unwrap_or(self.cfg.default_stock);
        let own_ammo = t.is_own_ammo(weapon);
        // a weapon that is its own ammunition: the grant includes the round in the hand (a 2-pack is two throws,
        // not a clip of one plus a stock of two); other weapons: a full clip plus the granted stock
        // DC-DEFAULT: DSV-39 (amended, proposed); DC-C2S-4 (proposed)
        let (clip, stock) = if own_ammo && clip_size > 0 {
            let clip = clip_size.min(granted);
            (clip, granted - clip)
        } else {
            (clip_size, granted)
        };
        WeaponState {
            weapon,
            ammo,
            clip_size,
            clip,
            stock,
            slot: 0,
            own_ammo,
            parts: gs::NO_PARTS,
            shot_open: false,
            shot_ok: false,
        }
    }

    /// The weapon a new kit is drawn with (the index into `weapons`): the slot-0 weapon when granted; else the
    /// carried weapon of the best item slot class (primary, secondary, melee, thrown, then the rest), the lowest slot
    /// first. A primary in a backpack slot (4..7) so beats the pistol and the knife when slot 0 is empty.
    // EVIDENCE: cshell+0x259f00 (item type -> slot class 0 primary .. 3 thrown); cshell+0x22ec00 (the 2009 empty
    // switch prefers the class-1 entry, then class 2, then class 3: rifles before pistols before the knife)
    // DC-DEFAULT: DSV-568 (proposed, USER REPORT 2026-11-04 "I respawned with my knife instead of my primary gun")
    pub(crate) fn draw_index(&self, weapons: &[WeaponState]) -> Option<usize> {
        // DC-DEFAULT: VAR-1 (proposed) a variant draws as its base gun's class
        let class = |w: u16| {
            self.tables
                .weapon_type(w)
                .and_then(dc_bute::arms::slot_class)
                .unwrap_or(u8::MAX)
        };
        (0..weapons.len()).min_by_key(|&i| {
            let w = &weapons[i];
            (w.slot != 0, class(w.weapon), w.slot, i)
        })
    }

    /// The spawn set. Spec order: teleport (body on the first spawn), facing, weapons, health / armour, state 1.
    /// Reference order: facing, state 1, maxima and status reset, every weapon given and the drawn one switched to,
    /// body (first spawn) with the friction, teleport, health / armour.
    // SPEC: GAME_MESSAGES_2009_C2S.md Appendix A step A6, §3.4 server answer; GAME_MESSAGES_2009_S2C.md §10 steps
    // 7-10, 18; MOVEMENT_2009.md §8.4, §12.4; UDP_REFERENCE_FINDINGS_botclient.md O2, O9 (REFERENCE)
    // DC-DEFAULT: DC-C2S-10' (proposed, GAME_MESSAGES_2009_C2S.md §12.3) spawn set; DSV-22 (proposed) reference order
    fn spawn(&mut self, pid: u8, now: Millis) {
        let Some((team, key)) = self.players.get(&pid).map(|p| {
            let key = (self.cfg.spawn_pick == SpawnPick::Keyed)
                .then(|| spawn_key(&p.name8, p.team, p.spawns.saturating_add(1)));
            (p.team, key)
        }) else {
            return;
        };
        let (pos, rot) = self.choose_spawn_for(team, key, Some(pid));
        let loadout = self
            .players
            .get(&pid)
            .map(|p| self.loadout_weapons(p.roster.as_ref()))
            .unwrap_or_default();
        let mut weapons: Vec<WeaponState> = loadout
            .iter()
            .map(|&(slot, w)| WeaponState {
                slot,
                ..self.new_weapon(w, None)
            })
            .collect();
        // DC-DEFAULT: DSV-215 (proposed) an infected player spawns with the claws only
        if let Some(c) = self.claws_of(pid) {
            weapons = vec![self.claws_state(c)];
        }
        // DC-DEFAULT: DSV-233 (proposed) the agent spawns with the agent weapons
        // DC-DEFAULT: DSV-481 (proposed) Gun Game: the kit of the player's rung
        if let Some(ws) = self
            .agent_weapons_of(pid)
            .or_else(|| self.gun_game_spawn_kit(pid))
        {
            weapons = ws
                .iter()
                .map(|&(slot, w)| WeaponState {
                    slot,
                    ..self.new_weapon(w, None)
                })
                .collect();
        }
        let cfg = self.cfg.clone();
        // DC-DEFAULT: DSV-568 (proposed) the kit's drawn weapon (slot 0, else the best slot class)
        let drawn = self.draw_index(&weapons);
        let spawn_ani = drawn
            .and_then(|d| weapons.get(d))
            .and_then(|w| self.ani_of(w.weapon));
        let dims = self.dims_of(pid);
        // DC-DEFAULT: DSV-48 (proposed) the box on the floor below the spawn point
        let pos = self.floor_point(pos, dims);
        let armour = self.armour_of(pid);
        let Some(p) = self.players.get_mut(&pid) else {
            return;
        };
        p.move_seq = p.move_seq.wrapping_add(1);
        p.pos = pos;
        p.track.restart(now, pos);
        p.vel = [0.0; 3];
        p.yaw = rot[1];
        p.last_move = Some(now);
        p.health = cfg.start_health;
        p.armor = armour.start;
        p.armor_absorb = armour.absorb;
        p.life = Life::Alive;
        // DC-DEFAULT: DSV-483 (proposed) a spawn ends a move
        p.flight = None;
        p.stand_pos = Some(pos);
        p.on_ground_report = true;
        let first = p.spawns == 0;
        p.spawns += 1;
        p.killed_by = None;
        p.kills_this_life = 0;
        p.weapons = weapons.clone();
        p.current = drawn;
        // DC-DEFAULT: DSV-615 (proposed) a spawn switches the heartbeat sensor off (the client does too, cshell+0xb4f00)
        p.scanning = false;
        p.scan_answered = None;
        p.motion = Motion::Stand;
        p.motion_since = now;
        p.spawned_at = now;
        // DC-DEFAULT: DSV-44 (amendment, proposed) the trackers start again at the spawn
        p.death_clip = None;
        // DC-DEFAULT: DSV-125 (proposed) no action in play and on the ground at the spawn
        p.pose.reset(spawn_ani);
        // reference order: the character becomes visible once its client reported the world ready (the first spawn
        // goes out before UP_WORLD_READY); spec order and respawns: at the spawn
        // SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run, CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §1.4 (flags 0x3803
        // after the 0x87; REFERENCE, ACCEPTED)
        let make_visible = cfg.start_order == StartOrder::Spec || p.ready || p.visible;
        if make_visible {
            p.visible = true;
        }
        let seq = p.move_seq;
        // reference order: the body again at every respawn (`0xa0` 0x0009 = body + friction; the first spawn adds the
        // empty container list, 0x000d); spec order: the first spawn only
        // SPEC: REFERENCE_FINDINGS_botduel.md §2 (respawn answer with 0xa0 flags 0x0009; REFERENCE); MOVEMENT_2009.md §12.4
        let body = if first || cfg.start_order == StartOrder::Reference {
            self.body_of(pid)
        } else {
            None
        };
        // `class_pos`: the weapon's position in its class list (the client keeps one slot per class position): the
        // number of weapons of the same class in lower slots
        // EVIDENCE: cshell+0x22edd0 (table [class][class_pos] := slot, class_pos < 33), cshell+0x47da0 (class)
        // OBSERVED: tdm6 (the reference stack's class_pos 0 for the four slots, 1 for the backpack shotgun, a second weapon of
        // the primary class; REFERENCE)
        // DC-DEFAULT: DSV-38 (extended, staged)
        let class_pos: Vec<u8> = weapons
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let class = self.tables.weapons.class_of(w.weapon);
                class.map_or(0, |c| {
                    weapons[..i]
                        .iter()
                        .filter(|x| self.tables.weapons.class_of(x.weapon) == Some(c))
                        .count()
                        .min(32) as u8
                })
            })
            .collect();
        // DC-DEFAULT: DSV-80 (proposed) the rows' default attachments (the scope the client's zoom needs)
        // DISASM: cshell+0x60d90, cshell+0x235ec0, cshell+0x1f4180
        // DC-DEFAULT: DSV-99 (proposed) the player's attachments from the lobby, socket by socket over the defaults
        // DISASM: cshell+0x235ec0 (the socket is the mod row's Kinds)
        let roster = self.players.get(&pid).and_then(|p| p.roster.clone());
        let mut notes = Vec::new();
        let parts: Vec<[i8; 4]> = weapons
            .iter()
            .map(|w| {
                if !cfg.weapon_parts {
                    return gs::NO_PARTS;
                }
                let chosen = if cfg.player_parts {
                    self.player_options(roster.as_ref(), w.slot, w.weapon)
                } else {
                    [None; 3]
                };
                let parts = self.tables.weapons.player_parts(w.weapon, chosen);
                if first && chosen.iter().any(Option::is_some) {
                    notes.push(format!(
                        "client {pid}: weapon {} attachments {:?} -> parts {:?} (zoom {})",
                        w.weapon,
                        chosen,
                        parts,
                        self.tables.weapons.zooms_with(&parts)
                    ));
                }
                parts
            })
            .collect();
        for n in notes {
            self.log(n);
        }
        // a magazine mod adds its rounds to the clip and to the carried maximum (the grant tells the client the
        // changed clip; its own clip size follows the same mod); a Fast Loader's negative entry shrinks the clip,
        // never below 1, and the rounds it no longer holds stay in the stock
        // EVIDENCE: cshell+0x233430, cshell+0x233490; DC-DEFAULT: DSV-182 (proposed) the server mirror follows them,
        // CLI-1440 (proposed) the bound of 1 and the rounds kept
        for (w, pp) in weapons.iter_mut().zip(&parts) {
            let (clip_plus, max_plus) = self.tables.weapons.magazine_bonus(w.weapon, pp);
            if w.clip_size > 0 && !w.own_ammo {
                let size = (i64::from(w.clip_size) + i64::from(clip_plus)).max(1);
                let size = u32::try_from(size).unwrap_or(u32::MAX);
                if size >= w.clip_size {
                    w.clip += size - w.clip_size;
                } else {
                    let cut = w.clip.saturating_sub(size);
                    w.clip -= cut;
                    w.stock += cut;
                }
                w.clip_size = size;
                w.stock = u32::try_from((i64::from(w.stock) + i64::from(max_plus)).max(0))
                    .unwrap_or(u32::MAX);
            }
        }
        // DC-DEFAULT: DSV-167 (proposed) each held weapon keeps its granted parts for the shot record's mods
        if let Some(p) = self.players.get_mut(&pid) {
            for ((w, pp), granted) in p.weapons.iter_mut().zip(&parts).zip(&weapons) {
                w.parts = *pp;
                w.clip_size = granted.clip_size;
                w.clip = granted.clip;
                w.stock = granted.stock;
            }
        }
        let spec = |i: usize, w: &WeaponState| WeaponSpec {
            weapon: w.weapon,
            ammo_kind: w.ammo,
            ammo: w.told_amount(),
            clip: w.clip,
            class_pos: class_pos.get(i).copied().unwrap_or(0),
            slot: w.slot,
            parts: parts.get(i).copied().unwrap_or(gs::NO_PARTS),
        };
        let drawn_w = drawn.and_then(|d| weapons.get(d).map(|w| (d, *w)));
        let command = drawn_w.and_then(|(_, w)| self.tables.weapons.command_of(w.weapon));
        // a player frozen at an earlier round result is released before the teleport (the reference stack's spawn set also
        // carried a freeze 0 after its teleport)
        // OBSERVED: capture 20261025_000014_tdm1 c5 seq 410 (`0d a0 80 00 00`; REFERENCE)
        // DC-DEFAULT: DSV-77 (reinstated, proposed)
        self.release_for_spawn(pid);
        if cfg.start_order == StartOrder::Reference {
            self.send(pid, gs::view_angles(gs::ANGLES_ALL, rot));
            self.send(pid, gs::player_state(gs::STATE_ALIVE));
            self.send(
                pid,
                gs::player_status(gs::STATUS_MAX_HEALTH, cfg.max_health),
            );
            self.send(pid, gs::player_status(gs::STATUS_MAX_ARMOR, armour.max));
            // SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (`66 13 01 ..` before the weapons; REFERENCE)
            self.send(pid, gs::player_status_b(gs::STATUS_RESET, 1, 0, 0.0));
            // without a command the drawn weapon goes by 0xb3 alone (not given twice)
            let given_apart = drawn.filter(|_| command.is_none());
            for (i, w) in weapons.iter().enumerate() {
                if Some(i) != given_apart {
                    self.send(pid, gs::weapon_give(&spec(i, w)));
                }
            }
            match (command, drawn_w) {
                // SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (0x7f auto switch; ammo -1 = the row's default)
                (Some(c), Some(_)) => self.send(pid, gs::weapon_switch(c, true, -1.0, 0)),
                (None, Some((d, w))) => self.send(pid, gs::drawn_weapon(&spec(d, &w))),
                _ => {}
            }
            if let Some(b) = &body {
                self.send(
                    pid,
                    gs::mover_update(&gs::MoverUpdate {
                        containers_friction: first.then_some(gs::FRICTION_NO_CONTAINER),
                        body: Some(b),
                        friction: Some(gs::FRICTION_NO_CONTAINER),
                        ..gs::MoverUpdate::default()
                    }),
                );
            }
            self.send(
                pid,
                gs::mover_update(&gs::MoverUpdate {
                    teleport: Some((seq, pos, dims)),
                    ..gs::MoverUpdate::default()
                }),
            );
            // DC-DEFAULT: DSV-49 (proposed) the melee and throwable slot status before health and armour
            self.slot_status(pid, &weapons);
            self.send(pid, gs::player_status(gs::STATUS_HEALTH, cfg.start_health));
            self.send(pid, gs::player_status(gs::STATUS_ARMOR, armour.start));
        } else {
            self.send(
                pid,
                gs::mover_update(&gs::MoverUpdate {
                    teleport: Some((seq, pos, dims)),
                    containers_friction: body.as_ref().map(|_| gs::FRICTION_NO_CONTAINER),
                    body: body.as_ref(),
                    ..gs::MoverUpdate::default()
                }),
            );
            self.send(pid, gs::view_angles(gs::ANGLES_ALL, rot));
            for (i, w) in weapons.iter().enumerate() {
                if Some(i) != drawn {
                    self.send(pid, gs::weapon_give(&spec(i, w)));
                }
            }
            if let Some((d, w)) = drawn_w {
                self.send(pid, gs::drawn_weapon(&spec(d, &w)));
            }
            self.send(
                pid,
                gs::player_status(gs::STATUS_MAX_HEALTH, cfg.max_health),
            );
            self.send(pid, gs::player_status(gs::STATUS_MAX_ARMOR, armour.max));
            // DC-DEFAULT: DSV-49 (proposed) the melee and throwable slot status before health and armour
            self.slot_status(pid, &weapons);
            self.send(pid, gs::player_status(gs::STATUS_HEALTH, cfg.start_health));
            self.send(pid, gs::player_status(gs::STATUS_ARMOR, armour.start));
            self.send(pid, gs::player_state(gs::STATE_ALIVE));
        }
        // the spawned player's record to everyone, itself included (kind 2, alive): the other clients learn of the
        // respawn from it
        // SPEC: REFERENCE_FINDINGS_botduel.md F2 (0x6b refresh of the respawned player, also to the other client; REFERENCE);
        // CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §2.5 (kind 2 `dead` 0 after the spawn set)
        // DC-DEFAULT: CV2-7 (proposed)
        if let Some(p) = self.players.get(&pid) {
            let rec = gs::player_info(&self.info_with(p, gs::INFO_UPDATE, false));
            if let Some(u) = p.uid {
                self.round_spawned.insert(u);
            }
            self.broadcast(&rec);
        }
        self.coop_after_spawn(pid);
        self.out.push(GameOut::Spawned {
            pid,
            pos,
            yaw: rot[1],
            first,
        });
        self.spawn_note_start(pid, now);
        if make_visible {
            self.out.push(GameOut::Visible {
                pid,
                pos,
                yaw: rot[1],
            });
        }
        self.out.push(GameOut::Mark(pid, Milestone::Spawned));
        // the drawn weapon in the spawn line: a respawn with the wrong weapon in the hand shows here
        let hand = drawn_w.map_or("none".to_string(), |(_, w)| {
            format!("slot {} weapon {}", w.slot, w.weapon)
        });
        self.log(format!(
            "client {pid}: spawned (team {team}, seq {seq}) at ({:.1}, {:.1}, {:.1}), drawn {hand}",
            pos[0], pos[1], pos[2]
        ));
    }

    /// `UP_CHAT` -> `GS_CHAT` to everyone (scope 0) or the sender's team (scope 1), the sender included.
    // SPEC: GAME_MESSAGES_2009_C2S.md §6.1 server answer; GAME_MESSAGES_2009_S2C.md §2.4
    // DC-DEFAULT: CD-30 (approved) audience includes the sender
    fn on_chat(&mut self, pid: u8, text: &[u16], scope: Option<u8>) {
        let Some(p) = self.players.get(&pid) else {
            return;
        };
        let team_only = scope == Some(1);
        let team = p.team;
        let m = gs::chat(&p.name16, text, pid as u32, team_only);
        let to: Vec<u8> = self
            .players
            .values()
            .filter(|q| q.introduced && (!team_only || q.team == team))
            .map(|q| q.pid)
            .collect();
        for q in to {
            self.send(q, m.clone());
        }
    }

    /// `UP_ROSTER_QUERY` -> the records asked for (0xFFFFFFFF = all).
    // SPEC: GAME_MESSAGES_2009_C2S.md §5.5
    fn on_roster_query(&mut self, pid: u8, q: u32) {
        let recs: Vec<PlayerInfo> = self
            .players
            .values()
            .filter(|p| p.introduced && (q == 0xFFFF_FFFF || p.pid as u32 == q))
            .map(|p| self.info_of(p, gs::INFO_REFRESH))
            .collect();
        for r in recs {
            self.send(pid, gs::player_info(&r));
        }
    }

    /// `UP_PERF_REPORT`: sub 3 is echoed, sub 2 relayed to everyone (its ping feeds the ping table).
    // SPEC: GAME_MESSAGES_2009_C2S.md §5.8; DC-DEFAULT: DC-C2S-7 (proposed), DC-C2S-1 (proposed) sub 1 dropped
    fn on_perf(&mut self, pid: u8, p: UpPerf) {
        match p {
            UpPerf::Echo { t, words, .. } => self.send(pid, gs::perf_echo(pid as u32, t, words)),
            UpPerf::Stats { values, .. } => {
                if let Some(pl) = self.players.get_mut(&pid) {
                    pl.client_ping_ms = Some(values[1]);
                }
                self.broadcast(&gs::perf_relay(pid as u32, values));
            }
            UpPerf::Console { .. } | UpPerf::Other { .. } => {}
        }
    }

    /// `@player` per identified account, then `@result` (once).
    // SPEC: DSERVER_PIPE.md §4.4, §4.5, §5; DC-DEFAULT: DSV-7 (staged proposal), CD-54 (approved)
    fn report(&mut self, win: u8) {
        if self.reported {
            return;
        }
        self.reported = true;
        // DC-DEFAULT: DSV-472 (proposed) Spy Hunt names its winners per player (the lobby's team rule cannot: the
        // in-match teams are not the lobby's sides) and the agent who uploaded
        let spy = self.is_spy();
        let (spy_wins, agent_uid) = if spy {
            self.spy_winner_uids()
        } else {
            (std::collections::BTreeSet::new(), None)
        };
        let lines: Vec<String> = self
            .stats
            .values()
            .filter_map(|s| {
                let e = s.entry.as_ref()?;
                let flags: Vec<(&str, u32)> = if spy {
                    vec![
                        ("win", u32::from(spy_wins.contains(&e.uid))),
                        ("agent", u32::from(agent_uid == Some(e.uid))),
                    ]
                } else {
                    Vec::new()
                };
                Some(pipe::player_line_with(
                    e.uid,
                    s.kills,
                    s.deaths,
                    e.team,
                    s.team_kills,
                    s.headshots,
                    &flags,
                    &e.nick,
                ))
            })
            .collect();
        let any = !lines.is_empty();
        for l in lines {
            self.out.push(GameOut::Pipe(l));
        }
        if any {
            // DC-DEFAULT: DSV-149 (proposed) the lobby's sides after a switch; DSV-148 (proposed) FFA reports no team
            // winner (the lobby's win bonus is by team)
            let (win, scores) = self.lobby_sides(win, self.scores);
            let (_, rounds) = self.lobby_sides(0, self.round_wins);
            // DSV-472: Spy Hunt's winners are named per player above, no team win for the lobby's team rule
            let win = if self.is_ffa() || spy { 0 } else { win };
            let l = pipe::result_line(
                self.room.mode_key(),
                win,
                scores[0],
                scores[1],
                self.rules.goal as i64,
                (rounds[0], rounds[1]),
            );
            self.out.push(GameOut::Pipe(l));
        }
    }

    /// Reports an aborted match (quit, stdin EOF, idle): the scores so far, no winner.
    // DC-DEFAULT: DSV-18 (staged proposal) an aborted match is reported as a draw
    pub fn report_aborted(&mut self) {
        self.report(0);
    }

    /// Timers: introductions after the identify wait, the death timeline, the round timer, the ping table and the
    /// end-of-match holds.
    pub fn tick(&mut self, now: Millis) {
        self.tick_introductions(now);
        self.tick_lives(now);
        self.tick_possession(now);
        self.tick_coop(now);
        self.tick_projectiles(now);
        self.tick_hazards(now);
        self.tick_burns(now);
        self.tick_hisec(now);
        self.tick_drops(now);
        self.tick_packs(now);
        self.tick_destruct(now);
        self.tick_spawn_notes(now);
        self.tick_ctf(now);
        self.tick_bomb(now);
        self.tick_infect(now);
        self.tick_spy(now);
        self.tick_warmup(now);
        self.tick_round(now);
        self.tick_ping(now);
        self.tick_place(now);
        self.tick_remote_health();
        self.tick_team_pings(now);
    }

    fn tick_introductions(&mut self, now: Millis) {
        let late: Vec<u8> = self
            .players
            .values()
            .filter(|p| !p.introduced && !later(p.intro_deadline, now))
            .map(|p| p.pid)
            .collect();
        for pid in late {
            self.log(format!(
                "client {pid}: no player info in time, introduced anyway"
            ));
            self.introduce(pid, now);
        }
    }

    /// The start note of a spawn (0xa6 type 0x18 sub 0x1f, unreliable) to every introduced client, after the record
    /// refresh of the spawn; the end note follows after `spawn_note_ms`, to the clients that were sent the start note.
    // SPEC: GAME_MESSAGES_2009_S2C.md §8.3 (type 0x18 subs 0x1f / 0x25: no body; roster-bound objects only; they set
    // and clear object +0x539 / +0x53a with a tick stamp, PLAUSIBLY a spawn-protection window)
    // OBSERVED: capture 20261025_065016_tdm6 c5 / c6 (`0da618<obj>1f` unreliable at each of the
    // 8 spawns seen by both links, after the spawn's 0x11 record notes; `0da618<obj>25` reliable 3.47 .. 3.53 s later
    // on the same link, every end note with its start note; also at the join spawn: seq 219 / 519),
    // 20261025_002236_tdm4 c6 seq 212 / 615, 815 / 1396; REFERENCE, accepted by the real client
    // DC-DEFAULT: DSV-63 (proposed)
    fn spawn_note_start(&mut self, pid: u8, now: Millis) {
        let ms = self.cfg.spawn_note_ms;
        let Some(obj) = self
            .players
            .get(&pid)
            .filter(|p| p.character.is_some())
            .map(|p| p.obj)
        else {
            return;
        };
        if ms == 0 {
            return;
        }
        let m = gs::spawn_note(obj, gs::NOTE_SPAWN_START);
        let to = self.introduced();
        for &t in &to {
            self.out.push(GameOut::Unreliable(t, m.clone()));
        }
        self.spawn_notes.retain(|n| n.pid != pid);
        self.spawn_notes.push(SpawnNote {
            pid,
            obj,
            due: now.wrapping_add(ms),
            to,
        });
    }

    /// A client introduced while spawn-note windows are open gets their start notes (unreliable) and joins their
    /// recipients, so that every end note it gets had its start note. Without this, a client introduced in the tick
    /// of another player's spawn (after that spawn) or later in the window got the end note alone.
    // OBSERVED: capture 20261025_132219_ad-polish (our own stack, not the reference stack: c5 got
    // `0da6180048 25` at t 41.78 without the `1f` of t 38.28, both first spawns in one tick; the joiner c9 got the
    // `25` of t 69.34 without the `1f` of t 65.84, sent before it joined); 20261025_065016_tdm6 c5 / c6 (REFERENCE:
    // every `25` has its `1f` on the same link)
    // DC-DEFAULT: DSV-63 (amendment, proposed) the catch-up at the introduction is our rule
    fn spawn_notes_catch_up(&mut self, pid: u8) {
        let mut notes = Vec::new();
        for n in &mut self.spawn_notes {
            if !n.to.contains(&pid) {
                n.to.push(pid);
                notes.push(gs::spawn_note(n.obj, gs::NOTE_SPAWN_START));
            }
        }
        for m in notes {
            self.out.push(GameOut::Unreliable(pid, m));
        }
    }

    /// The end notes that are due (reliable), each to the clients that were sent its start note and are still
    /// introduced; a player who left gets none.
    // DC-DEFAULT: DSV-63 (proposed; amendment: recipients = the start note's)
    fn tick_spawn_notes(&mut self, now: Millis) {
        if self.spawn_notes.is_empty() {
            return;
        }
        let (due, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.spawn_notes)
            .into_iter()
            .partition(|n| !later(n.due, now));
        self.spawn_notes = keep;
        for n in due {
            self.send_spawn_note_end(n);
        }
    }

    /// The end note (0x25, reliable) of one window to its recipients that are still introduced; none when the
    /// player left or has another object.
    // DC-DEFAULT: DSV-63 (proposed; amendment: recipients = the start note's)
    fn send_spawn_note_end(&mut self, n: SpawnNote) {
        if !self.players.get(&n.pid).is_some_and(|p| p.obj == n.obj) {
            return;
        }
        let m = gs::spawn_note(n.obj, gs::NOTE_SPAWN_END);
        for to in n.to {
            if self.players.get(&to).is_some_and(|p| p.introduced) {
                self.send(to, m.clone());
            }
        }
    }

    /// A player's first shot (any UP_FIRE: bullet, swing or throw, refused or not) ends its own open spawn window
    /// at once: the end note goes out and the shield (DSV-156) drops. Not within [`SPAWN_FIRE_GRACE_MS`] of the
    /// spawn: a shot that was in flight when the player died (or sent while dead) can land just after the respawn
    /// and must not strip the new life's shield; a live shot cannot arrive before the client learnt of the spawn.
    // DC-DEFAULT: DSV-477 (proposed; project decision; amendment: the grace is ours)
    fn end_spawn_note_of(&mut self, pid: u8, now: Millis) {
        let fresh = self.players.get(&pid).is_some_and(|p| {
            p.life != Life::Alive || now.wrapping_sub(p.spawned_at) < SPAWN_FIRE_GRACE_MS
        });
        if fresh {
            return;
        }
        if let Some(i) = self.spawn_notes.iter().position(|n| n.pid == pid) {
            let n = self.spawn_notes.remove(i);
            self.send_spawn_note_end(n);
        }
    }

    /// The ping table to everyone, every period while the round runs: the transport's round-trip estimate of each
    /// link (the value the lobby's `GET /net` reports too), the client's own reported ping only while the link has
    /// none yet.
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.2 (0xa2), §10 step 13; DC-DEFAULT: PD-S2C-4 (proposed) every 5 s,
    // DSV-410 (proposed; amends DC-C2S-7's "feed 0xa2") the server-measured round trip first
    fn tick_ping(&mut self, now: Millis) {
        let Some(t) = self.next_ping else {
            return;
        };
        if later(t, now) || !matches!(self.phase, RoundPhase::Running { .. }) {
            return;
        }
        self.next_ping = Some(now.wrapping_add(self.cfg.ping_period_ms));
        let rows: Vec<(u8, u16)> = self
            .players
            .values()
            .filter(|p| p.introduced)
            .map(|p| (p.pid, p.link_rtt_ms.or(p.client_ping_ms).unwrap_or(0)))
            .collect();
        if !rows.is_empty() {
            self.broadcast(&gs::ping_table(&rows));
        }
    }
}

/// Shell 0x88 state the reference stack sent after `UP_WORLD_READY`.
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md O2 (`0d 88 03` after 0x87; REFERENCE); UDP_SESSION_2009_A.md §4.11
pub const REFERENCE_SWITCH_STATE_READY: u8 = 3;

/// Bye code for an identity mismatch (the same refusal as an unknown hello name).
// DC-DEFAULT: CD-60 (approved) refuse with bye code 6
pub const CD60_REFUSE_CODE: u32 = 6;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell64::Shell64Update;
    use crate::world::{synthetic_world_bytes, world_from_bytes, SpawnPoint};

    fn world() -> WorldInfo {
        let mut w = world_from_bytes(&synthetic_world_bytes(), "Worlds\\S.dat", "S").unwrap();
        w.spawns = vec![
            SpawnPoint {
                pos: [-300.0, 0.0, 0.0],
                rot: [0.0, 1.0, 0.0],
                team_filter: Some(b"Red".to_vec()),
            },
            SpawnPoint {
                pos: [300.0, 0.0, 0.0],
                rot: [0.0, 2.0, 0.0],
                team_filter: Some(b"blue".to_vec()),
            },
            SpawnPoint {
                pos: [0.0, 0.0, 300.0],
                rot: [0.0; 3],
                team_filter: None,
            },
        ];
        w
    }

    fn tables() -> RomTables {
        RomTables {
            teams: crate::rom_tables::TeamNames {
                names: [Some(b"RED".to_vec()), Some(b"Blue".to_vec())],
            },
            ..RomTables::default()
        }
    }

    fn entry(uid: u32, team: u8, nick: &str) -> RosterEntry {
        RosterEntry {
            uid,
            sid: 0,
            key: 0,
            team,
            exp: 10,
            nick: nick.into(),
            extra: vec![],
        }
    }

    fn ids(out: &[GameOut], pid: u8) -> Vec<u8> {
        out.iter()
            .filter_map(|o| match o {
                GameOut::To(p, m) if *p == pid => Some(m.as_bytes()[1]),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn keyed_spawn_pick_ignores_the_draw_order() {
        let keyed = GameTuning {
            spawn_pick: SpawnPick::Keyed,
            ..GameTuning::default()
        };
        let mut a = Game::new(keyed.clone(), &world(), RomTables::default());
        let mut b = Game::new(keyed, &world(), RomTables::default());
        let k = |n: &str, s| spawn_key(n.as_bytes(), 1, s);
        // a's order: x, y, x again; b's order: y twice first (another load order), then x
        let ax1 = a.choose_spawn_keyed(1, Some(k("Arena", 1)));
        let ay1 = a.choose_spawn_keyed(1, Some(k("bot1", 1)));
        let ax2 = a.choose_spawn_keyed(1, Some(k("arena", 2)));
        let by1 = b.choose_spawn_keyed(1, Some(k("BOT1", 1)));
        b.choose_spawn(1); // a stream draw in between changes nothing for the keyed picks
        let bx1 = b.choose_spawn_keyed(1, Some(k("arena", 1)));
        let bx2 = b.choose_spawn_keyed(1, Some(k("arena", 2)));
        assert_eq!((ax1, ay1, ax2), (bx1, by1, bx2));
        // the keys spread over the candidates
        let mut seen = std::collections::BTreeSet::new();
        for s in 1..40 {
            seen.insert(a.choose_spawn_keyed(1, Some(k("arena", s))).0[0] as i32);
        }
        assert_eq!(seen.len(), 3, "{seen:?}");
        assert_ne!(spawn_key(b"arena", 1, 1), spawn_key(b"arena", 2, 1));
    }

    #[test]
    fn team_spawn_choice() {
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        for _ in 0..10 {
            assert_eq!(g.choose_spawn(1).0, [-300.0, 0.0, 0.0]);
            assert_eq!(g.choose_spawn(2).0, [300.0, 0.0, 0.0]);
        }
        // without team names every point is a candidate
        let mut g = Game::new(GameTuning::default(), &world(), RomTables::default());
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..50 {
            seen.insert(g.choose_spawn(1).0[0] as i32);
        }
        assert_eq!(seen.len(), 3, "{seen:?}");
    }

    /// DSV-569: a spawn point the file locks is no candidate until a statement unlocks it; the statements move the
    /// open set (lock the first group, unlock the second); with every point locked any point is used; a new round
    /// restores the file's locks.
    #[test]
    fn locked_spawn_points_are_skipped_until_a_statement_unlocks_them() {
        let mut w = world();
        w.spawns = (0..6)
            .map(|i| SpawnPoint {
                pos: [100.0 * i as f32, 0.0, 0.0],
                rot: [0.0; 3],
                team_filter: Some(b"Red".to_vec()),
            })
            .collect();
        w.spawn_modes = vec![None; 6];
        w.spawn_dc = vec![false; 6];
        w.spawn_names = (0..6).map(|i| format!("Start{i}").into_bytes()).collect();
        // points 0, 1 open; 2, 3 locked (the next checkpoint); 4, 5 locked
        w.spawn_locked = vec![false, false, true, true, true, true];
        let mut g = Game::new(GameTuning::default(), &w, tables());
        let xs = |g: &mut Game| {
            (0..60)
                .map(|_| g.choose_spawn(1).0[0] as i32)
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert_eq!(xs(&mut g), [0, 100].into());
        for i in 0..2 {
            assert_eq!(g.set_spawn_lock(format!("start{i}").as_bytes(), true), 1);
        }
        assert_eq!(g.set_spawn_lock(b"START2", false), 1);
        assert_eq!(g.set_spawn_lock(b"Start3", false), 1);
        assert_eq!(g.set_spawn_lock(b"Start3", false), 0, "no change");
        assert_eq!(g.set_spawn_lock(b"NoSuchPoint", false), 0);
        assert_eq!(xs(&mut g), [200, 300].into());
        for i in 2..4 {
            g.set_spawn_lock(format!("Start{i}").as_bytes(), true);
        }
        assert_eq!(xs(&mut g).len(), 6, "every point locked: any point");
        g.reset_spawn_locks();
        assert_eq!(xs(&mut g), [0, 100].into());
    }

    fn spec_order() -> GameTuning {
        GameTuning {
            start_order: StartOrder::Spec,
            ..GameTuning::default()
        }
    }

    /// Synthetic character tables (made-up names in the ROM key shape) and a rig with the four player clips.
    fn character_tables() -> RomTables {
        use crate::chars::{BodyRow, CharTables, FaceRow};
        let mut t = tables();
        let doc = dc_bute::ButeDoc::parse(concat!(
            "[Weapon3]\nName = \"G3\"\nHHModel = \"HH\\\\g3.ltb\"\nHHSkin0 = \"HH\\\\g3.dtx\"\n",
            "HHRenderStyle0 = \"RS\\\\a.ltb\"\n",
            "[Weapon4]\nName = \"G4\"\nHHModel = \"HH\\\\g4.ltb\"\n",
            "[WeaponOrder]\nClass0 = \"P\"\nP0 = \"G4\"\nP1 = \"G3\"\n",
        ));
        t.weapons = crate::rom_tables::WeaponTable::from_doc(&doc);
        let mut chars = CharTables::default();
        chars.bodies.insert(
            0,
            BodyRow {
                model: "BODY\\b.ltb".into(),
                styles: vec!["RS\\a.ltb".into(), "RS\\b.ltb".into(), "RS\\c.ltb".into()],
                skins: [(0, (Some("BODY\\u.dtx".into()), Some("BODY\\l.dtx".into())))].into(),
                ..BodyRow::default()
            },
        );
        chars.faces.insert(
            0,
            FaceRow {
                model: "FACE\\f.ltb".into(),
                skins: vec!["FACE\\f.dtx".into()],
                styles: vec!["RS\\a.ltb".into(), "RS\\a.ltb".into()],
            },
        );
        chars.team_marks.insert(0, "BODY\\m.dtx".into());
        t.chars = chars;
        t
    }

    fn rig() -> crate::chars::Rig {
        use crate::chars::{ClipFact, Rig};
        let c = |key, len_ms| ClipFact {
            key,
            len_ms,
            extent: [15.0, 50.0, 15.0],
            footsteps: 0,
            upper: None,
        };
        Rig {
            children: vec!["BODY\\kid.ltb".into()],
            clips: vec![
                c(([1; 8], 4), 7200),
                c(crate::chars::CLIP_LOWER_STAND, 1000),
                c(crate::chars::CLIP_LOWER_WALK, 800),
                c(crate::chars::CLIP_UPPER_STAND, 1500),
                c(crate::chars::CLIP_UPPER_WALK, 900),
            ],
            own_clips: 1,
            grip_socket: 2,
            sockets: Vec::new(),
            stand_extent: Some([15.0, 50.0, 15.0]),
            tracker_masks: [None; 2],
        }
    }

    /// DSV-28 refinement: the motion of a control word (crouch first, then run, else walk; the direction bits with
    /// opposite bits cancelling) and the lower clip it picks, with the walk / stand fallbacks of a rig that lacks a
    /// set; `directional` off keeps the walk pair.
    #[test]
    fn control_word_motion_and_lower_clips() {
        use crate::chars::{
            CLIP_LOWER_CROUCH, CLIP_LOWER_CROUCH_STAND, CLIP_LOWER_RUN, CLIP_LOWER_WALK_DIRS,
        };
        let m = |c| Motion::from_control(c, true);
        assert_eq!(m(0), Motion::Stand);
        assert_eq!(m(0x0003), Motion::Stand, "forward and back cancel");
        assert_eq!(m(0x0a01), Motion::Step(Gait::Run, Dir::Forward));
        assert_eq!(m(0x400a01), Motion::Step(Gait::Run, Dir::Forward), "sprint");
        assert_eq!(m(0x0a81), Motion::Step(Gait::Run, Dir::ForwardLeft));
        assert_eq!(m(0x0b01), Motion::Step(Gait::Run, Dir::ForwardRight));
        assert_eq!(m(0x0a82), Motion::Step(Gait::Run, Dir::BackLeft));
        assert_eq!(m(0x0b02), Motion::Step(Gait::Run, Dir::BackRight));
        assert_eq!(m(0x0a80), Motion::Step(Gait::Run, Dir::Left));
        assert_eq!(m(0x0001), Motion::Step(Gait::Walk, Dir::Forward));
        assert_eq!(m(0x0102), Motion::Step(Gait::Walk, Dir::BackRight));
        assert_eq!(m(0x0020), Motion::Crouch);
        assert_eq!(
            m(0x0221),
            Motion::Step(Gait::Crouch, Dir::Forward),
            "crouch wins"
        );
        assert_eq!(Motion::from_control(0x0a01, false), Motion::Walk);
        assert_eq!(Motion::from_control(0x0020, false), Motion::Stand);
        // a rig with the full run set, two walk clips and the crouch stand, no crouch move clips
        let mut r = rig();
        let base = r.clips.len() as u16;
        let c = |key| crate::chars::ClipFact {
            key,
            len_ms: 500,
            ..Default::default()
        };
        r.clips.extend(CLIP_LOWER_RUN.iter().map(|k| c(*k)));
        r.clips.push(c(CLIP_LOWER_WALK_DIRS[1]));
        r.clips.push(c(CLIP_LOWER_CROUCH_STAND));
        let a = AnimSet::from_rig(&r);
        for (i, d) in Dir::ALL.iter().enumerate() {
            assert_eq!(a.lower_clip(Motion::Step(Gait::Run, *d)).0, base + i as u16);
        }
        assert_eq!(
            a.lower_clip(Motion::Step(Gait::Walk, Dir::Back)).0,
            base + 8
        );
        assert_eq!(
            a.lower_clip(Motion::Step(Gait::Walk, Dir::Forward)),
            a.lower[1]
        );
        assert_eq!(
            a.lower_clip(Motion::Step(Gait::Walk, Dir::Left)),
            a.lower[1],
            "fallback"
        );
        assert_eq!(a.lower_clip(Motion::Crouch).0, base + 9);
        assert_eq!(
            a.lower_clip(Motion::Step(Gait::Crouch, Dir::Forward)),
            a.lower[1],
            "no crouch clip: walk"
        );
        assert!(r.clip(CLIP_LOWER_CROUCH[0]).is_none());
        // the trackers: moving -> the upper walk clip, crouched still -> the upper stand clip
        let t = a.trackers(Motion::Step(Gait::Run, Dir::Forward), 0, 0);
        assert_eq!((t[1].anim, t[2].anim), (a.upper[1].0, base));
        let t = a.trackers(Motion::Crouch, 0, 0);
        assert_eq!((t[1].anim, t[2].anim), (a.upper[0].0, base + 9));
        // a rig without any of the new clips behaves as before
        let old = AnimSet::from_rig(&rig());
        assert_eq!(old.crouch, old.lower[0]);
        assert!(old.steps.iter().flatten().all(|x| *x == old.lower[1]));
    }

    /// Reference order (DSV-22): identify request, endpoint and settings first; after the player info the player
    /// spawns at once (0x6b, 0x6a, 0x67 state 1, maxima and reset, weapons and the 0x7f auto switch, body with the
    /// friction, teleport, health / armour) and the round messages follow; after 0x87: 0x90 / 0x88 and a second
    /// teleport. Other players stream the character with yaw8 and the animation trackers.
    #[test]
    fn reference_order_spawns_before_world_ready_and_streams_animations() {
        let mut g = Game::new(
            GameTuning {
                default_loadout: vec![3, 4],
                ..GameTuning::default()
            },
            &world(),
            character_tables(),
        );
        g.insert_rig("BODY\\b.ltb", rig());
        let e = entry(7, 0, "Dave");
        g.note_identified(&e);
        g.add_player(0, Some(&e), None, 1, [0.0; 3], [0.0; 3], 0);
        assert_eq!(ids(&g.take_out(), 0), vec![0x72, 0x9e, 0x9d, 0x7d]);
        let c = g.players[&0].character.clone().expect("a character");
        assert_eq!(c.weapon.as_ref().map(|w| w.weapon), Some(3));
        assert_eq!(c.assets.body.len(), 2, "body + child");
        assert_eq!(c.anims.lower, [(1, 1000), (2, 800)]);
        assert_eq!(c.anims.upper, [(3, 1500), (4, 900)]);
        g.on_up(0, Up::PlayerInfo(Box::default()), 10);
        let o = g.take_out();
        assert_eq!(
            ids(&o, 0),
            vec![
                0x6b, 0x6a, 0x67, 0x66, 0x66, 0x66, 0xb2, 0xb2, 0x7f, 0xa0, 0xa0, 0x66, 0x66, 0x6b,
                0x43, 0x44, 0xb5, 0x45, 0xc3
            ]
        );
        assert!(o
            .iter()
            .any(|x| matches!(x, GameOut::Spawned { pid: 0, .. })));
        // the 0x7f names weapon 3 by its command (position 1 of the command table) with auto 1
        let sw = o
            .iter()
            .find_map(|x| match x {
                GameOut::To(0, m) if m.as_bytes()[1] == 0x7f => Some(m.as_bytes()[2..4].to_vec()),
                _ => None,
            })
            .unwrap();
        assert_eq!(sw, vec![0x15, 1]);
        assert_eq!(g.players[&0].life, Life::Alive);
        assert!(
            !g.players[&0].visible,
            "visible only once the client reported the world ready"
        );
        assert!(!o.iter().any(|x| matches!(x, GameOut::Visible { .. })));
        g.on_up(0, Up::WorldReady, 20);
        let o = g.take_out();
        assert_eq!(ids(&o, 0), vec![0x90, 0x88, 0xa0, 0x66, 0x66, 0xa0]);
        assert!(o
            .iter()
            .any(|x| matches!(x, GameOut::Visible { pid: 0, .. })));
        assert!(g.players[&0].visible);
        assert_eq!(g.players[&0].move_seq, 2);
        // a respawn in the reference stack order sends the body again (0xa0 0x0009 = body + friction) before the teleport
        g.kill(0, None, 3, false, 100);
        g.tick(100 + DYING_MS);
        g.take_out();
        g.tick(100 + CLIENT_RESPAWN_WAIT_MS);
        let o = g.take_out();
        let movers: Vec<u16> = o
            .iter()
            .filter_map(|x| match x {
                GameOut::To(0, m) if m.as_bytes()[1] == 0xa0 => {
                    Some(u16::from_le_bytes([m.as_bytes()[2], m.as_bytes()[3]]))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            movers,
            vec![0x0009, 0x0020],
            "respawn: body + friction, then the teleport"
        );
        assert!(o
            .iter()
            .any(|x| matches!(x, GameOut::Spawned { first: false, .. })));
        assert!(
            g.players[&0].visible,
            "visible again at once (already ready)"
        );
        // a second player sees the first one as a character entry: yaw8 + three trackers
        let e2 = entry(8, 1, "Erin");
        g.add_player(1, Some(&e2), None, 3, [0.0; 3], [0.0; 3], 30);
        let s = g.stream_for(1, 100 + CLIENT_RESPAWN_WAIT_MS + 1020);
        let mine = s.iter().find(|x| x.obj == 1).unwrap();
        assert!(mine.yaw8.is_some() && mine.rot.is_none());
        let a = mine.anim.unwrap();
        assert_eq!((a[1].anim, a[2].anim), (3, 1), "standing pair");
        assert_eq!(
            a[0].time_ms, 1020,
            "main tracker: ms since the respawn (clip 7200 ms)"
        );
    }

    /// DSV-44 amendment: from the death to the respawn the dead character's stream entry carries the corpse's death
    /// clip on the main tracker (state bits 1 / 0, time = ms since the death held at the clip's end, as the reference stack's
    /// tdm4 seq 3135 .. 3510), the body trackers unchanged; the respawn starts the trackers again.
    #[test]
    fn dead_character_streams_the_corpse_clip_until_the_respawn() {
        let mut g = Game::new(GameTuning::default(), &world(), character_tables());
        let mut r = rig();
        for k in [
            crate::chars::CLIP_DEATH_BODY_LEFT,
            crate::chars::CLIP_DEATH_BODY_RIGHT,
            crate::chars::CLIP_DEATH_HEAD_LEFT,
            crate::chars::CLIP_DEATH_HEAD_RIGHT,
        ] {
            r.clips.push(crate::chars::ClipFact {
                key: k,
                len_ms: 1333,
                extent: [15.0, 50.0, 15.0],
                footsteps: 0,
                upper: None,
            });
        }
        g.insert_rig("BODY\\b.ltb", r);
        let (e0, e1) = (entry(7, 0, "Dave"), entry(8, 1, "Erin"));
        g.note_identified(&e0);
        g.note_identified(&e1);
        g.add_player(0, Some(&e0), None, 1, [0.0; 3], [0.0; 3], 0);
        g.add_player(1, Some(&e1), None, 3, [0.0; 3], [0.0; 3], 0);
        for pid in [0, 1] {
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        g.take_out();
        let main_of = |g: &Game, t| {
            let s = g.stream_for(1, t);
            s.iter().find(|x| x.obj == 1).and_then(|x| x.anim).unwrap()
        };
        let alive = main_of(&g, 90);
        assert!(alive[0].state1, "a living character loops its main clip");
        let die = 100;
        g.kill(0, Some(1), 3, false, die);
        let clip = g.players[&0].death_clip.expect("the corpse's clip is kept");
        assert!(
            usize::from(clip.0) >= 5,
            "a death clip of the rig: {clip:?}"
        );
        let corpse = g
            .take_out()
            .into_iter()
            .find_map(|o| match o {
                GameOut::Died { clip, .. } => Some(clip),
                _ => None,
            })
            .expect("the corpse");
        assert_eq!(
            corpse, clip,
            "the corpse's clip and the character's are one"
        );
        for (t, want) in [(0, 0), (480, 480), (1200, 1200), (3000, 1333), (6900, 1333)] {
            let a = main_of(&g, die + t);
            assert_eq!(
                (
                    a[0].anim,
                    a[0].state0,
                    a[0].state1,
                    a[0].time_ms,
                    a[0].len_ms,
                    a[0].secondary
                ),
                (clip.0, true, false, want, clip.1, None),
                "{t} ms after the death"
            );
            assert_eq!(
                (a[1].anim, a[2].anim),
                (alive[1].anim, alive[2].anim),
                "body trackers kept"
            );
        }
        g.tick(die + DYING_MS);
        g.take_out();
        let back = die + CLIENT_RESPAWN_WAIT_MS;
        g.tick(back);
        assert_eq!(g.players[&0].life, Life::Alive);
        assert_eq!(g.players[&0].death_clip, None);
        let a = main_of(&g, back + 40);
        assert_eq!(
            (a[0].anim, a[0].state1, a[0].time_ms),
            (0, true, 40),
            "trackers from the respawn on"
        );
    }

    /// DSV-29 amendment: the shot effect's surface byte is the ROM head row (4) for a hit in the head zone of a
    /// player, the body row (6) for any other player hit, the world polygon's surface for a world hit (the fallback 71
    /// where no polygon is near, as in the synthetic world) and the fallback for a miss.
    #[test]
    fn shot_surface_by_what_was_hit() {
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        g.add_player(0, None, None, 1, [0.0; 3], [0.0; 3], 0);
        g.add_player(1, None, None, 3, [0.0; 3], [0.0; 3], 0);
        let victim = g.players[&1].clone();
        let d = g.dims_of(1);
        let f = crate::up::UpFire::default();
        let at = |dy: f32| [victim.pos[0], victim.pos[1] + dy, victim.pos[2]];
        const DOWN: [f32; 3] = [0.0, -1.0, 0.0];
        assert_eq!(
            g.shot_surface(0, Some(victim.obj), at(d[1] - 1.0), DOWN, &f),
            gs::SURFACE_HEAD
        );
        assert_eq!(
            g.shot_surface(0, Some(victim.obj), at(0.0), DOWN, &f),
            gs::SURFACE_BODY
        );
        assert_eq!(
            g.shot_surface(0, Some(0), at(0.0), DOWN, &f),
            gs::TODO_SURFACE_WORLD
        );
        assert_eq!(
            g.shot_surface(0, None, at(0.0), DOWN, &f),
            gs::TODO_SURFACE_WORLD
        );
        assert_eq!(
            g.shot_surface(0, Some(999), at(0.0), DOWN, &f),
            gs::SURFACE_BODY,
            "another object"
        );
        // the byte lands at offset 10 of the shell message (after id, type, first, shooter, weapon, ammo)
        let m = gs::shot_effect(&gs::ShotEffect {
            first: Some(victim.obj),
            shooter: 2,
            weapon: 145,
            ammo: 145,
            from: [0.0; 3],
            to: [0.0; 3],
            normal: [-1.0, 0.0, 0.0],
            surface: gs::SURFACE_BODY,
            mask: gs::SHOT_MASK_HIT,
            client: 0,
            mods: gs::NO_SHOT_MODS,
        });
        assert_eq!(m.as_bytes()[1], gs::FX_SHOT);
        assert_eq!(m.as_bytes()[10], 6);
    }

    /// CV2-5: a runaway yaw (the 2009 client's unwrapped accumulator) is stored folded and repaired with 0x6a kind 0
    /// at most once per interval.
    #[test]
    fn runaway_yaw_is_folded_and_repaired() {
        let mut g = Game::new(
            GameTuning {
                move_check: false,
                ..GameTuning::default()
            },
            &world(),
            tables(),
        );
        g.add_player(0, None, None, 1, [0.0; 3], [0.0; 3], 0);
        g.on_up(0, Up::PlayerInfo(Box::default()), 10);
        g.take_out();
        let seq = g.players[&0].move_seq;
        let upd = |yaw: f32| Shell64Update {
            view: Some([0.0, yaw, 0.0]),
            wide: Some([0.25f32.to_bits(), 0, 0, 0, 0, 0]),
            state: Some(0),
            trailer: Some(crate::shell64::Trailer {
                a: seq,
                vec_b: [-300.0, 0.0, 0.0],
                vec_c: [0.0; 3],
                t: 1.0,
                d: 1,
                e: 0,
                f: 0,
            }),
            complete: true,
            ..Shell64Update::default()
        };
        g.on_update(0, &upd(2_426_655.0), 100);
        let y = g.players[&0].yaw;
        assert!(y.abs() <= std::f32::consts::PI && (y - fold_yaw(2_426_655.0)).abs() < 1e-6);
        let o = g.take_out();
        assert_eq!(ids(&o, 0), vec![0x6a], "repair");
        g.on_update(0, &upd(2_426_655.0), 200);
        assert!(
            ids(&g.take_out(), 0).is_empty(),
            "at most once per interval"
        );
        g.on_update(0, &upd(2_426_655.0), 100 + VIEW_REPAIR_INTERVAL_MS);
        assert_eq!(ids(&g.take_out(), 0), vec![0x6a]);
        g.on_update(0, &upd(7.0), 5_000);
        assert!(
            ids(&g.take_out(), 0).is_empty(),
            "an ordinary unwrapped yaw is only folded"
        );
        assert!((g.players[&0].yaw - (7.0 - std::f32::consts::TAU)).abs() < 1e-5);
        assert_eq!(g.players[&0].view_repairs, 2);
    }

    #[test]
    fn spawn_set_order_after_world_ready() {
        let mut g = Game::new(
            GameTuning {
                default_loadout: vec![3, 4],
                ..spec_order()
            },
            &world(),
            tables(),
        );
        let e = entry(7, 0, "Dave");
        g.note_identified(&e);
        g.add_player(0, Some(&e), None, 1, [0.0; 3], [0.0; 3], 0);
        assert_eq!(ids(&g.take_out(), 0), vec![0x72]);
        g.on_up(0, Up::PlayerInfo(Box::default()), 10);
        let o = g.take_out();
        assert_eq!(ids(&o, 0), vec![0x6b, 0x7d, 0x67]);
        g.on_up(0, Up::WorldReady, 20);
        let o = g.take_out();
        assert_eq!(
            ids(&o, 0),
            vec![0x45, 0xb5, 0xc3, 0xa0, 0x6a, 0xb2, 0xb3, 0x66, 0x66, 0x66, 0x66, 0x67, 0x6b]
        );
        assert_eq!(g.players[&0].life, Life::Alive);
        assert_eq!(g.players[&0].pos, [-300.0, 0.0, 0.0]);
        assert!(matches!(g.phase(), RoundPhase::Running { start: 20 }));
    }

    /// DSV-410: the ping table carries the server-measured round trip; the client's reported ping only while the
    /// link has none.
    #[test]
    fn the_ping_table_prefers_the_measured_round_trip() {
        let mut g = Game::new(spec_order(), &world(), tables());
        let e = entry(7, 0, "Dave");
        g.note_identified(&e);
        g.add_player(0, Some(&e), None, 1, [0.0; 3], [0.0; 3], 0);
        g.on_up(0, Up::PlayerInfo(Box::default()), 10);
        g.on_up(0, Up::WorldReady, 20);
        g.take_out();
        let table = |o: &[GameOut]| -> Vec<(u16, u16)> {
            o.iter()
                .find_map(|x| match x {
                    GameOut::To(0, m) if m.as_bytes()[1] == gs::GS_PING_TABLE => Some(m.reader()),
                    _ => None,
                })
                .map(|mut r| {
                    r.read_u8();
                    r.read_u8();
                    let mut v = vec![];
                    loop {
                        let pid = r.read_u16();
                        if pid == gs::PING_TABLE_END {
                            break v;
                        }
                        v.push((pid, r.read_u16()));
                    }
                })
                .expect("a ping table")
        };
        g.players.get_mut(&0).unwrap().client_ping_ms = Some(300);
        g.tick(20);
        assert_eq!(table(&g.take_out()), vec![(0, 300)]);
        g.set_link_rtt(0, 42);
        g.tick(20 + PING_PERIOD_MS);
        assert_eq!(table(&g.take_out()), vec![(0, 42)]);
    }

    /// DSV-480: a culled match sends no other player's health (its output is the same as before); a match without
    /// culling sends every player's health to each client once and then each change.
    #[test]
    fn remote_health_goes_only_to_unculled_matches() {
        let run = |on: bool| {
            let mut g = Game::new(spec_order(), &world(), tables());
            g.set_remote_health(on);
            let mut out = Vec::new();
            for (pid, uid, nick) in [(0u8, 7u32, "Dave"), (1, 8, "Erin")] {
                let e = entry(uid, 0, nick);
                g.note_identified(&e);
                g.add_player(pid, Some(&e), None, 1 + pid as u16, [0.0; 3], [0.0; 3], 0);
                g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
                g.on_up(pid, Up::WorldReady, 20);
            }
            out.extend(g.take_out());
            g.tick(30);
            out.extend(g.take_out());
            g.players.get_mut(&1).unwrap().health = 39.5;
            g.tick(40);
            out.extend(g.take_out());
            g.tick(50);
            out.extend(g.take_out());
            out
        };
        let hp = |o: &GameOut| match o {
            GameOut::To(p, m) if m.as_bytes()[1] == dc_wire::remotehealth::SHELL_ID => {
                let mut r = m.reader();
                r.read_u8();
                r.read_u8();
                Some((*p, dc_wire::remotehealth::read_body(&mut r).unwrap()))
            }
            _ => None,
        };
        let off = run(false);
        let on = run(true);
        assert!(off.iter().all(|o| hp(o).is_none()));
        let rest: Vec<GameOut> = on.iter().filter(|o| hp(o).is_none()).cloned().collect();
        assert_eq!(rest, off, "the other messages are unchanged");
        let e = |pid, health| dc_wire::remotehealth::HealthEntry {
            pid,
            health,
            max: 100,
        };
        let sent: Vec<_> = on.iter().filter_map(hp).collect();
        assert_eq!(
            sent,
            vec![
                (0, vec![e(0, 100), e(1, 100)]),
                (1, vec![e(0, 100), e(1, 100)]),
                (0, vec![e(1, 40)]),
                (1, vec![e(1, 40)]),
            ]
        );
    }

    /// The 0xc6 events player 0 got: (event, flag team, carrier, actor, at_base).
    fn flag_events(o: &[GameOut]) -> Vec<(u32, u32, u32, u32, bool)> {
        o.iter()
            .filter_map(|x| match x {
                GameOut::To(0, m) if m.as_bytes()[1] == gs::GS_OBJECTIVE_STATE => {
                    let mut r = m.reader();
                    r.read_u8();
                    r.read_u8();
                    r.read_u16();
                    r.read_u32();
                    let (t, c, a, e) = (r.read_u32(), r.read_u32(), r.read_u32(), r.read_u32());
                    Some((e, t, c, a, r.read_bit()))
                }
                _ => None,
            })
            .collect()
    }

    /// DSV-190..DSV-193: a flag room (class 7, goal 2): the enemy flag is taken by touch, captured at the own base
    /// while the own flag is home (0xc6 event 1, 0xb5 with the captures), a dead carrier drops it, a team mate's touch
    /// returns it, the second capture ends the match; kills do not count for the goal.
    #[test]
    fn capture_the_flag_take_capture_drop_return_and_goal() {
        use super::ctf::{EVENT_CAPTURED, EVENT_DROPPED, EVENT_RETURNED, EVENT_TAKEN, NOBODY};
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "7".into());
        room.pairs.insert("goal".into(), "2".into());
        g.set_room(&room);
        g.bind_flags(&[(900, 1, [-1000.0, 0.0, 0.0]), (901, 2, [1000.0, 0.0, 0.0])]);
        for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3)] {
            let e = entry(uid, team, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        assert_eq!((g.players[&0].team, g.players[&1].team), (1, 2));
        g.take_out();
        let at =
            |g: &mut Game, pid: u8, x: f32| g.players.get_mut(&pid).unwrap().pos = [x, 0.0, 0.0];
        // take the enemy flag
        at(&mut g, 0, 990.0);
        g.tick(100);
        let o = g.take_out();
        assert_eq!(flag_events(&o), vec![(EVENT_TAKEN, 2, 0, 0, false)]);
        assert_eq!(g.ctf().flags[1].carrier, Some(0));
        // DSV-217: the flag goes onto its carrier
        assert!(o.contains(&GameOut::FlagCarry {
            obj: 901,
            team: 2,
            carrier: Some(0),
            before: None
        }));
        // it follows the carrier
        at(&mut g, 0, 500.0);
        g.tick(300);
        assert!(g
            .take_out()
            .iter()
            .any(|x| matches!(x, GameOut::FlagMove { obj: 901, pos } if pos[0] == 500.0)));
        // capture at the own base
        at(&mut g, 0, -1000.0);
        g.tick(500);
        let o = g.take_out();
        assert_eq!(flag_events(&o), vec![(EVENT_CAPTURED, 2, 0, 0, true)]);
        assert_eq!(g.ctf().captures, [1, 0]);
        // DSV-217: and off him at the capture
        assert!(o.contains(&GameOut::FlagCarry {
            obj: 901,
            team: 2,
            carrier: None,
            before: Some(0)
        }));
        assert_eq!(g.team_counters(), [1, 0]);
        assert!(o
            .iter()
            .any(|x| matches!(x, GameOut::To(0, m) if m.as_bytes()[1] == gs::GS_ROUND_WINS)));
        // DSV-253: the capturer's board mission values (v68 = captures) go to everyone in a 0x6b refresh
        assert_eq!(g.flag_mission_of(0), [0, 1, 0]);
        assert!(o.iter().any(
            |x| matches!(x, GameOut::To(1, m) if m.as_bytes()[1] == gs::GS_PLAYER_INFO
            && m.as_bytes().len() > 2)
        ));
        assert!(g.ctf().flags[1].at_base && g.ctf().flags[1].pos == [1000.0, 0.0, 0.0]);
        // the enemy takes our flag and dies with it: dropped where he died
        at(&mut g, 0, 0.0);
        at(&mut g, 1, -1000.0);
        g.tick(700);
        g.take_out();
        at(&mut g, 1, -600.0);
        g.kill(1, Some(0), 3, false, 800);
        g.tick(900);
        let o = g.take_out();
        assert_eq!(flag_events(&o), vec![(EVENT_DROPPED, 1, NOBODY, 1, false)]);
        assert!(o.contains(&GameOut::FlagCarry {
            obj: 900,
            team: 1,
            carrier: None,
            before: Some(1)
        }));
        // DSV-252: at the flag's height above the floor (no trace world here: the feet + FLAG_LIFT), not at his
        // centre
        let half = g.dims_of(1)[1];
        assert!(half > super::ctf::FLAG_LIFT);
        assert_eq!(
            g.ctf().flags[0].pos,
            [-600.0, super::ctf::FLAG_LIFT - half, 0.0]
        );
        assert_eq!(g.ctf().captures, [1, 0], "a kill is no capture");
        // our player returns it by touch
        at(&mut g, 0, -620.0);
        g.tick(1000);
        let o = g.take_out();
        assert_eq!(flag_events(&o), vec![(EVENT_RETURNED, 1, NOBODY, 0, true)]);
        assert!(g.ctf().flags[0].at_base);
        // DSV-253: v6c = his returns
        assert_eq!(g.flag_mission_of(0), [0, 1, 1]);
        assert_eq!(g.flag_mission_of(1), [0, 0, 0]);
        // the second capture ends the match
        at(&mut g, 0, 1000.0);
        g.tick(1100);
        at(&mut g, 0, -1000.0);
        g.tick(1200);
        assert_eq!(g.ctf().captures, [2, 0]);
        assert!(matches!(g.phase(), RoundPhase::Over { win: 1, .. }));
    }

    /// DSV-192 (amendment 2026-11-06): a living player near a flag he is not handed is logged once per life and flag,
    /// with both distances and the reason; nobody farther than twice the touch distances is logged.
    #[test]
    fn a_player_near_a_flag_he_is_not_handed_is_logged_once_per_life() {
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "7".into());
        room.pairs.insert("goal".into(), "3".into());
        g.set_room(&room);
        g.bind_flags(&[(900, 1, [-1000.0, 0.0, 0.0]), (901, 2, [1000.0, 0.0, 0.0])]);
        for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3)] {
            let e = entry(uid, team, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        g.take_out();
        let notes = |o: &[GameOut]| -> Vec<String> {
            o.iter()
                .filter_map(|x| match x {
                    GameOut::Log(l) if l.contains(" near the team ") => Some(l.clone()),
                    _ => None,
                })
                .collect()
        };
        let at = |g: &mut Game, pid: u8, p: [f32; 3]| g.players.get_mut(&pid).unwrap().pos = p;
        at(&mut g, 1, [0.0, 0.0, 3000.0]);
        // far away: nothing
        at(&mut g, 0, [700.0, 0.0, 0.0]);
        g.tick(100);
        assert!(notes(&g.take_out()).is_empty());
        // 100 across, 30 up from the enemy flag: outside the touch, logged once
        at(&mut g, 0, [900.0, 30.0, 0.0]);
        g.tick(200);
        assert_eq!(
            notes(&g.take_out()),
            vec![
                "ctf: client 0 (team 1) near the team 2 flag (at its base): 100 across, +30 up, not taken: \
                 outside the touch"
                    .to_string()
            ]
        );
        at(&mut g, 0, [890.0, 30.0, 0.0]);
        g.tick(300);
        assert!(notes(&g.take_out()).is_empty(), "once per life");
        // on the own flag at its base: logged with that reason
        at(&mut g, 0, [-1000.0, 20.0, 10.0]);
        g.tick(400);
        let n = notes(&g.take_out());
        assert_eq!(n.len(), 1, "{n:?}");
        assert!(n[0].contains(
            "team 1 flag (at its base): 10 across, +20 up, not taken: his own flag, at its base"
        ));
        // a new life logs again; on the enemy flag he takes it and nothing is logged for it
        g.players.get_mut(&0).unwrap().spawns += 1;
        at(&mut g, 0, [1000.0, 0.0, 0.0]);
        g.tick(500);
        let o = g.take_out();
        assert_eq!(g.ctf().flags[1].carrier, Some(0));
        assert!(notes(&o).is_empty(), "{:?}", notes(&o));
    }

    /// DSV-252: over a floor a dropped flag stands as high above the floor as at its base (the world file's flag
    /// objects sit 18..42 units above the floor), not at the carrier's centre.
    #[test]
    fn a_dropped_flag_stands_above_the_floor_like_at_its_base() {
        use dc_sim::{CollModel, CollisionWorld, ConvexPoly, SURF_SOLID, V3};
        let floor = ConvexPoly::from_corners(
            vec![
                V3::new(-3000.0, -100.0, -3000.0),
                V3::new(-3000.0, -100.0, 3000.0),
                V3::new(3000.0, -100.0, 3000.0),
                V3::new(3000.0, -100.0, -3000.0),
            ],
            SURF_SOLID,
        );
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        g.set_trace_world(Some(CollisionWorld::from_parts(
            vec![CollModel::from_polys(std::slice::from_ref(&floor))],
            &[],
        )));
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "7".into());
        g.set_room(&room);
        g.bind_flags(&[
            (900, 1, [-1000.0, -70.0, 0.0]),
            (901, 2, [1000.0, -70.0, 0.0]),
        ]);
        assert!(
            (g.ctf().flags[0].lift - 30.0).abs() < 0.5,
            "measured at the base"
        );
        for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3)] {
            let e = entry(uid, team, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        g.players.get_mut(&1).unwrap().pos = [-1000.0, -50.0, 0.0];
        g.tick(100);
        assert_eq!(g.ctf().flags[0].carrier, Some(1));
        g.players.get_mut(&1).unwrap().pos = [-400.0, -50.0, 0.0];
        g.kill(1, Some(0), 3, false, 200);
        g.tick(300);
        let p = g.ctf().flags[0].pos;
        assert_eq!([p[0], p[2]], [-400.0, 0.0]);
        assert!((p[1] - -70.0).abs() < 0.5, "30 above the floor: {p:?}");
    }

    /// DSV-192: a dropped flag nobody touches goes home after the return time.
    #[test]
    fn a_dropped_flag_goes_home_after_the_timer() {
        use super::ctf::{EVENT_RETURNED, NOBODY, RETURN_MS};
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "7".into());
        g.set_room(&room);
        g.bind_flags(&[(900, 1, [-1000.0, 0.0, 0.0]), (901, 2, [1000.0, 0.0, 0.0])]);
        for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3)] {
            let e = entry(uid, team, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        g.players.get_mut(&1).unwrap().pos = [-1000.0, 0.0, 0.0];
        g.players.get_mut(&0).unwrap().pos = [0.0, 500.0, 0.0];
        g.tick(100);
        g.kill(1, None, 3, false, 200);
        g.tick(300);
        // DSV-290 the drop starts the countdown note, then each whole second changes it, 0 at the return
        let notes = |o: &[GameOut]| -> Vec<(u16, u32)> {
            o.iter()
                .filter_map(|x| match x {
                    GameOut::FlagNote { obj, remaining } => Some((*obj, *remaining)),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(notes(&g.take_out()), vec![(900, 30)]);
        g.tick(300 + 1000);
        assert_eq!(notes(&g.take_out()), vec![(900, 29)]);
        g.tick(300 + RETURN_MS - 1);
        let o = g.take_out();
        assert!(flag_events(&o).is_empty());
        assert_eq!(notes(&o), vec![(900, 1)]);
        g.tick(300 + RETURN_MS);
        let o = g.take_out();
        assert_eq!(
            flag_events(&o),
            vec![(EVENT_RETURNED, 1, NOBODY, NOBODY, true)]
        );
        assert_eq!(notes(&o), vec![(900, 0)]);
    }

    /// DSV-291: the clan flag class (8) is played in two halves: the capture that brings a team to half the goal ends
    /// the first half with a round result carrying the captures (a counter below the goal, so the client draws its
    /// half-time picture), the flags go home, the sides switch with the captures and the second half plays to the
    /// goal; class 7 never splits.
    #[test]
    fn the_clan_flag_class_plays_two_halves_and_switches_sides() {
        use super::ctf::EVENT_CAPTURED;
        for (class, halves) in [("8", true), ("7", false)] {
            let mut g = Game::new(GameTuning::default(), &world(), tables());
            let mut room = RoomSettings::default();
            room.pairs.insert("type".into(), class.into());
            room.pairs.insert("goal".into(), "2".into());
            g.set_room(&room);
            g.bind_flags(&[(900, 1, [-1000.0, 0.0, 0.0]), (901, 2, [1000.0, 0.0, 0.0])]);
            for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3)] {
                let e = entry(uid, team, "P");
                g.note_identified(&e);
                g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
                g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
                g.on_up(pid, Up::WorldReady, 20);
            }
            g.take_out();
            let at = |g: &mut Game, x: f32| g.players.get_mut(&0).unwrap().pos = [x, 0.0, 0.0];
            at(&mut g, 990.0);
            g.tick(100);
            at(&mut g, -1000.0);
            g.tick(200);
            let o = g.take_out();
            assert_eq!(g.ctf().captures[0] + g.ctf().captures[1], 1);
            assert!(flag_events(&o).iter().any(|e| e.0 == EVENT_CAPTURED));
            let result: Vec<[u32; 2]> = o
                .iter()
                .filter_map(|x| match x {
                    GameOut::To(0, m) if m.as_bytes()[1] == gs::GS_ROUND_RESULT => {
                        let mut r = m.reader();
                        r.read_u8();
                        r.read_u8();
                        r.read_u32();
                        r.read_u8();
                        Some([r.read_u8() as u32, r.read_u8() as u32])
                    }
                    _ => None,
                })
                .collect();
            if !halves {
                assert!(result.is_empty(), "class 7: no half-time result");
                assert!(!g.modes.half_done);
                continue;
            }
            assert!(
                g.modes.half_done && g.modes.swap_pending,
                "class 8: first half over"
            );
            assert!(matches!(g.phase(), RoundPhase::Over { last: false, .. }));
            assert_eq!(result.len(), 1, "one round result for player 0");
            assert_eq!(result[0].iter().sum::<u32>(), 1, "it carries the captures");
            assert!(g.ctf().flags.iter().all(|f| f.at_base), "the flags go home");
            // the break, the switch and the second half
            let before = g.ctf().captures;
            let mut t = 200;
            while t < 200 + 60_000 && !matches!(g.phase(), RoundPhase::Running { .. }) {
                t += 50;
                g.tick(t);
            }
            assert!(matches!(g.phase(), RoundPhase::Running { .. }));
            assert!(g.modes.swapped);
            assert_eq!(
                g.ctf().captures,
                [before[1], before[0]],
                "the captures follow the sides"
            );
            assert_eq!((g.players[&0].team, g.players[&1].team), (2, 1));
            assert_eq!(g.round_no(), 2);
        }
    }

    /// A Search and Destroy room (class 2, goal 2) with one attacker (client 0) and one defender (client 1) and a
    /// bomb site round x = 1000.
    fn bomb_room() -> Game {
        // the round tests start at round 1: no warm-up (DSV-512 has its own tests)
        bomb_room_with(GameTuning {
            warmup_secs: Some(0),
            ..GameTuning::default()
        })
    }

    fn bomb_room_with(cfg: GameTuning) -> Game {
        let mut g = Game::new(cfg, &world(), tables());
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "2".into());
        room.pairs.insert("goal".into(), "2".into());
        g.set_room(&room);
        g.bomb.sites = vec![super::bomb::BombSite {
            number: 1,
            min: [900.0, -100.0, -100.0],
            max: [1100.0, 100.0, 100.0],
        }];
        for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3)] {
            let e = entry(uid, team, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        assert_eq!((g.players[&0].team, g.players[&1].team), (1, 2));
        g
    }

    /// DSV-512: a bomb match opens with a warm-up (the room's `train`, at least 20 s): the clock shows its time and
    /// the 0x45 the training banner; no carrier, no elimination end, no counted kill; then round 1 with the scores
    /// cleared, everyone spawned again, the banner off and the bomb carrier chosen.
    #[test]
    fn a_bomb_match_opens_with_a_warmup_where_nothing_counts() {
        use super::warmup::WARMUP_MIN_SECS;
        let mut g = bomb_room_with(GameTuning::default());
        assert_eq!(
            g.warmup_secs(),
            WARMUP_MIN_SECS,
            "no train key: the minimum"
        );
        g.room.pairs.insert("train".into(), "45".into());
        assert_eq!(g.warmup_secs(), 45);
        g.room.pairs.insert("train".into(), "10".into());
        assert_eq!(
            g.warmup_secs(),
            WARMUP_MIN_SECS,
            "the ROM's 10 s: the minimum"
        );
        assert!(g.in_warmup(), "the round start opened the warm-up");
        let o = g.take_out();
        // the 0x45 of the first spawn carries the banner flag
        let mb: Vec<&BitWriter> = o
            .iter()
            .filter_map(|x| match x {
                GameOut::To(0, m) if m.as_bytes()[1] == gs::GS_MATCH_BEGIN => Some(m),
                _ => None,
            })
            .collect();
        assert!(!mb.is_empty());
        let banner = |m: &BitWriter| {
            let mut r = m.reader();
            r.skip_bits(16);
            r.read_u32();
            r.read_bit(); // intro
            r.read_bit()
        };
        assert!(mb.iter().all(|m| banner(m)), "training banner on");
        g.tick(100);
        g.tick(5_000);
        assert!(g.bomb.carriers.is_empty(), "no bomb in the warm-up");
        // a kill in the warm-up: no round end, nothing counted
        g.kill(1, Some(0), 10, false, 6_000);
        g.tick(6_100);
        assert!(
            matches!(g.phase(), RoundPhase::Running { .. }),
            "no elimination"
        );
        assert_eq!(g.round_wins, [0, 0]);
        g.take_out();
        // the warm-up's end: the counters cleared and a round end (DSV-567), not round 1 at once
        let end = g.warmup_until().unwrap();
        g.tick(end.wrapping_add(10));
        assert!(!g.in_warmup());
        assert!(
            matches!(g.phase(), RoundPhase::Over { last: false, .. }),
            "{:?}",
            g.phase()
        );
        assert_eq!(g.scores, [0, 0]);
        assert_eq!(g.round_wins, [0, 0]);
        assert!(g.stats.values().all(|s| s.kills == 0 && s.deaths == 0));
        let o = g.take_out();
        for pid in [0u8, 1] {
            let ids = shell_ids(&o, pid);
            assert!(ids.contains(&gs::GS_SCORES_CLEAR), "{ids:x?}");
            assert!(ids.contains(&gs::GS_ROUND_RESULT), "{ids:x?}");
            assert!(!ids.contains(&gs::GS_ROUND_BEGIN), "{ids:x?}");
        }
        // after the result pause and the between-round pause: round 1, everyone back, the banner off
        let (p1, br) = (g.cfg.end_pause_ms.0, g.cfg.between_round_ms);
        g.tick(end.wrapping_add(10 + p1));
        g.take_out();
        let end = end.wrapping_add(20 + p1 + br);
        g.tick(end);
        assert_eq!(g.round_no, 1);
        assert_eq!(g.players[&1].life, Life::Alive, "the dead are back");
        let o = g.take_out();
        for pid in [0u8, 1] {
            let ids = shell_ids(&o, pid);
            assert!(ids.contains(&gs::GS_ROUND_BEGIN), "{ids:x?}");
        }
        let mb: Vec<&BitWriter> = o
            .iter()
            .filter_map(|x| match x {
                GameOut::To(0, m) if m.as_bytes()[1] == gs::GS_MATCH_BEGIN => Some(m),
                _ => None,
            })
            .collect();
        assert!(
            !mb.is_empty() && mb.iter().all(|m| !banner(m)),
            "banner off"
        );
        g.tick(end.wrapping_add(200));
        assert!(!g.bomb.carriers.is_empty(), "round 1 has a carrier");
        // `-warmup 0`: none
        let g = bomb_room();
        assert!(!g.in_warmup());
    }

    /// DSV-566: while the bomb warm-up runs no player damage lands: a shot that would kill leaves the victim's
    /// health as it was (no 0x74, no 0x6d, the shot drawn as a miss), as do a grenade blast and the thrower's own
    /// blast; damage without a killer (a fall) still hurts. Round 1 damages and kills again; `-combat
    /// warmup_damage=on` lets the warm-up hurt.
    #[test]
    fn the_bomb_warmup_refuses_all_player_damage() {
        let cfg = GameTuning {
            move_check: false,
            trace: false,
            ..GameTuning::default()
        };
        let mut g = bomb_room_with(cfg.clone());
        assert!(g.in_warmup() && g.warmup_holds_damage());
        g.tick(100);
        g.take_out();
        let src = super::combat::DamageSource {
            weapon: 3,
            dtype: super::TODO_DAMAGE_ZONE,
        };
        let ids_to = |o: &[GameOut], to: u8| shell_ids(o, to);
        let kill_sent = |o: &[GameOut]| {
            o.iter()
                .any(|x| matches!(x, GameOut::To(_, m) if m.as_bytes()[1] == gs::GS_KILL))
        };
        // a lethal shot (the victim at 1 health, no armour) after the spawn window
        if let Some(v) = g.players.get_mut(&1) {
            v.health = 1.0;
            v.armor = 0.0;
        }
        let f = hit_report(&g, 0, 1, 0.0, None);
        g.on_up(0, Up::Fire(Box::new(f)), 5_000);
        let o = g.take_out();
        assert_eq!(g.players[&1].health, 1.0, "health unchanged");
        assert_eq!(g.players[&1].life, Life::Alive);
        assert!(!kill_sent(&o), "no kill message");
        assert!(
            !ids_to(&o, 1).contains(&gs::GS_DAMAGE_TAKEN),
            "no damage message"
        );
        assert_eq!(g.hit_counts.get(hits::HitVerdict::Warmup), 1);
        assert_eq!(g.hit_counts.accepted(), 0);
        let e = effects(&o);
        assert!(
            !e.is_empty() && e.iter().all(|x| x.0 != g.players[&1].obj),
            "drawn as a miss: {e:?}"
        );
        // a blast of player 0 on player 1 and on himself: nothing
        g.apply_damage(1, Some(0), 1000.0, [1.0, 0.0, 0.0], src, 2, 5_100);
        g.apply_damage(0, Some(0), 1000.0, [1.0, 0.0, 0.0], src, 2, 5_100);
        let o = g.take_out();
        assert_eq!(g.players[&1].health, 1.0);
        assert_eq!(g.players[&0].life, Life::Alive, "no own-grenade death");
        assert!(!kill_sent(&o));
        assert_eq!(g.scores, [0, 0]);
        assert!(g.stats.values().all(|s| s.kills == 0 && s.deaths == 0));
        // a fall (no killer) still hurts
        if let Some(v) = g.players.get_mut(&1) {
            v.health = 50.0;
        }
        g.apply_hit_as(1, None, 10.0, 10.0, [0.0; 3], src, 2, 0, 5_200);
        assert_eq!(g.players[&1].health, 40.0, "fall damage stays");
        g.take_out();
        // round 1 (after the warm-up's round end and the between-round pause): the same lethal shot kills
        let end = g.warmup_until().unwrap();
        g.tick(end.wrapping_add(10));
        assert!(!g.in_warmup() && !g.warmup_holds_damage());
        let (p1, br) = (g.cfg.end_pause_ms.0, g.cfg.between_round_ms);
        g.tick(end.wrapping_add(10 + p1));
        let end = end.wrapping_add(20 + p1 + br);
        g.tick(end);
        assert_eq!(g.round_no, 1);
        g.tick(end.wrapping_add(5_000));
        g.take_out();
        if let Some(v) = g.players.get_mut(&1) {
            v.health = 1.0;
            v.armor = 0.0;
        }
        let f = hit_report(&g, 0, 1, 0.0, None);
        g.on_up(0, Up::Fire(Box::new(f)), end.wrapping_add(5_100));
        let o = g.take_out();
        assert_eq!(g.players[&1].health, 0.0, "round 1 damages");
        assert_ne!(g.players[&1].life, Life::Alive);
        assert!(kill_sent(&o), "kill message in round 1");
        assert!(ids_to(&o, 1).contains(&gs::GS_DAMAGE_TAKEN));
        // `-combat warmup_damage=on`: the warm-up hurts
        let mut on = cfg;
        on.set_combat("warmup_damage", "on").unwrap();
        let mut g = bomb_room_with(on);
        assert!(g.in_warmup() && !g.warmup_holds_damage());
        g.tick(100);
        let f = hit_report(&g, 0, 1, 0.0, None);
        g.on_up(0, Up::Fire(Box::new(f)), 5_000);
        assert!(g.players[&1].health < 100.0, "warm-up damage on");
    }

    /// DSV-567: the warm-up ends through the round end of every round: after its 0x43 the same messages, stage by
    /// stage, as the end of round 1 followed by round 2 (0x46 and 0xb5 with the result, the between-round set
    /// 0x47 / roster / 0x48 after the first pause, the spawn sets and 0x44 / 0xb5 / 0x45 after the between-round
    /// pause), with the round wins 0:0 and a draw in the 0x46.
    #[test]
    fn the_warmup_ends_like_a_round() {
        // the shell message ids to client 0 in the ticks at the round end, after the first pause and after the
        // between-round pause
        fn stages(g: &mut Game, at: Millis, end: impl FnOnce(&mut Game)) -> [Vec<u8>; 3] {
            let (p1, br) = (g.cfg.end_pause_ms.0, g.cfg.between_round_ms);
            g.take_out();
            end(g);
            let a = shell_ids(&g.take_out(), 0);
            g.tick(at.wrapping_add(p1));
            let b = shell_ids(&g.take_out(), 0);
            g.tick(at.wrapping_add(p1 + br + 10));
            let c = shell_ids(&g.take_out(), 0);
            [a, b, c]
        }
        let cfg = GameTuning {
            move_check: false,
            ..GameTuning::default()
        };
        // the warm-up's end
        let mut g = bomb_room_with(cfg.clone());
        g.tick(100);
        let end = g.warmup_until().unwrap();
        let w = stages(&mut g, end, |g| g.tick(end));
        assert_eq!(g.round_no, 1);
        // a real round end (round 1 of a room without a warm-up, a draw) and round 2
        let mut r = bomb_room_with(GameTuning {
            warmup_secs: Some(0),
            ..cfg
        });
        r.tick(100);
        r.tick(5_000);
        let at = 6_000;
        let n = stages(&mut r, at, |r| r.end_round(at, 0));
        assert_eq!(r.round_no, 2);
        assert_eq!(w[0].first(), Some(&gs::GS_SCORES_CLEAR), "{:x?}", w[0]);
        assert_eq!(
            w[0][1..],
            n[0][..],
            "the result tick: {:x?} vs {:x?}",
            w[0],
            n[0]
        );
        assert_eq!(w[1], n[1], "the between-round set");
        assert_eq!(w[2], n[2], "the round start");
        assert!(n[0].contains(&gs::GS_ROUND_RESULT) && n[0].contains(&gs::GS_ROUND_WINS));
        assert!(n[1].contains(&gs::GS_ROUND_OBJECTS_END) && n[1].contains(&gs::GS_ROUND_CLOSE));
        assert!(n[2].contains(&gs::GS_ROUND_BEGIN) && n[2].contains(&gs::GS_MATCH_BEGIN));
        assert!(!n[2].contains(&gs::GS_SCORES_CLEAR));
    }

    fn shell_ids(o: &[GameOut], to: u8) -> Vec<u8> {
        o.iter()
            .filter_map(|x| match x {
                GameOut::To(p, m) if *p == to => Some(m.as_bytes()[1]),
                _ => None,
            })
            .collect()
    }

    /// DSV-243: every next round of a round mode sends 0x44, 0xb5 and then 0x45 (no intro, waiting banner off) to each
    /// respawned player: the 2009 client plays its round-start voice in the 0x45 handler only.
    #[test]
    fn each_round_start_sends_the_match_begin_for_the_round_start_voice() {
        let mut g = bomb_room();
        g.tick(100);
        g.take_out();
        g.end_round(200, 1);
        let mut out = Vec::new();
        let mut t = 200;
        while t < 200 + 60_000 && g.round_no() < 2 {
            t += 50;
            g.tick(t);
            out.extend(g.take_out());
        }
        assert_eq!(g.round_no(), 2);
        for pid in [0u8, 1] {
            let ids = shell_ids(&out, pid);
            let rb = ids
                .iter()
                .position(|&i| i == gs::GS_ROUND_BEGIN)
                .expect("0x44");
            let mb = ids
                .iter()
                .position(|&i| i == gs::GS_MATCH_BEGIN)
                .expect("0x45");
            assert!(mb > rb, "client {pid}: 0x45 after 0x44: {ids:x?}");
        }
    }

    /// DSV-321 (playtest report): a next Quarantine round sends 0x44 but no 0x45 (no round-start voice; the
    /// countdown's intro voice is the round cue). Before, every round of class 11 sent 0x45 too.
    #[test]
    fn a_next_quarantine_round_sends_no_match_begin() {
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "11".into());
        room.pairs.insert("goal".into(), "3".into());
        g.set_room(&room);
        for (pid, uid, obj) in [(0u8, 7u32, 1u16), (1, 8, 3)] {
            let e = entry(uid, 0, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        g.tick(100);
        g.take_out();
        g.end_round(200, 1);
        let mut out = Vec::new();
        let mut t = 200;
        while t < 200 + 60_000 && g.round_no() < 2 {
            t += 50;
            g.tick(t);
            out.extend(g.take_out());
        }
        assert_eq!(g.round_no(), 2);
        for pid in [0u8, 1] {
            let ids = shell_ids(&out, pid);
            assert!(ids.contains(&gs::GS_ROUND_BEGIN), "{ids:x?}");
            assert!(!ids.contains(&gs::GS_MATCH_BEGIN), "client {pid}: {ids:x?}");
        }
    }

    /// DSV-473 (playtest report): a respawn inside the round of a respawn mode (Elimination, One Man Army) is
    /// followed by `GS_MATCH_BEGIN` after the spawn set's 0x67 [1], so the 2009 client says "round start" again; a
    /// dead player of a round mode (Search and Destroy) is not respawned inside the round and gets no 0x45.
    // DISASM: cshell+0x49f00..0x49f7b (the round-start voice for every 0x45 taken in player state 1)
    #[test]
    fn a_respawn_in_a_respawn_mode_sends_the_match_begin_again() {
        for class in ["1", "6"] {
            let mut g = Game::new(GameTuning::default(), &world(), tables());
            let mut room = RoomSettings::default();
            room.pairs.insert("type".into(), class.into());
            room.pairs.insert("time".into(), "600".into());
            g.set_room(&room);
            for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3)] {
                let e = entry(uid, team, "P");
                g.note_identified(&e);
                g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
                g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
                g.on_up(pid, Up::WorldReady, 20);
            }
            g.tick(100);
            g.take_out();
            g.kill(1, Some(0), 3, false, 1_000);
            g.tick(1_050);
            g.take_out();
            g.on_up(1, Up::RespawnRequest { id: 0x89 }, 8_100);
            assert_eq!(g.players[&1].life, Life::Alive, "class {class}");
            let o = g.take_out();
            let ids = shell_ids(&o, 1);
            let alive = ids
                .iter()
                .rposition(|&i| i == gs::GS_PLAYER_STATE)
                .expect("0x67");
            let mb = ids
                .iter()
                .position(|&i| i == gs::GS_MATCH_BEGIN)
                .expect("0x45 after the respawn");
            assert!(mb > alive, "class {class}: 0x45 after 0x67 [1]: {ids:x?}");
            assert!(
                !shell_ids(&o, 0).contains(&gs::GS_MATCH_BEGIN),
                "class {class}: only the respawned player"
            );
        }
        // a round mode: the dead player stays dead until the next round, no 0x45
        let mut g = bomb_room();
        g.tick(100);
        g.take_out();
        g.kill(1, Some(0), 3, false, 1_000);
        g.tick(1_050);
        g.on_up(1, Up::RespawnRequest { id: 0x89 }, 8_100);
        assert_ne!(g.players[&1].life, Life::Alive);
        let o = g.take_out();
        assert!(!shell_ids(&o, 1).contains(&gs::GS_MATCH_BEGIN));
    }

    /// DSV-242: plain Search and Destroy (class 2) keeps its sides at half the rounds; the clan class 3 switches.
    #[test]
    fn plain_search_and_destroy_does_not_switch_sides() {
        for (class, switch) in [("2", false), ("3", true)] {
            let mut g = bomb_room();
            let mut room = RoomSettings::default();
            room.pairs.insert("type".into(), class.into());
            room.pairs.insert("goal".into(), "4".into());
            g.set_room(&room);
            g.round_no = 2;
            g.side_switch_at_round_end([1, 1]);
            assert_eq!(g.modes.swap_pending, switch, "class {class}");
        }
    }

    /// DSV-240: the carrier gets the bomb item in slot 9 (0xb2), may draw it (0x7f), and loses it at the plant
    /// (0xb4, then his first weapon drawn again); a dead carrier loses it, the attacker who takes the bomb gets it.
    #[test]
    fn search_and_destroy_bomb_item_is_granted_drawn_and_taken() {
        use super::bomb::{BOMB_SLOT, PLANT_MS};
        const ITEM: u16 = 16;
        let mut g = bomb_room();
        g.bomb.item = Some(ITEM);
        g.bomb.item_setup = true;
        g.tick(100);
        assert_eq!(g.bomb().carriers, vec![0]);
        let o = g.take_out();
        let give = o
            .iter()
            .find_map(|x| match x {
                GameOut::To(0, m) if m.as_bytes()[1] == gs::GS_WEAPON_GIVE => Some(m.clone()),
                _ => None,
            })
            .expect("0xb2 bomb item to the carrier");
        let mut r = give.reader();
        r.skip_bits(16);
        let weapon = r.read_u16();
        r.skip_bits(16 + 32 + 8);
        assert_eq!((weapon, r.read_u8()), (ITEM, BOMB_SLOT));
        assert!(
            !shell_ids(&o, 1).contains(&gs::GS_WEAPON_GIVE),
            "not to the defender"
        );
        assert!(g.players[&0]
            .weapons
            .iter()
            .any(|w| w.weapon == ITEM && w.slot == BOMB_SLOT));
        // the carrier takes the bomb out
        g.on_up(
            0,
            Up::WeaponChange {
                slot: BOMB_SLOT,
                weapon_id: ITEM,
                aux: 0,
            },
            150,
        );
        let cur = g.players[&0]
            .current
            .and_then(|i| g.players[&0].weapons.get(i))
            .map(|w| w.weapon);
        assert_eq!(cur, Some(ITEM));
        g.take_out();
        // the plant takes the item away
        g.players.get_mut(&0).unwrap().pos = [1000.0, 0.0, 0.0];
        g.on_up(
            0,
            Up::UseHold {
                phase: 1,
                target: 50,
            },
            300,
        );
        g.tick(300 + PLANT_MS);
        assert!(g.bomb().planted.is_some());
        let o = g.take_out();
        let rm = o
            .iter()
            .find_map(|x| match x {
                GameOut::To(0, m) if m.as_bytes()[1] == gs::GS_WEAPON_REMOVE => Some(m.clone()),
                _ => None,
            })
            .expect("0xb4 at the plant");
        let mut r = rm.reader();
        r.skip_bits(16);
        assert_eq!((r.read_u8(), r.read_u16()), (BOMB_SLOT, ITEM));
        assert!(!g.players[&0].weapons.iter().any(|w| w.weapon == ITEM));
        assert_ne!(
            g.players[&0]
                .current
                .and_then(|i| g.players[&0].weapons.get(i))
                .map(|w| w.weapon),
            Some(ITEM)
        );

        // DSV-350: every attacker holds the bomb; a dead one loses it and drops nothing; the first plant takes it from
        // the others and nobody else can plant
        let mut g = bomb_room();
        g.bomb.item = Some(ITEM);
        g.bomb.item_setup = true;
        let e = entry(9, 0, "Q");
        g.note_identified(&e);
        g.add_player(2, Some(&e), None, 5, [0.0; 3], [0.0; 3], 0);
        g.on_up(2, Up::PlayerInfo(Box::default()), 10);
        g.on_up(2, Up::WorldReady, 20);
        assert_eq!(g.players[&2].team, 1);
        g.tick(100);
        let mut held = g.bomb().carriers.clone();
        held.sort();
        assert_eq!(held, vec![0, 2], "every attacker holds the bomb");
        let o = g.take_out();
        for p in [0, 2] {
            assert!(
                shell_ids(&o, p).contains(&gs::GS_WEAPON_GIVE),
                "grant to {p}"
            );
            assert!(
                shell_ids(&o, p).contains(&gs::GS_OBJECTIVE_ITEM),
                "mark to {p}"
            );
        }
        g.players.get_mut(&2).unwrap().pos = [500.0, 0.0, 0.0];
        g.kill(2, Some(1), 3, false, 200);
        g.tick(210);
        assert_eq!(g.bomb().carriers, vec![0]);
        let o = g.take_out();
        assert!(
            shell_ids(&o, 2).contains(&gs::GS_WEAPON_REMOVE),
            "the dead attacker's item goes"
        );
        g.players.get_mut(&0).unwrap().pos = [500.0, 0.0, 0.0];
        g.tick(300);
        assert_eq!(
            g.bomb().carriers,
            vec![0],
            "nothing lies in the world to take"
        );
        assert!(!shell_ids(&g.take_out(), 0).contains(&gs::GS_WEAPON_GIVE));
    }

    /// DSV-350: two living attackers both hold the bomb; the first completed plant removes it from both and the
    /// other cannot plant afterwards.
    #[test]
    fn search_and_destroy_every_attacker_holds_the_bomb_until_the_first_plant() {
        use super::bomb::PLANT_MS;
        const ITEM: u16 = 16;
        let mut g = bomb_room();
        g.bomb.item = Some(ITEM);
        g.bomb.item_setup = true;
        let e = entry(9, 0, "Q");
        g.note_identified(&e);
        g.add_player(2, Some(&e), None, 5, [0.0; 3], [0.0; 3], 0);
        g.on_up(2, Up::PlayerInfo(Box::default()), 10);
        g.on_up(2, Up::WorldReady, 20);
        g.tick(100);
        for p in [0, 2] {
            assert!(
                g.players[&p].weapons.iter().any(|w| w.weapon == ITEM),
                "{p}"
            );
            g.players.get_mut(&p).unwrap().pos = [1000.0, 0.0, 0.0];
        }
        g.take_out();
        g.on_up(
            2,
            Up::UseHold {
                phase: 1,
                target: 50,
            },
            200,
        );
        g.tick(200 + PLANT_MS);
        assert_eq!(g.bomb().planted.map(|p| p.by), Some(2));
        let o = g.take_out();
        for p in [0, 2] {
            assert!(
                shell_ids(&o, p).contains(&gs::GS_WEAPON_REMOVE),
                "removed from {p}"
            );
            assert!(!g.players[&p].weapons.iter().any(|w| w.weapon == ITEM));
        }
        assert!(g.bomb().carriers.is_empty());
        g.on_up(
            0,
            Up::UseHold {
                phase: 1,
                target: 50,
            },
            20_000,
        );
        assert!(g.bomb().hold.is_none(), "nobody else plants that round");
    }

    /// DSV-200..DSV-203: the only attacker carries the bomb (0x56 with his character), plants it by holding inside
    /// the site (0xad with 4 s, 0x51 [40] to everyone after the hold), the round clock no longer ends the round and
    /// his death does not either; the defender's 7 s hold at the bomb defuses it (0x50) and wins the round.
    #[test]
    fn search_and_destroy_carrier_plant_and_defuse() {
        use super::bomb::{BOMB_SECS, DEFUSE_MS, PLANT_MS};
        let mut g = bomb_room();
        g.tick(100);
        assert_eq!(g.bomb().carriers, vec![0]);
        let o = g.take_out();
        assert!(
            shell_ids(&o, 0).contains(&gs::GS_OBJECTIVE_ITEM),
            "carrier mark to the attacker"
        );
        assert!(
            !shell_ids(&o, 1).contains(&gs::GS_OBJECTIVE_ITEM),
            "not to the defender"
        );
        // a plant outside the site is refused
        g.on_up(
            0,
            Up::UseHold {
                phase: 1,
                target: 50,
            },
            200,
        );
        assert!(!shell_ids(&g.take_out(), 0).contains(&gs::GS_TRACK_OBJECT));
        // the defender cannot plant
        g.players.get_mut(&1).unwrap().pos = [1000.0, 0.0, 0.0];
        g.on_up(
            1,
            Up::UseHold {
                phase: 1,
                target: 50,
            },
            210,
        );
        assert!(g.bomb().hold.is_none());
        g.players.get_mut(&0).unwrap().pos = [1000.0, 0.0, 0.0];
        g.on_up(
            0,
            Up::UseHold {
                phase: 1,
                target: 50,
            },
            300,
        );
        let o = g.take_out();
        let t = o
            .iter()
            .find_map(|x| match x {
                GameOut::To(0, m) if m.as_bytes()[1] == gs::GS_TRACK_OBJECT => Some(m.clone()),
                _ => None,
            })
            .expect("0xad");
        let mut r = t.reader();
        r.skip_bits(16);
        assert_eq!((r.read_u16(), r.read_u8(), r.read_u32()), (50, 1, 0));
        assert_eq!(r.read_f32(), PLANT_MS as f32 / 1000.0);
        g.tick(300 + PLANT_MS - 1);
        assert!(g.bomb().planted.is_none());
        g.tick(300 + PLANT_MS);
        let o = g.take_out();
        assert!(g.bomb().planted.is_some());
        let planted = o
            .iter()
            .find_map(|x| match x {
                GameOut::To(1, m) if m.as_bytes()[1] == gs::GS_BOMB_PLANTED => Some(m.clone()),
                _ => None,
            })
            .expect("0x51 to the defender");
        let mut r = planted.reader();
        r.skip_bits(16);
        assert_eq!(r.read_u32(), BOMB_SECS);
        // the round clock (20 s here) passes: no time-out; the attacker dies: no elimination
        g.rules.round_secs = 20;
        let secs = g.rules.round_secs;
        g.kill(0, Some(1), 3, false, 5000);
        g.tick(20 + secs * 1000 + 10);
        let logs: Vec<String> = g
            .take_out()
            .into_iter()
            .filter_map(|x| match x {
                GameOut::Log(l) => Some(l),
                _ => None,
            })
            .collect();
        assert!(matches!(g.phase(), RoundPhase::Running { .. }), "{logs:#?}");
        // the defender defuses at the bomb
        let t0 = 20 + secs * 1000 + 100;
        g.on_up(
            1,
            Up::UseHold {
                phase: 2 - 1,
                target: 51,
            },
            t0,
        );
        assert!(g.bomb().hold.is_some_and(|h| h.defuse));
        g.tick(t0 + DEFUSE_MS);
        let o = g.take_out();
        assert!(shell_ids(&o, 0).contains(&gs::GS_BOMB_DEFUSED));
        assert!(matches!(g.phase(), RoundPhase::Over { win: 2, .. }));
    }

    /// DSV-202 / DSV-203: a hold ends when the key is let go; the planted bomb blows up after its timer (0x4f),
    /// kills the defender beside it and gives the round to the attackers.
    #[test]
    fn search_and_destroy_release_breaks_the_hold_and_the_bomb_explodes() {
        use super::bomb::{BOMB_SECS, PLANT_MS};
        let mut g = bomb_room();
        g.tick(100);
        g.players.get_mut(&0).unwrap().pos = [1000.0, 0.0, 0.0];
        g.on_up(
            0,
            Up::UseHold {
                phase: 1,
                target: 50,
            },
            200,
        );
        g.on_up(
            0,
            Up::UseHold {
                phase: 4,
                target: 50,
            },
            1000,
        );
        g.tick(200 + PLANT_MS);
        assert!(g.bomb().planted.is_none(), "let go before the time");
        g.on_up(
            0,
            Up::UseHold {
                phase: 1,
                target: 50,
            },
            5000,
        );
        g.tick(5000 + PLANT_MS);
        assert!(g.bomb().planted.is_some());
        g.players.get_mut(&1).unwrap().pos = [1050.0, 0.0, 0.0];
        g.players.get_mut(&0).unwrap().pos = [-2000.0, 0.0, 0.0];
        g.take_out();
        g.tick(5000 + PLANT_MS + BOMB_SECS * 1000);
        let o = g.take_out();
        assert!(shell_ids(&o, 0).contains(&gs::GS_BOMB_EXPLODED));
        assert!(
            !(g.players[&1].life == Life::Alive),
            "the defender beside it died"
        );
        assert_eq!(
            g.players[&0].life,
            Life::Alive,
            "the attacker far away lives"
        );
        assert!(matches!(g.phase(), RoundPhase::Over { win: 1, .. }));
    }

    /// DSV-400: a detonation that ends a Search and Destroy round sends both clients the round result 0x46 with the
    /// round wins after the round (the 2009 top counters of class 2 take only the 0x46 pair), and the match-deciding
    /// detonation carries the goal; the defender stays out of the blast so the bomb itself ends each round.
    #[test]
    fn a_detonation_sends_the_round_wins_after_the_round() {
        use super::bomb::{BOMB_SECS, PLANT_MS};
        let result_of = |o: &[GameOut], to: u8| -> Option<(i8, [u8; 2])> {
            o.iter().find_map(|x| match x {
                GameOut::To(p, m) if *p == to && m.as_bytes()[1] == gs::GS_ROUND_RESULT => {
                    let mut r = m.reader();
                    r.skip_bits(16);
                    r.read_u32();
                    let w = r.read_s8();
                    Some((w, [r.read_u8(), r.read_u8()]))
                }
                _ => None,
            })
        };
        let mut g = bomb_room();
        let mut t = 100;
        g.tick(t);
        for round in 1..=2u8 {
            assert_eq!(g.round_no(), round as u32);
            g.players.get_mut(&0).unwrap().pos = [1000.0, 0.0, 0.0];
            g.players.get_mut(&1).unwrap().pos = [-3000.0, 0.0, 0.0];
            t += 100;
            g.on_up(
                0,
                Up::UseHold {
                    phase: 1,
                    target: 50,
                },
                t,
            );
            t += PLANT_MS;
            g.tick(t);
            assert!(g.bomb().planted.is_some(), "round {round}: planted");
            g.players.get_mut(&0).unwrap().pos = [-2000.0, 0.0, 0.0];
            g.take_out();
            t += BOMB_SECS * 1000;
            g.tick(t);
            let o = g.take_out();
            assert!(shell_ids(&o, 0).contains(&gs::GS_BOMB_EXPLODED));
            assert_eq!(
                g.players[&1].life,
                Life::Alive,
                "the defender is out of reach"
            );
            for pid in [0u8, 1] {
                assert_eq!(
                    result_of(&o, pid),
                    Some((1, [round, 0])),
                    "round {round}: client {pid} gets the wins after the detonation"
                );
            }
            if round == 2 {
                assert!(matches!(g.phase(), RoundPhase::Over { last: true, .. }));
                break;
            }
            while t < 400_000 && !(g.round_no() == 2 && !g.bomb().carriers.is_empty()) {
                t += 50;
                g.tick(t);
            }
            for _ in 0..10 {
                t += 50;
                g.tick(t);
            }
            g.take_out();
        }
    }

    /// DSV-240 / GMD-8: after a planted round the next round's carrier gets the bomb item again (0xb2 after his spawn set)
    /// and the carrier mark (0x56).
    #[test]
    fn search_and_destroy_next_round_grants_the_bomb_item_again() {
        use super::bomb::{BOMB_SECS, BOMB_SLOT, PLANT_MS};
        const ITEM: u16 = 16;
        let mut g = bomb_room();
        g.bomb.item = Some(ITEM);
        g.bomb.item_setup = true;
        g.tick(100);
        assert_eq!(g.bomb().carriers, vec![0]);
        g.players.get_mut(&0).unwrap().pos = [1000.0, 0.0, 0.0];
        g.players.get_mut(&1).unwrap().pos = [-3000.0, 0.0, 0.0];
        g.on_up(
            0,
            Up::UseHold {
                phase: 1,
                target: 50,
            },
            200,
        );
        g.tick(200 + PLANT_MS);
        assert!(g.bomb().planted.is_some());
        let mut t = 200 + PLANT_MS + BOMB_SECS * 1000;
        g.tick(t);
        assert!(matches!(g.phase(), RoundPhase::Over { .. }));
        g.take_out();
        let mut out = Vec::new();
        while t < 200_000 && !(g.round_no() >= 2 && !g.bomb().carriers.is_empty()) {
            t += 50;
            g.tick(t);
            out.extend(g.take_out());
        }
        for _ in 0..10 {
            t += 50;
            g.tick(t);
            out.extend(g.take_out());
        }
        assert_eq!(g.round_no(), 2);
        assert_eq!(
            g.bomb().carriers,
            vec![0],
            "the only attacker carries again"
        );
        let ids = shell_ids(&out, 0);
        let give = ids
            .iter()
            .rposition(|&i| i == gs::GS_WEAPON_GIVE)
            .unwrap_or_else(|| panic!("0xb2 in round 2: {ids:x?}"));
        let rb = ids
            .iter()
            .position(|&i| i == gs::GS_ROUND_BEGIN)
            .expect("0x44");
        assert!(give > rb, "the grant after the round start: {ids:x?}");
        assert!(
            ids[rb..].contains(&gs::GS_OBJECTIVE_ITEM),
            "the carrier mark"
        );
        assert!(g.players[&0]
            .weapons
            .iter()
            .any(|w| w.weapon == ITEM && w.slot == BOMB_SLOT));
    }

    /// DSV-300: the plant hold freezes the carrier with its 0xad (`[0x0080][1]`), letting go releases him
    /// (`[0x0080][0]`); a completed plant releases him too, and the bomb is planted although he tried to walk.
    #[test]
    fn search_and_destroy_plant_hold_freezes_the_planter() {
        use super::bomb::PLANT_MS;
        let mut g = bomb_room();
        g.tick(100);
        g.take_out();
        g.players.get_mut(&0).unwrap().pos = [1000.0, 0.0, 0.0];
        g.on_up(
            0,
            Up::UseHold {
                phase: 1,
                target: 50,
            },
            200,
        );
        let o = g.take_out();
        assert!(shell_ids(&o, 0).contains(&gs::GS_TRACK_OBJECT));
        assert_eq!(mover_records(&o), vec![(gs::MOVER_FREEZE, Some(1))]);
        assert!(g.players[&0].frozen);
        g.on_up(
            0,
            Up::UseHold {
                phase: 4,
                target: 50,
            },
            1000,
        );
        assert_eq!(
            mover_records(&g.take_out()),
            vec![(gs::MOVER_FREEZE, Some(0))]
        );
        assert!(!g.players[&0].frozen);
        g.on_up(
            0,
            Up::UseHold {
                phase: 1,
                target: 50,
            },
            2000,
        );
        g.take_out();
        g.tick(2000 + PLANT_MS);
        assert!(g.bomb().planted.is_some(), "planted in place");
        assert_eq!(
            mover_records(&g.take_out()),
            vec![(gs::MOVER_FREEZE, Some(0))]
        );
        assert!(!g.players[&0].frozen);
    }

    /// DSV-461: a planter whose accepted position leaves the hold point by more than `HOLD_MOVE` (a client that
    /// moved through the freeze) loses the plant: no 0x51, the freeze 0, the bomb still carried; a small step within
    /// the tolerance keeps it.
    #[test]
    fn a_move_during_the_plant_cancels_it() {
        use super::bomb::{HOLD_MOVE, PLANT_MS};
        let mut g = bomb_room();
        g.tick(100);
        g.take_out();
        g.players.get_mut(&0).unwrap().pos = [1000.0, 0.0, 0.0];
        g.on_up(
            0,
            Up::UseHold {
                phase: 1,
                target: 50,
            },
            200,
        );
        g.take_out();
        // a small step (latency of the last update) keeps the hold
        g.players.get_mut(&0).unwrap().pos = [1000.0 + HOLD_MOVE * 0.5, 0.0, 0.0];
        g.tick(1200);
        assert!(g.bomb().hold.is_some(), "within the tolerance");
        // the update that walks away breaks it at the next tick
        g.players.get_mut(&0).unwrap().pos = [1000.0 + HOLD_MOVE + 10.0, 0.0, 0.0];
        g.tick(1300);
        assert!(g.bomb().hold.is_none(), "cancelled");
        let o = g.take_out();
        assert_eq!(mover_records(&o), vec![(gs::MOVER_FREEZE, Some(0))]);
        assert!(!shell_ids(&o, 0).contains(&gs::GS_BOMB_PLANTED));
        g.tick(200 + PLANT_MS + 100);
        assert!(g.bomb().planted.is_none(), "never planted");
        assert!(g.bomb().carriers.contains(&0), "still carries the bomb");
    }

    fn bomb_outs(o: &[GameOut]) -> Vec<GameOut> {
        o.iter()
            .filter(|x| {
                matches!(
                    x,
                    GameOut::BombShown { .. }
                        | GameOut::BombHidden
                        | GameOut::BombBlast { .. }
                        | GameOut::BombBlastGone
                )
            })
            .cloned()
            .collect()
    }

    /// Plants the bomb of [`bomb_room`] at (1000, 0, 0) by client 0 from `t`; returns the time of the plant.
    fn plant_at_site(g: &mut Game, t: u32) -> u32 {
        use super::bomb::PLANT_MS;
        g.players.get_mut(&0).unwrap().pos = [1000.0, 0.0, 0.0];
        g.on_up(
            0,
            Up::UseHold {
                phase: 1,
                target: 50,
            },
            t,
        );
        g.tick(t + PLANT_MS);
        assert!(g.bomb().planted.is_some());
        t + PLANT_MS
    }

    /// DSV-220 / DSV-222: the planted bomb's objects (model, indicator, defuse point) appear with the plant at its
    /// place in its site's zone, a defuse is taken only at the bomb (not at the far end of the site), and the objects
    /// go with the defuse.
    #[test]
    fn the_planted_bomb_is_shown_at_its_place_and_goes_with_the_defuse() {
        use super::bomb::DEFUSE_MS;
        let mut g = bomb_room();
        g.tick(100);
        g.take_out();
        let t = plant_at_site(&mut g, 200);
        // DSV-360: no trace world here: at the planter's feet
        let feet = -g.dims_of(0)[1];
        assert_eq!(
            bomb_outs(&g.take_out()),
            vec![GameOut::BombShown {
                site: 1,
                pos: [1000.0, feet, 0.0]
            }]
        );
        g.tick(t + 10);
        assert!(bomb_outs(&g.take_out()).is_empty(), "shown once");
        // a defender at the far edge of the site (and anywhere else) is not at the bomb
        g.players.get_mut(&1).unwrap().pos = [1100.0, 0.0, 90.0];
        g.on_up(
            1,
            Up::UseHold {
                phase: 1,
                target: 60,
            },
            t + 20,
        );
        assert!(g.bomb().hold.is_none(), "refused away from the bomb");
        g.players.get_mut(&1).unwrap().pos = [1040.0, 0.0, 0.0];
        g.on_up(
            1,
            Up::UseHold {
                phase: 1,
                target: 60,
            },
            t + 30,
        );
        assert!(g.bomb().hold.is_some_and(|h| h.defuse));
        g.tick(t + 30 + DEFUSE_MS);
        assert_eq!(bomb_outs(&g.take_out()), vec![GameOut::BombHidden]);
    }

    /// DSV-360: over a floor the planted bomb lies on the floor below the planter (not at his body centre); the
    /// defuse reach is still measured from where he stood.
    #[test]
    fn the_planted_bomb_lies_on_the_floor_below_the_planter() {
        use dc_sim::{CollModel, CollisionWorld, ConvexPoly, SURF_SOLID, V3};
        let floor = ConvexPoly::from_corners(
            vec![
                V3::new(-3000.0, -100.0, -3000.0),
                V3::new(-3000.0, -100.0, 3000.0),
                V3::new(3000.0, -100.0, 3000.0),
                V3::new(3000.0, -100.0, -3000.0),
            ],
            SURF_SOLID,
        );
        let mut g = bomb_room();
        g.set_trace_world(Some(CollisionWorld::from_parts(
            vec![CollModel::from_polys(std::slice::from_ref(&floor))],
            &[],
        )));
        g.tick(100);
        g.take_out();
        g.players.get_mut(&0).unwrap().pos = [1000.0, -47.0, 20.0];
        g.on_up(
            0,
            Up::UseHold {
                phase: 1,
                target: 50,
            },
            200,
        );
        g.tick(200 + super::bomb::PLANT_MS);
        let pl = g.bomb().planted.expect("planted");
        assert_eq!(pl.pos, [1000.0, -47.0, 20.0]);
        assert!((pl.floor[1] - -100.0).abs() < 0.5, "{:?}", pl.floor);
        assert_eq!([pl.floor[0], pl.floor[2]], [1000.0, 20.0]);
        let shown: Vec<_> = bomb_outs(&g.take_out());
        assert!(
            matches!(shown.as_slice(), [GameOut::BombShown { site: 1, pos }] if *pos == pl.floor),
            "{shown:?}"
        );
    }

    /// DSV-361: the defenders run out while the bomb is planted: the attackers win the round and the bomb is cancelled
    /// at once (its objects leave with the result); its timer running out later brings no 0x4f, no blast and no second
    /// win; the next round starts without a bomb. The match goal reached while planted cancels it the same way.
    #[test]
    fn a_round_that_ends_while_the_bomb_is_planted_cancels_the_bomb() {
        use super::bomb::{BombEnd, BOMB_SECS};
        let mut g = bomb_room();
        g.tick(100);
        let t = plant_at_site(&mut g, 200);
        g.take_out();
        g.players.get_mut(&1).unwrap().pos = [1050.0, 0.0, 0.0];
        g.kill(1, Some(0), 3, false, t + 1000);
        g.tick(t + 1010);
        assert!(matches!(g.phase(), RoundPhase::Over { win: 1, .. }));
        assert!(g.bomb().planted.is_none());
        assert_eq!(g.bomb().ended, Some(BombEnd::Cancelled));
        let o = g.take_out();
        assert_eq!(
            bomb_outs(&o),
            vec![GameOut::BombHidden],
            "gone with the result"
        );
        let wins = g.round_wins;
        let mut out = Vec::new();
        let mut now = t + 1010;
        while now < t + BOMB_SECS * 1000 + 2000 && g.round_no() < 2 {
            now += 50;
            g.tick(now);
            out.extend(g.take_out());
        }
        for pid in [0, 1] {
            assert!(
                !shell_ids(&out, pid).contains(&gs::GS_BOMB_EXPLODED),
                "no 0x4f to {pid}"
            );
        }
        assert!(!out
            .iter()
            .any(|x| matches!(x, GameOut::BombBlast { .. } | GameOut::BombShown { .. })));
        assert_eq!(g.round_wins, wins, "no second win");
        assert_eq!(g.round_no(), 2);
        g.tick(now + 50);
        assert!(g.bomb().planted.is_none() && g.bomb().ended.is_none());

        // the match goal reached by another rule while planted (here the round end itself): cancelled, no blast
        let mut g = bomb_room();
        g.tick(100);
        let t = plant_at_site(&mut g, 200);
        g.round_wins = [1, 0];
        g.take_out();
        g.end_round(t + 500, 1);
        assert!(matches!(g.phase(), RoundPhase::Over { last: true, .. }));
        assert_eq!(g.bomb().ended, Some(BombEnd::Cancelled));
        let mut out = g.take_out();
        for k in 1..=50 {
            g.tick(t + 500 + k * 1000);
            out.extend(g.take_out());
        }
        assert!(!shell_ids(&out, 0).contains(&gs::GS_BOMB_EXPLODED));
        assert!(!out.iter().any(|x| matches!(x, GameOut::BombBlast { .. })));
        assert_eq!(g.round_wins, [2, 0]);
    }

    /// DSV-221: the blast puts the blast effect at the bomb and removes the bomb; the effect leaves after its time.
    #[test]
    fn the_bomb_blast_shows_its_effect_and_removes_the_bomb() {
        use super::bomb::{BLAST_FX_MS, BOMB_SECS};
        let mut g = bomb_room();
        g.tick(100);
        let t = plant_at_site(&mut g, 200);
        g.take_out();
        let boom = t + BOMB_SECS * 1000;
        g.tick(boom);
        let feet = -g.dims_of(0)[1];
        assert_eq!(
            bomb_outs(&g.take_out()),
            vec![
                GameOut::BombBlast {
                    pos: [1000.0, feet, 0.0]
                },
                GameOut::BombHidden
            ]
        );
        g.tick(boom + BLAST_FX_MS - 1);
        assert!(bomb_outs(&g.take_out()).is_empty());
        g.tick(boom + BLAST_FX_MS);
        assert_eq!(bomb_outs(&g.take_out()), vec![GameOut::BombBlastGone]);
    }

    /// The 0xd3 messages client `to` got: (sub, team, pid, look byte 14) for sub 0, (1, round, limit, secs) for
    /// sub 1.
    fn infection_msgs(o: &[GameOut], to: u8) -> Vec<(u8, u32, u32, i32)> {
        o.iter()
            .filter_map(|x| match x {
                GameOut::To(p, m) if *p == to && m.as_bytes()[1] == gs::GS_INFECTION_MODE => {
                    let mut r = m.reader();
                    r.skip_bits(16);
                    match r.read_u8() {
                        0 => {
                            let team = r.read_u8();
                            if team == 2 {
                                r.read_u32();
                            }
                            let pid = r.read_u32();
                            let mut look = [0i8; 15];
                            for b in &mut look {
                                *b = r.read_u8() as i8;
                            }
                            Some((0, u32::from(team), pid, i32::from(look[14])))
                        }
                        1 => {
                            let (a, b) = (r.read_u8(), r.read_u8());
                            r.read_bit();
                            Some((1, u32::from(a), u32::from(b), r.read_f32() as i32))
                        }
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect()
    }

    /// The 0xd3 sub 3 rows client `to` got (DSV-320).
    fn override_msgs(o: &[GameOut], to: u8) -> Vec<i8> {
        o.iter()
            .filter_map(|x| match x {
                GameOut::To(p, m) if *p == to && m.as_bytes()[1] == gs::GS_INFECTION_MODE => {
                    let mut r = m.reader();
                    r.skip_bits(16);
                    (r.read_u8() == 3).then(|| r.read_u8() as i8)
                }
                _ => None,
            })
            .collect()
    }

    /// The 0xcb messages client `to` got: (sub, a, b, c) = sub 0 (value, team, pid), sub 1 (total, dropped, spies),
    /// sub 5 (phase, 0, 0), sub 6 (secs, colour, 0), subs 2 / 3 / 4 (0, 0, 0).
    fn bounty_msgs(o: &[GameOut], to: u8) -> Vec<(u8, u32, u32, u32)> {
        o.iter()
            .filter_map(|x| match x {
                GameOut::To(p, m) if *p == to && m.as_bytes()[1] == gs::GS_BOUNTY_MODE => {
                    let mut r = m.reader();
                    r.skip_bits(16);
                    let sub = r.read_u8();
                    Some(match sub {
                        0 => {
                            let v = r.read_u32();
                            let t = r.read_u8();
                            (0, v, u32::from(t), r.read_u32())
                        }
                        1 => (1, r.read_u32(), r.read_u32(), r.read_u32()),
                        5 => (5, u32::from(r.read_u8()), 0, 0),
                        6 => (6, r.read_u32(), r.read_u32(), 0),
                        s => (s, 0, 0, 0),
                    })
                }
                _ => None,
            })
            .collect()
    }

    /// The (status, value) pairs of the player-status messages to `to`.
    fn status_msgs(o: &[GameOut], to: u8) -> Vec<(u8, f32)> {
        o.iter()
            .filter_map(|x| match x {
                GameOut::To(p, m) if *p == to && m.as_bytes()[1] == gs::GS_PLAYER_STATUS => {
                    let mut r = m.reader();
                    r.skip_bits(16);
                    let kind = r.read_u8();
                    r.skip_bits(16);
                    Some((kind, r.read_f32()))
                }
                _ => None,
            })
            .collect()
    }

    /// A Spy Hunt room of three hunters (players 0..2) with six intel points and the agent weapons 30 / 10.
    fn spy_room() -> Game {
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        let pts: Vec<[f32; 3]> = (0..6).map(|i| [i as f32 * 1000.0, 0.0, 2000.0]).collect();
        g.set_spy_world(crate::world::SpyWorld {
            intel_points: pts,
            uplinks: Vec::new(),
            triggers: Vec::new(),
        });
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "9".into());
        g.set_room(&room);
        g.tables.weapons.commands = vec![10, 20, 30];
        g.tables.weapons.class_ends = vec![1, 2, 3];
        for w in [10u16, 20, 30] {
            g.tables.weapons.weapons.insert(w, Default::default());
        }
        g.spy.gms.agent_weapons = vec![30, 10];
        for (pid, uid, obj) in [(0u8, 7u32, 1u16), (1, 8, 3), (2, 9, 5)] {
            let e = entry(uid, 0, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        for p in g.players.values_mut() {
            p.pos = [-9000.0, 0.0, -9000.0];
        }
        g.tick(100);
        g.spawn_notes.clear();
        g.take_out();
        g
    }

    /// Player `pid` walks onto the first lying case and back away at `t`.
    fn spy_seize(g: &mut Game, pid: u8, t: Millis) {
        let at = g.spy().intel[0].pos;
        g.players.get_mut(&pid).unwrap().pos = at;
        g.tick(t);
        g.players.get_mut(&pid).unwrap().pos = [-9000.0, 0.0, -9000.0];
    }

    /// DSV-450 (project decision "spies need to be able to kill other spies"): a spy's hit on another spy is
    /// not refused, his shot kills him, the victim's intel drops at the body (DSV-232) and the killer takes it there.
    #[test]
    fn spy_hunt_a_spy_kills_a_spy_and_takes_his_intel() {
        use super::spy::*;
        let mut g = spy_room();
        spy_seize(&mut g, 0, 200);
        spy_seize(&mut g, 0, 300);
        spy_seize(&mut g, 1, 400);
        assert_eq!((g.players[&0].team, g.players[&1].team), (SPIES, SPIES));
        assert_eq!(g.target_refusal(0, g.players[&1].obj), None, "spy on spy");
        let src = super::combat::DamageSource {
            weapon: 10,
            dtype: 0,
        };
        g.apply_damage(1, Some(0), 1000.0, [1.0, 0.0, 0.0], src, 2, 500);
        assert_ne!(
            g.players[&1].life,
            Life::Alive,
            "the spy's shot kills a spy"
        );
        assert_eq!(g.players[&1].team, HUNTERS);
        assert_eq!(g.spy().dropped(), 1);
        assert_eq!(g.spy().points_of(0), 2 * POINTS_INTEL + POINTS_KILL);
        let at = g
            .spy()
            .intel
            .iter()
            .find(|i| i.dropped_at.is_some())
            .unwrap()
            .pos;
        g.players.get_mut(&0).unwrap().pos = at;
        g.tick(600);
        assert_eq!(
            g.spy().carried.get(&0),
            Some(&3),
            "the killer took the case"
        );
        assert_eq!(g.spy().dropped(), 0);
    }

    /// DSV-451 (amended 2026-10-30): the agent's health and armour come from `spy.agent_health` / `spy.agent_armor`
    /// (the tuning), the client gets both maxima before the values; a hit takes from them unclamped.
    #[test]
    fn spy_hunt_the_agents_health_and_armour_come_from_the_config() {
        let mut g = spy_room();
        g.cfg.agent_health = 1500.0;
        g.cfg.agent_armor = 250.0;
        for k in 0..4 {
            spy_seize(&mut g, 2, 200 + 100 * k);
        }
        g.take_out();
        spy_seize(&mut g, 2, 700);
        assert_eq!(g.spy().agent, Some(2));
        let o = g.take_out();
        assert_eq!((g.players[&2].health, g.players[&2].armor), (1500.0, 250.0));
        let st = status_msgs(&o, 2);
        let at = |k: (u8, f32)| st.iter().position(|x| *x == k);
        let (mh, ma) = (
            at((gs::STATUS_MAX_HEALTH, 1500.0)),
            at((gs::STATUS_MAX_ARMOR, 250.0)),
        );
        let (h, a) = (
            at((gs::STATUS_HEALTH, 1500.0)),
            at((gs::STATUS_ARMOR, 250.0)),
        );
        assert!(
            mh.is_some() && ma.is_some() && h.is_some() && a.is_some(),
            "{st:?}"
        );
        assert!(mh < h && ma < a, "the maxima first: {st:?}");
        assert!(
            status_msgs(&o, 0)
                .iter()
                .all(|(k, _)| *k != gs::STATUS_MAX_HEALTH),
            "only to him"
        );
        // a hit takes from the agent's health above 100 (no clamp to the normal maximum)
        let src = super::combat::DamageSource {
            weapon: 10,
            dtype: 0,
        };
        g.apply_damage(2, Some(0), 300.0, [1.0, 0.0, 0.0], src, 2, 800);
        let p = &g.players[&2];
        assert_eq!(p.life, Life::Alive);
        assert!(
            p.health > 100.0 && p.health + p.armor < 1750.0,
            "{} / {}",
            p.health,
            p.armor
        );
    }

    /// DSV-452 (project decision): the agent's death ends the match at once with the hunters' result (the
    /// win value 1: the win picture for each hunter, the loss picture for the agent); his intel is not dropped, no
    /// case comes back, his weapon does not drop, nobody seizes anything.
    #[test]
    fn spy_hunt_the_agents_death_ends_the_match_for_the_hunters() {
        use super::spy::*;
        let mut g = spy_room();
        g.cfg.drops.enabled = true;
        for k in 0..5 {
            spy_seize(&mut g, 2, 200 + 100 * k);
        }
        assert_eq!(g.spy().agent, Some(2));
        assert_eq!(g.players[&2].health, AGENT_HEALTH);
        g.take_out();
        g.kill(2, Some(0), 10, false, 2_000);
        let o = g.take_out();
        assert!(
            matches!(g.phase(), RoundPhase::Over { win: 1, .. }),
            "{:?}",
            g.phase()
        );
        assert_eq!(g.spy().points_of(0), POINTS_AGENT_KILL);
        assert_eq!(
            g.spy().carried.get(&2),
            Some(&5),
            "the intel stays with him"
        );
        assert!(g.spy().intel.is_empty(), "no case dropped or placed");
        assert!(g.weapon_drops().is_empty(), "no weapon dropped");
        assert!(bounty_msgs(&o, 0)
            .iter()
            .all(|m| m.0 != SUB_DROPPED && m.0 != SUB_RESPAWNED));
        let win_of = |to: u8| {
            o.iter()
                .find_map(|x| match x {
                    GameOut::To(p, m) if *p == to && m.as_bytes()[1] == gs::GS_ROUND_RESULT => {
                        let mut r = m.reader();
                        r.skip_bits(16);
                        r.read_u32();
                        Some(r.read_s8())
                    }
                    _ => None,
                })
                .expect("a round result")
        };
        assert_eq!(win_of(0), HUNTERS as i8, "a hunter wins");
        assert_eq!(win_of(1), HUNTERS as i8, "a hunter wins");
        assert_ne!(win_of(2), g.players[&2].team as i8, "the agent loses");
        assert_ne!(win_of(2), 0);
        // nothing moves afterwards
        let at = [0.0, 0.0, 2000.0];
        g.players.get_mut(&1).unwrap().pos = at;
        g.tick(2_100);
        assert_eq!(g.spy().carried.get(&1), None);
    }

    /// DSV-230..DSV-236: a Spy Hunt room (class 9) places the intel at the mode's points; touching a case makes a spy
    /// (0xcb sub 0 team 2 with the count, sub 1 counters); a dead spy drops his intel and is a hunter again (sub 3);
    /// the spy with every case transmits in the uplink box and becomes the agent (sub 5 phase 1, sub 6 stage clock,
    /// sub 2 and the agent weapons, the agent health of DSV-451); the agent surviving the stage time ends stage 2
    /// (phase 0, intel back, sub 4, the normal health again); the time limit ends the match.
    #[test]
    fn spy_hunt_intel_spies_transmission_agent_and_points() {
        use super::spy::*;
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        let pts: Vec<[f32; 3]> = (0..6).map(|i| [i as f32 * 1000.0, 0.0, 2000.0]).collect();
        g.set_spy_world(crate::world::SpyWorld {
            intel_points: pts.clone(),
            uplinks: vec![crate::world::UplinkInfo {
                pos: [0.0, 0.0, -2000.0],
                dims: [40.0, 60.0, 40.0],
                secs: Some(15.0),
                team_text: None,
                name: b"up".to_vec(),
                locked: false,
                work: Vec::new(),
                release: Vec::new(),
                activate: Vec::new(),
            }],
            triggers: Vec::new(),
        });
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "9".into());
        room.pairs.insert("goal".into(), "30".into());
        g.set_room(&room);
        assert!(g.rules.team_damage, "everyone against everyone");
        g.tables.weapons.commands = vec![10, 20, 30];
        g.tables.weapons.class_ends = vec![1, 2, 3];
        for w in [10u16, 20, 30] {
            g.tables.weapons.weapons.insert(w, Default::default());
        }
        g.spy.gms.agent_weapons = vec![30, 10];
        for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3), (2, 9, 0, 5)] {
            let e = entry(uid, team, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        assert!(
            g.players.values().all(|p| p.team == HUNTERS),
            "everybody a hunter"
        );
        let far = [-9000.0, 0.0, -9000.0];
        for p in g.players.values_mut() {
            p.pos = far;
        }
        g.tick(100);
        let o = g.take_out();
        assert_eq!(g.spy().intel.len(), 5);
        assert!(g
            .spy()
            .intel
            .iter()
            .all(|i| pts.iter().any(|p| p[0] == i.pos[0] && p[2] == i.pos[2])));
        assert!(
            bounty_msgs(&o, 0).contains(&(1, 5, 0, 0)),
            "{:?}",
            bounty_msgs(&o, 0)
        );
        // player 0 seizes two cases
        let mut t = 200;
        for n in 1..=2u32 {
            let at = g.spy().intel[0].pos;
            g.players.get_mut(&0).unwrap().pos = at;
            g.tick(t);
            t += 100;
            let o = g.take_out();
            assert_eq!(g.players[&0].team, SPIES);
            assert!(
                bounty_msgs(&o, 1).contains(&(0, n, 2, 0)),
                "{:?}",
                bounty_msgs(&o, 1)
            );
            assert!(bounty_msgs(&o, 1).contains(&(1, 5, 0, 1)));
        }
        g.players.get_mut(&0).unwrap().pos = far;
        assert_eq!(g.spy().points_of(0), 2);
        // DSV-322: the seized intel is not a kill on the board, it is in the mission column
        let info = g.info_of(&g.players[&0], gs::INFO_UPDATE);
        assert_eq!((info.kills, info.mission[0]), (0, 2));
        // player 1 kills him: two cases lie at the body, he is a hunter again
        g.kill(0, Some(1), 10, false, t);
        let info = g.info_of(&g.players[&1], gs::INFO_UPDATE);
        assert_eq!((info.kills, info.mission[0]), (1, POINTS_KILL as u8));
        let o = g.take_out();
        assert_eq!(g.players[&0].team, HUNTERS);
        assert_eq!(g.spy().dropped(), 2);
        assert_eq!(g.spy().points_of(1), POINTS_KILL);
        let m = bounty_msgs(&o, 2);
        assert!(
            m.contains(&(0, 0, 1, 0)) && m.contains(&(3, 0, 0, 0)) && m.contains(&(1, 5, 2, 0)),
            "{m:?}"
        );
        // the dropped cases go back after the timer
        g.tick(t + RETURN_MS + 10);
        let o = g.take_out();
        assert_eq!(g.spy().dropped(), 0);
        assert_eq!(g.spy().intel.len(), 5);
        assert!(bounty_msgs(&o, 2).contains(&(4, 0, 0, 0)));
        t += RETURN_MS + 100;
        // DSV-323: player 2 seizes every case and is the agent at the last one (no transmission first)
        let mut o = Vec::new();
        for k in 0..5 {
            assert_eq!(g.spy().phase, PHASE_INTEL, "case {k}");
            let at = g.spy().intel[0].pos;
            g.players.get_mut(&2).unwrap().pos = at;
            g.tick(t);
            t += 100;
            o.extend(g.take_out());
        }
        assert_eq!(g.spy().carried.get(&2), Some(&5));
        g.players.get_mut(&2).unwrap().pos = far;
        assert_eq!(g.spy().phase, PHASE_AGENT);
        assert_eq!(g.spy().agent, Some(2));
        let m = bounty_msgs(&o, 0);
        assert!(
            m.contains(&(5, 1, 0, 0))
                && m.contains(&(0, 5, 2, 2))
                && m.contains(&(6, 180, CLOCK_STAGE2, 0)),
            "{m:?}"
        );
        assert!(
            bounty_msgs(&o, 2).contains(&(2, 0, 0, 0)),
            "the agent's loadout reset"
        );
        let held: Vec<u16> = g.players[&2].weapons.iter().map(|w| w.weapon).collect();
        assert_eq!(held, vec![30, 10]);
        assert_eq!(g.spy().points_of(2), 5 * POINTS_INTEL + POINTS_AGENT);
        // DSV-451: the agent's health and its maximum go to him
        assert_eq!(g.players[&2].health, AGENT_HEALTH);
        assert!(
            AGENT_HEALTH >= 5.0 * g.cfg.max_health,
            "a lot more than a hunter"
        );
        let statuses = status_msgs(&o, 2);
        assert!(
            statuses.contains(&(gs::STATUS_MAX_HEALTH, AGENT_HEALTH))
                && statuses.contains(&(gs::STATUS_HEALTH, AGENT_HEALTH)),
            "{statuses:?}"
        );
        // DSV-451 amended 2026-10-30: 1000 health, 500 armour by default, both maxima sent
        assert_eq!((AGENT_HEALTH, AGENT_ARMOR), (1000.0, 500.0));
        assert_eq!(g.players[&2].armor, AGENT_ARMOR);
        assert!(
            statuses.contains(&(gs::STATUS_MAX_ARMOR, AGENT_ARMOR))
                && statuses.contains(&(gs::STATUS_ARMOR, AGENT_ARMOR)),
            "{statuses:?}"
        );
        // the agent survives the stage time: stage 1 again, the intel back at the points, the normal health maximum
        t += DEFAULT_STAGE2_SECS * 1000 + 100;
        g.tick(t);
        let o = g.take_out();
        assert_eq!(g.spy().phase, PHASE_INTEL);
        assert_eq!(g.spy().agent, None);
        assert_eq!(g.players[&2].team, HUNTERS);
        assert_eq!(g.players[&2].health, g.cfg.max_health);
        assert!(status_msgs(&o, 2).contains(&(gs::STATUS_MAX_HEALTH, g.cfg.max_health)));
        assert_eq!(
            g.spy().points_of(2),
            5 * POINTS_INTEL + POINTS_AGENT + POINTS_AGENT_SURVIVES
        );
        assert_eq!(g.spy().intel.len(), 5);
        let m = bounty_msgs(&o, 1);
        assert!(
            m.contains(&(5, 0, 0, 0)) && m.contains(&(4, 0, 0, 0)),
            "{m:?}"
        );
        assert!(matches!(g.phase(), RoundPhase::Running { .. }));
        // DSV-392: points past the room goal end nothing; the time limit ends the match and the most points win
        g.spy.points.insert(1, 29);
        g.take_out();
        g.kill(0, Some(1), 10, false, t + 100);
        assert!(g.spy().points_of(1) >= g.rules.goal);
        assert!(
            matches!(g.phase(), RoundPhase::Running { .. }),
            "the goal ends nothing"
        );
        g.rules.round_secs = 1;
        g.tick(t + 200);
        assert!(matches!(g.phase(), RoundPhase::Over { .. }));
        // DSV-280: the result value per receiver: the leader's own team for player 1, another value for the rest
        // (one broadcast value 1 showed every hunter the win picture)
        let o = g.take_out();
        let win_of = |to: u8| {
            o.iter()
                .find_map(|x| match x {
                    GameOut::To(p, m) if *p == to && m.as_bytes()[1] == gs::GS_ROUND_RESULT => {
                        let mut r = m.reader();
                        r.skip_bits(16);
                        r.read_u32();
                        Some(r.read_s8())
                    }
                    _ => None,
                })
                .expect("a round result")
        };
        for pid in 0..3u8 {
            let own = g.players[&pid].team as i8;
            if pid == 1 {
                assert_eq!(win_of(pid), own, "the leader sees the win picture");
            } else {
                assert!(
                    win_of(pid) != own && win_of(pid) != 0,
                    "player {pid}: {}",
                    win_of(pid)
                );
            }
        }
    }

    /// DSV-450 (project decision, replaces DSV-390): in Spy Hunt everybody hurts everybody: a spy's hit
    /// claim on another spy is not refused, hunters hurt hunters (DSV-236), spies and hunters each other.
    #[test]
    fn spy_hunt_hunters_are_a_team_and_spies_hurt_everyone() {
        use super::spy::*;
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "9".into());
        g.set_room(&room);
        for (pid, uid, obj) in [(0u8, 7u32, 1u16), (1, 8, 3), (2, 9, 5), (3, 10, 7)] {
            let e = entry(uid, 0, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        for p in g.players.values_mut() {
            p.life = Life::Alive;
        }
        g.spawn_notes.clear();
        g.players.get_mut(&0).unwrap().team = SPIES;
        g.players.get_mut(&1).unwrap().team = SPIES;
        assert_eq!(g.target_refusal(0, 3), None, "spy on spy");
        assert_eq!(g.target_refusal(1, 1), None, "spy on spy");
        assert_eq!(g.target_refusal(0, 5), None, "spy on hunter");
        assert_eq!(g.target_refusal(2, 1), None, "hunter on spy");
        // DSV-453 (amended 2026-10-31): the hunters are a team whatever the room's friendly fire says
        assert_eq!(
            g.target_refusal(2, 7),
            Some(super::hits::HitVerdict::Friendly),
            "hunter on hunter"
        );
        assert!(g.team_may_hurt(SPIES, SPIES));
        assert!(!g.team_may_hurt(HUNTERS, HUNTERS));
        assert!(g.team_may_hurt(SPIES, HUNTERS) && g.team_may_hurt(HUNTERS, SPIES));
        // other modes keep the room's friendly fire for the same team
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "1".into());
        g.set_room(&room);
        assert!(!g.team_may_hurt(2, 2) && g.team_may_hurt(1, 2));
        room.pairs.insert("ff".into(), "1".into());
        g.set_room(&room);
        assert!(g.team_may_hurt(2, 2));
    }

    /// DSV-450: a thrown blast of a spy hurts spies and hunters alike (DSV-390's spared spies are gone).
    #[test]
    fn spy_hunt_thrown_blast_of_a_spy_hurts_spies() {
        use super::spy::*;
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "9".into());
        g.set_room(&room);
        for (pid, uid, obj) in [(0u8, 7u32, 1u16), (1, 8, 3), (2, 9, 5)] {
            let e = entry(uid, 0, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        for p in g.players.values_mut() {
            p.life = Life::Alive;
        }
        g.spawn_notes.clear();
        g.flights.push(projectile::Flight {
            key: 1,
            owner: 0,
            owner_obj: g.players[&0].obj,
            owner_team: HUNTERS,
            weapon: 0,
            ammo: 0,
            origin: [0.0; 3],
            pos: [0.0; 3],
            vel: [0.0; 3],
            gravity: 0.0,
            detonate_at: 100,
            last: 100,
            resting: true,
            impact: false,
            burst: false,
            mine: None,
        });
        // the thrower took a case before it landed; player 1 is a spy, player 2 a hunter
        g.players.get_mut(&0).unwrap().team = SPIES;
        g.players.get_mut(&1).unwrap().team = SPIES;
        g.players.get_mut(&2).unwrap().team = HUNTERS;
        g.tick_projectiles(100);
        assert!(g.players[&1].health < 100.0, "the spy is hurt");
        assert!(g.players[&2].health < 100.0, "the hunter is hurt");
    }

    /// DSV-393: a Spy Hunt room with time 0 runs to the fallback time; other modes keep 0 = no timer.
    #[test]
    fn spy_hunt_time_zero_falls_back_to_the_default_round_time() {
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "9".into());
        room.pairs.insert("time".into(), "0".into());
        g.set_room(&room);
        assert_eq!(g.rules.round_secs, super::spy::FALLBACK_SECS);
        assert_eq!(super::spy::FALLBACK_SECS, 600);
        room.pairs.insert("time".into(), "3".into());
        g.set_room(&room);
        assert_eq!(g.rules.round_secs, 180, "a given time is kept");
        room.pairs.insert("type".into(), "7".into());
        room.pairs.insert("time".into(), "0".into());
        g.set_room(&room);
        assert_eq!(g.rules.round_secs, 0, "CTF stays untimed");
    }

    /// DSV-391 (user live report "high above an intel it auto-picked it up"): the touch has a height window: a
    /// player on a ledge or jumping over the case does not take it, one walking or landing on it (or with the case
    /// up to his head) does.
    #[test]
    fn spy_hunt_intel_touch_has_a_height_window() {
        use super::spy::*;
        let case = [100.0, 50.0, 100.0];
        let at = |dy: f32, dx: f32| [case[0] + dx, case[1] + dy, case[2]];
        assert!(touching(at(0.0, 0.0), case), "standing at it");
        assert!(touching(at(20.0, 30.0), case), "on a step beside it");
        assert!(touching(at(-40.0, 0.0), case), "the case at head height");
        assert!(!touching(at(50.0, 0.0), case), "jump apex over it");
        assert!(
            !touching(at(90.0, 0.0), case),
            "on a ledge above it (the old 96-unit rule took it)"
        );
        assert!(!touching(at(-70.0, 0.0), case), "under it");
        assert!(!touching(at(0.0, 60.0), case), "too far across");
        // in a room: a player above the case keeps away from it, the same player on its floor takes it
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        g.set_spy_world(crate::world::SpyWorld {
            intel_points: vec![[0.0, 0.0, 2000.0]],
            uplinks: Vec::new(),
            triggers: Vec::new(),
        });
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "9".into());
        g.set_room(&room);
        g.spy.gms.total = 1;
        let e = entry(7, 0, "P");
        g.note_identified(&e);
        g.add_player(0, Some(&e), None, 1, [0.0; 3], [0.0; 3], 0);
        g.on_up(0, Up::PlayerInfo(Box::default()), 10);
        g.on_up(0, Up::WorldReady, 20);
        g.players.get_mut(&0).unwrap().pos = [-9000.0, 0.0, -9000.0];
        g.tick(100);
        let c = g.spy().intel[0].pos;
        g.players.get_mut(&0).unwrap().pos = [c[0], c[1] + 80.0, c[2]];
        g.tick(200);
        assert_eq!(g.spy().intel.len(), 1, "not from above");
        assert_eq!(g.players[&0].team, HUNTERS);
        g.players.get_mut(&0).unwrap().pos = c;
        g.tick(300);
        assert!(g.spy().intel.is_empty(), "taken on its floor");
    }

    /// DSV-323 (playtest report "captured all 5 intel but did not become the super spy or trigger the upload
    /// station"): the last seized case makes the agent at once with the GMS stage-2 banner images (the agent's own
    /// comment); the mode file's trigger brings the station (its effects on, the box unlocked after its delay); the
    /// agent working the open box starts its work command, leaving runs its release command, 15 s inside runs the
    /// activate command (finish trigger, lock, defence win): the match ends with the agent the winner.
    #[test]
    fn spy_hunt_agent_brings_the_station_and_the_upload_wins() {
        use super::spy::*;
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        let pts: Vec<[f32; 3]> = (0..5).map(|i| [i as f32 * 1000.0, 0.0, 2000.0]).collect();
        let box_at = [0.0, 0.0, -2000.0];
        g.set_spy_world(crate::world::SpyWorld {
            intel_points: pts,
            uplinks: vec![crate::world::UplinkInfo {
                pos: box_at,
                dims: [40.0, 60.0, 40.0],
                secs: Some(15.0),
                team_text: None,
                name: b"Up".to_vec(),
                locked: true,
                work: b"msg Rdy (visible 0);\r\nmsg Trn on;".to_vec(),
                release: b"msg Trn (visible 0);\r\nmsg Rdy on;".to_vec(),
                activate: concat!("msg Fin trigger;\r\nmsg Up lock;\r\nmsg Ctl defence", "win;")
                    .as_bytes()
                    .to_vec(),
            }],
            triggers: vec![
                (
                    b"Init".to_vec(),
                    vec![
                        b"msg Mdl on;\r\ndelay 7 (msg Wall (solid 1));".to_vec(),
                        b"delay 40 (msg Mdl (visible 0));\r\ndelay 40 (msg Rdy on);\r\ndelay 40 (msg Up unlock);"
                            .to_vec(),
                    ],
                ),
                (b"Fin".to_vec(), vec![b"msg Trn (visible 0);\r\nmsg FinFx on;".to_vec()]),
            ],
        });
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "9".into());
        // DSV-392 (user live report: the room goal 8 ended the match at a seizure): the lowest ROM goal; the five
        // cases and the agent's points pass it, the match goes on to the station
        room.pairs.insert("goal".into(), "8".into());
        g.set_room(&room);
        g.spy.gms.agent_trigger = Some("init".into());
        g.spy.gms.stage2_title = Some("T.dtx".into());
        g.spy.gms.hunter_comment = Some("H.dtx".into());
        g.spy.gms.bounty_comment = Some("B.dtx".into());
        for (pid, uid, obj) in [(0u8, 7u32, 1u16), (1, 8, 3)] {
            let e = entry(uid, 0, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        let far = [-9000.0, 0.0, -9000.0];
        for p in g.players.values_mut() {
            p.pos = far;
        }
        g.tick(100);
        g.take_out();
        let fx = |o: &[GameOut]| -> Vec<(bool, String)> {
            o.iter()
                .filter_map(|x| match x {
                    GameOut::WorldFx { name, .. } => {
                        Some((true, String::from_utf8_lossy(name).into_owned()))
                    }
                    GameOut::WorldFxGone { key } => Some((false, format!("{key:x}"))),
                    _ => None,
                })
                .collect()
        };
        let banner = |o: &[GameOut], to: u8| -> Option<(Vec<u8>, Vec<u8>)> {
            o.iter().find_map(|x| match x {
                GameOut::To(p, m) if *p == to && m.as_bytes()[1] == gs::GS_BOUNTY_MODE => {
                    let mut r = m.reader();
                    r.skip_bits(16);
                    (r.read_u8() == SUB_PHASE && r.read_u8() == PHASE_AGENT)
                        .then(|| (r.read_zstr8(), r.read_zstr8()))
                }
                _ => None,
            })
        };
        let mut t = 200;
        let mut o = Vec::new();
        for _ in 0..5 {
            let at = g.spy().intel[0].pos;
            g.players.get_mut(&1).unwrap().pos = at;
            g.tick(t);
            t += 100;
            o.extend(g.take_out());
        }
        assert_eq!(g.spy().agent, Some(1), "the agent at the last case");
        assert!(g.spy().points_of(1) >= g.rules.goal);
        assert!(
            matches!(g.phase(), RoundPhase::Running { .. }),
            "no goal end"
        );
        assert_eq!(g.spy().phase, PHASE_AGENT);
        assert_eq!(banner(&o, 1), Some((b"T.dtx".to_vec(), b"B.dtx".to_vec())));
        assert_eq!(banner(&o, 0), Some((b"T.dtx".to_vec(), b"H.dtx".to_vec())));
        assert_eq!(fx(&o), vec![(true, "Mdl".to_string())], "the station comes");
        // in the locked box nothing happens
        let t0 = t;
        g.players.get_mut(&1).unwrap().pos = box_at;
        g.tick(t0 + 1000);
        assert!(fx(&g.take_out()).is_empty());
        assert!(g.spy().station.working.is_none());
        // 40 s after the agent: the station is ready and the box open; the agent works it
        g.tick(t0 + 40_000);
        let o = g.take_out();
        let f = fx(&o);
        assert!(f.contains(&(true, "Rdy".into())), "{f:?}");
        assert!(f.contains(&(true, "Trn".into())), "work command: {f:?}");
        assert!(g.spy().station.working.is_some());
        // leaving runs the release command
        g.players.get_mut(&1).unwrap().pos = far;
        g.tick(t0 + 41_000);
        let f = fx(&g.take_out());
        assert!(f.contains(&(true, "Rdy".into())), "release command: {f:?}");
        assert!(g.spy().station.working.is_none());
        // 15 s inside: the finish trigger, the box locked, the defence win: the agent wins the match
        g.players.get_mut(&1).unwrap().pos = box_at;
        g.tick(t0 + 42_000);
        g.tick(t0 + 56_900);
        assert!(matches!(g.phase(), RoundPhase::Running { .. }), "not yet");
        g.take_out();
        g.tick(t0 + 57_100);
        let o = g.take_out();
        assert!(fx(&o).contains(&(true, "FinFx".into())), "{:?}", fx(&o));
        assert!(matches!(g.phase(), RoundPhase::Over { .. }));
        assert_eq!(g.spy().winner, Some(1));
        // DSV-472: the report names the agent as the one winner, per player, and no team win for the lobby
        let pipe: Vec<&String> = o
            .iter()
            .filter_map(|x| match x {
                GameOut::Pipe(l) => Some(l),
                _ => None,
            })
            .collect();
        let players: Vec<&&String> = pipe.iter().filter(|l| l.starts_with("@player")).collect();
        assert!(players.len() >= 2, "{pipe:?}");
        assert_eq!(
            players
                .iter()
                .filter(|l| l.contains(" win=1 agent=1 name="))
                .count(),
            1,
            "{pipe:?}"
        );
        assert!(
            players
                .iter()
                .all(|l| l.contains(" win=1 agent=1 ") || l.contains(" win=0 agent=0 ")),
            "{pipe:?}"
        );
        assert!(
            pipe.iter()
                .any(|l| l.starts_with("@result") && l.contains(" win=0 ")),
            "{pipe:?}"
        );
        assert_eq!(
            g.spy().points_of(1),
            5 * POINTS_INTEL + POINTS_AGENT + POINTS_UPLOAD
        );
        let win = o
            .iter()
            .find_map(|x| match x {
                GameOut::To(1, m) if m.as_bytes()[1] == gs::GS_ROUND_RESULT => {
                    let mut r = m.reader();
                    r.skip_bits(16);
                    r.read_u32();
                    Some(r.read_s8())
                }
                _ => None,
            })
            .expect("a round result");
        assert_eq!(
            win, g.players[&1].team as i8,
            "the agent sees the win picture"
        );
    }

    /// DSV-210..DSV-213: a Quarantine room (class 11) puts everybody on the human team, starts the 20 s countdown
    /// (0xd3 sub 1), infects one human at the outbreak (0xd3 sub 0 team 2 with the infected look, 1000 health); an
    /// infected gun hit does nothing, a claw hit infects; a dead infected stays dead (DSV-213 amended); the last human
    /// infected gives the round to the infected.
    // DC-DEFAULT: DSV-250 the lobby's Quarantine line (session 20261028_123650: `time=3`, the ROM
    // mode file's minutes) gives a 3-minute round with an outbreak, not a 3-second round won by the humans
    #[test]
    fn the_lobby_quarantine_time_is_minutes_and_the_round_has_an_outbreak() {
        use super::infect::COUNTDOWN_SECS;
        assert_eq!(super::room_time_secs(3), 180);
        assert_eq!(super::room_time_secs(10), 600);
        assert_eq!(super::room_time_secs(180), 180);
        assert_eq!(super::room_time_secs(0), 0);
        assert_eq!(super::room_time_secs(-5), 0);
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        let mut room = RoomSettings::default();
        for (k, v) in [
            ("key", "QRT"),
            ("type", "11"),
            ("goal", "7"),
            ("ff", "1"),
            ("time", "3"),
            ("train", "10"),
            ("qrt", "20,3,-1,2,0,20,3"),
        ] {
            room.pairs.insert(k.into(), v.into());
        }
        g.set_room(&room);
        assert_eq!(g.rules.round_secs, 180);
        for (pid, uid, obj) in [(0u8, 7u32, 1u16), (1, 8, 3), (2, 9, 5)] {
            let e = entry(uid, 0, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        g.tick(100);
        g.take_out();
        g.tick(5_000);
        assert!(
            matches!(g.phase, RoundPhase::Running { .. }),
            "still running after 5 s"
        );
        g.tick(100 + COUNTDOWN_SECS * 1000 + 50);
        assert_eq!(g.infect().infected.len(), 1, "outbreak");
        assert!(matches!(g.phase, RoundPhase::Running { .. }));
    }

    #[test]
    fn quarantine_outbreak_claws_respawn_and_round_end() {
        use super::infect::{COUNTDOWN_SECS, INFECTED_HEALTH};
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "11".into());
        room.pairs.insert("goal".into(), "2".into());
        g.set_room(&room);
        g.tables.weapons.commands = vec![10, 20, 30];
        g.tables.weapons.class_ends = vec![1, 2, 3];
        for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3), (2, 9, 0, 5)] {
            let e = entry(uid, team, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        assert!(g.players.values().all(|p| p.team == 1), "everybody human");
        g.tick(100);
        let o = g.take_out();
        assert_eq!(
            infection_msgs(&o, 0),
            vec![(1, 1, 2, COUNTDOWN_SECS as i32)]
        );
        g.tick(100 + COUNTDOWN_SECS * 1000 - 200);
        assert!(g.infect().infected.is_empty());
        g.tick(100 + COUNTDOWN_SECS * 1000 + 50);
        let o = g.take_out();
        assert_eq!(g.infect().infected.len(), 1);
        let host = *g.infect().infected.iter().next().unwrap();
        assert_eq!(
            infection_msgs(&o, 0),
            vec![(
                0,
                2,
                u32::from(host),
                i32::from(super::infect::HOST_OVERRIDE)
            )]
        );
        assert_eq!(g.players[&host].team, 2);
        assert_eq!(g.players[&host].health, INFECTED_HEALTH);
        let humans: Vec<u8> = (0..3).filter(|p| *p != host).collect();
        let t = 30_000;
        let src = |weapon| super::combat::DamageSource { weapon, dtype: 0 };
        // the infected's gun does nothing
        let h0 = g.players[&humans[0]].health;
        g.apply_damage(humans[0], Some(host), 50.0, [1.0, 0.0, 0.0], src(10), 2, t);
        assert_eq!(g.players[&humans[0]].health, h0);
        assert_eq!(g.players[&humans[0]].team, 1);
        // the claws infect
        g.apply_damage(humans[0], Some(host), 50.0, [1.0, 0.0, 0.0], src(30), 2, t);
        assert_eq!(g.players[&humans[0]].team, 2);
        assert_eq!(g.infect().infections.get(&host), Some(&1));
        assert!(matches!(g.phase(), RoundPhase::Running { .. }));
        // a dead infected stays dead while another infected lives (the round goes on)
        g.kill(host, Some(humans[1]), 10, false, t + 100);
        assert!(matches!(g.phase(), RoundPhase::Running { .. }));
        g.tick(t + 200);
        g.take_out();
        g.tick(t + 200 + 10_000);
        assert!(
            !matches!(g.players[&host].life, Life::Alive),
            "no infected respawn"
        );
        assert!(!shell_ids(&g.take_out(), host).contains(&gs::GS_MATCH_BEGIN));
        assert_eq!(g.players[&host].team, 2);
        // the last human infected: round to the infected
        g.apply_damage(
            humans[1],
            Some(humans[0]),
            50.0,
            [1.0, 0.0, 0.0],
            src(30),
            2,
            t + 9000,
        );
        assert!(matches!(g.phase(), RoundPhase::Over { win: 2, .. }));
    }

    /// A three-player Quarantine room (class 11, `goal` rounds); `ready`: every player is in the world.
    fn quarantine_room(goal: &str, ready: bool) -> Game {
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "11".into());
        room.pairs.insert("goal".into(), goal.into());
        g.set_room(&room);
        g.tables.weapons.commands = vec![10, 20, 30];
        g.tables.weapons.class_ends = vec![1, 2, 3];
        for (pid, uid, obj) in [(0u8, 7u32, 1u16), (1, 8, 3), (2, 9, 5)] {
            let e = entry(uid, 0, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            if ready {
                g.on_up(pid, Up::WorldReady, 20);
            }
        }
        g
    }

    /// DSV-213 (amended 2026-11-03): every infected dead after the outbreak gives the round to the humans; nobody
    /// is brought back.
    #[test]
    fn quarantine_round_goes_to_the_humans_when_every_infected_is_dead() {
        use super::infect::COUNTDOWN_SECS;
        let mut g = quarantine_room("2", true);
        g.tick(100);
        g.tick(100 + COUNTDOWN_SECS * 1000 + 50);
        let host = *g.infect().infected.iter().next().expect("outbreak");
        let human = (0..3u8).find(|p| *p != host).unwrap();
        assert!(matches!(g.phase(), RoundPhase::Running { .. }));
        g.kill(host, Some(human), 10, false, 30_000);
        g.tick(30_100);
        assert!(
            matches!(g.phase(), RoundPhase::Over { win: 1, .. }),
            "{:?}",
            g.phase()
        );
    }

    /// DSV-607 amended: a poison hazard step below the lethal rate spares an infected player and hurts a human; at
    /// the lethal rate (deadly water) it hurts the infected as the human; a non-poison step hurts the infected.
    #[test]
    fn quarantine_poison_spares_the_infected_below_the_lethal_rate_only() {
        use super::infect::{COUNTDOWN_SECS, INFECTED_LETHAL_HAZARD_RATE};
        use crate::killvol::DAMAGE_TYPE_POISON;
        let mut g = quarantine_room("2", true);
        g.tick(100);
        g.tick(100 + COUNTDOWN_SECS * 1000 + 50);
        let host = *g.infect().infected.iter().next().expect("outbreak");
        let human = (0..3u8).find(|p| *p != host).unwrap();
        assert!(matches!(g.phase(), RoundPhase::Running { .. }));
        let health = |g: &Game, p: u8| g.players[&p].health;
        let below = INFECTED_LETHAL_HAZARD_RATE - 1.0;
        let (h0, z0) = (health(&g, human), health(&g, host));
        g.hazard_damage(host, 5.0, DAMAGE_TYPE_POISON, below, 21_000);
        g.hazard_damage(human, 5.0, DAMAGE_TYPE_POISON, below, 21_000);
        assert_eq!(health(&g, host), z0, "gas spares the infected");
        assert!(health(&g, human) < h0, "gas hurts the human");
        g.hazard_damage(
            host,
            5.0,
            DAMAGE_TYPE_POISON,
            INFECTED_LETHAL_HAZARD_RATE,
            21_500,
        );
        assert!(health(&g, host) < z0, "deadly water hurts the infected");
        let z1 = health(&g, host);
        g.hazard_damage(host, 5.0, 0, below, 22_000);
        assert!(
            health(&g, host) < z1,
            "a non-poison volume hurts the infected"
        );
        // the lethal step kills him with no killer, as a human
        let lethal = health(&g, host) + 1.0;
        g.hazard_damage(
            host,
            lethal,
            DAMAGE_TYPE_POISON,
            INFECTED_LETHAL_HAZARD_RATE * 10.0,
            22_500,
        );
        assert_ne!(
            g.players[&host].life,
            Life::Alive,
            "the water kills the infected"
        );
    }

    /// DSV-565: a 7-round Quarantine ends after round 7 whoever wins each round (humans, infected, draws, mixed);
    /// before, the goal counted round wins, so a human win made the infected need an 8th round.
    #[test]
    fn quarantine_plays_exactly_the_goal_in_rounds_whoever_wins() {
        use super::infect::COUNTDOWN_SECS;
        let patterns: [[u8; 7]; 5] = [
            [1, 1, 1, 1, 1, 1, 1],
            [2, 2, 2, 2, 2, 2, 2],
            [2, 1, 2, 2, 2, 2, 2],
            [0, 0, 1, 0, 2, 0, 1],
            [1, 2, 1, 2, 0, 1, 2],
        ];
        for wins in patterns {
            let mut g = quarantine_room("7", true);
            let mut t = 100;
            g.tick(t);
            for (i, &w) in wins.iter().enumerate() {
                while !matches!(g.phase(), RoundPhase::Running { .. }) {
                    t += 100;
                    g.tick(t);
                    assert!(t < 2_000_000, "{wins:?}: no round {}", i + 1);
                }
                assert_eq!(g.round_no(), i as u32 + 1, "{wins:?}");
                // past the outbreak, so the round is a played Quarantine round
                t += COUNTDOWN_SECS * 1000 + 50;
                g.tick(t);
                assert!(matches!(g.phase(), RoundPhase::Running { .. }), "{wins:?}");
                g.end_round(t, w);
                let last = matches!(g.phase(), RoundPhase::Over { last: true, .. });
                assert_eq!(last, i == 6, "{wins:?}: round {} last {last}", i + 1);
            }
            let humans = wins.iter().filter(|&&w| w == 1).count() as u32;
            let infected = wins.iter().filter(|&&w| w == 2).count() as u32;
            assert_eq!(g.round_wins(), [humans, infected], "{wins:?}");
            let lead = super::round::lead_of([humans, infected]);
            assert!(
                matches!(g.phase(), RoundPhase::Over { last: true, win, .. } if win == lead),
                "{wins:?}: {:?}",
                g.phase()
            );
            // no 8th round follows
            for _ in 0..600 {
                t += 100;
                g.tick(t);
            }
            assert_eq!(g.round_no(), 7, "{wins:?}");
            assert!(!matches!(g.phase(), RoundPhase::Running { .. }), "{wins:?}");
        }
    }

    /// DSV-565 through the real round ends: humans surviving the clock, the last human infected and every infected
    /// dead each count as one of the goal's rounds.
    #[test]
    fn quarantine_counts_each_kind_of_round_end() {
        use super::infect::COUNTDOWN_SECS;
        let mut g = quarantine_room("3", true);
        g.rules.round_secs = 60;
        let secs = g.rules.round_secs;
        let mut t = 100;
        g.tick(t);
        let wait_running = |g: &mut Game, t: &mut u32| {
            while !matches!(g.phase(), RoundPhase::Running { .. }) {
                *t += 100;
                g.tick(*t);
                assert!(*t < 3_000_000, "no next round");
            }
        };
        // round 1: every infected dead -> humans
        t += COUNTDOWN_SECS * 1000 + 50;
        g.tick(t);
        let host = *g.infect().infected.iter().next().expect("outbreak");
        let human = (0..3u8).find(|p| *p != host).unwrap();
        g.kill(host, Some(human), 10, false, t);
        t += 100;
        g.tick(t);
        assert!(
            matches!(
                g.phase(),
                RoundPhase::Over {
                    win: 1,
                    last: false,
                    ..
                }
            ),
            "{:?}",
            g.phase()
        );
        // round 2: the clock runs out with humans alive -> humans
        wait_running(&mut g, &mut t);
        assert_eq!(g.round_no(), 2);
        let start = t;
        while matches!(g.phase(), RoundPhase::Running { .. }) {
            t += 500;
            g.tick(t);
            assert!(t < start + secs * 1000 + 30_000, "no timeout");
        }
        assert!(
            matches!(
                g.phase(),
                RoundPhase::Over {
                    win: 1,
                    last: false,
                    ..
                }
            ),
            "{:?}",
            g.phase()
        );
        // round 3: no human left -> infected, and the match is over after the goal's 3 rounds
        wait_running(&mut g, &mut t);
        assert_eq!(g.round_no(), 3);
        t += COUNTDOWN_SECS * 1000 + 50;
        g.tick(t);
        let host = *g.infect().infected.iter().next().expect("outbreak");
        let claws = super::combat::DamageSource {
            weapon: 30,
            dtype: 0,
        };
        for p in (0..3u8).filter(|p| *p != host) {
            g.apply_damage(p, Some(host), 50.0, [1.0, 0.0, 0.0], claws, 2, t);
        }
        t += 100;
        g.tick(t);
        assert!(
            matches!(
                g.phase(),
                RoundPhase::Over {
                    last: true,
                    win: 1,
                    ..
                }
            ),
            "{:?}",
            g.phase()
        );
        assert_eq!(g.round_wins(), [2, 1]);
    }

    /// DSV-510: the infected's health is the override row's `MaxHitPoints` x 2 (1000 without the row).
    #[test]
    fn quarantine_infected_health_doubles_the_row() {
        use super::infect::{INFECTED_HEALTH, INFECTED_HEALTH_SCALE};
        let mut g = quarantine_room("2", true);
        assert_eq!(g.infected_health(-1), INFECTED_HEALTH);
        g.tables.chars.overrides.insert(
            0,
            crate::chars::OverrideRow {
                max_hit_points: Some(225),
                ..Default::default()
            },
        );
        assert_eq!(g.infected_health(0), 225.0 * INFECTED_HEALTH_SCALE);
        assert_eq!(INFECTED_HEALTH_SCALE, 2.0);
    }

    /// DSV-608: in Quarantine a head hit on an infected player takes half the head node's factor (never below 1);
    /// his body hits, a head node of factor 1 and every hit on a human keep their factor; `combat.infected_head_scale
    /// = 1` gives the infected the human factor again. The fire path applies it (health loss = damage x factor with
    /// no armour).
    #[test]
    fn quarantine_head_hits_on_the_infected_take_half_the_head_factor() {
        use super::infect::{COUNTDOWN_SECS, INFECTED_HEAD_SCALE};
        use crate::chars::HitNode;
        let mut t = character_tables();
        t.chars.bodies.get_mut(&0).unwrap().skeleton = Some(1);
        // node 0 torso 1.0, node 1 head 6.0, node 2 head 1.0
        let node = |factor: f32, location: u8| HitNode { factor, location };
        t.chars
            .skeletons
            .insert(1, vec![node(1.0, 2), node(6.0, 1), node(1.0, 1)]);
        let mut g = Game::new(
            GameTuning {
                move_check: false,
                trace: false,
                spawn_shield: false,
                armor_gear: ArmorGear::Off,
                ..GameTuning::default()
            },
            &world(),
            t,
        );
        g.insert_rig("BODY\\b.ltb", rig());
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "11".into());
        room.pairs.insert("goal".into(), "2".into());
        g.set_room(&room);
        for (pid, uid, obj) in [(0u8, 7u32, 1u16), (1, 8, 3), (2, 9, 5)] {
            let e = entry(uid, 0, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        g.tick(100);
        g.tick(100 + COUNTDOWN_SECS * 1000 + 50);
        let host = *g.infect().infected.iter().next().expect("outbreak");
        let humans: Vec<u8> = (0..3u8).filter(|p| *p != host).collect();
        let (a, b) = (humans[0], humans[1]);
        assert_eq!(INFECTED_HEAD_SCALE, 0.5);
        let zone = |g: &Game, shooter: u8, victim: u8, part: u8| {
            g.hit_zone(
                victim,
                &hit_report(g, shooter, victim, 0.0, Some(part)),
                false,
            )
        };
        assert_eq!(
            zone(&g, a, host, 1),
            (3.0, gs::KILL_ZONE_HEAD),
            "infected head"
        );
        assert_eq!(zone(&g, a, host, 0), (1.0, 2), "infected body");
        assert_eq!(
            zone(&g, a, host, 2),
            (1.0, gs::KILL_ZONE_HEAD),
            "head node 1.0"
        );
        assert_eq!(zone(&g, a, b, 1), (6.0, gs::KILL_ZONE_HEAD), "human head");
        assert_eq!(zone(&g, a, b, 0), (1.0, 2), "human body");
        // the box geometry (no node named): the infected head takes half the operator's head factor, at least 1
        let f = hit_report(&g, a, host, 0.0, None);
        assert_eq!(
            g.hit_zone(host, &f, true),
            ((g.cfg.head_factor * 0.5).max(1.0), gs::KILL_ZONE_HEAD)
        );
        // the fire path: a body hit, then a head hit on the infected; the head loses three times as much
        let mut now = 30_000;
        let mut loss = |g: &mut Game, part: u8| {
            let before = g.players[&host].health;
            let f = hit_report(g, a, host, 0.0, Some(part));
            g.on_up(a, Up::Fire(Box::new(f)), now);
            now += 500;
            before - g.players[&host].health
        };
        let body = loss(&mut g, 0);
        let head = loss(&mut g, 1);
        assert!(body > 0.0, "the body hit is accepted");
        assert!((head - 3.0 * body).abs() < 1e-3, "head {head} body {body}");
        // the switch: the human factor for the infected too
        g.cfg.set_combat("infected_head_scale", "1").unwrap();
        assert_eq!(zone(&g, a, host, 1), (6.0, gs::KILL_ZONE_HEAD));
        assert!(g.cfg.set_combat("infected_head_scale", "-1").is_err());
    }

    /// DSV-511: a player who enters the world after the round start gets the running outbreak countdown with the
    /// seconds left; in the next round every player gets it after his round begin (0x44), not before.
    #[test]
    fn quarantine_countdown_reaches_a_late_loader_and_follows_each_round_begin() {
        use super::infect::COUNTDOWN_SECS;
        let mut g = quarantine_room("3", false);
        // the first (a bot) is ready at once and starts the round; the others load for 6 s
        g.on_up(0, Up::WorldReady, 20);
        g.tick(100);
        let o = g.take_out();
        assert_eq!(
            infection_msgs(&o, 0),
            vec![(1, 1, 3, COUNTDOWN_SECS as i32)]
        );
        // a loading client may get it too (introduced at its player info), but its world entry resets its HUD
        g.on_up(1, Up::WorldReady, 6_100);
        g.on_up(2, Up::WorldReady, 6_100);
        let o = g.take_out();
        for pid in [1u8, 2] {
            assert_eq!(
                infection_msgs(&o, pid),
                vec![(1, 1, 3, (COUNTDOWN_SECS - 6) as i32)],
                "client {pid}"
            );
        }
        // round 1 ends (the infected killed), round 2: 0x44 first, then the countdown
        g.tick(100 + COUNTDOWN_SECS * 1000 + 50);
        let host = *g.infect().infected.iter().next().expect("outbreak");
        let human = (0..3u8).find(|p| *p != host).unwrap();
        g.kill(host, Some(human), 10, false, 30_000);
        g.tick(30_100);
        assert!(matches!(g.phase(), RoundPhase::Over { win: 1, .. }));
        g.take_out();
        let mut t = 30_100;
        while !matches!(g.phase(), RoundPhase::Running { .. }) {
            t += 100;
            g.tick(t);
            assert!(t < 120_000, "no next round");
        }
        let o = g.take_out();
        for pid in 0..3u8 {
            let ids = shell_ids(&o, pid);
            let begin = ids
                .iter()
                .position(|i| *i == gs::GS_ROUND_BEGIN)
                .expect("0x44");
            let count = ids
                .iter()
                .rposition(|i| *i == gs::GS_INFECTION_MODE)
                .expect("0xd3");
            assert!(
                begin < count,
                "client {pid}: 0x44 before the countdown: {ids:x?}"
            );
            assert!(infection_msgs(&o, pid)
                .iter()
                .any(|m| m.0 == 1 && m.1 == 2 && m.3 == COUNTDOWN_SECS as i32));
        }
    }

    /// DSV-281 with the user's ROM: a spy wears the GMS spy body, the agent the agent body (its own styles and
    /// armour maximum), each with the matching spy face; a hunter again gets his own character back. Before, every
    /// spy and the agent kept the player's own body.
    #[test]
    fn rom_spy_hunt_spies_and_the_agent_wear_the_gms_bodies() {
        use super::spy::{SpyGms, HUNTERS, SPIES};
        let rom = dc_rez::dc_rom::rom_or_skip!();
        let (t, _) = RomTables::load(&rom);
        let vfs = dc_rez::RezVfs::new(&rom);
        let gr = dc_bute::GameRoomData::from_vfs(&vfs).expect("room table");
        let key = gr
            .modes(false)
            .iter()
            .find(|m| m.get("GameType").map(|v| v.trim()) == Some("9"))
            .and_then(|m| m.get("Key").cloned())
            .expect("a class-9 row");
        let doc = crate::coop_rom::find_gms(&vfs, &key, r"Worlds\WARHEAD.dat").expect("GMS");
        let gms = SpyGms::from_gms(&doc);
        let (spy_row, agent_row) = (
            gms.spy_model.expect("spy row"),
            gms.agent_model.expect("agent row"),
        );
        let mut g = Game::new(
            GameTuning {
                move_check: false,
                trace: false,
                ..GameTuning::default()
            },
            &world(),
            t.clone(),
        );
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "9".into());
        g.set_room(&room);
        for (pid, uid, obj) in [(0u8, 7u32, 1u16), (1, 8, 3)] {
            let e = entry(uid, 0, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        g.tick(100);
        g.spy.gms = gms;
        let own = g.players[&0].character.clone().expect("ROM character");
        g.take_out();
        g.spy_team(0, SPIES, 1);
        let o = g.take_out();
        assert!(o.contains(&GameOut::CharacterLook { pid: 0 }));
        let spy = g.players[&0].character.clone().unwrap();
        assert_eq!(spy.assets.body[0], t.chars.bodies[&spy_row].model);
        let face = spy.assets.face.clone().expect("spy face");
        assert!(face
            .0
            .eq_ignore_ascii_case(g.spy.gms.face_model.as_deref().unwrap()));
        // the agent: the agent row's styles and armour maximum
        g.spy.agent = Some(0);
        g.spy_team(0, SPIES, 5);
        let agent = g.players[&0].character.clone().unwrap();
        assert_eq!(agent.assets.body_styles, t.chars.bodies[&agent_row].styles);
        assert_eq!(agent.assets.max_armor, t.chars.bodies[&agent_row].max_armor);
        assert_ne!(agent.assets.face, spy.assets.face, "the agent face");
        // a hunter again: his own character
        g.spy.agent = None;
        g.take_out();
        g.spy_team(0, HUNTERS, 0);
        assert!(g.take_out().contains(&GameOut::CharacterLook { pid: 0 }));
        assert_eq!(g.players[&0].character.as_ref().unwrap().assets, own.assets);
    }

    /// DSV-214 / DSV-215 with the user's ROM: the outbreak's host wears the host look (the override row's render
    /// styles and face, every client told) and loses his loadout for the host claws (0xb4, 0xb3, the hand model);
    /// a claw-infected human gets the infected look byte and the infected claws; a respawned infected spawns with the
    /// claws only; the next round gives the own look back.
    #[test]
    fn rom_quarantine_infected_wear_the_infected_look_and_the_claws() {
        use super::infect::{CLAWS_HOST, CLAWS_VIRUS, COUNTDOWN_SECS, INFECTED_HEALTH_SCALE};
        let rom = dc_rez::dc_rom::rom_or_skip!();
        let (t, _) = RomTables::load(&rom);
        let host_claws = t.weapons.by_name_key(CLAWS_HOST).expect("host claws row");
        let virus_claws = t.weapons.by_name_key(CLAWS_VIRUS).expect("claws row");
        let mut g = Game::new(
            GameTuning {
                move_check: false,
                trace: false,
                ..GameTuning::default()
            },
            &world(),
            t.clone(),
        );
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "11".into());
        room.pairs.insert("goal".into(), "2".into());
        g.set_room(&room);
        for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3), (2, 9, 0, 5)] {
            let mut e = entry(uid, team, "P");
            e.extra = vec![("loadout".into(), "0:145".into())];
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        g.tick(100);
        let own: BTreeMap<u8, CharacterLook> = g
            .players
            .iter()
            .map(|(k, p)| (*k, p.character.clone().expect("ROM character")))
            .collect();
        g.take_out();
        g.tick(100 + COUNTDOWN_SECS * 1000 + 50);
        let o = g.take_out();
        let host = *g.infect().infected.iter().next().unwrap();
        assert!(o.contains(&GameOut::CharacterLook { pid: host }));
        let host_voice = g.tables.event_sounds.host[0].clone().expect("host voice");
        assert!(
            o.iter()
                .any(|x| matches!(x, GameOut::Sound(s) if s.file == host_voice)),
            "the host voice"
        );
        // DSV-320: the body row's own host row (its look byte, the 0xd3 sub 3 to the host, its hit points)
        let body = t
            .chars
            .bodies
            .values()
            .find(|b| dc_bute::eq_fold(&b.model, &own[&host].assets.body[0]))
            .expect("the body row");
        let (host_n, infected_n) = (
            body.host_override.expect("HostInfectedOveridable"),
            body.infected_override.expect("InfectedOveridable"),
        );
        let host_row = t.chars.overrides[&host_n].clone();
        assert_eq!(
            g.players[&host].look.bytes[gs::LOOK_OVERRIDE_BYTE],
            host_n as i8
        );
        assert_eq!(override_msgs(&o, host), vec![host_n as i8]);
        assert_eq!(
            g.players[&host].health,
            host_row.max_hit_points.expect("MaxHitPoints") as f32 * INFECTED_HEALTH_SCALE
        );
        let c = g.players[&host].character.clone().unwrap();
        let face = &t.chars.faces[&host_row.face.unwrap()];
        assert_eq!(c.assets.face.as_ref().map(|f| &f.0), Some(&face.model));
        assert_eq!(c.assets.body_styles, host_row.styles);
        assert_eq!(c.assets.body, own[&host].assets.body, "the same body");
        let to_host = ids(&o, host);
        let rm = to_host.iter().position(|x| *x == gs::GS_WEAPON_REMOVE);
        let eq = to_host.iter().rposition(|x| *x == gs::GS_WEAPON_EQUIP);
        assert!(rm.is_some() && eq.is_some() && rm < eq, "{to_host:x?}");
        let equip = o
            .iter()
            .rev()
            .find_map(|x| match x {
                GameOut::To(p, m) if *p == host && m.as_bytes()[1] == gs::GS_WEAPON_EQUIP => {
                    Some(m.clone())
                }
                _ => None,
            })
            .unwrap();
        let mut r = equip.reader();
        r.skip_bits(16);
        assert_eq!(r.read_u16(), host_claws);
        assert!(o.contains(&GameOut::HandWeapon {
            pid: host,
            weapon: host_claws
        }));
        assert_eq!(
            g.players[&host]
                .weapons
                .iter()
                .map(|w| w.weapon)
                .collect::<Vec<_>>(),
            vec![host_claws]
        );
        // a claw hit infects with the infected look byte and the infected claws
        let human = (0..3).find(|p| *p != host).unwrap();
        let src = super::combat::DamageSource {
            weapon: host_claws,
            dtype: 0,
        };
        g.apply_damage(human, Some(host), 50.0, [1.0, 0.0, 0.0], src, 2, 30_000);
        let o = g.take_out();
        assert_eq!(g.players[&human].team, 2);
        assert_eq!(
            g.players[&human].look.bytes[gs::LOOK_OVERRIDE_BYTE],
            infected_n as i8
        );
        assert_eq!(override_msgs(&o, human), vec![infected_n as i8]);
        assert!(o.contains(&GameOut::HandWeapon {
            pid: human,
            weapon: virus_claws
        }));
        // a dead host stays dead (DSV-213 amended); a host spawned again (here by hand) has the host claws only
        g.kill(host, Some(human), 10, false, 30_100);
        g.tick(30_200);
        g.tick(40_000);
        assert_ne!(g.players[&host].life, Life::Alive);
        g.spawn(host, 40_000);
        assert_eq!(g.players[&host].life, Life::Alive);
        assert_eq!(
            g.players[&host]
                .weapons
                .iter()
                .map(|w| w.weapon)
                .collect::<Vec<_>>(),
            vec![host_claws]
        );
        // the next round: the own look again
        g.take_out();
        g.round_no += 1;
        g.infect_round_start(40_000, true);
        let o = g.take_out();
        assert!(o.contains(&GameOut::CharacterLook { pid: host }));
        assert_eq!(override_msgs(&o, host), vec![-1]);
        assert_eq!(g.players[&host].character.as_ref(), Some(&own[&host]));
        assert_eq!(g.players[&human].character.as_ref(), Some(&own[&human]));
    }

    /// One player of team 1 alive in a room of `mode_class`, killed at t = 100.
    fn dead_player(mode_class: u8) -> Game {
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), mode_class.to_string());
        g.set_room(&room);
        g.add_player(0, None, None, 1, [0.0; 3], [0.0; 3], 0);
        g.on_up(0, Up::PlayerInfo(Box::default()), 10);
        g.on_up(0, Up::WorldReady, 20);
        assert_eq!(g.players[&0].life, Life::Alive);
        g.take_out();
        g.kill(0, None, 3, false, 100);
        let o = g.take_out();
        let states: Vec<u8> = o
            .iter()
            .filter_map(|x| match x {
                GameOut::To(0, m) if m.as_bytes()[1] == gs::GS_PLAYER_STATE => {
                    Some(m.as_bytes()[2])
                }
                _ => None,
            })
            .collect();
        assert_eq!(states, vec![3, 2], "DSV-32: state 3 then 2 in one block");
        g
    }

    fn alive(g: &Game) -> bool {
        g.players[&0].life == Life::Alive
    }

    /// DSV-32: the server answers the client's own request in the classes whose client asks (no timer there), keeps
    /// a timer only where the client may not ask, and never respawns inside a round of the round modes.
    #[test]
    fn respawn_rules_by_mode_class() {
        use crate::up::{UP_RESPAWN_REQUEST, UP_RESPAWN_REQUEST_C};
        // TDM: no server timer; a request before the wait minus the guard is ignored, the next one answered
        let mut g = dead_player(1);
        g.tick(100 + 60_000);
        assert!(!alive(&g), "no server respawn in a class whose client asks");
        let mut g = dead_player(1);
        g.on_respawn_request(0, UP_RESPAWN_REQUEST, 100 + 6_749);
        assert!(!alive(&g), "6749 ms + 250 < 7000");
        g.on_respawn_request(0, UP_RESPAWN_REQUEST, 100 + 6_750);
        assert!(alive(&g));
        assert_eq!(g.players[&0].spawns, 2);
        // a living player's request is ignored
        g.on_respawn_request(0, UP_RESPAWN_REQUEST, 100 + 9_000);
        assert_eq!(g.players[&0].spawns, 2);
        // class 10: 1500 ms wait, request 0xd2
        let mut g = dead_player(10);
        g.on_respawn_request(0, UP_RESPAWN_REQUEST_C, 100 + 1_250);
        assert!(alive(&g));
        // class 5: the client arms only under a client state: request or the fallback 10 s after the death
        let mut g = dead_player(5);
        g.tick(100 + RESPAWN_FALLBACK_MS - 1);
        assert!(!alive(&g));
        g.tick(100 + RESPAWN_FALLBACK_MS);
        assert!(alive(&g), "fallback respawn");
        // the generic class of a room line without a type: the client never asks, the server respawns
        let mut g = dead_player(0);
        g.tick(100 + CLIENT_RESPAWN_WAIT_MS - 1);
        assert!(!alive(&g));
        g.tick(100 + CLIENT_RESPAWN_WAIT_MS);
        assert!(alive(&g));
        // round modes: no respawn inside the round, a request is ignored
        let mut g = dead_player(12);
        g.on_respawn_request(0, UP_RESPAWN_REQUEST, 100 + 8_000);
        g.tick(100 + 60_000);
        assert!(!alive(&g));
        // after the round end a request is not answered
        let mut g = dead_player(1);
        g.end_round(5_000, 0);
        g.on_respawn_request(0, UP_RESPAWN_REQUEST, 100 + 7_100);
        assert!(!alive(&g));
        assert!(g
            .take_out()
            .iter()
            .any(|x| matches!(x, GameOut::Log(l) if l.contains("ignored (the round is over)"))));
    }

    /// A fire report of `shooter` that hits `victim`'s object at its box centre + `up`, naming model part `part`.
    fn hit_report(
        g: &Game,
        shooter: u8,
        victim: u8,
        up: f32,
        part: Option<u8>,
    ) -> crate::up::UpFire {
        let s = &g.players[&shooter];
        let v = &g.players[&victim];
        let origin = [s.pos[0], s.pos[1] + 33.0, s.pos[2]];
        let target = [v.pos[0], v.pos[1] + up, v.pos[2]];
        let d = sub3(target, origin);
        let len = len3(d);
        crate::up::UpFire {
            weapon_id: 3,
            ammo_id: 0,
            origin,
            dir: [d[0] / len, d[1] / len, d[2] / len],
            extra: part.map(|k| crate::up::FireExtra {
                obj: Some(v.obj),
                kind: k,
                a: target,
                b: [-1.0, 0.0, 0.0],
            }),
            last: true,
            hit: Some((len, Some(v.obj))),
            complete: true,
            ..crate::up::UpFire::default()
        }
    }

    /// DSV-430: by default a player wears the armour rows of its body (row 0 when the body names none), head gear
    /// and goggle; their shares add up per damage type and hit location and their `Armor` gives the start armour.
    #[test]
    fn worn_gear_rows_protect_by_body_head_and_goggle() {
        use crate::chars::HitNode;
        use crate::rom_tables::GearProtection;
        let mut t = character_tables();
        t.chars.bodies.get_mut(&0).unwrap().skeleton = Some(1);
        t.chars.skeletons.insert(
            1,
            vec![
                HitNode {
                    factor: 1.0,
                    location: 2,
                },
                HitNode {
                    factor: 2.0,
                    location: 1,
                },
            ],
        );
        let dt = super::TODO_DAMAGE_ZONE;
        let row = |armor: f32, head: f32, torso: f32| GearProtection {
            armor: Some(armor),
            slots: vec![(dt, [head, torso, torso, torso])],
        };
        t.weapons.gear.insert(0, row(100.0, 0.0, 0.2));
        t.weapons.gear.insert(3, row(0.0, 0.25, 0.0));
        t.weapons.gear.insert(5, row(0.0, 0.1, 0.0));
        t.chars.hair_gear.insert(2, 3);
        t.chars.goggle_gear.insert(7, 5);
        let mut g = Game::new(
            GameTuning {
                move_check: false,
                trace: false,
                armor_cost_from_base: false,
                spawn_shield: false,
                ..GameTuning::default()
            },
            &world(),
            t,
        );
        assert_eq!(g.cfg.armor_gear, ArmorGear::Worn, "the default");
        g.insert_rig("BODY\\b.ltb", rig());
        let a = entry(7, 0, "Dave");
        let mut b = entry(8, 1, "Erin");
        // head item 4002 (row Hair2 -> gear 3), goggle 5007 (Goggle7 -> gear 5)
        b.extra = vec![("loadout".into(), "0:3,4:4002,6:5007".into())];
        for (pid, e, obj) in [(0u8, &a, 1u16), (1, &b, 3)] {
            g.note_identified(e);
            g.add_player(pid, Some(e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        assert_eq!(
            g.armour_of(0).absorb,
            Absorb::Worn([Some(0), None, None]),
            "no gear: the calibrated row 0 only"
        );
        let w = g.armour_of(1);
        assert_eq!(w.absorb, Absorb::Worn([Some(0), Some(3), Some(5)]));
        assert_eq!(w.start, 100.0, "the rows' Armor summed (100 + 0 + 0)");
        // a repeated row protects once
        assert_eq!(
            dedup_worn([Some(6), Some(6), Some(5)]),
            [Some(6), None, Some(5)]
        );
        assert_eq!(
            dedup_worn([Some(0), Some(3), Some(3)]),
            [Some(0), Some(3), None]
        );
        assert_eq!(dedup_worn([Some(0), None, None]), [Some(0), None, None]);
        // head node (factor 2): 50 damage, the head gear and goggle take 0.25 + 0.1 of it
        let f = hit_report(&g, 0, 1, 0.0, Some(1));
        g.on_up(0, Up::Fire(Box::new(f)), 100);
        let v = &g.players[&1];
        assert!(
            (v.health - 67.5).abs() < 1e-4 && (v.armor - 82.5).abs() < 1e-4,
            "{} {}",
            v.health,
            v.armor
        );
        // the same head hit on the bare player: no head share
        let f = hit_report(&g, 1, 0, 0.0, Some(1));
        g.on_up(1, Up::Fire(Box::new(f)), 200);
        let v = &g.players[&0];
        assert!(
            (v.health - 50.0).abs() < 1e-4 && v.armor == 100.0,
            "{} {}",
            v.health,
            v.armor
        );
    }

    /// DSV-30: health damage = InstDamage (the default 25 here) x the ROM skeleton node factor of the reported model
    /// part (else 2.0 head / 1.0 body by the box); armour from the loadout's armour items absorbs their grade / 100
    /// while it lasts; the kill feed carries the node's location code.
    #[test]
    fn damage_rule_rom_nodes_and_armour_items() {
        use crate::chars::HitNode;
        let mut t = character_tables();
        t.chars.bodies.get_mut(&0).unwrap().skeleton = Some(1);
        t.chars.skeletons.insert(
            1,
            vec![
                HitNode {
                    factor: 1.0,
                    location: 0,
                },
                HitNode {
                    factor: 3.5,
                    location: 1,
                },
                HitNode {
                    factor: 0.5,
                    location: 4,
                },
            ],
        );
        t.items.gear.insert(
            (0x0f, 5),
            crate::rom_tables::GearRow {
                armor: 20.0,
                grade: 40.0,
            },
        );
        let mut g = Game::new(
            GameTuning {
                move_check: false,
                trace: false,
                armor_gear: ArmorGear::Items,
                armor_cost_from_base: false,
                // these shots fall inside the spawn window: the damage rule is tested, not the shield (DSV-156)
                spawn_shield: false,
                ..GameTuning::default()
            },
            &world(),
            t,
        );
        g.insert_rig("BODY\\b.ltb", rig());
        let a = entry(7, 0, "Dave");
        let mut b = entry(8, 1, "Erin");
        // the lobby's form: equipment slot 0 holds weapon item 3, slot 8 the vest item 3005
        b.extra = vec![("loadout".into(), "0:3,8:3005".into())];
        for (pid, e, obj) in [(0u8, &a, 1u16), (1, &b, 3)] {
            g.note_identified(e);
            g.add_player(pid, Some(e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        let o = g.take_out();
        let armour_sent: Vec<f32> = o
            .iter()
            .filter_map(|x| match x {
                GameOut::To(1, m)
                    if m.as_bytes()[1] == gs::GS_PLAYER_STATUS && m.as_bytes()[2] == 3 =>
                {
                    let mut r = m.reader();
                    r.skip_bits(16 + 24);
                    Some(r.read_f32())
                }
                _ => None,
            })
            .collect();
        assert_eq!(armour_sent.last(), Some(&20.0), "armour from the vest item");
        assert_eq!(g.players[&1].armor_absorb, Absorb::Flat(0.4));
        assert_eq!(
            g.armour_of(0).start.to_bits(),
            0,
            "no armour items: +0.0, not -0.0"
        );
        let own_armour: Vec<u32> = o
            .iter()
            .filter_map(|x| match x {
                GameOut::To(0, m)
                    if m.as_bytes()[1] == gs::GS_PLAYER_STATUS && m.as_bytes()[2] == 3 =>
                {
                    let mut r = m.reader();
                    r.skip_bits(16 + 24);
                    Some(r.read_f32().to_bits())
                }
                _ => None,
            })
            .collect();
        assert!(
            !own_armour.is_empty() && own_armour.iter().all(|&b| b == 0),
            "{own_armour:x?}"
        );
        // leg node (factor 0.5): 12.5 damage, 40 % of it to the armour
        let f = hit_report(&g, 0, 1, 0.0, Some(2));
        g.on_up(0, Up::Fire(Box::new(f)), 100);
        let v = &g.players[&1];
        assert!(
            (v.health - 92.5).abs() < 1e-4 && (v.armor - 15.0).abs() < 1e-4,
            "{} {}",
            v.health,
            v.armor
        );
        // head node (factor 3.5): 87.5, the armour takes its last 15
        let f = hit_report(&g, 0, 1, 0.0, Some(1));
        g.on_up(0, Up::Fire(Box::new(f)), 200);
        let v = &g.players[&1];
        assert!(
            (v.health - 20.0).abs() < 1e-4 && v.armor == 0.0,
            "{} {}",
            v.health,
            v.armor
        );
        // no part named: the box geometry, body factor 1.0 -> 25 kills; the feed says torso (2)
        g.take_out();
        let f = hit_report(&g, 0, 1, 0.0, None);
        g.on_up(0, Up::Fire(Box::new(f)), 300);
        assert_eq!(g.players[&1].life, Life::Dying { died: 300 });
        let o = g.take_out();
        let feed = o
            .iter()
            .find_map(|x| match x {
                GameOut::To(0, m) if m.as_bytes()[1] == gs::GS_KILL => Some(m.as_bytes()[7]),
                _ => None,
            })
            .expect("0x6d");
        assert_eq!(feed, 2);
        // operator keys replace the ROM values
        let mut cfg = GameTuning::default();
        cfg.set_combat("combat.zone_factors", "off").unwrap();
        cfg.set_combat("armor_absorb", "0.75").unwrap();
        assert!(!cfg.zone_factors_from_rom);
        assert_eq!(cfg.armor_absorb, Some(0.75));
    }

    /// DSV-169: a hit claim at standing head height (inside the 50 half height, above the 40 of the crouch box) is
    /// accepted on a standing victim and refused on one whose last state says crouched; the box without a crouch
    /// clip is the standing one capped at 40, with a crouch clip its extent.
    #[test]
    fn a_crouched_victim_has_the_crouch_box() {
        let mut g = Game::new(
            GameTuning {
                move_check: false,
                trace: false,
                spawn_shield: false,
                hit_tolerance: 0.0,
                player_dims: [15.0, 50.0, 15.0],
                ..GameTuning::default()
            },
            &world(),
            tables(),
        );
        for (pid, obj) in [(0u8, 1u16), (1, 3)] {
            g.add_player(pid, None, None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        assert_eq!(g.hit_dims_of(1)[1], 50.0);
        let f = hit_report(&g, 0, 1, 45.0, None);
        g.on_up(0, Up::Fire(Box::new(f.clone())), 100);
        assert!(g.players[&1].health < 100.0, "standing victim: accepted");
        let before = g.players[&1].health;
        g.players.get_mut(&1).unwrap().motion = Motion::Crouch;
        assert_eq!(g.hit_dims_of(1)[1], 40.0);
        g.on_up(0, Up::Fire(Box::new(f)), 200);
        assert_eq!(g.players[&1].health, before, "crouched victim: refused");
        let f = hit_report(&g, 0, 1, 35.0, None);
        g.on_up(0, Up::Fire(Box::new(f)), 300);
        assert!(
            g.players[&1].health < before,
            "crouched victim: body point accepted"
        );
    }

    /// What the victim received for one hit: 0x74 `damage` and `|from_dir|`, the health and armour statuses.
    #[derive(Debug, Default)]
    struct Received {
        damage: Option<u32>,
        dir_len: Option<f32>,
        health: Option<f32>,
        armour: Option<f32>,
        feed_zone: Option<u8>,
    }

    fn received(o: &[GameOut], victim: u8) -> Received {
        let mut r = Received::default();
        for x in o {
            let GameOut::To(to, m) = x else {
                continue;
            };
            let b = m.as_bytes();
            if b[1] == gs::GS_KILL {
                r.feed_zone = Some(b[7]);
            }
            if *to != victim {
                continue;
            }
            let mut rd = m.reader();
            rd.skip_bits(16);
            if b[1] == gs::GS_DAMAGE_TAKEN {
                let d = [rd.read_f32(), rd.read_f32(), rd.read_f32()];
                rd.skip_bits(8 + 8 + 16 + 8 + 8 + 8);
                r.damage = Some(rd.read_u32());
                r.dir_len = Some(len3(d));
            } else if b[1] == gs::GS_PLAYER_STATUS {
                let kind = rd.read_u8();
                rd.skip_bits(16);
                let v = rd.read_f32();
                match kind {
                    gs::STATUS_HEALTH => r.health = Some(v),
                    gs::STATUS_ARMOR => r.armour = Some(v),
                    _ => {}
                }
            }
        }
        r
    }

    /// ROM-gated calibration check (project decision, keep the 2009 damage): with the default tuning and the
    /// user's ROM tables our server gives every captured 2009 hit class the per-hit health, armour and 0x74 values
    /// the 2009 server sent to the real client and to the bot (capture measurements: CAPTURE_tdm6_VALIDATION.md §3
    /// T1 / T2 and the damage rows, CAPTURE_tdm4_KILL_VALIDATION.md §3.1; weapon / ammunition 145, model nodes 24,
    /// 11, 6, 53 as the client named them, the blast of weapon 7 / ammunition 9). Within 1 point; nothing from the ROM
    /// is asserted by value except the location code of the head node, which the capture shows (`hit_zone` 1).
    #[test]
    fn rom_damage_reproduces_the_captured_2009_hits() {
        let rom = dc_rez::dc_rom::rom_or_skip!();
        let (t, notes) = RomTables::load(&rom);
        assert!(notes.is_empty(), "{notes:?}");
        // one fresh duel per class: player 0 shoots player 1 (both with the rifle of the captures)
        let duel = || {
            let mut g = Game::new(
                GameTuning {
                    move_check: false,
                    trace: false,
                    // these shots fall inside the spawn window: the damage rule is tested, not the shield (DSV-156)
                    spawn_shield: false,
                    ..GameTuning::default()
                },
                &world(),
                t.clone(),
            );
            for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3)] {
                let mut e = entry(uid, team, "P");
                e.extra = vec![("loadout".into(), "0:145".into())];
                g.note_identified(&e);
                g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
                g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
                g.on_up(pid, Up::WorldReady, 20);
            }
            let start = received(&g.take_out(), 1);
            assert_eq!(
                (start.health, start.armour),
                (Some(100.0), Some(100.0)),
                "health and armour 100 at the spawn (tdm4 / tdm6: 0x66 kinds 2 and 3)"
            );
            g
        };
        let mut now = 100;
        let mut fire = |g: &mut Game, node: u8, weapon: (u16, u16)| {
            let mut f = hit_report(g, 0, 1, 20.0, Some(node));
            (f.weapon_id, f.ammo_id) = weapon;
            g.on_up(0, Up::Fire(Box::new(f)), now);
            now += 200;
            received(&g.take_out(), 1)
        };
        let close = |a: Option<f32>, b: f32| a.is_some_and(|a| (a - b).abs() <= 1.0);
        // the victim's skeleton must be the one whose node 53 is the head (the capture's `hit_zone` 1)
        let g = duel();
        let nodes = &g.players[&1]
            .character
            .as_ref()
            .expect("ROM character")
            .assets
            .hit_nodes;
        assert_eq!(nodes.get(53).map(|n| n.location), Some(1));
        // class A: rifle on the arm nodes 24 / 11, four hits from 100 / 100 (tdm6 real client x 24, tdm4 bot x 4):
        // health -26.775, armour -5.25 each, 0x74 27 each (the lethal one too), |from_dir| 0.2678
        let mut g = duel();
        let want = [(73.225, 94.75), (46.45, 89.5), (19.675, 84.25), (0.0, 79.0)];
        for (i, (node, (h, a))) in [24u8, 24, 11, 11].into_iter().zip(want).enumerate() {
            let r = fire(&mut g, node, (145, 145));
            assert_eq!(r.damage, Some(27), "class A hit {i}: {r:?}");
            assert!(
                close(r.health, h) && close(r.armour, a),
                "class A hit {i}: {r:?}"
            );
            assert!(
                (r.dir_len.unwrap() - 0.26775).abs() < 1e-3,
                "class A hit {i}: {r:?}"
            );
            assert!(
                (g.players[&1].health - h).abs() < 1e-3,
                "class A hit {i} exact health"
            );
            assert!(
                (g.players[&1].armor - a).abs() < 1e-3,
                "class A hit {i} exact armour"
            );
        }
        assert_eq!(g.players[&1].life, Life::Dying { died: 700 });
        // class B: torso node 6 from 100 / 100 (tdm6 the user on the bot): health 70.25, armour 94.75, 0x74 30
        let mut g = duel();
        let r = fire(&mut g, 6, (145, 145));
        assert_eq!(r.damage, Some(30), "class B: {r:?}");
        assert!(
            close(r.health, 70.25) && close(r.armour, 94.75),
            "class B: {r:?}"
        );
        assert!((r.dir_len.unwrap() - 0.2975).abs() < 1e-3, "class B: {r:?}");
        // class C: the head node 53 at 70.25 / 94.75 (tdm6, the next hit on the same bot): 0x74 210 (not clamped),
        // health 0, the armour untouched (no armour status), |from_dir| 1.0, the kill feed's zone 1
        let r = fire(&mut g, 53, (145, 145));
        assert_eq!(r.damage, Some(210), "class C: {r:?}");
        assert_eq!(r.health, Some(0.0), "class C: {r:?}");
        assert_eq!(r.armour, None, "class C: no armour change: {r:?}");
        assert!((g.players[&1].armor - 94.75).abs() < 1e-3);
        assert!((r.dir_len.unwrap() - 1.0).abs() < 1e-4, "class C: {r:?}");
        assert_eq!(r.feed_zone, Some(1));
        // class D: a blast (the thrown weapon 7 / ammunition 9 of tdm6, 42.32 on the bot at full armour): the blast's
        // damage type is not protected, health 57.68, armour unchanged
        let mut g = duel();
        let area = g.tables.weapons.area(7, 9).expect("the ROM blast row");
        let src = combat::DamageSource {
            weapon: 7,
            dtype: area.damage_type.unwrap_or(TODO_DAMAGE_ZONE),
        };
        g.apply_damage(
            1,
            Some(0),
            42.3233,
            [0.0, 1.0, 0.0],
            src,
            TODO_KILL_ZONE_BODY,
            900,
        );
        let r = received(&g.take_out(), 1);
        assert_eq!(r.damage, Some(42), "class D: {r:?}");
        assert!(
            close(r.health, 57.68) && r.armour.is_none(),
            "class D: {r:?}"
        );
        assert!((g.players[&1].armor - 100.0).abs() < 1e-3);
        println!("ROM calibration: hit classes A (arm), B (torso), C (head), D (blast) reproduce the captured values");
    }

    /// ROM-gated: the node numbers the 2009-style client names in `UP_FIRE` (arena runs 20261026_032858_duel and
    /// _033350_teamkill_doublekill: the reference stack claimed nodes 9 and 53 on a level shot at a standing bot, the dc-client
    /// claims 24) map through the VICTIM'S skeleton row. Nothing from the ROM is asserted by value: only the
    /// relations the captures and the arena show (53 is the head location in the captured 2009 hit with `hit_zone` 1;
    /// 9 is a head node of the same row; the shoulder nodes 24 / 11 are arms; a head hit multiplies more than an arm
    /// hit), that the SAME node number is another place on another skeleton row (DSV-30 amendment), and that a node
    /// the row does not have falls back to the box (head band 2.0, not the node factor).
    // EVIDENCE: cshell+0x1e4760 (the client reads `Node<k>DamageFactor` / `Location` / `Radius` of every skeleton
    // row; the same keys the server reads), cshell+0x1e14f0 (a location above 5 reads as 0), cshell+0x235c00 (the
    // reported node = the nearest node whose Radius sphere the ray crosses, 0xff for none)
    // DC-DEFAULT: DSV-30 (the zone rule), DSV-130 (the node claim stays an unvalidated hint)
    #[test]
    fn rom_node_claims_map_through_the_victims_skeleton() {
        let rom = dc_rez::dc_rom::rom_or_skip!();
        let (t, notes) = RomTables::load(&rom);
        assert!(notes.is_empty(), "{notes:?}");
        // the table level: the male rows' skeleton and the female rows' skeleton name the same index differently
        let skel_of = |row: u32| {
            let s = t.chars.bodies[&row].skeleton.expect("body row skeleton");
            t.chars.skeletons[&s].clone()
        };
        let male = skel_of(0);
        let female = t
            .chars
            .bodies
            .values()
            .filter_map(|b| b.skeleton)
            .map(|s| t.chars.skeletons[&s].clone())
            .find(|s| s.get(53).is_some_and(|n| n.location != 1))
            .expect("a skeleton row whose node 53 is not a head");
        assert_eq!(
            male[53].location, 1,
            "53 is a head node (captured hit_zone 1)"
        );
        assert_eq!(male[9].location, 1, "9 is a head node of the same row");
        assert_eq!(male[53].factor, male[9].factor);
        assert_eq!(
            (male[24].location, male[11].location),
            (3, 3),
            "shoulders are arms"
        );
        assert_eq!(
            male[6].location, 2,
            "node 6 is the chest (captured hit_zone 2)"
        );
        assert!(male[9].factor > male[6].factor && male[6].factor > male[24].factor);
        assert_ne!(
            female[53].location, 1,
            "the same index is not a head on the other row"
        );
        // the game level: one fresh duel per claim, player 0 shoots player 1 with the captures' rifle
        let hit = |node: u8, up: f32| {
            let mut g = Game::new(
                GameTuning {
                    move_check: false,
                    trace: false,
                    // these shots fall inside the spawn window: the damage rule is tested, not the shield (DSV-156)
                    spawn_shield: false,
                    ..GameTuning::default()
                },
                &world(),
                t.clone(),
            );
            for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3)] {
                let mut e = entry(uid, team, "P");
                e.extra = vec![("loadout".into(), "0:145".into())];
                g.note_identified(&e);
                g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
                g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
                g.on_up(pid, Up::WorldReady, 20);
            }
            g.take_out();
            let mut f = hit_report(&g, 0, 1, up, Some(node));
            (f.weapon_id, f.ammo_id) = (145, 145);
            g.on_up(0, Up::Fire(Box::new(f)), 100);
            let base = g
                .tables
                .weapons
                .damage(145, 145)
                .expect("ROM ammunition row");
            (received(&g.take_out(), 1), base)
        };
        // a level shot at a standing player lands in the head band of the box (33 above the centre, the height of
        // both the arena's claims and the real client's head hit): node 9 and node 53 are both the one-shot head hit
        let (a, base) = hit(9, 33.0);
        let (b, _) = hit(53, 33.0);
        assert_eq!(a.damage, b.damage);
        assert_eq!(a.damage, Some((base * male[9].factor).round() as u32));
        assert_eq!((a.feed_zone, b.feed_zone), (Some(1), Some(1)));
        assert!(
            base * male[9].factor >= 100.0,
            "the head hit kills from full health"
        );
        // the dc-client's constant (24): an arm hit, zone 3, the arm multiplier, no kill
        let (c, _) = hit(24, 33.0);
        assert!(c.feed_zone.is_none(), "an arm hit does not kill: {c:?}");
        assert!(c.health.is_some_and(|h| h > 0.0), "{c:?}");
        assert!(c.damage.is_some_and(|d| d < 100), "{c:?}");
        // no usable node (the client's `none` 0xff, or an index the row lacks): the box decides, head band factor
        // 2.0 (DSV-11 / DSV-30 fallback), which is weaker than the node's own head factor
        let (d, _) = hit(255, 33.0);
        let box_head = GameTuning::default().head_factor;
        assert!(box_head < male[9].factor);
        assert_eq!(d.damage, Some((base * box_head).round() as u32));
    }

    /// Two players spawned at their team points (-300, 0, 0) and (300, 0, 0).
    fn duel(cfg: GameTuning) -> Game {
        let mut g = Game::new(cfg, &world(), tables());
        for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3)] {
            let e = entry(uid, team, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        g.take_out();
        g
    }

    /// The shot effects (type 0x06) of the last output: (first object, end point x).
    fn effects(o: &[GameOut]) -> Vec<(u16, f32)> {
        o.iter()
            .filter_map(|x| match x {
                GameOut::Unreliable(0, m) if m.as_bytes()[1] == gs::FX_SHOT => {
                    let b = m.as_bytes();
                    Some((
                        u16::from_le_bytes([b[2], b[3]]),
                        f32::from_le_bytes([b[26], b[27], b[28], b[29]]),
                    ))
                }
                _ => None,
            })
            .collect()
    }

    /// The surface byte `a` of every shot effect sent to client 0 (`gs::shot_effect`: after the type, `first`,
    /// shooter, weapon and ammo).
    fn shot_surfaces(o: &[GameOut]) -> Vec<u8> {
        o.iter()
            .filter_map(|x| match x {
                GameOut::Unreliable(0, m) if m.as_bytes()[1] == gs::FX_SHOT => {
                    Some(m.as_bytes()[10])
                }
                _ => None,
            })
            .collect()
    }

    /// DSV-34: a reported hit whose shot crosses a solid of the trace world (more than the slack before the hit
    /// point) does no damage and its effect ends at the wall as a world hit; a miss ends at the first wall.
    #[test]
    fn shots_through_a_wall_are_refused_and_end_at_it() {
        use dc_sim::{CollModel, CollisionWorld, ConvexPoly, SURF_SOLID, V3};
        let wall = ConvexPoly::from_corners(
            vec![
                V3::new(0.0, -100.0, 500.0),
                V3::new(0.0, 300.0, 500.0),
                V3::new(0.0, 300.0, -500.0),
                V3::new(0.0, -100.0, -500.0),
            ],
            SURF_SOLID,
        );
        let walled = || {
            CollisionWorld::from_parts(
                vec![CollModel::from_polys(std::slice::from_ref(&wall))],
                &[],
            )
        };
        let mut g = duel(GameTuning {
            move_check: false,
            ..GameTuning::default()
        });
        assert_eq!(g.players[&0].pos[0], -300.0);
        g.set_trace_world(Some(walled()));
        let f = hit_report(&g, 0, 1, 0.0, None);
        g.on_up(0, Up::Fire(Box::new(f.clone())), 100);
        let o = g.take_out();
        assert_eq!(g.players[&1].health, 100.0, "no damage through the wall");
        assert!(o
            .iter()
            .any(|x| matches!(x, GameOut::Log(l) if l.contains("refused (a wall"))));
        let e = effects(&o);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].0, 0, "a world hit");
        assert!(e[0].1.abs() < 1.0, "ends at the wall: x {}", e[0].1);
        // a plain wall is no sky: the record carries the world surface, not the sky id (MAP-24)
        assert_eq!(shot_surfaces(&o), vec![gs::TODO_SURFACE_WORLD]);
        // a miss (no hit block) also ends at the wall
        let mut miss = f.clone();
        miss.hit = None;
        miss.extra = None;
        g.on_up(0, Up::Fire(Box::new(miss.clone())), 200);
        let e = effects(&g.take_out());
        assert_eq!(e[0].0, 0);
        assert!(e[0].1.abs() < 1.0);
        // the same shot with a clear line damages and ends at the victim; a miss flies 4096 units
        g.set_trace_world(None);
        g.on_up(0, Up::Fire(Box::new(f.clone())), 300);
        assert_eq!(g.players[&1].health, 75.0);
        let e = effects(&g.take_out());
        assert_eq!(e[0].0, 3, "the victim's object");
        g.on_up(0, Up::Fire(Box::new(miss)), 400);
        let e = effects(&g.take_out());
        assert_eq!(e[0].0, 0xFFFF, "nothing hit");
        assert!(e[0].1 > 3000.0);
        // combat.trace = off accepts the hit through the wall
        let mut g = duel(GameTuning {
            move_check: false,
            trace: false,
            ..GameTuning::default()
        });
        g.set_trace_world(Some(walled()));
        let f = hit_report(&g, 0, 1, 0.0, None);
        g.on_up(0, Up::Fire(Box::new(f)), 100);
        assert_eq!(g.players[&1].health, 75.0);
    }

    /// MAP-24: a shot whose first solid is a sky face (surface flag 0x10) is refused like any wall but its effect is
    /// the no-hit record ending at the sky point with the sky surface id (`dc_world::SURFACE_SKY`, the client's
    /// guard: no decal, impact or sound); a miss into the sky the same; a hit whose continuation would land on a
    /// sky face has no continuation record. (Review C4: before, the record carried the fallback world surface,
    /// which the client draws.)
    #[test]
    fn shots_into_a_sky_face_answer_the_no_hit_record() {
        use dc_sim::{CollModel, CollisionWorld, ConvexPoly, SURF_SOLID, V3};
        let sky = ConvexPoly::from_corners(
            vec![
                V3::new(0.0, -100.0, 500.0),
                V3::new(0.0, 300.0, 500.0),
                V3::new(0.0, 300.0, -500.0),
                V3::new(0.0, -100.0, -500.0),
            ],
            SURF_SOLID | crate::world::SURF_SKY,
        );
        let skied = || {
            CollisionWorld::from_parts(vec![CollModel::from_polys(std::slice::from_ref(&sky))], &[])
        };
        let mut g = duel(GameTuning {
            move_check: false,
            ..GameTuning::default()
        });
        g.set_trace_world(Some(skied()));
        assert_eq!(
            g.wall_hit_along([-300.0, 33.0, 0.0], [1.0, 0.0, 0.0], 1000.0)
                .map(|(d, sky)| (d.round(), sky)),
            Some((300.0, true))
        );
        let f = hit_report(&g, 0, 1, 0.0, None);
        g.on_up(0, Up::Fire(Box::new(f.clone())), 100);
        let o = g.take_out();
        assert_eq!(g.players[&1].health, 100.0, "no damage through the sky");
        let e = effects(&o);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].0, 0xFFFF, "the no-hit record, no decal");
        assert!(e[0].1.abs() < 1.0, "ends at the sky face: x {}", e[0].1);
        assert_eq!(
            shot_surfaces(&o),
            vec![gs::SURFACE_SKY],
            "the sky surface id, the client's guard"
        );
        assert_eq!(u16::from(gs::SURFACE_SKY), dc_world::SURFACE_SKY);
        assert_eq!(gs::SURFACE_SKY, 0x6e);
        let mut miss = f.clone();
        miss.hit = None;
        miss.extra = None;
        g.on_up(0, Up::Fire(Box::new(miss)), 200);
        let o = g.take_out();
        let e = effects(&o);
        assert_eq!(e[0].0, 0xFFFF);
        assert!(e[0].1.abs() < 1.0);
        assert_eq!(shot_surfaces(&o), vec![gs::SURFACE_SKY]);
        // the victim in front of the sky: the hit lands, no continuation record on the sky behind him
        let far_sky = ConvexPoly::from_corners(
            vec![
                V3::new(600.0, -100.0, 500.0),
                V3::new(600.0, 300.0, 500.0),
                V3::new(600.0, 300.0, -500.0),
                V3::new(600.0, -100.0, -500.0),
            ],
            SURF_SOLID | crate::world::SURF_SKY,
        );
        g.set_trace_world(Some(CollisionWorld::from_parts(
            vec![CollModel::from_polys(std::slice::from_ref(&far_sky))],
            &[],
        )));
        g.on_up(0, Up::Fire(Box::new(f)), 300);
        let e = effects(&g.take_out());
        assert_eq!(g.players[&1].health, 75.0);
        assert_eq!(e.len(), 1, "no continuation onto the sky: {e:?}");
        assert_eq!(e[0].0, 3, "the victim's object");
    }

    /// DSV-90: the look a client sends is stored without an overridable-model row; an all-zero look (a bot without
    /// a character look) becomes the default look.
    // DC-DEFAULT: DSV-90 (proposed); OBSERVED: ae-triage-reddot2-blast §1.3 (rust6 / tdm6 looks)
    #[test]
    fn player_info_look_is_sanitised() {
        let g = duel(GameTuning::default());
        for pid in [0u8, 1] {
            assert_eq!(
                g.players[&pid].look.bytes,
                gs::DEFAULT_LOOK_BYTES,
                "pid {pid}"
            );
        }
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        let e = entry(7, 0, "P");
        g.note_identified(&e);
        g.add_player(0, Some(&e), None, 1, [0.0; 3], [0.0; 3], 0);
        let mut info = crate::up::UpPlayerInfo::default();
        info.look.bytes = [0, 0, 0, 0, 18, 1, 0, 0, 0, 1, 1, -1, 1, 0, 0];
        info.look.text_a = b"x".to_vec();
        g.on_up(0, Up::PlayerInfo(Box::new(info)), 10);
        let l = &g.players[&0].look;
        assert_eq!(l.bytes, [0, 0, 0, 0, 18, 1, 0, 0, 0, 1, 1, -1, 1, 0, -1]);
        assert_eq!(l.text_a, b"x");
    }

    /// DSV-341: with a `loadout=` token the look the other clients get carries the head gear, uniform, backpack
    /// and goggle ids of the equipment (bytes 7, 8, 10, 11); without one the client's look stays.
    #[test]
    fn the_look_carries_the_equipment_gear() {
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        let mut e = entry(7, 0, "P");
        e.extra = vec![("loadout".into(), "0:145,4:4012,7:2003,9:7001,6:5008".into())];
        g.note_identified(&e);
        g.add_player(0, Some(&e), None, 1, [0.0; 3], [0.0; 3], 0);
        let mut info = crate::up::UpPlayerInfo::default();
        info.look.bytes = [0, 0, 0, 0, 18, 1, 0, 0, 0, 1, -1, -1, 1, 0, 0];
        g.on_up(0, Up::PlayerInfo(Box::new(info.clone())), 10);
        assert_eq!(
            g.players[&0].look.bytes,
            [0, 0, 0, 0, 18, 1, 0, 12, 3, 1, 1, 8, 1, 0, -1]
        );
        // no head item, no goggles: the start values
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        e.extra = vec![("loadout".into(), "0:145,9:7002".into())];
        g.note_identified(&e);
        g.add_player(0, Some(&e), None, 1, [0.0; 3], [0.0; 3], 0);
        info.look.bytes = [0, 0, 0, 0, 18, 1, 0, 5, 5, 1, 5, 5, 1, 0, 0];
        g.on_up(0, Up::PlayerInfo(Box::new(info)), 10);
        assert_eq!(
            g.players[&0].look.bytes,
            [0, 0, 0, 0, 18, 1, 0, 0, 0, 1, 2, -1, 1, 0, -1]
        );
    }

    /// The death note: (index in the output, receiver, object) of every 0xa6 [0x18] sub 0x1d.
    fn death_notes(o: &[GameOut]) -> Vec<(usize, u8, u16)> {
        o.iter()
            .enumerate()
            .filter_map(|(i, x)| match x {
                GameOut::Unreliable(to, m)
                    if m.as_bytes().len() == 6
                        && m.as_bytes()[..3] == [0x0d, 0xa6, 0x18]
                        && m.as_bytes()[5] == gs::NOTE_DEATH =>
                {
                    let b = m.as_bytes();
                    Some((i, *to, u16::from_le_bytes([b[3], b[4]])))
                }
                _ => None,
            })
            .collect()
    }

    /// DSV-91: a lethal hit sends the victim character's death note to every introduced player, unreliable, after
    /// the shot effect; a hit that does not kill sends none.
    // OBSERVED: capture 20261025_065016_tdm6 seq 10134 (the note after the lethal shot effects)
    #[test]
    fn a_death_sends_the_death_note_after_the_shot_effect() {
        let mut g = duel(GameTuning {
            move_check: false,
            ..GameTuning::default()
        });
        let f = hit_report(&g, 0, 1, 0.0, None);
        g.on_up(0, Up::Fire(Box::new(f.clone())), 100);
        assert_eq!(g.players[&1].health, 75.0);
        assert!(
            death_notes(&g.take_out()).is_empty(),
            "no note without a death"
        );
        let mut g = duel(GameTuning {
            move_check: false,
            default_damage: 200.0,
            ..GameTuning::default()
        });
        g.on_up(0, Up::Fire(Box::new(f)), 100);
        let o = g.take_out();
        assert!(matches!(g.players[&1].life, Life::Dying { .. }));
        let notes = death_notes(&o);
        let victim = g.players[&1].obj;
        assert_eq!(
            notes.iter().map(|n| (n.1, n.2)).collect::<Vec<_>>(),
            vec![(0, victim), (1, victim)],
            "one note to each player"
        );
        let shot = o
            .iter()
            .position(|x| matches!(x, GameOut::Unreliable(0, m) if m.as_bytes()[1] == gs::FX_SHOT))
            .expect("the shot effect");
        // (no character tables here: no death presentation; game_sim checks the note with the corpse)
        assert!(shot < notes[0].0, "after the shot effect");
    }

    /// DSV-92: with `combat.blast_los` on a wall between the blast and a player keeps the player's damage at 0; off,
    /// every living player within the radius takes the falloff damage.
    #[test]
    fn blast_line_of_sight_switch() {
        use dc_sim::{CollModel, CollisionWorld, ConvexPoly, SURF_SOLID, V3};
        let wall = ConvexPoly::from_corners(
            vec![
                V3::new(0.0, -100.0, 500.0),
                V3::new(0.0, 300.0, 500.0),
                V3::new(0.0, 300.0, -500.0),
                V3::new(0.0, -100.0, -500.0),
            ],
            SURF_SOLID,
        );
        let blast = |los: bool| {
            let mut cfg = GameTuning {
                move_check: false,
                ..GameTuning::default()
            };
            cfg.projectile.blast_los = los;
            let mut g = duel(cfg);
            g.set_trace_world(Some(CollisionWorld::from_parts(
                vec![CollModel::from_polys(std::slice::from_ref(&wall))],
                &[],
            )));
            // a resting grenade of player 0 at x -50: the thrower (x -300, 250 away) on its side of the wall, the
            // enemy (x 300, 350 away) behind it; the defaults (no ROM row): 100 damage, radius 400
            g.flights.push(projectile::Flight {
                key: 1,
                owner: 0,
                owner_obj: g.players[&0].obj,
                owner_team: g.players[&0].team,
                weapon: 0,
                ammo: 0,
                origin: [-50.0, 0.0, 0.0],
                pos: [-50.0, 0.0, 0.0],
                vel: [0.0; 3],
                gravity: 0.0,
                detonate_at: 100,
                last: 100,
                resting: true,
                impact: false,
                burst: false,
                mine: None,
            });
            g.tick_projectiles(100);
            let o = g.take_out();
            let walled = o
                .iter()
                .any(|x| matches!(x, GameOut::Log(l) if l.contains("behind a wall")));
            (g.players[&0].health, g.players[&1].health, walled)
        };
        let f = |d: f32| 100.0 * blast_factor(d, 400.0, BLAST_INNER_FRACTION);
        let (h0, h1, walled) = blast(true);
        assert!((h0 - (100.0 - f(250.0))).abs() < 1e-3, "thrower {h0}");
        assert_eq!(h1, 100.0, "behind the wall");
        assert!(walled);
        let (h0, h1, walled) = blast(false);
        assert!((h0 - (100.0 - f(250.0))).abs() < 1e-3, "thrower {h0}");
        assert!(
            (h1 - (100.0 - f(350.0))).abs() < 1e-3,
            "through the wall {h1}"
        );
        assert!(!walled);
    }

    /// DSV-165: `room.blast_damage_scale` multiplies a blast's damage to players (2.0 doubles the falloff damage).
    #[test]
    fn blast_damage_scale_multiplies_a_blast_hit() {
        let lost = |scale: f32| {
            let mut cfg = GameTuning {
                move_check: false,
                ..GameTuning::default()
            };
            cfg.projectile.blast_damage_scale = scale;
            let mut g = duel(cfg);
            // a resting grenade of player 0 at the origin: the enemy (x 300) and the thrower (x -300) 300 units away; the defaults (no ROM row): 100 damage, radius 400
            g.flights.push(projectile::Flight {
                key: 1,
                owner: 0,
                owner_obj: g.players[&0].obj,
                owner_team: g.players[&0].team,
                weapon: 0,
                ammo: 0,
                origin: [0.0; 3],
                pos: [0.0; 3],
                vel: [0.0; 3],
                gravity: 0.0,
                detonate_at: 100,
                last: 100,
                resting: true,
                impact: false,
                burst: false,
                mine: None,
            });
            g.tick_projectiles(100);
            100.0 - g.players[&1].health
        };
        let one = lost(1.0);
        let want = 100.0 * blast_factor(300.0, 400.0, BLAST_INNER_FRACTION);
        assert!(
            one > 0.0 && (one - want).abs() < 1e-3,
            "scale 1: {one} vs {want}"
        );
        let two = lost(2.0);
        assert!(
            (two - 2.0 * one).abs() < 1e-3,
            "scale 2: {two} vs {}",
            2.0 * one
        );
        assert_eq!(lost(0.0), 0.0);
    }

    /// DSV-166: a blast inside the victim's spawn window (start note sent, end note not yet) deals 0; after the end
    /// note the same blast deals the normal falloff damage.
    /// DSV-477 amendment: a fire within `SPAWN_FIRE_GRACE_MS` of the spawn (a shot in flight at the death) leaves the
    /// new window open; a later one ends it and sends the end note to the start note's recipients.
    #[test]
    fn a_fire_right_after_the_spawn_leaves_the_window_open() {
        let mut g = duel(GameTuning {
            move_check: false,
            ..GameTuning::default()
        });
        let obj = g.players[&1].obj;
        g.players.get_mut(&1).unwrap().spawned_at = 1000;
        g.spawn_notes.push(SpawnNote {
            pid: 1,
            obj,
            due: 1000 + SPAWN_NOTE_MS,
            to: vec![0, 1],
        });
        g.out.clear();
        g.end_spawn_note_of(1, 1000 + SPAWN_FIRE_GRACE_MS - 1);
        assert_eq!(
            g.spawn_notes.len(),
            1,
            "a shot in flight at the death keeps the shield"
        );
        assert!(g.out.is_empty());
        g.end_spawn_note_of(1, 1000 + SPAWN_FIRE_GRACE_MS);
        assert!(
            g.spawn_notes.is_empty(),
            "the first shot of the new life ends the window"
        );
        let end = gs::spawn_note(obj, gs::NOTE_SPAWN_END);
        let ends = g
            .out
            .iter()
            .filter(|o| matches!(o, GameOut::To(_, m) if m.as_bytes() == end.as_bytes()))
            .count();
        assert_eq!(ends, 2, "one end note per recipient of the start note");
    }

    #[test]
    fn a_blast_inside_the_spawn_window_deals_nothing() {
        let mut g = duel(GameTuning {
            move_check: false,
            ..GameTuning::default()
        });
        // player 1's spawn window: open until 3600
        g.spawn_notes.push(SpawnNote {
            pid: 1,
            obj: g.players[&1].obj,
            due: 3600,
            to: vec![0, 1],
        });
        let throw = |g: &mut Game, key: u32, at: Millis| {
            g.flights.push(projectile::Flight {
                key,
                owner: 0,
                owner_obj: g.players[&0].obj,
                owner_team: g.players[&0].team,
                weapon: 0,
                ammo: 0,
                origin: [0.0; 3],
                pos: [0.0; 3],
                vel: [0.0; 3],
                gravity: 0.0,
                detonate_at: at,
                last: at,
                resting: true,
                impact: false,
                burst: false,
                mine: None,
            });
            g.tick_projectiles(at);
            g.take_out()
        };
        let o = throw(&mut g, 1, 100);
        assert_eq!(g.players[&1].health, 100.0, "inside the window");
        assert!(o
            .iter()
            .any(|x| matches!(x, GameOut::Log(l) if l.contains("spawn-shielded"))));
        g.tick_spawn_notes(3600);
        throw(&mut g, 2, 3700);
        let want = 100.0 * blast_factor(300.0, 400.0, BLAST_INNER_FRACTION);
        assert!(
            (100.0 - g.players[&1].health - want).abs() < 1e-3,
            "after the window: {}",
            g.players[&1].health
        );
    }

    /// DSV-630 / DSV-631: a flash blast (ammunition `AreaDamageType` [`projectile::DAMAGE_FLASH`]) sends every
    /// living player in its radius the 0x66 kind 0x0c screen flash, fading, for the falloff x the facing factor of
    /// the full seconds (the thrower too), and hurts by the row's area damage as a blast unless `flash_damage` is off.
    #[test]
    fn a_flash_blast_whites_out_the_players_in_reach_and_hurts_as_a_blast() {
        let flash_game = |tuning: GameTuning| {
            let mut t = tables();
            // synthetic flash row 0 with ammunition row 0 (made-up numbers in the ROM key shape)
            let doc = dc_bute::ButeDoc::parse(concat!(
                "[Weapon0]
Name = \"F\"
AmmoName0 = \"B\"
",
                "[Ammo0]
Name = \"B\"
Type = 1
AreaDamage = 25
AreaDamageType = 34
AreaDamageRadius = 1000
",
                "[WeaponOrder]
Class0 = \"P\"
P0 = \"F\"
",
            ));
            t.weapons = crate::rom_tables::WeaponTable::from_doc(&doc);
            let mut g = Game::new(tuning, &world(), t);
            for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3)] {
                let e = entry(uid, team, "P");
                g.note_identified(&e);
                g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
                g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
                g.on_up(pid, Up::WorldReady, 20);
            }
            // the thrower at the blast; the other 600 units out along +Z, looking further along +Z (blast behind)
            g.players.get_mut(&0).unwrap().pos = [0.0, 0.0, 0.0];
            let p1 = g.players.get_mut(&1).unwrap();
            p1.pos = [0.0, 0.0, 600.0];
            p1.yaw = 0.0;
            g.spawn_notes.clear();
            g.take_out();
            g.flights.push(projectile::Flight {
                key: 1,
                owner: 0,
                owner_obj: g.players[&0].obj,
                owner_team: g.players[&0].team,
                weapon: 0,
                ammo: 0,
                origin: [0.0; 3],
                pos: [0.0; 3],
                vel: [0.0; 3],
                gravity: 0.0,
                detonate_at: 100,
                last: 100,
                resting: true,
                impact: false,
                burst: false,
                mine: None,
            });
            g.tick_projectiles(100);
            let out = g.take_out();
            (g, out)
        };
        let flashes = |out: &[GameOut], pid: u8| -> Vec<Vec<u8>> {
            out.iter()
                .filter_map(|o| match o {
                    GameOut::To(to, m) if *to == pid => Some(m.as_bytes().to_vec()),
                    _ => None,
                })
                .filter(|b| {
                    b.len() >= 3 && b[1] == gs::GS_PLAYER_STATUS && b[2] == gs::STATUS_SCREEN_FLASH
                })
                .collect()
        };
        let tuning = GameTuning {
            move_check: false,
            ..GameTuning::default()
        };
        let pt = tuning.projectile;
        let (g, out) = flash_game(tuning.clone());
        let full = gs::screen_flash(true, pt.flash_secs);
        assert_eq!(
            flashes(&out, 0),
            vec![full.as_bytes().to_vec()],
            "the thrower: full length"
        );
        let behind = projectile::flash_secs(
            600.0,
            1000.0,
            pt.inner_fraction,
            -1.0,
            pt.flash_secs,
            pt.flash_behind,
        );
        assert!(behind > 0.0 && behind < pt.flash_secs * pt.flash_behind);
        assert_eq!(
            flashes(&out, 1),
            vec![gs::screen_flash(true, behind).as_bytes().to_vec()],
            "the other: the falloff x the behind factor"
        );
        assert!(
            g.players[&0].health < 100.0,
            "the row's area damage at the centre"
        );
        assert!(
            g.players[&1].health < 100.0 && g.players[&1].health > g.players[&0].health,
            "less further out"
        );
        // flash_damage=off: the flash only
        let mut t = tuning;
        t.set_combat("flash_damage", "off").unwrap();
        let (g, out) = flash_game(t);
        assert_eq!(flashes(&out, 1).len(), 1);
        assert_eq!(
            g.players[&0].health, 100.0,
            "no damage with flash_damage off"
        );
        assert_eq!(
            g.players[&1].health, 100.0,
            "no damage with flash_damage off"
        );
    }

    /// A synthetic fire row (weapon 0, ammunition 0; made-up numbers in the ROM key shape): progressive damage 4 per
    /// second of type 3 for 1 s after the last contact, within 300 units, lingering 2 s, no blast.
    fn hazard_weapons() -> crate::rom_tables::WeaponTable {
        crate::rom_tables::WeaponTable::from_doc(&dc_bute::ButeDoc::parse(concat!(
            "[Weapon0]\nName = \"I\"\nAmmoName0 = \"IB\"\n",
            "[Ammo0]\nName = \"IB\"\nType = 1\nAreaDamage = 0\nAreaDamageRadius = 0\nProgDamage = 4\n",
            "ProgDamageType = 3\nProgDamageDuration = 1\nProgDamageRadius = 300\nProgDamageLifetime = 2\n",
            "[WeaponOrder]\nClass0 = \"P\"\nP0 = \"I\"\n",
        )))
    }

    /// The fire of [`hazard_weapons`] thrown by player 0 goes off at `at` at `pos`.
    fn hazard_goes_off(g: &mut Game, pos: [f32; 3], at: Millis) {
        g.flights.push(projectile::Flight {
            key: 77,
            owner: 0,
            owner_obj: g.players[&0].obj,
            owner_team: g.players[&0].team,
            weapon: 0,
            ammo: 0,
            origin: pos,
            pos,
            vel: [0.0; 3],
            gravity: 0.0,
            detonate_at: at,
            last: at,
            resting: true,
            impact: false,
            burst: false,
            mine: None,
        });
        g.tick_projectiles(at);
    }

    /// DSV-635 / DSV-566: during a bomb room's warm-up a thrown fire burns no one (its burns have no killer, so the
    /// hold at the damage point would not see them); with `warmup_damage` on it burns as ever.
    #[test]
    fn a_thrown_fire_burns_no_one_while_the_warmup_holds_damage() {
        let burnt = |warmup_damage: bool| {
            let mut g = bomb_room_with(GameTuning {
                move_check: false,
                warmup_damage,
                ..GameTuning::default()
            });
            g.tables.weapons = hazard_weapons();
            assert!(g.in_warmup());
            g.spawn_notes.clear();
            g.players.get_mut(&0).unwrap().pos = [0.0; 3];
            g.players.get_mut(&1).unwrap().pos = [100.0, 0.0, 0.0];
            hazard_goes_off(&mut g, [0.0; 3], 1000);
            let mut t = 1000;
            while t < 5000 {
                t += 50;
                g.tick_hazards(t);
                g.tick_burns(t);
            }
            assert!(g.in_warmup(), "still the warm-up");
            100.0 - g.players[&1].health
        };
        assert_eq!(burnt(false), 0.0, "the warm-up holds the fire");
        assert!(burnt(true) > 0.0, "warmup_damage on: it burns");
    }

    /// DSV-635: the thrower is known by his account: a newcomer given the thrower's client id while the fire lasts is
    /// not taken for him (with `hazard_self` off the thrower is spared, the newcomer is not); a burn set by the
    /// thrower carries his Spy Hunt damage factor; a hazard's renewal keeps a credited burn's burner.
    #[test]
    fn a_hazard_knows_its_thrower_by_account_and_keeps_a_credited_burn() {
        let mut g = launcher_game(tables(), &[[0.0; 3], [5000.0, 0.0, 0.0]]);
        g.tables.weapons = hazard_weapons();
        g.cfg.projectile.hazard_self = false;
        hazard_goes_off(&mut g, [0.0; 3], 4000);
        g.tick_hazards(4000);
        g.tick_burns(4300);
        assert_eq!(g.players[&0].health, 100.0, "the thrower is spared");
        // the thrower leaves; a newcomer of the other team takes client id 0 and stands in the fire
        g.remove_player(0, false);
        let e = entry(99, 1, "N");
        g.note_identified(&e);
        g.add_player(0, Some(&e), None, 41, [0.0; 3], [0.0; 3], 4400);
        g.on_up(0, Up::PlayerInfo(Box::default()), 4410);
        g.on_up(0, Up::WorldReady, 4420);
        g.players.get_mut(&0).unwrap().pos = [0.0; 3];
        g.spawn_notes.clear();
        let mut t = 4400;
        while t < 7000 {
            t += 50;
            g.tick_hazards(t);
            g.tick_burns(t);
        }
        assert!(g.players[&0].health < 100.0, "the newcomer burns");
        // the agent's factor (DSV-456) on a burn he set by a hazard; a renewal keeps a flame's burner and credit
        let prog = g.tables.weapons.prog(0, 0).unwrap();
        g.burns.clear();
        g.spy.agent = Some(1);
        g.cfg.agent_damage_scale = 3.0;
        g.ignite(0, 1, 0, prog, 8000);
        assert_eq!((g.burns[0].owner, g.burns[0].credit), (1, false));
        g.burns[0].credit = true;
        g.ignite(0, gs::SHOT_CLIENT_NONE, 0, prog, 8100);
        assert_eq!(
            (g.burns[0].owner, g.burns[0].credit),
            (1, true),
            "a credited burn keeps its burner"
        );
        g.burns[0].credit = false;
        let before = g.players[&0].health;
        g.tick_burns(8350);
        assert!(
            (before - g.players[&0].health - 3.0).abs() < 1e-3,
            "3 x the row's 1 per step"
        );
    }

    /// DSV-636: a burn or poison death with no killer carries its damage type as `feed_value` (the 2009 feed's burn
    /// and poison images); a credited death and every other killerless death keep the reference value.
    #[test]
    fn a_killerless_burn_or_poison_death_takes_its_feed_image() {
        use combat::{killerless_feed, DAMAGE_BURN, DAMAGE_POISON};
        assert_eq!(killerless_feed(None, DAMAGE_BURN), 3);
        assert_eq!(killerless_feed(None, DAMAGE_POISON), 9);
        assert_eq!(killerless_feed(Some(1), DAMAGE_BURN), TODO_KILL_FEED_VALUE);
        assert_eq!(killerless_feed(None, 7), TODO_KILL_FEED_VALUE);
        assert_eq!(killerless_feed(None, 0), TODO_KILL_FEED_VALUE);
    }

    /// DSV-630: the flash's length rule.
    #[test]
    fn the_flash_falls_off_with_the_distance_and_halves_behind() {
        use projectile::{facing_cos, flash_secs};
        assert_eq!(flash_secs(100.0, 1000.0, 0.25, 1.0, 6.0, 0.5), 6.0);
        assert_eq!(flash_secs(100.0, 1000.0, 0.25, -0.2, 6.0, 0.5), 3.0);
        assert!((flash_secs(625.0, 1000.0, 0.25, 0.0, 6.0, 0.5) - 3.0).abs() < 1e-5);
        assert_eq!(flash_secs(1000.0, 1000.0, 0.25, 1.0, 6.0, 0.5), 0.0);
        // yaw 0 looks along +Z
        assert!((facing_cos(0.0, [0.0; 3], [0.0, 0.0, 50.0]) - 1.0).abs() < 1e-6);
        assert!((facing_cos(0.0, [0.0; 3], [0.0, 0.0, -50.0]) + 1.0).abs() < 1e-6);
        assert!(facing_cos(std::f32::consts::FRAC_PI_2, [0.0; 3], [50.0, 0.0, 0.0]) > 0.999);
        assert_eq!(
            facing_cos(1.0, [0.0; 3], [0.0, 80.0, 0.0]),
            1.0,
            "straight above: facing"
        );
    }

    /// DSV-181: a launcher's shot (ammunition `Type` 1 with a projectile row) arrives without a hit; the server
    /// flies the rocket and blows it up on the enemy it meets, who takes the row's area damage (x the room scale).
    #[test]
    fn a_rocket_flies_and_blows_up_on_the_enemy() {
        let mut t = tables();
        // synthetic launcher row 3 with a projectile ammunition row 0 (made-up numbers in the ROM key shape)
        let doc = dc_bute::ButeDoc::parse(concat!(
            "[Weapon3]\nName = \"L\"\nAmmoName0 = \"R\"\n",
            "[Ammo0]\nName = \"R\"\nType = 1\nAreaDamage = 40\nAreaDamageType = 7\nAreaDamageRadius = 550\n",
            "ProjectileFXName = \"Missile\"\n",
            "[WeaponOrder]\nClass0 = \"P\"\nP0 = \"L\"\n",
        ));
        t.weapons = crate::rom_tables::WeaponTable::from_doc(&doc);
        t.projectiles.rows.insert(
            dc_bute::fold_latin1("Missile"),
            crate::rom_tables::ProjectileRow {
                gravity: Some(false),
                lifetime_s: Some(10.0),
                velocity: Some(1300.0),
                ..Default::default()
            },
        );
        assert!(t.weapons.fires_projectile(3, 0));
        let mut g = Game::new(
            GameTuning {
                move_check: false,
                ..GameTuning::default()
            },
            &world(),
            t,
        );
        for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3)] {
            let e = entry(uid, team, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        g.players.get_mut(&0).unwrap().pos = [0.0, 0.0, 0.0];
        g.players.get_mut(&1).unwrap().pos = [600.0, 0.0, 0.0];
        g.spawn_notes.clear();
        g.take_out();
        let origin = [0.0, 33.0, 0.0];
        let d = sub3([600.0, 0.0, 0.0], origin);
        let l = len3(d);
        let f = crate::up::UpFire {
            weapon_id: 3,
            ammo_id: 0,
            origin,
            dir: [d[0] / l, d[1] / l, d[2] / l],
            last: true,
            hit: None,
            complete: true,
            ..crate::up::UpFire::default()
        };
        g.on_up(0, Up::Fire(Box::new(f)), 4000);
        assert_eq!(g.flights_in_air(), 1, "the rocket flies");
        assert_eq!(g.players[&1].health, 100.0, "no damage at the shot");
        g.tick_projectiles(4200);
        assert_eq!(g.flights_in_air(), 1, "260 units out after 0.2 s");
        g.tick_projectiles(4600);
        assert_eq!(g.flights_in_air(), 0, "blew up on the enemy");
        let lost = 100.0 - g.players[&1].health;
        let scale = g.cfg.projectile.blast_damage_scale;
        assert!(
            (lost - 40.0 * scale).abs() < 1e-3 || g.players[&1].life != Life::Alive,
            "lost {lost}"
        );
    }

    /// DSV-615 / DSV-616: the heartbeat sensor's switch is taken only with a sensor row drawn by a living player, is
    /// answered with the radar switch, gives the scan reach while on and ends with "off", a weapon change or a spawn.
    #[test]
    fn the_heartbeat_switch_gives_the_scan_reach_of_the_drawn_sensor() {
        let mut t = tables();
        // synthetic rows (made-up numbers in the ROM key shape): 4 a sensor, 5 a gun, 6 a scanner of another option
        let doc = dc_bute::ButeDoc::parse(concat!(
            "[Weapon4]\nName = \"S\"\nGuntype = 11\nSensorOption = 0\nScanDist = 900\n",
            "[Weapon5]\nName = \"G\"\n",
            "[Weapon6]\nName = \"Q\"\nGuntype = 11\nSensorOption = 2\nScanDist = 900\n",
            "[WeaponOrder]\nClass0 = \"P\"\nP0 = \"S\"\nP1 = \"G\"\nP2 = \"Q\"\n",
        ));
        t.weapons = crate::rom_tables::WeaponTable::from_doc(&doc);
        assert_eq!(t.weapons.weapons[&4].scan_dist, Some(900.0));
        assert_eq!(t.weapons.weapons[&6].scan_dist, None);
        let mut g = Game::new(GameTuning::default(), &world(), t);
        let e = entry(7, 0, "P");
        g.note_identified(&e);
        g.add_player(0, Some(&e), None, 1, [0.0; 3], [0.0; 3], 0);
        g.on_up(0, Up::PlayerInfo(Box::default()), 10);
        g.on_up(0, Up::WorldReady, 20);
        let ws: Vec<WeaponState> = [4, 5, 6].iter().map(|w| g.new_weapon(*w, None)).collect();
        let p = g.players.get_mut(&0).unwrap();
        p.life = Life::Alive;
        p.weapons = ws;
        p.current = Some(1);
        g.take_out();
        let answers = |g: &mut Game| -> Vec<Vec<u8>> {
            g.take_out()
                .iter()
                .filter_map(|o| match o {
                    GameOut::To(0, m) if m.as_bytes()[1] == crate::gs::GS_RADAR_PING => {
                        Some(m.as_bytes()[2..8].to_vec())
                    }
                    _ => None,
                })
                .collect()
        };
        let ans = |on: u8, w: u32| {
            let mut v = vec![1, on];
            v.extend_from_slice(&w.to_le_bytes());
            v
        };
        // the gun drawn: refused, answered off once; a repeat gets no answer
        g.on_up(0, Up::Scan { team: 0, on: true }, 100);
        assert_eq!(g.scan_reach(0), None);
        assert_eq!(answers(&mut g), vec![ans(0, 5)]);
        g.on_up(0, Up::Scan { team: 0, on: true }, 110);
        assert!(
            answers(&mut g).is_empty(),
            "a repeated switch is not answered"
        );
        // the sensor drawn later (its weapon change after the switch): the waiting switch counts, answered on
        g.on_up(
            0,
            Up::WeaponChange {
                slot: 0,
                weapon_id: 4,
                aux: 0,
            },
            200,
        );
        assert_eq!(g.players[&0].current, Some(0));
        assert_eq!(g.scan_reach(0), Some(900.0));
        assert_eq!(answers(&mut g), vec![ans(1, 4)]);
        g.on_up(0, Up::Scan { team: 0, on: true }, 210);
        assert!(answers(&mut g).is_empty(), "a repeated on is not answered");
        // another weapon drawn while on: no reach, answered off
        g.on_up(
            0,
            Up::WeaponChange {
                slot: 1,
                weapon_id: 5,
                aux: 0,
            },
            250,
        );
        assert_eq!(g.scan_reach(0), None);
        assert_eq!(answers(&mut g), vec![ans(0, 5)]);
        g.players.get_mut(&0).unwrap().current = Some(0);
        assert_eq!(g.scan_reach(0), Some(900.0));
        // dead: none; off: none
        g.players.get_mut(&0).unwrap().life = Life::Dying { died: 300 };
        assert_eq!(g.scan_reach(0), None);
        g.players.get_mut(&0).unwrap().life = Life::Alive;
        g.on_up(0, Up::Scan { team: 0, on: false }, 400);
        assert_eq!(g.scan_reach(0), None);
        // a scanner of another option is no heartbeat sensor
        g.players.get_mut(&0).unwrap().current = Some(2);
        g.on_up(0, Up::Scan { team: 0, on: true }, 500);
        assert_eq!(g.scan_reach(0), None);
        // a player not in the match is ignored
        g.players.get_mut(&0).unwrap().current = Some(0);
        g.players.get_mut(&0).unwrap().introduced = false;
        g.take_out();
        g.on_up(0, Up::Scan { team: 0, on: true }, 550);
        assert!(answers(&mut g).is_empty());
        assert_eq!(g.scan_reach(0), None);
        g.players.get_mut(&0).unwrap().introduced = true;
        // the round's end switches it off and says so
        g.on_up(0, Up::Scan { team: 0, on: true }, 560);
        assert!(g.scan_reach(0).is_some());
        g.take_out();
        g.scan_stop_all();
        assert_eq!(g.scan_reach(0), None);
        assert_eq!(answers(&mut g), vec![ans(0, 4)]);
        // a spawn switches it off
        g.on_up(0, Up::Scan { team: 0, on: true }, 600);
        assert!(g.scan_reach(0).is_some());
        g.spawn(0, 700);
        assert!(!g.players[&0].scanning);
    }

    /// A game with a synthetic mine: weapon 3 with its ammunition row 0 (`Type` 1, a blast, `MaxAmount` 2) whose
    /// projectile row names a mine class-data row (made-up numbers in the ROM key shape: detonate radius 50, armed
    /// 2 s after it rests, a 5 s lifetime that is no fuse). Players 0 (the thrower) and 2 are team 0, player 1 is
    /// team 1; all alive, out of their spawn windows.
    fn mine_game(ff: bool) -> Game {
        let mut t = tables();
        let doc = dc_bute::ButeDoc::parse(concat!(
            "[Weapon3]\nName = \"Mine\"\nAmmoName0 = \"M\"\n",
            "[Ammo0]\nName = \"M\"\nType = 1\nMaxAmount = 2\nAreaDamage = 40\nAreaDamageType = 7\n",
            "AreaDamageRadius = 300\nProjectileFXName = \"TestMine\"\n",
            "[Weapon4]\nName = \"Ctl\"\nAmmoName0 = \"C\"\nShotsPerClip = 1\nSubControlleeWeaponID = 3\n",
            "[Ammo1]\nName = \"C\"\nType = 5\nMaxAmount = 2\n",
            "[WeaponOrder]\nClass0 = \"P\"\nP0 = \"Mine\"\n",
        ));
        t.weapons = crate::rom_tables::WeaponTable::from_doc(&doc);
        t.projectiles = crate::rom_tables::ProjectileTable::from_doc(&dc_bute::ButeDoc::parse(concat!(
            "[ProjectileFX0]\nName = \"TestMine\"\nClass = \"CTestTrap\"\nClassData = \"TestTrapData\"\n",
            "Gravity = 1\nGravityOverride = -1000.0\nLifetime = 5.0\nVelocity = 300\n",
            "[LandTrapClassData0]\nName = \"TestTrapData\"\nDetonateRadius = 50\nArmDelay = 2.0\n",
        )));
        assert!(t.weapons.fires_projectile(3, 0), "a Type 1 round");
        let mut g = Game::new(
            GameTuning {
                move_check: false,
                ..GameTuning::default()
            },
            &world(),
            t,
        );
        g.rules.team_damage = ff;
        for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3), (2, 9, 0, 5)] {
            let e = entry(uid, team, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        for (pid, x) in [(0u8, 0.0f32), (1, 2000.0), (2, -2000.0)] {
            let p = g.players.get_mut(&pid).unwrap();
            p.pos = [x, 0.0, 0.0];
            p.life = Life::Alive;
        }
        g.spawn_notes.clear();
        g.take_out();
        g
    }

    /// Player 0 throws the synthetic mine at his feet (no hold: the release alone) at `now`.
    fn throw_mine(g: &mut Game, now: Millis) {
        let f = crate::up::UpFire {
            weapon_id: 3,
            ammo_id: 0,
            origin: [0.0, 33.0, 0.0],
            dir: [0.0, -1.0, 0.0],
            last: true,
            hit: None,
            complete: true,
            ..crate::up::UpFire::default()
        };
        g.on_up(0, Up::Fire(Box::new(f)), now);
    }

    /// DSV-611: a mine (ammunition `Type` 1 whose projectile row names mine class data) is thrown, not flown as a
    /// rocket: it lands and lies past the grenade fuse and its row's lifetime, is armed the delay after it rests, is
    /// not set off by its thrower or a team mate (friendly fire on or off), and blows up when an enemy comes within
    /// its radius: the enemy takes the ammunition row's blast, credited to the thrower.
    #[test]
    fn a_mine_lies_armed_and_goes_off_when_an_enemy_comes_near() {
        for ff in [false, true] {
            let mut g = mine_game(ff);
            assert!(g.mine_row(3, 0).is_some());
            throw_mine(&mut g, 4000);
            assert_eq!(g.flights_in_air(), 1, "ff {ff}: the mine is thrown");
            assert!(g.flights[0].mine.is_some() && !g.flights[0].impact);
            let o = g.take_out();
            assert!(
                o.iter().any(|x| matches!(x, GameOut::Projectile(_))),
                "ff {ff}: the object is created"
            );
            // it falls and lies; no fuse (grenade default 3 s, the row's 5 s)
            let mut now = 4000;
            let mut rest = None;
            while now < 20_000 {
                now += 100;
                g.tick_projectiles(now);
                if rest.is_none() && g.flights[0].resting {
                    rest = Some(now);
                }
            }
            assert_eq!(g.flights_in_air(), 1, "ff {ff}: no fuse");
            let rest = rest.expect("it comes to rest");
            assert_eq!(g.flights[0].mine.unwrap().armed_at, Some(rest + 2000));
            assert!(!g
                .take_out()
                .iter()
                .any(|x| matches!(x, GameOut::ProjectileGone { .. })));
            let at = g.flight_pos(g.flights[0].key).unwrap();
            let on_it = [at[0], at[1] + 30.0, at[2]];
            // the thrower and his team mate step on it: nothing
            for pid in [0u8, 2] {
                g.players.get_mut(&pid).unwrap().pos = on_it;
                now += 100;
                g.tick_projectiles(now);
                assert_eq!(
                    g.flights_in_air(),
                    1,
                    "ff {ff}: client {pid} does not set it off"
                );
                g.players.get_mut(&pid).unwrap().pos = [-2000.0, 0.0, 0.0];
            }
            // the enemy outside the radius: nothing; on it: the blast
            g.players.get_mut(&1).unwrap().pos = [at[0] + 120.0, at[1] + 30.0, at[2]];
            now += 100;
            g.tick_projectiles(now);
            assert_eq!(g.flights_in_air(), 1, "ff {ff}: outside the radius");
            g.players.get_mut(&1).unwrap().pos = on_it;
            now += 100;
            g.tick_projectiles(now);
            assert_eq!(g.flights_in_air(), 0, "ff {ff}: the enemy set it off");
            let lost = 100.0 - g.players[&1].health;
            assert!(
                lost > 0.0 || g.players[&1].life != Life::Alive,
                "ff {ff}: hurt"
            );
            assert_eq!(
                g.players[&0].health, 100.0,
                "ff {ff}: the thrower is far away"
            );
        }
    }

    /// DSV-611: a mine is not armed before its delay: an enemy on it at once does not set it off, after the delay
    /// he does.
    #[test]
    fn a_mine_is_not_armed_before_its_delay() {
        let mut g = mine_game(false);
        throw_mine(&mut g, 4000);
        let mut now = 4000;
        while !g.flights[0].resting {
            now += 50;
            g.tick_projectiles(now);
            assert!(now < 10_000, "it comes to rest");
        }
        let at = g.flight_pos(g.flights[0].key).unwrap();
        g.players.get_mut(&1).unwrap().pos = [at[0], at[1] + 30.0, at[2]];
        for _ in 0..19 {
            now += 100;
            g.tick_projectiles(now);
        }
        assert_eq!(g.flights_in_air(), 1, "not armed yet 1.9 s after it rests");
        now += 200;
        g.tick_projectiles(now);
        assert_eq!(g.flights_in_air(), 0, "armed: the enemy on it sets it off");
    }

    /// [`mine_game`] with a made-up mesh box on the mine's projectile row: 12 wide, 24 high, its origin at the
    /// bottom. Returns the game with the mine thrown at 4000 and resting, and the time it rests.
    fn resting_mine_with_a_box() -> (Game, Millis) {
        let mut g = mine_game(false);
        let key = dc_bute::fold_latin1("TestMine");
        g.tables.projectiles.rows.get_mut(&key).unwrap().model_box =
            Some(([-6.0, 0.0, -6.0], [6.0, 24.0, 6.0]));
        throw_mine(&mut g, 4000);
        let mut now = 4000;
        while !g.flights[0].resting {
            now += 50;
            g.tick_projectiles(now);
            assert!(now < 10_000, "it comes to rest");
        }
        g.take_out();
        (g, now)
    }

    /// DSV-614: a resting mine is streamed half its mesh height below the point it rests on (the mid-height of the
    /// mesh on the surface); in flight it is streamed where it is; its trigger keeps the surface point. A mesh
    /// centred at or below its origin does not rise.
    #[test]
    fn a_resting_mine_is_drawn_half_in_the_ground() {
        let mut g = mine_game(false);
        let key = dc_bute::fold_latin1("TestMine");
        g.tables.projectiles.rows.get_mut(&key).unwrap().model_box =
            Some(([-6.0, 0.0, -6.0], [6.0, 24.0, 6.0]));
        throw_mine(&mut g, 4000);
        g.tick_projectiles(4016);
        let fl = &g.flights[0];
        assert!(!fl.resting);
        assert_eq!(g.flight_stream()[0].1, fl.pos, "in flight: where it is");
        let mut now = 4016;
        while !g.flights[0].resting {
            now += 50;
            g.tick_projectiles(now);
            assert!(now < 10_000, "it comes to rest");
        }
        let at = g.flights[0].pos;
        let (k, drawn, vel) = g.flight_stream()[0];
        assert_eq!(k, g.flights[0].key);
        assert_eq!(vel, [0.0; 3]);
        assert_eq!(
            drawn,
            [at[0], at[1] - 12.0, at[2]],
            "sunk by half the 24-unit mesh"
        );
        assert_eq!(
            g.flight_pos(k),
            Some(at),
            "the trigger and the blast keep the surface point"
        );
        // a mesh centred on its origin (a claymore-like model) does not rise; scale applies; no mesh: no sink
        assert_eq!(
            super::projectile::Mine::look(Some(([-15.0, -12.0, -5.0], [2.0, 9.0, 7.0])), [1.0; 3])
                .0,
            0.0
        );
        let (sink, mid, half) = super::projectile::Mine::look(
            Some(([-6.0, 0.0, -6.0], [6.0, 24.0, 6.0])),
            [1.0, 0.5, 1.0],
        );
        assert_eq!((sink, mid, half), (6.0, 6.0, [6.0, 6.0, 6.0]));
        assert_eq!(
            super::projectile::Mine::look(None, [1.0; 3]),
            (0.0, 0.0, super::projectile::MINE_HIT_HALF)
        );
    }

    /// Player 0's pellet of a gun (weapon 50, no row) from 200 units beside the resting mine at `at`, aimed at
    /// `target`, at `now`.
    fn shoot_at(
        g: &mut Game,
        at: [f32; 3],
        target: [f32; 3],
        hit: Option<(f32, Option<u16>)>,
        now: Millis,
    ) {
        let pos = [at[0] + 200.0, at[1], at[2]];
        g.players.get_mut(&0).unwrap().pos = pos;
        let origin = [pos[0], pos[1] + 33.0, pos[2]];
        let f = crate::up::UpFire {
            weapon_id: 50,
            ammo_id: 50,
            origin,
            dir: sub3(target, origin),
            last: true,
            hit,
            complete: true,
            ..crate::up::UpFire::default()
        };
        g.on_up(0, Up::Fire(Box::new(f)), now);
    }

    /// DSV-618: a pellet that meets a resting mine's box sets it off (its own thrower's, before it is armed): the
    /// blast follows in the next flight step; a pellet past the box, and one that claims a hit on a player or a world
    /// model in front of the mine, do not; a mine already set off takes no further pellet of the volley.
    #[test]
    fn a_shot_at_a_resting_mine_sets_it_off_and_a_miss_does_not() {
        let (mut g, mut now) = resting_mine_with_a_box();
        let at = g.flights[0].pos;
        let centre = g.flights[0].mine.unwrap().box_centre(at);
        assert_eq!(centre, at, "the mesh's mid-height on the surface");
        // 10 units beside the 6-unit half width: a miss
        now += 16;
        shoot_at(
            &mut g,
            at,
            [centre[0], centre[1], centre[2] + 10.0],
            None,
            now,
        );
        now += 16;
        g.tick_projectiles(now);
        assert_eq!(g.flights_in_air(), 1, "a pellet past the box");
        // a pellet that claims a hit 100 units out (on someone in front of the mine, 200 away): not the mine
        now += 16;
        shoot_at(&mut g, at, centre, Some((100.0, Some(3))), now);
        now += 16;
        g.tick_projectiles(now);
        assert_eq!(
            g.flights_in_air(),
            1,
            "the pellet stopped at its claimed hit"
        );
        // a claimed hit on a world model (a crate: no object) 100 units out, short of the mine: not the mine
        now += 16;
        shoot_at(&mut g, at, centre, Some((100.0, None)), now);
        now += 16;
        g.tick_projectiles(now);
        assert_eq!(
            g.flights_in_air(),
            1,
            "the pellet stopped at the world model it hit"
        );
        // at the box's top edge: the thrower's own shot sets it off before it is armed
        assert!(g.flights[0]
            .mine
            .unwrap()
            .armed_at
            .is_some_and(|t| later(t, now)));
        now += 16;
        shoot_at(
            &mut g,
            at,
            [centre[0], centre[1] + 11.0, centre[2]],
            None,
            now,
        );
        assert_eq!(
            g.flights_in_air(),
            1,
            "the blast follows in the flight step"
        );
        let again = crate::up::UpFire {
            weapon_id: 50,
            ammo_id: 50,
            origin: [at[0] + 200.0, at[1] + 33.0, at[2]],
            dir: sub3(centre, [at[0] + 200.0, at[1] + 33.0, at[2]]),
            last: true,
            complete: true,
            ..crate::up::UpFire::default()
        };
        assert!(
            !g.shoot_mine(0, &again),
            "the set-off mine takes no more pellets"
        );
        now += 16;
        g.tick_projectiles(now);
        assert_eq!(g.flights_in_air(), 0, "the shot set it off");
        let o = g.take_out();
        assert!(o
            .iter()
            .any(|x| matches!(x, GameOut::ProjectileGone { .. })));
        assert!(
            g.players[&0].health < 100.0,
            "its blast hurts the thrower 200 units away (radius 300)"
        );
    }

    /// DSV-619: a controller's fire (weapon 4, whose row names the mine weapon 3 as its controllee) sets off every
    /// resting mine of its shooter (armed or not), not another player's and not one still in flight.
    #[test]
    fn a_controller_sets_off_its_shooters_resting_mines() {
        let mut g = mine_game(false);
        assert_eq!(g.tables.weapons.controllee(4), Some(3));
        assert_eq!(g.tables.weapons.controllee(3), None);
        let mut now = 4000;
        let settle = |g: &mut Game, now: &mut Millis| {
            for _ in 0..200 {
                *now += 50;
                g.tick_projectiles(*now);
                if g.flights.iter().all(|f| f.resting) {
                    return;
                }
            }
            panic!("the mines come to rest");
        };
        throw_mine(&mut g, now);
        settle(&mut g, &mut now);
        let own = g.flights[0].key;
        // an enemy's mine 2000 units away
        let theirs = crate::up::UpFire {
            weapon_id: 3,
            ammo_id: 0,
            origin: [2000.0, 33.0, 0.0],
            dir: [0.0, -1.0, 0.0],
            last: true,
            complete: true,
            ..crate::up::UpFire::default()
        };
        g.on_up(1, Up::Fire(Box::new(theirs)), now);
        settle(&mut g, &mut now);
        // a second own mine, still in the air when the controller fires
        now += 16;
        throw_mine(&mut g, now);
        assert_eq!(g.flights_in_air(), 3);
        g.take_out();
        now += 16;
        let ctl = crate::up::UpFire {
            weapon_id: 4,
            ammo_id: 1,
            origin: [0.0, 33.0, 0.0],
            dir: [1.0, 0.0, 0.0],
            last: true,
            hit: None,
            complete: true,
            ..crate::up::UpFire::default()
        };
        g.on_up(0, Up::Fire(Box::new(ctl)), now);
        now += 16;
        g.tick_projectiles(now);
        assert_eq!(g.flights_in_air(), 2, "only the resting own mine went off");
        assert!(g.flights.iter().all(|f| f.key != own));
        assert!(g.flights.iter().any(|f| f.owner == 1 && f.resting));
        assert!(g.flights.iter().any(|f| f.owner == 0 && !f.resting));
        let o = g.take_out();
        assert!(o
            .iter()
            .any(|x| matches!(x, GameOut::ProjectileGone { key } if *key == own)));
        assert!(
            g.players[&0].health < 100.0,
            "the blast at his feet hurts the thrower"
        );
        assert_eq!(g.players[&1].health, 100.0, "2000 units away");
        // nothing of the shooter's rests any more: a second set-off finds nothing
        assert_eq!(g.set_off_mines_of(0, 3), 0);
    }

    /// DSV-612: a thrower keeps at most the ammunition row's `MaxAmount` live mines (the oldest goes without a
    /// blast); his mines go when he leaves; all go without a blast when the round stops running.
    #[test]
    fn mines_are_capped_and_go_with_their_thrower_and_the_round() {
        let mut g = mine_game(false);
        for k in 0..3 {
            throw_mine(&mut g, 4000 + k * 300);
        }
        assert_eq!(g.flights_in_air(), 2, "MaxAmount 2");
        let first = g
            .take_out()
            .iter()
            .filter(|x| matches!(x, GameOut::ProjectileGone { .. }))
            .count();
        assert_eq!(first, 1, "the oldest is removed");
        g.tick_projectiles(9000);
        g.remove_player(0, false);
        assert_eq!(g.flights_in_air(), 0, "the thrower left");
        let gone = g
            .take_out()
            .iter()
            .filter(|x| matches!(x, GameOut::ProjectileGone { .. }))
            .count();
        assert_eq!(gone, 2);
        assert_eq!(g.players[&1].health, 100.0, "no blast");
        // round end
        let mut g = mine_game(false);
        throw_mine(&mut g, 4000);
        g.tick_projectiles(9000);
        assert_eq!(g.flights_in_air(), 1);
        g.phase = RoundPhase::Waiting;
        g.tick_projectiles(9100);
        assert_eq!(g.flights_in_air(), 0, "the round stopped");
    }

    /// DSV-611: a `Type` 1 round whose projectile row's `ClassData` names no mine class-data row is no mine: it
    /// flies as a rocket (DSV-181).
    #[test]
    fn a_round_whose_class_data_names_no_trap_row_flies_as_a_rocket() {
        let mut g = mine_game(false);
        g.tables.projectiles.traps.clear();
        assert!(g.mine_row(3, 0).is_none());
        throw_mine(&mut g, 4000);
        assert_eq!(g.flights_in_air(), 1);
        assert!(
            g.flights[0].impact && g.flights[0].mine.is_none(),
            "a rocket"
        );
    }

    /// Player 0's mine thrown at 4000, ticked until it is armed; returns the time and the mine's position.
    fn armed_mine(g: &mut Game) -> (Millis, [f32; 3]) {
        throw_mine(g, 4000);
        let mut now = 4000;
        while !g.flights[0]
            .mine
            .unwrap()
            .armed_at
            .is_some_and(|t| !dc_net_engine::time::later(t, now))
        {
            now += 100;
            g.tick_projectiles(now);
            assert!(now < 20_000, "it arms");
        }
        (now, g.flight_pos(g.flights[0].key).unwrap())
    }

    /// DSV-611: a dead enemy and an enemy inside his spawn window on an armed mine do not set it off; the living
    /// enemy out of his window does.
    #[test]
    fn a_dead_or_spawn_shielded_enemy_does_not_set_a_mine_off() {
        let mut g = mine_game(false);
        let (mut now, at) = armed_mine(&mut g);
        let on_it = [at[0], at[1] + 30.0, at[2]];
        g.players.get_mut(&1).unwrap().pos = on_it;
        g.players.get_mut(&1).unwrap().life = Life::Dying { died: now };
        now += 100;
        g.tick_projectiles(now);
        assert_eq!(g.flights_in_air(), 1, "dead");
        g.players.get_mut(&1).unwrap().life = Life::Alive;
        let obj = g.players[&1].obj;
        g.spawn_notes.push(SpawnNote {
            pid: 1,
            obj,
            due: now + 60_000,
            to: vec![0, 1, 2],
        });
        assert!(g.spawn_shielded(1));
        now += 100;
        g.tick_projectiles(now);
        assert_eq!(g.flights_in_air(), 1, "spawn-shielded");
        g.spawn_notes.clear();
        now += 100;
        g.tick_projectiles(now);
        assert_eq!(g.flights_in_air(), 0, "the living enemy sets it off");
    }

    /// DSV-611: an enemy on a mine still in flight does not set it off; a mine that never comes to rest (here kept
    /// in the air with no gravity) counts as resting the row's `Lifetime` after its release, is armed after the
    /// delay, and then the enemy on it sets it off.
    #[test]
    fn a_mine_in_flight_is_not_set_off_and_settles_after_its_lifetime() {
        let mut g = mine_game(false);
        throw_mine(&mut g, 4000);
        g.flights[0].gravity = 0.0;
        g.flights[0].vel = [10.0, 0.0, 0.0];
        let mut now = 4000;
        while now < 4000 + 5000 + 2000 - 100 {
            let at = g.flight_pos(g.flights[0].key).unwrap();
            g.players.get_mut(&1).unwrap().pos = [at[0], at[1] + 30.0, at[2]];
            now += 100;
            g.tick_projectiles(now);
            assert_eq!(g.flights_in_air(), 1, "{now}: not armed yet");
            if now < 9000 {
                assert!(!g.flights[0].resting, "{now}: still in flight");
            }
        }
        assert!(g.flights[0].resting, "rests after the row's lifetime");
        assert_eq!(g.flights[0].mine.unwrap().armed_at, Some(9000 + 2000));
        now += 100;
        g.tick_projectiles(now);
        assert_eq!(g.flights_in_air(), 0, "armed: the enemy on it sets it off");
    }

    /// DSV-260: the throw relay's team byte is the thrower's team (the 2009 client plays the callout only for its own
    /// team), for a thrower of either team.
    #[test]
    fn the_throw_callout_carries_the_throwers_team() {
        let mut g = duel(GameTuning::default());
        for pid in [0u8, 1] {
            let team = g.players[&pid].team;
            let f = crate::up::UpFire {
                origin: [1.0, 2.0, 3.0],
                ..crate::up::UpFire::default()
            };
            g.throw_sound(pid, &f);
            let want = gs::throw_sound(6, u32::from(pid), f.origin, team, 0);
            let o = g.take_out();
            let sent: Vec<&GameOut> = o
                .iter()
                .filter(
                    |x| matches!(x, GameOut::Unreliable(_, m) if m.as_bytes() == want.as_bytes()),
                )
                .collect();
            assert_eq!(
                sent.len(),
                2,
                "client {pid} (team {team}): to both, the thrower included"
            );
        }
        assert_ne!(
            g.players[&0].team, g.players[&1].team,
            "one thrower per team"
        );
    }

    /// DSV-168: a blast 100 units from a standing enemy 10 s after its spawn (its window closed at 1.75 s), on open
    /// floor, deals the falloff damage times the room scale and logs the hit.
    #[test]
    fn a_blast_ten_seconds_after_the_spawn_hits_with_the_scale() {
        let lost = |scale: f32| {
            let mut cfg = GameTuning {
                move_check: false,
                ..GameTuning::default()
            };
            cfg.projectile.blast_damage_scale = scale;
            let mut g = duel(cfg);
            g.players.get_mut(&0).unwrap().pos = [-2000.0, 0.0, 0.0];
            g.players.get_mut(&1).unwrap().pos = [100.0, 0.0, 0.0];
            g.spawn_notes.push(SpawnNote {
                pid: 1,
                obj: g.players[&1].obj,
                due: 3500,
                to: vec![0, 1],
            });
            g.tick_spawn_notes(3500);
            g.flights.push(projectile::Flight {
                key: 1,
                owner: 0,
                owner_obj: g.players[&0].obj,
                owner_team: g.players[&0].team,
                weapon: 0,
                ammo: 0,
                origin: [0.0; 3],
                pos: [0.0; 3],
                vel: [0.0; 3],
                gravity: 0.0,
                detonate_at: 10_000,
                last: 10_000,
                resting: true,
                impact: false,
                burst: false,
                mine: None,
            });
            g.tick_projectiles(10_000);
            let o = g.take_out();
            assert!(o
                .iter()
                .any(|x| matches!(x, GameOut::Log(l) if l.contains("client 1 takes"))));
            100.0 - g.players[&1].health
        };
        // the defaults (no ROM row): 100 damage, radius 400; 100 units lies in the full-damage inner quarter
        let want = 100.0 * blast_factor(100.0, 400.0, BLAST_INNER_FRACTION);
        assert!((lost(0.3) - 0.3 * want).abs() < 1e-3);
        assert!((lost(0.6) - 0.6 * want).abs() < 1e-3);
    }

    /// DSV-182: the lobby's magazine mod (a socket-2 mod listed by the weapon row with `ModModel` k) adds its
    /// `ShotsPerClipPlus<k>` to the clip the spawn grants and mirrors, and `MaxAmountPlus<k>` to the stock.
    #[test]
    fn an_extended_magazine_grants_a_bigger_clip() {
        let mut t = tables();
        // made-up rows: weapon 145 clip 30, ammunition 0 MaxAmount 90, magazine mod 14 with k 0 = +10 / +40
        let doc = dc_bute::ButeDoc::parse(concat!(
            "[Mod14]\nName = \"Mag\"\nKinds = 2\nShotsPerClipPlus0 = 10\nMaxAmountPlus0 = 40\n",
            "[Ammo0]\nName = \"A\"\nMaxAmount = 90\n",
            "[Weapon145]\nName = \"R\"\n",
            "ShotsPerClip = 30\n",
            "AmmoName0 = \"A\"\nModName0 = \"Mag\"\nModModel0 = 0\n",
            "[WeaponOrder]\nClass0 = \"P\"\nP0 = \"R\"\n",
        ));
        t.weapons = crate::rom_tables::WeaponTable::from_doc(&doc);
        assert_eq!(t.weapons.magazine_bonus(145, &[-1, -1, 14, -1]), (10, 40));
        assert_eq!(t.weapons.magazine_bonus(145, &gs::NO_PARTS), (0, 0));
        let mut g = Game::new(spec_order(), &world(), t);
        let mut e = entry(7, 0, "Dave");
        e.extra = vec![
            ("loadout".into(), "0:145".into()),
            ("parts".into(), "0:-1/-1/1014".into()),
        ];
        g.add_player(0, Some(&e), None, 1, [0.0; 3], [0.0; 3], 0);
        g.on_up(0, Up::PlayerInfo(Box::default()), 10);
        g.on_up(0, Up::WorldReady, 20);
        let w = g.players[&0].weapons[0];
        assert_eq!(
            (w.weapon, w.clip_size, w.clip, w.stock),
            (145, 40, 40, 130),
            "{:?}",
            w.parts
        );
    }

    /// CLI-1440 / DSV-182 / DSV-440: a Fast Loader (negative `ShotsPerClipPlus<k>`, a ROM-style comment after the
    /// number) grants a smaller clip, the rounds it no longer holds stay in the stock, and the server's reload fills
    /// that smaller clip; a suppressor's `Instdamage` -0.1 scales the hit damage by 0.9 from the server's own grant
    /// record (off with `combat.mod_damage=off`).
    #[test]
    fn a_fast_loader_grants_a_smaller_clip_and_a_suppressor_less_damage() {
        let mut t = tables();
        // made-up rows: weapon 145 clip 30, ammunition 0 MaxAmount 90 InstDamage 50, Fast Loader 45 with k 0 = -9,
        // suppressor 3 (socket 0) with Instdamage -0.1
        let doc = dc_bute::ButeDoc::parse(concat!(
            "[Mod3]\nName = \"Sup\"\nKinds = 0\nInstdamage = -0.1\n",
            "[Mod45]\nName = \"Fast\"\nKinds = 2\nShotsPerClipPlus0 = -9//30 rounds (30%)\n",
            "[Ammo0]\nName = \"A\"\nMaxAmount = 90\nInstDamage = 50\n",
            "[Weapon145]\nName = \"R\"\n",
            "ShotsPerClip = 30\n",
            "AmmoName0 = \"A\"\nModName0 = \"Fast\"\nModModel0 = 0\nModName1 = \"Sup\"\nModModel1 = 0\n",
            "[WeaponOrder]\nClass0 = \"P\"\nP0 = \"R\"\n",
        ));
        t.weapons = crate::rom_tables::WeaponTable::from_doc(&doc);
        assert_eq!(t.weapons.magazine_bonus(145, &[-1, -1, 45, -1]), (-9, 0));
        let mut g = Game::new(spec_order(), &world(), t);
        let mut e = entry(7, 0, "Dave");
        e.extra = vec![
            ("loadout".into(), "0:145".into()),
            ("parts".into(), "0:1003/-1/1045".into()),
        ];
        g.add_player(0, Some(&e), None, 1, [0.0; 3], [0.0; 3], 0);
        g.on_up(0, Up::PlayerInfo(Box::default()), 10);
        g.on_up(0, Up::WorldReady, 20);
        let w = g.players[&0].weapons[0];
        assert_eq!(
            (w.weapon, w.clip_size, w.clip, w.stock),
            (145, 21, 21, 99),
            "{:?}",
            w.parts
        );
        // five rounds left, then the reload: the clip of 21 again from the stock
        g.players.get_mut(&0).unwrap().weapons[0].clip = 5;
        g.on_reload(0, 0);
        let w = g.players[&0].weapons[0];
        assert_eq!((w.clip, w.stock), (21, 83));
        let f = crate::up::UpFire {
            weapon_id: 145,
            ammo_id: 0,
            ..Default::default()
        };
        assert!((g.hit_damage(0, &f) - 45.0).abs() < 1e-4, "50 x 0.9");
        assert_eq!(
            g.hit_damage(1, &f),
            50.0,
            "no such player: the row's damage"
        );
        g.cfg.mod_damage = false;
        assert_eq!(g.hit_damage(0, &f), 50.0);
    }

    // ------------------------------------------------------------------------------------ team pings (DSV-651)

    fn ping_game_of(class: &str, pings: bool) -> Game {
        let mut g = Game::new(
            GameTuning {
                move_check: false,
                pings,
                ..GameTuning::default()
            },
            &world(),
            tables(),
        );
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), class.into());
        g.set_room(&room);
        // pids 0 and 2 on team 0, pid 1 on team 1
        for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3), (2, 9, 0, 5)] {
            let e = entry(uid, team, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        g
    }

    fn ping_notices(o: &[GameOut], pid: u8) -> Vec<dc_wire::teamping::Notice> {
        o.iter()
            .filter_map(|x| match x {
                GameOut::To(p, m) if *p == pid => {
                    let mut r = m.reader();
                    (r.read_u8() == crate::msg::S2C_SHELL_MSG
                        && r.read_u8() == dc_wire::teamping::SHELL_ID)
                        .then(|| dc_wire::teamping::read_notice(&mut r))
                        .flatten()
                }
                _ => None,
            })
            .collect()
    }

    /// The offer at world entry, an enemy ping to the pinger's team only (the enemy spotted for that team until the
    /// tag's time is up), the per-player cooldown, and a place ping with the longer time.
    #[test]
    fn team_pings_reach_the_team_and_spot_the_enemy_for_its_time() {
        use dc_wire::teamping::{Notice, Request};
        let mut g = ping_game_of("1", true);
        let o = g.take_out();
        let offer = Notice::Offer {
            enemy_ms: teamping::ENEMY_MS,
            place_ms: teamping::PLACE_MS,
        };
        for pid in 0..3 {
            assert_eq!(ping_notices(&o, pid), vec![offer], "offer to {pid}");
        }
        let seen = |_: u8, _: u16| true;
        let t0 = 1u16 << g.players[&0].team;
        g.team_ping(0, Request::Enemy { obj: 3 }, &seen, 1000);
        let o = g.take_out();
        let want = Notice::Enemy {
            from: 1,
            obj: 3,
            ms: teamping::ENEMY_MS,
        };
        assert_eq!(ping_notices(&o, 0), vec![want]);
        assert_eq!(ping_notices(&o, 2), vec![want]);
        assert!(ping_notices(&o, 1).is_empty(), "the enemy is not told");
        assert_eq!(g.spotted_mask(1), t0);
        // the cooldown
        g.team_ping(
            0,
            Request::Place {
                pos: [10.0, 0.0, 0.0],
            },
            &seen,
            1500,
        );
        assert!(ping_notices(&g.take_out(), 0).is_empty());
        g.team_ping(
            0,
            Request::Place {
                pos: [10.0, 0.0, 0.0],
            },
            &seen,
            2100,
        );
        assert_eq!(
            ping_notices(&g.take_out(), 2),
            vec![Notice::Place {
                from: 1,
                pos: [10.0, 0.0, 0.0],
                ms: teamping::PLACE_MS
            }]
        );
        const _: () = assert!(teamping::PLACE_MS > teamping::ENEMY_MS);
        // the spot ends with the tag's time
        g.tick(3900);
        assert_eq!(g.spotted_mask(1), t0);
        g.tick(4001);
        assert_eq!(g.spotted_mask(1), 0);
    }

    /// Refused: an enemy out of the pinger's sight, a teammate, an unknown object, a place out of reach, a dead
    /// pinger; and nothing at all in a free-for-all room or without the flag.
    #[test]
    fn team_pings_refuse_unseen_friends_far_places_and_ffa_and_are_off_by_default() {
        use dc_wire::teamping::Request;
        let mut g = ping_game_of("1", true);
        g.take_out();
        let unseen = |_: u8, _: u16| false;
        let seen = |_: u8, _: u16| true;
        g.team_ping(0, Request::Enemy { obj: 3 }, &unseen, 1000);
        g.team_ping(0, Request::Enemy { obj: 5 }, &seen, 1000);
        g.team_ping(0, Request::Enemy { obj: 77 }, &seen, 1000);
        g.team_ping(
            0,
            Request::Place {
                pos: [teamping::PLACE_REACH + 1.0, 0.0, 0.0],
            },
            &seen,
            1000,
        );
        let o = g.take_out();
        assert!((0..3).all(|p| ping_notices(&o, p).is_empty()));
        assert_eq!(g.spotted_mask(1), 0);
        g.players.get_mut(&0).unwrap().life = Life::Waiting { died: 900 };
        g.team_ping(0, Request::Enemy { obj: 3 }, &seen, 1000);
        assert!(ping_notices(&g.take_out(), 2).is_empty());
        for (class, on) in [("6", true), ("1", false)] {
            let mut g = ping_game_of(class, on);
            let o = g.take_out();
            assert!(
                (0..3).all(|p| ping_notices(&o, p).is_empty()),
                "no offer: {class} {on}"
            );
            g.team_ping(0, Request::Place { pos: [0.0; 3] }, &seen, 1000);
            let o = g.take_out();
            assert!((0..3).all(|p| ping_notices(&o, p).is_empty()));
        }
    }

    /// DSV-651 (review): every attempt counts for the cooldown, so a flood of refused pings logs once a second and a
    /// valid ping right after a refusal waits; a dead or not yet introduced target is refused; a player who leaves
    /// takes his spots and his cooldown with him.
    #[test]
    fn team_ping_refusals_are_rate_limited_and_a_leaver_takes_his_spots() {
        use dc_wire::teamping::Request;
        let mut g = ping_game_of("1", true);
        g.take_out();
        let seen = |_: u8, _: u16| true;
        let logs = |o: &[GameOut]| {
            o.iter()
                .filter(|x| matches!(x, GameOut::Log(s) if s.starts_with("ping:")))
                .count()
        };
        for t in (1000..2000).step_by(50) {
            g.team_ping(0, Request::Enemy { obj: 5 }, &seen, t);
        }
        let o = g.take_out();
        assert_eq!(logs(&o), 1, "one refusal logged in the second");
        g.team_ping(0, Request::Enemy { obj: 3 }, &seen, 1999);
        assert!(
            ping_notices(&g.take_out(), 2).is_empty(),
            "the cooldown holds after a refusal"
        );
        g.team_ping(0, Request::Enemy { obj: 3 }, &seen, 2000);
        assert_eq!(ping_notices(&g.take_out(), 2).len(), 1);
        assert_ne!(g.spotted_mask(1), 0);
        // the enemy leaves: the spot goes at once; a dead enemy is refused
        g.remove_player(1, false);
        assert_eq!(g.spotted_mask(1), 0);
        let mut g = ping_game_of("1", true);
        g.take_out();
        g.players.get_mut(&1).unwrap().life = Life::Dying { died: 900 };
        g.team_ping(0, Request::Enemy { obj: 3 }, &seen, 1000);
        g.players.get_mut(&1).unwrap().life = Life::Alive;
        g.players.get_mut(&1).unwrap().introduced = false;
        g.team_ping(2, Request::Enemy { obj: 3 }, &seen, 1000);
        let o = g.take_out();
        assert!((0..3).all(|p| ping_notices(&o, p).is_empty()));
        assert_eq!(g.spotted_mask(1), 0);
    }

    // ------------------------------------------------------------------------------------ Possession (DSV-483)

    fn possession_game_of(class: &str, gun_game: bool) -> Game {
        let mut t = tables();
        t.items.weapon_types = [(30, 0), (164, 4), (31, 2), (0, 7), (40, 0)].into();
        let mut g = Game::new(
            GameTuning {
                move_check: false,
                possession: true,
                gun_game: gun_game.then(|| gungame::GunGameSetup {
                    seed: 5,
                    ladder: vec![30, 31],
                }),
                ..GameTuning::default()
            },
            &world(),
            t,
        );
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), class.into());
        room.pairs.insert("goal".into(), "50".into());
        g.set_room(&room);
        for (pid, uid, obj, kit) in [
            (0u8, 7u32, 1u16, "0:40,2:0"),
            (1, 8, 3, "0:30,1:164,2:0"),
            (2, 9, 5, "0:31,2:0"),
        ] {
            let mut e = entry(uid, 0, "P");
            e.extra = vec![("loadout".into(), kit.into())];
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        g.players.get_mut(&0).unwrap().pos = [0.0; 3];
        g.players.get_mut(&1).unwrap().pos = [900.0, 0.0, 0.0];
        g.players.get_mut(&2).unwrap().pos = [0.0, 0.0, 300.0];
        g.spawn_notes.clear();
        g.take_out();
        g
    }

    fn possession_game() -> Game {
        possession_game_of("6", false)
    }

    /// The move: straight along the line at constant speed, the time from the distance; a seq bump at the start so
    /// reports do not count, the closing teleport at the end, and the other clients' position is `Player::pos`.
    #[test]
    fn possession_moves_in_a_line_and_closes_with_a_teleport() {
        let mut g = possession_game();
        assert!(g.is_possession());
        let seq = g.players[&0].move_seq;
        g.kill(1, Some(0), 40, false, 100);
        let f = g.players[&0].flight.expect("the move");
        assert_eq!((f.to, f.ms), ([900.0, 0.0, 0.0], 300));
        assert_eq!(g.players[&0].move_seq, seq.wrapping_add(1));
        let o = g.take_out();
        assert!(
            ids(&o, 0).contains(&dc_wire::possession::SHELL_ID),
            "the client hears"
        );
        assert!(
            !ids(&o, 1).contains(&dc_wire::possession::SHELL_ID),
            "only the killer"
        );
        g.tick_possession(250);
        assert_eq!(g.players[&0].pos, [450.0, 0.0, 0.0]);
        assert!((g.players[&0].vel[0] - 3000.0).abs() < 0.1);
        assert!(
            !ids(&g.take_out(), 0).contains(&gs::GS_MOVER_UPDATE),
            "no teleport yet"
        );
        g.tick_possession(400);
        let p = &g.players[&0];
        assert!(p.flight.is_none());
        assert_eq!((p.pos, p.vel), ([900.0, 0.0, 0.0], [0.0; 3]));
        assert_eq!(p.move_seq, seq.wrapping_add(2));
        assert!(ids(&g.take_out(), 0).contains(&gs::GS_MOVER_UPDATE));
    }

    /// project decision: at most 0.5 s at any distance.
    #[test]
    fn possession_at_a_very_long_distance_takes_half_a_second() {
        let mut g = possession_game();
        g.players.get_mut(&1).unwrap().pos = [1_000_000.0, 0.0, 0.0];
        g.kill(1, Some(0), 40, false, 100);
        assert_eq!(g.players[&0].flight.unwrap().ms, 500);
        g.tick_possession(599);
        assert!(g.in_flight(0));
        g.tick_possession(600);
        assert!(!g.in_flight(0));
        assert_eq!(g.players[&0].pos, [1_000_000.0, 0.0, 0.0]);
    }

    /// During the move: no damage from anyone, no shots, the client's reports (even far off with the movement check
    /// on) neither move the body nor draw a correction.
    #[test]
    fn possession_takes_no_damage_or_fire_and_no_anti_cheat_in_flight() {
        let mut g = possession_game();
        g.cfg.move_check = true;
        g.kill(1, Some(0), 40, false, 100);
        g.take_out();
        let health = g.players[&0].health;
        g.apply_hit_as(
            0,
            Some(2),
            500.0,
            500.0,
            [0.0, 0.0, 1.0],
            combat::DamageSource {
                weapon: 31,
                dtype: 0,
            },
            0,
            0,
            110,
        );
        assert_eq!(g.players[&0].health, health);
        assert!(matches!(g.players[&0].life, Life::Alive));
        let f = hit_report(&g, 0, 2, 0.0, None);
        g.on_up(0, Up::Fire(Box::new(f)), 120);
        assert_eq!(g.players[&2].health, 100.0, "no shot");
        assert!(effects(&g.take_out()).is_empty());
        let (corr, seq) = (g.players[&0].corrections, g.players[&0].move_seq);
        let at = g.players[&0].pos;
        fall_step(&mut g, 90_000.0, true, 0, 130);
        assert_eq!((g.players[&0].corrections, g.players[&0].pos), (corr, at));
        g.tick_possession(500);
        assert_eq!(g.players[&0].corrections, corr);
        assert_eq!(g.players[&0].move_seq, seq.wrapping_add(1));
    }

    /// The end point: the last grounded standing spot for a death in the air.
    #[test]
    fn possession_ends_on_the_last_grounded_spot_after_a_death_in_the_air() {
        let mut g = possession_game();
        let v = g.players.get_mut(&1).unwrap();
        v.stand_pos = Some([200.0, 0.0, 0.0]);
        v.pos = [260.0, 700.0, 0.0];
        v.on_ground_report = false;
        g.kill(1, Some(0), 40, false, 100);
        assert_eq!(g.players[&0].flight.unwrap().to, [200.0, 0.0, 0.0]);
    }

    /// The guns: the victim's, each full with his parts, the killer's leave (0xb4 before 0xb2); armour and health stay;
    /// a respawn brings the own loadout back.
    #[test]
    fn possession_takes_the_guns_full_with_mods_and_a_respawn_restores_the_loadout() {
        let mut g = possession_game();
        {
            let v = g.players.get_mut(&1).unwrap();
            v.weapons[0].parts = [3, -1, -1, -1];
            v.weapons[0].clip = 1;
            v.weapons[0].stock = 0;
        }
        g.players.get_mut(&0).unwrap().armor = 42.0;
        g.players.get_mut(&0).unwrap().health = 61.0;
        let fresh = g.new_weapon(30, None);
        let own = held(&g, 0);
        let victim = held(&g, 1);
        g.kill(1, Some(0), 40, false, 100);
        assert_eq!(held(&g, 0), victim);
        let p = &g.players[&0];
        assert_eq!((p.armor, p.health, p.current), (42.0, 61.0, Some(0)));
        assert_eq!(p.weapons[0].parts, [3, -1, -1, -1]);
        assert_eq!(
            (p.weapons[0].clip, p.weapons[0].stock),
            (fresh.clip, fresh.stock)
        );
        let o = g.take_out();
        let sent = ids(&o, 0);
        let (rm, give) = (
            sent.iter().position(|x| *x == 0xb4).expect("0xb4"),
            sent.iter().position(|x| *x == 0xb2).expect("0xb2"),
        );
        assert!(rm < give, "{sent:x?}");
        // the old gun is gone: a change back is refused, not adopted
        g.tick_possession(1_000);
        g.on_up(
            0,
            Up::WeaponChange {
                slot: 0,
                weapon_id: 40,
                aux: 0,
            },
            1_010,
        );
        assert_eq!(held(&g, 0), victim);
        respawn(&mut g, 0, 2_000);
        assert_eq!(held(&g, 0), own);
    }

    /// The user's Possession report of 2026-11-04 ("my HP says 0 but I'm still alive"), headless: a grenade blast on
    /// the mover in the move sends him nothing and leaves his health; the same blast after the closing teleport hurts
    /// as ever, and a survivor's fraction below 1 (the log's 0.348) goes out as it is, alive, with no kill and no death
    /// state (the client's block shows 1 for it, CLI-1853).
    #[test]
    fn possession_a_blast_in_the_move_does_nothing_and_after_it_hurts_as_ever() {
        let mut g = possession_game();
        let nade = |g: &mut Game, at: [f32; 3], now: Millis| {
            g.flights.push(projectile::Flight {
                key: 1,
                owner: 2,
                owner_obj: g.players[&2].obj,
                owner_team: g.players[&2].team,
                weapon: 0,
                ammo: 0,
                origin: at,
                pos: at,
                vel: [0.0; 3],
                gravity: 0.0,
                detonate_at: now,
                last: now,
                resting: true,
                impact: false,
                burst: false,
                mine: None,
            });
            g.tick_projectiles(now);
        };
        g.kill(1, Some(0), 40, false, 100);
        g.tick_possession(200);
        g.take_out();
        let before = g.players[&0].health;
        let at = g.players[&0].pos;
        nade(&mut g, at, 200);
        let o = g.take_out();
        assert_eq!(g.players[&0].health, before);
        let r = received(&o, 0);
        assert!(r.damage.is_none() && r.health.is_none(), "{r:?}");
        g.tick_possession(400);
        assert!(!g.in_flight(0));
        let dmg = 100.0 * blast_factor(300.0, 400.0, BLAST_INNER_FRACTION);
        g.players.get_mut(&0).unwrap().health = dmg + 0.348;
        g.take_out();
        let at = g.players[&0].pos;
        nade(&mut g, [at[0] + 300.0, at[1], at[2]], 1_500);
        let o = g.take_out();
        let r = received(&o, 0);
        let h = r.health.expect("the health status");
        assert!((h - 0.348).abs() < 1e-3, "{r:?}");
        assert!(matches!(g.players[&0].life, Life::Alive));
        assert!(!ids(&o, 0).contains(&gs::GS_KILL) && !ids(&o, 0).contains(&gs::GS_PLAYER_STATE));
    }

    /// Sequential kills chain; a kill arriving during the move queues nothing; a knife kill counts.
    #[test]
    fn possession_chains_kills_and_a_kill_in_flight_queues_nothing() {
        let mut g = possession_game();
        let first = held(&g, 1);
        let third = held(&g, 2);
        g.kill(1, Some(0), 40, false, 100);
        // a grenade already flying lands: credit, no second move or swap
        g.kill(2, Some(0), 7, false, 150);
        assert_eq!(g.players[&0].flight.unwrap().to, [900.0, 0.0, 0.0]);
        assert_eq!(held(&g, 0), first);
        g.tick_possession(1_000);
        assert!(!g.in_flight(0));
        respawn(&mut g, 2, 1_100);
        g.players.get_mut(&2).unwrap().pos = [0.0, 0.0, 300.0];
        // a knife kill: the guns all the same
        g.kill(2, Some(0), 0, false, 1_200);
        assert_eq!(g.players[&0].flight.unwrap().to, [0.0, 0.0, 300.0]);
        assert_eq!(held(&g, 0), third);
    }

    /// A suicide, a world kill, a team kill and a dead killer move nobody.
    #[test]
    fn possession_ignores_suicide_world_team_and_dead_killers() {
        let mut g = possession_game();
        let own = held(&g, 0);
        g.kill(0, Some(0), 40, false, 100);
        g.kill(1, None, 0, false, 110);
        assert!(!g.in_flight(0) && !g.in_flight(1));
        // player 0 is dead: his round landing kills player 2
        g.kill(2, Some(0), 7, false, 120);
        assert!(!g.in_flight(0));
        assert_eq!(held(&g, 0), own);
        let mut g = possession_game_of("1", false);
        assert!(g.is_possession());
        let team = g.players[&0].team;
        g.players.get_mut(&1).unwrap().team = team;
        let own = held(&g, 0);
        g.kill(1, Some(0), 40, false, 100);
        assert!(!g.in_flight(0), "friendly fire does nothing");
        assert_eq!(held(&g, 0), own);
    }

    /// The flag does nothing in Fireteam, Spy Hunt, Quarantine or with Gun Game; without the flag nothing changes.
    #[test]
    fn possession_is_off_in_the_refused_modes_and_vanilla() {
        for class in ["5", "9", "11"] {
            assert!(
                !possession_game_of(class, false).is_possession(),
                "class {class}"
            );
        }
        let mut g = possession_game_of("6", true);
        assert!(g.is_gun_game() && !g.is_possession());
        g.kill(1, Some(0), 30, false, 100);
        assert!(!g.in_flight(0));
        let mut v = gun_game();
        v.cfg.gun_game = None;
        assert!(!v.is_possession());
        v.kill(1, Some(0), 30, false, 100);
        assert!(!v.in_flight(0));
    }

    // ------------------------------------------------------------------------------------- Hardcore (DSV-482)

    fn hardcore_game(scale: &str) -> Game {
        let mut tuning = GameTuning {
            move_check: false,
            spawn_shield: false,
            ..GameTuning::default()
        };
        tuning.set_combat("health_scale", scale).unwrap();
        let mut g = Game::new(tuning, &world(), tables());
        for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3)] {
            let e = entry(uid, team, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        g.take_out();
        g
    }

    /// Spawn values stay on the normal scale; the one damage point multiplies once, whatever the source: a hit, a
    /// blast (`apply_damage`, also grenades, the bomb, AI shots) and a fall (`apply_hit_as`) all arrive the same way.
    #[test]
    fn hardcore_scales_all_damage_once_and_shows_full_health() {
        let mut g = hardcore_game("0.2");
        assert_eq!(g.players[&1].health, 100.0, "shown full at the spawn");
        g.players.get_mut(&1).unwrap().armor = 0.0;
        let src = |dtype| combat::DamageSource { weapon: 3, dtype };
        g.apply_hit(1, Some(0), 4.0, 4.0, [1.0, 0.0, 0.0], src(3), 2, 100);
        assert_eq!(g.players[&1].health, 80.0, "a hit: 4 x 5");
        g.apply_damage(1, Some(0), 2.0, [1.0, 0.0, 0.0], src(3), 2, 110);
        assert_eq!(g.players[&1].health, 70.0, "a blast: 2 x 5");
        g.apply_hit_as(1, None, 1.0, 1.0, [0.0; 3], src(3), 2, 0, 120);
        assert_eq!(g.players[&1].health, 65.0, "a fall: 1 x 5");
        // what the victim is told is on the shown scale
        let o = g.take_out();
        assert!(o
            .iter()
            .any(|x| matches!(x, GameOut::To(1, m) if m.as_bytes()[1] == 0x66)));
        // lethal: kill feed, 0 health
        g.apply_hit(1, Some(0), 13.0, 13.0, [1.0, 0.0, 0.0], src(3), 2, 130);
        assert_eq!(g.players[&1].health, 0.0);
        assert!(matches!(g.players[&1].life, Life::Dying { .. }));
        assert_eq!(g.stats[&7].kills, 1, "the kill is credited");
    }

    /// The multiplied damage reaches the armour with the normal split; without the flag nothing changes.
    #[test]
    fn hardcore_reaches_the_armour_and_the_default_is_vanilla() {
        let mut g = hardcore_game("0.2");
        {
            let v = g.players.get_mut(&1).unwrap();
            v.armor = 100.0;
            v.armor_absorb = Absorb::Flat(0.5);
        }
        let src = combat::DamageSource {
            weapon: 3,
            dtype: 3,
        };
        g.apply_hit(1, Some(0), 10.0, 10.0, [1.0, 0.0, 0.0], src, 2, 100);
        let v = &g.players[&1];
        assert_eq!((v.health, v.armor), (75.0, 75.0), "50: 25 + 25");
        let mut g = hardcore_game("1");
        assert_eq!(g.cfg.health_scale, 1.0);
        g.players.get_mut(&1).unwrap().armor = 0.0;
        g.apply_hit(1, Some(0), 4.0, 4.0, [1.0, 0.0, 0.0], src, 2, 100);
        assert_eq!(g.players[&1].health, 96.0);
        // a value outside (0, 1] is refused
        let mut t = GameTuning::default();
        for bad in ["0", "-1", "1.5", "x"] {
            assert!(t.set_combat("health_scale", bad).is_err(), "{bad}");
        }
    }

    /// Gun Game on top: the same multiplier, the ladder's rules unchanged.
    #[test]
    fn hardcore_combines_with_gun_game() {
        let mut g = gun_game();
        g.cfg.health_scale = 0.2;
        g.players.get_mut(&1).unwrap().armor = 0.0;
        let src = combat::DamageSource {
            weapon: 30,
            dtype: 3,
        };
        g.apply_hit(1, Some(0), 20.0, 20.0, [1.0, 0.0, 0.0], src, 2, 100);
        assert_eq!(g.players[&1].health, 0.0, "one shot");
        assert_eq!(g.ffa_kills_of(0), 1, "the rung moved");
        assert_eq!(held(&g, 0), vec![(1, 164), (2, 0)]);
    }

    /// The one-shot table from the user's ROM: every shop-type gun against the default body's armour, per hit location
    /// (a shotgun: one pellet). Prints the table with --nocapture; the assert is the rule's goal.
    #[test]
    fn rom_hardcore_one_shot_table() {
        let Some(dir) = std::env::var_os("CA_ROM").map(std::path::PathBuf::from) else {
            eprintln!("skipped: CA_ROM unset");
            return;
        };
        let (t, _) = RomTables::load(&dir);
        let body = t
            .chars
            .bodies
            .values()
            .find(|b| b.gear.is_some())
            .expect("a body row");
        let gear_row = body.gear.unwrap();
        let gear = t.weapons.gear[&u16::try_from(gear_row).unwrap()].clone();
        let armor = gear.armor.unwrap_or(0.0);
        let nodes = t
            .chars
            .skeletons
            .get(&body.skeleton.unwrap_or(0))
            .cloned()
            .unwrap_or_default();
        // the factor of each location 0 (body), 1 (head), 3, 4 (limbs): the first node of it
        let factor = |loc: u8| {
            nodes
                .iter()
                .find(|n| n.location == loc)
                .map_or(1.0, |n| n.factor)
        };
        eprintln!(
            "TABLE armour {armor} gear {gear_row} factors body {} head {} l3 {} l4 {}",
            factor(0),
            factor(1),
            factor(3),
            factor(4)
        );
        let cost_from_base = GameTuning::default().armor_cost_from_base;
        let mut rows = 0;
        let (mut one_armoured, mut one_bare) = (0, 0);
        let mut ids: Vec<(u16, i32)> = t
            .items
            .weapon_types
            .iter()
            .map(|(w, ty)| (*w, *ty))
            .collect();
        ids.sort();
        for (w, ty) in ids {
            if !dc_bute::arms::is_ladder_gun(ty) {
                continue;
            }
            let Some(ammo) = t.weapons.ammo_of(w) else {
                continue;
            };
            let Some(base) = t.weapons.damage(w, ammo).filter(|d| *d > 0.0) else {
                continue;
            };
            let dtype = t.weapons.damage_type(w, ammo).unwrap_or(3);
            let hit = |loc: u8, scale: f32| {
                let f = factor(loc);
                combat::armour_split(
                    base * f / scale,
                    base / scale,
                    gear.share(dtype, if loc == 0 { 2 } else { loc }),
                    armor,
                    cost_from_base,
                )
                .0
            };
            let body_hit = hit(0, 0.2);
            rows += 1;
            one_armoured += (body_hit >= 100.0) as u32;
            one_bare += (base * factor(0) / 0.2 >= 100.0) as u32;
            eprintln!(
                "TABLE w{w} type {ty} dmg {base}: vanilla body {:.0}; x5 body {:.0} head {:.0} limb {:.0} no-armour body {:.0}",
                hit(0, 1.0),
                body_hit,
                hit(1, 0.2),
                hit(4, 0.2),
                base * factor(0) / 0.2
            );
        }
        eprintln!(
            "TABLE {one_armoured} of {rows} one-shot in armour (body), {one_bare} without armour"
        );
        assert!(rows >= 30, "{rows}");
        assert!(one_armoured * 4 >= rows * 3, "most guns kill in one shot");
        let dearest = t
            .weapons
            .gear
            .values()
            .flat_map(|g| g.slots.iter().flat_map(|s| s.1))
            .fold(0.0f32, f32::max);
        eprintln!("TABLE the largest single armour share in the ROM {dearest}");
    }

    // ---------------------------------------------------------------------------------------- Gun Game (DSV-481)

    /// A Gun Game match of two players: ladder rifle 30, pistol 164, rifle 31; knife 0 from the item table.
    fn gun_game() -> Game {
        let mut t = tables();
        t.items.weapon_types = [(30, 0), (164, 4), (31, 2), (0, 7), (40, 0)].into();
        let mut g = Game::new(
            GameTuning {
                move_check: false,
                gun_game: Some(gungame::GunGameSetup {
                    seed: 5,
                    ladder: vec![30, 164, 31],
                }),
                ..GameTuning::default()
            },
            &world(),
            t,
        );
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "6".into());
        room.pairs.insert("goal".into(), "50".into());
        room.pairs.insert("arms".into(), "6".into());
        g.set_room(&room);
        for (pid, uid, obj) in [(0u8, 7u32, 1u16), (1, 8, 3)] {
            let mut e = entry(uid, 0, "P");
            // a shop loadout that must not count: another rifle, with attachments
            e.extra = vec![("loadout".into(), "0:40,2:0".into())];
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        g.players.get_mut(&1).unwrap().pos = [300.0, 0.0, 0.0];
        g.spawn_notes.clear();
        g.take_out();
        g
    }

    fn held(g: &Game, pid: u8) -> Vec<(u8, u16)> {
        g.players[&pid]
            .weapons
            .iter()
            .map(|w| (w.slot, w.weapon))
            .collect()
    }

    fn respawn(g: &mut Game, pid: u8, now: Millis) {
        g.spawn(pid, now);
        g.take_out();
    }

    /// The goal is the ladder's length, the room's weapon limit is gone, and everybody starts with rung 0's gun and
    /// the knife whatever the shop loadout says.
    #[test]
    fn gun_game_starts_at_the_first_rung_with_the_knife() {
        let g = gun_game();
        assert!(g.is_gun_game());
        assert_eq!((g.rules().goal, g.rules().arms), (3, 0));
        assert_eq!(held(&g, 0), vec![(0, 30), (2, 0)]);
        assert_eq!(held(&g, 1), vec![(0, 30), (2, 0)]);
        assert_eq!(g.players[&0].current, Some(0));
    }

    /// A kill with the rung's gun: rung + 1, the next gun in the same life (0xb4, 0xb2, then the draw), the rung in the
    /// 0x6b counters; the secondary lands in slot 1. The last gun's kill ends the match with that player's team value.
    #[test]
    fn gun_game_promotes_in_the_same_life_and_the_last_gun_wins() {
        let mut g = gun_game();
        g.kill(1, Some(0), 30, false, 100);
        assert_eq!(g.ffa_kills_of(0), 1);
        assert_eq!(held(&g, 0), vec![(1, 164), (2, 0)]);
        assert!(matches!(g.players[&0].life, Life::Alive), "the same life");
        let o = g.take_out();
        let sent = ids(&o, 0);
        let (rm, give) = (
            sent.iter().position(|x| *x == 0xb4).expect("0xb4"),
            sent.iter().position(|x| *x == 0xb2).expect("0xb2"),
        );
        assert!(rm < give, "{sent:x?}");
        // the stats keep the real kill, the board shows the rung
        assert_eq!(g.stats[&7].kills, 1);
        // the old gun is a stale kill: nothing moves
        respawn(&mut g, 1, 200);
        g.kill(1, Some(0), 30, false, 300);
        assert_eq!(g.ffa_kills_of(0), 1);
        respawn(&mut g, 1, 400);
        g.kill(1, Some(0), 164, false, 500);
        assert_eq!(g.ffa_kills_of(0), 2);
        assert_eq!(held(&g, 0), vec![(0, 31), (2, 0)]);
        assert!(matches!(g.phase, RoundPhase::Running { .. }));
        respawn(&mut g, 1, 600);
        g.kill(1, Some(0), 31, false, 700);
        assert_eq!(g.ffa_kills_of(0), 3);
        assert!(!matches!(g.phase, RoundPhase::Running { .. }), "the win");
    }

    /// A knife kill: the victim one rung down (floor 0) with that gun at his respawn; the knifer stays and keeps his
    /// gun. A suicide or a fall moves nobody.
    #[test]
    fn gun_game_knife_demotes_with_a_floor_and_a_suicide_changes_nothing() {
        let mut g = gun_game();
        g.kill(1, Some(0), 30, false, 100);
        respawn(&mut g, 1, 150);
        assert_eq!(g.ffa_kills_of(0), 1);
        // 1 knifes 0: 0 back to rung 0, 1 stays on rung 0
        g.kill(0, Some(1), 0, false, 200);
        assert_eq!((g.ffa_kills_of(0), g.ffa_kills_of(1)), (0, 0));
        assert_eq!(
            held(&g, 1),
            vec![(0, 30), (2, 0)],
            "the knifer keeps his gun"
        );
        respawn(&mut g, 0, 300);
        assert_eq!(
            held(&g, 0),
            vec![(0, 30), (2, 0)],
            "the lower gun at the respawn"
        );
        // floor: knifed on rung 0 stays there
        g.kill(0, Some(1), 0, false, 400);
        assert_eq!(g.ffa_kills_of(0), 0);
        respawn(&mut g, 0, 500);
        // a respawn keeps the rung's gun; own death and a fall change nothing
        g.kill(1, Some(0), 30, false, 600);
        respawn(&mut g, 1, 650);
        assert_eq!(g.ffa_kills_of(0), 1);
        g.kill(0, Some(0), 164, false, 700);
        assert_eq!(g.ffa_kills_of(0), 1, "suicide");
        respawn(&mut g, 0, 800);
        g.kill(0, None, 0, false, 900);
        assert_eq!(g.ffa_kills_of(0), 1, "fall");
        respawn(&mut g, 0, 1000);
        assert_eq!(held(&g, 0), vec![(1, 164), (2, 0)]);
    }

    /// A shot or a weapon change naming a gun the server did not give is refused: no adoption, no record, no damage.
    #[test]
    fn gun_game_refuses_any_other_weapon() {
        let mut g = gun_game();
        g.on_up(
            0,
            Up::WeaponChange {
                slot: 0,
                weapon_id: 40,
                aux: 0,
            },
            100,
        );
        assert_eq!(held(&g, 0), vec![(0, 30), (2, 0)], "not adopted");
        let mut f = hit_report(&g, 0, 1, 0.0, None);
        f.weapon_id = 40;
        g.on_up(0, Up::Fire(Box::new(f.clone())), 200);
        assert_eq!(g.players[&1].health, 100.0);
        let o = g.take_out();
        assert!(effects(&o).is_empty());
        assert!(o
            .iter()
            .any(|x| matches!(x, GameOut::Log(l) if l.contains("refused (gun game)"))));
        f.weapon_id = 30;
        g.on_up(0, Up::Fire(Box::new(f)), 300);
        assert!(g.players[&1].health < 100.0, "the rung's gun hurts");
    }

    /// A late joiner starts on rung 0, however far the others are; a client id reused starts over.
    #[test]
    fn gun_game_late_join_starts_at_rung_zero() {
        let mut g = gun_game();
        g.kill(1, Some(0), 30, false, 100);
        let e = entry(9, 0, "Late");
        g.note_identified(&e);
        g.add_player(2, Some(&e), None, 5, [0.0; 3], [0.0; 3], 200);
        g.on_up(2, Up::PlayerInfo(Box::default()), 210);
        g.on_up(2, Up::WorldReady, 220);
        assert_eq!(g.ffa_kills_of(2), 0);
        assert_eq!(held(&g, 2), vec![(0, 30), (2, 0)]);
    }

    /// A gun that ran dry gets its reserve back; a flag on another mode class does nothing.
    #[test]
    fn gun_game_refills_a_dry_gun_and_needs_free_for_all() {
        let mut g = gun_game();
        {
            let w = &mut g.players.get_mut(&0).unwrap().weapons[0];
            w.clip_size = 30;
            w.clip = 1;
            w.stock = 0;
        }
        let mut f = hit_report(&g, 0, 1, 0.0, None);
        f.weapon_id = 30;
        f.last = true;
        g.on_up(0, Up::Fire(Box::new(f)), 100);
        let w = &g.players[&0].weapons[0];
        assert_eq!(w.clip, 0);
        assert!(w.stock >= 30, "refilled: {}", w.stock);

        let mut t = tables();
        t.items.weapon_types = [(30, 0)].into();
        let mut g = Game::new(
            GameTuning {
                gun_game: Some(gungame::GunGameSetup {
                    seed: 1,
                    ladder: vec![30],
                }),
                ..GameTuning::default()
            },
            &world(),
            t,
        );
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "7".into());
        room.pairs.insert("goal".into(), "50".into());
        g.set_room(&room);
        assert!(!g.is_gun_game());
        assert_eq!(g.rules().goal, 50, "vanilla rules");
    }

    /// DSV-180 / DSV-476: under the room's weapon limit (`arms=6`, Sniper Only) the lobby's rifle, pistol and grenade
    /// are not granted; the sniper rifle and the knife are. Without a limit everything is.
    #[test]
    fn a_sniper_only_room_grants_no_rifle() {
        let mut t = tables();
        // synthetic weapon types: 145 rifle (0), 164 pistol (4), 0 knife (7), 7 grenade (6), 30 sniper rifle (2)
        t.items.weapon_types = [(145, 0), (164, 4), (0, 7), (7, 6), (30, 2)].into();
        let mut g = Game::new(spec_order(), &world(), t);
        let mut e = entry(7, 0, "Dave");
        e.extra = vec![("loadout".into(), "0:145,1:164,2:0,3:7,10:30".into())];
        assert_eq!(
            g.loadout_weapons(Some(&e)),
            vec![(0, 145), (1, 164), (2, 0), (3, 7), (4, 30)]
        );
        let mut room = RoomSettings::default();
        room.pairs.insert("arms".into(), "6".into());
        g.set_room(&room);
        assert_eq!(g.rules().arms, 6);
        assert_eq!(g.loadout_weapons(Some(&e)), vec![(2, 0), (4, 30)]);
    }

    /// DSV-568: the spawn set draws the slot-0 weapon; with slot 0 empty a primary in a backpack slot beats the pistol
    /// and the knife (the 0x7f names it, the server's drawn weapon is it), at the first spawn and at every respawn.
    /// Sniper Only with the sniper rifle in the backpack draws the sniper rifle, not the knife. A kit with slot 0 is
    /// drawn as before; without a command (no weapon-order row) the 0xb3 carries the drawn weapon and no 0xb2 gives
    /// it twice.
    #[test]
    fn a_spawn_draws_the_primary_also_from_the_backpack() {
        let mut t = tables();
        // synthetic weapon types: 145 rifle (0), 164 pistol (4), 0 knife (7), 7 grenade (6), 30 sniper rifle (2)
        t.items.weapon_types = [(145, 0), (164, 4), (0, 7), (7, 6), (30, 2), (31, 0)].into();
        let doc = dc_bute::ButeDoc::parse(concat!(
            "[Weapon0]\nName = \"K\"\n[Weapon7]\nName = \"N\"\n[Weapon30]\nName = \"S\"\n",
            "[Weapon145]\nName = \"R\"\n[Weapon164]\nName = \"P\"\n[Weapon31]\nName = \"X\"\n",
            "[WeaponOrder]\nClass0 = \"A\"\nA0 = \"R\"\nA1 = \"S\"\nClass1 = \"B\"\nB0 = \"P\"\n",
            "Class2 = \"C\"\nC0 = \"K\"\nClass3 = \"D\"\nD0 = \"N\"\n",
        ));
        t.weapons = crate::rom_tables::WeaponTable::from_doc(&doc);
        let command = |w: u16| t.weapons.command_of(w).expect("in the order");
        let (rifle_cmd, sniper_cmd) = (command(145), command(30));
        let switched = |o: &[GameOut]| -> Vec<u8> {
            o.iter()
                .filter_map(|x| match x {
                    GameOut::To(0, m) if m.as_bytes()[1] == 0x7f => Some(m.as_bytes()[2]),
                    _ => None,
                })
                .collect()
        };
        let drawn = |g: &Game| {
            let p = &g.players[&0];
            p.current.map(|i| (p.weapons[i].slot, p.weapons[i].weapon))
        };
        let start = |t: RomTables, loadout: &str, arms: Option<&str>| {
            let mut g = Game::new(GameTuning::default(), &world(), t);
            if let Some(a) = arms {
                let mut room = RoomSettings::default();
                room.pairs.insert("arms".into(), a.into());
                g.set_room(&room);
            }
            let mut e = entry(7, 0, "Dave");
            e.extra = vec![("loadout".into(), loadout.into())];
            g.note_identified(&e);
            g.add_player(0, Some(&e), None, 1, [0.0; 3], [0.0; 3], 0);
            g.take_out();
            g.on_up(0, Up::PlayerInfo(Box::default()), 10);
            let o = g.take_out();
            g.on_up(0, Up::WorldReady, 20);
            g.take_out();
            (g, o)
        };
        // slot 0 empty (the default primary sat in the backpack, LG-403): the backpack rifle, not the pistol
        let (mut g, o) = start(t.clone(), "1:164,2:0,3:7,10:145", None);
        assert_eq!(drawn(&g), Some((4, 145)));
        assert_eq!(switched(&o), vec![rifle_cmd]);
        g.spawn(0, 5_000);
        let o = g.take_out();
        assert_eq!(drawn(&g), Some((4, 145)), "at the respawn too");
        assert_eq!(switched(&o), vec![rifle_cmd]);
        assert!(o.iter().any(|x| matches!(x,
            GameOut::Log(l) if l.contains("spawned") && l.contains("drawn slot 4 weapon 145"))));
        // no primary anywhere: the pistol before the knife (also when the pistol sits in the backpack)
        let (g, _) = start(t.clone(), "2:0,3:7,10:164", None);
        assert_eq!(drawn(&g), Some((4, 164)));
        // Sniper Only, the sniper rifle in the backpack: the sniper rifle, not the knife
        let (g, o) = start(t.clone(), "0:145,1:164,2:0,3:7,10:30", Some("6"));
        assert_eq!(drawn(&g), Some((4, 30)));
        assert_eq!(switched(&o), vec![sniper_cmd]);
        // a slot-0 weapon is drawn as ever, also beside a backpack rifle
        let (g, o) = start(t.clone(), "0:145,1:164,2:0,10:30", None);
        assert_eq!(drawn(&g), Some((0, 145)));
        assert_eq!(switched(&o), vec![rifle_cmd]);
        // weapon 31 has no order row (no command): its 0xb3 draws it and it is not also given by 0xb2
        let (g, o) = start(t.clone(), "1:164,2:0,11:31", None);
        assert_eq!(drawn(&g), Some((5, 31)));
        assert!(switched(&o).is_empty());
        let weapon_of = |m: &BitWriter| u16::from_le_bytes([m.as_bytes()[2], m.as_bytes()[3]]);
        let shells = |id: u8| -> Vec<u16> {
            o.iter()
                .filter_map(|x| match x {
                    GameOut::To(0, m) if m.as_bytes()[1] == id => Some(weapon_of(m)),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(shells(0xb3), vec![31]);
        assert_eq!(shells(0xb2), vec![164, 0]);
        // a list out of slot order (a Possession victim's, a taken drop appended): slot 0 still wins
        let kit = |list: &[(u8, u16)]| -> Vec<WeaponState> {
            list.iter()
                .map(|&(slot, w)| WeaponState {
                    slot,
                    ..g.new_weapon(w, None)
                })
                .collect()
        };
        assert_eq!(g.draw_index(&kit(&[(1, 164), (2, 0), (0, 145)])), Some(2));
        assert_eq!(g.draw_index(&kit(&[(2, 0), (3, 7), (1, 164)])), Some(2));
        assert_eq!(g.draw_index(&[]), None);
    }

    /// DSV-476: in a Sniper Only match a player spawns with the sniper rifle and the knife; a client that names the
    /// pistol anyway (a weapon change, a shot that hits) gets nothing: the change is not taken (no adopted weapon, the
    /// drawn weapon stays), the shot has no record and no damage and counts as `weapon_limit`. The sniper rifle's
    /// same shot hurts.
    #[test]
    fn a_pistol_does_nothing_in_a_sniper_only_match() {
        let mut t = tables();
        // synthetic weapon types: 164 pistol (4), 0 knife (7), 30 sniper rifle (2)
        t.items.weapon_types = [(164, 4), (0, 7), (30, 2)].into();
        let mut g = Game::new(
            GameTuning {
                move_check: false,
                ..GameTuning::default()
            },
            &world(),
            t,
        );
        let mut room = RoomSettings::default();
        room.pairs.insert("arms".into(), "6".into());
        g.set_room(&room);
        for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3)] {
            let mut e = entry(uid, team, "P");
            e.extra = vec![("loadout".into(), "0:30,1:164,2:0".into())];
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        g.players.get_mut(&1).unwrap().pos = [300.0, 0.0, 0.0];
        g.spawn_notes.clear();
        g.take_out();
        let carried =
            |g: &Game| -> Vec<u16> { g.players[&0].weapons.iter().map(|w| w.weapon).collect() };
        assert!(!carried(&g).contains(&164), "no pistol in the spawn set");
        let drawn = g.players[&0].current;
        g.on_up(
            0,
            Up::WeaponChange {
                slot: 1,
                weapon_id: 164,
                aux: 0,
            },
            100,
        );
        assert!(!carried(&g).contains(&164), "the pistol is not adopted");
        assert_eq!(g.players[&0].current, drawn, "the drawn weapon stays");
        let mut f = hit_report(&g, 0, 1, 0.0, None);
        f.weapon_id = 164;
        g.on_up(0, Up::Fire(Box::new(f.clone())), 200);
        assert_eq!(
            g.players[&1].health, 100.0,
            "the pistol's hit does no damage"
        );
        let o = g.take_out();
        assert!(effects(&o).is_empty(), "no shot record of the pistol");
        assert!(o
            .iter()
            .any(|x| matches!(x, GameOut::Log(l) if l.contains("refused (room weapon limit 6)"))));
        assert_eq!(g.hit_counts.get(hits::HitVerdict::WeaponLimit), 1);
        f.weapon_id = 30;
        g.on_up(0, Up::Fire(Box::new(f)), 300);
        assert!(g.players[&1].health < 100.0, "the sniper rifle's hit hurts");
    }

    /// Synthetic types for the weapon-rule tests: 145 rifle (0), 164 pistol (4), 0 knife (7), 7 grenade (6), 30 sniper
    /// rifle (2), 40 shotgun (0x1a); camo variants 500 of the rifle and 501 of the sniper rifle (no item rows of their
    /// own, VAR-1); 900 a pack gun without an item row.
    fn rule_tables() -> RomTables {
        let mut t = tables();
        t.items.weapon_types = [(145, 0), (164, 4), (0, 7), (7, 6), (30, 2), (40, 0x1a)].into();
        t.weapons.variants = [(500, 145), (501, 30)].into();
        t
    }

    /// DSV-610: every rule of the room's weapon list on one full loadout (a rifle variant in the primary slot, the
    /// pistol, knife, grenade, and in the backpack the sniper rifle, the shotgun, a sniper rifle variant and a pack
    /// gun): each rule grants only its kinds, a variant counts as its base gun.
    #[test]
    fn every_weapon_rule_grants_its_kinds_and_a_variant_as_its_base_gun() {
        let mut g = Game::new(spec_order(), &world(), rule_tables());
        let mut e = entry(7, 0, "Dave");
        e.extra = vec![(
            "loadout".into(),
            "0:500,1:164,2:0,3:7,10:30,11:40,12:501,13:900".into(),
        )];
        let all = vec![
            (0, 500),
            (1, 164),
            (2, 0),
            (3, 7),
            (4, 30),
            (5, 40),
            (6, 501),
            (7, 900),
        ];
        let rows: [(u32, Vec<(u8, u16)>); 9] = [
            (0, all.clone()),
            (1, vec![(1, 164), (2, 0)]),
            (2, vec![(2, 0)]),
            (
                3,
                vec![(0, 500), (1, 164), (2, 0), (3, 7), (5, 40), (7, 900)],
            ),
            (
                4,
                vec![
                    (0, 500),
                    (1, 164),
                    (2, 0),
                    (4, 30),
                    (5, 40),
                    (6, 501),
                    (7, 900),
                ],
            ),
            (5, vec![(0, 500), (1, 164), (2, 0), (3, 7)]),
            (6, vec![(2, 0), (4, 30), (6, 501)]),
            (7, vec![(2, 0), (5, 40)]),
            (
                8,
                vec![
                    (0, 500),
                    (1, 164),
                    (2, 0),
                    (3, 7),
                    (4, 30),
                    (6, 501),
                    (7, 900),
                ],
            ),
        ];
        for (arms, want) in rows {
            let mut room = RoomSettings::default();
            room.pairs.insert("arms".into(), arms.to_string());
            g.set_room(&room);
            assert_eq!(g.rules().arms, arms);
            assert_eq!(g.loadout_weapons(Some(&e)), want, "weapon rule {arms}");
        }
        // without the item table nothing is known, so nothing is held back (as before)
        let mut g = Game::new(spec_order(), &world(), tables());
        let mut room = RoomSettings::default();
        room.pairs.insert("arms".into(), "1".into());
        g.set_room(&room);
        assert_eq!(g.loadout_weapons(Some(&e)), all);
    }

    /// DSV-610: a client that names a weapon the rule kept out of its spawn set (No Backpack: its backpack sniper
    /// rifle; Pistols Only: its camo rifle) gets nothing: the change is not taken, the shot does no damage and counts
    /// as `weapon_limit`.
    #[test]
    fn a_weapon_the_rule_kept_out_cannot_be_drawn_or_fired() {
        for (arms, loadout, barred, slot) in [
            ("5", "0:145,1:164,2:0,10:30", 30u16, 4u8),
            ("1", "0:500,1:164,2:0", 500, 0),
            ("6", "0:500,2:0,10:501", 500, 0),
        ] {
            let mut g = Game::new(
                GameTuning {
                    move_check: false,
                    ..GameTuning::default()
                },
                &world(),
                rule_tables(),
            );
            let mut room = RoomSettings::default();
            room.pairs.insert("arms".into(), arms.into());
            g.set_room(&room);
            for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3)] {
                let mut e = entry(uid, team, "P");
                e.extra = vec![("loadout".into(), loadout.into())];
                g.note_identified(&e);
                g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
                g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
                g.on_up(pid, Up::WorldReady, 20);
            }
            g.players.get_mut(&1).unwrap().pos = [300.0, 0.0, 0.0];
            g.take_out();
            let carried =
                |g: &Game| -> Vec<u16> { g.players[&0].weapons.iter().map(|w| w.weapon).collect() };
            assert!(
                !carried(&g).contains(&barred),
                "rule {arms}: {:?}",
                carried(&g)
            );
            let drawn = g.players[&0].current;
            g.on_up(
                0,
                Up::WeaponChange {
                    slot,
                    weapon_id: barred,
                    aux: 0,
                },
                100,
            );
            assert!(!carried(&g).contains(&barred), "rule {arms}: not adopted");
            assert_eq!(
                g.players[&0].current, drawn,
                "rule {arms}: the drawn weapon stays"
            );
            let mut f = hit_report(&g, 0, 1, 0.0, None);
            f.weapon_id = barred;
            g.on_up(0, Up::Fire(Box::new(f)), 200);
            assert_eq!(g.players[&1].health, 100.0, "rule {arms}: no damage");
            let o = g.take_out();
            let refused = format!("weapon {barred} refused (room weapon limit {arms})");
            assert!(
                o.iter()
                    .any(|x| matches!(x, GameOut::Log(l) if l.contains(&refused))),
                "rule {arms}"
            );
            assert_eq!(
                g.hit_counts.get(hits::HitVerdict::WeaponLimit),
                1,
                "rule {arms}"
            );
        }
    }

    /// DSV-38: weapons by slot from the lobby's `loadout=slot:item,...` (ROM item numbers; gear items are not
    /// weapons), the slice-2 bare form in slot order, else the operator's `-loadout`, else the ROM default weapons;
    /// the spawn grants each weapon in its slot and draws the lowest slot.
    #[test]
    fn loadout_tokens_operator_default_and_rom_issue_weapons() {
        let mut t = tables();
        t.items.issue_weapons = [Some(12), Some(20), Some(30), None];
        let g = Game::new(
            GameTuning {
                default_loadout: vec![3, 4],
                ..spec_order()
            },
            &world(),
            t.clone(),
        );
        let with = |tok: &str| {
            let mut e = entry(7, 0, "Dave");
            e.extra = vec![("loadout".into(), tok.into())];
            e
        };
        assert_eq!(
            g.loadout_weapons(Some(&with("1:164,0:145,2:0,3:7,8:3005,9:7001"))),
            vec![(0, 145), (1, 164), (2, 0), (3, 7)]
        );
        // the backpack slots 10..13 give weapon slots 4..7 (rust3: the lobby stored a bought shotgun in slot 10);
        // other equipment slots never hold weapons here
        assert_eq!(
            g.loadout_weapons(Some(&with(
                "0:145,1:164,2:0,3:143,9:7001,10:125,11:7,14:126"
            ))),
            vec![(0, 145), (1, 164), (2, 0), (3, 143), (4, 125), (5, 7)]
        );
        assert_eq!(
            (0..16u8)
                .filter_map(weapon_slot_of_equip)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4, 5, 6, 7]
        );
        assert_eq!(
            g.loadout_weapons(Some(&with("4,0,7"))),
            vec![(0, 4), (1, 0), (2, 7)]
        );
        assert_eq!(
            g.loadout_weapons(Some(&with("0:5,0:6,2:9"))),
            vec![(0, 5), (2, 9)]
        );
        // no weapon in the token, no token: the operator's list
        assert_eq!(
            g.loadout_weapons(Some(&with("8:3005"))),
            vec![(0, 3), (1, 4)]
        );
        assert_eq!(g.loadout_weapons(None), vec![(0, 3), (1, 4)]);
        // without an operator list: the ROM default weapon of every slot
        let g = Game::new(spec_order(), &world(), t);
        assert_eq!(g.loadout_weapons(None), vec![(0, 12), (1, 20), (2, 30)]);
        // the spawn grants by slot: 0xb2 for slots 1, 2 (slot byte = the slot), 0xb3 for the drawn slot 0
        let mut g = g;
        let e = with("2:7,0:3,1:4");
        g.add_player(0, Some(&e), None, 1, [0.0; 3], [0.0; 3], 0);
        g.on_up(0, Up::PlayerInfo(Box::default()), 10);
        g.take_out();
        g.on_up(0, Up::WorldReady, 20);
        let grants: Vec<(u8, u16, u8)> = g
            .take_out()
            .iter()
            .filter_map(|x| match x {
                GameOut::To(0, m) if matches!(m.as_bytes()[1], 0xb2 | 0xb3) => {
                    let b = m.as_bytes();
                    let w = u16::from_le_bytes([b[2], b[3]]);
                    let slot = if b[1] == 0xb2 { b[11] } else { b[15] };
                    Some((b[1], w, slot))
                }
                _ => None,
            })
            .collect();
        assert_eq!(grants, vec![(0xb2, 4, 1), (0xb2, 7, 2), (0xb3, 3, 0)]);
        // class_pos: the weapon's position among the granted weapons of its class (classes of the command table)
        let doc = dc_bute::ButeDoc::parse(concat!(
            "[Weapon3]
Name = \"A\"
[Weapon4]
Name = \"B\"
[Weapon5]
Name = \"C\"
",
            "[Weapon6]
Name = \"D\"
",
            "[WeaponOrder]
Class0 = \"P\"
Class1 = \"S\"
P0 = \"A\"
P1 = \"C\"
S0 = \"B\"
",
        ));
        let mut t2 = tables();
        t2.weapons = crate::rom_tables::WeaponTable::from_doc(&doc);
        assert_eq!(
            (3..=6).map(|w| t2.weapons.class_of(w)).collect::<Vec<_>>(),
            vec![Some(0), Some(1), Some(0), None]
        );
        let mut g2 = Game::new(spec_order(), &world(), t2);
        let e = with("0:3,1:4,2:6,10:5");
        g2.add_player(0, Some(&e), None, 1, [0.0; 3], [0.0; 3], 0);
        g2.on_up(0, Up::PlayerInfo(Box::default()), 10);
        g2.take_out();
        g2.on_up(0, Up::WorldReady, 20);
        let gives: Vec<(u16, u8, u8)> = g2
            .take_out()
            .iter()
            .filter_map(|x| match x {
                GameOut::To(0, m) if m.as_bytes()[1] == 0xb2 => {
                    let b = m.as_bytes();
                    Some((u16::from_le_bytes([b[2], b[3]]), b[10], b[11]))
                }
                _ => None,
            })
            .collect();
        // weapon, class_pos, slot: the backpack weapon of the first class is its second weapon (class_pos 1, slot 4)
        assert_eq!(gives, vec![(4, 0, 1), (6, 0, 2), (5, 1, 4)]);
        assert_eq!(
            g.players[&0]
                .weapons
                .iter()
                .map(|w| w.slot)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn identify_deadline_introduces_anyway() {
        let mut g = Game::new(spec_order(), &world(), tables());
        g.add_player(0, None, None, 1, [0.0; 3], [0.0; 3], 0);
        g.take_out();
        g.tick(2_999);
        assert!(ids(&g.take_out(), 0).is_empty());
        g.tick(3_000);
        assert_eq!(ids(&g.take_out(), 0), vec![0x6b, 0x7d, 0x67]);
    }

    // ------------------------------------------------------------------------------------------- fall damage (DSV-76)

    /// One accepted trailer of player 0 at height `y` (`move_check` off: the synthetic world box is small).
    fn fall_step(g: &mut Game, y: f32, ground: bool, surface: u8, now: Millis) -> Vec<GameOut> {
        let seq = g.players[&0].move_seq;
        let u = Shell64Update {
            state: Some(0),
            trailer: Some(crate::shell64::Trailer {
                a: seq,
                vec_b: [-300.0, y, 0.0],
                vec_c: [0.0; 3],
                t: now as f32 / 1000.0,
                d: ground as u8,
                e: surface,
                f: 0,
            }),
            complete: true,
            ..Shell64Update::default()
        };
        g.on_update(0, &u, now);
        g.take_out()
    }

    /// A run of trailers 50 ms apart; the output of every step.
    fn fall_run(g: &mut Game, steps: &[(f32, bool, u8)], start: Millis) -> Vec<GameOut> {
        let mut out = Vec::new();
        for (i, &(y, ground, s)) in steps.iter().enumerate() {
            out.extend(fall_step(g, y, ground, s, start + 50 * i as u32));
        }
        out
    }

    fn fall_game() -> Game {
        let mut g = duel(GameTuning {
            move_check: false,
            ..GameTuning::default()
        });
        assert!(matches!(g.phase, RoundPhase::Running { .. }));
        // the synthetic tables give no armour; give the victim the captured 100 so an armour status would show
        g.players.get_mut(&0).unwrap().armor = 100.0;
        g
    }

    /// The `GS_DAMAGE_TAKEN` messages to player 0.
    fn damage_msgs(o: &[GameOut]) -> Vec<Vec<u8>> {
        o.iter()
            .filter_map(|x| match x {
                GameOut::To(0, m) if m.as_bytes()[1] == gs::GS_DAMAGE_TAKEN => {
                    Some(m.as_bytes().to_vec())
                }
                _ => None,
            })
            .collect()
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Our 0x74 against a captured one: `[0d 74][x][y][z][unused][mode][ammo_kind][dmg_kind][zone][part][damage]`
    /// bit for bit except `from_dir` y (within 1e-4).
    fn same_fall_message(ours: &[u8], captured: &str) {
        let c = hex(captured);
        assert_eq!(ours.len(), c.len(), "{ours:02x?}");
        assert_eq!(ours[..6], c[..6], "{ours:02x?}");
        assert_eq!(ours[10..], c[10..], "{ours:02x?}");
        let y = |b: &[u8]| f32::from_le_bytes([b[6], b[7], b[8], b[9]]);
        assert!(
            (y(ours) - y(&c)).abs() < 1e-4,
            "from_dir y {} vs {}",
            y(ours),
            y(&c)
        );
    }

    /// The three falls of the reference-stack captures replayed as trailer runs (tops and landings of the real client's
    /// trailers): our 0x74 equals the reference stack's bit for bit except `from_dir` y (within 1e-4), the health status within
    /// 1e-3, no armour status. tdm1 c5 seq 6660 (jump off a ledge) and seq 7933 (walk off, from 99.06134), tdm4 c6 seq
    /// 6695 (jump).
    // OBSERVED: capture 20261025_000014_tdm1 c5 seq 6660 / 7933, 20261025_002236_tdm4 c6 seq
    // 6695 (capdecode: the 0x74 and 0x66 kind 2 of each fall; REFERENCE, accepted by the real client)
    #[test]
    fn falls_reproduce_the_captured_2009_falls() {
        let mut g = fall_game();
        let o = fall_run(
            &mut g,
            &[
                (1077.203, true, 71),
                (1110.0, false, 0),
                (1124.579, false, 0),
                (1050.0, false, 0),
                (900.0, false, 0),
                (822.702, true, 71),
            ],
            1_000,
        );
        let d = damage_msgs(&o);
        assert_eq!(d.len(), 1, "one 0x74");
        same_fall_message(&d[0], "0d74000000003dca193c000000000501ffff00050001000000");
        let r = received(&o, 0);
        assert!((r.health.unwrap() - 99.06134).abs() < 1e-3, "{r:?}");
        assert_eq!(r.armour, None, "armour untouched");
        assert!((g.players[&0].armor - 100.0).abs() < 1e-6);
        let o = fall_run(
            &mut g,
            &[
                (716.441, true, 10),
                (700.0, false, 0),
                (500.0, false, 0),
                (374.027, true, 10),
            ],
            5_000,
        );
        let d = damage_msgs(&o);
        assert_eq!(d.len(), 1);
        same_fall_message(&d[0], "0d7400000000f629593e000000000501ffff00050015000000");
        let r = received(&o, 0);
        assert!((r.health.unwrap() - 77.85393).abs() < 1e-3, "{r:?}");
        // tdm4: a fresh player at 100
        let mut g = fall_game();
        let o = fall_run(
            &mut g,
            &[
                (1080.0, true, 71),
                (1122.735, false, 0),
                (950.0, false, 0),
                (818.736, true, 71),
            ],
            1_000,
        );
        let d = damage_msgs(&o);
        assert_eq!(d.len(), 1);
        same_fall_message(&d[0], "0d7400000000c3d5a33c000000000501ffff00050002000000");
        let r = received(&o, 0);
        assert!((r.health.unwrap() - 98.00006).abs() < 1e-3, "{r:?}");
        assert_eq!(g.players[&0].life, Life::Alive);
    }

    /// Landings that must not hurt: the largest captured harmless fall (h 265.531), a 400-unit ladder descent ending
    /// in a short drop, a fall interrupted by a death and the respawn teleport, any fall after the round end, and
    /// every fall with `fall=off`.
    #[test]
    fn falls_that_do_not_hurt() {
        let mut g = fall_game();
        let o = fall_run(
            &mut g,
            &[(600.0, true, 10), (565.531, false, 0), (334.469, true, 10)],
            1_000,
        );
        assert!(damage_msgs(&o).is_empty(), "h 265.531");
        let o = fall_run(
            &mut g,
            &[
                (900.0, true, 71),
                (890.0, false, fall::LADDER_SURFACE),
                (700.0, false, fall::LADDER_SURFACE),
                (520.0, false, fall::LADDER_SURFACE),
                (510.0, false, 0),
                (500.0, true, 71),
            ],
            2_000,
        );
        assert!(damage_msgs(&o).is_empty(), "ladder descent");
        // high up and airborne, then killed; the respawn (new teleport sequence) lands far below
        fall_run(&mut g, &[(2000.0, true, 10), (2010.0, false, 0)], 3_000);
        g.kill(0, Some(1), 145, false, 3_200);
        g.take_out();
        assert!(
            damage_msgs(&fall_step(&mut g, 100.0, true, 10, 3_300)).is_empty(),
            "trailers of the dead are ignored"
        );
        g.spawn(0, 4_000);
        g.take_out();
        let o = fall_run(&mut g, &[(0.0, true, 10), (0.0, true, 10)], 4_100);
        assert!(damage_msgs(&o).is_empty(), "a run across a death");
        g.end_round(5_000, 1);
        g.take_out();
        let o = fall_run(
            &mut g,
            &[(1000.0, true, 10), (1000.0, false, 0), (0.0, true, 10)],
            5_100,
        );
        assert!(damage_msgs(&o).is_empty(), "round over");
        let mut g = duel(GameTuning {
            move_check: false,
            fall: fall::FallTuning {
                enabled: false,
                ..fall::FallTuning::default()
            },
            ..GameTuning::default()
        });
        let o = fall_run(
            &mut g,
            &[(1000.0, true, 10), (999.0, false, 0), (0.0, true, 10)],
            1_000,
        );
        assert!(damage_msgs(&o).is_empty(), "fall=off");
    }

    /// A fall of 800 units: 200 damage (the cap at `fall_h_hi`), lethal, killed by nobody (0x6d killer 0xff) with the
    /// drawn weapon (none: 0), no score for anyone, the kill feed line before the victim's own 0x74.
    #[test]
    fn a_long_fall_kills_without_a_killer() {
        let mut g = fall_game();
        let o = fall_run(
            &mut g,
            &[(800.0, true, 10), (700.0, false, 0), (0.0, true, 10)],
            1_000,
        );
        let d = damage_msgs(&o);
        assert_eq!(d.len(), 1);
        assert_eq!(
            u32::from_le_bytes([d[0][21], d[0][22], d[0][23], d[0][24]]),
            200
        );
        let kill: Vec<&[u8]> = o
            .iter()
            .filter_map(|x| match x {
                GameOut::To(_, m) if m.as_bytes()[1] == gs::GS_KILL => Some(m.as_bytes()),
                _ => None,
            })
            .collect();
        assert!(!kill.is_empty());
        assert_eq!((kill[0][2], kill[0][3]), (0, gs::NO_KILLER));
        assert_eq!(
            u16::from_le_bytes([kill[0][5], kill[0][6]]),
            fall::FALL_DEATH_WEAPON
        );
        assert!(matches!(g.players[&0].life, Life::Dying { .. }));
        assert_eq!(g.scores, [0, 0]);
        let pos = |id: u8| {
            o.iter()
                .position(|x| matches!(x, GameOut::To(0, m) if m.as_bytes()[1] == id))
                .unwrap()
        };
        assert!(pos(gs::GS_KILL) < pos(gs::GS_DAMAGE_TAKEN));
    }

    /// A game with made-up event-sound rows: one pain variant and one fall voice, told apart by name.
    fn voiced_fall_game() -> Game {
        let mut g = fall_game();
        g.tables.event_sounds = crate::sounds::EventSoundTables {
            pain: vec!["pain.wav".into()],
            fall: Some("fall.wav".into()),
            ..Default::default()
        };
        g
    }

    /// The event sounds of an output: (file, form).
    fn voices(o: &[GameOut]) -> Vec<(String, crate::sounds::SoundForm)> {
        o.iter()
            .filter_map(|x| match x {
                GameOut::Sound(s) => Some((s.file.clone(), s.form)),
                _ => None,
            })
            .collect()
    }

    /// DSV-596: a lethal landing plays the fall voice in the fall-death form, once, and no pain variant; a hurting
    /// landing keeps the fall-hurt sound in the pain form (DSV-152); a shot kill keeps the pain variant; a drop
    /// through a kill volume plays the fall-death sound.
    #[test]
    fn a_fall_death_plays_the_fall_voice_and_the_other_causes_keep_theirs() {
        use crate::sounds::{FALL_DEATH_FORM, PAIN_FORM};
        // a lethal landing (800 units: 200 damage)
        let mut g = voiced_fall_game();
        let o = fall_run(
            &mut g,
            &[(800.0, true, 10), (700.0, false, 0), (0.0, true, 10)],
            1_000,
        );
        assert!(matches!(g.players[&0].life, Life::Dying { .. }));
        assert_eq!(voices(&o), vec![("fall.wav".to_string(), FALL_DEATH_FORM)]);
        assert!(g.plunge.is_none());
        // a hurting landing (21 damage) stays the fall-hurt sound
        let mut g = voiced_fall_game();
        let o = fall_run(
            &mut g,
            &[(716.441, true, 10), (600.0, false, 0), (374.027, true, 10)],
            1_000,
        );
        assert_eq!(g.players[&0].life, Life::Alive);
        assert_eq!(voices(&o), vec![("fall.wav".to_string(), PAIN_FORM)]);
        // a shot kill keeps the pain variant
        let mut g = voiced_fall_game();
        let src = combat::DamageSource {
            weapon: 3,
            dtype: 1,
        };
        g.apply_damage(
            0,
            Some(1),
            500.0,
            [1.0, 0.0, 0.0],
            src,
            TODO_KILL_ZONE_BODY,
            2_000,
        );
        assert!(matches!(g.players[&0].life, Life::Dying { .. }));
        assert_eq!(
            voices(&g.take_out()),
            vec![("pain.wav".to_string(), PAIN_FORM)]
        );
        // a drop through a kill volume
        let mut g = voiced_fall_game();
        g.volume_death(0, 2_000);
        assert!(matches!(g.players[&0].life, Life::Dying { .. }));
        assert_eq!(
            voices(&g.take_out()),
            vec![("fall.wav".to_string(), FALL_DEATH_FORM)]
        );
        // a fall death is not swallowed by a pain sound that still plays
        let mut g = voiced_fall_game();
        g.pain_sound(0, false, 2_000);
        g.take_out();
        g.volume_death(0, 2_100);
        assert_eq!(
            voices(&g.take_out()),
            vec![("fall.wav".to_string(), FALL_DEATH_FORM)]
        );
        // without a fall row the pain variant plays as before
        let mut g = voiced_fall_game();
        g.tables.event_sounds.fall = None;
        g.volume_death(0, 2_000);
        assert_eq!(
            voices(&g.take_out()),
            vec![("pain.wav".to_string(), PAIN_FORM)]
        );
    }

    /// `-combat` fall keys reach the rule.
    #[test]
    fn fall_keys_are_combat_keys() {
        let mut t = GameTuning::default();
        t.set_combat("combat.fall_h_lo", "100").unwrap();
        t.set_combat("fall_d_hi", "50").unwrap();
        t.set_combat("fall", "off").unwrap();
        assert_eq!(
            (t.fall.h_lo, t.fall.d_hi, t.fall.enabled),
            (100.0, 50.0, false)
        );
        assert!(t.set_combat("fall_h_hi", "x").is_err());
    }

    /// ROM-gated: with the user's ROM armour row (DSV-30 default gear) a fall's damage type 5 takes nothing from the
    /// armour, as in the reference stack's tdm4 fall (armour 100 unchanged, no 0x66 kind 3).
    #[test]
    fn rom_fall_damage_spares_the_armour() {
        let rom = dc_rez::dc_rom::rom_or_skip!();
        let (t, notes) = RomTables::load(&rom);
        assert!(notes.is_empty(), "{notes:?}");
        let mut g = Game::new(
            GameTuning {
                move_check: false,
                ..GameTuning::default()
            },
            &world(),
            t,
        );
        for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3)] {
            let e = entry(uid, team, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        g.take_out();
        let armour = g.players[&0].armor;
        assert!(armour > 0.0, "the ROM gear row gives armour");
        let o = fall_run(
            &mut g,
            &[(716.441, true, 10), (600.0, false, 0), (374.027, true, 10)],
            1_000,
        );
        let r = received(&o, 0);
        assert_eq!(r.damage, Some(21), "{r:?}");
        assert!((r.health.unwrap() - (100.0 - 21.207)).abs() < 1e-2, "{r:?}");
        assert_eq!(r.armour, None, "no armour status: {r:?}");
        assert_eq!(g.players[&0].armor, armour);
    }

    /// DSV-85 / DSV-86: a shot effect carries the shooter's client id in `c`; a character hit carries the hit mask
    /// and is followed by the continuation record (`first` 0, the exit mask, the same `from`, `to` at the wall
    /// behind the victim); a miss keeps the bullet mask; both switches work.
    // OBSERVED: capture 20261025_002236_tdm4 c6 seq 2968 (hit 0x0f11 + continuation 0x4bf0, c =
    // the shooter's client id), 20261025_065016_tdm6 (c 0 on the shots of client 0); REFERENCE
    #[test]
    fn hit_records_carry_the_shooter_id_the_hit_mask_and_a_continuation() {
        use dc_sim::{CollModel, CollisionWorld, ConvexPoly, SURF_SOLID, V3};
        let records = |o: &[GameOut]| -> Vec<Vec<u8>> {
            o.iter()
                .filter_map(|x| match x {
                    GameOut::Unreliable(1, m) if m.as_bytes()[1] == gs::FX_SHOT => {
                        Some(m.as_bytes().to_vec())
                    }
                    _ => None,
                })
                .collect()
        };
        let u16_at = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let f32_at = |b: &[u8], o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let walled = |x: f32| {
            let wall = ConvexPoly::from_corners(
                vec![
                    V3::new(x, -100.0, 500.0),
                    V3::new(x, 300.0, 500.0),
                    V3::new(x, 300.0, -500.0),
                    V3::new(x, -100.0, -500.0),
                ],
                SURF_SOLID,
            );
            CollisionWorld::from_parts(vec![CollModel::from_polys(&[wall])], &[])
        };
        let cfg = GameTuning {
            move_check: false,
            ..GameTuning::default()
        };
        let mut g = duel(cfg.clone());
        let (sx, vx) = (g.players[&0].pos[0], g.players[&1].pos[0]);
        assert!(vx > sx + 100.0, "the victim stands on +x: {sx} {vx}");
        let wx = vx + 300.0;
        g.set_trace_world(Some(walled(wx)));
        let f = hit_report(&g, 0, 1, 0.0, None);
        g.on_up(0, Up::Fire(Box::new(f.clone())), 100);
        let r = records(&g.take_out());
        assert_eq!(r.len(), 2, "hit record and continuation");
        assert_eq!(u16_at(&r[0], 2), 3, "the victim");
        assert_eq!(u16_at(&r[0], 11), gs::SHOT_MASK_HIT);
        assert_eq!(r[0][13], 0, "client id of the shooter (pid 0)");
        assert_eq!(u16_at(&r[1], 2), 0, "the world");
        assert_eq!(u16_at(&r[1], 11), gs::SHOT_MASK_EXIT);
        assert_eq!(r[1][13], 0);
        assert_eq!(r[1][4..10], r[0][4..10], "shooter, weapon, ammo");
        assert_eq!(r[1][14..26], r[0][14..26], "the same from");
        assert!((f32_at(&r[1], 26) - wx).abs() < 1.0, "ends at the wall");
        assert!(
            (f32_at(&r[1], 38).abs() - 1.0).abs() < 1e-3,
            "the wall normal"
        );
        // the other player's shot carries his id; a miss keeps the bullet mask and has no continuation
        let mut back = hit_report(&g, 1, 0, 0.0, None);
        back.hit = None;
        g.on_up(1, Up::Fire(Box::new(back)), 300);
        let r = records(&g.take_out());
        assert_eq!(r.len(), 1);
        assert_eq!((u16_at(&r[0], 11), r[0][13]), (gs::SHOT_MASK_MISS, 1));
        // no wall behind the victim: no continuation
        g.set_trace_world(None);
        g.on_up(0, Up::Fire(Box::new(f.clone())), 500);
        assert_eq!(records(&g.take_out()).len(), 1);
        // the switches
        let mut c2 = cfg;
        c2.set_combat("hit_mask", "0x4f10").unwrap();
        c2.set_combat("hit_continuation", "off").unwrap();
        assert!(c2.set_combat("hit_mask", "0x1ffff").is_err());
        let mut g = duel(c2);
        g.set_trace_world(Some(walled(wx)));
        let f = hit_report(&g, 0, 1, 0.0, None);
        g.on_up(0, Up::Fire(Box::new(f)), 100);
        let r = records(&g.take_out());
        assert_eq!(r.len(), 1);
        assert_eq!(u16_at(&r[0], 11), 0x4f10);
    }

    /// The `GS_MOVER_UPDATE` flag words sent to pid 0, in order, with the freeze bit where the record has one.
    fn mover_records(out: &[GameOut]) -> Vec<(u16, Option<u8>)> {
        out.iter()
            .filter_map(|o| match o {
                GameOut::To(0, m) if m.as_bytes()[1] == gs::GS_MOVER_UPDATE => {
                    let b = m.as_bytes();
                    let flags = u16::from_le_bytes([b[2], b[3]]);
                    Some((flags, (flags == gs::MOVER_FREEZE).then(|| b[4])))
                }
                _ => None,
            })
            .collect()
    }

    /// DSV-77 (reinstated): a player frozen at the round result is released (`[0x0080][freeze 0]`) at his next
    /// spawn, before the spawn set's teleport; a spawn of a player who is not frozen sends no release. (No round of
    /// the current classes follows a round end on our server, so the spawn is called directly here.)
    // OBSERVED: capture 20261025_000014_tdm1 c5 seq 410 (`0d a0 80 00 00`; REFERENCE)
    // DC-DEFAULT: DSV-77 (reinstated, proposed)
    #[test]
    fn a_frozen_player_is_released_before_the_spawn_teleport() {
        let mut g = Game::new(GameTuning::default(), &world(), tables());
        g.add_player(0, None, None, 1, [0.0; 3], [0.0; 3], 0);
        g.on_up(0, Up::PlayerInfo(Box::default()), 10);
        g.on_up(0, Up::WorldReady, 20);
        assert!(alive(&g));
        g.take_out();
        g.end_round(1_000, 1);
        let o = g.take_out();
        assert_eq!(mover_records(&o), vec![(gs::MOVER_FREEZE, Some(1))]);
        assert!(g.players[&0].frozen);
        // the end of the wait does not freeze him twice
        g.tick_round(3_000);
        assert!(mover_records(&g.take_out()).is_empty());
        g.spawn(0, 5_000);
        let m = mover_records(&g.take_out());
        assert_eq!(m.first(), Some(&(gs::MOVER_FREEZE, Some(0))), "{m:x?}");
        let teleport = m.iter().position(|x| x.0 & gs::MOVER_TELEPORT != 0);
        assert!(teleport.is_some_and(|i| i > 0), "{m:x?}");
        assert!(!g.players[&0].frozen);
        g.spawn(0, 6_000);
        let m = mover_records(&g.take_out());
        assert!(m.iter().all(|x| x.0 != gs::MOVER_FREEZE), "{m:x?}");
    }

    /// DSV-159: four team mates spawning in one tick stand on four different points; a point is handed out again
    /// only after its occupant left it (moved away or died). Playtest report: "players can still spawn in the exact
    /// spot as someone else" (the old draw ignored team mates).
    // DC-DEFAULT: DSV-159 (proposed)
    #[test]
    fn spawn_points_are_not_handed_out_while_someone_stands_on_them() {
        let mut w = world();
        w.spawns = (0..4)
            .map(|i| SpawnPoint {
                pos: [-600.0 + 200.0 * i as f32, 0.0, 0.0],
                rot: [0.0; 3],
                team_filter: Some(b"Red".to_vec()),
            })
            .collect();
        let mut g = Game::new(GameTuning::default(), &w, tables());
        for pid in 0..4u8 {
            let e = entry(10 + u32::from(pid), 0, "Mate");
            g.add_player(
                pid,
                Some(&e),
                None,
                1 + 2 * u16::from(pid),
                [0.0; 3],
                [0.0; 3],
                0,
            );
        }
        let x = |g: &Game, pid: u8| g.players[&pid].pos[0] as i32;
        for round in 0..20u32 {
            for pid in 0..4u8 {
                g.players.get_mut(&pid).unwrap().life = Life::Waiting { died: 0 };
            }
            for pid in 0..4u8 {
                g.spawn(pid, 1_000 * (round + 1));
            }
            let mut seen: Vec<i32> = (0..4).map(|p| x(&g, p)).collect();
            seen.sort_unstable();
            assert_eq!(seen, vec![-600, -400, -200, 0], "round {round}");
        }
        // player 3 respawns while 0..2 stand: only his own (free) point is left
        let left = x(&g, 3);
        for t in 0..30 {
            g.players.get_mut(&3).unwrap().life = Life::Waiting { died: 0 };
            g.spawn(3, 100_000 + t);
            assert_eq!(x(&g, 3), left);
        }
        // player 0 walks away: his point is free again and gets reused
        let freed = x(&g, 0);
        g.players.get_mut(&0).unwrap().pos = [0.0, 0.0, 900.0];
        let mut reused = false;
        for t in 0..60 {
            g.players.get_mut(&3).unwrap().life = Life::Waiting { died: 0 };
            g.spawn(3, 200_000 + t);
            reused |= x(&g, 3) == freed;
            assert!([freed, left].contains(&x(&g, 3)));
        }
        assert!(reused, "the left point is handed out again");
    }

    /// DSV-471 (project decision): in Elimination Pro (12 and 13) every team keeps its side for the whole
    /// match. Six rounds on two ROM bomb worlds, two players per team, one side wiped out each round (the winner
    /// alternates, so the goal is never reached): every spawn of every round is on the team's Search and Destroy
    /// set, the two teams never stand on one side, and a joiner in round 3 spawns on his team's base.
    #[test]
    fn rom_elimination_pro_teams_keep_their_side_for_six_rounds() {
        use crate::world::{load_from_rom, NameForm};
        let rom = dc_rez::dc_rom::rom_or_skip!();
        let (t, _) = RomTables::load(&rom);
        for world in ["SandHog", "SnowValley"] {
            for class in ["12", "13"] {
                let w = load_from_rom(&rom, &format!("Worlds/{world}"), NameForm::Listed, None)
                    .expect("world");
                let team_of = |i: usize| t.teams.team_of(w.spawns[i].team_filter.as_deref());
                let on_set = |pos: [f32; 3], team: u8| {
                    (0..w.spawns.len()).any(|i| {
                        (w.spawns[i].pos[0] - pos[0]).hypot(w.spawns[i].pos[2] - pos[2]) < 0.5
                            && team_of(i) == Some(team)
                            && w.spawn_modes[i].as_deref().is_some_and(|m| {
                                crate::chars::name_key(
                                    m.trim_ascii().to_ascii_lowercase().as_slice(),
                                ) == super::bomb::SND_SPAWN_FILTER
                            })
                    })
                };
                let mut g = Game::new(
                    GameTuning {
                        move_check: false,
                        trace: false,
                        ..GameTuning::default()
                    },
                    &w,
                    t.clone(),
                );
                let mut room = RoomSettings::default();
                room.pairs.insert("type".into(), class.into());
                room.pairs.insert("goal".into(), "8".into());
                g.set_room(&room);
                let join = |g: &mut Game, pid: u8, team: u8| {
                    let e = entry(100 + pid as u32, team, "P");
                    g.note_identified(&e);
                    g.add_player(
                        pid,
                        Some(&e),
                        None,
                        1 + 2 * pid as u16,
                        [0.0; 3],
                        [0.0; 3],
                        0,
                    );
                    g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
                    g.on_up(pid, Up::WorldReady, 20);
                };
                for (pid, team) in [(0u8, 0u8), (1, 0), (2, 1), (3, 1)] {
                    join(&mut g, pid, team);
                }
                let mut now = 100;
                g.tick(now);
                g.take_out();
                let mut spawns = 0;
                for round in 1..=6u32 {
                    assert_eq!(g.round_no().max(1), round, "{world} class {class}");
                    if round == 3 {
                        join(&mut g, 4, 0);
                        join(&mut g, 5, 1);
                        now += 50;
                        g.tick(now);
                    }
                    let mut sides = [Vec::new(), Vec::new()];
                    for p in g.players().values().filter(|p| p.spawns > 0) {
                        assert!(
                            on_set(p.pos, p.team),
                            "{world} class {class} round {round}: pid {} (team {}) at {:?} is not on its S&D base",
                            p.pid,
                            p.team,
                            p.pos
                        );
                        assert_eq!(
                            p.team,
                            1 + [0, 0, 1, 1, 0, 1][p.pid as usize],
                            "{world} class {class} round {round}: pid {} changed team",
                            p.pid
                        );
                        sides[p.team as usize - 1].push(p.pos);
                        spawns += 1;
                    }
                    assert!(!sides[0].is_empty() && !sides[1].is_empty());
                    // no two teams on one side: no position is on both teams' sets
                    for a in &sides[0] {
                        assert!(
                            !on_set(*a, 2),
                            "{world} class {class} round {round}: team 1 on team 2's base"
                        );
                    }
                    for b in &sides[1] {
                        assert!(
                            !on_set(*b, 1),
                            "{world} class {class} round {round}: team 2 on team 1's base"
                        );
                    }
                    if round == 6 {
                        break;
                    }
                    // wipe out the losing side (odd rounds team 2 falls, even rounds team 1)
                    let loser = if round % 2 == 1 { 2 } else { 1 };
                    let dead: Vec<u8> = g
                        .players()
                        .values()
                        .filter(|p| p.team == loser && p.life == Life::Alive)
                        .map(|p| p.pid)
                        .collect();
                    assert!(!dead.is_empty(), "{world} class {class} round {round}");
                    for pid in dead {
                        now += 50;
                        let killer = g
                            .players()
                            .values()
                            .find(|p| p.team != loser && p.life == Life::Alive)
                            .map(|p| p.pid);
                        g.kill(pid, killer, 0, false, now);
                        g.tick(now);
                    }
                    let limit = now + 120_000;
                    while now < limit && g.round_no() <= round {
                        now += 50;
                        g.tick(now);
                        g.take_out();
                    }
                    assert_eq!(
                        g.round_no(),
                        round + 1,
                        "{world} class {class}: round {round} did not end"
                    );
                    assert!(!g.modes.swapped, "{world} class {class}: sides switched");
                }
                assert!(
                    spawns >= 26,
                    "{world} class {class}: {spawns} spawns checked"
                );
            }
        }
    }
    /// Synthetic launcher rows (made-up numbers in the ROM key shape): weapon 3 a rocket (blast 40, radius 550),
    /// weapon 4 a flame (no blast; 30 per second for 1 s within 75 units), both flown straight.
    fn launcher_tables() -> RomTables {
        let mut t = tables();
        let doc = dc_bute::ButeDoc::parse(concat!(
            "[Weapon3]\nName = \"L\"\nAmmoName0 = \"R\"\n",
            "[Weapon4]\nName = \"F\"\nAmmoName0 = \"FL\"\n",
            "ShotsPerClip = 99\n",
            "[Ammo0]\nName = \"R\"\nType = 1\nAreaDamage = 40\nAreaDamageType = 7\nAreaDamageRadius = 550\n",
            "ProjectileFXName = \"Missile\"\n",
            "[Ammo1]\nName = \"FL\"\nType = 1\nAreaDamage = 0\nAreaDamageRadius = 0\nProgDamage = 30\n",
            "ProgDamageType = 3\nProgDamageDuration = 1\nProgDamageRadius = 75\nProgDamageLifetime = 2\n",
            "ProjectileFXName = \"Flame\"\n",
            "[WeaponOrder]\nClass0 = \"P\"\nP0 = \"L\"\nP1 = \"F\"\n",
        ));
        t.weapons = crate::rom_tables::WeaponTable::from_doc(&doc);
        for (name, speed, life) in [("Missile", 1300.0, 10.0), ("Flame", 500.0, 1.0)] {
            t.projectiles.rows.insert(
                dc_bute::fold_latin1(name),
                crate::rom_tables::ProjectileRow {
                    gravity: Some(false),
                    lifetime_s: Some(life),
                    velocity: Some(speed),
                    ..Default::default()
                },
            );
        }
        t
    }

    /// Three players at the given places (0 the shooter), in the running round, spawn windows over.
    fn launcher_game(t: RomTables, at: &[[f32; 3]]) -> Game {
        let mut g = Game::new(
            GameTuning {
                move_check: false,
                ..GameTuning::default()
            },
            &world(),
            t,
        );
        for (i, pos) in at.iter().enumerate() {
            let pid = i as u8;
            let e = entry(7 + u32::from(pid), pid.min(1), "P");
            g.note_identified(&e);
            g.add_player(
                pid,
                Some(&e),
                None,
                1 + 2 * u16::from(pid),
                [0.0; 3],
                [0.0; 3],
                0,
            );
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
            g.players.get_mut(&pid).unwrap().pos = *pos;
        }
        g.spawn_notes.clear();
        g.take_out();
        g
    }

    fn shot_at(weapon: u16, ammo: u16, origin: [f32; 3], at: [f32; 3]) -> crate::up::UpFire {
        let d = sub3(at, origin);
        let l = len3(d);
        crate::up::UpFire {
            weapon_id: weapon,
            ammo_id: ammo,
            weapon_pos: origin,
            origin,
            dir: [d[0] / l, d[1] / l, d[2] / l],
            last: true,
            hit: None,
            complete: true,
            ..crate::up::UpFire::default()
        }
    }

    /// The forward axis (rotated +Z) of a rotation (x, y, z, w).
    fn forward_of(q: [f32; 4]) -> [f32; 3] {
        let [x, y, z, w] = q;
        [
            2.0 * (x * z + w * y),
            2.0 * (y * z - w * x),
            1.0 - 2.0 * (x * x + y * y),
        ]
    }

    /// DSV-181 (amended): a launcher's shot that comes with a client hit claim (dc-client before CLI-1590) is answered
    /// with the record of no hit (first 0xFFFF, the sky surface: no impact or blood drawn at the aim point), no damage
    /// at the shot; the rocket object faces along its flight; the rocket blows up on the enemy (the row's 40).
    #[test]
    fn a_launcher_shot_has_no_hit_record_and_its_rocket_faces_its_flight() {
        let mut g = launcher_game(launcher_tables(), &[[0.0; 3], [600.0, 0.0, 0.0]]);
        let mut f = shot_at(3, 0, [0.0, 33.0, 0.0], [600.0, 0.0, 0.0]);
        f.hit = Some((600.0, Some(g.players[&1].obj)));
        g.on_up(0, Up::Fire(Box::new(f.clone())), 4000);
        let o = g.take_out();
        let recs: Vec<&[u8]> = o
            .iter()
            .filter_map(|x| match x {
                GameOut::Unreliable(0, m) if m.as_bytes()[1] == gs::FX_SHOT => Some(m.as_bytes()),
                _ => None,
            })
            .collect();
        assert_eq!(recs.len(), 1, "one shot record");
        assert_eq!(
            u16::from_le_bytes([recs[0][2], recs[0][3]]),
            0xFFFF,
            "no hit object"
        );
        let want = gs::shot_effect(&gs::ShotEffect {
            first: None,
            shooter: g.players[&0].obj,
            weapon: 3,
            ammo: 0,
            from: f.origin,
            to: [
                f.origin[0] + f.dir[0],
                f.origin[1] + f.dir[1],
                f.origin[2] + f.dir[2],
            ],
            normal: [-f.dir[0], -f.dir[1], -f.dir[2]],
            surface: projectile::LAUNCH_SURFACE,
            mask: gs::SHOT_MASK_MISS,
            client: 0,
            mods: gs::NO_SHOT_MODS,
        });
        assert_eq!(recs[0], want.as_bytes(), "the launch record");
        let rot = o
            .iter()
            .find_map(|x| match x {
                GameOut::Projectile(p) => Some(p.rot),
                _ => None,
            })
            .expect("the rocket object");
        let fw = forward_of(rot);
        assert!(
            (0..3).all(|i| (fw[i] - f.dir[i]).abs() < 1e-4),
            "faces {fw:?}, flies {:?}",
            f.dir
        );
        assert_eq!(g.players[&1].health, 100.0, "no damage at the shot");
        g.tick_projectiles(4600);
        let lost = 100.0 - g.players[&1].health;
        let scale = g.cfg.projectile.blast_damage_scale;
        assert!((lost - 40.0 * scale).abs() < 1e-3, "lost {lost}");
    }

    /// Synthetic rows in the shape of the user's ROM (made-up numbers): weapon 3 a rocket (`Type` 1, a projectile
    /// row with no class: the `Type` rule), weapon 5 a grenade launcher (`Type` 2 with an impact-class row and a
    /// `Lifetime` past the 600 s bound), weapon 6 a frag (`Type` 2, a fused-class row, 2.5 s), weapon 7 a flash
    /// grenade (`Type` 1, a fused-class row, 1.5 s), weapon 8 a hand-thrown ball (`ClientWeaponType` 2, `Type` 2,
    /// an impact-class row).
    fn round_class_tables() -> RomTables {
        let mut t = tables();
        let doc = dc_bute::ButeDoc::parse(concat!(
            "[Weapon3]\nName = \"L\"\nAmmoName0 = \"R\"\n",
            "[Weapon5]\nName = \"GL\"\nAmmoName0 = \"GS\"\nShotsPerClip = 6\n",
            "[Weapon6]\nName = \"N\"\nAmmoName0 = \"NA\"\n",
            "[Weapon7]\nName = \"FB\"\nAmmoName0 = \"FA\"\n",
            "[Weapon8]\nName = \"SB\"\nAmmoName0 = \"SA\"\nClientWeaponType = 2\nShotsPerClip = 1\n",
            "[Ammo0]\nName = \"R\"\nType = 1\nAreaDamage = 40\nAreaDamageType = 7\nAreaDamageRadius = 550\n",
            "ProjectileFXName = \"Missile\"\n",
            "[Ammo1]\nName = \"GS\"\nType = 2\nAreaDamage = 55\nAreaDamageType = 7\nAreaDamageRadius = 300\n",
            "ProjectileFXName = \"Shell\"\n",
            "[Ammo2]\nName = \"NA\"\nType = 2\nAreaDamage = 50\nAreaDamageType = 7\nAreaDamageRadius = 300\n",
            "ProjectileFXName = \"Nade\"\n",
            "[Ammo3]\nName = \"FA\"\nType = 1\nAreaDamage = 5\nAreaDamageType = 7\nAreaDamageRadius = 300\n",
            "ProjectileFXName = \"Flash\"\n",
            "[Ammo4]\nName = \"SA\"\nType = 2\nAreaDamage = 20\nAreaDamageType = 7\nAreaDamageRadius = 100\n",
            "ProjectileFXName = \"Ball\"\nMaxAmount = 3\n",
            "[WeaponOrder]\nClass0 = \"P\"\nP0 = \"L\"\nP1 = \"GL\"\nP2 = \"N\"\nP3 = \"FB\"\nP4 = \"SB\"\n",
        ));
        t.weapons = crate::rom_tables::WeaponTable::from_doc(&doc);
        use crate::rom_tables::RoundFlight::{Launched, Thrown};
        for (name, flight, gravity, life) in [
            ("Missile", None, false, 10.0),
            ("Shell", Some(Launched), true, 10000.0),
            ("Nade", Some(Thrown), true, 2.5),
            ("Flash", Some(Thrown), true, 1.5),
            ("Ball", Some(Launched), true, 10000.0),
        ] {
            t.projectiles.rows.insert(
                dc_bute::fold_latin1(name),
                crate::rom_tables::ProjectileRow {
                    gravity: Some(gravity),
                    gravity_override: gravity.then_some(-1000.0),
                    lifetime_s: Some(life),
                    velocity: Some(if gravity { 1550.0 } else { 1300.0 }),
                    flight,
                    ..Default::default()
                },
            );
        }
        t
    }

    /// The throw callouts (type 0x19 `k` 0x12) from the origin `[0, 33, 0]` in `o`.
    fn callouts(g: &Game, o: &[GameOut]) -> usize {
        o.iter()
            .filter(|x| {
                g.players.values().any(|p| {
                    let (a, b, c) = throw_sound_bytes(p.team);
                    let want = gs::throw_sound(a, u32::from(p.pid), [0.0, 33.0, 0.0], b, c);
                    matches!(x, GameOut::Unreliable(_, m) if m.as_bytes() == want.as_bytes())
                })
            })
            .count()
    }

    /// DSV-650: the class of the projectile row decides the flight, not the ammunition `Type`.
    #[test]
    fn the_projectile_class_tells_a_launched_round_from_a_thrown_one() {
        use crate::rom_tables::RoundFlight::{Launched, Thrown};
        let t = round_class_tables();
        assert_eq!(
            t.round_flight(3, 0),
            Some(Launched),
            "the rocket: the Type rule"
        );
        assert_eq!(
            t.round_flight(5, 1),
            Some(Launched),
            "Type 2 with the impact class"
        );
        assert_eq!(t.round_flight(6, 2), Some(Thrown), "the frag");
        assert_eq!(
            t.round_flight(7, 3),
            Some(Thrown),
            "Type 1 with the fused class"
        );
        assert!(t.weapons.throws_round(5, 1), "the launcher's row is Type 2");
        assert_eq!(crate::rom_tables::class_flight("not a class"), None);
    }

    /// DSV-650: a grenade launcher's shot (ammunition `Type` 2, an impact-class row) is fired, not thrown: no throw
    /// callout, the launch record, an object facing its flight, and it bursts on the enemy within a fraction of a
    /// second (before: the throw callout, a bouncing shell and the grenade fuse), even right after a cancelled hold.
    #[test]
    fn a_grenade_launcher_shell_bursts_on_impact_without_the_throw_callout() {
        let mut g = launcher_game(round_class_tables(), &[[0.0; 3], [300.0, 0.0, 0.0]]);
        // a hold just before (a grenade drawn and put away) does not make the launcher's shot a throw
        g.players.get_mut(&0).unwrap().last_throw = Some(3900);
        let f = shot_at(5, 1, [0.0, 33.0, 0.0], [300.0, 45.0, 0.0]);
        g.on_up(0, Up::Fire(Box::new(f.clone())), 4000);
        let o = g.take_out();
        assert_eq!(callouts(&g, &o), 0, "no throw callout");
        let rec = o.iter().any(|x| {
            matches!(x, GameOut::Unreliable(0, m) if m.as_bytes()[1] == gs::FX_SHOT
                && u16::from_le_bytes([m.as_bytes()[2], m.as_bytes()[3]]) == 0xFFFF)
        });
        assert!(rec, "the launch record");
        let rot = o
            .iter()
            .find_map(|x| match x {
                GameOut::Projectile(p) => Some(p.rot),
                _ => None,
            })
            .expect("the shell object");
        let fw = forward_of(rot);
        assert!(
            (0..3).all(|i| (fw[i] - f.dir[i]).abs() < 1e-4),
            "faces its flight"
        );
        assert_eq!(g.flights_in_air(), 1);
        assert!(g.flights[0].impact, "an impact flight");
        let mut t = 4000;
        while g.flights_in_air() > 0 && t < 8000 {
            t += 16;
            g.tick_projectiles(t);
        }
        assert!(t < 4600, "burst on the enemy at once, not at a fuse ({t})");
        assert!(g.players[&1].health < 100.0, "the blast hurt the enemy");
    }

    /// DSV-650 (amendment): a hand-thrown weapon (`ClientWeaponType` 2) whose row is of the impact class keeps the
    /// throw's client-side bookkeeping (the callout, no shot record, the count mirror after the release, even without
    /// a hold) while its flight is an impact one: it bursts on the enemy at once.
    #[test]
    fn a_hand_thrown_impact_round_keeps_the_throw_bookkeeping_and_bursts_on_impact() {
        let mut g = launcher_game(round_class_tables(), &[[0.0; 3], [300.0, 0.0, 0.0]]);
        assert!(g.tables.weapons.hand_thrown(8));
        assert!(!g.tables.weapons.hand_thrown(5), "the launcher is not");
        let f = shot_at(8, 4, [0.0, 33.0, 0.0], [300.0, 45.0, 0.0]);
        g.on_up(0, Up::Fire(Box::new(f)), 4000);
        let o = g.take_out();
        assert!(callouts(&g, &o) > 0, "the throw callout");
        let records = o
            .iter()
            .filter(|x| matches!(x, GameOut::Unreliable(_, m) if m.as_bytes()[1] == gs::FX_SHOT))
            .count();
        assert_eq!(records, 0, "no shot record");
        let w = g.players[&0]
            .weapons
            .iter()
            .find(|w| w.weapon == 8)
            .expect("the carried ball");
        let mirror = gs::player_ammo(
            w.weapon,
            w.ammo,
            w.carried() as f32,
            w.slot,
            gs::AMMO_EXTRA_THROW,
        );
        assert!(
            o.iter()
                .any(|x| matches!(x, GameOut::To(0, m) if m.as_bytes() == mirror.as_bytes())),
            "the count mirror to the thrower"
        );
        assert_eq!(g.flights_in_air(), 1);
        assert!(g.flights[0].impact, "an impact flight");
        let mut t = 4000;
        while g.flights_in_air() > 0 && t < 8000 {
            t += 16;
            g.tick_projectiles(t);
        }
        assert!(t < 4600, "burst on the enemy at once ({t})");
        assert!(g.players[&1].health < 100.0);
    }

    /// DSV-650 (amended): a launched round whose row has no `Lifetime` keeps the fuse default (the 10 s bound is only
    /// for a `Lifetime` past the 600 s bound).
    #[test]
    fn a_launched_round_without_a_lifetime_keeps_the_fuse_default() {
        let mut t = round_class_tables();
        let row = t
            .projectiles
            .rows
            .get_mut(&dc_bute::fold_latin1("Shell"))
            .unwrap();
        row.gravity = Some(false);
        row.lifetime_s = None;
        let mut g = launcher_game(t, &[[0.0; 3], [0.0, 0.0, 3000.0]]);
        g.pbox.min = [-1.0e6; 3];
        g.pbox.max = [1.0e6; 3];
        g.on_up(
            0,
            Up::Fire(Box::new(shot_at(
                5,
                1,
                [0.0, 33.0, 0.0],
                [300.0, 33.0, 0.0],
            ))),
            4000,
        );
        assert!(g.flights[0].impact);
        let fuse = g.cfg.projectile.fuse_ms;
        g.tick_projectiles(4000 + fuse - 50);
        assert_eq!(g.flights_in_air(), 1, "in flight before the fuse default");
        g.tick_projectiles(4000 + fuse + 16);
        assert_eq!(
            g.flights_in_air(),
            0,
            "burst at the fuse default, not at 10 s"
        );
    }

    /// DSV-650: a launched round that meets nothing bursts at the end of its flight bound (the row's `Lifetime` is
    /// past the 600 s bound: IMPACT_LIFE_MS), not at the grenade fuse.
    #[test]
    fn a_launched_round_without_a_usable_lifetime_flies_to_its_bound() {
        let mut t = round_class_tables();
        t.projectiles
            .rows
            .get_mut(&dc_bute::fold_latin1("Shell"))
            .unwrap()
            .gravity = Some(false);
        let mut g = launcher_game(t, &[[0.0; 3], [0.0, 0.0, 3000.0]]);
        g.pbox.min = [-1.0e6; 3];
        g.pbox.max = [1.0e6; 3];
        g.on_up(
            0,
            Up::Fire(Box::new(shot_at(
                5,
                1,
                [0.0, 33.0, 0.0],
                [300.0, 33.0, 0.0],
            ))),
            4000,
        );
        assert_eq!(g.flights_in_air(), 1);
        g.tick_projectiles(4000 + g.cfg.projectile.fuse_ms + 100);
        assert_eq!(g.flights_in_air(), 1, "still flying after the grenade fuse");
        g.tick_projectiles(4000 + projectile::IMPACT_LIFE_MS + 16);
        assert_eq!(g.flights_in_air(), 0, "burst at the bound");
    }

    /// DSV-650: the frag still waits for its fuse (with the throw callout), the flash grenade (`Type` 1, fused class)
    /// is thrown even with no hold (a sprint gap, CLI-573), and the rocket (no class) is unchanged.
    #[test]
    fn the_frag_and_the_flash_keep_their_fuse_and_the_rocket_its_impact() {
        for (weapon, ammo, fuse) in [(6u16, 2u16, 2500u32), (7, 3, 1500)] {
            let mut g = launcher_game(round_class_tables(), &[[0.0; 3], [300.0, 0.0, 0.0]]);
            let f = shot_at(weapon, ammo, [0.0, 33.0, 0.0], [300.0, 45.0, 0.0]);
            g.on_up(0, Up::Fire(Box::new(f)), 4000);
            let o = g.take_out();
            assert!(callouts(&g, &o) > 0, "weapon {weapon}: the throw callout");
            assert_eq!(g.flights_in_air(), 1, "weapon {weapon}");
            assert!(!g.flights[0].impact, "weapon {weapon}: a thrown flight");
            g.tick_projectiles(4000 + fuse - 50);
            assert_eq!(g.flights_in_air(), 1, "weapon {weapon}: waits for its fuse");
            g.tick_projectiles(4000 + fuse + 16);
            assert_eq!(g.flights_in_air(), 0, "weapon {weapon}: gone at its fuse");
        }
        let mut g = launcher_game(round_class_tables(), &[[0.0; 3], [600.0, 0.0, 0.0]]);
        g.on_up(
            0,
            Up::Fire(Box::new(shot_at(3, 0, [0.0, 33.0, 0.0], [600.0, 0.0, 0.0]))),
            4000,
        );
        let o = g.take_out();
        assert_eq!(callouts(&g, &o), 0, "the rocket: no callout");
        assert!(g.flights[0].impact, "the rocket: an impact flight");
        g.tick_projectiles(4600);
        assert_eq!(g.flights_in_air(), 0, "the rocket burst on the enemy");
        assert!(g.players[&1].health < 100.0);
    }

    /// DSV-455: a flame bursts on the enemy it reaches without a blast; he burns 30 per second for the row's second
    /// (four steps of 7.5), then stops; a player 300 units from the burst and the shooter do not burn.
    #[test]
    fn a_flame_burns_the_player_it_reaches_for_the_rows_duration() {
        let mut g = launcher_game(
            launcher_tables(),
            &[[0.0; 3], [300.0, 0.0, 0.0], [300.0, 0.0, 300.0]],
        );
        assert!(g.tables.weapons.prog(4, 1).is_some());
        assert!(g.tables.weapons.area(4, 1).is_none(), "no blast row");
        let f = shot_at(4, 1, [0.0, 33.0, 0.0], [300.0, 0.0, 0.0]);
        g.on_up(0, Up::Fire(Box::new(f)), 4000);
        assert_eq!(g.flights_in_air(), 1, "the flame flies");
        let mut t = 4000;
        while g.flights_in_air() > 0 && t < 5200 {
            t += 16;
            g.tick_projectiles(t);
            g.tick_burns(t);
        }
        assert_eq!(g.flights_in_air(), 0);
        assert!(
            t < 4700,
            "burst on the enemy after about 0.6 s, not at the end of its life ({t})"
        );
        assert_eq!(g.burning(), vec![1], "only the enemy it reached burns");
        assert_eq!(g.players[&1].health, 100.0, "no blast");
        let burst = t;
        while t < burst + 2000 {
            t += 16;
            g.tick_burns(t);
        }
        let lost = 100.0 - g.players[&1].health;
        assert!((lost - 30.0).abs() < 1e-3, "lost {lost}");
        assert!(g.burning().is_empty(), "the burn ended");
        assert_eq!(g.players[&0].health, 100.0, "the shooter does not burn");
        assert_eq!(g.players[&2].health, 100.0, "out of reach");
    }

    /// DSV-456: the agent's flame burns at `spy.agent_damage_scale` x the row (and his rocket blasts likewise); a
    /// hunter's identical flame burns at the row's value.
    #[test]
    fn the_agents_flame_burns_at_the_damage_scale() {
        let burn_of = |agent: bool| {
            let mut g = launcher_game(launcher_tables(), &[[0.0; 3], [300.0, 0.0, 0.0]]);
            g.cfg.agent_damage_scale = 3.0;
            g.spy.agent = if agent { Some(0) } else { None };
            let f = shot_at(4, 1, [0.0, 33.0, 0.0], [300.0, 0.0, 0.0]);
            g.on_up(0, Up::Fire(Box::new(f)), 4000);
            let mut t = 4000;
            while t < 7000 {
                t += 16;
                g.tick_projectiles(t);
                g.tick_burns(t);
            }
            100.0 - g.players[&1].health
        };
        let hunter = burn_of(false);
        let agent = burn_of(true);
        assert!((hunter - 30.0).abs() < 1e-3, "the row: {hunter}");
        assert!((agent - 90.0).abs() < 1e-3, "3 x the row: {agent}");
    }

    /// DSV-455: flames that keep reaching a player keep him burning (one burn, renewed: no stacking).
    #[test]
    fn a_flame_hose_renews_one_burn() {
        let mut g = launcher_game(launcher_tables(), &[[0.0; 3], [300.0, 0.0, 0.0]]);
        g.players.get_mut(&1).unwrap().health = 1000.0;
        let mut t = 4000;
        let mut next_shot = t;
        while t < 6000 {
            if t >= next_shot {
                let f = shot_at(4, 1, [0.0, 33.0, 0.0], [300.0, 0.0, 0.0]);
                g.on_up(0, Up::Fire(Box::new(f)), t);
                next_shot += 128;
            }
            t += 16;
            g.tick_projectiles(t);
            g.tick_burns(t);
            assert!(g.burning().len() <= 1);
        }
        // first burst about 0.6 s in, then 30 per second: about 1.4 s of burning by 6 s
        let lost = 1000.0 - g.players[&1].health;
        assert!((30.0..=50.0).contains(&lost), "lost {lost}");
    }

    /// ROM (skipped without CA_ROM): the Spy Hunt agent's launcher row (GMS `[AgentModelSetting] PrimaryWeapon1`,
    /// the bounty AT4) on WarHead's level geometry: a rocket at the floor between two enemies, each takes the row's
    /// `AreaDamage` x the falloff of his distance from the burst; the agent (500 health) who fired it from 400 units
    /// is not killed by his own rocket. Only relations are asserted; no ROM value is printed.
    #[test]
    fn rom_agent_rocket_hits_enemies_with_the_row_falloff_on_warhead() {
        let Some(rom) = dc_rez::dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let (t, _) = RomTables::load(&rom);
        let Ok(w) = crate::world::load_from_rom(
            &rom,
            "Worlds/WarHead",
            crate::world::NameForm::Listed,
            None,
        ) else {
            println!("skipped: no WarHead world");
            return;
        };
        let vfs = dc_rez::RezVfs::new(&rom);
        let gr = dc_bute::GameRoomData::from_vfs(&vfs).expect("room table");
        let key = gr
            .modes(false)
            .iter()
            .find(|m| m.get("GameType").map(|v| v.trim()) == Some("9"))
            .and_then(|m| m.get("Key").cloned())
            .expect("a class-9 row");
        let doc = crate::coop_rom::find_gms(&vfs, &key, r"Worlds\WARHEAD.dat").expect("GMS");
        let gms = super::spy::SpyGms::from_gms(&doc);
        let weapon = gms
            .agent_weapons
            .iter()
            .copied()
            .find(|&x| {
                t.weapons
                    .fires_projectile(x, t.weapons.ammo_of(x).unwrap_or(0))
                    && t.weapons
                        .prog(x, t.weapons.ammo_of(x).unwrap_or(0))
                        .is_none()
            })
            .expect("an agent launcher row");
        let ammo = t.weapons.ammo_of(weapon).expect("its ammunition");
        let area = t.weapons.area(weapon, ammo).expect("a blast row");
        let mut g = Game::new(
            GameTuning {
                move_check: false,
                ..GameTuning::default()
            },
            &w,
            t,
        );
        for (pid, uid, team, obj) in [(0u8, 7u32, 0u8, 1u16), (1, 8, 1, 3), (2, 9, 1, 5)] {
            let e = entry(uid, team, "P");
            g.note_identified(&e);
            g.add_player(pid, Some(&e), None, obj, [0.0; 3], [0.0; 3], 0);
            g.on_up(pid, Up::PlayerInfo(Box::default()), 10);
            g.on_up(pid, Up::WorldReady, 20);
        }
        g.tick(100);
        g.spawn_notes.clear();
        g.players.get_mut(&0).unwrap().health = 500.0;
        let p0 = g.players[&0].pos;
        let origin = [p0[0], p0[1] + 25.0, p0[2]];
        // the floor 400 units ahead
        let ahead = [p0[0] + 400.0, p0[1], p0[2]];
        let (tf, _) = g
            .projectile_ray(ahead, [ahead[0], ahead[1] - 500.0, ahead[2]])
            .expect("a floor ahead");
        let floor = [ahead[0], ahead[1] - 500.0 * tf, ahead[2]];
        let d = sub3(floor, origin);
        let l = len3(d);
        let dir = [d[0] / l, d[1] / l, d[2] / l];
        let (tb, _) = g
            .projectile_ray(
                origin,
                [
                    origin[0] + dir[0] * (l + 50.0),
                    origin[1] + dir[1] * (l + 50.0),
                    origin[2] + dir[2] * (l + 50.0),
                ],
            )
            .expect("the shot meets the floor");
        let burst = [
            origin[0] + dir[0] * (l + 50.0) * tb,
            origin[1] + dir[1] * (l + 50.0) * tb,
            origin[2] + dir[2] * (l + 50.0) * tb,
        ];
        // one enemy beside the burst point, one 200 units further, both standing beyond it
        let near = [burst[0] + 60.0, burst[1] + 50.0, burst[2]];
        let far = [burst[0] + 200.0, burst[1] + 50.0, burst[2]];
        g.players.get_mut(&1).unwrap().pos = near;
        g.players.get_mut(&2).unwrap().pos = far;
        g.take_out();
        let mut f = shot_at(weapon, ammo, origin, floor);
        f.dir = dir;
        g.on_up(0, Up::Fire(Box::new(f)), 4000);
        let mut now = 4000;
        while g.flights_in_air() > 0 && now < 6000 {
            now += 16;
            g.tick_projectiles(now);
        }
        let scale = g.cfg.projectile.blast_damage_scale;
        for (pid, pos) in [(1u8, near), (2, far)] {
            let want = area.damage
                * projectile::blast_factor(
                    len3(sub3(pos, burst)),
                    area.radius,
                    projectile::BLAST_INNER_FRACTION,
                )
                * scale;
            let lost = 100.0 - g.players[&pid].health;
            assert!(want > 0.0);
            assert!(
                (lost - want.min(100.0)).abs() < 0.5,
                "enemy {pid}: lost {lost:.2}, the row's falloff {want:.2}"
            );
        }
        assert!(
            100.0 - g.players[&1].health > 100.0 - g.players[&2].health,
            "the nearer enemy takes more"
        );
        let own = 500.0 - g.players[&0].health;
        assert!(
            g.players[&0].life == Life::Alive && own < area.damage * scale,
            "own {own}"
        );
    }
}
