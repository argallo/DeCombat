//! The menu screens drawn on the GPU ([`MenuPainter`], what the menu window draws) against the CPU image
//! ([`Menu::image`], what the headless PNGs keep using): every screen with made-up lobby data, the same pixels up to
//! the 8-bit rounding of the CPU's intermediate image. Also: inputs that change nothing on the screen rebuild no
//! quads, textures are uploaded once, a typed character uploads quads only.
//! GPU tests skip without an adapter; the ROM variant also needs `CA_ROM` (made-up data only, nothing of the ROM in
//! this file).

use dc_botclient::lobby::{LobbyProfile, RoomMember, ServerEntry};
use dc_client::hud::compose;
use dc_client::menu::link::{ChannelRow, LinkOptions, RoomRow, RoomView, UserRow};
use dc_client::menu::paint::MenuPainter;
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};
use dc_client::prof::Laps;
use dc_client::testkit::synth_font;
use dc_render::{AdapterChoice, GpuContext, OFFSCREEN_FORMAT};

const W: u32 = 800;
const H: u32 = 600;

fn gpu() -> Option<GpuContext> {
    match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => Some(c),
        Err(e) => {
            println!("skipped: no GPU adapter ({e})");
            None
        }
    }
}

fn menu(rom: Option<std::path::PathBuf>) -> Menu {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: LinkOptions::new("menu-paint"),
        rom,
        font: Some((synth_font(), 0)),
        width: W,
        height: H,
    });
    m.detach_link();
    let d = &mut m.data;
    d.servers = (0..3)
        .map(|i| ServerEntry {
            id: i + 1,
            name: format!("Server {}", i + 1),
            ip: "127.0.0.1".into(),
            port: 1,
            open: 1,
            capacity: 100,
            current: [10, 60, 97][i as usize],
            clan_only: 0,
            op_capacity: 0,
            op_current: 0,
        })
        .collect();
    d.server = d.servers.first().cloned();
    d.profile = LobbyProfile {
        nick: "tester".into(),
        char_no: 1,
        my_id: 1,
        chan_label: "Channel 1".into(),
        ..LobbyProfile::default()
    };
    d.channels = (0..3)
        .map(|i| ChannelRow {
            id: i,
            name: format!("Channel {}", i + 1),
            open: 1,
            capacity: 100,
            current: [5, 50, 90][i as usize],
            op_capacity: 0,
            op_current: 0,
        })
        .collect();
    d.rooms = (0..5)
        .map(|i| RoomRow {
            number: i,
            title: format!("room {i}"),
            locked: i == 1,
            map: (i % 3) as i32,
            players: 2,
            max_players: 8,
            ..RoomRow::default()
        })
        .collect();
    d.users = vec![UserRow {
        account: 1,
        id: 1,
        name: "tester".into(),
        exp: 0,
        ..Default::default()
    }];
    d.room = RoomView {
        title: "tester room".into(),
        host_id: 1,
        players: 2,
        max_players: 8,
        goal: 100,
        ..RoomView::default()
    };
    for i in 1..=2u32 {
        d.members.insert(
            i,
            RoomMember {
                char_no: i as i32,
                name: format!("P{i}"),
                team: i as i32,
                state: 0,
                exp: 0,
            },
        );
    }
    d.chat = vec!["P2 : hello".into()];
    d.result = Some(dc_client::matchexit::MatchResult::sample());
    m
}

/// Draws the painter's frame into an RGBA target and reads it back.
fn frame(ctx: &GpuContext, p: &MenuPainter) -> Vec<u8> {
    let tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("menu paint test"),
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
        label: Some("menu paint read"),
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

const SCREENS: [(&str, Go); 7] = [
    ("login", Go::Login),
    ("servers", Go::Servers),
    ("channels", Go::Channels),
    ("lobby", Go::Lobby),
    ("create", Go::Create),
    ("room", Go::Room),
    ("result", Go::Result),
];

/// (differing channels, max delta, channels above 3) of the GPU frame against the CPU image, colour only (the CPU
/// image is opaque; the frame's alpha is the target's).
fn compare(gpu: &[u8], cpu: &[u8]) -> (usize, u8, usize) {
    let mut n = 0;
    let mut max = 0u8;
    let mut over = 0;
    for (i, (a, b)) in gpu.iter().zip(cpu).enumerate() {
        if i % 4 == 3 {
            continue;
        }
        let d = a.abs_diff(*b);
        if d > 0 {
            n += 1;
            max = max.max(d);
        }
        if d > 3 {
            over += 1;
        }
    }
    (n, max, over)
}

fn screens_match(rom: Option<std::path::PathBuf>) {
    let Some(ctx) = gpu() else {
        return;
    };
    let mut m = menu(rom);
    let mut p = MenuPainter::new(&ctx, OFFSCREEN_FORMAT);
    for (name, g) in SCREENS {
        m.open(g);
        // the GPU frame, then the CPU compositor over the very display list the quads were built from (the caret
        // blinks on the menu's clock, so a second display list could differ); without a ROM two screens may draw
        // the same list (no rebuild then)
        p.prepare(&ctx, &mut m, &mut Laps::start());
        let got = frame(&ctx, &p);
        let items = p.drawn_items().expect("drawn").to_vec();
        let want = compose(&items, &mut m.cat.assets, W, H);
        assert_eq!((want.width, want.height), (W, H));
        let (n, max, over) = compare(&got, &want.rgba);
        println!(
            "{name}: {} quads, {n} channels differ, max delta {max}, above 3: {over}",
            p.quad_count()
        );
        assert_eq!(over, 0, "{name}: GPU frame departs from the CPU image");
    }
}

#[test]
fn menu_screens_on_the_gpu_match_the_cpu_image() {
    screens_match(None);
}

#[test]
fn rom_menu_screens_on_the_gpu_match_the_cpu_image() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM not set");
        return;
    };
    screens_match(Some(rom));
}

