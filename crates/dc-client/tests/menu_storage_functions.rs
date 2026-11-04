//! storage-functions: the storage page's Discard (shop screen 0x16) and the option storage's Discard / Extend Use
//! (Modification screen 0x14), driven by presses on the ROM layout's real widget rectangles (headless, no lobby
//! server: the requests are read back from the link's last-sent hook, the replies fed in).
//!
//! ROM-gated (skipped without `CA_ROM`).
// SPEC: PROTOCOL_2009.md §4.2 (0x334 / 0x335, 0x328)
// EVIDENCE: cshell+0x14f830 (1011 Discard), cshell+0x146840 (its refusals), cshell+0x142ca0 (0x334)

use dc_client::menu::link::{InvRecord, LinkEvent, LinkOptions};
use dc_client::menu::shop::{
    grid_cells, CMD_THROW_OUT, LEFT_WINDOW, STORAGE_GRID, STORAGE_WINDOW, WEAPON_LIST,
};
use dc_client::menu::storage::{own_slot, shelf_of, Wares, TAB_WEAPONS};
use dc_client::menu::{pages, Go, Menu, MenuInput, MenuKey, MenuOptions};
use dc_ui_data::WidgetKind;

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

fn tap(m: &mut Menu, p: [f32; 2]) {
    m.input(MenuInput::Move(p));
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
}

fn mid(c: [i32; 4]) -> [f32; 2] {
    [(c[0] + c[2] / 2) as f32, (c[1] + c[3] / 2) as f32]
}

fn button_mid(m: &Menu, cmd: i32) -> [f32; 2] {
    let b = m
        .sheet()
        .button(cmd)
        .filter(|b| b.shown)
        .unwrap_or_else(|| panic!("no shown button {cmd}"));
    mid([b.pos[0], b.pos[1], b.size[0], b.size[1]])
}

fn primaries(m: &Menu, n: usize) -> Vec<u32> {
    let wares = Wares::cached(m.cat.rom.as_deref());
    let v: Vec<u32> = (0..1000)
        .filter(|n| {
            wares.kind_of(*n as i32).and_then(shelf_of) == Some((TAB_WEAPONS, 0))
                && own_slot(&wares, *n) == Some(0)
        })
        .take(n)
        .collect();
    assert_eq!(v.len(), n, "primary weapons in the item table");
    v
}

/// The storage page of slot 0 with the given records (the first one equipped in slot 0).
fn storage_page(m: &mut Menu, items: Vec<InvRecord>) {
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[0] = items[0].item_no as i32;
    m.data.store.items = items;
    m.open(Go::Shop);
    let l = m
        .sheet()
        .part(LEFT_WINDOW, WEAPON_LIST, WidgetKind::List)
        .expect("weapon list")
        .pos;
    tap(m, [l[0] as f32 + 10.0, l[1] as f32 + 10.0]);
    let shown = m
        .sheet()
        .parts
        .iter()
        .position(|p| p.kind == WidgetKind::Window && p.layer == STORAGE_WINDOW)
        .is_some_and(|i| m.sheet().visible(i));
    assert!(shown, "the storage page is up");
}

fn rec(uid: u32, item_no: u32) -> InvRecord {
    InvRecord {
        uid,
        item_no,
        end: 1,
        ..Default::default()
    }
}

/// CLI-770: Discard refuses the equipped item (0x1642 titled 0x1826) and the default one (0x437), asks in the 2009
/// confirmation box (0x372 titled 0x1820, Yes / No) for another one, sends 0x334 on Yes and drops the record on the
/// ok reply.
#[test]
fn rom_storage_discard_refuses_equipped_and_default_items_then_asks_and_sends_0x334() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let p = primaries(&m, 3);
    let mut default = rec(9, p[2]);
    default.slot_kind = 1;
    storage_page(&mut m, vec![rec(7, p[0]), rec(8, p[1]), default]);
    let cells = grid_cells(&m.cat, pages::SHOP, STORAGE_WINDOW, STORAGE_GRID);
    let discard = button_mid(&m, CMD_THROW_OUT);
    // the equipped one: CLI-1265 the page opens with its first cell selected
    tap(&mut m, discard);
    let q = m.popup().cloned().expect("refusal box");
    assert_eq!((q.text, q.title), (m.cat.text(0x1642), 0x1826));
    m.input(MenuInput::Key(MenuKey::Enter));
    // the default one
    tap(&mut m, mid(cells[2]));
    tap(&mut m, discard);
    let q = m.popup().cloned().expect("refusal box");
    assert_eq!((q.text, q.title), (m.cat.text(0x437), 0x346));
    m.input(MenuInput::Key(MenuKey::Enter));
    // the free one: the confirmation, Yes sends 0x334
    tap(&mut m, mid(cells[1]));
    let sent = m.link_sent();
    tap(&mut m, discard);
    let q = m.popup().cloned().expect("confirmation box");
    assert_eq!(
        (q.text.as_str(), q.title),
        (m.cat.text(0x372).as_str(), 0x1820)
    );
    assert_eq!(q.captions, Some([0x35, 0x36]));
    assert_eq!(m.link_sent(), sent, "nothing sent before Yes");
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), format!("Discard(8, {})", p[1]));
    m.feed_event(LinkEvent::Discarded { ok: true });
    m.update();
    assert!(!m.data.store.items.iter().any(|r| r.uid == 8));
    assert_eq!(m.data.store.items.len(), 2);
}

