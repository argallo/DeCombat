//! ROM-gated validation against the spec's counts (SPEC: FMT_CRES_UI.md §9). Skipped when `CA_ROM` is
//! unset. No ROM text appears here: ROM-derived lists are compared by count and SHA-256 only.

use crate::fonts::{file_has_face, FontContainer};
use crate::*;
use dc_rom::{rom_or_skip, sha256_hex};

/// SHA-256 of the ascending list of CRes string ids, each as u32 little-endian (computed from the user's
/// ROM on 2026-10-24; fingerprint in CLEANROOM.md).
const STRING_IDS_SHA256: &str = "491c37489d1e0f4d045665ea3f07382f61ed13ccd467404f065c9c0ffbd39101";

#[test]
fn rom_cres_strings_match_spec_counts() {
    let rom = rom_or_skip!();
    let s = UiStrings::from_rom(&rom).expect("CRes strings");
    // SPEC §2 / §9: 3 resource types, 162 blocks, 1745 strings, 62062 units, longest 312, 197 with '%'.
    assert_eq!(s.resource_types(), &[1, 6, 12]);
    assert_eq!(s.string_block_count(), 162);
    assert_eq!(s.len(), 1745);
    let mut ids = Vec::new();
    for id in s.ids() {
        ids.extend_from_slice(&id.to_le_bytes());
    }
    assert_eq!(sha256_hex(&ids), STRING_IDS_SHA256);
    let units: usize = s.iter().map(|(_, t)| t.encode_utf16().count()).sum();
    let longest = s
        .iter()
        .map(|(_, t)| t.encode_utf16().count())
        .max()
        .unwrap_or(0);
    assert_eq!((units, longest), (62062, 312));
    assert_eq!(s.iter().filter(|(_, t)| t.contains('%')).count(), 197);
    // One cursor image and one cursor group (group id 125), language 0x0409.
    assert_eq!(s.cursors().len(), 1);
    assert_eq!(s.cursor_groups().len(), 1);
    assert_eq!(s.cursor_groups()[0].id, 125);
    assert_eq!(s.cursors()[0].lang, 0x409);
    // A missing id is an empty string, never an error.
    assert_eq!(s.get_or_empty(0xffff_ff00), "");
}

#[test]
fn rom_names_resolve_against_pins() {
    let rom = rom_or_skip!();
    let n = UiNames::from_rom(&rom).expect("every pinned literal matches");
    assert_eq!(n.hud_set0, format!("{}0", n.hud_prefix));
    assert_eq!(n.screens[0].len(), 13);
    assert!(UiNames::split_table_path(&n.layout_path).is_some());
    assert!(UiNames::split_table_path(&n.pages_path).is_some());
    for t in &n.page_tags {
        assert!(
            PageTag::end_tag_of(t).is_some(),
            "every page tag ends in ]]"
        );
    }
}

#[test]
fn rom_layout_table_matches_spec_counts() {
    let rom = rom_or_skip!();
    let names = UiNames::from_rom(&rom).expect("names");
    let vfs = dc_rez::RezVfs::new(&rom);
    let t = LayoutTable::load(&vfs, names).expect("layout table");
    // SPEC §6.2 / §9: 159 sections, 3275 key lines.
    assert_eq!(t.section_count(), 159);
    assert_eq!(t.key_count(), 3275);
    // 21 of the 30 screen table names have a section; the 9 others use the section-0 fallback.
    let with_section: Vec<usize> = UiScreen::ALL
        .iter()
        .filter(|s| t.screen_section(**s).is_some())
        .map(|s| s.index())
        .collect();
    assert_eq!(with_section.len(), 21);
    for missing in [1, 16, 17, 18, 20, 23, 27, 28, 29] {
        assert!(!with_section.contains(&missing));
    }
    // Section 0 exists and a screen without a section resolves every fallback key through it.
    assert!(t.screen_section(UiScreen::Generic).is_some());
    for key in [
        ScreenKey::TitleFont,
        ScreenKey::TitleSize,
        ScreenKey::FontIndex,
        ScreenKey::FontSize,
        ScreenKey::ItemSpacing,
        ScreenKey::ItemAlignment,
        ScreenKey::MusicIntensity,
    ] {
        assert_eq!(
            t.screen_int(UiScreen::Screen1, key),
            t.screen_int(UiScreen::Generic, key)
        );
    }
    // Numbered sections: 1 HUD set, 27 font records, 28 lights, 4 interface FX, 0 transitions.
    assert_eq!(t.hud_set_count(), 1);
    assert_eq!(t.font_record_count(), 27);
    assert_eq!(t.lights().len(), 28);
    assert_eq!(t.interface_fx().len(), 4);
    assert_eq!(t.transition_count(), 0);
    // HUD set 0 has 944 keys, the base HUD section 4 (the four pinned position keys).
    assert_eq!(t.hud_set(0).map(|s| s.len()), Some(944));
    assert_eq!(t.hud_base().map(|s| s.len()), Some(4));
    for i in 0..4 {
        assert!(t.hud_base_pos(i).is_some(), "HUD base position {i}");
    }
    // Every HUD set index resolves to set 0 through the fallback.
    let (k0, _) = t.hud_set(0).unwrap().iter().next().unwrap();
    assert!(t.hud_raw(5, k0).is_some());
    assert_eq!(t.hud_raw(5, k0), t.hud_raw(0, k0));
    // Base constants: 15 keys, the font indices exist, the background colour is a 24-bit value.
    assert_eq!(t.base().map(|s| s.len()), Some(15));
    assert!(t.help_font_index().is_some() && t.hud_font_index().is_some());
    assert!(t.background_colour() <= 0x00ff_ffff);
    // All named popup / mode HUD / constant sections exist.
    for s in UiSection::ALL {
        assert!(t.named(s).is_some(), "{s:?}");
    }
    // Font records: 27, bold in 17, 11 name a file, heights 14..=56 with 9 distinct values.
    let recs = t.font_records();
    assert_eq!(recs.len(), 27);
    assert_eq!(recs.iter().filter(|r| r.bold).count(), 17);
    assert_eq!(recs.iter().filter(|r| !r.file.is_empty()).count(), 11);
    let mut heights: Vec<i32> = recs.iter().map(|r| r.height).collect();
    heights.sort();
    heights.dedup();
    assert_eq!(heights.len(), 9);
    assert!(heights.iter().all(|h| (14..=56).contains(h)));
}

