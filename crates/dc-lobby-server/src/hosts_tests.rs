//! Remote game hosts (HOST_AGENT.md, LG-300 / LG-301, CD-21 amended, CD-88): the server list and its channel
//! blocks, the host registry (hello, refusals, one connection per name, offline, reattach, silence), and a room
//! start on a remote host through a fake agent on a loopback stream.

use crate::game;
use crate::hosts::*;
use crate::logs::Listener;
use crate::msg::*;
use crate::net::{lock, route};
use crate::room::{RoomKey, RoomPhase};
use crate::state::testing::*;
use crate::state::{Core, Deferred, Outgoing, Shared, Sid};
use dc_wire::PktWriter;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TOKEN: &str = "0123456789abcdef01234567";

fn entry(name: &str, host: &str) -> dc_config::LobbyServerCfg {
    dc_config::LobbyServerCfg {
        name: name.into(),
        capacity: 500,
        host: host.into(),
        channels: 3,
        features: Vec::new(),
        mods: Vec::new(),
        client_visual_allowed: false,
        lobby: String::new(),
        fingerprint: String::new(),
        policy: String::new(),
        region: String::new(),
    }
}

fn agent(name: &str, address: &str) -> dc_config::HostAgentCfg {
    dc_config::HostAgentCfg {
        name: name.into(),
        token: TOKEN.into(),
        public_address: address.into(),
    }
}

/// A lobby with "US Central" (here, channels 1..3) and "US West" (agent us-west, channels 4..6).
fn two_regions(st: &mut Core) {
    st.opts.server_name = "US Central".into();
    st.opts.clan_name = "US Central Clan".into();
    st.opts.servers = vec![entry("US Central", ""), entry("US West", "us-west")];
    st.opts.host_agents = vec![agent("us-west", "203.0.113.9")];
    st.hosts.timing = Timing {
        silence: Duration::from_secs(60),
        ping_every: Duration::from_secs(60),
        refuse_delay: Duration::from_millis(50),
        hello_wait: Duration::from_secs(2),
    };
}

fn shared_with(f: impl FnOnce(&mut Core)) -> Shared {
    let mut st = state();
    two_regions(&mut st);
    f(&mut st);
    Arc::new(Mutex::new(st))
}

