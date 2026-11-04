//! The stale game-port suite against the real lobby binary: a foreign holder of the game port on 127.0.0.1 is
//! detected and skipped (the lobby picks a port of its range), and our own finished game server that still holds
//! the port is stopped and the port reused. Driven over the 2009 protocol with a stand-in game server, plus the
//! foreign-holder case with the real `dc-dserver` (ROM-gated: it loads the room's world).
// HARNESS: lobby_harness::test_stale_port (same two behaviours, black-box over the binary instead of the
// in-process port chooser); SPEC: DSERVER_PIPE.md §2 "Port choice", §6 (stale port: immediate)
// DC-DEFAULT: CD-10 (dedicated server ports from a configurable range, `dserver.port_range`)

mod support;
use std::net::UdpSocket;
use std::time::Duration;
use support::*;

const T: Duration = Duration::from_secs(15);

/// Host + guest in one room; returns (host, guest, room).
fn two_in_room(l: &Lobby) -> (Client, Client, u32) {
    let (mut a, _) = player(l, "alice", "Alice");
    let (mut b, _) = player(l, "bob", "Bob");
    let no = open_room(&mut a, "Stale room", 0);
    join_room(&mut b, no);
    (a, b, no)
}

fn start_match(a: &mut Client, b: &mut Client) -> (u32, u32) {
    ready(b, 1);
    ready(a, 2);
    let (ip_a, pa) = game_address(a, T);
    let (ip_b, pb) = game_address(b, T);
    assert_eq!((ip_a.as_str(), ip_b.as_str()), ("127.0.0.1", "127.0.0.1"));
    (pa, pb)
}

