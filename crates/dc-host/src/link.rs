//! The control connection (HOST_AGENT.md §3, §4): dial, TLS with the pin, `hello`, the `ok` / `refuse` answer,
//! then the line loop until the stream ends or stays silent for [`SILENCE`]; [`run`] reconnects with backoff.
//!
//! One thread drives the stream: the socket has a short read timeout, so the same loop reads the lobby's lines,
//! writes the lines the game servers' threads queued through the [`Hub`], and keeps the ping / silence timers.

use crate::config::HostConfig;
use crate::games::{Games, Hub};
use crate::log;
use crate::proto::{self, out, FromLobby, LineBuf, Piece};
use crate::tls;
use std::io::{ErrorKind, Read, Write};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// TCP connect and TLS handshake time-out.
// DC-DEFAULT: HA-1 10 s
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the lobby may take to answer the hello.
// DC-DEFAULT: HA-1 15 s
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(15);
/// A stream that brings nothing for this long is closed (HOST_AGENT.md §3).
// DC-DEFAULT: HA-1 60 s
pub const SILENCE: Duration = Duration::from_secs(60);
/// The agent's own `ping` after this long without sending anything, so the lobby's silence rule never fires.
// DC-DEFAULT: HA-1 20 s
pub const PING_EVERY: Duration = Duration::from_secs(20);
/// The reconnect backoff: 1, 2, 4 ... up to 30 s; back to 1 s after a connection that got `ok`.
// DC-DEFAULT: HA-1 1..30 s
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// The read time-out of the loop (how quickly queued game lines go out).
const TICK: Duration = Duration::from_millis(20);

/// How a session ended.
#[derive(Debug)]
pub enum End {
    /// Before `ok` (connect, TLS, refuse, no answer).
    Early(String),
    /// After `ok`.
    Late(String),
}

/// The next backoff after `cur`.
pub fn next_backoff(cur: Duration) -> Duration {
    (cur * 2).min(BACKOFF_MAX)
}

/// Connects and reconnects for ever.
pub fn run(cfg: &HostConfig, games: &Arc<Games>, hub: &Arc<Hub>) -> ! {
    let mut backoff = Duration::from_secs(1);
    loop {
        match session(cfg, games, hub) {
            End::Early(e) => {
                log!("lobby {}: {e}; again in {} s", cfg.lobby, backoff.as_secs());
                std::thread::sleep(backoff);
                backoff = next_backoff(backoff);
            }
            End::Late(e) => {
                backoff = Duration::from_secs(1);
                log!(
                    "lobby {}: connection ended: {e}; again in {} s",
                    cfg.lobby,
                    backoff.as_secs()
                );
                std::thread::sleep(backoff);
                backoff = next_backoff(backoff);
            }
        }
    }
}

/// Detaches the hub when the session ends, however it ends.
struct Attached<'a>(&'a Hub);

impl Drop for Attached<'_> {
    fn drop(&mut self) {
        self.0.detach();
    }
}

struct Stream {
    t: tls::Tls,
}

impl Stream {
    fn send(&mut self, line: &str) -> Result<(), String> {
        if line.len() > proto::MAX_LINE {
            log!(
                "a line of {} bytes to the lobby dropped (over 16 KiB)",
                line.len()
            );
            return Ok(());
        }
        let mut b = line.as_bytes().to_vec();
        b.push(b'\n');
        self.t
            .conn
            .writer()
            .write_all(&b)
            .map_err(|e| format!("write: {e}"))
    }

    fn flush(&mut self) -> Result<(), String> {
        while self.t.conn.wants_write() {
            self.t
                .conn
                .write_tls(&mut self.t.sock)
                .map_err(|e| format!("write: {e}"))?;
        }
        Ok(())
    }

    /// Reads what arrived within [`TICK`]: Ok(None) = nothing yet, Ok(Some(bytes)) = plaintext (empty = closed).
    fn read(&mut self, buf: &mut Vec<u8>) -> Result<bool, String> {
        match self.t.conn.read_tls(&mut self.t.sock) {
            Ok(0) => return Ok(false),
            Ok(_) => {
                self.t
                    .conn
                    .process_new_packets()
                    .map_err(|e| format!("TLS: {e}"))?;
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("read: {e}")),
        }
        let mut chunk = [0u8; 8192];
        loop {
            match self.t.conn.reader().read(&mut chunk) {
                Ok(0) => return Ok(false),
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(true),
                Err(e) => return Err(format!("read: {e}")),
            }
        }
    }

    fn close(&mut self) {
        self.t.conn.send_close_notify();
        let _ = self.flush();
        let _ = self.t.sock.shutdown(std::net::Shutdown::Both);
    }
}

