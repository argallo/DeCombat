//! Network play: the client joins a game server (through the lobby, or directly by UDP) and its world, local
//! player, corrections, remote players and status lines come from that server.
//!
//! The protocol work is dc-botclient's, used as a library: the lobby TCP flow ([`dc_botclient::lobby`]), the sans-IO
//! UDP session ([`dc_botclient::session::BotSession`]) with its object and shell readers. This module adds what a
//! playing client needs on top:
//! - a network thread that owns the UDP socket and the session (the 2009 client also keeps acknowledging on a
//!   receive thread while its world loads, UDP_SESSION_2009_A.md §4.6), polled every 2 ms;
//! - the world load done by the client itself (render scene and collision, [`NetWorld`] as a
//!   [`WorldSource`]): the session holds its messages until the client reports the world's box;
//! - the local player's 0x64 update from dc-sim and the camera every frame ([`crate::update`]), sent by the session
//!   under the 2009 gate; the 0xa0 teleports handed back as dc-sim corrections; 0x6a view sets; the 0x67 state;
//! - remote players: the stream positions of every other roster player's object, time-stamped on arrival and
//!   interpolated for drawing;
//! - status lines from 0x66 / 0x67 (health, armour, state), 0x4a / 0xb5 (scores), 0x45 / 0x41 (round timer),
//!   0x6d (kill feed), 0x46 / 0x3f / 0x99 (round end, result screen, level exit);
//! - the life cycle of a 2009 client: after a death (0x67 state 2) the session arms the client's own respawn wait
//!   by the room's mode class (7000 ms in TDM) and then asks with 0x89; leaving a running match sends 0xbd and the
//!   lobby's 0x394 {0} in the same moment and the engine disconnect `C2S_BYE` one frame later;
//! - the movement notices 0x76 [1][e] of the own simulation (jump 1, fall 2, landing 3);
//! - the weapons ([`crate::arms`]): the server's grants into the inventory, select / fire / reload / throw messages,
//!   the fire bits and the spread float of the 0x64, and the held weapon for the first-person view;
//! - the looks of remote characters: their model files (the model block of the character object and of the weapon
//!   object attached to it, names from the session file table) and the stream's animation trackers.
// SPEC: UDP_SESSION_2009_A.md §2-§9 (start-up S1-S12), UDP_SESSION_2009_B.md §3-§4 (objects, stream);
// GAME_MESSAGES_2009_C2S.md §2.1, §2.2, §3.3, §3.4, §5.7; GAME_MESSAGES_2009_S2C.md §2-§4, §6.1; MOVEMENT_2009.md §12.4;
// PROTOCOL_2009.md §2-§3, §7
// OBSERVED: CAPTURE_tdm4_KILL_VALIDATION.md §1, §4 (own respawn wait, 0xbd + 0x394 {0} + C2S_BYE)

use std::collections::{BTreeMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dc_botclient::fx::Effect;
use dc_botclient::hostroom::{self, GoalChoice};
use dc_botclient::lobby::{auth_login, Lobby, LoginOptions};
use dc_botclient::objects::{ExtentBox, SoundNote};
use dc_botclient::record::Recorder;
use dc_botclient::romworld::RomWorlds;
use dc_botclient::run::{describe_datagram, player_info_from_lobby, RoomChoice};
use dc_botclient::session::{BotSession, GameConfig, GameEvent, Stage};
use dc_botclient::shell::{status_kind, KillRecord, PlayerStatus, RosterRecord, NO_KILLER};
use dc_net_engine::impair::PathSpec;
use dc_net_engine::shim::NetShim;
use dc_sim::{Correction, StepEvents, V3};
use dc_wire::udp::{is_oob_datagram, BitWriter, RECV_BUFFER_BYTES};
use dc_wire::PktWriter;

use crate::arms::{self, ArmsConfig, ArmsStats, Arsenal, Carried, Target, WeaponTable};
use crate::error::ClientError;
use crate::hud::state::{AmmoView, HudEvent};
use crate::sound::SoundCue;
use crate::update::LocalUpdate;
use crate::world::{
    ArmsFrame, ArmsResult, FrameState, GpuTarget, LoadedWorld, ModelRef, RemoteBody, RemoteLook,
    ModWorld, RemoteTracker, RomWorld, SpawnPoint, SynthSource, ViewWeapon, WorldSource,
};

/// How long the network thread sleeps when no datagram is waiting before it runs the session's timers again. The
/// socket is non-blocking and the wait is a thread sleep: a socket receive timeout on Windows waits a whole system
/// timer period (about 15.6 ms) instead of 2 ms, which stretched the 67 ms update gap to about 77 ms (13 updates per
/// second instead of 15; measured in the CLI-MOVE-1 loopback test).
// DC-DEFAULT: CLI-60 (proposed) network thread poll: non-blocking socket, 1 ms sleep
pub const POLL_WAIT_MS: u64 = 1;
/// After the client asked to quit, how long the network thread keeps running for the bye exchange.
pub const QUIT_GRACE_MS: u32 = 2500;
/// After the session ended, how long the thread keeps sending the remaining bye copies.
// SPEC: UDP_TRANSPORT_2009.md §9.1 (4 copies, 100 ms apart)
pub const BYE_FLUSH_MS: u32 = 600;
/// Stream samples kept per remote player.
pub const TRACK_KEEP: usize = 32;
/// Interpolation delay of remote players: 1.5 x the measured stream interval, within these bounds.
// DC-DEFAULT: CLI-11 (proposed) interpolation delay 1.5 x the stream interval, 50..250 ms
pub const INTERP_FACTOR: f32 = 1.5;
pub const INTERP_MIN_MS: f32 = 50.0;
pub const INTERP_MAX_MS: f32 = 250.0;
/// Clock-map window of a remote player's stream (samples): the smallest arrival-minus-server offset among them maps
/// the clocks. 256 (about 8.5 s at 30 Hz): a short window's minimum changed every time its fastest sample left,
/// and every change bent one segment's drawn speed (amended 2026-10-31, before 64).
// DC-DEFAULT: CLI-1330 (proposed; amended 2026-10-31) the clock map is the minimum offset of the last 256 samples
pub const LATE_WINDOW: usize = 256;
/// Wait window (samples): the playout delay covers the longest wait among them, a wait being how long after the
/// previous sample's stamp the next sample became available (its server step plus its arrival lateness: a lost or
/// skipped update doubles the step, a Wi-Fi spike adds to it). About 8.5 s at 30 Hz, 25 s at 10 Hz, so a spike or a
/// loss every few seconds stays covered instead of hitching every time it comes back; the margin covers the network
/// thread's poll and the frame. A wait above [`WAIT_SILENCE_MS`] is a silence (death, culling), not jitter: ignored.
// DC-DEFAULT: CLI-1330 (proposed; amended 2026-10-31) delay = max(CLI-11, the longest wait of the last 256 samples
// + 4 ms), within CLI-11's bounds, slewed (rise 10 %, fall 2 % of real time). Before: interval + the 95th percentile
// of the arrival lateness of 64 samples; on a Wi-Fi link to the 30 Hz VPS that left 3..8 % of the frames late
// (30..120 late runs per minute per player; a tenth of the updates never arrived, so the step doubled to 66 ms
// while the rule covered the mean 35 ms interval plus the lateness)
pub const LATE_STAT_WINDOW: usize = 256;
pub const LATE_MARGIN_MS: f32 = 4.0;
pub const WAIT_SILENCE_MS: f32 = 250.0;
/// A stream sample that repeats the last reported position while its velocity says the player moves is a server
/// tick that carried no new client update (a lost update; before CLI-1578 every other tick): the first such
/// sample is placed one step along that velocity, at most this far (s) from the last reported position; further
/// repeats hold there. A player pressed against a wall reports the same position with its wish velocity every
/// update, and placing each repeat from the previous placed one walked its body through the wall until the next
/// reported position snapped it back (seen 2026-10-31); one bounded step cannot.
// DC-DEFAULT: CLI-1577 (proposed; amended 2026-10-31) a repeated stream position of a moving player is placed one
// step along its velocity, at most 50 ms from the last reported position, once (the 2009 client moved streamed
// objects by their velocity between entries, B §4.3)
pub const RECKON_MAX_S: f32 = 0.05;
pub const DELAY_RISE: f32 = 0.10;
pub const DELAY_FALL: f32 = 0.02;
/// How far past its newest sample a running remote player is carried along its stream velocity.
// DC-DEFAULT: CLI-1331 (proposed) a late sample: extrapolate the newest stream velocity for at most 100 ms, then hold
pub const EXTRAP_MAX_MS: f32 = 100.0;
/// After the extrapolation bound the body eases back to the newest sample over this long (ms).
// DC-DEFAULT: CLI-1331 (proposed) the held body returns to the newest sample instead of staying parked ahead of it
pub const EXTRAP_EASE_MS: f32 = 100.0;
/// Server times closer than this (ms) to the newest one are not a usable step (f32 seconds of a long-running
/// server resolve 8 ms after a day); the sample is dropped without counting as stale.
pub const MIN_TIME_STEP_MS: f64 = 2.0;
/// The clock map follows a change of the window minimum, down or up, by at most this fraction of the arrival time
/// passed: a drop never crowds stamps together, and a rise (amended 2026-10-31; before it was taken at once) never
/// stretches one segment by the whole rise (one slow segment, then normal: a hitch). While the map lags a real
/// latency rise the stamps run early, the waits grow and the delay covers it.
// DC-DEFAULT: CLI-1330 (proposed; amended 2026-10-31) a change of the clock map is absorbed over ~10x its size
pub const BASE_SLEW: f64 = 0.1;
/// After this long on the arrival clock the track tries the server clock again (ms).
pub const SERVER_RETRY_MS: u32 = 3_000;
/// A stream velocity above this (world units / s) is not believed for extrapolation.
pub const EXTRAP_MAX_SPEED: f32 = 2000.0;
/// Cadence of the `remote_interp` lines of the event log.
pub const INTERP_REPORT_MS: u32 = 10_000;
/// Kill-feed lines kept for the overlay.
// DC-DEFAULT: CLI-12 (proposed) the last 4 kill-feed lines
pub const KILL_FEED_LINES: usize = 4;
/// Shell id of the server's music order (`GS_MUSIC`).
// SPEC: GAME_MESSAGES_2009_S2C.md §4.2 row 0xd9
pub const GS_MUSIC: u8 = 0xd9;
/// Shell id of the server's overlay switch (`GS_OVERLAY`).
// SPEC: GAME_MESSAGES_2009_S2C.md 0x77 row
pub const GS_OVERLAY: u8 = 0x77;
/// Box colours of remote players by team (team 1, team 2, no team), RGBA 0..1.
// DC-DEFAULT: CLI-13 (proposed) placeholder box colours: team 1 red, team 2 blue, no team grey
pub const TEAM_COLOURS: [[f32; 4]; 3] = [
    [0.85, 0.25, 0.20, 1.0],
    [0.20, 0.45, 0.90, 1.0],
    [0.75, 0.75, 0.75, 1.0],
];
/// Test hook ([`NetOptions::glitch_after_updates`]): the reported position is shifted this far along +X for this
/// long, which a validating server answers with a teleport back.
pub const GLITCH_OFFSET: f32 = 5000.0;
pub const GLITCH_MS: u32 = 200;
/// Leaving a running match: `C2S_BYE` goes out this long after `UP_LEAVE_MATCH` 0xbd (the 2009 client sent it in
/// the next reliable block, one frame later).
// OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.16 (0xbd at 297.218, C2S_BYE 16 ms later in the next block)
pub const LEAVE_BYE_DELAY_MS: u32 = 16;
/// The lobby frame the 2009 client sends with every match leave: 0x394 `[phase:u32 = 0]`.
// SPEC: PROTOCOL_2009.md §3.7 (0x394 +6 u32 phase); OBSERVED: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §4.4 (sent with
// +6 = 0 in the same millisecond as 0xbd on every leave, 2 of 2), CAPTURE_tdm4_KILL_VALIDATION.md §1 (137.031)
pub const LEAVE_NOTICE_PHASE: u32 = 0;
/// The lobby location the 2009 client reports when it leaves the result screen: 0x306 `[where:u32 = 0xb]`.
// SPEC: PROTOCOL_2009.md §3 row 0x306 (where 0xb = result)
// OBSERVED: capture 20261025_125432_rust6 (804.891) and 20261025_153705_rust7 (720.016): the
// real client sent `06 03 0a 00 00 00 0b 00 00 00` in the same millisecond as its first plain `c9 03`
pub const RESULT_LOCATION: u32 = 0x0b;
/// How long the client stays on the result screen after 0x3f before it leaves (lobby 0x306 {0xb} and the plain
/// transport bye, no game message, no `C2S_BYE`).
// OBSERVED: rust6 7657 ms, rust7 8172 ms from 0x3f to the real client's leave (no server bye in either); the 2009
// rule (a timer or the player's click) is not traced
// OBSERVED: capture 20261025_204341_realclient (9266 ms: 0x3f at 674.797, 0x306 {0xb} at
// 684.047, nobody touched the screen); the leave is sent from the result screen's focus (DISASM cshell+0x137dd0), so
// the screen takes the focus that long after 0x3f (the cause of the wait is not traced, see matchexit.rs)
// DC-DEFAULT: CLI-88 (proposed) 8000 ms on the result screen, `NetOptions::result_hold`
// DISASM: cshell+0x4ac5c (0x3f opens screen 7 at once: cshell+0x1bb140(7), interface state 5, whose frame draws the
// current screen instead of the in-game view, cshell+0x1bc590): the 2009 player saw the result page from 0x3f on.
// Our menu result page opens when the session hands the lobby link back, after this hold, so the hold is what the
// player waits on top of the 2009 timing (playtest report: "the end of game delay before post game scoreboard is
// slightly longer than it should be"; 0x46 -> board 3000 ms and 0x46 -> 0x3f 6000 ms already match 2009 / the reference stack)
// DC-DEFAULT: CLI-1124 (proposed; amends CLI-88) 1000 ms: the result page about one second after 0x3f; the leave
// (0x306 {0xb} + plain bye) goes out that much earlier than the 2009 client's 7.7..9.3 s
pub const RESULT_HOLD_MS: u32 = 1000;
/// Shell id and category of the movement notices.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.2 (`UP_MOVE_EVENT` 0x76 `[category:u8 = 1][event:u8]`, R)
pub const UP_MOVE_EVENT: u8 = 0x76;
pub const MOVE_EVENT_CATEGORY: u8 = 1;
/// `UP_SPRINT_STATE` 0xca (the sprint bit of the control word is dc-sim's `player::bits::SPRINT`).
// SPEC: GAME_MESSAGES_2009_C2S.md §2.3; MOVEMENT_2009.md §5.1 (SPRINT 0x400000)
pub const UP_SPRINT_STATE: u8 = 0xca;
/// The controller state byte of the reliable 0x64 state form the 2009 client sent once, right before 0x87.
// OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.2 (a) (`0b 64 10 00 01`, the state byte observed is 1)
pub const STATE_FORM_VALUE: u8 = 1;

/// How to reach the game server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connect {
    /// Through the lobby: auth address (the lobby address comes from its server list), the room to sit in.
    Lobby {
        auth: SocketAddr,
        room: RoomChoice,
        password: String,
        channel: Option<u32>,
    },
    /// Straight to a game server's UDP port (the server must expect the name, DSERVER_PIPE.md §3.1).
    Udp(SocketAddr),
}

/// A world file to load instead of the ROM world the server announces (a server started with a world file).
#[derive(Debug, Clone, PartialEq)]
pub struct WorldFile {
    pub bytes: Vec<u8>,
    pub stand_dims: V3,
    /// Named solid colours for its textures (unnamed ones draw with the renderer's fallback).
    pub textures: Vec<(Vec<u8>, [u8; 4])>,
}

impl WorldFile {
    /// A world file from disk with a made-up stand box (the server's teleport box replaces it).
    // DC-DEFAULT: CLI-9 (proposed) 16 x 40 x 16 half extents until the first teleport, for worlds not from the ROM
    pub fn from_path(path: &Path) -> Result<WorldFile, ClientError> {
        let bytes =
            std::fs::read(path).map_err(|e| ClientError::Io(format!("{}: {e}", path.display())))?;
        Ok(WorldFile {
            bytes,
            stand_dims: V3::new(16.0, 40.0, 16.0),
            textures: Vec::new(),
        })
    }
}

/// Options of a network session.
#[derive(Debug, Clone)]
pub struct NetOptions {
    pub connect: Connect,
    /// Nickname (lobby: the character name created for a new account; direct: the hello name).
    pub name: String,
    /// Login id (`LOCAL:<id>`); defaults to the lower-case name.
    pub user: Option<String>,
    /// ROM folder for the announced world (default `CA_ROM`).
    pub rom: Option<PathBuf>,
    pub world_file: Option<WorldFile>,
    /// JSONL event log and captap capture of the session (local files, never committed).
    pub log: Option<PathBuf>,
    pub capture: Option<PathBuf>,
    pub lobby_timeout: Duration,
    /// Room fill (host) and match address wait.
    pub match_wait: Duration,
    /// Wait for the server's world announcement.
    pub load_timeout: Duration,
    pub update_hz: f32,
    /// Test hook: after this many sent updates the reported position is shifted by [`GLITCH_OFFSET`] for
    /// [`GLITCH_MS`] (provokes a server correction). Off normally.
    pub glitch_after_updates: Option<u64>,
    /// Host only: the kill goal set right after the room is created (0x38c type 8); `Min` = the ROM mode row's
    /// smallest goal.
    pub goal: Option<GoalChoice>,
    /// Leave the running match this long after entering the world (0xbd + 0x394 {0}, then `C2S_BYE`), as a
    /// player who confirms the leave box would. `None` = stay until the server ends the round or the caller quits.
    pub leave_after: Option<Duration>,
    /// Send the movement notices 0x76 [1][e] of the own simulation.
    pub move_notices: bool,
    /// Weapon slots to fall back on without grants (a direct join has no lobby; the lobby flow uses 0x323).
    pub lobby_slots: Vec<i32>,
    /// The room's weapon limit (the waiting room's weapon list row; 0 = all permitted, a direct join knows none).
    pub arms_limit: u32,
    /// LG-274: the room's Hacks setting (`dc_wire::roomhacks` bits; 0 = none, a direct join knows none): with
    /// no-recoil the own shots kick no recoil and have no spread (CLI-1627).
    pub room_hacks: u32,
    /// Seed of the weapon's random numbers (spread, the 0x82 seed byte).
    pub arms_seed: u64,
    /// How long the client stays on the result screen (0x3f) before it leaves ([`RESULT_HOLD_MS`]).
    pub result_hold: Duration,
    /// A lobby connection handed over by the menu screens ([`crate::menu::handoff`]): used instead of the lobby
    /// flow of `connect`, and given back when the session ends.
    pub handoff: Option<crate::menu::handoff::LobbyHandoff>,
    /// The 2009 "Reality" option: hits jar the camera ([`crate::camfx`]) and light the HUD's damage arcs (always on,
    /// CLI-1847; the hidden `--no-reality` turns it off for the headless tests that compare against 2009 captures).
    pub reality: bool,
    /// A drawn round plays a draw voice (`--draw-voice`; the 2009 client plays none).
    // DC-DEFAULT: HUD-SND-2 (staged) off
    pub draw_voice: bool,
    /// Load the shot test's character rigs on threads of their own (until loaded a character is hit by its box with
    /// node 0xff); false loads them in the frame that first needs them ([`crate::hitnodes::HitRigs`]).
    // DC-DEFAULT: CLI-304 (proposed) on, `DC_ASYNC_LOADS=0` off
    pub hit_rigs_async: bool,
    /// CLI-1611: the weapon content of the server's mod set (the menu's mod step, `JoinReport::content`); the
    /// match's weapon table gets it. `None` (vanilla, a direct join) applies nothing.
    pub content: Option<Arc<dc_bute::modpatch::WeaponContent>>,
    /// MAP-31: the server's mod set as a recipe (`JoinReport::recipe`, or `--mod-dir` on a direct join): a `@mod:`
    /// world announce resolves through its folders when no mod layer is installed. `None`: vanilla.
    pub mods: Option<crate::mods::HostRecipe>,
}

impl NetOptions {
    pub fn new(connect: Connect, name: &str) -> NetOptions {
        NetOptions {
            connect,
            name: name.to_string(),
            user: None,
            rom: None,
            world_file: None,
            log: None,
            capture: None,
            lobby_timeout: Duration::from_secs(10),
            match_wait: Duration::from_secs(60),
            load_timeout: Duration::from_secs(90),
            update_hz: crate::update::SEND_RATE_HZ,
            glitch_after_updates: None,
            goal: None,
            leave_after: None,
            move_notices: true,
            lobby_slots: Vec::new(),
            arms_limit: 0,
            room_hacks: 0,
            arms_seed: 0x00dc_a7a5_5eed,
            result_hold: Duration::from_millis(RESULT_HOLD_MS as u64),
            handoff: None,
            reality: true,
            draw_voice: false,
            hit_rigs_async: crate::characters::async_loads_from_env(true),
            content: None,
            mods: None,
        }
    }

    fn user(&self) -> String {
        self.user
            .clone()
            .unwrap_or_else(|| self.name.to_ascii_lowercase())
    }
}

/// Where the network session is.
#[derive(Debug, Clone, PartialEq)]
pub enum NetPhase {
    Starting,
    /// A lobby step (auth, lobby login, room, waiting for players, waiting for the game server).
    Lobby(String),
    Joining,
    /// The server announced this world; the client loads it.
    Loading(String),
    /// World loaded, waiting for the in-world test (player state).
    Waiting,
    InWorld,
    Ended(String),
    Failed(String),
}

impl NetPhase {
    pub fn label(&self) -> String {
        match self {
            NetPhase::Starting => "starting".into(),
            NetPhase::Lobby(s) => format!("lobby: {s}"),
            NetPhase::Joining => "joining".into(),
            NetPhase::Loading(w) => format!("loading {w}"),
            NetPhase::Waiting => "waiting".into(),
            NetPhase::InWorld => "in world".into(),
            NetPhase::Ended(s) => format!("ended ({s})"),
            NetPhase::Failed(s) => format!("failed ({s})"),
        }
    }

    pub fn is_final(&self) -> bool {
        matches!(self, NetPhase::Ended(_) | NetPhase::Failed(_))
    }
}

/// What the drawing of one remote player went through (CLI-1330 / CLI-1331), since the track began (`total`) and
/// since the last event-log report (`window`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PlayoutStats {
    /// Frames drawn.
    pub frames: u64,
    /// Frames drawn past the newest sample of a moving player, carried along its velocity (CLI-1331).
    pub late_frames: u64,
    /// Frames held past the extrapolation bound (the body stands although the player runs).
    pub held_frames: u64,
    /// Runs of late / held frames (one late arrival = one run).
    pub late_runs: u64,
    pub held_runs: u64,
    /// Sum of the playout delay over the frames (ms).
    pub delay_sum_ms: f64,
    /// Stream samples taken / dropped as older than the newest (reordered).
    pub samples: u64,
    pub stale: u64,
    /// Drawn-motion smoothness (CLI-1331 amended 2026-10-31): frames whose drawn speed is above
    /// [`MOVING_SPEED`], the sum and the sum of squares of that speed (u/s), frames whose drawn step reversed
    /// the previous one (a rubber band), and frames whose step was over [`JUMP_FACTOR`] x the mean step (a jump).
    pub moving_frames: u64,
    pub speed_sum: f64,
    pub speed_sq: f64,
    pub backsteps: u64,
    pub jumps: u64,
    /// Stream samples whose position repeated the previous sample's although the stream said the player moves (a
    /// server tick that carried no new client update), and how many of them were placed along the velocity.
    pub repeats: u64,
    pub reckoned: u64,
}

/// Drawn speed (u/s) from which a frame counts as moving in the smoothness statistics.
pub const MOVING_SPEED: f32 = 30.0;
/// A drawn step this many times the window's mean moving step counts as a jump.
pub const JUMP_FACTOR: f32 = 3.0;

/// Stream samples of one remote player. With the stream's server time (B §9.1) a sample is stamped on the server
/// clock mapped to the local one by the fastest arrival of the window (so arrival jitter does not bend the drawn
/// speed); without it, on arrival.
#[derive(Debug, Clone, Default)]
pub struct Track {
    pub pid: u8,
    pub name: String,
    pub team: u8,
    /// (playout time on the local clock in ms, position)
    samples: VecDeque<(f64, [f32; 3])>,
    /// Stream velocity of the newest sample (world units / s).
    vel: Option<[f32; 3]>,
    /// Server time (ms) of the newest sample.
    last_server_ms: Option<f64>,
    /// Arrival minus server time (ms) of the last [`LATE_WINDOW`] samples; their minimum maps the clocks.
    offs: VecDeque<f64>,
    /// The waits of the last [`LATE_STAT_WINDOW`] samples (ms): arrival minus the previous sample's stamp (the server
    /// step plus the arrival lateness); the longest sizes the delay.
    waits: VecDeque<f32>,
    /// Consecutive stream entries whose server time did not advance; past 8 the track stamps on arrival.
    stale_run: u32,
    arrival_clock: bool,
    /// When the track fell back to the arrival clock (ms); the server clock is tried again after [`SERVER_RETRY_MS`].
    fallback_at: u32,
    /// The clock map (local minus server, ms): the window minimum, followed at a bounded rate either way.
    base: Option<f64>,
    /// Arrival time of the newest sample (ms).
    last_arrival: Option<u32>,
    /// The playout delay rule's value at the newest sample and the slewed delay at that time.
    delay_target: f32,
    delay_val: f32,
    delay_at: u32,
    /// The longest wait of the window beyond the mean stream interval (ms): the lateness the delay covers.
    pub late_q: f32,
    in_late: bool,
    in_held: bool,
    /// Trailing samples of `samples` that were placed along the velocity (CLI-1577); the next reported position
    /// puts them on the straight line to it when they are not drawn yet.
    reckoned_tail: usize,
    /// The previous frame's draw (time ms, position) and step, for the smoothness statistics.
    prev_draw: Option<(u32, [f32; 3])>,
    prev_step: Option<[f32; 3]>,
    pub total: PlayoutStats,
    pub window: PlayoutStats,
    /// The latest stream yaw (radians).
    pub yaw: Option<f32>,
    /// The upper-body aim from the server's aim notes ([`crate::aim`]) and when it was last advanced (ms).
    pub aim: Option<crate::aim::AimState>,
    aim_at: u32,
    entries_seen: u32,
    /// `CF_POS_SNAP` placements of the object seen so far.
    snaps_seen: u32,
    /// Animation blocks seen and when the latest one arrived (ms).
    anim_seen: u32,
    anim_at: u32,
    /// Samples taken so far.
    pub sample_count: u64,
    /// Distance covered between consecutive samples.
    pub travel: f32,
    /// Mean interval between samples (exponential average, ms).
    pub gap_ms: f32,
    pub first: Option<[f32; 3]>,
    pub last: Option<[f32; 3]>,
    /// The player left (0x6c) or its object went away: the track is no longer drawn and only kept for the record.
    pub left: bool,
}

