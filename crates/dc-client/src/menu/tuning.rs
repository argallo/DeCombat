//! w3-shop: the 2009 Modification screen (screen state 0x14): the weapon pane, the modification shop (the option
//! products of the catalogue in a grid of 2 x 3 cells with their prices, six weapon-class tabs) and the money panel.
//! Buy asks and sends 0x328 as the shop does.
//!
//! lobby-fixes10: the weapon select list (window 5, button 1009), the weapon's three option sockets (the top pane's
//! list), the option storage of a socket's kind (window 4) and its Equip (1008) attaching or detaching the picked option
//! (0x336 / 0x338; the replies 0x337 / 0x339 edit the client's own inventory).
//!
//! Open items (DC_DEFAULTS.md CLI-550, CLI-711): Extend Use / Discard in the option storage, the tuning sync 0x33a on
//! leaving, the tab filter by weapon class.
//!
//! shop-polish2 (CLI-760): first cell selected + the detail panel (menu::modpanel), socket frames and pictures, the
//! weapon picture and name image, the option storage cells with the Attached mark.
// SPEC: PROTOCOL_2009.md §4.1, §4.2 (0x328 / 0x329, 0x336 .. 0x33a)
// EVIDENCE: cshell+0x1bf20 (lobby bar 184 -> screen 0x14)
// EVIDENCE: cshell+0x15a110 (the screen's command handler: 1000 .. 1005 set the tab +0x5554 to 0..5; 1007 buys the
// selected cell, box 0x3d2 without one; 1012 / 1013 page back / next; 311 -> screen 9 when the shell's room flag
// +0x18 is set, else 0xf)
// OBSERVED: lobbyfx4/real/R33_mod.png (the 2009 screen: weapon pane, modification shop on AR, money
// panel)
// EVIDENCE: cshell+0x15a110 (1008 = 0x3f0: the picked option attached -> cshell+0x152c20 detach, else
// cshell+0x154e90 attach; 1009 = 0x3f1 toggles the weapon select list), cshell+0x154e90 (attach: nothing picked ->
// box 0x3d2; a request in flight -> nothing; 0x336 with the option's unique id, item number and the selected weapon's
// unique id), cshell+0x152c20 (detach: 0x338 with the option's unique id, item number and its parent's unique id)

use std::sync::Arc;

use super::buybox::{self, BoxPress, BuyArt, Extension};
use super::catalog::Catalog;
use super::dropdown::Dropdown;
use super::link::{InvRecord, LinkCmd, LinkEvent};
use super::lobby::{PanelArt, OWN_COLOUR, ROW_COLOUR};
use super::modpanel;
use super::popup::{text_ids, BoxTag, Popup};
use super::sheet::{image_fit, place_text, Sheet, ALIGN_CENTRE, ALIGN_RIGHT};
use super::shop::{
    bought_text, class_tag, draw_page_numbers, draw_ware_cell, exit_to, fill_money, grid_cells,
    group_step, inside, page_count, page_hit, sync_pager, CELLS, CELL_COLOUR, CELL_FONT,
    CELL_NAME_AT, CELL_SIZE, CMD_BAR_INBOX, CMD_BAR_MARKET, CMD_BAR_QUIT, CMD_BAR_SHOP, CMD_EXIT,
    CMD_PAGE_BACK, CMD_PAGE_FWD, SOUND_PRESS, TAG_X, TAG_Y, TUNING_FRAMES, TUNING_IMAGE_AT,
    TUNING_IMAGE_SIZE,
};
use super::storage::{Store, Ware, Wares, TAB_OPTIONS};
use super::{build_sheet, pages, Ctx, Go, MenuData, MenuKey, Screen};
use crate::hud::items::DrawItem;
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};

