//! shop-pane: the left pane of the "Shop / Storage" screen (window 2): the equipment rows of the weapons and body
//! tabs as the 2009 list items draw them (row image, the item's slot image, the name), the weapons tab's backpack
//! row ("A").
// EVIDENCE: cshell+0x1425d0 (the pane's lists are filled with list items of their own images: weapons list 2 four
// items 1st..4th_Slot_Off / _On, 0xc2 x 0x47; list 3 the backpack items, one Backpack_Default_off / _on 0x9f x 0x31
// while the character has no backpack slots (+0x2dc < 1); body list 4 six items MyItemSlot_Off / _On 0xc2 x 0x47)

use super::catalog::Catalog;
use super::link::InvRecord;
use super::scrollbar::ScrollBar;
use super::sheet::{image_fit, list_row, place_text, Part, ALIGN_LEFT};
use super::storage::{Store, Wares};
use crate::hud::items::DrawItem;
use dc_ui_data::pages::Align;

/// A row of the weapons and body lists (0xc2 x 0x47).
pub const ROW_SIZE: [i32; 2] = [0xc2, 0x47];
/// The row images: the weapons list's four rows (normal state), the body list's rows.
pub const WEAPON_ROW_IMAGES: [&str; 4] = [
    "UI_SCR_Page_Shop\\Inven\\1st_Slot_Off.dtx",
    "UI_SCR_Page_Shop\\Inven\\2nd_Slot_Off.dtx",
    "UI_SCR_Page_Shop\\Inven\\3rd_Slot_Off.dtx",
    "UI_SCR_Page_Shop\\Inven\\4th_Slot_Off.dtx",
];
pub const BODY_ROW_IMAGE: &str = "UI_SCR_Page_Shop\\Inven\\MyItemSlot_Off.dtx";
/// The rows' second images, shown while the pointer is over the row (CLI-971).
// EVIDENCE: cshell+0x1425d0 (every pane list item gets an _Off and an _On image), cshell+0x1d9ad0 (the item keeps
// them at +0xfc / +0x100), cshell+0x1da3f0 (pointer over the item: its vtable +0x84; off it: +0x88)
// DC-DEFAULT: CLI-971 +0x84 shows the _On image and +0x88 the _Off image (the two vtable entries are not traced)
pub const WEAPON_ROW_ON_IMAGES: [&str; 4] = [
    "UI_SCR_Page_Shop\\Inven\\1st_Slot_On.dtx",
    "UI_SCR_Page_Shop\\Inven\\2nd_Slot_On.dtx",
    "UI_SCR_Page_Shop\\Inven\\3rd_Slot_On.dtx",
    "UI_SCR_Page_Shop\\Inven\\4th_Slot_On.dtx",
];
pub const BODY_ROW_ON_IMAGE: &str = "UI_SCR_Page_Shop\\Inven\\MyItemSlot_On.dtx";
/// The weapons tab's backpack rows (list 3): one default row while the character has no backpack slots, else one
/// row per slot (A..D), 0x9f x 0x31, 3 px apart (the list's item spacing).
pub const BACKPACK_ROW_IMAGE: &str = "UI_SCR_Page_Shop\\Inven\\Backpack_Default_off.dtx";
pub const BACKPACK_SLOT_IMAGES: [&str; 4] = [
    "UI_SCR_Page_Shop\\Inven\\Backpack_Slot_A_OFF.dtx",
    "UI_SCR_Page_Shop\\Inven\\Backpack_Slot_B_OFF.dtx",
    "UI_SCR_Page_Shop\\Inven\\Backpack_Slot_C_OFF.dtx",
    "UI_SCR_Page_Shop\\Inven\\Backpack_Slot_D_OFF.dtx",
];
pub const BACKPACK_ROW_ON_IMAGE: &str = "UI_SCR_Page_Shop\\Inven\\Backpack_Default_on.dtx";
pub const BACKPACK_SLOT_ON_IMAGES: [&str; 4] = [
    "UI_SCR_Page_Shop\\Inven\\Backpack_Slot_A_ON.dtx",
    "UI_SCR_Page_Shop\\Inven\\Backpack_Slot_B_ON.dtx",
    "UI_SCR_Page_Shop\\Inven\\Backpack_Slot_C_ON.dtx",
    "UI_SCR_Page_Shop\\Inven\\Backpack_Slot_D_ON.dtx",
];
pub const BACKPACK_ROW_SIZE: [i32; 2] = [0x9f, 0x31];
/// The backpack rows' pitch: the row's height and the list's spacing.
pub const BACKPACK_PITCH: i32 = 0x31 + 3;

