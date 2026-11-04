//! Synthetic world render smoke test on whatever headless adapter exists (hardware, else the software fallback
//! adapter). All content is invented here. Skips cleanly when no adapter exists (CI without a GPU).

use dc_render::*;
use dc_world::synth::{SynthBlock, SynthRenderWorld, SynthSection, SynthWorld};
use dc_world::World;
use glam::Vec3;

const W: u32 = 96;
const H: u32 = 64;

fn vtx(p: [f32; 3], uv: [f32; 2], colour: u32) -> [u8; 68] {
    RenderVertex {
        pos: p,
        uv0: uv,
        uv1: uv,
        colour,
        normal: [0.0, 0.0, -1.0],
        tangent: [1.0, 0.0, 0.0],
        binormal: [0.0, 1.0, 0.0],
    }
    .to_bytes()
}

/// Quad facing a camera at the origin looking along +Z, wound clockwise on screen unless `flip`.
#[allow(clippy::too_many_arguments)]
fn quad(
    verts: &mut Vec<[u8; 68]>,
    tris: &mut Vec<[u32; 4]>,
    x0: f32,
    x1: f32,
    z: f32,
    hy: f32,
    colour: u32,
    flip: bool,
) {
    let b = verts.len() as u32;
    // bottom-left, top-left, top-right, bottom-right
    verts.push(vtx([x0, -hy, z], [0.25, 0.75], colour));
    verts.push(vtx([x0, hy, z], [0.25, 0.25], colour));
    verts.push(vtx([x1, hy, z], [0.75, 0.25], colour));
    verts.push(vtx([x1, -hy, z], [0.75, 0.75], colour));
    if flip {
        tris.push([b, b + 2, b + 1, 0]);
        tris.push([b, b + 3, b + 2, 0]);
    } else {
        tris.push([b, b + 1, b + 2, 0]);
        tris.push([b, b + 2, b + 3, 0]);
    }
}

fn section(tex: &[u8], kind: u8, tris: u32, lightmap: Option<(u32, u32, Vec<u8>)>) -> SynthSection {
    let (w, h, lm) = lightmap.unwrap_or((0, 0, Vec::new()));
    SynthSection {
        textures: [tex.to_vec(), Vec::new()],
        shader_kind: kind,
        triangles: tris,
        effect: Vec::new(),
        lightmap_w: w,
        lightmap_h: h,
        lightmap: lm,
    }
}

/// Three quads at depth 100: left = lightmapped (sections in file order: texture first, lightmap second, so the
/// bucket order must put the lightmap first), middle = vertex-lit, right = vertex-lit but wound
/// counter-clockwise (culled). Behind the middle quad, at depth 200, a taller red quad: the depth test must hide
/// it behind the middle quad (it is drawn after it) and it shows above and below.
fn synth_world() -> Vec<u8> {
    let mut verts = Vec::new();
    let mut tris = Vec::new();
    // section 0: kind 4 (lightmapped texture), 2 triangles
    quad(
        &mut verts, &mut tris, -60.0, -25.0, 100.0, 20.0, 0xffffffff, false,
    );
    // section 1: kind 2 (lightmap), duplicate vertices of the same quad
    quad(
        &mut verts, &mut tris, -60.0, -25.0, 100.0, 20.0, 0xffffffff, false,
    );
    // section 2: kind 1 vertex-lit, colour ARGB 0xff604020
    quad(
        &mut verts, &mut tris, -20.0, 20.0, 100.0, 20.0, 0xff604020, false,
    );
    // section 3: kind 1 vertex-lit, counter-clockwise: culled
    quad(
        &mut verts, &mut tris, 25.0, 60.0, 100.0, 20.0, 0xffffffff, true,
    );
    // section 4: kind 1 red quad behind the middle one
    quad(
        &mut verts, &mut tris, -30.0, 30.0, 200.0, 60.0, 0xffffffff, false,
    );
    // lightmap 4x4, stored bytes R, G, B = (60, 180, 120) (REN-20: the first stored byte is red)
    let lm: Vec<u8> = std::iter::repeat_n([60u8, 180, 120], 16)
        .flatten()
        .collect();
    let block = SynthBlock {
        center: [0.0, 0.0, 150.0],
        half: [100.0, 50.0, 100.0],
        sections: vec![
            section(b"syn/base", 4, 2, None),
            section(b"", 2, 2, Some((4, 4, pack_lightmap(&lm)))),
            section(b"syn/base", 1, 2, None),
            section(b"syn/base", 1, 2, None),
            section(b"syn/red", 1, 2, None),
        ],
        vertices: verts,
        triangles: tris,
        ..SynthBlock::default()
    };
    SynthWorld {
        extent_min: [-100.0; 3],
        extent_max: [100.0; 3],
        tree_shape: vec![false],
        grid_cell: [32.0; 3],
        grid_dims: [1, 1, 1],
        grid_rgb: vec![0; 3],
        render: SynthRenderWorld {
            blocks: vec![block],
            models: vec![],
        },
        ..SynthWorld::default()
    }
    .build()
}