/// The weapon-class tabs (AR, SR, MG, SMG, shotgun, pistol).
pub const CMD_TABS: [i32; 6] = [1000, 1001, 1002, 1003, 1004, 1005];
/// The tabs' class names (strings) and their label form `<name>(<option count>)`.
// EVIDENCE: cshell+0x156c50 (the tab labels: strings 0x3fa, 0x3fb, 0x3fd, 0x3fe, 0x3ff, 0x403 through
// sprintf "%s(%d)" with the size of the class's option list)
pub const TAB_NAMES: [i32; 6] = [0x3fa, 0x3fb, 0x3fd, 0x3fe, 0x3ff, 0x403];
/// Display groups of the modification shop per tab (muzzle, sight, clip).
pub const GROUPS_PER_TAB: usize = 3;
pub const CMD_BUY: i32 = 1007;
/// The option storage's Equip (attach / detach the picked option), its Close, the weapon select toggle.
// EVIDENCE: cshell+0x15a110 (0x3f0, 0x3f1); layout page 10 window 4 (Equip 1008 caption 0x160e, close 1006)
pub const CMD_FIT: i32 = 1008;
pub const CMD_CLOSE_STORAGE: i32 = 1006;
pub const CMD_PICK_GUN: i32 = 1009;
/// storage-functions: the option storage's Extend Use (caption 0x435) and Discard (caption 0x15fe).
// EVIDENCE: layout page 10 window 4 (1019 Extend Use, 148 Discard); cshell+0x15a110 (1019 = 0x3f1 + 10 -> the purchase
// window; 0x94 -> cshell+0x155440 then box 0x372 titled 0x371 with Yes / No, its Yes command 0x95 (cshell+0x1524c0) ->
// cshell+0x1550e0 sends 0x334)
pub const CMD_MORE_TIME: i32 = 1019;
/// storage-functions: the money panel's NX Charge (the 2009 charge page, CLI-773).
// EVIDENCE: cshell+0x15a110 (1024 = 0x3f1 + 0xf: ShellExecuteA of the charge page)
pub const CMD_CHARGE: i32 = 1024;
pub const CMD_THROW_OUT: i32 = 148;
/// Option kinds of the three sockets (muzzle, sight, magazine).
// SPEC: PROTOCOL_2009.md §4.2 0x33a (kinds 0x11 / 0x12 / 0x13)
pub const OPTION_KINDS: [i32; 3] = [0x11, 0x12, 0x13];
/// The interface sound an attach of each socket's kind plays.
// DISASM: cshell+0x158ad2 (attach reply ok: kind 0x11 -> sound 6, 0x12 -> 8, 0x13 -> 0xa through cshell+0x1b6470)
pub const ATTACH_SOUNDS: [i32; 3] = [6, 8, 0xa];
/// The socket rows of the top pane (x, y, w, h of the first; the next ones a pitch lower).
// DC-DEFAULT: CLI-711 (proposed) socket rows at (21, 156 + 75 k) of 194 x 72
// OBSERVED: lobby10/cpp tuncpp_bb00_mp5.png (reference: Muzzle / Sights / Clip boxes at y 156 / 231 / 306)
pub const SOCKET_ROW: [i32; 4] = [21, 156, 194, 72];
pub const SOCKET_PITCH: i32 = 75;
/// The weapon select list's rows (list (5, 0) at (32, 165), spacing 11): list items 0xfe x 0x56 with the
/// Inven_Item_Slot frames, the weapon's slot image over the row, its class tag at (5, 0x10), its name in font 0x15
/// size 0xe 0xffa3c3c3 at (5, 2) and the Equiped mark 0x35 x 0x12 at the top right while it sits in a slot.
// EVIDENCE: cshell+0x158e90 (the list's rows: owned rows of weapon groups 0..5 / 0x1a; list item cshell+0x2f9f90 with
// Inven_Item_Slot_Off / _On through vtable +0x140 (cshell+0x2f9680), sized 0xfe x 0x56 through +0x158; slot image
// cshell+0x2fa070 (0xfe x 0x56), class tag cshell+0x2fa1a0 of cshell+0x35a5b0, name cshell+0x2fb260 at (5, 2) font
// 0x15 size 0xe 0xffa3c3c3; Equiped_Mark 0x35 x 0x12 through cshell+0x2fa310 when cshell+0x325220 finds the weapon
// in a slot)
// DC-DEFAULT: CLI-765 rows one item height + the list spacing apart (0x56 + 11 = 97; the list layout's pitch is not
// traced; the reference stack shows rows 80 apart without frames and with smaller images, not followed), the shown weapon's
// row in the _On frame
pub const WEAPON_ROW: [i32; 4] = [32, 165, 0xfe, 0x56];
pub const WEAPON_PITCH: i32 = 0x56 + 11;
pub const WEAPON_FRAMES: [&str; 2] = [
    "UI_SCR_Page_Shop\\Inven\\Inven_Item_Slot_Off.dtx",
    "UI_SCR_Page_Shop\\Inven\\Inven_Item_Slot_On.dtx",
];
pub const EQUIPPED_MARK: (&str, [i32; 2]) = (
    "UI_SCR_Page_Shop\\Inventory\\Equiped_Mark.dtx",
    [0x35, 0x12],
);
/// The weapon select header (window 2): collapsed it shows Short_Front (image 0) and the last texels of Short_Back
/// (image 1, cut 0x54 texels from its left); with the pointer on it it expands to the full button (3).
// EVIDENCE: cshell+0x154810 (pointer over image 0 -> expand, else collapse; off the shown button with the list closed ->
// collapse), cshell+0x1535d0 (collapse: images 0 / 1 shown, button 3 hidden, image 1's source rectangle (cut, 0, w, h)
// and size (w - cut, h) with the cut growing 10 per 5 ms to 0x54; expand: the cut shrinks 10 per 5 ms to 0, then button
// 3 shown and images 0 / 1 hidden), cshell+0x159a99 (the screen opens collapsing from 0x53), cshell+0x152d20 /
// cshell+0x159420 (closing or picking in the list collapses)
// the slide runs on the menu clock ([`Screen::animate`]): 10 texels per 5 ms step, 9 steps (45 ms) over the 0x54 cut
pub const SLIDE_STEP_SECS: f32 = 0.005;
pub const SLIDE_STEP_CUT: i32 = 10;
pub const SELECT_FRONT: i32 = 0;
pub const SELECT_BACK: i32 = 1;
pub const SELECT_BUTTON: i32 = 3;
pub const SELECT_BACK_CUT: i32 = 0x54;
/// The socket labels (Muzzle, Sights, Clip), the weapon select caption, Attach / Detach.
// OBSERVED: the user's string table (0x376 .. 0x378 "Muzzle" / "Sights" / "Clip", 0x40d "Weapon Select ", 0x1564
// "Attach", 0x1565 "Detach"); EVIDENCE: cshell+0x15a110 (0x1564 in the 0x3f2 box)
pub const SOCKET_TEXTS: [i32; 3] = [0x376, 0x377, 0x378];
pub const WEAPON_SELECT_TEXT: i32 = 0x40d;
pub const ATTACH_TEXT: i32 = 0x1564;
pub const DETACH_TEXT: i32 = 0x1565;
/// The option storage grid (window 4) and the weapon pane's name image and select caption.
pub const STORE_GRID: i32 = 4;
pub const NAME_IMAGE: i32 = 2;
pub const SELECT_CAPTION: i32 = 4;
/// Windows: the weapon pane, the shop, the storage, the weapon select list.
pub const WEAPON_WINDOW: i32 = 2;
pub const SHOP_WINDOW: i32 = 3;
pub const STORAGE_WINDOW: i32 = 4;
pub const SELECT_WINDOW: i32 = 5;
/// The shop grid of window 3.
pub const SHOP_GRID: i32 = 2;
/// Page number buttons.
pub const PAGE_IDS: [i32; 5] = [22, 23, 24, 25, 26];
/// The price in a cell (right end, top): under the cell's right edge.
// DC-DEFAULT: CLI-550 the price's right end at (170, 73) of the 194 x 71 cell (OBSERVED R33_mod.png: the
// prices under the cells end near x 192 of the cell with the GP mark)
pub const PRICE_AT: [i32; 2] = [170, 73];
/// A modification cell's price with its unit (`100 GP`) as text, right end at (0xbd, 0x48); the cell also shows
/// the shop cell's rank badge, LOCKED tag, silhouette and class tag.
// EVIDENCE: cshell+0x157c00 (the modification cell fill: price text + " GP" / " NX" by the product's SaleType at
// (0xbd, 0x48) in 0xffa3c3c3, no mark image; LOCKED through cshell+0x2faea0 / 0x34fdb0, badge through
// cshell+0x2fb140 at (8, 0x2a), silhouette cshell+0x2f9e60, class tag cshell+0x25a5b0, as cshell+0x149f80)
pub const WARE_PRICE_AT: [i32; 2] = [0xbd, 0x48];
/// The money panel's value texts (on the screen): prepaid, credit, GP, grade.
// OBSERVED: lobbyfx4/real/R33_mod.png (NX Prepaid 0 at x 205, NX Credit 0 at x 396, GP 10,000 at
// x 705, grade Common at x 552: the value texts of the records at x 210 / 401 / 705 / 552)
pub const MONEY_TEXTS: [i32; 4] = [30, 31, 32, 34];
/// The pane's weapon picture (image 6) and its option pictures (images 7, 8: hidden while the socket is empty).
// EVIDENCE: cshell+0x155d30 (image 6 = the weapon's TuningScreenIMG), cshell+0x152e80 (images 7 / 8 hidden without a
// muzzle / sight option)
pub const PICTURE_ID: i32 = 6;
pub const OPTION_PICTURE_IDS: [i32; 2] = [7, 8];
/// A socket row: frame 0xce x 0x47 (lit while its storage is open), the option's slot image (or the socket's empty
/// image) 0xe3 x 0x47 0x2d px left of the row, its name (or the socket's label) in font 0x15 white at (5, 5).
// EVIDENCE: cshell+0x1549d0 (three rows Tunning_Slot_OFF / _ON 0xce x 0x47), cshell+0x1525f0 (an empty socket:
// Tunning_Slot_<kind>_ON when the weapon row's socket state is 1, else Tunning_Slot_Disable_ON, with the label
// 0x376 / 0x377 / 0x378), cshell+0xdb0d0 (a fitted socket: the option's slot image and name), cshell+0xda760 (image
// at x - 0x2d, 0xe3 x 0x47), cshell+0xda950 (label font 0x15 in 0xffffffff at (5, 5))
pub const SOCKET_FRAMES: [&str; 2] = [
    "UI_SCR_Page_Tuning\\Top\\Tunning_Slot_OFF.dtx",
    "UI_SCR_Page_Tuning\\Top\\Tunning_Slot_ON.dtx",
];
pub const SOCKET_FRAME_SIZE: [i32; 2] = [0xce, 0x47];
pub const SOCKET_EMPTY: [&str; 3] = [
    "UI_SCR_Page_Tuning\\Top\\Tunning_Slot_Silencer_ON.dtx",
    "UI_SCR_Page_Tuning\\Top\\Tunning_Slot_Sight_ON.dtx",
    "UI_SCR_Page_Tuning\\Top\\Tunning_Slot_Magazine_ON.dtx",
];
pub const SOCKET_DISABLED: &str = "UI_SCR_Page_Tuning\\Top\\Tunning_Slot_Disable_ON.dtx";
pub const SOCKET_IMAGE_AT: [i32; 2] = [-0x2d, 0];
pub const SOCKET_IMAGE_SIZE: [i32; 2] = [0xe3, 0x47];
pub const SOCKET_LABEL_AT: [i32; 2] = [5, 5];
/// An option storage cell: frame (lit when picked), the option's image, its name; an attached option's Attached
/// mark 0x35 x 0x12 at its top right and its weapon's name at its bottom right.
// EVIDENCE: cshell+0x154480 (storage cells Tunning_Inven_Slot_OFF / _ON), cshell+0x158210 (name font 0x15 size 0xe
// in 0xffa3c3c3 at (5, 2); a row with a parent: Attached_Mark 0x35 x 0x12 and the parent's name in font 0x18 size
// 0xe), cshell+0x2fa310 (the mark at x + width - 1 - 0x35, y + 1)
// DC-DEFAULT: CLI-760 the weapon name's text box: right end 7 px inside the cell, top 0x37 below the cell top, in
// 0xfffead00 (OBSERVED lobby10 tuncpp_bb01_attached.png: "M4A1" x 178..206, y 509..515 in (254, 173, 0))
pub const STORE_FRAMES: [&str; 2] = [
    "UI_SCR_Page_Tuning\\Bottom\\Tunning_Inven\\Tunning_Inven_Slot_OFF.dtx",
    "UI_SCR_Page_Tuning\\Bottom\\Tunning_Inven\\Tunning_Inven_Slot_ON.dtx",
];
pub const ATTACHED_MARK: (&str, [i32; 2]) = (
    "UI_SCR_Page_Shop\\Inventory\\Attached_Mark.dtx",
    [0x35, 0x12],
);
pub const PARENT_NAME_AT: [i32; 2] = [-7, 0x37];
pub const PARENT_NAME_FONT: i32 = 0x18;
pub const PARENT_NAME_COLOUR: u32 = 0xfffe_ad00;

const TAB_ALIGN: Align = Align {
    first: ALIGN_CENTRE,
    second: ALIGN_SECOND_CENTRE,
};

/// The Modification screen.
pub struct TuningScreen {
    sheet: Sheet,
    wares: Arc<Wares>,
    art: Option<PanelArt>,
    pub tab: usize,
    pub page: usize,
    pub selected: Option<usize>,
    cells: Vec<[i32; 4]>,
    buying: Option<Ware>,
    /// CLI-1875: the option word and price of a credit purchase in flight, deducted on its accepted answer.
    charge: Option<(u32, u32)>,
    /// The weapon the sockets show (unique id).
    pub weapon: Option<u32>,
    /// The socket whose option storage is open (index in [`OPTION_KINDS`]).
    pub socket: Option<usize>,
    /// The option picked in the storage (unique id).
    pub picked: Option<u32>,
    /// The attach / detach in flight: option, weapon, socket, detach.
    pending: Option<(u32, u32, usize, bool)>,
    store_cells: Vec<[i32; 4]>,
    weapon_caption: String,
    /// The weapons table (each weapon's `ModName<n>` list), read once per ROM.
    fits: Option<Arc<dc_bute::WeaponsTable>>,
    /// The option pictures over the weapon (images 7 / 8).
    pictures: Arc<OptionPictures>,
    /// The weapon select header shows the full button (else the short pair), and Short_Back's own texture and size.
    pub select_expanded: bool,
    select_back: (String, [i32; 2]),
    /// The header's slide: Short_Back's current cut, and the slide's start (clock, cut at the start; the clock is taken
    /// at the next frame).
    select_cut: i32,
    select_slide: Option<(Option<f32>, i32)>,
    socket_texts: [String; 3],
    /// storage-functions: the Extend Use purchase window, its textures, a purchase in flight (+0x5568) and the unique
    /// id a discard (0x334) is out for.
    pub extension: Option<Extension>,
    buy_art: BuyArt,
    buy_busy: bool,
    discarding: Option<u32>,
    /// CLI-880: a mod just bought (unique id, item number) that is attached to the shown weapon once the inventory
    /// reread has brought it in.
    auto_attach: Option<(u32, u32)>,
}

/// The ROM's weapons table, read once per ROM folder and process.
fn weapons_table(rom: &std::path::Path) -> Option<Arc<dc_bute::WeaponsTable>> {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};
    type Cache = Mutex<HashMap<PathBuf, Option<Arc<dc_bute::WeaponsTable>>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let mut c = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    c.entry(rom.to_path_buf())
        .or_insert_with(|| {
            dc_bute::WeaponsTable::from_vfs(&dc_bute::RezVfs::new(rom))
                .ok()
                .map(Arc::new)
        })
        .clone()
}