/// CLI-972 the backpack slot rows' drawer: a closed row shows an arrow at its right end; the pointer on the row slides
/// a drawer out to the right (the slide image revealed 12 px a step), which then shows three buttons Primary /
/// Secondary / Throw; a press on one sends 10000 / 10001 / 10002 with the row to the screen, which opens that
/// storage page (cshell+0x1509ba). The pointer off the row and its buttons slides the drawer back (20 px a step).
// EVIDENCE: cshell+0x1425d0 (backpack slot rows: list items of mode 1, row index at +0x17c, the screen as owner
// through cshell+0x1d9360), cshell+0x1d9ad0 (mode 1: arrow Backpack_Slide_Arrow 8 x 0x31, slide image
// Backpack_Slide_Over_Image 0xa0 x 0x3d, state 3 (closed) with the slide at 0xa0, three buttons from the prefixes
// Backpack_Primary_ / Backpack_Secondary_ / Backpack_Throw_ (a / b / d / c images through cshell+0x1d9810) of
// 0x31 x 0x3d, 0x31 x 0x3d, 0x3a x 0x3d), cshell+0x1da230 (arrow at (x + 0x9f, y), slide and first button at
// (x + 0xa1, y - 0xc), the others at x + 0xd4 / x + 0x107), cshell+0x1d97c0 (pointer on: state 2 when open else 0;
// off: 3 when closed else 1), cshell+0x1daa10 (state 0: arrow hidden, slide shown, after 50 ms the slide's hidden
// left part shrinks by 12 each update down to 0, then state 2 and the buttons shown (cshell+0x1d9920); state 1:
// buttons hidden, slide shown, +20 each update up to 0xa0, then state 3 and the arrow shown (cshell+0x1d97a0); states
// 2 and 3 hide the slide), cshell+0x1d9390 (the row's hit test takes its buttons too), cshell+0x1d94f0 (a button
// press sends 10000 / 0x2711 / 0x2712 with +0x17c to the owner)
// DC-DEFAULT: CLI-972 one slide step per 1/60 s after the 2009 code's 50 ms wait (the 2009 code steps once per update,
// so its speed follows the frame rate); the buttons use the layout buttons' letters a / b / c for normal / over /
// down (the 2009 order passed is a, b, d, c)
pub const SLIDE_ARROW: &str = "UI_SCR_Page_Shop\\Inven\\Backpack_Slide_Arrow.dtx";
pub const SLIDE_ARROW_AT: [i32; 4] = [0x9f, 0, 8, 0x31];
pub const SLIDE_IMAGE: &str = "UI_SCR_Page_Shop\\Inven\\Backpack_Slide_Over_Image.dtx";
pub const SLIDE_AT: [i32; 2] = [0xa1, -0xc];
pub const SLIDE_SIZE: [i32; 2] = [0xa0, 0x3d];
pub const SLIDE_OPEN_STEP: i32 = 12;
pub const SLIDE_CLOSE_STEP: i32 = 20;
pub const SLIDE_WAIT_MS: f32 = 50.0;
pub const SLIDE_TICK_MS: f32 = 1000.0 / 60.0;
/// The drawer's buttons: image prefix, place (x, y, w, h relative to the row) and command.
pub const SLIDE_BUTTONS: [(&str, [i32; 4], i32); 3] = [
    (
        "UI_SCR_Page_Shop\\Inven\\Backpack_Primary_",
        [0xa1, -0xc, 0x31, 0x3d],
        10000,
    ),
    (
        "UI_SCR_Page_Shop\\Inven\\Backpack_Secondary_",
        [0xd4, -0xc, 0x31, 0x3d],
        10001,
    ),
    (
        "UI_SCR_Page_Shop\\Inven\\Backpack_Throw_",
        [0x107, -0xc, 0x3a, 0x3d],
        10002,
    ),
];
/// The storage category each drawer button opens: primary, secondary, support (throw).
// DISASM: cshell+0x1509ba..0x150acf (10000 / 0x2711 / 0x2712: +0x55a8 = the row, the storage page (cshell+0x14b8a0(1)),
// category 0 / 1 / 3 through cshell+0x14a700)
pub const SLIDE_CATEGORIES: [usize; 3] = [0, 1, 3];

