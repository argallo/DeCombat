//! Remote game hosts (`dc-host` agents): the TLS-only host port, the hello and its `ok` / `refuse`, the registry of
//! the agents online, and the control lines that carry their game servers' pipes (`start`, `line`, `quit`, `ping`
//! out; `started`, `failed`, `event`, `exited`, `running`, `pong` in).
// SPEC: HOST_AGENT.md §3 (control protocol), §4 (connection life), §5 (security)
// DC-DEFAULT: LG-300 (proposed) host port and registry

use crate::game::Link;
use crate::net::{game_line, lock, report, run_deferred};
use crate::state::{Core, Shared};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The longest control line, without its `\n` (HOST_AGENT.md §3); a longer one is dropped with a log line.
pub const MAX_LINE: usize = 16 * 1024;
/// How long a socket read waits before the loop looks at the silence and ping clocks.
const TICK: Duration = Duration::from_millis(250);
/// The most host connections in their TLS handshake at once.
const MAX_HANDSHAKES: usize = 8;

/// What a host connection's writer thread receives.
#[derive(Debug)]
pub enum HostOut {
    /// One line, without its `\n`.
    Line(String),
    /// HA-2: a line of a build transfer; the writer takes its length off the counter once it is out, so the sender
    /// keeps at most [`UPDATE_QUEUE`] bytes queued.
    Paced(String, Arc<std::sync::atomic::AtomicUsize>),
    Close,
}

/// The clocks of the control connection (HOST_AGENT.md §3, §5); tests shorten them.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// A side that hears nothing for this long closes the stream (60 s).
    pub silence: Duration,
    /// The lobby's `ping` interval (well inside the agent's own 60 s).
    pub ping_every: Duration,
    /// The wait before a `refuse` (a wrong name or token costs the guesser time).
    pub refuse_delay: Duration,
    /// The time the agent has for its hello after the TLS handshake.
    pub hello_wait: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            silence: Duration::from_secs(60),
            ping_every: Duration::from_secs(20),
            refuse_delay: Duration::from_secs(1),
            hello_wait: Duration::from_secs(10),
        }
    }
}

/// One agent online.
pub struct Agent {
    /// The connection number (the registry's own count).
    pub conn: u64,
    /// The writer thread of its connection.
    pub tx: Sender<HostOut>,
    /// The address advertised for its game servers: `public_address`, else the peer address.
    pub address: String,
    pub peer: String,
    /// The UDP range of its hello (`ports=<lo>-<hi>`).
    pub ports: Option<(u16, u16)>,
    pub version: String,
    /// LG-313: its hello says it takes weapon content (`content=1`).
    pub takes_content: bool,
    /// MAP-83: its hello says it takes map packs (`packs=1`).
    pub takes_packs: bool,
    /// MAP-84: the packs it has: said by `have`, or sent on this connection.
    pub packs: std::collections::BTreeSet<String>,
    pub since: Instant,
    /// HA-2: its hello says it takes the lobby's server build (`update=1`).
    pub takes_update: bool,
    /// HA-2: the build last offered on this connection, and when.
    pub offered: Option<(String, Instant)>,
    /// HA-2: a transfer of the build is under way / how many this connection had.
    pub sending: bool,
    pub sends: u32,
}

/// The agents online (an agent that is not listed is offline).
#[derive(Default)]
pub struct Registry {
    pub online: BTreeMap<String, Agent>,
    next_conn: u64,
    pub timing: Timing,
}

/// A parsed hello.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    pub name: String,
    pub token: String,
    pub version: String,
    pub ports: Option<(u16, u16)>,
    pub running: u32,
    /// LG-313: `content=1` in the hello.
    pub takes_content: bool,
    /// MAP-83: `packs=1` in the hello.
    pub takes_packs: bool,
    /// HA-2: `update=1` in the hello.
    pub takes_update: bool,
}

fn kv(rest: &str) -> BTreeMap<&str, &str> {
    rest.split_whitespace()
        .filter_map(|t| t.split_once('='))
        .collect()
}

