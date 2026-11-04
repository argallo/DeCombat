//! The radar in a match of our own dedicated server over loopback UDP: `dc-dserver` with the made-up world of
//! `dc_client::testkit` and a room line that carries a radar field (the server's 0x45 intro block, DSV-50), a
//! dc-botclient walker on the client's own team, the client headless with our no-ROM HUD. The team mate (team from
//! the server's 0x6b roster) is drawn on the radar: pixels of the team-mate colour inside the radar's rectangle of
//! the last HUD image. GPU-gated (the HUD image is composed in the render branch of the frame loop).
// SPEC: DSERVER_PIPE.md §3.1 (room and user lines); GAME_MESSAGES_2009_S2C.md §2.1.1 (0x45 intro block fields f..h)
// EVIDENCE: cshell+0xb60f0 (team mates of the own team are drawn)

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::run_game;
use dc_botclient::session::{BotSession, GameConfig};
use dc_client::hud::radar::RadarLayout;
use dc_client::hud::HudChoice;
use dc_client::testkit::{synth_font, synth_source, SYNTH_DIMS};
use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetOptions, NetWorld, ViewAngles,
    WalkTest, WorldFile,
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

fn walker(port: u16, world: std::path::PathBuf, secs: u64) -> std::thread::JoinHandle<BotSession> {
    std::thread::spawn(move || {
        let mut cfg = GameConfig::new("MateBot");
        cfg.hello_tag = "matebot".into();
        cfg.motion = MotionKind::Circle;
        cfg.world_file = Some(world);
        cfg.load_ms = 300;
        let mut bot = BotSession::new(cfg, None, Recorder::null("MateBot"));
        let server: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        run_game(
            &mut bot,
            server,
            Duration::from_secs(secs),
            &Recorder::null("MateBot"),
        )
        .expect("bot udp");
        bot
    })
}

#[test]
fn team_mate_from_the_server_roster_is_on_the_radar() {
    let dir = scratch("netradar");
    let world = synth_world_file(&dir);
    let (mut srv, port) = start_dserver(
        &world,
        &[
            "-tick",
            "20",
            "-roundsecs",
            "14",
            "-holds",
            "1500,800",
            "-timeout-hold",
            "1500",
            "-seed",
            "5",
        ],
    );
    // a made-up radar field (picture | first vector | second vector | real | real | width | height)
    srv.line(
        "room key=TDM type=1 goal=50 ff=0 time=14 radar=Radar.dtx|0.2,0,0.2|0.5,0,0.4|0.0|0.34|1024|1024",
    );
    // both on the first team
    srv.line("user 1 1 1 0 0 RadarClient");
    srv.line("user 2 2 2 0 0 MateBot");
    let bot = walker(port, world.clone(), 24);

    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "RadarClient",
    );
    o.world_file = Some(world_file());
    o.result_hold = Duration::from_millis(1500);
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let (w, h) = (512, 384);
    let opts = HeadlessOptions {
        frames: 60 * 10,
        frame_us: 16_667,
        width: w,
        height: h,
        out: None,
        save_every: 30,
        font_bytes: Some(synth_font()),
        realtime: true,
        hud: HudChoice::Fallback,
        ..HeadlessOptions::default()
    };
    let mut walk = WalkTest::new(ViewAngles::default());
    let r = run_headless(&mut net, Some(&mut walk as &mut dyn InputSource), &opts)
        .expect("headless run");
    let st = handle.shutdown(Duration::from_secs(5));
    let _ = bot.join();
    srv.drain();
    let _ = srv.finish(Duration::from_secs(5));
    println!("{}", r.summary());
    if r.adapter.is_none() {
        println!("skipped: no adapter ({:?})", r.render_skipped);
        return;
    }
    let t = st
        .tracks
        .values()
        .find(|t| t.name == "MateBot")
        .unwrap_or_else(|| panic!("no remote track for the mate: {:?}", st.tracks));
    assert_eq!(t.team, 1, "team from 0x6b");
    assert!(srv.has("room settings"), "the server took the room line");
    let img = r.last_hud.expect("a HUD image");
    let l = RadarLayout::fallback();
    let (sx, sy) = (w as f32 / 1024.0, h as f32 / 768.0);
    let mut disc = 0;
    let mut mate = 0;
    for y in (l.place[1] * sy) as u32..((l.place[1] + l.size[1]) * sy) as u32 {
        for x in (l.place[0] * sx) as u32..((l.place[0] + l.size[0]) * sx) as u32 {
            let o = ((y * w + x) * 4) as usize;
            if img.rgba[o + 3] > 20 {
                disc += 1;
            }
            if img.rgba[o..o + 3] == l.mate_colour[..3] {
                mate += 1;
            }
        }
    }
    println!("radar pixels {disc}, team-mate pixels {mate}");
    assert!(disc > 500, "the radar is drawn");
    assert!(mate > 0, "the team mate is on the radar");
}
