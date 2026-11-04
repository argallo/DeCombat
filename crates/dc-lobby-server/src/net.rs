//! TCP listeners (auth, lobby, clan lobby), one reader and one writer thread per connection, the dispatcher, and
//! the background threads (countdowns, game-server pipes, expiry scan). Every event locks the state once.

use crate::game;
use crate::logs::Listener;
use crate::msg::SERVER_PICK;
use crate::state::{Core, Deferred, Outgoing, Session, Shared, Sid};
use crate::{auth, lobby, room};
use std::io::{BufRead, BufReader};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, MutexGuard};
use std::time::Duration;

static NEXT_SID: AtomicU64 = AtomicU64::new(1);

/// Locks the state; a panic in another handler does not take the server down.
pub fn lock(shared: &Shared) -> MutexGuard<'_, Core> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

/// Routes one client message. C->S frames dispatch on the inner id for the two override frames.
// SPEC: PROTOCOL_2009.md §1.2 "Inner-id overrides" (frame 0x324 / 0x356)
pub fn route(st: &mut Core, sid: Sid, frame_id: i32, payload: &[u8]) -> crate::state::HResult {
    let id = dc_wire::effective_packet_id(frame_id, payload);
    // LG-318: a bot seat asks for its way into a match and nothing else
    if crate::botpool::message_refused(st, sid, id) {
        return Ok(());
    }
    game::seat_notice_on_message(st, sid, id);
    if crate::clan::dispatch(st, sid, id, payload)? {
        return Ok(());
    }
    if crate::shop::dispatch(st, sid, id, payload)? {
        return Ok(());
    }
    if crate::gm::dispatch(st, sid, id, payload)? {
        return Ok(());
    }
    if crate::notices::dispatch(st, sid, id, payload)? {
        return Ok(());
    }
    if crate::messenger::dispatch(st, sid, id, payload)? {
        return Ok(());
    }
    if auth::dispatch(st, sid, id, payload)? {
        return Ok(());
    }
    if lobby::dispatch(st, sid, id, payload)? {
        return Ok(());
    }
    if room::dispatch(st, sid, id, payload)? {
        return Ok(());
    }
    // other ids belong to later tasks (shop, inbox, clans, GM, messenger); answered with nothing
    st.note(&format!(
        "connection {}: message 0x{:x} not handled by the core server",
        sid, id
    ));
    Ok(())
}

/// Runs work queued by committed scopes.
pub fn run_deferred(shared: &Shared, st: &mut Core) {
    for d in st.take_deferred() {
        match d {
            Deferred::Countdown { room, gen } => {
                let interval = Duration::from_secs_f64(st.opts.countdown.clamp(0.0, 60.0));
                let ticks = st.rules.countdown_ticks;
                let sh = Arc::clone(shared);
                let _ = std::thread::Builder::new()
                    .name("countdown".into())
                    .spawn(move || {
                        for i in 0..ticks {
                            if i > 0 {
                                std::thread::sleep(interval);
                            }
                            let mut g = lock(&sh);
                            let mut live = true;
                            let r = g.scope(|st| {
                                live = game::tick(st, room, gen, i);
                                Ok(())
                            });
                            report(&g, r);
                            if !live {
                                return;
                            }
                        }
                        std::thread::sleep(interval);
                        let mut g = lock(&sh);
                        let r = g.scope(|st| {
                            game::countdown_done(st, room, gen);
                            Ok(())
                        });
                        report(&g, r);
                        run_deferred(&sh, &mut g);
                    });
            }
            Deferred::WatchGame { game: id } => watch_game(shared, st, id),
            Deferred::ReapGame { game: id, secs } => {
                let sh = Arc::clone(shared);
                let _ = std::thread::Builder::new()
                    .name("reap".into())
                    .spawn(move || {
                        std::thread::sleep(Duration::from_secs(secs));
                        let mut g = lock(&sh);
                        if let Some(c) = g.games.get_mut(&id).and_then(|x| x.link.child_mut()) {
                            if let Ok(None) = c.try_wait() {
                                let _ = c.kill();
                                g.note(&format!("game {}: killed after the quit grace period", id));
                            }
                        }
                        // LG-301: the agent kills a remote server itself; a record whose agent is gone is dropped
                        crate::hosts::reap_orphan(&mut g, id);
                    });
            }
        }
    }
}

