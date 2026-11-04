//! storage-functions: the shop screen's slot choice windows. A weapon equipped from the shop (the "equip now" box of
//! a purchase) while the character wears a backpack with weapon slots asks in window 7 ("Equip Slot": the weapon's
//! own slot or a backpack slot); the storage page's Backpack button (1009) asks in window 6 ("Backpack": which
//! backpack slot). OK sends the equip request 0x324 into the chosen slot (backpack slots are equipment slots 10..13).
//!
//! The windows are layout windows of the shop page (texts, check buttons, OK / Cancel); the slot list is our
//! drop-down list under the window's field.
// SPEC: PROTOCOL_2009.md §4.2 (0x324: +6 unique id, +0xa item number, +0xe slot)
// EVIDENCE: cshell+0x14f830 (1004 toggles the list +0x55ec, 1005 / 1006 the window-7 mode +0x5590 = 0 / 1 through
// cshell+0x1402f0, 1007 OK -> cshell+0x14f290, 1008 Cancel: shop page -> cshell+0x14f180(0), storage page ->
// cshell+0x142b60(0); 1009 -> cshell+0x148020(1) then cshell+0x142b60(1); 0x3f8 (the equip box's Equip) on the weapons
// tab -> cshell+0x14f180(1) when cshell+0x148020(0) passes)
// EVIDENCE: cshell+0x14f180 (window 7 shown, the list moved to (0x21f, 0x1a8); a type-7 item is equipped at once
// (cshell+0x14ed50); else the field (7, 0) and arrow (7, 1) hidden, the list filled (cshell+0x13fed0), row 0 picked
// (cshell+0x140130), command 0x3ed), cshell+0x142b60 (window 6 shown, the list at (0x1cf, 0x1a8), filled, row 0
// picked), cshell+0x14f290 (OK: shop page mode 0 -> cshell+0x147e40 (the weapon's own slot), mode 1 or the storage
// page -> cshell+0x14baa0 (backpack slot = picked row + 10))

use dc_ui_data::WidgetKind;

use super::catalog::Catalog;
use super::dropdown::Dropdown;
use super::link::LinkCmd;
use super::sheet::Sheet;
use super::storage::{own_slot, Store, Wares};

/// The windows: 6 "Backpack" (storage page), 7 "Equip Slot" (shop page).
pub const PACK_WINDOW: i32 = 6;
pub const SLOT_WINDOW: i32 = 7;
/// Commands of the windows and the storage page's Backpack button.
pub const CMD_LIST: i32 = 1004;
pub const CMD_OWN_SLOT: i32 = 1005;
pub const CMD_PACK_SLOT: i32 = 1006;
pub const CMD_OK: i32 = 1007;
pub const CMD_CANCEL: i32 = 1008;
pub const CMD_BACKPACK: i32 = 1009;
/// Window 7's check buttons (own slot, backpack) and the controls of the field: image 0, arrow button 1, the picked
/// row's text (window 6 text 7, window 7 text 8).
pub const OWN_CHECK: i32 = 9;
pub const PACK_CHECK: i32 = 10;
pub const FIELD_IMAGE: i32 = 0;
pub const ARROW_BUTTON: i32 = 1;
pub const PICKED_TEXT: [i32; 2] = [7, 8];
/// The windows' description (text 5) wraps inside the window with the margin it has on the left.
// OBSERVED: storage-functions/pairs/item2/pack_window.png (the reference stack's window-6 text breaks
// after "Please" / "Any item" / "will", about 346 px from its left)
// DC-DEFAULT: CLI-772 (proposed) window 6 wraps at 346, window 7 at 318 (the window's width less twice the text's
// left margin; the 2009 width setter is not traced)
pub const DESC_TEXT: i32 = 5;
pub const DESC_WRAP: [i32; 2] = [346, 318];
/// The first backpack equipment slot.
// EVIDENCE: cshell+0x14baa0 (slot = +0x55a4 + 10)
pub const FIRST_PACK_SLOT: u32 = 10;
/// The rows' slot word (string 0x399) after the letters A..D.
// EVIDENCE: cshell+0x13fed0 (rows "%s %s": the letters (wide literals VA 0x374e3e84 / 80 / 7c / 78, "A".."D"; for a
// character of type 1 "S1".."S4" at VA 0x374e3e70 / 68 / 60 / 58) and string 0x399; slot count % 5 rows)
// DC-DEFAULT: CLI-772 (proposed) the letters A..D for every character (the specialist form S1..S4 by the shell's
// character type, cshell+0x3254d0, is not ported)
pub const SLOT_WORD: i32 = 0x399;
pub const LETTERS: [&str; 4] = ["A", "B", "C", "D"];
/// The refusal of the backpack check: the item's own slot is empty (0x39b titled 0x39a).
pub const NOT_EQUIPPED: (i32, i32) = (0x39b, 0x39a);
/// Nothing selected (0x3d2, Notice).
pub const NONE_SELECTED: (i32, i32) = (0x3d2, 0x371);

