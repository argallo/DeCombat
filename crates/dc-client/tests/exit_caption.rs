//! CLI-1907: the exit box's Options caption against the words baked into the three 2009 button pictures, with the
//! user's ROM (skipped without `CA_ROM`). The baked words are measured in the pictures themselves (one picture pixel
//! per virtual pixel) and on the composed screen; our caption must have the same capital height and stem width
//! within one virtual pixel, black letters and a light edge, at 1024 x 768 and at the owner's 2560 x 1440. With a GPU
//! adapter the batched overlay the match draws with must put the same pixels there. Nothing of the ROM in this file;
//! `DC_EXITBOX_PNG=<file>` saves the box as drawn at 2560 x 1440.

use dc_client::hud::{compose, Hud, HudState};
use dc_client::loading::{LeavePage, LeaveView, EXIT_OPTIONS};

/// The ink of a word: rows (top, baseline row) and the median horizontal stroke width, in the unit measured.
#[derive(Debug, Clone, Copy)]
struct Word {
    top: u32,
    base: u32,
    stem: f32,
    ink: usize,
}

impl Word {
    fn cap(&self) -> u32 {
        self.base - self.top + 1
    }
}

fn is_ink(p: [u8; 4]) -> bool {
    p[3] > 0x80 && p[0] < 0x40 && p[1] < 0x40 && p[2] < 0x40
}

/// The word inside the face band of a button `rect` (x, y, w, h; same unit as `px`) where the 2009 pictures carry
/// their words: x from 10 / 172 to 158 / 172 of the width, y from 7 / 30 to 24 / 30 of the height (the picture's
/// dark frame lies outside it). The baseline row is the last row with at least a third of the fullest row's ink
/// (descenders do not count), the top the first inked row.
fn word(px: &dyn Fn(u32, u32) -> [u8; 4], rect: [f32; 4]) -> Word {
    let x0 = (rect[0] + rect[2] * 10.0 / 172.0).round() as u32;
    let x1 = (rect[0] + rect[2] * 158.0 / 172.0).round() as u32;
    let y0 = (rect[1] + rect[3] * 7.0 / 30.0).round() as u32;
    let y1 = (rect[1] + rect[3] * 24.0 / 30.0).round() as u32;
    let mut rows = Vec::new();
    let mut runs = Vec::new();
    for y in y0..y1 {
        let mut n = 0;
        let mut run = 0;
        for x in x0..=x1 {
            if x < x1 && is_ink(px(x, y)) {
                n += 1;
                run += 1;
            } else if run > 0 {
                runs.push(run);
                run = 0;
            }
        }
        rows.push((y, n));
    }
    let most = rows.iter().map(|r| r.1).max().unwrap_or(0);
    assert!(most > 0, "no ink in {rect:?}");
    let top = rows.iter().find(|r| r.1 > 0).unwrap().0;
    let base = rows.iter().rev().find(|r| r.1 * 3 >= most).unwrap().0;
    runs.sort();
    Word {
        top,
        base,
        stem: runs[runs.len() / 2] as f32,
        ink: rows.iter().map(|r| r.1).sum(),
    }
}

fn exit_view() -> HudState {
    HudState {
        leave_box: Some(LeaveView {
            page: LeavePage::Exit,
            hover: None,
            down: None,
        }),
        ..HudState::default()
    }
}

fn pixel(rgba: &[u8], width: u32) -> impl Fn(u32, u32) -> [u8; 4] + '_ {
    move |x, y| {
        let o = ((y * width + x) * 4) as usize;
        [rgba[o], rgba[o + 1], rgba[o + 2], rgba[o + 3]]
    }
}

