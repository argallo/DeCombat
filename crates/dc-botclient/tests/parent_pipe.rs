//! BOT-102: a bot started with `--parent-pipe` ends when its parent's pipe closes, so a killed `swarm` command leaves
//! no bot behind.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn a_bot_ends_when_the_pipe_closes() {
    // a join to a port nobody answers on keeps it waiting for a minute
    let mut c = Command::new(env!("CARGO_BIN_EXE_dc-botclient"))
        .args([
            "--parent-pipe",
            "--udp",
            "127.0.0.1:9",
            "--name",
            "pipebot1",
            "--no-rom",
            "--join-timeout",
            "60",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(800));
    assert!(c.try_wait().unwrap().is_none(), "still waiting to join");
    drop(c.stdin.take());
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        if c.try_wait().unwrap().is_some() {
            return;
        }
        if Instant::now() > end {
            let _ = c.kill();
            panic!("the bot outlived its pipe");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
