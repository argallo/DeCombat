//! Capture-gated replay of the 2009 shot cone through the server's own check (specs/WEAPON_FIRE_2009_addendum_cone.md
//! AC-1 .. AC-3, AC-5, AC-6; DSV-102): every `UP_FIRE` 0x82 of the selected captures goes through
//! `dc_dserver::cone::check`, the function `Game::on_fire` calls, with a per-link counter started at the 0x7d
//! `own_value` its server sent. Every pellet of the real 2009 client must come out `cone_ok` with the counter the
//! server expects; no link (the real client's, DeCombat's bots', dc-client's) may come out `cone_off`. The table is
//! never committed: the ROM entry when `CA_ROM` is set (with the weapon rows' bounds, AC-2.1 / AC-2.3), else the
//! generator (AC-1.3) and the cone's shape alone.
//!
//! Skipped without the captures; captures are read in place and never copied (REFERENCE_TESTING.md §5).
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1 (table), AC-2 (cone), AC-3 (fire loop), AC-5 (replay)
// DISASM: cshell+0x176cb0 (cone), cshell+0x2973a0 (table read), cshell+0x2974d0 (table load), cshell+0x23bca0 (fire loop)

use dc_botclient::capdecode::{decode_file, unbundle, Decoded};
use dc_botclient::shell::s2c;
use dc_dserver::cone::{self, ConeVerdict, LinkCone};
use dc_dserver::msg::{read_from_client, FromClient};
use dc_dserver::up::{parse_fire, UpFire};

/// Captures with the real 2009 client: the four reference sessions (TDM, the multi-pellet shot of tdm6) and one
/// session against our stack with zoomed shots (rust7).
const CAPTURES: [(&str, usize); 5] = [
    ("20261025_000014_tdm1", 1),
    ("20261025_001409_tdm2", 1),
    ("20261025_002236_tdm4", 1),
    ("20261025_065016_tdm6", 1),
    ("20261025_153705_rust7", 1),
];

/// A capture folder's event file when present (under `DC_CAPTURES`).
fn capture_named(folder: &str) -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("DC_CAPTURES") {
        let c = std::path::PathBuf::from(p)
            .join(folder)
            .join("capture.jsonl");
        return c.is_file().then_some(c);
    }
    None
}

/// `own_value` of the last `GS_MATCH_SETTINGS` 0x7d the server sent on `conn` before `t` (the seed start, AC-1.5).
// SPEC: GAME_MESSAGES_2009_S2C.md 0x7d row (`local_player`+0x1fc := own_value)
fn seed_start(d: &Decoded, conn: u32, t: f64) -> Option<u32> {
    let mut v = None;
    for m in d.of(conn, false).filter(|m| m.t <= t) {
        for part in unbundle(&m.bits) {
            let b = part.as_bytes();
            if b.len() > 2 && b[0] == s2c::SHELL_MSG && b[1] == 0x7d {
                let mut r = part.reader();
                r.read_u8();
                r.read_u8();
                r.read_zstr8();
                r.read_u8();
                r.read_s8();
                v = Some(r.read_u32());
            }
        }
    }
    v
}

#[derive(Debug, Default)]
struct LinkResult {
    pellets: usize,
    ok: usize,
    off: usize,
    na: usize,
    /// `cone_ok` pellets whose half-width was checked against the weapon row's interval.
    bounded: usize,
    /// Pellets whose `stat` was not the one the server's counter expected.
    skew: usize,
}

#[test]
fn shot_cone_replays_through_the_server_check() {
    let present: Vec<_> = CAPTURES
        .iter()
        .filter_map(|&(c, n)| capture_named(c).map(|p| (c, n, p)))
        .collect();
    if present.is_empty() {
        println!("skipped: captures absent");
        return;
    }
    // the server's own sources (what `Game::on_fire` hands the check): the tables the server loads from the ROM
    // when CA_ROM is set (the ROM's random table and weapon rows), else the generator and no rows
    let tables = match dc_rez::dc_rom::test_rom() {
        Some(rom) => {
            let (t, notes) = dc_dserver::rom_tables::RomTables::load(&rom);
            assert!(t.random_table.is_some(), "{notes:?}");
            assert!(!t.weapons.is_empty(), "{notes:?}");
            t
        }
        None => dc_dserver::rom_tables::RomTables::default(),
    };
    let table = tables.cone_table();
    assert_eq!(cone::table_sha256(table), cone::TABLE_SHA256);
    let rows = (!tables.weapons.is_empty()).then_some(&tables.weapons);
    let mut total = LinkResult::default();
    for (name, expect_links, path) in present {
        let d = decode_file(&path).expect("capture");
        let mut links: std::collections::BTreeMap<u32, Vec<(f64, UpFire)>> = Default::default();
        for m in d.msgs.iter().filter(|m| m.c2s) {
            let FromClient::Shell {
                shell_id: 0x82,
                body,
            } = read_from_client(&m.bits)
            else {
                continue;
            };
            links
                .entry(m.conn)
                .or_default()
                .push((m.t, parse_fire(&body)));
        }
        let mut real = 0;
        for (conn, shots) in &links {
            let start = seed_start(&d, *conn, shots[0].0);
            let mut link = LinkCone::default();
            link.set_seed(start.unwrap_or(0));
            let mut res = LinkResult::default();
            for (_, f) in shots {
                let bounds = rows.and_then(|r| r.cone_bounds(f.weapon_id));
                let c = cone::check(table, bounds, &mut link, f, cone::DEFAULT_TOLERANCE_RAD);
                res.pellets += 1;
                match c.verdict {
                    ConeVerdict::Ok => res.ok += 1,
                    ConeVerdict::Off => res.off += 1,
                    ConeVerdict::Na => res.na += 1,
                    ConeVerdict::Unchecked => {}
                }
                if c.verdict == ConeVerdict::Ok && c.bounds.is_some() {
                    res.bounded += 1;
                }
                if c.expected_stat != f.stat {
                    res.skew += 1;
                }
            }
            // no link is ever cone_off: the real client passes, a client without the table never conforms
            assert_eq!(res.off, 0, "{name} conn {conn}: cone_off pellets {res:?}");
            // a link is the real client's when it drew from the table (DeCombat bots shoot along the aim, dc-client
            // uses its own disk cone, CLI-31: their links never conform)
            if !link.conforming {
                println!("{name} conn {conn}: not the 2009 cone ({res:?})");
                continue;
            }
            real += 1;
            assert_eq!(
                res.ok, res.pellets,
                "{name} conn {conn}: every pellet cone_ok {res:?}"
            );
            if start.is_some() {
                // the counter starts at the server's own_value and the server's expectation is exact
                assert_eq!(res.skew, 0, "{name} conn {conn}: stat skew {res:?}");
            }
            if rows.is_some() {
                assert!(
                    res.bounded > 0,
                    "{name} conn {conn}: no pellet checked against its row"
                );
            }
            println!(
                "{name} conn {conn}: {} pellets, {} cone_ok ({} within the row bounds), stat skew {}, seed start {start:?}",
                res.pellets, res.ok, res.bounded, res.skew
            );
            total.pellets += res.pellets;
            total.ok += res.ok;
            total.bounded += res.bounded;
            total.skew += res.skew;
        }
        assert_eq!(real, expect_links, "{name}: real-client links");
    }
    println!(
        "total: {} real-client pellets, {} cone_ok, {} within the row bounds, stat skew {}",
        total.pellets, total.ok, total.bounded, total.skew
    );
    assert!(total.ok > 0);
    assert_eq!(total.ok, total.pellets);
}
