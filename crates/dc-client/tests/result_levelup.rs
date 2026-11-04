//! w1-result: the result screen's level parts and the rank-up boxes against the user's ROM.
//!
//! - the level text (`%3.2f %%` progress), the level-up window 4 with the level's bonus digits, icon and text, and
//!   the level-up sound cue when the own level changed in the match;
//! - the level-up mark on the rows whose level changed;
//! - the coloured bonus run of the MY SCORE values;
//! - the rank-up boxes of a progress push (0x185f with the popup flag, then 0x487).
//!
//! ROM-gated (skipped without `CA_ROM`); nothing of the ROM is asserted by value. The PNG is ROM-derived and stays
//! local (reports/w1-result/).

use dc_botclient::lobby::LobbyProfile;
use dc_client::hud::board::LevelTable;
use dc_client::hud::items::DrawItem;
use dc_client::matchexit::MatchResult;
use dc_client::menu::catalog::{argb, Catalog};
use dc_client::menu::link::{LinkOptions, ProgressApplied};
use dc_client::menu::result::{
    rank_up_boxes, rank_up_texts, ResultArt, ResultScreen, BONUS_COLOUR, LEVEL_UP_SOUND,
    LEVEL_UP_TEXT, SUMMARY_LEVEL, WINDOW_LEVEL_UP, WINDOW_SUMMARY,
};
use dc_client::menu::{Go, Menu, MenuData, MenuOptions, Screen};
use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let o = dc_render::default_out_dir().join("w1-result");
    let _ = std::fs::create_dir_all(&o);
    o
}

/// A made-up result whose own row crossed from the level below `level` into it, with an experience bonus.
fn levelled_result(levels: &LevelTable, level: usize) -> (MatchResult, u32) {
    let before = levels.bounds[level - 1] - 1;
    let mut r = MatchResult::sample();
    let me = &mut r.rows[0];
    (me.exp_before, me.exp_gain, me.exp_bonus) = (before, 5, 3);
    (r, before)
}

