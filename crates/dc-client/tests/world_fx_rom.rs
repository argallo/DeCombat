//! ROM- and GPU-gated picture of world-fx (CLI-234; skipped without CA_ROM or a GPU adapter): a drum of JUNK_FLEA
//! blown up. The world file's own data pick the drum (a breakable whose death command names a placed effect and an
//! explosion object); the frame stage gets what the client hands it when the server breaks that drum: the placed-effect
//! object (the effect group the world file names, at its position) and the type-0x06 blast record the server sends for
//! the explosion object (the ammunition whose impact effect it names). Asserted: the groups start, the blast takes the
//! grenade path and asks for the explosion sound cue, quads are drawn. PNGs go to
//! `reports/client-worldfx/` (ROM-derived, never committed); names are never printed.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 (blast record), §8.4 (type 0x64); RENDER_2009.md §5.1 step 6
// DC-DEFAULT: CLI-234 (proposed)

use dc_client::fx::{FxEvent, FxStage, ShotRecord, WorldFx, NO_OBJECT};
use dc_client::objects::ServerObjects;
use dc_client::offscreen::Offscreen;
use dc_client::world::{FrameState, GpuTarget};
use dc_client::{RomWorld, ShootTest, ViewSettings, WorldSource};
use dc_dserver::destructibles::DeathAction;
use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
use dc_sim::V3;

const W: u32 = 800;
const H: u32 = 600;
const MAP: &str = "JUNK_FLEA";

fn out_dir() -> std::path::PathBuf {
    let o = dc_render::default_out_dir().join("client-worldfx");
    let _ = std::fs::create_dir_all(&o);
    o
}

/// The room's `trig` setting for this world in a room of the mode with this key, as our lobby writes it.
fn trig_for(rom: &std::path::Path, world: &str, key: &str) -> Option<String> {
    let vfs = dc_rez::RezVfs::new(rom);
    let gr = dc_bute::GameRoomData::from_vfs(&vfs).ok()?;
    let unq = |s: &str| s.trim().trim_matches('"').to_string();
    let mode = gr.modes(false).iter().position(|m| {
        m.get("Key")
            .is_some_and(|k| unq(k).eq_ignore_ascii_case(key))
    })?;
    let base = |p: &str| {
        let n = p.replace('/', "\\").to_ascii_uppercase();
        let b = n.rsplit('\\').next().unwrap_or(&n).to_string();
        b.strip_suffix(".DAT").map(str::to_string).unwrap_or(b)
    };
    let want = base(world);
    for (i, m) in gr.maps().iter().enumerate() {
        if !m.contains_key(&format!("GMS_{key}")) {
            continue;
        }
        let (_md, doc) = dc_bute::gms_file_with_vfs(&vfs, &gr, i, mode, false);
        if doc
            .get_str("MapInfo", "Map")
            .is_some_and(|w| base(&unq(w)) == want)
        {
            let cfg = dc_bute::RoomConfig {
                map: i,
                mode,
                ..dc_bute::RoomConfig::default()
            };
            let line =
                dc_bute::room_line_with_vfs(&vfs, &gr, &cfg, &dc_bute::RoomOptions::default());
            return match dc_dserver::pipe::parse_control(&line) {
                dc_dserver::pipe::Control::Room(r) => r.get("trig").map(str::to_string),
                _ => None,
            };
        }
    }
    None
}

/// The named brushes our server creates for this room `trig` (name -> first flag word), as a client sees them.
fn server_set(
    w: &dc_dserver::world::WorldInfo,
    tables: &dc_dserver::rom_tables::RomTables,
    trig: Option<&str>,
) -> ServerObjects {
    use dc_dserver::worldobj::{brush_flags, plan_scene, WorldObject, WorldObjects, ZoneClass};
    let mut next = 1000u16..;
    let mut s = ServerObjects::default();
    for o in plan_scene(
        w,
        tables,
        WorldObjects::World,
        ZoneClass::Room,
        12,
        trig,
        || next.next().unwrap(),
    ) {
        if let WorldObject::Brush { brush, .. } = o {
            s.brushes.insert(
                brush.name.to_ascii_lowercase(),
                brush_flags(&brush).0 & !0x0024,
            );
        }
    }
    s
}

/// An eye with a clear line to `target`: candidates on rings around it, the first whose ray reaches the target's box.
fn eye_for(cw: &dc_sim::CollisionWorld, target: V3, reach: f32) -> V3 {
    for dist in [260.0f32, 200.0, 340.0, 150.0, 420.0] {
        for k in 0..16 {
            let a = k as f32 * std::f32::consts::TAU / 16.0;
            for up in [80.0f32, 140.0, 40.0] {
                let eye = V3::new(
                    target.x + dist * a.sin(),
                    target.y + up,
                    target.z + dist * a.cos(),
                );
                let clear = match cw.ray(eye, target) {
                    None => true,
                    Some(h) => (1.0 - h.t) * (target - eye).len() < reach,
                };
                if clear {
                    return eye;
                }
            }
        }
    }
    V3::new(target.x, target.y + 200.0, target.z - 300.0)
}