pub(crate) fn report(st: &Core, r: crate::state::HResult) {
    if let Err(e) = r {
        st.note(&format!("error (rolled back): {}", e));
        // MON-5: counted for the error-spike alert
        crate::opsmon::fault(st, &format!("error (rolled back): {e}"));
    }
}

/// One stdout line of a game server enters the lobby: the local reader thread and a remote host's `event` line
/// both come through here (one scope, then the deferred work).
// DC-DEFAULT: LG-301 (proposed) one entry point for the pipe events of either link
pub fn game_line(shared: &Shared, g: &mut Core, id: u64, line: &str) {
    let r = g.scope(|st| game::on_line(st, id, line));
    report(g, r);
    run_deferred(shared, g);
}

/// Starts the stdout / stderr readers (a local child) and the `@ready` watchdog of a started game server.
fn watch_game(shared: &Shared, st: &mut Core, id: u64) {
    let Some(g) = st.games.get_mut(&id) else {
        return;
    };
    let (out, err) = g.link.take_output();
    let timeout = Duration::from_secs_f64(st.opts.dserver_timeout.clamp(0.1, 3600.0));
    if let Some(out) = out {
        let sh = Arc::clone(shared);
        let _ = std::thread::Builder::new()
            .name(format!("game{}-out", id))
            .spawn(move || {
                let mut rd = BufReader::new(out);
                let mut buf = Vec::new();
                loop {
                    buf.clear();
                    match rd.read_until(b'\n', &mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            // UTF-8 only; invalid sequences are replaced (SPEC: DSERVER_PIPE.md §7)
                            let line = String::from_utf8_lossy(&buf).to_string();
                            let mut g = lock(&sh);
                            game_line(&sh, &mut g, id, &line);
                        }
                    }
                }
                // EOF: crash unless the match already ended, then reap the process outside the lock
                let child = {
                    let mut g = lock(&sh);
                    let r = g.scope(|st| {
                        game::close_game(st, id, true);
                        Ok(())
                    });
                    report(&g, r);
                    run_deferred(&sh, &mut g);
                    g.games.get_mut(&id).and_then(|x| x.link.take_child())
                };
                let tmp = lock(&sh)
                    .games
                    .get_mut(&id)
                    .and_then(|x| x.tmp_workdir.take());
                let code =
                    child.map(|mut c| c.wait().map(exit_text).unwrap_or_else(|e| e.to_string()));
                // SPEC: DSERVER_PIPE addendum e2e C4 (the per-room working directory goes with the game)
                if let Some(d) = tmp {
                    let _ = std::fs::remove_dir_all(&d);
                }
                let mut g = lock(&sh);
                g.games.remove(&id);
                g.note(&format!(
                    "game {}: server exited ({})",
                    id,
                    code.unwrap_or_else(|| "already reaped".into())
                ));
            });
    }
    if let Some(err) = err {
        let sh = Arc::clone(shared);
        let _ = std::thread::Builder::new()
            .name(format!("game{}-err", id))
            .spawn(move || {
                let rd = BufReader::new(err);
                for line in rd.split(b'\n').map_while(Result::ok) {
                    let g = lock(&sh);
                    if g.opts.dserver_verbose {
                        g.note(&format!(
                            "game {}! {}",
                            id,
                            String::from_utf8_lossy(&line).trim_end()
                        ));
                    }
                }
            });
    }
    let sh = Arc::clone(shared);
    let _ = std::thread::Builder::new()
        .name(format!("game{}-ready", id))
        .spawn(move || {
            std::thread::sleep(timeout);
            let mut g = lock(&sh);
            let r = g.scope(|st| {
                game::ready_timeout(st, id);
                Ok(())
            });
            report(&g, r);
            run_deferred(&sh, &mut g);
        });
}

