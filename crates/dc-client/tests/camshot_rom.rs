//! CLI-240 / DSV-112 end to end over loopback UDP, ROM-gated (skipped without `CA_ROM`): our `dc-dserver` on the
//! user's first Search and Destroy map (mode class 2) with the room line our lobby writes for it (its `trig` names
//! the map's start and end triggers), one headless client. The round ends by its time limit; with the round result
//! the server switches the world camera on at the end trigger's camera point; the client draws the frames from it,
//! held still, until the result screen. The frames of the shot are written as PNGs (ROM-derived pictures: local
//! files, never committed) to `DC_CAMSHOT_PNG_DIR`, else under cargo's temporary directory.
// DISASM: cshell+0x1f83d0 (the client side); OBSERVED: ROM world scripts (end trigger: camera on at a camera point)

mod support;

use std::time::Duration;

use dc_client::hud::HudChoice;
use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetOptions, NetWorld, ViewAngles,
};
use dc_sim::Commands;
use support::*;

struct Idle;

impl InputSource for Idle {
    fn frame(&mut self, _view: &mut ViewAngles) {}
    fn tick(&mut self, _tick: u64, _hz: u32, _view: &mut ViewAngles) -> Commands {
        Commands {
            forward: true,
            ..Commands::default()
        }
    }
    fn label(&self) -> &'static str {
        "forward"
    }
}

#[test]
fn rom_snd_round_end_plays_the_world_camera_shot() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let vfs = dc_rez::RezVfs::new(&rom);
    let gr = dc_bute::GameRoomData::from_vfs(&vfs).expect("room table");
    let unq = |s: &str| s.trim().trim_matches('"').to_string();
    // the first plain mode of class 2 and its first map
    let Some((mode, key)) = gr.modes(false).iter().enumerate().find_map(|(i, m)| {
        let class = m.get("GameType").map(|t| unq(t))?;
        let key = unq(m.get("Key")?);
        (class == "2").then_some((i, key))
    }) else {
        println!("skipped: no class-2 mode");
        return;
    };
    let Some(map) = gr
        .maps()
        .iter()
        .position(|m| m.contains_key(&format!("GMS_{key}")))
    else {
        println!("skipped: no map of the class-2 mode");
        return;
    };
    let (_md, doc) = dc_bute::gms_file_with_vfs(&vfs, &gr, map, mode, false);
    let world = doc
        .get_str("MapInfo", "Map")
        .map(unq)
        .expect("the map's world");
    let line = dc_bute::room_line_with_vfs(
        &vfs,
        &gr,
        &dc_bute::RoomConfig {
            map,
            mode,
            ..dc_bute::RoomConfig::default()
        },
        &dc_bute::RoomOptions::default(),
    );
    assert!(line.contains(" trig="), "the room line names the triggers");

    let mut cmd = std::process::Command::new(dserver_exe());
    cmd.args(["-port", "0", "-world", &world, "-idle", "30", "-tick", "20"])
        .args(["-roundsecs", "8", "-teamempty", "60"])
        // one timed round per match (class 2 is a round mode): the session reaches its result screen in the run
        .args(["-roundloop", "off"]);
    let mut srv = Proc::spawn(cmd);
    let ready = srv
        .wait_line("@ready", Duration::from_secs(60))
        .unwrap_or_else(|| panic!("no @ready; server said {:?}", srv.seen));
    let port: u16 = ready
        .split_whitespace()
        .find_map(|t| t.strip_prefix("port="))
        .and_then(|p| p.parse().ok())
        .expect("port");
    srv.line(&line);
    srv.line("user 5 5 5 0 0 ShotClient");

    let dir = std::env::var_os("DC_CAMSHOT_PNG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| scratch("camshot"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "ShotClient",
    );
    o.rom = Some(rom.clone());
    o.result_hold = Duration::from_millis(1500);
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let opts = HeadlessOptions {
        frames: 60 * 30,
        frame_us: 16_667,
        width: 640,
        height: 480,
        out: Some(dir.clone()),
        prefix: "sndshot_".into(),
        save_every: 12,
        font_rom: Some(rom.clone()),
        hud: HudChoice::Rom(rom.clone()),
        debug_panel: false,
        realtime: true,
        stop_after_over: Some(30),
        ..HeadlessOptions::default()
    };
    let mut input = Idle;
    let r = run_headless(&mut net, Some(&mut input as &mut dyn InputSource), &opts)
        .expect("headless run");
    let st = handle.shutdown(Duration::from_secs(5));
    // the server ends by itself once the client left the result screen
    srv.finish(Duration::from_secs(8));
    srv.drain();
    println!("{}", r.summary());
    for l in srv
        .seen
        .iter()
        .filter(|l| l.contains("camera") || l.contains("round over"))
    {
        println!("  server: {l}");
    }
    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    assert!(
        srv.has("world camera: 1 changed, 1 live"),
        "the server switched the camera on"
    );
    let marks = &r.cam_shot_marks;
    println!(
        "shot: {} frames, first {:?}, last {:?}",
        r.cam_shot_frames,
        marks.first(),
        marks.last()
    );
    assert!(
        r.cam_shot_frames > 60 * 3,
        "the shot lasts until the result screen: {} frames",
        r.cam_shot_frames
    );
    // a still shot from one camera point, the body held
    let first = marks[0];
    for m in marks {
        assert_eq!(m.1, first.1, "one place");
        assert!(m.4, "the body is held");
    }
    // the shot's pictures (rendering needs an adapter)
    if r.adapter.is_some() {
        let shot = dir.join("shot");
        std::fs::create_dir_all(&shot).unwrap();
        let (a, b) = (first.0, marks.last().unwrap().0);
        let mut n = 0;
        for p in &r.images {
            let f: Option<u32> = p
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.strip_prefix("sndshot_"))
                .and_then(|s| s.parse().ok());
            if f.is_some_and(|f| f >= a && f <= b) {
                std::fs::copy(p, shot.join(p.file_name().unwrap())).unwrap();
                n += 1;
            }
        }
        println!("{n} shot pictures in {}", shot.display());
        assert!(n >= 10, "pictures of the shot: {n}");
    } else {
        println!("no adapter: rendering skipped ({:?})", r.render_skipped);
    }
}
