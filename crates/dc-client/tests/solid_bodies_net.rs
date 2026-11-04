//! Solid character bodies over loopback UDP: our `dc-dserver` with the made-up world of `dc_client::testkit`, a
//! dc-botclient that stands still as the second player, and the client headless (frames paced to the wall clock,
//! rendered offscreen). Both players spawn on the same point (the made-up world has no spawn points), so the
//! scripted input first backs away from the stander (moving away from an overlapping body is free), then runs
//! straight at it (the own body must stop at the stander's box, grown by the own radius), then runs past it at an
//! angle (the own body must slide along the box and get past it). Checked on the positions the frame loop draws
//! (the eye point and the remote as the client sees it every frame).
// SPEC: MOVEMENT_2009.md §9.6 stage 2 (objects by their box, engine+0x75860); DSERVER_PIPE.md §2-§4
// DC-DEFAULT: SIM-13 (proposed) the own mover collides with the visible remote characters' boxes

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::run_game;
use dc_botclient::session::{BotSession, GameConfig};
use dc_client::input::ServerHold;
use dc_client::testkit::{synth_font, synth_source, SYNTH_DIMS};
use dc_client::world::RemoteBody;
use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetOptions, NetWorld, ViewAngles,
    WorldFile,
};
use support::*;

fn world_file() -> WorldFile {
    let s = synth_source();
    WorldFile {
        bytes: s.bytes,
        stand_dims: SYNTH_DIMS,
        textures: s.textures,
    }
}

