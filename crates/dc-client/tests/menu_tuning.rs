//! lobby-fixes10 item 2: the Modification screen attaches a bought option to a weapon and detaches it again,
//! driven through the menu's own input (headless, no lobby server: the requests are read back from the link's
//! last-sent hook, the replies fed as link events). ROM-gated (names and option kinds from the user's item table).
// SPEC: PROTOCOL_2009.md §4.2 (0x336 / 0x337, 0x338 / 0x339)
// EVIDENCE: cshell+0x15a110 (Equip 1008, Weapon Select 1009), cshell+0x154e90, cshell+0x152c20

use dc_client::menu::link::{InvRecord, LinkEvent, LinkOptions};
use dc_client::menu::storage::Wares;
use dc_client::menu::tuning::{TuningScreen, CMD_FIT, CMD_PICK_GUN, CMD_TABS};

/// A point on the weapon select header's Short_Front (image (2, 0) at (20, 116), 187 x 33).
const HEADER_FRONT: [f32; 2] = [113.0, 132.0];
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};

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

fn press_button(m: &mut Menu, cmd: i32) {
    let b = m
        .sheet()
        .button(cmd)
        .filter(|b| b.shown)
        .expect("button shown")
        .clone();
    tap(
        m,
        [
            (b.pos[0] + b.size[0] / 2) as f32,
            (b.pos[1] + b.size[1] / 2) as f32,
        ],
    );
}

fn shot(m: &mut Menu, name: &str) {
    let Some(dir) = std::env::var_os("DC_TUNING_SHOTS") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let img = m.image();
    dc_render::FrameImage {
        width: img.width,
        height: img.height,
        rgba: img.rgba,
    }
    .save_png(&dir.join(format!("{name}.png")))
    .unwrap();
}

/// Runs the header's slide to its end on a fixed clock from `t0` (the first frame takes the start).
fn slide(m: &mut Menu, t0: f32) {
    m.items_at(t0);
    m.items_at(t0 + 1.0);
}

fn item(wares: &Wares, range: std::ops::Range<u32>, name: &str) -> u32 {
    range
        .clone()
        .find(|n| wares.label_of(*n as i32) == name)
        .unwrap_or_else(|| {
            panic!(
                "no {name} in the item table: {:?}",
                range
                    .filter_map(|n| {
                        let l = wares.label_of(n as i32);
                        (!l.starts_with('#')).then_some(l)
                    })
                    .collect::<Vec<_>>()
            )
        })
}

#[test]
fn rom_modification_attaches_an_s3_and_detaches_it() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let m4 = item(&wares, 0..1000, "M4A1");
    let mp5 = item(&wares, 0..1000, "MP5A4");
    let s3 = item(&wares, 1000..2000, "S3 Suppressor");
    let rec = |uid, item_no| InvRecord {
        uid,
        item_no,
        ..Default::default()
    };
    m.data.store.items = vec![rec(10, m4), rec(11, mp5), rec(20, s3)];
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[0] = m4 as i32;
    m.open(Go::Tuning);
    // the S3 does not fit the MP5A4 (the weapon's mod list): box 0x4df, nothing sent
    pick_s3_on(&mut m, 1);
    let before = m.link_latest_request();
    press_button(&mut m, CMD_FIT);
    assert_eq!(m.link_latest_request(), before);
    assert_eq!(
        m.popup().map(|p| p.text.clone()),
        Some(m.cat.text(0x4df)),
        "the cannot-attach box"
    );
    m.input(MenuInput::Key(dc_client::menu::MenuKey::Enter));
    // on the M4A1 it attaches
    pick_s3_on(&mut m, 0);
    press_button(&mut m, CMD_FIT);
    assert_eq!(m.link_latest_request(), format!("Attach(20, {s3}, 10)"));
    m.feed_event(LinkEvent::AttachDone {
        ok: true,
        detach: false,
    });
    m.update();
    let s3_row = |m: &Menu| {
        m.data
            .store
            .items
            .iter()
            .find(|r| r.uid == 20)
            .unwrap()
            .parent_uid
    };
    assert_eq!(s3_row(&m), 10, "the S3 row's parent is the M4A1");
    reopen_after_the_link_ends(&mut m);
    shot(&mut m, "tuning_attached");
    // Equip (now Detach) on the attached S3: detach from its weapon
    press_button(&mut m, CMD_FIT);
    assert_eq!(m.link_latest_request(), format!("Detach(20, {s3}, 10)"));
    m.feed_event(LinkEvent::AttachDone {
        ok: true,
        detach: true,
    });
    m.update();
    assert_eq!(s3_row(&m), 0, "detached");
    reopen_after_the_link_ends(&mut m);
    shot(&mut m, "tuning_detached");
}

