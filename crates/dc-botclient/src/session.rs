//! The UDP game client, sans-IO: datagrams in with the caller's time, datagrams out, everything observed kept
//! in [`Observed`] and written to the [`Recorder`] event log.
//!
//! Client side of the session start-up (UDP_SESSION_2009_A.md §9.2 S1-S12, with the addenda): join ask every
//! 300 ms (transport §5.2), `C2S_HELLO` first, `C2S_FILE_ACK` for every `S2C_FILE_LIST`, `S2C_LOAD_WORLD` ->
//! "load" (no session message is processed meanwhile, §4.6) -> `C2S_LOAD_STATE(0)`, `S2C_PRELOAD` sub 1 ->
//! `C2S_LOAD_STATE(1)`, the in-world test of §8.3 (view object created, switching-worlds state 2 / 3, world
//! loaded, player state non-zero) -> `UP_WORLD_READY` 0x87 once. Game layer: server 0x72 -> `UP_PLAYER_INFO` 0x72
//! and `UP_CLIENT_OPTIONS` 0x85; player state 1 (shell 0x67) -> the body spawns and the unreliable player update
//! 0x64 goes out at up to 15 per second. `C2S_RATE` once the world is loaded.
// SPEC: UDP_SESSION_2009_A.md §2-§9 (+ addendum A7-A18), UDP_SESSION_2009_B.md §2-§12 (+ addendum B4-B18)
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1, §5.1, §5.2, §5.4, §5.6, Appendix A; GAME_MESSAGES_2009_S2C.md §3-§5

use std::collections::{BTreeMap, VecDeque};

use dc_net_engine::time::{later, since};
use dc_net_engine::{
    Bye, CloseCause, JoinStatus, Joiner, Link, LinkEvent, LinkStats, Millis,
    APP_ID_BIG_ENDIAN_CANDIDATE,
};
use dc_wire::udp::{BitReader, BitWriter, TODO_GAME_APP_ID_GUID_LAYOUT};
use serde_json::json;

use crate::hunt::Hunter;
use crate::lifecycle::{self, IdleWatch, LifeEvent, Respawn};
use crate::motion::{Body, MotionKind, BOT_STAND_DIMS};
use crate::objects::ObjectTable;
use crate::record::{head, Recorder};
use crate::romworld::{LoadedWorld, RomWorlds};
use crate::shell::{
    self, c2s, s2c, shell_id, IntegrityAsk, KillRecord, MoveReport, PlayerInfo, PlayerStatus,
    RosterRecord, SummaryRow, UpdateExtras,
};
use crate::shooter::{self, Action, Seen, Shooter, ShooterConfig, View};
use crate::weapon;

/// Rate the 2009 client asks with in `OOB_JOIN_ASK` (the CShell sets the client-rate variable at profile load).
// SPEC: UDP_TRANSPORT_2009_addendum_disasm.md T8 (rate_bps = 5120000)
pub const CLIENT_JOIN_RATE_BPS: u32 = 5_120_000;
/// The client-rate global the 2009 client reports in `C2S_RATE` (`rate_8k = 640`).
// SPEC: UDP_SESSION_2009_A_addendum_disasm.md A14 (rate_8k = 640 with the CShell rate)
pub const CLIENT_RATE_BPS: u32 = 5_120_000;
/// A link sends at 90 % of its requested rate.
// SPEC: UDP_TRANSPORT_2009_addendum_disasm.md T12 (set-rate: rate_bps = min(v * 90 / 100, rate_cap))
pub const RATE_PERCENT: u64 = 90;
/// Player updates per second for a remote server. The 2009 client variable default is 15; the bots send 30 like
/// dc-client (CLI-1578, BOT-96): at 15 every other 30 Hz stream tick repeated their position, a stop-go body on
/// every player's screen.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (R = 15.0, PE-DATA VA 0x374ce220)
// DC-DEFAULT: BOT-96 (proposed) 30 updates per second (the 2009 default is 15)
pub const UPDATE_HZ: f32 = 30.0;
/// Required engine protocol version in `S2C_VERSION`.
// SPEC: UDP_SESSION_2009_A.md §4.2 (net_version must be 7, else 0x4E)
pub const NET_VERSION: u32 = 7;
/// Session results the client ends with.
// SPEC: UDP_SESSION_2009_A.md §6.1 (0x4E version, 0x29 world file missing, 1 sky table count)
pub mod result {
    pub const VERSION_MISMATCH: u32 = 0x4E;
    pub const WORLD_MISSING: u32 = 0x29;
    pub const SKY_TABLE: u32 = 1;
}
/// Largest sky-table count the client accepts.
// SPEC: UDP_SESSION_2009_A.md §4.8 (count > 30 -> result 1)
pub const SKY_TABLE_MAX: u16 = 30;
/// Simulated world-load time (the 2009 client processes nothing while it loads; seconds on real hardware).
// DC-DEFAULT: BOT-6 (proposed) simulated world load 1000 ms
pub const DEFAULT_LOAD_MS: u32 = 1000;
/// How long a quitting client waits after `C2S_BYE` for the server to close the link before it sends the
/// transport bye itself.
// DC-DEFAULT: BOT-7 (proposed) 1000 ms
pub const QUIT_FLUSH_MS: u32 = 1000;
/// Kept entries of the arrival-order list of server messages.
pub const ORDER_KEEP: usize = 4000;
/// Most game events kept for a caller that has not taken them yet (the oldest are dropped first).
pub const EVENTS_KEEP: usize = 4096;
/// BOT-148: a hunt / roam goal, where the bot walks instead (`None`: it stays), when that was worked out, and whether
/// the bot got there.
type SpreadAside = ([f32; 3], Option<[f32; 3]>, Millis, bool);
/// BOT-148: how long a goal moved aside stays moved while the goal stays put.
pub const SPREAD_ASIDE_MS: u32 = 4000;
/// BOT-148: a goal no team mate stood at is looked at again after this long.
pub const SPREAD_ASIDE_RECHECK_MS: u32 = 1000;
/// BOT-148: the side place counts as reached within this (flat); the goal itself is walked to then.
pub const SPREAD_ASIDE_REACHED: f32 = 96.0;
/// How often a hunting bot logs its place and walk goal (`track`).
pub const TRACK_MS: u32 = 2000;
/// BOT-113: a walking body that moved less than [`WALK_CHECK_MIN`] units in this time jumps.
// DC-DEFAULT: BOT-113 (proposed) 1500 ms, 24 units
pub const WALK_CHECK_MS: u32 = 1500;
pub const WALK_CHECK_MIN: f32 = 24.0;
/// BOT-116: how often the evasive move's sides and back are probed for danger, and how far (seconds of running;
/// BOT-134: 0.8 s with every place along the way tested, `evade::PROBE_SECS`).
// DC-DEFAULT: BOT-116 (proposed) 250 ms; BOT-134 (proposed) 0.8 s
pub const EVADE_CHECK_MS: u32 = 250;
/// BOT-134: the body-progress watchdog: a living bot whose walk goal lies more than `WATCH_FAR` units away (flat) and
/// whose body stayed within `WATCH_MOVE` units (flat; in height too on a ladder) for `WATCH_MS`, whatever steers it
/// (its way, the greedy way-finder, a ladder), jumps (when the jump is safe) and plans again; a second trip at the
/// same place is a trap (BOT-133).
// DC-DEFAULT: BOT-134 (proposed) 96 units, 48 units (the stuck episode's radius: a body that bounces back and forth
// by 30..40 units, slid back along a slope or set back to one place, never got 24 units from a moving anchor), 3000 ms
pub const WATCH_FAR: f32 = 96.0;
pub const WATCH_MOVE: f32 = 48.0;
pub const WATCH_MS: u32 = 3000;
/// BOT-134: a pause in the watch shorter than this (a goal that is gone for a tick or two while the hunt goal changes
/// hands, a door check) keeps the anchor; a longer one starts the watch again.
// DC-DEFAULT: BOT-134 (proposed) 1500 ms
pub const WATCH_GAP_MS: u32 = 1500;
/// BOT-151: the Spy Hunt agent hurt within this long fights back while it walks to its waiting place (it backs
/// along its walk facing its target); it sprints to a goal farther than [`AGENT_SPRINT_MIN`] units (flat).
// DC-DEFAULT: BOT-151 (proposed)
pub const AGENT_HURT_MS: u32 = 1500;
pub const AGENT_SPRINT_MIN: f32 = 150.0;
/// BOT-151: within this many units (flat) of the uplink the agent that set off makes no evasive moves (the box is
/// 48 units across its middle at the least, DSV-323); it fights the hunter nearest it, kept while that one stays
/// within [`AGENT_FOCUS_KEEP`] of the nearest one's distance.
pub const AGENT_BOX: f32 = 150.0;
pub const AGENT_FOCUS_KEEP: f32 = 1.6;
/// BOT-155: a chased player within this many units (flat) counts as caught up with (no stall).
// DC-DEFAULT: BOT-155 (proposed)
pub const CHASE_CLOSE: f32 = 300.0;
/// BOT-117: a roaming bot's goal is a grid node at least this far away (flat), kept this long or until the bot is
/// this near it.
// DC-DEFAULT: BOT-117 (proposed) 800 units, 20000 ms, 96 units
pub const ROAM_MIN: f32 = 800.0;
pub const ROAM_MS: u32 = 20_000;
pub const ROAM_REACHED: f32 = 96.0;
/// BOT-145: a bot that keeps getting stuck at one place gives its walk goal up for a while: the third watchdog trip or
/// stuck re-plan within [`LOST_NEAR`] (flat) of the first and within [`LOST_WINDOW_MS`] of it, or a walk at the end of
/// a way short of the goal (or on an island of the grid) that lasted [`LOST_EDGE_MS`]: for [`LOST_MS`] (or until it
/// gets within [`LOST_REACHED`]) it walks to a grid node it has a way to, at least [`LOST_MIN`] away (else the farthest
/// such node; else back along the way its body came). Only a hunt, idle or roam goal is given up (an objective, a
/// flag, a bomb walk keeps its goal). A human on a ledge with no way down walks off to find the stairs; before, the
/// bots paced 24..146 s (Snow Valley's rock, Cold Seed's spawn ledge, Two Towers above and below team 2's spawn).
// DC-DEFAULT: BOT-145 (proposed) 128 units, 30 s, 3 trips, 8 s, 15 s, 600 units
pub const LOST_NEAR: f32 = 128.0;
pub const LOST_WINDOW_MS: u32 = 30_000;
pub const LOST_TRIPS: u32 = 3;
pub const LOST_EDGE_MS: u32 = 8_000;
/// BOT-159: how long a bot walks (and jumps) along the way off an island of the grid (`evade::ESCAPE_SECS`).
// DC-DEFAULT: BOT-159 (proposed) 2000 ms
pub const ESCAPE_MS: u32 = (crate::evade::ESCAPE_SECS * 1000.0) as u32;
/// BOT-159: a bot on a small island of the grid whose goal lies off it (no node of the island within
/// [`CONFINED_GOAL`], flat) for [`CONFINED_MS`] takes a way off it; the island is looked at every
/// [`CONFINED_CHECK_MS`].
// DC-DEFAULT: BOT-159 (proposed) 10 s, 1 s, 96 units
pub const CONFINED_MS: u32 = 10_000;
pub const CONFINED_CHECK_MS: u32 = 1000;
pub const CONFINED_GOAL: f32 = 96.0;
/// BOT-159: the fall of a way off an island leaves the bot at least this much health by the server's fall damage
/// (DSV-76 defaults: none up to 300, then 1 per 2 units): the highest fall is `300 + 2 x (health - ESCAPE_HEALTH_LEFT)`,
/// at most `nav::ESCAPE_DROP`. Before, a bot hit to 30 health died of a 374-unit fall off Rattlesnake's deck.
// DC-DEFAULT: BOT-159 (proposed) 25 health, 300 units, 0.5 per unit
pub const ESCAPE_HEALTH_LEFT: f32 = 25.0;
pub const FALL_SAFE: f32 = 300.0;
pub const FALL_DAMAGE_PER_UNIT: f32 = 0.5;
pub const LOST_MS: u32 = 15_000;
pub const LOST_MIN: f32 = 600.0;
/// BOT-145: the place walked to instead counts as reached this near (flat; a place back along the trail lies 64 away).
// DC-DEFAULT: BOT-145 (proposed) 32 units
pub const LOST_REACHED: f32 = 32.0;
/// BOT-145: the greedy way-finder's probe is tested for danger at this many places along its walk (before: its end
/// only). On Two Towers 14 of 19 world deaths of three rooms were greedy walks from the end of a partial way on the
/// walks over the water (one lattice cell wide), where a walk crosses the edge and ends on the next walk.
// DC-DEFAULT: BOT-145 (proposed) 4 places
pub const GREEDY_PROBE_SAMPLES: usize = 4;
/// BOT-153: a stuck jump of the follower or of the greedy way-finder is taken only when the body is blocked by the world:
/// its walk run on a copy for this long moves less than this (flat); a face jump not while a team mate stands this
/// near the top. Before, 1 in 2 of these jumps had a next point no higher than a step (the body slowed by a mate, a
/// slope, the server), and 30..50 % of the face jumps left the body where it was (TDM Two Towers, Rattlesnake, TSV Gray
/// Hammer rooms of 2026-11-08).
// DC-DEFAULT: BOT-153 (proposed) 0.25 s, 8 units, 40 units
pub const BLOCK_PROBE_SECS: f32 = 0.25;
pub const BLOCK_MOVE: f32 = 8.0;
pub const MATE_ON_TOP: f32 = 40.0;
/// BOT-143: the probes the grid grows by when no node lies [`ROAM_MIN`] away, before the farthest known node is
/// the roam goal.
// DC-DEFAULT: BOT-143 (proposed) 64 probes
pub const ROAM_GROW: usize = 64;
/// BOT-136: the posts round the sites are kept this long (this long while a site has fewer than three, when the
/// grid also grows by this many probes toward it); the view turns toward the watched place at this rate (rad/s).
// DC-DEFAULT: BOT-136 (proposed) 15 s / 3 s, 48 probes, 3 rad/s
pub const POSTS_KEEP_MS: u32 = 15_000;
pub const POSTS_THIN_MS: u32 = 3000;
pub const POSTS_GROW: usize = 48;
pub const WATCH_TURN: f32 = 3.0;
/// BOT-136: a defuse hold counts as ending in time when it ends this long before the blast (the 0x51 reaches the
/// bot a little after the server's plant).
pub const HOLD_DONE_SLACK_MS: u32 = 300;

/// BOT-136: the posts round the sites as last worked out.
#[derive(Debug, Clone)]
struct PostCache {
    round: u32,
    at: Millis,
    sites: Vec<[f32; 3]>,
    posts: Vec<Vec<[f32; 3]>>,
}
pub const EVADE_PROBE_SECS: f32 = crate::evade::PROBE_SECS;
/// BOT-121: a bot farther than this (flat) from where it stood at its last navigation call was moved (a spawn, a
/// teleport): its way starts afresh.
// DC-DEFAULT: BOT-121 (proposed) 256 units
pub const NAV_TELEPORT: f32 = 256.0;
/// BOT-121: at the end of a way short of its goal (or on an island of the grid) the greedy way-finder walks from that
/// place for this long (BOT-113's edge walk), and a plan that leads back within `NAV_END_NEAR` of the place is not
/// taken meanwhile.
// DC-DEFAULT: BOT-121 (proposed) 3000 ms, 96 units
pub const NAV_END_GREEDY_MS: u32 = 3000;
pub const NAV_END_NEAR: f32 = 96.0;

/// BOT-121: the time a bot spends in its navigation (microseconds): in all, the most in one navigation step, the
/// steps (`dc_nav::follow::STEP_MS` each) and the time of the step under way.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NavCost {
    pub busy_us: u64,
    pub step_max_us: u64,
    pub steps: u64,
    step_us: u64,
}

/// A world load to run on a worker thread.
type WorldJob = Box<dyn FnOnce() -> Result<LoadedWorld, String> + Send>;

/// Settings of one game client.
#[derive(Debug, Clone)]
pub struct GameConfig {
    /// UTF-16 name of the hello blob (the lobby nickname; the game server identifies the player by it, CD-60).
    pub nick: String,
    /// The second 8-bit string of the hello blob (meaning UNPROVEN; the bot sends its account id).
    // DC-DEFAULT: BOT-8 (proposed) hello blob offset 0x40 = the bot's account id text
    pub hello_tag: String,
    /// The seat ticket the lobby gave for this match (`seatticket`); `None`: the hello carries none.
    // DC-DEFAULT: LG-327 (proposed, project decision)
    pub seat_ticket: Option<dc_wire::seatticket::Ticket>,
    /// Application ids tried in order: the next one after a join reply "wrong application" (0x52).
    // DC-DEFAULT: BOT-5 (proposed) try the Windows GUID record layout first, then the big-endian layout
    pub app_ids: Vec<[u8; 16]>,
    pub join_rate_bps: u32,
    /// How long the join ask is repeated before the start fails with 0x4F. The 2009 client's 10 s by default; a
    /// bot against a server on a starved machine passes a longer limit (`--join-timeout`).
    // DC-DEFAULT: BOT-50 (proposed) default 10000 ms (the 2009 value), configurable, see DC_DEFAULTS.md
    pub join_timeout_ms: u32,
    pub client_rate_bps: u32,
    pub load_ms: u32,
    pub update_hz: f32,
    pub sim_hz: u32,
    pub motion: MotionKind,
    /// Send player updates once the engine part of the in-world test holds even without player state 1 (not
    /// 2009 behaviour; lets a test exercise a server that never sends shell 0x67).
    pub force_updates: bool,
    /// Value of the reliable 0x64 state form sent once at the first spawn (`None` = not sent; the controller
    /// state byte it carries is UNPROVEN).
    pub state_form: Option<u8>,
    pub info: PlayerInfo,
    pub client_options: (u8, bool),
    /// Send the aim rotation (`pu_flags` 0x0100, `quat128`) in a player update whenever it changed since the
    /// last one (what sets the client's condition flag +0x455 is UNPROVEN).
    // DC-DEFAULT: BOT-9 (proposed) aim rotation on change
    pub aim_quat: bool,
    /// The u16 of the integrity answer (read from the user's CShell.dll; 0 when unknown).
    pub integrity_version: Option<u16>,
    /// A world file on disk used instead of the ROM lookup (a server started with a world file).
    pub world_file: Option<std::path::PathBuf>,
    /// A folder of unpacked packs, where a community map's `@mod:` announce is looked up (`--mod-cache`, BOT-101).
    pub mod_cache: Option<std::path::PathBuf>,
    /// The caller simulates the player: no bot body is created, the caller hands every frame's update to
    /// [`BotSession::set_player_update`] and applies the server's teleports itself (taken with
    /// [`BotSession::take_events`]). Off for the bot.
    pub external_body: bool,
    /// The caller loads the announced world (for example with its renderer): after `S2C_LOAD_WORLD` the session waits,
    /// besides `load_ms`, until [`BotSession::world_loaded`] or [`BotSession::world_failed`]. Off for the bot.
    pub external_load: bool,
    /// Keep a queue of game events ([`GameEvent`]) for the caller. Off for the bot.
    pub queue_events: bool,
    /// End the session when no datagram came from the server for this long (0 = never).
    // DC-DEFAULT: BOT-10 (proposed) 15000 ms
    pub idle_exit_ms: u32,
    /// Fallback respawn request this long after a death even when the server never armed the client's wait (not
    /// 2009 behaviour; `None` = only the armed wait asks).
    pub respawn_after_ms: Option<u32>,
    /// Shooter mode (`None` = hold fire: the bot never fires).
    pub shooter: Option<ShooterConfig>,
    /// Leave the game server after the result screen message 0x3f, as the 2009 client does.
    pub leave_after_result: bool,
    /// The mode class of the room's mode row (the 2009 client derives it from the room mode; `GS_GAME_ENDPOINT`
    /// 0x9e, when it arrives, takes precedence).
    // SPEC: GAME_MESSAGES_2009_C2S.md Appendix A step A0 (mode_class from the room's mode row at run time)
    pub mode_class_hint: Option<u8>,
    /// Ask for roster records the way the 2009 client does: a `GS_PLAYER_INFO` 0x6b whose `character` object the
    /// client does not have yet queues that player id, and the roster tick (every [`ROSTER_TICK_MS`]) sends one
    /// `UP_ROSTER_QUERY` 0x8c per queued id. Off for the bot.
    // SPEC: GAME_MESSAGES_2009_C2S.md §5.5 (cshell+0x2b170 sends one query per queued id and clears the list)
    // OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.15 (0x8c [pid 0] about 1 s after a 0x6b naming an object not yet
    // created), CAPTURE_tdm4_KILL_VALIDATION.md §2 (two queries in one tick)
    pub roster_query: bool,
    /// Scripted timeline (`--timeline`, [`crate::timeline`]): held fire, target choice, walk goals, throws and the
    /// leave time on the bot's own match clock (`None` = the bot plays on its own).
    pub timeline: Option<crate::timeline::Timeline>,
    /// The in-game frame ends at the result screen 0x3f, as the 2009 client's does: from then on no player update,
    /// no respawn request and no roster query go out (the session still answers the transport and leaves when
    /// asked). Off for the bot (its leave rules stay BOT-16 / BOT-44).
    // DISASM: cshell+0x4ac5c (0x3f switches the interface to screen 7 through cshell+0x1bb140, leaving the playing
    // state whose frame runs the local player); the respawn sender cshell+0x61e40 itself has no round gate
    // (cshell+0x62038..0x620db: mode class, life state, armed byte, elapsed wait)
    // OBSERVED: captures 20261025_125432_rust6, 20261025_153705_rust7, 20261025_204341_realclient (the real
    // client's last 0x64 at most 63 ms before 0x3f, no 0x64 / 0x89 / 0x8c after it)
    pub frame_ends_at_result: bool,
    /// BOT-120: the bot opens the doors on its way with the use key and its grid takes doors as ways (false: doors
    /// are walls, as before; env `DC_BOT_DOORS=off`).
    pub doors: bool,
}

/// Period of the client's roster tick that sends the queued roster queries.
// DC-DEFAULT: CLI-22 (proposed) 1000 ms; OBSERVED: tdm1 (query 1.0 s after the record), tdm4 (0.45 s)
pub const ROSTER_TICK_MS: u32 = 1000;

impl GameConfig {
    /// 2009-client defaults for a player named `nick`.
    pub fn new(nick: &str) -> Self {
        GameConfig {
            nick: nick.to_string(),
            hello_tag: String::new(),
            seat_ticket: None,
            app_ids: vec![TODO_GAME_APP_ID_GUID_LAYOUT, APP_ID_BIG_ENDIAN_CANDIDATE],
            join_rate_bps: CLIENT_JOIN_RATE_BPS,
            join_timeout_ms: dc_wire::udp::JOIN_TIMEOUT_MS,
            client_rate_bps: CLIENT_RATE_BPS,
            load_ms: DEFAULT_LOAD_MS,
            update_hz: UPDATE_HZ,
            sim_hz: dc_sim::DEFAULT_TICK_HZ,
            motion: MotionKind::Stand,
            force_updates: false,
            state_form: None,
            info: PlayerInfo {
                team: 1,
                ..PlayerInfo::default()
            },
            client_options: (0, false),
            aim_quat: true,
            integrity_version: None,
            world_file: None,
            mod_cache: None,
            external_body: false,
            external_load: false,
            queue_events: false,
            idle_exit_ms: lifecycle::DEFAULT_IDLE_EXIT_MS,
            respawn_after_ms: None,
            shooter: None,
            leave_after_result: true,
            mode_class_hint: None,
            roster_query: false,
            timeline: None,
            frame_ends_at_result: false,
            // DC-DEFAULT: BOT-120 (proposed) on; `DC_BOT_DOORS=off` keeps doors as walls (an A/B knob)
            doors: !matches!(
                std::env::var("DC_BOT_DOORS").as_deref(),
                Ok("off" | "0" | "false")
            ),
        }
    }
}

/// A game-layer event for a caller with its own player simulation and HUD ([`GameConfig::queue_events`]). Each one
/// is a server message the 2009 client acts on, in arrival order.
#[derive(Debug, Clone, PartialEq)]
pub enum GameEvent {
    /// `GS_MOVER_UPDATE` (0xa0) flag 0x0020: new `move_seq`, position and box.
    // SPEC: MOVEMENT_2009.md §12.4 (flag 0x0020)
    Teleport {
        seq: u8,
        pos: [f32; 3],
        dims: [f32; 3],
    },
    /// The other blocks of a `GS_MOVER_UPDATE` (friction, freeze, release), when present.
    // SPEC: MOVEMENT_2009.md §12.4 (flags 0x0004 / 0x0008, 0x0080, 0x0100)
    Mover {
        friction: Option<f32>,
        freeze: Option<bool>,
        release: Option<bool>,
    },
    /// `GS_VIEW_ANGLES` (0x6a): kind 0 = pitch, yaw, roll; kind 1 = yaw only.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.2; MOVEMENT_2009.md §12.5
    ViewAngles { kind: u8, angles: [f32; 3] },
    /// `GS_PLAYER_STATE` (0x67).
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.1
    PlayerState(u8),
    /// `GS_PLAYER_STATUS` (0x66).
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.3
    Status(PlayerStatus),
    /// `GS_KILL` (0x6d).
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1
    Kill(KillRecord),
    /// `GS_TEAM_SCORE` (0x4a).
    TeamScore([u8; 2]),
    /// `GS_MATCH_BEGIN` (0x45) head.
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.1.1
    MatchBegin {
        round_secs: u32,
        waiting_banner: bool,
    },
    /// `GS_ROUND_BEGIN` (0x44).
    RoundBegin {
        round_secs: u32,
        round_no: u32,
        round_limit: u32,
    },
    /// `GS_ROUND_TIMER_STOP` (0x41).
    RoundTimerStop(u32),
    /// `GS_ROUND_RESULT` (0x46) head.
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.1.2
    RoundResult { win_team: i8, wins: [u8; 2] },
    /// `GS_ROUND_WINS` (0xb5).
    RoundWins([u8; 2]),
    /// `GS_PLAYER_SUMMARY` (0x65).
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.4
    Summary(SummaryRow),
    /// `GS_MATCH_RESULT_SCREEN` (0x3f): the client opens its result screen.
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (0x3f, screen 7)
    ResultScreen { flag: bool, value: u32 },
    /// `GS_LEVEL_EXIT` (0x99): an empty `next_world` with `now` takes the client back to its menus.
    // SPEC: GAME_MESSAGES_2009_S2C.md §6.1
    LevelExit {
        next_world: String,
        keep: bool,
        now: bool,
    },
    /// `GS_WEAPON_GIVE` (0xb2) / `GS_WEAPON_EQUIP` (0xb3): a weapon in an inventory slot (`equip`: the drawn one).
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.4
    WeaponGrant(crate::weapon::WeaponGrant),
    /// `GS_WEAPON_REMOVE` (0xb4): the weapon leaves its slot.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.4
    WeaponRemove { slot: u8 },
    /// `GS_WEAPON_SWITCH` (0x7f): the server names a weapon by command number and (with `auto`) draws it.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (`[command:u8][auto:u8][ammo:f32][last:u8]`)
    WeaponSwitch {
        command: u8,
        auto: bool,
        ammo: f32,
        last: u8,
    },
    /// `GS_DAMAGE_TAKEN` (0x74).
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.3
    Damage(crate::weapon::DamageTaken),
    /// An engine id 17 effect that names a sound: the player sound record (type 0x19) or the shot effect (type 0x06).
    // SPEC: GAME_MESSAGES_2009_S2C.md §8.1, §8.2 (types 0x19, 0x06)
    Effect(crate::fx::Effect),
    /// A shell message of [`RAW_SHELL_EVENTS`], whole (after the shell id), for a caller that reads it itself (the
    /// client HUD: intro block, round result, HiSec case, co-op clock, object notes).
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.1, §3.4, §3.5, §8.3 (layouts read by the caller)
    Shell {
        shell: u8,
        body: Vec<u8>,
        bits: usize,
    },
}

/// Shell ids that are also handed to the caller whole as [`GameEvent::Shell`] (in addition to any event above):
/// 0x3f result screen, 0x45 match begin, 0x46 round result, 0x47 round objects end, 0x48 round close, 0x68 chat
/// line, 0x6c player left (its `kicked` byte; the roster side stays here), 0x77 overlay, 0x7d match settings, 0xa2
/// ping table (the board's ping column, CLI-1360), 0xa6 object notes, 0xc8 multi-kill, 0xd6 HiSec case, 0xd7 co-op, 0xd9 music order, 0xda player points (the client's
/// shot counter, WEAPON_FIRE_2009_addendum_cone.md AC-1.5), and the mode messages the client HUD reads: 0x4f / 0x50 /
/// 0x51 / 0x56 / 0xad Search and Destroy, 0xc6 flag state, 0xc9 Fire Team mission (goal list, banner), 0xcb Spy Hunt,
/// 0xd3 Quarantine; 0xcd the radar switch (the answer to the heartbeat sensor's switch, CLI-1912); and our own 0xf1, other players' health in a match with wallhacks (DSV-480,
/// `dc_wire::remotehealth`).
// DC-DEFAULT: BOT-95 (proposed, playtest report "no flag sounds, no visual when you grab the flag") the mode shells
// were parsed by the HUD (hud::msgs::read_shell) but never handed over, so no notice, sound or counter followed them
// SPEC: GAME_MESSAGES_2009_S2C.md Â§2.1, Â§2.4 (0x68), Â§3.2 (0x6c), Â§3.3, Â§3.5, Â§4.2 (0xd9), Â§8.3, 0x77 row, 0xda row
pub const RAW_SHELL_EVENTS: [u8; 28] = [
    0x3f,
    0x45,
    0x46,
    0x47,
    0x48,
    0x4f,
    0x50,
    0x51,
    0x56,
    0x68,
    0x6c,
    0x77,
    0x7d,
    0xa2,
    0xa6,
    0xad,
    0xc6,
    0xc8,
    0xc9,
    0xcb,
    0xcd,
    0xd3,
    0xd6,
    0xd7,
    0xd9,
    0xda,
    dc_wire::remotehealth::SHELL_ID,
    dc_wire::possession::SHELL_ID,
];

/// Client states (UDP_SESSION_2009_A.md §8.1 names).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Joining,
    Linked,
    LoadingWorld,
    WorldLoaded,
    InWorld,
    /// The engine start failed (join timeout 0x4F, refused 0x51, wrong app 0x52).
    Failed(u32),
    /// The session ended: a handler error code, or the transport drop code.
    Dropped(u32),
    /// The bot quit and the link is gone.
    Closed,
}

/// Everything the client observed (for the run report and assertions).
#[derive(Debug, Clone, Default)]
pub struct Observed {
    /// Milestones in the order reached, with the time since the first poll.
    pub milestones: Vec<(String, u32)>,
    /// Server messages in arrival order: (ms since start, reliable, msg id, shell id).
    pub s2c_order: Vec<(u32, bool, u8, Option<u8>)>,
    pub s2c_counts: BTreeMap<u8, u64>,
    pub shell_counts: BTreeMap<u8, u64>,
    pub c2s_counts: BTreeMap<u8, u64>,
    pub c2s_shell_counts: BTreeMap<u8, u64>,
    pub version: Option<(u32, u32)>,
    pub client_id: Option<(u16, u8)>,
    /// File table: id -> (file_info, name).
    pub files: BTreeMap<u16, (u32, String)>,
    pub load_world: Option<(f32, u16)>,
    /// The world file name and how it resolved in the ROM (`None` = no ROM to check against).
    pub announced_world: Option<String>,
    pub world_resolved: Option<Result<String, String>>,
    pub evars: Vec<(String, String)>,
    pub preload_subs: Vec<u8>,
    /// The id lists of the `S2C_PRELOAD` list kinds (sub, ids), in arrival order.
    // SPEC: UDP_SESSION_2009_A.md §4.7 (subs 2..6 carry `u16` values until the end of the message)
    pub preload_lists: Vec<(u8, Vec<u16>)>,
    pub sky_count: Option<u16>,
    pub view_object: Option<u16>,
    pub switch_states: Vec<u8>,
    pub life_states: Vec<u8>,
    pub mode_class: Option<u8>,
    pub roster: BTreeMap<u8, RosterRecord>,
    /// Roster records of players who left (0x6c).
    pub departed: Vec<RosterRecord>,
    pub console: Vec<String>,
    pub integrity: Vec<String>,
    pub teleports: Vec<(u8, [f32; 3], [f32; 3])>,
    pub kills: Vec<(u8, u8)>,
    pub updates_sent: u64,
    /// BOT-134: player updates sent with the jump bit, the body's take-offs, the watchdog's trips.
    pub jump_inputs: u64,
    pub jumps_taken: u64,
    pub watchdog_trips: u64,
    /// BOT-153: stuck jumps not taken (the body free in the world, a team mate on the top).
    pub jumps_skipped: u64,
    /// BOT-159: ways off a small island of the grid taken.
    pub escapes: u64,
    /// Gaps between two consecutive player updates on the session clock: gap (ms) -> count. Tests check the send
    /// gate with it on the session's own clock (a busy machine stretches gaps; the gate must never shorten one).
    pub update_gaps: BTreeMap<u32, u32>,
    /// The last `GS_SPRINT_ALLOWED` value (`None`: none received yet).
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.2
    pub sprint_allowed: Option<bool>,
    pub app_id_index: Option<usize>,
    pub join_asks: u32,
    /// Errors that end the session of a 2009 client (the bot reports them and goes on where it can).
    pub fatal: Vec<String>,
    /// Messages the bot could not parse completely (not necessarily fatal for a 2009 client).
    pub parse_notes: Vec<String>,
    pub end: Option<String>,
    /// The transport bye code of a server that closed the session (w2-loading: the client's disconnect box).
    pub server_bye: Option<u32>,
    /// The 48-byte block of `S2C_SKY_TABLE` and its object ids (at most 30), for the sky pass.
    // SPEC: UDP_SESSION_2009_A.md §4.8; EVIDENCE: engine+0xa44f0 (DISASM: block copied to the client state, ids into
    // the 30-entry table)
    pub sky_block: Option<[u8; 48]>,
    pub sky_ids: Vec<u16>,
}

impl Observed {
    pub fn reached(&self, m: &str) -> bool {
        self.milestones.iter().any(|x| x.0 == m)
    }
}

