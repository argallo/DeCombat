//! CLI-1593: multisample anti-aliasing of the 3D passes. A tilted quad over the clear colour is rendered with MSAA off
//! and with 4 samples through the offscreen path (the world pass draws into the multisampled target and resolves
//! into the read-back target): the 4x frame shows partial coverage along the quad's edges (pixels between the quad
//! colour and the clear colour), its interior and the background stay the single-sample values, and the frame has
//! the same size. With MSAA off nothing changes: `RenderSettings::msaa` 1, an out-of-range count and the default
//! build a renderer of count 1 whose frame hashes the same. Every count the adapter lists renders a frame without a
//! validation error. Skipped without a GPU adapter.

use dc_render::{
    fov_y_for, supported_msaa, texel_hash, AdapterChoice, Batch, Camera, FrameImage, FrameOptions,
    GpuContext, GpuVertex, PassKind, RenderSettings, SceneMesh, TextureImage, WorldRenderer,
    CLEAR_COLOUR, OFFSCREEN_FORMAT,
};
use glam::Vec3;

const W: u32 = 160;
const H: u32 = 120;
/// Vertex colour of the quad; the vertex-lit recipe with `saturate` doubles it over the white texture.
const VERTEX: [u8; 4] = [100, 60, 20, 255];
const QUAD: [u8; 3] = [200, 120, 40];

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

/// A quad facing the camera, centred at `c`, half size `s`, turned by `angle` in its plane (clockwise on screen, so
/// it is not culled), in bucket 1 with texture 0.
fn tilted_quad(mesh: &mut SceneMesh, c: [f32; 3], s: f32, angle: f32) {
    let base = mesh.vertices.len() as u32;
    let (sin, cos) = angle.sin_cos();
    for (dx, dy) in [(-s, -s), (-s, s), (s, s), (s, -s)] {
        mesh.vertices.push(GpuVertex {
            pos: [c[0] + dx * cos - dy * sin, c[1] + dx * sin + dy * cos, c[2]],
            colour: VERTEX,
            uv0: [0.5, 0.5],
            uv1: [0.5, 0.5],
        });
    }
    let first = mesh.indices.len() as u32;
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    mesh.batches.push(Batch {
        bucket: 1,
        pass: PassKind::VertexLit,
        tex0: Some(0),
        tex1: None,
        lightmap: None,
        first_index: first,
        index_count: 6,
    });
}

fn scene() -> SceneMesh {
    let mut mesh = SceneMesh {
        textures: vec![b"white".to_vec()],
        ..SceneMesh::default()
    };
    tilted_quad(&mut mesh, [0.0, 0.0, 100.0], 40.0, 0.5);
    mesh
}

fn render(ctx: &GpuContext, settings: RenderSettings) -> (WorldRenderer, FrameImage) {
    let r = WorldRenderer::new(ctx, OFFSCREEN_FORMAT, settings);
    let gpu = r.upload(ctx, &scene(), &mut |_| {
        Some(TextureImage::rgba(2, 2, [255u8; 4].repeat(4)))
    });
    let fx = 90f32.to_radians();
    let cam = Camera::looking(Vec3::ZERO, 0.0, 0.0, fx, fov_y_for(fx, W, H));
    let (img, _) = r
        .render_frame_offscreen(ctx, &gpu, &cam, &FrameOptions::default(), &[], W, H)
        .expect("frame");
    assert_eq!((img.width, img.height), (W, H));
    (r, img)
}

fn clear() -> [u8; 3] {
    [CLEAR_COLOUR[0], CLEAR_COLOUR[1], CLEAR_COLOUR[2]].map(|c| (c * 255.0).round() as u8)
}

/// Pixels whose red channel lies strictly between the clear colour's and the quad's (partial coverage).
fn partial_pixels(img: &FrameImage) -> usize {
    let (lo, hi) = (clear()[0] + 8, QUAD[0] - 8);
    img.rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] > lo && p[0] < hi)
        .count()
}

