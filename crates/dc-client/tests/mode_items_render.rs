//! ROM- and GPU-gated headless pictures of w2-sprites (skipped without `CA_ROM` or a GPU adapter): on the first ROM
//! map whose world file has capture flags and scaled sprites,
//! - a capture flag (type 0x17 kind 2) and a HiSec case (kind 5, spin flag) are drawn with their models at the
//!   object's place, the case turning about its up axis (M23);
//! - a world sprite, as our server sends it (kind 3, the reference stack's flag words 0x0081 / 0x0002, the file's scale and
//!   colour), is one quad of the effect pass at its place, facing the camera from every side (m45);
//! - a weapon drop with 0 rounds hides its row's `ProjectilePiece` piece (m54).
//!
//! PNGs go to the local render output folder (ROM-derived, never committed); nothing of the ROM is asserted by value
//! and no ROM name is printed.
// SPEC: GAME_MODES_2009.md §4.4 (pickup kinds); UDP_SESSION_2009_B.md §3.3 (kind 3), §3.5.1 (sprites `cf` 0x049b,
// flags 0x0081 / 0x0002, REFERENCE)
// EVIDENCE: engine+0x1af730 (camera-facing sprite quad); DISASM: cshell+0x282490 (pickup init: piece hidden at 0
// rounds), cshell+0x282670 (spin)

use std::collections::BTreeMap;

use dc_botclient::objects::{kind, ClientObject};
use dc_client::characters::ModelScene;
use dc_client::drops::{
    ProjectilePieces, WeaponDrop, PICKUP_KIND_CASE, PICKUP_KIND_FLAG, PICKUP_KIND_WEAPON,
};
use dc_client::fx::FxStage;
use dc_client::offscreen::Offscreen;
use dc_client::sprites::BillboardTracker;
use dc_client::world::{FrameState, GpuTarget};
use dc_client::{ModelRef, RomWorld, ShootTest, ViewSettings, WeaponTable, WorldSource};
use dc_render::{
    AdapterChoice, Camera, FrameImage, GpuContext, ModelInstance, RenderSettings, WorldRenderer,
    OFFSCREEN_FORMAT,
};
use dc_sim::V3;
use glam::{Quat, Vec3};

const W: u32 = 480;
const H: u32 = 320;

fn diff_count(a: &FrameImage, b: &FrameImage) -> usize {
    let mut n = 0;
    for y in 0..a.height {
        for x in 0..a.width {
            let (p, q) = (a.pixel(x, y), b.pixel(x, y));
            let d: i32 = (0..3).map(|k| (p[k] as i32 - q[k] as i32).abs()).sum();
            n += (d > 12) as usize;
        }
    }
    n
}

fn camera_at(eye: Vec3, at: Vec3) -> Camera {
    let view = ShootTest::look_at(eye.to_array(), at.to_array());
    ViewSettings::default().camera(V3::new(eye.x, eye.y, eye.z), &view, W, H)
}

/// Eyes around `at` (four sides and above), `dist` away.
fn eyes(at: Vec3, dist: f32) -> Vec<Vec3> {
    [
        Vec3::new(0.0, 0.3, -1.0),
        Vec3::new(1.0, 0.3, 0.0),
        Vec3::new(0.0, 0.3, 1.0),
        Vec3::new(-1.0, 0.3, 0.0),
        Vec3::new(0.2, 1.0, 0.1),
    ]
    .into_iter()
    .map(|d| at + d.normalize() * dist)
    .collect()
}

