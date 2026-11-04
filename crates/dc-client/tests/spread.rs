//! The spread, recoil and aim_mark model ([`dc_client::spread`]) against the real 2009 client.
//!
//! - ROM + capture tests (skipped without `CA_ROM` or the sweep folder): every burst of the experiment-A weapon sweep
//!   (`capture sweep/20261025/spread_recoil.csv` and the `analysis/shots.csv` of each capture it
//!   names) is replayed with its own shot times; the 0x82 spread byte of every shot and the view pitch drift are
//!   compared with what the 2009 client sent. The tick clock's phase against the 0x82 clock is not in the capture,
//!   so each burst is replayed at 64 phases and the best one is kept; the stance of a burst (stand, crouch, move) is
//!   not in `shots.csv` either and the best of the three is kept. The same bursts replayed with frames at 30, 60
//!   and 144 fps give the bytes of the 1 ms replay (frame-rate independence, project decision).
//! - Aim mark tests (skipped without `CA_ROM`): the HUD's aim_mark for the weapon the xhair captures held, at
//!   1680 x 1050, has the 14-pixel gap measured on the 2009 client's screenshots (xhair-frozen / xhair2-frozen
//!   `driver/rest_*.png`), on the CPU image and over a rendered frame (GPU-gated; the frames are written to the
//!   render output folder). PLAUSIBLE only: both driver logs record the 2009 window as `client [1280, 960]`, so the
//!   screenshots are a stretched 1280 x 960 frame, and their arms are anisotropic (11.7 px horizontal, 12.9 px
//!   vertical), which no uniform scale explains; the geometry rests on the disassembly (CLI-65).
//!
//! No ROM value is asserted or stored: the rows come from the user's ROM at run time, the expected bytes from the
//! captures.
// OBSERVED: capture sweep/20261025 (PROVEN-CAPTURE 0x82 `frac` and `aim` per shot),
// capture 20261025_124038_xhair2-frozen/driver/rest_1.png (green arms x 822..832 / 846..856 on
// row 524, column 839 rows 505..517 / 531..543), its driver_events.jsonl ("client [1280, 960]")

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dc_client::arms::WeaponTable;
use dc_client::hud::{Hud, HudState};
use dc_client::recoil::CameraRecoil;
use dc_client::spread::{
    clock, cone_units, frac_byte, AimMarkView, Motion, SpreadRow, SpreadState,
};
use dc_client::testkit::synth_font;

const SWEEP_CSV: &str = "sweep/20261025/spread_recoil.csv";

/// The weapon the player held on the xhair-frozen / xhair2-frozen screenshots (its row is read from the ROM).
// OBSERVED: the 0x82 weapon field of those captures (report client-spread.md); the same weapon as sweep-145
const XHAIR_WEAPON: u16 = 145;

fn sweep_csv() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DC_SWEEP_CSV").map(PathBuf::from) {
        return p.is_file().then_some(p);
    }
    let p = PathBuf::from(std::env::var_os("DC_CAPTURES")?).join(SWEEP_CSV);
    p.is_file().then_some(p)
}

/// Rows of a plain CSV file (no quoted fields) as name -> value maps.
fn read_csv(path: &Path) -> Vec<BTreeMap<String, String>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut lines = text.lines();
    let Some(head) = lines.next() else {
        return Vec::new();
    };
    let names: Vec<String> = head.split(',').map(|s| s.trim().to_string()).collect();
    lines
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            names
                .iter()
                .cloned()
                .zip(l.split(',').map(|s| s.trim().to_string()))
                .collect()
        })
        .collect()
}

/// One volley of a burst: client ms, spread byte, view pitch (degrees, positive looks down).
#[derive(Debug, Clone, Copy)]
struct Shot {
    ms: u32,
    frac: u8,
    pitch: f32,
}