/// The remote characters this client saw die and not come back yet: their bodies are out of the shot trace.
/// A character is dead from the kill message `GS_KILL` 0x6d that names its player as the victim until an animation
/// block that arrived after that message shows its main tracker looping again (the corpse's death clip does not
/// loop; a living character's main clip does), or until its player leaves.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 (0x6d victim = the roster id); UDP_SESSION_2009_B.md §4.3 (main tracker, its
// state bits)
// OBSERVED: dc-dserver relay.rs `dead_main_tracker` (the reference stack's corpse: the death clip on the main tracker with state
// bits 1 / 0 until the next spawn, REFERENCE tdm4 / tdm6); capture 20261025_171754_rustclient (the
// user's first dc-client round: 7 of its 17 hit claims named a dead character's standing box, refused victim_dead)
// DC-DEFAULT: CLI-99 (proposed) no hit volume for a dead remote character (the 2009 client traced the posed model, so a
// fallen corpse no longer stood in the way of a chest-high shot; this client's hit volume is a standing box)
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Corpses {
    /// Character object -> when its death was seen (ms of the shared clock).
    dead: BTreeMap<u16, u32>,
    /// Characters that came back so far (for reports and tests).
    pub revived: u64,
}

impl Corpses {
    /// A kill message named the player of this character as the victim.
    pub fn killed(&mut self, obj: u16, now: u32) {
        self.dead.insert(obj, now);
    }

    /// An animation block of `obj` arrived at `at` (ms) with its main tracker looping (`Some(true)`), not looping
    /// (`Some(false)`) or absent (`None`).
    pub fn animated(&mut self, obj: u16, at: u32, main_looping: Option<bool>) {
        if let Some(&died) = self.dead.get(&obj) {
            if at > died && main_looping == Some(true) {
                self.dead.remove(&obj);
                self.revived += 1;
            }
        }
    }

    /// The character's player left.
    pub fn forget(&mut self, obj: u16) {
        self.dead.remove(&obj);
    }

    pub fn is_dead(&self, obj: u16) -> bool {
        self.dead.contains_key(&obj)
    }

    pub fn count(&self) -> usize {
        self.dead.len()
    }
}

/// The item table's `ItemType` of every weapon row, for the room's weapon limit (empty without a limit or a ROM).
// SPEC: PROTOCOL_2009.md §3.3 (0x323 item numbers below 1000 are the weapon numbers)
fn limit_types(
    limit: u32,
    table: Option<&WeaponTable>,
    rom: Option<&std::path::Path>,
) -> Arc<BTreeMap<u16, i32>> {
    let (Some(t), Some(_), true) = (table, rom, limit != 0) else {
        return Arc::default();
    };
    let wares = crate::menu::storage::Wares::cached(rom);
    Arc::new(
        t.rows
            .keys()
            .filter_map(|&w| Some((w, wares.kind_of(i32::from(w))?)))
            .collect(),
    )
}

/// Moves every track whose object is not in `live` out of the drawn set into `left` (marked [`Track::left`]), so a
/// player who leaves before the own session ends (the 2009 client leaves on its own after 0x3f and the server tells
/// the others with 0x6c) stays in the session's record.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2 (0x6c); GAME_MESSAGES_2009_C2S.md Appendix A step A14
// DC-DEFAULT: DSV-35 (no server bye at the match end: the clients leave one by one), see DC_DEFAULTS.md
fn retire_tracks(tracks: &mut BTreeMap<u16, Track>, left: &mut BTreeMap<u16, Track>, live: &[u16]) {
    let gone: Vec<u16> = tracks
        .keys()
        .copied()
        .filter(|k| !live.contains(k))
        .collect();
    for k in gone {
        if let Some(mut t) = tracks.remove(&k) {
            t.left = true;
            left.insert(k, t);
        }
    }
}

impl Track {
    /// Takes the object's news at `now` ms: a snap placement (`CF_POS_SNAP`: spawn, respawn, the visibility park /
    /// show) restarts the track at the new position, so no drawn frame shows the body at, or sliding from, where it
    /// was before (its corpse or the park point); stream positions are added as samples.
    // SPEC: UDP_SESSION_2009_B.md §4.1 row 0x0200 (CF_POS_SNAP, PLAUSIBLY "place without smoothing")
    // DC-DEFAULT: CLI-620 (proposed) a snap placement restarts the remote body's interpolation at that point
    pub fn follow_object(&mut self, o: &dc_botclient::objects::ClientObject, now: u32) {
        self.follow_object_at(o, now, None)
    }

    /// [`Track::follow_object`] with the session's latest server time (seconds, the time entry of the stream message
    /// the object's entries came in; B §9.1).
    // SPEC: UDP_SESSION_2009_B.md §9.1 (time entry last in every stream message), §4.3 (position + velocity)
    pub fn follow_object_at(
        &mut self,
        o: &dc_botclient::objects::ClientObject,
        now: u32,
        server_s: Option<f32>,
    ) {
        if o.pos_snaps != self.snaps_seen {
            self.snaps_seen = o.pos_snaps;
            if let Some(p) = o.pos {
                let at = match (server_s, self.clock_base()) {
                    (Some(s), Some(b)) if !self.arrival_clock => {
                        (s as f64 * 1000.0 + b).min(now as f64)
                    }
                    _ => now as f64,
                };
                self.snap(at, p);
            }
        }
        if self.sample_count == 0 || o.stream_entries != self.entries_seen {
            self.entries_seen = o.stream_entries;
            if let Some(p) = o.pos {
                // the time of the message that carried this entry (a stall can absorb several messages at once)
                self.push_stream(now, p, o.vel, o.stream_time.or(server_s));
            }
            if let Some(y) = o.yaw8 {
                self.yaw = Some(dc_wire::udp::yaw8_to_radians(y));
            }
        }
    }

    /// Restarts the samples at `p` (taken at `t` ms): the body is drawn there at once and nothing is carried across
    /// (VIS-13: no extrapolation from before the placement).
    pub fn snap(&mut self, t: f64, p: [f32; 3]) {
        if !p.iter().all(|c| c.is_finite()) {
            return;
        }
        self.samples.clear();
        self.samples.push_back((t, p));
        self.vel = None;
        self.waits.clear();
        self.reckoned_tail = 0;
        self.first.get_or_insert(p);
        self.last = Some(p);
        self.sample_count += 1;
    }

    /// The local-minus-server clock offset (ms): the fastest arrival of the window.
    fn clock_base(&self) -> Option<f64> {
        self.base
    }

    /// Adds a stream sample that arrived at `now` ms: position, stream velocity and the server time of its message.
    /// A sample whose server time is not later than the newest one's is an older message that arrived late: dropped.
    pub fn push_stream(
        &mut self,
        now: u32,
        p: [f32; 3],
        vel: Option<[f32; 3]>,
        server_s: Option<f32>,
    ) {
        if !p.iter().all(|c| c.is_finite()) {
            return;
        }
        if self.last_arrival.is_some_and(|a| now < a) {
            // samples are taken in arrival order; an earlier time is a clock error, not data
            return;
        }
        let server_ms = server_s
            .filter(|s| s.is_finite())
            .map(|s| s as f64 * 1000.0);
        if self.arrival_clock
            && now.wrapping_sub(self.fallback_at) >= SERVER_RETRY_MS
            && server_ms.is_some_and(|sm| {
                self.last_server_ms
                    .is_none_or(|l| sm - l >= MIN_TIME_STEP_MS)
            })
        {
            // the server time advances again: map the clocks anew from here
            self.arrival_clock = false;
            self.stale_run = 0;
            self.offs.clear();
            self.waits.clear();
            self.base = None;
        }
        let stamp = match server_ms {
            Some(sm) if !self.arrival_clock => {
                if let Some(step) = self.last_server_ms.map(|l| sm - l) {
                    if step < MIN_TIME_STEP_MS {
                        // older than the newest: a late message (stale); equal or barely later: no usable step
                        if step < 0.0 {
                            self.total.stale += 1;
                            self.window.stale += 1;
                        }
                        self.stale_run += 1;
                        if self.stale_run >= 8 {
                            // the server clock stands still while entries come in: stamp on arrival for a while
                            self.arrival_clock = true;
                            self.fallback_at = now;
                            self.offs.clear();
                            self.waits.clear();
                            self.base = None;
                        }
                        return;
                    }
                }
                self.stale_run = 0;
                if self.last_server_ms.is_some_and(|l| sm - l > 5_000.0) {
                    // a long silence (death, culling): the old offsets say nothing about the link now
                    self.offs.clear();
                    self.waits.clear();
                    self.base = None;
                }
                self.last_server_ms = Some(sm);
                self.offs.push_back(now as f64 - sm);
                while self.offs.len() > LATE_WINDOW {
                    self.offs.pop_front();
                }
                let target = self
                    .offs
                    .iter()
                    .copied()
                    .reduce(f64::min)
                    .unwrap_or(now as f64 - sm);
                let passed = self.last_arrival.map_or(0.0, |a| now.saturating_sub(a) as f64);
                let base = match self.base {
                    Some(b) if target < b => (b - BASE_SLEW * passed).max(target),
                    Some(b) if target > b => (b + BASE_SLEW * passed).min(target),
                    _ => target,
                };
                self.base = Some(base);
                sm + base
            }
            _ => now as f64,
        };
        // the wait for this sample: since the previous sample's stamp (its server step plus its lateness)
        if let Some(&(t0, _)) = self.samples.back() {
            let wait = (now as f64 - t0) as f32;
            if (0.0..=WAIT_SILENCE_MS).contains(&wait) {
                self.waits.push_back(wait);
                while self.waits.len() > LATE_STAT_WINDOW {
                    self.waits.pop_front();
                }
            }
        }
        self.last_arrival = Some(now);
        // samples keep their order: a clock-map shift never stamps a sample before the one it follows
        let stamp = match self.samples.back() {
            Some(&(t0, _)) if stamp <= t0 => t0 + 0.5,
            _ => stamp,
        };
        // CLI-1577: a repeated position of a moving player is the server's tick without news: one step along the
        // velocity (bounded), not a stop; the next reported position straightens the placed samples that are not
        // drawn yet (no overshoot at a stop or a turn)
        let mut p = p;
        let mut reckoned = false;
        if let Some(&(t0, q)) = self.samples.back() {
            let moving = vel.is_some_and(|v| v.iter().any(|c| c.abs() > 1e-3));
            let last_reported = self
                .samples
                .len()
                .checked_sub(self.reckoned_tail + 1)
                .and_then(|i| self.samples.get(i).copied());
            if moving && last_reported.is_some_and(|(_, lp)| lp == p) {
                self.total.repeats += 1;
                self.window.repeats += 1;
                reckoned = true;
                if self.reckoned_tail == 0 {
                    // the first repeat: one bounded step from the reported position
                    let step_s = (((stamp - t0) / 1000.0) as f32).clamp(0.0, RECKON_MAX_S);
                    if let Some(v) = vel.filter(|v| v.iter().all(|c| c.is_finite())) {
                        if step_s > 0.0 {
                            p = [q[0] + v[0] * step_s, q[1] + v[1] * step_s, q[2] + v[2] * step_s];
                            self.total.reckoned += 1;
                            self.window.reckoned += 1;
                        }
                    }
                } else {
                    // further repeats (a stuck or a standing-with-wish-velocity player): hold the placed point
                    p = q;
                }
            } else if self.reckoned_tail > 0 {
                self.straighten_reckoned(now, stamp, p);
            }
        }
        self.add_sample(stamp, p);
        self.reckoned_tail = if reckoned { self.reckoned_tail + 1 } else { 0 };
        self.vel = vel.filter(|v| {
            v.iter().all(|c| c.is_finite())
                && v[0] * v[0] + v[1] * v[1] + v[2] * v[2] <= EXTRAP_MAX_SPEED * EXTRAP_MAX_SPEED
        });
        // until the interval is measured (two samples) the rule's value is taken as it is
        let first = self.total.samples < 2;
        self.total.samples += 1;
        self.window.samples += 1;
        self.late_q = self.late_quantile();
        self.delay_val = if first {
            self.delay_ms()
        } else {
            self.playout_delay(now)
        };
        self.delay_at = now;
        self.delay_target = self.delay_ms();
    }

    /// The longest wait among the last [`LATE_STAT_WINDOW`] samples beyond the mean stream interval (ms).
    fn late_quantile(&self) -> f32 {
        let longest = self.waits.iter().copied().fold(0.0, f32::max);
        if longest <= 0.0 {
            return 0.0;
        }
        let i = if self.gap_ms > 0.0 { self.gap_ms } else { 100.0 };
        (longest - i).max(0.0)
    }

    /// Adds a sample taken at `t` ms, stamped on arrival (no stream velocity, no server time).
    pub fn push(&mut self, t: u32, p: [f32; 3]) {
        self.push_stream(t, p, None, None)
    }

    fn add_sample(&mut self, t: f64, p: [f32; 3]) {
        if let Some(&(t0, q)) = self.samples.back() {
            let dt = (t - t0) as f32;
            // a step under 1 ms is the order clamp of a snap and its first stream sample, not an interval; a step
            // above the silence bound (death, culling) is a pause, not an interval either (CLI-1330 amended)
            if (1.0..=WAIT_SILENCE_MS).contains(&dt) {
                self.gap_ms = if self.gap_ms <= 0.0 {
                    dt
                } else {
                    0.9 * self.gap_ms + 0.1 * dt
                };
            }
            self.travel +=
                ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt();
        }
        self.first.get_or_insert(p);
        self.last = Some(p);
        self.sample_count += 1;
        self.samples.push_back((t, p));
        while self.samples.len() > TRACK_KEEP {
            self.samples.pop_front();
        }
    }

    /// The playout delay rule at the newest sample: the longest of 1.5 x the stream interval (CLI-11), two intervals
    /// plus the margin (one lost update never runs the buffer dry, the Source engine's interp ratio of 2) and the
    /// longest wait of the window plus the margin (CLI-1330: the interval plus `late_q`), within 50..250 ms.
    // DC-DEFAULT: CLI-1330 (proposed; amended 2026-10-31) floor of two stream intervals + 4 ms (70 ms at 30 Hz)
    pub fn delay_ms(&self) -> f32 {
        let i = if self.gap_ms > 0.0 {
            self.gap_ms
        } else {
            100.0
        };
        (INTERP_FACTOR * i)
            .max(2.0 * i + LATE_MARGIN_MS)
            .max(i + self.late_q + LATE_MARGIN_MS)
            .clamp(INTERP_MIN_MS, INTERP_MAX_MS)
    }

    /// The delay the body is drawn with at `now`: it follows the rule at a bounded rate (rising at most 10 %, falling
    /// at most 2 % of the time passed), so a change of the delay never makes the body jump.
    pub fn playout_delay(&self, now: u32) -> f32 {
        if self.total.samples == 0 {
            return self.delay_ms();
        }
        let dt = now.saturating_sub(self.delay_at) as f32;
        let (v, t) = (self.delay_val, self.delay_target);
        if t > v {
            (v + DELAY_RISE * dt).min(t)
        } else {
            (v - DELAY_FALL * dt).max(t)
        }
    }

    /// Position at time `t` ms: linear between the two samples around it, held at the first / last sample outside.
    pub fn at(&self, t: f64) -> Option<[f32; 3]> {
        let (&(t0, p0), &(t1, p1)) = (self.samples.front()?, self.samples.back()?);
        if t <= t0 {
            return Some(p0);
        }
        if t >= t1 {
            return Some(p1);
        }
        let mut prev = (t0, p0);
        for &(tn, pn) in self.samples.iter().skip(1) {
            if t <= tn {
                let span = (tn - prev.0).max(1e-3);
                let a = (((t - prev.0) / span) as f32).clamp(0.0, 1.0);
                return Some([
                    prev.1[0] + (pn[0] - prev.1[0]) * a,
                    prev.1[1] + (pn[1] - prev.1[1]) * a,
                    prev.1[2] + (pn[2] - prev.1[2]) * a,
                ]);
            }
            prev = (tn, pn);
        }
        Some(p1)
    }

    /// The position drawn at `now`: [`Track::at`] the playout delay behind; past the newest sample a moving player is
    /// carried along its stream velocity for at most [`EXTRAP_MAX_MS`], then held (counted in the stats).
    // DC-DEFAULT: CLI-1331 (proposed) bounded extrapolation; a stream velocity of 0 (DSV-21) or a snap (VIS-13)
    // stops it
    pub fn draw(&mut self, now: u32) -> Option<[f32; 3]> {
        let d = self.playout_delay(now);
        let r = now as f64 - d as f64;
        let &(t1, p1) = self.samples.back()?;
        let mut late = false;
        let mut held = false;
        let pos = match self.vel {
            Some(v) if r > t1 && v.iter().any(|c| c.abs() > 1e-3) => {
                let over = (r - t1) as f32;
                late = true;
                held = over > EXTRAP_MAX_MS;
                let s = over.min(EXTRAP_MAX_MS) / 1000.0;
                let e = [p1[0] + v[0] * s, p1[1] + v[1] * s, p1[2] + v[2] * s];
                if held {
                    // past the bound the body eases back to the newest sample, not parked ahead of it
                    let k = ((over - EXTRAP_MAX_MS) / EXTRAP_EASE_MS).min(1.0);
                    [
                        e[0] + (p1[0] - e[0]) * k,
                        e[1] + (p1[1] - e[1]) * k,
                        e[2] + (p1[2] - e[2]) * k,
                    ]
                } else {
                    e
                }
            }
            _ => self.at(r)?,
        };
        for st in [&mut self.total, &mut self.window] {
            st.frames += 1;
            st.delay_sum_ms += d as f64;
            st.late_frames += late as u64;
            st.held_frames += held as u64;
            st.late_runs += (late && !self.in_late) as u64;
            st.held_runs += (held && !self.in_held) as u64;
        }
        self.in_late = late;
        self.in_held = held;
        self.note_draw(now, pos);
        Some(pos)
    }

    /// A reported position after placed samples (CLI-1577): every placed sample whose segment is not drawn yet
    /// (the sample before it is still ahead of the playout point) moves onto the straight line from the last
    /// reported sample to the new one, so the drawn path does not overshoot a stop or a turn; a placed sample
    /// being drawn stays (moving it would jump the body).
    fn straighten_reckoned(&mut self, now: u32, stamp: f64, p: [f32; 3]) {
        let n = self.reckoned_tail.min(self.samples.len());
        let Some(anchor_i) = self.samples.len().checked_sub(n + 1) else {
            return;
        };
        let (ta, pa) = self.samples[anchor_i];
        let span = stamp - ta;
        if span <= 1e-3 {
            return;
        }
        let playout = now as f64 - self.playout_delay(now) as f64;
        for i in (anchor_i + 1)..self.samples.len() {
            // the segment that ends at sample i must not be in use: its start is still ahead of the playout point
            if self.samples[i - 1].0 <= playout {
                continue;
            }
            let (ti, _) = self.samples[i];
            let a = (((ti - ta) / span) as f32).clamp(0.0, 1.0);
            self.samples[i].1 = [
                pa[0] + (p[0] - pa[0]) * a,
                pa[1] + (p[1] - pa[1]) * a,
                pa[2] + (p[2] - pa[2]) * a,
            ];
        }
    }

    /// The smoothness statistics of one drawn frame: the drawn speed, a reversed step, a jump (CLI-1331 amended).
    fn note_draw(&mut self, now: u32, pos: [f32; 3]) {
        let prev = self.prev_draw.replace((now, pos));
        let Some((t0, p0)) = prev else {
            return;
        };
        let dt = now.saturating_sub(t0) as f32 / 1000.0;
        if dt <= 0.0 {
            return;
        }
        let step = [pos[0] - p0[0], pos[1] - p0[1], pos[2] - p0[2]];
        let len = (step[0] * step[0] + step[1] * step[1] + step[2] * step[2]).sqrt();
        let speed = len / dt;
        if speed > EXTRAP_MAX_SPEED {
            // a placement (respawn, snap), not motion: neither a speed nor a step to compare with
            self.prev_step = None;
            return;
        }
        let prev_step = self.prev_step.replace(step);
        if speed < MOVING_SPEED {
            return;
        }
        let back = prev_step.is_some_and(|q| {
            let ql = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt();
            ql > 0.5 && len > 0.5 && step[0] * q[0] + step[1] * q[1] + step[2] * q[2] < 0.0
        });
        let mean_step = if self.window.moving_frames >= 30 {
            (self.window.speed_sum / self.window.moving_frames as f64) as f32 * dt
        } else {
            f32::MAX
        };
        let jump = len > JUMP_FACTOR * mean_step;
        for st in [&mut self.total, &mut self.window] {
            st.moving_frames += 1;
            st.speed_sum += speed as f64;
            st.speed_sq += (speed as f64) * (speed as f64);
            st.backsteps += back as u64;
            st.jumps += jump as u64;
        }
    }

    /// Samples not yet played at `now` (stamped later than the playout point): the buffer depth in updates.
    pub fn buffer_depth(&self, now: u32) -> usize {
        let r = now as f64 - self.playout_delay(now) as f64;
        self.samples.iter().rev().take_while(|s| s.0 > r).count()
    }

    /// Since the newest sample arrived (ms).
    pub fn sample_age(&self, now: u32) -> Option<u32> {
        self.last_arrival.map(|t| now.saturating_sub(t))
    }

    /// How the last frame drew the player (CLI-1330 / CLI-1331) for the network overlay.
    pub fn extrap_state(&self) -> crate::hud::netview::Extrap {
        use crate::hud::netview::Extrap;
        if self.samples.is_empty() {
            Extrap::None
        } else if self.in_held {
            Extrap::Held
        } else if self.in_late {
            Extrap::Extrap
        } else {
            Extrap::Interp
        }
    }

    /// The `remote_interp` event-log fields of the window since the last report (`window_ms` long), which is then
    /// cleared.
    pub fn take_report(&mut self, now: u32, window_ms: u32) -> serde_json::Value {
        let w = std::mem::take(&mut self.window);
        let per_min = |n: u64| (n as f64 * 600_000.0 / window_ms.max(1) as f64).round() / 10.0;
        let r1 = |x: f64| (x * 10.0).round() / 10.0;
        // the spread of the drawn speed over its mean (0 = steady)
        let speed_cv = {
            let n = w.moving_frames as f64;
            if n > 1.0 {
                let m = w.speed_sum / n;
                let var = (w.speed_sq / n - m * m).max(0.0);
                ((var.sqrt() / m.max(1e-6)) * 1000.0).round() / 1000.0
            } else {
                0.0
            }
        };
        serde_json::json!({
            "pid": self.pid,
            "name": self.name,
            "window_ms": window_ms,
            "samples": w.samples,
            "stale": w.stale,
            "stream_gap_ms": r1(self.gap_ms as f64),
            "late_max_ms": r1(self.late_q as f64),
            "delay_ms": r1(self.playout_delay(now) as f64),
            "delay_mean_ms": r1(if w.frames > 0 { w.delay_sum_ms / w.frames as f64 } else { 0.0 }),
            "frames": w.frames,
            "late_frames": w.late_frames,
            "held_frames": w.held_frames,
            "late_per_min": per_min(w.late_runs),
            "holds_per_min": per_min(w.held_runs),
            "moving_frames": w.moving_frames,
            "speed_mean": r1(if w.moving_frames > 0 { w.speed_sum / w.moving_frames as f64 } else { 0.0 }),
            "speed_cv": speed_cv,
            "backsteps_per_min": per_min(w.backsteps),
            "jumps_per_min": per_min(w.jumps),
            "repeats": w.repeats,
            "reckoned": w.reckoned,
            "server_clock": !self.arrival_clock && self.clock_base().is_some(),
        })
    }
}

/// The status the overlay shows.
#[derive(Debug, Clone, Default)]
pub struct Hud {
    pub life_state: u8,
    pub health: Option<f32>,
    pub max_health: Option<f32>,
    pub armor: Option<f32>,
    pub max_armor: Option<f32>,
    pub money: Option<u32>,
    pub team_score: [u8; 2],
    pub round_wins: [u8; 2],
    /// Round timer: seconds at the time (ms) it was set, running or stopped.
    pub timer: Option<(u32, u32, bool)>,
    pub kill_feed: VecDeque<String>,
    pub round_result: Option<(i8, [u8; 2])>,
    pub result_screen: Option<(bool, u32)>,
    pub level_exit: Option<(String, bool, bool)>,
}

/// Our names of the player states (meanings PLAUSIBLE, GAME_MESSAGES_2009_S2C.md §4.1).
pub fn life_state_name(s: u8) -> &'static str {
    match s {
        0 => "not in play",
        1 => "alive",
        2 => "dead",
        3 => "waiting",
        4 => "state 4",
        5 => "revived",
        6 => "spectator",
        _ => "unknown",
    }
}

fn opt(v: Option<f32>) -> String {
    v.map_or("-".into(), |x| format!("{x:.0}"))
}

impl Hud {
    /// Seconds left on the round timer at `now` ms.
    pub fn time_left(&self, now: u32) -> Option<u32> {
        self.timer.map(|(secs, at, running)| {
            if running {
                secs.saturating_sub(now.saturating_sub(at) / 1000)
            } else {
                secs
            }
        })
    }

    /// The overlay lines.
    pub fn lines(&self, phase: &NetPhase, players: usize, now: u32) -> Vec<String> {
        let mut v = vec![
            format!(
                "net {}  state {} ({})  HP {}/{}  AR {}/{}",
                phase.label(),
                self.life_state,
                life_state_name(self.life_state),
                opt(self.health),
                opt(self.max_health),
                opt(self.armor),
                opt(self.max_armor)
            ),
            // 0xb5 feeds the 2009 scoreboard pair (S2C row 0xb5, cshell+0x92bb0); the reference stack sends the team kills
            // of a TDM round in it and never 0x4a (CAPTURE_tdm4_KILL_VALIDATION.md K9), so it is shown as "board"
            // DC-DEFAULT: CLI-27 (proposed) HUD label of the 0xb5 pair
            format!(
                "score {}:{}  board {}:{}  time {}  players {}",
                self.team_score[0],
                self.team_score[1],
                self.round_wins[0],
                self.round_wins[1],
                self.time_left(now)
                    .map_or("-".into(), |s| format!("{}:{:02}", s / 60, s % 60)),
                players
            ),
        ];
        v.extend(self.kill_feed.iter().cloned());
        if let Some((win, w)) = self.round_result {
            v.push(match win {
                1 | 2 => format!("ROUND OVER: team {win} wins ({}:{})", w[0], w[1]),
                _ => format!("ROUND OVER: draw ({}:{})", w[0], w[1]),
            });
        }
        if let Some((flag, value)) = self.result_screen {
            v.push(format!(
                "MATCH RESULT screen (flag {} value {value})",
                flag as u8
            ));
        }
        if let Some((w, keep, now_flag)) = &self.level_exit {
            v.push(if w.is_empty() && *now_flag {
                "LEVEL EXIT: back to the menus".to_string()
            } else {
                format!(
                    "LEVEL EXIT: next world {w:?} (keep {}, now {})",
                    *keep as u8, *now_flag as u8
                )
            });
        }
        v
    }
}

/// One kill-feed line: killer, weapon number, victim, head-shot mark.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 effect 8 (kill feed: killer, victim, weapon, head-shot flag)
pub fn kill_line(k: &KillRecord, name_of: &dyn Fn(u8) -> String) -> String {
    let killer = if k.killer == NO_KILLER {
        "(world)".to_string()
    } else {
        name_of(k.killer)
    };
    format!(
        "{killer} [weapon {}] {}{}",
        k.weapon,
        name_of(k.victim),
        if k.hit_zone == 1 { " (head)" } else { "" }
    )
}

