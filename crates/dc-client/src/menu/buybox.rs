//! The 2009 purchase window of the shop ("BuyItemBox"): Buy on a shop cell opens it over the shop with every shown
//! product of the cell's item in a drop-down list (usage period and price per row), the current balance and the
//! balance after the purchase; OK hands the chosen product back to the shop, which checks the balance and the
//! extensions and sends 0x328. Credit (NX) products show the payment choice (NX Credit / NX Prepaid) and ask once
//! more before the request.
//!
//! The window is built in code (no layout page): a 475 x 256 popup at (276, 256).
// SPEC: PROTOCOL_2009.md §4.2 (0x328 / 0x329), §3.3 (0x345 balance)
// DISASM: cshell+0x14fad3..0x14fbbe (shop command 266 Buy: a selected cell that is not a GP supply case opens the
// window of the shell's UI object +0x380 with the cell's item number and type (cshell+0x1dae90), owner = the shop
// screen with command 0x3fb (cshell+0x1d7950), shown (cshell+0x1d77b0))
// DISASM: cshell+0x150390..0x1503c7 (shop command 0x3fb with argument 1: chosen product -> +0x5598, the window's
// payment mode +0x88 -> +0x55e0, then the purchase check cshell+0x145b60)
// EVIDENCE: cshell+0x1d7d10 (the window's controls), cshell+0x1da000 (the period list), cshell+0x1d9de0 (a row
// picked), cshell+0x1d9af0 (payment mode), cshell+0x1d94a0 / cshell+0x1d95c0 (the balance texts), cshell+0x1dab70
// (its commands), cshell+0x1da810 (OK), cshell+0x145b60 (the shop's purchase check), cshell+0x1d7af0 (numbers with
// thousands separators), cshell+0x24af10 / cshell+0x24afd0 (title and text by item type and sale type)

use dc_ui_data::names::{read_pinned, Pin, CSHELL_REL_PATH};
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};
use std::path::Path;

use super::catalog::Catalog;
use super::dropdown::{self, Dropdown};
use super::link::{InvRecord, LinkCmd};
use super::popup::{BoxTag, Popup};
use super::result::grouped;
use super::roomtext::wrap_lines;
use super::sheet::{image_fit, place_text, ALIGN_CENTRE, ALIGN_LEFT, ALIGN_RIGHT};
use super::storage::{Store, Wares};
use super::MenuData;
use crate::hud::items::DrawItem;

/// The shop command the window answers with (argument 1 = OK with the chosen product).
// EVIDENCE: cshell+0x14fb91 (push 0x3fb to cshell+0x1d7950), cshell+0x1dab70 (0x3ed: owner command +0xc4 with 1 and
// the chosen product)
pub const OWNER_CMD: i32 = 0x3fb;

/// The window: place and size.
// EVIDENCE: cshell+0x1d7d10 (popup texture 0x1db x 0x100, moved to (0x114, 0x100))
pub const WINDOW_AT: [i32; 2] = [0x114, 0x100];
pub const WINDOW_SIZE: [i32; 2] = [0x1db, 0x100];
/// The period field (the list opens under it) and its arrow button (command 1000).
// EVIDENCE: cshell+0x1d7d10 (field 0x1ad x 0x1b at (0x17, 0x7e); arrow 0xf x 0xf at (0x1b0, 0x85), command 1000)
pub const FIELD_AT: [i32; 2] = [0x17, 0x7e];
pub const FIELD_SIZE: [i32; 2] = [0x1ad, 0x1b];
pub const ARROW_AT: [i32; 2] = [0x1b0, 0x85];
pub const ARROW_SIZE: [i32; 2] = [0xf, 0xf];
/// OK (command 6) and Cancel (command 7) with their captions.
// EVIDENCE: cshell+0x1d7d10 (OK string 0x42 at (0x7c, 0xcf), Cancel string 0x43 at (0xf6, 0xcf); caption point
// (0x33, 0xc) centred)
pub const OK_AT: [i32; 2] = [0x7c, 0xcf];
pub const CANCEL_AT: [i32; 2] = [0xf6, 0xcf];
pub const OK_TEXT: i32 = 0x42;
pub const CANCEL_TEXT: i32 = 0x43;
/// The buttons' size (the control: hit box and the picture's clip; the Solid_Bt picture itself is 102 x 25 at the
/// control's corner, the rest of the 128 x 32 texture is clear).
// DISASM: cshell+0x1d804b..0x1d806a (OK sized 0x78 x 0x19 through vfunc +0x144), cshell+0x1d8160..0x1d817f (Cancel the
// same); EVIDENCE: cshell+0x1d8c05..0x1d8d10 (the second page's buttons: 0x78 x 0x19)
pub const BUTTON_SIZE: [i32; 2] = [0x78, 0x19];
/// The buttons' captions: centred about (0x33, 0xc) of the button (the middle of the 102-wide picture, not of the
/// 120-wide control), white in all four states.
// DISASM: cshell+0x1d8070..0x1d80b3 (OK: font 0xe, string 0x42, point (0x33, 0xc), flags 4 | 0x40 = centred about the
// point on both axes, state colours 0xffffffff x 4 -> cshell+0x33e190), cshell+0x1d81b9 (Cancel 0x43, the same)
// EVIDENCE: cshell+0x33e190 (point to vfunc +0x14c, flags or-ed into the text call, colours to +0x44..+0x50)
pub const CAPTION_AT: [i32; 2] = [0x33, 0xc];
pub const CAPTION_COLOUR: u32 = 0xffff_ffff;
/// Texts: title (10, 8), description (0x30, 0x24) wrapped at 0x17c, chosen row (0x23, 0x86), current / remaining
/// balance labels at x 0x1b and their values right-aligned at x 0x1b9 (y 0xa1 / 0xb3).
// EVIDENCE: cshell+0x1d7d10 (font 0xe size 0xe; title / description / row colour 0xffeeeeee / 0xffeeeeee /
// 0xffdddddd; labels 0x5bf / 0x5c0 and values in 0xff828282; values flagged right (0x12))
pub const TITLE_AT: [i32; 2] = [10, 8];
pub const DESC_AT: [i32; 2] = [0x30, 0x24];
pub const DESC_WIDTH: i32 = 0x17c;
pub const CHOICE_AT: [i32; 2] = [0x23, 0x86];
pub const LABEL_X: i32 = 0x1b;
pub const VALUE_X: i32 = 0x1b9;
pub const BALANCE_Y: [i32; 2] = [0xa1, 0xb3];
pub const BALANCE_TEXTS: [i32; 2] = [0x5bf, 0x5c0];
pub const FONT: i32 = 0xe;
pub const FONT_SIZE: i32 = 0xe;
/// Row size of the period list: the reference stack's rows are 16 px apart (size 12 + spacing 4).
// OBSERVED: CLI-910 the reference stack's purchase list (rows 16 px apart); 2009 passes 0xe to the row builder (cshell+0x1da000)
pub const LIST_ROW_SIZE: i32 = 0xc;
pub const TEXT_COLOUR: u32 = 0xffee_eeee;
pub const CHOICE_COLOUR: u32 = 0xffdd_dddd;
pub const GREY: u32 = 0xff82_8282;
/// The payment row (credit products only): place, lamps (credit 0x3ef, prepaid 0x3f0) and labels.
// EVIDENCE: cshell+0x1d7d10 (row 0x154 x 0x14 at (0x3c, 0x67); lamp 0x3f0 at (0x6e, 3), lamp 0x3ef at (0xd7, 3);
// strings 0x184c at (0, 6), 0x184b at (0x82, 6), 0x3e5 at (0xeb, 6) in 0xfffead00)
// DC-DEFAULT: CLI-708 the payment row keeps the constructor's places (2009 moves it by the `BuyItemBox` attribute
// positions, cshell+0x1d9790, not read); lamps 20 x 20; the rank requirement page (window +0x5c) is not built
pub const PAY_AT: [i32; 2] = [0x3c, 0x67];
pub const PREPAID_LAMP_AT: [i32; 2] = [0x6e, 3];
pub const CREDIT_LAMP_AT: [i32; 2] = [0xd7, 3];
pub const LAMP_SIZE: [i32; 2] = [0x14, 0x14];
pub const PAY_TEXTS: [(i32, [i32; 2]); 3] =
    [(0x184c, [0, 6]), (0x184b, [0x82, 6]), (0x3e5, [0xeb, 6])];