/// The game client.
pub struct BotSession {
    /// The play-session pause switch (BOT-59).
    control: crate::control::Control,
    cfg: GameConfig,
    rom: Option<RomWorlds>,
    rec: Recorder,
    stage: Stage,
    joiner: Joiner,
    app_idx: usize,
    link: Option<Link>,
    start: Option<Millis>,
    pub obs: Observed,
    pub table: ObjectTable,
    world: Option<LoadedWorld>,
    /// The world file being parsed on a worker thread (name for logs, result channel).
    world_job: Option<(
        String,
        std::sync::mpsc::Receiver<Result<LoadedWorld, String>>,
    )>,
    world_loaded: bool,
    load_done_at: Option<Millis>,
    held: VecDeque<(bool, BitWriter)>,
    switch_state: Option<u8>,
    life_state: u8,
    rate_sent: bool,
    world_ready_sent: bool,
    body: Option<Body>,
    body_clock: Millis,
    next_update: Millis,
    state_form_sent: bool,
    last_aim: Option<[f32; 4]>,
    /// When the last player update went out (session clock).
    last_update_at: Option<Millis>,
    /// The (yaw, pitch) the body showed in the last game tick and when (the start of every skill-mode turn).
    facing: Option<(f32, f32)>,
    facing_at: Millis,
    pending_teleport: Option<(u8, [f32; 3], [f32; 3])>,
    /// A Possession move runs (BOT-100).
    gliding: bool,
    pending_angles: Option<[f32; 3]>,
    quit_at: Option<Millis>,
    /// The server closed the link after our `C2S_BYE`: the plain bye burst follows.
    plain_bye_due: bool,
    /// The caller's latest player update ([`GameConfig::external_body`]).
    external: Option<MoveReport>,
    /// The optional 0x64 blocks of the caller's latest update ([`BotSession::set_player_update_ext`]).
    external_extras: Option<UpdateExtras>,
    /// The camera point of the last update that carried one (0x0400 goes out only when it moved).
    last_cam: Option<[f32; 3]>,
    /// The weapon float of the last update that carried one (0x0200 goes out only when it changed).
    last_weapon_f: Option<f32>,
    /// `UP_LEAVE_MATCH` 0xbd was sent.
    left_match: bool,
    /// Player ids queued for a roster query ([`GameConfig::roster_query`]) and the next roster tick.
    roster_pending: Vec<u8>,
    roster_tick_at: Option<Millis>,
    /// The caller's player spawned (first player state 1 since the world load).
    external_spawned: bool,
    /// Waiting for the caller's world load ([`GameConfig::external_load`]).
    awaiting_world: bool,
    events: VecDeque<GameEvent>,
    /// Link statistics extremes over the session.
    pub max_outstanding: usize,
    pub paused_seen: u64,
    pub stalled_seen: u64,
    /// Datagrams the runner dropped on purpose (out, in), see `run::Impairment`.
    pub dropped: (u64, u64),
    last_stats_log: Millis,
    respawn: Respawn,
    idle: IdleWatch,
    /// Shooter mode state (`None` = hold fire).
    pub shooter: Option<Shooter>,
    /// When the bot leaves after the result screen.
    leave_at: Option<Millis>,
    /// The way-finder of `MotionKind::Hunt`.
    pub hunter: Hunter,
    /// BOT-92: the evasive movement while engaging.
    pub evader: crate::evade::Evader,
    /// BOT-93: the navigation grid of the loaded world; BOT-121: the way the bot follows on it (the co-op AI's
    /// follower), when its last navigation step began, where the bot stood at its last call, the time spent.
    pub nav: Option<crate::nav::NavGrid>,
    pub follow: crate::nav::follow::Follower,
    nav_step: Millis,
    nav_last_pos: Option<[f32; 3]>,
    pub nav_cost: NavCost,
    /// BOT-121: where and since when the bot walks greedily from the end of a way short of its goal.
    nav_end: Option<(Millis, [f32; 3])>,
    /// BOT-121: the goal of the last navigation step and where it said to go (asked again at the next step).
    nav_cache: Option<([f32; 3], crate::nav::follow::NavGoal)>,
    /// BOT-121: the goal the bot stands at because the body got no closer to it at the end of a whole way.
    nav_arrived: Option<[f32; 3]>,
    nav_said: Millis,
    /// BOT-113: where the greedy way-finder's walking body was at the last progress check, and when.
    walk_check: Option<(Millis, [f32; 3])>,
    /// BOT-117: the place the bot roams to and when it was picked; the picks' counter.
    roam: Option<([f32; 3], Millis)>,
    roam_seed: u64,
    /// BOT-130: since when the bot is the last one alive of its side in an elimination round (`None`: it is not).
    cautious_since: Option<Millis>,
    /// BOT-145: the place of the first of the stuck events counted, since when, how many; the goal walked to instead
    /// of the one given up and since when; since when the bot walks at the end of a way short of its goal.
    trouble: Option<([f32; 3], Millis, u32)>,
    lost: Option<([f32; 3], Millis)>,
    edge_since: Option<Millis>,
    /// BOT-159: the way off a small island of the grid the bot takes and since when.
    escape: Option<(crate::evade::Escape, Millis)>,
    escape_jumped: bool,
    /// BOT-159: since when the bot stands on a small island of the grid with its goal off it, and when that was last
    /// looked at.
    confined_since: Option<Millis>,
    confined_check: Millis,
    /// BOT-159: the own health as the server last sent it (0x66 kind 2).
    health: Option<f32>,
    escape_trail: Option<(Millis, [f32; 3])>,
    /// BOT-116: when the evasive move's sides and back were last probed, and whether each is safe; BOT-134: when the
    /// body was last tested for a kill volume's edge beside it, and the way away from it.
    evade_safe: (Millis, crate::evade::Safety),
    /// BOT-170: whether the push of the evasive move was safe at the last probe (probed only while one is due).
    push_safe: bool,
    /// BOT-145: the step inward from a deck's edge in force (until when, the walking direction).
    inside_step: Option<(Millis, Option<f32>)>,
    edge: (Millis, Option<f32>),
    /// BOT-134: the watchdog's anchor (since when the body stands within `WATCH_MOVE` of where), the arm that
    /// steered last (`track.branch`), whether the body was in a jump at the last tick (take-offs are counted).
    watch: Option<(Millis, [f32; 3])>,
    watch_seen: Millis,
    goal_seen: Millis,
    branch: &'static str,
    was_jumping: bool,
    /// BOT-111: the kill volumes the server created (the grid keeps its nodes out of them, the greedy way-finder
    /// turns away from them).
    kill_now: Vec<dc_sim::ConvexVolume>,
    /// BOT-155: the kill volumes without the hazard volumes, and whether the body stands in danger only by a
    /// hazard volume (then its moves are judged by the kill volumes alone, so it can walk and jump out).
    kill_only: Vec<dc_sim::ConvexVolume>,
    in_hazard: bool,
    /// BOT-120: the door logic, and the grid's solid set (which linked models the grid takes as solid) last built.
    doors: crate::doors::DoorMind,
    nav_key: Option<Vec<bool>>,
    doors_said: Millis,
    /// When the last `track` event was logged.
    track_said: Millis,
    /// BOT-95: the bot's role in the objective modes.
    pub role: crate::roles::Role,
    /// The goal the way-finder last walked to (a far new goal forgets the visited places).
    hunt_goal_last: Option<[f32; 3]>,
    /// BOT-147: the bot waits where it is for its own live grenade to go off.
    blast_wait: bool,
    /// BOT-148: the side step away from a team mate while fighting.
    spread_step: crate::spread::Step,
    /// BOT-148: the short wait behind a team mate right ahead.
    spread_trail: crate::spread::Trail,
    /// BOT-148: the clear place a blocked side step walks to, and when it was picked.
    spread_space: Option<([f32; 3], Millis)>,
    /// BOT-148: the last hunt / roam goal moved aside: the goal, where the bot walks instead (`None`: it stays), when
    /// it was worked out, and whether the bot got there (then the goal itself again until it moves).
    spread_aside: Option<SpreadAside>,
    /// BOT-103: the objective the bot stands at (it reached it).
    stood_at: Option<[f32; 3]>,
    /// The flag team the bot carries in a flag mode (server 0xc6, BOT-66).
    carrying: Option<u32>,
    /// BOT-125: the flag teams whose flag is off its base (0xc6 taken / dropped until returned / captured), where
    /// each flag was last seen at its base, and the bot's flag job as last logged.
    flag_away: std::collections::BTreeSet<u32>,
    flag_home: BTreeMap<u32, [f32; 3]>,
    flag_job: Option<crate::roles::FlagJob>,
    /// BOT-138: when each loose (dropped) flag fell, who carries each flag, the watch over the flag walk (and the
    /// flag team it walks to this tick), and the flag last found out of reach.
    flag_loose: BTreeMap<u32, Millis>,
    flag_carrier: BTreeMap<u32, u32>,
    flag_walk: crate::flagrun::FlagWalk,
    flag_walk_team: Option<u32>,
    flag_unreach: Option<(u32, [f32; 3], Millis)>,
    /// BOT-138: a defender's guard place and the turn of the ring it was picked for.
    flag_guard_at: Option<(u32, [f32; 3])>,
    /// BOT-144: since when the last guard has stood its post with no enemy near the flag, and whether it attacks.
    flag_quiet_since: Option<Millis>,
    flag_roving: bool,
    /// Spy Hunt: the cases the bot holds of how many (BOT-75).
    spy: crate::spyrun::SpyRun,
    /// BOT-137: the place a Spy Hunt holder falls back to from a near hunter and when it was picked.
    spy_retreat: Option<([f32; 3], Millis)>,
    /// BOT-151: the agent's waiting place while the uplink is shut and when it was picked; whether it set off for the
    /// uplink; a guard's approach (number, place or none found, when worked out); when the bot was last hurt.
    spy_lurk: Option<([f32; 3], Millis)>,
    spy_set_off: bool,
    spy_way: Option<(Option<f32>, Millis)>,
    /// BOT-152: since when the holder one case short has been at it, and its place off the last case.
    spy_fifth: Option<Millis>,
    spy_fifth_place: Option<([f32; 3], Millis)>,
    spy_fifth_try: Option<Millis>,
    /// BOT-155: the chase watched for headway (pid, since when, where the body was) and since when it has no way.
    spy_chase_watch: Option<(u32, Millis, [f32; 3])>,
    spy_no_way: Option<Millis>,
    /// BOT-151: the hunter the agent fights (its preferred target) and since when it was last near.
    spy_focus: Option<(u8, Millis)>,
    spy_guard: Option<(usize, Option<[f32; 3]>, Millis)>,
    /// BOT-164: the player the chaser fights from where it stands (in clear sight), none while it walks its chase.
    spy_engaged: Option<u32>,
    last_hurt: Option<Millis>,
    /// BOT-172: the knife run's session side (start, end, last contact, team mate killed near).
    knife: crate::shooter::knife::KnifeState,
    /// BOT-128: the place a Quarantine human falls back to and when it was picked; whether it walks back to the
    /// other humans.
    qrt_retreat: Option<([f32; 3], Millis)>,
    /// BOT-160: a Quarantine human's own place at the stand (whether it was picked after the outbreak, and when);
    /// whether it keeps to the stand this tick (no roam, no hunt).
    qrt_guard: Option<([f32; 3], bool, Millis)>,
    qrt_camp: bool,
    /// BOT-160: the round as the server's messages tell it (outbreak, infected, stand, chase), the world's ranked
    /// stands, and the human's plan for the track.
    qrt_round: crate::qrt::Round,
    qrt_spots: Option<(String, Vec<[f32; 3]>)>,
    qrt_plan: Option<&'static str>,
    /// BOT-143: the ROM speed rows (read once from the user's ROM; `None`: the neutral speeds), the Quarantine
    /// override row the server gave this bot (0xd3 sub 3), until when a hit slows the infected body, the sprint pair
    /// last sent (`UP_SPRINT_STATE`), and whether the bot lets its spent gauge refill before it sprints again.
    speed_rows: Option<std::sync::Arc<dc_sim::SpeedTable>>,
    /// BOT-149: the worn gear's speed rows (head, backpack, goggles) of the lobby loadout, and the last weapon in
    /// hand (its `MovePenalty` counts until another is held, CLI-341).
    speed_gear: (Option<u32>, Option<u32>, Option<u32>),
    speed_weapon: Option<u16>,
    qrt_override: Option<u32>,
    shot_slow_until: Option<Millis>,
    sprint_pair: Option<(bool, bool)>,
    sprint_rest: bool,
    /// BOT-143: when the bot last took hazard damage (the server's poison type), and the safe place it walks to.
    hazard_hit: Option<Millis>,
    hazard_leave: Option<[f32; 3]>,
    /// BOT-150: when the walk ahead was last tested for danger, and whether it is held.
    walk_guard: (Millis, bool),
    /// Search and Destroy: the carrier mark, the plant and the bot's hold (BOT-70).
    bomb: crate::bombrun::BombRun,
    /// BOT-122: the bot's team job in a bomb round and its last order.
    squad: crate::squad::Squad,
    squad_order: Option<crate::squad::Order>,
    /// BOT-136: when the bot last had an enemy in sight; the posts round the sites.
    enemy_seen: Option<Millis>,
    posts_cache: Option<PostCache>,
    /// BOT-136: the places out of the planted bomb's blast: the bomb, when worked out, the places.
    ring_cache: Option<([f32; 3], Millis, Vec<[f32; 3]>)>,
    /// BOT-142: the attackers' guard places round the planted bomb: the bomb, when worked out, the places.
    guard_cache: Option<([f32; 3], Millis, Vec<[f32; 3]>)>,
    /// The bot is within the stop distance of its timeline `approach` player (`approach_reached` was logged).
    approach_near: bool,
    /// The goto point whose `goto_reached` was logged.
    goto_logged: Option<[f32; 3]>,
    /// Per object: stream entries counted so far and when that count last grew (stream silence).
    stream_watch: BTreeMap<u16, (u32, Millis)>,
    /// The running timeline ([`GameConfig::timeline`]).
    pub timeline: Option<crate::timeline::Player>,
}

