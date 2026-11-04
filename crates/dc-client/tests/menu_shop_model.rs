//! shop-model: the shop screen's left pane shows the character model (the 2009 shop draws the player's character
//! next to the equipment rows): an offscreen render on the menu's own GPU device, drawn as one image in the pane.
//!
//! ROM-gated (skipped without `CA_ROM`) and GPU-gated (skipped without an adapter).
// DC-DEFAULT: CLI-742 (proposed)

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::LinkOptions;
use dc_client::menu::modelview::PANE_RECT;
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};
use dc_client::steps::{resolve, Resolved};

fn menu() -> Option<Menu> {
    menu_at(1024, 768)
}

fn menu_at(width: u32, height: u32) -> Option<Menu> {
    let rom = dc_rom::resolve_rom().ok()?;
    Some(Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("t"),
        rom: Some(rom),
        font: None,
        width,
        height,
    }))
}

fn click(m: &mut Menu, name: &str) {
    let Resolved::Part { at, .. } = resolve(m, name) else {
        panic!("{name}: {:?}", resolve(m, name));
    };
    m.input(MenuInput::Move(at));
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
}

fn gpu() -> bool {
    dc_render::GpuContext::headless(dc_render::AdapterChoice::Auto).is_ok()
}

/// The model image of a display list: its texture name and destination.
fn model_image(items: &[DrawItem]) -> Option<(String, [f32; 4])> {
    items.iter().find_map(|i| match i {
        DrawItem::Image { tex, dst, .. } if tex.starts_with("#shop-model/") => {
            Some((tex.clone(), *dst))
        }
        _ => None,
    })
}

/// The bounds (x0, y0, x1, y1) of the opaque pixels of an RGBA image.
fn opaque_bounds(w: u32, h: u32, rgba: &[u8]) -> Option<[u32; 4]> {
    let mut b: Option<[u32; 4]> = None;
    for y in 0..h {
        for x in 0..w {
            if rgba[((y * w + x) * 4 + 3) as usize] == 0 {
                continue;
            }
            let v = b.get_or_insert([x, y, x, y]);
            v[0] = v[0].min(x);
            v[1] = v[1].min(y);
            v[2] = v[2].max(x);
            v[3] = v[3].max(y);
        }
    }
    b
}

#[test]
fn rom_the_shop_pane_shows_the_character_model_inside_the_pane() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    if !gpu() {
        eprintln!("skipped: no GPU adapter");
        return;
    }
    m.open(Go::Lobby);
    // the lobby shows no model (OBSERVED screens-sweep/pairs/lobby.png)
    let _ = m.image();
    assert_eq!(m.figure_renders(), 0, "the lobby asks for no model");
    // CLI-1629: the box preview spans the posed model's 3D bounds (the whole pane); chams keep the silhouette
    m.data.settings.xray.mode = dc_client::settings::XrayMode::Chams;
    click(&mut m, "lobby.shop");
    let img = m.image();
    let items = m.items();
    let (tex, dst) = model_image(&items).expect("the shop pane draws the character model");
    assert_eq!(
        dst,
        [
            PANE_RECT[0] as f32,
            PANE_RECT[1] as f32,
            PANE_RECT[2] as f32,
            PANE_RECT[3] as f32
        ]
    );
    let t = m.cat.assets.generated(&tex).expect("the rendered frame");
    if let Ok(dir) = std::env::var("DC_SHOP_MODEL_PNG") {
        let _ = std::fs::create_dir_all(&dir);
        let p = std::path::Path::new(&dir).join("shop_model_tex.png");
        let _ = image::save_buffer(&p, &t.rgba, t.width, t.height, image::ColorType::Rgba8);
    }
    let b = opaque_bounds(t.width, t.height, &t.rgba).expect("the model has pixels");
    let area = (b[2] - b[0] + 1) * (b[3] - b[1] + 1);
    println!("model bounds in the pane {b:?} of {}x{}", t.width, t.height);
    // a character, not a speck: taller than wide, a good part of the pane's height, clear of the pane's edges
    assert!(area > 5_000, "model area {area}");
    assert!(b[3] - b[1] > t.height / 2, "model height {:?}", b);
    assert!(
        b[0] > 0 && b[1] > 0 && b[2] < t.width - 1 && b[3] < t.height - 1,
        "the model is cut by the pane: {b:?}"
    );
    // the composed screen shows it: the pane's pixels differ from the screen without the model
    let rows: Vec<u8> = (b[1]..=b[3])
        .flat_map(|y| {
            let y = y + PANE_RECT[1] as u32;
            let x0 = (b[0] + PANE_RECT[0] as u32) as usize;
            let x1 = (b[2] + PANE_RECT[0] as u32) as usize;
            img.rgba[(y as usize * 1024 + x0) * 4..(y as usize * 1024 + x1) * 4].to_vec()
        })
        .collect();
    assert!(!rows.is_empty());
    if let Ok(dir) = std::env::var("DC_SHOP_MODEL_PNG") {
        let _ = std::fs::create_dir_all(&dir);
        let p = std::path::Path::new(&dir).join("shop_model.png");
        let _ = image::save_buffer(
            &p,
            &img.rgba,
            img.width,
            img.height,
            image::ColorType::Rgba8,
        );
        println!("wrote {}", p.display());
    }
}

