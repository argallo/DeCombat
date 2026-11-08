//! DSV-625 (playtest report 2026-11-07: "mei's head seems to not be on its shoulders but instead on the ground by its
//! feet"). Since DSV-271 the game server dresses a player in his character item's body row, but it kept sending the
//! player's own face row, also for the rows that name a face of their own (`FaceIndex`; the 2009 client takes that
//! face, cshell+0x1e3430). A female character body then got a male face file: its head node is not on the female
//! skeleton, so the face kept its bind position, the male model space's head height (the male origin is at the hips,
//! the female one at the feet): at the female body's knees. The server now sends the row's own face.
//!
//! ROM-gated (skipped without `CA_ROM`). With `DC_MEI_HEAD_PNG=<file.png>` and a GPU adapter it also writes the
//! shop-pane render of the reported character with the face the server now sends (`<file>`) and with the male face
//! it used to send (`<file stem>_male_face.png`); ROM-derived, local files, never committed.

use std::path::{Path, PathBuf};
use std::time::Duration;

use dc_client::characters::FACE_IN_BODY_SPACE;
use dc_client::hud::items::DrawItem;
use dc_client::menu::figure::{FigureTables, LookIds, SLOT_CHARACTER};
use dc_client::menu::link::LinkOptions;
use dc_client::menu::modelview::{CharLook, ModelView, ViewReq, PANE_RECT, PLACE};
use dc_client::menu::{Menu, MenuOptions};
use dc_client::world::ModelRef;
use dc_dserver::chars::{loadout_look, CharTables, CharacterChoice, LOOK_CHARACTER};
use dc_render::{ModelFiles, ModelMesh, Skeleton};
use glam::{Mat4, Vec3};

/// The character item of the report's figure (body row 4, the female figure of CLI-1346).
const MEI_ITEM: u32 = 10004;

/// The bounds of a mesh skinned with `bones`.
fn skinned_bounds(mesh: &ModelMesh, bones: &[Mat4]) -> (Vec3, Vec3) {
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for v in &mesh.vertices {
        let p = Vec3::from(v.pos);
        let q: Vec3 = (0..4)
            .map(|k| v.weights[k] * bones[v.bones[k] as usize].transform_point3(p))
            .sum();
        lo = lo.min(q);
        hi = hi.max(q);
    }
    (lo, hi)
}

struct Loaded {
    skeleton: Skeleton,
    mesh: ModelMesh,
}

fn load(vfs: &dc_rez::RezVfs, path: &str) -> Option<Loaded> {
    let f = ModelFiles::load(vfs, path).ok()?;
    Some(Loaded {
        skeleton: Skeleton::of(&f.body),
        mesh: ModelMesh::build(&f.body),
    })
}

/// Where the face's vertices stand on the body in its bind pose, as the client skins it: the face's vertical centre
/// as a fraction of the body's height (0 the lowest body vertex, 1 the highest).
fn face_height(body: &Loaded, face: &Loaded) -> f32 {
    let map = face.skeleton.map_onto(&body.skeleton);
    let mut g = Vec::new();
    face.skeleton.follow_globals(
        &body.skeleton,
        &map,
        &body.skeleton.bind_global,
        FACE_IN_BODY_SPACE,
        &mut g,
    );
    let bones: Vec<Mat4> = g
        .iter()
        .zip(&face.skeleton.inverse_bind)
        .map(|(a, b)| *a * *b)
        .collect();
    let (flo, fhi) = skinned_bounds(&face.mesh, &bones);
    let (blo, bhi) = skinned_bounds(&body.mesh, &vec![Mat4::IDENTITY; body.skeleton.len()]);
    ((flo.y + fhi.y) * 0.5 - blo.y) / (bhi.y - blo.y)
}

/// Every character item whose body row names its own face, with every face row an account can carry: the server
/// sends the row's face and the client draws it on the body's head (its centre in the top fifth of the body).
#[test]
fn rom_every_character_with_its_own_face_wears_it_on_the_head() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let vfs = dc_rez::RezVfs::new(&rom);
    let t = CharTables::from_vfs(&vfs).expect("character tables");
    let mut cache: std::collections::BTreeMap<String, Option<Loaded>> = Default::default();
    let mut checked = 0;
    let mut low = Vec::new();
    let account_faces: Vec<u32> = t.faces.keys().copied().filter(|f| *f < 8).collect();
    for (&row, body_row) in t.bodies.iter().filter(|(_, b)| b.face_index.is_some()) {
        let equipped = [(Some(LOOK_CHARACTER.0), LOOK_CHARACTER.1 + row)];
        for &account_face in &account_faces {
            let (Some(body), _) = loadout_look(&t, &equipped, account_face) else {
                continue;
            };
            assert_eq!(body, row, "the character item picks its body row");
            let face_row = t.face_row_of(CharacterChoice {
                model_row: row,
                skin: 0,
                face_row: account_face,
            });
            assert_eq!(Some(face_row), body_row.face_index, "body row {row}");
            let Some(face_file) = t.faces.get(&face_row).map(|f| f.model.clone()) else {
                continue;
            };
            for p in [&body_row.model, &face_file] {
                if !cache.contains_key(p) {
                    cache.insert(p.clone(), load(&vfs, p));
                }
            }
            let (Some(b), Some(f)) = (&cache[&body_row.model], &cache[&face_file]) else {
                continue;
            };
            let h = face_height(b, f);
            checked += 1;
            if h < 0.8 {
                low.push((row, account_face, face_row, h));
            }
        }
    }
    eprintln!("checked {checked} body / face pairs");
    assert!(
        checked > 0,
        "the ROM has character items with their own face"
    );
    assert!(
        low.is_empty(),
        "faces drawn below the head (body row, account face, face row, height): {low:?}"
    );
}

