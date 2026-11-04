//! CLI-1095 (playtest report: "my character still doesn't appear in the shop screen"): the launcher runs dc-client with
//! `--profile`, which switches the offscreen frames to the profiler's in-flight path: an empty image unless
//! `DC_PROF_READBACK` is set, so the shop's character view got no pixels. Its device also must not take over the
//! window device's GPU timer. With the profiler on, the shop pane still shows the character.
//!
//! ROM-gated (skipped without `CA_ROM`) and GPU-gated (skipped without an adapter). Its own test binary: the
//! profiler switch is process-wide.

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::LinkOptions;
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};
use dc_client::steps::{resolve, Resolved};

fn click(m: &mut Menu, name: &str) {
    let Resolved::Part { at, .. } = resolve(m, name) else {
        panic!("{name}: {:?}", resolve(m, name));
    };
    m.input(MenuInput::Move(at));
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
}

#[test]
fn rom_the_shop_figure_renders_while_the_profiler_times_the_window() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    // the window: profiler on with GPU timestamps, the window's device made first (it installs the timer) and a
    // frame being recorded
    let report =
        std::env::temp_dir().join(format!("dc_shop_model_prof_{}.json", std::process::id()));
    dc_client::prof::enable(report);
    assert!(dc_client::prof::on());
    let Ok(window) = dc_render::GpuContext::headless(dc_render::AdapterChoice::Auto) else {
        eprintln!("skipped: no GPU adapter");
        return;
    };
    if !window
        .device
        .features()
        .contains(wgpu::Features::TIMESTAMP_QUERY)
    {
        eprintln!("skipped: the adapter has no timestamps");
        return;
    }
    dc_render::perf::gpu_frame_begin(1);
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("t"),
        rom: Some(rom),
        font: None,
        width: 1024,
        height: 768,
    });
    m.detach_link();
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    let _ = m.image();
    let items = m.items();
    let tex = items
        .iter()
        .find_map(|i| match i {
            DrawItem::Image { tex, .. } if tex.starts_with("#shop-model/") => Some(tex.clone()),
            _ => None,
        })
        .expect("the shop pane draws the character model with the profiler on");
    let t = m.cat.assets.generated(&tex).expect("the rendered frame");
    let opaque = t.rgba.chunks(4).filter(|p| p[3] != 0).count();
    assert!(opaque > 5_000, "the character has pixels: {opaque}");
    // the window's timer is still the window's: a pass on its device takes its stamps
    assert!(!dc_render::perf::thread_untimed());
    drop(window);
}