/// A drawer's state (cshell+0x1daa10's states 0 .. 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slide {
    Opening,
    Closing,
    Open,
    Closed,
}

/// The drawer of one backpack slot row: its state and the hidden left part of the slide image (0..0xa0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drawer {
    pub state: Slide,
    pub hidden: i32,
    /// The hidden part and the clock (ms) when the running slide began.
    from: i32,
    since: Option<f32>,
}

impl Default for Drawer {
    fn default() -> Drawer {
        Drawer {
            state: Slide::Closed,
            hidden: SLIDE_SIZE[0],
            from: SLIDE_SIZE[0],
            since: None,
        }
    }
}

impl Drawer {
    /// The pointer came onto the row.
    pub fn enter(&mut self) {
        if !matches!(self.state, Slide::Open | Slide::Opening) {
            self.state = Slide::Opening;
            self.since = None;
        }
    }

    /// The pointer left the row and its buttons.
    pub fn leave(&mut self) {
        if !matches!(self.state, Slide::Closed | Slide::Closing) {
            self.state = Slide::Closing;
            self.since = None;
        }
    }

    pub fn moving(&self) -> bool {
        matches!(self.state, Slide::Opening | Slide::Closing)
    }

    /// Moves a running slide to the clock `now_ms` (true: the picture changed).
    pub fn step(&mut self, now_ms: f32) -> bool {
        if !self.moving() {
            return false;
        }
        let Some(t0) = self.since else {
            self.since = Some(now_ms);
            self.from = self.hidden;
            return true;
        };
        let waited = now_ms - t0 - SLIDE_WAIT_MS;
        let steps = if waited > 0.0 {
            (waited / SLIDE_TICK_MS).floor() as i32 + 1
        } else {
            0
        };
        let before = (self.state, self.hidden);
        if self.state == Slide::Opening {
            self.hidden = (self.from - SLIDE_OPEN_STEP * steps).max(0);
            if self.hidden == 0 {
                self.state = Slide::Open;
                self.since = None;
            }
        } else {
            self.hidden = (self.from + SLIDE_CLOSE_STEP * steps).min(SLIDE_SIZE[0]);
            if self.hidden == SLIDE_SIZE[0] {
                self.state = Slide::Closed;
                self.since = None;
            }
        }
        before != (self.state, self.hidden)
    }
}

/// The pointer state of the pane's rows: the equipment row and the backpack row under the pointer, the backpack rows'
/// drawers and the drawer button under the pointer (row, button).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PaneLook {
    pub row: Option<usize>,
    pub pack: Option<usize>,
    pub drawers: [Drawer; 4],
    pub button: Option<(usize, usize)>,
    /// The filled backpack row whose unequip X is under the pointer.
    pub x: Option<usize>,
}

/// CLI-1160 the unequip X of a filled backpack row: the close-window glyph (images A normal / B pointer over, 16 x 16)
/// at the row's right end, 0x13 left of its width, 2 px below its top. A press on it takes the weapon of backpack
/// slot 10 + row off (unequip) and the row empties.
// EVIDENCE: cshell+0xd9ad0 (list item: the X is a button of the images Close_Window_A / B / C / D of the popup button folder,
// hidden when built), cshell+0xda230 (placed at x + width - 0x13, y + 2, width = 0x9f for a backpack row), cshell+0xd9780
// (shown by cshell+0x147100 for a backpack slot that holds an item, hidden for an empty one), cshell+0x14d6b0..0x14d8d8
// (the shop screen's press: the list item under the pointer, its X hit-tested, then cshell+0x1467b0 with the row)
// EVIDENCE: cshell+0x1467b0 (the weapon of backpack slot 10 + row goes to cshell+0x145ed0, the unequip request:
// item, uid, slot; sender cshell+0x18af30)
// OBSERVED: ROM Close_Window_A..D.dtx are 16 x 16
pub const UNEQUIP_X: &str = r"UI_SCR_PopUp\Popup\Button\Close_Window_";
pub const UNEQUIP_X_AT: [i32; 4] = [0x9f - 0x13, 2, 16, 16];

