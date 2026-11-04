//! The remote body's facing and aim as the frame loop is handed them: `dc-dserver` over loopback (the made-up world of
//! `dc_client::testkit`), a dc-botclient walking in a circle (its body yaw turns all the time), and the client
//! headless. Every frame's remote characters are recorded. Checked: the walker's `RemoteBody.yaw` takes many values
//! and follows the stream (playtest report: the remote body did not turn in the Rust client; the stream yaw
//! path was not the cause).
// SPEC: UDP_SESSION_2009_B.md §4.3 (yaw8 in every character entry)

mod support;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::run_game;
use dc_botclient::session::{BotSession, GameConfig};
use dc_client::testkit::{synth_font, synth_source, SYNTH_DIMS};
use dc_client::world::{ArmsFrame, ArmsResult, FrameState, GpuTarget, LoadedWorld};
use dc_client::{
    run_headless, ClientError, Connect, HeadlessOptions, LocalUpdate, NetOptions, NetWorld,
    RemoteBody, WorldFile, WorldSource,
};
use dc_sim::{Correction, StepEvents};
use support::*;

struct Spy<'a> {
    inner: &'a mut NetWorld,
    frames: Vec<Vec<RemoteBody>>,
}

impl WorldSource for Spy<'_> {
    fn describe(&self) -> String {
        self.inner.describe()
    }
    fn load(&mut self, gpu: Option<GpuTarget<'_>>) -> Result<LoadedWorld, ClientError> {
        self.inner.load(gpu)
    }
    fn poll_corrections(&mut self) -> Vec<Correction> {
        self.inner.poll_corrections()
    }
    fn report(&mut self, u: &LocalUpdate) {
        self.inner.report(u)
    }
    fn moved(&mut self, e: &StepEvents) {
        self.inner.moved(e)
    }
    fn frame_state(&mut self) -> FrameState {
        let f = self.inner.frame_state();
        self.frames.push(f.remotes.clone());
        f
    }
    fn arms(&mut self, a: &ArmsFrame<'_>) -> ArmsResult {
        self.inner.arms(a)
    }
    fn rom(&self) -> Option<PathBuf> {
        self.inner.rom()
    }
}

#[test]
fn walker_turns_in_the_frames_handed_to_the_draw() {
    let dir = scratch("remote_turn");
    let world = synth_world_file(&dir);
    let (mut srv, port) = start_dserver(&world, &["-tick", "20", "-roundsecs", "60", "-seed", "7"]);
    srv.line("user 1 1 1 0 0 NetClient");
    srv.line("user 2 2 2 1 0 WalkBot");
    let wpath = world.clone();
    let bot = std::thread::spawn(move || {
        let mut cfg = GameConfig::new("WalkBot");
        cfg.hello_tag = "walkbot".into();
        cfg.motion = MotionKind::Circle;
        cfg.world_file = Some(wpath);
        cfg.load_ms = 300;
        let mut bot = BotSession::new(cfg, None, Recorder::null("WalkBot"));
        let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let _ = run_game(
            &mut bot,
            server,
            Duration::from_secs(14),
            &Recorder::null("WalkBot"),
        );
    });
    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "NetClient",
    );
    let s = synth_source();
    o.world_file = Some(WorldFile {
        bytes: s.bytes,
        stand_dims: SYNTH_DIMS,
        textures: s.textures,
    });
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let opts = HeadlessOptions {
        frames: 60 * 12,
        frame_us: 16_667,
        width: 160,
        height: 120,
        out: None,
        font_bytes: Some(synth_font()),
        realtime: true,
        ..HeadlessOptions::default()
    };
    let mut spy = Spy {
        inner: &mut net,
        frames: Vec::new(),
    };
    let _ = run_headless(&mut spy, None, &opts).expect("headless run");
    let frames = std::mem::take(&mut spy.frames);
    handle.shutdown(Duration::from_secs(5));
    let _ = bot.join();
    srv.quit();
    srv.drain();
    let walker: Vec<&RemoteBody> = frames
        .iter()
        .filter_map(|f| f.iter().find(|r| r.name == "WalkBot"))
        .collect();
    assert!(
        walker.len() > 60,
        "frames with the walker: {}",
        walker.len()
    );
    let mut yaws: Vec<i32> = walker
        .iter()
        .filter_map(|r| r.yaw)
        .map(|y| (y * 100.0).round() as i32)
        .collect();
    yaws.dedup();
    assert!(
        yaws.len() > 10,
        "the walker's drawn yaw changes with the stream: {} values",
        yaws.len()
    );
    // the made-up world has no character records (no ROM tables), so no aim notes here: the notes' path is
    // tests/remote_aim.rs and dc-dserver game_sim view_pitch_is_relayed_as_aim_notes
}