#[test]
fn rom_drum_explosion_plays_its_world_effects() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let ctx = match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => c,
        Err(e) => {
            println!("skipped: no GPU adapter ({e})");
            return;
        }
    };
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut src = RomWorld::new(&rom, MAP);
    let Ok(mut world) = src.load(Some(GpuTarget {
        ctx: &ctx,
        renderer: &renderer,
    })) else {
        println!("skipped: {MAP} not in this ROM");
        return;
    };
    let info = dc_dserver::world::load_from_rom(
        &rom,
        &format!("Worlds\\{MAP}"),
        dc_dserver::world::NameForm::Listed,
        None,
    )
    .expect("server world");
    let (tables, _) = dc_dserver::rom_tables::RomTables::load(&rom);
    // a drum: a breakable whose death names a placed effect and an explosion object that names an impact effect
    let (drum_name, fx, expl) = info
        .destructibles
        .breakables
        .iter()
        .find_map(|b| {
            let fx = b.death.iter().find_map(|d| match d {
                DeathAction::Effect(n) => info
                    .effects
                    .iter()
                    .find(|e| e.name.eq_ignore_ascii_case(n))
                    .cloned(),
                _ => None,
            })?;
            let x = b.death.iter().find_map(|d| match d {
                DeathAction::Explode(i) => info.destructibles.explosions.get(*i).cloned(),
                _ => None,
            })?;
            x.impact_fx.as_ref()?;
            Some((b.name.clone(), fx, x))
        })
        .expect("a drum in the world file");
    let blast_ammo = tables
        .weapons
        .ammo
        .iter()
        .find_map(|(id, a)| {
            a.impact_fx
                .as_ref()
                .filter(|n| Some(n.as_bytes()) == expl.impact_fx.as_deref())
                .map(|_| *id)
        })
        .expect("the ammunition the explosion names");
    let blast_weapon = tables
        .weapons
        .weapons
        .iter()
        .find_map(|(w, r)| (r.ammo == Some(blast_ammo)).then_some(*w))
        .expect("its weapon");
    let drum_brush = info
        .brushes
        .iter()
        .find(|b| b.name.eq_ignore_ascii_case(&drum_name))
        .expect("the drum's brush");
    let trig = trig_for(&rom, &info.announce, "TSV");
    let mut set = server_set(&info, &tables, trig.as_deref());
    set.brushes
        .entry(drum_name.to_ascii_lowercase())
        .or_insert(0x3101);
    world.apply_objects(&set);
    let at = V3::new(drum_brush.pos[0], drum_brush.pos[1], drum_brush.pos[2]);
    let eye = eye_for(&world.collision, at, 60.0);
    let view = ShootTest::look_at(eye.to_array(), at.to_array());
    let camera = ViewSettings::default().camera(eye, &view, W, H);
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, W, H);
    let out = out_dir();
    let whole = target
        .render(
            &ctx,
            &renderer,
            world.scene.as_ref().unwrap(),
            &camera,
            None,
        )
        .unwrap();
    whole.save_png(&out.join("drum_whole.png")).unwrap();
    // the server breaks it: flags off, the placed-effect object, the blast record
    let mut broken = set.clone();
    broken
        .brushes
        .insert(drum_name.to_ascii_lowercase(), 0x0100);
    world.apply_objects(&broken);
    let floor = world
        .collision
        .ray(
            V3::new(expl.pos[0], expl.pos[1], expl.pos[2]),
            V3::new(expl.pos[0], expl.pos[1] - 500.0, expl.pos[2]),
        )
        .map(|h| world.surface_type(h.model, h.poly))
        .unwrap_or(0);
    let events = vec![
        FxEvent::World(WorldFx {
            name: String::from_utf8_lossy(&fx.fx).into_owned(),
            pos: fx.pos,
            rot: [0.0, 0.0, 0.0, 1.0],
            obj: None,
            value: 0,
        }),
        FxEvent::Record(ShotRecord {
            shooter: 0xFFFF,
            first: NO_OBJECT,
            weapon: blast_weapon,
            ammo: blast_ammo,
            from: expl.pos,
            to: expl.pos,
            normal: [0.0, 1.0, 0.0],
            surface: floor.min(255) as u8,
            clear: dc_dserver::gs::SHOT_MASK_MISS,
            own: false,
            mods: [0xff; 3],
        }),
    ];
    let mut stage = FxStage::open(Some(&rom));
    assert!(!stage.runner.library.is_empty(), "effect groups read");
    let fs = FrameState {
        fx_events: events,
        ..FrameState::default()
    };
    let cues = stage.frame(1000, &fs, &world);
    let st = stage.runner.stats;
    println!("drum: {st:?}, {} sound cues", cues.len());
    assert_eq!(st.world, 1, "the placed effect was handed over once");
    assert!(st.blasts >= 1, "the blast took the grenade path");
    assert!(!cues.is_empty(), "the explosion sound cue");
    assert!(
        st.groups_started >= 2,
        "the placed effect's group and the blast's groups started: {st:?}"
    );
    let scene = world.scene.as_ref().unwrap();
    let mut quads = 0usize;
    for (name, t) in [
        ("drum_explosion_100", 1100u32),
        ("drum_explosion_350", 1350),
        ("drum_explosion_700", 1700),
    ] {
        let _ = stage.frame(t, &FrameState::default(), &world);
        let img = target
            .render_with_fx(
                &ctx,
                &renderer,
                scene,
                &camera,
                None,
                &[],
                &[],
                Some((&mut stage, t, None)),
                None,
                None,
            )
            .unwrap();
        img.save_png(&out.join(format!("{name}.png"))).unwrap();
        quads = quads.max(stage.last.world.len());
        println!("{name}: {} world quads", stage.last.world.len());
    }
    assert!(quads > 0, "the effect quads were drawn");
    println!("PNGs in {} (ROM-derived, never commit)", out.display());
}