/// CLI-770: a weapon with an option on it is refused (0x40c).
#[test]
fn rom_storage_discard_refuses_a_weapon_with_an_attachment() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let p = primaries(&m, 2);
    let mut opt = rec(30, 1001);
    opt.parent_uid = 8;
    storage_page(&mut m, vec![rec(7, p[0]), rec(8, p[1]), opt]);
    let cells = grid_cells(&m.cat, pages::SHOP, STORAGE_WINDOW, STORAGE_GRID);
    tap(&mut m, mid(cells[1]));
    let discard = button_mid(&m, CMD_THROW_OUT);
    tap(&mut m, discard);
    let q = m.popup().cloned().expect("refusal box");
    assert_eq!((q.text, q.title), (m.cat.text(0x40c), 0x346));
}

/// The Modification screen with an M4A1 (equipped) and a loose S3 Suppressor, the S3 picked in the muzzle storage.
fn tuning_with_loose_s3(m: &mut Menu) -> u32 {
    use dc_client::menu::tuning::{TuningScreen, CMD_PICK_GUN, SELECT_FRONT, WEAPON_WINDOW};
    let wares = Wares::cached(m.cat.rom.as_deref());
    let find = |r: std::ops::Range<u32>, name: &str| {
        r.clone()
            .find(|n| wares.label_of(*n as i32) == name)
            .unwrap_or_else(|| panic!("no {name}"))
    };
    let m4 = find(0..1000, "M4A1");
    let s3 = find(1000..2000, "S3 Suppressor");
    m.data.store.items = vec![rec(10, m4), rec(20, s3)];
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[0] = m4 as i32;
    m.data.store.view = Some(wares.catalogue_ids());
    m.data.store.lists_asked = true;
    m.data.profile.gp = 5_000_000;
    m.open(Go::Tuning);
    // The 2009 header (CLI-765): the Weapon Select button is shown only while the pointer is over the collapsed
    // header's front image (cshell+0x154810), so hover it first as a player does.
    let f = m
        .sheet()
        .part(WEAPON_WINDOW, SELECT_FRONT, WidgetKind::Image)
        .expect("header front")
        .pos;
    m.input(MenuInput::Move([f[0] as f32 + 4.0, f[1] as f32 + 4.0]));
    // the header slides out over 45 ms of the menu clock before the button shows (cshell+0x1535d0)
    m.items_at(50.0);
    m.items_at(51.0);
    let b = button_mid(m, CMD_PICK_GUN);
    tap(m, b);
    tap(m, TuningScreen::weapon_point(0));
    tap(m, TuningScreen::socket_point(0));
    tap(m, [118.0, 485.0]);
    s3
}

/// CLI-770: the option storage's Discard (148) asks in box 0x372 (Yes / No) and sends 0x334; an attached option is
/// refused with 0x1642.
#[test]
fn rom_modification_storage_discard_asks_and_sends_0x334() {
    use dc_client::menu::tuning::CMD_THROW_OUT as TUNING_THROW_OUT;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let s3 = tuning_with_loose_s3(&mut m);
    let discard = button_mid(&m, TUNING_THROW_OUT);
    tap(&mut m, discard);
    let q = m.popup().cloned().expect("confirmation box");
    assert_eq!(
        (q.text.as_str(), q.title),
        (m.cat.text(0x372).as_str(), 0x371)
    );
    assert_eq!(q.captions, Some([0x35, 0x36]));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), format!("Discard(20, {s3})"));
    m.feed_event(LinkEvent::Discarded { ok: true });
    m.update();
    assert!(!m.data.store.items.iter().any(|r| r.uid == 20));
}

/// CLI-771: Extend Use (1019) on the picked option opens the 2009 purchase window over its products; OK sends 0x328
/// with the option's unique id as the first extension id.
#[test]
fn rom_modification_extend_use_opens_the_purchase_window_and_extends_the_option() {
    use dc_client::menu::buybox::{self, BUTTON_SIZE, OK_AT, WINDOW_AT};
    use dc_client::menu::tuning::CMD_MORE_TIME;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let s3 = tuning_with_loose_s3(&mut m);
    let wares = Wares::cached(m.cat.rom.as_deref());
    let offers = buybox::offers(&wares, &m.data.store, s3 as i32, 0);
    assert!(!offers.is_empty(), "the S3 has GP products");
    let extend = button_mid(&m, CMD_MORE_TIME);
    let sent = m.link_sent();
    tap(&mut m, extend);
    assert_eq!(m.popup_count(), 0, "a window, not a box");
    assert_eq!(m.link_sent(), sent);
    let ok = [
        (WINDOW_AT[0] + OK_AT[0] + BUTTON_SIZE[0] / 2) as f32,
        (WINDOW_AT[1] + OK_AT[1] + BUTTON_SIZE[1] / 2) as f32,
    ];
    tap(&mut m, ok);
    assert_eq!(
        m.link_latest_request(),
        format!("BuyWith({}, 0, [20, 0, 0, 0, 0])", offers[0].product)
    );
}

