//! The lobby side of a bot: one TCP connection to the auth server, then one to the lobby server, driven the way
//! the 2009 client drives them and the way the harness's scripted clients do (`lobby_harness`: `login`,
//! `lobby`, the room steps of the main suite).
//!
//! Flow (PROTOCOL_2009.md §2.1, §3.1, §3.7, §7): file check, login with the `LOCAL:<id>` token, nickname creation
//! when the account has no character, server list, server pick (session key), lobby login, item sync, character
//! info, equipment, inventory, balance, room settings, channel list and channel enter, room list, user list; then
//! create a room or enter one by number, mark present, ready (guest) or start (host), and wait for the match
//! address push 0x3043.
// SPEC: PROTOCOL_2009.md §1.2 (framing), §2, §3, §7
// HARNESS: lobby_harness::login, lobby_harness::lobby, lobby_harness::test_main_suite

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dc_wire::{
    parse_tcp_frames, FrameHeader8, PktReader, PktWriter, BALANCE, BALANCE_REPLY, CHANNEL_ENTER,
    CHANNEL_ENTER_REPLY, CHANNEL_LIST, CHANNEL_LIST_REPLY, CHANNEL_USERS, CHANNEL_USERS_REPLY,
    CHARACTER_CREATE, CHARACTER_CREATE_REPLY, CHARACTER_INFO, CHARACTER_INFO_REPLY, EQUIPMENT,
    EQUIPMENT_REPLY, FILE_CHECK, FILE_CHECK_REPLY, INVENTORY, INVENTORY_REPLY, ITEM_SYNC,
    ITEM_SYNC_REPLY, LOBBY_LOGIN, LOBBY_LOGIN_REPLY, LOBBY_ROOM_ADD_PUSH, LOCATION, LOGIN_REPLY,
    LOGIN_REQUEST, MATCH_ADDRESS_PUSH, MATCH_EXIT, NAME_CHECK, NAME_CHECK_REPLY, ROOM_CREATE,
    ROOM_CREATE_REPLY, ROOM_ENTER, ROOM_ENTER_REPLY, ROOM_INFO, ROOM_INFO_REPLY, ROOM_LEAVE,
    ROOM_LEAVE_REPLY, ROOM_LIST, ROOM_LIST_REPLY, ROOM_SETTINGS_FETCH, ROOM_SETTINGS_REPLY,
    ROOM_TEAM_PUSH, ROOM_UPDATE_PUSH, ROOM_USERS, ROOM_USERS_REPLY, ROOM_USER_ADD_PUSH,
    ROOM_USER_FLAG, ROOM_USER_FLAG_REPLY, ROOM_USER_REMOVE_PUSH, ROOM_USER_STATE_PUSH,
    SERVER_LIST_PUSH, SERVER_PICK, SERVER_PICK_REPLY,
};
use serde_json::json;

use crate::record::{head, Recorder};
use crate::{bot_err, BotError, BotResult};

/// Prefix of the login token the harness uses for local accounts (`LOCAL:<id>` in the 0x200 token field).
// HARNESS: lobby_harness::login (`wstr('LOCAL:' + user, 4096)`)
pub const LOCAL_TOKEN_PREFIX: &str = "LOCAL:";
/// 0x201 reason "no character yet": go to the nickname screen.
// SPEC: PROTOCOL_2009.md §2.2 (0x201 reason high word 3, low word 0x1e)
pub const REASON_NO_CHARACTER: u32 = 0x0003_001e;
/// The face value of 0x202 is 6000 + the model index.
// SPEC: PROTOCOL_2009.md §2.2 (face value), §3.3a (+0x56)
pub const FACE_BASE: i32 = 6000;
/// `server_entry` stride inside 0x2000 / 0x3b1.
// SPEC: PROTOCOL_2009.md §3.2 (0xc0 bytes)
pub const SERVER_ENTRY_SIZE: usize = 0xc0;
/// `channel_entry` stride inside 0x377.
// SPEC: PROTOCOL_2009.md §3.4 (0x5a stride)
pub const CHANNEL_ENTRY_SIZE: usize = 0x5a;
/// `room_record` stride inside 0x381.
// SPEC: PROTOCOL_2009.md §3.6a (0x7e bytes)
pub const ROOM_RECORD_SIZE: usize = 0x7e;
/// `room_user_record` stride inside 0x38b.
// SPEC: PROTOCOL_2009.md §3.7b (0xb8 bytes)
pub const ROOM_USER_RECORD_SIZE: usize = 0xb8;
/// The client's automatic server pick skips entries at or above this load (percent).
// SPEC: PROTOCOL_2009.md §3.2 (first connectable non-clan entry below 95 % load)
pub const PICK_MAX_LOAD_PCT: u64 = 95;

/// Room user flag kinds and values of 0x392.
// SPEC: PROTOCOL_2009.md §3.7 (0x392: (0, 0/1) ready off / on, (0, 2) start, (1, 0) present)
pub mod flag {
    pub const READY: (u32, u32) = (0, 1);
    pub const START: (u32, u32) = (0, 2);
    pub const PRESENT: (u32, u32) = (1, 0);
}

fn u32_at(d: &[u8], off: usize) -> u32 {
    PktReader::from_raw(d).at(off).u32()
}

fn i32_at(d: &[u8], off: usize) -> i32 {
    PktReader::from_raw(d).at(off).i32()
}

fn u16_at(d: &[u8], off: usize) -> u16 {
    PktReader::from_raw(d).at(off).u16()
}

fn wstr_at(d: &[u8], off: usize, n: usize) -> String {
    PktReader::from_raw(d).at(off).wstr(n)
}

// ------------------------------------------------------------------------------------------------- TCP link

/// One framed TCP connection. Frames are dispatched on the frame id (PROTOCOL_2009.md §1.2: the client dispatches
/// on the frame id). Frames that arrive while waiting for another id are kept, in order, for later calls.
pub struct TcpLink {
    stream: crate::tls::ClientStream,
    buf: Vec<u8>,
    pending: VecDeque<(u16, Vec<u8>)>,
    conn: u32,
    port: &'static str,
    rec: Recorder,
    closed: bool,
    /// Every frame id received, in order (for the run report).
    pub received: Vec<u16>,
}

impl TcpLink {
    /// Connects to `addr`; `port` names the connection in the capture (`auth` or `lobby`).
    pub fn connect(
        addr: SocketAddr,
        port: &'static str,
        rec: &Recorder,
        timeout: Duration,
    ) -> BotResult<TcpLink> {
        let stream = TcpStream::connect_timeout(&addr, timeout)
            .map_err(|e| bot_err!("{port}: connect {addr}: {e}"))?;
        Ok(Self::over(
            crate::tls::ClientStream::Plain(stream),
            addr,
            port,
            rec,
        ))
    }

    /// Connects to `addr` with TLS when the server speaks it (the auth port, CLI-1470..1475): the server is trusted
    /// by the process-wide [`crate::tls::Trust`]; a loopback server without TLS gets a plain connection.
    pub fn connect_auth(
        addr: SocketAddr,
        port: &'static str,
        rec: &Recorder,
        timeout: Duration,
    ) -> BotResult<TcpLink> {
        Self::connect_tls(addr, port, rec, timeout, &crate::tls::current())
    }

    /// [`TcpLink::connect_auth`] with an explicit trust.
    pub fn connect_tls(
        addr: SocketAddr,
        port: &'static str,
        rec: &Recorder,
        timeout: Duration,
        trust: &crate::tls::Trust,
    ) -> BotResult<TcpLink> {
        let stream = crate::tls::connect_with(addr, timeout, trust).map_err(crate::BotError)?;
        Ok(Self::over(stream, addr, port, rec))
    }

    /// True when the connection is encrypted.
    pub fn is_tls(&self) -> bool {
        self.stream.is_tls()
    }

    fn over(
        stream: crate::tls::ClientStream,
        addr: SocketAddr,
        port: &'static str,
        rec: &Recorder,
    ) -> TcpLink {
        let _ = stream.tcp().set_nodelay(true);
        let conn = rec.open_conn(port, "tcp", &addr.to_string());
        rec.transport_event(
            "tcp_open",
            json!({"port": port, "conn": conn, "addr": addr.to_string(), "tls": stream.is_tls()}),
        );
        TcpLink {
            stream,
            buf: Vec::new(),
            pending: VecDeque::new(),
            conn,
            port,
            rec: rec.clone(),
            closed: false,
            received: Vec::new(),
        }
    }

    /// Sends one frame built by `w`.
    pub fn send(&mut self, w: &PktWriter) -> BotResult<()> {
        let frame = w.to_frame();
        self.rec.tcp_frame(self.conn, self.port, "c2s", &frame);
        self.rec.transport_event(
            "tcp_out",
            json!({"port": self.port, "id": format!("0x{:x}", w.id()), "len": frame.len() - 8,
                   "head": head(&frame[8..])}),
        );
        self.stream
            .write_all(&frame)
            .map_err(|e| bot_err!("{}: send 0x{:x}: {e}", self.port, w.id()))
    }

    /// Sends the message built by `w` in an outer frame of id `frame` (the inner-id override frames: 0x368 /
    /// 0x36a travel in frame 0x324).
    // SPEC: PROTOCOL_2009.md §1.2 "Inner-id overrides" (EVIDENCE: cshell+0x18f7b0 / 0x18f890 open frame 0x324)
    pub fn send_in_frame(&mut self, frame: u16, w: &PktWriter) -> BotResult<()> {
        let p = w.to_payload();
        let mut out = FrameHeader8 {
            id: frame as i32,
            size: p.len() as i32,
        }
        .to_bytes()
        .to_vec();
        out.extend_from_slice(&p);
        self.rec.tcp_frame(self.conn, self.port, "c2s", &out);
        self.rec.transport_event(
            "tcp_out",
            json!({"port": self.port, "id": format!("0x{:x}", w.id()), "frame": format!("0x{:x}", frame),
                   "len": p.len(), "head": head(&p)}),
        );
        self.stream
            .write_all(&out)
            .map_err(|e| bot_err!("{}: send 0x{:x}: {e}", self.port, w.id()))
    }

    /// Sends a header-only request.
    pub fn send_empty(&mut self, id: u16) -> BotResult<()> {
        self.send(&PktWriter::new(id))
    }

