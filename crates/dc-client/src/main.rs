//! `dc-client`: walk a world of the user's ROM in first person, alone or in a match of a DeCombat server.
//!
//! ```text
//! dc-client --rom <CA_ROM> --map <name> [--spawn n] [--tick 60] [--headless-frames N --out dir]
//! dc-client --lobby <host:port> --name <nick> --room <n|new|any> [--walk-test] [--headless-frames N | --duration S]
//! dc-client --udp <host:port> --name <nick> [--world-file <path>] [--walk-test]
//! dc-client --rom <CA_ROM> --list
//! ```
//! Images written in headless mode are derived from the ROM: local files for the user, never committed.

use dc_botclient::hostroom::GoalChoice;
use dc_botclient::run::RoomChoice;
use dc_client::headless::{default_out_dir, ensure_dir};
use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetHandle, NetOptions, NetStats, NetWorld,
    RomWorld, ShootTest, ViewAngles, ViewSettings, WalkTest, WorldFile, WorldSource,
};
use dc_render::AdapterChoice;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

/// Counts heap allocations for the frame profiler (cargo feature `prof-alloc` only; the default build keeps the
/// system allocator untouched): [`dc_render::perf::CountingAlloc`], per thread and in total.
#[cfg(feature = "prof-alloc")]
#[global_allocator]
static ALLOC: dc_render::perf::CountingAlloc = dc_render::perf::CountingAlloc;

