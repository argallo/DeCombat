//! Pixel-neutral recording of the opaque world (render-core): view-frustum culling of world batches, merging of
//! touching same-material batches and skipping unchanged binds must not change one byte of the frame.
//!
//! - Synthetic (no ROM, any headless adapter, skipped without one): quads in front of, behind, beside and beyond the
//!   camera, two of them touching with the same material; the culled and drawn counts are checked and the frame
//!   with culling on equals the frame with culling off byte for byte.
//! - ROM-gated (`CA_ROM`): every map of the user's ROM from its first spawn at nine view directions (sky and
//!   translucent world models on, the viewer's defaults): on == off byte for byte; prints drawn / culled counts.
//!   Nothing of the ROM is asserted by value; no image is written unless `DC_RENDER_OUT` is set.

use dc_render::*;
use glam::Vec3;

const W: u32 = 320;
const H: u32 = 180;

fn context() -> Option<GpuContext> {
    match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => Some(c),
        Err(e) => {
            println!("skipped: {e}");
            None
        }
    }
}

/// A quad facing the camera at the origin (clockwise on screen), centred at `c`, half size `s`.
fn quad(mesh: &mut SceneMesh, c: [f32; 3], s: f32, colour: [u8; 4], tex: u32) {
    let base = mesh.vertices.len() as u32;
    for (dx, dy) in [(-s, -s), (-s, s), (s, s), (s, -s)] {
        mesh.vertices.push(GpuVertex {
            pos: [c[0] + dx, c[1] + dy, c[2]],
            colour,
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
        tex0: Some(tex),
        tex1: None,
        lightmap: None,
        first_index: first,
        index_count: 6,
    });
}

#[test]
fn culling_and_merging_keep_the_synthetic_frame() {
    let Some(ctx) = context() else {
        return;
    };
    let mut r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut mesh = SceneMesh {
        textures: vec![b"a".to_vec(), b"b".to_vec()],
        ..SceneMesh::default()
    };
    // in view: two touching quads with material 0 (merged into one draw), one with material 1
    quad(&mut mesh, [-30.0, 0.0, 100.0], 20.0, [200, 60, 60, 255], 0);
    quad(&mut mesh, [10.0, 0.0, 100.0], 20.0, [60, 200, 60, 255], 0);
    quad(&mut mesh, [0.0, 30.0, 120.0], 25.0, [60, 60, 200, 255], 1);
    // out of view: behind, far to the side, beyond the far plane, below
    quad(&mut mesh, [0.0, 0.0, -100.0], 20.0, [255, 255, 0, 255], 1);
    quad(&mut mesh, [5000.0, 0.0, 100.0], 20.0, [255, 0, 255, 255], 0);
    quad(&mut mesh, [0.0, 0.0, 20000.0], 50.0, [0, 255, 255, 255], 0);
    quad(
        &mut mesh,
        [0.0, -3000.0, 100.0],
        20.0,
        [255, 255, 255, 255],
        1,
    );
    // partly in view (crosses the left edge): must be drawn
    quad(
        &mut mesh,
        [-110.0, 0.0, 100.0],
        30.0,
        [120, 120, 120, 255],
        1,
    );
    let scene = r.upload(&ctx, &mesh, &mut |n| {
        Some(TextureImage::rgba(
            2,
            2,
            if n == b"a" {
                [255u8, 255, 255, 255]
            } else {
                [180, 180, 255, 255]
            }
            .repeat(4),
        ))
    });
    let fx = 90f32.to_radians();
    let cam = Camera::looking(Vec3::ZERO, 0.0, 0.0, fx, fov_y_for(fx, W, H));
    let opts = FrameOptions::opaque_only();
    let (on, ron) = r
        .render_frame_offscreen(&ctx, &scene, &cam, &opts, &[], W, H)
        .expect("frame");
    r.set_world_culling(false);
    let (off, roff) = r
        .render_frame_offscreen(&ctx, &scene, &cam, &opts, &[], W, H)
        .expect("frame");
    println!(
        "on: draws {} culled {} binds {}; off: draws {} culled {} binds {}",
        ron.world_draws,
        ron.world_culled,
        ron.bind_sets,
        roff.world_draws,
        roff.world_culled,
        roff.bind_sets
    );
    assert_eq!(
        ron.world_culled, 4,
        "behind, side, far and below are culled"
    );
    // 4 visible batches: the two touching material-0 quads merge, then material 1, then the edge quad (material 1,
    // touching the previous range only after the culled ones, so a separate draw)
    assert_eq!(ron.world_draws, 3);
    assert_eq!(roff.world_culled, 0);
    assert!(
        roff.world_draws < 8,
        "contiguous runs merge with culling off too"
    );
    assert!(on.rgba == off.rgba, "culling changed the frame");
    // something was drawn
    let clear = CLEAR_COLOUR.map(|c| (c * 255.0).round() as u8);
    assert!(on
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .any(|p| p[..3] != clear[..3]));
}

#[test]
fn culling_keeps_every_rom_map_frame() {
    let rom = dc_rom::rom_or_skip!();
    let mut viewer = match RomViewer::open(&rom, AdapterChoice::Auto, RenderSettings::default()) {
        Ok(v) => v,
        Err(ViewError::Gpu(e)) => {
            println!("skipped: {e}");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    let maps = viewer.map_names();
    assert!(!maps.is_empty());
    let (mut frames, mut drawn, mut culled) = (0u64, 0u64, 0u64);
    for map in &maps {
        let mut req = ViewRequest::new(map);
        req.width = W;
        req.height = H;
        viewer.renderer.set_world_culling(true);
        let mut p = viewer
            .prepare(&req)
            .unwrap_or_else(|e| panic!("{map}: {e}"));
        let base = p.camera;
        let (mut md, mut mc) = (0u64, 0u64);
        for k in 0..9 {
            let (yaw, pitch) = if k < 8 {
                (k as f32 * 45f32.to_radians(), 0.0)
            } else {
                (0.0, 60f32.to_radians())
            };
            p.camera = Camera {
                rot: glam::Quat::from_rotation_y(yaw)
                    * base.rot
                    * glam::Quat::from_rotation_x(pitch),
                ..base
            };
            viewer.renderer.set_world_culling(true);
            let (on, ron) = viewer.render_prepared(&p, W, H).expect("frame");
            viewer.renderer.set_world_culling(false);
            let (off, roff) = viewer.render_prepared(&p, W, H).expect("frame");
            assert_eq!(roff.world_culled, 0);
            if on.rgba != off.rgba {
                if let Ok(dir) = std::env::var("DC_RENDER_OUT") {
                    let d = std::path::Path::new(&dir);
                    let _ = on.save_png(&d.join(format!("cull_{map}_{k}_on.png")));
                    let _ = off.save_png(&d.join(format!("cull_{map}_{k}_off.png")));
                }
                panic!("{map} view {k}: culling changed the frame");
            }
            md += ron.world_draws;
            mc += ron.world_culled;
            frames += 1;
        }
        println!("{map}: 9 views equal; world draws {md}, culled batches {mc}");
        drawn += md;
        culled += mc;
    }
    viewer.renderer.set_world_culling(true);
    println!("{frames} frames equal; world draws {drawn}, culled batches {culled}");
}