pub const PAY_COLOUR: u32 = 0xfffe_ad00;
/// The row form (`%s (%s) - %s NX` / `... GP`): 0x5bd for credit products, 0x5be for GP products.
// EVIDENCE: cshell+0x1da000 (form 0x5bd + (sale type != 1) with the item name, the period and the price)
pub const ROW_FORM: i32 = 0x5bd;
/// The credit products' confirmation (`... for %s use for %s NX.  %s`) in a box titled 0x371.
// EVIDENCE: cshell+0x1da810 (form 0x17db: name, period, price, case note 0x1868 for consume kinds 2 / 6)
pub const NX_CONFIRM: i32 = 0x17db;
/// storage-functions: after the confirmation's OK (and the rank page) the window asks once more, Yes / No, 0x17dd
/// titled 0x371; Yes buys (the owner's command with argument 1), No leaves the window up.
// EVIDENCE: cshell+0x1d7c80 (the 0x17db box's OK -> 0x3ea), cshell+0x1da4a0 (no page -> cshell+0x1d9bc0),
// cshell+0x1d9bc0 (clan items 0x17dc first; else cshell+0x1d96e0), cshell+0x1d96e0 (box 0x17dd titled 0x371, buttons
// 0x35 / 0x36, callback cshell+0x1d7cc0: Yes -> 0x3ed with 1, No -> 0x3ed with 0), cshell+0x1dab70 (0x3ed: argument
// 1 -> owner command, window hidden; 0 -> nothing)
// OBSERVED: storage-functions/pairs/item3/ (the reference stack: 0x17db with OK, then 0x17dd Yes / No)
// DC-DEFAULT: CLI-775 (proposed) the clan-item note 0x17dc (titled 0x17ef, by the item's sub type cshell+0x24b0d0)
// is not shown
pub const NX_LAST_ASK: i32 = 0x17dd;
pub const CASE_NOTE: i32 = 0x1868;

/// Payment modes of the window (+0x88): 0 GP, 1 NX credit, 2 NX prepaid.
// EVIDENCE: cshell+0x1d9af0 (1 -> balance shell +0x108 " NX Credit", 2 -> +0x11c " NX Prepaid", else GP +0x104 and
// the payment row hidden), cshell+0x1da000 (a credit product list opens on mode 2)
pub const MODE_GP: u32 = 0;
pub const MODE_CREDIT: u32 = 1;
pub const MODE_PREPAID: u32 = 2;

/// Texture literals of the window (the user's CShell.dll).
// EVIDENCE: cshell+0x1d7d10 (popup VA 0x374ed944, field VA 0x374cdadc, arrow A..D VA 0x374edb74 / 0x374edb48 /
// 0x374edb1c / 0x374edaf0, button A..D VA 0x374eda10 / 0x374ed9e0 / 0x374ed9b0 / 0x374ed980, lamps off / on VA
// 0x374edca0 / 0x374edc7c)
pub const WINDOW_TEX: Pin = pin("buy window", 0x3ed944, "0d104f95ef3a7368", 35);
pub const ARROW_TEX: Pin = pin("buy window arrow", 0x3edb74, "b8bf6358be3291d6", 42);
pub const BUTTON_TEX: Pin = pin("buy window button", 0x3eda10, "6ac67d1865cd5011", 45);
pub const LAMP_OFF_TEX: Pin = pin("buy window lamp off", 0x3edca0, "5a30d9574a30c771", 33);
pub const LAMP_ON_TEX: Pin = pin("buy window lamp on", 0x3edc7c, "a1244bab42f8af5f", 32);
const fn pin(what: &'static str, rva: u32, sha16: &'static str, len: usize) -> Pin {
    Pin {
        what,
        rva,
        sha16,
        len,
    }
}

/// The window's textures (none without the ROM: fills then).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BuyArt {
    pub window: Option<String>,
    pub field: Option<String>,
    pub arrow: Option<String>,
    pub button: Option<String>,
    pub lamp_off: Option<String>,
    pub lamp_on: Option<String>,
}

impl BuyArt {
    pub fn from_rom(rom: Option<&Path>) -> BuyArt {
        let Some(pe) = rom.and_then(|r| {
            dc_rom::PeFile::from_file(&r.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1])).ok()
        }) else {
            return BuyArt::default();
        };
        let get = |p: &Pin| read_pinned(&pe, p).ok();
        BuyArt {
            window: get(&WINDOW_TEX),
            field: get(&dropdown::FRAME_TEX),
            arrow: get(&ARROW_TEX),
            button: get(&BUTTON_TEX),
            lamp_off: get(&LAMP_OFF_TEX),
            lamp_on: get(&LAMP_ON_TEX),
        }
    }
}

/// storage-functions: the window's rank requirement page (a second 0x1d8 x 0x100 popup at the window's place):
/// title, the item's name, the required rank with its icon, the text, and its buttons.
// EVIDENCE: cshell+0x1d7d10 (page +0x5c Popup_Window at (0x114, 0x100); title 0x1823 at (0xb, 9) in 0xffeeeeee; item
// line +0x70 at (0x15, 0x32); "Requirement: " at (0x15, 0x54); rank name +0x74 at (0x8c, 0x54) in 0xffffa500; rank icon
// +0x7c 0x1e x 0x16 at (0x6e, 0x50); text +0x78 at (0x15, 0x82) wrapped at 0x1b1; buttons 0x78 x 0x19: +0x60 OK
// (0x3ee) at (0xb8, 0xcf), +0x64 OK (0x3ee) at (0x75, 0xcf), +0x68 Details 0x18bd (0x3f1) at (0xf3, 0xcf))
pub const RANK_TITLE: i32 = 0x1823;
pub const RANK_TITLE_AT: [i32; 2] = [0xb, 9];
pub const RANK_ITEM_AT: [i32; 2] = [0x15, 0x32];
pub const RANK_LABEL_AT: [i32; 2] = [0x15, 0x54];
pub const RANK_NAME_AT: [i32; 2] = [0x8c, 0x54];
pub const RANK_ICON_AT: [i32; 2] = [0x6e, 0x50];
pub const RANK_ICON_SIZE: [i32; 2] = [0x1e, 0x16];
pub const RANK_TEXT_AT: [i32; 2] = [0x15, 0x82];
pub const RANK_TEXT_WIDTH: i32 = 0x1b1;
pub const RANK_OK_AT: [i32; 2] = [0xb8, 0xcf];
pub const RANK_OK_CREDIT_AT: [i32; 2] = [0x75, 0xcf];
pub const RANK_DETAILS_AT: [i32; 2] = [0xf3, 0xcf];
pub const RANK_DETAILS: i32 = 0x18bd;
pub const RANK_NAME_COLOUR: u32 = 0xffff_a500;
/// The page's texts: the prepaid note (0x1824) and the credit refusal form (0x18c3, `%s` the rank name).
// EVIDENCE: cshell+0x1da4a0 (mode 1: sprintf(0x18c3, rank name), OK +0x64 and Details +0x68 shown, +0x60 hidden;
// mode 2: 0x1824, +0x60 shown); the literals "Item Name:  ^0xffffa500^%s^" and "Requirement: " of cshell+0x1da4a0 /
// cshell+0x1d7d10
pub const RANK_PREPAID_TEXT: i32 = 0x1824;
pub const RANK_CREDIT_FORM: i32 = 0x18c3;
pub const RANK_ITEM_LABEL: &str = "Item Name:  ";
/// CLI-1630: the GP form of the page (`%s` the rank name): 0x18c3's first sentence without "using NX Credit"; the
/// 2009 client has no rank text for GP products (it draws the cell locked and buys anyway, cshell+0x1442f0 /
/// cshell+0x1da810 test no rank).
// DC-DEFAULT: CLI-1630 (proposed) our own line, after 0x18c3
pub const RANK_GP_FORM: &str =
    "You must be rank ^0xffffa500^%s^ or higher in order to purchase this item.";
pub const RANK_LABEL: &str = "Requirement: ";

/// The open rank requirement page: the credit form (OK + Details, opened by the NX Credit lamp), the prepaid form
/// (OK, opened after the purchase confirmation) or CLI-1630's GP form (OK, opened by OK on a GP product; the purchase
/// does not go on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RankPage {
    pub credit: bool,
    pub gp: bool,
}

impl RankPage {
    pub const CREDIT: RankPage = RankPage {
        credit: true,
        gp: false,
    };
    pub const PREPAID: RankPage = RankPage {
        credit: false,
        gp: false,
    };
    pub const GP: RankPage = RankPage {
        credit: false,
        gp: true,
    };
}