/// The X rectangle of the backpack row at `row`.
pub fn unequip_x_rect(row: [i32; 4]) -> [i32; 4] {
    let a = UNEQUIP_X_AT;
    [row[0] + a[0], row[1] + a[1], a[2], a[3]]
}

/// The backpack row whose X lies under `p` among the first `rows` rows of the list at `at` (a filled row is the
/// caller's check).
pub fn unequip_x_at(at: [i32; 2], rows: usize, p: [f32; 2]) -> Option<usize> {
    (0..rows.min(4)).find(|i| inside(unequip_x_rect(pack_row_rect(at, *i)), p))
}

/// Backpack row `i` of the list at `at`.
pub fn pack_row_rect(at: [i32; 2], i: usize) -> [i32; 4] {
    [
        at[0],
        at[1] + i as i32 * BACKPACK_PITCH,
        BACKPACK_ROW_SIZE[0],
        BACKPACK_ROW_SIZE[1],
    ]
}

/// Button `k` of the drawer of the row at `row`.
pub fn drawer_button_rect(row: [i32; 4], k: usize) -> [i32; 4] {
    let b = SLIDE_BUTTONS[k].1;
    [row[0] + b[0], row[1] + b[1], b[2], b[3]]
}

fn inside(r: [i32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] as f32
        && p[1] >= r[1] as f32
        && p[0] < (r[0] + r[2]) as f32
        && p[1] < (r[1] + r[3]) as f32
}

/// The open drawer button under `p` (row, button) of `rows` backpack slot rows at `at`.
pub fn drawer_button_at(
    at: [i32; 2],
    rows: usize,
    look: &PaneLook,
    p: [f32; 2],
) -> Option<(usize, usize)> {
    (0..rows.min(4)).find_map(|i| {
        if look.drawers[i].state != Slide::Open {
            return None;
        }
        let r = pack_row_rect(at, i);
        (0..SLIDE_BUTTONS.len())
            .find(|k| inside(drawer_button_rect(r, *k), p))
            .map(|k| (i, k))
    })
}

/// CLI-1090 the area a drawer of the row at `row` holds the pointer in: from the row's right edge (the arrow and
/// the 2 px between the arrow's start and the slide) to the slide image's right edge, the slide image's height.
// DC-DEFAULT: CLI-1090 (playtest report: "the drawer closes when trying to click the drawer options") the 2009 row
// hit test (cshell+0x1d9390) takes the row and its shown buttons only, so a pointer crossing the 2 px between the
// row (0x9f wide) and the first button (x + 0xa1), or the slide image while it still moves, slides the drawer back
// before a button can be pressed; ours keeps the drawer while the pointer is on the arrow, the slide or the buttons.
pub fn drawer_rect(row: [i32; 4]) -> [i32; 4] {
    let x = row[0] + SLIDE_ARROW_AT[0];
    [
        x,
        row[1] + SLIDE_AT[1],
        row[0] + SLIDE_AT[0] + SLIDE_SIZE[0] - x,
        SLIDE_SIZE[1],
    ]
}

/// The backpack row under `p` of `rows` rows at `at`: the row itself, its arrow, and a drawer that is not closed
/// (opening, open or closing: the slide area and the buttons) count as the row (CLI-1090).
pub fn pack_row_at(at: [i32; 2], rows: usize, look: &PaneLook, p: [f32; 2]) -> Option<usize> {
    let n = rows.min(4);
    (0..n)
        .find(|i| {
            let r = pack_row_rect(at, *i);
            let a = SLIDE_ARROW_AT;
            inside(r, p) || inside([r[0] + a[0], r[1] + a[1], a[2], a[3]], p)
        })
        .or_else(|| {
            (0..n).find(|i| {
                look.drawers[*i].state != Slide::Closed
                    && inside(drawer_rect(pack_row_rect(at, *i)), p)
            })
        })
        .or_else(|| drawer_button_at(at, rows, look, p).map(|(i, _)| i))
}

