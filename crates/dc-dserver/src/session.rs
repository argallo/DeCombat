//! The game-session state machine of the dedicated server, sans-IO and deterministic: datagrams and control lines
//! go in with the caller's time, transmissions, pipe events and log lines come out.
//!
//! Per client it runs the start-up of `UDP_SESSION_2009_A.md` §9 (steps S1..S11) with the 2009 host's gates
//! (§5.7, §8.2); slice 2 adds S12 and the game layer ([`crate::game`]): the other players' objects (create and
//! removal records), their positions in the per-tick object stream (`UDP_SESSION_2009_B.md` §4.3, §9.1), and the
//! client's game messages routed to the TDM rules.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Display;

use crate::game::{CharacterLook, Game, GameOut, GameTuning, Milestone};
use crate::modelobj::{self, CharacterSpec, ModelEntry, Tracker};
use crate::msg::{self, FileEntry, FromClient, Hello, SHELL_PLAYER_UPDATE};
use crate::objects::{self, CreateSpec, PosBox, PosForm, OBJ_PLAIN};
use crate::pipe::{self, Control, RoomSettings, RosterEntry};
use crate::rom_tables::RomTables;
use crate::shell64::{parse_shell64, Shell64Update};
use crate::stream;
use crate::up::{self, Up};
use crate::world::WorldInfo;
use dc_net_engine::time::{later, since};
use dc_net_engine::{Bye, CloseCause, Host, HostConfig, HostEvent, Millis};
use dc_wire::udp::BitWriter;

/// File id of the world in the session file table (any unique u16 works; the spec suggests W = 1).
// SPEC: UDP_SESSION_2009_A.md §9.1 P3
pub const WORLD_FILE_ID: u16 = 1;
/// LG-327: refused seats logged one by one in a match.
const SEAT_REFUSALS_LOGGED: u32 = 20;
/// First object id handed out (lowest free id from here, CD-66 proposed). 0 is avoided as a DeCombat choice.
// DC-DEFAULT: CD-66 (proposed) lowest free id; start value DSV-3 (staged proposal, dserver-slice1)
pub const FIRST_OBJECT_ID: u16 = 1;
/// Client ids are the lowest free id below this limit. The 2009 host allows 0..29999, but the game layer sends the
/// client id as the `u8` player id of every roster message and uses 0xff as "no killer", so ids stay below 255.
// SPEC: UDP_SESSION_2009_A.md §4.3 (host engine+0x8e9f0); GAME_MESSAGES_2009_S2C.md §1.1, §3.1 (ids < 256),
// §3.3.1 field 2 (0xff = no killer)
pub const CLIENT_ID_LIMIT: u16 = 255;
/// The switching-worlds state sent in shell 0x88. The in-world test accepts 2 or 3; which one the 2009 server
/// sends is UNPROVEN (V-A4).
// SPEC: UDP_SESSION_2009_A.md §4.11, §9.2 S11
pub const TODO_SWITCH_STATE: u8 = 2;
/// Whether `S2C_SKY_TABLE` (S8) is sent: the world's sky table (its sky models' slots and sky box) when the world
/// has a sky model among the created world objects.
// SPEC: UDP_SESSION_2009_A.md §4.8, §9.2 S8; RENDER_2009.md §4.4, §5.3 (the sky is drawn only from sky objects of
// the table, through the sky box); OBSERVED: capture 20261025_081704_rust3 (no table and no sky
// models: the real client drew the sky "very oddly")
// DC-DEFAULT: DSV-45 (proposed) on
pub const TODO_SEND_SKY_TABLE: bool = true;
/// How long the server waits for the client's `C2S_BYE` after its `UP_LEAVE_MATCH` (0xbd) before it closes the link
/// itself; either way with `CTL_BYE_CODE(5)`, never with the plain bye first.
// SPEC: UDP_TRANSPORT_2009.md §9.1 (the host answers the client's bye with code 5); UDP_SESSION_2009_A.md §5.6
// OBSERVED: capture 20261025_065016_tdm6 (the client's `C2S_BYE` 15 ms after its 0xbd, then the
// reference stack's `CTL_BYE_CODE(5)` x4; REFERENCE); capture 20261025_081704_rust3 (our plain bye x4 at the
// 0xbd, before the client's `C2S_BYE`: accepted, but not the 2009 order)
// DC-DEFAULT: DSV-51 (proposed) wait at most 1000 ms
pub const EXIT_BYE_WAIT_MS: u32 = 1000;
/// NET-23: a link moves to a new address only after its old one was silent this long (a client sends player updates
/// and acks many times a second, so a rebinding NAT leaves the old address silent at once).
// DC-DEFAULT: NET-23 (proposed) 250 ms
pub const ADDRESS_MOVE_SILENCE_MS: u32 = 250;
/// Default of [`ServerTuning::silent_drop_ms`].
// DC-DEFAULT: DSV-490 (proposed, project decision) 20 s
pub const SILENT_DROP_MS: u32 = 20_000;
/// Bye code of the close after a leave.
// SPEC: UDP_SESSION_2009_A.md §5.6 (C2S_BYE -> drop with bye code 5)
pub const EXIT_BYE_CODE: u32 = 5;
/// Largest object group of the world-file objects in bits (the records are split over several groups).
// OBSERVED: capture 20261025_002236_tdm4 (the reference stack's initial groups to the real client held up
// to 64,066 bits each; REFERENCE, accepted)
// DC-DEFAULT: DSV-37 (extended, staged) 60000 bits per group
pub const WORLD_GROUP_BITS: usize = 60_000;
/// Object kind of the player's own object. The view object only has to exist in the client object map
/// (in-world test a); which kind and model the 2009 server used for players is game-layer data not yet specified.
// SPEC: UDP_SESSION_2009_A.md §4.9, §8.3 a; UDP_SESSION_2009_B.md §3.3
// DC-DEFAULT: DSV-8 (staged proposal)
pub const TODO_PLAYER_OBJ_KIND: u8 = OBJ_PLAIN;
/// Position form of the player's create record (full precision avoids the UNPROVEN `pos48` bounds).
// SPEC: UDP_SESSION_2009_B.md §4.2 items 3 and 6; UDP_TRANSPORT_2009.md §15.1 (bounds UNPROVEN)
pub const TODO_PLAYER_POS_FORM: PosForm = PosForm::FullPrecision;

pub use crate::game::StartOrder;

/// Who may run remote console commands (`C2S_CONSOLE`). dc-dserver has no engine console yet, so a moderator's
/// command is logged as received and not executed; everyone else's is ignored and logged.
// DC-DEFAULT: CD-64 (approved) moderators only (the lobby marks them with `mod=1`, OPS-11 proposed)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteConsole {
    Moderators,
    Nobody,
}

/// Server settings (each one a DeCombat default, a proposed CD row or a spec value).
#[derive(Debug, Clone, PartialEq)]
pub struct ServerTuning {
    /// World ticks per second.
    // DC-DEFAULT: CD-63 (proposed) 10 ticks per second
    pub tick_hz: u32,
    /// Rate announced in `S2C_VERSION`.
    // DC-DEFAULT: CD-58 (proposed) 5120000
    pub host_rate_bps: u32,
    /// A client that has not reached the world within this time after its join is dropped with bye code 3.
    // DC-DEFAULT: CD-61 (proposed) 120 s, bye code 3
    pub startup_deadline_ms: u32,
    /// A linked client from which no datagram at all arrived for this long is dropped with bye code 3 through the
    /// normal leave path (`@leave`, its objects removed on the other clients); `None` = only the transport's 90 s idle
    /// drop. A live client sends at least a heartbeat ack every 200 ms (UDP_TRANSPORT_2009.md §11.1), so this
    /// silence means the client is gone (a crashed or killed process sends no bye).
    // DC-DEFAULT: DSV-490 (proposed, project decision) 20 s, bye code 3
    pub silent_drop_ms: Option<u32>,
    /// Bye code for a hello whose name is not in the roster.
    // DC-DEFAULT: CD-60 (proposed) match by UTF-16 name, refuse with bye code 6
    pub refuse_code: u32,
    /// Accept clients whose hello name is not in the roster (local experiments; off for lobby use).
    pub accept_unlisted: bool,
    /// Stop after this long with no client (`-idle`); `None` = never.
    // SPEC: DSERVER_PIPE.md §2 (`-idle`), §6 (idle with zero clients)
    pub idle_ms: Option<u32>,
    pub send_sky_table: bool,
    /// After the preload end mark, `S2C_PRELOAD` sub 5 with the world sounds' files (`-preloadsounds` /
    /// `-nopreloadsounds`).
    // DC-DEFAULT: DSV-65 (proposed) on
    pub preload_sounds: bool,
    pub switch_state: u8,
    pub player_kind: u8,
    pub player_pos_form: PosForm,
    /// Info reply session name.
    pub session_name: String,
    /// DeCombat link limit (`-maxplayers`), `None` = no limit as on the 2009 host.
    pub max_players: Option<usize>,
    /// Game rules (slice 2; the start-up order of slice 3 is `game.start_order`).
    pub game: GameTuning,
    /// World-file objects created on every client (`-worldobjects off|mode|all`) and the class their zones name
    /// (`-zoneclass own|room`).
    // DC-DEFAULT: DSV-37 (proposed) mode, own
    pub world_objects: crate::worldobj::WorldObjects,
    pub zone_class: crate::worldobj::ZoneClass,
    /// Send the world's global light after the world load (`-globallight world|off`).
    // DC-DEFAULT: DSV-40 (proposed) world
    pub global_light: crate::worldlight::LightMode,
    // DC-DEFAULT: CD-64 (approved)
    pub remote_console: RemoteConsole,
    /// Visibility culling (`-visibility`, `[vis]`).
    // DC-DEFAULT: VIS-1 (proposed) on
    pub vis: crate::visibility::VisTuning,
    /// The world-settings variables at the world load (`-evars world|off`) and the operator's own variables
    /// (`-evar name=value`, sent after them).
    // DC-DEFAULT: DSV-47 (proposed) world, no others
    pub evars: crate::evars::EvarMode,
    pub extra_evars: Vec<(String, String)>,
    /// Kill volumes kill on the server (`-killvolumes on|off`).
    // DC-DEFAULT: DSV-52 (proposed) on
    pub kill_volumes: bool,
    /// The world's sliding doors are created and move (`-doors on|off`).
    // DC-DEFAULT: DSV-46 (proposed) on
    pub doors: bool,
    /// The client's step height pushed before the world load (`-stepheight <units>|off`; `None` = not sent).
    // DC-DEFAULT: DSV-53 (proposed) 30
    pub step_height: Option<f32>,
    /// The client's world-north angle pushed before the world load (`-worldnorth <radians>|off`; `None` = not
    /// sent; `-evars off` sends it neither).
    // DC-DEFAULT: DSV-60 (proposed) 0
    pub world_north: Option<f32>,
    /// Start the world file's switched-on sounds at the world load (`-worldsounds on|off`).
    // DC-DEFAULT: DSV-56 (proposed) on
    pub world_sounds: bool,
    /// The world's camera objects are created and the room's trigger scripts move them (`-camshot on|off`).
    // DC-DEFAULT: DSV-112 (proposed) on
    pub cam_shot: bool,
}

impl Default for ServerTuning {
    fn default() -> Self {
        ServerTuning {
            tick_hz: 10,
            host_rate_bps: dc_wire::udp::TODO_HOST_RATE_BPS,
            startup_deadline_ms: 120_000,
            silent_drop_ms: Some(SILENT_DROP_MS),
            refuse_code: 6,
            accept_unlisted: false,
            idle_ms: Some(60_000),
            send_sky_table: TODO_SEND_SKY_TABLE,
            preload_sounds: true,
            switch_state: TODO_SWITCH_STATE,
            player_kind: TODO_PLAYER_OBJ_KIND,
            player_pos_form: TODO_PLAYER_POS_FORM,
            session_name: "DeCombat".into(),
            max_players: None,
            game: GameTuning::default(),
            world_objects: crate::worldobj::WorldObjects::World,
            zone_class: crate::worldobj::ZoneClass::Own,
            global_light: crate::worldlight::LightMode::World,
            remote_console: RemoteConsole::Moderators,
            vis: crate::visibility::VisTuning::default(),
            evars: crate::evars::EvarMode::World,
            extra_evars: Vec::new(),
            kill_volumes: true,
            doors: true,
            step_height: Some(crate::clientvars::DEFAULT_STEP_HEIGHT),
            world_north: Some(crate::clientvars::DEFAULT_WORLD_NORTH),
            world_sounds: true,
            cam_shot: true,
        }
    }
}

/// Something the caller has to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output<A> {
    /// Send a datagram.
    Transmit { to: A, bytes: Vec<u8> },
    /// Write an event line to stdout (starts with `@`).
    Pipe(String),
    /// Host log line (never starts with `@`).
    Log(String),
}

/// Milestones of one client, named after the slice slice list.
// SPEC: UDP_SESSION_2009_A.md §9.2 (S1..S13)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    /// Join accepted, link created.
    Linked,
    /// S1 `C2S_HELLO` received.
    Hello,
    /// S2 `S2C_VERSION` queued.
    Version,
    /// S3 `S2C_CLIENT_ID` queued.
    ClientId,
    /// S4 `S2C_FILE_LIST` queued.
    FileList,
    /// S4a every announced file answered.
    FilesAcked,
    /// S6 `S2C_LOAD_WORLD` queued.
    LoadWorld,
    /// S6a `C2S_LOAD_STATE(0)`.
    WorldLoaded,
    /// S7 `S2C_PRELOAD` start + end queued.
    Preload,
    /// S8 `S2C_SKY_TABLE` queued (optional).
    SkyTable,
    /// S8a `C2S_LOAD_STATE(1)`.
    PreloadDone,
    /// S9 `S2C_OBJ_GROUP` with the player's create record queued.
    ObjectCreated,
    /// S10 `S2C_VIEW_OBJECT` queued.
    ViewObject,
    /// S11 shell 0x88 queued.
    SwitchState,
    /// S13 first time entry queued.
    FirstTick,
    /// First player update (shell 0x64) parsed.
    FirstUpdate,
    /// Game layer: `GS_IDENTIFY_REQUEST` queued (A2).
    IdentifyRequest,
    /// The client's `UP_PLAYER_INFO` accepted.
    PlayerInfo,
    /// Roster and match settings queued (A3).
    Introduced,
    /// S12: `GS_PLAYER_STATE` 3 queued (the in-world test's condition d).
    WaitingState,
    /// The client's `UP_WORLD_READY` (0x87).
    WorldReady,
    /// Spawn set ending in `GS_PLAYER_STATE` 1 queued.
    Spawned,
}

/// The 2009 host's per-client phases.
// SPEC: UDP_SESSION_2009_A.md §8.2
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Phase 1: waiting for the hello and the file answers.
    Linked,
    /// Phase 2 / 0: world sent, waiting for `C2S_LOAD_STATE(0)`.
    WorldSent,
    /// Phase 2 / 2: preload sent, waiting for `C2S_LOAD_STATE(1)`.
    Preloading,
    /// Phase 3: in world.
    InWorld,
}

/// What the server knows about one connected client.
#[derive(Debug, Clone)]
pub struct Client {
    pub client_id: u16,
    pub phase: Phase,
    pub joined_at: Millis,
    pub hello: Option<Hello>,
    /// Roster entry matched by the hello name.
    pub player: Option<RosterEntry>,
    pub files_pending: BTreeSet<u16>,
    pub object_id: Option<u16>,
    /// The weapon model object attached to the character (slice 3).
    pub weapon_obj: Option<u16>,
    /// The weapon model object was removed on every client at the player's death (it is created again at the
    /// respawn, DSV-44).
    pub weapon_gone: bool,
    pub spawn: Option<[f32; 3]>,
    /// From `C2S_RATE` (`rate_8k * 8000`).
    pub rate_bps: Option<u32>,
    pub updates: u64,
    pub last_update: Option<Shell64Update>,
    pub shell_counts: BTreeMap<u8, u64>,
    pub steps: Vec<(Step, Millis)>,
    pub ticks: u64,
    left_reported: bool,
    /// Shell 0x90 [0] / 0x88 [3] went out at the link (the round was already running).
    switch_ready_early: bool,
    /// When the client left the match (shell 0xbd); the link closes at its `C2S_BYE` or [`EXIT_BYE_WAIT_MS`] later.
    pub leaving_since: Option<Millis>,
    /// Link statistics for the log (round trip, resends, update rate and jitter).
    pub watch: crate::linkwatch::LinkWatch,
    /// Why the server is closing this link (the first cause wins), for `@linksum how=` (MON-1).
    leave_how: Option<pipe::LeaveHow>,
    /// `@linksum` went out for this link.
    sum_reported: bool,
}

impl Client {
    /// The player id of the game layer: the engine client id (always < 255, [`CLIENT_ID_LIMIT`]).
    pub fn pid(&self) -> u8 {
        self.client_id as u8
    }

    pub fn reached(&self, s: Step) -> Option<Millis> {
        self.steps.iter().find(|x| x.0 == s).map(|x| x.1)
    }

    fn mark(&mut self, s: Step, now: Millis) {
        if self.reached(s).is_none() {
            self.steps.push((s, now));
        }
    }
}

/// A player that was identified at some point of the match (for `@player`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Played {
    pub entry: RosterEntry,
}

/// The dedicated server's session layer.
pub struct Server<A: Ord + Clone + Display> {
    cfg: ServerTuning,
    host: Host<A>,
    world: WorldInfo,
    files: Vec<FileEntry>,
    roster: Vec<RosterEntry>,
    /// LG-327: hellos refused for a missing or wrong seat ticket.
    seat_refusals: u32,
    room: RoomSettings,
    clients: BTreeMap<A, Client>,
    objects_used: BTreeSet<u16>,
    start: Millis,
    next_tick: Millis,
    /// Tick schedule: tick `k` is due at `tick_base + k * 1000 / tick_hz` (exact rates such as 60 Hz).
    tick_base: Millis,
    tick_count: u64,
    last_activity: Millis,
    played: BTreeMap<u32, Played>,
    finished: bool,
    game: Game,
    out: VecDeque<Output<A>>,
    /// The world-file objects, planned at the first placement (the room line is known then).
    world_objs: Option<Vec<crate::worldobj::WorldObject>>,
    /// Object ids of the thrown objects in flight, by the game's flight key (DSV-39).
    projectiles: BTreeMap<u32, u16>,
    /// HiSec cases on the clients: case key -> object id (HS-2).
    cases: BTreeMap<u32, u16>,
    /// The create record of each case, for clients that enter the world while it lies there (HS-2 amendment).
    case_records: BTreeMap<u32, BitWriter>,
    /// Spy Hunt intel cases on the clients: key -> object id, and each create record for later clients (DSV-230).
    intels: BTreeMap<u32, u16>,
    intel_records: BTreeMap<u32, BitWriter>,
    /// Dropped weapons on the clients: drop key -> object id, and each create record for clients that enter the
    /// world while it lies there (DSV-93).
    drops: BTreeMap<u32, u16>,
    drop_records: BTreeMap<u32, BitWriter>,
    /// Death effects of destroyed breakables on the clients: effect key -> object id (DSV-118).
    world_fx: BTreeMap<u32, u16>,
    /// The planted bomb's names out of the ROM, found when a bomb room plans its world (DSV-220).
    bomb_look: Option<crate::bomblook::BombLook>,
    /// The planted bomb's objects on the clients (model, indicator effect, defuse point) and their create records for
    /// clients that enter the world meanwhile (DSV-220); the blast effect object (DSV-221).
    bomb_objs: Vec<u16>,
    bomb_records: Vec<BitWriter>,
    bomb_blast: Option<u16>,
    /// World effects switched by a bomb site's planted command (CLI-1340): planned world effect objects switched off
    /// (left out of the world records of a client that enters meanwhile), and effects switched on that the plan did not
    /// have (lower-case name -> object, create record).
    fx_hidden: BTreeSet<u16>,
    fx_extra: BTreeMap<Vec<u8>, (u16, BitWriter)>,
    /// Carried capture flags: flag object -> (carrier, the socket it hangs on) (DSV-217).
    flag_carried: BTreeMap<u16, (u8, u32)>,
    /// The brush of each breakable as the world created it, kept at its death for the round restore (RND-5).
    broken_brushes: BTreeMap<u16, crate::world::BrushInfo>,
    /// Event sound tracks on the clients and when each one is removed (DSV-64).
    event_tracks: Vec<(u16, Millis, Vec<A>)>,
    /// Visibility culling state (VIS-1, proposed).
    vis: crate::visibility::Visibility,
    /// Corpse objects on the clients and when each one is removed (`None` = kept, DSV-44).
    corpses: Vec<(u16, Option<Millis>)>,
    /// The kill volumes created on the clients (indexes into the world's kill shapes) and, per player and volume,
    /// since when the player is inside (DSV-52).
    kill_active: Vec<usize>,
    kill_inside: BTreeMap<(u8, usize), Millis>,
    /// The doors on the clients and the time of their last step (DSV-46).
    doors: Vec<crate::doors::Door>,
    doors_at: Option<Millis>,
    /// The rotating doors and switches on the clients (w1-doors DOOR-4).
    turns: Vec<crate::doors::TurnDoor>,
    /// The doors' delayed command statements (DSV-270).
    door_delays: crate::doors::DelayedStatements,
    /// The world's triggers the doors' and switches' statements fire, and the touched ones of a co-op room (DSV-562,
    /// DSV-563).
    door_triggers: crate::doors::Triggers,
    /// The triggers the start scripts fired (DSV-562 seed, again at each round, DSV-564).
    door_start_fired: Vec<Vec<u8>>,
    /// The round the doors' script state belongs to, and the planned models the statements changed (their state as
    /// planned; `None`: created by a statement) (DSV-564).
    door_round: u32,
    model_start: BTreeMap<u16, Option<crate::world::BrushInfo>>,
    /// The activate entry `(disabled, state)` of each door object as the clients have it (DSV-340).
    door_marks: BTreeMap<u16, (bool, u8)>,
    /// The world cameras on the clients and their pending trigger steps (DSV-112).
    camshot: crate::camshot::CamShot,
    /// The world's first `UP_WORLD_READY` arrived (reference order: every link got shell 0x90 [0] / 0x88 [3] then).
    world_started: bool,
    /// NET-23: the "same host" rule for moving a link to a client's new address after a NAT rebinding (`None`: links
    /// never move, the 2009 behaviour).
    same_host: Option<fn(&A, &A) -> bool>,
}

