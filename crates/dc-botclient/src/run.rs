//! The I/O around [`BotSession`]: one UDP socket and a millisecond clock ([`run_game`]), and the whole run of one
//! bot from the lobby login to the end of the game session ([`run_bot`]), with a JSON report.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dc_net_engine::body::parse_body;
use dc_net_engine::impair::PathSpec;
use dc_net_engine::shim::NetShim;
use dc_net_engine::Millis;
use dc_wire::udp::{is_oob_datagram, BitReader, CtlBlock, RECV_BUFFER_BYTES};
use serde_json::{json, Value};

use crate::hostroom::{set_goal, GoalChoice};
use crate::lobby::{auth_login, Lobby, LoginOptions, MatchAddress};
use crate::motion::MotionKind;
use crate::record::{head, Recorder};
use crate::romworld::RomWorlds;
use crate::session::{BotSession, GameConfig, Stage};
use crate::shell::{s2c, PlayerInfo};
use crate::shooter::{RomWeapons, ShooterConfig};
use crate::{bot_err, BotResult};

/// How long the socket waits for a datagram before the loop runs the client's timers.
pub const POLL_WAIT_MS: u64 = 2;
/// After the bot asked to quit, how long the loop keeps running for the bye exchange.
pub const QUIT_GRACE_MS: u32 = 2500;
/// After the session ended, how long the loop keeps running to send the remaining bye copies and to record the
/// server's.
// SPEC: UDP_TRANSPORT_2009.md §9.1 (4 copies, 100 ms apart)
pub const BYE_FLUSH_MS: u32 = 600;

/// The decoded link blocks of a datagram, for the capture's `blocks` field.
// SPEC: UDP_TRANSPORT_2009.md §4 (out-of-band vs link), §6-§9 (check byte, control, reliable, unreliable blocks)
pub fn describe_datagram(d: &[u8]) -> Value {
    if is_oob_datagram(d) {
        let mut r = BitReader::new(d);
        r.skip_bits(32);
        return json!({"oob": r.read_bits(3)});
    }
    if d.is_empty() {
        return Value::Null;
    }
    let rx = parse_body(&d[1..]);
    let ctl = match rx.ctl {
        Some(CtlBlock::Ack { ack_seq, sack_mask }) => json!({"ack": ack_seq, "sack": sack_mask}),
        Some(CtlBlock::Bye) => json!({"bye": true}),
        Some(CtlBlock::ByeCode(c)) => json!({"bye_code": c}),
        Some(CtlBlock::Unknown(op)) => json!({"op": op}),
        None => Value::Null,
    };
    let rel = rx.rel.map(|b| {
        json!({"seq": b.rel_seq, "cont": b.tail_cont,
               "entries": b.entries.iter().map(|e| json!({"bits": e.bit_len(), "head": head(e.as_bytes())})).collect::<Vec<_>>()})
    });
    let unrel: Vec<Value> = rx
        .unrel
        .iter()
        .map(|e| json!({"bits": e.bit_len(), "head": head(e.as_bytes())}))
        .collect();
    json!({"ctl": ctl, "rel": rel, "unrel": unrel, "rel_abandoned": rx.rel_abandoned})
}

/// Drives `client` against the game server at `server` until `duration` passed (then the bot quits) or the
/// session ended. The link peer is whoever answered the join (transport §5.2: the client does not check it).
// SPEC: UDP_TRANSPORT_2009.md §5.2 (join; the reply's source becomes the peer), §4 (receive loop)
pub fn run_game(
    client: &mut BotSession,
    server: SocketAddr,
    duration: Duration,
    rec: &Recorder,
) -> BotResult<()> {
    run_game_with(client, server, duration, &Impairment::default(), rec)
}

/// Deliberate impairment of the bot's path, to exercise both transports against bad real-world connections: the
/// network lab's profile (`--net`, docs/NETLAB.md) plus the older single knobs `--loss` / `--drop-over`. Seeded, so
/// a run is repeatable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Impairment {
    /// Outgoing datagrams dropped per thousand (on top of the profile).
    pub loss_out_permille: u32,
    /// Incoming datagrams dropped per thousand (on top of the profile).
    pub loss_in_permille: u32,
    pub seed: u64,
    /// A path that loses every datagram longer than this many bytes, both directions (a small MTU with
    /// fragments dropped; `--drop-over`).
    // DC-DEFAULT: NET-10 (proposed) test impairment for the datagram cap
    pub drop_over_bytes: Option<usize>,
    /// The network lab path (`--net profile[,key=value...]`).
    // DC-DEFAULT: NET-20 (proposed) network lab impairment shim
    pub net: Option<PathSpec>,
}

impl Impairment {
    /// The whole path spec the shim runs (`None`: nothing impaired).
    pub fn path_spec(&self) -> Option<PathSpec> {
        let mut spec = self
            .net
            .clone()
            .unwrap_or_else(|| PathSpec::profile("clean").expect("clean profile"));
        if self.loss_out_permille > 0 {
            spec.up.loss_permille = spec.up.loss_permille.max(self.loss_out_permille);
        }
        if self.loss_in_permille > 0 {
            spec.down.loss_permille = spec.down.loss_permille.max(self.loss_in_permille);
        }
        if let Some(m) = self.drop_over_bytes {
            let m = m.min(u32::MAX as usize) as u32;
            spec.up.max_datagram = Some(m);
            spec.down.max_datagram = Some(m);
        }
        spec.seed ^= self.seed;
        (!spec.is_clean()).then_some(spec)
    }
}

/// [`run_game`] with an [`Impairment`].
pub fn run_game_with(
    client: &mut BotSession,
    server: SocketAddr,
    duration: Duration,
    imp: &Impairment,
    rec: &Recorder,
) -> BotResult<()> {
    run_game_watch(client, server, duration, imp, rec, None)
}

