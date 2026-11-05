//! w3-shop: what the shop screens know about the character's items and the shop's wares.
//!
//! [`Store`] keeps the lobby's answers (inventory 0x321, equipment 0x323, the shown products 0x351, the highlight
//! lists 0x353 / 0x355 / 0x357, the prices 0x359 and the inbox pages 0x32d); [`Wares`] reads the shop catalogue and
//! the item table of the user's ROM at run time (names, prices, item numbers: nothing of it is in the tree) and sorts
//! products and items onto the shop's shelves (left tab, category) by item type.
// SPEC: PROTOCOL_2009.md §3.3 (0x320 .. 0x323, table 3.3b), §4.1 - §4.3, §4.5 (item number ranges)

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use super::link::{InboxRow, InvRecord, LinkEvent};

/// The lobby's answers about the character's items, the shop lists and the inbox.
#[derive(Debug, Clone, Default)]
pub struct Store {
    pub items: Vec<InvRecord>,
    /// Item number per equipment slot (-1 empty), 0x323.
    pub equipped: [i32; 15],
    /// function-unequip: the function-slot item numbers of 0x323 +0x8a (-1 empty; empty until the first reply).
    pub function_slots: Vec<i32>,
    /// The products the shop shows (0x351); `None` until the first reply.
    pub view: Option<Vec<u32>>,
    /// Price per product (0x359).
    pub prices: BTreeMap<u32, u32>,
    /// Highlight lists 0 / 1 / 2 (0x353 / 0x355 / 0x357: the Hot, New and Sale marks).
    pub highlight: [BTreeSet<u32>; 3],
    /// The shop lists were asked for (once per connection).
    pub lists_asked: bool,
    /// Inbox rows (0x32d) and the total the server holds.
    pub inbox: Vec<InboxRow>,
    pub inbox_total: u32,
    /// An inbox page arrived since the inbox was last marked stale.
    pub inbox_fetched: bool,
    /// The inbox holds something new (0x303 +0xdc, 0x3014): the inbox tab asks again.
    pub inbox_stale: bool,
    /// CLI-1430: an option item an inbox opening added (unique id): the Modification screen opens its option storage
    /// with it picked once the inventory holds it.
    pub tuning_goto: Option<u32>,
}

impl Store {
    /// Takes in a lobby event (the shared lists of [`super::MenuData`]).
    pub fn apply(&mut self, e: &LinkEvent) {
        match e {
            LinkEvent::Inventory(v) => self.items = v.clone(),
            LinkEvent::Equipment(s) => self.equipped = *s,
            LinkEvent::FunctionSlots(s) => self.function_slots = s.to_vec(),
            // function-unequip: an ok slot answer edits the held slots at once (the reread that follows replaces them)
            // EVIDENCE: cshell+0x14cea0 (0x369 ok: cshell+0x22cb50 enters the record in the function list and its item
            // number in the slot of its kind), cshell+0x14d010 (0x36b ok: cshell+0x22cc10 removes it and clears the
            // slot that holds its number)
            LinkEvent::FunctionSlotDone { put, ok: true, uid } => {
                let Some(r) = self.items.iter().find(|r| r.uid == *uid).cloned() else {
                    return;
                };
                if *put {
                    if !self.in_function_slot(&r) {
                        match self.function_slots.iter_mut().find(|v| **v == -1) {
                            Some(v) => *v = r.item_no as i32,
                            None => self.function_slots.push(r.item_no as i32),
                        }
                    }
                } else if let Some(i) = self.function_slot_of(&r) {
                    self.function_slots[i] = -1;
                }
            }
            LinkEvent::ShopView(v) => self.view = Some(v.clone()),
            LinkEvent::Highlight { list, ids } => {
                if let Some(h) = self.highlight.get_mut(*list) {
                    *h = ids.iter().copied().collect();
                }
            }
            LinkEvent::Prices(v) => self.prices.extend(v.iter().copied()),
            // SPEC: PROTOCOL_2009.md §4.3 (inbox paging: the client keeps asking while it holds fewer rows than the
            // total; rows of later pages are added)
            LinkEvent::Inbox { total, rows } => {
                for r in rows {
                    if !self.inbox.iter().any(|x| x.entry == r.entry) {
                        self.inbox.push(r.clone());
                    }
                }
                self.inbox_total = *total;
                self.inbox_fetched = true;
                self.inbox_stale = false;
            }
            LinkEvent::InboxStale => self.inbox_stale = true,
            // audit-function (CLI-1389): expired records leave the inventory (cshell+0x22cde0 -> cshell+0x22ccb0)
            LinkEvent::Expired { uids, .. } => {
                for &u in uids {
                    self.remove(u);
                }
            }
            LinkEvent::CharInfo(x) if x.inbox_new => self.inbox_stale = true,
            _ => {}
        }
    }

    /// The equipment slot `item` sits in (by its item number in 0x323).
    pub fn slot_of(&self, item: &InvRecord) -> Option<usize> {
        let mut taken = [false; 15];
        // a number equipped twice is matched to the records in their order
        for r in &self.items {
            let slot = (0..15).find(|&s| !taken[s] && self.equipped[s] == r.item_no as i32);
            if let Some(s) = slot {
                taken[s] = true;
                if r.uid == item.uid {
                    return Some(s);
                }
            }
        }
        None
    }

    /// function-unequip: the function slot that holds `item`. A slot names an item number (2009 keeps n % 9000); the
    /// records in use take the slots of their number in their order.
    // EVIDENCE: cshell+0x2292e0 (the function list: the record of each slot's item number), cshell+0x226800 (a
    // unique id is in a slot when the list holds its record)
    // DC-DEFAULT: CLI-1395 (proposed) the records in use of a slot's item number take its slots in their order (the
    // 2009 lookup of a slot's record, cshell+0x2276e0, is not traced)
    pub fn function_slot_of(&self, item: &InvRecord) -> Option<usize> {
        if !(9000..10000).contains(&item.item_no) {
            return None;
        }
        let mut taken = vec![false; self.function_slots.len()];
        for r in self.items.iter().filter(|r| r.in_use) {
            let slot = (0..taken.len()).find(|&s| {
                !taken[s]
                    && self.function_slots[s] != -1
                    && self.function_slots[s].rem_euclid(9000) == (r.item_no % 9000) as i32
            });
            if let Some(s) = slot {
                taken[s] = true;
                if r.uid == item.uid {
                    return Some(s);
                }
            }
        }
        None
    }

    /// function-unequip: `item` sits in a function slot.
    pub fn in_function_slot(&self, item: &InvRecord) -> bool {
        self.function_slot_of(item).is_some()
    }

    /// The record in equipment slot `slot`.
    pub fn in_slot(&self, slot: usize) -> Option<&InvRecord> {
        self.items.iter().find(|r| self.slot_of(r) == Some(slot))
    }

    /// The next inbox page to ask for (`None` while all rows are held).
    // SPEC: PROTOCOL_2009.md §4.3 (inbox paging: the page is `(held - 1) / 4 + 1`, 1 when empty; asked while fewer
    // rows than the last total are held)
    pub fn next_inbox_page(&self) -> Option<u32> {
        let held = self.inbox.len() as u32;
        if self.inbox_fetched && held >= self.inbox_total {
            return None;
        }
        Some(if held == 0 { 1 } else { (held - 1) / 4 + 1 })
    }
}

