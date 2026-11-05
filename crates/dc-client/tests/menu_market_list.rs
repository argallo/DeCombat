//! bm-inuse-list (OWNER REPORT 2026-11-05): the black market tab's list of in-use function items (list 5 of the
//! shop screen's left pane). A press on a row opens the storage page of the function items with that item's cell
//! selected (CLI-1886); the list's scroll bar is the list record's, hidden below 7 rows and inside the record above
//! (CLI-741).
//!
//! ROM-gated (skipped without `CA_ROM`). Headless, detached link.
// EVIDENCE: cshell+0x14d5b0 (the press over list 5: storage page, cshell+0x14c7e0 with the row's unique id),
// cshell+0x14c7e0, cshell+0x1496e0 (the bar hidden below 7 records), cshell+0x2f00f0 (the bar at the record's offset)

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::{InvRecord, LinkOptions};
use dc_client::menu::pane;
use dc_client::menu::sheet::Look;
use dc_client::menu::shop::{grid_cells, STORAGE_FRAMES, STORAGE_GRID, STORAGE_WINDOW};
use dc_client::menu::storage::{stored, Wares, TAB_MARKET};
use dc_client::menu::{pages, Go, Menu, MenuInput, MenuOptions};
use dc_client::steps::{resolve, Resolved};
use dc_ui_data::WidgetKind;

/// The function items' shelf of the storage page (Black Market category 2).
const FUNCTION_SHELF: usize = 2;
const SCROLL_TEXTURES: [&str; 4] = [
    "scroll_back_a.dtx",
    "scroll_bar_a.dtx",
    "scroll_up_a.dtx",
    "scroll_down_a.dtx",
];

fn menu() -> Option<Menu> {
    let rom = dc_rom::resolve_rom().ok()?;
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("t"),
        rom: Some(rom),
        font: None,
        width: 1024,
        height: 768,
    });
    m.detach_link();
    Some(m)
}

fn tap(m: &mut Menu, p: [f32; 2]) {
    m.input(MenuInput::Move(p));
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
}

fn click(m: &mut Menu, name: &str) {
    let Resolved::Part { at, .. } = resolve(m, name) else {
        panic!("{name}: {:?}", resolve(m, name));
    };
    tap(m, at);
}

/// Function items that stay in the function list while in use (no `IsCanUnEquip`, so no slot is needed).
fn active_items(wares: &Wares, n: usize) -> Vec<u32> {
    let v: Vec<u32> = (9000..10000u32)
        .filter(|n| wares.kind_of(*n as i32) == Some(0x15) && wares.art_of(*n as i32).is_some())
        .filter(|n| !wares.can_unequip(*n as i32))
        .take(n)
        .collect();
    assert_eq!(v.len(), n, "the ROM has fewer than {n} such function items");
    v
}

/// The black market tab with in-use records of `items` (unique ids 100, 101, ...) plus `extra` records.
fn market(m: &mut Menu, items: &[u32], extra: Vec<InvRecord>) {
    m.data.store.items = items
        .iter()
        .enumerate()
        .map(|(k, n)| InvRecord {
            uid: 100 + k as u32,
            item_no: *n,
            in_use: true,
            ..Default::default()
        })
        .chain(extra)
        .collect();
    m.data.store.equipped = [-1; 15];
    m.open(Go::Lobby);
    click(m, "lobby.shop");
    click(m, "shop.market");
}

fn list_index(m: &Menu) -> usize {
    m.sheet()
        .find(2, pane::MARKET_LIST, WidgetKind::List)
        .expect("market list record")
}

/// Row `i`'s rectangle on the list, moved up by its bar's value.
fn row_rect(m: &Menu, i: usize) -> [i32; 4] {
    let at = list_index(m);
    let l = &m.sheet().parts[at];
    let scrolled = m.sheet().list_bar(at).map_or(0, |b| b.value);
    let step = pane::ROW_SIZE[1] + 3;
    [
        l.pos[0] + pane::MARKET_ROW_DX,
        l.pos[1] + i as i32 * step - scrolled,
        pane::ROW_SIZE[0],
        pane::ROW_SIZE[1],
    ]
}

/// A headless picture of the menu into `DC_MARKET_PNG` when that is set (a look, not a check).
fn shot(m: &mut Menu, name: &str) {
    let Ok(dir) = std::env::var("DC_MARKET_PNG") else {
        return;
    };
    let i = m.image();
    let f = dc_render::FrameImage {
        width: i.width,
        height: i.height,
        rgba: i.rgba,
    };
    let _ = f.save_png(&std::path::Path::new(&dir).join(format!("{name}.png")));
}

