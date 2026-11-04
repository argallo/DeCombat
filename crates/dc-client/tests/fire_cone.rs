//! dc-client's shot cone against the 2009 client and against the reference replay of
//! `crates/dc-dserver/tests/fire_cone_cap.rs` (WEAPON_FIRE_2009_addendum_cone.md):
//! - `client_cone_reproduces_the_real_client_shots` (capture- and ROM-gated): every pellet the real 2009 client sent
//!   in the replay captures (rust7 with its zoomed shots, and the four reference sessions) is computed again by
//!   `dc_client::cone` from the message's `aim`, `frac`, `shot_scale`, zoom bit and the weapon row, with the shot
//!   counter seeded from the server's 0x7d `own_value` (and 0xda kind 1) by dc-client's own readers; the counter after
//!   each pellet must equal the sent `stat`, and some spread inside the sent `frac` byte must give the sent direction.
//! - `burst_replays_through_the_reference` (headless, no ROM): bursts of dc-client's weapon state (an 8-pellet weapon,
//!   an automatic one, zoomed and moving, a counter set near the table end so the reads wrap) are encoded, decoded
//!   and replayed by the reference code: every pellet matches, every half-width lies in the row interval of its `frac`.
//!
//! Captures are read in place and never copied (REFERENCE_TESTING.md §5); the ROM is read at run time.
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1 .. AC-5, AC-7; GAME_MESSAGES_2009_C2S.md §4.1;
// GAME_MESSAGES_2009_S2C.md 0x7d / 0xda rows

mod cone_ref;

use dc_botclient::capdecode::decode_file;
use dc_botclient::weapon::{read_fire, FireReport, WeaponGrant};
use dc_bute::ButeDoc;
use dc_client::arms::{self, Aim, ArmsConfig, Arsenal, WeaponCommands, WeaponTable};
use dc_client::cone::{self, ConeInputs, ShotCounter, ShotTable};
use dc_rez::RezVfs;

/// The replay captures of the reference test, with the number of real-client links in each.
const CAPTURES: [(&str, usize); 5] = [
    ("20261025_000014_tdm1", 1),
    ("20261025_001409_tdm2", 1),
    ("20261025_002236_tdm4", 1),
    ("20261025_065016_tdm6", 1),
    ("20261025_153705_rust7", 1),
];

