//! The view recoil and the hit jar end to end over loopback UDP: `dc-dserver` (the workspace binary) with the
//! made-up world of `dc_client::testkit` and the two-weapon loadout 145 / 164 (rows from the user's ROM), a
//! dc-botclient shooter on the other team (the client starts with a health it cannot lose in the run), and the
//! client headless with the
//! "Reality" option on, holding the fire button with a still mouse.
//!
//! Checked from the client's own capture (its 0x82s) and its session counters:
//! - **recoil**: every 0x82 `aim` (yaw and pitch of the view) equals the recoil rule replayed on the 0x82 `time_ms`
//!   alone ([`dc_client::recoil::CameraRecoil`] with the ROM row): the kicks, the yaw's side changes and the drain
//!   depend on the session clock and the shot times, not on the client's frames; the yaw moved (the 2009 yaw kick).
//! - **jar**: the client jarred its camera on the bot's hits (the 0x74 damage type of the bot's ammunition jars by
//!   the ROM damage-type table), and every pellet's `dir` is the cone around the *camera* rotation (the view plus
//!   the jar's offsets at the shot's time), not around the reported `aim`, exactly (the reference cone replay).
//!
//! Skipped without `CA_ROM` (the weapon rows and the damage-type table are the ROM's).
// SPEC: DSERVER_PIPE.md §2-§4; GAME_MESSAGES_2009_C2S.md §4.1; GAME_MESSAGES_2009_S2C.md §4.3 (0x74)
// EVIDENCE: cshell+0x1f8b40, +0x1f5150, +0x1f2f60, +0x1f3070 (recoil); cshell+0x1f7d20 (jar); DISASM
// cshell+0x1fafba..0x1fb266 (camera rotation of the shots)

mod cone_ref;
mod support;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::run_game;
use dc_botclient::session::{BotSession, GameConfig};
use dc_botclient::shooter::ShooterConfig;
use dc_client::arms::{WeaponCommands, WeaponTable};
use dc_client::recoil::CameraRecoil;
use dc_client::testkit::{synth_font, synth_source, SYNTH_DIMS};
use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetOptions, NetWorld, ViewAngles,
    WorldFile,
};
use dc_sim::Commands;
use support::*;

fn world_file() -> WorldFile {
    let s = synth_source();
    WorldFile {
        bytes: s.bytes,
        stand_dims: SYNTH_DIMS,
        textures: s.textures,
    }
}

/// Holds the fire button from frame `from` on, never moves the mouse.
struct Trigger {
    frame: u64,
    from: u64,
}

impl InputSource for Trigger {
    fn frame(&mut self, _view: &mut ViewAngles) {
        self.frame += 1;
    }
    fn tick(&mut self, _tick: u64, _hz: u32, _view: &mut ViewAngles) -> Commands {
        Commands::default()
    }
    fn weapon(&mut self) -> WeaponCommands {
        WeaponCommands {
            fire: self.frame >= self.from,
            ..WeaponCommands::default()
        }
    }
    fn label(&self) -> &'static str {
        "trigger"
    }
}

fn shooter(port: u16, world: PathBuf, secs: u64) -> std::thread::JoinHandle<BotSession> {
    std::thread::spawn(move || {
        let mut cfg = GameConfig::new("Shooter");
        cfg.hello_tag = "shooter".into();
        cfg.motion = MotionKind::Stand;
        cfg.world_file = Some(world);
        cfg.load_ms = 2000;
        cfg.shooter = Some(ShooterConfig {
            shot_period_ms: 350,
            ..ShooterConfig::default()
        });
        let mut bot = BotSession::new(cfg, None, Recorder::null("Shooter"));
        let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        run_game(
            &mut bot,
            server,
            Duration::from_secs(secs),
            &Recorder::null("Shooter"),
        )
        .expect("bot udp");
        bot
    })
}

