//! CLI-1171 (playtest report: "His mask is above his head looking backwards"): a face attachment (goggle row) hangs at
//! its body socket (`Eyes`) as a model object of its own that plays its first clip; it was drawn in its bind pose,
//! and the masks whose files carry a turned root node (the skull masks, the skull bandana, the Russian mask) stood
//! above the head facing backwards. With the first clip at time 0 the mask covers the face.
//!
//! ROM-gated (skipped without `CA_ROM`) and GPU-gated (skipped without an adapter).

use std::time::Duration;

use dc_client::hud::items::DrawItem;
use dc_client::menu::figure::{FigureTables, LookIds};
use dc_client::menu::link::LinkOptions;
use dc_client::menu::modelview::{ModelView, ViewReq, PANE_RECT, PLACE};
use dc_client::menu::{Menu, MenuOptions};

/// The pane render (width, height, RGBA) of an equipment list, or `None` without a GPU.
fn figure(m: &mut Menu, rom: &std::path::Path, eq: &[i32; 15]) -> Option<(u32, u32, Vec<u8>)> {
    let t = FigureTables::cached(rom);
    let ids = LookIds::from_equipped(eq);
    let (look, pieces) = t.look(&ids)?;
    let mut v = ModelView::new(rom.to_path_buf());
    let req = ViewReq {
        look: Some(look),
        pieces,
        lights: t.lights(&ids),
        place: PLACE,
        rect: PANE_RECT,
        xray: None,
    };
    v.want(Some(req), (2560, 1440), Some(0.0), 0.0);
    v.wait(&mut m.cat.assets, Duration::from_secs(120));
    let DrawItem::Image { tex, .. } = v.item()? else {
        return None;
    };
    let t = m.cat.assets.generated(&tex)?;
    Some((t.width, t.height, t.rgba.clone()))
}

/// The rows where two renders differ (first, last).
fn changed_rows(a: &(u32, u32, Vec<u8>), b: &(u32, u32, Vec<u8>)) -> Option<(u32, u32)> {
    let w = a.0 as usize;
    let rows: Vec<u32> = (0..a.1)
        .filter(|y| {
            let s = *y as usize * w * 4;
            a.2[s..s + w * 4]
                .iter()
                .zip(&b.2[s..s + w * 4])
                .any(|(p, q)| p.abs_diff(*q) > 24)
        })
        .collect();
    Some((*rows.first()?, *rows.last()?))
}

#[test]
fn rom_a_skull_mask_covers_the_face() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("t"),
        rom: Some(rom.clone()),
        font: None,
        width: 2560,
        height: 1440,
    });
    m.detach_link();
    // default look; then the same with goggle row 8 (a skull mask: its file's root node is turned)
    let mut eq = [-1; 15];
    let Some(bare) = figure(&mut m, &rom, &eq) else {
        eprintln!("skipped: no character view (no GPU adapter)");
        return;
    };
    eq[6] = 5008;
    let masked = figure(&mut m, &rom, &eq).expect("the masked render");
    assert_eq!((bare.0, bare.1), (masked.0, masked.1));
    // the head of the default figure at this pane size: its top near row 55, the chin near row 150
    let top = (0..bare.1)
        .find(|y| {
            let s = *y as usize * bare.0 as usize * 4;
            bare.2[s..s + bare.0 as usize * 4]
                .chunks(4)
                .any(|p| p[3] > 128)
        })
        .expect("the figure has pixels");
    let (first, last) = changed_rows(&bare, &masked).expect("the mask is drawn");
    assert!(
        first >= top,
        "the mask starts above the head: rows {first}..{last}, head top {top}"
    );
    assert!(
        last > top + 40,
        "the mask reaches the face: rows {first}..{last}, head top {top}"
    );
}