/// [`run_game_with`] that also ends the session when `lobby_closed` turns true while no game link is up yet.
pub fn run_game_watch(
    client: &mut BotSession,
    server: SocketAddr,
    duration: Duration,
    imp: &Impairment,
    rec: &Recorder,
    lobby_closed: Option<Arc<AtomicBool>>,
) -> BotResult<()> {
    run_game_loop(client, server, duration, imp, rec, lobby_closed, None)
}

/// Socket-level counters of one [`run_game_until`] / [`run_game`] loop.
#[derive(Debug, Default)]
struct UdpIo {
    rx: u64,
    rx_reset: u64,
    rx_err: u64,
    tx: u64,
    tx_err: u64,
    first_rx_ms: Option<Millis>,
    last_err: Option<String>,
}

impl UdpIo {
    fn to_json(&self) -> Value {
        json!({"rx": self.rx, "rx_reset": self.rx_reset, "rx_err": self.rx_err, "tx": self.tx,
               "tx_err": self.tx_err, "first_rx_ms": self.first_rx_ms, "last_err": self.last_err})
    }
}

/// A caller's stop check: called once per pass of the loop with the client and the session clock (ms since the
/// socket opened); `true` makes the bot quit.
pub type StopCheck<'a> = &'a mut dyn FnMut(&BotSession, Millis) -> bool;

/// [`run_game`] that ends on a condition instead of a wall-clock budget: the bot quits as soon as `stop` returns
/// true. `limit` is only the ceiling for a run whose condition never holds (the caller then reports what it
/// waited for). A loaded machine slows the join, the world load and every reply, so a test that needs "the
/// kill happened" or "the walk ended" waits for that event rather than for a number of seconds.
pub fn run_game_until(
    client: &mut BotSession,
    server: SocketAddr,
    limit: Duration,
    rec: &Recorder,
    stop: StopCheck<'_>,
) -> BotResult<()> {
    run_game_loop(
        client,
        server,
        limit,
        &Impairment::default(),
        rec,
        None,
        Some(stop),
    )
}

fn run_game_loop(
    client: &mut BotSession,
    server: SocketAddr,
    duration: Duration,
    imp: &Impairment,
    rec: &Recorder,
    lobby_closed: Option<Arc<AtomicBool>>,
    mut stop: Option<StopCheck<'_>>,
) -> BotResult<()> {
    let bind = match server.ip() {
        IpAddr::V4(ip) if ip.is_loopback() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        _ => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
    };
    // read by a thread without a receive timeout: a timeout on the socket loses datagrams on Windows under load
    // (see `dc_net_engine::socket_reader`); the network lab's impairment, when asked for, sits around it
    let spec = imp.path_spec();
    let mut sock = NetShim::bind(bind, RECV_BUFFER_BYTES, spec.clone())
        .map_err(|e| bot_err!("udp: bind: {e}"))?;
    if let Some(s) = &spec {
        rec.event("netlab_path", json!({"profile": s.name, "spec": format!("{s:?}")}));
    }
    let local = sock.local_addr().map(|a| a.to_string()).unwrap_or_default();
    let conn = rec.open_conn("game", "udp", &server.to_string());
    rec.event(
        "udp_open",
        json!({"server": server.to_string(), "local": local}),
    );
    let start = Instant::now();
    let now = |s: &Instant| s.elapsed().as_millis() as Millis;
    let mut peer: Option<SocketAddr> = None;
    let dur_ms = duration.as_millis().min(u32::MAX as u128) as u32;
    let mut quit_at: Option<Millis> = None;
    let mut ended_at: Option<Millis> = None;
    // socket-level counters, logged at the end as `udp_io`: what the bot's socket saw (a lost datagram shows as a
    // difference to the server's own counters)
    let mut io = UdpIo::default();
    loop {
        // what arrived is read before the link's timers run, so a process that was not scheduled for a while acts
        // on the acknowledgements waiting in the socket instead of resending what they acknowledge
        let mut waiting = true;
        loop {
            let got = if waiting {
                waiting = false;
                sock.recv_within(Duration::from_millis(POLL_WAIT_MS))
            } else {
                sock.try_recv()
            };
            match got {
                Some(Ok((data, from))) => {
                    let buf = &data[..];
                    io.rx += 1;
                    io.first_rx_ms.get_or_insert(now(&start));
                    let joining = client.stage() == Stage::Joining;
                    if let Some(p) = peer {
                        if from != p {
                            continue;
                        }
                    }
                    rec.udp_dgram(conn, "s2c", buf, describe_datagram(buf));
                    client.handle_datagram(now(&start), buf);
                    if joining && client.stage() != Stage::Joining && peer.is_none() {
                        peer = Some(from);
                        rec.event("udp_peer", json!({"peer": from.to_string()}));
                    }
                }
                Some(Err(e)) => {
                    io.rx_err += 1;
                    io.last_err = Some(e.to_string());
                    break;
                }
                None => break,
            }
        }
        let t = now(&start);
        for d in client.poll(t) {
            let to = peer.unwrap_or(server);
            sock.send_to(&d, to);
            rec.udp_dgram(conn, "c2s", &d, describe_datagram(&d));
        }
        let t = now(&start);
        if client.stage() == Stage::Joining
            && lobby_closed
                .as_ref()
                .is_some_and(|f| f.load(Ordering::Relaxed))
        {
            client.end_gone(
                t,
                "the lobby connection closed before the game link was up".into(),
            );
        }
        if client.is_over() {
            // keep sending the remaining bye copies (4 copies, 100 ms apart) and keep recording what the server
            // still sends (its own bye copies) for a short while
            let over_at = *ended_at.get_or_insert(t);
            if t.wrapping_sub(over_at) > BYE_FLUSH_MS {
                break;
            }
            continue;
        }
        if quit_at.is_none() {
            let stopped = stop.as_mut().is_some_and(|f| f(client, t));
            if t >= dur_ms || stopped {
                client.quit(t);
                quit_at = Some(t);
            }
        }
        if let Some(q) = quit_at {
            if t.wrapping_sub(q) > QUIT_GRACE_MS {
                break;
            }
        }
    }
    io.rx_reset = sock.resets();
    let sio = sock.io();
    io.tx = sio.sent;
    io.tx_err = sio.send_errors;
    if let Some(e) = sock.last_error() {
        io.last_err.get_or_insert_with(|| e.to_string());
    }
    rec.event("udp_io", io.to_json());
    rec.close_conn(conn, "game", "udp", "c2s");
    let (mut dropped_out, mut dropped_in) = (0, 0);
    if let Some((profile, up, down, path)) = sock.summary() {
        dropped_out = up.dropped();
        dropped_in = down.dropped();
        let dir = |d: &dc_net_engine::impair::DirStats| {
            Value::Object(d.fields().iter().map(|(k, v)| (k.to_string(), json!(v))).collect())
        };
        rec.event(
            "impairment",
            json!({"profile": profile, "up": dir(&up), "down": dir(&down),
                   "blackouts": path.blackouts, "blackout_ms": path.blackout_ms, "rebinds": path.rebinds,
                   "nat_expiries": path.nat_expiries, "flow_kills": path.flow_kills, "ports": sock.ports(),
                   "dropped_out": dropped_out, "dropped_in": dropped_in}),
        );
    }
    client.dropped = (dropped_out, dropped_in);
    rec.flush();
    Ok(())
}