/// A fresh 64-bit link move secret (NET-24): SipHash under the process's random hash keys (std seeds them from the
/// operating system) over a counter and the clock. Unpredictable to anyone outside the process; no dependency.
// DC-DEFAULT: NET-24 (proposed, project decision)
fn fresh_move_secret() -> dc_wire::linkmove::Secret {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(N.fetch_add(1, Ordering::Relaxed));
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    h.write_u128(t);
    h.finish().to_le_bytes()
}

impl<A: Ord + Clone + Display> Server<A> {
    /// A server without ROM game tables (DeCombat defaults for damage and teams).
    pub fn new(cfg: ServerTuning, world: WorldInfo, now: Millis) -> Self {
        Self::with_tables(cfg, world, RomTables::default(), now)
    }

    /// A server with the game tables read from the user's ROM.
    pub fn with_tables(
        cfg: ServerTuning,
        world: WorldInfo,
        tables: RomTables,
        now: Millis,
    ) -> Self {
        let host = Host::new(HostConfig {
            session_name: cfg.session_name.as_bytes().to_vec(),
            max_links: cfg.max_players,
            ..HostConfig::default()
        });
        let files = vec![FileEntry {
            id: WORLD_FILE_ID,
            // DC-DEFAULT: CD-59 (proposed) file_info = file size in bytes
            info: world.size,
            name: world.announce.as_bytes().to_vec(),
        }];
        let mut game = Game::new(cfg.game.clone(), &world, tables);
        // DC-DEFAULT: DSV-480 (proposed) other players' health only where culling is off (a wallhacks room)
        game.set_remote_health(!cfg.vis.enabled);
        let spawn_points: Vec<[f32; 3]> = world.spawns.iter().map(|s| s.pos).collect();
        let vis = crate::visibility::Visibility::new(
            cfg.vis.clone(),
            world.occluders.clone(),
            &PosBox {
                min: world.extent_min,
                max: world.extent_max,
            },
            &spawn_points,
        );
        let first_tick = now.wrapping_add((1000 / cfg.tick_hz.max(1)).max(1));
        Server {
            cfg,
            host,
            world,
            files,
            roster: Vec::new(),
            seat_refusals: 0,
            room: RoomSettings::default(),
            clients: BTreeMap::new(),
            objects_used: BTreeSet::new(),
            start: now,
            next_tick: first_tick,
            tick_base: now,
            tick_count: 1,
            last_activity: now,
            played: BTreeMap::new(),
            finished: false,
            game,
            out: VecDeque::new(),
            world_objs: None,
            projectiles: BTreeMap::new(),
            cases: BTreeMap::new(),
            case_records: BTreeMap::new(),
            intels: BTreeMap::new(),
            intel_records: BTreeMap::new(),
            drops: BTreeMap::new(),
            drop_records: BTreeMap::new(),
            world_fx: BTreeMap::new(),
            bomb_look: None,
            bomb_objs: Vec::new(),
            bomb_records: Vec::new(),
            bomb_blast: None,
            fx_hidden: BTreeSet::new(),
            fx_extra: BTreeMap::new(),
            flag_carried: BTreeMap::new(),
            broken_brushes: BTreeMap::new(),
            event_tracks: Vec::new(),
            vis,
            corpses: Vec::new(),
            kill_active: Vec::new(),
            kill_inside: BTreeMap::new(),
            doors: Vec::new(),
            doors_at: None,
            turns: Vec::new(),
            door_delays: Vec::new(),
            door_triggers: crate::doors::Triggers::default(),
            door_start_fired: Vec::new(),
            door_round: 0,
            model_start: BTreeMap::new(),
            door_marks: BTreeMap::new(),
            camshot: crate::camshot::CamShot::default(),
            world_started: false,
            same_host: None,
        }
    }

    /// The world-file objects created on the clients (after the first placement).
    pub fn world_objects(&self) -> &[crate::worldobj::WorldObject] {
        self.world_objs.as_deref().unwrap_or(&[])
    }