/// One product of the window's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub product: u32,
    pub price: u32,
    /// 0 GP, 1 credit.
    pub sale_type: i32,
    pub bonus: i32,
    /// Consume kind and period of the product's first item.
    pub consume: i32,
    pub period: i32,
    /// Item numbers of the product's items.
    pub items: Vec<i32>,
}

/// The products of the cell of `item_no` with sale type `sale_type`: every product the shop shows (0x351) whose
/// first item is `item_no`, in product number order.
// EVIDENCE: cshell+0x1da000 (the cell's products (cshell+0x24fe50 / 0x24fe70 / 0x24fe10) whose shown flag +0xc is 1)
// DC-DEFAULT: CLI-705 the cell's products are the shown products of the same item and sale type in ascending product
// number (the 2009 cell map is keyed by item number and type; its order is not read)
pub fn offers(wares: &Wares, store: &Store, item_no: i32, sale_type: i32) -> Vec<Offer> {
    let (Some(cat), Some(view)) = (wares.catalog(), store.view.as_ref()) else {
        return Vec::new();
    };
    let mut ids: Vec<u32> = view.clone();
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter()
        .filter_map(|pid| {
            let p = cat.product(pid as i32)?;
            let items: Vec<(i32, i32, i32)> = p
                .items
                .iter()
                .filter_map(|&(ix, consume, period)| Some((cat.info(ix)?.itemid, consume, period)))
                .collect();
            let &(first, consume, period) = items.first()?;
            (first == item_no && p.sale_type == sale_type).then(|| Offer {
                product: pid,
                price: store
                    .prices
                    .get(&pid)
                    .copied()
                    .unwrap_or(p.sale_price.max(0) as u32),
                sale_type: p.sale_type,
                bonus: p.bonus,
                consume,
                period,
                items: items.iter().map(|i| i.0).collect(),
            })
        })
        .collect()
}

/// The period of an offer as the list shows it: days (permanent for 0 / 999, "One Time Use" for a function item
/// of period 0), or a count for consume kinds 2, 3 and 6; the bonus GP after it.
// EVIDENCE: cshell+0x1da000 (kind < 2 or (> 3 and != 6): 0 / 999 -> "Permanent" (type 0x15 with 0: "One Time Use"),
// 1 -> "%d day", else "%d days"; kinds 2 / 3 / 6: "%d item" / "%d items"; bonus: " +" n " GP")
pub fn period_text(o: &Offer, item_kind: Option<i32>) -> String {
    let n = o.period;
    let mut s = if o.consume < 2 || (o.consume > 3 && o.consume != 6) {
        match n {
            0 if item_kind == Some(0x15) => "One Time Use".to_string(),
            0 | 999 => "Permanent".to_string(),
            1 => "1 day".to_string(),
            _ => format!("{n} days"),
        }
    } else if n == 1 {
        "1 item".to_string()
    } else {
        format!("{n} items")
    };
    if o.bonus != 0 {
        s.push_str(&format!(" +{} GP", grouped(i64::from(o.bonus))));
    }
    s
}

/// Fills `%s` of a 2009 form in order.
pub fn fill_form(form: &str, args: &[&str]) -> String {
    let mut out = String::new();
    let mut rest = form;
    let mut k = 0;
    while let Some(i) = rest.find("%s") {
        out.push_str(&rest[..i]);
        out.push_str(args.get(k).copied().unwrap_or(""));
        k += 1;
        rest = &rest[i + 2..];
    }
    out.push_str(rest);
    out
}

/// The title string of the window by item type.
// EVIDENCE: cshell+0x24af10 (types 0..10, 0x14, 0x18, 0x1a, 0x1b, 0x1d, 0x1e -> 0x17e2; 0xb..0x10 -> 0x15f5; 0x11..0x13
// -> 0x17e9; 0x15 -> 0x17e6, 0x16 -> 0x17e4, 0x17 -> 0x17e2)
pub fn title_id(item_kind: i32) -> i32 {
    match item_kind {
        0xb..=0x10 => 0x15f5,
        0x11..=0x13 => 0x17e9,
        0x15 => 0x17e6,
        0x16 => 0x17e4,
        _ => 0x17e2,
    }
}

/// The description string by item type and sale type.
// EVIDENCE: cshell+0x24afd0 (credit 0x180f; GP: weapons 0x1803, body 0x3d3, options 0x17ea; function items 0x1803)
pub fn desc_id(item_kind: i32, sale_type: i32) -> i32 {
    if sale_type == 1 && !matches!(item_kind, 0x15..=0x17) {
        return 0x180f;
    }
    match item_kind {
        0xb..=0x10 => 0x3d3,
        0x11..=0x13 => 0x17ea,
        0x15 if sale_type == 1 => 0x17e7,
        0x16 if sale_type == 1 => 0x18ae,
        _ => 0x1803,
    }
}

/// The open purchase window.
#[derive(Debug, Clone, PartialEq)]
pub struct BuyBox {
    pub item_no: i32,
    pub item_kind: Option<i32>,
    pub name: String,
    pub offers: Vec<Offer>,
    pub rows: Vec<String>,
    pub chosen: usize,
    pub mode: u32,
    pub drop: Option<Dropdown>,
    /// storage-functions: the experience the item's credit cell asks (0 = none), the character's experience, the
    /// required rank's name and icon, and the open rank requirement page.
    pub rank_req: u32,
    pub exp: u32,
    pub needed_rank: String,
    pub rank_icon: Option<String>,
    pub rank_page: Option<RankPage>,
}

/// What a press in the window did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxPress {
    /// Inside the window, nothing to do for the shop.
    Inside,
    /// Outside the window (the window stays; the press is taken).
    Outside,
    /// OK on a GP product, or on a credit product after its confirmation: buy `product` with payment `mode`.
    Buy {
        product: u32,
        mode: u32,
    },
    /// OK on a credit product: ask first.
    Confirm,
    Cancel,
    /// storage-functions: the rank page's OK closed it; `proceed`: the prepaid form, the purchase goes on.
    RankClosed {
        proceed: bool,
    },
    /// storage-functions: the rank page's Details (the 2009 charge page, CLI-773).
    Charge,
}

fn abs(at: [i32; 2]) -> [i32; 2] {
    [WINDOW_AT[0] + at[0], WINDOW_AT[1] + at[1]]
}

fn inside(at: [i32; 2], size: [i32; 2], p: [f32; 2]) -> bool {
    p[0] >= at[0] as f32
        && p[1] >= at[1] as f32
        && p[0] < (at[0] + size[0]) as f32
        && p[1] < (at[1] + size[1]) as f32
}

const TL: Align = Align {
    first: ALIGN_LEFT,
    second: 0,
};
const TR: Align = Align {
    first: ALIGN_RIGHT,
    second: 0,
};
const MID: Align = Align {
    first: ALIGN_CENTRE,
    second: ALIGN_SECOND_CENTRE,
};

impl BuyBox {
    /// The window for the cell of `item_no` (sale type `sale_type`); `None` when the shop shows no product of it.
    pub fn open(
        cat: &Catalog,
        wares: &Wares,
        store: &Store,
        item_no: i32,
        sale_type: i32,
    ) -> Option<BuyBox> {
        let offers = offers(wares, store, item_no, sale_type);
        if offers.is_empty() {
            return None;
        }
        let item_kind = wares.kind_of(item_no);
        let name = wares.label_of(item_no);
        let rows = offers
            .iter()
            .map(|o| {
                let form = cat.text(ROW_FORM + i32::from(o.sale_type != 1));
                fill_form(
                    &form,
                    &[
                        &name,
                        &period_text(o, item_kind),
                        &grouped(i64::from(o.price)),
                    ],
                )
            })
            .collect();
        // EVIDENCE: cshell+0x1da000 (first product credit -> mode 2 and the payment row, else mode 0); cshell+0x1dae90
        // (row 0 picked)
        let mode = if offers[0].sale_type == 1 {
            MODE_PREPAID
        } else {
            MODE_GP
        };
        Some(BuyBox {
            item_no,
            item_kind,
            name,
            offers,
            rows,
            chosen: 0,
            mode,
            drop: None,
            rank_req: 0,
            exp: 0,
            needed_rank: String::new(),
            rank_icon: None,
            rank_page: None,
        })
    }

    pub fn offer(&self) -> &Offer {
        &self.offers[self.chosen.min(self.offers.len() - 1)]
    }