    /// Reads whatever arrives within `wait` and cuts complete frames. Returns false once the peer closed.
    pub fn pump(&mut self, wait: Duration) -> BotResult<bool> {
        if self.closed {
            return Ok(false);
        }
        let _ = self
            .stream
            .tcp()
            .set_read_timeout(Some(wait.max(Duration::from_millis(1))));
        let mut tmp = [0u8; 65536];
        match self.stream.read(&mut tmp) {
            Ok(0) => {
                self.closed = true;
                self.rec.close_conn(self.conn, self.port, "tcp", "s2c");
                self.rec.event("tcp_closed", json!({"port": self.port}));
                return Ok(false);
            }
            Ok(n) => self.buf.extend_from_slice(&tmp[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(true)
            }
            Err(e) => {
                self.closed = true;
                return Err(bot_err!("{}: receive: {e}", self.port));
            }
        }
        // SPEC: PROTOCOL_2009.md §1.2 (frame complete when len >= 8 and len >= body_len + 8; body <= 0x8000)
        let (frames, used) =
            parse_tcp_frames(&self.buf).map_err(|e| bot_err!("{}: framing: {e}", self.port))?;
        let mut off = 0usize;
        for (fid, payload) in frames {
            let flen = 8 + payload.len();
            self.rec
                .tcp_frame(self.conn, self.port, "s2c", &self.buf[off..off + flen]);
            off += flen;
            let id = fid as u16;
            self.rec.transport_event(
                "tcp_in",
                json!({"port": self.port, "id": format!("0x{id:x}"), "len": payload.len(),
                       "head": head(&payload)}),
            );
            self.received.push(id);
            self.pending.push_back((id, payload));
        }
        self.buf.drain(..used);
        Ok(true)
    }

    /// Takes the first kept frame with `id`, reading more until `timeout` passes.
    pub fn recv_until(&mut self, id: u16, timeout: Duration) -> BotResult<Vec<u8>> {
        let end = Instant::now() + timeout;
        loop {
            if let Some(i) = self.pending.iter().position(|f| f.0 == id) {
                return Ok(self.pending.remove(i).map(|f| f.1).unwrap_or_default());
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                let have: Vec<String> = self
                    .pending
                    .iter()
                    .map(|f| format!("0x{:x}", f.0))
                    .collect();
                return Err(bot_err!(
                    "{}: no 0x{id:x} within {:?}; kept {have:?}",
                    self.port,
                    timeout
                ));
            }
            if !self.pump(left.min(Duration::from_millis(100)))? {
                if self.pending.iter().any(|f| f.0 == id) {
                    continue;
                }
                return Err(bot_err!("{}: closed while waiting for 0x{id:x}", self.port));
            }
        }
    }

    /// Takes every kept frame whose id is in `ids` (in arrival order).
    pub fn take_ids(&mut self, ids: &[u16]) -> Vec<(u16, Vec<u8>)> {
        let mut out = Vec::new();
        let mut keep = VecDeque::new();
        for f in self.pending.drain(..) {
            if ids.contains(&f.0) {
                out.push(f);
            } else {
                keep.push_back(f);
            }
        }
        self.pending = keep;
        out
    }

    /// Drops every kept frame.
    pub fn clear(&mut self) {
        self.pending.clear();
    }

    /// True when a kept frame has `id`.
    pub fn has_pending(&self, id: u16) -> bool {
        self.pending.iter().any(|f| f.0 == id)
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Closes the connection (the client closes the auth socket after a good 0x20b).
    pub fn close(&mut self) {
        if !self.closed {
            self.closed = true;
            self.stream.close();
            self.rec.close_conn(self.conn, self.port, "tcp", "c2s");
        }
    }
}

// ------------------------------------------------------------------------------------------------- auth

/// What the bot logs in with.
#[derive(Debug, Clone)]
pub struct LoginOptions {
    /// BOT-98: `--server-id`: the server list entry to pick (none: `pick_server`).
    pub server_id: Option<u32>,
    pub auth: SocketAddr,
    /// Account id; sent as `LOCAL:<user>`.
    pub user: String,
    /// Nickname used when the account has no character yet (4..12 characters pass the client's filter).
    pub nick: String,
    /// Password box text (the harness sends `pw`).
    pub password: String,
    /// Adapter address text (the harness sends `00-11-22-33-44-55`).
    pub adapter: String,
    /// Character model index for a new character (sent as 6000 + n; the harness uses 2).
    pub face: i32,
    pub timeout: Duration,
    /// LG-271: the mod-set hash for the login name's suffix (`dc_wire::loginmods`); `None` = no suffix at all,
    /// which a vanilla lobby takes as the empty set and a modded lobby refuses.
    pub mod_set_hash: Option<String>,
    /// LG-271: the content hash of the suffix (`0` when `None`); sent only together with a suffix.
    pub content_hash: Option<String>,
}

impl LoginOptions {
    /// Harness-style defaults for account `user` with nickname `nick`.
    // HARNESS: lobby_harness::login (password `pw`, adapter `00-11-22-33-44-55`, face 6002)
    pub fn new(auth: SocketAddr, user: &str, nick: &str) -> Self {
        LoginOptions {
            server_id: None,
            auth,
            user: user.to_string(),
            nick: nick.to_string(),
            password: "pw".into(),
            adapter: "00-11-22-33-44-55".into(),
            face: 2,
            timeout: Duration::from_secs(10),
            mod_set_hash: None,
            content_hash: None,
        }
    }

    /// LG-271: the login-name field: `LOCAL:<user>`, then `;m=<set>;c=<content>` when a hash was given.
    pub fn login_field(&self) -> String {
        let mods = if self.mod_set_hash.is_some() || self.content_hash.is_some() {
            dc_wire::loginmods::suffix(
                self.mod_set_hash
                    .as_deref()
                    .unwrap_or(dc_wire::loginmods::EMPTY_SET_HASH),
                self.content_hash
                    .as_deref()
                    .unwrap_or(dc_wire::loginmods::NO_CONTENT),
            )
        } else {
            String::new()
        };
        // DC-DEFAULT: LG-317 (proposed) a bot always says it is one; LG-318: without a name it asks for a seat of the
        // lobby's bot pool
        format!(
            "{LOCAL_TOKEN_PREFIX}{}{}{mods}",
            self.user,
            dc_wire::loginmods::BOT_KEY
        )
    }
}

/// LG-318: how `dc-botclient --pool` ends on a refusal of its seat, for `swarm`.
pub const EXIT_NO_POOL: i32 = 5;
pub const EXIT_POOL_FULL: i32 = 3;
pub const EXIT_POOL_ADDRESS: i32 = 4;

/// The start of the error of a bot whose `--match-wait` passed with no room to join (BOT-158: a normal end).
pub const NO_ROOM_PREFIX: &str = "lobby: no room appeared within";

/// The error of a leave the lobby answered 0x387 {0} (a member held for a start countdown, LG-177, or a seat the lobby
/// no longer counts in a room).
pub const LEAVE_REFUSED: &str = "lobby: leave refused";

/// The report's `end` for an error that is a normal end of the run, not a failure (exit 0): the bot's own
/// `--match-wait` ran out with no room (BOT-158), or the lobby refused its leave from a room (BOT-169: the bot ends
/// its run and closes its connection; the lobby drops the session as on any disconnect, so the seat goes too).
// DC-DEFAULT: BOT-158, BOT-169 (proposed)
pub fn clean_end(error: &str) -> Option<String> {
    if error.starts_with(NO_ROOM_PREFIX) {
        Some(format!("ran its wait: {error}"))
    } else if error == LEAVE_REFUSED {
        Some("ran its hour: the lobby refused the leave, the connection closes".to_string())
    } else {
        None
    }
}

/// LG-318: the exit code of a bot's error text, when it is a refusal of a seat.
pub fn exit_code_of(error: &str) -> Option<i32> {
    use dc_wire::loginmods::{REASON_NO_POOL, REASON_POOL_ADDRESS, REASON_POOL_FULL};
    [
        (REASON_NO_POOL, EXIT_NO_POOL),
        (REASON_POOL_FULL, EXIT_POOL_FULL),
        (REASON_POOL_ADDRESS, EXIT_POOL_ADDRESS),
    ]
    .iter()
    .find(|(r, _)| error == format!("auth: login refused, reason 0x{r:08x}"))
    .map(|(_, c)| *c)
}

/// The result of the auth half of the login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthTicket {
    pub session_key: [u8; 16],
    pub account_no: u32,
    pub lobby: SocketAddr,
    pub server_id: u32,
    pub server_name: String,
    /// True when this login created the character (nickname screen path).
    pub created_character: bool,
}

/// One `server_entry` of 0x2000.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerEntry {
    pub id: u32,
    pub name: String,
    pub ip: String,
    pub port: u32,
    pub open: u32,
    pub capacity: u32,
    pub current: u32,
    pub clan_only: u32,
    /// The operator slots: capacity (+0x98) and in use (+0x9c).
    pub op_capacity: u32,
    pub op_current: u32,
}

/// Parses the entries of a 0x2000 / 0x3b1 body starting at `first`.
// SPEC: PROTOCOL_2009.md §3.2 (server_entry fields), §2.2 (0x2000: count at +0x12, entries at +0x16)
pub fn parse_server_entries(d: &[u8], count_off: usize, first: usize) -> Vec<ServerEntry> {
    let n = u32_at(d, count_off).min(20) as usize;
    (0..n)
        .map(|i| first + i * SERVER_ENTRY_SIZE)
        .filter(|o| o + SERVER_ENTRY_SIZE <= d.len())
        .map(|o| ServerEntry {
            id: u32_at(d, o),
            name: wstr_at(d, o + 4, 33),
            ip: wstr_at(d, o + 0x46, 33),
            port: u32_at(d, o + 0x88),
            open: u32_at(d, o + 0x8c),
            capacity: u32_at(d, o + 0x90),
            current: u32_at(d, o + 0x94),
            clan_only: u32_at(d, o + 0xbc),
            // EVIDENCE: cshell+0x139d70 (an operator may enter while +0x9c is below +0x98)
            op_capacity: u32_at(d, o + 0x98),
            op_current: u32_at(d, o + 0x9c),
        })
        .collect()
}

/// The client's automatic pick: the first non-clan entry below 95 % load, else the first entry.
// SPEC: PROTOCOL_2009.md §3.2 (automatic pick cs+0x13bf70)
pub fn pick_server(entries: &[ServerEntry]) -> Option<&ServerEntry> {
    let below = |e: &ServerEntry| {
        e.capacity == 0 || (e.current as u64 * 100) / (e.capacity as u64) < PICK_MAX_LOAD_PCT
    };
    entries
        .iter()
        .find(|e| e.clan_only == 0 && below(e))
        .or_else(|| entries.first())
}

