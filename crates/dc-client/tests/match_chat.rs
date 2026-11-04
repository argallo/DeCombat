//! In-match chat (M7): `UP_CHAT` from the client's chat input, `GS_CHAT` back from our dedicated server, the line in
//! the other clients' HUD state with its colour class.
//!
//! - `chat_line_reaches_the_other_clients_with_their_colour_class`: `dc-dserver` over loopback UDP with three
//!   headless dc-client network sessions (no window, no GPU): Talker and Mate on the first team, Foe on the second.
//!   Talker sends one line to all and one to its team through `WorldSource::chat`; each client's HUD state is fed
//!   from its own `FrameState::hud_events`. Mate shows both (friendly, team), Foe only the line to all (enemy),
//!   Talker its own echo of both (marked own).
//! - `tdm1_chat_bytes_decode_whole`: the real 2009 client's two `UP_CHAT` and the reference stack's two `GS_CHAT`
//!   echoes of `capture 20261025_000014_tdm1` (link 5), read in place (skipped without the
//!   capture; nothing of it is copied: the user's name and line are compared, never printed).
// SPEC: GAME_MESSAGES_2009_C2S.md §6.1; GAME_MESSAGES_2009_S2C.md §2.4 row 0x68, §3.2.2; DSERVER_PIPE.md §3.1
// OBSERVED: capture 20261025_000014_tdm1 (CAPTURE_tdm1_GAME_VALIDATION.md 2.18, 3.12)

mod support;

use std::net::SocketAddr;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use dc_client::hud::chat::{read_gs_chat, up_chat_body, ChatClass, ChatScope, GS_CHAT, UP_CHAT};
use dc_client::hud::state::{ChatKind, HudEvent, HudState};
use dc_client::testkit::{synth_source, SYNTH_DIMS};
use dc_client::{Connect, NetOptions, NetWorld, WorldFile, WorldSource};
use support::*;

fn world_file() -> WorldFile {
    let s = synth_source();
    WorldFile {
        bytes: s.bytes,
        stand_dims: SYNTH_DIMS,
        textures: s.textures,
    }
}

/// One headless session: loads the world, feeds its HUD state from the frame states for `secs`, and (Talker) sends
/// two lines once all three players are on its roster. Returns the HUD state's chat lines.
fn session(
    port: u16,
    name: &'static str,
    talk: bool,
    secs: u64,
    ready: Arc<Barrier>,
) -> std::thread::JoinHandle<Vec<(String, ChatClass, bool)>> {
    std::thread::spawn(move || {
        let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let mut o = NetOptions::new(Connect::Udp(server), name);
        o.world_file = Some(world_file());
        o.result_hold = Duration::from_millis(500);
        let mut net = NetWorld::start(o);
        let handle = net.handle();
        net.load(None).expect("world load");
        ready.wait();
        let start = Instant::now();
        let mut hud = HudState::default();
        let mut log: Vec<(String, ChatClass, bool)> = Vec::new();
        let mut sent = false;
        while start.elapsed() < Duration::from_secs(secs) {
            let fs = net.frame_state();
            for (t, e) in &fs.hud_events {
                if let HudEvent::Own { pid, .. } = e {
                    hud.own_pid = *pid;
                }
                hud.apply(e, *t, 0.0);
                if matches!(e, HudEvent::Chat(_)) {
                    if let Some(l) = hud.chat.back() {
                        if let ChatKind::Chat { class, own, .. } = l.kind {
                            log.push((l.text.clone(), class, own));
                        }
                    }
                }
            }
            if talk && !sent && hud.roster.len() >= 3 && fs.life_state == Some(1) {
                net.chat("hello all", ChatScope::All);
                net.chat("hello team", ChatScope::Team);
                sent = true;
            }
            std::thread::sleep(Duration::from_millis(16));
        }
        handle.shutdown(Duration::from_secs(5));
        assert!(
            !talk || sent,
            "{name} never saw the whole roster in the world"
        );
        log
    })
}

#[test]
fn chat_line_reaches_the_other_clients_with_their_colour_class() {
    let dir = scratch("matchchat");
    let world = synth_world_file(&dir);
    let (mut srv, port) = start_dserver(&world, &["-tick", "20", "-roundsecs", "60", "-seed", "3"]);
    // Talker and Mate on the first team, Foe on the second (DSERVER_PIPE.md §3.1 user lines)
    srv.line("user 1 1 1 0 0 Talker");
    srv.line("user 2 2 2 0 0 Mate");
    srv.line("user 3 3 3 1 0 Foe");
    let ready = Arc::new(Barrier::new(3));
    let talker = session(port, "Talker", true, 10, ready.clone());
    let mate = session(port, "Mate", false, 10, ready.clone());
    let foe = session(port, "Foe", false, 10, ready);
    let (talker, mate, foe) = (
        talker.join().expect("talker"),
        mate.join().expect("mate"),
        foe.join().expect("foe"),
    );
    srv.drain();
    let _ = srv.finish(Duration::from_secs(5));
    println!("talker {talker:?}\nmate {mate:?}\nfoe {foe:?}");
    let line = |t: &str| format!("Talker: {t}");
    assert_eq!(
        mate,
        [
            (line("hello all"), ChatClass::Friendly, false),
            (line("hello team"), ChatClass::Team, false)
        ],
        "the mate sees both lines"
    );
    assert_eq!(
        foe,
        [(line("hello all"), ChatClass::Enemy, false)],
        "the other team sees only the line to all"
    );
    assert_eq!(
        talker,
        [
            (line("hello all"), ChatClass::Friendly, true),
            (line("hello team"), ChatClass::Team, true)
        ],
        "the talker sees its own lines from the server's echo"
    );
}

