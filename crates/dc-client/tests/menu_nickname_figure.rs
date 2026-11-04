//! CLI-1485 (playtest report: on the character creation screen the last model's head floats above the body):
//! the figure of every creation-table model is its body row with the row's face, and the face is skinned in the
//! body's bind space, so with the body in its bind pose every face node the body shares by name gets the identity
//! skin (the face's vertices stay where the body's model space puts them). The sixth model's face file binds its
//! nodes 2.27 units below the body file's; skinned with its own bind the face stood that far above the neck.
//!
//! ROM-gated (skipped without `CA_ROM`). With `DC_MENU_SHOTS` and a GPU adapter it also writes the pane render of
//! each model as `nickname_model<i>.png` (ROM-derived: a local file, never committed).

use std::time::Duration;

use dc_client::characters::FACE_IN_BODY_SPACE;
use dc_client::hud::items::DrawItem;
use dc_client::menu::figure::FigureTables;
use dc_client::menu::link::LinkOptions;
use dc_client::menu::modelview::{ModelView, ViewReq};
use dc_client::menu::nickname::{models_from_rom, PANE_RECT, PLACE};
use dc_client::menu::{Menu, MenuOptions};
use dc_render::{ModelFiles, Skeleton};

/// The largest entry of the face's skin matrices (face node transform x face inverse bind) away from the identity,
/// over the nodes the face shares with the body, with the body in its bind pose.
fn face_offset(body: &Skeleton, face: &Skeleton, body_space: bool) -> (usize, f32) {
    let map = face.map_onto(body);
    let mut g = Vec::new();
    face.follow_globals(body, &map, &body.bind_global, body_space, &mut g);
    let mut worst = 0f32;
    let mut shared = 0;
    for (n, m) in map.iter().enumerate() {
        if m.is_none() {
            continue;
        }
        shared += 1;
        let skin = g[n] * face.inverse_bind[n];
        let d = skin
            .to_cols_array()
            .iter()
            .zip(glam::Mat4::IDENTITY.to_cols_array())
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        worst = worst.max(d);
    }
    (shared, worst)
}

#[test]
fn rom_every_creation_model_wears_its_face_on_the_body() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let models = models_from_rom(&rom);
    assert_eq!(models.len(), 6, "the EU table offers six models");
    if let Some(dir) = std::env::var_os("DC_MENU_SHOTS") {
        shots(&rom, &models, std::path::Path::new(&dir));
    }
    let t = FigureTables::cached(&rom);
    let vfs = dc_rez::RezVfs::new(&rom);
    let mut own_bind_off = Vec::new();
    for (i, m) in models.iter().enumerate() {
        let ids = m.look_ids();
        let (look, _) = t.look(&ids).expect("the model's look");
        assert_eq!(look.models.len(), 2, "model {i}: body and face");
        let body = ModelFiles::load(&vfs, &look.models[0].model).expect("body file");
        let face = ModelFiles::load(&vfs, &look.models[1].model).expect("face file");
        let (bs, fs) = (Skeleton::of(&body.body), Skeleton::of(&face.body));
        let (shared, off) = face_offset(&bs, &fs, FACE_IN_BODY_SPACE);
        let (_, own) = face_offset(&bs, &fs, false);
        eprintln!(
            "model {i}: body row {}, face row {}, shared nodes {shared}, skin offset {off:.4} (own bind {own:.4})",
            t.body_row(&ids),
            m.face
        );
        assert!(shared > 0, "model {i}: the face shares nodes with the body");
        assert!(
            off < 1e-3,
            "model {i}: the face is skinned {off} away from the body's model space"
        );
        if own > 1.0 {
            own_bind_off.push(i);
        }
    }
    // the case of the report: the sixth model's face file binds its nodes away from the body's (a face skinned
    // with its own bind moves by that much)
    assert!(
        own_bind_off.contains(&5),
        "the sixth model's face file binds as the body: {own_bind_off:?}"
    );
}

/// The pane render of each model, as the creation screen requests it.
fn shots(
    rom: &std::path::Path,
    models: &[dc_client::menu::nickname::ModelChoice],
    dir: &std::path::Path,
) {
    let mut menu = Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("t"),
        rom: Some(rom.to_path_buf()),
        font: None,
        width: 1024,
        height: 768,
    });
    menu.detach_link();
    let t = FigureTables::cached(rom);
    let _ = std::fs::create_dir_all(dir);
    for (i, m) in models.iter().enumerate() {
        let ids = m.look_ids();
        let Some((look, pieces)) = t.look(&ids) else {
            continue;
        };
        let mut v = ModelView::new(rom.to_path_buf());
        let req = ViewReq {
            look: Some(look),
            pieces,
            lights: t.lights(&ids),
            place: PLACE,
            rect: PANE_RECT,
            xray: None,
        };
        v.want(Some(req), (1024, 768), Some(0.0), 0.0);
        v.wait(&mut menu.cat.assets, Duration::from_secs(120));
        let Some(DrawItem::Image { tex, .. }) = v.item() else {
            eprintln!("no character view (no GPU adapter)");
            return;
        };
        let Some(img) = menu.cat.assets.generated(&tex) else {
            continue;
        };
        let p = dir.join(format!("nickname_model{i}.png"));
        let _ = image::save_buffer(
            &p,
            &img.rgba,
            img.width,
            img.height,
            image::ColorType::Rgba8,
        );
    }
}
