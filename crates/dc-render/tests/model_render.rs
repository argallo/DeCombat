//! Synthetic model, sky and translucent renders on whatever headless adapter exists (hardware, else the
//! software fallback). Every model, world and texture here is invented in the test and written with dc-model's
//! writer; nothing comes from the ROM. Skips cleanly when no adapter exists.

use dc_model::{
    BlendSet, Clip, Lod, Marker, Mesh, Model, Node, Piece, PosKeys, RawStr, RigidMesh, RotKeys,
    SkinSpan, SkinTail, SkinnedMesh, Track, VertexStream, IDENTITY,
};
use dc_render::*;
use glam::{Quat, Vec3};

const W: u32 = 128;
const H: u32 = 96;

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

fn camera() -> Camera {
    let fx = 90f32.to_radians();
    Camera::looking(Vec3::ZERO, 0.0, 0.0, fx, fov_y_for(fx, W, H))
}

fn node(name: &str, x: f32, parent: Option<usize>, children: Vec<usize>) -> Node {
    let mut m = IDENTITY;
    m[3] = x;
    Node {
        name: RawStr::from(name),
        index: 0,
        flags: 0,
        model_xform: m,
        parent,
        children,
    }
}

/// One vertex of a stream with flags 0x13 (position, normal, one uv pair) and `weights` blend floats.
fn put_vertex(out: &mut Vec<u8>, p: [f32; 3], weights: &[f32], n: [f32; 3], uv: [f32; 2]) {
    for v in p.iter().chain(weights).chain(&n).chain(&uv) {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

/// Axis-aligned cube of half size `h` centred on `c`: 24 vertices (4 per face with the face normal), 12
/// triangles wound clockwise seen from outside.
fn cube_stream(c: [f32; 3], h: f32, weights: &[f32]) -> (Vec<u8>, Vec<u16>) {
    let mut data = Vec::new();
    let mut idx = Vec::new();
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
            let p = [0, 1, 2].map(|i| c[i] + h * (n[i] + su * u[i] + sv * v[i]));
            put_vertex(&mut data, p, weights, *n, [0.5, 0.5]);
        }
        idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (data, idx)
}

fn streams(data: Vec<u8>) -> [VertexStream; 4] {
    [
        VertexStream { flags: 0x13, data },
        VertexStream::default(),
        VertexStream::default(),
        VertexStream::default(),
    ]
}

fn lod(mesh: Mesh, slot: u32, used: Vec<u8>) -> Lod {
    Lod {
        slot_count: 1,
        slots: [slot, 1, 2, 3],
        style_index: 0,
        priority: 0,
        declared_body_len: 0,
        mesh,
        used_nodes: used,
    }
}

fn finish(mut m: Model) -> ModelFiles {
    m.radius = 30.0;
    m.clip_overrides = vec![Vec::new(); m.child_models.len() + 1];
    m.fix_body_lens();
    // through the writer and back: the renderer reads what the format carries
    let bytes = m.to_bytes().expect("synthetic model encodes");
    let parsed = Model::parse(&bytes).expect("synthetic model parses");
    ModelFiles::from_model("SYN\\cube.ltb", parsed)
}

/// A rigid cube of half size 10 at (0, 0, 100), on the root node.
fn rigid_cube() -> ModelFiles {
    let (data, idx) = cube_stream([0.0, 0.0, 100.0], 10.0, &[]);
    let mesh = Mesh::Rigid(RigidMesh {
        vertex_count: 24,
        tri_count: 12,
        todo_influences: 1,
        todo_unused: 1,
        node: 0,
        streams: streams(data),
        indices: idx,
    });
    finish(Model {
        pieces: vec![Piece {
            name: RawStr::from("cube"),
            lod_dist: vec![0.0],
            todo_lod_lo: 0,
            todo_lod_hi: 0,
            lods: vec![lod(mesh, 0, vec![0])],
        }],
        nodes: vec![node("root", 0.0, None, vec![])],
        ..Model::default()
    })
}

/// Two nodes (root, "arm" at x = 0); a skinned cube of half size 6 at (0, 0, 100) bound to the arm with weight
/// 1 (two influences: stored weight 0 for the root, the arm gets 1 - 0); one clip moving the arm from x = 0 to
/// x = 40 over 1000 ms; blend sets: set 0 weights only the arm.
fn skinned_cube() -> ModelFiles {
    let (data, idx) = cube_stream([0.0, 0.0, 100.0], 6.0, &[0.0]);
    let mesh = Mesh::Skinned(SkinnedMesh {
        vertex_count: 24,
        tri_count: 12,
        max_influences: 2,
        palette_influences: 2,
        has_palette_map: 0,
        streams: streams(data),
        indices: idx,
        tail: SkinTail::Spans(vec![SkinSpan {
            first_vertex: 0,
            vertex_count: 24,
            nodes: [0, 1, 0xff, 0xff],
            index_end: 36,
        }]),
    });
    let id = [0.0f32, 0.0, 0.0, 1.0];
    let track = |p0: [f32; 3], p1: [f32; 3]| Track::Keys {
        pos: PosKeys::F32(vec![p0, p1]),
        rot: RotKeys::F32(vec![id, id]),
    };
    let clip = Clip {
        extent: [1.0; 3],
        name: RawStr::from("slide"),
        codec: 0,
        blend_ms: 0,
        markers: [0u32, 1000]
            .iter()
            .map(|&t| Marker {
                time_ms: t,
                command: RawStr::default(),
            })
            .collect(),
        tracks: vec![track([0.0; 3], [0.0; 3]), track([0.0; 3], [40.0, 0.0, 0.0])],
    };
    finish(Model {
        pieces: vec![Piece {
            name: RawStr::from("cube"),
            lod_dist: vec![0.0],
            todo_lod_lo: 0,
            todo_lod_hi: 0,
            lods: vec![lod(mesh, 0, vec![0, 1])],
        }],
        nodes: vec![
            node("root", 0.0, None, vec![1]),
            node("arm", 0.0, Some(0), vec![]),
        ],
        blend_sets: vec![BlendSet {
            name: RawStr::from("arm only"),
            weights: vec![0.0, 1.0],
        }],
        clips: vec![clip],
        ..Model::default()
    })
}

fn solid(rgba: [u8; 4]) -> TextureImage {
    TextureImage::rgba(2, 2, rgba.repeat(4))
}

fn near(a: [u8; 4], b: [u8; 3], tol: i32) -> bool {
    (0..3).all(|k| (a[k] as i32 - b[k] as i32).abs() <= tol)
}

fn clear_rgb() -> [u8; 3] {
    let c = CLEAR_COLOUR.map(|c| (c * 255.0).round() as u8);
    [c[0], c[1], c[2]]
}

/// World vertex (vertex-lit recipe: T0 * D x2 with `saturate`; D = 128 grey keeps the texture colour).
fn wv(p: [f32; 3]) -> GpuVertex {
    GpuVertex {
        pos: p,
        colour: [128, 128, 128, 255],
        uv0: [0.5, 0.5],
        uv1: [0.5, 0.5],
    }
}

/// Appends a quad at depth `z` spanning x0..x1, y -hy..hy (clockwise on screen) and returns its batch.
fn quad(mesh: &mut SceneMesh, x0: f32, x1: f32, z: f32, hy: f32, tex: u32) -> Batch {
    let b = mesh.vertices.len() as u32;
    for p in [[x0, -hy, z], [x0, hy, z], [x1, hy, z], [x1, -hy, z]] {
        mesh.vertices.push(wv(p));
    }
    let first = mesh.indices.len() as u32;
    mesh.indices
        .extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    Batch {
        bucket: 1,
        pass: PassKind::VertexLit,
        tex0: Some(tex),
        tex1: None,
        lightmap: None,
        first_index: first,
        index_count: 6,
    }
}

fn upload(ctx: &GpuContext, r: &WorldRenderer, mesh: &SceneMesh) -> GpuScene {
    let colours: Vec<[u8; 4]> = vec![
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 255, 255],
    ];
    r.upload(ctx, mesh, &mut |name| {
        let i = name.last().map(|c| (c - b'0') as usize)?;
        colours.get(i).map(|c| solid(*c))
    })
}