/// Takes a place for a new connection of `stream`'s peer (LG-326); `None`: the caller drops the connection. The
/// first refusal of a burst is logged, the rest would only fill the disk.
pub(crate) fn admit_conn(
    shared: &Shared,
    cap: &crate::limits::ConnCap,
    stream: &TcpStream,
    tag: &str,
    last_note: &mut Option<std::time::Instant>,
) -> Option<Option<crate::limits::ConnSlot>> {
    let Ok(peer) = stream.peer_addr() else {
        return None;
    };
    match cap.admit(peer.ip()) {
        Ok(slot) => Some(slot),
        Err(why) => {
            let now = std::time::Instant::now();
            if last_note.is_none_or(|t| now.duration_since(t) > Duration::from_secs(10)) {
                *last_note = Some(now);
                lock(shared).note(&format!(
                    "{tag} accept: connection from {} refused: too many open ({why:?}, LG-326)",
                    peer.ip()
                ));
            }
            None
        }
    }
}

/// After a failed accept (out of file descriptors, most likely): a pause instead of a loop that fills the log.
pub(crate) fn accept_failed(shared: &Shared, tag: &str, e: std::io::Error) {
    lock(shared).note(&format!("{tag} accept: {e}"));
    std::thread::sleep(Duration::from_millis(100));
}

/// A login with its Argon2 work on the pool: locked first phase, the pool without the lock, locked second phase.
// DC-DEFAULT: LG-328 (proposed, project decision)
fn login_off_lock<'a>(
    shared: &'a Shared,
    mut g: MutexGuard<'a, Core>,
    sid: Sid,
    id: u16,
    payload: &[u8],
) -> MutexGuard<'a, Core> {
    let mut step = None;
    let r = g.scope(|st| {
        // LG-318: as in [`route`]
        if !crate::botpool::message_refused(st, sid, id) {
            step = Some(auth::login_begin(st, sid, payload)?);
        }
        Ok(())
    });
    report(&g, r);
    let Some(auth::LoginStep::Check(pending)) = step else {
        return g;
    };
    run_deferred(shared, &mut g);
    drop(g);
    let keys = pending.keys().clone();
    let out = crate::pwpool::global().run(move || {
        let c = pending.compute();
        (pending, c)
    });
    let mut g = lock(shared);
    let gone = g.sessions.get(&sid).map(|s| s.closing).unwrap_or(true);
    let r = g.scope(|st| {
        match out {
            Ok(_) if gone => st.limits.login.end_attempt(&keys),
            Ok((pending, checked)) => auth::login_finish(st, sid, pending, checked)?,
            Err(_) => auth::login_busy(st, sid, &keys),
        }
        Ok(())
    });
    report(&g, r);
    g
}