#[test]
fn rom_mode_items_billboards_and_projectile_pieces_are_drawn() {
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
    let maps = RomWorld::map_names(&rom).expect("ROM world archive");
    // the first map whose world file has a flag with a model and a sprite that starts on
    let Some((map_i, map, info)) = maps.iter().enumerate().find_map(|(i, m)| {
        let info = dc_dserver::world::load_from_rom(
            &rom,
            &format!("Worlds\\{m}"),
            dc_dserver::world::NameForm::Listed,
            None,
        )
        .ok()?;
        (info.flags.iter().any(|f| f.model.is_some()) && info.sprites.iter().any(|s| s.start_on))
            .then(|| (i, m.clone(), info))
    }) else {
        println!("skipped: no ROM map with flags and sprites");
        return;
    };
    let mut src = RomWorld::new(&rom, &map);
    let world = src
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))
        .expect("ROM world");
    let scene = world.scene.as_ref().expect("scene");
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, W, H);
    let out = dc_render::default_out_dir()
        .join("dc-client")
        .join("w2-sprites");
    let _ = std::fs::create_dir_all(&out);
    println!(
        "map #{map_i}: {} flags, {} sprites",
        info.flags.len(),
        info.sprites.len()
    );
    let mut models = ModelScene::open(&rom).expect("ROM models");
    let lights = world.model_lights.as_ref();
    let render = |inst: &[&ModelInstance], cam: &Camera, stage: Option<(&mut FxStage, u32)>| {
        target
            .render_with_fx(
                &ctx,
                &renderer,
                scene,
                cam,
                None,
                inst,
                &[],
                stage.map(|(s, t)| (s, t, None)),
                None,
                None,
            )
            .unwrap()
    };

    // ---- M23: a capture flag and a HiSec case at their places
    let flag = info.flags.iter().find(|f| f.model.is_some()).unwrap();
    let flag_at = Vec3::from(flag.pos);
    let case_row = dc_dserver::rom_tables::HisecRows::from_doc(
        &dc_bute::ButeDoc::from_vfs(
            &dc_rez::RezVfs::new(&rom),
            "ATTRIBUTES.rez",
            dc_dserver::rom_tables::HISECS_ENTRY,
        )
        .expect("case table"),
    )
    .rows
    .into_values()
    .find(|r| r.model.is_some())
    .expect("a case row");
    let case_at = flag_at + Vec3::new(60.0, 0.0, 0.0);
    let item = |obj: u16, kind: u32, spin: bool, at: Vec3, model: ModelRef| WeaponDrop {
        obj,
        kind,
        weapon: 0,
        rounds: 0,
        spin,
        fall: false,
        pos: at.to_array(),
        rot: [0.0, 0.0, 0.0, 1.0],
        model: Some(model),
        trackers: Vec::new(),
        seen_ms: 0,
    };
    let items = vec![
        item(
            70,
            PICKUP_KIND_FLAG,
            false,
            flag_at,
            ModelRef {
                model: flag.model.clone().unwrap(),
                textures: Vec::new(),
            },
        ),
        item(
            71,
            PICKUP_KIND_CASE,
            true,
            case_at,
            ModelRef {
                model: case_row.model.clone().unwrap(),
                textures: case_row.skins.iter().take(1).cloned().collect(),
            },
        ),
    ];
    models.update_drops(&ctx, &renderer, &items, 500, None, lights);
    assert_eq!(models.drops_drawn(), 2, "both mode items have a model");
    let flag_inst = models.drop_models().instance(70).expect("flag instance");
    assert_eq!(flag_inst.position, flag_at, "the flag stands at its object");
    let case_inst = models.drop_models().instance(71).expect("case instance");
    assert_eq!(case_inst.position, case_at, "the case stands at its object");
    let turned = case_inst.rotation.angle_between(Quat::IDENTITY);
    assert!(turned > 0.5, "the case spins: {turned}");
    let mut best = 0usize;
    for (k, eye) in eyes(flag_at + Vec3::new(30.0, 40.0, 0.0), 220.0)
        .into_iter()
        .enumerate()
    {
        let cam = camera_at(eye, flag_at + Vec3::new(30.0, 30.0, 0.0));
        let plain = render(&[], &cam, None);
        let inst = models.instances();
        let with = render(&inst, &cam, None);
        let n = diff_count(&plain, &with);
        let _ = with.save_png(&out.join(format!("mode_items_{k}.png")));
        best = best.max(n);
    }
    println!("mode items: best {best} changed pixels");
    assert!(best > 50, "the mode items are visible: {best}");
    models.update_drops(&ctx, &renderer, &[], 600, None, lights);

    // ---- m45: a world sprite as our server sends it, drawn as a camera-facing quad
    let sprite = info.sprites.iter().find(|s| s.start_on).unwrap();
    let mut o = ClientObject::default();
    o.kind = kind::BILLBOARD;
    o.pos = Some(sprite.pos);
    o.rot = Some(dc_dserver::worldobj::euler_quat(sprite.rot));
    o.flags = dc_dserver::worldobj::SPRITE_FLAGS.flags;
    let f = dc_dserver::worldobj::SPRITE_FLAGS;
    o.flag_words = Some((f.flags, f.flags2, f.uflags, f.flags3));
    o.sprite_file = Some(1);
    o.scale = Some([sprite.scale[0], sprite.scale[1], 1.0]);
    o.color = dc_dserver::worldobj::sprite_color(sprite);
    let mut objects = BTreeMap::new();
    objects.insert(80u16, o);
    let file = sprite.file.clone();
    let list = BillboardTracker::default().collect(
        &objects,
        &|fid| (fid == 1).then(|| file.clone()),
        1000,
    );
    assert_eq!(list.len(), 1, "the sprite is a billboard");
    let at = Vec3::from(sprite.pos);
    let mut stage = FxStage::open(Some(&rom));
    let fs = FrameState {
        billboards: list,
        ..FrameState::default()
    };
    let mut best = 0usize;
    for (k, eye) in eyes(at, 150.0).into_iter().enumerate() {
        let cam = camera_at(eye, at);
        let _ = stage.frame(1200, &FrameState::default(), &world);
        let plain = render(&[], &cam, Some((&mut stage, 1200)));
        assert_eq!(stage.billboard_quads, 0);
        let _ = stage.frame(1200, &fs, &world);
        let with = render(&[], &cam, Some((&mut stage, 1200)));
        assert_eq!(stage.billboard_quads, 1, "one quad");
        let q = stage.last.world.last().expect("the billboard quad");
        assert_eq!(q.centre, at, "at the object's place");
        let n = q.u.cross(q.v).normalize();
        assert!(
            n.dot(cam.forward()).abs() > 0.9999,
            "faces the camera from eye {k}: normal {n:?}"
        );
        assert!(
            q.u.normalize().dot(cam.rot * Vec3::X) > 0.9999,
            "width along the camera's right"
        );
        let px = diff_count(&plain, &with);
        let _ = with.save_png(&out.join(format!("billboard_{k}.png")));
        best = best.max(px);
    }
    println!("billboard: best {best} changed pixels");
    assert!(best > 0, "the billboard changes the frame");
    // nearer than the cut along the view: not drawn
    let cam = camera_at(at - Vec3::Z * 5.0, at);
    let _ = stage.frame(1300, &fs, &world);
    let _ = render(&[], &cam, Some((&mut stage, 1300)));
    assert_eq!(stage.billboard_quads, 0, "near cut");

    // ---- m54: a weapon drop with 0 rounds hides its row's ProjectilePiece
    let table = WeaponTable::open(&rom).expect("weapon table");
    let pieces = ProjectilePieces::from_vfs(&dc_rez::RezVfs::new(&rom));
    println!("{} weapon rows name a projectile piece", pieces.len());
    let drop_at = flag_at + Vec3::new(0.0, 20.0, 0.0);
    let mut found = None;
    for (w, row) in table
        .rows
        .iter()
        .filter(|(w, _)| pieces.get(**w).is_some())
        .take(60)
    {
        let Some(m) = row.hh_model.clone() else {
            continue;
        };
        let d = WeaponDrop {
            obj: 90,
            kind: PICKUP_KIND_WEAPON,
            weapon: *w,
            rounds: 0,
            spin: false,
            fall: false,
            pos: drop_at.to_array(),
            rot: [0.0, 0.0, 0.0, 1.0],
            model: Some(ModelRef {
                model: m,
                textures: row.hh_skins.iter().take(1).cloned().collect(),
            }),
            trackers: Vec::new(),
            seen_ms: 0,
        };
        models.update_drops(&ctx, &renderer, std::slice::from_ref(&d), 700, None, lights);
        if models.drop_models().hidden == 1 {
            found = Some(d);
            break;
        }
        models.update_drops(&ctx, &renderer, &[], 710, None, lights);
    }
    let Some(mut d) = found else {
        println!("no weapon row's model has its projectile piece: piece test skipped");
        return;
    };
    let hidden = models
        .drop_models()
        .instance(90)
        .unwrap()
        .hidden_pieces()
        .to_vec();
    assert!(!hidden.is_empty());
    let cam = camera_at(drop_at + Vec3::new(25.0, 15.0, -40.0), drop_at);
    let empty = render(&models.instances(), &cam, None);
    d.rounds = 30;
    models.update_drops(&ctx, &renderer, std::slice::from_ref(&d), 720, None, lights);
    assert_eq!(
        models.drop_models().hidden,
        0,
        "with rounds nothing is hidden"
    );
    assert!(models
        .drop_models()
        .instance(90)
        .unwrap()
        .hidden_pieces()
        .is_empty());
    let loaded = render(&models.instances(), &cam, None);
    let _ = empty.save_png(&out.join("drop_no_rounds.png"));
    let _ = loaded.save_png(&out.join("drop_rounds.png"));
    let px = diff_count(&empty, &loaded);
    println!(
        "projectile piece: {} piece(s) hidden at 0 rounds, {px} pixels differ; PNGs in {} (ROM-derived, never commit)",
        hidden.len(),
        out.display()
    );
}
