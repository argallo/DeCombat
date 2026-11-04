//! Capture-gated check of the round-start set (AF-5, the round-start voice): the reference stack's start messages on every
//! link of five reference captures (the reference stack against the real 2009 client and the bots) against the set our
//! server sends to two synthetic clients, one that loads first and one that loads later. Compared: the order and
//! values of shell 0x90, 0x88, 0x67, 0x43, 0x44 and 0x45 and the head bits of 0x45 (intro, waiting flag, countdown).
//! Not compared: `round_secs` (GMD-2: our rooms carry a time limit, the reference stack's 0), the place of 0xb5 (the reference stack
//! sent it before 0x45 on one link and after it on the other, tdm6 c5 / c6). Skipped without the captures; captures
//! are read in place and never copied (REFERENCE_TESTING.md §5).

use dc_botclient::capdecode::{decode_file, Decoded};
use dc_dserver::session::{Output, Server, ServerTuning};
use dc_dserver::synth_client::{ClientState, Observed, SynthClient, SynthConfig};
use dc_dserver::world::{synthetic_world_bytes, world_from_bytes};
use dc_net_engine::sim::{Faults, SimChannel};

/// A capture folder under `DC_CAPTURES` when present.
fn capture_named(folder: &str) -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("DC_CAPTURES") {
        let c = std::path::PathBuf::from(p)
            .join(folder)
            .join("capture.jsonl");
        return c.is_file().then_some(c);
    }
    None
}

/// One start-set entry: shell id and its value (0x90 flag, 0x88 state, 0x67 state, 0x45 head bits as
/// `intro | waiting << 1 | (countdown != 0) << 2`; 0 for 0x43 / 0x44).
type Entry = (u8, u32);

const START_IDS: [u8; 6] = [0x90, 0x88, 0x67, 0x43, 0x44, 0x45];

/// The reference stack's start set of one link, up to and including its first 0x45 and the 0x90 / 0x88 pair after it (for
/// the link of the first loader), in arrival order.
fn reference_set(d: &Decoded, conn: u32) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut after_begin = false;
    for m in d.of(conn, false) {
        let Some(id) = m.shell() else { continue };
        if !START_IDS.contains(&id) {
            continue;
        }
        let mut r = m.bits.reader();
        r.read_u8();
        r.read_u8();
        let v = match id {
            0x90 => r.read_bit() as u32,
            0x88 | 0x67 => r.read_u8() as u32,
            0x45 => {
                r.read_u32();
                let intro = r.read_bit() as u32;
                let waiting = r.read_bit() as u32;
                let countdown = (r.read_u32() != 0) as u32;
                intro | waiting << 1 | countdown << 2
            }
            _ => 0,
        };
        if after_begin && !matches!(id, 0x90 | 0x88) {
            break;
        }
        out.push((id, v));
        if id == 0x45 {
            after_begin = true;
        }
        if after_begin && out.iter().filter(|e| e.0 == 0x88).count() == 2 {
            break;
        }
    }
    out
}

/// Our start set as a synthetic client took it (same cut as [`reference_set`]).
fn our_set(s: &Observed) -> Vec<Entry> {
    let (mut f90, mut f88, mut f67) = (
        s.flag90.iter(),
        s.switch_states.iter(),
        s.life_states.iter(),
    );
    let mut out = Vec::new();
    let mut after_begin = false;
    for &(_, id) in &s.shell_ids {
        let v = match id {
            0x90 => *f90.next().expect("0x90 value") as u32,
            0x88 => *f88.next().expect("0x88 value") as u32,
            0x67 => f67.next().expect("0x67 value").1 as u32,
            0x45 => {
                // our 0x45 head: intro on (DSV-50), waiting flag clear (DSV-20), no countdown
                let (_, waiting) = s.match_begin.expect("0x45");
                (s.intro.is_some() as u32) | (waiting as u32) << 1
            }
            0x43 | 0x44 => 0,
            _ => continue,
        };
        if after_begin && !matches!(id, 0x90 | 0x88) {
            break;
        }
        out.push((id, v));
        if id == 0x45 {
            after_begin = true;
        }
        if after_begin && out.iter().filter(|e| e.0 == 0x88).count() == 2 {
            break;
        }
    }
    out
}

/// Our server with two synthetic clients linked together, the second loading 4 s longer (the user's rounds: the
/// bot loads first, the real client later).
fn our_two_links() -> (Observed, Observed) {
    let world = world_from_bytes(&synthetic_world_bytes(), "Worlds\\Synth.dat", "Synth").unwrap();
    let mut server: Server<u32> = Server::new(ServerTuning::default(), world, 0);
    assert!(server.handle_line(0, "room key=TDM type=1 goal=100 ff=0 time=600"));
    assert!(server.handle_line(0, "user 7 3 99 0 120 Dave"));
    assert!(server.handle_line(0, "user 8 4 55 1 0 Big_Erin"));
    let mut late = SynthConfig::named("Big_Erin");
    late.load_ms = 5_500;
    let mut peers: Vec<(u32, SynthClient, SimChannel, SimChannel)> = vec![
        (
            1,
            SynthClient::new(SynthConfig::named("Dave")),
            SimChannel::new(3, Faults::clean(2)),
            SimChannel::new(4, Faults::clean(2)),
        ),
        (
            2,
            SynthClient::new(late),
            SimChannel::new(5, Faults::clean(2)),
            SimChannel::new(6, Faults::clean(2)),
        ),
    ];
    let mut now = 0u32;
    while now < 40_000 {
        for p in &mut peers {
            for d in p.1.poll(now) {
                p.2.send(now, d);
            }
        }
        for p in &mut peers {
            for d in p.2.deliver(now) {
                server.handle_datagram(now, p.0, &d);
            }
        }
        for o in server.poll(now) {
            if let Output::Transmit { to, bytes } = o {
                if let Some(p) = peers.iter_mut().find(|p| p.0 == to) {
                    p.3.send(now, bytes);
                }
            }
        }
        for p in &mut peers {
            for d in p.3.deliver(now) {
                p.1.handle_datagram(now, &d);
            }
        }
        now += 5;
        if peers.iter().all(|p| p.1.state() == ClientState::InWorld) && now > 12_000 {
            break;
        }
    }
    for p in &peers {
        assert!(p.1.seen.errors.is_empty(), "{:?}", p.1.seen.errors);
        assert_eq!(p.1.state(), ClientState::InWorld);
    }
    (peers[0].1.seen.clone(), peers[1].1.seen.clone())
}

