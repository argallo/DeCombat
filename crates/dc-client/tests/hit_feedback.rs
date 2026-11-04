//! The shooter's side of a hit, over loopback UDP against our own dedicated server: `dc-dserver` (the workspace
//! binary) with the made-up world of `dc_client::testkit`, a dc-botclient standing still on the other team, and the
//! client headless with a scripted shooter. Checked: the bot's health drops with the client's accepted hits and it
//! dies; the server's shot record of each own character hit (`c` = the own client id, `b` keeping 0x4000) reaches
//! the client and plays the shooter's hit feedback once per hit, the continuation records and misses play none; and
//! after the kill the corpse is out of the client's shot trace until the bot comes back (CLI-99).
// SPEC: GAME_MESSAGES_2009_C2S.md §4.1; GAME_MESSAGES_2009_S2C.md §3.3.1, §4.3, §8.2 (type 0x06)
// DISASM: cshell+0x2c8b4a..0x2c8be6 (own-shot hit feedback), cshell+0x201c00 (the feedback sound)
// DC-DEFAULT: CLI-98, CLI-99 (proposed)

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::run_game;
use dc_botclient::session::{BotSession, GameConfig};
use dc_client::arms::WeaponCommands;
use dc_client::sound::Trigger;
use dc_client::testkit::{synth_font, synth_source, SYNTH_DIMS};
use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetOptions, NetWorld, RemoteBody,
    ShootTest, ViewAngles, WorldFile,
};
use dc_sim::Commands;
use serde_json::Value;
use support::*;

fn world_file() -> WorldFile {
    let s = synth_source();
    WorldFile {
        bytes: s.bytes,
        stand_dims: SYNTH_DIMS,
        textures: s.textures,
    }
}

/// The shooter, plus: while the nearest remote player is closer than `keep`, walk backward (the view faces it).
struct BackOffShooter {
    shoot: ShootTest,
    keep: f32,
    nearest: Option<f32>,
}

impl InputSource for BackOffShooter {
    fn frame(&mut self, view: &mut ViewAngles) {
        self.shoot.frame(view);
    }

    fn tick(&mut self, _tick: u64, _hz: u32, _view: &mut ViewAngles) -> Commands {
        Commands {
            backward: self.nearest.is_some_and(|d| d < self.keep),
            run: true,
            ..Commands::default()
        }
    }

    fn aim(
        &mut self,
        eye: [f32; 3],
        remotes: &[RemoteBody],
        own_team: Option<u8>,
        view: &mut ViewAngles,
    ) {
        self.nearest = remotes
            .iter()
            .map(|r| ((r.pos[0] - eye[0]).powi(2) + (r.pos[2] - eye[2]).powi(2)).sqrt())
            .min_by(|a, b| a.total_cmp(b));
        self.shoot.aim(eye, remotes, own_team, view);
    }

    fn weapon(&mut self) -> WeaponCommands {
        let mut c = self.shoot.weapon();
        c.fire &= self.nearest.is_some_and(|d| d >= 0.6 * self.keep);
        c
    }

    fn label(&self) -> &'static str {
        "back-off shooter"
    }
}

fn stander(
    port: u16,
    world: std::path::PathBuf,
    secs: u64,
    rec: Recorder,
) -> std::thread::JoinHandle<BotSession> {
    std::thread::spawn(move || {
        let mut cfg = GameConfig::new("StandBot");
        cfg.hello_tag = "standbot".into();
        cfg.motion = MotionKind::Stand;
        cfg.world_file = Some(world);
        cfg.load_ms = 300;
        let mut bot = BotSession::new(cfg, None, rec.clone());
        let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        run_game(&mut bot, server, Duration::from_secs(secs), &rec).expect("bot udp");
        bot
    })
}

