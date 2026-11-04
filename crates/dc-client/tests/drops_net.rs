//! Weapon drops over loopback UDP against our own dedicated server: `dc-dserver` (the workspace binary, the ROM's
//! weapon rows) with the made-up world of `dc_client::testkit`, a dc-botclient standing still on the other team with
//! only an automatic rifle, and the client headless with only a pistol in slot 1. The client shoots the bot until it
//! dies; the bot's rifle drops at its body (a type 0x17 kind-6 pickup object, DSV-93); the client sees it, walks onto
//! it, the server gives it the rifle (0xb2 into the empty slot 0, DSV-94) and removes the object; the client's
//! inventory now holds the rifle in slot 0 with a full clip and it is drawn. The client never sends a pickup message
//! (the 2009 client has none). ROM-gated: without `CA_ROM` the server has no weapon rows and drops nothing.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.4 (0xb2), §8.2 (type 0x17); GAME_MESSAGES_2009_C2S.md §1.3
// DISASM: cshell+0x282490 / cshell+0x282670 (the pickup object), cshell+0x62f80 (the pickup notice of 0xb2)
// DC-DEFAULT: DSV-93, DSV-94 (the server's drop rules), CLI-151 (the pickup grant)

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
    run_headless, Connect, HeadlessOptions, InputSource, NetHandle, NetOptions, NetWorld,
    RemoteBody, ShootTest, ViewAngles, WorldFile,
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

/// Backs away from the nearest remote player to `keep` units and shoots it (trigger pressed and released, for a
/// one-shot-per-press pistol, reload when the clip is empty) until a drop lies in the world; then turns toward the
/// drop and walks onto it. (The made-up world has one spawn point, so both players start at the same place.)
struct DropHunter {
    net: NetHandle,
    frame: u64,
    eye: Option<[f32; 3]>,
    target: bool,
    drop_at: Option<[f32; 3]>,
    walking: bool,
    backing: bool,
    keep: f32,
    reload: bool,
}

impl DropHunter {
    fn horizontal(&self) -> Option<f32> {
        let (e, d) = (self.eye?, self.drop_at?);
        Some(((d[0] - e[0]).powi(2) + (d[2] - e[2]).powi(2)).sqrt())
    }
}

impl InputSource for DropHunter {
    fn frame(&mut self, _view: &mut ViewAngles) {
        self.frame += 1;
        let st = self.net.stats();
        self.drop_at = st.drops.live.first().map(|d| d.2);
        self.reload = st.weapon.is_some_and(|w| w.clip == 0 && w.stock > 0);
    }

    fn tick(&mut self, _tick: u64, _hz: u32, _view: &mut ViewAngles) -> Commands {
        Commands {
            forward: self.walking && self.horizontal().is_some_and(|d| d > 2.0),
            backward: !self.walking && self.backing,
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
        self.eye = Some(eye);
        if let Some(d) = self.drop_at {
            let v = ShootTest::look_at(eye, d);
            *view = ViewAngles {
                yaw: v.yaw,
                pitch: 0.0,
            };
            self.walking = true;
            self.target = false;
            return;
        }
        self.walking = false;
        let d2 = |p: [f32; 3]| {
            (p[0] - eye[0]).powi(2) + (p[1] - eye[1]).powi(2) + (p[2] - eye[2]).powi(2)
        };
        let best = remotes
            .iter()
            .filter(|r| r.pos.iter().all(|c| c.is_finite()))
            .min_by(|a, b| d2(a.pos).total_cmp(&d2(b.pos)));
        self.target = best.is_some();
        self.backing = best.is_some_and(|r| {
            ((r.pos[0] - eye[0]).powi(2) + (r.pos[2] - eye[2]).powi(2)).sqrt() < self.keep
        });
        if let Some(r) = best {
            *view = ShootTest::look_at(eye, [r.pos[0], r.pos[1] + 0.5 * r.half[1], r.pos[2]]);
        }
    }

    fn weapon(&mut self) -> WeaponCommands {
        WeaponCommands {
            fire: self.target && !self.backing && self.frame % 8 < 3,
            reload: self.reload && self.frame.is_multiple_of(30),
            ..WeaponCommands::default()
        }
    }

    fn label(&self) -> &'static str {
        "drop hunter"
    }
}

fn stander(port: u16, world: std::path::PathBuf, secs: u64) -> std::thread::JoinHandle<BotSession> {
    std::thread::spawn(move || {
        let mut cfg = GameConfig::new("StandBot");
        cfg.hello_tag = "standbot".into();
        cfg.motion = MotionKind::Stand;
        cfg.world_file = Some(world);
        cfg.load_ms = 300;
        let rec = Recorder::null("StandBot");
        let mut bot = BotSession::new(cfg, None, rec.clone());
        let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        run_game(&mut bot, server, Duration::from_secs(secs), &rec).expect("bot udp");
        bot
    })
}

