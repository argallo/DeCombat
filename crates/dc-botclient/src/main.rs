//! `dc-botclient`: one headless 2009-protocol player.
//!
//! ```text
//! dc-botclient --lobby <host:port> --name <nick> --room <n|new|any> [--walk | --hunt] [--duration <s>] [--log <jsonl>]
//!              [--capture <jsonl>] [--report <json>] [--user <id>] [--start-after <n>] [--password <p>]
//!              [--login-password <pw>] [--mod-set-hash <hex>] [--content-hash <hex|0>]
//!              [--mode <n>] [--title <t>] [--max-players <n>] [--channel <id>] [--force-updates]
//!              [--state-form <n>] [--update-hz <f>] [--load-ms <n>] [--rom <dir>] [--no-rom] [--mod-cache <dir>]
//!              [--match-wait <s>] [--loss <permille>] [--loss-seed <n>] [--drop-over <bytes>] [--net <profile[,k=v..]>] [--no-aim-quat]
//!              [--join-timeout <s>] [--idle-exit <s>] [--respawn-after <ms>] [--shooter | --hold-fire] [--fire-interval <ms>]
//!              [--throw-every <ms>]
//!              [--hit-node <n>] [--skill <0..100|lo-hi|off>] [--goal <n|min>] [--gun-game] [--stay-after-result] [--matches <n>]
//!              [--filler <prefix>] [--pool]
//!              [--timeline <file>]   (timeline lines: team, host, setting, sync <nick> [ms], @<ms> <verb>)
//! dc-botclient --udp <host:port> --name <nick> [same game options]      (join a game server directly)
//! ```
//! `--lobby` is the auth address (the lobby address comes from the server list). `--password` is the room
//! password; `--login-password` (or env `DC_BOT_LOGIN_PASSWORD` when the flag is absent) is the account password the
//! login packet carries, for a lobby that checks passwords (`--check-password`; BOT-83); without either the bot
//! sends the harness text `pw`. `--mod-set-hash` (LG-271) is a modded lobby's published `mod_set_hash`
//! (server_list.json) and `--content-hash` the hash of its weapon content (the lobby's start log, `0` = none): the
//! login name then carries `;m=<hash>;c=<hash>`; without them it carries nothing, which only a vanilla lobby takes.
//! `--user` is the account name (default: the name in lower case). Exit status: 0 when the game
//! session ran and ended by the bot's own quit, the server's bye, or a dead link (`--idle-exit`, default 15 s without
//! a datagram, event `server_gone`; `--respawn-after` is a fallback respawn request when the server never armed the
//! client's wait), 1 on any failure, 2 on a usage error. The JSON report goes to
//! stdout (and to `--report`). `--shooter`: after each spawn the bot selects its primary weapon and reports a hit
//! on the nearest enemy every `--fire-interval` ms (default 150) until a kill names it; `--hold-fire` (the
//! default) never fires. `--goal` (host): the kill goal set after creating the room (`min` = the ROM mode row's
//! smallest goal). After the result screen (0x3f) the bot leaves the match unless `--stay-after-result`.
//! `--matches <n>` (default 1; 0 = until the lobby or the room ends): after a match the bot keeps its room seat,
//! readies again once the room is back in the waiting state (the host waits for its members and starts) and plays
//! the next match (BOT-62).
//! `--hunt` (with `--shooter`): walk toward the target while the line of sight in the ROM world is blocked.
//! `--skill <0..100>` (default 50): the shooter aims like a player: its body turns toward the target at a finite
//! rate, it waits a reaction delay after the target comes into view, fires bursts only with the aim within a
//! tolerance, and every shot carries an aim error plus the weapon's spread, so it hits only sometimes; a hit is
//! claimed only along a clear line in the loaded world (no ROM / world: it holds fire). `--skill off`: the instant
//! aim of the first shooter (every shot a hit on a clear line).

use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;

use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::{open_rom, run_bot, run_game_with, session_report, BotConfig, RoomChoice};
use dc_botclient::session::{BotSession, GameConfig};

