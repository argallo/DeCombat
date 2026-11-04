//! The batched overlay (instanced quads over an atlas) against the CPU compositor it replaces in the frame loops.
//!
//! Each test draws the same display list both ways over the same rendered frame of the made-up world and compares
//! the read-back pixels: the quads must land on the same pixels with the same values, up to the 8-bit rounding of
//! the CPU path's intermediate image (an LSB or two where translucent items overlap). Also: textures and glyph
//! sheets are uploaded once, an unchanged list uploads nothing, a replaced texture is uploaded again.
//! GPU tests skip without an adapter; the ROM test also needs `CA_ROM`. Our own made-up content only.

use dc_client::hud::layout::TextStyle;
use dc_client::hud::state::HudEvent;
use dc_client::hud::{compose, DrawItem, Hud, HudAssets, HudFonts, Tex, TextureSource};
use dc_client::offscreen::Offscreen;
use dc_client::overlay::{GlyphAtlas, OverlayRenderer};
use dc_client::testkit::{synth_font, synth_source};
use dc_client::world::{GpuTarget, LoadedWorld};
use dc_client::{LocalPlayer, ViewAngles, ViewSettings, WorldSource};
use dc_render::{
    AdapterChoice, Camera, FrameImage, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT,
};

const W: u32 = 640;
const H: u32 = 480;

struct Gpu {
    ctx: GpuContext,
    renderer: WorldRenderer,
    world: LoadedWorld,
    target: Offscreen,
    camera: Camera,
}

fn gpu(w: u32, h: u32) -> Option<Gpu> {
    let ctx = match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => c,
        Err(e) => {
            println!("skipped: no GPU adapter ({e})");
            return None;
        }
    };
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let world = synth_source()
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))
        .expect("synthetic world");
    let spawn = world.spawn(0).unwrap();
    let me = LocalPlayer::spawn(
        dc_sim::MovementParams::default(),
        &world.collision,
        &spawn,
        world.stand_dims,
        60,
    );
    let view = ViewAngles {
        yaw: spawn.yaw,
        pitch: 0.0,
    };
    let camera = ViewSettings::default().camera(me.eye_target(&view), &view, w, h);
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, w, h);
    Some(Gpu {
        ctx,
        renderer,
        world,
        target,
        camera,
    })
}

impl Gpu {
    fn frame(&self, hud: Option<&OverlayRenderer>, panel: Option<&OverlayRenderer>) -> FrameImage {
        self.target
            .render_with_hud(
                &self.ctx,
                &self.renderer,
                self.world.scene.as_ref().unwrap(),
                &self.camera,
                None,
                &[],
                &[],
                hud,
                panel,
            )
            .unwrap()
    }
}

/// (differing pixels, largest channel difference, pixels that differ by more than 3).
fn compare(a: &FrameImage, b: &FrameImage) -> (usize, u8, usize) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let (mut n, mut max, mut big) = (0, 0u8, 0);
    for o in (0..a.rgba.len()).step_by(4) {
        let (p, q) = (&a.rgba[o..o + 4], &b.rgba[o..o + 4]);
        let d = (0..4).map(|k| p[k].abs_diff(q[k])).max().unwrap();
        if d > 0 {
            n += 1;
            max = max.max(d);
        }
        if d > 3 {
            big += 1;
        }
    }
    (n, max, big)
}

/// Made-up textures: a 4 x 4 gradient with translucent texels and an 8 x 2 strip.
struct Made;
impl TextureSource for Made {
    fn load(&mut self, path: &str) -> Option<Tex> {
        match path {
            "grad" => {
                let mut rgba = Vec::new();
                for y in 0..4u8 {
                    for x in 0..4u8 {
                        rgba.extend_from_slice(&[
                            x * 60,
                            y * 60,
                            200 - x * 40,
                            120 + x * 30 + y * 5,
                        ]);
                    }
                }
                Some(Tex {
                    width: 4,
                    height: 4,
                    rgba,
                })
            }
            "strip" => Some(Tex {
                width: 8,
                height: 2,
                rgba: (0..16u8)
                    .flat_map(|i| [i * 15, 255 - i * 15, 90, 255])
                    .collect(),
            }),
            _ => None,
        }
    }
}

