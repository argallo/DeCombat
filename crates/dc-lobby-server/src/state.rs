//! Server state behind ONE non-reentrant mutex, and the per-event transaction scope.
//!
//! Every packet, timer tick and game-server event locks the state once and runs inside [`Core::scope`]:
//! the scope opens an SQLite write transaction; handlers queue their sends in the outbox; on COMMIT the outbox is
//! flushed to the per-connection writer channels (in queue order, still under the lock, so every client sees
//! pushes in the order the state changed); on any error the transaction is rolled back and the outbox discarded.
//! Work that needs the shared handle (threads for timers and game-server pipes) is queued as [`Deferred`] and run
//! by the caller after the scope.

use crate::cli::Options;
use crate::config::Rules;
use crate::game::Game;
use crate::logs::{Listener, Logger};
use crate::modhost::ModClient;
use crate::rom::RomData;
use crate::room::{Room, RoomKey};
use rusqlite::Connection;
use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub type Sid = u64;
pub type Shared = Arc<Mutex<Core>>;

/// What a connection's writer thread receives.
#[derive(Debug)]
pub enum Outgoing {
    Frame(Vec<u8>),
    Close,
}

/// Error of one handler run: the transaction is rolled back and nothing is sent.
#[derive(Debug)]
pub enum Fault {
    Db(rusqlite::Error),
    Other(String),
}

impl From<rusqlite::Error> for Fault {
    fn from(e: rusqlite::Error) -> Self {
        Fault::Db(e)
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fault::Db(e) => write!(f, "db: {e}"),
            Fault::Other(s) => write!(f, "{s}"),
        }
    }
}

pub type HResult = Result<(), Fault>;

pub struct Session {
    pub sid: Sid,
    pub listener: Listener,
    pub peer: String,
    /// Local address (ip:port) of the accepted stream: the address this peer reached us at (LG-30).
    pub local: String,
    pub tx: Sender<Outgoing>,
    /// Account bound by the auth login (0x200) or the lobby login (0x300).
    pub account: Option<u32>,
    /// True after a successful lobby login.
    pub in_lobby: bool,
    pub channel: Option<u32>,
    /// The server-list entry the lobby login's ticket picked (0 = none; CD-21 amended: its channel block).
    pub server: u32,
    pub room: Option<RoomKey>,
    pub location: u32,
    /// Private-state mode of 0x30a (0 = none, 1 shop, 3 tuning, 4 clan).
    pub private_state: u32,
    /// Set when the server dropped this connection; the reader thread's later EOF is then a no-op.
    pub closing: bool,
    /// When the connection was accepted (the TCP connection age of `GET /net`, LG-185).
    pub since: std::time::Instant,
}

/// A session key handed out by 0x20b / 0x3b3.
// DC-DEFAULT: CD-19 16 random bytes, single use, valid auth.session_ttl_secs
#[derive(Clone, Debug)]
pub struct Ticket {
    pub account: u32,
    pub server: u32,
    pub expires: Instant,
}

/// Work that needs the shared handle; run after the scope that queued it.
#[derive(Debug, Clone)]
pub enum Deferred {
    /// Start the countdown thread of a room start (`gen` guards against a stale start).
    Countdown { room: RoomKey, gen: u64 },
    /// Start the pipe reader threads and the `@ready` watchdog of a spawned game server.
    WatchGame { game: u64 },
    /// Force-kill a game server that has not exited `secs` seconds after `quit`.
    ReapGame { game: u64, secs: u64 },
}

/// Unpredictable session keys: SHA-256 over 32 bytes of the OS generator, an OS-seeded hasher state, the clock, the
/// process id and a counter.
pub struct KeySource {
    seed: [u8; 32],
    counter: u64,
}

