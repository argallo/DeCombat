//! gpu-lost-menu: the menu's painter is lent to the match with the window; when the match's recovery
//! (`window::App::rebuild_gpu`) replaces the window's device, the painter handed back must be made again on the new
//! device before the menu (match result screen, lobby) draws with it. The user's crash of 2026-11-04: two in-match
//! recoveries, then `Texture[Id(..)] does not exist` in wgpu-core on the first menu frame after the match.
//! Headless: device A stands for the menu's device before the match, device B for the device the recovery built
//! (a new wgpu instance, as the recovery makes), `gpulost::device_replaced` for the recovery's adoption of B, and
//! `MenuPainter::for_device` for the menu's return path (`menu::window::return_live`). Skipped without a GPU adapter.
// ORIGINAL: our own test of the device recovery's menu return (gpu-lost-menu); no 2009 behaviour

use dc_client::gpulost;
use dc_client::menu::link::LinkOptions;
use dc_client::menu::paint::MenuPainter;
use dc_client::menu::popup::{BoxTag, Popup};
use dc_client::menu::{Go, Menu, MenuOptions};
use dc_client::prof::Laps;
use dc_client::testkit::synth_font;
use dc_render::{AdapterChoice, GpuContext, OFFSCREEN_FORMAT};

const W: u32 = 320;
const H: u32 = 240;

fn gpu() -> Option<GpuContext> {
    match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => Some(c),
        Err(e) => {
            println!("skipped: no GPU adapter ({e})");
            None
        }
    }
}

fn menu() -> Menu {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: LinkOptions::new("gpu-lost-menu"),
        rom: None,
        font: Some((synth_font(), 0)),
        width: W,
        height: H,
    });
    m.detach_link();
    m.data.result = Some(dc_client::matchexit::MatchResult::sample());
    m
}

/// One menu frame as the menu window draws it (prepare, encode, submit), into an offscreen target, read back.
fn frame(ctx: &GpuContext, p: &mut MenuPainter, m: &mut Menu) -> Vec<u8> {
    p.prepare(ctx, m, &mut Laps::start());
    let tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gpu lost menu test"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OFFSCREEN_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    let row = (4 * W).div_ceil(256) * 256;
    let buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gpu lost menu read"),
        size: (row * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    p.encode(ctx, &mut enc, &view, (W, H));
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
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
    let mut out = Vec::with_capacity((4 * W * H) as usize);
    for y in 0..H {
        let o = (y * row) as usize;
        out.extend_from_slice(&data[o..o + (4 * W) as usize]);
    }
    out
}

#[test]
fn the_menu_draws_after_one_or_two_in_match_device_recoveries() {
    let Some(a) = gpu() else {
        return;
    };
    // the lobby on the menu's first device, then the painter is lent to the match
    let mut m = menu();
    m.open(Go::Lobby);
    // a message box: drawn text (the synthetic font) and fills without a ROM, so the frame is not all black
    m.show_popup(Popup::ok("match over".into(), BoxTag::None));
    let mut p = MenuPainter::new(&a, OFFSCREEN_FORMAT);
    let before = frame(&a, &mut p, &mut m);
    let lit = |img: &[u8]| {
        img.chunks_exact(4)
            .filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8)
            .count()
    };
    assert!(lit(&before) > 0, "the box draws something");

    // a match without a device loss hands the same painter back: nothing is made again (no cost)
    let p_builds = p.builds;
    let mut p = p.for_device(&a, OFFSCREEN_FORMAT);
    assert_eq!(p.builds, p_builds, "an unchanged device keeps the painter");
    m.set_size(W, H);
    frame(&a, &mut p, &mut m);

    // the match's recovery: a new instance and device, adopted by the window
    let Some(b) = gpu() else {
        return;
    };
    gpulost::device_replaced();
    assert_ne!(p.device_gen(), gpulost::device_generation());
    // back to the menu on the new device: the same screen draws the same frame, the result screen draws too
    let mut p = p.for_device(&b, OFFSCREEN_FORMAT);
    assert_eq!(p.device_gen(), gpulost::device_generation());
    assert_eq!(p.builds, 0, "a new painter on the new device");
    m.set_size(W, H);
    assert!(
        frame(&b, &mut p, &mut m) == before,
        "the menu draws the same frame on the new device"
    );
    assert!(p.builds > 0, "the quads were built on the new device");
    m.open(Go::Result);
    frame(&b, &mut p, &mut m);

    // the next match loses the device again (a second recovery): the menu draws once more
    let Some(c) = gpu() else {
        return;
    };
    gpulost::device_replaced();
    let mut p = p.for_device(&c, OFFSCREEN_FORMAT);
    m.open(Go::Lobby);
    m.show_popup(Popup::ok("match over".into(), BoxTag::None));
    m.set_size(W, H);
    assert!(
        frame(&c, &mut p, &mut m) == before,
        "the menu after a second recovery"
    );

    // the old devices' objects were put away, never dropped (lost devices deadlock in wgpu-core's drop paths)
    assert!(gpulost::retired() >= 2);
    std::mem::forget((a, b));
}
