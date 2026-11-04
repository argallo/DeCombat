//! The grenade's last throw against our own dedicated server over loopback UDP (ROM-gated): `dc-dserver` with the
//! made-up world, a standing bot, and a player whose loadout is the rifle 145, the pistol 164, the knife 0, the
//! two-grenade pack 143 and the backpack rifle 27 (weapon slot 4). The client draws the backpack rifle, then the knife,
//! then the pack, and throws twice. Checked against the 2009 rule (cshell+0x233d90 round use, cshell+0x237614 empty
//! test, cshell+0x22ec00 empty switch, cshell+0x22e704 / 0x22e790 the draw test; the real client in rust6):
//! - throw 1 of 2: the release is followed in the same moment by the reload `0x83 [143]`, no weapon change, the
//!   first-person view keeps the grenade, the server answers 1.0;
//! - throw 2 (the last): the release is followed in the same moment by `76 03 02` + `7f [4][27][27]`: the primary drawn
//!   last (slot 4), not slot 0 and not the knife drawn just before the pack; the server answers 0.0;
//! - the pack's key afterwards draws nothing: no `7f [3]`, the view never shows the grenade again;
//! - the last rendered frame (the hands after the last throw) is written as a PNG.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.2 (0x83), §4.4 (0x7f), §4.6 (0x4d hold, the release 0x82); GAME_MESSAGES_2009_S2C.md
// §4.3 (0x66 kind 1); DSERVER_PIPE.md §2-§4
// OBSERVED: capture 20261025_125432_rust6/analysis (the 2009 client: 2-pack releases with 0x83,
// last releases with the change to the class-1 slot drawn last, 280.437 s to the backpack rifle 27 in slot 4)

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use dc_botclient::capdecode::unbundle;
use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::run_game;
use dc_botclient::session::{BotSession, GameConfig};
use dc_botclient::weapon::read_fire;
use dc_client::arms::{WeaponCommands, WeaponTable};
use dc_client::testkit::{synth_font, synth_source, SYNTH_DIMS};
use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetOptions, NetWorld, RemoteBody,
    ServerHold, ShootTest, ViewAngles, WorldFile,
};
use dc_sim::Commands;
use support::*;

const PACK: u16 = 143;
const BACKPACK_RIFLE: u16 = 27;

fn world_file() -> WorldFile {
    let s = synth_source();
    WorldFile {
        bytes: s.bytes,
        stand_dims: SYNTH_DIMS,
        textures: s.textures,
    }
}

/// Draws slot 4, the knife, the pack; two throws; then the pack's key once more. The throws follow the client's own
/// state (its live counters), not fixed times, so that a loaded machine's late frames cannot put a press inside the
/// reload.
struct TwoThrows {
    net: dc_client::NetHandle,
    t: f64,
    frame_s: f64,
    alive_at: Option<f64>,
    stage: u32,
    stage_at: f64,
    select: Option<u8>,
    fire: bool,
    log: Vec<String>,
}

impl TwoThrows {
    fn go(&mut self, stage: u32) {
        self.stage = stage;
        self.stage_at = self.t;
        self.log.push(format!("{:.2}s stage {stage}", self.t));
    }
}