const USAGE: &str = "usage: dc-client [--rom DIR] --map NAME [--spawn N] [--tick HZ] [--size WxH] [--windowed|--fullscreen] [--monitor N] [--fov DEG]
                 [--headless-frames N [--out DIR] [--frame-ms MS] [--save-every K] [--profile JSON]]
                 [--window-secs S] [--script] [--vsync|--no-vsync] [--fps-cap-match N] [--fps-cap-menu N] [--sensitivity S] [--invert-y] [--mouse-clamp N]
                 [--adapter auto|dx12|vulkan|metal|fallback] [--no-overlay]
                 [--stress bsp|characters|fx|hud|cold|match [--stress-view SPAWN,YAW_DEG,PITCH_DEG]
                  [--stress-remotes N]] [--stress-search]
       dc-client (--lobby HOST:PORT | --udp HOST:PORT) --name NICK [--room n|new|any] [--walk-test]
                 [--user ID] [--start-after N] [--title T] [--mode N|NAME] [--pilot-entry ID] [--pilot-map NAME|N] [--max-players N] [--password P]
                 [--login-password P] [--server-fingerprint SHA256] [--identity HOST:PORT|off]
                 [--identity-fingerprint SHA256] [--community URL|off]
                 [--channel ID] [--goal n|min] [--world-file PATH] [--net-log JSONL] [--capture JSONL]
                 [--sound-log JSONL] [--no-sound] [--audio device|null] [--volume N] [--music-volume N]
                 [--match-wait S] [--load-timeout S] [--leave-after S] [--duration S] [--no-move-notices]
                 [--shoot-test] [--draw-voice] [--solid-bodies on|off]
                 [--slots w,w,..] [--debug-boxes] [--net-overlay[=2]] [--no-hud] [--debug-overlay] [--show-board]
                 [--show-board-at A,B] [--result-hold S] [--state-shots]
                 [--test-correction-after N] [window / headless options as above]
       scenario options (any mode): [--steps FILE.jsonl [--steps-strict]] [--events FILE.jsonl] [--shot-dir DIR]
                 [--no-activate]
       hang gate (any mode): [--heartbeat FILE.jsonl]
       dc-client [--rom DIR] --list
       dc-client --world-file PATH.dat [--rom DIR] [--mod-dir DIR] [--spawn N] [--headless-frames N --out DIR]
                 (the standalone preview of a world file: ROM textures by name when a ROM is at hand,
                 @mod:<any id>/assets/... textures from --mod-dir, solid colours for the rest)
  Without --headless-frames a window opens on the primary monitor (feature `window`; --monitor N picks another,
  0 = primary) in one of two looks: full screen (a borderless window filling the monitor; never exclusive, never a
  display-mode change) or windowed (a bordered centred window at the resolution setting, 1280x720 without one).
  Alt+Enter switches the look; the look and the resolution (options, Graphic tab) are saved to the profile and
  used at the next start; a first start without saved values opens full screen and saves the monitor's size.
  --windowed / --fullscreen pick the look for this run only (nothing saved); --size WxH alone opens a smaller
  centred borderless window (test tools), with --windowed it is the windowed size for this run.
  --lobby is the auth address (the lobby address comes from its server list); --room new creates a room and starts
  it once --start-after members (default 2) are in and ready; any joins the first listed room; a number joins
  that room. --udp joins a game server directly (it must expect the nick). The world the server announces is
  loaded from the ROM (or --world-file). --walk-test walks a repeating square instead of reading the keyboard
  (headless network runs always use it); --test-correction-after N shifts the reported position once after N
  updates so a validating server corrects it.
  The login and the web pages use TLS (loopback servers without it stay plain): --server-fingerprint SHA256 pins the
  lobby's certificate (the fingerprint its log prints; also DC_SERVER_FINGERPRINT); without a pin a CA-issued
  certificate is accepted, else the first one seen is remembered in the profile (known_servers.txt) and a changed one
  is refused until the new fingerprint is given.
  Identity (--menu lobby): the login screen's name + password go to the identity authority first (POST
  /identity/token on its TLS web port; default: the lobby's host at the auth port + 8080, pinned like the lobby);
  the lobby gets only the token (`tok:<token>`, kept in the profile's identity_token.json until it expires, so a later
  start logs in without the password). --identity HOST:PORT names another authority (--identity-fingerprint pins
  it); --identity off logs in with the password at the lobby itself (a local account).
  --community URL names the community directory file (static JSON, CLI-1615); off disables the list.
  Keys: W/S/A/D move, Space jump, Ctrl or C crouch, Left Shift sprint, Left Alt walk, Caps Lock run lock,
  mouse look; Escape releases the cursor, Escape again (or the close request) quits; a click re-captures.
  --headless-frames runs the scripted sequence (forward 2 s, turn 90 degrees, jump, stop) for N frames of
  --frame-ms each (default 1000/60) and writes one PNG per frame (or every K-th) into --out.
  Network runs: --match-wait S bounds the room fill (host) and the wait for the match address (default 60);
  --load-timeout S bounds the whole join until the server announces its world, counted from the login (default 90;
  a host waiting for players needs more than its --match-wait);
  --goal sets the room's kill goal as host (min = the ROM mode row's smallest goal); --leave-after S
  leaves the running match S seconds after entering the world (0xbd + lobby 0x394 {0}, then C2S_BYE);
  --duration S runs headless for at most S seconds (instead of --headless-frames) and leaves at the end; a
  headless network run ends one second after the server ended the session. After a death the client asks for
  its respawn itself (0x89 after the 2009 wait of the room's mode, 7 s in TDM; the 2009 client has no respawn key)
  and shows the death camera until the server places it again. After the result screen (0x3f) the client stays
  --result-hold S seconds (default 1) and leaves it the 2009 way (lobby 0x306 {0xb} and the plain transport bye).
  --state-shots (headless) saves one PNG at the first damage, death, respawn and result screen into --out.
  --fov DEG: horizontal field at a 4:3 window (default: the 2009 vertical field of 75 degrees, which widens with the
  window's aspect: 91 degrees at 4:3, 107.5 at 16:9); the vertical field is what stays fixed, whatever the aspect.
  --mouse-clamp N: largest mouse motion per frame and axis in raw counts (default 4096, 0 = off).
  Weapons: left mouse button fires (cursor captured), 1-4 choose weapon slots, the wheel steps through them, R
  reloads. --shoot-test aims at the nearest remote player and fires in bursts (with --walk-test it also walks);
  --slots gives a direct --udp join weapon slots without server grants (weapon numbers, slot order);
  --debug-boxes draws the team-coloured boxes of remote players next to their models.
  --net-overlay starts every match with the network overlay on (=2: with the player rows; `/network` in the chat
  line toggles it, DC_NET_OVERLAY=1|2 does the same as the flag).
  --draw-voice: a drawn round plays a draw voice (the 2009 client plays none).
  --solid-bodies on|off: other players' bodies block the own body (default on; same as DC_SOLID_BODIES).
  HUD: the 2009 in-match HUD from the ROM's UI data (our text layout without a ROM); Tab holds the scoreboard
  (--show-board shows it in headless runs, --show-board-at A,B from A to B seconds of a window run); --no-hud turns
  it off; with the HUD on, the debug text panel is drawn
  only with --debug-overlay.
  Sound: a window run plays through the default output device (--audio null: mixed but not played; headless runs
  always use the null backend); --volume / --music-volume are the 2009 options page's effects / music volume
  0..100 (default 100); --no-sound turns the sound director off. A status line of the output (callbacks, period,
  latency, late callbacks = probable underruns, lock waits, peak, clipping) is printed every 10 s and at the end.
  --rom defaults to CA_ROM.
  Stress scenarios (docs/RENDER_PERF_GATES.md G1, with --profile JSON): --stress NAME replaces the scripted walk with
  a frame-indexed made-up scene on --map (bsp: the view with the most world triangles, nothing else; characters:
  --stress-remotes N (default 16) skinned characters in view; fx: they fire, blasts and placed effects; hud: every
  HUD element, the scoreboard open; cold: all of it from the first frame; match: moderate rates while walking), in
  headless runs and in the window. --stress-search prints the visibility search of the map (every spawn, 16 yaws,
  3 pitches, ranked by world triangles) as one JSON line; bsp without --stress-view searches first.
  --menu skip|lobby: skip (default) joins as the options above say; lobby shows the 2009 front end first (login with
  the --name / --user ID, server, channel, lobby, room) and returns to the waiting room after every match; with
  --headless-frames / --duration a scripted player walks the screens (--room new|any|n) and saves one PNG per
  screen into --out; --menu-pilot lets the same scripted player click through the window's screens (hands-free);
  --pilot-keep-result leaves each match's result screen to the player (Exit, or the 60 s timer).
  --mode picks the room's mode for --room new: a ROM mode row number, the row's key or a class name (TDM, FFA, ELIM,
  COOP, TDM_CLAN, BOMB, FLAG, BOUNTY, INFECT, ...; the first row of that class); the scripted player picks it in the
  creation box's mode list (without --mode the box's start row). --pilot-map picks the map in the waiting room's map
  list before the start: a ROM map row number, its key or its display name (an unknown name lists the ROM's maps).
  --pilot-entry picks server entry ID of the lobby's list on the server screen (with --menu lobby; a featured entry's
  set then loads as for a player); an entry the list lacks or that refuses the pick ends the run with the reason.
  Scenarios (gate T2): --steps FILE.jsonl runs a step file inside the process (window and headless; the scripted
  player then stays off): clicks, keys, look, fire, shots, expectations (crate::steps); --steps-strict fails a step
  whose widget has no rectangle or misses the hit-test instead of sending its command. --events FILE.jsonl writes the
  event stream (screens, clicks with handled / handler / sound, hovers, sounds, music, damage, kills, deaths,
  respawns, rounds, view model, corpses). --shot-dir DIR is where shot / burst steps save PNGs (the window reads its
  swapchain image back before presenting). --no-activate opens the window without focus, below other windows; with
  --steps the window never reads the user's mouse or keys and never grabs the cursor.
  Frame watchdog (window; headless with DC_WATCHDOG=1): DC_WATCHDOG=0 turns it off. No progress for the phase's
  budget (2 s in the menu and in play, 15 s loading, 10 s teardown) writes a stall dir stall_<pid>_<n> (stall.json,
  threads.txt with every thread's breadcrumbs and stack, events_tail.jsonl, dump1.dmp) next to the --sound-log (or
  into DC_WATCHDOG_DIR); 6 s later a second dump, then the process aborts (DC_WATCHDOG_ABORT=0 keeps it). A panic
  writes panic_<pid>.txt, an unhandled exception crash_<pid>/ there. DC_FAULT=deadlock_test [DC_FAULT_AT=s]
  deadlocks the frame thread on purpose. --heartbeat FILE writes one JSON line per second (frame, phase, handles,
  threads, private bytes, voices_looping, corpses) for an external watchdog.";

struct Args {
    stress: Option<String>,
    stress_view: Option<(usize, f32, f32)>,
    /// `--camera X,Y,Z,YAW,PITCH` (degrees, positive pitch looks down): where a headless map run draws from.
    camera: Option<([f32; 3], f32, f32)>,
    /// `--clean-frame`: a headless picture of the map only, no HUD and no debug panel.
    clean_frame: bool,
    stress_remotes: usize,
    stress_search: bool,
    rom: Option<PathBuf>,
    map: Option<String>,
    spawn: usize,
    tick: u32,
    size: Option<(u32, u32)>,
    /// `--windowed` (Some(true)) / `--fullscreen` (Some(false)): the start look for this run (CLI-1282).
    windowed: Option<bool>,
    monitor: usize,
    fov: f32,
    headless_frames: Option<u32>,
    out: Option<PathBuf>,
    frame_ms: f64,
    save_every: u32,
    window_secs: Option<f32>,
    script: bool,
    vsync: bool,
    no_vsync: bool,
    sensitivity: Option<f32>,
    invert_y: bool,
    adapter: AdapterChoice,
    /// `--adapter` was given: it wins over the saved Renderer choice (CLI-1852) and is exclusive.
    adapter_given: bool,
    overlay: bool,
    list: bool,
    lobby: Option<SocketAddr>,
    udp: Option<SocketAddr>,
    name: Option<String>,
    user: Option<String>,
    room: String,
    start_after: usize,
    title: Option<String>,
    mode: Option<String>,
    pilot_map: Option<String>,
    /// MAP-88: `--pilot-entry`: the server entry the scripted player enters.
    pilot_entry: Option<u32>,
    max_players: i32,
    password: String,
    /// `--login-password`: the login screen's password (CLI-810; never logged).
    login_password: String,
    /// CLI-1610: `--identity HOST:PORT|off` and `--identity-fingerprint SHA256` (None: the default authority).
    identity: Option<String>,
    identity_fingerprint: Option<String>,
    channel: Option<u32>,
    walk_test: bool,
    world_file: Option<PathBuf>,
    /// MAP-31 / MAP-33: `--mod-dir DIR`, a mod folder: on a network run the server's one mod (its manifest is
    /// read), on a `--world-file` preview the folder every `@mod:<id>` names.
    mod_dir: Option<PathBuf>,
    net_log: Option<PathBuf>,
    sound_log: Option<PathBuf>,
    sound: bool,
    audio_out: dc_client::sound::AudioOut,
    volume: u16,
    music_volume: u16,
    capture: Option<PathBuf>,
    match_wait: f64,
    load_timeout: Option<f64>,
    glitch_after: Option<u64>,
    mouse_clamp: Option<f64>,
    goal: Option<GoalChoice>,
    leave_after: Option<f64>,
    duration: Option<f64>,
    move_notices: bool,
    reality: bool,
    draw_voice: bool,
    shoot_test: bool,
    /// fix-hang: the scripted menu player clicks through the window menu (`--menu lobby`, hands-free runs).
    menu_pilot: bool,
    /// CLI-611: `--menu-pilot` leaves the result screen to the player.
    pilot_keep_result: bool,
    slots: Vec<i32>,
    debug_boxes: bool,
    hud: bool,
    debug_overlay: bool,
    show_board: bool,
    board_at: Option<(f32, f32)>,
    result_hold: Option<f64>,
    state_shots: bool,
    /// `--menu skip|lobby`.
    menu: String,
    /// Gate T2: `--steps`, `--steps-strict`, `--events`, `--shot-dir`, `--no-activate`.
    steps: Option<PathBuf>,
    steps_strict: bool,
    events: Option<PathBuf>,
    heartbeat: Option<PathBuf>,
    shot_dir: Option<PathBuf>,
    no_activate: bool,
}

fn addr(s: &str, flag: &str) -> Result<SocketAddr, String> {
    s.to_socket_addrs()
        .map_err(|e| format!("{flag} {s}: {e}"))?
        .next()
        .ok_or_else(|| format!("{flag} {s}: no address"))
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        rom: None,
        map: None,
        spawn: 0,
        tick: dc_sim::DEFAULT_TICK_HZ,
        size: None,
        windowed: None,
        monitor: 0,
        fov: 0.0,
        headless_frames: None,
        out: None,
        frame_ms: 1000.0 / 60.0,
        save_every: 1,
        window_secs: None,
        script: false,
        vsync: false,
        no_vsync: false,
        sensitivity: None,
        invert_y: false,
        adapter: AdapterChoice::Auto,
        adapter_given: false,
        overlay: true,
        list: false,
        lobby: None,
        udp: None,
        name: None,
        user: None,
        room: "any".into(),
        start_after: 2,
        title: None,
        mode: None,
        pilot_map: None,
        pilot_entry: None,
        max_players: 8,
        password: String::new(),
        login_password: String::new(),
        identity: None,
        identity_fingerprint: None,
        channel: None,
        walk_test: false,
        world_file: None,
        mod_dir: None,
        net_log: None,
        sound_log: None,
        sound: true,
        audio_out: dc_client::sound::AudioOut::Device,
        volume: 100,
        music_volume: 100,
        capture: None,
        match_wait: 60.0,
        load_timeout: None,
        glitch_after: None,
        mouse_clamp: None,
        goal: None,
        leave_after: None,
        duration: None,
        move_notices: true,
        reality: true,
        draw_voice: false,
        shoot_test: false,
        menu_pilot: false,
        pilot_keep_result: false,
        slots: Vec::new(),
        debug_boxes: false,
        hud: true,
        debug_overlay: false,
        show_board: false,
        board_at: None,
        result_hold: None,
        state_shots: false,
        menu: "skip".into(),
        steps: None,
        steps_strict: false,
        events: None,
        heartbeat: None,
        shot_dir: None,
        no_activate: false,
        stress: None,
        stress_view: None,
        camera: None,
        clean_frame: false,
        stress_remotes: dc_client::stress::DEFAULT_REMOTES,
        stress_search: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) if k.starts_with("--") => (k.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut val = |name: &str| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| it.next())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        let num = |s: String, name: &str| -> Result<f64, String> {
            s.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("{name}: expected a number, got {s:?}"))
        };
        match key.as_str() {
            "--rom" => a.rom = Some(PathBuf::from(val("--rom")?)),
            "--map" => a.map = Some(val("--map")?),
            "--spawn" => a.spawn = val("--spawn")?.parse().map_err(|_| "--spawn: integer")?,
            "--tick" => {
                a.tick = val("--tick")?.parse().map_err(|_| "--tick: integer")?;
                if !(1..=1000).contains(&a.tick) {
                    return Err("--tick: 1..1000".into());
                }
            }
            "--size" => {
                let v = val("--size")?;
                let (w, h) = v.split_once(['x', 'X']).ok_or("--size: WxH")?;
                let w: u32 = w.parse().map_err(|_| "--size: WxH")?;
                let h: u32 = h.parse().map_err(|_| "--size: WxH")?;
                if !(16..=8192).contains(&w) || !(16..=8192).contains(&h) {
                    return Err("--size: each side 16..8192".into());
                }
                a.size = Some((w, h));
            }
            "--windowed" => a.windowed = Some(true),
            "--fullscreen" => a.windowed = Some(false),
            "--monitor" => {
                a.monitor = val("--monitor")?
                    .parse()
                    .map_err(|_| "--monitor: integer (0 = primary)")?
            }
            "--fov" => a.fov = num(val("--fov")?, "--fov")? as f32,
            "--headless-frames" => {
                a.headless_frames = Some(
                    val("--headless-frames")?
                        .parse()
                        .map_err(|_| "--headless-frames: integer")?,
                )
            }
            "--out" => a.out = Some(PathBuf::from(val("--out")?)),
            "--frame-ms" => {
                a.frame_ms = num(val("--frame-ms")?, "--frame-ms")?;
                if !(0.1..=1000.0).contains(&a.frame_ms) {
                    return Err("--frame-ms: 0.1..1000".into());
                }
            }
            "--save-every" => {
                a.save_every = val("--save-every")?
                    .parse()
                    .map_err(|_| "--save-every: integer")?
            }
            "--window-secs" => {
                a.window_secs = Some(num(val("--window-secs")?, "--window-secs")? as f32)
            }
            "--script" => a.script = true,
            // perf-g1 stress scenarios (crate::stress)
            "--stress" => a.stress = Some(val("--stress")?),
            "--stress-search" => a.stress_search = true,
            "--stress-remotes" => {
                a.stress_remotes = val("--stress-remotes")?
                    .parse::<usize>()
                    .ok()
                    .filter(|n| (1..=64).contains(n))
                    .ok_or("--stress-remotes: 1..64")?
            }
            "--stress-view" => {
                let v = val("--stress-view")?;
                let p: Vec<&str> = v.split(',').collect();
                let [s, y, pi] = p.as_slice() else {
                    return Err("--stress-view: SPAWN,YAW_DEG,PITCH_DEG".into());
                };
                let s: usize = s.trim().parse().map_err(|_| "--stress-view: spawn")?;
                let y = num(y.trim().to_string(), "--stress-view")? as f32;
                let pi = num(pi.trim().to_string(), "--stress-view")? as f32;
                a.stress_view = Some((s, y.to_radians(), pi.to_radians()));
            }
            "--camera" => {
                let v = val("--camera")?;
                let p = v
                    .split(',')
                    .map(|t| num(t.trim().to_string(), "--camera").map(|n| n as f32))
                    .collect::<Result<Vec<f32>, _>>()?;
                let [x, y, z, yaw, pitch] = p.as_slice() else {
                    return Err("--camera: X,Y,Z,YAW_DEG,PITCH_DEG".into());
                };
                a.camera = Some(([*x, *y, *z], yaw.to_radians(), pitch.to_radians()));
            }
            // the frame profiler of the headless loop (crate::prof): every frame rendered, stage times to JSON
            "--profile" => dc_client::prof::enable(PathBuf::from(val("--profile")?)),
            "--vsync" => a.vsync = true,
            // CLI-1575: macOS presents at the display rate (Fifo) unless --no-vsync asks for Immediate
            "--no-vsync" => a.no_vsync = true,
            // CLI-890: presentation caps, frames per second, 0 = uncapped
            "--fps-cap-match" => dc_client::pacer::set_match_flag(
                val("--fps-cap-match")?
                    .parse()
                    .map_err(|_| "--fps-cap-match: integer")?,
            ),
            "--fps-cap-menu" => dc_client::pacer::set_menu_flag(
                val("--fps-cap-menu")?
                    .parse()
                    .map_err(|_| "--fps-cap-menu: integer")?,
            ),
            "--sensitivity" => {
                a.sensitivity = Some(num(val("--sensitivity")?, "--sensitivity")? as f32)
            }
            "--invert-y" => a.invert_y = true,
            "--no-overlay" => a.overlay = false,
            "--adapter" => {
                a.adapter_given = true;
                a.adapter = match val("--adapter")?.as_str() {
                    "auto" => AdapterChoice::Auto,
                    "dx12" => AdapterChoice::Backends(wgpu::Backends::DX12),
                    "vulkan" => AdapterChoice::Backends(wgpu::Backends::VULKAN),
                    "metal" => AdapterChoice::Backends(wgpu::Backends::METAL),
                    "fallback" => AdapterChoice::Fallback,
                    o => return Err(format!("--adapter: unknown {o:?}")),
                }
            }
            "--list" => a.list = true,
            "--lobby" => {
                let v = val("--lobby")?;
                let at = addr(&v, "--lobby")?;
                // CLI-1473: the typed host name is what a CA-issued certificate names
                if let Some((host, _)) = v.rsplit_once(':') {
                    dc_botclient::tls::note_host(
                        at.ip(),
                        host.trim_start_matches('[').trim_end_matches(']'),
                    );
                }
                a.lobby = Some(at);
            }
            // CLI-1471: the lobby's certificate fingerprint (beats DC_SERVER_FINGERPRINT and the remembered one)
            "--server-fingerprint" => dc_botclient::tls::set_pin(&val("--server-fingerprint")?)
                .map_err(|e| format!("--server-fingerprint: {e}"))?,
            "--udp" => a.udp = Some(addr(&val("--udp")?, "--udp")?),
            "--name" => a.name = Some(val("--name")?),
            "--user" => a.user = Some(val("--user")?),
            "--room" => a.room = val("--room")?,
            "--start-after" => {
                a.start_after = val("--start-after")?
                    .parse()
                    .map_err(|_| "--start-after: integer")?
            }
            "--title" => a.title = Some(val("--title")?),
            "--mode" => a.mode = Some(val("--mode")?),
            "--pilot-map" => a.pilot_map = Some(val("--pilot-map")?),
            "--pilot-entry" => {
                a.pilot_entry = Some(
                    val("--pilot-entry")?
                        .parse()
                        .map_err(|_| "--pilot-entry: a server entry id (a whole number)")?,
                )
            }
            "--max-players" => {
                a.max_players = val("--max-players")?
                    .parse()
                    .map_err(|_| "--max-players: integer")?
            }
            "--password" => a.password = val("--password")?,
            "--login-password" => a.login_password = val("--login-password")?,
            "--identity" => a.identity = Some(val("--identity")?),
            "--identity-fingerprint" => {
                a.identity_fingerprint = Some(val("--identity-fingerprint")?)
            }
            // CLI-1615: the community directory file's URL ("off" or empty switches it off)
            "--community" => {
                let v = val("--community")?;
                dc_client::community::set_url(if v == "off" { "" } else { &v });
            }
            "--channel" => {
                a.channel = Some(
                    val("--channel")?
                        .parse()
                        .map_err(|_| "--channel: integer")?,
                )
            }
            "--walk-test" => a.walk_test = true,
            "--world-file" => a.world_file = Some(PathBuf::from(val("--world-file")?)),
            "--mod-dir" => a.mod_dir = Some(PathBuf::from(val("--mod-dir")?)),
            "--net-log" => a.net_log = Some(PathBuf::from(val("--net-log")?)),
            "--sound-log" => a.sound_log = Some(PathBuf::from(val("--sound-log")?)),
            // DC-DEFAULT: NET-20 (proposed) a tester's bad network (docs/NETLAB.md): sets what `DC_NETLAB` sets,
            // before any thread; the match socket then runs through the network lab's impairment shim
            "--netlab" => {
                let v = val("--netlab")?;
                dc_net_engine::impair::PathSpec::parse(&v).map_err(|e| format!("--netlab: {e}"))?;
                std::env::set_var(dc_client::net::NETLAB_ENV, v);
            }
            "--no-sound" => a.sound = false,
            // DC-DEFAULT: SIM-BODY-1 (proposed) on; the switch sets what `DC_SOLID_BODIES` sets, before any thread
            "--solid-bodies" => match val("--solid-bodies")?.as_str() {
                "on" => std::env::set_var("DC_SOLID_BODIES", "1"),
                "off" => std::env::set_var("DC_SOLID_BODIES", "0"),
                o => return Err(format!("--solid-bodies: on or off, got {o:?}")),
            },
            "--audio" => {
                a.audio_out = match val("--audio")?.as_str() {
                    "device" => dc_client::sound::AudioOut::Device,
                    "null" => dc_client::sound::AudioOut::Null,
                    o => return Err(format!("--audio: device or null, got {o:?}")),
                }
            }
            "--volume" => {
                a.volume = val("--volume")?
                    .parse::<u16>()
                    .ok()
                    .filter(|v| *v <= 100)
                    .ok_or("--volume: 0..100")?
            }
            "--music-volume" => {
                a.music_volume = val("--music-volume")?
                    .parse::<u16>()
                    .ok()
                    .filter(|v| *v <= 100)
                    .ok_or("--music-volume: 0..100")?
            }
            "--capture" => a.capture = Some(PathBuf::from(val("--capture")?)),
            "--match-wait" => a.match_wait = num(val("--match-wait")?, "--match-wait")?.max(0.0),
            "--load-timeout" => {
                let s = num(val("--load-timeout")?, "--load-timeout")?;
                if s <= 0.0 {
                    return Err("--load-timeout: seconds above 0".into());
                }
                a.load_timeout = Some(s)
            }
            "--mouse-clamp" => {
                a.mouse_clamp = Some(num(val("--mouse-clamp")?, "--mouse-clamp")?.max(0.0))
            }
            "--goal" => {
                let v = val("--goal")?;
                a.goal = Some(if v == "min" {
                    GoalChoice::Min
                } else {
                    GoalChoice::Value(v.parse().map_err(|_| "--goal: a number or min")?)
                })
            }
            "--leave-after" => {
                a.leave_after = Some(num(val("--leave-after")?, "--leave-after")?.max(0.0))
            }
            "--duration" => {
                let d = num(val("--duration")?, "--duration")?;
                if d <= 0.0 {
                    return Err("--duration: seconds above 0".into());
                }
                a.duration = Some(d)
            }
            "--no-move-notices" => a.move_notices = false,
            "--reality" => a.reality = true,
            "--no-reality" => a.reality = false,
            "--draw-voice" => a.draw_voice = true,
            "--shoot-test" => a.shoot_test = true,
            "--menu-pilot" => a.menu_pilot = true,
            "--pilot-keep-result" => a.pilot_keep_result = true,
            "--debug-boxes" => a.debug_boxes = true,
            // DC-DEFAULT: CLI-1570 (proposed) scripted runs start with the network overlay on
            "--net-overlay" => dc_client::settings::force_net_overlay(1),
            "--net-overlay=1" => dc_client::settings::force_net_overlay(1),
            "--net-overlay=2" => dc_client::settings::force_net_overlay(2),
            "--no-hud" => a.hud = false,
            "--clean-frame" => a.clean_frame = true,
            "--debug-overlay" => a.debug_overlay = true,
            "--show-board" => a.show_board = true,
            "--state-shots" => a.state_shots = true,
            "--result-hold" => {
                a.result_hold = Some(num(val("--result-hold")?, "--result-hold")?.max(0.0))
            }
            "--show-board-at" => {
                let v = val("--show-board-at")?;
                let (x, y) = v.split_once(',').ok_or("--show-board-at: A,B seconds")?;
                let x = num(x.to_string(), "--show-board-at")? as f32;
                let y = num(y.to_string(), "--show-board-at")? as f32;
                a.board_at = Some((x, y));
            }
            "--slots" => {
                a.slots = val("--slots")?
                    .split(',')
                    .map(|x| {
                        x.trim()
                            .parse::<i32>()
                            .map_err(|_| format!("--slots: bad value {x:?}"))
                    })
                    .collect::<Result<_, _>>()?
            }
            "--test-correction-after" => {
                a.glitch_after = Some(
                    val("--test-correction-after")?
                        .parse()
                        .map_err(|_| "--test-correction-after: integer")?,
                )
            }
            "--menu" => {
                a.menu = val("--menu")?;
                if a.menu != "skip" && a.menu != "lobby" {
                    return Err(format!("--menu: skip or lobby, got {:?}", a.menu));
                }
            }
            "--steps" => a.steps = Some(PathBuf::from(val("--steps")?)),
            "--steps-strict" => a.steps_strict = true,
            "--events" => a.events = Some(PathBuf::from(val("--events")?)),
            "--heartbeat" => a.heartbeat = Some(PathBuf::from(val("--heartbeat")?)),
            "--shot-dir" => a.shot_dir = Some(PathBuf::from(val("--shot-dir")?)),
            "--no-activate" => a.no_activate = true,
            "-h" | "--help" => return Err(String::new()),
            o => return Err(format!("unknown argument {o:?}")),
        }
    }
    if a.pilot_entry.is_some() && a.menu != "lobby" {
        return Err(
            "--pilot-entry needs --menu lobby (the server screen's scripted player)".into(),
        );
    }
    Ok(a)
}