#[test]
fn rom_screen_pages_match_spec_counts() {
    let rom = rom_or_skip!();
    let names = UiNames::from_rom(&rom).expect("names");
    let vfs = dc_rez::RezVfs::new(&rom);
    let p = ScreenPages::load(&vfs, &names).expect("screen pages");
    // SPEC §9: 20 pages matched to the 20 tags; 575 records by kind; all schema-conformant.
    assert_eq!(p.pages.len(), 20);
    let mut tags: Vec<usize> = p.pages.iter().map(|pg| pg.tag.index()).collect();
    tags.sort();
    tags.dedup();
    assert_eq!(tags.len(), 20);
    assert_eq!(p.kind_counts(), [52, 96, 163, 201, 30, 4, 7, 20, 1, 1]);
    assert_eq!(p.widgets().count(), 575);
    assert_eq!(p.widgets().filter(|w| w.conformant).count(), 575);
    // OBSERVED (2026-10-24, this test): exactly one record has a line the 2009 tokeniser does not split
    // (no space after '='); the client keeps that field's initial value.
    let token_issues = p
        .widgets()
        .filter(|w| w.issues.iter().any(|i| i.kind == IssueKind::Tokens))
        .count();
    assert_eq!(token_issues, 1);
    // 4 framed buttons, 4 lists with a back image.
    let framed = p
        .widgets()
        .filter(|w| matches!(&w.widget, Widget::Button(b) if b.frame.is_some()))
        .count();
    let backed = p
        .widgets()
        .filter(|w| matches!(&w.widget, Widget::List(l) if l.back_texture.is_some()))
        .count();
    assert_eq!((framed, backed), (4, 4));
    // font_index of buttons and text widgets: 0..=25, all below the 27 system font records.
    let idx: Vec<i32> = p
        .widgets()
        .filter_map(|w| match &w.widget {
            Widget::Button(b) => Some(b.font_index),
            Widget::Text(t) => Some(t.font_index),
            _ => None,
        })
        .collect();
    assert_eq!(idx.iter().min(), Some(&0));
    assert_eq!(idx.iter().max(), Some(&25));
    assert!(idx.iter().all(|i| *i < 27));
    // 127 distinct command ids over buttons, text widgets and check buttons.
    let mut cmds: Vec<i32> = p
        .widgets()
        .filter_map(|w| match &w.widget {
            Widget::Button(b) => Some(b.command_id),
            Widget::Text(t) => Some(t.command_id),
            Widget::CheckButton(c) => Some(c.command_id),
            _ => None,
        })
        .collect();
    cmds.sort();
    cmds.dedup();
    assert_eq!(cmds.len(), 127);
}

