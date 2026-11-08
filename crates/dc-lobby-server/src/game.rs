//! Match start (state pushes, countdown, address push), late join, return to the room, match end, and the
//! dedicated-server process per room (stdin/stdout line contract).
// SPEC: PROTOCOL_2009.md §7 (lobby side of the game start); SPEC: DSERVER_PIPE.md (process and line contract)

use crate::db;
use crate::msg::{field, *};
use crate::rewards::{self, RewardCtx};
use crate::room::{
    RoomKey, RoomPhase, MEMBER_PLAYING, MEMBER_READY, MEMBER_STARTING, MEMBER_WAITING,
};
use crate::state::{Core, Deferred, HResult, Sid};
use crate::wire;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};

/// One `@player` line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlayerResult {
    pub uid: u32,
    pub kills: i64,
    pub deaths: i64,
    pub team: i64,
    pub exp: Option<i64>,
    pub gp: Option<i64>,
    pub team_kills: i64,
    pub headshots: i64,
    /// DSV-472: the server's own verdict for this player (Spy Hunt), `None` when the line carries no `win=`.
    pub win: Option<bool>,
    /// DSV-472: the Spy Hunt agent whose upload won.
    pub agent: bool,
    pub name: String,
}

/// One `@result` line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MatchResult {
    pub mode: String,
    pub winner: i64,
    pub alpha: i64,
    pub bravo: i64,
    pub limit: i64,
    pub rounds: String,
}

/// One `@net` line: a client's link window as the game server measured it (numbers of that window, not totals).
// SPEC: DSERVER_PIPE_addendum_net.md §1; DC-DEFAULT: LG-185 (proposed)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetReport {
    pub client: String,
    pub uid: u32,
    pub window_ms: u64,
    pub rtt_mean: f64,
    pub rtt_min: f64,
    pub rtt_max: f64,
    pub rtt_samples: u64,
    pub jitter_ms: f64,
    pub input_rate: f64,
    pub longest_gap_ms: u64,
    pub resends: u64,
    pub flow_drops: u64,
    pub datagrams_in: u64,
    pub datagrams_out: u64,
}

impl NetReport {
    /// The `k=v` tokens after `@net` (unknown keys ignored, missing or bad numbers 0); `None` without `client=`.
    pub fn from_tokens(tokens: &[&str]) -> Option<NetReport> {
        let m = kv(tokens);
        let f = |k: &str| {
            m.get(k)
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v >= 0.0)
                .unwrap_or(0.0)
        };
        let u = |k: &str| m.get(k).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        Some(NetReport {
            client: m.get("client")?.clone(),
            uid: m.get("uid").and_then(|v| v.parse().ok()).unwrap_or(0),
            window_ms: u("ms"),
            rtt_mean: f("rtt"),
            rtt_min: f("rtt_min"),
            rtt_max: f("rtt_max"),
            rtt_samples: u("rtt_n"),
            jitter_ms: f("jitter"),
            input_rate: f("input"),
            longest_gap_ms: u("gap"),
            resends: u("resends"),
            flow_drops: u("drops"),
            datagrams_in: u("in"),
            datagrams_out: u("out"),
        })
    }
}

/// Server -> lobby events.
// SPEC: DSERVER_PIPE.md §4
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Ready {
        port: String,
        world: String,
    },
    Join {
        client: String,
        uid: Option<u32>,
    },
    Player(PlayerResult),
    Result(MatchResult),
    Leave(String),
    End,
    Crash(String),
    /// `@hisec`: a player picked up a HiSec case (DeCombat addition HS-13 of dc-dserver).
    Hisec(crate::hisec::Pickup),
    /// CRATE-3: a Camo Crate picked up.
    Crate(crate::camo_crate::Pickup),
    /// `@net`: one client's link window (DeCombat addition DSV-411 of dc-dserver).
    Net(NetReport),
    /// `@seat_refused <uid> noticket`: a client too old for seat tickets asked for this account's seat (LG-327).
    SeatRefused(u32),
    /// `@` event this server does not use (`@connect`, `@teamswap`, ...).
    Other(String),
    /// Console text (no leading `@`).
    Console(String),
}