/// storage-functions: the box strings of the storage page's Discard (1011).
// EVIDENCE: cshell+0x14f830 (1011 = 0x3f3: the check cshell+0x146840, then box 0x372 or, for an item whose cheapest
// shown product (cshell+0x2510e0) is a credit product, 0x1821; title 0x1820, buttons 0x35 / 0x36, its first button runs
// shop command 0x95 (cshell+0x13f7a0), which sends 0x334 (cshell+0x142ca0))
pub const DISCARD_ASK: i32 = 0x372;
pub const DISCARD_ASK_CREDIT: i32 = 0x1821;
pub const DISCARD_TITLE: i32 = 0x1820;
pub const DISCARD_BUTTONS: [i32; 2] = [0x35, 0x36];
/// The refusals: nothing selected (0x3d2, titled 0x371), a weapon with an option attached (0x40c, 0x346), the
/// category's default item (0x437, 0x346), an equipped item (0x1642, titled 0x1826).
pub const DISCARD_NONE: (i32, i32) = (0x3d2, 0x371);
pub const DISCARD_ATTACHED: (i32, i32) = (0x40c, 0x346);
pub const DISCARD_DEFAULT: (i32, i32) = (0x437, 0x346);
pub const DISCARD_EQUIPPED: (i32, i32) = (0x1642, 0x1826);

/// Why the storage page's Discard refuses `item` (box string and title), `None` when it may be discarded.
// EVIDENCE: cshell+0x146840 (in this order: no cell -> 0x3d2; the item's record missing or one of its three option
// slots +0x30 / +0x34 / +0x38 taken -> 0x40c; the category's first item (cshell+0x228790) flagged default (+0x1c) and
// the item -> 0x437; the item in an equipment slot (cshell+0x225220) -> 0x1642 titled 0x1826; part of an equipped
// package (cshell+0x1462b0) -> 0x188b with the package's name)
// SPEC: PROTOCOL_2009.md §3.3 table 3.3b (+8 parent unique id: an option sits on the weapon of that id; +0x28 default
// slot kind)
// DC-DEFAULT: CLI-770 (proposed) an item is the default one when its record's default slot kind (+0x28) is set, an
// option is attached when an owned record names the item as its parent; the package check (0x188b, backpack sets) is
// not ported
pub fn discard_refusal(store: &Store, item: Option<&InvRecord>) -> Option<(i32, i32)> {
    let Some(r) = item else {
        return Some(DISCARD_NONE);
    };
    if store
        .items
        .iter()
        .any(|o| o.parent_uid != 0 && o.parent_uid == r.uid)
    {
        return Some(DISCARD_ATTACHED);
    }
    if r.slot_kind != 0 {
        return Some(DISCARD_DEFAULT);
    }
    if store.slot_of(r).is_some() {
        return Some(DISCARD_EQUIPPED);
    }
    None
}

/// The question of the discard box: 0x1821 when the item's cheapest shown product is a credit product, else 0x372.
// EVIDENCE: cshell+0x2510e0 (the item's shown products (+0xc set), the cheapest by price; none shown -> its first
// product), cshell+0x14f830 (product +0x18 = 1 -> 0x1821)
pub fn discard_question(wares: &Wares, store: &Store, item_no: i32) -> i32 {
    let Some(cat) = wares.catalog() else {
        return DISCARD_ASK;
    };
    let shown = |pid: u32| store.view.as_ref().is_some_and(|v| v.contains(&pid));
    let mut first: Option<(u32, i32)> = None;
    let mut cheapest: Option<(u32, i32)> = None;
    let mut ids: Vec<u32> = cat.view_list().into_iter().map(|p| p as u32).collect();
    if let Some(v) = &store.view {
        ids.extend(v.iter().copied());
    }
    ids.sort_unstable();
    ids.dedup();
    for pid in ids {
        let Some(p) = cat.product(pid as i32) else {
            continue;
        };
        let Some(&(ix, _, _)) = p.items.first() else {
            continue;
        };
        if cat.info(ix).map(|r| r.itemid) != Some(item_no) {
            continue;
        }
        first.get_or_insert((0, p.sale_type));
        if shown(pid) {
            let price = store
                .prices
                .get(&pid)
                .copied()
                .unwrap_or(p.sale_price.max(0) as u32);
            if cheapest.is_none_or(|(c, _)| price < c) {
                cheapest = Some((price, p.sale_type));
            }
        }
    }
    match cheapest.or(first) {
        Some((_, 1)) => DISCARD_ASK_CREDIT,
        _ => DISCARD_ASK,
    }
}

impl Store {
    /// Drops the record `uid` (an ok discard reply: the client edits its own inventory).
    // EVIDENCE: cshell+0x14bbf0 (ok 0x335 on the storage page: the selected record removed, the page rebuilt, the
    // selection cleared), cshell+0x158cb0 (the same on the Modification screen)
    pub fn remove(&mut self, uid: u32) {
        self.items.retain(|r| r.uid != uid);
    }
}

/// One product of the shop catalogue as the shop shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ware {
    pub product: u32,
    pub name: String,
    /// Item number of the product's first item.
    pub item_no: i32,
    pub kind_of: Option<i32>,
    /// The type the cell's tag shows: the item row's own `ItemType` when it has one (characters 0x16 /
    /// specialists 0x1e share the 10000s), else [`Ware::kind_of`].
    // EVIDENCE: cshell+0x149f80 (the type tag row is picked by the cell's item type), cshell+0x4467c8
    pub tag_kind: Option<i32>,
    /// 0 GP, 1 credit (SaleType).
    pub sale_type: i32,
    pub price: u32,
    /// The item's slot image (`InventoryIcon` of its item-table row), if any.
    pub icon: Option<String>,
    /// The item's place in the shop list of its sale type (`GP_DisplayNum` / `NX_DisplayNum` of the catalogue's
    /// item row; 0 = not listed): its shelf ([`display_shelf`]) and its order on the shelf.
    pub display: i32,
    /// The experience the cell asks of the character (the item row's GP / NX unlock of the sale type; 0 = none).
    // EVIDENCE: cshell+0x255860 (cell +0x10 = the item row's requirement of the pass: GP cells the first, credit
    // cells the second), cshell+0x34fdb0 (locked: character exp below +0x10)
    pub unlock_exp: u32,
    /// The cell's Hot, New and Sale marks: one of the item's products of the cell's sale type is in highlight list
    /// 0 / 1 / 2 (0x353 / 0x355 / 0x357).
    // EVIDENCE: cshell+0x255860 (each listed product sets +0x19 / +0x18 / +0x1a on the cell of its item and sale type)
    pub marks: [bool; 3],
    /// The product's period in days (0 / 999: permanent, the PERMANENT mark).
    // EVIDENCE: cshell+0x149f80 / cshell+0x157c00 (the mark shown when the cell's product +0x20 is 0 or 999)
    pub period: i32,
    /// The product's consume type (2 / 4 / 6 single use, 3 a count, 5 hours, else days).
    // EVIDENCE: cshell+0x69ff0 (the inbox row's period mark by product +0x24)
    pub consume: i32,
}

impl Ware {
    /// A product without a period (the cells' and the panels' PERMANENT mark).
    pub fn permanent(&self) -> bool {
        self.period == 0 || self.period == 999
    }
}

