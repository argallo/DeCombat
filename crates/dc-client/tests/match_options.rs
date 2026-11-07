//! CLI-1907 / CLI-1908: the options over a match (the exit box's Options button): the lobby's options box on a menu
//! of its own (no lobby connection), a fifth tab Mods that lists the match's mods with settings and opens the Mods
//! screen's settings page for one, a server that refuses the player's own mods saying so, OK handing the settings to
//! the match and the menu saving them after it; a changed mod value loads the match's mod set again with it. The
//! lobby's options keep their four tabs. Headless; with `CA_ROM` the boxes use the ROM layouts (nothing of the ROM in
//! this file). The mod is the repository's example `drawn_crosshair`.

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::LinkOptions;
use dc_client::menu::modsettings::{CMD_PAGE_STEP_BASE, T_APPLIES_NOW};
use dc_client::menu::optionsmods::{
    MatchModsInfo, CMD_SETTINGS_BASE, CMD_TAB_MODS, T_GRAPHIC_NOTE, T_LIVE, T_NO_LOCAL, T_TAB,
    T_TITLE, T_VANILLA,
};
use dc_client::menu::{Go, Menu, MenuInput, MenuKey, MenuOptions, CMD_BAR_PREFS};
use dc_client::mods::{HostRecipe, RecipeMod};
use dc_client::settings::{CommandTable, ProfileStore, Settings};
use dc_client::testkit::synth_font;
use dc_mod_lua::{ModHost, ModOrigin, ModPolicy};
use std::path::{Path, PathBuf};

const ALLOWED: ModPolicy = ModPolicy::Modded {
    client_visual_allowed: true,
};

fn crosshair_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods/drawn_crosshair")
}

fn crosshair_info(policy: ModPolicy) -> MatchModsInfo {
    let mut host = ModHost::new();
    host.set_policy(ALLOWED);
    host.load_mod_dir_as(ModOrigin::Local, &crosshair_dir())
        .expect("the example crosshair mod loads");
    MatchModsInfo::of_host(Some(&host), Some(policy))
}

fn roms() -> Vec<Option<PathBuf>> {
    let mut v = vec![None];
    v.extend(std::env::var_os("CA_ROM").map(|r| Some(PathBuf::from(r))));
    v
}

fn open(rom: Option<&Path>, settings: Settings, info: MatchModsInfo) -> Menu {
    Menu::over_match(
        rom,
        Some((synth_font(), 0)),
        (1024, 768),
        settings,
        CommandTable::default(),
        info,
    )
}

