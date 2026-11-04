//! Launch arguments: the lobby's contract (`-port -world -guid -idle -log` and the named host options) plus a few
//! DeCombat options for local runs and live tests.

use std::net::Ipv4Addr;
use std::path::PathBuf;

use crate::gs::BodySpec;
use crate::objects::PosForm;
use crate::session::{RemoteConsole, ServerTuning};
use crate::world::{NameForm, TODO_WORLD_NAME_FORM};

/// Bind address when `-bindip` is absent: loopback.
// SPEC: DSERVER_PIPE.md §2 (`-port`: loopback unless `-bindip`)
pub const DEFAULT_BIND: Ipv4Addr = Ipv4Addr::LOCALHOST;

/// Parsed launch arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct Args {
    /// UDP game port (0 = any free port, reported in `@ready`).
    pub port: u16,
    pub bind: Ipv4Addr,
    /// `-world`: the room's world (lobby form, for example `Worlds\<name>`).
    pub world: Option<String>,
    /// `-worldfile`: load a world file from disk instead of the ROM (tests, experiments).
    pub world_file: Option<PathBuf>,
    /// `-announce`: announce this client path instead of the derived one.
    pub announce_path: Option<String>,
    pub world_form: NameForm,
    /// `-rom`: the ROM folder (default `CA_ROM`).
    pub rom: Option<PathBuf>,
    /// `-content`: the mod set's weapon content document (MOD-21), applied to the weapon table at start.
    // DC-DEFAULT: DSV-478 (proposed)
    pub content: Option<PathBuf>,
    pub log: Option<PathBuf>,
    /// `-config` / `--config`: a `server.toml` for stand-alone runs (also `DC_CONFIG`).
    pub config: Option<PathBuf>,
    pub server: ServerTuning,
    /// Options accepted but not used (named in the lobby contract or unknown).
    pub ignored: Vec<String>,
    /// What the config layer set (for the start-up log).
    pub config_notes: Vec<String>,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            port: dc_wire::udp::DEFAULT_GAME_PORT,
            bind: DEFAULT_BIND,
            world: None,
            world_file: None,
            announce_path: None,
            world_form: TODO_WORLD_NAME_FORM,
            rom: None,
            content: None,
            log: None,
            config: None,
            server: ServerTuning::default(),
            ignored: Vec::new(),
            config_notes: Vec::new(),
        }
    }
}

/// Usage text.
pub const USAGE: &str = "usage: dc-dserver -port <udp> -world <Worlds\\name> [-guid ca] [-idle <secs>] [-log <file>]
       [-config <server.toml>] [-bindip <ipv4>] [-maxplayers <n>] [-name <text>] [-rom <dir>] [-worldfile <path>]
       [-content <file>] [-announce <path>] [-startorder reference|spec] [-character <model>,<skin>,<face>] [-streamown [on|off]]
       [-worldform listed|noext|arg] [-tick 10|20|30|60] [-switchstate <n>] [-skytable|-noskytable] [-preloadsounds|-nopreloadsounds] [-compactpos] [-open]
       [-roundsecs <s>] [-goal <n>] [-loadout <w,w,..>] [-bodymodel <file>] [-bodyskin <file>] [-playerbox <x,y,z>]
       [-damage <n>] [-holds <first_ms>,<second_ms>] [-timeout-hold <ms>] [-roundfreeze on|off] [-linger <secs>] [-teamempty <secs>]
       [-roundloop on|off] [-between <ms>] [-maxrounds <n>] [-intro|-nointro] [-rawspawn] [-nomovecheck]
       [-sideswitch off|5e|5f] [-ffateams on|off] [-spyintel <n>]
       [-waitbanner on|off] [-respawnbegin on|off]
       [-seed <n>] [-spawnpick stream|keyed] [-place keyed=<KEY>] [-place NICK,x,y,z,yaw[,at_ms]] [-verbose]
       [-deathstates reference|spec] [-respawnwait <ms>] [-respawnfallback <ms>] [-combat <key>=<value>,...]
       [-no9d] [-worldobjects off|mode|world|all] [-zoneclass own|room]
       [-globallight world|off] [-projectile-stream on|off] [-evars world|off] [-evar <name>=<value>]
       [-killvolumes on|off] [-doors on|off] [-camshot on|off]
       [-visibility on|off] [-visibility-window <ms>] [-visibility-radius <units>]
       [-coop <key>=<value>,...] [-directionalanims on|off] [-worldsounds on|off]
       [-stepheight <units>|off] [-worldnorth <radians>|off] [-hisec <key>=<value>,...]
       [-drops <key>=<value>,...] [-destruct <key>=<value>,...] [-gungame <seed>:<weapon>,...] [-possession]";

/// World tick rates offered by `-tick` (any 1..=240 is accepted, the range `dc-config` validates for
/// `dserver.tick_hz`; these are the documented choices).
// DC-DEFAULT: CD-63 (approved) 10 Hz default; 20 / 30 / 60 selectable for the live tests
pub const TICK_CHOICES: [u32; 4] = [10, 20, 30, 60];
/// Highest accepted tick rate (same bound as `dc_config::check` for `dserver.tick_hz`).
pub const TICK_MAX: u32 = 240;
/// Environment variable naming a `server.toml` when `-config` is absent.
pub const CONFIG_ENV: &str = "DC_CONFIG";
/// Environment variable naming the ROM folder; it wins over `paths.rom` of the file (same as the lobby).
pub const ROM_ENV: &str = "CA_ROM";
/// Log file name inside `paths.log_dir` when the config names a log folder and `-log` is absent.
pub const CONFIG_LOG_FILE: &str = "dc-dserver.log";

fn on_off(flag: &str, v: Option<String>) -> Result<bool, String> {
    match v.as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("on") => Ok(true),
        Some("off") => Ok(false),
        other => Err(format!("{flag}: bad value {other:?} (on|off)")),
    }
}

fn num<T: std::str::FromStr>(flag: &str, v: Option<String>) -> Result<T, String> {
    let v = v.ok_or_else(|| format!("{flag} needs a value"))?;
    v.parse().map_err(|_| format!("{flag}: bad value {v:?}"))
}