fn kv(tokens: &[&str]) -> BTreeMap<String, String> {
    tokens
        .iter()
        .filter_map(|t| t.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Parses one stdout line (whitespace split; unknown keys ignored).
// SPEC: DSERVER_PIPE.md §4 (parsing rules), §4.3 (tolerant @join: tokens 1-2 only)
pub fn parse_event(line: &str) -> Event {
    let line = line.trim_end_matches(['\r', '\n']);
    if !line.starts_with('@') {
        return Event::Console(line.to_string());
    }
    let toks: Vec<&str> = line.split_whitespace().collect();
    let head = toks.first().copied().unwrap_or("");
    match head {
        "@ready" => {
            let m = kv(&toks[1..]);
            Event::Ready {
                port: m.get("port").cloned().unwrap_or_default(),
                world: m.get("world").cloned().unwrap_or_default(),
            }
        }
        "@join" => Event::Join {
            client: toks.get(1).map(|s| s.to_string()).unwrap_or_default(),
            uid: toks.get(2).and_then(|s| s.parse().ok()),
        },
        "@player" => {
            // `name=` is the last key and runs to the end of the line
            let (head, name) = match line.find(" name=") {
                Some(i) => (&line[..i], line[i + 6..].to_string()),
                None => (line, String::new()),
            };
            let t: Vec<&str> = head.split_whitespace().collect();
            let m = kv(&t[1..]);
            let n = |k: &str| m.get(k).and_then(|v| v.parse::<i64>().ok());
            Event::Player(PlayerResult {
                uid: n("uid").unwrap_or(0).clamp(0, u32::MAX as i64) as u32,
                kills: n("kills").unwrap_or(0).max(0),
                deaths: n("deaths").unwrap_or(0).max(0),
                team: n("team").unwrap_or(0),
                exp: n("exp"),
                gp: n("gp"),
                team_kills: n("tk").unwrap_or(0).max(0),
                headshots: n("hs").unwrap_or(0).max(0),
                win: n("win").map(|v| v != 0),
                agent: n("agent").unwrap_or(0) != 0,
                name,
            })
        }
        "@result" => {
            let m = kv(&toks[1..]);
            let n = |k: &str| m.get(k).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
            Event::Result(MatchResult {
                mode: m.get("mode").cloned().unwrap_or_default(),
                winner: n("win"),
                alpha: n("alpha"),
                bravo: n("bravo"),
                limit: n("limit"),
                rounds: m.get("rounds").cloned().unwrap_or_default(),
            })
        }
        "@leave" => Event::Leave(toks.get(1).map(|s| s.to_string()).unwrap_or_default()),
        "@end" => Event::End,
        "@crash" => Event::Crash(line.get(6..).unwrap_or("").trim().to_string()),
        "@net" => match NetReport::from_tokens(&toks[1..]) {
            Some(n) => Event::Net(n),
            None => Event::Other(line.to_string()),
        },
        "@seat_refused" => match toks.get(1).and_then(|s| s.parse().ok()) {
            Some(uid) => Event::SeatRefused(uid),
            None => Event::Other(line.to_string()),
        },
        "@hisec" => match crate::hisec::pickup_from_line(line) {
            Some(p) => Event::Hisec(p),
            None => Event::Other(line.to_string()),
        },
        "@crate" => match crate::camo_crate::pickup_from_line(line) {
            Some(p) => Event::Crate(p),
            None => Event::Other(line.to_string()),
        },
        _ => Event::Other(line.to_string()),
    }
}

/// `user <uid> <sid> <key> <team 0|1> <exp> <nick>` (team 0-based, spaces in the nick become `_`).
// SPEC: DSERVER_PIPE.md §3.1
pub fn user_line(uid: u32, sid: u64, key: u32, team_1based: u32, exp: i64, nick: &str) -> String {
    let nick: String = nick
        .chars()
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .collect();
    let nick = if nick.is_empty() {
        "_".to_string()
    } else {
        nick
    };
    format!(
        "user {} {} {} {} {} {}",
        uid,
        sid,
        key,
        team_1based.saturating_sub(1).min(1),
        exp.max(0),
        nick
    )
}

/// How the lobby reaches a game server: its own child process, or one a remote game host runs (HOST_AGENT.md).
// DC-DEFAULT: LG-301 (proposed) one pipe contract over either link
pub enum Link {
    /// A child of this lobby (DSERVER_PIPE.md §2); the readers take `stdout` / `stderr` (net.rs `watch_game`).
    Local {
        child: Option<Child>,
        stdin: Option<ChildStdin>,
        stdout: Option<ChildStdout>,
        stderr: Option<ChildStderr>,
    },
    /// A game server of a `dc-host` agent: its pipe lines travel over the agent's control connection.
    Remote(RemoteLink),
}

impl Default for Link {
    /// A local link without a process (a record the tests make, a game whose process is gone).
    fn default() -> Self {
        Link::Local {
            child: None,
            stdin: None,
            stdout: None,
            stderr: None,
        }
    }
}

/// The remote side of a game: the agent, the address it advertises and the connection its lines go out on.
pub struct RemoteLink {
    /// The agent's name (`[[hosts.agents]]`).
    pub agent: String,
    /// The address advertised in the game-start packet (the agent's `public_address` or its peer address).
    pub address: String,
    /// The port of `started` (or `running`); None until then.
    pub port: Option<u16>,
    pub pid: Option<u32>,
    /// The agent connection the lines go out on; None while the agent is offline (lines are lost then).
    pub tx: Option<std::sync::mpsc::Sender<crate::hosts::HostOut>>,
    /// False once `quit <gid>` went out (the stdin of a local child is closed then).
    pub stdin_open: bool,
    /// False once the agent said `exited` / `failed`.
    pub alive: bool,
}

impl Link {
    pub fn child_mut(&mut self) -> Option<&mut Child> {
        match self {
            Link::Local { child, .. } => child.as_mut(),
            Link::Remote(_) => None,
        }
    }

    pub fn take_child(&mut self) -> Option<Child> {
        match self {
            Link::Local { child, .. } => child.take(),
            Link::Remote(_) => None,
        }
    }

    /// The readers of a local child (taken once).
    pub fn take_output(&mut self) -> (Option<ChildStdout>, Option<ChildStderr>) {
        match self {
            Link::Local { stdout, stderr, .. } => (stdout.take(), stderr.take()),
            Link::Remote(_) => (None, None),
        }
    }

    pub fn pid(&self) -> Option<u32> {
        match self {
            Link::Local { child, .. } => child.as_ref().map(|c| c.id()),
            Link::Remote(r) => r.pid,
        }
    }

    pub fn remote(&self) -> Option<&RemoteLink> {
        match self {
            Link::Remote(r) => Some(r),
            Link::Local { .. } => None,
        }
    }

    pub fn remote_mut(&mut self) -> Option<&mut RemoteLink> {
        match self {
            Link::Remote(r) => Some(r),
            Link::Local { .. } => None,
        }
    }

    /// A process is (or may still be) running behind this link.
    pub fn has_process(&self) -> bool {
        match self {
            Link::Local { child, .. } => child.is_some(),
            Link::Remote(r) => r.alive,
        }
    }
}

/// A dedicated server process owned by one room start.
pub struct Game {
    pub id: u64,
    pub room: RoomKey,
    pub port: u16,
    pub map: u32,
    pub mode: u32,
    pub link: Link,
    pub ready: bool,
    /// The match is over (result stored, `@end`, crash or quit): the port may be reused.
    pub over: bool,
    pub players: Vec<PlayerResult>,
    /// `@join` client id -> account (the header form's tokens 1 and 2).
    pub clients: BTreeMap<String, u32>,
    /// Accounts whose client sent `@leave` and has not joined again: they were gone before the round end.
    pub left: std::collections::BTreeSet<u32>,
    pub result_done: bool,
    pub entry_keys: BTreeMap<u32, u32>,
    /// LG-327: the seat ticket per account: made at its first `user` line, kept for a rejoin, gone with this match.
    pub seat_tickets: BTreeMap<u32, dc_wire::seatticket::Ticket>,
    /// LG-327: the accounts already told that their client is too old for this match.
    pub seat_told: std::collections::BTreeSet<u32>,
    /// A reap (forced stop after the grace period) is already scheduled.
    pub reap_scheduled: bool,
    /// Per-room working directory the lobby created itself (no `--dserver-logdir`); removed with the game.
    // SPEC: DSERVER_PIPE addendum e2e C4 (removed when the game is reaped)
    pub tmp_workdir: Option<PathBuf>,
    /// HiSec case keys already delivered for this match (one inbox row per case, HS-6).
    pub hisec_keys: std::collections::BTreeSet<u32>,
    /// The newest `@net` window of each client (by client id) and when it arrived (LG-185).
    pub net: BTreeMap<String, (std::time::Instant, NetReport)>,
}

impl Game {
    /// One pipe line to the game server (DSERVER_PIPE.md §3): its stdin, or `line <gid> <text>` to its agent.
    pub fn write_line(&mut self, line: &str) -> bool {
        match &mut self.link {
            Link::Local { stdin, .. } => match stdin.as_mut() {
                Some(w) => w
                    .write_all(line.as_bytes())
                    .and_then(|_| w.write_all(b"\n"))
                    .and_then(|_| w.flush())
                    .is_ok(),
                None => false,
            },
            Link::Remote(r) => match (&r.tx, r.stdin_open) {
                (Some(tx), true) => tx
                    .send(crate::hosts::HostOut::Line(format!(
                        "line {} {}",
                        self.id, line
                    )))
                    .is_ok(),
                _ => false,
            },
        }
    }

    /// The game server still takes pipe lines.
    pub fn stdin_open(&self) -> bool {
        match &self.link {
            Link::Local { stdin, .. } => stdin.is_some(),
            Link::Remote(r) => r.stdin_open,
        }
    }

    /// Closes the game server's stdin: a local child's pipe, or `quit <gid>` to its agent (which closes the stdin
    /// and kills the server after its grace period, HOST_AGENT.md §3).
    pub fn close_stdin(&mut self) {
        let id = self.id;
        match &mut self.link {
            Link::Local { stdin, .. } => *stdin = None,
            Link::Remote(r) => {
                if r.stdin_open {
                    r.stdin_open = false;
                    if let Some(tx) = &r.tx {
                        let _ = tx.send(crate::hosts::HostOut::Line(format!("quit {id}")));
                    }
                }
            }
        }
    }

    /// Stops the game server now: kills a local child; asks the agent to quit it (its grace kill follows).
    pub fn kill(&mut self) {
        if let Some(c) = self.link.child_mut() {
            let _ = c.kill();
        } else {
            self.close_stdin();
        }
    }
}

/// True when a UDP bind on 127.0.0.1 succeeds (a 0.0.0.0-only probe misses a loopback holder on Windows).
// SPEC: DSERVER_PIPE.md §2 "Port choice" (HARNESS: lobby_harness::test_stale_port)
pub fn udp_port_free(port: u16) -> bool {
    udp_port_free_on(std::net::Ipv4Addr::LOCALHOST, port)
}

/// True when a UDP bind on `ip` succeeds. An address this host does not have (the advertised address of a
/// forwarded setup) cannot be probed: the 127.0.0.1 probe answers then.
pub fn udp_port_free_on(ip: std::net::Ipv4Addr, port: u16) -> bool {
    match std::net::UdpSocket::bind((ip, port)) {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AddrNotAvailable && !ip.is_loopback() => {
            udp_port_free(port)
        }
        Err(_) => false,
    }
}

/// The address the game server will bind, which is the one the port probe must test: the `-bindip` of
/// `--dserver-args`, else the one [`bindip_for`] adds, else 127.0.0.1 (the server's default bind, probed on
/// loopback because a 0.0.0.0-only probe misses a loopback holder on Windows). A relay that holds the ADVERTISED
/// port on another address (a capture tap on 127.0.0.1 forwarding to a server on 127.0.0.2) is then no holder: the
/// next start of the room keeps the relayed port instead of moving to a port nothing relays (LG-11, playtest report:
/// the second match of a session timed out on the join).
// DC-DEFAULT: LG-11 the port probe uses the game server's own bind address
pub fn probe_ip(gameip: &str, extra_args: &str) -> std::net::Ipv4Addr {
    let mut it = extra_args.split_whitespace();
    while let Some(a) = it.next() {
        if a.eq_ignore_ascii_case("-bindip") {
            return it
                .next()
                .and_then(|v| v.parse().ok())
                .filter(|ip: &std::net::Ipv4Addr| !ip.is_unspecified())
                .unwrap_or(std::net::Ipv4Addr::LOCALHOST);
        }
    }
    bindip_for(gameip, extra_args)
        .and_then(|v| v.parse().ok())
        .unwrap_or(std::net::Ipv4Addr::LOCALHOST)
}

/// Port choice for a new game server: the preferred port when free; else, when one of OUR finished games holds
/// it, that game is stopped and its port reused; else the first free port of the CD-10 range that none of our
/// live games uses. `ours` lists (port, game id, match over) of our games.
// SPEC: DSERVER_PIPE.md §2; DC-DEFAULT: CD-10 port range
pub fn choose_port(
    preferred: u16,
    range: (u16, u16),
    ours: &[(u16, u64, bool)],
    free: &dyn Fn(u16) -> bool,
    stop: &mut dyn FnMut(u64) -> bool,
) -> Option<u16> {
    let live = |p: u16| ours.iter().any(|&(q, _, over)| q == p && !over);
    if preferred != 0 && !live(preferred) {
        if free(preferred) {
            return Some(preferred);
        }
        if let Some(&(_, id, _)) = ours.iter().find(|&&(q, _, over)| q == preferred && over) {
            if stop(id) && free(preferred) {
                return Some(preferred);
            }
        }
    }
    (range.0..=range.1).find(|&p| p != 0 && !ours.iter().any(|&(q, _, _)| q == p) && free(p))
}

/// The `-idle` value: seconds, never negative (the server refuses a negative number).
// SPEC: DSERVER_PIPE addendum e2e B1
pub fn idle_arg(idle: i64) -> String {
    idle.max(0).to_string()
}

/// The address the game server binds: the `-bindip` of `--dserver-args` (0.0.0.0 included), else the one
/// [`bindip_for`] adds, else 127.0.0.1 (the server default).
pub fn dserver_bind(gameip: &str, extra_args: &str) -> std::net::Ipv4Addr {
    let mut it = extra_args.split_whitespace();
    while let Some(a) = it.next() {
        if a.eq_ignore_ascii_case("-bindip") {
            return it
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or(std::net::Ipv4Addr::LOCALHOST);
        }
    }
    bindip_for(gameip, extra_args)
        .and_then(|v| v.parse().ok())
        .unwrap_or(std::net::Ipv4Addr::LOCALHOST)
}

/// The `-content` path of the game-server launch: the mod set's content document, `None` for vanilla content.
// DC-DEFAULT: DSV-478 (proposed)
pub fn content_arg(mods: Option<&crate::modhost::ModClient>) -> Option<&std::path::Path> {
    mods.and_then(|m| m.content_file.as_deref())
}

/// The `-visibility` value of the game-server launch: `off` when a loaded mod declares `world.full_visibility`.
// DC-DEFAULT: MOD-22 (proposed), project decision
pub fn culling_arg(mods: Option<&crate::modhost::ModClient>) -> Option<&'static str> {
    mods.and_then(|m| m.culling_off_by.as_ref()).map(|_| "off")
}

/// The Hacks choices the loaded set offers its rooms (LG-274): `dc_wire::roomhacks::WALLHACKS` when a mod declares
/// `world.full_visibility`, `NO_RECOIL` when one declares `combat.no_recoil`; 0 for vanilla.
// DC-DEFAULT: LG-274 (proposed), project decision
pub fn hacks_offered(mods: Option<&crate::modhost::ModClient>) -> u32 {
    use dc_wire::roomhacks::{NO_RECOIL, WALLHACKS};
    mods.map_or(0, |m| {
        (if m.culling_off_by.is_some() {
            WALLHACKS
        } else {
            0
        }) | (if m.no_recoil_by.is_some() {
            NO_RECOIL
        } else {
            0
        })
    })
}

/// The game-server flags of a room's Hacks setting (after the operator's own, so they win): with any choice
/// offered, `-visibility off` for a room with wallhacks and `-visibility on` without (a "no-recoil only" room keeps
/// its culling although the set turns culling off elsewhere), and `-combat cone_check=off` for a room with
/// no-recoil (its shots leave along the aim, outside the 2009 cone). Nothing offered: no flag (vanilla, or a set
/// that declares neither; MOD-22's culling flag needs `world.full_visibility`, which offers wallhacks).
// DC-DEFAULT: DSV-479 (proposed), project decision
pub fn hacks_args(offered: u32, value: u32) -> Vec<String> {
    use dc_wire::roomhacks::{NO_RECOIL, WALLHACKS};
    if offered == 0 {
        return Vec::new();
    }
    let culling = if value & WALLHACKS != 0 { "off" } else { "on" };
    let mut out = vec!["-visibility".to_string(), culling.to_string()];
    if value & NO_RECOIL != 0 {
        out.push("-combat".to_string());
        out.push("cone_check=off".to_string());
    }
    out
}

/// LG-306: the value of `lobby.servers[].features` that offers Hardcore rooms.
pub const HARDCORE_FEATURE: &str = "hardcore";

/// LG-307: the value of `lobby.servers[].features` that offers the Possession Arms.
pub const POSSESSION_FEATURE: &str = "possession";

/// LG-307 / DSV-483: the game server's flag of a Possession room; never for another entry's room.
pub fn possession_args(st: &Core, key: RoomKey) -> Vec<String> {
    if st
        .rooms
        .get(&key)
        .is_some_and(|r| r.possession && !r.gun_game)
        && crate::servers::channel_has_feature(st, key.channel, POSSESSION_FEATURE)
    {
        vec!["-possession".to_string()]
    } else {
        Vec::new()
    }
}

/// LG-306 / DSV-482: the game server's flag of a Hardcore room (after the operator's flags, so it wins); never for a
/// room of an entry without the feature.
pub fn hardcore_args(st: &Core, key: RoomKey) -> Vec<String> {
    if st.rooms.get(&key).is_some_and(|r| r.hardcore)
        && crate::servers::channel_has_feature(st, key.channel, HARDCORE_FEATURE)
    {
        vec![
            "-combat".to_string(),
            format!("health_scale={}", dc_wire::roomhardcore::HEALTH_SCALE),
        ]
    } else {
        Vec::new()
    }
}

/// CRATE-2 (amended 2026-11-07): the game server's `-hisec` flag for the operator's `hisec.camo_share`, after the
/// operator's own flags so that the lobby key wins; nothing while the key is unset (the server's default applies).
pub fn hisec_args(st: &Core) -> Vec<String> {
    match st.rules.hisec.camo_share {
        Some(n) => vec!["-hisec".to_string(), format!("camo_share={n}")],
        None => Vec::new(),
    }
}

/// The flags of room `key`'s Hacks setting ([`hacks_args`]).
pub fn room_hacks_args(st: &Core, key: RoomKey) -> Vec<String> {
    let value = st.rooms.get(&key).map_or(0, |r| r.hacks);
    hacks_args(hacks_offered(st.mods.as_ref()), value)
}

/// The address to pass as `-bindip`: the advertised game address when it is a non-loopback IPv4 address and the
/// operator did not already give `-bindip` in `--dserver-args`. Names and loopback addresses give none.
// SPEC: DSERVER_PIPE addendum e2e B2
pub fn bindip_for(gameip: &str, extra_args: &str) -> Option<String> {
    if extra_args
        .split_whitespace()
        .any(|a| a.eq_ignore_ascii_case("-bindip"))
    {
        return None;
    }
    let ip: std::net::Ipv4Addr = gameip.trim().parse().ok()?;
    if ip.is_loopback() {
        None
    } else {
        Some(ip.to_string())
    }
}

fn python() -> String {
    for k in ["PYTHON", "CA_PYTHON"] {
        if let Ok(v) = std::env::var(k) {
            if !v.trim().is_empty() {
                return v;
            }
        }
    }
    "python".into()
}

fn stop_child(st: &mut Core, id: u64) -> bool {
    let Some(g) = st.games.get_mut(&id) else {
        return false;
    };
    if let Some(c) = g.link.child_mut() {
        let _ = c.kill();
        let _ = c.wait();
    }
    let _ = g.link.take_child();
    g.over = true;
    let port = g.port;
    st.note(&format!(
        "game {}: stale server stopped to reuse port {}",
        id, port
    ));
    true
}

/// Spawns the dedicated server of a room start. Returns the game id or the reason it failed.
pub(crate) fn spawn(st: &mut Core, key: RoomKey) -> Result<u64, String> {
    // LG-305: the ladder of this start (it may fit the goal, so before the room line)
    crate::gungame::prepare_start(st, key)?;
    let (map, mode, goal, ff, arms) = {
        let r = st.rooms.get(&key).ok_or("room gone")?;
        (r.map, r.mode, r.goal, r.ff_on, r.weapon_limit)
    };
    // DC-DEFAULT: MAP-36 (proposed; MAPKIT_PLAN.md D12) a clan room on a community index does not start: its
    // world is the ROM clan table's (`clan::world`), which is no community world (the room stays waiting, the
    // caller logs this reason); `on_setting` bounds a clan room's pick by the ROM rows, so this guards a room
    // whose index was set another way
    if crate::clan::is_clan_channel(key.channel) {
        if let Some(m) = st.maps_of_channel(key.channel).community_of(map) {
            return Err(format!(
                "community map \"{}\" (mod '{}') cannot start in a clan room: clan rooms play ROM maps only \
                 (D12)",
                m.key(),
                m.record.mod_id
            ));
        }
    }
    // SPEC: HOST_AGENT.md §1 the match of a room starts on its channel's server entry's host ("" = here)
    let host = crate::servers::host_of_channel(st, key.channel);
    if !host.is_empty() {
        return spawn_remote(st, key, &host);
    }
    let preferred = {
        let base = st.opts.gameport as u32;
        let p = if st.opts.port_per_room {
            base + key.channel * 100 + key.no
        } else {
            base
        };
        (p & 0xffff) as u16
    };
    // the ports of OUR local game servers (a remote host's ports are on another machine)
    let ours: Vec<(u16, u64, bool)> = st
        .games
        .values()
        .filter(|g| g.link.remote().is_none())
        .map(|g| (g.port, g.id, g.over))
        .collect();
    let range = (st.rules.dserver_port_lo, st.rules.dserver_port_hi);
    let probe = probe_ip(&st.opts.gameip, &st.opts.dserver_args);
    let free = move |p: u16| udp_port_free_on(probe, p);
    let port = choose_port(preferred, range, &ours, &free, &mut |id| stop_child(st, id))
        .ok_or("no free UDP port for the game server")?;
    let world = crate::clan::world(st, key).unwrap_or_else(|| {
        st.maps_of_channel(key.channel)
            .world_for(map, mode, &st.opts.dserver_world)
    });
    if world.is_empty() {
        return Err("no world for the room: set CA_ROM or --dserver-world".into());
    }
    // MAP-36: a community map's launch flags (clan rooms never choose one, so `clan::world` cannot be theirs);
    // MAP-82: the table of the room's channel, so a featured entry's map launches from that entry's pack cache
    let community = st
        .maps_of_channel(key.channel)
        .community_of(map)
        .map(|m| (m.key().to_string(), m.record.world.clone()));
    let exe = st.opts.dserver_exe.clone();
    let exe = if exe.is_relative() {
        std::env::current_dir().map(|d| d.join(&exe)).unwrap_or(exe)
    } else {
        exe
    };
    let is_py = exe
        .extension()
        .map(|e| e.eq_ignore_ascii_case("py"))
        .unwrap_or(false);
    // SPEC: DSERVER_PIPE.md §2 a `.py` server runs through Python (PYTHON / CA_PYTHON / python)
    let mut cmd = if is_py {
        let mut c = Command::new(python());
        c.arg(&exe);
        c
    } else {
        Command::new(&exe)
    };
    let id = st.next_game;
    st.next_game += 1;
    cmd.arg("-port").arg(port.to_string());
    cmd.arg("-world").arg(&world);
    cmd.arg("-guid").arg(&st.opts.dserver_guid);
    // SPEC: DSERVER_PIPE addendum e2e B1 (`-idle` is an unsigned number of seconds; 0 = no idle stop)
    cmd.arg("-idle").arg(idle_arg(st.opts.dserver_idle));
    let mut own_workdir = None;
    let workdir: PathBuf = match &st.opts.dserver_logdir {
        Some(d) => {
            cmd.arg("-log")
                .arg(d.join(format!("dserver_c{}_r{}_g{}.log", key.channel, key.no, id)));
            d.clone()
        }
        None => {
            let d = std::env::temp_dir().join(format!(
                "dc-lobby-dserver-{}-{}",
                std::process::id(),
                id
            ));
            own_workdir = Some(d.clone());
            d
        }
    };
    let _ = std::fs::create_dir_all(&workdir);
    // SPEC: DSERVER_PIPE addendum e2e B2 (a non-loopback advertised address needs -bindip)
    if let Some(ip) = bindip_for(&st.opts.gameip, &st.opts.dserver_args) {
        cmd.arg("-bindip").arg(ip);
    }
    for a in st.opts.dserver_args.split_whitespace() {
        cmd.arg(a);
    }
    // MAP-36 / MAPKIT_PLAN.md D11: a community map is loaded from its file in the mod cache (absolute: the server
    // runs in its own working directory) and announced to the clients as the record's `@mod:` path, byte for
    // byte (the client's ModAssets lookup is case-sensitive); the brushes of the world are created on every
    // client. After `--dserver-args`, so these win (dc-dserver takes the last value of a flag).
    if let Some((_, announce)) = &community {
        cmd.arg("-worldfile").arg(&world);
        cmd.arg("-announce").arg(announce);
        cmd.arg("-worldobjects").arg("world");
    }
    // MOD-21 / DSV-478: the mod set's weapon content (only when the set patches any row; vanilla passes nothing)
    // LG-313: on a featured entry its own set's content, so a pack never reaches another entry's game server
    let content_of = crate::servers::entry_set_of_channel(st, key.channel)
        .map(|e| &e.client)
        .or(st.mods.as_ref());
    if let Some(p) = content_arg(content_of) {
        cmd.arg("-content").arg(p);
    }
    // MOD-22: a mod that declares world.full_visibility turns the match's culling off (after the operator's args);
    // LG-274 / DSV-479: per room, by the room's Hacks setting
    let hacks = room_hacks_args(st, key);
    if hacks.is_empty() {
        if let Some(v) = culling_arg(st.mods.as_ref()) {
            cmd.arg("-visibility").arg(v);
        }
    }
    for a in &hacks {
        cmd.arg(a);
    }
    // LG-305 / DSV-481: a Gun Game room's ladder
    for a in crate::gungame::args(st, key) {
        cmd.arg(a);
    }
    for a in hardcore_args(st, key) {
        cmd.arg(a);
    }
    for a in possession_args(st, key) {
        cmd.arg(a);
    }
    for a in hisec_args(st) {
        cmd.arg(a);
    }
    // The game server runs in its own working directory (SPEC: DSERVER_PIPE addendum e2e C4), so a ROM named
    // relative to the lobby's directory (a launcher's "pz", playtest report: "5 then unready") reaches it
    // absolute, whether the lobby got it as --rom or only through its own CA_ROM environment.
    let rom = st
        .rom
        .dir
        .clone()
        .or_else(|| std::env::var_os("CA_ROM").map(PathBuf::from));
    if let Some(rom) = rom {
        cmd.env("CA_ROM", absolute_rom(&rom));
    }
    // DC-DEFAULT: LG-130 (proposed) the game server runs in its own console process group, so the operator's
    // Ctrl+C reaches the lobby only and the lobby ends the match through the pipe (`quit`)
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.current_dir(&workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot start {}: {}", exe.display(), e))?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    st.note(&format!(
        "game {}: room {}-{} server pid {} port {} world {}{}",
        id,
        key.channel,
        key.no,
        child.id(),
        port,
        if world.is_empty() {
            "(default)"
        } else {
            &world
        },
        match &community {
            Some((k, a)) => format!(" (community map {k}, announced as {a})"),
            None => String::new(),
        }
    ));
    let mut g = Game {
        id,
        room: key,
        port,
        map,
        mode,
        link: Link::Local {
            child: Some(child),
            stdin,
            stdout,
            stderr,
        },
        ready: false,
        over: false,
        players: Vec::new(),
        clients: BTreeMap::new(),
        left: Default::default(),
        result_done: false,
        entry_keys: BTreeMap::new(),
        seat_tickets: BTreeMap::new(),
        seat_told: Default::default(),
        reap_scheduled: false,
        tmp_workdir: own_workdir,
        hisec_keys: Default::default(),
        net: Default::default(),
    };
    let line = crate::clan::room_line(st, key).unwrap_or_else(|| {
        // DC-DEFAULT: GMD-2 (clarification) the operator's round-time fallback (`room.round_secs.*`);
        // MAP-37 a community map's record time
        st.maps_of_channel(key.channel)
            .room_line(map, mode, goal, ff, arms, &st.rules.round_clock)
    });
    g.write_line(&line);
    st.games.insert(id, g);
    st.defer(Deferred::WatchGame { game: id });
    Ok(id)
}

