//! Synthetic render-style, LOD and socket renders on whatever headless adapter exists (hardware, else the
//! software fallback). Every model, style file and texture is invented here and goes through dc-model's writer
//! and reader (style files: `StyleFile::to_bytes` -> `StyleFile::parse` -> `RenderStyle::from_file`); nothing
//! comes from the ROM. Skips cleanly when no adapter exists.

use dc_model::{
    Clip, Lod, Marker, Mesh, Model, Node, Piece, PosKeys, RawStr, RigidMesh, RotKeys, Socket,
    StyleFile, StyleMaterial, StylePass, StyleVariant, Track, VertexStream, IDENTITY,
};
use dc_render::*;
use glam::{Quat, Vec3};
use std::sync::Arc;

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

fn put_vertex(out: &mut Vec<u8>, p: [f32; 3], n: [f32; 3], uv: [f32; 2]) {
    for v in p.iter().chain(&n).chain(&uv) {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

/// Axis-aligned cube of half size `h` centred on `c`, 12 triangles wound clockwise seen from outside
/// (Direct3D front faces).
fn cube_stream(c: [f32; 3], h: f32) -> (Vec<u8>, Vec<u16>) {
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
            put_vertex(&mut data, p, *n, [0.5, 0.5]);
        }
        idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (data, idx)
}

fn rigid(data: Vec<u8>, idx: Vec<u16>, node: u32) -> Mesh {
    let n = (data.len() / 32) as u32;
    Mesh::Rigid(RigidMesh {
        vertex_count: n,
        tri_count: idx.len() as u32 / 3,
        todo_influences: 1,
        todo_unused: 1,
        node,
        streams: [
            VertexStream { flags: 0x13, data },
            VertexStream::default(),
            VertexStream::default(),
            VertexStream::default(),
        ],
        indices: idx,
    })
}

fn lod(mesh: Mesh, style: u32, used: Vec<u8>) -> Lod {
    Lod {
        slot_count: 1,
        slots: [0, 1, 2, 3],
        style_index: style,
        priority: 0,
        declared_body_len: 0,
        mesh,
        used_nodes: used,
    }
}

fn finish(mut m: Model, path: &str) -> ModelFiles {
    m.radius = 30.0;
    m.clip_overrides = vec![Vec::new(); m.child_models.len() + 1];
    m.fix_body_lens();
    let bytes = m.to_bytes().expect("synthetic model encodes");
    let parsed = Model::parse(&bytes).expect("synthetic model parses");
    ModelFiles::from_model(path, parsed)
}

/// A rigid cube of half size `h` at (0, 0, 100) on the root node, style slot 0.
fn cube(h: f32) -> ModelFiles {
    let (data, idx) = cube_stream([0.0, 0.0, 100.0], h);
    finish(
        Model {
            pieces: vec![Piece {
                name: RawStr::from("cube"),
                lod_dist: vec![0.0],
                todo_lod_lo: 0,
                todo_lod_hi: 0,
                lods: vec![lod(rigid(data, idx, 0), 0, vec![0])],
            }],
            nodes: vec![node("root", 0.0, None, vec![])],
            ..Model::default()
        },
        "SYN\\cube.ltb",
    )
}

/// A single quad facing the camera at z = 100 (half size 10), wound counter-clockwise on screen when `ccw`.
fn quad_model(ccw: bool) -> ModelFiles {
    let mut data = Vec::new();
    for p in [
        [-10.0, -10.0, 100.0],
        [-10.0, 10.0, 100.0],
        [10.0, 10.0, 100.0],
        [10.0, -10.0, 100.0],
    ] {
        put_vertex(&mut data, p, [0.0, 0.0, -1.0], [0.5, 0.5]);
    }
    let idx = if ccw {
        vec![0, 2, 1, 0, 3, 2]
    } else {
        vec![0, 1, 2, 0, 2, 3]
    };
    finish(
        Model {
            pieces: vec![Piece {
                name: RawStr::from("quad"),
                lod_dist: vec![0.0],
                todo_lod_lo: 0,
                todo_lod_hi: 0,
                lods: vec![lod(rigid(data, idx, 0), 0, vec![0])],
            }],
            nodes: vec![node("root", 0.0, None, vec![])],
            ..Model::default()
        },
        "SYN\\quad.ltb",
    )
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

/// A pass record in the engine's own values: stage 0 = piece texture 0 with `colour_op` (engine value) on
/// (TEXTURE, DIFFUSE), alpha MODULATE(TEXTURE, DIFFUSE), trilinear wrap; no blending, depth test + write with
/// LESSEQUAL, no culling, lighting on, solid.
fn pass(colour_op: u32) -> StylePass {
    let mut p = StylePass::default();
    let s = &mut p.stages[0];
    s.texture = 1;
    s.colour_op = colour_op;
    s.colour_arg1 = 2;
    s.colour_arg2 = 1;
    s.alpha_op = 3;
    s.alpha_arg1 = 2;
    s.alpha_arg2 = 1;
    s.filter = 2;
    s.transform_count = 2;
    p.texture_factor = 0x8080_8080;
    p.alpha_reference = 128;
    p.lighting_word = 1;
    p.depth_compare = 2;
    p.fill = 1;
    p
}

fn white_material() -> StyleMaterial {
    StyleMaterial {
        ambient: [1.0; 4],
        diffuse: [1.0; 4],
        emissive: [0.0, 0.0, 0.0, 1.0],
        specular: [0.0, 0.0, 0.0, 1.0],
        power: 0.0,
    }
}

/// Builds a style file in memory, writes it, reads it back and translates it.
fn style(material: StyleMaterial, passes: Vec<StylePass>) -> Arc<RenderStyle> {
    let mut v = StyleVariant {
        len_word: 0,
        material,
        passes,
        options: Some([0; 8]),
    };
    v.len_word = StyleFile::variant_len(&v);
    let f = StyleFile {
        header_pad: 0,
        header_rest: [0; 16],
        data_len: v.len_word,
        variants: vec![v],
        trailing: 0,
    };
    let bytes = f.to_bytes().expect("style encodes");
    let back = StyleFile::parse(&bytes).expect("style parses");
    Arc::new(RenderStyle::from_file(&back, "SYN\\style.ltb").expect("style translates"))
}

struct Bench {
    ctx: GpuContext,
    r: WorldRenderer,
    scene: GpuScene,
}

impl Bench {
    fn new() -> Option<Bench> {
        let ctx = context()?;
        let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
        let scene = r.upload(&ctx, &SceneMesh::default(), &mut |_| None);
        Some(Bench { ctx, r, scene })
    }

    fn model(
        &self,
        files: &ModelFiles,
        tex: [u8; 4],
        styles: &[Option<Arc<RenderStyle>>],
    ) -> ModelInstance {
        self.r.upload_model_look(
            &self.ctx,
            files,
            ModelLook {
                textures: &[Some(solid(tex))],
                styles: StyleChoice::List(styles),
            },
        )
    }

    fn render(&self, models: &[&ModelInstance]) -> FrameImage {
        self.r
            .render_frame_offscreen(
                &self.ctx,
                &self.scene,
                &camera(),
                &FrameOptions::opaque_only(),
                models,
                W,
                H,
            )
            .expect("frame")
            .0
    }

    fn centre(&self, m: &ModelInstance) -> [u8; 4] {
        self.render(&[m]).pixel(W / 2, H / 2)
    }
}

fn lit(m: &mut ModelInstance, ambient: f32, lights: Vec<ModelLight>) {
    m.lights = LightSet {
        ambient: Vec3::splat(ambient),
        lights,
    };
}

#[test]
fn modulate2x_doubles_the_lit_texture() {
    let Some(b) = Bench::new() else {
        return;
    };
    let tex = [100, 60, 20, 255];
    let one = style(white_material(), vec![pass(3)]);
    let two = style(white_material(), vec![pass(4)]);
    let mut m1 = b.model(&cube(10.0), tex, &[Some(one)]);
    let mut m2 = b.model(&cube(10.0), tex, &[Some(two)]);
    assert_eq!(m2.style_stats.stage0_ops.get("Modulate2x"), Some(&1));
    lit(&mut m1, 100.0, vec![]);
    lit(&mut m2, 100.0, vec![]);
    let a = b.centre(&m1);
    let c = b.centre(&m2);
    // ambient 100 / 255 x texture, then x 2 for MODULATE2X
    let k = 100.0 / 255.0;
    let want1 = [100.0 * k, 60.0 * k, 20.0 * k].map(|v: f32| v.round() as u8);
    let want2 = [200.0 * k, 120.0 * k, 40.0 * k].map(|v: f32| v.round() as u8);
    println!("modulate {a:?} want {want1:?}; modulate2x {c:?} want {want2:?}");
    assert!(near(a, want1, 3), "modulate {a:?}");
    assert!(near(c, want2, 3), "modulate2x {c:?}");
}

#[test]
fn material_ambient_emissive_and_unlit_passes() {
    let Some(b) = Bench::new() else {
        return;
    };
    let tex = [200, 100, 50, 255];
    // emissive 0.2 + material ambient 0.5 x ambient 100/255
    let mut mat = white_material();
    mat.ambient = [0.5, 0.5, 0.5, 1.0];
    mat.emissive = [0.2, 0.2, 0.2, 1.0];
    let s = style(mat, vec![pass(3)]);
    let mut m = b.model(&cube(10.0), tex, &[Some(s)]);
    lit(&mut m, 100.0, vec![]);
    let px = b.centre(&m);
    let l = 0.2 + 0.5 * 100.0 / 255.0;
    let want = [200.0 * l, 100.0 * l, 50.0 * l].map(|v: f32| v.round() as u8);
    println!("emissive + ambient {px:?} want {want:?}");
    assert!(near(px, want, 3));
    // lighting off: the diffuse is white, the texture is drawn as stored whatever the ambient
    let mut p = pass(3);
    p.lighting_word = 0;
    let s = style(white_material(), vec![p]);
    let mut m = b.model(&cube(10.0), tex, &[Some(s)]);
    lit(&mut m, 10.0, vec![]);
    let px = b.centre(&m);
    println!("unlit {px:?}");
    assert!(near(px, [200, 100, 50], 2));
}

#[test]
fn engine_default_style_for_an_empty_slot() {
    let Some(b) = Bench::new() else {
        return;
    };
    let tex = [255, 255, 255, 255];
    // a directional light along +Z hits the cube's front face (normal -Z) with N.L = 1
    let sun = ModelLight::sun(Vec3::Z, Vec3::splat(100.0));
    let mut plain = b.r.upload_model(&b.ctx, &cube(10.0), &[Some(solid(tex))]);
    let mut dflt = b.model(&cube(10.0), tex, &[None]);
    assert_eq!(dflt.style_stats.default_pieces, 1);
    lit(&mut plain, 100.0, vec![sun]);
    lit(&mut dflt, 100.0, vec![sun]);
    let p = b.centre(&plain);
    let d = b.centre(&dflt);
    // plain: (100 + 100) / 255; default: material ambient 1 x 100/255 + diffuse 0.8 x 100/255
    let wp = (200.0f32 / 255.0 * 255.0).round() as u8;
    let wd = ((100.0 + 80.0) / 255.0f32 * 255.0).round() as u8;
    println!("plain {p:?} want {wp}; engine default {d:?} want {wd}");
    assert!(near(p, [wp; 3], 3));
    assert!(near(d, [wd; 3], 3));
}

#[test]
fn alpha_test_and_additive_second_pass() {
    let Some(b) = Bench::new() else {
        return;
    };
    // texture alpha 64: GREATER 128 discards it; without the test the cube is drawn
    let tex = [180, 180, 180, 64];
    let mut p = pass(3);
    p.lighting_word = 0;
    p.alpha_test = 3;
    let tested = style(white_material(), vec![p]);
    let mut m = b.model(&cube(10.0), tex, &[Some(tested)]);
    lit(&mut m, 255.0, vec![]);
    let px = b.centre(&m);
    println!("alpha tested {px:?}");
    assert!(near(px, clear_rgb(), 2), "alpha test kept the pixel");
    p.alpha_test = 0;
    let open = style(white_material(), vec![p]);
    let mut m = b.model(&cube(10.0), tex, &[Some(open)]);
    lit(&mut m, 255.0, vec![]);
    assert!(near(b.centre(&m), [180, 180, 180], 2));
    // pass 2: SELECTARG1(TFACTOR 0x303030) added with ONE / ONE over the first pass, depth test only
    let mut base = pass(3);
    base.lighting_word = 0;
    let mut add = pass(2);
    add.stages[0].colour_arg1 = 3; // TFACTOR
    add.stages[0].colour_op = 1; // SELECTARG1
    add.texture_factor = 0xff30_3030;
    add.blend = 1; // ONE, ONE
    add.depth_mode = 1;
    let two = style(white_material(), vec![base, add]);
    let mut m = b.model(&cube(10.0), [100, 100, 100, 255], &[Some(two)]);
    assert_eq!(m.pass_count(), 2);
    lit(&mut m, 255.0, vec![]);
    let px = b.centre(&m);
    println!("two passes {px:?}");
    assert!(near(px, [148, 148, 148], 2));
}

#[test]
fn cull_mode_follows_the_style() {
    let Some(b) = Bench::new() else {
        return;
    };
    let mut ccw_cull = pass(3);
    ccw_cull.lighting_word = 0;
    ccw_cull.cull = 1;
    let mut cw_cull = ccw_cull;
    cw_cull.cull = 2;
    let mut none = ccw_cull;
    none.cull = 0;
    let tex = [10, 200, 10, 255];
    let drawn = |files: &ModelFiles, p: StylePass| {
        let m = b.model(files, tex, &[Some(style(white_material(), vec![p]))]);
        near(b.centre(&m), [10, 200, 10], 2)
    };
    let front = quad_model(false);
    let back = quad_model(true);
    // Direct3D: front faces are clockwise on screen; CULLMODE CCW removes the counter-clockwise ones
    assert!(drawn(&front, ccw_cull), "clockwise quad culled by CCW");
    assert!(
        !drawn(&back, ccw_cull),
        "counter-clockwise quad kept by CCW"
    );
    assert!(!drawn(&front, cw_cull), "clockwise quad kept by CW");
    assert!(drawn(&back, cw_cull), "counter-clockwise quad culled by CW");
    assert!(drawn(&back, none) && drawn(&front, none), "NONE culled");
}

/// One piece with three LODs (cubes of half size 10, 6 and 3) switching at 400 and 800.
fn lod_cube() -> ModelFiles {
    let lods = [10.0, 6.0, 3.0]
        .iter()
        .map(|&h| {
            let (data, idx) = cube_stream([0.0, 0.0, 100.0], h);
            lod(rigid(data, idx, 0), 0, vec![0])
        })
        .collect();
    finish(
        Model {
            pieces: vec![Piece {
                name: RawStr::from("cube"),
                lod_dist: vec![0.0, 400.0, 800.0],
                todo_lod_lo: 0,
                todo_lod_hi: 2,
                lods,
            }],
            nodes: vec![node("root", 0.0, None, vec![])],
            ..Model::default()
        },
        "SYN\\lods.ltb",
    )
}

#[test]
fn lods_switch_by_distance() {
    assert_eq!(select_lod(&[0.0, 400.0, 800.0], 0.0, 0), 0);
    assert_eq!(select_lod(&[0.0, 400.0, 800.0], 399.0, 0), 0);
    assert_eq!(select_lod(&[0.0, 400.0, 800.0], 400.0, 0), 1);
    assert_eq!(select_lod(&[0.0, 400.0, 800.0], 799.0, 0), 1);
    assert_eq!(select_lod(&[0.0, 400.0, 800.0], 5000.0, 0), 2);
    assert_eq!(select_lod(&[0.0, 400.0, 800.0], 100.0, 1), 1);
    assert_eq!(select_lod(&[0.0, 400.0, 800.0], 5000.0, -1), 1);
    assert_eq!(select_lod(&[0.0], 5000.0, 3), 0);
    let Some(b) = Bench::new() else {
        return;
    };
    let mut m = b.model(&lod_cube(), [255, 255, 255, 255], &[None]);
    lit(&mut m, 255.0, vec![]);
    let mut covered = Vec::new();
    for (d, want) in [(100.0, 0usize), (500.0, 1), (1000.0, 2)] {
        m.lod_distance = Some(d);
        assert_eq!(m.select_lods(Vec3::ZERO), vec![(0, want)], "distance {d}");
        let img = b.render(&[&m]);
        let n = img
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] > 200)
            .count();
        println!("distance {d}: LOD {want}, {n} pixels");
        covered.push(n);
    }
    assert!(covered[0] > covered[1] && covered[1] > covered[2] && covered[2] > 0);
    // without a fixed distance: the eye distance (the cube's object position is the origin here)
    m.lod_distance = None;
    m.position = Vec3::new(0.0, 0.0, 450.0);
    assert_eq!(m.select_lods(Vec3::ZERO), vec![(0, 1)]);
}