fn stander(port: u16, world: std::path::PathBuf, secs: u64) -> std::thread::JoinHandle<BotSession> {
    std::thread::spawn(move || {
        let mut cfg = GameConfig::new("Stander");
        cfg.hello_tag = "stander".into();
        cfg.motion = MotionKind::Stand;
        cfg.world_file = Some(world);
        cfg.load_ms = 300;
        let mut bot = BotSession::new(cfg, None, Recorder::null("Stander"));
        let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        run_game(
            &mut bot,
            server,
            Duration::from_secs(secs),
            &Recorder::null("Stander"),
        )
        .expect("bot udp");
        bot
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Wait,
    Back,
    Charge,
    Slide,
    Done,
}

/// One drawn frame: the phase, the own eye point (the body centre in x / z: no forward eye offset) and the remote.
#[derive(Debug, Clone, Copy)]
struct Sample {
    phase: Phase,
    own: [f32; 3],
    remote: [f32; 3],
    half: [f32; 3],
    /// Server corrections applied before this frame.
    corrections: u32,
}

/// Frames of the charge (2 s) and of the slide (2 s).
const CHARGE_FRAMES: u32 = 120;
const SLIDE_FRAMES: u32 = 120;
/// How far to back away before the charge.
const BACK_DIST: f32 = 150.0;
/// The slide's turn from straight at the stander.
const SLIDE_TURN_DEG: f32 = 30.0;

struct Charger {
    alive: bool,
    corrections: u32,
    /// The correction count the current run started with: a server teleport restarts the run.
    run_corrections: u32,
    restarts: u32,
    phase: Phase,
    frames: u32,
    cmd: dc_sim::Commands,
    log: Vec<Sample>,
}

impl Charger {
    fn new() -> Charger {
        Charger {
            alive: false,
            corrections: 0,
            run_corrections: 0,
            restarts: 0,
            phase: Phase::Wait,
            frames: 0,
            cmd: dc_sim::Commands::default(),
            log: Vec::new(),
        }
    }
}

impl InputSource for Charger {
    fn frame(&mut self, _view: &mut ViewAngles) {}

    fn tick(&mut self, _tick: u64, _hz: u32, _view: &mut ViewAngles) -> dc_sim::Commands {
        self.cmd
    }

    fn server(&mut self, hold: &ServerHold) {
        self.alive = hold.life_state == Some(1);
        self.corrections = hold.corrections;
    }

    fn aim(
        &mut self,
        eye: [f32; 3],
        remotes: &[RemoteBody],
        _own_team: Option<u8>,
        view: &mut ViewAngles,
    ) {
        self.cmd = dc_sim::Commands::default();
        let Some(r) = remotes.iter().find(|r| r.name == "Stander") else {
            return;
        };
        let visible = r.look.as_ref().is_none_or(|l| l.visible);
        if !self.alive || !visible {
            return;
        }
        if self.corrections != self.run_corrections && self.phase != Phase::Done {
            // a server teleport (the spawn, or a correction) restarts the run from where it put the body
            self.run_corrections = self.corrections;
            if self.phase != Phase::Wait {
                self.restarts += 1;
                println!(
                    "restart: correction {} in {:?} frame {} after {} samples, last {:?}",
                    self.corrections,
                    self.phase,
                    self.frames,
                    self.log.len(),
                    self.log.last()
                );
            }
            self.phase = Phase::Back;
            self.frames = 0;
            self.log.clear();
        }
        self.log.push(Sample {
            phase: self.phase,
            own: eye,
            remote: r.pos,
            half: r.half,
            corrections: self.corrections,
        });
        let (dx, dz) = (r.pos[0] - eye[0], r.pos[2] - eye[2]);
        let towards = dx.atan2(dz);
        let dist = dx.hypot(dz);
        let run = |forward: bool| dc_sim::Commands {
            forward,
            backward: !forward,
            run: true,
            ..dc_sim::Commands::default()
        };
        match self.phase {
            Phase::Wait => {
                self.phase = Phase::Back;
            }
            Phase::Back => {
                view.yaw = towards;
                self.cmd = run(false);
                if dist > BACK_DIST {
                    self.phase = Phase::Charge;
                    self.frames = 0;
                }
            }
            Phase::Charge => {
                view.yaw = towards;
                self.cmd = run(true);
                self.frames += 1;
                if self.frames >= CHARGE_FRAMES {
                    self.phase = Phase::Slide;
                    self.frames = 0;
                }
            }
            Phase::Slide => {
                if self.frames == 0 {
                    // one fixed heading for the whole slide
                    view.yaw = towards + SLIDE_TURN_DEG.to_radians();
                }
                self.cmd = run(true);
                self.frames += 1;
                if self.frames >= SLIDE_FRAMES {
                    self.phase = Phase::Done;
                }
            }
            Phase::Done => {}
        }
    }

    fn label(&self) -> &'static str {
        "charger"
    }
}

/// How far the own centre is outside the stander's box grown by the own radius (negative: inside), in x / z with
/// the rounded corners of the 2009 box test. The client gives every remote the box of its own last teleport
/// (CLI-16), so the remote's half extents are the own box too and its radius is the own radius.
fn outside(s: &Sample) -> f32 {
    let r = s.half[0].max(s.half[2]);
    let ax = (s.own[0] - s.remote[0]).abs() - s.half[0];
    let az = (s.own[2] - s.remote[2]).abs() - s.half[2];
    let d = if ax > 0.0 && az > 0.0 {
        ax.hypot(az)
    } else {
        ax.max(az)
    };
    d - r
}

#[test]
fn the_own_body_stops_at_a_standing_player_and_slides_past_it() {
    let dir = scratch("bodies");
    let world = synth_world_file(&dir);
    let (mut srv, port) = start_dserver(
        &world,
        &[
            "-tick",
            "20",
            "-roundsecs",
            "40",
            "-holds",
            "1500,800",
            "-timeout-hold",
            "1500",
            "-seed",
            "7",
        ],
    );
    srv.line("user 1 1 1 0 0 NetClient");
    srv.line("user 2 2 2 1 0 Stander");
    let bot = stander(port, world.clone(), 40);

    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "NetClient",
    );
    o.world_file = Some(world_file());
    o.log = Some(dir.join("client_events.jsonl"));
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let opts = HeadlessOptions {
        frames: 60 * 24,
        frame_us: 16_667,
        width: 160,
        height: 120,
        out: None,
        save_every: 6,
        font_bytes: Some(synth_font()),
        realtime: true,
        ..HeadlessOptions::default()
    };
    let mut input = Charger::new();
    let r = run_headless(&mut net, Some(&mut input as &mut dyn InputSource), &opts)
        .expect("headless run");
    let st = handle.shutdown(Duration::from_secs(5));
    let _ = bot.join();
    srv.drain();
    let _ = srv.finish(Duration::from_secs(5));
    println!("{}", r.summary());
    println!(
        "net: phase {:?} life {:?} updates {}",
        st.phase, st.life_states, st.updates_sent
    );
    assert_eq!(r.nan_ticks, 0);
    let of =
        |p: Phase| -> Vec<Sample> { input.log.iter().filter(|s| s.phase == p).copied().collect() };
    let (back, charge, slide) = (of(Phase::Back), of(Phase::Charge), of(Phase::Slide));
    println!(
        "samples: back {} charge {} slide {} (phase now {:?}, restarts {}, corrections {})",
        back.len(),
        charge.len(),
        slide.len(),
        input.phase,
        input.restarts,
        input.corrections
    );
    for l in srv
        .seen
        .iter()
        .filter(|l| l.contains("teleport") || l.contains("reject"))
    {
        println!("  server: {l}");
    }
    assert!(
        !back.is_empty(),
        "the stander was seen and the client backed away"
    );
    assert!(
        input
            .log
            .iter()
            .all(|s| s.corrections == input.run_corrections),
        "no server teleport inside the checked run"
    );
    assert!(
        charge.len() as u32 >= CHARGE_FRAMES - 1 && slide.len() as u32 >= SLIDE_FRAMES - 1,
        "the run finished: phase {:?}",
        input.phase
    );
    // the stander stands
    let still = charge.iter().chain(&slide).all(|s| {
        (s.remote[0] - charge[0].remote[0]).hypot(s.remote[2] - charge[0].remote[2]) < 1.0
    });
    assert!(still, "the stander did not move");

    // the charge: from about BACK_DIST away straight at the stander, never into its box, stopped at it
    let first = outside(&charge[0]);
    assert!(first > 80.0, "the charge started away from it: {first}");
    let deepest = charge.iter().map(outside).fold(f32::MAX, f32::min);
    let last = outside(charge.last().unwrap());
    println!("charge: start {first:.2} deepest {deepest:.3} end {last:.3} (0 = the grown box)");
    for s in charge.iter().step_by(10).chain(slide.iter().step_by(20)) {
        println!(
            "  {:?} own {:?} remote {:?} half {:?} outside {:.2}",
            s.phase,
            s.own,
            s.remote,
            s.half,
            outside(s)
        );
    }
    assert!(
        deepest > -0.5,
        "the own body entered the stander's box: {deepest}"
    );
    assert!(
        (0.0..0.5).contains(&last),
        "the own body stays at the box (0.1 back-off) while running into it: {last}"
    );
    let at_box = charge.iter().filter(|s| outside(s) < 1.0).count();
    assert!(
        at_box > 60,
        "held at the box for most of the charge: {at_box}"
    );

    // the slide: along the box and past it, never into it
    let deepest = slide.iter().map(outside).fold(f32::MAX, f32::min);
    let end = slide.last().unwrap();
    let start = slide[0];
    // progress along the charge direction: from the near side of the stander to its far side
    let axis = {
        let (dx, dz) = (
            start.remote[0] - start.own[0],
            start.remote[2] - start.own[2],
        );
        let l = dx.hypot(dz);
        (dx / l, dz / l)
    };
    let along = |s: &Sample| (s.own[0] - s.remote[0]) * axis.0 + (s.own[2] - s.remote[2]) * axis.1;
    println!(
        "slide: deepest {deepest:.3} along {:.1} -> {:.1}",
        along(&start),
        along(end)
    );
    assert!(
        deepest > -0.5,
        "the own body entered the stander's box: {deepest}"
    );
    assert!(
        along(end) > SYNTH_DIMS.z,
        "the own body slid past the stander: {:.1}",
        along(end)
    );
}