impl KeySource {
    pub fn new() -> Self {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        let mut material = Vec::new();
        for i in 0..4u64 {
            let mut h = RandomState::new().build_hasher();
            h.write_u64(i);
            material.extend_from_slice(&h.finish().to_le_bytes());
        }
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        material.extend_from_slice(&t.as_nanos().to_le_bytes());
        material.extend_from_slice(&std::process::id().to_le_bytes());
        // session keys, invite and recovery codes rest on this seed: the OS generator itself, 32 bytes
        let mut os = [0u8; 32];
        if ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut os).is_ok() {
            material.extend_from_slice(&os);
        }
        Self {
            seed: dc_rom::sha256(&material),
            counter: 0,
        }
    }

    fn next32(&mut self) -> [u8; 32] {
        self.counter += 1;
        let mut m = self.seed.to_vec();
        m.extend_from_slice(&self.counter.to_le_bytes());
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        m.extend_from_slice(&t.as_nanos().to_le_bytes());
        dc_rom::sha256(&m)
    }

    pub fn key16(&mut self) -> [u8; 16] {
        let d = self.next32();
        let mut k = [0u8; 16];
        k.copy_from_slice(&d[..16]);
        k
    }

    pub fn u32(&mut self) -> u32 {
        let d = self.next32();
        u32::from_le_bytes([d[0], d[1], d[2], d[3]])
    }
}

impl Default for KeySource {
    fn default() -> Self {
        Self::new()
    }
}

/// LG-271: what the hashes of a login must equal (`dc_wire::loginmods`): this lobby's mod-set hash (the one
/// `server_list.json` publishes) and the hash of the weapon content its game servers get (`0`: none).
// DC-DEFAULT: LG-271 (proposed, project decision)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedMods {
    pub set: String,
    pub content: String,
}

impl Default for ExpectedMods {
    fn default() -> Self {
        ExpectedMods {
            set: dc_wire::loginmods::EMPTY_SET_HASH.to_string(),
            content: dc_wire::loginmods::NO_CONTENT.to_string(),
        }
    }
}

pub struct Core {
    pub opts: Options,
    /// LG-327: accounts to tell, back in their room, that their client is too old for seat tickets.
    pub seat_notices: std::collections::BTreeSet<u32>,
    pub rules: Rules,
    pub rom: Arc<RomData>,
    pub log: Arc<Logger>,
    pub db: Connection,
    pub sessions: BTreeMap<Sid, Session>,
    pub tickets: HashMap<[u8; 16], Ticket>,
    pub rooms: BTreeMap<RoomKey, Room>,
    pub games: BTreeMap<u64, Game>,
    pub next_game: u64,
    pub join_seq: u64,
    pub keys: KeySource,
    pub mods: Option<ModClient>,
    /// The community maps of the loaded mod set, in table order after the ROM's rows (MAP-35; `Core::maps`).
    pub community_maps: Vec<crate::maps::CommunityMap>,
    /// LG-271: the mod-set and content hashes every login must carry (vanilla: the empty set, no content).
    pub login_mods: ExpectedMods,
    /// LG-313: the curated sets of the featured server entries, by entry id; none = every pick as before.
    pub entry_mods: BTreeMap<u32, crate::modset::EntrySet>,
    /// LG-427: the commands the lobby-wide mod set declares in its manifests (`/help` lists them; empty unless the
    /// lobby runs a modded set; a featured entry's set carries its own).
    pub mod_commands: Vec<dc_mod_lua::ModCommand>,
    /// LG-313: the hashes a connection last claimed for an entry (0x3f11), by connection.
    pub entry_claims: BTreeMap<Sid, dc_wire::entrymods::EntryHashes>,
    /// Per-session operator state (observer mode, report counters); see gm.rs.
    pub ops: crate::gm::OpsState,
    /// Set by the graceful stop (LG-130): new connections are refused from then on.
    pub stopping: bool,
    /// Process start (the admin endpoint's uptime).
    pub started: Instant,
    /// Lobby logins accepted since the start (the admin endpoint's status).
    pub lobby_logins: u64,
    /// Registration and login attempt limits (LG-145, LG-146).
    pub limits: crate::limits::Limits,
    /// The recovery mail sender (LG-242, LG-245); off until main starts it.
    pub mail: crate::mail::Mailer,
    /// The report mails on their way (LG-316).
    pub report_mail: crate::reportmail::ReportMail,
    /// The beta signup flows: limiter windows, the mail answers, the operator page's CSRF token (LG-370..LG-375).
    pub beta: crate::beta::Runtime,
    /// Identity tokens issued / accepted by this lobby (LG-68); off by default.
    pub identity: crate::identity::Authority,
    /// The remote game hosts connected now (LG-300).
    pub hosts: crate::hosts::Registry,
    /// The bot seats (LG-318).
    pub pool: crate::botpool::Pool,
    /// The operator monitor's per-game bookkeeping (MON-2).
    pub monitor: crate::opsmon::Monitor,
    /// LG-388: the managed bots' processes (`bots.managed`; botsup.rs).
    pub bot_sup: Option<crate::botsup::Handle>,
    outbox: Vec<(Sid, Outgoing)>,
    deferred: Vec<Deferred>,
    ready_deferred: Vec<Deferred>,
    in_scope: bool,
}

