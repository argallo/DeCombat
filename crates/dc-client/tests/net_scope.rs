//! The scope and the class keys against our own dedicated server over loopback UDP (ROM-gated, headless, scripted
//! input): `dc-dserver` with the made-up world, a standing bot, and a player whose loadout is the rifle 145, the
//! pistol 164, the knife 0, the two-grenade pack 143 and the backpack sniper rifle 27 (weapon slot 4; the server
//! grants it with its default scope, DSV-80). The script:
//! 1. key 1 (class key 0) with the primary held: the next weapon of order class 1, the backpack rifle in slot 4
//!    (`7f [4][27][27]`; M18: the backpack was reachable only with the wheel);
//! 2. the zoom key held for 2 s (as the real client in rust6): every 0x64 inside the hold carries control 0x80000 and
//!    none outside it; the level goes to 1 at once and `pu_flags` 0x0800 is set while zoomed; the camera field moves to
//!    the scope's `ZoomFov0` (the frame's zoom mark);
//! 3. one shot while zoomed: the 0x82 zoom byte (`moving`) is set;
//! 4. no key: the sniper (a `ReloadAfterFire` row) leaves the scope by itself 300 ms after the zoomed shot (CLI-646):
//!    `pu_flags` 0x0800 ends with the change out, the field is the normal one again;
//! 5. one shot unzoomed: the zoom byte is clear; key 1 again: back to the rifle in slot 0.
// SPEC: MOVEMENT_2009.md §5.1 (0x80000), GAME_MESSAGES_2009_C2S.md §2.1 (0x64 layout, pu_flags 0x0800), §4.1 (0x82),
// §4.4 (0x7f); GAME_MESSAGES_2009_S2C.md §4.4 (grant parts); DSERVER_PIPE_addendum_parts.md §2
// OBSERVED: capture 20261025_125432_rust6/analysis/updates.csv (the 2009 client: the 0x80000
// bit in all 13 updates of a 2 s hold, 226.672 .. 228.687 s, and in none before or after; no sight on that rifle, so
// pu_flags 0x0800 stayed clear), capture 20261025_153705_rust7 (with a sight: 0x0800 from the
// press, gone 120-200 ms after the press that zooms out)

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::run_game;
use dc_botclient::session::{BotSession, GameConfig};
use dc_botclient::weapon::read_fire;
use dc_client::arms::{WeaponCommands, WeaponTable};
use dc_client::testkit::{synth_font, synth_source, SYNTH_DIMS};
use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetOptions, NetWorld, ServerHold,
    ViewAngles, WorldFile,
};
use dc_sim::Commands;
use support::*;

const SNIPER: u16 = 27;
const RIFLE: u16 = 145;
const ZOOM_BIT: u32 = 0x0008_0000;
const SPRINT_BIT: u32 = 0x0040_0000;
const PU_ZOOM: u16 = 0x0800;

fn world_file() -> WorldFile {
    let s = synth_source();
    WorldFile {
        bytes: s.bytes,
        stand_dims: SYNTH_DIMS,
        textures: s.textures,
    }
}

/// The scripted steps; the times are wall-clock seconds (a paced run catches up after the world load with frames
/// that come faster than one per `frame_us`, so the frame count is no clock; the session clock is the wall clock).
struct ScopeScript {
    net: dc_client::NetHandle,
    started: std::time::Instant,
    t: f64,
    alive_at: Option<f64>,
    stage: u32,
    stage_at: f64,
    /// Presses that bring the level back to 0 after the first one (the scope's `ZoomLevel`).
    out_presses: u32,
    class_key: Option<u8>,
    zoom: bool,
    fire: bool,
    /// Frame-clock windows of the zoom key (start, end).
    holds: Vec<(f64, f64)>,
    log: Vec<String>,
}

impl ScopeScript {
    fn go(&mut self, stage: u32) {
        self.stage = stage;
        self.stage_at = self.t;
        self.log.push(format!("{:.2}s stage {stage}", self.t));
    }

    fn hold(&mut self, on: bool) {
        if on && !self.zoom {
            self.holds.push((self.t, f64::INFINITY));
        }
        if !on && self.zoom {
            if let Some(h) = self.holds.last_mut() {
                h.1 = self.t;
            }
        }
        self.zoom = on;
    }
}

