//! ROM- and GPU-gated (skipped without `CA_ROM` or a GPU adapter): the infected look of Quarantine (DSV-214) drawn
//! headless. The character of the default look is rendered offscreen twice, once with its own face and once with the
//! face row of the infected override row the server sends (override row 0, its `FaceIndex`); the head region of the
//! two pictures differs and the rest of the body is drawn in both. With `DC_SHOT_DIR` set the two pictures are saved
//! there (`qrt_human.png`, `qrt_infected.png`). Nothing of the ROM is asserted by value and no ROM name is printed.
// DC-DEFAULT: DSV-214 (proposed); VIS-25 (proposed) the headless picture of the infected look

use std::time::Duration;

use dc_client::hud::compose::{HudAssets, HudFonts, NoTextures};
use dc_client::menu::figure::{FigureTables, LookIds};
use dc_client::menu::modelview::{ModelView, ViewReq, PANE_RECT, PLACE};
use dc_client::ModelRef;

fn render(rom: &std::path::Path, req: ViewReq) -> Option<dc_client::hud::compose::Tex> {
    let mut view = ModelView::new(rom.to_path_buf());
    let mut assets = HudAssets::new(Box::new(NoTextures), HudFonts::new(None, 16.0, None, 1.0));
    view.want(Some(req), (238, 480), Some(0.0), 0.0);
    view.wait(&mut assets, Duration::from_secs(120));
    let dc_client::hud::items::DrawItem::Image { tex, .. } = view.item()? else {
        return None;
    };
    assets.texture(&tex).map(|t| (*t).clone())
}

fn opaque(t: &dc_client::hud::compose::Tex, rows: std::ops::Range<u32>) -> usize {
    rows.flat_map(|y| (0..t.width).map(move |x| (x, y)))
        .filter(|&(x, y)| t.rgba[((y * t.width + x) * 4 + 3) as usize] > 0)
        .count()
}

#[test]
fn rom_the_infected_face_changes_the_head_of_the_figure() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    if dc_render::GpuContext::headless(dc_render::AdapterChoice::Auto).is_err() {
        println!("skipped: no GPU adapter");
        return;
    }
    let vfs = dc_rez::RezVfs::new(&rom);
    let chars = dc_dserver::chars::CharTables::from_vfs(&vfs).expect("character tables");
    let infected_face = chars.overrides[&0]
        .face
        .and_then(|f| chars.faces.get(&f))
        .expect("the infected face row");
    let tables = FigureTables::from_rom(&rom);
    let (look, pieces) = tables
        .look(&LookIds::from_equipped(&[-1; 15]))
        .expect("the default look");
    assert!(look.models.len() >= 2, "body and face");
    let mut infected = look.clone();
    infected.models[1] = ModelRef {
        model: infected_face.model.clone(),
        textures: infected_face.skins.iter().take(1).cloned().collect(),
    };
    let req = |look| ViewReq {
        look: Some(look),
        pieces: pieces.clone(),
        lights: Vec::new(),
        place: PLACE,
        rect: PANE_RECT,
        xray: None,
    };
    // CLI-1150: the override row's render styles over the body, as the server's model block names them
    let mut styled = infected.clone();
    styled.styles = chars.overrides[&0].styles.clone();
    assert!(
        !styled.styles.is_empty(),
        "the override row names render styles"
    );
    let human = render(&rom, req(look)).expect("the human figure");
    let zombie = render(&rom, req(infected)).expect("the infected figure");
    let shaded = render(&rom, req(styled)).expect("the infected figure with its styles");
    assert_eq!((human.width, human.height), (zombie.width, zombie.height));
    let h = human.height;
    // the head lies in the top fifth of the pane; the legs in the bottom half are the same body in both
    let head = 0..h / 5;
    let diff = head
        .clone()
        .flat_map(|y| (0..human.width).map(move |x| ((y * human.width + x) * 4) as usize))
        .filter(|&i| human.rgba[i..i + 3] != zombie.rgba[i..i + 3])
        .count();
    assert!(diff > 50, "the head differs: {diff} pixels");
    assert!(opaque(&human, h / 2..h) > 500 && opaque(&zombie, h / 2..h) > 500);
    // the override styles change the body below the head (the client drew the table row's styles before CLI-1150)
    let body = h / 2..h;
    let changed = body
        .flat_map(|y| (0..zombie.width).map(move |x| ((y * zombie.width + x) * 4) as usize))
        .filter(|&i| zombie.rgba[i..i + 3] != shaded.rgba[i..i + 3])
        .count();
    assert!(changed > 500, "the body shading differs: {changed} pixels");
    if let Ok(dir) = std::env::var("DC_SHOT_DIR") {
        let dir = std::path::Path::new(&dir);
        let _ = std::fs::create_dir_all(dir);
        for (name, t) in [
            ("qrt_human.png", &human),
            ("qrt_infected.png", &zombie),
            ("qrt_infected_styled.png", &shaded),
        ] {
            image::RgbaImage::from_raw(t.width, t.height, t.rgba.clone())
                .expect("rgba")
                .save(dir.join(name))
                .expect("png");
        }
    }
}