/// `hello name=<name> token=<token> version=<v> ports=<lo>-<hi> running=<n>`; None when it is no hello or has no
/// name.
pub fn parse_hello(line: &str) -> Option<Hello> {
    let rest = line.strip_prefix("hello ")?;
    let m = kv(rest);
    let name = m.get("name").filter(|n| !n.is_empty())?.to_string();
    let ports = m.get("ports").and_then(|p| {
        let (a, b) = p.split_once('-')?;
        Some((a.parse().ok()?, b.parse().ok()?))
    });
    Some(Hello {
        name,
        token: m.get("token").map(|t| t.to_string()).unwrap_or_default(),
        version: m
            .get("version")
            .map(|t| t.to_string())
            .unwrap_or_else(|| "unknown".into()),
        ports,
        running: m.get("running").and_then(|n| n.parse().ok()).unwrap_or(0),
        takes_content: m.get("content") == Some(&"1"),
        takes_packs: m.get("packs") == Some(&"1"),
        takes_update: m.get("update") == Some(&"1"),
    })
}

/// A token comparison whose time does not depend on where the tokens differ.
pub fn token_eq(got: &str, want: &str) -> bool {
    let (a, b) = (got.as_bytes(), want.as_bytes());
    let mut diff = (a.len() != b.len()) as u8;
    for (i, &w) in b.iter().enumerate() {
        diff |= a.get(i).copied().unwrap_or(!w) ^ w;
    }
    std::hint::black_box(diff) == 0
}

/// One line of an agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentLine {
    Started {
        gid: u64,
        port: u16,
        pid: u32,
    },
    Failed {
        gid: u64,
        reason: String,
    },
    Event {
        gid: u64,
        text: String,
    },
    Exited {
        gid: u64,
        code: i64,
    },
    Running {
        gid: u64,
        port: u16,
        pid: u32,
    },
    /// MAP-84: `have <sha>`: a map pack the agent keeps from before.
    Have(String),
    /// HA-2: `getupdate <commit>`: the agent asks for the offered build.
    GetUpdate(String),
    Ping(String),
    Pong(String),
    /// Anything else (a newer agent may send more; logged).
    Unknown(String),
}

/// Parses one agent line (without its newline).
pub fn parse_line(line: &str) -> AgentLine {
    let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
    let (g, tail) = rest.split_once(' ').unwrap_or((rest, ""));
    let gid = g.parse::<u64>().ok();
    let m = kv(tail);
    let port = || m.get("port").and_then(|p| p.parse::<u16>().ok());
    let pid = || {
        m.get("pid")
            .and_then(|p| p.parse::<u32>().ok())
            .unwrap_or(0)
    };
    let unknown = || AgentLine::Unknown(line.to_string());
    match (verb, gid) {
        ("started", Some(gid)) => match port() {
            Some(port) => AgentLine::Started {
                gid,
                port,
                pid: pid(),
            },
            None => unknown(),
        },
        ("running", Some(gid)) => match port() {
            Some(port) => AgentLine::Running {
                gid,
                port,
                pid: pid(),
            },
            None => unknown(),
        },
        ("failed", Some(gid)) => AgentLine::Failed {
            gid,
            reason: tail.trim().to_string(),
        },
        // the text after the one space following the gid, verbatim
        ("event", Some(gid)) => AgentLine::Event {
            gid,
            text: tail.to_string(),
        },
        ("exited", Some(gid)) => AgentLine::Exited {
            gid,
            code: m.get("code").and_then(|c| c.parse().ok()).unwrap_or(-1),
        },
        ("have", _) => AgentLine::Have(rest.trim().to_string()),
        ("getupdate", _) => AgentLine::GetUpdate(rest.trim().to_string()),
        ("ping", _) => AgentLine::Ping(rest.trim().to_string()),
        ("pong", _) => AgentLine::Pong(rest.trim().to_string()),
        _ => unknown(),
    }
}