/// The tdm1 capture's event file when present (under `DC_CAPTURES`).
fn tdm1_capture() -> Option<std::path::PathBuf> {
    const FOLDER: &str = "20261025_000014_tdm1";
    if let Ok(p) = std::env::var("DC_CAPTURES") {
        let c = std::path::PathBuf::from(p)
            .join(FOLDER)
            .join("capture.jsonl");
        return c.is_file().then_some(c);
    }
    None
}

/// The shell body of a captured message (after the engine id and the shell id).
fn shell_body(m: &dc_botclient::capdecode::CapMsg) -> dc_wire::udp::BitWriter {
    let mut r = m.bits.reader();
    r.read_u8();
    r.read_u8();
    let n = r.remaining_bits();
    r.read_bit_string(n)
}

#[test]
fn tdm1_chat_bytes_decode_whole() {
    use dc_botclient::capdecode::{decode_file, unbundle};
    let Some(cap) = tdm1_capture() else {
        println!("skipped: no tdm1 capture");
        return;
    };
    let d = decode_file(&cap).expect("capture");
    // link 5: the real 2009 client's game link (CAPTURE_tdm1_GAME_VALIDATION.md, evidence line)
    let up: Vec<_> = d
        .of(5, true)
        .filter(|m| m.shell() == Some(UP_CHAT))
        .map(shell_body)
        .collect();
    let down: Vec<_> = d
        .of(5, false)
        .flat_map(|m| unbundle(&m.bits))
        .filter(|b| {
            b.as_bytes().first() == Some(&dc_botclient::shell::s2c::SHELL_MSG)
                && b.as_bytes().get(1) == Some(&GS_CHAT)
        })
        .map(|b| {
            let mut r = b.reader();
            r.read_u8();
            r.read_u8();
            let n = r.remaining_bits();
            r.read_bit_string(n)
        })
        .collect();
    assert_eq!((up.len(), down.len()), (2, 2), "two lines and two echoes");
    for (u, e) in up.iter().zip(&down) {
        // our writer gives the real client's exact bits for its own line (scope 0, empty tag)
        let mut r = u.reader();
        let text = String::from_utf16(&r.read_zstr16()).expect("utf-16 line");
        assert_eq!(
            up_chat_body(&text, ChatScope::All).as_bytes(),
            u.as_bytes(),
            "UP_CHAT bits"
        );
        assert_eq!(up_chat_body(&text, ChatScope::All).bit_len(), u.bit_len());
        // the echo parses whole: two strings and the three numbers, no bit left
        let m = read_gs_chat(e.as_bytes(), e.bit_len()).expect("GS_CHAT");
        let used = 16 * (m.name.encode_utf16().count() + 1)
            + 16 * (m.text.encode_utf16().count() + 1)
            + 72;
        assert_eq!(used, e.bit_len(), "every bit of the echo is read");
        assert_eq!((m.sender, m.team_only, m.whisper), (0, false, 0));
        assert_eq!(m.name.encode_utf16().count(), 5);
        // the reference stack puts the name in front of the line: the HUD does not name the sender twice
        assert!(m.text.starts_with(&m.name) && m.text.ends_with(&text));
        assert_eq!(dc_client::hud::chat::line_text(&m.name, &m.text), m.text);
    }
}

#[test]
fn rom_chat_look_reads_every_pinned_key() {
    let rom = dc_rom::rom_or_skip!();
    let pe = dc_rom::PeFile::from_file(&rom.join("Game").join("CShell.dll")).expect("CShell");
    let keys =
        dc_client::hud::chat::ChatKeys::from_cshell(&pe).expect("every chat key pin matches");
    assert_eq!(
        keys.moderator_tag
            .as_ref()
            .map(|t| t.encode_utf16().count()),
        Some(2),
        "the moderator tag pin matches"
    );
    let hud = dc_client::hud::Hud::from_rom(&rom, 1024, 768, None).expect("ROM HUD");
    let look = &hud.layout.chat_look;
    let fb = dc_client::hud::chat::ChatLook::fallback();
    println!(
        "chat colours team {:?} friendly {:?} enemy {:?} moderator {:?} same {:?}; input font {} at {:?}, caret {} ms, {} images, buttons {}/{}",
        look.team,
        look.friendly,
        look.enemy,
        look.moderator,
        look.same_line,
        look.input_font,
        look.input_pos,
        look.caret_ms,
        look.images.len(),
        look.all_button.is_some(),
        look.team_button.is_some()
    );
    // the four classes are told apart by colour in the ROM layout (the values themselves are the user's data)
    let mut c = vec![look.team, look.friendly, look.enemy, look.moderator];
    c.sort_unstable();
    c.dedup();
    assert_eq!(c.len(), 4, "four distinct class colours");
    assert_ne!(
        look.selected_form, fb.selected_form,
        "button forms come from CShell"
    );
    assert!(look.moderator_tag.is_some());
    // CLI-1123: a team line is tagged with the ROM's team chat name (CRes 0x543)
    assert_eq!(hud.layout.chat.team_chat_name, "Team Chat");
}
