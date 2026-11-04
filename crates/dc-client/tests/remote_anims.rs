//! ROM-gated end-to-end proof of the remote-character animation path: our `dc-dserver` on a ROM world (loopback
//! UDP, culling off), a `dc-botclient` driven by a scripted timeline (waits for the client's spawn, never fires, walks
//! up to the client and stands), and the client headless with its scripted shooter. Every frame's remote characters and corpses as the client would draw them are recorded; afterwards a
//! third-person camera at the client's eye renders three offscreen sequences with dc-render: the bot running (the
//! server's run trackers), the bot hit, and the bot dying (the hidden character replaced by the corpse object, its
//! death clip played locally from the create and held on its last frame). Checked: the run pose changes from frame
//! to frame; a corpse appears with the kill; its tracker stops at its clip's end and never wraps; the corpse ends
//! lying (its posed height well below the standing one).
//!
//! Ammunition and time: the client has about 120 rounds and used to fire them all at the bot while it was still
//! walking across the map (`ShootTest::range` 8000), so a slower bot found it empty (no hits, no corpse); the client
//! now shoots only at a bot within [`SHOOT_RANGE`]. The bot's walk across the map (about 15 s on an idle machine, up to twice that under load) and the client's
//! frame loop run on the wall clock of two processes, so the test does not budget a fixed number of frames. The bot's
//! clock starts at the client's spawn (timeline `sync`), and the frame loop runs until the event the checks need has
//! happened (the bot's corpse has been drawn for [`CORPSE_SECONDS`]), with a generous cap; a run that misses the
//! event fails with the state of every process. Frame stalls are logged and named in the failure message. Skipped without `CA_ROM` or a GPU adapter; the PNGs are
//! ROM-derived local files (`DC_ANIMS_OUT`, else dc-render's local output folder), never committed.
// SPEC: UDP_SESSION_2009_B.md §3.5.5 (death: hidden character, corpse type 0x26 with one tracker), §4.3 (three
// trackers per stream entry), §4.4 (state bits); RENDER_2009.md §7 (models)
// DC-DEFAULT: CLI-180, CLI-181 (proposed)

mod support;

use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use dc_client::anims::AnimFrame;
use dc_client::characters::ModelScene;
use dc_client::offscreen::Offscreen;
use dc_client::world::{ArmsFrame, ArmsResult, FrameState, GpuTarget, LoadedWorld};
use dc_client::{
    run_headless, ClientError, Connect, HeadlessOptions, InputSource, LocalUpdate, NetOptions,
    NetWorld, RemoteBody, RomWorld, ShootTest, ViewSettings, WorldSource,
};
use dc_render::{
    name_key, AdapterChoice, GpuContext, ModelFiles, RenderSettings, WorldRenderer,
    CLIP_LOWER_STAND, OFFSCREEN_FORMAT,
};
use dc_sim::{Correction, StepEvents, V3};
use support::*;

/// One recorded frame.
#[derive(Clone)]
struct Rec {
    ms: u32,
    remotes: Vec<RemoteBody>,
    anims: AnimFrame,
    hits: u32,
}

/// The network source, recording what the frame loop is handed.
struct Spy<'a> {
    inner: &'a mut NetWorld,
    recs: Vec<Rec>,
    hits: u32,
    /// Session clock of the first frame that had a corpse object.
    corpse_ms: Option<u32>,
    /// Session clock of the first frame that had the bot's character.
    bot_ms: Option<u32>,
    /// The bot's process: started at the client's first frame (the client is in the world then), to notice that it
    /// ended before it was ever seen.
    bot: Option<Proc>,
    /// Starts a bot process.
    start_bot: Box<dyn FnMut() -> Proc + 'a>,
    /// Bot restarts after a failed join (the bot's join timeout, 10 s, passed with no answer from a starved server).
    restarts: u32,
    /// Set when the bot's process ended before the client saw it (exit status).
    bot_ended: Option<String>,
    /// The bot's event log, to tell a failed join from a bot that ran and left.
    bot_log: PathBuf,
}

