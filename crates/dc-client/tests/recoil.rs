//! The view recoil ([`dc_client::recoil`]) and the hit jar ([`dc_client::camfx`]) against the real 2009 client.
//!
//! - ROM + capture tests (skipped without `CA_ROM` or the sweep folder): every burst of the experiment-A weapon sweep
//!   (`capture sweep/20261025/spread_recoil.csv` and the `analysis/shots.csv` of each capture it
//!   names) is replayed with its own shot times and compared with the 2009 client's 0x82 `aim` yaw and pitch. The
//!   side of each yaw kick comes from the low bit of the 2009 client's millisecond clock, whose phase is not in the
//!   capture: the replay therefore takes, at every kick that may change side (the rule: only when the pending yaw
//!   exceeds `MaxRandomWag`), the choice that fits the next shots, and never changes side where the rule forbids it.
//!   Checked: the yaw stays exactly at the burst's first value until the shot the rule names (the pending pitch
//!   passes `MaxRandomPitch`), every yaw within the stated tolerance afterwards, and the side changes happen at about
//!   half of the kicks that allowed one (a coin of the clock's low bit).
//! - Capture test (skipped without the capture folder): the real client of capture rust6 (against our server, 52
//!   bullet hits) shot right after hits from the front without a jar of its shot direction: its "Reality" option was
//!   off, the 2009 default, which is DeCombat's default.
//!
//! No ROM value is asserted or stored: the rows come from the user's ROM at run time, the expected angles from the
//! captures.
// OBSERVED: capture sweep/20261025 (PROVEN-CAPTURE 0x82 `aim` per shot),
// capture 20261025_125432_rust6/analysis (shots.csv, events.csv)

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dc_client::arms::WeaponTable;
use dc_client::recoil::CameraRecoil;
use dc_client::spread::{clock, RecoilRow};

const SWEEP_CSV: &str = "sweep/20261025/spread_recoil.csv";
const RUST6: &str = "20261025_125432_rust6/analysis";

fn find(rel: &str, env: &str) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(env).map(PathBuf::from) {
        return p.exists().then_some(p);
    }
    let p = PathBuf::from(std::env::var_os("DC_CAPTURES")?).join(rel);
    p.exists().then_some(p)
}

/// Rows of a CSV file as name -> value maps (quoted fields with commas are kept whole).
fn read_csv(path: &Path) -> Vec<BTreeMap<String, String>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let split = |l: &str| -> Vec<String> {
        let (mut out, mut cur, mut q) = (Vec::new(), String::new(), false);
        let mut it = l.chars().peekable();
        while let Some(ch) = it.next() {
            match ch {
                '"' if q && it.peek() == Some(&'"') => {
                    cur.push('"');
                    it.next();
                }
                '"' => q = !q,
                ',' if !q => out.push(std::mem::take(&mut cur)),
                c => cur.push(c),
            }
        }
        out.push(cur);
        out
    };
    let mut lines = text.lines();
    let Some(head) = lines.next() else {
        return Vec::new();
    };
    let names = split(head);
    lines
        .filter(|l| !l.trim().is_empty())
        .map(|l| names.iter().cloned().zip(split(l)).collect())
        .collect()
}

fn f(r: &BTreeMap<String, String>, k: &str) -> Option<f64> {
    r.get(k).and_then(|x| x.trim().parse().ok())
}

/// One volley: client ms, the 0x82 `aim` yaw and pitch (degrees, capanalyze: pitch positive up).
#[derive(Debug, Clone, Copy)]
struct Shot {
    ms: f64,
    yaw: f64,
    pitch: f64,
}

