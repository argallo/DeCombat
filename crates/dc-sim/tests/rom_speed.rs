//! ROM-gated (skipped when CA_ROM is unset): the speed rows of the user's own tables against the speeds the real
//! 2009 client showed. The rows are read at run time; the test keeps only weapon NUMBERS and the speeds measured in
//! our captures of the real client (OBSERVED), never ROM values.
//!
//! The capture replay (`capture_replay_matches_the_rom_speed_rule`) is gated as well: set `DC_CAPTURE_ANALYSIS` to
//! one or more `analysis` folders (capanalyze output of a real-client capture: `updates.csv`, `events.csv`),
//! separated by `;`. It measures the run / sprint plateaus per held weapon from the client's own movement updates
//! and compares their ratios with the ROM rule.
// SPEC: MOVEMENT_2009.md §3, §7
// EVIDENCE: cshell+0x5c270, cshell+0x1fd390, cshell+0x1e4760, cshell+0x1dfc0a; DISASM: cshell+0x17dbd5
// OBSERVED: captures 20261025_000014_tdm1, 20261025_002236_tdm4, 20261025_065016_tdm6,
// 20261025_095358_rust4, 20261025_125432_rust6 (the real 2009 client; median reported speed of the on-ground,
// forward, steady updates per held weapon, capanalyze updates.csv; body row 0, the default of both servers)
#![cfg(feature = "rom")]

use std::collections::BTreeMap;
use std::path::PathBuf;

mod common;

use dc_sim::{Commands, Loadout, MovementParams, Simulation, SpeedTable, TickInput};

fn rom() -> Option<PathBuf> {
    let r = std::env::var("CA_ROM").ok()?;
    let p = PathBuf::from(r.trim());
    p.is_dir().then_some(p)
}

fn table() -> Option<SpeedTable> {
    let rom = rom()?;
    let vfs = dc_rez::RezVfs::new(&rom);
    Some(SpeedTable::from_vfs(&vfs).expect("speed rows from the ROM"))
}

/// The run and sprint caps of `weapon` with body row 0.
fn caps(t: &SpeedTable, weapon: u16) -> (f32, f32) {
    let p = MovementParams::default();
    let m = t.mods(&Loadout {
        weapon: Some(weapon),
        body: Some(0),
        ..Loadout::default()
    });
    let k = 1.0 - m.penalty.clamp(-1.0, 1.0);
    let run = k * p.run_forward;
    let sprint = k * (m.sprint_add + 1.0) * p.run_forward * m.sprint_mul.unwrap();
    (run, sprint)
}

/// (weapon, run, sprint) medians of the real client's reported speed (OBSERVED, see the header).
const OBSERVED: [(u16, f32, f32); 6] = [
    (0, 255.4, 356.3),
    (145, 227.0, 316.6),
    (164, 246.9, 344.4),
    (7, 246.9, 344.4),
    (143, 246.9, 344.4),
    (27, 187.0, 261.1),
];

#[test]
fn rom_rows_give_the_2009_speed_per_weapon() {
    let Some(t) = table() else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    assert!(!t.is_empty() && t.weapons.len() > 100 && !t.bodies.is_empty());
    let (run0, sprint0) = caps(&t, 0);
    for (w, run, sprint) in OBSERVED {
        let (r, s) = caps(&t, w);
        // the weapon's share: the ratio to the zero-penalty weapon is the 2009 one within 0.5 %
        let want = run / OBSERVED[0].1;
        assert!(
            (r / run0 - want).abs() < 0.005,
            "weapon {w}: run ratio {} vs observed {want}",
            r / run0
        );
        let want = sprint / OBSERVED[0].2;
        assert!(
            (s / sprint0 - want).abs() < 0.005,
            "weapon {w}: sprint ratio {} vs observed {want}",
            s / sprint0
        );
        // the absolute caps: within 1 % (the 2009 client at 1-2 ms frames reports 0.4 % below the cap)
        assert!((r / run - 1.0).abs() < 0.01, "weapon {w}: run {r} vs {run}");
        assert!(
            (s / sprint - 1.0).abs() < 0.01,
            "weapon {w}: sprint {s} vs {sprint}"
        );
        // sprint / run is the character's factor, the same for every weapon
        assert!(
            (s / r - sprint / run).abs() < 0.01,
            "weapon {w}: sprint / run {} vs {}",
            s / r,
            sprint / run
        );
    }
    // the knife of the default loadout has no penalty of its own and sprints faster than the rifles
    let knife = t
        .weapons
        .iter()
        .filter(|(_, &v)| v == 0.0)
        .map(|(&k, _)| k)
        .find(|&k| k > 0)
        .expect("a zero-penalty weapon row besides 0");
    assert!(caps(&t, knife).1 > caps(&t, 145).1);
}