const USAGE: &str = "usage: dc-botclient swarm --help     (a few bots that join a public lobby with you, docs/BOTS.md)
       dc-botclient (--lobby <host:port> | --udp <host:port>) --name <nick> [--room <n|new|any>]
       [--walk | --hunt] [--duration <s>] [--log <jsonl>] [--log-max <bytes>] [--log-transport] [--capture <jsonl>] [--report <json>] [--user <id>]
       [--start-after <n>] [--password <p>] [--login-password <pw>] [--mod-set-hash <hex>] [--content-hash <hex|0>]
       [--mode <n>] [--title <t>] [--max-players <n>] [--mod-cache <dir>] [--room-title <part>]
       [--parent-pipe] [--known-servers <file>] [--pool]   (--pool: a seat of the lobby's bot pool, no --name needed)
       [--channel <id>] [--server-id <n>]
       [--force-updates] [--state-form <n>] [--update-hz <f>] [--load-ms <n>] [--rom <dir>] [--no-rom]
       [--match-wait <s>] [--loss <permille>] [--loss-seed <n>] [--drop-over <bytes>] [--net <profile[,k=v..]>] [--no-aim-quat]
       [--join-timeout <s>] [--idle-exit <s>] [--respawn-after <ms>] [--shooter | --hold-fire] [--fire-interval <ms>]
       [--throw-every <ms>] [--grenades <chance 0..1>]
       [--hit-node <n>] [--skill <0..100|lo-hi|off>] [--goal <n|min>] [--gun-game] [--stay-after-result] [--matches <n>]
       [--timeline <file>]   (timeline lines: team, host, setting, sync <nick> [ms], @<ms> <verb>)";

struct Cli {
    /// BOT-102: end when standard input closes (the `swarm` command's bots).
    parent_pipe: bool,
    /// BOT-102: the client's list of known servers, for the first-seen certificate rule.
    known_servers: Option<PathBuf>,
    lobby: Option<SocketAddr>,
    udp: Option<SocketAddr>,
    cfg: BotConfig,
    log: Option<PathBuf>,
    log_max: u64,
    log_transport: bool,
    capture: Option<PathBuf>,
    report: Option<PathBuf>,
    start_after: usize,
    mode: i32,
    title: Option<String>,
    max_players: i32,
    room: String,
}

fn addr(s: &str) -> Result<SocketAddr, String> {
    s.to_socket_addrs()
        .map_err(|e| format!("{s}: {e}"))?
        .next()
        .ok_or_else(|| format!("{s}: no address"))
}

/// A `--mod-set-hash` / `--content-hash` value: 64 hex digits (lower-cased), or `0` where no content is allowed.
fn hash_arg(flag: &str, v: String, zero_ok: bool) -> Result<String, String> {
    let v = v.trim().to_ascii_lowercase();
    if (zero_ok && v == "0") || (v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit())) {
        Ok(v)
    } else {
        Err(format!(
            "{flag}: {v:?} is not a SHA-256 hash (64 hex digits)"
        ))
    }
}