/// Auth server half: file check, login (with nickname creation when needed), server list, server pick.
// SPEC: PROTOCOL_2009.md §2.1 (flow), §2.2 (layouts)
// HARNESS: lobby_harness::login
pub fn auth_login(o: &LoginOptions, rec: &Recorder) -> BotResult<AuthTicket> {
    let mut a = TcpLink::connect_auth(o.auth, "auth", rec, o.timeout)?;
    let t = o.timeout;
    // SPEC: PROTOCOL_2009.md §2.2 (0x206: u32[8]; the harness sends 32 zero bytes)
    let mut w = PktWriter::new(FILE_CHECK);
    w.pad(32);
    a.send(&w)?;
    let d = a.recv_until(FILE_CHECK_REPLY, t)?;
    if u32_at(&d, 6) == 0 {
        return Err(bot_err!("auth: file check refused (0x207 ok = 0)"));
    }
    // SPEC: PROTOCOL_2009.md §2.2 (0x200: wstr[4096] token, wstr[17] typed text, wstr[20] adapter, i32 partner)
    let mut w = PktWriter::new(LOGIN_REQUEST);
    w.wstr(&o.login_field(), 4096)
        .wstr(&o.password, 17)
        .wstr(&o.adapter, 20)
        .i32(0);
    a.send(&w)?;
    let d = a.recv_until(LOGIN_REPLY, t)?;
    let mut created = false;
    let account_no = if u32_at(&d, 6) != 0 {
        u32_at(&d, 0x12)
    } else {
        let reason = u32_at(&d, 0xa);
        if reason != REASON_NO_CHARACTER {
            return Err(bot_err!("auth: login refused, reason 0x{reason:08x}"));
        }
        // SPEC: PROTOCOL_2009.md §2.2 (0x204 name check, 0x202 create: i32 face, wstr[17] name)
        let mut w = PktWriter::new(NAME_CHECK);
        w.wstr(&o.nick, 17);
        a.send(&w)?;
        let d = a.recv_until(NAME_CHECK_REPLY, t)?;
        if u32_at(&d, 6) == 0 {
            return Err(bot_err!("auth: nickname {:?} not available", o.nick));
        }
        let mut w = PktWriter::new(CHARACTER_CREATE);
        w.i32(FACE_BASE + o.face).wstr(&o.nick, 17);
        a.send(&w)?;
        let d = a.recv_until(CHARACTER_CREATE_REPLY, t)?;
        if u32_at(&d, 6) == 0 {
            return Err(bot_err!(
                "auth: character create refused, code 0x{:x}",
                u32_at(&d, 0xa)
            ));
        }
        created = true;
        u32_at(&d, 0xe)
    };
    let d = a.recv_until(SERVER_LIST_PUSH, t)?;
    // SPEC: PROTOCOL_2009.md §2.2 (0x2000: +6 result 6 = list, +0x12 count, +0x16 entries)
    let entries = parse_server_entries(&d, 0x12, 0x16);
    let picked = match o.server_id {
        Some(id) => entries.iter().find(|e| e.id == id),
        None => pick_server(&entries),
    };
    let pick = picked.cloned().ok_or_else(|| {
        bot_err!(
            "auth: no server {:?} in a list of {} (result {})",
            o.server_id,
            entries.len(),
            u32_at(&d, 6)
        )
    })?;
    rec.event(
        "server_list",
        json!({"entries": entries.len(), "picked": pick.id, "ip": pick.ip, "port": pick.port}),
    );
    // DC-DEFAULT: BOT-101 a featured entry refuses a pick without the claim of its set (LG-313). The lobby's push
    // came before 0x2000; a bot holds no mod content, so it repeats the published hashes of the entry it picks
    let claim = a
        .take_ids(&[dc_wire::entrymods::PUSH])
        .last()
        .map(|(_, d)| dc_wire::entrymods::parse_push(d))
        .unwrap_or_default()
        .into_iter()
        .find(|e| e.entry == pick.id);
    if let Some(c) = claim {
        rec.event("entry_claim", json!({"entry": c.entry, "set": c.set}));
        a.send(&dc_wire::entrymods::claim_message(&c))?;
    }
    let mut w = PktWriter::new(SERVER_PICK);
    w.u32(pick.id);
    a.send(&w)?;
    let d = a.recv_until(SERVER_PICK_REPLY, t)?;
    if u32_at(&d, 6) == 0 {
        return Err(bot_err!(
            "auth: server pick refused, reason {}",
            u32_at(&d, 0xa)
        ));
    }
    let mut session_key = [0u8; 16];
    let k = PktReader::from_raw(&d).at(0xe).bytes_padded(16);
    session_key.copy_from_slice(&k);
    // SPEC: PROTOCOL_2009.md §2.2 (on ok: auth socket closed, lobby socket opened to entry +0x46 / +0x88)
    a.close();
    let ip = pick
        .ip
        .parse::<Ipv4Addr>()
        .map(IpAddr::V4)
        .unwrap_or(o.auth.ip());
    Ok(AuthTicket {
        session_key,
        account_no,
        lobby: SocketAddr::new(ip, pick.port as u16),
        server_id: pick.id,
        server_name: pick.name,
        created_character: created,
    })
}

// ------------------------------------------------------------------------------------------------- lobby

/// What the lobby told the bot about its character and session (the 2009 client keeps the same values in its
/// shell state and later copies several of them into the game server's player-info answer 0x72).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LobbyProfile {
    /// 0x301 +0x1e, the lobby session index (CD-20).
    pub lobby_index: u32,
    /// 0x303 +0xe.
    pub char_no: u32,
    /// 0x303 +0x34.
    pub nick: String,
    pub face: i32,
    pub gp: u32,
    pub exp: u32,
    pub clan_id: u32,
    pub nation: u32,
    /// 0x323 +0x12: item number per equipment slot (-1 empty).
    pub equipped: Vec<i32>,
    pub inventory_count: i32,
    pub chan_id: u32,
    pub chan_label: String,
    /// 0x373 +0xe: own in-channel id.
    pub my_id: u32,
}

/// One member of the bot's room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomMember {
    pub char_no: i32,
    pub name: String,
    pub team: i32,
    pub state: i32,
    /// +0x42: the member's experience (the room screen's rank icon is the icon of its level).
    pub exp: u32,
}

/// The room the bot sits in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoomSeat {
    pub number: u32,
    pub slot: u32,
    pub host: bool,
    pub host_id: u32,
    pub map: i32,
    pub mode: i32,
    pub members: BTreeMap<u32, RoomMember>,
    /// Room state of the 0x3035 type 7 pushes (0 waiting, 1 starting, 2 playing; 0 until one arrives).
    pub phase: u32,
    /// The room's weapon limit (0x389 +0x84, then the 0x3035 type 0xe pushes; `dc_bute::arms` values).
    pub weapon_limit: u32,
}

impl RoomSeat {
    /// The own team (1 or 2) from the member list, 1 when unknown.
    pub fn team_of(&self, my_id: u32) -> i32 {
        self.members
            .get(&my_id)
            .map(|m| m.team)
            .filter(|t| *t == 1 || *t == 2)
            .unwrap_or(1)
    }
}

/// The game server address from 0x3043.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchAddress {
    pub ip: String,
    pub port: u32,
    pub records: u32,
    /// LG-327: the seat ticket that came with it (`None`: a lobby without a ticket for this match).
    pub ticket: Option<dc_wire::seatticket::Ticket>,
}

impl MatchAddress {
    /// The UDP address to join (the text form the client builds is `<ip>:<port>`).
    // SPEC: UDP_SESSION_2009_A.md §2.2 step 1 (`<ip>:<port>`), §2.4 (dotted quad parsed)
    pub fn socket_addr(&self) -> BotResult<SocketAddr> {
        let ip: Ipv4Addr = self
            .ip
            .trim()
            .parse()
            .map_err(|_| bot_err!("match address {:?} is not a dotted quad", self.ip))?;
        Ok(SocketAddr::new(IpAddr::V4(ip), self.port as u16))
    }
}

/// What a batch of room pushes means for the ready state.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct PushEffects {
    /// 0x3042 for the own account with state 0 (not ready).
    own_unready: bool,
    /// 0x3035 with a room setting type (map, mode, limits, title, ...).
    settings: bool,
    /// 0x3042 state 3 (in game) for the host.
    host_in_game: bool,
}

/// 0x3035 types that are settings changes (not host hand-over 2, player count 5, room state 7, badge 0xf).
// SPEC: PROTOCOL_2009.md §3.7 (0x3035 types 1..0xf)
fn is_setting_type(t: u32) -> bool {
    matches!(t, 1 | 3 | 4 | 6 | 8 | 9 | 0xa | 0xb | 0xc | 0xd | 0xe)
}

/// 0x3035 type of the room state (value 0 waiting, 1 starting, 2 playing).
// SPEC: PROTOCOL_2009.md §3.7 (0x3035 type 7 room state)
const ROOM_STATE_FIELD: u32 = 7;
/// 0x3035 type of the weapon limit.
// SPEC: PROTOCOL_2009.md §3.7 (0x3035 type 0xe weapon limit)
const WEAPON_LIMIT_FIELD: u32 = 0xe;
/// Room state value of a playing room.
const ROOM_PLAYING: u32 = 2;

/// Delay before a re-ready is sent.
const RE_READY_DELAY: Duration = Duration::from_millis(300);
/// At most one re-ready per this interval.
const RE_READY_MIN_GAP: Duration = Duration::from_secs(1);
/// A running host match without 0x3043 for this long is reported.
const MISSED_START_AFTER: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchAction {
    None,
    SendReady,
    MissedStart,
}

/// Sans-IO guest watcher: when to send ready again and when to report a missed start.
#[derive(Debug, Default)]
struct ReadyWatch {
    due: Option<Instant>,
    last_request: Option<Instant>,
    host_started: Option<Instant>,
    missed_logged: bool,
}

impl ReadyWatch {
    fn on_effects(&mut self, fx: &PushEffects, now: Instant) {
        if (fx.own_unready || fx.settings) && self.due.is_none() {
            self.due = Some(now + RE_READY_DELAY);
        }
        if fx.host_in_game && self.host_started.is_none() {
            self.host_started = Some(now);
        }
    }

    /// `own_state` is the bot's last known 0x3042 state.
    fn poll(&mut self, now: Instant, own_state: i32) -> WatchAction {
        if let Some(due) = self.due {
            let gap_ok = self
                .last_request
                .is_none_or(|l| now.duration_since(l) >= RE_READY_MIN_GAP);
            if now >= due && gap_ok {
                self.due = None;
                self.last_request = Some(now);
                // starting (2) or in game (3): ready no longer means anything here
                if own_state != 2 && own_state != 3 {
                    return WatchAction::SendReady;
                }
            }
        }
        if let Some(t) = self.host_started {
            if !self.missed_logged && now.duration_since(t) >= MISSED_START_AFTER {
                self.missed_logged = true;
                return WatchAction::MissedStart;
            }
        }
        WatchAction::None
    }
}

/// A logged-in lobby connection.
pub struct Lobby {
    pub link: TcpLink,
    pub ticket: AuthTicket,
    pub profile: LobbyProfile,
    pub room: Option<RoomSeat>,
    timeout: Duration,
    rec: Recorder,
    /// BOT-84: a filler bot's prefix (every member whose name starts with it is a bot); `None` = not a filler.
    filler: Option<String>,
    room_title: Option<String>,
    /// BOT-84: rooms a filler left for want of a player or was refused, with the end of the wait; `find_room` passes
    /// them over till then.
    skipped: BTreeMap<u32, Instant>,
    /// BOT-156: the weapon limits the lobby refused this bot's ready under; the bot readies under none of them again.
    refused_limits: BTreeSet<u32>,
    /// LG-319: the lobby took this bot out of its room, and it has not sat down since.
    removed: bool,
    /// LG-327: the seat ticket the menu's connection read before it handed this lobby over.
    pub seat_ticket: Option<dc_wire::seatticket::Ticket>,
}

/// How long a filler bot passes over a room it left without a player or was refused (BOT-84).
pub const SKIPPED_ROOM_COOLDOWN: Duration = Duration::from_secs(20);
/// LG-319: how long a bot passes over a room that held a person and needed its seat.
pub const YIELDED_ROOM_COOLDOWN: Duration = Duration::from_secs(60);
/// The error text of a bot's wait after the lobby took it out of its room, for a bot that does not look for another.
pub const REMOVED_FROM_ROOM: &str = "lobby: removed from the room";

/// The error text a filler bot's waits end with when its room holds no player (BOT-84).
pub const FILLER_ALONE: &str = "filler: no player in the room";

/// BOT-156: the error text of a ready the room's weapon limit refuses (or would refuse: a limit refused before).
pub const WEAPON_LIMIT_REFUSED: &str = "lobby: the equipped weapons fail the room's weapon limit";
/// BOT-156: how long a bot passes over a room whose weapon limit its loadout fails.
pub const WEAPON_LIMIT_ROOM_COOLDOWN: Duration = Duration::from_secs(300);