    /// The balance the window shows for its payment mode.
    pub fn balance(&self, data: &MenuData) -> i64 {
        match self.mode {
            MODE_CREDIT => i64::from(data.balance.0),
            MODE_PREPAID => i64::from(data.balance.1),
            _ => i64::from(data.profile.gp),
        }
    }

    fn unit(&self) -> &'static str {
        match self.mode {
            MODE_CREDIT => " NX Credit",
            MODE_PREPAID => " NX Prepaid",
            _ => " GP",
        }
    }

    /// The current and the remaining balance texts.
    // EVIDENCE: cshell+0x1d94a0 (0 -> "Not Enough", else n + unit), cshell+0x1d95c0 (balance - price + unit)
    pub fn balance_texts(&self, data: &MenuData) -> [String; 2] {
        let b = self.balance(data);
        let now = if b == 0 {
            "Not Enough".to_string()
        } else {
            format!("{}{}", grouped(b), self.unit())
        };
        let left = b - i64::from(self.offer().price);
        let after = if left < 0 {
            format!("-{}{}", grouped(-left), self.unit())
        } else {
            format!("{}{}", grouped(left), self.unit())
        };
        [now, after]
    }

    fn field_rect(&self) -> [i32; 4] {
        let a = abs(FIELD_AT);
        [a[0], a[1], FIELD_SIZE[0], FIELD_SIZE[1]]
    }

    /// Opens or closes the period list (command 1000).
    // EVIDENCE: cshell+0x1dab70 (1000 toggles the list +0x4c; a picked row runs 0x3e9 -> cshell+0x1d9de0)
    pub fn toggle_list(&mut self) {
        if self.drop.take().is_some() {
            return;
        }
        let rows = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| (r.clone(), i as i32))
            .collect();
        self.drop = Some(Dropdown::new(
            "buy_period",
            self.field_rect(),
            rows,
            LIST_ROW_SIZE,
            true,
        ));
    }

    /// Row `index` picked.
    pub fn pick(&mut self, index: usize) {
        if index < self.offers.len() {
            self.chosen = index;
        }
    }

    /// A press at `p`.
    pub fn press(&mut self, p: [f32; 2]) -> BoxPress {
        if !inside(WINDOW_AT, WINDOW_SIZE, p) {
            return BoxPress::Outside;
        }
        if let Some(page) = self.rank_page {
            // EVIDENCE: cshell+0x1dab70 (0x3ee: the page hidden; 0x3f1: the charge page)
            let ok_at = if page.credit {
                RANK_OK_CREDIT_AT
            } else {
                RANK_OK_AT
            };
            if inside(abs(ok_at), BUTTON_SIZE, p) {
                self.rank_page = None;
                return BoxPress::RankClosed {
                    proceed: page == RankPage::PREPAID,
                };
            }
            if page.credit && inside(abs(RANK_DETAILS_AT), BUTTON_SIZE, p) {
                return BoxPress::Charge;
            }
            return BoxPress::Inside;
        }
        if inside(abs(ARROW_AT), ARROW_SIZE, p) || inside(abs(FIELD_AT), FIELD_SIZE, p) {
            self.toggle_list();
            return BoxPress::Inside;
        }
        if inside(abs(OK_AT), BUTTON_SIZE, p) {
            // EVIDENCE: cshell+0x1da810 (GP product -> 0x3ed at once; credit product -> the confirmation box)
            let o = self.offer();
            return if o.sale_type == 0 {
                // CLI-1630: a GP product whose cell asks more experience than the character has shows the GP form
                if let Some(page) = self.rank_check(self.mode) {
                    self.rank_page = Some(page);
                    return BoxPress::Inside;
                }
                BoxPress::Buy {
                    product: o.product,
                    mode: self.mode,
                }
            } else {
                BoxPress::Confirm
            };
        }
        if inside(abs(CANCEL_AT), BUTTON_SIZE, p) {
            return BoxPress::Cancel;
        }
        if self.mode != MODE_GP {
            let pay = abs(PAY_AT);
            let at = |q: [i32; 2]| [pay[0] + q[0], pay[1] + q[1]];
            if inside(at(CREDIT_LAMP_AT), LAMP_SIZE, p) {
                // EVIDENCE: cshell+0x1dab70 (0x3ef: cshell+0x1da4a0(product, 1) first; its page keeps the mode)
                match self.rank_check(MODE_CREDIT) {
                    Some(page) => self.rank_page = Some(page),
                    None => self.mode = MODE_CREDIT,
                }
            } else if inside(at(PREPAID_LAMP_AT), LAMP_SIZE, p) {
                self.mode = MODE_PREPAID;
            }
        }
        BoxPress::Inside
    }

    /// The rank requirement check of the chosen product for payment `mode`: the page to show, if any.
    // EVIDENCE: cshell+0x1da4a0 (credit products only (+0x18 = 1); the cell's requirement cshell+0x24fde0; mode 1: a
    // requirement above the character's experience (shell +0x194); mode 2: any requirement above 0)
    // DC-DEFAULT: CLI-1630 (proposed) a GP product with a requirement above the character's experience shows the GP
    // form (2009 checks no rank for GP products; the lobby refuses them, LG-302)
    pub fn rank_check(&self, mode: u32) -> Option<RankPage> {
        if self.rank_req == 0 {
            return None;
        }
        if self.offer().sale_type != 1 {
            return (self.rank_req > self.exp).then_some(RankPage::GP);
        }
        match mode {
            MODE_CREDIT if self.rank_req > self.exp => Some(RankPage::CREDIT),
            MODE_PREPAID => Some(RankPage::PREPAID),
            _ => None,
        }
    }

    /// The rank page's text lines (the credit form filled with the rank name, wrapped at the page's width).
    pub fn rank_lines(&self, cat: &mut Catalog, page: RankPage) -> Vec<String> {
        let raw = if page.gp {
            fill_form(RANK_GP_FORM, &[&self.needed_rank])
        } else if page.credit {
            fill_form(&cat.text(RANK_CREDIT_FORM), &[&self.needed_rank])
        } else {
            cat.text(RANK_PREPAID_TEXT)
        };
        // DC-DEFAULT: CLI-774 (proposed) the page's text is drawn without its colour codes (the rank name of the
        // credit form in the text colour)
        let plain = plain_text(&raw);
        let mut out = Vec::new();
        for para in plain.split('\n') {
            let para = para.trim();
            out.extend(wrap_lines(para, RANK_TEXT_WIDTH as f32, |s| {
                cat.measure_text(FONT, FONT_SIZE, s)
                    .map_or(s.chars().count() as f32 * 7.0, |m| m.0)
            }));
        }
        out
    }

    fn draw_rank_page(
        &self,
        cat: &mut Catalog,
        art: &BuyArt,
        page: RankPage,
        out: &mut Vec<DrawItem>,
    ) {
        match art.window.as_deref() {
            Some(t) => out.push(image_fit(cat, t, WINDOW_AT, WINDOW_SIZE)),
            None => out.push(fill(WINDOW_AT, WINDOW_SIZE, [24, 24, 24, 240])),
        }
        let text = |s: &str, at: [i32; 2], colour: u32| {
            place_text(s, abs(at), [0, 0], TL, FONT, FONT_SIZE, colour)
        };
        out.push(text(&cat.text(RANK_TITLE), RANK_TITLE_AT, TEXT_COLOUR));
        out.push(text(RANK_ITEM_LABEL, RANK_ITEM_AT, TEXT_COLOUR));
        let label_w = cat
            .measure_text(FONT, FONT_SIZE, RANK_ITEM_LABEL)
            .map_or(70.0, |m| m.0) as i32;
        out.push(text(
            &self.name,
            [RANK_ITEM_AT[0] + label_w, RANK_ITEM_AT[1]],
            RANK_NAME_COLOUR,
        ));
        out.push(text(RANK_LABEL, RANK_LABEL_AT, TEXT_COLOUR));
        if let Some(t) = self.rank_icon.as_deref() {
            out.push(image_fit(cat, t, abs(RANK_ICON_AT), RANK_ICON_SIZE));
        }
        out.push(text(&self.needed_rank, RANK_NAME_AT, RANK_NAME_COLOUR));
        let line_h = FONT_SIZE + 2;
        for (k, l) in self.rank_lines(cat, page).iter().enumerate() {
            let at = [RANK_TEXT_AT[0], RANK_TEXT_AT[1] + k as i32 * line_h];
            out.push(text(l, at, TEXT_COLOUR));
        }
        let buttons: &[(i32, [i32; 2])] = if page.credit {
            &[
                (OK_TEXT, RANK_OK_CREDIT_AT),
                (RANK_DETAILS, RANK_DETAILS_AT),
            ]
        } else {
            &[(OK_TEXT, RANK_OK_AT)]
        };
        // EVIDENCE: cshell+0x1d7d10 (the page's OK / Details captions: (0x33, 0xc) centred, 0xffffffff, as the window's)
        for (id, at) in buttons {
            let caption = cat.text(*id);
            draw_button(cat, art.button.as_deref(), abs(*at), &caption, out);
        }
    }

    /// The text of the credit products' confirmation box.
    pub fn confirm_text(&self, cat: &Catalog) -> String {
        let o = self.offer();
        let note = if matches!(o.consume, 2 | 6) {
            cat.text(CASE_NOTE)
        } else {
            String::new()
        };
        // the confirmation counts uses as "time(s)" (cshell+0x1da810)
        let period = period_text(o, self.item_kind)
            .replace(" items", " times")
            .replace(" item", " time");
        let t = fill_form(
            &cat.text(NX_CONFIRM),
            &[&self.name, &period, &grouped(i64::from(o.price)), &note],
        );
        // the form's name carries its colour code; the message box draws the runs (CLI-803)
        t
    }

    pub fn draw(&self, cat: &mut Catalog, art: &BuyArt, data: &MenuData, out: &mut Vec<DrawItem>) {
        if let Some(page) = self.rank_page {
            self.draw_rank_page(cat, art, page, out);
            return;
        }
        match art.window.as_deref() {
            Some(t) => out.push(image_fit(cat, t, WINDOW_AT, WINDOW_SIZE)),
            None => out.push(fill(WINDOW_AT, WINDOW_SIZE, [24, 24, 24, 240])),
        }
        let text = |s: &str, at: [i32; 2], align: Align, colour: u32| {
            place_text(s, abs(at), [0, 0], align, FONT, FONT_SIZE, colour)
        };
        let ty = self.item_kind.unwrap_or(0);
        out.push(text(&cat.text(title_id(ty)), TITLE_AT, TL, TEXT_COLOUR));
        let desc = cat.text(desc_id(ty, self.offer().sale_type));
        let lines = wrap_lines(&desc, DESC_WIDTH as f32, |s| {
            cat.measure_text(FONT, FONT_SIZE, s)
                .map_or(s.chars().count() as f32 * 7.0, |m| m.0)
        });
        let line_h = FONT_SIZE + 2;
        for (k, l) in lines.iter().enumerate() {
            let at = [DESC_AT[0], DESC_AT[1] + k as i32 * line_h];
            out.push(text(l, at, TL, TEXT_COLOUR));
        }
        // the period field, its arrow and the chosen row
        let f = self.field_rect();
        // CLI-910: the 32 x 32 field texture is drawn as nine parts (corners kept, edges stretched), as the list frame
        // EVIDENCE: cshell+0x1d7d10 (field control made with BOX_32_32.dtx at 0x1ad x 0x1b, as the list's frame)
        let drawn = match art.field.as_deref() {
            Some(t) => super::nine::draw(cat, t, f, [255; 4], out),
            None => 0,
        };
        if drawn == 0 {
            out.push(fill([f[0], f[1]], [f[2], f[3]], [0, 0, 0, 204]));
        }
        if let Some(t) = art.arrow.as_deref() {
            out.push(image_fit(cat, t, abs(ARROW_AT), ARROW_SIZE));
        }
        if let Some(r) = self.rows.get(self.chosen) {
            out.push(text(r, CHOICE_AT, TL, CHOICE_COLOUR));
        }
        // the payment row of credit products
        if self.mode != MODE_GP {
            let pay = abs(PAY_AT);
            for (id, at) in PAY_TEXTS {
                out.push(place_text(
                    &cat.text(id),
                    [pay[0] + at[0], pay[1] + at[1]],
                    [0, 0],
                    TL,
                    FONT,
                    FONT_SIZE,
                    PAY_COLOUR,
                ));
            }
            for (at, on) in [
                (CREDIT_LAMP_AT, self.mode == MODE_CREDIT),
                (PREPAID_LAMP_AT, self.mode == MODE_PREPAID),
            ] {
                let tex = if on { &art.lamp_on } else { &art.lamp_off };
                if let Some(t) = tex.as_deref() {
                    out.push(image_fit(
                        cat,
                        t,
                        [pay[0] + at[0], pay[1] + at[1]],
                        LAMP_SIZE,
                    ));
                }
            }
        }
        // balances
        let values = self.balance_texts(data);
        for (k, y) in BALANCE_Y.iter().enumerate() {
            out.push(text(&cat.text(BALANCE_TEXTS[k]), [LABEL_X, *y], TL, GREY));
            out.push(text(&values[k], [VALUE_X, *y], TR, GREY));
        }
        // OK / Cancel
        for (at, id) in [(OK_AT, OK_TEXT), (CANCEL_AT, CANCEL_TEXT)] {
            let caption = cat.text(id);
            draw_button(cat, art.button.as_deref(), abs(at), &caption, out);
        }
    }
}