/// The pictures a fitted muzzle or sight draws over the weapon picture (images 7 / 8), read from the item table.
// EVIDENCE: cshell+0x25dd60 (the item-table loader: a weapon row's SilencerImgIndex / SightImgIndex / MagazineImgIndex
// -> +0x514 / +0x518 / +0x51c; an option row's ItemImgName / ItemPosName / ItemScaleName / TuningImgSize -> +0x580 /
// +0x59c / +0x5b8 / +0x5d4), cshell+0x25bdb0 / 0x25b110 / 0x25b1b0 / 0x25b250 / 0x25ba50 (their readers)
#[derive(Default)]
pub struct OptionPictures {
    /// Weapon item number -> its muzzle and sight picture index.
    weapons: std::collections::HashMap<i32, [i32; 2]>,
    /// Option item number -> picture name, position key, scale key, source size.
    options: std::collections::HashMap<i32, (String, String, String, [i32; 2])>,
    /// The `[Silencer]` and `[Sight]` sections.
    sections: [dc_bute::SectionMap; 2],
}

/// A picture over the weapon: texture, destination (x, y, w, h), source size.
#[derive(Debug, Clone, PartialEq)]
pub struct OptionPicture {
    pub tex: String,
    pub rect: [i32; 4],
    pub source: [i32; 2],
}

/// The sections of the muzzle and sight pictures.
// EVIDENCE: cshell+0x152e80 (section "Silencer" for image 7, "Sight" for image 8; the magazine has no picture)
pub const OPTION_PICTURE_SECTIONS: [&str; 2] = ["Silencer", "Sight"];

fn table_value(v: &str) -> String {
    let v = v.trim();
    match v.strip_prefix('"').and_then(|r| r.split_once('"')) {
        Some((inner, _)) => inner.trim().to_string(),
        None => v.split("//").next().unwrap_or("").trim().to_string(),
    }
}

fn table_number(v: &str) -> Option<f32> {
    table_value(v)
        .trim_end_matches(['f', 'F'])
        .parse::<f32>()
        .ok()
}

impl OptionPictures {
    pub fn from_doc(doc: &dc_bute::ButeDoc) -> OptionPictures {
        let mut out = OptionPictures::default();
        let int = |d: &dc_bute::SectionMap, k: &str| {
            d.get(k).and_then(|v| table_value(v).parse::<i32>().ok())
        };
        for (tag, d) in doc.iter() {
            let Some(id) = int(d, "ItemID") else {
                continue;
            };
            match super::storage::item_family_base(tag) {
                Some(0) => {
                    out.weapons.insert(
                        id,
                        [
                            int(d, "SilencerImgIndex").unwrap_or(-1),
                            int(d, "SightImgIndex").unwrap_or(-1),
                        ],
                    );
                }
                Some(1000) => {
                    let text = |k: &str| d.get(k).map(|v| table_value(v)).unwrap_or_default();
                    let size = d
                        .get("TuningImgSize")
                        .and_then(|v| {
                            let v = table_value(v);
                            let (a, b) = v.trim_matches(['(', ')']).split_once(',')?;
                            Some([table_number(a)? as i32, table_number(b)? as i32])
                        })
                        .unwrap_or([0, 0]);
                    out.options.insert(
                        1000 + id,
                        (
                            text("ItemImgName"),
                            text("ItemPosName"),
                            text("ItemScaleName"),
                            size,
                        ),
                    );
                }
                _ => {}
            }
        }
        out.sections = OPTION_PICTURE_SECTIONS.map(|s| doc.get(s).cloned().unwrap_or_default());
        out
    }

    /// The picture option `option` (item number) draws on weapon `weapon` (item number) in socket `k` (0 muzzle,
    /// 1 sight): `<ItemImgName><the weapon's index>` of the socket's section, placed at the section's
    /// `ItemID<weapon>_<ItemPosName>` and sized TuningImgSize, both times `ItemID<weapon>_<ItemScaleName>`
    /// (truncated); `None` without a picture name or a scale.
    // EVIDENCE: cshell+0x152e80 (sprintf "%s%d" name + index, "%s%d_%s" "ItemID" + the weapon's ItemID + key; the
    // image's source rectangle (0, 0, TuningImgSize), its place pos x scale and size TuningImgSize x scale through
    // fild / fmul / ftol), cshell+0x1993f0 (a missing scale reads as 1.2e-38: nothing drawn)
    pub fn picture(&self, weapon: i32, option: i32, k: usize) -> Option<OptionPicture> {
        let index = self.weapons.get(&weapon)?[k];
        let (name, pos_key, scale_key, size) = self.options.get(&option)?;
        let sec = &self.sections[k];
        let tex = sec.get(&format!("{name}{index}")).map(|v| table_value(v))?;
        let scale = sec
            .get(&format!("ItemID{weapon}_{scale_key}"))
            .and_then(|v| table_number(v))?;
        let pos = sec
            .get(&format!("ItemID{weapon}_{pos_key}"))
            .and_then(|v| {
                let v = table_value(v);
                let (a, b) = v.trim_matches(['(', ')']).split_once(',')?;
                Some([table_number(a)? as i32, table_number(b)? as i32])
            })
            .unwrap_or([0, 0]);
        if tex.is_empty() {
            return None;
        }
        let f = |v: i32| (v as f32 * scale) as i32;
        Some(OptionPicture {
            tex,
            rect: [f(pos[0]), f(pos[1]), f(size[0]), f(size[1])],
            source: *size,
        })
    }

    /// [`OptionPictures::from_doc`] of the ROM's item table, read once per ROM folder and process.
    pub fn cached(rom: &std::path::Path) -> Arc<OptionPictures> {
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::sync::{Mutex, OnceLock};
        static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<OptionPictures>>>> = OnceLock::new();
        let mut c = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        c.entry(rom.to_path_buf())
            .or_insert_with(|| {
                Arc::new(
                    dc_bute::ItemsTable::from_rom(rom)
                        .map(|t| OptionPictures::from_doc(t.doc()))
                        .unwrap_or_default(),
                )
            })
            .clone()
    }
}

/// The catalogue's name of an inventory row.
fn label(wares: &Wares, r: &InvRecord) -> String {
    wares.label_of(r.item_no as i32)
}

fn centre(r: [i32; 4]) -> [f32; 2] {
    [(r[0] + r[2] / 2) as f32, (r[1] + r[3] / 2) as f32]
}

fn row(first: [i32; 4], pitch: i32, k: usize) -> [i32; 4] {
    [first[0], first[1] + pitch * k as i32, first[2], first[3]]
}

impl TuningScreen {
    pub fn new(cat: &Catalog, data: &MenuData) -> TuningScreen {
        let mut sheet = build_sheet(cat, &[pages::LOBBY_COMMON, pages::TUNING]);
        // DC-DEFAULT: CLI-550 the screen opens on the shop (window 3) with the storage (window 4) and the
        // weapon select list (window 5) hidden (OBSERVED R33_mod.png)
        sheet.show_window(STORAGE_WINDOW, false);
        sheet.show_window(SELECT_WINDOW, false);
        let mut s = TuningScreen {
            sheet,
            wares: Wares::cached(cat.rom.as_deref()),
            art: cat.rom.as_deref().and_then(PanelArt::cached),
            tab: 0,
            page: 0,
            selected: None,
            cells: grid_cells(cat, pages::TUNING, SHOP_WINDOW, SHOP_GRID),
            buying: None,
            charge: None,
            weapon: None,
            socket: None,
            picked: None,
            pending: None,
            store_cells: grid_cells(cat, pages::TUNING, STORAGE_WINDOW, STORE_GRID),
            weapon_caption: String::new(),
            fits: cat.rom.as_deref().and_then(weapons_table),
            pictures: cat
                .rom
                .as_deref()
                .map(OptionPictures::cached)
                .unwrap_or_default(),
            select_expanded: false,
            select_back: (String::new(), [0, 0]),
            select_cut: SELECT_BACK_CUT,
            select_slide: None,
            socket_texts: SOCKET_TEXTS.map(|t| cat.text(t)),
            extension: None,
            buy_art: BuyArt::from_rom(cat.rom.as_deref()),
            buy_busy: false,
            discarding: None,
            auto_attach: None,
        };
        for id in OPTION_PICTURE_IDS {
            s.sheet.show(-1, id, dc_ui_data::WidgetKind::Image, false);
        }
        if let Some(p) = s
            .sheet
            .part(WEAPON_WINDOW, SELECT_BACK, dc_ui_data::WidgetKind::Image)
        {
            if let super::sheet::Look::Panel { tex, .. } = &p.look {
                s.select_back = (tex.clone(), p.size);
            }
        }
        s.sync_select_header();
        s.weapon_caption = cat.text(WEAPON_SELECT_TEXT);
        s.refresh(data);
        s
    }

    /// The option products of the current tab's weapon class; CLI-1606: on the shown weapon's own tab without the
    /// mods its closed sockets cannot take.
    pub fn wares_shown(&self, data: &MenuData) -> Vec<Ware> {
        let weapon = self
            .weapon
            .and_then(|w| data.store.items.iter().find(|r| r.uid == w))
            .map(|r| r.item_no as i32);
        let weapon = weapon.filter(|_| self.fits.is_some());
        weapon_tab_wares(&self.wares, &data.store, self.tab, weapon)
    }

    /// CLI-1606: socket `k` of weapon `weapon_no` takes an option (its socket state is 1); without the weapons
    /// table every socket does (CLI-712's ModName check is skipped too).
    fn socket_takes(&self, weapon_no: i32, k: usize) -> bool {
        self.fits.is_none() || socket_open(&self.wares, weapon_no, k)
    }

    /// The pager's enabled buttons for the current tab and page.
    /// The current tab button is selected, the others not.
    // EVIDENCE: cshell+0x1528d0 (controls 10 .. 15: vtable +0x84 on the tab +0x5554's, +0x88 on the others)
    fn select_tab(&mut self) {
        for (k, cmd) in CMD_TABS.iter().enumerate() {
            if let Some(p) = self
                .sheet
                .parts
                .iter_mut()
                .find(|p| p.kind == dc_ui_data::WidgetKind::Button && p.command == *cmd)
            {
                p.selected = k == self.tab;
            }
        }
    }

