//! w2-loading: the loading page (interface state 3) in the menu's window, frame by frame through the engine's own
//! menu painter and capture (the GPU quads read back, the CPU compositor without an adapter): the waiting room's
//! last frame, then the game start, then the loading frames while the session joins. No frame of that sequence
//! is blank (one colour) or white, the first frame after the room is the loading page, and the loading page
//! draws its records (map picture tiles, the bar) with the bar growing with the load stage the server's messages
//! set. Made-up lobby data only; the ROM variant needs `CA_ROM` (nothing of the ROM in this file).

use dc_botclient::lobby::{LobbyProfile, RoomMember};
use dc_client::hud::DrawItem;
use dc_client::loading::{bar_fill, stage};
use dc_client::menu::drawlist::Origin;
use dc_client::menu::link::{LinkOptions, RoomView};
use dc_client::menu::paint::MenuPainter;
use dc_client::menu::{Go, Menu, MenuOptions};
use dc_client::prof::Laps;
use dc_client::testkit::synth_font;
use dc_render::{AdapterChoice, GpuContext, OFFSCREEN_FORMAT};

const W: u32 = 640;
const H: u32 = 480;

fn menu(rom: Option<std::path::PathBuf>) -> Menu {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: LinkOptions::new("loading-page"),
        rom,
        font: Some((synth_font(), 0)),
        width: W,
        height: H,
    });
    let d = &mut m.data;
    d.profile = LobbyProfile {
        nick: "tester".into(),
        char_no: 1,
        my_id: 1,
        chan_label: "Channel 1".into(),
        ..LobbyProfile::default()
    };
    d.room = RoomView {
        title: "tester room".into(),
        host_id: 1,
        players: 2,
        max_players: 8,
        goal: 100,
        map: 0,
        mode: 0,
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
    m
}

/// The engine's frame of the menu: the GPU painter's quads read back, or the CPU compositor without an adapter.
struct Capture {
    gpu: Option<(GpuContext, MenuPainter)>,
}

impl Capture {
    fn new() -> Capture {
        let gpu = GpuContext::headless(AdapterChoice::Auto).ok().map(|c| {
            let p = MenuPainter::new(&c, OFFSCREEN_FORMAT);
            (c, p)
        });
        if gpu.is_none() {
            println!("no GPU adapter: CPU compositor frames");
        }
        Capture { gpu }
    }

    fn frame(&mut self, m: &mut Menu) -> Vec<u8> {
        match self.gpu.as_mut() {
            Some((ctx, p)) => {
                p.prepare(ctx, m, &mut Laps::start());
                read_back(ctx, p)
            }
            None => m.image().rgba,
        }
    }
}