/// The address of a peer without the IPv4-mapped IPv6 form.
fn peer_ip(peer: &SocketAddr) -> String {
    match peer.ip() {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(|v4| v4.to_string())
            .unwrap_or_else(|| v6.to_string()),
        v4 => v4.to_string(),
    }
}

/// The hello's verdict: the agent's advertised address, or the reason of the refusal. A known name with the right
/// token that is not online yet is accepted.
pub fn judge(st: &Core, hello: Option<&Hello>, peer: &SocketAddr) -> Result<String, String> {
    let Some(h) = hello else {
        return Err("no hello".into());
    };
    let cfg = st.opts.host_agents.iter().find(|a| a.name == h.name);
    // the token is compared even for an unknown name (against nothing), so both refusals take the same time
    let want = cfg.map(|c| c.token.as_str()).unwrap_or("");
    let token_ok = token_eq(&h.token, want) && want.chars().count() >= dc_config::MIN_HOST_TOKEN;
    match cfg {
        None => Err("unknown name".into()),
        Some(_) if !token_ok => Err("bad token".into()),
        Some(c) if st.hosts.online.contains_key(&c.name) => {
            Err("already connected (one connection per name)".into())
        }
        Some(c) if !c.public_address.trim().is_empty() => Ok(c.public_address.trim().to_string()),
        Some(_) => Ok(peer_ip(peer)),
    }
}

/// Where a host connection's lines go out.
pub trait Sink: Send + 'static {
    fn put(&mut self, data: &[u8]) -> io::Result<()>;
    fn end(&mut self);
}

impl Sink for crate::tls::ConnWriter {
    fn put(&mut self, data: &[u8]) -> io::Result<()> {
        self.write_all(data)
    }
    fn end(&mut self) {
        self.close()
    }
}

impl Sink for TcpStream {
    fn put(&mut self, data: &[u8]) -> io::Result<()> {
        self.write_all(data).and_then(|_| self.flush())
    }
    fn end(&mut self) {
        let _ = self.shutdown(std::net::Shutdown::Both);
    }
}

/// What one read of [`Lines`] gave.
enum Got {
    Line(String),
    /// A line over [`MAX_LINE`] was dropped.
    Long,
    /// Nothing arrived within one tick.
    Idle,
    End,
}

/// The `\n`-separated lines of a stream whose reads time out every [`TICK`].
struct Lines<R> {
    r: R,
    buf: Vec<u8>,
    skipping: bool,
}