/// Parses the arguments after the program name.
// SPEC: DSERVER_PIPE.md §2 (launch line and named host options)
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = args.into_iter().peekable();
    while let Some(flag) = it.next() {
        let f = flag.to_ascii_lowercase();
        let f = f.trim_start_matches('-');
        let mut value = || it.next();
        match f {
            "port" => a.port = num("-port", value())?,
            "world" => a.world = Some(value().ok_or("-world needs a value")?),
            "worldfile" => {
                a.world_file = Some(PathBuf::from(value().ok_or("-worldfile needs a value")?))
            }
            "announce" => a.announce_path = Some(value().ok_or("-announce needs a value")?),
            "worldform" => {
                a.world_form = match value().as_deref() {
                    Some("listed") => NameForm::Listed,
                    Some("noext") => NameForm::NoExtension,
                    Some("arg") => NameForm::Argument,
                    other => return Err(format!("-worldform: bad value {other:?}")),
                }
            }
            "rom" => a.rom = Some(PathBuf::from(value().ok_or("-rom needs a value")?)),
            "content" => {
                a.content = Some(PathBuf::from(value().ok_or("-content needs a value")?))
            }
            "log" => a.log = Some(PathBuf::from(value().ok_or("-log needs a value")?)),
            "bindip" => a.bind = num("-bindip", value())?,
            "idle" => {
                // SPEC: DSERVER_PIPE_addendum_e2e.md B1 (`-idle` u32 seconds, 0 = no idle stop); a negative value
                // (the lobby passes its i64 as given) is read as 0 instead of failing the launch
                let v = value().ok_or("-idle needs a value")?;
                let s: i64 = v
                    .trim()
                    .parse()
                    .map_err(|_| format!("-idle: bad value {v:?}"))?;
                if s < 0 {
                    a.ignored
                        .push(format!("-idle {s} (negative: no idle stop)"));
                }
                let s = s.clamp(0, u32::MAX as i64) as u32;
                a.server.idle_ms = (s > 0).then(|| s.saturating_mul(1000));
            }
            "config" => a.config = Some(PathBuf::from(value().ok_or("-config needs a value")?)),
            "startorder" => {
                a.server.game.start_order = match value().as_deref() {
                    Some("reference") => crate::session::StartOrder::Reference,
                    Some("spec") => crate::session::StartOrder::Spec,
                    other => return Err(format!("-startorder: bad value {other:?}")),
                }
            }
            "character" => {
                let v = value().ok_or("-character needs <model>,<skin>,<face>")?;
                let n: Vec<u32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if n.len() != 3 {
                    return Err(format!("-character: bad value {v:?}"));
                }
                a.server.game.character = crate::chars::CharacterChoice {
                    model_row: n[0],
                    skin: n[1],
                    face_row: n[2],
                };
            }
            "guid" => {
                let g = value().unwrap_or_default();
                if g != "ca" {
                    a.ignored
                        .push(format!("-guid {g} (only `ca` is supported)"));
                }
            }
            "maxplayers" => {
                let n: usize = num("-maxplayers", value())?;
                a.server.max_players = (n > 0).then_some(n);
            }
            "name" => a.server.session_name = value().unwrap_or_default(),
            "tickhz" | "tick" => {
                let hz = num::<u32>("-tick", value())?.clamp(1, TICK_MAX);
                if !TICK_CHOICES.contains(&hz) {
                    a.ignored
                        .push(format!("-tick {hz} (not one of 10/20/30/60; used anyway)"));
                }
                a.server.tick_hz = hz;
            }
            "roundsecs" => a.server.game.round_secs = Some(num("-roundsecs", value())?),
            "goal" => a.server.game.goal = Some(num("-goal", value())?),
            "spyintel" => a.server.game.spy_intel = Some(num("-spyintel", value())?),
            "loadout" => {
                let v = value().ok_or("-loadout needs a value")?;
                a.server.game.default_loadout = v
                    .split(',')
                    .map(|x| {
                        x.trim()
                            .parse::<u16>()
                            .map_err(|_| format!("-loadout: bad value {x:?}"))
                    })
                    .collect::<Result<_, _>>()?;
            }
            "bodymodel" => {
                let m = value().ok_or("-bodymodel needs a value")?;
                a.server
                    .game
                    .body
                    .get_or_insert_with(BodySpec::default)
                    .model = m.into_bytes();
            }
            "bodyskin" => {
                let m = value().ok_or("-bodyskin needs a value")?;
                a.server
                    .game
                    .body
                    .get_or_insert_with(BodySpec::default)
                    .skins
                    .push(m.into_bytes());
            }
            "playerbox" => {
                let v = value().ok_or("-playerbox needs x,y,z")?;
                let d: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if d.len() != 3 || d.iter().any(|x| !(x.is_finite() && *x > 0.0)) {
                    return Err(format!("-playerbox: bad value {v:?}"));
                }
                a.server.game.player_dims = [d[0], d[1], d[2]];
                a.server.game.player_dims_from_rom = false;
            }
            "damage" => a.server.game.default_damage = num("-damage", value())?,
            "holds" => {
                // DC-DEFAULT: DSV-35 (proposed) the two pauses of the round-end set
                let v = value().ok_or("-holds needs <first_ms>,<second_ms>")?;
                let h: Vec<u32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if h.len() != 2 {
                    return Err(format!("-holds: bad value {v:?}"));
                }
                a.server.game.end_pause_ms = (h[0], h[1]);
            }
            "timeout-hold" => {
                // project decision; DC-DEFAULT: DSV-35 (amendment, proposed) the first pause of a time-limit
                // end (0 = the scoreboard set in the tick of the result)
                a.server.game.timeout_hold_ms = num("-timeout-hold", value())?
            }
            // project decision; DC-DEFAULT: DSV-77 (reinstated, proposed) the players stop with the round result
            "roundfreeze" | "round-freeze" => {
                a.server.game.round_end_freeze = on_off("-roundfreeze", value())?
            }
            "linger" => {
                a.server.game.linger_ms = num::<u32>("-linger", value())?.saturating_mul(1000)
            }
            "teamempty" => {
                a.server.game.team_empty_ms =
                    num::<u32>("-teamempty", value())?.saturating_mul(1000)
            }
            // DC-DEFAULT: RND-1 (staged) the loop, GMD-5 (proposed) the pause, RND-4 (staged) the round cap
            "roundloop" => a.server.game.round_loop = on_off("-roundloop", value())?,
            "between" => a.server.game.between_round_ms = num("-between", value())?,
            // DC-DEFAULT: GMD-7 (proposed), DSV-147 (proposed) the side switch and its message
            "sideswitch" => {
                use crate::game::modes::SideSwitch;
                a.server.game.side_switch = match value().as_deref() {
                    Some("off") => SideSwitch::Off,
                    Some("5e") => SideSwitch::TeamSwap,
                    Some("5f") => SideSwitch::ClanStatus,
                    v => return Err(format!("-sideswitch: off, 5e or 5f, not {v:?}")),
                }
            }
            // DC-DEFAULT: GMD-10 (proposed), DSV-148 (proposed) one team value per FFA player
            "ffateams" => a.server.game.ffa_teams = on_off("-ffateams", value())?,
            "maxrounds" => {
                let n: u32 = num("-maxrounds", value())?;
                a.server.game.max_rounds = (n > 0).then_some(n);
            }
            "deathstates" => {
                a.server.game.death_states = match value().as_deref() {
                    Some("reference") => crate::game::DeathStates::Reference,
                    Some("spec") => crate::game::DeathStates::Spec,
                    other => return Err(format!("-deathstates: bad value {other:?}")),
                }
            }
            "respawnwait" => a.server.game.respawn_wait_ms = Some(num("-respawnwait", value())?),
            "respawnfallback" => {
                a.server.game.respawn_fallback_ms = num("-respawnfallback", value())?
            }
            "combat" => {
                // DC-DEFAULT: DSV-30 (proposed) the damage multipliers are config keys
                let v = value().ok_or("-combat needs key=value[,key=value]")?;
                for kv in v.split(',').filter(|x| !x.trim().is_empty()) {
                    let (k, val) = kv
                        .split_once('=')
                        .ok_or_else(|| format!("-combat: {kv:?} is not key=value"))?;
                    a.server.game.set_combat(k, val)?;
                }
            }
            // DC-DEFAULT: DSV-481 (proposed) the lobby's ladder for a Gun Game room
            "gungame" => {
                let v = value().ok_or("-gungame needs seed:w,w,...")?;
                a.server.game.gun_game = Some(crate::game::gungame::parse_arg(&v)?);
            }
            // DC-DEFAULT: DSV-483 (proposed) the lobby's Possession room
            "possession" => a.server.game.possession = true,
            "hisec" => {
                // DC-DEFAULT: HS-1..HS-5 (proposed; HS-1 project decision) the case rules are operator keys
                let v = value().ok_or("-hisec needs key=value[,key=value]")?;
                for kv in v.split(',').filter(|x| !x.trim().is_empty()) {
                    let (k, val) = kv
                        .split_once('=')
                        .ok_or_else(|| format!("-hisec: {kv:?} is not key=value"))?;
                    a.server.game.hisec.set(k, val)?;
                }
            }
            "drops" => {
                // DC-DEFAULT: DSV-93..DSV-95 (proposed) the weapon drop rules are operator keys
                let v = value().ok_or("-drops needs key=value[,key=value]")?;
                for kv in v.split(',').filter(|x| !x.trim().is_empty()) {
                    let (k, val) = kv
                        .split_once('=')
                        .ok_or_else(|| format!("-drops: {kv:?} is not key=value"))?;
                    a.server.game.drops.set(k, val)?;
                }
            }
            "destruct" => {
                // DC-DEFAULT: DSV-115..DSV-120 (proposed) the destructible rules are operator keys
                let v = value().ok_or("-destruct needs key=value[,key=value]")?;
                for kv in v.split(',').filter(|x| !x.trim().is_empty()) {
                    let (k, val) = kv
                        .split_once('=')
                        .ok_or_else(|| format!("-destruct: {kv:?} is not key=value"))?;
                    a.server.game.destruct.set(k, val)?;
                }
            }
            "coop" => {
                // DC-DEFAULT: FT-S1..S8 (proposed) the co-op rule values are operator keys
                let v = value().ok_or("-coop needs key=value[,key=value]")?;
                for kv in v.split(',').filter(|x| !x.trim().is_empty()) {
                    let (k, val) = kv
                        .split_once('=')
                        .ok_or_else(|| format!("-coop: {kv:?} is not key=value"))?;
                    a.server.game.coop.set(k, val)?;
                }
            }
            "intro" => a.server.game.show_intro = true,
            // DC-DEFAULT: DSV-50 (proposed) the intro is on by default
            "nointro" | "no-intro" => a.server.game.show_intro = false,
            // DC-DEFAULT: DSV-48 (proposed) spawns on the floor; `-rawspawn` keeps the spawn point's height
            "rawspawn" => a.server.game.spawn_on_floor = false,
            // DC-DEFAULT: DSV-20 (revision, proposed) the 0x45 waiting banner off
            // DC-DEFAULT: DSV-512 (proposed) the Search and Destroy warm-up length (0 = none)
            "warmup" => {
                let v = value().ok_or("-warmup needs seconds")?;
                a.server.game.warmup_secs = Some(
                    v.trim()
                        .parse::<u32>()
                        .map_err(|_| format!("-warmup: bad value {v:?}"))?,
                );
            }
            "waitbanner" => {
                a.server.game.waiting_banner = match value().as_deref() {
                    Some("on") => true,
                    Some("off") => false,
                    other => return Err(format!("-waitbanner: bad value {other:?}")),
                }
            }
            // DC-DEFAULT: DSV-473 (settles DSV-98) a 0x45 after each respawn; `off` restores the reference stack's single one
            "respawnbegin" => {
                a.server.game.respawn_match_begin = match value().as_deref() {
                    Some("on") => true,
                    Some("off") => false,
                    other => return Err(format!("-respawnbegin: bad value {other:?}")),
                }
            }
            "nomovecheck" => a.server.game.move_check = false,
            "streamown" => {
                // DC-DEFAULT: DSV-23 (change, proposed) on by default; a bare `-streamown` keeps meaning on
                a.server.game.stream_own = match it.peek().map(|v| v.to_ascii_lowercase()) {
                    Some(v) if v == "off" => {
                        it.next();
                        false
                    }
                    Some(v) if v == "on" => {
                        it.next();
                        true
                    }
                    _ => true,
                }
            }
            "directionalanims" => {
                // DC-DEFAULT: DSV-28 (refinement, proposed) on
                a.server.game.directional_anims = on_off("-directionalanims", value())?
            }
            "worldsounds" => {
                // DC-DEFAULT: DSV-56 (proposed) on
                a.server.world_sounds = on_off("-worldsounds", value())?
            }
            "no9d" => a.server.game.send_9d = false,
            "worldobjects" => {
                a.server.world_objects = match value().as_deref() {
                    Some("off") => crate::worldobj::WorldObjects::Off,
                    Some("mode") => crate::worldobj::WorldObjects::Mode,
                    Some("world") => crate::worldobj::WorldObjects::World,
                    Some("all") => crate::worldobj::WorldObjects::All,
                    other => return Err(format!("-worldobjects: bad value {other:?}")),
                }
            }
            "globallight" => {
                a.server.global_light = match value().as_deref() {
                    Some("world") => crate::worldlight::LightMode::World,
                    Some("off") => crate::worldlight::LightMode::Off,
                    other => return Err(format!("-globallight: bad value {other:?}")),
                }
            }
            // DC-DEFAULT: DSV-53 (proposed) the client's step height 30 before the world load; `off` sends none
            "stepheight" | "step-height" => {
                a.server.step_height = match value().as_deref() {
                    Some("off") => None,
                    v => {
                        let h: f32 = num("-stepheight", v.map(str::to_string))?;
                        if !(h.is_finite() && (0.0..=1000.0).contains(&h)) {
                            return Err(format!("-stepheight: {h} is outside 0..1000"));
                        }
                        Some(h)
                    }
                }
            }
            // DC-DEFAULT: DSV-60 (proposed) the client's world-north angle 0 before the world load; `off` sends none
            "worldnorth" | "world-north" => {
                a.server.world_north = match value().as_deref() {
                    Some("off") => None,
                    v => {
                        let r: f32 = num("-worldnorth", v.map(str::to_string))?;
                        if !(r.is_finite() && (-7.0..=7.0).contains(&r)) {
                            return Err(format!("-worldnorth: {r} is outside -7..7 (radians)"));
                        }
                        Some(r)
                    }
                }
            }
            "projectile-stream" | "projectilestream" => {
                a.server.game.projectile_stream = match value().as_deref() {
                    Some("on") => true,
                    Some("off") => false,
                    other => return Err(format!("-projectile-stream: bad value {other:?}")),
                }
            }
            "zoneclass" => {
                a.server.zone_class = match value().as_deref() {
                    Some("own") => crate::worldobj::ZoneClass::Own,
                    Some("room") => crate::worldobj::ZoneClass::Room,
                    other => return Err(format!("-zoneclass: bad value {other:?}")),
                }
            }
            // DC-DEFAULT: VIS-1 (proposed) culling on by default; VIS-7 / VIS-12 (proposed) window and radius
            "visibility" => {
                a.server.vis.enabled = match value().as_deref() {
                    Some("on") => true,
                    Some("off") => false,
                    other => return Err(format!("-visibility: bad value {other:?}")),
                }
            }
            "visibility-window" => a.server.vis.exposure_ms = num("-visibility-window", value())?,
            "visibility-radius" => {
                let r: f32 = num("-visibility-radius", value())?;
                if !(r.is_finite() && r >= 0.0) {
                    return Err(format!("-visibility-radius: bad value {r}"));
                }
                a.server.vis.always_radius = r;
            }
            "seed" => a.server.game.rng_seed = num("-seed", value())?,
            // DC-DEFAULT: DSV-121 (staged, arena-bots; renumber at fold if taken) the keyed spawn pick for the arena comparison harness
            "spawnpick" => {
                a.server.game.spawn_pick = match value().as_deref() {
                    Some("stream") => crate::game::SpawnPick::Stream,
                    Some("keyed") => crate::game::SpawnPick::Keyed,
                    other => return Err(format!("-spawnpick: bad value {other:?} (stream|keyed)")),
                }
            }
            // DC-DEFAULT: DSV-146 (gate-place) test-only placement: `-place keyed=<KEY>` turns it on,
            // `-place NICK,x,y,z,yaw[,at_ms]` scripts one (needs the key; at most 16)
            "place" => {
                let v = value().ok_or("-place needs a value")?;
                if let Some(k) = v.strip_prefix("keyed=") {
                    if k.is_empty() {
                        return Err("-place keyed= needs a key".into());
                    }
                    a.server.game.place.key = Some(k.to_string());
                } else {
                    if a.server.game.place.entries.len() >= crate::game::place::MAX_ENTRIES {
                        return Err("-place: too many entries".into());
                    }
                    let e = crate::game::place::parse_entry(&v)?;
                    a.server.game.place.entries.push(e);
                }
            }
            // DC-DEFAULT: DSV-42 (proposed) every refused hit claim in the log
            "verbose" => a.server.game.verbose_hits = true,
            // DC-DEFAULT: DSV-47 (proposed) the world-settings variables; `-evar` adds the operator's own
            "evars" => {
                a.server.evars = match value().as_deref() {
                    Some("world") => crate::evars::EvarMode::World,
                    Some("off") => crate::evars::EvarMode::Off,
                    other => return Err(format!("-evars: bad value {other:?}")),
                }
            }
            "evar" => {
                let v = value().ok_or("-evar needs name=value")?;
                let (k, val) = v
                    .split_once('=')
                    .filter(|(k, _)| !k.trim().is_empty())
                    .ok_or_else(|| format!("-evar: {v:?} is not name=value"))?;
                a.server
                    .extra_evars
                    .push((k.trim().to_string(), val.to_string()));
            }
            // DC-DEFAULT: DSV-52 (proposed) kill volumes act on the server
            "killvolumes" => {
                a.server.kill_volumes = match value().as_deref() {
                    Some("on") => true,
                    Some("off") => false,
                    other => return Err(format!("-killvolumes: bad value {other:?}")),
                }
            }
            // DC-DEFAULT: DSV-46 (proposed) the sliding doors are created and move
            "doors" => {
                a.server.doors = match value().as_deref() {
                    Some("on") => true,
                    Some("off") => false,
                    other => return Err(format!("-doors: bad value {other:?}")),
                }
            }
            // DC-DEFAULT: DSV-112 (proposed) the world cameras and their trigger statements
            "camshot" => {
                a.server.cam_shot = match value().as_deref() {
                    Some("on") => true,
                    Some("off") => false,
                    other => return Err(format!("-camshot: bad value {other:?}")),
                }
            }
            "switchstate" => a.server.switch_state = num("-switchstate", value())?,
            "skytable" => a.server.send_sky_table = true,
            "noskytable" => a.server.send_sky_table = false,
            // DC-DEFAULT: DSV-65 (proposed)
            "preloadsounds" => a.server.preload_sounds = true,
            "nopreloadsounds" => a.server.preload_sounds = false,
            "compactpos" => a.server.player_pos_form = PosForm::Compact,
            "open" => a.server.accept_unlisted = true,
            "game" | "fps" | "netfmt" => {
                let v = value().unwrap_or_default();
                a.ignored.push(format!("-{f} {v}"));
            }
            _ => {
                // unknown option: keep going; take its value if one follows
                let v = match it.peek() {
                    Some(n) if !n.starts_with('-') => it.next().unwrap_or_default(),
                    _ => String::new(),
                };
                a.ignored.push(format!("{flag} {v}").trim_end().to_string());
            }
        }
    }
    if !a.server.game.place.entries.is_empty() && a.server.game.place.key.is_none() {
        return Err("-place NICK,...: refused without -place keyed=<KEY>".into());
    }
    if a.world.is_none() && a.world_file.is_none() {
        return Err("no world: pass -world (with CA_ROM) or -worldfile".into());
    }
    Ok(a)
}

