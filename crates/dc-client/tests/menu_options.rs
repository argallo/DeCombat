//! The top bar's option, report and help buttons (w3-options): each opens its box over the screen it was pressed on
//! (the lobby here), the box closes again, the options' OK saves the profile and changes the settings in effect, the
//! report box refuses empty entries and sends one report, and a key box takes the next key. Headless: the menu is
//! driven by inputs; with `CA_ROM` the screens use the ROM layouts and the buttons are pressed by their rectangles,
//! without it by their command numbers. Nothing of the ROM is in this file.

use dc_botclient::lobby::LobbyProfile;
use dc_client::menu::help;
use dc_client::menu::link::{LinkOptions, UserRow};
use dc_client::menu::options::{CMD_CANCEL, CMD_OK, CMD_TABS, ROW_CMD_BASE};
use dc_client::menu::report;
use dc_client::menu::{
    Go, Menu, MenuInput, MenuKey, MenuOptions, CMD_BAR_ABUSE, CMD_BAR_ASSIST, CMD_BAR_PREFS,
};
use dc_client::settings::{ProfileStore, Settings};
use dc_client::sound::SoundCue;
use dc_client::testkit::synth_font;
use dc_ui_data::WidgetKind;
use std::path::PathBuf;

fn menu() -> (Menu, PathBuf) {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: LinkOptions::new("menu-options"),
        rom: dc_rom::resolve_rom().ok(),
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.detach_link();
    m.data.profile = LobbyProfile {
        nick: "tester".into(),
        char_no: 1,
        my_id: 1,
        chan_label: "Channel 1".into(),
        ..LobbyProfile::default()
    };
    m.data.users = vec![
        UserRow {
            account: 1,
            id: 1,
            name: "tester".into(),
            exp: 0,
            ..Default::default()
        },
        UserRow {
            account: 2,
            id: 2,
            name: "Bot1".into(),
            exp: 0,
            ..Default::default()
        },
    ];
    let dir = std::env::temp_dir().join(format!(
        "dc-menu-options-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("player.txt");
    // the command table comes from the ROM here, as it does at start-up
    m.load_settings(ProfileStore {
        read: vec![file.clone()],
        write: Some(file.clone()),
        key: None,
    });
    m.open(Go::Lobby);
    (m, file)
}

/// Presses the button with `cmd` on the current screen: by its rectangle when the screen has one, else by command.
fn press(m: &mut Menu, cmd: i32) {
    let s = m.sheet();
    let at = (0..s.parts.len()).find(|&i| {
        s.parts[i].command == cmd && s.parts[i].kind == WidgetKind::Button && s.visible(i)
    });
    match at {
        Some(i) => {
            let r = m.sheet().parts[i].hit.unwrap_or_else(|| {
                let p = &m.sheet().parts[i];
                [p.pos[0], p.pos[1], p.size[0], p.size[1]]
            });
            let c = [(r[0] + r[2] / 2) as f32, (r[1] + r[3] / 2) as f32];
            m.input(MenuInput::Move(c));
            m.input(MenuInput::Press(c));
            m.input(MenuInput::Release(c));
        }
        None => m.input(MenuInput::Command(cmd)),
    }
}

#[test]
fn the_top_bar_buttons_open_their_boxes_over_the_lobby_and_close_again() {
    // needs the ROM's screen layouts (or body models): CI has none
    if dc_rom::resolve_rom().is_err() {
        println!("skipped: CA_ROM unset");
        return;
    }
    let (mut m, _) = menu();
    assert_eq!(m.screen_name(), "lobby");
    for (cmd, name, close) in [
        (CMD_BAR_PREFS, "options", CMD_CANCEL),
        (CMD_BAR_ABUSE, "report", report::CMD_CANCEL),
        (CMD_BAR_ASSIST, "help", help::CMD_DISMISS),
    ] {
        press(&mut m, cmd);
        assert_eq!(m.screen_name(), name, "button {cmd} opens {name}");
        assert!(m.overlay_open());
        assert_eq!(m.base_screen_name(), "lobby");
        assert!(
            m.take_sound_cues()
                .iter()
                .any(|c| matches!(c, SoundCue::Interface { .. })),
            "the click cue of the interface"
        );
        press(&mut m, close);
        assert_eq!(m.screen_name(), "lobby", "{name} closes");
        assert!(!m.overlay_open());
    }
    // Escape closes a box too; the help button pressed again toggles its box away
    m.input(MenuInput::Command(CMD_BAR_ASSIST));
    assert_eq!(m.screen_name(), "help");
    m.input(MenuInput::Command(CMD_BAR_ASSIST));
    assert_eq!(m.screen_name(), "lobby");
    m.input(MenuInput::Command(CMD_BAR_ABUSE));
    m.input(MenuInput::Key(MenuKey::Escape));
    assert_eq!(m.screen_name(), "lobby");
}

#[test]
fn ok_keeps_the_options_and_cancel_drops_them() {
    let (mut m, file) = menu();
    let before: Settings = m.data.settings.clone();
    // every minus and every first choice of the sound tab, then Cancel: nothing changes, nothing is written
    press(&mut m, CMD_BAR_PREFS);
    m.input(MenuInput::Command(CMD_TABS[1]));
    for r in 0..12 {
        m.input(MenuInput::Command(ROW_CMD_BASE + 16 * r));
    }
    m.input(MenuInput::Command(CMD_CANCEL));
    assert_eq!(m.screen_name(), "lobby");
    assert_eq!(m.data.settings, before);
    assert!(m.take_settings_change().is_none());
    assert!(!file.exists());
    // the same, then OK
    press(&mut m, CMD_BAR_PREFS);
    m.input(MenuInput::Command(CMD_TABS[1]));
    for r in 0..12 {
        m.input(MenuInput::Command(ROW_CMD_BASE + 16 * r));
    }
    press(&mut m, CMD_OK);
    assert_eq!(m.screen_name(), "lobby");
    let after = m.take_settings_change().expect("OK reports a change");
    if m.cat.rom.is_some() {
        assert!(
            after.effects_volume < 100 || after.music_volume < 100,
            "a minus lowered a volume: {after:?}"
        );
    }
    assert_eq!(m.data.settings, after);
    // the profile was written and reads back as the same options
    assert!(file.exists(), "the profile is saved");
    let (doc, _) = m
        .data
        .profile_store
        .as_ref()
        .unwrap()
        .load()
        .expect("readable");
    let back = Settings::from_doc(&doc, &m.data.commands, &|id| m.cat.text(id));
    assert_eq!(back.effects_volume, after.effects_volume);
    assert_eq!(back.music_volume, after.music_volume);
    assert_eq!(back.effects_on, after.effects_on);
    assert_eq!(back.music_on, after.music_on);
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

#[test]
fn a_key_box_takes_the_next_key_and_ok_publishes_it() {
    let (mut m, file) = menu();
    if m.cat.rom.is_none() {
        println!("skipped: the control page's key boxes need the ROM layout");
        return;
    }
    press(&mut m, CMD_BAR_PREFS);
    m.input(MenuInput::Command(CMD_TABS[2]));
    // the first key box of the page waits for a key
    let mut waiting = false;
    for r in 0..40 {
        m.input(MenuInput::Command(ROW_CMD_BASE + 16 * r));
        if m.wants_scan() {
            waiting = true;
            break;
        }
    }
    assert!(waiting, "a key box waits for a key");
    m.input(MenuInput::Scan(0x58));
    assert!(!m.wants_scan(), "the key was taken");
    press(&mut m, CMD_OK);
    let bound = m
        .data
        .settings
        .binds
        .keys
        .values()
        .filter(|k| **k == Some(0x58))
        .count();
    assert_eq!(bound, 1, "F12 is bound to exactly one command");
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

#[test]
fn the_report_refuses_empty_entries_and_sends_one_report() {
    let (mut m, file) = menu();
    press(&mut m, CMD_BAR_ABUSE);
    assert_eq!(m.screen_name(), "report");
    // nothing typed: the box stays with a notice
    let sent = m.link_sent();
    press(&mut m, report::CMD_SEND);
    assert_eq!(m.screen_name(), "report");
    assert!(m.popup().is_some(), "the refusal is a box");
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(m.popup().is_none());
    // the find button names a channel user, then the description
    press(&mut m, report::CMD_FIND);
    // r10a: the find list's first row names the user (the list opens first since the report-box lists)
    if let Some(at) = m.dropdown().and_then(|d| d.row_point(0)) {
        m.input(MenuInput::Move(at));
        m.input(MenuInput::Press(at));
        m.input(MenuInput::Release(at));
    }
    assert!(m.data.last_report.is_none());
    press(&mut m, report::CMD_SEND);
    assert_eq!(m.screen_name(), "report", "the description is still empty");
    m.input(MenuInput::Key(MenuKey::Enter));
    m.input(MenuInput::Key(MenuKey::Tab));
    for c in "uses a wall hack".chars() {
        m.input(MenuInput::Char(c));
    }
    press(&mut m, report::CMD_SEND);
    assert_eq!(
        m.screen_name(),
        "lobby",
        "the report went and the box closed"
    );
    assert!(m.data.last_report.is_some());
    assert!(m.popup().is_some(), "the sent notice");
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(m.popup().is_none());
    let _ = sent;
    // a second report right away only gets the notice
    press(&mut m, CMD_BAR_ABUSE);
    assert_eq!(m.screen_name(), "lobby");
    assert!(m.popup().is_some(), "the send-again notice");
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

/// The options box keeps the screen it was opened on drawn under it (as the report and help boxes do): the lobby's
/// pictures stay in the display list, before the box's own.
#[test]
fn the_options_box_draws_the_lobby_under_it() {
    use dc_client::hud::items::DrawItem;
    let (mut m, _) = menu();
    let texes = |items: &[DrawItem]| -> Vec<String> {
        items
            .iter()
            .filter_map(|i| match i {
                DrawItem::Image { tex, .. } => Some(tex.clone()),
                _ => None,
            })
            .collect()
    };
    let lobby = texes(&m.items_at(0.0));
    if lobby.is_empty() {
        return; // no ROM: the screens draw no pictures
    }
    m.input(MenuInput::Command(CMD_BAR_PREFS));
    assert_eq!(m.screen_name(), "options");
    let shown = texes(&m.items_at(0.0));
    assert_eq!(
        shown.first(),
        lobby.first(),
        "the lobby's first picture comes first under the options box"
    );
    let kept = lobby.iter().filter(|t| shown.contains(t)).count();
    assert!(
        kept * 10 >= lobby.len() * 9,
        "the lobby's pictures stay drawn: {kept} of {}",
        lobby.len()
    );
}

/// The lobby bar's place text shows "<server> - <channel>" on the lobby screens (the lobby and the shop here).
#[test]
fn the_lobby_bar_names_the_server_and_the_channel() {
    use dc_botclient::lobby::ServerEntry;
    use dc_client::hud::items::DrawItem;
    let (mut m, _) = menu();
    if m.sheet().parts.is_empty() {
        return; // no ROM
    }
    m.data.server = Some(ServerEntry {
        id: 1,
        name: "DeCombat".into(),
        ip: "127.0.0.1".into(),
        port: 1,
        open: 1,
        capacity: 100,
        current: 1,
        clan_only: 0,
        op_capacity: 0,
        op_current: 0,
    });
    let shows = |m: &mut Menu| {
        m.items_at(0.0)
            .iter()
            .any(|i| matches!(i, DrawItem::Label { text, .. } if text == "DeCombat - Channel 1"))
    };
    assert!(shows(&mut m), "the lobby's bar names the place");
    m.open(Go::Shop);
    assert!(shows(&mut m), "the shop's bar names the place");
}

/// The graphic tab (quality = user control) shows its full list: the list's back image (the rows' labels and
/// section headers) moves with the rows, and the list has its scroll bar.
#[test]
fn the_graphic_tab_draws_its_labels_and_scrolls_its_full_list() {
    use dc_client::hud::items::DrawItem;
    use dc_client::menu::options::{LIST_FULL, PAGE_WINDOW, TAB_PAGES};
    use dc_client::menu::sheet::Look;
    use dc_ui_data::Widget;
    let (mut m, _) = menu();
    let back = m.cat.pages.page(TAB_PAGES[0]).and_then(|pg| {
        pg.widgets.iter().find_map(|w| match &w.widget {
            Widget::List(l) if l.control_id == LIST_FULL => l.back_texture.clone(),
            _ => None,
        })
    });
    let Some(back) = back else {
        return; // no ROM
    };
    m.input(MenuInput::Command(CMD_BAR_PREFS));
    assert_eq!(m.screen_name(), "options");
    let back_at = |m: &mut Menu| {
        m.items_at(0.0).iter().find_map(|i| match i {
            DrawItem::Image { tex, .. } if tex.starts_with(&format!("{back}#")) => {
                Some(tex.clone())
            }
            _ => None,
        })
    };
    let top = back_at(&mut m).expect("the list's back image is drawn");
    let s = m.sheet();
    let list = s
        .find(PAGE_WINDOW, LIST_FULL, WidgetKind::List)
        .expect("the full list");
    assert!(s.visible(list), "the full list is shown for user control");
    let at = match &s.parts[list].look {
        Look::List { bar: Some(b), .. } => {
            assert!(b.top() > 0, "the rows are higher than the list");
            b.rect
        }
        _ => panic!("the full list has a scroll bar"),
    };
    // a wheel turn over the list moves the rows and the back image with them
    m.input(MenuInput::Wheel(
        [(at[0] - 50) as f32, (at[1] + 50) as f32],
        -2,
    ));
    let moved = back_at(&mut m).expect("still drawn");
    assert_ne!(top, moved, "the back image follows the rows");
}

/// No page of the options box shows the page window's reset button.
#[test]
fn no_options_page_shows_the_reset_button() {
    use dc_client::menu::options::CMD_RESET;
    let (mut m, _) = menu();
    if m.sheet().parts.is_empty() {
        return; // no ROM
    }
    m.input(MenuInput::Command(CMD_BAR_PREFS));
    for (t, tab) in CMD_TABS.iter().enumerate() {
        if t > 0 {
            press(&mut m, *tab);
        }
        let s = m.sheet();
        assert!(
            !(0..s.parts.len()).any(|i| s.parts[i].command == CMD_RESET
                && s.parts[i].kind == WidgetKind::Button
                && s.visible(i)),
            "tab {t} shows no reset button"
        );
    }
}

/// The control tab draws no key labels of its own (they are part of the list's back image) and names the
/// modifier keys as the system does ("Ctrl", not "L-Ctrl").
#[test]
fn the_control_tab_draws_key_names_only() {
    use dc_client::hud::items::DrawItem;
    let (mut m, _) = menu();
    if m.sheet().parts.is_empty() {
        return; // no ROM
    }
    m.input(MenuInput::Command(CMD_BAR_PREFS));
    press(&mut m, CMD_TABS[2]);
    let labels: Vec<String> = (0..4)
        .filter_map(|i| m.data.commands.row(i))
        .map(|r| m.cat.text(r.label))
        .filter(|t| !t.is_empty())
        .collect();
    let texts: Vec<String> = m
        .items_at(0.0)
        .iter()
        .filter_map(|i| match i {
            DrawItem::Label { text, .. } | DrawItem::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(!labels.is_empty(), "the command table has labels");
    for l in &labels {
        assert!(!texts.contains(l), "no drawn key label {l:?}");
    }
    assert!(texts.iter().any(|t| t == "Ctrl"), "crouch shows Ctrl");
    assert!(!texts.iter().any(|t| t.starts_with("L-")), "no L- names");
}

/// The quick enter tab shows three on / off pairs, each on "off" by default, a click moves the pair; the spectator
/// row stays hidden.
#[test]
fn the_other_tab_shows_three_pairs_on_off() {
    use dc_client::menu::options::{QUICK_FIRST, QUICK_PAIRS};
    let (mut m, _) = menu();
    if m.sheet().parts.is_empty() {
        return; // no ROM
    }
    m.input(MenuInput::Command(CMD_BAR_PREFS));
    press(&mut m, CMD_TABS[3]);
    let state = |m: &Menu| -> Vec<(i32, bool, bool)> {
        let s = m.sheet();
        (0..s.parts.len())
            .filter(|&i| {
                s.parts[i].kind == WidgetKind::CheckButton && s.parts[i].command >= QUICK_FIRST
            })
            .map(|i| (s.parts[i].command, s.visible(i), s.parts[i].checked))
            .collect()
    };
    let st = state(&m);
    let shown: Vec<_> = st.iter().filter(|x| x.1).collect();
    assert_eq!(
        shown.len() as i32,
        2 * QUICK_PAIRS,
        "three pairs shown: {st:?}"
    );
    for (cmd, _, checked) in &shown {
        assert_eq!(
            *checked,
            (cmd - QUICK_FIRST) % 2 == 1,
            "off by default: {cmd}"
        );
    }
    // the spectator words are hidden with their pair
    let s = m.sheet();
    assert!(
        !(0..s.parts.len()).any(|i| s.parts[i].kind == WidgetKind::Text
            && s.parts[i].layer == 2
            && (1..=3).contains(&s.parts[i].id)
            && s.visible(i))
    );
    // "on" of the first pair
    let at = {
        let s = m.sheet();
        let i = (0..s.parts.len())
            .find(|&i| {
                s.parts[i].command == QUICK_FIRST && s.parts[i].kind == WidgetKind::CheckButton
            })
            .unwrap();
        let p = &s.parts[i];
        [
            (p.pos[0] + p.size[0] / 2) as f32,
            (p.pos[1] + p.size[1] / 2) as f32,
        ]
    };
    m.input(MenuInput::Move(at));
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
    let st = state(&m);
    let pair: Vec<bool> = st
        .iter()
        .filter(|x| x.0 < QUICK_FIRST + 2)
        .map(|x| x.2)
        .collect();
    assert_eq!(pair, vec![true, false], "the first pair moved to on");
}

/// Clicks the choice button with `cmd` (the rows' on / off pairs) by its rectangle.
fn press_choice(m: &mut Menu, cmd: i32) {
    let s = m.sheet();
    let i = (0..s.parts.len())
        .find(|&i| {
            s.parts[i].command == cmd && s.parts[i].kind == WidgetKind::CheckButton && s.visible(i)
        })
        .expect("the choice button is drawn");
    let p = &m.sheet().parts[i];
    let c = [
        (p.pos[0] + p.size[0] / 2) as f32,
        (p.pos[1] + p.size[1] / 2) as f32,
    ];
    m.input(MenuInput::Move(c));
    m.input(MenuInput::Press(c));
    m.input(MenuInput::Release(c));
}

/// CLI-1030 (round 11: "turning off BGM does not stop the background music"): a sound row's change is heard at once,
/// before OK (cshell+0x151af0 sets the music / effects volume on every change), and Cancel restores the volumes in
/// effect.
#[test]
fn the_sound_rows_are_heard_at_once_and_cancel_restores_them() {
    let (mut m, file) = menu();
    if m.cat.rom.is_none() {
        println!("skipped: the sound page's rows need the ROM layout");
        return;
    }
    let mut d = dc_client::sound::SoundDirector::open(None);
    m.sound_frame(Some(&mut d));
    let music = |d: &dc_client::sound::SoundDirector| d.mixer().music_volume();
    let effects = |d: &dc_client::sound::SoundDirector| d.mixer().effects_volume();
    assert_eq!(music(&d), 100);
    press(&mut m, CMD_BAR_PREFS);
    m.input(MenuInput::Command(CMD_TABS[1]));
    // row 1 music on / off: the second button is off
    press_choice(&mut m, ROW_CMD_BASE + 16 + 1);
    m.sound_frame(Some(&mut d));
    assert_eq!(music(&d), 0, "music off silences the music before OK");
    press_choice(&mut m, ROW_CMD_BASE + 16);
    m.sound_frame(Some(&mut d));
    assert_eq!(music(&d), 100, "music on brings it back");
    // our BGM row (DC_DEFAULTS CLI-1599, user request 2026-10-31) is row 2, right after the music switch, so the
    // ROM rows below it move down one: row 3 the music volume's minus, row 5 the effects volume's minus, row 4
    // effects off
    press(&mut m, ROW_CMD_BASE + 48);
    press(&mut m, ROW_CMD_BASE + 80);
    m.sound_frame(Some(&mut d));
    assert_eq!((music(&d), effects(&d)), (95, 95));
    press_choice(&mut m, ROW_CMD_BASE + 64 + 1);
    m.sound_frame(Some(&mut d));
    assert_eq!(effects(&d), 0, "effects off silences the effects");
    // Cancel: the volumes in effect again
    press(&mut m, CMD_CANCEL);
    m.sound_frame(Some(&mut d));
    assert_eq!((music(&d), effects(&d)), (100, 100));
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

/// CLI-1031 (round 11: "mouse sensitivity does not work, nor its hotkeys in game"): a value the match's hotkeys set is
/// taken into the settings and the profile by the menu (the next match starts with it), and the look turns by the
/// value at once: the aim delta per mouse count follows the sensitivity.
#[test]
fn the_match_sensitivity_keys_change_the_look_and_the_saved_setting() {
    use dc_client::input::{DeviceInput, InputSource, KeyboardMouse};
    use dc_client::{LookParams, ViewAngles};
    let (mut m, file) = menu();
    let mut look = LookParams::DEFAULT;
    look.sensitivity = m.data.settings.sensitivity as f32;
    let mut k = KeyboardMouse::new(look);
    let turn = |k: &mut KeyboardMouse| {
        let mut v = ViewAngles::default();
        k.device(DeviceInput::MouseDelta(100.0, 0.0));
        k.frame(&mut v);
        v.yaw
    };
    let at = |s: f32| 100.0 * (s * look.scale_mul + look.scale_base);
    assert!((turn(&mut k) - at(5.0)).abs() < 1e-6);
    // `]` twice in the match: 7
    let v = dc_client::settings::step_sensitivity(look.sensitivity, 1);
    let v = dc_client::settings::step_sensitivity(v, 1);
    k.set_sensitivity(v);
    dc_client::settings::note_sensitivity(v as i32);
    assert!(
        (turn(&mut k) - at(7.0)).abs() < 1e-6,
        "the look follows at once"
    );
    m.update();
    assert_eq!(m.data.settings.sensitivity, 7);
    let s = m.take_settings_change().expect("the next match takes it");
    assert_eq!(s.sensitivity, 7);
    let (doc, _) = m
        .data
        .profile_store
        .as_ref()
        .unwrap()
        .load()
        .expect("saved");
    let back = Settings::from_doc(&doc, &m.data.commands, &|id| m.cat.text(id));
    assert_eq!(back.sensitivity, 7);
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

/// CLI-1032 (round 11: "hotkeys do nothing in game"): the Control tab's rebind flow: click the Fire row's key box,
/// press a key, OK: the bindings the match reads fire on that key, the key's old command loses it, the profile keeps
/// it.
#[test]
fn rebinding_fire_on_the_control_tab_moves_the_key_the_match_reads() {
    use dc_client::settings::Bound;
    let (mut m, file) = menu();
    if m.cat.rom.is_none() {
        println!("skipped: the control page's key boxes need the ROM layout");
        return;
    }
    let fire_row = m
        .data
        .commands
        .rows
        .iter()
        .position(|r| r.command == 8)
        .expect("the table has the fire command");
    assert_eq!(fire_row, 11, "the page's row 11 holds the fire box");
    press(&mut m, CMD_BAR_PREFS);
    m.input(MenuInput::Command(CMD_TABS[2]));
    // the page's row 11 (two boxes: table rows 11 and 12), its first box
    m.input(MenuInput::Command(ROW_CMD_BASE + 16 * 11));
    assert!(m.wants_scan(), "the fire box waits for a key");
    m.input(MenuInput::Scan(19)); // R, the reload key
    press(&mut m, CMD_OK);
    let b = &m.data.settings.binds;
    assert_eq!(b.lookup(19), Some(Bound::Fire), "R fires");
    assert_eq!(b.key_of(3), None, "reload lost R");
    let (doc, _) = m
        .data
        .profile_store
        .as_ref()
        .unwrap()
        .load()
        .expect("saved");
    let back = Settings::from_doc(&doc, &m.data.commands, &|id| m.cat.text(id));
    assert_eq!(back.binds.lookup(19), Some(Bound::Fire));
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

/// CLI-1852: the Graphic tab's Renderer row (Windows only) opens a Vulkan / DirectX 12 list; a pick is kept with OK
/// in the profile's own section, and applies at the next start (nothing of the run changes).
#[test]
fn the_renderer_row_saves_the_pick_for_the_next_start() {
    use dc_client::menu::options::LIST_RENDERER;
    use dc_client::settings::{Renderer, OWN_RENDERER};
    if dc_rom::resolve_rom().is_err() {
        println!("skipped: CA_ROM unset");
        return;
    }
    let (mut m, file) = menu();
    press(&mut m, CMD_BAR_PREFS);
    assert_eq!(m.screen_name(), "options");
    let mut found = false;
    for r in 0..16 {
        let cmd = ROW_CMD_BASE + 16 * r;
        m.input(MenuInput::Command(cmd));
        let Some(d) = m.dropdown() else { continue };
        if d.list != LIST_RENDERER {
            m.input(MenuInput::Command(cmd)); // a second click closes the list
            continue;
        }
        assert_eq!(d.items, vec!["Vulkan".to_string(), "DirectX 12".to_string()]);
        let at = d.row_point(0).unwrap(); // Vulkan: the opt-in (DirectX 12 is the default)
        m.input(MenuInput::Move(at));
        m.input(MenuInput::Press(at));
        m.input(MenuInput::Release(at));
        found = true;
        break;
    }
    assert_eq!(found, cfg!(windows), "the row exists on Windows only");
    if !found {
        return;
    }
    assert_eq!(m.data.settings.renderer, Renderer::Dx12, "not changed before OK");
    press(&mut m, CMD_OK);
    let after = m.take_settings_change().expect("OK reports a change");
    assert_eq!(after.renderer, Renderer::Vulkan);
    let (doc, _) = m.data.profile_store.as_ref().unwrap().load().expect("readable");
    assert_eq!(doc.own_get(OWN_RENDERER), Some("vulkan"));
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}