/// The network options of the command line, when `--lobby` or `--udp` was given.
fn net_options(a: &Args) -> Result<Option<NetOptions>, String> {
    let connect = match (a.lobby, a.udp) {
        (Some(_), Some(_)) => return Err("give --lobby or --udp, not both".into()),
        (None, None) => return Ok(None),
        (Some(auth), None) => {
            let name = a.name.clone().unwrap_or_default();
            let room = match a.room.as_str() {
                "new" => RoomChoice::New {
                    max_players: a.max_players,
                    title: a.title.clone().unwrap_or_else(|| format!("{name} room")),
                    mode: mode_row_arg(a)?.unwrap_or(0),
                    start_after: a.start_after.max(1),
                },
                "any" => RoomChoice::Any,
                n => RoomChoice::Number(
                    n.parse()
                        .map_err(|_| format!("--room: {n:?} is not new, any or a number"))?,
                ),
            };
            Connect::Lobby {
                auth,
                room,
                password: a.password.clone(),
                channel: a.channel,
            }
        }
        (None, Some(u)) => Connect::Udp(u),
    };
    let name = match a.name.clone() {
        Some(n) => n,
        // DC-DEFAULT: CLI-1490 the 2009 front end has an ID box: without --name it starts empty (the friend pack's
        // double-click launcher passes no name; the player types the ID or registers)
        None if a.menu == "lobby" && a.rom.is_some() => String::new(),
        None => return Err("--name is required with --lobby / --udp".into()),
    };
    let mut o = NetOptions::new(connect, &name);
    o.user = a.user.clone();
    o.rom = a.rom.clone();
    o.log = a.net_log.clone();
    o.capture = a.capture.clone();
    o.match_wait = Duration::from_secs_f64(a.match_wait);
    if let Some(s) = a.load_timeout {
        o.load_timeout = Duration::from_secs_f64(s);
    }
    o.glitch_after_updates = a.glitch_after;
    o.goal = a.goal;
    o.leave_after = a.leave_after.map(Duration::from_secs_f64);
    if let Some(h) = a.result_hold {
        o.result_hold = Duration::from_secs_f64(h);
    }
    o.move_notices = a.move_notices;
    o.reality = a.reality;
    o.draw_voice = a.draw_voice;
    o.lobby_slots = a.slots.clone();
    if let Some(p) = &a.world_file {
        o.world_file = Some(WorldFile::from_path(p).map_err(|e| e.to_string())?);
    }
    o.mods = mod_recipe_of(a)?;
    Ok(Some(o))
}