fn labels(m: &mut Menu) -> Vec<String> {
    m.items()
        .into_iter()
        .filter_map(|i| match i {
            DrawItem::Label { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

fn has(t: &[String], want: &str) -> bool {
    t.iter().any(|x| x == want)
}

/// The Graphic tab's note over a match (on one or two lines, as the font measures it).
fn has_note(t: &[String]) -> bool {
    let first = T_GRAPHIC_NOTE.split(',').next().unwrap();
    t.iter().any(|x| x.starts_with(first))
}

#[test]
fn the_mods_tab_lists_the_match_mods_and_opens_their_settings_page() {
    for rom in roms() {
        let info = crosshair_info(ALLOWED);
        assert_eq!(info.mods.len(), 1, "the crosshair declares settings");
        let mut m = open(rom.as_deref(), Settings::default(), info.clone());
        assert!(m.over_match_open());
        assert_eq!(m.screen_name(), "options");
        let t = labels(&mut m);
        assert!(has(&t, T_TAB), "the fifth tab: {t:?}");
        assert!(has_note(&t), "the Graphic tab's note over a match: {t:?}");
        m.input(MenuInput::Command(CMD_TAB_MODS));
        let t = labels(&mut m);
        for want in [T_TITLE, T_LIVE, "Custom Crosshair", "Settings"] {
            assert!(has(&t, want), "{rom:?}: missing {want:?} in {t:?}");
        }
        assert!(!has_note(&t));
        // the Mods screen's page, with the in-match note; a step changes the settings at once (no OK)
        m.input(MenuInput::Command(CMD_SETTINGS_BASE));
        let t = labels(&mut m);
        for want in [T_APPLIES_NOW, "Shape", "Style", "Circle"] {
            assert!(has(&t, want), "{rom:?}: missing {want:?} in {t:?}");
        }
        assert!(
            m.items()
                .iter()
                .any(|i| matches!(i, DrawItem::Crosshair(_))),
            "the preview"
        );
        assert!(m.data.settings.mods_settings.is_empty());
        m.input(MenuInput::Command(CMD_PAGE_STEP_BASE));
        let values = m
            .data
            .settings
            .mods_settings
            .get("drawn_crosshair")
            .cloned();
        assert_eq!(
            values
                .as_ref()
                .and_then(|v| v.get("style"))
                .map(String::as_str),
            Some("dot"),
            "{rom:?}: the style row stepped from circle to dot"
        );
        // Escape closes the page first, then the box; Cancel keeps the mod value (applied at once)
        m.input(MenuInput::Key(MenuKey::Escape));
        assert!(m.over_match_open());
        assert!(has(&labels(&mut m), T_TITLE), "back on the Mods tab");
        m.input(MenuInput::Key(MenuKey::Escape));
        assert!(!m.over_match_open(), "Escape closes the box");
        assert_eq!(
            m.data.settings.mods_settings.get("drawn_crosshair"),
            values.as_ref()
        );
        // a second Options click opens the box again on its first tab
        m.reopen_over_match(None, info);
        assert!(m.over_match_open());
        assert!(has_note(&labels(&mut m)));
    }
}

#[test]
fn a_vanilla_match_and_a_server_without_own_mods_say_so() {
    let mut m = open(None, Settings::default(), MatchModsInfo::default());
    m.input(MenuInput::Command(CMD_TAB_MODS));
    let t = labels(&mut m);
    assert!(has(&t, T_VANILLA), "{t:?}");
    assert!(!has(&t, "Settings"));
    let refused = ModPolicy::Curated {
        client_visual_allowed: false,
    };
    let mut m = open(
        None,
        Settings::default(),
        MatchModsInfo {
            policy: Some(refused),
            mods: Vec::new(),
        },
    );
    m.input(MenuInput::Command(CMD_TAB_MODS));
    assert!(has(&labels(&mut m), T_NO_LOCAL));
}

/// OK: the settings change is handed out (the match applies it) and the menu saves what the match noted.
#[test]
fn ok_hands_the_settings_over_and_the_menu_saves_them_after_the_match() {
    let start = Settings {
        sensitivity: 9,
        ..Settings::default()
    };
    let mut m = open(None, start.clone(), MatchModsInfo::default());
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(!m.over_match_open(), "OK closes the box");
    let s = m
        .take_settings_change()
        .expect("OK hands the settings over");
    assert_eq!(s.sensitivity, 9);

    // the lobby menu takes the note and saves it in the profile
    let dir = std::env::temp_dir().join(format!("dc-match-options-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("player.txt");
    let store = || ProfileStore {
        read: vec![file.clone()],
        write: Some(file.clone()),
        key: None,
    };
    let mut lobby = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: LinkOptions::new("match-options"),
        rom: None,
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    lobby.detach_link();
    lobby.load_settings(store());
    let mut changed = lobby.data.settings.clone();
    changed.sensitivity = 13;
    changed
        .mods_settings
        .entry("drawn_crosshair".into())
        .or_default()
        .insert("style".into(), "dot".into());
    dc_client::settings::note_match_settings(changed);
    lobby.store_match_settings();
    assert_eq!(lobby.data.settings.sensitivity, 13);
    assert!(
        lobby.take_settings_change().is_some(),
        "the next match starts with them"
    );
    let mut again = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: LinkOptions::new("match-options-2"),
        rom: None,
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    again.detach_link();
    again.load_settings(store());
    assert_eq!(again.data.settings.sensitivity, 13, "saved in the profile");
    assert_eq!(
        again.data.settings.mods_settings["drawn_crosshair"]["style"],
        "dot"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// CLI-1908: the match's mod set loaded again with a changed value draws the new crosshair (the path the window's
/// reload takes: the recipe with the new values).
#[test]
fn a_changed_value_reloads_the_match_mods_with_it() {
    let recipe = |style: &str| HostRecipe {
        policy: ALLOWED,
        mods: vec![RecipeMod {
            dir: crosshair_dir(),
            origin: ModOrigin::Local,
            id: "drawn_crosshair".into(),
            version: "1.1.0".into(),
            kind: "client_visual".into(),
        }],
        settings: [(
            "drawn_crosshair".to_string(),
            [("style".to_string(), style.to_string())]
                .into_iter()
                .collect(),
        )]
        .into_iter()
        .collect(),
    };
    let style = |s: &str| {
        let m = dc_client::modassets::MatchMods::open(Some(&recipe(s))).expect("the set loads");
        m.overrides.crosshair_style.map(|x| x.name().to_string())
    };
    assert_eq!(style("circle").as_deref(), Some("circle"));
    assert_eq!(style("cross").as_deref(), Some("cross"));
}

/// The lobby's options keep the 2009 four tabs: no Mods tab, its command does nothing.
#[test]
fn the_lobby_options_have_no_mods_tab() {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: LinkOptions::new("match-options-lobby"),
        rom: std::env::var_os("CA_ROM").map(PathBuf::from),
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.detach_link();
    m.open(Go::Lobby);
    m.input(MenuInput::Command(CMD_BAR_PREFS));
    assert_eq!(m.screen_name(), "options");
    m.input(MenuInput::Command(CMD_TAB_MODS));
    let t = labels(&mut m);
    assert!(!has(&t, T_TITLE) && !has_note(&t), "{t:?}");
}
