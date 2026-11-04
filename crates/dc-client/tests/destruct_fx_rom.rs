//! ROM- and GPU-gated picture sequences of client-destructfx (skipped without CA_ROM or a GPU adapter): a crate and a
//! drum of JUNK_FLEA break. The world file's own data pick them (a breakable whose death command names a placed
//! effect only, and one that also names an explosion object); the frame stage gets exactly what the client receives
//! when the server breaks one (the brush's flags switched off, the placed-effect object at its world-file position and
//! rotation, and for the drum the type-0x06 blast record of the explosion object), on a fixed session clock. Asserted:
//! the crate's group starts its anchors, draws its pieces as models that move apart over the key's life and plays its
//! breaking sound; the drum's group launches the drum model upward with its fire column, and the blast takes the
//! grenade path. PNGs go to `reports/client-destructfx/` (ROM-derived, never committed); names
//! are never printed.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 (blast record), §8.4 (type 0x64); FMT_SPR_LTC_FX.md §5.7, §6.3
// DISASM: cshell+0x26e070 (the placed effect takes its object's position and rotation)
// EVIDENCE: clientfx+0x1cae0 / +0x1cd50 (anchors), clientfx+0x17290 (model keys), clientfx+0x20ec0 (sound keys)
// DC-DEFAULT: CLI-360..CLI-362 (proposed)

use dc_client::characters::ModelScene;
use dc_client::fx::{FxEvent, FxStage, ShotRecord, WorldFx, NO_OBJECT};
use dc_client::objects::ServerObjects;
use dc_client::offscreen::Offscreen;
use dc_client::world::{FrameState, GpuTarget, LoadedWorld};
use dc_client::{RomWorld, ShootTest, ViewSettings, WorldSource};
use dc_dserver::destructibles::DeathAction;
use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
use dc_sim::V3;

const W: u32 = 800;
const H: u32 = 600;
const MAP: &str = "JUNK_FLEA";
/// The session time of the break in these pictures.
const T0: u32 = 1000;