#[test]
fn client_cone_reproduces_the_real_client_shots() {
    let present: Vec<_> = CAPTURES
        .iter()
        .filter_map(|&(c, n)| cone_ref::capture_named(c).map(|p| (c, n, p)))
        .collect();
    if present.is_empty() {
        println!("skipped: captures absent");
        return;
    }
    let rom = dc_rom::rom_or_skip!();
    let weapons = WeaponTable::open(&rom).expect("ROM weapon table");
    let table = ShotTable::from_rom(&RezVfs::new(&rom)).expect("ROM random table");
    // the client's table is the ROM entry, and the generator gives the same values
    let generated = ShotTable::generated();
    assert_eq!(table.len(), generated.len());
    assert!((0..table.len())
        .all(|i| table.get(i).map(f32::to_bits) == generated.get(i).map(f32::to_bits)));
    let reference = cone_ref::rebuilt_table();
    let doc = ButeDoc::from_vfs(
        &RezVfs::new(&rom),
        arms::ATTRIBUTES_REZ,
        arms::WEAPONS_ENTRY,
    )
    .expect("weapons entry");
    let (mut pellets, mut matched, mut bitwise, mut zoomed) = (0usize, 0usize, 0usize, 0usize);
    let mut rust7 = 0usize;
    for (name, expect_links, path) in present {
        let d = decode_file(&path).expect("capture");
        let mut real = 0;
        for (conn, shots) in cone_ref::fires(&d) {
            // a real-client link: its pellets replay through the reference and some of them are tilted
            let hs: Vec<Option<i64>> = shots
                .iter()
                .map(|(_, f)| cone_ref::replay(&reference, f))
                .collect();
            if !hs.iter().any(|h| h.is_some_and(|h| h > 0)) {
                continue;
            }
            real += 1;
            // the counter as dc-client keeps it: 0x7d sets it, 0xda kind 1 adds to it
            let mut counter = ShotCounter::default();
            for (shell, msg) in cone_ref::shells_before(&d, conn, shots[0].0) {
                let mut r = msg.reader();
                r.read_u8();
                r.read_u8();
                match shell {
                    arms::down::MATCH_SETTINGS => {
                        if let Some(v) = arms::match_settings_shot_seed(&mut r) {
                            counter.set(v);
                        }
                    }
                    arms::down::PLAYER_POINTS => {
                        if let Some(v) = arms::player_points_shot_seed(&mut r) {
                            counter.add(v);
                        }
                    }
                    _ => {}
                }
            }
            let mut in_shot = 0u32;
            for (i, (_, f)) in shots.iter().enumerate() {
                pellets += 1;
                let row = weapons
                    .get(f.weapon_id)
                    .unwrap_or_else(|| panic!("{name}: row of weapon {}", f.weapon_id));
                let scale = f32::from_bits(f.shot_no);
                // shot_scale: the row's DynamicPerturbFactor, or the movement factor (binary default 1.0) above it
                // SPEC: addendum AC-3.2
                assert!(
                    scale == row.cone.scale || (row.cone.scale < 1.0 && scale == 1.0),
                    "{name} conn {conn} shot {i}: shot_scale {scale} for row factor {}",
                    row.cone.scale
                );
                // the pellets of one shot: VectorsPerRound (no mods)
                in_shot += 1;
                if f.last {
                    assert_eq!(
                        in_shot,
                        cone::pellets(row.pellets, &[None; 3]),
                        "{name} conn {conn} shot {i}: pellets of weapon {}",
                        f.weapon_id
                    );
                    in_shot = 0;
                }
                let start = counter;
                let mut hit = None;
                for k in 0..512u32 {
                    let spread = (f32::from(f.frac) + k as f32 / 512.0) / 255.0;
                    let c = ConeInputs {
                        aim: f.aim,
                        spread,
                        scale,
                        zoom: f.moving,
                        triple: f.wflag,
                    };
                    let mut probe = start;
                    let dir = cone::pellet(&row.cone, &c, &mut probe, &table);
                    let err = cone_ref::angle(dir, f.dir);
                    if err < 2e-5 {
                        let exact = dir.map(f32::to_bits) == f.dir.map(f32::to_bits);
                        let better = hit.is_none_or(|(_, e, x): (ShotCounter, f64, bool)| {
                            (exact && !x) || (exact == x && err < e)
                        });
                        if better {
                            hit = Some((probe, err, exact));
                        }
                    }
                }
                let Some((after, _, exact)) = hit else {
                    panic!(
                        "{name} conn {conn} shot {i}: weapon {} frac {} zoom {} stat {}: no spread in the frac byte gives the sent direction (reference h {:?})",
                        f.weapon_id, f.frac, f.moving, f.stat, hs[i]
                    );
                };
                counter = after;
                assert_eq!(counter.value, f.stat, "{name} conn {conn} shot {i}: stat");
                // the reference's interval check, with the row read by the reference
                if let (Some(h), Some(r)) = (hs[i], cone_ref::row_of(&doc, f.weapon_id)) {
                    let lo = cone_ref::half_width(
                        &r,
                        f.moving,
                        f.wflag,
                        scale,
                        f32::from(f.frac) / 255.0,
                    );
                    let hi = cone_ref::half_width(
                        &r,
                        f.moving,
                        f.wflag,
                        scale,
                        (f32::from(f.frac) + 0.999) / 255.0,
                    );
                    if let (Some(lo), Some(hi)) = (lo, hi) {
                        assert!(
                            lo <= h && h <= hi,
                            "{name} conn {conn} shot {i}: h {h} not in {lo}..{hi}"
                        );
                    }
                }
                matched += 1;
                bitwise += usize::from(exact);
                zoomed += usize::from(f.moving);
                if name.ends_with("rust7") {
                    rust7 += 1;
                }
            }
        }
        assert_eq!(real, expect_links, "{name}: real-client links");
    }
    println!(
        "dc-client cone: {matched} of {pellets} real-client pellets reproduced ({bitwise} bit for bit, {zoomed} zoomed; rust7 {rust7})"
    );
    assert_eq!(matched, pellets);
    assert!(pellets > 0);
}

// --------------------------------------------------------------------------------------------- headless burst

