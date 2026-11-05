//! CLI-MOVE-1 evidence: the client's mover gives the same trajectory at 30, 60, 144 and 1000 rendered frames per
//! second for a scripted input (walk, crouch-walk, run, a running jump, sprint, a standing jump), offline on the
//! made-up world and against our own dedicated server over loopback UDP (the body placed by the server's teleport,
//! the sprint permission from its 0xca echo, the step height from its `S2C_EVAR` when the ROM names the variable).
//! The per-frame 2009 rule (displacements of 0.1 units or less are dropped) is shown once at a 1 ms variable step
//! for contrast: crouch-walking freezes and the jump apex drops, which the fixed step does not reproduce.
// SPEC: MOVEMENT_2009.md §4 (time step), §8.1 step 5 (0.1-unit drop), §7 (stance speeds), §6.7 (jump), §12.2 (0x64);
// MOVEMENT_2009_addendum_servervars.md B (per-frame rules, fixed-step recipe); GAME_MESSAGES_2009_C2S.md §2.1 (rate)
// DC-DEFAULT: CLI-MOVE-1 (proposed) fixed 1/60 s step with render interpolation, no frame-rate cap

mod support;

use std::time::Duration;

use dc_client::testkit::{synth_source, SYNTH_DIMS};
use dc_client::{
    run_headless, AnchoredScript, Connect, HeadlessOptions, HeadlessReport, InputSource,
    MoveScript, NetOptions, NetWorld, TracePoint, ViewAngles, WorldFile, WorldSource,
};
use dc_sim::{drop_to_floor, Commands, Cylinder, MovementParams, PlayerState, TickInput, V3};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use support::*;

/// The four display rates and their frame intervals (microseconds).
const RATES: [(u32, u64); 4] = [(30, 33_333), (60, 16_667), (144, 6_944), (1000, 1_000)];

/// Simulation rate (CLI-MOVE-1).
const HZ: u32 = 60;

/// Facing -Z: the made-up world's floor runs 2000 units that way without a wall.
fn view() -> ViewAngles {
    ViewAngles {
        yaw: std::f32::consts::PI,
        pitch: 0.0,
    }
}

/// Positions of the ticks from `anchor` on, relative to the position at the anchor (index = ticks after it).
fn rel_path(trace: &[TracePoint], anchor: u64, origin: V3) -> Vec<V3> {
    trace
        .iter()
        .filter(|p| p.tick >= anchor)
        .map(|p| p.pos - origin)
        .collect()
}

/// Largest distance between two paths at every whole simulated second (and at the last common tick).
fn per_second_diff(a: &[V3], b: &[V3]) -> f32 {
    let n = a.len().min(b.len());
    let mut idx: Vec<usize> = (0..n).step_by(HZ as usize).collect();
    if n > 0 {
        idx.push(n - 1);
    }
    idx.into_iter()
        .map(|i| (a[i] - b[i]).len())
        .fold(0.0f32, f32::max)
}

/// Speed over the ticks `[t0, t1)` seconds of a path (horizontal displacement / time).
fn speed(path: &[V3], t0: f64, t1: f64) -> f32 {
    let (i, j) = ((t0 * HZ as f64) as usize, (t1 * HZ as f64) as usize);
    (path[j] - path[i]).len_xz() / (t1 - t0) as f32
}

/// The jump apex above the take-off height for a jump starting at `t0` seconds (within 1 s).
fn apex(path: &[V3], t0: f64) -> f32 {
    let i = (t0 * HZ as f64) as usize;
    let y0 = path[i.saturating_sub(1)].y;
    path[i..(i + HZ as usize).min(path.len())]
        .iter()
        .map(|p| p.y - y0)
        .fold(f32::MIN, f32::max)
}

#[derive(Debug, Clone, Copy)]
struct Measured {
    walk: f32,
    crouch_walk: f32,
    run: f32,
    sprint: f32,
    run_jump_apex: f32,
    stand_jump_apex: f32,
}

