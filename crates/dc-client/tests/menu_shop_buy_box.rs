//! shop-buy-box: the shop's purchase window ("Weapon Purchase") drawn from the user's ROM at 1024 x 768 and
//! 1920 x 1080 (headless, no lobby server): OK / Cancel sit inside the window below the balance lines, their
//! captions are centred on the Solid_Bt picture (not on the wider control), and both balance lines carry numbers for
//! the product's currency (GP for a GP cell, NX Prepaid for a black market credit cell).
//!
//! ROM-gated (skipped without `CA_ROM`). `DC_MENU_SHOTS=<dir>` keeps the drawn screens as PNGs.
// DISASM: cshell+0x1d8070..0x1d80b3, cshell+0x1d81b9 (captions at (0x33, 0xc) centred), cshell+0x1d804b (0x78 x 0x19)
// EVIDENCE: cshell+0x1d94a0 / cshell+0x1d95c0 (the balance texts)

use dc_client::hud::items::DrawItem;
use dc_client::menu::buybox::{
    BuyArt, BALANCE_Y, BUTTON_SIZE, CANCEL_AT, CANCEL_TEXT, CAPTION_COLOUR, FONT_SIZE, OK_AT,
    OK_TEXT, VALUE_X, WINDOW_AT, WINDOW_SIZE,
};
use dc_client::menu::link::LinkOptions;
use dc_client::menu::shop::{grid_cells, CMD_BUY, SHOP_GRID, SHOP_WINDOW};
use dc_client::menu::storage::{Wares, TAB_MARKET, TAB_WEAPONS};
use dc_client::menu::{pages, Go, Menu, MenuInput, MenuOptions};

const SIZES: [(u32, u32); 2] = [(1024, 768), (1920, 1080)];

type Label = ([f32; 2], [f32; 2], [u8; 4], String);

fn menu(w: u32, h: u32) -> Option<Menu> {
    let rom = dc_rom::resolve_rom().ok()?;
    Some(Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("t"),
        rom: Some(rom),
        font: None,
        width: w,
        height: h,
    }))
}

fn tap(m: &mut Menu, p: [f32; 2]) {
    m.input(MenuInput::Move(p));
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
}

fn mid(c: [i32; 4]) -> [f32; 2] {
    [(c[0] + c[2] / 2) as f32, (c[1] + c[3] / 2) as f32]
}

/// Buy on cell `cell` of the open shop page: the purchase window.
fn open_box(m: &mut Menu, cell: usize) {
    let cells = grid_cells(&m.cat, pages::SHOP, SHOP_WINDOW, SHOP_GRID);
    tap(m, mid(cells[cell]));
    let b = m.sheet().button(CMD_BUY).filter(|b| b.shown).expect("Buy");
    let p = mid([b.pos[0], b.pos[1], b.size[0], b.size[1]]);
    tap(m, p);
}

fn labels(m: &mut Menu) -> Vec<Label> {
    m.items()
        .iter()
        .filter_map(|i| match i {
            DrawItem::Label {
                pos,
                anchor,
                style,
                text,
            } => Some((*pos, *anchor, style.colour, text.clone())),
            _ => None,
        })
        .collect()
}

