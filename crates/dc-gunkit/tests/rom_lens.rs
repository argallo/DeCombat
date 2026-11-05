//! With the user's ROM (skipped without one): the lenses the studio finds, against the circles found by hand for the example packs, which are the
//! answer key; and what the reflection leaves alone.

use dc_gunkit::guns::View;
use dc_gunkit::pack;
use dc_gunkit::studio::Studio;

fn studio() -> Option<Studio> {
    let rom = std::env::var_os("CA_ROM")?;
    match Studio::open(std::path::Path::new(&rom)) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

/// Every lens circle of an example pack that is large enough to be a lens, and not one of its small knobs, is found near where a person put it.
#[test]
fn rom_the_lenses_found_are_where_the_example_packs_put_them_by_hand() {
    let Some(mut s) = studio() else {
        return;
    };
    for example in ["G36E Galaxy", "L96A1 Toxic"] {
        let opened = pack::open_example(example).unwrap();
        for ((row, view), by_hand) in &opened.design.keep {
            s.design = Default::default();
            s.select(*row).unwrap();
            s.set_view(*view).unwrap();
            let main = by_hand
                .iter()
                .max_by(|a, b| a.radius.total_cmp(&b.radius))
                .unwrap();
            let found = s.lenses();
            let near = found.iter().any(|l| {
                let off = (l.circle.x - main.x).hypot(l.circle.y - main.y);
                off < main.radius * 0.3 && (l.circle.radius - main.radius).abs() < main.radius * 0.4
            });
            assert!(
                near,
                "{example} {view:?}: by hand {main:?}, found {found:?}"
            );
            assert_eq!(
                s.design.keep_of(*row, *view).len(),
                found.len(),
                "they are kept"
            );
        }
    }
}

#[test]
fn rom_a_gun_without_a_sight_is_painted_whole() {
    let Some(mut s) = studio() else {
        return;
    };
    for name in ["AK-47", "M16A3", "M1911 MEU"] {
        let row = s.find_gun(name).unwrap();
        s.select(row).unwrap();
        assert!(!s.gun().unwrap().has_scope, "{name}");
        assert!(s.lenses().is_empty(), "{name}: {:?}", s.lenses());
        assert!(
            s.design.keep_of(row, View::FirstPerson).is_empty(),
            "{name}"
        );
    }
}

#[test]
fn rom_a_list_set_by_hand_stays_until_asked_to_go_back() {
    let Some(mut s) = studio() else {
        return;
    };
    let row = s.find_gun("L96A1").unwrap();
    s.select(row).unwrap();
    assert!(!s.design.keep_of(row, View::FirstPerson).is_empty());
    s.design.keep.insert((row, View::FirstPerson), Vec::new());
    s.repaint().unwrap();
    assert!(
        s.design.keep_of(row, View::FirstPerson).is_empty(),
        "an empty list paints the lens"
    );
    s.use_found_lenses().unwrap();
    assert!(!s.design.keep_of(row, View::FirstPerson).is_empty());
}

/// A saved pack carries the lenses of the model that was never on show.
#[test]
fn rom_a_saved_pack_keeps_the_lens_of_both_models() {
    let Some(mut s) = studio() else {
        return;
    };
    let row = s.find_gun("G36E").unwrap();
    s.select(row).unwrap();
    s.design.paint = Some(dc_gunkit::patterns::generate("urban", None, 1).unwrap());
    let dir = std::env::temp_dir().join(format!("dc-gunkit-rom-lens-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let opts = pack::SaveOptions {
        name: "Lens Test".into(),
        author: String::new(),
        row: Some(800),
        dir: Some(dir.clone()),
    };
    s.save_pack(&opts).unwrap();
    let back = pack::open(&dir).unwrap();
    assert!(!back.design.keep_of(row, View::FirstPerson).is_empty());
    assert!(!back.design.keep_of(row, View::ThirdPerson).is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The texture's parts say where they lie, and the largest parts cover most of the gun.
#[test]
fn rom_describing_a_texture_names_its_parts() {
    let Some(mut s) = studio() else {
        return;
    };
    let row = s.find_gun("G36E").unwrap();
    s.select(row).unwrap();
    let parts = s.texture_parts(12);
    assert!(parts.len() > 3, "{parts:?}");
    let covered: f32 = parts.iter().map(|p| p.share).sum();
    assert!(covered > 0.5, "{covered}");
    let base = s.base_texture().unwrap();
    for p in &parts {
        assert!(!p.place.is_empty());
        assert!(
            p.texels[0] >= 0.0 && p.texels[2] <= base.width as f32,
            "{p:?}"
        );
    }
}

fn third_person_lenses(s: &mut Studio, name: &str) -> Vec<(f32, f32, f32)> {
    let row = s
        .guns
        .iter()
        .find(|g| g.name == name)
        .unwrap_or_else(|| panic!("no gun named {name}"))
        .row;
    s.select(row).unwrap();
    s.set_view(View::ThirdPerson).unwrap();
    s.lenses()
        .iter()
        .map(|l| (l.circle.x, l.circle.y, l.circle.radius))
        .collect()
}

/// Where the lens art lies on low third-person textures, read off the textures by eye. These lenses have two to four triangles, and are too plain to be told by
/// being round.
#[test]
fn rom_a_low_models_lenses_are_found_on_their_pictures() {
    let Some(mut s) = studio() else {
        return;
    };
    let answers: [(&str, &[(f32, f32)]); 6] = [
        ("M24", &[(26.0, 28.0), (163.0, 21.0)]),
        ("Dragunov SVD", &[(121.0, 235.0)]),
        ("PSG-1", &[(201.0, 25.0), (159.0, 25.0)]),
        ("SR25", &[(219.0, 20.0)]),
        ("MSG-90", &[(124.0, 133.0)]),
        ("AUG A1", &[(155.0, 69.0)]),
    ];
    for (name, spots) in answers {
        let found = third_person_lenses(&mut s, name);
        for spot in spots {
            assert!(
                found
                    .iter()
                    .any(|l| (l.0 - spot.0).hypot(l.1 - spot.1) < 6.0),
                "{name}: nothing near {spot:?} in {found:?}"
            );
        }
    }
}

/// A reflex sight has no lens picture, and its windows are kept clean instead. A sight with a skin of its own is not painted, so nothing is kept for it.
#[test]
fn rom_a_reflex_sight_keeps_its_windows_and_a_sight_on_its_own_skin_needs_nothing() {
    let Some(mut s) = studio() else {
        return;
    };
    assert_eq!(third_person_lenses(&mut s, "M416 CQB").len(), 2);
    assert!(third_person_lenses(&mut s, "RPK-74M").is_empty());
}

#[test]
fn rom_a_gun_without_a_sight_has_no_lens_on_its_low_model() {
    let Some(mut s) = studio() else {
        return;
    };
    for name in ["AK-47", "M16A3", "M14", "RPK"] {
        assert!(third_person_lenses(&mut s, name).is_empty(), "{name}");
    }
}