/// The point a button's caption is centred about (`at`: the button's corner on the screen).
pub fn caption_point(at: [i32; 2]) -> [i32; 2] {
    [at[0] + CAPTION_AT[0], at[1] + CAPTION_AT[1]]
}

/// A code-built OK / Cancel button: the Solid_Bt picture from the control's corner (clipped to 0x78 x 0x19) and the
/// caption centred about [`CAPTION_AT`].
fn draw_button(
    cat: &mut Catalog,
    tex: Option<&str>,
    at: [i32; 2],
    caption: &str,
    out: &mut Vec<DrawItem>,
) {
    match tex {
        Some(t) => out.push(image_fit(cat, t, at, BUTTON_SIZE)),
        None => out.push(fill(at, BUTTON_SIZE, [60, 60, 60, 255])),
    }
    out.push(place_text(
        caption,
        caption_point(at),
        [0, 0],
        MID,
        FONT,
        FONT_SIZE,
        CAPTION_COLOUR,
    ));
}

/// A text without its colour codes.
fn plain_text(raw: &str) -> String {
    super::shop::colour_runs(raw, TEXT_COLOUR)
        .into_iter()
        .map(|(t, _)| t)
        .collect()
}

/// storage-functions: the experience the item's cell of `sale_type` asks (its catalogue row's GP / NX unlock), with
/// the name and icon of the rank of that experience.
// EVIDENCE: cshell+0x24fde0 (the cell's requirement: +0x14 / +0x10), cshell+0x255860 (GP cells take the item row's
// first requirement, credit cells the second), cshell+0x367f10 / cshell+0x367e90 (the rank of an experience and its
// name)
pub fn needed_rank_of(
    cat: &Catalog,
    wares: &Wares,
    store: &Store,
    item_no: i32,
    sale_type: i32,
) -> (u32, String, Option<String>) {
    let req = offers(wares, store, item_no, sale_type)
        .first()
        .and_then(|o| wares.ware(o.product, &store.prices))
        .map_or(0, |w| w.unlock_exp);
    let (name, icon) = rank_of_exp(cat, req);
    (req, name, icon)
}

/// The name and icon of the rank of experience `exp` (empty / none without a ROM).
pub fn rank_of_exp(cat: &Catalog, exp: u32) -> (String, Option<String>) {
    let Some(rom) = cat.rom.as_deref() else {
        return (String::new(), None);
    };
    let levels = crate::hud::board::LevelTable::from_rom(rom);
    let level = levels.as_ref().map_or(0, |l| l.level_of(exp));
    let name = super::result::ResultArt::from_rom(rom)
        .map(|a| a.name(level).to_string())
        .unwrap_or_default();
    let icon = levels.and_then(|l| l.icon_of_exp(exp, "").map(str::to_string));
    (name, icon)
}

/// CLI-1630: the GP form as a plain box titled 0x1823 (a GP supply case's Buy, and the lobby's rank refusal 0x60 of
/// a purchase the character's rank does not reach, LG-302).
pub fn rank_box(needed_rank: &str) -> Popup {
    let mut q = Popup::ok(
        plain_text(&fill_form(RANK_GP_FORM, &[needed_rank])),
        BoxTag::None,
    );
    q.title = RANK_TITLE;
    q
}