/// CLI-1172 (playtest report: "purchasing mods did not give option for various expiration times"): Buy (1007) on a
/// modification shop cell opens the purchase window with the cell's products (one row per period), as the shop does
/// (cshell+0x15a110 -> cshell+0x1dae90), not a yes / no box; OK sends 0x328 for the chosen row with no extension ids
/// (cshell+0x154a60).
#[test]
fn rom_modification_buy_opens_the_purchase_window_with_its_periods() {
    use dc_client::menu::buybox::{self, BUTTON_SIZE, OK_AT, WINDOW_AT};
    use dc_client::menu::tuning::{self, CMD_BUY};
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    m.data.store.items = Vec::new();
    m.data.store.equipped = [-1; 15];
    m.data.store.view = Some(wares.catalogue_ids());
    m.data.store.lists_asked = true;
    m.data.profile.gp = 5_000_000;
    m.open(Go::Tuning);
    let cells = grid_cells(
        &m.cat,
        pages::TUNING,
        tuning::SHOP_WINDOW,
        tuning::SHOP_GRID,
    );
    tap(&mut m, mid(cells[0]));
    let w = tuning::tab_wares(&wares, &m.data.store, 0)
        .first()
        .cloned()
        .expect("a modification on the first tab");
    let offers = buybox::offers(&wares, &m.data.store, w.item_no, w.sale_type);
    assert!(
        offers.len() > 1,
        "the mod sells for several periods: {offers:?}"
    );
    let buy = button_mid(&m, CMD_BUY);
    let sent = m.link_sent();
    tap(&mut m, buy);
    assert_eq!(m.popup_count(), 0, "a window, not a box");
    assert_eq!(m.link_sent(), sent);
    // the window shows the first product's period in its field
    let first = buybox::period_text(&offers[0], wares.kind_of(w.item_no));
    assert!(
        m.items().iter().any(|i| matches!(i,
            dc_client::hud::items::DrawItem::Text { text, .. } | dc_client::hud::items::DrawItem::Label { text, .. }
            if text.contains(&first))),
        "the period field shows {first:?}"
    );
    let ok = [
        (WINDOW_AT[0] + OK_AT[0] + BUTTON_SIZE[0] / 2) as f32,
        (WINDOW_AT[1] + OK_AT[1] + BUTTON_SIZE[1] / 2) as f32,
    ];
    tap(&mut m, ok);
    assert_eq!(
        m.link_latest_request(),
        format!("BuyWith({}, 0, [0, 0, 0, 0, 0])", offers[0].product)
    );
}

/// A backpack item with weapon slots (the item table's `MultyInvenSlot`).
fn backpack(m: &Menu) -> u32 {
    let wares = Wares::cached(m.cat.rom.as_deref());
    (7000..8000)
        .find(|n| wares.backpack_slots(*n as i32) >= 2)
        .expect("a backpack with two weapon slots")
}

fn window_shown(m: &Menu, id: i32) -> bool {
    m.sheet()
        .parts
        .iter()
        .position(|p| p.kind == WidgetKind::Window && p.layer == id)
        .is_some_and(|i| m.sheet().visible(i))
}

/// CLI-772: the storage page's Backpack (1009) opens window 6 for the selected weapon (its own slot filled, a
/// backpack with weapon slots on); the list holds the backpack's slots; OK equips into slot 10 + the picked row.
#[test]
fn rom_storage_backpack_button_asks_the_slot_and_equips_into_it() {
    use dc_client::menu::slotpick::{CMD_BACKPACK, CMD_LIST, CMD_OK, PACK_WINDOW};
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let p = primaries(&m, 2);
    let pack = backpack(&m);
    storage_page(&mut m, vec![rec(7, p[0]), rec(8, p[1]), rec(9, pack)]);
    m.data.store.equipped[9] = pack as i32;
    let cells = grid_cells(&m.cat, pages::SHOP, STORAGE_WINDOW, STORAGE_GRID);
    tap(&mut m, mid(cells[1]));
    assert!(!window_shown(&m, PACK_WINDOW));
    let b = button_mid(&m, CMD_BACKPACK);
    tap(&mut m, b);
    assert!(window_shown(&m, PACK_WINDOW), "window 6 is up");
    assert_eq!(m.popup_count(), 0);
    // the list: one row per backpack slot ("A <0x399>", "B ...")
    let arrow = button_mid(&m, CMD_LIST);
    tap(&mut m, arrow);
    let rows: Vec<String> = m.dropdown().expect("slot list").items.clone();
    assert!(rows.len() >= 2, "{rows:?}");
    assert_eq!(rows[0], format!("A {}", m.cat.text(0x399)));
    // the second row, then OK
    let d = m.dropdown().unwrap();
    let at = (0..768)
        .flat_map(|y| {
            (0..1024)
                .step_by(4)
                .map(move |x| [x as f32 + 0.5, y as f32 + 0.5])
        })
        .find(|p| d.row_at(*p) == Some(1))
        .expect("row 1 on screen");
    tap(&mut m, at);
    let ok = button_mid(&m, CMD_OK);
    tap(&mut m, ok);
    assert_eq!(m.link_latest_request(), format!("Equip(8, {}, 11)", p[1]));
    assert!(!window_shown(&m, PACK_WINDOW));
}

/// CLI-772: Backpack without a backpack does nothing; with an empty own slot it refuses with 0x39b.
#[test]
fn rom_storage_backpack_button_refuses_without_the_own_slot_filled() {
    use dc_client::menu::slotpick::{CMD_BACKPACK, PACK_WINDOW};
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let p = primaries(&m, 2);
    let pack = backpack(&m);
    storage_page(&mut m, vec![rec(7, p[0]), rec(8, p[1])]);
    let cells = grid_cells(&m.cat, pages::SHOP, STORAGE_WINDOW, STORAGE_GRID);
    tap(&mut m, mid(cells[1]));
    let b = button_mid(&m, CMD_BACKPACK);
    tap(&mut m, b);
    assert!(
        !window_shown(&m, PACK_WINDOW) && m.popup_count() == 0,
        "no backpack: nothing"
    );
    m.data.store.equipped[9] = pack as i32;
    m.data.store.equipped[0] = -1;
    tap(&mut m, b);
    let q = m.popup().cloned().expect("refusal");
    assert_eq!((q.text, q.title), (m.cat.text(0x39b), 0x39a));
}

