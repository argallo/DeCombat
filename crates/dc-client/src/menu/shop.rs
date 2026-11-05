//! w3-shop: the 2009 "Shop / Storage" screen (screen state 0x16) with its left tabs (weapons, body, black market,
//! inbox), the shop page (the product grid of the chosen tab and category with its prices), the storage page (the
//! character's items of a category with equip / unequip) and the inbox page ([`super::inbox`]).
//!
//! The lobby bar's Shop / Storage, Black Market and Inbox buttons open this screen on the weapons tab, the black
//! market tab and the inbox page; Exit goes back to the lobby or, from a room, to the waiting room.
// SPEC: PROTOCOL_2009.md §4.1 (0x350 .. 0x359), §4.2 (0x324 .. 0x329), §4.3 (0x32c / 0x32d)
// EVIDENCE: cshell+0x1bf20 (lobby bar: 61 -> screen 0x16 then its command 1000 (cshell+0x140f50); 622 -> 61 then
// command 0x3ea (cshell+0x140f10); 619 -> the screen's inbox flag +0x55f0 then 61; 184 -> screen 0x14)
// EVIDENCE: cshell+0x146fe0 (the screen's constructor, vtable VA 0x374e4744; command handler at vtable +4)
// DISASM: cshell+0x14f830 (the screen's command handler: 1000 / 1001 / 1002 set the left tab +0x5564 to 0 / 1 / 2
// with category +0x5568 = 0 and the shop page; 1003 opens the inbox page; 619 runs 1003; 311 -> screen 9 when the
// shell's room flag +0x18 is set, else screen 0xf)
// DISASM: cshell+0x14b8a0 (the right page +0x5570: 0 shop = window 3 + control 0x21, 1 storage = window 4, 2 inbox =
// window 5 + control 0x21; controls 0x10..0x20 hidden first), cshell+0x14b720 (the shop page shows the category
// buttons of its tab: 0x10..0x15 weapons, 0x16..0x1a body, 0x1b..0x20 black market)
// OBSERVED: lobbyfx4/real/R32_now.png (the 2009 screen: weapons tab, shop page, Primary category)

use std::sync::Arc;

use super::buybox::{self, BoxPress, BuyArt, BuyBox};
use super::catalog::Catalog;
use super::colorbox::{self, ColourStep};
use super::countrybox::{self, CountryStep};
use super::dropdown::Dropdown;
use super::facebox::{self, FaceStep};
use super::inbox;
use super::link::{CharExtras, LinkCmd, LinkEvent};
use super::lobby::{PanelArt, OWN_COLOUR, ROW_COLOUR};
use super::markbox::{self, MarkStep};
use super::megabox::{self, MegaStep};
use super::popup::{text_ids, BoxTag, Popup};
use super::recordbox::{self, RecordStep};
use super::result::grouped;
use super::sheet::{image_fit, place_text, Look, Sheet, ALIGN_CENTRE, ALIGN_LEFT, ALIGN_RIGHT};
use super::slotpick::{self, SlotPick};
use super::storage::{self, equip_rows, own_slot, stored, Ware, Wares, TAB_BODY, TAB_MARKET};
use super::usebox::{self, NameStep, UseAction};
use super::{
    build_sheet, frontfx, pages, pane, Ctx, Go, MenuData, MenuKey, Screen, DOUBLE_CLICK_MS,
};
use crate::hud::items::DrawItem;
use dc_ui_data::pages::{Align, Widget, ALIGN_SECOND_CENTRE};
use dc_ui_data::{PageTag, WidgetKind};

/// Lobby bar commands (the common lobby page): Shop / Storage, Black Market, Modification, Inbox, the bar's exit.
pub const CMD_BAR_SHOP: i32 = 61;
pub const CMD_BAR_MARKET: i32 = 622;
pub const CMD_BAR_TUNING: i32 = 184;
pub const CMD_BAR_INBOX: i32 = 619;
pub const CMD_BAR_QUIT: i32 = 303;

/// The screen's own commands.
pub const CMD_TAB_WEAPONS: i32 = 1000;
pub const CMD_TAB_BODY: i32 = 1001;
pub const CMD_TAB_MARKET: i32 = 1002;
pub const CMD_TAB_INBOX: i32 = 1003;
/// The left tab of the inbox page (no shop shelf, no equipment rows).
pub const GIFTS_TAB: usize = 3;
/// The tab buttons (weapons, body, black market, inbox).
// EVIDENCE: cshell+0x13fd20 (controls 0xc .. 0xf)
pub const TAB_IDS: [i32; 4] = [0xc, 0xd, 0xe, 0xf];
pub const CMD_EXIT: i32 = 311;
pub const CMD_PAGE_BACK: i32 = 1012;
pub const CMD_PAGE_FWD: i32 = 1013;
pub const CMD_BUY: i32 = 266;
/// The shop window's Gift button (window 3, id 1; command 1026).
pub const GIFT_ID: i32 = 1;
pub const CMD_WEAR: i32 = 265;
/// function-unequip: the storage Equip button's captions on the function-item shelf (Black Market category 2): Equip,
/// Unequip, and the caption of an item not in use.
// EVIDENCE: cshell+0x147470 (tab +0x5564 = 2 and category +0x5568 = 2: an equipped cell 0x160f, else 0x1640; a record
// in use whose row SubType is not 0x16: 0x160f while cshell+0x226800 finds it in the function list, else 0x160e, and
// the button enabled only when the row's +0x241 (IsCanUnEquip) is set)
pub const WEAR_CAPTION: i32 = 0x160e;
pub const UNWEAR_CAPTION: i32 = 0x160f;
pub const FUNCTION_IDLE_CAPTION: i32 = 0x1640;
/// function-unequip: the refusals of 0x369 (put in) and 0x36b (take out), titled 0x371.
// EVIDENCE: cshell+0x14cea0 (0x369 not ok: box 0x506, title 0x371, OK 0x42), cshell+0x14d010 (0x36b not ok: box 0x4dd)
pub const FUNCTION_PUT_REFUSED: i32 = 0x506;
pub const FUNCTION_TAKE_REFUSED: i32 = 0x4dd;
/// The Black Market category of the function items (CLI-544).
const FUNCTION_CATEGORY: usize = 2;
pub const CMD_STORAGE_SHOP: i32 = 1010;
/// storage-functions: the storage page's Discard (window 4 button 0, caption 0x15fe).
// DISASM: cshell+0x14f830 (1011 = 0x3f3 -> cshell+0x1505df)
pub const CMD_THROW_OUT: i32 = 1011;
/// storage-functions: the money panel's NX Charge (window 9 button 3).
// DISASM: cshell+0x14f830 (1034 = 0x40a -> cshell+0x15020d: the charge page, CLI-773)
pub const CMD_CHARGE: i32 = 1034;
/// The inbox page's Activate (open) button (window 5 button 0).
// DISASM: cshell+0x15025d..0x150333 (the inbox screen's open command: the key check, then the confirmation, then 0x32e)
pub const CMD_ACTIVATE: i32 = 1027;
/// The HiSec boxes' strings: a key is needed (case, key names); the case is being opened (key name); the award
/// pop-up's HiSec and plain case titles.
// EVIDENCE: HISEC_2009.md 4.2 / 4.3 (res 0x18b9, 0x18ba, 0x18c4, 0x393)
pub const HISEC_NEEDS_KEY: i32 = 0x18b9;
pub const HISEC_OPENING: i32 = 0x18ba;
pub const HISEC_AWARD_TITLE: i32 = 0x18c4;
pub const CASE_AWARD_TITLE: i32 = 0x393;
/// The award pop-up title of a crate: the opened product's consume type is 6.
// DISASM: cshell+0x14e010 (the product row's +0x24 == 6 sets the crate flag passed to cshell+0x1412c0), cshell+0x286d40
// (crate flag -> title res 0x18f7, else res 0x393; key flag -> 0x18c4)
// DC-DEFAULT: CLI-1390 (proposed) a crate's award pop-up is titled 0x18f7
pub const CRATE_AWARD_TITLE: i32 = 0x18f7;
/// The award pop-up's title: a used key means HiSec, a crate product (consume type 6) its own title.
pub fn award_title(keyed: bool, consume: Option<i32>) -> i32 {
    if keyed {
        HISEC_AWARD_TITLE
    } else if consume == Some(CRATE_CONSUME) {
        CRATE_AWARD_TITLE
    } else {
        CASE_AWARD_TITLE
    }
}
/// The consume type that marks a crate product (the product row's +0x24, cshell+0x257bc0).
pub const CRATE_CONSUME: i32 = 6;
/// The generic open question: title, buttons, and the form by item type (function item, option, other).
// EVIDENCE: cshell+0x148ac0 (box title 0x17ec, buttons 0x35 / 0x36, form 0x17b7 for type 0x15, 0x1825 for 0x11..0x13,
// else 0x17b8, filled with the item name)
pub const OPEN_TITLE: i32 = 0x17ec;
/// The category buttons of the three tabs in the order of their control ids (category 0, 1, ...).
// EVIDENCE: cshell+0x14b550 (weapons: controls 0x10..0x15 lit for categories 0..5), cshell+0x14b300 (body),
// cshell+0x14b260 (black market)
pub const CATEGORY_COMMANDS: [&[i32]; 3] = [
    &[251, 252, 253, 254, 288, 1040],
    &[259, 260, 261, 262, 263],
    &[1028, 1036, 1029, 1030, 1031, 1041],
];
/// Control id of the first category button of each tab, of the exit button and of the first page number button.
pub const FIRST_CATEGORY_ID: [i32; 3] = [0x10, 0x16, 0x1b];
pub const EXIT_ID: i32 = 0x21;
pub const PAGE_IDS: [i32; 5] = [36, 37, 38, 39, 40];
/// The paging buttons (back, next).
pub const PAGING_IDS: [i32; 2] = [34, 35];

/// Windows of the screen's page: the left pane, the shop, storage and inbox pages, the two equip dialogs, the
/// drop list, the money panel, the case box.
pub const LEFT_WINDOW: i32 = 2;
pub const SHOP_WINDOW: i32 = 3;
pub const STORAGE_WINDOW: i32 = 4;
pub const INBOX_WINDOW: i32 = 5;
pub const DIALOG_WINDOWS: [i32; 4] = [6, 7, 8, 10];
pub const MONEY_WINDOW: i32 = 9;
/// The inventory grids of the shop and the storage page and the left pane's equipment lists (weapons, body).
pub const SHOP_GRID: i32 = 3;
pub const STORAGE_GRID: i32 = 8;
pub const WEAPON_LIST: i32 = 2;
pub const BODY_LIST: i32 = 4;
/// The nickname button and the rank icon image under the left pane.
pub const NICK_BUTTON_CMD: i32 = 1018;
pub const RANK_IMAGE_ID: i32 = 11;
/// The money panel's value texts: prepaid, credit, GP, grade.
// OBSERVED: lobbyfx4/real/R32_now.png (window 9: NX Prepaid, NX Credit, Grade, GP; the value texts
// of the records at x 206 on the label rows)
pub const MONEY_TEXTS: [i32; 4] = [7, 8, 9, 11];

/// Cells per page.
// EVIDENCE: cshell+0x149f80 (cell i of the page shows entry i + page x 6)
pub const CELLS: usize = 6;

/// round 10: an inbox opening awaiting its answer 0x32f: the row, whether its product is a supply case (type 0x14),
/// and the product's first item number.
#[derive(Debug, Clone, Copy)]
struct Opening {
    entry: u32,
    case: bool,
    item: i32,
    product: u32,
}
/// name-change: a use request in flight (0x330): the item and its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Using {
    uid: u32,
    action: usebox::UseAction,
    /// use-windows: the weapon change's target item number (0 otherwise).
    change: u32,
}
/// Page number buttons per group.
pub const PAGE_GROUP: usize = 5;
/// Cell text: name at (5, 2), price right-aligned at (0xe5, 0x49), font 0x15 size 14 in 0xffa3c3c3.
// EVIDENCE: cshell+0x149f80 (font 0x15 through cshell+0x2c07b0, size 0xe, colour 0xffa3c3c3; name at (5, 2), price
// at (0xe5, 0x49))
pub const CELL_FONT: i32 = 0x15;
pub const CELL_SIZE: i32 = 14;
pub const CELL_COLOUR: u32 = 0xffa3_c3c3;
pub const CELL_NAME_AT: [i32; 2] = [5, 2];
pub const CELL_PRICE_AT: [i32; 2] = [0xe5, 0x49];
/// The money panel's numbers in white (as the lobby's character info).
pub const MONEY_COLOUR: u32 = 0xffff_ffff;
/// The left pane's equipment rows: 0xc2 x 0x47 ([`pane::ROW_SIZE`]), 3 px apart (the lists' item spacing).
// EVIDENCE: cshell+0x1425d0 (list items 0xc2 x 0x47)
// OBSERVED: R32_now.png (rows at y 126, 200, 274, 348)
pub const ROW_SIZE: [i32; 2] = pane::ROW_SIZE;
pub const ROW_PITCH: i32 = pane::ROW_SIZE[1] + 3;
/// Lit cell / row and page fills.
// DC-DEFAULT: CLI-545 the selected cell is lit by a white 10 % fill, a row by a black 40 % fill
pub const LIT_FILL: [u8; 4] = [255, 255, 255, 26];
pub const ROW_FILL: [u8; 4] = [0, 0, 0, 102];

/// The interface sound of a button press.
// EVIDENCE: cshell+0x1aeb90 (a press on a control requests sound id 2 when the control's +0x58 is set, else id 0x14;
// cshell+0x1b6470 keeps the highest id of the frame)
// DC-DEFAULT: CLI-551 the lobby bar's Shop / Storage, Black Market, Modification and Inbox presses play id
// 0x14 (+0x58 of the bar buttons is not traced)
pub const SOUND_PRESS: i32 = frontfx::SOUND_SELECT;

/// Which page the right side shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Shop,
    Storage,
    Inbox,
}

/// A lobby-bar command of the shop family: the screen it opens, where Exit returns to and the press sound.
/// `from` is the screen the bar belongs to. True = taken.
// EVIDENCE: cshell+0x1bf20 (61 -> screen 0x16, 622 -> 0x16 + black market, 619 -> 0x16 + inbox, 184 -> 0x14)
pub fn bar_command(cmd: i32, from: Go, ctx: &mut Ctx) -> bool {
    let to = match cmd {
        CMD_BAR_SHOP => Go::Shop,
        CMD_BAR_MARKET => Go::Market,
        CMD_BAR_INBOX => Go::Inbox,
        CMD_BAR_TUNING => Go::Tuning,
        _ => return false,
    };
    if matches!(from, Go::Lobby | Go::Room) {
        ctx.data.shop_back = from;
    }
    ctx.sounds.push(SOUND_PRESS);
    *ctx.go = Some(to);
    true
}

/// Where Exit goes: the waiting room when the screen was opened from it, else the lobby.
// DISASM: cshell+0x14fee8 (311: shell +0x18 set -> screen 9, else screen 0xf), cshell+0x15a110 (the tuning screen the
// same)
pub fn exit_to(data: &MenuData) -> Go {
    if data.shop_back == Go::Room {
        Go::Room
    } else {
        Go::Lobby
    }
}

/// The cells of inventory grid `id` of window `window` on page `tag` (x, y, w, h; row by row).
// SPEC: FMT_CRES_UI.md §6.3 (inventory grid: control id, parent, parent kind, pos, rows / columns, pos2, cell size,
// cell spacing)
// OBSERVED: lobbyfx4/real/R32_now.png (shop cells x 487 / 750, y 126 / 227 / 328: window 3 + grid pos
// + column x (254 + 9), row x (91 + 10)), R33_mod.png (tuning: 2 rows of 3)
pub fn grid_cells(cat: &Catalog, tag: PageTag, window: i32, id: i32) -> Vec<[i32; 4]> {
    let Some(page) = cat.pages.page(tag) else {
        return Vec::new();
    };
    let wpos = page.widgets.iter().find_map(|w| match &w.widget {
        Widget::Window(r) if r.control_id == window => Some(r.pos),
        _ => None,
    });
    let Some(wpos) = wpos else {
        return Vec::new();
    };
    let Some(g) = page.widgets.iter().find_map(|w| match &w.widget {
        Widget::Inventory(r) if r.parent == window && r.control_id == id => Some(r.clone()),
        _ => None,
    }) else {
        return Vec::new();
    };
    let (rows, cols) = (g.rows_cols[0].clamp(0, 16), g.rows_cols[1].clamp(0, 16));
    let mut out = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            out.push([
                wpos[0] + g.pos[0] + c * (g.cell_size[0] + g.cell_spacing[0]),
                wpos[1] + g.pos[1] + r * (g.cell_size[1] + g.cell_spacing[1]),
                g.cell_size[0],
                g.cell_size[1],
            ]);
        }
    }
    out
}

pub fn inside(r: [i32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] as f32
        && p[1] >= r[1] as f32
        && p[0] < (r[0] + r[2]) as f32
        && p[1] < (r[1] + r[3]) as f32
}

fn fill(r: [i32; 4], colour: [u8; 4]) -> DrawItem {
    DrawItem::Fill {
        rect: [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32],
        colour,
    }
}

const ALIGN_TL: Align = Align {
    first: ALIGN_LEFT,
    second: 0,
};
const LEFT_MID: Align = Align {
    first: ALIGN_LEFT,
    second: ALIGN_SECOND_CENTRE,
};
const RIGHT_TOP: Align = Align {
    first: ALIGN_RIGHT,
    second: 0,
};
const CENTRE: Align = Align {
    first: ALIGN_CENTRE,
    second: ALIGN_SECOND_CENTRE,
};

/// A price as the cells show it: GP products with the GP mark, credit products in NX.
// DC-DEFAULT: CLI-545 the price reads `<n> GP` / `<n> NX` with thousands separators (the 2009 GP mark
// image after the number is not drawn)
pub fn price_text(w: &Ware) -> String {
    let unit = if w.sale_type == 1 { "NX" } else { "GP" };
    format!("{} {unit}", grouped(i64::from(w.price)))
}

/// Draws one grid cell: the name at its top-left and an optional price right-aligned at `at` in the cell.
pub fn draw_cell(
    r: [i32; 4],
    name: &str,
    price: Option<(&str, [i32; 2])>,
    lit: bool,
    colour: u32,
    out: &mut Vec<DrawItem>,
) {
    if lit {
        out.push(fill(r, LIT_FILL));
    }
    out.push(place_text(
        name,
        [r[0] + CELL_NAME_AT[0], r[1] + CELL_NAME_AT[1]],
        [0, 0],
        ALIGN_TL,
        CELL_FONT,
        CELL_SIZE,
        colour,
    ));
    if let Some((p, at)) = price {
        out.push(place_text(
            p,
            [r[0] + at[0], r[1] + at[1]],
            [0, 0],
            RIGHT_TOP,
            CELL_FONT,
            CELL_SIZE,
            colour,
        ));
    }
}

/// A shop cell's frame: normal, selected (the two slot images the cells are built with).
// EVIDENCE: cshell+0x141ef0 (shop page cells: Item_Slot_B then Item_Slot_A, cell 0xfe x 0x5b; the storage page's
// Inven_Item_Slot_Off / _On; the order off, on as the modification cells' Tunning_Shop_Slot_OFF / _ON of
// cshell+0x154480)
/// The part of the screen where a press starts turning the pane's character (virtual pixels: x, y, w, h).
// EVIDENCE: cshell+0x1cd30 (left button down: 0 <= x <= 200 and 110 <= y < 560, both times the screen scale, starts
// the turn and keeps x), cshell+0x1ceb0 (left button up ends it)
pub const FIGURE_DRAG_RECT: [i32; 4] = [0, 110, 201, 450];
/// The turn per screen pixel of horizontal pointer movement (radians).
// EVIDENCE: cshell+0x1fb00 (mouse move while turning: angle = (x - last x) / 60, then last x = x), cshell+0x1e610 (the
// character's rotation turned by the angle about the up axis)
pub const FIGURE_TURN_PER_PIXEL: f32 = 1.0 / 60.0;

/// The character's yaw after the pointer moved `dx` screen pixels while turning it: its front follows the pointer
/// (a move to the right turns the face to the right of the screen).
// OBSERVED: lobby-r14/pairs/item2_drag_reference_rust_2560.png (the reference client after a 150 px drag to
// the right: the back to the camera, the face to the screen's right)
pub fn turned_yaw(yaw: f32, dx: f32) -> f32 {
    (yaw - dx * FIGURE_TURN_PER_PIXEL).rem_euclid(std::f32::consts::TAU)
}

/// The cell the shop frames are made for.
// EVIDENCE: cshell+0x141ef0 (shop page: 0xfe x 0x5b)
pub const SHOP_CELL_SIZE: [i32; 2] = [0xfe, 0x5b];
/// The storage page's cell frames (off, on) and cell size: tab-less, no price box.
// EVIDENCE: cshell+0x141ef0 (storage page: Inven_Item_Slot_Off then Inven_Item_Slot_On, cell 0xfe x 0x56)
// OBSERVED: storage-cells-r14/cpp/st2560/ (reference client: the _On frame on the selected cell and on
// the cell under the pointer, filled or empty)
pub const STORAGE_FRAMES: [&str; 2] = super::tuning::WEAPON_FRAMES;
pub const STORAGE_CELL_SIZE: [i32; 2] = [0xfe, 0x56];
/// CLI-1264 the days / hours left of a temporary item on a storage cell: a label at the PERMANENT mark's place, in
/// the mark's green, the full words when they fit the width left of the item picture, else the short form.
// DC-DEFAULT: CLI-1264 (user 2026-10-29: storage cells keep the period as a label in the panel; the 2009 cell shows
// only PERMANENT, OBSERVED storage-cells-r14/cpp/st2560/shop_bb00_storage.png: a timed M1911 MEU
// shows its name and PISTOL only) text size 11 in the cell font, colour sampled from Permenant_Mark (126, 255, 140)
pub const PERIOD_LABEL_SIZE: i32 = 11;
pub const PERIOD_LABEL_COLOUR: u32 = 0xff7e_ff8c;
pub const PERIOD_LABEL_ROOM: i32 = 0x80;

/// The period label of a storage cell at `at` (its top left): the full words when they fit, else the short form.
pub fn period_label(cat: &mut Catalog, at: [i32; 2], full: &str, short: &str) -> DrawItem {
    let fits = cat
        .measure_text_font_units(CELL_FONT, PERIOD_LABEL_SIZE, full)
        .is_some_and(|w| w <= PERIOD_LABEL_ROOM as f32);
    place_text(
        if fits { full } else { short },
        at,
        [0, 0],
        ALIGN_TL,
        CELL_FONT,
        PERIOD_LABEL_SIZE,
        PERIOD_LABEL_COLOUR,
    )
}

/// The places of a storage cell's marks (screen-virtual, top-left): class / type tag, Equipped badge, PERMANENT mark
/// or period label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageMarkPlaces {
    pub tag: [i32; 2],
    pub equipped: [i32; 2],
    pub period: [i32; 2],
}

/// CLI-1266 the storage cell's mark images sit 1 px below the cell fill's offsets (the reference client draws them so at
/// 1024 x 768 and 2560 x 1440; the frame, the item picture and the name text are not shifted).
// OBSERVED: storage-cells-r14/pairs/storage_cells_reference_left_rust_right_1024x768.png (reference at
// 1024 x 768, cell top 222: PISTOL tag rows 239..245, PERMANENT rows 248..254, Equipped badge from cell + (199, 4))
pub const STORAGE_MARK_DY: i32 = 1;