    fn sync_pages(&mut self, data: &MenuData) {
        self.show_weapon(data);
        self.select_first(data);
        self.select_tab();
        let pages = page_count(self.wares_shown(data).len());
        sync_pager(&mut self.sheet, &PAGE_IDS, self.page, pages);
    }

    /// The owned weapons (item numbers below 1000 of a known class), in inventory order.
    pub fn weapons<'a>(&self, data: &'a MenuData) -> Vec<&'a InvRecord> {
        data.store
            .items
            .iter()
            .filter(|r| r.item_no < 1000 && self.wares.kind_of(r.item_no as i32).is_some())
            .collect()
    }

    /// The owned options of socket `k`'s kind.
    pub fn options<'a>(&self, data: &'a MenuData, k: usize) -> Vec<&'a InvRecord> {
        data.store
            .items
            .iter()
            .filter(|r| {
                (1000..2000).contains(&r.item_no)
                    && self.wares.kind_of(r.item_no as i32) == Some(OPTION_KINDS[k])
            })
            .collect()
    }

    /// The option on the shown weapon's socket `k`.
    pub fn socket_option<'a>(&self, data: &'a MenuData, k: usize) -> Option<&'a InvRecord> {
        let w = self.weapon?;
        self.options(data, k)
            .into_iter()
            .find(|o| o.parent_uid == w)
    }

    /// The point a scripted player clicks for socket `k`, weapon row `k` or storage row `k`.
    pub fn socket_point(k: usize) -> [f32; 2] {
        centre(row(SOCKET_ROW, SOCKET_PITCH, k))
    }
    pub fn weapon_point(k: usize) -> [f32; 2] {
        centre(row(WEAPON_ROW, WEAPON_PITCH, k))
    }
    pub fn store_point(&self, k: usize) -> Option<[f32; 2]> {
        self.store_cells.get(k).map(|r| centre(*r))
    }

    /// The Equip button reads Attach, or Detach while the picked option sits on a weapon.
    // OBSERVED: lobby10/cpp tuncpp_bb01_storage.png (the reference stack's button reads "Detach" over an attached option)
    fn equip_caption(&mut self, data: &MenuData) {
        let attached = self
            .picked
            .and_then(|u| data.store.items.iter().find(|r| r.uid == u))
            .is_some_and(|r| r.parent_uid != 0);
        let id = if attached { DETACH_TEXT } else { ATTACH_TEXT };
        for p in self.sheet.parts.iter_mut() {
            if p.kind == dc_ui_data::WidgetKind::Button && p.command == CMD_FIT {
                if let super::sheet::Look::Button {
                    caption: Some(c), ..
                } = &mut p.look
                {
                    c.string_id = id;
                }
            }
        }
    }

    /// Shows the shop (window 3) or the option storage of socket `k` (window 4).
    fn open_storage(&mut self, k: Option<usize>, data: &MenuData) {
        self.socket = k;
        // DC-DEFAULT: CLI-711 the storage opens with the socket's own option (else nothing) picked
        self.picked = k.and_then(|k| self.socket_option(data, k)).map(|r| r.uid);
        self.equip_caption(data);
        self.sheet.show_window(STORAGE_WINDOW, k.is_some());
        self.sheet.show_window(SHOP_WINDOW, k.is_none());
    }

    /// The weapon select header: the full button once expanded and fully slid out, else Short_Front and Short_Back's
    /// texels from the current cut on.
    // EVIDENCE: cshell+0x1535d0 (the collapse / expand slide, see [`SELECT_BACK_CUT`])
    fn sync_select_header(&mut self) {
        use dc_ui_data::WidgetKind::{Button, Image};
        let full = self.select_expanded && self.select_cut == 0;
        self.sheet.show(WEAPON_WINDOW, SELECT_BUTTON, Button, full);
        self.sheet.show(WEAPON_WINDOW, SELECT_FRONT, Image, !full);
        self.sheet.show(WEAPON_WINDOW, SELECT_BACK, Image, !full);
        let (tex, [w, h]) = self.select_back.clone();
        if let (false, Some(p)) = (
            tex.is_empty(),
            self.sheet.part_mut(WEAPON_WINDOW, SELECT_BACK, Image),
        ) {
            let cut = self.select_cut.min(w);
            p.look = super::sheet::Look::Panel {
                tex: crate::hud::compose::part_of(
                    &tex,
                    [cut as u32, 0, (w - cut) as u32, h as u32],
                ),
                stretch: false,
            };
            p.size = [w - cut, h];
        }
    }

    /// The header changes direction: a new slide from the cut it has now.
    fn select_changed(&mut self) {
        self.select_slide = Some((None, self.select_cut));
        self.sync_select_header();
    }

    fn select_target(&self) -> i32 {
        if self.select_expanded {
            0
        } else {
            SELECT_BACK_CUT.min(self.select_back.1[0])
        }
    }

    /// The rectangle of window 2's control `id` of `kind`.
    fn select_part_rect(&self, id: i32, kind: dc_ui_data::WidgetKind) -> Option<[i32; 4]> {
        self.sheet
            .part(WEAPON_WINDOW, id, kind)
            .map(|p| [p.pos[0], p.pos[1], p.size[0], p.size[1]])
    }

    fn select_open(&self) -> bool {
        self.sheet
            .parts
            .iter()
            .position(|p| p.kind == dc_ui_data::WidgetKind::Window && p.layer == SELECT_WINDOW)
            .is_some_and(|i| self.sheet.visible(i))
    }

    /// Equip of the option storage: detach the picked option when it sits on a weapon, else attach it to the shown
    /// weapon.
    fn equip(&mut self, ctx: &mut Ctx) {
        if self.pending.is_some() {
            return;
        }
        let Some(k) = self.socket else {
            return;
        };
        let Some(o) = self
            .picked
            .and_then(|u| ctx.data.store.items.iter().find(|r| r.uid == u))
            .cloned()
        else {
            ctx.notice(0x3d2);
            return;
        };
        if o.parent_uid != 0 {
            ctx.send(LinkCmd::Detach {
                opt_uid: o.uid,
                opt_item: o.item_no,
                weapon_uid: o.parent_uid,
            });
            self.pending = Some((o.uid, o.parent_uid, k, true));
        } else if let Some(w) = self.weapon {
            // the weapon must take the option
            // DC-DEFAULT: CLI-712 (proposed) "fits" = the weapon's ModName list of the item table (2009:
            // cshell+0x152d90 compares the option's weapon class with the weapon's and the weapon's per-kind socket
            // flags, box 0x4df when it fails)
            let weapon_no = ctx
                .data
                .store
                .items
                .iter()
                .find(|r| r.uid == w)
                .map_or(-1, |r| r.item_no as i32);
            // CLI-1606: the weapon's socket of the option's kind must be open (state 1): a weapon whose model
            // carries the part as standard (state 2, the suppressed snipers) or has no such socket (0) takes none
            // EVIDENCE: cshell+0x152d90 (kind 0x11 / 0x12 / 0x13 -> the weapon record's +0x42c / +0x430 / +0x434 must
            // be 1, else 0 and box 0x4df through cshell+0x154e90)
            if self
                .fits
                .as_ref()
                .is_some_and(|t| !t.option_weapon_compatible(o.item_no as i32, weapon_no))
                || !self.socket_takes(weapon_no, k)
            {
                ctx.notice(0x4df);
                return;
            }
            ctx.send(LinkCmd::Attach {
                opt_uid: o.uid,
                opt_item: o.item_no,
                weapon_uid: w,
            });
            self.pending = Some((o.uid, w, k, false));
        } else {
            ctx.notice(0x3d2);
        }
    }

    /// CLI-880: attaches the mod bought a moment ago to the shown weapon once its record is in the inventory.
    fn try_auto_attach(&mut self, ctx: &mut Ctx) {
        let Some((uid, item)) = self.auto_attach else {
            return;
        };
        if self.pending.is_some() || !ctx.data.store.items.iter().any(|r| r.uid == uid) {
            return;
        }
        self.auto_attach = None;
        let (Some(w), Some(k)) = (
            self.weapon,
            self.wares
                .kind_of(item as i32)
                .and_then(|kd| OPTION_KINDS.iter().position(|x| *x == kd)),
        ) else {
            return;
        };
        let weapon_no = ctx
            .data
            .store
            .items
            .iter()
            .find(|r| r.uid == w)
            .map_or(-1, |r| r.item_no as i32);
        if self
            .fits
            .as_ref()
            .is_some_and(|t| !t.option_weapon_compatible(item as i32, weapon_no))
            || !self.socket_takes(weapon_no, k)
        {
            return;
        }
        ctx.send(LinkCmd::Attach {
            opt_uid: uid,
            opt_item: item,
            weapon_uid: w,
        });
        self.pending = Some((uid, w, k, false));
    }

    /// CLI-1430: the option an inbox opening added (the store's `tuning_goto`) is shown once the inventory holds it:
    /// the tab of its weapon class, the option storage of its kind, the option picked. The kept id is dropped when
    /// the record is in or the inventory has no such option.
    // EVIDENCE: cshell+0x1596f0 (the item's weapon class picks the tab +0x5554: 0 -> 0, 1 -> 3, 2 -> 1, 3 -> 2,
    // 4 -> 5, 5 and 0x1a -> 4; the item found in the tab's option list: window 3 hidden, window 4 shown, its page and
    // cell selected), cshell+0x159a99 (the screen's entry runs it with the kept unique id)
    fn goto_option(&mut self, data: &mut MenuData) {
        let Some(uid) = data.store.tuning_goto else {
            return;
        };
        let Some(rec) = data.store.items.iter().find(|r| r.uid == uid).cloned() else {
            return;
        };
        data.store.tuning_goto = None;
        let item = rec.item_no as i32;
        let Some(k) = self
            .wares
            .kind_of(item)
            .and_then(|kd| OPTION_KINDS.iter().position(|x| *x == kd))
        else {
            return;
        };
        let class = self.wares.art_of(item).and_then(|a| a.attach_class);
        self.tab = class_tab(class.unwrap_or(0));
        self.page = 0;
        self.selected = None;
        self.open_storage(Some(k), data);
        self.picked = Some(uid);
        self.equip_caption(data);
    }

    /// The tab follows the shown weapon's class, so the shop lists the mods made for that weapon (a sniper shows
    /// the SR tab and its SR magazines, not the AR ones of the same name).
    // EVIDENCE: cshell+0x155d30 (showing a weapon keeps the tab of its class, record +0x2c: 0 -> 0, 1 -> 3, 2 -> 1,
    // 3 -> 2, 4 -> 5, 5 / 0x1a -> 4), cshell+0x159420 (a weapon picked in the select list: the current tab becomes
    // that tab, the shop or the storage refilled for it), cshell+0x15a6e0 (the same on the screen's entry)
    fn follow_weapon_tab(&mut self, data: &MenuData) {
        let class = self
            .weapon
            .and_then(|w| data.store.items.iter().find(|r| r.uid == w))
            .and_then(|r| self.wares.kind_of(r.item_no as i32));
        if let Some(c) = class {
            self.tab = class_tab(c);
            self.page = 0;
            self.selected = None;
        }
    }

    /// The picked option of the open storage.
    fn picked_record(&self, data: &MenuData) -> Option<InvRecord> {
        self.socket?;
        self.picked
            .and_then(|u| data.store.items.iter().find(|r| r.uid == u))
            .cloned()
    }

    /// Discard (148): nothing picked -> 0x3d2, an attached option -> 0x1642 titled 0x1826, else the confirmation.
    // EVIDENCE: cshell+0x155440 (the cell's parent +0x244 set -> 0x1642 titled 0x1826), cshell+0x15a110 (box 0x372
    // titled 0x371, Yes / No)
    fn discard(&mut self, ctx: &mut Ctx) {
        let Some(o) = self.picked_record(ctx.data) else {
            ctx.notice(0x3d2);
            return;
        };
        if o.parent_uid != 0 {
            let (text, title) = super::storage::DISCARD_EQUIPPED;
            let mut q = Popup::ok(ctx.cat.text(text), BoxTag::None);
            q.title = title;
            ctx.popups.push(q);
            return;
        }
        let mut q = Popup::ask(
            ctx.cat.text(super::storage::DISCARD_ASK),
            BoxTag::Discard {
                uid: o.uid,
                item: o.item_no,
            },
        );
        q.captions = Some(super::storage::DISCARD_BUTTONS);
        ctx.popups.push(q);
    }

    /// The window's purchase check and request: a new item (`uid` 0, cshell+0x154a60) or the extension of an owned
    /// one (cshell+0x154c60).
    fn extend_request(&mut self, offer: &buybox::Offer, mode: u32, uid: u32, ctx: &mut Ctx) {
        let checked = if uid == 0 {
            buybox::mod_purchase(offer, mode, ctx.data, self.buy_busy)
        } else {
            buybox::extend_purchase(offer, mode, uid, ctx.data, self.buy_busy)
        };
        match checked {
            Ok(cmd) => {
                self.buy_busy = true;
                self.charge = match &cmd {
                    LinkCmd::BuyWith { option, .. } if offer.sale_type == 1 => {
                        Some((*option, offer.price))
                    }
                    _ => None,
                };
                ctx.send(cmd);
            }
            Err((buybox::SHORT_NX_TEXT, _)) => ctx.popups.push(buybox::short_nx_box(ctx.cat)),
            Err((text, title)) => {
                let mut q = Popup::ok(ctx.cat.text(text), BoxTag::None);
                q.title = title;
                ctx.popups.push(q);
            }
        }
    }

    /// A press while the Extend Use window is open (the window takes every press).
    fn extension_press(&mut self, p: [f32; 2], ctx: &mut Ctx) {
        let Some(x) = self.extension.as_mut() else {
            return;
        };
        match x.window.press(p) {
            BoxPress::Inside | BoxPress::Outside => {}
            BoxPress::Cancel => self.extension = None,
            BoxPress::RankClosed { .. } => {}
            BoxPress::Charge => ctx.popups.push(buybox::charge_box()),
            BoxPress::Buy { mode, .. } => {
                let (offer, uid) = (x.window.offer().clone(), x.uid);
                self.extension = None;
                self.extend_request(&offer, mode, uid, ctx);
            }
            BoxPress::Confirm => {
                let t = x.window.confirm_text(ctx.cat);
                ctx.popups
                    .push(buybox::nx_confirm_box(t, x.window.offer().product));
            }
        }
    }

    fn refresh(&mut self, data: &MenuData) {
        // DC-DEFAULT: CLI-711 the sockets first show the weapon equipped in slot 0, else the first owned weapon
        let owned: Vec<u32> = self.weapons(data).iter().map(|r| r.uid).collect();
        // DC-DEFAULT: CLI-711 the caption's count as two digits in brackets ("Weapon Select (02)", OBSERVED R33)
        let caption = format!("{}({:02})", self.weapon_caption, owned.len());
        self.sheet.set_text(WEAPON_WINDOW, SELECT_CAPTION, &caption);
        if !self.weapon.is_some_and(|w| owned.contains(&w)) {
            self.weapon = data
                .store
                .in_slot(0)
                .map(|r| r.uid)
                .filter(|u| owned.contains(u))
                .or(owned.first().copied());
        }
        self.show_weapon(data);
        fill_money(
            &mut self.sheet,
            -1,
            MONEY_TEXTS,
            data,
            self.art.as_ref(),
            &data.char_extras,
        );
    }
}

