//! The client's melee weapon and grenade against our own dedicated server over loopback UDP: `dc-dserver` with the made-up
//! world and a four-weapon loadout (rifle, pistol, the sickle 83 the user carried, the grenade 7), a standing bot. The
//! client walks up to the bot and swings (the melee row's first-person clips set the swing timing: two swings per
//! clip, CLI-199 when the row's first model is missing), then backs off and throws the grenade (wind-up, hold passes,
//! release on button up). Checked: the swings' 0x82 come at the clip's key spacing (not the 105 ms gun stand-in), the
//! server accepts the knife hits, it creates the thrown object and blows it up, and the client receives the blast
//! record; with a ROM the client draws the thrown object and plays the explosion (effect group and sound cue).
// SPEC: GAME_MESSAGES_2009_C2S.md §4.1 (0x82, the melee swing form), §4.6 (0x4d hold, the release 0x82);
// GAME_MESSAGES_2009_S2C.md §8.2 (the blast record); DSERVER_PIPE.md §2-§4

mod cone_ref;
mod support;

use std::net::SocketAddr;
use std::time::Duration;

use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::run_game;
use dc_botclient::session::{BotSession, GameConfig};
use dc_client::arms::WeaponCommands;
use dc_client::testkit::{synth_font, synth_source, SYNTH_DIMS};
use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetOptions, NetWorld, RemoteBody,
    ServerHold, ShootTest, ViewAngles, WorldFile,
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

/// Walks up to the nearest remote player, knifes it, then steps back and throws the grenade.
struct MeleeThrow {
    t: f64,
    frame_s: f64,
    alive_at: Option<f64>,
    nearest: Option<f32>,
    stage: u32,
    stage_at: f64,
    select: Option<u8>,
    fire: bool,
    log: Vec<String>,
}

impl MeleeThrow {
    fn go(&mut self, stage: u32) {
        self.stage = stage;
        self.stage_at = self.t;
        self.log.push(format!(
            "{:.2}s stage {stage} dist {:?}",
            self.t, self.nearest
        ));
    }
}

impl InputSource for MeleeThrow {
    fn frame(&mut self, _view: &mut ViewAngles) {
        self.t += self.frame_s;
    }
    fn server(&mut self, v: &ServerHold) {
        if v.life_state == Some(1) && self.alive_at.is_none() {
            self.alive_at = Some(self.t);
        }
    }
    fn tick(&mut self, _tick: u64, _hz: u32, _view: &mut ViewAngles) -> Commands {
        let close = self.nearest.is_some_and(|d| d < 45.0);
        let far = self.nearest.is_some_and(|d| d > 300.0);
        Commands {
            forward: self.stage == 1 && !close,
            backward: self.stage == 4 && !far,
            ..Commands::default()
        }
    }
    fn aim(
        &mut self,
        eye: [f32; 3],
        remotes: &[RemoteBody],
        _own_team: Option<u8>,
        view: &mut ViewAngles,
    ) {
        let near = remotes.iter().min_by(|a, b| {
            let d = |r: &RemoteBody| (r.pos[0] - eye[0]).powi(2) + (r.pos[2] - eye[2]).powi(2);
            d(a).total_cmp(&d(b))
        });
        self.nearest =
            near.map(|r| ((r.pos[0] - eye[0]).powi(2) + (r.pos[2] - eye[2]).powi(2)).sqrt());
        if let Some(r) = near {
            if self.stage < 5 {
                *view = ShootTest::look_at(eye, r.pos);
            }
        }
        let since = self.t - self.stage_at;
        self.select = None;
        match self.stage {
            0 if self.alive_at.is_some_and(|a| self.t - a > 1.5) && self.nearest.is_some() => {
                self.go(1)
            }
            1 if self.nearest.is_some_and(|d| d < 50.0) => {
                self.select = Some(2);
                self.go(2)
            }
            2 if since > 1.0 => self.go(3),
            3 => {
                self.fire = since < 1.4;
                if since > 2.0 {
                    self.go(4);
                }
            }
            4 if self.nearest.is_some_and(|d| d > 280.0) || since > 4.0 => {
                self.select = Some(3);
                self.go(5)
            }
            5 if since > 1.0 => {
                view.pitch = -0.3;
                self.go(6)
            }
            6 => {
                self.fire = since < 1.2;
                if since > 6.0 {
                    self.go(7);
                }
            }
            _ => {}
        }
    }
    fn weapon(&mut self) -> WeaponCommands {
        WeaponCommands {
            fire: self.fire,
            select: self.select.take(),
            ..WeaponCommands::default()
        }
    }
    fn label(&self) -> &'static str {
        "melee and throw"
    }
}

fn stander(port: u16, world: std::path::PathBuf, secs: u64) -> std::thread::JoinHandle<BotSession> {
    std::thread::spawn(move || {
        let mut cfg = GameConfig::new("StandBot");
        cfg.hello_tag = "standbot".into();
        cfg.motion = MotionKind::Stand;
        cfg.world_file = Some(world);
        cfg.load_ms = 300;
        let mut bot = BotSession::new(cfg, None, Recorder::null("StandBot"));
        let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        run_game(
            &mut bot,
            server,
            Duration::from_secs(secs),
            &Recorder::null("StandBot"),
        )
        .expect("bot udp");
        bot
    })
}