/// BOT-156: whether a 0x393 reply refuses the flag for the room's weapon limit (LG-342's reason).
// SPEC: PROTOCOL_2009.md §3.7 (0x393: +6 u32 ok, +0xa u16 reason)
fn weapon_limit_refusal(d: &[u8]) -> bool {
    u32_at(d, 6) == 0 && u16_at(d, 0xa) == dc_wire::flagrefusal::REASON_WEAPON_LIMIT
}

/// BOT-84: whether `members` hold a player: a member other than `my_id` whose name does not start with `prefix`
/// (ASCII case-insensitive; the bots of one launcher share a prefix, e.g. `VpsBot1`, `VpsBot2`).
/// BOT-102: `=<nick>` asks for that one player (`swarm --follow`).
pub fn room_has_player(members: &BTreeMap<u32, RoomMember>, my_id: u32, prefix: &str) -> bool {
    if let Some(nick) = prefix.strip_prefix('=') {
        return members
            .iter()
            .any(|(id, m)| *id != my_id && m.name.eq_ignore_ascii_case(nick));
    }
    let p = prefix.to_ascii_lowercase();
    members.iter().any(|(id, m)| {
        *id != my_id && (p.is_empty() || !m.name.to_ascii_lowercase().starts_with(&p))
    })
}

/// Parses a `room_user_record` (0xb8 bytes) at `o`.
// SPEC: PROTOCOL_2009.md §3.7b
fn room_member(d: &[u8], o: usize) -> (u32, RoomMember) {
    (
        u32_at(d, o + 4),
        RoomMember {
            char_no: i32_at(d, o),
            name: wstr_at(d, o + 0x10, 17),
            team: i32_at(d, o + 0x36),
            state: i32_at(d, o + 0x3a),
            exp: u32_at(d, o + 0x42),
        },
    )
}

impl Lobby {
    /// Lobby login and the client's start-up requests up to the lobby screen (channel entered).
    // SPEC: PROTOCOL_2009.md §3.1 (flow), §3.3, §3.4, §3.5, §3.6
    // HARNESS: lobby_harness::lobby
    pub fn login(
        ticket: &AuthTicket,
        channel: Option<u32>,
        rec: &Recorder,
        timeout: Duration,
    ) -> BotResult<Lobby> {
        let mut link = TcpLink::connect(ticket.lobby, "lobby", rec, timeout)?;
        let t = timeout;
        let mut p = LobbyProfile::default();
        // SPEC: PROTOCOL_2009.md §3.3 (0x300: u8[16] key, u32 account number)
        let mut w = PktWriter::new(LOBBY_LOGIN);
        w.raw(&ticket.session_key).u32(ticket.account_no);
        link.send(&w)?;
        let d = link.recv_until(LOBBY_LOGIN_REPLY, t)?;
        if u32_at(&d, 6) == 0 {
            return Err(bot_err!("lobby: login refused (0x301 ok = 0)"));
        }
        p.lobby_index = u32_at(&d, 0x1e);
        // SPEC: PROTOCOL_2009.md §3.3 (0x342 again while 0x343 +0xe > 0, then 0x302)
        for _ in 0..64 {
            link.send_empty(ITEM_SYNC)?;
            let d = link.recv_until(ITEM_SYNC_REPLY, t)?;
            if i32_at(&d, 0xe) <= 0 {
                break;
            }
        }
        link.send_empty(CHARACTER_INFO)?;
        let d = link.recv_until(CHARACTER_INFO_REPLY, t)?;
        // SPEC: PROTOCOL_2009.md §3.3a (0x303 fields)
        p.char_no = u32_at(&d, 0xe);
        p.nick = wstr_at(&d, 0x34, 17);
        p.face = i32_at(&d, 0x56);
        p.gp = u32_at(&d, 0x5a);
        p.exp = u32_at(&d, 0x5e);
        p.clan_id = u32_at(&d, 0x86);
        p.nation = u32_at(&d, 0x100);
        link.send_empty(EQUIPMENT)?;
        let d = link.recv_until(EQUIPMENT_REPLY, t)?;
        // SPEC: PROTOCOL_2009.md §3.3 (0x323 +0x12 i32[15] item number per slot, -1 = empty)
        p.equipped = (0..15).map(|i| i32_at(&d, 0x12 + 4 * i)).collect();
        link.send_empty(INVENTORY)?;
        let d = link.recv_until(INVENTORY_REPLY, t)?;
        p.inventory_count = i32_at(&d, 0xe);
        link.send_empty(BALANCE)?;
        link.recv_until(BALANCE_REPLY, t)?;
        // SPEC: PROTOCOL_2009.md §3.5 (0x310 with the held key / version; nothing held -> 0, 0)
        let mut w = PktWriter::new(ROOM_SETTINGS_FETCH);
        w.u32(0).u32(0);
        link.send(&w)?;
        link.recv_until(ROOM_SETTINGS_REPLY, t)?;
        link.send_empty(CHANNEL_LIST)?;
        let d = link.recv_until(CHANNEL_LIST_REPLY, t)?;
        // SPEC: PROTOCOL_2009.md §3.4 (0x377: +0xe count, +0x12 entries of 0x5a: id, wstr[33] name, open)
        let n = i32_at(&d, 0xe).clamp(0, 64) as usize;
        let chans: Vec<(u32, String, u32)> = (0..n)
            .map(|i| 0x12 + i * CHANNEL_ENTRY_SIZE)
            .filter(|o| o + CHANNEL_ENTRY_SIZE <= d.len())
            .map(|o| (u32_at(&d, o), wstr_at(&d, o + 4, 33), u32_at(&d, o + 0x46)))
            .collect();
        let pick = match channel {
            Some(c) => chans.iter().find(|x| x.0 == c).cloned(),
            None => chans.iter().find(|x| x.2 != 0).or(chans.first()).cloned(),
        }
        .ok_or_else(|| bot_err!("lobby: no channel to enter ({} listed)", chans.len()))?;
        p.chan_id = pick.0;
        p.chan_label = pick.1.clone();
        // SPEC: PROTOCOL_2009.md §3.4 (0x372: u32 account number, u32 channel id, wstr[32] channel name)
        let mut w = PktWriter::new(CHANNEL_ENTER);
        w.u32(ticket.account_no).u32(pick.0).wstr(&pick.1, 32);
        link.send(&w)?;
        let d = link.recv_until(CHANNEL_ENTER_REPLY, t)?;
        if u32_at(&d, 6) == 0 {
            return Err(bot_err!("lobby: channel {} refused (full?)", pick.0));
        }
        p.my_id = u32_at(&d, 0xe);
        // SPEC: PROTOCOL_2009.md §3.1 (0x380 -> 0x381, then the client sends 0x370; 0x306(0) has no reply)
        link.send_empty(ROOM_LIST)?;
        link.recv_until(ROOM_LIST_REPLY, t)?;
        let mut w = PktWriter::new(LOCATION);
        w.u32(0);
        link.send(&w)?;
        link.send_empty(CHANNEL_USERS)?;
        link.recv_until(CHANNEL_USERS_REPLY, t)?;
        rec.event(
            "lobby_ready",
            json!({"char_no": p.char_no, "nick": p.nick, "lobby_index": p.lobby_index,
                   "channel": p.chan_id, "my_id": p.my_id, "exp": p.exp, "gp": p.gp}),
        );
        Ok(Lobby {
            link,
            ticket: ticket.clone(),
            profile: p,
            room: None,
            timeout,
            rec: rec.clone(),
            filler: None,
            room_title: None,
            skipped: BTreeMap::new(),
            refused_limits: BTreeSet::new(),
            removed: false,
            seat_ticket: None,
        })
    }

    /// BOT-84: notes that room `number` held no player; `find_room` passes it over for [`SKIPPED_ROOM_COOLDOWN`].
    pub fn skip_room(&mut self, number: u32) {
        self.skip_room_for(number, SKIPPED_ROOM_COOLDOWN);
    }

    /// Passes over a room for `wait`; a longer wait already running stays.
    fn skip_room_for(&mut self, number: u32, wait: Duration) {
        let now = Instant::now();
        self.skipped.retain(|_, t| *t > now);
        let end = self.skipped.entry(number).or_insert(now);
        *end = (*end).max(now + wait);
    }

    fn is_skipped(&self, number: u32) -> bool {
        self.skipped.get(&number).is_some_and(|t| *t > Instant::now())
    }

    /// LG-319: reads the pushes kept so far and says whether the lobby took this bot out of its room.
    pub fn removed_from_room(&mut self) -> bool {
        self.absorb_room_pushes();
        self.removed
    }

    /// The error of a wait that cannot go on: no player left for a filler, or the lobby took the seat.
    fn seat_gone(&self) -> Option<BotError> {
        if self.filler_alone() {
            Some(bot_err!("{FILLER_ALONE}"))
        } else if self.removed {
            Some(bot_err!("{REMOVED_FROM_ROOM}"))
        } else {
            None
        }
    }

    /// BOT-84: makes this bot a filler with `prefix` (or none).
    /// BOT-102: `find_room` takes only rooms whose title holds this text (ASCII case-insensitive).
    pub fn set_room_title(&mut self, part: Option<String>) {
        self.room_title = part.map(|p| p.to_ascii_lowercase()).filter(|p| !p.is_empty());
    }

    fn title_fits(&self, title: &str) -> bool {
        self.room_title
            .as_ref()
            .is_none_or(|p| title.to_ascii_lowercase().contains(p))
    }

    pub fn set_filler(&mut self, prefix: Option<String>) {
        self.filler = prefix.filter(|p| !p.is_empty());
    }

    /// BOT-84: a filler bot that sits in a room without a player (the host left, only bots remain).
    pub fn filler_alone(&self) -> bool {
        if self.removed && self.filler.is_some() {
            return true;
        }
        match (&self.filler, &self.room) {
            (Some(p), Some(r)) => !room_has_player(&r.members, self.profile.my_id, p),
            _ => false,
        }
    }

    /// Leaves the room (0x386 / 0x387) and forgets the seat; the bot is back in the lobby's room list.
    // SPEC: PROTOCOL_2009.md §3.7 (0x386 header only, 0x387 +6 u32 ok -> room = -1, lobby state 15)
    pub fn leave_room(&mut self) -> BotResult<()> {
        self.leave_room_why("no player in the room")
    }

    fn leave_room_why(&mut self, why: &str) -> BotResult<()> {
        // LG-319: the lobby took this bot out already
        if self.room.is_none() {
            return Ok(());
        }
        let number = self.room.as_ref().map_or(0, |r| r.number);
        self.link.send_empty(ROOM_LEAVE)?;
        let d = self.link.recv_until(ROOM_LEAVE_REPLY, self.timeout)?;
        let ok = u32_at(&d, 6) != 0;
        self.room = None;
        self.rec
            .event("room_left", json!({"room": number, "ok": ok, "why": why}));
        if !ok {
            return Err(bot_err!("{LEAVE_REFUSED}"));
        }
        Ok(())
    }