/// A body with a root and a "hand" node at x = 20, a socket on the hand at (0, 5, 0) rotated a quarter turn
/// about +Y, and a clip moving the hand to x = 40 over 1000 ms.
fn socket_body() -> ModelFiles {
    let (data, idx) = cube_stream([0.0, 0.0, 0.0], 2.0);
    let id = [0.0f32, 0.0, 0.0, 1.0];
    let track = |p0: [f32; 3], p1: [f32; 3]| Track::Keys {
        pos: PosKeys::F32(vec![p0, p1]),
        rot: RotKeys::F32(vec![id, id]),
    };
    let half = std::f32::consts::FRAC_1_SQRT_2;
    finish(
        Model {
            pieces: vec![Piece {
                name: RawStr::from("body"),
                lod_dist: vec![0.0],
                todo_lod_lo: 0,
                todo_lod_hi: 0,
                lods: vec![lod(rigid(data, idx, 0), 0, vec![0])],
            }],
            nodes: vec![
                node("root", 0.0, None, vec![1]),
                node("hand", 20.0, Some(0), vec![]),
            ],
            sockets: vec![Socket {
                node: 1,
                name: RawStr::from("grip"),
                rot: [0.0, half, 0.0, half],
                pos: [0.0, 5.0, 0.0],
                scale: [1.0; 3],
            }],
            clips: vec![Clip {
                extent: [1.0; 3],
                name: RawStr::from("reach"),
                codec: 0,
                blend_ms: 0,
                markers: [0u32, 1000]
                    .iter()
                    .map(|&t| Marker {
                        time_ms: t,
                        command: RawStr::default(),
                    })
                    .collect(),
                tracks: vec![
                    track([0.0; 3], [0.0; 3]),
                    track([20.0, 0.0, 0.0], [40.0, 0.0, 0.0]),
                ],
            }],
            ..Model::default()
        },
        "SYN\\body.ltb",
    )
}