fn read_back(ctx: &GpuContext, p: &MenuPainter) -> Vec<u8> {
    let tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("loading page test"),
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
        label: Some("loading page read"),
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

/// FNV-1a over the colour channels (the frame's pixel hash).
fn pixel_hash(rgba: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for (i, b) in rgba.iter().enumerate() {
        if i % 4 == 3 {
            continue;
        }
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The hash a frame of one colour would have.
fn solid_hash(rgb: [u8; 3]) -> u64 {
    let px: Vec<u8> = (0..W * H)
        .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
        .collect();
    pixel_hash(&px)
}

/// Share of the frame's pixels that are (nearly) white.
fn white_share(rgba: &[u8]) -> f32 {
    let n = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] > 245 && p[1] > 245 && p[2] > 245)
        .count();
    n as f32 / (W * H) as f32
}

fn one_colour(rgba: &[u8]) -> bool {
    let first = &rgba[..3];
    rgba.as_chunks::<4>().0.iter().all(|p| &p[..3] == first)
}

/// The waiting room's last frame, the game start, loading frames over 2 s of the menu clock with the load stages a
/// session reaches: no frame blank or white, the loading page right after the room.
fn transition(rom: Option<std::path::PathBuf>) {
    // without a ROM the waiting room has no pages: its own frame is black by construction (not checked for blank)
    let with_rom = rom.is_some();
    let mut m = menu(rom);
    let mut cap = Capture::new();
    m.open(Go::Room);
    let room = cap.frame(&mut m);
    let mut frames = vec![("room".to_string(), room)];
    // the window's game start: the loading page opens in the same window at once (menu::window::run_menu_window)
    m.begin_loading();
    assert!(m.loading_up());
    assert_eq!(m.screen_name(), "loading");
    let trail = m.trail.clone();
    let i = trail.iter().rposition(|s| s == "room").expect("room shown");
    assert_eq!(
        trail[i + 1..],
        ["loading".to_string()],
        "nothing between the room and the loading page"
    );
    for (k, st) in [
        stage::JOIN,
        stage::JOIN,
        stage::STATUS_RESET,
        stage::WEAPON_GIVE,
        stage::AMMO,
    ]
    .into_iter()
    .enumerate()
    {
        m.set_load_stage(st);
        frames.push((format!("loading {k} (stage {st})"), cap.frame(&mut m)));
    }
    let blank = [
        solid_hash([255, 255, 255]),
        solid_hash([0, 0, 0]),
        solid_hash([128, 128, 128]),
    ];
    for (name, f) in &frames {
        let h = pixel_hash(f);
        println!("{name}: hash {h:016x}, white {:.3}", white_share(f));
        assert!(white_share(f) < 0.5, "{name}: a white frame");
        assert_ne!(h, blank[0], "{name}: a white frame");
        if name == "room" && !with_rom {
            continue;
        }
        assert!(!blank.contains(&h), "{name}: a blank frame");
        assert!(!one_colour(f), "{name}: one colour (blank)");
    }
    assert_ne!(
        pixel_hash(&frames[0].1),
        pixel_hash(&frames[1].1),
        "the loading page is a frame of its own"
    );
    // the bar grows with the stage: the last frame differs from the first loading frame
    assert_ne!(pixel_hash(&frames[1].1), pixel_hash(&frames[5].1));
}

#[test]
fn room_to_loading_page_has_no_blank_or_white_frame() {
    transition(None);
}

#[test]
fn rom_room_to_loading_page_has_no_blank_or_white_frame() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM not set");
        return;
    };
    transition(Some(rom));
}

/// The filled width the draw list gives the bar (its first-row pieces, or the light fills without a texture).
fn filled_width(rows: &[dc_client::menu::drawlist::TraceRow]) -> f32 {
    rows.iter()
        .filter(|r| r.origin == Origin::Extra)
        .filter_map(|r| match &r.item {
            DrawItem::Image { tex, dst, .. }
                if tex.ends_with("#slice00")
                    || tex.ends_with("#slice10")
                    || tex.ends_with("#slice20") =>
            {
                Some(dst[2])
            }
            DrawItem::Fill { rect, colour } if colour[0] == 200 => Some(rect[2]),
            _ => None,
        })
        .sum()
}

fn records(rom: Option<std::path::PathBuf>) -> usize {
    let mut m = menu(rom);
    m.open(Go::Room);
    m.begin_loading();
    let rows = m.draw_trace();
    assert!(
        rows.iter().all(|r| r.origin != Origin::Cursor),
        "no cursor on the loading page"
    );
    let extra: Vec<_> = rows.iter().filter(|r| r.origin == Origin::Extra).collect();
    assert!(!extra.is_empty(), "the loading page draws its records");
    let tiles = extra
        .iter()
        .filter(|r| matches!(&r.item, DrawItem::Image { tex, .. } if tex.ends_with(".dtx")))
        .count();
    let w0 = filled_width(&rows);
    m.set_load_stage(stage::AMMO);
    let w1 = filled_width(&m.draw_trace());
    println!(
        "loading page: {} records, {tiles} tiles, bar {w0} -> {w1}",
        extra.len()
    );
    assert!(w0 > 0.0, "the bar shows its first part at once");
    assert!(w1 > w0, "a later load stage fills more of the bar");
    let page = m.data.loading.expect("loading state while the page is up");
    assert_eq!(page.stage, stage::AMMO);
    // back in the waiting room the page's state is gone and the cursor is drawn again
    m.open(Go::Room);
    assert!(m.data.loading.is_none());
    tiles
}

#[test]
fn loading_page_draws_its_records_during_the_join() {
    let tiles = records(None);
    assert_eq!(tiles, 0, "no tiles without a ROM");
}

#[test]
fn rom_loading_page_draws_the_map_tiles_and_the_bar() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM not set");
        return;
    };
    let tiles = records(Some(rom.clone()));
    assert!(
        tiles >= 4,
        "the map's loading picture is drawn ({tiles} tiles)"
    );
    let m = menu(Some(rom));
    let bar = dc_client::loading::bar_layout(&m.cat).expect("the HUD layout's bar");
    println!(
        "bar {:?} {:?} textured {}",
        bar.pos,
        bar.size,
        bar.tex.is_some()
    );
    assert!(bar.size[0] > 0.0 && bar.size[1] > 0.0 && bar.tex.is_some());
}

