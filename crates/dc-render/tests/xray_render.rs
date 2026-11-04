//! CLI-1624: the x-ray styles on whatever headless adapter exists (skips cleanly without one). A synthetic skinned
//! figure (invented here, written with dc-model's writer) stands behind a white wall: chams show the primary colour
//! at its centre instead of the wall, the skeleton draws its bone in the primary colour, the box fills and borders
//! with the two colours, and nothing is drawn over the first-person depth range.

use dc_model::{
    Lod, Mesh, Model, Node, Piece, RawStr, SkinSpan, SkinTail, SkinnedMesh, VertexStream, IDENTITY,
};
use dc_render::*;
use glam::Vec3;

const W: u32 = 128;
const H: u32 = 96;

const PRIMARY: [u8; 4] = [255, 0, 255, 255];
const SECONDARY: [u8; 4] = [0, 255, 0, 255];

fn context() -> Option<GpuContext> {
    match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => Some(c),
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

fn node(name: &str, p: [f32; 3], parent: Option<usize>, children: Vec<usize>) -> Node {
    let mut m = IDENTITY;
    m[3] = p[0];
    m[7] = p[1];
    m[11] = p[2];
    Node {
        name: RawStr::from(name),
        index: 0,
        flags: 0,
        model_xform: m,
        parent,
        children,
    }
}

/// A box of half sizes `h` centred on `c`, every vertex on node 1 (two influences, the root's weight 0).
fn box_stream(c: [f32; 3], h: [f32; 3]) -> (Vec<u8>, Vec<u16>) {
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
            let p = [0, 1, 2].map(|i| c[i] + h[i] * (n[i] + su * u[i] + sv * v[i]));
            for f in p.iter().chain(&[0.0f32]).chain(n).chain(&[0.5f32, 0.5]) {
                data.extend_from_slice(&f.to_le_bytes());
            }
        }
        idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (data, idx)
}

/// The figure: root at the origin, "hip" at (0, 0, 100), "hand" at (20, 0, 100); a body box of half size
/// (8, 16, 4) at the hip, skinned to the hip.
fn figure() -> ModelFiles {
    let (data, idx) = box_stream([0.0, 0.0, 100.0], [8.0, 16.0, 4.0]);
    let mesh = Mesh::Skinned(SkinnedMesh {
        vertex_count: 24,
        tri_count: 12,
        max_influences: 2,
        palette_influences: 2,
        has_palette_map: 0,
        streams: [
            VertexStream { flags: 0x13, data },
            VertexStream::default(),
            VertexStream::default(),
            VertexStream::default(),
        ],
        indices: idx,
        tail: SkinTail::Spans(vec![SkinSpan {
            first_vertex: 0,
            vertex_count: 24,
            nodes: [0, 1, 0xff, 0xff],
            index_end: 36,
        }]),
    });
    let mut m = Model {
        pieces: vec![Piece {
            name: RawStr::from("body"),
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
                used_nodes: vec![0, 1],
            }],
        }],
        nodes: vec![
            node("root", [0.0; 3], None, vec![1]),
            node("hip", [0.0, 0.0, 100.0], Some(0), vec![2]),
            node("hand", [20.0, 0.0, 100.0], Some(1), vec![]),
        ],
        ..Model::default()
    };
    m.radius = 30.0;
    m.clip_overrides = vec![Vec::new(); m.child_models.len() + 1];
    m.fix_body_lens();
    let bytes = m.to_bytes().expect("synthetic model encodes");
    ModelFiles::from_model("SYN\\figure.ltb", Model::parse(&bytes).expect("parses"))
}

