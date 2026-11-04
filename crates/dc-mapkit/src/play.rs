//! `dc-mapkit play <dir>`: Vanilla++ on this machine (MAPKIT_PLAN.md D9, 5.2; the 2026-11-01 decision that
//! community maps ride the Vanilla++ FEATURED ENTRY of the vanilla lobby): the project is exported, a VANILLA
//! `dc-lobby-server` instance with one `[[lobby.servers]]` entry "Vanilla++" whose curated set (LG-313) is the
//! exported zip is started on loopback together with its `dc-dserver`, and `dc-client` joins it through the 2009
//! front end (by hand in a window, or hands-free headless with the menu pilot picking the entry, creating a room on
//! the map and playing it). `--modded` keeps the stage-3 shape: a modded lobby with the zip as its lobby-wide set.
//! DC-DEFAULT: MAP-67 (binaries, ROM, profile folder, ports), MAP-68 (the play instance's server.toml and set),
//! MAP-69 (client launch, the headless pilot, the stop), MAP-89 (the entry shape: the entry document's map
//! index, the pilot's entry pick, the report's `shape`).
//!
//! ```text
//! play::play(dir, &PlayOptions { rom, binaries, headless, seconds, keep_profile, modded }) -> Result<PlayReport, Diagnostics>
//! ```
//!
//! 1. The three binaries are located (`binaries` folder when given, else next to this executable, then on `PATH`,
//!    then `target/debug` and `target/release` of the checkout around the executable or the working directory);
//!    the first missing one is named in a `play.binaries` diagnostic.
//! 2. The ROM is found as every other verb finds it ([`crate::romlist::discover_rom`]; `rom.missing` otherwise).
//! 3. The project is built and exported as a zip ([`crate::export::export`]).
//! 4. `<profile>/mapkit/play/` (the client's own profile folder rule, [`crate::romlist::profile_dir`]) gets
//!    `server.toml` (the [`Shape`]: by default policy vanilla with the entry "Vanilla++" carrying the zip's id /
//!    version / sha256 as its `mods = [...]`; `--modded`: policy modded with the zip as the lobby-wide set;
//!    loopback bind, free ports picked by binding and releasing, db and logs under the folder, accounts
//!    auto-created), `mods/<sha256>.zip`, `data/` and, for a headless run, `client/` as the client's profile
//!    (fresh: its own mod cache). `keep_profile` keeps `data/` and `client/` of the previous run (accounts, the
//!    cached set); the default starts from nothing.
//! 5. The file is checked with dc-config's own `check` (the same code as `dc-config check --fs`); errors end the
//!    run with their text (`play.config`).
//! 6. The lobby starts (`--config`, `--dserver`, `--admin`, `--log`, `--dserver-logdir` into `build/play/`), its
//!    auth port is waited for and the map's index asked for ([`INDEX_WAIT`]). Entry shape: `GET /entry_mods/1.json` on the web port (plain HTTP on loopback) gives
//!    the entry's `community_map_base` and `maps`; the map's index is the base plus the record's position
//!    (`GET /status` `community_maps` is the fallback). Modded shape: `GET /status` on the admin port as before.
//! 7. The client starts (`--lobby 127.0.0.1:<auth> --rom <rom> --menu lobby`); headless adds the menu pilot flags
//!    (`--room new --start-after 1 --mode TDM --pilot-map <index> --duration <seconds>`, and on the entry shape
//!    `--pilot-entry 1` when the client binary's usage names that flag; without it the scripted player enters the
//!    row the server screen picks by itself, which is the one open entry), so it picks the entry, creates a room
//!    on the map, starts it alone and plays `seconds` seconds.
//! 8. Everything stops when the client exits or the time is up: the lobby through `POST /stop` (its graceful stop
//!    ends the match through the game server's pipe), then a termination signal, then a kill; every game server
//!    the lobby log names (`server pid N`) that is still alive is killed too. No shell scripts: `std::process`
//!    throughout, `kill` on unix and `taskkill` on Windows by PID.
//!
//! The logs are `build/play/lobby.log` (the lobby's own log, with the game server's console lines),
//! `build/play/lobby.out` (its stdout + stderr), `build/play/dserver_c<ch>_r<room>_g<n>.log` (per match) and
//! `build/play/client.log` (+ `client.net.jsonl`). The report says which shape ran (`shape`: `entry` or `modded`)
//! and cross-checks the lobby's launch line (`world <path> (community map <key>, announced as
//! @mod:<id>/assets/maps/<key>.dat)`) and, headless, the client's match line (the `@mod:` world, `textures missing
//! 0`, frames rendered, a spawn used).

use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::project::{write_atomic, Project};
use crate::{build, export, romlist};
use serde::Serialize;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlayOptions {
    pub rom: Option<PathBuf>,
    /// Explicit binary folder: the three binaries must be there (no further search).
    pub binaries: Option<PathBuf>,
    /// Headless client driven by the menu pilot (creates a room on the map and plays it) instead of a window.
    pub headless: bool,
    /// Headless: the match length in seconds (the client leaves afterwards; default [`DEFAULT_SECONDS`]).
    /// Window: the whole session is stopped after this long (`None`: when the window closes).
    pub seconds: Option<u64>,
    /// Keep the previous run's accounts and client profile under `<profile>/mapkit/play/`.
    pub keep_profile: bool,
    /// `--modded`: the stage-3 shape (a modded lobby with the zip as its lobby-wide set) instead of the Vanilla++
    /// entry shape ([`Shape`]).
    pub modded: bool,
}

/// The shape of the play instance (MAP-68, MAP-89).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    /// Vanilla++ as the user's server runs it: a vanilla lobby with ONE featured `[[lobby.servers]]` entry
    /// "Vanilla++" (entry id 1) whose curated set (LG-313) is the exported zip; the client loads the set at the
    /// pick (CLI-1643) and the entry's own game servers load the world.
    #[default]
    Entry,
    /// The stage-3 shape (`--modded`): `[mods] policy = "modded"` with the zip as the lobby-wide set.
    Modded,
}

impl Shape {
    pub fn of(opts: &PlayOptions) -> Shape {
        if opts.modded {
            Shape::Modded
        } else {
            Shape::Entry
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Shape::Entry => "entry",
            Shape::Modded => "modded",
        }
    }
}