    /// Plans the world-file objects once: ids from the lowest free, their files announced.
    // DC-DEFAULT: DSV-37 (proposed); CD-66 (approved) lowest free id
    fn plan_world_objects(&mut self) {
        if self.world_objs.is_some() {
            return;
        }
        let room_class = self.game.rules().mode_class;
        // DC-DEFAULT: DSV-220 (proposed) a bomb room finds the bomb's model and effect names once, with its world
        if matches!(room_class, 2 | 3) && self.bomb_look.is_none() {
            let look = self
                .game
                .tables()
                .rom
                .as_deref()
                .map(crate::bomblook::BombLook::from_rom)
                .unwrap_or_default();
            self.log(format!(
                "bomb look: model {} skin {} light {} blast {}",
                look.model.is_some(),
                look.skin.is_some(),
                look.light_fx.is_some(),
                look.blast_fx.is_some()
            ));
            self.bomb_look = Some(look);
        }
        let used = &mut self.objects_used;
        let next = || {
            let id = (FIRST_OBJECT_ID..=u16::MAX - 1)
                .find(|i| !used.contains(i))
                .unwrap_or(FIRST_OBJECT_ID);
            used.insert(id);
            id
        };
        let trig = self.room.get("trig").map(str::to_string);
        let mut objs = crate::worldobj::plan_scene(
            &self.world,
            self.game.tables(),
            self.cfg.world_objects,
            self.cfg.zone_class,
            room_class,
            trig.as_deref(),
            next,
        );
        // the sliding doors, closed (DSV-46): with `world` they are added here; `all` already has every brush
        // DC-DEFAULT: DSV-46 (proposed)
        if self.cfg.doors
            && matches!(
                self.cfg.world_objects,
                crate::worldobj::WorldObjects::World | crate::worldobj::WorldObjects::All
            )
        {
            // DC-DEFAULT: DSV-501 (proposed) the doors in the state the room's start scripts leave them: `visible` /
            // `solid` as for every named brush (a door left with none of the three flags is not created), `on` opens
            // a door from the start (with `all` no scripts run)
            let (ov, start_fired) = if self.cfg.world_objects == crate::worldobj::WorldObjects::World {
                crate::worldobj::start_scripts_run(&self.world.scripts, trig.as_deref())
            } else {
                Default::default()
            };
            let get = |name: &[u8]| {
                ov.get(&name.to_ascii_lowercase())
                    .copied()
                    .unwrap_or_default()
            };
            for d in &self.world.doors {
                let Some(b) = self
                    .world
                    .brushes
                    .iter()
                    .find(|b| b.class == crate::world::BrushClass::Door && b.name == d.name)
                    .and_then(|b| crate::worldobj::scripted_brush(b, get(&b.name)))
                else {
                    continue;
                };
                let existing = objs.iter().find_map(|o| match o {
                    crate::worldobj::WorldObject::Brush { obj, brush } if brush.name == d.name => {
                        Some(*obj)
                    }
                    _ => None,
                });
                let obj = match existing {
                    Some(o) => o,
                    None => {
                        let id = (FIRST_OBJECT_ID..=u16::MAX - 1)
                            .find(|i| !self.objects_used.contains(i))
                            .unwrap_or(FIRST_OBJECT_ID);
                        self.objects_used.insert(id);
                        objs.push(crate::worldobj::WorldObject::Brush {
                            obj: id,
                            brush: b.clone(),
                        });
                        id
                    }
                };
                self.doors.push(crate::doors::Door::new(obj, d.clone()));
            }
            // DC-DEFAULT: DOOR-4 (proposed, w1-doors) the rotating doors players may open and the rotating switches
            let coop = self.game.is_coop();
            for d in &self.world.turns {
                // DSV-500: a co-op room keeps the message doors out: its mission triggers that turn them at run time
                // are not run, and a door that never opens would be a wall in the mission's way
                if coop
                    && d.open_by == crate::doors::OpenBy::Message
                    && d.kind == crate::doors::TurnKind::Door
                {
                    continue;
                }
                let Some(b) = self
                    .world
                    .brushes
                    .iter()
                    .find(|b| {
                        matches!(
                            b.class,
                            crate::world::BrushClass::Door | crate::world::BrushClass::Switch
                        ) && b.name == d.name
                    })
                    .and_then(|b| crate::worldobj::scripted_brush(b, get(&b.name)))
                else {
                    continue;
                };
                let mut d = d.clone();
                if get(&d.name).on == Some(true) {
                    d.start_open = true;
                }
                let existing = objs.iter().find_map(|o| match o {
                    crate::worldobj::WorldObject::Brush { obj, brush } if brush.name == d.name => {
                        Some(*obj)
                    }
                    _ => None,
                });
                let obj = match existing {
                    Some(o) => o,
                    None => {
                        let id = (FIRST_OBJECT_ID..=u16::MAX - 1)
                            .find(|i| !self.objects_used.contains(i))
                            .unwrap_or(FIRST_OBJECT_ID);
                        self.objects_used.insert(id);
                        objs.push(crate::worldobj::WorldObject::Brush {
                            obj: id,
                            brush: b.clone(),
                        });
                        id
                    }
                };
                self.turns.push(crate::doors::TurnDoor::new(obj, d));
            }
            self.door_marks = self.door_entries().into_iter().collect();
            // DC-DEFAULT: DSV-562 (proposed) the world's triggers for the doors' and switches' statements
            self.door_triggers = crate::doors::Triggers::new(self.world.triggers.clone());
            // DSV-562: the triggers the start scripts fired count as fired
            self.door_triggers.seed(&start_fired);
            self.door_start_fired = start_fired;
        } else if self.cfg.world_objects == crate::worldobj::WorldObjects::All {
            // `-doors off`: no door objects (a door that never opens would be a wall)
            objs.retain(|o| {
                !matches!(o, crate::worldobj::WorldObject::Brush { brush, .. }
                    if brush.class == crate::world::BrushClass::Door)
            });
        }
        // DC-DEFAULT: DSV-56 (proposed) the switched-on world sounds start with the world load (`-worldsounds`)
        if self.cfg.world_sounds {
            let used = &mut self.objects_used;
            let next = || {
                let id = (FIRST_OBJECT_ID..=u16::MAX - 1)
                    .find(|i| !used.contains(i))
                    .unwrap_or(FIRST_OBJECT_ID);
                used.insert(id);
                id
            };
            objs.extend(crate::worldobj::plan_sounds(
                &self.world,
                self.cfg.world_objects,
                trig.as_deref(),
                next,
            ));
        }
        for o in &objs {
            let files = o.files();
            self.file_ids(&files);
        }
        let mut n = [0usize; 7];
        for o in &objs {
            n[match o {
                crate::worldobj::WorldObject::Zone { .. }
                | crate::worldobj::WorldObject::Uplink { .. } => 0,
                crate::worldobj::WorldObject::Flag { .. } => 1,
                crate::worldobj::WorldObject::Brush { .. } => 2,
                crate::worldobj::WorldObject::Volume { .. } => 3,
                crate::worldobj::WorldObject::Sprite { .. } => 4,
                crate::worldobj::WorldObject::Effect { .. } => 5,
                crate::worldobj::WorldObject::Sound { .. } => 6,
            }] += 1;
        }
        // DC-DEFAULT: DSV-52 (proposed) the kill volumes the clients have act on the server
        self.kill_active = self
            .world
            .kill_shapes
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                objs.iter().any(|o| {
                    matches!(o, crate::worldobj::WorldObject::Volume { volume, .. }
                        if volume.kind == crate::world::VolumeKind::KillVolume
                            && volume.name.eq_ignore_ascii_case(&s.name))
                })
            })
            .map(|(i, _)| i)
            .collect();
        self.log(format!(
            "world objects: {} ({} zones, {} flags, {} brushes, {} volumes, {} sprites, {} effects), {} world \
             sounds; {} doors, {} kill volumes act",
            objs.len() - n[6],
            n[0],
            n[1],
            n[2],
            n[3],
            n[4],
            n[5],
            n[6],
            self.doors.len(),
            self.kill_active.len()
        ));
        // DC-DEFAULT: DSV-115 (proposed) the created breakables take damage (`crate::destructibles`)
        let breakables: Vec<(u16, Vec<u8>, [f32; 3])> = objs
            .iter()
            .filter_map(|o| match o {
                crate::worldobj::WorldObject::Brush { obj, brush }
                    if crate::destructibles::bindable(brush) =>
                {
                    Some((*obj, brush.name.clone(), brush.pos))
                }
                _ => None,
            })
            .collect();
        self.game.bind_breakables(&breakables);
        // DC-DEFAULT: DSV-192 (proposed) the created capture flags are the flag mode's flags
        let flags: Vec<(u16, u8, [f32; 3])> = objs
            .iter()
            .filter_map(|o| match o {
                crate::worldobj::WorldObject::Flag {
                    obj, flag, team, ..
                } => Some((*obj, *team as u8, flag.pos)),
                _ => None,
            })
            .collect();
        self.game.bind_flags(&flags);
        self.world_objs = Some(objs);
        // DC-DEFAULT: DSV-112 (proposed) the world's camera objects, off at their file places
        if self.cfg.cam_shot {
            let used = &mut self.objects_used;
            let next = || {
                let id = (FIRST_OBJECT_ID..=u16::MAX - 1)
                    .find(|i| !used.contains(i))
                    .unwrap_or(FIRST_OBJECT_ID);
                used.insert(id);
                id
            };
            self.camshot =
                crate::camshot::CamShot::plan(&self.world.scripts.cameras, trig.as_deref(), next);
            if !self.camshot.cameras.is_empty() {
                self.log(format!(
                    "world cameras: {}, end trigger {}",
                    self.camshot.cameras.len(),
                    if self.camshot.end_trigger().is_some() {
                        "set"
                    } else {
                        "none"
                    }
                ));
            }
        }
    }

    /// The id of an announced resource (0xFFFF when it is not in the file table).
    fn file_id_of(&self, path: &str) -> u16 {
        let name = crate::chars::announce_spelling(path).into_bytes();
        self.files
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(&name))
            .map_or(0xFFFF, |f| f.id)
    }

    /// The activate entry `(disabled, state)` of every door object now (DSV-340).
    fn door_entries(&self) -> Vec<(u16, (bool, u8))> {
        self.doors
            .iter()
            .map(|d| (d.obj, d.activate()))
            .chain(self.turns.iter().filter_map(|t| Some((t.obj, t.activate()?))))
            .collect()
    }

    /// Create records of the world-file objects (not the world sounds).
    fn world_records(&self) -> Vec<BitWriter> {
        let pbox = self.pos_box();
        // DC-DEFAULT: DSV-340 (proposed) a door's create blob carries its activate entry as it is now
        let entries: BTreeMap<u16, (bool, u8)> = self.door_entries().into_iter().collect();
        let mut v: Vec<BitWriter> = self
            .world_objects()
            .iter()
            .filter(|o| !o.is_sound())
            // CLI-1340: a world effect switched off by a plant is not created for a client entering meanwhile
            .filter(|o| !self.fx_hidden.contains(&o.obj()))
            .map(|o| match o {
                crate::worldobj::WorldObject::Brush { obj, brush } if entries.contains_key(obj) => {
                    let (off, state) = entries[obj];
                    crate::worldobj::brush_record_with(
                        *obj,
                        brush,
                        &pbox,
                        Some(crate::doors::door_blob_of(
                            crate::doors::DOOR_ACTIVATE_ROW,
                            off,
                            state,
                        )),
                    )
                }
                _ => o.record(&pbox, &|p| self.file_id_of(p)),
            })
            .collect();
        // DC-DEFAULT: DSV-112 (proposed) the world cameras in their current state
        v.extend(self.camshot.cameras.iter().map(|c| c.create_record(&pbox)));
        // DC-DEFAULT: DOOR-5 (proposed, w1-doors) a turned door's pose after its create (created at the model origin)
        v.extend(
            self.turns
                .iter()
                .filter(|t| t.frac > 0.0)
                .map(|t| t.record()),
        );
        v
    }

    /// The file ids of the world sounds, each once, in plan order.
    // DC-DEFAULT: DSV-65 (proposed)
    fn world_sound_file_ids(&self) -> Vec<u16> {
        let mut ids: Vec<u16> = Vec::new();
        for o in self.world_objects().iter().filter(|o| o.is_sound()) {
            for f in o.files() {
                let id = self.file_id_of(&f);
                if id != 0xFFFF && !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids
    }

    /// Tracked-sound records of the world sounds (they follow the creates in the id 8 message).
    // SPEC: UDP_SESSION_2009_B.md §8.3; OBSERVED: capture 20261025_002236_tdm4 c6 seq 807 (the
    // creates, then the 11 sounds, then the stream entry; REFERENCE)
    // DC-DEFAULT: DSV-56 (proposed)
    fn world_sound_records(&self) -> Vec<BitWriter> {
        let pbox = self.pos_box();
        self.world_objects()
            .iter()
            .filter(|o| o.is_sound())
            .map(|o| o.record(&pbox, &|p| self.file_id_of(p)))
            .collect()
    }

    // ------------------------------------------------------------------------------------------- accessors

    pub fn config(&self) -> &ServerTuning {
        &self.cfg
    }

    pub fn world(&self) -> &WorldInfo {
        &self.world
    }

    pub fn clients(&self) -> &BTreeMap<A, Client> {
        &self.clients
    }

    pub fn roster(&self) -> &[RosterEntry] {
        &self.roster
    }

    pub fn room(&self) -> &RoomSettings {
        &self.room
    }

    pub fn host(&self) -> &Host<A> {
        &self.host
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// The game layer (rules, players, scores).
    /// The visibility culling state (tests, statistics).
    pub fn visibility(&self) -> &crate::visibility::Visibility {
        &self.vis
    }

    pub fn game(&self) -> &Game {
        &self.game
    }

    /// Server game time in seconds since the first client of the current empty-to-occupied span connected.
    // SPEC: UDP_SESSION_2009_B.md §9.1 (the same value S2C_LOAD_WORLD carries; seconds PLAUSIBLE)
    pub fn server_time(&self, now: Millis) -> f32 {
        since(now, self.start) as f32 / 1000.0
    }

    /// `@ready` for the bound port (the runner emits it once the socket is bound).
    pub fn ready_line(&self, port: u16) -> String {
        pipe::ready_line(port, &self.world.display)
    }

    /// Reads the default character from the ROM before the first player (see [`Game::preload_character`]).
    pub fn preload_character(&mut self) -> Option<String> {
        self.game.preload_character()
    }

    /// Puts a body model's rig into the game's cache (tests and tools without a ROM).
    pub fn insert_rig(&mut self, body: &str, rig: crate::chars::Rig) {
        self.game.insert_rig(body, rig);
    }

    /// The bound address, for `GS_GAME_ENDPOINT`.
    pub fn set_endpoint(&mut self, host: &str, port: u16) {
        self.game.set_endpoint(host.as_bytes(), port);
    }

    /// The session file table (world file first).
    pub fn files(&self) -> &[FileEntry] {
        &self.files
    }

    /// File ids of resource paths; paths not yet in the table are added and announced at once to every linked
    /// client as single-entry `S2C_FILE_LIST` messages (clients still before `S2C_LOAD_WORLD` must answer them).
    // SPEC: UDP_SESSION_2009_A.md §4.4 (host: a file used later goes to every client as a single-entry list; ids are
    // a server-wide counter); UDP_REFERENCE_FINDINGS_botclient.md O2 (single-entry lists before the creates; REFERENCE)
    // DC-DEFAULT: CD-59 (approved) file_info = size in bytes
    fn file_ids(&mut self, paths: &[String]) -> Vec<u16> {
        let mut ids = Vec::with_capacity(paths.len());
        for p in paths {
            let name = crate::chars::announce_spelling(p).into_bytes();
            if let Some(f) = self
                .files
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(&name))
            {
                ids.push(f.id);
                continue;
            }
            let id = self
                .files
                .iter()
                .map(|f| f.id)
                .max()
                .unwrap_or(WORLD_FILE_ID)
                + 1;
            let entry = FileEntry {
                id,
                info: self.game.resource_size(p),
                name,
            };
            let peers: Vec<A> = self.clients.keys().cloned().collect();
            for peer in peers {
                if let Some(c) = self.clients.get_mut(&peer) {
                    if c.phase == Phase::Linked {
                        c.files_pending.insert(id);
                    }
                }
                self.send(&peer, msg::s2c_file_list(std::slice::from_ref(&entry)));
            }
            self.files.push(entry);
            ids.push(id);
        }
        ids
    }

    /// The model entries of a character (body with its child models, then the face) as file ids.
    fn character_models(&mut self, c: &CharacterLook) -> Vec<ModelEntry> {
        let a = &c.assets;
        let body = self.file_ids(&a.body);
        let textures = self.file_ids(&a.body_textures);
        let styles = self.file_ids(&a.body_styles);
        let mut models = vec![ModelEntry {
            file: body[0],
            subs: body[1..].to_vec(),
            textures,
            styles,
        }];
        if let Some((m, sk, st)) = &a.face {
            let file = self.file_ids(std::slice::from_ref(m))[0];
            let textures = self.file_ids(sk);
            let styles = self.file_ids(st);
            models.push(ModelEntry {
                file,
                subs: Vec::new(),
                textures,
                styles,
            });
        }
        models
    }

    fn weapon_model(&mut self, w: &crate::game::WeaponLook) -> ModelEntry {
        let file = self.file_ids(std::slice::from_ref(&w.model))[0];
        let textures = self.file_ids(&w.textures);
        let styles = self.file_ids(&w.styles);
        ModelEntry {
            file,
            subs: Vec::new(),
            textures,
            styles,
        }
    }

    /// Create records of a player's objects as they are now: the character and its weapon model (slice 3), or
    /// the slice-2 plain object when the player has no character.
    // SPEC: UDP_SESSION_2009_B.md §3.3; UDP_REFERENCE_FINDINGS_botclient.md reference run (create form; REFERENCE)
    fn create_records(
        &mut self,
        pid: u8,
        obj: u16,
        weapon_obj: Option<u16>,
        fallback_pos: [f32; 3],
        for_owner: bool,
    ) -> Vec<BitWriter> {
        let pbox = self.pos_box();
        let (character, pos, spawned, complete, yaw, dead) = match self.game.players().get(&pid) {
            Some(p) => (
                p.character.clone(),
                p.pos,
                p.visible,
                p.spawns > 0,
                p.yaw,
                matches!(
                    p.life,
                    crate::game::Life::Dying { .. } | crate::game::Life::Waiting { .. }
                ),
            ),
            None => (None, fallback_pos, false, false, 0.0, false),
        };
        // a dead player is created hidden and without its weapon model (removed at the death, created again at the
        // respawn)
        // DC-DEFAULT: DSV-44 (proposed)
        let (spawned, weapon_obj) = if dead {
            (false, None)
        } else {
            (spawned, weapon_obj)
        };
        let Some(c) = character else {
            return vec![objects::create_record(&self.create_spec(obj, pos), &pbox)];
        };
        let models = self.character_models(&c);
        let weapon_model = c.weapon.as_ref().map(|w| self.weapon_model(w));
        let main = c.anims.main;
        let create_tracker = |secondary| Tracker {
            anim: main.0,
            state0: true,
            state1: true,
            time_ms: 0,
            len_ms: main.1,
            secondary,
        };
        let spec = CharacterSpec {
            obj,
            weapon_obj,
            pid,
            pos,
            dims: self.game.dims_of(pid),
            models,
            // SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (create: three trackers on animation 0, time 0;
            // REFERENCE); the stream carries the real animations from the next tick on
            anim: vec![
                create_tracker(None),
                create_tracker(Some(c.anims.secondary[0])),
                create_tracker(Some(c.anims.secondary[1])),
            ],
            spawned,
            record_complete: complete,
            // CD-67 (approved): no reliable rotation for the own view object
            rot: (!for_owner).then(|| crate::stream::yaw_quat(yaw)),
            socket: c.assets.rig.grip_socket,
            weapon_model,
            weapon_anim_len_ms: crate::game::TODO_WEAPON_ANIM_LEN_MS,
        };
        let mut out = vec![modelobj::character_create(&spec).write(&pbox)];
        if let Some(w) = modelobj::weapon_create(&spec) {
            out.push(w.write(&pbox));
        }
        out
    }

    /// No client and the idle time is over.
    // SPEC: DSERVER_PIPE.md §6 (idle with zero clients; the server prints @end)
    pub fn idle_expired(&self, now: Millis) -> bool {
        match self.cfg.idle_ms {
            Some(ms) => self.clients.is_empty() && since(now, self.last_activity) >= ms,
            None => false,
        }
    }

    /// The transport host's counters (join asks, accepted, repeated, stale re-joins ...).
    pub fn host_stats(&self) -> dc_net_engine::HostStats {
        self.host.stats()
    }

    /// True while a link is still held, open or closing with bye copies to send (the runner keeps pumping
    /// before it exits).
    pub fn has_links(&self) -> bool {
        self.host.link_count() > 0
    }

    // ------------------------------------------------------------------------------------------- input

    /// Lets a link follow its client to a new address of the same host (`same_host`: the same IP for socket
    /// addresses) after a NAT rebinding (NET-23, `Host::migrate`).
    // DC-DEFAULT: NET-23 (proposed) address move of a link after a NAT rebinding, see DC_DEFAULTS.md
    pub fn set_address_moves(&mut self, same_host: fn(&A, &A) -> bool) {
        self.same_host = Some(same_host);
    }

    /// A datagram from the network.
    pub fn handle_datagram(&mut self, now: Millis, from: A, bytes: &[u8]) {
        if let Some(same) = self.same_host {
            let before = self.host.stats();
            if let Some(old) = self
                .host
                .migrate(now, &from, bytes, same, ADDRESS_MOVE_SILENCE_MS)
            {
                self.on_address_moved(&old, &from);
            } else {
                // NET-24: refused moves (no or a wrong secret, or the move limit), the first ones and then every 1000th
                let st = self.host.stats();
                let n = st.migrations_refused + st.migrations_limited;
                if n > before.migrations_refused + before.migrations_limited
                    && (n <= 5 || n % 1000 == 0)
                {
                    self.log(format!(
                        "link move to {from} refused ({} unproven, {} over the move limit, NET-24)",
                        st.migrations_refused, st.migrations_limited
                    ));
                }
            }
        }
        self.host.handle_datagram(now, from, bytes);
        self.pump(now);
    }

    /// NET-23: the client of the link at `old` now talks from `new` (it proved it with its move secret, NET-24):
    /// everything kept by address follows.
    fn on_address_moved(&mut self, old: &A, new: &A) {
        if let Some(c) = self.clients.remove(old) {
            self.log(format!(
                "client {}: address moved from {old} to {new} (NAT rebinding, NET-23)",
                c.client_id
            ));
            self.clients.insert(new.clone(), c);
        } else {
            self.log(format!(
                "link moved from {old} to {new} (NAT rebinding, NET-23)"
            ));
        }
        for (_, _, peers) in self.event_tracks.iter_mut() {
            for p in peers.iter_mut() {
                if p == old {
                    *p = new.clone();
                }
            }
        }
    }

    /// One stdin control line. Returns false on `quit`.
    // SPEC: DSERVER_PIPE.md §3
    pub fn handle_line(&mut self, now: Millis, line: &str) -> bool {
        match pipe::parse_control(line) {
            Control::User(u) => {
                self.log(format!(
                    "roster: uid {} team {} nick {}",
                    u.uid, u.team, u.nick
                ));
                self.game.note_roster(u.uid, now);
                if let Some(old) = self.roster.iter_mut().find(|r| r.uid == u.uid) {
                    *old = u;
                } else {
                    self.roster.push(u);
                }
            }
            Control::Room(r) => {
                self.log(format!("room settings: {:?}", r.pairs));
                self.game.set_room(&r);
                self.room = r;
            }
            Control::Console(c) => {
                self.log(format!("console line ignored (no engine console): {c}"))
            }
            Control::Gain {
                uid,
                exp,
                gp,
                exp_bonus,
                gp_bonus,
            } => {
                self.log(format!(
                    "lobby gains: uid {uid} exp {exp} gp {gp} bonus {exp_bonus}/{gp_bonus}"
                ));
                self.game.set_gain(uid, (exp, gp, exp_bonus, gp_bonus));
            }
            Control::Quit => return false,
            Control::Blank => {}
            Control::Bad(l) => {
                let cut: String = l.chars().take(200).collect();
                self.log(format!("malformed control line ignored: {cut}"));
            }
        }
        true
    }

    /// Runs timers (tick, start-up deadline) and the transport, and returns everything to do.
    pub fn poll(&mut self, now: Millis) -> Vec<Output<A>> {
        self.check_deadlines(now);
        if !self.finished {
            self.game.tick(now);
            self.apply_game(now);
        }
        if !later(self.next_tick, now) {
            self.tick(now);
            let hz = self.cfg.tick_hz.max(1) as u64;
            self.tick_count += 1;
            self.next_tick = self
                .tick_base
                .wrapping_add((self.tick_count * 1000 / hz) as u32);
            if !later(self.next_tick, now) {
                // fell behind by more than a tick: resynchronise instead of bursting
                self.tick_base = now;
                self.tick_count = 1;
                self.next_tick = now.wrapping_add((1000 / hz).max(1) as u32);
            }
        }
        self.pump(now);
        self.out.drain(..).collect()
    }

    /// Ends the match: `@player` per identified player, `@result`, `@leave` per connected player, `@end`; every link
    /// gets a plain bye. Further polls only flush the bye copies.
    // SPEC: DSERVER_PIPE.md §3.4 (quit), §4.4-4.7, §5 (event order)
    // DC-DEFAULT: DSV-7 (staged proposal) slice-1 reporting; CD-54 (proposed) rewards computed by the lobby
    pub fn finish(&mut self, now: Millis) {
        if self.finished {
            return;
        }
        if !self.game.reported() {
            // DC-DEFAULT: DSV-18 (staged proposal) a match stopped before its end is reported as a draw
            self.game.report_aborted();
            self.apply_game(now);
        }
        self.finished = true;
        // DC-DEFAULT: DSV-42 (proposed) the hit verdict counters of the whole session
        let hits = format!("session {}", self.game.hit_summary());
        self.log(hits);
        // DC-DEFAULT: DSV-102 (proposed) the shot-direction counters of the whole session
        let cone = format!("session {}", self.game.cone_summary());
        self.log(cone);
        let peers: Vec<A> = self.clients.keys().cloned().collect();
        for peer in peers {
            self.report_leave(&peer);
            // DC-DEFAULT: MON-1 (proposed) the players still on get their summary before `@end`
            self.mark_leave(&peer, pipe::LeaveHow::End);
            self.report_linksum(now, &peer, pipe::LeaveHow::End, 0);
            self.host.disconnect(now, &peer, Bye::Plain);
        }
        self.pipe(pipe::END_LINE.to_string());
        self.pump(now);
    }

    // ------------------------------------------------------------------------------------------- internals

    fn log(&mut self, s: String) {
        self.out.push_back(Output::Log(format!("dc-dserver: {s}")));
    }

    fn pipe(&mut self, s: String) {
        self.out.push_back(Output::Pipe(s));
    }

    fn pump(&mut self, now: Millis) {
        for _ in 0..8 {
            let events = self.host.poll(now);
            if events.is_empty() {
                break;
            }
            for e in events {
                match e {
                    HostEvent::Transmit { to, bytes } => {
                        self.out.push_back(Output::Transmit { to, bytes })
                    }
                    HostEvent::LinkOpened {
                        peer,
                        rate_bps,
                        app_id_index,
                    } => self.on_link_opened(now, peer, rate_bps, app_id_index),
                    HostEvent::Message {
                        peer,
                        reliable,
                        bits,
                    } => self.on_message(now, peer, reliable, bits),
                    HostEvent::LinkClosed { peer, code, cause } => {
                        self.on_link_closed(now, peer, code, cause)
                    }
                }
            }
        }
    }

    fn send(&mut self, peer: &A, msg: BitWriter) {
        self.host.send_reliable(peer, msg);
    }

    fn peer_of(&self, pid: u8) -> Option<A> {
        self.clients
            .iter()
            .find(|(_, c)| c.pid() == pid && c.object_id.is_some())
            .map(|(p, _)| p.clone())
    }

    /// Carries out what the game layer asked for.
    fn apply_game(&mut self, now: Millis) {
        for o in self.game.take_out() {
            match o {
                GameOut::To(pid, m) => {
                    if let Some(peer) = self.peer_of(pid) {
                        self.send(&peer, m);
                    }
                }
                GameOut::Pipe(l) => self.pipe(l),
                GameOut::Log(l) => self.log(l),
                GameOut::Kick(pid, code) => {
                    if let Some(peer) = self.peer_of(pid) {
                        self.mark_leave(&peer, pipe::LeaveHow::Kicked);
                        self.host.disconnect(now, &peer, Bye::Code(code));
                    }
                }
                GameOut::Mark(pid, m) => {
                    let step = match m {
                        Milestone::IdentifyRequest => Step::IdentifyRequest,
                        Milestone::PlayerInfo => Step::PlayerInfo,
                        Milestone::Introduced => Step::Introduced,
                        Milestone::WaitingState => Step::WaitingState,
                        Milestone::WorldReady => Step::WorldReady,
                        Milestone::Spawned => Step::Spawned,
                    };
                    if let Some(peer) = self.peer_of(pid) {
                        if let Some(c) = self.clients.get_mut(&peer) {
                            c.mark(step, now);
                        }
                        if step == Step::WorldReady {
                            self.start_world(&peer);
                        }
                    }
                }
                GameOut::Finish => {
                    self.log("match over, ending the session".into());
                    self.finish(now);
                }
                GameOut::Spawned { pid, first, .. } => self.on_spawned(pid, first),
                GameOut::Visible { pid, pos, yaw } => self.on_visible(pid, pos, yaw, now),
                GameOut::Unreliable(pid, m) => {
                    if let Some(peer) = self.peer_of(pid) {
                        self.host.send_unreliable(now, &peer, m);
                    }
                }
                GameOut::HandWeapon { pid, weapon } => self.on_hand_weapon(pid, weapon),
                GameOut::Projectile(p) => self.on_projectile(&p),
                GameOut::ProjectileGone { key } => self.on_projectile_gone(key),
                GameOut::AiCreate { slot } => self.on_ai_create(slot),
                GameOut::AiShow { slot } => self.on_ai_show(slot),
                GameOut::AiHide { slot } => self.on_ai_hide(slot),
                GameOut::AiLook { slot } => self.on_ai_look(slot),
                GameOut::Died {
                    pid,
                    pos,
                    yaw,
                    clip,
                } => self.on_died(pid, pos, yaw, clip, now),
                GameOut::CaseCreate(c) => self.on_case_create(&c),
                GameOut::CaseNote { key, remaining } => self.on_case_note(key, remaining),
                GameOut::FlagNote { obj, remaining } => {
                    let m = crate::gs::hisec_countdown(obj, remaining);
                    for v in self.world_viewers() {
                        self.send(&v, m.clone());
                    }
                }
                GameOut::CaseGone { key } => self.on_case_gone(key),
                GameOut::IntelCreate(c) => self.on_intel_create(&c),
                GameOut::IntelGone { key } => self.on_intel_gone(key),
                GameOut::Sound(s) => self.on_event_sound(&s, now),
                GameOut::DropCreate(d) => self.on_drop_create(&d),
                GameOut::DropGone { key } => self.on_drop_gone(key),
                GameOut::WorldBreak(b) => self.on_world_break(&b),
                GameOut::WorldFxGone { key } => self.on_world_fx_gone(key),
                GameOut::WorldFx { key, name } => self.on_world_fx(key, &name),
                GameOut::FlagMove { obj, pos } => self.on_flag_move(obj, pos),
                GameOut::BombShown { site, pos } => self.on_bomb_shown(site, pos),
                GameOut::BombHidden => self.on_bomb_hidden(),
                GameOut::WorldFxSwitch { name, on } => self.on_world_fx_switch(&name, on),
                GameOut::WorldFxRestore => self.on_world_fx_restore(),
                GameOut::BombBlast { pos } => self.on_bomb_blast(pos),
                GameOut::BombBlastGone => self.on_bomb_blast_gone(),
                GameOut::CharacterLook { pid } => self.on_character_look(pid),
                GameOut::FlagCarry {
                    obj,
                    team,
                    carrier,
                    before,
                } => self.on_flag_carry(obj, team, carrier, before),
            }
        }
        // DC-DEFAULT: RND-5 (staged) the breakables a round start restored come back on the clients
        let back = self.game.take_restored_breakables();
        if !back.is_empty() {
            self.on_world_restore(&back);
        }
    }

    fn lowest_free_client_id(&self) -> Option<u16> {
        let used: BTreeSet<u16> = self.clients.values().map(|c| c.client_id).collect();
        (0..CLIENT_ID_LIMIT).find(|i| !used.contains(i))
    }

    /// New link: `S2C_VERSION`, `S2C_CLIENT_ID`, one `S2C_FILE_LIST` with every used file; phase 1.
    // SPEC: UDP_SESSION_2009_A.md §8.2 (new link), §9.2 S2-S4 (may precede S1)
    fn on_link_opened(&mut self, now: Millis, peer: A, rate_bps: u32, app_id_index: usize) {
        if self.finished {
            self.host.disconnect(now, &peer, Bye::Plain);
            return;
        }
        let Some(client_id) = self.lowest_free_client_id() else {
            self.host.disconnect(now, &peer, Bye::Plain);
            return;
        };
        self.log(format!(
            "client {client_id} linked from {peer} (link rate {rate_bps}, app id layout {app_id_index})"
        ));
        let mut c = Client {
            client_id,
            phase: Phase::Linked,
            joined_at: now,
            hello: None,
            player: None,
            files_pending: self.files.iter().map(|f| f.id).collect(),
            object_id: None,
            weapon_obj: None,
            weapon_gone: false,
            spawn: None,
            rate_bps: None,
            updates: 0,
            last_update: None,
            shell_counts: BTreeMap::new(),
            steps: Vec::new(),
            ticks: 0,
            left_reported: false,
            switch_ready_early: false,
            leaving_since: None,
            watch: Default::default(),
            leave_how: None,
            sum_reported: false,
        };
        c.mark(Step::Linked, now);
        if self.cfg.game.start_order == StartOrder::Reference {
            // SPEC: UDP_REFERENCE_FINDINGS_botclient.md O6 (`0d 90 01`, `0d 88 02` ahead of S2C_VERSION; REFERENCE);
            // CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §2.3 (a running world: `0d 90 00`, `0d 88 03` at once; REFERENCE,
            // ACCEPTED)
            // DC-DEFAULT: DSV-36 (proposed)
            if matches!(
                self.game.phase(),
                crate::game::RoundPhase::Running { .. } | crate::game::RoundPhase::Over { .. }
            ) {
                self.send(&peer, crate::gs::shell_flag_90(false));
                self.send(
                    &peer,
                    msg::s2c_switch_state(crate::game::REFERENCE_SWITCH_STATE_READY),
                );
                c.switch_ready_early = true;
            } else {
                self.send(&peer, crate::gs::shell_flag_90(true));
                self.send(&peer, msg::s2c_switch_state(self.cfg.switch_state));
            }
        }
        self.send(&peer, msg::s2c_version(self.cfg.host_rate_bps));
        c.mark(Step::Version, now);
        self.send(&peer, msg::s2c_client_id(client_id));
        c.mark(Step::ClientId, now);
        let list = msg::s2c_file_list(&self.files);
        self.send(&peer, list);
        c.mark(Step::FileList, now);
        if self.clients.is_empty() {
            // DC-DEFAULT: DSV-21 (the stream time restarts with the first client of an empty server, so the f32
            // seconds keep their resolution: 8 ms steps after a day of uptime, 31 ms after three days)
            self.start = now;
        }
        self.clients.insert(peer, c);
    }

    fn on_message(&mut self, now: Millis, peer: A, reliable: bool, bits: BitWriter) {
        if !self.clients.contains_key(&peer) {
            return;
        }
        match msg::read_from_client(&bits) {
            FromClient::Hello(blob) => self.on_hello(now, &peer, &blob),
            FromClient::FileAck(entries) => self.on_file_ack(now, &peer, &entries),
            FromClient::LoadState(state) => self.on_load_state(now, &peer, state),
            FromClient::Rate { rate_8k, .. } => {
                // SPEC: UDP_SESSION_2009_A.md §5.5 (client rate := rate_8k * 8000); the tracked-sound acknowledgements
                // it may carry are ignored (looping world sounds, DSV-56)
                if let Some(c) = self.clients.get_mut(&peer) {
                    c.rate_bps = Some(rate_8k as u32 * 8000);
                }
            }
            // SPEC: UDP_SESSION_2009_B.md §10.2 (acknowledgements of tracked sounds); the world sounds loop and are
            // never removed, so an acknowledgement changes nothing (DSV-56, proposed)
            FromClient::SoundAck(_) => {}
            FromClient::Console(text) => {
                // DC-DEFAULT: CD-64 (approved) only moderators (`mod=1` on the lobby's `user` line); no engine console
                let moderator = self
                    .clients
                    .get(&peer)
                    .and_then(|c| c.player.as_ref())
                    .is_some_and(|e| e.moderator());
                let cut: String = String::from_utf8_lossy(&text).chars().take(120).collect();
                // GATE-PLACE: test-only keyed placement (refused unless `-place keyed=<KEY>` and the key match)
                let pid = self.clients.get(&peer).map(|c| c.pid());
                if pid.is_some_and(|pid| self.game.place_request(pid, &cut, now)) {
                    return;
                }
                if moderator && self.cfg.remote_console == RemoteConsole::Moderators {
                    self.log(format!(
                        "console command from moderator {peer}: {cut:?} (dc-dserver has no console commands; not run)"
                    ));
                } else {
                    self.log(format!(
                        "console command from {peer} ignored (not a moderator)"
                    ));
                }
            }
            FromClient::Shell { shell_id, body } => {
                self.on_shell(now, &peer, reliable, shell_id, body)
            }
            FromClient::Bye => {
                // SPEC: UDP_SESSION_2009_A.md §5.6 (C2S_BYE -> drop with bye code 5)
                self.log(format!("{peer} said bye"));
                if let Some(c) = self.clients.get_mut(&peer) {
                    c.leaving_since = None;
                    c.leave_how.get_or_insert(pipe::LeaveHow::Bye);
                }
                self.host.disconnect(now, &peer, Bye::Code(EXIT_BYE_CODE));
            }
            FromClient::Other(_) => {}
        }
    }

    /// `C2S_HELLO`: keep the blob, identify the player by the UTF-16 name against the roster.
    // SPEC: UDP_SESSION_2009_A.md §5.2, §9.2 S1; DC-DEFAULT: CD-60 (proposed)
    fn on_hello(&mut self, now: Millis, peer: &A, blob: &[u8]) {
        let hello = msg::parse_hello(blob);
        let name = hello.name();
        let Some(c) = self.clients.get_mut(peer) else {
            return;
        };
        if c.hello.is_some() {
            return;
        }
        let client_id = c.client_id;
        c.mark(Step::Hello, now);
        let entry = self
            .roster
            .iter()
            .find(|r| r.nick == name || r.nick == underscore(&name))
            .cloned();
        // DC-DEFAULT: LG-327 (proposed, project decision) a roster name with a ticket is seated only with that ticket;
        // the refusal closes this link alone, and the rightful player's link is replaced only after this check
        if let Some(e) = entry.as_ref().filter(|e| !e.ticket_accepts(hello.ticket.as_ref())) {
            let e = e.clone();
            self.refuse_seat(now, peer, client_id, &e, hello.ticket.is_some());
            return;
        }
        if let Some(c) = self.clients.get_mut(peer) {
            c.hello = Some(hello.clone());
        }
        self.log(format!(
            "client {client_id}: hello name {:?} (second string {} bytes)",
            name,
            hello.extra8.len()
        ));
        match entry {
            Some(e) => {
                // DC-DEFAULT: DSV-2 (staged proposal) a second hello of a connected player replaces the older link
                let older: Vec<A> = self
                    .clients
                    .iter()
                    .filter(|(p, c)| {
                        *p != peer && c.player.as_ref().is_some_and(|x| x.uid == e.uid)
                    })
                    .map(|(p, _)| p.clone())
                    .collect();
                for o in older {
                    self.log(format!(
                        "uid {} reconnected: closing the older link {o}",
                        e.uid
                    ));
                    self.report_leave(&o);
                    self.host.disconnect(now, &o, Bye::Plain);
                }
                self.pipe(pipe::join_line(client_id, &e));
                self.game.note_identified(&e);
                self.played.insert(e.uid, Played { entry: e.clone() });
                if let Some(c) = self.clients.get_mut(peer) {
                    c.player = Some(e);
                }
            }
            None if self.cfg.accept_unlisted => {
                self.log(format!(
                    "client {client_id}: name not in the roster, accepted (open server)"
                ));
            }
            None => {
                self.log(format!(
                    "client {client_id}: name not in the roster, refused with bye code {}",
                    self.cfg.refuse_code
                ));
                self.host
                    .disconnect(now, peer, Bye::Code(self.cfg.refuse_code));
                return;
            }
        }
        self.try_send_world(now, peer);
    }

    /// A hello for a roster name without its ticket: closes this link with the code of an unlisted name.
    // DC-DEFAULT: LG-327 (proposed, project decision)
    fn refuse_seat(&mut self, now: Millis, peer: &A, client_id: u16, e: &RosterEntry, sent: bool) {
        self.seat_refusals += 1;
        // a stranger may repeat this for as long as he likes: the log gets the first ones, then every hundredth
        if self.seat_refusals <= SEAT_REFUSALS_LOGGED || self.seat_refusals % 100 == 0 {
            self.log(format!(
                "client {client_id} from {peer}: seat of uid {} refused ({} ticket, refusal {}), bye code {}",
                e.uid,
                if sent { "wrong" } else { "no" },
                self.seat_refusals,
                self.cfg.refuse_code
            ));
        }
        let seated = self
            .clients
            .values()
            .any(|c| c.player.as_ref().is_some_and(|x| x.uid == e.uid));
        if !sent && !seated {
            self.pipe(pipe::seat_refused_line(e.uid));
        }
        self.host
            .disconnect(now, peer, Bye::Code(self.cfg.refuse_code));
    }

    /// `C2S_FILE_ACK`: bit 15 completes a file, else the file is announced again.
    // SPEC: UDP_SESSION_2009_A.md §5.4 (host engine+0x3f740)
    fn on_file_ack(&mut self, now: Millis, peer: &A, entries: &[u16]) {
        let mut again = Vec::new();
        if let Some(c) = self.clients.get_mut(peer) {
            for &e in entries {
                let id = e & !msg::FILE_ACK_ACCEPTED;
                if !c.files_pending.contains(&id) {
                    continue;
                }
                if e & msg::FILE_ACK_ACCEPTED != 0 {
                    c.files_pending.remove(&id);
                } else {
                    again.push(id);
                }
            }
            if c.files_pending.is_empty() {
                c.mark(Step::FilesAcked, now);
            }
        }
        let resend: Vec<FileEntry> = self
            .files
            .iter()
            .filter(|f| again.contains(&f.id))
            .cloned()
            .collect();
        if !resend.is_empty() {
            self.send(peer, msg::s2c_file_list(&resend));
        }
        self.try_send_world(now, peer);
    }

    /// Ready test passed (hello received, every announced file answered): `S2C_LOAD_WORLD`, phase 2 / 0.
    // SPEC: UDP_SESSION_2009_A.md §5.7, §7.2 (ready test engine+0x8b2e0), §9.2 S5-S6
    fn try_send_world(&mut self, now: Millis, peer: &A) {
        let t = self.server_time(now);
        let Some(c) = self.clients.get_mut(peer) else {
            return;
        };
        if c.phase != Phase::Linked || c.hello.is_none() || !c.files_pending.is_empty() {
            return;
        }
        c.phase = Phase::WorldSent;
        c.mark(Step::LoadWorld, now);
        let id = c.client_id;
        // SPEC: UDP_SESSION_2009_A.md §8.2 (phase 1: `S2C_EVAR` per variable, then `S2C_LOAD_WORLD`); OBSERVED: tdm4 /
        // tdm6 (the world-settings variables and the server constants in the datagrams right before
        // `S2C_LOAD_WORLD`, in one fixed order; REFERENCE)
        // DC-DEFAULT: DSV-47 (proposed), DSV-53 (proposed) the step height, DSV-60 (proposed) world north and the
        // reference stack's order
        let (world_vars, north): (&[crate::evars::WorldVar], Option<f32>) = match self.cfg.evars {
            crate::evars::EvarMode::World => (&self.world.world_vars, self.cfg.world_north),
            crate::evars::EvarMode::Off => (&[], None),
        };
        let consts = crate::clientvars::vars(&self.world.client_vars, self.cfg.step_height, north);
        let msgs = crate::evars::preload_messages(world_vars, &consts, &self.cfg.extra_evars);
        let n_world = world_vars.iter().filter(|v| v.name.is_some()).count();
        let n = msgs.len();
        for m in msgs {
            self.send(peer, m);
        }
        if n > 0 {
            let labels: Vec<&str> = consts.iter().map(|c| c.label).collect();
            self.log(format!(
                "client {id}: {n} server variables sent ({} world, constants {labels:?}, {} configured)",
                n_world,
                self.cfg.extra_evars.len()
            ));
        }
        self.send(peer, msg::s2c_load_world(t, WORLD_FILE_ID));
        self.log(format!("client {id}: load world (clock {t:.3} s)"));
    }

    // SPEC: UDP_SESSION_2009_A.md §5.3, §8.2 (state 0 -> sub 1 -> preload + sky -> sub 2; state 1 -> sub 3)
    fn on_load_state(&mut self, now: Millis, peer: &A, state: u8) {
        let Some(c) = self.clients.get_mut(peer) else {
            return;
        };
        match (c.phase, state) {
            (Phase::WorldSent, 0) => {
                c.mark(Step::WorldLoaded, now);
                c.phase = Phase::Preloading;
                c.mark(Step::Preload, now);
                let id = c.client_id;
                // the world objects are planned now: the sky table names their ids
                self.plan_world_objects();
                let sky = self
                    .cfg
                    .send_sky_table
                    .then(|| crate::worldobj::sky_table(self.world_objects()))
                    .flatten();
                if sky.is_some() {
                    if let Some(c) = self.clients.get_mut(peer) {
                        c.mark(Step::SkyTable, now);
                    }
                }
                // SPEC: UDP_SESSION_2009_A.md §9.2 S7 (`13 00` optional, `13 01`), S8
                self.send(peer, msg::s2c_preload(msg::PRELOAD_START, &[]));
                self.send(peer, msg::s2c_preload(msg::PRELOAD_END, &[]));
                // the world sounds' files as the load-sound-file list after the end mark (the reference stack's position)
                // SPEC: UDP_SESSION_2009_A.md §4.7 (sub 5: file references handed to the file service and the sound
                // layer; meaning UNPROVEN, plausibly sounds)
                // OBSERVED: capture 20261025_000014_tdm1 c5 seq 402, 20261025_002236_tdm4 c6 seq
                // 788, 20261025_065016_tdm6 c6 seq 701 (`13 05` + one u16 after `13 01`; REFERENCE)
                // DC-DEFAULT: DSV-65 (proposed) the list names the world sounds' announced files
                if self.cfg.preload_sounds {
                    let ids = self.world_sound_file_ids();
                    if !ids.is_empty() {
                        self.send(peer, msg::s2c_preload(msg::PRELOAD_SOUNDS, &ids));
                    }
                }
                // SPEC: UDP_SESSION_2009_A.md §4.8 (the 2009 host sends its whole table of 30 slots); OBSERVED: tdm4 /
                // tdm6 (count 30, free slots 0xFFFF; REFERENCE)
                // DC-DEFAULT: DSV-45 (proposed)
                if let Some((block, ids)) = &sky {
                    self.send(peer, msg::s2c_sky_table(block, ids));
                    self.log(format!("client {id}: sky table sent"));
                }
                // SPEC: RENDER_2009_addendum_styles.md §S2.1 item 2 (with the other world-bound tables)
                // DC-DEFAULT: DSV-40 (proposed) the world's global light once per client after its world load
                if self.cfg.global_light == crate::worldlight::LightMode::World {
                    if let Some(l) = self.world.global_light {
                        self.send(peer, msg::s2c_dir_light(l.dir, l.colour, l.extra));
                        self.log(format!("client {id}: global light sent"));
                    }
                }
                self.log(format!("client {id}: world loaded, preload sent"));
            }
            (Phase::Preloading, s) if s != 0 => {
                c.mark(Step::PreloadDone, now);
                self.place_in_world(now, peer);
            }
            _ => {}
        }
    }

    /// Phase 2 / 3: create the player's object at a spawn point, name it as the view object, set the
    /// switching-worlds state; phase 3.
    // SPEC: UDP_SESSION_2009_A.md §8.2 (2 / 3), §9.2 S9-S11; UDP_SESSION_2009_B.md §3.1-3.3
    fn create_spec(&self, obj: u16, pos: [f32; 3]) -> CreateSpec {
        CreateSpec {
            obj,
            kind: self.cfg.player_kind,
            pos,
            pos_form: self.cfg.player_pos_form,
            models: Vec::new(),
        }
    }

    fn pos_box(&self) -> PosBox {
        PosBox {
            min: self.world.extent_min,
            max: self.world.extent_max,
        }
    }

    /// In-world clients other than `peer`: `(peer, pid, object id)`.
    fn others_in_world(&self, peer: &A) -> Vec<(A, u8, u16)> {
        self.clients
            .iter()
            .filter(|(p, c)| *p != peer && c.phase == Phase::InWorld)
            .filter_map(|(p, c)| c.object_id.map(|o| (p.clone(), c.pid(), o)))
            .collect()
    }

    /// Phase 2 / 3: create the player's object at a spawn point of its team together with the objects of the
    /// players already in the world, name it as the view object, set the switching-worlds state; the others get
    /// the new object's create record; phase 3. Then the game layer starts with the info request (A2).
    // SPEC: UDP_SESSION_2009_A.md §8.2 (2 / 3), §9.2 S9-S11; UDP_SESSION_2009_B.md §3.1-3.3, §12.2 (create before
    // any record or game message that names the object); GAME_MESSAGES_2009_S2C.md §10 step 3
    // DC-DEFAULT: DC-C2S-12 (proposed) team spawn point (replaces the round-robin of DSV-1)
    fn place_in_world(&mut self, now: Millis, peer: &A) {
        self.plan_world_objects();
        let obj = (FIRST_OBJECT_ID..=u16::MAX - 1)
            .find(|i| !self.objects_used.contains(i))
            .unwrap_or(FIRST_OBJECT_ID);
        self.objects_used.insert(obj);
        let (pid, entry, hello) = match self.clients.get(peer) {
            Some(c) => (c.pid(), c.player.clone(), c.hello.clone()),
            None => return,
        };
        self.vis.forget(pid);
        let team = self.game.team_in_room(entry.as_ref(), pid);
        let (pos, rot) = self.game.choose_spawn_for(team, None, Some(pid));
        let others = self.others_in_world(peer);
        let character = match self.game.character_for(entry.as_ref()) {
            Ok(c) => c,
            Err(e) => {
                self.log(format!(
                    "client {pid}: no character model ({e}); a plain object instead"
                ));
                None
            }
        };
        let weapon_obj = if character.as_ref().is_some_and(|c| c.weapon.is_some()) {
            let w = (FIRST_OBJECT_ID..=u16::MAX - 1)
                .find(|i| !self.objects_used.contains(i))
                .unwrap_or(FIRST_OBJECT_ID);
            self.objects_used.insert(w);
            Some(w)
        } else {
            None
        };
        let has_character = character.is_some();
        self.game.prepare_player(
            pid,
            entry.as_ref(),
            hello.as_ref(),
            obj,
            pos,
            rot,
            character,
            now,
        );
        let reference = self.cfg.game.start_order == StartOrder::Reference;
        if self.clients.get(peer).is_some_and(|c| c.switch_ready_early) {
            self.game.mark_switch_sent(pid);
        }
        let Some(c) = self.clients.get_mut(peer) else {
            return;
        };
        c.object_id = Some(obj);
        c.weapon_obj = weapon_obj;
        c.spawn = Some(pos);
        c.phase = Phase::InWorld;
        if reference {
            // SPEC: UDP_REFERENCE_FINDINGS_botclient.md O2 (identify request, endpoint and settings before the view
            // object; REFERENCE)
            self.game.ask_identity(pid);
            self.apply_game(now);
        }
        // the view object before any group that creates the own character: the client's character effect runs its
        // local-player setup only once, when the create arrives, and only if the engine already names this object as
        // the client's own (the own step sounds, the ladder sound and the own-hit feedback depend on it)
        // OBSERVED: capture 20261025_065016_tdm6 c6 seq 714 < own create seq 743, c5 212 < 230;
        // 20261025_002236_tdm4 c6 799 < 855, c5 205 < 256 (id 7 after the join shell messages, before the world-load
        // groups; REFERENCE, accepted by the real client); 20261025_125432_rust6 c5 seq 804 (ours: id 7 after the
        // create, no own steps)
        // EVIDENCE: cshell+0x222480 (create slot +0x10, then init slot +0x08 at once), cshell+0x203d30 (asks the own
        // object accessor once, then the local-player setup cshell+0x202c70)
        // SPEC: UDP_SESSION_2009_A.md §4.9 (the engine stores the id and resolves it when the object exists)
        // DC-DEFAULT: DSV-23 (amendment, proposed) wire order
        self.send(peer, msg::s2c_view_object(obj));
        // records name file ids: the single-entry file lists go out first (UDP_SESSION_2009_B.md §12.2 order); the
        // world-file objects come first in the group, as in the reference stack's initial groups
        // SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §2.2 (the group with the world objects; REFERENCE, ACCEPTED)
        // the world-file objects in groups of at most WORLD_GROUP_BITS (DSV-37 extended), then the characters
        let world = self.world_records();
        let mut chunk: Vec<BitWriter> = Vec::new();
        let mut bits = 0usize;
        for r in world {
            if !chunk.is_empty() && bits + r.bit_len() + 32 > WORLD_GROUP_BITS {
                if let Some(g) = objects::obj_group(&chunk) {
                    self.send(peer, g);
                }
                chunk.clear();
                bits = 0;
            }
            bits += r.bit_len() + 32;
            chunk.push(r);
        }
        let mut records = chunk;
        records.extend(self.create_records(pid, obj, weapon_obj, pos, true));
        let mine = self.create_records(pid, obj, weapon_obj, pos, false);
        let other_objs: Vec<(u8, u16, Option<u16>)> = others
            .iter()
            .map(|(p, opid, oobj)| (*opid, *oobj, self.clients.get(p).and_then(|c| c.weapon_obj)))
            .collect();
        for (opid, oobj, owobj) in other_objs {
            let r = self.create_records(opid, oobj, owobj, pos, false);
            records.extend(r);
        }
        // co-op AI characters already created (visible or hidden)
        let slots: Vec<usize> = self
            .game
            .ai_chars()
            .iter()
            .filter(|a| a.obj.is_some())
            .map(|a| a.slot)
            .collect();
        for slot in slots {
            records.extend(self.ai_create_records(slot));
        }
        records.extend(self.world_sound_records());
        // the HiSec cases lying in the world (their reservation and HUD state follow at the introduction)
        // DC-DEFAULT: HS-2 (amendment, proposed) a client that enters the world gets the existing cases
        records.extend(self.case_records.values().cloned());
        // DC-DEFAULT: DSV-230 (proposed) and the Spy Hunt intel cases
        records.extend(self.intel_records.values().cloned());
        // the dropped weapons lying in the world
        // DC-DEFAULT: DSV-93 (proposed) a client that enters the world gets the existing drops
        records.extend(self.drop_records.values().cloned());
        // DC-DEFAULT: DSV-220 (proposed) a client that enters the world gets the planted bomb
        records.extend(self.bomb_records.iter().cloned());
        // CLI-1340: and the site indicator a plant switched on
        records.extend(self.fx_extra.values().map(|(_, r)| r.clone()));
        // DC-DEFAULT: DSV-217 (proposed) and the carried flags on their carriers
        records.extend(self.flag_carry_records());
        let group = objects::obj_group(&records).expect("own record");
        self.send(peer, group);
        // the countdown of a reserved case, as the other clients show it
        // DC-DEFAULT: HS-3 (amendment, proposed)
        for (key, shown) in self.game.hisec_countdowns() {
            if let Some(&cobj) = self.cases.get(&key) {
                self.send(peer, crate::gs::hisec_countdown(cobj, shown));
            }
        }
        if !reference {
            self.send(peer, msg::s2c_switch_state(self.cfg.switch_state));
        }
        for (opeer, _, _) in &others {
            let g = objects::obj_group(&mine).expect("own records");
            self.send(opeer, g);
        }
        let Some(c) = self.clients.get_mut(peer) else {
            return;
        };
        c.mark(Step::ObjectCreated, now);
        c.mark(Step::ViewObject, now);
        c.mark(Step::SwitchState, now);
        let id = c.client_id;
        self.log(format!(
            "client {id}: in world, object {obj}{} at ({:.1}, {:.1}, {:.1}), {} other player(s)",
            if has_character { " (character)" } else { "" },
            pos[0],
            pos[1],
            pos[2],
            others.len()
        ));
        if !reference {
            self.game.ask_identity(pid);
        }
        self.offer_move_secret(peer);
        self.apply_game(now);
    }

    /// NET-24: a server that moves links (NET-23) gives each link its move secret once, when the player enters the
    /// world (seated: the hello and its seat ticket were accepted). Only datagrams that carry it move the link.
    // DC-DEFAULT: NET-24 (proposed, project decision) the link move secret, see DC_DEFAULTS.md
    fn offer_move_secret(&mut self, peer: &A) {
        if self.same_host.is_none() {
            return;
        }
        let Some(link) = self.host.link_mut(peer) else {
            return;
        };
        if link.move_secret().is_some() {
            return;
        }
        let secret = fresh_move_secret();
        link.set_move_secret(secret);
        let mut body = BitWriter::new();
        dc_wire::linkmove::write_body(&mut body, &secret);
        self.send(peer, msg::s2c_shell_msg(dc_wire::linkmove::SHELL_ID, &body));
    }

    /// A spawn: the character becomes visible at its spawn point on every in-world client (the player's own client
    /// gets no rotation, CD-67), with its weapon model.
    // SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (spawn change records cf 0x468a / 0x000a; REFERENCE)
    fn on_spawned(&mut self, pid: u8, first: bool) {
        // the first spawn completes the character record on every client (0xa6 sub 0x11)
        // SPEC: CAPTURE_tdm2_TWOPLAYER_VALIDATION.md §1.5, §1.7 (record (1, 255) before the spawn, (3, 2) after it, sent
        // again with sub 0x11; REFERENCE, ACCEPTED)
        if !first {
            return;
        }
        let Some(dims) = self
            .game
            .players()
            .get(&pid)
            .filter(|p| p.character.is_some())
            .map(|_| self.game.dims_of(pid))
        else {
            return;
        };
        let Some(obj) = self
            .clients
            .values()
            .find(|c| c.pid() == pid && c.object_id.is_some())
            .and_then(|c| c.object_id)
        else {
            return;
        };
        let blob = modelobj::CharacterBlob::for_player(pid, dims, true).write();
        let note = crate::gs::character_record_note(obj, &blob);
        let viewers: Vec<A> = self
            .clients
            .iter()
            .filter(|(_, c)| c.phase == Phase::InWorld && c.object_id.is_some())
            .map(|(p, _)| p.clone())
            .collect();
        for peer in viewers {
            self.send(&peer, note.clone());
        }
    }

    /// The character becomes visible on every in-world client (flag words, position; rotation for the others).
    fn on_visible(&mut self, pid: u8, pos: [f32; 3], yaw: f32, now: Millis) {
        let has_character = self
            .game
            .players()
            .get(&pid)
            .is_some_and(|p| p.character.is_some());
        let Some((obj, wobj, gone)) = self
            .clients
            .values()
            .find(|c| c.pid() == pid && c.object_id.is_some())
            .map(|c| (c.object_id.unwrap_or(0), c.weapon_obj, c.weapon_gone))
        else {
            return;
        };
        if !has_character {
            return;
        }
        let pbox = self.pos_box();
        // after a death the weapon model is created again and named in the character's attachment list
        // DC-DEFAULT: DSV-44 (proposed)
        let rebuilt = match (gone, wobj) {
            (true, Some(w)) => self.respawn_weapon_model(pid).map(|m| (w, m)),
            _ => None,
        };
        if gone {
            if let Some(c) = self.clients.values_mut().find(|c| c.pid() == pid) {
                c.weapon_gone = false;
            }
        }
        let socket = self
            .game
            .players()
            .get(&pid)
            .and_then(|p| p.character.as_ref())
            .map_or(crate::chars::TODO_WEAPON_SOCKET_FALLBACK, |c| {
                c.assets.rig.grip_socket
            });
        // after a death: the model block and the trackers from their start again, as the reference stack sent them
        // DC-DEFAULT: DSV-44 (amendment, proposed)
        let respawn_parts = if gone {
            self.game.players().get(&pid).and_then(|p| {
                p.character
                    .clone()
                    .map(|c| (c.anims.trackers(p.motion, 0, 0).to_vec(), c))
            })
        } else {
            None
        }
        .map(|(t, c)| (self.character_models(&c), t));
        let viewers: Vec<(A, u8)> = self
            .clients
            .iter()
            .filter(|(_, c)| c.phase == Phase::InWorld && c.object_id.is_some())
            .map(|(p, c)| (p.clone(), c.pid()))
            .collect();
        let actors = self.vis_actors();
        for (peer, vpid) in viewers {
            let rot = (vpid != pid).then(|| crate::stream::yaw_quat(yaw));
            // DC-DEFAULT: VIS-9 (proposed) a hidden enemy spawns at the park point on this viewer's client
            let at = if vpid != pid && self.vis_spawn_parked(&actors, vpid, pid, now) {
                self.vis.park_point()
            } else {
                pos
            };
            // the reference stack's order: the character's respawn change, then the weapon model's create
            // OBSERVED: capture 20261025_065016_tdm6 (seq 3758, 10964: change `cf` 0x2ba8, then
            // create `cf` 0x01af in the same group; REFERENCE, accepted by the real client)
            let respawned = |weapon| {
                let (models, trackers) = respawn_parts.clone().unwrap_or_default();
                modelobj::character_respawned(obj, at, models, trackers, weapon).write(&pbox)
            };
            let recs = match (&rebuilt, gone) {
                (Some((w, m)), _) => vec![
                    respawned(Some((*w, socket))),
                    modelobj::weapon_recreate(
                        *w,
                        m.clone(),
                        at,
                        crate::game::TODO_WEAPON_ANIM_LEN_MS,
                    )
                    .write(&pbox),
                ],
                (None, true) => vec![respawned(None)],
                (None, false) => {
                    let mut recs = vec![modelobj::character_spawned(obj, at, rot).write(&pbox)];
                    if let Some(w) = wobj {
                        recs.push(modelobj::weapon_spawned(w, at).write(&pbox));
                    }
                    recs
                }
            };
            if let Some(g) = objects::obj_group(&recs) {
                self.send(&peer, g);
            }
        }
    }

    /// The drawn weapon changed: the attached weapon object gets the new model on every in-world client.
    // DC-DEFAULT: DSV-25 (proposed); SPEC: UDP_SESSION_2009_B.md §4.2 item 1 (CF_FILES on a kind-1 object)
    fn on_hand_weapon(&mut self, pid: u8, weapon: u16) {
        let Some(peer) = self
            .clients
            .iter()
            .find(|(_, c)| c.pid() == pid && c.object_id.is_some())
            .map(|(p, _)| p.clone())
        else {
            return;
        };
        let Some(look) = self.game.weapon_look(weapon) else {
            return;
        };
        let pbox = self.pos_box();
        let (obj, wobj) = match self.clients.get(&peer) {
            // a dead player's weapon model is gone: the respawn creates it with the weapon drawn then
            Some(c) if c.weapon_gone => return,
            Some(c) => (c.object_id.unwrap_or(0), c.weapon_obj),
            None => return,
        };
        let recs = match wobj {
            Some(w) => {
                let m = self.weapon_model(&look);
                vec![modelobj::weapon_model_change(w, m).write(&pbox)]
            }
            None => {
                // the character had no weapon model: create the object and attach it at the grip socket
                let w = (FIRST_OBJECT_ID..=u16::MAX - 1)
                    .find(|i| !self.objects_used.contains(i))
                    .unwrap_or(FIRST_OBJECT_ID);
                self.objects_used.insert(w);
                if let Some(c) = self.clients.get_mut(&peer) {
                    c.weapon_obj = Some(w);
                }
                let mut recs = self.create_records(pid, obj, Some(w), [0.0; 3], false);
                // the first record is the character itself: send only its attachment as a change
                recs.remove(0);
                let socket = self
                    .game
                    .players()
                    .get(&pid)
                    .and_then(|p| p.character.as_ref())
                    .map_or(crate::chars::TODO_WEAPON_SOCKET_FALLBACK, |c| {
                        c.assets.rig.grip_socket
                    });
                // DC-DEFAULT: DSV-217 (proposed) a carried flag stays in the list
                match self.attach_list(pid) {
                    Some((cobj, list)) if self.flag_carried.values().any(|f| f.0 == pid) => {
                        recs.push(
                            modelobj::ModelRecord {
                                obj: cobj,
                                attach: Some(list),
                                ..modelobj::ModelRecord::default()
                            }
                            .write(&pbox),
                        );
                    }
                    _ => recs.push(modelobj::character_attach(obj, w, socket).write(&pbox)),
                }
                recs
            }
        };
        let viewers: Vec<A> = self
            .clients
            .iter()
            .filter(|(_, c)| c.phase == Phase::InWorld && c.object_id.is_some())
            .map(|(p, _)| p.clone())
            .collect();
        for v in viewers {
            if let Some(g) = objects::obj_group(&recs) {
                self.send(&v, g);
            }
        }
    }

    /// The weapon model a respawned player holds: the drawn weapon's look, else the character's loadout look.
    fn respawn_weapon_model(&mut self, pid: u8) -> Option<ModelEntry> {
        let p = self.game.players().get(&pid)?;
        let look = p
            .hand_weapon
            .and_then(|w| self.game.weapon_look(w))
            .or_else(|| p.character.as_ref().and_then(|c| c.weapon.clone()))?;
        Some(self.weapon_model(&look))
    }

    /// A death (DSV-44): every in-world client gets one object group with the character's dead flag words and
    /// empty attachment list, the create of a corpse object playing the death clip at the victim's position, and
    /// the removal of the weapon model (last, as removals must be). The corpse is removed after `corpse_ms`.
    // OBSERVED: capture 20261025_065016_tdm6 (the reference stack's death records in the tick of the kill,
    // to both links: character `cf` 0x0188, the corpse create `cf` 0x25bb, the removal of the weapon model; the
    // corpse removed about 18.8 s later; REFERENCE, accepted by the real client)
    // DC-DEFAULT: DSV-44 (proposed)
    fn on_died(&mut self, pid: u8, pos: [f32; 3], yaw: f32, clip: (u16, u32), now: Millis) {
        let Some((obj, wobj)) = self
            .clients
            .values()
            .find(|c| c.pid() == pid && c.object_id.is_some())
            .map(|c| {
                (
                    c.object_id.unwrap_or(0),
                    if c.weapon_gone { None } else { c.weapon_obj },
                )
            })
        else {
            return;
        };
        let Some(look) = self
            .game
            .players()
            .get(&pid)
            .and_then(|p| p.character.clone())
        else {
            return;
        };
        let pbox = self.pos_box();
        // world-fixes: the corpse wears the victim's whole model block, body and face (the head): the 2009 corpse
        // client object adds only its goggle / backpack attachments (cshell+0x2dacc0 CreateGoggle, cshell+0x2d8940);
        // every model it shows comes from the object's model block (UDP_SESSION_2009_B.md §3.4, §3.5)
        // DC-DEFAULT: CLI-585 (proposed; amends DSV-44, whose reference form had the body entry only)
        let models = self.character_models(&look);
        let corpse = self.free_object_id();
        let dims = self.game.dims_of(pid);
        let records = vec![
            modelobj::character_died(obj).write(&pbox),
            modelobj::corpse_create(
                corpse,
                models,
                clip,
                dims,
                pos,
                crate::stream::yaw_quat(yaw),
            )
            .write(&pbox),
        ];
        let gone: Vec<u16> = wobj.into_iter().collect();
        let group = stream::group_with_removal(&records, &gone);
        let viewers: Vec<A> = self
            .clients
            .iter()
            .filter(|(_, c)| c.phase == Phase::InWorld && c.object_id.is_some())
            .map(|(p, _)| p.clone())
            .collect();
        for v in viewers {
            self.send(&v, group.clone());
        }
        if wobj.is_some() {
            if let Some(c) = self.clients.values_mut().find(|c| c.pid() == pid) {
                c.weapon_gone = true;
            }
        }
        let ms = self.cfg.game.corpse_ms;
        self.corpses
            .push((corpse, (ms > 0).then(|| now.wrapping_add(ms))));
    }

    /// Corpses past their time are removed from every in-world client; their ids become free.
    // DC-DEFAULT: DSV-44 (proposed); CD-66 (approved) an id is reused only after its removal record was queued
    fn expire_corpses(&mut self, now: Millis) {
        let due: Vec<u16> = self
            .corpses
            .iter()
            .filter(|(_, t)| t.is_some_and(|t| !later(t, now)))
            .map(|(o, _)| *o)
            .collect();
        if due.is_empty() {
            return;
        }
        self.corpses.retain(|(o, _)| !due.contains(o));
        let viewers: Vec<A> = self
            .clients
            .iter()
            .filter(|(_, c)| c.phase == Phase::InWorld)
            .map(|(p, _)| p.clone())
            .collect();
        for v in viewers {
            if let Some(g) = stream::removal_group(&due) {
                self.send(&v, g);
            }
        }
        for o in due {
            self.objects_used.remove(&o);
        }
    }

    fn free_object_id(&mut self) -> u16 {
        let id = (FIRST_OBJECT_ID..=u16::MAX - 1)
            .find(|i| !self.objects_used.contains(i))
            .unwrap_or(FIRST_OBJECT_ID);
        self.objects_used.insert(id);
        id
    }

    /// The create records of an AI slot as it is now: the character object in the player create form with the NPC
    /// record (`roster_bound` 0, the tail) and its weapon model attached at the grip socket; visible flag words at
    /// its position while alive or lying dead, else the hidden flag words at the park point.
    // SPEC: FIRETEAM_2009.md §1.1, §1.2, §1.8, §1.9 (FT-C1 proposed); UDP_SESSION_2009_B.md §3.3, §3.5.1
    // DC-DEFAULT: FT-C1 (proposed); FT-R1 (proposed) hidden slots at the park point
    fn ai_create_records(&mut self, slot: usize) -> Vec<BitWriter> {
        let pbox = self.pos_box();
        let Some(a) = self.game.ai_chars().get(slot).cloned() else {
            return Vec::new();
        };
        let (Some(obj), Some(tail)) = (a.obj, self.game.ai_tail(slot)) else {
            return Vec::new();
        };
        let look = a.spec.look.clone();
        let models = self.character_models(&look);
        let weapon_model = look.weapon.as_ref().map(|w| self.weapon_model(w));
        let dims = self.game.ai_dims(slot);
        let main = look.anims.main;
        let tracker = |secondary| Tracker {
            anim: main.0,
            state0: true,
            state1: true,
            time_ms: 0,
            len_ms: main.1,
            secondary,
        };
        let visible = a.visible();
        let pos = if visible {
            a.pos
        } else {
            self.vis.park_point()
        };
        let spec = CharacterSpec {
            obj,
            weapon_obj: a.weapon_obj,
            pid: modelobj::NPC_PID,
            pos,
            dims,
            models,
            anim: vec![
                tracker(None),
                tracker(Some(look.anims.secondary[0])),
                tracker(Some(look.anims.secondary[1])),
            ],
            spawned: visible,
            record_complete: true,
            rot: Some(crate::stream::yaw_quat(a.yaw)),
            socket: look.assets.rig.grip_socket,
            weapon_model,
            weapon_anim_len_ms: crate::game::TODO_WEAPON_ANIM_LEN_MS,
        };
        let mut rec = modelobj::character_create(&spec);
        rec.blob = Some(modelobj::CharacterBlob::for_npc(dims, tail).write());
        let mut out = vec![rec.write(&pbox)];
        if let Some(w) = modelobj::weapon_create(&spec) {
            out.push(w.write(&pbox));
        }
        out
    }

    /// A new AI slot: two object ids (the weapon one only with a weapon model), bound in the game, the create
    /// records to every client in the world.
    // DC-DEFAULT: CD-66 (approved) lowest free id; FT-2 (proposed) two ids per AI
    fn on_ai_create(&mut self, slot: usize) {
        let has_weapon = self
            .game
            .ai_chars()
            .get(slot)
            .is_some_and(|a| a.spec.look.weapon.is_some());
        let obj = self.free_object_id();
        let wobj = has_weapon.then(|| self.free_object_id());
        self.game.bind_ai(slot, obj, wobj);
        let recs = self.ai_create_records(slot);
        for v in self.world_viewers() {
            if let Some(g) = objects::obj_group(&recs) {
                self.send(&v, g);
            }
        }
        self.log(format!(
            "co-op: AI slot {slot} created as object {obj}{}",
            wobj.map(|w| format!(" (weapon {w})")).unwrap_or_default()
        ));
    }

    fn ai_objects(&self, slot: usize) -> Option<(u16, Option<u16>, [f32; 3], f32)> {
        let a = self.game.ai_chars().get(slot)?;
        Some((a.obj?, a.weapon_obj, a.pos, a.yaw))
    }

    /// A reused AI becomes visible at its spawn (spawned flag words, position, rotation).
    fn on_ai_show(&mut self, slot: usize) {
        let Some((obj, wobj, pos, yaw)) = self.ai_objects(slot) else {
            return;
        };
        let pbox = self.pos_box();
        let mut recs =
            vec![
                modelobj::character_spawned(obj, pos, Some(crate::stream::yaw_quat(yaw)))
                    .write(&pbox),
            ];
        if let Some(w) = wobj {
            recs.push(modelobj::weapon_spawned(w, pos).write(&pbox));
        }
        for v in self.world_viewers() {
            if let Some(g) = objects::obj_group(&recs) {
                self.send(&v, g);
            }
        }
    }

    /// A dead AI is hidden at the park point; its objects stay (FT-R1).
    // DC-DEFAULT: FT-R1 (proposed)
    fn on_ai_hide(&mut self, slot: usize) {
        let Some((obj, wobj, _, _)) = self.ai_objects(slot) else {
            return;
        };
        let pbox = self.pos_box();
        let park = self.vis.park_point();
        let mut recs = vec![modelobj::character_hidden(obj, park).write(&pbox)];
        if let Some(w) = wobj {
            recs.push(modelobj::weapon_hidden(w, park).write(&pbox));
        }
        for v in self.world_viewers() {
            if let Some(g) = objects::obj_group(&recs) {
                self.send(&v, g);
            }
        }
    }

    /// A reused AI slot gets the body and the weapon model of its new spec.
    fn on_ai_look(&mut self, slot: usize) {
        let Some((obj, wobj, _, _)) = self.ai_objects(slot) else {
            return;
        };
        let Some(look) = self.game.ai_chars().get(slot).map(|a| a.spec.look.clone()) else {
            return;
        };
        let pbox = self.pos_box();
        let models = self.character_models(&look);
        let mut recs = vec![modelobj::character_model_change(obj, models).write(&pbox)];
        if let (Some(w), Some(wl)) = (wobj, look.weapon.as_ref()) {
            let m = self.weapon_model(wl);
            recs.push(modelobj::weapon_model_change(w, m).write(&pbox));
        }
        for v in self.world_viewers() {
            if let Some(g) = objects::obj_group(&recs) {
                self.send(&v, g);
            }
        }
    }

    /// Clients whose world holds the objects of others (in the world, own object placed).
    fn world_viewers(&self) -> Vec<A> {
        self.clients
            .iter()
            .filter(|(_, c)| c.phase == Phase::InWorld && c.object_id.is_some())
            .map(|(p, _)| p.clone())
            .collect()
    }

    /// A thrown object: a new id (lowest free), its model and skin announced, the create record to every client in
    /// the world. Clients that enter the world during the flight do not get it.
    // SPEC: UDP_SESSION_2009_B.md §3.5 (thrown grenade: kind-1 object with the type-0x13 blob; REFERENCE, ACCEPTED),
    // part A §4.4 (files announced before the record that uses them)
    // DC-DEFAULT: DSV-39 (proposed); CD-66 (approved) lowest free id
    fn on_projectile(&mut self, p: &crate::game::ProjectileOut) {
        let Some(look) = &p.look else {
            return;
        };
        let obj = (FIRST_OBJECT_ID..=u16::MAX - 1)
            .find(|i| !self.objects_used.contains(i))
            .unwrap_or(FIRST_OBJECT_ID);
        self.objects_used.insert(obj);
        self.projectiles.insert(p.key, obj);
        let model = self.weapon_model(look);
        let rec = modelobj::projectile_create(&modelobj::ProjectileSpec {
            obj,
            weapon: p.weapon,
            ammo: p.ammo,
            model,
            scale: p.scale,
            pos: p.pos,
            vel: p.vel,
            rot: p.rot,
            anim_len_ms: crate::game::TODO_WEAPON_ANIM_LEN_MS,
        })
        .write(&self.pos_box());
        for v in self.world_viewers() {
            if let Some(g) = objects::obj_group(std::slice::from_ref(&rec)) {
                self.send(&v, g);
            }
        }
    }

    /// The thrown object of `key` is gone: a removal record to every client in the world, then the id is free.
    // SPEC: UDP_SESSION_2009_B.md §3.5 (removed by a kind-3 record when it explodes), §5.1
    // DC-DEFAULT: CD-66 (approved) an id is reused only after its removal record was queued
    fn on_projectile_gone(&mut self, key: u32) {
        let Some(obj) = self.projectiles.remove(&key) else {
            return;
        };
        for v in self.world_viewers() {
            if let Some(g) = stream::removal_group(&[obj]) {
                self.send(&v, g);
            }
        }
        self.objects_used.remove(&obj);
    }

    /// A HiSec case: a new id (lowest free), its model, skin and render style announced, the create record to every
    /// client in the world (before the 0xd6 messages the game queued after it). Clients that enter the world later
    /// do not get it.
    // SPEC: HISEC_2009.md §3.1, §6.1; UDP_SESSION_2009_A.md §4.4 (files announced before the record that uses them)
    // OBSERVED: capture 20261025_065016_tdm6 c6 seq 3015 (the case create; REFERENCE, accepted)
    // DC-DEFAULT: HS-2 (proposed); CD-66 (approved) lowest free id
    fn on_case_create(&mut self, c: &crate::game::hisec::CaseOut) {
        let obj = self.free_object_id();
        self.cases.insert(c.key, obj);
        let file = self.file_ids(std::slice::from_ref(&c.look.model))[0];
        let textures = self.file_ids(&c.look.textures);
        let styles = self.file_ids(&c.look.styles);
        let rec = modelobj::case_create(&modelobj::CaseSpec {
            obj,
            case_key: c.key,
            model: ModelEntry {
                file,
                subs: Vec::new(),
                textures,
                styles,
            },
            scale: c.look.scale,
            pos: c.pos,
            rot: crate::stream::yaw_quat(c.yaw),
        })
        .write(&self.pos_box());
        for v in self.world_viewers() {
            if let Some(g) = objects::obj_group(std::slice::from_ref(&rec)) {
                self.send(&v, g);
            }
        }
        self.case_records.insert(c.key, rec);
    }

    /// An event sound (DSV-64): a track id from the object id space (lowest free), its file announced, the tracked
    /// sound record to every client in the world; the track is removed after the sound's life.
    // SPEC: UDP_SESSION_2009_B.md §8.3 (tracked sound, removal through the removed-object record), part A §4.4
    // (files announced before the record that uses them)
    // OBSERVED: capture 20261025_002236_tdm4 c6 seq 2968 / 3100 (start, removal 0.84 s later);
    // REFERENCE
    // DC-DEFAULT: DSV-64 (proposed); CD-66 (approved) lowest free id
    fn on_event_sound(&mut self, s: &crate::sounds::SoundOut, now: Millis) {
        let viewers = self.world_viewers();
        if viewers.is_empty() {
            return;
        }
        let track = self.free_object_id();
        let sfile = self.file_ids(std::slice::from_ref(&s.file))[0];
        let rec = crate::worldobj::event_sound_record(track, sfile, s.form, s.pos, &self.pos_box());
        for v in &viewers {
            if let Some(g) = objects::obj_group(std::slice::from_ref(&rec)) {
                self.send(v, g);
            }
        }
        self.event_tracks
            .push((track, now.wrapping_add(s.life_ms), viewers));
    }

    /// Removes the event sound tracks whose life ended: one removal record to each client that was sent the start
    /// and is still in the world (a client that entered later never had the track and gets nothing).
    // DC-DEFAULT: DSV-64 (proposed; amendment: recipients = the start's); CD-66 (approved) an id is reused only
    // after its removal record was queued
    fn expire_event_sounds(&mut self, now: Millis) {
        if !self.event_tracks.iter().any(|(_, t, _)| !later(*t, now)) {
            return;
        }
        let (due, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.event_tracks)
            .into_iter()
            .partition(|(_, t, _)| !later(*t, now));
        self.event_tracks = keep;
        let mut per_client: Vec<(A, Vec<u16>)> = Vec::new();
        for (o, _, to) in &due {
            for v in to {
                if !self
                    .clients
                    .get(v)
                    .is_some_and(|c| c.phase == Phase::InWorld)
                {
                    continue;
                }
                match per_client.iter_mut().find(|(p, _)| p == v) {
                    Some((_, objs)) => objs.push(*o),
                    None => per_client.push((v.clone(), vec![*o])),
                }
            }
        }
        for (v, objs) in per_client {
            if let Some(g) = stream::removal_group(&objs) {
                self.send(&v, g);
            }
        }
        for (o, _, _) in due {
            self.objects_used.remove(&o);
        }
    }

    /// The countdown note to the case object of `key` (0xa6 type 0x17 sub 2) to every client in the world.
    // SPEC: GAME_MESSAGES_2009_S2C.md §8.3; DC-DEFAULT: HS-3 (proposed)
    fn on_case_note(&mut self, key: u32, remaining: u32) {
        let Some(&obj) = self.cases.get(&key) else {
            return;
        };
        let m = crate::gs::hisec_countdown(obj, remaining);
        for v in self.world_viewers() {
            self.send(&v, m.clone());
        }
    }

    /// The case of `key` is gone: a removal record to every client in the world, then the id is free.
    // SPEC: HISEC_2009.md §6.1 (sub 1, then remove the object); UDP_SESSION_2009_B.md §5.1
    // OBSERVED: capture 20261025_065016_tdm6 c6 seq 10688 / 10691 (0xd6 sub 1, then the
    // removal; REFERENCE)
    // DC-DEFAULT: CD-66 (approved) an id is reused only after its removal record was queued
    /// A Spy Hunt intel case: a new id (lowest free), its model, skin and style announced, the kind-3 create record
    /// to every client in the world (DSV-230).
    // SPEC: GAME_MODES_2009.md §4.4 (kind 3); DC-DEFAULT: DSV-230 (proposed)
    fn on_intel_create(&mut self, c: &crate::game::spy::IntelOut) {
        let obj = self.free_object_id();
        self.intels.insert(c.key, obj);
        let file = self.file_ids(std::slice::from_ref(&c.model))[0];
        let textures = self.file_ids(&c.textures);
        let styles = self.file_ids(&c.styles);
        let rec = modelobj::intel_create(&modelobj::CaseSpec {
            obj,
            case_key: c.key,
            model: ModelEntry {
                file,
                subs: Vec::new(),
                textures,
                styles,
            },
            scale: [1.0; 3],
            pos: c.pos,
            rot: crate::stream::yaw_quat(0.0),
        })
        .write(&self.pos_box());
        for v in self.world_viewers() {
            if let Some(g) = objects::obj_group(std::slice::from_ref(&rec)) {
                self.send(&v, g);
            }
        }
        self.intel_records.insert(c.key, rec);
    }

    fn on_intel_gone(&mut self, key: u32) {
        self.intel_records.remove(&key);
        let Some(obj) = self.intels.remove(&key) else {
            return;
        };
        for v in self.world_viewers() {
            if let Some(g) = stream::removal_group(&[obj]) {
                self.send(&v, g);
            }
        }
        self.objects_used.remove(&obj);
    }

    fn on_case_gone(&mut self, key: u32) {
        self.case_records.remove(&key);
        let Some(obj) = self.cases.remove(&key) else {
            return;
        };
        for v in self.world_viewers() {
            if let Some(g) = stream::removal_group(&[obj]) {
                self.send(&v, g);
            }
        }
        self.objects_used.remove(&obj);
    }

    /// A dropped weapon: a new id (lowest free), the weapon's third-person model, skin and render style announced,
    /// the create record to every client in the world.
    // SPEC: GAME_MODES_2009.md §4.4 (pickup kind 6); UDP_SESSION_2009_A.md §4.4 (files announced before the record
    // that uses them)
    // OBSERVED: capture 20261025_065016_tdm6 c6 seq 3015 (the drop create in the tick of the
    // kill, to both links; REFERENCE, accepted by the real client)
    // DC-DEFAULT: DSV-93 (proposed); CD-66 (approved) lowest free id
    fn on_drop_create(&mut self, d: &crate::game::drops::DropOut) {
        let obj = self.free_object_id();
        self.drops.insert(d.key, obj);
        let model = self.weapon_model(&d.look);
        let rec = modelobj::weapon_drop_create(&modelobj::DropSpec {
            obj,
            weapon: d.look.weapon,
            rounds: d.rounds,
            model,
            pos: d.pos,
            rot: crate::stream::yaw_quat(d.yaw),
        })
        .write(&self.pos_box());
        for v in self.world_viewers() {
            if let Some(g) = objects::obj_group(std::slice::from_ref(&rec)) {
                self.send(&v, g);
            }
        }
        self.drop_records.insert(d.key, rec);
    }

    /// The drop of `key` is gone: a removal record to every client in the world, then the id is free.
    // SPEC: UDP_SESSION_2009_B.md §5.1
    // OBSERVED: capture 20261025_065016_tdm6 c6 seq 5442 (the drop removed by a removal record;
    // REFERENCE)
    // DC-DEFAULT: CD-66 (approved) an id is reused only after its removal record was queued
    fn on_drop_gone(&mut self, key: u32) {
        self.drop_records.remove(&key);
        let Some(obj) = self.drops.remove(&key) else {
            return;
        };
        for v in self.world_viewers() {
            if let Some(g) = stream::removal_group(&[obj]) {
                self.send(&v, g);
            }
        }
        self.objects_used.remove(&obj);
    }

    /// A breakable died (DSV-117): its brush object gets the flag words without visible / solid / ray-hit on every
    /// client in the world (and keeps them for clients that join later), and each death effect the world file names
    /// becomes a placed-effect object at that effect's position (DSV-118).
    // SPEC: UDP_SESSION_2009_B.md §4.2 item 3; GAME_MESSAGES_2009_S2C.md §8.4 (type 0x64)
    // DC-DEFAULT: DSV-117, DSV-118 (proposed)
    fn on_world_break(&mut self, b: &crate::game::destruct::BreakOut) {
        let mut recs = Vec::new();
        if let Some(objs) = self.world_objs.as_mut() {
            for o in objs.iter_mut() {
                if let crate::worldobj::WorldObject::Brush { obj, brush } = o {
                    if *obj == b.obj {
                        // DC-DEFAULT: RND-5 (staged) the created form is kept for the round restore
                        self.broken_brushes
                            .entry(*obj)
                            .or_insert_with(|| brush.clone());
                        *brush = crate::destructibles::broken(brush);
                        recs.push(crate::destructibles::brush_flags_record(*obj, brush));
                    }
                }
            }
        }
        let pbox = self.pos_box();
        for (key, name) in &b.effects {
            let Some(e) = self
                .world
                .effects
                .iter()
                .find(|e| e.name.eq_ignore_ascii_case(name))
                .cloned()
            else {
                continue;
            };
            let obj = self.free_object_id();
            self.world_fx.insert(*key, obj);
            recs.push(crate::worldobj::effect_record(obj, &e, &pbox));
        }
        if let Some(g) = objects::obj_group(&recs) {
            for v in self.world_viewers() {
                self.send(&v, g.clone());
            }
        }
    }

    /// A new round restores dead breakables (RND-5): each brush gets the flag words it was created with again, on
    /// every client in the world (and for clients that join later).
    // SPEC: UDP_SESSION_2009_B.md §4.2 item 3 (flag words of a change record)
    // DC-DEFAULT: RND-5 (staged)
    fn on_world_restore(&mut self, which: &[u16]) {
        let mut recs = Vec::new();
        if let Some(objs) = self.world_objs.as_mut() {
            for o in objs.iter_mut() {
                if let crate::worldobj::WorldObject::Brush { obj, brush } = o {
                    if !which.contains(obj) {
                        continue;
                    }
                    if let Some(orig) = self.broken_brushes.remove(obj) {
                        *brush = crate::destructibles::restored(brush, &orig);
                        recs.push(crate::destructibles::brush_flags_record(*obj, brush));
                    }
                }
            }
        }
        if let Some(g) = objects::obj_group(&recs) {
            for v in self.world_viewers() {
                self.send(&v, g.clone());
            }
        }
    }

    /// A death effect's time is up: a removal record to every client in the world, then the id is free.
    // DC-DEFAULT: DSV-118 (proposed); CD-66 (approved)
    /// A capture flag's new place on every in-world client (a change record with its shown flag words and the position
    /// as a cut); the planned object keeps the place for later joiners' creates.
    // DC-DEFAULT: DSV-193 (proposed)
    fn on_flag_move(&mut self, obj: u16, pos: [f32; 3]) {
        if let Some(objs) = self.world_objs.as_mut() {
            for o in objs.iter_mut() {
                if let crate::worldobj::WorldObject::Flag {
                    obj: o_obj, flag, ..
                } = o
                {
                    if *o_obj == obj {
                        flag.pos = pos;
                    }
                }
            }
        }
        let pbox = self.pos_box();
        let rec = crate::worldobj::flag_move_record(obj, pos, &pbox);
        if let Some(g) = objects::obj_group(&[rec]) {
            for v in self.world_viewers() {
                self.send(&v, g.clone());
            }
        }
    }

    /// A bomb was planted (DSV-220): on every in-world client the bomb model at `pos`, its looping indicator effect
    /// and the defuse point (a kind-5 `z_sub` 2 zone of the planted site, moved to the bomb).
    // SPEC: GAME_MODES_2009.md §5.2 step 3 (the defuse zone type 0x37 kind 5 sub 2 at the bomb, PLAUSIBLY a world
    // object of the bomb), §4.3 (zone record); GAME_MESSAGES_2009_S2C.md §8.4 (type 0x64)
    // DC-DEFAULT: DSV-220 (proposed); DSV-222 (proposed); CD-66 (approved) lowest free ids
    fn on_bomb_shown(&mut self, site: i32, pos: [f32; 3]) {
        self.on_bomb_hidden();
        let look = self.bomb_look.clone().unwrap_or_default();
        let pbox = self.pos_box();
        let mut recs = Vec::new();
        if let Some(zone) = self
            .world
            .zones
            .iter()
            .find(|z| z.kind == crate::world::ZoneKind::BombSite && z.number == site)
            .cloned()
        {
            let obj = self.free_object_id();
            let reach = crate::game::bomb::DEFUSE_REACH;
            let point = crate::worldobj::WorldObject::Zone {
                obj,
                zone: crate::world::ZoneInfo {
                    pos,
                    rot: [0.0; 3],
                    dims: [reach; 3],
                    ..zone
                },
                team: crate::worldobj::ZONE_TEAM_ANY,
                class: crate::worldobj::CLASS_BOMB,
                point: crate::worldobj::ZONE_SUB_DEFUSE,
            };
            recs.push(point.record(&pbox, &|_| 0));
            self.bomb_objs.push(obj);
        }
        if let Some(model) = look.model.clone() {
            let file = self.file_ids(std::slice::from_ref(&model))[0];
            let textures = match &look.skin {
                Some(s) => self.file_ids(std::slice::from_ref(s)),
                None => Vec::new(),
            };
            let obj = self.free_object_id();
            let m = ModelEntry {
                file,
                subs: Vec::new(),
                textures,
                styles: Vec::new(),
            };
            recs.push(crate::worldobj::bomb_record(obj, m, pos, 0.0, &pbox));
            self.bomb_objs.push(obj);
        }
        if let Some(fx) = look.light_fx.clone() {
            let obj = self.free_object_id();
            let e = crate::world::EffectInfo {
                name: Vec::new(),
                pos,
                rot: [0.0; 3],
                fx,
                looped: true,
                start_on: true,
            };
            recs.push(crate::worldobj::effect_record(obj, &e, &pbox));
            self.bomb_objs.push(obj);
        }
        self.log(format!(
            "bomb shown at ({:.0}, {:.0}, {:.0}): objects {:?}",
            pos[0], pos[1], pos[2], self.bomb_objs
        ));
        if let Some(g) = objects::obj_group(&recs) {
            for v in self.world_viewers() {
                self.send(&v, g.clone());
            }
        }
        self.bomb_records = recs;
    }

    /// A player's character look changed (DSV-214): the character's model block on every in-world client.
    // SPEC: UDP_SESSION_2009_B.md §3.4 (model block of a change record); DC-DEFAULT: DSV-214 (proposed)
    fn on_character_look(&mut self, pid: u8) {
        let Some(obj) = self
            .clients
            .values()
            .find(|c| c.pid() == pid)
            .and_then(|c| c.object_id)
        else {
            return;
        };
        let Some(look) = self
            .game
            .players()
            .get(&pid)
            .and_then(|p| p.character.clone())
        else {
            return;
        };
        let models = self.character_models(&look);
        let rec = modelobj::character_model_change(obj, models).write(&self.pos_box());
        if let Some(g) = objects::obj_group(&[rec]) {
            for v in self.world_viewers() {
                self.send(&v, g.clone());
            }
        }
        self.log(format!("client {pid}: character look changed"));
    }

    /// The attachment list of `pid`'s character: the drawn weapon (unless it is gone with a death) and every flag he
    /// carries at its socket.
    // SPEC: UDP_SESSION_2009_B.md §4.2 item 8 (the reader clears the attachments, then adds one per entry)
    // DC-DEFAULT: DSV-217 (proposed)
    fn attach_list(&self, pid: u8) -> Option<(u16, Vec<modelobj::Attachment>)> {
        let c = self.clients.values().find(|c| c.pid() == pid)?;
        let obj = c.object_id?;
        let mut list = Vec::new();
        if let (Some(w), false) = (c.weapon_obj, c.weapon_gone) {
            let socket = self
                .game
                .players()
                .get(&pid)
                .and_then(|p| p.character.as_ref())
                .map_or(crate::chars::TODO_WEAPON_SOCKET_FALLBACK, |c| {
                    c.assets.rig.grip_socket
                });
            list.push(modelobj::Attachment {
                child: w,
                socket,
                offset: [0.0; 3],
                rot: modelobj::IDENTITY_ROT,
            });
        }
        for (flag, (carrier, socket)) in &self.flag_carried {
            if *carrier == pid {
                list.push(modelobj::Attachment {
                    child: *flag,
                    socket: *socket,
                    offset: [0.0; 3],
                    rot: modelobj::IDENTITY_ROT,
                });
            }
        }
        Some((obj, list))
    }

    /// The model of a capture flag: carried (the flag row's carried model with the row's textures and styles) or as
    /// the world created it.
    fn flag_model(&mut self, obj: u16, team: u8, carried: bool) -> Option<ModelEntry> {
        let (model, textures, styles) = self.world_objs.as_ref()?.iter().find_map(|o| match o {
            crate::worldobj::WorldObject::Flag {
                obj: o_obj,
                model,
                textures,
                styles,
                ..
            } if *o_obj == obj => Some((model.clone(), textures.clone(), styles.clone())),
            _ => None,
        })?;
        let model = if carried {
            // SPEC: reference flags (team 1's flag carried the files of [Flag1]; tests/rom_world.rs)
            self.game
                .tables()
                .flag_looks
                .carried
                .get(&(u32::from(team) % 2))
                .map_or(model, |c| c.0.clone())
        } else {
            model
        };
        let file = self.file_ids(std::slice::from_ref(&model))[0];
        let textures = self.file_ids(&textures);
        let styles = self.file_ids(&styles);
        Some(ModelEntry {
            file,
            subs: Vec::new(),
            textures,
            styles,
        })
    }

    /// A capture flag changed hands (DSV-217): the carrier's attachment list with the flag at the flag row's socket
    /// and the flag in its carried form on every client; the last carrier's list without it and the flag in its ground
    /// form again (the flag's place follows as its move record).
    // OBSERVED: ROM attachment table [Flag<k>] (the carried model and its body socket)
    // DC-DEFAULT: DSV-217 (proposed)
    fn on_flag_carry(&mut self, obj: u16, team: u8, carrier: Option<u8>, before: Option<u8>) {
        let pbox = self.pos_box();
        let mut recs = Vec::new();
        self.flag_carried.remove(&obj);
        if let Some(b) = before {
            if let Some((cobj, list)) = self.attach_list(b) {
                recs.push(
                    modelobj::ModelRecord {
                        obj: cobj,
                        attach: Some(list),
                        ..modelobj::ModelRecord::default()
                    }
                    .write(&pbox),
                );
            }
        }
        let socket = carrier.and_then(|pid| {
            let name = self
                .game
                .tables()
                .flag_looks
                .carried
                .get(&(u32::from(team) % 2))?
                .1
                .clone();
            self.game
                .players()
                .get(&pid)?
                .character
                .as_ref()?
                .assets
                .rig
                .socket_at(&name)
        });
        match (carrier, socket) {
            (Some(pid), Some(socket)) => {
                self.flag_carried.insert(obj, (pid, socket));
                if let Some(m) = self.flag_model(obj, team, true) {
                    recs.push(crate::worldobj::flag_model_record(obj, m, &pbox));
                }
                if let Some((cobj, list)) = self.attach_list(pid) {
                    recs.push(
                        modelobj::ModelRecord {
                            obj: cobj,
                            attach: Some(list),
                            ..modelobj::ModelRecord::default()
                        }
                        .write(&pbox),
                    );
                }
                self.log(format!(
                    "ctf: flag {obj} attached to client {pid} at socket {socket}"
                ));
            }
            _ => {
                if before.is_some() {
                    if let Some(m) = self.flag_model(obj, team, false) {
                        recs.push(crate::worldobj::flag_model_record(obj, m, &pbox));
                    }
                }
            }
        }
        if let Some(g) = objects::obj_group(&recs) {
            for v in self.world_viewers() {
                self.send(&v, g.clone());
            }
        }
    }

    /// For a client entering the world: each carried flag in its carried form and its carrier's attachment list.
    fn flag_carry_records(&mut self) -> Vec<BitWriter> {
        let pbox = self.pos_box();
        let carried: Vec<(u16, u8)> = self
            .flag_carried
            .iter()
            .map(|(f, (c, _))| (*f, *c))
            .collect();
        let mut recs = Vec::new();
        for (flag, carrier) in carried {
            let team = self
                .game
                .ctf()
                .flags
                .iter()
                .find(|f| f.obj == flag)
                .map_or(0, |f| f.team);
            if let Some(m) = self.flag_model(flag, team, true) {
                recs.push(crate::worldobj::flag_model_record(flag, m, &pbox));
            }
            if let Some((cobj, list)) = self.attach_list(carrier) {
                recs.push(
                    modelobj::ModelRecord {
                        obj: cobj,
                        attach: Some(list),
                        ..modelobj::ModelRecord::default()
                    }
                    .write(&pbox),
                );
            }
        }
        recs
    }

    /// The planted bomb's objects leave every in-world client.
    // SPEC: UDP_SESSION_2009_B.md §5.1; DC-DEFAULT: DSV-220 (proposed); CD-66 (approved)
    fn on_bomb_hidden(&mut self) {
        self.bomb_records.clear();
        let objs = std::mem::take(&mut self.bomb_objs);
        if objs.is_empty() {
            return;
        }
        if let Some(g) = stream::removal_group(&objs) {
            for v in self.world_viewers() {
                self.send(&v, g.clone());
            }
        }
        for o in objs {
            self.objects_used.remove(&o);
        }
    }

    /// The bomb blew up (DSV-221): the blast effect group (played once) at `pos` on every in-world client.
    // SPEC: GAME_MESSAGES_2009_S2C.md §8.4 (type 0x64); DC-DEFAULT: DSV-221 (proposed)
    fn on_bomb_blast(&mut self, pos: [f32; 3]) {
        self.on_bomb_blast_gone();
        let Some(fx) = self.bomb_look.as_ref().and_then(|l| l.blast_fx.clone()) else {
            return;
        };
        let obj = self.free_object_id();
        let e = crate::world::EffectInfo {
            name: Vec::new(),
            pos,
            rot: [0.0; 3],
            fx,
            looped: false,
            start_on: true,
        };
        let rec = crate::worldobj::effect_record(obj, &e, &self.pos_box());
        if let Some(g) = objects::obj_group(&[rec]) {
            for v in self.world_viewers() {
                self.send(&v, g.clone());
            }
        }
        self.log(format!("bomb blast effect object {obj}"));
        self.bomb_blast = Some(obj);
    }

    fn on_bomb_blast_gone(&mut self) {
        let Some(obj) = self.bomb_blast.take() else {
            return;
        };
        if let Some(g) = stream::removal_group(&[obj]) {
            for v in self.world_viewers() {
                self.send(&v, g.clone());
            }
        }
        self.objects_used.remove(&obj);
    }

    /// The world's placed effect `name` switched on under `key`: a placed-effect object on every client in the
    /// world (the Spy Hunt station, DSV-323).
    // DC-DEFAULT: DSV-323 (proposed) as the breakables' death effects (DSV-118)
    fn on_world_fx(&mut self, key: u32, name: &[u8]) {
        self.on_world_fx_gone(key);
        let Some(e) = self
            .world
            .effects
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(name))
            .cloned()
        else {
            return;
        };
        let obj = self.free_object_id();
        self.world_fx.insert(key, obj);
        let rec = crate::worldobj::effect_record(obj, &e, &self.pos_box());
        if let Some(g) = objects::obj_group(&[rec]) {
            for v in self.world_viewers() {
                self.send(&v, g.clone());
            }
        }
        self.log(format!(
            "world effect {} on (object {obj})",
            String::from_utf8_lossy(name)
        ));
    }

    /// A planned world effect object of this name (CLI-1340).
    fn planned_effect(&self, name: &[u8]) -> Option<(u16, crate::world::EffectInfo)> {
        self.world_objects().iter().find_map(|o| match o {
            crate::worldobj::WorldObject::Effect { obj, effect } if effect.name.eq_ignore_ascii_case(name) => {
                Some((*obj, effect.clone()))
            }
            _ => None,
        })
    }

    fn send_world_group(&mut self, recs: &[BitWriter]) {
        if let Some(g) = objects::obj_group(recs) {
            for v in self.world_viewers() {
                self.send(&v, g.clone());
            }
        }
    }

    fn send_world_removal(&mut self, objs: &[u16]) {
        if let Some(g) = stream::removal_group(objs) {
            for v in self.world_viewers() {
                self.send(&v, g.clone());
            }
        }
    }

    /// The world effect of this name on or off on every in-world client (CLI-1340): a planned one is removed and
    /// created again under its own id, another one gets a new object.
    // DC-DEFAULT: CLI-1340 (proposed) the bomb site's planted command switches the site indicators
    fn on_world_fx_switch(&mut self, name: &[u8], on: bool) {
        let pbox = self.pos_box();
        if let Some((obj, e)) = self.planned_effect(name) {
            if on && self.fx_hidden.remove(&obj) {
                let rec = crate::worldobj::effect_record(obj, &e, &pbox);
                self.send_world_group(&[rec]);
            } else if !on && self.fx_hidden.insert(obj) {
                self.send_world_removal(&[obj]);
            } else {
                return;
            }
        } else {
            let key = name.to_ascii_lowercase();
            if on {
                if self.fx_extra.contains_key(&key) {
                    return;
                }
                let Some(e) = self
                    .world
                    .effects
                    .iter()
                    .find(|e| e.name.eq_ignore_ascii_case(name))
                    .cloned()
                else {
                    return;
                };
                let obj = self.free_object_id();
                let rec = crate::worldobj::effect_record(obj, &e, &pbox);
                self.send_world_group(std::slice::from_ref(&rec));
                self.fx_extra.insert(key, (obj, rec));
            } else {
                let Some((obj, _)) = self.fx_extra.remove(&key) else {
                    return;
                };
                self.send_world_removal(&[obj]);
                self.objects_used.remove(&obj);
            }
        }
        self.log(format!(
            "world effect {} switched {}",
            String::from_utf8_lossy(name),
            if on { "on" } else { "off" }
        ));
    }

    /// Every effect a plant switched goes back to the world's plan (CLI-1340).
    fn on_world_fx_restore(&mut self) {
        let pbox = self.pos_box();
        let back: Vec<BitWriter> = std::mem::take(&mut self.fx_hidden)
            .into_iter()
            .filter_map(|obj| {
                self.world_objects().iter().find_map(|o| match o {
                    crate::worldobj::WorldObject::Effect { obj: o, effect } if *o == obj => {
                        Some(crate::worldobj::effect_record(obj, effect, &pbox))
                    }
                    _ => None,
                })
            })
            .collect();
        let gone: Vec<u16> = std::mem::take(&mut self.fx_extra)
            .into_values()
            .map(|(obj, _)| obj)
            .collect();
        if !gone.is_empty() {
            self.send_world_removal(&gone);
            for o in &gone {
                self.objects_used.remove(o);
            }
        }
        if !back.is_empty() {
            self.send_world_group(&back);
        }
        self.log(format!(
            "world effects restored: {} back, {} removed",
            back.len(),
            gone.len()
        ));
    }

    fn on_world_fx_gone(&mut self, key: u32) {
        let Some(obj) = self.world_fx.remove(&key) else {
            return;
        };
        for v in self.world_viewers() {
            if let Some(g) = stream::removal_group(&[obj]) {
                self.send(&v, g);
            }
        }
        self.objects_used.remove(&obj);
    }

    fn on_shell(&mut self, now: Millis, peer: &A, reliable: bool, shell_id: u8, body: BitWriter) {
        let Some(c) = self.clients.get_mut(peer) else {
            return;
        };
        *c.shell_counts.entry(shell_id).or_default() += 1;
        let first_of_id = c.shell_counts[&shell_id] == 1;
        let id = c.client_id;
        let pid = c.pid();
        let in_game = c.object_id.is_some();
        if shell_id == SHELL_PLAYER_UPDATE {
            // SPEC: UDP_SESSION_2009_B.md §10.3, §10.5; GAME_MESSAGES_2009_C2S.md §2.1 (never trusted)
            let u = parse_shell64(&body);
            c.updates += 1;
            c.watch
                .on_update(now, u.trailer.as_ref().map_or(f32::NAN, |t| t.time()));
            c.mark(Step::FirstUpdate, now);
            let first = c.updates == 1;
            c.last_update = Some(u.clone());
            if first {
                self.log(format!(
                    "client {id}: first player update ({}, flags {:#06x}, position {:?})",
                    if reliable { "reliable" } else { "unreliable" },
                    u.flags,
                    u.position()
                ));
            }
            if in_game {
                self.game.on_update(pid, &u, now);
            }
        } else {
            let msg = up::parse_up(shell_id, &body);
            match &msg {
                Up::Unknown
                | Up::Event { .. }
                | Up::Sprint { .. }
                | Up::ObserveTarget { .. }
                | Up::ObserveFree
                | Up::WeaponSound { .. }
                | Up::ClientOptions { .. } => {
                    if first_of_id {
                        // DC-DEFAULT: DSV-9 (staged proposal) shell ids without a server answer are logged once
                        self.log(format!(
                            "client {id}: shell message {shell_id:#04x} ({} bits) has no server answer",
                            body.bit_len()
                        ));
                    }
                }
                Up::Developer => {
                    if first_of_id {
                        // DC-DEFAULT: DC-C2S-1 (proposed) developer and console messages are dropped
                        self.log(format!(
                            "client {id}: developer message {shell_id:#04x} dropped"
                        ));
                    }
                }
                Up::Perf(up::UpPerf::Console { .. }) => {}
                Up::LeaveMatch => {
                    // SPEC: GAME_MESSAGES_2009_C2S.md §5.7 (remove the player at once, do not wait for the timeout)
                    // DC-DEFAULT: DSV-51 (proposed) the link closes with code 5 at the client's `C2S_BYE` (at most
                    // EXIT_BYE_WAIT_MS later), not with the plain bye at once
                    self.log(format!("client {id}: left the match"));
                    if let Some(c) = self.clients.get_mut(peer) {
                        c.leaving_since.get_or_insert(now);
                    }
                    self.report_leave(peer);
                    self.remove_from_world(now, peer);
                }
                _ => {}
            }
            if in_game && crate::visibility::is_audible(&msg) {
                // DC-DEFAULT: VIS-10 (proposed) a sound shows its source before the (unculled) relay
                let actors = self.vis_actors();
                let shown = self.vis.on_event(&actors, pid, now);
                self.vis_send(shown);
            }
            // DC-DEFAULT: DSV-170 (proposed) the use key pressed on a dropped weapon's object asks to take it
            if let (true, Up::Use { target, phase: 0 }) = (in_game, &msg) {
                let key = self
                    .drops
                    .iter()
                    .find(|(_, o)| **o == *target)
                    .map(|(k, _)| *k);
                if let Some(key) = key {
                    self.game.drop_use(pid, key, now);
                }
            }
            // DC-DEFAULT: DSV-270 (proposed) the use key of a living player toggles the lever / button door in reach
            if let (true, Up::Use { target, phase: 0 }) = (in_game, &msg) {
                let at = self
                    .game
                    .players()
                    .get(&pid)
                    .filter(|p| p.life == crate::game::Life::Alive)
                    .map(|p| p.pos);
                let is_drop = self.drops.values().any(|o| *o == *target);
                if let (Some(at), false) = (at, is_drop) {
                    if crate::doors::use_key(&mut self.doors, &mut self.turns, at, now) {
                        self.log(format!("client {id}: use key on a door"));
                    }
                    // DSV-560: a locked door's locked command runs as well
                    self.run_door_statements(Vec::new(), now);
                }
            }
            if in_game {
                self.game.on_up(pid, msg, now);
            }
        }
        self.apply_game(now);
    }

    /// Reference order, the world's first `UP_WORLD_READY` (from `ready`): shell 0x90 [0] and 0x88 [3] to every other
    /// link that has not had them, also to links still loading the world. The ready player gets the pair from the
    /// game's after-ready set. So a player who loads later already holds both when its spawn set and
    /// `GS_MATCH_BEGIN` arrive, as on the reference stack's links; before this, our pair came only after the player's own
    /// 0x87 (rust7 c5 t 114.953, after the 0x45 at 114.859), so the 2009 client took that 0x45 with its
    /// `client_shell`+0x1a09 flag still set (the flag that zeroes the game frame time, cshell+0x1aa810).
    // OBSERVED: captures 20261025_065016_tdm6 (c6 t 167.406 after c5's 0x87 at 167.359; c6's own
    // spawn set and 0x45 at 173.437), 20261025_002236_tdm4 (c6 57.656 / 62.812), 20261025_001409_tdm2 (c6 58.937 /
    // 63.594), 20261025_000014_tdm1, 20261025_012331_botduel3 (REFERENCE, accepted by the real client)
    // EVIDENCE: cshell+0x1a58d0 (0x90 writes `client_shell`+0x1a09), cshell+0x1a58f0 (0x88 writes +0x1a0c)
    // SPEC: UDP_REFERENCE_FINDINGS_botclient.md O6 ("sends both again to all clients after its world starts")
    // DC-DEFAULT: DSV-97 (proposed)
    fn start_world(&mut self, ready: &A) {
        if self.world_started || self.cfg.game.start_order != StartOrder::Reference {
            return;
        }
        self.world_started = true;
        let waiting: Vec<(A, u8, bool)> = self
            .clients
            .iter()
            .filter(|(p, c)| *p != ready && !c.switch_ready_early)
            .map(|(p, c)| (p.clone(), c.pid(), c.object_id.is_some()))
            .collect();
        for (peer, pid, placed) in waiting {
            self.send(&peer, crate::gs::shell_flag_90(false));
            self.send(
                &peer,
                msg::s2c_switch_state(crate::game::REFERENCE_SWITCH_STATE_READY),
            );
            if let Some(c) = self.clients.get_mut(&peer) {
                c.switch_ready_early = true;
            }
            if placed {
                self.game.mark_switch_sent(pid);
            }
        }
    }

    fn report_leave(&mut self, peer: &A) {
        if let Some(c) = self.clients.get_mut(peer) {
            if c.player.is_some() && !c.left_reported {
                c.left_reported = true;
                let id = c.client_id;
                self.pipe(pipe::leave_line(id));
            }
        }
    }

    /// The player leaves the world: the game drops it, the other clients get the removal of its objects, the ids
    /// become free. The link itself may stay open (a leave waits for the client's bye, DSV-51).
    fn remove_from_world(&mut self, now: Millis, peer: &A) {
        let Some((pid, obj, wobj)) = self
            .clients
            .get_mut(peer)
            .and_then(|c| Some((c.pid(), c.object_id.take()?, c.weapon_obj.take())))
        else {
            return;
        };
        self.vis.forget(pid);
        self.game.remove_player(pid, false);
        self.apply_game(now);
        // SPEC: UDP_SESSION_2009_B.md §5.1 (removal record; ids reusable once it is queued)
        let others: Vec<A> = self
            .clients
            .iter()
            .filter(|(p, x)| *p != peer && x.phase == Phase::InWorld)
            .map(|(p, _)| p.clone())
            .collect();
        let gone: Vec<u16> = std::iter::once(obj).chain(wobj).collect();
        for p in others {
            if let Some(g) = stream::removal_group(&gone) {
                self.send(&p, g);
            }
        }
        // DC-DEFAULT: CD-66 (approved) an id is reused only after its removal record was queued
        for g in gone {
            self.objects_used.remove(&g);
        }
    }

    /// The first cause the server gives for closing a link.
    fn mark_leave(&mut self, peer: &A, how: pipe::LeaveHow) {
        if let Some(c) = self.clients.get_mut(peer) {
            c.leave_how.get_or_insert(how);
        }
    }

    /// `@linksum` of an identified player, once per link (MON-1); `fallback` when no cause was marked.
    // DC-DEFAULT: MON-1 (proposed)
    fn report_linksum(&mut self, now: Millis, peer: &A, fallback: pipe::LeaveHow, code: u32) {
        let Some(c) = self.clients.get_mut(peer) else {
            return;
        };
        let Some(uid) = c.player.as_ref().map(|e| e.uid) else {
            return;
        };
        if c.sum_reported {
            return;
        }
        c.sum_reported = true;
        let how = c.leave_how.unwrap_or(fallback);
        let line = pipe::linksum_line(c.client_id, uid, how, code, &c.watch.totals(now));
        self.pipe(line);
    }

    fn on_link_closed(&mut self, now: Millis, peer: A, code: u32, cause: CloseCause) {
        self.report_leave(&peer);
        let fallback = match cause {
            CloseCause::PeerBye => pipe::LeaveHow::Bye,
            CloseCause::IdleTimeout => pipe::LeaveHow::Timeout,
            CloseCause::StaleRejoin => pipe::LeaveHow::Rejoin,
            CloseCause::Local => pipe::LeaveHow::Closed,
        };
        self.report_linksum(now, &peer, fallback, code);
        self.remove_from_world(now, &peer);
        if let Some(c) = self.clients.remove(&peer) {
            self.vis.forget(c.pid());
            self.log(format!(
                "client {} gone ({:?}, code {code}{}) after {} player updates",
                c.client_id,
                cause,
                c.leave_how.map(|h| format!(", {}", h.token())).unwrap_or_default(),
                c.updates
            ));
            self.log(format!("client {}: {}", c.client_id, c.watch.summary(now)));
        }
        if self.clients.is_empty() {
            self.last_activity = now;
        }
    }

    // DC-DEFAULT: CD-61 (proposed) start-up deadline, bye code 3; DSV-51 (proposed) the leave wait
    fn check_deadlines(&mut self, now: Millis) {
        let gone: Vec<A> = self
            .clients
            .iter()
            .filter(|(_, c)| {
                c.leaving_since
                    .is_some_and(|t| since(now, t) >= EXIT_BYE_WAIT_MS)
            })
            .map(|(p, _)| p.clone())
            .collect();
        for p in gone {
            self.log(format!(
                "{p}: no bye within {EXIT_BYE_WAIT_MS} ms after the leave, closing (bye code {EXIT_BYE_CODE})"
            ));
            if let Some(c) = self.clients.get_mut(&p) {
                c.leaving_since = None;
            }
            self.mark_leave(&p, pipe::LeaveHow::NoBye);
            self.host.disconnect(now, &p, Bye::Code(EXIT_BYE_CODE));
        }
        let late: Vec<A> = self
            .clients
            .iter()
            .filter(|(_, c)| {
                c.phase != Phase::InWorld && since(now, c.joined_at) > self.cfg.startup_deadline_ms
            })
            .map(|(p, _)| p.clone())
            .collect();
        for p in late {
            self.log(format!(
                "{p}: start-up deadline passed, dropping (bye code 3)"
            ));
            self.mark_leave(&p, pipe::LeaveHow::Startup);
            self.host
                .disconnect(now, &p, Bye::Code(dc_wire::udp::DROP_STALE));
        }
        // DC-DEFAULT: DSV-490 (proposed, project decision) a client silent for 20 s is gone: bye code 3 and the
        // normal leave path (the link's close event reports `@leave` and removes its objects)
        if let Some(limit) = self.cfg.silent_drop_ms {
            let silent: Vec<(A, u16, u32)> = self
                .clients
                .iter()
                .filter_map(|(p, c)| {
                    let l = self.host.link(p).filter(|l| l.is_open())?;
                    let quiet = since(now, l.last_recv());
                    (quiet > limit).then(|| (p.clone(), c.client_id, quiet))
                })
                .collect();
            for (p, id, quiet) in silent {
                self.log(format!(
                    "client {id}: nothing received for {quiet} ms, dropping (bye code 3)"
                ));
                self.mark_leave(&p, pipe::LeaveHow::Silent);
                self.host
                    .disconnect(now, &p, Bye::Code(dc_wire::udp::DROP_STALE));
            }
        }
    }

    /// One world tick: every in-world client gets one `S2C_OBJ_STREAM` with the other players' objects (position,
    /// velocity, changed rotation; the longest unsent first, within the budget) and the time entry last. Creates and
    /// removals go out on the reliable channel when they happen, before any message that names the object.
    // SPEC: UDP_SESSION_2009_B.md §4.3, §9.1, §3.1, §12.2, §12.3
    // DC-DEFAULT: CD-63 (approved) tick rate; CD-65 (approved) per-tick send policy
    fn tick(&mut self, now: Millis) {
        self.expire_corpses(now);
        self.expire_event_sounds(now);
        self.kill_volume_tick(now);
        self.door_tick(now);
        self.camshot_tick(now);
        let t = self.server_time(now);
        let pbox = self.pos_box();
        let peers: Vec<(A, u8)> = self
            .clients
            .iter()
            .filter(|(_, c)| c.phase == Phase::InWorld)
            .map(|(p, c)| (p.clone(), c.pid()))
            .collect();
        let budget = self.cfg.game.stream_budget_bits;
        // DC-DEFAULT: DSV-41 (amended, proposed) thrown objects are streamed in every tick from the create to the
        // removal (`-projectile-stream`, on by default; at rest without velocity); entry form of a player's
        // `pos48` + velocity, no rotation, no animation (appended after the players)
        // SPEC: UDP_SESSION_2009_B.md §3.5.4
        let flying: Vec<stream::StreamEntry> = self
            .game
            .flight_stream()
            .into_iter()
            .filter_map(|(key, pos, vel)| {
                Some(stream::StreamEntry {
                    obj: *self.projectiles.get(&key)?,
                    pos,
                    vel,
                    rot: None,
                    yaw8: None,
                    anim: None,
                })
            })
            .collect();
        // visibility culling (VIS-1, proposed): the hide / show records go out first (reliable), then each viewer's
        // stream without the enemies parked on its client
        let changes = self.vis_update(now, &peers);
        self.vis_send(changes);
        for (p, pid) in peers {
            if let Some(l) = self.host.link(&p) {
                let rtt = l.latency().rtt_mean();
                if rtt.is_finite() && rtt >= 0.0 {
                    self.game.set_link_rtt(pid, rtt.min(65_535.0) as u16);
                }
                let stats = l.stats();
                let newest = l.latency().newest();
                if let Some(c) = self.clients.get_mut(&p) {
                    if let Some(win) = c.watch.tick(now, stats, rtt, newest) {
                        let id = c.client_id;
                        let uid = c.player.as_ref().map_or(0, |e| e.uid);
                        self.log(format!("client {id}: {}", win.log_line()));
                        // DC-DEFAULT: DSV-411 (proposed) the same window to the lobby (`GET /net`, LG-185)
                        self.pipe(pipe::net_line(id, uid, &win));
                    }
                }
            }
            let mut entries = self.game.stream_for(pid, now);
            entries.retain(|e| !self.vis.parked(pid, e.obj));
            let players = entries.len();
            entries.extend(flying.iter().copied());
            let (m, n) = stream::obj_stream(&entries, t, &pbox, budget);
            self.game
                .mark_streamed(pid, &entries[..n.min(players)], now);
            self.host.send_unreliable(now, &p, m);
            if let Some(c) = self.clients.get_mut(&p) {
                c.ticks += 1;
                c.mark(Step::FirstTick, now);
            }
        }
    }
}