/// Where a storage cell at `r` with tag `tag` puts its marks.
// EVIDENCE: DISASM cshell+0x2fa1dd .. 0x2fa221 (class tag at cell x + 5, y + 0x10, its source rectangle the tag's
// size), cshell+0x2f97f0 (the PERMANENT mark at x + 5, the tag's y + its height (vtable +0x1c) + 1; the tag textures
// are 8 texels high), cshell+0x2fa310 (the Equipped badge at the cell's right end less 1 less its width, y + 1)
// DC-DEFAULT: CLI-1266 the tag's height counted for the mark is its texture's height (8 for every Item_Info_Mark tag),
// the badge 1 px further left than the cell width suggests (the reference stack's cell + 0xc7 at 1024 x 768); a cell without a
// tag keeps CLI-761's PERMANENT_UNDER_NAME
pub fn storage_mark_places(
    cat: &mut Catalog,
    r: [i32; 4],
    tag: Option<(&'static str, [i32; 2])>,
) -> StorageMarkPlaces {
    let dy = STORAGE_MARK_DY;
    let tag_at = [r[0] + TAG_X, r[1] + TAG_Y + dy];
    let period_y = match tag {
        Some((t, size)) => {
            let h = cat.tex_size(t).map_or(size[1], |(_, th)| th as i32);
            tag_at[1] + h + 1
        }
        None => r[1] + PERMANENT_UNDER_NAME,
    };
    let badge = super::tuning::EQUIPPED_MARK.1;
    StorageMarkPlaces {
        tag: tag_at,
        equipped: [r[0] + r[2] - 2 - badge[0], r[1] + 1 + dy],
        period: [r[0] + TAG_X, period_y],
    }
}

fn inside_cell(r: [i32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] as f32
        && p[1] >= r[1] as f32
        && p[0] < (r[0] + r[2]) as f32
        && p[1] < (r[1] + r[3]) as f32
}
pub const SHOP_FRAMES: [&str; 2] = [
    "UI_SCR_Page_Shop\\Inventory\\Item_Slot_B.dtx",
    "UI_SCR_Page_Shop\\Inventory\\Item_Slot_A.dtx",
];
pub const TUNING_FRAMES: [&str; 2] = [
    "UI_SCR_Page_Tuning\\Bottom\\Tunning_Shop\\Tunning_Shop_Slot_OFF.dtx",
    "UI_SCR_Page_Tuning\\Bottom\\Tunning_Shop\\Tunning_Shop_Slot_ON.dtx",
];
/// The item image of a cell (at the cell's top-left) and the price mark after the price.
// EVIDENCE: cshell+0x149f80 (item image size 0xfe x 0x56; price right end at (0xe5, 0x49) in 0xffa3c3c3, credit
// 0xfffaa677; mark GP_Mark / CP_Mark at (0xe7, 0x49), 0x16 x 0xf)
pub const CELL_IMAGE_SIZE: [i32; 2] = [0xfe, 0x56];
/// A modification cell's item image: 0x100 x 0x80, 0x2d px left of the cell.
// EVIDENCE: cshell+0x154480 (each modification / option storage cell's image offset +0x250 = -0x2d, +0x254 = 0),
// cshell+0x157c00 / cshell+0x158210 (the item image set with 0x100 x 0x80), cshell+0x2fa070 (drawn at the cell's
// place + that offset)
pub const TUNING_IMAGE_AT: [i32; 2] = [-0x2d, 0];
pub const TUNING_IMAGE_SIZE: [i32; 2] = [0x100, 0x80];
pub const MARK_GP: &str = "UI_SCR_Page_Shop\\Inventory\\GP_Mark.dtx";
pub const MARK_CREDIT: &str = "UI_SCR_Page_Shop\\Inventory\\CP_Mark.dtx";
pub const MARK_AT: [i32; 2] = [0xe7, 0x49];
pub const MARK_SIZE: [i32; 2] = [0x16, 0xf];
pub const CREDIT_PRICE_COLOUR: u32 = 0xfffa_a677;

/// The number of a price as a cell shows it (the unit is the mark image after it).
pub fn price_number(w: &Ware) -> String {
    grouped(i64::from(w.price))
}

/// The rank badge of a cell: the level icon of the cell's required experience at (8, 0x2a), 0x1e x 0x16; none for
/// level 0.
// EVIDENCE: cshell+0x149f80 (cell +0x10 through cshell+0x267f10; level < 1 -> no image, else cshell+0x2682b0 of the
// level; image at (8, 0x2a), 0x1e x 0x16 through cshell+0x2fb140)
pub const BADGE_AT: [i32; 2] = [8, 0x2a];
pub const BADGE_SIZE: [i32; 2] = [0x1e, 0x16];

/// The LOCKED tag of a cell whose experience the character lacks: under the name at (5, 0x10), 0x2b x 7, or 2 px
/// after the cell's class tag.
// EVIDENCE: cshell+0x149f80 (Item_Info_Mark_Locked through cshell+0x2faea0 at (0x1f, 0x11) 0x2b x 7, shown by
// cshell+0x34fdb0: character exp (+0x194) below cell +0x10); cshell+0x2faea0 (x = cell x + 5, or + 7 + the class
// tag's width when it shows; y = cell y + 0x10)
pub const LOCKED_TAG: &str = "UI_SCR_Page_Shop\\shop\\Item_Info_Mark_Locked.dtx";
pub const LOCKED_SIZE: [i32; 2] = [0x2b, 7];
pub const TAG_Y: i32 = 0x10;
/// The item image of a cell the character cannot buy is drawn black (a silhouette).
// EVIDENCE: cshell+0x149f80 (cshell+0x2f9e60 on the cell image: tint 0xff000000 + grey x 0x10101 unless the cell is
// sold and its +0x10 is at most the character's exp; grey from a client setting)
// DC-DEFAULT: CLI-721 the silhouette grey is 0 (OBSERVED shop-guns/cpp/: black
// silhouettes) and a listed cell counts as sold, so the silhouette goes with the LOCKED tag
pub const LOCKED_TINT: [u8; 4] = [0, 0, 0, 255];

/// The class tag of a weapon cell under its name at (5, 0x10): image and size by the item's type.
// EVIDENCE: cshell+0x25a5b0 (type 0 AR / 2 SR 0xf x 7, 1 SMG 0x16 x 7, 3 MG 0x11 x 7, 4 PISTOL 0x26 x 7, 5 / 0x1a
// SHOTGUN 0x32 x 7, others none), cshell+0x2fa1a0 (at cell x + 5, y + 0x10; hidden without an image)
pub fn class_tag(kind: i32) -> Option<(&'static str, [i32; 2])> {
    Some(match kind {
        0 => ("UI_SCR_Page_Shop\\shop\\Item_Info_Mark_AR.dtx", [0xf, 7]),
        1 => ("UI_SCR_Page_Shop\\shop\\Item_Info_Mark_SMG.dtx", [0x16, 7]),
        2 => ("UI_SCR_Page_Shop\\shop\\Item_Info_Mark_SR.dtx", [0xf, 7]),
        3 => ("UI_SCR_Page_Shop\\shop\\Item_Info_Mark_MG.dtx", [0x11, 7]),
        4 => (
            "UI_SCR_Page_Shop\\shop\\Item_Info_Mark_PISTOL.dtx",
            [0x26, 7],
        ),
        5 | 0x1a => (
            "UI_SCR_Page_Shop\\shop\\Item_Info_Mark_SHOTGUN.dtx",
            [0x32, 7],
        ),
        _ => return None,
    })
}
pub const TAG_X: i32 = 5;

/// The type tag of a non-weapon cell (gear, characters, special items) under its name: image and size by the
/// item's type, from the CShell table of 32 rows {width, height, image, red image} indexed by item type.
// EVIDENCE: cshell+0x4467c8 (data: 0xb HEAD 0x1d x 7, 0xc FACE 0x1d x 7, 0xe UNIFORM 0x2e x 7, 0xf VEST 0x1d x 7,
// 0x10 BACKPACK 0x39 x 7, 0x16 CHARACTER 0x40 x 7, 0x1d special item 0x40 x 7, 0x1e specialist 0x40 x 7; other rows
// empty), cshell+0x149f80 (the cell shows the row's first image through cshell+0x2fafd0 at cell x + 5, y + 0x10)
// DC-DEFAULT: CLI-726 the type tag takes the class tag's place and LOCKED follows it 2 px after (OBSERVED
// shop-guns/cpp/shop_bb100_body_c4_p1.png: BACKPACK at x + 5, LOCKED at x + 5 + 0x39 + 2;
// cshell+0x2fafd0 would put the tag 4 px after a shown LOCKED, not reproduced by the reference stack)
pub fn type_tag(kind: i32) -> Option<(&'static str, [i32; 2])> {
    Some(match kind {
        0xb => ("UI_SCR_Page_Shop\\shop\\Item_Info_Mark_HEAD.dtx", [0x1d, 7]),
        0xc => ("UI_SCR_Page_Shop\\shop\\Item_Info_Mark_FACE.dtx", [0x1d, 7]),
        0xe => (
            "UI_SCR_Page_Shop\\shop\\Item_Info_Mark_UNIFORM.dtx",
            [0x2e, 7],
        ),
        0xf => ("UI_SCR_Page_Shop\\shop\\Item_Info_Mark_VEST.dtx", [0x1d, 7]),
        0x10 => (
            "UI_SCR_Page_Shop\\shop\\Item_Info_Mark_BACKPACK.dtx",
            [0x39, 7],
        ),
        0x16 => (
            "UI_SCR_Page_Shop\\shop\\Item_Info_Mark_CHARACTER.dtx",
            [0x40, 7],
        ),
        0x1d => (
            "UI_SCR_Page_Shop\\shop\\Item_Info_Mark_specialitem.dtx",
            [0x40, 7],
        ),
        0x1e => (
            "UI_SCR_Page_Shop\\shop\\Item_Info_Mark_specialist.dtx",
            [0x40, 7],
        ),
        _ => return None,
    })
}

/// The tag a cell shows under its name: the weapon class tag, else the type tag.
pub fn cell_tag(kind: i32) -> Option<(&'static str, [i32; 2])> {
    class_tag(kind).or_else(|| type_tag(kind))
}

/// The Sale, Hot and New marks at the cell's top right, right to left in that order (each shown one moves the next
/// left by its width).
// EVIDENCE: cshell+0x141ef0 (Sale_Mark 0x23 x 0x12, New_Mark 0x23 x 0xf, Hot_Mark 0x22 x 0xf), cshell+0x149f80 (Sale
// shown by cell +0x1a, Hot by +0x19, New by +0x18), cshell+0x2fa430 / 0x2fa650 / 0x2fa8f0 (x = cell x + width - the
// widths of the shown marks right of it - its own width; y = cell y)
pub const SALE_MARK: (&str, [i32; 2]) =
    ("UI_SCR_Page_Shop\\Inventory\\Sale_Mark.dtx", [0x23, 0x12]);
pub const HOT_MARK: (&str, [i32; 2]) = ("UI_SCR_Page_Shop\\Inventory\\Hot_Mark.dtx", [0x22, 0xf]);
pub const NEW_MARK: (&str, [i32; 2]) = ("UI_SCR_Page_Shop\\Inventory\\New_Mark.dtx", [0x23, 0xf]);

/// The detail panel under the shop grid: the selected cell's item (frame 0x205 x 0x7f at (0x1e6, 0x20e)).
// EVIDENCE: cshell+0x149210 (the panel built by cshell+0xd46d0, placed at (0x1e6, 0x20e) and shown), cshell+0xd46d0
// (Item_Detail_Info 0x205 x 0x7f; detail image 0x100 x 0x80; name font 0xe size 0xe at (5, 3); class tag at
// (5, 0x12); description font 0x17 size 0xe in 0xffaaaaaa at (9, 0x84); the three attachment marks 0x2b x 0x12 at
// (6, 0x66), (0x36, 0x66), (0x66, 0x66) with the _D forms; the gauge list at (0x13b, 0xb)), cshell+0xd7ec0 (fill:
// name, class tag, detail image, description wrapped at 0x1fe, attachment marks for weapon groups 0 / 1 of
// cshell+0x359f00 but type 0x1d, gauges 0x14de / 0x14e3 / 0x14df / 0x14e0 / 0x3a1), cshell+0xd5f80 (a gauge: label
// font 0x17 size 0xe in 0xff999999, Guage_Bar 0x67 x 9 over 0..100, Guage_Outline 0x69 x 0xb)
pub const DETAIL_AT: [i32; 2] = [0x1e6, 0x20e];
pub const DETAIL_FRAME: (&str, [i32; 2]) = (
    "UI_SCR_Page_Shop\\Inven\\Item_Detail_Info.dtx",
    [0x205, 0x7f],
);
pub const DETAIL_IMAGE_SIZE: [i32; 2] = [0x100, 0x80];
pub const DETAIL_NAME_AT: [i32; 2] = [5, 3];
pub const DETAIL_TAG_AT: [i32; 2] = [5, 0x12];
pub const DETAIL_DESC_AT: [i32; 2] = [9, 0x84];
pub const DETAIL_DESC_WIDTH: i32 = 0x1fe;
/// The width the description wraps at in DeCombat's own text measure (unhinted advances at the whole-pixel em).
// EVIDENCE: cshell+0x2eb410 (wrap width = (0x1fe - the control's indent) x its scale), cshell+0x2edf70 (a line
// breaks at its last space once the engine font's extent of the line is past the width)
// DC-DEFAULT: CLI-724 the engine font's extent reads about 7 % wider than DeCombat's unhinted advances, so the
// description wraps at 477 of DeCombat's measure (OBSERVED shop-guns/cpp/: 20 reference line
// breaks bracket the limit to 474.4 .. 479.2 of that measure)
pub const DETAIL_DESC_WRAP: f32 = 477.0;
pub const DETAIL_DESC_FONT: i32 = 0x17;
pub const DETAIL_DESC_COLOUR: u32 = 0xffaa_aaaa;
/// The three attachment marks (muzzle, sight, clip) and their places; each in its form for the weapon row's socket
/// state: `_disable` (0), `_D` (1), `_Equip` (2).
// EVIDENCE: cshell+0xd46d0 (the _D / _disable / _A / _Equip images of each mark), cshell+0xd7ec0 (state 1 -> _D,
// 2 -> _Equip, 0 -> _disable; the _A form is not used)
pub const DETAIL_MODS: [(&str, [i32; 2]); 3] = [
    (
        "UI_SCR_Page_Tuning\\Step1_Gun_select\\Tuning_Status\\Silencer",
        [6, 0x66],
    ),
    (
        "UI_SCR_Page_Tuning\\Step1_Gun_select\\Tuning_Status\\Sight",
        [0x36, 0x66],
    ),
    (
        "UI_SCR_Page_Tuning\\Step1_Gun_select\\Tuning_Status\\Magazine",
        [0x66, 0x66],
    ),
];
/// The mark image of a socket state (`None`: a state the panel leaves unchanged).
pub fn mark_form(base: &str, state: i32) -> Option<String> {
    let form = match state {
        0 => "disable",
        1 => "D",
        2 => "Equip",
        _ => return None,
    };
    Some(format!("{base}_{form}.dtx"))
}
pub const DETAIL_MOD_SIZE: [i32; 2] = [0x2b, 0x12];
pub const GAUGE_LIST_AT: [i32; 2] = [0x13b, 0xb];
pub const GAUGE_LABELS: [i32; 5] = [0x14de, 0x14e3, 0x14df, 0x14e0, 0x3a1];
pub const GAUGE_COLOUR: u32 = 0xff99_9999;
pub const GAUGE_FILL: (&str, [i32; 2]) = ("UI_SCR_Page_Shop\\shop\\Guage_Bar.dtx", [0x67, 9]);
pub const GAUGE_OUTLINE: (&str, [i32; 2]) =
    ("UI_SCR_Page_Shop\\shop\\Guage_Outline.dtx", [0x69, 0xb]);
/// Gauge rows: 18 px apart, the outline 0x44 px right of the label, the bar 1 px inside it.
// DC-DEFAULT: CLI-723 gauge row geometry and the name colour 0xfffead00 (OBSERVED
// shop-guns/cpp/shop_bb00_weapons_c0_p1.png: labels at x 801, outlines x 869..974, rows y 537 +
// 18 n; name pixels 254, 173, 0); the row control's own layout is not traced
pub const GAUGE_PITCH: i32 = 18;
pub const GAUGE_OUTLINE_X: i32 = 0x44;
pub const DETAIL_NAME_COLOUR: u32 = 0xfffe_ad00;
/// Description line height.
// DC-DEFAULT: CLI-723 description lines 16 px apart (OBSERVED shop_bb01: lines at y 658 / 674)
pub const DETAIL_LINE: i32 = 16;

/// What the detail panel's mark stack needs besides the ware: the character lacks the experience (LOCKED), the
/// owned item has a real end date (TEMPORARY).
#[derive(Debug, Clone, Copy, Default)]
pub struct DetailMarks {
    pub locked: bool,
    pub temporary: bool,
}

/// The UTC date and time of a FILETIME as the period line shows it: `dd/MM/yyyy HH:mm:ss`.
// EVIDENCE: cshell+0xd7ec0 (FileTimeToSystemTime, then GetDateFormatW "dd/MM/yyyy " and GetTimeFormatW "HH:mm:ss"
// appended to the string 0x18e8)
pub fn filetime_text(ft: u64) -> String {
    let secs = ft / 10_000_000;
    let days = (secs / 86_400) as i64 - 134_774; // days since 1601-01-01 -> since 1970-01-01
    let tod = secs % 86_400;
    // civil date from days since 1970-01-01
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!(
        "{d:02}/{m:02}/{y} {:02}:{:02}:{:02}",
        tod / 3600,
        tod % 3600 / 60,
        tod % 60
    )
}

/// The rank icon of a locked ware in the detail panel.
pub const RANK_ICON_AT: [i32; 2] = [8, 100];
pub const RANK_ICON_SIZE: [i32; 2] = [0x1e, 0x16];

/// The period line's place: its right edge at the panel's right edge, 22 px above the panel's top.
// EVIDENCE: cshell+0xd46d0 (text +0x108: font 0x17 size 0xe, alignment 0x12 right / top, colour 0xffaaaaaa, added
// at (0x205, -0x16))
pub const EXPIRES_AT: [i32; 2] = [0x205, -0x16];

/// Draws the period line (right edge at the panel's, above it) of the panel at [`DETAIL_AT`].
pub fn draw_period_line(line: &str, out: &mut Vec<DrawItem>) {
    out.push(place_text(
        line,
        [DETAIL_AT[0] + EXPIRES_AT[0], DETAIL_AT[1] + EXPIRES_AT[1]],
        [0, 0],
        RIGHT_TOP,
        DETAIL_DESC_FONT,
        0xe,
        DETAIL_DESC_COLOUR,
    ));
}

/// The remaining-use line of an owned item: days and hours left of a real end date, else Permanent / Default Item.
// EVIDENCE: cshell+0x226a80 (end less than 10 years away: string 0x18f0 with the whole days and hours left, 0x18f1
// with the hours when under a day; else 0x18ee Permanent Item, or 0x18ef Default Item for a default item), cshell+0x2286b0
// (the text of the item's record), cshell+0xd6450 / 0xd7470 (the owned item's panel puts it in the right-aligned text
// control at (0x205, -0x16)); OBSERVED: lobby23 (reference storage: "Remaining use : Default Item")
// DC-DEFAULT: CLI-942 (proposed) a default item is a record with a default slot kind (+0x28 of table 3.3b)
pub fn remaining_use_text(cat: &Catalog, rec: &super::link::InvRecord, now: u64) -> String {
    let fmt = |id: i32, a: i64, b: i64| {
        let t = cat.text(id);
        let t = t.replacen("%ld", &a.to_string(), 1);
        t.replacen("%ld", &b.to_string(), 1)
    };
    if inbox::has_end(rec.end, now) {
        let hours = (rec.end.saturating_sub(now) / 10_000_000 / 3600) as i64;
        return if hours / 24 > 0 {
            fmt(0x18f0, hours / 24, hours % 24)
        } else {
            fmt(0x18f1, hours % 24, 0)
        };
    }
    cat.text(if rec.slot_kind == 0 { 0x18ee } else { 0x18ef })
}

/// The uses left of a counted record (string 0x18e7, "%d" the count).
// EVIDENCE: cshell+0xd6450 (string 0x18e7 with the record's count +0x14)
pub fn uses_left_text(cat: &Catalog, count: u32) -> String {
    cat.text(0x18e7).replacen("%d", &count.to_string(), 1)
}

/// The short form of [`uses_left_text`] for a storage cell ("100 uses").
// DC-DEFAULT: CLI-1879 (proposed)
pub fn uses_left_short(count: u32) -> String {
    if count == 1 {
        "1 use".to_string()
    } else {
        format!("{count} uses")
    }
}

/// The period line of an owned item in the storage detail panel. A function item in use with uses left shows them
/// (string 0x18e7); a function item not in use shows what it will give: "Duration : " (0x18f8) with its days
/// (0 = One Time Use, 999 = Permanent) or, for a counted record, "Usage : " (0x18f9) with its items; everything
/// else (and a function item in use without a count) shows [`remaining_use_text`].
// EVIDENCE: cshell+0xd6450 (the line is cshell+0x2286b0's text, then for type 0x15: not in use (+0x18 = 0x321 +0xc)
// -> the product's consume kind 2 / 3 / 6: 0x18f9 with "%d item(s)" of +0x24 (0x321 +0x30), else 0x18f8 with "One
// Time Use" (0) / "Permanent" (999) / "%d day(s)"; in use with a count +0x14 (0x321 +0x24) -> 0x18e7 with it)
// DC-DEFAULT: CLI-1879 (proposed; OWNER REPORT 2026-11-04 "the bonus EXP passes are not supposed to be permanent")
// the consume kind is the record's use kind (3 = counted; the lobby writes 3 for consume 3, 1 for consume 1), the
// amount its remaining period (+0x30) or, when that is 0, its count; an unused function item with an end date keeps
// the days / hours left; the text is drawn in the line's colour (the 2009 0xFFFFC000 run is not applied)
pub fn owned_use_text(
    cat: &Catalog,
    rec: &super::link::InvRecord,
    function: bool,
    now: u64,
) -> String {
    if !function {
        return remaining_use_text(cat, rec, now);
    }
    if rec.in_use {
        if rec.count > 0 {
            return uses_left_text(cat, rec.count);
        }
        return remaining_use_text(cat, rec, now);
    }
    if inbox::has_end(rec.end, now) {
        return remaining_use_text(cat, rec, now);
    }
    let n = if rec.period != 0 {
        rec.period
    } else {
        rec.count
    };
    let value = if rec.use_kind == 3 {
        match n {
            1 => "1 item".to_string(),
            _ => format!("{n} items"),
        }
    } else {
        match n {
            0 => "One Time Use".to_string(),
            999 => "Permanent".to_string(),
            1 => "1 day".to_string(),
            _ => format!("{n} days"),
        }
    };
    let label = cat.text(if rec.use_kind == 3 { 0x18f9 } else { 0x18f8 });
    format!("{label}{value}")
}

/// The days / hours left of a temporary owned item as a storage cell shows them.
pub fn cell_period_text(cat: &Catalog, rec: &super::link::InvRecord, now: u64) -> String {
    let t = remaining_use_text(cat, rec, now);
    // the detail panel's line starts "Remaining use : "; the cell keeps the amount
    match t.split_once(" : ") {
        Some((_, rest)) => rest.to_string(),
        None => t,
    }
}

/// The short form of the days / hours left of a dated record ("29d 16h", "5h").
pub fn cell_period_short(rec: &super::link::InvRecord, now: u64) -> String {
    let hours = (rec.end.saturating_sub(now) / 10_000_000 / 3600) as i64;
    if hours / 24 > 0 {
        format!("{}d {}h", hours / 24, hours % 24)
    } else {
        format!("{}h", hours % 24)
    }
}

/// Draws the detail panel of ware `w` (its item-table row `art`).
pub fn draw_detail(
    cat: &mut Catalog,
    w: &Ware,
    art: Option<&storage::ItemArt>,
    marks: DetailMarks,
    out: &mut Vec<DrawItem>,
) {
    let at = |p: [i32; 2]| [DETAIL_AT[0] + p[0], DETAIL_AT[1] + p[1]];
    out.push(image_fit(cat, DETAIL_FRAME.0, DETAIL_AT, DETAIL_FRAME.1));
    if let Some(t) = art
        .map(|a| a.panel_picture.as_str())
        .filter(|t| !t.is_empty() && cat.tex_size(t).is_some())
    {
        out.push(image_fit(cat, t, DETAIL_AT, DETAIL_IMAGE_SIZE));
    }
    out.push(place_text(
        &w.name,
        at(DETAIL_NAME_AT),
        [0, 0],
        ALIGN_TL,
        0xe,
        0xe,
        DETAIL_NAME_COLOUR,
    ));
    let kind = w.kind_of.unwrap_or(-1);
    let mut mark_y = DETAIL_TAG_AT[1];
    if let Some((t, size)) = w.tag_kind.and_then(cell_tag) {
        out.push(image_fit(cat, t, at(DETAIL_TAG_AT), size));
        mark_y += size[1] + 1;
    }
    // the LOCKED mark of a ware the character's rank is below, 2 px under the class tag (or in its row)
    // EVIDENCE: cshell+0xd46d0 (Item_Info_Mark_Locked 0x2b x 7, +0x148, at (5, 0x12), hidden until filled),
    // cshell+0xd7ec0 (shown when cshell+0x34fdb0 holds: the character's level (+0x194) below the record's +0x10;
    // set at the class tag's x, 2 px under a shown tag, and the stack below starts 1 px under it)
    if marks.locked {
        let ly = if w.tag_kind.and_then(cell_tag).is_some() {
            mark_y + 1
        } else {
            DETAIL_TAG_AT[1]
        };
        out.push(image_fit(
            cat,
            LOCKED_TAG,
            at([DETAIL_TAG_AT[0], ly]),
            LOCKED_SIZE,
        ));
        mark_y = ly + LOCKED_SIZE[1] + 1;
    }
    // the rank icon (30 x 22) of the experience the ware asks for, at (8, 100) of the panel, beside the LOCKED mark
    // EVIDENCE: cshell+0xd46d0 (+0x14c: a 30 x 22 image, hidden until filled), cshell+0xd7ec0 (shown with the LOCKED mark
    // when cshell+0x34fdb0 holds: the texture is the level icon of the record's experience (cshell+0x34fde0 ->
    // cshell+0x367f10 level -> cshell+0x3682b0 icon), then placed at (8, 100), or (8, 78) while the +0x128 note shows)
    // DC-DEFAULT: CLI-962 the +0x128 note is never shown here, so the icon sits at y 100
    if marks.locked {
        let icon = cat.rom.as_deref().and_then(super::lobby::PanelArt::cached);
        if let Some(t) = icon
            .as_ref()
            .and_then(|a| a.levels.icon_of_exp(w.unlock_exp, ""))
        {
            out.push(image_fit(cat, t, at(RANK_ICON_AT), RANK_ICON_SIZE));
        }
    }
    // the HiSec mark of a keyed supply case, stacked 1 px under the class tag
    // EVIDENCE: cshell+0xd46d0 (Item_Info_Mark_HiSec 0x41 x 7, +0x138, hidden until filled), cshell+0xd7ec0 (shown
    // for item type 0x14 whose record's first value (cshell+0x69e80 get(0, -1)) is not -1, placed at the stack's x
    // and 1 px under the control above)
    // DC-DEFAULT: CLI-854 x = the class tag's (5), y under the tag; with no tag the tag's own row (0x12): the stack's
    // anchor control (cshell+0x371d5c30) is not traced
    if kind == 0x14 && art.is_some_and(|a| a.keyed_case) {
        out.push(image_fit(
            cat,
            super::inbox::HISEC_MARK.0,
            at([DETAIL_TAG_AT[0], mark_y]),
            super::inbox::HISEC_MARK.1,
        ));
        mark_y += super::inbox::HISEC_MARK.1[1] + 1;
    } else if marks.temporary {
        // the TEMPORARY mark of an owned item with a real end date, in the HiSec mark's place
        // EVIDENCE: cshell+0xd7ec0 (the end FILETIME of the owned item, cshell+0x325400 = 0 for a date less than
        // 10 years away: the expiry line, and TEMPORARY (+0x13c) when the HiSec mark (+0x138) is not shown)
        let (t, size) = super::inbox::TEMPORARY_MARK;
        out.push(image_fit(cat, t, at([DETAIL_TAG_AT[0], mark_y]), size));
        mark_y += size[1] + 1;
    }
    // the PERMANENT mark of a product without a period, last in the stack
    // EVIDENCE: cshell+0xd7ec0 (+0x140 shown when the selected product's +0x20 is 0 or 999, cshell+0x3510e0)
    if w.permanent() {
        out.push(image_fit(
            cat,
            PERMANENT_MARK.0,
            at([DETAIL_TAG_AT[0], mark_y]),
            PERMANENT_MARK.1,
        ));
    }
    // attachment marks and gauges: weapon groups 0 / 1 of cshell+0x359f00, not type 0x1d
    if matches!(kind, 0..=5 | 8..=10 | 0x1a) {
        let states = art.map_or([1; 3], |a| a.socket_states);
        for ((base, p), state) in DETAIL_MODS.iter().zip(states) {
            let t = mark_form(base, state).unwrap_or_else(|| format!("{base}_D.dtx"));
            out.push(image_fit(cat, &t, at(*p), DETAIL_MOD_SIZE));
        }
        let gauges = art.map_or([0; 5], |a| a.gauges);
        for (k, (label, v)) in GAUGE_LABELS.iter().zip(gauges).enumerate() {
            let row = at([GAUGE_LIST_AT[0], GAUGE_LIST_AT[1] + k as i32 * GAUGE_PITCH]);
            out.push(place_text(
                &cat.text(*label),
                row,
                [0, GAUGE_OUTLINE.1[1]],
                LEFT_MID,
                DETAIL_DESC_FONT,
                0xe,
                GAUGE_COLOUR,
            ));
            let o = [row[0] + GAUGE_OUTLINE_X, row[1]];
            let fill = GAUGE_FILL.1[0] * v.clamp(0, 100) / 100;
            if fill > 0 {
                out.push(super::sheet::image(
                    GAUGE_FILL.0,
                    [o[0] + 1, o[1] + 1],
                    [fill, GAUGE_FILL.1[1]],
                ));
            }
            out.push(image_fit(cat, GAUGE_OUTLINE.0, o, GAUGE_OUTLINE.1));
        }
    }
    if let Some(a) = art.filter(|a| !a.functions.is_empty()) {
        draw_function_lines(cat, &a.functions, DETAIL_AT, DETAIL_FRAME.1[1], out);
    }
    let desc = art.map(|a| a.description.clone()).unwrap_or_default();
    let lines = super::roomtext::wrap_lines(&desc, DETAIL_DESC_WRAP, |s| {
        cat.measure_text(DETAIL_DESC_FONT, 0xe, s)
            .map_or(s.chars().count() as f32 * 7.0, |m| m.0)
    });
    for (k, l) in lines.iter().enumerate() {
        let p = at([
            DETAIL_DESC_AT[0],
            DETAIL_DESC_AT[1] + k as i32 * DETAIL_LINE,
        ]);
        out.push(place_text(
            l,
            p,
            [0, 0],
            ALIGN_TL,
            DETAIL_DESC_FONT,
            0xe,
            DETAIL_DESC_COLOUR,
        ));
    }
}

/// The PERMANENT mark and its place under the cell's name (the name's top 2 + its text height + 2).
// DC-DEFAULT: CLI-761 the name's text height 13 (OBSERVED shop-guns/cpp/
// shop_bb130_market_c2_p1.png: the marks of Clan Name Change / Clan Record Reset / Player Name Change 17 px under
// their cells' tops)
pub const PERMANENT_MARK: (&str, [i32; 2]) = (
    "UI_SCR_Page_Shop\\Inventory\\Permenant_Mark.dtx",
    [0x41, 10],
);
pub const PERMANENT_UNDER_NAME: i32 = 17;

/// The function lines' list (panel offset), font, base colour.
// EVIDENCE: cshell+0xd46d0 (the list at (0x13b, 0x24)), DISASM cshell+0xd8a1c .. 0xd8a8b (each line a text of font
// 0xe size 0xe in 0xffebb14d, its runs through cshell+0x2ebc10, 200 px wide), cshell+0xd7ec0 (two lines or more:
// the list centred in the panel's height)
// DC-DEFAULT: CLI-761 function lines 16 px apart (the list's row layout is not traced)
// The reference client draws "Additional Weapon/Gear Slots" in red (255, 32, 32) (shop-guns/cpp/
// shop_bb100_body_c4_p1.png); DeCombat follows the 2009 base colour 0xffebb14d.
pub const FUNCTION_LIST_AT: [i32; 2] = [0x13b, 0x24];
pub const FUNCTION_FONT: i32 = 0xe;
pub const FUNCTION_COLOUR: u32 = 0xffeb_b14d;
pub const FUNCTION_LINE: i32 = 16;
pub const FUNCTION_LINE_H: i32 = 14;

/// The colour runs of a function line: text before a `^0xAARRGGBB^` code in `base`, the code's text in its colour,
/// the rest parsed again.
// EVIDENCE: cshell+0x2ebc10 (wcsstr "^0x", wcstok "^": colour (swscanf %x), text, rest)
pub fn colour_runs(line: &str, base: u32) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(i) = rest.find("^0x") {
        if i > 0 {
            out.push((rest[..i].to_string(), base));
        }
        let after = &rest[i + 3..];
        let mut parts = after.splitn(3, '^');
        let code = parts.next().unwrap_or("");
        let text = parts.next().unwrap_or("");
        let colour = u32::from_str_radix(code.trim(), 16).unwrap_or(base);
        if !text.is_empty() {
            out.push((text.to_string(), colour));
        }
        rest = parts.next().unwrap_or("");
    }
    if !rest.is_empty() {
        out.push((rest.to_string(), base));
    }
    out
}