/// The three binaries a run needs, as found.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Binaries {
    pub lobby: PathBuf,
    pub dserver: PathBuf,
    pub client: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct PlayReport {
    /// Which path ran: `entry` (the Vanilla++ featured entry, the default) or `modded` (`--modded`).
    pub shape: Shape,
    /// Entry shape: the featured entry's id ([`ENTRY_ID`]) and name.
    pub entry: Option<u32>,
    pub entry_name: Option<String>,
    /// Entry shape: the entry document's `community_map_base` (the index of its first map).
    pub community_map_base: Option<u32>,
    /// Entry shape, headless: the client binary took `--pilot-entry <id>` (its usage names the flag); `false`
    /// means the scripted player entered the row the server screen picks by itself (the one open entry).
    pub pilot_entry_flag: bool,
    pub lobby_port: u16,
    pub admin_port: u16,
    pub game_port: u16,
    /// The map's community index on the lobby (`None`: the lobby did not list it).
    pub map_index: Option<u32>,
    pub map_key: String,
    pub mod_id: String,
    pub zip: PathBuf,
    pub sha256: String,
    pub profile: PathBuf,
    pub config: PathBuf,
    pub binaries: Binaries,
    pub rom: PathBuf,
    pub headless: bool,
    /// The client's exit code (`None`: killed when the time was up, or no code).
    pub client_exit: Option<i32>,
    /// `client exit` or `seconds`.
    pub stopped_by: String,
    /// The lobby's launch line of the match (`world ... (community map ..., announced as @mod:...)`).
    pub launch_line: Option<String>,
    /// Headless: the client's match summary lines (`match on ...`).
    pub match_lines: Vec<String>,
    /// Headless: the world the client reports (`@mod:...` when the community map was loaded).
    pub client_world: Option<String>,
    /// Headless: textures the client could not resolve (0 = every `@mod:` texture was found).
    pub textures_missing: Option<u32>,
    /// Headless: frames the client rendered.
    pub frames: Option<u32>,
    /// Headless: the spawn the server placed the client on, as the client reports it.
    pub spawn: Option<String>,
    /// Lobby: the game server reported its end (`@end`) before the stop.
    pub match_ended: bool,
    pub logs: Vec<PathBuf>,
}

/// Headless match length without `--seconds`.
// DC-DEFAULT: MAP-69
pub const DEFAULT_SECONDS: u64 = 20;
/// Headless: the allowance for the menu walk, the world load and the result screen on top of the match.
pub const MENU_ALLOWANCE: Duration = Duration::from_secs(150);
/// Wait for the lobby's auth port.
pub const LOBBY_START: Duration = Duration::from_secs(30);
/// How long the map's index is asked for once the auth port listens (the web and admin ports open just after it).
pub const INDEX_WAIT: Duration = Duration::from_secs(5);
/// The login the headless pilot uses (the lobby auto-creates the account).
pub const PILOT_LOGIN: &str = "mapkit";
/// The room mode the headless pilot picks (the starter template serves it).
pub const PILOT_MODE: &str = "TDM";
/// Entry shape (MAP-89): the featured entry's id (the first `[[lobby.servers]]` entry of a lobby is entry 1,
/// LG-304) and name (the first entry carries the server's own name, so `[server] name` is this too).
pub const ENTRY_ID: u32 = 1;
pub const ENTRY_NAME: &str = "Vanilla++";
/// The entry's capacity in the generated file (dc-config wants one above 0).
pub const ENTRY_CAPACITY: u32 = 16;
/// The client flag that makes the scripted menu player pick a given server entry (the client's own flag; a client
/// whose usage does not name it is started without it and the server screen's own pick enters the one open entry).
pub const PILOT_ENTRY_FLAG: &str = "--pilot-entry";

// ------------------------------------------------------------------------------------------------ binaries

fn exe_name(stem: &str) -> String {
    format!("{stem}{}", std::env::consts::EXE_SUFFIX)
}

/// The folders searched, in order (MAP-67): `explicit` alone when given; else the executable's folder, every
/// `PATH` entry, then `target/debug` and `target/release` under every ancestor of the executable's folder and of
/// the working directory that holds a `Cargo.toml`.
pub fn binary_folders(explicit: Option<&Path>) -> Vec<PathBuf> {
    if let Some(d) = explicit {
        return vec![d.to_path_buf()];
    }
    let mut out = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    if let Some(d) = &exe_dir {
        push(d.clone());
    }
    if let Some(path) = std::env::var_os("PATH") {
        for p in std::env::split_paths(&path) {
            if !p.as_os_str().is_empty() {
                push(p);
            }
        }
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(d) = &exe_dir {
        roots.push(d.clone());
    }
    if let Ok(d) = std::env::current_dir() {
        roots.push(d);
    }
    for root in roots {
        for a in root.ancestors() {
            if a.join("Cargo.toml").is_file() {
                push(a.join("target").join("debug"));
                push(a.join("target").join("release"));
            }
        }
    }
    out
}

/// Finds `dc-lobby-server`, `dc-dserver` and `dc-client` (MAP-67); the first missing one is the diagnostic.
pub fn find_binaries(explicit: Option<&Path>) -> Result<Binaries, Diagnostics> {
    Ok(Binaries {
        lobby: find_binary(explicit, "dc-lobby-server")?,
        dserver: find_binary(explicit, "dc-dserver")?,
        client: find_binary(explicit, "dc-client")?,
    })
}

/// One game binary by its stem, with the same search and the same diagnostic as [`find_binaries`].
pub fn find_binary(explicit: Option<&Path>, stem: &str) -> Result<PathBuf, Diagnostics> {
    let folders = binary_folders(explicit);
    let name = exe_name(stem);
    folders
        .iter()
        .map(|f| f.join(&name))
        .find(|p| p.is_file())
        // absolute: the lobby and the client run in folders of their own
        .map(|p| std::path::absolute(&p).unwrap_or(p))
        .ok_or_else(|| {
            let tried: Vec<String> = folders.iter().map(|f| f.display().to_string()).collect();
            Diagnostics::one(
                Diagnostic::new(
                    Code::PlayBinaries,
                    stem,
                    format!("`{name}` was not found in: {}", tried.join(", ")),
                )
                .with_fix(
                    "build it (cargo build -p dc-lobby-server -p dc-dserver -p dc-client), put the three binaries \
                     next to dc-mapkit, or give their folder with --binaries",
                ),
            )
        })
}

// ------------------------------------------------------------------------------------------------ ports

fn tcp_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn udp_free(port: u16) -> bool {
    UdpSocket::bind(("127.0.0.1", port)).is_ok()
}

fn free_tcp() -> Option<u16> {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .ok()
}

fn free_udp() -> Option<u16> {
    UdpSocket::bind("127.0.0.1:0")
        .and_then(|s| s.local_addr())
        .map(|a| a.port())
        .ok()
}

/// The ports of one play instance (MAP-67): auth `a` with `a + 2` (lobby), `a + 3` (clan) and `a + 8080` (web:
/// the client derives the mod download port from the auth port) free, an admin port, a UDP game port with the
/// next [`GAME_RANGE`] ports free for further rooms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ports {
    pub auth: u16,
    pub admin: u16,
    pub game: u16,
}