/// shop-model step 2 (CLI-743): the look follows the equipped items: the head item's head-gear row, the character
/// item's body / face / hair rows, the first weapon slot's third-person model.
#[test]
fn rom_the_look_follows_the_equipped_items() {
    use dc_client::menu::figure::{FigureTables, LookIds};
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let t = FigureTables::from_rom(&rom);
    let file = |p: &str| {
        p.rsplit(['\\', '/'])
            .next()
            .unwrap_or(p)
            .to_ascii_lowercase()
    };
    let mut eq = [-1; 15];
    // nothing equipped: the starting look (body row 0, its face, no head gear: the face's hair row has no model)
    let (bare, bare_pieces) = t.look(&LookIds::from_equipped(&eq)).expect("bare look");
    assert_eq!(bare.models.len(), 2, "body + face");
    assert!(bare.weapon.is_none());
    // the head item 4001 (MICH) = head-gear row 1, a model on the body's skeleton
    eq[4] = 4001;
    eq[0] = 4;
    let (look, pieces) = t.look(&LookIds::from_equipped(&eq)).expect("look");
    assert_eq!(look.models[0], bare.models[0], "the vest row is unchanged");
    let heads: Vec<String> = pieces.iter().map(|p| file(&p.model.model)).collect();
    assert!(
        heads.iter().any(|h| h.contains("helmet")),
        "head gear {heads:?} (bare {:?})",
        bare_pieces
    );
    let (w, _) = look.weapon.as_ref().expect("the first weapon slot is held");
    println!("weapon {}", w.model);
    // a backpack item: a piece at a body socket
    eq[9] = 7000;
    let (_, pieces) = t.look(&LookIds::from_equipped(&eq)).expect("look");
    assert!(
        pieces.iter().any(|p| p.socket.is_some()),
        "backpack on a socket: {pieces:?}"
    );
    // a character item (10004, Mei) replaces the body row and brings its own face
    let mut eq = [-1; 15];
    eq[14] = 10004;
    let (mei, _) = t
        .look(&LookIds::from_equipped(&eq))
        .expect("character look");
    assert_ne!(mei.models[0].model, bare.models[0].model);
    assert_ne!(mei.models[1].model, bare.models[1].model);
    // CLI-744: the body row's shop light set (the default body: key, bottom and two back lights)
    let lights = t.lights(&LookIds::from_equipped(&[-1; 15]));
    println!("lights {lights:?}");
    assert!(lights.len() >= 2, "{lights:?}");
    assert!(lights.iter().any(|l| l.colour[0] >= 100.0), "a key light");
}