fn textures(mesh: &mut SceneMesh) {
    mesh.textures = (0..4u8).map(|i| vec![b's', b'0' + i]).collect();
}

/// Expected vertex-lit colour of the cube's front-face centre: the four corner vertices are lit per vertex
/// (ambient + attenuation x max(N.L, 0) x colour, clamped) and the centre is their average.
fn expected_front(tex: [f32; 3], ambient: f32, light: &ModelLight) -> [u8; 3] {
    let n = Vec3::new(0.0, 0.0, -1.0);
    let mut sum = 0f32;
    for (x, y) in [(-10.0, -10.0), (-10.0, 10.0), (10.0, 10.0), (10.0, -10.0)] {
        let p = Vec3::new(x, y, 90.0);
        let v = light.pos - p;
        let a = light.attenuation(v.length_squared());
        let l = ambient / 255.0 + (light.colour.x / 255.0) * a * n.dot(v.normalize()).max(0.0);
        sum += l.min(1.0);
    }
    let lit = sum / 4.0;
    tex.map(|t| (t * lit).round() as u8)
}

#[test]
fn rigid_cube_lit_and_depth_tested_against_the_world() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let files = rigid_cube();
    let mesh = ModelMesh::build(&files.body);
    assert_eq!(
        (mesh.stats.rigid, mesh.stats.triangles, mesh.stats.skipped),
        (1, 12, 0)
    );
    let mut cube = r.upload_model(&ctx, &files, &[Some(solid([200, 100, 50, 255]))]);
    assert_eq!(cube.missing_textures, 0);
    // SPEC: RENDER_2009.md §6.3: a dynamic light at (0, 0, 50) radius 200, white, plus ambient 64
    let light = ModelLight::dynamic(Vec3::new(0.0, 0.0, 50.0), 200.0, Vec3::splat(255.0));
    cube.lights = LightSet::build(
        Vec3::new(0.0, 0.0, 100.0),
        cube.radius,
        None,
        &[],
        &[light],
        &ModelLightSettings::default(),
    );
    cube.lights.ambient = Vec3::splat(64.0);
    assert_eq!(cube.lights.lights.len(), 1);

    // the world: a white wall at z = 80 covering the left half (x < 0 at the cube's depth)
    let mut world = SceneMesh::default();
    textures(&mut world);
    let wall = quad(&mut world, -60.0, -2.0, 80.0, 40.0, 3);
    world.batches.push(wall);
    let scene = upload(&ctx, &r, &world);
    let (img, rep) = r
        .render_frame_offscreen(
            &ctx,
            &scene,
            &camera(),
            &FrameOptions::opaque_only(),
            &[&cube],
            W,
            H,
        )
        .expect("frame");
    if let Ok(dir) = std::env::var("DC_RENDER_OUT") {
        img.save_png(&std::path::Path::new(&dir).join("synthetic_model_cube.png"))
            .expect("png");
    }
    assert_eq!(rep.models, 1);
    // the cube's front face spans x in [-10, 10] at z = 90: pixels (0.5 +- 10/(2*90)) * W
    let cx = W / 2 + 4; // right of the wall's edge (x = -2 at z = 80 -> pixel 62.4)
    let cy = H / 2;
    let px = img.pixel(cx, cy);
    let want = expected_front([200.0, 100.0, 50.0], 64.0, &light);
    println!("cube {px:?} want {want:?}");
    assert!(near(px, want, 6), "cube front {px:?} want {want:?}");
    // left of centre: the wall at z = 80 is in front of the cube (depth test)
    let wallpx = img.pixel(W / 2 - 8, cy);
    assert!(near(wallpx, [255, 255, 255], 4), "wall {wallpx:?}");
    // outside both: clear colour
    assert!(near(img.pixel(W - 2, 2), clear_rgb(), 2));
}