/// MAP-31: `--mod-dir DIR` as the recipe of a one-mod server set (the folder's manifest names the mod), so a
/// direct join resolves `@mod:` announces and the match installs the folder's files. `None` without the flag.
fn mod_recipe_of(a: &Args) -> Result<Option<dc_client::mods::HostRecipe>, String> {
    let Some(dir) = &a.mod_dir else {
        return Ok(None);
    };
    let man = dc_mod_lua::ModManifest::from_file(&dir.join("manifest.json"))
        .map_err(|e| format!("--mod-dir {}: {e}", dir.display()))?;
    Ok(Some(dc_client::mods::HostRecipe {
        policy: dc_mod_lua::ModPolicy::Modded {
            client_visual_allowed: false,
        },
        settings: Default::default(),
        mods: vec![dc_client::mods::RecipeMod {
            dir: dir.clone(),
            origin: dc_mod_lua::ModOrigin::Server,
            id: man.id.clone(),
            version: man.version.to_string(),
            kind: man.kind.to_string(),
        }],
    }))
}

/// Prints what the network session did.
fn print_net(st: &NetStats) {
    println!(
        "dc-client net: {} | stage {} | end {:?}",
        st.phase.as_ref().map_or("-".into(), |p| p.label()),
        st.stage,
        st.end
    );
    println!(
        "  world {:?} client id {:?} own object {:?} | updates sent {} | teleports {} (taken {}) | life states {:?}",
        st.world, st.client_id, st.view_object, st.updates_sent, st.teleports.len(), st.corrections_taken,
        st.life_states
    );
    for (obj, t) in &st.tracks {
        println!(
            "  remote {} (pid {}, team {}, object {obj}): {} samples, travel {:.1}, interval {:.0} ms, last {:?}",
            t.name, t.pid, t.team, t.sample_count, t.travel, t.gap_ms, t.last
        );
    }
    for k in &st.kills {
        println!("  kill: {k}");
    }
    println!(
        "  round result {:?} result screen {:?} level exit {:?} | bad check {} resends {}",
        st.hud.round_result, st.hud.result_screen, st.hud.level_exit, st.bad_check, st.resends
    );
    println!(
        "  mode class {:?} | move notices {} | left by {:?} (lobby 0x394 sent {}) | HUD HP {:?} AR {:?}",
        st.mode_class, st.notices_sent, st.leave_kind, st.leave_notice_sent, st.hud.health, st.hud.armor
    );
    println!(
        "  weapons: grants {:?} held {:?} | {:?} | kills (victim, killer) {:?} | damage taken {}",
        st.grants, st.weapon, st.arms, st.kill_pairs, st.damage_taken
    );
    let ms: Vec<&str> = st.milestones.iter().map(|m| m.0.as_str()).collect();
    println!("  milestones: {}", ms.join(" "));
    for f in &st.fatal {
        println!("  fatal: {f}");
    }
}