#[test]
fn rom_result_screen_level_up_window_mark_level_text_and_bonus_run() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let levels = LevelTable::from_rom(&rom).expect("level table");
    let art = ResultArt::from_rom(&rom).expect("result art");
    assert!(art.level_up_mark.is_some(), "level-up mark literal pinned");
    assert!(
        art.digit_form.as_deref().is_some_and(|f| f.contains("%d")),
        "digit form pinned"
    );
    assert!(
        art.names.len() >= 2 && art.bonus.len() >= 2,
        "level columns read"
    );
    println!(
        "levels: {} bounds, {} names, {} bonus values, {} with a bonus",
        levels.bounds.len(),
        art.names.len(),
        art.bonus.len(),
        art.bonus.iter().filter(|b| **b >= 1).count()
    );
    // the first level with a bonus: the window opens when the match crosses into it
    let level = (1..levels.bounds.len())
        .find(|l| art.bonus(*l as u32) >= 1)
        .expect("a level with a bonus");
    let (result, before) = levelled_result(&levels, level);
    let cat = Catalog::from_rom(&rom, 1024, 768).expect("catalog");
    let data = MenuData {
        profile: LobbyProfile {
            nick: "Player".into(),
            exp: before,
            gp: 10_000,
            ..LobbyProfile::default()
        },
        result: Some(result.clone()),
        ..MenuData::default()
    };
    let screen = ResultScreen::new(&cat, &data);
    assert!(screen.level_up_shown(), "window 4 opens on a level change");
    assert_eq!(screen.sound, Some(LEVEL_UP_SOUND));
    let up = screen
        .text_of(WINDOW_LEVEL_UP, LEVEL_UP_TEXT)
        .unwrap_or_default();
    assert!(
        !up.is_empty() && !up.contains('%'),
        "level-up text filled: {up:?}"
    );
    let lv = screen
        .text_of(WINDOW_SUMMARY, SUMMARY_LEVEL)
        .unwrap_or_default();
    assert!(lv.ends_with(" %"), "level text {lv:?}");
    let mut cat2 = Catalog::from_rom(&rom, 1024, 768).expect("catalog");
    let mut items = Vec::new();
    screen.draw_extra(&mut cat2, &data, 0.0, &mut items);
    let mark = art.level_up_mark.clone().unwrap();
    let marks = items
        .iter()
        .filter(|i| matches!(i, DrawItem::Image { tex, .. } if *tex == mark))
        .count();
    assert_eq!(marks, 1, "one row levelled up");
    let bonus = argb(BONUS_COLOUR);
    let bonus_runs = items
        .iter()
        .filter(|i| matches!(i, DrawItem::Label { style, .. } if style.colour == bonus))
        .count();
    assert_eq!(bonus_runs, 1, "the experience bonus run in its colour");
    // no level change: no window, no mark
    let mut plain = result.clone();
    plain.rows[0].exp_gain = 0;
    plain.rows[0].exp_bonus = 0;
    let data_plain = MenuData {
        result: Some(plain),
        ..MenuData::default()
    };
    let s = ResultScreen::new(&cat, &data_plain);
    assert!(!s.level_up_shown());
    assert_eq!(s.sound, None);

    // the rank-up boxes of the push that follows
    let p = ProgressApplied {
        exp_before: before,
        exp: before + 5,
        gp: 10_012,
        popup: true,
    };
    let texts = rank_up_texts(&levels, &art, &p, |id| cat.text(id));
    assert_eq!(texts.len(), 2, "box 0x185f and notice 0x487");
    for t in &texts {
        assert!(!t.contains('%'), "filled: {t:?}");
    }
    assert_eq!(rank_up_boxes(&cat, &p).len(), 2);
    assert_eq!(
        rank_up_boxes(&cat, &ProgressApplied { popup: false, ..p }).len(),
        1
    );

    // the whole screen through the menu (local PNG)
    let mut menu = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "Player".into(),
        link: LinkOptions::new("w1-result"),
        rom: Some(rom.clone()),
        font: Some((dc_client::testkit::synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    menu.data.profile = data.profile.clone();
    menu.data.result = Some(result);
    menu.open(Go::Result);
    let img = menu.image();
    let path = out_dir().join("result_level_up.png");
    dc_render::FrameImage {
        width: img.width,
        height: img.height,
        rgba: img.rgba,
    }
    .save_png(&path)
    .unwrap();
    println!("png: {}", path.display());
}

/// CLI-1161: the result row's mission column is `captures/returns` in the flag classes (7, 8) and the first mission
/// value elsewhere; it was a fixed 0.
// EVIDENCE: cshell+0x130220 (the row builder's mission text: classes 7 and 8 take two values, else one)
#[test]
fn rom_result_row_mission_column_follows_the_mode_class() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let cat = Catalog::from_rom(&rom, 1024, 768).expect("catalog");
    let texts = |class: u8, mission: [u8; 3]| -> Vec<String> {
        let mut r = MatchResult::sample();
        r.mode_class = Some(class);
        r.rows[0].mission = mission;
        let data = MenuData {
            result: Some(r),
            ..MenuData::default()
        };
        let screen = ResultScreen::new(&cat, &data);
        let mut cat2 = Catalog::from_rom(&rom, 1024, 768).expect("catalog");
        let mut items = Vec::new();
        screen.draw_extra(&mut cat2, &data, 0.0, &mut items);
        items
            .iter()
            .filter_map(|i| match i {
                DrawItem::Label { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    };
    assert!(texts(8, [9, 2, 3]).iter().any(|t| t == "2/3"));
    assert!(!texts(1, [9, 2, 3]).iter().any(|t| t == "2/3"));
    assert!(texts(1, [9, 2, 3]).iter().any(|t| t == "9"));
}