    /// Creates a room (the bot becomes its host).
    // SPEC: PROTOCOL_2009.md §3.7 (0x382: i32 max players, wstr[33] title, wstr[9] password, i32 mode; 0x383)
    pub fn open_room(
        &mut self,
        max_players: i32,
        title: &str,
        password: &str,
        mode: i32,
    ) -> BotResult<u32> {
        let mut w = PktWriter::new(ROOM_CREATE);
        w.i32(max_players)
            .wstr(title, 33)
            .wstr(password, 9)
            .i32(mode);
        self.link.send(&w)?;
        let d = self.link.recv_until(ROOM_CREATE_REPLY, self.timeout)?;
        if u32_at(&d, 6) == 0 {
            return Err(bot_err!(
                "lobby: room create refused, code 0x{:x}",
                u16_at(&d, 0xa)
            ));
        }
        let seat = RoomSeat {
            number: u32_at(&d, 0xe),
            slot: u32_at(&d, 0x12),
            host: true,
            host_id: self.profile.my_id,
            ..RoomSeat::default()
        };
        let no = seat.number;
        self.room = Some(seat);
        self.after_enter()?;
        Ok(no)
    }

    /// Waits until the room list shows a room (0x381 or 0x3031) and returns the first room number.
    // SPEC: PROTOCOL_2009.md §3.6 (0x381: +0xe count, +0x12 room records; 0x3031 one record at +6)
    pub fn find_room(&mut self, wait: Duration) -> BotResult<u32> {
        let end = Instant::now() + wait;
        loop {
            if let Some(n) = self
                .link
                .take_ids(&[LOBBY_ROOM_ADD_PUSH])
                .into_iter()
                // SPEC: PROTOCOL_2009.md §3.6 table 3.6a (the record: u32 number, wstr[33] title)
                .map(|(_, d)| (u32_at(&d, 6), wstr_at(&d, 10, 33)))
                .find(|(n, t)| !self.is_skipped(*n) && self.title_fits(t))
                .map(|(n, _)| n)
            {
                return Ok(n);
            }
            self.link.send_empty(ROOM_LIST)?;
            let d = self.link.recv_until(ROOM_LIST_REPLY, self.timeout)?;
            // the first listed room that is not a room this bot just left for want of a player (BOT-84)
            let count = u32_at(&d, 0xe) as usize;
            let found = (0..count)
                .map(|i| 0x12 + i * ROOM_RECORD_SIZE)
                .take_while(|o| d.len() >= o + ROOM_RECORD_SIZE)
                .map(|o| (u32_at(&d, o), wstr_at(&d, o + 4, 33)))
                .find(|(n, t)| !self.is_skipped(*n) && self.title_fits(t))
                .map(|(n, _)| n);
            if let Some(n) = found {
                return Ok(n);
            }
            if Instant::now() >= end {
                return Err(bot_err!("{NO_ROOM_PREFIX} {wait:?}"));
            }
            self.link.pump(Duration::from_millis(400))?;
        }
    }

    /// Enters room `number`.
    // SPEC: PROTOCOL_2009.md §3.7 (0x384: i32 room number, wstr[9] password, wstr[65] extra; 0x385)
    pub fn join_room_no(&mut self, number: u32, password: &str) -> BotResult<()> {
        self.try_join_room(number, password)?.map_err(|reason| {
            bot_err!("lobby: room {number} refused, reason 0x{reason:x}")
        })
    }

    /// As [`Lobby::join_room_no`], with the lobby's refusal as a value: a bot looks for another room then (the room is
    /// full, or LG-319 keeps its last seat free).
    pub fn try_join_room(&mut self, number: u32, password: &str) -> BotResult<Result<(), u16>> {
        let mut w = PktWriter::new(ROOM_ENTER);
        w.i32(number as i32).wstr(password, 9).wstr("", 65);
        self.link.send(&w)?;
        let d = self.link.recv_until(ROOM_ENTER_REPLY, self.timeout)?;
        if u32_at(&d, 6) == 0 {
            return Ok(Err(u16_at(&d, 0xa)));
        }
        self.removed = false;
        self.room = Some(RoomSeat {
            number,
            slot: u32_at(&d, 0xe),
            ..RoomSeat::default()
        });
        self.after_enter()?;
        Ok(Ok(()))
    }

    /// The waiting-room requests after entering: location 1, room info, room users, present.
    // SPEC: PROTOCOL_2009.md §3.7 (waiting 9: 0x306(1); 0x388 / 0x389; 0x38a / 0x38b; 0x392(1, 0))
    fn after_enter(&mut self) -> BotResult<()> {
        let mut w = PktWriter::new(LOCATION);
        w.u32(1);
        self.link.send(&w)?;
        self.link.send_empty(ROOM_INFO)?;
        let d = self.link.recv_until(ROOM_INFO_REPLY, self.timeout)?;
        let host_id = u32_at(&d, 0x5c);
        let (map, mode) = (i32_at(&d, 0x60), i32_at(&d, 0x64));
        // SPEC: PROTOCOL_2009.md §3.7 table 3.7a (+0x84 i32 weapon limit)
        let weapon_limit = u32_at(&d, 0x84);
        self.link.send_empty(ROOM_USERS)?;
        let d = self.link.recv_until(ROOM_USERS_REPLY, self.timeout)?;
        let n = i32_at(&d, 0xe).clamp(0, 32) as usize;
        let my_id = self.profile.my_id;
        if let Some(r) = self.room.as_mut() {
            r.host_id = host_id;
            r.host = host_id == my_id;
            r.map = map;
            r.mode = mode;
            r.weapon_limit = weapon_limit;
            for i in 0..n {
                let o = 0x12 + i * ROOM_USER_RECORD_SIZE;
                if o + ROOM_USER_RECORD_SIZE <= d.len() {
                    let (id, m) = room_member(&d, o);
                    r.members.insert(id, m);
                }
            }
        }
        self.send_flag(flag::PRESENT)?;
        let r = self.room.clone().unwrap_or_default();
        self.rec.event(
            "room_entered",
            json!({"room": r.number, "slot": r.slot, "host": r.host, "members": r.members.len(),
                   "map": r.map, "mode": r.mode, "weapon_limit": r.weapon_limit,
                   "team": r.team_of(my_id)}),
        );
        Ok(())
    }

    fn send_flag(&mut self, (kind, value): (u32, u32)) -> BotResult<()> {
        // SPEC: PROTOCOL_2009.md §3.7 (0x392: +6 u32 kind, +0xa u32 value)
        let mut w = PktWriter::new(ROOM_USER_FLAG);
        w.u32(kind).u32(value);
        self.link.send(&w)
    }

    /// Applies the room pushes kept so far (members joining, leaving, team and state changes, host hand-over).
    // SPEC: PROTOCOL_2009.md §3.7 (0x3033, 0x3034, 0x3041, 0x3042, 0x3035 type 2)
    pub fn absorb_room_pushes(&mut self) {
        let _ = self.absorb_effects();
    }

    /// As `absorb_room_pushes`, and reports what the pushes mean for the ready state.
    // SPEC: PROTOCOL_2009.md §3.7 (0x3042 state 0 waiting / 1 ready / 2 starting / 3 in game; 0x3035 types)
    fn absorb_effects(&mut self) -> PushEffects {
        let frames = self.link.take_ids(&[
            ROOM_USER_ADD_PUSH,
            ROOM_USER_REMOVE_PUSH,
            ROOM_TEAM_PUSH,
            ROOM_USER_STATE_PUSH,
            ROOM_UPDATE_PUSH,
        ]);
        let my_id = self.profile.my_id;
        let mut fx = PushEffects::default();
        let filler = self.filler.clone().unwrap_or_default();
        let mut removed = None;
        let Some(r) = self.room.as_mut() else {
            return fx;
        };
        for (id, d) in frames {
            match id {
                ROOM_USER_ADD_PUSH => {
                    let (uid, m) = room_member(&d, 6);
                    r.members.insert(uid, m);
                }
                ROOM_USER_REMOVE_PUSH => {
                    let uid = u32_at(&d, 6);
                    // LG-319: a room that still holds a person needed the seat; else it is closing
                    if uid == my_id {
                        removed = Some((r.number, room_has_player(&r.members, my_id, &filler)));
                    }
                    r.members.remove(&uid);
                }
                ROOM_TEAM_PUSH => {
                    if let Some(m) = r.members.get_mut(&u32_at(&d, 6)) {
                        m.team = i32_at(&d, 0xa);
                    }
                }
                ROOM_USER_STATE_PUSH => {
                    let (uid, st) = (u32_at(&d, 6), i32_at(&d, 0xa));
                    if let Some(m) = r.members.get_mut(&uid) {
                        m.state = st;
                    }
                    if uid == my_id && st == 0 {
                        fx.own_unready = true;
                    }
                    if uid == r.host_id && uid != my_id && st == 3 {
                        fx.host_in_game = true;
                    }
                }
                ROOM_UPDATE_PUSH => {
                    let (ty, val) = (u32_at(&d, 0xa), u32_at(&d, 0xe));
                    if ty == 2 {
                        r.host_id = val;
                        r.host = r.host_id == my_id;
                    } else if ty == ROOM_STATE_FIELD {
                        r.phase = val;
                    } else if is_setting_type(ty) {
                        if ty == WEAPON_LIMIT_FIELD {
                            r.weapon_limit = val;
                        }
                        fx.settings = true;
                    }
                }
                _ => {}
            }
        }
        if let Some((number, needed)) = removed {
            self.room = None;
            self.removed = true;
            let wait = if needed {
                YIELDED_ROOM_COOLDOWN
            } else {
                SKIPPED_ROOM_COOLDOWN
            };
            self.skip_room_for(number, wait);
            self.rec.event("room_left", json!({"room": number, "ok": true, "why": "removed by the lobby"}));
        }
        fx
    }

    /// The own state in the room (0 when unknown).
    fn own_state(&self) -> i32 {
        self.room
            .as_ref()
            .and_then(|r| r.members.get(&self.profile.my_id))
            .map_or(0, |m| m.state)
    }

    /// Asks for team `team` (1 / 2) in the waiting room; returns (accepted, reason). A refusal is not an error (the
    /// lobby keeps the seat it gave).
    // SPEC: PROTOCOL_2009.md §3.7 (0x390: +0xe u32 destination team, +6 / +0xa unwritten; 0x391: +6 ok, +0xa reason)
    pub fn ask_team(&mut self, team: u32) -> BotResult<(bool, u32)> {
        let mut w = PktWriter::new(dc_wire::TEAM_CHANGE);
        w.u32(0).u32(0).u32(team);
        self.link.send(&w)?;
        let d = self
            .link
            .recv_until(dc_wire::TEAM_CHANGE_REPLY, self.timeout)?;
        Ok((u32_at(&d, 6) != 0, u32_at(&d, 0xa)))
    }

    /// BOT-156: the room's weapon limit is one the lobby refused this bot's ready under before. The bot cannot change
    /// what it carries (a pool seat may not send 0x324 / 0x326, and the lobby already stands an owned passing primary
    /// in for a failing one, LG-67), so a refusal holds for every room of that limit.
    fn limit_refused(&self) -> bool {
        self.room
            .as_ref()
            .is_some_and(|r| r.weapon_limit != 0 && self.refused_limits.contains(&r.weapon_limit))
    }

    /// BOT-156: notes a weapon-limit refusal of the ready in the current room; the error the wait ends with.
    fn note_limit_refusal(&mut self) -> BotError {
        if let Some(r) = &self.room {
            self.refused_limits.insert(r.weapon_limit);
        }
        bot_err!("{WEAPON_LIMIT_REFUSED}")
    }