impl TuningScreen {
    /// The pane shows the shown weapon's picture (image 6 at the row's place and scale) and its name image (2, 2).
    // EVIDENCE: cshell+0x155d30 (image 6: TuningScreenIMG, its source rectangle TuningScreenIMGREC, placed and sized
    // by TuningScreenIMGPOS / REC x TuningScreenIMGScale; image (2, 2): NameforTuningScreen)
    fn show_weapon(&mut self, data: &MenuData) {
        self.show_option_pictures(data);
        let art = self
            .weapon
            .and_then(|w| data.store.items.iter().find(|r| r.uid == w))
            .and_then(|r| self.wares.art_of(r.item_no as i32))
            .cloned();
        let Some(a) = art.filter(|a| !a.tuning_picture.is_empty()) else {
            return;
        };
        if let Some(p) = self
            .sheet
            .part_mut(-1, PICTURE_ID, dc_ui_data::WidgetKind::Image)
        {
            p.look = super::sheet::Look::Panel {
                tex: a.tuning_picture.clone(),
                stretch: true,
            };
            p.pos = [a.tuning_rect[0], a.tuning_rect[1]];
            p.size = [a.tuning_rect[2], a.tuning_rect[3]];
        }
        if let Some(p) =
            self.sheet
                .part_mut(WEAPON_WINDOW, NAME_IMAGE, dc_ui_data::WidgetKind::Image)
        {
            if !a.tuning_name.is_empty() {
                p.look = super::sheet::Look::Panel {
                    tex: a.tuning_name.clone(),
                    stretch: false,
                };
            }
        }
    }

    /// Images 7 / 8 show the muzzle / sight fitted on the shown weapon (hidden without one or its picture).
    // EVIDENCE: cshell+0x152e80 (the weapon row's muzzle / sight option -1 hides image 7 / 8, else the picture of
    // OptionPictures::picture is set and shown)
    fn show_option_pictures(&mut self, data: &MenuData) {
        let weapon_no = self
            .weapon
            .and_then(|w| data.store.items.iter().find(|r| r.uid == w))
            .map(|r| r.item_no as i32);
        for (k, id) in OPTION_PICTURE_IDS.into_iter().enumerate() {
            let pic = weapon_no
                .zip(self.socket_option(data, k))
                .and_then(|(w, o)| self.pictures.picture(w, o.item_no as i32, k));
            let Some(p) = self.sheet.part_mut(-1, id, dc_ui_data::WidgetKind::Image) else {
                continue;
            };
            p.shown = pic.is_some();
            if let Some(pic) = pic {
                p.look = super::sheet::Look::Panel {
                    tex: pic.tex,
                    stretch: true,
                };
                p.pos = [pic.rect[0], pic.rect[1]];
                p.size = [pic.rect[2], pic.rect[3]];
            }
        }
    }

    /// A refilled shop page selects its first cell.
    // EVIDENCE: cshell+0x15a110 (a tab change refills the grid and selects cell 0 through cshell+0x152b10),
    // cshell+0x1595e0 / cshell+0x1598e0 (the pager does the same), cshell+0x152b10 (the selected cell fills the detail
    // panel through cshell+0xd7470)
    fn select_first(&mut self, data: &MenuData) {
        if self.selected.is_none() && self.page * CELLS < self.wares_shown(data).len() {
            self.selected = Some(0);
        }
    }
}

/// The Modification tab (AR, SR, MG, SMG, SHOTGUN, PISTOL) of an option's weapon class (`AttachItemtype`, the
/// weapon item types 0 AR, 1 SMG, 2 SR, 3 MG, 4 pistol, 5 / 0x1a shotgun).
// EVIDENCE: cshell+0x1596f0 (the switch on the item's class: 1 -> 3, 2 -> 1, 3 -> 2, 4 -> 5, 5 / 0x1a -> 4, other 0)
pub fn class_tab(class: i32) -> usize {
    match class {
        1 => 3,
        2 => 1,
        3 => 2,
        4 => 5,
        5 | 0x1a => 4,
        _ => 0,
    }
}

/// The option cells of tab `tab`: its three display groups (muzzle, sight, clip), in display order.
// DC-DEFAULT: CLI-696 (proposed) the modification tab of weapon class k lists the option cells of display groups
// 13 + 3k .. 15 + 3k (OBSERVED shop-guns/cpp/: AR 9, SR 6, MG 3, SMG 9, SHOTGUN 3, PISTOL 9)
pub fn tab_wares(wares: &Wares, store: &Store, tab: usize) -> Vec<Ware> {
    let mut v: Vec<Ware> = (0..GROUPS_PER_TAB)
        .flat_map(|k| wares.shelf(store, TAB_OPTIONS, tab * GROUPS_PER_TAB + k))
        .collect();
    v.sort_by_key(|w| w.display);
    v
}