/// The shop catalogue and the item table of the user's ROM.
#[derive(Debug, Clone, Default)]
pub struct Wares {
    shop: Option<dc_bute::ShopCatalog>,
    items: Option<dc_bute::ItemsTable>,
    /// Item number -> the catalogue's item name (from the catalogue's item rows).
    names: BTreeMap<i32, String>,
    /// Item number -> the item table's name and slot image.
    art: BTreeMap<i32, ItemArt>,
}

/// An item's own name and slot image (the item table's `ItemName` / `InventoryIcon`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemArt {
    pub name: String,
    pub icon: String,
    /// The shop's detail panel: the item's detail image, its description and its five gauge values (damage,
    /// portability, fire rate, accuracy, recoil; weapons only, else 0).
    pub panel_picture: String,
    pub description: String,
    pub gauges: [i32; 5],
    /// The row's own `ItemType` (a character row: 22 character, 30 specialist).
    pub row_type: Option<i32>,
    /// A weapon's muzzle, sight and clip socket states (the weapon table's [`SOCKET_KEYS`] of `Weapon<item
    /// number>`: 0 none, 1 open, 2 fitted as standard; 1 without the key, CLI-1606 amendment; 0 for a weapon without a
    /// weapon-table row).
    pub socket_states: [i32; 3],
    /// The row's `FeatureString` (the detail panels' Function line; empty without one).
    pub feature: String,
    /// The row's eighteen modifier values ([`STAT_KEYS`]) in thousandths (0 without the key).
    pub stats: [i32; 18],
    /// An option's weapon class (`AttachItemtype`).
    pub attach_class: Option<i32>,
    /// A weapon's Modification pane picture (`TuningScreenIMG`), its place on the screen and its name image
    /// (`NameforTuningScreen`).
    pub tuning_picture: String,
    pub tuning_rect: [i32; 4],
    pub tuning_name: String,
    /// The row's `Functioninfo<n>` lines (the detail panels' function lines; `^0xAARRGGBB^text^` runs inside).
    // EVIDENCE: cshell+0x25b630 (the record's function line list at +1000: a count byte and the lines),
    // cshell+0xd7ec0 (the shop panel draws them)
    pub functions: Vec<String>,
    /// A supply case (type 0x14) whose first `FunctionValue0` is not -1: a HiSec case (the inbox row's HiSec mark).
    // EVIDENCE: cshell+0x69ff0 (the row's HiSec mark is shown for item type 0x14 when the item record's value list
    // cshell+0x69e80 (get(0, -1)) is not -1), specs/HISEC_2009.md 4.1
    pub keyed_case: bool,
    /// The row's first `FunctionValue0` (-1 without one) and `SubType`: a HiSec key is a function item (9000..) of
    /// SubType 19 whose value equals its case's.
    // EVIDENCE: HISEC_2009.md 4.1 (cshell+0x14cc00 / cshell+0x25c760)
    pub function_value: i32,
    pub sub_type: i32,
    /// use-windows: a weapon change item's target weapon (`ChangeItemID`; 0 without one).
    // EVIDENCE: cshell+0x25dd60 (the function-item rows: ChangeItemID -> record +0x1c), cshell+0x25c500 (its reader)
    pub change_item: i32,
    /// function-unequip: an in-use function item of this row can be taken out of / put back into its function slot
    /// (`IsCanUnEquip`; false without the key).
    // EVIDENCE: cshell+0x25dd60 (IsCanUnEquip -> record +0x241), cshell+0x1413c0 / 0x141480 (read before 0x368 / 0x36a)
    pub can_unequip: bool,
    /// use-items-sweep: a reticle item's picture for the colour window (`PreViewIcon`) and its shape (`ShapeType`,
    /// 0 without one).
    // EVIDENCE: cshell+0x25dd60 (PreviewIcon -> record +0x3b4, ShapeType -> record +0x20), cshell+0x25ac20 / 0x25ac80
    // (their readers)
    pub preview_icon: String,
    pub shape: i32,
}

/// The item-table columns of the eighteen modifier values a detail panel lists, in the order of their labels.
// EVIDENCE: cshell+0x25dd60 (the item-table loader: these keys -> the record's floats +0x1f8 .. +0x23c; the label
// string ids of the eighteen rows at the manager's +0x1650 .. +0x1694), cshell+0x25b670 (value k of a record),
// cshell+0x259af0 (label k)
pub const STAT_KEYS: [&str; 18] = [
    "ArmorHandicap",
    "MoveHandicap",
    "Protection",
    "ClipSize",
    "Slot",
    "Muzzle",
    "Stability",
    "Volume",
    "RecoilDec",
    "Tracer",
    "DamageDec",
    "FireRateDec",
    "AccuracyInc",
    "AimingspeedInc",
    "SelectSpeedDec",
    "ReloadSpeedDec",
    "DashStamina",
    "DashVelocity",
];
pub const STAT_LABELS: [i32; 18] = [
    0x14e5, 0x1651, 0x35c, 0x35d, 0x399, 0x5b4, 0x35f, 0x5b5, 0x3a1, 0x149, 0x14de, 0x14df, 0x14e0,
    0x36e, 0x1552, 0x1553, 0x1841, 0x1842,
];

/// The numbers of a table pair `(x, y)`.
fn table_pair(v: &str) -> Option<[f32; 2]> {
    let v = table_text(v);
    let (a, b) = v
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split_once(',')?;
    Some([table_float(a)?, table_float(b)?])
}

/// A table number (`0.6f`, `-50`).
fn table_float(v: &str) -> Option<f32> {
    v.trim().trim_end_matches(['f', 'F']).parse::<f32>().ok()
}

/// The weapon table's columns of a weapon's muzzle, sight and clip socket states (see [`ItemArt::socket_states`]).
// EVIDENCE: cshell+0x17bd20 (the weapon table row loader: these keys -> record +0x42c / +0x430 / +0x434),
// cshell+0x172300 (the record of an item number)
pub const SOCKET_KEYS: [&str; 3] = ["MuzzleOPtion", "SightOption", "MagazineOption"];

/// The item-table columns of the detail panel's gauges, in the panel's row order.
// DC-DEFAULT: CLI-723 the gauges read the weapon row's Damage / Portable / FireSpeed / FirstAccuracy /
// Recoil (the 2009 record fields +0x34 / +0x48 / +0x38 / +0x3c / +0xc4 of cshell+0x35b2f0 / 0x35b3a0 / 0x35b3f0 /
// 0x35b440 / 0x35b340; their column names are matched by meaning, the table reader is not traced)
pub const GAUGE_KEYS: [&str; 5] = ["Damage", "Portable", "FireSpeed", "FirstAccuracy", "Recoil"];

/// The item number base of an item-table section family (`<family><n>` rows: number = base + `ItemID`).
// SPEC: PROTOCOL_2009.md §4.5 (item number ranges: < 1000 weapons, 1000s options, 2000 camo 0xe, 3000 vest 0xf,
// 4000 head 0xb, 5000 face 0xc, 6000 face model 0xd, 7000 backpack 0x10, 8000 supply case 0x14, 9000 function item
// 0x15, 10000s characters); the families' type codes in the item table (`ItemType` 14 / 15 / 11 / 12 / 13 / 16 /
// 20 / 21) name the range
pub fn item_family_base(section: &str) -> Option<i32> {
    let family = section.trim_end_matches(|c: char| c.is_ascii_digit());
    Some(match family {
        "WeaponItem" => 0,
        "Option" => 1000,
        "CamoItem" => 2000,
        "ClassVest" => 3000,
        "HeadItem" => 4000,
        "FaceAttachItem" => 5000,
        "FaceModelItem" => 6000,
        "BackpackItem" => 7000,
        "Supply" => 8000,
        "UseItem" => 9000,
        "Character" => 10000,
        _ => return None,
    })
}