/// The backpack's equipment slot.
pub const PACK_SLOT_NO: usize = 9;
/// The weapon image on a backpack row: the slot image's 0xfe x 0x47 sprite scaled by 0.9 in both directions, 0x48
/// left of and 0xb above the row (CLI-973; CLI-891 drew 0xe6 x 0x31 texels unscaled, cutting the gun).
// EVIDENCE: cshell+0x1da7f0 (the backpack item image: a 0xfe x 0x47 sprite of the item's slot image at the item's
// (x - 0x48, y - 0xb), scale 0x3f666666 = 0.9 on both axes)
// CLI-1091 (playtest report: "backpack gun icons still not perfectly scaled") the 0.9 scale shrinks the sprite about
// its own centre, not its top-left corner: the 0xfe x 0x47 sprite at (x - 0x48, y - 0xb) keeps its middle and draws
// 0.9 of its size, so its top-left lands at (x - 0x48 + 0xfe * 0.05, y - 0xb + 0x47 * 0.05)
// OBSERVED: reference client shop page with the user's account (AK-103 in backpack row A) at 1024 x 768 and at
// 2560 x 1440 (shop-r12/pairs/item2a_*): a fit of the row's gun against ours gives the
// same picture 0.975 x ours (0.8775 of the texels) about a point 12-14 px right / 4-5 px down at both window
// sizes, i.e. 0.9 about the sprite's centre within a pixel at the gun's middle
pub const PACK_IMAGE_SPRITE: [i32; 2] = [0xfe, 0x47];
pub const PACK_IMAGE_AT: [i32; 2] = [-0x48, -0xb];
pub const PACK_IMAGE_SCALE: f32 = 0.9;

/// The weapon image `tex` on the backpack row at `r`.
pub fn pack_item_image(cat: &mut Catalog, tex: &str, r: [i32; 4]) -> DrawItem {
    let (tw, th) = cat
        .tex_size(tex)
        .unwrap_or((PACK_IMAGE_SPRITE[0] as u32, PACK_IMAGE_SPRITE[1] as u32));
    let src = [
        PACK_IMAGE_SPRITE[0].min(tw as i32) as f32,
        PACK_IMAGE_SPRITE[1].min(th as i32) as f32,
    ];
    DrawItem::Image {
        tex: tex.to_string(),
        src,
        dst: [
            (r[0] + PACK_IMAGE_AT[0]) as f32 + src[0] * (1.0 - PACK_IMAGE_SCALE) / 2.0,
            (r[1] + PACK_IMAGE_AT[1]) as f32 + src[1] * (1.0 - PACK_IMAGE_SCALE) / 2.0,
            src[0] * PACK_IMAGE_SCALE,
            src[1] * PACK_IMAGE_SCALE,
        ],
        tint: [255; 4],
        turn: None,
    }
}
/// The first backpack weapon slot.
pub const FIRST_PACK_SLOT: usize = 10;
/// The backpack list of the pane.
pub const BACKPACK_LIST: i32 = 3;

/// The item's slot image on a row: 0xfe x 0x47 of the image from its top-left, 0x3e px left of the row.
// EVIDENCE: cshell+0xda610 (a list item's item image: the item's slot image through cshell+0x35a990 / 0x35b890 in a
// 0xfe x 0x47 sprite at the item's x - 0x3e, y; none for an empty character slot 0xe)
pub const ROW_IMAGE_SIZE: [i32; 2] = [0xfe, 0x47];
pub const ROW_IMAGE_DX: i32 = -0x3e;
/// The row's name: font 0x15 at the row's (5, 5), white.
// EVIDENCE: cshell+0xda950 (a list item's text: font 0x15 through cshell+0x2c07b0, placed at the item's x + 5,
// y + 5, colours all 0xffffffff)
// DC-DEFAULT: CLI-740 (proposed) the row name's size is the shop cells' 14 (the text object's size argument is 0,
// its default is not traced; OBSERVED screens-sweep/pairs/shop.png: row and cell names of the
// same height)
pub const ROW_TEXT_AT: [i32; 2] = [5, 5];
pub const ROW_FONT: i32 = 0x15;
pub const ROW_FONT_SIZE: i32 = 14;
pub const ROW_COLOUR: u32 = 0xffff_ffff;

const ALIGN_TL: Align = Align {
    first: ALIGN_LEFT,
    second: 0,
};

