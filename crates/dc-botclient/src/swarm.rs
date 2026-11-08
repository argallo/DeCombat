//! `dc-botclient swarm`: a few bots on the player's own PC that join a public lobby with them (docs/BOTS.md).
//!
//! The command stays in the foreground and runs each bot as a child process of this binary. A bot needs no account: it
//! takes a seat of the lobby's bot pool at login (LG-318) and gives it back with its connection. A child ends when its
//! parent's pipe closes, so no bot outlives the command, however that ends. `swarm list` / `swarm stop` work from a
//! second terminal through a state file the command keeps fresh.
// DC-DEFAULT: BOT-102 (proposed, USER REQUEST 2026-11-02; amended by LG-318), see DC_DEFAULTS.md

use crate::tls;
use crate::lobby::{EXIT_NO_POOL, EXIT_POOL_ADDRESS, EXIT_POOL_FULL};
use serde_json::{json, Value};
use std::io::Write;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Most bots of one run (the lobby default of bots.pool_per_ip; a room holds 8: the player and seven).
pub const MAX_BOTS: usize = 7;
/// Default number of bots.
pub const DEFAULT_BOTS: usize = 3;
/// Between two bot starts: the lobby closes new connections while three TLS handshakes are in progress (LG-223).
pub const STAGGER: Duration = Duration::from_secs(3);
/// After a bot ended: before its next start, doubled up to [`RESTART_MAX`] while it keeps ending quickly.
const RESTART_MIN: Duration = Duration::from_secs(10);
const RESTART_MAX: Duration = Duration::from_secs(120);
/// With `--follow`: after this long, one hint that the nick may not match (nothing in the bot's output says a room was found).
const FOLLOW_HINT_AFTER: Duration = Duration::from_secs(180);
/// A bot that ran this long ended normally.
const RAN_WELL: Duration = Duration::from_secs(120);
/// A state file older than this belongs to a command that is gone.
const STATE_FRESH_SECS: u64 = 10;
/// A bot's log is started over above this size.
const LOG_MAX: u64 = 5_000_000;
/// The default port of a lobby.
const DEFAULT_PORT: u16 = 10000;

const USAGE: &str = "usage: dc-botclient swarm [--count <1..7>] [--server <host[:port]>] [--server-id <n>] [--channel <n>]
           [--room <any | part of a room's title>] [--follow <nick>] [--room-password <p>]
           [--skill <off|0..100|lo-hi>]
           [--rom <client folder>] [--mod-cache <dir>] [--mod-set-hash <hex>] [--content-hash <hex|0>]
           [--dir <folder>]
       dc-botclient swarm list [--dir <folder>]
       dc-botclient swarm stop [--dir <folder>]";

/// The command line of a run.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Options {
    pub count: usize,
    pub server: Option<String>,
    pub server_id: Option<u32>,
    pub channel: Option<u32>,
    /// None = any room with a player in it.
    pub room_title: Option<String>,
    pub follow: Option<String>,
    pub room_password: Option<String>,
    pub skill: String,
    pub rom: Option<PathBuf>,
    pub mod_cache: Option<PathBuf>,
    pub mod_set_hash: Option<String>,
    pub content_hash: Option<String>,
    pub dir: Option<PathBuf>,
}

/// Parses the flags of a run.
pub fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut o = Options {
        count: DEFAULT_BOTS,
        skill: "30-65".into(),
        ..Default::default()
    };
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        i += 1;
        let mut val = || -> Result<String, String> {
            let v = args
                .get(i)
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))?;
            i += 1;
            Ok(v)
        };
        match flag {
            "--count" => {
                let v = val()?;
                o.count = v
                    .parse()
                    .map_err(|_| format!("--count: {v:?} is not a number"))?;
            }
            "--server" => o.server = Some(val()?),
            "--server-id" => {
                let v = val()?;
                o.server_id = Some(v.parse().map_err(|_| format!("--server-id: {v:?}"))?);
            }
            "--channel" => {
                let v = val()?;
                o.channel = Some(v.parse().map_err(|_| format!("--channel: {v:?}"))?);
            }
            "--room" => {
                let v = val()?;
                o.room_title = (!v.eq_ignore_ascii_case("any")).then_some(v);
            }
            "--follow" => o.follow = Some(val()?),
            "--room-password" => o.room_password = Some(val()?),
            "--skill" => o.skill = val()?,
            "--prefix" | "--invite" | "--web" => {
                return Err(format!(
                    "{flag} is gone: bots take a seat of the lobby's bot pool and need no account of their own"
                ))
            }
            "--rom" => o.rom = Some(PathBuf::from(val()?)),
            "--mod-cache" => o.mod_cache = Some(PathBuf::from(val()?)),
            "--mod-set-hash" => o.mod_set_hash = Some(val()?),
            "--content-hash" => o.content_hash = Some(val()?),
            "--dir" => o.dir = Some(PathBuf::from(val()?)),
            _ => return Err(format!("unknown flag {flag}")),
        }
    }
    if !(1..=MAX_BOTS).contains(&o.count) {
        return Err(format!("--count: 1 to {MAX_BOTS} bots"));
    }
    Ok(o)
}