/// The reference stack rule (five captures, ten links): every link that existed when the world started got 0x90 [1] /
/// 0x88 [2] at the link and 0x90 [0] / 0x88 [3] right after the world's first `UP_WORLD_READY`, so a later loader
/// holds both before its 0x67 [1] and its 0x45; the first loader's pair follows its 0x45. Each link's 0x45 comes
/// after its 0x67 [1] (the 2009 client's voice gate, cshell+0x49f00..0x49f7b) with the intro and without the
/// waiting flag, once per link. Our two links give the same sets (DSV-97).
// OBSERVED: captures 20261025_065016_tdm6, 20261025_002236_tdm4, 20261025_001409_tdm2,
// 20261025_000014_tdm1, 20261025_012331_botduel3 (REFERENCE); before DSV-97 ours differed on the later loader
// (20261025_153705_rust7 c5: 0x67 [1], 0x43, 0x44, 0x45, then 0x90 [0] / 0x88 [3])
// DISASM: cshell+0x49f00..0x49f7b
// DC-DEFAULT: DSV-97 (proposed)
#[test]
fn round_start_set_matches_the_reference_on_first_and_later_loaders() {
    const FOLDERS: [&str; 5] = [
        "20261025_065016_tdm6",
        "20261025_002236_tdm4",
        "20261025_001409_tdm2",
        "20261025_000014_tdm1",
        "20261025_012331_botduel3",
    ];
    let first_form: Vec<Entry> = vec![
        (0x90, 1),
        (0x88, 2),
        (0x67, 1),
        (0x43, 0),
        (0x44, 0),
        (0x45, 1),
        (0x90, 0),
        (0x88, 3),
    ];
    let later_form: Vec<Entry> = vec![
        (0x90, 1),
        (0x88, 2),
        (0x90, 0),
        (0x88, 3),
        (0x67, 1),
        (0x43, 0),
        (0x44, 0),
        (0x45, 1),
    ];
    let mut links = 0;
    let (mut firsts, mut laters) = (0, 0);
    for folder in FOLDERS {
        let Some(cap) = capture_named(folder) else {
            eprintln!("skipped {folder}: capture not present");
            continue;
        };
        let d = decode_file(&cap).unwrap();
        let world_start = d
            .msgs
            .iter()
            .filter(|m| m.c2s && m.shell() == Some(0x87))
            .map(|m| m.t)
            .fold(f64::INFINITY, f64::min);
        for &conn in d.conns.keys() {
            let begins = d
                .of(conn, false)
                .filter(|m| m.shell() == Some(0x45))
                .count();
            assert!(begins <= 1, "{folder} c{conn}: {begins} x 0x45");
            let set = reference_set(&d, conn);
            if set.first() != Some(&(0x90, 1)) || begins == 0 {
                // a link of a running world (tdm2 c7): 0x90 [0] at the link, no 0x45
                continue;
            }
            links += 1;
            let ready = d
                .of(conn, true)
                .find(|m| m.shell() == Some(0x87))
                .map(|m| m.t)
                .expect("0x87");
            if ready <= world_start + 1e-6 {
                assert_eq!(set, first_form, "{folder} c{conn} (first loader)");
                firsts += 1;
            } else {
                assert_eq!(set, later_form, "{folder} c{conn} (later loader)");
                let pair = d
                    .of(conn, false)
                    .find(|m| {
                        m.shell() == Some(0x90) && m.bits.as_bytes().get(2).copied() == Some(0)
                    })
                    .map(|m| m.t)
                    .expect("0x90 [0]");
                assert!(
                    pair >= world_start && pair < world_start + 0.2,
                    "{folder} c{conn}: pair at {pair}, world start {world_start}"
                );
                laters += 1;
            }
        }
    }
    if links == 0 {
        eprintln!("skipped: no reference capture present");
        return;
    }
    assert!(firsts > 0 && laters > 0, "first {firsts}, later {laters}");
    let (first, later) = our_two_links();
    assert_eq!(our_set(&first), first_form, "our first loader");
    assert_eq!(our_set(&later), later_form, "our later loader");
    // the voice gate: one cue per link, in state 1, with the round-start key
    for s in [&first, &later] {
        assert_eq!(s.round_start_cues.len(), 1);
        assert!(!s.round_start_cues[0].1);
    }
    // the later loader takes its 0x45 with the 0x90 flag clear, as tdm6 / tdm4 / tdm2 c6 did
    assert!(!later.round_start_cues[0].2);
}