fn solid(rgba: [u8; 4]) -> TextureImage {
    TextureImage::rgba(2, 2, rgba.repeat(4))
}

fn near(a: [u8; 4], b: [u8; 3], tol: i32) -> bool {
    (0..3).all(|k| (a[k] as i32 - b[k] as i32).abs() <= tol)
}

fn context(choice: AdapterChoice) -> Option<GpuContext> {
    match GpuContext::headless(choice) {
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

#[test]
fn adapter_probe_reports_every_attempt() {
    let probes = probe_adapters();
    assert_eq!(probes.len(), 4);
    for p in &probes {
        println!("probe {}: {:?}", p.attempt, p.result);
    }
}

#[test]
fn synthetic_world_renders_lightmap_then_multiply() {
    let data = synth_world();
    let world = World::parse(&data).expect("synthetic world parses");
    let mesh = SceneMesh::build(&world, SceneOptions::default(), &mut |_| TextureTraits {
        present: true,
        mode: 0,
        hi: false,
    });
    assert_eq!(mesh.stats.sections_drawn, 5);
    assert_eq!(mesh.stats.lightmaps, 1);
    assert_eq!(mesh.stats.per_bucket[14], 1);
    assert_eq!(mesh.stats.per_bucket[15], 1);
    assert_eq!(mesh.stats.per_bucket[1], 3);
    // flush order: bucket 1, then 14, then 15
    let order: Vec<u8> = mesh.batches.iter().map(|b| b.bucket).collect();
    assert_eq!(order, vec![1, 1, 1, 14, 15]);

    let Some(ctx) = context(AdapterChoice::Auto) else {
        return;
    };
    render_and_check(&ctx, &mesh);
}

/// The same frame on the software fallback adapter (WARP on Windows): what a GPU-less CI machine would use.
#[test]
fn synthetic_world_on_software_adapter() {
    let data = synth_world();
    let world = World::parse(&data).expect("synthetic world parses");
    let mesh = SceneMesh::build(&world, SceneOptions::default(), &mut |_| TextureTraits {
        present: true,
        mode: 0,
        hi: false,
    });
    let Some(ctx) = context(AdapterChoice::Fallback) else {
        return;
    };
    render_and_check(&ctx, &mesh);
}

fn render_and_check(ctx: &GpuContext, mesh: &SceneMesh) {
    let renderer = WorldRenderer::new(ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let scene = renderer.upload(ctx, mesh, &mut |name| match name {
        b"syn/base" => Some(solid([200, 40, 160, 255])),
        b"syn/red" => Some(solid([255, 0, 0, 255])),
        _ => None,
    });
    assert_eq!(scene.stats.missing_textures, 0);
    let fov_x = 90f32.to_radians();
    let cam = Camera::looking(Vec3::ZERO, 0.0, 0.0, fov_x, fov_y_for(fov_x, W, H));
    let (img, ms) = renderer
        .render_offscreen(ctx, &scene, &cam, W, H)
        .expect("frame");
    println!("gpu frame {ms:.2} ms");
    if let Ok(dir) = std::env::var("DC_RENDER_OUT") {
        let file = format!("synthetic_{:?}.png", ctx.info.device_type).to_lowercase();
        img.save_png(&std::path::Path::new(&dir).join(file))
            .expect("png");
    }
    // project the quad centres: x = 100 * x_world / (z * tan(45°)) -> pixel
    let px = |xw: f32, z: f32| ((xw / z) * 0.5 + 0.5) * W as f32;
    let cy = H / 2;
    let left = img.pixel(px(-42.5, 100.0) as u32, cy);
    let mid = img.pixel(px(0.0, 100.0) as u32, cy);
    let right = img.pixel(px(42.5, 100.0) as u32, cy);
    let corner = img.pixel(1, 1);
    println!("left {left:?} mid {mid:?} right {right:?} corner {corner:?}");
    // lightmap pass writes (60, 180, 120); the multiply pass with `saturate` gives 2 * dst * src, clamped
    let expect_left = [
        (2.0f32 * 60.0 * 200.0 / 255.0).min(255.0),
        (2.0f32 * 180.0 * 40.0 / 255.0).min(255.0),
        (2.0f32 * 120.0 * 160.0 / 255.0).min(255.0),
    ]
    .map(|v| v.round() as u8);
    assert!(
        near(left, expect_left, 4),
        "left {left:?} want {expect_left:?}"
    );
    // vertex-lit x2: 2 * T0 * D with D = (0x60, 0x40, 0x20) / 255
    let expect_mid = [
        2.0f32 * 200.0 * 96.0 / 255.0,
        2.0 * 40.0 * 64.0 / 255.0,
        2.0 * 160.0 * 32.0 / 255.0,
    ]
    .map(|v| v.round() as u8);
    assert!(near(mid, expect_mid, 4), "mid {mid:?} want {expect_mid:?}");
    // counter-clockwise quad culled: background (the red quad behind does not reach x = 42.5 at depth 200)
    let clear = CLEAR_COLOUR.map(|c| (c * 255.0).round() as u8);
    assert!(
        near(right, [clear[0], clear[1], clear[2]], 2),
        "right {right:?}"
    );
    assert!(
        near(corner, [clear[0], clear[1], clear[2]], 2),
        "corner {corner:?}"
    );
    // the red quad behind the middle one shows above it (y_ndc 0.38 at depth 200 with ty = tan(fov_y / 2))
    let ty = (fov_y_for(fov_x, W, H) * 0.5).tan();
    let y_ndc = 0.38f32;
    assert!(y_ndc * 200.0 * ty < 60.0 && y_ndc * 100.0 * ty > 20.0);
    let above = img.pixel(W / 2, ((0.5 - 0.5 * y_ndc) * H as f32) as u32);
    assert!(
        above[0] > 200 && above[1] < 30,
        "red quad above the middle quad: {above:?}"
    );
}

#[test]
fn bucket_order_flips_without_lightmaps() {
    let data = synth_world();
    let world = World::parse(&data).unwrap();
    let opts = SceneOptions {
        settings: RenderSettings {
            lightmaps: false,
            ..RenderSettings::default()
        },
        ..SceneOptions::default()
    };
    let mesh = SceneMesh::build(&world, opts, &mut |_| TextureTraits {
        present: true,
        mode: 0,
        hi: false,
    });
    // kind 4 falls to the vertex-lit path, the lightmap section is not drawn
    assert_eq!(mesh.stats.per_bucket[1], 4);
    assert_eq!(mesh.stats.per_bucket[14], 0);
    assert_eq!(mesh.stats.sections_hidden, 1);
}