    /// BOT-156: leaves the room whose weapon limit the loadout fails (no ready is sent), passes it over for
    /// [`WEAPON_LIMIT_ROOM_COOLDOWN`] and logs `weapon_limit` once.
    // DC-DEFAULT: BOT-156 (proposed)
    pub fn leave_for_weapon_limit(&mut self) -> BotResult<()> {
        let Some((number, limit)) = self.room.as_ref().map(|r| (r.number, r.weapon_limit)) else {
            return Ok(());
        };
        self.skip_room_for(number, WEAPON_LIMIT_ROOM_COOLDOWN);
        self.rec.event(
            "weapon_limit",
            json!({"room": number, "limit": limit,
                   "reason": "the equipped weapons fail the room's weapon limit; a bot seat cannot change them",
                   "pass_over_s": WEAPON_LIMIT_ROOM_COOLDOWN.as_secs()}),
        );
        self.leave_room_why("weapon limit")
    }

    /// Guest: ready on. BOT-156: under a weapon limit refused before nothing is sent; a weapon-limit refusal ends
    /// with [`WEAPON_LIMIT_REFUSED`].
    pub fn ready(&mut self) -> BotResult<()> {
        if self.limit_refused() {
            return Err(bot_err!("{WEAPON_LIMIT_REFUSED}"));
        }
        self.send_flag(flag::READY)?;
        let d = self.link.recv_until(ROOM_USER_FLAG_REPLY, self.timeout)?;
        if weapon_limit_refusal(&d) {
            return Err(self.note_limit_refusal());
        }
        if u32_at(&d, 6) == 0 {
            return Err(bot_err!(
                "lobby: ready refused, reason 0x{:x}",
                u16_at(&d, 0xa)
            ));
        }
        Ok(())
    }

    /// Host: start (0x392 (0, 2)).
    pub fn start(&mut self) -> BotResult<()> {
        self.send_flag(flag::START)?;
        let d = self.link.recv_until(ROOM_USER_FLAG_REPLY, self.timeout)?;
        if u32_at(&d, 6) == 0 {
            return Err(bot_err!(
                "lobby: start refused, reason 0x{:x}",
                u16_at(&d, 0xa)
            ));
        }
        Ok(())
    }

    /// Host: waits until the room has `count` members and every other member is ready (state 1).
    pub fn wait_members(&mut self, count: usize, wait: Duration) -> BotResult<()> {
        let end = Instant::now() + wait;
        let my_id = self.profile.my_id;
        loop {
            self.absorb_room_pushes();
            if self.filler_alone() {
                return Err(bot_err!("{FILLER_ALONE}"));
            }
            if let Some(r) = &self.room {
                let others_ready = r
                    .members
                    .iter()
                    .filter(|(id, _)| **id != my_id)
                    .all(|(_, m)| m.state == 1);
                if r.members.len() >= count && others_ready {
                    return Ok(());
                }
            }
            if Instant::now() >= end {
                let have = self.room.as_ref().map_or(0, |r| r.members.len());
                return Err(bot_err!(
                    "lobby: room has {have} of {count} ready members after {wait:?}"
                ));
            }
            self.link.pump(Duration::from_millis(100))?;
        }
    }

    /// Waits for the match address push 0x3043. A guest re-sends ready when the lobby resets it (a 0x3042 for the
    /// own account with state 0, or a room settings push 0x3035) and logs `missed_start` when the host's match runs
    /// without an address for this bot.
    // SPEC: PROTOCOL_2009.md §7 (0x3043: +6 wstr[17] IPv4 text, +0x28 u32 UDP port, +0x2c u32 record count)
    // SPEC: PROTOCOL_2009.md §3.7 (0x392 (0, 1) ready on; 0x3042 state 0 after a settings change)
    // OBSERVED: capture 20261025_000014_tdm1 (bot never re-readied, 0x3043 went to the host only)
    pub fn wait_match_address(&mut self, wait: Duration) -> BotResult<MatchAddress> {
        let end = Instant::now() + wait;
        let mut watch = ReadyWatch::default();
        let d = loop {
            if let Some((_, d)) = self.link.take_ids(&[MATCH_ADDRESS_PUSH]).pop() {
                break d;
            }
            let replies = self.link.take_ids(&[ROOM_USER_FLAG_REPLY]);
            let fx = self.absorb_effects();
            // BOT-84: a filler bot waits only with a player in the room
            if let Some(e) = self.seat_gone() {
                return Err(e);
            }
            let now = Instant::now();
            let guest = self.room.as_ref().is_some_and(|r| !r.host);
            // BOT-156: a re-ready the weapon limit refused, or a limit change to one refused before, ends the wait
            // (the caller leaves the room): no ready is sent again under that limit
            if guest && replies.iter().any(|(_, d)| weapon_limit_refusal(d)) {
                return Err(self.note_limit_refusal());
            }
            if guest && self.limit_refused() {
                return Err(bot_err!("{WEAPON_LIMIT_REFUSED}"));
            }
            if guest {
                watch.on_effects(&fx, now);
                match watch.poll(now, self.own_state()) {
                    WatchAction::SendReady => {
                        self.send_flag(flag::READY)?;
                        self.rec.event(
                            "re_ready",
                            json!({"own_state": self.own_state(), "unready_push": fx.own_unready,
                                   "settings_push": fx.settings}),
                        );
                    }
                    WatchAction::MissedStart => {
                        let own = self.own_state();
                        let why = if own == 0 {
                            "not ready: the lobby holds this account at state 0"
                        } else {
                            "not invited: ready, but the lobby sent no match address to this account"
                        };
                        self.rec.event(
                            "missed_start",
                            json!({"own_state": own, "reason": why,
                                   "after_ms": MISSED_START_AFTER.as_millis() as u64}),
                        );
                    }
                    WatchAction::None => {}
                }
            }
            let left = end.saturating_duration_since(now);
            if left.is_zero() {
                return Err(bot_err!(
                    "lobby: no 0x{MATCH_ADDRESS_PUSH:x} within {wait:?}"
                ));
            }
            if !self.link.pump(left.min(Duration::from_millis(50)))?
                && !self.link.has_pending(MATCH_ADDRESS_PUSH)
            {
                return Err(bot_err!(
                    "lobby: closed while waiting for the match address"
                ));
            }
        };
        self.absorb_room_pushes();
        // DC-DEFAULT: LG-327 (proposed, project decision) the push comes right before 0x3043, so it is here already
        let ticket = self
            .link
            .take_ids(&[dc_wire::seatticket::PUSH])
            .pop()
            .and_then(|(_, t)| dc_wire::seatticket::parse_push(&t));
        let a = MatchAddress {
            ip: wstr_at(&d, 6, 17),
            port: u32_at(&d, 0x28),
            records: u32_at(&d, 0x2c),
            ticket,
        };
        self.rec.event(
            "match_address",
            json!({"ip": a.ip, "port": a.port, "records": a.records}),
        );
        Ok(a)
    }

    /// Notes that the room's match runs (the bot has its address): the room counts as playing until the lobby pushes
    /// the waiting state (0x3035 type 7 value 0).
    pub fn mark_playing(&mut self) {
        if let Some(r) = self.room.as_mut() {
            r.phase = ROOM_PLAYING;
        }
    }

    /// After a match: waits until the room is back in the waiting state (every player left the game server; the
    /// other players may sit on the result screen for a while). Errors when the room is gone, the connection
    /// closed or `wait` passed.
    // SPEC: PROTOCOL_2009.md §3.7 (0x3035 type 7 room state; 0x392 ready only in a waiting room)
    // DC-DEFAULT: BOT-62 the next match of a seat starts from the room's waiting push
    pub fn wait_room_waiting(&mut self, wait: Duration) -> BotResult<()> {
        let end = Instant::now() + wait;
        loop {
            self.absorb_room_pushes();
            if let Some(e) = self.seat_gone() {
                return Err(e);
            }
            match &self.room {
                None => return Err(bot_err!("lobby: no room after the match")),
                Some(r) if r.phase == 0 => return Ok(()),
                Some(_) => {}
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(bot_err!(
                    "lobby: the room did not return to waiting within {wait:?}"
                ));
            }
            if !self.link.pump(left.min(Duration::from_millis(100)))? {
                self.absorb_room_pushes();
                if self.room.as_ref().is_some_and(|r| r.phase == 0) {
                    return Ok(());
                }
                return Err(bot_err!("lobby: closed while waiting for the room"));
            }
        }
    }

    /// The seat's next match: waits for the waiting room, readies (guest) or waits for `start_after` ready members
    /// and starts (host), then waits for the new match address.
    // SPEC: PROTOCOL_2009.md §3.7 (0x392 (0, 1) ready, (0, 2) start; 0x3035 type 7), §7 (0x3043)
    // DC-DEFAULT: BOT-62
    pub fn next_match(&mut self, start_after: usize, wait: Duration) -> BotResult<MatchAddress> {
        self.wait_room_waiting(wait)?;
        // replies of the last match's flags (re-ready while it started) are no answer to this one
        let _ = self.link.take_ids(&[ROOM_USER_FLAG_REPLY]);
        if self.room.as_ref().is_some_and(|r| r.host) {
            self.wait_members(start_after, wait)?;
            self.start()?;
        } else {
            self.ready()?;
        }
        self.wait_match_address(wait)
    }

    /// Tells the lobby that the game connection failed, as the 2009 client does (twice, phase 0 then 1).
    // SPEC: PROTOCOL_2009.md §7 step 3 (LIVE: 0x394 twice, +6 = 0 then 1)
    pub fn report_join_failed(&mut self) -> BotResult<()> {
        for phase in [0u32, 1] {
            let mut w = PktWriter::new(MATCH_EXIT);
            w.u32(phase);
            self.link.send(&w)?;
        }
        Ok(())
    }

    /// A lobby session assembled by a caller that walked the login and channel steps itself (dc-client's menu
    /// screens: the player picks the server and the channel by hand), so the game join can take it over like one
    /// from [`Lobby::login`].
    pub fn from_parts(
        link: TcpLink,
        ticket: AuthTicket,
        profile: LobbyProfile,
        room: Option<RoomSeat>,
        timeout: Duration,
        rec: Recorder,
    ) -> Lobby {
        Lobby {
            link,
            ticket,
            profile,
            room,
            timeout,
            rec,
            filler: None,
            room_title: None,
            skipped: BTreeMap::new(),
            refused_limits: BTreeSet::new(),
            removed: false,
            seat_ticket: None,
        }
    }

    /// The own team (1 or 2) in the room.
    pub fn team(&self) -> i32 {
        self.room
            .as_ref()
            .map_or(1, |r| r.team_of(self.profile.my_id))
    }

    /// Keeps reading (and recording) the lobby connection on a thread while the bot plays: the lobby connection
    /// stays up during a match (PROTOCOL_2009.md §1.1, LIVE).
    pub fn drain_in_background(self) -> LobbyDrain {
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let closed = Arc::new(AtomicBool::new(false));
        let c2 = closed.clone();
        let alone = Arc::new(AtomicBool::new(false));
        let a2 = alone.clone();
        let mut me = self;
        let filler = me.filler.is_some();
        let handle = std::thread::spawn(move || {
            while !s2.load(Ordering::Relaxed) {
                match me.link.pump(Duration::from_millis(100)) {
                    Ok(true) => {}
                    _ => {
                        if me.link.is_closed() {
                            c2.store(true, Ordering::Relaxed);
                            std::thread::sleep(Duration::from_millis(50));
                        }
                    }
                }
                // BOT-84: a filler follows the room while it plays; the last player gone ends its match
                if filler {
                    me.absorb_room_pushes();
                    if me.filler_alone() {
                        a2.store(true, Ordering::Relaxed);
                    }
                }
            }
            me
        });
        LobbyDrain {
            stop,
            closed,
            alone,
            handle: Some(handle),
        }
    }
}