/// The shop with an equipped primary and a backpack with weapon slots; returns the item number of the weapons
/// tab's second cell.
fn shop_with_backpack(m: &mut Menu) -> u32 {
    let wares = Wares::cached(m.cat.rom.as_deref());
    let p = primaries(m, 1);
    let pack = backpack(m);
    m.data.store.items = vec![rec(7, p[0]), rec(9, pack)];
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[0] = p[0] as i32;
    m.data.store.equipped[9] = pack as i32;
    m.data.store.view = Some(wares.catalogue_ids());
    m.data.store.lists_asked = true;
    m.data.profile.gp = 5_000_000;
    m.open(Go::Shop);
    wares.shelf(&m.data.store, TAB_WEAPONS, 0)[1].item_no as u32
}

/// Buy on the second cell (the purchase window, cancelled: headless, the link ends at its first request), the
/// purchase reply with unique id `uid`, the purchase box and the "equip now" box's Equip.
fn buy_and_equip(m: &mut Menu, uid: u32) {
    use dc_client::menu::buybox::{BUTTON_SIZE, CANCEL_AT, WINDOW_AT};
    use dc_client::menu::link::BuyReply;
    use dc_client::menu::shop::{CMD_BUY, SHOP_GRID, SHOP_WINDOW};
    let cells = grid_cells(&m.cat, pages::SHOP, SHOP_WINDOW, SHOP_GRID);
    tap(m, mid(cells[1]));
    let b = button_mid(m, CMD_BUY);
    tap(m, b);
    let cancel = [
        (WINDOW_AT[0] + CANCEL_AT[0] + BUTTON_SIZE[0] / 2) as f32,
        (WINDOW_AT[1] + CANCEL_AT[1] + BUTTON_SIZE[1] / 2) as f32,
    ];
    tap(m, cancel);
    m.feed_event(LinkEvent::Bought(BuyReply {
        ok: true,
        reason: 0,
        uid,
        gp: 4_000_000,
        award: -1,
    }));
    m.update();
    m.input(MenuInput::Key(MenuKey::Enter)); // the purchase box
    m.input(MenuInput::Key(MenuKey::Enter)); // "equip now": Equip
}

fn check_button(m: &mut Menu, id: i32) {
    use dc_client::menu::slotpick::SLOT_WINDOW;
    let c = m
        .sheet()
        .part(SLOT_WINDOW, id, WidgetKind::CheckButton)
        .map(|c| mid([c.pos[0], c.pos[1], c.size[0], c.size[1]]))
        .expect("check button");
    tap(m, c);
}

/// CLI-772: a bought weapon's "equip now" (box 0x36b, Equip) asks in window 7 when its slot is filled and the backpack
/// has weapon slots; Cancel closes it without a request; the own slot is checked first and OK equips there.
#[test]
fn rom_bought_weapon_asks_the_slot_in_window_7_and_equips_into_its_own_slot() {
    use dc_client::menu::slotpick::{CMD_CANCEL, CMD_OK, OWN_CHECK, PACK_CHECK, SLOT_WINDOW};
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let bought = shop_with_backpack(&mut m);
    buy_and_equip(&mut m, 30);
    assert!(window_shown(&m, SLOT_WINDOW), "window 7 is up");
    assert_eq!(m.popup_count(), 0);
    let sent = m.link_sent();
    let cancel = button_mid(&m, CMD_CANCEL);
    tap(&mut m, cancel);
    assert_eq!(m.link_sent(), sent);
    assert!(!window_shown(&m, SLOT_WINDOW));
    buy_and_equip(&mut m, 31);
    assert!(window_shown(&m, SLOT_WINDOW));
    check_button(&mut m, PACK_CHECK);
    check_button(&mut m, OWN_CHECK);
    let ok = button_mid(&m, CMD_OK);
    tap(&mut m, ok);
    assert_eq!(m.link_latest_request(), format!("Equip(31, {bought}, 0)"));
    assert!(!window_shown(&m, SLOT_WINDOW));
}

/// CLI-772: window 7's Backpack check button switches to the backpack's slots; OK equips into slot 10.
#[test]
fn rom_bought_weapon_goes_into_the_backpack_from_window_7() {
    use dc_client::menu::slotpick::{CMD_OK, PACK_CHECK, SLOT_WINDOW};
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let bought = shop_with_backpack(&mut m);
    buy_and_equip(&mut m, 30);
    assert!(window_shown(&m, SLOT_WINDOW));
    check_button(&mut m, PACK_CHECK);
    let ok = button_mid(&m, CMD_OK);
    tap(&mut m, ok);
    assert_eq!(m.link_latest_request(), format!("Equip(30, {bought}, 10)"));
}

/// The black market's first credit cell: (category command, cell index on page 0, item).
fn market_cell(m: &mut Menu) -> (i32, usize, i32) {
    use dc_client::menu::shop::CATEGORY_COMMANDS;
    use dc_client::menu::storage::TAB_MARKET;
    let wares = Wares::cached(m.cat.rom.as_deref());
    m.data.store.view = Some(wares.catalogue_ids());
    m.data.store.lists_asked = true;
    for (c, cmd) in CATEGORY_COMMANDS[TAB_MARKET].iter().enumerate().skip(2) {
        let shelf = wares.shelf(&m.data.store, TAB_MARKET, c);
        if let Some(w) = shelf.first().filter(|w| w.sale_type == 1) {
            return (*cmd, 0, w.item_no);
        }
    }
    panic!("no credit cell");
}