/// Which room the bot sits in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomChoice {
    /// Create one; the bot is the host and starts once `start_after` members are in the room and ready.
    New {
        max_players: i32,
        title: String,
        mode: i32,
        start_after: usize,
    },
    /// Enter room number `n`.
    Number(u32),
    /// Enter the first room the list shows.
    Any,
}

/// One bot run.
#[derive(Debug, Clone)]
pub struct BotConfig {
    /// Auth server address (the lobby address comes from the server list).
    pub auth: SocketAddr,
    pub user: String,
    pub nick: String,
    pub room: RoomChoice,
    /// Room password (`--password`).
    pub password: String,
    /// The login password box text (`--login-password`, BOT-83); `None` = the harness text `pw`.
    pub login_password: Option<String>,
    /// LG-271: `--mod-set-hash` / `--content-hash` for a modded lobby (none: the bot logs in as vanilla).
    pub mod_set_hash: Option<String>,
    pub content_hash: Option<String>,
    pub channel: Option<u32>,
    /// BOT-98: `--server-id`: which server list entry to log in to.
    pub server_id: Option<u32>,
    pub motion: MotionKind,
    /// Time in the game session before the bot quits.
    pub duration: Duration,
    pub force_updates: bool,
    pub state_form: Option<u8>,
    pub update_hz: f32,
    pub load_ms: u32,
    /// Use the ROM (`CA_ROM` or this folder) for the world checks.
    pub rom: Option<PathBuf>,
    /// `--mod-cache`: unpacked packs, for a room on a community map (BOT-101).
    pub mod_cache: Option<PathBuf>,
    pub use_rom: bool,
    pub lobby_timeout: Duration,
    /// How long to wait for the room to fill (host) or for the match address push (everyone).
    pub match_wait: Duration,
    pub impairment: Impairment,
    pub aim_quat: bool,
    /// How long the join ask is repeated before the start fails (`--join-timeout`; the 2009 client's 10 s).
    pub join_timeout: Duration,
    /// End the game session when the server sent nothing for this long (0 = never).
    pub idle_exit: Duration,
    /// Fallback respawn request after a death (`None` = only the armed wait asks).
    pub respawn_after: Option<Duration>,
    /// Shooter mode (`--shooter`); false = hold fire.
    pub shooter: bool,
    pub fire_interval_ms: u32,
    /// Test throws of the slot-3 grenade every this long (`--throw-every <ms>`, shooter only).
    pub throw_every_ms: Option<u32>,
    /// BOT-147: the chance a skill bot answers a sighting in grenade range with its grenade (`--grenades <0..1>`).
    pub grenade_chance: f32,
    pub hit_node: u8,
    /// Human-like aiming level 0..=100 (`--skill`, default 50); `None` = instant aim (`--skill off`).
    pub skill: Option<u8>,
    /// Host only: the kill goal set after creating the room.
    pub goal: Option<GoalChoice>,
    /// Host only: ask for Gun Game after creating the room (BOT-99; the lobby refuses it off a featured entry).
    pub gun_game: bool,
    /// BOT-100: the host picks the Possession Arms.
    pub possession: bool,
    /// Leave the game server after the result screen 0x3f (false = `--stay-after-result`).
    pub leave_after_result: bool,
    /// Scripted timeline (`--timeline`); its `team` line is asked for in the room before ready.
    pub timeline: Option<crate::timeline::Timeline>,
    /// Matches to play from the same room seat (`--matches`; 0 = until the lobby or the room ends).
    pub matches: u32,
    /// BOT-84 `--filler PREFIX`: with `--room any`, the bot stays only in rooms that hold a player (a member whose
    /// name does not start with the prefix); alone with bots it leaves and waits for the next room.
    pub filler: Option<String>,
    /// BOT-102 `--room-title PART`: with `--room any`, only rooms whose title holds this text.
    pub room_title: Option<String>,
}

