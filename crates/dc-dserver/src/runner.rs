//! The process around the session state machine: one UDP socket, a stdin reader thread, a millisecond clock,
//! stdout for event lines and host log lines, and an optional log file.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, Write};
use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::cli::Args;
use crate::rom_tables::RomTables;
use dc_bute::modpatch::WeaponContent;
use crate::session::{Output, Server};
use crate::world::{self, WorldInfo};
use dc_net_engine::socket_reader::SocketReader;
use dc_net_engine::time::since;
use dc_net_engine::Millis;
use dc_wire::udp::RECV_BUFFER_BYTES;

/// How long the socket waits for a datagram before the loop runs its timers.
pub const POLL_WAIT_MS: u64 = 2;
/// After `@end`, how long the process keeps sending the final bye copies at most.
// SPEC: UDP_TRANSPORT_2009.md §9.1 (4 copies, 100 ms apart)
pub const EXIT_FLUSH_MS: u32 = 1000;
/// A pass of the main loop that took longer than this (from one pass start to the next) is logged with where the
/// time went: the server is one thread, so a long pass delays every client's join ask and acknowledgement.
// DC-DEFAULT: DSV-135 (proposed) slow-pass diagnostic line at 1000 ms, see DC_DEFAULTS.md
pub const SLOW_PASS_MS: u32 = 1000;

/// Exit code: world could not be loaded.
pub const EXIT_NO_WORLD: i32 = 2;
/// Exit code: the UDP port could not be bound.
pub const EXIT_NO_SOCKET: i32 = 3;
/// Exit code: the `-content` document is malformed or its patches do not fit the weapon table (DSV-478).
pub const EXIT_BAD_CONTENT: i32 = 4;

struct Sink {
    file: Option<File>,
    start: Instant,
}

impl Sink {
    fn stdout_line(&mut self, line: &str) -> bool {
        let mut out = io::stdout().lock();
        writeln!(out, "{line}").is_ok() && out.flush().is_ok()
    }

    fn log(&mut self, line: &str) {
        if let Some(f) = self.file.as_mut() {
            let _ = writeln!(f, "{:>9} {line}", self.start.elapsed().as_millis());
        }
        self.stdout_line(line);
    }

    /// A line for the `-log` file only (not on stdout): the closing counters come after `@end`, which must stay the
    /// last stdout line.
    fn file_only(&mut self, line: &str) {
        if let Some(f) = self.file.as_mut() {
            let _ = writeln!(f, "{:>9} {line}", self.start.elapsed().as_millis());
        }
    }

    fn event(&mut self, line: &str) -> bool {
        if let Some(f) = self.file.as_mut() {
            let _ = writeln!(f, "{:>9} {line}", self.start.elapsed().as_millis());
        }
        self.stdout_line(line)
    }
}