/// The row image of row `i` of a tab's equipment list (weapons: by row, body: one image for all).
/// `hot`: the pointer is over the row (its _On image, CLI-971).
// DC-DEFAULT: CLI-740 (proposed) the body list's character row keeps the plain row image (its special-slot image for a
// type 0x1e character item of cshell+0x1425d0 is not ported)
pub fn row_image(weapons: bool, i: usize, hot: bool) -> Option<&'static str> {
    match (weapons, hot) {
        (true, false) => WEAPON_ROW_IMAGES.get(i).copied(),
        (true, true) => WEAPON_ROW_ON_IMAGES.get(i).copied(),
        (false, false) => Some(BODY_ROW_IMAGE),
        (false, true) => Some(BODY_ROW_ON_IMAGE),
    }
}

/// The image of backpack row `i` for a backpack of `slots` extra slots (`hot`: its _On image).
pub fn backpack_row_image(slots: i32, i: usize, hot: bool) -> Option<&'static str> {
    if slots < 1 {
        return (i == 0).then_some(if hot {
            BACKPACK_ROW_ON_IMAGE
        } else {
            BACKPACK_ROW_IMAGE
        });
    }
    if i >= (slots % 5) as usize {
        return None;
    }
    if hot {
        BACKPACK_SLOT_ON_IMAGES.get(i).copied()
    } else {
        BACKPACK_SLOT_IMAGES.get(i).copied()
    }
}

/// The backpack list's row images for a backpack of `slots` extra slots.
// EVIDENCE: cshell+0x1425d0 (count = +0x2dc; < 1: one Backpack_Default row; else count % 5 rows of the slot images)
// DC-DEFAULT: CLI-740 (proposed) the slot rows use the Backpack_Slot_A..D images (the Special_Slot_S1..S4 form for
// the shell's +0x2e0 == 1 is not ported; the items in the backpack slots 10.. are not drawn on the rows)
pub fn backpack_rows(slots: i32) -> Vec<&'static str> {
    if slots < 1 {
        return vec![BACKPACK_ROW_IMAGE];
    }
    BACKPACK_SLOT_IMAGES
        .iter()
        .take((slots % 5) as usize)
        .copied()
        .collect()
}

/// Draws the pane's equipment rows (`rows`: slot, category, empty-slot string, rectangle) and, on the weapons tab,
/// the backpack row at `backpack`; `look`: the rows under the pointer and the backpack drawers.
#[allow(clippy::too_many_arguments)]
pub fn draw_rows(
    cat: &mut Catalog,
    weapons: bool,
    rows: &[(usize, usize, i32, [i32; 4])],
    backpack: Option<[i32; 2]>,
    wares: &Wares,
    store: &Store,
    look: &PaneLook,
    out: &mut Vec<DrawItem>,
) {
    for (i, (slot, _, empty, r)) in rows.iter().enumerate() {
        if let Some(t) = row_image(weapons, i, look.row == Some(i)) {
            out.push(image_fit(cat, t, [r[0], r[1]], ROW_SIZE));
        }
        let item = store.in_slot(*slot);
        if let Some(icon) = item.and_then(|it| {
            wares
                .icons_of(it.item_no as i32)
                .into_iter()
                .find(|i| cat.tex_size(i).is_some())
        }) {
            out.push(image_fit(
                cat,
                &icon,
                [r[0] + ROW_IMAGE_DX, r[1]],
                ROW_IMAGE_SIZE,
            ));
        }
        let name = match item {
            Some(it) => wares.label_of(it.item_no as i32),
            None if *empty != 0 => cat.text(*empty),
            None => String::new(),
        };
        out.push(place_text(
            &name,
            [r[0] + ROW_TEXT_AT[0], r[1] + ROW_TEXT_AT[1]],
            [0, 0],
            ALIGN_TL,
            ROW_FONT,
            ROW_FONT_SIZE,
            ROW_COLOUR,
        ));
    }
    if let Some(at) = backpack.filter(|_| weapons) {
        let slots = wares.backpack_slots(store.equipped[PACK_SLOT_NO]);
        for i in 0..backpack_rows(slots).len() {
            let r = pack_row_rect(at, i);
            let y = r[1];
            if let Some(t) = backpack_row_image(slots, i, look.pack == Some(i)) {
                out.push(image_fit(cat, t, [at[0], y], BACKPACK_ROW_SIZE));
            }
            // CLI-891: the weapon in backpack slot 10 + i shows its slot image on the row (no name)
            // EVIDENCE: cshell+0x147100 (list 3's items get the item image of each record in slots 10..13,
            // cshell+0x371da7f0, item index slot - 10), cshell+0xda610 (a list item's image 0xfe x 0x47 at x - 0x3e)
            // OBSERVED: storage-functions/cpp/item2/shop_bb07_pack_equipped.png
            if slots >= 1 {
                if let Some(icon) = store
                    .in_slot(FIRST_PACK_SLOT + i)
                    .and_then(|it| wares.art_of(it.item_no as i32))
                    .map(|a| a.icon.clone())
                    .filter(|t| cat.tex_size(t).is_some())
                {
                    out.push(pack_item_image(cat, &icon, r));
                }
                if store.in_slot(FIRST_PACK_SLOT + i).is_some() {
                    let x = unequip_x_rect(r);
                    let letter = if look.x == Some(i) { "B" } else { "A" };
                    out.push(image_fit(
                        cat,
                        &format!("{UNEQUIP_X}{letter}.dtx"),
                        [x[0], x[1]],
                        [x[2], x[3]],
                    ));
                }
                draw_drawer(cat, r, &look.drawers[i.min(3)], look.button, i, out);
            }
        }
    }
}