impl<A: Ord + Clone + Display> Server<A> {
    // ------------------------------------------------------------------------------------------- visibility

    fn vis_actors(&self) -> Vec<crate::visibility::Actor> {
        crate::visibility::actors(
            &self.game,
            self.cfg.player_pos_form == PosForm::FullPrecision,
        )
    }

    fn vis_tick_ms(&self) -> u32 {
        (1000 / self.cfg.tick_hz.max(1)).max(1)
    }

    fn vis_ffa(&self) -> bool {
        self.game.rules().mode_class == crate::visibility::MODE_CLASS_FFA
    }

    /// The tick's verdicts for the in-world viewers (and the periodic statistics line).
    fn vis_update(&mut self, now: Millis, peers: &[(A, u8)]) -> Vec<crate::visibility::Change> {
        if !self.vis.enabled() {
            return Vec::new();
        }
        let actors = self.vis_actors();
        let viewers: Vec<u8> = peers.iter().map(|p| p.1).collect();
        let (ffa, tick_ms) = (self.vis_ffa(), self.vis_tick_ms());
        let changes = self.vis.update(&actors, &viewers, ffa, tick_ms, now);
        if let Some(line) = self.vis.take_report(now) {
            self.log(line);
        }
        changes
    }

    fn vis_spawn_parked(
        &mut self,
        actors: &[crate::visibility::Actor],
        viewer: u8,
        enemy: u8,
        now: Millis,
    ) -> bool {
        let (ffa, tick_ms) = (self.vis_ffa(), self.vis_tick_ms());
        self.vis
            .spawn_parked(actors, viewer, enemy, ffa, tick_ms, now)
    }