/// Loads the world named by the arguments.
pub fn load_world(args: &Args) -> Result<WorldInfo, String> {
    if let Some(path) = &args.world_file {
        let announce = args
            .announce_path
            .clone()
            .or_else(|| args.world.clone())
            .unwrap_or_else(|| {
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
        // a ROM at hand (`-rom`, else `CA_ROM`) resolves the evar and client-variable names of the file's world
        // settings; without one the file is served as it is (DC-DEFAULT: MAP-20)
        let rom = args
            .rom
            .clone()
            .or_else(|| dc_rez::dc_rom::resolve_rom().ok());
        return world::load_from_file_with_rom(path, &announce, rom.as_deref());
    }
    let arg = args.world.as_deref().unwrap_or_default();
    let rom = match &args.rom {
        Some(r) => r.clone(),
        None => dc_rez::dc_rom::resolve_rom().map_err(|e| format!("CA_ROM: {e}"))?,
    };
    world::load_from_rom(&rom, arg, args.world_form, args.announce_path.as_deref())
}

/// Reads the game tables from the ROM named by `-rom` or `CA_ROM`; empty tables (DeCombat defaults) without one.
/// With `-content` the mod set's weapon content is read and applied to the weapon table first (DSV-478): a
/// malformed document, a patch the table refuses or a missing ROM is an error and the server does not start.
// DC-DEFAULT: DSV-478 (proposed)
pub fn load_tables(args: &Args) -> Result<(RomTables, Vec<String>), String> {
    let content = load_content(args)?;
    let rom = match &args.rom {
        Some(r) => Some(r.clone()),
        None => dc_rez::dc_rom::resolve_rom().ok(),
    };
    match rom {
        Some(r) => {
            let (t, mut notes) = RomTables::load_with_content(&r, content.as_ref())?;
            if let Some(c) = &content {
                notes.push(content_line(c));
            }
            Ok((t, notes))
        }
        None if content.is_some() => Err("content: no ROM, the patches have no weapon table".into()),
        None => Ok((
            RomTables::default(),
            vec!["no ROM: game tables empty, DeCombat defaults apply".into()],
        )),
    }
}

/// The `-content` document of the arguments (`None` without the flag); a malformed file is an error.
pub fn load_content(args: &Args) -> Result<Option<WeaponContent>, String> {
    match &args.content {
        Some(p) => WeaponContent::load(p).map(Some).map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

/// The start-up log line of an applied content document.
pub fn content_line(c: &WeaponContent) -> String {
    format!(
        "content: {} weapon patches, {} ammo patches, hash {}",
        c.weapons.len(),
        c.ammo.len(),
        c.hash()
    )
}

/// Runs the server until `quit`, stdin EOF, the idle limit, the end of the match or a broken stdout. Returns the
/// exit code.
// SPEC: DSERVER_PIPE.md §1 (process model), §4 (events), §7 (exit on stdin EOF; error line then @end)
pub fn run(args: Args) -> i32 {
    // the control lines are read from the start: the lobby writes the expected players while the world loads, and a
    // busy machine must not let a client's hello overtake its roster line (refused as unlisted) after `@ready`
    let (tx, rx) = mpsc::channel::<Option<String>>();
    std::thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => {
                    if tx.send(Some(l)).is_err() {
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(None);
    });

    let start = Instant::now();
    let now = || start.elapsed().as_millis() as Millis;
    let mut sink = Sink {
        file: args
            .log
            .as_ref()
            .and_then(|p| OpenOptions::new().create(true).append(true).open(p).ok()),
        start,
    };
    for i in &args.ignored {
        sink.log(&format!("dc-dserver: option ignored: {i}"));
    }
    for n in &args.config_notes {
        sink.log(&format!("dc-dserver: config: {n}"));
    }
    let world = match load_world(&args) {
        Ok(w) => w,
        Err(e) => {
            sink.log(&format!("dc-dserver: error: {e}"));
            sink.event(crate::pipe::END_LINE);
            return EXIT_NO_WORLD;
        }
    };
    sink.log(&format!(
        "dc-dserver: world {} ({} bytes, sha256 {}, {} objects, {} player spawns), announced as {:?}",
        world.display,
        world.size,
        &world.sha256[..16],
        world.objects,
        world.spawns.len(),
        world.announce
    ));
    let sock = match UdpSocket::bind((args.bind, args.port)) {
        Ok(s) => s,
        Err(e) => {
            sink.log(&format!(
                "dc-dserver: error: cannot bind {}:{}: {e}",
                args.bind, args.port
            ));
            sink.event(crate::pipe::END_LINE);
            return EXIT_NO_SOCKET;
        }
    };
    // read by a thread without a receive timeout: a timeout on the socket loses datagrams on Windows under load
    // (see `dc_net_engine::socket_reader`; measured with this server and the bots)
    let reader = match SocketReader::start(&sock, RECV_BUFFER_BYTES) {
        Ok(r) => r,
        Err(e) => {
            sink.log(&format!(
                "dc-dserver: error: cannot start the receiver: {e}"
            ));
            sink.event(crate::pipe::END_LINE);
            return EXIT_NO_SOCKET;
        }
    };
    let port = sock.local_addr().map(|a| a.port()).unwrap_or(args.port);
    let (tables, notes) = match load_tables(&args) {
        Ok(t) => t,
        Err(e) => {
            sink.log(&format!("dc-dserver: error: {e}"));
            sink.event(crate::pipe::END_LINE);
            return EXIT_BAD_CONTENT;
        }
    };
    for n in notes {
        sink.log(&format!("dc-dserver: {n}"));
    }
    sink.log(&format!(
        "dc-dserver: game tables: {} weapons, {} ammunition rows, team names {}; tick {} Hz",
        tables.weapons.weapons.len(),
        tables.weapons.ammo.len(),
        tables.teams.names.iter().filter(|n| n.is_some()).count(),
        args.server.tick_hz
    ));
    let mut server: Server<SocketAddr> =
        Server::with_tables(args.server.clone(), world, tables, now());
    server.set_endpoint(&args.bind.to_string(), port);
    // NET-23: a client behind a NAT that changes its source port keeps its link (the same IP)
    server.set_address_moves(|a, b| a.ip() == b.ip());
    sink.log(&format!("dc-dserver: {}", server.visibility().describe()));
    if let Some(line) = server.preload_character() {
        sink.log(&format!("dc-dserver: {line}"));
    }
    if !sink.event(&server.ready_line(port)) {
        return 1;
    }

    let mut quitting: Option<Millis> = None;
    // where the previous pass spent its time (control lines, datagrams, timers and output), for the slow-pass line
    let mut last_top: Option<Millis> = None;
    let mut last_phases = [0u32; 3];
    let mut send_errors = 0u64;
    let mut recv_errors = 0u64;
    let (mut received, mut sent) = (0u64, 0u64);
    // per peer: datagrams received from it, datagrams sent to it (and the failed sends)
    let mut per_peer: std::collections::BTreeMap<SocketAddr, [u64; 3]> = Default::default();
    loop {
        // control lines first: a roster line that came with a hello in the same pass is applied before it
        let lines_at = now();
        if let Some(prev) = last_top {
            let gap = since(lines_at, prev);
            if gap >= SLOW_PASS_MS {
                sink.log(&format!(
                    "dc-dserver: slow pass: {gap} ms since the previous one (control lines {} ms, datagrams {} ms,                      timers and output {} ms; the rest was spent outside the loop: the process was not scheduled)",
                    last_phases[0], last_phases[1], last_phases[2]
                ));
            }
        }
        last_top = Some(lines_at);
        while let Ok(msg) = rx.try_recv() {
            let go_on = match msg {
                Some(line) => server.handle_line(lines_at, &line),
                None => {
                    sink.log("dc-dserver: stdin closed, stopping");
                    false
                }
            };
            if !go_on && quitting.is_none() {
                server.finish(lines_at);
                quitting = Some(lines_at);
            }
        }
        let after_lines = now();
        let mut waiting = true;
        loop {
            let got = if waiting {
                waiting = false;
                reader.next_within(Duration::from_millis(POLL_WAIT_MS))
            } else {
                reader.try_next()
            };
            match got {
                Some(Ok((data, from))) => {
                    received += 1;
                    per_peer.entry(from).or_default()[0] += 1;
                    server.handle_datagram(now(), from, &data)
                }
                // an oversized datagram fails recvfrom and is dropped (spec §4); anything else: logged (the first few)
                // SPEC: UDP_TRANSPORT_2009.md §4 (WSAECONNRESET is ignored; the reader thread skips it)
                Some(Err(e)) => {
                    recv_errors += 1;
                    if recv_errors <= 5 {
                        sink.log(&format!("dc-dserver: receive error ignored: {e}"));
                    }
                    break;
                }
                None => break,
            }
        }
        // the timers run on a clock read after the datagrams (never behind a datagram's own time)
        let t = now();
        let datagrams_ms = since(t, after_lines);
        if quitting.is_none() && server.idle_expired(t) {
            sink.log("dc-dserver: idle limit reached, stopping");
            server.finish(t);
            quitting = Some(t);
        }
        let mut stdout_ok = true;
        for o in server.poll(t) {
            match o {
                Output::Transmit { to, bytes } => {
                    // a failed send is logged (the first few): a datagram that never left is otherwise invisible
                    sent += 1;
                    per_peer.entry(to).or_default()[1] += 1;
                    if let Err(e) = sock.send_to(&bytes, to) {
                        send_errors += 1;
                        per_peer.entry(to).or_default()[2] += 1;
                        if send_errors <= 5 {
                            sink.log(&format!(
                                "dc-dserver: send to {to} failed ({} bytes): {e} [failure {send_errors}]",
                                bytes.len()
                            ));
                        }
                    }
                }
                Output::Pipe(line) => stdout_ok &= sink.event(&line),
                Output::Log(line) => sink.log(&line),
            }
        }
        last_phases = [since(after_lines, lines_at), datagrams_ms, since(now(), t)];
        if quitting.is_none() && server.is_finished() {
            // the match ended and its hold times passed (the session already printed @end)
            quitting = Some(t);
        }
        if !stdout_ok && quitting.is_none() {
            // the lobby is gone: stop like on quit (results cannot be delivered anyway)
            server.finish(t);
            quitting = Some(t);
        }
        if let Some(q) = quitting {
            if !server.has_links() || since(t, q) > EXIT_FLUSH_MS {
                sink.file_only(&format!(
                    "dc-dserver: udp: {received} datagrams received ({recv_errors} receive errors), {sent} sent ({send_errors} send errors)"
                ));
                sink.file_only(&format!("dc-dserver: udp host: {:?}", server.host_stats()));
                for (peer, c) in &per_peer {
                    sink.file_only(&format!(
                        "dc-dserver: udp peer {peer}: {} received, {} sent, {} failed sends",
                        c[0], c[1], c[2]
                    ));
                }
                return 0;
            }
        }
    }
}