/// The weapon select list: weapon row `w`; its muzzle socket opens the option storage; the S3 cell.
fn pick_s3_on(m: &mut Menu, w: usize) {
    // the pointer on the collapsed header expands it to the button
    m.input(MenuInput::Move(HEADER_FRONT));
    slide(m, 50.0);
    tap(m, HEADER_FRONT);
    tap(m, TuningScreen::weapon_point(w));
    tap(m, TuningScreen::socket_point(0));
    tap(m, [118.0, 485.0]);
}

/// Without a lobby server the link ends at its first request ("connection ended" box, back to the login): the
/// screen is opened again over the same inventory and the S3 picked again on the M4A1.
fn reopen_after_the_link_ends(m: &mut Menu) {
    if m.popup_count() > 0 {
        m.input(MenuInput::Key(dc_client::menu::MenuKey::Enter));
        m.update();
        m.open(Go::Tuning);
        pick_s3_on(m, 0);
    }
}

/// CLI-760: the shop page opens with its first cell selected and the detail panel (frame, name, modifier rows);
/// the pane shows the shown weapon's picture and name image; a fitted socket shows its option's image and name; the
/// option storage marks an attached option with the Attached mark and its weapon's name.
#[test]
fn rom_modification_panel_pictures_and_attached_tag() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::modpanel::{PANEL_AT, PANEL_FRAME};
    use dc_client::menu::tuning::{ATTACHED_MARK, SOCKET_IMAGE_AT, SOCKET_ROW};
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    m.data.store.view = Some(wares.catalogue_ids());
    m.data.store.lists_asked = true;
    m.data.profile.exp = u32::MAX;
    let m4 = item(&wares, 0..1000, "M4A1");
    let s3 = item(&wares, 1000..2000, "S3 Suppressor");
    m.data.store.items = vec![
        InvRecord {
            uid: 10,
            item_no: m4,
            ..Default::default()
        },
        InvRecord {
            uid: 20,
            item_no: s3,
            parent_uid: 10,
            ..Default::default()
        },
    ];
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[0] = m4 as i32;
    m.open(Go::Tuning);
    let items = m.items().to_vec();
    let image_at = |items: &[DrawItem], tex: &str, x: i32, y: i32| {
        items.iter().any(|i| {
            matches!(i, DrawItem::Image { tex: t, dst, .. } if t == tex && dst[0] == x as f32 && dst[1] == y as f32)
        })
    };
    let label = |items: &[DrawItem], s: &str| {
        items
            .iter()
            .any(|i| matches!(i, DrawItem::Label { text, .. } if text == s))
    };
    assert!(
        image_at(&items, PANEL_FRAME.0, PANEL_AT[0], PANEL_AT[1]),
        "panel"
    );
    assert!(label(&items, &m.cat.text(0x5b5)), "Gunshot Noise row");
    let art = wares.art_of(m4 as i32).unwrap();
    assert!(
        image_at(&items, &art.tuning_picture, 360, 102),
        "M4A1 picture"
    );
    assert!(
        items
            .iter()
            .any(|i| matches!(i, DrawItem::Image { tex, .. } if *tex == art.tuning_name)),
        "name image"
    );
    let s3_icon = wares.art_of(s3 as i32).unwrap().icon.clone();
    assert!(image_at(
        &items,
        &s3_icon,
        SOCKET_ROW[0] + SOCKET_IMAGE_AT[0],
        SOCKET_ROW[1] + SOCKET_IMAGE_AT[1]
    ));
    tap(&mut m, TuningScreen::socket_point(0));
    let items = m.items().to_vec();
    assert!(
        items
            .iter()
            .any(|i| matches!(i, DrawItem::Image { tex, .. } if tex == ATTACHED_MARK.0)),
        "Attached mark"
    );
    assert!(label(&items, &wares.label_of(m4 as i32)), "weapon name");
    shot(&mut m, "tuning_attached_tag");
}