impl<R: Read> Lines<R> {
    fn next(&mut self) -> Got {
        loop {
            if let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
                let mut l: Vec<u8> = self.buf.drain(..=i).collect();
                l.pop();
                if l.last() == Some(&b'\r') {
                    l.pop();
                }
                if std::mem::take(&mut self.skipping) || l.len() > MAX_LINE {
                    return Got::Long;
                }
                return Got::Line(String::from_utf8_lossy(&l).into_owned());
            }
            if self.buf.len() > MAX_LINE {
                self.buf.clear();
                self.skipping = true;
            }
            let mut tmp = [0u8; 4096];
            match self.r.read(&mut tmp) {
                Ok(0) => return Got::End,
                Ok(n) => self.buf.extend_from_slice(&tmp[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return Got::Idle
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return Got::End,
            }
        }
    }
}

/// Serves one host connection after its TLS handshake (the reads of `reader` must time out, [`TICK`]): the hello,
/// then the control lines until the stream ends or stays silent; the agent is offline afterwards.
pub fn serve<R: Read, S: Sink>(shared: &Shared, reader: R, mut sink: S, peer: SocketAddr) {
    let timing = lock(shared).hosts.timing;
    let mut lines = Lines {
        r: reader,
        buf: Vec::new(),
        skipping: false,
    };
    let start = Instant::now();
    let first = loop {
        match lines.next() {
            Got::Line(l) => break Some(l),
            Got::Idle if start.elapsed() < timing.hello_wait => {}
            Got::Long => break None,
            Got::Idle | Got::End => break None,
        }
    };
    let hello = first.as_deref().and_then(parse_hello);
    let (tx, rx) = channel::<HostOut>();
    let verdict = {
        let mut g = lock(shared);
        match judge(&g, hello.as_ref(), &peer) {
            Ok(address) => {
                let h = hello.clone().expect("judged");
                g.hosts.next_conn += 1;
                let conn = g.hosts.next_conn;
                // `ok` is queued before the agent is visible to a room start, so it is the first line out
                let _ = tx.send(HostOut::Line(format!(
                    "ok name={} lobby={}",
                    h.name,
                    env!("CARGO_PKG_VERSION")
                )));
                g.hosts.online.insert(
                    h.name.clone(),
                    Agent {
                        conn,
                        tx: tx.clone(),
                        address: address.clone(),
                        peer: peer.to_string(),
                        ports: h.ports,
                        version: h.version.clone(),
                        takes_content: h.takes_content,
                        takes_packs: h.takes_packs,
                        packs: Default::default(),
                        since: Instant::now(),
                        takes_update: h.takes_update,
                        offered: None,
                        sending: false,
                        sends: 0,
                    },
                );
                offer_update(&mut g, &h.name);
                g.note(&format!(
                    "host {} online from {} (version {}, ports {}, {} running), its game servers announced at {}",
                    h.name,
                    peer,
                    h.version,
                    h.ports
                        .map(|(a, b)| format!("{a}-{b}"))
                        .unwrap_or_else(|| "?".into()),
                    h.running,
                    address
                ));
                Ok((h.name, conn))
            }
            Err(why) => {
                g.note(&format!(
                    "host connection from {} refused: {}{}",
                    peer,
                    why,
                    hello
                        .as_ref()
                        .map(|h| format!(" (name {})", h.name))
                        .unwrap_or_default()
                ));
                Err(why)
            }
        }
    };
    let (name, conn) = match verdict {
        Ok(v) => v,
        Err(why) => {
            std::thread::sleep(timing.refuse_delay);
            let _ = sink.put(format!("refuse {why}\n").as_bytes());
            sink.end();
            return;
        }
    };
    let _ = std::thread::Builder::new()
        .name(format!("host-{name}-w"))
        .spawn(move || {
            for o in rx {
                match o {
                    HostOut::Line(l) => {
                        let mut b = l.into_bytes();
                        b.push(b'\n');
                        if sink.put(&b).is_err() {
                            break;
                        }
                    }
                    HostOut::Paced(l, queued) => {
                        let n = l.len();
                        let mut b = l.into_bytes();
                        b.push(b'\n');
                        let ok = sink.put(&b).is_ok();
                        queued.fetch_sub(n, std::sync::atomic::Ordering::AcqRel);
                        if !ok {
                            break;
                        }
                    }
                    HostOut::Close => break,
                }
            }
            sink.end();
        });
    let mut heard = Instant::now();
    let mut pinged = Instant::now();
    let mut ping_n = 0u64;
    let why = loop {
        match lines.next() {
            Got::Line(l) => {
                heard = Instant::now();
                let mut g = lock(shared);
                on_agent_line(shared, &mut g, &name, &tx, &l);
            }
            Got::Long => {
                heard = Instant::now();
                lock(shared).note(&format!(
                    "host {name}: a line over {MAX_LINE} bytes dropped"
                ));
            }
            Got::Idle => {
                if heard.elapsed() >= timing.silence {
                    break format!("silent for {:?}", timing.silence);
                }
            }
            Got::End => break "connection closed".to_string(),
        }
        if pinged.elapsed() >= timing.ping_every {
            ping_n += 1;
            pinged = Instant::now();
            let _ = tx.send(HostOut::Line(format!("ping {ping_n}")));
            // HA-2: a build deployed since the hello (deploy_from_git.sh `hosts`) is offered here
            offer_update(&mut lock(shared), &name);
        }
    };
    let _ = tx.send(HostOut::Close);
    let mut g = lock(shared);
    offline(&mut g, &name, conn, &why);
}

/// The agent's stream ended: it is offline, and the lines of its games are lost until it reconnects.
fn offline(st: &mut Core, name: &str, conn: u64, why: &str) {
    if st.hosts.online.get(name).map(|a| a.conn) != Some(conn) {
        return;
    }
    st.hosts.online.remove(name);
    let mut n = 0;
    for g in st.games.values_mut() {
        if let Some(r) = g.link.remote_mut().filter(|r| r.agent == name) {
            r.tx = None;
            n += 1;
        }
    }
    st.note(&format!(
        "host {name} offline ({why}); {n} game(s) of it unreachable until it reconnects"
    ));
}

/// True when game `gid` is a remote game of agent `name`.
fn ours(st: &Core, gid: u64, name: &str) -> bool {
    st.games
        .get(&gid)
        .and_then(|g| g.link.remote())
        .map(|r| r.agent == name)
        .unwrap_or(false)
}

/// One control line of agent `name` (under the state lock).
fn on_agent_line(shared: &Shared, g: &mut Core, name: &str, tx: &Sender<HostOut>, line: &str) {
    let quit = |gid: u64| {
        let _ = tx.send(HostOut::Line(format!("quit {gid}")));
    };
    match parse_line(line) {
        AgentLine::Started { gid, port, pid } => {
            if !ours(g, gid, name) {
                quit(gid);
                g.note(&format!(
                    "host {name}: started game {gid} that is not ours, told to quit"
                ));
                return;
            }
            if let Some(gm) = g.games.get_mut(&gid) {
                gm.port = port;
                if let Some(r) = gm.link.remote_mut() {
                    r.port = Some(port);
                    r.pid = Some(pid);
                }
            }
            let room = g.games.get(&gid).map(|x| x.room);
            if let Some(k) = room {
                g.note(&format!(
                    "game {}: room {}-{} server pid {} port {} on host {}",
                    gid, k.channel, k.no, pid, port, name
                ));
            }
        }
        AgentLine::Running { gid, port, pid } => {
            if !ours(g, gid, name) {
                quit(gid);
                g.note(&format!(
                    "host {name}: game {gid} still runs there but not here, told to quit"
                ));
                return;
            }
            let mut quit_again = false;
            if let Some(gm) = g.games.get_mut(&gid) {
                if gm.port == 0 {
                    gm.port = port;
                }
                if let Some(r) = gm.link.remote_mut() {
                    r.tx = Some(tx.clone());
                    r.port = Some(port);
                    r.pid = Some(pid);
                    r.alive = true;
                    // a quit that went out while the agent was offline was lost: say it again
                    quit_again = !r.stdin_open;
                }
            }
            if quit_again {
                quit(gid);
            }
            g.note(&format!(
                "game {gid}: reattached on host {name} (port {port}, pid {pid})"
            ));
        }
        AgentLine::GetUpdate(commit) => send_update(shared, g, name, tx, &commit),
        AgentLine::Have(sha) => {
            if let Some(a) = g.hosts.online.get_mut(name) {
                a.packs.insert(sha.clone());
            }
            g.note(&format!("host {name}: has map pack {sha}"));
        }
        AgentLine::Failed { gid, reason } => {
            // MAP-84: `failed <gid> pack <sha>: ...`: the agent does not hold that pack, the next start sends it again
            let lost = reason
                .strip_prefix("pack ")
                .and_then(|r| r.split(':').next());
            if let (Some(sha), Some(a)) = (lost, g.hosts.online.get_mut(name)) {
                a.packs.remove(sha);
            }
            if ours(g, gid, name) {
                g.note(&format!(
                    "game {gid}: host {name} could not start the server: {reason}"
                ));
                gone(shared, g, gid, &format!("start failed: {reason}"));
            }
        }
        AgentLine::Event { gid, text } => {
            if ours(g, gid, name) {
                game_line(shared, g, gid, &text);
            } else if g.opts.dserver_verbose {
                g.note(&format!("host {name}: event of unknown game {gid} ignored"));
            }
        }
        AgentLine::Exited { gid, code } => {
            if ours(g, gid, name) {
                gone(shared, g, gid, &format!("exit code: {code}"));
            }
        }
        AgentLine::Ping(n) => {
            let _ = tx.send(HostOut::Line(format!("pong {n}")));
        }
        AgentLine::Pong(_) => {}
        AgentLine::Unknown(l) => g.note(&format!("host {name}: line ignored: {l}")),
    }
}

/// HA-2: an agent on another build hears the offer again after this long (it may have been busy).
const OFFER_EVERY: Duration = Duration::from_secs(120);
/// HA-2: the most build transfers one connection gets.
const MAX_SENDS: u32 = 3;
/// HA-2: the raw bytes of one `updpart` line (16000 base64 characters, under the 16 KiB line limit).
// DC-DEFAULT: HA-2 (proposed)
pub const UPDATE_CHUNK: usize = 12_000;
/// HA-2: the largest build the lobby sends.
const MAX_UPDATE_BYTES: u64 = 256 * 1024 * 1024;
/// HA-2: the most bytes of a transfer queued for the writer at once.
pub const UPDATE_QUEUE: usize = 2 * 1024 * 1024;
/// HA-2: a writer that takes nothing for this long ends the transfer (the connection is stuck or gone).
const UPDATE_STALL: Duration = Duration::from_secs(60);

/// HA-2: offers agent `name` the build of `[deploy] dir`'s `host/desired.txt` when its hello took offers and it runs
/// another build (an agent of an unknown build is left alone); at most once per [`OFFER_EVERY`] per build. The build
/// must be this lobby's own, or published after this lobby started (a deploy to the hosts alone): a lobby installed
/// by hand since (deploy_all.sh) does not send its hosts back to an older build.
pub fn offer_update(st: &mut Core, name: &str) {
    let Some(b) = st.opts.deploy_dir.as_deref().and_then(crate::deploy::host_build) else {
        return;
    };
    let started = std::time::SystemTime::now().checked_sub(st.started.elapsed());
    let fresh = matches!((b.published, started), (Some(p), Some(s)) if p > s);
    if !fresh && !crate::opsmon::same_build(crate::opsmon::build_sha(), &b.commit) {
        return;
    }
    let Some(a) = st.hosts.online.get_mut(name) else {
        return;
    };
    if !a.takes_update || a.sending || a.version == "unknown" || crate::opsmon::same_build(&a.version, &b.commit) {
        return;
    }
    let first = a.offered.as_ref().is_none_or(|(c, _)| *c != b.commit);
    if !first && a.offered.as_ref().is_some_and(|(_, t)| t.elapsed() < OFFER_EVERY) {
        return;
    }
    let _ = a.tx.send(HostOut::Line(format!(
        "update {} {} {} {} {} {}",
        b.commit, b.sha256, b.size, b.tarball, b.seq, b.sig
    )));
    a.offered = Some((b.commit.clone(), Instant::now()));
    let version = a.version.clone();
    if first {
        st.note(&format!(
            "host {name}: runs {version}, offered build {} ({} bytes)",
            &b.commit[..8],
            b.size
        ));
    }
}

/// HA-2: `getupdate <commit>`: the offered build to an idle agent, as `updpart` lines and `updend`, from a thread of
/// its own that reads the file in pieces (never more than [`UPDATE_QUEUE`] bytes queued) and hashes as it goes; a file
/// that changed underway ends with `updateno` instead of `updend`, which the agent drops. `updateno <commit> <why>`
/// otherwise.
fn send_update(shared: &Shared, g: &mut Core, name: &str, tx: &Sender<HostOut>, commit: &str) {
    let no = |why: &str| {
        let _ = tx.send(HostOut::Line(format!("updateno {commit} {why}")));
    };
    let b = g.opts.deploy_dir.as_deref().and_then(crate::deploy::host_build);
    let Some(b) = b.filter(|b| b.commit == commit) else {
        g.note(&format!("host {name}: asked for build {commit:?}, which is not the offered one"));
        return no("not the offered build");
    };
    let busy = g
        .games
        .values()
        .any(|x| !x.over && x.link.remote().is_some_and(|r| r.agent == name));
    let Some(a) = g.hosts.online.get_mut(name) else {
        return;
    };
    if a.sending {
        return;
    }
    if busy {
        return no("a match runs on this host");
    }
    if a.sends >= MAX_SENDS || b.size > MAX_UPDATE_BYTES {
        return no("no more transfers on this connection");
    }
    a.sending = true;
    a.sends += 1;
    let conn = a.conn;
    let (sh, tx, name) = (Arc::clone(shared), tx.clone(), name.to_string());
    let _ = std::thread::Builder::new()
        .name(format!("host-{name}-update"))
        .spawn(move || {
            let sent = stream_build(&b, &tx);
            let mut g = lock(&sh);
            match sent {
                Ok(n) => g.note(&format!("host {name}: build {} sent ({n} bytes)", &b.commit[..8])),
                Err(e) => {
                    let _ = tx.send(HostOut::Line(format!("updateno {} {e}", b.commit)));
                    g.note(&format!("host {name}: build {} not sent: {e}", &b.commit[..8]));
                }
            }
            if let Some(a) = g.hosts.online.get_mut(&name).filter(|a| a.conn == conn) {
                a.sending = false;
            }
        });
}

/// HA-2: the pieces of build `b` to `tx`, paced; the number of bytes sent, or why it stopped.
fn stream_build(b: &crate::deploy::HostBuild, tx: &Sender<HostOut>) -> Result<u64, String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut f = std::fs::File::open(&b.path).map_err(|e| format!("{}: {e}", b.path.display()))?;
    let queued = Arc::new(AtomicUsize::new(0));
    let mut hash = dc_rom::Sha256::new();
    let mut buf = vec![0u8; UPDATE_CHUNK];
    let mut total = 0u64;
    loop {
        let mut n = 0;
        while n < buf.len() {
            match f.read(&mut buf[n..]) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(format!("{}: {e}", b.path.display())),
            }
        }
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > b.size {
            return Err("the build changed on disk; a new deploy writes it again".into());
        }
        hash.update(&buf[..n]);
        let line = format!("updpart {} {}", b.commit, crate::identity::b64std(&buf[..n]));
        let since = Instant::now();
        while queued.load(Ordering::Acquire) > UPDATE_QUEUE {
            if since.elapsed() > UPDATE_STALL {
                return Err("the connection took nothing for a minute".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        queued.fetch_add(line.len(), Ordering::AcqRel);
        tx.send(HostOut::Paced(line, Arc::clone(&queued)))
            .map_err(|_| "the connection ended".to_string())?;
    }
    let got: String = hash.finalize().iter().map(|x| format!("{x:02x}")).collect();
    if total != b.size || got != b.sha256 {
        return Err("the build changed on disk; a new deploy writes it again".into());
    }
    tx.send(HostOut::Line(format!("updend {}", b.commit)))
        .map_err(|_| "the connection ended".to_string())?;
    Ok(total)
}

/// A remote server ended (`exited`, `failed`): as a local child's end of stdout (crash unless the match already
/// ended), then the record goes.
fn gone(shared: &Shared, g: &mut Core, gid: u64, why: &str) {
    if let Some(r) = g.games.get_mut(&gid).and_then(|x| x.link.remote_mut()) {
        r.alive = false;
        r.stdin_open = false;
    }
    let r = g.scope(|st| {
        crate::game::close_game(st, gid, true);
        Ok(())
    });
    report(g, r);
    run_deferred(shared, g);
    g.games.remove(&gid);
    g.note(&format!("game {gid}: server exited ({why})"));
}

/// After the quit grace period: a finished remote game whose agent is offline is forgotten (nobody will report
/// its exit); a remote game of an agent online waits for its `exited`.
pub fn reap_orphan(st: &mut Core, gid: u64) {
    let orphan = st
        .games
        .get(&gid)
        .filter(|g| g.over)
        .and_then(|g| match &g.link {
            Link::Remote(r) if r.tx.is_none() => Some(r.agent.clone()),
            _ => None,
        });
    if let Some(agent) = orphan {
        st.games.remove(&gid);
        st.note(&format!(
            "game {gid}: host {agent} offline, its finished game forgotten"
        ));
    }
}

/// The host port's gate: TLS from every peer with the lobby's own certificate (no plain mode).
pub fn gate(identity: Arc<crate::tls::Identity>) -> crate::tls::Gate {
    crate::tls::Gate {
        identity: Some(identity),
        tls_everywhere: true,
        tls_for_remote: true,
    }
}

/// The host port: each connection's TLS handshake, then [`serve`] on its own thread.
pub fn spawn_listener(shared: Shared, l: TcpListener, gate: Arc<crate::tls::Gate>) {
    let slots = crate::tls::Slots::default();
    let _ = std::thread::Builder::new()
        .name("host-accept".into())
        .spawn(move || {
            for s in l.incoming() {
                let stream = match s {
                    Ok(s) => s,
                    Err(e) => {
                        lock(&shared).note(&format!("host accept: {e}"));
                        continue;
                    }
                };
                if lock(&shared).stopping {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    continue;
                }
                let Some(slot) = slots.take(MAX_HANDSHAKES) else {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    continue;
                };
                let sh = Arc::clone(&shared);
                let gate = Arc::clone(&gate);
                let _ = std::thread::Builder::new()
                    .name("host-conn".into())
                    .spawn(move || admit_and_serve(&sh, stream, &gate, slot));
            }
        });
}

fn admit_and_serve(
    shared: &Shared,
    stream: TcpStream,
    gate: &crate::tls::Gate,
    slot: crate::tls::Slot,
) {
    let Ok(peer) = stream.peer_addr() else {
        return;
    };
    match crate::tls::admit(stream, gate, Some(slot)) {
        crate::tls::Admitted::Conn(c) => {
            let _ = c.tcp().set_nodelay(true);
            let _ = c.tcp().set_read_timeout(Some(TICK));
            match c.split() {
                Ok((r, w)) => serve(shared, r, w, peer),
                Err(e) => lock(shared).note(&format!("host connection from {peer}: {e}")),
            }
        }
        crate::tls::Admitted::PlainRefused(s) => {
            let _ = s.shutdown(std::net::Shutdown::Both);
            lock(shared).note(&format!(
                "host connection from {peer} refused: plain (the host port is TLS only, LG-300)"
            ));
        }
        crate::tls::Admitted::Failed(e) => {
            lock(shared).note(&format!("host connection from {peer}: {e}"))
        }
    }
}

/// The `hosts` list of the admin `GET /status` (only when an agent is configured): every configured agent, online
/// or not, with its connection's details.
pub fn status_json(st: &Core) -> dc_mod_lua::JsonValue {
    use dc_mod_lua::JsonValue as J;
    let s = |v: &str| J::String(v.to_string());
    J::Array(
        st.opts
            .host_agents
            .iter()
            .map(|c| {
                let mut o = BTreeMap::new();
                o.insert("name".to_string(), s(&c.name));
                let a = st.hosts.online.get(&c.name);
                o.insert("online".to_string(), J::Bool(a.is_some()));
                if let Some(a) = a {
                    o.insert("address".to_string(), s(&a.address));
                    o.insert("peer".to_string(), s(&a.peer));
                    o.insert("version".to_string(), s(&a.version));
                    o.insert(
                        "ports".to_string(),
                        a.ports
                            .map(|(lo, hi)| s(&format!("{lo}-{hi}")))
                            .unwrap_or(J::Null),
                    );
                    o.insert(
                        "online_secs".to_string(),
                        J::Number(a.since.elapsed().as_secs() as f64),
                    );
                }
                let games = st
                    .games
                    .values()
                    .filter(|g| g.link.remote().map(|r| r.agent == c.name).unwrap_or(false))
                    .count();
                o.insert("games".to_string(), J::Number(games as f64));
                J::Object(o)
            })
            .collect(),
    )
}