/// Draws the function lines of a detail panel at `panel` (its height `panel_h`).
pub fn draw_function_lines(
    cat: &mut Catalog,
    lines: &[String],
    panel: [i32; 2],
    panel_h: i32,
    out: &mut Vec<DrawItem>,
) {
    let n = lines.len() as i32;
    let top = if n < 2 {
        panel[1] + FUNCTION_LIST_AT[1]
    } else {
        let h = (n - 1) * FUNCTION_LINE + FUNCTION_LINE_H;
        panel[1] + (panel_h - h) / 2
    };
    for (k, l) in lines.iter().enumerate() {
        let mut x = panel[0] + FUNCTION_LIST_AT[0];
        let y = top + k as i32 * FUNCTION_LINE;
        for (text, colour) in colour_runs(l, FUNCTION_COLOUR) {
            out.push(place_text(
                &text,
                [x, y],
                [0, 0],
                ALIGN_TL,
                FUNCTION_FONT,
                0xe,
                colour,
            ));
            x += cat
                .measure_text(FUNCTION_FONT, 0xe, &text)
                .map_or(text.chars().count() as f32 * 7.0, |m| m.0)
                .round() as i32;
        }
    }
}

/// Whether a cell asking `unlock` experience is locked for a character with `exp`.
pub fn locked(unlock: u32, exp: u32) -> bool {
    exp < unlock
}

/// The experience the shop's rank locks compare with: a GM account's shop has every ware unlocked.
// DC-DEFAULT: LG-401 (proposed) a GM account (the login's operator flag) sees no rank lock in the shop
pub fn lock_exp(data: &MenuData) -> u32 {
    if data.gm.operator {
        u32::MAX
    } else {
        data.profile.exp
    }
}

/// What a shop cell needs of the character: the level table (badges) and the character's experience.
pub struct CellRank<'a> {
    pub levels: &'a crate::hud::board::LevelTable,
    pub exp: u32,
}

/// The badge image of a cell asking `unlock` experience (`None` at level 0 or past the icon list).
pub fn badge_of(levels: &crate::hud::board::LevelTable, unlock: u32) -> Option<&str> {
    let level = levels.level_of(unlock);
    if level < 1 {
        return None;
    }
    levels
        .small
        .get(level as usize)
        .map(String::as_str)
        .filter(|t| !t.is_empty())
}

// DC-DEFAULT: CLI-697 (proposed) cell look (frame, item image, item name, price and mark)
/// Draws a product cell: frame (`frames`: normal, selected), the item's image, its name, the price and the price
/// mark at `price_at` / `mark_at` of the cell (no `mark_at`: the price with its unit as text, as the modification
/// cells draw it).
#[allow(clippy::too_many_arguments)]
pub fn draw_ware_cell(
    cat: &mut Catalog,
    r: [i32; 4],
    w: &Ware,
    lit: bool,
    frames: [&str; 2],
    price_at: [i32; 2],
    mark_at: Option<[i32; 2]>,
    rank: Option<&CellRank>,
    out: &mut Vec<DrawItem>,
) {
    out.push(image_fit(
        cat,
        frames[lit as usize],
        [r[0], r[1]],
        [r[2], r[3]],
    ));
    let is_locked = rank.is_some_and(|k| locked(w.unlock_exp, k.exp));
    if let Some(icon) = w.icon.as_deref().filter(|i| cat.tex_size(i).is_some()) {
        let (at, size) = if mark_at.is_some() {
            (
                [r[0], r[1]],
                [CELL_IMAGE_SIZE[0].min(r[2]), CELL_IMAGE_SIZE[1].min(r[3])],
            )
        } else {
            (
                [r[0] + TUNING_IMAGE_AT[0], r[1] + TUNING_IMAGE_AT[1]],
                TUNING_IMAGE_SIZE,
            )
        };
        let mut img = image_fit(cat, icon, at, size);
        if is_locked {
            if let DrawItem::Image { tint, .. } = &mut img {
                *tint = LOCKED_TINT;
            }
        }
        out.push(img);
    }
    let tag = w.tag_kind.and_then(cell_tag);
    if let Some((t, size)) = tag {
        out.push(image_fit(cat, t, [r[0] + TAG_X, r[1] + TAG_Y], size));
    }
    if is_locked {
        let x = tag.map_or(TAG_X, |(_, size)| 7 + size[0]);
        out.push(image_fit(
            cat,
            LOCKED_TAG,
            [r[0] + x, r[1] + TAG_Y],
            LOCKED_SIZE,
        ));
    }
    // hot = list 0, new = list 1, sale = list 2; drawn sale, hot, new from the right
    let mut right = r[0] + r[2];
    for (on, (t, size)) in [
        (w.marks[2], SALE_MARK),
        (w.marks[0], HOT_MARK),
        (w.marks[1], NEW_MARK),
    ] {
        if on {
            right -= size[0];
            out.push(image_fit(cat, t, [right, r[1]], size));
        }
    }
    // PERMANENT: a product without a period, under the tag / LOCKED row, else under the name
    // EVIDENCE: cshell+0x149f80 / cshell+0x157c00 (shown when the product's period +0x20 is 0 or 999),
    // cshell+0x141ef0 / cshell+0x154480 (Permenant_Mark 0x41 x 10), cshell+0x2f97f0 (at x + 5, 1 px under a shown
    // class / type tag or LOCKED, else 2 px under the name)
    if w.permanent() {
        let y = if tag.is_some() || is_locked {
            TAG_Y + LOCKED_SIZE[1] + 1
        } else {
            PERMANENT_UNDER_NAME
        };
        out.push(image_fit(
            cat,
            PERMANENT_MARK.0,
            [r[0] + TAG_X, r[1] + y],
            PERMANENT_MARK.1,
        ));
    }
    if let Some(b) = rank.and_then(|k| badge_of(k.levels, w.unlock_exp)) {
        out.push(image_fit(
            cat,
            b,
            [r[0] + BADGE_AT[0], r[1] + BADGE_AT[1]],
            BADGE_SIZE,
        ));
    }
    out.push(place_text(
        &w.name,
        [r[0] + CELL_NAME_AT[0], r[1] + CELL_NAME_AT[1]],
        [0, 0],
        ALIGN_TL,
        CELL_FONT,
        CELL_SIZE,
        CELL_COLOUR,
    ));
    let credit = w.sale_type == 1;
    let Some(mark_at) = mark_at else {
        // the modification cells: the price with its unit as text, no mark (cshell+0x157c00)
        out.push(place_text(
            &price_text(w),
            [r[0] + price_at[0], r[1] + price_at[1]],
            [0, 0],
            RIGHT_TOP,
            CELL_FONT,
            CELL_SIZE,
            CELL_COLOUR,
        ));
        return;
    };
    out.push(place_text(
        &price_number(w),
        [r[0] + price_at[0], r[1] + price_at[1]],
        [0, 0],
        RIGHT_TOP,
        CELL_FONT,
        CELL_SIZE,
        if credit {
            CREDIT_PRICE_COLOUR
        } else {
            CELL_COLOUR
        },
    ));
    let mark = if credit { MARK_CREDIT } else { MARK_GP };
    out.push(image_fit(
        cat,
        mark,
        [r[0] + mark_at[0], r[1] + mark_at[1]],
        MARK_SIZE,
    ));
}

/// Fills the money panel's values (prepaid, credit, GP, grade name in its colour) of `layer`.
// DC-DEFAULT: CLI-552 the shop's and the tuning screen's money panels show the 0x345 balance, the
// character's GP and the grade name in the grade colour as the lobby's character info does (cshell+0x126fa0); their
// own fill functions are not traced
pub fn fill_money(
    sheet: &mut Sheet,
    layer: i32,
    ids: [i32; 4],
    data: &MenuData,
    art: Option<&PanelArt>,
    extras: &CharExtras,
) {
    sheet.set_text(layer, ids[0], &grouped(i64::from(data.balance.1)));
    sheet.set_text(layer, ids[1], &grouped(i64::from(data.balance.0)));
    sheet.set_text(layer, ids[2], &grouped(i64::from(data.profile.gp)));
    for id in &ids[..3] {
        sheet.set_colour(layer, *id, MONEY_COLOUR);
    }
    let row = art.and_then(|a| a.grade(extras.grade));
    sheet.set_text(layer, ids[3], row.map_or("", |r| r.name.as_str()));
    sheet.set_colour(layer, ids[3], row.map_or(0xffff_ffff, |r| r.colour));
}

/// The colour of a page number past the last page (its button shows its disabled look).
// OBSERVED: shop-guns/cpp/ (pager of a two-page category: 3, 4, 5 on disabled buttons in
// 0x505050, 2 in 0xa3a3a3, the current page in 0xfead00)
pub const PAGE_OFF_COLOUR: u32 = 0xff50_5050;

/// The first page of the group that holds `page`.
pub fn group_base(page: usize) -> usize {
    page / PAGE_GROUP * PAGE_GROUP
}

/// The page the group arrows go to: back = the first page of the previous group, next = the first page of the next
/// group (`None` at the first / last group).
// OBSERVED: shop-guns/cpp/ (an eight-page category: the next arrow on page 1 shows page 6 with
// the buttons 6..10, the back arrow lit; on the last group the next arrow is disabled and stays)
// DC-DEFAULT: CLI-697 (proposed) the arrows step by page groups
pub fn group_step(page: usize, pages: usize, forward: bool) -> Option<usize> {
    let base = group_base(page);
    if forward {
        Some(base + PAGE_GROUP).filter(|b| *b < pages)
    } else {
        base.checked_sub(PAGE_GROUP)
    }
}

/// Enables the page number buttons `ids` of the group that holds `page` up to the last of `pages`, and the group
/// arrows (commands `CMD_PAGE_BACK` / `CMD_PAGE_FWD`) when a group lies that way.
pub fn sync_pager(sheet: &mut Sheet, ids: &[i32], page: usize, pages: usize) {
    let base = group_base(page);
    for (k, id) in ids.iter().enumerate() {
        if let Some(b) = sheet.part_mut(-1, *id, WidgetKind::Button) {
            b.enabled = base + k < pages;
        }
    }
    let back = group_step(page, pages, false).is_some();
    let fwd = group_step(page, pages, true).is_some();
    for b in sheet.parts.iter_mut() {
        if b.kind == WidgetKind::Button && b.command == CMD_PAGE_BACK {
            b.enabled = back;
        }
        if b.kind == WidgetKind::Button && b.command == CMD_PAGE_FWD {
            b.enabled = fwd;
        }
    }
}

/// Draws the page number buttons (`ids`) of the group that holds `page` of `pages`: every button of the group gets
/// its number, those past the last page in [`PAGE_OFF_COLOUR`].
pub fn draw_page_numbers(
    sheet: &Sheet,
    ids: &[i32],
    page: usize,
    pages: usize,
    out: &mut Vec<DrawItem>,
) {
    let base = group_base(page);
    for (k, id) in ids.iter().enumerate() {
        let n = base + k;
        if let Some(b) = sheet.part(-1, *id, WidgetKind::Button) {
            let c = if n >= pages {
                PAGE_OFF_COLOUR
            } else if n == page {
                OWN_COLOUR
            } else if n < pages {
                ROW_COLOUR
            } else {
                PAGE_OFF_COLOUR
            };
            out.push(place_text(
                &(n + 1).to_string(),
                b.pos,
                b.size,
                CENTRE,
                14,
                13,
                c,
            ));
        }
    }
}

/// The page number under `p` among the buttons `ids` (absolute page).
pub fn page_hit(
    sheet: &Sheet,
    ids: &[i32],
    page: usize,
    pages: usize,
    p: [f32; 2],
) -> Option<usize> {
    let base = page / PAGE_GROUP * PAGE_GROUP;
    ids.iter().enumerate().find_map(|(k, id)| {
        let b = sheet.part(-1, *id, WidgetKind::Button)?;
        (base + k < pages && b.contains(p)).then_some(base + k)
    })
}

/// The number of pages of `n` entries.
pub fn page_count(n: usize) -> usize {
    n.div_ceil(CELLS).max(1)
}

/// A yes / no box asking to buy `w`.
// DC-DEFAULT: CLI-548 Buy asks in the generic yes / no box (`<name>  <price>`) and sends 0x328 with
// option 0 and no unique id to extend; the 2009 purchase dialogs (period choice, credit / prepaid, windows 6 / 7 of
// the page, cshell+0x145b60) are not built
pub fn ask_buy(w: &Ware) -> Popup {
    Popup::ask(
        format!("{}  {}", w.name, price_text(w)),
        BoxTag::Buy(w.product),
    )
}

/// The answer to a purchase: the box string (0x439 bought, 0x46e short of GP, 0x1873 short of credit, 0x503
/// refused).
// SPEC: PROTOCOL_2009.md §4.2 (0x329: reason 0x3b not enough money -> GP box res 0x46e / credit box res 0x1873; 0x3c
// -> res 0x503; a new unique id -> res 0x439, a held one -> res 0x438)
pub fn bought_text(ok: bool, reason: u16, credit: bool) -> i32 {
    match (ok, reason) {
        (true, _) => 0x439,
        (false, 0x3b) if credit => 0x1873,
        (false, 0x3b) => 0x46e,
        _ => 0x503,
    }
}

/// The box of a refused purchase on the shop page: short of money (GP 0x46e titled 0x346, credit the short-of-NX
/// box with Charge NX), inventory full 0x503, any other reason the shared reason table (0x60 not for sale 0x17f9 ...).
// EVIDENCE: cshell+0x150c40 (0x3b: credit product (+0x18 != 0) -> 0x1873 titled 0x17e5 with the charge button, else
// 0x46e; 0x3c -> 0x503; any other reason -> cshell+0x393f0)
// DC-DEFAULT: CLI-1431 (proposed) the refusals follow the 2009 page (before: every other reason showed 0x503)
pub fn refused_box(cat: &Catalog, reason: u16, credit: bool) -> Popup {
    match reason {
        0x3b if credit => buybox::short_nx_box(cat),
        0x3b | 0x3c => {
            let mut q = Popup::ok(cat.text(bought_text(false, reason, credit)), BoxTag::None);
            q.title = text_ids::TITLE_ERROR;
            q
        }
        r => usebox::reason_box(cat, u32::from(r)),
    }
}

/// The purchase box's title, the equip question after it with its title / first button and its second button.
// EVIDENCE: cshell+0x150c40 (title 0x15f5), cshell+0x150484 (0x36b, 0x160e, 0x36a)
pub const BOUGHT_TITLE: i32 = 0x15f5;
/// audit-function: the title of the credit purchase's "placed in your inbox" box (CLI-1388).
// EVIDENCE: cshell+0x150c40 (box 0x17b6 titled 0x17d1)
pub const CREDIT_BOUGHT_TITLE: i32 = 0x17d1;
pub const EQUIP_ASK: i32 = 0x36b;
pub const EQUIP_TITLE: i32 = 0x160e;
pub const KEEP_TEXT: i32 = 0x36a;

/// The equip / unequip refusals.
// SPEC: PROTOCOL_2009.md §4.2 (0x325 ok 0 -> res 0x506; 0x327 ok 0 -> res 0x4dd)
pub fn equip_refused_text(unequip: bool) -> i32 {
    if unequip {
        0x4dd
    } else {
        0x506
    }
}

/// The shop screen.
pub struct ShopScreen {
    sheet: Sheet,
    wares: Arc<Wares>,
    /// shop-model: the model tables the pane's character is built from (`None` without a ROM).
    figure: Option<Arc<super::figure::FigureTables>>,
    art: Option<PanelArt>,
    pub tab: usize,
    pub pane: Pane,
    pub category: usize,
    pub page: usize,
    /// The selected cell of the current page (shop and storage) or inbox row.
    pub selected: Option<usize>,
    shop_cells: Vec<[i32; 4]>,
    store_cells: Vec<[i32; 4]>,
    /// The page the screen opened on (shop, black market, inbox).
    entry: Go,
    /// The last product asked for in the buy box.
    buying: Option<Ware>,
    /// The payment option (0x328 +0xa: 0 = the credit balance, else prepaid) and the price of the product the
    /// purchase in flight asked for (the window's picked row, not the cell's product), for the balance the client
    /// deducts itself on the reply (CLI-552 amended, CLI-1875).
    buying_charge: (u32, u32),
    /// lobby-3: the 2009 purchase window over the shop, its textures, and a purchase request in flight (+0x5550).
    pub buy_box: Option<BuyBox>,
    buy_art: BuyArt,
    pub buy_busy: bool,
    /// name-change: the open rename window (CLI-1365), its textures, and the item whose use waits for its answer.
    pub name_box: Option<usebox::NameBox>,
    name_art: usebox::NameArt,
    using: Option<Using>,
    /// use-windows: the open record reset window (CLI-1371), the use a question box waits to confirm (weapon change
    /// 0x1876, the record window's check box) and the weapon change boxes' slot words (CLI-1370).
    pub record_box: Option<recordbox::RecordBox>,
    confirming: Option<Using>,
    slot_words: [String; 4],
    /// use-items: the open country window (CLI-1873) and megaphone composer (CLI-1874) and their textures.
    pub country_box: Option<countrybox::CountryBox>,
    country_art: countrybox::CountryArt,
    pub mega_box: Option<megabox::MegaBox>,
    mega_art: megabox::MegaArt,
    /// use-items-sweep: the open colour window (CLI-1876), its textures and the ROM colour tables.
    pub colour_box: Option<colorbox::ColourBox>,
    colour_art: colorbox::ColourArt,
    colour_tables: colorbox::ColourTables,
    /// use-items-emblem: the open clan mark window (CLI-1884) and face window (CLI-1885), its textures.
    pub mark_box: Option<markbox::MarkBox>,
    pub face_box: Option<facebox::FaceBox>,
    face_art: facebox::FaceArt,
    /// shop-model: the last press on a shop cell (page cell, time) and the item a double click previews on the
    /// pane's character (equipment slot, item number) until the page changes.
    cell_pressed: Option<(usize, std::time::Instant)>,
    /// CLI-1263: the pane character's yaw (radians) and the drag that turns it (the pointer's last x, virtual
    /// pixels, and the horizontal screen scale at the press).
    pub figure_yaw: f32,
    figure_drag: Option<(f32, f32)>,
    pub preview: Option<(usize, i32)>,
    /// storage-functions: the unique id a discard request (0x334) is out for.
    discarding: Option<u32>,
    /// round 10: the inbox entry being opened, until the answer 0x32f.
    opening: Option<Opening>,
    /// CLI-1345: the unique id of an item an inbox opening added, selected on the storage page once the inventory
    /// reread holds it.
    goto_uid: Option<u32>,
    /// storage-functions: the open slot choice window (6 backpack, 7 equip slot).
    pub slot_pick: Option<SlotPick>,
    /// CLI-971 / CLI-972: the left pane's rows under the pointer and the backpack rows' drawers.
    pub look: pane::PaneLook,
    /// CLI-972: the backpack row a drawer button opened the storage page for (+0x55a8; `None` = -1): Equip then
    /// puts the item into that backpack slot.
    pub pack_target: Option<usize>,
    /// CLI-1003: the storage page is the Character page (shelf Black Market / 3 under the Body tab).
    pub character_page: bool,
    /// The storage page's Discard / Mod / Backpack buttons' layout places (cshell+0x140590 moves them).
    store_homes: [[i32; 2]; 3],
    /// The pointer of the last hover (an opened drawer lights the button under it) and the backpack slot rows then.
    pointer: Option<[f32; 2]>,
    pack_count: usize,
    /// CLI-1628: the x-ray controls next to the nickname (the open palette).
    pub xray_controls: super::shopxray::XrayControls,
    /// function-unequip: the layout's caption of the storage Equip button (restored off the function-item shelf).
    wear_caption: i32,
}

