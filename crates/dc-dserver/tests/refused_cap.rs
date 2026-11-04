//! Capture-gated evidence scan for DSV-101 (refused hit claims): every hit claim (`UP_FIRE` 0x82 with a hit object)
//! in the reference stack captures (the reference stack against the real 2009 client and our bots), with the victim's life at
//! the claim (dead from the `GS_KILL` 0x6d naming it until its next respawn request 0x89 was answered) and the
//! shot effect record (id 17, type 0x06) the reference stack sent for that shot on the shooter's own link. Skipped without the
//! captures; captures are read in place and never copied (REFERENCE_TESTING.md §5).

use dc_botclient::capdecode::{decode_file, Decoded};
use dc_dserver::msg::{read_from_client, FromClient};

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

/// The reference stack captures with game traffic (tdm3 has none).
const REFERENCE: [&str; 8] = [
    "20261025_000014_tdm1",
    "20261025_001409_tdm2",
    "20261025_002236_tdm4",
    "20261025_065016_tdm6",
    "20261025_010543_botduel1",
    "20261025_011446_botduel2",
    "20261025_012331_botduel3",
    "20261025_013028_botduel4",
];

/// One shot effect record: (t, first, shooter object, `b`, `c`, `to`).
type Rec = (f64, u16, u16, u16, u8, [f32; 3]);

/// One claim on a dead character: (t, shooter conn, claimed object, the record, the record after it at the same
/// time, the claimed distance, the distance of the record's `to` from the shot origin).
type CorpseClaim = (f64, u32, u16, Option<Rec>, Option<Rec>, f32, f32);

#[derive(Debug, Default)]
struct Scan {
    claims: usize,
    /// Claims on a character that was dead at the claim, with the forms the reference stack sent for them.
    on_corpse: Vec<CorpseClaim>,
    /// Forms of the claims on a living character: (first == the claimed object, `b`) -> count.
    alive: std::collections::BTreeMap<(bool, u16), usize>,
    /// Claims by a shooter that was dead at the claim.
    by_dead: Vec<(f64, u32, u16, Option<Rec>)>,
    /// Game links of the capture (every reference capture has one player per team: no friendly claim can occur).
    conns: usize,
}

fn records(d: &Decoded, conn: u32) -> Vec<Rec> {
    let mut v = Vec::new();
    for m in d.of(conn, false) {
        let b = m.bits.as_bytes();
        if b.len() >= 14 && b[0] == 17 && b[1] == dc_dserver::gs::FX_SHOT {
            let u = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
            let f = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
            v.push((m.t, u(2), u(4), u(11), b[13], [f(26), f(30), f(34)]));
        }
    }
    v
}

fn scan(d: &Decoded) -> Scan {
    let conns: Vec<u32> = d.conns.keys().copied().collect();
    // per link: client id, own objects (S2C_VIEW_OBJECT), deaths (0x6d naming the id), answered respawns (0x89)
    let mut pid = std::collections::BTreeMap::new();
    let mut objs: std::collections::BTreeMap<u32, Vec<u16>> = Default::default();
    for &c in &conns {
        for m in d.of(c, false) {
            let b = m.bits.as_bytes();
            match b.first() {
                Some(&dc_dserver::msg::S2C_CLIENT_ID) if b.len() >= 2 => {
                    pid.entry(c).or_insert(b[1]);
                }
                Some(&dc_dserver::msg::S2C_VIEW_OBJECT) if b.len() >= 3 => {
                    objs.entry(c)
                        .or_default()
                        .push(u16::from_le_bytes([b[1], b[2]]));
                }
                _ => {}
            }
        }
    }
    // dead intervals per connection, from any link's 0x6d (first link that saw it) to the conn's next 0x89 + 0.2 s
    let mut kills: Vec<(f64, u8)> = Vec::new();
    if let Some(&c0) = conns.first() {
        for m in d.of(c0, false) {
            let b = m.bits.as_bytes();
            if b.len() >= 3 && b[0] == 13 && b[1] == dc_dserver::gs::GS_KILL {
                kills.push((m.t, b[2]));
            }
        }
    }
    let dead_at = |c: u32, t: f64| -> bool {
        let Some(&p) = pid.get(&c) else { return false };
        let Some(&(kt, _)) = kills.iter().rev().find(|(kt, v)| *v == p && *kt <= t) else {
            return false;
        };
        // back alive at the victim link's next `GS_PLAYER_STATE` 1
        let back = d
            .of(c, false)
            .filter(|m| {
                let b = m.bits.as_bytes();
                m.t > kt
                    && b.len() >= 3
                    && b[0] == 13
                    && b[1] == dc_dserver::gs::GS_PLAYER_STATE
                    && b[2] == 1
            })
            .map(|m| m.t)
            .next();
        back.is_none_or(|bt| t < bt)
    };
    let mut s = Scan {
        conns: conns.len(),
        ..Scan::default()
    };
    let recs: std::collections::BTreeMap<u32, Vec<Rec>> =
        conns.iter().map(|&c| (c, records(d, c))).collect();
    for &c in &conns {
        for m in d.of(c, true) {
            let FromClient::Shell { shell_id, body } = read_from_client(&m.bits) else {
                continue;
            };
            if shell_id != 0x82 {
                continue;
            }
            let f = dc_dserver::up::parse_fire(&body);
            let Some((_, Some(obj))) = f.hit else {
                continue;
            };
            s.claims += 1;
            let my_obj = objs.get(&c).and_then(|v| v.last()).copied();
            // the reference stack's records of this shot on the shooter's link: the first one by the shooter's object within
            // 0.3 s, and the one right after it in the same arrival time (a continuation)
            let mine = recs.get(&c).map(Vec::as_slice).unwrap_or(&[]);
            let i = mine
                .iter()
                .position(|r| r.0 >= m.t && r.0 <= m.t + 0.3 && Some(r.2) == my_obj);
            let first = i.map(|i| mine[i]);
            let next = i
                .and_then(|i| mine.get(i + 1))
                .copied()
                .filter(|r| Some(r.0) == first.map(|f| f.0));
            if dead_at(c, m.t) {
                s.by_dead.push((m.t, c, obj, first));
                continue;
            }
            let victim = objs
                .iter()
                .find(|(vc, v)| **vc != c && v.contains(&obj))
                .map(|(vc, _)| *vc);
            if victim.is_some_and(|vc| dead_at(vc, m.t)) {
                let (dist, _) = f.hit.unwrap();
                let to_dist = first
                    .map(|r| {
                        let v = [
                            r.5[0] - f.origin[0],
                            r.5[1] - f.origin[1],
                            r.5[2] - f.origin[2],
                        ];
                        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
                    })
                    .unwrap_or(-1.0);
                s.on_corpse.push((m.t, c, obj, first, next, dist, to_dist));
            } else if victim.is_some() {
                if let Some(r) = first {
                    *s.alive.entry((r.1 == obj, r.3)).or_default() += 1;
                }
            }
        }
    }
    s
}

