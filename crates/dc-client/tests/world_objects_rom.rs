//! ROM- and GPU-gated pictures of world-objects (skipped without CA_ROM or a GPU adapter): JUNK_FLEA as the file's
//! own properties draw it (the former dc-client behaviour), with the object set our server creates in an elimination
//! room, with the quarantine room's set, and in the elimination set with one breakable destroyed (its flags switched
//! off). The camera looks at the model that only the file-property view shows, then at the breakable. PNGs go to
//! `reports/world-objects/` (ROM-derived, never committed); nothing of the ROM is asserted by
//! value, names never printed (counts and hashes).
// SPEC: RENDER_2009.md §5.1 step 6; UDP_SESSION_2009_B.md §3.5.1; FMT_DAT85.md §6.4
// DISASM: engine+0xa2190 (kind-2 create binds by name)

use dc_client::objects::ServerObjects;
use dc_client::offscreen::Offscreen;
use dc_client::world::GpuTarget;
use dc_client::{RomWorld, ShootTest, ViewSettings, WorldSource};
use dc_render::{
    AdapterChoice, FrameImage, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT,
};
use dc_sim::V3;

const W: u32 = 800;
const H: u32 = 600;
const MAP: &str = "JUNK_FLEA";

fn out_dir() -> std::path::PathBuf {
    let o = dc_render::default_out_dir().join("world-objects");
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

fn diff_px(a: &FrameImage, b: &FrameImage) -> usize {
    let mut n = 0;
    for y in 0..a.height {
        for x in 0..a.width {
            let (p, q) = (a.pixel(x, y), b.pixel(x, y));
            if (0..3)
                .map(|k| (p[k] as i32 - q[k] as i32).abs())
                .sum::<i32>()
                > 24
            {
                n += 1;
            }
        }
    }
    n
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
fn rom_junk_flea_draws_the_server_set_of_the_mode() {
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
    let elim = server_set(
        &info,
        &tables,
        trig_for(&rom, &info.announce, "TSV").as_deref(),
    );
    let qrt = server_set(
        &info,
        &tables,
        trig_for(&rom, &info.announce, "QRT").as_deref(),
    );
    let file_drawn: Vec<Vec<u8>> = world
        .world_models
        .drawn()
        .into_iter()
        .map(<[u8]>::to_vec)
        .collect();
    // the models only the file-property view shows: drawn by the file, not created in elimination
    let extra: Vec<&Vec<u8>> = file_drawn
        .iter()
        .filter(|n| elim.brushes.get(*n).is_none_or(|f| f & 0x0001 == 0))
        .collect();
    println!(
        "{MAP}: {} linked models drawn by the file, {} in the elimination set, {} in the quarantine set, {} extra",
        file_drawn.len(),
        elim.brushes.len(),
        qrt.brushes.len(),
        extra.len()
    );
    assert!(
        !extra.is_empty(),
        "the file view shows models the server never creates in elimination"
    );
    let parsed_bytes = dc_render::WorldSource::open(&rom)
        .unwrap()
        .read(MAP)
        .unwrap()
        .unwrap();
    let parsed = dc_world::World::parse(&parsed_bytes).unwrap();
    let centre_of = |name: &[u8]| {
        let m = parsed
            .models
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(name))
            .expect("model");
        V3::new(
            0.5 * (m.box_min[0] + m.box_max[0]),
            0.5 * (m.box_min[1] + m.box_max[1]),
            0.5 * (m.box_min[2] + m.box_max[2]),
        )
    };
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, W, H);
    let shoot = |world: &dc_client::world::LoadedWorld, at: V3, eye: V3| {
        let view = ShootTest::look_at(eye.to_array(), at.to_array());
        let camera = ViewSettings::default().camera(eye, &view, W, H);
        target
            .render(
                &ctx,
                &renderer,
                world.scene.as_ref().unwrap(),
                &camera,
                None,
            )
            .unwrap()
    };
    let out = out_dir();
    let focus = centre_of(extra[0]);
    let eye = eye_for(&world.collision, focus, 120.0);
    let file_img = shoot(&world, focus, eye);
    world.apply_objects(&elim);
    let elim_img = shoot(&world, focus, eye);
    world.apply_objects(&qrt);
    let qrt_img = shoot(&world, focus, eye);
    // a breakable of the elimination set, destroyed
    world.apply_objects(&elim);
    let crate_name = elim
        .brushes
        .keys()
        .find(|n| {
            info.brushes.iter().any(|b| {
                b.name.eq_ignore_ascii_case(n)
                    && b.class == dc_dserver::world::BrushClass::Breakable
            })
        })
        .cloned()
        .expect("a breakable in the set");
    let c = centre_of(&crate_name);
    let ceye = eye_for(&world.collision, c, 60.0);
    let crate_whole = shoot(&world, c, ceye);
    let mut broken = elim.clone();
    broken.brushes.insert(crate_name.clone(), 0x0100);
    world.apply_objects(&broken);
    let crate_broken = shoot(&world, c, ceye);
    assert_eq!(world.world_models.state(&crate_name), Some((false, false)));
    for (n, img) in [
        ("junk_flea_file_props", &file_img),
        ("junk_flea_elimination_set", &elim_img),
        ("junk_flea_quarantine_set", &qrt_img),
        ("junk_flea_elimination_crate_whole", &crate_whole),
        ("junk_flea_elimination_crate_broken", &crate_broken),
    ] {
        img.save_png(&out.join(format!("{n}.png"))).unwrap();
    }
    let d_extra = diff_px(&file_img, &elim_img);
    let d_qrt = diff_px(&elim_img, &qrt_img);
    let d_crate = diff_px(&crate_whole, &crate_broken);
    println!(
        "pixels changed: file vs elimination {d_extra}, elimination vs quarantine {d_qrt}, crate whole vs broken {d_crate}; \
         PNGs in {}",
        out.display()
    );
    assert!(
        d_extra > 500,
        "the extra model is gone in the elimination set"
    );
    assert!(d_crate > 200, "the destroyed crate is gone");
}