/// The phase speeds and apexes of a path that starts at the script's t = 0 (MoveScript table).
fn measure(path: &[V3]) -> Measured {
    Measured {
        walk: speed(path, 1.5, 2.5),
        crouch_walk: speed(path, 4.0, 5.0),
        run: speed(path, 6.0, 6.5),
        sprint: speed(path, 8.5, 9.5),
        run_jump_apex: apex(path, 6.5),
        stand_jump_apex: apex(path, 10.5),
    }
}

fn offline_run(frame_us: u64) -> HeadlessReport {
    let mut src = synth_source();
    let secs = MoveScript::SECONDS + 0.5;
    let opts = HeadlessOptions {
        frames: (secs * 1e6 / frame_us as f64).ceil() as u32,
        frame_us,
        render: false,
        tick_hz: HZ,
        trace: true,
        ..HeadlessOptions::default()
    };
    let mut script = MoveScript::new(view());
    run_headless(&mut src, Some(&mut script as &mut dyn InputSource), &opts).expect("headless")
}

#[test]
fn offline_trajectory_is_the_same_at_30_60_144_and_1000_fps() {
    let runs: Vec<(u32, HeadlessReport)> = RATES
        .iter()
        .map(|&(fps, us)| (fps, offline_run(us)))
        .collect();
    let paths: Vec<(u32, Vec<V3>)> = runs
        .iter()
        .map(|(fps, r)| (*fps, rel_path(&r.trace, 0, r.trace[0].pos)))
        .collect();
    let (_, base) = &paths[1]; // 60 fps
    for (fps, p) in &paths {
        assert!(
            p.len() as f64 >= MoveScript::SECONDS * HZ as f64,
            "{fps} fps: {} ticks",
            p.len()
        );
        let d = per_second_diff(base, p);
        println!(
            "offline {fps:>4} fps: {} ticks, largest per-second difference to 60 fps {d:.6}",
            p.len()
        );
        assert!(d <= 0.01, "{fps} fps differs from 60 fps by {d}");
        // bit-identical in fact: the same ticks with the same inputs
        let n = base.len().min(p.len());
        assert!(base[..n] == p[..n], "{fps} fps: not bit-identical");
    }
    let m = measure(base);
    println!("offline phases (60 Hz mover): {m:?}");
    // MOVEMENT_2009 §6.3 caps the speed before the step's acceleration; the 2009 client at its own 1-2 ms frames
    // therefore REPORTED and (running / sprinting) moved at the caps x (1 - p). DeCombat's fixed step keeps the
    // grounded displacement at the cap (SIM-22): the body moves at walk 70, crouch 50, run 285 units/s at any step.
    assert!((m.walk - 70.0).abs() < 0.5, "walk {}", m.walk);
    assert!(
        (m.crouch_walk - 50.0).abs() < 0.5,
        "crouch-walk {}",
        m.crouch_walk
    );
    assert!((m.run - 285.0).abs() < 0.5, "run {}", m.run);
    assert!(
        m.sprint > 1.3 * m.run,
        "sprint {} vs run {}",
        m.sprint,
        m.run
    );
    // the 60 Hz apex of the spec (V-M3, dt^2/4 term): 50.875
    assert!(
        (m.stand_jump_apex - 50.875).abs() < 0.05,
        "standing apex {}",
        m.stand_jump_apex
    );
    assert!(m.run_jump_apex > 45.0, "running apex {}", m.run_jump_apex);
}