/// CLI-1606: socket `k` (muzzle, sight, magazine) of weapon `weapon_no` is open, its weapon-table state 1 (0 no
/// socket, 2 the part is on the model as standard, such as the built-in suppressor of the L96A1 Ghillie).
// EVIDENCE: cshell+0x152d90 (attach only into a socket whose state is 1), cshell+0x17bd20 (the states'
// MuzzleOPtion / SightOption / MagazineOption keys, see storage::SOCKET_KEYS)
pub fn socket_open(wares: &Wares, weapon_no: i32, k: usize) -> bool {
    wares
        .art_of(weapon_no)
        .is_some_and(|a| a.socket_states.get(k) == Some(&1))
}

/// [`tab_wares`] for weapon `weapon` (item number) shown on tab `tab`: on the weapon's own class tab the options
/// whose socket the weapon has closed are left out, so a sniper with a built-in suppressor lists no suppressor.
// DC-DEFAULT: CLI-1606 (project decision) the 2009 shop lists the whole class (cshell+0x156c50) and refuses at
// attach (cshell+0x152d90); ours also hides what the shown weapon cannot take
pub fn weapon_tab_wares(
    wares: &Wares,
    store: &Store,
    tab: usize,
    weapon: Option<i32>,
) -> Vec<Ware> {
    let mut v = tab_wares(wares, store, tab);
    if let Some(w) = weapon.filter(|w| wares.kind_of(*w).map(class_tab) == Some(tab)) {
        v.retain(|ware| {
            match ware
                .kind_of
                .and_then(|kd| OPTION_KINDS.iter().position(|x| *x == kd))
            {
                Some(k) => socket_open(wares, w, k),
                None => true,
            }
        });
    }
    v
}

/// The label of tab `tab` with `n` options (`AR(9)`).
pub fn tab_label(cat: &Catalog, tab: usize, n: usize) -> String {
    format!("{}({n})", cat.text(TAB_NAMES[tab]))
}

impl Screen for TuningScreen {
    fn name(&self) -> &'static str {
        "tuning"
    }
    fn sheet(&self) -> &Sheet {
        &self.sheet
    }
    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheet
    }

    fn enter(&mut self, ctx: &mut Ctx) {
        if !ctx.data.store.lists_asked {
            ctx.data.store.lists_asked = true;
            ctx.send(LinkCmd::ShopLists);
        }
        self.refresh(ctx.data);
        self.follow_weapon_tab(ctx.data);
        self.goto_option(ctx.data);
        self.sync_pages(ctx.data);
    }

    fn input_seen(&mut self, ctx: &mut Ctx) {
        self.sync_pages(ctx.data);
    }

    // EVIDENCE: cshell+0x1535d0 (the cut moves 10 texels per 5 ms toward 0 (expand) or 0x54 (collapse))
    fn animate(&mut self, secs: f32) -> bool {
        let target = self.select_target();
        let Some((t0, from)) = self.select_slide else {
            return false;
        };
        let t0 = t0.unwrap_or(secs);
        self.select_slide = Some((Some(t0), from));
        let steps = ((secs - t0).max(0.0) / SLIDE_STEP_SECS) as i32;
        let d = steps.saturating_mul(SLIDE_STEP_CUT);
        let cut = if target < from {
            (from - d).max(target)
        } else {
            (from + d).min(target)
        };
        if cut == target {
            self.select_slide = None;
        }
        if cut == self.select_cut {
            return false;
        }
        self.select_cut = cut;
        self.sync_select_header();
        true
    }

    fn animating(&self) -> bool {
        self.select_slide.is_some()
    }

    fn list_rows(&self, data: &MenuData) -> Vec<(i32, i32, usize, i32)> {
        vec![(SELECT_WINDOW, 0, self.weapons(data).len(), WEAPON_ROW[3])]
    }

    fn hover(&mut self, p: [f32; 2], _data: &MenuData) {
        use dc_ui_data::WidgetKind::{Button, Image};
        let expanded = if self.select_expanded {
            self.select_open()
                || self
                    .select_part_rect(SELECT_BUTTON, Button)
                    .is_some_and(|r| inside(r, p))
        } else {
            self.select_part_rect(SELECT_FRONT, Image)
                .is_some_and(|r| inside(r, p))
        };
        if expanded != self.select_expanded {
            self.select_expanded = expanded;
            self.select_changed();
        }
    }

    fn key(&mut self, k: MenuKey, ctx: &mut Ctx) {
        if k == MenuKey::Escape {
            self.click(CMD_EXIT, usize::MAX, ctx);
        }
    }

    fn click(&mut self, command: i32, _part: usize, ctx: &mut Ctx) {
        match command {
            CMD_EXIT => *ctx.go = Some(exit_to(ctx.data)),
            CMD_BAR_SHOP | CMD_BAR_MARKET | CMD_BAR_INBOX => {
                ctx.sounds.push(SOUND_PRESS);
                *ctx.go = Some(match command {
                    CMD_BAR_SHOP => Go::Shop,
                    CMD_BAR_MARKET => Go::Market,
                    _ => Go::Inbox,
                });
            }
            // CLI-1021: the bar's clan button (see the shop page)
            super::clan::BAR_CLAN_CMD => {
                ctx.sounds.push(SOUND_PRESS);
                *ctx.go = Some(Go::Clan);
            }
            CMD_BAR_QUIT => {
                let t = ctx.cat.text(text_ids::ASK_QUIT);
                ctx.popups.push(Popup::ask(t, BoxTag::Quit));
            }
            CMD_PAGE_BACK | CMD_PAGE_FWD => {
                let pages = page_count(self.wares_shown(ctx.data).len());
                if let Some(p) = group_step(self.page, pages, command == CMD_PAGE_FWD) {
                    self.page = p;
                    self.selected = None;
                }
            }
            CMD_FIT => self.equip(ctx),
            CMD_THROW_OUT => self.discard(ctx),
            CMD_CHARGE => ctx.popups.push(buybox::charge_box()),
            CMD_MORE_TIME => {
                if let Some(o) = self.picked_record(ctx.data) {
                    self.extension = Extension::open(ctx.cat, &self.wares, &ctx.data.store, &o);
                    if self.extension.is_none() {
                        ctx.notice(0x3d2);
                    }
                }
            }
            CMD_CLOSE_STORAGE => self.open_storage(None, ctx.data),
            CMD_PICK_GUN => {
                let open = !self.select_open();
                self.sheet.show_window(SELECT_WINDOW, open);
                if !open {
                    self.select_expanded = false;
                    self.select_changed();
                }
            }
            CMD_BUY => {
                let shown = self.wares_shown(ctx.data);
                match self.selected.and_then(|i| shown.get(self.page * CELLS + i)) {
                    // CLI-1172 (playtest report: "purchasing mods did not give option for various expiration
                    // times"): Buy opens the purchase window with the cell's products (its period list), as the
                    // shop does; it asked a yes / no question for one product
                    // EVIDENCE: cshell+0x15a110 (1007 -> cshell+0x1dae90 with the cell's item and type)
                    Some(w) if !self.buy_busy => {
                        self.buying = Some(w.clone());
                        self.extension = Extension::new_purchase(
                            ctx.cat,
                            &self.wares,
                            &ctx.data.store,
                            w.item_no,
                            w.sale_type,
                        );
                        // CLI-1630: the cell's rank requirement (the window's OK on a GP mod shows the GP form)
                        if let Some(x) = self.extension.as_mut() {
                            buybox::set_rank(
                                &mut x.window,
                                ctx.cat,
                                &self.wares,
                                &ctx.data.store,
                                w.sale_type,
                                ctx.data.profile.exp,
                            );
                        }
                        if self.extension.is_none() {
                            ctx.notice(0x3d2);
                        }
                    }
                    Some(_) => {}
                    None => ctx.notice(0x3d2),
                }
            }
            c => {
                if let Some(t) = CMD_TABS.iter().position(|x| *x == c) {
                    self.tab = t;
                    self.page = 0;
                    self.selected = None;
                }
            }
        }
        self.sync_pages(ctx.data);
    }

    fn press(&mut self, p: [f32; 2], ctx: &mut Ctx) -> bool {
        if self.extension.is_some() {
            self.extension_press(p, ctx);
            return true;
        }
        if self.select_open() {
            let weapons: Vec<u32> = self.weapons(ctx.data).iter().map(|r| r.uid).collect();
            if let Some(k) =
                (0..weapons.len()).find(|&k| self.weapon_row(k).is_some_and(|r| inside(r, p)))
            {
                self.weapon = Some(weapons[k]);
                self.follow_weapon_tab(ctx.data);
                self.sheet.show_window(SELECT_WINDOW, false);
                self.select_expanded = false;
                self.select_changed();
                return true;
            }
        } else if let Some(k) = (0..3).find(|&k| inside(row(SOCKET_ROW, SOCKET_PITCH, k), p)) {
            self.open_storage(Some(k), ctx.data);
            return true;
        }
        if let Some(k) = self.socket {
            let opts: Vec<u32> = self.options(ctx.data, k).iter().map(|r| r.uid).collect();
            if let Some(i) = (0..opts.len().min(self.store_cells.len()))
                .find(|&i| inside(self.store_cells[i], p))
            {
                self.picked = Some(opts[i]);
                self.equip_caption(ctx.data);
                return true;
            }
            return false;
        }
        let n = self.wares_shown(ctx.data).len();
        if let Some(i) = self
            .cells
            .iter()
            .position(|r| inside(*r, p))
            .filter(|i| self.page * CELLS + i < n)
        {
            self.selected = Some(i);
            return true;
        }
        if let Some(pg) = page_hit(&self.sheet, &PAGE_IDS, self.page, page_count(n), p) {
            self.page = pg;
            self.selected = None;
            return true;
        }
        false
    }

    fn popup_done(&mut self, p: &Popup, yes: bool, ctx: &mut Ctx) {
        match (p.tag, yes) {
            (BoxTag::NxShort, true) => ctx.popups.push(buybox::charge_box()),
            (BoxTag::Discard { uid, item }, true) => {
                self.discarding = Some(uid);
                ctx.send(LinkCmd::Discard { uid, item });
            }
            // the credit confirmation of the Extend Use window, then its last question (0x17dd)
            (BoxTag::Buy(product), true)
                if self
                    .extension
                    .as_ref()
                    .is_some_and(|x| x.window.offer().product == product) =>
            {
                ctx.popups.push(buybox::nx_last_ask(ctx.cat, product));
            }
            (BoxTag::BuyNx(product), true)
                if self
                    .extension
                    .as_ref()
                    .is_some_and(|x| x.window.offer().product == product) =>
            {
                if let Some(x) = self.extension.take() {
                    self.extend_request(&x.window.offer().clone(), x.window.mode, x.uid, ctx);
                }
            }
            (BoxTag::Buy(product), true) => ctx.send(LinkCmd::Buy(product)),
            _ => {}
        }
    }

    fn dropdown(&self) -> Option<&Dropdown> {
        self.extension.as_ref().and_then(|x| x.window.drop.as_ref())
    }

    fn dropdown_mut(&mut self) -> Option<&mut Dropdown> {
        self.extension.as_mut().and_then(|x| x.window.drop.as_mut())
    }

    fn close_dropdown(&mut self) {
        if let Some(x) = self.extension.as_mut() {
            x.window.drop = None;
        }
    }

    // EVIDENCE: cshell+0x1dab70 (0x3e9: the picked row -> cshell+0x1d9de0)
    fn dropdown_pick(&mut self, _list: &'static str, _value: i32, index: usize, _ctx: &mut Ctx) {
        if let Some(x) = self.extension.as_mut() {
            x.window.pick(index);
        }
    }

    fn event(&mut self, e: &LinkEvent, ctx: &mut Ctx) {
        match e {
            LinkEvent::Balance { .. }
            | LinkEvent::Grade(_)
            | LinkEvent::CharInfo(_)
            | LinkEvent::Profile(_) => self.refresh(ctx.data),
            // the reply edits the client's own inventory: the option's parent is the weapon (one option per kind and
            // weapon) or nothing
            // EVIDENCE: cshell+0x34b10 / cshell+0x34b70 -> cshell+0x158a50 (ok: the option filed under the weapon,
            // the views redrawn, the request flag cleared) / cshell+0x158b80
            LinkEvent::AttachDone { ok, detach } => {
                let Some((opt, weapon, k, d)) = self.pending.take() else {
                    return;
                };
                if !*ok || *detach != d {
                    return;
                }
                let kind = OPTION_KINDS[k];
                for r in ctx.data.store.items.iter_mut() {
                    if r.uid == opt {
                        r.parent_uid = if d { 0 } else { weapon };
                    } else if !d
                        && r.parent_uid == weapon
                        && (1000..2000).contains(&r.item_no)
                        && self.wares.kind_of(r.item_no as i32) == Some(kind)
                    {
                        r.parent_uid = 0;
                    }
                }
                if !d {
                    ctx.sounds.push(ATTACH_SOUNDS[k]);
                }
                self.equip_caption(ctx.data);
            }
            LinkEvent::Bought(r) => {
                // EVIDENCE: cshell+0x150c40 (a returned unique id already held -> res 0x438 "extended")
                let extended =
                    r.ok && r.uid != 0 && ctx.data.store.items.iter().any(|i| i.uid == r.uid);
                let credit = self.buying.as_ref().is_some_and(|w| w.sale_type == 1);
                // CLI-1432: the Modification page's own reply boxes, titled 0x371: a refusal 0x3c -> 0x503, any
                // other -> 0x46f; a credit product's success shows no box (it waits in the inbox); else 0x438 for a
                // held unique id, 0x439 for a new one
                // EVIDENCE: cshell+0x1588a0 (ok 0: reason 0x3c -> 0x503 else 0x46f; ok: only when the bought
                // product's sale type (+0x18) is not 1, box 0x438 / 0x439 by whether the unique id is held)
                let boxed = if !r.ok {
                    Some(if r.reason == 0x3c { 0x503 } else { 0x46f })
                } else if credit {
                    None
                } else if extended {
                    Some(0x438)
                } else {
                    Some(bought_text(true, 0, false))
                };
                // CLI-1630: the lobby's rank refusal (0x60, LG-302) of a mod the character's rank does not reach shows
                // the rank box
                let rank_refused = self
                    .buying
                    .as_ref()
                    .filter(|w| {
                        !r.ok
                            && r.reason == 0x60
                            && super::shop::locked(w.unlock_exp, ctx.data.profile.exp)
                    })
                    .map(|w| buybox::rank_of_exp(ctx.cat, w.unlock_exp).0);
                if let Some(name) = rank_refused {
                    ctx.popups.push(buybox::rank_box(&name));
                } else if let Some(t) = boxed {
                    let mut q = Popup::ok(ctx.cat.text(t), BoxTag::None);
                    q.title = text_ids::TITLE_NOTICE;
                    ctx.popups.push(q);
                }
                // DC-DEFAULT: CLI-880 (user: "buying a mod should auto attach it to the selected gun"): the 2009 reply
                // handler cshell+0x1588a0 shows box 0x439 whose OK runs command 0x3f2, the question 0x36b (buttons
                // 0x1564 "Attach" / 0x36a), and its Yes runs command 0x3f3 -> cshell+0x154e90 (the bought item at
                // +0x5570 on the shown weapon, fit boxes 0x4df / 0x4e0, then 0x336). We attach at once, without
                // the question, when a weapon is shown and the mod is new.
                if r.ok && !extended && !credit && r.uid != 0 {
                    self.auto_attach = self
                        .buying
                        .as_ref()
                        .filter(|w| (1000..2000).contains(&w.item_no))
                        .map(|w| (r.uid, w.item_no as u32));
                }
                // DC-DEFAULT: CLI-1875 (proposed) an accepted credit purchase here takes its price off the balance
                // it was charged to, as the shop's answer does (cshell+0x150c40); the 2009 handler of this page
                // (cshell+0x1588a0) is not traced for it. LG-405's 0x345 after the answer sets the lobby's own.
                if let Some((option, price)) = self.charge.take().filter(|_| r.ok) {
                    buybox::deduct(ctx.data, option, price);
                    self.refresh(ctx.data);
                }
                self.buy_busy = false;
                self.buying = None;
            }
            // EVIDENCE: cshell+0x158cb0 (ok: the picked record leaves the inventory, the views are rebuilt)
            LinkEvent::Discarded { ok } => {
                if let (true, Some(uid)) = (*ok, self.discarding) {
                    ctx.data.store.remove(uid);
                    if self.picked == Some(uid) {
                        self.picked = None;
                    }
                    self.equip_caption(ctx.data);
                }
                self.discarding = None;
            }
            _ => {}
        }
        self.try_auto_attach(ctx);
        self.goto_option(ctx.data);
        self.sync_pages(ctx.data);
    }

    fn draw_extra(&self, cat: &mut Catalog, data: &MenuData, secs: f32, out: &mut Vec<DrawItem>) {
        self.draw_screen(cat, data, secs, out);
        self.draw_select_list(cat, data, out);
        if let Some(x) = &self.extension {
            x.window.draw(cat, &self.buy_art, data, out);
        }
    }
}

