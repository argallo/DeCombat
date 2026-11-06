//! Front-end effects the 2009 screens run besides their layouts: the black fade between some screens, the
//! interface sounds by id, and the waiting room's start countdown line (red, with its tick sound).
//!
//! The texture name, the sound key and the two countdown forms are literals of the user's CShell.dll, read at run
//! time by pin (rva + hash); nothing of them is in the tree.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use dc_ui_data::names::{matches_pin, read_pinned, Pin, CSHELL_REL_PATH};

use super::Go;
use crate::hud::items::DrawItem;

/// Length of each half of the screen fade (to black, then from black).
// DISASM: cshell+0x1ab35d (`cmp ecx, 0x258`: the phase lasts 600 ms), cshell+0x1ab365..0x1ab385 (alpha =
// elapsed x 255 / 600; the second phase draws 255 minus that)
pub const FADE_MS: u32 = 600;

/// The fader image (drawn over the whole screen, its alpha the fade's).
// EVIDENCE: cshell+0x1ac6e0 (the fader is built from this image at the screen size)
pub const FADER: Pin = Pin {
    what: "screen fader image",
    rva: 0x3eb120,
    sha16: "c0fc4369f971143d",
    len: 42,
};

/// Interface sound ids of the dispatcher (the id picks a key of the interface sound section).
// EVIDENCE: cshell+0x1b3960 (id -> key: 1 navigation change, 2 navigation select, 3 select item, 4 shop item equip,
// 0xc start count, 0xd level up, 0xe notice error, 0xf result win, 0x10 result lose, 0x12 messenger twinkle, 0x13
// select change (cshell+0x1bf0c0), 0x14 select (cshell+0x1bf040), 0x1b map loading, 0x1c game start, 0x1d game over), cshell+0x1b6470 (a request
// with an id not below the pending one plays it at once and clears it)
pub const SOUND_NAV_CHANGE: i32 = 1;
pub const SOUND_NAV_SELECT: i32 = 2;
pub const SOUND_SELECT_ITEM: i32 = 3;
pub const SOUND_SHOP_EQUIP: i32 = 4;
pub const SOUND_START_COUNT: i32 = 0xc;
pub const SOUND_LEVEL_UP: i32 = 0xd;
pub const SOUND_NOTICE_ERROR: i32 = 0xe;
pub const SOUND_RESULT_WIN: i32 = 0xf;
pub const SOUND_RESULT_LOSE: i32 = 0x10;
pub const SOUND_MESSENGER_TWINKLE: i32 = 0x12;
pub const SOUND_SELECT_CHANGE: i32 = 0x13;
pub const SOUND_SELECT: i32 = 0x14;
pub const SOUND_MAP_LOADING: i32 = 0x1b;
pub const SOUND_GAME_START: i32 = 0x1c;
pub const SOUND_GAME_OVER: i32 = 0x1d;

/// The start-count sound key.
// EVIDENCE: cshell+0x1b3960 (case 0xc pushes this literal before the lookup cshell+0x1b2c00)
pub const START_COUNT_KEY: Pin = Pin {
    what: "start count sound key",
    rva: 0x3ebad8,
    sha16: "bff4ac4e745bb0f8",
    len: 15,
};

const fn key_pin(what: &'static str, rva: u32, sha16: &'static str, len: usize) -> Pin {
    Pin {
        what,
        rva,
        sha16,
        len,
    }
}