pub const GAME_RANGE: u16 = 16;

pub fn pick_ports() -> Option<Ports> {
    // The auth port is scanned from a clock-seeded start in 20000..41100, below the dynamic range: the OS hands out
    // dynamic ports in order (Windows from 49152), so one above 57455 leaves no room for `+ 8080` and a drawn one
    // failed about a third of the time.
    let mut auth = None;
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos())
        ^ std::process::id();
    let start = (seed % 5000) as u16;
    for k in 0..256u16 {
        let a = 20_000 + (start + k) * 4;
        if tcp_free(a) && tcp_free(a + 2) && tcp_free(a + 3) && tcp_free(a + 8080) {
            auth = Some(a);
            break;
        }
    }
    let auth = auth?;
    let mut admin = None;
    for _ in 0..64 {
        let p = free_tcp()?;
        if p != auth && p != auth + 2 && p != auth + 3 && p != auth + 8080 {
            admin = Some(p);
            break;
        }
    }
    let mut game = None;
    for _ in 0..64 {
        let g = free_udp()?;
        if g.checked_add(GAME_RANGE)
            .is_some_and(|hi| (g + 1..=hi).all(udp_free))
        {
            game = Some(g);
            break;
        }
    }
    Some(Ports {
        auth,
        admin: admin?,
        game: game?,
    })
}

// ------------------------------------------------------------------------------------------------ server.toml

/// A TOML basic string of a path: forward slashes (std::path takes them on Windows too), quotes escaped.
pub fn toml_str(p: &Path) -> String {
    let s = p.display().to_string().replace('\\', "/");
    format!("\"{}\"", s.replace('"', "\\\""))
}

/// The play instance's `server.toml` (MAP-68; the entry shape MAP-89). `name` is the server's name: on the entry
/// shape it is [`ENTRY_NAME`] (the first entry of a lobby carries the server's name, LG-304) and `name` becomes
/// part of the comment only.
#[allow(clippy::too_many_arguments)]
pub fn server_toml(
    shape: Shape,
    ports: Ports,
    rom: &Path,
    play_dir: &Path,
    mod_id: &str,
    version: &str,
    sha256: &str,
    name: &str,
) -> String {
    let name = name.replace('"', "'");
    let (server_name, head, set) = match shape {
        Shape::Entry => (
            ENTRY_NAME.to_string(),
            format!(
                "# dc-mapkit play: Vanilla++ on this machine for `{name}` (DC_DEFAULTS.md MAP-68, MAP-89): a vanilla lobby\n\
                 # whose one featured entry \"{ENTRY_NAME}\" (entry 1) carries the exported map as its curated set (LG-313).\n\
                 # Generated; the next `dc-mapkit play` writes it again."
            ),
            format!(
                "[mods]\n\
                 policy = \"vanilla\"\n\
                 \n\
                 # The featured entry: the first entry of the lobby carries the server's name above; its curated set is the\n\
                 # exported zip in mods_dir as <sha256>.zip (the client loads it at the pick, CLI-1643; the entry's game\n\
                 # servers load the world).\n\
                 [[lobby.servers]]\n\
                 name = \"{ENTRY_NAME}\"\n\
                 capacity = {ENTRY_CAPACITY}\n\
                 mods = [\n\
                 \x20 {{ id = \"{mod_id}\", version = \"{version}\", sha256 = \"{sha256}\" }},\n\
                 ]\n"
            ),
        ),
        Shape::Modded => (
            format!("{name} (dc-mapkit play)"),
            "# dc-mapkit play --modded: a one-mod modded instance on this machine (DC_DEFAULTS.md MAP-68). Generated; the\n\
             # next `dc-mapkit play` writes it again."
                .to_string(),
            format!(
                "[mods]\n\
                 policy = \"modded\"\n\
                 \n\
                 [[mods.set]]\n\
                 id = \"{mod_id}\"\n\
                 version = \"{version}\"\n\
                 sha256 = \"{sha256}\"\n"
            ),
        ),
    };
    format!(
        "{head}\n\
         [server]\n\
         name = \"{server_name}\"\n\
         bind = \"127.0.0.1\"\n\
         public = false\n\
         advertise_ip = \"127.0.0.1\"\n\
         auth_port = {auth}\n\
         lobby_port = {lobby}\n\
         clan_port = {clan}\n\
         http_port = {web}\n\
         http_enabled = true\n\
         game_port = {game}\n\
         \n\
         [paths]\n\
         rom = {rom}\n\
         db = {db}\n\
         log_dir = {logs}\n\
         mods_dir = {mods}\n\
         \n\
         [dserver]\n\
         port_range = \"{glo}-{ghi}\"\n\
         tick_hz = 30\n\
         \n\
         [auth]\n\
         auto_create = true\n\
         \n\
         {set}",
        auth = ports.auth,
        lobby = ports.auth + 2,
        clan = ports.auth + 3,
        web = ports.auth + 8080,
        game = ports.game,
        rom = toml_str(rom),
        db = toml_str(&play_dir.join("data").join("ca.db")),
        logs = toml_str(&play_dir.join("logs")),
        mods = toml_str(&play_dir.join("mods")),
        glo = ports.game + 1,
        ghi = ports.game + GAME_RANGE,
    )
}