/// CLI-773: NX Charge (1034) and the short-of-NX box's Charge NX show the 2009 charge page in a box (no browser).
#[test]
fn rom_charge_nx_shows_the_nx_earned_box() {
    use dc_client::menu::buybox::{BUTTON_SIZE, OK_AT, WINDOW_AT};
    use dc_client::menu::shop::{CMD_BUY, CMD_CHARGE, SHOP_GRID, SHOP_WINDOW};
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let (cat_cmd, cell, _) = market_cell(&mut m);
    m.data.balance = (0, 0);
    m.open(Go::Market);
    let b = button_mid(&m, CMD_CHARGE);
    tap(&mut m, b);
    let q = m.popup().cloned().expect("charge box");
    assert_eq!(q.text, "NX is earned from daily logins and clan wins");
    assert_eq!(q.kind, dc_client::menu::popup::BoxKind::Ok);
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.popup_count(), 0);
    // no NX: the short-of-NX box with Charge NX / Cancel
    m.input(MenuInput::Command(cat_cmd));
    let cells = grid_cells(&m.cat, pages::SHOP, SHOP_WINDOW, SHOP_GRID);
    tap(&mut m, mid(cells[cell]));
    let b = button_mid(&m, CMD_BUY);
    tap(&mut m, b);
    let ok = [
        (WINDOW_AT[0] + OK_AT[0] + BUTTON_SIZE[0] / 2) as f32,
        (WINDOW_AT[1] + OK_AT[1] + BUTTON_SIZE[1] / 2) as f32,
    ];
    let sent = m.link_sent();
    tap(&mut m, ok);
    let q = m.popup().cloned().expect("the 0x17db confirmation");
    assert_eq!(
        q.kind,
        dc_client::menu::popup::BoxKind::Ok,
        "0x17db has OK only"
    );
    // the item name carries a colour code (cshell+0x2ebc10): the box draws it as its own run, no code as text
    let texts: Vec<([u8; 4], String)> = m
        .items()
        .iter()
        .filter_map(|i| match i {
            dc_client::hud::items::DrawItem::Text { style, text, .. }
            | dc_client::hud::items::DrawItem::Label { style, text, .. } => {
                Some((style.colour, text.clone()))
            }
            _ => None,
        })
        .collect();
    assert!(texts.iter().all(|(_, t)| !t.contains("^0x")), "{texts:?}");
    assert!(
        texts.iter().any(|(c, _)| *c == [255, 165, 0, 255]),
        "an orange run: {texts:?}"
    );
    m.input(MenuInput::Key(MenuKey::Enter));
    let q = m.popup().cloned().expect("the 0x17dd question");
    assert_eq!(
        (q.text.clone(), q.kind),
        (m.cat.text(0x17dd), dc_client::menu::popup::BoxKind::YesNo)
    );
    m.input(MenuInput::Key(MenuKey::Enter)); // Yes: the purchase check
    let q = m.popup().cloned().expect("short-of-NX box");
    assert_eq!((q.text.clone(), q.title), (m.cat.text(0x1873), 0x17e5));
    assert_eq!(q.captions, Some([0x17ff, 0x43]));
    m.input(MenuInput::Key(MenuKey::Enter)); // Charge NX
    let q = m.popup().cloned().expect("charge box");
    assert_eq!(q.text, "NX is earned from daily logins and clan wins");
    assert_eq!(q.kind, dc_client::menu::popup::BoxKind::Ok);
    assert_eq!(m.link_sent(), sent, "nothing sent");
}