/// What a test or the caller reads back after (or during) a session.
#[derive(Debug, Clone, Default)]
pub struct NetStats {
    /// Weapon counters (selects, shots, 0x82 messages, hits on boxes, reloads, holds, throws, sounds).
    pub arms: ArmsStats,
    /// The held weapon at the end.
    pub weapon: Option<Carried>,
    /// Every weapon grant received (slot, weapon, ammo id).
    pub grants: Vec<(u8, u16, u16)>,
    /// Every kill message as (victim, killer).
    pub kill_pairs: Vec<(u8, u8)>,
    /// 0x74 messages received (damage to the own player).
    pub damage_taken: u32,
    /// Shot records of the own shots that carried the hit feedback (`sound::hit_feedback_wanted`).
    pub own_hit_records: u64,
    /// CLI-1603: own pellets that met a character's box: each played the hit feedback at the shot.
    pub predicted_hits: u64,
    /// Remote characters dead at the end, and how many came back during the session ([`Corpses`]).
    pub corpses: usize,
    pub revived: u64,
    /// Shot traces that left out at least one dead character's box.
    pub corpse_skips: u64,
    pub phase: Option<NetPhase>,
    pub milestones: Vec<(String, u32)>,
    pub stage: String,
    pub end: Option<String>,
    pub fatal: Vec<String>,
    pub parse_notes: Vec<String>,
    pub updates_sent: u64,
    /// Gaps between consecutive player updates on the session clock (ms -> count).
    pub update_gaps: std::collections::BTreeMap<u32, u32>,
    pub teleports: Vec<(u8, [f32; 3], [f32; 3])>,
    pub life_states: Vec<u8>,
    /// Corrections the client applied to its simulation.
    pub corrections_taken: u64,
    /// Remote players by object id: the ones in the world now and the ones who left during the session
    /// ([`Track::left`]; a live player on the same object id wins).
    pub tracks: BTreeMap<u16, Track>,
    pub kills: Vec<String>,
    pub hud: Hud,
    /// The test hook shifted at least one sent update.
    pub glitch_sent: bool,
    /// Movement notices 0x76 [1][e] sent.
    pub notices_sent: u64,
    /// How the session was left (see [`NetPhase`]): `leave_match`, `quit` or `None` (ended by the server).
    pub leave_kind: Option<String>,
    /// The lobby leave notice 0x394 {0} was sent.
    pub leave_notice_sent: bool,
    /// The mode class the respawn wait uses (server 0x9e, else the room's ROM mode row).
    pub mode_class: Option<u8>,
    /// Respawn requests sent (`UP_RESPAWN_REQUEST` 0x89 and its class variants 0xc5 / 0xd2).
    pub respawn_requests: u64,
    /// The killer of the last own death (the 0x6d that named the local player as the victim).
    pub last_killer: Option<u8>,
    /// Freeze / release blocks of 0xa0 received.
    pub mover_notes: u64,
    /// When the result screen 0x3f opened and when the client left it (ms of the session clock).
    pub result_at: Option<u32>,
    pub result_left_at: Option<u32>,
    /// `move_seq` of the last update handed to the session.
    pub last_seq_reported: Option<u8>,
    pub client_id: Option<u16>,
    pub view_object: Option<u16>,
    pub bad_check: u64,
    pub resends: u64,
    pub world: Option<String>,
    /// Dropped weapons seen and removed, and the grants taken as pickups ([`crate::drops`]).
    pub drops: crate::drops::DropCounters,
    /// What the result screen shows (after 0x3f).
    pub match_result: Option<crate::matchexit::MatchResult>,
    /// The camera jars of hits on the own player: (session ms, sector) ([`crate::camfx`]).
    pub camera_jars: Vec<(u32, u8)>,
    /// The recoil's yaw kicks and side flips ([`crate::recoil`]).
    pub recoil_wags: u32,
    pub recoil_flips: u32,
}

impl NetStats {
    pub fn reached(&self, m: &str) -> bool {
        self.milestones.iter().any(|x| x.0 == m)
    }
}

/// State shared by the network thread and the client loop.
struct Shared {
    start: Instant,
    phase: NetPhase,
    session: Option<BotSession>,
    tracks: BTreeMap<u16, Track>,
    /// Tracks of players who left, kept for [`NetStats::tracks`] only.
    left_tracks: BTreeMap<u16, Track>,
    hud: Hud,
    corrections: Vec<Correction>,
    view_sets: Vec<(u8, [f32; 3])>,
    /// Box of the last teleport (drawn for remote players too).
    player_dims: Option<[f32; 3]>,
    corrections_taken: u64,
    kills: Vec<String>,
    stop: bool,
    glitch_until: Option<u32>,
    glitch_done: bool,
    glitch_sent: bool,
    last_seq_reported: Option<u8>,
    world: Option<String>,
    /// Game messages of the client loop waiting for the network thread: (shell id, body).
    outgoing: Vec<(u8, BitWriter)>,
    /// Lobby frames of the client loop (the in-match report 0x312, CLI-950) for the lobby connection's keeper.
    lobby_out: Vec<PktWriter>,
    /// The last sprint pair sent (`UP_SPRINT_STATE` 0xca).
    sprint_pair: Option<(bool, bool)>,
    /// Movement notices sent.
    notices_sent: u64,
    /// When the client entered the world (ms of the shared clock).
    in_world_at: Option<u32>,
    /// How the session was left: "leave_match" (0xbd + 0x394 + C2S_BYE), "quit" (C2S_BYE only), "server".
    leave_kind: Option<String>,
    /// 0x394 {0} handed to the lobby connection.
    leave_notice_sent: bool,
    /// The local player's weapons.
    arsenal: Arsenal,
    /// The own equipment slots of the lobby (0x323), for the speed rows (CLI-1137).
    equipment: Vec<i32>,
    /// Where the thrown objects are drawn between their stream samples ([`crate::thrown`]).
    thrown: crate::thrown::ThrownMotion,
    grants: Vec<(u8, u16, u16)>,
    damage_taken: u32,
    own_hit_records: u64,
    /// CLI-1603: own pellets that met a character's box (the feedback played at the shot).
    predicted_hits: u64,
    corpses: Corpses,
    /// Characters inside their spawn window: object -> session ms of the start note (CLI-572).
    spawn_shield: std::collections::HashMap<u16, u32>,
    corpse_skips: u64,
    /// HUD inputs since the last frame (the frame loop applies them with its view).
    hud_events: Vec<(u32, HudEvent)>,
    /// The arsenal's select count as last reported to the HUD's weapon panel (client-hudgaps).
    panel_selects: u32,
    /// The roster as last reported to the HUD.
    hud_roster: BTreeMap<u8, RosterRecord>,
    /// Own id and mode class as last reported to the HUD.
    hud_own: (Option<u8>, Option<u8>),
    /// A health of 0 while alive was logged (CLI-1853); cleared by a health above 0 or a death state.
    zero_health_logged: bool,
    /// The server's step height (`S2C_EVAR`) and how many of the session's variables were already looked at.
    step_height: Option<f32>,
    evars_scanned: usize,
    /// Sound cues since the last frame.
    sound_cues: Vec<SoundCue>,
    /// Weapon-effect events since the last frame ([`crate::fx`]).
    fx_events: Vec<crate::fx::FxEvent>,
    /// The own character object and its animation arrival times (the server streams it to its owner, DSV-23).
    own_track: Option<(u16, Track)>,
    /// The world-load group (the first group with creates) was seen.
    world_group_done: bool,
    /// Preload lists already handed on.
    preload_seen: usize,
    /// The session's create count when the ladder volumes were last handed to the sounds (w2-audiocues).
    ladders_seen: u64,
    /// The own character object as last reported to the HUD.
    hud_view_object: Option<u16>,
    /// The last countdown sent to each pickup object (shell 0xa6 type 0x17 sub 2).
    case_countdown: BTreeMap<u16, i32>,
    /// The world cameras' field-of-view notes (shell 0xa6 kind 0x0d, CLI-1862).
    cam_fovs: crate::camshot::CameraFovs,
    /// Soft failures of the HUD's shell readers (appended to [`NetStats::parse_notes`]).
    hud_notes: Vec<String>,
    /// The own death time (0x67 state 2; cleared by state 1) and the last killer of the local player (0x6d).
    death_at: Option<u32>,
    last_killer: Option<u8>,
    /// The `from_dir` of the last 0x74 received while playing (the killer camera looks back along it).
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.3 row 0x74 (stored at controller +0x1ec..+0x1f4; ignored in states 2, 3,
    // 5, 6); DISASM cshell+0x279404..0x27945b (the killer camera reads it through cshell+0x1fcd00)
    last_shot_dir: Option<[f32; 3]>,
    /// 0xa0 freeze / release blocks since the last frame, and their count.
    mover_notes: Vec<crate::world::MoverNote>,
    mover_count: u64,
    /// When 0x3f opened the result screen, and when the client left it.
    result_at: Option<u32>,
    result_left_at: Option<u32>,
    /// The dropped weapons in the world and the pickup grants ([`crate::drops`]).
    drops: crate::drops::DropTracker,
    /// The doors' activate entries and the last use target given to the HUD (CLI-1230).
    use_entries: crate::hud::prompt::UseEntries,
    use_target: Option<crate::hud::prompt::UseEntry>,
    /// A dropped weapon lay in the use key's reach at the last frame (the item prompt, CLI-1352).
    item_target: bool,
    /// The target of the running hold to plant / defuse (CLI-1054).
    bomb_hold: Option<u16>,
    /// The own plant / defuse hold's sounds (CLI-1530): its start (ms), defuse, the bomb row.
    hold_sound: Option<(u32, bool, u16)>,
    /// w2-sprites: the billboard objects and the list of the last poll ([`crate::sprites`]).
    billboards: crate::sprites::BillboardTracker,
    billboard_list: Vec<crate::sprites::Billboard>,
    /// The weapon and the three mod numbers of each character's last shot record (CLI-618).
    remote_mods: BTreeMap<u16, (u16, [u8; 3])>,
    /// Arrival times of the animation blocks of corpse objects ([`crate::anims`]).
    anim_clocks: crate::anims::AnimClocks,
    /// The match's HUD inputs applied on the network side too (the result screen's data does not depend on a
    /// frame loop), and how many of the queued inputs it has seen.
    tally: crate::hud::HudState,
    tally_seen: usize,
    /// w2-loading: the loading screen's load stage ([`crate::loading::stage`]) as the server's messages set it.
    load_stage: u8,
    /// The network overlay's link meter (CLI-1571) and the game server's address.
    meter: meter::NetMeter,
    game_addr: Option<SocketAddr>,
}

impl Shared {
    fn now(&self) -> u32 {
        self.start.elapsed().as_millis().min(u32::MAX as u128) as u32
    }

    /// After every session poll: events into the HUD, corrections and view sets; stream samples into the tracks;
    /// the phase from the session stage.
    fn absorb(&mut self, now: u32) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let events = s.take_events();
        let names: BTreeMap<u8, String> = s
            .obs
            .roster
            .values()
            .chain(s.obs.departed.iter())
            .map(|r| (r.pid, r.name.clone()))
            .collect();
        let name_of = |pid: u8| {
            names
                .get(&pid)
                .cloned()
                .unwrap_or_else(|| format!("#{pid}"))
        };
        for e in events {
            match e {
                GameEvent::Teleport { seq, pos, dims } => {
                    self.player_dims = Some(dims);
                    self.arsenal.set_glide(false);
                    self.corrections.push(Correction {
                        seq,
                        pos: V3::from_array(pos),
                        dims: V3::from_array(dims),
                        velocity: None,
                    });
                }
                GameEvent::ViewAngles { kind, angles } => self.view_sets.push((kind, angles)),
                GameEvent::PlayerState(st) => {
                    self.hud_events.push((now, HudEvent::Life(st)));
                    self.hud.life_state = st;
                    self.arsenal.on_life(now, st);
                    self.drops.on_life(now, st);
                    // SPEC: GAME_MESSAGES_2009_S2C.md §4.1 (state 2 writes the death time `local_player`+0x440,
                    // state 1 clears it, state 3 leaves it)
                    match st {
                        1 => self.death_at = None,
                        2 => self.death_at = Some(now),
                        _ => {}
                    }
                }
                GameEvent::WeaponGrant(g) => {
                    // SPEC: GAME_MESSAGES_2009_S2C.md row 0xb2 (`client_shell`+0xc3b0 := 6)
                    self.load_stage = crate::loading::next_stage(
                        self.load_stage,
                        crate::loading::stage::WEAPON_GIVE,
                    );
                    self.grants.push((g.slot, g.weapon, g.ammo_kind));
                    // the inventory takes it; a pickup grant also keeps an identical weapon's clip and draws it
                    self.drops.grant(&mut self.arsenal, &g, now);
                }
                GameEvent::WeaponRemove { slot } => self.arsenal.on_remove(slot),
                GameEvent::WeaponSwitch {
                    command,
                    auto,
                    ammo,
                    ..
                } => self.arsenal.switch_command(command, auto, ammo),
                GameEvent::Damage(d) => {
                    self.damage_taken += 1;
                    // the camera jar of the hit (the "Reality" option, crate::camfx)
                    self.arsenal.own_hit(now, &d);
                    if !matches!(self.hud.life_state, 2 | 3 | 5 | 6) {
                        self.last_shot_dir = Some(d.from_dir);
                    }
                    self.hud_events.push((now, HudEvent::Damage(d.from_dir)));
                    self.hud_events.push((
                        now,
                        HudEvent::Hit {
                            damage: d.damage,
                            zone: d.zone,
                        },
                    ));
                    self.hud_events.push((
                        now,
                        HudEvent::Wound {
                            damage: d.damage,
                            kind: d.dmg_kind,
                        },
                    ));
                }
                GameEvent::Status(PlayerStatus::Value { kind, value, .. }) => {
                    // SPEC: GAME_MESSAGES_2009_S2C.md row 0x66 (kind 0x13: `client_shell`+0xc3b0 := 5)
                    if kind == crate::loading::STATUS_RESET_KIND {
                        self.load_stage = crate::loading::next_stage(
                            self.load_stage,
                            crate::loading::stage::STATUS_RESET,
                        );
                        // CLI-1762: a spawn set's grants refill the weapons, also for a round's survivor
                        self.drops.on_spawn_set(now);
                        // CLI-1855: the reset empties the carried weapons; the set's grants fill them again
                        self.arsenal.on_status_reset(now);
                    }
                    self.hud_events
                        .push((now, HudEvent::Status { kind, value }));
                    match kind {
                        status_kind::HEALTH => self.hud.health = Some(value),
                        status_kind::ARMOR => self.hud.armor = Some(value),
                        status_kind::MAX_HEALTH => self.hud.max_health = Some(value),
                        status_kind::MAX_ARMOR => self.hud.max_armor = Some(value),
                        _ => {}
                    }
                }
                GameEvent::Status(PlayerStatus::Money { money, .. }) => {
                    self.hud.money = Some(money)
                }
                GameEvent::Status(PlayerStatus::Ammo {
                    weapon, ammo, slot, ..
                }) => {
                    // SPEC: GAME_MESSAGES_2009_S2C.md row 0x66 (kind 1: `client_shell`+0xc3b0 := 7)
                    self.load_stage =
                        crate::loading::next_stage(self.load_stage, crate::loading::stage::AMMO);
                    self.arsenal.on_ammo(slot, weapon, ammo)
                }
                GameEvent::Kill(k) => {
                    // SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 effect 3 (the local victim keeps `killer` at
                    // `roster`+0x10, -1 when it killed itself); 0xff names no killer (field 2)
                    if s.obs.client_id.map(|c| c.0 as u8) == Some(k.victim) {
                        self.last_killer =
                            (k.killer != NO_KILLER && k.killer != k.victim).then_some(k.killer);
                    } else if let Some(r) = s.obs.roster.get(&k.victim) {
                        // DC-DEFAULT: CLI-99 (proposed), see [`Corpses`]
                        self.corpses.killed(r.character, now);
                    }
                    // the own kill voice (auto-cast local row 1)
                    if crate::sound::cues::own_kill_voice(
                        k.killer,
                        k.hit_zone,
                        s.obs.client_id.map(|c| c.0 as u8),
                    ) {
                        self.sound_cues.push(SoundCue::OwnKill);
                    }
                    self.hud_events.push((now, HudEvent::Kill(k)));
                    // the victim's own record is dead until a record says otherwise (spec 3.3.1 effect 2: +0x74 := 1):
                    // both copies change, so the respawn refresh (kind 2, `dead` 0) differs from them even when the
                    // death changed no count, and reaches the HUD (it clears the grey look of the marks and the board)
                    // EVIDENCE: cshell+0x2db36 (0x6d), cshell+0x8a760 (a record refresh sets the marker's flag again)
                    if let Some(r) = s.obs.roster.get_mut(&k.victim) {
                        r.dead = true;
                    }
                    if let Some(r) = self.hud_roster.get_mut(&k.victim) {
                        r.dead = true;
                    }
                    let line = kill_line(&k, &name_of);
                    self.kills.push(line.clone());
                    self.hud.kill_feed.push_back(line);
                    while self.hud.kill_feed.len() > KILL_FEED_LINES {
                        self.hud.kill_feed.pop_front();
                    }
                }
                GameEvent::TeamScore(p) => self.hud.team_score = p,
                GameEvent::RoundWins(p) => {
                    self.hud_events.push((now, HudEvent::TeamPair(p)));
                    self.hud.round_wins = p
                }
                GameEvent::MatchBegin { round_secs, .. } => {
                    // the HUD takes the whole 0x45 from `GameEvent::Shell`
                    self.hud.timer = (round_secs > 0).then_some((round_secs, now, true));
                }
                GameEvent::RoundBegin {
                    round_secs,
                    round_no,
                    round_limit,
                } => {
                    self.hud_events
                        .push((now, HudEvent::RoundBegin(round_secs)));
                    self.hud_events.push((
                        now,
                        HudEvent::RoundNumbers {
                            round_no,
                            round_limit,
                        },
                    ));
                    self.hud.timer = (round_secs > 0).then_some((round_secs, now, true));
                }
                GameEvent::RoundTimerStop(secs) => {
                    self.hud_events.push((now, HudEvent::ClockStop(secs)));
                    self.hud.timer = Some((secs, now, false))
                }
                GameEvent::RoundResult { win_team, wins } => {
                    // the HUD takes the whole 0x46 from `GameEvent::Shell`
                    self.hud.round_result = Some((win_team, wins));
                    self.hud.round_wins = wins;
                    // CLI-1250: the server's round-end stop takes over an own hold's stop
                    self.mover_notes.push(crate::world::MoverNote {
                        hold: Some(crate::world::HoldNote::HandOver),
                        ..crate::world::MoverNote::default()
                    });
                }
                GameEvent::Summary(row) => self.hud_events.push((now, HudEvent::Summary(row))),
                GameEvent::ResultScreen { flag, value } => {
                    self.hud.result_screen = Some((flag, value));
                    self.result_at.get_or_insert(now);
                }
                GameEvent::LevelExit {
                    next_world,
                    keep,
                    now: at_once,
                } => self.hud.level_exit = Some((next_world, keep, at_once)),
                GameEvent::Mover {
                    freeze, release, ..
                } => {
                    if freeze.is_some() || release.is_some() {
                        self.mover_notes
                            .push(crate::world::MoverNote {
                                freeze,
                                release,
                                hold: None,
                                glide: None,
                            });
                        self.mover_count += 1;
                    }
                }
                GameEvent::Effect(e) => {
                    if let Some(r) = crate::fx::record_of(&e, effect_names_own(s, &e)) {
                        self.fx_events.push(crate::fx::FxEvent::Record(r));
                    }
                    // CLI-1603: the record's hit feedback is counted only; the sound and the mark's hit colour
                    // played at the shot (the own pellet trace)
                    if effect_cue(&mut self.sound_cues, s, e, false) {
                        self.own_hit_records += 1;
                    }
                }
                GameEvent::Shell { shell, body, bits } => {
                    // the shot counter of the cone: 0x7d sets it, 0xda kind 1 advances it
                    // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1.5
                    let mut r = dc_wire::udp::BitReader::with_bit_len(&body, bits);
                    match shell {
                        // DC-DEFAULT: CLI-1642 (proposed) a Possession move: no firing, the body glides
                        dc_wire::possession::SHELL_ID => {
                            if let Some((to, ms)) = dc_wire::possession::read_move(&mut r) {
                                self.mover_notes.push(crate::world::MoverNote {
                                    glide: Some((to, ms)),
                                    ..crate::world::MoverNote::default()
                                });
                                self.arsenal.set_glide(true);
                            }
                        }
                        arms::down::MATCH_SETTINGS => {
                            if let Some(v) = arms::match_settings_shot_seed(&mut r) {
                                self.arsenal.on_shot_seed(v);
                            }
                        }
                        arms::down::PLAYER_POINTS => {
                            if let Some(d) = arms::player_points_shot_seed(&mut r) {
                                self.arsenal.on_shot_seed_add(d);
                            }
                        }
                        // SPEC: GAME_MESSAGES_2009_S2C.md 0x77 row `GS_OVERLAY` `[overlay_slot:u8][on:u8]`
                        // (cshell+0x1b90e0)
                        GS_OVERLAY => {
                            let slot = r.read_u8();
                            let on = r.read_u8() != 0;
                            self.arsenal.on_overlay(slot, on);
                        }
                        // SPEC: GAME_MESSAGES_2009_S2C.md §4.2 row 0xd9 `GS_MUSIC` (cshell+0x5de30)
                        GS_MUSIC => {
                            if let Some(o) = crate::music::read_music_order(&mut r) {
                                self.sound_cues.push(crate::sound::SoundCue::Music(o));
                            }
                        }
                        // w2-sprites: a pickup's new client FX
                        // SPEC: GAME_MESSAGES_2009_S2C.md §8.3 (type 0x17 sub 1)
                        crate::drops::SHELL_OBJECT_NOTE => {
                            if let Some((obj, name)) =
                                crate::drops::read_pickup_fx_note(&body, bits)
                            {
                                self.drops.on_fx_note(obj, name);
                            }
                            // a door's activate entry (type 0x39 sub 1 / 2, CLI-1230)
                            if let Some((obj, sub, v)) =
                                crate::hud::prompt::read_door_note(&body, bits)
                            {
                                self.use_entries.on_note(obj, sub, v);
                            }
                            // a remote character's aim (shell 0xa6 type 0x18 sub 0x80 / 0xc0)
                            // EVIDENCE: cshell+0x26f60, cshell+0x259b0
                            if let Some((obj, n)) = crate::aim::read_aim_note(&body, bits) {
                                if let Some(t) = self.tracks.get_mut(&obj) {
                                    t.aim.get_or_insert_with(Default::default).on_note(n);
                                }
                            }
                            // a world camera's field of view (type 0x0d sub 1, CLI-1862)
                            if let Some(n) = crate::camshot::read_fov_note(&body, bits) {
                                let blob = s
                                    .table
                                    .objects
                                    .get(&n.obj)
                                    .and_then(crate::camshot::camera_blob)
                                    .map(|b| b.2);
                                self.cam_fovs.on_note(n, blob, now);
                            }
                        }
                        _ => {}
                    }
                    let class = s.obs.mode_class.or(s.config().mode_class_hint);
                    if let Some(e) = shell_hud_event(
                        shell,
                        &body,
                        bits,
                        class,
                        &mut self.case_countdown,
                        &mut self.hud_notes,
                    ) {
                        // the death note mutes that character's footsteps until the spawn note (CLI-680)
                        if let HudEvent::Note(crate::hud::msgs::ObjectNote::StepMute { obj, on }) =
                            &e
                        {
                            self.sound_cues
                                .push(SoundCue::StepMute { obj: *obj, on: *on });
                        }
                        if let HudEvent::Note(crate::hud::msgs::ObjectNote::SpawnMark { obj, on }) =
                            &e
                        {
                            self.sound_cues
                                .push(SoundCue::SpawnMark { obj: *obj, on: *on });
                            // the shot test skips a character in its spawn window (CLI-572)
                            if *on {
                                self.spawn_shield.insert(*obj, now);
                            } else {
                                self.spawn_shield.remove(obj);
                            }
                        }
                        self.hud_events.push((now, e));
                    }
                }
            }
        }
        // the lethal 0x66 comes with the death states in one block, so a health of 0 still alive after the whole
        // poll contradicts the state: the HUD keeps the last live value (HudState::shown_health), logged once
        // DC-DEFAULT: CLI-1853 (proposed)
        let zero_alive =
            self.hud.life_state == 1 && self.hud.health.is_some_and(|h| h.is_nan() || h <= 0.0);
        if zero_alive && !self.zero_health_logged {
            eprintln!(
                "dc-client: the server sent health {:?} while the own player is alive; the HUD keeps the last live value",
                self.hud.health
            );
        }
        self.zero_health_logged = zero_alive;
        // the field-of-view notes of camera objects that are gone
        self.cam_fovs.retain(&s.table.objects);
        if let Some(c) = s.obs.client_id {
            self.arsenal.set_own_id(c.0 as u8);
        }
        // HUD: own id / mode class, roster records that changed, joins and departures
        let own = (
            s.obs.client_id.map(|c| c.0 as u8),
            s.obs.mode_class.or(s.config().mode_class_hint),
        );
        if own != self.hud_own {
            self.hud_own = own;
            self.hud_events.push((
                now,
                HudEvent::Own {
                    pid: own.0,
                    mode_class: own.1,
                },
            ));
        }
        if s.obs.view_object != self.hud_view_object {
            self.hud_view_object = s.obs.view_object;
            self.hud_events
                .push((now, HudEvent::ViewObject(s.obs.view_object)));
        }
        let in_world = self.in_world_at.is_some();
        for (pid, r) in &s.obs.roster {
            if self.hud_roster.get(pid) != Some(r) {
                let new = !self.hud_roster.contains_key(pid);
                self.hud_events.push((now, HudEvent::Roster(r.clone())));
                if new && in_world && own.0 != Some(*pid) {
                    self.hud_events
                        .push((now, HudEvent::Joined(r.name.clone())));
                }
                self.hud_roster.insert(*pid, r.clone());
            }
        }
        let gone: Vec<u8> = self
            .hud_roster
            .keys()
            .filter(|p| !s.obs.roster.contains_key(p))
            .copied()
            .collect();
        for pid in gone {
            if let Some(r) = self.hud_roster.remove(&pid) {
                self.hud_events.push((now, HudEvent::Left(pid, r.name)));
            }
        }
        // sounds: the records of the object groups, then the preload lists
        // SPEC: UDP_SESSION_2009_B.md §8.3 (world sounds come in the id 8 message of the world-load creates)
        let first_group = !self.world_group_done && s.table.creates > 0;
        if s.table.creates > 0 {
            self.world_group_done = true;
        }
        for e in std::mem::take(&mut s.table.sound_events) {
            self.sound_cues.push(match e {
                SoundNote::Start(st) => SoundCue::Server {
                    file: s.obs.files.get(&st.file).map(|f| f.1.clone()),
                    start: st,
                    at_load: first_group,
                },
                SoundNote::Move { track, pos } => SoundCue::Move { track, pos },
                SoundNote::Stop { track } => SoundCue::Stop { track },
            });
        }
        // the ladder volumes' movement-sound flag and surface override (w2-audiocues, CLI-528), read again only when
        // the session created objects since the last look
        if s.table.creates != self.ladders_seen {
            self.ladders_seen = s.table.creates;
            let ladders = crate::sound::cues::ladder_sounds(&s.table.objects);
            self.sound_cues.push(SoundCue::Ladders(ladders));
        }
        while self.preload_seen < s.obs.preload_lists.len() {
            let (sub, ids) = &s.obs.preload_lists[self.preload_seen];
            self.preload_seen += 1;
            self.sound_cues.push(SoundCue::Preload {
                sub: *sub,
                files: ids
                    .iter()
                    .map(|id| (*id, s.obs.files.get(id).map(|f| f.1.clone())))
                    .collect(),
            });
        }
        // remote players: every roster player's object except the own one
        let own_obj = s.obs.view_object;
        let own_pid = s.obs.client_id.map(|c| c.0);
        // the own character object: its animation for the own footsteps
        let own_char = s
            .obs
            .roster
            .values()
            .find(|r| Some(r.pid as u16) == own_pid && r.character != 0xFFFF)
            .map(|r| r.character);
        match own_char.and_then(|c| s.table.objects.get(&c).map(|o| (c, o))) {
            Some((c, o)) => {
                let t = match self.own_track.as_mut() {
                    Some((oc, t)) if *oc == c => t,
                    _ => {
                        self.own_track = Some((c, Track::default()));
                        &mut self.own_track.as_mut().unwrap().1
                    }
                };
                if o.anim_updates != t.anim_seen {
                    t.anim_seen = o.anim_updates;
                    t.anim_at = now;
                }
            }
            None => self.own_track = None,
        }
        let mut live: Vec<u16> = Vec::new();
        let server_time = s.table.server_time;
        let chars = drawn_characters(s, own_obj, own_pid);
        for (character, pid, name, team) in chars {
            let Some(o) = s.table.objects.get(&character) else {
                continue;
            };
            live.push(character);
            let t = self.tracks.entry(character).or_default();
            t.pid = pid;
            t.name = name;
            t.team = team;
            t.follow_object_at(o, now, server_time);
            // the drawn aim eases toward the last note on the session clock (cshell+0x256f0, CLI-569)
            if let Some(a) = t.aim.as_mut() {
                a.advance(now.saturating_sub(t.aim_at) as f32 / 1000.0);
            }
            t.aim_at = now;
            if o.anim_updates != t.anim_seen {
                t.anim_seen = o.anim_updates;
                t.anim_at = now;
                let main = o
                    .anim
                    .iter()
                    .find(|a| a.tracker.is_none())
                    .map(|a| a.state1);
                self.corpses.animated(character, now, main);
            }
        }
        for gone in self.tracks.keys().filter(|k| !live.contains(k)) {
            self.corpses.forget(*gone);
        }
        retire_tracks(&mut self.tracks, &mut self.left_tracks, &live);
        self.drops.sync(s, now);
        // the thrown objects' network samples, stamped on arrival
        let secs = self.start.elapsed().as_secs_f64();
        for (&obj, o) in s.table.objects.iter() {
            if o.blob_type == Some(crate::fx::THROWN_BLOB) {
                if let Some(p) = o.pos {
                    self.thrown.observe(obj, p, o.vel.unwrap_or([0.0; 3]), secs);
                }
            }
        }
        // CLI-1136: a thrown object that left takes its projectile effect with it
        for k in self.thrown.objects() {
            if !s.table.objects.contains_key(&k) {
                self.fx_events.push(crate::fx::FxEvent::WorldGone(k));
            }
        }
        self.thrown.retain(|k| s.table.objects.contains_key(&k));
        // w2-sprites: the pickups' client FX (create blob, 0xa6 sub 1) and the billboards
        self.fx_events.extend(
            self.drops
                .take_fx()
                .into_iter()
                .map(crate::fx::FxEvent::World),
        );
        self.billboard_list = self.billboards.sync(s, now);
        if let Some(w) = s.world_request() {
            self.world = Some(w.to_string());
        }
        let phase = match s.stage() {
            Stage::Joining | Stage::Linked => NetPhase::Joining,
            Stage::LoadingWorld => {
                NetPhase::Loading(s.obs.announced_world.clone().unwrap_or_default())
            }
            Stage::WorldLoaded => NetPhase::Waiting,
            Stage::InWorld => {
                if self.in_world_at.is_none() {
                    // the match's first in-world update: 2009 changes no music here; the cue records the state
                    // SPEC: AUDIO_2009.md §8.2 (in-match music during normal modes: none found)
                    self.sound_cues
                        .push(SoundCue::MusicCue(crate::music::MusicCue::MatchStart));
                }
                self.in_world_at.get_or_insert(now);
                // EVIDENCE: cshell+0x1bb910 (the in-world branch writes `client_shell`+0xc3b0 := 8)
                self.load_stage = crate::loading::stage::IN_WORLD;
                NetPhase::InWorld
            }
            Stage::Failed(c) => NetPhase::Failed(format!(
                "{} (0x{c:x})",
                s.obs.end.clone().unwrap_or_else(|| "join failed".into())
            )),
            Stage::Dropped(_) | Stage::Closed => {
                NetPhase::Ended(s.obs.end.clone().unwrap_or_else(|| "closed".into()))
            }
        };
        self.phase = phase;
        for (t, e) in self.hud_events.iter().skip(self.tally_seen) {
            self.tally.apply(e, *t, 0.0);
            self.tally.tick(*t, 10_000);
        }
        self.tally_seen = self.hud_events.len();
    }