#[test]
fn the_shooter_hears_its_hits_and_the_bot_loses_health_and_dies() {
    let dir = scratch("hitfeedback");
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
        ],
    );
    // the client on team 1, the bot on team 2 (an enemy: no friendly-fire refusal)
    srv.line("user 1 1 1 0 0 GunClient");
    srv.line("user 2 2 2 1 0 StandBot");
    let rec = Recorder::null("StandBot").keep_in_memory();
    let bot = stander(port, world.clone(), 26, rec.clone());

    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "GunClient",
    );
    o.world_file = Some(world_file());
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let opts = HeadlessOptions {
        frames: 60 * 22,
        frame_us: 16_667,
        width: 320,
        height: 240,
        out: None,
        save_every: 15,
        font_bytes: Some(synth_font()),
        realtime: true,
        ..HeadlessOptions::default()
    };
    let mut input = BackOffShooter {
        shoot: ShootTest::new(1.0 / 60.0, None),
        keep: 400.0,
        nearest: None,
    };
    let r = run_headless(&mut net, Some(&mut input as &mut dyn InputSource), &opts)
        .expect("headless run");
    let st = handle.shutdown(Duration::from_secs(5));
    let bot = bot.join().expect("bot thread");
    srv.quit();
    let code = srv.finish(Duration::from_secs(5));
    srv.drain();

    // the bot's side: its health statuses (0x66 kind 2) and damage messages (0x74)
    let events = rec.memory();
    let ev = |name: &str| -> Vec<&Value> {
        events
            .iter()
            .filter(|e| e.get("ev").and_then(Value::as_str) == Some(name))
            .collect()
    };
    let health: Vec<f64> = ev("status")
        .iter()
        .filter(|e| e.get("kind").and_then(Value::as_u64) == Some(2))
        .filter_map(|e| e.get("value").and_then(Value::as_f64))
        .collect();
    let damage = ev("damage").len() as u64;
    let feedback: Vec<_> = r
        .sound_log
        .iter()
        .filter(|e| matches!(e.trigger, Trigger::OwnHit { .. }))
        .collect();
    println!("{}", r.summary());
    println!(
        "arms {:?} | own hit records {} feedback sounds {} (stats {}) | corpses {} revived {} corpse skips {}",
        st.arms,
        st.own_hit_records,
        feedback.len(),
        r.sound_stats.own_hits,
        st.corpses,
        st.revived,
        st.corpse_skips
    );
    println!(
        "bot: damage {damage} health {health:?} kills {:?} life {:?}",
        bot.obs.kills, bot.obs.life_states
    );
    println!("server exit {code:?}");
    for l in srv
        .seen
        .iter()
        .filter(|l| l.contains("hits") || l.contains("killed"))
    {
        println!("    {l}");
    }

    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    let me = st.client_id.expect("own client id") as u8;
    let bot_pid = bot.obs.client_id.map(|c| c.0 as u8).expect("the bot's id");
    // the bot lost health to the client's hits and died by its hand
    assert!(st.arms.hits >= 4, "hits on the bot's box: {:?}", st.arms);
    assert!(damage >= 1, "the bot took no damage message");
    assert!(
        health.iter().any(|h| *h > 0.0 && *h < 100.0),
        "health between 0 and 100: {health:?}"
    );
    assert!(health.contains(&0.0), "health reached 0: {health:?}");
    assert!(
        bot.obs.kills.contains(&(bot_pid, me)),
        "the bot's death by the client: {:?}",
        bot.obs.kills
    );
    assert!(
        st.kill_pairs.contains(&(bot_pid, me)),
        "{:?}",
        st.kill_pairs
    );
    // the shooter's feedback: one own hit record per character hit the server answered (at least the accepted ones),
    // never more than the client's hits (continuation records and misses give none); the feedback sound plays once
    // per client hit, at the shot (CLI-1603), not per record
    assert!(
        st.own_hit_records >= damage && st.own_hit_records <= u64::from(st.arms.hits),
        "own hit records {} vs damage {damage} / client hits {}",
        st.own_hit_records,
        st.arms.hits
    );
    assert_eq!(
        st.predicted_hits,
        u64::from(st.arms.hits),
        "one predicted feedback per client hit"
    );
    assert_eq!(
        r.sound_stats.own_hits, st.predicted_hits,
        "one feedback per own hit, at the shot"
    );
    assert_eq!(feedback.len() as u64, r.sound_stats.own_hits);
    assert!(feedback
        .iter()
        .all(|e| e.pos.is_none() && e.prio == dc_client::sound::HIT_FEEDBACK_PRIO));
    // the kill's own voice: every own kill (not in the groin zone) plays auto-cast local row 1 as a local voice at
    // priority 6, besides the hit feedback above and the banner's interface line (CLI-601)
    // EVIDENCE: cshell+0x2c430 (case 0x6d: the own kill voice)
    let own_kills = st.kill_pairs.iter().filter(|(_, k)| *k == me).count() as u64;
    let kill_voices: Vec<_> = r
        .sound_log
        .iter()
        .filter(|e| matches!(e.trigger, Trigger::OwnKill))
        .collect();
    assert!(own_kills >= 1);
    assert!(
        !kill_voices.is_empty() && kill_voices.len() as u64 <= own_kills,
        "own kill voices {} for {own_kills} own kills",
        kill_voices.len()
    );
    assert!(kill_voices.iter().all(|e| e.pos.is_none()
        && e.prio == dc_client::sound::cues::OWN_KILL_PRIO
        && e.volume == 100));
    if std::env::var_os("CA_ROM").is_some() {
        assert!(
            kill_voices.iter().all(|e| e.file.is_some()),
            "the ROM's row 1 names a file"
        );
    }
    // the corpse: the kill marked the bot's character dead
    assert!(
        st.corpses + st.revived as usize >= 1,
        "the kill marked no corpse"
    );
    // with the ROM the server's corpse plays its death clip (DSV-44): the shooter kept firing at it, and none of those
    // shots claimed a hit on it (no victim_dead refusal)
    if std::env::var_os("CA_ROM").is_some() {
        assert!(st.corpse_skips > 0, "no shot while a corpse lay there");
        assert!(
            !srv.seen
                .iter()
                .any(|l| l.contains("session hits") && l.contains("victim_dead")),
            "a shot at the corpse still claimed a hit"
        );
    }
}