fn sweep_bursts(csv: &Path) -> BTreeMap<u16, Vec<Vec<Shot>>> {
    let root = csv
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent());
    let mut caps: BTreeMap<u16, String> = BTreeMap::new();
    for r in read_csv(csv) {
        if let (Some(w), Some(c)) = (
            r.get("weapon").and_then(|w| w.parse().ok()),
            r.get("capture"),
        ) {
            caps.entry(w).or_insert_with(|| c.clone());
        }
    }
    let mut out = BTreeMap::new();
    for (w, cap) in caps {
        let Some(root) = root else { break };
        let mut bursts: BTreeMap<u32, Vec<Shot>> = BTreeMap::new();
        for s in read_csv(&root.join(&cap).join("analysis").join("shots.csv")) {
            let first = s.get("pellet").is_none_or(|p| p.is_empty() || p == "0");
            if s.get("weapon").and_then(|x| x.parse::<u16>().ok()) != Some(w) || !first {
                continue;
            }
            let (Some(b), Some(ms), Some(yaw), Some(pitch)) = (
                s.get("burst").and_then(|x| x.parse::<u32>().ok()),
                f(&s, "client_ms"),
                f(&s, "shot_aim_yaw_deg"),
                f(&s, "shot_aim_pitch_deg"),
            ) else {
                continue;
            };
            bursts.entry(b).or_default().push(Shot { ms, yaw, pitch });
        }
        out.insert(w, bursts.into_values().filter(|b| b.len() >= 3).collect());
    }
    out
}

fn rom_table() -> Option<WeaponTable> {
    WeaponTable::open(&dc_rom::test_rom()?).ok()
}

fn wrap(d: f64) -> f64 {
    (d + 180.0).rem_euclid(360.0) - 180.0
}

/// The replay state: the view's offsets (radians) and the recoil.
#[derive(Clone)]
struct Sim {
    cam: CameraRecoil,
    yaw: f64,
    pitch: f64,
}

impl Sim {
    /// The drain up to `t`, then the kick at `t`; the side stays as `side` decides (`None`: no change of side).
    fn shot(&mut self, r: &RecoilRow, t: f64, side: Option<bool>) -> (bool, bool) {
        let before = self.cam.wag_right;
        let wags = self.cam.wags;
        let k = self.cam.kick(r, t, clock::TICK_MS, false);
        self.yaw += k.yaw as f64;
        self.pitch += k.pitch as f64;
        let wagged = self.cam.wags > wags;
        let allowed = wagged && r.wag_threshold < self.cam.wag_acc.abs();
        self.cam.wag_right = match side {
            Some(s) if allowed => s,
            _ => before,
        };
        (wagged, allowed)
    }

    fn drain(&mut self, r: &RecoilRow, t: f64) {
        let d = self.cam.drain(r, t, clock::TICK_MS, false);
        self.yaw += d.yaw as f64;
        self.pitch += d.pitch as f64;
    }
}

/// Our yaw (degrees, relative to the first shot) at every shot of `b`, the side choices made where the rule allows
/// one, and (allowed, changed) counts.
fn fit_burst(
    r: &RecoilRow,
    b: &[Shot],
    start_right: bool,
    phase: f64,
) -> (Vec<f64>, Vec<f64>, usize, usize) {
    let base = 100_000.0 + phase;
    let t0 = b[0].ms;
    let mut s = Sim {
        cam: CameraRecoil::default(),
        yaw: 0.0,
        pitch: 0.0,
    };
    s.cam.wag_right = start_right;
    s.drain(r, base + t0 - 1500.0);
    let obs = |i: usize| wrap(b[i].yaw - b[0].yaw);
    let (mut ys, mut ps) = (Vec::new(), Vec::new());
    let (mut allowed_n, mut changed_n) = (0, 0);
    for i in 0..b.len() {
        let t = base + b[i].ms;
        s.drain(r, t);
        ys.push(s.yaw.to_degrees());
        ps.push(s.pitch.to_degrees());
        // the side for the next kicks: try both where the rule lets this kick change it, keep the one that fits the
        // two shots after it best
        let keep = s.cam.wag_right;
        let mut trial = s.clone();
        let (_, allowed) = trial.shot(r, t, None);
        if !allowed {
            s.shot(r, t, None);
            continue;
        }
        allowed_n += 1;
        let cost = |side: bool| -> f64 {
            let mut c = s.clone();
            c.shot(r, t, Some(side));
            let mut e = 0.0;
            for (j, sj) in b.iter().enumerate().take(i + 3).skip(i + 1) {
                let tj = base + sj.ms;
                c.drain(r, tj);
                e += (c.yaw.to_degrees() - obs(j)).abs();
                let hold = c.cam.wag_right;
                c.shot(r, tj, Some(hold));
            }
            e
        };
        let side = if cost(!keep) < cost(keep) {
            !keep
        } else {
            keep
        };
        changed_n += usize::from(side != keep);
        s.shot(r, t, Some(side));
    }
    (ys, ps, allowed_n, changed_n)
}