impl ShopScreen {
    pub fn new(cat: &Catalog, data: &MenuData, entry: Go) -> ShopScreen {
        let mut sheet = build_sheet(cat, &[pages::LOBBY_COMMON, pages::SHOP]);
        for w in DIALOG_WINDOWS {
            sheet.show_window(w, false);
        }
        // OBSERVED: lobbyfx4/real/R32_now.png (nothing drawn at the left pane's image 0, x 196 y 614)
        // DC-DEFAULT: CLI-546 the left pane's image 0 stays hidden (its 2009 trigger is not traced)
        sheet.show(LEFT_WINDOW, 0, WidgetKind::Image, false);
        let mut s = ShopScreen {
            sheet,
            wares: Wares::cached(cat.rom.as_deref()),
            figure: cat.rom.as_deref().map(super::figure::FigureTables::cached),
            art: cat.rom.as_deref().and_then(PanelArt::cached),
            tab: storage::TAB_WEAPONS,
            pane: Pane::Shop,
            category: 0,
            page: 0,
            selected: None,
            shop_cells: grid_cells(cat, pages::SHOP, SHOP_WINDOW, SHOP_GRID),
            store_cells: grid_cells(cat, pages::SHOP, STORAGE_WINDOW, STORAGE_GRID),
            entry,
            buying: None,
            buying_charge: (0, 0),
            buy_box: None,
            buy_art: BuyArt::from_rom(cat.rom.as_deref()),
            buy_busy: false,
            name_box: None,
            name_art: usebox::NameArt::from_rom(cat.rom.as_deref()),
            using: None,
            record_box: None,
            confirming: None,
            slot_words: usebox::slot_words(cat.rom.as_deref()),
            country_box: None,
            country_art: countrybox::CountryArt::from_rom(cat.rom.as_deref()),
            mega_box: None,
            mega_art: megabox::MegaArt::from_rom(cat.rom.as_deref()),
            colour_box: None,
            colour_art: colorbox::ColourArt::from_rom(cat.rom.as_deref()),
            colour_tables: colorbox::ColourTables::from_rom(cat.rom.as_deref()),
            mark_box: None,
            face_box: None,
            face_art: facebox::FaceArt::from_rom(cat.rom.as_deref()),
            cell_pressed: None,
            figure_yaw: super::modelview::PLACE.yaw,
            figure_drag: None,
            preview: None,
            discarding: None,
            opening: None,
            goto_uid: None,
            slot_pick: None,
            look: pane::PaneLook::default(),
            pack_target: None,
            character_page: false,
            store_homes: [[0; 2]; 3],
            pointer: None,
            xray_controls: Default::default(),
            pack_count: 0,
            wear_caption: WEAR_CAPTION,
        };
        if let Some(c) = s.sheet.parts.iter().find_map(|p| match &p.look {
            Look::Button {
                caption: Some(c), ..
            } if p.layer == STORAGE_WINDOW && p.command == CMD_WEAR => Some(c.string_id),
            _ => None,
        }) {
            s.wear_caption = c;
        }
        for (k, h) in s.store_homes.iter_mut().enumerate() {
            if let Some(b) = s.sheet.part(STORAGE_WINDOW, k as i32, WidgetKind::Button) {
                *h = b.pos;
            }
        }
        s.show_pane();
        s.refresh(data);
        s.select_first(data);
        s
    }

    /// The shop page selects its first cell whenever nothing is selected and the page has a cell.
    // OBSERVED: shop-guns/cpp/ (every reference page shows its first cell lit and in the detail panel)
    // DC-DEFAULT: CLI-723 the first cell of a shown shop page is selected (the 2009 selection code is not
    // traced)
    fn select_first(&mut self, data: &MenuData) {
        // EVIDENCE: cshell+0x140d80 (the inbox page selects its first row when it holds an entry)
        if self.pane == Pane::Inbox
            && self.selected.is_none()
            && !inbox::rows_on(data, self.page).is_empty()
        {
            self.selected = Some(0);
        }
        // CLI-1265 the storage page too selects its first cell when the page holds an item
        // OBSERVED: storage-cells-r14/cpp/st1024/shop_bb00_storage.png (reference client: the storage
        // page opened from the pistol row shows its first cell lit and its detail panel)
        if matches!(self.pane, Pane::Shop | Pane::Storage)
            && self.selected.is_none()
            && self.page * CELLS < self.entries(data)
        {
            self.selected = Some(0);
        }
        self.sync_gift(data);
        self.sync_wear(data);
    }

    /// function-unequip: the storage Equip button on the function-item shelf reads Equip / Unequip for an item in use
    /// (by the function list) and is enabled only when its row has `IsCanUnEquip`; an item not in use reads 0x1640.
    /// Other shelves keep the layout's caption (CLI-1396).
    // EVIDENCE: cshell+0x147470 (see [`WEAR_CAPTION`])
    // DC-DEFAULT: CLI-1396 (proposed) only the function-item shelf's caption follows the selection
    fn sync_wear(&mut self, data: &MenuData) {
        let function_shelf = self.pane == Pane::Storage
            && self.tab == TAB_MARKET
            && self.category == FUNCTION_CATEGORY;
        let sel = self
            .selected
            .and_then(|i| self.items_shown(data).get(self.page * CELLS + i).cloned())
            .filter(|_| function_shelf);
        let (caption, enabled) = match sel {
            Some(r) if self.wares.kind_of(r.item_no as i32) == Some(usebox::FUNCTION_TYPE) => {
                let sub = self
                    .wares
                    .art_of(r.item_no as i32)
                    .map_or(-1, |a| a.sub_type);
                if r.in_use && sub != usebox::SUB_MEGAPHONE {
                    let on = self.wares.in_function_list(&data.store, &r);
                    (
                        if on { UNWEAR_CAPTION } else { WEAR_CAPTION },
                        self.wares.can_unequip(r.item_no as i32),
                    )
                } else if self.wares.in_function_list(&data.store, &r) {
                    (UNWEAR_CAPTION, true)
                } else {
                    (FUNCTION_IDLE_CAPTION, true)
                }
            }
            _ => (self.wear_caption, true),
        };
        for p in self.sheet.parts.iter_mut() {
            if p.layer == STORAGE_WINDOW && p.kind == WidgetKind::Button && p.command == CMD_WEAR {
                p.enabled = enabled;
                if let Look::Button {
                    caption: Some(c), ..
                } = &mut p.look
                {
                    c.string_id = caption;
                }
            }
        }
    }

    /// Gift (shop window button 1) is enabled only while the selected cell's product is a credit product.
    // EVIDENCE: cshell+0x13f9f0 (a cell selection disables button (3, 1), then enables it and Buy (3, 2) when the
    // product of the cell (+0x24c, through cshell+0x250110) has +0x18 = 1), cshell+0x257bc0 (product +0x18 =
    // `SaleType`)
    fn sync_gift(&mut self, data: &MenuData) {
        let credit = self.pane == Pane::Shop
            && self
                .selected
                .and_then(|i| self.wares_shown(data).get(self.page * CELLS + i).cloned())
                .is_some_and(|w| w.sale_type == 1);
        if let Some(b) = self
            .sheet
            .part_mut(SHOP_WINDOW, GIFT_ID, WidgetKind::Button)
        {
            b.enabled = credit;
        }
    }

    /// The products of the current shop page's shelf.
    pub fn wares_shown(&self, data: &MenuData) -> Vec<Ware> {
        self.wares.shelf(&data.store, self.tab, self.category)
    }

    /// The items of the current storage page's shelf.
    pub fn items_shown(&self, data: &MenuData) -> Vec<super::link::InvRecord> {
        if self.character_page {
            return data
                .store
                .items
                .iter()
                .filter(|r| self.wares.is_person_item(r.item_no as i32))
                .cloned()
                .collect();
        }
        stored(&data.store, &self.wares, self.tab, self.category)
    }

    /// The pager's enabled buttons for the current page and entry count.
    fn sync_pages(&mut self, data: &MenuData) {
        let pages = self.pages(data);
        sync_pager(&mut self.sheet, &PAGE_IDS, self.page, pages);
    }

    /// Pages of the current page's list.
    fn pages(&self, data: &MenuData) -> usize {
        match self.pane {
            Pane::Inbox => inbox::page_count(data),
            _ => page_count(self.entries(data)),
        }
    }

    fn entries(&self, data: &MenuData) -> usize {
        match self.pane {
            Pane::Shop => self.wares_shown(data).len(),
            Pane::Storage => self.items_shown(data).len(),
            Pane::Inbox => data.store.inbox.len(),
        }
    }

    /// Shows the windows and buttons of the current page.
    // DISASM: cshell+0x14b8a0, cshell+0x14b720 (see the module head)
    fn show_pane(&mut self) {
        // shop-model: a preview ends with the page (cshell+0x14b8a0 runs cshell+0x1453d0, the look back to the
        // equipped set)
        self.preview = None;
        let s = &mut self.sheet;
        for id in FIRST_CATEGORY_ID[0]..=EXIT_ID {
            s.show(-1, id, WidgetKind::Button, false);
        }
        s.show_window(SHOP_WINDOW, self.pane == Pane::Shop);
        s.show_window(STORAGE_WINDOW, self.pane == Pane::Storage);
        s.show_window(INBOX_WINDOW, self.pane == Pane::Inbox);
        if self.pane != Pane::Storage {
            s.show(-1, EXIT_ID, WidgetKind::Button, true);
        }
        if self.pane == Pane::Shop && self.tab < CATEGORY_COMMANDS.len() {
            let first = FIRST_CATEGORY_ID[self.tab];
            for k in 0..CATEGORY_COMMANDS[self.tab].len() as i32 {
                s.show(-1, first + k, WidgetKind::Button, true);
            }
        }
        // the page switch hides controls 0x10 .. 0x20 only: the pager stays on every page
        // EVIDENCE: cshell+0x14b8a0, cshell+0x1482b0 (inbox page: the pager's buttons enabled up to the inbox total)
        for id in PAGING_IDS.iter().chain(PAGE_IDS.iter()) {
            s.show(-1, *id, WidgetKind::Button, true);
        }
        // the shop page window's image of the tab
        let market = self.tab == TAB_MARKET;
        if let Some(p) = s.parts.iter_mut().rev().find(|p| {
            p.kind == WidgetKind::Window && p.layer == SHOP_WINDOW && p.page == pages::SHOP
        }) {
            if let Look::Panel { tex, .. } = &mut p.look {
                *tex = pane::shop_window_image(market).to_string();
            }
        }
        // the current tab button is selected, the others not
        // EVIDENCE: cshell+0x13fd20 (controls 0xc .. 0xf: vtable +0x88 on all, +0x84 on the tab's)
        for (k, id) in TAB_IDS.iter().enumerate() {
            if let Some(p) = s.part_mut(-1, *id, WidgetKind::Button) {
                p.selected = k == self.tab;
            }
        }
        // the left pane's equipment lists: weapons or body
        let (w, b) = (self.tab == storage::TAB_WEAPONS, self.tab == TAB_BODY);
        s.show(LEFT_WINDOW, WEAPON_LIST, WidgetKind::List, w);
        s.show(LEFT_WINDOW, BODY_LIST, WidgetKind::List, b);
        if self.pane == Pane::Storage {
            self.layout_storage_buttons();
        }
    }

    /// CLI-1628: the x-ray controls' rectangles, right of the nickname box (none without its layout record).
    pub fn xray_rects(&self, data: &MenuData) -> Option<super::shopxray::XrayRects> {
        if !data.xray_offered {
            return None;
        }
        let b = self.sheet.button(NICK_BUTTON_CMD)?;
        Some(super::shopxray::XrayRects::of_nick(b.pos, b.size))
    }

    /// CLI-1628: a press on the x-ray controls; a changed style is the player's setting at once (published for the
    /// preview and the next match) and saved in the profile.
    fn xray_press(&mut self, p: [f32; 2], ctx: &mut Ctx) -> bool {
        let Some(r) = self.xray_rects(ctx.data) else {
            return false;
        };
        let mut style = ctx.data.settings.xray;
        match self.xray_controls.press(&r, p, &mut style) {
            super::shopxray::XrayPress::Miss => false,
            super::shopxray::XrayPress::Taken => true,
            super::shopxray::XrayPress::Changed => {
                ctx.data.settings.xray = style;
                ctx.data.settings_changed = true;
                crate::settings::publish(&ctx.data.settings);
                super::store_settings(ctx.data, ctx.cat);
                true
            }
        }
    }

    /// The money panel, the nickname and the rank icon.
    pub fn refresh(&mut self, data: &MenuData) {
        fill_money(
            &mut self.sheet,
            MONEY_WINDOW,
            MONEY_TEXTS,
            data,
            self.art.as_ref(),
            &data.char_extras,
        );
        if let Some(icon) = self
            .art
            .as_ref()
            .and_then(|a| a.levels.icon_of_exp(data.profile.exp, &data.profile.nick))
        {
            if let Some(p) = self.sheet.part_mut(-1, RANK_IMAGE_ID, WidgetKind::Image) {
                if let Look::Panel { tex, .. } = &mut p.look {
                    *tex = icon.to_string();
                }
            }
        }
    }

    /// Left tab `tab` with the shop page on its first category.
    // DISASM: cshell+0x14ff3f..0x14fffe (1000 / 1001 / 1002: +0x5564 = tab, +0x5568 = 0, page shop)
    pub fn set_tab(&mut self, tab: usize) {
        self.tab = tab;
        self.pane = Pane::Shop;
        self.look = pane::PaneLook::default();
        self.pack_target = None;
        self.character_page = false;
        self.category = 0;
        self.page = 0;
        self.selected = None;
        self.show_pane();
    }

    /// The inbox page's Activate: the key check of a HiSec case, then the confirmation that sends 0x32e.
    // EVIDENCE: HISEC_2009.md 4.2 (cshell+0x14cc00 key check; no key: the key-required box and nothing is sent; a key:
    // the opening box; other rows the generic question cshell+0x148ac0)
    // EVIDENCE: cshell+0x148e30 (CLI-960: the boxes are window 10: case and key images, the text, the keys-left line;
    // [`super::popup::HisecBox`])
    fn activate_selected(&mut self, ctx: &mut Ctx) {
        if self.opening.is_some() {
            return;
        }
        let Some(row) = self
            .selected
            .and_then(|i| inbox::rows_on(ctx.data, self.page).get(i))
            .cloned()
        else {
            return;
        };
        let Some(w) = self.wares.ware(row.product, &ctx.data.store.prices) else {
            return;
        };
        let tag = BoxTag::OpenGift {
            entry: row.entry,
            product: row.product,
        };
        let keyed =
            w.kind_of == Some(0x14) && self.wares.art_of(w.item_no).is_some_and(|a| a.keyed_case);
        if keyed {
            let key = self.wares.key_of(w.item_no);
            let owned = key.is_some_and(|k| {
                ctx.data
                    .store
                    .items
                    .iter()
                    .any(|i| i.item_no as i32 == k && i.count > 0)
            });
            let key_name = key.map_or(String::new(), |k| self.wares.label_of(k));
            let held = key.map_or(0, |k| {
                ctx.data
                    .store
                    .items
                    .iter()
                    .find(|i| i.item_no as i32 == k && i.count > 0)
                    .map_or(0, |i| i.count as u32)
            });
            let icon = |n: Option<i32>| {
                n.and_then(|n| self.wares.art_of(n))
                    .map_or(String::new(), |a| a.icon.clone())
            };
            let hisec = super::popup::HisecBox::build(
                ctx.cat,
                &icon(Some(w.item_no)),
                &icon(key),
                owned,
                held,
            );
            if !owned {
                let t = buybox::fill_form(&ctx.cat.text(HISEC_NEEDS_KEY), &[&key_name, &w.name]);
                let mut p = Popup::ok(t, BoxTag::None);
                p.hisec = hisec;
                ctx.popups.push(p);
            } else {
                let t = buybox::fill_form(&ctx.cat.text(HISEC_OPENING), &[&key_name]);
                let mut p = Popup::ask(t, tag);
                p.hisec = hisec;
                ctx.popups.push(p);
            }
            return;
        }
        let form = match w.kind_of {
            Some(0x15) => 0x17b7,
            Some(0x11..=0x13) => 0x1825,
            _ => 0x17b8,
        };
        let t = buybox::fill_form(&ctx.cat.text(form), &[&w.name]);
        let mut q = Popup::ask(t, tag);
        q.title = OPEN_TITLE;
        q.captions = Some([0x35, 0x36]);
        ctx.popups.push(q);
    }

    /// The inbox page; the inbox is asked for when it was never fetched or is stale.
    // DISASM: cshell+0x1500f6..0x150187 (1003: the shell's inbox flags +0x1a / +0x18 clear -> the held rows, else the
    // request, then +0x1a cleared)
    // EVIDENCE: cshell+0x14b8a0 (page 2: the left pane on tab 3, cshell+0x14b720 / 0x13fd20, its function item list
    // cshell+0x149a20)
    pub fn open_inbox(&mut self, ctx: &mut Ctx) {
        self.pane = Pane::Inbox;
        self.tab = GIFTS_TAB;
        self.selected = None;
        self.page = 0;
        self.show_pane();
        let st = &mut ctx.data.store;
        if !st.inbox_fetched || st.inbox_stale {
            st.inbox.clear();
            st.inbox_fetched = false;
            st.inbox_stale = false;
            ctx.send(LinkCmd::InboxPage(1));
        }
    }

    /// shop-model: a second press on the same shop cell within the double-click time previews the cell's item on the
    /// pane's character (its equipment slot in the look replaced, nothing saved).
    // EVIDENCE: cshell+0x14edb0 (shop page: the cell's item id / type go to cshell+0x14e410 with preview 1 after the
    // class check cshell+0x34fd10; a credit product then opens the buy box, command 0x10a)
    // DC-DEFAULT: CLI-749 (proposed) the double click is the one of CLI-169; the class check and the buy box of a
    // credit product are not ported
    fn preview_on_double(&mut self, cell: usize, data: &MenuData) {
        let now = std::time::Instant::now();
        let double = self.pane == Pane::Shop
            && self
                .cell_pressed
                .is_some_and(|(c, t)| c == cell && t.elapsed().as_millis() < DOUBLE_CLICK_MS);
        self.cell_pressed = Some((cell, now));
        if !double {
            return;
        }
        let Some(w) = self
            .wares_shown(data)
            .get(self.page * CELLS + cell)
            .cloned()
        else {
            return;
        };
        if let Some(slot) = own_slot(&self.wares, w.item_no.max(0) as u32) {
            self.preview = Some((slot as usize, w.item_no));
        }
    }

    /// Asks for the inbox page shown when its rows are not held yet.
    // EVIDENCE: cshell+0x148980 (a page whose rows are not held is asked for, the rows shown when they are)
    fn ask_inbox_page(&mut self, ctx: &mut Ctx) {
        if self.pane != Pane::Inbox {
            return;
        }
        let st = &ctx.data.store;
        let need = (self.page + 1) * inbox::ROWS;
        if st.inbox_fetched && st.inbox.len() < need && st.inbox.len() < st.inbox_total as usize {
            if let Some(n) = st.next_inbox_page() {
                ctx.send(LinkCmd::InboxPage(n));
            }
        }
    }

    /// CLI-1345 the storage page holding item `item_no`: its tab and category (characters on the body tab's
    /// character page); an item without a storage shelf (options, untyped) leaves the screen as it is.
    // EVIDENCE: cshell+0x14c7e0 (the item type picks the tab and category; types outside the three tabs go to the
    // modification screen: CLI-1430, handled by the inbox reply before this)
    fn goto_storage_of(&mut self, item_no: i32) {
        let kind = self.wares.kind_of(item_no);
        if kind == Some(0x16) {
            self.set_tab(TAB_BODY);
            self.open_character_storage();
            return;
        }
        match kind.and_then(storage::shelf_of) {
            Some((tab, cat)) if tab <= TAB_MARKET => {
                self.set_tab(tab);
                self.open_storage(cat);
            }
            _ => {}
        }
    }

    /// The storage page on category `cat` of the current tab.
    // DC-DEFAULT: CLI-546 the storage page also opens from the left pane's equipment rows (the 2009 commands 10000 ..
    // 10002 come from the backpack rows' drawers, CLI-972); CLI-972 the backpack row of a drawer is forgotten (+0x55a8 =
    // -1) when the storage page opens any other way
    pub fn open_storage(&mut self, cat: usize) {
        self.pane = Pane::Storage;
        self.character_page = false;
        self.category = cat;
        self.page = 0;
        self.selected = None;
        self.pack_target = None;
        self.show_pane();
    }

    /// CLI-1003 the Character row of the body list: the storage page of the characters (item type 0x16: shelf
    /// Black Market, category 3), shown under the Body tab.
    // EVIDENCE: cshell+0x14a700 (page of tab 2 / category 3 lights the Body tab through cshell+0x13fd20(1)), cshell+0x149d20
    // (the body list's first row is slot 0xe of item type 0x16, string 0x5b6)
    pub fn open_character_storage(&mut self) {
        self.open_storage(0);
        self.character_page = true;
        self.show_pane();
    }

    /// CLI-972 a drawer button of backpack row `row`: the storage page of the button's category for that row.
    // DISASM: cshell+0x1509ba..0x150acf (10000 / 0x2711 / 0x2712: +0x55a8 = the row, cshell+0x142d70, the storage
    // page, category 0 / 1 / 3 through cshell+0x14a700, the storage buttons laid out by cshell+0x140590)
    pub fn open_drawer(&mut self, button: usize, row: usize) {
        self.open_storage(pane::SLIDE_CATEGORIES[button.min(2)]);
        self.pack_target = Some(row);
        self.layout_storage_buttons();
    }

    /// The storage page's Discard / Mod / Backpack buttons by tab, category and drawer row.
    // DISASM: cshell+0x140590 (window 4's controls 0 / 1 / 2 = Discard, Mod, Backpack; weapons tab, category 2 or 3:
    // no drawer row: Discard at Mod's place, Mod hidden, Backpack shown; a drawer row: Discard at Backpack's place, Mod
    // and Backpack hidden; other weapons categories: no drawer row: Discard and Mod at their places, both shown with
    // Backpack; a drawer row: Discard at Mod's place, Mod at Backpack's place, Backpack hidden; body and black market
    // tabs: Discard at Backpack's place, Mod and Backpack hidden), cshell+0x14b8a0 (the storage page runs it)
    fn layout_storage_buttons(&mut self) {
        // CLI-1001: the top tab strip. General (control 5) is the page's mode 0 and is selected; Special Item (7) and
        // Free Pass (6) are enabled only when the character holds such items
        // EVIDENCE: cshell+0x13fe00 (the page mode 0 / 1 / 2 selects control 5 / 7 / 6, clears the other two),
        // cshell+0x140fe0 (control 6 enabled on the weapons tab when category 5 holds a record), cshell+0x141060
        // (control 7 enabled when the special list of the page holds one); OBSERVED
        // storage-functions/pairs/item1/storage.png (the reference stack draws General alone, selected)
        // DC-DEFAULT: CLI-1001 no special or free-pass inventory is kept yet (weapons category 5 is empty, the special
        // list is not on the wire as a separate list), so both tabs stay hidden, as a disabled button draws nothing
        if let Some(b) = self.sheet.part_mut(STORAGE_WINDOW, 5, WidgetKind::Button) {
            b.selected = true;
        }
        for id in [6, 7] {
            self.sheet
                .show(STORAGE_WINDOW, id, WidgetKind::Button, false);
        }
        let [discard, modify, pack] = self.store_homes;
        let to_pack = self.pack_target.is_some();
        let (d_at, m, b_shown) = match self.tab {
            // DC-DEFAULT: CLI-974 the melee page shows no Backpack button (the backpack takes no melee weapon,
            // slotpick::MELEE_SLOT; cshell+0x140590 shows it on categories 2 and 3)
            storage::TAB_WEAPONS if matches!(self.category, 2 | 3) => {
                if to_pack {
                    (pack, None, false)
                } else {
                    (modify, None, self.category != 2)
                }
            }
            storage::TAB_WEAPONS => {
                if to_pack {
                    (modify, Some(pack), false)
                } else {
                    (discard, Some(modify), true)
                }
            }
            TAB_BODY | TAB_MARKET => (pack, None, false),
            _ => return,
        };
        let s = &mut self.sheet;
        if let Some(b) = s.part_mut(STORAGE_WINDOW, 0, WidgetKind::Button) {
            b.pos = d_at;
        }
        if let Some(b) = s.part_mut(STORAGE_WINDOW, 1, WidgetKind::Button) {
            b.shown = m.is_some();
            b.pos = m.unwrap_or(modify);
        }
        s.show(STORAGE_WINDOW, 2, WidgetKind::Button, b_shown);
    }

    fn category_of(&self, cmd: i32) -> Option<usize> {
        CATEGORY_COMMANDS
            .get(self.tab)
            .and_then(|c| c.iter().position(|x| *x == cmd))
    }

    /// The equipment rows of the left pane (slot, category, empty string, rectangle).
    pub fn equip_row_rects(&self) -> Vec<(usize, usize, i32, [i32; 4])> {
        let list = match self.tab {
            storage::TAB_WEAPONS => WEAPON_LIST,
            TAB_BODY => BODY_LIST,
            _ => return Vec::new(),
        };
        let Some(l) = self.sheet.part(LEFT_WINDOW, list, WidgetKind::List) else {
            return Vec::new();
        };
        equip_rows(self.tab)
            .iter()
            .enumerate()
            .map(|(i, (slot, cat, empty))| {
                (
                    *slot,
                    *cat,
                    *empty,
                    [
                        l.pos[0],
                        l.pos[1] + i as i32 * ROW_PITCH,
                        ROW_SIZE[0],
                        ROW_SIZE[1],
                    ],
                )
            })
            .collect()
    }

    /// The backpack list's place and its slot rows' count (0: the default row, no drawers).
    fn pack_rows(&self, data: &MenuData) -> Option<([i32; 2], usize)> {
        if self.tab != storage::TAB_WEAPONS {
            return None;
        }
        let at = self
            .sheet
            .part(LEFT_WINDOW, pane::BACKPACK_LIST, WidgetKind::List)?
            .pos;
        let slots = slotpick::pack_slots(&data.store, &self.wares);
        Some((at, if slots < 1 { 0 } else { (slots % 5) as usize }))
    }