/// Flags present on the command line (lower case, without dashes), for the "flag wins" rule of the config layer.
fn given_flags(argv: &[String]) -> Vec<String> {
    argv.iter()
        .filter(|x| x.starts_with('-') && x[1..].chars().any(|c| c.is_ascii_alphabetic()))
        .map(|x| x.trim_start_matches('-').to_ascii_lowercase())
        .collect()
}

/// Parses the arguments, then layers `server.toml` (`-config`, else `DC_CONFIG`) and the `DC_*` environment under
/// them: flag > environment > file > built-in default. A config value counts only when the file or the environment
/// names its path (`dc_config::Explicit`), because a loaded config cannot tell "unset" from "default". Without a
/// config file the `DC_*` variables still apply. Config errors (parse or `dc_config::check`) fail the launch.
// SPEC: DSERVER_PIPE.md §2 (the lobby's flags always win); DSERVER_PIPE_addendum_e2e.md D (contract unchanged)
// DC-DEFAULT: CD-10, CD-56, CD-58, CD-61, CD-63, CD-64, CD-65 (approved) through their `dserver.*` / `room.*` keys
pub fn parse_with_env<I: IntoIterator<Item = String>>(
    args: I,
    env: &[(String, String)],
) -> Result<Args, String> {
    let argv: Vec<String> = args.into_iter().collect();
    let mut a = parse(argv.iter().cloned())?;
    let env_get = |k: &str| {
        env.iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.clone())
            .filter(|v| !v.trim().is_empty())
    };
    if a.config.is_none() {
        a.config = env_get(CONFIG_ENV).map(PathBuf::from);
    }
    let text = match &a.config {
        Some(p) => {
            Some(std::fs::read_to_string(p).map_err(|e| format!("config {}: {e}", p.display()))?)
        }
        None => None,
    };
    let dc_env: Vec<(String, String)> = env
        .iter()
        .filter(|(k, _)| k.starts_with("DC_") && k.contains("__"))
        .cloned()
        .collect();
    if text.is_none() && dc_env.is_empty() {
        return Ok(a);
    }
    let (cfg, ex) = dc_config::load_layered_explicit(
        text.as_deref(),
        dc_env,
        &dc_config::CliOverrides::default(),
    )
    .map_err(|issues| config_error(&issues))?;
    let issues = dc_config::check(&cfg, dc_config::CheckOptions::default());
    if dc_config::has_errors(&issues) {
        return Err(config_error(&issues));
    }
    let given = given_flags(&argv);
    let flag = |names: &[&str]| names.iter().any(|n| given.iter().any(|g| g == n));
    let mut notes = Vec::new();
    if !flag(&["port"]) && ex.has("dserver.port_range") {
        let (lo, _) = cfg.dserver_range()?;
        a.port = lo;
        notes.push(format!("port {lo} (dserver.port_range)"));
    }
    if !flag(&["tickhz", "tick"]) && ex.has("dserver.tick_hz") {
        a.server.tick_hz = cfg.dserver.tick_hz.clamp(1, TICK_MAX);
        notes.push(format!("tick {} Hz (dserver.tick_hz)", a.server.tick_hz));
    }
    if !flag(&["rom"])
        && env_get(ROM_ENV).is_none()
        && ex.has("paths.rom")
        && !cfg.paths.rom.is_empty()
    {
        a.rom = Some(PathBuf::from(&cfg.paths.rom));
        notes.push("ROM folder from paths.rom".into());
    }
    if !flag(&["bindip"]) && (ex.has("server.bind") || ex.has("server.public")) {
        a.bind = cfg.effective_bind().parse().map_err(|_| {
            format!(
                "server.bind: {:?} is not an IPv4 address",
                cfg.effective_bind()
            )
        })?;
        notes.push(format!("bind {} (server.bind / server.public)", a.bind));
    }
    if !flag(&["name"]) && ex.has("server.name") {
        a.server.session_name = cfg.server.name.clone();
        notes.push("session name from server.name".into());
    }
    if !flag(&["maxplayers"]) && ex.has("room.max_players_cap") {
        let n = cfg.room.max_players_cap as usize;
        a.server.max_players = (n > 0).then_some(n);
        notes.push(format!("max players {n} (room.max_players_cap)"));
    }
    if ex.has("room.blast_damage_scale") {
        // DC-DEFAULT: DSV-165 (gameplay choice, user-tunable)
        a.server.game.projectile.blast_damage_scale = cfg.room.blast_damage_scale as f32;
        notes.push(format!(
            "blast damage x {} (room.blast_damage_scale)",
            cfg.room.blast_damage_scale
        ));
    }
    if !flag(&["log"]) && ex.has("paths.log_dir") && !cfg.paths.log_dir.is_empty() {
        a.log = Some(PathBuf::from(&cfg.paths.log_dir).join(CONFIG_LOG_FILE));
        notes.push("log file in paths.log_dir".into());
    }
    if ex.has("dserver.host_rate_bps") {
        a.server.host_rate_bps = cfg.dserver.host_rate_bps;
        notes.push(format!(
            "host rate {} (dserver.host_rate_bps)",
            cfg.dserver.host_rate_bps
        ));
    }
    if ex.has("dserver.startup_deadline_secs") {
        a.server.startup_deadline_ms = cfg.dserver.startup_deadline_secs.saturating_mul(1000);
        notes.push("start-up deadline from dserver.startup_deadline_secs".into());
    }
    if ex.has("dserver.send_budget_bits") && cfg.dserver.send_budget_bits > 0 {
        a.server.game.stream_budget_bits = cfg.dserver.send_budget_bits as usize;
        notes.push(format!(
            "stream budget {} bits (dserver.send_budget_bits)",
            cfg.dserver.send_budget_bits
        ));
    }
    if ex.has("dserver.remote_console") {
        a.server.remote_console = if cfg.dserver.remote_console == "moderators" {
            RemoteConsole::Moderators
        } else {
            a.ignored.push(format!(
                "dserver.remote_console = {:?} (only \"moderators\" is implemented; no command runs)",
                cfg.dserver.remote_console
            ));
            RemoteConsole::Nobody
        };
    }
    for (key, supported, value) in [
        (
            "dserver.hello_match",
            "name",
            cfg.dserver.hello_match.as_str(),
        ),
        ("dserver.file_info", "size", cfg.dserver.file_info.as_str()),
    ] {
        if ex.has(key) && value != supported {
            a.ignored.push(format!(
                "{key} = {value:?} (only {supported:?} is implemented)"
            ));
        }
    }
    if ex.has("dserver.echo_own_rotation") && cfg.dserver.echo_own_rotation {
        // DC-DEFAULT: CD-67 (approved); the own object's stream entries carry the yaw8 of a character, never a
        // rotation echo (DSV-23 change, proposed)
        a.ignored.push(
            "dserver.echo_own_rotation = true (the own object never gets a rotation echo)".into(),
        );
    }
    // [combat] (DSV-30, DSV-34 proposed): the keys the file or the environment names go onto the tuning first, then
    // the command line's `-combat key=value` (and `-damage`) are applied again on top: flag > env > file > default
    // DC-DEFAULT: DSV-30 (proposed) the damage multipliers are config keys, set by `[combat]` or `-combat`
    let mut combat_keys = Vec::new();
    for (k, v) in cfg.combat.pairs() {
        if !ex.has(&format!("combat.{k}")) || (k == "default_damage" && flag(&["damage"])) {
            continue;
        }
        a.server.game.set_combat(k, &v)?;
        combat_keys.push(k);
    }
    if !combat_keys.is_empty() {
        for v in combat_flag_values(&argv) {
            for kv in v.split(',').filter(|x| !x.trim().is_empty()) {
                if let Some((k, val)) = kv.split_once('=') {
                    a.server.game.set_combat(k, val)?;
                }
            }
        }
        notes.push(format!("combat {} from [combat]", combat_keys.join(", ")));
    }
    // [spy] (DSV-451 proposed, amended 2026-10-30): env > file > default
    for (key, v) in [
        ("spy.agent_health", cfg.spy.agent_health),
        ("spy.agent_armor", cfg.spy.agent_armor),
        ("spy.agent_damage_scale", cfg.spy.agent_damage_scale),
    ] {
        if !ex.has(key) {
            continue;
        }
        let v = v as f32;
        match key {
            "spy.agent_health" => a.server.game.agent_health = v.max(1.0),
            "spy.agent_armor" => a.server.game.agent_armor = v.max(0.0),
            // DC-DEFAULT: DSV-456 (proposed)
            _ => a.server.game.agent_damage_scale = v.max(0.0),
        }
        notes.push(format!("{key} = {v}"));
    }
    // [vis] (VIS-1, VIS-7, VIS-8, VIS-12 proposed): flag > env > file > default
    if !flag(&["visibility"]) && ex.has("vis.enabled") {
        a.server.vis.enabled = cfg.vis.enabled;
        notes.push(format!(
            "visibility culling {} (vis.enabled)",
            if cfg.vis.enabled { "on" } else { "off" }
        ));
    }
    if !flag(&["visibility-window"]) && ex.has("vis.exposure_ms") {
        a.server.vis.exposure_ms = cfg.vis.exposure_ms;
        notes.push(format!(
            "visibility window {} ms (vis.exposure_ms)",
            cfg.vis.exposure_ms
        ));
    }
    if !flag(&["visibility-radius"]) && ex.has("vis.always_radius") {
        a.server.vis.always_radius = cfg.vis.always_radius as f32;
        notes.push(format!(
            "visibility radius {} (vis.always_radius)",
            cfg.vis.always_radius
        ));
    }
    if ex.has("vis.max_segments_per_tick") {
        a.server.vis.max_segments_per_tick = cfg.vis.max_segments_per_tick;
        notes.push(format!(
            "visibility budget {} segments per tick (vis.max_segments_per_tick)",
            cfg.vis.max_segments_per_tick
        ));
    }
    // [evars] (DSV-47 / DSV-60 proposed): flag > env > file > default. `-evars` decides the mode; the table's extra
    // variables go before the command line's `-evar` ones, so a flag replaces a table variable of the same name
    // DC-DEFAULT: DSV-47, DSV-60 (proposed) the evar switches are config keys
    if !flag(&["evars"]) && ex.has("evars.mode") {
        a.server.evars = match cfg.evars.mode.as_str() {
            "off" => crate::evars::EvarMode::Off,
            _ => crate::evars::EvarMode::World,
        };
        notes.push(format!("server variables {} (evars.mode)", cfg.evars.mode));
    }
    let table: Vec<(String, String)> = cfg
        .evars
        .extra
        .iter()
        .filter(|(k, _)| ex.has(&format!("evars.extra.{k}")))
        .map(|(k, v)| (k.trim().to_string(), v.clone()))
        .collect();
    if !table.is_empty() {
        notes.push(format!(
            "{} extra server variables (evars.extra)",
            table.len()
        ));
        let mut all = table;
        all.append(&mut a.server.extra_evars);
        a.server.extra_evars = all;
    }
    a.config_notes = notes;
    Ok(a)
}