fn run_net(a: Args, opts: NetOptions) -> Result<(), String> {
    let view = ViewSettings::with_reference_fov_x(a.fov);
    let font_rom = if a.overlay {
        a.rom.clone().or_else(|| dc_rom::resolve_rom().ok())
    } else {
        None
    };
    let load_timeout = opts.load_timeout;
    let mods = opts.mods.clone();
    let mut net = NetWorld::start(opts);
    let handle: NetHandle = net.handle();
    println!("dc-client: {}", net.describe());
    // --duration S: at most S seconds of frames (the end leaves the match cleanly through the shutdown)
    let by_duration = a
        .duration
        .map(|d| (d * 1000.0 / a.frame_ms).ceil().min(u32::MAX as f64) as u32);
    let headless = match (a.headless_frames, by_duration) {
        (Some(n), Some(d)) => Some(n.min(d)),
        (n, d) => n.or(d),
    };
    if let Some(frames) = headless {
        let out = a.out.clone().unwrap_or_else(default_out_dir);
        ensure_dir(&out).map_err(|e| e.to_string())?;
        let (w, h) = a.size.unwrap_or((800, 600));
        let hopts = HeadlessOptions {
            camera: None,
            room_hacks: None,
            frames,
            frame_us: (a.frame_ms * 1000.0).round() as u64,
            width: w,
            height: h,
            out: Some(out.clone()),
            prefix: "net_".into(),
            save_every: a.save_every.max(1),
            render: true,
            adapter: a.adapter,
            tick_hz: a.tick,
            spawn: 0,
            view,
            params: dc_sim::MovementParams::default(),
            font_rom,
            font_bytes: None,
            realtime: true,
            // one second of frames after the server ended the session (the round end, a bye)
            // DC-DEFAULT: CLI-26 (proposed) headless network run end
            stop_after_over: Some((1000.0 / a.frame_ms).ceil() as u32),
            models: true,
            debug_boxes: a.debug_boxes,
            hud: hud_choice(&a),
            show_board: a.show_board,
            debug_panel: !a.hud || a.debug_overlay,
            trace: false,
            state_shots: a.state_shots,
            sound: dc_client::sound::SoundOptions {
                enabled: a.sound,
                log: a.sound_log.clone(),
            },
            hash_frames: false,
            // MAP-31: the headless match installs the mod layer, as the window does
            mods,
        };
        let mut walk = WalkTest::new(ViewAngles::default());
        let mut shoot = ShootTest::new(a.frame_ms / 1000.0, a.walk_test.then_some(walk));
        let mut steps = dc_client::steps::StepInput::new(look_of(&a));
        let input: &mut dyn InputSource = if dc_client::steps::active() {
            &mut steps
        } else if a.shoot_test {
            &mut shoot
        } else {
            &mut walk
        };
        let r = run_headless(&mut net, Some(input), &hopts);
        let st = handle.shutdown(Duration::from_secs(5));
        let r = r.map_err(|e| e.to_string())?;
        println!("{}", r.summary());
        if let Some(n) = &r.sound_note {
            println!("  {n}");
        }
        if let Some(n) = &r.sound_bounds {
            println!("  {n}");
        }
        println!(
            "  HUD {} | inputs {} | composed {}",
            r.hud.as_deref().unwrap_or("off"),
            r.hud_events,
            r.hud_composed
        );
        for l in &r.last_lines {
            println!("  | {l}");
        }
        print_net(&st);
        println!("  PNGs in {} (ROM-derived, never commit)", out.display());
        if r.nan_ticks > 0 || r.nan_frames > 0 || r.nan_remotes > 0 {
            return Err("non-finite state (see the counts above)".into());
        }
        return Ok(());
    }
    // the lobby and the join run before the window opens, so the window does not wait on them
    let world = handle.wait_world(load_timeout).map_err(|e| e.to_string())?;
    println!("dc-client: server announced {world:?}");
    let r = run_window_mode(Box::new(net), a, view, font_rom);
    let st = handle.shutdown(Duration::from_secs(5));
    print_net(&st);
    r
}

fn run(a: Args) -> Result<(), String> {
    if a.menu == "lobby" {
        return run_menu(a);
    }
    if let Some(opts) = net_options(&a)? {
        return run_net(a, opts);
    }
    // MAP-33: --world-file without a server is the standalone preview of a world file
    if let Some(file) = a.world_file.clone() {
        return run_preview(a, &file);
    }
    let rom = match a.rom.clone() {
        Some(r) => r,
        None => dc_rom::resolve_rom().map_err(|e| format!("--rom not given and CA_ROM: {e}"))?,
    };
    if a.list {
        for n in RomWorld::map_names(&rom).map_err(|e| e.to_string())? {
            println!("{n}");
        }
        return Ok(());
    }
    let map = a.map.clone().ok_or("--map is required (see --list)")?;
    let view = ViewSettings::with_reference_fov_x(a.fov);
    let font_rom = a.overlay.then(|| rom.clone());
    let stress = stress_config(&a, &rom, &map)?;
    if a.stress_search && stress.is_none() {
        return Ok(());
    }
    if let Some(frames) = a.headless_frames {
        let (opts, out) = map_headless_options(
            &a,
            frames,
            format!("{}_s{}_", map.to_ascii_lowercase(), a.spawn),
            font_rom,
            stress.as_ref().map_or(a.spawn, |c| c.spawn()),
            a.show_board || stress.as_ref().is_some_and(|c| c.scenario.board()),
            view,
        )?;
        let mut src: Box<dyn WorldSource> = match &stress {
            Some(c) => Box::new(dc_client::stress::StressSource::new(&rom, &map, c.clone())),
            None => Box::new(RomWorld::new(&rom, &map)),
        };
        let r = run_headless(src.as_mut(), None, &opts).map_err(|e| e.to_string())?;
        return print_map_report(&r, &out);
    }
    let mut a = a;
    if let Some(c) = &stress {
        a.spawn = c.spawn();
    }
    let src: Box<dyn WorldSource> = match stress {
        Some(c) => Box::new(dc_client::stress::StressSource::new(&rom, &map, c)),
        None => Box::new(RomWorld::new(&rom, &map)),
    };
    run_window_mode(src, a, view, font_rom)
}