/// One analysis folder's per-(weapon, stance) list of reported on-ground speeds.
fn plateaus(dir: &std::path::Path) -> BTreeMap<(u16, bool), Vec<f32>> {
    let ups = std::fs::read_to_string(dir.join("updates.csv")).expect("updates.csv");
    let evs = std::fs::read_to_string(dir.join("events.csv")).expect("events.csv");
    // the held weapon over time: c2s weapon_change rows (column 4 = the new weapon)
    let mut held: Vec<(f64, u16)> = Vec::new();
    for l in evs.lines().skip(1) {
        let c: Vec<&str> = l.splitn(5, ',').collect();
        if c.len() >= 4 && c[1] == "c2s" && c[2] == "weapon_change" {
            if let (Ok(t), Ok(w)) = (c[0].parse::<f64>(), c[3].parse::<u16>()) {
                held.push((t, w));
            }
        }
    }
    let mut lines = ups.lines();
    let head: Vec<&str> = lines.next().expect("header").split(',').collect();
    let col = |n: &str| head.iter().position(|h| *h == n).expect(n);
    let (ct, cs, cg, cf, cb, cc, cr, csp) = (
        col("time_s"),
        col("speed_xz"),
        col("on_ground"),
        col("FWD"),
        col("BACK"),
        col("CROUCH"),
        col("RUN"),
        col("SPRINT"),
    );
    let rows: Vec<Vec<&str>> = lines.map(|l| l.split(',').collect()).collect();
    let mut out: BTreeMap<(u16, bool), Vec<f32>> = BTreeMap::new();
    for pair in rows.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let on = |r: &Vec<&str>, c: usize| r.get(c) == Some(&"1");
        if !(on(a, cg) && on(b, cg) && on(a, cf) && on(b, cf)) {
            continue;
        }
        if on(a, cb) || on(b, cb) || on(a, cc) || on(b, cc) {
            continue;
        }
        let sprint = on(a, csp) && on(b, csp);
        let run = on(a, cr) && on(b, cr) && !on(a, csp) && !on(b, csp);
        if !(sprint || run) {
            continue;
        }
        let (Ok(t), Ok(v)) = (b[ct].parse::<f64>(), b[cs].parse::<f32>()) else {
            continue;
        };
        let Some(&(_, w)) = held.iter().rev().find(|(ht, _)| *ht <= t) else {
            continue;
        };
        out.entry((w, sprint)).or_default().push(v);
    }
    out
}

fn median(v: &mut [f32]) -> f32 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