/// The bursts of every swept weapon: weapon -> bursts (one value per volley, the first pellet).
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
        let shots = read_csv(&root.join(&cap).join("analysis").join("shots.csv"));
        let mut bursts: BTreeMap<u32, Vec<Shot>> = BTreeMap::new();
        for s in shots {
            let first = s.get("pellet").is_none_or(|p| p.is_empty() || p == "0");
            if s.get("weapon").and_then(|x| x.parse::<u16>().ok()) != Some(w) || !first {
                continue;
            }
            let (Some(b), Some(ms), Some(frac)) = (
                s.get("burst").and_then(|x| x.parse::<u32>().ok()),
                s.get("client_ms").and_then(|x| x.parse::<u32>().ok()),
                s.get("frac").and_then(|x| x.parse::<u8>().ok()),
            ) else {
                continue;
            };
            let pitch = s
                .get("shot_aim_pitch_deg")
                .and_then(|x| x.parse::<f32>().ok())
                .unwrap_or(f32::NAN);
            bursts.entry(b).or_default().push(Shot { ms, frac, pitch });
        }
        out.insert(w, bursts.into_values().filter(|b| !b.is_empty()).collect());
    }
    out
}

fn rom_table() -> Option<WeaponTable> {
    let rom = dc_rom::test_rom()?;
    WeaponTable::open(&rom).ok()
}

fn stance(kind: u8) -> Motion {
    use dc_sim::player::bits;
    match kind {
        0 => Motion::default(),
        1 => Motion {
            controls: bits::CROUCH,
            ..Motion::default()
        },
        _ => Motion {
            controls: bits::RUN | bits::ANY_MOVE | bits::LEFT,
            speed_frac: 1.0,
            ..Motion::default()
        },
    }
}

/// Replays one burst with frames every `period` ms at tick phase `phase` (ms added to the shot times): the bytes
/// our client would send and the view pitch (degrees) at each shot relative to the first.
fn replay_at(
    table: &WeaponTable,
    weapon: u16,
    burst: &[Shot],
    m: &Motion,
    phase: f64,
    period: f64,
) -> (Vec<u8>, Vec<f32>) {
    let row = table.spread.get(weapon).cloned().unwrap_or_default();
    let rates = table.spread.rates;
    let mut s = SpreadState::default();
    let mut cam = CameraRecoil::default();
    let t0 = burst[0].ms as f64;
    let base = 100_000.0 + phase;
    let mut t = t0 - 1500.0;
    let (mut bytes, mut pitch) = (Vec::new(), Vec::new());
    let mut view = 0.0f32;
    for sh in burst {
        let at = sh.ms as f64;
        while t < at {
            s.frame(&row, &rates, base + t, m, clock::TICK_MS);
            view += cam
                .drain(&row.recoil, base + t, clock::TICK_MS, false)
                .pitch;
            t += period;
        }
        pitch.push(view.to_degrees());
        let r = s.shot(&row, &rates, base + at, m, clock::TICK_MS);
        bytes.push(frac_byte(r.value));
        view += cam
            .kick(&row.recoil, base + at, clock::TICK_MS, false)
            .pitch;
    }
    (bytes, pitch)
}

/// [`replay_at`] with a frame every millisecond.
fn replay(
    table: &WeaponTable,
    weapon: u16,
    burst: &[Shot],
    m: &Motion,
    phase: f64,
) -> (Vec<u8>, Vec<f32>) {
    replay_at(table, weapon, burst, m, phase, 1.0)
}

/// Best (matching shots, phase, stance) of a burst.
fn best_fit(table: &WeaponTable, weapon: u16, burst: &[Shot]) -> (usize, f64, u8) {
    let mut best = (0usize, 0.0, 0u8);
    for k in 0..3u8 {
        for p in 0..64 {
            let phase = p as f64 * clock::TICK_MS / 64.0;
            let (b, _) = replay(table, weapon, burst, &stance(k), phase);
            let ok = b.iter().zip(burst).filter(|(a, s)| **a == s.frac).count();
            if ok > best.0 {
                best = (ok, phase, k);
            }
        }
    }
    best
}