/// `host[:port]` with the lobby's default port.
pub fn with_port(server: &str) -> String {
    let s = server.trim();
    let has_port = s
        .rsplit_once(':')
        .is_some_and(|(_, p)| p.parse::<u16>().is_ok());
    if has_port {
        s.to_string()
    } else {
        format!("{s}:{DEFAULT_PORT}")
    }
}

fn resolve(s: &str) -> Result<SocketAddr, String> {
    s.to_socket_addrs()
        .map_err(|e| format!("{s}: {e}"))?
        .next()
        .ok_or_else(|| format!("{s}: no address"))
}

/// The folder of the state and the logs: `--dir`, `DC_BOT_DIR`, else `bots` in DeCombat's profile folder.
pub fn bots_dir(dir: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(d) = dir {
        return Ok(d.to_path_buf());
    }
    if let Some(d) = std::env::var_os("DC_BOT_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(d));
    }
    profile_dir()
        .map(|d| d.join("bots"))
        .ok_or_else(|| "no home folder: name one with --dir".to_string())
}

/// DeCombat's profile folder, where the client also keeps its known servers and mods (CLI-553, CLI-1286).
fn profile_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("DC_PROFILE_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    if let Some(d) = std::env::var_os("APPDATA").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d).join("DeCombat"));
    }
    if let Some(d) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d).join("decombat"));
    }
    let home = PathBuf::from(std::env::var_os("HOME").filter(|d| !d.is_empty())?);
    Some(if cfg!(target_os = "macos") {
        home.join("Library/Application Support/DeCombat")
    } else {
        home.join(".config/decombat")
    })
}