/// CLI-1630: the purchase window's rank fields for the cell of `item_no` / `sale_type` and the character's `exp`.
pub fn set_rank(
    b: &mut BuyBox,
    cat: &Catalog,
    wares: &Wares,
    store: &Store,
    sale_type: i32,
    exp: u32,
) {
    let (req, name, icon) = needed_rank_of(cat, wares, store, b.item_no, sale_type);
    b.rank_req = req;
    b.exp = exp;
    b.needed_rank = name;
    b.rank_icon = icon;
}

/// The short-of-NX box: 0x1873 titled 0x17e5 with Charge NX (0x17ff) and Cancel (0x43); Charge NX opens the charge
/// page.
// EVIDENCE: cshell+0x145b60 (buttons 0x17ff / 0x43, callback cshell+0x13f860 -> shop command 0x40a), cshell+0x154a60
// / cshell+0x154c60 (the Modification screen's: the same box)
pub const SHORT_NX_TEXT: i32 = 0x1873;
pub const CHARGE_TEXT: i32 = 0x17ff;

pub fn short_nx_box(cat: &Catalog) -> Popup {
    let mut q = Popup::ask(cat.text(SHORT_NX_TEXT), BoxTag::NxShort);
    q.title = SHORT_NX_TITLE;
    q.captions = Some([CHARGE_TEXT, CANCEL_TEXT]);
    q
}

/// The 0x17dd question of a credit purchase of `product`.
pub fn nx_last_ask(cat: &Catalog, product: u32) -> Popup {
    Popup::ask(cat.text(NX_LAST_ASK), BoxTag::BuyNx(product))
}

/// The confirmation of a credit product (0x17db, OK only); its OK runs the rank check.
pub fn nx_confirm_box(text: String, product: u32) -> Popup {
    Popup::ok(text, BoxTag::Buy(product))
}

/// What every Charge NX opens (the shop's NX Charge 1034, Modification's 1024, the short-of-NX box's Charge NX, the
/// purchase window's Details): a plain OK box saying where NX comes from, no charge page, no browser.
// USER RULE 2026-10-28 (CLI-913): replaces CLI-773's box naming the 2009 charge page
pub const NX_EARNED: &str = "NX is earned from daily logins and clan wins";
pub fn charge_box() -> Popup {
    Popup::ok(NX_EARNED.to_string(), BoxTag::None)
}

fn fill(at: [i32; 2], size: [i32; 2], colour: [u8; 4]) -> DrawItem {
    DrawItem::Fill {
        rect: [at[0] as f32, at[1] as f32, size[0] as f32, size[1] as f32],
        colour,
    }
}

/// The NX balance a purchase with this 0x328 option word is charged to: 0 the credit balance, any other the prepaid
/// balance (dc-lobby-server's CD-37 rule).
// DC-DEFAULT: CLI-1875 (proposed) the client checks and deducts the balance the lobby charges for the option it sends
pub fn charged_balance(data: &MenuData, option: u32) -> i64 {
    if option == 0 {
        i64::from(data.balance.0)
    } else {
        i64::from(data.balance.1)
    }
}

/// Takes an accepted credit purchase's price off the balance its option word is charged to (until the lobby's
/// 0x345 sets the balance again).
// EVIDENCE: cshell+0x150c40 (credit product ok: SalePrice off +0x108 (credit) or +0x11c (prepaid))
pub fn deduct(data: &mut MenuData, option: u32, price: u32) {
    if option == 0 {
        data.balance.0 = data.balance.0.saturating_sub(price);
    } else {
        data.balance.1 = data.balance.1.saturating_sub(price);
    }
}

/// The shop's purchase check of the chosen product (the request, or the box string that refuses it).
// EVIDENCE: cshell+0x145b60 (a request in flight -> res 0x46f; GP products: each item the player owns is extended by
// its unique id, an owned permanent item -> res 0x46d; balance (GP, or credit for mode 1 / prepaid otherwise) below
// the price -> res 0x46e / 0x1873; else 0x328 with option 0 (GP, credit) or mode - 1 (prepaid))
pub fn purchase(
    offer: &Offer,
    mode: u32,
    store: &Store,
    data: &MenuData,
    busy: bool,
) -> Result<LinkCmd, i32> {
    if busy {
        return Err(0x46f);
    }
    let mut extend = [0u32; 5];
    if offer.sale_type == 0 {
        for (k, item) in offer.items.iter().take(5).enumerate() {
            if let Some(r) = store.items.iter().find(|r| r.item_no as i32 == *item) {
                if permanent(r) {
                    return Err(0x46d);
                }
                extend[k] = r.uid;
            }
        }
    }
    let (balance, option, short) = if offer.sale_type != 1 {
        (i64::from(data.profile.gp), 0, 0x46e)
    } else if mode == MODE_CREDIT {
        (i64::from(data.balance.0), 0, 0x1873)
    } else {
        (i64::from(data.balance.1), mode.saturating_sub(1), 0x1873)
    };
    if balance < i64::from(offer.price) {
        return Err(short);
    }
    Ok(LinkCmd::BuyWith {
        product: offer.product,
        option,
        extend,
    })
}

/// storage-functions: Extend Use of the Modification screen's option storage (1019): the purchase window over the
/// picked owned item's products; its OK extends that item (0x328 with its unique id).
// EVIDENCE: cshell+0x15a110 (1019 = 0x3f1 + 10: the picked storage cell (+0x5564; none -> nothing) -> the window with
// the cell's item number and type (cshell+0x1dae90), owner = the screen with command 0x3f6 (cshell+0x1d7950), shown
// (cshell+0x1d77b0); 0x3f6 with argument 1: product -> +0x5618, the window's payment mode (UI object +0x380 + 0x88)
// -> +0x5628, then cshell+0x154c60 while the option storage is up)
#[derive(Debug, Clone, PartialEq)]
pub struct Extension {
    pub window: BuyBox,
    /// The owned item the purchase extends.
    pub uid: u32,
}

/// The Modification screen's command the window answers with.
// EVIDENCE: cshell+0x15a110 (cshell+0x1d7950 with 0x3f6)
pub const EXTEND_OWNER_CMD: i32 = 0x3f6;

impl Extension {
    /// The window for the owned item `item`; `None` when the shop shows no product of it.
    // DC-DEFAULT: CLI-771 (proposed) the window lists the item's GP products, the credit ones when it has none (2009
    // keys the cell's products by item number and type, cshell+0x1dae90 / cshell+0x24fe50; the sale type of the
    // cell it picks is not traced)
    pub fn open(
        cat: &Catalog,
        wares: &Wares,
        store: &Store,
        item: &InvRecord,
    ) -> Option<Extension> {
        let n = item.item_no as i32;
        let window = BuyBox::open(cat, wares, store, n, 0)
            .or_else(|| BuyBox::open(cat, wares, store, n, 1))?;
        Some(Extension {
            window,
            uid: item.uid,
        })
    }

    /// CLI-1172: the window for Buy (1007) on a modification shop cell: the products of the cell's item and sale type,
    /// as the shop's purchase window; `uid` 0 (a new item, no extension). `None` when the shop shows no product of it.
    // EVIDENCE: cshell+0x15a110 (1007 = 0x3ef: no cell -> box 0x3d2; a request in flight -> nothing; else the window
    // with the cell's item number +0x234 and type +0x23c (cshell+0x1dae90), owner = the screen with 0x3f6
    // (cshell+0x1d7950), shown (cshell+0x1d77b0); the yes / no detour before it tests cshell+0x24af00, which returns 0)
    pub fn new_purchase(
        cat: &Catalog,
        wares: &Wares,
        store: &Store,
        item_no: i32,
        sale_type: i32,
    ) -> Option<Extension> {
        let window = BuyBox::open(cat, wares, store, item_no, sale_type)?;
        Some(Extension { window, uid: 0 })
    }
}