fn mid(r: [i32; 4]) -> [f32; 2] {
    [(r[0] + r[2] / 2) as f32, (r[1] + r[3] / 2) as f32]
}

fn storage_up(m: &Menu) -> bool {
    m.sheet()
        .parts
        .iter()
        .position(|p| p.kind == WidgetKind::Window && p.layer == STORAGE_WINDOW)
        .is_some_and(|i| m.sheet().visible(i))
}

fn inside(r: [i32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] as f32
        && p[1] >= r[1] as f32
        && p[0] < (r[0] + r[2]) as f32
        && p[1] < (r[1] + r[3]) as f32
}

/// CLI-1886: a tap on each shown row of the in-use list opens the storage page of the function items with the
/// page holding that record and its cell selected (lit, the item's name in it); 8 records, so the shelf spans two
/// pages and the list shows its bar.
#[test]
fn rom_a_tap_on_an_in_use_row_opens_the_function_storage_on_that_item() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let items = active_items(&wares, 8);
    market(&mut m, &items, Vec::new());
    let order: Vec<u32> = pane::market_records(&wares, &m.data.store)
        .iter()
        .map(|r| r.uid)
        .collect();
    let shelf: Vec<u32> = stored(&m.data.store, &wares, TAB_MARKET, FUNCTION_SHELF)
        .iter()
        .map(|r| r.uid)
        .collect();
    assert_eq!(shelf.len(), 8, "the function shelf holds every record");
    let cells = grid_cells(&m.cat, pages::SHOP, STORAGE_WINDOW, STORAGE_GRID);
    assert_eq!(cells.len(), 6);
    let mut second_page = false;
    let list = m.sheet().parts[list_index(&m)].clone();
    let Look::List { height, .. } = list.look else {
        panic!("not a list");
    };
    for (i, uid) in order.iter().enumerate() {
        assert!(!storage_up(&m), "row {i}: the shop page before the tap");
        // the first six rows at the top of the list, the last two after the wheel brought them in
        let over = [(list.pos[0] + 100) as f32, (list.pos[1] + 100) as f32];
        m.input(MenuInput::Wheel(over, if i < 6 { 20 } else { -20 }));
        let r = row_rect(&m, i);
        assert!(
            r[1] >= list.pos[1] && r[1] + r[3] <= list.pos[1] + height,
            "row {i} shown"
        );
        let at = mid(r);
        tap(&mut m, at);
        assert!(storage_up(&m), "row {i}: the storage page is up");
        let k = shelf.iter().position(|u| u == uid).unwrap();
        second_page |= k >= 6;
        let rec = m.data.store.items.iter().find(|r| r.uid == *uid).unwrap();
        let label = wares.label_of(rec.item_no as i32);
        m.input(MenuInput::Move([2.0, 2.0]));
        let drawn = m.items_at(500.0);
        let cell = cells[k % 6];
        assert!(
            drawn.iter().any(|d| matches!(d,
                DrawItem::Text { text, pos, .. } | DrawItem::Label { text, pos, .. }
                    if *text == label && inside(cell, *pos))),
            "row {i}: {label} not in cell {} of its page",
            k % 6
        );
        let lit = drawn.iter().any(|d| {
            matches!(d,
            DrawItem::Image { tex, dst, .. }
                if tex == STORAGE_FRAMES[1] && dst[0] == cell[0] as f32 && dst[1] == cell[1] as f32)
        });
        assert!(lit, "row {i}: cell {} selected", k % 6);
        // back to the shop page of the black market tab for the next row
        click(&mut m, "shop.market");
    }
    assert!(
        second_page,
        "one of the rows was on the shelf's second page"
    );
}

/// CLI-1886: with no function item in use the list's one row (string 0x17cc) opens the function items' storage
/// page; the other shelves' items are not on it. A press under the row is not a row.
#[test]
fn rom_the_empty_in_use_row_opens_the_function_storage() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let idle = active_items(&wares, 1)[0];
    let weapon = (0..1000u32)
        .find(|n| wares.label_of(*n as i32) == "M4A1")
        .expect("M4A1");
    let extra = vec![
        InvRecord {
            uid: 7,
            item_no: weapon,
            ..Default::default()
        },
        InvRecord {
            uid: 8,
            item_no: idle,
            ..Default::default()
        },
    ];
    market(&mut m, &[], extra.clone());
    assert!(pane::market_records(&wares, &m.data.store).is_empty());
    let r = row_rect(&m, 0);
    tap(&mut m, [r[0] as f32 + 40.0, (r[1] + 3 * r[3]) as f32]);
    assert!(!storage_up(&m), "a press under the row is not a row");
    tap(&mut m, mid(r));
    assert!(storage_up(&m), "the storage page is up");
    let drawn = m.items_at(500.0);
    let right = |t: &str| {
        drawn.iter().any(|d| {
            matches!(d,
            DrawItem::Text { text, pos, .. } | DrawItem::Label { text, pos, .. }
                if text == t && pos[0] > 470.0)
        })
    };
    assert!(
        right(&wares.label_of(idle as i32)),
        "the idle function item is on the page"
    );
    assert!(!right("M4A1"), "a weapon is not on the function shelf");
}