#[test]
fn skinned_cube_follows_its_clip() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let files = skinned_cube();
    let mut m = r.upload_model(&ctx, &files, &[Some(solid([255, 255, 0, 255]))]);
    assert_eq!(m.mesh_stats.skinned, 1);
    m.lights.ambient = Vec3::splat(255.0); // full bright
    let scene = upload(&ctx, &r, &SceneMesh::default());
    let render = |m: &ModelInstance| {
        r.render_frame_offscreen(
            &ctx,
            &scene,
            &camera(),
            &FrameOptions::opaque_only(),
            &[m],
            W,
            H,
        )
        .expect("frame")
        .0
    };
    // bind pose / time 0: the cube at the centre
    m.set_pose(&[Tracker::main(0, 0.0)]);
    let a = render(&m);
    assert!(near(a.pixel(W / 2, H / 2), [255, 255, 0], 3));
    // halfway: the arm moved +20 in x: the cube centre is at x = 20 (pixel 0.5 + 20/(2*94) of W)
    m.set_pose(&[Tracker::main(0, 500.0)]);
    let b = render(&m);
    let moved = ((0.5 + 20.0 / (2.0 * 94.0)) * W as f32) as u32;
    assert!(near(b.pixel(moved, H / 2), [255, 255, 0], 3), "moved");
    assert!(
        near(b.pixel(W / 2 - 4, H / 2), clear_rgb(), 2),
        "left the centre"
    );
    // a weight set that masks the arm out keeps the bind pose: tracker with weight set 0 weights only the arm,
    // so build a pose where the main tracker is at 0 ms and the masked tracker at 1000 ms (arm at +40)
    m.set_pose(&[
        Tracker::main(0, 0.0),
        Tracker {
            clip: 0,
            time_ms: 999.0,
            blend_set: Some(0),
        },
    ]);
    let c = render(&m);
    let far = ((0.5 + 40.0 / (2.0 * 94.0)) * W as f32) as u32;
    assert!(
        near(c.pixel(far - 1, H / 2), [255, 255, 0], 3),
        "masked tracker moved the arm"
    );
    // the world transform moves it too
    m.set_pose(&[]);
    m.position = Vec3::new(0.0, 20.0, 0.0);
    m.rotation = Quat::IDENTITY;
    let d = render(&m);
    let up = ((0.5 - 20.0 / (2.0 * 94.0) * W as f32 / H as f32) * H as f32) as u32;
    assert!(
        near(d.pixel(W / 2, up), [255, 255, 0], 3),
        "world transform"
    );
}

