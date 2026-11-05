//! With the user's ROM (skipped without one): which guns are drawn whole, and that the bake is the first-person texture and stays out of a pack.

use dc_gunkit::design::Art;
use dc_gunkit::guns::View;
use dc_gunkit::studio::Studio;
use dc_gunkit::whole;

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

fn gun(s: &Studio, name: &str) -> u32 {
    s.guns
        .iter()
        .find(|g| g.name == name)
        .unwrap_or_else(|| panic!("no gun named {name}"))
        .row
}

#[test]
fn rom_long_guns_are_drawn_whole_and_fit_closely() {
    let Some(mut s) = studio() else {
        return;
    };
    // FAMAS keeps its body on a later skin, and reads its muzzle the wrong way round on one model
    for name in ["G36E", "L96A1", "M16A3", "AK-47", "FAMAS G2", "MP7"] {
        let row = gun(&s, name);
        s.select(row).unwrap();
        s.set_view(View::FirstPerson).unwrap();
        let f = s
            .whole_fit()
            .unwrap_or_else(|| panic!("{name} is not whole"));
        assert!(f.overlap >= whole::MIN_OVERLAP, "{name}: {f:?}");
        assert!(f.matched > 0.95, "{name}: {f:?}");
        assert!(f.gap < whole::MAX_GAP, "{name}: {f:?}");
        assert!(!s.is_reflected(), "{name}: one or the other");
    }
}

#[test]
fn rom_what_only_looks_alike_keeps_its_own_first_person_view() {
    let Some(mut s) = studio() else {
        return;
    };
    for name in ["Medkit", "Bowie Knife", "M18 Smoke"] {
        let row = gun(&s, name);
        s.select(row).unwrap();
        s.set_view(View::FirstPerson).unwrap();
        assert_eq!(s.whole_fit(), None, "{name}");
    }
}

#[test]
fn rom_the_whole_gun_can_be_turned_off_and_gives_way_to_the_hands_and_third_person() {
    let Some(mut s) = studio() else {
        return;
    };
    s.select(gun(&s, "G36E")).unwrap();
    s.set_view(View::FirstPerson).unwrap();
    assert!(s.whole_fit().is_some());
    s.set_whole_gun(false).unwrap();
    assert!(s.whole_fit().is_none());
    assert!(s.is_reflected());
    s.set_whole_gun(true).unwrap();
    assert!(s.whole_fit().is_some());
    s.set_hands(true).unwrap();
    assert!(s.whole_fit().is_none());
    s.set_hands(false).unwrap();
    assert!(s.whole_fit().is_some());
    s.set_view(View::ThirdPerson).unwrap();
    assert!(s.whole_fit().is_none());
}

#[test]
fn rom_the_panel_still_shows_the_first_person_texture_and_a_change_of_paint_is_drawn() {
    let Some(mut s) = studio() else {
        return;
    };
    s.select(gun(&s, "G36E")).unwrap();
    s.set_view(View::FirstPerson).unwrap();
    let base = s.base_texture().unwrap();
    assert_eq!(base.width, 1024, "the first-person texture, not the bake");
    let before = s.picture(320, 180).unwrap().rgba;
    s.design.paint = Some(Art::from_rgba(2, 2, [255, 0, 255, 255].repeat(4)).unwrap());
    s.repaint().unwrap();
    assert_eq!(s.shown_texture().unwrap().width, 1024);
    let after = s.picture(320, 180).unwrap().rgba;
    let changed = before
        .as_chunks::<4>()
        .0
        .iter()
        .zip(after.as_chunks::<4>().0)
        .filter(|(a, b)| a != b)
        .count();
    assert!(changed > 1000, "{changed}");
}