impl BotSession {
    /// A client that will join when first polled. `rom` enables the ROM world checks (name, box, collision).
    pub fn new(cfg: GameConfig, rom: Option<RomWorlds>, rec: Recorder) -> Self {
        let first = cfg
            .app_ids
            .first()
            .copied()
            .unwrap_or(TODO_GAME_APP_ID_GUID_LAYOUT);
        // BOT-143: the speed rows of the loadout, read at run time from the user's ROM as the client does
        let speed_rows = rom
            .as_ref()
            .and_then(|r| dc_sim::SpeedTable::from_vfs(&dc_rez::RezVfs::new(r.rom())).ok())
            .map(std::sync::Arc::new);
        // BOT-149: the gear rows of the lobby loadout (0x323), as the client reads its own equipment (CLI-1137)
        let speed_gear = crate::motion::gear_rows(
            cfg.shooter
                .as_ref()
                .map_or(&[][..], |s| s.loadout.as_slice()),
        );
        BotSession {
            control: crate::control::Control::from_env(&cfg.nick),
            joiner: Joiner::new(first, cfg.join_rate_bps).with_timeout(cfg.join_timeout_ms),
            respawn: Respawn::new(cfg.respawn_after_ms),
            idle: IdleWatch::new(cfg.idle_exit_ms),
            shooter: cfg.shooter.clone().map(Shooter::new),
            leave_at: None,
            hunter: Hunter::default(),
            nav: None,
            follow: crate::nav::follow::Follower::default(),
            nav_step: 0,
            nav_last_pos: None,
            nav_cost: NavCost::default(),
            nav_end: None,
            nav_arrived: None,
            nav_cache: None,
            nav_said: 0,
            walk_check: None,
            roam: None,
            cautious_since: None,
            trouble: None,
            lost: None,
            edge_since: None,
            escape: None,
            escape_jumped: false,
            confined_since: None,
            confined_check: 0,
            health: None,
            escape_trail: None,
            roam_seed: 0x00f0_a3e5
                ^ cfg
                    .nick
                    .bytes()
                    .fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64)),
            evade_safe: (0, crate::evade::Safety::default()),
            push_safe: false,
            inside_step: None,
            edge: (0, None),
            watch: None,
            watch_seen: 0,
            goal_seen: 0,
            branch: "stand",
            was_jumping: false,
            kill_now: Vec::new(),
            kill_only: Vec::new(),
            in_hazard: false,
            doors: crate::doors::DoorMind::default(),
            nav_key: None,
            doors_said: 0,
            track_said: 0,
            role: crate::roles::role_for(&cfg.nick),
            evader: crate::evade::Evader::new(
                cfg.shooter.as_ref().map_or(0x5eed, |s| s.seed)
                    ^ cfg
                        .nick
                        .bytes()
                        .fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64)),
            ),
            hunt_goal_last: None,
            blast_wait: false,
            spread_step: Default::default(),
            spread_trail: Default::default(),
            spread_space: None,
            spread_aside: None,
            stood_at: None,
            carrying: None,
            flag_away: Default::default(),
            flag_home: BTreeMap::new(),
            flag_job: None,
            flag_loose: BTreeMap::new(),
            flag_carrier: BTreeMap::new(),
            flag_walk: Default::default(),
            flag_walk_team: None,
            flag_unreach: None,
            flag_guard_at: None,
            flag_quiet_since: None,
            flag_roving: false,
            spy: crate::spyrun::SpyRun::default(),
            spy_retreat: None,
            spy_lurk: None,
            spy_set_off: false,
            spy_way: None,
            spy_fifth: None,
            spy_fifth_place: None,
            spy_fifth_try: None,
            spy_chase_watch: None,
            spy_no_way: None,
            spy_focus: None,
            spy_guard: None,
            spy_engaged: None,
            last_hurt: None,
            knife: Default::default(),
            qrt_retreat: None,
            qrt_guard: None,
            qrt_camp: false,
            qrt_round: Default::default(),
            qrt_spots: None,
            qrt_plan: None,
            speed_rows,
            speed_gear,
            speed_weapon: None,
            qrt_override: None,
            shot_slow_until: None,
            sprint_pair: None,
            sprint_rest: false,
            hazard_hit: None,
            hazard_leave: None,
            walk_guard: (0, false),
            bomb: crate::bombrun::BombRun::default(),
            squad: crate::squad::Squad::default(),
            squad_order: None,
            enemy_seen: None,
            posts_cache: None,
            ring_cache: None,
            guard_cache: None,
            approach_near: false,
            goto_logged: None,
            stream_watch: BTreeMap::new(),
            timeline: cfg.timeline.clone().map(crate::timeline::Player::new),
            cfg,
            rom,
            rec,
            stage: Stage::Joining,
            app_idx: 0,
            link: None,
            start: None,
            obs: Observed::default(),
            table: ObjectTable::default(),
            world: None,
            world_job: None,
            world_loaded: false,
            load_done_at: None,
            held: VecDeque::new(),
            switch_state: None,
            life_state: 0,
            rate_sent: false,
            world_ready_sent: false,
            body: None,
            body_clock: 0,
            next_update: 0,
            state_form_sent: false,
            last_aim: None,
            last_update_at: None,
            facing: None,
            facing_at: 0,
            pending_teleport: None,
            gliding: false,
            pending_angles: None,
            quit_at: None,
            plain_bye_due: false,
            external: None,
            external_extras: None,
            last_cam: None,
            last_weapon_f: None,
            left_match: false,
            roster_pending: Vec::new(),
            roster_tick_at: None,
            external_spawned: false,
            awaiting_world: false,
            events: VecDeque::new(),
            max_outstanding: 0,
            paused_seen: 0,
            stalled_seen: 0,
            dropped: (0, 0),
            last_stats_log: 0,
        }
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn config(&self) -> &GameConfig {
        &self.cfg
    }

    pub fn link(&self) -> Option<&Link> {
        self.link.as_ref()
    }

    pub fn link_stats(&self) -> Option<LinkStats> {
        self.link.as_ref().map(Link::stats)
    }

    pub fn life_state(&self) -> u8 {
        self.life_state
    }

    pub fn body(&self) -> Option<&Body> {
        self.body.as_ref()
    }

    /// The own object (the view object), if the client has it.
    pub fn own_object(&self) -> Option<u16> {
        self.obs
            .view_object
            .filter(|o| self.table.objects.contains_key(o))
    }

    /// True once the session ended (failed, dropped or closed).
    pub fn is_over(&self) -> bool {
        matches!(
            self.stage,
            Stage::Failed(_) | Stage::Dropped(_) | Stage::Closed
        )
    }

    /// The caller's player update for the next send ([`GameConfig::external_body`]): the session sends the latest one
    /// under the 2009 gate (player state 1, the send rate), with the aim rotation only when it changed (BOT-9 rule).
    // SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (gate, send rule, flag 0x0100 on change)
    pub fn set_player_update(&mut self, u: MoveReport) {
        self.external = Some(u);
        self.external_extras = None;
    }

    /// As [`BotSession::set_player_update`], with the optional blocks a playing client sends (eye offset, weapon
    /// float, camera point, flag bits without a field). The camera point goes out only when it differs from the last
    /// one sent, as the aim rotation does.
    // SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (0x0400 when the camera position differs from the last one sent)
    pub fn set_player_update_ext(&mut self, u: MoveReport, extras: UpdateExtras) {
        self.external = Some(u);
        self.external_extras = Some(extras);
    }

    /// The player confirmed leaving the match: `UP_LEAVE_MATCH` 0xbd (no fields, reliable). The engine disconnect
    /// (`C2S_BYE`, [`BotSession::quit`]) is the caller's next step; the 2009 client sent it 16 ms later in the next
    /// reliable block. Returns false when there is no open link or the session is already leaving.
    // SPEC: GAME_MESSAGES_2009_C2S.md §5.7, Appendix A step A14
    // OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.16, CAPTURE_tdm4_KILL_VALIDATION.md §1 (0xbd, then C2S_BYE)
    pub fn leave_match(&mut self, now: Millis) -> bool {
        if self.left_match
            || self.quit_at.is_some()
            || self.is_over()
            || !self.link.as_ref().is_some_and(Link::is_open)
        {
            return false;
        }
        self.left_match = true;
        self.send_shell(true, shell_id::UP_LEAVE_MATCH, &BitWriter::new(), now);
        self.rec.event("leave_match", json!({"ms": self.ms(now)}));
        self.mark(now, "leave_match_sent");
        true
    }

    /// True once `UP_LEAVE_MATCH` was sent.
    pub fn left_match(&self) -> bool {
        self.left_match
    }

    /// Sends a game message of the caller (for example a movement notice 0x76 of its own simulation) once the
    /// client is in the world and not leaving. Returns false when it was not sent.
    // SPEC: GAME_MESSAGES_2009_C2S.md §1.1 (shell messages on the game link), §1.3 (all reliable except 0x64 U)
    pub fn send_game_message(
        &mut self,
        now: Millis,
        reliable: bool,
        shell: u8,
        body: &BitWriter,
    ) -> bool {
        if !self.world_ready_sent
            || self.quit_at.is_some()
            || self.left_match
            || self.is_over()
            || !self.link.as_ref().is_some_and(Link::is_open)
        {
            return false;
        }
        self.send_shell(reliable, shell, body, now);
        true
    }

    /// The world file name the session waits for the caller to load ([`GameConfig::external_load`]).
    pub fn world_request(&self) -> Option<&str> {
        if self.awaiting_world {
            self.obs.announced_world.as_deref()
        } else {
            None
        }
    }

    /// The caller loaded the announced world: its box is the `pos48` box of the stream, `label` names what was loaded.
    // SPEC: UDP_TRANSPORT_2009_addendum_disasm.md T14 (pos48 box = the world file's extents)
    pub fn world_loaded(&mut self, now: Millis, extents: crate::objects::ExtentBox, label: &str) {
        if !self.awaiting_world {
            return;
        }
        self.awaiting_world = false;
        self.table.extents = Some(extents);
        self.obs.world_resolved = Some(Ok(label.to_string()));
        self.rec.event(
            "world",
            json!({"external": label, "box_min": extents.min, "box_max": extents.max, "ms": self.ms(now)}),
        );
    }

    /// The caller could not load the announced world: the session ends as the 2009 client's does (0x29).
    // SPEC: UDP_SESSION_2009_A.md §4.6 step 4 (no stream -> 0x29)
    pub fn world_failed(&mut self, now: Millis, why: &str) {
        if !self.awaiting_world {
            return;
        }
        self.awaiting_world = false;
        self.obs.world_resolved = Some(Err(why.to_string()));
        self.fatal(
            now,
            result::WORLD_MISSING,
            format!("world load: {why}"),
            true,
        );
    }

    /// The game events since the last call ([`GameConfig::queue_events`]).
    pub fn take_events(&mut self) -> Vec<GameEvent> {
        self.events.drain(..).collect()
    }

    /// The player is spawned and its updates go out (the caller's player or the bot body).
    pub fn player_spawned(&self) -> bool {
        self.external_spawned || self.body.is_some()
    }

    fn event(&mut self, e: GameEvent) {
        if !self.cfg.queue_events {
            return;
        }
        if self.events.len() >= EVENTS_KEEP {
            self.events.pop_front();
        }
        self.events.push_back(e);
    }

    /// The engine part of the in-world test (a, b, c) and the player state (d).
    // SPEC: UDP_SESSION_2009_A.md §8.3 (a view object exists, b state 2 or 3, c world loaded, d player state)
    pub fn in_world_test(&self) -> (bool, bool) {
        let a = self.own_object().is_some();
        let b = matches!(self.switch_state, Some(2) | Some(3));
        let engine = a && b && self.world_loaded;
        (engine, engine && self.life_state != 0)
    }

    fn ms(&self, now: Millis) -> u32 {
        since(now, self.start.unwrap_or(now))
    }

    fn mark(&mut self, now: Millis, m: &str) {
        if !self.obs.reached(m) {
            let t = self.ms(now);
            self.obs.milestones.push((m.to_string(), t));
            self.rec.event("milestone", json!({"step": m, "ms": t}));
        }
    }

    fn fatal(&mut self, now: Millis, code: u32, why: String, stop: bool) {
        self.rec.event(
            "fatal",
            json!({"code": format!("0x{code:x}"), "why": why, "stopped": stop}),
        );
        self.obs.fatal.push(format!("0x{code:x}: {why}"));
        if stop {
            self.stage = Stage::Dropped(code);
            self.obs.end = Some(format!("dropped: result 0x{code:x}"));
            // SPEC: UDP_SESSION_2009_A.md §6.1 (error processing destroys the server connection)
            if let Some(l) = self.link.as_mut() {
                l.close(now, Bye::Plain);
            }
        }
    }

    fn send(&mut self, reliable: bool, msg: BitWriter, now: Millis) {
        let bytes = msg.as_bytes();
        let id = bytes.first().copied().unwrap_or(0);
        let sh = (id == c2s::SHELL_MSG).then(|| bytes.get(1).copied().unwrap_or(0));
        *self.obs.c2s_counts.entry(id).or_default() += 1;
        if let Some(s) = sh {
            *self.obs.c2s_shell_counts.entry(s).or_default() += 1;
        }
        // updates are frequent: log the first ones and then every 50th
        let quiet = sh == Some(shell_id::UP_PLAYER_UPDATE)
            && !reliable
            && self.obs.updates_sent > 5
            && !self.obs.updates_sent.is_multiple_of(50);
        // BOT-157 (amended by BOT-168): per-message game records only with `--log-transport`
        if !quiet {
            self.rec.transport_event(
                "msg_out",
                json!({"rel": reliable, "id": id, "shell": sh, "bits": msg.bit_len(),
                       "head": head(bytes), "ms": self.ms(now)}),
            );
        }
        if let Some(l) = self.link.as_mut() {
            if reliable {
                l.send_reliable(msg);
            } else {
                l.send_unreliable(now, msg);
            }
        }
    }

    fn send_shell(&mut self, reliable: bool, shell: u8, body: &BitWriter, now: Millis) {
        self.send(reliable, shell::shell_msg(shell, body), now);
    }

    /// Asks to end the session: `C2S_BYE` (reliable), then the transport bye once it is acknowledged.
    // SPEC: UDP_SESSION_2009_A.md §5.6; UDP_TRANSPORT_2009.md §9.1
    pub fn quit(&mut self, now: Millis) {
        if self.quit_at.is_some() {
            return;
        }
        self.quit_at = Some(now);
        if self.link.as_ref().is_some_and(Link::is_open) {
            self.send(true, shell::bye(), now);
            // after the C2S_BYE block has left the link goes quiet: no ack, no keep-alive, no resend
            // PROVEN-CAPTURE: UDP_TRANSPORT_2009.md §9.1 (silence after C2S_BYE)
            if let Some(l) = self.link.as_mut() {
                l.quiesce_when_flushed();
            }
            self.rec.event("quit", json!({"ms": self.ms(now)}));
        } else if !self.is_over() {
            self.stage = Stage::Closed;
            self.obs.end = Some("quit before the link existed".into());
        }
    }

    /// Leaves the way the 2009 client leaves its result screen: no game message and no `C2S_BYE`, only the plain
    /// transport bye (`CTL_BYE`, four copies 100 ms apart). Returns false when the link is not open.
    // OBSERVED: capture 20261025_125432_rust6 conn 5 (0x3f at 797.234, nothing more on the game
    // link, `c9 03` x4 from 804.891) and 20261025_153705_rust7 conn 5 (0x3f at 711.844, `c9 03` x4 from 720.016)
    // SPEC: UDP_TRANSPORT_2009.md §9.1 (plain bye when the shell leaves the result screen)
    pub fn leave_plain(&mut self, now: Millis) -> bool {
        if self.quit_at.is_some() || !self.link.as_ref().is_some_and(Link::is_open) {
            return false;
        }
        self.quit_at = Some(now);
        self.plain_bye_due = true;
        if let Some(l) = self.link.as_mut() {
            l.send_plain_bye(now);
        }
        self.rec.event("leave_plain", json!({"ms": self.ms(now)}));
        self.mark(now, "leave_plain");
        true
    }

    /// A datagram from the server.
    pub fn handle_datagram(&mut self, now: Millis, bytes: &[u8]) {
        self.start.get_or_insert(now);
        if self.stage == Stage::Joining {
            match self.joiner.handle_datagram(bytes) {
                JoinStatus::Accepted => self.on_joined(now),
                JoinStatus::WrongApp if self.app_idx + 1 < self.cfg.app_ids.len() => {
                    self.rec.event(
                        "join_wrong_app",
                        json!({"app_id_index": self.app_idx, "next": self.app_idx + 1}),
                    );
                    self.app_idx += 1;
                    self.joiner =
                        Joiner::new(self.cfg.app_ids[self.app_idx], self.cfg.join_rate_bps)
                            .with_timeout(self.cfg.join_timeout_ms);
                }
                JoinStatus::Pending => {}
                other => {
                    let code = other.result_code().unwrap_or(0);
                    self.stage = Stage::Failed(code);
                    self.obs.end = Some(format!("join failed: {other:?} (0x{code:x})"));
                    self.rec
                        .event("join_failed", json!({"status": format!("{other:?}")}));
                }
            }
            return;
        }
        self.idle.touch(now);
        // the link's events (acks to send, delivered messages) come out of the next poll
        if let Some(l) = self.link.as_mut() {
            l.handle_datagram(now, bytes);
        }
    }

    fn on_joined(&mut self, now: Millis) {
        let Some(mut link) = self.joiner.clone().into_link(now) else {
            return;
        };
        self.obs.app_id_index = Some(self.app_idx);
        self.idle.touch(now);
        self.mark(now, "joined");
        self.rec.event(
            "joined",
            json!({"app_id_index": self.app_idx, "asks": self.obs.join_asks}),
        );
        self.stage = Stage::Linked;
        // SPEC: UDP_SESSION_2009_A.md §2.4 step 4, §5.2 (C2S_HELLO first, reliable)
        let hello = shell::hello(
            &self.cfg.nick,
            &self.cfg.hello_tag,
            self.cfg.seat_ticket.as_ref(),
        );
        self.rec.transport_event(
            "msg_out",
            json!({"rel": true, "id": c2s::HELLO, "bits": hello.bit_len(), "head": head(hello.as_bytes())}),
        );
        *self.obs.c2s_counts.entry(c2s::HELLO).or_default() += 1;
        link.send_reliable(hello);
        self.link = Some(link);
        self.mark(now, "S1_hello_sent");
    }

    /// Runs the client at `now` and returns the datagrams to send.
    pub fn poll(&mut self, now: Millis) -> Vec<Vec<u8>> {
        self.start.get_or_insert(now);
        let mut out = Vec::new();
        if self.stage == Stage::Joining {
            if let Some(d) = self.joiner.poll(now) {
                self.obs.join_asks += 1;
                if self.obs.join_asks == 1 {
                    self.mark(now, "join_ask");
                }
                out.push(d);
            }
            if self.joiner.status() == JoinStatus::TimedOut {
                let code = JoinStatus::TimedOut.result_code().unwrap_or(0);
                self.stage = Stage::Failed(code);
                self.obs.end = Some(format!("join timed out (0x{code:x})"));
                self.rec.event("join_failed", json!({"status": "TimedOut"}));
            }
            return out;
        }
        if let Some(t) = self.load_done_at {
            if !later(t, now) && !self.awaiting_world && self.world_job_done(now) && !self.is_over()
            {
                self.finish_load(now);
            }
        }
        self.check_idle(now);
        self.game_tick(now);
        if let Some(q) = self.quit_at {
            if let Some(l) = self.link.as_mut() {
                // wait for the server's own close (bye code 5 after C2S_BYE, UDP_SESSION_2009_A.md §5.6); without
                // it the bot sends the transport bye itself
                if (l.is_open() && since(now, q) >= QUIT_FLUSH_MS) || self.plain_bye_due {
                    l.send_plain_bye(now);
                }
            }
        }
        let events = self.link.as_mut().map(|l| l.poll(now)).unwrap_or_default();
        for e in events {
            match e {
                LinkEvent::Transmit(d) => out.push(d),
                LinkEvent::Message { reliable, bits } => self.deliver(now, reliable, bits),
                LinkEvent::Closed { code, cause } => self.on_closed(now, code, cause),
            }
        }
        self.track_link(now);
        out
    }

    fn track_link(&mut self, now: Millis) {
        let Some(l) = self.link.as_ref() else {
            return;
        };
        self.max_outstanding = self.max_outstanding.max(l.outstanding());
        if l.is_paused() {
            self.paused_seen += 1;
        }
        if l.is_stalled() {
            self.stalled_seen += 1;
        }
        if since(now, self.last_stats_log) >= 5000 {
            self.last_stats_log = now;
            let s = l.stats();
            self.rec.event(
                "link_stats",
                json!({"ms": self.ms(now), "out": s.datagrams_out, "in": s.datagrams_in,
                       "bad_check": s.bad_check, "rel_blocks_out": s.rel_blocks_out, "resends": s.resends,
                       "keepalive_blocks": s.keepalive_blocks, "acks_out": s.acks_out,
                       "flow_drops": s.flow_drops, "rel_in_order": s.rel_in_order, "rel_stored": s.rel_stored,
                       "rel_discarded": s.rel_discarded, "delivered_rel": s.delivered_rel,
                       "delivered_unrel": s.delivered_unrel, "unrel_expired": s.unrel_expired,
                       "outstanding": l.outstanding(), "l_ms": l.l_ms(), "rate_bps": l.rate_bps(),
                       "paused": l.is_paused(), "stalled": l.is_stalled(),
                       "last_in_order": l.last_in_order(), "newest_sent": l.newest_block_seq(),
                       "server_time": self.table.server_time}),
            );
        }
    }

    fn on_closed(&mut self, now: Millis, code: u32, cause: CloseCause) {
        self.rec.event(
            "link_closed",
            json!({"code": code, "cause": format!("{cause:?}"), "ms": self.ms(now)}),
        );
        if matches!(self.stage, Stage::Dropped(_) | Stage::Closed) {
            return;
        }
        if cause == CloseCause::PeerBye && self.quit_at.is_none() {
            // the server ended the session: the bot's session ends cleanly too
            self.stage = Stage::Closed;
            self.obs.end = Some(format!("server bye (code {code})"));
            self.obs.server_bye = Some(code);
            self.rec
                .event("server_bye", json!({"code": code, "ms": self.ms(now)}));
        } else if self.quit_at.is_some() && cause != CloseCause::IdleTimeout {
            self.plain_bye_due |= cause == CloseCause::PeerBye;
            self.stage = Stage::Closed;
            self.obs
                .end
                .get_or_insert_with(|| format!("quit ({cause:?}, code {code})"));
        } else {
            self.stage = Stage::Dropped(code);
            self.obs.end = Some(format!("link closed: {cause:?}, code {code}"));
        }
    }

    /// A delivered message: recorded in arrival order; held while the world loads, else processed.
    fn deliver(&mut self, now: Millis, reliable: bool, bits: BitWriter) {
        let bytes = bits.as_bytes();
        let id = bytes.first().copied().unwrap_or(0);
        let sh = (id == s2c::SHELL_MSG).then(|| bytes.get(1).copied().unwrap_or(0));
        let t = self.ms(now);
        if self.obs.s2c_order.len() < ORDER_KEEP {
            self.obs.s2c_order.push((t, reliable, id, sh));
        }
        let n = {
            let c = self.obs.s2c_counts.entry(id).or_default();
            *c += 1;
            *c
        };
        if let Some(s) = sh {
            *self.obs.shell_counts.entry(s).or_default() += 1;
        }
        // the stream arrives every tick: log the first ones and then every 100th
        // BOT-157 (amended by BOT-168): per-message game records only with `--log-transport`
        if id != s2c::OBJ_STREAM || n <= 5 || n.is_multiple_of(100) {
            self.rec.transport_event(
                "msg_in",
                json!({"rel": reliable, "id": id, "name": s2c::name(id), "shell": sh,
                       "bits": bits.bit_len(), "head": head(bytes), "ms": t, "n": n}),
            );
        }
        // NET-24: the server's link move secret goes to the link at once (from now on every datagram carries it, so
        // the server can follow this client to a new port after a NAT rebinding); it is a transport matter, never held
        // DC-DEFAULT: NET-24 (proposed, project decision)
        if sh == Some(dc_wire::linkmove::SHELL_ID) {
            let mut r = bits.reader();
            r.seek_bits(16);
            if let (Some(secret), Some(l)) =
                (dc_wire::linkmove::read_body(&mut r), self.link.as_mut())
            {
                l.set_move_trailer(secret);
                self.rec.event("move_secret", json!({"ms": t}));
            }
            return;
        }
        if self.load_done_at.is_some() {
            self.held.push_back((reliable, bits));
        } else {
            self.process(now, &bits);
        }
    }

    /// True when no world job is pending; takes its result when it finished.
    fn world_job_done(&mut self, now: Millis) -> bool {
        let Some((label, rx)) = self.world_job.take() else {
            return true;
        };
        match rx.try_recv() {
            Ok(Ok(w)) => {
                self.rec.event(
                    "world",
                    json!({"bytes": w.size, "sha16": w.sha16, "box_min": w.extents.min, "box_max": w.extents.max,
                           "player_spawns": w.player_spawns, "ms": self.ms(now)}),
                );
                self.table.extents = Some(w.extents);
                self.obs.world_resolved = Some(Ok(w.listed.clone()));
                // BOT-120: the world's doors; the grid takes them as ways
                let boxes = crate::doors::door_boxes(&w);
                self.rec.event(
                    "door_set",
                    json!({"ms": self.ms(now), "doors": boxes.iter().map(|d| json!({
                        "name": String::from_utf8_lossy(&d.name), "lo": d.lo, "hi": d.hi})).collect::<Vec<_>>()}),
                );
                self.doors = crate::doors::DoorMind::new(boxes, self.roam_seed);
                self.world = Some(w);
                // BOT-93: the navigation grid grows from where the bot stands, on this world
                let coll = self.nav_collision();
                self.nav = coll.map(|c| crate::nav::NavGrid::new(c, BOT_STAND_DIMS));
                self.nav_reset();
                self.follow.forget_refused();
                true
            }
            Ok(Err(e)) => {
                self.obs.world_resolved = Some(Err(e.clone()));
                self.fatal(now, 0x2a, format!("world {label:?}: {e}"), true);
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.world_job = Some((label, rx));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.fatal(
                    now,
                    0x2a,
                    format!("world {label:?}: loader thread failed"),
                    true,
                );
                true
            }
        }
    }

    fn finish_load(&mut self, now: Millis) {
        self.load_done_at = None;
        self.world_loaded = true;
        self.stage = Stage::WorldLoaded;
        // SPEC: UDP_SESSION_2009_A.md §4.6 step 6-7 (entered-world callback, then `[9][0]` reliable)
        self.send(true, shell::load_state(0), now);
        self.mark(now, "S6a_world_loaded_sent");
        while let Some((_, m)) = self.held.pop_front() {
            self.process(now, &m);
            if self.load_done_at.is_some() || self.is_over() {
                break;
            }
        }
    }

    /// Dispatches one server message (engine id first).
    // SPEC: UDP_SESSION_2009_A.md §3.3 (dispatcher; unknown ids ignored), UDP_SESSION_2009_B.md §2.4
    fn process(&mut self, now: Millis, msg: &BitWriter) {
        if self.is_over() {
            return;
        }
        let mut r = msg.reader();
        let id = r.read_u8();
        match id {
            s2c::VERSION => {
                // SPEC: UDP_SESSION_2009_A.md §4.2 (7 else 0x4E; a lower host rate lowers the link rate)
                let v = r.read_u32();
                let rate = r.read_u32();
                self.obs.version = Some((v, rate));
                self.mark(now, "S2_version");
                if v != NET_VERSION {
                    self.fatal(
                        now,
                        result::VERSION_MISMATCH,
                        format!("net_version {v}"),
                        true,
                    );
                    return;
                }
                if let Some(l) = self.link.as_mut() {
                    let eff = ((rate as u64 * RATE_PERCENT) / 100) as u32;
                    if eff < l.rate_bps() {
                        l.change_rate(now, eff);
                    }
                }
            }
            s2c::CLIENT_ID => {
                // SPEC: UDP_SESSION_2009_A.md §4.3 (local_host must be 0 for a remote server)
                let cid = r.read_u16();
                let local = r.read_u8();
                self.obs.client_id = Some((cid, local));
                self.mark(now, "S3_client_id");
                if local != 0 {
                    self.obs.parse_notes.push(format!(
                        "S2C_CLIENT_ID local_host = {local}: a 2009 client would take the local-server branch at load"
                    ));
                }
            }
            s2c::FILE_LIST => {
                // SPEC: UDP_SESSION_2009_A.md §4.4 (entries to the end; one ack per list, bit 15 always set)
                let mut ids = Vec::new();
                while r.remaining_bits() >= 16 + 32 + 8 {
                    let fid = r.read_u16();
                    let info = r.read_u32();
                    let name = String::from_utf8_lossy(&r.read_zstr8()).into_owned();
                    self.obs.files.entry(fid).or_insert((info, name));
                    ids.push(fid);
                }
                self.mark(now, "S4_file_list");
                self.send(true, shell::file_ack(&ids), now);
                self.mark(now, "S4a_file_ack_sent");
            }
            s2c::EVAR => {
                let n = String::from_utf8_lossy(&r.read_zstr8()).into_owned();
                let v = String::from_utf8_lossy(&r.read_zstr8()).into_owned();
                self.obs.evars.push((n, v));
                self.mark(now, "S5_evar");
            }
            s2c::LOAD_WORLD => self.on_load_world(now, &mut r),
            s2c::PRELOAD => {
                // SPEC: UDP_SESSION_2009_A.md §4.7 (sub 1 -> `[9][1]` whenever it arrives)
                let sub = r.read_u8();
                self.obs.preload_subs.push(sub);
                if (2..=6).contains(&sub) {
                    let mut ids = Vec::new();
                    while r.remaining_bits() >= 16 {
                        ids.push(r.read_u16());
                    }
                    self.obs.preload_lists.push((sub, ids));
                }
                if sub == 1 {
                    self.mark(now, "S7_preload_end");
                    self.send(true, shell::load_state(1), now);
                    self.mark(now, "S8a_preload_done_sent");
                }
            }
            s2c::SKY_TABLE => {
                // client-hudgaps: the block and the object ids are kept for the sky pass (dc-client)
                let mut block = [0u8; 48];
                for b in block.iter_mut() {
                    *b = r.read_u8();
                }
                self.obs.sky_block = Some(block);
                let count = r.read_u16();
                self.obs.sky_count = Some(count);
                // SPEC: UDP_SESSION_2009_A.md §4.8 (`count x sky_object:u16`)
                self.obs.sky_ids.clear();
                for _ in 0..count.min(SKY_TABLE_MAX) {
                    if r.remaining_bits() < 16 {
                        break;
                    }
                    self.obs.sky_ids.push(r.read_u16());
                }
                self.mark(now, "S8_sky_table");
                if count > SKY_TABLE_MAX {
                    self.fatal(
                        now,
                        result::SKY_TABLE,
                        format!("sky table count {count}"),
                        true,
                    );
                }
            }
            s2c::VIEW_OBJECT => {
                let o = r.read_u16();
                self.obs.view_object = Some(o);
                self.mark(now, "S10_view_object");
            }
            s2c::LEAVE_WORLD => {
                // SPEC: UDP_SESSION_2009_A.md §4.10 (view id reset, world unloaded, link kept)
                self.obs.view_object = None;
                self.world_loaded = false;
                self.stage = Stage::Linked;
                self.body = None;
                self.external_spawned = false;
                self.mark(now, "leave_world");
            }
            s2c::OBJ_GROUP => {
                let rest = r.read_bit_string(r.remaining_bits());
                let before = self.table.errors.len();
                self.table.apply_group(&rest);
                self.after_objects(now, before);
            }
            s2c::OBJ_STREAM => {
                let rest = r.read_bit_string(r.remaining_bits());
                let before = self.table.errors.len();
                let had_time = self.table.time_entries;
                self.table.apply_stream(&rest);
                if self.table.time_entries > had_time {
                    self.mark(now, "S13_first_time_entry");
                }
                self.after_objects(now, before);
            }
            s2c::SHELL_OBJ_MSG => {
                let rest = r.read_bit_string(r.remaining_bits());
                if let Some(e) = crate::fx::read_effect(&rest) {
                    if !matches!(e, crate::fx::Effect::Other(_)) {
                        self.event(GameEvent::Effect(e));
                    }
                }
            }
            s2c::SHELL_MSG => {
                let shell = r.read_u8();
                let rest = r.read_bit_string(r.remaining_bits());
                self.on_shell(now, shell, &rest);
            }
            s2c::BUNDLE => {
                // SPEC: UDP_SESSION_2009_A.md §3.4; addendum A20 (len in bits, len > remaining -> 0x2c)
                while r.remaining_bits() >= 8 {
                    let len = r.read_u8() as usize;
                    if len == 0 {
                        break;
                    }
                    if len > r.remaining_bits() {
                        self.fatal(now, 0x2c, "bundle entry past the end".into(), true);
                        return;
                    }
                    let inner = r.read_bit_string(len);
                    self.process(now, &inner);
                }
            }
            _ => {}
        }
        self.check_in_world(now);
    }

    fn on_load_world(&mut self, now: Millis, r: &mut BitReader<'_>) {
        // SPEC: UDP_SESSION_2009_A.md §4.6 (view object reset, unload, file id -> stream, no stream -> 0x29)
        let clock = r.read_f32();
        let file = r.read_u16();
        self.obs.load_world = Some((clock, file));
        self.obs.view_object = None;
        self.world_loaded = false;
        self.body = None;
        self.external_spawned = false;
        self.mark(now, "S6_load_world");
        let Some((_, name)) = self.obs.files.get(&file).cloned() else {
            self.fatal(
                now,
                result::WORLD_MISSING,
                format!("world file id {file} was never announced"),
                true,
            );
            return;
        };
        self.obs.announced_world = Some(name.clone());
        // The 2009 client loads inside the handler while its separate receive thread keeps acknowledging
        // (UDP_SESSION_2009_A.md §4.6). The bot keeps its single loop running (acks, keep-alive) and parses the
        // world file on a worker thread; the simulated load ends when both the load time and the worker are done.
        let job: Option<(String, WorldJob)> = if self.cfg.external_load {
            // the caller loads the world (world_loaded / world_failed)
            self.awaiting_world = true;
            None
        } else if let Some(path) = self.cfg.world_file.clone() {
            let label = path.display().to_string();
            let rom_dir = self.rom.as_ref().map(|r| r.rom().to_path_buf());
            Some((
                label,
                Box::new(move || crate::romworld::load_world_file(&path, rom_dir.as_deref())),
            ))
        } else if let Some(path) = self
            .cfg
            .mod_cache
            .as_deref()
            .and_then(|cache| crate::romworld::mod_world_path(cache, &name))
        {
            let label = path.display().to_string();
            let rom_dir = self.rom.as_ref().map(|r| r.rom().to_path_buf());
            Some((
                label,
                Box::new(move || crate::romworld::load_world_file(&path, rom_dir.as_deref())),
            ))
        } else if let Some(rom) = &self.rom {
            match rom.resolve(name.as_bytes()) {
                Some(listed) => {
                    let root = rom.rom().to_path_buf();
                    let l2 = listed.clone();
                    Some((listed, Box::new(move || RomWorlds::open(&root)?.load(&l2))))
                }
                None => {
                    self.obs.world_resolved = Some(Err(format!("{name:?} not in the ROM")));
                    self.fatal(
                        now,
                        result::WORLD_MISSING,
                        format!("announced world {name:?} does not resolve in the ROM"),
                        true,
                    );
                    return;
                }
            }
        } else {
            None
        };
        if let Some((label, f)) = job {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(f());
            });
            self.world_job = Some((label, rx));
        }
        self.stage = Stage::LoadingWorld;
        self.load_done_at = Some(now.wrapping_add(self.cfg.load_ms));
    }

    fn after_objects(&mut self, now: Millis, errors_before: usize) {
        let new: Vec<(u32, String)> = self.table.errors[errors_before..].to_vec();
        for (code, e) in new {
            // a 2009 client stops here; the bot reports it and keeps the session for the record
            self.fatal(now, code, format!("object message: {e}"), false);
        }
        if let Some(o) = self.obs.view_object {
            if self.table.objects.contains_key(&o) {
                self.mark(now, "S9_own_object");
            }
        }
        self.follow_world(now);
        let own = self.obs.view_object;
        if self
            .obs
            .roster
            .values()
            .any(|p| Some(p.character) != own && self.table.objects.contains_key(&p.character))
        {
            self.mark(now, "other_player_object");
        }
    }

    /// BOT-110 / BOT-111: the loaded world follows the server's objects: the world models it made solid (and the doors
    /// still closed) are the collision set of the body, the shooter's view and the grid; the kill volumes it created
    /// keep the grid's nodes out. The grid starts again when either changed.
    // DC-DEFAULT: BOT-110 (proposed), BOT-111 (proposed)
    fn follow_world(&mut self, now: Millis) {
        let Some(w) = self.world.as_mut() else {
            return;
        };
        let changed = w.follow_server(&self.table);
        let kill = w.active_kill_volumes(&self.table);
        // BOT-120: the grid takes doors as ways, so a door that opens or closes does not start it again
        let key =
            crate::doors::nav_key(w, &self.doors.boxes, &self.doors.given_up(), self.cfg.doors);
        let grid_changed = self.nav_key.as_ref() != Some(&key) || kill.len() != self.kill_now.len();
        if changed.is_none() && !grid_changed {
            return;
        }
        let coll = w.collision.clone();
        let solid = w
            .solid
            .as_ref()
            .map_or(0, |s| s.iter().filter(|x| **x).count());
        let linked = w.linked.len();
        // BOT-141: the linked models the server left out (named, so an offline check can build the same set)
        let off: Vec<String> = w.solid.as_ref().map_or(Vec::new(), |s| {
            w.linked
                .iter()
                .zip(s)
                .filter(|(_, on)| !**on)
                .map(|(l, _)| String::from_utf8_lossy(&l.name).into_owned())
                .collect()
        });
        let gone = changed.map(|c| c.1).unwrap_or_default();
        if let Some(b) = self.body.as_mut() {
            b.set_world(coll, &gone);
        }
        if !grid_changed {
            return;
        }
        self.kill_now = kill.clone();
        let hazard = &w.hazard_volumes;
        self.kill_only = kill
            .iter()
            .filter(|k| !hazard.contains(k))
            .cloned()
            .collect();
        let nav_coll = self.nav_collision();
        let (deadly, hazard) = self.nav_volumes();
        self.nav = nav_coll.map(|c| {
            crate::nav::NavGrid::new(c, self.nav_dims())
                .with_kill_volumes(deadly)
                .with_hazard_volumes(hazard)
        });
        // BOT-132: the ladders are ways of the grid
        let ladders = self.nav_ladders();
        self.nav = self.nav.take().map(|n| n.with_ladders(&ladders));
        self.nav_reset();
        self.follow.forget_refused();
        self.rec.event(
            "world_set",
            json!({"ms": self.ms(now), "linked": linked, "solid": solid, "gone": gone.len(),
                   "kill_volumes": self.kill_now.len(), "off": off}),
        );
    }

    /// BOT-132: the world's ladder volumes, ways of the grid.
    fn nav_ladders(&self) -> Vec<dc_sim::ConvexVolume> {
        self.world.as_ref().map_or(Vec::new(), |w| {
            w.ladders.volumes.iter().map(|v| v.0.clone()).collect()
        })
    }

    /// BOT-120: the grid's collision set: the world's solid set with the doors the bot has not given up taken as
    /// open (doors off: the solid set). Remembers the key it was built for.
    // DC-DEFAULT: BOT-120 (proposed)
    /// BOT-154: the grid's kill volumes (the ones the server created) and hazard volumes, apart.
    // DC-DEFAULT: BOT-154 (proposed)
    fn nav_volumes(&self) -> (Vec<dc_sim::ConvexVolume>, Vec<dc_sim::ConvexVolume>) {
        self.world.as_ref().map_or((Vec::new(), Vec::new()), |w| {
            (
                w.active_deadly_volumes(&self.table),
                w.hazard_volumes.clone(),
            )
        })
    }

    fn nav_collision(&mut self) -> Option<std::sync::Arc<dc_sim::CollisionWorld>> {
        let w = self.world.as_ref()?;
        let key =
            crate::doors::nav_key(w, &self.doors.boxes, &self.doors.given_up(), self.cfg.doors);
        let c = crate::doors::nav_collision(w, &key);
        self.nav_key = Some(key);
        Some(c)
    }

    /// BOT-120: the door on the bot's way: `Some(yaw)` while it stands at a closed door (facing it), the use key
    /// sent when due. Logs the presses, opens and give-ups, and the counters every 30 s.
    // DC-DEFAULT: BOT-120 (proposed)
    fn door_tick(
        &mut self,
        now: Millis,
        alive: bool,
        pos: [f32; 3],
        goal: Option<[f32; 3]>,
    ) -> Option<f32> {
        let w = self.world.as_ref()?;
        if self.doors.boxes.is_empty() {
            return None;
        }
        let views = crate::doors::door_views(w, &self.table, &self.doors.boxes);
        let path: Vec<[f32; 3]> = match goal {
            None => Vec::new(),
            Some(_) if !self.follow.points().is_empty() => self.follow.points().to_vec(),
            Some(g) => vec![g],
        };
        let mut notes = Vec::new();
        let hold = self
            .doors
            .tick(now, alive, pos, &path, &views, self.cfg.doors, &mut notes);
        if hold.is_some_and(|h| h.press) {
            self.send_shell(true, crate::doors::UP_USE, &crate::doors::use_body(), now);
        }
        let mut regrid = false;
        for n in notes {
            let (what, door, extra) = match n {
                crate::doors::DoorNote::Press { door, n } => ("door_press", door, n),
                crate::doors::DoorNote::Opened { door, waited_ms } => {
                    ("door_opened", door, waited_ms)
                }
                crate::doors::DoorNote::GaveUp { door, presses } => {
                    regrid = true;
                    ("door_given_up", door, presses)
                }
                crate::doors::DoorNote::Back { door } => {
                    regrid = true;
                    ("door_back", door, 0)
                }
            };
            let name = String::from_utf8_lossy(&self.doors.boxes[door].name).into_owned();
            self.rec.event(
                what,
                json!({"ms": self.ms(now), "door": name, "n": extra, "pos": pos}),
            );
        }
        if since(now, self.doors_said) >= 30_000 {
            self.doors_said = now;
            let s = self.doors.stats;
            self.rec.event(
                "door_stats",
                json!({"ms": self.ms(now), "on": self.cfg.doors, "presses": s.presses, "opened": s.opened,
                       "given_up": s.given_up, "wait_ms": s.wait_ms, "near_closed_ms": s.near_closed_ms}),
            );
        }
        if regrid {
            self.follow_world(now);
        }
        hold.map(|h| h.yaw)
    }

    /// BOT-117: the place a bot with nothing to walk to roams to: a node of the grid at least [`ROAM_MIN`] units away
    /// (flat), kept until the bot comes within [`ROAM_REACHED`] of it or [`ROAM_MS`] passed. `None` without a grid.
    // DC-DEFAULT: BOT-117 (proposed)
    fn roam_goal(&mut self, now: Millis) -> Option<[f32; 3]> {
        let pos = self.body.as_ref().map(|b| b.pos())?;
        let flat =
            |a: [f32; 3], b: [f32; 3]| ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
        if let Some((g, at)) = self.roam {
            if since(now, at) < ROAM_MS && flat(g, pos) > ROAM_REACHED {
                return Some(g);
            }
        }
        let mut x = self.roam_seed.max(1);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.roam_seed = x;
        // BOT-143: with no node that far (a young grid: Junk Flea's round 1 knew 61..200 nodes within 800 units) the
        // grid grows a little, then the farthest known node is the goal (walking to the grid's edge grows it); before,
        // the bot had no goal and the grid never grew
        let nav = self.nav.as_mut()?;
        let (g, why) = match nav.far_node(pos, ROAM_MIN, x) {
            Some(g) => (g, "far"),
            None => {
                nav.expand(ROAM_GROW, None);
                match nav.far_node(pos, ROAM_MIN, x) {
                    Some(g) => (g, "far"),
                    None => (nav.farthest_node(pos)?, "farthest"),
                }
            }
        };
        self.roam = Some((g, now));
        self.rec.event(
            "roam",
            json!({"ms": self.ms(now), "pos": pos, "goal": g, "pick": why}),
        );
        Some(g)
    }

    /// The body the grid is grown for: the body's own box (the server's teleport box), at least [`BOT_STAND_DIMS`]
    /// and at least the box of the grid there is (it only grows: a new life's body has the stand box until its
    /// teleport, and the grid is kept).
    // DC-DEFAULT: BOT-112 (proposed) the grid's body is the body's box
    fn nav_dims(&self) -> [f32; 3] {
        let d = self.body.as_ref().map_or(BOT_STAND_DIMS, |b| b.dims());
        let have = self.nav.as_ref().map_or(BOT_STAND_DIMS, |n| n.dims());
        [0, 1, 2].map(|i| d[i].max(BOT_STAND_DIMS[i]).max(have[i]))
    }

    /// BOT-121: the next point of the bot's way from `pos` to `goal` over the grid, `None` where the grid gives no
    /// way (no grid, off it, or on an island of it: the greedy way-finder walks). Once per navigation step (50 ms)
    /// the bot's place joins the grid, the grid grows between the bot and the goal while no whole way is known, and
    /// a plan may be made; the rules are the co-op AI's (`dc_nav::follow`, FT-S11).
    // DC-DEFAULT: BOT-121 (proposed)
    fn nav_way(&mut self, now: Millis, pos: [f32; 3], goal: [f32; 3]) -> Option<[f32; 3]> {
        use crate::nav::follow;
        self.nav.as_ref()?;
        let t0 = std::time::Instant::now();
        // the grid follows the body's box (it arrives with the first teleport)
        let dims = self.nav_dims();
        if self.nav.as_ref().is_some_and(|n| n.dims() != dims) {
            if let Some(c) = self.nav_collision() {
                let (deadly, hazard) = self.nav_volumes();
                self.nav = Some(
                    crate::nav::NavGrid::new(c, dims)
                        .with_kill_volumes(deadly)
                        .with_hazard_volumes(hazard)
                        .with_ladders(&self.nav_ladders()),
                );
                self.nav_reset();
                self.follow.forget_refused();
            }
        }
        // a teleport (a spawn, the server's correction) starts the way afresh
        let flat = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).hypot(a[2] - b[2]);
        if self
            .nav_last_pos
            .is_some_and(|p| flat(p, pos) > NAV_TELEPORT)
        {
            self.nav_reset();
            // BOT-162: a new life (or a move elsewhere): the places jumped from are forgotten
            self.follow.forget_places();
        }
        self.nav_last_pos = Some(pos);
        let step = self.nav_cost.steps == 0 || since(now, self.nav_step) >= follow::STEP_MS;
        let nav = self.nav.as_mut()?;
        if step {
            self.nav_step = now;
            self.follow.grow(nav, pos, crate::nav::follow::GROW_PROBES);
        }
        // between two steps the bot walks toward the point the last step gave (the co-op AI asks once per step too)
        let flat = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).hypot(a[2] - b[2]);
        let out = match self.nav_cache {
            Some((g, out)) if !step && flat(g, goal) <= follow::TARGET_MOVED => out,
            _ => self.follow.next(nav, now, pos, goal, step),
        };
        self.nav_cache = Some((goal, out));
        let us = t0.elapsed().as_micros() as u64;
        let c = &mut self.nav_cost;
        if step {
            c.step_max_us = c.step_max_us.max(c.step_us);
            c.step_us = 0;
            c.steps += 1;
        }
        c.step_us += us;
        c.busy_us += us;
        match out {
            follow::NavGoal::Way(w) => Some(w),
            follow::NavGoal::Hold | follow::NavGoal::Steer => None,
        }
    }

    /// BOT-121: the greedy way-finder's walk from the end of a way short of the goal (or from an island of the grid):
    /// `way` is the follower's next point; while the walk lasts ([`NAV_END_GREEDY_MS`] from the place it began) a way
    /// that leads back within [`NAV_END_NEAR`] of that place is not taken (before, the bot went to and fro between the
    /// place and the greedy way-finder's first steps). Returns the way to take.
    // DC-DEFAULT: BOT-121 (proposed)
    fn nav_edge(&mut self, now: Millis, pos: [f32; 3], way: Option<[f32; 3]>) -> Option<[f32; 3]> {
        let walking = self
            .nav_end
            .is_some_and(|(t, _)| since(now, t) < NAV_END_GREEDY_MS);
        if !walking {
            self.nav_end = None;
        }
        if way.is_some() && !(walking && self.nav_same_end()) {
            self.nav_end = None;
            return way;
        }
        let short = self.follow.at_end() || self.follow.island() || walking;
        if self.nav.is_some() && short && self.nav_end.is_none() {
            self.nav_end = Some((now, pos));
        }
        None
    }

    /// BOT-121: whether the way planned is short of the goal and ends within [`NAV_END_NEAR`] of the end place the
    /// bot already stood at (the grid has not found more since).
    fn nav_same_end(&self) -> bool {
        let flat = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).hypot(a[2] - b[2]);
        match (self.nav_end, self.follow.points().last()) {
            (Some((_, p)), Some(e)) => !self.follow.whole() && flat(p, *e) <= NAV_END_NEAR,
            _ => false,
        }
    }

    /// BOT-129: the length of the bot's planned way from `pos` to `goal` when the way is whole and ends at that goal
    /// (`None`: no such way).
    fn nav_way_left(&self, pos: [f32; 3], goal: [f32; 3]) -> Option<f32> {
        let flat = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).hypot(a[2] - b[2]);
        let pts = self.follow.points();
        let (first, last) = (pts.first()?, pts.last()?);
        if !self.follow.whole() || flat(*last, goal) > crate::nav::follow::TARGET_MOVED {
            return None;
        }
        let along: f32 = pts.windows(2).map(|w| flat(w[0], w[1])).sum();
        Some(flat(pos, *first) + along + flat(*last, goal))
    }

    /// BOT-153: whether the body is blocked by the world along `steer`: its walk run on a copy for
    /// [`BLOCK_PROBE_SECS`] moves less than [`BLOCK_MOVE`] (flat). The copy has no players in its world, so a body slowed
    /// by a team mate, on a slope or by the server's corrections is free. True off the ROM world.
    fn body_blocked(&self, steer: Option<f32>) -> bool {
        match (self.body.as_ref(), steer) {
            (Some(b), Some(y)) if b.rom_world => {
                let (p, e) = (b.pos(), b.probe_walk(y, BLOCK_PROBE_SECS));
                (p[0] - e[0]).hypot(p[2] - e[2]) < BLOCK_MOVE
            }
            _ => true,
        }
    }

    /// BOT-153: whether the follower's stuck jump (BOT-121/141) is worth taking: a face jump (BOT-141) unless a team
    /// mate stands on the top (within [`MATE_ON_TOP`] flat and a body height above its point), else only when the next
    /// point lies more than a step above the body or the body is blocked by the world ([`Self::body_blocked`]).
    fn jump_needed(&self, now: Millis, steer: Option<f32>) -> bool {
        let Some(why) = self.follow.last_jump else {
            return true;
        };
        let next = self.follow.points().first().copied();
        if why.face {
            let on_top = next.is_some_and(|w| {
                self.spread_mates(now).iter().any(|(_, m)| {
                    (m[0] - w[0]).hypot(m[2] - w[2]) <= MATE_ON_TOP && (m[1] - w[1]).abs() <= MATE_ON_TOP
                })
            });
            return !on_top;
        }
        why.rise > crate::nav::STEP_UP || self.body_blocked(steer)
    }

    /// BOT-134: whether a jump now (walking along `steer`, or the evasive move `evade`) keeps the body out of danger:
    /// its flight and landing run on a copy (`evade::jump_safe`). True off the ROM world.
    fn jump_ok(&self, steer: Option<f32>, evade: Option<crate::evade::Move>) -> bool {
        match (self.body.as_ref(), self.world.as_ref()) {
            (Some(b), Some(w)) if b.rom_world => crate::evade::jump_safe(
                b,
                &w.collision,
                if self.in_hazard {
                    &self.kill_only
                } else {
                    &self.kill_now
                },
                steer,
                evade,
            ),
            _ => true,
        }
    }

    /// BOT-155: whether the body stands in danger only by a hazard volume (it hurts, it does not kill): then the
    /// move tests (jump, greedy probe, evasive steps, edge step) take the kill volumes alone. Before, a bot in
    /// Warhead's nook by the placed case at (-892, 2621, 445) (inside a hazard volume the server does not hurt
    /// with) found every move and jump unsafe and stood there 100..340 s.
    // DC-DEFAULT: BOT-155 (proposed)
    fn hazard_check(&mut self, now: Millis) {
        let was = self.in_hazard;
        self.in_hazard = match (self.body.as_ref(), self.world.as_ref()) {
            (Some(b), Some(w)) if b.rom_world && self.kill_only.len() < self.kill_now.len() => {
                let (dims, pos) = (b.dims(), b.pos());
                crate::nav::in_danger(&w.collision, &self.kill_now, dims, pos)
                    && !crate::nav::in_danger(&w.collision, &self.kill_only, dims, pos)
            }
            _ => false,
        };
        if self.in_hazard && !was {
            let pos = self.body.as_ref().map(|b| b.pos());
            self.rec
                .event("in_hazard", json!({"ms": self.ms(now), "pos": pos}));
        }
    }

    /// BOT-145: one stuck event (a watchdog trip, a stuck re-plan) at `pos`: the third within [`LOST_NEAR`] of the
    /// first and within [`LOST_WINDOW_MS`] of it gives the walk goal up ([`Self::nav_lose`]).
    fn nav_trouble(&mut self, now: Millis, pos: [f32; 3], why: &'static str) {
        let flat = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).hypot(a[2] - b[2]);
        let t = match self.trouble {
            Some((p, t0, n)) if flat(p, pos) <= LOST_NEAR && since(now, t0) <= LOST_WINDOW_MS => {
                (p, t0, n + 1)
            }
            _ => (pos, now, 1),
        };
        self.trouble = Some(t);
        if t.2 >= LOST_TRIPS {
            self.nav_lose(now, pos, why);
        }
    }

    /// BOT-145: the bot at `pos` gives its walk goal up for [`LOST_MS`]: it walks to a grid node it has a way to, at
    /// least [`LOST_MIN`] away (else the farthest such node), else back along the way its body came; event `lost`.
    fn nav_lose(&mut self, now: Millis, pos: [f32; 3], why: &'static str) {
        self.trouble = None;
        // BOT-159: on a small island of the grid there is no place far enough to walk to: the bot takes a way off it
        if self.nav_escape(now, pos, why) {
            return;
        }
        let mut x = self.roam_seed.max(1);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.roam_seed = x;
        let (g, how) = match self
            .nav
            .as_mut()
            .and_then(|nav| nav.reachable_far(pos, LOST_MIN, x))
        {
            Some(g) => (Some(g), "grid"),
            None => (self.follow.trail_back(pos), "trail"),
        };
        self.rec.event(
            "lost",
            json!({"ms": self.ms(now), "pos": pos, "why": why, "goal": g, "how": how}),
        );
        let Some(g) = g else {
            return;
        };
        self.lost = Some((g, now));
        self.roam = None;
        self.nav_reset();
        self.nav_end = None;
        self.walk_check = None;
        self.hunter.reset();
    }

    /// BOT-159: when the body at `pos` is confined to a small island of the grid (every node its links lead to lies
    /// within [`LOST_MIN`]: `NavGrid::confined_part`), the way off it (`evade::escape_move`, a fall of at most
    /// `nav::ESCAPE_DROP`) is taken for [`ESCAPE_MS`] whatever the goal; event `escape` (`found` false: none, the
    /// goal is given up as BOT-145 says). Returns whether a way off was found.
    // DC-DEFAULT: BOT-159 (proposed)
    fn nav_escape(&mut self, now: Millis, pos: [f32; 3], why: &'static str) -> bool {
        let Some(part) = self
            .nav
            .as_mut()
            .and_then(|nav| nav.confined_part(pos, LOST_MIN))
        else {
            return false;
        };
        let e = match (self.body.as_ref(), self.world.as_ref()) {
            (Some(b), Some(w)) if b.rom_world => crate::evade::escape_move(
                b,
                &w.collision,
                if self.in_hazard {
                    &self.kill_only
                } else {
                    &self.kill_now
                },
                &part,
                self.health.map_or(crate::nav::ESCAPE_DROP, |h| {
                    (FALL_SAFE + (h - ESCAPE_HEALTH_LEFT).max(0.0) / FALL_DAMAGE_PER_UNIT)
                        .min(crate::nav::ESCAPE_DROP)
                }),
            ),
            _ => None,
        };
        self.rec.event(
            "escape",
            json!({"ms": self.ms(now), "pos": pos, "why": why, "nodes": part.len(), "found": e.is_some(),
                   "health": self.health,
                   "yaw": e.map(|e| e.yaw), "jump": e.map(|e| format!("{:?}", e.jump)), "fall": e.map(|e| e.fall),
                   "end": e.map(|e| e.end)}),
        );
        let Some(e) = e else {
            return false;
        };
        self.escape = Some((e, now));
        self.escape_jumped = false;
        self.escape_trail = None;
        self.lost = None;
        self.nav_end = None;
        self.edge_since = None;
        self.walk_check = None;
        self.hunter.reset();
        self.obs.escapes += 1;
        true
    }

    /// BOT-159: the bot at `pos` with its goal at `goal`: on a small island of the grid (`NavGrid::confined_part`)
    /// with the goal off it for [`CONFINED_MS`], it takes a way off ([`Self::nav_escape`], `why` "confined"); again
    /// after as long when none was found.
    // DC-DEFAULT: BOT-159 (proposed)
    fn confined_tick(&mut self, now: Millis, pos: [f32; 3], goal: [f32; 3]) {
        if self.escape.is_some() || since(now, self.confined_check) < CONFINED_CHECK_MS {
            return;
        }
        self.confined_check = now;
        let off = self
            .nav
            .as_mut()
            .and_then(|nav| nav.confined_part(pos, LOST_MIN))
            .is_some_and(|part| {
                part.iter()
                    .all(|q| (q[0] - goal[0]).hypot(q[2] - goal[2]) > CONFINED_GOAL)
            });
        if !off {
            self.confined_since = None;
            return;
        }
        let t0 = *self.confined_since.get_or_insert(now);
        if since(now, t0) >= CONFINED_MS {
            self.confined_since = Some(now);
            self.nav_escape(now, pos, "confined");
        }
    }

    /// BOT-134: the watchdog trips (the body at `pos` stayed within `moved` units for [`WATCH_MS`] while it wants to walk):
    /// a jump when it is safe, the way is dropped and planned again (a second trip at the same place is a trap,
    /// BOT-133), the greedy way-finder starts afresh; event `watchdog` with the arm that steered. Returns the
    /// evasive move of this tick (with the jump).
    fn watchdog_trip(
        &mut self,
        now: Millis,
        pos: [f32; 3],
        goal: Option<[f32; 3]>,
        moved: f32,
        steer: Option<f32>,
        evade: Option<crate::evade::Move>,
    ) -> Option<crate::evade::Move> {
        let mut m = evade.unwrap_or(crate::evade::Move {
            yaw: self.body.as_ref().map_or(0.0, |b| b.angles[1]),
            strafe: 0,
            back: false,
            jump: false,
            crouch: false,
            push: false,
            note: None,
        });
        m.jump = true;
        let jumped = self.jump_ok(steer, Some(m));
        let trap = match self.nav.as_mut() {
            Some(nav) => self.follow.kick(nav, now, pos),
            None => false,
        };
        self.nav_cache = None;
        self.nav_end = None;
        self.walk_check = None;
        self.hunter.reset();
        self.watch = Some((now, pos));
        self.obs.watchdog_trips += 1;
        self.nav_trouble(now, pos, "watchdog");
        self.rec.event(
            "watchdog",
            json!({"ms": self.ms(now), "pos": pos, "goal": goal, "arm": self.branch, "moved": moved,
                   "jumped": jumped, "trap": trap, "backing": self.follow.backing(),
                   "trip": self.obs.watchdog_trips}),
        );
        if jumped {
            Some(m)
        } else {
            evade
        }
    }

    /// BOT-121: the bot's way starts afresh (a new grid, a teleport, no goal).
    fn nav_reset(&mut self) {
        self.follow.clear();
        self.nav_cache = None;
    }

    /// BOT-121: the `nav` event: the grid, the follower's counters and the time spent.
    fn nav_event(&mut self, now: Millis) {
        let Some(nav) = self.nav.as_ref() else {
            return;
        };
        let (s, c) = (&self.follow.stats, &self.nav_cost);
        self.rec.event(
            "nav",
            json!({"ms": self.ms(now), "nodes": nav.len(), "probes": nav.probes, "plans": s.plans,
                   "whole": s.whole, "failed": s.failed, "islands": s.islands, "jumps": s.jumps,
                   "stuck_replans": s.stuck_replans, "refusals": s.refusals,
                   "refusals_kept": s.refusals_kept, "traps": s.traps,
                   "place_spent": s.place_spent, "out_of_reach": s.out_of_reach,
                   "waypoints": self.follow.points().len(),
                   "busy_us": c.busy_us, "steps": c.steps, "step_max_us": c.step_max_us,
                   "plan_max_us": s.plan_max_us, "grow_max_us": s.grow_max_us,
                   "role": format!("{:?}", self.role)}),
        );
    }

    // SPEC: UDP_SESSION_2009_B.md §6.2, §7.1 (shell dispatch); GAME_MESSAGES_2009_S2C.md §3-§5
    fn on_shell(&mut self, now: Millis, shell: u8, body: &BitWriter) {
        if self.cfg.queue_events && RAW_SHELL_EVENTS.contains(&shell) {
            self.event(GameEvent::Shell {
                shell,
                body: body.to_bytes(),
                bits: body.bit_len(),
            });
        }
        let mut r = body.reader();
        match shell {
            shell_id::SWITCH_STATE => {
                let s = r.read_u8();
                self.switch_state = Some(s);
                self.obs.switch_states.push(s);
                self.mark(now, "S11_switch_state");
            }
            shell_id::GS_PLAYER_STATE => {
                let s = r.read_u8();
                let was = self.life_state;
                self.life_state = s;
                self.obs.life_states.push(s);
                self.mark(now, "S12_player_state");
                self.event(GameEvent::PlayerState(s));
                let ev = self.respawn.on_state(now, s);
                self.log_life(now, ev);
                if let Some(sh) = self.shooter.as_mut() {
                    sh.on_life(now, s);
                }
                if s == 1 && was != 1 {
                    if self.body.is_some() && matches!(was, 2 | 3 | 5 | 6) {
                        // back from a death: the 0xa0 teleport already moved the body; stand there
                        self.body_clock = now;
                        self.next_update = now;
                        self.last_aim = None;
                        self.facing = None;
                    } else {
                        self.spawn_body(now);
                    }
                }
            }
            shell_id::GS_IDENTIFY_REQUEST => {
                // SPEC: GAME_MESSAGES_2009_C2S.md §5.1 (answer 0x72, then 0x85 at once)
                self.mark(now, "info_request");
                let info = shell::player_info_body(&self.cfg.info);
                self.send_shell(true, shell_id::UP_PLAYER_INFO, &info, now);
                let (a, b) = self.cfg.client_options;
                self.send_shell(
                    true,
                    shell_id::UP_CLIENT_OPTIONS,
                    &shell::client_options_body(a, b),
                    now,
                );
                self.mark(now, "info_sent");
            }
            shell_id::GS_RELOAD_WEAPON_TABLE => {
                let (a, b) = self.cfg.client_options;
                self.send_shell(
                    true,
                    shell_id::UP_CLIENT_OPTIONS,
                    &shell::client_options_body(a, b),
                    now,
                );
            }
            shell_id::GS_GAME_ENDPOINT => {
                let (mc, ep) = shell::read_game_endpoint(&mut r);
                self.obs.mode_class = Some(mc);
                if let Some(s) = self.shooter.as_mut() {
                    s.set_mode_class(mc);
                }
                self.rec
                    .event("mode_class", json!({"mode_class": mc, "endpoint": ep}));
            }
            shell_id::INTEGRITY => {
                // SPEC: GAME_MESSAGES_2009_C2S.md §5.6
                let ask = shell::read_integrity(&mut r);
                self.obs.integrity.push(format!("{ask:?}"));
                match ask {
                    IntegrityAsk::Version(0x100) => {
                        let v = self.cfg.integrity_version.unwrap_or(0);
                        self.send_shell(true, shell_id::INTEGRITY, &shell::integrity_reply_body(v, 0), now);
                    }
                    IntegrityAsk::Version(c) => self.obs.parse_notes.push(format!(
                        "integrity check word {c:#x}: a 2009 client records disconnect reason 1"
                    )),
                    IntegrityAsk::Nonce(n) => self.obs.parse_notes.push(format!(
                        "integrity nonce challenge {n:#x}: the 2009 answer needs client checksums the bot does not compute"
                    )),
                    IntegrityAsk::Other(s) => self.obs.parse_notes.push(format!("integrity sub {s}")),
                }
            }
            shell_id::GS_PLAYER_INFO => {
                let p = shell::read_player_info(&mut r);
                if !p.complete {
                    self.obs
                        .parse_notes
                        .push(format!("0x6b for pid {} shorter than its layout", p.pid));
                }
                let own = self.obs.client_id.map(|c| c.0 as u8);
                self.rec.event(
                    "roster",
                    json!({"pid": p.pid, "name": p.name, "character": p.character, "team": p.team,
                           "kind": p.kind, "own": Some(p.pid) == own}),
                );
                if self.cfg.roster_query
                    && p.character != 0xFFFF
                    && !self.table.objects.contains_key(&p.character)
                    && !self.roster_pending.contains(&p.pid)
                {
                    self.roster_pending.push(p.pid);
                }
                self.obs.roster.insert(p.pid, p);
                self.mark(now, "roster_record");
            }
            shell_id::GS_PLAYER_LEFT => {
                let pid = r.read_u8();
                if let Some(p) = self.obs.roster.remove(&pid) {
                    self.obs.departed.push(p);
                }
                if let Some(sh) = self.shooter.as_mut() {
                    sh.on_left(pid);
                }
            }
            // DC-DEFAULT: BOT-100 (proposed) a Possession move: no shots until the closing teleport
            dc_wire::possession::SHELL_ID => self.gliding = true,
            shell_id::GS_MOVER_UPDATE => {
                let m = shell::read_mover_update(&mut r);
                if !m.complete {
                    self.obs.parse_notes.push(format!(
                        "0xa0 flags {:#06x} shorter than its layout",
                        m.flags
                    ));
                }
                if let Some(t) = m.teleport {
                    self.gliding = false;
                    self.obs.teleports.push(t);
                    self.mark(now, "teleport");
                    // every teleport with its position in the event log (the spawn-set evidence of a bot match)
                    let ms = self.ms(now);
                    self.rec
                        .event("teleport", json!({"ms": ms, "seq": t.0, "pos": t.1}));
                    self.event(GameEvent::Teleport {
                        seq: t.0,
                        pos: t.1,
                        dims: t.2,
                    });
                    if !self.cfg.external_body {
                        match self.body.as_mut() {
                            Some(b) => b.teleport(t.0, t.1, t.2),
                            None => self.pending_teleport = Some(t),
                        }
                    }
                }
                if m.friction.is_some() || m.freeze.is_some() || m.release.is_some() {
                    self.event(GameEvent::Mover {
                        friction: m.friction,
                        freeze: m.freeze,
                        release: m.release,
                    });
                }
            }
            shell_id::GS_VIEW_ANGLES => {
                let (k, a) = shell::read_view_angles(&mut r);
                self.event(GameEvent::ViewAngles { kind: k, angles: a });
                let angles = if k == 1 {
                    let mut cur = self.body.as_ref().map_or([0.0; 3], |b| b.angles);
                    cur[1] = a[1];
                    cur
                } else {
                    a
                };
                match self.body.as_mut() {
                    Some(b) => b.angles = angles,
                    None => self.pending_angles = Some(angles),
                }
            }
            lifecycle::GS_COOP => {
                // SPEC: GAME_MESSAGES_2009_S2C.md §3.5 (0xd7 sub 2 [a:u8][b:u8] arms the respawn wait when a == b)
                if r.read_u8() == 2 {
                    let (a, b) = (r.read_u8(), r.read_u8());
                    let ev = self.respawn.on_coop_sub2(now, a, b);
                    self.log_life(now, ev);
                }
            }
            lifecycle::GS_RESPAWN_TOKEN => {
                // SPEC: GAME_MESSAGES_2009_S2C.md (0xdb [k:u8][sub:u8], k = 0; sub 6 [n:u32] arms the wait)
                if r.read_u8() == 0 && r.read_u8() == 6 {
                    let n = r.read_u32();
                    let ev = self.respawn.on_token_sub6(now, n);
                    self.log_life(now, ev);
                }
            }
            shell_id::GS_KILL => {
                // SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 (fields 1-16; the 151-bit form of
                // CAPTURE_tdm4_KILL_VALIDATION.md §3.2 decodes with no bit left over)
                let k = shell::read_kill(&mut r);
                if !k.complete {
                    self.obs.parse_notes.push(format!(
                        "0x6d for victim {} shorter than its layout",
                        k.victim
                    ));
                }
                self.obs.kills.push((k.victim, k.killer));
                // BOT-172: a team mate killed: where (a knife run near it ends)
                let own_pid = self.obs.client_id.map(|c| c.0 as u8);
                let mate = Some(k.victim) != own_pid
                    && self.own_team().is_some_and(|t| {
                        self.obs.roster.get(&k.victim).is_some_and(|r| r.team == t)
                    });
                if let Some(p) = self
                    .obs
                    .roster
                    .get(&k.victim)
                    .and_then(|r| self.table.objects.get(&r.character))
                    .and_then(|o| o.pos)
                    .filter(|_| mate)
                {
                    self.knife.mate_down = Some((now, p));
                }
                // the victim's roster record is dead until the server's next record of it (the spawn refresh), as
                // the 2009 client's 0x6d sets the record's dead flag; BOT-124: the team jobs count the living by it
                // SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 field 13 (record +0x74, set to 1 by 0x6d)
                if let Some(rec) = self.obs.roster.get_mut(&k.victim) {
                    rec.dead = true;
                }
                self.rec.event(
                    "kill",
                    json!({"ms": self.ms(now), "victim": k.victim, "killer": k.killer, "kill_kind": k.kill_kind,
                           "weapon": k.weapon, "hit_zone": k.hit_zone, "f_relation": k.relation,
                           "f_feed2": k.feed2, "keep_hud": k.keep_hud, "f_feed6": k.feed6,
                           "streak_flag": k.streak_flag, "feed_value": k.feed_value, "no_count": k.no_count,
                           "revenge": k.revenge, "revenge_done": k.revenge_done, "coop": [k.coop.0, k.coop.1],
                           "own_pid": self.obs.client_id.map(|c| c.0)}),
                );
                if self.shooter.is_some() {
                    let note = weapon::KillNote::from(&k);
                    let view = self.shooter_view(now);
                    let acts = self
                        .shooter
                        .as_mut()
                        .map(|sh| sh.on_kill(now, &note, &view));
                    self.run_actions(now, acts.unwrap_or_default());
                }
                self.event(GameEvent::Kill(k));
            }
            weapon::gs::WEAPON_GIVE | weapon::gs::WEAPON_EQUIP => {
                // SPEC: GAME_MESSAGES_2009_S2C.md §4.4
                let g = weapon::read_weapon_grant(&mut r, shell == weapon::gs::WEAPON_EQUIP);
                self.rec.event(
                    "weapon_grant",
                    json!({"ms": self.ms(now), "equip": g.equip, "weapon": g.weapon, "ammo_kind": g.ammo_kind,
                           "ammo": g.ammo, "clip": g.clip, "class_pos": g.class_pos, "slot": g.slot,
                           "parts": g.parts}),
                );
                if let Some(sh) = self.shooter.as_mut() {
                    sh.on_grant(g);
                }
                self.bomb.on_grant(g.slot, g.weapon, g.ammo_kind);
                self.event(GameEvent::WeaponGrant(g));
            }
            weapon::gs::WEAPON_REMOVE => {
                let slot = r.read_u8();
                if let Some(sh) = self.shooter.as_mut() {
                    sh.on_remove(slot);
                }
                self.bomb.on_remove(slot);
                self.event(GameEvent::WeaponRemove { slot });
            }
            weapon::gs::WEAPON_SWITCH => {
                // SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (0x7f `[command:u8][auto:u8][ammo:f32][last:u8]`)
                let command = r.read_u8();
                let auto = r.read_u8() != 0;
                let ammo = r.read_f32();
                let last = r.read_u8();
                self.event(GameEvent::WeaponSwitch {
                    command,
                    auto,
                    ammo,
                    last,
                });
            }
            weapon::gs::DAMAGE_TAKEN => {
                // SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (0x74)
                let d = weapon::read_damage(&mut r);
                self.rec.event(
                    "damage",
                    json!({"ms": self.ms(now), "damage": d.damage, "zone": d.zone, "dmg_kind": d.dmg_kind,
                           "mode": d.mode, "ammo_kind": d.ammo_kind, "part": d.part, "from_dir": d.from_dir}),
                );
                self.evader.on_damage(now);
                if d.damage > 0 {
                    self.last_hurt = Some(now);
                }
                // BOT-143: a hit slows an infected body for a moment as the client's does (CLI-1760); the server's
                // poison type is a hazard volume: the bot leaves it
                if self.qrt_override.is_some()
                    && d.damage > 0
                    && d.dmg_kind != crate::qrt::FALL_DAMAGE_KIND
                {
                    self.shot_slow_until = Some(now.wrapping_add(crate::qrt::SHOT_SLOW_MS));
                }
                if d.zone == crate::qrt::HAZARD_DAMAGE_TYPE {
                    self.hazard_hit = Some(now);
                }
                self.event(GameEvent::Damage(d));
            }
            shell_id::GS_PLAYER_STATUS => {
                // SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (0x66: 2 health, 3 armour, 0x0f / 0x10 maxima, 1 ammo,
                // 0x14 money)
                let st = shell::read_player_status(&mut r);
                match st {
                    PlayerStatus::Value { kind, value, .. } => {
                        // BOT-159: the own health (kind 2) bounds the fall of a way off an island
                        if kind == 2 {
                            self.health = Some(value);
                        }
                        self.rec.event(
                            "status",
                            json!({"ms": self.ms(now), "kind": kind, "value": value}),
                        )
                    }
                    PlayerStatus::Ammo {
                        weapon,
                        ammo_kind,
                        ammo,
                        slot,
                        ..
                    } => self.rec.event(
                        "status",
                        json!({"ms": self.ms(now), "kind": 1, "weapon": weapon, "ammo_kind": ammo_kind,
                               "ammo": ammo, "slot": slot}),
                    ),
                    PlayerStatus::Money { .. } => {}
                }
                self.event(GameEvent::Status(st));
            }
            shell_id::GS_TEAM_SCORE => {
                let p = shell::read_team_pair(&mut r);
                self.event(GameEvent::TeamScore(p));
            }
            shell_id::GS_ROUND_WINS => {
                let p = shell::read_team_pair(&mut r);
                self.event(GameEvent::RoundWins(p));
            }
            shell_id::GS_MATCH_BEGIN => {
                let (round_secs, _intro, waiting_banner, _countdown) =
                    shell::read_match_begin_head(&mut r);
                self.event(GameEvent::MatchBegin {
                    round_secs,
                    waiting_banner,
                });
            }
            // DC-DEFAULT: BOT-70 (proposed) the bomb state of Search and Destroy
            crate::bombrun::GS_OBJECTIVE_ITEM => {
                self.bomb.carrier = crate::bombrun::read_objective_item(body);
                let me = self.bomb.carrier.is_some() && self.bomb.carrier == self.obs.view_object;
                self.rec.event(
                    "bomb_carrier",
                    json!({"ms": self.ms(now), "obj": self.bomb.carrier, "me": me}),
                );
            }
            crate::bombrun::GS_BOMB_PLANTED
            | crate::bombrun::GS_BOMB_DEFUSED
            | crate::bombrun::GS_BOMB_EXPLODED => {
                // BOT-136: the bomb timer of the 0x51 (GAME_MESSAGES_2009_S2C.md row 0x51: u32 seconds) times the
                // defenders' run from the blast
                if shell == crate::bombrun::GS_BOMB_PLANTED && !self.bomb.planted {
                    let secs = r.read_u32();
                    let secs = if secs > 0 && secs < 600 {
                        secs
                    } else {
                        crate::bombrun::BOMB_SECS_DEFAULT
                    };
                    self.bomb.fuse_end = Some(now.wrapping_add(secs * 1000));
                }
                self.bomb.planted = true;
                self.bomb.hold = None;
                self.rec
                    .event("bomb", json!({"ms": self.ms(now), "id": shell}));
            }
            shell_id::GS_ROUND_BEGIN => {
                self.bomb.reset();
                let (round_secs, round_no, round_limit, _) = shell::read_round_begin(&mut r);
                self.bomb.round = round_no;
                self.event(GameEvent::RoundBegin {
                    round_secs,
                    round_no,
                    round_limit,
                });
            }
            shell_id::GS_ROUND_TIMER_STOP => {
                let s = r.read_u32();
                self.event(GameEvent::RoundTimerStop(s));
            }
            shell_id::GS_ROUND_RESULT => {
                let (_, win_team, w1, w2) = shell::read_round_result_head(&mut r);
                self.mark(now, "round_result");
                self.respawn.on_round_over();
                // BOT-115: the victims of the round come back with the next one
                if let Some(s) = self.shooter.as_mut() {
                    s.on_round_result();
                }
                self.event(GameEvent::RoundResult {
                    win_team,
                    wins: [w1, w2],
                });
            }
            shell_id::GS_PLAYER_SUMMARY => {
                let row = shell::read_player_summary(&mut r);
                self.event(GameEvent::Summary(row));
            }
            shell_id::GS_MATCH_RESULT_SCREEN => {
                // SPEC: GAME_MESSAGES_2009_S2C.md §2.1 (0x3f opens the result screen and sets the leaving latches);
                // GAME_MESSAGES_2009_C2S.md Appendix A step A13 (the engine disconnect follows)
                let (flag, value) = shell::read_result_screen(&mut r);
                self.mark(now, "result_screen");
                self.rec.event(
                    "result_screen",
                    json!({"ms": self.ms(now), "result_flag": flag, "result_value": value,
                           "leave_in_ms": self.cfg.leave_after_result.then_some(lifecycle::LEAVE_AFTER_RESULT_MS)}),
                );
                if self.cfg.leave_after_result && self.leave_at.is_none() {
                    self.leave_at = Some(now.wrapping_add(lifecycle::LEAVE_AFTER_RESULT_MS));
                }
                self.event(GameEvent::ResultScreen { flag, value });
            }
            shell_id::GS_LEVEL_EXIT => {
                let (next_world, keep, at_once) = shell::read_level_exit(&mut r);
                self.mark(now, "level_exit");
                self.event(GameEvent::LevelExit {
                    next_world,
                    keep,
                    now: at_once,
                });
            }
            // DC-DEFAULT: BOT-75 (proposed) the intel the bot holds
            crate::spyrun::GS_BOUNTY_MODE => {
                let own = self.obs.client_id.map(|c| u32::from(c.0));
                let seen = |s: &crate::spyrun::SpyRun| (s.held, s.total, s.agent());
                let before = seen(&self.spy);
                self.spy.on_bounty(body, own);
                if seen(&self.spy) != before {
                    self.rec.event(
                        "spy",
                        json!({"ms": self.ms(now), "held": self.spy.held, "total": self.spy.total,
                               "agent": self.spy.agent()}),
                    );
                }
            }
            // DC-DEFAULT: BOT-66 (proposed) the flag the bot carries
            crate::flagrun::GS_OBJECTIVE_STATE => {
                if let Some((team, carrier, event)) = crate::flagrun::read_objective_state(body) {
                    let own = self.obs.client_id.map(|c| u32::from(c.0));
                    self.carrying =
                        crate::flagrun::carrying_after(self.carrying, own, team, carrier, event);
                    // BOT-125: taken (2) or dropped (4) leaves the base; returned (3) or captured (1) is home again
                    match event {
                        2 | 4 => {
                            self.flag_away.insert(team);
                        }
                        1 | 3 => {
                            self.flag_away.remove(&team);
                        }
                        _ => {}
                    }
                    // BOT-138: a dropped flag lies loose until it is taken, returned or sent home
                    if event == 4 {
                        self.flag_loose.insert(team, now);
                    } else if matches!(event, 1..=3) {
                        self.flag_loose.remove(&team);
                    }
                    if event == 2 && carrier != u32::MAX {
                        self.flag_carrier.insert(team, carrier);
                    } else if matches!(event, 1 | 3 | 4) {
                        self.flag_carrier.remove(&team);
                    }
                    self.rec.event(
                        "flag",
                        json!({"ms": self.ms(now), "team": team, "carrier": carrier, "event": event,
                               "carrying": self.carrying}),
                    );
                }
            }
            shell_id::GS_MATCH_SETTINGS => {
                // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1.5 (`own_value` := the shot counter)
                let seed = shell::read_match_shot_seed(&mut body.reader());
                if let (Some(v), Some(sh)) = (seed, self.shooter.as_mut()) {
                    sh.on_shot_seed(v);
                }
                let (team, goal, s1, s2, rl, rn) = shell::read_match_settings(&mut r);
                self.rec.event(
                    "match_settings",
                    json!({"own_team": team, "goal": goal, "scores": [s1, s2], "round_limit": rl, "round": rn}),
                );
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md 0xda row (kind 1: the shot counter += delta)
            shell_id::GS_PLAYER_POINTS => {
                if let (Some(d), Some(sh)) =
                    (shell::read_points_shot_seed(&mut r), self.shooter.as_mut())
                {
                    sh.on_shot_seed_add(d);
                }
            }
            // SPEC: GAME_MESSAGES_2009_S2C.md §3.2 (0xca `[allowed:bool1]` -> own record +0x1f0)
            shell_id::GS_SPRINT_ALLOWED => self.obs.sprint_allowed = Some(r.read_bit()),
            // BOT-143: Quarantine sub 3 `[row:i8]` is the own override row (-1: human again), whose speed keys the
            // client's mover uses (DSV-320)
            // SPEC: GAME_MESSAGES_2009_S2C.md §3.5 row 0xd3; GAME_MODES_2009.md §5.7
            crate::qrt::QRT_SHELL => match r.read_u8() {
                crate::qrt::QRT_SUB_OVERRIDE => {
                    let row = r.read_u8() as i8;
                    self.qrt_override = u32::try_from(row).ok();
                    self.shot_slow_until = None;
                    self.rec
                        .event("qrt_override", json!({"ms": self.ms(now), "row": row}));
                }
                // BOT-160: sub 0 `[team]([infector:u32] when infected)[pid:u32]` tells who is infected now, sub 1
                // `[round][limit][flag][secs]` starts a round's countdown (humans only): the outbreak state comes
                // from these, not from the bodies in view
                // SPEC: GAME_MODES_2009.md §5.7 (subs 0 and 1)
                crate::qrt::QRT_SUB_TEAM => {
                    let team = r.read_u8();
                    let infector = (team == crate::qrt::INFECTED).then(|| r.read_u32());
                    let pid = r.read_u32();
                    if let Ok(pid) = u8::try_from(pid) {
                        self.qrt_round.on_team(pid, team);
                        self.rec.event(
                            "qrt_team",
                            json!({"ms": self.ms(now), "pid": pid, "team": team,
                                   "host": infector == Some(crate::qrt::NO_INFECTOR)}),
                        );
                    }
                }
                crate::qrt::QRT_SUB_COUNTDOWN => {
                    let round = r.read_u8();
                    self.qrt_round.on_countdown(round);
                }
                _ => {}
            },
            shell_id::CONSOLE_TEXT => {
                let t = String::from_utf8_lossy(&r.read_zstr8()).into_owned();
                self.rec.event("console", json!({"text": t}));
                self.obs.console.push(t);
            }
            _ => {}
        }
    }

    /// The loading screen closes once the in-world test passes: `UP_WORLD_READY` once.
    // SPEC: GAME_MESSAGES_2009_C2S.md §5.4; UDP_SESSION_2009_A.md §8.3
    fn check_in_world(&mut self, now: Millis) {
        if self.is_over() {
            return;
        }
        if self.own_object().is_some() {
            self.mark(now, "S9_own_object");
        }
        let (engine, full) = self.in_world_test();
        if engine {
            self.mark(now, "engine_in_world");
        }
        if full && !self.world_ready_sent {
            self.world_ready_sent = true;
            self.stage = Stage::InWorld;
            self.send_shell(true, shell_id::UP_WORLD_READY, &BitWriter::new(), now);
            self.mark(now, "world_ready_sent");
        }
        if engine && self.cfg.force_updates && self.body.is_none() && !self.external_spawned {
            self.spawn_body(now);
        }
    }

    fn log_life(&mut self, now: Millis, ev: Option<LifeEvent>) {
        let ms = self.ms(now);
        match ev {
            Some(LifeEvent::Died { state }) => {
                self.rec.event("died", json!({"ms": ms, "state": state}));
                // BOT-163: a death by the case the bot walked to is a failed attempt at it
                if let Some(p) = self.body.as_ref().map(|b| b.pos()) {
                    self.spy.on_died(p, now);
                }
            }
            Some(LifeEvent::Armed { via, a, b }) => {
                self.rec.event(
                    "respawn_armed",
                    json!({"ms": ms, "via": via, "a": a, "b": b, "wait_ms": self.respawn.wait_ms()}),
                );
            }
            Some(LifeEvent::Respawned { after_ms }) => {
                // BOT-143: a new life starts with a full gauge and tells its sprint state again
                if let Some(b) = self.body.as_mut() {
                    b.refill_gauge();
                }
                self.sprint_pair = None;
                self.sprint_rest = false;
                self.hazard_hit = None;
                self.hazard_leave = None;
                let pos = self.body.as_ref().map(|b| b.pos());
                self.rec.event(
                    "respawned",
                    json!({"ms": ms, "after_ms": after_ms, "pos": pos}),
                );
            }
            Some(LifeEvent::Waiting { reason }) => {
                self.rec.event(
                    "respawn_wait",
                    json!({"ms": ms, "reason": reason, "mode_class": self.obs.mode_class}),
                );
            }
            None => {}
        }
    }

    /// Ends the session when the server went silent (no datagram for the configured idle time).
    fn check_idle(&mut self, now: Millis) {
        if self.is_over() || self.quit_at.is_some() || self.stage == Stage::Joining {
            return;
        }
        if let Some(silent) = self.idle.expired(now) {
            self.end_gone(now, format!("no datagram from the server for {silent} ms"));
        }
    }

    /// Ends the session cleanly because the other side is gone (`server_gone` event), sending the bye copies.
    pub fn end_gone(&mut self, now: Millis, why: String) {
        if self.is_over() {
            return;
        }
        self.rec
            .event("server_gone", json!({"why": why, "ms": self.ms(now)}));
        self.obs.end = Some(format!("server gone: {why}"));
        self.stage = Stage::Closed;
        if let Some(l) = self.link.as_mut() {
            l.close(now, Bye::Plain);
        }
    }

    fn spawn_pos(&self) -> [f32; 3] {
        if let Some(t) = self.pending_teleport {
            return t.1;
        }
        self.own_object()
            .and_then(|o| self.table.objects.get(&o))
            .and_then(|o| o.pos)
            .or_else(|| {
                self.world.as_ref().map(|w| {
                    let b = w.extents;
                    [
                        (b.min[0] + b.max[0]) * 0.5,
                        (b.min[1] + b.max[1]) * 0.5,
                        (b.min[2] + b.max[2]) * 0.5,
                    ]
                })
            })
            .unwrap_or([0.0; 3])
    }

    fn spawn_body(&mut self, now: Millis) {
        if self.cfg.external_body {
            // the caller's player: its own simulation, the teleports go to the event queue
            self.external_spawned = true;
            self.next_update = now;
            self.rec.event("spawn", json!({"external": true}));
            self.mark(now, "spawned");
            self.send_state_form(now);
            return;
        }
        let pos = self.spawn_pos();
        let dims = self.pending_teleport.map_or(BOT_STAND_DIMS, |t| t.2);
        let yaw = self.pending_angles.map_or(0.0, |a| a[1]);
        let coll = self.world.as_ref().map(|w| w.collision.clone());
        let mut b = Body::new(pos, dims, yaw, self.cfg.motion, self.cfg.sim_hz, coll);
        if b.rom_world {
            b.ladders = self.world.as_ref().map(|w| w.ladders.clone());
        }
        if let Some(t) = self.pending_teleport.take() {
            b.teleport(t.0, t.1, t.2);
        }
        if let Some(a) = self.pending_angles.take() {
            b.angles = a;
        }
        self.rec.event(
            "spawn",
            json!({"pos": b.pos(), "rom_collision": b.rom_world, "motion": format!("{:?}", self.cfg.motion)}),
        );
        self.body = Some(b);
        self.body_clock = now;
        self.next_update = now;
        self.mark(now, "spawned");
        self.send_state_form(now);
    }

    fn send_state_form(&mut self, now: Millis) {
        if let (Some(v), false) = (self.cfg.state_form, self.state_form_sent) {
            self.state_form_sent = true;
            self.send_shell(
                true,
                shell_id::UP_PLAYER_UPDATE,
                &shell::player_state_form_body(v),
                now,
            );
        }
    }

    /// What the shooter sees: own pid / team / position, the roster's players and their stream positions.
    fn shooter_view(&mut self, now: Millis) -> View {
        let own_pid = self.obs.client_id.map(|c| c.0 as u8);
        let team_ok = |t: u8| t == 1 || t == 2;
        let own_team = own_pid
            .and_then(|p| self.obs.roster.get(&p))
            .map(|r| r.team)
            .filter(|t| team_ok(*t))
            .or_else(|| {
                u8::try_from(self.cfg.info.team)
                    .ok()
                    .filter(|t| team_ok(*t))
            })
            // DC-DEFAULT: BOT-75 (proposed; amended by DSV-450, project decision) Spy Hunt is everyone
            // against everyone (DSV-236, DSV-450): no team filter, a spy shoots other spies too
            .filter(|_| self.obs.mode_class != Some(9));
        // DC-DEFAULT: BOT-127 (proposed) the damage matrix of DSV-453: the hunters are a team (a hunter's hit on a
        // hunter is refused as friendly), a spy or the agent hurts anyone; the spy side is the 0xcb holders
        let spy_room = self.obs.mode_class == Some(9);
        let own_team = if spy_room && self.spy.held == 0 {
            Some(crate::spyrun::HUNTER_TEAM)
        } else {
            own_team
        };
        let own_dims = self
            .obs
            .teleports
            .last()
            .map(|t| t.2)
            .filter(|d| d.iter().all(|x| *x > 0.0 && x.is_finite()))
            .unwrap_or(BOT_STAND_DIMS);
        let floor = self.table.extents.map(|e| e.min[1]);
        for p in self.obs.roster.values() {
            if let Some(o) = self.table.objects.get(&p.character) {
                let w = self
                    .stream_watch
                    .entry(p.character)
                    .or_insert((o.stream_entries, now));
                if w.0 != o.stream_entries {
                    *w = (o.stream_entries, now);
                }
            }
        }
        let players = self
            .obs
            .roster
            .values()
            .filter(|p| Some(p.pid) != own_pid)
            .map(|p| {
                let o = self.table.objects.get(&p.character);
                Seen {
                    pid: p.pid,
                    team: if spy_room {
                        self.spy.team_of(u32::from(p.pid))
                    } else {
                        p.team
                    },
                    obj: p.character,
                    pos: o.and_then(|o| o.pos),
                    dims: o.and_then(|o| o.dims),
                    silent_ms: self
                        .stream_watch
                        .get(&p.character)
                        .map_or(0, |w| since(now, w.1)),
                    // the park point lies on the bottom face of the world box (VIS-9, proposed)
                    parked: o
                        .and_then(|o| o.pos)
                        .zip(floor)
                        .is_some_and(|(pos, f)| pos[1] <= f + 2.0),
                    // compressed stream positions need the world box (BOT-25, proposed)
                    pos_known: o.is_some_and(|o| self.table.extents.is_some() || o.pos48.is_none()),
                }
            })
            .collect();
        // BOT-142: an attacker takes a defender seen defusing as its target whenever it sees him
        let prefer = (matches!(self.obs.mode_class, Some(2 | 3))
            && self.bomb.planted
            && self.bomb_attacker())
        .then(|| {
            let bomb = crate::bombrun::defuse_point(&self.table).map(|p| p.1);
            self.seen_defuser(now, bomb).map(|d| d.0)
        })
        .flatten()
        .or_else(|| self.qrt_focus())
        .or_else(|| self.spy_focus_pick(now));
        View {
            own_pid,
            own_team,
            own_pos: self.body.as_ref().map_or([0.0; 3], |b| b.pos()),
            own_dims,
            players,
            time_ms: self.ms(now),
            world: self.world.as_ref().map(|w| w.collision.clone()),
            facing: self.facing,
            moving: self.body.as_ref().map_or_else(Default::default, |b| {
                dc_cone::MoveState::of_controls(b.controls(), b.jumping())
            }),
            prefer,
            mates: self.spread_mates(now),
            rushers: self.qrt_rushers(),
        }
    }

    /// BOT-161: a Quarantine human fires full-auto (and at the head of one coming head-on) at every infected, as the
    /// server's messages and the roster name them.
    // DC-DEFAULT: BOT-161 (proposed)
    fn qrt_rushers(&self) -> Vec<u8> {
        use crate::qrt;
        if self.obs.mode_class != Some(qrt::QRT_CLASS) || self.own_team() != Some(qrt::HUMANS) {
            return Vec::new();
        }
        let mut v: Vec<u8> = self
            .obs
            .roster
            .values()
            .filter(|r| r.team == qrt::INFECTED)
            .map(|r| r.pid)
            .chain(self.qrt_round.infected.iter().copied())
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// BOT-148: the bot's living team mates (player id, streamed place: not parked, streamed within
    /// [`crate::shooter::UNSEEN_SILENT_MS`]) in the modes whose bots spread out (`spread::spreads`); empty elsewhere.
    // DC-DEFAULT: BOT-148 (proposed)
    fn spread_mates(&self, now: Millis) -> Vec<(u8, [f32; 3])> {
        let (Some(own_pid), Some(team)) = (self.obs.client_id.map(|c| c.0 as u8), self.own_team())
        else {
            return Vec::new();
        };
        if !crate::spread::spreads(self.obs.mode_class) {
            return Vec::new();
        }
        let floor = self.table.extents.map(|e| e.min[1]);
        self.obs
            .roster
            .values()
            .filter(|r| r.pid != own_pid && !r.dead && r.team == team)
            .filter(|r| {
                self.stream_watch
                    .get(&r.character)
                    .is_none_or(|w| since(now, w.1) <= crate::shooter::UNSEEN_SILENT_MS)
            })
            .filter_map(|r| {
                let p = self.table.objects.get(&r.character).and_then(|o| o.pos)?;
                floor.is_none_or(|f| p[1] > f + 2.0).then_some((r.pid, p))
            })
            .collect()
    }

    /// BOT-148: the short walk over the grid to a clear place near the bot (`spread::space_place`, the side of the
    /// look `yaw` first) that a fighting bot takes when its side step away from a team mate has run
    /// [`crate::spread::SPACE_AFTER_MS`] without clearing it (a wall, or both sides unsafe), and a walking bot takes when
    /// its wait behind a team mate finds the mate on top of it; it lasts until the bot is there or
    /// [`crate::spread::SPACE_MS`] passed. `start` (the yaw) picks a new place when none is under way.
    // DC-DEFAULT: BOT-148 (proposed)
    fn spread_space_goal(
        &mut self,
        now: Millis,
        pos: [f32; 3],
        start: Option<f32>,
    ) -> Option<[f32; 3]> {
        use crate::spread::{space_place, SPACE_MS, SPACE_REACHED, SPACE_SNAP};
        let flat = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).hypot(a[2] - b[2]);
        if let Some((g, at)) = self.spread_space {
            if since(now, at) < SPACE_MS && flat(g, pos) > SPACE_REACHED {
                return Some(g);
            }
            self.spread_space = None;
            return None;
        }
        let yaw = start?;
        let mates = self.spread_mates(now);
        let nav = self.nav.as_ref()?;
        let g = space_place(pos, yaw, &mates, |p| nav.safe_node(p, SPACE_SNAP))?;
        self.rec.event(
            "spread_space",
            json!({"ms": self.ms(now), "pos": pos, "goal": g}),
        );
        self.spread_space = Some((g, now));
        Some(g)
    }

    /// BOT-148: a hunt or roam goal a team mate stands at (`spread::push_aside`) is moved to a grid node beside it;
    /// the move is kept while the goal stays within [`crate::hunt::NEW_GOAL_DIST`] for up to [`SPREAD_ASIDE_MS`]
    /// (a goal left in place is looked at again after [`SPREAD_ASIDE_RECHECK_MS`]), and once the bot got within
    /// [`SPREAD_ASIDE_REACHED`] of the side place the goal itself counts again (until it moves).
    // DC-DEFAULT: BOT-148 (proposed)
    fn spread_goal(&mut self, now: Millis, goal: [f32; 3], pos: [f32; 3]) -> [f32; 3] {
        let flat = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).hypot(a[2] - b[2]);
        if let Some((g, aside, at, done)) = self.spread_aside.as_mut() {
            let keep = if aside.is_some() {
                SPREAD_ASIDE_MS
            } else {
                SPREAD_ASIDE_RECHECK_MS
            };
            if flat(*g, goal) <= crate::hunt::NEW_GOAL_DIST && since(now, *at) < keep {
                if *done {
                    return goal;
                }
                return match *aside {
                    Some(a) if flat(a, pos) <= SPREAD_ASIDE_REACHED => {
                        *done = true;
                        goal
                    }
                    Some(a) => a,
                    None => goal,
                };
            }
        }
        let mates = self.spread_mates(now);
        let aside = if crate::spread::crowded(goal, &mates) {
            let nav = self.nav.as_ref();
            crate::spread::push_aside(pos, goal, &mates, |p| {
                nav.and_then(|n| n.safe_node(p, crate::spread::ASIDE_SNAP))
            })
        } else {
            None
        };
        if let Some(a) = aside {
            self.rec.event(
                "spread_aside",
                json!({"ms": self.ms(now), "pos": pos, "goal": goal, "aside": a}),
            );
        }
        self.spread_aside = Some((goal, aside, now, false));
        aside.unwrap_or(goal)
    }

    /// Sends what the shooter asked for.
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.1 (0x82 R, one per pellet), §4.2 (0x83 R), §4.4 (0x7f R), §4.5 (0x76 R)
    fn run_actions(&mut self, now: Millis, acts: Vec<Action>) {
        for a in acts {
            match a {
                Action::Select(c) => {
                    // OBSERVED: tdm4 conn 6: `76 03 02` then `7f [slot][weapon][aux]` in the same instant
                    let ev = weapon::weapon_event_body(weapon::WEAPON_EVENT_SELECT);
                    self.send_shell(true, weapon::up::EVENT, &ev, now);
                    let ch = weapon::weapon_change_body(&c);
                    self.send_shell(true, weapon::up::WEAPON_CHANGE, &ch, now);
                }
                // paused (BOT-59): no shot leaves the bot
                Action::Fire(_) | Action::Throw(_) | Action::Hold | Action::Release(_)
                    if self.control.paused() => {}
                Action::Fire(v) => {
                    if let Some(f) = v.first() {
                        let held = self.shooter.as_ref().and_then(|s| s.held().copied());
                        self.rec.event(
                            "fire",
                            json!({"ms": self.ms(now), "target_obj": f.hit.map(|h| h.1),
                                   "dist": f.hit.map(|h| h.0), "pellets": v.len(), "weapon": f.weapon_id,
                                   "ammo": f.ammo_id, "origin": f.origin, "hit_point": f.extra.map(|x| x.a),
                                   "shot": self.shooter.as_ref().map(|s| s.stats.shots),
                                   "clip": held.map(|h| h.clip)}),
                        );
                    }
                    for f in v {
                        self.send_shell(true, weapon::up::FIRE, &weapon::fire_body(&f), now);
                    }
                }
                Action::Throw(f) => {
                    // SPEC: GAME_MESSAGES_2009_C2S.md §4.6 (0x4d while held, no fields; then the release 0x82)
                    self.send_shell(true, weapon::up::THROW_HOLD, &BitWriter::new(), now);
                    self.send_shell(true, weapon::up::FIRE, &weapon::fire_body(&f), now);
                }
                // BOT-147: a pass of the grenade's hold, then its release
                // SPEC: GAME_MESSAGES_2009_C2S.md §4.6 (0x4d while held, no fields; the release 0x82 after the last)
                Action::Hold => {
                    self.send_shell(true, weapon::up::THROW_HOLD, &BitWriter::new(), now);
                }
                Action::Release(f) => {
                    self.send_shell(true, weapon::up::FIRE, &weapon::fire_body(&f), now);
                }
                Action::Reload(ammo) => {
                    // SPEC: GAME_MESSAGES_2009_C2S.md §4.2 (`[ammo_id:u16]`)
                    let mut b = BitWriter::new();
                    b.write_u16(ammo);
                    self.send_shell(true, weapon::up::RELOAD, &b, now);
                }
                Action::Event(kind, mut fields) => {
                    if let serde_json::Value::Object(m) = &mut fields {
                        m.insert("ms".into(), json!(self.ms(now)));
                    }
                    self.rec.event(kind, fields);
                }
            }
        }
    }

    /// Polls the control file; logs each change (BOT-59).
    fn control_tick(&mut self, now: Millis) {
        if let Some(p) = self.control.poll(now) {
            let state = if p { "pause" } else { "resume" };
            self.rec
                .event("control", json!({"ms": self.ms(now), "state": state}));
        }
    }

    /// Per-frame game work: rate report, body steps, player updates.
    fn game_tick(&mut self, now: Millis) {
        if let Some(t) = self.leave_at {
            if !later(t, now) && self.quit_at.is_none() && !self.is_over() {
                self.leave_at = None;
                self.rec
                    .event("leave_after_result", json!({"ms": self.ms(now)}));
                self.quit(now);
            }
        }
        if self.is_over() || self.quit_at.is_some() {
            return;
        }
        if self.cfg.frame_ends_at_result && self.obs.reached("result_screen") {
            return;
        }
        self.hazard_check(now);
        // SPEC: UDP_SESSION_2009_A_addendum_disasm.md A14 (sent once the world-loaded flag is set; first frame)
        if self.world_loaded && !self.rate_sent {
            self.rate_sent = true;
            self.send(true, shell::rate(self.cfg.client_rate_bps), now);
            if let Some(l) = self.link.as_mut() {
                l.change_rate(
                    now,
                    ((self.cfg.client_rate_bps as u64 * RATE_PERCENT) / 100) as u32,
                );
            }
            self.mark(now, "rate_sent");
        }
        // the client's own arming after 0x67 state 2 (once per death), then the request after the armed wait
        // SPEC: GAME_MESSAGES_2009_C2S.md §3.4 (the request follows the armed wait; one per arming)
        let class = self.obs.mode_class.or(self.cfg.mode_class_hint);
        let ev = self.respawn.self_arm(now, class);
        self.log_life(now, ev);
        // DC-DEFAULT: BOT-168 (proposed) one `respawn_wait` per death where the mode decides
        let ev = self.respawn.wait_note(class);
        self.log_life(now, ev);
        if let Some(q) = self.respawn.poll(now, class) {
            let mut body = BitWriter::new();
            if q.with_zero {
                body.write_u8(0);
            }
            self.rec.event(
                "respawn_requested",
                json!({"shell": format!("0x{:02x}", q.shell), "mode_class": self.obs.mode_class,
                       "waited_ms": q.waited_ms, "source": q.source, "ms": self.ms(now)}),
            );
            self.send_shell(true, q.shell, &body, now);
        }
        self.roster_tick(now);
        if self.cfg.external_body {
            self.external_tick(now);
            return;
        }
        self.control_tick(now);
        let paused = self.control.paused();
        if !paused {
            self.timeline_tick(now);
        }
        if self.quit_at.is_some() {
            return;
        }
        if !paused {
            self.approach_tick(now);
        }
        // no shooting and no hunting between the round result and the next spawn: the players are frozen at the
        // result, and the shooter's re-acquire timer would otherwise fire at a dead or frozen target
        // DC-DEFAULT: BOT-44 (proposed)
        let playing = !self.respawn.round_over();
        if playing
            && !paused
            && !self.gliding
            && self.shooter.is_some()
            && self.body.is_some()
            && !self.respawn.dead()
            && self.life_state == 1
        {
            let mut view = self.shooter_view(now);
            self.timeline_view(&mut view);
            let acts = self
                .shooter
                .as_mut()
                .map(|sh| sh.poll(now, &view))
                .unwrap_or_default();
            self.timeline_count(now, &acts);
            self.run_actions(now, acts);
        }
        // the hunting way-finder walks toward the target while no clear shot exists, and looks where it walks
        let mut walk_look = None;
        if self.cfg.motion == MotionKind::Hunt {
            // BOT-81: the carrier stands still while its plant hold runs (walking away broke every hold); BOT-104:
            // the defuser too
            // BOT-136: a defender whose defuse hold cannot end before the blast lets go (and runs, the squad order)
            self.bomb_flee_release(now);
            let planting = matches!(self.obs.mode_class, Some(2 | 3)) && self.bomb.holding(now);
            // BOT-103: an enemy in clear sight and near is fought first; the walk to the objective waits
            let engage = if matches!(self.obs.mode_class, Some(2 | 3))
                && self.bomb.planted
                && !self.bomb_attacker()
            {
                crate::roles::DEFUSE_ENGAGE
            } else {
                crate::roles::OBJECTIVE_ENGAGE
            };
            let in_sight = self.shooter.as_ref().is_some_and(|s| {
                s.target().is_some() && s.target_dist().is_some_and(|d| d <= engage)
            });
            // BOT-136: the quiet-round clock (an enemy in sight at any range)
            if self
                .shooter
                .as_ref()
                .is_some_and(|s| s.target().is_some() && s.target_dist().is_some())
            {
                self.enemy_seen = Some(now);
            }
            let mut stood = None;
            let mut may_roam = false;
            // BOT-140: a Spy Hunt hunter that chases nobody roams (and fights what it meets) instead of hunting the
            // holder it sees
            let mut spy_free = false;
            let mut squad_wait = false;
            let mut squad_shift = false;
            let goal = (self.life_state == 1 && playing && !paused && !planting)
                .then(|| {
                    // BOT-46: a `goto x y z r` point holds the bot there: no hunting toward targets meanwhile
                    let stay = self
                        .timeline
                        .as_ref()
                        .is_some_and(|t| t.goto.is_some() && t.goto_radius.is_some());
                    // BOT-122: the bot's team job in a bomb round: where it walks, and whether it stands for now
                    let order = if stay { None } else { self.squad_tick(now) };
                    let waiting = order.is_some_and(|o| o.wait) && !in_sight;
                    // BOT-136: a waiting bot shifts a few steps now and then (its wait goes on)
                    let shift = order.and_then(|o| o.shift).filter(|_| waiting);
                    squad_shift = shift.is_some();
                    let bomb = if stay {
                        None
                    } else {
                        self.bomb_goal(now, in_sight, order.as_ref())
                    };
                    let bomb = bomb.filter(|_| !waiting);
                    // BOT-95: the role's objective comes after the mode runs of the carrier / runner / intel bot
                    // and before the hunt; the last one standing in an elimination round does not hunt
                    let situation = if stay {
                        None
                    } else {
                        Some(crate::roles::Situation {
                            squad: order,
                            ..self.situation(now)
                        })
                    };
                    let pos = self.body.as_ref().map(|b| b.pos());
                    // BOT-103: at its objective the bot stands (and fights what it sees) instead of circling it
                    let stood_at = self.stood_at;
                    let mut reach = |o: crate::roles::Objective| match pos {
                        Some(p)
                            if crate::roles::stands(
                                &o,
                                crate::roles::flat_dist(p, o.goal),
                                stood_at.is_some_and(|g| crate::roles::flat_dist(g, o.goal) < 1.0),
                            ) =>
                        {
                            stood = Some(o.goal);
                            None
                        }
                        _ => Some(o.goal),
                    };
                    let objective = situation
                        .as_ref()
                        .filter(|_| !in_sight && !waiting)
                        .and_then(|s| crate::roles::objective_goal(self.role, s));
                    squad_wait = waiting;
                    let objective_stood = waiting || objective.is_some_and(|o| reach(o).is_none());
                    let objective = objective.and_then(&mut reach);
                    let idle = situation.as_ref().and_then(crate::roles::idle_goal);
                    // BOT-130: the last one alive holds for a while, then hunts and roams again
                    let last_alive = situation.as_ref().is_some_and(crate::roles::cautious);
                    let since_last = if last_alive {
                        now.wrapping_sub(*self.cautious_since.get_or_insert(now))
                    } else {
                        self.cautious_since = None;
                        0
                    };
                    let cautious = situation
                        .as_ref()
                        .is_some_and(|s| crate::roles::cautious_holds(s, since_last));
                    may_roam = !stay && !cautious;
                    let spy = if stay { None } else { self.spy_goal(now) };
                    // DC-DEFAULT: BOT-140 (proposed)
                    // (BOT-164: a chaser fighting the chased player where it stands is not free)
                    spy_free = !stay
                        && spy.is_none()
                        && self.spy_engaged.is_none()
                        && self.obs.mode_class == Some(crate::spyrun::SPY_CLASS)
                        && self.spy.held == 0;
                    // BOT-128: a Quarantine human keeps away from the infected and regroups
                    let (qrt, qrt_hold) = if stay {
                        self.qrt_camp = false;
                        self.qrt_plan = None;
                        (None, false)
                    } else {
                        self.qrt_goal(now)
                    };
                    let flag = if stay {
                        self.flag_walk_team = None;
                        None
                    } else {
                        self.flag_goal(now, in_sight)
                    };
                    let tag = |k: &'static str| move |g: [f32; 3]| (g, k);
                    // BOT-143: hurt by a hazard volume: leave it for a safe grid place first
                    let hazard = if stay { None } else { self.hazard_goal(now) };
                    self.timeline_goal()
                        .map(tag("timeline"))
                        .or(hazard.map(tag("leave_hazard")))
                        .or(flag.map(tag("flag")))
                        .or(spy)
                        .or(qrt)
                        .or(shift.map(tag("shift")))
                        .or(bomb.map(tag("bomb")))
                        .or(objective.map(tag("objective")))
                        .or_else(|| {
                            (!stay && !cautious && !objective_stood && !qrt_hold && !spy_free)
                                .then(|| self.shooter.as_ref().and_then(Shooter::hunt_goal))
                                .flatten()
                                .map(tag("hunt"))
                        })
                        .or_else(|| {
                            (!in_sight && !objective_stood)
                                .then(|| idle.and_then(&mut reach))
                                .flatten()
                                .map(tag("idle"))
                        })
                })
                .flatten();
            // BOT-130: the last-alive hold starts again with the next life or round
            if !(self.life_state == 1 && playing) {
                self.cautious_since = None;
                // BOT-145: and a goal given up, the stuck count; BOT-159: a way off an island
                self.lost = None;
                self.escape = None;
                self.confined_since = None;
                self.trouble = None;
                self.edge_since = None;
            }
            // BOT-122: a waiting bot stands where it is (and fights what it sees); BOT-136: but for its shifts
            if squad_wait && !squad_shift {
                stood = self.body.as_ref().map(|b| b.pos());
            }
            // BOT-117: nothing to walk to, no target and no objective to stand at: the bot roams the grid
            let seeing = self.shooter.as_ref().is_some_and(|s| s.target().is_some());
            let goal = match goal {
                None if may_roam && !self.qrt_camp && stood.is_none() && (!seeing || spy_free) => {
                    self.roam_goal(now).map(|g| (g, "roam"))
                }
                g => {
                    self.roam = None;
                    g
                }
            };
            // BOT-148: a hunt or roam goal a team mate stands at is moved aside (the team modes that spread); a bot
            // walking right behind a team mate (no enemy in sight) waits a moment
            let goal = match (goal, self.body.as_ref().map(|b| b.pos())) {
                (Some((g, k @ ("hunt" | "roam"))), Some(p))
                    if crate::spread::spreads(self.obs.mode_class) =>
                {
                    let g = self.spread_goal(now, g, p);
                    let mates = self.spread_mates(now);
                    let in_view = self
                        .shooter
                        .as_ref()
                        .is_some_and(|s| s.target_dist().is_some());
                    if let Some(s) = self.spread_space_goal(now, p, None) {
                        Some((s, "space"))
                    } else if !in_view && self.spread_trail.tick(now, p, g, &mates) {
                        // the mate ahead is on top of the bot (it stopped): a few steps to a clear place
                        let close =
                            crate::spread::mate_ahead(p, g, &mates, crate::spread::TRAIL_CLOSE)
                                .then(|| crate::hunt::yaw_to(p, g));
                        match self.spread_space_goal(now, p, close) {
                            Some(s) => Some((s, "space")),
                            None => {
                                stood = Some(p);
                                None
                            }
                        }
                    } else {
                        Some((g, k))
                    }
                }
                // a side step that did not clear the team mate: a short walk over the grid to a clear place
                (None, Some(p)) if crate::spread::spreads(self.obs.mode_class) => {
                    let stepping = self
                        .spread_step
                        .on
                        .is_some_and(|(_, at)| since(now, at) >= crate::spread::SPACE_AFTER_MS);
                    let yaw = self
                        .shooter
                        .as_ref()
                        .and_then(Shooter::look)
                        .map(|l| l.0)
                        .filter(|_| stepping);
                    self.spread_space_goal(now, p, yaw).map(|g| (g, "space"))
                }
                (g, _) => {
                    self.spread_space = None;
                    g
                }
            };
            // BOT-145: a goal given up: the bot walks to the place it has a way to instead, for a while
            let goal = match (goal, self.lost) {
                (Some((_, k)), Some((g, at)))
                    if since(now, at) < LOST_MS
                        && matches!(k, "hunt" | "idle" | "roam" | "spy_chase" | "spy_guard") =>
                {
                    let near = self.body.as_ref().is_some_and(|b| {
                        let p = b.pos();
                        (p[0] - g[0]).hypot(p[2] - g[2]) <= LOST_REACHED
                    });
                    if near {
                        self.lost = None;
                        goal
                    } else {
                        Some((g, "lost"))
                    }
                }
                (g, Some((_, at))) if since(now, at) >= LOST_MS => {
                    self.lost = None;
                    g
                }
                (g, _) => g,
            };
            // BOT-147: a walk to or through the bot's own live grenade, or one heading toward it near it, waits for
            // the blast where it is (the thrower takes its own blast; attackers walked to their site through their
            // grenade, TMM Warhead r4 / r5)
            // DC-DEFAULT: BOT-147 (proposed)
            let blast = self.shooter.as_ref().and_then(Shooter::live_blast);
            if blast.is_none() {
                self.blast_wait = false;
            }
            let body = self.body.as_ref().map(|b| (b.pos(), b.vel()));
            let goal = match (goal, body, blast) {
                (Some((g, _)), Some((p, v)), Some((b, r)))
                    if self.blast_wait || crate::grenade::walks_into(p, g, v, b, r) =>
                {
                    self.blast_wait = true;
                    stood = Some(p);
                    None
                }
                (g, _, _) => g,
            };
            self.stood_at = stood.filter(|_| goal.is_none());
            let goal_kind = goal
                .map(|g| g.1)
                .or(self.stood_at.map(|_| "stand"))
                .or(planting.then_some("hold"));
            let goal = goal.map(|g| g.0);
            // BOT-172: the knife out on a long quiet walk, the gun back on any contact
            if !paused {
                self.knife_tick(now, goal, goal_kind, playing);
            }
            let at = self.body.as_ref().map(|b| (b.pos(), b.rom_world));
            // the bot's place and walk goal every 2 s while it lives in a running round (orbit checks, live logs)
            if self.life_state == 1 && playing && since(now, self.track_said) >= TRACK_MS {
                self.track_said = now;
                if let Some((pos, _)) = at {
                    let job = self.squad_order.map(|o| format!("{:?}", o.job));
                    let wait = self.squad_order.filter(|o| o.wait).map(|o| o.why);
                    // BOT-134: the target, its distance (in sight: the last poll's; else to where it was last
                    // known) and whether it is in clear sight
                    let sh = self.shooter.as_ref();
                    let tgt = sh.and_then(Shooter::target);
                    let sight = sh.and_then(Shooter::target_dist).is_some();
                    let tgt_dist = sh.and_then(Shooter::target_dist).or_else(|| {
                        tgt.and(sh.and_then(Shooter::hunt_goal))
                            .map(|g| (g[0] - pos[0]).hypot(g[2] - pos[2]))
                    });
                    self.rec.event(
                        "track",
                        json!({"ms": self.ms(now), "pos": pos, "goal": goal, "kind": goal_kind,
                               "hold": self.bomb.hold.is_some(), "job": job, "wait": wait,
                               "post": self.squad_order.map(|o| o.post).filter(|p| !p.is_empty()),
                               "crouch": self.squad_order.is_some_and(|o| o.crouch),
                               "via": self.squad.via(), "team": self.own_team(),
                               "edge": self.nav_end.is_some(), "arrived": self.nav_arrived.is_some(),
                               "tgt": tgt, "tgt_dist": tgt_dist, "sight": sight, "branch": self.branch,
                               "knife": sh.is_some_and(Shooter::knifing),
                               "qrt": self.qrt_plan}),
                    );
                }
            }
            // BOT-103: a site the bot gets no closer to is out of its reach this round; it picks another. BOT-129: a
            // spacing wait behind a teammate still on its way keeps the clock running (before, every wait started it
            // again and two bots under a site they could not reach stood for the whole round), any other wait stops
            // it; progress along the planned way counts as well as the straight line
            if matches!(self.obs.mode_class, Some(2 | 3)) {
                let waiting_site = self
                    .squad_order
                    .filter(|_| squad_wait)
                    .and_then(|o| o.goal.map(|g| (g.goal, o.stall_counts)));
                let site_walk = goal
                    .filter(|_| matches!(goal_kind, Some("bomb" | "objective" | "idle")))
                    .or(waiting_site.map(|w| w.0))
                    .filter(|g| {
                        crate::bombrun::plant_points(&self.table)
                            .iter()
                            .any(|p| (p.1[0] - g[0]).abs() < 1.0 && (p.1[2] - g[2]).abs() < 1.0)
                    });
                let clock = match waiting_site {
                    Some((_, false)) => crate::bombrun::SiteClock::Stops,
                    _ => crate::bombrun::SiteClock::Runs,
                };
                let body_pos = at.map_or([0.0; 3], |a| a.0);
                let way_left = site_walk.and_then(|g| self.nav_way_left(body_pos, g));
                if let (Some((pos, _)), Some(g)) = (
                    at,
                    self.bomb.walk(now, site_walk, body_pos, way_left, clock),
                ) {
                    self.rec.event(
                        "site_out_of_reach",
                        json!({"ms": self.ms(now), "site": g, "pos": pos}),
                    );
                }
            }
            // BOT-138: a flag walk that makes no way outside the touch ends: the flag is out of reach from here
            if matches!(self.obs.mode_class, Some(7 | 8)) {
                self.flag_reach(now, at.map(|a| a.0), goal.zip(goal_kind));
            }
            if let Some(g) = goal {
                let far = self.hunt_goal_last.is_none_or(|l| {
                    ((l[0] - g[0]).powi(2) + (l[2] - g[2]).powi(2)).sqrt()
                        > crate::hunt::NEW_GOAL_DIST
                });
                if far {
                    self.hunter.reset();
                }
                self.hunt_goal_last = Some(g);
            }
            // BOT-121: on a ROM world the bot follows its way over the grid (the co-op AI's planner, FT-S11); the
            // greedy way-finder takes over only where the grid gives no way (off it, on an island of it)
            if goal.is_some() {
                self.goal_seen = now;
            }
            let mut way = match (goal, at) {
                (Some(g), Some((pos, true))) => self.nav_way(now, pos, g),
                _ => None,
            };
            if let (Some(_), Some((pos, true))) = (goal, at) {
                way = self.nav_edge(now, pos, way);
                // BOT-145: a walk at the end of a way short of the goal (or on an island) that lasts gives it up
                if way.is_some() || self.nav_end.is_none() {
                    self.edge_since = None;
                } else {
                    let t0 = *self.edge_since.get_or_insert(now);
                    if since(now, t0) >= LOST_EDGE_MS {
                        self.edge_since = None;
                        self.nav_lose(now, pos, "edge");
                    }
                }
            }
            // BOT-159: a bot that stays on a small island of the grid while its goal lies off it takes a way off (the
            // edge walk above is broken off by every short way the follower finds on the island)
            if let (Some(g), Some((pos, true))) = (goal, at) {
                self.confined_tick(now, pos, g);
            }
            // BOT-121: at a goal the body cannot get closer to, the bot stands until the goal moves
            let flat = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).hypot(a[2] - b[2]);
            // BOT-131: and only while the body stays near the goal (a fight that pushed it away ends the wait)
            let body_at = at.map(|a| a.0);
            self.nav_arrived = self.nav_arrived.filter(|a| {
                goal.is_some_and(|g| flat(*a, g) <= crate::nav::follow::TARGET_MOVED)
                    && body_at.is_none_or(|p| flat(p, *a) <= crate::nav::follow::TARGET_MOVED)
            });
            let arrived = self.nav_arrived.is_some();
            let steer = match (goal, at) {
                (Some(_), Some(_)) if arrived => {
                    self.hunter.stop();
                    None
                }
                (Some(_), Some((pos, true))) if way.is_some() => {
                    let wp = way.unwrap_or(pos);
                    self.hunter.stop();
                    if since(now, self.nav_said) >= 5000 {
                        self.nav_said = now;
                        self.nav_event(now);
                    }
                    Some(crate::hunt::yaw_to(pos, wp))
                }
                (Some(g), Some((pos, rom))) => {
                    let flips = self.hunter.flips;
                    let body = self.body.as_ref();
                    let world = self.world.as_ref().map(|w| w.collision.clone());
                    let kill = if self.in_hazard {
                        &self.kill_only
                    } else {
                        &self.kill_now
                    };
                    // BOT-111: a probe that ends in danger (a kill volume, above one, a high drop) made no progress;
                    // BOT-145: and is no way to fall back on either
                    let probe = |y: f32| {
                        body.map_or(Some(pos), |b| {
                            let path = b.probe_path(
                                Some(y),
                                None,
                                crate::hunt::PROBE_SECS,
                                GREEDY_PROBE_SAMPLES,
                            );
                            let end = path[path.len() - 1];
                            match &world {
                                Some(w) if !crate::evade::path_safe(w, kill, b.dims(), &path) => {
                                    None
                                }
                                _ => Some(end),
                            }
                        })
                    };
                    let y = self.hunter.steer(
                        now,
                        pos,
                        g,
                        rom.then_some(&probe as &dyn Fn(f32) -> Option<[f32; 3]>),
                    );
                    if self.hunter.flips != flips {
                        self.rec.event(
                            "hunt_stuck",
                            json!({"ms": self.ms(now), "pos": pos, "goal": g, "flips": self.hunter.flips}),
                        );
                    }
                    y
                }
                _ => {
                    self.hunter.stop();
                    // BOT-134: a goal gone for a tick or two (the hunt goal changing hands) keeps the way and the
                    // stuck count; before, each such tick cleared the follower, so its stuck count never ran out
                    if goal.is_some() || since(now, self.goal_seen) > WATCH_GAP_MS {
                        self.nav_reset();
                    }
                    None
                }
            };
            // BOT-120: a closed door on the way: stand, face it, press the use key, wait for it to open
            let door_yaw = match at {
                Some((pos, true)) => {
                    let alive = self.life_state == 1 && playing && !paused;
                    self.door_tick(now, alive, pos, goal.filter(|_| steer.is_some()))
                }
                _ => None,
            };
            let steer = if door_yaw.is_some() { None } else { steer };
            // BOT-159: the way off a small island of the grid is walked (jumped first) for its time, whatever the goal
            let escaping = match self.escape {
                Some((e, t0)) if since(now, t0) < ESCAPE_MS && at.is_some_and(|a| a.1) => {
                    let t = since(now, t0) as f32 / 1000.0;
                    let pos = at.map_or([0.0; 3], |a| a.0);
                    let back = (crate::evade::ESCAPE_BLOCK_SECS * 1000.0) as Millis;
                    let speed = match self.escape_trail {
                        Some((t1, q)) if since(now, t1) >= back => {
                            self.escape_trail = Some((now, pos));
                            Some((pos[0] - q[0]).hypot(pos[2] - q[2]) * 1000.0 / since(now, t1) as f32)
                        }
                        Some(_) => None,
                        None => {
                            self.escape_trail = Some((now, pos));
                            None
                        }
                    };
                    let jump = !self.escape_jumped
                        && self.body.as_ref().is_some_and(|b| {
                            b.escape_jump_due(e.jump, t, speed.unwrap_or(f32::MAX))
                        });
                    self.escape_jumped |= jump;
                    Some((e, jump))
                }
                Some(_) => {
                    self.escape = None;
                    self.nav_reset();
                    let pos = at.map(|a| a.0);
                    self.rec
                        .event("escape_done", json!({"ms": self.ms(now), "pos": pos}));
                    None
                }
                None => None,
            };
            let steer = escaping.map_or(steer, |e| Some(e.0.yaw));
            // BOT-92: a target in sight (no walk goal) is fought on the move: strafe, step back, jump, crouch;
            // BOT-124: not while the own plant / defuse hold runs (a strafe or a step back carried the body past the
            // server's 48-unit hold limit and broke every defuse under fire)
            // BOT-151: the Spy Hunt agent in the uplink box stands its ground (an evasive step left the box and
            // started the upload afresh)
            let uploading = self.spy_set_off
                && self
                    .spy
                    .uplink(&self.table)
                    .zip(at)
                    .is_some_and(|(u, (p, _))| (u[0] - p[0]).hypot(u[2] - p[2]) <= AGENT_BOX);
            let engaged = steer.is_none()
                && !planting
                && !uploading
                && self.shooter.as_ref().is_some_and(|s| {
                    s.target().is_some() && s.target_dist().is_some() && s.look().is_some()
                });
            let level = self
                .shooter
                .as_ref()
                .and_then(|s| s.config().skill.map(|k| k.level));
            let evade = match (engaged, level) {
                (true, Some(level)) => {
                    let yaw = self
                        .shooter
                        .as_ref()
                        .and_then(Shooter::look)
                        .map_or(0.0, |l| l.0);
                    let dist = self
                        .shooter
                        .as_ref()
                        .and_then(Shooter::target_dist)
                        .unwrap_or(f32::MAX);
                    let mut mv = self.evader.tick(now, yaw, dist, level);
                    // BOT-148: fighting from where a team mate stands: a side step away from it (the strafe of this
                    // move, so the danger gate below still judges it)
                    // DC-DEFAULT: BOT-148 (proposed)
                    let mates = self.spread_mates(now);
                    let own_pid = self.obs.client_id.map(|c| c.0 as u8);
                    if let (Some(pid), Some(b)) = (own_pid, self.body.as_ref()) {
                        let p = b.pos();
                        if let Some((side, began)) = self.spread_step.tick(now, pid, p, yaw, &mates)
                        {
                            mv.strafe = side;
                            mv.back = false;
                            if began {
                                self.rec.event(
                                    "spread_step",
                                    json!({"ms": self.ms(now), "pos": p, "side": side, "mates": mates}),
                                );
                            }
                        }
                    }
                    // BOT-116: no evasive step toward danger (a kill volume, the void, a high drop): the sides
                    // and the back are probed with the movement code every EVADE_CHECK_MS; a side in danger stops
                    // the strafe and the jump, the back in danger the step back
                    if since(now, self.evade_safe.0) >= EVADE_CHECK_MS || self.evade_safe.0 == 0 {
                        let safe = match (self.body.as_ref(), self.world.as_ref()) {
                            (Some(b), Some(w)) if b.rom_world => crate::evade::evade_safety(
                                b,
                                &w.collision,
                                if self.in_hazard {
                                    &self.kill_only
                                } else {
                                    &self.kill_now
                                },
                                yaw,
                                EVADE_PROBE_SECS,
                            ),
                            _ => crate::evade::Safety::default(),
                        };
                        self.evade_safe = (now.max(1), safe);
                        // BOT-170: a push is walked only when its run on a copy keeps out of danger (probed only
                        // while one is due; off the ROM world it is not walked)
                        self.push_safe = mv.push
                            && match (self.body.as_ref(), self.world.as_ref()) {
                                (Some(b), Some(w)) if b.rom_world => crate::evade::move_safe(
                                    b,
                                    &w.collision,
                                    if self.in_hazard {
                                        &self.kill_only
                                    } else {
                                        &self.kill_now
                                    },
                                    crate::evade::Move { jump: false, ..mv },
                                    EVADE_PROBE_SECS,
                                ),
                                _ => false,
                            };
                    }
                    if !self.push_safe {
                        mv.push = false;
                    }
                    // BOT-134: a strafe toward an unsafe side turns to the safe one (or stops), and a jump runs its
                    // whole flight on a copy first
                    let gated = crate::evade::gate(&mut mv, self.evade_safe.1);
                    // BOT-145: no side safe at a deck's edge: a step inward (the aim stays on the target), else a
                    // crouch where it stands
                    let stepping = self
                        .inside_step
                        .is_some_and(|s| since(s.0, now) < 0x8000_0000 && s.0 != now);
                    if gated == Some("stop") && !stepping {
                        let inward = match (self.body.as_ref(), self.world.as_ref()) {
                            (Some(b), Some(w)) if b.rom_world => crate::evade::inside_heading(
                                b,
                                &w.collision,
                                if self.in_hazard {
                                    &self.kill_only
                                } else {
                                    &self.kill_now
                                },
                            ),
                            _ => None,
                        };
                        let ms = (crate::evade::INSIDE_SECS * 1000.0) as Millis;
                        self.inside_step = Some((now.wrapping_add(ms), inward));
                    }
                    if self.inside_step.is_some_and(|s| s.1.is_none()) {
                        mv.crouch = true;
                    }
                    let mut vetoed = false;
                    if mv.jump && !self.jump_ok(None, Some(mv)) {
                        mv.jump = false;
                        vetoed = true;
                    }
                    if let Some(n) = mv.note {
                        self.rec.event(
                            "evade",
                            json!({"ms": self.ms(now), "what": n, "strafe": mv.strafe, "back": mv.back,
                                   "crouch": mv.crouch, "push": mv.push, "dist": dist, "gated": gated,
                                   "jump_vetoed": vetoed}),
                        );
                    }
                    Some(mv)
                }
                _ => {
                    self.evader.idle();
                    self.spread_step.idle(now);
                    None
                }
            };
            let mut evade = evade;
            if let Some((e, true)) = escaping {
                evade = Some(crate::evade::Move {
                    yaw: e.yaw,
                    strafe: 0,
                    back: false,
                    jump: true,
                    crouch: false,
                    push: false,
                    note: None,
                });
            }
            let climbing = self.body.as_ref().is_some_and(|b| b.on_ladder());
            let jump_move = |evade: Option<crate::evade::Move>| {
                let mut m = evade.unwrap_or(crate::evade::Move {
                    yaw: 0.0,
                    strafe: 0,
                    back: false,
                    jump: false,
                    crouch: false,
                    push: false,
                    note: None,
                });
                m.jump = true;
                Some(m)
            };
            match (steer, self.body.as_ref().map(|b| b.pos())) {
                // BOT-159: the way off an island is no way of the grid: no stuck rule
                (Some(_), Some(_)) if escaping.is_some() => self.walk_check = None,
                // BOT-121: walking its way: the stuck rule of the grid's follower (FT-S11): the way left gets no
                // shorter for 1.5 s: a jump; for 3 s: the link it was on is given up and it plans again. Also on a
                // ladder: the grid has no ladder links, so a way never climbs one (a body caught on one is stuck)
                (Some(_), Some(p)) if way.is_some() && door_yaw.is_none() => {
                    self.walk_check = None;
                    // walking to the goal itself at the end of a whole way
                    let to_goal = self.follow.points().is_empty() && self.follow.whole();
                    let stuck = match self.nav.as_mut() {
                        Some(nav) => self.follow.stuck(nav, now, p),
                        None => crate::nav::follow::Stuck::No,
                    };
                    match stuck {
                        crate::nav::follow::Stuck::Jump if !self.jump_needed(now, steer) => {
                            self.obs.jumps_skipped += 1;
                        }
                        crate::nav::follow::Stuck::Jump => {
                            // BOT-134: no jump whose flight or landing is in danger
                            let safe = self.jump_ok(steer, jump_move(evade));
                            if safe {
                                evade = jump_move(evade);
                            }
                            let why = self.follow.last_jump;
                            // BOT-145: a jump up whose flight the jump test refuses is no way for this body: the
                            // jump links onto that top go for a while and the bot plans round
                            let refused = !safe
                                && why.is_some_and(|w| w.top)
                                && self
                                    .nav
                                    .as_mut()
                                    .is_some_and(|nav| self.follow.refuse(nav, now, p, false));
                            if refused {
                                self.nav_cache = None;
                            }
                            self.rec.event(
                                "stuck_jump",
                                json!({"ms": self.ms(now), "pos": p, "goal": goal, "by": "way",
                                       "vetoed": !safe, "face": why.map(|w| w.face),
                                       "rise": why.map(|w| w.rise), "flat": why.map(|w| w.flat),
                                       "top": why.map(|w| w.top), "refused": refused}),
                            );
                        }
                        crate::nav::follow::Stuck::Replan | crate::nav::follow::Stuck::Trap => {
                            // the goal itself is out of the body's reach (on a stand, in a corner): the bot stands
                            // where it got to until the goal moves
                            if to_goal {
                                self.nav_arrived = goal;
                            }
                            let trap = stuck == crate::nav::follow::Stuck::Trap;
                            self.rec.event(
                                "stuck_replan",
                                json!({"ms": self.ms(now), "pos": p, "goal": goal, "at_goal": to_goal,
                                       "trap": trap, "backing": self.follow.backing()}),
                            );
                            if !to_goal {
                                self.nav_trouble(now, p, "replan");
                            }
                        }
                        crate::nav::follow::Stuck::No => {}
                    }
                }
                // a climb on a ladder moves the body up, not across: no check there
                (Some(_), Some(_)) if climbing => self.walk_check = None,
                // BOT-113: the greedy way-finder's body that made no way for a while jumps (out of a pit, over a
                // low edge the grid and the greedy way-finder do not know)
                (Some(_), Some(p)) => match self.walk_check {
                    Some((at, from)) if since(now, at) >= WALK_CHECK_MS => {
                        let moved = ((p[0] - from[0]).powi(2) + (p[2] - from[2]).powi(2)).sqrt();
                        if moved < WALK_CHECK_MIN && !self.body_blocked(steer) {
                            // BOT-153: free ahead in the world (a team mate or the server held the body): no jump
                            self.obs.jumps_skipped += 1;
                        } else if moved < WALK_CHECK_MIN {
                            let safe = self.jump_ok(steer, jump_move(evade));
                            if safe {
                                evade = jump_move(evade);
                            }
                            self.rec.event(
                                "stuck_jump",
                                json!({"ms": self.ms(now), "pos": p, "moved": moved, "goal": goal,
                                       "by": "greedy", "vetoed": !safe}),
                            );
                        }
                        self.walk_check = Some((now, p));
                    }
                    Some(_) => {}
                    None => self.walk_check = Some((now, p)),
                },
                _ => self.walk_check = None,
            }
            // BOT-134: the arm that steers (`track.branch`, the watchdog's event)
            self.branch = match (door_yaw, steer) {
                (Some(_), _) => "door",
                (None, Some(_)) if way.is_some() => "way",
                (None, Some(_)) if climbing => "ladder",
                (None, Some(_)) => "greedy",
                (None, None) if goal.is_some() => "hold",
                (None, None) => "stand",
            };
            // BOT-134: one body-progress watchdog over every arm (the follower's and the greedy way-finder's own
            // checks start again whenever the steering changes hands, so a body went 10 s without either firing)
            let alive = self.life_state == 1 && playing && !paused;
            let far = match (goal, at) {
                (Some(g), Some((p, _))) => flat(g, p) > WATCH_FAR,
                _ => false,
            };
            let body_now = self.body.as_ref().map(|b| (b.pos(), b.on_ladder()));
            match body_now {
                Some((p, ladder)) if alive && far && !arrived && door_yaw.is_none() => {
                    self.watch_seen = now;
                    match self.watch {
                        Some((t, a)) => {
                            let moved = flat(a, p);
                            if moved >= WATCH_MOVE || (ladder && (p[1] - a[1]).abs() >= WATCH_MOVE)
                            {
                                self.watch = Some((now, p));
                            } else if since(now, t) >= WATCH_MS {
                                evade = self.watchdog_trip(now, p, goal, moved, steer, evade);
                            }
                        }
                        None => self.watch = Some((now, p)),
                    }
                }
                // a short gap keeps the anchor (before, a goal gone for one tick started the watch again, and a
                // body frozen 24 s on Overdose never tripped it)
                _ if since(now, self.watch_seen) <= WATCH_GAP_MS => {}
                _ => self.watch = None,
            }
            // BOT-136: a bot standing at its post crouches now and then (nothing steers, no fight)
            if evade.is_none()
                && steer.is_none()
                && door_yaw.is_none()
                && !planting
                && self.squad_order.is_some_and(|o| o.crouch)
            {
                if let Some(b) = self.body.as_ref() {
                    evade = Some(crate::evade::Move {
                        yaw: b.angles[1],
                        strafe: 0,
                        back: false,
                        jump: false,
                        crouch: true,
                        push: false,
                        note: None,
                    });
                }
            }
            // BOT-134: a body that stands (or fights on the spot) within a body width of a kill volume's edge steps
            // away from it first
            let mut body_steer = steer;
            if steer.is_none() && door_yaw.is_none() && !planting && alive {
                if since(now, self.edge.0) >= EVADE_CHECK_MS || self.edge.0 == 0 {
                    let away = match self.body.as_ref() {
                        Some(b) if b.rom_world => crate::evade::edge_away(
                            if self.in_hazard {
                                &self.kill_only
                            } else {
                                &self.kill_now
                            },
                            b.dims(),
                            b.pos(),
                        ),
                        _ => None,
                    };
                    if let (Some(y), None) = (away, self.edge.1) {
                        let pos = self.body.as_ref().map(|b| b.pos());
                        self.rec.event(
                            "step_away",
                            json!({"ms": self.ms(now), "pos": pos, "yaw": y}),
                        );
                    }
                    self.edge = (now.max(1), away);
                }
                if let Some(y) = self.edge.1 {
                    body_steer = Some(y);
                    evade = None;
                } else if let Some((until, h)) = self.inside_step {
                    // BOT-145: the step inward from a deck's edge (the jump and the crouch stay off meanwhile)
                    if since(until, now) < 0x8000_0000 && until != now {
                        if let Some(h) = h {
                            body_steer = Some(h);
                            evade = None;
                        }
                    } else {
                        self.inside_step = None;
                    }
                }
            } else {
                self.edge.1 = None;
                self.inside_step = None;
            }
            // BOT-150: in Quarantine a walk whose next half second (run on a copy) passes over a deadly drop or into a
            // kill or hazard volume while the body stands safe is held until the next check (Two Towers: bots on
            // the upper walk slipped off its edge into the poison water on their way to the group)
            // DC-DEFAULT: BOT-150 (proposed)
            if self.obs.mode_class == Some(crate::qrt::QRT_CLASS) {
                if let (Some(y), Some(b), Some(w)) =
                    (body_steer, self.body.as_ref(), self.world.as_ref())
                {
                    if b.rom_world {
                        if since(now, self.walk_guard.0) >= EVADE_CHECK_MS || self.walk_guard.0 == 0
                        {
                            let (coll, kill, dims) = (&w.collision, &self.kill_now, b.dims());
                            let here = crate::nav::in_danger(coll, kill, dims, b.pos());
                            let path = b.probe_path(Some(y), None, crate::qrt::WALK_GUARD_SECS, 5);
                            let held = !here && !crate::evade::path_safe(coll, kill, dims, &path);
                            let pos = b.pos();
                            if held && !self.walk_guard.1 {
                                // the grid's step that leads there is given up, so the way is planned anew
                                // around the edge (a held body that only waited made the stuck watchdog jump)
                                let wp = self.follow.points().first().copied();
                                if let (Some(wp), Some(n)) = (wp, self.nav.as_mut()) {
                                    n.ban_step(pos, wp);
                                    self.follow.clear();
                                }
                                self.rec.event(
                                    "walk_guard",
                                    json!({"ms": self.ms(now), "pos": pos, "yaw": y, "way": wp}),
                                );
                            }
                            self.walk_guard = (now.max(1), held);
                        }
                        if self.walk_guard.1 {
                            body_steer = None;
                        }
                    }
                } else {
                    self.walk_guard.1 = false;
                }
            }
            if let Some(b) = self.body.as_mut() {
                b.steer = body_steer;
                // BOT-139: on a ladder the body climbs toward the height of the next point of its way (a way up a ladder
                // to a goal below, or down to one above), else of the goal
                b.goal_y = way.or(goal).map(|g| g[1]);
                b.evade = evade;
            }
            walk_look = steer.or(door_yaw).map(|y| (y, 0.0));
            // BOT-128: a human falling back from an infected it fights backs away facing it (the shooter's aim
            // turns the view, the walk keeps its own direction)
            // BOT-151: the Spy Hunt agent hurt while it waits away from the uplink fights back on the move
            let agent_hurt = matches!(goal_kind, Some("spy_lurk"))
                && self
                    .last_hurt
                    .is_some_and(|t| since(now, t) <= AGENT_HURT_MS);
            let backing = (matches!(goal_kind, Some("keep_away")) || agent_hurt)
                && self.shooter.as_ref().is_some_and(|s| s.target().is_some());
            if backing {
                walk_look = None;
            }
            // BOT-149: every bot moves with its loadout's speed rows as a player's mover does: the weapon in hand
            // (a rifle slows it, a pistol or a knife less), the worn gear and body row 0; BOT-143: an infected body
            // takes its override row (faster) and a hit's slow; an infected bot chasing a human sprints while its
            // gauge lasts
            let qrt_room = self.obs.mode_class == Some(crate::qrt::QRT_CLASS);
            if let Some(h) = self.shooter.as_ref().and_then(|s| s.held()) {
                self.speed_weapon = Some(h.weapon);
            }
            let override_row = self.qrt_override.filter(|_| qrt_room);
            let mods = match &self.speed_rows {
                Some(rows) => crate::motion::loadout_mods(
                    rows,
                    self.speed_weapon,
                    self.speed_gear,
                    override_row,
                    override_row.is_some() && self.shot_slow_until.is_some_and(|u| later(u, now)),
                ),
                None => dc_sim::SpeedMods::default(),
            };
            let chasing = qrt_room
                && self.own_team() == Some(crate::qrt::INFECTED)
                && !backing
                && matches!(goal_kind, Some("hunt" | "chase"))
                && goal
                    .zip(self.body.as_ref().map(|b| b.pos()))
                    .is_some_and(|(g, p)| {
                        (g[0] - p[0]).hypot(g[2] - p[2]) <= crate::qrt::SPRINT_RANGE
                    });
            // BOT-151: the Spy Hunt agent sprints to its waiting place and to the uplink
            let agent_run = self.obs.mode_class == Some(crate::spyrun::SPY_CLASS)
                && self.spy.total > 0
                && self.spy.held >= self.spy.total
                && !backing
                && matches!(goal_kind, Some("spy" | "spy_lurk"))
                && goal
                    .zip(self.body.as_ref().map(|b| b.pos()))
                    .is_some_and(|(g, p)| (g[0] - p[0]).hypot(g[2] - p[2]) > AGENT_SPRINT_MIN);
            let chasing = chasing || agent_run;
            let sprint_allowed = self.obs.sprint_allowed == Some(true);
            let mut rest = self.sprint_rest;
            if let Some(b) = self.body.as_mut() {
                b.backing = backing;
                b.mods = mods;
                let (gauge, mode) = b.gauge();
                b.sprint = crate::qrt::sprint_key(chasing, gauge, mode, &mut rest);
                b.sprint_allowed = sprint_allowed;
            }
            self.sprint_rest = rest;
        }
        let t = self.ms(now) as f32 / 1000.0;
        let dead = self.respawn.dead();
        let mut look = walk_look
            .or_else(|| {
                (!paused)
                    .then(|| self.shooter.as_ref().and_then(Shooter::look))
                    .flatten()
            })
            .or_else(|| (!paused).then(|| self.timeline_look()).flatten())
            .or_else(|| (!paused).then(|| self.post_watch_look(now)).flatten());
        // skill mode: the walking heading also turns the body at the skill's rate (the shooter's own aim is
        // already rate-limited); the steering itself is unchanged
        // DC-DEFAULT: BOT-21 (proposed)
        let turn = self.shooter.as_ref().and_then(Shooter::turn_rate);
        if let (Some(rate), Some(w), Some(f)) = (turn, walk_look, self.facing) {
            let dt = since(now, self.facing_at).min(250);
            look = Some(crate::skill::turn_toward(f, w, rate * dt as f32 / 1000.0));
        }
        let Some(b) = self.body.as_mut() else {
            return;
        };
        b.frozen = paused;
        self.facing = Some(look.unwrap_or((b.angles[1], b.angles[0])));
        self.facing_at = now;
        if dead {
            // not playing: no movement and no updates until state 1 returns
            self.body_clock = now;
            return;
        }
        let dt_ms = since(now, self.body_clock);
        self.body_clock = now;
        b.advance(dt_ms as u64 * 1000);
        // BOT-134: the body's take-offs (a jump the server sees, whatever the control word said)
        let jumping = b.jumping();
        if jumping && !self.was_jumping {
            self.obs.jumps_taken += 1;
        }
        self.was_jumping = jumping;
        let sprinting = b.controls() & dc_sim::bits::SPRINT != 0;
        let gate = self.life_state == 1 || self.cfg.force_updates;
        if gate && !later(self.next_update, now) {
            let mut u = b.update(t);
            // the shooter's aim turns the view (the walk keeps its own direction)
            if let Some((yaw, pitch)) = look {
                u.angles = [pitch, yaw, 0.0];
            }
            let fired = self.shooter.as_mut().is_some_and(Shooter::take_fired);
            if fired {
                u.control |= shooter::FIRE_CONTROL_BIT;
            }
            if u.control & dc_sim::bits::JUMP != 0 {
                self.obs.jump_inputs += 1;
            }
            if self.cfg.aim_quat {
                let q = look.map_or_else(|| b.aim_quat(), |(y, p)| shooter::aim_quat(y, p));
                if self.last_aim != Some(q) {
                    self.last_aim = Some(q);
                    u.aim = Some(q);
                }
            }
            let mut body = shell::player_update_body(&u);
            if fired {
                body = shooter::with_pu_flags(&body, shooter::FIRE_PU_FLAG);
            }
            // SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (at least 1 / R seconds since the last send on a whole-ms clock: 67 ms
            // at R = 15, the 2009 client's gap in tdm1 / tdm4 / tdm6)
            let period = (1000.0 / self.cfg.update_hz.max(0.1)).ceil() as u32;
            self.next_update = now.wrapping_add(period.max(1));
            // SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (the update is unreliable)
            self.send_shell(false, shell_id::UP_PLAYER_UPDATE, &body, now);
            self.note_update_sent(now);
            self.obs.updates_sent += 1;
            if self.obs.updates_sent == 1 {
                self.mark(now, "first_update_sent");
            }
        }
        // BOT-143: the sprint pair `UP_SPRINT_STATE` `[sprinting][stopped]` whenever it changes, as the client sends
        // it; the server's answer `GS_SPRINT_ALLOWED` lets the body reach its sprint speed
        // SPEC: GAME_MESSAGES_2009_C2S.md §2.3
        if self.life_state == 1 {
            let pair = (sprinting, !sprinting);
            if self.sprint_pair != Some(pair) {
                self.sprint_pair = Some(pair);
                let mut w = BitWriter::new();
                w.write_bit(pair.0).write_bit(pair.1);
                self.send_shell(true, shell_id::UP_SPRINT_STATE, &w, now);
            }
        }
    }

    /// Counts the gap since the previous player update ([`Observed::update_gaps`]).
    fn note_update_sent(&mut self, now: Millis) {
        if let Some(last) = self.last_update_at {
            *self.obs.update_gaps.entry(since(now, last)).or_default() += 1;
        }
        self.last_update_at = Some(now);
    }

    /// The roster tick ([`GameConfig::roster_query`]): every [`ROSTER_TICK_MS`] once in the world, one 0x8c
    /// `[pid:u32]` per queued player id, then the queue is cleared.
    // SPEC: GAME_MESSAGES_2009_C2S.md §5.5
    fn roster_tick(&mut self, now: Millis) {
        if !self.cfg.roster_query || !self.world_ready_sent || self.left_match {
            return;
        }
        let at = *self
            .roster_tick_at
            .get_or_insert(now.wrapping_add(ROSTER_TICK_MS));
        if later(at, now) {
            return;
        }
        self.roster_tick_at = Some(now.wrapping_add(ROSTER_TICK_MS));
        for pid in std::mem::take(&mut self.roster_pending) {
            let mut b = BitWriter::new();
            b.write_u32(pid as u32);
            self.send_shell(true, shell_id::UP_ROSTER_QUERY, &b, now);
        }
    }

    /// The caller's player update under the 2009 gate: player state 1 (or forced), spawned, at most `update_hz`
    /// per second; the aim rotation only when it changed since the last update (BOT-9).
    // SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (gate, unreliable, R = 15 per second, flag 0x0100 on change)
    fn external_tick(&mut self, now: Millis) {
        // OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.2 (a) (no 0x64 after the 0xbd)
        let gate = (self.life_state == 1 || self.cfg.force_updates)
            && self.external_spawned
            && !self.left_match;
        if !gate || later(self.next_update, now) {
            return;
        }
        let Some(mut u) = self.external else {
            return;
        };
        match (self.cfg.aim_quat, u.aim) {
            (true, Some(q)) if self.last_aim != Some(q) => self.last_aim = Some(q),
            _ => u.aim = None,
        }
        let body = match self.external_extras {
            Some(mut x) => {
                match x.cam_point {
                    Some(c) if self.last_cam != Some(c) => self.last_cam = Some(c),
                    _ => x.cam_point = None,
                }
                // SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (0x0200 when the weapon's float differs from the last one sent)
                match x.weapon_f {
                    Some(f) if self.last_weapon_f != Some(f) => self.last_weapon_f = Some(f),
                    _ => x.weapon_f = None,
                }
                shell::player_update_body_ext(&u, &x)
            }
            None => shell::player_update_body(&u),
        };
        // SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (at least 1 / R seconds since the last send on a whole-ms clock: 67 ms
        // at R = 15, the 2009 client's gap in tdm1 / tdm4 / tdm6)
        let period = (1000.0 / self.cfg.update_hz.max(0.1)).ceil() as u32;
        self.next_update = now.wrapping_add(period.max(1));
        // SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (the update is unreliable)
        self.send_shell(false, shell_id::UP_PLAYER_UPDATE, &body, now);
        self.note_update_sent(now);
        self.obs.updates_sent += 1;
        if self.obs.updates_sent == 1 {
            self.mark(now, "first_update_sent");
        }
    }
}