fn out_dir() -> std::path::PathBuf {
    let o = dc_render::default_out_dir().join("client-destructfx");
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
    for dist in [240.0f32, 300.0, 200.0, 360.0, 420.0] {
        for k in 0..16 {
            let a = k as f32 * std::f32::consts::TAU / 16.0;
            for up in [60.0f32, 110.0, 30.0] {
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

fn changed(a: &dc_render::FrameImage, b: &dc_render::FrameImage) -> usize {
    let n = a.rgba.len().min(b.rgba.len()) / 4;
    (0..n)
        .filter(|&k| (0..3).any(|i| a.rgba[k * 4 + i].abs_diff(b.rgba[k * 4 + i]) > 24))
        .count()
}

struct Rig {
    rom: std::path::PathBuf,
    ctx: GpuContext,
    renderer: WorldRenderer,
    world: LoadedWorld,
    info: dc_dserver::world::WorldInfo,
    tables: dc_dserver::rom_tables::RomTables,
    set: ServerObjects,
}

fn rig() -> Option<Rig> {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return None;
    };
    let ctx = match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => c,
        Err(e) => {
            println!("skipped: no GPU adapter ({e})");
            return None;
        }
    };
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut src = RomWorld::new(&rom, MAP);
    let Ok(world) = src.load(Some(GpuTarget {
        ctx: &ctx,
        renderer: &renderer,
    })) else {
        println!("skipped: {MAP} not in this ROM");
        return None;
    };
    let info = dc_dserver::world::load_from_rom(
        &rom,
        &format!("Worlds\\{MAP}"),
        dc_dserver::world::NameForm::Listed,
        None,
    )
    .expect("server world");
    let (tables, _) = dc_dserver::rom_tables::RomTables::load(&rom);
    let trig = trig_for(&rom, &info.announce, "TSV");
    let set = server_set(&info, &tables, trig.as_deref());
    Some(Rig {
        rom,
        ctx,
        renderer,
        world,
        info,
        tables,
        set,
    })
}

/// What one sequence measured.
#[derive(Debug, Default)]
struct Seq {
    /// Effect models drawn per picture.
    models: Vec<u32>,
    /// The first model's place per picture (`None`: no model).
    first: Vec<Option<glam::Vec3>>,
    /// Pixels that differ from the broken-model picture without effects.
    px: Vec<usize>,
    stats: dc_client::fx::FxStats,
    sound_files: Vec<String>,
}

/// Breaks the breakable `name` (`explosive`: a drum) and draws the pictures at the given milliseconds after the break.
fn sequence(r: &mut Rig, explosive: bool, tag: &str, at_ms: &[u32]) -> Seq {
    let b = r
        .info
        .destructibles
        .breakables
        .iter()
        .find(|b| {
            let fx = b.death.iter().any(|d| matches!(d, DeathAction::Effect(_)));
            let ex = b.death.iter().any(|d| matches!(d, DeathAction::Explode(_)));
            fx && ex == explosive
                && r.info.destructibles.shape(&b.name).is_some()
                && r.set.brushes.contains_key(&b.name.to_ascii_lowercase())
        })
        .cloned()
        .expect("a breakable of that kind the server creates");
    let fx = b
        .death
        .iter()
        .find_map(|d| match d {
            DeathAction::Effect(n) => r
                .info
                .effects
                .iter()
                .find(|e| e.name.eq_ignore_ascii_case(n))
                .cloned(),
            _ => None,
        })
        .expect("its placed effect");
    let expl = b.death.iter().find_map(|d| match d {
        DeathAction::Explode(i) => r.info.destructibles.explosions.get(*i).cloned(),
        _ => None,
    });
    let (lo, hi) = r
        .info
        .destructibles
        .shape(&b.name)
        .and_then(|s| s.models.first().map(|m| m.bounds()))
        .expect("its model bounds");
    let centre = (lo + hi) * 0.5;
    let name = b.name.to_ascii_lowercase();
    let mut set = r.set.clone();
    set.brushes.insert(name.clone(), 0x3101);
    r.world.apply_objects(&set);
    let eye = eye_for(&r.world.collision, centre, 0.6 * (hi - lo).len());
    let look_at = if explosive {
        // look a little above the drum: its group throws it upward
        centre + V3::new(0.0, 60.0, 0.0)
    } else {
        centre
    };
    let view = ShootTest::look_at(eye.to_array(), look_at.to_array());
    let camera = ViewSettings::default().camera(eye, &view, W, H);
    let target = Offscreen::new(&r.ctx, OFFSCREEN_FORMAT, W, H);
    let out = out_dir();
    let scene_whole = target
        .render(
            &r.ctx,
            &r.renderer,
            r.world.scene.as_ref().unwrap(),
            &camera,
            None,
        )
        .unwrap();
    scene_whole
        .save_png(&out.join(format!("{tag}_whole.png")))
        .unwrap();
    // the server breaks it: the flags change, the placed-effect object, for a drum the blast record
    let mut broken = set.clone();
    broken.brushes.insert(name, 0x0100);
    r.world.apply_objects(&broken);
    let reference = target
        .render(
            &r.ctx,
            &r.renderer,
            r.world.scene.as_ref().unwrap(),
            &camera,
            None,
        )
        .unwrap();
    let mut events = vec![FxEvent::World(WorldFx {
        name: String::from_utf8_lossy(&fx.fx).into_owned(),
        pos: fx.pos,
        rot: dc_dserver::worldobj::euler_quat(fx.rot),
        obj: None,
        value: 0,
    })];
    if let Some(x) = expl.as_ref().filter(|x| x.impact_fx.is_some()) {
        let ammo = r
            .tables
            .weapons
            .ammo
            .iter()
            .find_map(|(id, a)| {
                a.impact_fx
                    .as_ref()
                    .filter(|n| Some(n.as_bytes()) == x.impact_fx.as_deref())
                    .map(|_| *id)
            })
            .expect("the ammunition the explosion names");
        let weapon = r
            .tables
            .weapons
            .weapons
            .iter()
            .find_map(|(w, row)| (row.ammo == Some(ammo)).then_some(*w))
            .expect("its weapon");
        let floor = r
            .world
            .collision
            .ray(
                V3::new(x.pos[0], x.pos[1], x.pos[2]),
                V3::new(x.pos[0], x.pos[1] - 500.0, x.pos[2]),
            )
            .map(|h| r.world.surface_type(h.model, h.poly))
            .unwrap_or(0);
        events.push(FxEvent::Record(ShotRecord {
            shooter: 0xFFFF,
            first: NO_OBJECT,
            weapon,
            ammo,
            from: x.pos,
            to: x.pos,
            normal: [0.0, 1.0, 0.0],
            surface: floor.min(255) as u8,
            clear: dc_dserver::gs::SHOT_MASK_MISS,
            own: false,
            mods: [0xff; 3],
        }));
    }
    let mut stage = FxStage::open(Some(&r.rom));
    let mut models = ModelScene::open(&r.rom).expect("model scene");
    let fs = FrameState {
        fx_events: events,
        ..FrameState::default()
    };
    let mut seq = Seq::default();
    for c in stage.frame(T0, &fs, &r.world) {
        if let dc_client::sound::SoundCue::Server { file: Some(f), .. } = c {
            seq.sound_files.push(f);
        }
    }
    let lights = r.world.model_lights.clone();
    for &ms in at_ms {
        let t = T0 + ms;
        for c in stage.frame(t, &FrameState::default(), &r.world) {
            if let dc_client::sound::SoundCue::Server { file: Some(f), .. } = c {
                seq.sound_files.push(f);
            }
        }
        let list = stage.runner.models_at(t);
        models.update_fx_models(&r.ctx, &r.renderer, &list, lights.as_ref());
        seq.models.push(models.fx_models.drawn);
        seq.first.push(list.first().map(|m| m.pos));
        let inst = models.instances();
        let img = target
            .render_with_fx(
                &r.ctx,
                &r.renderer,
                r.world.scene.as_ref().unwrap(),
                &camera,
                None,
                &inst,
                &[],
                Some((&mut stage, t, None)),
                None,
                None,
            )
            .unwrap();
        img.save_png(&out.join(format!("{tag}_{ms:04}ms.png")))
            .unwrap();
        seq.px.push(changed(&img, &reference));
    }
    seq.stats = stage.runner.stats;
    // the next sequence starts from the server's set
    r.world.apply_objects(&r.set);
    seq
}

#[test]
fn rom_a_crate_breaks_into_its_pieces_and_a_drum_is_thrown_up() {
    let Some(mut r) = rig() else {
        return;
    };
    let at = [30, 100, 250, 500, 900, 1400, 1900];
    let c = sequence(&mut r, false, "crate", &at);
    println!(
        "crate: models {:?}, px {:?}, sounds {}, {:?}",
        c.models,
        c.px,
        c.sound_files.len(),
        c.stats
    );
    // before the model keys start (0.05 s) no piece, then all five pieces while they live
    assert_eq!(c.models[0], 0, "no piece before the model keys start");
    assert!(
        c.models[1..6].iter().all(|&n| n >= 5),
        "five pieces drawn: {:?}",
        c.models
    );
    assert!(c.stats.anchors >= 5, "five anchors: {:?}", c.stats);
    assert!(c.stats.sound_keys >= 1, "the breaking sound: {:?}", c.stats);
    assert!(
        c.sound_files
            .iter()
            .any(|f| f.to_ascii_uppercase().ends_with(".WAV")),
        "a wave cue"
    );
    let moved = c.first[1].zip(c.first[5]).map(|(a, b)| a.distance(b));
    assert!(moved.is_some_and(|d| d > 5.0), "a piece moved: {moved:?}");
    assert!(
        c.px[1..6].iter().all(|&p| p > 300),
        "the pieces show where the crate stood: {:?}",
        c.px
    );
    let d = sequence(&mut r, true, "drum", &[30, 100, 250, 500, 900, 1400, 1900]);
    println!(
        "drum: models {:?}, px {:?}, sounds {}, {:?}",
        d.models,
        d.px,
        d.sound_files.len(),
        d.stats
    );
    assert!(
        d.models[1..6].iter().all(|&n| n >= 1),
        "the drum model: {:?}",
        d.models
    );
    assert!(d.stats.blasts >= 1, "the blast record: {:?}", d.stats);
    // world-fixes2 (CLI-600): the 2009 rise of the drum's anchor is v^2 / (2 x 2 x 845) = 121..175 units
    let rise = d.first[1].zip(d.first[2]).map(|(a, b)| b.y - a.y);
    assert!(rise.is_some_and(|y| y > 30.0), "the drum flew up: {rise:?}");
    assert!((100..=190).contains(&d.stats.anchor_rise), "{:?}", d.stats);
    println!(
        "PNGs in {} (ROM-derived, never commit)",
        out_dir().display()
    );
}