/// The headless options of a map run (`--map`, `--world-file`): `frames` frames of `--frame-ms`, the PNGs under
/// `--out` (or the default folder) named `<prefix>NNNN.png`. The frame length is checked against dc-sim's
/// catch-up limit.
#[allow(clippy::too_many_arguments)]
fn map_headless_options(
    a: &Args,
    frames: u32,
    prefix: String,
    font_rom: Option<PathBuf>,
    spawn: usize,
    show_board: bool,
    view: ViewSettings,
) -> Result<(HeadlessOptions, PathBuf), String> {
    // dc-sim's FixedStep runs at most 8 catch-up ticks per frame and drops a longer backlog
    // (DC-DEFAULT: SIM-1, proposed); a longer frame would silently slow the simulation down.
    let ticks_per_frame = a.frame_ms * a.tick as f64 / 1000.0;
    if ticks_per_frame > 8.0 {
        return Err(format!(
            "--frame-ms {} at --tick {} needs {ticks_per_frame:.1} ticks per frame; at most 8 run per frame",
            a.frame_ms, a.tick
        ));
    }
    let out = a.out.clone().unwrap_or_else(default_out_dir);
    ensure_dir(&out).map_err(|e| e.to_string())?;
    let (w, h) = a.size.unwrap_or((800, 600));
    let opts = HeadlessOptions {
        room_hacks: None,
        frames,
        frame_us: (a.frame_ms * 1000.0).round() as u64,
        width: w,
        height: h,
        out: Some(out.clone()),
        prefix,
        save_every: a.save_every.max(1),
        render: true,
        adapter: a.adapter,
        tick_hz: a.tick,
        spawn,
        view,
        params: dc_sim::MovementParams::default(),
        font_rom,
        font_bytes: None,
        realtime: false,
        stop_after_over: None,
        models: true,
        debug_boxes: a.debug_boxes,
        hud: if a.clean_frame {
            dc_client::hud::HudChoice::Off
        } else {
            hud_choice(a)
        },
        show_board,
        debug_panel: !a.clean_frame && (!a.hud || a.debug_overlay),
        camera: a.camera,
        trace: false,
        state_shots: a.state_shots,
        sound: dc_client::sound::SoundOptions {
            enabled: a.sound,
            log: a.sound_log.clone(),
        },
        hash_frames: false,
        mods: None,
    };
    Ok((opts, out))
}

/// Prints what a headless map run did; the simulation checks decide the exit code.
fn print_map_report(r: &dc_client::HeadlessReport, out: &std::path::Path) -> Result<(), String> {
    println!("{}", r.summary());
    if let Some(n) = &r.sound_note {
        println!("  {n}");
    }
    if let Some(n) = &r.sound_bounds {
        println!("  {n}");
    }
    println!(
        "  HUD {} | inputs {} | composed {}",
        r.hud.as_deref().unwrap_or("off"),
        r.hud_events,
        r.hud_composed
    );
    for l in &r.last_lines {
        println!("  | {l}");
    }
    println!("  PNGs in {} (ROM-derived, never commit)", out.display());
    if r.nan_ticks > 0 || r.nan_frames > 0 || r.tunnel_ticks > 0 || r.fell_out {
        return Err("simulation check failed (see the counts above)".into());
    }
    Ok(())
}

/// MAP-33: the standalone preview of a world file (no server): `--rom` / `CA_ROM` for the ROM's textures, models
/// and fonts when at hand (none is fine), `--mod-dir` for the `@mod:` textures, solid colours for the rest; the
/// spawns are the file's own `dc.player_spawn` objects. Headless (`--headless-frames N --out DIR`) or in the window.
fn run_preview(a: Args, file: &std::path::Path) -> Result<(), String> {
    let rom = match a.rom.clone() {
        Some(r) => {
            dc_client::check_rom_dir(&r)?;
            Some(r)
        }
        None => dc_rom::resolve_rom().ok(),
    };
    let src = dc_client::ModWorld::preview(file, rom.clone(), a.mod_dir.as_deref())
        .map_err(|e| e.to_string())?;
    println!(
        "dc-client: {} from {} (ROM {}, mod folder {})",
        src.describe(),
        file.display(),
        rom.as_ref()
            .map_or("none".to_string(), |r| r.display().to_string()),
        a.mod_dir
            .as_ref()
            .map_or("none".to_string(), |d| d.display().to_string())
    );
    let view = ViewSettings::with_reference_fov_x(a.fov);
    let font_rom = rom.clone().filter(|_| a.overlay);
    if let Some(frames) = a.headless_frames {
        let (opts, out) = map_headless_options(
            &a,
            frames,
            format!("{}_s{}_", src.name.to_ascii_lowercase(), a.spawn),
            font_rom,
            a.spawn,
            a.show_board,
            view,
        )?;
        let mut src: Box<dyn WorldSource> = Box::new(src);
        let r = run_headless(src.as_mut(), None, &opts).map_err(|e| e.to_string())?;
        return print_map_report(&r, &out);
    }
    run_window_mode(Box::new(src), a, view, font_rom)
}

/// The stress scenario of the command line (`--stress`), with the visibility search (`--stress-search`, or `bsp`
/// without `--stress-view`) printed as one JSON line and its worst view pinned for `bsp`.
fn stress_config(
    a: &Args,
    rom: &std::path::Path,
    map: &str,
) -> Result<Option<dc_client::stress::StressConfig>, String> {
    use dc_client::stress::{search_views, Scenario, StressConfig};
    let scenario = match a.stress.as_deref() {
        Some(n) => {
            Some(Scenario::parse(n).ok_or_else(|| format!("--stress: unknown scenario {n:?}"))?)
        }
        None => None,
    };
    let need_search =
        a.stress_search || (scenario == Some(Scenario::Bsp) && a.stress_view.is_none());
    let mut best = None;
    if need_search {
        let size = a.size.unwrap_or((800, 600));
        let s = search_views(rom, map, size, a.adapter).map_err(|e| e.to_string())?;
        println!("dc-client view search: {}", s.json());
        best = s.top.first().map(|v| (v.spawn, v.yaw, v.pitch));
    }
    Ok(scenario.map(|s| {
        dc_client::prof::set_scenario(s.name());
        let mut c = StressConfig::new(s);
        c.remotes = a.stress_remotes;
        c.frame_ms = a.frame_ms;
        c.view = a
            .stress_view
            .or(if s == Scenario::Bsp { best } else { None });
        c
    }))
}

/// The HUD of the command line: the ROM layout when a ROM is known (else our fallback layout), none with `--no-hud`.
fn hud_choice(a: &Args) -> dc_client::hud::HudChoice {
    use dc_client::hud::HudChoice;
    if !a.hud {
        return HudChoice::Off;
    }
    match a.rom.clone().or_else(|| dc_rom::resolve_rom().ok()) {
        Some(r) => HudChoice::Rom(r),
        None => HudChoice::Fallback,
    }
}

fn look_of(a: &Args) -> dc_client::LookParams {
    let mut look = dc_client::LookParams::DEFAULT;
    if let Some(s) = a.sensitivity {
        look.sensitivity = s;
    }
    if let Some(m) = a.mouse_clamp {
        look.max_delta_counts = m;
    }
    look.invert_y = a.invert_y;
    look
}

/// CLI-1852: the adapter choice of a game window and whether it is only a first attempt: `--adapter` wins, else the
/// profile's Renderer choice where this build offers it (Windows), else the default backends.
#[cfg(feature = "window")]
fn window_adapter(a: &Args) -> (AdapterChoice, bool) {
    let choosable = dc_client::settings::renderer_choosable();
    let saved = if choosable && !a.adapter_given {
        let store = dc_client::settings::ProfileStore::for_client(
            a.rom
                .clone()
                .or_else(|| dc_rom::resolve_rom().ok())
                .as_deref(),
            dc_client::settings::own_profile_dir().as_deref(),
        );
        dc_client::settings::saved_renderer(&store)
    } else {
        dc_client::settings::RENDERER_DEFAULT
    };
    dc_client::settings::adapter_for_run(a.adapter_given.then_some(a.adapter), saved, choosable)
}

#[cfg(feature = "window")]
fn run_window_mode(
    source: Box<dyn WorldSource>,
    a: Args,
    view: ViewSettings,
    font_rom: Option<PathBuf>,
) -> Result<(), String> {
    use dc_client::window::{run_window, WindowOptions};
    let window_choice = window_adapter(&a);
    let opts = WindowOptions {
        // CLI-1282: `--size` alone is the test tools' centred borderless window; with a look flag it is the
        // windowed size only (full screen fills the monitor)
        size: a.size.filter(|_| a.windowed.is_none()),
        windowed_size: a.size,
        start_look: a.windowed.map(|w| {
            if w {
                dc_client::window::WindowLook::Bordered
            } else {
                dc_client::window::WindowLook::Borderless
            }
        }),
        keep_size: a.size.is_some(),
        monitor: a.monitor,
        seconds: a.window_secs,
        scripted: a.script,
        walk_test: a.walk_test,
        shoot_test: a.shoot_test,
        debug_boxes: a.debug_boxes,
        tick_hz: a.tick,
        spawn: a.spawn,
        view,
        params: dc_sim::MovementParams::default(),
        look: look_of(&a),
        vsync: a.vsync,
        no_vsync: a.no_vsync,
        font_rom,
        adapter: window_choice.0,
        adapter_first: window_choice.1,
        hud: hud_choice(&a),
        debug_panel: !a.hud || a.debug_overlay,
        board_at: a.board_at,
        // MAP-31: a direct join's `--mod-dir` is the match's mod set (a preview's folder, MAP-33, is a texture
        // folder, not a mod set: the world source reads it itself)
        mods: if a.world_file.is_some() {
            None
        } else {
            mod_recipe_of(&a)?
        },
        room_hacks: None,
        gun_ladder: Vec::new(),
        sound: dc_client::sound::SoundOptions {
            enabled: a.sound,
            log: a.sound_log.clone(),
        },
        audio: dc_client::sound::AudioConfig {
            out: a.audio_out,
            effects_volume: a.volume,
            music_volume: a.music_volume,
            ..Default::default()
        },
    };
    let r = run_window(source, opts).map_err(|e| e.to_string())?;
    for l in dc_client::window::exit_report_lines(&r) {
        println!("{l}");
    }
    Ok(())
}