#[test]
fn sky_pass_draws_only_into_the_portal_rect_behind_the_world() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut mesh = SceneMesh::default();
    textures(&mut mesh);
    // sky geometry: a big blue quad far away around the sky box centre (0, 0, 5000) seen from the sky eye
    let sky = quad(&mut mesh, -9000.0, 9000.0, 6000.0, 9000.0, 2);
    mesh.sky_batches.push(sky);
    mesh.sky_bounds = Some(([-9000.0, -9000.0, 4000.0], [9000.0, 9000.0, 6000.0]));
    // a red world wall on the left half, in front of everything
    let wall = quad(&mut mesh, -100.0, 0.0, 100.0, 100.0, 0);
    mesh.batches.push(wall);
    // the sky portal: the top-right quarter of the screen at depth 100
    mesh.sky_portals = vec![vec![
        [0.0, 0.0, 100.0],
        [100.0, 0.0, 100.0],
        [100.0, 100.0, 100.0],
        [0.0, 100.0, 100.0],
    ]];
    mesh.world_extents = Some(([-100.0; 3], [100.0; 3]));
    let scene = upload(&ctx, &r, &mesh);
    let cam = camera();
    let (img, rep) = r
        .render_frame_offscreen(&ctx, &scene, &cam, &FrameOptions::default(), &[], W, H)
        .expect("frame");
    if let Ok(dir) = std::env::var("DC_RENDER_OUT") {
        img.save_png(&std::path::Path::new(&dir).join("synthetic_sky.png"))
            .expect("png");
    }
    let rect = rep.sky_rect.expect("portal on screen");
    println!("sky rect {rect:?} eye {:?}", rep.sky_eye);
    assert_eq!(rep.sky_draws, 1);
    // the sky eye sits at the centre of the sky geometry (zero-size default view box)
    let eye = rep.sky_eye.unwrap();
    assert!((eye[2] - 5000.0).abs() < 1e-3);
    // inside the portal rect and right of the wall: blue sky
    assert!(near(img.pixel(W * 3 / 4, H / 4), [0, 0, 255], 4));
    // the wall covers the sky (depth cleared after the sky, world drawn over it)
    assert!(near(img.pixel(W / 4, H / 4), [255, 0, 0], 4));
    // outside the portal rect: clear colour, no sky
    assert!(near(img.pixel(W * 3 / 4, H * 3 / 4), clear_rgb(), 2));
    // sky off: clear colour in the portal
    let (img2, rep2) = r
        .render_frame_offscreen(
            &ctx,
            &scene,
            &cam,
            &FrameOptions {
                sky: None,
                ..FrameOptions::default()
            },
            &[],
            W,
            H,
        )
        .expect("frame");
    assert!(rep2.sky_rect.is_none());
    assert!(near(img2.pixel(W * 3 / 4, H / 4), clear_rgb(), 2));
}