#[test]
fn a_foreign_holder_of_the_game_port_is_skipped() {
    let [held, range] = free_udp_ports(2)[..] else {
        unreachable!()
    };
    // a foreign holder bound on 127.0.0.1 (what a game server binds; a 0.0.0.0-only probe misses it on Windows)
    let holder = UdpSocket::bind(("127.0.0.1", held)).expect("hold the game port");
    let l = Lobby::start(
        "stale-foreign",
        LobbyOpts {
            dserver_exe: Some(fake_dserver_exe()),
            dserver_world: Some("Worlds\\SnowValley".into()),
            gameport: held,
            lobby_json: Some(format!(r#"{{"dserver.port_range": [{range}, {range}]}}"#)),
            ..Default::default()
        },
    );
    let (mut a, mut b, _) = two_in_room(&l);
    let (pa, pb) = start_match(&mut a, &mut b);
    assert_eq!(
        (pa, pb),
        (range as u32, range as u32),
        "the advertised port skips the held one"
    );
    let f = l.fake();
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(
        f[0].arg("-port"),
        Some(range.to_string().as_str()),
        "{:?}",
        f[0].args
    );
    assert!(
        !l.log_has("stale server stopped"),
        "a foreign holder is never stopped:\n{}",
        l.log_text()
    );
    // the foreign socket is untouched
    holder
        .send_to(b"x", ("127.0.0.1", held))
        .expect("holder still usable");
    // the stand-in finishes its match: the result is stored once
    a.recv_until(0x3004, T);
    assert_eq!(l.match_rows(), 1);
}

#[test]
fn our_own_finished_server_is_stopped_and_its_port_reused() {
    let game = free_udp_port();
    let l = Lobby::start(
        "stale-own",
        LobbyOpts {
            dserver_exe: Some(fake_dserver_exe()),
            dserver_world: Some("Worlds\\SnowValley".into()),
            gameport: game,
            // a long reap grace keeps the lingering server alive for this test (pipe_addendum.rs covers the reap)
            lobby_json: Some(r#"{"dserver.quit_grace_secs": 120}"#.into()),
            ..Default::default()
        },
    );
    l.set_fake_mode("linger");
    let (mut a, mut b, _) = two_in_room(&l);
    assert_eq!(start_match(&mut a, &mut b), (game as u32, game as u32));
    // the stand-in reports the match and `@end`, then keeps running with the port bound
    a.recv_until(0x3004, T);
    room_waiting(&mut a, T);
    let first = l.fake();
    assert_eq!(first.len(), 1);
    let stale = first[0].pid;
    assert!(pid_alive(stale), "the lingering server runs");
    assert!(!udp_free(game), "the lingering server holds the game port");

    // a second start of the same room: the stale child is stopped and the port reused
    l.set_fake_mode("normal");
    let (pa, pb) = start_match(&mut a, &mut b);
    assert_eq!((pa, pb), (game as u32, game as u32), "port reused");
    assert!(
        l.wait_log(&format!("stale server stopped to reuse port {game}"), T),
        "log:\n{}",
        l.log_text()
    );
    assert!(
        wait_until(T, || !pid_alive(stale)),
        "the stale server was stopped"
    );
    let all = l.fake();
    assert_eq!(all.len(), 2, "{all:?}");
    let second = all.iter().find(|x| x.pid != stale).unwrap();
    assert_eq!(second.arg("-port"), Some(game.to_string().as_str()));
    a.recv_until(0x3004, T);
    assert_eq!(l.match_rows(), 2);
}

#[test]
fn a_foreign_holder_is_skipped_with_the_real_dserver() {
    let Some(_rom) = Rom::open() else {
        println!("skipped: CA_ROM unset or room table unavailable");
        return;
    };
    let [held, range] = free_udp_ports(2)[..] else {
        unreachable!()
    };
    let _holder = UdpSocket::bind(("127.0.0.1", held)).expect("hold the game port");
    let l = Lobby::start(
        "stale-real",
        LobbyOpts {
            dserver_exe: Some(dserver_exe()),
            gameport: held,
            lobby_json: Some(format!(r#"{{"dserver.port_range": [{range}, {range}]}}"#)),
            ..Default::default()
        },
    );
    let (mut a, mut b, _) = two_in_room(&l);
    // the address push waits for `@ready`, which the real server prints once it bound the port it was given
    let (pa, pb) = start_match(&mut a, &mut b);
    assert_eq!((pa, pb), (range as u32, range as u32));
    assert!(
        l.wait_log(&format!("@ready port={range}"), T),
        "lobby log:\n{}\ndserver log:\n{}",
        l.log_text(),
        l.dserver_logs()
    );
    // nobody joins over UDP: both players report back, the lobby quits the server, the room waits again
    back_to_room(&mut a);
    back_to_room(&mut b);
    room_waiting(&mut a, T);
    assert!(l.wait_log("server exited", T), "log:\n{}", l.log_text());
    assert_eq!(l.match_rows(), 0, "no player identified: nothing stored");
}

/// Playtest report (LG-11): the capture tap relays the advertised game port on 127.0.0.1 to a server bound on
/// 127.0.0.2 (`-bindip`). The second start of the room must keep that port: the old probe on 127.0.0.1 saw the
/// relay as a holder, moved the second game server to a port of the range nothing relays, and the client's join
/// timed out.
#[test]
fn a_relay_of_the_advertised_port_does_not_move_the_second_match() {
    // macOS configures only 127.0.0.1 on lo0 (`sudo ifconfig lo0 alias 127.0.0.2` adds the second address)
    if UdpSocket::bind(("127.0.0.2", 0)).is_err() {
        println!("skipped: 127.0.0.2 is not an address of this host");
        return;
    }
    let [game, range] = free_udp_ports(2)[..] else {
        unreachable!()
    };
    let l = Lobby::start(
        "relay-second",
        LobbyOpts {
            dserver_exe: Some(fake_dserver_exe()),
            dserver_world: Some("Worlds\\SnowValley".into()),
            gameport: game,
            lobby_json: Some(format!(r#"{{"dserver.port_range": [{range}, {range}]}}"#)),
            extra: vec!["--dserver-args".into(), "-bindip 127.0.0.2".into()],
            ..Default::default()
        },
    );
    let (mut a, mut b, _) = two_in_room(&l);
    assert_eq!(start_match(&mut a, &mut b), (game as u32, game as u32));
    // the relay binds the advertised address once the server holds the port on 127.0.0.2
    let _relay = UdpSocket::bind(("127.0.0.1", game)).expect("relay on the advertised address");
    a.recv_until(0x3004, T);
    room_waiting(&mut a, T);
    assert!(l.wait_log("server exited", T), "log:\n{}", l.log_text());

    // match 2 of the same room: a second game server on the relayed port
    let (pa, pb) = start_match(&mut a, &mut b);
    assert_eq!(
        (pa, pb),
        (game as u32, game as u32),
        "the second match keeps the relayed port:\n{}",
        l.log_text()
    );
    let all = l.fake();
    assert_eq!(all.len(), 2, "{all:?}");
    assert!(all
        .iter()
        .all(|x| x.arg("-port") == Some(game.to_string().as_str())
            && x.arg("-bindip") == Some("127.0.0.2")));
    a.recv_until(0x3004, T);
    assert_eq!(l.match_rows(), 2);
}