    /// The park / show records, one object group per viewer, reliable.
    // DC-DEFAULT: VIS-9 (proposed)
    fn vis_send(&mut self, changes: Vec<crate::visibility::Change>) {
        if changes.is_empty() {
            return;
        }
        let pbox = self.pos_box();
        let mut by: BTreeMap<u8, Vec<BitWriter>> = BTreeMap::new();
        for c in &changes {
            by.entry(c.viewer)
                .or_default()
                .push(crate::visibility::snap_record(c, &pbox));
        }
        for (pid, recs) in by {
            if let (Some(peer), Some(g)) = (self.peer_of(pid), objects::obj_group(&recs)) {
                self.send(&peer, g);
            }
        }
    }
}

impl<A: Ord + Clone + Display> Server<A> {
    /// Kills the living players that entered a created kill volume: a player whose position lies inside one is armed
    /// with that volume's pending time and dies when it has passed, also when it left the volume meanwhile (a fall
    /// through the volume goes on below it). The arm is dropped when the player is no longer alive.
    // DC-DEFAULT: DSV-52 (proposed)
    fn kill_volume_tick(&mut self, now: Millis) {
        if !self.cfg.kill_volumes || self.kill_active.is_empty() {
            self.kill_inside.clear();
            return;
        }
        let alive: Vec<(u8, [f32; 3])> = self
            .game
            .players()
            .values()
            .filter(|p| p.life == crate::game::Life::Alive && p.visible)
            .map(|p| (p.pid, p.pos))
            .collect();
        self.kill_inside
            .retain(|k, _| alive.iter().any(|a| a.0 == k.0));
        let mut dies = Vec::new();
        for (pid, pos) in alive {
            if !self.kill_inside.keys().any(|k| k.0 == pid) {
                let hit = self.kill_active.iter().copied().find(|&i| {
                    self.world
                        .kill_shapes
                        .get(i)
                        .is_some_and(|s| s.contains(pos))
                });
                if let Some(i) = hit {
                    self.kill_inside.insert((pid, i), now);
                    self.log(format!("client {pid}: entered kill volume {i}"));
                }
            }
            let due = self
                .kill_inside
                .iter()
                .find(|(k, _)| k.0 == pid)
                .map(|(k, t)| {
                    let wait = self.world.kill_shapes.get(k.1).map_or(0, |s| s.pending_ms);
                    since(now, *t) >= wait
                });
            if due == Some(true) {
                dies.push(pid);
            }
        }
        for pid in dies {
            self.kill_inside.retain(|k, _| k.0 != pid);
            self.game.volume_death(pid, now);
        }
        self.apply_game(now);
    }