    /// The backpack list's place and the slot rows' count of the last hover.
    fn pack_rows_now(&self) -> Option<([i32; 2], usize)> {
        let at = self
            .sheet
            .part(LEFT_WINDOW, pane::BACKPACK_LIST, WidgetKind::List)?
            .pos;
        (self.tab == storage::TAB_WEAPONS && self.pack_count > 0).then_some((at, self.pack_count))
    }

    /// The open drawer button under `p` (backpack row, button).
    fn drawer_button_at(&self, data: &MenuData, p: [f32; 2]) -> Option<(usize, usize)> {
        let (at, n) = self.pack_rows(data)?;
        pane::drawer_button_at(at, n, &self.look, p)
    }

    /// CLI-1160 the filled backpack row whose unequip X is under `p`: (row, the weapon in its slot).
    fn pack_x_at(&self, data: &MenuData, p: [f32; 2]) -> Option<(usize, (u32, u32))> {
        let (at, n) = self.pack_rows(data)?;
        let row = pane::unequip_x_at(at, n, p)?;
        let r = data
            .store
            .in_slot(slotpick::FIRST_PACK_SLOT as usize + row)?;
        Some((row, (r.uid, r.item_no)))
    }

    /// A press while the purchase window is open (the window takes every press).
    fn buy_press(&mut self, p: [f32; 2], ctx: &mut Ctx) {
        let Some(b) = self.buy_box.as_mut() else {
            return;
        };
        match b.press(p) {
            BoxPress::Inside | BoxPress::Outside => {}
            BoxPress::Cancel => self.buy_box = None,
            // storage-functions: the rank page's OK (the prepaid form goes on with the purchase) and Details
            BoxPress::RankClosed { proceed } => {
                if proceed {
                    let product = b.offer().product;
                    ctx.popups.push(buybox::nx_last_ask(ctx.cat, product));
                }
            }
            BoxPress::Charge => ctx.popups.push(buybox::charge_box()),
            BoxPress::Buy { mode, .. } => {
                let offer = b.offer().clone();
                self.buy_box = None;
                self.request(&offer, mode, ctx);
            }
            BoxPress::Confirm => {
                // EVIDENCE: cshell+0x1da810 (box res 0x17db titled 0x371 with OK, callback cshell+0x1d7c80)
                let t = b.confirm_text(ctx.cat);
                ctx.popups
                    .push(buybox::nx_confirm_box(t, b.offer().product));
            }
        }
    }

    /// The box of a refused purchase: CLI-1630's rank box when the lobby refuses (0x60, LG-302) a ware the character's
    /// rank does not reach, else [`refused_box`].
    fn refusal_box(&self, cat: &Catalog, reason: u16, credit: bool, data: &MenuData) -> Popup {
        match self.buying.as_ref() {
            Some(w) if reason == 0x60 && locked(w.unlock_exp, lock_exp(data)) => {
                buybox::rank_box(&buybox::rank_of_exp(cat, w.unlock_exp).0)
            }
            _ => refused_box(cat, reason, credit),
        }
    }

    /// The shop's purchase check and the 0x328 request (cshell+0x145b60).
    fn request(&mut self, offer: &buybox::Offer, mode: u32, ctx: &mut Ctx) {
        match buybox::purchase(offer, mode, &ctx.data.store, ctx.data, self.buy_busy) {
            Ok(cmd) => {
                self.buy_busy = true;
                // CLI-1875: the charge is the requested row's (a cell shows its item's cheapest product)
                self.buying_charge = match &cmd {
                    LinkCmd::BuyWith { option, .. } => (*option, offer.price),
                    _ => (0, offer.price),
                };
                ctx.send(cmd);
            }
            Err(buybox::SHORT_NX_TEXT) => ctx.popups.push(buybox::short_nx_box(ctx.cat)),
            Err(id) => ctx.error(id, BoxTag::None),
        }
    }

    /// Discard (1011) on the storage page: the 2009 refusals, else the confirmation box.
    // EVIDENCE: cshell+0x14f830 (1011), cshell+0x146840 (the refusals, see storage::discard_refusal)
    fn discard_selected(&mut self, ctx: &mut Ctx) {
        let items = self.items_shown(ctx.data);
        let r = self
            .selected
            .and_then(|i| items.get(self.page * CELLS + i))
            .cloned();
        if let Some((text, title)) = storage::discard_refusal(&ctx.data.store, r.as_ref()) {
            let mut q = Popup::ok(ctx.cat.text(text), BoxTag::None);
            q.title = title;
            ctx.popups.push(q);
            return;
        }
        let Some(r) = r else {
            return;
        };
        let ask = storage::discard_question(&self.wares, &ctx.data.store, r.item_no as i32);
        let mut q = Popup::ask(
            ctx.cat.text(ask),
            BoxTag::Discard {
                uid: r.uid,
                item: r.item_no,
            },
        );
        q.title = storage::DISCARD_TITLE;
        q.captions = Some(storage::DISCARD_BUTTONS);
        ctx.popups.push(q);
    }

    /// Opens slot choice window `window` for the item (unique id, item number).
    fn open_slot_pick(&mut self, window: i32, uid: u32, item: u32, ctx: &mut Ctx) {
        self.slot_pick = Some(SlotPick::open(
            &mut self.sheet,
            ctx.cat,
            &ctx.data.store,
            &self.wares,
            window,
            uid,
            item,
        ));
    }

    /// The slot choice windows' commands (`false`: not one of them).
    fn slot_pick_click(&mut self, command: i32, ctx: &mut Ctx) -> bool {
        let Some(p) = self.slot_pick.as_mut() else {
            return false;
        };
        match command {
            slotpick::CMD_LIST => p.toggle_list(&self.sheet),
            slotpick::CMD_OK => {
                let req = p.request(&self.wares);
                p.close(&mut self.sheet);
                self.slot_pick = None;
                if let Some(cmd) = req {
                    ctx.send(cmd);
                }
            }
            slotpick::CMD_CANCEL => {
                p.close(&mut self.sheet);
                self.slot_pick = None;
            }
            _ => {}
        }
        // the window is modal: other commands do nothing while it is up
        // DC-DEFAULT: CLI-772 (proposed) presses outside an open slot choice window do nothing
        true
    }

    /// The storage page's Backpack (1009): the backpack check, then window 6 for the selected item.
    // EVIDENCE: cshell+0x14f830 (1009 -> cshell+0x148020(1), then cshell+0x142b60(1))
    fn backpack_selected(&mut self, ctx: &mut Ctx) {
        let items = self.items_shown(ctx.data);
        let r = self
            .selected
            .and_then(|i| items.get(self.page * CELLS + i))
            .cloned();
        match slotpick::pack_check(&ctx.data.store, &self.wares, r.as_ref().map(|r| r.item_no)) {
            Ok(()) => {
                if let Some(r) = r {
                    self.open_slot_pick(slotpick::PACK_WINDOW, r.uid, r.item_no, ctx);
                }
            }
            Err(Some((text, title))) => {
                let mut q = Popup::ok(ctx.cat.text(text), BoxTag::None);
                q.title = title;
                ctx.popups.push(q);
            }
            Err(None) => {}
        }
    }

    /// name-change: the use of function item `uid` (item number `item_no`): the window of its kind, or the use
    /// request at once (CLI-1365).
    // EVIDENCE: cshell+0x1db770 (the window by the item row's SubType), cshell+0x2450e0 (the generic window sends at
    // once), cshell+0x242330 (the name window shows the own name for kind 4, the clan name for kind 1)
    pub fn use_function(&mut self, uid: u32, item_no: i32, ctx: &mut Ctx) {
        if self.using.is_some() || self.use_window_open() || self.confirming.is_some() {
            return;
        }
        let sub = self.wares.art_of(item_no).map_or(-1, |a| a.sub_type);
        match usebox::action_of(sub) {
            // use-windows: the weapon change asks first (CLI-1370)
            // EVIDENCE: cshell+0x2450e0 (the item's name, the change item (cshell+0x25c500) and its slot word in box
            // 0x1876; OK sends, Cancel runs the window's 0x267: the cancelled box)
            UseAction::WeaponChange => {
                let change = self
                    .wares
                    .art_of(item_no)
                    .map_or(0, |a| a.change_item)
                    .max(0) as u32;
                let word = self.slot_word(change);
                let name = self.wares.label_of(item_no);
                ctx.popups
                    .push(usebox::weapon_change_ask(ctx.cat, &name, &word, uid));
                self.confirming = Some(Using {
                    uid,
                    action: UseAction::WeaponChange,
                    change,
                });
            }
            // use-windows: the record reset window (CLI-1371); the clan window asks for the clan's record
            UseAction::Record { clan } => {
                let x = &ctx.data.char_extras;
                let own = [x.kills, x.deaths, x.headshots];
                self.record_box = Some(recordbox::RecordBox::open(ctx.cat, uid, clan, own));
                if clan {
                    ctx.send(LinkCmd::ClanInfo);
                }
            }
            UseAction::Rename { clan } => {
                let current = if clan {
                    ctx.data.char_extras.clan_label.clone()
                } else {
                    ctx.data.profile.nick.clone()
                };
                self.name_box = Some(usebox::NameBox::open(ctx.cat, uid, clan, &current));
            }
            // use-items: the country window (CLI-1873) with the own nation and the ROM country table
            UseAction::Country => {
                let countries = ctx
                    .cat
                    .rom
                    .clone()
                    .map(|r| super::charinfo::Tables::cached(&r).countries)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(full, short, flag)| countrybox::Country { full, short, flag })
                    .collect();
                let current = ctx.data.char_extras.nation;
                self.country_box = Some(countrybox::CountryBox::open(
                    ctx.cat, uid, current, countries,
                ));
            }
            // use-items: the megaphone (CLI-1874): in use -> the composer; else the use first, its ok answer opens it
            // EVIDENCE: cshell+0x1db770 (kind 0x16 with the in-use flag -> the composer, else the generic window),
            // cshell+0x2450e0 (the generic window sends at once)
            UseAction::Megaphone => {
                let rec = ctx.data.store.items.iter().find(|i| i.uid == uid).cloned();
                match rec.filter(|r| r.in_use) {
                    Some(r) => self.open_megaphone(uid, r.count, ctx),
                    None => {
                        self.using = Some(Using {
                            uid,
                            action: UseAction::Megaphone,
                            change: 0,
                        });
                        ctx.send(LinkCmd::ItemUse {
                            uid,
                            name: String::new(),
                            change_item: 0,
                            nation: 0,
                        });
                    }
                }
            }
            UseAction::Direct => {
                self.using = Some(Using {
                    uid,
                    action: UseAction::Direct,
                    change: 0,
                });
                ctx.send(LinkCmd::ItemUse {
                    uid,
                    name: String::new(),
                    change_item: 0,
                    nation: 0,
                });
            }
            // use-items-sweep: the colour window (CLI-1876), the in-use question first (CLI-1877)
            UseAction::Colour { kind } => self.open_colour(uid, item_no, kind, ctx),
            // use-items-emblem: the clan mark window (CLI-1884) with the clan's mark now and the ROM mark table
            // EVIDENCE: cshell+0x2ae340 (the own clan's mark: shell +0x428 / +0x42c)
            UseAction::Mark => {
                let x = &ctx.data.char_extras;
                let own = (x.clan_mark, x.clan_mark_ground);
                let rom = ctx.cat.rom.clone();
                let marks = rom
                    .as_deref()
                    .map(|r| super::clan::ClanLits::cached(r).marks)
                    .unwrap_or_default();
                let frame = markbox::frame_art(rom.as_deref());
                self.mark_box = Some(markbox::MarkBox::open(ctx.cat, uid, own, marks, frame));
            }
            // use-items-emblem: the face window (CLI-1885) for the own face row (the character record's face value
            // less 6000)
            UseAction::Face { kind } => {
                let rows = facebox::rows_from_rom(ctx.cat.rom.as_deref());
                let own = ctx.data.profile.face - facebox::FACE_BASE;
                self.face_box = Some(facebox::FaceBox::open(ctx.cat, kind, uid, own, &rows));
            }
            UseAction::NoWindow(_) => {}
        }
    }

    /// use-items: a use window is up (it takes the pointer and the keys).
    fn use_window_open(&self) -> bool {
        self.name_box.is_some()
            || self.record_box.is_some()
            || self.country_box.is_some()
            || self.mega_box.is_some()
            || self.colour_box.is_some()
            || self.mark_box.is_some()
            || self.face_box.is_some()
    }

    /// use-items-sweep: the colour window of kind `kind` for item `uid` (number `item_no`); another item of the kind in
    /// use asks first (CLI-1877).
    // EVIDENCE: cshell+0x244f30 (the in-use record of the kind: its unique id for the send, its colour chosen, the
    // question box), cshell+0x1db770 (kinds 8, 9, 11 -> the colour window)
    // DISASM: cshell+0x24a780 (kind 8: the own name; 9: the clan name; 11: the reticle in use in its colour, else the
    // plain picture in green; this item's PreViewIcon)
    // DC-DEFAULT: CLI-1877 (proposed) the item in use is any other in-use item of the same SubType (the 2009 client looks
    // a reticle up by its item number, cshell+0x228810)
    fn open_colour(&mut self, uid: u32, item_no: i32, kind: i32, ctx: &mut Ctx) {
        let in_use = ctx
            .data
            .store
            .items
            .iter()
            .find(|r| {
                r.in_use
                    && r.uid != uid
                    && self
                        .wares
                        .art_of(r.item_no as i32)
                        .is_some_and(|a| a.sub_type == kind)
            })
            .map(|r| colorbox::InUse {
                uid: r.uid,
                colour: r.function,
                same_item: r.item_no as i32 == item_no,
            });
        let name = if kind == colorbox::SUB_CLAN_COLOUR {
            ctx.data.char_extras.clan_label.clone()
        } else {
            ctx.data.profile.nick.clone()
        };
        let after = self
            .wares
            .art_of(item_no)
            .map(|a| a.preview_icon.clone())
            .unwrap_or_default();
        let plain = (
            self.colour_art.plain.clone().unwrap_or_default(),
            colorbox::NO_RETICLE_COLOUR,
        );
        let before = in_use
            .and_then(|u| {
                let rec = ctx.data.store.items.iter().find(|r| r.uid == u.uid)?;
                let tex = self.wares.art_of(rec.item_no as i32)?.preview_icon.clone();
                let c = self
                    .colour_tables
                    .reticle_argb(u.colour)
                    .unwrap_or(colorbox::NO_RETICLE_COLOUR);
                Some((tex, c))
            })
            .unwrap_or(plain);
        let swatches = self.colour_tables.of(kind).to_vec();
        self.colour_box = Some(colorbox::ColourBox::open(
            ctx.cat, kind, uid, swatches, &name, before, &after, in_use,
        ));
        if let Some(u) = in_use {
            ctx.popups
                .push(colorbox::in_use_box(ctx.cat, kind, &u, uid));
        }
    }

    /// use-items-sweep: what the colour window asked for.
    fn colour_step(&mut self, step: ColourStep, ctx: &mut Ctx) {
        match step {
            ColourStep::None => {}
            // EVIDENCE: cshell+0x242b80 (0x330 with the unique id, the colour and the in-use item's unique id)
            ColourStep::Use { colour, used } => {
                let Some(b) = self.colour_box.as_ref() else {
                    return;
                };
                self.using = Some(Using {
                    uid: b.uid,
                    action: UseAction::Colour { kind: b.kind },
                    change: 0,
                });
                ctx.send(LinkCmd::ColourUse {
                    uid: b.uid,
                    colour,
                    used,
                });
            }
            ColourStep::Cancel => {
                self.colour_box = None;
                ctx.popups.push(usebox::cancelled_box(ctx.cat));
            }
        }
    }

    /// use-items-emblem: what the clan mark window asked for (CLI-1884).
    fn mark_step(&mut self, step: MarkStep, ctx: &mut Ctx) {
        let Some(b) = self.mark_box.as_ref() else {
            return;
        };
        match step {
            MarkStep::None => {}
            // DISASM: cshell+0x2aada0 (0x34c with the window's emblem and background)
            MarkStep::Check { emblem, background } => {
                ctx.send(LinkCmd::MarkCheck { emblem, background })
            }
            // EVIDENCE: cshell+0x2ae470 (0x262: 0x330 with the unique id and the pair)
            MarkStep::Use { emblem, background } => {
                self.using = Some(Using {
                    uid: b.uid,
                    action: UseAction::Mark,
                    change: 0,
                });
                ctx.send(LinkCmd::MarkUse {
                    uid: b.uid,
                    emblem,
                    background,
                });
            }
            MarkStep::Notice(text) => ctx.popups.push(markbox::notice_box(ctx.cat, text)),
            MarkStep::Cancel => {
                self.mark_box = None;
                ctx.popups.push(usebox::cancelled_box(ctx.cat));
            }
        }
    }

    /// use-items-emblem: what the face window asked for (CLI-1885).
    fn face_step(&mut self, step: FaceStep, ctx: &mut Ctx) {
        let Some(b) = self.face_box.as_ref() else {
            return;
        };
        match step {
            FaceStep::None => {}
            // DISASM: cshell+0x242ad0 (0x330 with the unique id and the face value)
            FaceStep::Use { face } => {
                self.using = Some(Using {
                    uid: b.uid,
                    action: UseAction::Face { kind: b.kind },
                    change: 0,
                });
                ctx.send(LinkCmd::FaceUse { uid: b.uid, face });
            }
            // EVIDENCE: cshell+0x246a30 (other commands go to cshell+0x2433d0, whose 0x267 shows box 0x1807 titled
            // 0x1811 and closes the window)
            FaceStep::Cancel => {
                self.face_box = None;
                ctx.popups.push(usebox::cancelled_box(ctx.cat));
            }
        }
    }

    /// use-items: the megaphone composer for `uid` with `uses` left.
    fn open_megaphone(&mut self, uid: u32, uses: u32, ctx: &mut Ctx) {
        let name = ctx.data.profile.nick.clone();
        self.mega_box = Some(megabox::MegaBox::open(ctx.cat, uid, uses, &name));
    }

    /// use-items: what the country window asked for.
    fn country_step(&mut self, step: CountryStep, ctx: &mut Ctx) {
        match step {
            CountryStep::None => {}
            // EVIDENCE: cshell+0x2a9440 (0x330 with the unique id and the picked nation)
            CountryStep::Use(nation) => {
                let Some(b) = self.country_box.as_ref() else {
                    return;
                };
                self.using = Some(Using {
                    uid: b.uid,
                    action: UseAction::Country,
                    change: 0,
                });
                ctx.send(LinkCmd::ItemUse {
                    uid: b.uid,
                    name: String::new(),
                    change_item: 0,
                    nation,
                });
            }
            CountryStep::Cancel => {
                self.country_box = None;
                ctx.popups.push(usebox::cancelled_box(ctx.cat));
            }
        }
    }

    /// use-items: what the megaphone composer asked for.
    fn mega_step(&mut self, step: MegaStep, ctx: &mut Ctx) {
        match step {
            MegaStep::None => {}
            MegaStep::Queue => ctx.send(LinkCmd::MegaphoneQueue),
            MegaStep::Send(text) => {
                if let Some(b) = self.mega_box.as_ref() {
                    ctx.send(LinkCmd::MegaphoneSend { uid: b.uid, text });
                }
            }
            MegaStep::Cancel => {
                self.mega_box = None;
                ctx.popups.push(usebox::cancelled_box(ctx.cat));
            }
        }
    }

    /// use-items: the answer to a megaphone send: the uses left (0: the item is gone); the composer closes either way.
    // EVIDENCE: cshell+0x2a5c90 (ok: the record's count := +0x12, 0 -> the record deleted, the page rebuilt; refused:
    // the reason box; then the window closes, cshell+0x1db770(0))
    fn megaphone_sent(&mut self, r: &super::link::SendReply, ctx: &mut Ctx) {
        let Some(b) = self.mega_box.take() else {
            return;
        };
        if !r.ok {
            ctx.popups.push(usebox::reason_box(ctx.cat, r.reason));
            return;
        }
        let uid = if r.uid != 0 { r.uid } else { b.uid };
        if r.left == 0 {
            ctx.data.store.remove(uid);
            self.selected = None;
            let n = self.entries(ctx.data);
            if self.page >= page_count(n) {
                self.page = 0;
            }
        } else if let Some(rec) = ctx.data.store.items.iter_mut().find(|i| i.uid == uid) {
            rec.count = r.left;
        }
    }

    /// use-windows: the slot word of item `item`'s equipment slot (empty for another slot).
    fn slot_word(&self, item: u32) -> String {
        own_slot(&self.wares, item)
            .and_then(|s| self.slot_words.get(s as usize).cloned())
            .unwrap_or_default()
    }

    /// use-windows: what the record window asked for.
    fn record_step(&mut self, step: RecordStep, ctx: &mut Ctx) {
        let Some(b) = self.record_box.as_ref() else {
            return;
        };
        match step {
            RecordStep::None => {}
            RecordStep::Ask => {
                ctx.popups.push(b.ask_box(ctx.cat));
                self.confirming = Some(Using {
                    uid: b.uid,
                    action: UseAction::Record { clan: b.clan },
                    change: 0,
                });
            }
            RecordStep::Cancel => {
                self.record_box = None;
                ctx.popups.push(usebox::cancelled_box(ctx.cat));
            }
        }
    }

    /// use-windows: the answer to a use window's question box: OK sends the use, Cancel of the weapon change shows
    /// the cancelled box (the record window's check box only closes).
    // EVIDENCE: cshell+0x243220 (the weapon change box: OK -> the send vfunc +0x3c, Cancel -> command 0x267, the
    // cancelled box of cshell+0x2433d0), cshell+0x242170 (the record check box: OK only)
    fn use_confirmed(&mut self, uid: u32, yes: bool, ctx: &mut Ctx) {
        let Some(u) = self.confirming.take().filter(|u| u.uid == uid) else {
            return;
        };
        if !yes {
            if u.action == UseAction::WeaponChange {
                ctx.popups.push(usebox::cancelled_box(ctx.cat));
            }
            return;
        }
        if let Some(b) = self.record_box.as_mut() {
            b.waiting = true;
        }
        self.using = Some(u);
        ctx.send(LinkCmd::ItemUse {
            uid: u.uid,
            name: String::new(),
            change_item: u.change,
            nation: 0,
        });
    }

    /// name-change: what the rename window asked for.
    fn name_step(&mut self, step: NameStep, ctx: &mut Ctx) {
        match step {
            NameStep::None => {}
            NameStep::Check(name) => ctx.send(LinkCmd::NameCheck(name)),
            NameStep::Use(name) => {
                let Some(b) = self.name_box.as_ref() else {
                    return;
                };
                self.using = Some(Using {
                    uid: b.uid,
                    action: UseAction::Rename { clan: b.clan },
                    change: 0,
                });
                ctx.send(LinkCmd::ItemUse {
                    uid: b.uid,
                    name,
                    change_item: 0,
                    nation: 0,
                });
            }
            NameStep::Cancel => {
                self.name_box = None;
                ctx.popups.push(usebox::cancelled_box(ctx.cat));
            }
        }
    }

    /// name-change: the answer to the use request (0x331).
    // EVIDENCE: cshell+0x249d80 (name window: ok -> the success box with the new name, the own (kind 4) or clan
    // (kind 1) name set, the item deleted (cshell+0x22ccb0), the page rebuilt; refused -> the reason box
    // cshell+0x393f0, the window stays), cshell+0x2452f0 (generic window: refused -> the reason box; ok -> the item
    // marked in use with the reply's end and count, a second item +0x12 deleted)
    // DC-DEFAULT: CLI-1365 (proposed) an ok generic use asks for the inventory again (the lobby's record carries the
    // in-use mark, end and count); a refused rename keeps the window with the check dropped
    fn item_used(&mut self, r: &super::link::UseReply, ctx: &mut Ctx) {
        let Some(u) = self.using.take().filter(|u| u.uid == r.uid || r.uid == 0) else {
            return;
        };
        if !r.ok {
            if let Some(b) = self.name_box.as_mut() {
                b.waiting = false;
                b.checked = None;
                b.message = b.texts.errors[2];
            }
            if let Some(b) = self.record_box.as_mut() {
                b.waiting = false;
            }
            if let Some(b) = self.country_box.as_mut() {
                b.refused();
            }
            if let Some(b) = self.colour_box.as_mut() {
                b.refused();
            }
            if let Some(b) = self.mark_box.as_mut() {
                b.refused();
            }
            if let Some(b) = self.face_box.as_mut() {
                b.refused();
            }
            ctx.popups.push(usebox::reason_box(ctx.cat, r.reason));
            return;
        }
        if r.removed != 0 && r.removed != u.uid {
            ctx.data.store.remove(r.removed);
        }
        match u.action {
            UseAction::Rename { clan } => {
                let Some(b) = self.name_box.take() else {
                    return;
                };
                let name = b.text.clone();
                ctx.popups
                    .push(usebox::renamed_box(ctx.cat, &b.texts, &name));
                if clan {
                    ctx.data.char_extras.clan_label = name;
                } else {
                    ctx.data.profile.nick = name.clone();
                    ctx.send(LinkCmd::OwnName(name));
                }
                ctx.data.store.remove(u.uid);
                self.selected = None;
                let n = self.entries(ctx.data);
                if self.page >= page_count(n) {
                    self.page = 0;
                }
            }
            // use-windows: the record reset (CLI-1371)
            UseAction::Record { clan } => {
                if let Some(b) = self.record_box.take() {
                    ctx.popups.push(b.done_box(ctx.cat));
                }
                if !clan {
                    let x = &mut ctx.data.char_extras;
                    x.kills = 0;
                    x.deaths = 0;
                    x.headshots = 0;
                }
                ctx.data.store.remove(u.uid);
                self.selected = None;
                let n = self.entries(ctx.data);
                if self.page >= page_count(n) {
                    self.page = 0;
                }
            }
            // use-windows: the weapon change (CLI-1370): the item becomes the change weapon, the replaced default
            // (+0x12) is gone, the success box names the slot and the weapon; the inventory is read again (the
            // lobby's records carry the new default and its slot)
            // EVIDENCE: cshell+0x2452f0 (kind 12: cshell+0x22c880 converts the record to the change item, +0x12
            // deleted, box 0x187b, the page rebuilt)
            UseAction::WeaponChange => {
                if let Some(rec) = ctx.data.store.items.iter_mut().find(|i| i.uid == u.uid) {
                    rec.item_no = u.change;
                }
                let word = self.slot_word(u.change);
                let name = self.wares.label_of(u.change as i32);
                ctx.popups
                    .push(usebox::weapon_changed_box(ctx.cat, &word, &name));
                ctx.send(LinkCmd::Inventory);
            }
            // use-items: the country change (CLI-1873): the own nation is the picked one, the item is gone
            // EVIDENCE: cshell+0x2a94b0 (ok: shell +0x444 := the picked nation, the item deleted (cshell+0x22ccb0), the
            // page rebuilt, the window closed; no box)
            UseAction::Country => {
                if let Some(b) = self.country_box.take() {
                    ctx.data.char_extras.nation = b.picked;
                    ctx.data.profile.nation = b.picked;
                }
                ctx.data.store.remove(u.uid);
                self.selected = None;
                let n = self.entries(ctx.data);
                if self.page >= page_count(n) {
                    self.page = 0;
                }
            }
            // use-items: the megaphone's first use (CLI-1874): marked in use, the composer opens
            // EVIDENCE: cshell+0x2452f0 (kind 22 ok: the item marked in use with the end and count (cshell+0x22c990),
            // then cshell+0x1db770 with the in-use flag: the composer)
            UseAction::Megaphone => {
                let mut uses = r.count;
                if let Some(rec) = ctx.data.store.items.iter_mut().find(|i| i.uid == u.uid) {
                    rec.in_use = true;
                    if r.count != 0 {
                        rec.count = r.count;
                    }
                    uses = rec.count;
                }
                self.open_megaphone(u.uid, uses, ctx);
            }
            // use-items-sweep: the colour (CLI-1876): the item in use with the colour as its function word; a name or
            // clan colour also becomes the own one
            // DISASM: cshell+0x24a3a0 (ok: the colour's ARGB -> shell +0x434 (kind 8) / +0x438 (9) / +0x43c (11); the
            // record marked in use with the end and the colour (cshell+0x22c990); a second unique id other than the
            // item's deleted; the window closed)
            UseAction::Colour { kind } => {
                let colour = self.colour_box.take().map_or(0, |b| b.picked);
                if let Some(rec) = ctx.data.store.items.iter_mut().find(|i| i.uid == u.uid) {
                    rec.in_use = true;
                    rec.function = colour;
                    rec.end = r.end;
                }
                match kind {
                    colorbox::SUB_NAME_COLOUR => ctx.data.char_extras.name_colour = colour,
                    colorbox::SUB_CLAN_COLOUR => ctx.data.char_extras.clan_colour = colour,
                    _ => {}
                }
            }
            // use-items-emblem: the clan mark (CLI-1884): the box, the pair the own clan's mark, the item in use
            // EVIDENCE: cshell+0x2ab220 (ok: box 0x1798 titled 0x17e8 whose OK closes the window; the emblem and
            // background -> shell +0x428 / +0x42c (cshell+0x1b14e0 / 0x1b1550); the record marked in use with the reply's
            // end (cshell+0x22c990); a +0x12 unique id other than the item's deleted)
            UseAction::Mark => {
                if let Some(b) = self.mark_box.take() {
                    ctx.data.char_extras.clan_mark = b.emblem;
                    ctx.data.char_extras.clan_mark_ground = b.background;
                }
                if let Some(rec) = ctx.data.store.items.iter_mut().find(|i| i.uid == u.uid) {
                    rec.in_use = true;
                    rec.end = r.end;
                }
                ctx.popups.push(markbox::done_box(ctx.cat));
            }
            // use-items-emblem: the face (CLI-1885): the own face is the picked one (the pane's figure wears it), the
            // item is gone, no box
            // DISASM: cshell+0x24a1d0 (ok: shell character +0x158 := the picked face, the look rebuilt
            // (cshell+0x1d3350 / 0x1d33d0), the item deleted (cshell+0x22ccb0), the window closed (cshell+0x1db770))
            UseAction::Face { .. } => {
                if let Some(face) = self.face_box.take().and_then(|b| b.picked_face()) {
                    ctx.data.profile.face = face + facebox::FACE_BASE;
                }
                ctx.data.store.remove(u.uid);
                self.selected = None;
                let n = self.entries(ctx.data);
                if self.page >= page_count(n) {
                    self.page = 0;
                }
            }
            _ => ctx.send(LinkCmd::Inventory),
        }
    }

    fn equip_selected(&mut self, ctx: &mut Ctx) {
        let items = self.items_shown(ctx.data);
        let Some(r) = self
            .selected
            .and_then(|i| items.get(self.page * CELLS + i))
            .cloned()
        else {
            ctx.notice(0x3d2);
            return;
        };
        // name-change: Equip on a function item opens its use window (CLI-1365); an item in use opens nothing
        // DISASM: cshell+0x14f8a7..0x14f965 (265 on a type 0x15 cell: in use and kind != 0x16 -> no window)
        if self.wares.kind_of(r.item_no as i32) == Some(usebox::FUNCTION_TYPE) {
            // use-items: an in-use megaphone opens its composer (CLI-1874)
            let sub = self
                .wares
                .art_of(r.item_no as i32)
                .map_or(-1, |a| a.sub_type);
            if !r.in_use || sub == usebox::SUB_MEGAPHONE {
                self.use_function(r.uid, r.item_no as i32, ctx);
                return;
            }
            // function-unequip (CLI-1395): Equip on an item in use whose row has IsCanUnEquip takes it out of its
            // function slot (0x36a) while the function list holds it, else puts it back (0x368)
            // DISASM: cshell+0x14f96a..0x14f9b4 (record in use and row +0x241 set: cshell+0x226800 finds the unique id
            // in the function list -> cshell+0x141480, else cshell+0x1413c0); cshell+0x1413c0 / 0x141480 (the same
            // checks again, then the sender cshell+0x18f7b0 (0x368) / 0x18f890 (0x36a) with the unique id and the
            // record's item number)
            if self.wares.can_unequip(r.item_no as i32) {
                ctx.send(LinkCmd::FunctionSlot {
                    put: !self.wares.in_function_list(&ctx.data.store, &r),
                    uid: r.uid,
                    item: r.item_no,
                });
            }
            return;
        }
        // DC-DEFAULT: CLI-547 Equip on an equipped item unequips it (0x326 with its slot); the 2009
        // unequip gesture (cshell+0x145ed0 -> sender cshell+0x18af30) is not traced
        match (ctx.data.store.slot_of(&r), own_slot(&self.wares, r.item_no)) {
            (Some(slot), _) => ctx.send(LinkCmd::Unequip {
                uid: r.uid,
                item: r.item_no,
                slot: slot as u32,
            }),
            // CLI-972 the storage page of a backpack drawer equips into that backpack slot
            // DISASM: cshell+0x14fa39..0x14fa85 (Equip 265: an unequipped item with backpack slots on the weapons tab
            // and +0x55a8 set -> +0x55a4 = +0x55a8 and the backpack request cshell+0x14baa0 (slot +0x55a4 + 10); else
            // the own-slot request cshell+0x147e40)
            (None, Some(_))
                if self.tab == storage::TAB_WEAPONS
                    && self.pack_target.is_some()
                    && slotpick::pack_slots(&ctx.data.store, &self.wares) >= 1 =>
            {
                ctx.send(LinkCmd::Equip {
                    uid: r.uid,
                    item: r.item_no,
                    slot: slotpick::FIRST_PACK_SLOT + self.pack_target.unwrap_or(0) as u32,
                })
            }
            (None, Some(slot)) => ctx.send(LinkCmd::Equip {
                uid: r.uid,
                item: r.item_no,
                slot,
            }),
            (None, None) => ctx.notice(equip_refused_text(false)),
        }
    }
}