/// A table value: the text between its quotes (a trailing `//` comment dropped), else the trimmed value.
fn table_text(v: &str) -> String {
    let v = v.trim();
    match v.strip_prefix('"').and_then(|r| r.split_once('"')) {
        Some((inner, _)) => inner.trim().to_string(),
        None => v.split("//").next().unwrap_or("").trim().to_string(),
    }
}

/// Item number -> name and slot image of every item-table row with an `ItemID` and an `InventoryIcon`.
// EVIDENCE: cshell+0x149f80 (a shop cell's image is the item's slot image through cshell+0x35a990 / cshell+0x35b890,
// set with the size 0xfe x 0x56; its name through cshell+0x35af00)
pub fn item_art(doc: &dc_bute::ButeDoc) -> BTreeMap<i32, ItemArt> {
    let mut out = BTreeMap::new();
    for (tag, d) in doc.iter() {
        let Some(base) = item_family_base(tag) else {
            continue;
        };
        let Some(id) = d
            .get("ItemID")
            .and_then(|v| table_text(v).parse::<i32>().ok())
        else {
            continue;
        };
        let icon = d
            .get("InventoryIcon")
            .map(|v| table_text(v))
            .unwrap_or_default();
        let name = d.get("ItemName").map(|v| table_text(v)).unwrap_or_default();
        let text = |k: &str| d.get(k).map(|v| table_text(v)).unwrap_or_default();
        let gauges = GAUGE_KEYS.map(|k| text(k).parse::<i32>().unwrap_or(0));
        let stats =
            STAT_KEYS.map(|k| table_float(&text(k)).map_or(0, |v| (v * 1000.0).round() as i32));
        // the pane picture's place: the row's position and source size, both scaled by its scale
        // EVIDENCE: cshell+0x155d30 (the weapon's picture on the pane image 6: TuningScreenIMG through
        // cshell+0x35b9d0, TuningScreenIMGPOS / TuningScreenIMGREC / TuningScreenIMGScale through cshell+0x35bf60 /
        // 0x35bfa0 / 0x35bf30; the name image (2, 2) through cshell+0x35b950)
        // OBSERVED: shop-guns/cpp/shop_bb171_tuning_c0_p1.png (the M4A1 at (600, 170) x 0.6 =
        // (360, 102), 1024 x 512 x 0.6: its drawn pixels start at x 393 = 360 + 55 x 0.6)
        let scale = table_float(&text("TuningScreenIMGScale")).unwrap_or(1.0);
        let pos = d
            .get("TuningScreenIMGPOS")
            .and_then(|v| table_pair(v))
            .unwrap_or([0.0; 2]);
        let rec = d
            .get("TuningScreenIMGREC")
            .and_then(|v| table_pair(v))
            .unwrap_or([0.0; 2]);
        let tuning_rect = [pos[0], pos[1], rec[0], rec[1]].map(|v| (v * scale).round() as i32);
        if id >= 0 && !icon.is_empty() {
            out.entry(base + id).or_insert(ItemArt {
                name,
                icon,
                panel_picture: text("DetailInfoImage"),
                description: text("DetailDescription"),
                gauges,
                row_type: text("ItemType").parse::<i32>().ok(),
                socket_states: [0; 3],
                feature: text("FeatureString"),
                stats,
                attach_class: text("AttachItemtype").parse::<i32>().ok(),
                tuning_picture: text("TuningScreenIMG"),
                tuning_rect,
                tuning_name: text("NameforTuningScreen"),
                functions: (0..)
                    .map_while(|k| d.get(&format!("Functioninfo{k}")).map(|v| table_text(v)))
                    .collect(),
                keyed_case: base == 8000
                    && text("FunctionValue0").parse::<i32>().is_ok_and(|v| v != -1),
                function_value: text("FunctionValue0").parse::<i32>().unwrap_or(-1),
                sub_type: text("SubType").parse::<i32>().unwrap_or(-1),
                change_item: table_float(&text("ChangeItemID")).map_or(0, |v| v as i32),
                can_unequip: text("IsCanUnEquip").parse::<i32>().is_ok_and(|v| v != 0),
                preview_icon: text("PreViewIcon"),
                shape: table_float(&text("ShapeType")).map_or(0, |v| v as i32),
            });
        }
    }
    out
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').trim().to_string()
}

impl Wares {
    /// [`Wares::from_rom`], read once per ROM folder and process.
    pub fn cached(rom: Option<&Path>) -> Arc<Wares> {
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::sync::{Mutex, OnceLock};
        static CACHE: OnceLock<Mutex<HashMap<(PathBuf, Vec<String>), Arc<Wares>>>> = OnceLock::new();
        let Some(rom) = rom else {
            return Arc::new(Wares::default());
        };
        let mut c = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let plain = c
            .entry((rom.to_path_buf(), Vec::new()))
            .or_insert_with(|| Arc::new(Wares::from_rom(rom)))
            .clone();
        // VAR-1: the loaded variants have their own pictures, so each set of them has its own table
        let variants = crate::camo::variants();
        if variants.is_empty() {
            return plain;
        }
        let key: Vec<String> = variants.iter().map(|v| format!("{v:?}")).collect();
        c.entry((rom.to_path_buf(), key))
            .or_insert_with(|| Arc::new(plain.with_variants(&variants)))
            .clone()
    }

    /// This table with a row for each variant: its base gun's row, with its own name and painted pictures.
    pub fn with_variants(&self, variants: &[dc_bute::modpatch::VariantInfo]) -> Wares {
        let mut wares = self.clone();
        for v in variants {
            let Some(mut art) = self.art.get(&v.base).cloned() else {
                continue;
            };
            // VAR-5: it wears another row's look, so it shows that row's pictures
            if let Some(look) = v.look_of.and_then(|row| self.art.get(&row)) {
                art.icon = look.icon.clone();
                art.panel_picture = look.panel_picture.clone();
                art.tuning_picture = look.tuning_picture.clone();
            }
            let paint = |picture: &mut String| {
                if let Some(path) = crate::camo::art_path(v, picture).filter(|_| !picture.is_empty()) {
                    *picture = path;
                }
            };
            paint(&mut art.icon);
            paint(&mut art.panel_picture);
            paint(&mut art.tuning_picture);
            if let Some(icon) = &v.icon {
                art.icon = icon.clone();
            }
            if let Some(name) = &v.name {
                art.name = name.clone();
            }
            wares.art.insert(v.row, art);
        }
        wares
    }