/// CLI-741 (OWNER REPORT 2026-11-05 "the scroll bar is getting cutoff"): three in-use items show no bar (2009 hides
/// it below 7 rows) and no scroll art is drawn in the pane, and a row's right end is the row; with eight the bar is
/// the list record's (x + 197, y + 4, 14 wide, the record's height less 4), its arrows drawn once, every piece inside the list
/// record and the pane window.
#[test]
fn rom_the_in_use_list_bar_is_hidden_below_seven_rows_and_inside_the_list_above() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let scroll_art = |items: &[DrawItem]| -> Vec<(String, [f32; 4])> {
        items
            .iter()
            .filter_map(|d| match d {
                DrawItem::Image { tex, dst, .. } if dst[0] < 470.0 => {
                    let t = tex.to_ascii_lowercase();
                    SCROLL_TEXTURES
                        .iter()
                        .any(|s| t.contains(s))
                        .then_some((t, *dst))
                }
                _ => None,
            })
            .collect()
    };
    market(&mut m, &active_items(&wares, 3), Vec::new());
    shot(&mut m, "market_3_rows");
    assert!(
        m.sheet().list_bar(list_index(&m)).is_none(),
        "no bar for 3 rows"
    );
    assert_eq!(
        scroll_art(&m.items_at(500.0)),
        Vec::new(),
        "no bar drawn for 3 rows"
    );
    let r = row_rect(&m, 2);
    tap(&mut m, [(r[0] + r[2] - 3) as f32, (r[1] + r[3] / 2) as f32]);
    assert!(storage_up(&m), "the row's right end opens the storage page");

    let mut m = menu().unwrap();
    market(&mut m, &active_items(&wares, 8), Vec::new());
    shot(&mut m, "market_8_rows");
    let i = list_index(&m);
    let list = m.sheet().parts[i].clone();
    let Look::List {
        height, scroll_at, ..
    } = list.look
    else {
        panic!("not a list");
    };
    let window = m
        .sheet()
        .parts
        .iter()
        .find(|p| p.layer == 2 && p.kind == WidgetKind::Window)
        .expect("pane window")
        .clone();
    let bar = m.sheet().list_bar(i).expect("a bar for 8 rows").clone();
    assert_eq!(
        bar.rect,
        [
            list.pos[0] + scroll_at[0],
            list.pos[1] + scroll_at[1],
            14,
            height - scroll_at[1]
        ]
    );
    let items = m.items_at(500.0);
    let art = scroll_art(&items);
    // drawn over the rows (2009: the list draws its bar after its rows)
    let last_row = items
        .iter()
        .rposition(|d| {
            matches!(d, DrawItem::Image { tex, dst, .. }
            if tex.eq_ignore_ascii_case(pane::BODY_ROW_IMAGE) && dst[0] < 470.0)
        })
        .expect("rows drawn");
    let first_bar = items
        .iter()
        .position(|d| {
            matches!(d, DrawItem::Image { tex, dst, .. }
            if dst[0] < 470.0 && tex.to_ascii_lowercase().contains("scroll_"))
        })
        .expect("bar drawn");
    assert!(first_bar > last_row, "the bar is drawn over the rows");
    for t in SCROLL_TEXTURES {
        assert!(art.iter().any(|(n, _)| n.contains(t)), "{t} drawn: {art:?}");
    }
    for t in &SCROLL_TEXTURES[2..] {
        let n = art.iter().filter(|(n, _)| n.contains(t)).count();
        assert_eq!(n, 1, "{t} drawn once");
    }
    let (top, bottom) = (list.pos[1] as f32, (list.pos[1] + height) as f32);
    let right = (window.pos[0] + window.size[0]) as f32;
    // the down arrow sits 7 above the bar's bottom at its texture's height (2009 placement): a pixel's overhang
    for (t, d) in art {
        assert!(
            d[1] >= top
                && d[1] + d[3] <= bottom + 1.0
                && d[0] >= list.pos[0] as f32
                && d[0] + d[2] <= right,
            "{t} at {d:?} outside the list record ({top}..{bottom}) or the pane (right {right})"
        );
    }
}