fn wrap(a: f64) -> f64 {
    (a + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI
}

/// Yaw and pitch (radians, pitch positive down) of a 0x82 `aim` rotation.
fn yaw_pitch(q: [f32; 4]) -> (f64, f64) {
    let f = dc_sim::Quat {
        x: q[0],
        y: q[1],
        z: q[2],
        w: q[3],
    }
    .forward();
    (
        (f.x as f64).atan2(f.z as f64),
        -(f.y as f64).clamp(-1.0, 1.0).asin(),
    )
}

#[test]
fn recoil_follows_the_rule_and_hits_jar_the_camera_over_loopback() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: needs CA_ROM (weapon rows, damage-type table)");
        return;
    };
    let table = WeaponTable::open(&rom).expect("weapon table");
    let _wd = Watchdog::arm("net_recoil", Duration::from_secs(300));
    let dir = scratch("recoil");
    let world = synth_world_file(&dir);
    let (mut srv, port) = start_dserver(
        &world,
        &[
            "-tick",
            "20",
            "-roundsecs",
            "60",
            "-holds",
            "1500,800",
            "-seed",
            "17",
            "-loadout",
            "145,164",
            "-combat",
            "start_health=100000,max_health=100000",
            "-nomovecheck",
        ],
    );
    srv.line("user 1 1 1 0 0 RecoilClient");
    srv.line("user 2 2 2 1 0 Shooter");
    let bot = shooter(port, world.clone(), 24);

    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "RecoilClient",
    );
    o.world_file = Some(world_file());
    o.capture = Some(dir.join("client_capture.jsonl"));
    o.reality = true;
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    // 60 frames a second paced to the wall clock: the bot needs real time to come in and shoot
    let opts = HeadlessOptions {
        frames: 60 * 18,
        frame_us: 16_667,
        width: 160,
        height: 120,
        out: None,
        save_every: 1_000_000,
        font_bytes: Some(synth_font()),
        realtime: true,
        ..HeadlessOptions::default()
    };
    let mut input = Trigger {
        frame: 0,
        from: 150,
    };
    let r = run_headless(&mut net, Some(&mut input as &mut dyn InputSource), &opts)
        .expect("headless run");
    let st = handle.shutdown(Duration::from_secs(5));
    let _ = bot.join();
    srv.quit();
    srv.finish(Duration::from_secs(5));
    srv.drain();
    println!("{}", r.summary());
    println!(
        "net: phase {:?} arms {:?} damage taken {} jars {} (first {:?}) wags {} flips {} life {:?}",
        st.phase,
        st.arms,
        st.damage_taken,
        st.camera_jars.len(),
        st.camera_jars.first(),
        st.recoil_wags,
        st.recoil_flips,
        st.life_states
    );
    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    assert_eq!(
        st.weapon.map(|c| c.weapon),
        Some(145),
        "the client holds 145"
    );
    assert!(st.arms.shots >= 40, "{:?}", st.arms);

    let d = dc_botclient::capdecode::decode_file(&dir.join("client_capture.jsonl"))
        .expect("client capture");
    let fires = cone_ref::fires(&d);
    let shots: Vec<_> = fires
        .values()
        .flatten()
        .map(|(_, f)| *f)
        .filter(|f| f.weapon_id == 145)
        .collect();
    assert!(shots.len() >= 40, "{} 0x82 of weapon 145", shots.len());

    // --- recoil: the rule replayed on the 0x82 times alone
    let row = table.spread.get(145).cloned().unwrap_or_default().recoil;
    let mut cam = CameraRecoil::default();
    let (y0, p0) = yaw_pitch(shots[0].aim);
    let (mut yaw, mut pitch) = (0.0f64, 0.0f64);
    let (mut worst, mut yaw_span) = (0.0f64, 0.0f64);
    for (i, f) in shots.iter().enumerate() {
        let t = f.time_ms as f64;
        let dr = cam.drain(&row, t, dc_client::spread::clock::TICK_MS, false);
        yaw += dr.yaw as f64;
        pitch += dr.pitch as f64;
        let (y, p) = yaw_pitch(f.aim);
        let (dy, dp) = (wrap(y - y0), p - p0);
        let e = (dy - yaw).abs().max((dp - pitch).abs()).to_degrees();
        if e > worst {
            println!(
                "shot {i} t {}: sent yaw {:+.3} pitch {:+.3}, rule {:+.3} {:+.3} (deg)",
                f.time_ms,
                dy.to_degrees(),
                dp.to_degrees(),
                yaw.to_degrees(),
                pitch.to_degrees()
            );
        }
        worst = worst.max(e);
        yaw_span = yaw_span.max(dy.abs());
        let k = cam.kick(&row, t, dc_client::spread::clock::TICK_MS, false);
        yaw += k.yaw as f64;
        pitch += k.pitch as f64;
    }
    println!(
        "recoil: {} shots, largest difference to the rule {worst:.4} deg, yaw reached {:.2} deg, replay wags {} flips {}",
        shots.len(),
        yaw_span.to_degrees(),
        cam.wags,
        cam.flips
    );
    assert!(
        worst < 0.02,
        "the sent view is {worst:.4} degrees off the recoil rule"
    );
    assert!(yaw_span.to_degrees() > 0.5, "the yaw kicked");
    assert!(cam.flips > 0, "the yaw changed side");

    // --- jar: the bot's hits jarred the camera and the pellets follow the camera
    assert!(st.damage_taken >= 5, "{} hits", st.damage_taken);
    assert!(
        st.camera_jars.len() >= 3,
        "jars {:?} of {} hits",
        st.camera_jars,
        st.damage_taken
    );
    let t = cone_ref::rebuilt_table();
    let (mut jarred, mut plain) = (0usize, 0usize);
    for (i, f) in shots.iter().enumerate() {
        let ms = f.time_ms as f64;
        let a = st
            .camera_jars
            .iter()
            .rev()
            .find(|j| j.0 as f64 <= ms)
            .and_then(|j| dc_client::camfx::wave_of(j.0 as f64, j.1))
            .map(|w| {
                let s = ((ms - w.start_ms) / 1000.0) as f32;
                (w.pitch.value(s), w.yaw.value(s), w.roll.value(s))
            })
            .unwrap_or((0.0, 0.0, 0.0));
        let (y, p) = yaw_pitch(f.aim);
        let q = dc_client::camfx::camera_quat(y as f32 + a.1, p as f32 + a.0, a.2);
        assert!(
            dc_cone::replay(&t, q, f.dir, f.stat).is_some(),
            "pellet {i} at {} ms is not the cone around the camera (jar {a:?})",
            f.time_ms
        );
        if a.0.abs().max(a.2.abs()) > 0.3f32.to_radians() {
            jarred += 1;
            // A roll turns the camera about its forward axis: it moves a pellet only by the roll times the pellet's
            // cone offset, which for a small offset stays inside the replay's tolerance, so only a pitch jar must
            // leave the cone around the reported aim (a roll-only jar was seen replaying around both: 0.33 deg roll).
            assert!(
                a.0.abs() <= 0.3f32.to_radians()
                    || dc_cone::replay(&t, f.aim, f.dir, f.stat).is_none(),
                "pellet {i}: a jarred shot still replays around the reported aim (jar pitch / yaw / roll {a:?})"
            );
        } else {
            plain += 1;
        }
    }
    println!("jar: {jarred} pellets left along a jarred camera, {plain} along the plain view");
    assert!(jarred >= 2, "{jarred} jarred pellets");
}