    pub fn from_rom(rom: &Path) -> Wares {
        let shop = dc_bute::ShopCatalog::from_rom(rom).ok();
        let items = dc_bute::ItemsTable::from_rom(rom).ok();
        let mut names = BTreeMap::new();
        if let Some(s) = &shop {
            for pid in s.view_list() {
                for (index, _, _) in s.product(pid).map(|p| p.items.clone()).unwrap_or_default() {
                    if let Some(row) = s.info(index) {
                        names
                            .entry(row.itemid)
                            .or_insert_with(|| unquote(&row.name));
                    }
                }
            }
        }
        // DC-DEFAULT: LG-51 items the shop does not sell and the table gives no icon (the default uniform, vest and head
        // rows) are labelled with the table's own name; a name the table keeps in the Korean code page is replaced
        // by "Default <part>"
        if let Some(t) = &items {
            for (tag, d) in t.doc().iter() {
                let (Some(base), Some(id)) = (
                    item_family_base(tag),
                    d.get("ItemID")
                        .and_then(|v| table_text(v).parse::<i32>().ok()),
                ) else {
                    continue;
                };
                let name = d.get("ItemName").map(|v| table_text(v)).unwrap_or_default();
                let part = match base {
                    2000 => "Uniform",
                    3000 => "Vest",
                    4000 => "Head",
                    _ => continue,
                };
                let name = if name.is_ascii() && !name.is_empty() {
                    name
                } else {
                    format!("Default {part}")
                };
                names.entry(base + id).or_insert(name);
            }
        }
        let mut art = items
            .as_ref()
            .map(|t| item_art(t.doc()))
            .unwrap_or_default();
        for (n, a) in art.iter_mut() {
            if !a.name.is_ascii() {
                if let Some(d) = names.get(n).filter(|d| d.starts_with("Default ")) {
                    a.name = d.clone();
                }
            }
        }
        if let Ok(weapons) = dc_bute::WeaponsTable::from_rom(rom) {
            for (n, a) in art.range_mut(0..1000) {
                if let Some(row) = weapons.weapon(*n) {
                    // both spellings of the muzzle key (K1A / K7 rows) and a missing key read as open, as dc-bute
                    // reads them (CLI-1606 amendment)
                    a.socket_states = SOCKET_KEYS.map(|k| dc_bute::socket_state(row, k));
                }
            }
        }
        Wares {
            shop,
            items,
            names,
            art,
        }
    }

    /// The item table's name and slot image of item number `n`.
    /// The key item number that opens HiSec case `case_no` (a function item of SubType 19 with the case's first
    /// `FunctionValue0`).
    // EVIDENCE: HISEC_2009.md 4.1 (cshell+0x25c760(type 0x15, index 0, value, subtype 0x13))
    pub fn key_of(&self, case_no: i32) -> Option<i32> {
        let v = self
            .art
            .get(&case_no)
            .filter(|a| a.keyed_case)?
            .function_value;
        self.art
            .range(9000..10000)
            .find(|(_, a)| a.sub_type == 19 && a.function_value == v)
            .map(|(n, _)| *n)
    }

    /// The slot pictures to try for item `n`: the item table's `InventoryIcon`, then a stand-in for the default head
    /// (its row names no icon, or one the ROM does not hold).
    // EVIDENCE: cshell+0x25b890 (the cell's picture is the record's image name, "" when the row names none: 2009 draws
    // nothing for the default rows, cshell+0x14a700)
    // DC-DEFAULT: CLI-1004 (playtest report: "body says medium vest equipped but no vest icon visible") the default rows
    // 2000 / 3000 / 4000 get the ROM's Woodland camo / Normal AR vest / default helmet slot pictures
    // DC-DEFAULT: CLI-1093 (playtest report: "default vest color should not be woodland", "no vest is default") the
    // default uniform row draws no picture, as 2009 and the reference client (OBSERVED shop-r12/pairs/
    // item3_reference_body_2560.png: the default uniform and vest rows without a picture); the vest is no default item
    // any more (LG-90); only the default head keeps its stand-in
    pub fn icons_of(&self, n: i32) -> Vec<String> {
        // VAR-1: a variant's own row, else (a table made before it loaded) its base gun's
        let n = if self.art.contains_key(&n) { n } else { crate::camo::art_row(n) };
        let mut out: Vec<String> = self
            .art
            .get(&n)
            .filter(|a| !a.icon.is_empty())
            .map(|a| a.icon.clone())
            .into_iter()
            .collect();
        let t = match n {
            4000 => "UI_SCR_ItemImg_Slot\\Head\\Helmet_Default_Slot.dtx",
            _ => return out,
        };
        out.push(t.to_string());
        out
    }

    /// Whether item `n` is a character (item type 0x16, the row's own `ItemType` 22; the 10000s share the number range
    /// with the specialists, 0x1e).
    // EVIDENCE: cshell+0x149d20 (the body list's first row is slot 0xe of item type 0x16), cshell+0x14a700 (its page is
    // the Black Market shelf 3)
    pub fn is_person_item(&self, n: i32) -> bool {
        self.kind_of(n) == Some(0x16) || self.art_of(n).and_then(|a| a.row_type) == Some(0x16)
    }

    pub fn art_of(&self, n: i32) -> Option<&ItemArt> {
        // VAR-1: a variant's own row, else (a table made before it loaded) its base gun's
        self.art.get(&n).or_else(|| self.art.get(&crate::camo::art_row(n)))
    }

    /// function-unequip: item `n`'s row has `IsCanUnEquip` set.
    pub fn can_unequip(&self, n: i32) -> bool {
        self.art_of(n).is_some_and(|a| a.can_unequip)
    }

    /// function-unequip: function item `r` is in the function list (its storage cell shows the Equiped mark): it sits
    /// in a function slot, or it is in use and its row has no `IsCanUnEquip` (such an item stays on until it ends).
    // EVIDENCE: cshell+0x2292e0 (the list: the records of the slots, then every record of type 0x15 in use whose row
    // +0x241 is 0), cshell+0x14a700 (a type 0x15 cell shows Equiped_Mark when cshell+0x226800 finds its unique id)
    pub fn in_function_list(&self, store: &Store, r: &InvRecord) -> bool {
        store.in_function_slot(r) || (r.in_use && !self.can_unequip(r.item_no as i32))
    }

    /// After a match: the first record of the function list of each bonus pass kind (`SubType` 13 EXP, 14 GP, 15
    /// both) loses one use (the lobby counts the same match and removes a pass at 0 with 0x33f).
    // EVIDENCE: cshell+0x134b80 (the result rows' fill: for kinds 0xd..0xf the function list's record of the kind,
    // cshell+0x227070, has its count +0x14 lowered by one, then the inventory view is refreshed)
    // DC-DEFAULT: CLI-1880 (proposed) on every entry of the result screen, a record without uses left is not
    // lowered (the 2009 condition on the shell's vfunc +0xa8 is not traced)
    pub fn count_down_passes(&self, store: &mut Store) {
        let uids: Vec<u32> = (13..=15)
            .filter_map(|k| {
                self.function_list(store)
                    .into_iter()
                    .find(|r| {
                        self.art_of(r.item_no as i32)
                            .is_some_and(|a| a.sub_type == k)
                    })
                    .map(|r| r.uid)
            })
            .collect();
        for r in store.items.iter_mut().filter(|r| uids.contains(&r.uid)) {
            r.count = r.count.saturating_sub(1);
        }
    }