/// The arguments a remote game server gets from the lobby's `--dserver-args`: without the flags the agent sets itself
/// (`-port`, `-world`, `-guid`, `-idle`, `-log`, `-bindip`, `-worldfile`, `-announce`, `-worldobjects`, each with its
/// value; HOST_AGENT.md §5).
pub fn remote_args(extra_args: &str) -> Vec<String> {
    const OWN: [&str; 9] = [
        "-port",
        "-world",
        "-guid",
        "-idle",
        "-log",
        "-bindip",
        "-worldfile",
        "-announce",
        "-worldobjects",
    ];
    let mut out = Vec::new();
    let mut it = extra_args.split_whitespace();
    while let Some(a) = it.next() {
        if OWN.iter().any(|o| a.eq_ignore_ascii_case(o)) {
            it.next();
            continue;
        }
        out.push(a.to_string());
    }
    out
}

/// The `start` request of HOST_AGENT.md §3: port 0 (the agent picks one of its range), the room's world, the
/// identity token, the idle time, the log name the local path uses, the lobby's extra arguments.
///
/// LG-313: `content` = (hash, document) of a featured entry's weapon content; `None` leaves the request as before.
/// LG-431: an empty document leaves it out and names the hash alone (the document went as `content` lines).
/// MAP-83: `pack` = a featured entry's community map; `None` leaves the request as before.
pub fn start_json(
    world: &str,
    guid: &str,
    idle: i64,
    log: &str,
    args: &[String],
    content: Option<(&str, &str)>,
    pack: Option<&RemoteWorld>,
) -> String {
    use dc_mod_lua::JsonValue as J;
    let mut o = BTreeMap::new();
    o.insert("port".to_string(), J::Number(0.0));
    o.insert("world".to_string(), J::String(world.to_string()));
    o.insert("guid".to_string(), J::String(guid.to_string()));
    o.insert("idle".to_string(), J::Number(idle.max(0) as f64));
    o.insert("log".to_string(), J::String(log.to_string()));
    o.insert(
        "args".to_string(),
        J::Array(args.iter().map(|a| J::String(a.clone())).collect()),
    );
    if let Some((hash, doc)) = content {
        o.insert("content_hash".to_string(), J::String(hash.to_string()));
        if !doc.is_empty() {
            o.insert("content".to_string(), J::String(doc.to_string()));
        }
    }
    if let Some(p) = pack {
        let mut w = BTreeMap::new();
        w.insert("sha256".to_string(), J::String(p.sha256.clone()));
        w.insert("size".to_string(), J::Number(p.size as f64));
        w.insert(
            "url".to_string(),
            J::String(format!("/mods/{}.zip", p.sha256)),
        );
        o.insert("world_pack".to_string(), J::Object(w));
        o.insert("world_file".to_string(), J::String(p.world_file.clone()));
        o.insert("announce".to_string(), J::String(p.announce.clone()));
    }
    J::Object(o).to_json_string()
}

/// MAP-83: what a remote `start` says about a featured entry's community map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteWorld {
    pub sha256: String,
    /// The zip's size in bytes.
    pub size: u64,
    /// The zip on this machine (the bytes of the `pack` lines).
    pub zip: PathBuf,
    /// The world's path inside the pack.
    pub world_file: String,
    /// The record's `world`, verbatim.
    pub announce: String,
}

/// MAP-83: the pack behind community map `m` of an entry's set.
fn remote_world(
    set: &crate::modset::EntrySet,
    m: &crate::maps::CommunityMap,
) -> Result<RemoteWorld, String> {
    let (sha, zip) = set.pack_of(&m.record.mod_id).ok_or_else(|| {
        format!(
            "community map \"{}\": mod '{}' is not in the entry's set",
            m.key(),
            m.record.mod_id
        )
    })?;
    let size = std::fs::metadata(&zip)
        .map_err(|e| format!("community map \"{}\": {}: {e}", m.key(), zip.display()))?
        .len();
    let world_file = m.record.world_rel_path().ok_or_else(|| {
        format!(
            "community map \"{}\": world {:?} is not the mod's own asset",
            m.key(),
            m.record.world
        )
    })?;
    Ok(RemoteWorld {
        sha256: sha.to_string(),
        size,
        zip,
        world_file: world_file.to_string(),
        announce: m.record.world.clone(),
    })
}

/// The raw bytes of one `pack <sha> <base64>` line: 16000 base64 characters, so the line stays under the 16 KiB limit
/// of HOST_AGENT.md section 3, and a multiple of 3, so each line decodes alone.
// DC-DEFAULT: MAP-84 (proposed)
pub const PACK_CHUNK: usize = 12_000;

/// MAP-84: the `pack` lines of a zip, checked against its name first (a zip replaced after the start is refused here
/// rather than after the whole transfer).
pub fn pack_lines(w: &RemoteWorld) -> Result<Vec<String>, String> {
    let bytes = std::fs::read(&w.zip).map_err(|e| format!("{}: {e}", w.zip.display()))?;
    if dc_rom::sha256_hex(&bytes) != w.sha256 {
        return Err(format!("{} no longer hashes to its name", w.zip.display()));
    }
    Ok(bytes
        .chunks(PACK_CHUNK)
        .map(|c| format!("pack {} {}", w.sha256, crate::identity::b64std(c)))
        .collect())
}

/// LG-431: the `content <hash> <base64>` lines of a weapon content document, in order. Each carries [`PACK_CHUNK`] raw
/// bytes, so a line stays under the agent's 16 KiB limit (the same size as a `pack` line) and each decodes alone.
pub fn content_lines(hash: &str, doc: &str) -> Vec<String> {
    doc.as_bytes()
        .chunks(PACK_CHUNK)
        .map(|c| format!("content {hash} {}", crate::identity::b64std(c)))
        .collect()
}

/// The most JSON a `start` line carries: an agent drops a line over 16 KiB (HOST_AGENT.md §3).
// DC-DEFAULT: LG-313 (proposed)
const START_LINE_ROOM: usize = 16 * 1024 - 64;