#[cfg(not(feature = "window"))]
fn run_window_mode(
    _source: Box<dyn WorldSource>,
    _a: Args,
    _view: ViewSettings,
    _font_rom: Option<PathBuf>,
) -> Result<(), String> {
    Err("built without the `window` feature: use --headless-frames".into())
}

/// `--menu lobby`: the 2009 front end before every match (window), or a scripted walk through it (headless).
fn run_menu(a: Args) -> Result<(), String> {
    use dc_client::menu::link::LinkOptions;
    use dc_client::menu::pilot::{headless_runner, run as run_pilot, Pilot, PilotPlan};
    use dc_client::menu::{Menu, MenuOptions};
    let auth = a
        .lobby
        .ok_or("--menu lobby needs --lobby HOST:PORT (the auth address)")?;
    let base = net_options(&a)?.ok_or("--menu lobby needs --lobby")?;
    let login = a
        .user
        .clone()
        .or_else(|| a.name.clone())
        .unwrap_or_default();
    let mut link = LinkOptions::new(&base.name);
    // CLI-1610: the identity authority (None with --identity off)
    let identity = dc_client::identity::resolve(
        a.identity.as_deref(),
        a.identity_fingerprint.as_deref(),
        auth,
        &dc_botclient::tls::current(),
        dc_client::settings::own_profile_dir().as_deref(),
    )?;
    link.identity = identity.clone();
    dc_client::identity::set_current(identity.clone());
    // CLI-1480: the player's client opens the nickname screen for a login without a character; scripted and headless
    // runs (and DC_AUTO_CHARACTER=1) make the character at once (CLI-162)
    link.auto_character = a.menu_pilot
        || a.steps.is_some()
        || a.headless_frames.is_some()
        || a.duration.is_some()
        || std::env::var("DC_AUTO_CHARACTER").is_ok_and(|v| v == "1");
    // CLI-1608: a server's mods are cached and unpacked under the profile folder (<data>/mods)
    link.mods.data_dir = dc_client::settings::own_profile_dir();
    // the menu's lobby connection is recorded next to the session's files (the auth connection never is)
    link.log = a.net_log.as_ref().map(|p| p.with_extension("menu.jsonl"));
    link.capture = a
        .capture
        .as_ref()
        .map(|p| p.with_extension("menu.capture.jsonl"));
    if let Some(r) = &a.rom {
        dc_client::check_rom_dir(r)?;
    }
    let rom = a.rom.clone().or_else(|| dc_rom::resolve_rom().ok());
    let (w, h) = a.size.unwrap_or((1024, 768));
    let mut menu = Menu::new(MenuOptions {
        auth,
        user: login.clone(),
        link,
        rom,
        font: None,
        width: w,
        height: h,
    });
    println!("dc-client menu: {}", menu.source());
    if let Some(id) = &identity {
        let now = dc_client::identity::now();
        let cached = id
            .cache
            .as_deref()
            .and_then(|p| dc_client::identity::load(p, now))
            .filter(|c| c.authority == id.key)
            .map(|c| c.login);
        println!(
            "dc-client menu: identity authority {}{}",
            id.key,
            if cached.is_some() {
                " (cached login)"
            } else {
                ""
            }
        );
        menu.set_identity(Some(id.addr), cached);
    }
    // w3-options: the player's profile (options, key binds) is read at start-up
    let store = dc_client::settings::ProfileStore::for_client(
        a.rom
            .clone()
            .or_else(|| dc_rom::resolve_rom().ok())
            .as_deref(),
        dc_client::settings::own_profile_dir().as_deref(),
    );
    match menu.load_settings(store) {
        Some(p) => println!("dc-client menu: settings from {}", p.display()),
        None => println!("dc-client menu: no profile found, default settings"),
    }
    // w2-lobbychat: scripted drivers run without the waiting room's idle rules (CLI-486)
    if a.menu_pilot || a.steps.is_some() {
        menu.set_idle_rules(false);
    }
    // CLI-810: a by-hand run starts with the password in the box; a pilot types it itself (PilotPlan): the window
    // pilot and the headless walk (--duration / --headless-frames), which doubled the password when the box was
    // filled too (found against the VPS lobby 2026-10-30: "login refused (password)" for a correct password)
    let scripted_login = a.menu_pilot || a.duration.is_some() || a.headless_frames.is_some();
    if !scripted_login && !a.login_password.is_empty() {
        menu.set_login_password(&a.login_password);
    }
    let by_duration = a
        .duration
        .map(|d| (d * 1000.0 / a.frame_ms).ceil().min(u32::MAX as f64) as u32);
    let headless = match (a.headless_frames, by_duration) {
        (Some(n), Some(d)) => Some(n.min(d)),
        (n, d) => n.or(d),
    };
    let Some(frames) = headless else {
        // CLI-1842: the reports queued by earlier runs go now; files of a crashed run become a problem to report
        dc_client::diag::start();
        return run_menu_window(menu, a, base);
    };
    let out = a.out.clone().unwrap_or_else(default_out_dir);
    ensure_dir(&out).map_err(|e| e.to_string())?;
    let room = pilot_room_checked(&a)?;
    let mut pilot = Pilot::new(PilotPlan {
        password: a.login_password.clone(),
        room,
        matches: 1,
        shots: Some(out.clone()),
        timeout: base.match_wait + base.load_timeout + Duration::from_secs(60),
    });
    if let Some(id) = a.pilot_entry {
        pilot = pilot.on_entry(id);
    }
    let hopts = HeadlessOptions {
        camera: a.camera,
        mods: None,
        room_hacks: None,
        frames,
        debug_panel: !a.hud || a.debug_overlay,
        frame_us: (a.frame_ms * 1000.0).round() as u64,
        width: w,
        height: h,
        out: Some(out.clone()),
        prefix: "net_".into(),
        save_every: a.save_every.max(1),
        adapter: a.adapter,
        tick_hz: a.tick,
        font_rom: a.rom.clone().or_else(|| dc_rom::resolve_rom().ok()),
        realtime: true,
        // DC-DEFAULT: CLI-26 (proposed) headless network run end
        stop_after_over: Some((1000.0 / a.frame_ms).ceil() as u32),
        hud: hud_choice(&a),
        sound: dc_client::sound::SoundOptions {
            enabled: a.sound,
            log: a.sound_log.clone(),
        },
        ..HeadlessOptions::default()
    };
    let mut lines = Vec::new();
    let steps = dc_client::steps::active();
    let report = {
        let mut play = headless_runner(&base, login, &hopts, &mut lines);
        // `--room new`: the scripted player walks the menu, and the steps take over in the match
        if steps && a.room != "new" {
            let limit = a.duration.map_or(
                base.match_wait + base.load_timeout + Duration::from_secs(60),
                Duration::from_secs_f64,
            );
            dc_client::menu::pilot::run_steps(&mut menu, &mut play, limit)
        } else {
            run_pilot(&mut menu, &mut pilot, &mut play)
        }
    };
    println!("dc-client menu: screens {}", report.trail.join(" -> "));
    for l in &lines {
        println!("  {l}");
    }
    for b in &report.boxes {
        println!("  box: {b}");
    }
    println!(
        "  {} menu PNGs in {} (ROM-derived, never commit); end: {}",
        report.shots.len(),
        out.display(),
        report.end
    );
    if report.matches == 0 && !steps {
        return Err(format!("no match played ({})", report.end));
    }
    Ok(())
}

/// The ROM room table's map and mode names (empty without a ROM), read only when a name has to be looked up.
fn room_names(a: &Args) -> dc_client::menu::pilot::RoomNames {
    a.rom
        .clone()
        .or_else(|| dc_rom::resolve_rom().ok())
        .and_then(|r| dc_bute::GameRoomData::from_rom(&r).ok())
        .map(|g| dc_client::menu::pilot::RoomNames::from_rows(&g.maps(), &g.modes(false)))
        .unwrap_or_default()
}

/// `--mode` as a mode row: a number as given, a name through the ROM room table.
fn mode_row_arg(a: &Args) -> Result<Option<i32>, String> {
    match a.mode.as_deref() {
        None => Ok(None),
        Some(m) => match m.trim().parse::<i32>() {
            Ok(n) => Ok(Some(n)),
            Err(_) => room_names(a).mode(m).map(Some),
        },
    }
}

/// `--pilot-map` as a map row (a number or a name through the ROM room table).
fn map_row_arg(a: &Args) -> Result<Option<i32>, String> {
    match a.pilot_map.as_deref() {
        None => Ok(None),
        Some(m) => room_names(a).map(m).map(Some),
    }
}