#[test]
fn capture_replay_matches_the_rom_speed_rule() {
    let (Some(t), Ok(dirs)) = (table(), std::env::var("DC_CAPTURE_ANALYSIS")) else {
        eprintln!("CA_ROM or DC_CAPTURE_ANALYSIS not set: skipped");
        return;
    };
    let mut checked = 0;
    for d in dirs.split(';').filter(|d| !d.trim().is_empty()) {
        let mut m = plateaus(std::path::Path::new(d.trim()));
        // the reference: the held weapon with the most run samples in this capture
        let Some(base) = m
            .iter()
            .filter(|((_, s), v)| !*s && v.len() >= 20)
            .max_by_key(|(_, v)| v.len())
            .map(|((w, _), _)| *w)
        else {
            continue;
        };
        let base_run = median(m.get_mut(&(base, false)).unwrap());
        let base_sprint = m
            .get_mut(&(base, true))
            .filter(|v| v.len() >= 15)
            .map(|v| median(v));
        let (rom_run, rom_sprint) = caps(&t, base);
        // a capture whose character is not body row 0 shifts every weapon by the same body term: the DIFFERENCE
        // to the capture's own reference weapon is the weapon rows' share, `-(p_w - p_ref) x stance speed`
        for ((w, sprint), v) in m.iter_mut() {
            if v.len() < 15 {
                continue;
            }
            let got = median(v);
            let (r, s) = caps(&t, *w);
            let (got_d, rom_d) = match (*sprint, base_sprint) {
                (false, _) => (got - base_run, r - rom_run),
                (true, Some(bs)) => (got - bs, s - rom_sprint),
                (true, None) => continue,
            };
            eprintln!(
                "{d}: weapon {w} {} n={} median {got:.1} difference to weapon {base} {got_d:.1} rom {rom_d:.1}",
                if *sprint { "sprint" } else { "run" },
                v.len()
            );
            assert!(
                (got_d - rom_d).abs() <= 4.0 + 0.1 * rom_d.abs(),
                "{d}: weapon {w} sprint {sprint}: {got_d} vs rom {rom_d}"
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no capture plateau compared");
}

/// The sprint gauge of body row 0 (the default body of both servers) against the real client's sprints in tdm1,
/// run through the gauge law at 1 ms steps (the 2009 client's frame time): two sprints from a rested gauge (13.2 s
/// and 11.2 s of rest) ended by the stamina path (`0xca (0, 0)`) after 3.312 s and 3.313 s; three sprints after a
/// stamina end, the sprint key let go (`(0, 1)`) and a short rest lasted 0.250 s (rest 0.328 s), 0.531 s (rest
/// 0.688 s) and 0.516 s (rest 0.656 s), each ended by the stamina path again. The rates are read from the ROM at
/// run time; only these OBSERVED times are kept.
// OBSERVED: capture 20261025_000014_tdm1 (the human connection's 0xca notices, decoded with
// capanalyze + capdecode: (1, 0) 173.578 -> (0, 0) 176.890; (1, 0) 195.218 -> (0, 0) 198.531; (0, 1) 199.515 ->
// (1, 0) 199.843 -> (0, 0) 200.093; (0, 1) 200.437 -> (1, 0) 201.125 -> (0, 0) 201.656; (0, 1) 202.015 -> (1, 0)
// 202.671 -> (0, 0) 203.187)
// DISASM: cshell+0x1d33d0 (rates); EVIDENCE: cshell+0x95850 (law), cshell+0x1ff980 (a sprint needs the value
// truncated above 0)
#[test]
fn rom_gauge_matches_the_2009_sprint_times() {
    use dc_sim::{GaugeMode, Stamina};
    let Some(t) = table() else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    let l = Loadout {
        weapon: Some(0),
        body: Some(0),
        ..Loadout::default()
    };
    let r = t
        .stamina(&l)
        .expect("body row 0 has a figure row with Maxstamina");
    assert!(r.max > 0 && r.drain > 0 && r.regen > 0, "{r:?}");
    let ms = 0.001f32;
    // seconds of sprint from `start` (None = full) after `rest` seconds of refill
    let sprint = |start: Option<f32>, rest: f32| {
        let mut g = Stamina {
            value: start,
            mode: GaugeMode::Rest,
        };
        for _ in 0..(rest / ms).round() as u32 {
            g.advance(Some(r), ms);
        }
        let mut n = 0u32;
        g.mode = GaugeMode::Sprint;
        while g.can_sprint(Some(r)) && n < 60_000 {
            g.advance(Some(r), ms);
            n += 1;
        }
        n as f32 * ms
    };
    let cases = [
        (None, 13.2, 3.312),
        (None, 11.2, 3.313),
        (Some(0.0), 0.328, 0.250),
        (Some(0.0), 0.688, 0.531),
        (Some(0.0), 0.656, 0.516),
    ];
    for (start, rest, seen) in cases {
        let got = sprint(start, rest);
        println!("rest {rest:.3} s: sprint {got:.3} s (2009 {seen:.3} s)");
        // the notices are stamped by the capture at 15.6 ms resolution, the release / press by a human
        assert!(
            (got - seen).abs() < 0.05,
            "rest {rest}: {got} s vs {seen} s"
        );
    }
}

/// Mean horizontal speed (displacement / time) over the last second of `secs` of forward running with `weapon`
/// held (body row 0) on the fixed step `hz`.
fn moved_speed(t: &SpeedTable, weapon: u16, hz: u32, secs: f32) -> f32 {
    let p = MovementParams::default();
    let w = common::flat_world();
    let s0 = common::settled(&p, &w, 1.0 / hz as f32);
    let mut sim = Simulation::new(p, &w, s0, hz);
    let input = TickInput {
        commands: Commands {
            forward: true,
            run: true,
            ..Commands::default()
        },
        mods: t.mods(&Loadout {
            weapon: Some(weapon),
            body: Some(0),
            ..Loadout::default()
        }),
        ..TickInput::default()
    };
    let ticks = (secs * hz as f32) as u64;
    let mut mark = sim.state.pos;
    for k in 0..ticks {
        sim.tick(&input);
        if k == ticks - 1 - hz as u64 {
            mark = sim.state.pos;
        }
    }
    let d = sim.state.pos - mark;
    (d.x * d.x + d.z * d.z).sqrt()
}

/// The body MOVES at the 2009 run speed of the held weapon (the real client at its 1-2 ms frames moved at the cap,
/// MOVEMENT_2009 §6.3 / CLI-MOVE-1 note), not only reports it: weapon 27 (sniper) at 187.0, the knife at 255.4.
/// At the 60 Hz step the cap-before-acceleration order alone moved about 9 % (sniper) / 5 % (knife) too fast.
// DC-DEFAULT: SIM-22 (grounded displacement capped at the target speed)
#[test]
fn rom_the_body_moves_at_the_2009_run_speed_of_its_weapon() {
    let Some(t) = table() else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    for hz in [60u32, 144] {
        for (w, run, _) in [OBSERVED[5], OBSERVED[0]] {
            let (cap, _) = caps(&t, w);
            let got = moved_speed(&t, w, hz, 3.0);
            eprintln!("{hz} Hz weapon {w}: moved {got} cap {cap} 2009 {run}");
            assert!(
                (got / cap - 1.0).abs() < 0.005,
                "{hz} Hz weapon {w}: moved {got} vs cap {cap}"
            );
            assert!(
                (got / run - 1.0).abs() < 0.01,
                "{hz} Hz weapon {w}: moved {got} vs 2009 {run}"
            );
        }
    }
}