/// LG-52 (playtest report: "none of body storage appears"): the storage page draws each stored item in a framed cell with
/// its slot image, not a bare text. CLI-1264: the frames are the storage page's own tab-less Inven_Item_Slot art at the
/// storage cell's 0xfe x 0x56 (the shop page's frames are not drawn), _On on the selected cell and on the cell
/// under the pointer, filled or empty, _Off on the others.
#[test]
fn rom_storage_cells_are_drawn_as_framed_cells_with_the_item_image() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::shop::{SHOP_FRAMES, STORAGE_CELL_SIZE, STORAGE_FRAMES};
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let m4 = (0..1000u32)
        .find(|n| wares.label_of(*n as i32) == "M4A1")
        .expect("M4A1");
    storage_page(&mut m, vec![rec(10, m4)]);
    let cells = grid_cells(&m.cat, pages::SHOP, STORAGE_WINDOW, STORAGE_GRID);
    // the frame drawn at each cell's top left
    let frame_at = |items: &[DrawItem], c: [i32; 4]| -> Option<String> {
        items.iter().find_map(|i| match i {
            DrawItem::Image { tex, dst, src, .. }
                if STORAGE_FRAMES.contains(&tex.as_str())
                    && dst[0] == c[0] as f32
                    && dst[1] == c[1] as f32 =>
            {
                assert_eq!(
                    [dst[2], dst[3], src[0], src[1]],
                    [
                        STORAGE_CELL_SIZE[0] as f32,
                        STORAGE_CELL_SIZE[1] as f32,
                        254.0,
                        86.0
                    ],
                    "{tex}"
                );
                Some(tex.clone())
            }
            _ => None,
        })
    };
    let items = m.items_at(500.0);
    assert!(
        !items.iter().any(
            |i| matches!(i, DrawItem::Image { tex, .. } if SHOP_FRAMES.contains(&tex.as_str()))
        ),
        "no shop frames on the storage page"
    );
    for (i, c) in cells.iter().enumerate() {
        assert!(frame_at(&items, *c).is_some(), "cell {i} framed");
    }
    // select the filled cell, then point at an empty one: both lit, the others not
    tap(&mut m, mid(cells[0]));
    m.input(MenuInput::Move(mid(cells[3])));
    let items = m.items_at(500.0);
    for (i, c) in cells.iter().enumerate() {
        let want = STORAGE_FRAMES[(i == 0 || i == 3) as usize];
        assert_eq!(frame_at(&items, *c).as_deref(), Some(want), "cell {i}");
    }
    // the pointer off the cells: only the selected one stays lit
    m.input(MenuInput::Move([2.0, 2.0]));
    let items = m.items_at(500.0);
    assert_eq!(
        frame_at(&items, cells[3]).as_deref(),
        Some(STORAGE_FRAMES[0])
    );
    assert_eq!(
        frame_at(&items, cells[0]).as_deref(),
        Some(STORAGE_FRAMES[1])
    );
    let icon = wares.art_of(m4 as i32).map(|a| a.icon.clone()).unwrap();
    assert!(
        items
            .iter()
            .any(|i| matches!(i, DrawItem::Image { tex, .. } if *tex == icon)),
        "the item's slot image {icon}"
    );
    // CLI-1266: the Equipped badge of the equipped item at the cell's (0xc7, 2) (virtual units, as the reference stack)
    let badge = dc_client::menu::tuning::EQUIPPED_MARK.0;
    assert!(
        items.iter().any(|i| matches!(i, DrawItem::Image { tex, dst, .. }
            if tex == badge && [dst[0], dst[1]] == [(cells[0][0] + 0xc7) as f32, (cells[0][1] + 2) as f32])),
        "Equipped badge at (0xc7, 2)"
    );
}

/// LG-51: the default uniform and vest rows (no icon, a Korean-code-page name in the item table) get a readable label.
#[test]
fn rom_default_body_items_have_labels() {
    let Some(rom) = dc_rom::resolve_rom().ok() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(Some(&rom));
    assert_eq!(wares.label_of(2000), "Default Uniform");
    assert!(
        !wares.label_of(3000).starts_with('#'),
        "{}",
        wares.label_of(3000)
    );
}

/// CLI-891: a weapon equipped into a backpack slot (equipment slot 10 + row, 0x323) shows its slot image on the left
/// pane's backpack row (playtest report: "can't assign items to backpack": the server took the equip and the
/// row stayed empty).
#[test]
fn rom_backpack_row_shows_the_weapon_in_the_backpack_slot() {
    use dc_client::hud::items::DrawItem;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let p = primaries(&m, 2);
    let pack = backpack(&m);
    storage_page(&mut m, vec![rec(7, p[0]), rec(8, p[1]), rec(9, pack)]);
    m.data.store.equipped[9] = pack as i32;
    m.data.store.equipped[10] = p[1] as i32;
    let wares = Wares::cached(m.cat.rom.as_deref());
    let icon = wares.art_of(p[1] as i32).unwrap().icon.clone();
    let on_left = |m: &mut Menu| {
        m.items()
            .iter()
            .filter(
                |i| matches!(i, DrawItem::Image { tex, dst, .. } if *tex == icon && dst[0] < 300.0),
            )
            .count()
    };
    assert_eq!(on_left(&mut m), 1, "backpack row A shows the weapon");
    m.data.store.equipped[10] = -1;
    assert_eq!(on_left(&mut m), 0);
}

/// CLI-1094 (playtest report): the 2009 box builder never adds its icon controls to the box window, so a Yes / No box
/// draws no warning triangle (the reference client shows none); the body text, title and buttons stay where they were.
#[test]
fn rom_a_yes_no_box_draws_no_warning_icon() {
    use dc_client::hud::items::DrawItem;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let p = primaries(&m, 2);
    storage_page(&mut m, vec![rec(7, p[0]), rec(8, p[1])]);
    let cells = grid_cells(&m.cat, pages::SHOP, STORAGE_WINDOW, STORAGE_GRID);
    tap(&mut m, mid(cells[1]));
    let discard = button_mid(&m, CMD_THROW_OUT);
    tap(&mut m, discard);
    assert!(m.popup().is_some(), "the confirmation box is up");
    let icon = m
        .cat
        .paths
        .warning
        .clone()
        .expect("the ROM names the warning icon");
    assert!(m.cat.tex_size(&icon).is_some(), "the ROM holds {icon}");
    let items = m.items();
    assert!(
        !items
            .iter()
            .any(|i| matches!(i, DrawItem::Image { tex, .. } if tex.eq_ignore_ascii_case(&icon))),
        "no warning icon"
    );
    assert!(items
        .iter()
        .any(|i| matches!(i, DrawItem::Label { text, .. } if *text == m.cat.text(0x35))));
}