impl Screen for ShopScreen {
    fn name(&self) -> &'static str {
        "shop"
    }
    // shop-model: the character in the left pane on every tab (OBSERVED screens-sweep/pane1/,
    // pane3/: the reference stack shows it on the weapons, body and black market tabs)
    // shop-model: the look from the equipped items (CLI-743, cshell+0x14e410)
    fn pane_figure(&self, data: &MenuData) -> Option<super::modelview::ViewReq> {
        let mut equipped = data.store.equipped;
        if let Some(e) = self
            .preview
            .and_then(|(slot, n)| Some((equipped.get_mut(slot)?, n)))
        {
            *e.0 = e.1;
        }
        // CLI-853: the account's own face (the character record's face model, 6000 + row)
        let face = data.profile.face - dc_botclient::lobby::FACE_BASE;
        let ids = super::figure::LookIds::with_account_face(
            &equipped,
            if (0..1000).contains(&face) { face } else { 0 },
        );
        let (look, pieces) = self
            .figure
            .as_ref()
            .and_then(|t| t.look(&ids))
            .map_or((None, Vec::new()), |(l, p)| (Some(l), p));
        let lights = self.figure.as_ref().map_or(Vec::new(), |t| t.lights(&ids));
        Some(super::modelview::ViewReq {
            look,
            pieces,
            lights,
            place: super::modelview::Placement {
                yaw: self.figure_yaw,
                ..super::modelview::PLACE
            },
            rect: super::modelview::PANE_RECT,
            // CLI-1629: the player's x-ray style previewed on the character (the shop page only)
            xray: data.xray_offered.then_some(data.settings.xray),
        })
    }
    /// CLI-971 / CLI-972: the pane row under the pointer takes its _On image; a backpack slot row under the pointer
    /// (or its open drawer's buttons) slides its drawer out, the others slide theirs back.
    fn hover(&mut self, p: [f32; 2], data: &MenuData) {
        if let Some((last, sx)) = self.figure_drag {
            self.figure_yaw = turned_yaw(self.figure_yaw, (p[0] - last) * sx);
            self.figure_drag = Some((p[0], sx));
        }
        let modal = self.buy_box.is_some() || self.slot_pick.is_some() || self.use_window_open();
        let rows = self.equip_row_rects();
        self.look.row = rows
            .iter()
            .position(|(_, _, _, r)| inside(*r, p))
            .filter(|_| !modal);
        let (pack, button) = match self.pack_rows(data).filter(|_| !modal) {
            Some((at, 0)) => (inside(pane::pack_row_rect(at, 0), p).then_some(0), None),
            Some((at, n)) => (
                pane::pack_row_at(at, n, &self.look, p),
                pane::drawer_button_at(at, n, &self.look, p),
            ),
            None => (None, None),
        };
        let slots = self.pack_rows(data).map_or(0, |(_, n)| n);
        self.pack_count = slots;
        for (i, d) in self.look.drawers.iter_mut().enumerate() {
            if pack == Some(i) && i < slots {
                d.enter();
            } else {
                d.leave();
            }
        }
        self.look.pack = pack;
        self.look.button = button;
        self.look.x = self
            .pack_x_at(data, p)
            .filter(|_| !modal)
            .map(|(row, _)| row);
        self.pointer = Some(p);
    }

    fn animate(&mut self, secs: f32) -> bool {
        let mut changed = false;
        for d in self.look.drawers.iter_mut() {
            changed |= d.step(secs * 1000.0);
        }
        if changed {
            // a drawer that just opened lights the button already under the pointer
            self.look.button = None;
            if let (Some(p), Some((at, n))) = (self.pointer, self.pack_rows_now()) {
                self.look.button = pane::drawer_button_at(at, n, &self.look, p);
            }
        }
        changed
    }

    fn animating(&self) -> bool {
        self.look.drawers.iter().any(|d| d.moving())
    }

    fn sheet(&self) -> &Sheet {
        &self.sheet
    }
    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheet
    }

    // CLI-741: the black market list scrolls under its record's bar
    fn list_rows(&self, data: &MenuData) -> Vec<(i32, i32, usize, i32)> {
        let n = pane::market_records(&self.wares, &data.store).len();
        vec![(LEFT_WINDOW, pane::MARKET_LIST, n, pane::ROW_SIZE[1])]
    }

    fn enter(&mut self, ctx: &mut Ctx) {
        if !ctx.data.store.lists_asked {
            ctx.data.store.lists_asked = true;
            ctx.send(LinkCmd::ShopLists);
        }
        match self.entry {
            Go::Market => self.set_tab(TAB_MARKET),
            Go::Inbox => self.open_inbox(ctx),
            _ => self.set_tab(storage::TAB_WEAPONS),
        }
        self.refresh(ctx.data);
        self.sync_pages(ctx.data);
        self.select_first(ctx.data);
    }

    fn input_seen(&mut self, ctx: &mut Ctx) {
        self.sync_pages(ctx.data);
        self.select_first(ctx.data);
    }

    // DC-DEFAULT: CLI-543 Escape on the shop and tuning screens runs Exit (311)
    fn key(&mut self, k: MenuKey, ctx: &mut Ctx) {
        if let Some(b) = self.name_box.as_mut() {
            let step = match k {
                MenuKey::Enter => b.enter(),
                MenuKey::Escape => NameStep::Cancel,
                _ => NameStep::None,
            };
            self.name_step(step, ctx);
            return;
        }
        if let Some(b) = self.record_box.as_ref() {
            let step = match k {
                MenuKey::Enter => b.ok(),
                MenuKey::Escape => RecordStep::Cancel,
                _ => RecordStep::None,
            };
            self.record_step(step, ctx);
            return;
        }
        if let Some(b) = self.country_box.as_mut() {
            let step = match k {
                MenuKey::Enter => b.ok(),
                MenuKey::Escape => b.escape(),
                _ => CountryStep::None,
            };
            self.country_step(step, ctx);
            return;
        }
        if let Some(b) = self.mega_box.as_mut() {
            let step = match k {
                MenuKey::Enter => b.ok(),
                MenuKey::Escape => b.escape(),
                _ => MegaStep::None,
            };
            self.mega_step(step, ctx);
            return;
        }
        if let Some(b) = self.colour_box.as_mut() {
            let step = match k {
                MenuKey::Enter => b.ok(),
                MenuKey::Escape => b.escape(),
                _ => ColourStep::None,
            };
            self.colour_step(step, ctx);
            return;
        }
        if let Some(b) = self.mark_box.as_mut() {
            let step = match k {
                MenuKey::Enter => b.enter(),
                MenuKey::Escape => b.escape(),
                _ => MarkStep::None,
            };
            self.mark_step(step, ctx);
            return;
        }
        if let Some(b) = self.face_box.as_mut() {
            let step = match k {
                MenuKey::Enter => b.ok(),
                MenuKey::Escape => b.escape(),
                _ => FaceStep::None,
            };
            self.face_step(step, ctx);
            return;
        }
        if k == MenuKey::Escape {
            // DC-DEFAULT: CLI-706 Escape closes the purchase window first (as its Cancel, command 7)
            if self.buy_box.take().is_some() {
                return;
            }
            self.click(CMD_EXIT, usize::MAX, ctx);
        }
    }

    fn click(&mut self, command: i32, _part: usize, ctx: &mut Ctx) {
        if self.slot_pick_click(command, ctx) {
            return;
        }
        match command {
            CMD_TAB_WEAPONS | CMD_BAR_SHOP => self.set_tab(storage::TAB_WEAPONS),
            CMD_TAB_BODY => self.set_tab(TAB_BODY),
            CMD_TAB_MARKET | CMD_BAR_MARKET => self.set_tab(TAB_MARKET),
            CMD_TAB_INBOX | CMD_BAR_INBOX => self.open_inbox(ctx),
            CMD_BAR_TUNING => {
                ctx.sounds.push(SOUND_PRESS);
                *ctx.go = Some(Go::Tuning);
            }
            // CLI-1021: the bar's clan button works on the shop family's pages as on the lobby's
            // EVIDENCE: cshell+0x1bf20 (the bar's commands are the common handler's, the same on every page)
            super::clan::BAR_CLAN_CMD => {
                ctx.sounds.push(SOUND_PRESS);
                *ctx.go = Some(Go::Clan);
            }
            CMD_EXIT => *ctx.go = Some(exit_to(ctx.data)),
            CMD_BAR_QUIT => {
                let t = ctx.cat.text(text_ids::ASK_QUIT);
                ctx.popups.push(Popup::ask(t, BoxTag::Quit));
            }
            CMD_STORAGE_SHOP => {
                self.pane = Pane::Shop;
                self.pack_target = None;
                self.page = 0;
                self.selected = None;
                self.show_pane();
            }
            CMD_PAGE_BACK | CMD_PAGE_FWD => {
                let pages = self.pages(ctx.data);
                if let Some(p) = group_step(self.page, pages, command == CMD_PAGE_FWD) {
                    self.page = p;
                    self.selected = None;
                    self.ask_inbox_page(ctx);
                }
            }
            CMD_BUY => {
                let shown = self.wares_shown(ctx.data);
                match self.selected.and_then(|i| shown.get(self.page * CELLS + i)) {
                    // DISASM: cshell+0x14fb0d (a GP supply case (type 0x14) asks in a yes / no box; every other cell
                    // opens the purchase window)
                    // CLI-1630: a GP supply case the character's rank does not reach shows the rank box instead
                    Some(w)
                        if w.kind_of == Some(0x14)
                            && w.sale_type == 0
                            && locked(w.unlock_exp, lock_exp(ctx.data)) =>
                    {
                        let (name, _) = buybox::rank_of_exp(ctx.cat, w.unlock_exp);
                        ctx.popups.push(buybox::rank_box(&name));
                    }
                    Some(w) if w.kind_of == Some(0x14) && w.sale_type == 0 => {
                        self.buying = Some(w.clone());
                        ctx.popups.push(ask_buy(w));
                    }
                    Some(w) => {
                        self.buying = Some(w.clone());
                        self.buy_box = BuyBox::open(
                            ctx.cat,
                            &self.wares,
                            &ctx.data.store,
                            w.item_no,
                            w.sale_type,
                        );
                        // CLI-1630: the rank requirement of GP cells too (the window's OK shows the GP form)
                        if let Some(b) = self.buy_box.as_mut() {
                            buybox::set_rank(
                                b,
                                ctx.cat,
                                &self.wares,
                                &ctx.data.store,
                                w.sale_type,
                                lock_exp(ctx.data),
                            );
                        }
                        if self.buy_box.is_none() {
                            ctx.notice(0x3d2);
                        }
                    }
                    None => ctx.notice(0x3d2),
                }
            }
            CMD_WEAR => self.equip_selected(ctx),
            CMD_ACTIVATE if self.pane == Pane::Inbox => self.activate_selected(ctx),
            CMD_CHARGE => ctx.popups.push(buybox::charge_box()),
            CMD_THROW_OUT if self.pane == Pane::Storage => self.discard_selected(ctx),
            slotpick::CMD_BACKPACK if self.pane == Pane::Storage => self.backpack_selected(ctx),
            c => {
                if let Some(cat) = self.category_of(c) {
                    self.character_page = false;
                    self.category = cat;
                    self.page = 0;
                    self.selected = None;
                }
            }
        }
        self.sync_pages(ctx.data);
        self.select_first(ctx.data);
    }

    fn release(&mut self, _p: [f32; 2]) {
        self.figure_drag = None;
    }

    // use-items-emblem: a use window takes the presses before the pane lists' scroll bars (CLI-1884)
    fn over_lists(&self) -> bool {
        self.use_window_open()
    }

    // use-items: the country window's open list scrolls by the wheel (CLI-1873); a use window takes every turn
    fn wheel(&mut self, _p: [f32; 2], notches: i32, _ctx: &mut Ctx) -> bool {
        if let Some(b) = self.country_box.as_mut() {
            b.wheel(notches);
            return true;
        }
        self.use_window_open()
    }

    // name-change: the rename window's field takes the typed characters and Backspace
    fn type_char(&mut self, c: char, _ctx: &mut Ctx) -> bool {
        if let Some(b) = self.mega_box.as_mut() {
            b.type_char(c);
            return true;
        }
        match self.name_box.as_mut() {
            Some(b) => {
                b.type_char(c);
                true
            }
            None => false,
        }
    }

    fn backspace(&mut self, _ctx: &mut Ctx) -> bool {
        if let Some(b) = self.mega_box.as_mut() {
            b.backspace();
            return true;
        }
        match self.name_box.as_mut() {
            Some(b) => {
                b.backspace();
                true
            }
            None => false,
        }
    }

    fn press(&mut self, p: [f32; 2], ctx: &mut Ctx) -> bool {
        // name-change: the rename window takes every press while it is up
        if let Some(b) = self.name_box.as_mut() {
            let step = b.press(p).unwrap_or(NameStep::None);
            self.name_step(step, ctx);
            return true;
        }
        if let Some(b) = self.record_box.as_mut() {
            let step = b.press(p).unwrap_or(RecordStep::None);
            self.record_step(step, ctx);
            return true;
        }
        if let Some(b) = self.country_box.as_mut() {
            let step = b.press(p).unwrap_or(CountryStep::None);
            self.country_step(step, ctx);
            return true;
        }
        if let Some(b) = self.mega_box.as_mut() {
            let step = b.press(p).unwrap_or(MegaStep::None);
            self.mega_step(step, ctx);
            return true;
        }
        if let Some(b) = self.colour_box.as_mut() {
            let step = b.press(p).unwrap_or(ColourStep::None);
            self.colour_step(step, ctx);
            return true;
        }
        if let Some(b) = self.mark_box.as_mut() {
            let step = b.press(p).unwrap_or(MarkStep::None);
            self.mark_step(step, ctx);
            return true;
        }
        if let Some(b) = self.face_box.as_mut() {
            let step = b.press(p).unwrap_or(FaceStep::None);
            self.face_step(step, ctx);
            return true;
        }
        if self.buy_box.is_some() {
            self.buy_press(p, ctx);
            return true;
        }
        if self.slot_pick.is_some() {
            // the window's own controls answer through click / check
            return false;
        }
        if self.xray_press(p, ctx) {
            return true;
        }
        // CLI-1263: a press on the character's part of the pane starts turning it; the press goes on to the controls
        if inside(FIGURE_DRAG_RECT, p) {
            let sx = ctx.cat.size().0 as f32 / dc_ui_data::scale::VIRTUAL_WIDTH as f32;
            self.figure_drag = Some((p[0], sx));
        }
        // CLI-1160 the X of a filled backpack row takes its weapon off (unequip, backpack slot 10 + row)
        if let Some((row, (uid, item))) = self.pack_x_at(ctx.data, p) {
            ctx.send(LinkCmd::Unequip {
                uid,
                item,
                slot: slotpick::FIRST_PACK_SLOT + row as u32,
            });
            return true;
        }
        // CLI-972 an open drawer's buttons lie over the shop page's left edge: they take the press first
        if let Some((row, k)) = self.drawer_button_at(ctx.data, p) {
            self.open_drawer(k, row);
            return true;
        }
        let cells = match self.pane {
            Pane::Shop => &self.shop_cells,
            Pane::Storage => &self.store_cells,
            Pane::Inbox => {
                if let Some(i) = inbox::row_hit(&self.sheet, ctx.data, self.page, p) {
                    self.selected = Some(i);
                    return true;
                }
                let pages = self.pages(ctx.data);
                if let Some(pg) = page_hit(&self.sheet, &PAGE_IDS, self.page, pages, p) {
                    self.page = pg;
                    self.selected = None;
                    self.ask_inbox_page(ctx);
                    self.select_first(ctx.data);
                    return true;
                }
                &self.shop_cells
            }
        };
        if self.pane != Pane::Inbox {
            let n = self.entries(ctx.data);
            if let Some(i) = cells
                .iter()
                .position(|r| inside(*r, p))
                .filter(|i| self.page * CELLS + i < n)
            {
                self.selected = Some(i);
                self.preview_on_double(i, ctx.data);
                self.sync_wear(ctx.data);
                return true;
            }
            if let Some(pg) = page_hit(&self.sheet, &PAGE_IDS, self.page, page_count(n), p) {
                self.page = pg;
                self.selected = None;
                self.select_first(ctx.data);
                return true;
            }
        }
        if let Some((slot, cat, _, _)) = self
            .equip_row_rects()
            .into_iter()
            .find(|(_, _, _, r)| inside(*r, p))
        {
            if self.tab == TAB_BODY && slot == storage::CHARACTER_SLOT {
                self.open_character_storage();
            } else {
                self.open_storage(cat);
            }
            return true;
        }
        false
    }

    fn popup_done(&mut self, p: &Popup, yes: bool, ctx: &mut Ctx) {
        match p.tag {
            BoxTag::UseConfirm { uid } => {
                self.use_confirmed(uid, yes, ctx);
                return;
            }
            // use-items-sweep: the colour window's in-use question (CLI-1877)
            BoxTag::ColourInUse { uid } => {
                let Some(b) = self.colour_box.as_mut().filter(|b| b.uid == uid) else {
                    return;
                };
                let same = b.in_use.is_some_and(|u| u.same_item);
                let step = match colorbox::in_use_answer(same, p.chosen, yes) {
                    Some(true) => b.send(),
                    Some(false) => ColourStep::Cancel,
                    None => ColourStep::None,
                };
                self.colour_step(step, ctx);
                return;
            }
            // EVIDENCE: cshell+0x150333 (the confirmed open sends 0x32e: entry id, product id)
            BoxTag::OpenGift { entry, product } => {
                if yes {
                    let ware = ctx
                        .data
                        .store
                        .inbox
                        .iter()
                        .find(|r| r.entry == entry)
                        .and_then(|r| self.wares.ware(r.product, &ctx.data.store.prices));
                    self.opening = Some(Opening {
                        entry,
                        case: ware.as_ref().is_some_and(|w| w.kind_of == Some(0x14)),
                        item: ware.map_or(0, |w| w.item_no),
                        product,
                    });
                    ctx.send(LinkCmd::InboxOpen { entry, product });
                }
                return;
            }
            // EVIDENCE: cshell+0x150484 (box 0x36b, title and first button 0x160e, second button 0x36a; the first
            // runs command 0x3f8 (cshell+0x13f800), which equips the bought item (+0x559c) through cshell+0x14f180)
            BoxTag::Bought { uid, item } => {
                let mut q = Popup::ask(ctx.cat.text(EQUIP_ASK), BoxTag::EquipBought { uid, item });
                q.title = EQUIP_TITLE;
                q.captions = Some([EQUIP_TITLE, KEEP_TEXT]);
                ctx.popups.push(q);
                return;
            }
            // DC-DEFAULT: CLI-709 Equip sends 0x324 for the bought item into its own slot (the 2009 window-7 slot
            // choice of cshell+0x14f180 / 0x1402f0 is not ported)
            // EVIDENCE: cshell+0x13f7a0 (yes -> command 0x95: cshell+0x142ca0 sends 0x334; no -> 0x96: nothing)
            BoxTag::Discard { uid, item } => {
                if yes {
                    self.discarding = Some(uid);
                    ctx.send(LinkCmd::Discard { uid, item });
                }
                return;
            }
            BoxTag::EquipBought { uid, item } => {
                // storage-functions: a weapon whose slot is filled while the backpack has weapon slots asks in window
                // 7 (cshell+0x14f830 0x3f8 on the weapons tab: cshell+0x148020(0) -> cshell+0x14f180(1)); a type-7 item
                // is equipped at once (cshell+0x14f180 -> cshell+0x14ed50)
                let ask = yes
                    && self.tab == storage::TAB_WEAPONS
                    && self.wares.kind_of(item as i32) != Some(7)
                    && slotpick::pack_check(&ctx.data.store, &self.wares, Some(item)).is_ok();
                if ask {
                    self.open_slot_pick(slotpick::SLOT_WINDOW, uid, item, ctx);
                    return;
                }
                if yes {
                    if let Some(slot) = own_slot(&self.wares, item) {
                        ctx.send(LinkCmd::Equip { uid, item, slot });
                    }
                }
                return;
            }
            _ => {}
        }
        if let (BoxTag::NxShort, true) = (p.tag, yes) {
            ctx.popups.push(buybox::charge_box());
            return;
        }
        if let BoxTag::BuyNx(product) = p.tag {
            if yes {
                if let Some(b) = self.buy_box.take() {
                    if b.offer().product == product {
                        self.request(&b.offer().clone(), b.mode, ctx);
                    } else {
                        self.buy_box = Some(b);
                    }
                }
            }
            return;
        }
        if let (BoxTag::Buy(product), true) = (p.tag, yes) {
            // the credit products' confirmation of the purchase window (its OK runs the rank check first,
            // cshell+0x1d7c80 -> 0x3ea -> cshell+0x1da4a0), else the supply case box
            match self.buy_box.take() {
                Some(mut b) if b.offer().product == product => {
                    match b.rank_check(b.mode) {
                        Some(page) => b.rank_page = Some(page),
                        None => ctx.popups.push(buybox::nx_last_ask(ctx.cat, product)),
                    }
                    self.buy_box = Some(b);
                }
                _ => {
                    self.buy_busy = true;
                    ctx.send(LinkCmd::Buy(product));
                }
            }
        }
    }

    fn dropdown(&self) -> Option<&Dropdown> {
        match &self.buy_box {
            Some(b) => b.drop.as_ref(),
            None => self.slot_pick.as_ref().and_then(|p| p.drop.as_ref()),
        }
    }

    fn dropdown_mut(&mut self) -> Option<&mut Dropdown> {
        match &mut self.buy_box {
            Some(b) => b.drop.as_mut(),
            None => self.slot_pick.as_mut().and_then(|p| p.drop.as_mut()),
        }
    }

    fn close_dropdown(&mut self) {
        if let Some(b) = self.buy_box.as_mut() {
            b.drop = None;
        }
        if let Some(p) = self.slot_pick.as_mut() {
            p.drop = None;
        }
    }

    // EVIDENCE: cshell+0x1dab70 (0x3e9: the picked row -> cshell+0x1d9de0); cshell+0x140130 (a slot row picked)
    fn dropdown_pick(&mut self, _list: &'static str, _value: i32, index: usize, _ctx: &mut Ctx) {
        if let Some(b) = self.buy_box.as_mut() {
            b.pick(index);
        } else if let Some(p) = self.slot_pick.as_mut() {
            p.pick(&mut self.sheet, index);
        }
    }

    // storage-functions: window 7's check buttons (1005 own slot, 1006 backpack)
    // EVIDENCE: cshell+0x14f830 (1005 / 1006 -> +0x5590 = 0 / 1, cshell+0x1402f0)
    fn check(&mut self, cmd: i32, _part: usize, _on: bool, _ctx: &mut Ctx) {
        if let Some(p) = self.slot_pick.as_mut() {
            match cmd {
                slotpick::CMD_OWN_SLOT => p.set_mode(&mut self.sheet, false),
                slotpick::CMD_PACK_SLOT => p.set_mode(&mut self.sheet, true),
                _ => {}
            }
        }
    }

    fn event(&mut self, e: &LinkEvent, ctx: &mut Ctx) {
        match e {
            LinkEvent::Balance { .. }
            | LinkEvent::Grade(_)
            | LinkEvent::CharInfo(_)
            | LinkEvent::Profile(_) => self.refresh(ctx.data),
            LinkEvent::EquipDone { ok: false, unequip } => {
                ctx.error(equip_refused_text(*unequip), BoxTag::None)
            }
            LinkEvent::Bought(r) => {
                // EVIDENCE: cshell+0x150c40 (the reply clears the request flag; a returned unique id already held ->
                // res 0x438 "extended", else 0x439; credit products -> res 0x17b6 with the name)
                self.buy_busy = false;
                let credit = self.buying.as_ref().is_some_and(|w| w.sale_type == 1);
                if r.ok && credit {
                    // CLI-552 amended (playtest report: the NX shown stayed at 10,000 after buying): the 2009
                    // client deducts a credit product's price from the balance it shows, credit for dialog mode 1
                    // (option 0), prepaid otherwise; LG-405's 0x345 after the answer then sets the lobby's own
                    // EVIDENCE: cshell+0x150c40 (credit product ok: SalePrice off +0x108 (credit) or +0x11c (prepaid))
                    // CLI-1875 (owner report 2026-11-04: the NX in game was not the admin dashboard's): the price of
                    // the product asked for, which the lobby charged; it was the cell's (its cheapest period)
                    let (option, price) = self.buying_charge;
                    buybox::deduct(ctx.data, option, price);
                    self.refresh(ctx.data);
                    // CLI-1261: a credit product waits in the inbox, so the held inbox rows are stale (playtest report:
                    // an NX supply crate did not show in the inbox)
                    // EVIDENCE: cshell+0x150c40 (credit purchase ok: shell +0x1bc +0x1a = 1, the inbox flag the inbox
                    // tab tests before it shows the held rows, DISASM cshell+0x1500f6)
                    ctx.data.store.inbox_stale = true;
                    let name = self
                        .buying
                        .as_ref()
                        .map_or(String::new(), |w| w.name.clone());
                    let t = buybox::fill_form(&ctx.cat.text(0x17b6), &[&name]);
                    // audit-function (CLI-1388): the "placed in your inbox" box is titled 0x17d1
                    // EVIDENCE: cshell+0x150c40 (credit ok: box 0x17b6 with the name, title 0x17d1, OK 0x42)
                    let mut q = Popup::ok(t, BoxTag::None);
                    q.title = CREDIT_BOUGHT_TITLE;
                    ctx.popups.push(q);
                } else if r.ok && r.award != -1 {
                    // audit-function (CLI-1386): a GP supply case opened at the purchase shows the award pop-up (no
                    // 0x439 box); the link reads the inventory again. Any other reply naming an award shows nothing.
                    // EVIDENCE: cshell+0x150c40 (+0x1e != -1: the bought ware's type +0x23c 0x14 -> the award pop-up
                    // cshell+0x2411b0 with +0x1e, +0xe, +0x12 and the record added; then straight to the end)
                    if self
                        .buying
                        .as_ref()
                        .is_some_and(|w| w.kind_of == Some(0x14))
                    {
                        let mut q = Popup::ok(self.wares.label_of(r.award), BoxTag::None);
                        q.title = CASE_AWARD_TITLE;
                        ctx.popups.push(q);
                    }
                } else if r.ok
                    && r.uid != 0
                    && ctx.data.store.items.iter().any(|i| i.uid == r.uid)
                    && !self
                        .buying
                        .as_ref()
                        .is_some_and(|w| w.kind_of == Some(usebox::FUNCTION_TYPE))
                {
                    // audit-function (CLI-1387): an owned function item bought again (a counted key or pass) takes
                    // the new-item path below: box 0x439 and its use window, never the "extended" box
                    // EVIDENCE: cshell+0x150c40 (unique id already held and type 0x15 -> past box 0x438 to box
                    // 0x439 and cshell+0x1db770(1, unique id, 0))
                    // CLI-912: the extension's success box shows the OK icon too
                    let mut q = Popup::ok(ctx.cat.text(0x438), BoxTag::None);
                    q.ok_icon = true;
                    ctx.popups.push(q);
                } else if r.ok {
                    // EVIDENCE: cshell+0x150c40 (a new item: box 0x439 titled 0x15f5; its OK runs command 0x3f7
                    // through cshell+0x13f820, which asks 0x36b "equip now or keep in storage" (cshell+0x150484))
                    let item = self.buying.as_ref().map_or(0, |w| w.item_no.max(0) as u32);
                    let tag = if r.uid != 0 && own_slot(&self.wares, item).is_some() {
                        BoxTag::Bought { uid: r.uid, item }
                    } else {
                        BoxTag::None
                    };
                    let mut q = Popup::ok(ctx.cat.text(0x439), tag);
                    q.title = BOUGHT_TITLE;
                    // CLI-912: a purchase success shows the OK icon
                    q.ok_icon = true;
                    ctx.popups.push(q);
                    // name-change: a bought function item opens its use window with the box
                    // EVIDENCE: cshell+0x150c40 (type 0x15: box 0x439, then the item-use manager cshell+0x1db770(1,
                    // unique id, 0) with the screen as owner)
                    if r.uid != 0 && self.wares.kind_of(item as i32) == Some(usebox::FUNCTION_TYPE)
                    {
                        self.use_function(r.uid, item as i32, ctx);
                    }
                } else {
                    ctx.popups
                        .push(self.refusal_box(ctx.cat, r.reason, credit, ctx.data));
                }
                self.buying = None;
            }
            // EVIDENCE: cshell+0x14bbf0 (ok: the record leaves the inventory, the page is rebuilt and the selection
            // cleared; cshell+0x32ee0 drops a failed reply)
            LinkEvent::NameChecked(free) => {
                if let Some(b) = self.name_box.as_mut() {
                    b.checked_reply(*free);
                }
            }
            // use-items-emblem: the clan mark window's check (CLI-1884)
            // EVIDENCE: cshell+0x2ab3a0 (ok: box 0x1804 titled 0x1812, OK on; refused: the reason box)
            LinkEvent::MarkChecked { ok, reason } => {
                if let Some(b) = self.mark_box.as_mut() {
                    b.checked_reply(*ok);
                    ctx.popups.push(if *ok {
                        markbox::checked_box(ctx.cat)
                    } else {
                        usebox::reason_box(ctx.cat, *reason)
                    });
                }
            }
            LinkEvent::ItemUsed(r) => self.item_used(r, ctx),
            // use-items: the composer's queue question and send (CLI-1874)
            LinkEvent::MegaphoneQueued(r) => {
                if let Some(b) = self.mega_box.as_mut() {
                    if !b.queued(r) {
                        ctx.popups.push(usebox::reason_box(ctx.cat, r.reason));
                    }
                }
            }
            LinkEvent::MegaphoneSent(r) => self.megaphone_sent(r, ctx),
            // function-unequip: a refused slot answer shows its box; an ok one was applied by the store
            // EVIDENCE: cshell+0x14cea0 / 0x14d010 (see [`FUNCTION_PUT_REFUSED`])
            LinkEvent::FunctionSlotDone { put, ok: false, .. } => {
                let id = if *put {
                    FUNCTION_PUT_REFUSED
                } else {
                    FUNCTION_TAKE_REFUSED
                };
                let mut q = Popup::ok(ctx.cat.text(id), BoxTag::None);
                q.title = text_ids::TITLE_NOTICE;
                ctx.popups.push(q);
            }
            // use-windows: the clan record window's clan info answer
            // EVIDENCE: cshell+0x2428e0 (the answer's wins / losses / draws into str_CurrentRecord)
            LinkEvent::ClanInfo(v) => {
                if let Some(b) = self.record_box.as_mut() {
                    b.clan_answer(v.wins, v.losses, v.draws);
                }
            }
            LinkEvent::Discarded { ok } => {
                if let (true, Some(uid)) = (*ok, self.discarding.take()) {
                    ctx.data.store.remove(uid);
                    self.selected = None;
                    let n = self.entries(ctx.data);
                    if self.page >= page_count(n) {
                        self.page = 0;
                    }
                }
                self.discarding = None;
            }
            // EVIDENCE: cshell+0x14e010 (the 0x32f handler: a refusal 0x8b / 0x8c deletes the row, others show the
            // reason box; an ok answer shows the award pop-up, the row leaves the inbox, the used key is counted
            // down), HISEC_2009.md 4.3
            LinkEvent::InboxOpened(r) => {
                let opened = self.opening.take();
                if let Some(o) = opened.filter(|_| r.ok || matches!(r.reason, 0x8b | 0x8c)) {
                    let st = &mut ctx.data.store;
                    st.inbox.retain(|x| x.entry != o.entry);
                    st.inbox_total = st.inbox_total.saturating_sub(1);
                }
                // CLI-1345: only a supply case's answer names an award (+0x12); a product's answer carries its item
                // count at +0xe and the new unique ids from +0x22, and +0x12 is 0 (item 0 is the M9 knife, the name
                // every opening showed). 2009 shows no pop-up for a product: the items join the inventory and the
                // screen moves to the storage page of the first one with it selected.
                // EVIDENCE: cshell+0x14e010 (the case branch only when the row's product type +0x22c is 0x14; the
                // product branch adds each item via cshell+0x22c790 and calls cshell+0x14c7e0 with its unique id),
                // cshell+0x14c7e0 ("GotoInven_ItemUID": the item's tab and category, the page holding the unique id,
                // the cell selected)
                let product = opened.filter(|o| !o.case);
                // CLI-1433: an opened product's bonus GP (`BonusGP`, a mercenary's +10,000 / +50,000) is added to
                // the shown GP at once; the lobby adds it to the account on the same opening (CD-72)
                // EVIDENCE: cshell+0x14e010 (product branch: GP +0x104 += the product's +0x1c, the money panel
                // refreshed through cshell+0x1485b0)
                if let (true, Some(o)) = (r.ok, product) {
                    let bonus = self
                        .wares
                        .catalog()
                        .and_then(|c| c.product(o.product as i32))
                        .map_or(0, |p| p.bonus.max(0) as u32);
                    if bonus > 0 {
                        ctx.data.profile.gp = ctx.data.profile.gp.saturating_add(bonus);
                        self.refresh(ctx.data);
                    }
                }
                let option = product.filter(|o| {
                    self.wares
                        .kind_of(o.item)
                        .is_some_and(|k| (0x11..=0x13).contains(&k))
                });
                if let (true, Some(_)) = (r.ok, option) {
                    // CLI-1430: an opened option item goes to the Modification screen, whose option storage shows
                    // it picked
                    // EVIDENCE: cshell+0x14c7e0 (an item of the options tab: the screen changes to 0x14 and
                    // cshell+0x1596f0 runs with its unique id), cshell+0x1596f0 (not on screen 0x14: the unique id is
                    // kept at +0x5624), cshell+0x159a99 (the Modification screen's entry runs cshell+0x1596f0 with the
                    // kept unique id and clears it)
                    ctx.data.store.tuning_goto = Some(r.award_uid).filter(|&u| u != 0);
                    ctx.send(LinkCmd::Inventory);
                    *ctx.go = Some(Go::Tuning);
                } else if let (true, Some(o)) = (r.ok, product) {
                    self.goto_storage_of(o.item);
                    self.goto_uid = Some(r.award_uid).filter(|&u| u != 0);
                    ctx.send(LinkCmd::Inventory);
                    // name-change: an opened function item opens its use window on the black market tab
                    // EVIDENCE: cshell+0x14e010 (an added item the use manager takes (cshell+0x1db0b0: type 0x15):
                    // command 0x3ea, then cshell+0x1db770(1, unique id, 0) when the reply holds one item)
                    if r.award_uid != 0 && self.wares.kind_of(o.item) == Some(usebox::FUNCTION_TYPE)
                    {
                        self.use_function(r.award_uid, o.item, ctx);
                    }
                } else if r.ok {
                    let name = self.wares.label_of(r.award_item as i32);
                    let mut q = Popup::ok(name, BoxTag::None);
                    let consume = opened
                        .and_then(|o| self.wares.catalog()?.product(o.product as i32))
                        .map(|p| p.consume);
                    q.title = award_title(r.key_uid != 0, consume);
                    // crate-result-window: the 2009 award window (picture, name, class tag, "<name> / <period>",
                    // message, OK after one second); the plain box above stays the fallback without the ROM literals
                    // EVIDENCE: cshell+0x14e010 (cshell+0x1412c0 with the award item, its type, +0x22, +0x1a, +0x16,
                    // the key flag and the crate flag), cshell+0x286d40
                    q.award = super::award::AwardBox::build(
                        &ctx.cat.paths.award,
                        &self.wares,
                        r.award_item as i32,
                        r.days,
                        r.gp,
                        r.key_uid != 0,
                        consume == Some(6),
                        std::time::Instant::now(),
                    );
                    ctx.popups.push(q);
                    ctx.send(LinkCmd::Inventory);
                } else if !matches!(r.reason, 0x8b | 0x8c) {
                    ctx.notice(0x503);
                }
                self.selected = None;
                if self.page >= self.pages(ctx.data).max(1) {
                    self.page = 0;
                }
            }
            LinkEvent::Inventory(_) | LinkEvent::Equipment(_) => {
                if let Some(uid) = self.goto_uid {
                    if let Some(k) = self.items_shown(ctx.data).iter().position(|i| i.uid == uid) {
                        self.goto_uid = None;
                        if self.pane == Pane::Storage {
                            self.page = k / CELLS;
                            self.selected = Some(k % CELLS);
                        }
                    }
                }
                let n = self.entries(ctx.data);
                if self.page >= page_count(n) {
                    self.page = 0;
                }
                if self.selected.is_some_and(|i| self.page * CELLS + i >= n) {
                    self.selected = None;
                }
            }
            _ => {}
        }
        self.sync_pages(ctx.data);
        self.select_first(ctx.data);
    }

    fn draw_extra(&self, cat: &mut Catalog, data: &MenuData, secs: f32, out: &mut Vec<DrawItem>) {
        // the left pane's equipment rows and the weapons tab's backpack row
        let backpack = self
            .sheet
            .part(LEFT_WINDOW, pane::BACKPACK_LIST, WidgetKind::List)
            .map(|l| l.pos);
        pane::draw_rows(
            cat,
            self.tab == storage::TAB_WEAPONS,
            &self.equip_row_rects(),
            backpack,
            &self.wares,
            &data.store,
            &self.look,
            out,
        );
        if self.tab == TAB_MARKET && self.pane != Pane::Inbox {
            if let Some(l) = self
                .sheet
                .part(LEFT_WINDOW, pane::MARKET_LIST, WidgetKind::List)
            {
                pane::draw_market_rows(cat, l, &self.wares, &data.store, out);
            }
        }
        // the nickname on its button
        if let Some(b) = self.sheet.button(NICK_BUTTON_CMD) {
            out.push(place_text(
                &data.profile.nick,
                b.pos,
                b.size,
                CENTRE,
                14,
                13,
                ROW_COLOUR,
            ));
        }
        // CLI-1628: the x-ray style cycler and colour pair right of the nickname
        if let Some(r) = self.xray_rects(data) {
            self.xray_controls.draw(
                cat,
                &self.buy_art,
                &r,
                &data.settings.xray,
                self.pointer,
                out,
            );
        }
        match self.pane {
            Pane::Shop => {
                let shown = self.wares_shown(data);
                let rank = self.art.as_ref().map(|a| CellRank {
                    levels: &a.levels,
                    exp: lock_exp(data),
                });
                for (i, r) in self.shop_cells.iter().enumerate() {
                    match shown.get(self.page * CELLS + i) {
                        Some(w) => draw_ware_cell(
                            cat,
                            *r,
                            w,
                            self.selected == Some(i),
                            SHOP_FRAMES,
                            CELL_PRICE_AT,
                            Some(MARK_AT),
                            rank.as_ref(),
                            out,
                        ),
                        // an empty cell keeps its frame
                        // OBSERVED: shop-guns/cpp/ (empty cells of a short page drawn framed)
                        None => {
                            out.push(image_fit(cat, SHOP_FRAMES[0], [r[0], r[1]], [r[2], r[3]]))
                        }
                    }
                }
                if let Some(w) = self.selected.and_then(|i| shown.get(self.page * CELLS + i)) {
                    let locked = locked(w.unlock_exp, lock_exp(data));
                    draw_detail(
                        cat,
                        w,
                        self.wares.art_of(w.item_no),
                        DetailMarks {
                            locked,
                            temporary: false,
                        },
                        out,
                    );
                }
            }
            Pane::Storage => {
                let items = self.items_shown(data);
                for (i, r) in self.store_cells.iter().enumerate() {
                    // DC-DEFAULT: LG-52 (playtest report: "none of body storage appears") the storage cells show the
                    // item's slot image, class tag, name and the Equipped mark of an item in use
                    // CLI-1264 (user 2026-10-29: the storage page's own 2009 cell frames): the tab-less
                    // Inven_Item_Slot frames at the storage cell's 0xfe x 0x56, _On while the cell is selected or
                    // under the pointer (supersedes CLI-1262's shop frames)
                    let lit = self.selected == Some(i)
                        || self.pointer.is_some_and(|p| {
                            self.buy_box.is_none() && self.slot_pick.is_none() && inside_cell(*r, p)
                        });
                    out.push(image_fit(
                        cat,
                        STORAGE_FRAMES[lit as usize],
                        [r[0], r[1]],
                        [r[2], r[3]],
                    ));
                    if let Some(it) = items.get(self.page * CELLS + i) {
                        // function-unequip: a function item's mark follows the function list (CLI-1395)
                        // EVIDENCE: cshell+0x14a700 (type 0x15: Equiped_Mark when cshell+0x226800 finds the unique id)
                        let equipped = if (9000..10000).contains(&it.item_no) {
                            self.wares.in_function_list(&data.store, it)
                        } else {
                            data.store.slot_of(it).is_some()
                        };
                        if let Some(icon) = self
                            .wares
                            .icons_of(it.item_no as i32)
                            .into_iter()
                            .find(|t| cat.tex_size(t).is_some())
                        {
                            out.push(image_fit(
                                cat,
                                &icon,
                                [r[0], r[1]],
                                [CELL_IMAGE_SIZE[0].min(r[2]), CELL_IMAGE_SIZE[1].min(r[3])],
                            ));
                        }
                        let tag = self.wares.kind_of(it.item_no as i32).and_then(cell_tag);
                        let places = storage_mark_places(cat, *r, tag);
                        if let Some((t, size)) = tag {
                            out.push(image_fit(cat, t, places.tag, size));
                        }
                        if equipped {
                            let (t, size) = super::tuning::EQUIPPED_MARK;
                            out.push(image_fit(cat, t, places.equipped, size));
                        }
                        // CLI-1000: the period of the owned item under the class tag (else under the name): the
                        // PERMANENT mark of a record without an end date; CLI-1264 the days / hours left of a
                        // temporary one as a label in its place; CLI-1879 a counted record (no end, uses left) is not
                        // permanent: its uses left as the label
                        // EVIDENCE: cshell+0x14a700 (Permenant_Mark shown for a record without an end and without a
                        // count (+0x14 = 0x321 +0x24); no period text in the cell); places: `storage_mark_places`
                        let now = inbox::now_filetime();
                        let at = places.period;
                        if inbox::has_end(it.end, now) {
                            out.push(period_label(
                                cat,
                                at,
                                &cell_period_text(cat, it, now),
                                &cell_period_short(it, now),
                            ));
                        } else if it.count > 0 {
                            out.push(period_label(
                                cat,
                                at,
                                &uses_left_text(cat, it.count),
                                &uses_left_short(it.count),
                            ));
                        } else {
                            out.push(image_fit(cat, PERMANENT_MARK.0, at, PERMANENT_MARK.1));
                        }
                        draw_cell(
                            *r,
                            &self.wares.label_of(it.item_no as i32),
                            None,
                            false,
                            // CLI-1002: 2009 names every storage cell in 0xffa3c3c3, an item in use is told by its
                            // Equipped mark (cshell+0x14a700: the name text is set with 0xffa3c3c3 for all)
                            CELL_COLOUR,
                            out,
                        );
                    }
                }
                // the selected cell's detail panel, as the shop's, with the owned item's period
                // CLI-942: the storage page drew no panel for the selected cell
                if let Some(it) = self.selected.and_then(|i| items.get(self.page * CELLS + i)) {
                    let w = self.wares.item_ware(it.item_no as i32);
                    draw_detail(
                        cat,
                        &w,
                        self.wares.art_of(w.item_no),
                        DetailMarks {
                            locked: false,
                            temporary: false,
                        },
                        out,
                    );
                    let function =
                        self.wares.kind_of(it.item_no as i32) == Some(usebox::FUNCTION_TYPE);
                    let line = owned_use_text(cat, it, function, inbox::now_filetime());
                    draw_period_line(&line, out);
                }
            }
            Pane::Inbox => {
                let (sheet, wares) = (&self.sheet, &self.wares);
                inbox::draw(cat, sheet, wares, data, self.page, self.selected, out);
                inbox::draw_items(cat, sheet, wares, data, out);
            }
        }
        draw_page_numbers(&self.sheet, &PAGE_IDS, self.page, self.pages(data), out);
        if let Some(b) = &self.buy_box {
            b.draw(cat, &self.buy_art, data, out);
        }
        if let Some(b) = &self.name_box {
            b.draw(cat, &self.name_art, out);
        }
        if let Some(b) = &self.record_box {
            b.draw(cat, &self.name_art, out);
        }
        if let Some(b) = &self.country_box {
            b.draw(cat, &self.country_art, &self.name_art, out);
        }
        if let Some(b) = &self.mega_box {
            b.draw(cat, &self.mega_art, &self.name_art, out);
        }
        if let Some(b) = &self.colour_box {
            b.draw(cat, &self.colour_art, &self.name_art, out);
        }
        if let Some(b) = &self.mark_box {
            b.draw(cat, &self.name_art, out);
        }
        if let Some(b) = &self.face_box {
            b.draw(cat, &self.face_art, &self.name_art, out);
        }
        // storage-functions: the slot choice window over the pane rows and the cells
        if let Some(p) = &self.slot_pick {
            self.sheet.draw_layer(p.window, cat, secs, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CLI-1879: a counted function item reads its uses (in use) or its usage (not yet used); a function item without
    /// a count keeps the remaining-use line; days for a period item not yet used.
    #[test]
    fn owned_use_text_reads_uses_left_and_usage() {
        let cat = Catalog::fallback(None, 1024, 768);
        let rec = |in_use, use_kind, count, period| super::super::link::InvRecord {
            uid: 1,
            item_no: 9019,
            in_use,
            use_kind,
            count,
            period,
            ..Default::default()
        };
        let now = inbox::now_filetime();
        let t = |r| owned_use_text(&cat, &r, true, now);
        assert_eq!(t(rec(true, 3, 100, 0)), cat.text(0x18e7));
        assert_eq!(
            t(rec(false, 3, 100, 0)),
            format!("{}100 items", cat.text(0x18f9))
        );
        assert_eq!(
            t(rec(false, 1, 0, 7)),
            format!("{}7 days", cat.text(0x18f8))
        );
        assert_eq!(
            t(rec(false, 1, 0, 0)),
            format!("{}One Time Use", cat.text(0x18f8))
        );
        assert_eq!(
            t(rec(true, 0, 0, 0)),
            remaining_use_text(&cat, &rec(true, 0, 0, 0), now)
        );
        assert_eq!(
            owned_use_text(&cat, &rec(true, 3, 100, 0), false, now),
            remaining_use_text(&cat, &rec(true, 3, 100, 0), now)
        );
        assert_eq!(
            (uses_left_short(1), uses_left_short(42)),
            ("1 use".to_string(), "42 uses".to_string())
        );
    }

    #[test]
    fn purchase_answers_pick_the_2009_boxes() {
        assert_eq!(bought_text(true, 0, false), 0x439);
        assert_eq!(bought_text(false, 0x3b, false), 0x46e);
        assert_eq!(bought_text(false, 0x3b, true), 0x1873);
        assert_eq!(bought_text(false, 0x3c, false), 0x503);
        assert_eq!(equip_refused_text(false), 0x506);
        assert_eq!(equip_refused_text(true), 0x4dd);
    }

    /// CLI-1431: a refusal outside 0x3b / 0x3c takes the shared reason table (0x60 not for sale -> 0x17f9), not the
    /// inventory-full box; short of credit is the box with Charge NX.
    #[test]
    fn refused_purchases_take_the_2009_reason_boxes() {
        let cat = Catalog::fallback(None, 1024, 768);
        let t = |r, c| refused_box(&cat, r, c);
        assert_eq!(t(0x60, false).text, cat.text(0x17f9));
        assert_eq!(t(0x3c, false).text, cat.text(0x503));
        assert_eq!(t(0x3b, false).text, cat.text(0x46e));
        assert_eq!(t(0x3b, true).title, buybox::SHORT_NX_TITLE);
    }

    #[test]
    fn pages_hold_six_cells() {
        assert_eq!(page_count(0), 1);
        assert_eq!(page_count(6), 1);
        assert_eq!(page_count(7), 2);
    }

    #[test]
    fn function_lines_split_into_colour_runs() {
        assert_eq!(
            colour_runs("Restriction:", 7),
            vec![("Restriction:".to_string(), 7)]
        );
        assert_eq!(
            colour_runs("^0xFFFFFFFF^Only usable^ now", 7),
            vec![
                ("Only usable".to_string(), 0xffff_ffff),
                (" now".to_string(), 7)
            ]
        );
    }

    #[test]
    fn prices_read_with_their_unit() {
        let w = Ware {
            product: 1,
            name: "x".into(),
            item_no: 1,
            kind_of: Some(0),
            tag_kind: Some(0),
            sale_type: 0,
            price: 10000,
            icon: None,
            display: 1,
            unlock_exp: 0,
            marks: [false; 3],
            period: 30,
            consume: 0,
        };
        assert_eq!(price_text(&w), "10,000 GP");
        assert_eq!(price_text(&Ware { sale_type: 1, ..w }), "10,000 NX");
    }
}

#[cfg(test)]
mod crate_title_tests {
    use super::*;

    // CLI-1390: cshell+0x286d40 picks the title from the key flag, then the crate flag
    #[test]
    fn award_titles_follow_the_key_and_crate_flags() {
        assert_eq!(award_title(true, Some(6)), HISEC_AWARD_TITLE);
        assert_eq!(award_title(false, Some(6)), 0x18f7);
        assert_eq!(award_title(false, Some(2)), CASE_AWARD_TITLE);
        assert_eq!(award_title(false, None), CASE_AWARD_TITLE);
    }

    /// use-items (CLI-1873 / CLI-1874): the ROM's country code item and megaphone open their windows and their
    /// requests and answers go through (ROM only).
    #[test]
    fn rom_country_change_and_megaphone_flow() {
        let Ok(rom) = dc_rom::resolve_rom() else {
            return;
        };
        let cat = Catalog::from_rom(&rom, 1024, 768).unwrap();
        let mut data = MenuData::default();
        let mut s = ShopScreen::new(&cat, &data, Go::Market);
        let of_kind = |k: i32| {
            (9000..10000).find(|&n| {
                s.wares.kind_of(n) == Some(usebox::FUNCTION_TYPE)
                    && s.wares.art_of(n).is_some_and(|a| a.sub_type == k)
            })
        };
        let (Some(country), Some(mega)) =
            (of_kind(usebox::SUB_COUNTRY), of_kind(usebox::SUB_MEGAPHONE))
        else {
            panic!("the ROM has a country code item and a megaphone");
        };
        data.char_extras.nation = 2;
        data.profile.nick = "Alice".into();
        data.store.items = vec![
            super::super::link::InvRecord {
                uid: 50,
                item_no: country as u32,
                ..Default::default()
            },
            super::super::link::InvRecord {
                uid: 60,
                item_no: mega as u32,
                count: 3,
                use_kind: 3,
                ..Default::default()
            },
        ];
        let link = super::super::link::Link::detached();
        let (mut popups, mut go, mut quit, mut sounds) = (Vec::new(), None, false, Vec::new());
        let mut ctx = Ctx {
            cat: &cat,
            data: &mut data,
            link: &link,
            popups: &mut popups,
            go: &mut go,
            quit: &mut quit,
            sounds: &mut sounds,
        };
        let sent = |link: &super::super::link::Link| {
            link.take_sent()
                .iter()
                .map(|c| format!("{c:?}"))
                .collect::<Vec<_>>()
        };
        // the country window: pick nation 5, confirm, send; the ok answer sets the own nation
        s.use_function(50, country, &mut ctx);
        let b = s.country_box.as_mut().expect("the country window opens");
        assert_eq!(b.current, 2);
        assert!(b.list_len() > 5);
        b.pick(4);
        assert_eq!(b.ok(), CountryStep::None);
        let step = b.ok();
        s.country_step(step, &mut ctx);
        assert_eq!(sent(&link), vec!["ItemUse(50, \"\", 0, 5)".to_string()]);
        s.item_used(
            &super::super::link::UseReply {
                ok: true,
                uid: 50,
                ..Default::default()
            },
            &mut ctx,
        );
        assert!(s.country_box.is_none());
        assert_eq!(
            (ctx.data.char_extras.nation, ctx.data.profile.nation),
            (5, 5)
        );
        assert!(ctx.data.store.items.iter().all(|i| i.uid != 50));
        // the megaphone: the first use, then the composer, the queue question, the confirmation and the send
        s.use_function(60, mega, &mut ctx);
        assert_eq!(sent(&link), vec!["ItemUse(60, \"\", 0, 0)".to_string()]);
        assert!(s.mega_box.is_none());
        s.item_used(
            &super::super::link::UseReply {
                ok: true,
                uid: 60,
                count: 3,
                ..Default::default()
            },
            &mut ctx,
        );
        let b = s.mega_box.as_mut().expect("the composer opens");
        assert_eq!((b.uses, b.name.as_str()), (3, "Alice"));
        for c in "hi all".chars() {
            b.type_char(c);
        }
        let step = b.ok();
        s.mega_step(step, &mut ctx);
        assert_eq!(sent(&link), vec!["MegaphoneQueue".to_string()]);
        let q = super::super::link::QueueReply {
            ok: true,
            secs: 30,
            on_air: 1,
            last: 2,
            waiting: 1,
            ..Default::default()
        };
        s.event(&LinkEvent::MegaphoneQueued(q), &mut ctx);
        let b = s.mega_box.as_mut().unwrap();
        assert!(b.confirm.is_some());
        let step = b.ok();
        s.mega_step(step, &mut ctx);
        assert_eq!(
            sent(&link),
            vec!["MegaphoneSend(60, \"hi all\")".to_string()]
        );
        let r = super::super::link::SendReply {
            ok: true,
            reason: 0,
            uid: 60,
            left: 2,
        };
        s.event(&LinkEvent::MegaphoneSent(r), &mut ctx);
        assert!(s.mega_box.is_none());
        let rec = ctx.data.store.items.iter().find(|i| i.uid == 60).unwrap();
        assert_eq!((rec.in_use, rec.count), (true, 2));
        // in use now: the composer opens at once; a refused queue question shows the reason box
        s.use_function(60, mega, &mut ctx);
        assert!(sent(&link).is_empty());
        let b = s
            .mega_box
            .as_mut()
            .expect("the composer opens for an item in use");
        b.text = "again".into();
        let step = b.ok();
        s.mega_step(step, &mut ctx);
        let before = ctx.popups.len();
        let refused = super::super::link::QueueReply {
            ok: false,
            reason: 0x8f,
            ..Default::default()
        };
        s.event(&LinkEvent::MegaphoneQueued(refused), &mut ctx);
        assert_eq!(ctx.popups.len(), before + 1);
        assert!(s.mega_box.as_ref().is_some_and(|b| b.confirm.is_none()));
    }

    /// use-items-sweep (CLI-1876 .. CLI-1878): the ROM's name colour item and two custom reticles open the colour
    /// window, their requests carry the colour and the replaced reticle, and the match gets the reticle in use (ROM
    /// only).
    #[test]
    fn rom_colour_and_reticle_flow() {
        let Ok(rom) = dc_rom::resolve_rom() else {
            return;
        };
        let cat = Catalog::from_rom(&rom, 1024, 768).unwrap();
        let mut data = MenuData::default();
        let mut s = ShopScreen::new(&cat, &data, Go::Market);
        let of_kind = |k: i32| -> Vec<i32> {
            (9000..10000)
                .filter(|&n| {
                    s.wares.kind_of(n) == Some(usebox::FUNCTION_TYPE)
                        && s.wares.art_of(n).is_some_and(|a| a.sub_type == k)
                })
                .collect()
        };
        let names = of_kind(colorbox::SUB_NAME_COLOUR);
        let reticles = of_kind(colorbox::SUB_RETICLE);
        assert!(
            !names.is_empty() && reticles.len() >= 2,
            "the ROM has colour and reticle items"
        );
        let (r1, r2) = (reticles[0], reticles[1]);
        data.profile.nick = "Alice".into();
        let rec = |uid: u32, item: i32| super::super::link::InvRecord {
            uid,
            item_no: item as u32,
            ..Default::default()
        };
        data.store.items = vec![rec(70, names[0]), rec(80, r1), rec(90, r2)];
        let link = super::super::link::Link::detached();
        let (mut popups, mut go, mut quit, mut sounds) = (Vec::new(), None, false, Vec::new());
        let mut ctx = Ctx {
            cat: &cat,
            data: &mut data,
            link: &link,
            popups: &mut popups,
            go: &mut go,
            quit: &mut quit,
            sounds: &mut sounds,
        };
        let sent = |link: &super::super::link::Link| {
            link.take_sent()
                .iter()
                .map(|c| format!("{c:?}"))
                .collect::<Vec<_>>()
        };
        let used = |s: &mut ShopScreen, uid: u32, ctx: &mut Ctx| {
            s.item_used(
                &super::super::link::UseReply {
                    ok: true,
                    uid,
                    ..Default::default()
                },
                ctx,
            )
        };
        // the name colour: sixteen swatches on one page, a pick, OK sends at once
        s.use_function(70, names[0], &mut ctx);
        let b = s.colour_box.as_mut().expect("the colour window opens");
        assert_eq!(
            (b.swatches.len(), b.pages(), b.name.as_str()),
            (16, 1, "Alice")
        );
        b.pick(10);
        let step = b.ok();
        s.colour_step(step, &mut ctx);
        assert_eq!(sent(&link), vec!["ColourUse(70, 10, 0)".to_string()]);
        used(&mut s, 70, &mut ctx);
        assert!(s.colour_box.is_none());
        assert_eq!(ctx.data.char_extras.name_colour, 10);
        // the first reticle: eight swatches, the confirmation, the send
        s.use_function(80, r1, &mut ctx);
        let b = s.colour_box.as_mut().expect("the reticle window opens");
        assert_eq!(b.swatches.len(), 8);
        assert!(!b.after.is_empty() && !b.before.0.is_empty());
        b.pick(3);
        assert_eq!(b.ok(), ColourStep::None);
        let step = b.ok();
        s.colour_step(step, &mut ctx);
        assert_eq!(sent(&link), vec!["ColourUse(80, 3, 0)".to_string()]);
        used(&mut s, 80, &mut ctx);
        let r = ctx.data.store.items.iter().find(|i| i.uid == 80).unwrap();
        assert_eq!((r.in_use, r.function), (true, 3));
        // the match draws it: its shape and table colour
        let (shape, argb) =
            colorbox::match_reticle(Some(&rom), &ctx.data.store).expect("a reticle");
        let shape_type = s.wares.art_of(r1).unwrap().shape;
        assert_eq!(shape, crate::spread::mark::shape_of(shape_type));
        assert_eq!(argb, s.colour_tables.reticle_argb(3).unwrap());
        // the second reticle: the question names the one in use, its OK leaves the window for a pick that replaces it
        let n = ctx.popups.len();
        s.use_function(90, r2, &mut ctx);
        assert_eq!(ctx.popups.len(), n + 1);
        let q = ctx.popups.pop().unwrap();
        assert_eq!(q.tag, BoxTag::ColourInUse { uid: 90 });
        s.popup_done(&q, true, &mut ctx);
        let b = s.colour_box.as_mut().expect("the window stays");
        assert_eq!(b.before.1, s.colour_tables.reticle_argb(3).unwrap());
        b.pick(6);
        b.ok();
        let step = b.ok();
        s.colour_step(step, &mut ctx);
        assert_eq!(sent(&link), vec!["ColourUse(90, 6, 80)".to_string()]);
        s.item_used(
            &super::super::link::UseReply {
                ok: true,
                uid: 90,
                removed: 80,
                ..Default::default()
            },
            &mut ctx,
        );
        assert!(ctx.data.store.items.iter().all(|i| i.uid != 80));
        let (_, argb) = colorbox::match_reticle(Some(&rom), &ctx.data.store).unwrap();
        assert_eq!(argb, s.colour_tables.reticle_argb(6).unwrap());
    }

    /// use-items-emblem (CLI-1884 / CLI-1885): the ROM's clan emblem item opens the mark window on the ROM mark table,
    /// its check and use carry the picked pair and the answer makes it the clan's mark; the base character and gender
    /// items list the ROM creation table's rows of the own / the other gender and their use sets the own face (ROM
    /// only).
    #[test]
    fn rom_mark_and_face_flow() {
        let Ok(rom) = dc_rom::resolve_rom() else {
            return;
        };
        let cat = Catalog::from_rom(&rom, 1024, 768).unwrap();
        let mut data = MenuData::default();
        let mut s = ShopScreen::new(&cat, &data, Go::Market);
        let of_kind = |k: i32| -> Option<i32> {
            (9000..10000).find(|&n| {
                s.wares.kind_of(n) == Some(usebox::FUNCTION_TYPE)
                    && s.wares.art_of(n).is_some_and(|a| a.sub_type == k)
            })
        };
        let (mark, face, gender) = (
            of_kind(markbox::SUB_CLAN_MARK).expect("the ROM has a clan emblem item"),
            of_kind(facebox::SUB_FACE).expect("the ROM has a base character item"),
            of_kind(facebox::SUB_GENDER).expect("the ROM has a gender item"),
        );
        let rec = |uid: u32, item: i32| super::super::link::InvRecord {
            uid,
            item_no: item as u32,
            ..Default::default()
        };
        data.store.items = vec![rec(71, mark), rec(72, face), rec(73, gender)];
        data.profile.face = 6002;
        let link = super::super::link::Link::detached();
        let (mut popups, mut go, mut quit, mut sounds) = (Vec::new(), None, false, Vec::new());
        let mut ctx = Ctx {
            cat: &cat,
            data: &mut data,
            link: &link,
            popups: &mut popups,
            go: &mut go,
            quit: &mut quit,
            sounds: &mut sounds,
        };
        let sent = |link: &super::super::link::Link| {
            link.take_sent()
                .iter()
                .map(|c| format!("{c:?}"))
                .collect::<Vec<_>>()
        };
        let used = |s: &mut ShopScreen, uid: u32, ctx: &mut Ctx| {
            s.item_used(
                &super::super::link::UseReply {
                    ok: true,
                    uid,
                    ..Default::default()
                },
                ctx,
            )
        };
        // the clan mark: every tab of both steps lists entries of the ROM table
        s.use_function(71, mark, &mut ctx);
        let b = s.mark_box.as_mut().expect("the mark window opens");
        assert_eq!(b.layout.window_at(), [229, 164]);
        for step in 0..2 {
            b.step = step;
            for tab in 0..markbox::TABS {
                b.choose_tab(tab);
                assert!(!b.entries().is_empty(), "step {step} tab {tab}");
            }
        }
        b.step = 0;
        b.choose_tab(0);
        let e = b.entries()[5];
        b.pick(e);
        assert_eq!(b.command(markbox::CMD_NEXT), MarkStep::None);
        let g = b.entries()[2];
        b.pick(g);
        let step = b.command(markbox::CMD_CHECK);
        s.mark_step(step, &mut ctx);
        assert_eq!(sent(&link), vec![format!("MarkCheck({e}, {g})")]);
        let n = ctx.popups.len();
        Screen::event(
            &mut s,
            &LinkEvent::MarkChecked {
                ok: true,
                reason: 0,
            },
            &mut ctx,
        );
        assert_eq!(ctx.popups.len(), n + 1);
        let b = s.mark_box.as_mut().unwrap();
        assert!(b.checked);
        b.command(markbox::CMD_OK);
        let step = b.command(markbox::CMD_SEND);
        s.mark_step(step, &mut ctx);
        assert_eq!(sent(&link), vec![format!("MarkUse(71, {e}, {g})")]);
        used(&mut s, 71, &mut ctx);
        assert!(s.mark_box.is_none());
        let x = &ctx.data.char_extras;
        assert_eq!((x.clan_mark, x.clan_mark_ground), (e, g));
        assert!(ctx.data.store.items.iter().any(|i| i.uid == 71 && i.in_use));
        // the base character: the own gender's rows but the own one; the gender change: the other gender's
        let rows = facebox::rows_from_rom(Some(&rom));
        let own_man = rows.iter().find(|r| r.face == 2).expect("face row 2").man;
        s.use_function(72, face, &mut ctx);
        let b = s.face_box.as_mut().expect("the face window opens");
        assert!(!b.own_portrait.is_empty());
        assert_eq!(
            b.choices.len(),
            rows.iter().filter(|r| r.man == own_man).count() - 1
        );
        assert!(b.choices.iter().all(|r| r.man == own_man && r.face != 2));
        b.pick(0);
        let want = b.choices[0].face;
        b.ok();
        let step = b.ok();
        s.face_step(step, &mut ctx);
        assert_eq!(sent(&link), vec![format!("FaceUse(72, {})", 6000 + want)]);
        used(&mut s, 72, &mut ctx);
        assert!(s.face_box.is_none());
        assert_eq!(ctx.data.profile.face, 6000 + want);
        assert!(ctx.data.store.items.iter().all(|i| i.uid != 72));
        s.use_function(73, gender, &mut ctx);
        let b = s.face_box.as_mut().expect("the gender window opens");
        let man = rows.iter().find(|r| r.face == want).unwrap().man;
        assert!(!b.choices.is_empty() && b.choices.iter().all(|r| r.man != man));
        // Cancel closes it with the cancelled box
        let n = ctx.popups.len();
        let step = b.escape();
        s.face_step(step, &mut ctx);
        assert!(s.face_box.is_none());
        assert_eq!(ctx.popups.len(), n + 1);
    }
}

#[cfg(test)]
mod balance_tests {
    use super::super::link::{BuyReply, Link};
    use super::*;

    fn ware(product: u32, price: u32) -> Ware {
        Ware {
            product,
            name: "Megaphone".into(),
            item_no: 9042,
            kind_of: Some(0x15),
            tag_kind: Some(0x15),
            sale_type: 1,
            price,
            icon: None,
            display: 1,
            unlock_exp: 0,
            marks: [false; 3],
            period: 10,
            consume: 3,
        }
    }

    fn drawn(s: &ShopScreen) -> (String, String) {
        let t = |id| {
            s.sheet
                .part(MONEY_WINDOW, id, WidgetKind::Text)
                .map(|p| p.text.clone())
                .unwrap_or_default()
        };
        (t(MONEY_TEXTS[0]), t(MONEY_TEXTS[1]))
    }

    /// CLI-1875 / LG-405 (owner report 2026-11-04: the NX in game was not the admin dashboard's): an accepted credit
    /// purchase takes off the price of the row the window asked for (the cell shows the item's cheapest product), and
    /// every 0x345 sets the balance, the money panel drawing what it says (with the ROM: its texts).
    #[test]
    fn a_purchase_answer_deducts_the_asked_rows_price_and_each_balance_sets_the_panel() {
        let rom = dc_rom::resolve_rom().ok();
        let cat = match rom.as_deref() {
            Some(r) => Catalog::from_rom(r, 1024, 768).unwrap(),
            None => Catalog::fallback(None, 1024, 768),
        };
        let mut data = MenuData {
            balance: (0, 20_000),
            ..Default::default()
        };
        let mut s = ShopScreen::new(&cat, &data, Go::Shop);
        let link = Link::detached();
        let (mut popups, mut go, mut quit, mut sounds) = (Vec::new(), None, false, Vec::new());
        let mut ctx = Ctx {
            cat: &cat,
            data: &mut data,
            link: &link,
            popups: &mut popups,
            go: &mut go,
            quit: &mut quit,
            sounds: &mut sounds,
        };
        // the cell is the 10-pack at 2,900; the window's picked row is the 50-pack at 12,900
        s.buying = Some(ware(1748, 2900));
        let asked = buybox::Offer {
            product: 1749,
            price: 12_900,
            sale_type: 1,
            bonus: 0,
            consume: 3,
            period: 50,
            items: vec![9042],
        };
        s.request(&asked, buybox::MODE_PREPAID, &mut ctx);
        assert_eq!(
            link.take_sent()
                .iter()
                .map(|c| format!("{c:?}"))
                .collect::<Vec<_>>()
                .len(),
            1
        );
        let ok = BuyReply {
            ok: true,
            award: -1,
            ..Default::default()
        };
        s.event(&LinkEvent::Bought(ok), &mut ctx);
        assert_eq!(ctx.data.balance, (0, 20_000 - 12_900));
        // the lobby's 0x345 after the answer (LG-405), and later ones (an admin change, a reward): each one is the
        // balance, whatever the client deducted
        for (credit, prepaid) in [(0, 7000), (0, 17_000), (250, 3)] {
            let e = LinkEvent::Balance { credit, prepaid };
            super::super::shared_event(ctx.data, &e);
            s.event(&e, &mut ctx);
            assert_eq!(ctx.data.balance, (credit, prepaid));
            if rom.is_some() {
                assert_eq!(
                    drawn(&s),
                    (grouped(i64::from(prepaid)), grouped(i64::from(credit)))
                );
            }
        }
        // a refused purchase deducts nothing
        s.request(&asked, buybox::MODE_PREPAID, &mut ctx);
        s.event(
            &LinkEvent::Bought(BuyReply {
                ok: false,
                reason: 0x3b,
                ..ok
            }),
            &mut ctx,
        );
        assert_eq!(ctx.data.balance, (250, 3));
    }
}