#[test]
fn rom_options_caption_matches_the_baked_words() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    // the baked words in the pictures: picture pixels are virtual pixels
    let mut hud = Hud::from_rom(&rom, 1024, 768, None).expect("ROM HUD");
    let b = hud.exit_box.clone();
    let mut baked = Vec::new();
    for (i, row) in b.button_tex.iter().enumerate() {
        let t = hud.assets.texture(row[0].as_deref().expect("picture")).expect("texture");
        let px = pixel(&t.rgba, t.width);
        let r = b.buttons[i];
        let w = word(&px, [0.0, 0.0, r[2] as f32, r[3] as f32]);
        println!("baked word {i} in its picture: {w:?} cap {}", w.cap());
        baked.push(w);
    }
    for (w, h) in [(1024u32, 768u32), (2560, 1440)] {
        hud.set_size(w, h);
        let (sx, sy) = (w as f32 / 1024.0, h as f32 / 768.0);
        let items = hud.items(&exit_view());
        let img = compose(&items, &mut hud.assets, w, h);
        let px = pixel(&img.rgba, w);
        let on_screen = |r: [i32; 4]| {
            [
                (r[0] as f32 * sx).floor(),
                (r[1] as f32 * sy).floor(),
                r[2] as f32 * sx,
                r[3] as f32 * sy,
            ]
        };
        let ours = word(&px, on_screen(b.buttons[EXIT_OPTIONS]));
        println!(
            "{w}x{h}: Options {ours:?}: cap {:.2} virtual, stem {:.2} virtual",
            ours.cap() as f32 / sy,
            ours.stem / sx
        );
        for i in 0..3 {
            let theirs = word(&px, on_screen(b.buttons[i]));
            println!(
                "{w}x{h}: baked word {i} {theirs:?}: cap {:.2} virtual, stem {:.2} virtual",
                theirs.cap() as f32 / sy,
                theirs.stem / sx
            );
            for (what, a, b) in [
                ("cap height", ours.cap() as f32 / sy, theirs.cap() as f32 / sy),
                ("cap height (picture)", ours.cap() as f32 / sy, baked[i].cap() as f32),
                ("stem", ours.stem / sx, theirs.stem / sx),
                ("stem (picture)", ours.stem / sx, baked[i].stem),
            ] {
                assert!((a - b).abs() <= 1.0, "{w}x{h} word {i} {what}: ours {a:.2}, baked {b:.2}");
            }
        }
        // black letters with a light edge: the ink is black, and right above the top ink row of each inked column
        // lies a light pixel (the edge), never the button face
        let r = on_screen(b.buttons[EXIT_OPTIONS]);
        assert!(ours.ink > (40.0 * sx * sy) as usize, "ink pixels {}", ours.ink);
        // the face band of [`word`] (the button picture's dark frame lies outside it)
        let x0 = (r[0] + r[2] * 10.0 / 172.0).round() as u32;
        let x1 = (r[0] + r[2] * 158.0 / 172.0).round() as u32;
        let mut edged = 0;
        let mut columns = 0;
        for x in x0..x1 {
            if !is_ink(px(x, ours.top)) {
                continue;
            }
            columns += 1;
            let lightest = (1..=sy.ceil() as u32)
                .map(|d| px(x, ours.top - d))
                .map(|p| p[0].min(p[1]).min(p[2]))
                .max()
                .unwrap();
            if lightest >= 0xc0 {
                edged += 1;
            }
        }
        println!("{w}x{h}: {edged} of {columns} top ink columns have a light edge above");
        assert!(columns > 0 && edged == columns, "{edged} of {columns}");
        if w == 2560 {
            check_gpu(&items, &mut hud, &img, r);
            if let Ok(out) = std::env::var("DC_EXITBOX_PNG") {
                let win = on_screen(b.window);
                let (cx, cy, cw, ch) = (win[0] as u32, win[1] as u32, win[2] as u32, win[3] as u32);
                let mut rgba = Vec::new();
                for y in cy..cy + ch {
                    for x in cx..cx + cw {
                        let p = px(x, y);
                        // over a black backdrop (the picture's own alpha)
                        let a = p[3] as u32;
                        rgba.extend([0, 1, 2].map(|k| (p[k] as u32 * a / 255) as u8));
                        rgba.push(255);
                    }
                }
                dc_render::FrameImage {
                    width: cw,
                    height: ch,
                    rgba,
                }
                .save_png(std::path::Path::new(&out))
                .expect("png");
                println!("saved {out}");
            }
        }
    }
}

/// The batched overlay (what the match draws with) puts the compositor's pixels on the Options button `r`.
fn check_gpu(
    items: &[dc_client::hud::DrawItem],
    hud: &mut Hud,
    cpu: &dc_client::overlay::RgbaImage,
    r: [f32; 4],
) {
    use dc_client::offscreen::Offscreen;
    use dc_client::overlay::OverlayRenderer;
    use dc_client::world::GpuTarget;
    use dc_client::{LocalPlayer, ViewAngles, ViewSettings, WorldSource};
    use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
    let ctx = match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => c,
        Err(e) => {
            println!("GPU part skipped: no adapter ({e})");
            return;
        }
    };
    let (w, h) = (cpu.width, cpu.height);
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let world = dc_client::testkit::synth_source()
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
    let mut layer = OverlayRenderer::new(&ctx, OFFSCREEN_FORMAT);
    layer.update_items(&ctx, items, &mut hud.assets, (w, h));
    let frame = target
        .render_with_hud(
            &ctx,
            &renderer,
            world.scene.as_ref().unwrap(),
            &camera,
            None,
            &[],
            &[],
            Some(&layer),
            None,
        )
        .unwrap();
    // the button and its caption are opaque: the frame under them does not show
    let px = pixel(&cpu.rgba, w);
    let mut worst = 0u8;
    for y in r[1] as u32..(r[1] + r[3]) as u32 {
        for x in r[0] as u32..(r[0] + r[2]) as u32 {
            let (c, g) = (px(x, y), frame.pixel(x, y));
            if c[3] == 255 {
                worst = worst.max((0..3).map(|k| c[k].abs_diff(g[k])).max().unwrap());
            }
        }
    }
    println!("GPU ({:?}) vs compositor on the Options button: max difference {worst}", ctx.info.backend);
    assert!(worst <= 3, "max difference {worst}");
}