/// CLI-764: a fitted muzzle / sight draws its picture over the weapon picture (images 7 / 8): the S3 on the M4A1 at
/// ItemID4_Silencer_C_Pos (930, 650) x 0.3, TuningImgSize 512 x 256 x 0.3; the ACOG at ItemID4_Sight_A_Pos
/// (2150, 450) x 0.3; nothing without the option.
#[test]
fn rom_fitted_options_draw_their_pictures_on_the_weapon() {
    use dc_client::hud::items::DrawItem;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    m.data.store.view = Some(wares.catalogue_ids());
    m.data.store.lists_asked = true;
    let m4 = item(&wares, 0..1000, "M4A1");
    let s3 = item(&wares, 1000..2000, "S3 Suppressor");
    let acog = (1000..2000)
        .find(|n| {
            wares.label_of(*n as i32) == "ACOG TA1 Scope"
                && wares.art_of(*n as i32).and_then(|a| a.attach_class) == Some(0)
        })
        .expect("ACOG");
    let rec = |uid, item_no, parent_uid| InvRecord {
        uid,
        item_no,
        parent_uid,
        ..Default::default()
    };
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[0] = m4 as i32;
    let pictures = |m: &mut Menu| -> Vec<(String, [f32; 4])> {
        m.items()
            .iter()
            .filter_map(|i| match i {
                DrawItem::Image { tex, dst, .. } if tex.contains(r"ItemImg_Large\Option") => {
                    Some((tex.clone(), *dst))
                }
                _ => None,
            })
            .collect()
    };
    m.data.store.items = vec![rec(10, m4, 0), rec(20, s3, 0), rec(21, acog, 0)];
    m.open(Go::Tuning);
    assert!(pictures(&mut m).is_empty(), "no option fitted: no picture");
    m.data.store.items = vec![rec(10, m4, 0), rec(20, s3, 10), rec(21, acog, 10)];
    m.open(Go::Tuning);
    let got = pictures(&mut m);
    assert!(
        got.contains(&(
            r"UI_SCR_ItemImg_Large\Option\Sirencer_C.dtx".into(),
            [279.0, 195.0, 153.0, 76.0]
        )),
        "{got:?}"
    );
    assert!(
        got.contains(&(
            r"UI_SCR_ItemImg_Large\Option\Sight_A_AR_ALL.dtx".into(),
            [645.0, 135.0, 153.0, 76.0]
        )),
        "{got:?}"
    );
    shot(&mut m, "tuning_option_pictures");
}