impl InputSource for TwoThrows {
    fn frame(&mut self, _view: &mut ViewAngles) {
        self.t += self.frame_s;
    }
    fn server(&mut self, v: &ServerHold) {
        if v.life_state == Some(1) && self.alive_at.is_none() {
            self.alive_at = Some(self.t);
        }
    }
    fn tick(&mut self, _tick: u64, _hz: u32, _view: &mut ViewAngles) -> Commands {
        Commands::default()
    }
    fn aim(
        &mut self,
        eye: [f32; 3],
        remotes: &[RemoteBody],
        _own_team: Option<u8>,
        view: &mut ViewAngles,
    ) {
        // throw away from the bot, upwards: the blasts land far from both players
        if let Some(r) = remotes.first() {
            if self.stage < 3 {
                let away = [2.0 * eye[0] - r.pos[0], eye[1], 2.0 * eye[2] - r.pos[2]];
                *view = ShootTest::look_at(eye, away);
                view.pitch = -0.4;
            }
        }
        let since = self.t - self.stage_at;
        self.select = None;
        self.fire = false;
        match self.stage {
            0 if self.alive_at.is_some_and(|a| self.t - a > 1.5) => {
                self.select = Some(4);
                self.go(1)
            }
            1 if since > 1.0 => {
                self.select = Some(2);
                self.go(2)
            }
            2 if since > 1.0 => {
                self.select = Some(3);
                self.go(3)
            }
            3 if since > 1.5 => self.go(4),
            // a short press (repeated every 3 s until the release went out)
            4 => {
                let st = self.net.stats();
                self.fire = st.arms.throws == 0 && since % 3.0 < 0.4;
                if st.arms.throws >= 1 {
                    self.go(5);
                }
            }
            // the reload of the pack's second grenade, then a short press
            5 => {
                let st = self.net.stats();
                let ready = st.weapon.is_some_and(|c| c.weapon == PACK && c.clip == 1);
                if ready && since > 1.0 {
                    self.go(6);
                }
            }
            6 => {
                let st = self.net.stats();
                self.fire = st.arms.throws < 2 && since % 3.0 < 0.4;
                if st.arms.throws >= 2 && since > 1.5 {
                    self.select = Some(3);
                    self.go(7);
                }
            }
            7 if since > 1.0 => self.go(8),
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
        "two throws"
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

/// The UP_FIRE report of a client shell message (`None` for any other message).
fn fire_of(b: &[u8]) -> Option<dc_botclient::weapon::FireReport> {
    if b.get(1) != Some(&0x82) {
        return None;
    }
    let w = dc_wire::udp::BitWriter::from_bytes(b);
    let mut r = w.reader();
    r.read_u8();
    r.read_u8();
    read_fire(&mut r)
}

/// The weapon the view held at session clock `ms` (the last mark at or before it).
fn view_at(marks: &[(u32, Option<u32>, Option<u16>)], ms: u32) -> Option<u16> {
    marks
        .iter()
        .rev()
        .find(|m| m.1.is_some_and(|c| c <= ms))
        .and_then(|m| m.2)
}

#[test]
fn the_last_grenade_switches_to_the_primary_drawn_last_and_no_grenade_is_shown() {
    let Some(rom) = dc_rom::test_rom() else {
        println!(
            "skipped: CA_ROM unset (the throwable and the order classes come from the ROM table)"
        );
        return;
    };
    let table = WeaponTable::open(&rom).expect("weapon table");
    let dir = scratch("grenade_switch");
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
            "11",
            "-verbose",
        ],
    );
    // equipment slots 0..3 and backpack slot 10 (= weapon slot 4)
    srv.line("user 1 1 1 0 0 NadeClient loadout=0:145,1:164,2:0,3:143,10:27");
    srv.line("user 2 2 2 1 0 StandBot");
    let bot = stander(port, world.clone(), 44);
    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "NadeClient",
    );
    o.world_file = Some(world_file());
    o.log = Some(dir.join("client_events.jsonl"));
    o.capture = Some(dir.join("client_capture.jsonl"));
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let opts = HeadlessOptions {
        frames: 60 * 36,
        frame_us: 16_667,
        width: 320,
        height: 240,
        out: None,
        save_every: 6,
        font_bytes: Some(synth_font()),
        realtime: true,
        debug_panel: false,
        ..HeadlessOptions::default()
    };
    let mut input = TwoThrows {
        net: net.handle(),
        t: 0.0,
        frame_s: 1.0 / 60.0,
        alive_at: None,
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
        "grants {:?}\nheld {:?}\narms {:?}\nview weapons {:?}\nview models {:?}",
        st.grants, st.weapon, st.arms, r.view_weapon_marks, r.view_model_marks
    );
    for l in srv.seen.iter().filter(|l| !l.contains("ping")) {
        println!("    {l}");
    }
    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    assert!(input.stage >= 8, "the input ran through: {:?}", input.log);
    assert!(
        st.grants.iter().any(|g| g.1 == PACK) && st.grants.iter().any(|g| g.1 == BACKPACK_RIFLE),
        "the grants {:?}",
        st.grants
    );