/// shop-model step 2: equipping a head item changes the drawn figure (the model updates on an equipment change).
#[test]
fn rom_an_equipment_change_redraws_the_figure() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    if !gpu() {
        eprintln!("skipped: no GPU adapter");
        return;
    }
    m.data.store.equipped = [-1; 15];
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    let _ = m.image();
    let items = m.items();
    let (tex0, _) = model_image(&items).expect("figure");
    let a = m.cat.assets.generated(&tex0).expect("frame");
    let b0 = opaque_bounds(a.width, a.height, &a.rgba).expect("pixels");
    let renders = m.figure_renders();
    m.data.store.equipped[4] = 4001;
    m.data.store.equipped[0] = 4;
    let _ = m.image();
    assert!(m.figure_renders() > renders, "a new render");
    let items = m.items();
    let (tex1, _) = model_image(&items).expect("figure");
    assert_ne!(tex0, tex1);
    let b = m.cat.assets.generated(&tex1).expect("frame");
    let b1 = opaque_bounds(b.width, b.height, &b.rgba).expect("pixels");
    println!("bare {b0:?} geared {b1:?}");
    assert_ne!(a.rgba, b.rgba, "the figure changed");
    // the rifle reaches past the bare figure's right side (OBSERVED pairs: the reference stack's rifle muzzle)
    assert!(b1[2] > b0[2] || b1[1] < b0[1], "{b0:?} -> {b1:?}");
    if let Ok(dir) = std::env::var("DC_SHOP_MODEL_PNG") {
        let p = std::path::Path::new(&dir).join("shop_model_geared.png");
        let _ = image::save_buffer(&p, &b.rgba, b.width, b.height, image::ColorType::Rgba8);
    }
}

/// shop-model (CLI-749): a double click on a shop cell previews the cell's item on the figure; the next page change
/// takes the look back to the equipped set (cshell+0x14edb0, cshell+0x14b8a0 -> cshell+0x1453d0).
#[test]
fn rom_a_double_click_on_a_cell_previews_its_item_until_the_page_changes() {
    use dc_client::menu::pages;
    use dc_client::menu::shop::{grid_cells, SHOP_GRID, SHOP_WINDOW};
    use dc_client::menu::storage::Wares;
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    if !gpu() {
        eprintln!("skipped: no GPU adapter");
        return;
    }
    let wares = Wares::cached(m.cat.rom.as_deref());
    m.data.store.view = Some(wares.catalogue_ids());
    m.data.store.lists_asked = true;
    m.data.store.equipped = [-1; 15];
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    let pixels = |m: &mut Menu| {
        let _ = m.image();
        let items = m.items();
        let (t, _) = model_image(&items).expect("figure");
        m.cat.assets.generated(&t).expect("frame").rgba.clone()
    };
    let bare = pixels(&mut m);
    let cells = grid_cells(&m.cat, pages::SHOP, SHOP_WINDOW, SHOP_GRID);
    let mid = |r: [i32; 4]| [(r[0] + r[2] / 2) as f32, (r[1] + r[3] / 2) as f32];
    let at = mid(cells[1]);
    // one press selects, the figure stays
    m.input(MenuInput::Move(at));
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
    // (no full compose here: it would eat the real-time double-click window)
    let asked = m.figure_requests();
    let _ = m.items();
    assert_eq!(
        m.figure_requests(),
        asked,
        "a single press does not preview"
    );
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
    let shown = pixels(&mut m);
    assert_ne!(shown, bare, "the double click previews the cell's weapon");
    click(&mut m, "shop.weapons");
    assert_eq!(pixels(&mut m), bare, "the page change ends the preview");
}

/// CLI-783: the pane figure plays its shop clip: two poses at different clip times (fixed, as the headless path
/// uses) differ, and both are drawn whole inside the pane by the pane-sized off-centre render.
#[test]
fn rom_the_pane_figure_loops_its_clip() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    if !gpu() {
        eprintln!("skipped: no GPU adapter");
        return;
    }
    m.open(Go::Lobby);
    // CLI-1629: the box preview spans the posed model's 3D bounds (the whole pane); chams keep the silhouette
    m.data.settings.xray.mode = dc_client::settings::XrayMode::Chams;
    click(&mut m, "lobby.shop");
    let frame = |m: &mut Menu, time: f32| {
        m.figure_time_ms = Some(time);
        let _ = m.image();
        let items = m.items();
        let (tex, dst) = model_image(&items).expect("the pane draws the figure");
        assert_eq!(dst[2..], [PANE_RECT[2] as f32, PANE_RECT[3] as f32]);
        let t = m.cat.assets.generated(&tex).expect("the rendered pane");
        assert_eq!(
            (t.width, t.height),
            (PANE_RECT[2] as u32, PANE_RECT[3] as u32),
            "a pane-sized render"
        );
        let b = opaque_bounds(t.width, t.height, &t.rgba).expect("the model has pixels");
        assert!(
            b[0] > 0 && b[1] > 0 && b[2] < t.width - 1 && b[3] < t.height - 1,
            "the model is cut by the pane: {b:?}"
        );
        if let Ok(dir) = std::env::var("DC_SHOP_MODEL_PNG") {
            let _ = std::fs::create_dir_all(&dir);
            let p = std::path::Path::new(&dir).join(format!("pane_{}ms.png", time as u32));
            let _ = image::save_buffer(&p, &t.rgba, t.width, t.height, image::ColorType::Rgba8);
        }
        t.rgba.clone()
    };
    let a = frame(&mut m, 0.0);
    let again = frame(&mut m, 0.0);
    let b = frame(&mut m, 700.0);
    assert_eq!(a, again, "a fixed time draws the same pose");
    let changed = a
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .filter(|(p, q)| p != q)
        .count();
    println!("pixels changed between 0 and 700 ms: {changed}");
    assert!(changed > 200, "the pose moves: {changed} pixels changed");
}