/// CLI-765: the weapon select header is collapsed (Short_Front + Short_Back's last texels, no button) until the
/// pointer is on it, then shows the full button; the open list draws its rows as list items (Inven_Item_Slot frame,
/// slot image, class tag, name, Equiped mark) one item height + spacing apart and hides the socket rows.
#[test]
fn rom_weapon_select_header_collapses_and_the_list_draws_item_rows() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::tuning::{
        EQUIPPED_MARK, SOCKET_FRAMES, WEAPON_FRAMES, WEAPON_PITCH, WEAPON_ROW,
    };
    use dc_client::menu::MenuInput;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let m4 = item(&wares, 0..1000, "M4A1");
    let mp5 = item(&wares, 0..1000, "MP5A4");
    m.data.store.items = vec![
        InvRecord {
            uid: 10,
            item_no: m4,
            ..Default::default()
        },
        InvRecord {
            uid: 11,
            item_no: mp5,
            ..Default::default()
        },
    ];
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[0] = m4 as i32;
    m.open(Go::Tuning);
    let shown = |m: &Menu| m.sheet().button(CMD_PICK_GUN).is_some();
    let images = |m: &mut Menu| -> Vec<(String, [f32; 4])> {
        m.items()
            .iter()
            .filter_map(|i| match i {
                DrawItem::Image { tex, dst, .. } => Some((tex.clone(), *dst)),
                _ => None,
            })
            .collect()
    };
    assert!(!shown(&m), "collapsed: no button");
    let back = images(&mut m)
        .into_iter()
        .find(|(t, _)| t.contains("Short_Back"))
        .expect("Short_Back drawn");
    assert!(back.0.ends_with("#84,0,18,33"), "{back:?}");
    assert_eq!(back.1, [207.0, 116.0, 18.0, 33.0]);
    m.input(MenuInput::Move(HEADER_FRONT));
    slide(&mut m, 100.0);
    assert!(shown(&m), "the pointer on the header shows the button");
    assert!(!images(&mut m).iter().any(|(t, _)| t.contains("Short_")));
    m.input(MenuInput::Move([600.0, 300.0]));
    slide(&mut m, 200.0);
    assert!(!shown(&m), "off the header it collapses again");
    m.input(MenuInput::Move(HEADER_FRONT));
    slide(&mut m, 250.0);
    tap(&mut m, HEADER_FRONT);
    slide(&mut m, 300.0);
    let got = images(&mut m);
    for k in 0..2 {
        let y = (WEAPON_ROW[1] + WEAPON_PITCH * k) as f32;
        let lit = (k == 0) as usize;
        assert!(
            got.iter()
                .any(|(t, d)| t == WEAPON_FRAMES[lit] && d[0] == 32.0 && d[1] == y),
            "row {k} frame: {got:?}"
        );
    }
    let mark_x = (WEAPON_ROW[0] + WEAPON_ROW[2] - 1 - EQUIPPED_MARK.1[0]) as f32;
    assert_eq!(
        got.iter()
            .filter(|(t, d)| t == EQUIPPED_MARK.0 && d[0] == mark_x)
            .count(),
        1,
        "the equipped M4A1 only"
    );
    assert!(
        !got.iter().any(|(t, _)| t == SOCKET_FRAMES[0]),
        "socket rows hidden while the list is open"
    );
    m.input(MenuInput::Move([600.0, 300.0]));
    slide(&mut m, 400.0);
    assert!(shown(&m), "the open list keeps the header expanded");
    shot(&mut m, "tuning_weapon_list");
}

/// CLI-831: a modifier row's value is centred on the row's x + 0x82 (value control align 0x14, cshell+0xd4540 /
/// 0xd61d0), not right-aligned.
#[test]
fn rom_modifier_values_are_centred_on_the_row() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::modpanel::{PANEL_AT, STAT_LIST_AT, VALUE_CENTRE};
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    m.data.store.view = Some(wares.catalogue_ids());
    m.data.store.lists_asked = true;
    m.open(Go::Tuning);
    let want = (PANEL_AT[0] + STAT_LIST_AT[0] + VALUE_CENTRE) as f32;
    let found = m.items().iter().any(|i| {
        matches!(i, DrawItem::Label { text, pos, anchor, .. }
            if text.ends_with('%') && pos[0] == want && anchor[0] == 0.5)
    });
    assert!(found, "no value centred on x {want}");
}

/// CLI-850: the header slides on the menu clock: 10 texels per 5 ms toward the target (45 ms in all), so half way
/// through the cut is part way and the full button waits for the end.
#[test]
fn rom_weapon_select_header_slides_over_45_ms() {
    use dc_client::hud::items::DrawItem;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    m.open(Go::Tuning);
    let back_w = |m: &mut Menu, t: f32| -> Option<f32> {
        m.items_at(t).iter().find_map(|i| match i {
            DrawItem::Image { tex, dst, .. } if tex.contains("Short_Back") => Some(dst[2]),
            _ => None,
        })
    };
    assert_eq!(
        back_w(&mut m, 10.0),
        Some(18.0),
        "collapsed: 18 texels left"
    );
    m.input(MenuInput::Move(HEADER_FRONT));
    assert!(m.needs_redraw());
    assert_eq!(
        back_w(&mut m, 20.0),
        Some(18.0),
        "the slide starts at the first frame"
    );
    // 21 ms = 4 steps of 10 texels: the cut 0x54 -> 0x2c, the back is 18 + 40 = 58 wide
    assert_eq!(back_w(&mut m, 20.0 + 0.021), Some(58.0));
    assert!(m.sheet().button(CMD_PICK_GUN).is_none_or(|b| !b.shown));
    assert_eq!(
        back_w(&mut m, 20.2),
        None,
        "slid out: the full button replaces the pair"
    );
    assert!(m.sheet().button(CMD_PICK_GUN).is_some_and(|b| b.shown));
}