/// The census of refused-claim situations in the reference stack captures, printed case by case (EVIDENCE for DSV-101). Every
/// claim on a dead character got the miss form: a record that does not name the character, bit 0x4000 of `b` set
/// (no hit sound on the shooter's client), no continuation; a claim by a dead shooter got no record. botduel4 holds
/// the cases (40 corpse claims, 1 dead-shooter claim); no capture has a friendly claim (one player per team).
// OBSERVED: capture 20261025_013028_botduel4 (conn 5 on object 252 at t 103.140 .. 109.922,
// conn 6 on object 255 at t 69.937 .. 72.312: `first` 0 or none, b 0x4f10 / 0x4f00, `to` past the claimed point;
// conn 6 at t 123.156: no record); REFERENCE
// DISASM: cshell+0x2c8b36..0x2c8b8e (the own-shot hit sound needs bit 0x4000 of `b` clear)
#[test]
fn reference_refused_claim_census() {
    use dc_dserver::gs::SHOT_MASK_EXIT;
    // the continuation after a 0x0f01 hit record (tdm4 seq 3135; REFERENCE)
    const EXIT_LOW: u16 = 0x4be0;
    let mut seen = 0;
    for folder in REFERENCE {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {folder}");
            continue;
        };
        let d = decode_file(&cap).expect("capture");
        let s = scan(&d);
        println!(
            "{folder}: {} links, {} hit claims, {} on a dead character, {} by a dead shooter",
            s.conns,
            s.claims,
            s.on_corpse.len(),
            s.by_dead.len()
        );
        for x in &s.on_corpse {
            println!(
                "  corpse claim t {:.3} conn {} obj {} claimed dist {:.1}, record to at {:.1}: record {:?} then {:?}",
                x.0, x.1, x.2, x.5, x.6, x.3, x.4
            );
        }
        println!(
            "  claims on living characters (first is the victim, b) -> count: {:?}",
            s.alive
        );
        for x in &s.by_dead {
            println!(
                "  dead-shooter claim t {:.3} conn {} obj {}: record {:?}",
                x.0, x.1, x.2, x.3
            );
        }
        for x in &s.on_corpse {
            let r = x.3.expect("a record for every corpse claim");
            assert_ne!(r.1, x.2, "{folder} t {:.3}: the corpse is not named", x.0);
            assert_ne!(r.3 & 0x4000, 0, "{folder} t {:.3}: miss form", x.0);
            assert!(
                !x.4.is_some_and(|n| n.3 == SHOT_MASK_EXIT || n.3 == EXIT_LOW),
                "{folder} t {:.3}: no continuation",
                x.0
            );
        }
        assert!(
            s.by_dead.iter().all(|x| x.3.is_none()),
            "{folder}: no record for a dead shooter"
        );
        if folder.ends_with("botduel4") {
            assert!(
                s.on_corpse.len() >= 40 && !s.by_dead.is_empty(),
                "{folder}: the cases"
            );
        }
        seen += 1;
    }
    if seen == 0 {
        println!("skipped: no capture");
    }
}