    /// One step of every door: opened by a living player near it, closed a while after the last one left; each door
    /// that moved goes to every in-world client as a change record (one object group per tick), and the planned
    /// world object keeps the position for clients that join later.
    // DC-DEFAULT: DSV-46 (proposed)
    fn door_tick(&mut self, now: Millis) {
        if self.doors.is_empty() && self.turns.is_empty() {
            return;
        }
        let dt = self.doors_at.map_or(0, |t| since(now, t)).min(1000);
        self.doors_at = Some(now);
        let round = self.game.round_no();
        if round != self.door_round {
            if self.door_round != 0 {
                self.door_script_reset(now);
            }
            self.door_round = round;
        }
        let near: Vec<[f32; 3]> = self
            .game
            .players()
            .values()
            .filter(|p| p.life == crate::game::Life::Alive && p.visible)
            .map(|p| p.pos)
            .collect();
        let mut recs = Vec::new();
        let mut moved = Vec::new();
        for d in &mut self.doors {
            if let Some((pos, vel)) = d.step(now, dt, &near) {
                recs.push(crate::doors::door_move_record(d.obj, pos, vel));
                moved.push((d.obj, pos));
            }
        }
        // DC-DEFAULT: DOOR-4, DOOR-5 (proposed, w1-doors)
        for t in &mut self.turns {
            if let Some(r) = t.step(now, dt, &near) {
                recs.push(r);
            }
        }
        // DSV-270: the commands of the doors that changed state and the delayed statements due now
        // DC-DEFAULT: DSV-563 (proposed) a co-op room runs the triggers its living players walk into (their door,
        // trigger and model statements: the mission's door unlocks); other rooms do not
        let touched = if self.game.is_coop() {
            self.door_triggers.touch(&near)
        } else {
            Vec::new()
        };
        self.run_door_statements(touched, now);
        // DSV-340: a changed activate entry goes to every in-world client as the door's object notes
        self.door_notes();
        // DSV-302: the sounds of the moves that started, at the door plus its sound offset
        let mut played = false;
        for d in &mut self.doors {
            for f in std::mem::take(&mut d.sounds_due) {
                let p = d.pos();
                let o = d.info.sounds.offset;
                self.game
                    .object_sound(f, [p[0] + o[0], p[1] + o[1], p[2] + o[2]], d.info.sounds.radius);
                played = true;
            }
        }
        for t in &mut self.turns {
            for f in std::mem::take(&mut t.sounds_due) {
                let p = t.pos();
                let o = t.info.sounds.offset;
                self.game
                    .object_sound(f, [p[0] + o[0], p[1] + o[1], p[2] + o[2]], t.info.sounds.radius);
                played = true;
            }
        }
        if played {
            self.apply_game(now);
        }
        if recs.is_empty() {
            return;
        }
        if let Some(objs) = self.world_objs.as_mut() {
            for (obj, pos) in &moved {
                for o in objs.iter_mut() {
                    if let crate::worldobj::WorldObject::Brush { obj: id, brush } = o {
                        if id == obj {
                            brush.pos = *pos;
                        }
                    }
                }
            }
        }
        if let Some(g) = objects::obj_group(&recs) {
            for v in self.world_viewers() {
                self.send(&v, g.clone());
            }
        }
    }