    // the client's shell messages in order, and its two releases
    let d = dc_botclient::capdecode::decode_file(&dir.join("client_capture.jsonl"))
        .expect("client capture");
    let up: Vec<(f64, Vec<u8>)> = d
        .msgs
        .iter()
        .filter(|m| m.c2s && m.shell().is_some())
        .map(|m| (m.t, m.bits.as_bytes().to_vec()))
        .collect();
    let releases: Vec<usize> = up
        .iter()
        .enumerate()
        .filter(|(_, m)| m.1.get(1) == Some(&0x82))
        .filter(|(_, m)| fire_of(&m.1).is_some_and(|f| f.weapon_id == PACK))
        .map(|(i, _)| i)
        .collect();
    let hexs = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    for &i in &releases {
        let next: Vec<String> = up[i + 1..(i + 4).min(up.len())]
            .iter()
            .map(|m| hexs(&m.1))
            .collect();
        println!("release at {:.3}: then {next:?}", up[i].0);
    }
    assert_eq!(releases.len(), 2, "two releases of the pack");
    let time_of = |i: usize| fire_of(&up[i].1).map(|f| f.time_ms).unwrap();
    let (r1, r2) = (releases[0], releases[1]);
    let (t1, t2) = (time_of(r1), time_of(r2));
    // throw 1 of 2: the reload of the pack in the same moment, no change before the second release
    assert_eq!(
        hexs(&up[r1 + 1].1),
        "0b838f00",
        "0x83 [143] after the first release"
    );
    assert!(up[r1 + 1].0 - up[r1].0 < 0.05);
    assert!(
        up[r1..r2].iter().all(|m| m.1.get(1) != Some(&0x7f)),
        "no weapon change between the releases"
    );
    // the last one: the empty switch to the backpack rifle in slot 4, in the same moment
    assert_eq!(hexs(&up[r2 + 1].1), "0b760302");
    assert_eq!(hexs(&up[r2 + 2].1), "0b7f041b001b00", "7f [4][27][27]");
    assert!(up[r2 + 2].0 - up[r2].0 < 0.05);
    // the pack's key afterwards draws nothing
    assert!(
        up[r2 + 3..]
            .iter()
            .all(|m| !(m.1.get(1) == Some(&0x7f) && m.1.get(2) == Some(&3))),
        "the empty pack was not drawn again"
    );
    assert_eq!(
        st.weapon.map(|c| (c.slot, c.weapon)),
        Some((4, BACKPACK_RIFLE))
    );

    // the server: two flights, no refusal, the counts 1.0 then 0.0 to the thrower
    assert!(srv.has("throw 1 (weapon 143"), "the server's first throw");
    assert!(srv.has("throw 2 (weapon 143"), "the server's second throw");
    assert!(!srv.has("shot beyond the clip"), "no release refused");
    let own_conn = d
        .msgs
        .iter()
        .find(|m| m.c2s)
        .map(|m| m.conn)
        .expect("own connection");
    let counts: Vec<f32> = d
        .of(own_conn, false)
        .flat_map(|m| unbundle(&m.bits))
        .filter_map(|p| {
            let b = p.as_bytes().to_vec();
            (b.len() >= 11 && b[1] == 0x66 && b[2] == 1 && u16::from_le_bytes([b[3], b[4]]) == PACK)
                .then(|| f32::from_le_bytes([b[7], b[8], b[9], b[10]]))
        })
        .collect();
    println!("0x66 kind 1 counts of the pack: {counts:?}");
    let tail: Vec<f32> = counts.iter().copied().skip_while(|c| *c > 1.5).collect();
    assert!(tail.starts_with(&[1.0, 0.0]), "{counts:?}");

    // the first-person view: the grenade stays after throw 1, is gone with throw 2 and never comes back
    let marks = &r.view_weapon_marks;
    assert_eq!(view_at(marks, t1 + 300), Some(PACK), "{marks:?}");
    assert_eq!(
        view_at(marks, t2.saturating_sub(50)),
        Some(PACK),
        "{marks:?}"
    );
    assert_eq!(view_at(marks, t2 + 100), Some(BACKPACK_RIFLE), "{marks:?}");
    assert!(
        marks
            .iter()
            .filter(|m| m.1.is_some_and(|c| c > t2))
            .all(|m| m.2 != Some(PACK)),
        "no grenade in the view after the last throw: {marks:?}"
    );
    let pack_models: Vec<String> = table
        .get(PACK)
        .map(|w| w.pv_models.iter().map(|m| m.to_ascii_lowercase()).collect())
        .unwrap_or_default();
    if r.render_skipped.is_none() && r.adapter.is_some() {
        let last = r.view_model_marks.last().cloned();
        println!("last model at the eye: {last:?}; the pack's models {pack_models:?}");
        let drawn = last
            .and_then(|m| m.1)
            .expect("a first-person model at the end");
        assert!(
            !pack_models.contains(&drawn.to_ascii_lowercase()),
            "the hands after the last throw hold no grenade: {drawn}"
        );
        if let Some(img) = &r.last_frame {
            let out = dc_render::default_out_dir().join("dc-client");
            let _ = std::fs::create_dir_all(&out);
            let path = out.join("grenade_after_last_throw.png");
            img.save_png(&path).expect("png");
            println!("PNG {} (ROM-derived, never commit)", path.display());
        }
    } else {
        println!(
            "no GPU adapter: view checked on the frame source only ({:?})",
            r.render_skipped
        );
    }
}