/// DSV-625: the report's character with an account's male face (row 0) gets its own face row from the server.
#[test]
fn rom_the_reported_character_gets_its_own_face() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let vfs = dc_rez::RezVfs::new(&rom);
    let t = CharTables::from_vfs(&vfs).expect("character tables");
    let equipped = [(Some(LOOK_CHARACTER.0), MEI_ITEM)];
    let (Some(row), _) = loadout_look(&t, &equipped, 0) else {
        eprintln!("skipped: no body row for the character item");
        return;
    };
    let own = t.bodies[&row].face_index.expect("the row names its face");
    let sent = t.face_row_of(CharacterChoice {
        model_row: row,
        skin: 0,
        face_row: 0,
    });
    assert_eq!(sent, own, "body row {row}: the server sends face {sent}");
    // the face the server used to send stands at the body's knees (the report); the one it sends now on the head
    let body = load(&vfs, &t.bodies[&row].model).expect("body");
    let old = load(&vfs, &t.faces[&0].model).expect("face 0");
    let new = load(&vfs, &t.faces[&sent].model).expect("the row's face");
    let (h_old, h_new) = (face_height(&body, &old), face_height(&body, &new));
    eprintln!("body row {row}: face 0 at {h_old:.2} of the height, face {sent} at {h_new:.2}");
    assert!(
        h_old < 0.5,
        "face 0 is drawn at {h_old} of the body's height"
    );
    assert!(
        h_new > 0.8,
        "face {sent} is drawn at {h_new} of the body's height"
    );
    if let Some(p) = std::env::var_os("DC_MEI_HEAD_PNG") {
        shots(&rom, &t, row, sent, &PathBuf::from(p));
    }
}

/// The shop-pane renders of body row `row` with face `face` and with face row 0.
fn shots(rom: &Path, t: &CharTables, row: u32, face: u32, out: &Path) {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("t"),
        rom: Some(rom.to_path_buf()),
        font: None,
        width: 1024,
        height: 768,
    });
    m.detach_link();
    let figure = FigureTables::cached(rom);
    let mut eq = [-1; 15];
    eq[SLOT_CHARACTER] = MEI_ITEM as i32;
    let ids = LookIds::from_equipped(&eq);
    let Some((look, _)) = figure.look(&ids) else {
        return;
    };
    let male = out.with_file_name(format!(
        "{}_male_face.png",
        out.file_stem().unwrap_or_default().to_string_lossy()
    ));
    for (f, path) in [(face, out.to_path_buf()), (0, male)] {
        let Some(row_face) = t.faces.get(&f) else {
            continue;
        };
        let models = vec![
            look.models[0].clone(),
            ModelRef {
                model: row_face.model.clone(),
                textures: row_face.skins.iter().take(1).cloned().collect(),
            },
        ];
        let mut v = ModelView::new(rom.to_path_buf());
        let req = ViewReq {
            look: Some(CharLook {
                models,
                weapon: None,
                styles: Vec::new(),
            }),
            pieces: Vec::new(),
            lights: figure.lights(&ids),
            place: PLACE,
            rect: PANE_RECT,
            xray: None,
            showcase: None,
        };
        v.want(Some(req), (1024, 768), Some(0.0), 0.0);
        v.wait(&mut m.cat.assets, Duration::from_secs(120));
        let Some(DrawItem::Image { tex, .. }) = v.item() else {
            eprintln!("no character view (no GPU adapter)");
            return;
        };
        let Some(img) = m.cat.assets.generated(&tex) else {
            continue;
        };
        let _ = image::save_buffer(
            &path,
            &img.rgba,
            img.width,
            img.height,
            image::ColorType::Rgba8,
        );
        eprintln!("body row {row} face {f}: {}", path.display());
    }
}