#[test]
fn bar_follows_the_clock_not_the_frames() {
    // the page's draw list at one clock is the same whatever the number of frames drawn before it
    let mut once = menu(None);
    once.open(Go::Room);
    once.begin_loading();
    let mut many = menu(None);
    many.open(Go::Room);
    many.begin_loading();
    let since = many.data.loading.unwrap().since;
    for f in 0..50 {
        let _ = many.items_at(since + 0.01 * f as f32);
    }
    let at = |m: &mut Menu| {
        let since = m.data.loading.unwrap().since;
        m.items_at(since + 0.75)
    };
    assert_eq!(at(&mut once), at(&mut many));
    // and it grows with the clock: one update at the opening plus 8 per second (CLI-507)
    assert_eq!(dc_client::loading::bar_updates(0.0), 1);
    assert_eq!(dc_client::loading::bar_updates(0.75), 7);
    assert!(bar_fill(400.0, stage::JOIN, 1.0) > bar_fill(400.0, stage::JOIN, 0.1));
}

#[test]
fn the_loading_page_redraws_only_when_the_bar_changes() {
    let mut m = menu(None);
    m.open(Go::Room);
    m.begin_loading();
    assert!(m.needs_redraw(), "the page's first frame");
    let _ = m.frame_items();
    assert!(
        !m.needs_redraw(),
        "the same bar update (1/8 s) and stage: nothing to draw again"
    );
    m.set_load_stage(stage::AMMO);
    assert!(m.needs_redraw(), "a new stage moves the bar");
    let _ = m.frame_items();
    std::thread::sleep(std::time::Duration::from_millis(130));
    assert!(
        m.needs_redraw(),
        "the next bar update (clock) moves the bar"
    );
}

#[test]
fn no_end_box_on_top_of_the_lost_lobby_box() {
    use dc_client::loading::SessionEnd;
    let mut m = menu(None);
    m.open(Go::Room);
    // the lobby link was lost during the match: the connection-ended box (to login) opens
    m.back_from_match(None, None);
    assert_eq!(m.popup_count(), 1);
    let first = m.popup().map(|p| p.text.clone());
    m.match_end_box(SessionEnd::Dropped);
    assert_eq!(m.popup_count(), 1, "one box to dismiss, not two");
    assert_eq!(m.popup().map(|p| p.text.clone()), first);
}

#[test]
fn match_end_boxes_open_in_the_waiting_room() {
    use dc_client::loading::SessionEnd;
    let mut m = menu(None);
    m.open(Go::Room);
    m.match_end_box(SessionEnd::Played);
    assert!(m.popup().is_none(), "a played match opens no box");
    m.match_end_box(SessionEnd::JoinFailed);
    assert_eq!(
        m.popup().map(|p| p.text.as_str()),
        Some("#3024"),
        "time-out box 0xbd0"
    );
    let mut m = menu(None);
    m.open(Go::Room);
    m.match_end_box(SessionEnd::Dropped);
    assert_eq!(
        m.popup().map(|p| p.text.as_str()),
        Some("#1154"),
        "disconnect box 0x482"
    );
}

#[test]
fn leave_box_draws_over_the_hud_while_open() {
    use dc_client::hud::{Hud, HudState};
    let mut hud = Hud::fallback(W, H, Some((synth_font(), 0)));
    let mut st = HudState::default();
    let texts = |items: &[DrawItem]| -> Vec<String> {
        items
            .iter()
            .filter_map(|i| match i {
                DrawItem::Label { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    };
    let closed = texts(&hud.items(&st));
    assert!(!closed.iter().any(|t| t == "#3007"), "no box while closed");
    st.leave_box = Some(dc_client::loading::LeaveView {
        page: dc_client::loading::LeavePage::Ask,
        ..Default::default()
    });
    let open = texts(&hud.items(&st));
    // title 0x371, text 0xbbf, Yes 0x35, No 0x36 (string ids: the fallback HUD writes `#id`)
    for want in ["#881", "#3007", "#53", "#54"] {
        assert!(open.iter().any(|t| t == want), "{want} drawn: {open:?}");
    }
}