/// CLI-851: the open Weapon Select list scrolls: a bar of the list's range, the wheel moves it a row (97 texels) at
/// a time, rows that do not fit whole are not shown, and a press lands on the row under the pointer.
#[test]
fn rom_weapon_select_list_scrolls() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::tuning::{WEAPON_FRAMES, WEAPON_PITCH, WEAPON_ROW};
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let nos: Vec<u32> = ["M4A1", "MP5A4"]
        .iter()
        .map(|n| item(&wares, 0..1000, n))
        .collect();
    m.data.store.items = (0..12)
        .map(|k| InvRecord {
            uid: 10 + k,
            item_no: nos[k as usize % 2],
            ..Default::default()
        })
        .collect();
    m.open(Go::Tuning);
    m.input(MenuInput::Move(HEADER_FRONT));
    slide(&mut m, 450.0);
    tap(&mut m, HEADER_FRONT);
    let frames = |m: &mut Menu| -> Vec<f32> {
        m.items_at(500.0)
            .iter()
            .filter_map(|i| match i {
                DrawItem::Image { tex, dst, .. } if WEAPON_FRAMES.contains(&tex.as_str()) => {
                    Some(dst[1])
                }
                _ => None,
            })
            .collect()
    };
    let y0 = WEAPON_ROW[1] as f32;
    let first = frames(&mut m);
    assert_eq!(first.len(), 5, "five whole rows fit 475 texels: {first:?}");
    assert_eq!(first[0], y0);
    let list = m
        .sheet()
        .find(5, 0, dc_ui_data::WidgetKind::List)
        .expect("list 5/0");
    let bar = m
        .sheet()
        .list_bar(list)
        .expect("the list has a scroll bar")
        .clone();
    assert_eq!(bar.range, 12 * 0x56 + 11 * 11);
    assert_eq!(bar.step, WEAPON_PITCH);
    // turns away from the user scroll up (nothing at the top); two toward the user move two rows
    m.input(MenuInput::Wheel([100.0, 300.0], 3));
    assert_eq!(m.sheet().list_bar(list).unwrap().value, 0);
    m.input(MenuInput::Wheel([100.0, 300.0], -2));
    assert_eq!(m.sheet().list_bar(list).unwrap().value, 2 * WEAPON_PITCH);
    let moved = frames(&mut m);
    assert_eq!(moved.len(), 5, "{moved:?}");
    assert_eq!(moved[0], y0, "row 2 is now at the top");
    // the bar is drawn over the list (thumb below the top)
    assert!(m.sheet().list_bar(list).unwrap().thumb_rect()[1] > WEAPON_ROW[1] + 12);
    // the bar's down arrow scrolls one more row
    let arrow = m.sheet().list_bar(list).unwrap().arrow_point(1);
    tap(&mut m, arrow);
    assert_eq!(m.sheet().list_bar(list).unwrap().value, 3 * WEAPON_PITCH);
}

/// CLI-1432: the Modification page's purchase refusals are its own (cshell+0x1588a0): inventory full 0x3c -> 0x503,
/// any other reason (short of GP too) -> 0x46f "Item purchase request has failed.", titled 0x371.
#[test]
fn rom_modification_purchase_refusals_use_the_pages_own_boxes() {
    use dc_client::menu::link::BuyReply;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    m.open(Go::Tuning);
    // without a lobby the link ends at once: its box is dismissed and the screen opened again
    m.update();
    while m.popup_count() > 0 {
        m.input(MenuInput::Key(dc_client::menu::MenuKey::Enter));
        m.update();
    }
    m.open(Go::Tuning);
    for (reason, text) in [(0x3bu16, 0x46f), (0x3c, 0x503), (0x60, 0x46f)] {
        let n = m.popup_count();
        m.feed_event(LinkEvent::Bought(BuyReply {
            ok: false,
            reason,
            uid: 0,
            gp: 0,
            award: -1,
        }));
        m.update();
        assert!(m.popup_count() > n, "{reason:#x}: a box");
        let p = m.popup().unwrap();
        assert_eq!(p.text, m.cat.text(text), "{reason:#x}");
        assert_eq!(p.title, 0x371, "{reason:#x}");
        m.input(MenuInput::Key(dc_client::menu::MenuKey::Enter));
        m.update();
    }
}