// ------------------------------------------------------------------------------------------------ the timeline

/// A `goto` point counts as reached within this flat distance.
// DC-DEFAULT: BOT-41 (proposed) 48 units (about one and a half body widths)
pub const TIMELINE_GOTO_REACHED: f32 = 48.0;
/// An `approach` walk stops within this flat distance of the player.
// DC-DEFAULT: BOT-42 (proposed) 96 units (close, in front, without touching)
pub const TIMELINE_APPROACH_STOP: f32 = 96.0;

impl BotSession {
    /// Starts the timeline clock at the first spawn and applies the steps that are due: logs each one (event
    /// `timeline`), throws now on `throw`, quits on `leave`.
    fn timeline_tick(&mut self, now: Millis) {
        let ms = self.ms(now);
        let alive = self.life_state == 1 && self.body.is_some() && !self.respawn.dead();
        let peer_spawned = self
            .timeline
            .as_ref()
            .and_then(|t| t.sync_wanted())
            .is_some_and(|n| self.peer_spawned(n));
        let Some(tl) = self.timeline.as_mut() else {
            return;
        };
        if alive {
            if let Some(gave_up) = tl.start_alive(ms, peer_spawned) {
                self.rec.event(
                    "timeline_start",
                    json!({"ms": ms, "sync": peer_spawned, "gave_up": gave_up}),
                );
            }
        }
        let Some(tl) = self.timeline.as_mut() else {
            return;
        };
        let clock = tl.clock(ms);
        let steps = tl.due(ms);
        for s in steps {
            self.rec.event(
                "timeline",
                json!({"ms": ms, "clock": clock, "at": s.at_ms, "line": s.line, "verb": s.verb.word(),
                       "args": format!("{:?}", s.verb)}),
            );
            match s.verb {
                crate::timeline::Verb::Throw if alive => {
                    let view = self.shooter_view(now);
                    let f = self
                        .shooter
                        .as_mut()
                        .and_then(|sh| sh.throw_now(now, &view));
                    match f {
                        Some(f) => self.run_actions(now, vec![Action::Throw(f)]),
                        None => self.rec.event(
                            "timeline_skipped",
                            json!({"ms": ms, "verb": "throw", "why": "no shooter or no slot-3 grant"}),
                        ),
                    }
                }
                crate::timeline::Verb::Throw => self.rec.event(
                    "timeline_skipped",
                    json!({"ms": ms, "verb": "throw", "why": "not alive"}),
                ),
                crate::timeline::Verb::Leave => self.quit(now),
                _ => {}
            }
        }
    }