impl BotConfig {
    pub fn new(auth: SocketAddr, nick: &str) -> Self {
        BotConfig {
            auth,
            user: nick.to_ascii_lowercase(),
            nick: nick.to_string(),
            room: RoomChoice::Any,
            password: String::new(),
            login_password: None,
            mod_set_hash: None,
            content_hash: None,
            channel: None,
            server_id: None,
            motion: MotionKind::Stand,
            duration: Duration::from_secs(10),
            force_updates: false,
            state_form: None,
            update_hz: crate::session::UPDATE_HZ,
            load_ms: crate::session::DEFAULT_LOAD_MS,
            rom: None,
            mod_cache: None,
            use_rom: true,
            lobby_timeout: Duration::from_secs(10),
            match_wait: Duration::from_secs(60),
            impairment: Impairment::default(),
            aim_quat: true,
            join_timeout: Duration::from_millis(u64::from(dc_wire::udp::JOIN_TIMEOUT_MS)),
            idle_exit: Duration::from_millis(crate::lifecycle::DEFAULT_IDLE_EXIT_MS as u64),
            respawn_after: None,
            shooter: false,
            fire_interval_ms: crate::shooter::DEFAULT_FIRE_INTERVAL_MS,
            throw_every_ms: None,
            grenade_chance: crate::grenade::CHANCE,
            hit_node: crate::shooter::DEFAULT_HIT_NODE,
            skill: Some(crate::skill::STANDARD_LEVEL),
            goal: None,
            gun_game: false,
            possession: false,
            leave_after_result: true,
            timeline: None,
            matches: 1,
            filler: None,
            room_title: None,
        }
    }

    /// The shooter settings for a bot with lobby loadout `loadout` (`None` without `--shooter`).
    pub fn shooter_config(&self, loadout: &[i32], rec: &Recorder) -> Option<ShooterConfig> {
        if !self.shooter {
            return None;
        }
        let rom =
            rom_dir(self.use_rom, self.rom.as_ref()).and_then(|d| match RomWeapons::open(&d) {
                Ok(w) => Some(Arc::new(w)),
                Err(e) => {
                    rec.event("rom_weapons", json!({"error": e}));
                    None
                }
            });
        let skill = self.skill.map(crate::skill::Skill::new);
        // the cone's random table: the ROM entry, else its generator (WEAPON_FIRE_2009_addendum_cone.md AC-1)
        let table = Arc::new(dc_cone::ShotTable::open(
            rom_dir(self.use_rom, self.rom.as_ref()).as_deref(),
        ));
        rec.event(
            "shooter",
            json!({"shot_period_ms": self.fire_interval_ms, "throw_every_ms": self.throw_every_ms,
                   "grenade_chance": if skill.is_some() { self.grenade_chance } else { 0.0 },
                   "hit_node": self.hit_node, "loadout": loadout,
                   "rom_weapons": rom.as_ref().map(|r| r.rows.len()), "cone_table": table.source,
                   "skill": skill.map(|s| json!({"level": s.level, "turn_deg_s": s.turn_deg_s,
                       "fire_tolerance_deg": s.fire_tolerance_deg, "reaction_ms": s.reaction_ms,
                       "reacquire_ms": s.reacquire_ms, "aim_sigma_deg": s.aim_sigma_deg,
                       "burst": [s.burst_min, s.burst_max], "burst_pause_ms": s.burst_pause_ms}))}),
        );
        if skill.is_some() && rom_dir(self.use_rom, self.rom.as_ref()).is_none() {
            // without the ROM the bot has no collision and cannot decode stream positions: it never fires
            rec.event(
                "shooter_warning",
                json!({"why": "no ROM (--rom or CA_ROM): the skill shooter holds fire without a collision world"}),
            );
        }
        Some(ShooterConfig {
            shot_period_ms: self.fire_interval_ms,
            loadout: loadout.to_vec(),
            rom,
            hit_node: self.hit_node,
            seed: self.impairment.seed ^ 0x0005_4007,
            skill,
            throw_every_ms: self.throw_every_ms,
            grenade_chance: if skill.is_some() {
                self.grenade_chance
            } else {
                0.0
            },
            table: Some(table),
            ..ShooterConfig::default()
        })
    }
}

/// The ROM folder for a run (explicit folder, else `CA_ROM`), or none.
pub fn rom_dir(use_rom: bool, rom: Option<&PathBuf>) -> Option<PathBuf> {
    if !use_rom {
        return None;
    }
    rom.cloned().or_else(|| dc_rez::dc_rom::resolve_rom().ok())
}

/// Opens the ROM worlds for a run (explicit folder, else `CA_ROM`), or none.
pub fn open_rom(use_rom: bool, rom: Option<&PathBuf>) -> Option<RomWorlds> {
    if !use_rom {
        return None;
    }
    match rom {
        Some(p) => RomWorlds::open(p).ok(),
        None => RomWorlds::from_env(),
    }
}