    /// Runs the doors' fired commands, the delayed statements due now and the texts `texts` (DSV-270, DSV-562), then
    /// the models' visible / solid switches they asked for.
    fn run_door_statements(&mut self, texts: Vec<Vec<u8>>, now: Millis) {
        let over = self.door_triggers.over_budget;
        let models = crate::doors::run_world(
            &mut self.doors,
            &mut self.turns,
            &mut self.door_triggers,
            &mut self.door_delays,
            texts,
            now,
        );
        if over == 0 && self.door_triggers.over_budget > 0 {
            self.log(format!(
                "world statements: a run passed {} statements and was cut (logged once)",
                crate::doors::TRIGGER_STATEMENTS
            ));
        }
        if !models.is_empty() {
            self.on_model_switches(&models);
        }
    }

    /// A new round (round modes reuse the server, GMD-24): the triggers as at the start (no fires but the start
    /// scripts', the file's locks), every door's and switch's lock as in the file, nothing delayed, the models the
    /// statements changed back as planned (the ones they created removed); then each switch that is on runs its on
    /// command again so its models follow it. Doors and switches keep their place.
    // DC-DEFAULT: DSV-564 (proposed)
    fn door_script_reset(&mut self, now: Millis) {
        self.door_triggers.reset();
        self.door_triggers.seed(&self.door_start_fired);
        self.door_delays.clear();
        for d in &mut self.doors {
            d.locked = d.info.locked;
            d.fired.clear();
        }
        for t in &mut self.turns {
            t.locked = t.info.locked;
            t.fired.clear();
            if t.info.kind == crate::doors::TurnKind::Switch && t.want {
                t.fired.push(t.info.cmds.open.clone());
            }
        }
        let changed = std::mem::take(&mut self.model_start);
        let mut recs = Vec::new();
        let mut gone = Vec::new();
        if let Some(objs) = self.world_objs.as_mut() {
            objs.retain_mut(|o| {
                let crate::worldobj::WorldObject::Brush { obj, brush } = o else {
                    return true;
                };
                match changed.get(obj) {
                    Some(Some(orig)) => {
                        *brush = orig.clone();
                        recs.push(crate::destructibles::brush_flags_record(*obj, brush));
                        true
                    }
                    Some(None) => {
                        gone.push(*obj);
                        false
                    }
                    None => true,
                }
            });
        }
        for o in &gone {
            self.objects_used.remove(o);
        }
        if !gone.is_empty() {
            self.send_world_removal(&gone);
        }
        if !recs.is_empty() {
            self.send_world_group(&recs);
        }
        self.log(format!(
            "world statements: round reset ({} models back, {} removed)",
            recs.len(),
            gone.len()
        ));
        self.run_door_statements(Vec::new(), now);
    }

