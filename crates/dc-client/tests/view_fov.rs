//! The field of view follows the 2009 rule: the vertical angle is fixed and the horizontal one widens with the
//! aspect ratio of the target. The synthetic test needs nothing; the ROM-gated one renders the JUNK_FLEA frame of
//! the 2009 captures at the camera the 2009 client had there (2560x1440, plus 1920x1080 for the comparison with the
//! 2009 frame) with the 2009 rule and with the old fixed 90 degrees horizontal, and writes the PNGs (ROM-derived,
//! never commit). Skipped without `CA_ROM` or a GPU adapter; nothing of the ROM is asserted by value.
// EVIDENCE: cshell+0x1f3cb0 (FovX = 2 atan(width / height * tan(FovY / 2)), FovY default 75 from cshell+0x1f6fb0)

use dc_client::offscreen::Offscreen;
use dc_client::world::GpuTarget;
use dc_client::{RomWorld, ViewAngles, ViewSettings, WorldSource};
use dc_render::{
    fov_y_for, AdapterChoice, Camera, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT,
};
use glam::{Vec3, Vec4};

/// Normalised screen x (-1..1) of the world point at `azimuth` radians right of the view axis, 100 units ahead.
fn ndc_x(c: &Camera, azimuth: f32) -> f32 {
    let p = c.pos + Vec3::new(azimuth.sin(), 0.0, azimuth.cos()) * 100.0;
    let v = c.view_projection() * Vec4::new(p.x, p.y, p.z, 1.0);
    v.x / v.w
}

#[test]
fn widescreen_sees_wider_than_four_to_three_at_the_same_height() {
    let view = ViewAngles::default();
    let eye = dc_sim::V3::new(0.0, 0.0, 0.0);
    let s = ViewSettings::default();
    let (c43, c169) = (
        s.camera(eye, &view, 1024, 768),
        s.camera(eye, &view, 2560, 1440),
    );
    // the old rule: 90 degrees horizontal at every aspect
    let old169 = {
        let fx = 90f32.to_radians();
        Camera::looking(Vec3::ZERO, 0.0, 0.0, fx, fov_y_for(fx, 2560, 1440))
    };
    let az = 48f32.to_radians();
    // a point 48 degrees to the side: out of view at 4:3 and for the old 90 degrees, in view at 16:9 now
    assert!(ndc_x(&c43, az) > 1.0);
    assert!(ndc_x(&old169, az) > 1.0);
    assert!(ndc_x(&c169, az) < 1.0, "{}", ndc_x(&c169, az));
    // the vertical extent is the same at both aspects (a point 30 degrees up)
    let up = |c: &Camera| {
        let p = Vec3::new(0.0, 30f32.to_radians().tan(), 1.0) * 100.0;
        let v = c.view_projection() * Vec4::new(p.x, p.y, p.z, 1.0);
        v.y / v.w
    };
    assert!((up(&c43) - up(&c169)).abs() < 1e-4);
}

#[test]
fn rom_junk_flea_frame_with_the_2009_field_of_view() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let ctx = match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => c,
        Err(e) => {
            println!("skipped: no GPU adapter ({e})");
            return;
        }
    };
    let names = RomWorld::map_names(&rom).unwrap();
    if !names.iter().any(|n| n == "JUNK_FLEA") {
        println!("skipped: no JUNK_FLEA in this ROM");
        return;
    }
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut src = RomWorld::new(&rom, "JUNK_FLEA");
    let world = src
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))
        .unwrap();
    let scene = world.scene.as_ref().unwrap();
    // the camera of the 2009 frame (OBSERVED in the captures): a spot in the first room looking along -Z
    let eye = dc_sim::V3::new(588.0, 83.1, 932.0);
    let view = ViewAngles {
        yaw: std::f32::consts::PI,
        pitch: 0.0,
    };
    let out = dc_render::default_out_dir().join("dc-client");
    let _ = std::fs::create_dir_all(&out);
    for (w, h) in [(2560u32, 1440u32), (1920, 1080)] {
        let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, w, h);
        let new = ViewSettings::default().camera(eye, &view, w, h);
        let old = {
            let fx = 90f32.to_radians();
            let mut c = Camera::looking(
                Vec3::new(eye.x, eye.y, eye.z),
                view.yaw,
                view.pitch,
                fx,
                fov_y_for(fx, w, h),
            );
            c.far_z = new.far_z;
            c
        };
        for (name, cam) in [("fov2009", &new), ("fov90h", &old)] {
            let img = target
                .render_with(&ctx, &renderer, scene, cam, None, &[], None)
                .unwrap();
            let lit = (0..h)
                .step_by(8)
                .any(|y| (0..w).step_by(8).any(|x| img.pixel(x, y)[..3] != [0, 0, 0]));
            assert!(lit, "{name} {w}x{h}: the frame is not empty");
            let _ = img.save_png(&out.join(format!("junk_flea_{name}_{w}x{h}.png")));
        }
        println!(
            "{w}x{h}: new fov_x {:.2} fov_y {:.2}; old fov_x {:.2} fov_y {:.2}",
            new.fov_x.to_degrees(),
            new.fov_y.to_degrees(),
            old.fov_x.to_degrees(),
            old.fov_y.to_degrees()
        );
    }
    println!("PNGs in {} (ROM-derived, never commit)", out.display());
}
