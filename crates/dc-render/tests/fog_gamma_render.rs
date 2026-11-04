//! Synthetic renders of the linear fog (RENDER_2009.md §3.3, with the per-pass fog colours of §5.2) and of the gamma
//! copy (§2.5) on whatever headless adapter exists. Everything is invented here; nothing comes from the ROM. Skips
//! cleanly when no adapter exists.

use dc_render::*;
use glam::Vec3;

const W: u32 = 64;
const H: u32 = 48;

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

fn quad(mesh: &mut SceneMesh, z: f32, colour: [u8; 4]) -> u32 {
    let first = mesh.indices.len() as u32;
    let base = mesh.vertices.len() as u32;
    let s = z * 4.0;
    for (p, uv) in [
        ([-s, -s, z], [0.0, 1.0]),
        ([-s, s, z], [0.0, 0.0]),
        ([s, s, z], [1.0, 0.0]),
        ([s, -s, z], [1.0, 1.0]),
    ] {
        mesh.vertices.push(GpuVertex {
            pos: p,
            colour,
            uv0: uv,
            uv1: uv,
        });
    }
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    first
}

fn batch(bucket: u8, first: u32, tex0: Option<u32>, lightmap: Option<u32>) -> Batch {
    Batch {
        bucket,
        pass: PassKind::of_bucket(bucket).unwrap(),
        tex0,
        tex1: None,
        lightmap,
        first_index: first,
        index_count: 6,
    }
}

fn centre(img: &FrameImage) -> [u8; 4] {
    img.pixel(W / 2, H / 2)
}

fn near(a: [u8; 4], b: [u8; 3], tol: i32) -> bool {
    (0..3).all(|k| (a[k] as i32 - b[k] as i32).abs() <= tol)
}

const FOG: Fog = Fog {
    near: 0.0,
    far: 200.0,
    colour: [0.0, 0.0, 255.0],
};

#[test]
fn vertex_lit_wall_blends_toward_the_fog_colour_by_view_depth() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut mesh = SceneMesh {
        textures: vec![b"w".to_vec()],
        ..SceneMesh::default()
    };
    // grey vertex colour 128 x white texture x2 (`saturate`) = white; wall at depth 100 = half way
    let f = quad(&mut mesh, 100.0, [128, 128, 128, 255]);
    mesh.batches.push(batch(1, f, Some(0), None));
    let white = |_: &[u8]| Some(TextureImage::rgba(2, 2, [255u8; 4].repeat(4)));
    let mut scene = r.upload(&ctx, &mesh, &mut { white });
    let opts = FrameOptions::opaque_only();
    let (plain, _) = r
        .render_frame_offscreen(&ctx, &scene, &camera(), &opts, &[], W, H)
        .unwrap();
    assert!(
        near(centre(&plain), [255, 255, 255], 2),
        "{:?}",
        centre(&plain)
    );
    scene.fog = Some(FOG);
    let (fogged, _) = r
        .render_frame_offscreen(&ctx, &scene, &camera(), &opts, &[], W, H)
        .unwrap();
    // f = (200 - 100) / 200 = 0.5: half white, half blue
    assert!(
        near(centre(&fogged), [128, 128, 255], 3),
        "{:?}",
        centre(&fogged)
    );
}

#[test]
fn lightmapped_wall_past_the_far_plane_shows_only_the_fog_colour() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut mesh = SceneMesh {
        textures: vec![b"t".to_vec()],
        ..SceneMesh::default()
    };
    mesh.lightmaps.push(LightmapImage {
        width: 2,
        height: 2,
        rgba: [100u8, 150, 200, 255].repeat(4),
    });
    // the lightmap pass and the multiply pass over the same quad, 300 units away (past fog far = 200)
    let a = quad(&mut mesh, 300.0, [255; 4]);
    let b = quad(&mut mesh, 300.0, [255; 4]);
    mesh.batches.push(batch(14, a, None, Some(0)));
    mesh.batches.push(batch(15, b, Some(0), None));
    let tex = |_: &[u8]| Some(TextureImage::rgba(2, 2, [60u8, 90, 30, 255].repeat(4)));
    let mut scene = r.upload(&ctx, &mesh, &mut { tex });
    let opts = FrameOptions::opaque_only();
    let (plain, _) = r
        .render_frame_offscreen(&ctx, &scene, &camera(), &opts, &[], W, H)
        .unwrap();
    // 2 x lightmap x texture: (2*100*60, 2*150*90, 2*200*30) / 255 = (47, 106, 47)
    assert!(
        near(centre(&plain), [47, 106, 47], 3),
        "{:?}",
        centre(&plain)
    );
    scene.fog = Some(FOG);
    let (fogged, _) = r
        .render_frame_offscreen(&ctx, &scene, &camera(), &opts, &[], W, H)
        .unwrap();
    // full fog: the lightmap pass gives the fog colour, the multiply pass the neutral grey: 2 x fog x 0.5 = fog
    assert!(
        near(centre(&fogged), [0, 0, 255], 3),
        "{:?}",
        centre(&fogged)
    );
}

#[test]
fn gamma_stage_copies_through_the_engine_ramp() {
    let Some(ctx) = context() else {
        return;
    };
    let format = OFFSCREEN_FORMAT;
    let mut stage = GammaStage::new(&ctx, format, 1.5);
    assert!(stage.active());
    stage.prepare(&ctx, (W, H));
    let dst = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gamma test screen"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let dst_view = dst.create_view(&Default::default());
    let level = 64.0 / 255.0;
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    {
        let _clear = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gamma test source"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: stage.view().unwrap(),
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: level,
                        g: 0.0,
                        b: 1.0,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
    }
    stage.encode(&mut enc, &dst_view);
    let row = (4 * W).div_ceil(256) * 256;
    let buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gamma test read"),
        size: (row * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &dst,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue.submit([enc.finish()]);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = ctx.device.poll(wgpu::PollType::Wait);
    let data = slice.get_mapped_range();
    let o = ((H / 2) * row + (W / 2) * 4) as usize;
    let got = [data[o], data[o + 1], data[o + 2]];
    let mut want = vec![64u8, 0, 255, 255];
    apply_ramp(&mut want, &ramp_for_gamma(1.5));
    assert!(
        (got[0] as i32 - want[0] as i32).abs() <= 1 && got[1] == 0 && got[2] == 255,
        "{got:?} vs {want:?}"
    );
    assert!(got[0] > 64, "the ramp lifts the dark red");
}