/// CLI-972 the drawer of the backpack slot row at `r`: the arrow while closed, the sliding image while it moves, the
/// three buttons while open (`hot`: the button under the pointer, row and button).
fn draw_drawer(
    cat: &mut Catalog,
    r: [i32; 4],
    d: &Drawer,
    hot: Option<(usize, usize)>,
    row: usize,
    out: &mut Vec<DrawItem>,
) {
    match d.state {
        Slide::Closed => {
            let a = SLIDE_ARROW_AT;
            out.push(image_fit(
                cat,
                SLIDE_ARROW,
                [r[0] + a[0], r[1] + a[1]],
                [a[2], a[3]],
            ));
        }
        Slide::Opening | Slide::Closing => {
            let w = SLIDE_SIZE[0] - d.hidden;
            if w > 0 {
                let part = crate::hud::compose::part_of(
                    SLIDE_IMAGE,
                    [d.hidden as u32, 0, w as u32, SLIDE_SIZE[1] as u32],
                );
                out.push(image_fit(
                    cat,
                    &part,
                    [r[0] + SLIDE_AT[0], r[1] + SLIDE_AT[1]],
                    [w, SLIDE_SIZE[1]],
                ));
            }
        }
        Slide::Open => {
            for (k, (prefix, _, _)) in SLIDE_BUTTONS.iter().enumerate() {
                let b = drawer_button_rect(r, k);
                let letter = if hot == Some((row, k)) { "b" } else { "a" };
                let tex = format!("{prefix}{letter}.dtx");
                out.push(image_fit(cat, &tex, [b[0], b[1]], [b[2], b[3]]));
            }
        }
    }
}

/// The shop page window's image: the black market tab swaps in its own (its title reads Black Market).
// EVIDENCE: cshell+0x14b720 (the tab switch sets window 3's image to Shop_Item_Window, then for tab 2 to
// Blackmarket_Window)
pub const SHOP_WINDOW_IMAGE: &str = "UI_SCR_Page_Shop\\Inven\\Shop_Item_Window.dtx";
pub const MARKET_WINDOW_IMAGE: &str = "UI_SCR_Page_Shop\\Inven\\Blackmarket_Window.dtx";

pub fn shop_window_image(market: bool) -> &'static str {
    if market {
        MARKET_WINDOW_IMAGE
    } else {
        SHOP_WINDOW_IMAGE
    }
}