impl TuningScreen {
    /// The open weapon select list over the screen: its window's panel again (it covers the shop below, as the 2009
    /// window 5 does) and its rows.
    // OBSERVED: shop-polish3/cpp_tun/shop_bb03_tun_list_away.png (the open list covers the tabs
    // and the shop cells)
    fn draw_select_list(&self, cat: &mut Catalog, data: &MenuData, out: &mut Vec<DrawItem>) {
        if !self.select_open() {
            return;
        }
        if let Some(w) = self
            .sheet
            .parts
            .iter()
            .find(|p| p.kind == dc_ui_data::WidgetKind::Window && p.layer == SELECT_WINDOW)
        {
            if let super::sheet::Look::Panel { tex, .. } = &w.look {
                out.push(image_fit(cat, tex, w.pos, w.size));
            }
        }
        for (k, w) in self.weapons(data).iter().enumerate() {
            if let Some(r) = self.weapon_row(k) {
                self.draw_weapon_row(cat, data, r, w, out);
            }
        }
        // the list's scroll bar (rows shown from the bar's value on)
        if let Some(i) = self
            .sheet
            .find(SELECT_WINDOW, 0, dc_ui_data::WidgetKind::List)
        {
            if let Some(b) = self.sheet.list_bar(i) {
                b.draw(cat, out);
            }
        }
    }

    /// Row `k` of the weapon select list: the list's row geometry under its scroll bar (rows 0x56 high, 11 apart;
    /// a row that does not fit whole is not shown).
    // EVIDENCE: cshell+0x2f0430 (rows from the bar's value, each at its top less the value, down to the last that
    // fits the list's height), the list (5, 0) is a scrolling list (scroll bar at +266, 475 high)
    fn weapon_row(&self, k: usize) -> Option<[i32; 4]> {
        let i = self
            .sheet
            .find(SELECT_WINDOW, 0, dc_ui_data::WidgetKind::List)?;
        let at = super::sheet::list_row(&self.sheet.parts[i], WEAPON_ROW[3], k, 0)?;
        Some([at[0], at[1], WEAPON_ROW[2], WEAPON_ROW[3]])
    }

