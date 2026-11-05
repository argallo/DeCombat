//! ROM-gated (skipped when CA_ROM is unset): the far plane of the view and its independence of the graphics backend.
//!
//! - Two Towers and Waverider ask, in their world settings, for far planes past the 12000 default (CLI-1851): the
//!   viewer's camera takes the world's value, and at least one view of each map draws differently with it than
//!   with the default (the distant sea and the fogged boundary walls the default far plane cut).
//! - The same views on Vulkan and on DX12 record the same world draws, culled batches and sky draws, and the
//!   frames agree (each backend is skipped when the machine has no adapter for it; the comparison needs both).
//!
//! Nothing of the ROM is written; no image is kept.

use dc_render::*;

const W: u32 = 320;
const H: u32 = 180;
const MAPS: [&str; 2] = ["TWOTOWERS", "WAVERIDER"];
const YAWS: [f32; 4] = [0.0, 90.0, 180.0, 270.0];

fn request(map: &str, yaw: f32, world_far: bool) -> ViewRequest {
    let mut req = ViewRequest::new(map);
    req.camera = CameraSpec::Spawn(0);
    req.look_override = Some((yaw, 0.0));
    req.eye_lift = 60.0;
    req.width = W;
    req.height = H;
    req.world_far = world_far;
    req
}

fn viewer(rom: &std::path::Path, adapter: AdapterChoice) -> Option<RomViewer> {
    match RomViewer::open(rom, adapter, RenderSettings::default()) {
        Ok(v) => Some(v),
        Err(e) => {
            println!("skipped {adapter:?}: {e}");
            None
        }
    }
}

/// Pixels whose largest channel difference is above 8.
fn differing(a: &FrameImage, b: &FrameImage) -> usize {
    a.rgba
        .chunks(4)
        .zip(b.rgba.chunks(4))
        .filter(|(x, y)| (0..3).any(|i| x[i].abs_diff(y[i]) > 8))
        .count()
}

#[test]
fn the_camera_takes_the_world_far_plane() {
    let rom = dc_rom::rom_or_skip!();
    let src = WorldSource::open(&rom).expect("worlds");
    let Some(v) = viewer(&rom, AdapterChoice::Auto) else {
        return;
    };
    for map in MAPS {
        let bytes = src.read(map).expect("read").expect("world present");
        let far = world_far_z(&dc_world::World::parse(&bytes).expect("parse"))
            .unwrap_or_else(|| panic!("{map}: no far plane in the world settings"));
        assert!(far > DEFAULT_FAR_Z, "{map}: far plane {far}");
        let p = v.prepare(&request(map, 0.0, true)).expect("prepare");
        assert_eq!(p.camera.far(), far, "{map}: camera far plane");
        let p = v.prepare(&request(map, 0.0, false)).expect("prepare");
        assert_eq!(p.camera.far(), DEFAULT_FAR_Z, "{map}: settings far plane");
        let mut changed = 0;
        for yaw in YAWS {
            let (a, _) = v.render(&request(map, yaw, true)).expect("render");
            let (b, _) = v.render(&request(map, yaw, false)).expect("render");
            let n = differing(&a, &b);
            println!(
                "{map} yaw {yaw}: {n} pixels differ between the world far plane and the default"
            );
            changed += usize::from(n > 0);
        }
        assert!(changed > 0, "{map}: the world far plane changed no view");
    }
}

#[test]
fn the_drawn_sections_do_not_depend_on_the_backend() {
    let rom = dc_rom::rom_or_skip!();
    let viewers: Vec<RomViewer> = [wgpu::Backends::VULKAN, wgpu::Backends::DX12]
        .into_iter()
        .filter_map(|b| viewer(&rom, AdapterChoice::Backends(b)))
        .collect();
    if viewers.len() < 2 {
        println!("skipped: needs a Vulkan and a DX12 adapter");
        return;
    }
    for map in MAPS {
        for yaw in YAWS {
            let req = request(map, yaw, true);
            let (a, ra) = viewers[0].render(&req).expect("vulkan render");
            let (b, rb) = viewers[1].render(&req).expect("dx12 render");
            let (fa, fb) = (&ra.frame, &rb.frame);
            println!(
                "{map} yaw {yaw}: world draws {} / {}, culled {} / {}, sky draws {} / {}",
                fa.world_draws,
                fb.world_draws,
                fa.world_culled,
                fb.world_culled,
                fa.sky_draws,
                fb.sky_draws
            );
            assert_eq!(
                (fa.world_draws, fa.world_culled, fa.world_tris),
                (fb.world_draws, fb.world_culled, fb.world_tris),
                "{map} yaw {yaw}: world batches"
            );
            assert_eq!(
                (fa.sky_draws, fa.sky_rect),
                (fb.sky_draws, fb.sky_rect),
                "{map} yaw {yaw}: sky pass"
            );
            assert_eq!(fa.blended_objects, fb.blended_objects, "{map} yaw {yaw}");
            // the same geometry; rasterisation and filtering may differ in the last bits between drivers
            let n = differing(&a, &b);
            assert!(
                n * 1000 <= (W * H) as usize,
                "{map} yaw {yaw}: {n} pixels differ between Vulkan and DX12"
            );
        }
    }
}