impl Core {
    pub fn new(
        opts: Options,
        rules: Rules,
        rom: Arc<RomData>,
        log: Arc<Logger>,
        db: Connection,
    ) -> Self {
        let mail = crate::mail::Mailer::off(Arc::clone(&log));
        Self {
            report_mail: crate::reportmail::ReportMail::new(),
            beta: crate::beta::Runtime::new(),
            opts,
            rules,
            rom,
            log,
            db,
            sessions: BTreeMap::new(),
            seat_notices: Default::default(),
            tickets: HashMap::new(),
            rooms: BTreeMap::new(),
            games: BTreeMap::new(),
            next_game: 1,
            join_seq: 0,
            keys: KeySource::new(),
            mods: None,
            community_maps: Vec::new(),
            login_mods: ExpectedMods::default(),
            entry_mods: BTreeMap::new(),
            mod_commands: Vec::new(),
            entry_claims: BTreeMap::new(),
            ops: Default::default(),
            stopping: false,
            started: Instant::now(),
            lobby_logins: 0,
            limits: Default::default(),
            mail,
            identity: Default::default(),
            hosts: Default::default(),
            pool: Default::default(),
            monitor: Default::default(),
            bot_sup: None,
            outbox: Vec::new(),
            deferred: Vec::new(),
            ready_deferred: Vec::new(),
            in_scope: false,
        }
    }