/// The values of every `-combat` / `--combat` option in `argv`, in order.
fn combat_flag_values(argv: &[String]) -> Vec<String> {
    argv.windows(2)
        .filter(|w| {
            w[0].trim_start_matches('-').eq_ignore_ascii_case("combat") && w[0].starts_with('-')
        })
        .map(|w| w[1].clone())
        .collect()
}

fn config_error(issues: &[dc_config::Issue]) -> String {
    let parts: Vec<String> = issues
        .iter()
        .filter(|i| i.is_error())
        .map(|i| format!("{}: {}", i.path, i.message))
        .collect();
    format!("config: {}", parts.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<Args, String> {
        parse(s.split_whitespace().map(String::from))
    }

    fn pe(s: &str, env: &[(&str, &str)]) -> Result<Args, String> {
        let env: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        parse_with_env(s.split_whitespace().map(String::from), &env)
    }

    fn temp_toml(name: &str, text: &str) -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("dc-dserver-cli-{}-{name}.toml", std::process::id()));
        std::fs::write(&p, text).unwrap();
        p
    }

    /// DSV-479: a room's hacks reach the game server as plain flags after the operator's own; the last value wins,
    /// so a "no-recoil only" room runs culling on and the cone check off even under an operator's `-visibility off`
    /// / `cone_check=refuse`.
    #[test]
    fn room_hacks_flags_set_culling_and_the_cone_check_last_wins() {
        let a = parse(
            "-world w -visibility off -combat cone_check=refuse -visibility on -combat cone_check=off"
                .split_whitespace()
                .map(String::from),
        )
        .unwrap();
        assert!(a.server.vis.enabled);
        assert_eq!(a.server.game.cone_check, crate::cone::ConeMode::Off);
        let a = parse(
            "-world w -visibility off"
                .split_whitespace()
                .map(String::from),
        )
        .unwrap();
        assert!(!a.server.vis.enabled);
        assert_eq!(a.server.game.cone_check, crate::cone::ConeMode::Log);
    }

    #[test]
    fn visibility_switches_and_the_vis_table() {
        // DC-DEFAULT: VIS-1 (proposed) on by default
        let a = parse(["-world", "w"].map(String::from)).unwrap();
        assert!(a.server.vis.enabled);
        assert_eq!(
            (a.server.vis.exposure_ms, a.server.vis.always_radius),
            (1000, 800.0)
        );
        let a = parse(
            "-world w -visibility off -visibility-window 1500 -visibility-radius 300"
                .split_whitespace()
                .map(String::from),
        )
        .unwrap();
        assert!(!a.server.vis.enabled);
        assert_eq!(
            (a.server.vis.exposure_ms, a.server.vis.always_radius),
            (1500, 300.0)
        );
        assert!(parse(
            "-world w -visibility maybe"
                .split_whitespace()
                .map(String::from)
        )
        .is_err());
        assert!(parse(
            "-world w -visibility-radius -5"
                .split_whitespace()
                .map(String::from)
        )
        .is_err());
        // MOD-22: a config file may not switch culling off (the dserver flag of a modded match does)
        let p = temp_toml(
            "vis",
            "[vis]
enabled = true
exposure_ms = 700
always_radius = 250.0
max_segments_per_tick = 64
",
        );
        let a = pe(&format!("-world w -config {}", p.display()), &[]).unwrap();
        assert!(a.server.vis.enabled);
        assert_eq!(a.server.vis.exposure_ms, 700);
        assert_eq!(a.server.vis.always_radius, 250.0);
        assert_eq!(a.server.vis.max_segments_per_tick, 64);
        // the flag wins over the file, the environment over the file
        let a = pe(
            &format!("-world w -visibility off -config {}", p.display()),
            &[("DC_VIS__EXPOSURE_MS", "900")],
        )
        .unwrap();
        assert!(!a.server.vis.enabled);
        assert_eq!(a.server.vis.exposure_ms, 900);
        let _ = std::fs::remove_file(p);
    }

    /// DSV-451 (amended 2026-10-30): `[spy] agent_health / agent_armor` and their `DC_SPY__*` variables set the
    /// agent's vitals; unset, the defaults 1000 / 500.
    #[test]
    fn config_spy_table_sets_the_agents_health_and_armour() {
        let a = pe("-world w", &[]).unwrap();
        assert_eq!((a.server.game.agent_health, a.server.game.agent_armor), (1000.0, 500.0));
        assert_eq!(a.server.game.agent_damage_scale, 2.0, "DSV-456 default");
        let p = temp_toml("spy", "[spy]
agent_health = 1200.0
agent_armor = 300.0
agent_damage_scale = 3.5
");
        let a = pe(&format!("-world w -config {}", p.display()), &[]).unwrap();
        assert_eq!((a.server.game.agent_health, a.server.game.agent_armor), (1200.0, 300.0));
        assert_eq!(a.server.game.agent_damage_scale, 3.5);
        let a = pe(
            &format!("-world w -config {}", p.display()),
            &[("DC_SPY__AGENT_HEALTH", "2000")],
        )
        .unwrap();
        assert_eq!((a.server.game.agent_health, a.server.game.agent_armor), (2000.0, 300.0));
        let bad = temp_toml("spy-bad", "[spy]
agent_health = 0.0
");
        assert!(pe(&format!("-world w -config {}", bad.display()), &[])
            .unwrap_err()
            .contains("spy.agent_health"));
        let _ = std::fs::remove_file(p);
        let _ = std::fs::remove_file(bad);
    }

    #[test]
    fn config_combat_table_sets_the_tuning_and_the_flag_wins() {
        // DC-DEFAULT: DSV-30 (proposed) the damage multipliers are config keys
        let p = temp_toml(
            "combat",
            "[combat]
head_factor = 3.0
body_factor = 1.5
armor_absorb = 0.5
zone_factors = \"off\"
trace = \"off\"
",
        );
        let base = ServerTuning::default().game;
        let a = pe(&format!("-world w -config {}", p.display()), &[]).unwrap();
        assert_eq!(base.head_factor, 2.0);
        assert_eq!(a.server.game.head_factor, 3.0);
        assert_eq!(a.server.game.body_factor, 1.5);
        assert_eq!(a.server.game.armor_absorb, Some(0.5));
        assert!(!a.server.game.zone_factors_from_rom);
        assert!(!a.server.game.trace);
        assert_eq!(
            a.server.game.trace_slack, base.trace_slack,
            "unset keys keep the default"
        );
        assert!(a.config_notes.iter().any(|n| n.contains("head_factor")));
        // -combat overrides the file; a key the flag does not name keeps the file's value
        let a = pe(
            &format!(
                "-world w -combat head_factor=5,start_health=80 -config {}",
                p.display()
            ),
            &[],
        )
        .unwrap();
        assert_eq!(a.server.game.head_factor, 5.0);
        assert_eq!(a.server.game.start_health, 80.0);
        assert_eq!(a.server.game.body_factor, 1.5);
        // the DC_* environment wins over the file; the flag wins over both
        let a = pe(
            &format!("-world w -config {}", p.display()),
            &[
                ("DC_COMBAT__HEAD_FACTOR", "4"),
                ("DC_COMBAT__ARMOR_ABSORB", "rom"),
            ],
        )
        .unwrap();
        assert_eq!(a.server.game.head_factor, 4.0);
        assert_eq!(a.server.game.armor_absorb, None);
        let a = pe(
            "-world w -damage 40 -combat head_factor=6",
            &[
                ("DC_COMBAT__HEAD_FACTOR", "4"),
                ("DC_COMBAT__DEFAULT_DAMAGE", "10"),
            ],
        )
        .unwrap();
        assert_eq!(
            (a.server.game.head_factor, a.server.game.default_damage),
            (6.0, 40.0)
        );
        // a bad table is a launch error, not a silent default
        let bad = temp_toml(
            "combat-bad",
            "[combat]
zone_factors = \"sideways\"
",
        );
        assert!(pe(&format!("-world w -config {}", bad.display()), &[])
            .unwrap_err()
            .contains("combat.zone_factors"));
    }

    #[test]
    fn config_layer_fills_unset_flags_only() {
        let p = temp_toml(
            "a",
            "[server]\nname = \"Cfg\"\nbind = \"127.0.0.2\"\n[dserver]\nport_range = \"30100-30110\"\n\
             tick_hz = 30\nhost_rate_bps = 640000\nstartup_deadline_secs = 9\n[room]\nmax_players_cap = 12\n\
             [paths]\nrom = \"R\"\n",
        );
        let a = pe(&format!("-world w -config {}", p.display()), &[]).unwrap();
        assert_eq!(a.port, 30100);
        assert_eq!(a.server.tick_hz, 30);
        assert_eq!(a.bind, Ipv4Addr::new(127, 0, 0, 2));
        assert_eq!(a.server.session_name, "Cfg");
        assert_eq!(a.server.max_players, Some(12));
        assert_eq!(a.server.host_rate_bps, 640_000);
        assert_eq!(a.server.startup_deadline_ms, 9_000);
        assert_eq!(a.rom, Some(PathBuf::from("R")));
        assert!(!a.config_notes.is_empty());
        // flags win; CA_ROM wins over paths.rom; the DC_* environment wins over the file
        let a = pe(
            &format!(
                "-world w -port 5 -tickhz 20 -bindip 127.0.0.1 --config {}",
                p.display()
            ),
            &[("CA_ROM", "X"), ("DC_SERVER__NAME", "Env")],
        )
        .unwrap();
        assert_eq!((a.port, a.server.tick_hz), (5, 20));
        assert_eq!(a.bind, Ipv4Addr::LOCALHOST);
        assert_eq!(a.rom, None, "CA_ROM stays the ROM layer");
        assert_eq!(a.server.session_name, "Env");
        // DC_CONFIG names the file when -config is absent; unset keys keep the built-in defaults
        let q = temp_toml("b", "[dserver]\ntick_hz = 60\n");
        let a = pe("-world w", &[("DC_CONFIG", q.to_str().unwrap())]).unwrap();
        assert_eq!(a.server.tick_hz, 60);
        assert_eq!(a.port, dc_wire::udp::DEFAULT_GAME_PORT);
        assert_eq!(
            a.server.host_rate_bps,
            ServerTuning::default().host_rate_bps
        );
        // the environment alone (no file) also applies
        let a = pe("-world w", &[("DC_DSERVER__TICK_HZ", "30")]).unwrap();
        assert_eq!(a.server.tick_hz, 30);
        // a config error fails the launch
        let bad = temp_toml("c", "[dserver]\ntick_hz = 0\n");
        assert!(pe(&format!("-world w -config {}", bad.display()), &[]).is_err());
        assert!(pe("-world w -config no_such_file_dc_dserver.toml", &[]).is_err());
        for f in [p, q, bad] {
            let _ = std::fs::remove_file(f);
        }
    }

    #[test]
    fn negative_idle_means_no_idle_stop_and_new_switches() {
        let a = p("-world w -idle -1").unwrap();
        assert_eq!(a.server.idle_ms, None);
        assert_eq!(a.ignored.len(), 1);
        let a = p("-world w -startorder spec -character 2,1,3 -tickhz 240 -streamown").unwrap();
        assert!(a.server.game.stream_own);
        assert!(
            p("-world w").unwrap().server.game.stream_own,
            "DSV-23: on by default"
        );
        let b = p("-world w -streamown off -directionalanims off -worldsounds off").unwrap();
        assert!(!b.server.game.stream_own);
        assert!(!b.server.game.directional_anims);
        assert!(!b.server.world_sounds);
        let b = p("-world w -streamown on -idle 5").unwrap();
        assert!(
            b.server.game.stream_own && b.server.game.directional_anims && b.server.world_sounds
        );
        assert_eq!(b.server.idle_ms, Some(5000), "the next flag is not eaten");
        let b = p("-world w -streamown -idle 5").unwrap();
        assert!(b.server.game.stream_own);
        assert_eq!(b.server.idle_ms, Some(5000));
        assert!(p("-world w -worldsounds maybe").is_err());
        assert_eq!(a.server.game.start_order, crate::session::StartOrder::Spec);
        let c = a.server.game.character;
        assert_eq!((c.model_row, c.skin, c.face_row), (2, 1, 3));
        assert_eq!(a.server.tick_hz, 240);
        assert!(p("-world w -startorder odd").is_err());
        assert_eq!(
            p("-world w -spawnpick keyed")
                .unwrap()
                .server
                .game
                .spawn_pick,
            crate::game::SpawnPick::Keyed
        );
        assert!(p("-world w -spawnpick odd").is_err());
        assert!(p("-world w -character 1,2").is_err());
    }

    #[test]
    fn blast_damage_scale_from_the_environment() {
        // DC-DEFAULT: DSV-165 (gameplay choice, user-tunable) room.blast_damage_scale / DC_ROOM__BLAST_DAMAGE_SCALE
        assert_eq!(
            pe("-world w", &[])
                .unwrap()
                .server
                .game
                .projectile
                .blast_damage_scale,
            1.0
        );
        let a = pe("-world w", &[("DC_ROOM__BLAST_DAMAGE_SCALE", "2.0")]).unwrap();
        assert_eq!(a.server.game.projectile.blast_damage_scale, 2.0);
        assert!(pe("-world w", &[("DC_ROOM__BLAST_DAMAGE_SCALE", "-1")]).is_err());
    }

    #[test]
    fn evars_table_and_blast_inner_from_the_config() {
        // DC-DEFAULT: DSV-47 / DSV-60 (proposed) [evars]; DSV-39 (amendment, proposed) combat.blast_inner
        let dir = std::env::temp_dir().join(format!("dc_dserver_evars_cfg_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("server.toml");
        std::fs::write(
            &file,
            "[evars]\nmode = \"off\"\n[evars.extra]\nAlpha = \"1\"\nBeta = \"2\"\n[combat]\nblast_inner = 0.5\n",
        )
        .unwrap();
        let cfg = file.to_string_lossy().to_string();
        let a = pe(&format!("-world w -config {cfg}"), &[]).unwrap();
        assert_eq!(a.server.evars, crate::evars::EvarMode::Off);
        assert_eq!(
            a.server.extra_evars,
            vec![
                ("Alpha".to_string(), "1".to_string()),
                ("Beta".to_string(), "2".to_string())
            ]
        );
        assert_eq!(a.server.game.projectile.inner_fraction, 0.5);
        // the flags win: -evars decides the mode, -evar follows the table (and so replaces Beta)
        let a = pe(
            &format!("-world w -config {cfg} -evars world -evar Beta=9 -combat blast_inner=0"),
            &[],
        )
        .unwrap();
        assert_eq!(a.server.evars, crate::evars::EvarMode::World);
        assert_eq!(
            a.server.extra_evars.last(),
            Some(&("Beta".to_string(), "9".to_string()))
        );
        assert_eq!(a.server.extra_evars.len(), 3);
        assert_eq!(a.server.game.projectile.inner_fraction, 0.0);
        // the environment beats the file
        let a = pe(
            &format!("-world w -config {cfg}"),
            &[("DC_EVARS__MODE", "world")],
        )
        .unwrap();
        assert_eq!(a.server.evars, crate::evars::EvarMode::World);
        // the schema default stays the dserver's own default
        assert_eq!(
            dc_config::Config::default().combat.blast_inner as f32,
            crate::game::GameTuning::default().projectile.inner_fraction
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn drop_keys() {
        let a = p("-world w -drops lifetime_ms=5000,pickup_radius=64,arm_ms=0").unwrap();
        let d = a.server.game.drops;
        assert!(d.enabled);
        assert_eq!((d.lifetime_ms, d.pickup_radius, d.arm_ms), (5000, 64.0, 0));
        assert!(
            !p("-world w -drops enabled=off")
                .unwrap()
                .server
                .game
                .drops
                .enabled
        );
        assert!(p("-world w -drops bogus=1").is_err());
        let a = p("-world w -destruct enabled=off,effect_ms=500").unwrap();
        assert!(!a.server.game.destruct.enabled);
        assert_eq!(a.server.game.destruct.effect_ms, 500);
        assert!(p("-world w -destruct bogus=1").is_err());
        assert!(p("-world w -drops lifetime_ms").is_err());
    }

    #[test]
    fn hisec_keys() {
        let a = p("-world w -hisec force=on,pickup_radius=5000,reserve_ms=3000").unwrap();
        let h = a.server.game.hisec;
        assert!(h.force && h.enabled);
        assert_eq!((h.pickup_radius, h.reserve_ms), (5000.0, 3000));
        assert!(p("-world w -hisec bogus=1").is_err());
        assert!(p("-world w -hisec force").is_err());
        let a = p("-world w -combat blast_inner=0").unwrap();
        assert_eq!(a.server.game.projectile.inner_fraction, 0.0);
        // DC-DEFAULT: DSV-92 (proposed)
        assert!(p("-world w").unwrap().server.game.projectile.blast_los);
        let a = p("-world w -combat blast_los=off").unwrap();
        assert!(!a.server.game.projectile.blast_los);
        assert!(p("-world w -combat blast_los=maybe").is_err());
    }

    /// DSV-80: the weapon grants' default attachments are on unless `-combat weapon_parts=none`.
    // DC-DEFAULT: DSV-80 (proposed)
    #[test]
    fn weapon_parts_switch() {
        assert!(p("-world w").unwrap().server.game.weapon_parts);
        assert!(
            !p("-world w -combat weapon_parts=none")
                .unwrap()
                .server
                .game
                .weapon_parts
        );
        assert!(
            p("-world w -combat weapon_parts=DEFAULT")
                .unwrap()
                .server
                .game
                .weapon_parts
        );
        assert!(p("-world w -combat weapon_parts=maybe").is_err());
    }

    #[test]
    fn light_and_projectile_stream_switches() {
        use crate::worldlight::LightMode;
        let a = p("-world w").unwrap();
        assert_eq!(a.server.global_light, LightMode::World);
        assert!(
            a.server.game.projectile_stream,
            "DSV-41 (amended): on by default"
        );
        let a = p("-world w -globallight off -projectile-stream off").unwrap();
        assert_eq!(a.server.global_light, LightMode::Off);
        assert!(!a.server.game.projectile_stream);
        assert!(
            p("-world w -projectile-stream on")
                .unwrap()
                .server
                .game
                .projectile_stream
        );
        assert!(p("-world w -globallight dim").is_err());
        assert!(p("-world w -projectile-stream maybe").is_err());
    }

    #[test]
    fn step_height_switch() {
        assert_eq!(p("-world w").unwrap().server.step_height, Some(30.0));
        assert_eq!(
            p("-world w -stepheight off").unwrap().server.step_height,
            None
        );
        assert_eq!(
            p("-world w -stepheight 18.5").unwrap().server.step_height,
            Some(18.5)
        );
        assert!(p("-world w -stepheight").is_err());
        assert!(p("-world w -stepheight tall").is_err());
        assert!(p("-world w -stepheight -1").is_err());
        assert!(p("-world w -stepheight 5000").is_err());
    }

    #[test]
    fn world_north_switch() {
        assert_eq!(p("-world w").unwrap().server.world_north, Some(0.0));
        assert_eq!(
            p("-world w -worldnorth off").unwrap().server.world_north,
            None
        );
        assert_eq!(
            p("-world w -worldnorth 1.5").unwrap().server.world_north,
            Some(1.5)
        );
        assert!(p("-world w -worldnorth").is_err());
        assert!(p("-world w -worldnorth up").is_err());
        assert!(p("-world w -worldnorth 90").is_err());
    }

    #[test]
    fn lobby_launch_line() {
        // the lobby's launch: -port <p> -world <w> -guid <g> -idle <s> [-log <f>] [extra args]
        let a = p("-port 27888 -world Worlds\\Alpha -guid ca -idle 60 -log x.log -fps 30 -zzz 1")
            .unwrap();
        assert_eq!(a.port, 27888);
        assert_eq!(a.world.as_deref(), Some("Worlds\\Alpha"));
        assert_eq!(a.server.idle_ms, Some(60_000));
        assert_eq!(a.log, Some(PathBuf::from("x.log")));
        assert_eq!(a.bind, Ipv4Addr::LOCALHOST);
        assert_eq!(a.ignored, vec!["-fps 30".to_string(), "-zzz 1".to_string()]);
    }

    #[test]
    fn decombat_options_and_errors() {
        let a = p("-port 0 -worldfile w.dat -announce Worlds\\W.dat -open -skytable -compactpos -tickhz 20 -idle 0")
            .unwrap();
        assert!(a.server.accept_unlisted && a.server.send_sky_table);
        assert_eq!(a.server.player_pos_form, PosForm::Compact);
        assert_eq!(a.server.tick_hz, 20);
        assert_eq!(a.server.idle_ms, None);
        assert!(p("-port 1").is_err());
        assert!(p("-port x -world a").is_err());
        assert!(p("-world a -worldform odd").is_err());
    }

    #[test]
    fn game_options() {
        let a = p("-world w -tick 60 -roundsecs 300 -goal 20 -loadout 3,4 -bodymodel m.ltb -bodyskin s.dtx \
                   -playerbox 20,50,20 -damage 30 -intro -nomovecheck -seed 9 -holds 300,200 -timeout-hold 250")
        .unwrap();
        assert_eq!(a.server.tick_hz, 60);
        let g = &a.server.game;
        assert_eq!((g.round_secs, g.goal), (Some(300), Some(20)));
        assert_eq!((g.end_pause_ms, g.timeout_hold_ms), ((300, 200), 250));
        assert_eq!(
            p("-world w").unwrap().server.game.timeout_hold_ms,
            3_000,
            "DSV-35 amendment: no first hold at a time-limit end"
        );
        assert_eq!(g.default_loadout, vec![3, 4]);
        let b = g.body.as_ref().unwrap();
        assert_eq!((b.model.as_slice(), b.skins.len()), (&b"m.ltb"[..], 1));
        assert_eq!(g.player_dims, [20.0, 50.0, 20.0]);
        assert_eq!(g.default_damage, 30.0);
        assert!(g.show_intro && !g.move_check);
        assert!(
            !g.waiting_banner,
            "DSV-20 revision: the waiting banner is off by default"
        );
        assert!(
            p("-world w -waitbanner on")
                .unwrap()
                .server
                .game
                .waiting_banner
        );
        assert!(
            !p("-world w -waitbanner off")
                .unwrap()
                .server
                .game
                .waiting_banner
        );
        assert!(p("-world w -waitbanner maybe").is_err());
        // DSV-77 (reinstated): the round-end freeze is on by default
        assert!(g.round_end_freeze);
        assert_eq!(g.freeze_wait_ms, 1_000);
        let off = p("-world w -roundfreeze off").unwrap();
        assert!(!off.server.game.round_end_freeze);
        assert!(
            p("-world w -roundfreeze ON")
                .unwrap()
                .server
                .game
                .round_end_freeze
        );
        assert!(p("-world w -roundfreeze maybe").is_err());
        assert!(p("-world w").unwrap().server.game.respawn_match_begin);
        assert!(
            !p("-world w -respawnbegin off")
                .unwrap()
                .server
                .game
                .respawn_match_begin
        );
        assert!(p("-world w -respawnbegin x").is_err());
        assert_eq!(g.rng_seed, 9);
        assert_eq!(g.end_pause_ms, (300, 200));
        let a = p("-world w -coop lives=3,max_alive=4,ray_hits=off").unwrap();
        let c = &a.server.game.coop;
        assert_eq!((c.lives, c.max_alive, c.ray_hits), (Some(3), 4, false));
        assert!(p("-world w -coop lives").is_err());
        assert!(p("-world w -coop nope=1").is_err());
        let a = p("-world w -roundloop off -between 2500 -maxrounds 7").unwrap();
        assert!(!a.server.game.round_loop);
        assert_eq!(a.server.game.between_round_ms, 2500);
        assert_eq!(a.server.game.max_rounds, Some(7));
        assert!(a.ignored.is_empty(), "{:?}", a.ignored);
        let a = p("-world w -maxrounds 0").unwrap();
        assert_eq!(a.server.game.max_rounds, None);
        assert!(a.server.game.round_loop);
        assert!(p("-world w -roundloop maybe").is_err());
        let a = p("-world w -sideswitch 5f -ffateams off").unwrap();
        assert_eq!(
            a.server.game.side_switch,
            crate::game::modes::SideSwitch::ClanStatus
        );
        assert!(!a.server.game.ffa_teams);
        assert!(p("-world w -sideswitch 5d").is_err());
        let a = p("-world w -linger 30 -teamempty 0").unwrap();
        assert_eq!(
            (a.server.game.linger_ms, a.server.game.team_empty_ms),
            (30_000, 0)
        );
        assert!(a.ignored.is_empty(), "{:?}", a.ignored);
        for hz in TICK_CHOICES {
            assert_eq!(
                p(&format!("-world w -tick {hz}")).unwrap().server.tick_hz,
                hz
            );
        }
        assert_eq!(p("-world w -tick 25").unwrap().ignored.len(), 1);
        assert!(p("-world w -playerbox 1,2").is_err());
        assert!(p("-world w -loadout 1,x").is_err());
        // DSV-32 (proposed): death state order and the respawn wait / fallback
        let a = p("-world w -deathstates spec -respawnwait 2000 -respawnfallback 9000").unwrap();
        let g = &a.server.game;
        assert_eq!(g.death_states, crate::game::DeathStates::Spec);
        assert_eq!(
            (g.respawn_wait_ms, g.respawn_fallback_ms),
            (Some(2000), 9000)
        );
        assert_eq!(
            p("-world w").unwrap().server.game.death_states,
            crate::game::DeathStates::Reference
        );
        assert!(p("-world w -deathstates odd").is_err());
        // DSV-30 (proposed): combat.* keys
        let a = p("-world w -combat combat.head_factor=3,body_factor=0.8,armor_absorb=0.25,max_armor=rom \
                   -combat zone_factors=off,start_armor=50,trace=off,trace_slack=4")
        .unwrap();
        let g = &a.server.game;
        assert_eq!((g.head_factor, g.body_factor), (3.0, 0.8));
        assert_eq!(
            (g.armor_absorb, g.max_armor, g.start_armor),
            (Some(0.25), None, Some(50.0))
        );
        assert!(!g.zone_factors_from_rom && !g.trace);
        assert_eq!(g.trace_slack, 4.0);
        assert!(p("-world w -combat nosuch=1").is_err());
        assert!(p("-world w -combat head_factor=x").is_err());
        assert!(p("-world w -combat head_factor").is_err());
        // the 2009 calibration (project decision): armour row 0, cost from the base damage; overridable
        let g = &p("-world w").unwrap().server.game;
        assert_eq!(
            (g.armor_gear, g.armor_cost_from_base),
            (crate::game::ArmorGear::Worn, true)
        );
        for (v, want) in [
            ("3", crate::game::ArmorGear::Row(3)),
            ("Worn", crate::game::ArmorGear::Worn),
            ("items", crate::game::ArmorGear::Items),
            ("OFF", crate::game::ArmorGear::Off),
        ] {
            let a = p(&format!("-world w -combat armor_gear={v},armor_cost=hit")).unwrap();
            assert_eq!(
                (a.server.game.armor_gear, a.server.game.armor_cost_from_base),
                (want, false)
            );
        }
        assert!(p("-world w -combat armor_gear=vest").is_err());
        assert!(p("-world w -combat armor_cost=all").is_err());
        let a = p("-world w -worldobjects all -zoneclass room -no9d").unwrap();
        assert_eq!(a.server.world_objects, crate::worldobj::WorldObjects::All);
        assert_eq!(a.server.zone_class, crate::worldobj::ZoneClass::Room);
        assert!(!a.server.game.send_9d);
        assert!(p("-world w -worldobjects some").is_err());
    }
}
