//! ROM-gated (skipped when CA_ROM is unset): the scripted sequence on three maps of the user's ROM (the
//! first, middle and last name of the world archive, reported by index), rendered offscreen with the ROM
//! overlay font. Nothing may become non-finite, the body may never cross a solid surface or drop out of the
//! world, and it must end standing. The client loop must also equal dc-sim on a ROM map. Images are written
//! outside the tree (DC_RENDER_OUT or the temp folder) and are never committed.
// SPEC: MOVEMENT_2009.md §9-§10 (collision, ground), §15; FMT_DAT85.md §6.2 (player spawns)

use dc_client::{run_headless, HeadlessOptions, RomWorld, ScriptedInput, ViewAngles, WorldSource};
use dc_sim::{drop_to_floor, Cylinder, MovementParams, PlayerState, Simulation, TickInput};

#[test]
fn rom_three_maps_walk_jump_render() {
    let rom = dc_rom::rom_or_skip!();
    let names = RomWorld::map_names(&rom).expect("map names");
    assert!(names.len() >= 3, "world archive lists {} maps", names.len());
    let out = dc_client::headless::default_out_dir().join("rom_maps");
    for i in [0, names.len() / 2, names.len() - 1] {
        let opts = HeadlessOptions {
            frames: 50,
            frame_us: 100_000,
            width: 640,
            height: 360,
            out: Some(out.clone()),
            prefix: format!("map{i:02}_"),
            save_every: 10,
            font_rom: Some(rom.clone()),
            ..HeadlessOptions::default()
        };
        let mut src = RomWorld::new(&rom, &names[i]);
        let r = run_headless(&mut src, None, &opts).expect("headless run");
        println!("map index {i}: {}", r.summary());
        for l in &r.last_lines {
            println!("    | {l}");
        }
        assert_eq!(r.ticks, 300, "map {i}");
        assert_eq!(r.nan_ticks, 0, "map {i}: non-finite state");
        assert_eq!(r.nan_frames, 0, "map {i}: non-finite camera");
        assert_eq!(
            r.tunnel_ticks, 0,
            "map {i}: the body centre crossed a surface"
        );
        assert!(!r.fell_out, "map {i}: fell out of the world");
        assert!(r.ended_supported, "map {i}: not standing at the end");
        assert!(r.jumps >= 1, "map {i}: the scripted jump did not happen");
        assert!(r.path_xz > 100.0, "map {i}: hardly moved ({})", r.path_xz);
        if r.adapter.is_some() {
            assert_eq!(r.images.len(), 6, "map {i}");
            assert!(
                r.font
                    .as_deref()
                    .is_some_and(|f| !f.starts_with("not loaded")),
                "map {i}: ROM overlay font: {:?}",
                r.font
            );
        }
    }
    println!("PNGs in {} (ROM-derived, never commit)", out.display());
}

#[test]
fn rom_client_loop_equals_dc_sim() {
    let rom = dc_rom::rom_or_skip!();
    let names = RomWorld::map_names(&rom).expect("map names");
    let mut src = RomWorld::new(&rom, &names[0]);
    let world = src.load(None).expect("load");
    let spawn = world.spawn(0).expect("spawn");
    let p = MovementParams::default();
    let at = drop_to_floor(
        &world.collision,
        &Cylinder::from_dims(world.stand_dims),
        spawn.pos,
        p.step(),
        1000.0,
    );
    let script = ScriptedInput::new(ViewAngles {
        yaw: spawn.yaw,
        pitch: 0.0,
    });
    let start = PlayerState::new(at, world.stand_dims, &p);
    let mut sim = Simulation::new(p, &world.collision, start, 60);
    // the client applies the ROM speed rows of its body row 0 (no weapon offline; CLI-340, MOVEMENT_2009.md §7)
    let mods = dc_sim::SpeedTable::from_vfs(&dc_rez::RezVfs::new(&rom))
        .expect("speed rows")
        .mods(&dc_sim::Loadout {
            body: Some(0),
            ..dc_sim::Loadout::default()
        });
    for t in 0..ScriptedInput::ticks(60) {
        let (commands, v) = script.at(t as f64 / 60.0);
        sim.tick(&TickInput {
            commands,
            yaw: v.yaw,
            pitch: v.pitch,
            mods,
            ..TickInput::default()
        });
    }
    let want = sim.state.digest();
    for frame_us in [16_667u64, 7_000, 33_334] {
        let opts = HeadlessOptions {
            frames: 5_000_000u64.div_ceil(frame_us) as u32,
            frame_us,
            render: false,
            ..HeadlessOptions::default()
        };
        let r = run_headless(&mut src, None, &opts).expect("headless");
        assert_eq!(r.ticks, 300);
        assert_eq!(r.final_digest, want, "frame {frame_us} us");
    }
    println!("map index 0: client loop digest {want:#018x} equals dc-sim at 3 frame intervals");
}