/// The interface sound keys by dispatcher id, with the name the sound log gives the cue (`what`).
// EVIDENCE: cshell+0x1b3960 (each case pushes its key literal before the lookup cshell+0x1b2c00; cases 0x13 / 0x14
// read theirs through cshell+0x1bf0c0 / cshell+0x1bf040, which cache the file under the same section)
pub const SOUND_KEYS: [(i32, &str, Pin); 15] = [
    (
        SOUND_NAV_CHANGE,
        "nav_change",
        key_pin(
            "navigation change sound key",
            0x3ebbbc,
            "36d41b6f41a03ab3",
            21,
        ),
    ),
    (
        SOUND_NAV_SELECT,
        "nav_select",
        key_pin(
            "navigation select sound key",
            0x3ebba4,
            "8c40af4673c435e8",
            21,
        ),
    ),
    (
        SOUND_SELECT_ITEM,
        "select_item",
        key_pin("select item sound key", 0x3ebb94, "4bca22ad99da29f2", 15),
    ),
    (
        SOUND_SHOP_EQUIP,
        "item_equip",
        key_pin("shop equip sound key", 0x3ebb80, "4648030e9f780c55", 19),
    ),
    (SOUND_START_COUNT, "start_count", START_COUNT_KEY),
    (
        SOUND_LEVEL_UP,
        "level_up",
        key_pin("level up sound key", 0x3ebac8, "de36077ef1c96217", 12),
    ),
    (
        SOUND_NOTICE_ERROR,
        "error_box",
        key_pin("notice error sound key", 0x3ebab4, "77dbe70271cc8d64", 16),
    ),
    (
        SOUND_RESULT_WIN,
        "match_won",
        key_pin("result win sound key", 0x3ebaa4, "b50262817b2630da", 14),
    ),
    (
        SOUND_RESULT_LOSE,
        "match_lost",
        key_pin("result lose sound key", 0x3eba94, "2c323f789d3cc79d", 15),
    ),
    (
        SOUND_MESSENGER_TWINKLE,
        "messenger_twinkle",
        key_pin(
            "messenger twinkle sound key",
            0x3eba68,
            "df2cc531e83ba4d1",
            21,
        ),
    ),
    (
        SOUND_SELECT_CHANGE,
        "select_change",
        key_pin("select change sound key", 0x3ec9d4, "7fc22a459903209d", 17),
    ),
    (
        SOUND_SELECT,
        "select",
        key_pin("select sound key", 0x3ec9b4, "3d262b78746cab41", 11),
    ),
    (
        SOUND_MAP_LOADING,
        "map_loading",
        key_pin("map loading sound key", 0x3eba58, "19f04dcff5d21e73", 15),
    ),
    (
        SOUND_GAME_START,
        "game_start",
        key_pin("game start sound key", 0x3eba48, "88fe43839ec4baac", 14),
    ),
    (
        SOUND_GAME_OVER,
        "game_over",
        key_pin("game over sound key", 0x3eba38, "3d061c21b15a43d9", 13),
    ),
];

/// The sound log's name of interface sound `id` (`interface` for an id without a key here).
pub fn cue_name(id: i32) -> &'static str {
    SOUND_KEYS
        .iter()
        .find(|(i, _, _)| *i == id)
        .map_or("interface", |(_, n, _)| n)
}

/// The countdown line forms (`<string 0x57a> <n>` with the singular form for 1).
// EVIDENCE: cshell+0x15f2b0 (5 - count; 1 -> the first form, else the second; swprintf with string 0x57a and the
// number; nothing for 0)
pub const COUNTDOWN_FORMS: [Pin; 2] = [
    Pin {
        what: "countdown form, one second",
        rva: 0x3e5b64,
        sha16: "2a58d89fa8a375a4",
        len: 13,
    },
    Pin {
        what: "countdown form, seconds",
        rva: 0x3e5b44,
        sha16: "ce4bd6bab4548a55",
        len: 14,
    },
];
/// The countdown line's colour (opaque red) and its string.
// EVIDENCE: cshell+0x15f2b0 (the line goes to the chat with 0xffff0000; text 0x57a)
pub const COUNTDOWN_COLOUR: u32 = 0xffff_0000;
pub const COUNTDOWN_TEXT: i32 = 0x57a;
/// The lobby counts 0..4; the line shows 5 - count.
// SPEC: PROTOCOL_2009.md §7 (0x3044)
pub const COUNTDOWN_FROM: u32 = 5;

/// What the front end reads from the user's CShell.dll for these effects.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrontLits {
    pub fader: Option<String>,
    /// Interface sound files by id (the key's value in the client attribute file's interface section).
    pub sounds: BTreeMap<i32, String>,
    /// The two countdown forms (one second, seconds).
    pub countdown: [Option<String>; 2],
    /// The user's CShell.dll was read (a key without a file then plays nothing, as in 2009).
    pub rom: bool,
    /// The drop-down list's textures.
    pub drop_art: super::dropdown::DropArt,
}

impl FrontLits {
    pub fn from_rom(rom: &Path) -> FrontLits {
        let mut out = FrontLits::default();
        let p = rom.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1]);
        let Ok(pe) = dc_rom::PeFile::from_file(&p) else {
            return out;
        };
        out.rom = true;
        out.fader = read_pinned(&pe, &FADER).ok();
        out.drop_art = super::dropdown::DropArt::from_pe(&pe);
        for (i, f) in COUNTDOWN_FORMS.iter().enumerate() {
            out.countdown[i] = pe
                .wstr(f.rva)
                .ok()
                .filter(|s| matches_pin(s, f.sha16, f.len));
        }
        // each key's file: the client attribute file's interface section (the same lookup as the HUD's voices)
        // EVIDENCE: cshell+0x1b2c00 (section literal, key, into a 0x80 path buffer)
        let section = read_pinned(&pe, &crate::hud::banners::SOUND_SECTION).ok();
        let vfs = dc_rez::RezVfs::new(rom);
        if let (Some(section), Some(doc)) = (
            section,
            dc_bute::ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", crate::sound::CLIENT_ENTRY),
        ) {
            let Some(m) = doc.get(&section) else {
                return out;
            };
            for (id, _, pin) in SOUND_KEYS.iter() {
                let Ok(key) = read_pinned(&pe, pin) else {
                    continue;
                };
                if let Some(f) = m
                    .get(&key)
                    .map(|v| v.trim().trim_matches('"').to_string())
                    .filter(|v| !v.is_empty())
                {
                    out.sounds.insert(*id, f);
                }
            }
        }
        out
    }

    /// The countdown line for `count` (0..4 from 0x3044); `None` when the line shows 0 seconds.
    // EVIDENCE: cshell+0x15f2b0
    // DC-DEFAULT: CLI-475 (staged) without the two forms (no ROM) the line is `<text> <n>`
    pub fn countdown_line(&self, text: &str, count: u32) -> Option<String> {
        let n = COUNTDOWN_FROM.saturating_sub(count);
        if n == 0 {
            return None;
        }
        let form = if n == 1 {
            &self.countdown[0]
        } else {
            &self.countdown[1]
        };
        Some(match form {
            Some(f) => f.replacen("%s", text, 1).replacen("%d", &n.to_string(), 1),
            None => format!("{text} {n}"),
        })
    }
}