fn shot(m: &mut Menu, name: &str) {
    if let Some(dir) = std::env::var_os("DC_MENU_SHOTS") {
        let img = m.image();
        let p = std::path::Path::new(&dir).join(name);
        image::save_buffer(
            &p,
            &img.rgba,
            img.width,
            img.height,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
}

/// The opaque extent of a texture (width, height of the alpha > 0 box from the corner).
fn opaque_extent(m: &mut Menu, tex: &str) -> [i32; 2] {
    let t = m.cat.assets.texture(tex).expect("button texture");
    let (mut w, mut h) = (0, 0);
    for y in 0..t.height {
        for x in 0..t.width {
            if t.rgba[((y * t.width + x) * 4 + 3) as usize] > 0 {
                w = w.max(x as i32 + 1);
                h = h.max(y as i32 + 1);
            }
        }
    }
    [w, h]
}

fn abs(at: [i32; 2]) -> [i32; 2] {
    [WINDOW_AT[0] + at[0], WINDOW_AT[1] + at[1]]
}

fn grouped(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// The value drawn right-aligned at the value column of balance line `y`.
fn value_at(ls: &[Label], y: i32) -> Option<String> {
    let at = abs([VALUE_X, y]);
    ls.iter()
        .find(|l| l.0 == [at[0] as f32, at[1] as f32] && l.1 == [1.0, 0.0])
        .map(|l| l.3.clone())
}

/// The K2 purchase window: OK / Cancel inside the window below the remaining-balance line, each caption white and
/// centred on its button picture, both balance lines filled with GP, at 1024 x 768 and 1920 x 1080.
#[test]
fn rom_k2_purchase_window_buttons_captions_and_balances() {
    for (w, h) in SIZES {
        let Some(mut m) = menu(w, h) else {
            eprintln!("skipped: no CA_ROM");
            return;
        };
        let wares = Wares::cached(m.cat.rom.as_deref());
        m.data.store.view = Some(wares.catalogue_ids());
        m.data.store.lists_asked = true;
        m.data.profile.gp = 12_345;
        m.open(Go::Shop);
        let shelf = wares.shelf(&m.data.store, TAB_WEAPONS, 0);
        let k2 = shelf
            .iter()
            .position(|s| wares.label_of(s.item_no) == "K2")
            .expect("K2 on the first weapons page");
        open_box(&mut m, k2);
        shot(&mut m, &format!("buy_box_k2_{w}x{h}.png"));
        let ls = labels(&mut m);
        let row = ls
            .iter()
            .find(|l| l.3.starts_with("K2 ("))
            .map(|l| l.3.clone())
            .expect("the chosen row");
        // "K2 (1 day) - 800 GP": the price of the chosen row
        let price: i64 = row
            .rsplit(" - ")
            .next()
            .and_then(|p| p.strip_suffix(" GP"))
            .map(|p| p.replace(',', "").parse().unwrap())
            .expect("a GP row");
        assert_eq!(
            value_at(&ls, BALANCE_Y[0]).as_deref(),
            Some("12,345 GP"),
            "{w}x{h}"
        );
        assert_eq!(
            value_at(&ls, BALANCE_Y[1]),
            Some(format!("{} GP", grouped(12_345 - price))),
            "{w}x{h}"
        );
        // the buttons: inside the window, below the last balance line, side by side
        let art = BuyArt::from_rom(m.cat.rom.as_deref());
        let tex = art.button.clone().expect("Solid_Bt_A");
        let pic = opaque_extent(&mut m, &tex);
        assert_eq!(pic, [102, 25], "the Solid_Bt picture");
        let below = abs([0, BALANCE_Y[1] + FONT_SIZE])[1];
        assert_eq!(CAPTION_COLOUR, 0xffff_ffff);
        for (at, id) in [(OK_AT, OK_TEXT), (CANCEL_AT, CANCEL_TEXT)] {
            let b = abs(at);
            assert!(b[0] >= WINDOW_AT[0] && b[1] >= below, "{at:?}");
            assert!(b[0] + BUTTON_SIZE[0] <= WINDOW_AT[0] + WINDOW_SIZE[0]);
            assert!(b[1] + BUTTON_SIZE[1] <= WINDOW_AT[1] + WINDOW_SIZE[1]);
            let caption = m.cat.text(id);
            let c = ls
                .iter()
                .find(|l| l.3 == caption)
                .unwrap_or_else(|| panic!("caption {caption}"));
            assert_eq!(c.1, [0.5, 0.5], "centred about a point");
            // the picture's middle to within half a pixel
            let dx = c.0[0] - (b[0] as f32 + pic[0] as f32 / 2.0);
            let dy = c.0[1] - (b[1] as f32 + pic[1] as f32 / 2.0);
            assert!(dx.abs() <= 0.5 && dy.abs() <= 0.5, "{caption} {c:?} {b:?}");
            assert_eq!(c.2, [255; 4], "white in every state");
        }
        assert!(abs(OK_AT)[0] + pic[0] < abs(CANCEL_AT)[0]);
    }
}

/// The black market's first credit cell: the window opens on NX Prepaid and both lines show prepaid NX.
#[test]
fn rom_nx_purchase_window_shows_prepaid_balances() {
    use dc_client::menu::shop::CATEGORY_COMMANDS;
    for (w, h) in SIZES {
        let Some(mut m) = menu(w, h) else {
            eprintln!("skipped: no CA_ROM");
            return;
        };
        let wares = Wares::cached(m.cat.rom.as_deref());
        m.data.store.view = Some(wares.catalogue_ids());
        m.data.store.lists_asked = true;
        m.data.balance = (700, 50_000);
        let cmd = CATEGORY_COMMANDS[TAB_MARKET]
            .iter()
            .enumerate()
            .skip(2)
            .find_map(|(c, cmd)| {
                let shelf = wares.shelf(&m.data.store, TAB_MARKET, c);
                shelf.first().filter(|w| w.sale_type == 1).map(|_| *cmd)
            })
            .expect("a credit cell");
        m.open(Go::Market);
        m.input(MenuInput::Command(cmd));
        open_box(&mut m, 0);
        shot(&mut m, &format!("buy_box_nx_{w}x{h}.png"));
        let ls = labels(&mut m);
        let row = ls
            .iter()
            .find(|l| l.3.ends_with(" NX") && l.3.contains(") - "))
            .map(|l| l.3.clone())
            .expect("the chosen credit row");
        let price: i64 = row
            .rsplit(" - ")
            .next()
            .and_then(|p| p.strip_suffix(" NX"))
            .map(|p| p.replace(',', "").parse().unwrap())
            .expect("an NX row");
        assert_eq!(
            value_at(&ls, BALANCE_Y[0]).as_deref(),
            Some("50,000 NX Prepaid"),
            "{w}x{h}"
        );
        assert_eq!(
            value_at(&ls, BALANCE_Y[1]),
            Some(format!("{} NX Prepaid", grouped(50_000 - price))),
            "{w}x{h}"
        );
    }
}