fn assets() -> HudAssets {
    HudAssets::new(
        Box::new(Made),
        HudFonts::new(Some((synth_font(), 0)), 18.0, None, 1.0),
    )
}

fn style(colour: [u8; 4]) -> TextStyle {
    TextStyle {
        font: -1,
        colour,
        size: None,
    }
}

/// One of every item kind, overlapping on purpose, a few partly off the target.
fn items() -> Vec<DrawItem> {
    vec![
        DrawItem::Fill {
            rect: [20.0, 20.0, 300.0, 120.0],
            colour: [10, 20, 30, 150],
        },
        DrawItem::Image {
            tex: "grad".into(),
            src: [4.0, 4.0],
            dst: [40.0, 40.0, 200.0, 150.0],
            tint: [255, 230, 200, 220],
            turn: None,
        },
        DrawItem::Image {
            tex: "strip".into(),
            src: [6.0, 2.0],
            dst: [100.3, 300.7, 300.0, 50.0],
            tint: [255; 4],
            turn: Some(([250.0, 325.0], 0.6)),
        },
        DrawItem::Image {
            tex: String::new(),
            src: [1.0, 1.0],
            dst: [600.0, 500.0, 80.0, 20.0],
            tint: [255, 0, 0, 128],
            turn: Some(([640.0, 510.0], -1.2)),
        },
        DrawItem::Image {
            tex: "missing".into(),
            src: [4.0, 4.0],
            dst: [0.0, 0.0, 1024.0, 768.0],
            tint: [255; 4],
            turn: None,
        },
        DrawItem::Text {
            style: style([0, 255, 0, 255]),
            pos: [60.0, 60.0],
            text: "ABBA".into(),
            right: false,
        },
        DrawItem::Text {
            style: style([255, 255, 0, 140]),
            pos: [1000.0, 700.0],
            text: "BAB".into(),
            right: true,
        },
        DrawItem::Label {
            style: style([200, 200, 255, 255]),
            pos: [512.0, 200.0],
            text: "AB AB".into(),
            anchor: [0.5, 0.5],
        },
        DrawItem::Text {
            style: style([255, 255, 255, 255]),
            pos: [1010.0, 10.0],
            text: "ABABAB".into(),
            right: false,
        },
        DrawItem::AimCross {
            bar: 6.0,
            half_gap: 4.0,
            colour: [0, 255, 0, 200],
        },
        DrawItem::AimMark(dc_client::spread::AimMarkView {
            distance: 10.0,
            since_shot_s: None,
            colour: [255, 64, 64, 255],
            rest: 5.0,
        }),
        DrawItem::Fill {
            rect: [900.0, 600.0, 400.0, 400.0],
            colour: [255, 255, 255, 60],
        },
    ]
}

#[test]
fn gpu_display_list_matches_the_cpu_compositor() {
    let Some(g) = gpu(W, H) else {
        return;
    };
    let list = items();
    let mut a1 = assets();
    let img = compose(&list, &mut a1, W, H);
    let mut cpu_layer = OverlayRenderer::new(&g.ctx, OFFSCREEN_FORMAT);
    cpu_layer.update(&g.ctx, &img);
    let cpu = g.frame(Some(&cpu_layer), None);
    let mut a2 = assets();
    let mut gpu_layer = OverlayRenderer::new(&g.ctx, OFFSCREEN_FORMAT);
    let n = gpu_layer.update_items(&g.ctx, &list, &mut a2, (W, H));
    let ours = g.frame(Some(&gpu_layer), None);
    let bare = g.frame(None, None);
    let (drawn, _, _) = compare(&bare, &cpu);
    let (n_diff, max, big) = compare(&cpu, &ours);
    println!(
        "{n} quads; HUD pixels {drawn}; GPU vs CPU: {n_diff} differ, max {max}, {big} by more than 3"
    );
    assert!(drawn > 20_000, "the list draws: {drawn}");
    assert_eq!(big, 0, "only rounding differences");
    assert!(max <= 3, "max difference {max}");
    // the list overlaps translucent items on purpose: an LSB of rounding on a share of those pixels
    assert!(n_diff * 4 < drawn, "{n_diff} of {drawn} pixels differ");
    assert_eq!(a2.missing, vec!["missing".to_string()], "as the compositor");
}