/// A white wall at z = 60 over the whole view.
fn wall(ctx: &GpuContext, r: &WorldRenderer) -> GpuScene {
    let mut mesh = SceneMesh::default();
    mesh.textures = vec![b"white".to_vec()];
    let v = |p: [f32; 3]| GpuVertex {
        pos: p,
        colour: [128, 128, 128, 255],
        uv0: [0.5, 0.5],
        uv1: [0.5, 0.5],
    };
    for p in [
        [-200.0, -200.0, 60.0],
        [-200.0, 200.0, 60.0],
        [200.0, 200.0, 60.0],
        [200.0, -200.0, 60.0],
    ] {
        mesh.vertices.push(v(p));
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
    r.upload(ctx, &mesh, &mut |_| {
        Some(TextureImage::rgba(2, 2, [255u8, 255, 255, 255].repeat(4)))
    })
}

fn near(a: [u8; 4], b: [u8; 4], tol: i32) -> bool {
    (0..3).all(|k| (a[k] as i32 - b[k] as i32).abs() <= tol)
}

fn frame(
    ctx: &GpuContext,
    r: &WorldRenderer,
    scene: &GpuScene,
    body: &ModelInstance,
    mode: Option<XrayMode>,
) -> FrameImage {
    let xray: Vec<XrayModel<'_>> = mode
        .map(|mode| XrayModel {
            instances: vec![body],
            mode,
            primary: PRIMARY,
            secondary: SECONDARY,
        })
        .into_iter()
        .collect();
    let (img, _) = r
        .render_frame_offscreen_xray(
            ctx,
            scene,
            &camera(),
            &FrameOptions::opaque_only(),
            &[body],
            &[],
            &PlayerViewSettings::default(),
            &xray,
            W,
            H,
        )
        .expect("frame");
    img
}

#[test]
fn xray_styles_show_an_occluded_figure_through_a_wall() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let scene = wall(&ctx, &r);
    let body = r.upload_model(
        &ctx,
        &figure(),
        &[Some(TextureImage::rgba(2, 2, [0u8, 0, 255, 255].repeat(4)))],
    );
    let cam = camera();
    let centre = project_px(&cam, (W, H), Vec3::new(0.0, 0.0, 96.0)).unwrap();
    let (cx, cy) = (centre[0] as u32, centre[1] as u32);
    // vanilla: the wall hides the figure
    let plain = frame(&ctx, &r, &scene, &body, None);
    assert!(
        near(plain.pixel(cx, cy), [255, 255, 255, 255], 4),
        "{:?}",
        plain.pixel(cx, cy)
    );
    // chams: the primary colour at the body centre, nothing of the wall
    let chams = frame(&ctx, &r, &scene, &body, Some(XrayMode::Chams));
    assert!(
        near(chams.pixel(cx, cy), PRIMARY, 2),
        "chams centre {:?}",
        chams.pixel(cx, cy)
    );
    // skeleton: the hip-hand bone's midpoint in the primary colour, the hand joint in the secondary
    let skel = frame(&ctx, &r, &scene, &body, Some(XrayMode::Skeleton));
    let mid = project_px(&cam, (W, H), Vec3::new(10.0, 0.0, 100.0)).unwrap();
    let px = skel.pixel(mid[0] as u32, mid[1] as u32);
    assert!(near(px, PRIMARY, 2), "bone midpoint {px:?}");
    let hand = project_px(&cam, (W, H), Vec3::new(20.0, 0.0, 100.0)).unwrap();
    let px = skel.pixel(hand[0] as u32, hand[1] as u32);
    assert!(near(px, SECONDARY, 2), "hand joint {px:?}");
    // the bone ends at the hip: nothing drawn left of it, the root's line is left out
    let left = project_px(&cam, (W, H), Vec3::new(-10.0, 0.0, 100.0)).unwrap();
    assert!(near(
        skel.pixel(left[0] as u32, left[1] as u32),
        [255, 255, 255, 255],
        4
    ));
    // box: the border in the secondary colour, the inside the wall tinted by a third of the primary
    let boxed = frame(&ctx, &r, &scene, &body, Some(XrayMode::Box));
    let px = boxed.pixel(cx, cy);
    assert!(px[1] < 230 && px[0] > 230 && px[2] > 230, "box fill {px:?}");
    let (lo, hi) = body.bounds_in_world().unwrap();
    let edge = project_px(&cam, (W, H), Vec3::new(lo.x, 0.0, lo.z)).unwrap();
    let top = project_px(&cam, (W, H), Vec3::new(0.0, hi.y, lo.z)).unwrap();
    let px = boxed.pixel((edge[0] + 1.0) as u32, cy);
    assert!(near(px, SECONDARY, 2), "box border {px:?}");
    assert!(top[1] < cy as f32);
}