/// CLI-1160: a filled backpack row shows the close-window X at its right end; a press on it unequips the weapon of
/// backpack slot 10 + row (0x326 with the slot); an empty row has no X.
// EVIDENCE: cshell+0xd9ad0, cshell+0xda230, cshell+0x147100, cshell+0x14d6b0, cshell+0x1467b0, cshell+0x145ed0
#[test]
fn rom_a_filled_backpack_row_has_an_x_that_unequips_the_weapon() {
    use dc_client::hud::items::DrawItem;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let p = primaries(&m, 2);
    let pack = backpack(&m);
    storage_page(&mut m, vec![rec(7, p[0]), rec(8, p[1]), rec(9, pack)]);
    m.data.store.equipped[9] = pack as i32;
    let x_tex = format!("{}A.dtx", dc_client::menu::pane::UNEQUIP_X);
    let has_x = |m: &mut Menu| {
        m.items().iter().any(|i| {
            matches!(i, DrawItem::Image { tex, dst, .. }
            if tex.eq_ignore_ascii_case(&x_tex) && dst[0] < 460.0)
        })
    };
    assert!(!has_x(&mut m), "an empty backpack row has no X");
    m.data.store.equipped[10] = p[1] as i32;
    assert!(has_x(&mut m), "a filled row shows the X");
    let at = m
        .sheet()
        .part(
            2,
            dc_client::menu::pane::BACKPACK_LIST,
            dc_ui_data::WidgetKind::List,
        )
        .expect("backpack list")
        .pos;
    let row = dc_client::menu::pane::pack_row_rect(at, 0);
    let x = dc_client::menu::pane::unequip_x_rect(row);
    tap(&mut m, mid(x));
    assert_eq!(m.link_latest_request(), format!("Unequip(8, {}, 10)", p[1]));
}

/// CLI-1261 (playtest report: an NX supply crate did not show in the inbox): a credit purchase's success marks the held
/// inbox rows stale, so the inbox tab asks the server again instead of showing the rows it fetched before.
#[test]
fn rom_a_credit_purchase_marks_the_inbox_stale() {
    use dc_client::menu::buybox::{BUTTON_SIZE, CANCEL_AT, WINDOW_AT};
    use dc_client::menu::link::BuyReply;
    use dc_client::menu::shop::{CMD_BUY, SHOP_GRID, SHOP_WINDOW};
    use dc_client::menu::storage::TAB_MARKET;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    m.data.store.view = Some(wares.catalogue_ids());
    let credit: Vec<u32> = (2..6)
        .flat_map(|c| wares.shelf(&m.data.store, TAB_MARKET, c))
        .map(|w| w.product)
        .take(2)
        .collect();
    assert_eq!(credit.len(), 2, "credit products on the black market");
    // the black market opens on its Hot list (highlight list 0)
    m.data.store.highlight[0] = credit.iter().copied().collect();
    m.data.store.view = Some(credit);
    m.data.store.lists_asked = true;
    m.data.store.inbox_fetched = true;
    m.data.profile.gp = 5_000_000;
    m.open(Go::Market);
    let cells = grid_cells(&m.cat, pages::SHOP, SHOP_WINDOW, SHOP_GRID);
    tap(&mut m, mid(cells[1]));
    let b = button_mid(&m, CMD_BUY);
    tap(&mut m, b);
    let cancel = [
        (WINDOW_AT[0] + CANCEL_AT[0] + BUTTON_SIZE[0] / 2) as f32,
        (WINDOW_AT[1] + CANCEL_AT[1] + BUTTON_SIZE[1] / 2) as f32,
    ];
    tap(&mut m, cancel);
    assert!(!m.data.store.inbox_stale);
    m.feed_event(LinkEvent::Bought(BuyReply {
        ok: true,
        reason: 0,
        uid: 0,
        gp: 5_000_000,
        award: -1,
    }));
    m.update();
    assert!(
        m.data.store.inbox_stale,
        "the inbox asks again after a credit purchase"
    );
    // audit-function (CLI-1388): the "placed in your inbox" box is titled 0x17d1 (cshell+0x150c40)
    let q = m.popup().expect("the inbox box");
    assert_eq!(q.title, dc_client::menu::shop::CREDIT_BOUGHT_TITLE);
}

/// audit-function (CLI-1386): a GP supply case bought in the shop opens at once: the reply's +0x1e names the award
/// and the client shows the award pop-up (titled 0x393), not the "bought" box 0x439 (cshell+0x150c40).
#[test]
fn rom_a_gp_case_purchase_shows_the_award_not_the_bought_box() {
    use dc_client::menu::link::BuyReply;
    use dc_client::menu::shop::{
        CASE_AWARD_TITLE, CATEGORY_COMMANDS, CMD_BUY, SHOP_GRID, SHOP_WINDOW,
    };
    use dc_client::menu::storage::TAB_MARKET;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    m.data.store.view = Some(wares.catalogue_ids());
    let mut found = None;
    'tabs: for tab in [TAB_WEAPONS, TAB_MARKET] {
        for (c, cmd) in CATEGORY_COMMANDS[tab].iter().enumerate() {
            if let Some(w) = wares
                .shelf(&m.data.store, tab, c)
                .into_iter()
                .find(|w| w.kind_of == Some(0x14) && w.sale_type == 0)
            {
                found = Some((tab, *cmd, w));
                break 'tabs;
            }
        }
    }
    let (tab, cmd, w) = found.expect("the catalogue lists a GP supply case");
    m.data.store.view = Some(vec![w.product]);
    m.data.store.lists_asked = true;
    m.data.store.inbox_fetched = true;
    m.data.profile.gp = 5_000_000;
    // past any rank requirement of the case (CLI-1630)
    m.data.profile.exp = 100_000_000;
    m.open(if tab == TAB_MARKET { Go::Market } else { Go::Shop });
    let b = button_mid(&m, cmd);
    tap(&mut m, b);
    let cells = grid_cells(&m.cat, pages::SHOP, SHOP_WINDOW, SHOP_GRID);
    tap(&mut m, mid(cells[0]));
    let b = button_mid(&m, CMD_BUY);
    tap(&mut m, b);
    assert!(m.popup().is_some(), "a GP case asks first");
    m.input(MenuInput::Key(MenuKey::Enter));
    m.update();
    let award = 6;
    m.feed_event(LinkEvent::Bought(BuyReply {
        ok: true,
        reason: 0,
        uid: 77,
        gp: 4_000_000,
        award,
    }));
    m.update();
    let q = m.popup().expect("the award pop-up");
    assert_eq!(q.title, CASE_AWARD_TITLE);
    assert_eq!(q.text, wares.label_of(award));
    assert_eq!(m.popup_count(), 1, "no 0x439 box behind it");
}