/// The JSON report of a game session.
pub fn session_report(c: &BotSession) -> Value {
    let o = &c.obs;
    let ids = |m: &std::collections::BTreeMap<u8, u64>| -> Value {
        Value::Object(
            m.iter()
                .map(|(k, v)| (format!("0x{k:02x}"), json!(v)))
                .collect(),
        )
    };
    let order: Vec<Value> = o
        .s2c_order
        .iter()
        .take(400)
        .map(|(t, rel, id, sh)| {
            json!([
                t,
                rel,
                format!("0x{id:02x}"),
                sh.map(|s| format!("0x{s:02x}"))
            ])
        })
        .collect();
    // the distinct sequence of (id, shell) in first-arrival order
    let mut first_seen: Vec<String> = Vec::new();
    for (_, _, id, sh) in &o.s2c_order {
        let k = match sh {
            Some(s) => format!("{}:0x{s:02x}", s2c::name(*id)),
            None => s2c::name(*id).to_string(),
        };
        if !first_seen.contains(&k) {
            first_seen.push(k);
        }
    }
    let own = c.own_object();
    let objects: Vec<Value> = c
        .table
        .objects
        .iter()
        .take(64)
        .map(|(id, ob)| {
            json!({"id": id, "kind": ob.kind, "pos": ob.pos, "records": ob.records,
                   "stream_entries": ob.stream_entries, "travel": ob.travel, "own": Some(*id) == own})
        })
        .collect();
    let players: Vec<Value> = o
        .roster
        .values()
        .chain(o.departed.iter())
        .map(|p| {
            let ob = c.table.seen(p.character);
            json!({"pid": p.pid, "name": p.name, "team": p.team, "character": p.character,
                   "pos": ob.and_then(|x| x.pos), "travel": ob.map(|x| x.travel),
                   "pos_min": ob.and_then(|x| x.pos_min), "pos_max": ob.and_then(|x| x.pos_max),
                   "stream_entries": ob.map(|x| x.stream_entries),
                   "samples": ob.map(|x| x.samples.iter().take(24).map(|s| json!([s.0, s.1])).collect::<Vec<_>>())})
        })
        .collect();
    let ls = c.link_stats();
    let link = c.link().map(|l| {
        json!({"outstanding": l.outstanding(), "l_ms": l.l_ms(), "rate_bps": l.rate_bps(),
               "last_in_order": l.last_in_order(), "newest_sent": l.newest_block_seq(),
               "state": format!("{:?}", l.state())})
    });
    json!({
        "stage": format!("{:?}", c.stage()),
        "end": o.end,
        "milestones": o.milestones.iter().map(|(m, t)| json!([m, t])).collect::<Vec<_>>(),
        "first_seen": first_seen,
        "s2c_order_head": order,
        "s2c_counts": ids(&o.s2c_counts),
        "shell_counts": ids(&o.shell_counts),
        "c2s_counts": ids(&o.c2s_counts),
        "c2s_shell_counts": ids(&o.c2s_shell_counts),
        "version": o.version, "client_id": o.client_id,
        "files": o.files.iter().map(|(k, v)| json!([k, v.0, v.1])).collect::<Vec<_>>(),
        "load_world": o.load_world, "announced_world": o.announced_world,
        "world_resolved": o.world_resolved.as_ref().map(|r| match r { Ok(s) => json!({"ok": s}), Err(e) => json!({"err": e}) }),
        "evars": o.evars, "preload_subs": o.preload_subs, "sky_count": o.sky_count,
        "view_object": o.view_object, "switch_states": o.switch_states, "life_states": o.life_states,
        "mode_class": o.mode_class, "integrity": o.integrity, "console": o.console,
        "teleports": o.teleports, "kills": o.kills,
        "updates_sent": o.updates_sent, "join_asks": o.join_asks, "app_id_index": o.app_id_index,
        "jump_inputs": o.jump_inputs, "jumps_taken": o.jumps_taken, "watchdog_trips": o.watchdog_trips,
        "jumps_skipped": o.jumps_skipped, "escapes": o.escapes,
        "fatal": o.fatal, "parse_notes": o.parse_notes,
        "objects": objects, "players": players,
        "own_pos": c.body().map(|b| b.pos()),
        "server_time": c.table.server_time, "time_entries": c.table.time_entries,
        "stale_time_entries": c.table.stale_time_entries,
        "object_records": {"creates": c.table.creates, "changes": c.table.changes, "removals": c.table.removals,
                           "sounds": c.table.sound_records, "stream_entries": c.table.stream_entries,
                           "unknown_stream_entries": c.table.unknown_stream_entries},
        "link": link,
        "link_stats": ls.map(|s| json!({
            "datagrams_out": s.datagrams_out, "datagrams_in": s.datagrams_in, "bad_check": s.bad_check,
            "rel_blocks_out": s.rel_blocks_out, "resends": s.resends, "keepalive_blocks": s.keepalive_blocks,
            "acks_out": s.acks_out, "flow_drops": s.flow_drops, "rel_in_order": s.rel_in_order,
            "rel_stored": s.rel_stored, "rel_discarded": s.rel_discarded, "rel_slots_full": s.rel_slots_full,
            "rel_abandoned": s.rel_abandoned, "delivered_rel": s.delivered_rel, "delivered_unrel": s.delivered_unrel,
            "unrel_expired": s.unrel_expired, "split_parts_out": s.split_parts_out, "max_body_bits": s.max_body_bits,
            "max_datagram_bytes": s.max_datagram_bytes, "oversize_out": s.oversize_out})),
        "max_outstanding": c.max_outstanding, "paused_polls": c.paused_seen, "stalled_polls": c.stalled_seen,
        "dropped_by_bot": {"out": c.dropped.0, "in": c.dropped.1},
        "shooter": c.shooter.as_ref().map(|s| json!({
            "selects": s.stats.selects, "shots": s.stats.shots, "fire_msgs": s.stats.fire_msgs,
            "reloads": s.stats.reloads, "throws": s.stats.throws, "kills_by_me": s.stats.kills_by_me, "target_downs": s.stats.target_downs,
            "target_ups": s.stats.target_ups, "target_unseen": s.stats.target_unseen,
            "target_seen": s.stats.target_seen, "hunt_decisions": c.hunter.decisions, "hunt_flips": c.hunter.flips,
            "skill": s.config().skill.map(|k| k.level), "hit_claims": s.stats.hit_claims,
            "miss_reports": s.stats.miss_reports, "reactions": s.stats.reactions, "bursts": s.stats.bursts})),
    })
}