/// One connection, from the dial to its end.
pub fn session(cfg: &HostConfig, games: &Arc<Games>, hub: &Arc<Hub>) -> End {
    let t = match tls::connect(&cfg.lobby, &cfg.fingerprint, CONNECT_TIMEOUT) {
        Ok(t) => t,
        Err(e) => return End::Early(e),
    };
    let _ = t.sock.set_read_timeout(Some(TICK));
    let _ = t.sock.set_write_timeout(Some(CONNECT_TIMEOUT));
    let mut s = Stream { t };
    // MAP-84: a pack stream the last connection cut off starts over; HA-2: a build transfer too
    games.packs.reset();
    games.incoming.reset();
    let hello = out::hello(
        &cfg.name,
        &cfg.token,
        crate::version(),
        cfg.port_lo,
        cfg.port_hi,
        games.running().len(),
    );
    if let Err(e) = s.send(&hello).and_then(|_| s.flush()) {
        return End::Early(e);
    }
    let started = Instant::now();
    let mut greeted = false;
    let (tx, rx) = mpsc::channel::<String>();
    let mut tx = Some(tx);
    let mut attached: Option<Attached> = None;
    let mut lines = LineBuf::default();
    let mut heard = Instant::now();
    let mut said = Instant::now();
    let mut ping_n: u64 = 0;
    let end = |greeted: bool, e: String| if greeted { End::Late(e) } else { End::Early(e) };
    loop {
        let mut data = Vec::new();
        let open = match s.read(&mut data) {
            Ok(o) => o,
            Err(e) => return end(greeted, e),
        };
        if !data.is_empty() {
            heard = Instant::now();
        }
        for p in lines.push(&data) {
            let l = match p {
                Piece::Line(l) => l,
                Piece::TooLong(n) => {
                    log!("a line of {n} bytes from the lobby dropped (over 16 KiB)");
                    continue;
                }
            };
            let msg = proto::parse(&l);
            if !greeted {
                match msg {
                    FromLobby::Ok(rest) => {
                        greeted = true;
                        log!("lobby {}: ok {rest}", cfg.lobby);
                        // attached before the list is taken: a server that ends meanwhile still reports `exited`
                        // (after its `running` line, which goes out first)
                        if let Some(tx) = tx.take() {
                            hub.attach(tx);
                            attached = Some(Attached(hub));
                        }
                        for (gid, port, pid) in games.running() {
                            if let Err(e) = s.send(&out::running(gid, port, pid)) {
                                return End::Late(e);
                            }
                        }
                        // MAP-84: what the lobby need not send again
                        for sha in games.packs.cached() {
                            if let Err(e) = s.send(&out::have(&sha)) {
                                return End::Late(e);
                            }
                        }
                    }
                    FromLobby::Refuse(why) => {
                        s.close();
                        return End::Early(format!("refused: {why}"));
                    }
                    _ => {
                        s.close();
                        return End::Early(format!("expected ok or refuse, got {l:?}"));
                    }
                }
                continue;
            }
            match msg {
                FromLobby::Start { gid, json } => {
                    let answer = games.start(gid, &json);
                    if let Err(e) = s.send(&answer) {
                        return End::Late(e);
                    }
                }
                FromLobby::Pack { sha, data } => {
                    if let Err(e) = games.packs.chunk(&sha, &data) {
                        log!("pack {sha}: {e}");
                    }
                }
                FromLobby::Line { gid, text } => {
                    if !games.line(gid, &text) {
                        log!("line for game {gid}: no such game server (or its stdin is closed)");
                    }
                }
                FromLobby::Quit { gid } => {
                    if games.quit(gid) {
                        log!("game {gid}: quit (stdin closed)");
                    } else {
                        log!("quit for game {gid}: no such game server");
                    }
                }
                FromLobby::Ping(n) => {
                    if let Err(e) = s.send(&out::pong(&n)) {
                        return End::Late(e);
                    }
                }
                FromLobby::Pong(_) => {}
                // HA-2: the lobby's server build; tools/host/update.sh installs it later
                FromLobby::Update(rest) => {
                    use crate::update::Decision;
                    match games.incoming.offer(&rest, !games.running().is_empty()) {
                        Decision::Fetch(line) => {
                            log!("lobby offers build {rest}: fetching it");
                            if let Err(e) = s.send(&line) {
                                return End::Late(e);
                            }
                        }
                        Decision::Refused(why) => log!("update offer refused: {why}"),
                        Decision::Have | Decision::Busy | Decision::Matches => {}
                    }
                }
                FromLobby::UpdPart { commit, data } => {
                    if let Err(e) = games.incoming.part_line(&commit, &data) {
                        log!("build {commit}: {e}");
                    }
                }
                FromLobby::UpdEnd(commit) => match games.incoming.end(&commit) {
                    Ok(o) => log!(
                        "build {commit}: {} ({} bytes) checked, left in {} for update.sh",
                        o.tarball,
                        o.size,
                        games.incoming.root().display()
                    ),
                    Err(e) => log!("build {commit}: {e}"),
                },
                FromLobby::UpdNo { commit, reason } => {
                    games.incoming.refused(&commit);
                    log!("build {commit}: the lobby did not send it: {reason}");
                }
                FromLobby::Ok(_)
                | FromLobby::Refuse(_)
                | FromLobby::Unknown(_)
                | FromLobby::Bad(_) => {
                    log!("lobby line ignored: {l:?}");
                }
            }
        }
        if !open {
            s.close();
            return end(greeted, "closed by the lobby".into());
        }
        if !greeted && started.elapsed() > HELLO_TIMEOUT {
            s.close();
            return End::Early("no answer to the hello".into());
        }
        if attached.is_some() {
            while let Ok(l) = rx.try_recv() {
                if let Err(e) = s.send(&l) {
                    return End::Late(e);
                }
            }
        }
        if greeted && heard.elapsed() > SILENCE {
            s.close();
            return End::Late(format!("nothing heard for {} s", SILENCE.as_secs()));
        }
        if greeted && said.elapsed() > PING_EVERY {
            ping_n += 1;
            if let Err(e) = s.send(&out::ping(ping_n)) {
                return End::Late(e);
            }
        }
        if s.t.conn.wants_write() {
            said = Instant::now();
        }
        if let Err(e) = s.flush() {
            return end(greeted, e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backoff_doubles_up_to_30_s() {
        let mut b = Duration::from_secs(1);
        let mut seen = vec![b.as_secs()];
        for _ in 0..6 {
            b = next_backoff(b);
            seen.push(b.as_secs());
        }
        assert_eq!(seen, [1, 2, 4, 8, 16, 30, 30]);
    }
}