#[test]
fn attached_model_hangs_at_the_socket() {
    let Some(b) = Bench::new() else {
        return;
    };
    let files = socket_body();
    let mut body = b.model(&files, [255, 0, 0, 255], &[None]);
    body.position = Vec3::new(0.0, 0.0, 100.0);
    body.rotation = Quat::IDENTITY;
    let mut child = b.model(&cube(3.0), [0, 255, 0, 255], &[None]);
    assert!(body.attach(&files.body, 0, Vec3::ZERO, Quat::IDENTITY, &mut child));
    // bind pose: hand at x = 20, socket 5 above it, rotated a quarter turn
    assert!(
        (child.position - Vec3::new(20.0, 5.0, 100.0)).length() < 1e-3,
        "{}",
        child.position
    );
    assert!(
        (child
            .rotation
            .dot(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)))
        .abs()
            > 0.999
    );
    // the hand moves with the clip and the child follows
    body.set_pose(&[Tracker::main(0, 1000.0 - 1.0)]);
    assert!(body.attach(&files.body, 0, Vec3::ZERO, Quat::IDENTITY, &mut child));
    assert!(
        (child.position.x - 40.0).abs() < 0.1 && (child.position.y - 5.0).abs() < 1e-3,
        "{}",
        child.position
    );
    // an attachment offset applies in the socket's frame (the quarter turn maps +X to -Z)
    assert!(body.attach(
        &files.body,
        0,
        Vec3::new(1.0, 0.0, 0.0),
        Quat::IDENTITY,
        &mut child
    ));
    assert!((child.position.z - 99.0).abs() < 0.1, "{}", child.position);
    assert!(!body.attach(&files.body, 7, Vec3::ZERO, Quat::IDENTITY, &mut child));
    // the node bounds of the hand hold the socket's x (the body cube sits on the root, so the hand owns no
    // vertex here: no bounds)
    assert_eq!(body.node_bounds(1), None);
    assert!(body.node_bounds(0).is_some());
}