    /// What the result screen shows, once 0x3f arrived ([`crate::matchexit`]).
    fn match_result(&self) -> Option<crate::matchexit::MatchResult> {
        crate::matchexit::MatchResult::from_hud(&self.tally)
    }

    fn stats(&self) -> NetStats {
        let mut st = NetStats {
            arms: self.arsenal.stats,
            weapon: self.arsenal.held().copied(),
            grants: self.grants.clone(),
            damage_taken: self.damage_taken,
            own_hit_records: self.own_hit_records,
            predicted_hits: self.predicted_hits,
            corpses: self.corpses.count(),
            revived: self.corpses.revived,
            corpse_skips: self.corpse_skips,
            phase: Some(self.phase.clone()),
            corrections_taken: self.corrections_taken,
            tracks: self
                .left_tracks
                .iter()
                .chain(self.tracks.iter())
                .map(|(k, t)| (*k, t.clone()))
                .collect(),
            kills: self.kills.clone(),
            hud: self.hud.clone(),
            glitch_sent: self.glitch_sent,
            last_seq_reported: self.last_seq_reported,
            world: self.world.clone(),
            notices_sent: self.notices_sent,
            leave_kind: self.leave_kind.clone(),
            leave_notice_sent: self.leave_notice_sent,
            last_killer: self.last_killer,
            mover_notes: self.mover_count,
            result_at: self.result_at,
            result_left_at: self.result_left_at,
            drops: self.drops.stats.clone(),
            match_result: self.match_result(),
            camera_jars: self.arsenal.camfx.history.clone(),
            recoil_wags: self.arsenal.recoil.wags,
            recoil_flips: self.arsenal.recoil.flips,
            ..NetStats::default()
        };
        if let Some(s) = &self.session {
            let o = &s.obs;
            st.milestones = o.milestones.clone();
            st.stage = format!("{:?}", s.stage());
            st.end = o.end.clone();
            st.fatal = o.fatal.clone();
            st.parse_notes = o.parse_notes.clone();
            st.parse_notes.extend(self.hud_notes.iter().cloned());
            st.updates_sent = o.updates_sent;
            st.update_gaps = o.update_gaps.clone();
            st.teleports = o.teleports.clone();
            st.life_states = o.life_states.clone();
            st.client_id = o.client_id.map(|c| c.0);
            st.kill_pairs = o.kills.clone();
            st.view_object = o.view_object;
            st.mode_class = o.mode_class.or(s.config().mode_class_hint);
            st.respawn_requests = [0x89u8, 0xc5, 0xd2]
                .iter()
                .map(|id| o.c2s_shell_counts.get(id).copied().unwrap_or(0))
                .sum();
            if let Some(l) = s.link_stats() {
                st.bad_check = l.bad_check;
                st.resends = l.resends;
            }
        }
        st
    }
}

/// An engine id 17 effect as a sound cue: the player sound relay (own = the record names the own client id) or
/// the shot effect (own = the shooter is the own character).
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 (type 0x19 `player` = the engine client id; type 0x06 `shooter` = the
// shooter's character)
/// Returns true when the record was an own shot's hit feedback; with `feedback` the [`SoundCue::OwnHit`] is pushed
/// too (CLI-1603 plays it at the shot instead, so the session passes false).
fn effect_cue(cues: &mut Vec<SoundCue>, s: &BotSession, e: Effect, feedback: bool) -> bool {
    let own_pid = s.obs.client_id.map(|c| c.0 as u32);
    let own_rec = s
        .obs
        .roster
        .values()
        .find(|r| Some(r.pid as u32) == own_pid);
    let own_char = own_rec.map(|r| r.character);
    match e {
        Effect::SoundRelay(p) => cues.push(SoundCue::Player {
            k: p.k,
            weapon: p.weapon,
            player: p.player,
            pos: p.pos,
            own: Some(p.player) == own_pid,
            extra: p.extra,
            own_team: own_rec.map(|r| r.team),
        }),
        // the 2009 rule is the record's `c` against the own client id (DISASM cshell+0x2c6580); the own character
        // check covers a server that does not fill `c` (the reference stack wrote 1 on every record)
        Effect::Shot(x) => {
            cues.push(SoundCue::Shot {
                shooter: x.shooter,
                weapon: x.weapon,
                ammo: x.ammo,
                from: x.from,
                to: x.to,
                mods: x.mods,
                clear: x.clear,
                own: crate::sound::shot_names_own_client(x.client, own_pid)
                    || Some(x.shooter) == own_char
                    || Some(x.shooter) == s.obs.view_object,
            });
            // the shooter's hit feedback: only the record's `c` decides that the shot is the own one
            // DISASM: cshell+0x2c8b80..0x2c8b91 (+0xdd == +0xdc, then the 0x4000 test)
            if crate::sound::shot_names_own_client(x.client, own_pid)
                && crate::sound::hit_feedback_wanted(x.clear)
            {
                if feedback {
                    cues.push(SoundCue::OwnHit {
                        first: x.first,
                        ammo: x.ammo,
                    });
                }
                return true;
            }
        }
        Effect::Other(_) => {}
    }
    false
}

/// Whether a shot-effect record belongs to the own player: the record's client id is the own one, or its shooter is
/// the own character or view object (the rule of [`effect_cue`]).
fn effect_names_own(s: &BotSession, e: &Effect) -> bool {
    let Effect::Shot(x) = e else {
        return false;
    };
    let own_pid = s.obs.client_id.map(|c| c.0 as u32);
    let own_char = s
        .obs
        .roster
        .values()
        .find(|r| Some(r.pid as u32) == own_pid)
        .map(|r| r.character);
    crate::sound::shot_names_own_client(x.client, own_pid)
        || Some(x.shooter) == own_char
        || Some(x.shooter) == s.obs.view_object
}

/// The ammunition of a thrown object: the third byte of its create blob `[0x13][weapon:u8][ammo:u8][flag:u8]`.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 row 0x13
fn thrown_ammo(s: &BotSession, obj: u16) -> Option<u16> {
    let b = s.table.objects.get(&obj)?.blob.as_ref()?;
    (b.bit_len() >= 24)
        .then(|| b.as_bytes().get(2).map(|&a| u16::from(a)))
        .flatten()
}

/// The player id an AI character's record carries (it has no roster id).
// SPEC: FIRETEAM_2009.md §1.2 (the record's pid field; an AI is no roster player)
pub const NPC_PID: u8 = 0xFF;
/// The create blob type of a character record.
// SPEC: FIRETEAM_2009.md §1.1 (players and AI characters: create blob type 0x18, one client class)
pub const CHARACTER_BLOB: u8 = 0x18;
/// Bits of a character record before the NPC tail (the type byte included).
// SPEC: FIRETEAM_2009.md §1.2 (the 572-bit fixed record after the type byte; widths 1, 1, 8 x 6, 64, 32 x 6, 96, 96,
// 32, 32, 1, 8, 1)
pub const CHARACTER_FIXED_BITS: usize = 8 + 2 + 48 + 64 + 192 + 96 + 96 + 32 + 32 + 1 + 8 + 1;

/// The NPC tail of an AI character's record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpcCharacter {
    pub team: u8,
    pub kind: u8,
    pub caption: String,
}

/// An AI character: a character record whose first bit (roster binding) is clear and whose second (NPC tail) is set;
/// its team and caption come from the tail. `None` for players and for blobs the table did not keep.
// SPEC: FIRETEAM_2009.md §1.2 (path selection: roster_bound 0, has_tail 1; tail T1-T5: u8 rank, u8 team, u8 kind,
// u32 health, zstr8 caption), §1.8 (the AI's model block is laid out as a player's: the client draws it as one)
// DC-DEFAULT: CLI-1857 (proposed) an AI character is drawn as a remote body with its caption as the name
pub fn npc_character(o: &dc_botclient::objects::ClientObject) -> Option<NpcCharacter> {
    if o.blob_type != Some(CHARACTER_BLOB) {
        return None;
    }
    let b = o.blob.as_ref()?;
    if b.bit_len() < CHARACTER_FIXED_BITS + 8 * 7 + 8 {
        return None;
    }
    let mut r = b.reader();
    r.read_u8();
    let (roster_bound, has_tail) = (r.read_bit(), r.read_bit());
    if roster_bound || !has_tail {
        return None;
    }
    r.skip_bits(CHARACTER_FIXED_BITS - 10);
    let _rank = r.read_u8();
    let team = r.read_u8();
    let kind = r.read_u8();
    let _health = r.read_u32();
    let caption = String::from_utf8_lossy(&r.read_zstr8()).into_owned();
    Some(NpcCharacter {
        team,
        kind,
        caption,
    })
}

/// The characters drawn as remote bodies: (object, pid, name, team) of every roster player's character but the own
/// one, then every AI character of the object table (CLI-1857: no roster record; its caption is the name, the NPC
/// tail's team its team).
// DC-DEFAULT: CLI-1857 (proposed)
pub fn drawn_characters(
    s: &BotSession,
    own_obj: Option<u16>,
    own_pid: Option<u16>,
) -> Vec<(u16, u8, String, u8)> {
    let mut chars: Vec<(u16, u8, String, u8)> = s
        .obs
        .roster
        .values()
        .filter(|r| {
            Some(r.character) != own_obj && r.character != 0xFFFF && Some(r.pid as u16) != own_pid
        })
        .map(|r| (r.character, r.pid, r.name.clone(), r.team))
        .collect();
    let npcs: Vec<(u16, u8, String, u8)> = s
        .table
        .objects
        .iter()
        .filter(|(obj, _)| Some(**obj) != own_obj && !chars.iter().any(|c| c.0 == **obj))
        .filter_map(|(&obj, o)| {
            let n = npc_character(o)?;
            Some((obj, NPC_PID, n.caption, n.team))
        })
        .collect();
    chars.extend(npcs);
    chars
}

/// The thrown objects of the session: objects whose create blob is the thrown-object type, with their model block,
/// position and rotation (no box, no trackers).
// SPEC: UDP_SESSION_2009_B.md §3.5 (kind-1 object with a model block; thrown object = type 0x13 create blob)
// EVIDENCE: cshell+0x2ca820 / +0x2cab60 (the model is the server object's own model block)
fn thrown_bodies(s: &BotSession) -> Vec<RemoteBody> {
    let name = |id: u16| -> Option<String> {
        (id != 0xFFFF)
            .then(|| s.obs.files.get(&id).map(|f| f.1.clone()))
            .flatten()
    };
    s.table
        .objects
        .iter()
        .filter(|(_, o)| o.blob_type == Some(crate::fx::THROWN_BLOB))
        .filter_map(|(&obj, o)| {
            let pos = o.pos?;
            let models: Vec<ModelRef> = o
                .models
                .iter()
                .filter_map(|e| {
                    Some(ModelRef {
                        model: name(e.model)?,
                        textures: e
                            .textures
                            .iter()
                            .map(|t| name(*t).unwrap_or_default())
                            .collect(),
                    })
                })
                .collect();
            if models.is_empty() {
                return None;
            }
            let yaw = o.rot.map(|q| {
                let q = glam::Quat::from_xyzw(q[0], q[1], q[2], q[3]);
                let f = q * glam::Vec3::Z;
                f.x.atan2(f.z)
            });
            Some(RemoteBody {
                obj,
                pid: 0xFF,
                name: String::new(),
                team: 0,
                pos,
                half: [0.0; 3],
                yaw,
                look: Some(RemoteLook {
                    styles: Vec::new(),
                    models,
                    weapon: None,
                    trackers: Vec::new(),
                    visible: true,
                    aim: None,
                }),
            })
        })
        .collect()
}

/// The running match's game socket and server address for [`last_gasp`], with the owner's token (the newest network
/// thread owns it; a finished thread clears only its own entry).
// DC-DEFAULT: CLI-1702 (proposed, project decision)
static GASP: Mutex<Option<(u64, UdpSocket, SocketAddr)>> = Mutex::new(None);
static GASP_TOKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn gasp_set(token: u64, entry: Option<(UdpSocket, SocketAddr)>) {
    let mut g = GASP.lock().unwrap_or_else(|p| p.into_inner());
    match entry {
        Some((s, a)) => *g = Some((token, s, a)),
        None => {
            if g.as_ref().is_some_and(|e| e.0 == token) {
                *g = None;
            }
        }
    }
}

fn gasp_peer(token: u64, peer: SocketAddr) {
    let mut g = GASP.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(e) = g.as_mut().filter(|e| e.0 == token) {
        e.2 = peer;
    }
}

/// The last word of a client that is about to end (the frame watchdog's abort, a lost GPU device that did not come
/// back): the plain transport bye twice, straight on the match's socket, from any thread and without the session's
/// lock, so the game server closes the player's link at once (the normal leave) instead of seeing a silent client.
/// Returns whether a bye went out (false outside a match or when the entry is busy).
// SPEC: UDP_TRANSPORT_2009.md §9.1 (the plain CTL_BYE; the host closes the link on it)
// DC-DEFAULT: CLI-1702 (proposed, project decision) a plain bye before the process ends itself in a match
pub fn last_gasp() -> bool {
    let g = match GASP.try_lock() {
        Ok(g) => g,
        Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return false,
    };
    let Some((_, sock, to)) = g.as_ref() else {
        return false;
    };
    let d = dc_wire::udp::encode_bye_datagram(None);
    let mut sent = false;
    for _ in 0..2 {
        sent |= sock.send_to(&d, to).is_ok();
    }
    sent
}

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    // a panic on the other thread must not take the client down with it
    shared.lock().unwrap_or_else(|p| p.into_inner())
}

/// The session's start configuration for a client with its own player and world load. `mode_class` is the room's
/// mode class from the ROM mode row (the server's 0x9e takes precedence when it arrives): it selects the client's own
/// respawn wait after a death (7000 ms and then 0x89 in TDM) the way the 2009 client arms it.
// SPEC: GAME_MESSAGES_2009_C2S.md §3.3, §3.4, Appendix A step A0 (mode class from the room's mode row)
// DISASM: cshell+0x60ac0 (the client arms its own wait after 0x67 state 2), see CAPTURE_tdm4_KILL_VALIDATION.md §4
pub fn session_config(
    opts: &NetOptions,
    nick: &str,
    rom: Option<&RomWorlds>,
    mode_class: Option<u8>,
) -> GameConfig {
    let mut gc = GameConfig::new(nick);
    gc.hello_tag = opts.user();
    gc.external_body = true;
    gc.external_load = true;
    gc.queue_events = true;
    // the client's own load is the load time (the session holds its messages until the world is reported)
    // DC-DEFAULT: CLI-17 (proposed) no simulated load time on top of the client's real world load
    gc.load_ms = 0;
    gc.update_hz = opts.update_hz;
    gc.integrity_version = rom.and_then(RomWorlds::integrity_version);
    // the respawn wait is the client's own (no fallback timer: a 2009 client asks only after its armed wait)
    // DC-DEFAULT: CLI-21 (proposed) own respawn wait by the room's ROM mode class, reliable state form [0x0010][1]
    gc.respawn_after_ms = None;
    gc.mode_class_hint = mode_class;
    // the client leaves the result screen itself (net_main, CLI-88), the 2009 way: lobby 0x306 {0xb} and the plain
    // transport bye, no `C2S_BYE` (the bot's BOT-16 leave sends `C2S_BYE` 3 s after 0x3f)
    gc.leave_after_result = false;
    // the in-game frame ends at 0x3f: no player update, no respawn request, no roster query after it
    // DISASM: cshell+0x4ac5c (0x3f leaves the playing state); OBSERVED: rust6 / rust7 / realclient (no 0x64 / 0x89
    // after 0x3f), see dc_botclient::session::GameConfig::frame_ends_at_result
    gc.frame_ends_at_result = true;
    gc.state_form = Some(STATE_FORM_VALUE);
    // DC-DEFAULT: CLI-22 (proposed) roster queries as the 2009 client sends them
    gc.roster_query = true;
    gc
}

/// The lobby connection during a match: read (and recorded) on its own thread, with an outbox for the frames the
/// client sends while it plays (the leave notice 0x394). The lobby connection stays up during a match.
// SPEC: PROTOCOL_2009.md §1.1 (the lobby connection stays up, LIVE), §3.7 (0x394)
pub struct LobbyKeeper {
    tx: Sender<PktWriter>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<Lobby>>,
}