fn near(a: [u8; 4], b: [u8; 3]) -> bool {
    (0..3).all(|i| (a[i] as i32 - b[i] as i32).abs() <= 2)
}

#[test]
fn four_samples_smooth_the_edges_and_keep_the_interior() {
    let Some(ctx) = context() else {
        return;
    };
    let supported = supported_msaa(&ctx.adapter, OFFSCREEN_FORMAT);
    println!("supported MSAA counts: {supported:?}");
    assert_eq!(supported.first(), Some(&1));
    assert!(supported.iter().all(|n| n.is_power_of_two() && *n <= 8));
    assert!(
        supported.contains(&4),
        "wgpu guarantees 4 samples for Rgba8Unorm and Depth32Float"
    );
    let (r1, one) = render(&ctx, RenderSettings::default());
    let (r4, four) = render(
        &ctx,
        RenderSettings {
            msaa: 4,
            ..RenderSettings::default()
        },
    );
    assert_eq!((r1.msaa(), r4.msaa()), (1, 4));
    if let Ok(dir) = std::env::var("DC_RENDER_OUT") {
        let dir = std::path::Path::new(&dir);
        one.save_png(&dir.join("msaa_1.png")).expect("png");
        four.save_png(&dir.join("msaa_4.png")).expect("png");
    }
    // interior and background: the same values (one fragment per pixel either way; all samples covered or none)
    let centre = (W / 2, H / 2);
    assert!(
        near(one.pixel(centre.0, centre.1), QUAD),
        "{:?}",
        one.pixel(centre.0, centre.1)
    );
    assert_eq!(
        one.pixel(centre.0, centre.1),
        four.pixel(centre.0, centre.1)
    );
    assert!(near(one.pixel(2, 2), clear()), "{:?}", one.pixel(2, 2));
    assert_eq!(one.pixel(2, 2), four.pixel(2, 2));
    // edges: binary coverage without MSAA, partial coverage with it
    let (p1, p4) = (partial_pixels(&one), partial_pixels(&four));
    println!("partially covered pixels: 1x {p1}, 4x {p4}");
    assert_eq!(p1, 0, "single-sample pixels are quad or clear");
    assert!(p4 >= 20, "the tilted edges show intermediate values ({p4})");
    // most of the frame is untouched by the anti-aliasing
    let differing = one
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(four.rgba.as_chunks::<4>().0.iter())
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        differing < (W * H / 10) as usize,
        "{differing} of {} pixels differ",
        W * H
    );
}

#[test]
fn msaa_off_is_the_default_renderer() {
    let Some(ctx) = context() else {
        return;
    };
    let (r, base) = render(&ctx, RenderSettings::default());
    assert_eq!(r.msaa(), 1);
    let h = texel_hash(W, H, &base.rgba);
    for msaa in [1u32, 0, 3, 16] {
        let (r, img) = render(
            &ctx,
            RenderSettings {
                msaa,
                ..RenderSettings::default()
            },
        );
        assert_eq!(r.msaa(), 1, "msaa {msaa} reads as off");
        assert_eq!(
            texel_hash(W, H, &img.rgba),
            h,
            "msaa {msaa}: the same bytes"
        );
    }
}

#[test]
fn every_supported_count_renders() {
    let Some(ctx) = context() else {
        return;
    };
    for n in supported_msaa(&ctx.adapter, OFFSCREEN_FORMAT) {
        let (r, img) = render(
            &ctx,
            RenderSettings {
                msaa: n,
                ..RenderSettings::default()
            },
        );
        assert_eq!(r.msaa(), n);
        assert!(
            near(img.pixel(W / 2, H / 2), QUAD),
            "{n}x: {:?}",
            img.pixel(W / 2, H / 2)
        );
        assert!(
            near(img.pixel(2, 2), clear()),
            "{n}x: {:?}",
            img.pixel(2, 2)
        );
        println!("{n}x: partially covered pixels {}", partial_pixels(&img));
    }
}