/// A lobby connection being drained on a thread.
pub struct LobbyDrain {
    stop: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    alone: Arc<AtomicBool>,
    handle: Option<JoinHandle<Lobby>>,
}

impl LobbyDrain {
    /// Shared flag that turns true once the lobby TCP connection closed.
    pub fn closed_flag(&self) -> Arc<AtomicBool> {
        self.closed.clone()
    }

    /// BOT-84: shared flag that turns true once a filler bot's room holds no player any more.
    pub fn alone_flag(&self) -> Arc<AtomicBool> {
        self.alone.clone()
    }

    /// Stops the thread and returns the lobby (with every frame received meanwhile kept).
    pub fn finish(mut self) -> Option<Lobby> {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.take().and_then(|h| h.join().ok())
    }
}

impl Drop for LobbyDrain {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    // BOT-84: a filler bot's room holds a player when a member other than itself has a name without the prefix
    #[test]
    fn a_follower_wants_that_one_player() {
        let member = |n: &str| RoomMember {
            char_no: 0,
            name: n.into(),
            team: 1,
            state: 0,
            exp: 0,
        };
        let mut m = BTreeMap::new();
        m.insert(1, member("abBot1"));
        m.insert(2, member("Other"));
        assert!(!room_has_player(&m, 1, "=zoe"));
        m.insert(3, member("Zoe"));
        assert!(room_has_player(&m, 1, "=zoe"));
        assert!(!room_has_player(&m, 3, "=zoe"), "not itself");
    }

    #[test]
    fn room_has_player_ignores_the_bots_of_the_prefix_and_itself() {
        use super::{room_has_player, RoomMember};
        use std::collections::BTreeMap;
        let m = |name: &str| RoomMember {
            char_no: 0,
            name: name.into(),
            team: 1,
            state: 0,
            exp: 0,
        };
        let mut members = BTreeMap::new();
        members.insert(1, m("VpsBot1"));
        assert!(!room_has_player(&members, 1, "VpsBot"), "alone");
        members.insert(2, m("vpsbot2"));
        assert!(
            !room_has_player(&members, 1, "VpsBot"),
            "bots only, any case"
        );
        members.insert(3, m("Erika"));
        assert!(room_has_player(&members, 1, "VpsBot"), "a player");
        members.remove(&3);
        members.insert(4, m("VpsBotFan"));
        assert!(
            !room_has_player(&members, 1, "VpsBot"),
            "the prefix decides, not the digits"
        );
        assert!(
            room_has_player(&members, 1, "Other"),
            "another prefix: VpsBot1 counts as a player"
        );
        assert!(
            room_has_player(&members, 1, ""),
            "no prefix: everybody else is a player"
        );
        assert!(!room_has_player(&BTreeMap::new(), 1, ""), "nobody");
    }

    use super::*;
    use std::net::TcpListener;

    fn entry(id: u32, port: u32, cap: u32, cur: u32, clan: u32) -> Vec<u8> {
        let mut w = PktWriter::empty();
        w.u32(id)
            .wstr("S", 33)
            .wstr("127.0.0.1", 33)
            .u32(port)
            .u32(1);
        w.u32(cap).u32(cur).at(0xbc).u32(clan);
        w.raw_bytes().to_vec()
    }

    #[test]
    fn server_entries_and_pick() {
        let mut w = PktWriter::new(SERVER_LIST_PUSH);
        w.u32(6).u32(0).u32(0).u32(3);
        w.raw(&entry(7, 100, 10, 10, 0))
            .raw(&entry(8, 200, 10, 1, 1))
            .raw(&entry(9, 300, 10, 2, 0));
        w.at(0xf16);
        let d = w.to_payload();
        let e = parse_server_entries(&d, 0x12, 0x16);
        assert_eq!(e.len(), 3);
        assert_eq!(e[1].clan_only, 1);
        assert_eq!(e[2].ip, "127.0.0.1");
        // first is full (100 %), second is a clan server: the third is picked
        assert_eq!(pick_server(&e).unwrap().id, 9);
    }