#[test]
fn translucent_and_additive_world_models_blend_back_to_front() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut mesh = SceneMesh::default();
    textures(&mut mesh);
    // opaque blue wall behind everything
    let wall = quad(&mut mesh, -300.0, 300.0, 300.0, 300.0, 2);
    mesh.batches.push(wall);
    // left: translucent red at z = 100 (alpha 0.5) in front of translucent green at z = 200 (alpha 0.5); the
    // red one is listed first so only the back-to-front sort puts green underneath
    let red = quad(&mut mesh, -100.0, 0.0, 100.0, 50.0, 0);
    let green = quad(&mut mesh, -200.0, 0.0, 200.0, 100.0, 1);
    mesh.blended.push(BlendedModel {
        batches: vec![red],
        origin: [-50.0, 0.0, 100.0],
        additive: false,
        alpha: 0.5,
    });
    mesh.blended.push(BlendedModel {
        batches: vec![green],
        origin: [-100.0, 0.0, 200.0],
        additive: false,
        alpha: 0.5,
    });
    // right: additive red over the blue wall
    let add = quad(&mut mesh, 0.0, 100.0, 100.0, 50.0, 0);
    mesh.blended.push(BlendedModel {
        batches: vec![add],
        origin: [50.0, 0.0, 100.0],
        additive: true,
        alpha: 1.0,
    });
    let scene = upload(&ctx, &r, &mesh);
    let (img, rep) = r
        .render_frame_offscreen(&ctx, &scene, &camera(), &FrameOptions::default(), &[], W, H)
        .expect("frame");
    if let Ok(dir) = std::env::var("DC_RENDER_OUT") {
        img.save_png(&std::path::Path::new(&dir).join("synthetic_translucent.png"))
            .expect("png");
    }
    assert_eq!(rep.blended_objects, 3);
    // left: blue, then green at 0.5, then red at 0.5: (0.5*255, 0.25*255, 0.25*255)
    let left = img.pixel(W / 4 + 4, H / 2);
    assert!(near(left, [128, 64, 64], 6), "sorted blend {left:?}");
    // right: blue + red
    let right = img.pixel(W * 3 / 4 - 4, H / 2);
    assert!(near(right, [255, 0, 255], 4), "additive {right:?}");
    // without sorting the scene order draws red first, then green over it
    let (img2, _) = r
        .render_frame_offscreen(
            &ctx,
            &scene,
            &camera(),
            &FrameOptions {
                sort_translucent: false,
                ..FrameOptions::default()
            },
            &[],
            W,
            H,
        )
        .expect("frame");
    let unsorted = img2.pixel(W / 4 + 4, H / 2);
    assert!(near(unsorted, [64, 128, 64], 6), "scene order {unsorted:?}");
    // translucent pass off: only the wall
    let (img3, _) = r
        .render_frame_offscreen(
            &ctx,
            &scene,
            &camera(),
            &FrameOptions::opaque_only(),
            &[],
            W,
            H,
        )
        .expect("frame");
    assert!(near(img3.pixel(W / 4 + 4, H / 2), [0, 0, 255], 3));
}