/// Writes `text` with `path` readable by the user alone (on Windows the profile folder's own rights do that).
fn write_private(path: &Path, text: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(&tmp)
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    f.write_all(text.as_bytes())
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The folder, private to the user on unix.
fn make_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir.join("logs")).map_err(|e| format!("{}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// The name of bot `n` in this run's logs and its report; the lobby's seat gives the name players see.
pub fn seat_label(n: usize) -> String {
    format!("seat{n}")
}

/// The flags of one bot.
pub fn bot_args(o: &Options, server: &str, label: &str, known: Option<&Path>) -> Vec<String> {
    let mut a: Vec<String> = [
        "--parent-pipe",
        "--lobby",
        server,
        "--pool",
        "--name",
        label,
        "--room",
        "any",
        "--duration",
        "86400",
        "--match-wait",
        "86400",
        "--shooter",
        "--hunt",
        "--respawn-after",
        "0",
        "--matches",
        "0",
        "--skill",
        &o.skill,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let mut add = |k: &str, v: String| {
        a.push(k.to_string());
        a.push(v);
    };
    match &o.follow {
        // BOT-102: the filler rule with one name: the bots sit only where that player sits
        Some(n) => add("--filler", format!("={n}")),
        // LG-318: every seat of the pool has the prefix: a member without it is a person
        None => add("--filler", dc_wire::loginmods::POOL_NICK_PREFIX.to_string()),
    }
    if let Some(t) = &o.room_title {
        add("--room-title", t.clone());
    }
    if let Some(p) = &o.room_password {
        add("--password", p.clone());
    }
    if let Some(v) = o.server_id {
        add("--server-id", v.to_string());
    }
    if let Some(v) = o.channel {
        add("--channel", v.to_string());
    }
    if let Some(v) = &o.rom {
        add("--rom", v.display().to_string());
    }
    if let Some(v) = &o.mod_cache {
        add("--mod-cache", v.display().to_string());
    }
    if let Some(v) = &o.mod_set_hash {
        add("--mod-set-hash", v.clone());
    }
    if let Some(v) = &o.content_hash {
        add("--content-hash", v.clone());
    }
    if let Some(v) = known {
        add("--known-servers", v.display().to_string());
    }
    a
}

/// One seat of the run.
struct Slot {
    number: usize,
    child: Option<Child>,
    started: Instant,
    next_start: Instant,
    backoff: Duration,
    restarts: u32,
    /// The last exit reason printed for this seat (BOT-102), so a repeat is not printed again.
    said: String,
}

/// The last non-empty line of a bot's log, for the console line of an exit.
fn last_log_line(path: &Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return "(no log)".into();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(4096)));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf)
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map_or_else(|| "(log empty)".into(), |l| l.chars().take(200).collect())
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_state(dir: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(dir.join("swarm.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// The state file of a command still running.
fn live_state(dir: &Path) -> Option<Value> {
    read_state(dir).filter(|v| {
        now_unix().saturating_sub(v["updated"].as_u64().unwrap_or(0)) <= STATE_FRESH_SECS
    })
}

fn write_state(dir: &Path, server: &str, slots: &[Slot]) {
    let bots: Vec<Value> = slots
        .iter()
        .map(|s| {
            let name = seat_label(s.number);
            let status = if s.child.is_some() {
                "running"
            } else {
                "waiting to start"
            };
            json!({
                "name": name,
                "pid": s.child.as_ref().map(|c| c.id()),
                "status": status,
                "restarts": s.restarts,
            })
        })
        .collect();
    let v = json!({
        "pid": std::process::id(),
        "server": server,
        "updated": now_unix(),
        "bots": bots,
    });
    let _ = write_private(&dir.join("swarm.json"), &v.to_string());
}

fn start_bot(
    exe: &Path,
    dir: &Path,
    o: &Options,
    server: &str,
    known: Option<&Path>,
    label: &str,
) -> Result<Child, String> {
    let log_path = dir.join("logs").join(format!("{label}.out"));
    if std::fs::metadata(&log_path).is_ok_and(|m| m.len() > LOG_MAX) {
        let _ = std::fs::remove_file(&log_path);
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("{}: {e}", log_path.display()))?;
    let err = log.try_clone().map_err(|e| e.to_string())?;
    Command::new(exe)
        .args(bot_args(o, server, label, known))
        .stdin(Stdio::piped())
        .stdout(log)
        .stderr(err)
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", exe.display()))
}

fn run(o: Options) -> Result<(), String> {
    let dir = bots_dir(o.dir.as_deref())?;
    make_dir(&dir)?;
    if let Some(v) = live_state(&dir) {
        return Err(format!(
            "bots already run here (for {}): stop them first (bots.bat stop / dc-botclient swarm stop)",
            v["server"].as_str().unwrap_or("?")
        ));
    }
    let last = dir.join("last_server.txt");
    let server = match &o.server {
        Some(s) => with_port(s),
        None => std::fs::read_to_string(&last)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or("name the lobby once with --server <host[:port]>")?,
    };
    resolve(&server)?;
    let _ = std::fs::write(&last, &server);
    // the client's own list of known servers: the same first-seen certificate rule (CLI-1470)
    let known = profile_dir().map(|d| d.join(tls::KNOWN_SERVERS_FILE));
    if let Some(k) = &known {
        if let Some(p) = k.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        tls::set_store(k.clone());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mod_cache = o.mod_cache.clone().or_else(|| {
        profile_dir()
            .map(|d| d.join("mods").join("unpacked"))
            .filter(|d| d.is_dir())
    });
    let o = Options { mod_cache, ..o };
    if o.rom.is_none() && std::env::var_os("CA_ROM").is_none() {
        eprintln!("warning: no client folder (--rom <folder> or CA_ROM): the bots stand still and hold fire");
    }
    let t0 = Instant::now();
    let mut slots: Vec<Slot> = (1..=o.count)
        .map(|n| Slot {
            number: n,
            child: None,
            started: t0,
            next_start: t0 + STAGGER * (n as u32 - 1),
            backoff: RESTART_MIN,
            restarts: 0,
            said: String::new(),
        })
        .collect();
    let stop_file = dir.join("stop");
    let _ = std::fs::remove_file(&stop_file);
    println!(
        "{} bot(s) for {server}; Ctrl-C or `dc-botclient swarm stop` ends them. Logs: {}",
        o.count,
        dir.join("logs").display()
    );
    let mut said_full = false;
    let mut said_address = false;
    let mut said_follow = false;
    let mut last_start = t0 - STAGGER;
    let mut state_at = t0 - Duration::from_secs(60);
    loop {
        if stop_file.exists() {
            break;
        }
        let now = Instant::now();
        let mut no_pool = false;
        for s in &mut slots {
            if let Some(c) = s.child.as_mut() {
                match c.try_wait() {
                    Ok(None) => {}
                    Ok(Some(status)) if status.code() == Some(EXIT_NO_POOL) => {
                        no_pool = true;
                    }
                    ended => {
                        match ended.ok().flatten().and_then(|x| x.code()) {
                            Some(EXIT_POOL_FULL) if !said_full => {
                                said_full = true;
                                println!("the lobby's bot seats are all taken: trying again until one is free");
                            }
                            Some(EXIT_POOL_ADDRESS) if !said_address => {
                                said_address = true;
                                println!("this address holds as many bot seats as the lobby allows: the others wait for one to free");
                            }
                            Some(EXIT_POOL_FULL | EXIT_POOL_ADDRESS) | None => {}
                            Some(code) => {
                                let label = seat_label(s.number);
                                let line = last_log_line(&dir.join("logs").join(format!("{label}.out")));
                                if s.said != line {
                                    // the wait printed is the one the next line of this arm computes
                                    let wait = if s.started.elapsed() >= RAN_WELL {
                                        RESTART_MIN
                                    } else {
                                        (s.backoff * 2).min(RESTART_MAX)
                                    };
                                    println!(
                                        "{label} ended (exit {code}), starts again in {} s: {line}",
                                        wait.as_secs()
                                    );
                                    s.said = line;
                                }
                            }
                        }
                        s.child = None;
                        s.restarts += 1;
                        s.backoff = if s.started.elapsed() >= RAN_WELL {
                            RESTART_MIN
                        } else {
                            (s.backoff * 2).min(RESTART_MAX)
                        };
                        s.next_start = now + s.backoff;
                    }
                }
            }
            // one start per STAGGER, so a lobby restart is not seven TLS handshakes in one tick (LG-223)
            if s.child.is_none() && now >= s.next_start && now >= last_start + STAGGER {
                let label = seat_label(s.number);
                let c = start_bot(&exe, &dir, &o, &server, known.as_deref(), &label)?;
                println!("{label} started");
                s.child = Some(c);
                s.started = now;
                last_start = now;
            }
        }
        if let Some(nick) = o.follow.as_deref() {
            if !said_follow && t0.elapsed() >= FOLLOW_HINT_AFTER {
                said_follow = true;
                println!(
                    "no room with {nick} seen yet (the name must match the in-game name exactly); still looking"
                );
            }
        }
        if no_pool {
            stop_all(&mut slots);
            let _ = std::fs::remove_file(dir.join("swarm.json"));
            return Err(format!(
                "{server} has no bot seats: its host turned them off (bots.pool_size) or runs an older build. Ask the host."
            ));
        }
        if state_at.elapsed() >= Duration::from_secs(2) {
            write_state(&dir, &server, &slots);
            state_at = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    stop_all(&mut slots);
    let _ = std::fs::remove_file(&stop_file);
    let _ = std::fs::remove_file(dir.join("swarm.json"));
    println!("bots stopped");
    Ok(())
}

/// Closes every pipe first, then gives all bots one shared two seconds, then kills the survivors.
fn stop_all(slots: &mut [Slot]) {
    let mut kids: Vec<Child> = slots.iter_mut().filter_map(|s| s.child.take()).collect();
    for c in &mut kids {
        drop(c.stdin.take());
    }
    let end = Instant::now() + Duration::from_secs(2);
    while Instant::now() < end {
        kids.retain_mut(|c| !matches!(c.try_wait(), Ok(Some(_))));
        if kids.is_empty() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    for mut c in kids {
        let _ = c.kill();
        let _ = c.wait();
    }
}

fn list(dir: &Path) -> Result<(), String> {
    let Some(v) = live_state(dir) else {
        println!("no bots running");
        return Ok(());
    };
    println!(
        "bots for {} (command pid {})",
        v["server"].as_str().unwrap_or("?"),
        v["pid"]
    );
    for b in v["bots"].as_array().into_iter().flatten() {
        println!(
            "  {}  {}  restarts {}",
            b["name"].as_str().unwrap_or("?"),
            b["status"].as_str().unwrap_or("?"),
            b["restarts"]
        );
    }
    println!("logs: {}", dir.join("logs").display());
    Ok(())
}

fn stop(dir: &Path) -> Result<(), String> {
    if live_state(dir).is_none() {
        println!("no bots running");
        return Ok(());
    }
    std::fs::write(dir.join("stop"), b"stop").map_err(|e| e.to_string())?;
    let end = Instant::now() + Duration::from_secs(25);
    while Instant::now() < end {
        if read_state(dir).is_none() {
            println!("bots stopped");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err("the bots did not stop in time".to_string())
}

/// `dc-botclient swarm ...`; the process's exit status.
pub fn main(args: &[String]) -> i32 {
    let r = match args.first().map(String::as_str) {
        Some("-h") | Some("--help") => {
            println!("{USAGE}");
            return 0;
        }
        Some(cmd @ ("list" | "stop")) => parse_options(&args[1..]).map(|o| (Some(cmd), o)),
        _ => parse_options(args).map(|o| (None, o)),
    };
    // the usage text is for a command line that does not parse; a runtime error stands alone
    let (cmd, o) = match r {
        Ok(x) => x,
        Err(e) => {
            eprintln!("dc-botclient swarm: {e}");
            eprintln!("{USAGE}");
            return 1;
        }
    };
    let r = match cmd {
        Some(c) => bots_dir(o.dir.as_deref()).and_then(|dir| if c == "list" { list(&dir) } else { stop(&dir) }),
        None => run(o),
    };
    match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("dc-botclient swarm: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc_swarm_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_count_is_capped_and_defaults_are_sensible() {
        let o = parse_options(&[]).unwrap();
        assert_eq!((o.count, o.skill.as_str()), (DEFAULT_BOTS, "30-65"));
        assert!(parse_options(&args(&["--count", "7"])).is_ok());
        assert!(parse_options(&args(&["--count", "8"])).is_err());
        assert!(parse_options(&args(&["--count", "0"])).is_err());
        for gone in ["--prefix", "--invite", "--web"] {
            assert!(parse_options(&args(&[gone, "x"])).unwrap_err().contains("is gone"));
        }
        assert_eq!(
            parse_options(&args(&["--room", "any"])).unwrap().room_title,
            None
        );
    }

    #[test]
    fn ports_default() {
        assert_eq!(with_port("example.org"), "example.org:10000");
        assert_eq!(with_port("example.org:27000"), "example.org:27000");
    }

    #[test]
    fn a_bot_asks_for_a_seat_and_follow_uses_the_exact_rule() {
        let o = Options {
            follow: Some("Zoe".into()),
            skill: "40".into(),
            server_id: Some(2),
            ..Default::default()
        };
        let a = bot_args(&o, "h:1", "seat1", None);
        assert!(a.iter().any(|x| x == "--pool"));
        assert!(!a.iter().any(|x| x == "--user"));
        assert!(a.windows(2).any(|w| w[0] == "--filler" && w[1] == "=Zoe"));
        assert!(a.windows(2).any(|w| w[0] == "--server-id" && w[1] == "2"));
        assert!(a.iter().any(|x| x == "--parent-pipe"));
        assert!(!a.iter().any(|x| x == "--login-password"));
        let o = Options {
            room_title: Some("fun".into()),
            ..o
        };
        let o = Options { follow: None, ..o };
        let a = bot_args(&o, "h:1", "seat1", None);
        assert!(a.windows(2).any(|w| w[0] == "--filler" && w[1] == "PoolBot"));
        assert!(a
            .windows(2)
            .any(|w| w[0] == "--room-title" && w[1] == "fun"));
    }

    #[test]
    fn a_stale_state_file_is_no_running_command() {
        let d = temp("state");
        std::fs::write(
            d.join("swarm.json"),
            json!({"updated": now_unix() - 60, "server": "h:1"}).to_string(),
        )
        .unwrap();
        assert!(live_state(&d).is_none());
        std::fs::write(
            d.join("swarm.json"),
            json!({"updated": now_unix(), "server": "h:1"}).to_string(),
        )
        .unwrap();
        assert!(live_state(&d).is_some());
        let _ = std::fs::remove_dir_all(&d);
    }
}
