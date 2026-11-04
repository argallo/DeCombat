//! End to end: the real lobby binary supervises the real `dc-dserver` binary. Two scripted 2009-protocol TCP
//! clients log in, pick the server, enter channel 1, open / join a room, get ready and start; the lobby launches
//! the game server, forwards the room line and the roster, and relays the game-start packet (0x3043) with the game
//! port once the server is ready. Two synthetic UDP clients (`dc-dserver`'s own test client) then join the game
//! server under the players' nicks. When the server stops (the lobby's `quit` after both players are back in the
//! room, or the server's idle limit), the lobby processes `@player` / `@result` / `@end` and pushes 0x3004 with the
//! new experience and GP, which must equal the DeCombat rule over the ROM map factor. A stand-in server checks the
//! winner / loser split, the advisory reported numbers and exactly-once storage.
//! ROM-gated where the real server runs (it loads the room's world); nothing from the ROM is asserted by value
//! except through `dc-bute`.
// SPEC: DSERVER_PIPE.md §1-§5 (process model, launch, room / user lines, events, order); SPEC: PROTOCOL_2009.md §7
// HARNESS: lobby_harness::test_main_suite (lobby flow, 0x3043 port at +0x28), ::test_persist_restart
// (0x3004 +6 exp / +0xa GP, characters row, one match_log row)
// DC-DEFAULT: CD-03 / CD-04 (rewards), CD-54 (proposed: the lobby computes, reported numbers advisory)

mod support;
use dc_dserver::synth_client::{ClientState, SynthClient, SynthConfig};
use dc_net_engine::socket_reader::SocketReader;
use std::collections::BTreeMap;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};
use support::*;

const T: Duration = Duration::from_secs(20);

/// One synthetic UDP player.
struct UdpPlayer {
    sock: UdpSocket,
    rd: SocketReader,
    client: SynthClient,
}

impl UdpPlayer {
    fn new(nick: &str, ticket: Option<[u8; 16]>) -> UdpPlayer {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let rd = SocketReader::start(&sock, 8192).unwrap();
        let mut cfg = SynthConfig::named(nick);
        cfg.load_ms = 300;
        cfg.seat_ticket = ticket;
        UdpPlayer {
            sock,
            rd,
            client: SynthClient::new(cfg),
        }
    }
}

/// Drives every player against the server until `done` holds for all of them or the timeout passes.
fn drive(
    players: &mut [UdpPlayer],
    server: SocketAddr,
    clock: Instant,
    t: Duration,
    done: impl Fn(&SynthClient) -> bool,
) -> bool {
    let end = Instant::now() + t;
    while Instant::now() < end {
        for p in players.iter_mut() {
            let now = clock.elapsed().as_millis() as u32;
            for d in p.client.poll(now) {
                let _ = p.sock.send_to(&d, server);
            }
            while let Some(Ok((d, from))) = p.rd.next_within(Duration::from_millis(1)) {
                if from == server {
                    p.client
                        .handle_datagram(clock.elapsed().as_millis() as u32, &d);
                }
            }
        }
        if players.iter().all(|p| done(&p.client)) {
            return true;
        }
    }
    false
}