    /// A row of the weapon select list (see [`WEAPON_ROW`]).
    fn draw_weapon_row(
        &self,
        cat: &mut Catalog,
        data: &MenuData,
        r: [i32; 4],
        w: &InvRecord,
        out: &mut Vec<DrawItem>,
    ) {
        let lit = self.weapon == Some(w.uid);
        out.push(image_fit(
            cat,
            WEAPON_FRAMES[lit as usize],
            [r[0], r[1]],
            [r[2], r[3]],
        ));
        if let Some(icon) = self
            .wares
            .art_of(w.item_no as i32)
            .map(|a| a.icon.as_str())
            .filter(|t| !t.is_empty() && cat.tex_size(t).is_some())
        {
            out.push(image_fit(cat, icon, [r[0], r[1]], [r[2], r[3]]));
        }
        if let Some((t, size)) = self.wares.kind_of(w.item_no as i32).and_then(class_tag) {
            out.push(image_fit(cat, t, [r[0] + TAG_X, r[1] + TAG_Y], size));
        }
        out.push(place_text(
            &label(&self.wares, w),
            [r[0] + CELL_NAME_AT[0], r[1] + CELL_NAME_AT[1]],
            [0, 0],
            Align {
                first: super::sheet::ALIGN_LEFT,
                second: 0,
            },
            CELL_FONT,
            CELL_SIZE,
            CELL_COLOUR,
        ));
        if data.store.equipped.contains(&(w.item_no as i32)) {
            out.push(image_fit(
                cat,
                EQUIPPED_MARK.0,
                [r[0] + r[2] - 1 - EQUIPPED_MARK.1[0], r[1] + 1],
                EQUIPPED_MARK.1,
            ));
        }
    }

    fn draw_screen(&self, cat: &mut Catalog, data: &MenuData, _secs: f32, out: &mut Vec<DrawItem>) {
        // the tabs' labels: class name and option count
        // DC-DEFAULT: CLI-696 (proposed) the labels are drawn centred on the tab buttons in font 14 size 13, the
        // current tab's in the own-row colour (the 2009 label setter cshell+0x43dd60 is not traced)
        for (t, cmd) in CMD_TABS.iter().enumerate() {
            if let Some(b) = self.sheet.button(*cmd) {
                let n = tab_wares(&self.wares, &data.store, t).len();
                let c = if t == self.tab {
                    OWN_COLOUR
                } else {
                    ROW_COLOUR
                };
                out.push(place_text(
                    &tab_label(cat, t, n),
                    b.pos,
                    b.size,
                    TAB_ALIGN,
                    14,
                    13,
                    c,
                ));
            }
        }
        let name_of = |uid: u32| {
            data.store
                .items
                .iter()
                .find(|r| r.uid == uid)
                .map(|r| label(&self.wares, r))
                .unwrap_or_default()
        };
        // the three sockets (the weapon's name is the pane's name image, see show_weapon)
        let weapon_art = self
            .weapon
            .and_then(|w| data.store.items.iter().find(|r| r.uid == w))
            .and_then(|r| self.wares.art_of(r.item_no as i32));
        // the socket list (2, 5) is hidden while the weapon select list is open
        // EVIDENCE: cshell+0x152d20 (control (2, 5) vtable +0x98 with !open)
        for k in (0..3).filter(|_| !self.select_open()) {
            let r = row(SOCKET_ROW, SOCKET_PITCH, k);
            let lit = self.socket == Some(k);
            out.push(image_fit(
                cat,
                SOCKET_FRAMES[lit as usize],
                [r[0], r[1]],
                SOCKET_FRAME_SIZE,
            ));
            let on = self.socket_option(data, k);
            let (tex, text) = match on {
                Some(o) => (
                    self.wares
                        .art_of(o.item_no as i32)
                        .map(|a| a.icon.clone())
                        .unwrap_or_default(),
                    label(&self.wares, o),
                ),
                None => (
                    if weapon_art.is_some_and(|a| a.socket_states[k] == 1) {
                        SOCKET_EMPTY[k].to_string()
                    } else {
                        SOCKET_DISABLED.to_string()
                    },
                    self.socket_texts[k].clone(),
                ),
            };
            if !tex.is_empty() && cat.tex_size(&tex).is_some() {
                out.push(image_fit(
                    cat,
                    &tex,
                    [r[0] + SOCKET_IMAGE_AT[0], r[1] + SOCKET_IMAGE_AT[1]],
                    SOCKET_IMAGE_SIZE,
                ));
            }
            out.push(place_text(
                &text,
                [r[0] + SOCKET_LABEL_AT[0], r[1] + SOCKET_LABEL_AT[1]],
                [0, 0],
                Align {
                    first: super::sheet::ALIGN_LEFT,
                    second: 0,
                },
                CELL_FONT,
                CELL_SIZE,
                0xffff_ffff,
            ));
        }
        if let Some(k) = self.socket {
            let opts = self.options(data, k);
            for (i, r) in self.store_cells.iter().enumerate() {
                let o = opts.get(i);
                let lit = o.is_some_and(|o| self.picked == Some(o.uid));
                out.push(image_fit(
                    cat,
                    STORE_FRAMES[lit as usize],
                    [r[0], r[1]],
                    [r[2], r[3]],
                ));
                let Some(o) = o else {
                    continue;
                };
                let art = self.wares.art_of(o.item_no as i32);
                if let Some(t) = art
                    .map(|a| a.icon.as_str())
                    .filter(|t| !t.is_empty() && cat.tex_size(t).is_some())
                {
                    out.push(image_fit(
                        cat,
                        t,
                        [r[0] + TUNING_IMAGE_AT[0], r[1] + TUNING_IMAGE_AT[1]],
                        TUNING_IMAGE_SIZE,
                    ));
                }
                out.push(place_text(
                    &label(&self.wares, o),
                    [r[0] + CELL_NAME_AT[0], r[1] + CELL_NAME_AT[1]],
                    [0, 0],
                    Align {
                        first: super::sheet::ALIGN_LEFT,
                        second: 0,
                    },
                    CELL_FONT,
                    CELL_SIZE,
                    CELL_COLOUR,
                ));
                if o.parent_uid != 0 {
                    out.push(image_fit(
                        cat,
                        ATTACHED_MARK.0,
                        [r[0] + r[2] - 1 - ATTACHED_MARK.1[0], r[1] + 1],
                        ATTACHED_MARK.1,
                    ));
                    out.push(place_text(
                        &name_of(o.parent_uid),
                        [r[0] + r[2] + PARENT_NAME_AT[0], r[1] + PARENT_NAME_AT[1]],
                        [0, 0],
                        Align {
                            first: ALIGN_RIGHT,
                            second: 0,
                        },
                        PARENT_NAME_FONT,
                        0xe,
                        PARENT_NAME_COLOUR,
                    ));
                }
            }
            // the picked option's detail panel
            // EVIDENCE: cshell+0x1560b0 (a picked storage cell fills the panel through cshell+0xd7470 as an owned
            // item: no LOCKED, badge or PERMANENT)
            if let Some(o) = self
                .picked
                .and_then(|u| data.store.items.iter().find(|r| r.uid == u))
            {
                let name = label(&self.wares, o);
                modpanel::draw(
                    cat,
                    &modpanel::PanelItem {
                        name: &name,
                        art: self.wares.art_of(o.item_no as i32),
                        tag_kind: self.wares.kind_of(o.item_no as i32),
                        locked: false,
                        badge: None,
                        permanent: false,
                    },
                    out,
                );
            }
            return;
        }
        let shown = self.wares_shown(data);
        let rank = self.art.as_ref().map(|a| super::shop::CellRank {
            levels: &a.levels,
            exp: data.profile.exp,
        });
        for (i, r) in self.cells.iter().enumerate() {
            match shown.get(self.page * CELLS + i) {
                Some(w) => draw_ware_cell(
                    cat,
                    *r,
                    w,
                    self.selected == Some(i),
                    TUNING_FRAMES,
                    WARE_PRICE_AT,
                    None,
                    rank.as_ref(),
                    out,
                ),
                None => out.push(image_fit(cat, TUNING_FRAMES[0], [r[0], r[1]], [r[2], r[3]])),
            }
        }
        if let Some(w) = self.selected.and_then(|i| shown.get(self.page * CELLS + i)) {
            let locked = rank
                .as_ref()
                .is_some_and(|k| super::shop::locked(w.unlock_exp, k.exp));
            modpanel::draw(
                cat,
                &modpanel::PanelItem {
                    name: &w.name,
                    art: self.wares.art_of(w.item_no),
                    tag_kind: w.tag_kind,
                    locked,
                    badge: rank
                        .as_ref()
                        .and_then(|k| super::shop::badge_of(k.levels, w.unlock_exp)),
                    permanent: w.permanent(),
                },
                out,
            );
        }
        draw_page_numbers(
            &self.sheet,
            &PAGE_IDS,
            self.page,
            page_count(shown.len()),
            out,
        );
    }
}

#[cfg(test)]
mod balance_tests {
    use super::super::link::{BuyReply, Link};
    use super::*;

    /// CLI-1875: a credit purchase on the Modification page is checked on the balance the lobby charges (the prepaid
    /// one for either mode) and its accepted answer takes the price off it; a refusal takes nothing.
    #[test]
    fn a_credit_purchase_here_is_checked_and_deducted_on_the_charged_balance() {
        let cat = Catalog::fallback(None, 1024, 768);
        let mut data = MenuData {
            balance: (0, 5000),
            ..Default::default()
        };
        let mut s = TuningScreen::new(&cat, &data);
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
        let mod_offer = buybox::Offer {
            product: 1300,
            price: 300,
            sale_type: 1,
            bonus: 0,
            consume: 1,
            period: 30,
            items: vec![1300],
        };
        s.extend_request(&mod_offer, buybox::MODE_CREDIT, 0, &mut ctx);
        let sent: Vec<String> = link.take_sent().iter().map(|c| format!("{c:?}")).collect();
        assert_eq!(sent.len(), 1, "the credit balance of 0 does not refuse it");
        let ok = BuyReply {
            ok: true,
            award: -1,
            ..Default::default()
        };
        s.event(&LinkEvent::Bought(ok), &mut ctx);
        assert_eq!(ctx.data.balance, (0, 4700));
        s.extend_request(&mod_offer, buybox::MODE_PREPAID, 0, &mut ctx);
        s.event(
            &LinkEvent::Bought(BuyReply {
                ok: false,
                reason: 0x3b,
                ..ok
            }),
            &mut ctx,
        );
        assert_eq!(ctx.data.balance, (0, 4700));
    }
}