#[test]
fn inputs_that_change_nothing_rebuild_nothing_and_typing_uploads_quads_only() {
    let Some(ctx) = gpu() else {
        return;
    };
    let mut m = menu(dc_rom::test_rom());
    let mut p = MenuPainter::new(&ctx, OFFSCREEN_FORMAT);
    m.open(Go::Login);
    let mut laps = Laps::start();
    assert!(p.prepare(&ctx, &mut m, &mut laps));
    let first = p.last_upload();
    assert!(first.1 > 0, "first build uploads its quads");
    // nothing asked for a redraw: no work
    assert!(!p.prepare(&ctx, &mut m, &mut laps));
    // a pointer input at the same place (no hover change): the list is the same, no rebuild, no upload
    // (a caret blink in between may rebuild once per blink period)
    m.input(MenuInput::Move([2.0, 2.0]));
    p.prepare(&ctx, &mut m, &mut laps);
    let builds = p.builds;
    let t = std::time::Instant::now();
    for _ in 0..50 {
        m.input(MenuInput::Move([2.0, 2.0]));
        p.prepare(&ctx, &mut m, &mut laps);
    }
    let blinks = (t.elapsed().as_secs_f32() / dc_client::menu::sheet::CARET_PERIOD) as u64 + 1;
    assert!(
        p.builds - builds <= blinks,
        "{} rebuilds for 50 inputs that changed nothing",
        p.builds - builds
    );
    // moves over empty space: with the ROM cursor image (L1) the drawn cursor moves, one rebuild per move that
    // uploads quads only (the image is in the atlas); without it nothing changes
    let builds = p.builds;
    let t = std::time::Instant::now();
    for i in 1..=50 {
        m.input(MenuInput::Move([2.0 + i as f32 * 0.1, 2.0]));
        p.prepare(&ctx, &mut m, &mut laps);
        if m.has_cursor_image() {
            assert_eq!(p.last_upload().0, 0, "no atlas upload for a cursor move");
        }
    }
    let blinks = (t.elapsed().as_secs_f32() / dc_client::menu::sheet::CARET_PERIOD) as u64 + 1;
    let moved = if m.has_cursor_image() { 50 } else { 0 };
    assert!(
        p.builds - builds <= moved + blinks && p.builds - builds >= moved,
        "{} rebuilds for 50 moves ({moved} expected)",
        p.builds - builds
    );
    // a typed character changes the list: a rebuild that uploads quads; the second one no texture or glyph sheet
    if m.sheet().focused().is_some() {
        for c in ['a', 'b'] {
            m.input(MenuInput::Char(c));
            assert!(p.prepare(&ctx, &mut m, &mut laps));
        }
        let (texels, quads) = p.last_upload();
        assert_eq!(texels, 0, "no atlas upload for a typed character");
        assert!(quads > 0);
    }
}

#[test]
fn rom_menu_textures_are_decoded_ahead() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM not set");
        return;
    };
    let mut m = menu(Some(rom));
    let names = dc_client::menu::prefetch::menu_textures(&m.cat);
    assert!(m.prefetch.is_some(), "a ROM menu starts the prefetch");
    let t = std::time::Instant::now();
    while !m.prefetch.as_ref().unwrap().done {
        assert!(t.elapsed().as_secs() < 120, "prefetch did not finish");
        m.update();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let p = m.prefetch.as_ref().unwrap();
    let bytes: usize = names
        .iter()
        .filter_map(|n| m.cat.assets.generated(n))
        .map(|t| t.rgba.len())
        .sum();
    println!(
        "prefetch: {} names, {} taken ({:.1} MB decoded) in {:.2} s",
        p.asked,
        p.taken,
        bytes as f64 / 1e6,
        t.elapsed().as_secs_f32()
    );
    assert_eq!(p.asked, names.len());
    assert!(p.taken > 0);
    // every screen's images: the ones the pages and code literals name are in the cache before the screen opens
    for (name, g) in SCREENS {
        m.open(g);
        let items = m.frame_items();
        let mut drawn: Vec<&str> = items
            .iter()
            .filter_map(|i| match i {
                dc_client::hud::DrawItem::Image { tex, .. } => Some(tex.as_str()),
                _ => None,
            })
            .collect();
        drawn.sort_unstable();
        drawn.dedup();
        let ahead = drawn
            .iter()
            .filter(|n| names.iter().any(|x| x == *n))
            .count();
        println!("{name}: {ahead} of {} images decoded ahead", drawn.len());
        assert!(ahead > 0, "{name}: nothing of the screen was decoded ahead");
    }
}