/// What the bot logs in with: its account name, its name, and the login password when one was given (else the
/// harness text).
// DC-DEFAULT: BOT-83 (proposed) the account password goes in the login packet's password box
pub fn login_options(cfg: &BotConfig) -> LoginOptions {
    let mut lo = LoginOptions::new(cfg.auth, &cfg.user, &cfg.nick);
    lo.timeout = cfg.lobby_timeout;
    if let Some(pw) = &cfg.login_password {
        lo.password = pw.clone();
    }
    lo.mod_set_hash = cfg.mod_set_hash.clone();
    lo.content_hash = cfg.content_hash.clone();
    lo.server_id = cfg.server_id;
    lo
}

/// A whole bot run: lobby login, room, start, game session. Returns the report (also on a failed session; an
/// error only when the lobby part failed before a game address was known).
pub fn run_bot(cfg: &BotConfig, rec: &Recorder) -> BotResult<Value> {
    let lo = login_options(cfg);
    let ticket = auth_login(&lo, rec)?;
    rec.event(
        "auth_ok",
        json!({"account_no": ticket.account_no, "lobby": ticket.lobby.to_string(),
               "created_character": ticket.created_character}),
    );
    let mut lobby = Lobby::login(&ticket, cfg.channel, rec, cfg.lobby_timeout)?;
    match &cfg.room {
        RoomChoice::New {
            max_players,
            title,
            mode,
            start_after,
        } => {
            lobby.open_room(*max_players, title, &cfg.password, *mode)?;
            timeline_team(&mut lobby, cfg, rec)?;
            if let Some(g) = cfg.goal {
                let dir = rom_dir(cfg.use_rom, cfg.rom.as_ref());
                set_goal(&mut lobby, *mode, g, dir.as_deref(), cfg.lobby_timeout, rec)?;
            }
            // DC-DEFAULT: BOT-99 (proposed) our request 0x3f06; the match needs nothing else of the bot: it fires what
            // the server grants (0xb4, 0xb2, then the usual pick)
            if cfg.gun_game {
                let mut w = dc_wire::tcp::PktWriter::new(dc_wire::gungame::REQUEST);
                w.u32(1);
                lobby.link.send(&w)?;
                rec.event("room_gun_game", json!({"on": true}));
            }
            // DC-DEFAULT: BOT-100 (proposed) our request 0x3f0b
            if cfg.possession {
                let mut w = dc_wire::tcp::PktWriter::new(dc_wire::possession::REQUEST);
                w.u32(1);
                lobby.link.send(&w)?;
                rec.event("room_possession", json!({"on": true}));
            }
            for (kind, value) in cfg.timeline.iter().flat_map(|t| t.settings.iter().copied()) {
                let r =
                    crate::hostroom::set_setting(&mut lobby, *mode, kind, value, cfg.lobby_timeout);
                rec.event(
                    "room_setting",
                    json!({"field": kind, "value": value, "ok": r.is_ok(), "error": r.err().map(|e| e.0)}),
                );
            }
            lobby.wait_members(*start_after, cfg.match_wait)?;
            lobby.start()?;
        }
        RoomChoice::Number(n) => {
            lobby.join_room_no(*n, &cfg.password)?;
            timeline_team(&mut lobby, cfg, rec)?;
            lobby.ready()?;
        }
        RoomChoice::Any => {
            lobby.set_filler(cfg.filler.clone());
            lobby.set_room_title(cfg.room_title.clone());
        }
    }
    let mut addr = match &cfg.room {
        // BOT-84: a filler finds a room with a player, else the next one
        RoomChoice::Any => filler_seat(&mut lobby, cfg, rec)?,
        _ => lobby.wait_match_address(cfg.match_wait)?,
    };
    let mut played = 0u32;
    loop {
        lobby.mark_playing();
        let (mut rep, back) = play_match(cfg, lobby, &addr, rec)?;
        played += 1;
        if let Value::Object(m) = &mut rep {
            m.insert("matches_played".into(), json!(played));
        }
        let more = cfg.matches == 0 || played < cfg.matches;
        let failed = rep["stage"] != "Closed";
        let Some(mut l) = back.filter(|_| more && !failed) else {
            return Ok(rep);
        };
        // DC-DEFAULT: BOT-62 the seat plays the next match: back in the waiting room, ready again (the host waits
        // for its members and starts), then the new match address
        let start_after = match &cfg.room {
            RoomChoice::New { start_after, .. } => *start_after,
            _ => 1,
        };
        match l.next_match(start_after, cfg.match_wait) {
            Ok(a) => {
                rec.event(
                    "next_match",
                    json!({"match": played + 1, "ip": a.ip, "port": a.port}),
                );
                lobby = l;
                addr = a;
            }
            // BOT-84: the players left this room: the filler leaves too and seats itself in the next room
            // BOT-156: a weapon limit set between matches that the loadout fails: the same, through the limit's leave
            Err(e)
                if e.0 == crate::lobby::FILLER_ALONE
                    || (cfg.filler.is_some() && e.0 == crate::lobby::WEAPON_LIMIT_REFUSED) =>
            {
                rec.event("next_match_room_left", json!({"after": played}));
                if e.0 == crate::lobby::WEAPON_LIMIT_REFUSED {
                    l.leave_for_weapon_limit()?;
                } else if l.room.is_some() {
                    l.leave_room()?;
                }
                match filler_seat(&mut l, cfg, rec) {
                    Ok(a) => {
                        lobby = l;
                        addr = a;
                    }
                    Err(e) => {
                        rec.event("next_match_end", json!({"after": played, "why": e.0}));
                        rec.flush();
                        return Ok(rep);
                    }
                }
            }
            Err(e) => {
                rec.event("next_match_end", json!({"after": played, "why": e.0}));
                rec.flush();
                return Ok(rep);
            }
        }
    }
}

