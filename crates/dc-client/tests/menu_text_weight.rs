//! CLI-780: menu text is drawn the way GDI draws the installed Tahoma faces at their whole-pixel em: monochrome
//! where the face's `gasp` table asks for no grey-scale (Tahoma 9..16 ppem), the line's ascent from its `VDMX`
//! record, advances from its `hdmx` widths. The glyph runs of the shop's description (font 0x17) and cell name
//! (font 0x15) are measured against the reference client's picture.
//!
//! ROM-gated (the font records come from the user's ROM) and host-gated (the faces from the host's font folder).
// OBSERVED: shop-guns/cpp/shop_bb00_weapons_c0_p1.png (description line 2 "Highly adaptable."
// pen x 495, line top 674: ink x 495..600, 'H' rows 677..685 (baseline 12 below the top), 441 text pixels, no grey
// ramp; cell name "M4A1": ink 35 px wide, 9 rows, 135 text pixels)

use dc_client::menu::link::LinkOptions;
use dc_client::menu::{Menu, MenuOptions};
use dc_client::overlay::RgbaImage;

fn menu() -> Option<Menu> {
    let rom = dc_rom::resolve_rom().ok()?;
    Some(Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("t"),
        rom: Some(rom),
        font: None,
        width: 1024,
        height: 768,
    }))
}

/// Ink of `text` drawn with its line top at y 20, pen at x 10: (pixel count, first / last ink column relative to
/// the pen, first / last ink row relative to the baseline, ascent, grey pixels).
fn run(m: &mut Menu, font: i32, text: &str) -> Option<(usize, i32, i32, i32, i32, f32, usize)> {
    let atlas = m.cat.assets.fonts.atlas_sized(font, Some(0xe))?;
    let mut img = RgbaImage::new(400, 60, [0, 0, 0, 0]);
    let baseline = 20.0 + atlas.ascent;
    atlas.draw(&mut img, 10.0, baseline, text, [255, 255, 255]);
    let (mut n, mut grey) = (0, 0);
    let (mut x0, mut x1, mut y0, mut y1) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for y in 0..img.height {
        for x in 0..img.width {
            let a = img.pixel(x, y)[3];
            if a > 0 && a < 255 {
                grey += 1;
            }
            if a >= 128 {
                n += 1;
                x0 = x0.min(x as i32 - 10);
                x1 = x1.max(x as i32 - 10);
                y0 = y0.min(y as i32 - baseline as i32);
                y1 = y1.max(y as i32 - baseline as i32);
            }
        }
    }
    Some((n, x0, x1, y0, y1, atlas.ascent, grey))
}

#[test]
fn rom_menu_text_glyph_runs_measure_like_the_reference() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    if !std::path::Path::new("C:/Windows/Fonts/tahomabd.ttf").exists() {
        eprintln!("skipped: no Tahoma Bold on this host");
        return;
    }
    // font 0x17 (Tahoma Bold, cell 14 -> 12 ppem): the description's second line
    let (n, x0, x1, _, _, ascent, grey) =
        run(&mut m, 0x17, "Highly adaptable.").expect("font 0x17");
    let (_, _, _, cap_top, cap_bottom, _, _) = run(&mut m, 0x17, "H").unwrap();
    eprintln!("0x17: n {n} x {x0}..{x1} cap {cap_top}..{cap_bottom} ascent {ascent} grey {grey}");
    assert_eq!(grey, 0, "monochrome at 12 ppem (Tahoma gasp)");
    assert_eq!(ascent, 12.0, "baseline 12 below the line top (VDMX)");
    assert_eq!(
        (cap_top, cap_bottom),
        (-9, -1),
        "'H' 9 rows on the baseline"
    );
    assert!(
        (0..=1).contains(&x0) && x1 == 105,
        "ink x {x0}..{x1}, reference 0..105"
    );
    assert!(
        (n as f32 - 441.0).abs() <= 441.0 * 0.08,
        "{n} text pixels, reference 441"
    );
    // font 0x15: the cell name
    let (n, x0, x1, y0, y1, _, grey) = run(&mut m, 0x15, "M4A1").expect("font 0x15");
    eprintln!("0x15: n {n} x {x0}..{x1} y {y0}..{y1} grey {grey}");
    assert_eq!(grey, 0);
    assert_eq!(y1 - y0 + 1, 9, "9 rows");
    assert!(
        (x1 - x0 + 1 - 35).abs() <= 1,
        "{} wide, reference 35",
        x1 - x0 + 1
    );
    assert!(
        (n as f32 - 135.0).abs() <= 135.0 * 0.12,
        "{n} text pixels, reference 135"
    );
}
