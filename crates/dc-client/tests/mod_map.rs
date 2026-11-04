//! A community map end to end (MAPKIT_PLAN.md 6.5, MAP-30 / MAP-31): a tiny map mod in a temporary folder (a
//! `shared` manifest, a `main.lua` registering one map, a world of `dc.*` objects with `@mod:` textures written by
//! our own writers), `dc-dserver` started on that world file announcing it as `@mod:<id>/assets/maps/x.dat` with
//! `-worldobjects world`, and the client headless with the recipe of that folder: the announce resolves through the
//! mod layer the headless run installs (no 0x29), the file's own `dc.player_spawn` is the spawn the server places
//! the player on, the textures resolve through the layer, and frames render when an adapter exists. Nothing of the
//! ROM is in this file.
// SPEC: DSERVER_PIPE.md §2 (launch line: -worldfile, -announce), §3.1 (user line)

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use dc_client::mods::{HostRecipe, RecipeMod};
use dc_client::testkit::synth_world_bytes_with;
use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetOptions, NetWorld, ViewAngles,
};
use dc_mod_lua::{ModOrigin, ModPolicy};
use dc_sim::Commands;
use dc_world::synth::{SynthObject, SynthValue};
use support::*;

const MOD_ID: &str = "synth_map";
const WORLD: &str = "@mod:synth_map/assets/maps/x.dat";
/// The file's one spawn: on the floor at the origin, facing +Z (the server drops it to the floor, DSV-48).
const SPAWN: [f32; 3] = [100.0, 40.0, -300.0];

/// A player who does nothing.
struct Idle;

impl InputSource for Idle {
    fn frame(&mut self, _view: &mut ViewAngles) {}
    fn tick(&mut self, _tick: u64, _hz: u32, _view: &mut ViewAngles) -> Commands {
        Commands::default()
    }
    fn label(&self) -> &'static str {
        "idle"
    }
}

fn put(p: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// The map mod folder: manifest, main.lua, the world, three 2 x 2 textures, a screenshot.
fn write_map_mod(root: &Path) -> PathBuf {
    let dir = root.join(MOD_ID);
    put(
        &dir.join("manifest.json"),
        format!(
            r#"{{"id": "{MOD_ID}", "name": "Synth Map", "version": "1.0.0", "entry": "main.lua", "kind": "shared",
                "requires_client": true, "capabilities": ["content.maps", "assets"],
                "description": "A test map mod for the client's community-map tests.", "author": "tests",
                "screenshots": ["screenshots/main.png"]}}"#
        )
        .as_bytes(),
    );
    put(
        &dir.join("screenshots/main.png"),
        &[0x89u8, b'P', b'N', b'G', 13, 10, 26, 10, 0, 0, 0, 0],
    );
    put(
        &dir.join("main.lua"),
        br#"return function(mod)
            mod.content.maps:register({
                id = "synth_map",
                name = "Synth Map",
                description = "A made-up course.",
                world = mod.assets:path("assets/maps/x.dat"),
                modes = { "TDM", "FFA" },
            })
        end"#,
    );
    for (name, colour) in [
        ("floor", [150u8, 150, 150, 255]),
        ("step", [180, 120, 60, 255]),
        ("wall", [70, 90, 200, 255]),
    ] {
        let dtx = dc_tex::write_bgra8(2, 2, &colour.repeat(4)).unwrap();
        put(&dir.join(format!("assets/textures/{name}.dtx")), &dtx);
    }
    let spawn = SynthObject::dc(
        "dc.player_spawn",
        "spawn_1",
        SPAWN,
        [0.0, 0.0, 0.0],
        &[
            ("team", SynthValue::Text(b"1".to_vec())),
            ("modes", SynthValue::Text(Vec::new())),
        ],
    );
    let bytes = synth_world_bytes_with(
        &|k| format!("@mod:{MOD_ID}/assets/textures/{k}.dtx").into_bytes(),
        vec![spawn],
    );
    put(&dir.join("assets/maps/x.dat"), &bytes);
    dir
}

fn recipe(dir: &Path) -> HostRecipe {
    HostRecipe {
        policy: ModPolicy::Modded {
            client_visual_allowed: false,
        },
        settings: Default::default(),
        mods: vec![RecipeMod {
            dir: dir.to_path_buf(),
            origin: ModOrigin::Server,
            id: MOD_ID.into(),
            version: "1.0.0".into(),
            kind: "shared".into(),
        }],
    }
}