impl LobbyKeeper {
    pub fn start(lobby: Lobby) -> LobbyKeeper {
        let (tx, rx): (Sender<PktWriter>, Receiver<PktWriter>) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let mut me = lobby;
        let handle = std::thread::Builder::new()
            .name("dc-client lobby".into())
            .spawn(move || {
                while !s2.load(Ordering::Relaxed) {
                    while let Ok(w) = rx.try_recv() {
                        let _ = me.link.send(&w);
                    }
                    if me.link.pump(Duration::from_millis(20)).is_err() || me.link.is_closed() {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
                while let Ok(w) = rx.try_recv() {
                    let _ = me.link.send(&w);
                }
                me
            })
            .ok();
        LobbyKeeper { tx, stop, handle }
    }

    /// Queues one frame for the lobby connection (sent within about 20 ms).
    pub fn send(&self, w: PktWriter) -> bool {
        self.tx.send(w).is_ok()
    }

    /// Stops the thread (after the queued frames went out) and returns the lobby.
    pub fn finish(mut self) -> Option<Lobby> {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.take().and_then(|h| h.join().ok())
    }
}

impl Drop for LobbyKeeper {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The lobby leave notice 0x394 `[phase:u32 = 0]`.
// SPEC: PROTOCOL_2009.md §3.7; OBSERVED: CAPTURE_tdm4_KILL_VALIDATION.md §1 (body `94030a00000000000000`)
pub fn leave_notice() -> PktWriter {
    let mut w = PktWriter::new(dc_wire::MATCH_EXIT);
    w.u32(LEAVE_NOTICE_PHASE);
    w
}

/// The lobby location report 0x306 `[where:u32]` (0xb when the client leaves the result screen).
// SPEC: PROTOCOL_2009.md §3 row 0x306 (`location`, +6 u32 where, no reply)
// OBSERVED: capture 20261025_153705_rust7 (frame `06 03 0a 00 00 00 0b 00 00 00` at 720.016)
pub fn location_notice(place: u32) -> PktWriter {
    let mut w = PktWriter::new(dc_wire::msg_ids::LOCATION);
    w.u32(place);
    w
}

/// The sprint state 0xca `[sprinting:bool1][stopped:bool1]`.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.3; OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §1 (18 bits, `0b ca 02` / `0b ca 01`)
pub fn sprint_body(pair: (bool, bool)) -> BitWriter {
    let mut b = BitWriter::new();
    b.write_bit(pair.0).write_bit(pair.1);
    b
}

/// The movement notice 0x76 `[1][event]`.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.2; OBSERVED: CAPTURE_tdm1_GAME_VALIDATION.md §2.3 (event 1 take-off, 2 fall
// without a jump, 3 landing)
pub fn move_notice_body(event: u8) -> BitWriter {
    let mut b = BitWriter::new();
    b.write_u8(MOVE_EVENT_CATEGORY).write_u8(event);
    b
}

fn set_phase(shared: &Mutex<Shared>, p: NetPhase) {
    lock(shared).phase = p;
}

/// The lobby part on the network thread: auth, lobby login, room, start / ready, match address.
// SPEC: PROTOCOL_2009.md §2.1, §3.1, §3.7, §7; HARNESS: lobby_harness::login, ::lobby, ::test_main_suite
fn lobby_flow(
    shared: &Mutex<Shared>,
    opts: &NetOptions,
    rec: &Recorder,
    rom: Option<&RomWorlds>,
    rom_dir: Option<&Path>,
) -> Result<(SocketAddr, GameConfig, LobbyKeeper, Vec<i32>), String> {
    let Connect::Lobby {
        auth,
        room,
        password,
        channel,
    } = &opts.connect
    else {
        return Err("not a lobby connection".into());
    };
    set_phase(shared, NetPhase::Lobby("auth".into()));
    let user = opts.user();
    let mut lo = LoginOptions::new(*auth, &user, &opts.name);
    lo.timeout = opts.lobby_timeout;
    let ticket = auth_login(&lo, rec).map_err(|e| e.0)?;
    set_phase(shared, NetPhase::Lobby("lobby login".into()));
    let mut lobby = Lobby::login(&ticket, *channel, rec, opts.lobby_timeout).map_err(|e| e.0)?;
    match room {
        RoomChoice::New {
            max_players,
            title,
            mode,
            start_after,
        } => {
            set_phase(
                shared,
                NetPhase::Lobby("room created, waiting for players".into()),
            );
            lobby
                .open_room(*max_players, title, password, *mode)
                .map_err(|e| e.0)?;
            // the goal before the guests ready up: a settings change clears their ready state (RM-1)
            if let Some(g) = opts.goal {
                hostroom::set_goal(&mut lobby, *mode, g, rom_dir, opts.lobby_timeout, rec)
                    .map_err(|e| e.0)?;
            }
            lobby
                .wait_members(*start_after, opts.match_wait)
                .map_err(|e| e.0)?;
            lobby.start().map_err(|e| e.0)?;
        }
        RoomChoice::Number(n) => {
            lobby.join_room_no(*n, password).map_err(|e| e.0)?;
            lobby.ready().map_err(|e| e.0)?;
        }
        RoomChoice::Any => {
            set_phase(shared, NetPhase::Lobby("looking for a room".into()));
            let n = lobby.find_room(opts.match_wait).map_err(|e| e.0)?;
            lobby.join_room_no(n, password).map_err(|e| e.0)?;
            lobby.ready().map_err(|e| e.0)?;
        }
    }
    set_phase(
        shared,
        NetPhase::Lobby("waiting for the game server".into()),
    );
    let addr = lobby.wait_match_address(opts.match_wait).map_err(|e| e.0)?;
    let server = addr.socket_addr().map_err(|e| e.0)?;
    let mode_row = lobby.room.as_ref().map_or(0, |r| r.mode);
    let mode_class = rom_dir.and_then(|d| hostroom::rom_mode_class(d, mode_row.max(0) as usize));
    rec.event(
        "mode_class_hint",
        serde_json::json!({"mode_row": mode_row, "mode_class": mode_class}),
    );
    let mut gc = session_config(opts, &lobby.profile.nick, rom, mode_class);
    gc.info = player_info_from_lobby(&lobby);
    gc.seat_ticket = addr.ticket;
    // the equipment slots 0x323 +0x12: weapon slot i = equipment slot i (the weapons' fallback without grants)
    // SPEC: PROTOCOL_2009.md §3.3; OBSERVED: CAPTURE_tdm4_WEAPON_FORMS.md §2 (slots 0..3 = the granted weapons)
    let slots = lobby.profile.equipped.clone();
    Ok((server, gc, LobbyKeeper::start(lobby), slots))
}

/// The environment variable (and `--netlab`) naming the network lab path of the match socket (docs/NETLAB.md).
// DC-DEFAULT: NET-20 (proposed) network lab impairment shim for a human tester
pub const NETLAB_ENV: &str = "DC_NETLAB";

/// The tester's network lab path from `DC_NETLAB` (`None`: unset, empty or `off`; a bad value is logged and ignored).
fn netlab_spec(rec: &Recorder) -> Option<PathSpec> {
    let v = std::env::var(NETLAB_ENV).ok()?;
    let v = v.trim();
    if v.is_empty() || v.eq_ignore_ascii_case("off") {
        return None;
    }
    match PathSpec::parse(v) {
        Ok(s) => {
            eprintln!("dc-client: network lab path {v:?} on the match socket ({NETLAB_ENV})");
            rec.event("netlab_path", serde_json::json!({"profile": s.name, "spec": v}));
            Some(s)
        }
        Err(e) => {
            eprintln!("dc-client: {NETLAB_ENV}={v:?} ignored: {e}");
            None
        }
    }
}

/// CLI-1611: applies the server set's weapon content to a match's weapon table. `Ok(false)`: nothing to apply
/// (vanilla, or a set without weapon content). A refused document (the client's ROM table differs from the server's;
/// the lobby already compared the content hashes, LG-271) leaves the table as it was and is reported.
// DC-DEFAULT: CLI-1611 (proposed)
pub(crate) fn apply_set_content(
    table: &mut WeaponTable,
    content: Option<&dc_bute::modpatch::WeaponContent>,
) -> Result<bool, String> {
    crate::camo::set_content(content);
    let Some(c) = content.filter(|c| !c.is_empty()) else {
        return Ok(false);
    };
    table
        .apply_content(c)
        .map(|()| true)
        .map_err(|e| format!("the server's weapon content does not fit this ROM: {e}"))
}

/// The network thread: lobby (when asked), then the UDP session until the server ends it or the client quits.
fn net_main(shared: Arc<Mutex<Shared>>, opts: NetOptions) {
    let rec = match Recorder::new(opts.capture.as_deref(), opts.log.as_deref(), &opts.name) {
        Ok(r) => r,
        Err(_) => Recorder::null(&opts.name),
    };
    let rom_dir = opts.rom.clone().or_else(|| dc_rom::resolve_rom().ok());
    let rom = rom_dir.as_deref().and_then(|r| RomWorlds::open(r).ok());
    // the ROM weapon table (rows, first-person models) for the weapons; without it the fallback timings apply
    let table = rom_dir
        .as_deref()
        .and_then(|r| WeaponTable::open(r).ok())
        // CLI-1611: the server set's weapon content over the ROM table (the game servers apply the same document)
        .map(|mut t| {
            match apply_set_content(&mut t, opts.content.as_deref()) {
                Ok(false) => {}
                Ok(true) => rec.event("weapon_content", serde_json::json!({"applied": true})),
                Err(e) => {
                    eprintln!("dc-client: {e}");
                    rec.event("weapon_content", serde_json::json!({"applied": false, "error": e}));
                }
            }
            t
        })
        .map(Arc::new);
    rec.event(
        "weapon_table",
        serde_json::json!({"rows": table.as_ref().map(|t| t.rows.len())}),
    );
    let (server, gc, drain, lobby_slots) = match &opts.connect {
        // a direct join knows no room mode: the server's 0x9e (when it sends one) gives the mode class
        Connect::Udp(a) => (
            *a,
            session_config(&opts, &opts.name, rom.as_ref(), None),
            None,
            opts.lobby_slots.clone(),
        ),
        Connect::Lobby { .. } => {
            let handed = opts.handoff.as_ref().and_then(|h| h.take_ready());
            let flow = match handed {
                Some((server, lobby, mode)) => {
                    let (g, d, slots) = crate::menu::handoff::session_from(
                        &opts,
                        &rec,
                        rom.as_ref(),
                        rom_dir.as_deref(),
                        lobby,
                        mode,
                    );
                    Ok((server, g, d, slots))
                }
                None => lobby_flow(&shared, &opts, &rec, rom.as_ref(), rom_dir.as_deref()),
            };
            match flow {
                Ok((s, g, d, slots)) => (s, g, Some(d), slots),
                Err(e) => {
                    set_phase(&shared, NetPhase::Failed(format!("lobby: {e}")));
                    return;
                }
            }
        }
    };
    lock(&shared).equipment = lobby_slots.clone();
    let weapon_types = limit_types(opts.arms_limit, table.as_deref(), rom_dir.as_deref());
    lock(&shared).arsenal = Arsenal::new(ArmsConfig {
        table,
        rom: rom_dir.clone(),
        lobby_slots,
        seed: opts.arms_seed,
        reality: opts.reality,
        arms_limit: opts.arms_limit,
        weapon_types,
        no_recoil: opts.room_hacks & dc_wire::roomhacks::NO_RECOIL != 0,
    });
    set_phase(&shared, NetPhase::Joining);
    let bind = match server.ip() {
        IpAddr::V4(ip) if ip.is_loopback() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        _ => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
    };
    // NET-20: a tester's bad network (`--netlab` / `DC_NETLAB`, docs/NETLAB.md): the match socket runs through the
    // network lab's shim, read by its own thread (the plain socket clone below is only the last gasp's sender)
    let mut shim = match netlab_spec(&rec) {
        Some(spec) => match NetShim::bind(bind, RECV_BUFFER_BYTES, Some(spec)) {
            Ok(s) => Some(s),
            Err(e) => {
                set_phase(&shared, NetPhase::Failed(format!("udp bind: {e}")));
                return;
            }
        },
        None => None,
    };
    let sock = match shim
        .as_ref()
        .map(NetShim::try_clone_socket)
        .unwrap_or_else(|| UdpSocket::bind(SocketAddr::new(bind, 0)))
    {
        Ok(s) => s,
        Err(e) => {
            set_phase(&shared, NetPhase::Failed(format!("udp bind: {e}")));
            return;
        }
    };
    if shim.is_none() {
        let _ = sock.set_nonblocking(true);
    }
    let gasp = GASP_TOKEN.fetch_add(1, Ordering::Relaxed) + 1;
    gasp_set(gasp, sock.try_clone().ok().map(|c| (c, server)));
    // the game link's datagrams go into the capture too (when one is asked for), so the client's own sessions can
    // be read with the capture tools
    let conn = rec.open_conn("game", "udp", &server.to_string());
    lock(&shared).session = Some(BotSession::new(gc, None, rec.clone()));
    let mut peer: Option<SocketAddr> = None;
    lock(&shared).game_addr = Some(server);
    // CLI-1571: datagrams sent since the meter last saw them (count, bytes); the send happens outside the lock
    let mut sent_unmetered: (u32, u64) = (0, 0);
    let mut buf = vec![0u8; RECV_BUFFER_BYTES];
    let mut quit_at: Option<u32> = None;
    // when the client sent the result screen's 0x306 {0xb}: the menu's result screen counts its 60 s from there
    let mut result_left: Option<Instant> = None;
    let mut over_at: Option<u32> = None;
    let mut bye_due: Option<u32> = None;
    // DC-DEFAULT: CLI-1331 (proposed) one `remote_interp` event-log line per remote player every 10 s
    // (started when the first remote body appears: the first window does not span the lobby and the loading)
    let mut interp_at: Option<u32> = None;
    let mut interp_lines: Vec<serde_json::Value> = Vec::new();
    loop {
        let (out, now, stop, over, leave_due) = {
            let mut g = lock(&shared);
            let now = g.now();
            if interp_at.is_none() && !g.tracks.is_empty() {
                interp_at = Some(now);
            }
            if interp_at.is_some_and(|a| now.wrapping_sub(a) >= INTERP_REPORT_MS) {
                let w = now.wrapping_sub(interp_at.unwrap_or(now));
                interp_at = Some(now);
                interp_lines = g
                    .tracks
                    .values_mut()
                    .filter(|t| t.window.samples > 0 || t.window.frames > 0)
                    .map(|t| t.take_report(now, w))
                    .collect();
            }
            let mut queued = std::mem::take(&mut g.outgoing);
            // the frames for the lobby connection the session keeps during the match
            let lobby_out = std::mem::take(&mut g.lobby_out);
            if let Some(k) = drain.as_ref() {
                for w in lobby_out {
                    k.send(w);
                }
            }
            // nothing of the in-game frame goes out after 0x3f (matchexit.rs)
            if g.result_at.is_some() {
                queued.clear();
            }
            let mut sent = 0;
            if let Some(s) = g.session.as_mut() {
                for (shell, body) in queued {
                    let ok = s.send_game_message(now, true, shell, &body);
                    sent += (ok && shell == UP_MOVE_EVENT) as u64;
                }
            }
            g.notices_sent += sent;
            let out = g.session.as_mut().map(|s| s.poll(now)).unwrap_or_default();
            g.absorb(now);
            // CLI-1571: the overlay's meter takes the transport's counters and the stream's time entries
            {
                let Shared { session, meter, .. } = &mut *g;
                if sent_unmetered.0 > 0 {
                    meter.sent(now, sent_unmetered.0, sent_unmetered.1);
                    sent_unmetered = (0, 0);
                }
                if let Some(s) = session.as_ref() {
                    if let Some(l) = s.link() {
                        let st = l.stats();
                        let (rtt_total, newest) = l.latency().newest();
                        meter.link(
                            now,
                            meter::LinkTotals {
                                rel_out: st.rel_blocks_out,
                                resends: st.resends,
                                seq_span: st.rel_seq_span,
                                seq_gaps: st.rel_seq_gaps,
                                rtt_total,
                            },
                            newest,
                        );
                    }
                    meter.time_entries(now, s.table.time_entries);
                }
            }
            let over = g.session.as_ref().is_none_or(BotSession::is_over);
            let leave_due = match (opts.leave_after, g.in_world_at) {
                (Some(d), Some(t)) => now.wrapping_sub(t) as u128 >= d.as_millis(),
                _ => false,
            };
            (out, now, g.stop, over, leave_due)
        };
        for v in interp_lines.drain(..) {
            rec.event("remote_interp", v);
        }
        for d in out {
            sent_unmetered.0 += 1;
            sent_unmetered.1 += d.len() as u64;
            match shim.as_mut() {
                Some(s) => s.send_to(&d, peer.unwrap_or(server)),
                None => {
                    let _ = sock.send_to(&d, peer.unwrap_or(server));
                }
            }
            rec.udp_dgram(conn, "c2s", &d, describe_datagram(&d));
        }
        let mut raw: Vec<(Vec<u8>, SocketAddr)> = Vec::new();
        match shim.as_mut() {
            Some(s) => {
                while raw.len() < 64 {
                    match s.try_recv() {
                        Some(Ok(x)) => raw.push(x),
                        _ => break,
                    }
                }
            }
            None => loop {
                match sock.recv_from(&mut buf) {
                    Ok((n, from)) => raw.push((buf[..n].to_vec(), from)),
                    Err(e) => match e.kind() {
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => break,
                        // SPEC: UDP_TRANSPORT_2009.md §4 (WSAECONNRESET from an ICMP port-unreachable is ignored)
                        std::io::ErrorKind::ConnectionReset => continue,
                        _ => break,
                    },
                }
                if raw.len() >= 64 {
                    break;
                }
            },
        }
        let mut got: Vec<Vec<u8>> = Vec::new();
        for (d, from) in raw {
            if peer.is_some_and(|p| p != from) {
                continue;
            }
            if peer.is_none() && !is_oob_datagram(&d) && from != server {
                // before the join only the server's address is accepted for link datagrams
                continue;
            }
            if peer.is_none() {
                // SPEC: UDP_TRANSPORT_2009.md §5.2 (the join reply's source becomes the peer)
                peer = Some(from);
                gasp_peer(gasp, from);
            }
            rec.udp_dgram(conn, "s2c", &d, describe_datagram(&d));
            got.push(d);
        }
        if got.is_empty() {
            std::thread::sleep(Duration::from_millis(POLL_WAIT_MS));
        } else {
            let mut g = lock(&shared);
            let now = g.now();
            for d in &got {
                g.meter.received(now, d.len());
            }
            if let Some(s) = g.session.as_mut() {
                for d in &got {
                    s.handle_datagram(now, d);
                }
            }
        }
        if over {
            let at = *over_at.get_or_insert(now);
            // the server closed the link after our C2S_BYE: the session's link sends the plain bye x4, 100 ms
            // apart (BotSession::poll -> Link::send_plain_bye), as the 2009 client does when it leaves the result screen
            // OBSERVED: CAPTURE_tdm1_TRANSPORT_VALIDATION.md C-T12 (`c9 03` x4 after the host's bye code 5)
            // DC-DEFAULT: CLI-24 (proposed) own plain bye x4 right after the host's bye
            if now.wrapping_sub(at) > BYE_FLUSH_MS {
                break;
            }
            continue;
        }
        // the result screen: after the hold the client leaves it the 2009 way (lobby 0x306 {0xb} and the plain
        // transport bye in the same moment; no game message, no `C2S_BYE`)
        // DC-DEFAULT: CLI-88 (proposed) result screen hold and leave form
        if quit_at.is_none() && bye_due.is_none() {
            let mut g = lock(&shared);
            let due = g.result_left_at.is_none()
                && g.result_at
                    .is_some_and(|t| now.wrapping_sub(t) as u128 >= opts.result_hold.as_millis());
            if due {
                if let Some(k) = drain.as_ref() {
                    k.send(location_notice(RESULT_LOCATION));
                    result_left = Some(Instant::now());
                }
                let plain = g.session.as_mut().is_some_and(|s| s.leave_plain(now));
                if !plain {
                    if let Some(s) = g.session.as_mut() {
                        s.quit(now);
                    }
                }
                g.result_left_at = Some(now);
                g.leave_kind.get_or_insert_with(|| "result".into());
                quit_at = Some(now);
            }
        }
        if (stop || leave_due) && quit_at.is_none() && bye_due.is_none() {
            // a player in a running match leaves through the leave box: 0xbd and the lobby's 0x394 {0} in the same
            // moment, the engine disconnect one frame later; otherwise (still joining or loading) only C2S_BYE
            // DC-DEFAULT: CLI-23 (proposed) leave sequence
            let mut g = lock(&shared);
            let in_match = g
                .session
                .as_ref()
                .is_some_and(|s| s.stage() == Stage::InWorld && !s.obs.reached("result_screen"));
            let left = in_match && g.session.as_mut().is_some_and(|s| s.leave_match(now));
            if left {
                g.leave_kind = Some("leave_match".into());
                if let Some(k) = drain.as_ref() {
                    g.leave_notice_sent = k.send(leave_notice());
                }
                bye_due = Some(now.wrapping_add(LEAVE_BYE_DELAY_MS));
            } else {
                if g.leave_kind.is_none() {
                    g.leave_kind = Some("quit".into());
                }
                if let Some(s) = g.session.as_mut() {
                    s.quit(now);
                }
                quit_at = Some(now);
            }
        }
        // the bye is due once `now` reached `bye_due` (wrapping millisecond clock)
        if bye_due.is_some_and(|t| now.wrapping_sub(t) < u32::MAX / 2) {
            if let Some(s) = lock(&shared).session.as_mut() {
                s.quit(now);
            }
            bye_due = None;
            quit_at = Some(now);
        }
        if quit_at.is_some_and(|q| now.wrapping_sub(q) > QUIT_GRACE_MS) {
            break;
        }
    }
    gasp_set(gasp, None);
    if let Some((profile, up, down, path)) = shim.as_ref().and_then(NetShim::summary) {
        let dir = |d: &dc_net_engine::impair::DirStats| {
            serde_json::Value::Object(
                d.fields()
                    .iter()
                    .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
                    .collect(),
            )
        };
        rec.event(
            "impairment",
            serde_json::json!({"profile": profile, "up": dir(&up), "down": dir(&down),
                   "blackouts": path.blackouts, "rebinds": path.rebinds, "nat_expiries": path.nat_expiries,
                   "flow_kills": path.flow_kills}),
        );
    }
    {
        let mut g = lock(&shared);
        let now = g.now();
        g.absorb(now);
        if !g.phase.is_final() {
            g.phase = NetPhase::Ended("client quit".into());
        }
    }
    rec.flush();
    // the session logs' writer counters (perf-hitch: lines dropped by a full queue, queue high-water marks)
    let logs = rec.log_stats();
    if !logs.is_empty() {
        println!("dc-client net logs: {logs}");
    }
    if let Some(d) = drain {
        let back = d.finish();
        if let (Some(h), Some(l)) = (opts.handoff.as_ref(), back) {
            let result = lock(&shared).match_result();
            h.set_result(result.map(|r| match result_left {
                Some(at) => r.filled(at),
                None => r,
            }));
            h.give_back(l);
        }
    }
}

/// Name key of the client's step-height variable: first 8 bytes of SHA-256 of the name, and its length. The name
/// itself is a client string and stays in the user's ROM; the server sends it.
// SPEC: MOVEMENT_2009.md §2 (`STEP_HEIGHT`, key eda10d3a6476dd56/15)
pub const STEP_HEIGHT_KEY: ([u8; 8], usize) =
    ([0xed, 0xa1, 0x0d, 0x3a, 0x64, 0x76, 0xdd, 0x56], 15);

/// The step height of an `S2C_EVAR` pair when the name is the step-height variable and the value text a finite
/// number (`30.000000` in the 2009 form).
// SPEC: UDP_SESSION_2009_A.md §4.5 (`S2C_EVAR` `[name:zstr8][value:zstr8]`); MOVEMENT_2009.md §2, §8.1 step 2
// OBSERVED: capture 20261025_002236_tdm4 (value text `30.000000` before `S2C_LOAD_WORLD`)
pub fn step_height_of(name: &str, value: &str) -> Option<f32> {
    let n = name.as_bytes();
    if n.len() != STEP_HEIGHT_KEY.1 || dc_world::hash16(n) != STEP_HEIGHT_KEY.0 {
        return None;
    }
    value.trim().parse::<f32>().ok().filter(|v| v.is_finite())
}

/// Handle to a running network session (cheap to clone; the window takes the [`NetWorld`] itself).
#[derive(Clone)]
pub struct NetHandle {
    shared: Arc<Mutex<Shared>>,
    thread: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl NetHandle {
    pub fn phase(&self) -> NetPhase {
        lock(&self.shared).phase.clone()
    }

    pub fn stats(&self) -> NetStats {
        lock(&self.shared).stats()
    }

    /// The announced world (`Some(Ok)`), the end of the session before one (`Some(Err)`), or `None` while still
    /// joining: the non-blocking form of [`NetHandle::wait_world`] (the loading screen polls it every frame).
    pub fn world_ready(&self) -> Option<Result<String, ClientError>> {
        let g = lock(&self.shared);
        if let Some(w) = g.session.as_ref().and_then(|s| s.world_request()) {
            return Some(Ok(w.to_string()));
        }
        g.phase
            .is_final()
            .then(|| Err(ClientError::Net(g.phase.label())))
    }

    /// The loading screen's load stage ([`crate::loading::stage`]).
    pub fn load_stage(&self) -> u8 {
        lock(&self.shared).load_stage
    }

    /// CLI-1840: the link's numbers for a diagnostic report: the session's totals (datagrams, resends, the largest
    /// datagram, the smoothed round trip) and the network overlay's last 5 s, the game server, the world, the phase.
    pub fn diag_link(&self) -> serde_json::Value {
        let mut g = lock(&self.shared);
        let now = g.now();
        let v = g.meter.view(now);
        let mut out = serde_json::json!({
            "game_server": g.game_addr.map(|a| a.to_string()),
            "world": g.world,
            "phase": g.phase.label(),
            "left": g.leave_kind,
            "last5s": {
                "rtt_mean": v.rtt_mean, "rtt_min": v.rtt_min, "rtt_max": v.rtt_max, "jitter": v.jitter,
                "loss_in_pct": v.loss_in, "loss_out_pct": v.loss_out, "since_update_ms": v.since_update,
                "tick_gap_max_ms": v.tick_gap_max,
            },
        });
        if let Some(l) = g.session.as_ref().and_then(|s| s.link()) {
            let st = l.stats();
            out["totals"] = serde_json::json!({
                "rtt_ms": l.latency().l_ms(),
                "datagrams_out": st.datagrams_out, "datagrams_in": st.datagrams_in,
                "reliable_out": st.rel_blocks_out, "resends": st.resends, "bad_check": st.bad_check,
                "flow_drops": st.flow_drops, "abandoned": st.rel_abandoned,
                "max_datagram_bytes": st.max_datagram_bytes, "oversize_out": st.oversize_out,
                "seq_span_in": st.rel_seq_span, "seq_gaps_in": st.rel_seq_gaps,
            });
        }
        out
    }

    /// How the session ended, for the waiting room's disconnect box ([`crate::loading::SessionEnd`]).
    pub fn session_end(&self) -> crate::loading::SessionEnd {
        use crate::loading::SessionEnd;
        let g = lock(&self.shared);
        let Some(s) = g.session.as_ref() else {
            return match &g.phase {
                NetPhase::Failed(_) => SessionEnd::JoinFailed,
                _ => SessionEnd::Played,
            };
        };
        // the client left itself (its leave box, the result screen) or saw the result: no box
        if g.leave_kind.is_some() || g.result_at.is_some() {
            return SessionEnd::Played;
        }
        match s.stage() {
            Stage::Failed(_) => SessionEnd::JoinFailed,
            Stage::Dropped(_) => SessionEnd::Dropped,
            Stage::Closed => match s.obs.server_bye {
                Some(code) => SessionEnd::ServerBye(code),
                None => SessionEnd::Played,
            },
            _ => SessionEnd::Played,
        }
    }

    /// Waits until the server announced its world (returns its name) or the session failed.
    pub fn wait_world(&self, timeout: Duration) -> Result<String, ClientError> {
        let end = Instant::now() + timeout;
        loop {
            if let Some(r) = self.world_ready() {
                return r;
            }
            if Instant::now() >= end {
                return Err(ClientError::Net(format!(
                    "no world announced within {timeout:?} ({})",
                    self.phase().label()
                )));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Asks the session to end (`C2S_BYE`), waits up to `wait` for the network thread, and returns the statistics.
    pub fn shutdown(&self, wait: Duration) -> NetStats {
        lock(&self.shared).stop = true;
        let end = Instant::now() + wait;
        let handle = self.thread.lock().ok().and_then(|mut t| t.take());
        if let Some(h) = handle {
            while !h.is_finished() && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(10));
            }
            if h.is_finished() {
                let _ = h.join();
            }
        }
        self.stats()
    }
}

/// The world source of a network session.
pub struct NetWorld {
    opts: NetOptions,
    handle: NetHandle,
    /// The characters' node spheres for the shot test (CLI-300).
    hit_rigs: crate::hitnodes::HitRigs,
    /// The world path the server announced, once it was loaded (gpu-recover: the scene is loaded again from it).
    announced: Option<String>,
}

impl NetWorld {
    /// Starts the network thread (lobby flow or direct join) and returns at once.
    pub fn start(opts: NetOptions) -> NetWorld {
        let shared = Arc::new(Mutex::new(Shared {
            start: Instant::now(),
            phase: NetPhase::Starting,
            session: None,
            tracks: BTreeMap::new(),
            left_tracks: BTreeMap::new(),
            hud: Hud::default(),
            corrections: Vec::new(),
            view_sets: Vec::new(),
            player_dims: None,
            corrections_taken: 0,
            kills: Vec::new(),
            stop: false,
            glitch_until: None,
            glitch_done: false,
            glitch_sent: false,
            last_seq_reported: None,
            world: None,
            outgoing: Vec::new(),
            lobby_out: Vec::new(),
            sprint_pair: None,
            notices_sent: 0,
            in_world_at: None,
            leave_kind: None,
            leave_notice_sent: false,
            arsenal: Arsenal::new(ArmsConfig::default()),
            equipment: Vec::new(),
            thrown: crate::thrown::ThrownMotion::default(),
            grants: Vec::new(),
            damage_taken: 0,
            own_hit_records: 0,
            predicted_hits: 0,
            corpses: Corpses::default(),
            spawn_shield: Default::default(),
            corpse_skips: 0,
            anim_clocks: crate::anims::AnimClocks::default(),
            hud_events: Vec::new(),
            panel_selects: 0,
            hud_view_object: None,
            case_countdown: BTreeMap::new(),
            cam_fovs: crate::camshot::CameraFovs::default(),
            hud_notes: Vec::new(),
            hud_roster: BTreeMap::new(),
            hud_own: (None, None),
            zero_health_logged: false,
            step_height: None,
            evars_scanned: 0,
            sound_cues: Vec::new(),
            fx_events: Vec::new(),
            own_track: None,
            world_group_done: false,
            preload_seen: 0,
            ladders_seen: 0,
            death_at: None,
            last_killer: None,
            last_shot_dir: None,
            mover_notes: Vec::new(),
            mover_count: 0,
            result_at: None,
            result_left_at: None,
            drops: crate::drops::DropTracker::default(),
            use_entries: Default::default(),
            use_target: None,
            item_target: false,
            bomb_hold: None,
            hold_sound: None,
            billboards: crate::sprites::BillboardTracker::default(),
            billboard_list: Vec::new(),
            remote_mods: BTreeMap::new(),
            tally: crate::hud::HudState::default(),
            tally_seen: 0,
            load_stage: crate::loading::stage::JOIN,
            meter: meter::NetMeter::default(),
            game_addr: None,
        }));
        let s2 = shared.clone();
        let o2 = opts.clone();
        let t = std::thread::Builder::new()
            .name("dc-client net".into())
            .spawn(move || net_main(s2, o2))
            .ok();
        let hit_rigs = crate::hitnodes::HitRigs::new(
            opts.rom.clone().or_else(|| dc_rom::resolve_rom().ok()),
            opts.hit_rigs_async,
        );
        NetWorld {
            opts,
            handle: NetHandle {
                shared,
                thread: Arc::new(Mutex::new(t)),
            },
            hit_rigs,
            announced: None,
        }
    }

    pub fn handle(&self) -> NetHandle {
        self.handle.clone()
    }

    fn loader_for(&self, announced: &str) -> Result<Box<dyn WorldSource>, ClientError> {
        if let Some(f) = &self.opts.world_file {
            return Ok(Box::new(SynthSource {
                name: announced.to_string(),
                bytes: f.bytes.clone(),
                spawns: Vec::new(),
                stand_dims: f.stand_dims,
                textures: f.textures.clone(),
            }));
        }
        // MAP-30: a `@mod:` announce is a community map of the server's mod set, read before any ROM lookup
        if crate::modassets::is_mod_path(announced.trim()) {
            let rom = self.opts.rom.clone().or_else(|| dc_rom::resolve_rom().ok());
            return mod_world_for(announced, self.opts.mods.as_ref(), rom)
                .map(|w| Box::new(w) as Box<dyn WorldSource>);
        }
        let rom = match &self.opts.rom {
            Some(r) => r.clone(),
            None => dc_rom::resolve_rom().map_err(|e| ClientError::Rom(format!("CA_ROM: {e}")))?,
        };
        let map = map_name_of(announced).ok_or_else(|| {
            ClientError::NoSuchMap(format!("announced world {announced:?} has no file name"))
        })?;
        let names = RomWorld::map_names(&rom)?;
        let found = names
            .iter()
            .find(|n| n.eq_ignore_ascii_case(&map))
            .ok_or_else(|| ClientError::NoSuchMap(format!("{announced:?} (map {map})")))?;
        Ok(Box::new(RomWorld::new(&rom, found)))
    }
}

/// The drop object the frame's use key asks for: the key went down while alive (player state 1) and a dropped weapon
/// lies within reach of the eye ([`crate::drops::DropTracker::use_target`]).
// DC-DEFAULT: CLI-675 (proposed)
pub fn use_request(
    commands: &arms::WeaponCommands,
    drops: &crate::drops::DropTracker,
    eye: [f32; 3],
    life_state: u8,
) -> Option<u16> {
    (commands.use_key && life_state == 1)
        .then(|| drops.use_target(eye))
        .flatten()
}

/// The world of a `@mod:<id>/assets/<file>.dat` announce (MAP-30): the bytes through the installed mod layer
/// ([`crate::modassets::active`], the match's [`crate::modassets::MatchMods`]), else through the folders of the
/// join's `recipe`; without either the announce fails as a missing world (the 2009 0x29 case). The textures come
/// from the same layer and the ROM (`rom`: for textures, models and fonts; `None` without one).
// DC-DEFAULT: MAP-30 (proposed) `@mod:` announces resolve through the mod set before the ROM, see DC_DEFAULTS.md
pub fn mod_world_for(
    announced: &str,
    recipe: Option<&crate::mods::HostRecipe>,
    rom: Option<PathBuf>,
) -> Result<ModWorld, ClientError> {
    let announced = announced.trim();
    let assets = match crate::modassets::active() {
        Some(a) => a,
        None => match recipe {
            Some(r) => Arc::new(crate::modassets::ModAssets::from_recipe(r)),
            None => {
                return Err(ClientError::NoSuchMap(format!(
                    "{announced:?} (a community map, and no mod set is loaded)"
                )))
            }
        },
    };
    let path = assets
        .resolve(announced)
        .map_err(|e| ClientError::NoSuchMap(format!("{announced:?} ({e})")))?;
    let bytes = std::fs::read(&path)
        .map_err(|e| ClientError::NoSuchMap(format!("{announced:?} ({}: {e})", path.display())))?;
    Ok(ModWorld::new(announced, bytes, rom, Some(assets)))
}

/// The map name of an announced world path: the file name without its extension (`Worlds\X.dat` -> `X`).
// SPEC: UDP_SESSION_2009_A_addendum_disasm.md A9 (name form: relative path, `\` or `/`, case-insensitive)
pub fn map_name_of(announced: &str) -> Option<String> {
    let file = announced.rsplit(['\\', '/']).next()?.trim();
    let base = match file.rsplit_once('.') {
        Some((b, _)) => b,
        None => file,
    };
    (!base.is_empty()).then(|| base.to_ascii_uppercase())
}

impl Drop for NetWorld {
    fn drop(&mut self) {
        self.handle.shutdown(Duration::from_secs(3));
    }
}

impl WorldSource for NetWorld {
    // SPEC: GAME_MESSAGES_2009_C2S.md §6.1 (`UP_CHAT`, reliable; the line is shown from the server's echo only)
    fn chat(&mut self, line: &str, mode: crate::hud::chat::ChatScope) {
        let mut g = lock(&self.handle.shared);
        g.outgoing.push((
            crate::hud::chat::UP_CHAT,
            crate::hud::chat::up_chat_body(line, mode),
        ));
    }

    // SPEC: PROTOCOL_2009.md §1.1 (the lobby connection stays up during a match)
    fn lobby_frame(&mut self, frame: PktWriter) {
        lock(&self.handle.shared).lobby_out.push(frame);
    }

    // DC-DEFAULT: CLI-1571 (proposed) the overlay's numbers, from data the client already has
    fn net_sample(&mut self, players: bool) -> Option<crate::hud::netview::NetSample> {
        let lobby = match &self.opts.connect {
            Connect::Lobby { auth, .. } => Some(auth.to_string()),
            Connect::Udp(_) => None,
        };
        let mut g = lock(&self.handle.shared);
        let now = g.now();
        let meter = g.meter.view(now);
        let live: Vec<&Track> = g.tracks.values().filter(|t| !t.left).collect();
        let n = live.iter().filter(|t| t.total.samples > 0).count();
        let (delay_ms, depth) = if n > 0 {
            let with = || live.iter().filter(|t| t.total.samples > 0);
            (
                Some(with().map(|t| t.playout_delay(now)).sum::<f32>() / n as f32),
                Some(with().map(|t| t.buffer_depth(now) as f32).sum::<f32>() / n as f32),
            )
        } else {
            (None, None)
        };
        let rows = if players {
            live.iter()
                .map(|t| crate::hud::netview::PlayerLink {
                    pid: t.pid,
                    name: t.name.clone(),
                    age_ms: t.sample_age(now),
                    state: t.extrap_state(),
                })
                .collect()
        } else {
            Vec::new()
        };
        Some(crate::hud::netview::NetSample {
            meter,
            delay_ms,
            depth,
            lobby,
            server: g.game_addr.map(|a| a.to_string()),
            players: rows,
        })
    }

    fn describe(&self) -> String {
        match &self.opts.connect {
            Connect::Lobby { auth, .. } => {
                format!("network session via lobby {auth} as {}", self.opts.name)
            }
            Connect::Udp(a) => format!("network session with {a} as {}", self.opts.name),
        }
    }

    fn load(&mut self, gpu: Option<GpuTarget<'_>>) -> Result<LoadedWorld, ClientError> {
        let announced = self.handle.wait_world(self.opts.load_timeout)?;
        let loaded = self
            .loader_for(&announced)
            .and_then(|mut src| src.load(gpu));
        if loaded.is_ok() {
            self.announced = Some(announced.clone());
        }
        let mut g = lock(&self.handle.shared);
        let now = g.now();
        match loaded {
            Ok(mut w) => {
                let (min, max) = w.extents;
                if let Some(s) = g.session.as_mut() {
                    s.world_loaded(now, ExtentBox { min, max }, &w.name);
                }
                if w.spawns.is_empty() {
                    // a placeholder until the server's teleport: the world box centre (the body is held still
                    // until player state 1, CLI-9)
                    w.spawns.push(SpawnPoint {
                        pos: V3::new(
                            (min[0] + max[0]) * 0.5,
                            (min[1] + max[1]) * 0.5,
                            (min[2] + max[2]) * 0.5,
                        ),
                        yaw: 0.0,
                    });
                }
                g.world = Some(w.name.clone());
                Ok(w)
            }
            Err(e) => {
                if let Some(s) = g.session.as_mut() {
                    s.world_failed(now, &e.to_string());
                }
                Err(e)
            }
        }
    }

    // gpu-recover: the same world file again, without the session's world-loaded step (the session goes on)
    fn reload_scene(&mut self, gpu: GpuTarget<'_>) -> Result<LoadedWorld, ClientError> {
        let announced = self
            .announced
            .clone()
            .ok_or_else(|| ClientError::Gpu("no world loaded to rebuild".into()))?;
        let mut w = self.loader_for(&announced)?.load(Some(gpu))?;
        // a world without spawn points gets the placeholder of `load` (only the scene is used)
        w.spawns.clear();
        Ok(w)
    }

    fn poll_corrections(&mut self) -> Vec<Correction> {
        let mut g = lock(&self.handle.shared);
        let c = std::mem::take(&mut g.corrections);
        g.corrections_taken += c.len() as u64;
        c
    }

    fn report(&mut self, update: &LocalUpdate) {
        let mut g = lock(&self.handle.shared);
        let now = g.now();
        let mut u = *update;
        // the trailer's client time: seconds since the session started (engine game time of the 2009 client)
        // DC-DEFAULT: CLI-15 (proposed) trailer time = seconds since the network session started
        u.time = now as f32 / 1000.0;
        if let (Some(k), false) = (self.opts.glitch_after_updates, g.glitch_done) {
            let sent = g.session.as_ref().map_or(0, |s| s.obs.updates_sent);
            if sent >= k && g.glitch_until.is_none() {
                g.glitch_until = Some(now.wrapping_add(GLITCH_MS));
            }
        }
        if let Some(t) = g.glitch_until.filter(|_| !g.glitch_done) {
            if now < t {
                u.pos[0] += GLITCH_OFFSET;
                g.glitch_sent = true;
            } else {
                g.glitch_done = true;
            }
        }
        g.last_seq_reported = Some(u.move_seq);
        // the sprint pair: (1, 0) when sprinting starts, (0, 1) when it stops; the 2009 client sends (0, 1) once on
        // its first frame in the world (OBSERVED: tdm1 / tdm4, `0b ca 02` right after 0x87)
        // DC-DEFAULT: CLI-25 (proposed) sprint state and movement notices from the own simulation
        // the in-game frame ended at 0x3f: no sprint pair any more (the session sends no update either)
        if g.phase == NetPhase::InWorld && g.hud.life_state == 1 && g.result_at.is_none() {
            let sprinting = u.control & dc_sim::player::bits::SPRINT != 0;
            let pair = (sprinting, !sprinting);
            if g.sprint_pair != Some(pair) {
                g.sprint_pair = Some(pair);
                g.outgoing.push((UP_SPRINT_STATE, sprint_body(pair)));
            }
        }
        let weapon = g
            .arsenal
            .held()
            .is_some()
            .then(|| (g.arsenal.spread(), g.arsenal.trigger_flags(now)));
        let (mut wire, mut extras) = update_with_weapon(&u, weapon);
        update_with_zoom(
            &mut wire,
            &mut extras,
            g.arsenal.zoom_control_bits(u.control),
            g.arsenal.zoom_active(),
        );
        if let Some(s) = g.session.as_mut() {
            s.set_player_update_ext(wire, extras);
        }
    }

    fn moved(&mut self, events: &StepEvents) {
        if !self.opts.move_notices {
            return;
        }
        let mut g = lock(&self.handle.shared);
        if g.phase != NetPhase::InWorld || g.hud.life_state != 1 || g.result_at.is_some() {
            return;
        }
        for e in events.notice_codes() {
            g.outgoing.push((UP_MOVE_EVENT, move_notice_body(e)));
        }
    }

    fn frame_state(&mut self) -> FrameState {
        self.frame_state_led(0.0)
    }

    fn frame_state_led(&mut self, lead_ms: f32) -> FrameState {
        let mut g = lock(&self.handle.shared);
        // CLI-1576: the remote bodies, the effects and the sounds are drawn for the frame's render time
        let now = (g.now() as f32 + lead_ms).max(0.0).round() as u32;
        // DC-DEFAULT: CLI-16 (proposed) every remote player is drawn with the box of the own last teleport
        // (CLI-9's made-up box before the first one)
        let half = g.player_dims.unwrap_or([16.0, 40.0, 16.0]);
        let looks: BTreeMap<u16, RemoteLook> = match g.session.as_ref() {
            Some(s) => g
                .tracks
                .iter()
                .filter_map(|(&obj, t)| Some((obj, remote_look(s, obj, t, now)?)))
                .collect(),
            None => BTreeMap::new(),
        };
        // CLI-1571: the overlay's late / held frame counts (sums over the drawn tracks)
        let (late, held) = g.tracks.values().fold((0, 0), |(l, h), t| {
            (l + t.total.late_frames, h + t.total.held_frames)
        });
        g.meter.playout(now, late, held);
        let remotes: Vec<RemoteBody> = g
            .tracks
            .iter_mut()
            .filter_map(|(&obj, t)| {
                let pos = t.draw(now)?;
                Some(RemoteBody {
                    obj,
                    pid: t.pid,
                    name: t.name.clone(),
                    team: t.team,
                    pos,
                    half,
                    // past the yaw offset limit the body turns with the view (CLI-634)
                    yaw: t.yaw.map(|y| y + t.aim.map_or(0.0, |a| a.body_follow())),
                    look: looks.get(&obj).cloned(),
                })
            })
            .collect();
        let mut hud = g.hud.lines(&g.phase, g.tracks.len() + 1, now);
        if let Some(l) = g.arsenal.hud_line() {
            hud.insert(1, l);
        }
        // the first-person weapon belongs to the own view: none while dead or waiting (the death camera is a
        // third-person view)
        // DC-DEFAULT: CLI-86 (proposed) no first-person weapon in the not-playing states 2, 3, 5, 6
        let playing = !matches!(g.hud.life_state, 2 | 3 | 5 | 6);
        // a live world camera hides the first-person weapon too (cshell+0x1f50b0, CLI-240)
        let mut cam_shot = g
            .session
            .as_ref()
            .and_then(|s| crate::camshot::live_camera(&s.table.objects));
        // the camera's field-of-view notes (CLI-1862)
        if let Some(v) = cam_shot.as_mut() {
            g.cam_fovs.apply(v, now);
        }
        let view_weapon =
            view_weapon(&mut g.arsenal, now).filter(|_| playing && cam_shot.is_none());
        let own_pid = g
            .session
            .as_ref()
            .and_then(|s| s.obs.client_id.map(|c| c.0 as u8));
        let own_team = g
            .session
            .as_ref()
            .zip(own_pid)
            .and_then(|(s, p)| s.obs.roster.get(&p).map(|r| r.team));
        let death = match g.death_at {
            Some(at) if !playing => {
                // the observer search: team mates only, any player in classes 6 and 9 (cshell+0x60010)
                let class = g.hud_own.1;
                let mut dead = crate::world::PidSet::default();
                for (&obj, t) in &g.tracks {
                    if g.corpses.is_dead(obj) || t.left {
                        dead.insert(t.pid);
                    }
                }
                Some(crate::world::DeathView {
                    since_ms: now.saturating_sub(at),
                    killer: g.last_killer,
                    shot_dir: g.last_shot_dir,
                    own_pid,
                    own_team,
                    any_team: crate::deathcam::follows_any_team(class, own_team),
                    dead,
                })
            }
            _ => None,
        };
        let mover = std::mem::take(&mut g.mover_notes);
        let (own_object, own_look) = match (g.session.as_ref(), g.own_track.as_ref()) {
            (Some(s), Some((obj, t))) => (Some(*obj), remote_look(s, *obj, t, now)),
            _ => (None, None),
        };
        let sound_cues = std::mem::take(&mut g.sound_cues);
        let remote_attach = remote_attachments(&mut g, &sound_cues, &remotes);
        let (remote_carried, (hud_flags, flag_markers), remote_gear) = match g.session.as_ref() {
            Some(s) => (
                remote_carried(s, &remotes),
                hud_flags(s, &g.case_countdown),
                remote_gear(s),
            ),
            None => Default::default(),
        };
        let mut fx_events = std::mem::take(&mut g.fx_events);
        let secs = g.start.elapsed().as_secs_f64();
        let thrown = match g.session.as_ref() {
            Some(s) => {
                let mut v = thrown_bodies(s);
                for b in &mut v {
                    if let Some(p) = g.thrown.draw(b.obj, secs) {
                        b.pos = p;
                    }
                    // CLI-1136: the projectile row's effect rides on the drawn object
                    if let Some(ammo) = thrown_ammo(s, b.obj) {
                        // CLI-1136 (amended): in the object's rotation
                        let rot = s
                            .table
                            .objects
                            .get(&b.obj)
                            .and_then(|o| o.rot)
                            .unwrap_or([0.0, 0.0, 0.0, 1.0]);
                        fx_events.push(crate::fx::FxEvent::Thrown {
                            obj: b.obj,
                            ammo,
                            pos: b.pos,
                            rot,
                        });
                    }
                }
                v
            }
            None => Vec::new(),
        };
        let over = g.phase.is_final() || g.session.as_ref().is_some_and(BotSession::is_over);
        // the movement inputs a server gives: the step-height variable (looked up by name hash, the last value
        // wins) and the sprint permission: false until the server's first 0xca, also in the frames before the
        // network thread has its session (a server-backed source never leaves the sprint ungated; only a local
        // source gives `None`)
        // SPEC: MOVEMENT_2009.md §7 step 2; GAME_MESSAGES_2009_S2C.md §3.2; DC-DEFAULT: CLI-58 (proposed)
        let (mut step_height, scanned) = (g.step_height, g.evars_scanned);
        let mut sprint_allowed = false;
        if let Some(s) = g.session.as_ref() {
            for (name, value) in s.obs.evars.iter().skip(scanned) {
                if let Some(v) = step_height_of(name, value) {
                    step_height = Some(v);
                }
            }
            sprint_allowed = s.obs.sprint_allowed.unwrap_or(false);
        }
        g.evars_scanned = g.session.as_ref().map_or(scanned, |s| s.obs.evars.len());
        g.step_height = step_height;
        // client-hudgaps: every local weapon change (a 0x7f sent) shows the HUD's weapon panel
        // EVIDENCE: cshell+0x479d0 / 0x483e0 / 0x488d0 (the weapon-change paths call the panel's show and select)
        if g.arsenal.stats.selects != g.panel_selects {
            g.panel_selects = g.arsenal.stats.selects;
            if let Some(sel) = g.arsenal.held().map(|c| c.weapon) {
                let carried: Vec<u16> = g.arsenal.slots.values().map(|c| c.weapon).collect();
                g.hud_events.push((
                    now,
                    HudEvent::WeaponPanel {
                        carried,
                        selected: sel,
                    },
                ));
            }
        }
        let ammo = g.arsenal.held().map(|c| AmmoView {
            weapon: c.weapon,
            clip: c.clip,
            reserve: c.stock,
            clip_size: c.clip_size,
            single_shot: g.arsenal.single_shot(c.weapon),
            order_class: g.arsenal.order_class(c.weapon),
            infinite: g.arsenal.infinite_ammo(c.weapon),
        });
        let hud_cases = match g.session.as_ref() {
            Some(s) => hud_cases(s, &g.case_countdown),
            None => Vec::new(),
        };
        let anims = {
            let sh = &mut *g;
            let characters: Vec<u16> = sh.tracks.keys().copied().collect();
            match sh.session.as_ref() {
                Some(s) => crate::anims::frame(s, &mut sh.anim_clocks, &characters, now, half),
                None => crate::anims::AnimFrame::default(),
            }
        };
        FrameState {
            hud_cases,
            life_state: Some(g.hud.life_state),
            view_sets: std::mem::take(&mut g.view_sets),
            remotes,
            hud,
            session_over: over,
            view_weapon,
            view_hidden: g.arsenal.scope().weapons_hidden(),
            hud_events: {
                g.tally_seen = 0;
                std::mem::take(&mut g.hud_events)
            },
            ammo,
            clock_ms: Some(now),
            step_height,
            sprint_allowed: Some(sprint_allowed),
            // CLI-982: no aim mark while a live world camera holds the weapons
            aim_mark: g.arsenal.aim_mark(now).filter(|_| cam_shot.is_none()),
            scope: crate::hud::scope::ScopeView::of(g.arsenal.scope()),
            aim_cross_hidden: g.arsenal.aim_cross_hidden(),
            view_attach: g.arsenal.view_attachments(),
            remote_attach,
            remote_carried,
            remote_gear,
            hud_flags,
            flag_markers,
            aim_picture: g.arsenal.aim_picture(),
            sound_cues,
            own_object,
            own_look,
            death,
            mover,
            drops: g.drops.objects(),
            billboards: g.billboard_list.clone(),
            fx_events,
            thrown,
            anims,
            // DC-DEFAULT: CLI-230 (proposed) the world models come from the server's objects
            world_objects: Some(
                g.session
                    .as_ref()
                    .map(|s| crate::objects::server_objects(&s.table))
                    .unwrap_or_default(),
            ),
            cam_shot,
            // client-hudgaps: the sky table with its objects resolved now
            sky: g.session.as_ref().and_then(|s| {
                crate::objects::sky_table(s.obs.sky_block.as_ref(), &s.obs.sky_ids, &s.table)
            }),
            own_team,
            held_weapon: g.arsenal.held().map(|c| c.weapon),
            equipment: g.equipment.clone(),
        }
    }

    fn rom(&self) -> Option<PathBuf> {
        self.opts.rom.clone().or_else(|| dc_rom::resolve_rom().ok())
    }

    fn reality(&self) -> bool {
        self.opts.reality
    }

    fn draw_voice(&self) -> bool {
        self.opts.draw_voice
    }

    fn arms(&mut self, frame: &ArmsFrame<'_>) -> ArmsResult {
        // DC-DEFAULT: CLI-300 (proposed) the characters' node spheres at this frame's pose, posed outside the lock
        let hit_nodes = self.hit_rigs.frame(frame.remotes, frame.anims);
        let mut g = lock(&self.handle.shared);
        // no weapon after 0x3f: the in-game frame that runs the weapon ended (DISASM cshell+0x4ac5c, matchexit.rs)
        if g.phase != NetPhase::InWorld || g.result_at.is_some() {
            return ArmsResult::default();
        }
        g.arsenal.set_hit_nodes(hit_nodes);
        g.arsenal.set_normal_fov_x(frame.normal_fov_x);
        // a live world camera (the round-end shot) holds the weapons and hides the aim mark (CLI-982)
        let cam_live = g
            .session
            .as_ref()
            .is_some_and(|s| crate::camshot::live_camera(&s.table.objects).is_some());
        g.arsenal.set_cinematic(cam_live);
        let now = g.now();
        // DC-DEFAULT: CLI-99 (proposed) a dead remote character has no hit volume ([`Corpses`])
        let corpses = frame
            .remotes
            .iter()
            .filter(|r| g.corpses.is_dead(r.obj))
            .count();
        // a character inside its spawn window takes no hit: the 2009 weapon claims none on it, and its flag clears by
        // itself 3500 ms after the start note
        // EVIDENCE: cshell+0x238eb0 (impact on a character with +0x53a set: return), cshell+0x206260 (0x1f sets,
        // 0x25 clears), cshell+0x203e20 (cleared when 0xdac ms passed)
        // DC-DEFAULT: CLI-572 (proposed) the shot passes the shielded character (no target) instead of stopping on it
        g.spawn_shield
            .retain(|_, at| now.saturating_sub(*at) < SPAWN_SHIELD_MS);
        let shielded: Vec<u16> = g.spawn_shield.keys().copied().collect();
        let targets: Vec<Target> = frame
            .remotes
            .iter()
            .filter(|r| !g.corpses.is_dead(r.obj))
            .filter(|r| !shielded.contains(&r.obj))
            .filter(|r| r.pos.iter().chain(r.half.iter()).all(|v| v.is_finite()))
            .map(|r| Target {
                obj: r.obj,
                centre: r.pos,
                half: r.half,
            })
            .collect();
        g.arsenal.set_motion(frame.motion);
        let shots_before = g.arsenal.stats.shots;
        // CLI-1100: the bomb item in the hand fires no shot: the fire key is the plant hold (below)
        let bomb_out =
            crate::hud::bomb::bomb_in_hand(g.hud_own.1, g.arsenal.held().map(|c| c.slot));
        let mut commands = frame.commands;
        if bomb_out {
            commands.fire = false;
            commands.fire_press = false;
        }
        // CLI-1191: no weapon change while the own plant / defuse hold runs
        crate::hud::bomb::lock_weapon_change(&mut commands, g.bomb_hold.is_some());
        let out = g.arsenal.step(
            now,
            &commands,
            &frame.aim,
            &targets,
            // DC-DEFAULT: CLI-250 (proposed) the shot trace also meets the shot-only world models
            Some(&crate::shots::ShotWorld {
                main: frame.world,
                extra: frame.loaded.and_then(|w| w.world_models.shot_world()),
            } as &dyn dc_sim::Collider),
        );
        if corpses > 0 {
            g.corpse_skips += u64::from(g.arsenal.stats.shots - shots_before);
        }
        // DC-DEFAULT: CLI-1856 (proposed) one line per spawn: the weapon drawn, whether the server named it, the kit
        if let Some(d) = g.arsenal.take_spawn_draw() {
            let kit: Vec<String> = d.carried.iter().map(|(s, w)| format!("{s}:{w}")).collect();
            eprintln!(
                "dc-client: spawn draw: slot {} weapon {} ({}), carried [{}]",
                d.slot,
                d.weapon,
                if d.by_server {
                    "the server's draw"
                } else {
                    "own pick, lowest slot"
                },
                kit.join(" ")
            );
        }
        // the own weapon's sounds: one fire sound per shot, and the key sounds the client announced with 0x80
        // DC-DEFAULT: CLI-72 (amended by w2-scope) one own fire sound per shot of the arsenal (the 2009 fire state plays
        // it on each entry, cshell+0x237180)
        // SPEC: GAME_MESSAGES_2009_C2S.md §4.3 (the sound key plays the weapon row's sound n locally and sends 0x80)
        // the muzzle mod's silencer picks the silenced sound (weapon sound 9; EVIDENCE: cshell+0x2330e0)
        if let Some(w) = g.arsenal.held().map(|c| c.weapon) {
            let silencer = g.arsenal.silencer_level(w);
            for _ in shots_before..g.arsenal.stats.shots {
                g.sound_cues.push(SoundCue::OwnFire {
                    weapon: w,
                    silencer,
                });
            }
        }
        // the zoom sounds: a local sound of the sight's ZoomInSound / ZoomOutSound at volume 100
        // EVIDENCE: cshell+0x1f3590 (cshell+0x2b1e20(name, 1, 0, 100, 1.0, 0))
        for z in g.arsenal.take_zoom_sounds() {
            g.sound_cues.push(SoundCue::Interface {
                file: Some(z.file),
                what: if z.zoom_in { "zoom_in" } else { "zoom_out" },
            });
        }
        // DC-DEFAULT: CLI-250 (proposed) the impact of a pellet that met the world is resolved now, with the models
        // the server had switched on when it was fired
        // DC-DEFAULT: CLI-1603 (proposed, USER REQUEST 2026-10-31 "the view for the player should be instant even if
        // the latency compensation is working in the background") a pellet that met a character's box plays the
        // shooter's hit feedback (CLI-98's sound) and gives the mark under the cross the hit colour (CLI-594) now,
        // at the shot; the server's record of the hit (one round trip later) repeats neither. 2009 waited for the
        // record (cshell+0x2c8b4a); the hit test is the client's in both, so the server's refusals (DSV-42) are the
        // only hits the feedback over-reports
        for mut s in g.arsenal.take_shot_fx() {
            match s.hit {
                arms::RayHit::Miss => {
                    if let Some(w) = frame.loaded {
                        s.impact = w.shot_hit(s.origin, s.dir, s.range).map(|h| h.at());
                    }
                }
                arms::RayHit::Character { obj, .. } => {
                    g.predicted_hits += 1;
                    g.sound_cues.push(SoundCue::OwnHit {
                        first: obj,
                        ammo: s.ammo,
                    });
                    g.hud_events.push((now, HudEvent::OwnHit));
                }
            }
            g.fx_events.push(crate::fx::FxEvent::Shot(s));
        }
        for m in out.iter().filter(|m| m.shell == arms::up::WEAPON_SOUND) {
            let mut r = m.body.reader();
            let k = r.read_u8();
            let weapon = r.read_u16();
            g.sound_cues.push(SoundCue::OwnWeapon { k, weapon });
        }
        // CLI-1845: the inspect's sounds play here only, nothing goes out
        for (k, weapon) in g.arsenal.take_inspect_sounds() {
            g.sound_cues.push(SoundCue::OwnWeapon { k, weapon });
        }
        for m in out {
            g.outgoing.push((m.shell, m.body));
        }
        // CLI-1230 / CLI-1795: the use prompt's door: the one the use key turns now (our server's reach, DSV-270)
        let gm = &mut *g;
        if let Some(s) = gm.session.as_ref() {
            gm.use_entries.sync(&s.table);
        }
        let target = match (gm.session.as_ref(), frame.loaded) {
            (Some(s), Some(w)) if !gm.use_entries.doors.is_empty() && gm.hud.life_state == 1 => gm
                .use_entries
                .reach_target(&s.table, frame.body, |n| w.world_models.file_box(n)),
            _ => None,
        };
        if target != g.use_target {
            g.use_target = target;
            g.hud_events.push((now, HudEvent::UseTarget(target)));
        }
        // CLI-1352: the item prompt while the use key would take a dropped weapon
        // DISASM: cshell+0x2b27c0 / 0x2b31a0 (the aim ray meets the drop within the activation distance, world first)
        let item = g.hud.life_state == 1 && {
            let dir = crate::arms::forward(frame.aim.yaw, frame.aim.pitch);
            let reach = crate::hud::prompt::ACTIVATION_DISTANCE;
            g.drops
                .aim_target(frame.aim.origin, dir, reach)
                .is_some_and(|(_, d)| {
                    frame
                        .loaded
                        .and_then(|w| w.shot_hit(frame.aim.origin, dir, d))
                        .is_none()
                })
        };
        if item != g.item_target {
            g.item_target = item;
            g.hud_events.push((now, HudEvent::ItemTarget(item)));
        }
        // the use key over a dropped weapon asks the server for it (it gives the rounds, the weapon, or swaps it in)
        // DC-DEFAULT: CLI-675 (proposed)
        if let Some(obj) = use_request(
            &frame.commands,
            &g.drops,
            frame.aim.origin,
            g.hud.life_state,
        ) {
            g.outgoing
                .push((crate::drops::UP_USE, crate::drops::up_use_body(obj)));
        } else if frame.commands.use_key && g.hud.life_state == 1 {
            // DC-DEFAULT: CLI-1133 (proposed) the use key with no drop in reach sends `UP_USE` with no object: the
            // server toggles the lever / button door in reach (DSV-270)
            g.outgoing.push((
                crate::drops::UP_USE,
                crate::drops::up_use_body(crate::drops::USE_NO_OBJECT),
            ));
        }
        // the use key held at a plant / defuse point (classes 2 / 3): 0xd8 phase 1, phase 4 when let go
        // DC-DEFAULT: CLI-1054 (proposed)
        let points = g
            .session
            .as_ref()
            .map(|s| crate::hud::bomb::use_points(&s.table.objects))
            .unwrap_or_default();
        let (own_pid, mode_class) = g.hud_own;
        let team = g
            .session
            .as_ref()
            .zip(own_pid)
            .and_then(|(s, p)| s.obs.roster.get(&p).map(|r| r.team));
        let alive = g.hud.life_state == 1;
        let mut hold = g.bomb_hold;
        if let Some((phase, target)) = crate::hud::bomb::hold_step(
            &mut hold,
            frame.commands.use_held || (bomb_out && frame.commands.fire),
            alive,
            mode_class,
            team,
            frame.aim.origin,
            &points,
        ) {
            g.outgoing.push((
                crate::hud::bomb::UP_USE_HOLD,
                crate::hud::bomb::use_hold_body(phase, target),
            ));
            // CLI-1250: the own body stops with the hold start at a point the server takes (not one round trip
            // later with the server's freeze) and goes again with the own release
            let note = if phase == 1 {
                g.session
                    .as_ref()
                    .is_some_and(|s| {
                        crate::hud::bomb::hold_point_takes(
                            &s.table.objects,
                            target,
                            frame.aim.origin,
                        )
                    })
                    .then_some(crate::world::HoldNote::Start)
            } else {
                Some(crate::world::HoldNote::End)
            };
            // CLI-1530 / CLI-1531: a hold the server takes plays the bomb's plant clip keys (looped for a defuse)
            g.hold_sound = match note {
                Some(crate::world::HoldNote::Start) => g
                    .arsenal
                    .bomb_row()
                    .map(|w| (now, team == Some(crate::hud::bomb::DEFUSE_TEAM), w)),
                _ => None,
            };
            if let Some(h) = note {
                g.mover_notes.push(crate::world::MoverNote {
                    hold: Some(h),
                    ..crate::world::MoverNote::default()
                });
            }
            if phase == 4 {
                g.hud_events
                    .push((now, HudEvent::Bomb(crate::hud::bomb::BombEvent::Released)));
            }
        }
        g.bomb_hold = hold;
        let (weapon, elapsed, looped) = match g.hold_sound {
            Some((at, defuse, w)) => (w, Some(now.saturating_sub(at)), defuse),
            None => (0, None, false),
        };
        for m in g.arsenal.hold_sounds(weapon, elapsed, looped, &frame.aim) {
            let k = m.body.reader().read_u8();
            g.sound_cues.push(SoundCue::OwnWeapon { k, weapon });
            g.outgoing.push((m.shell, m.body));
        }
        ArmsResult {
            view_pitch: g.arsenal.take_view_pitch(),
            view_yaw: g.arsenal.take_view_yaw(),
            camera: g.arsenal.camera_angles(now),
            eye_back: g.arsenal.eye_back(),
            zoom: crate::world::ZoomFrame {
                level: g.arsenal.scope().level(),
                active: g.arsenal.zoom_active(),
                fov_x: g.arsenal.zoom_fov_x(now),
            },
        }
    }
}

/// The HUD event of a whole shell message (`GameEvent::Shell`); countdown notes are kept per object in `countdown`.
/// `mode_class` `None` (no 0x6c endpoint yet and no room hint): a 0x45 with the intro block set keeps its head and
/// skips the block (its caption is class-dependent), with a line in `notes`.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1, §3.2, §3.5, §8.3 (read by `hud::msgs`)
fn shell_hud_event(
    shell: u8,
    body: &[u8],
    bits: usize,
    mode_class: Option<u8>,
    countdown: &mut BTreeMap<u16, i32>,
    notes: &mut Vec<String>,
) -> Option<HudEvent> {
    use crate::hud::msgs::{read_shell, ObjectNote, ShellRead};
    Some(match read_shell(shell, body, bits, mode_class)? {
        ShellRead::MatchBegin(m) => {
            if m.intro_skipped {
                notes.push(
                    "0x45: intro block skipped (mode class unknown; its caption field depends on the class)"
                        .to_string(),
                );
            }
            HudEvent::MatchBegin(m)
        }
        ShellRead::RoundResult {
            round_secs,
            win_team,
            wins,
        } => HudEvent::RoundResult {
            round_secs,
            win_team,
            wins,
        },
        ShellRead::RoundClose(secs) => HudEvent::ClockStop(secs),
        ShellRead::ResultScreen { flag, value } => HudEvent::ResultScreen { flag, value },
        ShellRead::Goal(g) => HudEvent::Goal(g),
        ShellRead::Hisec(h) => HudEvent::Hisec(h),
        ShellRead::Flag(f) => HudEvent::Flag(f),
        ShellRead::Bomb(b) => HudEvent::Bomb(b),
        ShellRead::Qrt(q) => HudEvent::Qrt(q),
        ShellRead::Spy(e) => HudEvent::Spy(e),
        ShellRead::Mission(e) => HudEvent::Mission(e),
        ShellRead::CoopClock { running, secs } => HudEvent::CoopClock { running, secs },
        ShellRead::Note(ObjectNote::Countdown { obj, remaining }) => {
            countdown.insert(obj, remaining);
            return None;
        }
        ShellRead::Note(n) => HudEvent::Note(n),
        ShellRead::KillRun { kills, head, combo } => HudEvent::KillRun { kills, head, combo },
        ShellRead::ObjectsEnd => return None,
        // SPEC: GAME_MESSAGES_2009_S2C.md §2.4 row 0x68
        ShellRead::Chat(m) => HudEvent::Chat(m),
        // the kick form of the leave line (the leave itself comes from the roster)
        // SPEC: GAME_MESSAGES_2009_S2C.md §3.2 row 0x6c
        ShellRead::PlayerLeft { pid, kicked } => {
            if !kicked {
                return None;
            }
            HudEvent::Kicked(pid)
        }
        // SPEC: GAME_MESSAGES_2009_S2C.md §3.2 row 0xa2; DC-DEFAULT: CLI-1360 (proposed) the board's ping column
        ShellRead::PingTable(v) => HudEvent::Pings(v),
        // DC-DEFAULT: CLI-1631 (proposed) the x-ray health bars' numbers (DSV-480)
        ShellRead::RemoteHealth(v) => HudEvent::RemoteHealth(v),
    })
}

/// The pickups attached to each remote character (a carried capture flag): the pickup's model and the socket index.
// SPEC: UDP_SESSION_2009_B.md §4.2 item 8 (the attachment list: child object, socket index)
// DC-DEFAULT: CLI-1072 (proposed) a type-0x17 child is drawn at its body socket
/// CLI-1231: the gear each roster record's look names, by its character object.
// DC-DEFAULT: CLI-1231 (proposed) the look bytes of [`crate::menu::figure::GearIds::from_look`]
fn remote_gear(s: &BotSession) -> BTreeMap<u16, crate::menu::figure::GearIds> {
    s.obs
        .roster
        .values()
        .filter(|r| r.character != 0xffff)
        .map(|r| (r.character, crate::menu::figure::GearIds::from_look(&r.look)))
        .collect()
}

fn remote_carried(
    s: &BotSession,
    remotes: &[RemoteBody],
) -> BTreeMap<u16, Vec<(crate::world::ModelRef, u32)>> {
    let name = |id: u16| s.obs.files.get(&id).map(|f| f.1.clone());
    let mut out = BTreeMap::new();
    for r in remotes {
        let Some(o) = s.table.objects.get(&r.obj) else {
            continue;
        };
        let list: Vec<(crate::world::ModelRef, u32)> = o
            .attachments
            .iter()
            .filter_map(|a| {
                let child = s.table.objects.get(&a.child)?;
                if child.blob_type != Some(crate::drops::OBJ_PICKUP) {
                    return None;
                }
                let e = child.models.first()?;
                // MAP-34: a flag created without a model (an empty name) is drawn by its marker, not as a model
                Some((
                    crate::world::ModelRef {
                        model: name(e.model).filter(|n| !n.is_empty())?,
                        textures: e
                            .textures
                            .iter()
                            .map(|t| name(*t).unwrap_or_default())
                            .collect(),
                    },
                    a.socket,
                ))
            })
            .collect();
        if !list.is_empty() {
            out.insert(r.obj, list);
        }
    }
    out
}

/// The capture flags (type 0x17 kind 2: the blob's team) and the capture zones (type 0x37 kind 6: `z_team`) of the
/// session's objects for the in-world flag markers; a flag attached to a character stands at that character. The
/// second list: the flags on the ground whose create names no model (MAP-34), drawn as a bar in the team colour.
// SPEC: GAME_MODES_2009.md §4.3 (zone kind 6, `z_team`), §4.4 (kind 2 `[state][team][at_base]`)
// EVIDENCE: cshell+0x86f10 (each kind-2 mode object and each kind-6 zone of the own team, at its position)
// DC-DEFAULT: MAP-34 (proposed) a capture flag without a model draws a marker bar, see DC_DEFAULTS.md
fn hud_flags(
    s: &BotSession,
    countdown: &BTreeMap<u16, i32>,
) -> (Vec<crate::world::HudFlag>, Vec<crate::world::FlagMarker>) {
    let mut out = Vec::new();
    let mut markers = Vec::new();
    let carrier_of = |flag: u16| {
        s.table
            .objects
            .values()
            .find(|o| o.attachments.iter().any(|a| a.child == flag))
            .and_then(|o| o.pos)
    };
    let file_name = |id: u16| s.obs.files.get(&id).map(|f| f.1.clone());
    for (id, o) in &s.table.objects {
        let Some(b) = o.blob.as_ref() else {
            continue;
        };
        match o.blob_type {
            Some(crate::drops::OBJ_PICKUP) => {
                let Some(team) = crate::hud::modetop::flag_blob_team(b.as_bytes(), b.bit_len())
                else {
                    continue;
                };
                let carried = carrier_of(*id);
                let Some(pos) = carried.or(o.pos).or(Some(o.create_pos)) else {
                    continue;
                };
                out.push(crate::world::HudFlag {
                    team,
                    base: false,
                    pos,
                    remaining: countdown.get(id).copied().unwrap_or(0),
                });
                let no_model = crate::drops::pickup_model(o, &file_name)
                    .is_none_or(|m| m.model.trim().is_empty());
                if carried.is_none() && no_model && crate::drops::pickup_shown(o) {
                    markers.push(crate::world::FlagMarker { team, pos });
                }
            }
            Some(0x37) => {
                let Some(team) = crate::hud::modetop::capture_zone_team(b.as_bytes(), b.bit_len())
                else {
                    continue;
                };
                out.push(crate::world::HudFlag {
                    team,
                    base: true,
                    pos: o.pos.unwrap_or(o.create_pos),
                    remaining: 0,
                });
            }
            _ => {}
        }
    }
    (out, markers)
}

/// The HiSec cases among the session's objects: type 0x17 create blobs of pickup kind 5, with their position and the
/// last countdown note.
// SPEC: HISEC_2009.md §3.1, §3.3
fn hud_cases(s: &BotSession, countdown: &BTreeMap<u16, i32>) -> Vec<crate::world::HudCase> {
    s.table
        .objects
        .iter()
        .filter(|(_, o)| o.blob_type == Some(0x17))
        .filter_map(|(id, o)| {
            let b = o.blob.as_ref()?;
            let key = crate::hud::msgs::case_key(b.as_bytes(), b.bit_len())?;
            Some(crate::world::HudCase {
                key,
                pos: o.pos?,
                remaining: countdown.get(id).copied().unwrap_or(0),
            })
        })
        .collect()
}

/// The held weapon for the first-person view: the weapon row's first-person model with its skins and the clip
/// the weapon state plays.
/// The third-person mod models of the remote characters: the mods of each character's last shot record, drawn
/// while the character holds that record's weapon (its weapon object's model is the row's `HHModel`).
// DC-DEFAULT: CLI-618 (see `crate::attach::AttachTable::hh_attachments`)
fn remote_attachments(
    g: &mut Shared,
    cues: &[crate::sound::SoundCue],
    remotes: &[RemoteBody],
) -> BTreeMap<u16, Vec<crate::attach::WeaponAttach>> {
    for c in cues {
        if let crate::sound::SoundCue::Shot {
            shooter,
            weapon,
            mods,
            own: false,
            ..
        } = c
        {
            g.remote_mods.insert(*shooter, (*weapon, *mods));
        }
    }
    let Some(t) = g.arsenal.config().table.clone() else {
        return BTreeMap::new();
    };
    remotes
        .iter()
        .filter_map(|r| {
            let (w, mods) = g.remote_mods.get(&r.obj)?;
            let hh = t.get(*w)?.hh_model.as_ref()?;
            let held = r.look.as_ref()?.weapon.as_ref()?;
            if !held.0.model.eq_ignore_ascii_case(hh) {
                return None;
            }
            let v = t.attach.hh_attachments(*w, *mods);
            (!v.is_empty()).then_some((r.obj, v))
        })
        .collect()
}

fn view_weapon(a: &mut Arsenal, now: u32) -> Option<ViewWeapon> {
    let c = *a.held()?;
    let row = a
        .config()
        .table
        .as_ref()
        .and_then(|t| t.get(c.weapon))
        .cloned();
    let pos = row.as_ref().and_then(|r| r.pv_pos).unwrap_or([0.0; 3]);
    // DC-DEFAULT: CLI-199 (proposed) the model the clips came from (the row's first-identifier model may be missing)
    let loaded = a.clip_model(c.weapon);
    let model = row.and_then(|r| {
        Some(ModelRef {
            model: loaded.or(r.pv_model)?,
            textures: r.pv_skins,
        })
    });
    let clip = a.view_clip(now).map(|(_, i, t)| (i, t));
    Some(ViewWeapon {
        weapon: c.weapon,
        model,
        clip,
        pos,
        inspect: a.inspect_pose(now),
    })
}

/// The 2009 client's own end of a spawn window: 0xdac ms after the start note.
// EVIDENCE: cshell+0x203e20 (GetTickCount - 0xdac against the stamp of sub 0x1f)
pub const SPAWN_SHIELD_MS: u32 = 3_500;

/// Most time an animation tracker is advanced past its last stream entry.
// DC-DEFAULT: CLI-37 (proposed) the trackers run on locally between stream entries for at most 1000 ms
pub const TRACKER_EXTRAPOLATE_MS: u32 = 1000;

/// The models and pose of a remote character from the session's object table: the model block of its object
/// (names from the file table), the weapon object attached to it, the stream's trackers.
// SPEC: UDP_SESSION_2009_B.md §3.4, §3.5 (body entry, face entry; the weapon object at socket 2; CF_FILES on it is
// the weapon change), §4.3 (trackers in every stream entry), part A §4.4 (file table names)
fn remote_look(s: &BotSession, obj: u16, t: &Track, now: u32) -> Option<RemoteLook> {
    let o = s.table.objects.get(&obj)?;
    let name = |id: u16| -> Option<String> {
        (id != 0xFFFF)
            .then(|| s.obs.files.get(&id).map(|f| f.1.clone()))
            .flatten()
    };
    let refs = |models: &[dc_botclient::objects::ModelFileEntry]| -> Vec<ModelRef> {
        models
            .iter()
            .filter_map(|e| {
                Some(ModelRef {
                    model: name(e.model)?,
                    textures: e
                        .textures
                        .iter()
                        .map(|t| name(*t).unwrap_or_default())
                        .collect(),
                })
            })
            .collect()
    };
    // a pickup on the character (a carried flag) is not its weapon (CLI-1072)
    let weapon = o.attachments.iter().find_map(|a| {
        let child = s.table.objects.get(&a.child)?;
        if child.blob_type == Some(crate::drops::OBJ_PICKUP) {
            return None;
        }
        let m = refs(&child.models).into_iter().next()?;
        Some((m, a.socket))
    });
    let age = now.saturating_sub(t.anim_at).min(TRACKER_EXTRAPOLATE_MS) as f32;
    let trackers = o
        .anim
        .iter()
        .map(|a| RemoteTracker {
            clip: a.anim as usize,
            time_ms: a.time_ms as f32 + age,
            blend_set: a.blend_set.map(|w| w as usize),
        })
        .collect();
    // the hidden form before the spawn: bit 0x0002 set, 0x0001 clear (PLAUSIBLE reading, B §3.5)
    // DC-DEFAULT: CLI-42 (proposed) a character object in that form is not drawn
    let visible = !(o.flags & 0x0002 != 0 && o.flags & 0x0001 == 0);
    let styles = block_body_styles(&o.models, name);
    Some(RemoteLook {
        styles,
        models: refs(&o.models),
        weapon,
        trackers,
        visible,
        aim: t.aim.map(|a| a.drawn()),
    })
}

/// The body entry's render styles as the model block names them (CLI-1150); a name the file table lacks drops the
/// whole list, the table row's styles then stay (CLI-109).
// SPEC: UDP_SESSION_2009_B.md §3.4 (per entry the render-style file ids)
fn block_body_styles(
    models: &[dc_botclient::objects::ModelFileEntry],
    name: impl Fn(u16) -> Option<String>,
) -> Vec<String> {
    models
        .first()
        .and_then(|e| {
            e.styles
                .iter()
                .map(|&id| name(id))
                .collect::<Option<Vec<_>>>()
        })
        .unwrap_or_default()
}

/// The 0x64 fields of an update with the held weapon's part: the spread float (`weapon_f`, 0x0200; the session
/// sends it when it changed) and, while the fire key is held, control bit 0x400 with `pu_flags` 0x2000.
/// `weapon` = (spread, fire bits) when a weapon is held.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (flag 0x0200, 0x2000; control FIRE 0x400)
// OBSERVED: CAPTURE_tdm4_WEAPON_FORMS.md §4 (0x3321 / 0x600 at the shots, 0x1321 / 0x200 between them)
pub fn update_with_weapon(
    u: &LocalUpdate,
    weapon: Option<(f32, bool)>,
) -> (
    dc_botclient::shell::MoveReport,
    dc_botclient::shell::UpdateExtras,
) {
    let mut wire = u.to_wire();
    let mut extras = u.extras();
    if let Some((spread, fire)) = weapon {
        extras.weapon_f = Some(spread);
        if fire {
            wire.control |= arms::FIRE_CONTROL_BIT;
            extras.flag_bits |= arms::FIRE_PU_FLAG;
        }
    }
    (wire, extras)
}

/// The 0x64 fields of the scope: the control word's 0x80000 bit (`bits`, from
/// [`crate::arms::Arsenal::zoom_control_bits`]) and `pu_flags` 0x0800 while the zoom state is on.
// SPEC: MOVEMENT_2009.md §5.1 (0x80000), GAME_MESSAGES_2009_C2S.md §2.1 (pu_flags 0x0800); OBSERVED: rust7 updates.csv
pub fn update_with_zoom(
    wire: &mut dc_botclient::shell::MoveReport,
    extras: &mut dc_botclient::shell::UpdateExtras,
    bits: u32,
    active: bool,
) {
    wire.control |= bits;
    if active {
        extras.flag_bits |= crate::zoom::ZOOM_PU_FLAG;
    }
}

/// The box colour of a team (CLI-13).
pub fn team_colour(team: u8) -> [f32; 4] {
    match team {
        1 => TEAM_COLOURS[0],
        2 => TEAM_COLOURS[1],
        _ => TEAM_COLOURS[2],
    }
}

#[cfg(test)]
mod jitter_sim;
pub mod meter;

#[cfg(test)]
mod tests {

    /// CLI-1857: an AI character (dc-dserver's NPC record: no roster binding, the NPC tail) is drawn with its caption
    /// and team; a player's character is drawn from its roster record only; other blobs are no characters.
    #[test]
    fn an_ai_character_without_a_roster_record_is_drawn_with_its_caption_and_team() {
        use dc_botclient::objects::ClientObject;
        use dc_botclient::record::Recorder;
        use dc_botclient::session::{BotSession, GameConfig};
        use dc_botclient::shell::RosterRecord;
        use dc_dserver::modelobj::{CharacterBlob, NpcBlock};
        let ai = CharacterBlob::for_npc(
            [16.0, 40.0, 16.0],
            NpcBlock {
                npc_rank: 0,
                team: 2,
                kind: 1,
                health_bits: 1.0f32.to_bits(),
                caption: b"Guard".to_vec(),
            },
        )
        .write();
        let player = CharacterBlob::for_player(3, [16.0, 40.0, 16.0], true).write();
        let obj = |blob: dc_wire::udp::BitWriter| {
            let mut o = ClientObject::default();
            o.blob_type = blob.as_bytes().first().copied();
            o.blob = Some(blob);
            o
        };
        let n = super::npc_character(&obj(ai.clone())).expect("an AI character");
        assert_eq!((n.team, n.kind, n.caption.as_str()), (2, 1, "Guard"));
        assert!(super::npc_character(&obj(player.clone())).is_none());
        let mut other = dc_wire::udp::BitWriter::new();
        other.write_u8(0x13).write_u32(0);
        assert!(super::npc_character(&obj(other)).is_none());

        let mut s = BotSession::new(GameConfig::new("Me"), None, Recorder::null("t"));
        s.table.objects.insert(200, obj(ai));
        s.table.objects.insert(201, obj(player.clone()));
        s.table.objects.insert(202, obj(player));
        s.obs.roster.insert(
            3,
            RosterRecord {
                name: "Dave".into(),
                pid: 3,
                character: 201,
                team: 1,
                ..Default::default()
            },
        );
        s.obs.roster.insert(
            4,
            RosterRecord {
                name: "Me".into(),
                pid: 4,
                character: 202,
                team: 1,
                ..Default::default()
            },
        );
        let drawn = super::drawn_characters(&s, Some(202), Some(4));
        assert_eq!(
            drawn,
            vec![
                (201, 3, "Dave".to_string(), 1),
                (200, super::NPC_PID, "Guard".to_string(), 2)
            ]
        );
    }

    #[test]
    fn the_last_gasp_sends_two_plain_byes_to_the_registered_server_and_nothing_once_cleared() {
        // CLI-1702 (fix-first-match-drop): the watchdog's abort leaves the match with the plain bye
        use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
        let server = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        server
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let to: SocketAddr = server.local_addr().unwrap();
        let client = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let token = 0xfeed_0000_0001;
        super::gasp_set(token, Some((client.try_clone().unwrap(), to)));
        assert!(super::last_gasp());
        let mut buf = [0u8; 64];
        for _ in 0..2 {
            let (n, from) = server.recv_from(&mut buf).expect("a bye");
            assert_eq!(from, client.local_addr().unwrap());
            assert_eq!(
                dc_wire::udp::decode_link_datagram(&buf[..n]).unwrap().ctl,
                Some(dc_wire::udp::CtlBlock::Bye)
            );
        }
        // another thread's token does not clear it; the owner's does
        super::gasp_set(token + 1, None);
        assert!(super::last_gasp());
        super::gasp_set(token, None);
        assert!(!super::last_gasp());
    }

    #[test]
    fn the_body_styles_of_the_model_block_reach_the_look() {
        // CLI-1150: the infected look (class 11) is the body with the override row's styles in the block; the
        // client dropped them and drew the table row's styles
        use dc_botclient::objects::ModelFileEntry;
        let e = |styles: Vec<u16>| ModelFileEntry {
            model: 1,
            subs: Vec::new(),
            textures: vec![2],
            styles,
        };
        let name = |id: u16| match id {
            7 => Some("rs/infected.ltb".to_string()),
            8 => Some("rs/infected_b.ltb".to_string()),
            _ => None,
        };
        assert_eq!(
            super::block_body_styles(&[e(vec![7, 8]), e(vec![9])], name),
            vec![
                "rs/infected.ltb".to_string(),
                "rs/infected_b.ltb".to_string()
            ]
        );
        assert!(
            super::block_body_styles(&[e(vec![7, 99])], name).is_empty(),
            "an unknown id keeps the table's"
        );
        assert!(super::block_body_styles(&[], name).is_empty());
    }

    #[test]
    fn the_result_page_follows_the_0x3f_within_about_a_second() {
        // CLI-1124: 2009 opened the result page at 0x3f (cshell+0x1bb140(7)); our page waits for the hold
        assert!(RESULT_HOLD_MS <= 1000, "{RESULT_HOLD_MS}");
        assert!(RESULT_HOLD_MS >= BYE_FLUSH_MS);
    }
    use super::*;

    #[test]
    fn a_corpse_stays_dead_until_a_later_looping_main_clip_or_its_player_leaves() {
        let mut c = Corpses::default();
        c.killed(72, 1000);
        assert!(c.is_dead(72) && !c.is_dead(74));
        // the block of the death tick itself (same arrival time) and the corpse's non-looping death clip keep it dead
        c.animated(72, 1000, Some(true));
        c.animated(72, 1033, Some(false));
        c.animated(72, 1066, None);
        assert!(c.is_dead(72));
        // the spawn: a later block with a looping main clip
        c.animated(72, 8100, Some(true));
        assert!(!c.is_dead(72));
        assert_eq!(c.revived, 1);
        // a player who leaves while dead
        c.killed(74, 9000);
        c.forget(74);
        assert_eq!(c.count(), 0);
    }

    #[test]
    fn a_respawned_remote_is_never_drawn_at_its_stale_position() {
        // an enemy runs, dies at (500, 0, 500), lies there; the server's respawn change record places it at the
        // spawn point with CF_POS_SNAP (no stream entry yet), then its stream goes on from the spawn point
        use dc_botclient::objects::ClientObject;
        let spawn = [-800.0f32, 0.0, 120.0];
        let corpse = [500.0f32, 0.0, 500.0];
        let mut o = ClientObject::default();
        let mut t = Track::default();
        let mut now = 0u32;
        for i in 0..10 {
            o.stream_entries += 1;
            o.pos = Some([400.0 + 10.0 * i as f32, 0.0, 500.0]);
            t.follow_object(&o, now);
            now += 50;
        }
        o.stream_entries += 1;
        o.pos = Some(corpse);
        t.follow_object(&o, now);
        // dead for 7 s: no stream entries
        now += 7_000;
        // the respawn record
        o.pos_snaps += 1;
        o.pos = Some(spawn);
        let respawn_at = now;
        let near = |p: [f32; 3], q: [f32; 3]| (0..3).all(|i| (p[i] - q[i]).abs() < 1.0);
        // every drawn frame (1 ms apart, the interpolation delay behind) from the respawn record on
        for frame in 0..400u32 {
            now = respawn_at + frame;
            t.follow_object(&o, now);
            if frame % 50 == 49 {
                o.stream_entries += 1;
                o.pos = Some([spawn[0] + frame as f32 * 0.1, 0.0, spawn[2]]);
            }
            let drawn = t.draw(now).expect("a position");
            assert!(
                drawn[0] < -700.0 && !near(drawn, corpse),
                "frame {frame}: drawn at {drawn:?}, not at the spawn point"
            );
        }
    }

    #[test]
    fn a_player_who_leaves_first_keeps_its_track_in_the_record_but_is_no_longer_drawn() {
        // the walker (object 1) leaves after the result screen before the own session closes (DSV-35, 0x6c)
        let mut tracks = BTreeMap::new();
        let mut left = BTreeMap::new();
        let mut walker = Track {
            pid: 0,
            name: "Walker".into(),
            team: 2,
            ..Track::default()
        };
        walker.push(0, [0.0; 3]);
        walker.push(50, [30.0, 0.0, 40.0]);
        tracks.insert(1u16, walker);
        tracks.insert(5u16, Track::default());
        retire_tracks(&mut tracks, &mut left, &[1, 5]);
        assert_eq!(
            (tracks.len(), left.len()),
            (2, 0),
            "both still in the world"
        );
        retire_tracks(&mut tracks, &mut left, &[5]);
        assert!(!tracks.contains_key(&1), "a departed player is not drawn");
        let t = &left[&1];
        assert!(t.left && t.name == "Walker" && t.team == 2);
        assert_eq!(t.sample_count, 2);
        assert!((t.travel - 50.0).abs() < 1e-3);
        assert!(!tracks[&5].left);
    }

    /// A running stream: 30 samples, 33 ms apart on the server clock, arriving 60 ms later, moving +x at 300 u/s.
    fn running_track() -> (Track, u32) {
        let mut t = Track::default();
        let mut now = 0;
        for k in 0..30u32 {
            let s = k as f32 * 0.033;
            now = (s * 1000.0) as u32 + 60;
            t.push_stream(now, [300.0 * s, 0.0, 0.0], Some([300.0, 0.0, 0.0]), Some(s));
        }
        (t, now)
    }

    #[test]
    fn a_late_sample_is_bridged_along_the_velocity_for_at_most_100_ms() {
        let (mut t, now) = running_track();
        let newest = t.last.unwrap()[0];
        // a clean link: the two-interval floor (CLI-1330), 70 ms at 30 Hz
        assert!(
            (t.delay_ms() - (2.0 * t.gap_ms + LATE_MARGIN_MS)).abs() < 1e-3,
            "{}",
            t.delay_ms()
        );
        // no news for 120 ms: drawn 50 ms past the newest sample, along +x
        let p = t.draw(now + 120).unwrap();
        let over = 120.0 - t.playout_delay(now + 120);
        assert!((p[0] - (newest + 0.3 * over)).abs() < 0.5, "{p:?}");
        assert_eq!((t.total.late_runs, t.total.held_runs), (1, 0));
        // 400 ms: past the bound the body has eased back to the newest sample, counted once as a hold
        let a = t.draw(now + 400).unwrap();
        let b = t.draw(now + 401).unwrap();
        assert!((a[0] - newest).abs() < 1e-3 && a == b, "{a:?} {b:?}");
        assert_eq!(
            (t.total.late_runs, t.total.held_runs, t.total.held_frames),
            (1, 1, 2)
        );
    }

    #[test]
    fn past_the_extrapolation_bound_the_body_eases_back_to_the_newest_sample() {
        let (mut t, _) = running_track();
        let newest = t.last.unwrap()[0];
        let t1 = t.samples.back().unwrap().0;
        let d = t.playout_delay(t1 as u32 + 1000);
        // 150 ms over: halfway between the bound (newest + 30) and the newest sample
        let p = t.draw((t1 + d as f64 + 150.0) as u32).unwrap();
        assert!((p[0] - (newest + 15.0)).abs() < 1.0, "{p:?} vs {newest}");
        let p = t.draw((t1 + d as f64 + 400.0) as u32).unwrap();
        assert!((p[0] - newest).abs() < 1e-3, "{p:?}");
    }

    #[test]
    fn a_snap_and_its_first_stream_sample_do_not_seed_the_interval() {
        let mut t = Track::default();
        t.snap(1000.0, [0.0; 3]);
        t.push_stream(1000, [0.0; 3], None, Some(1.0));
        for k in 1..4u32 {
            t.push_stream(1000 + 100 * k, [0.0; 3], None, Some(1.0 + 0.1 * k as f32));
        }
        // a 10 Hz stream: the interval is 100 ms, not 0.5 ms (a delay stuck at the 50 ms floor)
        assert!((t.gap_ms - 100.0).abs() < 2.0, "{}", t.gap_ms);
        assert!(t.delay_ms() > 100.0, "{}", t.delay_ms());
    }

    #[test]
    fn a_latency_drop_does_not_crowd_the_stamps_together() {
        let mut t = Track::default();
        for k in 0..60u32 {
            let s = k as f32 * 0.033;
            // 100 ms of latency, from sample 30 on 40 ms
            let lat = if k < 30 { 100 } else { 40 };
            t.push_stream((s * 1000.0) as u32 + lat, [k as f32, 0.0, 0.0], None, Some(s));
        }
        let stamps: Vec<f64> = t.samples.iter().map(|x| x.0).collect();
        let min_step = stamps.windows(2).map(|w| w[1] - w[0]).fold(f64::MAX, f64::min);
        assert!(min_step > 20.0, "stamps crowd to {min_step} ms: {stamps:?}");
        // and the map has followed the drop down to the new latency
        for k in 60..140u32 {
            let s = k as f32 * 0.033;
            t.push_stream((s * 1000.0) as u32 + 40, [k as f32, 0.0, 0.0], None, Some(s));
        }
        assert!((t.clock_base().unwrap() - 40.0).abs() < 1.0, "{:?}", t.clock_base());
    }

    /// A latency rise is followed at the bounded rate too: no stamp step stretches by the whole rise.
    #[test]
    fn a_latency_rise_is_followed_at_a_bounded_rate() {
        let mut t = Track::default();
        for k in 0..400u32 {
            let s = k as f32 * 0.033;
            // 40 ms of latency, from sample 30 on 100 ms
            let lat = if k < 30 { 40 } else { 100 };
            t.push_stream((s * 1000.0) as u32 + lat, [k as f32, 0.0, 0.0], None, Some(s));
            if k == 31 {
                let stamps: Vec<f64> = t.samples.iter().map(|x| x.0).collect();
                let max_step = stamps.windows(2).map(|w| w[1] - w[0]).fold(0.0, f64::max);
                assert!(max_step < 40.0, "a stamp step stretched to {max_step} ms: {stamps:?}");
            }
        }
        assert!((t.clock_base().unwrap() - 100.0).abs() < 1.0, "{:?}", t.clock_base());
    }

    /// A clean 30 Hz stream is drawn two intervals behind (one lost update never runs the buffer dry).
    #[test]
    fn the_delay_floor_is_two_intervals() {
        let mut t = Track::default();
        for k in 0..40u32 {
            let s = k as f32 * 0.033;
            t.push_stream((s * 1000.0) as u32 + 60, [0.0; 3], None, Some(s));
        }
        assert!((t.delay_ms() - (2.0 * t.gap_ms + LATE_MARGIN_MS)).abs() < 1e-3, "{}", t.delay_ms());
        assert!(t.delay_ms() > 68.0 && t.delay_ms() < 72.0, "{}", t.delay_ms());
    }

    /// The smoothness statistics: a steady run has no speed spread, a reversed step is a backstep, a long step a
    /// jump.
    #[test]
    fn drawn_motion_statistics_count_backsteps_and_jumps() {
        let (mut t, now) = running_track();
        // samples past the drawn span (but within TRACK_KEEP), so every frame interpolates
        for k in 30..50u32 {
            let s = k as f32 * 0.033;
            t.push_stream((s * 1000.0) as u32 + 60, [300.0 * s, 0.0, 0.0], Some([300.0, 0.0, 0.0]), Some(s));
        }
        for f in 0..60u32 {
            t.draw(now + 200 + f * 8);
        }
        let v = t.take_report(now + 680, 10_000);
        assert!(v["moving_frames"].as_u64().unwrap() >= 50, "{v}");
        assert!(v["speed_cv"].as_f64().unwrap() < 0.05, "{v}");
        assert_eq!((v["backsteps_per_min"].as_f64().unwrap(), v["jumps_per_min"].as_f64().unwrap()), (0.0, 0.0));
        // a body drawn back and forth (8 units per 8 ms frame, a running speed): every step reverses the previous
        let mut t = Track::default();
        for f in 0..40u32 {
            let x = if f % 2 == 0 { 0.0 } else { 8.0 };
            t.note_draw(f * 8, [x, 0.0, 0.0]);
        }
        assert!(t.window.backsteps >= 30, "{}", t.window.backsteps);
        // a steady run with one long step (six times the mean, still a running speed): one jump, no backstep
        let mut t = Track::default();
        for f in 0..60u32 {
            let x = f as f32 * 2.0 + if f >= 50 { 10.0 } else { 0.0 };
            t.note_draw(f * 8, [x, 0.0, 0.0]);
        }
        assert_eq!((t.window.jumps, t.window.backsteps), (1, 0), "{:?}", t.window);
        // a respawn placement (far beyond any running speed) is no motion at all
        let mut t = Track::default();
        t.note_draw(0, [0.0; 3]);
        t.note_draw(8, [5000.0, 0.0, 0.0]);
        t.note_draw(16, [5002.0, 0.0, 0.0]);
        assert_eq!((t.window.moving_frames, t.window.jumps), (1, 0), "{:?}", t.window);
    }

    #[test]
    fn a_stalled_server_clock_is_tried_again_after_a_few_seconds() {
        let mut t = Track::default();
        t.push_stream(100, [0.0; 3], None, Some(1.0));
        for k in 1..=9u32 {
            t.push_stream(100 + 33 * k, [0.0; 3], None, Some(1.0));
        }
        assert!(t.arrival_clock, "eight flat time entries: arrival clock");
        // flat entries are no usable step but not counted as stale messages
        assert_eq!(t.total.stale, 0);
        t.push_stream(1_000, [1.0; 3], None, Some(1.0));
        assert!(t.arrival_clock, "not before the retry time");
        t.push_stream(4_000, [2.0; 3], None, Some(1.0));
        assert!(t.arrival_clock, "the time still stands");
        t.push_stream(4_033, [3.0; 3], None, Some(1.033));
        assert!(!t.arrival_clock, "the time advances again");
        assert_eq!(t.take_report(4_033, 1_000)["server_clock"], true);
    }

    #[test]
    fn velocity_zero_or_a_snap_stops_the_extrapolation() {
        // DSV-21: the server's velocity-0 record of a stale player
        let (mut t, now) = running_track();
        let last = t.last.unwrap();
        t.push_stream(now + 33, last, Some([0.0; 3]), Some(30.0 * 0.033));
        assert_eq!(t.draw(now + 500), Some(last));
        assert_eq!(t.total.late_frames, 0);
        // VIS-13: a snap placement: drawn there, nothing carried from before
        let (mut t, now) = running_track();
        t.snap(now as f64, [-800.0, 0.0, 0.0]);
        assert_eq!(t.draw(now + 500), Some([-800.0, 0.0, 0.0]));
        assert_eq!(t.total.late_frames, 0);
    }

    #[test]
    fn jitter_raises_the_delay_by_the_largest_lateness_within_the_bounds() {
        let mut t = Track::default();
        for k in 0..64u32 {
            let s = k as f32 * 0.033;
            // every 8th sample 50 ms late and the one after it 20 ms (a queueing spike; no reordering)
            let late = match k % 8 {
                7 => 50,
                0 => 20,
                _ => 0,
            };
            t.push_stream((s * 1000.0) as u32 + 60 + late, [0.0; 3], None, Some(s));
        }
        assert!((t.late_q - 50.0).abs() < 1.5, "{}", t.late_q);
        let want = t.gap_ms + 50.0 + LATE_MARGIN_MS;
        assert!(
            (t.delay_ms() - want).abs() < 2.0,
            "{} vs {want}",
            t.delay_ms()
        );
        // the drawn delay rises toward it at 10 % of real time, never in a step
        let d0 = t.playout_delay(t.delay_at);
        assert!(t.playout_delay(t.delay_at + 10) - d0 <= 1.0 + 1e-3);
        assert!((t.playout_delay(t.delay_at + 2_000) - t.delay_ms()).abs() < 1e-3);
        // a huge jitter is capped
        t.late_q = 900.0;
        assert_eq!(t.delay_ms(), INTERP_MAX_MS);
    }

    /// A single spike stays covered for the whole lateness window (256 samples), not only for the clock-map window
    /// (64): a Wi-Fi link whose spikes come back every few seconds stops hitching each time.
    #[test]
    fn one_late_spike_keeps_the_delay_up_for_the_lateness_window() {
        let mut t = Track::default();
        let at = |k: u32| (k as f32 * 0.033 * 1000.0) as u32 + 60;
        let srv = |k: u32| Some(k as f32 * 0.033);
        for k in 0..10u32 {
            t.push_stream(at(k), [0.0; 3], None, srv(k));
        }
        // one sample 25 ms late (less than the interval: the next one still arrives after it)
        t.push_stream(at(10) + 25, [0.0; 3], None, srv(10));
        assert!((t.late_q - 25.0).abs() < 1.5, "{}", t.late_q);
        for k in 11..(10 + LATE_STAT_WINDOW as u32) {
            t.push_stream(at(k), [0.0; 3], None, srv(k));
            assert!((t.late_q - 25.0).abs() < 1.5, "sample {k}: {}", t.late_q);
        }
        assert_eq!(t.total.samples as usize, 10 + LATE_STAT_WINDOW);
        // the spike leaves the window: the lateness is back to ~0 and CLI-11's value wins
        let k = 10 + LATE_STAT_WINDOW as u32;
        t.push_stream(at(k), [0.0; 3], None, srv(k));
        assert!(t.late_q < 1.5, "{}", t.late_q);
        // CLI-11 / the two-interval floor win
        assert!((t.delay_ms() - (2.0 * t.gap_ms + LATE_MARGIN_MS)).abs() < 1e-3);
    }

    /// A lost or skipped update doubles the server step: the delay covers that step, not only the mean interval
    /// (2026-10-31: a tenth of the updates missing over Wi-Fi left 5..8 % of the frames late at a 72 ms delay).
    #[test]
    fn a_missing_update_raises_the_delay_to_the_doubled_step() {
        let mut t = Track::default();
        let at = |k: u32| (k as f32 * 0.033 * 1000.0) as u32 + 60;
        let srv = |k: u32| Some(k as f32 * 0.033);
        for k in 0..40u32 {
            if k % 10 == 7 {
                continue; // lost
            }
            t.push_stream(at(k), [0.0; 3], None, srv(k));
        }
        // the longest wait is two steps (66 ms): the delay is that plus the margin, or the two-interval floor
        // (the lost steps lift the mean interval a little)
        let want = (66.0 + LATE_MARGIN_MS).max(2.0 * t.gap_ms + LATE_MARGIN_MS);
        assert!((t.delay_ms() - want).abs() < 2.0, "{} vs {want}", t.delay_ms());
        assert!((t.late_q - (66.0 - t.gap_ms)).abs() < 2.0, "{}", t.late_q);
        // a culled player's silence (693 ms) is neither jitter nor an interval: the delay stays
        t.push_stream(at(60), [0.0; 3], None, srv(60));
        assert!((t.delay_ms() - want).abs() < 2.0, "{} vs {want}", t.delay_ms());
        assert!(t.gap_ms < 40.0, "{}", t.gap_ms);
        // a snap forgets the waits
        t.snap(at(61) as f64, [1.0; 3]);
        t.push_stream(at(62), [1.0; 3], None, srv(62));
        assert!(t.late_q < 1.5, "{}", t.late_q);
    }

    /// A 15 Hz reporter on a 30 Hz stream: every other sample repeats the position. Drawn as is, the body moved
    /// for a tick and stood for a tick (half the samples repeated in a local match, 2026-10-31); placed along the
    /// velocity the drawn speed is steady.
    #[test]
    fn a_repeated_position_of_a_moving_player_is_placed_along_its_velocity() {
        let mut t = Track::default();
        let v = Some([300.0, 0.0, 0.0]);
        let mut now = 0;
        for k in 0..60u32 {
            let s = k as f32 * 0.033;
            now = (s * 1000.0) as u32 + 60;
            // the reported position advances every second tick only
            let reported = 300.0 * ((k / 2) as f32 * 0.066);
            t.push_stream(now, [reported, 0.0, 0.0], v, Some(s));
        }
        assert_eq!(t.total.repeats, 30, "{}", t.total.repeats);
        assert_eq!(t.total.reckoned, 30);
        // the kept samples advance every tick
        let xs: Vec<f32> = t.samples.iter().map(|s| s.1[0]).collect();
        for w in xs.windows(2) {
            assert!(w[1] - w[0] > 5.0, "a standing tick: {xs:?}");
        }
        // and the drawn motion is steady (drawn inside the kept samples, never past the newest)
        for f in 0..40u32 {
            t.draw(now - 400 + f * 8);
        }
        let r = t.take_report(now, 10_000);
        assert!(r["speed_cv"].as_f64().unwrap() < 0.15, "{r}");
        assert_eq!(r["backsteps_per_min"].as_f64().unwrap(), 0.0, "{r}");
        // a stop: the placed sample ahead of the playout point is straightened onto the reported stop (no
        // overshoot, no backstep); the reported position arrives 20 ms after its server time
        let mut t = Track::default();
        let at = |k: u32| (k as f32 * 0.033 * 1000.0) as u32 + 60;
        let srv = |k: u32| Some(k as f32 * 0.033);
        for k in 0..20u32 {
            let x = 300.0 * ((k / 2) as f32 * 0.066);
            t.push_stream(at(k), [x, 0.0, 0.0], v, srv(k));
        }
        let xk19 = t.samples.back().unwrap().1[0];
        let x_stop = 300.0 * (9.0 * 0.066);
        assert!(xk19 > x_stop + 5.0, "placed ahead: {xk19} vs {x_stop}");
        // the player stopped: sample 20 reports the same position with velocity 0
        t.push_stream(at(20), [x_stop, 0.0, 0.0], Some([0.0; 3]), srv(20));
        let n = t.samples.len();
        let placed = t.samples[n - 2].1[0];
        assert!((placed - x_stop).abs() < 1e-3, "straightened: {placed} vs {x_stop}");
        assert_eq!(t.reckoned_tail, 0);
        // a player pressed against a wall: the same position with its wish velocity every update; the body is
        // placed one step once and then holds (it never walks through the wall)
        let mut t = Track::default();
        for k in 0..30u32 {
            t.push_stream(at(k), [100.0, 0.0, 0.0], v, srv(k));
        }
        let xs: Vec<f32> = t.samples.iter().map(|s| s.1[0]).collect();
        let far = xs.iter().cloned().fold(0.0, f32::max);
        assert!(far <= 100.0 + 300.0 * RECKON_MAX_S + 1e-3, "drifted to {far}: {xs:?}");
        assert_eq!(t.total.reckoned, 1, "{}", t.total.reckoned);
        // a standing player (velocity 0, DSV-21) is never moved
        let mut t = Track::default();
        for k in 0..10u32 {
            let s = k as f32 * 0.033;
            t.push_stream((s * 1000.0) as u32 + 60, [5.0, 0.0, 0.0], Some([0.0; 3]), Some(s));
        }
        assert_eq!((t.total.repeats, t.total.reckoned), (0, 0));
        assert_eq!(t.last.unwrap(), [5.0, 0.0, 0.0]);
    }

    #[test]
    fn an_older_stream_message_arriving_late_is_dropped() {
        let (mut t, now) = running_track();
        let n = t.sample_count;
        t.push_stream(now + 5, [0.0; 3], None, Some(0.5));
        assert_eq!((t.sample_count, t.total.stale), (n, 1));
        assert_eq!(t.last.unwrap()[0], 300.0 * 29.0 * 0.033);
    }

    #[test]
    fn the_report_counts_the_window_and_clears_it() {
        let (mut t, now) = running_track();
        for f in 0..10 {
            t.draw(now + f);
        }
        let v = t.take_report(now + 10, 10_000);
        assert_eq!(v["samples"], 30);
        assert_eq!(v["frames"], 10);
        assert_eq!(v["server_clock"], true);
        assert_eq!(t.window, PlayoutStats::default());
        assert_eq!(t.total.frames, 10);
    }

    #[test]
    fn track_interpolates_between_samples_and_holds_outside() {
        let mut t = Track::default();
        assert_eq!(t.at(0.0), None);
        t.push(100, [0.0, 0.0, 0.0]);
        t.push(200, [10.0, 0.0, 0.0]);
        t.push(300, [10.0, 0.0, 20.0]);
        assert_eq!(t.at(50.0), Some([0.0, 0.0, 0.0]));
        assert_eq!(t.at(150.0), Some([5.0, 0.0, 0.0]));
        assert_eq!(t.at(250.0), Some([10.0, 0.0, 10.0]));
        assert_eq!(t.at(999.0), Some([10.0, 0.0, 20.0]));
        assert!((t.gap_ms - 100.0).abs() < 1e-3);
        // two intervals + the margin (204) beat CLI-11's 150 at 10 Hz
        assert!((t.delay_ms() - (200.0 + LATE_MARGIN_MS)).abs() < 1e-3, "{}", t.delay_ms());
        assert!((t.travel - 30.0).abs() < 1e-4);
        t.push(400, [f32::NAN, 0.0, 0.0]);
        assert_eq!(t.sample_count, 3, "non-finite samples are dropped");
        // the wait window (LATE_STAT_WINDOW samples) forgets the slow stream's waits before the bound applies
        for i in 0..300 {
            t.push(400 + i * 10, [0.0; 3]);
        }
        assert_eq!(t.samples.len(), TRACK_KEEP);
        assert!(
            (t.delay_ms() - INTERP_MIN_MS).abs() < 1.0,
            "fast stream: the lower bound"
        );
    }

    #[test]
    fn hud_lines_show_state_scores_kills_and_the_end() {
        let mut h = Hud {
            life_state: 1,
            health: Some(75.0),
            max_health: Some(100.0),
            team_score: [3, 1],
            timer: Some((90, 1000, true)),
            ..Hud::default()
        };
        assert_eq!(h.time_left(31_000), Some(60));
        let names = |p: u8| ["Ann", "Bob"][p as usize].to_string();
        let k = KillRecord {
            victim: 1,
            killer: 0,
            weapon: 12,
            hit_zone: 1,
            ..KillRecord::default()
        };
        h.kill_feed.push_back(kill_line(&k, &names));
        let world = KillRecord {
            victim: 0,
            killer: NO_KILLER,
            ..KillRecord::default()
        };
        assert_eq!(kill_line(&world, &names), "(world) [weapon 0] Ann");
        h.round_result = Some((2, [0, 1]));
        h.result_screen = Some((false, 0));
        h.level_exit = Some((String::new(), false, true));
        let l = h.lines(&NetPhase::InWorld, 2, 31_000);
        assert!(
            l[0].contains("state 1 (alive)") && l[0].contains("HP 75/100"),
            "{l:?}"
        );
        assert!(
            l[1].contains("score 3:1") && l[1].contains("time 1:00"),
            "{l:?}"
        );
        assert_eq!(l[2], "Ann [weapon 12] Bob (head)");
        assert!(l.iter().any(|x| x.starts_with("ROUND OVER: team 2 wins")));
        assert!(l.iter().any(|x| x.starts_with("MATCH RESULT")));
        assert!(l.iter().any(|x| x == "LEVEL EXIT: back to the menus"));
    }

    #[test]
    fn leave_sprint_and_move_notice_forms_are_the_2009_clients() {
        // OBSERVED forms: lobby 0x394 body `94030a00000000000000` (tdm4), 0xca `0b ca 02` / `0b ca 01` (18 bits with
        // the ids), 0x76 [1][e] (32 bits with the ids)
        let f = leave_notice().to_frame();
        assert_eq!(&f[8..], &[0x94, 0x03, 0x0a, 0, 0, 0, 0, 0, 0, 0]);
        let stop = sprint_body((false, true));
        assert_eq!((stop.bit_len(), stop.as_bytes()), (2, &[0x02u8][..]));
        let start = sprint_body((true, false));
        assert_eq!(start.as_bytes(), &[0x01]);
        let jump = move_notice_body(1);
        assert_eq!((jump.bit_len(), jump.as_bytes()), (16, &[0x01u8, 0x01][..]));
    }

    #[test]
    fn update_while_firing_has_the_2009_flag_and_control_words() {
        // OBSERVED: CAPTURE_tdm4_WEAPON_FORMS.md §4: head `0b6421332b00000000d7708a3f00000000202a5e3c9d7b0f`
        // (pu_flags 0x3321, yaw8c 0x2b, body angles (0, 1.0816, 0), aux[0] = the view pitch; control 0x600 at bit
        // 584). Our aux[1] (view yaw offset) is 0, so the bytes are compared up to aux[0].
        let f = |h: u32| f32::from_bits(h);
        let mut u = LocalUpdate {
            body_angles: [0.0, f(0x3f8a_70d7), 0.0],
            aim: [0.0, 0.5, 0.0, 0.866],
            pitch: f(0x3c5e_2a20),
            yaw_offset: 0.0,
            eye_offset: [0.0, 33.0, 0.0],
            control: 0x200,
            move_seq: 2,
            pos: [-1468.163, 671.999, 1104.178],
            vel: [0.0; 3],
            time: 75.4,
            on_ground: true,
            surface: 0,
            standing: u64::MAX,
        };
        let (w, mut x) = update_with_weapon(&u, Some((0.0488, true)));
        x.cam_point = None;
        let b = dc_botclient::shell::shell_msg(
            0x64,
            &dc_botclient::shell::player_update_body_ext(&w, &x),
        );
        let hex: String = b.as_bytes()[..21]
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect();
        assert_eq!(hex, "0b6421332b00000000d7708a3f00000000202a5e3c");
        let mut r = b.reader();
        r.seek_bits(584);
        assert_eq!(r.read_u32(), 0x600, "control with the fire bit");
        assert_eq!(b.bit_len(), 928, "the observed size of this form");
        // between shots: 0x1321 / 0x200
        u.control = 0x200;
        let (w, mut x) = update_with_weapon(&u, Some((0.0263, false)));
        x.cam_point = None;
        let b = dc_botclient::shell::shell_msg(
            0x64,
            &dc_botclient::shell::player_update_body_ext(&w, &x),
        );
        assert_eq!(&b.as_bytes()[2..4], &[0x21, 0x13]);
        // no weapon: no float, no fire bits
        let (w, x) = update_with_weapon(&u, None);
        assert_eq!(
            (x.weapon_f, x.flag_bits & 0x2000, w.control),
            (None, 0, 0x200)
        );
    }

    #[test]
    fn announced_names_map_to_rom_map_names() {
        assert_eq!(
            map_name_of("WORLDS\\Alpha_Two.DAT").as_deref(),
            Some("ALPHA_TWO")
        );
        assert_eq!(map_name_of("Worlds/Beta").as_deref(), Some("BETA"));
        assert_eq!(map_name_of(""), None);
        assert_eq!(team_colour(1), TEAM_COLOURS[0]);
        assert_eq!(team_colour(0xff), TEAM_COLOURS[2]);
    }
}