/// The weapon slots of the equipped backpack (slot 9; 0 without one).
// EVIDENCE: cshell+0x148020 (shell +0x1b4 +0x2dc below 1 -> no backpack choice), cshell+0x227240 (the count from the
// equipped backpack's item row)
pub fn pack_slots(store: &Store, wares: &Wares) -> i32 {
    wares.backpack_slots(store.equipped[super::pane::PACK_SLOT_NO])
}

/// The melee weapon slot: the backpack takes no melee weapon (CLI-974).
// DC-DEFAULT: CLI-974 (playtest report: "a knife cannot go into the backpack") the backpack takes primary, secondary and
// support weapons only, as its drawer offers (cshell+0x1d9ad0: Primary / Secondary / Throw; cshell+0x1509ba opens the
// storage categories 0 / 1 / 3); the 2009 check cshell+0x148020 itself passes slot 2 when it is filled
pub const MELEE_SLOT: u32 = 2;

/// The backpack check of an item (cshell+0x148020): `Ok` when the backpack can take it, `Err(None)` when there is no
/// backpack slot (no box), `Err(Some(box))` otherwise.
// EVIDENCE: cshell+0x148020 (no slot -> 0 without a box; nothing selected -> 0x3d2; the item's slot by type
// (cshell+0x359f00) 0..3 must already hold an item, else box 0x39b titled 0x39a; slot 3 also asks cshell+0x146640;
// other types -> 0x39b)
// DC-DEFAULT: CLI-772 (proposed) the special-item check (0x189e / 0x189f against the shell's character type) and the
// slot-3 extra check cshell+0x146640 are not ported
pub fn pack_check(
    store: &Store,
    wares: &Wares,
    item_no: Option<u32>,
) -> Result<(), Option<(i32, i32)>> {
    if pack_slots(store, wares) < 1 {
        return Err(None);
    }
    let Some(n) = item_no else {
        return Err(Some(NONE_SELECTED));
    };
    match own_slot(wares, n) {
        Some(MELEE_SLOT) => Err(None),
        Some(s @ 0..=3) if store.equipped[s as usize] != -1 => Ok(()),
        _ => Err(Some(NOT_EQUIPPED)),
    }
}

/// The rows of the slot list.
pub fn pack_rows(cat: &Catalog, slots: i32) -> Vec<String> {
    let word = cat.text(SLOT_WORD);
    (0..(slots.rem_euclid(5) as usize).min(LETTERS.len()))
        .map(|k| format!("{} {}", LETTERS[k], word))
        .collect()
}

/// An open slot choice window.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotPick {
    pub window: i32,
    pub uid: u32,
    pub item: u32,
    /// Window 7: the backpack is chosen (mode 1) instead of the item's own slot.
    pub to_pack: bool,
    pub rows: Vec<String>,
    pub row: usize,
    pub drop: Option<Dropdown>,
}

impl SlotPick {
    /// Shows window `window` for the item and picks row 0 (window 7 on the own slot).
    pub fn open(
        sheet: &mut Sheet,
        cat: &Catalog,
        store: &Store,
        wares: &Wares,
        window: i32,
        uid: u32,
        item: u32,
    ) -> SlotPick {
        let mut p = SlotPick {
            window,
            uid,
            item,
            to_pack: window == PACK_WINDOW,
            rows: pack_rows(cat, pack_slots(store, wares)),
            row: 0,
            drop: None,
        };
        sheet.show_window(window, true);
        sheet.set_wrap(
            window,
            DESC_TEXT,
            DESC_WRAP[usize::from(window == SLOT_WINDOW)],
        );
        p.pick(sheet, 0);
        if window == SLOT_WINDOW {
            p.set_mode(sheet, false);
        }
        p
    }