#[test]
fn rom_fonts_match_spec_counts() {
    let rom = rom_or_skip!();
    let names = UiNames::from_rom(&rom).expect("names");
    let vfs = dc_rez::RezVfs::new(&rom);
    let t = LayoutTable::load(&vfs, names).expect("layout table");
    let fonts = SystemFonts::from_layout(&t);
    assert_eq!(fonts.len(), 27);
    let r = FontResolver::open(&rom).expect("font archive");
    // SPEC §7.3: 19 files, 3 collections, 24 faces.
    let inv = r.inventory().expect("inventory");
    assert_eq!(inv.len(), 19);
    assert_eq!(
        inv.iter()
            .filter(|(_, i)| i.container == FontContainer::Collection)
            .count(),
        3
    );
    assert_eq!(
        inv.iter().map(|(_, i)| i.face_offsets.len()).sum::<usize>(),
        24
    );
    // SPEC §7.1: 11 records with a file, 9 present in the archive, 7 whose face is named in the file;
    // SPEC §7.3: those 9 resolve to 5 distinct files.
    let with_file: Vec<&FontSpec> = fonts
        .iter()
        .flatten()
        .filter(|s| s.file.is_some())
        .collect();
    assert_eq!(with_file.len(), 11);
    let resolved: Vec<(&FontSpec, ResolvedFont)> = with_file
        .iter()
        .filter_map(|s| r.resolve(s).map(|x| (*s, x)))
        .collect();
    assert_eq!(resolved.len(), 9);
    let mut files: Vec<&str> = resolved.iter().map(|(_, x)| x.entry.as_str()).collect();
    files.sort();
    files.dedup();
    assert_eq!(files.len(), 5);
    let face_found = resolved
        .iter()
        .filter(|(s, x)| file_has_face(&r.read(x).expect("font bytes"), &s.face))
        .count();
    assert_eq!(face_found, 7);
}

#[test]
fn rom_ui_data_loads_in_one_call() {
    let rom = rom_or_skip!();
    let ui = UiData::load(&rom).expect("UiData::load");
    assert_eq!(ui.strings.len(), 1745);
    assert_eq!(ui.layout.section_count(), 159);
    assert_eq!(ui.pages.pages.len(), 20);
    assert_eq!(ui.fonts.len(), 27);
}

#[test]
fn rom_hud_keys_resolve_in_hud_set_0_with_their_forms() {
    use crate::hud::ValueKind;
    let rom = rom_or_skip!();
    let keys = HudKeys::from_rom(&rom).expect("every HUD key literal matches its pin");
    let names = UiNames::from_rom(&rom).expect("names");
    let layout = LayoutTable::load(&dc_rez::RezVfs::new(&rom), names).expect("layout");
    let v = HudView::new(&layout, &keys, 0);
    let mut missing = Vec::new();
    for k in HudKey::all() {
        let ok = match k.kind() {
            ValueKind::Text => v.raw(k).is_some(),
            ValueKind::Pair => v.pair(k).is_some(),
            ValueKind::Int => v.int(k).is_some(),
            ValueKind::Colour => v.colour(k).is_some(),
            ValueKind::Real => v.real(k).is_some(),
        };
        // the team-mate health alpha is read with a default of 1.0 and the 2009 ROM has no such key
        // EVIDENCE: cshell+0x586d0 (alpha getter called with 0x3f800000)
        if !ok && k != HudKey::MateHealthAlpha {
            missing.push(k);
        }
    }
    assert!(
        missing.is_empty(),
        "keys absent or of another form: {missing:?}"
    );
    // the round-clock digit form has exactly one integer conversion
    assert_eq!(
        keys.literal(HudLiteral::ClockDigits).matches("%d").count(),
        1
    );
    // the respawn number texture is a printf form too (one integer conversion)
    assert_eq!(
        v.text(HudKey::RespawnNumber)
            .map(|s| s.matches("%d").count()),
        Some(1)
    );
}

/// The activate-type table reads through its pins; the first row's on / off texts are CRes strings, and both are the
/// two ids the door prompt element draws for (cshell+0x82b34: 0x1fd / 0x208).
#[test]
fn rom_activate_types_resolve() {
    let rom = rom_or_skip!();
    let t = crate::activate::ActivateTypes::from_rom(&rom).expect("activate table");
    assert!(t.rows.len() >= 2, "rows {}", t.rows.len());
    let s = UiStrings::from_rom(&rom).expect("CRes strings");
    let on = t.text_id(0, crate::activate::STATE_ON).expect("row 0 on");
    let off = t.text_id(0, crate::activate::STATE_OFF).expect("row 0 off");
    assert!(!s.get_or_empty(on).is_empty() && !s.get_or_empty(off).is_empty());
    let mut ids = [on, off];
    ids.sort();
    assert_eq!(ids, [0x1fd, 0x208]);
}