/// Weapons whose cadence is shorter than their fall delay, or that never spread: no fall between shots, so the
/// bytes do not depend on the tick phase.
// OBSERVED: spread_recoil.csv (cadence 91 / 96 / 135 / 950 / 301 ms against the rows' DecDelay)
const PHASE_FREE: &[u16] = &[3, 6, 11, 17, 21, 22];

#[test]
fn rom_sweep_replay_reproduces_the_2009_spread_bytes() {
    let (Some(csv), Some(table)) = (sweep_csv(), rom_table()) else {
        println!("skipped: needs CA_ROM and the sweep folder");
        return;
    };
    let all = sweep_bursts(&csv);
    assert!(!all.is_empty(), "no bursts read from {}", csv.display());
    let (mut ok_all, mut n_all) = (0usize, 0usize);
    let mut report = Vec::new();
    for (w, bursts) in &all {
        let (mut ok, mut n) = (0, 0);
        for b in bursts {
            let (k, _, _) = best_fit(&table, *w, b);
            ok += k;
            n += b.len();
        }
        ok_all += ok;
        n_all += n;
        report.push(format!("{w}: {ok}/{n}"));
        if PHASE_FREE.contains(w) {
            assert_eq!(ok, n, "weapon {w}: every byte exact");
        } else if *w == 12 {
            // the 2009 client fell two steps in 28 % of its 11 ms windows (a frame longer than one tick); our
            // model runs one step per tick (report client-spread.md)
            assert!(ok * 2 >= n, "weapon 12: {ok}/{n}");
        } else {
            assert!(ok * 100 >= n * 85, "weapon {w}: {ok}/{n}");
        }
    }
    println!(
        "spread bytes exact: {ok_all}/{n_all}; {}",
        report.join(", ")
    );
    // 1851 / 1966 on the sweep of 2026-10-25 (1886 with the 2009 client's byte-before-the-steps order, which the
    // frame-rate rule replaced: the lost volleys are shots whose frame held a tick boundary)
    assert!(ok_all * 100 >= n_all * 93, "{ok_all}/{n_all}");
}

#[test]
fn rom_sweep_replay_gives_the_same_bytes_at_every_frame_rate() {
    // project decision: frame-rate independence by design, never a frame-rate cap
    let (Some(csv), Some(table)) = (sweep_csv(), rom_table()) else {
        println!("skipped: needs CA_ROM and the sweep folder");
        return;
    };
    let all = sweep_bursts(&csv);
    assert!(!all.is_empty(), "no bursts read from {}", csv.display());
    let mut n = 0usize;
    for (w, bursts) in &all {
        for (i, b) in bursts.iter().enumerate() {
            // a different phase and stance per burst, so the falls land on different ticks
            let phase = (i % 64) as f64 * clock::TICK_MS / 64.0;
            let m = stance((i % 3) as u8);
            let (reference, _) = replay(&table, *w, b, &m, phase);
            for fps in [30.0, 60.0, 144.0] {
                let (bytes, _) = replay_at(&table, *w, b, &m, phase, 1000.0 / fps);
                assert_eq!(bytes, reference, "weapon {w} burst {i} at {fps} fps");
            }
            n += b.len();
        }
    }
    println!("{n} volleys byte-identical at 30 / 60 / 144 / 1000 fps");
}