    /// Row `k` picked: the field's text.
    // EVIDENCE: cshell+0x140130 (the picked row's text into window 7 text 8 / window 6 text 7, OK / Cancel enabled,
    // +0x55a4 = row)
    pub fn pick(&mut self, sheet: &mut Sheet, k: usize) {
        if k < self.rows.len() {
            self.row = k;
        }
        let text = self.rows.get(self.row).cloned().unwrap_or_default();
        let id = PICKED_TEXT[usize::from(self.window == SLOT_WINDOW)];
        sheet.set_text(self.window, id, &text);
    }

    /// Window 7's mode: the own slot (field hidden) or the backpack (field shown); the check buttons follow.
    // EVIDENCE: cshell+0x1402f0 (command 9 / 10: check 9 on / off, the field image 0, the arrow 1 and text 8 hidden /
    // shown)
    pub fn set_mode(&mut self, sheet: &mut Sheet, to_pack: bool) {
        self.to_pack = to_pack;
        self.drop = None;
        let w = self.window;
        sheet.show(w, FIELD_IMAGE, WidgetKind::Image, to_pack);
        sheet.show(w, ARROW_BUTTON, WidgetKind::Button, to_pack);
        sheet.show(w, PICKED_TEXT[1], WidgetKind::Text, to_pack);
        for (id, on) in [(OWN_CHECK, !to_pack), (PACK_CHECK, to_pack)] {
            if let Some(c) = sheet.part_mut(w, id, WidgetKind::CheckButton) {
                c.checked = on;
            }
        }
    }

    /// The field's rectangle (window `window`'s image 0).
    pub fn field(&self, sheet: &Sheet) -> Option<[i32; 4]> {
        sheet
            .part(self.window, FIELD_IMAGE, WidgetKind::Image)
            .map(|p| [p.pos[0], p.pos[1], p.size[0], p.size[1]])
    }

    /// Command 1004: the list opens under the field, or closes.
    pub fn toggle_list(&mut self, sheet: &Sheet) {
        if self.drop.take().is_some() {
            return;
        }
        let Some(f) = self.field(sheet) else {
            return;
        };
        let rows = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| (r.clone(), i as i32))
            .collect();
        self.drop = Some(Dropdown::new("pack_slot", f, rows, 0xe, false));
    }

    /// OK: the equip request into the chosen slot (`None` without a backpack row in backpack mode).
    pub fn request(&self, wares: &Wares) -> Option<LinkCmd> {
        let slot = if self.to_pack {
            if self.rows.is_empty() {
                return None;
            }
            FIRST_PACK_SLOT + self.row as u32
        } else {
            own_slot(wares, self.item)?
        };
        Some(LinkCmd::Equip {
            uid: self.uid,
            item: self.item,
            slot,
        })
    }

    /// Hides the window.
    pub fn close(&self, sheet: &mut Sheet) {
        sheet.show_window(self.window, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backpack_takes_a_weapon_whose_own_slot_is_filled() {
        let w = Wares::default();
        let mut s = Store::default();
        s.equipped = [-1; 15];
        // no backpack: no choice, no box
        assert_eq!(pack_check(&s, &w, Some(4001)), Err(None));
    }

    #[test]
    fn requests_go_to_the_own_slot_or_backpack_slot_10_plus_row() {
        let w = Wares::default();
        let mut p = SlotPick {
            window: SLOT_WINDOW,
            uid: 5,
            item: 4001,
            to_pack: true,
            rows: vec!["A".into(), "B".into()],
            row: 1,
            drop: None,
        };
        match p.request(&w) {
            Some(LinkCmd::Equip { uid, slot, .. }) => assert_eq!((uid, slot), (5, 11)),
            other => panic!("{other:?}"),
        }
        p.to_pack = false;
        match p.request(&w) {
            Some(LinkCmd::Equip { slot, .. }) => assert_eq!(slot, 4),
            other => panic!("{other:?}"),
        }
    }
}