#[test]
fn panel_quads_match_the_panel_image() {
    let Some(g) = gpu(W, H) else {
        return;
    };
    let font = dc_client::overlay::font_from_bytes(synth_font(), 20.0).unwrap();
    let lines: Vec<String> = vec![
        "ABAB".into(),
        "B A".into(),
        String::new(),
        "AAAA BBBB".into(),
    ];
    let mut img_panel = OverlayRenderer::new(&g.ctx, OFFSCREEN_FORMAT);
    img_panel.update(&g.ctx, &font.atlas.panel(&lines));
    let cpu = g.frame(None, Some(&img_panel));
    let mut quad_panel = OverlayRenderer::new(&g.ctx, OFFSCREEN_FORMAT);
    quad_panel.update_panel(&g.ctx, &font.atlas, &lines);
    let ours = g.frame(None, Some(&quad_panel));
    let (n_diff, max, big) = compare(&cpu, &ours);
    println!("panel: {n_diff} differ, max {max}");
    assert_eq!(big, 0);
    assert!(max <= 2, "max difference {max}");
}

#[test]
fn atlas_uploads_once_and_unchanged_lists_upload_nothing() {
    let Some(g) = gpu(W, H) else {
        return;
    };
    let list = items();
    let mut a = assets();
    let mut layer = OverlayRenderer::new(&g.ctx, OFFSCREEN_FORMAT);
    layer.update_items(&g.ctx, &list, &mut a, (W, H));
    let (tex0, inst0) = layer.last_upload();
    assert!(
        tex0 > 0 && inst0 > 0,
        "first build uploads {tex0} / {inst0}"
    );
    layer.update_items(&g.ctx, &list, &mut a, (W, H));
    assert_eq!(layer.last_upload(), (0, 0), "same list: nothing uploaded");
    // a moved item: only the quad list goes up
    let mut moved = list.clone();
    if let DrawItem::Fill { rect, .. } = &mut moved[0] {
        rect[0] += 7.0;
    }
    layer.update_items(&g.ctx, &moved, &mut a, (W, H));
    let (tex1, inst1) = layer.last_upload();
    assert_eq!(tex1, 0, "no texel upload for a moved fill");
    assert!(inst1 > 0);
    // a texture replaced under the same name (as the radar does) is uploaded again, in place (same size)
    a.put(
        "grad",
        Tex {
            width: 4,
            height: 4,
            rgba: [9, 9, 9, 255].repeat(16),
        },
    );
    layer.update_items(&g.ctx, &moved, &mut a, (W, H));
    assert_eq!(layer.last_upload().0, 4 * 4 * 4, "the new texels only");
    let ours = g.frame(Some(&layer), None);
    let img = compose(&moved, &mut a, W, H);
    let mut cpu_layer = OverlayRenderer::new(&g.ctx, OFFSCREEN_FORMAT);
    cpu_layer.update(&g.ctx, &img);
    let cpu = g.frame(Some(&cpu_layer), None);
    let (_, max, big) = compare(&cpu, &ours);
    assert!(big == 0 && max <= 3, "replaced texture drawn: max {max}");
    let (pages, resets) = layer.atlas_stats();
    assert_eq!((pages, resets), (1, 0));
}