/// The 2009 client's own step: one mover step per frame with `dt` in whole milliseconds. At 1000 frames per
/// second (dt 1 ms) every displacement of 0.1 units or less is dropped (MOVEMENT_2009 §8.1 step 5): the crouched
/// body (cap 50 units/s, 0.05 units per frame) does not move, as the real client's crouch-walk did not in tdm1 and
/// rust4 (0 of 39 / 62 crouched forward updates moved, see the report). CLI-MOVE-1 applies the rule to the 1/60 s
/// step only, where the dead zone is 6 units/s. (The jump at 1 ms steps is not compared: dc-sim's ground probe and
/// slide-to-stop are not validated at steps that short; the captured 2009 apexes are in the report.)
#[test]
fn the_2009_per_frame_drop_at_1_ms_frames_freezes_crouch_walking_the_fixed_step_does_not() {
    let mut src = synth_source();
    let world = src.load(None).expect("load");
    let p = MovementParams::default();
    let at = drop_to_floor(
        &world.collision,
        &Cylinder::from_dims(SYNTH_DIMS),
        V3::new(0.0, 60.0, 0.0),
        p.step(),
        1000.0,
    );
    let run = |dt: f32, secs: f32, commands: Commands| -> f32 {
        let mut s = PlayerState::new(at, SYNTH_DIMS, &p);
        let settle = TickInput::default();
        for _ in 0..((0.5 / dt) as usize) {
            s.step(&p, &world.collision, &settle, dt);
        }
        let start = s.pos;
        let input = TickInput {
            commands,
            yaw: std::f32::consts::PI,
            ..TickInput::default()
        };
        for _ in 0..((secs / dt) as usize) {
            s.step(&p, &world.collision, &input, dt);
        }
        (s.pos - start).len_xz() / secs
    };
    let crouch = Commands {
        forward: true,
        crouch: true,
        ..Commands::default()
    };
    let walk = Commands {
        forward: true,
        ..Commands::default()
    };
    let (crouch_1ms, crouch_60) = (run(0.001, 2.0, crouch), run(1.0 / 60.0, 2.0, crouch));
    let (walk_1ms, walk_60) = (run(0.001, 2.0, walk), run(1.0 / 60.0, 2.0, walk));
    println!(
        "2009 per-frame step at 1 ms: crouch-walk {crouch_1ms:.2}, walk {walk_1ms:.2} units/s; fixed 1/60 s step: crouch-walk {crouch_60:.2}, walk {walk_60:.2} units/s"
    );
    assert!(
        crouch_1ms < 0.5,
        "at 1 ms frames the crouched body is frozen: {crouch_1ms}"
    );
    assert!(
        walk_1ms < 0.5,
        "at 1 ms frames the walking body is frozen: {walk_1ms}"
    );
    assert!(crouch_60 > 40.0, "the fixed step crouch-walks: {crouch_60}");
    assert!(walk_60 > 60.0, "the fixed step walks: {walk_60}");
}

fn world_file() -> WorldFile {
    let s = synth_source();
    WorldFile {
        bytes: s.bytes,
        stand_dims: SYNTH_DIMS,
        textures: s.textures,
    }
}

/// The client's step-height variable name from the user's own `Game/CShell.dll` (found by its hash, as the
/// dedicated server does), when CA_ROM names a ROM. The name is never stored in the tree.
fn step_height_name() -> Option<String> {
    let rom = std::env::var("CA_ROM")
        .ok()
        .filter(|r| !r.trim().is_empty())?;
    let bytes = std::fs::read(std::path::Path::new(&rom).join("Game").join("CShell.dll")).ok()?;
    let (key, len) = dc_client::net::STEP_HEIGHT_KEY;
    bytes
        .iter()
        .enumerate()
        .filter(|&(i, &b)| b == 0 && i >= len)
        .map(|(i, _)| &bytes[i - len..i])
        .find(|w| w.iter().all(u8::is_ascii_graphic) && dc_world::hash16(w) == key)
        .map(|w| String::from_utf8_lossy(w).into_owned())
}

/// The server's step height in the networked runs (not 30, so that following it is visible).
const SERVER_STEP: f32 = 24.0;

struct NetRun {
    fps: u32,
    report: HeadlessReport,
    anchor: u64,
    /// First tick after the anchor whose frame had the server's sprint permission.
    sprint_allowed_at: Option<u64>,
    updates: u64,
    in_world_s: f64,
    teleports: usize,
    /// Gaps between the client's updates on its session clock (ms -> count).
    gaps: std::collections::BTreeMap<u32, u32>,
    /// The server's update rejections (each answered with a teleport back).
    rejected: Vec<String>,
}