/// The scripted menu player's room plan from `--room` (`new`: create with `--title` / `--mode` / `--pilot-map` /
/// `--start-after`).
fn pilot_room_checked(a: &Args) -> Result<dc_client::menu::pilot::PilotRoom, String> {
    use dc_client::menu::pilot::PilotRoom;
    Ok(match a.room.as_str() {
        "new" => PilotRoom::Create {
            title: a
                .title
                .clone()
                .unwrap_or_else(|| format!("{} room", a.name.clone().unwrap_or_default())),
            mode: mode_row_arg(a)?,
            map: map_row_arg(a)?,
            start_after: a.start_after.max(1),
        },
        "any" => PilotRoom::Join(None),
        n => PilotRoom::Join(Some(
            n.parse()
                .map_err(|_| format!("--room: {n:?} is not new, any or a number"))?,
        )),
    })
}

#[cfg(feature = "window")]
fn run_menu_window(menu: dc_client::menu::Menu, a: Args, base: NetOptions) -> Result<(), String> {
    use dc_client::window::WindowOptions;
    let view = ViewSettings::with_reference_fov_x(a.fov);
    let font_rom = if a.overlay {
        a.rom.clone().or_else(|| dc_rom::resolve_rom().ok())
    } else {
        None
    };
    let window_choice = window_adapter(&a);
    let mut opts = WindowOptions {
        // CLI-1282: `--size` alone is the test tools' centred borderless window; with a look flag it is the
        // windowed size only (full screen fills the monitor)
        size: a.size.filter(|_| a.windowed.is_none()),
        windowed_size: a.size,
        start_look: a.windowed.map(|w| {
            if w {
                dc_client::window::WindowLook::Bordered
            } else {
                dc_client::window::WindowLook::Borderless
            }
        }),
        keep_size: a.size.is_some(),
        monitor: a.monitor,
        seconds: a.window_secs,
        scripted: a.script,
        walk_test: a.walk_test,
        shoot_test: a.shoot_test,
        debug_boxes: a.debug_boxes,
        tick_hz: a.tick,
        spawn: a.spawn,
        view,
        params: dc_sim::MovementParams::default(),
        look: look_of(&a),
        vsync: a.vsync,
        no_vsync: a.no_vsync,
        font_rom,
        adapter: window_choice.0,
        adapter_first: window_choice.1,
        hud: hud_choice(&a),
        debug_panel: !a.hud || a.debug_overlay,
        board_at: a.board_at,
        mods: None,
        room_hacks: None,
        gun_ladder: Vec::new(),
        sound: dc_client::sound::SoundOptions {
            enabled: a.sound,
            log: a.sound_log.clone(),
        },
        audio: dc_client::sound::AudioConfig {
            out: a.audio_out,
            effects_volume: a.volume,
            music_volume: a.music_volume,
            ..Default::default()
        },
    };
    // w3-options: a profile that was read sets what the command line did not (the volume flags scale it)
    // DC-DEFAULT: CLI-554 (proposed) command line flags win over the profile; --volume / --music-volume are a
    // percentage of the profile's volume
    if menu.data.profile_doc.is_some() {
        let s = &menu.data.settings;
        if a.sensitivity.is_none() {
            opts.look.sensitivity = s.sensitivity as f32;
        }
        if !a.invert_y {
            opts.look.invert_y = s.invert_y;
        }
        // CLI-1280: the saved resolution is the windowed look's size (full screen fills the monitor)
        if a.size.is_none() {
            opts.windowed_size = s.resolution;
        }
        let scale = |own: u16, flag: u16| (u32::from(own) * u32::from(flag.min(100)) / 100) as u16;
        opts.audio.effects_volume = scale(s.effective_effects(), a.volume);
        opts.audio.music_volume = scale(s.effective_music(), a.music_volume);
    }
    // fix-hang: --menu-pilot clicks through the screens in the window as the headless scripted player does
    // (an unknown --mode / --pilot-map name stops here, before the window opens)
    let room = if a.menu_pilot {
        pilot_room_checked(&a)?
    } else {
        dc_client::menu::pilot::PilotRoom::Join(None)
    };
    let pilot = a.menu_pilot.then(|| {
        let p = dc_client::menu::pilot::Pilot::new(dc_client::menu::pilot::PilotPlan {
            password: a.login_password.clone(),
            room,
            matches: usize::MAX,
            shots: None,
            timeout: Duration::from_secs(24 * 3600),
        });
        let p = match a.pilot_entry {
            Some(id) => p.on_entry(id),
            None => p,
        };
        if a.pilot_keep_result {
            p.keep_result()
        } else {
            p
        }
    });
    let trail = dc_client::menu::window::run_menu_window(menu, opts, base, pilot)
        .map_err(|e| e.to_string())?;
    println!("dc-client menu: screens {}", trail.join(" -> "));
    Ok(())
}

#[cfg(not(feature = "window"))]
fn run_menu_window(
    _menu: dc_client::menu::Menu,
    _a: Args,
    _base: NetOptions,
) -> Result<(), String> {
    Err("built without the `window` feature: use --headless-frames".into())
}

/// Gate T2: the event stream, the step file and the scenario settings, before any screen or match starts.
fn start_scenario(a: &Args) -> Result<(), String> {
    dc_client::steps::configure(dc_client::steps::StepConfig {
        shot_dir: a.shot_dir.clone(),
        no_activate: a.no_activate,
    });
    if a.events.is_some() || a.steps.is_some() {
        dc_client::events::open(a.events.as_deref())
            .map_err(|e| format!("--events {:?}: {e}", a.events))?;
    }
    // gate T1: the panic report and the exception filter go where the watchdog writes; the heartbeat ledger
    let report_dir = a
        .sound_log
        .as_ref()
        .or(a.heartbeat.as_ref())
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    dc_hangdump::crumb::configure_from_env();
    dc_client::watchdog::install_crash_reports(dc_client::watchdog::report_dir(report_dir));
    // CLI-1840: the diagnostic reports' folders: the launcher's logs (the --net-log file's folder), the watchdog's
    // report folder, the profile (queue, ledger)
    dc_client::diag::configure(dc_client::diag::Config {
        logs_dir: a
            .net_log
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .or_else(dc_client::diag::default_logs_dir),
        net_log: a.net_log.clone(),
        report_dir: Some(dc_client::watchdog::report_dir(None).to_path_buf()),
        profile: dc_client::settings::own_profile_dir(),
        off: false,
    });
    if let Some(p) = &a.events {
        dc_client::watchdog::set_event_log(p);
    }
    if let Some(p) = &a.heartbeat {
        dc_client::heartbeat::start(p).map_err(|e| format!("--heartbeat {}: {e}", p.display()))?;
    }
    if let Some(p) = &a.steps {
        let text =
            std::fs::read_to_string(p).map_err(|e| format!("--steps {}: {e}", p.display()))?;
        let steps =
            dc_client::steps::parse(&text).map_err(|e| format!("--steps {}: {e}", p.display()))?;
        println!(
            "dc-client steps: {} step(s) from {}{}",
            steps.len(),
            p.display(),
            if a.steps_strict { " (strict)" } else { "" }
        );
        dc_client::events::emit(
            "run",
            serde_json::json!({ "steps": p.display().to_string(), "strict": a.steps_strict,
                                "count": steps.len() }),
        );
        dc_client::steps::install(dc_client::steps::StepRunner::new(steps, a.steps_strict));
    }
    Ok(())
}

/// The step file's result line and the stream's last event.
fn end_scenario(result: &Result<(), String>) {
    let err = result.as_ref().err();
    if let Some((ok, failed, done)) = dc_client::steps::summary() {
        println!(
            "dc-client steps: ok {ok} failed {} {}",
            failed.len(),
            if done {
                "(all steps ran)"
            } else {
                "(not all steps ran)"
            }
        );
        for f in &failed {
            println!("  step failed: {f}");
        }
        dc_client::events::emit(
            "run_end",
            serde_json::json!({ "ok": ok, "failed": failed.len(), "all_ran": done, "error": err }),
        );
    } else {
        dc_client::events::emit("run_end", serde_json::json!({ "error": err }));
    }
}

fn main() -> ExitCode {
    let a = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("dc-client: {e}");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    // CLI-1470: the trust-on-first-use fingerprints live in the profile folder
    if let Some(d) = dc_client::settings::own_profile_dir() {
        dc_botclient::tls::set_store(d.join(dc_botclient::tls::KNOWN_SERVERS_FILE));
    }
    // the client needs the full client folder, not the server ROM a host made for the servers (docs/SERVER_ROM.md)
    if let Some(rom) = a.rom.clone().or_else(|| dc_rom::resolve_rom().ok()) {
        if let Err(e) = dc_rom::server_rom::refuse_for_client(&rom) {
            eprintln!("dc-client: {e}");
            return ExitCode::from(2);
        }
    }
    if let Err(e) = start_scenario(&a) {
        eprintln!("dc-client: {e}");
        return ExitCode::from(2);
    }
    let r = run(a);
    end_scenario(&r);
    // gate T1: a normal end removes the empty crash folder made at start-up
    dc_hangdump::crash::cleanup();
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dc-client: {e}");
            ExitCode::FAILURE
        }
    }
}