#[test]
fn a_mod_announce_loads_the_community_map_headless_and_spawns_on_its_own_spawn() {
    let dir = scratch("modmap");
    let mod_dir = write_map_mod(&dir);
    let world = mod_dir.join("assets/maps/x.dat");
    let (mut srv, port) = start_dserver(
        &world,
        &[
            "-announce",
            WORLD,
            "-worldobjects",
            "world",
            "-tick",
            "20",
            "-roundsecs",
            "30",
            "-nomovecheck",
        ],
    );
    srv.line("room key=TDM type=1 goal=50 ff=0 time=30");
    srv.line("user 1 1 1 0 0 MapClient");

    let log = dir.join("client_events.jsonl");
    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "MapClient",
    );
    // no ROM, no world file: the announce must resolve through the mod set alone
    o.rom = None;
    o.log = Some(log.clone());
    o.mods = Some(recipe(&mod_dir));
    o.leave_after = Some(Duration::from_secs(4));
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    let shots = dir.join("shots");
    std::fs::create_dir_all(&shots).unwrap();
    let opts = HeadlessOptions {
        frames: 60 * 7,
        frame_us: 16_667,
        width: 320,
        height: 240,
        out: Some(shots.clone()),
        prefix: "modmap_".into(),
        save_every: u32::MAX,
        font_rom: None,
        realtime: true,
        stop_after_over: Some(30),
        debug_panel: false,
        mods: Some(recipe(&mod_dir)),
        ..HeadlessOptions::default()
    };
    let mut idle = Idle;
    let r = run_headless(&mut net, Some(&mut idle as &mut dyn InputSource), &opts)
        .expect("headless run: the @mod: world loads");
    let st = handle.shutdown(Duration::from_secs(5));
    srv.quit();
    srv.drain();
    let _ = srv.finish(Duration::from_secs(8));
    srv.drain();

    println!("{}", r.summary());
    println!("world: {}", r.world);
    println!(
        "net: phase {:?} end {:?} world {:?} teleports {:?} fatal {:?}",
        st.phase, st.end, st.world, st.teleports, st.fatal
    );
    for l in srv.seen.iter().filter(|l| !l.contains("ping")) {
        println!("  srv: {l}");
    }

    // 1. the world loaded through the mod layer: no world failure (the 2009 0x29), the announce as the name
    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    assert_eq!(st.world.as_deref(), Some(WORLD));
    assert!(
        r.world.contains(" 1 spawns"),
        "the file's own dc.player_spawn is the world's spawn: {}",
        r.world
    );
    // 2. the three @mod: textures resolved (none missing) when the scene was built
    if r.adapter.is_some() {
        assert!(
            r.world.contains("textures missing 0"),
            "the mod textures resolve: {}",
            r.world
        );
        assert!(r.frames > 0, "frames rendered");
        assert!(r.last_frame.is_some(), "a frame image");
    } else {
        println!("no adapter: rendering skipped ({:?})", r.render_skipped);
    }
    // 3. the server placed the player on the file's spawn (dropped to the floor, DSV-48: the same x / z, the body's
    // centre a box half height above the floor at y = 0)
    let (_, pos, half) = st
        .teleports
        .first()
        .copied()
        .expect("the server's first teleport (the spawn)");
    assert!(
        (pos[0] - SPAWN[0]).abs() < 1.0 && (pos[2] - SPAWN[2]).abs() < 1.0,
        "the spawn's x / z: {pos:?} vs {SPAWN:?}"
    );
    assert!(
        pos[1] >= 0.0 && (pos[1] - half[1]).abs() < 2.0,
        "standing on the floor: {pos:?} (box {half:?})"
    );
    assert_eq!(r.nan_ticks + r.nan_frames + r.nan_remotes, 0);
    // the mod layer the run installed is gone with it
    assert!(dc_client::modassets::active().is_none());
}

#[test]
fn without_a_mod_set_a_mod_announce_fails_as_a_missing_world() {
    let dir = scratch("modmap-nomods");
    let mod_dir = write_map_mod(&dir);
    // the loader alone (no server): nothing installed and no recipe is the 0x29 case, the recipe resolves it
    let e = dc_client::net::mod_world_for(WORLD, None, None)
        .err()
        .expect("refused");
    assert!(e.to_string().contains("no mod set is loaded"), "{e}");
    let r = recipe(&mod_dir);
    let w = dc_client::net::mod_world_for(WORLD, Some(&r), None).expect("through the recipe");
    assert_eq!(w.name, WORLD);
    assert!(!w.bytes.is_empty());
    let e = dc_client::net::mod_world_for("@mod:other/assets/maps/x.dat", Some(&r), None)
        .err()
        .expect("another mod's world");
    assert!(e.to_string().contains("not in the loaded mod set"), "{e}");
}