/// audit-function (CLI-1389): the expiry push 0x33f shows one box per expired item with its name (res 0x1786,
/// title 0x371) and the record leaves the storage (cshell+0x22cde0).
#[test]
fn rom_an_expired_item_shows_its_box_and_leaves_the_storage() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    m.data.store.items = vec![rec(41, 9019), rec(42, 9020)];
    m.feed_event(LinkEvent::Expired {
        uids: vec![41],
        refresh: false,
    });
    m.update();
    assert_eq!(
        m.data.store.items.iter().map(|i| i.uid).collect::<Vec<_>>(),
        vec![42]
    );
    let q = m.popup().expect("the expiry box");
    assert_eq!(q.title, 0x371);
    let name = wares.label_of(9019);
    assert!(!name.is_empty() && q.text.contains(&name), "{:?}", q.text);
    assert_eq!(m.popup_count(), 1);
}

/// CLI-1630 (user 2026-11-01: guns restricted to a rank could be bought below it): Buy on a GP weapon cell whose
/// requirement is above the character's experience opens the purchase window; its OK shows the rank requirement
/// page's GP form and sends nothing. A purchase the lobby refuses for the rank (0x60, LG-302) shows the rank box.
#[test]
fn rom_a_gp_weapon_above_the_rank_shows_the_rank_page_and_sends_nothing() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::buybox::{
        BUTTON_SIZE, CANCEL_AT, OK_AT, RANK_OK_AT, RANK_TITLE, WINDOW_AT,
    };
    use dc_client::menu::link::BuyReply;
    use dc_client::menu::shop::{CATEGORY_COMMANDS, CELLS, CMD_BUY, SHOP_GRID, SHOP_WINDOW};
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    m.data.store.view = Some(wares.catalogue_ids());
    m.data.store.lists_asked = true;
    m.data.profile.gp = 5_000_000;
    m.data.profile.exp = 0;
    let found = CATEGORY_COMMANDS[TAB_WEAPONS]
        .iter()
        .enumerate()
        .find_map(|(c, cmd)| {
            let shelf = wares.shelf(&m.data.store, TAB_WEAPONS, c);
            shelf
                .iter()
                .take(CELLS)
                .position(|w| w.sale_type == 0 && w.unlock_exp > 0 && w.kind_of != Some(0x14))
                .map(|k| (*cmd, k))
        });
    let Some((cat_cmd, cell)) = found else {
        eprintln!("skipped: no GP weapon cell with a rank requirement");
        return;
    };
    m.open(Go::Shop);
    m.input(MenuInput::Command(cat_cmd));
    let cells = grid_cells(&m.cat, pages::SHOP, SHOP_WINDOW, SHOP_GRID);
    let at = |q: [i32; 2]| {
        [
            (WINDOW_AT[0] + q[0] + BUTTON_SIZE[0] / 2) as f32,
            (WINDOW_AT[1] + q[1] + BUTTON_SIZE[1] / 2) as f32,
        ]
    };
    let texts = |m: &mut Menu| -> String {
        m.items()
            .iter()
            .filter_map(|i| match i {
                DrawItem::Text { text, .. } | DrawItem::Label { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    tap(&mut m, mid(cells[cell]));
    let b = button_mid(&m, CMD_BUY);
    tap(&mut m, b);
    let sent = m.link_sent();
    tap(&mut m, at(OK_AT));
    let shown = texts(&mut m);
    assert!(shown.contains(&m.cat.text(RANK_TITLE)), "{shown}");
    assert!(
        shown.contains("or higher in order to purchase this item."),
        "{shown}"
    );
    assert_eq!(m.link_sent(), sent, "nothing sent");
    assert_eq!(m.popup_count(), 0);
    // the page's OK closes it, the window stays and nothing is sent
    tap(&mut m, at(RANK_OK_AT));
    assert!(!texts(&mut m).contains("or higher in order to purchase"));
    assert_eq!(m.link_sent(), sent, "nothing sent");
    // the lobby's rank refusal of that ware shows the rank box
    tap(&mut m, at(CANCEL_AT));
    m.feed_event(LinkEvent::Bought(BuyReply {
        ok: false,
        reason: 0x60,
        uid: 0,
        gp: 5_000_000,
        award: -1,
    }));
    m.update();
    let q = m.popup().cloned().expect("rank box");
    assert_eq!(q.title, RANK_TITLE);
    assert!(
        q.text.contains("or higher in order to purchase this item."),
        "{}",
        q.text
    );
}
