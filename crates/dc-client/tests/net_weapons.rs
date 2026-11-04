//! The client fights a match of our own dedicated server over loopback UDP: `dc-dserver` (the workspace binary) on
//! an ephemeral port with the made-up world of `dc_client::testkit` and a two-weapon loadout, a standing
//! dc-botclient as the enemy, and the client headless with a scripted shooter (it backs off from the bot, turns
//! toward it and holds the fire button in bursts). Checked: the server's grants reach the inventory, the client
//! selects its weapon with the 2009 select pair, fires 0x82 reports that hit the bot's box, the server applies them
//! and kills the bot, and the client receives the kill message 0x6d naming itself as the killer.
// SPEC: DSERVER_PIPE.md §2-§4; GAME_MESSAGES_2009_C2S.md §4.1, §4.4, §4.5; GAME_MESSAGES_2009_S2C.md §3.3.1, §4.4

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
    ShootTest, ViewAngles, WorldFile,
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
        // fire only once some distance is kept (the hit test of a shot from inside the box is degenerate)
        let mut c = self.shoot.weapon();
        c.fire &= self.nearest.is_some_and(|d| d >= 0.6 * self.keep);
        c
    }

    fn label(&self) -> &'static str {
        "back-off shooter"
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
fn client_shoots_and_kills_a_standing_bot() {
    let dir = scratch("weapons");
    let world = synth_world_file(&dir);
    // a long round (the kill comes first), 20 Hz stream; the made-up loadout: weapon table indices 145 and 164 (no
    // ROM: the server's default clip, stock and damage)
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
            "145,164",
            "-combat",
            "cone_check=log",
        ],
    );
    srv.line("user 1 1 1 0 0 GunClient");
    srv.line("user 2 2 2 1 0 StandBot");
    let bot = stander(port, world.clone(), 26);

    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "GunClient",
    );
    o.world_file = Some(world_file());
    o.log = Some(dir.join("client_events.jsonl"));
    o.capture = Some(dir.join("client_capture.jsonl"));
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
    println!("{}", r.summary());
    for l in &r.last_lines {
        println!("  | {l}");
    }
    println!(
        "net: phase {:?} end {:?} updates {} life {:?}\n  grants {:?}\n  held {:?}\n  arms {:?}\n  kills {:?} damage taken {}",
        st.phase, st.end, st.updates_sent, st.life_states, st.grants, st.weapon, st.arms, st.kill_pairs,
        st.damage_taken
    );
    for (obj, t) in &st.tracks {
        println!(
            "  remote {} pid {} object {obj}: {} samples last {:?}",
            t.name, t.pid, t.sample_count, t.last
        );
    }
    println!(
        "  bot: stage {:?} life {:?} kills {:?} end {:?}",
        bot.stage(),
        bot.obs.life_states,
        bot.obs.kills,
        bot.obs.end
    );
    println!("  server exit {code:?}; lines:");
    for l in srv.seen.iter().filter(|l| !l.contains("ping")) {
        println!("    {l}");
    }
    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    assert_eq!(st.bad_check, 0);
    // the server's weapons reached the inventory and the client drew one with the 2009 select pair
    assert!(
        st.grants.iter().any(|g| g.1 == 145),
        "grants {:?}",
        st.grants
    );
    assert!(st.arms.selects >= 1, "{:?}", st.arms);
    let held = st.weapon.expect("a held weapon");
    assert!([145, 164].contains(&held.weapon), "{held:?}");
    // the shots
    assert!(r.fire_frames > 0, "the input held the fire button");
    assert!(
        st.arms.shots >= 4 && st.arms.fire_msgs >= st.arms.shots,
        "{:?}",
        st.arms
    );
    assert!(st.arms.hits >= 4, "hits on the bot's box: {:?}", st.arms);
    // the server applied them and killed the bot; the client got the kill message naming itself
    let me = st.client_id.expect("own client id") as u8;
    let bot_pid = st
        .tracks
        .values()
        .find(|t| t.name == "StandBot")
        .map(|t| t.pid)
        .or_else(|| bot.obs.client_id.map(|c| c.0 as u8))
        .expect("the bot's id");
    assert!(
        st.kill_pairs.contains(&(bot_pid, me)),
        "kill 0x6d (victim {bot_pid}, killer {me}) not received: {:?}",
        st.kill_pairs
    );
    assert!(
        srv.has(&format!("client {bot_pid} killed by Some({me})")),
        "the server's kill line"
    );
    assert!(
        bot.obs.kills.contains(&(bot_pid, me)),
        "the bot saw its own death: {:?}",
        bot.obs.kills
    );
    assert_eq!(r.nan_ticks + r.nan_frames + r.nan_remotes, 0);
    // the client's own 0x82s, read back from its capture, replay through the reference cone code pellet for pellet;
    // `stat` is the counter the server's 0x7d set (own_value) plus two reads per pellet
    // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-5 (replay), AC-7 (dc-client)
    let d = dc_botclient::capdecode::decode_file(&dir.join("client_capture.jsonl"))
        .expect("client capture");
    let table = cone_ref::rebuilt_table();
    let fires = cone_ref::fires(&d);
    let shots: Vec<_> = fires.values().flatten().collect();
    assert!(
        shots.len() as u32 >= st.arms.shots,
        "{} 0x82 in the capture",
        shots.len()
    );
    let conn = *fires.keys().next().expect("the client's link");
    let mut seed = None;
    for (shell, msg) in cone_ref::shells_before(&d, conn, shots[0].0) {
        if shell == dc_client::arms::down::MATCH_SETTINGS {
            let mut r = msg.reader();
            r.read_u8();
            r.read_u8();
            seed = dc_client::arms::match_settings_shot_seed(&mut r);
        }
    }
    let seed = seed.expect("0x7d before the first shot");
    let rom_rows = dc_rom::test_rom().is_some();
    let mut tilted = 0;
    for (i, (_, f)) in shots.iter().enumerate() {
        let h = cone_ref::replay(&table, f)
            .unwrap_or_else(|| panic!("pellet {i} does not replay through the reference: {f:?}"));
        tilted += usize::from(h > 0);
        if rom_rows {
            // the ROM rows of 145 / 164 have a cone: two reads per pellet from the server's own_value
            assert_eq!(f.stat, seed + 2 * (i as u32 + 1), "pellet {i}: stat");
        } else {
            // no ROM rows: no bounds, the gate refuses, the counter stays where the server put it
            assert_eq!((h, f.stat), (0, seed), "pellet {i}");
        }
    }
    println!(
        "loopback cone: {} of {} client pellets replayed through the reference ({tilted} tilted, own_value {seed}, ROM rows {rom_rows})",
        shots.len(),
        shots.len()
    );
    if rom_rows {
        assert!(tilted > 0, "the ROM weapons' shots are tilted");
    }
    // the server's own shot check (DSV-102) on the same pellets: every one `cone_ok` (the stand bot never fires, so
    // the session line counts the client's link only)
    // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-6
    let line = srv
        .seen
        .iter()
        .find(|l| l.contains("session cone:"))
        .cloned()
        .unwrap_or_else(|| panic!("no session cone line: {:?}", srv.seen));
    println!("{line}");
    let count = |key: &str| -> u64 {
        let i = line.find(key).unwrap_or_else(|| panic!("{key} in {line}"));
        line[..i]
            .split_whitespace()
            .last()
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("{key} in {line}"))
    };
    let (pellets, ok, off, na) = (
        count(" pellets"),
        count(" ok,"),
        count(" off ("),
        count(" na,"),
    );
    assert_eq!(pellets, shots.len() as u64, "{line}");
    assert_eq!((ok, off, na), (pellets, 0, 0), "{line}");
    if rom_rows {
        assert!(
            line.contains("0 without row bounds; stat skew 0;"),
            "{line}"
        );
    }
}