    /// The function list in its 2009 order: the record of each function slot (slot order), then every other function
    /// item in use whose row has no `IsCanUnEquip` (record order).
    // EVIDENCE: cshell+0x2292e0 (the seven slots first, then the records of type 0x15 in use whose row +0x241 is 0)
    pub fn function_list<'a>(&self, store: &'a Store) -> Vec<&'a InvRecord> {
        let mut list: Vec<&InvRecord> = (0..store.function_slots.len())
            .filter_map(|s| {
                store
                    .items
                    .iter()
                    .find(|r| store.function_slot_of(r) == Some(s))
            })
            .collect();
        for r in &store.items {
            if r.in_use
                && self.kind_of(r.item_no as i32) == Some(0x15)
                && !self.can_unequip(r.item_no as i32)
                && !list.iter().any(|o| o.uid == r.uid)
            {
                list.push(r);
            }
        }
        list
    }

    /// The extra weapon slots of backpack item `n` (the item table's `MultyInvenSlot`; 0 when unknown).
    // EVIDENCE: cshell+0x227240 (the shell's backpack slot count +0x2dc is read from the equipped backpack's item row)
    pub fn backpack_slots(&self, n: i32) -> i32 {
        self.items.as_ref().map_or(0, |t| t.backpack_slot_count(n))
    }

    /// The type of item number `n` (the client's item number ranges; weapons and options by the item table).
    // SPEC: PROTOCOL_2009.md §4.5 (< 1000 weapons by the item table, 1000s options 0x11..0x13, 2000 0xe, 3000 0xf,
    // 4000 0xb, 5000 0xc, 6000 0xd, 7000 0x10, 8000 0x14, 9000 0x15, 10000s 0x1e, 11000 0x17, 12000 0x18, 13000 0x19)
    // EVIDENCE: cshell+0x25c010
    pub fn kind_of(&self, n: i32) -> Option<i32> {
        let n = crate::camo::art_row(n);
        Some(match n {
            0..=999 => {
                let t = self.items.as_ref().map_or(-1, |t| t.weapon_class_code(n));
                if t < 0 {
                    return None;
                }
                t
            }
            1000..=1999 => {
                let k = self.items.as_ref().map_or(-1, |t| t.option_kind(n));
                if k >= 0 {
                    k
                } else {
                    0x11
                }
            }
            2000..=2999 => 0xe,
            3000..=3999 => 0xf,
            4000..=4999 => 0xb,
            5000..=5999 => 0xc,
            6000..=6999 => 0xd,
            7000..=7999 => 0x10,
            8000..=8999 => 0x14,
            9000..=9999 => 0x15,
            10000..=10999 => 0x1e,
            11000..=11999 => 0x17,
            12000..=12999 => 0x18,
            13000..=13999 => 0x19,
            _ => return None,
        })
    }

    /// The catalogue's name of item number `n` (`#n` for an item the catalogue does not sell).
    // DC-DEFAULT: CLI-545 items and products are labelled with the catalogue's own names (the 2009 cell
    // label lookup cshell+0x35a990 / 0x35b890 is not ported)
    // EVIDENCE: cshell+0x149f80 (the cell's name is the item's own name, cshell+0x35af00)
    pub fn label_of(&self, n: i32) -> String {
        if let Some(name) = crate::camo::variant(n).and_then(|v| v.name) {
            return name;
        }
        let n = crate::camo::art_row(n);
        self.art
            .get(&n)
            .map(|a| a.name.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| self.names.get(&n).cloned())
            .unwrap_or_else(|| format!("#{n}"))
    }

    /// Product `pid` as the shop shows it (`prices` from 0x359 win over the catalogue's sale price).
    pub fn ware(&self, pid: u32, prices: &BTreeMap<u32, u32>) -> Option<Ware> {
        let s = self.shop.as_ref()?;
        let p = s.product(pid as i32)?;
        let row = p.items.first().and_then(|(index, _, _)| s.info(*index));
        let item_no = row.map_or(-1, |r| r.itemid);
        let (display, unlock) = row.map_or((0, 0), |r| {
            if p.sale_type == 1 {
                (r.nx_display, r.nx_unlock)
            } else {
                (r.gp_display, r.gp_unlock)
            }
        });
        let art = self.art.get(&item_no);
        Some(Ware {
            product: pid,
            name: art
                .map(|a| a.name.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| unquote(&p.name)),
            icon: art.map(|a| a.icon.clone()),
            item_no,
            kind_of: self.kind_of(item_no),
            tag_kind: art.and_then(|a| a.row_type).or(self.kind_of(item_no)),
            sale_type: p.sale_type,
            price: prices
                .get(&pid)
                .copied()
                .unwrap_or(p.sale_price.max(0) as u32),
            display,
            unlock_exp: unlock.max(0) as u32,
            marks: [false; 3],
            period: p.period,
            consume: p.consume,
        })
    }

    /// An owned item as the detail panel shows it (no product: the item's own name and class; the owned item's panel
    /// shows none of the PERMANENT / TEMPORARY / HiSec marks).
    // EVIDENCE: cshell+0xd6450 (the owned item's panel hides the HiSec, TEMPORARY and PERMANENT marks, +0x138 / +0x13c
    // / +0x140, and shows none again); OBSERVED: lobby23/pairs/item3_storage_detail.png
    pub fn item_ware(&self, n: i32) -> Ware {
        let art = self.art_of(n);
        Ware {
            product: 0,
            name: self.label_of(n),
            item_no: n,
            kind_of: self.kind_of(n),
            tag_kind: art.and_then(|a| a.row_type).or(self.kind_of(n)),
            sale_type: 0,
            price: 0,
            icon: art.map(|a| a.icon.clone()),
            display: 0,
            unlock_exp: 0,
            marks: [false; 3],
            // a period keeps the PERMANENT mark off
            period: 1,
            consume: 0,
        }
    }

    /// The shop catalogue of the ROM (the purchase window reads the products of a cell).
    pub fn catalog(&self) -> Option<&dc_bute::ShopCatalog> {
        self.shop.as_ref()
    }

    /// The products the catalogue itself marks as shown (tools and tests that have no 0x351).
    pub fn catalogue_ids(&self) -> Vec<u32> {
        self.shop.as_ref().map_or_else(Vec::new, |s| {
            s.view_list().into_iter().map(|p| p as u32).collect()
        })
    }

    /// The cells of shelf (`tab`, `cat`): one per item of the shown list's products, each the item's cheapest
    /// product, in the order of the items' display numbers.
    // EVIDENCE: cshell+0x255860 (a product whose item already has a cell only lowers that cell's price: `if (price <
    // cell price) cell price = price`, through the cell lookup cshell+0x34fb10; every category list is then sorted
    // through cshell+0x251ec0 / cshell+0x250ca0 by the cell's +0x1c, the item row's display number of the pass)
    // DC-DEFAULT: CLI-545 the cell buys its cheapest product (the 2009 period choice of the purchase
    // dialog is not built)
    pub fn shelf(&self, store: &Store, tab: usize, cat: usize) -> Vec<Ware> {
        let Some(view) = &store.view else {
            return Vec::new();
        };
        let mut out: Vec<Ware> = Vec::new();
        for w in view
            .iter()
            .filter_map(|pid| self.ware(*pid, &store.prices))
            .filter(|w| ware_on_shelf(w, store, tab, cat))
        {
            let mut w = w;
            for (m, list) in w.marks.iter_mut().zip(&store.highlight) {
                *m = list.contains(&w.product);
            }
            if let Some(c) = out
                .iter_mut()
                .find(|c| c.item_no == w.item_no && c.sale_type == w.sale_type)
            {
                for (a, b) in c.marks.iter_mut().zip(w.marks) {
                    *a |= b;
                }
                w.marks = c.marks;
            }
            match out
                .iter_mut()
                .find(|c| c.item_no == w.item_no && c.sale_type == w.sale_type)
            {
                Some(c) if w.price < c.price => *c = w,
                Some(_) => {}
                None => out.push(w),
            }
        }
        out.sort_by_key(|w| w.display);
        out
    }
}