/// CLI-1172: the Modification screen's purchase check of a new item (the request, or the box string and title that
/// refuse it).
// EVIDENCE: cshell+0x15a110 (0x3f6 with argument 1 and the option storage closed -> cshell+0x154a60),
// cshell+0x154a60 (a request in flight -> 0x46f titled 0x371; the balance (GP +0x104, credit products the credit
// balance +0x108 whatever the mode) below the price -> 0x46e titled 0x346 or 0x1873 titled 0x17e5; else 0x328 with
// no extension ids and the window's mode as the option word)
// DC-DEFAULT: CLI-1172 (proposed) the option word as CLI-771: the window's payment mode for credit products, 0 for GP
pub fn mod_purchase(
    offer: &Offer,
    mode: u32,
    data: &MenuData,
    busy: bool,
) -> Result<LinkCmd, (i32, i32)> {
    if busy {
        return Err((0x46f, 0x371));
    }
    // CLI-1875: a credit product is checked against the balance the lobby charges for the option word (the prepaid
    // one, CD-37), not the 2009 credit balance: with the credit balance at 0 every NX mod was refused here
    let option = if offer.sale_type == 1 { mode } else { 0 };
    let (balance, short) = if offer.sale_type == 1 {
        (charged_balance(data, option), (0x1873, SHORT_NX_TITLE))
    } else {
        (i64::from(data.profile.gp), (0x46e, 0x346))
    };
    if balance < i64::from(offer.price) {
        return Err(short);
    }
    Ok(LinkCmd::BuyWith {
        product: offer.product,
        option,
        extend: [0; 5],
    })
}

/// The Modification screen's purchase check of an extension (the request, or the box string and title that refuse
/// it).
// EVIDENCE: cshell+0x154c60 (nothing picked -> 0x3d2; a request in flight -> 0x46f titled 0x371; the balance (GP
// +0x104, credit +0x108 for mode 1, else prepaid +0x11c) below the price -> 0x46e titled 0x346 (GP) or 0x1873 titled
// 0x17e5 (credit); else 0x328 with the extension ids (the picked item's unique id, 0, 0, 0, 0) and the window's mode
// as the option word)
// DC-DEFAULT: CLI-771 (proposed) the option word is the window's payment mode as 2009 sends it here (1 credit, 2
// prepaid; the shop's own check sends mode - 1); dc-lobby-server reads any non-zero option as prepaid (CD-37)
pub fn extend_purchase(
    offer: &Offer,
    mode: u32,
    uid: u32,
    data: &MenuData,
    busy: bool,
) -> Result<LinkCmd, (i32, i32)> {
    if busy {
        return Err((0x46f, 0x371));
    }
    // CLI-1875: the balance the lobby charges for the option word sent (mode 1 is charged to the prepaid balance)
    let option = if offer.sale_type == 1 { mode } else { 0 };
    let (balance, short) = if offer.sale_type == 1 {
        (charged_balance(data, option), (0x1873, SHORT_NX_TITLE))
    } else {
        (i64::from(data.profile.gp), (0x46e, 0x346))
    };
    if balance < i64::from(offer.price) {
        return Err(short);
    }
    Ok(LinkCmd::BuyWith {
        product: offer.product,
        option,
        extend: [uid, 0, 0, 0, 0],
    })
}

/// The title of the short-of-NX box (0x1873).
// EVIDENCE: cshell+0x145b60, cshell+0x154a60, cshell+0x154c60 (title 0x17e5)
pub const SHORT_NX_TITLE: i32 = 0x17e5;