    #[test]
    fn link_keeps_frames_out_of_order() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut f = PktWriter::new(0x3044).u32(1).to_frame();
            f.extend(PktWriter::new(0x3043).u32(5).to_frame());
            // split the stream in the middle of a header
            s.write_all(&f[..5]).unwrap();
            std::thread::sleep(Duration::from_millis(30));
            s.write_all(&f[5..]).unwrap();
            std::thread::sleep(Duration::from_millis(200));
        });
        let rec = Recorder::null("t");
        let mut c = TcpLink::connect(addr, "lobby", &rec, Duration::from_secs(2)).unwrap();
        let d = c.recv_until(0x3043, Duration::from_secs(2)).unwrap();
        assert_eq!(u32_at(&d, 6), 5);
        let d = c.recv_until(0x3044, Duration::from_secs(1)).unwrap();
        assert_eq!(u32_at(&d, 6), 1);
        assert!(c.recv_until(0x3044, Duration::from_millis(100)).is_err());
        t.join().unwrap();
    }

    #[test]
    fn match_address_parses() {
        let a = MatchAddress {
            ip: "127.0.0.1".into(),
            port: 27888,
            records: 0,
            ticket: None,
        };
        assert_eq!(a.socket_addr().unwrap().to_string(), "127.0.0.1:27888");
        assert!(MatchAddress {
            ip: "host".into(),
            port: 1,
            records: 0,
            ticket: None,
        }
        .socket_addr()
        .is_err());
    }

    #[test]
    fn watch_rereadies_rate_limited() {
        let t0 = Instant::now();
        let mut w = ReadyWatch::default();
        let un = PushEffects {
            own_unready: true,
            ..Default::default()
        };
        w.on_effects(&un, t0);
        assert_eq!(w.poll(t0, 0), WatchAction::None);
        assert_eq!(
            w.poll(t0 + Duration::from_millis(300), 0),
            WatchAction::SendReady
        );
        // a second reset right away waits for the one-second gap
        w.on_effects(&un, t0 + Duration::from_millis(400));
        assert_eq!(
            w.poll(t0 + Duration::from_millis(800), 0),
            WatchAction::None
        );
        assert_eq!(
            w.poll(t0 + Duration::from_millis(1300), 0),
            WatchAction::SendReady
        );
        // once starting, no more ready
        w.on_effects(&un, t0 + Duration::from_secs(5));
        assert_eq!(w.poll(t0 + Duration::from_secs(6), 2), WatchAction::None);
    }

    #[test]
    fn watch_reports_missed_start_once() {
        let t0 = Instant::now();
        let mut w = ReadyWatch::default();
        let fx = PushEffects {
            host_in_game: true,
            ..Default::default()
        };
        w.on_effects(&fx, t0);
        assert_eq!(w.poll(t0 + Duration::from_secs(4), 1), WatchAction::None);
        assert_eq!(
            w.poll(t0 + Duration::from_secs(5), 1),
            WatchAction::MissedStart
        );
        assert_eq!(w.poll(t0 + Duration::from_secs(9), 1), WatchAction::None);
    }

    fn push_state(id: u32, st: u32) -> Vec<u8> {
        PktWriter::new(0x3042).u32(id).u32(st).u32(0).to_frame()
    }

    fn lobby_for_test(addr: SocketAddr, rec: &Recorder) -> Lobby {
        let link = TcpLink::connect(addr, "lobby", rec, Duration::from_secs(2)).unwrap();
        let mut room = RoomSeat {
            number: 1,
            host_id: 7,
            host: false,
            ..RoomSeat::default()
        };
        let m = |n: &str, state| RoomMember {
            char_no: 1,
            name: n.into(),
            team: 1,
            state,
            exp: 0,
        };
        room.members.insert(7, m("host", 1));
        room.members.insert(9, m("bot", 1));
        Lobby {
            link,
            ticket: AuthTicket {
                session_key: [0; 16],
                account_no: 1,
                lobby: addr,
                server_id: 1,
                server_name: String::new(),
                created_character: false,
            },
            profile: LobbyProfile {
                my_id: 9,
                ..LobbyProfile::default()
            },
            room: Some(room),
            timeout: Duration::from_secs(2),
            rec: rec.clone(),
            filler: None,
            room_title: None,
            skipped: BTreeMap::new(),
            refused_limits: BTreeSet::new(),
            removed: false,
            seat_ticket: None,
        }
    }

    /// LG-319: the lobby takes a filler out of a room that holds a person: its wait ends, it asks nothing more of the
    /// room, passes it over for the long wait, and sits down again in another room.
    #[test]
    fn a_filler_removed_for_a_person_looks_elsewhere() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.write_all(&PktWriter::new(ROOM_USER_REMOVE_PUSH).u32(9).u32(1).to_frame())
                .unwrap();
            s.set_read_timeout(Some(Duration::from_millis(400))).unwrap();
            let mut buf = [0u8; 64];
            matches!(s.read(&mut buf), Ok(n) if n > 0)
        });
        let rec = Recorder::new(None, None, "t").unwrap();
        let mut me = lobby_for_test(addr, &rec);
        me.set_filler(Some("PoolBot".into()));
        let e = me.wait_match_address(Duration::from_secs(5)).unwrap_err();
        assert_eq!(e.0, FILLER_ALONE);
        assert!(me.room.is_none());
        assert!(me.is_skipped(1));
        let end = me.skipped[&1];
        assert!(end > Instant::now() + SKIPPED_ROOM_COOLDOWN, "the long wait");
        me.leave_room().unwrap();
        me.skip_room(1);
        assert_eq!(me.skipped[&1], end, "a short wait does not cut it");
        assert!(!t.join().unwrap(), "no message about the room it left");
    }

    /// BOT-169: a leave the lobby refuses (0x387 {0}) ends with LEAVE_REFUSED, which the run takes as a normal end
    /// (exit 0, `ran its hour`); an accepted leave and other errors are unchanged.
    #[test]
    fn a_refused_leave_is_a_clean_end_of_the_run() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut buf = [0u8; 64];
            let n = s.read(&mut buf).unwrap_or(0);
            s.write_all(&PktWriter::new(ROOM_LEAVE_REPLY).u32(0).u16(0xa).to_frame())
                .unwrap();
            std::thread::sleep(Duration::from_millis(200));
            n
        });
        let rec = Recorder::new(None, None, "t").unwrap();
        let mut me = lobby_for_test(addr, &rec);
        let e = me.leave_room().unwrap_err();
        assert_eq!(e.0, LEAVE_REFUSED);
        assert!(me.room.is_none(), "the seat is forgotten");
        assert!(t.join().unwrap() > 0, "the leave went out");
        let end = clean_end(&e.0).expect("a clean end");
        assert!(end.starts_with("ran its hour"), "{end}");
        assert!(clean_end(&format!("{NO_ROOM_PREFIX} 3600s"))
            .is_some_and(|s| s.starts_with("ran its wait")));
        assert_eq!(clean_end("lobby: closed while waiting for the room"), None);
        assert_eq!(clean_end(FILLER_ALONE), None);
    }

    /// LG-319: a room closing on the bot (no person left) is only the short wait: the number comes back with a new room.
    #[test]
    fn a_filler_of_a_closing_room_waits_briefly() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut f = PktWriter::new(ROOM_USER_REMOVE_PUSH).u32(7).u32(0).to_frame();
            f.extend(PktWriter::new(ROOM_USER_REMOVE_PUSH).u32(9).u32(1).to_frame());
            s.write_all(&f).unwrap();
            std::thread::sleep(Duration::from_millis(200));
        });
        let rec = Recorder::new(None, None, "t").unwrap();
        let mut me = lobby_for_test(addr, &rec);
        me.set_filler(Some("PoolBot".into()));
        assert_eq!(me.wait_match_address(Duration::from_secs(5)).unwrap_err().0, FILLER_ALONE);
        assert!(me.skipped[&1] <= Instant::now() + SKIPPED_ROOM_COOLDOWN);
        t.join().unwrap();
    }

    /// BOT-62 (playtest report): after match 1 the bot's own state drops to 0 while the room still plays (the others
    /// sit on the result screen); the bot readies only once the room is back in the waiting state (0x3035 type 7
    /// value 0) and then gets the second match address.
    #[test]
    fn the_seat_readies_for_the_next_match_once_the_room_waits() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_millis(400)))
                .unwrap();
            // the bot left the game server: own state 0, room still playing; a stale ready reply of match 1
            let mut f = push_state(9, 0);
            f.extend(PktWriter::new(0x393).u32(1).u32(0).u32(0).to_frame());
            s.write_all(&f).unwrap();
            let mut buf = [0u8; 256];
            // nothing may come before the room waits
            let early = s.read(&mut buf).unwrap_or_default();
            if early != 0 {
                return Err("the bot sent something while the room still played");
            }
            s.write_all(
                &PktWriter::new(0x3035)
                    .u32(1)
                    .u32(7)
                    .u32(0)
                    .u32(0)
                    .to_frame(),
            )
            .unwrap();
            s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let n = s.read(&mut buf).map_err(|_| "no ready")?;
            let (frames, _) = parse_tcp_frames(&buf[..n]).unwrap();
            if !frames
                .iter()
                .any(|(id, p)| *id == 0x392 && u32_at(p, 6) == 0 && u32_at(p, 0xa) == 1)
            {
                return Err("no ready (0, 1) after the waiting push");
            }
            let mut a = PktWriter::new(0x393).u32(1).u32(0).u32(0).to_frame();
            let mut m = PktWriter::new(0x3043);
            m.wstr("127.0.0.1", 17).u32(29988).u32(2).pad(0x40);
            a.extend(m.to_frame());
            s.write_all(&a).unwrap();
            Ok(())
        });
        let rec = Recorder::null("t");
        let mut lb = lobby_for_test(addr, &rec);
        lb.mark_playing();
        let r = lb.next_match(1, Duration::from_secs(5));
        assert_eq!(t.join().unwrap(), Ok(()));
        let a = r.expect("second match address");
        assert_eq!((a.ip.as_str(), a.port), ("127.0.0.1", 29988));
    }

    /// The frames of one read on the fake lobby's socket (empty on a timeout).
    fn read_frames(s: &mut TcpStream) -> Vec<(i32, Vec<u8>)> {
        let mut buf = [0u8; 512];
        match s.read(&mut buf) {
            Ok(n) if n > 0 => parse_tcp_frames(&buf[..n]).unwrap().0,
            _ => Vec::new(),
        }
    }

    fn limit_refusal() -> Vec<u8> {
        PktWriter::new(0x393)
            .u32(0)
            .u32(dc_wire::flagrefusal::REASON_WEAPON_LIMIT as u32)
            .to_frame()
    }

    /// BOT-156: only a 0x393 refusal with LG-342's weapon-limit reason counts as a weapon-limit refusal.
    #[test]
    fn the_weapon_limit_refusal_is_told_by_its_reason() {
        let body = |f: Vec<u8>| parse_tcp_frames(&f).unwrap().0.remove(0).1;
        assert!(weapon_limit_refusal(&body(limit_refusal())));
        let other = PktWriter::new(0x393).u32(0).u32(0).to_frame();
        assert!(!weapon_limit_refusal(&body(other)));
        let ok = PktWriter::new(0x393)
            .u32(1)
            .u32(dc_wire::flagrefusal::REASON_WEAPON_LIMIT as u32)
            .to_frame();
        assert!(!weapon_limit_refusal(&body(ok)));
    }

    /// BOT-156: in a "Sniper Only" room (limit 6) the lobby refuses the bot's ready: the bot leaves without readying
    /// again, passes the room over for the long wait, readies in a "Shotguns Only" room (limit 7) its loadout passes,
    /// and in another limit-6 room sends no ready at all.
    #[test]
    fn a_weapon_limit_refusal_is_not_repeated() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || -> Result<(), &'static str> {
            let (mut s, _) = l.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let is_ready = |f: &[(i32, Vec<u8>)]| {
                f.iter()
                    .any(|(id, p)| *id == 0x392 && u32_at(p, 6) == 0 && u32_at(p, 0xa) == 1)
            };
            if !is_ready(&read_frames(&mut s)) {
                return Err("no first ready");
            }
            s.write_all(&limit_refusal()).unwrap();
            // the leave, nothing before it
            let f = read_frames(&mut s);
            if f.len() != 1 || f[0].0 != i32::from(ROOM_LEAVE) {
                return Err("a second message in the refused room instead of the leave");
            }
            s.write_all(&PktWriter::new(ROOM_LEAVE_REPLY).u32(1).to_frame())
                .unwrap();
            // the limit-7 room: a ready, accepted
            if !is_ready(&read_frames(&mut s)) {
                return Err("no ready in the room the loadout passes");
            }
            s.write_all(&PktWriter::new(0x393).u32(1).u32(0).to_frame())
                .unwrap();
            // the second limit-6 room: nothing
            s.set_read_timeout(Some(Duration::from_millis(400)))
                .unwrap();
            if !read_frames(&mut s).is_empty() {
                return Err("a ready under a limit refused before");
            }
            Ok(())
        });
        let rec = Recorder::null("t");
        let mut me = lobby_for_test(addr, &rec);
        me.room.as_mut().unwrap().weapon_limit = dc_bute::arms::SNIPER_ONLY;
        assert_eq!(me.ready().unwrap_err().0, WEAPON_LIMIT_REFUSED);
        assert_eq!(
            me.ready().unwrap_err().0,
            WEAPON_LIMIT_REFUSED,
            "no second ready sent"
        );
        me.leave_for_weapon_limit().unwrap();
        assert!(me.room.is_none());
        assert!(
            me.skipped[&1] > Instant::now() + YIELDED_ROOM_COOLDOWN,
            "the long pass-over"
        );
        me.room = Some(RoomSeat {
            number: 2,
            weapon_limit: dc_bute::arms::SHOTGUNS_ONLY,
            ..RoomSeat::default()
        });
        me.ready().unwrap();
        me.room = Some(RoomSeat {
            number: 3,
            weapon_limit: dc_bute::arms::SNIPER_ONLY,
            ..RoomSeat::default()
        });
        assert_eq!(me.ready().unwrap_err().0, WEAPON_LIMIT_REFUSED);
        assert_eq!(t.join().unwrap(), Ok(()));
    }

    /// BOT-156: the host sets "Sniper Only" after the bot readied: the lobby resets it, its re-ready is refused, and
    /// the wait ends with the weapon-limit error (the caller leaves) without a second ready.
    #[test]
    fn a_limit_set_while_waiting_ends_the_wait_after_one_refusal() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || -> Result<(), &'static str> {
            let (mut s, _) = l.accept().unwrap();
            let mut f = PktWriter::new(0x3035)
                .u32(1)
                .u32(WEAPON_LIMIT_FIELD)
                .u32(dc_bute::arms::SNIPER_ONLY)
                .u32(0)
                .to_frame();
            f.extend(push_state(9, 0));
            s.write_all(&f).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            if !read_frames(&mut s).iter().any(|(id, _)| *id == 0x392) {
                return Err("no re-ready");
            }
            s.write_all(&limit_refusal()).unwrap();
            // a further settings push: no ready follows
            s.write_all(&push_state(9, 0)).unwrap();
            s.set_read_timeout(Some(Duration::from_millis(1500)))
                .unwrap();
            if !read_frames(&mut s).is_empty() {
                return Err("a second ready after the refusal");
            }
            Ok(())
        });
        let rec = Recorder::null("t");
        let mut me = lobby_for_test(addr, &rec);
        let e = me.wait_match_address(Duration::from_secs(5)).unwrap_err();
        assert_eq!(e.0, WEAPON_LIMIT_REFUSED);
        assert!(me.refused_limits.contains(&dc_bute::arms::SNIPER_ONLY));
        assert_eq!(t.join().unwrap(), Ok(()));
    }

    /// Scripted host: a settings change resets the bot to state 0, and the match address goes only to a bot
    /// that sent ready again (0x392 (0, 1)).
    #[test]
    fn rereadies_after_settings_change_and_gets_start() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            // 0x3035 type 3 (map) then 0x3042 {bot, 0}
            let mut f = PktWriter::new(0x3035)
                .u32(1)
                .u32(3)
                .u32(5)
                .u32(0)
                .to_frame();
            f.extend(push_state(9, 0));
            s.write_all(&f).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut buf = [0u8; 64];
            let n = s.read(&mut buf).unwrap();
            let (frames, _) = parse_tcp_frames(&buf[..n]).unwrap();
            let ok = frames
                .iter()
                .any(|(id, p)| *id == 0x392 && u32_at(p, 6) == 0 && u32_at(p, 0xa) == 1);
            if ok {
                let mut a = PktWriter::new(0x3043);
                a.wstr("127.0.0.1", 17).pad(0x50);
                s.write_all(&a.to_frame()).ok();
            }
            ok
        });
        let rec = Recorder::null("t").keep_in_memory();
        let mut lb = lobby_for_test(addr, &rec);
        let r = lb.wait_match_address(Duration::from_secs(4));
        assert!(t.join().unwrap(), "bot did not re-send ready (0, 1)");
        assert!(r.is_ok(), "{r:?}");
        assert!(rec.memory().iter().any(|e| e["ev"] == "re_ready"));
    }
}

#[cfg(test)]
mod seat_tests {
    use super::*;
    use dc_wire::loginmods::{REASON_NO_POOL, REASON_POOL_ADDRESS, REASON_POOL_FULL};

    /// LG-318: the login field of a seat is the mark alone; the lobby's refusals become the exit codes `swarm` reads.
    #[test]
    fn a_seat_login_names_nobody_and_refusals_have_exit_codes() {
        let a = "127.0.0.1:1".parse().unwrap();
        let lo = LoginOptions::new(a, "", "seat1");
        assert_eq!(lo.login_field(), format!("{LOCAL_TOKEN_PREFIX};b=1"));
        for (r, code) in [
            (REASON_NO_POOL, EXIT_NO_POOL),
            (REASON_POOL_FULL, EXIT_POOL_FULL),
            (REASON_POOL_ADDRESS, EXIT_POOL_ADDRESS),
        ] {
            let text = format!("auth: login refused, reason 0x{r:08x}");
            assert_eq!(exit_code_of(&text), Some(code));
        }
        assert_eq!(exit_code_of("auth: login refused, reason 0x00030025"), None);
    }
}
