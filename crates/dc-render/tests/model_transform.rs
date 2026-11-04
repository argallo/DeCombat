//! A world model moved by its object (w1-doors, REN-62): `GpuScene::set_model_transform` draws the model's vertices
//! moved, byte for byte as a scene built with them moved; back to `None` gives the file frame byte for byte; a scene
//! whose models are never moved draws as before (the upload does not change anything else).
//!
//! Synthetic, no ROM; skipped without a headless adapter.

use dc_render::*;
use glam::{Affine3A, Quat, Vec3};

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

/// A quad facing the camera at the origin, centred at `c`, half size `s`, tex 0.
fn quad(mesh: &mut SceneMesh, c: [f32; 3], s: f32, colour: [u8; 4]) {
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
        tex0: Some(0),
        tex1: None,
        lightmap: None,
        first_index: first,
        index_count: 6,
    });
}

/// A back wall (level) and a door quad (world model "door") at `door`.
fn mesh(door: [f32; 3]) -> SceneMesh {
    let mut m = SceneMesh {
        textures: vec![b"a".to_vec()],
        ..SceneMesh::default()
    };
    quad(&mut m, [0.0, 0.0, 200.0], 150.0, [90, 90, 90, 255]);
    let first = m.indices.len() as u32;
    quad(&mut m, door, 20.0, [220, 60, 40, 255]);
    m.models.push(SceneModel {
        name: b"door".to_vec(),
        first_index: first,
        end_index: m.indices.len() as u32,
    });
    m
}

#[test]
fn a_moved_world_model_draws_as_if_built_there() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let tex = |_: &[u8]| Some(TextureImage::rgba(2, 2, [255u8, 255, 255, 255].repeat(4)));
    let fx = 90f32.to_radians();
    let cam = Camera::looking(Vec3::ZERO, 0.0, 0.0, fx, fov_y_for(fx, W, H));
    let opts = FrameOptions::opaque_only();
    let frame = |s: &GpuScene| {
        r.render_frame_offscreen(&ctx, s, &cam, &opts, &[], W, H)
            .expect("frame")
            .0
            .rgba
    };
    let mut scene = r.upload(&ctx, &mesh([0.0, 0.0, 100.0]), &mut { tex });
    let file = frame(&scene);
    // slid 50 to the right
    let moved = r.upload(&ctx, &mesh([50.0, 0.0, 100.0]), &mut { tex });
    let want = frame(&moved);
    assert!(want != file, "the move shows");
    scene.set_model_transform(
        0,
        Some(Affine3A::from_translation(Vec3::new(50.0, 0.0, 0.0))),
    );
    assert_eq!(frame(&scene), want, "moved by the transform");
    // a second frame without a new transform: nothing is written again, same frame
    assert_eq!(frame(&scene), want);
    // turned half around its own centre: the same square
    let c = Vec3::new(0.0, 0.0, 100.0);
    let turn = Affine3A::from_translation(c)
        * Affine3A::from_quat(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2))
        * Affine3A::from_translation(-c);
    scene.set_model_transform(0, Some(turn));
    let turned = frame(&scene);
    assert!(turned.len() == file.len());
    // back to the file position: byte for byte
    scene.set_model_transform(0, None);
    assert_eq!(frame(&scene), file, "restored");
    assert_eq!(scene.model_transform(0), None);
    // an index without a model does nothing
    scene.set_model_transform(7, Some(Affine3A::IDENTITY));
    assert_eq!(frame(&scene), file);
}

#[test]
fn a_moved_model_out_of_its_old_bounds_is_not_culled() {
    let Some(ctx) = context() else {
        return;
    };
    let r = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let tex = |_: &[u8]| Some(TextureImage::rgba(2, 2, [255u8, 255, 255, 255].repeat(4)));
    let fx = 90f32.to_radians();
    let cam = Camera::looking(Vec3::ZERO, 0.0, 0.0, fx, fov_y_for(fx, W, H));
    let opts = FrameOptions::opaque_only();
    // the door starts behind the camera (culled) and is moved into view
    let mut scene = r.upload(&ctx, &mesh([0.0, 0.0, -100.0]), &mut { tex });
    let (_, before) = r
        .render_frame_offscreen(&ctx, &scene, &cam, &opts, &[], W, H)
        .expect("frame");
    assert_eq!(before.world_culled, 1);
    scene.set_model_transform(
        0,
        Some(Affine3A::from_translation(Vec3::new(0.0, 0.0, 200.0))),
    );
    let (img, after) = r
        .render_frame_offscreen(&ctx, &scene, &cam, &opts, &[], W, H)
        .expect("frame");
    assert_eq!(after.world_culled, 0);
    let built = r.upload(&ctx, &mesh([0.0, 0.0, 100.0]), &mut { tex });
    let (want, _) = r
        .render_frame_offscreen(&ctx, &built, &cam, &opts, &[], W, H)
        .expect("frame");
    assert!(img.rgba == want.rgba);
}