#[test]
fn rom_sweep_yaw_kick_matches_the_2009_view_yaw() {
    let (Some(csv), Some(table)) = (find(SWEEP_CSV, "DC_SWEEP_CSV"), rom_table()) else {
        println!("skipped: needs CA_ROM and the sweep folder");
        return;
    };
    let all = sweep_bursts(&csv);
    assert!(!all.is_empty(), "no bursts read from {}", csv.display());
    let (mut allowed_all, mut changed_all, mut kicked_weapons) = (0usize, 0usize, 0usize);
    let mut lines = Vec::new();
    let mut fails: Vec<String> = Vec::new();
    for (w, bursts) in &all {
        let row = table.spread.get(*w).cloned().unwrap_or_default().recoil;
        let mut worst = 0.0f64;
        let mut worst_pitch = 0.0f64;
        let mut first_ok = 0usize;
        let mut first_n = 0usize;
        let (mut allowed, mut changed) = (0, 0);
        for b in bursts {
            let n = b.len().min(30);
            let b = &b[..n];
            // the tick clock's phase (the drain's rate switch counts whole ticks) and the starting side are not in
            // the capture: the best of 16 phases and both sides
            let best = (0..32)
                .map(|k| fit_burst(&row, b, k % 2 == 1, (k / 2) as f64 * clock::TICK_MS / 16.0))
                .min_by(|x, y| {
                    let e = |v: &[f64]| {
                        v.iter()
                            .enumerate()
                            .map(|(i, y)| (y - wrap(b[i].yaw - b[0].yaw)).abs())
                            .fold(0.0, f64::max)
                    };
                    e(&x.0).total_cmp(&e(&y.0))
                })
                .expect("two fits");
            let (ys, ps, a, c) = best;
            allowed += a;
            changed += c;
            for (i, y) in ys.iter().enumerate() {
                worst = worst.max((y - wrap(b[i].yaw - b[0].yaw)).abs());
                // capanalyze writes the pitch positive upward; ours is positive downward
                worst_pitch = worst_pitch.max((ps[i] + (b[i].pitch - b[0].pitch)).abs());
            }
            // before the rule's first yaw kick the 2009 yaw does not move at all
            // (a kick drained away before the next shot is not visible at the shots)
            if let Some(k) = ys.iter().position(|y| y.abs() > 0.01) {
                first_n += 1;
                let still = (0..k).all(|i| wrap(b[i].yaw - b[0].yaw).abs() < 0.0015);
                let moved = wrap(b[k].yaw - b[0].yaw).abs() > 0.05;
                first_ok += usize::from(still && moved);
            }
        }
        if first_n > 0 {
            kicked_weapons += 1;
            allowed_all += allowed;
            changed_all += changed;
        }
        lines.push(format!(
            "{w}: yaw worst {worst:.2} deg, pitch worst {worst_pitch:.2} deg, first yaw kick at the rule's shot {first_ok}/{first_n}, side changes {changed}/{allowed}"
        ));
        // weapon 12's capture is the sweep's odd one (its spread bytes too: longer client frames, client-spread.md)
        let (ty, tp) = if *w == 12 { (0.6, 0.6) } else { (0.15, 0.35) };
        if worst > ty || worst_pitch > tp {
            fails.push(format!(
                "weapon {w}: yaw off by {worst:.2}, pitch by {worst_pitch:.2} degrees"
            ));
        }
        if first_ok != first_n {
            fails.push(format!(
                "weapon {w}: first yaw kick off the rule's shot ({first_ok}/{first_n})"
            ));
        }
    }
    println!("recoil yaw against the sweep:\n  {}", lines.join("\n  "));
    // the largest errors on the sweep of 2026-10-25 are noted in the report (client-recoil.md)
    assert!(fails.is_empty(), "{fails:#?}");
    assert!(
        kicked_weapons >= 10,
        "{kicked_weapons} weapons kicked the yaw"
    );
    // the side changes when the clock's low bit is odd: about half of the allowed kicks
    let share = changed_all as f64 / allowed_all.max(1) as f64;
    println!(
        "side changes {changed_all} of {allowed_all} allowed kicks ({:.0} %)",
        share * 100.0
    );
    assert!((0.3..=0.7).contains(&share), "{share}");
}