/// render-core: the instance and bone uniforms are written only when they change and only up to the skeleton's
/// length. A pose rendered, changed, rendered again unchanged (no write), then changed back must give the same bytes
/// as a fresh instance rendered once in that pose, and the uniform byte counter shows the skipped writes.
#[test]
fn cached_model_uniforms_render_like_a_fresh_upload() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let files = skinned_cube();
    let tex = [Some(solid([255, 255, 0, 255]))];
    let scene = upload(&ctx, &r, &SceneMesh::default());
    let render = |m: &ModelInstance| {
        r.render_frame_offscreen(
            &ctx,
            &scene,
            &camera(),
            &FrameOptions::opaque_only(),
            &[m],
            W,
            H,
        )
        .expect("frame")
    };
    let mut m = r.upload_model(&ctx, &files, &tex);
    m.lights.ambient = Vec3::splat(255.0);
    m.set_pose(&[Tracker::main(0, 500.0)]);
    let (moved, r1) = render(&m);
    assert!(r1.uniform_bytes > 0, "first frame writes");
    let (again, r2) = render(&m);
    assert_eq!(r2.uniform_bytes, 0, "unchanged uniforms are not written");
    assert!(moved.rgba == again.rgba);
    m.set_pose(&[Tracker::main(0, 0.0)]);
    let (back, r3) = render(&m);
    assert!(r3.uniform_bytes > 0);
    assert!(
        r3.uniform_bytes < 16384,
        "only the skeleton's bones are written"
    );
    let mut fresh = r.upload_model(&ctx, &files, &tex);
    fresh.lights.ambient = Vec3::splat(255.0);
    fresh.set_pose(&[Tracker::main(0, 0.0)]);
    let (want, _) = render(&fresh);
    assert!(
        back.rgba == want.rgba,
        "cached writes differ from a fresh upload"
    );
    assert!(back.rgba != moved.rgba, "the pose change is drawn");
}

/// An instance of an uploaded model ([`ModelPipelines::instantiate`]) and a model uploaded on
/// another thread ([`ModelUploader`]) draw exactly what fresh uploads draw, alone and side by side, and an
/// instance's pose is its own (the template and the other instance keep theirs).
#[test]
fn instances_render_like_fresh_uploads() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let files = skinned_cube();
    let tex = [Some(solid([0, 200, 255, 255]))];
    let scene = upload(&ctx, &r, &SceneMesh::default());
    let render = |ms: &[&ModelInstance]| {
        r.render_frame_offscreen(
            &ctx,
            &scene,
            &camera(),
            &FrameOptions::opaque_only(),
            ms,
            W,
            H,
        )
        .expect("frame")
        .0
    };
    let place = |m: &mut ModelInstance, x: f32, t: f32| {
        m.lights.ambient = Vec3::splat(255.0);
        m.position = Vec3::new(x, 0.0, 0.0);
        m.set_pose(&[Tracker::main(0, t)]);
    };
    // the template comes from a loader-style thread
    let up = r.model_uploader(&ctx);
    let (files2, tex2) = (files.clone(), tex.clone());
    let template = std::thread::spawn(move || {
        up.upload_look(
            &files2,
            ModelLook {
                textures: &tex2,
                styles: StyleChoice::Plain,
            },
        )
    })
    .join()
    .expect("upload thread");
    let mut a = r.instantiate_model(&ctx, &template);
    let mut b = r.model_uploader(&ctx).instantiate(&template);
    place(&mut a, -20.0, 0.0);
    place(&mut b, 20.0, 700.0);
    let mut fa = r.upload_model(&ctx, &files, &tex);
    let mut fb = r.upload_model(&ctx, &files, &tex);
    place(&mut fa, -20.0, 0.0);
    place(&mut fb, 20.0, 700.0);
    let both = render(&[&a, &b]);
    assert!(
        both.rgba == render(&[&fa, &fb]).rgba,
        "two instances differ from two fresh uploads"
    );
    assert!(render(&[&a]).rgba == render(&[&fa]).rgba);
    assert!(render(&[&b]).rgba == render(&[&fb]).rgba);
    assert!(
        render(&[&a]).rgba != render(&[&b]).rgba,
        "the instances keep their own poses"
    );
    // the untouched template still draws like a fresh upload in the bind pose
    let fresh = r.upload_model(&ctx, &files, &tex);
    assert!(render(&[&template]).rgba == render(&[&fresh]).rgba);
    assert_eq!(a.draw_count(), fa.draw_count());
    assert_eq!(a.anim.clip_count(), fa.anim.clip_count());
}