impl InputSource for ScopeScript {
    fn frame(&mut self, _view: &mut ViewAngles) {
        self.t = self.started.elapsed().as_secs_f64();
    }
    fn server(&mut self, v: &ServerHold) {
        if v.life_state == Some(1) && self.alive_at.is_none() {
            self.alive_at = Some(self.t);
        }
    }
    fn tick(&mut self, _tick: u64, _hz: u32, _view: &mut ViewAngles) -> Commands {
        Commands {
            run: true,
            ..Commands::default()
        }
    }
    fn aim(
        &mut self,
        _eye: [f32; 3],
        _remotes: &[dc_client::RemoteBody],
        _own_team: Option<u8>,
        _view: &mut ViewAngles,
    ) {
        let since = self.t - self.stage_at;
        self.class_key = None;
        self.fire = false;
        match self.stage {
            0 if self.alive_at.is_some_and(|a| self.t - a > 1.5) => {
                self.class_key = Some(0);
                self.go(1)
            }
            // the select clip, then a 2 s hold
            1 if since > 1.5 => {
                self.hold(true);
                self.go(2)
            }
            2 if since > 2.0 => {
                self.hold(false);
                self.go(3)
            }
            // one short shot while zoomed
            3 if since > 0.5 => {
                let shots = self.net.stats().arms.shots;
                self.fire = shots == 0 && since < 0.7;
                if shots >= 1 {
                    self.go(4)
                }
            }
            // the presses back to level 0: 0.2 s down, 1.3 s apart (the bolt and the change finish first)
            4 => {
                let n = (since / 1.5) as u32;
                let within = since % 1.5;
                if since > 0.5 && n < self.out_presses + 1 && n >= 1 {
                    self.hold(within < 0.2);
                } else {
                    self.hold(false);
                }
                if n > self.out_presses + 1 {
                    self.go(5)
                }
            }
            5 if since > 0.5 => {
                let shots = self.net.stats().arms.shots;
                self.fire = shots == 1 && since < 0.7;
                if shots >= 2 && since > 2.0 {
                    self.class_key = Some(0);
                    self.go(6)
                }
            }
            6 if since > 1.5 => self.go(7),
            _ => {}
        }
    }
    fn weapon(&mut self) -> WeaponCommands {
        WeaponCommands {
            fire: self.fire,
            zoom: self.zoom,
            class_key: self.class_key.take(),
            ..WeaponCommands::default()
        }
    }
    fn label(&self) -> &'static str {
        "scope script"
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

/// `pu_flags` and the control word of an unreliable 0x64 body (the shell id byte first), `None` for any other form.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1 (`[pu_flags:u16][yaw8c:u8][body_angles:vec96][aux:6 x u32]` then the blocks
// announced by 0x0001 (vec96), 0x0100 (quat128), 0x0200 (f32), 0x0400 (vec96), then `[control:u32]`)
fn update_of(b: &[u8]) -> Option<(u16, u32)> {
    if b.get(1) != Some(&0x64) {
        return None;
    }
    let w = dc_wire::udp::BitWriter::from_bytes(b);
    let mut r = w.reader();
    r.read_u8();
    r.read_u8();
    let flags = r.read_u16();
    if flags & 0x0020 == 0 {
        return None;
    }
    r.read_u8();
    let mut skip = |n: usize| {
        for _ in 0..n {
            r.read_u32();
        }
    };
    skip(3 + 6);
    if flags & 0x0001 != 0 {
        skip(3);
    }
    if flags & 0x0100 != 0 {
        skip(4);
    }
    if flags & 0x0200 != 0 {
        skip(1);
    }
    if flags & 0x0400 != 0 {
        skip(3);
    }
    Some((flags, r.read_u32()))
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

#[test]
fn the_scope_zooms_with_the_control_bit_fov_and_cone_and_key_1_reaches_the_backpack() {
    let Some(rom) = dc_rom::test_rom() else {
        println!(
            "skipped: CA_ROM unset (the scope, the order classes and the clips come from the ROM)"
        );
        return;
    };
    let table = WeaponTable::open(&rom).expect("weapon table");
    let scope = table
        .mod_info
        .get(&27)
        .cloned()
        .expect("the sniper's default scope row");
    assert!(scope.zoom_level >= 1);
    let zoom_fov0 = scope.zoom_fov[0];
    let dir = scratch("scope");
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
    srv.line("user 1 1 1 0 0 ScopeClient loadout=0:145,1:164,2:0,3:143,10:27");
    srv.line("user 2 2 2 1 0 StandBot");
    let bot = stander(port, world.clone(), 44);
    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "ScopeClient",
    );
    o.world_file = Some(world_file());
    o.log = Some(dir.join("client_events.jsonl"));
    o.capture = Some(dir.join("client_capture.jsonl"));
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let opts = HeadlessOptions {
        frames: 60 * 34,
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
    let mut input = ScopeScript {
        net: net.handle(),
        started: std::time::Instant::now(),
        t: 0.0,
        alive_at: None,
        stage: 0,
        stage_at: 0.0,
        // CLI-646: the bolt-action rifle zooms out by itself after the zoomed shot; a press would zoom in again
        out_presses: 0,
        class_key: None,
        zoom: false,
        fire: false,
        holds: Vec::new(),
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
        "grants {:?}\nheld {:?}\narms {:?}\nzoom marks {:?}\nholds {:?}",
        st.grants, st.weapon, st.arms, r.zoom_marks, input.holds
    );
    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    assert!(input.stage >= 7, "the input ran through: {:?}", input.log);

    let d = dc_botclient::capdecode::decode_file(&dir.join("client_capture.jsonl"))
        .expect("client capture");
    let up: Vec<(f64, Vec<u8>)> = d
        .msgs
        .iter()
        .filter(|m| m.c2s && m.shell().is_some())
        .map(|m| (m.t, m.bits.as_bytes().to_vec()))
        .collect();
    let hexs = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    // 1. key 1 drew the backpack rifle in slot 4, the second key 1 the rifle in slot 0
    let changes: Vec<String> = up
        .iter()
        .filter(|m| m.1.get(1) == Some(&0x7f))
        .map(|m| hexs(&m.1))
        .collect();
    println!("weapon changes {changes:?}");
    let to_sniper = changes
        .iter()
        .position(|c| c == "0b7f041b001b00")
        .expect("7f [4][27][27] after key 1");
    assert!(
        changes[to_sniper + 1..]
            .iter()
            .any(|c| c.starts_with("0b7f00")),
        "key 1 again: back to slot 0 ({changes:?})"
    );
    assert_eq!(st.weapon.map(|c| (c.slot, c.weapon)), Some((0, RIFLE)));

    // 2. / 4. the control bit and the zoom flag in the 0x64 updates
    let updates: Vec<(f64, u16, u32)> = up
        .iter()
        .filter_map(|m| update_of(&m.1).map(|(f, c)| (m.0, f, c)))
        .collect();
    let with_bit: Vec<f64> = updates
        .iter()
        .filter(|u| u.2 & ZOOM_BIT != 0)
        .map(|u| u.0)
        .collect();
    println!("{} updates, {} with 0x80000", updates.len(), with_bit.len());
    assert!(
        updates
            .iter()
            .all(|u| u.2 & ZOOM_BIT == 0 || u.2 & SPRINT_BIT == 0),
        "never with SPRINT"
    );
    // the 2 s hold: every update inside it carries the bit (the capture clock and the frame clock differ by an
    // offset; the hold is found as the longest run of updates with the bit)
    let mut best = (0usize, 0usize);
    let mut i = 0;
    while i < updates.len() {
        if updates[i].2 & ZOOM_BIT != 0 {
            let j = (i..updates.len())
                .find(|k| updates[*k].2 & ZOOM_BIT == 0)
                .unwrap_or(updates.len());
            if j - i > best.1 - best.0 {
                best = (i, j);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    let run_s = updates[best.1 - 1].0 - updates[best.0].0;
    println!(
        "longest run with the bit: {} updates over {:.3} s",
        best.1 - best.0,
        run_s
    );
    assert!(
        run_s > 1.7 && run_s < 2.2,
        "the 2 s hold carries the bit throughout"
    );
    assert!(
        best.1 - best.0 >= 13,
        "at least as many updates as the 2009 client's 13"
    );
    // pu_flags 0x0800 from the press of the hold, still set at its end (level 1)
    assert!(updates[best.0..best.1].iter().all(|u| u.1 & PU_ZOOM != 0));
    assert!(
        updates[..best.0].iter().all(|u| u.1 & PU_ZOOM == 0),
        "not zoomed before"
    );
    // the last updates: unzoomed again
    assert!(updates
        .last()
        .is_some_and(|u| u.1 & PU_ZOOM == 0 && u.2 & ZOOM_BIT == 0));
    // the bit appears in exactly the press windows: one run for the hold and one per press back to 0
    let runs = updates
        .windows(2)
        .filter(|w| w[0].2 & ZOOM_BIT == 0 && w[1].2 & ZOOM_BIT != 0)
        .count();
    assert!(runs <= 1 + scope.zoom_level as usize, "{runs} runs");

    // 3. / 5. the zoomed shot carries the zoom byte, the later one does not
    let shots: Vec<_> = up
        .iter()
        .filter_map(|m| fire_of(&m.1))
        .filter(|f| f.weapon_id == SNIPER)
        .collect();
    assert!(shots.len() >= 2, "{} shots", shots.len());
    assert!(shots[0].moving, "the zoomed shot");
    assert!(!shots.last().unwrap().moving, "the unzoomed shot");

    // the camera field: level 1 at ZoomFov0, back to the normal field at the end
    let levels: Vec<u32> = r.zoom_marks.iter().map(|m| m.2.level).collect();
    assert!(levels.contains(&1), "{levels:?}");
    let settled = r
        .zoom_marks
        .iter()
        .filter(|m| m.2.level == 1)
        .filter_map(|m| m.2.fov_x)
        .map(f32::to_degrees)
        .fold(f32::INFINITY, f32::min);
    println!("level-1 field {settled:.3} degrees (ZoomFov0 {zoom_fov0})");
    assert!(
        (settled - zoom_fov0).abs() < 0.01,
        "{settled} vs {zoom_fov0}"
    );
    let last = r.zoom_marks.last().expect("zoom marks").2;
    assert_eq!((last.level, last.active, last.fov_x), (0, false, None));
}