#[test]
fn knife_and_grenade_against_dserver() {
    let dir = scratch("melee_throw");
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
            "11",
            "-loadout",
            "145,164,83,7",
            "-verbose",
        ],
    );
    srv.line("user 1 1 1 0 0 KnifeClient");
    srv.line("user 2 2 2 1 0 StandBot");
    let bot = stander(port, world.clone(), 30);
    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "KnifeClient",
    );
    o.world_file = Some(world_file());
    o.log = Some(dir.join("client_events.jsonl"));
    o.capture = Some(dir.join("client_capture.jsonl"));
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let opts = HeadlessOptions {
        frames: 60 * 24,
        frame_us: 16_667,
        width: 320,
        height: 240,
        out: None,
        save_every: 15,
        font_bytes: Some(synth_font()),
        realtime: true,
        ..HeadlessOptions::default()
    };
    let mut input = MeleeThrow {
        t: 0.0,
        frame_s: 1.0 / 60.0,
        alive_at: None,
        nearest: None,
        stage: 0,
        stage_at: 0.0,
        select: None,
        fire: false,
        log: Vec::new(),
    };
    let r = run_headless(&mut net, Some(&mut input as &mut dyn InputSource), &opts)
        .expect("headless run");
    let st = handle.shutdown(Duration::from_secs(5));
    let _bot = bot.join().expect("bot thread");
    srv.quit();
    let _ = srv.finish(Duration::from_secs(5));
    srv.drain();
    println!("{}", r.summary());
    for l in &input.log {
        println!("  input {l}");
    }
    println!(
        "grants {:?}
held {:?}
arms {:?}
kills {:?}
fx {:?} quads {} thrown frames {} (max {})",
        st.grants,
        st.weapon,
        st.arms,
        st.kill_pairs,
        r.fx,
        r.fx_quads,
        r.thrown_frames,
        r.thrown_max
    );
    for l in srv.seen.iter().filter(|l| !l.contains("ping")) {
        println!("    {l}");
    }
    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    assert!(
        st.grants.iter().any(|g| g.1 == 83),
        "the melee grant {:?}",
        st.grants
    );
    assert!(
        st.grants.iter().any(|g| g.1 == 7),
        "the grenade grant {:?}",
        st.grants
    );
    // the swings: the client's 0x82 of weapon 83, read back from its capture
    let d = dc_botclient::capdecode::decode_file(&dir.join("client_capture.jsonl"))
        .expect("client capture");
    let fires = cone_ref::fires(&d);
    let all: Vec<_> = fires.values().flatten().map(|x| x.1).collect();
    let swings: Vec<_> = all.iter().filter(|f| f.weapon_id == 83).collect();
    let gaps: Vec<i64> = swings
        .windows(2)
        .map(|w| w[1].time_ms as i64 - w[0].time_ms as i64)
        .collect();
    println!(
        "swings {} gaps {:?} hits {}",
        swings.len(),
        gaps,
        swings.iter().filter(|f| f.hit.is_some()).count()
    );
    assert!(swings.len() >= 2, "the client swung the melee weapon");
    // the melee swing form: weapon_pos = origin (both row vectors zero), last 1
    // OBSERVED: GAME_MESSAGES_2009_C2S.md §4.1.1 (tdm6 melee swing)
    let rom = dc_rom::test_rom().is_some();
    if rom {
        assert!(
            swings.iter().all(|f| f.weapon_pos == f.origin && f.last),
            "melee form"
        );
        // two swings per fire clip, 247 ms apart (the clip's two fire keys), never the 105 ms gun stand-in
        assert!(gaps.iter().all(|g| *g >= 200), "swing spacing {gaps:?}");
    }
    let hits = swings.iter().filter(|f| f.hit.is_some()).count();
    assert!(hits >= 1, "a swing hit the bot");
    let line = srv
        .seen
        .iter()
        .find(|l| l.contains("session hits:"))
        .cloned()
        .unwrap_or_default();
    // A swing claimed after the bot's lethal hit and before its kill notice reached the client is refused as
    // `victim_dead`: a race of the swing clip against the loopback (seen on a slow macOS runner), not a bad claim.
    // Every other refusal still fails the test.
    let refused_other = line
        .split("refused ")
        .nth(1)
        .map(|r| r.trim() != "none" && r.split(", ").any(|v| !v.starts_with("victim_dead ")))
        .unwrap_or(true);
    assert!(
        !refused_other,
        "the server accepted the hits (victim_dead alone is the kill notice in flight): {line}"
    );
    if r.adapter.is_none() {
        println!("no GPU adapter: the effect checks are skipped");
        return;
    }
    assert!(
        r.fx.records >= 1,
        "the client received shot records ({:?})",
        r.fx
    );
    // the throw: the server created the thrown object from the release and blew it up (without a ROM the client has
    // no weapon table and cannot tell that the grenade is a throwable)
    if rom {
        assert!(st.arms.holds >= 1 && st.arms.throws >= 1, "{:?}", st.arms);
        assert!(srv.has("throw 1 (weapon 7"), "the server's throw line");
        assert!(srv.has("throw 1 blew up"), "the server's blast line");
        assert!(
            r.fx.blasts >= 1,
            "the blast record reached the effects: {:?}",
            r.fx
        );
        assert!(r.fx.sounds >= 1, "the explosion sound cue: {:?}", r.fx);
        assert!(r.thrown_frames > 0, "the thrown object was drawn");
    }
}
