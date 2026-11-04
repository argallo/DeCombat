//! net-overlay (CLI-1570..1574): the network overlay drawn into a headless HUD frame, on and off, with fed numbers.
//!
//! Structural without a ROM (the fallback HUD); with `CA_ROM` the ROM HUD is drawn too and a PNG of the overlay is
//! written (ROM-derived, never committed) to `DC_NET_OVERLAY_PNG` when set, else the render out dir.

use dc_client::hud::chat::ChatPress;
use dc_client::hud::netview::{self, Extrap, NetSample, PlayerLink};
use dc_client::hud::state::BoardRow;
use dc_client::hud::{DrawItem, Hud, HudDriver, HudState};
use dc_client::net::meter::MeterView;

fn sample() -> NetSample {
    NetSample {
        meter: MeterView {
            rtt_mean: Some(62.0),
            rtt_min: Some(57.0),
            rtt_max: Some(71.0),
            jitter: Some(3.4),
            rtt_samples: 40,
            loss_in: Some(0.0),
            loss_out: Some(0.8),
            pps_in: 51.0,
            pps_out: 29.0,
            kbps_in: 38.6,
            kbps_out: 11.2,
            tick_gap_mean: Some(33.4),
            tick_gap_max: Some(51),
            since_update: Some(14),
            late_frames: 2,
            held_frames: 0,
        },
        delay_ms: Some(52.0),
        depth: Some(1.6),
        lobby: Some("127.0.0.1:5001".into()),
        server: Some("127.0.0.1:27015".into()),
        players: vec![
            PlayerLink {
                pid: 2,
                name: "Hunter1".into(),
                age_ms: Some(18),
                state: Extrap::Interp,
            },
            PlayerLink {
                pid: 3,
                name: "Hunter2".into(),
                age_ms: Some(140),
                state: Extrap::Extrap,
            },
        ],
    }
}

fn state(mode: u8) -> HudState {
    let mut s = HudState {
        own_pid: Some(1),
        ..HudState::default()
    };
    for (pid, ping) in [(1u8, 64u16), (2, 70), (3, 95)] {
        s.roster.insert(
            pid,
            BoardRow {
                pid,
                ping: Some(ping),
                ..BoardRow::default()
            },
        );
    }
    s.net_mode = mode;
    s.net_lines = netview::lines(Some(&sample()), &s, 240.0, Some(125.0), mode);
    s
}

fn overlay_items(items: &[DrawItem]) -> (Vec<[f32; 4]>, Vec<(String, [f32; 2])>) {
    let fills = items
        .iter()
        .filter_map(|i| match i {
            DrawItem::Fill { rect, colour } if *colour == netview::BACKGROUND => Some(*rect),
            _ => None,
        })
        .collect();
    let texts = items
        .iter()
        .filter_map(|i| match i {
            DrawItem::Text { style, pos, text, .. } if style.size == Some(netview::CELL) => {
                Some((text.clone(), *pos))
            }
            _ => None,
        })
        .collect();
    (fills, texts)
}

fn check(hud: &mut Hud) {
    // off: nothing of the overlay
    let off = hud.items(&state(netview::OFF));
    let (f, t) = overlay_items(&off);
    assert!(f.is_empty() && t.is_empty(), "off draws nothing");
    // on: one background at the top left, the lines in order under it
    let on = hud.items(&state(netview::LINK));
    let (f, t) = overlay_items(&on);
    assert_eq!(f.len(), 1);
    let r = f[0];
    assert!(r[0] < 20.0 && r[1] < 20.0, "top left: {r:?}");
    assert!(r[2] > 100.0 && r[2] < 600.0, "width {r:?}");
    let labels: Vec<&str> = t
        .iter()
        .map(|(s, _)| s.split_whitespace().next().unwrap_or(""))
        .collect();
    assert_eq!(
        labels,
        ["ping", "felt", "jitter", "loss", "in", "server", "playout", "late", "fps", "mouse", "last", "lobby"]
    );
    assert_eq!(t[0].0, "ping 62 ms  min 57  max 71  (table 64)");
    assert_eq!(t[1].0, "felt 147 ms  (ping 62 + update gap 33 + playout 52)");
    for (i, (_, p)) in t.iter().enumerate() {
        assert!(p[0] >= r[0] && p[1] >= r[1] && p[1] < r[1] + r[3], "line {i} inside the panel");
    }
    assert!(t.windows(2).all(|w| w[1].1[1] > w[0].1[1]), "lines go down");
    // the players block
    let both = hud.items(&state(netview::PLAYERS));
    let (_, t2) = overlay_items(&both);
    assert_eq!(t2.len(), t.len() + 3);
    assert_eq!(t2[t.len() + 1].0, "Hunter1  70 ms  18 ms  interp");
    assert_eq!(t2[t.len() + 2].0, "Hunter2  95 ms  140 ms  extrap");
}

#[test]
fn overlay_draws_top_left_on_and_nothing_off() {
    let mut hud = Hud::fallback(1024, 768, None);
    check(&mut hud);
}

#[test]
fn chat_command_turns_the_overlay_on_and_off_without_sending() {
    let mut d = HudDriver::new(Hud::fallback(1024, 768, None));
    let mut say = |d: &mut HudDriver, line: &str| {
        d.chat_press(ChatPress::Enter);
        d.chat_press(ChatPress::Text(line.into()));
        d.chat_press(ChatPress::Enter)
    };
    let o = say(&mut d, "/network");
    assert!(o.send.is_none());
    assert_eq!(d.state.net_mode, netview::LINK);
    say(&mut d, "/network 2");
    assert_eq!(d.state.net_mode, netview::PLAYERS);
    say(&mut d, "/network what");
    assert_eq!(d.state.net_mode, netview::PLAYERS, "unknown argument ignored");
    say(&mut d, "/net");
    assert_eq!(d.state.net_mode, netview::OFF);
    assert!(d.state.net_lines.is_empty());
    let o = say(&mut d, "gg");
    assert!(o.send.is_some(), "a plain line is still chat");
    assert_eq!(
        dc_client::settings::take_net_overlay_note(),
        Some(netview::OFF),
        "the last choice is noted for the profile"
    );
}

#[test]
fn rom_overlay_frame() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let mut hud = Hud::from_rom(&rom, 1024, 768, None).expect("ROM HUD");
    check(&mut hud);
    let mut hud = Hud::from_rom(&rom, 1600, 900, None).expect("ROM HUD");
    let img = hud.render(&state(netview::PLAYERS));
    let mut rgb = Vec::with_capacity((img.width * img.height * 3) as usize);
    for px in img.rgba.as_chunks::<4>().0 {
        let a = px[3] as u32;
        for c in &px[..3] {
            rgb.push(((*c as u32 * a + 70 * (255 - a)) / 255) as u8);
        }
    }
    let path = std::env::var_os("DC_NET_OVERLAY_PNG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| dc_render::default_out_dir().join("net_overlay.png"));
    if let Some(p) = path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let b = image::RgbImage::from_raw(img.width, img.height, rgb).expect("image");
    b.save(&path).expect("png");
    println!("wrote {}", path.display());
}