fn parse(args: Vec<String>) -> Result<Cli, String> {
    let placeholder: SocketAddr = "127.0.0.1:0".parse().expect("literal address");
    let mut c = Cli {
        parent_pipe: false,
        known_servers: None,
        lobby: None,
        udp: None,
        cfg: BotConfig::new(placeholder, "Bot1"),
        log: None,
        log_max: 0,
        log_transport: false,
        capture: None,
        report: None,
        start_after: 2,
        mode: 0,
        title: None,
        max_players: 8,
        room: "any".into(),
    };
    let mut name: Option<String> = None;
    let mut pool = false;
    let mut skill_text: Option<String> = None;
    let mut timeline: Option<PathBuf> = None;
    let mut user: Option<String> = None;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut val = |flag: &str| it.next().ok_or_else(|| format!("{flag} needs a value"));
        let num = |flag: &str, v: String| -> Result<f64, String> {
            v.parse::<f64>()
                .map_err(|_| format!("{flag}: bad number {v:?}"))
        };
        match a.as_str() {
            "--lobby" => c.lobby = Some(addr(&val("--lobby")?)?),
            "--udp" => c.udp = Some(addr(&val("--udp")?)?),
            "--name" => name = Some(val("--name")?),
            "--user" => user = Some(val("--user")?),
            "--room" => c.room = val("--room")?,
            "--walk" => c.cfg.motion = MotionKind::Circle,
            "--hunt" => c.cfg.motion = MotionKind::Hunt,
            "--duration" => {
                c.cfg.duration =
                    Duration::from_secs_f64(num("--duration", val("--duration")?)?.max(0.0))
            }
            "--match-wait" => {
                c.cfg.match_wait =
                    Duration::from_secs_f64(num("--match-wait", val("--match-wait")?)?.max(0.0))
            }
            "--log" => c.log = Some(PathBuf::from(val("--log")?)),
            "--log-max" => c.log_max = num("--log-max", val("--log-max")?)?.max(0.0) as u64,
            "--log-transport" => c.log_transport = true,
            "--capture" => c.capture = Some(PathBuf::from(val("--capture")?)),
            "--report" => c.report = Some(PathBuf::from(val("--report")?)),
            "--start-after" => {
                c.start_after = num("--start-after", val("--start-after")?)? as usize
            }
            "--password" => c.cfg.password = val("--password")?,
            "--login-password" => c.cfg.login_password = Some(val("--login-password")?),
            // LG-271: the modded lobby's published mod-set hash (and its content hash) for the login
            "--mod-set-hash" => {
                c.cfg.mod_set_hash =
                    Some(hash_arg("--mod-set-hash", val("--mod-set-hash")?, false)?)
            }
            "--content-hash" => {
                c.cfg.content_hash = Some(hash_arg("--content-hash", val("--content-hash")?, true)?)
            }
            "--mode" => c.mode = num("--mode", val("--mode")?)? as i32,
            "--title" => c.title = Some(val("--title")?),
            "--max-players" => c.max_players = num("--max-players", val("--max-players")?)? as i32,
            "--channel" => c.cfg.channel = Some(num("--channel", val("--channel")?)? as u32),
            "--server-id" => {
                c.cfg.server_id = Some(num("--server-id", val("--server-id")?)? as u32)
            }
            "--force-updates" => c.cfg.force_updates = true,
            "--state-form" => {
                c.cfg.state_form = Some(num("--state-form", val("--state-form")?)? as u8)
            }
            "--update-hz" => c.cfg.update_hz = num("--update-hz", val("--update-hz")?)? as f32,
            "--load-ms" => c.cfg.load_ms = num("--load-ms", val("--load-ms")?)? as u32,
            "--rom" => c.cfg.rom = Some(PathBuf::from(val("--rom")?)),
            "--mod-cache" => c.cfg.mod_cache = Some(PathBuf::from(val("--mod-cache")?)),
            "--no-rom" => c.cfg.use_rom = false,
            "--no-aim-quat" => c.cfg.aim_quat = false,
            "--join-timeout" => {
                c.cfg.join_timeout = Duration::from_secs_f64(
                    num("--join-timeout", val("--join-timeout")?)?.clamp(1.0, 3600.0),
                )
            }
            "--idle-exit" => {
                c.cfg.idle_exit =
                    Duration::from_secs_f64(num("--idle-exit", val("--idle-exit")?)?.max(0.0))
            }
            "--respawn-after" => {
                c.cfg.respawn_after = Some(Duration::from_millis(
                    num("--respawn-after", val("--respawn-after")?)?.max(0.0) as u64,
                ))
            }
            "--shooter" => c.cfg.shooter = true,
            "--hold-fire" => c.cfg.shooter = false,
            "--fire-interval" => {
                c.cfg.fire_interval_ms =
                    num("--fire-interval", val("--fire-interval")?)?.clamp(1.0, 60_000.0) as u32
            }
            "--throw-every" => {
                c.cfg.throw_every_ms = Some(
                    num("--throw-every", val("--throw-every")?)?.clamp(500.0, 600_000.0) as u32,
                )
            }
            // BOT-147: the chance a sighting in grenade range is answered with the grenade (0 = never)
            "--grenades" => {
                c.cfg.grenade_chance = num("--grenades", val("--grenades")?)?.clamp(0.0, 1.0) as f32
            }
            "--hit-node" => c.cfg.hit_node = num("--hit-node", val("--hit-node")?)? as u8,
            // BOT-91: `<n>`, `off`, or `<lo>-<hi>` (one level per bot, picked from its name once the name is known)
            "--skill" => skill_text = Some(val("--skill")?),
            "--goal" => {
                let v = val("--goal")?;
                c.cfg.goal = Some(if v == "min" {
                    dc_botclient::hostroom::GoalChoice::Min
                } else {
                    dc_botclient::hostroom::GoalChoice::Value(num("--goal", v)? as u32)
                })
            }
            "--gun-game" => c.cfg.gun_game = true,
            "--possession" => c.cfg.possession = true,
            "--stay-after-result" => c.cfg.leave_after_result = false,
            "--matches" => c.cfg.matches = num("--matches", val("--matches")?)?.max(0.0) as u32,
            "--filler" => c.cfg.filler = Some(val("--filler")?),
            // LG-318: a seat of the lobby's bot pool: no account of its own
            "--pool" => pool = true,
            "--room-title" => c.cfg.room_title = Some(val("--room-title")?),
            "--parent-pipe" => c.parent_pipe = true,
            "--known-servers" => c.known_servers = Some(PathBuf::from(val("--known-servers")?)),
            "--timeline" => timeline = Some(PathBuf::from(val("--timeline")?)),
            "--loss" => {
                let v = num("--loss", val("--loss")?)? as u32;
                c.cfg.impairment.loss_out_permille = v;
                c.cfg.impairment.loss_in_permille = v;
            }
            "--drop-over" => {
                let v = num("--drop-over", val("--drop-over")?)? as usize;
                c.cfg.impairment.drop_over_bytes = Some(v);
            }
            // NET-20: the network lab's path (docs/NETLAB.md): `profile[,key=value...]`
            "--net" => {
                c.cfg.impairment.net = Some(dc_net_engine::impair::PathSpec::parse(&val("--net")?)?)
            }
            "--loss-seed" => {
                c.cfg.impairment.seed = num("--loss-seed", val("--loss-seed")?)? as u64
            }
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown option {other:?}")),
        }
    }
    let name = match name {
        Some(n) => n,
        None if pool => "seat".to_string(),
        None => return Err("--name is required".into()),
    };
    if c.cfg.login_password.is_none() {
        c.cfg.login_password = std::env::var("DC_BOT_LOGIN_PASSWORD")
            .ok()
            .filter(|p| !p.is_empty());
    }
    if c.lobby.is_none() && c.udp.is_none() {
        return Err("one of --lobby or --udp is required".into());
    }
    c.cfg.nick = name.clone();
    if let Some(v) = skill_text {
        let seed = c.cfg.nick.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
        });
        c.cfg.skill = match dc_botclient::skill::Skill::parse_level(&v, seed) {
            Some(l) => Some(l),
            None if v.trim().eq_ignore_ascii_case("off") => None,
            None => {
                return Err(format!(
                    "--skill: {v:?} is not a level, a range lo-hi or off"
                ))
            }
        };
    }
    if let Some(p) = &timeline {
        c.cfg.timeline = Some(dc_botclient::timeline::load(p, &name)?);
    }
    // a different loss pattern per bot name (FNV-1a of the name mixed into the seed)
    c.cfg.impairment.seed ^= name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    });
    c.cfg.user = if pool {
        String::new()
    } else {
        user.unwrap_or_else(|| name.to_ascii_lowercase())
    };
    if let Some(l) = c.lobby {
        c.cfg.auth = l;
    }
    // a timeline `host` line makes this bot the host whatever --room says
    if let Some((members, mode)) = c.cfg.timeline.as_ref().and_then(|t| t.host) {
        c.room = "new".into();
        c.start_after = members;
        c.mode = mode;
    }
    c.cfg.room = match c.room.as_str() {
        "new" => RoomChoice::New {
            max_players: c.max_players,
            title: c.title.clone().unwrap_or_else(|| format!("{name} room")),
            mode: c.mode,
            start_after: c.start_after.max(1),
        },
        "any" => RoomChoice::Any,
        n => RoomChoice::Number(
            n.parse()
                .map_err(|_| format!("--room: {n:?} is not new, any or a number"))?,
        ),
    };
    Ok(c)
}