/// A made-up table (numbers are ours): weapon 3 = an 8-pellet weapon, weapon 5 = an automatic weapon with a zoomed
/// pair and a scale above 1, weapon 6 = a row whose scale is 0 (no cone unless a movement factor lifts it).
fn table() -> std::sync::Arc<WeaponTable> {
    let text = [
        "[Ammo0]",
        "Name = \"a\"",
        "MaxAmount = 99",
        "[Weapon3]",
        "AmmoName0 = \"a\"",
        "VectorsPerRound = 8",
        "ShotsPerClip = 50",
        "MinPerturb = 40",
        "MaxPerturb = 120",
        "PerturbIncSpeed = 2.0",
        "[Weapon5]",
        "AmmoName0 = \"a\"",
        "VectorsPerRound = 1",
        "ShotsPerClip = 50",
        "DynamicPerturbFactor = 1.3",
        "MinPerturb = 5",
        "MaxPerturb = 90",
        "ZoomedMinPerturb = 1",
        "ZoomedMaxPerturb = 12",
        "PerturbIncSpeed = 1.0",
        "[Weapon6]",
        "AmmoName0 = \"a\"",
        "ShotsPerClip = 50",
        "DynamicPerturbFactor = 0.0",
        "MinPerturb = 10",
        "MaxPerturb = 60",
    ]
    .join("\n");
    std::sync::Arc::new(WeaponTable::from_doc(&ButeDoc::parse(&text)))
}

fn grant(slot: u8, weapon: u16) -> WeaponGrant {
    WeaponGrant {
        weapon,
        ammo_kind: 0,
        ammo: 99.0,
        clip: None,
        class_pos: 0,
        slot,
        equip: false,
        parts: [-1; 4],
    }
}

fn burst(a: &mut Arsenal, from: u32, to: u32, yaw: f32, pitch: f32) -> Vec<FireReport> {
    let mut out = Vec::new();
    let mut t = from;
    while t < to {
        let aim = Aim {
            origin: [10.0, 40.0, -5.0],
            yaw: yaw + t as f32 * 1e-4,
            pitch,
            control: 0,
        };
        let cmd = WeaponCommands {
            fire: true,
            ..WeaponCommands::default()
        };
        for m in a.step(t, &cmd, &aim, &[], None) {
            if m.shell == arms::up::FIRE {
                out.push(read_fire(&mut m.body.reader()).expect("0x82"));
            }
        }
        t += 16;
    }
    out
}

fn select(a: &mut Arsenal, slot: u8, at: u32) -> u32 {
    let aim = Aim {
        origin: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        control: 0,
    };
    let cmd = WeaponCommands {
        select: Some(slot),
        ..WeaponCommands::default()
    };
    a.step(at, &cmd, &aim, &[], None);
    let mut t = at + 16;
    while t < at + 700 {
        a.step(t, &WeaponCommands::default(), &aim, &[], None);
        t += 16;
    }
    t
}