/// User report 2026-10-31: an extended magazine would not go on a sniper. All magazine classes share the name
/// "Extended Magazine I" and the tab stayed on AR after the sniper was picked, so the AR one was bought and refused.
/// Picking the M24 now selects the SR tab (cshell+0x155d30 / 0x159420), and the SR magazine attaches.
#[test]
fn rom_picking_a_sniper_selects_the_sr_tab_and_its_magazine_attaches() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    let m4 = item(&wares, 0..1000, "M4A1");
    let sr = item(&wares, 0..1000, "M24");
    // the extended magazine of the sniper class (option weapon class 2)
    let mag = (1000..2000u32)
        .find(|n| {
            wares.label_of(*n as i32) == "Extended Magazine I"
                && wares.art_of(*n as i32).and_then(|a| a.attach_class) == Some(2)
        })
        .expect("a sniper extended magazine in the item table");
    let rec = |uid, item_no| InvRecord {
        uid,
        item_no,
        ..Default::default()
    };
    m.data.store.items = vec![rec(10, m4), rec(11, sr), rec(20, mag)];
    m.data.store.equipped = [-1; 15];
    m.data.store.equipped[0] = m4 as i32;
    m.open(Go::Tuning);
    let tab_on = |m: &Menu, k: usize| m.sheet().button(CMD_TABS[k]).is_some_and(|b| b.selected);
    assert!(tab_on(&m, 0), "the M4A1 shows the AR tab");
    m.input(MenuInput::Move(HEADER_FRONT));
    slide(&mut m, 50.0);
    tap(&mut m, HEADER_FRONT);
    tap(&mut m, TuningScreen::weapon_point(1));
    assert!(tab_on(&m, 1) && !tab_on(&m, 0), "the M24 shows the SR tab");
    tap(&mut m, TuningScreen::socket_point(2));
    tap(&mut m, [118.0, 485.0]);
    press_button(&mut m, CMD_FIT);
    assert_eq!(m.popup().map(|p| p.text.clone()), None, "no refusal box");
    assert_eq!(m.link_latest_request(), format!("Attach(20, {mag}, 11)"));
}

/// CLI-1606 (playtest report: "snipers that already have a supressor shouldn't be able to equip one from the
/// mod store"): a sniper whose muzzle socket state is 2 (the suppressor is on its model, e.g. the L96A1 Ghillie) lists
/// no muzzle option on the SR tab, while the M24 (state 1) and an ordinary rifle on the AR tab still list theirs.
// EVIDENCE: cshell+0x152d90 (attach only into a socket whose state is 1), cshell+0x17bd20 (MuzzleOPtion)
#[test]
fn rom_a_built_in_suppressor_lists_no_suppressor() {
    use dc_client::menu::tuning::{socket_open, tab_wares, weapon_tab_wares, OPTION_KINDS};
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let wares = Wares::cached(m.cat.rom.as_deref());
    m.data.store.view = Some(wares.catalogue_ids());
    let m4 = item(&wares, 0..1000, "M4A1") as i32;
    let m24 = item(&wares, 0..1000, "M24") as i32;
    let suppressed = (0..1000)
        .find(|n| {
            wares.kind_of(*n) == Some(2)
                && wares.art_of(*n).is_some_and(|a| a.socket_states[0] == 2)
        })
        .expect("a sniper with a built-in suppressor in the weapon table");
    let muzzles = |v: &[dc_client::menu::storage::Ware]| {
        v.iter()
            .filter(|w| w.kind_of == Some(OPTION_KINDS[0]))
            .count()
    };
    assert!(
        muzzles(&tab_wares(&wares, &m.data.store, 1)) > 0,
        "the SR tab sells suppressors"
    );
    assert!(socket_open(&wares, m24, 0) && !socket_open(&wares, suppressed, 0));
    let sr = |w: i32| weapon_tab_wares(&wares, &m.data.store, 1, Some(w));
    assert_eq!(
        muzzles(&sr(suppressed)),
        0,
        "weapon {suppressed} lists no suppressor"
    );
    assert!(muzzles(&sr(m24)) > 0, "the M24 lists its suppressors");
    assert!(sr(suppressed).len() > 0, "its magazines stay");
    assert!(
        muzzles(&weapon_tab_wares(&wares, &m.data.store, 0, Some(m4))) > 0,
        "an ordinary rifle lists suppressors"
    );
}