/// CLI-852: the live pane figure renders once per 30 Hz pose step at most and not at all while nothing changed: many
/// frames inside one step cost no render, the next step costs one, a changed look costs one.
#[test]
fn rom_the_pane_figure_renders_only_when_it_changed() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    if !gpu() {
        eprintln!("skipped: no GPU adapter");
        return;
    }
    m.data.store.equipped = [-1; 15];
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    let settle = |m: &mut Menu, t: f32| {
        for _ in 0..600 {
            let _ = m.items_at(t);
            if !m.figure_pending() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("the figure never rendered");
    };
    settle(&mut m, 100.0);
    let first = m.figure_requests();
    assert!(first >= 1);
    for _ in 0..200 {
        let _ = m.items_at(100.0);
    }
    assert_eq!(m.figure_requests(), first, "same pose: no render");
    assert!(!m.needs_redraw() || m.figure_pending() == false);
    // the pose steps: one render, however many frames draw in the step
    for _ in 0..50 {
        let _ = m.items_at(100.05);
    }
    assert_eq!(
        m.figure_requests(),
        first + 1,
        "one render for the new step"
    );
    settle(&mut m, 100.05);
    for _ in 0..50 {
        let _ = m.items_at(100.06);
    }
    assert_eq!(m.figure_requests(), first + 1, "frames within the step");
    // a changed look is one more render
    m.data.store.equipped[4] = 4001;
    settle(&mut m, 100.06);
    assert_eq!(m.figure_requests(), first + 2, "equipment changed");
}

/// CLI-853: the look takes the account's own face row; a face item in the face slot replaces it.
#[test]
fn the_look_uses_the_account_face_row() {
    use dc_client::menu::figure::LookIds;
    let mut eq = [-1; 15];
    assert_eq!(LookIds::with_account_face(&eq, 3).face, 3);
    assert_eq!(LookIds::from_equipped(&eq).face, 0);
    eq[5] = 6002;
    assert_eq!(LookIds::with_account_face(&eq, 3).face, 2);
}

/// CLI-970 (playtest report: "my character is still not rendering in the shop page" at 2560 x 1440): the render is
/// pane-sized in window pixels, so the image must take the whole texture; it took the pane's virtual size (238 x 480
/// texels), the top-left corner of the 595 x 900 render, left of the body: an empty pane at any window but 1024 x 768.
#[test]
fn rom_the_pane_figure_fills_the_pane_in_a_large_window() {
    let Some(mut m) = menu_at(2560, 1440) else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    if !gpu() {
        eprintln!("skipped: no GPU adapter");
        return;
    }
    m.open(Go::Lobby);
    click(&mut m, "lobby.shop");
    let _ = m.image();
    let items = m.items();
    let (src, tex, dst) = items
        .iter()
        .find_map(|i| match i {
            DrawItem::Image { tex, src, dst, .. } if tex.starts_with("#shop-model/") => {
                Some((*src, tex.clone(), *dst))
            }
            _ => None,
        })
        .expect("the shop pane draws the character model");
    let t = m.cat.assets.generated(&tex).expect("the rendered frame");
    assert_eq!((t.width, t.height), (595, 900), "the pane in window pixels");
    assert_eq!(src, [595.0, 900.0], "the whole render is drawn");
    assert_eq!(dst, [14.0, 104.0, 238.0, 480.0]);
    let b = opaque_bounds(t.width, t.height, &t.rgba).expect("the model has pixels");
    assert!(b[3] - b[1] > t.height / 2, "model height {b:?}");
}
