//! CLI-1295 (user 2026-10-29: "the character in shop when you rotate them lags significantly"): a steady drag on the
//! shop character at the menu cap (360 frames a second, 2 virtual px a frame) shows new renders while the pointer
//! moves. Before, each move asked for a new yaw and the render in flight was dropped as stale: 974 requests, 0 renders
//! shown in a 3 s drag (release, 2560 x 1440); after, 972 shown, the shown yaw 1 frame behind (2 px mean, 4 px max).
//! Prints the numbers (`--nocapture`). ROM- and GPU-gated.

use std::time::{Duration, Instant};

use dc_client::menu::link::LinkOptions;
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};

#[test]
fn rom_a_steady_drag_shows_new_renders_while_the_pointer_moves() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("t"),
        rom: Some(rom),
        font: None,
        width: 2560,
        height: 1440,
    });
    m.detach_link();
    m.open(Go::Shop);
    let _ = m.image();
    if m.figure_renders() == 0 {
        eprintln!("skipped: no character view (no GPU adapter)");
        return;
    }
    let frame = Duration::from_micros(2778);
    let mut x = 20.0f32;
    m.input(MenuInput::Move([x, 300.0]));
    m.input(MenuInput::Press([x, 300.0]));
    let start = Instant::now();
    let (r0, q0) = (m.figure_renders(), m.figure_requests());
    let mut last_change = Instant::now();
    let mut shown = m.figure_shown_yaw();
    let mut max_gap = Duration::ZERO;
    let mut frames = 0u32;
    let mut behind_sum = 0.0f32;
    let mut behind_max = 0.0f32;
    let mut dir = 1.0f32;
    while start.elapsed() < Duration::from_secs(2) {
        let t = Instant::now();
        x += 2.0 * dir;
        if !(10.0..190.0).contains(&x) {
            dir = -dir;
        }
        m.input(MenuInput::Move([x, 300.0]));
        let _ = m.items();
        let now = m.figure_shown_yaw();
        if now != shown {
            max_gap = max_gap.max(last_change.elapsed());
            last_change = Instant::now();
            shown = now;
        }
        let want = m.figure_request().unwrap().place.yaw;
        let d = (want - now.unwrap_or(want)).rem_euclid(std::f32::consts::TAU);
        let d = d.min(std::f32::consts::TAU - d) * 60.0 * 1024.0 / 2560.0; // virtual px behind
        behind_sum += d;
        behind_max = behind_max.max(d);
        frames += 1;
        if let Some(r) = frame.checked_sub(t.elapsed()) {
            std::thread::sleep(r);
        }
    }
    max_gap = max_gap.max(last_change.elapsed());
    eprintln!(
        "DRAGLAG frames {frames} in {:?}; requests {} renders shown {}; max gap between shown renders {:?}; \
         trail mean {:.1} max {:.1} virtual px",
        start.elapsed(),
        m.figure_requests() - q0,
        m.figure_renders() - r0,
        max_gap,
        behind_sum / frames as f32,
        behind_max
    );
    assert!(
        m.figure_renders() - r0 >= 5,
        "the character turns while the pointer moves, not only when the drag pauses"
    );
}
