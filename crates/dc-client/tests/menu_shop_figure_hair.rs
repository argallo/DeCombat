//! CLI-1346 (playtest report: "the Mei character has her hair too high above her head"): a hair / head-gear piece is
//! posed with the body by node name; the female figure's hair file binds its head node 2.3 units lower than the body
//! file does, and skinned with its own bind the hair stood that far above the scalp. Skinned with the body's
//! matrices (`ModelInstance::follow_skinned`) it sits on the head as in the reference client's shop figure
//! (inbox-mei/pairs/item2_mei_hair_reference_old_new_2560.png).
//!
//! ROM-gated (skipped without `CA_ROM`) and GPU-gated (skipped without an adapter).

use std::time::Duration;

use dc_client::hud::items::DrawItem;
use dc_client::menu::figure::{FigureTables, LookIds, SLOT_CHARACTER};
use dc_client::menu::link::LinkOptions;
use dc_client::menu::modelview::{ModelView, ViewReq, PANE_RECT, PLACE};
use dc_client::menu::{Menu, MenuOptions};

type Shot = (u32, u32, Vec<u8>);

/// The pane render of an equipment list (with or without its gear pieces), or `None` without a GPU.
fn figure(m: &mut Menu, rom: &std::path::Path, eq: &[i32; 15], pieces: bool) -> Option<Shot> {
    let t = FigureTables::cached(rom);
    let ids = LookIds::from_equipped(eq);
    let (look, mut list) = t.look(&ids)?;
    if !pieces {
        list.clear();
    }
    let mut v = ModelView::new(rom.to_path_buf());
    let req = ViewReq {
        look: Some(look),
        pieces: list,
        lights: t.lights(&ids),
        place: PLACE,
        rect: PANE_RECT,
        xray: None,
        showcase: None,
    };
    v.want(Some(req), (2560, 1440), Some(0.0), 0.0);
    v.wait(&mut m.cat.assets, Duration::from_secs(120));
    let DrawItem::Image { tex, .. } = v.item()? else {
        return None;
    };
    let t = m.cat.assets.generated(&tex)?;
    Some((t.width, t.height, t.rgba.clone()))
}

/// The first row holding a covered pixel.
fn top(s: &Shot) -> Option<u32> {
    let w = s.0 as usize * 4;
    (0..s.1).find(|y| {
        s.2[*y as usize * w..(*y as usize + 1) * w]
            .chunks(4)
            .any(|p| p[3] > 128)
    })
}

/// The last row where two renders differ.
fn last_changed(a: &Shot, b: &Shot) -> Option<u32> {
    let w = a.0 as usize * 4;
    (0..a.1).rev().find(|y| {
        let s = *y as usize * w;
        a.2[s..s + w]
            .iter()
            .zip(&b.2[s..s + w])
            .any(|(p, q)| p.abs_diff(*q) > 24)
    })
}

#[test]
fn rom_the_female_figure_wears_her_hair_on_the_head() {
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
    // character item of body row 4 (the female figure; her face row's HairIndex names her hair row)
    let mut eq = [-1; 15];
    eq[SLOT_CHARACTER] = 10004;
    let t = FigureTables::cached(&rom);
    let Some((_, pieces)) = t.look(&LookIds::from_equipped(&eq)) else {
        eprintln!("skipped: no body row 4");
        return;
    };
    if pieces.len() != 1 || pieces[0].socket.is_some() {
        eprintln!("skipped: body row 4 has no hair piece");
        return;
    }
    let Some(bare) = figure(&mut m, &rom, &eq, false) else {
        eprintln!("skipped: no character view (no GPU adapter)");
        return;
    };
    let hair = figure(&mut m, &rom, &eq, true).expect("the render with hair");
    let (head, hair_top) = (top(&bare).unwrap(), top(&hair).unwrap());
    let low = last_changed(&bare, &hair).expect("the hair is drawn");
    eprintln!("head top {head}, hair top {hair_top}, hair reaches {low}");
    // the hair covers the scalp: its top a few pixels above the bare head, not a hand's width
    assert!(
        head.saturating_sub(hair_top) <= 12,
        "hair top {hair_top} stands above the head top {head}"
    );
    // and it falls to the jaw line (the bob cut)
    assert!(low >= head + 85, "hair ends at row {low}, head top {head}");
}