/// An owned item without an end and without a count (2009: no extension of permanent items).
// EVIDENCE: cshell+0x226d40 (the owned item's period: permanent -> refused), cshell+0x1db0b0 (function items 0x15
// apart); SPEC: PROTOCOL_2009.md §3.3 table 3.3b (+0x18 end 0 = permanent, +0x24 count)
pub fn permanent(r: &InvRecord) -> bool {
    r.end == 0 && r.count == 0 && r.period == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(consume: i32, period: i32, bonus: i32) -> Offer {
        Offer {
            product: 7,
            price: 12500,
            sale_type: 0,
            bonus,
            consume,
            period,
            items: vec![27],
        }
    }

    #[test]
    fn periods_read_as_the_2009_list_shows_them() {
        assert_eq!(period_text(&offer(0, 0, 0), Some(0)), "Permanent");
        assert_eq!(period_text(&offer(0, 999, 0), Some(0)), "Permanent");
        assert_eq!(period_text(&offer(0, 0, 0), Some(0x15)), "One Time Use");
        assert_eq!(period_text(&offer(0, 1, 0), Some(0)), "1 day");
        assert_eq!(period_text(&offer(1, 30, 0), Some(0)), "30 days");
        assert_eq!(period_text(&offer(3, 5, 0), Some(0)), "5 items");
        assert_eq!(period_text(&offer(0, 7, 1500), Some(0)), "7 days +1,500 GP");
        assert_eq!(
            fill_form("%s (%s) - %s GP", &["G23", "7 days", "1,200"]),
            "G23 (7 days) - 1,200 GP"
        );
    }

    /// CLI-1875: every purchase check tests the balance the lobby charges for the option word it sends (CD-37: 0
    /// credit, else prepaid); the Modification page sends the window's mode, so both of its modes are charged to the
    /// prepaid balance, and the credit balance (0 on our servers) no longer refuses an NX mod.
    #[test]
    fn every_purchase_check_tests_the_balance_its_option_word_is_charged_to() {
        let mut data = MenuData::default();
        let nx = Offer {
            sale_type: 1,
            price: 300,
            ..offer(0, 7, 0)
        };
        let opt = |r: Result<LinkCmd, (i32, i32)>| match r {
            Ok(LinkCmd::BuyWith { option, .. }) => Ok(option),
            Ok(other) => panic!("{other:?}"),
            Err(e) => Err(e.0),
        };
        data.balance = (0, 5000);
        assert_eq!(opt(mod_purchase(&nx, MODE_CREDIT, &data, false)), Ok(1));
        assert_eq!(opt(mod_purchase(&nx, MODE_PREPAID, &data, false)), Ok(2));
        assert_eq!(
            opt(extend_purchase(&nx, MODE_CREDIT, 9, &data, false)),
            Ok(1)
        );
        assert_eq!(
            opt(extend_purchase(&nx, MODE_PREPAID, 9, &data, false)),
            Ok(2)
        );
        data.balance = (5000, 0);
        assert_eq!(
            opt(mod_purchase(&nx, MODE_CREDIT, &data, false)),
            Err(0x1873)
        );
        assert_eq!(
            opt(extend_purchase(&nx, MODE_CREDIT, 9, &data, false)),
            Err(0x1873)
        );
        // the shop's own window: credit mode sends option 0 and is checked on the credit balance
        let store = Store::default();
        assert!(purchase(&nx, MODE_CREDIT, &store, &data, false).is_ok());
        assert_eq!(
            purchase(&nx, MODE_PREPAID, &store, &data, false).err(),
            Some(0x1873)
        );
        // the deduction of an accepted answer takes the same balance
        assert_eq!(
            (charged_balance(&data, 0), charged_balance(&data, 2)),
            (5000, 0)
        );
        deduct(&mut data, 0, 300);
        assert_eq!(data.balance, (4700, 0));
        data.balance = (0, 200);
        deduct(&mut data, 1, 300);
        assert_eq!(data.balance, (0, 0), "never below 0");
    }

    #[test]
    fn the_purchase_check_extends_owned_items_and_refuses_short_balances() {
        let mut data = MenuData::default();
        data.profile.gp = 20000;
        let mut store = Store::default();
        let o = offer(0, 7, 0);
        match purchase(&o, MODE_GP, &store, &data, false) {
            Ok(LinkCmd::BuyWith {
                product, extend, ..
            }) => assert_eq!((product, extend), (7, [0; 5])),
            other => panic!("{other:?}"),
        }
        store.items.push(InvRecord {
            uid: 91,
            item_no: 27,
            end: 1,
            ..InvRecord::default()
        });
        match purchase(&o, MODE_GP, &store, &data, false) {
            Ok(LinkCmd::BuyWith { extend, .. }) => assert_eq!(extend[0], 91),
            other => panic!("{other:?}"),
        }
        store.items[0].end = 0;
        assert_eq!(
            purchase(&o, MODE_GP, &store, &data, false).err(),
            Some(0x46d)
        );
        assert_eq!(
            purchase(&o, MODE_GP, &store, &data, true).err(),
            Some(0x46f)
        );
        data.profile.gp = 100;
        store.items.clear();
        assert_eq!(
            purchase(&o, MODE_GP, &store, &data, false).err(),
            Some(0x46e)
        );
        let nx = Offer { sale_type: 1, ..o };
        data.balance = (20000, 0);
        assert_eq!(
            purchase(&nx, MODE_PREPAID, &store, &data, false).err(),
            Some(0x1873)
        );
        match purchase(&nx, MODE_CREDIT, &store, &data, false) {
            Ok(LinkCmd::BuyWith { option, .. }) => assert_eq!(option, 0),
            other => panic!("{other:?}"),
        }
    }

    fn credit_box(req: u32, exp: u32) -> BuyBox {
        BuyBox {
            item_no: 9001,
            item_kind: Some(0x15),
            name: "Item".into(),
            offers: vec![Offer {
                sale_type: 1,
                ..offer(0, 7, 0)
            }],
            rows: vec!["row".into()],
            chosen: 0,
            mode: MODE_PREPAID,
            drop: None,
            rank_req: req,
            exp,
            needed_rank: "Sergeant".into(),
            rank_icon: None,
            rank_page: None,
        }
    }

    fn at(q: [i32; 2], size: [i32; 2]) -> [f32; 2] {
        [
            (WINDOW_AT[0] + q[0] + size[0] / 2) as f32,
            (WINDOW_AT[1] + q[1] + size[1] / 2) as f32,
        ]
    }

    /// CLI-774: the NX Credit lamp of a credit cell whose requirement is above the character's experience shows the
    /// page's credit form and keeps prepaid; its OK closes it, Details is the charge page; met, the lamp switches.
    #[test]
    fn the_credit_lamp_shows_the_rank_page_while_the_rank_is_missing() {
        let lamp = at(
            [PAY_AT[0] + CREDIT_LAMP_AT[0], PAY_AT[1] + CREDIT_LAMP_AT[1]],
            LAMP_SIZE,
        );
        let mut b = credit_box(5000, 100);
        assert_eq!(b.press(lamp), BoxPress::Inside);
        assert_eq!(b.rank_page, Some(RankPage::CREDIT));
        assert_eq!(b.mode, MODE_PREPAID);
        // the window's own controls are covered by the page
        assert_eq!(b.press(at(ARROW_AT, ARROW_SIZE)), BoxPress::Inside);
        assert!(b.drop.is_none(), "the period list stays shut");
        assert_eq!(b.press(at(RANK_DETAILS_AT, BUTTON_SIZE)), BoxPress::Charge);
        assert_eq!(
            b.press(at(RANK_OK_CREDIT_AT, BUTTON_SIZE)),
            BoxPress::RankClosed { proceed: false }
        );
        assert_eq!(b.rank_page, None);
        let mut b = credit_box(5000, 6000);
        b.press(lamp);
        assert_eq!((b.rank_page, b.mode), (None, MODE_CREDIT));
    }

    /// CLI-910: the period list's rows are 16 px apart as in the reference stack (size 12 + spacing 4).
    #[test]
    fn the_period_list_rows_are_sixteen_pixels_apart() {
        let mut b = credit_box(0, 0);
        b.toggle_list();
        let d = b.drop.as_ref().unwrap();
        assert_eq!(d.font_size + dropdown::SPACING, 16);
    }

    /// CLI-774: on prepaid any requirement shows the page's prepaid form after the confirmation; its OK goes on.
    #[test]
    fn prepaid_purchases_of_a_ranked_cell_show_the_prepaid_form() {
        let mut b = credit_box(5000, 6000);
        assert_eq!(b.rank_check(MODE_PREPAID), Some(RankPage::PREPAID));
        assert_eq!(credit_box(0, 0).rank_check(MODE_PREPAID), None);
        b.rank_page = b.rank_check(MODE_PREPAID);
        assert_eq!(
            b.press(at(RANK_OK_AT, BUTTON_SIZE)),
            BoxPress::RankClosed { proceed: true }
        );
        // a GP product has no prepaid form; its missing rank shows the GP form (CLI-1630)
        let mut gp = credit_box(5000, 6000);
        gp.offers[0].sale_type = 0;
        assert_eq!(gp.rank_check(MODE_PREPAID), None);
    }

    /// CLI-1630: OK on a GP product whose cell asks more experience than the character has shows the page's GP form
    /// (OK only) instead of buying; its OK closes the page and the purchase does not go on; at the requirement OK
    /// buys.
    #[test]
    fn a_gp_product_above_the_rank_shows_the_gp_form_on_ok() {
        let gp_box = |req, exp| {
            let mut b = credit_box(req, exp);
            b.offers[0].sale_type = 0;
            b.mode = MODE_GP;
            b
        };
        let mut b = gp_box(5000, 4999);
        assert_eq!(b.press(at(OK_AT, BUTTON_SIZE)), BoxPress::Inside);
        assert_eq!(b.rank_page, Some(RankPage::GP));
        let mut cat = Catalog::fallback(None, 1024, 768);
        let text = b.rank_lines(&mut cat, RankPage::GP).join(" ");
        assert!(text.contains("must be rank Sergeant or higher"), "{text}");
        assert!(!text.contains("NX"), "{text}");
        assert_eq!(
            b.press(at(RANK_OK_AT, BUTTON_SIZE)),
            BoxPress::RankClosed { proceed: false }
        );
        assert_eq!(b.rank_page, None);
        for (req, exp) in [(5000, 5000), (0, 0)] {
            assert_eq!(
                gp_box(req, exp).press(at(OK_AT, BUTTON_SIZE)),
                BoxPress::Buy {
                    product: 7,
                    mode: MODE_GP
                }
            );
        }
        let q = rank_box("Sergeant");
        assert_eq!(q.title, RANK_TITLE);
        assert_eq!(
            q.text,
            "You must be rank Sergeant or higher in order to purchase this item."
        );
    }

    /// shop-buy-box: OK / Cancel inside the window under the balance lines, captions centred about (0x33, 0xc) (the
    /// middle of the 102 x 25 picture) in white, and both balance lines filled for GP and NX Prepaid.
    #[test]
    fn buttons_sit_in_the_window_with_centred_captions_and_filled_balances() {
        let mut b = credit_box(0, 0);
        b.offers[0].sale_type = 0;
        b.mode = MODE_GP;
        let mut data = MenuData::default();
        data.profile.gp = 12_345;
        data.balance = (700, 50_000);
        let mut cat = Catalog::fallback(None, 1024, 768);
        let mut out = Vec::new();
        b.draw(&mut cat, &BuyArt::default(), &data, &mut out);
        let labels: Vec<([f32; 2], [f32; 2], [u8; 4], String)> = out
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
            .collect();
        for (at, id) in [(OK_AT, OK_TEXT), (CANCEL_AT, CANCEL_TEXT)] {
            let r = abs(at);
            assert!(r[1] >= abs([0, BALANCE_Y[1] + FONT_SIZE])[1]);
            assert!(r[0] + BUTTON_SIZE[0] <= WINDOW_AT[0] + WINDOW_SIZE[0]);
            assert!(r[1] + BUTTON_SIZE[1] <= WINDOW_AT[1] + WINDOW_SIZE[1]);
            let c = labels
                .iter()
                .find(|l| l.3 == cat.text(id))
                .expect("caption");
            assert_eq!(c.0, [(r[0] + 51) as f32, (r[1] + 12) as f32]);
            assert_eq!((c.1, c.2), ([0.5, 0.5], [255; 4]));
        }
        let value = |labels: &[([f32; 2], [f32; 2], [u8; 4], String)], y: i32| {
            let at = abs([VALUE_X, y]);
            labels
                .iter()
                .find(|l| l.0 == [at[0] as f32, at[1] as f32] && l.1 == [1.0, 0.0])
                .map(|l| l.3.clone())
        };
        assert_eq!(value(&labels, BALANCE_Y[0]).as_deref(), Some("12,345 GP"));
        assert_eq!(value(&labels, BALANCE_Y[1]).as_deref(), Some("-155 GP"));
        b.offers[0].sale_type = 1;
        b.mode = MODE_PREPAID;
        assert_eq!(
            b.balance_texts(&data),
            [
                "50,000 NX Prepaid".to_string(),
                "37,500 NX Prepaid".to_string()
            ]
        );
        b.mode = MODE_CREDIT;
        assert_eq!(
            b.balance_texts(&data),
            ["700 NX Credit".to_string(), "-11,800 NX Credit".to_string()]
        );
    }

    #[test]
    fn titles_and_texts_follow_the_item_type() {
        assert_eq!(title_id(0), 0x17e2);
        assert_eq!(title_id(0xb), 0x15f5);
        assert_eq!(title_id(0x12), 0x17e9);
        assert_eq!(desc_id(0, 0), 0x1803);
        assert_eq!(desc_id(0xc, 0), 0x3d3);
        assert_eq!(desc_id(0x11, 0), 0x17ea);
        assert_eq!(desc_id(0, 1), 0x180f);
    }
}