/// `--room any`: joins the first listed room, readies and waits for the match address. A filler bot (BOT-84)
/// leaves a room that holds no player (only bots of its prefix, or nobody) and takes the next listed room, until
/// an address arrives or `--match-wait` passes without a room.
// DC-DEFAULT: BOT-84 (proposed)
fn filler_seat(lobby: &mut Lobby, cfg: &BotConfig, rec: &Recorder) -> BotResult<MatchAddress> {
    loop {
        let n = lobby.find_room(cfg.match_wait)?;
        // LG-319: a refused bot waits and looks again, it does not start over: the room is full or keeps its last seat
        if let Err(reason) = lobby.try_join_room(n, &cfg.password)? {
            if cfg.filler.is_none() {
                return Err(crate::bot_err!("lobby: room {n} refused, reason 0x{reason:x}"));
            }
            rec.event("room_refused", json!({"room": n, "reason": reason}));
            lobby.skip_room(n);
            lobby.link.pump(Duration::from_millis(500))?;
            continue;
        }
        timeline_team(lobby, cfg, rec)?;
        if lobby.filler_alone() {
            rec.event(
                "room_skipped",
                json!({"room": n, "why": "no player in the room"}),
            );
            lobby.leave_room()?;
            // the room is passed over for a while; the next listed room (a player's) is taken instead
            lobby.skip_room(n);
            lobby.link.pump(Duration::from_millis(500))?;
            continue;
        }
        if let Err(e) = lobby.ready() {
            // LG-319: the seat was taken back between the entry and the ready
            if cfg.filler.is_some() && lobby.removed_from_room() {
                continue;
            }
            // BOT-156: the loadout fails the room's weapon limit: leave, pass the room over, look elsewhere
            if cfg.filler.is_some() && e.0 == crate::lobby::WEAPON_LIMIT_REFUSED {
                lobby.leave_for_weapon_limit()?;
                lobby.link.pump(Duration::from_millis(500))?;
                continue;
            }
            return Err(e);
        }
        match lobby.wait_match_address(cfg.match_wait) {
            Ok(a) => return Ok(a),
            Err(e) if cfg.filler.is_some() && e.0 == crate::lobby::WEAPON_LIMIT_REFUSED => {
                lobby.leave_for_weapon_limit()?;
                lobby.link.pump(Duration::from_millis(500))?;
            }
            Err(e) if e.0 == crate::lobby::FILLER_ALONE => {
                rec.event("room_left_waiting", json!({"room": n}));
                lobby.leave_room()?;
                lobby.skip_room(n);
                lobby.link.pump(Duration::from_millis(500))?;
            }
            Err(e) => return Err(e),
        }
    }
}