#[test]
fn a_killed_players_weapon_drops_and_the_client_walks_onto_it_and_picks_it_up() {
    if dc_rom::test_rom().is_none() {
        println!("skipped: CA_ROM unset (the server drops only weapons with a ROM row)");
        return;
    }
    let dir = scratch("drops");
    let world = synth_world_file(&dir);
    let (mut srv, port) = start_dserver(
        &world,
        &[
            "-tick",
            "20",
            "-roundsecs",
            "90",
            "-holds",
            "1500,800",
            "-seed",
            "23",
        ],
    );
    // the client on team 1 with only a pistol (slot 1), the bot on team 2 with only the rifle (slot 0)
    srv.line("user 1 1 1 0 0 DropClient loadout=1:164");
    srv.line("user 2 2 2 1 0 StandBot loadout=0:145");
    let bot = stander(port, world.clone(), 50);

    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "DropClient",
    );
    o.world_file = Some(world_file());
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let opts = HeadlessOptions {
        frames: 60 * 40,
        frame_us: 16_667,
        width: 320,
        height: 240,
        out: None,
        save_every: 10,
        font_bytes: Some(synth_font()),
        realtime: true,
        ..HeadlessOptions::default()
    };
    let mut input = DropHunter {
        net: handle.clone(),
        frame: 0,
        eye: None,
        target: false,
        drop_at: None,
        walking: false,
        backing: false,
        keep: 160.0,
        reload: false,
    };
    let r = run_headless(&mut net, Some(&mut input as &mut dyn InputSource), &opts)
        .expect("headless run");
    let st = handle.shutdown(Duration::from_secs(5));
    let bot = bot.join().expect("bot thread");
    srv.quit();
    let code = srv.finish(Duration::from_secs(5));
    srv.drain();
    println!("{}", r.summary());
    println!(
        "arms {:?} | grants {:?} | held {:?}\n  drops {:?}\n  drops drawn {} | kills {:?}",
        st.arms, st.grants, st.weapon, st.drops, r.drops_drawn, st.kill_pairs
    );
    println!("server exit {code:?}");
    for l in srv
        .seen
        .iter()
        .filter(|l| l.contains("drops:") || l.contains("killed") || l.contains("spawned"))
    {
        println!("    {l}");
    }

    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    let me = st.client_id.expect("own client id") as u8;
    let bot_pid = bot.obs.client_id.map(|c| c.0 as u8).expect("the bot's id");
    assert!(
        st.kill_pairs.contains(&(bot_pid, me)),
        "the client killed the bot: {:?}",
        st.kill_pairs
    );
    // the server dropped the bot's rifle and the client saw the pickup object
    assert!(
        srv.has("drops: drop 1: weapon 145"),
        "no drop on the server"
    );
    assert!(
        st.drops.created >= 1,
        "the client saw no drop: {:?}",
        st.drops
    );
    let seen = st.drops.seen.first().copied().expect("a drop seen");
    assert_eq!(seen.1, 145, "the drop is the bot's rifle");
    assert!(seen.2 > 0, "with rounds");
    // the client walked onto it and the server gave it the rifle into the empty slot 0
    assert!(
        srv.has("(weapon 145) taken by client"),
        "the server saw no pickup"
    );
    assert!(
        st.grants.iter().any(|g| g.0 == 0 && g.1 == 145),
        "no 0xb2 of the rifle into slot 0: {:?}",
        st.grants
    );
    let p = st
        .drops
        .pickups
        .iter()
        .find(|p| p.weapon == 145)
        .copied()
        .unwrap_or_else(|| panic!("no pickup noted: {:?}", st.drops));
    assert_eq!(p.slot, 0);
    assert!(!p.same_weapon, "the slot was empty");
    assert!(p.clip > 0, "a new weapon starts with a full clip: {p:?}");
    assert_eq!(
        p.drop_obj,
        Some(seen.0),
        "the removal paired with the grant"
    );
    assert!(
        st.drops.removed >= st.drops.pickups.len() as u64,
        "every taken drop's object is gone: {:?}",
        st.drops
    );
    // the inventory changed: the rifle is drawn (the pickup notice's weapon change)
    let held = st.weapon.expect("a held weapon");
    assert_eq!(
        (held.slot, held.weapon),
        (0, 145),
        "the rifle is drawn: {held:?}"
    );
    // the client walked: away from the bot, then back onto each drop
    assert!(
        r.path_xz > 160.0,
        "the client did not walk: path {}",
        r.path_xz
    );
    // the drop was drawn with its model while it lay there (the client has the ROM)
    if r.adapter.is_some() && r.models_note.is_none() {
        assert!(r.drops_drawn > 0, "the drop was never drawn");
    }
}