    /// Runs `f` in one SQLite write transaction. Sends queued by `f` are delivered only after COMMIT; on error
    /// the transaction is rolled back and the queued sends and deferred work are discarded. Not re-entrant (a
    /// nested call panics). A panic inside `f` is caught and treated as an error, so one bad packet cannot leave
    /// the state locked in a half-open scope.
    pub fn scope<F>(&mut self, f: F) -> HResult
    where
        F: FnOnce(&mut Core) -> HResult,
    {
        assert!(!self.in_scope, "Core::scope is not re-entrant");
        self.in_scope = true;
        self.outbox.clear();
        self.deferred.clear();
        let r = match self.db.execute_batch("BEGIN IMMEDIATE") {
            Ok(()) => {
                let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)));
                let r = match caught {
                    Ok(r) => r,
                    Err(p) => {
                        let msg = p
                            .downcast_ref::<&str>()
                            .map(|s| s.to_string())
                            .or_else(|| p.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "unknown".into());
                        Err(Fault::Other(format!("handler panic: {msg}")))
                    }
                };
                match r {
                    Ok(()) => self.db.execute_batch("COMMIT").map_err(Fault::from),
                    Err(e) => Err(e),
                }
            }
            Err(e) => Err(Fault::from(e)),
        };
        match &r {
            Ok(()) => {
                self.flush_outbox();
                let d = std::mem::take(&mut self.deferred);
                self.ready_deferred.extend(d);
            }
            Err(_) => {
                if !self.db.is_autocommit() {
                    let _ = self.db.execute_batch("ROLLBACK");
                }
                self.outbox.clear();
                self.deferred.clear();
            }
        }
        self.in_scope = false;
        r
    }

    /// Deferred work queued by committed scopes since the last call.
    pub fn take_deferred(&mut self) -> Vec<Deferred> {
        std::mem::take(&mut self.ready_deferred)
    }

    pub fn defer(&mut self, d: Deferred) {
        debug_assert!(self.in_scope);
        self.deferred.push(d);
    }

    /// Queues one frame for `sid` (delivered after COMMIT).
    pub fn send(&mut self, sid: Sid, frame: Vec<u8>) {
        debug_assert!(self.in_scope, "send outside a scope");
        self.outbox.push((sid, Outgoing::Frame(frame)));
    }

    pub fn send_all(&mut self, sids: &[Sid], frame: &[u8]) {
        for &s in sids {
            self.send(s, frame.to_vec());
        }
    }

    /// The address announced to connection `sid` in place of `configured` (LG-30, `announce::announced_ip`).
    pub fn announced_ip(&self, sid: Sid, configured: &str) -> String {
        match self.sessions.get(&sid) {
            Some(s) => crate::announce::announced_ip(&s.peer, &s.local, configured),
            None => configured.to_string(),
        }
    }

    /// Queues a connection close after everything already queued for it.
    pub fn close(&mut self, sid: Sid) {
        if let Some(s) = self.sessions.get_mut(&sid) {
            s.closing = true;
        }
        self.outbox.push((sid, Outgoing::Close));
    }

    /// Frames queued so far in the current scope (tests).
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn queued(&self) -> Vec<(Sid, Vec<u8>)> {
        self.outbox
            .iter()
            .filter_map(|(s, o)| match o {
                Outgoing::Frame(f) => Some((*s, f.clone())),
                Outgoing::Close => None,
            })
            .collect()
    }

    fn flush_outbox(&mut self) {
        let out = std::mem::take(&mut self.outbox);
        for (sid, o) in out {
            let Some(s) = self.sessions.get(&sid) else {
                continue;
            };
            if let Outgoing::Frame(f) = &o {
                if self.log.has_pktlog() && f.len() >= 8 {
                    let id = i32::from_le_bytes([f[0], f[1], f[2], f[3]]);
                    self.log.packet(s.listener, sid, true, id, &f[8..]);
                }
            }
            let _ = s.tx.send(o);
        }
        // sessions the server dropped are forgotten once their close is queued
        self.sessions.retain(|_, s| !s.closing);
    }

    pub fn note(&self, msg: &str) {
        self.log.line(msg);
    }

    /// The lobby-wide map table: the ROM's rows, then the community maps of the lobby's own set (MAP-35).
    pub fn maps(&self) -> crate::maps::MapTable<'_> {
        self.maps_for(None)
    }

    /// The map table of a server entry: the ROM's rows, then that entry's community maps when it has a curated set
    /// (MAP-82), else the lobby-wide ones (a plain entry of a vanilla lobby has none; `None` is the lobby-wide table).
    // DC-DEFAULT: MAP-82 (proposed)
    pub fn maps_for(&self, entry: Option<u32>) -> crate::maps::MapTable<'_> {
        let community = entry
            .and_then(|e| self.entry_mods.get(&e))
            .map_or(&self.community_maps[..], |e| &e.client.maps[..]);
        crate::maps::MapTable {
            rom: &self.rom,
            community,
        }
    }

    /// The map table of a room's channel: every room query goes through this one (MAP-82). The clan lobby's channels
    /// belong to no entry: the lobby-wide table.
    pub fn maps_of_channel(&self, channel: u32) -> crate::maps::MapTable<'_> {
        self.maps_for(crate::servers::entry_of_channel(self, channel).map(|e| e.id))
    }

    /// Installs the loaded set's community maps (right after `mods` is set). Returns the log lines: the table,
    /// and a warning when the 0x311 table (30 pairs at most) cannot carry every map.
    // DC-DEFAULT: MAP-35 (proposed) the 0x311 cap: pairs past the 30th are not offered, with a warning
    pub fn install_community_maps(&mut self, maps: Vec<crate::maps::CommunityMap>) -> Vec<String> {
        self.community_maps = maps;
        self.map_notes(None)
    }

    /// The log lines of a map table: the table, and the warning of the 0x311 cap (MAP-35). Nothing without a community
    /// map. `Some(entry)` names the entry's table (MAP-82).
    pub fn map_notes(&self, entry: Option<u32>) -> Vec<String> {
        let mut notes = Vec::new();
        let table = self.maps_for(entry);
        if table.community.is_empty() {
            return notes;
        }
        let listed: Vec<String> = table
            .community_indices()
            .into_iter()
            .map(|(i, k)| format!("{i}={k}"))
            .collect();
        let whose = entry.map_or(String::new(), |e| format!("entry {e}: "));
        notes.push(format!(
            "maps: {whose}{} community map(s) after the {} ROM row(s): {}",
            table.community.len(),
            table.first_community_index(),
            listed.join(", ")
        ));
        let offered = crate::lobby::filter_masks(
            self,
            table.map_mode_masks(),
            false,
            &crate::lobby::policy(),
        );
        if offered.len() > crate::lobby::ROOM_SETTINGS_MAX_PAIRS {
            notes.push(format!(
                "maps: {whose}WARNING the room-settings table carries at most {} (map, modes) pairs and this \
                 lobby has {}: the last {} map(s) are not offered to clients",
                crate::lobby::ROOM_SETTINGS_MAX_PAIRS,
                offered.len(),
                offered.len() - crate::lobby::ROOM_SETTINGS_MAX_PAIRS
            ));
        }
        notes
    }

    /// The 0x311 room-settings version announced in 0x301: a modded lobby derives it from its mod-set hash (the
    /// first eight hex digits as a number), so a client that cached another set's table fetches this one; a
    /// vanilla lobby (the empty set) keeps today's constant exactly.
    // DC-DEFAULT: MAP-38 (proposed)
    pub fn room_settings_version(&self) -> u32 {
        self.room_settings_version_for(None)
    }

    /// [`Core::room_settings_version`] for a session on `entry`: a featured entry's derives from ITS set hash, so a
    /// client that moves between a plain entry and that one fetches the table with the entry's maps (MAP-38, MAP-82).
    pub fn room_settings_version_for(&self, entry: Option<u32>) -> u32 {
        let set = entry
            .and_then(|e| self.entry_mods.get(&e))
            .map_or(&self.login_mods.set, |e| &e.expected.set);
        if set == dc_wire::loginmods::EMPTY_SET_HASH || set.len() < 8 {
            return crate::lobby::ROOM_SETTINGS_VERSION;
        }
        u32::from_str_radix(&set[..8], 16).unwrap_or(crate::lobby::ROOM_SETTINGS_VERSION)
    }

    /// Session ids of the lobby session(s) bound to an account.
    pub fn sids_of(&self, account: u32) -> Vec<Sid> {
        self.sessions
            .values()
            .filter(|s| s.in_lobby && !s.closing && s.account == Some(account))
            .map(|s| s.sid)
            .collect()
    }

    /// Users in a channel lobby who are not in a room (the audience of lobby pushes).
    // DC-DEFAULT: CD-29 lobby push audience
    pub fn lobby_audience(&self, channel: u32) -> Vec<Sid> {
        self.sessions
            .values()
            .filter(|s| s.in_lobby && !s.closing && s.channel == Some(channel) && s.room.is_none())
            .map(|s| s.sid)
            .collect()
    }
}