    /// The player with this (lower-case) nick has a character object at a known position that is not the park point
    /// on the floor of the world box: it is spawned (the `sync` condition).
    fn peer_spawned(&self, nick: &str) -> bool {
        let floor = self.table.extents.map(|e| e.min[1]);
        self.obs
            .roster
            .values()
            .filter(|r| r.name.to_ascii_lowercase() == nick)
            .filter_map(|r| self.table.objects.get(&r.character))
            .filter_map(|o| o.pos)
            .any(|p| floor.is_none_or(|f| p[1] > f + 2.0))
    }

    /// Hides the players the timeline does not let the shooter aim at.
    fn timeline_view(&self, view: &mut View) {
        // BOT-46: no shot on the way to a `goto x y z r` point; the bot acts once it stands there
        if let Some(tl) = &self.timeline {
            if tl.goto_radius.is_some() && tl.goto.is_some() && self.goto_logged != tl.goto {
                view.players.clear();
                return;
            }
        }
        if let Some(tl) = &self.timeline {
            let roster = &self.obs.roster;
            tl.filter(&mut view.players, |pid| {
                roster.get(&pid).map(|r| r.name.clone())
            });
        }
    }

    /// Counts the shots of a `fire <n>` step.
    fn timeline_count(&mut self, now: Millis, acts: &[Action]) {
        let n = acts.iter().filter(|a| matches!(a, Action::Fire(_))).count() as u32;
        let ms = self.ms(now);
        if n > 0 && self.timeline.as_mut().is_some_and(|t| t.shots_fired(n)) {
            self.rec.event(
                "timeline_hold",
                json!({"ms": ms, "why": "shot count reached"}),
            );
        }
    }