/// Shelves: left tab 0 weapons, 1 body, 2 black market; 3 the tuning shop's options; 4 the rest.
pub const TAB_WEAPONS: usize = 0;
pub const TAB_BODY: usize = 1;
pub const TAB_MARKET: usize = 2;
pub const TAB_OPTIONS: usize = 3;

/// The shelf (tab list, category) of an item type.
// DISASM: cshell+0x256c85..0x256eb1 (the catalogue build cshell+0x255860 files each product by its item type through
// the jump table at cshell+0x257b58 / byte table cshell+0x257ba0 into the category lists of the tab objects at
// +0x15d4: +4 weapons (categories +0, +0x10, +0x20, +0x30, +0x40, +0x50), +0x14 body, +0x24 black market, +0x34
// options, +0x44 the rest)
// DC-DEFAULT: CLI-544 function items (types 0x15 / 0x17) all go to the black market's third category (the
// 2009 split by the item's sub type, cshell+0x256dd9, is not ported); types 0x18 / 0x19 have no shelf
pub fn shelf_of(ty: i32) -> Option<(usize, usize)> {
    Some(match ty {
        0 | 1 | 2 | 3 | 8 | 9 | 0xa | 0x1a => (TAB_WEAPONS, 0),
        4 | 5 => (TAB_WEAPONS, 1),
        7 => (TAB_WEAPONS, 2),
        6 | 0x1b => (TAB_WEAPONS, 3),
        0x14 => (TAB_WEAPONS, 4),
        0xb => (TAB_BODY, 0),
        0xc | 0xd => (TAB_BODY, 1),
        0xe => (TAB_BODY, 2),
        0xf => (TAB_BODY, 3),
        0x10 => (TAB_BODY, 4),
        0x15 | 0x17 => (TAB_MARKET, 2),
        0x16 => (TAB_MARKET, 3),
        0x1d | 0x1e => (TAB_MARKET, 5),
        0x11 => (TAB_OPTIONS, 0),
        0x12 => (TAB_OPTIONS, 1),
        0x13 => (TAB_OPTIONS, 2),
        0x1c => (4, 0),
        _ => return None,
    })
}

/// The shop shelf (tab list, category) of a cell by its sale type and display number: the thousands of the number
/// name the category. GP: 0..4 the weapons tab's Primary, Secondary, Melee, Support and Other, 5..9 the body tab's
/// five categories, 31 the weapons tab's Special; credit: 10 / 11 / 12 / 31 the black market's Func., Merc., Gear and
/// Special; either: 13..30 the modification shop (three per weapon class: muzzle, sight, clip).
// OBSERVED: shop-guns/cpp/ (the reference client's shop against dc-lobby-server,
// 2026-10-27): every GP category lists exactly the shown products whose GP display number has these
// thousands (Primary 47, Secondary 9, Melee 5, Support 9, Other 7, Special 13, Head 12, Face 9, Uniform 4, Vest 3,
// Backpack 2), the modification tabs AR 9 / SR 6 / MG 3 / SMG 9 / SHOTGUN 3 / PISTOL 9 the groups 13..30
// DC-DEFAULT: CLI-695 (proposed) shop categories by display-number thousands (the 2009 type switch of
// cshell+0x255860 with its sub type split cshell+0x35c480 gives the same lists for the ROM's rows)
pub fn display_shelf(sale_type: i32, display: i32) -> Option<(usize, usize)> {
    if display <= 0 {
        return None;
    }
    let g = (display / 1000) as usize;
    Some(match (sale_type, g) {
        (_, 13..=30) => (TAB_OPTIONS, g - 13),
        (0, 0..=4) => (TAB_WEAPONS, g),
        (0, 5..=9) => (TAB_BODY, g - 5),
        (0, 31) => (TAB_WEAPONS, 5),
        (1, 10..=12) => (TAB_MARKET, g - 8),
        (1, 31) => (TAB_MARKET, 5),
        _ => return None,
    })
}

/// Whether a product shows on shelf (`tab`, `cat`): the weapons and body tabs list GP products, the black market
/// credit products (Hot = list 0, New = list 1, the others by display number), the options tab (`cat` = the
/// display group 13..30 less 13) the option products of either sale type.
// DC-DEFAULT: CLI-544 tabs 0 / 1 show GP products (SaleType 0), the black market credit products
// (SaleType 1); its Hot and New categories are the highlight lists 0x353 / 0x355
pub fn ware_on_shelf(w: &Ware, store: &Store, tab: usize, cat: usize) -> bool {
    let shelf = display_shelf(w.sale_type, w.display);
    match tab {
        TAB_WEAPONS | TAB_BODY => w.sale_type == 0 && shelf == Some((tab, cat)),
        TAB_MARKET => {
            w.sale_type == 1
                && match cat {
                    0 => store.highlight[0].contains(&w.product),
                    1 => store.highlight[1].contains(&w.product),
                    c => shelf == Some((TAB_MARKET, c)),
                }
        }
        TAB_OPTIONS => shelf == Some((TAB_OPTIONS, cat)),
        _ => false,
    }
}

/// The items of the inventory on shelf (`tab`, `cat`) (the storage page).
pub fn stored(store: &Store, wares: &Wares, tab: usize, cat: usize) -> Vec<InvRecord> {
    store
        .items
        .iter()
        .filter(|r| {
            let shelf = wares.kind_of(r.item_no as i32).and_then(shelf_of);
            match tab {
                TAB_MARKET => matches!(shelf, Some((TAB_MARKET, _))),
                _ => shelf == Some((tab, cat)),
            }
        })
        .cloned()
        .collect()
}

/// The equipment slot of an item type.
// EVIDENCE: cshell+0x259f00 (dc_bute::equip_slot_by_class_code)
pub fn own_slot(wares: &Wares, item_no: u32) -> Option<u32> {
    wares
        .kind_of(item_no as i32)
        .and_then(dc_bute::equip_slot_by_class_code)
        .map(|s| s as u32)
}

/// The body list's Character row (item type 0x16, slot 0xe).
pub const CHARACTER_SLOT: usize = 14;