#[test]
fn burst_replays_through_the_reference() {
    let reference = cone_ref::rebuilt_table();
    let doc = ButeDoc::parse(
        &[
            "[Weapon3]",
            "MinPerturb = 40",
            "MaxPerturb = 120",
            "[Weapon5]",
            "MinPerturb = 5",
            "MaxPerturb = 90",
            "ZoomedMinPerturb = 1",
            "ZoomedMaxPerturb = 12",
            "[Weapon6]",
            "MinPerturb = 10",
            "MaxPerturb = 60",
        ]
        .join("\n"),
    );
    let mut a = Arsenal::new(ArmsConfig {
        table: Some(table()),
        ..ArmsConfig::default()
    });
    a.on_grant(&grant(0, 3));
    a.on_grant(&grant(1, 5));
    a.on_grant(&grant(2, 6));
    a.on_life(0, 1);
    // the counter near the table end: the reads wrap with the store-back reduction
    let n = cone::TABLE_LEN as u32;
    a.on_shot_seed(n - 37);
    a.on_shot_seed_add(4);
    let mut expect = n - 33;
    let mut reports = Vec::new();
    // weapon 3 standing (8 pellets per shot)
    let t = select(&mut a, 0, 16);
    reports.extend(burst(&mut a, t, t + 600, 0.4, 0.1));
    // weapon 5 moving, then zoomed
    let t = select(&mut a, 1, t + 700);
    a.set_motion(dc_client::spread::Motion {
        controls: dc_sim::player::bits::ANY_MOVE,
        speed_frac: 1.0,
        ..Default::default()
    });
    reports.extend(burst(&mut a, t, t + 500, -1.2, -0.3));
    a.set_zoom(true);
    reports.extend(burst(&mut a, t + 500, t + 900, 2.0, 0.05));
    a.set_zoom(false);
    // weapon 6: scale 0 standing (the gate refuses: no draw), lifted to the walk factor 1.0 while moving
    let t = select(&mut a, 2, t + 1000);
    a.set_motion(dc_client::spread::Motion::default());
    let still = burst(&mut a, t, t + 300, 0.0, 0.0);
    a.set_motion(dc_client::spread::Motion {
        controls: dc_sim::player::bits::ANY_MOVE,
        ..Default::default()
    });
    let moving = burst(&mut a, t + 300, t + 600, 0.0, 0.0);
    assert!(!still.is_empty() && !moving.is_empty());
    let (mut tilted, mut zoomed, mut wrapped) = (0, 0, false);
    for f in reports.iter().chain(&moving) {
        let h = cone_ref::replay(&reference, f)
            .unwrap_or_else(|| panic!("pellet does not replay: {f:?}"));
        tilted += usize::from(h > 0);
        zoomed += usize::from(f.moving);
        // the reads of this pellet: the counter before, reduced at the table size, plus two
        let mut c = expect;
        for _ in 0..2 {
            if c >= n {
                c %= n;
                wrapped = true;
            }
            c += 1;
        }
        expect = c;
        assert_eq!(
            f.stat, expect,
            "stat = the counter after the pellet's two reads"
        );
        let row = cone_ref::row_of(&doc, f.weapon_id).expect("row");
        let scale = f32::from_bits(f.shot_no);
        let lo = cone_ref::half_width(&row, f.moving, f.wflag, scale, f32::from(f.frac) / 255.0)
            .unwrap();
        let hi = cone_ref::half_width(
            &row,
            f.moving,
            f.wflag,
            scale,
            (f32::from(f.frac) + 0.999) / 255.0,
        )
        .unwrap();
        assert!(lo <= h && h <= hi, "h {h} not in {lo}..{hi}: {f:?}");
    }
    // weapon 6 standing: scale 0, nothing drawn, the counter does not move, the shot goes along the aim
    for f in &still {
        assert_eq!(f32::from_bits(f.shot_no), 0.0);
        assert_eq!(cone_ref::replay(&reference, f), Some(0));
    }
    assert!(still.windows(2).all(|w| w[0].stat == w[1].stat));
    for f in &moving {
        assert_eq!(f32::from_bits(f.shot_no), 1.0, "raised to the walk factor");
    }
    // shapes: 8 pellets per shot of weapon 3, the zoom bit while zoomed, scale 1.3 for weapon 5
    let w3: Vec<_> = reports.iter().filter(|f| f.weapon_id == 3).collect();
    assert!(w3.len() >= 16 && w3.len() % 8 == 0, "{}", w3.len());
    assert!(w3
        .chunks(8)
        .all(|c| c[7].last && c[..7].iter().all(|f| !f.last)));
    assert!(reports
        .iter()
        .filter(|f| f.weapon_id == 5)
        .all(|f| f32::from_bits(f.shot_no) == 1.3f32));
    assert!(zoomed > 0 && wrapped && tilted > reports.len() / 2);
    println!(
        "burst: {} pellets replayed through the reference ({tilted} tilted, {zoomed} zoomed, counter wrapped), {} untilted at scale 0",
        reports.len() + moving.len(),
        still.len()
    );
}

#[test]
fn counter_writers_read_the_server_bodies() {
    use dc_wire::udp::BitWriter;
    // 0x7d: [caption zstr8][unused u8][own_team i8][own_value u32][goal u8]...
    let mut w = BitWriter::new();
    for b in b"Host\0" {
        w.write_u8(*b);
    }
    w.write_u8(1)
        .write_s8(0)
        .write_u32(0x0001_2345)
        .write_u8(100);
    assert_eq!(
        arms::match_settings_shot_seed(&mut w.reader()),
        Some(0x0001_2345)
    );
    let mut w = BitWriter::new();
    w.write_u8(0).write_u8(1);
    assert_eq!(
        arms::match_settings_shot_seed(&mut w.reader()),
        None,
        "short body"
    );
    // 0xda kind 1 [delta u32]; another kind does not touch the counter
    let mut w = BitWriter::new();
    w.write_u8(1).write_u32(77);
    assert_eq!(arms::player_points_shot_seed(&mut w.reader()), Some(77));
    let mut w = BitWriter::new();
    w.write_u8(2).write_u32(77);
    assert_eq!(arms::player_points_shot_seed(&mut w.reader()), None);
}