/// A running screen fade: to black, the screen change, then from black.
#[derive(Debug, Clone, Copy)]
pub struct Fade {
    pub target: Go,
    pub start: Instant,
    /// The second half (from black, the new screen already up).
    pub fading_in: bool,
}

impl Fade {
    pub fn new(target: Go) -> Fade {
        Fade {
            target,
            start: Instant::now(),
            fading_in: false,
        }
    }

    pub fn age_ms(&self) -> u32 {
        self.start.elapsed().as_millis().min(u32::MAX as u128) as u32
    }
}

/// The black's alpha `ms` into a half (rising to black, falling from it).
// DISASM: cshell+0x1ab365..0x1ab390
pub fn fade_alpha(ms: u32, fading_in: bool) -> u8 {
    let a = (ms.min(FADE_MS) * 255 / FADE_MS) as u8;
    if fading_in {
        255 - a
    } else {
        a
    }
}

/// Screen changes the 2009 client makes through its fade (the others switch at once).
// EVIDENCE: cshell+0xfacd0 (channel entered -> the fade to the lobby), cshell+0x35010 (channel left -> the fade to
// the server or the channel screen), cshell+0x165400 (room left -> the fade to the lobby); cshell+0x13ab00 and
// cshell+0xfb3f9 (server -> channel and channel -> server switch at once)
pub fn fades(from: &str, to: Go) -> bool {
    matches!(
        (from, to),
        ("channels", Go::Lobby)
            | ("lobby", Go::Servers)
            | ("lobby", Go::Channels)
            | ("room", Go::Lobby)
    )
}

/// The fader over the whole screen at `alpha` (the image when the ROM names it, else a black fill).
// DISASM: cshell+0x1ab387..0x1ab397 (colour alpha << 24 | 0xffffff on the fader image)
pub fn fade_item(fader: Option<&str>, tex_size: Option<(u32, u32)>, alpha: u8) -> DrawItem {
    match (fader, tex_size) {
        (Some(t), Some((w, h))) => DrawItem::Image {
            tex: t.to_string(),
            src: [w as f32, h as f32],
            dst: [0.0, 0.0, 1024.0, 768.0],
            tint: [255, 255, 255, alpha],
            turn: None,
        },
        _ => DrawItem::Fill {
            rect: [0.0, 0.0, 1024.0, 768.0],
            colour: [0, 0, 0, alpha],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_alpha_rises_then_falls_over_600_ms() {
        assert_eq!(fade_alpha(0, false), 0);
        assert_eq!(fade_alpha(300, false), 127);
        assert_eq!(fade_alpha(600, false), 255);
        assert_eq!(fade_alpha(900, false), 255);
        assert_eq!(fade_alpha(0, true), 255);
        assert_eq!(fade_alpha(600, true), 0);
    }

    #[test]
    fn only_the_2009_fade_changes_fade() {
        assert!(fades("channels", Go::Lobby));
        assert!(fades("lobby", Go::Servers));
        assert!(fades("lobby", Go::Channels));
        assert!(fades("room", Go::Lobby));
        assert!(!fades("servers", Go::Channels));
        assert!(!fades("channels", Go::Servers));
        assert!(!fades("lobby", Go::Room));
    }

    #[test]
    fn countdown_line_uses_the_forms_and_skips_zero() {
        let l = FrontLits {
            countdown: [Some("%s %d one.".into()), Some("%s %d many.".into())],
            ..FrontLits::default()
        };
        assert_eq!(
            l.countdown_line("Go in", 0).as_deref(),
            Some("Go in 5 many.")
        );
        assert_eq!(
            l.countdown_line("Go in", 4).as_deref(),
            Some("Go in 1 one.")
        );
        assert_eq!(l.countdown_line("Go in", 5), None);
        assert_eq!(
            FrontLits::default().countdown_line("Go in", 2).as_deref(),
            Some("Go in 3")
        );
    }
}