/// The room start on a remote game host: `start <gid> <json>` to its agent, then the room line as a `line`. The
/// agent's `started` gives the port, its `event` lines run through [`on_line`] as a local server's stdout does.
// DC-DEFAULT: LG-301 (proposed) an offline host fails the start like a local spawn failure
fn spawn_remote(st: &mut Core, key: RoomKey, host: &str) -> Result<u64, String> {
    let (map, mode, goal, ff, arms) = {
        let r = st.rooms.get(&key).ok_or("room gone")?;
        (r.map, r.mode, r.goal, r.ff_on, r.weapon_limit)
    };
    // DC-DEFAULT: MAP-36 (proposed; MAPKIT_PLAN.md D12) a lobby-wide (modded) set's community map does not start on
    // a remote game host (the room stays waiting, the caller logs this reason); MAP-83: a featured entry's does, its
    // pack goes to the agent
    let pack = match st.maps_of_channel(key.channel).community_of(map) {
        None => None,
        Some(m) => match crate::servers::entry_set_of_channel(st, key.channel) {
            Some(set) => Some(remote_world(set, m)?),
            None => {
                return Err(format!(
                    "community map \"{}\" (mod '{}') cannot start on remote game host {host}: only a featured \
                     entry's maps reach a remote host (D12); move the room to a channel of this lobby's own machine",
                    m.key(),
                    m.record.mod_id
                ))
            }
        },
    };
    let (tx, address, takes_content, takes_content_lines, takes_packs) =
        match st.hosts.online.get(host) {
            Some(a) => (
                a.tx.clone(),
                a.address.clone(),
                a.takes_content,
                a.takes_content_lines,
                a.takes_packs,
            ),
            None => return Err(format!("game host {host} is offline")),
        };
    // DC-DEFAULT: MAP-83 (proposed) as for weapon content: an agent that cannot take the pack is refused before anything
    if pack.is_some() && !takes_packs {
        return Err(format!(
            "game host {host} is too old for this entry's community maps (no packs=1 in its hello); update its agent"
        ));
    }
    // LG-313: a featured entry's weapon content reaches the agent's machine (it has no lobby data folder), LG-431: as
    // `content` lines before the `start` when the agent takes them, else in the request
    let content = crate::servers::entry_set_of_channel(st, key.channel)
        .map(|e| &e.client.content)
        .filter(|c| !c.is_empty())
        .map(|c| (c.hash(), c.to_json()));
    if content.is_some() && !takes_content {
        return Err(format!(
            "game host {host} is too old for this entry's weapon content; update its agent"
        ));
    }
    // MAP-83: the lobby's path of a community world means nothing on the host: the record's name goes, and the agent
    // puts its own path of the unpacked pack on the command line
    let world = match &pack {
        Some(p) => p.announce.clone(),
        None => crate::clan::world(st, key).unwrap_or_else(|| {
            st.maps_of_channel(key.channel)
                .world_for(map, mode, &st.opts.dserver_world)
        }),
    };
    if world.is_empty() {
        return Err("no world for the room: set CA_ROM or --dserver-world".into());
    }
    let id = st.next_game;
    // LG-274 / DSV-479: the room's Hacks flags after the operator's
    let mut args = remote_args(&st.opts.dserver_args);
    args.extend(room_hacks_args(st, key));
    // LG-305 / DSV-481: a Gun Game room's ladder
    args.extend(crate::gungame::args(st, key));
    args.extend(hardcore_args(st, key));
    args.extend(possession_args(st, key));
    args.extend(hisec_args(st));
    let json = start_json(
        &world,
        &st.opts.dserver_guid,
        st.opts.dserver_idle,
        &format!("dserver_c{}_r{}_g{}.log", key.channel, key.no, id),
        &args,
        content.as_ref().map(|(h, d)| {
            (
                h.as_str(),
                if takes_content_lines { "" } else { d.as_str() },
            )
        }),
        pack.as_ref(),
    );
    if json.len() > START_LINE_ROOM {
        return Err(format!(
            "this entry's weapon content is too large for a start request ({} bytes); game host {host} takes it only as lines (no contentlines=1 in its hello): update its agent",
            json.len()
        ));
    }
    // DC-DEFAULT: MAP-84 (proposed) the pack goes once per connection unless the agent said `have`: all its lines are
    // queued here under the state lock, so they reach the agent whole and before this `start`
    if let Some(p) = pack.as_ref().filter(|p| {
        !st.hosts
            .online
            .get(host)
            .is_some_and(|a| a.packs.contains(&p.sha256))
    }) {
        let lines = pack_lines(p)?;
        let n = lines.len();
        for l in lines {
            if tx.send(crate::hosts::HostOut::Line(l)).is_err() {
                return Err(format!("game host {host} is offline"));
            }
        }
        if let Some(a) = st.hosts.online.get_mut(host) {
            a.packs.insert(p.sha256.clone());
        }
        st.note(&format!(
            "host {host}: map pack {} ({} bytes) sent in {n} line(s)",
            p.sha256, p.size
        ));
    }
    // DC-DEFAULT: LG-431 (proposed) the weapon content goes like a pack: once per connection unless the agent said
    // `have <hash>`, queued here under the state lock so the lines reach the agent whole and before this `start`
    if let Some((hash, doc)) = content.as_ref().filter(|_| takes_content_lines) {
        if !st
            .hosts
            .online
            .get(host)
            .is_some_and(|a| a.packs.contains(hash))
        {
            let lines = content_lines(hash, doc);
            let n = lines.len();
            for l in lines {
                if tx.send(crate::hosts::HostOut::Line(l)).is_err() {
                    return Err(format!("game host {host} is offline"));
                }
            }
            if let Some(a) = st.hosts.online.get_mut(host) {
                a.packs.insert(hash.clone());
            }
            st.note(&format!(
                "host {host}: weapon content {hash} ({} bytes) sent in {n} line(s)",
                doc.len()
            ));
        }
    }
    if tx
        .send(crate::hosts::HostOut::Line(format!("start {id} {json}")))
        .is_err()
    {
        return Err(format!("game host {host} is offline"));
    }
    st.next_game += 1;
    st.note(&format!(
        "game {}: room {}-{} server asked of host {} ({}) world {}{}",
        id,
        key.channel,
        key.no,
        host,
        address,
        world,
        match &pack {
            Some(p) => format!(
                " (community map in pack {}, file {})",
                p.sha256, p.world_file
            ),
            None => String::new(),
        }
    ));
    let mut g = Game {
        id,
        room: key,
        port: 0,
        map,
        mode,
        link: Link::Remote(RemoteLink {
            agent: host.to_string(),
            address,
            port: None,
            pid: None,
            tx: Some(tx),
            stdin_open: true,
            alive: true,
        }),
        ready: false,
        over: false,
        players: Vec::new(),
        clients: BTreeMap::new(),
        left: Default::default(),
        result_done: false,
        entry_keys: BTreeMap::new(),
        seat_tickets: BTreeMap::new(),
        seat_told: Default::default(),
        reap_scheduled: false,
        tmp_workdir: None,
        hisec_keys: Default::default(),
        net: Default::default(),
    };
    let line = crate::clan::room_line(st, key).unwrap_or_else(|| {
        // DC-DEFAULT: GMD-2 (clarification) the operator's round-time fallback (`room.round_secs.*`)
        st.maps_of_channel(key.channel)
            .room_line(map, mode, goal, ff, arms, &st.rules.round_clock)
    });
    g.write_line(&line);
    st.games.insert(id, g);
    st.defer(Deferred::WatchGame { game: id });
    Ok(id)
}

/// What the log calls a game's server: the executable, or the remote host.
fn server_label(st: &Core, game: u64) -> String {
    match st.games.get(&game).and_then(|g| g.link.remote()) {
        Some(r) => format!("host {}", r.agent),
        None => st.opts.dserver_exe.display().to_string(),
    }
}

/// Highest equipment slot of the 0x323 view (15 slots, 0..=14) that the `loadout=` token carries.
// SPEC: PROTOCOL_2009.md §4.2 0x323 (15 equipment slots); DSERVER_PIPE.md §12.A (slots 0..3 weapons, 4..9 / 14 gear
// and backpack, 10..13 backpack weapons)
const LOADOUT_LAST_SLOT: i64 = 14;

/// Trailing `user` line tokens describing what the player wears: `loadout=<slot>:<item>,...` (every equipped slot
/// 0..=14 of the 0x323 view: weapon slots 0..3, gear 4..8, backpack 9, backpack weapon slots 10..13, slot 14; ROM
/// item numbers, ascending slot order; absent when nothing is equipped) and `face=<n>`
/// (the stored face index), with `parts=` between them when an equipped weapon carries options (LDO-2,
/// `parts_token`). The character record has no class, body or skin field yet, so `char=` and `skin=` are
/// not sent. Each token starts with a space.
// DC-DEFAULT: LDO-1 (proposed; rust3 extension: every equipped slot 0..=14, so a backpack weapon and the gear reach
// the game server) the lobby tells the game server the equipped slots and the face; the server resolves item
// numbers to its own tables and ignores slots it cannot grant; config `dserver.loadout_tokens` (default true) switches the tokens off
pub fn loadout_tokens(st: &Core, account: u32, c: &crate::db::Character) -> String {
    if !st.rules.dserver_loadout_tokens {
        return String::new();
    }
    let mut out = String::new();
    if let Ok(items) = crate::db::items(&st.db, account) {
        // LG-314: a pack item never reaches a game server of a plain entry
        let items = crate::inventory::visible_items(st, account, items);
        // LG-407: a free weapon of a Weapon Free Pass reaches the game server as an equipped slot
        let items = crate::freepass::equipped_view(st, account, items);
        let limit = st
            .rooms
            .values()
            .find(|r| r.member(account).is_some())
            .map_or(0, |r| r.weapon_limit);
        let items = match_items(st, limit, &items);
        let items = rule_items(st, limit, items);
        let mut w: Vec<(i64, i64)> = items
            .iter()
            .filter(|i| (0..=LOADOUT_LAST_SLOT).contains(&i.slot))
            .map(|i| (i.slot, i.item_no))
            .collect();
        w.sort();
        if !w.is_empty() {
            let v: Vec<String> = w.iter().map(|(s, n)| format!("{s}:{n}")).collect();
            out.push_str(&format!(" loadout={}", v.join(",")));
        }
        out.push_str(&parts_token(st, &items));
    }
    out.push_str(&format!(" face={}", c.face.max(0)));
    out
}

/// The account's items (`items` = every item of the account) as a match under weapon limit `limit` equips them: when
/// the primary in equipment slot 0 is missing or the limit does not grant it, the first owned, unexpired primary
/// weapon of the inventory (ascending unique id) that the limit grants takes slot 0 for this match, with its own
/// attached options; the failing primary is left out and the substitute's old slot (a backpack slot) is vacated. The
/// stored loadout is not changed. Without a limit, without the ROM weapon types, or when no inventory weapon passes,
/// the items are returned as they are (LG-66 refuses Ready, as the 2009 client did; LG-342 tells the player why).
// DC-DEFAULT: LG-67 (proposed, project decision "if you pick snipers only and didn't have a sniper in your
// first slot it starts you with a knife") no 2009 evidence: the 2009 client's limit check cshell+0x160f70 ->
// cshell+0x226dd0 only refuses Ready; the grant rule is `dc_bute::arms::match_grants` (DSV-476)
pub fn match_items(st: &Core, limit: u32, items: &[db::Item]) -> Vec<db::Item> {
    let mut out = items.to_vec();
    if limit == 0 || st.rom.weapon_kinds.is_empty() {
        return out;
    }
    let kind = |i: &db::Item| weapon_type(st, i.item_no);
    let passes = |t: i32| {
        dc_bute::arms::slot_class(t) == Some(0) && dc_bute::arms::match_grants(limit, t, false)
    };
    let primary = out
        .iter()
        .position(|i| i.slot == 0 && (0..1000).contains(&i.item_no));
    if let Some(p) = primary {
        // a weapon whose type the table does not know passes (the game server grants it too)
        if kind(&out[p]).is_none_or(passes) {
            return out;
        }
    }
    let now = crate::clock::now_ticks();
    let mut order: Vec<usize> = (0..out.len()).collect();
    order.sort_by_key(|&i| out[i].uid);
    let Some(sub) = order.into_iter().find(|&i| {
        Some(i) != primary
            && out[i].parent_uid <= 0
            && (out[i].end_time == 0 || out[i].end_time > now)
            && kind(&out[i]).is_some_and(passes)
    }) else {
        return out;
    };
    if let Some(p) = primary {
        out[p].slot = -1;
    }
    out[sub].slot = 0;
    out
}

/// The item table's `ItemType` of weapon item `item_no` (below 1000); a camo variant (VAR-1) has its base gun's type,
/// since it has no item row of its own. `None` for a number the table does not know and for other items.
// DC-DEFAULT: VAR-1 (proposed) type lookups go through the base gun
pub fn weapon_type(st: &Core, item_no: i64) -> Option<i32> {
    if !(0..1000).contains(&item_no) {
        return None;
    }
    st.rom
        .weapon_kinds
        .get(&crate::inventory::base_weapon(st, item_no))
        .copied()
}

/// Whether equipment slot `slot` holds a weapon: a weapon slot 0..3 or a backpack weapon slot 10..13.
// SPEC: DSERVER_PIPE.md §12.A (slots 0..3 weapons, 10..13 backpack weapons)
pub fn is_weapon_equip_slot(slot: i64) -> bool {
    (0..=3).contains(&slot) || dc_bute::arms::BACKPACK_EQUIP_SLOTS.contains(&slot)
}