#[cfg(test)]
pub mod testing {
    //! A state over an in-memory DB with fake sessions whose frames land in channels.
    use super::*;
    use std::sync::mpsc::{channel, Receiver};

    pub fn opts() -> Options {
        crate::cli::parse_from(["test", "--countdown", "0.01"]).unwrap()
    }

    pub fn state() -> Core {
        // LG-310 / LG-311 stay out of the other flows' balances; the reward tests switch them on
        let rules = Rules {
            nx_enabled: false,
            ..Rules::default()
        };
        let mut st = Core::new(
            opts(),
            rules,
            Arc::new(RomData::empty()),
            Arc::new(Logger::none()),
            crate::db::memory(),
        );
        // synthetic starter kit (no ROM data): four weapon slots and a backpack
        Arc::get_mut(&mut st.rom).unwrap().starter = vec![
            crate::rom::StarterItem {
                item_no: 101,
                slot: 0,
                default_kind: 1,
            },
            crate::rom::StarterItem {
                item_no: 102,
                slot: 1,
                default_kind: 2,
            },
            crate::rom::StarterItem {
                item_no: 103,
                slot: 2,
                default_kind: 3,
            },
            crate::rom::StarterItem {
                item_no: 104,
                slot: 3,
                default_kind: 4,
            },
            crate::rom::StarterItem {
                item_no: 7001,
                slot: 9,
                default_kind: 6,
            },
        ];
        st
    }