    /// The current flat distance to the timeline's `approach` player (`None`: no approach, or no position yet).
    fn approach_distance(&self) -> Option<(String, f32)> {
        let nick = self.timeline.as_ref()?.approach.clone()?;
        let p = self.body.as_ref()?.pos();
        let g = self
            .obs
            .roster
            .values()
            .find(|r| r.name.to_ascii_lowercase() == nick)
            .and_then(|r| self.table.objects.get(&r.character))
            .and_then(|o| o.pos)?;
        Some((nick, ((p[0] - g[0]).powi(2) + (p[2] - g[2]).powi(2)).sqrt()))
    }

    /// Logs `approach_reached` each time the bot comes within the approach stop distance of its `approach` player
    /// (again only after it was farther than twice that distance), so a harness can act on the arrival.
    // DC-DEFAULT: BOT-45 (proposed)
    fn approach_tick(&mut self, now: Millis) {
        self.goto_tick(now);
        let Some((nick, d)) = self.approach_distance() else {
            self.approach_near = false;
            return;
        };
        if !self.approach_near && d <= TIMELINE_APPROACH_STOP {
            self.approach_near = true;
            self.rec.event(
                "approach_reached",
                json!({"ms": self.ms(now), "nick": nick, "dist": d}),
            );
        } else if self.approach_near && d > 2.0 * TIMELINE_APPROACH_STOP {
            self.approach_near = false;
        }
    }

    /// Logs `goto_reached {ms, goal, pos, dist}` once per goto point when the bot comes within its reach radius
    /// (`goto x y z r`, else BOT-41's 48 units), so a harness can act on the arrival.
    // DC-DEFAULT: BOT-46 (proposed)
    fn goto_tick(&mut self, now: Millis) {
        let goal = self.timeline.as_ref().and_then(|t| {
            t.goto
                .map(|g| (g, t.goto_radius.unwrap_or(TIMELINE_GOTO_REACHED)))
        });
        let Some((g, r)) = goal else {
            self.goto_logged = None;
            return;
        };
        let Some(p) = self.body.as_ref().map(|b| b.pos()) else {
            return;
        };
        let d = ((p[0] - g[0]).powi(2) + (p[2] - g[2]).powi(2)).sqrt();
        if d <= r && self.goto_logged != Some(g) {
            self.goto_logged = Some(g);
            self.rec.event(
                "goto_reached",
                json!({"ms": self.ms(now), "goal": g, "pos": p, "dist": d}),
            );
        }
    }

    /// The timeline's walk goal while it is not reached: the `approach` player's position, else the `goto` point.
    /// BOT-66: in a flag mode the bot walks to the enemy flag (unless it has a target to chase) and home with a
    /// carried flag.
    // DC-DEFAULT: BOT-66 (proposed)
    /// BOT-70: in Search and Destroy the bomb carrier walks to a plant point; there it holds the use command (0xd8
    /// phase 1) and stands until the 0x51, letting go and trying again after a while. BOT-103: the point of the
    /// team's site this round, not the nearest; an enemy in sight is fought before a new hold. BOT-104: after the
    /// plant a defender walks to the bomb's defuse point and holds there the same way until the 0x50.
    // DC-DEFAULT: BOT-70 (proposed); BOT-103 (proposed); BOT-104 (proposed)
    /// BOT-122: an attacker plants at the point of its job's site and walks its job's way there.
    // DC-DEFAULT: BOT-122 (proposed)
    fn bomb_goal(
        &mut self,
        now: Millis,
        in_sight: bool,
        order: Option<&crate::squad::Order>,
    ) -> Option<[f32; 3]> {
        use crate::bombrun::{
            defuse_point, plant_points, team_point, usable, use_hold_body, PLANT_REACH,
        };
        if !matches!(self.obs.mode_class, Some(2 | 3)) {
            return None;
        }
        let carrying = self.bomb.carrying(self.obs.view_object);
        // BOT-124: with a team job only the defusers go for the defuse point; a cover's walk is its objective
        let defusing = self.bomb.planted && !self.bomb_attacker() && order.is_none_or(|o| o.defuse);
        if !carrying && !defusing {
            return None;
        }
        let pos = self.body.as_ref()?.pos();
        if let Some((t, at)) = self.bomb.hold {
            if now.wrapping_sub(at) < self.bomb.retry_ms() {
                return None;
            }
            self.send_shell(true, crate::bombrun::UP_USE_HOLD, &use_hold_body(4, t), now);
            self.bomb.hold = None;
        }
        let (target, p) = if defusing {
            defuse_point(&self.table)?
        } else {
            let points = usable(plant_points(&self.table), &self.bomb.blocked);
            let job_point = order.and_then(|o| o.plant_at).and_then(|at| {
                points
                    .iter()
                    .copied()
                    .find(|q| (q.1[0] - at[0]).abs() < 1.0 && (q.1[2] - at[2]).abs() < 1.0)
            });
            job_point.or_else(|| team_point(&points, self.bomb.round))?
        };
        let d = ((p[0] - pos[0]).powi(2) + (p[2] - pos[2]).powi(2)).sqrt();
        if in_sight {
            return None;
        }
        // BOT-136: within reach across the floor and at most PLANT_RISE up or down (on Sand Hog a bot on a ledge
        // above a site held 445 s; the server refused every hold: outside the site's zone)
        if d > PLANT_REACH || (p[1] - pos[1]).abs() > crate::bombrun::PLANT_RISE {
            return Some(order.and_then(|o| o.goal).map_or(p, |g| g.goal));
        }
        // BOT-80: the bomb item out first (0x76 select, 0x7f change, as a weapon switch)
        if let Some(c) = self.bomb.draw_item().filter(|_| carrying) {
            let ev = weapon::weapon_event_body(weapon::WEAPON_EVENT_SELECT);
            self.send_shell(true, weapon::up::EVENT, &ev, now);
            self.send_shell(
                true,
                weapon::up::WEAPON_CHANGE,
                &weapon::weapon_change_body(&c),
                now,
            );
            self.rec.event(
                "bomb_drawn",
                json!({"ms": self.ms(now), "weapon": c.weapon_id}),
            );
        }
        self.send_shell(
            true,
            crate::bombrun::UP_USE_HOLD,
            &use_hold_body(1, target),
            now,
        );
        self.bomb.hold = Some((target, now));
        self.rec.event(
            "bomb_hold",
            json!({"ms": self.ms(now), "target": target, "pos": pos, "defuse": defusing}),
        );
        None
    }