/// The run ends this long (session clock) after the first corpse was handed to the frame loop: the death clip
/// (1.3 to 1.4 s) has ended and the held frame has been drawn, and the pose sequence (3 s from the create) is complete.
const CORPSE_SECONDS: u32 = 3200;
/// How near the bot has to be for the client to shoot at it (world units).
const SHOOT_RANGE: f32 = 450.0;
/// The bot has to be seen by the client within this long (session clock); a bot that never joined ends the run at
/// once, with its state, instead of after the frame cap.
const BOT_DEADLINE_MS: u32 = 90_000;
/// Frames of the bot running that the pose check wants (normally; with a starved frame loop at least the second).
const RUN_FRAMES: usize = 24;
const RUN_FRAMES_STARVED: usize = 8;
/// Restarts of a bot whose join failed (see [`Spy::restarts`]).
const BOT_RESTARTS: u32 = 2;
/// The frame loop's cap (60 frames a second). Nothing but a failure reaches it: the bot's walk and the kill take
/// about 20 s idle.
const FRAME_CAP: u32 = 60 * 150;

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
        let mut f = self.inner.frame_state();
        self.hits += f
            .sound_cues
            .iter()
            .filter(|c| format!("{c:?}").contains("OwnHit"))
            .count() as u32;
        self.recs.push(Rec {
            ms: f.clock_ms.unwrap_or(0),
            remotes: f.remotes.clone(),
            anims: f.anims.clone(),
            hits: self.hits,
        });
        // the wait condition: the frame loop ends (stop_after_over) when the corpse has been drawn long enough
        let ms = f.clock_ms.unwrap_or(0);
        if self.bot_ms.is_none() && f.remotes.iter().any(|r| r.pid != 0xFF) {
            self.bot_ms = Some(ms);
        }
        if self.bot_ms.is_none() && ms >= BOT_DEADLINE_MS {
            f.session_over = true;
        }
        if self.bot.is_none() {
            self.bot = Some((self.start_bot)());
        }
        if self.bot_ms.is_none() && self.bot_ended.is_none() {
            let st = self
                .bot
                .as_mut()
                .and_then(|b| b.child.as_mut())
                .and_then(|c| c.try_wait().ok().flatten());
            if let Some(st) = st {
                let failed_join = std::fs::read_to_string(&self.bot_log)
                    .unwrap_or_default()
                    .contains("\"join_failed\"");
                if failed_join && self.restarts < BOT_RESTARTS {
                    // nothing of the bot reached the server (no hello), so a new bot can take its place
                    self.restarts += 1;
                    println!(
                        "UNDER LOAD: the bot's join timed out ({st}); starting it again ({} of {BOT_RESTARTS})",
                        self.restarts
                    );
                    self.bot = Some((self.start_bot)());
                } else {
                    self.bot_ended = Some(format!("{st}"));
                    f.session_over = true;
                }
            }
        }
        if !f.anims.corpses.is_empty() {
            let first = *self.corpse_ms.get_or_insert(ms);
            if ms >= first.saturating_add(CORPSE_SECONDS) {
                f.session_over = true;
            }
        }
        f
    }
    fn arms(&mut self, a: &ArmsFrame<'_>) -> ArmsResult {
        self.inner.arms(a)
    }
    fn rom(&self) -> Option<PathBuf> {
        self.inner.rom()
    }
}

fn out_dir() -> PathBuf {
    let d = out_dir_inner();
    let _ = std::fs::create_dir_all(&d);
    d
}