/// One match on the game server of `addr`; returns the session report and the lobby connection (None when it
/// closed meanwhile).
fn play_match(
    cfg: &BotConfig,
    lobby: Lobby,
    addr: &MatchAddress,
    rec: &Recorder,
) -> BotResult<(Value, Option<Lobby>)> {
    let server = addr.socket_addr()?;
    let rom = open_rom(cfg.use_rom, cfg.rom.as_ref());
    let mut gc = GameConfig::new(&lobby.profile.nick);
    // LG-318: a seat's account name is its name in lower case
    gc.hello_tag = if cfg.user.is_empty() {
        lobby.profile.nick.to_ascii_lowercase()
    } else {
        cfg.user.clone()
    };
    gc.seat_ticket = addr.ticket;
    gc.motion = cfg.motion;
    gc.force_updates = cfg.force_updates;
    gc.state_form = cfg.state_form;
    gc.update_hz = cfg.update_hz;
    gc.load_ms = cfg.load_ms;
    gc.aim_quat = cfg.aim_quat;
    gc.idle_exit_ms = cfg.idle_exit.as_millis().min(u32::MAX as u128) as u32;
    gc.join_timeout_ms = cfg.join_timeout.as_millis().min(u32::MAX as u128) as u32;
    gc.respawn_after_ms = cfg
        .respawn_after
        .map(|d| d.as_millis().min(u32::MAX as u128) as u32);
    gc.integrity_version = rom.as_ref().and_then(RomWorlds::integrity_version);
    gc.leave_after_result = cfg.leave_after_result;
    gc.mod_cache = cfg.mod_cache.clone();
    gc.timeline = cfg.timeline.clone();
    // SPEC: PROTOCOL_2009.md §3.3 (0x323 +0x12 item number per equipment slot)
    gc.shooter = cfg.shooter_config(&lobby.profile.equipped, rec);
    let p = lobby.profile.clone();
    let seat = lobby.room.clone().unwrap_or_default();
    // BOT-97: the room's weapon limit picks the weapon the shooter draws (a sniper rifle in a sniper-only room)
    if let Some(s) = gc.shooter.as_mut() {
        s.weapon_limit = seat.weapon_limit;
    }
    gc.mode_class_hint = rom_dir(cfg.use_rom, cfg.rom.as_ref())
        .and_then(|d| crate::hostroom::rom_mode_class(&d, seat.mode.max(0) as usize));
    rec.event(
        "mode_class_hint",
        json!({"room_class": seat.mode, "mode_class": gc.mode_class_hint}),
    );
    // SPEC: GAME_MESSAGES_2009_C2S.md §5.1 (sources: room slot, 0x303 character number, 0x301 session index,
    // exp, own team, GP, the loadout lists, clan id, nation)
    gc.info = PlayerInfo {
        room_slot: seat.slot,
        char_no: p.char_no,
        session_idx: p.lobby_index as u16,
        exp: p.exp,
        team: lobby.team() as i8,
        gp: p.gp,
        // DC-DEFAULT: BOT-3 (proposed) loadout entries = the equipped item numbers of 0x323, per-item bytes 0
        loadout: p
            .equipped
            .iter()
            .filter(|n| **n > 0 && **n <= u16::MAX as i32)
            .map(|n| (*n as u16, 0, 0, 0))
            .collect(),
        nation: p.nation,
        ..PlayerInfo::default()
    };
    let drain = lobby.drain_in_background();
    let mut client = BotSession::new(gc, rom, rec.clone());
    let closed = drain.closed_flag();
    // BOT-84: a filler quits the match as soon as the room holds no player (the drain thread watches the room)
    let alone = drain.alone_flag();
    let mut left_alone = false;
    let mut alone_stop = |_: &BotSession, _: Millis| {
        let a = alone.load(Ordering::Relaxed);
        if a && !left_alone {
            left_alone = true;
            rec.event("match_left_alone", json!({"why": "no player in the room"}));
        }
        a
    };
    let stop: Option<StopCheck<'_>> = cfg
        .filler
        .as_ref()
        .map(|_| &mut alone_stop as StopCheck<'_>);
    run_game_loop(
        &mut client,
        server,
        cfg.duration,
        &cfg.impairment,
        rec,
        Some(closed),
        stop,
    )?;
    let mut lobby_back = drain.finish();
    let failed = matches!(client.stage(), Stage::Failed(_));
    if failed {
        if let Some(l) = lobby_back.as_mut() {
            let _ = l.report_join_failed();
        }
    }
    let mut rep = session_report(&client);
    if let Value::Object(m) = &mut rep {
        m.insert("bot".into(), json!(cfg.nick));
        m.insert("match_address".into(), json!(server.to_string()));
        m.insert("room".into(), json!(seat.number));
        m.insert("host".into(), json!(seat.host));
        m.insert("team".into(), json!(seat.team_of(p.my_id)));
        if let Some(l) = &lobby_back {
            let tail: Vec<String> = l
                .link
                .received
                .iter()
                .rev()
                .take(40)
                .rev()
                .map(|i| format!("0x{i:x}"))
                .collect();
            m.insert("lobby_tail".into(), json!(tail));
        }
    }
    rec.event("report", rep.clone());
    rec.flush();
    Ok((rep, lobby_back.filter(|l| !l.link.is_closed())))
}

/// The timeline's `team` request (0x390) in the room, before ready; logged as event `team_change`.
fn timeline_team(lobby: &mut Lobby, cfg: &BotConfig, rec: &Recorder) -> BotResult<()> {
    let Some(team) = cfg.timeline.as_ref().and_then(|t| t.team) else {
        return Ok(());
    };
    let (ok, reason) = lobby.ask_team(team)?;
    lobby.absorb_room_pushes();
    rec.event(
        "team_change",
        json!({"asked": team, "ok": ok, "reason": reason, "team": lobby.team()}),
    );
    Ok(())
}

/// The fields of the player-info answer 0x72 from what the lobby told the player: room slot, character number,
/// lobby session index, experience, own team, GP, the equipped items as loadout entries, nation (the sources
/// [`run_bot`] uses, for a caller that runs its own game session).
// SPEC: GAME_MESSAGES_2009_C2S.md §5.1 (sources: room slot, 0x303 character number, 0x301 session index, exp, own
// team, GP, the loadout lists, clan id, nation)
// DC-DEFAULT: BOT-3 (proposed) loadout entries = the equipped item numbers of 0x323, per-item bytes 0
pub fn player_info_from_lobby(lobby: &Lobby) -> PlayerInfo {
    let p = &lobby.profile;
    let seat = lobby.room.clone().unwrap_or_default();
    PlayerInfo {
        room_slot: seat.slot,
        char_no: p.char_no,
        session_idx: p.lobby_index as u16,
        exp: p.exp,
        team: lobby.team() as i8,
        gp: p.gp,
        loadout: p
            .equipped
            .iter()
            .filter(|n| **n > 0 && **n <= u16::MAX as i32)
            .map(|n| (*n as u16, 0, 0, 0))
            .collect(),
        nation: p.nation,
        ..PlayerInfo::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_password_goes_in_the_password_box() {
        let auth: SocketAddr = "127.0.0.1:10000".parse().unwrap();
        let mut cfg = BotConfig::new(auth, "Bravo");
        let lo = login_options(&cfg);
        assert_eq!((lo.user.as_str(), lo.password.as_str()), ("bravo", "pw"));
        cfg.login_password = Some("s3cret pass".into());
        cfg.password = "room".into();
        assert_eq!(login_options(&cfg).password, "s3cret pass");
    }

    /// LG-271: no flag, no suffix (vanilla lobbies take it); `--mod-set-hash` adds `;m=<hash>;c=<content or 0>`.
    #[test]
    fn the_mod_hashes_follow_the_login_name_only_when_given() {
        let auth: SocketAddr = "127.0.0.1:10000".parse().unwrap();
        let mut cfg = BotConfig::new(auth, "Bravo");
        assert_eq!(login_options(&cfg).login_field(), "LOCAL:bravo;b=1");
        let set = "ab".repeat(32);
        cfg.mod_set_hash = Some(set.clone());
        assert_eq!(
            login_options(&cfg).login_field(),
            format!("LOCAL:bravo;b=1;m={set};c=0")
        );
        cfg.content_hash = Some("cd".repeat(32));
        let field = login_options(&cfg).login_field();
        let (name, mods) = dc_wire::loginmods::split(&field);
        assert_eq!(
            (name, mods.set.as_str(), mods.content),
            ("LOCAL:bravo;b=1", set.as_str(), "cd".repeat(32))
        );
    }
}