/// `k=v` tokens after the first token, as the server logs its room settings.
fn pairs(line: &str) -> BTreeMap<String, String> {
    line.split_whitespace()
        .skip(1)
        .filter_map(|t| t.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

struct Match {
    l: Lobby,
    rom: Rom,
    a: Client,
    b: Client,
    na: u32,
    nb: u32,
    map: u32,
    mode: u32,
    udp: Vec<UdpPlayer>,
    tickets: Vec<[u8; 16]>,
    server: SocketAddr,
    clock: Instant,
}

/// Lobby flow up to both synthetic players in the world. Returns None when the ROM is unavailable.
fn play_until_in_world(tag: &str, idle: Option<u32>, lobby_json: Option<&str>) -> Option<Match> {
    let rom = Rom::open()?;
    let l = Lobby::start(
        tag,
        LobbyOpts {
            dserver_exe: Some(dserver_exe()),
            dserver_idle: idle,
            lobby_json: lobby_json.map(String::from),
            ..Default::default()
        },
    );
    let (mut a, na) = player(&l, "alice", "Alice");
    let (mut b, nb) = player(&l, "bob", "Bob");
    let mode = 0u32;
    let map = rom.default_map(mode);
    let goal = rom.game_room.default_goal(mode as usize, false).max(0) as u32;
    let no = open_room(&mut a, "E2E room", mode);
    join_room(&mut b, no);
    ready(&mut b, 1);
    ready(&mut a, 2);

    // 0x3043 to both players with the game port, after the server's `@ready`
    // LG-327: each with its seat ticket right before it
    let mut tickets = Vec::new();
    for c in [&mut a, &mut b] {
        let t = c.recv_until(dc_wire::seatticket::PUSH, T);
        tickets.push(dc_wire::seatticket::parse_push(&t).expect("a seat ticket"));
        let (ip, port) = game_address(c, T);
        assert_eq!(
            (ip.as_str(), port),
            ("127.0.0.1", l.gameport as u32),
            "{}",
            c.name
        );
    }
    assert_ne!(tickets[0], tickets[1], "one ticket per player");
    assert!(
        l.log_has("@ready port="),
        "the address push follows @ready; log:\n{}",
        l.log_text()
    );

    // the lobby launched dc-dserver on the game port with the room's world
    let launch = l
        .log_text()
        .lines()
        .find(|x| x.contains(" server pid "))
        .map(String::from);
    let launch = launch.unwrap_or_else(|| panic!("no launch line; log:\n{}", l.log_text()));
    assert!(
        launch.contains(&format!(" port {} ", l.gameport)),
        "{launch}"
    );
    assert!(
        launch.ends_with(&format!(" world {}", rom.world(map, mode))),
        "{launch}"
    );
    assert_eq!(l.game_pids().len(), 1);

    // the room line and the roster reached the server (its own log echoes what it parsed)
    let want_room = format!(
        "dc-dserver: room settings: {:?}",
        pairs(&rom.room_line(map, mode, goal, 0))
    );
    let roster_ok = |d: &str| {
        d.contains(&want_room)
            && [(na, "Alice"), (nb, "Bob")].iter().all(|(n, nick)| {
                d.lines().any(|x| {
                    x.contains(&format!("roster: uid {n} team "))
                        && x.ends_with(&format!(" nick {nick}"))
                })
            })
    };
    assert!(
        wait_until(T, || roster_ok(&l.dserver_logs())),
        "want {want_room:?}; dserver log:\n{}",
        l.dserver_logs()
    );

    // both players join the game server over UDP under their nicks and reach the world
    let server: SocketAddr = format!("127.0.0.1:{}", l.gameport).parse().unwrap();
    let mut udp = vec![
        UdpPlayer::new("Alice", Some(tickets[0])),
        UdpPlayer::new("Bob", Some(tickets[1])),
    ];
    let clock = Instant::now();
    let ok = drive(&mut udp, server, clock, T, |c| {
        c.state() == ClientState::InWorld
    });
    assert!(
        ok,
        "states {:?}; dserver log:\n{}",
        udp.iter().map(|p| p.client.state()).collect::<Vec<_>>(),
        l.dserver_logs()
    );
    for n in [na, nb] {
        assert!(
            l.wait_log("@join client ", T) && l.log_has(&format!("uid Some({n})")),
            "log:\n{}",
            l.log_text()
        );
    }
    Some(Match {
        l,
        rom,
        a,
        b,
        na,
        nb,
        map,
        mode,
        udp,
        tickets,
        server,
        clock,
    })
}

/// After the server stopped: 0x3004 to both, the characters rows and one match_log row (a draw: slice 1 of the
/// server has no game rules, so every identified player has 0 kills and `win=0`).
fn check_draw_result(m: &mut Match) {
    let f = m.rom.factors(m.map, m.mode);
    let (e, g) = expected_reward(0, false, 0, f);
    for c in [&mut m.a, &mut m.b] {
        let d = c.recv_until(0x3004, T);
        assert_eq!(
            (u32_at(&d, 6) as i64, u32_at(&d, 0xa) as i64),
            (e, START_GP + g),
            "{}: 0x3004",
            c.name
        );
    }
    for n in [m.na, m.nb] {
        assert_eq!(
            m.l.character(n),
            (e, START_GP + g, 0, 0, 0, 0, 0, 1),
            "characters row of {n}"
        );
    }
    assert_eq!(m.l.match_rows(), 1);
    let (winner, players): (i64, i64) =
        m.l.db()
            .query_row("SELECT winner, players FROM match_log", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
    assert_eq!((winner, players), (0, 2));
    assert!(
        m.l.wait_log("server exited (exit code: 0)", T),
        "log:\n{}",
        m.l.log_text()
    );
}

#[test]
fn quit_after_both_players_return_stores_the_result() {
    let Some(mut m) = play_until_in_world("e2e-quit", None, None) else {
        println!("skipped: CA_ROM unset or room table unavailable");
        return;
    };
    // both players report back to the room: nobody is in the match, the lobby writes `quit`
    back_to_room(&mut m.a);
    back_to_room(&mut m.b);
    room_waiting(&mut m.a, T);
    // the server ends the match and says goodbye to the UDP clients
    let (server, clock) = (m.server, m.clock);
    let gone = drive(&mut m.udp, server, clock, T, |c| {
        matches!(c.state(), ClientState::Dropped(_))
    });
    assert!(
        gone,
        "{:?}",
        m.udp.iter().map(|p| p.client.state()).collect::<Vec<_>>()
    );
    check_draw_result(&mut m);
    let d = m.l.dserver_logs();
    let tail: Vec<&str> = d
        .lines()
        .filter_map(|x| {
            x.trim_start()
                .split_once(' ')
                .map(|(_, r)| r)
                .filter(|r| r.starts_with('@'))
        })
        .skip_while(|x| !x.starts_with("@player"))
        .collect();
    assert_eq!(
        tail.iter().filter(|x| x.starts_with("@player")).count(),
        2,
        "{tail:?}"
    );
    assert!(
        tail.iter()
            .any(|x| x.starts_with("@result ") && x.contains(" win=0 ")),
        "{tail:?}"
    );
    assert_eq!(tail.last(), Some(&"@end"), "{tail:?}");
}

#[test]
fn idle_server_ends_the_match_and_the_room_waits_again() {
    // the players quit before the round ends: paid only because the operator switched the leaver rule off
    let Some(mut m) = play_until_in_world(
        "e2e-idle",
        Some(2),
        Some(r#"{"rewards.pay_leavers": true}"#),
    ) else {
        println!("skipped: CA_ROM unset or room table unavailable");
        return;
    };
    // the players leave the game server; the lobby still has them in the match
    for p in m.udp.iter_mut() {
        p.client.quit();
    }
    let (server, clock) = (m.server, m.clock);
    let gone = drive(&mut m.udp, server, clock, T, |c| {
        matches!(c.state(), ClientState::Dropped(_))
    });
    assert!(gone);
    assert!(m.l.wait_log("@leave client", T), "log:\n{}", m.l.log_text());
    // after the idle limit the server reports, the lobby stores and pushes, then `@end` puts the room back
    check_draw_result(&mut m);
    room_waiting(&mut m.a, T);
    assert!(
        m.l.dserver_logs().contains("idle limit reached"),
        "dserver log:\n{}",
        m.l.dserver_logs()
    );
}

#[test]
fn players_who_quit_before_the_round_end_are_not_paid_by_default() {
    // DC-DEFAULT: REW-1 (project decision
    let Some(mut m) = play_until_in_world("e2e-leavers", Some(2), None) else {
        println!("skipped: CA_ROM unset or room table unavailable");
        return;
    };
    let before = [m.l.character(m.na), m.l.character(m.nb)];
    for p in m.udp.iter_mut() {
        p.client.quit();
    }
    let (server, clock) = (m.server, m.clock);
    let gone = drive(&mut m.udp, server, clock, T, |c| {
        matches!(c.state(), ClientState::Dropped(_))
    });
    assert!(gone);
    assert!(
        m.l.wait_log("@leave client", T),
        "log:
{}",
        m.l.log_text()
    );
    room_waiting(&mut m.a, T);
    assert!(
        m.l.wait_log("server exited (exit code: 0)", T),
        "log:
{}",
        m.l.log_text()
    );
    assert_eq!([m.l.character(m.na), m.l.character(m.nb)], before);
    assert_eq!(m.l.match_rows(), 1, "the match itself is still logged");
}

#[test]
fn stand_in_result_win_split_advisory_numbers_and_exactly_once() {
    // no ROM needed: with a ROM the lobby applies its map factor, which the expectation then uses too
    let rom = Rom::open();
    let l = Lobby::start(
        "e2e-fake",
        LobbyOpts {
            dserver_exe: Some(fake_dserver_exe()),
            dserver_world: Some("Worlds\\SnowValley".into()),
            ..Default::default()
        },
    );
    let (mut a, na) = player(&l, "alice", "Alice");
    let (mut b, nb) = player(&l, "bob", "Bob");
    let no = open_room(&mut a, "Stand-in room", 0);
    join_room(&mut b, no);
    ready(&mut b, 1);
    ready(&mut a, 2);
    for c in [&mut a, &mut b] {
        assert_eq!(game_address(c, T).1, l.gameport as u32);
    }
    let da = a.recv_until(0x3004, T);
    let db = b.recv_until(0x3004, T);
    room_waiting(&mut a, T);

    let rec = l.fake();
    assert_eq!(rec.len(), 1);
    let users = rec[0].users();
    assert_eq!(users.len(), 2, "{:?}", rec[0].stdin);
    let f = match &rom {
        Some(r) => r.factors(r.default_map(0), 0),
        None => (1.0, 1.0),
    };
    for (n, nick, d) in [(na, "Alice", &da), (nb, "Bob", &db)] {
        let u = users
            .iter()
            .find(|u| u[1] == n.to_string())
            .unwrap_or_else(|| panic!("no user line for {n}"));
        // user <uid> <sid> <key> <team 0|1> <exp> <nick>
        // then optional k=v tokens (LDO-1 proposed: loadout= / face=; CD-64: mod=1)
        assert!(
            u.len() >= 7 && u[7..].iter().all(|t| t.contains('=')),
            "{u:?}"
        );
        assert!(u[4] == "0" || u[4] == "1", "{u:?}");
        assert_eq!((u[5].as_str(), u[6].as_str()), ("0", nick));
        let alpha = u[4] == "0";
        // the stand-in reports exp=3 gp=1 for Alpha: advisory only (CD-54), the lobby pays the DeCombat rule
        let (e, g) = expected_reward(if alpha { 2 } else { 0 }, alpha, 0, f);
        assert_eq!(
            (u32_at(d, 6) as i64, u32_at(d, 0xa) as i64),
            (e, START_GP + g),
            "{nick}: 0x3004"
        );
        let want = if alpha {
            (e, START_GP + g, 2, 0, 1, 0, 1, 1)
        } else {
            (e, START_GP + g, 0, 2, 0, 1, 0, 1)
        };
        assert_eq!(l.character(n), want, "{nick}");
    }
    // two `@result` lines for one game: one stored row
    assert_eq!(l.match_rows(), 1);
    let (winner, alpha, bravo): (i64, i64, i64) = l
        .db()
        .query_row("SELECT winner, alpha, bravo FROM match_log", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!((winner, alpha, bravo), (1, 2, 0));
    assert!(
        !a.saw_within(0x3004, Duration::from_millis(500)),
        "a second 0x3004 for the duplicate @result"
    );
}

#[test]
fn a_server_that_cannot_load_its_world_puts_the_room_back() {
    // the real server prints an error line and `@end` before `@ready` and exits non-zero (DSERVER_PIPE.md §2
    // "Environment", §7): no game-start packet, the room waits again, nothing is stored
    let l = Lobby::start(
        "e2e-noworld",
        LobbyOpts {
            dserver_exe: Some(dserver_exe()),
            dserver_world: Some("Worlds\\DcNoSuchWorld".into()),
            ..Default::default()
        },
    );
    let (mut a, _) = player(&l, "alice", "Alice");
    open_room(&mut a, "No world", 0);
    ready(&mut a, 2);
    room_waiting(&mut a, T);
    assert!(
        l.wait_log("server exited (exit code: 2)", T),
        "log:\n{}",
        l.log_text()
    );
    assert!(l.log_has("game 1: @end"), "log:\n{}", l.log_text());
    assert!(!l.log_has("@ready"), "log:\n{}", l.log_text());
    assert!(
        !a.saw_within(0x3043, Duration::from_millis(500)),
        "no game-start packet without @ready"
    );
    assert_eq!(l.match_rows(), 0);
}

/// HS-6 / HS-13 end to end through the pipe: the stand-in server reports one HiSec pickup twice (same case key) and
/// one for an account outside the match; the lobby stores exactly one inbox row for the picker, pushes 0x3014 to
/// him and none to the other player, and the match still ends normally. No ROM: the operator names the product.
// DC-DEFAULT: HS-6 (proposed) delivery; HS-13 (proposed) the `@hisec` pipe line of dc-dserver
#[test]
fn stand_in_hisec_pickup_line_delivers_one_case_and_a_stale_push() {
    let l = Lobby::start(
        "e2e-hisec",
        LobbyOpts {
            dserver_exe: Some(fake_dserver_exe()),
            dserver_world: Some("Worlds\\SnowValley".into()),
            lobby_json: Some(r#"{"hisec.case_product": 4242}"#.into()),
            ..Default::default()
        },
    );
    l.set_fake_mode("hisec");
    let (mut a, na) = player(&l, "alice", "Alice");
    let (mut b, nb) = player(&l, "bob", "Bob");
    let no = open_room(&mut a, "HiSec room", 0);
    join_room(&mut b, no);
    ready(&mut b, 1);
    ready(&mut a, 2);
    for c in [&mut a, &mut b] {
        game_address(c, T);
    }
    // the first user line the stand-in got is the picker
    let rec = l.fake();
    assert_eq!(rec.len(), 1);
    let picker: u32 = rec[0].users()[0][1].parse().unwrap();
    let (pc, other) = if picker == na {
        (&mut a, &mut b)
    } else {
        assert_eq!(picker, nb);
        (&mut b, &mut a)
    };
    pc.recv_until(0x3014, T);
    pc.recv_until(0x3004, T);
    // keep every frame the other player got up to his 0x3004
    assert!(other.saw_within(0x3004, T));
    assert!(
        !other.got.iter().any(|(f, _)| *f == 0x3014),
        "0x3014 only to the picker"
    );
    let rows: Vec<(i64, i64)> = {
        let db = l.db();
        let mut q = db
            .prepare("SELECT user_no, product_id FROM inbox ORDER BY uproduct")
            .unwrap();
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(
        rows,
        vec![(picker as i64, 4242)],
        "one row for the one case"
    );
    assert!(l.log_has("already delivered") && l.log_has("not a player of this game"));
    assert_eq!(l.match_rows(), 1, "the match result is unaffected");
}

/// LG-327, over real sockets through the lobby and the real game server: a stranger with a player's name and no or
/// a wrong ticket is refused and the player stays in the world; the player's new link with its ticket is seated.
#[test]
fn a_seat_needs_its_ticket_and_a_stranger_does_not_move_the_player() {
    let Some(mut m) = play_until_in_world("seat_ticket", None, None) else {
        eprintln!("skipped: no ROM");
        return;
    };
    let joins = |l: &Lobby| l.log_text().matches("@join client ").count();
    let before = joins(&m.l);
    let mut wrong = m.tickets[0];
    wrong[0] ^= 1;
    let (server, clock) = (m.server, m.clock);
    // the players keep being driven along with the strangers
    let strangers = [
        UdpPlayer::new("Alice", None),
        UdpPlayer::new("Alice", Some(wrong)),
        UdpPlayer::new("Bob", Some(m.tickets[0])),
    ];
    m.udp.extend(strangers);
    let ended = |c: &SynthClient| {
        c.state() == ClientState::InWorld
            || matches!(c.state(), ClientState::Dropped(_) | ClientState::Failed(_))
    };
    let ok = drive(&mut m.udp, server, clock, T, ended);
    assert!(ok, "dserver log:\n{}", m.l.dserver_logs());
    for p in &m.udp[2..] {
        assert_eq!(
            p.client.state(),
            ClientState::Dropped(6),
            "dserver log:\n{}",
            m.l.dserver_logs()
        );
    }
    assert_eq!(m.udp[0].client.state(), ClientState::InWorld);
    assert_eq!(m.udp[1].client.state(), ClientState::InWorld);
    assert_eq!(joins(&m.l), before, "nobody new on a seat");
    // the player's reconnect from another socket
    m.udp.truncate(2);
    m.udp.push(UdpPlayer::new("Alice", Some(m.tickets[0])));
    let ok = drive(&mut m.udp[2..], server, clock, T, |c| {
        c.state() == ClientState::InWorld
    });
    assert!(ok, "dserver log:\n{}", m.l.dserver_logs());
    assert!(
        wait_until(T, || joins(&m.l) == before + 1),
        "log:\n{}",
        m.l.log_text()
    );
    assert!(m
        .l
        .dserver_logs()
        .contains("reconnected: closing the older link"));
}
