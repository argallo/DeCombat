//! LOC-10: `dc-client --local-match <spec.json>` as a process: the client starts `dc-dserver` (beside it in the
//! target folder) from the description, joins it headless on loopback with the player's ticket, and ends it when the
//! session ends. No ROM, no window: the made-up world of `dc_client::testkit`, `--duration` for a headless run.
// SPEC: DSERVER_PIPE.md §1, §4.1, §4.3

mod support;

use std::process::Command;
use std::time::Duration;

use support::*;

fn spec_file(dir: &std::path::Path, world: &std::path::Path, bots: u8) -> std::path::PathBuf {
    let spec = serde_json::json!({
        "world": {"file": {"path": world, "announce": "Worlds\\Synth.dat"}},
        "mode": {"key": "TDM", "goal": 50, "time_secs": 600},
        "player": {"name": "Tester"},
        "bots": {"count": bots},
        "idle_secs": 30
    });
    let p = dir.join("match.json");
    std::fs::write(&p, serde_json::to_vec_pretty(&spec).unwrap()).unwrap();
    p
}

#[test]
fn the_client_runs_its_own_server_joins_it_and_ends_it() {
    // the supervisor finds dc-dserver beside the client: make sure it is built
    let _ = dserver_exe();
    let dir = scratch("local-match");
    let world = synth_world_file(&dir);
    let spec = spec_file(&dir, &world, 0);
    let profile = dir.join("profile");
    let o = Command::new(env!("CARGO_BIN_EXE_dc-client"))
        .args(["--local-match"])
        .arg(&spec)
        .args(["--duration", "6", "--no-sound"])
        .env("DC_PROFILE_DIR", &profile)
        .env_remove("CA_ROM")
        .env_remove("DC_SEAT_TICKET")
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    let err = String::from_utf8_lossy(&o.stderr);
    let log_path = profile
        .join("logs")
        .join("local-match")
        .join("dc-dserver.log");
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        err.contains("dc-client: local match on 127.0.0.1:"),
        "stderr {err}\nstdout {out}"
    );
    assert!(
        log.contains("@join 0 1 1 0 Tester"),
        "the player was seated by its ticket: {log}\nstderr {err}"
    );
    // the server was stopped with the session: its log ends with the end line (written before it exits)
    let end = std::time::Instant::now() + Duration::from_secs(10);
    let mut log = log;
    while !log.contains("@end") && std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(100));
        log = std::fs::read_to_string(&log_path).unwrap_or_default();
    }
    assert!(log.contains("@end"), "the server ended: {log}");
    let ticketless = log.lines().filter(|l| l.contains("ticket=")).count();
    assert_eq!(ticketless, 0, "no ticket in the server's log");
}

#[test]
fn local_match_refuses_other_connections_and_a_bad_description() {
    let dir = scratch("local-match-bad");
    let run = |args: &[&str]| {
        let o = Command::new(env!("CARGO_BIN_EXE_dc-client"))
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stderr).into_owned()
    };
    let spec = dir.join("match.json");
    std::fs::write(&spec, "{\"world\": 1}").unwrap();
    let s = spec.to_string_lossy().to_string();
    assert!(run(&["--local-match", &s, "--udp", "127.0.0.1:1"])
        .contains("--local-match runs its own server"));
    assert!(run(&["--local-match", &s]).contains("--local-match"));
    assert!(run(&["--help"]).contains("--local-match"));
}