/// dc-config's own check of the file (the code behind `dc-config check --fs`); errors are `play.config`.
pub fn check_config(text: &str, path: &Path) -> Result<Vec<String>, Diagnostics> {
    let cfg = dc_config::parse_toml(text).map_err(|issues| {
        Diagnostics::one(Diagnostic::new(
            Code::PlayConfig,
            path.display().to_string(),
            issues
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        ))
    })?;
    let issues = dc_config::check(&cfg, dc_config::CheckOptions { fs: true });
    if dc_config::has_errors(&issues) {
        return Err(Diagnostics::one(Diagnostic::new(
            Code::PlayConfig,
            path.display().to_string(),
            issues
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        )));
    }
    Ok(issues.iter().map(ToString::to_string).collect())
}

// ------------------------------------------------------------------------------------------------ processes

fn io_diag(code: Code, path: &Path, e: &dyn std::fmt::Display) -> Diagnostics {
    Diagnostics::one(Diagnostic::new(
        code,
        path.display().to_string(),
        e.to_string(),
    ))
}

/// One child with its PID; killed when dropped if still running.
struct Proc {
    child: Option<Child>,
    name: &'static str,
}

impl Proc {
    fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    fn running(&mut self) -> bool {
        match self.child.as_mut() {
            Some(c) => matches!(c.try_wait(), Ok(None)),
            None => false,
        }
    }

    fn exit_code(&mut self) -> Option<i32> {
        self.child
            .as_mut()
            .and_then(|c| c.try_wait().ok().flatten())
            .and_then(|s| s.code())
    }

    /// Waits up to `t` for the process to end by itself.
    fn wait_exit(&mut self, t: Duration) -> bool {
        let end = Instant::now() + t;
        while Instant::now() < end {
            if !self.running() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        !self.running()
    }

    /// Ends the process: a termination signal, up to `grace` to leave, then a kill (MAP-69).
    fn stop(&mut self, grace: Duration) {
        if !self.running() {
            return;
        }
        if let Some(pid) = self.pid() {
            terminate_pid(pid);
        }
        if !self.wait_exit(grace) {
            if let Some(c) = self.child.as_mut() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        self.stop(Duration::from_secs(3));
    }
}

/// A polite stop by PID: `kill -TERM` on unix, `taskkill /PID` (the process tree, no force) on Windows.
pub fn terminate_pid(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// A hard stop by PID (`kill -9` / `taskkill /F /T`).
pub fn kill_pid(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill")
            .args(["-9", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// True while a process with this PID runs (`kill -0` / `tasklist`).
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")))
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// PIDs of the game servers a lobby log reports starting (`... server pid N port ...`).
pub fn logged_pids(log: &str) -> Vec<u32> {
    log.lines()
        .filter_map(|l| {
            let i = l.find(" server pid ")?;
            l[i + 12..].split_whitespace().next()?.parse().ok()
        })
        .collect()
}

/// The lobby's launch line of a community map match.
pub fn launch_line(log: &str) -> Option<String> {
    log.lines()
        .find(|l| l.contains("announced as @mod:"))
        .map(str::to_string)
}

/// One HTTP/1.1 request on loopback; the body of the answer (`None` when nothing answered).
pub fn http(port: u16, method: &str, path: &str, timeout: Duration) -> Option<String> {
    let mut s =
        TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], port)), timeout)
            .ok()?;
    let _ = s.set_read_timeout(Some(timeout));
    let _ = s.set_write_timeout(Some(timeout));
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    let body = match out.find("\r\n\r\n") {
        Some(i) => out[i + 4..].to_string(),
        None => out,
    };
    Some(body)
}

/// The map's community index from the lobby's `GET /status` answer (`community_maps[].{key, index}`).
pub fn map_index_of(status: &str, key: &str) -> Option<u32> {
    let v: serde_json::Value = serde_json::from_str(status).ok()?;
    let maps = v.get("community_maps")?.as_array()?;
    maps.iter()
        .find(|m| m.get("key").and_then(|k| k.as_str()) == Some(key))
        .or_else(|| maps.first())
        .and_then(|m| m.get("index"))
        .and_then(|i| i.as_u64())
        .map(|i| i as u32)
}

/// What the entry document (`GET /entry_mods/<entry>.json`, LG-313 with the maps of the 2026-11-01 contract) says
/// about the entry's maps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryMaps {
    pub name: Option<String>,
    /// `community_map_base`: the table index of `maps[0]`.
    pub base: u32,
    /// The map keys (`maps[].id`) in table order.
    pub keys: Vec<String>,
}

impl EntryMaps {
    /// The index of the map with this key: the base plus its position (the first map when the key is absent).
    pub fn index_of(&self, key: &str) -> Option<u32> {
        if self.keys.is_empty() {
            return None;
        }
        let pos = self.keys.iter().position(|k| k == key).unwrap_or(0);
        Some(self.base + pos as u32)
    }
}

/// Reads the entry document (MAP-89): `None` when it is not JSON, has no `community_map_base` or no `maps` (a lobby
/// that does not offer the entry's maps yet, or an entry whose set holds no map).
pub fn entry_maps_of(doc: &str) -> Option<EntryMaps> {
    let v: serde_json::Value = serde_json::from_str(doc).ok()?;
    let base = v.get("community_map_base")?.as_u64()? as u32;
    let keys = v
        .get("maps")?
        .as_array()?
        .iter()
        .filter_map(|m| m.get("id").and_then(|k| k.as_str()).map(str::to_string))
        .collect();
    Some(EntryMaps {
        name: v.get("name").and_then(|n| n.as_str()).map(str::to_string),
        base,
        keys,
    })
}

/// True when a client's usage text names `flag` (the client slice's own flags are probed, not assumed, so an older
/// client is started without them instead of refusing an unknown argument).
pub fn client_takes_flag(usage: &str, flag: &str) -> bool {
    usage.contains(flag)
}

/// The client's usage text (`--help` prints it and leaves; stdout and stderr both read).
fn client_usage(client: &Path) -> String {
    Command::new(client)
        .arg("--help")
        .stdin(Stdio::null())
        .output()
        .map(|o| {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            s
        })
        .unwrap_or_default()
}

/// The number after `label ` in a text (`textures missing 0` -> 0).
fn number_after(text: &str, label: &str) -> Option<u32> {
    let i = text.find(label)? + label.len();
    text[i..]
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// What the headless client's log says about the match (MAP-69).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientSummary {
    /// Each match's summary block (`match on ...` and its indented continuation lines), joined with ` | `.
    pub match_lines: Vec<String>,
    pub world: Option<String>,
    pub textures_missing: Option<u32>,
    pub frames: Option<u32>,
    pub spawn: Option<String>,
}

/// Reads the client's `match on <server>: <world line>` block: the line itself and the indented lines after it
/// up to the menu's closing line (`N menu PNGs in ...`) or a box line.
pub fn client_summary(log: &str) -> ClientSummary {
    let mut s = ClientSummary::default();
    let lines: Vec<&str> = log.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if !lines[i].contains("match on ") {
            i += 1;
            continue;
        }
        let mut block: Vec<String> = vec![lines[i].trim().to_string()];
        i += 1;
        while i < lines.len() {
            let l = lines[i];
            let t = l.trim();
            if !l.starts_with(' ')
                || t.is_empty()
                || t.contains("menu PNGs in")
                || t.starts_with("box:")
                || t.contains("match on ")
            {
                break;
            }
            block.push(t.to_string());
            i += 1;
        }
        for t in &block {
            if s.textures_missing.is_none() {
                s.textures_missing = number_after(t, "textures missing ");
            }
            if let Some(rest) = t.strip_prefix("frames ") {
                if s.frames.is_none() {
                    s.frames = number_after(rest, "");
                }
                if let Some(j) = rest.find("spawn Some(") {
                    let r = &rest[j + 11..];
                    s.spawn = r.find("))").map(|k| r[..k + 1].to_string());
                }
            }
            if s.world.is_none() {
                if let Some(j) = t.find("world Some(\"") {
                    let r = &t[j + 12..];
                    s.world = r.find('"').map(|k| r[..k].to_string());
                }
            }
        }
        s.match_lines.push(block.join(" | "));
    }
    s
}