/// The match's items (after [`match_items`]) with every equipped weapon that the room's weapon rule `limit` keeps
/// out of the match taken out of its slot (its attached options go with it, they are filed under an equipped weapon
/// only), so the game server's `loadout=` / `parts=` tokens name only what the rule allows: the game server's own
/// rule (`dc_bute::arms::match_grants_in_slot`, DSV-610) then has nothing left to drop, and a game server of an older
/// build, which let a weapon without an item row through, gets no such weapon either. A variant counts as its base
/// gun. Without a rule or without the ROM item table the items are returned as they are. The stored loadout is not
/// changed.
// DC-DEFAULT: LG-419 (proposed, OWNER REPORT 2026-11-05 "no backpack option for a game mode still lets you bring your
// backpack item, same with pistols only")
pub fn rule_items(st: &Core, limit: u32, mut items: Vec<db::Item>) -> Vec<db::Item> {
    if limit == 0 || st.rom.weapon_kinds.is_empty() {
        return items;
    }
    for i in items.iter_mut() {
        if is_weapon_equip_slot(i.slot)
            && (0..1000).contains(&i.item_no)
            && !dc_bute::arms::match_grants_in_slot(limit, weapon_type(st, i.item_no), Some(i.slot))
        {
            i.slot = -1;
        }
    }
    items
}

/// The options attached to one equipped weapon, by option kind in the order of `loadout::OPTION_KINDS` (muzzle,
/// sight, magazine): the option's ROM item number, `None` = nothing attached of that kind.
pub type WeaponOptions = [Option<i64>; 3];

/// The options attached to the account's equipped weapons (`items` = every item of the account): one entry per
/// equipped weapon item (slots 0..=14 of the 0x323 view, item number below 1000) that carries at least one option
/// whose ROM kind is known, in ascending slot order. An option of an unknown kind (no ROM item table, or a number the
/// table does not know) is left out: the receiver could not place it in a socket. Two options of one kind on one
/// weapon (the attach rule keeps one, CD-79): the lowest unique id wins.
// SPEC: PROTOCOL_2009.md §3.3b (+8 parent unique id), §4.2 0x336 / 0x33a (option kinds 0x11 / 0x12 / 0x13)
// EVIDENCE: cshell+0x287930, cshell+0x226700 (the 2009 client files each option whose parent is set under its
// weapon by kind: 0x11 / 0x12 / 0x13 -> the weapon record's three option fields)
// OBSERVED: capture 20261025_153705_rust7 (the user's `UP_PLAYER_INFO` names the attached
// sight of the M4A1 in the second option field; the lobby DB row: option item with parent = the weapon's uid)
// DC-DEFAULT: LDO-2 (proposed) the lobby tells the game server each equipped weapon's attachments
pub fn weapon_options(st: &Core, items: &[db::Item]) -> Vec<(i64, WeaponOptions)> {
    let mut out: Vec<(i64, WeaponOptions)> = Vec::new();
    let mut weapons: Vec<&db::Item> = items
        .iter()
        .filter(|i| (0..=LOADOUT_LAST_SLOT).contains(&i.slot) && (0..1000).contains(&i.item_no))
        .collect();
    weapons.sort_by_key(|i| (i.slot, i.uid));
    for wpn in weapons {
        let mut opts: Vec<&db::Item> = items
            .iter()
            .filter(|o| o.parent_uid == wpn.uid && o.uid != wpn.uid)
            .collect();
        opts.sort_by_key(|o| o.uid);
        let mut set: WeaponOptions = [None; 3];
        for o in opts {
            let kind = st
                .rom
                .items
                .as_ref()
                .map(|t| t.option_kind(o.item_no as i32))
                .unwrap_or(-1);
            if let Some(k) = crate::loadout::OPTION_KINDS.iter().position(|x| *x == kind) {
                set[k].get_or_insert(o.item_no);
            }
        }
        if set.iter().any(Option::is_some) && !out.iter().any(|(s, _)| *s == wpn.slot) {
            out.push((wpn.slot, set));
        }
    }
    out
}

/// The `parts=<slot>:<muzzle>/<sight>/<magazine>,...` token (each part the attached option's ROM item number, -1 =
/// none), or empty when no equipped weapon carries an option. Starts with a space.
// SPEC: DSERVER_PIPE_addendum_parts.md §1 (`parts=` token)
// DC-DEFAULT: LDO-2 (proposed)
pub fn parts_token(st: &Core, items: &[db::Item]) -> String {
    let v: Vec<String> = weapon_options(st, items)
        .iter()
        .map(|(slot, set)| {
            let p: Vec<String> = set
                .iter()
                .map(|o| o.map_or("-1".to_string(), |n| n.to_string()))
                .collect();
            format!("{slot}:{}", p.join("/"))
        })
        .collect();
    if v.is_empty() {
        String::new()
    } else {
        format!(" parts={}", v.join(","))
    }
}

/// The rank number of `exp` by the 2009 client's own rule over the level table's experience bounds (`bounds[i]` =
/// the ROM `Experience<i>` value): the first row whose bound is above the experience; 0 when none is (also past the
/// last row). The client reads at most 255 rows (the row count is a byte) and compares unsigned.
// EVIDENCE: cshell+0x267f10 (`for i < count(u8 at +0x1650): if exp < atoi(bound[i]) return i;` then `return 0`),
// cshell+0x2683d0 (lobby rank icon = icon of this rank), cshell+0x268810 (the level table load)
pub fn client_rank(bounds: &[i64], exp: i64) -> u32 {
    let e = exp.clamp(0, u32::MAX as i64);
    bounds
        .iter()
        .take(u8::MAX as usize)
        .position(|b| e < (*b).clamp(0, u32::MAX as i64))
        .map_or(0, |i| i as u32)
}

/// The badge tokens of one `user` line: `rank=<n>` (the rank of the account's experience by [`client_rank`] over
/// the ROM level table, or the DeCombat fallback table without a ROM) and, for a member of an existing clan (the
/// clan row, as the lobby's own clan lists show it), `clan=<clan number> clanlabel=<escaped name>
/// mark=<emblem>/<background>` (and `clancolour=<word>` while the clan has a clan colour), and `namecolour=<word>`
/// while the player has a name colour (LG-406: each the colour word of the ROM colour table's entry for the stored
/// colour number). The game server puts them into the
/// world mark and clan fields of `GS_PLAYER_INFO` 0x6b. Each token starts with a space.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 fields 15-17 (group block, `h2` = the world mark's rank, `g3` / `h1` the
// clan / name colour words)
// SPEC: PROTOCOL_2009.md table 3.6b / 3.7b (the lobby's own clan id, emblem, background, clan name per user)
// DC-DEFAULT: DSV-141 (proposed) the lobby tells the game server each player's rank and clan
// DC-DEFAULT: LG-406 (proposed) and their name / clan colour words (the 2009 client sent its own, cshell+0x23ccb0)
pub fn badge_tokens(st: &Core, account: u32, c: &crate::db::Character) -> String {
    let mut out = format!(" rank={}", client_rank(&st.rom.ranks.need, c.exp));
    let word = |n: i64| {
        st.rom
            .shop
            .name_colour_word(n.clamp(0, u32::MAX as i64) as u32)
    };
    let name = word(c.nick_color);
    if name != 0 {
        out.push_str(&format!(" namecolour={name}"));
    }
    if let Ok(Some(k)) = crate::clan::clan_of(st, account) {
        let name = if k.name.trim().is_empty() {
            "_".to_string()
        } else {
            escape_token(k.name.trim())
        };
        out.push_str(&format!(
            " clan={} clanlabel={} mark={}/{}",
            k.uid.clamp(0, u32::MAX as i64),
            name,
            k.emblem.clamp(0, u32::MAX as i64),
            k.background.clamp(0, u32::MAX as i64)
        ));
        let clan = word(i64::from(st.clan_colour(k.uid)));
        if clan != 0 {
            out.push_str(&format!(" clancolour={clan}"));
        }
    }
    out
}