fn serve_connection(shared: Shared, conn: crate::tls::Conn, listener: Listener) {
    let sid = NEXT_SID.fetch_add(1, Ordering::Relaxed);
    let stream = conn.tcp();
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_default();
    let local = stream
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_default();
    let _ = stream.set_nodelay(true);
    let tls = conn.is_tls();
    // LG-223: a TLS connection is read and written through the shared TLS state
    let Ok((mut reader, mut writer)) = conn.split() else {
        return;
    };
    // LG-326: until its login a connection has a time limit (on the reader's own socket: a clone's timeout is not
    // the reader's on every platform)
    let pre_login = lock(&shared).limits.pre_login;
    reader.set_read_timeout(Some(pre_login));
    let mut logged_in = false;
    // DC-DEFAULT: LG-329 (proposed, project decision) over the cap a message is dropped here, before the lock
    let mut rate = crate::limits::SessionRate::new(std::time::Instant::now());
    let (mut dropped, mut dropped_note) = (0u64, None::<std::time::Instant>);
    let (tx, rx) = channel::<Outgoing>();
    {
        let mut g = lock(&shared);
        g.sessions.insert(
            sid,
            Session {
                sid,
                listener,
                peer: peer.clone(),
                local,
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
        g.note(&format!(
            "{} connection {} from {}{}",
            listener.tag(),
            sid,
            peer,
            if tls { " (TLS)" } else { "" }
        ));
    }
    let _ = std::thread::Builder::new()
        .name(format!("w{}", sid))
        .spawn(move || {
            for o in rx {
                match o {
                    Outgoing::Frame(f) => {
                        if writer.write_all(&f).is_err() {
                            break;
                        }
                    }
                    Outgoing::Close => {
                        writer.close();
                        break;
                    }
                }
            }
        });
    loop {
        match dc_wire::read_tcp_frame(&mut reader) {
            Ok(Some((fid, payload))) => {
                let id = dc_wire::effective_packet_id(fid, &payload);
                let at = std::time::Instant::now();
                if !rate.admit(id, at) {
                    dropped += 1;
                    if dropped_note.is_none_or(|t| at.duration_since(t) > Duration::from_secs(10)) {
                        dropped_note = Some(at);
                        lock(&shared).note(&format!(
                            "connection {sid} from {peer}: over the message rate, {dropped} dropped so far (LG-329)"
                        ));
                    }
                    continue;
                }
                let delay = {
                    let g = lock(&shared);
                    if g.log.has_pktlog() {
                        g.log.packet(listener, sid, false, fid, &payload);
                    }
                    g.opts.delay
                };
                if fid as u16 == SERVER_PICK && delay > 0.0 {
                    // SPEC: LOBBY_HARNESS.md §1 --delay before answering 0x20a (outside the lock)
                    std::thread::sleep(Duration::from_secs_f64(delay.min(600.0)));
                }
                let mut g = lock(&shared);
                if g.sessions.get(&sid).map(|s| s.closing).unwrap_or(true) {
                    break;
                }
                if id == crate::msg::LOGIN_REQUEST {
                    // this reader waits for the check, so what the client sends next stays behind its login
                    g = login_off_lock(&shared, g, sid, id, &payload);
                    if g.sessions.get(&sid).map(|s| s.closing).unwrap_or(true) {
                        break;
                    }
                } else {
                    let r = g.scope(|st| route(st, sid, fid, &payload));
                    report(&g, r);
                }
                run_deferred(&shared, &mut g);
                if !logged_in && g.sessions.get(&sid).is_some_and(|s| s.account.is_some()) {
                    logged_in = true;
                    reader.set_read_timeout(None);
                }
            }
            Ok(None) => break,
            Err(e) => {
                lock(&shared).note(&format!("connection {}: {}", sid, e));
                break;
            }
        }
    }
    let mut g = lock(&shared);
    let r = g.scope(|st| lobby::disconnect(st, sid));
    report(&g, r);
    // a failed scope must not leave a dead session behind
    g.sessions.remove(&sid);
    run_deferred(&shared, &mut g);
}

pub fn spawn_listener(shared: Shared, l: TcpListener, kind: Listener) {
    spawn_listener_tls(shared, l, kind, None)
}

/// A listener whose connections pass `gate` first (the auth port, LG-223 / LG-224); None = plain only.
pub fn spawn_listener_tls(
    shared: Shared,
    l: TcpListener,
    kind: Listener,
    gate: Option<Arc<crate::tls::Gate>>,
) {
    spawn_listener_capped(shared, l, kind, gate, crate::tls::MAX_HANDSHAKES)
}

/// [`spawn_listener_tls`] with its own cap of handshakes in progress: further connections are closed at once.
pub fn spawn_listener_capped(
    shared: Shared,
    l: TcpListener,
    kind: Listener,
    gate: Option<Arc<crate::tls::Gate>>,
    max_handshakes: usize,
) {
    let slots = crate::tls::Slots::default();
    let cap = lock(&shared).limits.conns.clone();
    let mut refused_note = None;
    let _ = std::thread::Builder::new()
        .name(format!("{}-accept", kind.tag()))
        .spawn(move || {
            for s in l.incoming() {
                match s {
                    Ok(stream) => {
                        // LG-130: a stopping server takes no new connections
                        if lock(&shared).stopping {
                            let _ = stream.shutdown(std::net::Shutdown::Both);
                            continue;
                        }
                        // LG-326: the place is held by this connection's thread until it ends
                        let Some(place) =
                            admit_conn(&shared, &cap, &stream, kind.tag(), &mut refused_note)
                        else {
                            let _ = stream.shutdown(std::net::Shutdown::Both);
                            continue;
                        };
                        // LG-223: a cap of handshakes in progress keeps the accept loop and the threads alive
                        let slot = if gate.is_some() {
                            match slots.take(max_handshakes) {
                                Some(s) => Some(s),
                                None => {
                                    let _ = stream.shutdown(std::net::Shutdown::Both);
                                    if slots.first_of_burst() {
                                        lock(&shared).note(&format!(
                                            "{} accept: {} handshakes in progress, closing new connections at once (LG-223)",
                                            kind.tag(),
                                            max_handshakes
                                        ));
                                    }
                                    continue;
                                }
                            }
                        } else {
                            None
                        };
                        let sh = Arc::clone(&shared);
                        let gate = gate.clone();
                        let _ = std::thread::Builder::new()
                            .name(format!("{}-conn", kind.tag()))
                            .spawn(move || {
                                let _place = place;
                                match gate {
                                    None => {
                                        serve_connection(sh, crate::tls::Conn::Plain(stream), kind)
                                    }
                                    Some(g) => admit_and_serve(sh, stream, kind, &g, slot),
                                }
                            });
                    }
                    Err(e) => accept_failed(&shared, kind.tag(), e),
                }
            }
        });
}

/// The TLS gate of one connection, then the session (LG-224: a refused plain login is closed before any frame).
fn admit_and_serve(
    shared: Shared,
    stream: TcpStream,
    kind: Listener,
    gate: &crate::tls::Gate,
    slot: Option<crate::tls::Slot>,
) {
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_default();
    match crate::tls::admit(stream, gate, slot) {
        crate::tls::Admitted::Conn(c) => serve_connection(shared, c, kind),
        crate::tls::Admitted::PlainRefused(s) => {
            let _ = s.shutdown(std::net::Shutdown::Both);
            lock(&shared).note(&format!(
                "{} connection from {} refused: plain (unencrypted) login on a TLS-only port (LG-224; the client must use TLS, or start the lobby with --plain-auth-public)",
                kind.tag(),
                peer
            ));
        }
        crate::tls::Admitted::Failed(e) => {
            lock(&shared).note(&format!("{} connection from {}: {}", kind.tag(), peer, e))
        }
    }
}

/// Periodic expiry scan.
// DC-DEFAULT: CD-06 every inventory.expiry_scan_secs (60 s)
pub fn spawn_expiry(shared: Shared) {
    let _ = std::thread::Builder::new()
        .name("expiry".into())
        .spawn(move || loop {
            let secs = lock(&shared).rules.expiry_scan_secs.max(1);
            std::thread::sleep(Duration::from_secs(secs));
            let mut g = lock(&shared);
            let r = g.scope(|st| crate::notices::expire_with_boxes(st, None));
            report(&g, r);
        });
}

/// LG-316: retries the report mails waiting in the DB (a report also starts its own mail when it arrives). No thread
/// without `mail.reports_to`.
pub fn spawn_report_mail(shared: Shared) {
    if !crate::reportmail::enabled(&lock(&shared)) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("report-mail".into())
        .spawn(move || loop {
            std::thread::sleep(crate::reportmail::PUMP_EVERY);
            let mut g = lock(&shared);
            let r = g.scope(crate::reportmail::pump);
            report(&g, r);
        });
}

/// LG-379: records the mail worker's answers for beta mail every 10 s. Without it an answer waited until the operator
/// opened the beta list, and a lobby restart before that lost it: the mail was sent, but its row was later written off
/// as failed. No thread without `beta.enabled`.
pub fn spawn_beta_mail_drain(shared: Shared) {
    if !lock(&shared).opts.beta.enabled {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("beta-mail".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(10));
            let g = lock(&shared);
            g.beta.drain(&g.db, crate::clock::unix_now());
        });
}

/// The text of a game server's exit in the log: `exit code: N` on every platform (`ExitStatus` prints
/// "exit status: N" on unix and "exit code: N" on Windows); a signal death keeps the platform's own text.
pub fn exit_text(s: std::process::ExitStatus) -> String {
    match s.code() {
        Some(c) => format!("exit code: {c}"),
        None => s.to_string(),
    }
}