fn main() {
    let all: Vec<String> = std::env::args().skip(1).collect();
    if all.first().is_some_and(|a| a == "swarm") {
        std::process::exit(dc_botclient::swarm::main(&all[1..]));
    }
    let c = match parse(all) {
        Ok(c) => c,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("dc-botclient: {e}");
            }
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    if c.parent_pipe {
        // BOT-102: no bot outlives the `swarm` command, however that ends
        let _ = std::thread::Builder::new().name("parent-pipe".into()).spawn(|| {
            let mut sink = Vec::new();
            let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut sink);
            std::process::exit(0);
        });
    }
    if let Some(p) = &c.known_servers {
        dc_botclient::tls::set_store(p.clone());
    }
    // the bot runs on a server ROM as well as on the full client folder: it reads only worlds, tables and the
    // first-person weapon models' clips, all kept there (BOT-82; the DSV-372 refusal is dc-client's alone)
    if c.cfg.shooter
        && c.cfg.skill.is_some()
        && dc_botclient::run::rom_dir(c.cfg.use_rom, c.cfg.rom.as_ref()).is_none()
    {
        eprintln!(
            "dc-botclient: warning: --shooter without a ROM (--rom <dir> or CA_ROM): no collision world and no \
             stream positions, so the bot holds fire"
        );
    }
    let rec = match Recorder::with_log_max(
        c.capture.as_deref(),
        c.log.as_deref(),
        &c.cfg.nick,
        c.log_max,
    ) {
        Ok(r) => {
            r.set_log_transport(c.log_transport);
            r
        }
        Err(e) => {
            eprintln!("dc-botclient: cannot open the log / capture: {e}");
            std::process::exit(2);
        }
    };
    let mut seat_refused = None;
    let (report, ok) = if let Some(server) = c.udp {
        let rom = open_rom(c.cfg.use_rom, c.cfg.rom.as_ref());
        let mut gc = GameConfig::new(&c.cfg.nick);
        gc.hello_tag = c.cfg.user.clone();
        gc.motion = c.cfg.motion;
        gc.force_updates = c.cfg.force_updates;
        gc.state_form = c.cfg.state_form;
        gc.update_hz = c.cfg.update_hz;
        gc.load_ms = c.cfg.load_ms;
        gc.aim_quat = c.cfg.aim_quat;
        gc.idle_exit_ms = c.cfg.idle_exit.as_millis().min(u32::MAX as u128) as u32;
        gc.join_timeout_ms = c.cfg.join_timeout.as_millis().min(u32::MAX as u128) as u32;
        gc.respawn_after_ms = c
            .cfg
            .respawn_after
            .map(|d| d.as_millis().min(u32::MAX as u128) as u32);
        gc.integrity_version = rom
            .as_ref()
            .and_then(dc_botclient::romworld::RomWorlds::integrity_version);
        gc.leave_after_result = c.cfg.leave_after_result;
        gc.timeline = c.cfg.timeline.clone();
        gc.shooter = c.cfg.shooter_config(&[], &rec);
        let mut client = BotSession::new(gc, rom, rec.clone());
        match run_game_with(&mut client, server, c.cfg.duration, &c.cfg.impairment, &rec) {
            Ok(()) => {
                let r = session_report(&client);
                let ok = r["stage"] == "Closed";
                (r, ok)
            }
            Err(e) => (serde_json::json!({"error": e.0}), false),
        }
    } else {
        match run_bot(&c.cfg, &rec) {
            Ok(r) => {
                let ok = r["stage"] == "Closed";
                (r, ok)
            }
            // a lobby connection that closed while no game link was up ends the run cleanly
            Err(e) if e.0.contains("closed while waiting") => {
                rec.event(
                    "server_gone",
                    serde_json::json!({"why": "lobby connection closed before a game link was up", "detail": e.0}),
                );
                (
                    serde_json::json!({"bot": c.cfg.nick, "stage": "Closed",
                        "end": format!("server gone: lobby closed ({})", e.0)}),
                    true,
                )
            }
            // BOT-158: the bot's own --match-wait ran out with no room to join: a normal end, not a failure
            Err(e) if e.0.starts_with(dc_botclient::lobby::NO_ROOM_PREFIX) => (
                serde_json::json!({"bot": c.cfg.nick, "stage": "Closed",
                    "end": format!("ran its wait: {}", e.0)}),
                true,
            ),
            Err(e) => {
                seat_refused = dc_botclient::lobby::exit_code_of(&e.0);
                (serde_json::json!({"bot": c.cfg.nick, "error": e.0}), false)
            }
        }
    };
    rec.event("final", serde_json::json!({"ok": ok}));
    rec.flush();
    let text = serde_json::to_string_pretty(&report).unwrap_or_default();
    if let Some(p) = &c.report {
        let _ = std::fs::write(p, &text);
    }
    println!("{text}");
    std::process::exit(seat_refused.unwrap_or(if ok { 0 } else { 1 }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(extra: &[&str]) -> Vec<String> {
        ["--lobby", "127.0.0.1:10000", "--name", "Bravo"]
            .iter()
            .chain(extra)
            .map(|s| s.to_string())
            .collect()
    }

    /// LG-271: `--mod-set-hash` / `--content-hash` reach the login options; a value that is no hash is a usage error.
    #[test]
    fn the_mod_hash_flags_are_parsed_into_the_login() {
        let c = parse(args(&[])).ok().unwrap();
        assert_eq!(c.cfg.mod_set_hash, None);
        let set = "AB".repeat(32);
        let c = parse(args(&["--mod-set-hash", &set, "--content-hash", "0"]))
            .ok()
            .unwrap();
        let lo = dc_botclient::run::login_options(&c.cfg);
        assert_eq!(
            lo.login_field(),
            format!("LOCAL:bravo;b=1;m={};c=0", "ab".repeat(32))
        );
        assert!(parse(args(&["--mod-set-hash", "0"])).is_err());
        assert!(parse(args(&["--mod-set-hash", "xyz"])).is_err());
        assert!(parse(args(&["--content-hash", &"g".repeat(64)])).is_err());
    }
}