fn out_dir_inner() -> PathBuf {
    std::env::var("DC_ANIMS_OUT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| dc_render::default_out_dir().join("client-anims"))
}

/// The bot's event log (jsonl) boiled down to the lines that explain a missed kill: when it spawned, what the
/// timeline did, what it saw, whether it fired, was hit or died.
fn bot_notes(log: &std::path::Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let mut out = format!("bot events ({}):\n", log.display());
    let mut stuck = 0;
    for l in text.lines() {
        let Ok(e) = serde_json::from_str::<serde_json::Value>(l) else {
            continue;
        };
        let ev = e["ev"].as_str().unwrap_or("");
        match ev {
            "spawn" | "timeline_start" | "timeline" | "sight" | "fire" | "damage" | "died"
            | "kill" | "respawned" | "server_gone" | "server_bye" | "link_closed" | "quit" => {
                let mut v = e.clone();
                for k in ["bot", "wall", "cone_table"] {
                    v.as_object_mut().map(|o| o.remove(k));
                }
                out.push_str(&format!("  {v}\n"));
            }
            "hunt_stuck" => stuck += 1,
            _ => {}
        }
    }
    out.push_str(&format!("  hunt_stuck events: {stuck}\n"));
    out
}

#[test]
fn rom_remote_character_runs_is_hit_and_leaves_a_falling_corpse() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let ctx = match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => c,
        Err(e) => {
            println!("skipped: no GPU adapter ({e})");
            return;
        }
    };
    let maps = RomWorld::map_names(&rom).expect("ROM world archive");
    // a small indoor map keeps the hunt short; any map works
    let map = maps
        .iter()
        .find(|m| m.eq_ignore_ascii_case("JUNK_FLEA"))
        .or(maps.first())
        .cloned()
        .expect("a ROM map");
    // a hard limit for the whole test: a lost process must not hang the run
    let watchdog = Watchdog::arm("remote_anims", Duration::from_secs(420));
    let mut cmd = std::process::Command::new(dserver_exe());
    cmd.args([
        "-port",
        "0",
        "-world",
        &format!("Worlds/{map}"),
        "-idle",
        "30",
        "-tick",
        "20",
        "-roundsecs",
        "600",
        "-loadout",
        "145,164",
        "-visibility",
        "off",
    ]);
    let mut srv = Proc::spawn(cmd);
    watchdog.watch_pid(srv.pid());
    let ready = srv
        .wait_line("@ready", Duration::from_secs(60))
        .unwrap_or_else(|| panic!("no @ready; server said {:?}", srv.seen));
    let port: u16 = ready
        .split_whitespace()
        .find_map(|t| t.strip_prefix("port="))
        .and_then(|p| p.parse().ok())
        .expect("port");
    srv.line("room key=TDM type=1 goal=50 ff=0 time=600");
    srv.line("user 1 1 1 0 0 AnimClient");
    srv.line("user 2 2 2 1 0 AnimBot");
    // The bot's script: its clock starts when the client is spawned (so its walk never races the client's world
    // load), it never fires, and it walks up to the client and stands there to be shot.
    let dir = scratch("remote-anims");
    let timeline = dir.join("bot.timeline");
    std::fs::write(
        &timeline,
        "sync animclient 180000\n@0 hold\n@0 approach animclient\n",
    )
    .expect("write the bot timeline");

    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "AnimClient",
    );
    o.rom = Some(rom.clone());
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    // The bot joins after the client: the server loads the world on the first hello, which takes 20 s on a busy
    // machine, and a bot whose hello waits that long gives up after its 10 s join timeout. The server's `@join`
    // line says that the world is loaded and the client is in.
    if srv
        .wait_line("@join 0 ", Duration::from_secs(180))
        .is_none()
    {
        srv.drain();
        panic!(
            "the client did not join the server within 180 s; the server said {:?}",
            srv.seen
        );
    }
    // the bot's logs are per run (the output folder can be shared by test runs of several worktrees)
    let bot_log = dir.join("bot_events.jsonl");
    let bot_err = dir.join("bot_stderr.txt");
    let bot_exe = workspace_bin("dc-botclient", "DC_BOTCLIENT_EXE", "dc-botclient");
    let start_bot = || -> Proc {
        let mut b = std::process::Command::new(&bot_exe);
        b.args([
            "--udp",
            &format!("127.0.0.1:{port}"),
            "--name",
            "AnimBot",
            "--user",
            "2",
            "--hunt",
            "--shooter",
            "--duration",
            "400",
            "--timeline",
        ])
        .arg(&timeline)
        .arg("--log")
        .arg(&bot_log)
        .arg("--rom")
        .arg(&rom);
        let bot = Proc::spawn_with_stderr(
            b,
            std::fs::File::create(&bot_err).map_or_else(|_| Stdio::null(), Stdio::from),
        );
        watchdog.watch_pid(bot.pid());
        bot
    };

    let opts = HeadlessOptions {
        frames: FRAME_CAP,
        frame_us: 16_667,
        width: 64,
        height: 48,
        out: None,
        realtime: true,
        // the Spy ends the run (it reports the session over) when the corpse has been drawn long enough
        stop_after_over: Some(0),
        ..HeadlessOptions::default()
    };
    let mut shoot = ShootTest::new(1.0 / 60.0, None);
    // The client fires only at a bot that is near. The shooter holds one weapon with a fixed supply (about 120
    // rounds): with the default range of 8000 it empties it at the bot while the bot is still walking across the
    // map behind walls (about 6 rounds a second from its first sight), and a bot that arrives later than that
    // (a busy machine slows its walk) finds nothing left to be shot with. Near the client the line is usually open.
    shoot.range = SHOOT_RANGE;
    let mut spy = Spy {
        inner: &mut net,
        recs: Vec::new(),
        hits: 0,
        corpse_ms: None,
        bot_ms: None,
        bot: None,
        start_bot: Box::new(start_bot),
        restarts: 0,
        bot_ended: None,
        bot_log: bot_log.clone(),
    };
    let ran = run_headless(&mut spy, Some(&mut shoot as &mut dyn InputSource), &opts);
    let recs = std::mem::take(&mut spy.recs);
    let (corpse_ms, bot_ended, restarts) = (spy.corpse_ms, spy.bot_ended.take(), spy.restarts);
    let mut bot = spy
        .bot
        .take()
        .expect("the bot was started with the first frame");
    drop(spy);
    handle.shutdown(Duration::from_secs(5));
    srv.quit();
    srv.finish(Duration::from_secs(5));
    let bot_exit = bot.finish(Duration::from_secs(3));
    srv.drain();
    bot.drain();
    let report = ran.unwrap_or_else(|e| panic!("headless run failed: {e:?}"));
    let kills = srv.seen.iter().filter(|l| l.contains("killed by")).count();
    println!(
        "{} frames recorded, server kills {kills}; {}",
        recs.len(),
        report.stall_note()
    );
    // the state of every process, for a failure message (and the local run log)
    let state = || -> String {
        let mut t = String::new();
        let bot_obj = recs
            .iter()
            .flat_map(|r| r.remotes.iter())
            .find(|r| r.pid != 0xFF)
            .map(|r| r.obj);
        t.push_str(&format!(
            "frames {} (cap {FRAME_CAP}), first corpse at {:?}, own hits {}, server kills {kills}; {}\n",
            recs.len(),
            recs.iter()
                .find(|r| !r.anims.corpses.is_empty())
                .map(|r| r.ms),
            recs.last().map_or(0, |r| r.hits),
            report.stall_note()
        ));
        // the bot's track, one sample every 2 s of the session clock
        t.push_str("  bot track:");
        let mut next = 0;
        for r in &recs {
            if r.ms >= next {
                next = r.ms + 2000;
                if let Some(b) = bot_obj.and_then(|o| r.remotes.iter().find(|x| x.obj == o)) {
                    t.push_str(&format!(
                        " [{} {:.0} {:.0} {:.0}]",
                        r.ms, b.pos[0], b.pos[1], b.pos[2]
                    ));
                }
            }
        }
        t.push('\n');
        let mut last = None;
        for r in &recs {
            let b = bot_obj.and_then(|o| r.remotes.iter().find(|x| x.obj == o));
            let vis = b.map(|b| b.look.as_ref().is_some_and(|l| l.visible));
            let key = (vis, r.anims.corpses.len(), r.hits);
            if last != Some(key) {
                t.push_str(&format!(
                    "  client frame t={} bot={:?} visible={vis:?} corpses={} own hits={}\n",
                    r.ms,
                    b.map(|b| b.pos.map(|c| c.round())),
                    r.anims.corpses.len(),
                    r.hits
                ));
                last = Some(key);
            }
        }
        t.push_str(&format!(
            "bot process exit code {bot_exit:?} (ended before it was seen: {bot_ended:?}; restarts {restarts}); stderr {}:\n",
            bot_err.display()
        ));
        for l in std::fs::read_to_string(&bot_err)
            .unwrap_or_default()
            .lines()
            .take(12)
        {
            t.push_str(&format!("  {l}\n"));
        }
        t.push_str(&bot_notes(&bot_log));
        t.push_str("dserver (all lines):\n");
        for l in srv.seen.iter().take(120) {
            t.push_str(&format!("  {l}\n"));
        }
        t
    };
    let _ = std::fs::write(dir.join("run_log.txt"), state());
    println!("run log and bot logs in {}", dir.display());

    // the bot's character object (the one remote player)
    let bot_obj = recs
        .iter()
        .flat_map(|r| r.remotes.iter())
        .find(|r| r.pid != 0xFF)
        .map(|r| r.obj)
        .unwrap_or_else(|| {
            panic!(
                "the bot was never seen (its process ended before the client saw it: {bot_ended:?})\n{}",
                state()
            )
        });
    // The wait condition has to have been met: the frame loop ends on its own only after the corpse was drawn.
    assert!(
        corpse_ms.is_some(),
        "no corpse appeared within {} s of frames: the bot never died (the client's hits {}, server kills {kills})\n{}",
        FRAME_CAP / 60,
        recs.last().map_or(0, |r| r.hits),
        state()
    );
    let body = |r: &Rec| r.remotes.iter().find(|x| x.obj == bot_obj).cloned();

    // models and the world for the offscreen frames
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut src = RomWorld::new(&rom, &map);
    let world = src
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))
        .expect("ROM world");
    let scene = world.scene.as_ref().expect("scene");
    let lights = world.model_lights.as_ref();
    let mut models = ModelScene::open(&rom).expect("ROM models");
    let default_body = models.default_look();
    let vfs = dc_rez::RezVfs::new(&rom);
    let anim = ModelFiles::load(&vfs, &default_body[0].model)
        .expect("body model")
        .anim_set();
    let stand_lower = anim
        .find_clip(|n| name_key(n) == CLIP_LOWER_STAND)
        .expect("the stand clip");
    let (w, h) = (480u32, 360u32);
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, w, h);
    let out = out_dir();
    let _ = std::fs::create_dir_all(&out);
    let mut log = std::fs::File::create(out.join("remote_anims.txt")).ok();

    // renders one recorded frame: the given bodies, from `eye` toward `at`; returns the posed body heights
    let mut shot = |name: &str, r: &Rec, bodies: &[RemoteBody], at: [f32; 3], eye: [f32; 3]| {
        let fs = AnimFrame {
            corpses: Vec::new(),
            once: r.anims.once.clone(),
        };
        // the corpses are passed as bodies here (with their play-once flags)
        models.update_remotes_animated(&ctx, &renderer, bodies, &fs, lights);
        // a second update poses with the clip lengths of the bodies built by the first
        models.update_remotes_animated(&ctx, &renderer, bodies, &fs, lights);
        let inst = models.instances();
        let size = inst
            .first()
            .and_then(|i| i.bounds_in_world())
            .map(|(lo, hi)| {
                (
                    hi.y - lo.y,
                    ((hi.x - lo.x).powi(2) + (hi.z - lo.z).powi(2)).sqrt(),
                )
            });
        let view = ShootTest::look_at(eye, at);
        let camera = ViewSettings::default().camera(V3::from_array(eye), &view, w, h);
        let img = target
            .render_with(&ctx, &renderer, scene, &camera, None, &inst, None)
            .unwrap();
        let _ = img.save_png(&out.join(format!("{name}.png")));
        if let Some(l) = log.as_mut() {
            // the trackers as posed (after the play-once rule)
            let tr: Vec<String> = inst
                .first()
                .map(|m| m.trackers.iter())
                .into_iter()
                .flatten()
                .map(|t| format!("{}@{:.1}/{:?}", t.clip, t.time_ms, t.blend_set))
                .collect();
            let _ = writeln!(
                l,
                "{name} t={} size {size:?} trackers {}",
                r.ms,
                tr.join(" ")
            );
        }
        size
    };
    // a camera beside the body (its right side first, then around it), 140 units away and `lift` above the box
    // centre, with a free line of sight to the body
    let collision = &world.collision;
    let cam = |b: &RemoteBody, lift: f32| -> [f32; 3] {
        let yaw = b.yaw.unwrap_or(0.0);
        let at = V3::from_array(b.pos);
        for k in 0..16 {
            let a = yaw + std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::TAU / 16.0;
            for r in [140.0f32, 110.0, 80.0] {
                let c = at + V3::new(a.sin() * r, lift, a.cos() * r);
                if collision
                    .ray(at + V3::new(0.0, lift * 0.5, 0.0), c)
                    .is_none()
                {
                    return c.to_array();
                }
            }
        }
        (at + V3::new(140.0, lift, 0.0)).to_array()
    };

    // 1. running: frames where the bot moves fast with a moving lower-body clip
    let mut run_frames: Vec<usize> = Vec::new();
    for i in 1..recs.len() {
        let (Some(a), Some(b)) = (body(&recs[i - 1]), body(&recs[i])) else {
            continue;
        };
        let dt = recs[i].ms.saturating_sub(recs[i - 1].ms).max(1) as f32 / 1000.0;
        let v = ((b.pos[0] - a.pos[0]).powi(2) + (b.pos[2] - a.pos[2]).powi(2)).sqrt() / dt;
        let lower = b
            .look
            .as_ref()
            .and_then(|l| l.trackers.get(2))
            .map(|t| t.clip);
        if v > 150.0
            && lower.is_some_and(|c| c != stand_lower)
            && b.look.as_ref().is_some_and(|l| l.visible)
        {
            run_frames.push(i);
        }
    }
    println!("run frames {}", run_frames.len());
    // A frame loop that a busy machine starved (several stalls after the first model load: the bot ran its 15 s
    // walk in front of a handful of frames) has fewer run frames; eight are enough for the pose check.
    let late_stalls = report.stalls.iter().filter(|s| s.frame > 2).count();
    let starved = late_stalls >= 5;
    let need = if starved {
        RUN_FRAMES_STARVED
    } else {
        RUN_FRAMES
    };
    if starved && run_frames.len() < RUN_FRAMES {
        println!(
            "UNDER LOAD: {} run frames (normally >= {RUN_FRAMES}) after {late_stalls} frame stalls; {}",
            run_frames.len(),
            report.stall_note()
        );
    }
    assert!(
        run_frames.len() >= need,
        "the bot ran: {} frames, {need} needed\n{}",
        run_frames.len(),
        state()
    );
    // eight frames 100 ms apart around the middle of the run; with too few run frames for that, eight spread
    // over all of them
    let start = run_frames[run_frames.len() / 2];
    let spaced = start + 7 * 6 < recs.len() && run_frames.len() >= RUN_FRAMES;
    let mut run_heights = Vec::new();
    let mut run_widths = Vec::new();
    for k in 0..8 {
        let i = if spaced {
            start + k * 6
        } else {
            run_frames[k * run_frames.len() / 8]
        };
        let Some(b) = body(&recs[i]) else { continue };
        let eye = cam(&b, 20.0);
        if let Some((ht, wd)) = shot(
            &format!("run_{k}"),
            &recs[i],
            std::slice::from_ref(&b),
            b.pos,
            eye,
        ) {
            run_heights.push(ht);
            run_widths.push(wd);
        }
    }
    let wmin = run_widths.iter().cloned().fold(f32::MAX, f32::min);
    let wmax = run_widths.iter().cloned().fold(0.0, f32::max);
    println!("run: body footprint {wmin:.1}..{wmax:.1}, heights {run_heights:?}");
    assert!(
        wmax - wmin > 3.0,
        "the run pose changes: {run_widths:?}\n{}",
        state()
    );

    // 2. hit: the frames around the client's first hit on the bot (the shooter's hit feedback)
    if let Some(i) = recs.iter().position(|r| r.hits > 0) {
        for (k, j) in [i.saturating_sub(3), i, i + 3, i + 6, i + 9, i + 12]
            .into_iter()
            .enumerate()
        {
            let j = j.min(recs.len() - 1);
            if let Some(b) = body(&recs[j]) {
                let eye = cam(&b, 20.0);
                shot(
                    &format!("hit_{k}"),
                    &recs[j],
                    std::slice::from_ref(&b),
                    b.pos,
                    eye,
                );
            }
        }
    }

    // 2b. the hit reaction (DSV-127, on by default): after a hit that did not kill, the bot's upper-body tracker
    // plays one of the body's two hit clips once (the bot never fires, so no fire one-shot competes)
    // DC-DEFAULT: DSV-127 (proposed; project decision on)
    let hit_clips: Vec<usize> = [
        dc_dserver::anims::CLIP_HIT,
        dc_dserver::anims::CLIP_HIT_BACK,
    ]
    .iter()
    .filter_map(|k| anim.find_clip(|n| name_key(n) == *k))
    .collect();
    assert_eq!(hit_clips.len(), 2, "the body has both hit clips");
    let hit_frame = recs.iter().position(|r| {
        body(r).is_some_and(|b| {
            b.look.as_ref().is_some_and(|l| {
                l.visible
                    && l.trackers
                        .get(1)
                        .is_some_and(|t| hit_clips.contains(&t.clip))
            })
        })
    });
    let hf = hit_frame.unwrap_or_else(|| {
        panic!(
            "the bot's upper body never played a hit clip (own hits {})
{}",
            recs.last().map_or(0, |r| r.hits),
            state()
        )
    });
    assert!(
        recs[hf].anims.once_of(bot_obj).get(1) == Some(&true),
        "the hit clip plays once (state bit 1 clear)
{}",
        state()
    );
    let hit_clip = body(&recs[hf]).unwrap().look.unwrap().trackers[1].clip;
    println!(
        "hit reaction: clip {hit_clip} ({:?} ms) on tracker 1 at t={} (first own hit at t={:?})",
        anim.clip_length_ms(hit_clip),
        recs[hf].ms,
        recs.iter().find(|r| r.hits > 0).map(|r| r.ms)
    );
    for (k, j) in [hf, hf + 6, hf + 12, hf + 18].into_iter().enumerate() {
        let j = j.min(recs.len() - 1);
        if let Some(b) = body(&recs[j]) {
            let eye = cam(&b, 20.0);
            shot(
                &format!("hitclip_{k}"),
                &recs[j],
                std::slice::from_ref(&b),
                b.pos,
                eye,
            );
        }
    }

    // 3. death: the first corpse; the bot's character hidden from then on
    // the bot's corpse: the first corpse within 80 units of where the bot was last drawn standing
    let (first, corpse_obj) = (1..recs.len())
        .find_map(|i| {
            let last = (0..i).rev().find_map(|j| {
                body(&recs[j]).filter(|b| b.look.as_ref().is_some_and(|l| l.visible))
            })?;
            recs[i].anims.corpses.iter().find_map(|c| {
                let d =
                    ((c.pos[0] - last.pos[0]).powi(2) + (c.pos[2] - last.pos[2]).powi(2)).sqrt();
                (d < 80.0).then_some((i, c.obj))
            })
        })
        .unwrap_or_else(|| {
            panic!(
                "no corpse object appeared at the bot's last standing place\n{}",
                state()
            )
        });
    // from the corpse's create on, the bot's character is hidden (flags 0x0802) until its respawn
    assert!(
        body(&recs[first]).is_none_or(|b| !b.look.as_ref().is_some_and(|l| l.visible)),
        "the dead character is hidden while its corpse is drawn\n{}",
        state()
    );
    let corpse_at = |i: usize| {
        recs[i]
            .anims
            .corpses
            .iter()
            .find(|c| c.obj == corpse_obj)
            .cloned()
    };
    // The frame as the window draws it (every remote character plus the corpses): the character, drawn standing in
    // the frame before, is gone from the scene in the frame its corpse appears and the corpse is there
    // (user retest 2026-10-26 item 7: the last living pose stayed drawn upright until the respawn).
    // DC-DEFAULT: CLI-350 (proposed)
    let mut window_scene = ModelScene::open(&rom).expect("ROM models");
    let mut window_frame = |r: &Rec| {
        for _ in 0..2 {
            window_scene.update_remotes_animated(&ctx, &renderer, &r.remotes, &r.anims, lights);
        }
        window_scene.drawn()
    };
    let before = window_frame(&recs[first - 1]);
    let after = window_frame(&recs[first]);
    assert!(
        body(&recs[first - 1]).is_none_or(|b| !b.look.as_ref().is_some_and(|l| l.visible))
            || before.contains(&bot_obj),
        "the living bot is drawn in the frame before its corpse: {before:?}
{}",
        state()
    );
    assert!(
        !after.contains(&bot_obj) && after.contains(&corpse_obj),
        "the frame of the corpse draws the corpse {corpse_obj} and not the hidden character {bot_obj}: {after:?}
{}",
        state()
    );
    // and so it stays while the corpse lies there (until the respawn)
    for r in recs[first..]
        .iter()
        .take_while(|r| body(r).is_none_or(|b| !b.look.as_ref().is_some_and(|l| l.visible)))
    {
        let d = window_frame(r);
        assert!(
            !d.contains(&bot_obj),
            "the dead character is not drawn at t={}: {d:?}
{}",
            r.ms,
            state()
        );
    }
    let c0 = corpse_at(first).unwrap();
    let clip = c0.look.as_ref().unwrap().trackers[0].clip;
    let len = anim.clip_length_ms(clip).expect("the death clip") as f32;
    assert!(
        recs[first].anims.once_of(corpse_obj) == [true],
        "the corpse's tracker plays once\n{}",
        state()
    );
    // the standing height for comparison: the last frame of the bot before its corpse (or of any frame)
    let standing = run_heights.iter().cloned().fold(0.0, f32::max);
    let mut k = 0;
    let t0 = recs[first].ms;
    let mut i = first;
    while i < recs.len() && recs[i].ms <= t0 + 3000 {
        if let Some(c) = corpse_at(i) {
            let dt = recs[i].ms - t0;
            if k == 0 || dt >= k * 150 {
                // from above: the death clip throws the body 1-2 m from its box
                let eye = cam(&c, 120.0);
                let at = [c.pos[0], c.pos[1] - 30.0, c.pos[2]];
                // every body of the frame (the hidden character too), as the window hands them over
                let mut all = recs[i].remotes.clone();
                all.push(c.clone());
                shot(&format!("death_{k:02}"), &recs[i], &all, at, eye);
                k += 1;
            }
        }
        i += 1;
    }
    // the held frame: the corpse's tracker never passes its clip end
    let late = (first..recs.len())
        .rev()
        .find(|&i| corpse_at(i).is_some() && recs[i].ms >= t0 + 2000)
        .unwrap_or_else(|| {
            panic!(
                "the corpse did not stay until 2 s after its create\n{}",
                state()
            )
        });
    let mut c_late = corpse_at(late).unwrap();
    // the corpse's pose in the held frame (a frame loop that stalled for seconds can have no sequence frame in
    // the clip's last part)
    let last_height = {
        let eye = cam(&c_late, 120.0);
        let at = [c_late.pos[0], c_late.pos[1] - 30.0, c_late.pos[2]];
        shot(
            "death_end",
            &recs[late],
            std::slice::from_ref(&c_late),
            at,
            eye,
        )
        .map(|s| s.0)
    };
    {
        let once = recs[late].anims.once_of(corpse_obj).to_vec();
        let look = c_late.look.as_mut().unwrap();
        dc_client::anims::hold_once(&mut look.trackers, &once, |c| anim.clip_length_ms(c));
        assert_eq!(
            look.trackers[0].time_ms,
            len - dc_client::anims::HOLD_BEFORE_END_MS,
            "held on the clip's last frame\n{}",
            state()
        );
    }
    println!(
        "death clip {clip} ({len} ms): standing height {standing:.1}, corpse height at the end {last_height:?}"
    );
    let lying = last_height.expect("the corpse was posed");
    assert!(
        lying < 0.6 * standing,
        "the corpse lies: {lying:.1} vs standing {standing:.1}\n{}",
        state()
    );
    println!(
        "PNGs and log in {} (ROM-derived, never commit)",
        out.display()
    );
}