/// The left pane's equipment rows of a tab: the slots and the category each row opens in the storage.
// EVIDENCE: cshell+0x247100 (weapons: equipment slots 0..3 in list 2 of window 2; an empty slot shows strings
// 0x1663, 0x1664, 0x15da, 0x155c), cshell+0x259f00 (slot of a type: 4 head 0xb, 5 0xd, 6 0xc, 7 0xe, 8 0xf, 9 0x10),
// cshell+0x249d20 (body: list 4 of window 2 holds slots 0xe, 4, 6, 7, 8, 9 in that order; an empty slot shows
// strings 0x5b6, 0x33d, 0x1634, 0x33e, 0x33f, 0x340)
// DC-DEFAULT: CLI-546 a press on a row opens the storage page on the row's category; CLI-732 (proposed) the body
// list's first row (slot 0xe) opens the first body category
pub fn equip_rows(tab: usize) -> &'static [(usize, usize, i32)] {
    // (slot, category, empty-slot string)
    match tab {
        TAB_WEAPONS => &[
            (0, 0, 0x1663),
            (1, 1, 0x1664),
            (2, 2, 0x15da),
            (3, 3, 0x155c),
        ],
        TAB_BODY => &[
            (14, 0, 0x5b6),
            (4, 0, 0x33d),
            (6, 1, 0x1634),
            (7, 2, 0x33e),
            (8, 3, 0x33f),
            (9, 4, 0x340),
        ],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(uid: u32, item_no: u32) -> InvRecord {
        InvRecord {
            uid,
            item_no,
            ..InvRecord::default()
        }
    }

    #[test]
    fn a_variant_gets_its_own_row_with_painted_pictures() {
        let mut wares = Wares::default();
        wares.art.insert(
            25,
            ItemArt {
                name: "G36E".into(),
                icon: "icon.dtx".into(),
                panel_picture: "panel.dtx".into(),
                ..ItemArt::default()
            },
        );
        let variant = dc_bute::modpatch::VariantInfo {
            row: 500,
            base: 25,
            name: Some("G36E Candy".into()),
            icon: None,
            hh_skins: [(0, "@camo:paint:25:HHSkin0:@mod:p/assets/c.dtx".to_string())].into_iter().collect(),
            pv_skins: Default::default(),
            look_of: None,
        };
        let wares = wares.with_variants(&[variant]);
        let art = wares.art.get(&500).unwrap();
        assert_eq!(art.name, "G36E Candy");
        assert_eq!(art.icon, "@camoart:@mod:p/assets/c.dtx|icon.dtx");
        assert_eq!(art.panel_picture, "@camoart:@mod:p/assets/c.dtx|panel.dtx");
        assert_eq!(art.tuning_picture, "", "a picture the base gun lacks stays empty");
        assert_eq!(wares.art.get(&25).unwrap().icon, "icon.dtx", "the base gun keeps its own");
    }

    #[test]
    fn shelves_follow_the_2009_type_table() {
        assert_eq!(shelf_of(0), Some((TAB_WEAPONS, 0)));
        assert_eq!(shelf_of(5), Some((TAB_WEAPONS, 1)));
        assert_eq!(shelf_of(7), Some((TAB_WEAPONS, 2)));
        assert_eq!(shelf_of(0x1b), Some((TAB_WEAPONS, 3)));
        assert_eq!(shelf_of(0x14), Some((TAB_WEAPONS, 4)));
        assert_eq!(shelf_of(0xd), Some((TAB_BODY, 1)));
        assert_eq!(shelf_of(0x10), Some((TAB_BODY, 4)));
        assert_eq!(shelf_of(0x12), Some((TAB_OPTIONS, 1)));
        assert_eq!(shelf_of(0x18), None);
    }

    #[test]
    fn shop_shelves_follow_the_display_number_thousands() {
        assert_eq!(display_shelf(0, 1), Some((TAB_WEAPONS, 0)));
        assert_eq!(display_shelf(0, 4001), Some((TAB_WEAPONS, 4)));
        assert_eq!(display_shelf(0, 31003), Some((TAB_WEAPONS, 5)));
        assert_eq!(display_shelf(0, 5002), Some((TAB_BODY, 0)));
        assert_eq!(display_shelf(0, 9001), Some((TAB_BODY, 4)));
        assert_eq!(display_shelf(1, 10002), Some((TAB_MARKET, 2)));
        assert_eq!(display_shelf(1, 12007), Some((TAB_MARKET, 4)));
        assert_eq!(display_shelf(1, 31000), Some((TAB_MARKET, 5)));
        assert_eq!(display_shelf(0, 13001), Some((TAB_OPTIONS, 0)));
        assert_eq!(display_shelf(0, 30003), Some((TAB_OPTIONS, 17)));
        assert_eq!(display_shelf(0, 0), None);
        assert_eq!(display_shelf(1, 1), None);
    }

    #[test]
    fn item_types_follow_the_number_ranges_without_a_rom() {
        let w = Wares::default();
        assert_eq!(w.kind_of(5), None);
        assert_eq!(w.kind_of(1001), Some(0x11));
        assert_eq!(w.kind_of(4001), Some(0xb));
        assert_eq!(w.kind_of(7002), Some(0x10));
        assert_eq!(w.kind_of(9001), Some(0x15));
        assert_eq!(own_slot(&w, 4001), Some(4));
        assert_eq!(own_slot(&w, 7001), Some(9));
        assert_eq!(w.label_of(4001), "#4001");
    }

    #[test]
    fn equipment_slots_are_matched_to_records_in_order() {
        let mut s = Store {
            items: vec![rec(10, 4001), rec(11, 4001), rec(12, 7001)],
            ..Store::default()
        };
        s.equipped = [-1; 15];
        s.equipped[4] = 4001;
        s.equipped[9] = 7001;
        assert_eq!(s.slot_of(&s.items[0].clone()), Some(4));
        assert_eq!(s.slot_of(&s.items[1].clone()), None);
        assert_eq!(s.in_slot(9).map(|r| r.uid), Some(12));
        assert!(s.in_slot(0).is_none());
    }

    #[test]
    fn inbox_paging_asks_until_the_total_is_held() {
        let mut s = Store::default();
        assert_eq!(s.next_inbox_page(), Some(1));
        let row = |entry| InboxRow {
            entry,
            ..InboxRow::default()
        };
        s.apply(&LinkEvent::Inbox {
            total: 6,
            rows: (1..=4).map(row).collect(),
        });
        assert_eq!(s.next_inbox_page(), Some(1));
        s.apply(&LinkEvent::Inbox {
            total: 6,
            rows: (5..=6).map(row).collect(),
        });
        assert_eq!(s.inbox.len(), 6);
        assert_eq!(s.next_inbox_page(), None);
        s.apply(&LinkEvent::InboxStale);
        assert!(s.inbox_stale);
    }

    /// CLI-741 (amended) / CLI-1880: the function list holds the slot records, then the in-use records without
    /// IsCanUnEquip; the black market list sorts it by SubType; after a match each bonus pass kind's record loses a
    /// use, never below 0.
    #[test]
    fn function_list_market_order_and_the_pass_count_down() {
        let mut wares = Wares::default();
        for (n, sub, unequip) in [
            (9019, 13, true),
            (9020, 14, true),
            (9010, 10, false),
            (9011, 10, false),
        ] {
            wares.art.insert(
                n,
                ItemArt {
                    sub_type: sub,
                    can_unequip: unequip,
                    ..ItemArt::default()
                },
            );
        }
        let used = |uid, n, count| InvRecord {
            in_use: true,
            count,
            ..rec(uid, n)
        };
        let mut store = Store {
            items: vec![used(1, 9010, 0), used(2, 9020, 1), used(3, 9019, 3), rec(4, 9011)],
            function_slots: vec![-1, 9019, 9020, -1, -1, -1, -1],
            ..Store::default()
        };
        let uids = |l: Vec<&InvRecord>| l.iter().map(|r| r.uid).collect::<Vec<_>>();
        assert_eq!(uids(wares.function_list(&store)), [3, 2, 1]);
        assert_eq!(uids(super::super::pane::market_records(&wares, &store)), [1, 3, 2]);
        wares.count_down_passes(&mut store);
        let counts = |s: &Store| s.items.iter().map(|r| r.count).collect::<Vec<_>>();
        assert_eq!(counts(&store), [0, 0, 2, 0]);
        wares.count_down_passes(&mut store);
        assert_eq!(counts(&store), [0, 0, 1, 0]);
    }
}
