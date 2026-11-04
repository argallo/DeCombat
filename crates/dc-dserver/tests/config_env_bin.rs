//! The dc-dserver binary reads the `DC_*` environment layer: the lobby spawns it with the launcher's environment.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

/// DSV-168: `DC_ROOM__BLAST_DAMAGE_SCALE` in the process environment reaches the server (the binary used the
/// flag-only parse, so the launcher's grenade scale never applied). The world is missing on purpose: the config notes
/// are logged before the world loads, then the server stops.
// DC-DEFAULT: DSV-168 (proposed) the binary applies the DC_* environment layer; DSV-165 the knob
#[test]
fn blast_scale_from_the_environment_reaches_the_binary() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dc-dserver"))
        .args(["-port", "0", "-world", "no_such_world_dsv168.dat"])
        .env("DC_ROOM__BLAST_DAMAGE_SCALE", "2.5")
        .env_remove("DC_CONFIG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let out: Vec<String> = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map_while(Result::ok)
        .collect();
    let _ = child.wait();
    assert!(
        out.iter().any(|l| l.contains("blast damage x 2.5")),
        "{out:#?}"
    );
}