impl NetRun {
    /// Frame stalls of the client after the script's anchor (the frame loop held, then caught up).
    fn stalls_in_script(&self) -> Vec<dc_client::FrameStall> {
        let anchor_frame = self
            .report
            .clock_marks
            .iter()
            .find(|m| m.ticks >= self.anchor)
            .map_or(0, |m| m.frame);
        self.report
            .stalls
            .iter()
            .filter(|s| s.frame >= anchor_frame)
            .copied()
            .collect()
    }

    /// A correction during the script that a busy machine explains: every rejection was "too fast" (the body's
    /// catch-up after a held frame loop arrives at the server in one update) and the client logged a frame stall
    /// during the script. Any other correction is a failure of the run.
    fn load_explained(&self) -> bool {
        self.teleports > 2
            && !self.rejected.is_empty()
            && self.rejected.iter().all(|l| l.contains("too fast"))
            && !self.stalls_in_script().is_empty()
    }
}

/// Re-runs of one rate whose correction a frame stall explains ([`NetRun::load_explained`]).
const LOAD_RETRIES: usize = 2;

/// The client's update period on the session clock in whole ms (the gate's ceil(1000 / R)). The 2009 default is
/// R = 15 (GAME_MESSAGES_2009_C2S.md §2.1, 67 ms); DeCombat sends 30 (DC_DEFAULTS CLI-1578, 2026-10-31: every
/// 30 Hz server tick carries a new position), 34 ms.
const UPDATE_PERIOD_MS: u32 = (1000.0 / dc_client::update::SEND_RATE_HZ).ceil() as u32;

fn median_gap(gaps: &std::collections::BTreeMap<u32, u32>) -> Option<u32> {
    let n: u32 = gaps.values().sum();
    let mut seen = 0;
    for (g, c) in gaps {
        seen += c;
        if 2 * seen > n {
            return Some(*g);
        }
    }
    None
}

fn net_run(fps: u32, frame_us: u64, evar: Option<&str>) -> NetRun {
    let dir = scratch(&format!("mover-{fps}"));
    let world = synth_world_file(&dir);
    let ev = evar.map(|name| format!("{name}={SERVER_STEP:.6}"));
    let mut args = vec![
        "-tick",
        "20",
        "-roundsecs",
        "120",
        "-holds",
        "1500,800",
        "-seed",
        "7",
    ];
    if let Some(e) = ev.as_deref() {
        args.extend(["-evar", e]);
    }
    let (mut srv, port) = start_dserver(&world, &args);
    srv.line("user 1 1 1 0 0 MoverClient");
    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "MoverClient",
    );
    o.world_file = Some(world_file());
    o.log = Some(dir.join("client_events.jsonl"));
    o.capture = Some(dir.join("capture.jsonl"));
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    // the session's update counter sampled every 100 ms on the wall clock (the 0x64 cadence)
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = {
        let (h, stop) = (handle.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut out: Vec<(Instant, u64)> = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                out.push((Instant::now(), h.stats().updates_sent));
                std::thread::sleep(Duration::from_millis(100));
            }
            out
        })
    };
    // join + spawn (a few seconds), then the script
    let secs = 6.0 + MoveScript::SECONDS + 1.0;
    let opts = HeadlessOptions {
        frames: (secs * 1e6 / frame_us as f64).ceil() as u32,
        frame_us,
        render: false,
        tick_hz: HZ,
        realtime: true,
        trace: true,
        ..HeadlessOptions::default()
    };
    // our server teleports twice at the spawn (seq 1, then seq 2 after the client's 0x87, as the reference stack did)
    let mut script = AnchoredScript::new(MoveScript::new(view()), 2);
    let report = run_headless(&mut net, Some(&mut script as &mut dyn InputSource), &opts)
        .expect("headless run");
    stop.store(true, Ordering::Relaxed);
    let samples = sampler.join().expect("sampler");
    let st = handle.shutdown(Duration::from_secs(5));
    srv.drain();
    let _ = srv.finish(Duration::from_secs(3));
    srv.drain();
    let rejected: Vec<String> = srv
        .seen
        .iter()
        .filter(|l| l.contains("update rejected"))
        .cloned()
        .collect();
    // from the first sample with an update sent to the last one
    let live: Vec<&(Instant, u64)> = samples.iter().filter(|s| s.1 > 0).collect();
    let (updates, in_world_s) = match (live.first(), live.last()) {
        (Some(a), Some(b)) => (b.1 - a.1, (b.0 - a.0).as_secs_f64()),
        _ => (0, 0.0),
    };
    let anchor = script.anchor.unwrap_or_else(|| {
        panic!(
            "{fps} fps: never anchored: life {:?} teleports {:?} milestones {:?}",
            st.life_states,
            st.teleports,
            st.milestones.iter().map(|m| &m.0).collect::<Vec<_>>()
        )
    });
    let sprint_allowed_at = report
        .trace
        .iter()
        .find(|p| p.tick >= anchor && p.sprint_allowed)
        .map(|p| p.tick - anchor);
    println!(
        "{fps} fps: anchor tick {anchor}, teleports {:?}, updates {}, sprint permission {:?} ticks after the anchor, \
         step height {}, capture {}",
        st.teleports,
        st.updates_sent,
        sprint_allowed_at,
        report.step_height,
        dir.join("capture.jsonl").display()
    );
    NetRun {
        fps,
        anchor,
        sprint_allowed_at,
        updates,
        in_world_s,
        teleports: st.teleports.len(),
        gaps: st.update_gaps.clone(),
        rejected,
        report,
    }
}