/// The black market tab's list of the character's active function items (list 5 of the pane): the function list
/// sorted by `SubType`, at most 16 rows, the first row named by string 0x17cc while the list is empty. The list
/// scrolls: its rows run under the list's scroll bar (the record's), which is shown from 7 rows on.
// EVIDENCE: cshell+0x1496e0 (list 5 shown; one row per record of the function list (inventory manager +0x38), at
// most 0x10, each with the item's image and name of type 0x15; the first row stays with string 0x17cc and no item
// when the list is empty; fewer than 7 records: the list's bar off (cshell+0x3f0330(0)) and width 0x104, else bar on
// and width 0xf8), cshell+0x2292e0 (the function list: the records of the seven function slots, then every record
// of type 0x15 in use whose row +0x241 is 0), cshell+0x145a10 (the copy sorted by the row's +0x14 = SubType,
// cshell+0x227070, then the row's +0), cshell+0x14b8a0 (tab 2 of the shop and storage pages runs it),
// cshell+0x1425d0 (its 16 items MyItemSlot_Off 0xc2 x 0x47)
// DC-DEFAULT: CLI-741 (amended, OWNER REPORT 2026-11-04 "activating all of them causes them to clip off screen")
// the rows are the function list's records (the records in the function slots, then the records in use without
// IsCanUnEquip), sorted by SubType then item number (row +0 taken for the item number); rows at the list's x + 12
// (OBSERVED screens-sweep/cpp/shop/shop_bb03_shop_market.png: the row's left edge at x 260, the list record at 248);
// the list record's height (450: six rows) clips them, the record's bar scrolls them (wheel, arrows, thumb) a row
// at a time; the row x stays where it is when the bar shows (the 2009 width 0xf8 is not applied to the rows)
pub const MARKET_LIST: i32 = 5;
pub const MARKET_ROWS: usize = 0x10;
pub const MARKET_EMPTY_TEXT: i32 = 0x17cc;
pub const MARKET_ROW_DX: i32 = 12;
/// The list shows its scroll bar from this many rows on.
// EVIDENCE: cshell+0x1496e0 (count < 7 -> bar off)
pub const MARKET_BAR_ROWS: usize = 7;

/// The records the black market list shows, in its order (see [`MARKET_LIST`]).
pub fn market_records<'a>(wares: &Wares, store: &'a Store) -> Vec<&'a InvRecord> {
    let mut list = wares.function_list(store);
    list.sort_by_key(|r| {
        (
            wares.art_of(r.item_no as i32).map_or(-1, |a| a.sub_type),
            r.item_no,
        )
    });
    list.truncate(MARKET_ROWS);
    list
}

/// Draws the black market list into list record `list` (its bar scrolls the rows; rows outside the list's height
/// are not drawn).
pub fn draw_market_rows(
    cat: &mut Catalog,
    list: &Part,
    wares: &Wares,
    store: &Store,
    out: &mut Vec<DrawItem>,
) {
    let held: Vec<i32> = market_records(wares, store)
        .iter()
        .map(|r| r.item_no as i32)
        .collect();
    let row = |i: usize| {
        list_row(list, ROW_SIZE[1], i, 0)
            .map(|at| [at[0] + MARKET_ROW_DX, at[1], ROW_SIZE[0], ROW_SIZE[1]])
    };
    if held.is_empty() {
        let r = [
            list.pos[0] + MARKET_ROW_DX,
            list.pos[1],
            ROW_SIZE[0],
            ROW_SIZE[1],
        ];
        out.push(image_fit(cat, BODY_ROW_IMAGE, [r[0], r[1]], ROW_SIZE));
        let t = cat.text(MARKET_EMPTY_TEXT);
        out.push(row_text(&t, r));
        return;
    }
    for (i, n) in held.iter().enumerate() {
        let Some(r) = row(i) else {
            continue;
        };
        out.push(image_fit(cat, BODY_ROW_IMAGE, [r[0], r[1]], ROW_SIZE));
        if let Some(icon) = wares
            .art_of(*n)
            .map(|a| a.icon.clone())
            .filter(|i| cat.tex_size(i).is_some())
        {
            out.push(image_fit(
                cat,
                &icon,
                [r[0] + ROW_IMAGE_DX, r[1]],
                ROW_IMAGE_SIZE,
            ));
        }
        out.push(row_text(&wares.label_of(*n), r));
    }
    if let Some(b) = market_bar(list, held.len()) {
        b.draw(cat, out);
    }
}

/// The black market list's bar (None while it is hidden: fewer than [`MARKET_BAR_ROWS`] rows).
pub fn market_bar(list: &Part, rows: usize) -> Option<&ScrollBar> {
    match &list.look {
        super::sheet::Look::List { bar: Some(b), .. } if rows >= MARKET_BAR_ROWS => Some(b),
        _ => None,
    }
}

fn row_text(t: &str, r: [i32; 4]) -> DrawItem {
    place_text(
        t,
        [r[0] + ROW_TEXT_AT[0], r[1] + ROW_TEXT_AT[1]],
        [0, 0],
        ALIGN_TL,
        ROW_FONT,
        ROW_FONT_SIZE,
        ROW_COLOUR,
    )
}