    pub fn connect(st: &mut Core, sid: Sid, l: Listener) -> Receiver<Outgoing> {
        let (tx, rx) = channel();
        st.sessions.insert(
            sid,
            Session {
                sid,
                listener: l,
                peer: "test".into(),
                local: "test".into(),
                tx,
                account: None,
                in_lobby: false,
                channel: None,
                server: 0,
                room: None,
                location: 0,
                private_state: 0,
                closing: false,
                since: std::time::Instant::now(),
            },
        );
        rx
    }

    /// Drains the frames delivered to one fake connection as (frame id, payload).
    pub fn drain(rx: &Receiver<Outgoing>) -> Vec<(u16, Vec<u8>)> {
        let mut v = Vec::new();
        while let Ok(o) = rx.try_recv() {
            if let Outgoing::Frame(f) = o {
                let id = u16::from_le_bytes([f[0], f[1]]);
                v.push((id, f[8..].to_vec()));
            }
        }
        v
    }

    pub fn u32_at(p: &[u8], o: usize) -> u32 {
        u32::from_le_bytes([p[o], p[o + 1], p[o + 2], p[o + 3]])
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn outbox_flushes_only_after_commit() {
        let mut st = state();
        let rx = connect(&mut st, 1, Listener::Lobby);
        st.scope(|st| {
            st.send(1, crate::wire::u32_push(0x3022, 5));
            assert!(drain(&rx).is_empty(), "nothing leaves before COMMIT");
            Ok(())
        })
        .unwrap();
        let got = drain(&rx);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, 0x3022);
    }

    #[test]
    fn outbox_and_db_writes_are_discarded_on_error() {
        let mut st = state();
        let rx = connect(&mut st, 1, Listener::Lobby);
        let r = st.scope(|st| {
            st.db
                .execute("INSERT INTO accounts (login) VALUES ('x')", [])?;
            st.send(1, crate::wire::u32_push(0x3022, 5));
            st.defer(Deferred::WatchGame { game: 1 });
            Err(Fault::Other("boom".into()))
        });
        assert!(r.is_err());
        assert!(drain(&rx).is_empty());
        assert!(st.take_deferred().is_empty());
        let n: i64 = st
            .db
            .query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "rolled back");
        // the state is usable again
        st.scope(|st| {
            st.db
                .execute("INSERT INTO accounts (login) VALUES ('y')", [])?;
            st.defer(Deferred::WatchGame { game: 2 });
            Ok(())
        })
        .unwrap();
        let n: i64 = st
            .db
            .query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(st.take_deferred().len(), 1);
    }

    #[test]
    fn scope_is_not_reentrant_and_survives_a_panic() {
        let mut st = state();
        let r = st.scope(|st| st.scope(|_| Ok(())));
        assert!(
            matches!(&r, Err(Fault::Other(m)) if m.contains("not re-entrant")),
            "{r:?}"
        );
        let r = st.scope(|_| panic!("bad packet"));
        assert!(
            matches!(&r, Err(Fault::Other(m)) if m.contains("bad packet")),
            "{r:?}"
        );
        // still usable
        st.scope(|st| {
            st.db
                .execute("INSERT INTO accounts (login) VALUES ('z')", [])?;
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn close_is_ordered_after_queued_frames_and_forgets_the_session() {
        let mut st = state();
        let rx = connect(&mut st, 1, Listener::Lobby);
        st.scope(|st| {
            st.send(1, crate::wire::u32_push(0x3022, 1));
            st.close(1);
            Ok(())
        })
        .unwrap();
        let first = rx.try_recv().unwrap();
        assert!(matches!(first, Outgoing::Frame(_)));
        assert!(matches!(rx.try_recv().unwrap(), Outgoing::Close));
        assert!(!st.sessions.contains_key(&1));
    }

    #[test]
    fn keys_are_distinct() {
        let mut k = KeySource::new();
        let a = k.key16();
        let b = k.key16();
        assert_ne!(a, b);
        assert_ne!(k.u32(), k.u32());
    }
}