#[test]
fn trajectory_against_our_dserver_is_the_same_at_30_60_144_and_1000_fps() {
    let name = step_height_name();
    // the four rates in parallel, one server each (loopback, ephemeral ports)
    let runs: Vec<NetRun> = std::thread::scope(|sc| {
        let hs: Vec<_> = RATES
            .iter()
            .map(|&(fps, us)| {
                let n = name.clone();
                sc.spawn(move || net_run(fps, us, n.as_deref()))
            })
            .collect();
        hs.into_iter().map(|h| h.join().expect("run")).collect()
    });
    // a rate whose run a busy machine disturbed (a held frame loop, then a catch-up the server's speed check
    // rejects) runs again alone, at most LOAD_RETRIES times; an unexplained correction fails at once below
    let runs: Vec<NetRun> = runs
        .into_iter()
        .map(|mut r| {
            for retry in 1..=LOAD_RETRIES {
                if !r.load_explained() {
                    break;
                }
                println!(
                    "UNDER LOAD: {} fps corrected during the script ({:?}) after frame stalls {:?}; retry {retry}",
                    r.fps,
                    r.rejected,
                    r.stalls_in_script()
                );
                let us = RATES.iter().find(|x| x.0 == r.fps).map_or(16_667, |x| x.1);
                r = net_run(r.fps, us, name.as_deref());
            }
            r
        })
        .collect();
    let mut paths = Vec::new();
    let mut rates = Vec::new();
    for r in &runs {
        assert_eq!(r.report.nan_ticks, 0, "{} fps", r.fps);
        assert_eq!(
            r.teleports,
            2,
            "{} fps: the two spawn teleports, no correction during the script; server {:?}; {}",
            r.fps,
            r.rejected,
            r.report.stall_note()
        );
        let origin = r
            .report
            .trace
            .iter()
            .find(|p| p.tick == r.anchor)
            .map(|p| p.pos)
            .expect("anchor tick traced");
        let path = rel_path(&r.report.trace, r.anchor, origin);
        assert!(
            path.len() as f64 >= MoveScript::SECONDS * HZ as f64,
            "{} fps: {} ticks after the anchor",
            r.fps,
            path.len()
        );
        if name.is_some() {
            assert_eq!(
                r.report.step_height, SERVER_STEP,
                "{} fps: the server's step height",
                r.fps
            );
        } else {
            println!(
                "CA_ROM unset: the step-height variable is not sent; the mover keeps 30 (CLI-57)"
            );
            assert_eq!(r.report.step_height, 30.0);
        }
        // 0x64 cadence: one update per period (CLI-1578: 30 per second; the 2009 client's 67 ms at 15), moving or
        // standing
        let rate = r.updates as f64 / r.in_world_s;
        println!(
            "{} fps: {} updates in {:.1} s in the world = {rate:.2} per second",
            r.fps, r.updates, r.in_world_s
        );
        let gaps = &r.gaps;
        let short: u32 = gaps.range(..UPDATE_PERIOD_MS).map(|(_, c)| c).sum();
        println!(
            "{} fps: update gaps on the session clock {gaps:?} (median {:?}, {short} below {UPDATE_PERIOD_MS} ms); {}",
            r.fps,
            median_gap(gaps),
            r.report.stall_note()
        );
        paths.push((r.fps, path, r.sprint_allowed_at));
        rates.push((r.fps, rate, median_gap(gaps), short));
    }
    // the 0x64 gate on the session's own clock: the usual gap is the period (the median within 3 ms: the network
    // thread's poll), never shorter except at a life-state / spawn reset of the gate; on the wall clock never faster
    // than the send rate (CLI-1578). (A busy machine only stretches gaps: the wall-clock rate alone is no evidence.)
    let bound = dc_client::update::SEND_RATE_HZ as f64 * (15.4 / 15.0);
    let starved = dc_client::update::SEND_RATE_HZ as f64 * 0.9;
    for (fps, rate, median, short) in &rates {
        assert!(*rate < bound, "{fps} fps: update rate {rate}");
        // a machine that cannot hold the send rate (a shared CI runner: 26 updates per second at any fps) only
        // stretches the gaps, so the median says nothing there; the never-shorter check below still holds
        if *rate < starved {
            println!("{fps} fps: wall rate {rate:.2} below {starved:.1}: the median gap check is skipped (starved runner)");
        } else {
            assert!(
                median.is_some_and(|m| (UPDATE_PERIOD_MS..=UPDATE_PERIOD_MS + 3).contains(&m)),
                "{fps} fps: median update gap {median:?} (wall rate {rate:.2})"
            );
        }
        assert!(
            *short <= 3,
            "{fps} fps: {short} update gaps below the period"
        );
    }
    // up to the sprint phase (t = 7.5 s) the paths are equal at every simulated second, and in fact bit for bit
    let sprint_tick = (7.5 * HZ as f64) as usize;
    let (_, base, _) = &paths[1];
    for (fps, p, _) in &paths {
        let d = per_second_diff(&base[..sprint_tick], &p[..sprint_tick]);
        println!("dserver {fps:>4} fps: largest per-second difference to 60 fps before the sprint {d:.6}");
        assert!(d <= 0.01, "{fps} fps differs by {d} before the sprint");
        assert!(
            base[..sprint_tick] == p[..sprint_tick],
            "{fps} fps: not bit-identical before the sprint"
        );
    }
    // the sprint speed depends on when the server's permission arrived (network round trip, not the frame rate):
    // the body runs until the 0xca [1] echo is in, then sprints. Past that point the phase is the same law: the
    // sprint plateau speed agrees across the rates
    for (fps, p, allowed) in &paths {
        let m = measure(p);
        println!(
            "dserver {fps:>4} fps phases: {m:?}, sprint permission at script tick {allowed:?}"
        );
        assert!(
            allowed.is_some(),
            "{fps} fps: the server allowed the sprint"
        );
        let base_m = measure(base);
        assert!(
            (m.sprint - base_m.sprint).abs() < 0.01,
            "{fps} fps sprint plateau {} vs {}",
            m.sprint,
            base_m.sprint
        );
        // (the ROM speed rows scale both by the body row's penalty, CLI-340: compare with the path's own run)
        assert!(
            m.sprint > 1.3 * m.run,
            "{fps} fps: the sprint is faster than the run: {} vs {}",
            m.sprint,
            m.run
        );
    }
    // runs whose permission came on the same script tick are the same over the whole script, sprint included
    for (fps, p, allowed) in &paths {
        for (fps2, p2, allowed2) in &paths {
            if fps < fps2 && allowed == allowed2 {
                let d = per_second_diff(p, p2);
                println!("dserver {fps} vs {fps2} fps (same permission tick): largest per-second difference {d:.6}");
                assert!(d <= 0.01, "{fps} vs {fps2} fps differ by {d}");
            }
        }
    }
}
