//! Lobby-side items of the DSERVER_PIPE addendum (lobby <-> dc-dserver end-to-end findings), against the real lobby
//! binary and the stand-in game server: A5 (reap after `@end` plus a grace period), B1 (`-idle` never negative),
//! B2 (`-bindip` for a non-loopback advertised address), C4 (the lobby's own per-room working directory is removed).
// SPEC: DSERVER_PIPE addendum e2e (A5, B1, B2, C4); SPEC: DSERVER_PIPE.md §2 (launch line), §3.4, §6
// HARNESS: stale_port.rs (same process-test style)

mod support;
use std::time::Duration;
use support::*;

const T: Duration = Duration::from_secs(20);

fn two_in_room(l: &Lobby) -> (Client, Client) {
    let (mut a, _) = player(l, "alice", "Alice");
    let (mut b, _) = player(l, "bob", "Bob");
    let no = open_room(&mut a, "Pipe room", 0);
    join_room(&mut b, no);
    (a, b)
}

fn start_match(a: &mut Client, b: &mut Client) -> (String, u32) {
    ready(b, 1);
    ready(a, 2);
    let r = game_address(a, T);
    let _ = game_address(b, T);
    r
}

fn fake_opts() -> LobbyOpts {
    LobbyOpts {
        dserver_exe: Some(fake_dserver_exe()),
        dserver_world: Some(r"Worlds\SnowValley".into()),
        ..Default::default()
    }
}

#[test]
fn a_lingering_server_is_reaped_after_end_plus_the_grace_period() {
    // SPEC: DSERVER_PIPE addendum e2e A5
    let game = free_udp_port();
    let l = Lobby::start(
        "pipe-reap",
        LobbyOpts {
            gameport: game,
            lobby_json: Some(r#"{"dserver.quit_grace_secs": 1}"#.into()),
            ..fake_opts()
        },
    );
    l.set_fake_mode("linger");
    let (mut a, mut b) = two_in_room(&l);
    start_match(&mut a, &mut b);
    a.recv_until(0x3004, T);
    room_waiting(&mut a, T);
    let pid = l.fake()[0].pid;
    // no later start needs the port: the reap alone must stop the server
    assert!(
        wait_until(T, || !pid_alive(pid)),
        "the lingering server was not reaped:\n{}",
        l.log_text()
    );
    assert!(wait_until(T, || udp_free(game)), "the game port stays held");
    assert!(
        l.wait_log("killed after the quit grace period", T),
        "log:\n{}",
        l.log_text()
    );
}

#[test]
fn a_negative_idle_is_passed_as_zero() {
    // SPEC: DSERVER_PIPE addendum e2e B1
    let l = Lobby::start(
        "pipe-idle",
        LobbyOpts {
            extra: vec!["--dserver-idle".into(), "-5".into()],
            ..fake_opts()
        },
    );
    let (mut a, mut b) = two_in_room(&l);
    start_match(&mut a, &mut b);
    let f = l.fake();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].arg("-idle"), Some("0"), "{:?}", f[0].args);
}

#[test]
fn a_non_loopback_gameip_is_passed_as_bindip() {
    // SPEC: DSERVER_PIPE addendum e2e B2
    let l = Lobby::start(
        "pipe-bindip",
        LobbyOpts {
            extra: vec!["--gameip".into(), "10.9.8.7".into()],
            ..fake_opts()
        },
    );
    let (mut a, mut b) = two_in_room(&l);
    let (ip, _) = start_match(&mut a, &mut b);
    assert_eq!(ip, "10.9.8.7");
    let f = l.fake();
    assert_eq!(f[0].arg("-bindip"), Some("10.9.8.7"), "{:?}", f[0].args);
}

#[test]
fn the_default_loopback_gameip_passes_no_bindip() {
    // SPEC: DSERVER_PIPE addendum e2e B2 (loopback is the server default)
    let l = Lobby::start("pipe-nobind", fake_opts());
    let (mut a, mut b) = two_in_room(&l);
    start_match(&mut a, &mut b);
    assert_eq!(l.fake()[0].arg("-bindip"), None);
}

#[test]
fn the_own_working_directory_is_removed_with_the_game() {
    // SPEC: DSERVER_PIPE addendum e2e C4
    let tmp = scratch_dir("pipe-tmp");
    let tmp_s = tmp.to_string_lossy().to_string();
    let l = Lobby::start(
        "pipe-workdir",
        LobbyOpts {
            no_logdir: true,
            env: vec![
                ("TEMP".into(), tmp_s.clone()),
                ("TMP".into(), tmp_s),
                ("TMPDIR".into(), tmp.to_string_lossy().to_string()),
            ],
            ..fake_opts()
        },
    );
    l.set_fake_mode("record");
    let (mut a, mut b) = two_in_room(&l);
    start_match(&mut a, &mut b);
    let own = |tmp: &std::path::Path| -> Vec<String> {
        std::fs::read_dir(tmp)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|n| n.starts_with("dc-lobby-dserver-"))
                    .collect()
            })
            .unwrap_or_default()
    };
    assert_eq!(
        own(&tmp).len(),
        1,
        "one per-room directory while the game runs: {:?}",
        own(&tmp)
    );
    assert_eq!(l.fake()[0].arg("-log"), None, "no logdir was given");
    back_to_room(&mut a);
    back_to_room(&mut b);
    room_waiting(&mut a, T);
    assert!(l.wait_log("server exited", T), "log:\n{}", l.log_text());
    assert!(
        wait_until(T, || own(&tmp).is_empty()),
        "directory not removed: {:?}",
        own(&tmp)
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