    /// Named models made visible / invisible, solid / non-solid by the doors' and triggers' statements: a created
    /// model gets its new flag words on every in-world client (a change record, as a breakable's death, DSV-117); a
    /// model the plan left out (none of visible, solid, ray-hit at the start, DSV-37) is created once it is visible or
    /// solid; the planned objects keep the state for clients that join later. A model made invisible loses its
    /// ray-hit flag, made visible again it gets the file's back (as the start scripts, DSV-37).
    // SPEC: UDP_SESSION_2009_B.md §4.2 item 3 (flag words of a change record), §3.3 (kind 2 create)
    // DC-DEFAULT: DSV-562 (proposed)
    fn on_model_switches(&mut self, switches: &[crate::doors::ModelSwitch]) {
        if !matches!(
            self.cfg.world_objects,
            crate::worldobj::WorldObjects::World | crate::worldobj::WorldObjects::All
        ) || self.world_objs.is_none()
        {
            return;
        }
        let pbox = self.pos_box();
        let mut recs = Vec::new();
        for sw in switches {
            // every model of the file with that name (names repeat in some worlds)
            let files: Vec<crate::world::BrushInfo> = self
                .world
                .brushes
                .iter()
                .filter(|b| b.name.eq_ignore_ascii_case(&sw.name))
                .filter(|b| b.class != crate::world::BrushClass::Sky)
                .cloned()
                .collect();
            if files.is_empty() {
                continue;
            }
            let set = |b: &mut crate::world::BrushInfo, file_rayhit: bool| {
                if let Some(v) = sw.visible {
                    b.visible = v;
                    b.rayhit = file_rayhit && v;
                }
                if let Some(v) = sw.solid {
                    b.solid = v;
                }
            };
            let rayhit = files.iter().any(|f| f.rayhit);
            let mut found = false;
            if let Some(objs) = self.world_objs.as_mut() {
                for o in objs.iter_mut() {
                    if let crate::worldobj::WorldObject::Brush { obj, brush } = o {
                        if !brush.name.eq_ignore_ascii_case(&sw.name) {
                            continue;
                        }
                        found = true;
                        // DC-DEFAULT: DSV-562 (proposed) a dead breakable stays dead: a statement does not show it
                        // again (it would come back without hit points); the round restore (RND-5) does
                        if self.broken_brushes.contains_key(obj) {
                            continue;
                        }
                        let before = brush.clone();
                        set(brush, rayhit);
                        if *brush != before {
                            self.model_start.entry(*obj).or_insert(Some(before));
                            recs.push(crate::destructibles::brush_flags_record(*obj, brush));
                        }
                    }
                }
            }
            if found {
                continue;
            }
            for file in files {
                // a door or switch the plan left out stays out (its object belongs to the door code); a breakable
                // is not created by a statement (it would have no hit points)
                if matches!(
                    file.class,
                    crate::world::BrushClass::Door
                        | crate::world::BrushClass::Switch
                        | crate::world::BrushClass::Breakable
                ) {
                    continue;
                }
                let mut b = file.clone();
                set(&mut b, file.rayhit);
                if !(b.visible || b.solid) {
                    continue;
                }
                let obj = self.free_object_id();
                recs.push(crate::worldobj::brush_record(obj, &b, &pbox));
                self.model_start.insert(obj, None);
                if let Some(objs) = self.world_objs.as_mut() {
                    objs.push(crate::worldobj::WorldObject::Brush { obj, brush: b });
                }
            }
        }
        self.log(format!(
            "world models switched: {} statements, {} records",
            switches.len(),
            recs.len()
        ));
        if !recs.is_empty() {
            self.send_world_group(&recs);
        }
    }

    /// The object notes of the doors whose activate entry changed since the clients got it (DSV-340): sub 1 for the
    /// disabled flag, sub 2 for the state, reliable, to every in-world client.
    // DC-DEFAULT: DSV-340 (proposed)
    fn door_notes(&mut self) {
        let mut notes = Vec::new();
        for (obj, (off, state)) in self.door_entries() {
            let was = self.door_marks.insert(obj, (off, state));
            let Some((was_off, was_state)) = was else {
                continue;
            };
            if was_off != off {
                notes.push(crate::gs::door_note(obj, crate::doors::NOTE_DISABLED, off as u8));
            }
            if was_state != state {
                notes.push(crate::gs::door_note(obj, crate::doors::NOTE_STATE, state));
            }
        }
        if notes.is_empty() {
            return;
        }
        for v in self.world_viewers() {
            for n in &notes {
                self.send(&v, n.clone());
            }
        }
    }

    /// The door object notes sent so far (tests): `(disabled, state)` per door object.
    pub fn door_marks(&self) -> &BTreeMap<u16, (bool, u8)> {
        &self.door_marks
    }

    /// The world-camera steps (DSV-112): the start triggers' statements with the round start, the end trigger's
    /// with the round result (the 0x46 moment), each step at its time; a changed camera goes to every in-world client
    /// as a change record (one object group per tick).
    // DC-DEFAULT: DSV-112 (proposed)
    fn camshot_tick(&mut self, now: Millis) {
        if self.camshot.cameras.is_empty() {
            return;
        }
        // DC-DEFAULT: RND-2 (staged) each round of a round mode runs the start and end triggers again
        self.camshot.track_round(self.game.round_no());
        let scripts = &self.world.scripts;
        match self.game.phase() {
            crate::game::RoundPhase::Running { .. } => {
                self.camshot.round_started(scripts, &scripts.cameras, now)
            }
            crate::game::RoundPhase::Over { at, .. } => {
                self.camshot.round_ended(scripts, &scripts.cameras, at)
            }
            _ => {}
        }
        let pbox = self.pos_box();
        let recs = self
            .camshot
            .due(now, &self.world.scripts.cameras.points, &pbox);
        if recs.is_empty() {
            return;
        }
        let live = self.camshot.cameras.iter().filter(|c| c.live).count();
        self.log(format!("world camera: {} changed, {live} live", recs.len()));
        if let Some(g) = objects::obj_group(&recs) {
            for v in self.world_viewers() {
                self.send(&v, g.clone());
            }
        }
    }

    /// The world cameras on the clients (tests).
    pub fn world_cameras(&self) -> &[crate::camshot::LiveCamera] {
        &self.camshot.cameras
    }

    /// The doors on the clients (tests).
    pub fn doors(&self) -> &[crate::doors::Door] {
        &self.doors
    }

    /// The rotating doors and switches on the clients (tests).
    pub fn turns(&self) -> &[crate::doors::TurnDoor] {
        &self.turns
    }

    /// The kill volumes the session acts on (indexes into the world's kill shapes; tests).
    pub fn active_kill_volumes(&self) -> &[usize] {
        &self.kill_active
    }
}

fn underscore(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .collect()
}
