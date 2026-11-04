//! Synthetic player-view pass renders (RENDER_2009.md §4.2 / §4.3) on whatever headless adapter exists (hardware,
//! else the software fallback). Every model and texture is invented here and goes through dc-model's writer and
//! reader; nothing comes from the ROM. Skips cleanly when no adapter exists.

use dc_model::{Lod, Mesh, Model, Node, Piece, RawStr, RigidMesh, VertexStream, IDENTITY};
use dc_render::*;
use glam::{Quat, Vec3, Vec4};

const W: u32 = 160;
const H: u32 = 120;

fn context() -> Option<GpuContext> {
    match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => {
            println!("adapter: {}", c.describe());
            Some(c)
        }
        Err(e) => {
            println!("skipped: {e}");
            None
        }
    }
}

fn put_vertex(out: &mut Vec<u8>, p: [f32; 3], n: [f32; 3], uv: [f32; 2]) {
    for v in p.iter().chain(&n).chain(&uv) {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

/// Axis-aligned cube of half size `h` centred on the model origin, wound clockwise seen from outside.
fn cube_files(h: f32) -> ModelFiles {
    let mut data = Vec::new();
    let mut idx: Vec<u16> = Vec::new();
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, 1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
    ];
    for (k, (n, u, v)) in faces.iter().enumerate() {
        let base = (k * 4) as u16;
        for (su, sv) in [(-1.0, -1.0), (-1.0, 1.0), (1.0, 1.0), (1.0, -1.0)] {
            let p = [0, 1, 2].map(|i| h * (n[i] + su * u[i] + sv * v[i]));
            put_vertex(&mut data, p, *n, [0.5, 0.5]);
        }
        idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    let vertex_count = (data.len() / 32) as u32;
    let mesh = Mesh::Rigid(RigidMesh {
        vertex_count,
        tri_count: idx.len() as u32 / 3,
        todo_influences: 1,
        todo_unused: 1,
        node: 0,
        streams: [
            VertexStream { flags: 0x13, data },
            VertexStream::default(),
            VertexStream::default(),
            VertexStream::default(),
        ],
        indices: idx,
    });
    let mut m = Model {
        pieces: vec![Piece {
            name: RawStr::from("cube"),
            lod_dist: vec![0.0],
            todo_lod_lo: 0,
            todo_lod_hi: 0,
            lods: vec![Lod {
                slot_count: 1,
                slots: [0, 1, 2, 3],
                style_index: 0,
                priority: 0,
                declared_body_len: 0,
                mesh,
                used_nodes: vec![0],
            }],
        }],
        nodes: vec![Node {
            name: RawStr::from("root"),
            index: 0,
            flags: 0,
            model_xform: IDENTITY,
            parent: None,
            children: vec![],
        }],
        ..Model::default()
    };
    m.radius = 2.0;
    m.clip_overrides = vec![Vec::new(); m.child_models.len() + 1];
    m.fix_body_lens();
    let bytes = m.to_bytes().expect("synthetic model encodes");
    ModelFiles::from_model(
        "SYN\\cube.ltb",
        Model::parse(&bytes).expect("synthetic model parses"),
    )
}

fn solid(rgba: [u8; 4]) -> TextureImage {
    TextureImage::rgba(2, 2, rgba.repeat(4))
}

fn near(a: [u8; 4], b: [u8; 3], tol: i32) -> bool {
    (0..3).all(|k| (a[k] as i32 - b[k] as i32).abs() <= tol)
}

/// Full-bright light set: ambient 255, no lights (the model keeps its texture colour).
fn full_bright() -> LightSet {
    LightSet {
        ambient: Vec3::splat(255.0),
        lights: Vec::new(),
    }
}

/// A world wall: one white quad at eye depth `z` (camera at the origin looking along +Z) covering the view.
fn wall_scene(ctx: &GpuContext, r: &WorldRenderer, z: f32) -> GpuScene {
    let mut mesh = SceneMesh {
        textures: vec![b"w0".to_vec()],
        ..SceneMesh::default()
    };
    let s = z * 4.0;
    for p in [[-s, -s, z], [-s, s, z], [s, s, z], [s, -s, z]] {
        mesh.vertices.push(GpuVertex {
            pos: p,
            colour: [128, 128, 128, 255],
            uv0: [0.5, 0.5],
            uv1: [0.5, 0.5],
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    mesh.batches.push(Batch {
        bucket: 1,
        pass: PassKind::VertexLit,
        tex0: Some(0),
        tex1: None,
        lightmap: None,
        first_index: 0,
        index_count: 6,
    });
    r.upload(ctx, &mesh, &mut |_| Some(solid([255, 255, 255, 255])))
}

fn world_camera(pos: Vec3, yaw: f32) -> Camera {
    let fx = 90f32.to_radians();
    Camera::looking(pos, yaw, 0.0, fx, fov_y_for(fx, W, H))
}

/// Pixel of an eye-space point through the player-view projection.
fn pv_pixel(pv: &PlayerViewSettings, p: Vec3) -> (u32, u32) {
    let c = pv.projection(W, H) * Vec4::new(p.x, p.y, p.z, 1.0);
    let x = (c.x / c.w * 0.5 + 0.5) * W as f32;
    let y = (0.5 - c.y / c.w * 0.5) * H as f32;
    (x as u32, y as u32)
}

fn save(img: &FrameImage, name: &str) {
    if let Ok(dir) = std::env::var("DC_RENDER_OUT") {
        let _ = img.save_png(&std::path::Path::new(&dir).join(name));
    }
}

const RED: [u8; 3] = [220, 40, 30];

#[test]
fn view_model_draws_in_the_lower_half_and_wins_against_the_world() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    // the world: a white wall 8 units ahead (just past the world near plane of 7, so it is drawn); without the
    // depth split its window depth (~0.2) would beat the view model's (~0.97 at 2 units with near 0.1, far 7)
    let scene = wall_scene(&ctx, &r, 8.0);
    let mut cube = r.upload_model(&ctx, &cube_files(0.25), &[Some(solid([220, 40, 30, 255]))]);
    cube.lights = full_bright();
    // eye space: 2 units ahead, 0.5 below the eye
    let at = Vec3::new(0.0, -0.5, 2.0);
    cube.position = at;
    cube.rotation = Quat::IDENTITY;
    let pv = PLAYER_VIEW_DEFAULT;
    let camera = world_camera(Vec3::ZERO, 0.0);
    let (img, rep) = r
        .render_frame_offscreen_with_view(
            &ctx,
            &scene,
            &camera,
            &FrameOptions::opaque_only(),
            &[],
            &[&cube],
            &pv,
            W,
            H,
        )
        .expect("frame");
    save(&img, "playerview_wall.png");
    assert_eq!((rep.models, rep.view_models), (0, 1));
    let (cx, cy) = pv_pixel(&pv, at);
    assert!(
        cy > H / 2,
        "the view model's centre projects into the lower half ({cx}, {cy})"
    );
    let px = img.pixel(cx, cy);
    assert!(near(px, RED, 8), "view model over the wall {px:?}");
    // every red pixel is in the lower half; the upper half shows the wall
    let mut red = 0;
    for y in 0..H {
        for x in 0..W {
            if near(img.pixel(x, y), RED, 8) {
                assert!(y >= H / 2, "red pixel in the upper half at ({x}, {y})");
                red += 1;
            }
        }
    }
    assert!(red > 50, "view model covers {red} pixels");
    assert!(near(img.pixel(W / 2, 4), [255, 255, 255], 4), "wall above");
}

#[test]
fn view_model_is_eye_relative_and_escapes_the_world_near_plane() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let scene = wall_scene(&ctx, &r, 1000.0);
    let pv = PLAYER_VIEW_DEFAULT;
    let at = Vec3::new(0.0, -0.4, 2.0);
    // a camera somewhere else, turned: the view model's place on screen does not change (VIEW = identity)
    let camera = world_camera(Vec3::new(120.0, 40.0, -60.0), 0.7);
    let mut cube = r.upload_model(&ctx, &cube_files(0.25), &[Some(solid([220, 40, 30, 255]))]);
    cube.lights = full_bright();
    cube.position = at;
    let (img, _) = r
        .render_frame_offscreen_with_view(
            &ctx,
            &scene,
            &camera,
            &FrameOptions::opaque_only(),
            &[],
            &[&cube],
            &pv,
            W,
            H,
        )
        .expect("frame");
    let (cx, cy) = pv_pixel(&pv, at);
    assert!(near(img.pixel(cx, cy), RED, 8), "eye-relative view model");
    // the same model 2 units from the eye drawn in the world pass is removed by the world near plane (7)
    cube.position = camera.world_from_eye().transform_point3(at);
    cube.rotation = camera.rot;
    let (img2, rep2) = r
        .render_frame_offscreen(
            &ctx,
            &scene,
            &camera,
            &FrameOptions::opaque_only(),
            &[&cube],
            W,
            H,
        )
        .expect("frame");
    save(&img2, "playerview_world_pass_clipped.png");
    assert_eq!((rep2.models, rep2.view_models), (1, 0));
    let mut red = 0;
    for y in 0..H {
        for x in 0..W {
            red += near(img2.pixel(x, y), RED, 8) as u32;
        }
    }
    assert_eq!(red, 0, "the world near plane clips a model 2 units ahead");
}

#[test]
fn empty_view_list_draws_nothing() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let scene = wall_scene(&ctx, &r, 50.0);
    let camera = world_camera(Vec3::ZERO, 0.0);
    let opts = FrameOptions::opaque_only();
    let (a, ra) = r
        .render_frame_offscreen(&ctx, &scene, &camera, &opts, &[], W, H)
        .expect("frame");
    let (b, rb) = r
        .render_frame_offscreen_with_view(
            &ctx,
            &scene,
            &camera,
            &opts,
            &[],
            &[],
            &PLAYER_VIEW_DEFAULT,
            W,
            H,
        )
        .expect("frame");
    assert_eq!((ra.view_models, rb.view_models), (0, 0));
    assert!(a.rgba == b.rgba, "an empty view list changes nothing");
}

#[test]
fn view_model_lighting_matches_the_world_pass() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let scene = wall_scene(&ctx, &r, 5000.0);
    let camera = world_camera(Vec3::new(30.0, 10.0, -20.0), 0.4);
    // the player-view camera made equal to the world camera's frustum (same vertical field, deep far plane), so
    // one cube 10 units ahead can be drawn by either pass
    let pv = PlayerViewSettings {
        fov_y_deg: camera.fov_y.to_degrees(),
        near: 0.1,
        far: 50.0,
    };
    let at = Vec3::new(0.0, 0.0, 10.0);
    let world_at = camera.world_from_eye().transform_point3(at);
    // a world light up and to the left of the cube, plus a dim ambient
    let light = ModelLight::dynamic(
        camera
            .world_from_eye()
            .transform_point3(Vec3::new(-3.0, 3.0, 6.0)),
        40.0,
        Vec3::splat(255.0),
    );
    let mut lights = LightSet::build(
        world_at,
        2.0,
        None,
        &[],
        &[light],
        &ModelLightSettings::default(),
    );
    lights.ambient = Vec3::splat(40.0);
    let files = cube_files(1.0);
    let tex = [Some(solid([200, 200, 200, 255]))];
    let mut a = r.upload_model(&ctx, &files, &tex);
    a.lights = lights.clone();
    a.position = world_at;
    a.rotation = camera.rot;
    let mut b = r.upload_model(&ctx, &files, &tex);
    b.lights = lights;
    b.position = at;
    let opts = FrameOptions::opaque_only();
    let (ia, _) = r
        .render_frame_offscreen(&ctx, &scene, &camera, &opts, &[&a], W, H)
        .expect("frame");
    let (ib, rb) = r
        .render_frame_offscreen_with_view(&ctx, &scene, &camera, &opts, &[], &[&b], &pv, W, H)
        .expect("frame");
    assert_eq!(rb.view_models, 1);
    let (cx, cy) = (W / 2, H / 2);
    let (pa, pb) = (ia.pixel(cx, cy), ib.pixel(cx, cy));
    println!("world pass {pa:?}, player view {pb:?}");
    assert!(
        pa[0] > 50 && near(pb, [pa[0], pa[1], pa[2]], 3),
        "same lighting {pa:?} {pb:?}"
    );
}