#[test]
fn rom_sweep_recoil_drift_matches_the_2009_view_pitch() {
    let (Some(csv), Some(table)) = (sweep_csv(), rom_table()) else {
        println!("skipped: needs CA_ROM and the sweep folder");
        return;
    };
    let all = sweep_bursts(&csv);
    let mut worst: Vec<(u16, f32)> = Vec::new();
    for (w, bursts) in &all {
        let mut wmax = 0.0f32;
        for b in bursts {
            if b.len() < 3 || b.iter().any(|s| !s.pitch.is_finite()) {
                continue;
            }
            let (_, phase, k) = best_fit(&table, *w, b);
            let (_, ours) = replay(&table, *w, b, &stance(k), phase);
            for (i, s) in b.iter().enumerate().take(30) {
                // capanalyze writes the pitch positive upward; ours is positive downward
                let theirs = -(s.pitch - b[0].pitch);
                wmax = wmax.max((ours[i] - theirs).abs());
            }
        }
        worst.push((*w, wmax));
    }
    println!(
        "recoil drift, largest error per weapon (degrees): {}",
        worst
            .iter()
            .map(|(w, e)| format!("{w}: {e:.2}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    for (w, e) in worst {
        // largest errors on the sweep of 2026-10-25 (the phase is the byte fit's, so it moves with that fit):
        // 0.27 degrees (weapon 25), 0.40 for weapon 12
        let tol = if w == 12 { 0.6 } else { 0.35 };
        assert!(e <= tol, "weapon {w}: recoil drift off by {e:.2} degrees");
    }
}

/// Green aim_mark pixels as the capture analysis counts them (PIL threshold on the screenshots).
fn green(p: [u8; 4]) -> bool {
    p[1] > 150 && p[1] as i32 > p[0] as i32 + 60 && p[1] as i32 > p[2] as i32 + 60
}

/// Inner edges (last lit before the centre, first lit after) along the centre row and column.
fn gaps(w: u32, h: u32, px: impl Fn(u32, u32) -> [u8; 4]) -> ((u32, u32), (u32, u32)) {
    let (cx, cy) = (w / 2, h / 2);
    let lit_row = |x: u32| (cy.saturating_sub(2)..=cy + 2).any(|y| green(px(x, y)));
    let lit_col = |y: u32| (cx.saturating_sub(2)..=cx + 2).any(|x| green(px(x, y)));
    let l = (cx - 80..cx).rev().find(|&x| lit_row(x)).unwrap_or(0);
    let r = (cx..cx + 80).find(|&x| lit_row(x)).unwrap_or(0);
    let t = (cy - 80..cy).rev().find(|&y| lit_col(y)).unwrap_or(0);
    let b = (cy..cy + 80).find(|&y| lit_col(y)).unwrap_or(0);
    ((l, r), (t, b))
}

fn aim_mark_state(distance: f32, since: Option<f32>) -> HudState {
    HudState {
        life_state: 1,
        aim_mark: Some(AimMarkView {
            distance,
            since_shot_s: since,
            colour: dc_client::spread::mark::COLOUR,
            rest: distance,
        }),
        ..HudState::default()
    }
}

/// The spread row of the xhair captures' weapon from the user's ROM (`None`: no ROM, or no such row).
fn xhair_row() -> Option<SpreadRow> {
    rom_table()?.spread.get(XHAIR_WEAPON).cloned()
}

#[test]
fn rom_idle_aim_mark_gap_is_the_measured_14_px_at_1680_by_1050() {
    let Some(row) = xhair_row() else {
        println!("skipped: CA_ROM unset (or no row for the xhair weapon)");
        return;
    };
    let still = Motion::default();
    let rest = cone_units(&row, &still, 0.0);
    let mut hud = Hud::fallback(1680, 1050, Some((synth_font(), 0)));
    let img = hud.render(&aim_mark_state(rest, None));
    let ((l, r), (t, b)) = gaps(img.width, img.height, |x, y| img.pixel(x, y));
    // xhair-frozen / xhair2-frozen rest_*.png: 832 / 846 and 517 / 531 (gap 14 on both axes). PLAUSIBLE only: the
    // 2009 client rendered a 1280 x 960 frame (driver log) that the wrapper stretched to 1680 x 1050, and its arms
    // are anisotropic (11.7 px horizontal, 12.9 px vertical); this pins our geometry (CLI-65) to the images.
    assert_eq!((l, r, r - l), (832, 846, 14));
    assert_eq!((t, b, b - t), (517, 531, 14));
    // the arms' outer ends: 822 / 856 as on the screenshots (the vertical arms of the screenshots end 2 px further
    // out, 505 / 543; report client-spread.md)
    let lit: Vec<u32> = (800..880).filter(|&x| green(img.pixel(x, 524))).collect();
    assert_eq!((lit[0], *lit.last().unwrap()), (822, 856));
    // a shot pushes the arms out by 4 x 1680 / 1024 at once and they come back within 0.1 s
    let fired = hud.render(&aim_mark_state(rest, Some(0.0)));
    let ((l2, r2), _) = gaps(fired.width, fired.height, |x, y| fired.pixel(x, y));
    assert!(r2 - l2 >= 14 + 12, "{l2} {r2}");
    // the full spread opens the gap well beyond the rest gap (or out of the 80-pixel search window)
    let open = hud.render(&aim_mark_state(cone_units(&row, &still, 1.0), None));
    let ((l3, r3), _) = gaps(open.width, open.height, |x, y| open.pixel(x, y));
    assert!(r3 == 0 || r3 - l3 > 2 * 14, "{l3} {r3}");
}

#[test]
fn rom_aim_mark_over_a_rendered_frame_keeps_the_gap() {
    let Some(row) = xhair_row() else {
        println!("skipped: CA_ROM unset (or no row for the xhair weapon)");
        return;
    };
    let still = Motion::default();
    use dc_client::offscreen::Offscreen;
    use dc_client::overlay::OverlayRenderer;
    use dc_client::testkit::synth_source;
    use dc_client::world::GpuTarget;
    use dc_client::{LocalPlayer, ViewAngles, ViewSettings, WorldSource};
    use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
    let (w, h) = (1680u32, 1050u32);
    let ctx = match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => c,
        Err(e) => {
            println!("skipped: no GPU adapter ({e})");
            return;
        }
    };
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut src = synth_source();
    let world = src
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))
        .expect("synthetic world");
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, w, h);
    let mut layer = OverlayRenderer::new(&ctx, OFFSCREEN_FORMAT);
    let spawn = world.spawn(0).unwrap();
    let me = LocalPlayer::spawn(
        dc_sim::MovementParams::default(),
        &world.collision,
        &spawn,
        world.stand_dims,
        60,
    );
    let view = ViewAngles {
        yaw: spawn.yaw,
        pitch: 0.0,
    };
    let camera = ViewSettings::default().camera(me.eye_target(&view), &view, w, h);
    let mut hud = Hud::fallback(w, h, Some((synth_font(), 0)));
    let out = dc_render::default_out_dir().join("dc-client");
    let _ = std::fs::create_dir_all(&out);
    let rest = cone_units(&row, &still, 0.0);
    for (name, d, since) in [
        ("aim_mark_idle", rest, None),
        ("aim_mark_shot", rest, Some(0.02)),
        ("aim_mark_burst", cone_units(&row, &still, 0.3), None),
    ] {
        layer.update(&ctx, &hud.render(&aim_mark_state(d, since)));
        let img = target
            .render_with_hud(
                &ctx,
                &renderer,
                world.scene.as_ref().unwrap(),
                &camera,
                None,
                &[],
                &[],
                Some(&layer),
                None,
            )
            .unwrap();
        let _ = img.save_png(&out.join(format!("{name}.png")));
        let ((l, r), (t, b)) = gaps(img.width, img.height, |x, y| img.pixel(x, y));
        println!("{name}: horizontal {l}..{r}, vertical {t}..{b}");
        if name == "aim_mark_idle" {
            assert_eq!((r - l, b - t), (14, 14));
        } else {
            assert!(r - l > 14 && b - t > 14);
        }
    }
}