fn wait_until(shared: &Shared, what: &str, f: impl Fn(&Core) -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if f(&lock(shared)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

// ------------------------------------------------------------------ the fake agent

/// The test's agent: one end of a loopback stream whose other end the lobby serves ([`serve`], the part of the
/// host port after its TLS handshake).
struct FakeAgent {
    r: BufReader<TcpStream>,
    w: TcpStream,
}

fn dial(shared: &Shared) -> FakeAgent {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
    let (s, peer) = l.accept().unwrap();
    s.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
    let sh = Arc::clone(shared);
    std::thread::spawn(move || {
        let r = s.try_clone().unwrap();
        serve(&sh, r, s, peer)
    });
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    FakeAgent {
        r: BufReader::new(c.try_clone().unwrap()),
        w: c,
    }
}

impl FakeAgent {
    fn send(&mut self, line: &str) {
        self.w.write_all(format!("{line}\n").as_bytes()).unwrap();
    }

    /// The next line, None at the end of the stream.
    fn recv(&mut self) -> Option<String> {
        let mut l = String::new();
        match self.r.read_line(&mut l) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(l.trim_end_matches('\n').to_string()),
        }
    }

    /// The next line that is no `ping`.
    fn expect(&mut self) -> String {
        loop {
            let l = self.recv().expect("a line from the lobby");
            if !l.starts_with("ping ") {
                return l;
            }
        }
    }

    fn hello(&mut self, name: &str, token: &str) -> String {
        self.send(&format!(
            "hello name={name} token={token} version=abc1234 ports=27888-27899 running=0"
        ));
        self.expect()
    }
}

// ------------------------------------------------------------------ lobby clients

struct Client {
    sid: Sid,
    rx: Receiver<Outgoing>,
    got: Vec<(u16, Vec<u8>)>,
}

impl Client {
    fn send(&mut self, st: &mut Core, id: u16, body: &[u8]) {
        let mut w = PktWriter::new(id);
        w.bytes(body);
        let p = w.into_payload();
        st.scope(|st| route(st, self.sid, id as i32, &p)).unwrap();
        self.got.extend(drain(&self.rx));
    }

    fn take(&mut self, id: u16) -> Vec<u8> {
        self.got.extend(drain(&self.rx));
        let i = self
            .got
            .iter()
            .position(|(f, _)| *f == id)
            .unwrap_or_else(|| panic!("no 0x{id:x}"));
        let d = self.got[i].1.clone();
        self.got.drain(..=i);
        d
    }

    fn has(&mut self, id: u16) -> bool {
        self.got.extend(drain(&self.rx));
        self.got.iter().any(|(f, _)| *f == id)
    }
}

fn wstr(s: &str, n: usize) -> Vec<u8> {
    let mut w = PktWriter::empty();
    w.wstr(s, n);
    w.raw_bytes().to_vec()
}

fn u32s(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Auth login (+ character) and the pick of server `server`, then the lobby login with its key.
fn login(st: &mut Core, sid: Sid, login: &str, server: u32) -> (Client, u32) {
    let mut a = Client {
        sid,
        rx: connect(st, sid, Listener::Auth),
        got: Vec::new(),
    };
    let mut body = wstr(&format!("LOCAL:{login}"), 4096);
    body.extend(wstr("pw", 17));
    body.extend(wstr("00-11", 20));
    body.extend(0i32.to_le_bytes());
    a.send(st, LOGIN_REQUEST, &body);
    let r = a.take(LOGIN_REPLY);
    let account = u32_at(&r, 0x12);
    if u32_at(&r, 6) == 0 {
        let mut b = 6002i32.to_le_bytes().to_vec();
        b.extend(wstr(login, 17));
        a.send(st, NEW_CHAR, &b);
        assert_eq!(u32_at(&a.take(NEW_CHAR_REPLY), 6), 1);
    }
    a.send(st, SERVER_PICK, &u32s(&[server]));
    let r = a.take(SERVER_PICK_REPLY);
    assert_eq!(u32_at(&r, 6), 1, "server {server} picked");
    let mut key = r[0xe..0x1e].to_vec();
    st.sessions.remove(&sid);
    let mut l = Client {
        sid: sid + 1,
        rx: connect(st, sid + 1, Listener::Lobby),
        got: Vec::new(),
    };
    key.extend(account.to_le_bytes());
    l.send(st, LOBBY_LOGIN, &key);
    assert_eq!(u32_at(&l.take(LOBBY_LOGIN_REPLY), 6), 1);
    (l, account)
}

fn enter_channel(st: &mut Core, c: &mut Client, account: u32, ch: u32) -> bool {
    let mut b = u32s(&[account, ch]);
    b.extend(wstr("Channel", 32));
    c.send(st, CHANNEL_ENTER, &b);
    u32_at(&c.take(CHANNEL_ENTER_REPLY), 6) == 1
}

fn open_room(st: &mut Core, c: &mut Client) -> u32 {
    let mut b = 8i32.to_le_bytes().to_vec();
    b.extend(wstr("Room", 33));
    b.extend(wstr("", 9));
    b.extend(0i32.to_le_bytes());
    c.send(st, ROOM_CREATE, &b);
    let r = c.take(ROOM_CREATE_REPLY);
    assert_eq!(u32_at(&r, 6), 1);
    u32_at(&r, 0xe)
}

/// The countdown of a start, driven by hand (the deferred thread is not run).
fn countdown(st: &mut Core, key: RoomKey) {
    let gen = st
        .take_deferred()
        .iter()
        .find_map(|x| match x {
            Deferred::Countdown { room, gen } if *room == key => Some(*gen),
            _ => None,
        })
        .expect("countdown queued");
    for i in 0..st.rules.countdown_ticks {
        st.scope(|st| {
            assert!(game::tick(st, key, gen, i));
            Ok(())
        })
        .unwrap();
    }
    st.scope(|st| {
        game::countdown_done(st, key, gen);
        Ok(())
    })
    .unwrap();
}

// ------------------------------------------------------------------ server list and channel blocks (CD-21 amended)

#[test]
fn the_server_list_follows_lobby_servers_and_the_clan_entry_moves_behind_them() {
    // no entry: today's list (one entry + the clan entry id 2)
    let mut st = state();
    let e = crate::auth::server_entries(&st);
    assert_eq!(
        e.iter().map(|e| (e.id, e.clan_only)).collect::<Vec<_>>(),
        vec![(1, false), (2, true)]
    );
    assert_eq!(e[1].name, format!("{} Clan", st.opts.server_name));
    // one entry: the clan entry keeps id 2
    st.opts.servers = vec![entry("Solo", "")];
    let e = crate::auth::server_entries(&st);
    assert_eq!(
        e.iter().map(|e| (e.id, e.clan_only)).collect::<Vec<_>>(),
        vec![(1, false), (2, true)]
    );
    // two entries: ids 1 and 2, the clan entry 3; --server-max stays the first entry's capacity
    two_regions(&mut st);
    st.opts.server_max = 777;
    let e = crate::auth::server_entries(&st);
    assert_eq!(
        e.iter()
            .map(|e| (e.id, e.name.as_str(), e.capacity, e.clan_only))
            .collect::<Vec<_>>(),
        vec![
            (1, "US Central", 777, false),
            (2, "US West", 500, false),
            (3, "US Central Clan", 777, true)
        ]
    );
    assert_eq!(e[0].port, e[1].port, "both entries are this lobby");
    assert_eq!(e[2].port, st.opts.clanport as u32);
    assert_eq!(crate::servers::host_of_channel(&st, 5), "us-west");
    assert_eq!(crate::servers::host_of_channel(&st, 2), "");
    assert_eq!(
        crate::servers::host_of_channel(&st, crate::clan::CLAN_CHANNEL_BASE + 1),
        "",
        "the clan lobby's rooms play here"
    );
}

#[test]
fn each_entry_sees_its_own_channel_block_and_cannot_enter_another() {
    let mut st = state();
    two_regions(&mut st);
    let (mut west, wa) = login(&mut st, 1, "westy", 2);
    let (mut central, ca) = login(&mut st, 11, "centra", 1);
    let ids = |st: &Core, sid: Sid| {
        crate::lobby::channels_for(st, sid)
            .iter()
            .map(|c| (c.id, c.name.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(&st, west.sid),
        vec![
            (4, "Channel 1".to_string()),
            (5, "Channel 2".to_string()),
            (6, "Channel 3".to_string())
        ]
    );
    assert_eq!(
        ids(&st, central.sid)
            .iter()
            .map(|c| c.0)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert!(
        !enter_channel(&mut st, &mut west, wa, 1),
        "US West cannot enter channel 1"
    );
    assert!(enter_channel(&mut st, &mut west, wa, 4));
    assert!(!enter_channel(&mut st, &mut central, ca, 4));
    assert!(enter_channel(&mut st, &mut central, ca, 3));
    // the live counts follow the picked entry; the 0x377 reply carries the block
    let e = crate::auth::server_entries(&st);
    assert_eq!((e[0].current, e[1].current), (1, 1));
    west.send(&mut st, CHANNEL_LIST, &[]);
    let r = west.take(CHANNEL_LIST_REPLY);
    assert_eq!(
        r,
        crate::wire::channel_list_reply(&crate::lobby::channels_for(&st, west.sid))[8..].to_vec()
    );
    // the clan entry id 3 is a valid pick
    assert!(crate::auth::server_entries(&st)
        .iter()
        .any(|e| e.id == 3 && e.clan_only));
}

// ------------------------------------------------------------------ parsing

#[test]
fn hello_and_agent_lines_parse_and_tokens_compare() {
    let h =
        parse_hello("hello name=us-west token=T version=abc ports=27888-27899 running=2").unwrap();
    assert_eq!(
        (
            h.name.as_str(),
            h.token.as_str(),
            h.version.as_str(),
            h.ports,
            h.running
        ),
        ("us-west", "T", "abc", Some((27888, 27899)), 2)
    );
    assert!(parse_hello("hello token=T").is_none(), "no name");
    assert!(parse_hello("hi name=x").is_none());
    assert_eq!(
        parse_line("started 7 port=27890 pid=42"),
        AgentLine::Started {
            gid: 7,
            port: 27890,
            pid: 42
        }
    );
    assert_eq!(
        parse_line("event 7 @player uid=3 kills=1 name=A b"),
        AgentLine::Event {
            gid: 7,
            text: "@player uid=3 kills=1 name=A b".into()
        }
    );
    assert_eq!(
        parse_line("event 7 "),
        AgentLine::Event {
            gid: 7,
            text: String::new()
        }
    );
    assert_eq!(
        parse_line("exited 7 code=-1"),
        AgentLine::Exited { gid: 7, code: -1 }
    );
    assert_eq!(
        parse_line("failed 7 no free port"),
        AgentLine::Failed {
            gid: 7,
            reason: "no free port".into()
        }
    );
    assert_eq!(
        parse_line("running 3 port=27891 pid=9"),
        AgentLine::Running {
            gid: 3,
            port: 27891,
            pid: 9
        }
    );
    assert_eq!(parse_line("ping 4"), AgentLine::Ping("4".into()));
    assert!(matches!(
        parse_line("started x port=1"),
        AgentLine::Unknown(_)
    ));
    assert!(matches!(parse_line("started 1"), AgentLine::Unknown(_)));
    assert!(token_eq(TOKEN, TOKEN));
    assert!(!token_eq("0123456789abcdef01234568", TOKEN));
    assert!(!token_eq("0123456789abcdef0123456", TOKEN), "a prefix");
    assert!(!token_eq("", TOKEN));
    assert!(!token_eq(&format!("{TOKEN}x"), TOKEN));
}

#[test]
fn the_remote_start_request_and_its_arguments() {
    assert_eq!(
        game::remote_args("-bindip 0.0.0.0 -tickhz 30 -PORT 5 -open"),
        vec!["-tickhz", "30", "-open"],
        "the agent's own flags are its own"
    );
    let j = game::start_json(
        "Worlds\\a",
        "ca",
        60,
        "dserver_c4_r1_g9.log",
        &["-tickhz".into(), "30".into()],
        None,
        None,
    );
    let v = dc_mod_lua::JsonValue::parse(&j).unwrap();
    assert_eq!(v.get("port").and_then(|p| p.as_i64()), Some(0));
    assert_eq!(v.get("world").and_then(|p| p.as_str()), Some("Worlds\\a"));
    assert_eq!(v.get("idle").and_then(|p| p.as_i64()), Some(60));
    assert_eq!(
        v.get("args").and_then(|a| a.as_array()).map(|a| a.len()),
        Some(2)
    );
    assert!(!j.contains('\n'), "one line");
}

/// MAP-83 / MAP-84: `packs=1` in the hello; `have` says what the agent holds, and `failed <gid> pack <sha>: ...` takes
/// it back, so the next start sends the pack again.
#[test]
fn an_agent_says_which_map_packs_it_holds_and_a_failed_pack_is_forgotten() {
    let hello = |tail: &str| parse_hello(&format!("hello name=a token=t {tail}")).unwrap();
    assert!(hello("content=1 packs=1").takes_packs);
    assert!(!hello("content=1").takes_packs);
    assert!(!hello("packs=0").takes_packs);
    let sha = "ab".repeat(32);
    assert_eq!(
        parse_line(&format!("have {sha}")),
        AgentLine::Have(sha.clone())
    );
    let shared = shared_with(|_| {});
    let mut a = dial(&shared);
    a.send(&format!(
        "hello name=us-west token={TOKEN} version=v ports=27888-27899 running=0 content=1 packs=1"
    ));
    assert!(a.expect().starts_with("ok "));
    a.send(&format!("have {sha}"));
    let holds = |c: &Core| {
        c.hosts
            .online
            .get("us-west")
            .is_some_and(|x| x.takes_packs && x.packs.contains(&sha))
    };
    wait_until(&shared, "the have line", holds);
    a.send(&format!(
        "failed 99 pack {sha}: this agent does not hold it"
    ));
    wait_until(&shared, "the failed line", |c| {
        c.hosts
            .online
            .get("us-west")
            .is_some_and(|x| x.packs.is_empty())
    });
}

// ------------------------------------------------------------------ the registry

#[test]
fn a_known_agent_is_accepted_and_bad_hellos_are_refused_after_a_delay() {
    let shared = shared_with(|_| {});
    let mut a = dial(&shared);
    let ok = a.hello("us-west", TOKEN);
    assert!(ok.starts_with("ok name=us-west lobby="), "{ok}");
    {
        let g = lock(&shared);
        let ag = &g.hosts.online["us-west"];
        assert_eq!(ag.address, "203.0.113.9", "the configured public address");
        assert_eq!(ag.ports, Some((27888, 27899)));
    }
    // a second connection of the same name
    let t = Instant::now();
    let mut b = dial(&shared);
    assert!(b
        .hello("us-west", TOKEN)
        .starts_with("refuse already connected"));
    assert!(
        t.elapsed() >= Duration::from_millis(50),
        "refused after the delay"
    );
    assert_eq!(b.recv(), None, "and closed");
    // a wrong token, an unknown name, no hello at all
    let mut c = dial(&shared);
    assert_eq!(
        c.hello("us-west", "0123456789abcdef0123456X"),
        "refuse bad token"
    );
    let mut d = dial(&shared);
    assert_eq!(d.hello("nobody", TOKEN), "refuse unknown name");
    let mut e = dial(&shared);
    e.send("GET / HTTP/1.1");
    assert_eq!(e.expect(), "refuse no hello");
    // the first connection is untouched
    a.send("ping 9");
    assert_eq!(a.expect(), "pong 9");
    assert!(lock(&shared).hosts.online.contains_key("us-west"));
}

#[test]
fn an_agent_is_offline_when_its_stream_ends_or_stays_silent() {
    let shared = shared_with(|st| st.opts.host_agents = vec![agent("us-west", "")]);
    let mut a = dial(&shared);
    assert!(a.hello("us-west", TOKEN).starts_with("ok "));
    assert_eq!(
        lock(&shared).hosts.online["us-west"].address,
        "127.0.0.1",
        "no public_address: the peer address"
    );
    drop(a);
    wait_until(&shared, "offline after close", |st| {
        st.hosts.online.is_empty()
    });
    // a new connection of the name is accepted again; a silent one is closed
    lock(&shared).hosts.timing.silence = Duration::from_millis(300);
    let mut b = dial(&shared);
    assert!(b.hello("us-west", TOKEN).starts_with("ok "));
    assert_eq!(b.recv(), None, "closed after the silence");
    wait_until(&shared, "offline after silence", |st| {
        st.hosts.online.is_empty()
    });
}

#[test]
fn the_lobby_pings_a_quiet_agent() {
    let shared = shared_with(|st| st.hosts.timing.ping_every = Duration::from_millis(100));
    let mut a = dial(&shared);
    assert!(a.hello("us-west", TOKEN).starts_with("ok "));
    let l = a.recv().unwrap();
    assert!(l.starts_with("ping "), "{l}");
}

/// A remote game record of agent `agent` as the lobby keeps it while the agent is offline.
fn remote_record(id: u64, agent: &str, over: bool) -> game::Game {
    game::Game {
        id,
        room: RoomKey { channel: 4, no: 1 },
        port: 27890,
        map: 0,
        mode: 0,
        link: game::Link::Remote(game::RemoteLink {
            agent: agent.into(),
            address: "203.0.113.9".into(),
            port: Some(27890),
            pid: Some(1),
            tx: None,
            stdin_open: !over,
            alive: true,
        }),
        ready: true,
        over,
        players: Vec::new(),
        clients: Default::default(),
        left: Default::default(),
        result_done: false,
        entry_keys: Default::default(),
        seat_tickets: Default::default(),
        seat_told: Default::default(),
        reap_scheduled: false,
        tmp_workdir: None,
        hisec_keys: Default::default(),
        net: Default::default(),
    }
}

#[test]
fn a_reconnected_agent_reattaches_the_games_the_lobby_tracks_and_quits_the_rest() {
    let shared = shared_with(|st| {
        st.games.insert(5, remote_record(5, "us-west", false));
        st.games.insert(6, remote_record(6, "us-west", true));
    });
    let mut a = dial(&shared);
    a.send(&format!(
        "hello name=us-west token={TOKEN} version=abc ports=27888-27899 running=3"
    ));
    assert!(a.expect().starts_with("ok "));
    a.send("running 5 port=27891 pid=77");
    a.send("running 6 port=27892 pid=78");
    a.send("running 9 port=27893 pid=79");
    assert_eq!(
        a.expect(),
        "quit 6",
        "a quit lost while offline is said again"
    );
    assert_eq!(a.expect(), "quit 9", "a game the lobby does not know");
    {
        let g = lock(&shared);
        let r = g.games[&5].link.remote().unwrap();
        assert!(r.tx.is_some() && r.alive);
        assert_eq!((r.port, r.pid), (Some(27891), Some(77)));
    }
    // the reattached game's lines reach the agent again
    assert!(lock(&shared)
        .games
        .get_mut(&5)
        .unwrap()
        .write_line("con 1 2"));
    assert_eq!(a.expect(), "line 5 con 1 2");
    // offline again: the record stays, its lines are lost; a finished one is forgotten at its reap
    drop(a);
    wait_until(&shared, "offline", |st| st.hosts.online.is_empty());
    let mut g = lock(&shared);
    assert!(g.games[&5].link.remote().unwrap().tx.is_none());
    assert!(!g.games.get_mut(&5).unwrap().write_line("con 1 2"));
    crate::hosts::reap_orphan(&mut g, 5);
    assert!(
        g.games.contains_key(&5),
        "a running match waits for its agent"
    );
    g.games.get_mut(&5).unwrap().over = true;
    crate::hosts::reap_orphan(&mut g, 5);
    assert!(!g.games.contains_key(&5));
}

// ------------------------------------------------------------------ a match on a remote host (LG-301)

#[test]
fn a_room_of_the_remote_entry_plays_on_its_agent_and_reports_its_result() {
    let shared = shared_with(|st| {
        st.opts.dserver = true;
        st.opts.dserver_world = "Worlds\\Test".into();
    });
    let mut a = dial(&shared);
    assert!(a.hello("us-west", TOKEN).starts_with("ok "));
    let (mut c, acc, key, before) = {
        let mut g = lock(&shared);
        let (mut c, acc) = login(&mut g, 1, "wes", 2);
        assert!(enter_channel(&mut g, &mut c, acc, 4));
        let no = open_room(&mut g, &mut c);
        let key = RoomKey { channel: 4, no };
        c.send(&mut g, ROOM_USER_FLAG, &u32s(&[0, 2]));
        assert_eq!(u32_at(&c.take(ROOM_USER_FLAG_REPLY), 6), 1, "start ok");
        countdown(&mut g, key);
        let before = crate::db::character(&g.db, acc).unwrap().unwrap();
        (c, acc, key, before)
    };
    // the agent gets the start, the room line and the player
    let start = a.expect();
    let (gid, json) = {
        let rest = start.strip_prefix("start ").expect("start first");
        let (g, j) = rest.split_once(' ').unwrap();
        (g.parse::<u64>().unwrap(), j.to_string())
    };
    let v = dc_mod_lua::JsonValue::parse(&json).unwrap();
    assert_eq!(
        v.get("world").and_then(|w| w.as_str()),
        Some("Worlds\\Test")
    );
    assert_eq!(v.get("port").and_then(|w| w.as_i64()), Some(0));
    assert!(a.expect().starts_with(&format!("line {gid} room ")));
    assert!(a.expect().starts_with(&format!("line {gid} user {acc} ")));
    // started, then @ready: the address push carries the agent's address and port
    a.send(&format!("started {gid} port=27890 pid=4242"));
    a.send(&format!("event {gid} @ready port=27890 world=x"));
    wait_until(&shared, "@ready", |st| {
        st.games.get(&gid).map(|g| g.ready).unwrap_or(false)
    });
    {
        let g = lock(&shared);
        let p = c.take(MATCH_ADDRESS_PUSH);
        let ip = dc_wire::PktReader::new(&p).at(6).wstr(17);
        assert_eq!((ip.as_str(), u32_at(&p, 0x28)), ("203.0.113.9", 27890));
        assert_eq!(g.games[&gid].port, 27890);
    }
    // the match's lines run through the same result path as a local server's
    a.send(&format!("event {gid} @join 0 {acc} 1 0 wes"));
    a.send(&format!(
        "event {gid} @player uid={acc} kills=3 deaths=1 team=1 tk=0 hs=1 name=wes"
    ));
    a.send(&format!(
        "event {gid} @result mode=TDM win=1 alpha=5 bravo=2 limit=5 rounds=1:0"
    ));
    wait_until(&shared, "@result", |st| {
        st.games.get(&gid).map(|g| g.result_done).unwrap_or(false)
    });
    {
        let g = lock(&shared);
        let after = crate::db::character(&g.db, acc).unwrap().unwrap();
        assert_eq!(after.kills, before.kills + 3);
        assert!(after.exp > before.exp && after.gp > before.gp, "paid");
    }
    assert!(c.has(PROGRESS_PUSH), "0x3004 as for a local game");
    // @end closes the game's stdin (quit to the agent), the agent's exited ends the record
    a.send(&format!("event {gid} @end"));
    loop {
        let l = a.expect();
        if l == format!("quit {gid}") {
            break;
        }
        assert!(l.starts_with(&format!("line {gid} ")), "{l}");
    }
    a.send(&format!("exited {gid} code=0"));
    wait_until(&shared, "the record to go", |st| {
        !st.games.contains_key(&gid)
    });
    let g = lock(&shared);
    assert_eq!(g.rooms[&key].state, RoomPhase::Waiting);
}

#[test]
fn a_start_on_an_offline_host_is_refused_like_a_failed_local_start() {
    let mut st = state();
    two_regions(&mut st);
    st.opts.dserver = true;
    st.opts.dserver_world = "Worlds\\Test".into();
    let (mut c, acc) = login(&mut st, 1, "wes", 2);
    assert!(enter_channel(&mut st, &mut c, acc, 4));
    let no = open_room(&mut st, &mut c);
    c.send(&mut st, ROOM_USER_FLAG, &u32s(&[0, 2]));
    let r = c.take(ROOM_USER_FLAG_REPLY);
    assert_eq!(
        (u32_at(&r, 6), u16::from_le_bytes([r[0xa], r[0xb]])),
        (0, crate::wire::REASON_JOIN_FAILED)
    );
    assert_eq!(
        st.rooms[&RoomKey { channel: 4, no }].state,
        RoomPhase::Waiting
    );
    assert!(st.games.is_empty());
}

/// LG-327: the seat ticket goes to the agent inside the `user` line and to the player right before the address; a
/// rejoin of the same match keeps it; the agent's `@seat_refused` puts one line into the player's chat.
#[test]
fn a_remote_match_carries_the_seat_ticket_to_the_agent_and_the_player() {
    let shared = shared_with(|st| {
        st.opts.dserver = true;
        st.opts.dserver_world = "Worlds\\Test".into();
    });
    let mut a = dial(&shared);
    assert!(a.hello("us-west", TOKEN).starts_with("ok "));
    let (mut c, mut d, acc, guest) = {
        let mut g = lock(&shared);
        let (mut c, acc) = login(&mut g, 1, "wes", 2);
        assert!(enter_channel(&mut g, &mut c, acc, 4));
        let no = open_room(&mut g, &mut c);
        let key = RoomKey { channel: 4, no };
        let (mut d, guest) = login(&mut g, 3, "rue", 2);
        assert!(enter_channel(&mut g, &mut d, guest, 4));
        let mut b = (no as i32).to_le_bytes().to_vec();
        b.extend(wstr("", 9));
        b.extend(wstr("", 65));
        d.send(&mut g, ROOM_ENTER, &b);
        assert_eq!(u32_at(&d.take(ROOM_ENTER_REPLY), 6), 1);
        d.send(&mut g, ROOM_USER_FLAG, &u32s(&[0, 1]));
        assert_eq!(u32_at(&d.take(ROOM_USER_FLAG_REPLY), 6), 1);
        c.send(&mut g, ROOM_USER_FLAG, &u32s(&[0, 2]));
        assert_eq!(u32_at(&c.take(ROOM_USER_FLAG_REPLY), 6), 1, "start ok");
        countdown(&mut g, key);
        (c, d, acc, guest)
    };
    let start = a.expect();
    let gid: u64 = start["start ".len()..]
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(a.expect().starts_with(&format!("line {gid} room ")));
    let ticket_of = |line: &str, who: u32| -> String {
        assert!(
            line.starts_with(&format!("line {gid} user {who} ")),
            "{line}"
        );
        line.split(' ')
            .find_map(|t| t.strip_prefix("ticket="))
            .expect("a ticket token")
            .to_string()
    };
    let mut lines = [a.expect(), a.expect()];
    lines.sort_by_key(|l| !l.contains(&format!(" user {acc} ")));
    let (host_ticket, guest_ticket) = (ticket_of(&lines[0], acc), ticket_of(&lines[1], guest));
    assert_eq!(host_ticket.len(), 32);
    assert_ne!(host_ticket, guest_ticket);
    a.send(&format!("started {gid} port=27890 pid=4242"));
    a.send(&format!("event {gid} @ready port=27890 world=x"));
    wait_until(&shared, "@ready", |st| {
        st.games.get(&gid).map(|g| g.ready).unwrap_or(false)
    });
    let hex_of =
        |f: Vec<u8>| dc_wire::seatticket::to_hex(&dc_wire::seatticket::parse_push(&f).unwrap());
    assert_eq!(hex_of(c.take(dc_wire::seatticket::PUSH)), host_ticket);
    assert_eq!(hex_of(d.take(dc_wire::seatticket::PUSH)), guest_ticket);
    assert!(c.has(MATCH_ADDRESS_PUSH));
    assert!(d.has(MATCH_ADDRESS_PUSH));
    d.got.clear();
    // the guest leaves the match and rejoins it: the same ticket
    {
        let mut g = lock(&shared);
        d.send(&mut g, MATCH_EXIT, &u32s(&[0]));
        d.send(&mut g, ROOM_USER_FLAG, &u32s(&[0, 1]));
    }
    let again = loop {
        let l = a.expect();
        if l.contains(" user ") {
            break l;
        }
    };
    assert_eq!(ticket_of(&again, guest), guest_ticket);
    assert_eq!(hex_of(d.take(dc_wire::seatticket::PUSH)), guest_ticket);
    d.got.clear();
    // the refusal of a client without a ticket: one chat line per match
    for _ in 0..2 {
        a.send(&format!("event {gid} @seat_refused {guest} noticket"));
    }
    wait_until(&shared, "the refusal", |st| {
        st.games
            .get(&gid)
            .is_some_and(|g| g.seat_told.contains(&guest))
    });
    std::thread::sleep(Duration::from_millis(200));
    d.got.extend(drain(&d.rx));
    assert!(
        !d.got.iter().any(|f| f.0 == CHAT_PUSH),
        "not while the game screen is up: the client drops what comes then"
    );
    // back in the room: location 1, room info, room users
    {
        let mut g = lock(&shared);
        d.send(&mut g, LOCATION, &u32s(&[1]));
        d.send(&mut g, ROOM_INFO, &[]);
    }
    d.got.extend(drain(&d.rx));
    let chats: Vec<Vec<u8>> = d
        .got
        .iter()
        .filter(|f| f.0 == CHAT_PUSH)
        .map(|f| f.1.clone())
        .collect();
    assert_eq!(chats.len(), 1, "once");
    let text: Vec<u16> = chats[0][0x38..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .take_while(|u| *u != 0)
        .collect();
    assert_eq!(String::from_utf16_lossy(&text), game::SEAT_TEXT_OLD_CLIENT);
}

#[test]
fn a_failed_remote_start_puts_the_room_back_to_waiting() {
    let shared = shared_with(|st| {
        st.opts.dserver = true;
        st.opts.dserver_world = "Worlds\\Test".into();
    });
    let mut a = dial(&shared);
    assert!(a.hello("us-west", TOKEN).starts_with("ok "));
    let key = {
        let mut g = lock(&shared);
        let (mut c, acc) = login(&mut g, 1, "wes", 2);
        assert!(enter_channel(&mut g, &mut c, acc, 4));
        let no = open_room(&mut g, &mut c);
        c.send(&mut g, ROOM_USER_FLAG, &u32s(&[0, 2]));
        let _ = g.take_deferred();
        RoomKey { channel: 4, no }
    };
    let gid: u64 = a.expect()["start ".len()..]
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    a.send(&format!("failed {gid} no free port"));
    wait_until(&shared, "the record to go", |st| {
        !st.games.contains_key(&gid)
    });
    assert_eq!(lock(&shared).rooms[&key].state, RoomPhase::Waiting);
}
