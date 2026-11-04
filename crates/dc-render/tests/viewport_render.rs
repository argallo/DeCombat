//! The editor viewport's render-to-texture entry (`WorldRenderer::render_frame_to_target` into an
//! `OffscreenTarget`, MAP-27) on whatever headless adapter exists: the frame it leaves on the GPU is, pixel for
//! pixel, the frame the read-back path returns, with MSAA off and on, before and after a resize. All content is
//! invented here. Skips cleanly when no adapter exists (CI without a GPU).

use dc_render::*;
use dc_world::synth::{SynthBlock, SynthRenderWorld, SynthSection, SynthWorld};
use dc_world::World;
use glam::Vec3;

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

/// Two vertex-lit quads facing a camera at the origin looking along +Z: a grey one at depth 100 and a taller
/// red one behind it at depth 200.
fn synth_world() -> Vec<u8> {
    let mut verts = Vec::new();
    let mut tris = Vec::new();
    let mut quad = |x0: f32, x1: f32, z: f32, hy: f32, colour: u32| {
        let b = verts.len() as u32;
        verts.push(vtx([x0, -hy, z], [0.25, 0.75], colour));
        verts.push(vtx([x0, hy, z], [0.25, 0.25], colour));
        verts.push(vtx([x1, hy, z], [0.75, 0.25], colour));
        verts.push(vtx([x1, -hy, z], [0.75, 0.75], colour));
        tris.push([b, b + 1, b + 2, 0]);
        tris.push([b, b + 2, b + 3, 0]);
    };
    quad(-20.0, 20.0, 100.0, 20.0, 0xff80_8080);
    quad(-30.0, 30.0, 200.0, 60.0, 0xffff_ffff);
    let section = |tex: &[u8]| SynthSection {
        textures: [tex.to_vec(), Vec::new()],
        shader_kind: 1,
        triangles: 2,
        ..SynthSection::default()
    };
    let block = SynthBlock {
        center: [0.0, 0.0, 150.0],
        half: [100.0, 80.0, 100.0],
        sections: vec![section(b"own/base"), section(b"own/red")],
        vertices: verts,
        triangles: tris,
        ..SynthBlock::default()
    };
    SynthWorld {
        extent_min: [-300.0; 3],
        extent_max: [300.0; 3],
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

fn camera(w: u32, h: u32) -> Camera {
    let fov_x = 90f32.to_radians();
    Camera::looking(Vec3::ZERO, 0.0, 0.0, fov_x, fov_y_for(fov_x, w, h))
}

/// Renders the scene through the read-back path and through the target; both frames must match byte for byte.
fn same_frame(
    ctx: &GpuContext,
    renderer: &WorldRenderer,
    scene: &GpuScene,
    target: &OffscreenTarget,
) -> FrameImage {
    let (w, h) = target.size();
    let cam = camera(w, h);
    let opts = FrameOptions::default();
    let (want, _) = renderer
        .render_frame_offscreen(ctx, scene, &cam, &opts, &[], w, h)
        .expect("read-back frame");
    let report = renderer.render_frame_to_target(
        ctx,
        scene,
        &cam,
        &opts,
        &[],
        &[],
        &PlayerViewSettings::default(),
        &[],
        target,
    );
    assert_eq!(report.gpu_ms, 0.0, "no wait on the viewport path");
    assert!(report.world_draws >= 1, "{report:?}");
    let got = target.read_back(ctx).expect("target read-back");
    assert_eq!((got.width, got.height), (w, h));
    assert_eq!(got.rgba.len(), (4 * w * h) as usize);
    assert!(
        got.rgba == want.rgba,
        "viewport frame differs from the read-back frame at {w}x{h}, msaa {}",
        renderer.msaa()
    );
    // the frame is not empty: the grey quad covers the centre (vertex-lit x2 with vertex colour 0x80 = unit
    // brightness: the texture's 200), the clear colour the corner. Not at 1 x 1: there the multisample
    // positions (up to 0.75 of the one pixel from its centre) lie outside the quad, so only the equality above
    // says anything.
    if (w, h) != (1, 1) {
        let centre = got.pixel(w / 2, h / 2);
        let clear = CLEAR_COLOUR.map(|c| (c * 255.0).round() as u8);
        assert!(
            (0..3).all(|k| (centre[k] as i32 - 200).abs() <= 4),
            "centre {centre:?} at {w}x{h}, msaa {}",
            renderer.msaa()
        );
        assert!(
            (0..3).all(|k| (got.pixel(1, 1)[k] as i32 - clear[k] as i32).abs() <= 2),
            "corner {:?}",
            got.pixel(1, 1)
        );
    }
    got
}

fn check(ctx: &GpuContext, msaa: u32) {
    let data = synth_world();
    let world = World::parse(&data).expect("synthetic world parses");
    let mesh = SceneMesh::build(&world, SceneOptions::default(), &mut |_| TextureTraits {
        present: true,
        mode: 0,
        hi: false,
    });
    let settings = RenderSettings {
        msaa,
        ..RenderSettings::default()
    };
    let renderer = WorldRenderer::new(ctx, OFFSCREEN_FORMAT, settings);
    assert_eq!(renderer.msaa(), msaa);
    let scene = renderer.upload(ctx, &mesh, &mut |name| match name {
        b"own/base" => Some(solid([200, 200, 200, 255])),
        b"own/red" => Some(solid([255, 0, 0, 255])),
        _ => None,
    });
    assert_eq!(scene.stats.missing_textures, 0);

    let mut target = OffscreenTarget::for_renderer(ctx, &renderer, (96, 64));
    assert_eq!(target.format(), OFFSCREEN_FORMAT);
    assert_eq!(target.format(), renderer.colour_format());
    assert_eq!(target.samples(), msaa);
    assert_eq!(target.size(), (96, 64));
    assert_eq!(target.generation(), 0);
    // the colour texture can be bound by a sampler (the egui paint callback) and copied out (screenshots)
    let usage = target.texture().usage();
    assert!(
        usage.contains(wgpu::TextureUsages::TEXTURE_BINDING),
        "{usage:?}"
    );
    assert!(
        usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT),
        "{usage:?}"
    );
    assert!(usage.contains(wgpu::TextureUsages::COPY_SRC), "{usage:?}");
    assert_eq!(target.texture().sample_count(), 1);
    assert_eq!(target.texture().format(), OFFSCREEN_FORMAT);
    // a bind group over the view is accepted by the device (what the UI toolkit does with it)
    let layout = ctx
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport sample"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
    let _group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("viewport sample"),
        layout: &layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(target.view()),
        }],
    });

    let first = same_frame(ctx, &renderer, &scene, &target);
    // the same size again: nothing remade; a new size: remade, drawn again at that size
    assert!(!target.resize(ctx, (96, 64)));
    assert_eq!(target.generation(), 0);
    assert!(target.resize(ctx, (128, 80)));
    assert_eq!(target.generation(), 1);
    assert_eq!(target.size(), (128, 80));
    let second = same_frame(ctx, &renderer, &scene, &target);
    assert_ne!((first.width, first.height), (second.width, second.height));
    // a degenerate size is clamped to 1 x 1 and still renders
    assert!(target.resize(ctx, (0, 0)));
    assert_eq!(target.size(), (1, 1));
    let _ = same_frame(ctx, &renderer, &scene, &target);
    if let Ok(dir) = std::env::var("DC_RENDER_OUT") {
        second
            .save_png(&std::path::Path::new(&dir).join(format!("viewport_msaa{msaa}.png")))
            .expect("png");
    }
}

#[test]
fn viewport_target_matches_the_read_back_frame() {
    let Some(ctx) = context() else {
        return;
    };
    check(&ctx, 1);
}

/// With 4 samples (the WebGPU-guaranteed count) the target holds the multisampled attachments and the frame's
/// last pass resolves into the colour texture, as the read-back path does.
#[test]
fn viewport_target_resolves_msaa_like_the_read_back_frame() {
    let Some(ctx) = context() else {
        return;
    };
    if !supported_msaa(&ctx.adapter, OFFSCREEN_FORMAT).contains(&4) {
        println!("skipped: the adapter offers no 4x MSAA for {OFFSCREEN_FORMAT:?}");
        return;
    }
    check(&ctx, 4);
}