    /// BOT-172: starts, ends or keeps the knife run from this tick's walk goal (`goal`, its kind) and what the bot
    /// knows of its surroundings; a run that lapsed (death, a respawn's select) is closed without a draw.
    fn knife_tick(
        &mut self,
        now: Millis,
        goal: Option<[f32; 3]>,
        kind: Option<&'static str>,
        playing: bool,
    ) {
        use crate::shooter::knife::{self, Situation, Verdict};
        let Some(pos) = self.body.as_ref().map(|b| b.pos()) else {
            return;
        };
        let alive = self.life_state == 1 && playing && !self.respawn.dead();
        let view = self.shooter_view(now);
        let Some(sh) = self.shooter.as_mut() else {
            return;
        };
        let Some(level) = sh.config().skill.map(|s| s.level) else {
            return;
        };
        let lapsed = sh.knife_lapsed(alive);
        let contact = if alive {
            sh.contact(now, &view, knife::HEAR_RANGE)
        } else {
            None
        };
        let knifing = sh.knifing();
        let ready = sh.knife_ready();
        if lapsed {
            let why = if alive {
                "reselected"
            } else if self.life_state == 1 && !self.respawn.dead() {
                "round_over"
            } else {
                "died"
            };
            self.knife_closed(now, why);
        }
        if !alive {
            return;
        }
        if let Some(c) = contact {
            self.knife.contact = Some((now, c));
        }
        let flat = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).hypot(a[2] - b[2]);
        let barred = if matches!(
            self.obs.mode_class,
            Some(crate::qrt::QRT_CLASS | crate::spyrun::SPY_CLASS)
        ) {
            Some("mode")
        } else if self.bomb.carrying(self.obs.view_object) {
            Some("bomb_carrier")
        } else {
            None
        };
        let s = Situation {
            kind,
            left: goal.map(|g| self.nav_way_left(pos, g).unwrap_or_else(|| flat(pos, g))),
            contact: self.knife.contact.map(|(t, c)| (c, since(now, t))),
            hurt_ago: self.last_hurt.map(|t| since(now, t)),
            mate_down_ago: self
                .knife
                .mate_down
                .filter(|(_, p)| flat(*p, pos) <= knife::MATE_DOWN_RANGE)
                .map(|(t, _)| since(now, t)),
            barred,
            level,
            ready,
            since_stop: self.knife.stopped.map(|t| since(now, t)),
        };
        match knife::verdict(knifing, &s) {
            Verdict::Start(why) => {
                let acts = self
                    .shooter
                    .as_mut()
                    .map(|sh| sh.draw_knife(now))
                    .unwrap_or_default();
                if acts.is_empty() {
                    return;
                }
                self.knife.started = Some(now);
                self.knife.runs += 1;
                self.rec.event(
                    "knife_run",
                    json!({"ms": self.ms(now), "on": true, "why": why, "kind": kind, "left": s.left,
                           "pos": pos, "goal": goal}),
                );
                self.run_actions(now, acts);
            }
            Verdict::Stop(why) => {
                let acts = self
                    .shooter
                    .as_mut()
                    .map(|sh| sh.end_knife(now))
                    .unwrap_or_default();
                self.knife_closed(now, why);
                self.run_actions(now, acts);
            }
            Verdict::Stay => {}
        }
    }

    /// BOT-172: a knife run ended (`why`): event `knife_run` off with its length.
    fn knife_closed(&mut self, now: Millis, why: &'static str) {
        let ran = self.knife.started.take().map(|t| since(now, t));
        self.knife.stopped = Some(now);
        self.knife.run_ms += u64::from(ran.unwrap_or(0));
        self.rec.event(
            "knife_run",
            json!({"ms": self.ms(now), "on": false, "why": why, "ran_ms": ran,
                   "pos": self.body.as_ref().map(|b| b.pos())}),
        );
    }

    /// The bot's own team (the roster row of its client id).
    fn own_team(&self) -> Option<u8> {
        let own_pid = self.obs.client_id.map(|c| c.0 as u8)?;
        self.obs.roster.get(&own_pid).map(|r| r.team)
    }

    /// BOT-103: whether the bot attacks in this bomb round.
    fn bomb_attacker(&self) -> bool {
        let s = crate::roles::Situation {
            own_team: self.own_team(),
            own_carries_bomb: self.bomb.carrying(self.obs.view_object),
            ..Default::default()
        };
        crate::roles::bomb_attacker(self.role, &s)
    }

    /// BOT-122: the bot's team as it sees it in a bomb round (`None` in other modes, before the roster names the
    /// bot's team, or without plant points).
    fn squad_team(&self) -> Option<crate::squad::Team> {
        if !matches!(
            self.obs.mode_class.or(self.cfg.mode_class_hint),
            Some(2 | 3)
        ) {
            return None;
        }
        let own_pid = self.obs.client_id.map(|c| c.0 as u8)?;
        let own_team = self.obs.roster.get(&own_pid).map(|r| r.team)?;
        let own_pos = self.body.as_ref()?.pos();
        let floor = self.table.extents.map(|e| e.min[1]);
        let mates = self
            .obs
            .roster
            .values()
            .filter(|r| r.team == own_team)
            .map(|r| {
                if r.pid == own_pid {
                    return crate::squad::Mate {
                        pid: r.pid,
                        pos: Some(own_pos),
                        alive: true,
                    };
                }
                let pos = self.table.objects.get(&r.character).and_then(|o| o.pos);
                // BOT-124: a killed teammate's body keeps its place: the roster's dead flag says it fell
                crate::squad::Mate {
                    pid: r.pid,
                    pos,
                    alive: !r.dead && pos.is_some_and(|p| floor.is_none_or(|f| p[1] > f + 2.0)),
                }
            })
            .collect();
        Some(crate::squad::Team {
            own_pid,
            own_pos,
            mates,
            round: self.bomb.round,
            attacking: self.bomb_attacker(),
            sites: crate::bombrun::usable(
                crate::bombrun::plant_points(&self.table),
                &self.bomb.blocked,
            )
            .into_iter()
            .map(|p| p.1)
            .collect(),
            planted: self.bomb.planted,
            bomb_pos: crate::bombrun::defuse_point(&self.table).map(|p| p.1),
            posts: Vec::new(),
            enemy_seen: self.enemy_seen,
            blast_ring: Vec::new(),
            fuse_left: None,
            hold_in_time: false,
            guard_ring: Vec::new(),
            defuser: None,
        })
    }

    /// BOT-142: the place of a defender at the planted bomb as the bot knows it: a living enemy the server streams
    /// (not parked, its stream not silent for more than `shooter::UNSEEN_SILENT_MS`) within `squad::DEFUSER_NEAR`
    /// (flat) and `bombrun::PLANT_RISE` up or down of the defuse point; the nearest of several: (pid, place).
    // DC-DEFAULT: BOT-142 (proposed)
    fn seen_defuser(&self, now: Millis, bomb: Option<[f32; 3]>) -> Option<(u8, [f32; 3])> {
        let bomb = bomb.filter(|_| self.bomb.planted)?;
        let own_team = self.own_team()?;
        let floor = self.table.extents.map(|e| e.min[1]);
        self.obs
            .roster
            .values()
            .filter(|r| r.team != own_team && !r.dead)
            .filter(|r| !self.shooter.as_ref().is_some_and(|s| s.is_down(r.pid)))
            .filter(|r| {
                self.stream_watch
                    .get(&r.character)
                    .is_some_and(|w| since(now, w.1) <= shooter::UNSEEN_SILENT_MS)
            })
            .filter_map(|r| {
                let p = self.table.objects.get(&r.character).and_then(|o| o.pos)?;
                Some((r.pid, p))
            })
            .filter(|(_, p)| floor.is_none_or(|f| p[1] > f + 2.0))
            .map(|(pid, p)| (crate::roles::flat_dist(p, bomb), pid, p))
            .filter(|(d, _, p)| {
                *d <= crate::squad::DEFUSER_NEAR
                    && (p[1] - bomb[1]).abs() <= crate::bombrun::PLANT_RISE
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|x| (x.1, x.2))
    }

    /// BOT-136: the bomb fuse left at `now` (ms; `None` before a plant or when it is not known).
    fn fuse_left(&self, now: Millis) -> Option<u32> {
        let end = self.bomb.fuse_end.filter(|_| self.bomb.planted)?;
        Some(if later(end, now) {
            end.wrapping_sub(now)
        } else {
            0
        })
    }

    /// BOT-136: whether the bot's own running defuse hold ends before the blast.
    fn hold_in_time(&self) -> bool {
        let (Some((_, at)), Some(end)) = (self.bomb.hold, self.bomb.fuse_end) else {
            return false;
        };
        let done = at.wrapping_add(crate::squad::DEFUSE_MS + HOLD_DONE_SLACK_MS);
        self.bomb.planted && !self.bomb_attacker() && later(end, done)
    }

    /// BOT-136: a defender's defuse hold that cannot end before the blast is let go (0xd8 phase 4).
    fn bomb_flee_release(&mut self, now: Millis) {
        if !matches!(self.obs.mode_class, Some(2 | 3)) || !self.bomb.planted || self.bomb_attacker()
        {
            return;
        }
        let Some((target, _)) = self.bomb.hold else {
            return;
        };
        let late = self
            .fuse_left(now)
            .is_some_and(|f| f < crate::squad::DEFUSE_MS + crate::squad::FLEE_MARGIN_MS);
        if late && !self.hold_in_time() {
            let body = crate::bombrun::use_hold_body(4, target);
            self.send_shell(true, crate::bombrun::UP_USE_HOLD, &body, now);
            self.bomb.hold = None;
            self.rec.event(
                "bomb_flee",
                json!({"ms": self.ms(now), "fuse_left": self.fuse_left(now)}),
            );
        }
    }

    /// BOT-136: the places out of the planted bomb's blast (`squad::Ring::BLAST` over the linked nodes of the
    /// bot's grid, a sight line to the bomb at eye height), kept until the bomb moves (3 s while fewer than three,
    /// when the grid also grows toward the bomb); empty before a plant.
    fn blast_ring(&mut self, now: Millis, bomb: Option<[f32; 3]>) -> Vec<[f32; 3]> {
        self.bomb_ring(now, bomb, false)
    }

    /// BOT-142: the attackers' guard places round the planted bomb (`squad::Ring::GUARD` over the linked nodes of
    /// the bot's grid, a sight line from the eye to a crouched defuser's head, `squad::DEFUSER_HEAD` over the
    /// defuse point), kept like the blast ring; empty before a plant and for a defender.
    // DC-DEFAULT: BOT-142 (proposed)
    fn guard_ring(&mut self, now: Millis, bomb: Option<[f32; 3]>) -> Vec<[f32; 3]> {
        if !self.bomb_attacker() {
            self.guard_cache = None;
            return Vec::new();
        }
        self.bomb_ring(now, bomb, true)
    }

    /// The blast ring (BOT-136) or the guard places (BOT-142, `guard`) round the planted bomb, cached per kind.
    fn bomb_ring(&mut self, now: Millis, bomb: Option<[f32; 3]>, guard: bool) -> Vec<[f32; 3]> {
        let cache = if guard {
            &mut self.guard_cache
        } else {
            &mut self.ring_cache
        };
        let Some(bomb) = bomb.filter(|_| self.bomb.planted) else {
            *cache = None;
            return Vec::new();
        };
        if let Some((b, at, ring)) = cache {
            let same = (b[0] - bomb[0]).abs() < 1.0 && (b[2] - bomb[2]).abs() < 1.0;
            if same && (ring.len() >= 3 || since(now, *at) < POSTS_THIN_MS) {
                return ring.clone();
            }
        }
        let world = self.world.as_ref().map(|w| w.collision.clone());
        let eye = 0.5 * BOT_STAND_DIMS[1];
        let (shape, from_eye, to_head) = if guard {
            (
                crate::squad::Ring::GUARD,
                shooter::EYE_FRACTION * BOT_STAND_DIMS[1],
                crate::squad::DEFUSER_HEAD,
            )
        } else {
            (crate::squad::Ring::BLAST, eye, eye)
        };
        let Some(nav) = self.nav.as_mut() else {
            return Vec::new();
        };
        let nodes: Vec<[f32; 3]> = nav
            .links_near(bomb, shape.max)
            .into_iter()
            .filter(|(_, l)| !l.is_empty())
            .map(|(p, _)| p)
            .collect();
        let ring = crate::squad::post_ring(bomb, &nodes, shape, |n, s| {
            world.as_deref().is_none_or(|w| {
                crate::hunt::line_clear(
                    w,
                    [n[0], n[1] + from_eye, n[2]],
                    [s[0], s[1] + to_head, s[2]],
                )
            })
        });
        if ring.len() < 3 {
            nav.expand(POSTS_GROW, Some(bomb));
        }
        let cache = if guard {
            &mut self.guard_cache
        } else {
            &mut self.ring_cache
        };
        *cache = Some((bomb, now, ring.clone()));
        ring
    }

    /// BOT-136: the posts round each of `sites` (`squad::post_set` over the linked nodes of the bot's grid, a sight
    /// line from a node to the site at eye height), kept for 15 s (3 s while a site has fewer than three, when the
    /// grid also grows toward that site) and worked out again with each round.
    // DC-DEFAULT: BOT-136 (proposed)
    fn squad_posts(&mut self, now: Millis, sites: &[[f32; 3]]) -> Vec<Vec<[f32; 3]>> {
        if let Some(c) = &self.posts_cache {
            let thin = c.posts.iter().any(|p| p.len() < 3);
            let keep = if thin { POSTS_THIN_MS } else { POSTS_KEEP_MS };
            if c.round == self.bomb.round && c.sites == sites && since(now, c.at) < keep {
                return c.posts.clone();
            }
        }
        let world = self.world.as_ref().map(|w| w.collision.clone());
        let eye = 0.5 * BOT_STAND_DIMS[1];
        let mut out = Vec::with_capacity(sites.len());
        for site in sites {
            let Some(nav) = self.nav.as_mut() else {
                out.push(Vec::new());
                continue;
            };
            let nodes: Vec<[f32; 3]> = nav
                .links_near(*site, crate::squad::POST_MAX)
                .into_iter()
                .filter(|(_, l)| !l.is_empty())
                .map(|(p, _)| p)
                .collect();
            let posts = crate::squad::post_set(*site, &nodes, |n, s| {
                world.as_deref().is_none_or(|w| {
                    crate::hunt::line_clear(w, [n[0], n[1] + eye, n[2]], [s[0], s[1] + eye, s[2]])
                })
            });
            if posts.len() < 3 {
                nav.expand(POSTS_GROW, Some(*site));
            }
            out.push(posts);
        }
        self.posts_cache = Some(PostCache {
            round: self.bomb.round,
            at: now,
            sites: sites.to_vec(),
            posts: out.clone(),
        });
        out
    }

    /// BOT-136: standing at its post with no target the bot turns its view toward the place it watches (the site,
    /// the bomb) at [`WATCH_TURN`] radians a second.
    fn post_watch_look(&self, now: Millis) -> Option<(f32, f32)> {
        let w = self.squad_order?.watch?;
        let p = self.body.as_ref()?.pos();
        let want = ((w[0] - p[0]).atan2(w[2] - p[2]), 0.0);
        let cur = self.facing.unwrap_or(want);
        let dt = since(now, self.facing_at).min(250);
        Some(crate::skill::turn_toward(
            cur,
            want,
            WATCH_TURN * dt as f32 / 1000.0,
        ))
    }

    /// BOT-122: this tick's order of the bot's team job (a flank waypoint is moved onto the nearest known node of
    /// the way grid within 512 units); a new job or a new wait reason is logged.
    // DC-DEFAULT: BOT-122 (proposed)
    fn squad_tick(&mut self, now: Millis) -> Option<crate::squad::Order> {
        let order = match self.squad_team() {
            Some(mut team) => {
                team.posts = self.squad_posts(now, &team.sites);
                team.blast_ring = self.blast_ring(now, team.bomb_pos);
                team.guard_ring = self.guard_ring(now, team.bomb_pos);
                team.defuser = self.seen_defuser(now, team.bomb_pos).map(|d| d.1);
                team.fuse_left = self.fuse_left(now);
                team.hold_in_time = self.hold_in_time();
                let nav = self.nav.as_ref();
                let mut snap = |p: [f32; 3]| {
                    nav?.links_near(p, 512.0)
                        .into_iter()
                        .map(|n| n.0)
                        .min_by(|a, b| {
                            crate::roles::flat_dist(*a, p)
                                .total_cmp(&crate::roles::flat_dist(*b, p))
                        })
                };
                self.squad.order(now, &team, &mut snap)
            }
            None => None,
        };
        let last = self.squad_order;
        if order.map(|o| (o.job, o.why, o.defuse)) != last.map(|o| (o.job, o.why, o.defuse)) {
            if let Some(o) = order {
                self.rec.event(
                    "squad",
                    json!({"ms": self.ms(now), "job": format!("{:?}", o.job), "wait": o.wait.then_some(o.why),
                           "posts": self.posts_cache.as_ref().map(|c| c.posts.iter().map(Vec::len).collect::<Vec<_>>()),
                           "goal": o.goal.map(|g| g.goal), "plant_at": o.plant_at, "via": self.squad.via(),
                           "round": self.bomb.round, "defuse": o.defuse}),
                );
            }
        }
        self.squad_order = order;
        order
    }

    /// BOT-95: what the round looks like for the role's goal.
    fn situation(&mut self, now: Millis) -> crate::roles::Situation {
        let (flag_job, own_flag_away, flag_guard) = self.flag_tick(now);
        let own_pid = self.obs.client_id.map(|c| c.0 as u8);
        let own_team = own_pid
            .and_then(|p| self.obs.roster.get(&p))
            .map(|r| r.team);
        let own_pos = self.body.as_ref().map_or([0.0; 3], |b| b.pos());
        let floor = self.table.extents.map(|e| e.min[1]);
        let carrier_team = self
            .bomb
            .carrier
            .and_then(|c| self.obs.roster.values().find(|r| r.character == c))
            .map(|r| r.team);
        let carrier_pos = self
            .bomb
            .carrier
            .and_then(|c| self.table.objects.get(&c))
            .and_then(|o| o.pos)
            .filter(|p| floor.is_none_or(|f| p[1] > f + 2.0));
        let standing = |own_side: bool| {
            self.obs
                .roster
                .values()
                .filter(|r| Some(r.pid) != own_pid && !r.dead)
                .filter(|r| own_team.is_some() && (Some(r.team) == own_team) == own_side)
                .filter(|r| {
                    self.table
                        .objects
                        .get(&r.character)
                        .and_then(|o| o.pos)
                        .is_some_and(|p| floor.is_none_or(|f| p[1] > f + 2.0))
                })
                .count()
        };
        let mates_alive = standing(true);
        let enemies_alive = standing(false);
        crate::roles::Situation {
            class: self.obs.mode_class.or(self.cfg.mode_class_hint),
            own_team,
            own_pos,
            bot_no: crate::roles::bot_number(&self.cfg.nick),
            round: self.bomb.round,
            plant_points: crate::bombrun::usable(
                crate::bombrun::plant_points(&self.table),
                &self.bomb.blocked,
            )
            .into_iter()
            .map(|p| p.1)
            .collect(),
            bomb_pos: crate::bombrun::defuse_point(&self.table).map(|p| p.1),
            planted: self.bomb.planted,
            carrier_team,
            carrier_pos,
            own_carries_bomb: self.bomb.carrying(self.obs.view_object),
            own_flag: own_team.and_then(|t| crate::flagrun::flag_pos(&self.table, u32::from(t))),
            own_carries_flag: self.carrying.is_some(),
            own_flag_away,
            flag_job,
            flag_guard,
            mates_alive,
            enemies_alive,
            squad: None,
        }
    }

    /// The other players that live and have a known place (not parked, not dead by the roster or the shooter):
    /// (pid, team, place).
    fn living_others(&self) -> Vec<(u8, u8, [f32; 3])> {
        let own_pid = self.obs.client_id.map(|c| c.0 as u8);
        let floor = self.table.extents.map(|e| e.min[1]);
        self.obs
            .roster
            .values()
            .filter(|r| Some(r.pid) != own_pid && !r.dead)
            .filter(|r| !self.shooter.as_ref().is_some_and(|s| s.is_down(r.pid)))
            .filter_map(|r| {
                let p = self.table.objects.get(&r.character).and_then(|o| o.pos)?;
                floor
                    .is_none_or(|f| p[1] > f + 2.0)
                    .then_some((r.pid, r.team, p))
            })
            .collect()
    }

    /// Spy Hunt (class 9): the uplink with every case, the agent while another player is, else the case of the
    /// bot's spread or the spy with the most cases (the shooter still fires on the way).
    // DC-DEFAULT: BOT-75 (proposed), BOT-127 (proposed), BOT-137 (proposed), BOT-163 (proposed), BOT-164 (proposed)
    fn spy_goal(&mut self, now: Millis) -> Option<([f32; 3], &'static str)> {
        let was_engaged = self.spy_engaged.take();
        if self.obs.mode_class != Some(9) {
            return None;
        }
        let at = self.body.as_ref()?.pos();
        let own = self.obs.client_id.map(|c| u32::from(c.0));
        let others: Vec<(u32, [f32; 3])> = self
            .living_others()
            .into_iter()
            .map(|o| (u32::from(o.0), o.2))
            .collect();
        // BOT-137: a case where no body stands near it, or in danger (the water, a fall volume), is no goal while it
        // lies there
        let fresh = self.spy.unchecked(&self.table, now);
        if !fresh.is_empty() {
            let ms = self.ms(now);
            let world = self.world.as_ref().map(|w| w.collision.clone());
            // BOT-155: a case in a hazard volume that only hurts (Warhead's water) is taken; only a kill volume is
            // danger (BOT-143 put the hazard volumes into the kill list, and Warhead's placed case at (-1123, 2452,
            // -903) was no goal for any bot)
            let hazard = self
                .world
                .as_ref()
                .map(|w| w.hazard_volumes.clone())
                .unwrap_or_default();
            let mut kill = self.kill_now.clone();
            kill.retain(|k| !hazard.contains(k));
            if let (Some(w), Some(nav)) = (world, self.nav.as_mut()) {
                for c in fresh {
                    // BOT-163: a pass while the grid grows is tested again later
                    let provisional = nav.growing() || nav.is_empty();
                    let why = crate::spyrun::case_reach(nav, &w, &kill, c);
                    self.spy.note_reach(c, why, provisional, now);
                    if let Some(why) = why {
                        self.rec.event(
                            "spy_blocked",
                            json!({"ms": ms, "case": c, "pos": at, "why": why}),
                        );
                    }
                }
            }
        }
        // BOT-141: the stall rule follows the way to the case walked to last
        let growing = self.nav.as_ref().is_some_and(|n| n.growing());
        self.spy.way = self.spy.current_case().map(|c| crate::spyrun::SpyWay {
            target: c,
            left: self.follow.way_left(at, c),
            growing: growing && self.follow.waiting_for_way(c),
        });
        let own_pid = self.obs.client_id.map(|c| c.0 as u8);
        self.spy.roster = self
            .obs
            .roster
            .values()
            .filter(|r| Some(r.pid) != own_pid)
            .map(|r| u32::from(r.pid))
            .collect();
        let mut g = self.spy.goal(&self.table, own, at, &others, now);
        // BOT-164: a chaser with the chased player in clear sight fights him from where it stands (the TDM hunt's
        // engagement), and walks its chase again once he is out of sight
        let fighting = self.shooter.as_ref().and_then(|s| {
            s.target()
                .filter(|_| s.target_dist().is_some())
                .map(u32::from)
        });
        let engaged = crate::spyrun::chase_engages(g.as_ref(), fighting);
        if engaged.is_some() && was_engaged != engaged {
            self.rec.event(
                "spy_engage",
                json!({"ms": self.ms(now), "pos": at, "pid": engaged,
                       "dist": self.shooter.as_ref().and_then(Shooter::target_dist)}),
            );
        }
        self.spy_engaged = engaged;
        // BOT-155: a chase with no headway is given up (not while the bot fights the chased player where it stands)
        if engaged.is_some() {
            self.spy_chase_watch = None;
            self.spy_no_way = None;
        } else if let Some(crate::spyrun::SpyGoal::Chase(pid, p)) = g {
            if self.spy_chase_stalled(now, at, pid, p) {
                self.spy.drop_chase(pid, now);
                g = self.spy.goal(&self.table, own, at, &others, now);
            }
        } else {
            self.spy_chase_watch = None;
            self.spy_no_way = None;
        }
        if let Some(c) = self.spy.just_blocked.take() {
            self.rec.event(
                "spy_blocked",
                json!({"ms": self.ms(now), "case": c, "pos": at, "why": "stall",
                       "fails": self.spy.fail_count(&c)}),
            );
        }
        // BOT-163: a case place that defeated the bot's attempts is out of its reach for the match
        if let Some(c) = self.spy.just_given_up.take() {
            self.rec.event(
                "spy_unreachable",
                json!({"ms": self.ms(now), "case": c, "pos": at, "fails": self.spy.fail_count(&c)}),
            );
        }
        // BOT-151: the agent waits away from the hunters until the uplink is about to open; a guard holds its
        // approach of the uplink
        let agent = self.spy.total > 0 && self.spy.held >= self.spy.total;
        if !agent {
            self.spy_lurk = None;
            self.spy_set_off = false;
            self.spy_way = None;
        }
        if !matches!(g, Some(crate::spyrun::SpyGoal::Guard(..))) {
            self.spy_guard = None;
        }
        match g {
            Some(crate::spyrun::SpyGoal::Uplink(u)) if agent => {
                return Some(self.agent_goal(now, at, u, &others));
            }
            Some(crate::spyrun::SpyGoal::Guard(k, u, base)) => {
                return Some(self.guard_goal(now, at, k, u, base));
            }
            // BOT-164: no walk while the chaser fights the chased player in sight (the hunt goal is none then too)
            Some(crate::spyrun::SpyGoal::Chase(..)) if engaged.is_some() => {
                self.spy_retreat = None;
                return None;
            }
            _ => {}
        }
        // BOT-152: a holder one case short waits for the last case while hunters are near it
        if let Some(r) = self.fifth_goal(now, at, g.as_ref(), &others) {
            return Some(r);
        }
        // BOT-137: a holder falls back from a near hunter (its set survives), toward the uplink
        let hunters: Vec<[f32; 3]> = others
            .iter()
            .filter(|o| self.spy.team_of(o.0) == crate::spyrun::HUNTER_TEAM)
            .map(|o| o.1)
            .collect();
        let threat =
            crate::spyrun::keep_away_from(self.spy.held, self.spy.total, g.as_ref(), at, &hunters);
        let Some(t) = threat else {
            self.spy_retreat = None;
            return g.map(|g| (g.pos(), g.kind()));
        };
        let flat = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).hypot(a[2] - b[2]);
        if let Some((r, when)) = self.spy_retreat {
            if since(now, when) < crate::qrt::RETREAT_MS
                && flat(r, at) > crate::qrt::RETREAT_MIN * 0.5
            {
                return Some((r, "spy_keep_away"));
            }
        }
        let uplink = self.spy.uplink(&self.table);
        let nodes: Vec<[f32; 3]> = self
            .nav
            .as_ref()
            .map(|n| n.links_near(at, crate::qrt::RETREAT_MAX))
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, l)| !l.is_empty())
            .map(|(p, _)| p)
            .collect();
        // (no sight bonus: a holder does not look for a place to shoot from)
        let r = crate::qrt::retreat(at, t, &hunters, uplink, &nodes, |_, _| false);
        if self.spy_retreat.is_none() {
            self.rec.event(
                "spy_keep_away",
                json!({"ms": self.ms(now), "pos": at, "hunter": t, "goal": r, "held": self.spy.held}),
            );
        }
        self.spy_retreat = Some((r, now));
        Some((r, "spy_keep_away"))
    }

    /// BOT-155: whether the chase of `pid` (at `p`) makes no headway: the body moved less than
    /// [`crate::spyrun::CHASE_GAIN`] (flat) in [`crate::spyrun::CHASE_STALL_MS`] while he is farther than
    /// [`CHASE_CLOSE`], or the bot walked without a grid way (the steering branch greedy) for
    /// [`crate::spyrun::CHASE_NO_WAY_MS`]; event `spy_chase_drop`.
    // DC-DEFAULT: BOT-155 (proposed)
    fn spy_chase_stalled(&mut self, now: Millis, at: [f32; 3], pid: u32, p: [f32; 3]) -> bool {
        use crate::spyrun as spy;
        let flat = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).hypot(a[2] - b[2]);
        let mut w = match self.spy_chase_watch {
            Some(w) if w.0 == pid => w,
            _ => (pid, now, at),
        };
        if flat(at, w.2) >= spy::CHASE_GAIN || flat(at, p) <= CHASE_CLOSE {
            w = (pid, now, at);
        }
        self.spy_chase_watch = Some(w);
        let no_way = if self.branch == "greedy" {
            since(now, *self.spy_no_way.get_or_insert(now))
        } else {
            self.spy_no_way = None;
            0
        };
        let stalled = since(now, w.1) >= spy::CHASE_STALL_MS;
        if !stalled && no_way < spy::CHASE_NO_WAY_MS {
            return false;
        }
        self.rec.event(
            "spy_chase_drop",
            json!({"ms": self.ms(now), "pos": at, "pid": pid, "target": p,
                   "why": if stalled { "stall" } else { "no_way" }, "no_way_ms": no_way}),
        );
        self.spy_chase_watch = None;
        self.spy_no_way = None;
        true
    }

    /// BOT-152: the walk goal of a holder one case short whose goal is the last case: while
    /// [`crate::spyrun::fifth_waits`] says wait, a grid place off the case away from the hunters (picked like the
    /// agent's waiting place, kept 2 s at least, again when a hunter comes within 600 of it); once it may take the case,
    /// the case itself (no fall-back from hunters: it goes). `None`: the rule does not apply.
    // DC-DEFAULT: BOT-152 (proposed)
    fn fifth_goal(
        &mut self,
        now: Millis,
        at: [f32; 3],
        g: Option<&crate::spyrun::SpyGoal>,
        others: &[(u32, [f32; 3])],
    ) -> Option<([f32; 3], &'static str)> {
        use crate::spyrun as spy;
        let short = self.spy.total >= 2 && self.spy.held + 1 == self.spy.total;
        let case = match g {
            Some(spy::SpyGoal::Case(c)) if short => *c,
            _ => {
                if !short {
                    self.spy_fifth = None;
                }
                self.spy_fifth_place = None;
                return None;
            }
        };
        let since_wait = *self.spy_fifth.get_or_insert(now);
        let flat = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).hypot(a[2] - b[2]);
        let hunters = self.spy.hunter_places(others);
        let holders: Vec<[f32; 3]> = others
            .iter()
            .filter(|o| self.spy.holders.contains_key(&o.0))
            .map(|o| o.1)
            .collect();
        let waited = since(now, since_wait);
        let waits = spy::fifth_waits(
            self.spy.held,
            self.spy.total,
            case,
            &hunters,
            &holders,
            waited,
        );
        if !waits {
            if self.spy_fifth_place.take().is_some() {
                self.rec.event(
                    "spy_fifth_go",
                    json!({"ms": self.ms(now), "pos": at, "case": case, "waited_ms": waited}),
                );
            }
            return Some((case, "spy"));
        }
        let repick = match self.spy_fifth_place {
            None => self.spy_fifth_try.is_none_or(|t| since(now, t) >= 1000),
            Some((p, t)) => {
                since(now, t) >= spy::LURK_HOLD_MS
                    && hunters.iter().any(|h| flat(*h, p) <= spy::LURK_THREAT)
            }
        };
        if repick {
            let nodes: Vec<[f32; 3]> = self
                .nav
                .as_ref()
                .map(|n| n.links_near(case, spy::FIFTH_RING.1))
                .unwrap_or_default()
                .into_iter()
                .filter(|(_, l)| !l.is_empty())
                .map(|(p, _)| p)
                .collect();
            // round the case on the holder's own level (a case on a deck above: the place is below it)
            let level = [case[0], at[1], case[2]];
            self.spy_fifth_try = Some(now);
            let p = spy::safe_place(&nodes, level, spy::FIFTH_RING, at, &hunters);
            self.rec.event(
                "spy_fifth_wait",
                json!({"ms": self.ms(now), "pos": at, "case": case, "goal": p,
                       "hunters": hunters.len(), "waited_ms": waited}),
            );
            self.spy_fifth_place = p.map(|p| (p, now));
        }
        // no place off the case: the usual walk (BOT-137's fall-back still holds)
        self.spy_fifth_place.map(|(p, _)| (p, "spy_wait"))
    }

    /// BOT-151: the Spy Hunt agent's preferred target (`View::prefer`): the living hunter nearest it, kept while it
    /// stays within [`AGENT_FOCUS_KEEP`] of the nearest one's distance (the agent swapped targets among the hunters
    /// round it every few hundred ms and fired 3 shots in 33 s).
    // DC-DEFAULT: BOT-151 (proposed)
    fn spy_focus_pick(&mut self, now: Millis) -> Option<u8> {
        let agent = self.obs.mode_class == Some(crate::spyrun::SPY_CLASS)
            && self.spy.total > 0
            && self.spy.held >= self.spy.total;
        if !agent {
            self.spy_focus = None;
            return None;
        }
        let at = self.body.as_ref()?.pos();
        let d = |p: [f32; 3]| {
            ((p[0] - at[0]).powi(2) + (p[1] - at[1]).powi(2) + (p[2] - at[2]).powi(2)).sqrt()
        };
        let hunters: Vec<(u8, f32)> = self
            .living_others()
            .into_iter()
            .filter(|o| self.spy.team_of(u32::from(o.0)) == crate::spyrun::HUNTER_TEAM)
            .map(|o| (o.0, d(o.2)))
            .collect();
        let nearest = hunters.iter().copied().min_by(|a, b| a.1.total_cmp(&b.1))?;
        let keep = self
            .spy_focus
            .and_then(|(p, _)| hunters.iter().find(|h| h.0 == p))
            .filter(|h| h.1 <= nearest.1 * AGENT_FOCUS_KEEP)
            .map(|h| h.0);
        let pick = keep.unwrap_or(nearest.0);
        self.spy_focus = Some((pick, now));
        Some(pick)
    }

    /// BOT-151: the agent's walk goal: while the uplink `u` is shut and the way there is short enough to wait, a
    /// grid place away from the hunters it knows of ([`crate::spyrun::lurk_place`]); then the uplink, where it stays.
    // DC-DEFAULT: BOT-151 (proposed)
    fn agent_goal(
        &mut self,
        now: Millis,
        at: [f32; 3],
        u: [f32; 3],
        others: &[(u32, [f32; 3])],
    ) -> ([f32; 3], &'static str) {
        use crate::spyrun as spy;
        let flat = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).hypot(a[2] - b[2]);
        // the way to the uplink over the grid, worked out again every 2 s
        if self.spy_way.is_none_or(|w| since(now, w.1) >= 2000) {
            let len = self.nav.as_mut().and_then(|n| n.path(at, u)).map(|p| {
                let mut prev = at;
                let mut l = 0.0;
                for q in p {
                    l += ((q[0] - prev[0]).powi(2)
                        + (q[1] - prev[1]).powi(2)
                        + (q[2] - prev[2]).powi(2))
                    .sqrt();
                    prev = q;
                }
                l + flat(prev, u)
            });
            self.spy_way = Some((len, now));
        }
        let way = self.spy_way.and_then(|w| w.0);
        if self.spy_set_off || self.spy.agent_sets_off(now, at, u, way) {
            if !self.spy_set_off {
                self.spy_set_off = true;
                self.spy_lurk = None;
                self.rec.event(
                    "spy_set_off",
                    json!({"ms": self.ms(now), "pos": at, "uplink": u,
                           "stage_ms": self.spy.stage_ms(now), "dist": flat(at, u), "way": way}),
                );
            }
            return (u, "spy");
        }
        let hunters = self.spy.hunter_places(others);
        let repick = match self.spy_lurk {
            None => true,
            Some((p, t)) => {
                since(now, t) >= spy::LURK_HOLD_MS
                    && hunters.iter().any(|h| flat(*h, p) <= spy::LURK_THREAT)
            }
        };
        if repick {
            let nodes: Vec<[f32; 3]> = self
                .nav
                .as_ref()
                .map(|n| n.links_near(u, spy::LURK_MAX))
                .unwrap_or_default()
                .into_iter()
                .filter(|(_, l)| !l.is_empty())
                .map(|(p, _)| p)
                .collect();
            let p = spy::lurk_place(&nodes, u, at, &hunters);
            if let Some(p) = p {
                if self.spy_lurk.is_none_or(|l| flat(l.0, p) > 1.0) {
                    self.rec.event(
                        "spy_lurk",
                        json!({"ms": self.ms(now), "pos": at, "goal": p, "uplink": u,
                               "hunters": hunters.len(), "nodes": nodes.len(),
                               "stage_ms": self.spy.stage_ms(now)}),
                    );
                }
            }
            self.spy_lurk = p.map(|p| (p, now));
        }
        match self.spy_lurk {
            Some((p, _)) => (p, "spy_lurk"),
            None => (u, "spy"),
        }
    }

    /// BOT-151: a guard's walk goal: its approach of the uplink `u` ([`crate::spyrun::guard_place`]), kept while the
    /// number stays; with no grid place there yet the uplink itself, looked for again after 3 s.
    // DC-DEFAULT: BOT-151 (proposed)
    fn guard_goal(
        &mut self,
        now: Millis,
        at: [f32; 3],
        k: usize,
        u: [f32; 3],
        base: f32,
    ) -> ([f32; 3], &'static str) {
        let fresh = match self.spy_guard {
            Some((gk, Some(_), _)) => gk != k,
            Some((gk, None, t)) => gk != k || since(now, t) >= 3000,
            None => true,
        };
        if fresh {
            let nodes: Vec<[f32; 3]> = self
                .nav
                .as_ref()
                .map(|n| n.links_near(u, crate::spyrun::GUARD_MAX))
                .unwrap_or_default()
                .into_iter()
                .filter(|(_, l)| !l.is_empty())
                .map(|(p, _)| p)
                .collect();
            let p = crate::spyrun::guard_place(&nodes, u, base, k);
            self.rec.event(
                "spy_guard",
                json!({"ms": self.ms(now), "pos": at, "k": k, "goal": p, "uplink": u, "nodes": nodes.len()}),
            );
            self.spy_guard = Some((k, p, now));
        }
        match self.spy_guard {
            Some((_, Some(p), _)) => (p, "spy_guard"),
            _ => (u, "spy_guard"),
        }
    }

    /// BOT-143: after a hazard hit (the server's poison damage type) the bot walks for [`crate::qrt::HAZARD_LEAVE_MS`]
    /// to the nearest safe grid node (no pit; the grid keeps its nodes out of hazard and kill volumes), picked once
    /// per hit.
    // DC-DEFAULT: BOT-143 (proposed)
    fn hazard_goal(&mut self, now: Millis) -> Option<[f32; 3]> {
        use crate::qrt;
        let Some(hit) = self.hazard_hit else {
            self.hazard_leave = None;
            return None;
        };
        if since(now, hit) >= qrt::HAZARD_LEAVE_MS {
            self.hazard_hit = None;
            self.hazard_leave = None;
            return None;
        }
        if self.hazard_leave.is_none() {
            let at = self.body.as_ref()?.pos();
            let g = self
                .nav
                .as_ref()
                .and_then(|n| n.safe_node(at, qrt::HAZARD_LEAVE_RANGE))?;
            self.rec.event(
                "leave_hazard",
                json!({"ms": self.ms(now), "pos": at, "goal": g}),
            );
            self.hazard_leave = Some(g);
        }
        self.hazard_leave
    }

    /// Quarantine (class 11) for a human (team 1): the walk goal (falling back from a near infected, regrouping with
    /// the other humans) and whether the hunt toward an infected is off.
    // DC-DEFAULT: BOT-128 (proposed)
    /// BOT-150: a Quarantine human fires first at the infected nearest the humans' middle (itself included), so
    /// the group's fire meets on one body.
    // DC-DEFAULT: BOT-150 (proposed)
    fn qrt_focus(&self) -> Option<u8> {
        use crate::qrt;
        if self.obs.mode_class != Some(qrt::QRT_CLASS) || self.own_team() != Some(qrt::HUMANS) {
            return None;
        }
        let at = self.body.as_ref()?.pos();
        let others = self.living_others();
        let mut humans: Vec<[f32; 3]> = others
            .iter()
            .filter(|o| o.1 == qrt::HUMANS)
            .map(|o| o.2)
            .collect();
        humans.push(at);
        let infected: Vec<(u8, [f32; 3])> = others
            .iter()
            .filter(|o| o.1 == qrt::INFECTED)
            .map(|o| (o.0, o.2))
            .collect();
        qrt::focus(qrt::centroid(&humans)?, &infected)
    }

    fn qrt_goal(&mut self, now: Millis) -> (Option<([f32; 3], &'static str)>, bool) {
        use crate::qrt;
        let team = self.own_team();
        self.qrt_camp = false;
        self.qrt_plan = None;
        if self.obs.mode_class != Some(qrt::QRT_CLASS) || team != Some(qrt::HUMANS) {
            self.qrt_retreat = None;
            self.qrt_guard = None;
            // BOT-143: an infected bot whose hunt has no goal (no target, or its place was lost) walks to the
            // nearest living human it knows of; BOT-160: one it gains nothing on in 10 s is given up for 30 s
            // DC-DEFAULT: BOT-143 (proposed), BOT-160 (proposed)
            if self.obs.mode_class == Some(qrt::QRT_CLASS)
                && team == Some(qrt::INFECTED)
                && self
                    .shooter
                    .as_ref()
                    .is_some_and(|s| s.hunt_goal().is_none())
            {
                let Some(at) = self.body.as_ref().map(|b| b.pos()) else {
                    return (None, false);
                };
                let humans: Vec<(u8, [f32; 3])> = self
                    .living_others()
                    .iter()
                    .filter(|o| o.1 == qrt::HUMANS && !self.qrt_round.infected.contains(&o.0))
                    .map(|o| (o.0, o.2))
                    .collect();
                let ms = self.ms(now);
                let (g, gave_up) = self.qrt_round.chase_pick(ms, at, &humans);
                if let Some(pid) = gave_up {
                    self.rec.event(
                        "qrt_chase_drop",
                        json!({"ms": ms, "pos": at, "pid": pid, "next": g.map(|g| g.0)}),
                    );
                }
                return (g.map(|g| (g.1, "chase")), false);
            }
            // a hunt goal of the shooter's own: the chase clock starts again with the next chase
            self.qrt_round.chase = None;
            return (None, false);
        }
        let Some(at) = self.body.as_ref().map(|b| b.pos()) else {
            return (None, false);
        };
        // BOT-160: who is infected and whether the outbreak happened come from the server's messages (0xd3 sub 0, the
        // roster's teams), never from the bodies in view (an infected out of view is parked by the server's
        // culling and drops out of `living_others`)
        // DC-DEFAULT: BOT-160 (proposed)
        let ms = self.ms(now);
        let own_pid = self.obs.client_id.map(|c| c.0 as u8);
        let roster_infected = self
            .obs
            .roster
            .values()
            .any(|r| Some(r.pid) != own_pid && r.team == qrt::INFECTED);
        let outbreak = self.qrt_round.outbreak(roster_infected);
        let others = self.living_others();
        let is_infected =
            |o: &(u8, u8, [f32; 3])| o.1 == qrt::INFECTED || self.qrt_round.infected.contains(&o.0);
        let infected: Vec<[f32; 3]> = others.iter().filter(|o| is_infected(o)).map(|o| o.2).collect();
        let mates: Vec<[f32; 3]> = others.iter().filter(|o| !is_infected(o)).map(|o| o.2).collect();
        self.qrt_round.note_seen(ms, &infected);
        let spot = self.qrt_spot(now);
        let flat = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).hypot(a[2] - b[2]);
        // the nearest infected in view, or a place one was seen at moments ago (it went round a corner)
        let recent = self.qrt_round.seen_within(ms, qrt::SEEN_THREAT_MS);
        let threat = qrt::nearest(at, &infected)
            .into_iter()
            .chain(qrt::nearest(at, &recent))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let alone = mates.iter().all(|m| flat(*m, at) > qrt::ALONE);
        // the pull of a fall-back: the stand, else the humans' middle as a safe place (BOT-143)
        let raw_mid = qrt::centroid(&mates);
        let nav = self.nav.as_ref();
        let mid = spot.or_else(|| {
            qrt::regroup_place(raw_mid, &mates, |m| {
                nav.and_then(|n| n.safe_node(m, qrt::REGROUP_SNAP))
            })
        });
        // a human in a running round never hunts nor roams: it keeps to the stand
        self.qrt_camp = true;
        match qrt::human_plan(outbreak, threat.map(|t| t.1), alone) {
            qrt::Plan::FallBack => {
                self.qrt_plan = Some("fall_back");
                // and away from where infected were seen lately
                let mut infected = infected;
                infected.extend(self.qrt_round.seen_within(ms, qrt::SEEN_KEEP_MS));
                if let Some((g, when)) = self.qrt_retreat {
                    if since(now, when) < qrt::RETREAT_MS && flat(g, at) > qrt::RETREAT_MIN * 0.5 {
                        return (Some((g, "keep_away")), true);
                    }
                }
                let t = threat.map_or(at, |t| t.0);
                let nodes: Vec<[f32; 3]> = self
                    .nav
                    .as_ref()
                    .map(|n| n.links_near(at, qrt::RETREAT_MAX))
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|(_, l)| !l.is_empty())
                    .map(|(p, _)| p)
                    .collect();
                let world = self.world.as_ref().map(|w| w.collision.clone());
                let eye = 0.5 * BOT_STAND_DIMS[1];
                let sees = |n: [f32; 3], t: [f32; 3]| {
                    world.as_deref().is_none_or(|w| {
                        crate::hunt::line_clear(
                            w,
                            [n[0], n[1] + eye, n[2]],
                            [t[0], t[1] + eye, t[2]],
                        )
                    })
                };
                let g = qrt::retreat(at, t, &infected, mid, &nodes, sees);
                if self.qrt_retreat.is_none() {
                    self.rec.event(
                        "keep_away",
                        json!({"ms": self.ms(now), "pos": at, "infected": t, "goal": g}),
                    );
                }
                self.qrt_retreat = Some((g, now));
                (Some((g, "keep_away")), true)
            }
            qrt::Plan::Hold => {
                // BOT-160: an infected within reach of the guns, none too close: stand and fire (BOT-150's focus)
                self.qrt_retreat = None;
                self.qrt_plan = Some("hold");
                (None, true)
            }
            qrt::Plan::Post => {
                self.qrt_retreat = None;
                // BOT-160: the own place at the stand: spread round it before the outbreak, at it after
                let Some(spot) = spot else {
                    self.qrt_plan = Some("no_stand");
                    let g = mid.filter(|m| flat(*m, at) > qrt::REGROUP_NEAR);
                    return (g.map(|g| (g, "regroup")), true);
                };
                let (min, max) = if outbreak && qrt::GATHER {
                    (0.0, qrt::POST_RADIUS)
                } else {
                    (qrt::PRE_POST_MIN, qrt::PRE_POST_MAX)
                };
                // a new place with the outbreak (when the humans gather); before it, a place another human stands
                // near is picked again
                if self.qrt_guard.is_some_and(|g| {
                    (qrt::GATHER && g.1 != outbreak)
                        || (!outbreak
                            && since(now, g.2) >= qrt::PRE_REPICK_MS
                            && mates.iter().any(|m| flat(*m, g.0) < qrt::PRE_CROWD))
                }) {
                    self.qrt_guard = None;
                }
                // BOT-161: a place not reached in 15 s is given up for the round
                if let Some(g) = self.qrt_guard.filter(|g| {
                    since(now, g.2) >= qrt::POST_GIVE_UP_MS && flat(g.0, at) > qrt::POST_REACHED
                }) {
                    self.rec.event(
                        "qrt_post_drop",
                        json!({"ms": ms, "pos": at, "post": g.0}),
                    );
                    self.qrt_round.bad.push(g.0);
                    self.qrt_guard = None;
                }
                if self.qrt_guard.is_none() {
                    let bad = self.qrt_round.bad.clone();
                    let nodes: Vec<[f32; 3]> = self
                        .nav
                        .as_ref()
                        .map(|n| n.links_near(spot, max))
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|(p, l)| !l.is_empty() && bad.iter().all(|b| flat(*b, *p) > 64.0))
                        .map(|(p, _)| p)
                        .collect();
                    let slot = own_pid.unwrap_or(0).wrapping_add(self.qrt_round.round);
                    let crowd: &[[f32; 3]] = if outbreak { &[] } else { &mates };
                    let cands = qrt::posts(spot, &nodes, min, max, slot, crowd);
                    // BOT-161: only a place the grid has a short way to; else the reachable place nearest the stand
                    let mut g = None;
                    if let Some(nav) = self.nav.as_mut() {
                        g = cands.iter().take(qrt::POST_TRIES).copied().find(|c| {
                            nav.path(at, *c).is_some_and(|p| qrt::way_reaches(at, &p, *c))
                        });
                        if g.is_none() {
                            g = nav
                                .path(at, spot)
                                .and_then(|p| p.last().copied())
                                .filter(|p| bad.iter().all(|b| flat(*b, *p) > 64.0));
                        }
                    }
                    if let Some(g) = g {
                        self.rec.event(
                            "qrt_post",
                            json!({"ms": ms, "pos": at, "spot": spot, "post": g, "outbreak": outbreak}),
                        );
                    }
                    // no way known yet: toward the stand (the grid grows on the way), or stay once it was given up
                    let g = g.unwrap_or(if bad.iter().any(|b| flat(*b, spot) <= 64.0) {
                        at
                    } else {
                        spot
                    });
                    self.qrt_guard = Some((g, outbreak, now));
                }
                let g = self.qrt_guard.map_or(spot, |g| g.0);
                if flat(g, at) <= qrt::POST_REACHED {
                    self.qrt_plan = Some("post_stand");
                    (None, true)
                } else {
                    self.qrt_plan = Some("post");
                    (Some((g, "post")), true)
                }
            }
        }
    }

    /// BOT-160: the Quarantine humans' stand of this round: the world's spawn places ranked (once per world) by
    /// [`crate::qrt::rank_spots`] (rays through the world's collision, no place in danger), the round's turn of
    /// them. Every bot of the room reads the same world and round, so all pick the same stand.
    // DC-DEFAULT: BOT-160 (proposed)
    fn qrt_spot(&mut self, now: Millis) -> Option<[f32; 3]> {
        use crate::qrt;
        if let Some(s) = self.qrt_round.spot {
            return Some(s);
        }
        let w = self.world.as_ref()?;
        if self.qrt_spots.as_ref().is_none_or(|s| s.0 != w.sha16) {
            let ranked = qrt::world_spots(&w.collision, &self.kill_now, &w.spawn_points);
            self.rec.event(
                "qrt_spots",
                json!({"ms": self.ms(now), "spawns": w.spawn_points.len(),
                       "ranked": ranked.iter().take(qrt::SPOT_TURNS).collect::<Vec<_>>()}),
            );
            self.qrt_spots = Some((w.sha16.clone(), ranked));
        }
        let s = qrt::spot_for_round(&self.qrt_spots.as_ref()?.1, self.qrt_round.round)?;
        self.rec.event(
            "qrt_spot",
            json!({"ms": self.ms(now), "round": self.qrt_round.round, "spot": s}),
        );
        self.qrt_round.spot = Some(s);
        Some(s)
    }

    /// BOT-125: the bot's flag job from its team's roster rows (logged when it changes), whether its own flag is off
    /// its base, and its guard place (moved onto the nearest node of the way grid within 512 units). Also notes
    /// where each flag at its base stands.
    // DC-DEFAULT: BOT-125 (proposed)
    fn flag_tick(
        &mut self,
        now: Millis,
    ) -> (Option<crate::roles::FlagJob>, bool, Option<[f32; 3]>) {
        if !matches!(
            self.obs.mode_class.or(self.cfg.mode_class_hint),
            Some(7 | 8)
        ) {
            return (None, false, None);
        }
        for team in [1u32, 2] {
            if !self.flag_away.contains(&team) {
                if let Some(p) = crate::flagrun::flag_pos(&self.table, team) {
                    self.flag_home.entry(team).or_insert(p);
                }
            }
        }
        let Some(own_pid) = self.obs.client_id.map(|c| c.0 as u8) else {
            return (None, false, None);
        };
        let Some(own_team) = self.obs.roster.get(&own_pid).map(|r| r.team) else {
            return (None, false, None);
        };
        let pids: Vec<u8> = self
            .obs
            .roster
            .values()
            .filter(|r| r.team == own_team)
            .map(|r| r.pid)
            .collect();
        let job = crate::roles::flag_job(&pids, own_pid);
        if job != self.flag_job {
            self.flag_job = job;
            if let Some(j) = job {
                self.rec.event(
                    "flag_job",
                    json!({"ms": self.ms(now), "role": format!("{:?}", j.role), "guard": j.guard,
                           "guards": j.guards, "team": own_team, "members": pids}),
                );
            }
        }
        let team = u32::from(own_team);
        let away = self.flag_away.contains(&team);
        let job = self.flag_rove(now, job, team, own_team, away);
        let flat = crate::roles::flat_dist;
        // BOT-144: the team's other guards (their bodies), from the same job list
        let floor = self.table.extents.map(|e| e.min[1]);
        let mate_guards: Vec<(usize, [f32; 3])> = pids
            .iter()
            .filter(|p| **p != own_pid)
            .filter_map(|p| {
                let j = crate::roles::flag_job(&pids, *p)?;
                let r = self.obs.roster.get(p).filter(|r| !r.dead)?;
                let at = self.table.objects.get(&r.character).and_then(|o| o.pos)?;
                (j.role == crate::roles::Role::Defend && floor.is_none_or(|f| at[1] > f + 2.0))
                    .then_some((j.guard, at))
            })
            .collect();
        let body = self.body.as_ref().map(|b| b.pos());
        let guard = job
            .filter(|j| j.role == crate::roles::Role::Defend && !away)
            .and_then(|j| {
                let flag = crate::flagrun::flag_pos(&self.table, team)?;
                let turn = self.ms(now) / crate::roles::FLAG_GUARD_TURN_MS;
                // BOT-138: one place per turn of the ring, a node of the ring's band, another than the last one;
                // BOT-144: the later guard of two picks again when its place lies by an earlier guard
                if let Some((_, at)) = self.flag_guard_at.filter(|g| g.0 == turn) {
                    let crowded = mate_guards
                        .iter()
                        .any(|(i, m)| *i < j.guard && flat(*m, at) < crate::roles::GUARD_MOVE);
                    if !crowded {
                        return Some(at);
                    }
                }
                let p = crate::roles::flag_guard_point(flag, j, turn);
                let mut avoid: Vec<[f32; 3]> = mate_guards.iter().map(|m| m.1).collect();
                avoid.extend(self.flag_guard_at.map(|g| g.1));
                avoid.extend(body);
                let snapped = self.nav.as_ref().and_then(|nav| {
                    let nodes: Vec<[f32; 3]> = nav
                        .links_near(flag, crate::roles::GUARD_FAR + 64.0)
                        .into_iter()
                        .map(|n| n.0)
                        .collect();
                    crate::roles::snap_guard(flag, p, &nodes, &avoid)
                });
                let at = snapped.unwrap_or(p);
                self.flag_guard_at = Some((turn, at));
                Some(at)
            });
        (job, away, guard)
    }

    /// BOT-144: the last guard of two attacks after [`crate::roles::ROVE_QUIET_MS`] at its post with the flag at base
    /// and no enemy known within [`crate::roles::ROVE_THREAT`] of it; a guard again once the flag leaves the base or
    /// the bot dies. Returns the job the bot plays (`flag_rove` logged when it changes).
    // DC-DEFAULT: BOT-144 (proposed)
    fn flag_rove(
        &mut self,
        now: Millis,
        job: Option<crate::roles::FlagJob>,
        team: u32,
        own_team: u8,
        away: bool,
    ) -> Option<crate::roles::FlagJob> {
        let Some(j) = job.filter(|j| crate::roles::may_rove(*j)) else {
            self.flag_quiet_since = None;
            self.flag_roving = false;
            return job;
        };
        let roving = if away || self.life_state != 1 {
            self.flag_quiet_since = None;
            false
        } else if self.flag_roving {
            true
        } else {
            let flag = self.flag_home.get(&team).copied();
            let threat = flag.is_some_and(|f| {
                self.living_others().iter().any(|(_, t, p)| {
                    *t != own_team && crate::roles::flat_dist(*p, f) <= crate::roles::ROVE_THREAT
                })
            });
            if threat || flag.is_none() {
                self.flag_quiet_since = Some(now);
            }
            let since = *self.flag_quiet_since.get_or_insert(now);
            now.wrapping_sub(since) >= crate::roles::ROVE_QUIET_MS
        };
        if roving != self.flag_roving {
            self.flag_roving = roving;
            self.rec.event(
                "flag_rove",
                json!({"ms": self.ms(now), "roving": roving, "guard": j.guard, "guards": j.guards}),
            );
        }
        Some(if roving {
            crate::roles::FlagJob {
                role: crate::roles::Role::Attack,
                guard: 0,
                guards: j.guards,
            }
        } else {
            j
        })
    }

    /// BOT-66: the flag run. BOT-125: a defender (flag job) runs no flags unless it carries one; an enemy suspends
    /// the run only in clear sight within [`crate::roles::OBJECTIVE_ENGAGE`] (`in_sight`) and never the carrier's;
    /// the carrier heads for its flag's base (the capture needs the own flag there), the run's old goal (the own
    /// flag where it is) while the base is unknown. BOT-138: a loose flag (either team's) comes first for the bots
    /// [`crate::flagrun::loose_rush`] sends, whatever their job (within the grab distance even with an enemy in
    /// sight; a carrier only for its own flag within that distance); a flag found out of reach is not walked to
    /// (the attacker hunts instead).
    // DC-DEFAULT: BOT-125 (proposed); BOT-138 (proposed)
    fn flag_goal(&mut self, now: Millis, in_sight: bool) -> Option<[f32; 3]> {
        use crate::flagrun::{flag_pos, loose_rush, unreachable, LOOSE_GRAB};
        self.flag_walk_team = None;
        if !matches!(self.obs.mode_class, Some(7 | 8)) {
            return None;
        }
        let own_pid = self.obs.client_id.map(|c| c.0 as u8)?;
        let team = self.obs.roster.get(&own_pid).map(|r| r.team)?;
        let own = u32::from(team);
        let enemy = if own == 1 { 2 } else { 1 };
        let flat = crate::roles::flat_dist;
        if let Some(pos) = self.body.as_ref().map(|b| b.pos()) {
            let mates = self.flag_mates(own_pid, team);
            let mut best: Option<(f32, u32, [f32; 3])> = None;
            for t in [own, enemy] {
                let Some(fell) = self.flag_loose.get(&t) else {
                    continue;
                };
                let Some(p) = flag_pos(&self.table, t) else {
                    continue;
                };
                if unreachable(self.flag_unreach, t, p, now) {
                    continue;
                }
                let d = flat(pos, p);
                let rush = if self.carrying.is_some() {
                    t == own && d <= LOOSE_GRAB
                } else {
                    loose_rush(pos, p, &mates, now.wrapping_sub(*fell))
                        && (d <= LOOSE_GRAB || !in_sight)
                };
                if rush && best.is_none_or(|b| d < b.0) {
                    best = Some((d, t, p));
                }
            }
            if let Some((_, t, p)) = best {
                self.flag_walk_team = Some(t);
                return Some(p);
            }
        }
        if self.carrying.is_none() {
            if self
                .flag_job
                .is_some_and(|j| j.role == crate::roles::Role::Defend)
                && !self.flag_roving
            {
                return None;
            }
            // BOT-144: the enemy flag is with a team mate and the own flag off its base: take the own flag back (the
            // enemy carrier, the loose flag) like the defenders, so the mate can capture; with the own flag out of
            // reach or at its base the bot does not walk to the mate either (it stood on the carrier at the base:
            // Sand Hog 49 % of the time within 36 units), it hunts and roams
            let mate_carries = self.flag_carrier.get(&enemy).is_some_and(|c| {
                u8::try_from(*c)
                    .ok()
                    .and_then(|c| self.obs.roster.get(&c))
                    .is_some_and(|r| r.team == team)
            });
            if mate_carries && self.flag_away.contains(&own) && !in_sight {
                if let Some(p) = flag_pos(&self.table, own)
                    .filter(|p| !unreachable(self.flag_unreach, own, *p, now))
                {
                    self.flag_walk_team = Some(own);
                    return Some(p);
                }
            }
            if mate_carries {
                return None;
            }
            let p = flag_pos(&self.table, enemy)?;
            if unreachable(self.flag_unreach, enemy, p, now) {
                return None;
            }
            // BOT-144: within the grab distance of the enemy flag the attacker runs on for it with an enemy in sight
            // (it fights on the way; before, it stopped at the guards' ring and fought them there)
            let near = self
                .body
                .as_ref()
                .is_some_and(|b| flat(b.pos(), p) <= crate::flagrun::FLAG_DASH);
            if in_sight && !near {
                return None;
            }
            self.flag_walk_team = Some(enemy);
            return Some(p);
        }
        self.flag_walk_team = Some(own);
        if let Some(home) = self.flag_home.get(&own) {
            return Some(*home);
        }
        crate::flagrun::goal(&self.table, team, self.carrying)
    }

    /// BOT-138: the places of the bot's living teammates (streamed, not parked), flag carriers left out.
    fn flag_mates(&self, own_pid: u8, team: u8) -> Vec<[f32; 3]> {
        let floor = self.table.extents.map(|e| e.min[1]);
        self.obs
            .roster
            .values()
            .filter(|r| r.pid != own_pid && !r.dead && r.team == team)
            .filter(|r| !self.flag_carrier.values().any(|c| *c == u32::from(r.pid)))
            .filter_map(|r| self.table.objects.get(&r.character).and_then(|o| o.pos))
            .filter(|p| floor.is_none_or(|f| p[1] > f + 2.0))
            .collect()
    }

    /// BOT-138: the watch over the flag walk of this tick (`goal` with its kind): one that makes no way outside the
    /// touch for [`crate::flagrun::STALL_MS`] is reported (`flag_unreachable`: the flag, the bot's place, the grid
    /// node nearest the flag, the way's state) and, unless the bot carries a flag, that flag is out of reach for a
    /// while.
    fn flag_reach(
        &mut self,
        now: Millis,
        pos: Option<[f32; 3]>,
        goal: Option<([f32; 3], &'static str)>,
    ) {
        let Some(pos) = pos else {
            return;
        };
        // a carrier within the server's capture distance of its base waits there for its own flag: no watch
        let carrier_home = |g: [f32; 3]| {
            self.carrying.is_some()
                && crate::roles::flat_dist(pos, g) <= crate::flagrun::CARRIER_HOME
        };
        let walk = self.flag_walk_team.zip(
            goal.filter(|g| g.1 == "flag" && !carrier_home(g.0))
                .map(|g| g.0),
        );
        let left = walk.and_then(|(_, g)| self.nav_way_left(pos, g));
        if !self.flag_walk.tick(now, pos, walk, left) {
            return;
        }
        let Some((team, flag)) = walk else {
            return;
        };
        let flat = crate::roles::flat_dist;
        let node = self.nav.as_ref().and_then(|nav| {
            nav.links_near(flag, 600.0)
                .into_iter()
                .map(|n| n.0)
                .min_by(|a, b| flat(*a, flag).total_cmp(&flat(*b, flag)))
        });
        let carrier = self.carrying.is_some();
        if !carrier {
            self.flag_unreach = Some((team, flag, now));
        }
        self.rec.event(
            "flag_unreachable",
            json!({"ms": self.ms(now), "flag_team": team, "flag": flag, "pos": pos,
                   "dist": flat(pos, flag), "node": node, "node_dist": node.map(|n| flat(n, flag)),
                   "whole": self.follow.whole(), "at_end": self.follow.at_end(),
                   "island": self.follow.island(), "arrived": self.nav_arrived.is_some(),
                   "carrier": carrier, "loose": self.flag_loose.contains_key(&team)}),
        );
    }

    fn timeline_goal(&self) -> Option<[f32; 3]> {
        let tl = self.timeline.as_ref()?;
        let p = self.body.as_ref()?.pos();
        let flat = |g: [f32; 3]| ((p[0] - g[0]).powi(2) + (p[2] - g[2]).powi(2)).sqrt();
        if let Some(nick) = &tl.approach {
            let at = self
                .obs
                .roster
                .values()
                .find(|r| r.name.to_ascii_lowercase() == *nick)
                .and_then(|r| self.table.objects.get(&r.character))
                .and_then(|o| o.pos);
            return at.filter(|g| flat(*g) > TIMELINE_APPROACH_STOP);
        }
        let g = tl.goto?;
        (flat(g) > tl.goto_radius.unwrap_or(TIMELINE_GOTO_REACHED)).then_some(g)
    }

    /// The (yaw, pitch) toward the timeline's look point from the eye.
    fn timeline_look(&self) -> Option<(f32, f32)> {
        let to = self.timeline.as_ref()?.look?;
        let p = self.body.as_ref()?.pos();
        let eye = [p[0], p[1] + shooter::EYE_FRACTION * BOT_STAND_DIMS[1], p[2]];
        Some(shooter::aim_angles(eye, to))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::{MoveTrailer, PlayerStatus};

    fn shell_message(id: u8, body: impl FnOnce(&mut BitWriter)) -> BitWriter {
        let mut w = BitWriter::new();
        w.write_u8(s2c::SHELL_MSG).write_u8(id);
        body(&mut w);
        w
    }

    fn external_session() -> BotSession {
        let mut cfg = GameConfig::new("Ext");
        cfg.external_body = true;
        cfg.external_load = true;
        cfg.queue_events = true;
        let mut s = BotSession::new(cfg, None, Recorder::null("Ext"));
        s.start = Some(0);
        s.stage = Stage::WorldLoaded;
        s
    }

    #[test]
    fn a_paused_bot_sends_no_fire_and_a_resumed_one_does() {
        let f = crate::weapon::FireReport {
            wkind: 0,
            weapon_id: 0x91,
            ammo_id: 0x91,
            weapon_pos: [0.0; 3],
            origin: [0.0; 3],
            aim: [0.0, 0.0, 0.0, 1.0],
            yaw: 0.0,
            dir: [0.0, 0.0, 1.0],
            seed: 1,
            moving: false,
            wflag: true,
            mode1: false,
            frac: 0,
            shot_no: 1,
            time_ms: 0,
            extra: None,
            flag_c: false,
            last: true,
            hit: None,
            stat: 0,
        };
        let p = std::env::temp_dir().join(format!("dc_ctl_sess_{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut s = external_session();
        s.control = crate::control::Control::new(Some(p.clone()), "Ext");
        let fires = |s: &BotSession| {
            s.obs
                .c2s_shell_counts
                .get(&weapon::up::FIRE)
                .copied()
                .unwrap_or(0)
        };
        std::fs::write(
            &p, "pause
",
        )
        .unwrap();
        s.control_tick(1000);
        assert!(s.control.paused());
        s.run_actions(1000, vec![Action::Fire(vec![f]), Action::Throw(f)]);
        assert_eq!(fires(&s), 0);
        std::fs::remove_file(&p).unwrap();
        s.control_tick(2000);
        s.run_actions(2000, vec![Action::Fire(vec![f])]);
        assert_eq!(fires(&s), 1);
    }

    fn report(seq: u8, x: f32, yaw: f32) -> MoveReport {
        MoveReport {
            angles: [0.0, yaw, 0.0],
            aux: [0; 6],
            aim: Some([0.0, (yaw * 0.5).sin(), 0.0, (yaw * 0.5).cos()]),
            control: 0x201,
            trailer: MoveTrailer {
                move_seq: seq,
                pos: [x, 40.0, 0.0],
                vel: [0.0; 3],
                time: 1.0,
                on_ground: 1,
                surface: 0,
                standing: u64::MAX,
            },
        }
    }

    #[test]
    fn roster_query_asks_for_records_whose_object_is_unknown_on_the_roster_tick() {
        let rec = Recorder::null("Ext").keep_in_memory();
        rec.set_log_transport(true);
        let mut cfg = GameConfig::new("Ext");
        cfg.external_body = true;
        cfg.external_load = true;
        cfg.roster_query = true;
        let mut s = BotSession::new(cfg, None, rec.clone());
        s.start = Some(0);
        s.stage = Stage::WorldLoaded;
        s.world_ready_sent = true;
        let record = |pid: u8, character: u16| {
            shell_message(shell_id::GS_PLAYER_INFO, |w| {
                w.write_zstr8(b"p")
                    .write_u8(pid)
                    .write_u32(1000 + pid as u32);
                w.write_bit(false).write_u16(character);
                for _ in 0..12 {
                    w.write_u32(0);
                }
            })
        };
        s.game_tick(0); // the first tick sets the roster clock (next tick at 1000 ms)
        s.process(10, &record(0, 408)); // object 408 not created yet: queued
        s.process(11, &record(1, 0xFFFF)); // no character: nothing to ask
        s.process(12, &record(0, 408)); // queued once
        for t in (20..1000).step_by(10) {
            s.game_tick(t);
        }
        let queries = |rec: &Recorder| -> Vec<String> {
            rec.memory()
                .iter()
                .filter(|e| e["ev"] == "msg_out" && e["shell"] == 0x8c)
                .filter_map(|e| e["head"].as_str().map(str::to_string))
                .collect()
        };
        assert!(queries(&rec).is_empty(), "nothing before the roster tick");
        s.game_tick(1000);
        assert_eq!(queries(&rec), vec!["0b8c00000000"], "one query for pid 0");
        s.game_tick(2000);
        assert_eq!(queries(&rec).len(), 1, "the queue was cleared");
        // off by default (the bot)
        assert!(!GameConfig::new("Bot").roster_query);
    }

    #[test]
    fn extras_carry_the_eye_offset_always_and_the_camera_point_when_it_moved() {
        let rec = Recorder::null("Ext").keep_in_memory();
        rec.set_log_transport(true);
        let mut cfg = GameConfig::new("Ext");
        cfg.external_body = true;
        cfg.external_load = true;
        cfg.update_hz = 15.0; // the test steps the 2009 gate (BOT-96 sends 30 by default)
        let mut s = BotSession::new(cfg, None, rec.clone());
        s.start = Some(0);
        s.stage = Stage::WorldLoaded;
        s.process(
            5,
            &shell_message(shell_id::GS_PLAYER_STATE, |w| {
                w.write_u8(1);
            }),
        );
        let x = UpdateExtras {
            eye_offset: Some([0.0, 33.0, 0.0]),
            weapon_f: None,
            cam_point: Some([2.0, 73.0, 0.0]),
            flag_bits: shell::pu::FLAG_1000,
        };
        s.set_player_update_ext(report(4, 2.0, 0.5), x);
        for t in (10..140).step_by(5) {
            s.game_tick(t);
        }
        // the camera moves: the next update carries the point again
        let moved = UpdateExtras {
            cam_point: Some([3.0, 73.0, 0.0]),
            ..x
        };
        s.set_player_update_ext(report(4, 3.0, 0.5), moved);
        for t in (140..210).step_by(5) {
            s.game_tick(t);
        }
        let flags: Vec<String> = rec
            .memory()
            .iter()
            .filter(|e| e["ev"] == "msg_out" && e["shell"] == 0x64)
            .filter_map(|e| e["head"].as_str().map(|h| h[4..8].to_string()))
            .collect();
        // pu_flags little-endian: 0x1521 first (eye, view, aim, camera, 0x1000), 0x1021 when nothing changed
        assert_eq!(flags, vec!["2115", "2110", "2114"], "{flags:?}");
        // plain updates (set_player_update) drop the extras again
        s.set_player_update(report(4, 3.0, 0.5));
        assert!(s.external_extras.is_none());
    }

    #[test]
    fn weapon_float_goes_out_only_when_it_changed_and_weapon_messages_become_events() {
        let rec = Recorder::null("Ext").keep_in_memory();
        rec.set_log_transport(true);
        let mut cfg = GameConfig::new("Ext");
        cfg.external_body = true;
        cfg.external_load = true;
        cfg.queue_events = true;
        cfg.update_hz = 15.0; // the test steps the 2009 gate (BOT-96 sends 30 by default)
        let mut s = BotSession::new(cfg, None, rec.clone());
        s.start = Some(0);
        s.stage = Stage::WorldLoaded;
        s.process(
            5,
            &shell_message(shell_id::GS_PLAYER_STATE, |w| {
                w.write_u8(1);
            }),
        );
        let x = UpdateExtras {
            eye_offset: Some([0.0, 33.0, 0.0]),
            weapon_f: Some(0.0),
            cam_point: None,
            flag_bits: shell::pu::FLAG_1000,
        };
        s.set_player_update_ext(report(4, 2.0, 0.5), x);
        for t in (10..140).step_by(5) {
            s.game_tick(t);
        }
        s.set_player_update_ext(
            report(4, 2.0, 0.5),
            UpdateExtras {
                weapon_f: Some(0.05),
                ..x
            },
        );
        for t in (140..210).step_by(5) {
            s.game_tick(t);
        }
        let flags: Vec<String> = rec
            .memory()
            .iter()
            .filter(|e| e["ev"] == "msg_out" && e["shell"] == 0x64)
            .filter_map(|e| e["head"].as_str().map(|h| h[4..8].to_string()))
            .collect();
        // 0x1321 (weapon float, aim), then 0x1021, then 0x1221 when the float changed
        assert_eq!(flags, vec!["2113", "2110", "2112"], "{flags:?}");
        // 0xb2 / 0xb4 / 0x74 reach the caller as events
        s.process(
            220,
            &shell_message(weapon::gs::WEAPON_GIVE, |w| {
                w.write_u16(145)
                    .write_u16(145)
                    .write_f32(120.0)
                    .write_u8(0)
                    .write_u8(0);
                w.write_u32(u32::MAX);
            }),
        );
        s.process(
            221,
            &shell_message(weapon::gs::WEAPON_REMOVE, |w| {
                w.write_u8(2).write_u16(0).write_u16(0).write_u32(0);
            }),
        );
        let ev = s.take_events();
        assert!(ev
            .iter()
            .any(|e| matches!(e, GameEvent::WeaponGrant(g) if g.weapon == 145 && g.slot == 0)));
        assert!(ev.contains(&GameEvent::WeaponRemove { slot: 2 }));
    }

    #[test]
    fn external_body_sends_the_callers_update_under_the_gate_and_queues_events() {
        let mut s = external_session();
        s.cfg.update_hz = 15.0; // the test counts the 2009 gate (BOT-96 sends 30 by default)
        s.set_player_update(report(0, 1.0, 0.5));
        s.game_tick(10);
        assert_eq!(s.obs.updates_sent, 0, "no update before player state 1");
        // spawn set of the server: teleport, facing, health, state 1
        let tp = shell_message(shell_id::GS_MOVER_UPDATE, |w| {
            w.write_u16(0x0020).write_u8(4);
            for v in [10.0f32, 60.0, -5.0, 24.0, 53.0, 24.0] {
                w.write_f32(v);
            }
        });
        s.process(20, &tp);
        s.process(
            21,
            &shell_message(shell_id::GS_VIEW_ANGLES, |w| {
                w.write_u8(0).write_f32(0.0).write_f32(1.5).write_f32(0.0);
            }),
        );
        s.process(
            22,
            &shell_message(shell_id::GS_PLAYER_STATUS, |w| {
                w.write_u8(2).write_u8(0).write_u8(0).write_f32(100.0);
            }),
        );
        s.process(
            23,
            &shell_message(shell_id::GS_PLAYER_STATE, |w| {
                w.write_u8(1);
            }),
        );
        assert!(s.player_spawned());
        assert!(s.body().is_none(), "no bot body in external mode");
        let ev = s.take_events();
        assert_eq!(
            ev[0],
            GameEvent::Teleport {
                seq: 4,
                pos: [10.0, 60.0, -5.0],
                dims: [24.0, 53.0, 24.0]
            }
        );
        assert!(matches!(ev[1], GameEvent::ViewAngles { kind: 0, .. }));
        assert!(matches!(
            ev[2],
            GameEvent::Status(PlayerStatus::Value { kind: 2, value, .. }) if value == 100.0
        ));
        assert_eq!(ev[3], GameEvent::PlayerState(1));
        assert!(s.take_events().is_empty());
        // updates at most 15 per second; the aim block only when it changed
        s.set_player_update(report(4, 2.0, 0.5));
        for t in (30..1030).step_by(5) {
            s.game_tick(t);
        }
        assert_eq!(s.obs.updates_sent, 15, "15 per second");
        assert_eq!(
            s.obs.c2s_shell_counts.get(&shell_id::UP_PLAYER_UPDATE),
            Some(&15)
        );
        assert!(s.last_aim.is_some());
        // death: no updates in state 2
        s.process(
            1100,
            &shell_message(shell_id::GS_PLAYER_STATE, |w| {
                w.write_u8(2);
            }),
        );
        let before = s.obs.updates_sent;
        for t in (1100..1500).step_by(5) {
            s.game_tick(t);
        }
        assert_eq!(s.obs.updates_sent, before);
    }

    #[test]
    fn external_world_load_holds_the_session_until_the_caller_reports() {
        let mut s = external_session();
        s.cfg.load_ms = 0;
        s.obs.files.insert(3, (0, "Worlds/Synth.dat".into()));
        let mut lw = BitWriter::new();
        lw.write_u8(s2c::LOAD_WORLD).write_f32(0.0).write_u16(3);
        s.process(5, &lw);
        assert_eq!(s.world_request(), Some("Worlds/Synth.dat"));
        assert_eq!(s.stage(), Stage::LoadingWorld);
        s.poll(50);
        assert_eq!(s.stage(), Stage::LoadingWorld, "waits for the caller");
        let b = crate::objects::ExtentBox {
            min: [-1.0; 3],
            max: [1.0; 3],
        };
        s.world_loaded(60, b, "synthetic");
        assert_eq!(s.world_request(), None);
        s.poll(61);
        assert_eq!(s.stage(), Stage::WorldLoaded);
        assert_eq!(s.table.extents, Some(b));
        assert!(s.obs.reached("S6a_world_loaded_sent"));
        // a failed load ends the session with 0x29
        let mut f = external_session();
        f.obs.files.insert(3, (0, "Worlds/X.dat".into()));
        f.process(5, &lw);
        f.world_failed(6, "not in the ROM");
        assert_eq!(f.stage(), Stage::Dropped(result::WORLD_MISSING));
    }

    #[test]
    fn chat_and_player_left_are_handed_over_whole() {
        // the client HUD reads 0x68 itself (S2C §2.4) and the `kicked` byte of 0x6c (S2C §3.2)
        let mut s = external_session();
        s.process(
            1,
            &shell_message(0x68, |w| {
                w.write_zstr16_presumed(&[0x41])
                    .write_zstr16_presumed(&[0x42, 0x43])
                    .write_u32(2)
                    .write_u8(1)
                    .write_u32(0);
            }),
        );
        s.process(
            2,
            &shell_message(shell_id::GS_PLAYER_LEFT, |w| {
                w.write_u8(2).write_u8(1);
            }),
        );
        let raw: Vec<(u8, usize)> = s
            .take_events()
            .iter()
            .filter_map(|e| match e {
                GameEvent::Shell { shell, bits, .. } => Some((*shell, *bits)),
                _ => None,
            })
            .collect();
        assert_eq!(raw, [(0x68, 2 * 16 + 3 * 16 + 72), (0x6c, 16)]);
    }

    #[test]
    fn match_end_messages_become_events_and_milestones() {
        let mut s = external_session();
        s.obs.roster.insert(
            2,
            RosterRecord {
                pid: 2,
                team: 2,
                ..Default::default()
            },
        );
        s.process(
            1,
            &shell_message(shell_id::GS_KILL, |w| {
                w.write_u8(2)
                    .write_u8(1)
                    .write_u8(0)
                    .write_u16(7)
                    .write_u8(1);
                w.write_bits(0, 4).write_u8(0).write_u8(0).write_bits(0, 3);
                w.write_u32(0).write_u32(0);
            }),
        );
        s.process(
            2,
            &shell_message(shell_id::GS_ROUND_RESULT, |w| {
                w.write_u32(0).write_s8(1).write_u8(1).write_u8(0);
            }),
        );
        s.process(
            3,
            &shell_message(shell_id::GS_MATCH_RESULT_SCREEN, |w| {
                w.write_bit(false).write_u32(0);
            }),
        );
        s.process(
            4,
            &shell_message(shell_id::GS_LEVEL_EXIT, |w| {
                w.write_zstr8(b"").write_bit(false).write_bit(true);
            }),
        );
        let all = s.take_events();
        // 0x46 and 0x3f are also handed over whole (RAW_SHELL_EVENTS), ahead of their own events
        let raw: Vec<u8> = all
            .iter()
            .filter_map(|e| match e {
                GameEvent::Shell { shell, .. } => Some(*shell),
                _ => None,
            })
            .collect();
        assert_eq!(raw, [0x46, 0x3f]);
        let ev: Vec<GameEvent> = all
            .into_iter()
            .filter(|e| !matches!(e, GameEvent::Shell { .. }))
            .collect();
        assert!(
            matches!(ev[0], GameEvent::Kill(k) if k.victim == 2 && k.killer == 1 && k.hit_zone == 1 && k.complete)
        );
        assert_eq!(
            ev[1],
            GameEvent::RoundResult {
                win_team: 1,
                wins: [1, 0]
            }
        );
        assert_eq!(
            ev[2],
            GameEvent::ResultScreen {
                flag: false,
                value: 0
            }
        );
        assert_eq!(
            ev[3],
            GameEvent::LevelExit {
                next_world: String::new(),
                keep: false,
                now: true
            }
        );
        for m in ["round_result", "result_screen", "level_exit"] {
            assert!(s.obs.reached(m), "{m}");
        }
        assert_eq!(s.obs.kills, vec![(2, 1)]);
        // BOT-124: the victim's roster record is dead (a teammate's body lying near the bomb is no defuser)
        assert!(s.obs.roster[&2].dead);
        // the bot (no queue) keeps nothing
        let mut bot = BotSession::new(GameConfig::new("Bot"), None, Recorder::null("Bot"));
        bot.start = Some(0);
        bot.process(
            1,
            &shell_message(shell_id::GS_TEAM_SCORE, |w| {
                w.write_u8(1).write_u8(2);
            }),
        );
        assert!(bot.take_events().is_empty());
    }

    /// A death at the round result arms the 7000 ms wait; the 0x3f result screen 6 s later ends the in-game frame
    /// (`frame_ends_at_result`): no respawn request and no player update after it. Without the flag (the bot) the
    /// armed request still goes out at 7 s.
    #[test]
    fn the_in_game_frame_ends_at_the_result_screen() {
        let run = |gate: bool, revive: bool| {
            let mut s = external_session();
            s.cfg.mode_class_hint = Some(1);
            s.cfg.frame_ends_at_result = gate;
            s.set_player_update(report(0, 1.0, 0.5));
            let state = |v: u8| {
                shell_message(shell_id::GS_PLAYER_STATE, move |w| {
                    w.write_u8(v);
                })
            };
            s.process(10, &state(1));
            for t in (20..1000).step_by(5) {
                s.game_tick(t);
            }
            s.process(
                1000,
                &shell_message(shell_id::GS_ROUND_RESULT, |w| {
                    w.write_u32(0).write_s8(2).write_u8(0).write_u8(1);
                }),
            );
            s.process(1001, &state(2));
            for t in (1005..7000).step_by(5) {
                s.game_tick(t);
            }
            s.process(
                7000,
                &shell_message(shell_id::GS_MATCH_RESULT_SCREEN, |w| {
                    w.write_bit(false).write_u32(2);
                }),
            );
            if revive {
                s.process(7001, &state(1));
            }
            let updates = s.obs.updates_sent;
            for t in (7005..12000).step_by(5) {
                s.game_tick(t);
            }
            let asked = s
                .obs
                .c2s_shell_counts
                .get(&crate::lifecycle::UP_RESPAWN_REQUEST)
                .copied()
                .unwrap_or(0);
            (asked, s.obs.updates_sent - updates)
        };
        assert_eq!(run(true, false).0, 0, "no request after 0x3f");
        assert_eq!(run(false, false).0, 1, "the bot's armed request at 7 s");
        assert_eq!(run(true, true).1, 0, "no update after 0x3f");
        assert!(run(false, true).1 > 0, "the bot keeps updating");
    }
}