fn read_text(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

fn tail(p: &Path, lines: usize) -> String {
    let t = read_text(p);
    let all: Vec<&str> = t.lines().collect();
    let from = all.len().saturating_sub(lines);
    all[from..].join("\n")
}

fn log_file(p: &Path) -> Result<std::fs::File, Diagnostics> {
    std::fs::File::create(p).map_err(|e| io_diag(Code::ProjectIo, p, &e))
}

// ------------------------------------------------------------------------------------------------ the verb

/// Vanilla++ on this machine: see the module doc.
pub fn play(dir: &Path, opts: &PlayOptions) -> Result<PlayReport, Diagnostics> {
    // 1. binaries (cheap, no side effects: first)
    let binaries = find_binaries(opts.binaries.as_deref())?;
    // 2. ROM
    let (rom, _source) = romlist::discover_rom(opts.rom.as_deref()).ok_or_else(romlist::no_rom)?;
    let rom = std::path::absolute(&rom).unwrap_or(rom);
    // 3. export
    let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    let (p, h) = Project::open(&dir)?;
    let built = build::build(&p, &h, &build::Assets::new(&dir), &p.build)?;
    let e = export::export(
        &p,
        &built,
        &dir.join("build"),
        export::ExportOptions {
            zip: true,
            no_rom_paths: false,
            project_dir: Some(dir.clone()),
            force: false,
        },
    )?;
    let (Some(zip), Some(sha256)) = (e.zip.clone(), e.sha256.clone()) else {
        return Err(Diagnostics::one(Diagnostic::new(
            Code::ExportPack,
            "export",
            "the export produced no zip",
        )));
    };
    // 4. the play folder under the profile
    let out = dir.join("build").join("play");
    let play_dir = romlist::profile_dir()
        .map(|d| d.join("mapkit").join("play"))
        .unwrap_or_else(|| out.join("profile"));
    if !opts.keep_profile {
        for sub in ["data", "client"] {
            let _ = std::fs::remove_dir_all(play_dir.join(sub));
        }
    }
    for sub in ["data", "logs", "mods", "client"] {
        let d = play_dir.join(sub);
        std::fs::create_dir_all(&d).map_err(|e| io_diag(Code::ProjectIo, &d, &e))?;
    }
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).map_err(|e| io_diag(Code::ProjectIo, &out, &e))?;
    let zip_copy = play_dir.join("mods").join(format!("{sha256}.zip"));
    std::fs::copy(&zip, &zip_copy).map_err(|e| io_diag(Code::ProjectIo, &zip_copy, &e))?;
    let ports = pick_ports().ok_or_else(|| {
        Diagnostics::one(Diagnostic::new(
            Code::PlayConfig,
            "ports",
            "no free loopback ports for the lobby (auth, auth + 2, auth + 3, auth + 8080), the admin endpoint and the game servers",
        ))
    })?;
    let shape = Shape::of(opts);
    let config = play_dir.join("server.toml");
    let text = server_toml(
        shape,
        ports,
        &rom,
        &play_dir,
        &p.mod_meta.id,
        &p.mod_meta.version,
        &sha256,
        &p.name,
    );
    write_atomic(&config, text.as_bytes()).map_err(|e| io_diag(Code::ProjectIo, &config, &e))?;
    // 5. the config check
    let mut diags = Diagnostics::new();
    for w in check_config(&text, &config)? {
        diags.warn(Code::PlayConfig, config.display().to_string(), w);
    }
    // 6. the lobby
    let lobby_log = out.join("lobby.log");
    let lobby_out = out.join("lobby.out");
    let mut cmd = Command::new(&binaries.lobby);
    cmd.arg("--config")
        .arg(&config)
        .arg("--dserver")
        .arg("--dserver-exe")
        .arg(&binaries.dserver)
        .arg("--dserver-logdir")
        .arg(&out)
        .arg("--dserver-verbose")
        .args(["--dserver-idle", "10"])
        .arg("--admin")
        .arg(format!("127.0.0.1:{}", ports.admin))
        .arg("--log")
        .arg(&lobby_log)
        .arg("--autochar")
        .args(["--stop-grace", "10"])
        .env("CA_ROM", &rom)
        .current_dir(&play_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file(&lobby_out)?))
        .stderr(Stdio::from(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&lobby_out)
                .map_err(|e| io_diag(Code::ProjectIo, &lobby_out, &e))?,
        ));
    let mut lobby = Proc {
        child: Some(cmd.spawn().map_err(|e| {
            Diagnostics::one(Diagnostic::new(
                Code::PlayLobby,
                binaries.lobby.display().to_string(),
                format!("cannot start the lobby: {e}"),
            ))
        })?),
        name: "dc-lobby-server",
    };
    let up = {
        let end = Instant::now() + LOBBY_START;
        loop {
            if TcpStream::connect(("127.0.0.1", ports.auth)).is_ok() {
                break true;
            }
            if !lobby.running() || Instant::now() > end {
                break false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    if !up {
        let why = format!(
            "the lobby did not listen on 127.0.0.1:{} within {} s (exit {:?}); {}:\n{}\n{}:\n{}",
            ports.auth,
            LOBBY_START.as_secs(),
            lobby.exit_code(),
            lobby_log.display(),
            tail(&lobby_log, 12),
            lobby_out.display(),
            tail(&lobby_out, 12)
        );
        lobby.stop(Duration::from_secs(3));
        return Err(Diagnostics::one(Diagnostic::new(
            Code::PlayLobby,
            lobby.name,
            why,
        )));
    }
    // the map's index: entry shape from the entry document (MAP-89), else / fallback from `GET /status` (MAP-68). The
    // web and admin listeners open just after the auth port, so a read is repeated for a moment
    let index_end = Instant::now() + INDEX_WAIT;
    let (status, entry_doc, entry_maps, map_index) = loop {
        let status =
            http(ports.admin, "GET", "/status", Duration::from_secs(5)).unwrap_or_default();
        let entry_doc = match shape {
            Shape::Entry => http(
                ports.auth + 8080,
                "GET",
                &format!("/entry_mods/{ENTRY_ID}.json"),
                Duration::from_secs(5),
            )
            .unwrap_or_default(),
            Shape::Modded => String::new(),
        };
        let entry_maps = entry_maps_of(&entry_doc);
        let map_index = entry_maps
            .as_ref()
            .and_then(|m| m.index_of(&p.key))
            .or_else(|| map_index_of(&status, &p.key));
        if map_index.is_some() || Instant::now() >= index_end {
            break (status, entry_doc, entry_maps, map_index);
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    if map_index.is_none() {
        let why = match shape {
            Shape::Entry => format!(
                "the lobby does not offer the map `{}` on the entry {ENTRY_ID} \"{ENTRY_NAME}\" (GET /entry_mods/{ENTRY_ID}.json: {}; GET /status community_maps: {}); {}:\n{}",
                p.key,
                entry_doc.trim(),
                status.trim(),
                lobby_log.display(),
                tail(&lobby_log, 12)
            ),
            Shape::Modded => format!(
                "the lobby does not list the map `{}` under community_maps (GET /status: {}); {}:\n{}",
                p.key,
                status.trim(),
                lobby_log.display(),
                tail(&lobby_log, 12)
            ),
        };
        let _ = http(ports.admin, "POST", "/stop", Duration::from_secs(3));
        lobby.stop(Duration::from_secs(10));
        return Err(Diagnostics::one(Diagnostic::new(
            Code::PlayLobby,
            lobby.name,
            why,
        )));
    }
    // the client's own pilot flag for the entry pick is probed, never assumed (an unknown argument stops the client)
    let pilot_entry_flag = shape == Shape::Entry
        && opts.headless
        && client_takes_flag(&client_usage(&binaries.client), PILOT_ENTRY_FLAG);
    // 7. the client
    let client_log = out.join("client.log");
    let seconds = opts.seconds.unwrap_or(DEFAULT_SECONDS);
    let mut cmd = Command::new(&binaries.client);
    cmd.arg("--lobby")
        .arg(format!("127.0.0.1:{}", ports.auth))
        .arg("--rom")
        .arg(&rom)
        .args(["--menu", "lobby"])
        .arg("--net-log")
        .arg(out.join("client.net.jsonl"))
        .env("CA_ROM", &rom)
        .current_dir(&out)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file(&client_log)?))
        .stderr(Stdio::from(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&client_log)
                .map_err(|e| io_diag(Code::ProjectIo, &client_log, &e))?,
        ));
    if opts.headless {
        cmd.args(["--name", PILOT_LOGIN, "--login-password", PILOT_LOGIN])
            .args(["--identity", "off"])
            .args(["--room", "new", "--start-after", "1", "--mode", PILOT_MODE])
            .arg("--title")
            .arg(&p.name)
            .args(["--pilot-map", &map_index.unwrap_or(0).to_string()])
            .args(["--duration", &seconds.to_string()]);
        if pilot_entry_flag {
            cmd.args([PILOT_ENTRY_FLAG, &ENTRY_ID.to_string()]);
        }
        cmd.args(["--match-wait", "60", "--load-timeout", "90"])
            // one picture per screen and one per state, not one per frame (a PNG per frame runs the match
            // many times slower than the wall clock)
            .args(["--save-every", "1000000", "--size", "640x480"])
            .arg("--out")
            .arg(out.join("shots"))
            .arg("--no-sound")
            .env("DC_PROFILE_DIR", play_dir.join("client"));
    }
    let mut client = Proc {
        child: Some(cmd.spawn().map_err(|e| {
            let _ = http(ports.admin, "POST", "/stop", Duration::from_secs(3));
            lobby.stop(Duration::from_secs(10));
            Diagnostics::one(Diagnostic::new(
                Code::PlayClient,
                binaries.client.display().to_string(),
                format!("cannot start the client: {e}"),
            ))
        })?),
        name: "dc-client",
    };
    // 8. wait, then stop everything
    let deadline = match (opts.headless, opts.seconds) {
        (true, _) => Some(Duration::from_secs(seconds) + MENU_ALLOWANCE),
        (false, Some(s)) => Some(Duration::from_secs(s)),
        (false, None) => None,
    };
    let start = Instant::now();
    let stopped_by = loop {
        if !client.running() {
            break "client exit";
        }
        if deadline.is_some_and(|d| start.elapsed() > d) {
            break "seconds";
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let client_exit = client.exit_code();
    client.stop(Duration::from_secs(5));
    // the graceful stop ends the match through the game server's pipe; the log then carries `@end`
    let _ = http(ports.admin, "POST", "/stop", Duration::from_secs(3));
    lobby.wait_exit(Duration::from_secs(15));
    lobby.stop(Duration::from_secs(5));
    for pid in logged_pids(&read_text(&lobby_log)) {
        if pid_alive(pid) {
            terminate_pid(pid);
            std::thread::sleep(Duration::from_millis(300));
            if pid_alive(pid) {
                kill_pid(pid);
            }
        }
    }
    let lobby_text = read_text(&lobby_log);
    let client_text = read_text(&client_log);
    let summary = client_summary(&client_text);
    let mut logs = vec![lobby_log.clone(), lobby_out.clone(), client_log.clone()];
    if let Ok(rd) = std::fs::read_dir(&out) {
        let mut ds: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().starts_with("dserver_"))
                    .unwrap_or(false)
            })
            .collect();
        ds.sort();
        logs.extend(ds);
    }
    logs.push(out.join("client.net.jsonl"));
    let report = PlayReport {
        shape,
        entry: (shape == Shape::Entry).then_some(ENTRY_ID),
        entry_name: match shape {
            Shape::Entry => Some(
                entry_maps
                    .as_ref()
                    .and_then(|m| m.name.clone())
                    .unwrap_or_else(|| ENTRY_NAME.to_string()),
            ),
            Shape::Modded => None,
        },
        community_map_base: entry_maps.as_ref().map(|m| m.base),
        pilot_entry_flag,
        lobby_port: ports.auth,
        admin_port: ports.admin,
        game_port: ports.game,
        map_index,
        map_key: p.key.clone(),
        mod_id: p.mod_meta.id.clone(),
        zip,
        sha256,
        profile: play_dir,
        config,
        binaries,
        rom,
        headless: opts.headless,
        client_exit,
        stopped_by: stopped_by.to_string(),
        launch_line: launch_line(&lobby_text),
        match_lines: summary.match_lines,
        client_world: summary.world,
        textures_missing: summary.textures_missing,
        frames: summary.frames,
        spawn: summary.spawn,
        match_ended: lobby_text.lines().any(|l| l.contains(": @end")),
        logs,
    };
    // headless: the run is judged (MAP-69); a window run is the user's
    if opts.headless {
        if report.launch_line.is_none() {
            diags.warn(
                Code::PlayLobby,
                lobby_log.display().to_string(),
                "no match was started on the community map (no `announced as @mod:` launch line)",
            );
        }
        if report.match_lines.is_empty() {
            diags.error(
                Code::PlayClient,
                client_log.display().to_string(),
                format!(
                    "the headless client played no match (exit {client_exit:?}, stopped by {stopped_by}); last lines:\n{}",
                    tail(&client_log, 15)
                ),
            );
        } else {
            if !report
                .client_world
                .as_deref()
                .is_some_and(|w| w.starts_with("@mod:"))
            {
                diags.error(
                    Code::PlayClient,
                    client_log.display().to_string(),
                    format!(
                        "the client did not load the `@mod:` world: {}",
                        report.match_lines.join(" | ")
                    ),
                );
            }
            if report.textures_missing.is_some_and(|n| n > 0) {
                diags.warn(
                    Code::PlayClient,
                    client_log.display().to_string(),
                    format!(
                        "{} texture(s) missing on the client",
                        report.textures_missing.unwrap_or(0)
                    ),
                );
            }
        }
    }
    if diags.has_errors() {
        return Err(diags);
    }
    let _ = diags;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_are_free_and_derivable() {
        let p = pick_ports().expect("ports");
        assert!(tcp_free(p.auth) && tcp_free(p.auth + 2) && tcp_free(p.auth + 3));
        assert!(tcp_free(p.auth + 8080));
        assert!(udp_free(p.game) && (p.game + 1..=p.game + GAME_RANGE).all(udp_free));
        assert_ne!(p.admin, p.auth);
    }

    #[test]
    fn the_server_toml_checks_clean_and_carries_the_set() {
        let tmp = std::env::temp_dir().join(format!("mapkit-play-toml-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let rom = tmp.join("rom");
        for d in ["data", "logs", "mods"] {
            std::fs::create_dir_all(tmp.join(d)).unwrap();
        }
        std::fs::create_dir_all(rom.join("Game")).unwrap();
        let sha = "ab".repeat(32);
        std::fs::write(tmp.join("mods").join(format!("{sha}.zip")), b"zip").unwrap();
        let ports = Ports {
            auth: 31000,
            admin: 31050,
            game: 31100,
        };
        // the default shape (MAP-89): a vanilla lobby, the entry "Vanilla++" with the zip as its curated set
        let text = server_toml(
            Shape::Entry,
            ports,
            &rom,
            &tmp,
            "starter_map",
            "0.1.0",
            &sha,
            "Play \"x\"",
        );
        assert!(text.contains("policy = \"vanilla\""), "{text}");
        assert!(text.contains("[[lobby.servers]]"), "{text}");
        assert!(text.contains(&format!("name = \"{ENTRY_NAME}\"")), "{text}");
        assert!(text.contains("lobby_port = 31002") && text.contains("http_port = 39080"));
        assert!(text.contains("port_range = \"31101-31116\""));
        assert!(!text.contains('\\'), "forward slashes only:\n{text}");
        let cfg = dc_config::parse_toml(&text).expect("parses");
        assert_eq!(cfg.mods.policy, "vanilla");
        assert!(cfg.mods.set.is_empty());
        assert_eq!(
            cfg.server.name, ENTRY_NAME,
            "the first entry carries the server's name"
        );
        assert_eq!(cfg.lobby.servers.len(), 1);
        assert_eq!(cfg.lobby.servers[0].name, ENTRY_NAME);
        assert_eq!(cfg.lobby.servers[0].mods.len(), 1);
        assert_eq!(cfg.lobby.servers[0].mods[0].id, "starter_map");
        assert_eq!(cfg.lobby.servers[0].mods[0].version, "0.1.0");
        assert_eq!(cfg.lobby.servers[0].mods[0].sha256, sha);
        assert_eq!(cfg.server.auth_port, 31000);
        // the fs check sees the ROM folder, the mods dir with the zip and the db folder
        let warnings = check_config(&text, &tmp.join("server.toml")).expect("no error");
        println!("warnings: {warnings:?}");
        // `--modded`: the stage-3 shape, the zip as the lobby-wide set
        let text = server_toml(
            Shape::Modded,
            ports,
            &rom,
            &tmp,
            "starter_map",
            "0.1.0",
            &sha,
            "Play \"x\"",
        );
        assert!(text.contains("policy = \"modded\""));
        assert!(text.contains("id = \"starter_map\""));
        assert!(!text.contains("[[lobby.servers]]"));
        assert!(!text.contains('\\'), "forward slashes only:\n{text}");
        let cfg = dc_config::parse_toml(&text).expect("parses");
        assert_eq!(cfg.mods.policy, "modded");
        assert_eq!(cfg.mods.set.len(), 1);
        assert_eq!(cfg.mods.set[0].sha256, sha);
        assert_eq!(cfg.server.name, "Play 'x' (dc-mapkit play)");
        assert!(cfg.lobby.servers.is_empty());
        let warnings = check_config(&text, &tmp.join("server.toml")).expect("no error");
        println!("warnings: {warnings:?}");
        // a broken file is refused with dc-config's text
        let e = check_config("[server]\nbind = \"nowhere\"\n", &tmp.join("x.toml")).unwrap_err();
        assert_eq!(e.items[0].code, Code::PlayConfig);
        assert!(
            e.items[0].message.contains("bind"),
            "{}",
            e.items[0].message
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn log_readers() {
        let lobby = "x\ngame 1: room 1-1 server pid 4242 port 27889 world /a/b.dat (community map starter_map, announced as @mod:starter_map/assets/maps/starter_map.dat)\ngame 1: @end\n";
        assert_eq!(logged_pids(lobby), vec![4242]);
        assert!(launch_line(lobby)
            .unwrap()
            .contains("announced as @mod:starter_map"));
        let status = r#"{"community_maps": [{"key": "other", "index": 21}, {"key": "starter_map", "index": 22}]}"#;
        assert_eq!(map_index_of(status, "starter_map"), Some(22));
        assert_eq!(map_index_of(status, "none"), Some(21));
        assert_eq!(map_index_of(r#"{"community_maps": []}"#, "x"), None);
        assert_eq!(map_index_of("nonsense", "x"), None);
        // the entry document (MAP-89): base + the record's position; the first record for an unknown key
        let doc = r#"{"entry": 1, "name": "Vanilla++", "curated": true, "community_map_base": 19,
            "maps": [{"id": "other", "mod_id": "other", "world": "@mod:other/assets/maps/other.dat"},
                     {"id": "starter_map", "mod_id": "starter_map", "world": "@mod:starter_map/assets/maps/starter_map.dat"}]}"#;
        let m = entry_maps_of(doc).expect("the entry's maps");
        assert_eq!(m.name.as_deref(), Some("Vanilla++"));
        assert_eq!(m.base, 19);
        assert_eq!(m.keys, vec!["other".to_string(), "starter_map".to_string()]);
        assert_eq!(m.index_of("starter_map"), Some(20));
        assert_eq!(m.index_of("none"), Some(19));
        let empty = entry_maps_of(r#"{"community_map_base": 19, "maps": []}"#).unwrap();
        assert_eq!(empty.index_of("starter_map"), None);
        // a document without the maps (a lobby before the entry maps, a set with no map) or without a base
        assert_eq!(
            entry_maps_of(r#"{"entry": 1, "curated": true, "mod_set": {}}"#),
            None
        );
        assert_eq!(entry_maps_of(r#"{"maps": []}"#), None);
        assert_eq!(entry_maps_of("This server entry has no mod set."), None);
        // the report says which path ran
        let report = |shape| {
            serde_json::to_value(PlayReport {
                shape,
                ..PlayReport::default()
            })
            .unwrap()
        };
        assert_eq!(report(Shape::Entry)["shape"], "entry");
        assert_eq!(report(Shape::Modded)["shape"], "modded");
        assert_eq!(
            Shape::of(&PlayOptions {
                modded: true,
                ..PlayOptions::default()
            }),
            Shape::Modded
        );
        assert_eq!(Shape::of(&PlayOptions::default()), Shape::Entry);
        // the client's pilot entry flag is probed in its usage text
        assert!(client_takes_flag(
            "usage: dc-client [--pilot-map NAME|N] [--pilot-entry ID]",
            PILOT_ENTRY_FLAG
        ));
        assert!(!client_takes_flag(
            "usage: dc-client [--pilot-map NAME|N]",
            PILOT_ENTRY_FLAG
        ));
        let client = "dc-client menu: screens login -> servers -> room -> match -> back -> room\n  \
match on 127.0.0.1:27889: @mod:starter_map/assets/maps/starter_map.dat: 4 sections drawn, textures missing 0, load 12 ms\n  \
adapter metal | font rom\n  \
frames 300 ticks 240 spawn Some(([1.0, 2.0, 3.0], 1.5))\n  \
nan ticks 0 nan frames 0 tunnel ticks 0 fell out false | images 2\n               \
models: instances drawn 1 own weapon frames 1 fire frames 0  | world Some(\"@mod:starter_map/assets/maps/starter_map.dat\") | updates 100 | end Some(\"quit\")\n  \
8 menu PNGs in /x (ROM-derived, never commit); end: matches played\n";
        let s = client_summary(client);
        assert_eq!(s.match_lines.len(), 1);
        assert!(
            s.match_lines[0].starts_with("match on 127.0.0.1:27889")
                && s.match_lines[0].contains("end Some")
        );
        assert!(!s.match_lines[0].contains("menu PNGs"));
        assert_eq!(s.textures_missing, Some(0));
        assert_eq!(
            s.frames,
            Some(300),
            "the frames line, not `nan frames` or `fire frames`"
        );
        assert_eq!(
            s.world.as_deref(),
            Some("@mod:starter_map/assets/maps/starter_map.dat")
        );
        assert_eq!(s.spawn.as_deref(), Some("([1.0, 2.0, 3.0], 1.5)"));
        assert_eq!(client_summary("nothing\n"), ClientSummary::default());
    }

    #[test]
    fn an_explicit_binary_folder_is_the_only_place_searched() {
        let tmp = std::env::temp_dir().join(format!("mapkit-play-bins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        assert_eq!(binary_folders(Some(&tmp)), vec![tmp.clone()]);
        let e = find_binaries(Some(&tmp)).unwrap_err();
        assert_eq!(e.items[0].code, Code::PlayBinaries);
        assert_eq!(e.items[0].path, "dc-lobby-server");
        assert!(e.items[0].message.contains(&tmp.display().to_string()));
        // without one, the executable's folder comes first
        let folders = binary_folders(None);
        let exe_dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert_eq!(folders.first(), Some(&exe_dir));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