#[test]
fn rust6_real_client_shot_without_a_jar_after_front_hits() {
    let Some(dir) = find(RUST6, "DC_RUST6_ANALYSIS") else {
        println!("skipped: needs the rust6 capture analysis");
        return;
    };
    // the hits on the real client (0x74 to it, `damage` > 0), with the direction and the damage type ("zone" repeats
    // the byte the jar tests, capture analysis of the same messages)
    let hits: Vec<(f64, [f32; 3])> = read_csv(&dir.join("events.csv"))
        .into_iter()
        .filter(|r| r.get("event").map(String::as_str) == Some("s2c_damage"))
        .filter_map(|r| {
            let t = f(&r, "time_s")?;
            let js: serde_json::Value = serde_json::from_str(r.get("fields")?).ok()?;
            (js["damage"].as_u64()? > 0 && js["zone"].as_u64()? == 2).then_some(())?;
            let d: Vec<f32> = js["from_dir"]
                .as_str()?
                .split_whitespace()
                .filter_map(|x| x.parse().ok())
                .collect();
            Some((t, [*d.first()?, *d.get(1)?, *d.get(2)?]))
        })
        .collect();
    assert!(hits.len() >= 20, "{} hits", hits.len());
    let mut fx = dc_client::camfx::CameraFx::new(
        true,
        dc_client::camfx::DamageJar::from_flags(vec![false, false, true]),
    );
    let (mut n, mut worst) = (0usize, 0.0f64);
    for s in read_csv(&dir.join("shots.csv")) {
        if s.get("pellet").is_some_and(|p| p != "0") {
            continue;
        }
        let (Some(t), Some(dp), Some(ap), Some(ay), Some(frac)) = (
            f(&s, "time_s"),
            f(&s, "dir_pitch_deg"),
            f(&s, "shot_aim_pitch_deg"),
            f(&s, "shot_aim_yaw_deg"),
            f(&s, "frac"),
        ) else {
            continue;
        };
        let Some(&(th, d)) = hits.iter().rev().find(|h| h.0 <= t) else {
            continue;
        };
        // what the jar would do to this shot if the option had been on: shots after a hit that would have jarred
        // the pitch up by 0.9 degrees or more, with a narrow cone (a spread byte up to 12: a few tenths of a degree)
        fx.slot = None;
        fx.jar_hit(th * 1000.0, d, 2, 27, (ay as f32).to_radians());
        let jar = -(fx.angles(t * 1000.0).pitch as f64).to_degrees();
        if jar < 0.9 || frac > 12.0 {
            continue;
        }
        n += 1;
        let off = dp - ap;
        worst = worst.max(off.abs() / jar);
        println!(
            "shot {:.3} s after a hit: the jar would be {jar:+.2} deg, the shot left {off:+.3} deg off its aim",
            t - th
        );
    }
    println!("rust6: {n} shots where the jar would show; largest offset {worst:.2} of the jar");
    assert!(n >= 4, "{n} shots");
    // without the jar the shot leaves within its cone: well below the jar the option would have added
    assert!(
        worst < 0.5,
        "the real client's shots left {worst:.2} of a jar off their aim"
    );
}