#[test]
fn glyph_layout_matches_draw() {
    let f = dc_client::overlay::ttf::Font::parse(synth_font(), 0).unwrap();
    let atlas = GlyphAtlas::new(&f, 20.0);
    let mut placed = Vec::new();
    atlas.layout(10.4, 30.6, "AB?", |i, bm, x, y| {
        placed.push((i, x, y, bm.width))
    });
    // the made-up font draws A and B; '?' has no outline (nothing placed, the pen still advances)
    assert_eq!(placed.len(), 2, "A and B");
    assert!(placed[1].1 > placed[0].1);
    let clone = atlas.clone();
    assert_eq!(clone.id(), atlas.id(), "clones share the sheet");
    assert_ne!(GlyphAtlas::new(&f, 20.0).id(), atlas.id());
    // every placed bitmap pixel is where draw() puts it
    let mut img = dc_client::overlay::RgbaImage::new(80, 50, [0, 0, 0, 0]);
    atlas.draw(&mut img, 10.4, 30.6, "AB?", [255, 255, 255]);
    let bms: Vec<_> = atlas.bitmaps().cloned().collect();
    for (i, x, y, _) in placed {
        let bm = &bms[i];
        let (r, c) = (0..bm.height)
            .flat_map(|r| (0..bm.width).map(move |c| (r, c)))
            .find(|&(r, c)| bm.alpha[(r * bm.width + c) as usize] == 255)
            .expect("a solid pixel");
        assert_eq!(
            img.pixel((x + c as i32) as u32, (y + r as i32) as u32)[3],
            255
        );
    }
}

#[test]
fn rom_hud_on_the_gpu_matches_the_compositor() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let (w, h) = (1280, 720);
    let Some(g) = gpu(w, h) else {
        return;
    };
    let mut hud = Hud::from_rom(&rom, w, h, None).expect("ROM HUD");
    let mut s = dc_client::hud::HudState::default();
    let rec = |pid: u8, name: &str, team: u8| dc_botclient::shell::RosterRecord {
        name: name.into(),
        pid,
        team,
        complete: true,
        ..Default::default()
    };
    for e in [
        HudEvent::Own {
            pid: Some(0),
            mode_class: Some(1),
        },
        HudEvent::Roster(rec(0, "alpha", 1)),
        HudEvent::Roster(rec(1, "bravo", 2)),
        HudEvent::Life(1),
        HudEvent::Status {
            kind: 2,
            value: 64.0,
        },
        HudEvent::Status {
            kind: 3,
            value: 12.0,
        },
        HudEvent::RoundBegin(180),
        HudEvent::Kill(dc_botclient::shell::KillRecord {
            victim: 1,
            killer: 0,
            kill_kind: 1,
            weapon: 145,
            hit_zone: 1,
            feed_value: 2,
            complete: true,
            ..Default::default()
        }),
        HudEvent::Damage([0.6, 0.0, -0.5]),
    ] {
        s.apply(&e, 1000, 0.0);
    }
    s.tick(1200, hud.layout.feed.lifetime_ms.max(1));
    for board in [false, true] {
        s.show_board = board;
        let list = hud.items(&s);
        let img = compose(&list, &mut hud.assets, w, h);
        let mut cpu_layer = OverlayRenderer::new(&g.ctx, OFFSCREEN_FORMAT);
        cpu_layer.update(&g.ctx, &img);
        let cpu = g.frame(Some(&cpu_layer), None);
        let mut layer = OverlayRenderer::new(&g.ctx, OFFSCREEN_FORMAT);
        let n = layer.update_items(&g.ctx, &list, &mut hud.assets, (w, h));
        let ours = g.frame(Some(&layer), None);
        let (drawn, _, _) = compare(&g.frame(None, None), &cpu);
        let (n_diff, max, big) = compare(&cpu, &ours);
        println!(
            "board {board}: {} items, {n} quads, {drawn} HUD pixels; {n_diff} differ, max {max}, {big} by more than 3",
            list.len()
        );
        assert!(drawn > 10_000);
        assert_eq!(big, 0, "only rounding differences");
        assert!(n_diff * 5 < drawn);
    }
}