/// Escapes a token value for the game server's control line: `%` and whitespace become `%XX` (UTF-8 bytes).
// DC-DEFAULT: DSV-141 (proposed) (the reader is dc-dserver `pipe::unescape_token`)
pub fn escape_token(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for ch in v.chars() {
        if ch == '%' || ch.is_whitespace() || ch.is_control() {
            let mut b = [0u8; 4];
            for x in ch.encode_utf8(&mut b).bytes() {
                out.push_str(&format!("%{x:02X}"));
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Announces one expected player to the room's game server.
fn announce_player(st: &mut Core, game: u64, account: u32, sid: Sid, team: u32) {
    let c = st.character(account).unwrap_or_default();
    let key = st.keys.u32();
    let fresh = st.keys.key16();
    let tok = loadout_tokens(st, account, &c)
        + &crate::gm::moderator_token(st, account)
        + &badge_tokens(st, account, &c);
    if let Some(g) = st.games.get_mut(&game) {
        g.entry_keys.insert(account, key);
        // DC-DEFAULT: LG-327 (proposed, project decision) one ticket per player and match, the same after a rejoin. Never
        // logged: this line goes to the game server's stdin, or to a host agent over its TLS link.
        let ticket = *g.seat_tickets.entry(account).or_insert(fresh);
        let line = user_line(account, sid, key, team, c.exp, &c.nick)
            + &tok
            + &format!(
                " {}={}",
                dc_wire::seatticket::TOKEN,
                dc_wire::seatticket::to_hex(&ticket)
            );
        if !g.write_line(&line) {
            st.note(&format!(
                "game {}: stdin closed, user line for {} lost",
                game, account
            ));
        }
    }
}

/// Host pressed Start in a waiting room (validated by the caller): 0x393 ok, state pushes, countdown.
// DC-DEFAULT: CD-23 state pushes, N countdown ticks, then the address push per player once its server is ready
pub fn start(st: &mut Core, sid: Sid, key: RoomKey) -> HResult {
    // DC-DEFAULT: MG-1 (staged) second line of the playable-mode gate (the dispatch gate in lobby.rs runs first)
    if crate::lobby::start_refused(st, key) {
        crate::lobby::refuse_start(st, sid, key);
        return Ok(());
    }
    let (host, unready) = {
        let r = &st.rooms[&key];
        (
            r.host,
            r.members
                .iter()
                .any(|m| m.account != r.host && m.state != MEMBER_READY),
        )
    };
    if st.opts.require_ready && unready {
        // MON-3: who held the start up
        let names: Vec<String> = st.rooms[&key]
            .members
            .iter()
            .filter(|m| m.account != host && m.state != MEMBER_READY)
            .map(|m| m.account.to_string())
            .collect();
        crate::opsmon::event(
            st,
            host,
            crate::opsmon::kind::START_BLOCKED,
            &format!(
                "room {}-{}: start blocked, not ready: {}",
                key.channel,
                key.no,
                names.join(", ")
            ),
        );
        st.send(sid, wire::user_flag_reply(false, 0, 0));
        return Ok(());
    }
    let mut game = None;
    if st.opts.dserver {
        match spawn(st, key) {
            Ok(g) => {
                game = Some(g);
                crate::opsmon::match_started(st, g);
            }
            Err(e) => {
                st.note(&format!(
                    "room {}-{}: game server start failed: {}",
                    key.channel, key.no, e
                ));
                crate::opsmon::event(
                    st,
                    host,
                    crate::opsmon::kind::START_FAILED,
                    &format!(
                        "room {}-{}: game server start failed: {e}",
                        key.channel, key.no
                    ),
                );
                st.send(
                    sid,
                    wire::user_flag_reply(
                        false,
                        wire::REASON_JOIN_FAILED,
                        wire::FLAG_REPLY_KIND_JOIN,
                    ),
                );
                return Ok(());
            }
        }
    }
    st.send(sid, wire::user_flag_reply(true, 0, 0));
    let (gen, starters, left) = {
        let r = st.rooms.get_mut(&key).expect("room");
        r.state = RoomPhase::Starting;
        r.gen += 1;
        r.game = game;
        r.address_sent = false;
        let mut v = Vec::new();
        let mut left = Vec::new();
        for m in r.members.iter_mut() {
            if m.account == host || m.state == MEMBER_READY {
                m.state = MEMBER_STARTING;
                v.push((m.account, m.sid, m.team, m.sub));
            } else {
                left.push((m.account, m.state, m.sub));
            }
        }
        (r.gen, v, left)
    };
    st.push_room_update(key, field::STATE, RoomPhase::Starting as u32, "", true);
    for &(a, _, _, sub) in &starters {
        st.push_room(key, &wire::room_user_state_push(a, MEMBER_STARTING, sub));
    }
    if let Some(g) = game {
        for &(a, s, team, _) in &starters {
            announce_player(st, g, a, s, team);
        }
    }
    st.defer(Deferred::Countdown { room: key, gen });
    // DC-DEFAULT: LG-341 (proposed, user playtest 2026-11-03 "people can't start in the game from the lobby") the
    // start names every member it leaves in the room and why (not ready, shopping / away, a stale state)
    let left_txt = if left.is_empty() {
        String::new()
    } else {
        let names: Vec<String> = left
            .iter()
            .map(|&(a, state, sub)| {
                let why = match (state, sub) {
                    (_, 1) => "shopping",
                    (_, 2) => "away",
                    (MEMBER_WAITING, _) => "not ready",
                    _ => "stale state",
                };
                format!("{a} ({why}, state {state})")
            })
            .collect();
        format!("; left in the room: {}", names.join(", "))
    };
    // MON-3: each member the start left behind, and why
    for &(a, state, sub) in &left {
        let why = match (state, sub) {
            (_, 1) => "shopping",
            (_, 2) => "away",
            (MEMBER_WAITING, _) => "not ready",
            _ => "stale state",
        };
        crate::opsmon::event(
            st,
            a,
            crate::opsmon::kind::LEFT_AT_START,
            &format!(
                "room {}-{}: the start by {host} left this player in the room ({why}, state {state})",
                key.channel, key.no
            ),
        );
    }
    st.note(&format!(
        "room {}-{}: start by {} with {} player(s){}",
        key.channel,
        key.no,
        host,
        starters.len(),
        left_txt
    ));
    Ok(())
}

/// One countdown tick (0x3044 count 0..N-1). Returns false when the start is stale.
pub fn tick(st: &mut Core, key: RoomKey, gen: u64, count: u32) -> bool {
    let live = st
        .rooms
        .get(&key)
        .map(|r| r.gen == gen && r.state == RoomPhase::Starting)
        .unwrap_or(false);
    if live {
        st.push_room(key, &wire::u32_push(START_COUNTDOWN_PUSH, count));
    }
    live
}

/// End of the countdown: room in game, players in game, then the address push (now, or on `@ready`).
pub fn countdown_done(st: &mut Core, key: RoomKey, gen: u64) {
    let players = {
        let Some(r) = st.rooms.get_mut(&key) else {
            return;
        };
        if r.gen != gen || r.state != RoomPhase::Starting {
            return;
        }
        r.state = RoomPhase::Playing;
        r.played_since = Some(std::time::Instant::now());
        let mut v = Vec::new();
        for m in r.members.iter_mut() {
            if m.state == MEMBER_STARTING {
                m.state = MEMBER_PLAYING;
                v.push((m.account, m.sub));
            }
        }
        v
    };
    st.push_room_update(key, field::STATE, RoomPhase::Playing as u32, "", true);
    for (a, sub) in players {
        st.push_room(key, &wire::room_user_state_push(a, MEMBER_PLAYING, sub));
    }
    let ready = match st.rooms.get(&key).and_then(|r| r.game) {
        Some(g) => st.games.get(&g).map(|g| g.ready).unwrap_or(false),
        None => true,
    };
    if ready {
        send_addresses(st, key, None);
    }
    if st
        .rooms
        .get(&key)
        .map(|r| !r.anyone_in_match())
        .unwrap_or(false)
    {
        check_end(st, key);
    }
}

/// LG-327: what a client without seat tickets reads in its room's chat after the refusal.
pub const SEAT_TEXT_OLD_CLIENT: &str =
    "Your DeCombat client is too old to join matches. Please download the current version.";

/// LG-327: says why to an account whose client was refused a seat, at its first message from the waiting room.
pub fn seat_notice_on_message(st: &mut Core, sid: Sid, id: u16) {
    if st.seat_notices.is_empty()
        || !matches!(
            id,
            crate::msg::LOCATION
                | crate::msg::ROOM_INFO
                | crate::msg::ROOM_USERS
                | crate::msg::ROOM_USER_FLAG
        )
    {
        return;
    }
    let Some(account) = st.sessions.get(&sid).and_then(|s| s.account) else {
        return;
    };
    if st.seat_notices.remove(&account) {
        st.send(sid, wire::chat_push(0, "", 0, SEAT_TEXT_OLD_CLIENT));
    }
}

/// 0x3043 to every member in game (or only to `only`).
fn send_addresses(st: &mut Core, key: RoomKey, only: Option<u32>) {
    let port = st.game_port(key);
    let targets: Vec<(Sid, u32)> = match st.rooms.get_mut(&key) {
        Some(r) => {
            if only.is_none() {
                r.address_sent = true;
            }
            r.members
                .iter()
                .filter(|m| {
                    m.state == MEMBER_PLAYING && only.map(|a| a == m.account).unwrap_or(true)
                })
                .map(|m| (m.sid, m.account))
                .collect()
        }
        None => Vec::new(),
    };
    // LG-301: a remote host's game server is announced at the host's address
    let remote = st
        .rooms
        .get(&key)
        .and_then(|r| r.game)
        .and_then(|g| st.games.get(&g))
        .and_then(|g| g.link.remote())
        .map(|r| r.address.clone());
    // LG-30: each member gets the address announced to its own lobby connection
    let bind = dserver_bind(&st.opts.gameip, &st.opts.dserver_args);
    let game = st.rooms.get(&key).and_then(|r| r.game);
    for (sid, account) in targets {
        // DC-DEFAULT: LG-327 (proposed, project decision) the ticket right before the address
        let ticket = game
            .and_then(|g| st.games.get(&g))
            .and_then(|g| g.seat_tickets.get(&account))
            .copied();
        if let Some(t) = ticket {
            st.send(sid, dc_wire::seatticket::push_frame(&t));
        }
        let ip = match (&remote, st.sessions.get(&sid)) {
            (Some(a), _) => a.clone(),
            (None, Some(s)) => {
                crate::announce::announced_game_ip(&s.peer, &s.local, &st.opts.gameip, bind)
            }
            (None, None) => st.opts.gameip.clone(),
        };
        st.send(sid, wire::match_address_push(&ip, port));
    }
}

/// A ready request while the room starts or plays (validated by the caller).
// DC-DEFAULT: CD-24 late join: 0x393 ok + 0x3043 (during the countdown: the remaining ticks, address at the end)
pub fn late_join(st: &mut Core, sid: Sid, key: RoomKey, account: u32) -> HResult {
    let (state, game, team, sub) = {
        let r = &st.rooms[&key];
        let m = r.member(account).cloned();
        (
            r.state,
            r.game,
            m.as_ref().map(|m| m.team).unwrap_or(1),
            m.map(|m| m.sub).unwrap_or(0),
        )
    };
    let new = if state == RoomPhase::Starting {
        MEMBER_STARTING
    } else {
        MEMBER_PLAYING
    };
    if let Some(m) = st.rooms.get_mut(&key).and_then(|r| r.member_mut(account)) {
        m.state = new;
    }
    st.send(sid, wire::user_flag_reply(true, 0, 0));
    st.push_room(key, &wire::room_user_state_push(account, new, sub));
    if let Some(g) = game {
        announce_player(st, g, account, sid, team);
    }
    let ready = game
        .map(|g| st.games.get(&g).map(|g| g.ready).unwrap_or(false))
        .unwrap_or(true);
    if new == MEMBER_PLAYING && ready {
        send_addresses(st, key, Some(account));
    }
    let tail = if new == MEMBER_PLAYING && !ready {
        ", address pending (game not ready)"
    } else if new == MEMBER_STARTING {
        ", address follows the countdown"
    } else {
        ""
    };
    st.note(&format!(
        "room {}-{}: account {} joins the running match{}",
        key.channel, key.no, account, tail
    ));
    Ok(())
}

/// 0x394: the player is back in the waiting room (failed join or left the match).
// DC-DEFAULT: CD-45 after 0x394 the player is set back to waiting and the room is told
///
/// LG-179: phase 1 is the client's start watchdog (no match address 15 s after the last countdown tick,
/// cshell+0x1608b0 -> cshell+0x18b010): while the room never got its address the start failed for everybody, so the
/// room goes back to waiting at once, the pending game server is told to quit and the members get the pushes.
// DC-DEFAULT: LG-179 (proposed) the 2009 lobby side of 0x394 {1} is not in the evidence; this reset is our rule
pub fn back_to_room(st: &mut Core, key: RoomKey, account: u32, phase: u32) {
    let failed = phase == 1
        && st
            .rooms
            .get(&key)
            .map(|r| {
                r.state != RoomPhase::Waiting
                    && !r.address_sent
                    && r.member(account)
                        .map(|m| m.state == MEMBER_STARTING || m.state == MEMBER_PLAYING)
                        .unwrap_or(false)
            })
            .unwrap_or(false);
    if failed {
        st.note(&format!(
            "room {}-{}: account {} reports the start failed (no match address in time), room reset to waiting",
            key.channel, key.no, account
        ));
        end_match(st, key, true);
        return;
    }
    let sub = match st.rooms.get_mut(&key).and_then(|r| r.member_mut(account)) {
        Some(m) if m.state == MEMBER_STARTING || m.state == MEMBER_PLAYING => {
            m.state = MEMBER_WAITING;
            m.sub
        }
        _ => return,
    };
    st.push_room(
        key,
        &wire::room_user_state_push(account, MEMBER_WAITING, sub),
    );
    check_end(st, key);
}

/// Ends the match of a room in which nobody is in game any more.
pub fn check_end(st: &mut Core, key: RoomKey) {
    let end = st
        .rooms
        .get(&key)
        .map(|r| r.state != RoomPhase::Waiting && !r.anyone_in_match())
        .unwrap_or(false);
    if end {
        end_match(st, key, true);
    }
}

/// Room back to waiting: members in game -> waiting, room state 0, and the game server is told to quit.
pub fn end_match(st: &mut Core, key: RoomKey, quit: bool) {
    let (back, game) = {
        let Some(r) = st.rooms.get_mut(&key) else {
            return;
        };
        r.state = RoomPhase::Waiting;
        r.gen += 1;
        r.address_sent = false;
        let mut v = Vec::new();
        for m in r.members.iter_mut() {
            if m.state == MEMBER_STARTING || m.state == MEMBER_PLAYING {
                m.state = MEMBER_WAITING;
                v.push((m.account, m.sub));
            }
        }
        (v, r.game.take())
    };
    for (a, sub) in back {
        st.push_room(key, &wire::room_user_state_push(a, MEMBER_WAITING, sub));
    }
    st.push_room_update(key, field::STATE, RoomPhase::Waiting as u32, "", true);
    if let Some(g) = game {
        if quit {
            quit_game(st, g);
        }
    }
    st.note(&format!(
        "room {}-{}: match over, room waiting",
        key.channel, key.no
    ));
}

/// Schedules the forced stop of a game server that is still running after the grace period (once per game).
// SPEC: DSERVER_PIPE addendum e2e A5
fn schedule_reap(st: &mut Core, game: u64) {
    let grace = st.rules.dserver_quit_grace_secs;
    let due = match st.games.get_mut(&game) {
        Some(g) if g.link.has_process() && !g.reap_scheduled => {
            g.reap_scheduled = true;
            true
        }
        _ => false,
    };
    if due {
        st.defer(Deferred::ReapGame { game, secs: grace });
    }
}

/// `quit`, then a forced kill after the grace period.
// SPEC: DSERVER_PIPE.md §3.4 (quit; force-kill after a 5 s grace period)
pub fn quit_game(st: &mut Core, game: u64) {
    let grace = st.rules.dserver_quit_grace_secs;
    if let Some(g) = st.games.get_mut(&game) {
        if !g.over || g.stdin_open() {
            g.write_line("quit");
        }
        g.over = true;
        g.close_stdin();
        if !g.reap_scheduled {
            g.reap_scheduled = true;
            st.defer(Deferred::ReapGame { game, secs: grace });
        }
    }
}

/// One stdout line of a game server.
pub fn on_line(st: &mut Core, game: u64, line: &str) -> HResult {
    let ev = parse_event(line);
    // MON-2: the monitor sees each line before the lobby acts on it
    crate::opsmon::on_pipe(st, game, line, &ev);
    match ev {
        Event::Console(text) => {
            if st.opts.dserver_verbose {
                st.note(&format!("game {}| {}", game, text));
            }
        }
        Event::Ready { port, world } => {
            st.note(&format!(
                "game {}: @ready port={} world={}",
                game, port, world
            ));
            let room = match st.games.get_mut(&game) {
                Some(g) => {
                    g.ready = true;
                    // LG-301: a remote server whose `started` has not come yet is announced at its @ready port
                    if g.port == 0 && g.link.remote().is_some() {
                        if let Ok(p) = port.parse::<u16>() {
                            g.port = p;
                        }
                    }
                    g.room
                }
                None => return Ok(()),
            };
            let pending = st
                .rooms
                .get(&room)
                .map(|r| r.game == Some(game) && r.state == RoomPhase::Playing && !r.address_sent)
                .unwrap_or(false);
            if pending {
                send_addresses(st, room, None);
            }
        }
        Event::Join { client, uid } => {
            st.note(&format!(
                "game {}: @join client {} uid {:?}",
                game, client, uid
            ));
            if let (Some(g), Some(uid)) = (st.games.get_mut(&game), uid) {
                g.clients.insert(client, uid);
                g.left.remove(&uid);
            }
        }
        Event::Leave(client) => {
            st.note(&format!("game {}: @leave client {}", game, client));
            // DC-DEFAULT: REW-1 (project decision a client that left before `@result` is a leaver
            if let Some(g) = st.games.get_mut(&game) {
                if !g.result_done {
                    if let Some(&uid) = g.clients.get(&client) {
                        g.left.insert(uid);
                    }
                }
            }
        }
        Event::Player(p) => {
            if let Some(g) = st.games.get_mut(&game) {
                if !g.result_done {
                    g.players.retain(|q| q.uid != p.uid);
                    g.players.push(p);
                }
            }
        }
        Event::Result(r) => finish(st, game, &r)?,
        Event::End => {
            st.note(&format!("game {}: @end", game));
            close_game(st, game, false);
        }
        Event::Crash(text) => {
            st.note(&format!("game {}: @crash {}", game, text));
            close_game(st, game, true);
        }
        // DC-DEFAULT: HS-6 (proposed) delivery of a picked-up HiSec case
        Event::Hisec(p) => crate::hisec::on_pickup(st, game, &p)?,
        Event::Crate(p) => crate::camo_crate::on_pickup(st, game, &p)?,
        // DC-DEFAULT: LG-185 (proposed) the newest link window per client, served by `GET /net`
        Event::Net(n) => {
            if let Some(g) = st.games.get_mut(&game) {
                g.net
                    .insert(n.client.clone(), (std::time::Instant::now(), n));
            }
        }
        // DC-DEFAULT: LG-327 (proposed, project decision) the one place an old client can read why: its disconnect box
        // has no such text. Once per match and account, and only when it is back in the room: a client drops what the
        // lobby sent while its game screen was up.
        Event::SeatRefused(uid) => {
            let new = st
                .games
                .get_mut(&game)
                .is_some_and(|g| g.seat_told.insert(uid));
            st.note(&format!(
                "game {game}: seat of account {uid} refused: a client without a seat ticket"
            ));
            if new {
                st.seat_notices.insert(uid);
            }
        }
        Event::Other(text) => {
            if st.opts.dserver_verbose {
                st.note(&format!("game {}: ignored event {}", game, text));
            }
        }
    }
    Ok(())
}

/// The server stopped (`@end`, `@crash`, EOF): its room goes back to waiting; results already stored stay.
pub fn close_game(st: &mut Core, game: u64, crashed: bool) {
    // MON-2: the match record gets its outcome (once per game)
    crate::opsmon::match_closed(st, game, crashed);
    let room = match st.games.get_mut(&game) {
        Some(g) => {
            if crashed && !g.result_done {
                g.players.clear();
            }
            g.over = true;
            g.close_stdin();
            g.room
        }
        None => return,
    };
    // SPEC: DSERVER_PIPE addendum e2e A5 (reap the server after `@end` plus a grace period)
    schedule_reap(st, game);
    if st
        .rooms
        .get(&room)
        .map(|r| r.game == Some(game))
        .unwrap_or(false)
    {
        // LG-179: a server that went away before `@ready` fails the start; say why, then reset the room
        if st.games.get(&game).map(|g| !g.ready).unwrap_or(false) {
            st.note(&format!(
                "game {}: server ({}) stopped before it reported ready; start failed, room {}-{} reset to waiting",
                game,
                server_label(st, game),
                room.channel,
                room.no
            ));
        }
        end_match(st, room, false);
    }
}

/// `@result`: one transaction for every `@player` that was still connected at the round end (rewards through
/// `lobby.calc_exp_gp`; a leaver is paid nothing, gets no match count and no push unless `rewards.pay_leavers`),
/// one `match_log` row, then 0x3004 to each paid player online. A second `@result` of the same game is ignored.
/// DeCombat choice (kept on purpose): every rewarded player gets 0x3004. The capture reference sends it only to
/// accounts whose totals changed (CAPTURE_tdm4 K16, bot only); the client accepts either.
// SPEC: DSERVER_PIPE.md §4.5 (lobby effect, exactly-once); rewards: DC-DEFAULT: CD-03 / CD-04 via the hook
fn finish(st: &mut Core, game: u64, r: &MatchResult) -> HResult {
    let pay_leavers = st.rules.pay_leavers;
    let (players, leavers, room, map, mode) = match st.games.get_mut(&game) {
        Some(g) if !g.result_done => {
            g.result_done = true;
            g.over = true;
            let (stay, gone): (Vec<PlayerResult>, Vec<PlayerResult>) = g
                .players
                .iter()
                .cloned()
                .partition(|p| pay_leavers || !g.left.contains(&p.uid));
            (stay, gone, g.room, g.map, g.mode)
        }
        _ => return Ok(()),
    };
    // MAP-36: a community map has no `.gms`, its factors are 1.0
    let (exp_factor, gp_factor) = st.maps_of_channel(room.channel).map_factors(map, mode);
    let mode_key = if r.mode.is_empty() {
        st.rom.mode_key(mode)
    } else {
        r.mode.clone()
    };
    let unpaid = rewards::unpaid_bots(st, &players, &leavers);
    let mut detail = Vec::new();
    let mut pushes = Vec::new();
    let mut gains = Vec::new();
    for p in &players {
        let Some(before) = db::character(&st.db, p.uid)? else {
            continue;
        };
        let ctx = RewardCtx {
            account: p.uid,
            kills: p.kills,
            deaths: p.deaths,
            headshots: p.headshots,
            team_kills: p.team_kills,
            team: p.team,
            winner: r.winner,
            player_win: p.win,
            agent_win: p.agent,
            objective: 0,
            map,
            mode,
            mode_key: mode_key.clone(),
            exp_factor,
            gp_factor,
            reported_exp: p.exp,
            reported_gp: p.gp,
        };
        let (exp, gp) = rewards::calc(st, &ctx);
        let bonus = reward_bonus(st, &ctx, (exp, gp));
        // DC-DEFAULT: LG-200 (proposed) the active EXP / GP passes add their percentage; the extra is part of the bonus
        let extra = rewards::pass_extra(rewards::pass_fractions(st, p.uid), (exp, gp));
        let (exp, gp) = (exp + extra.0, gp + extra.1);
        let bonus = (bonus.0 + extra.0, bonus.1 + extra.1);
        // LG-320: a pool seat keeps no record at all: no pay, no match count, no kills on a sheet
        if st.pool.has(p.uid) {
            gains.push((p.uid, 0, 0, (0, 0)));
            detail.push(format!("{}:{}:seat:+0exp:+0gp", p.uid, p.team));
            continue;
        }
        // LG-317: nothing for a bot run from a player's PC
        let no_pay = unpaid.contains(&p.uid);
        let (exp, gp, bonus) = if no_pay {
            (0, 0, (0, 0))
        } else {
            (exp, gp, bonus)
        };
        rewards::count_down_passes(st, p.uid)?;
        // DSV-472: a per-player verdict (Spy Hunt) over the team rule
        let decided = r.winner != 0 || p.win.is_some();
        let win = ctx.won();
        let loss = decided && !win;
        db::apply_match(
            &st.db,
            p.uid,
            &db::MatchDelta {
                exp,
                gp,
                kills: p.kills,
                deaths: p.deaths,
                headshots: p.headshots,
                win,
                loss,
            },
        )?;
        let paid = db::character(&st.db, p.uid)?.unwrap_or_default();
        let promo = rewards::promote(st, p.uid, st.rank_of(before.exp), st.rank_of(paid.exp))?;
        let after = db::character(&st.db, p.uid)?.unwrap_or_default();
        // DC-DEFAULT: LG-303 (proposed) the popup flag (box 0x185f "Your Event items can be found in the Inbox") only
        // when the promotion put gift rows into the inbox; the rank-up notice 0x487 shows on every rank-up anyway
        // (the reference stack sent the flag on every push, PROTOCOL_2009.md §7.1)
        let popup = promo.gift_rows > 0;
        pushes.push((p.uid, after.exp, after.gp, popup));
        gains.push((p.uid, exp, gp, bonus));
        detail.push(format!(
            "{}:{}:{}/{}:+{}exp:+{}gp",
            p.uid, p.team, p.kills, p.deaths, exp, gp
        ));
    }
    for p in &leavers {
        detail.push(format!("{}:{}:left:+0exp:+0gp", p.uid, p.team));
    }
    db::log_match(
        &st.db,
        &db::MatchRow {
            ended: crate::clock::unix_now(),
            channel: room.channel,
            room: room.no,
            map,
            mode,
            mode_key: &mode_key,
            winner: r.winner,
            alpha: r.alpha,
            bravo: r.bravo,
            players: (players.len() + leavers.len()) as i64,
            detail: &detail.join(" "),
        },
    )?;
    crate::clan::on_match_result(st, room, map, mode, r, &players)?;
    for (uid, exp, gp, popup) in pushes {
        let f = wire::progress_push(
            exp.clamp(0, u32::MAX as i64) as u32,
            gp.clamp(0, u32::MAX as i64) as u32,
            popup,
        );
        for sid in st.sids_of(uid) {
            st.send(sid, f.clone());
        }
        // LG-303: the gift rows wait in the inbox; its stale mark makes the client fetch them
        if popup {
            crate::inbox::push_stale(st, uid);
        }
    }
    if unpaid.is_empty() {
        crate::nxrewards::on_match_result(st, game, r, &players)?;
    }
    // SPEC: DSERVER_PIPE.md §12.B `gain <uid> <exp> <gp>`: one line per @player account with what the lobby added
    // (negative as 0); the server shows it in its result summary. Config `dserver.send_gain_lines` (DSV-35 proposed).
    // DC-DEFAULT: DSV-142 (proposed) the line also carries the bonus part of each gain
    if st.rules.send_gain_lines {
        if let Some(g) = st.games.get_mut(&game) {
            for (uid, exp, gp, bonus) in gains {
                g.write_line(&gain_line(uid, exp, gp, bonus));
            }
        }
    }
    st.note(&format!(
        "game {}: result stored ({} player(s), win {}) {}",
        game,
        players.len(),
        r.winner,
        detail.join(" ")
    ));
    Ok(())
}

/// The `gain` control line for one account, `gain <uid> <exp> <gp> <exp_bonus> <gp_bonus>`; a negative value is
/// sent as 0, values clamp to u32, a bonus never exceeds its gain. A game server that reads only the first three
/// numbers ignores the bonus pair.
// SPEC: DSERVER_PIPE.md §12.B; GAME_MESSAGES_2009_S2C.md §3.4 row 0x65 (`exp_bonus`, `gp_bonus`)
// DC-DEFAULT: DSV-142 (proposed)
pub fn gain_line(uid: u32, exp: i64, gp: i64, bonus: (i64, i64)) -> String {
    let c = |v: i64| v.clamp(0, u32::MAX as i64);
    let (e, g) = (c(exp), c(gp));
    format!(
        "gain {uid} {e} {g} {} {}",
        c(bonus.0).min(e),
        c(bonus.1).min(g)
    )
}

/// The bonus part of one player's reward (exp, gp): what the DeCombat reward rule (CD-03 / CD-04) added on top of the
/// plain reward, that is the win bonus and a map factor above 1. The plain reward is the same rule without the win
/// bonus and with factor 1. When the reward did not come from that rule (a mod hook changed it, or the operator
/// trusts the game server's numbers) the bonus is unknown and 0.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.4 row 0x65 (the result screen shows (gain - bonus) + bonus; +0x1ae := bonus > 0)
// DC-DEFAULT: DSV-142 (proposed); CD-03 / CD-04 (the rule whose parts are named here)
pub fn reward_bonus(st: &Core, ctx: &RewardCtx, got: (i64, i64)) -> (i64, i64) {
    let trusted =
        st.rules.trust_server_rewards && ctx.reported_exp.is_some() && ctx.reported_gp.is_some();
    if trusted || rewards::base(&st.rules, ctx) != got {
        return (0, 0);
    }
    bonus_of(&st.rules, ctx, got)
}

/// [`reward_bonus`] for a reward known to come from the DeCombat rule.
pub fn bonus_of(rules: &crate::config::Rules, ctx: &RewardCtx, got: (i64, i64)) -> (i64, i64) {
    let plain_ctx = RewardCtx {
        winner: 0,
        player_win: None,
        agent_win: false,
        ..ctx.clone()
    };
    let plain = (
        rewards::one(&rules.exp, &plain_ctx, 1.0),
        rewards::one(&rules.gp, &plain_ctx, 1.0),
    );
    (
        (got.0 - plain.0).clamp(0, got.0.max(0)),
        (got.1 - plain.1).clamp(0, got.1.max(0)),
    )
}

/// `@ready` watchdog: a server that is not ready in time is killed and the start fails.
// SPEC: DSERVER_PIPE.md §6 (spawn -> @ready 30 s, --dserver-timeout), §7 (no @ready in time)
pub fn ready_timeout(st: &mut Core, game: u64) {
    let stale = st
        .games
        .get(&game)
        .map(|g| !g.ready && !g.over)
        .unwrap_or(false);
    if !stale {
        return;
    }
    st.note(&format!(
        "game {}: no @ready in time ({}), stopping it",
        game,
        server_label(st, game)
    ));
    if let Some(g) = st.games.get_mut(&game) {
        g.kill();
    }
    close_game(st, game, true);
}

/// `rom` as an absolute path: a relative one is taken against the lobby's current directory, which is what the
/// launchers' relative client folder meant.
pub(crate) fn absolute_rom(rom: &std::path::Path) -> PathBuf {
    if rom.is_absolute() {
        return rom.to_path_buf();
    }
    std::env::current_dir()
        .map(|d| d.join(rom))
        .unwrap_or_else(|_| rom.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    // a relative ROM folder is resolved against the lobby's directory before the game server gets it
    #[test]
    fn a_relative_rom_folder_reaches_the_game_server_absolute() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(absolute_rom(std::path::Path::new("pz")), cwd.join("pz"));
        let abs = cwd.join("client");
        assert_eq!(absolute_rom(&abs), abs);
    }

    #[test]
    fn gain_lines_clamp_negative_to_zero() {
        assert_eq!(gain_line(1001, 12, 30, (0, 0)), "gain 1001 12 30 0 0");
        assert_eq!(gain_line(7, -4, 6, (0, 0)), "gain 7 0 6 0 0");
        assert_eq!(gain_line(7, 5, -6, (0, 0)), "gain 7 5 0 0 0");
        assert_eq!(gain_line(7, 1 << 40, 0, (0, 0)), "gain 7 4294967295 0 0 0");
        // DC-DEFAULT: DSV-142 (proposed) the bonus pair, never above its gain, never negative
        assert_eq!(gain_line(7, 23, 40, (5, 8)), "gain 7 23 40 5 8");
        assert_eq!(gain_line(7, 3, 4, (9, -1)), "gain 7 3 4 3 0");
    }

    #[test]
    fn parses_the_stand_in_lines() {
        // HARNESS: lobby_harness FAKE_DSERVER output
        assert_eq!(
            parse_event("@ready port=5000 world=fake"),
            Event::Ready {
                port: "5000".into(),
                world: "fake".into()
            }
        );
        assert_eq!(
            parse_event("@join 0 7 3 99 0"),
            Event::Join {
                client: "0".into(),
                uid: Some(7)
            }
        );
        let Event::Player(p) = parse_event(
            "@player uid=7 kills=2 deaths=0 team=1 exp=3 gp=1 tk=0 hs=1 name=Dave the Great",
        ) else {
            panic!()
        };
        assert_eq!(
            (p.uid, p.kills, p.deaths, p.team, p.exp, p.gp, p.headshots),
            (7, 2, 0, 1, Some(3), Some(1), 1)
        );
        assert_eq!(p.name, "Dave the Great");
        assert_eq!((p.win, p.agent), (None, false), "no Spy Hunt flags");
        // DSV-472: the Spy Hunt flags before the name
        let Event::Player(p) = parse_event(
            "@player uid=7 kills=2 deaths=0 team=1 tk=0 hs=1 win=1 agent=1 name=Dave the Great",
        ) else {
            panic!()
        };
        assert_eq!(
            (p.win, p.agent, p.name.as_str()),
            (Some(true), true, "Dave the Great")
        );
        assert_eq!(
            parse_event("@result mode=TDM win=1 alpha=2 bravo=0 limit=2 rounds=0:0"),
            Event::Result(MatchResult {
                mode: "TDM".into(),
                winner: 1,
                alpha: 2,
                bravo: 0,
                limit: 2,
                rounds: "0:0".into()
            })
        );
        assert_eq!(parse_event("@leave 0\r\n"), Event::Leave("0".into()));
        assert_eq!(parse_event("@end"), Event::End);
        assert_eq!(parse_event("@connect 0"), Event::Other("@connect 0".into()));
        assert_eq!(
            parse_event("engine says hi"),
            Event::Console("engine says hi".into())
        );
        assert_eq!(
            parse_event("@crash access violation"),
            Event::Crash("access violation".into())
        );
    }

    #[test]
    fn idle_is_clamped_to_zero_or_more() {
        // SPEC: DSERVER_PIPE addendum e2e B1
        assert_eq!(idle_arg(-5), "0");
        assert_eq!(idle_arg(0), "0");
        assert_eq!(idle_arg(60), "60");
    }

    #[test]
    fn the_port_probe_tests_the_servers_own_bind_address() {
        use std::net::Ipv4Addr;
        // DC-DEFAULT: LG-11
        assert_eq!(probe_ip("127.0.0.1", ""), Ipv4Addr::LOCALHOST);
        assert_eq!(
            probe_ip("127.0.0.1", "-tickhz 30 -bindip 127.0.0.2"),
            Ipv4Addr::new(127, 0, 0, 2)
        );
        assert_eq!(
            probe_ip("127.0.0.1", "-BINDIP 127.0.0.3 -tickhz 30"),
            Ipv4Addr::new(127, 0, 0, 3)
        );
        assert_eq!(probe_ip("10.0.0.5", ""), Ipv4Addr::new(10, 0, 0, 5));
        assert_eq!(
            probe_ip("127.0.0.1", "-bindip 0.0.0.0"),
            Ipv4Addr::LOCALHOST
        );
        assert_eq!(probe_ip("127.0.0.1", "-bindip"), Ipv4Addr::LOCALHOST);
    }

    #[test]
    fn bindip_only_for_a_routable_literal_address() {
        // SPEC: DSERVER_PIPE addendum e2e B2
        assert_eq!(bindip_for("192.168.1.20", ""), Some("192.168.1.20".into()));
        assert_eq!(bindip_for("127.0.0.1", ""), None);
        assert_eq!(bindip_for("game.example.org", ""), None);
        assert_eq!(bindip_for("192.168.1.20", "-fps 30 -BINDIP 10.0.0.1"), None);
        assert_eq!(
            dserver_bind("203.0.113.7", "-bindip 0.0.0.0"),
            std::net::Ipv4Addr::UNSPECIFIED
        );
        assert_eq!(
            dserver_bind("192.168.1.20", ""),
            std::net::Ipv4Addr::new(192, 168, 1, 20)
        );
        assert_eq!(dserver_bind("127.0.0.1", ""), std::net::Ipv4Addr::LOCALHOST);
    }

    /// LDO-2: the attached options of the equipped weapons by kind (muzzle / sight / magazine), from the parent
    /// unique id of the option rows; the user line carries them between `loadout=` and `face=`.
    // DC-DEFAULT: LDO-2 (proposed)
    #[test]
    fn parts_token_lists_each_equipped_weapons_options_by_kind() {
        let mut st = crate::state::testing::state();
        // synthetic option rows (not ROM rows): items 1000 + ItemID of kinds 17 / 18 / 19
        std::sync::Arc::get_mut(&mut st.rom).unwrap().items =
            Some(dc_bute::ItemsTable::from_doc(dc_bute::ButeDoc::parse(
                r#"
[Option0]
ItemID=3
ItemType=17
[Option1]
ItemID=4
ItemType=18
[Option2]
ItemID=6
ItemType=19
[Option3]
ItemID=9
ItemType=18
"#,
            )));
        let add = |st: &Core, uid: i64, no: i64, slot: i64, parent: i64| {
            st.db
                .execute(
                    "INSERT INTO items (uid, user_no, item_no, parent_uid) VALUES (?1, 77, ?2, ?3)",
                    rusqlite::params![uid, no, parent],
                )
                .unwrap();
            db::set_slot(&st.db, uid, slot).unwrap();
        };
        // weapon 145 in slot 0 with a sight; weapon 27 in backpack slot 10 with muzzle + magazine; a second sight
        // with a higher uid on the slot-0 weapon loses; an unequipped weapon with a sight stays out; an option of an
        // unknown kind (1005) and a weapon without options (slot 1) send nothing
        add(&st, 1, 145, 0, 0);
        add(&st, 2, 164, 1, 0);
        add(&st, 3, 27, 10, 0);
        add(&st, 4, 7, -1, 0);
        add(&st, 10, 1004, -1, 1);
        add(&st, 11, 1003, -1, 3);
        add(&st, 12, 1006, -1, 3);
        add(&st, 13, 1009, -1, 1);
        add(&st, 14, 1009, -1, 4);
        add(&st, 15, 1005, -1, 2);
        let items = db::items(&st.db, 77).unwrap();
        assert_eq!(
            weapon_options(&st, &items),
            vec![
                (0, [None, Some(1004), None]),
                (10, [Some(1003), None, Some(1006)])
            ]
        );
        assert_eq!(
            parts_token(&st, &items),
            " parts=0:-1/1004/-1,10:1003/-1/1006"
        );
        let c = db::Character {
            face: 2,
            ..Default::default()
        };
        assert_eq!(
            loadout_tokens(&st, 77, &c),
            " loadout=0:145,1:164,10:27 parts=0:-1/1004/-1,10:1003/-1/1006 face=2"
        );
        // detached: the token disappears, the loadout stays
        st.db
            .execute("UPDATE items SET parent_uid = 0", [])
            .unwrap();
        assert_eq!(
            loadout_tokens(&st, 77, &c),
            " loadout=0:145,1:164,10:27 face=2"
        );
        // without the ROM item table no option kind is known: no token
        std::sync::Arc::get_mut(&mut st.rom).unwrap().items = None;
        st.db
            .execute("UPDATE items SET parent_uid = 1 WHERE uid = 10", [])
            .unwrap();
        let items = db::items(&st.db, 77).unwrap();
        assert_eq!(parts_token(&st, &items), "");
    }

    #[test]
    fn user_line_shape() {
        assert_eq!(
            user_line(7, 3, 99, 1, 120, "Dave"),
            "user 7 3 99 0 120 Dave"
        );
        assert_eq!(
            user_line(8, 4, 1, 2, 0, "Big Erin"),
            "user 8 4 1 1 0 Big_Erin"
        );
    }

    #[test]
    fn client_rank_follows_the_2009_rule() {
        // synthetic bounds (no ROM values): rank = first row whose bound is above the experience, else 0
        let b = [100i64, 500, 2000];
        assert_eq!(client_rank(&b, 0), 0);
        assert_eq!(client_rank(&b, 99), 0);
        assert_eq!(client_rank(&b, 100), 1);
        assert_eq!(client_rank(&b, 499), 1);
        assert_eq!(client_rank(&b, 1999), 2);
        assert_eq!(
            client_rank(&b, 2000),
            0,
            "past the last row: 0 (cshell+0x267f10)"
        );
        assert_eq!(client_rank(&b, -5), 0);
        assert_eq!(client_rank(&[], 50), 0);
        // the client counts at most 255 rows
        let long: Vec<i64> = (1..=300).map(|i| i * 10).collect();
        assert_eq!(client_rank(&long, 2545), 254);
        assert_eq!(client_rank(&long, 2550), 0);
        // the DeCombat fallback table (CD-05) agrees with RankTable::rank_of below the top row
        let t = crate::rom::RankTable::fallback();
        for exp in [0i64, 99, 100, 101, 5_000, 123_456] {
            assert_eq!(
                client_rank(&t.need, exp) as usize,
                t.rank_of(exp),
                "exp {exp}"
            );
        }
    }

    #[test]
    fn token_escape_keeps_the_value_in_one_token() {
        assert_eq!(escape_token("Wolves"), "Wolves");
        assert_eq!(escape_token("Night Owls"), "Night%20Owls");
        assert_eq!(escape_token("100%"), "100%25");
        assert_eq!(escape_token("a\tb"), "a%09b");
        assert_eq!(escape_token("a\u{3000}b"), "a%E3%80%80b");
        assert_eq!(escape_token("x y").split_whitespace().count(), 1);
    }

    #[test]
    fn bonus_is_the_rule_part_above_the_plain_reward() {
        // DC-DEFAULT: DSV-142 (proposed) over CD-03 / CD-04 defaults (finish 15 / 20, kill 6 / 6, win +25 %)
        let r = crate::config::Rules::default();
        let ctx = |team: i64, winner: i64, f: f64| RewardCtx {
            kills: 2,
            team,
            winner,
            exp_factor: f,
            gp_factor: f,
            ..Default::default()
        };
        // winner, factor 1: (27, 32) plain, (34, 40) with the win bonus
        let w = ctx(1, 1, 1.0);
        let got = rewards::base(&r, &w);
        assert_eq!(got, (34, 40));
        assert_eq!(bonus_of(&r, &w, got), (7, 8));
        // loser: no bonus
        let l = ctx(2, 1, 1.0);
        assert_eq!(bonus_of(&r, &l, rewards::base(&r, &l)), (0, 0));
        // map factor 2 on a loser: the doubled part is the bonus
        let l2 = ctx(2, 1, 2.0);
        let got2 = rewards::base(&r, &l2);
        assert_eq!(got2, (54, 64));
        assert_eq!(bonus_of(&r, &l2, got2), (27, 32));
        // map factor below 1: never negative
        let l3 = ctx(2, 1, 0.5);
        assert_eq!(bonus_of(&r, &l3, rewards::base(&r, &l3)), (0, 0));
    }

    #[test]
    fn port_choice_rules() {
        // HARNESS: lobby_harness::test_stale_port (same asserts, black-box over the chooser)
        let range = (40000, 40005);
        let mut stopped = Vec::new();
        // preferred free -> preferred
        assert_eq!(
            choose_port(30000, range, &[], &|_| true, &mut |_| true),
            Some(30000)
        );
        // a foreign holder: skipped, another free port is chosen
        let busy = |p: u16| p != 30000;
        let q = choose_port(30000, range, &[], &busy, &mut |id| {
            stopped.push(id);
            true
        });
        assert!(q.is_some() && q != Some(30000));
        assert!(stopped.is_empty());
        // our own stale (finished) game holds it: stopped and the port reused
        let holder = std::cell::Cell::new(true);
        let free = |p: u16| !(p == 30000 && holder.get());
        let q = choose_port(30000, range, &[(30000, 9, true)], &free, &mut |id| {
            assert_eq!(id, 9);
            holder.set(false);
            true
        });
        assert_eq!(q, Some(30000));
        // our own live game holds it: never stopped
        let q = choose_port(
            30000,
            range,
            &[(30000, 9, false)],
            &|p| p != 30000,
            &mut |_| panic!("live game stopped"),
        );
        assert_eq!(q, Some(40000));
        // range ports used by our games are skipped
        let q = choose_port(
            0,
            range,
            &[(40000, 1, false), (40001, 2, true)],
            &|_| true,
            &mut |_| true,
        );
        assert_eq!(q, Some(40002));
    }

    #[test]
    fn real_udp_probe_detects_a_loopback_holder() {
        let s = std::net::UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let p = s.local_addr().unwrap().port();
        assert!(!udp_port_free(p));
        drop(s);
        assert!(udp_port_free(p));
    }
}
