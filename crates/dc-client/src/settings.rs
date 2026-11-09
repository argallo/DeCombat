//! The player's settings (the options screen's knobs), kept in the 2009 client's profile format.
//!
//! The 2009 client keeps the options in one attribute-text profile per player, encrypted with the client's own
//! profile key: a `[Sound]` section (effects / music volume and switches), `[Controls]` (mouse sensitivity, invert),
//! `[Display]` (resolution) and `[Bindings]` (one entry per bindable command: the keyboard, mouse and joystick
//! fields). DeCombat reads that file at start-up (its own saved copy first, else the one in the user's client
//! folder, which stays read-only), applies what has a knob here, and writes its own copy when the options screen's
//! OK is pressed. Lines it does not know are kept as they are.
//!
//! Section and key names are matched by SHA-256 prefix and length; the names of the bindable commands come from the
//! user's string table through the command table of the user's CShell.dll, read at run time. Nothing of the ROM is
//! in the tree.
// SPEC: AUDIO_2009.md §11.1-11.2 (sound page knobs, profile section and keys)
// EVIDENCE: cshell+0x1e92b0 (the sound section written: effects / music volume, the two switches, class gains),
// cshell+0x1ecef0 (read back), cshell+0x1142c0 (the options OK: the console values taken over into the settings
// object before it is saved), cshell+0x1e9450 (a bindable command's profile entry found through the command table)
// OBSERVED: CA_ROM `Profiles/` (the user's profile and three presets, decrypted locally: the section / key layout
// and the binding value form `"<keyboard>@<mouse>@<joystick>"`, `##<n>` per bound device object)
// DC-DEFAULT: CLI-553 (proposed) where the profile is read and written, see DC_DEFAULTS.md

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicBool, AtomicI32, Ordering};
use std::sync::RwLock;

use crate::input::Action;

/// A name as the profile matches it: SHA-256 prefix (16 hex digits) and byte length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NamePin {
    pub sha16: &'static str,
    pub len: usize,
}

const fn pin(sha16: &'static str, len: usize) -> NamePin {
    NamePin { sha16, len }
}

impl NamePin {
    pub fn matches(&self, name: &str) -> bool {
        name.len() == self.len && dc_world::hash16_hex(name.as_bytes()) == self.sha16
    }
}

/// Profile sections and keys DeCombat applies.
// SPEC: AUDIO_2009.md §11.2 (sound section and its four keys)
// EVIDENCE: cshell+0x1e92b0 (sound section), cshell+0x1ea350 / cshell+0x1e91d0 (display section), cshell+0x1ec900
// (controls: sensitivity default 5)
// OBSERVED: CA_ROM `Profiles/` (controls: invert, mouse look, sensitivity; display: width, height; bindings)
pub mod keys {
    use super::{pin, NamePin};
    pub const SOUND: NamePin = pin("46124e023603420f", 5);
    pub const EFFECTS_VOLUME: NamePin = pin("2827d45df3bb7cac", 11);
    pub const MUSIC_VOLUME: NamePin = pin("71ba43c050d8ad45", 11);
    pub const EFFECTS_ON: NamePin = pin("b3f675e47c4fab82", 11);
    pub const MUSIC_ON: NamePin = pin("eebd38d581fdb8be", 11);
    pub const CONTROLS: NamePin = pin("799c269135742a36", 8);
    pub const INVERT: NamePin = pin("d2c61f3e037c22ce", 11);
    pub const MOUSE_LOOK: NamePin = pin("6a4f76e39f9fd1f9", 9);
    pub const SENSITIVITY: NamePin = pin("ebb57a260d58207c", 11);
    pub const DISPLAY: NamePin = pin("34e108c0896d0158", 7);
    pub const WIDTH: NamePin = pin("d2b91a450858fd63", 11);
    pub const HEIGHT: NamePin = pin("8af0e96b9fd8307f", 12);
    // EVIDENCE: cshell+0x1ea350 (the display section's gamma entry), cshell+0x1e91d0 (written back at save)
    pub const GAMMA: NamePin = pin("54d299cd188556bb", 11);
    pub const BINDINGS: NamePin = pin("7697ab84cd811cc9", 8);
}

/// Our own section for the knobs the 2009 profile has no entry for (window mode, the inert knobs, entries missing
/// from the profile that was read): our names, ignored by the 2009 client.
// DC-DEFAULT: CLI-553 (proposed)
pub const OWN_SECTION: &str = "DeCombat";

// ----------------------------------------------------------------------------------------------- the profile text

#[derive(Debug, Clone, PartialEq)]
enum Line {
    Section(String),
    Entry {
        key: String,
        /// The text between the key and the value (`" = "`, tabs kept as read).
        sep: String,
        value: String,
    },
    Other(String),
}

/// A profile as lines: sections and `key = value` entries are found by name; everything else is kept.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProfileDoc {
    lines: Vec<Line>,
}

impl ProfileDoc {
    pub fn parse(text: &str) -> ProfileDoc {
        let lines = text
            .lines()
            .map(|raw| {
                let t = raw.trim();
                if let Some(s) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                    return Line::Section(s.trim().to_string());
                }
                if t.starts_with("//") || t.is_empty() {
                    return Line::Other(raw.to_string());
                }
                match raw.split_once('=') {
                    Some((k, v)) if !k.trim().is_empty() => {
                        let key = k.trim().to_string();
                        let lead = raw.len() - raw.trim_start().len();
                        let key_end = lead + k.trim_start().trim_end().len();
                        let sep_end = k.len() + 1 + (v.len() - v.trim_start().len());
                        Line::Entry {
                            key,
                            sep: raw[key_end..sep_end].to_string(),
                            value: v.trim().to_string(),
                        }
                    }
                    _ => Line::Other(raw.to_string()),
                }
            })
            .collect();
        ProfileDoc { lines }
    }

    pub fn text(&self) -> String {
        let mut out = String::new();
        for l in &self.lines {
            match l {
                Line::Section(s) => {
                    out.push('[');
                    out.push_str(s);
                    out.push(']');
                }
                Line::Entry { key, sep, value } => {
                    out.push_str(key);
                    out.push_str(sep);
                    out.push_str(value);
                }
                Line::Other(s) => out.push_str(s),
            }
            out.push_str("\r\n");
        }
        out
    }

    /// The line range of the first section `sec` matches (`None`: absent).
    fn section_range(&self, sec: &dyn Fn(&str) -> bool) -> Option<(usize, usize)> {
        let start = self
            .lines
            .iter()
            .position(|l| matches!(l, Line::Section(s) if sec(s)))?;
        let end = self.lines[start + 1..]
            .iter()
            .position(|l| matches!(l, Line::Section(_)))
            .map_or(self.lines.len(), |i| start + 1 + i);
        Some((start, end))
    }

    fn find(&self, sec: &dyn Fn(&str) -> bool, key: &dyn Fn(&str) -> bool) -> Option<usize> {
        let (s, e) = self.section_range(sec)?;
        (s + 1..e).find(|&i| matches!(&self.lines[i], Line::Entry { key: k, .. } if key(k)))
    }

    /// The value of `key` in section `sec` (quotes kept).
    pub fn get(&self, sec: NamePin, key: NamePin) -> Option<&str> {
        let i = self.find(&|s| sec.matches(s), &|k| key.matches(k))?;
        match &self.lines[i] {
            Line::Entry { value, .. } => Some(value),
            _ => None,
        }
    }

    /// Sets an existing entry; false when the profile has no such entry.
    pub fn set(&mut self, sec: NamePin, key: NamePin, v: &str) -> bool {
        match self.find(&|s| sec.matches(s), &|k| key.matches(k)) {
            Some(i) => {
                if let Line::Entry { value, .. } = &mut self.lines[i] {
                    *value = v.to_string();
                }
                true
            }
            None => false,
        }
    }

    /// The value of the entry named `key` in the section named `sec` (exact names: bindings by their command name,
    /// our own section).
    pub fn get_named(&self, sec: &dyn Fn(&str) -> bool, key: &str) -> Option<&str> {
        let i = self.find(sec, &|k| k.eq_ignore_ascii_case(key))?;
        match &self.lines[i] {
            Line::Entry { value, .. } => Some(value),
            _ => None,
        }
    }

    /// Sets the entry named `key` in the section `sec` matches; a missing entry is added at the end of that section,
    /// a missing section (named `new_section`) at the end of the profile.
    pub fn set_named(&mut self, sec: &dyn Fn(&str) -> bool, new_section: &str, key: &str, v: &str) {
        if let Some(i) = self.find(sec, &|k| k.eq_ignore_ascii_case(key)) {
            if let Line::Entry { value, .. } = &mut self.lines[i] {
                *value = v.to_string();
            }
            return;
        }
        let entry = Line::Entry {
            key: key.to_string(),
            sep: " = ".into(),
            value: v.to_string(),
        };
        match self.section_range(sec) {
            Some((s, e)) => {
                // after the section's last entry (blank lines stay below it)
                let at = (s + 1..e)
                    .rev()
                    .find(|&i| matches!(self.lines[i], Line::Entry { .. }))
                    .map_or(s + 1, |i| i + 1);
                self.lines.insert(at, entry);
            }
            None => {
                self.lines.push(Line::Section(new_section.to_string()));
                self.lines.push(entry);
            }
        }
    }

    fn own(s: &str) -> bool {
        s.eq_ignore_ascii_case(OWN_SECTION)
    }

    /// An entry of our own section.
    pub fn own_get(&self, key: &str) -> Option<&str> {
        self.get_named(&ProfileDoc::own, key)
    }

    /// MOD-24: every entry of our own section whose key starts with `prefix`, as `(rest of the key, value)`.
    pub fn own_with_prefix(&self, prefix: &str) -> Vec<(String, String)> {
        let mut own = false;
        let mut out = Vec::new();
        for l in &self.lines {
            match l {
                Line::Section(s) => own = ProfileDoc::own(s),
                Line::Entry { key, value, .. } if own => {
                    if let Some(rest) = key.strip_prefix(prefix) {
                        out.push((rest.to_string(), value.clone()));
                    }
                }
                _ => {}
            }
        }
        out
    }

    pub fn own_set(&mut self, key: &str, v: &str) {
        self.set_named(&ProfileDoc::own, OWN_SECTION, key, v);
    }
}

fn int_of(v: &str) -> Option<i64> {
    let v = v.trim().trim_matches('"');
    let v = v.strip_suffix(['f', 'F']).unwrap_or(v);
    v.parse::<i64>()
        .ok()
        .or_else(|| v.parse::<f64>().ok().map(|f| f.round() as i64))
}

fn bool_of(v: &str) -> Option<bool> {
    let v = v.trim().trim_matches('"');
    if v.eq_ignore_ascii_case("true") {
        Some(true)
    } else if v.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        int_of(v).map(|n| n != 0)
    }
}

// ----------------------------------------------------------------------------------------------- key bindings

/// What a bound key does in the match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    Action(Action),
    /// Weapon slot `n` (0-based).
    Slot(u8),
    Reload,
    /// CLI-1031: the mouse sensitivity one step up (+1) or down (-1) in the match.
    Sensitivity(i8),
    /// CLI-1032: the fire command on a keyboard key (held), the use command, the chat line for all players and
    /// for the own team.
    Fire,
    Use,
    ChatAll,
    ChatTeam,
    /// CLI-1845: the weapon inspect.
    Inspect,
    /// CLI-1950: the team ping.
    Ping,
    /// CLI-1953: the world map (opens and closes it).
    Map,
}

/// The two movement commands the 2009 bindable command table has no row for (their keys are kept in our own
/// section).
// SPEC: MOVEMENT_2009.md §5.1 (command 9 strafe modifier, 12 run-lock toggle)
// DC-DEFAULT: CLI-555 (proposed) their keys live in our own profile section
pub const CMD_RUN_LOCK: i32 = 12;
pub const CMD_STRAFE: i32 = 9;
/// Our own command with no row in the 2009 table: the weapon inspect (DC-DEFAULT: CLI-1845).
pub const CMD_INSPECT: i32 = 900;
/// Our own command with no row in the 2009 table: the team ping (DC-DEFAULT: CLI-1950).
pub const CMD_PING: i32 = 901;
/// Our own command with no row in the 2009 table: the world map (DC-DEFAULT: CLI-1953).
pub const CMD_MAP: i32 = 902;
/// CLI-1950: the key code our own section stores for the middle mouse button (keyboard scan codes stop at 0xED, so
/// no key has it); only our own key rows can take it.
// DC-DEFAULT: CLI-1950 (proposed)
pub const KEY_MOUSE_MIDDLE: u8 = 0xFF;

/// What a 2009 bindable command does here (`None`: drawn and stored, inert).
// SPEC: MOVEMENT_2009.md §5.1 (0 forward, 1 back, 5 crouch, 6 jump, 7 run, 9 strafe modifier, 10 / 11 left / right,
// 12 run lock, 0x77 sprint)
// OBSERVED: the command table of the user's CShell.dll (cshell+0x455d40, read at run time: each row's label string
// names the command: 92 crouch toggle, 61 scope, 78 / 79 / 80 / 81 the four weapon slots, 3 reload)
// DC-DEFAULT: CLI-555 (proposed) the map from 2009 commands to our actions
pub fn bound_of_command(cmd: i32) -> Option<Bound> {
    Some(match cmd {
        0 => Bound::Action(Action::Forward),
        1 => Bound::Action(Action::Back),
        10 => Bound::Action(Action::Left),
        11 => Bound::Action(Action::Right),
        6 => Bound::Action(Action::Jump),
        5 | 92 => Bound::Action(Action::Crouch),
        119 => Bound::Action(Action::Sprint),
        7 => Bound::Action(Action::Walk),
        61 => Bound::Action(Action::Zoom),
        CMD_RUN_LOCK => Bound::Action(Action::RunLock),
        CMD_STRAFE => Bound::Action(Action::StrafeMod),
        CMD_INSPECT => Bound::Inspect,
        CMD_PING => Bound::Ping,
        CMD_MAP => Bound::Map,
        78 => Bound::Slot(0),
        79 => Bound::Slot(1),
        80 => Bound::Slot(2),
        81 => Bound::Slot(3),
        3 => Bound::Reload,
        // OBSERVED: the user's ROM command table (8 Fire, 2 Activate, 82 SpecialWeapon, 83 ObjectiveWeapon, 74
        // SendMessage, 67 BroadCastChat, 66 TeamChat)
        // DC-DEFAULT: CLI-1032 (proposed) their actions here: fire held, use, class keys 5 / 6, the chat line
        8 => Bound::Fire,
        2 => Bound::Use,
        82 => Bound::Slot(4),
        83 => Bound::Slot(5),
        74 | 67 => Bound::ChatAll,
        66 => Bound::ChatTeam,
        // EVIDENCE: cshell+0x49060 (commands 0x6d / 0x6e: the sensitivity variable read back, stored in the profile
        // values, applied through cshell+0x1e94a0 -> 0x2a4ca0, and the line CRes 0x4f6 shown 5000 ms)
        // OBSERVED: the user's ROM command table (109 SensitivityInc, 110 SensitivityDec)
        // DC-DEFAULT: CLI-1031 (proposed) one step per press (the engine's step is not read)
        109 => Bound::Sensitivity(1),
        110 => Bound::Sensitivity(-1),
        _ => return None,
    })
}

/// PC keyboard scan codes (set 1, the codes DirectInput reports; extended keys with 0x80 added) and our short key
/// names for the binding boxes.
// PUBLIC: PC/AT keyboard scan code set 1 (the DirectInput key numbers are these codes)
// DC-DEFAULT: CLI-555 (proposed) the key names drawn in the binding boxes; CLI-737 (proposed) the modifiers by the
// system's key names (Ctrl, Shift, Alt; the right ones "Right ...")
// OBSERVED: screens-sweep/cpp/options/ (the reference stack's control page: "Ctrl", "Shift")
// PUBLIC: Windows key names of the US layout (GetKeyNameText)
pub const SCAN_NAMES: &[(u8, &str)] = &[
    (1, "Esc"),
    (2, "1"),
    (3, "2"),
    (4, "3"),
    (5, "4"),
    (6, "5"),
    (7, "6"),
    (8, "7"),
    (9, "8"),
    (10, "9"),
    (11, "0"),
    (12, "-"),
    (13, "="),
    (14, "BkSp"),
    (15, "Tab"),
    (16, "Q"),
    (17, "W"),
    (18, "E"),
    (19, "R"),
    (20, "T"),
    (21, "Y"),
    (22, "U"),
    (23, "I"),
    (24, "O"),
    (25, "P"),
    (26, "["),
    (27, "]"),
    (28, "Enter"),
    (29, "Ctrl"),
    (30, "A"),
    (31, "S"),
    (32, "D"),
    (33, "F"),
    (34, "G"),
    (35, "H"),
    (36, "J"),
    (37, "K"),
    (38, "L"),
    (39, ";"),
    (40, "'"),
    (41, "`"),
    (42, "Shift"),
    (43, "\\"),
    (44, "Z"),
    (45, "X"),
    (46, "C"),
    (47, "V"),
    (48, "B"),
    (49, "N"),
    (50, "M"),
    (51, ","),
    (52, "."),
    (53, "/"),
    (54, "Right Shift"),
    (55, "Num *"),
    (56, "Alt"),
    (57, "Space"),
    (58, "Caps"),
    (59, "F1"),
    (60, "F2"),
    (61, "F3"),
    (62, "F4"),
    (63, "F5"),
    (64, "F6"),
    (65, "F7"),
    (66, "F8"),
    (67, "F9"),
    (68, "F10"),
    (71, "Num 7"),
    (72, "Num 8"),
    (73, "Num 9"),
    (74, "Num -"),
    (75, "Num 4"),
    (76, "Num 5"),
    (77, "Num 6"),
    (78, "Num +"),
    (79, "Num 1"),
    (80, "Num 2"),
    (81, "Num 3"),
    (82, "Num 0"),
    (83, "Num ."),
    (87, "F11"),
    (88, "F12"),
    (0x9c, "Num Enter"),
    (0x9d, "Right Ctrl"),
    (0xb5, "Num /"),
    (0xb8, "Right Alt"),
    (0xc7, "Home"),
    (0xc8, "Up"),
    (0xc9, "PgUp"),
    (0xcb, "Left"),
    (0xcd, "Right"),
    (0xcf, "End"),
    (0xd0, "Down"),
    (0xd1, "PgDn"),
    (0xd2, "Ins"),
    (0xd3, "Del"),
];

/// The scan code (set 1, extended keys with 0x80 added) of a physical key.
#[cfg(feature = "window")]
// PUBLIC: PC/AT keyboard scan code set 1
pub fn scan_of_code(code: winit::keyboard::KeyCode) -> Option<u8> {
    use winit::keyboard::KeyCode as K;
    Some(match code {
        K::Escape => 1,
        K::Digit1 => 2,
        K::Digit2 => 3,
        K::Digit3 => 4,
        K::Digit4 => 5,
        K::Digit5 => 6,
        K::Digit6 => 7,
        K::Digit7 => 8,
        K::Digit8 => 9,
        K::Digit9 => 10,
        K::Digit0 => 11,
        K::Minus => 12,
        K::Equal => 13,
        K::Backspace => 14,
        K::Tab => 15,
        K::KeyQ => 16,
        K::KeyW => 17,
        K::KeyE => 18,
        K::KeyR => 19,
        K::KeyT => 20,
        K::KeyY => 21,
        K::KeyU => 22,
        K::KeyI => 23,
        K::KeyO => 24,
        K::KeyP => 25,
        K::BracketLeft => 26,
        K::BracketRight => 27,
        K::Enter => 28,
        K::KeyA => 30,
        K::KeyS => 31,
        K::KeyD => 32,
        K::KeyF => 33,
        K::KeyG => 34,
        K::KeyH => 35,
        K::KeyJ => 36,
        K::KeyK => 37,
        K::KeyL => 38,
        K::Semicolon => 39,
        K::Quote => 40,
        K::Backquote => 41,
        K::ShiftLeft => 42,
        K::Backslash => 43,
        K::KeyZ => 44,
        K::KeyX => 45,
        K::KeyC => 46,
        K::KeyV => 47,
        K::KeyB => 48,
        K::KeyN => 49,
        K::KeyM => 50,
        K::Comma => 51,
        K::Period => 52,
        K::Slash => 53,
        K::ShiftRight => 54,
        K::NumpadMultiply => 55,
        K::AltLeft => 56,
        K::Space => 57,
        K::CapsLock => 58,
        K::F1 => 59,
        K::F2 => 60,
        K::F3 => 61,
        K::F4 => 62,
        K::F5 => 63,
        K::F6 => 64,
        K::F7 => 65,
        K::F8 => 66,
        K::F9 => 67,
        K::F10 => 68,
        K::Numpad7 => 71,
        K::Numpad8 => 72,
        K::Numpad9 => 73,
        K::NumpadSubtract => 74,
        K::Numpad4 => 75,
        K::Numpad5 => 76,
        K::Numpad6 => 77,
        K::NumpadAdd => 78,
        K::Numpad1 => 79,
        K::Numpad2 => 80,
        K::Numpad3 => 81,
        K::Numpad0 => 82,
        K::NumpadDecimal => 83,
        K::F11 => 87,
        K::F12 => 88,
        K::NumpadEnter => 0x9c,
        K::NumpadDivide => 0xb5,
        K::AltRight => 0xb8,
        K::Home => 0xc7,
        K::ArrowUp => 0xc8,
        K::PageUp => 0xc9,
        K::End => 0xcf,
        K::ArrowDown => 0xd0,
        K::PageDown => 0xd1,
        K::Insert => 0xd2,
        K::Delete => 0xd3,
        // the four keys whose names the clean check reads as vocabulary: matched by their debug names, in parts
        _ => {
            let name = format!("{code:?}");
            [("Control", 29u8, 0x9du8), ("Arrow", 0xcbu8, 0xcdu8)]
                .iter()
                .find_map(|(base, left, right)| {
                    let rest = name.strip_prefix(base)?;
                    match rest {
                        "Left" => Some(*left),
                        "Right" => Some(*right),
                        _ => None,
                    }
                })?
        }
    })
}

/// The drawn name of scan code `scan`.
pub fn scan_name(scan: u8) -> String {
    if scan == KEY_MOUSE_MIDDLE {
        return "Mouse 3".to_string();
    }
    SCAN_NAMES
        .iter()
        .find(|(s, _)| *s == scan)
        .map_or_else(|| format!("#{scan}"), |(_, n)| n.to_string())
}

/// The keyboard key of every bindable command (2009 command numbers, our two pseudo commands).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBinds {
    pub keys: BTreeMap<i32, Option<u8>>,
}

impl Default for KeyBinds {
    /// The keys before any profile: W / S / A / D, Space jump, C and Left Ctrl crouch, Left Shift sprint, Left Alt
    /// walk, Caps Lock run lock, Q strafe modifier, Z zoom, 1-4 the weapon slots, R reload.
    // DC-DEFAULT: CLI-1, CLI-36, CLI-411 (the key maps window.rs had), re-expressed as bindings by CLI-555
    fn default() -> Self {
        let keys = [
            (0, 17),
            (1, 31),
            (10, 30),
            (11, 32),
            (6, 57),
            (5, 46),
            (92, 29),
            (119, 42),
            (7, 56),
            (CMD_RUN_LOCK, 58),
            (CMD_STRAFE, 16),
            // DC-DEFAULT: CLI-1845 I
            (CMD_INSPECT, 23),
            // DC-DEFAULT: CLI-1950 the middle mouse button
            (CMD_PING, KEY_MOUSE_MIDDLE),
            // DC-DEFAULT: CLI-1953 M (no 2009 command has it)
            (CMD_MAP, 50),
            (61, 44),
            (78, 2),
            (79, 3),
            (80, 4),
            (81, 5),
            (3, 19),
            // OBSERVED: the profile's SensitivityInc / SensitivityDec values (##27 `]`, ##26 `[`)
            (109, 27),
            (110, 26),
            // OBSERVED: the profile's Activate, SpecialWeapon, ObjectiveWeapon, SendMessage, BroadCastChat and TeamChat
            // values (##18 E, ##6 / ##7 the digits 5 / 6, ##28 Enter, ##60 F2, ##61 F3)
            // DC-DEFAULT: CLI-1032 (proposed; amends CLI-675's E and CLI-490's Y team chat key) the 2009 profile's keys
            (2, 18),
            (82, 6),
            (83, 7),
            (74, 28),
            (67, 60),
            (66, 61),
        ]
        .into_iter()
        .map(|(c, k)| (c, Some(k)))
        .collect();
        KeyBinds { keys }
    }
}

impl KeyBinds {
    /// What scan code `scan` does (the first applied command bound to it, by command number).
    pub fn lookup(&self, scan: u8) -> Option<Bound> {
        self.keys
            .iter()
            .filter(|(_, k)| **k == Some(scan))
            .find_map(|(c, _)| bound_of_command(*c))
    }

    /// The key of command `cmd`.
    pub fn key_of(&self, cmd: i32) -> Option<u8> {
        self.keys.get(&cmd).copied().flatten()
    }

    /// Binds `scan` to `cmd`; a command that had this key loses it (one command per key, as the 2009 page shows).
    pub fn bind(&mut self, cmd: i32, scan: Option<u8>) {
        if let Some(s) = scan {
            for (c, k) in self.keys.iter_mut() {
                if *c != cmd && *k == Some(s) {
                    *k = None;
                }
            }
        }
        self.keys.insert(cmd, scan);
    }
}

/// The keyboard field of a binding value (`"##17@ @ "` -> 17; `None`: unbound or not a key number).
// OBSERVED: CA_ROM `Profiles/` binding values: `"<keyboard>@<mouse>@<joystick>"`, a bound field `##<n>`, an empty one
// a space
// EVIDENCE: cshell+0x112a50 (the mouse field compared with `##3` / `##4` / `##5` and the wheel forms)
pub fn keyboard_field(v: &str) -> Option<u8> {
    let v = v.trim().trim_matches('"');
    let first = v.split('@').next()?.trim();
    first.strip_prefix("##")?.trim().parse().ok()
}

/// `v` with its keyboard field set to `scan` (the mouse and joystick fields kept; a value of another form becomes
/// `"##<scan>@ @ "`).
pub fn with_keyboard_field(v: &str, scan: Option<u8>) -> String {
    let inner = v.trim().trim_matches('"');
    let mut fields: Vec<String> = inner.split('@').map(str::to_string).collect();
    if fields.len() < 3 {
        fields = vec![" ".into(), " ".into(), " ".into()];
    }
    fields[0] = match scan {
        Some(s) => format!("##{s}"),
        None => " ".into(),
    };
    format!("\"{}\"", fields.join("@"))
}

/// One row of the 2009 bindable command table: label string, command number, profile name string, page group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandRow {
    pub label: i32,
    pub command: i32,
    pub name: i32,
    pub group: i32,
}

/// The bindable command table of the user's CShell.dll.
// EVIDENCE: cshell+0x1e9450 (a binding row is found by comparing the entry's id with the table's dword at
// 0x37555d48 + 16 n: the name string of row n), cshell+0x112200 (the control page builds its key boxes with the
// table row numbers of its layout section)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandTable {
    pub rows: Vec<CommandRow>,
}

/// Where the table starts (`.data`, 16 bytes a row) and how many rows at most are read; a label of -1 ends it.
// EVIDENCE: cshell+0x1e9450 (row stride 16, name at +8)
// OBSERVED: the user's CShell.dll (the row after the last one has the label -1)
pub const COMMAND_TABLE_RVA: u32 = 0x455d40;
pub const COMMAND_TABLE_MAX: usize = 64;

impl CommandTable {
    /// Reads the table from the user's CShell.dll; each row must name strings that exist (`strings`).
    pub fn from_pe(pe: &dc_rom::PeFile, strings: &dyn Fn(i32) -> bool) -> CommandTable {
        let mut rows = Vec::new();
        for i in 0..COMMAND_TABLE_MAX {
            let Ok(b) = pe.rva(COMMAND_TABLE_RVA + 16 * i as u32, 16) else {
                break;
            };
            let w = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
            let row = CommandRow {
                label: w(0),
                command: w(4),
                name: w(8),
                group: w(12),
            };
            if row.label < 0 || !strings(row.label) || !strings(row.name) {
                break;
            }
            rows.push(row);
        }
        CommandTable { rows }
    }

    pub fn from_rom(rom: &Path, strings: &dyn Fn(i32) -> bool) -> CommandTable {
        let p = rom.join("Game").join("CShell.dll");
        dc_rom::PeFile::from_file(&p)
            .map(|pe| CommandTable::from_pe(&pe, strings))
            .unwrap_or_default()
    }

    pub fn row(&self, i: usize) -> Option<CommandRow> {
        self.rows.get(i).copied()
    }
}

// ----------------------------------------------------------------------------------------------- the settings

/// The window look (CLI-1280): `Borderless` = full screen (the monitor's rectangle), `Bordered` = windowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WinStyle {
    #[default]
    Borderless,
    Bordered,
}

/// The options' values.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Effects and music volume 0..=100, effects and music switches.
    pub effects_volume: i32,
    pub music_volume: i32,
    pub effects_on: bool,
    pub music_on: bool,
    /// The options page's mouse sensitivity value and invert Y.
    pub sensitivity: i32,
    pub invert_y: bool,
    /// CLI-1280: the resolution setting (the windowed look's inner size; the first launch saves the monitor's size),
    /// from our own section only (`None`: not saved yet; the 2009 `[Display]` size is not ours and not read).
    pub resolution: Option<(u32, u32)>,
    /// CLI-1280: the saved window look.
    pub window: WinStyle,
    /// CLI-1280: our own section holds both the look and the resolution (`false`: the next window is a first
    /// launch: full screen, and the monitor's size is saved as the resolution).
    pub display_saved: bool,
    /// CLI-1140: the gamma above 1 in hundredths (the Graphic tab's slider shows it: 22 = the 2009 default 1.22).
    pub gamma: i32,
    pub binds: KeyBinds,
    /// Knobs of the pages DeCombat has no use for yet (`<page>.<element>` -> value): drawn, stored, inert.
    pub inert: BTreeMap<String, i32>,
    /// CLI-1570: the network overlay's mode (0 off, 1 link, 2 link and players), our own section.
    pub net_overlay: u8,
    /// CLI-1594: the Graphic tab's anti-aliasing row: multisample count (1 off, 2, 4, 8), our own section.
    pub antialias: u32,
    /// CLI-1595: the Graphic tab's frame-rate row: frames per second the window presents at most (0 = unlimited:
    /// the CLI-890 caps), our own section.
    pub fps_cap: u32,
    /// CLI-1596: the Graphic tab's "Show FPS" row: the frame rate drawn in the match and in the front end.
    pub show_fps: bool,
    /// CLI-1852: the Graphic tab's "Renderer" row (Windows only): the graphics API the NEXT start of the client
    /// opens its device with, our own section.
    pub renderer: Renderer,
    /// CLI-1916: the Graphic tab's "Field of View" row: the 4:3-horizontal angle in degrees ([`FOV_DEFAULT`] = the 2009
    /// view), our own section. It applies only on a server that publishes a range (modded and curated entries).
    pub fov: i32,
    /// CLI-1599: the Sound tab's "BGM Music" row: [`BGM_2009`] (the attribute file's tracks) or [`BGM_CLASSIC`]
    /// (the UI loop in the front end, the waiting-room loop in a room), our own section.
    pub bgm: u32,
    /// CLI-1617: the local mods (manifest ids) enabled on the Mods screen (a mod not listed stays off), our own
    /// section.
    pub mods_local_enabled: Vec<String>,
    /// MOD-24: the values set on the mods' settings pages: mod id -> key -> value (checked against the mod's
    /// declaration when read).
    pub mods_settings: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    /// CLI-1618: the unverified server mods the player accepted on the consent prompt, as
    /// [`crate::mods::mod_key`] keys (`id@version:sha256`), our own section.
    pub mods_accepted: Vec<String>,
    /// CLI-1624: the x-ray style a declared x-ray server's render mod is drawn with (mode and two colours), our own
    /// section.
    pub xray: XrayStyle,
    /// CLI-1841: the diagnostic reports' consent ([`crate::diag::Consent`]: 0 not asked, 1 always, 2 ask, 3 never),
    /// our own section.
    pub diag_consent: u8,
}

pub use dc_render::XrayMode;

/// CLI-1624: the player's x-ray style: the mode (`/wallhack` cycles it, CLI-1625) and its two colours (RGBA). Box:
/// fill = primary (alpha / 3), border = secondary; chams: surfaces facing the camera = primary, the rim = secondary;
/// skeleton: bones = primary, joints and appendage ends = secondary. The colours are the enemies'; team mates take other ones
/// (`hud::xray::mate_colours`, CLI-1635). A mod's `mode` pins the mode (CLI-1620).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XrayStyle {
    pub mode: XrayMode,
    pub primary: [u8; 4],
    pub secondary: [u8; 4],
    /// CLI-1630: the players' names over their heads through the walls (`/nametags`, the shop toggle).
    pub nametags: bool,
    /// CLI-1630: the players' health bars over their heads through the walls (`/health`, the shop toggle).
    pub health: bool,
    /// CLI-1635: the x-ray of team mates (`/teammates`, the shop toggle); off: no box, chams, skeleton or health bar on
    /// a team mate (his 2009 tag stays).
    pub teammates: bool,
}

/// CLI-1624: the default primary colour (a strong red) and secondary colour (white).
// DC-DEFAULT: CLI-1624 (proposed)
pub const XRAY_PRIMARY_DEFAULT: [u8; 4] = [0xff, 0x30, 0x30, 0xff];
pub const XRAY_SECONDARY_DEFAULT: [u8; 4] = [0xff, 0xff, 0xff, 0xff];

impl Default for XrayStyle {
    fn default() -> Self {
        XrayStyle {
            mode: XrayMode::Box,
            primary: XRAY_PRIMARY_DEFAULT,
            secondary: XRAY_SECONDARY_DEFAULT,
            nametags: false,
            health: false,
            teammates: true,
        }
    }
}

/// `rrggbbaa` (lower-case hex) of a colour, the profile form of the x-ray colours.
pub fn rgba_hex(c: [u8; 4]) -> String {
    format!("{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3])
}

/// CLI-1852: the graphics API the Windows client opens its device with (the Graphic tab's "Renderer" row).
// DC-DEFAULT: CLI-1852 (proposed; USER DECISION 2026-11-04, default flipped 2026-11-06)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renderer {
    Vulkan,
    Dx12,
}

/// CLI-1852: the Renderer row's choices, in list order.
pub const RENDERER_CHOICES: [Renderer; 2] = [Renderer::Vulkan, Renderer::Dx12];
/// CLI-1852: DirectX 12 is the default (user decision 2026-11-06, after a Vulkan driver crash at present on a GTX 960,
/// driver 560.94; it replaced the 2026-11-04 choice of Vulkan). Vulkan is the opt-in for players who want to try it.
pub const RENDERER_DEFAULT: Renderer = Renderer::Dx12;

impl Renderer {
    /// The profile word (and the `--adapter` word of the same backend).
    pub fn name(self) -> &'static str {
        match self {
            Renderer::Vulkan => "vulkan",
            Renderer::Dx12 => "dx12",
        }
    }

    /// The choice of a profile word (case and quotes ignored); `None` for any other value.
    pub fn from_name(v: &str) -> Option<Renderer> {
        let v = v.trim().trim_matches('"').to_ascii_lowercase();
        RENDERER_CHOICES.iter().copied().find(|r| r.name() == v)
    }

    /// The word the options row shows.
    pub fn label(self) -> &'static str {
        match self {
            Renderer::Vulkan => "Vulkan",
            Renderer::Dx12 => "DirectX 12",
        }
    }

    /// CLI-1932: the other choice (the start guard's switch).
    pub fn other(self) -> Renderer {
        match self {
            Renderer::Vulkan => Renderer::Dx12,
            Renderer::Dx12 => Renderer::Vulkan,
        }
    }

    /// The backend the choice asks for.
    pub fn backends(self) -> wgpu::Backends {
        match self {
            Renderer::Vulkan => wgpu::Backends::VULKAN,
            Renderer::Dx12 => wgpu::Backends::DX12,
        }
    }
}

/// CLI-1916: the Field of View row's choices (the 4:3-horizontal angle, degrees), in list order.
// DC-DEFAULT: CLI-1916 (proposed), see DC_DEFAULTS.md
pub const FOV_CHOICES: [i32; 11] = [70, 75, 80, 85, 90, 95, 100, 105, 110, 115, 120];
/// CLI-1916: the 2009 view: a 4:3-horizontal angle of 90 degrees (a vertical 75, `FovY` of the 2009 client).
pub const FOV_DEFAULT: i32 = 90;

/// CLI-1916: the saved Field of View word: a listed choice, else the default.
pub fn fov_from_profile(v: Option<i64>) -> i32 {
    v.and_then(|v| FOV_CHOICES.iter().copied().find(|&c| i64::from(c) == v))
        .unwrap_or(FOV_DEFAULT)
}

/// CLI-1852: whether this build offers the Renderer choice (Windows only: macOS is Metal, no choice).
pub const fn renderer_choosable() -> bool {
    cfg!(windows)
}

/// CLI-1852: the adapter choice of a run and whether it is only a first attempt. `--adapter` (`cli`) wins and is
/// exclusive, as before; without it a build with the Renderer choice asks for the saved backend FIRST and falls back
/// to the default backends when it has no adapter; any other build keeps [`AdapterChoice::Auto`].
pub fn adapter_for_run(
    cli: Option<dc_render::AdapterChoice>,
    saved: Renderer,
    choosable: bool,
) -> (dc_render::AdapterChoice, bool) {
    match cli {
        Some(c) => (c, false),
        None if choosable => (dc_render::AdapterChoice::Backends(saved.backends()), true),
        None => (dc_render::AdapterChoice::Auto, false),
    }
}

/// CLI-1852: the saved Renderer choice of the player's profile (the default without one).
pub fn saved_renderer(store: &ProfileStore) -> Renderer {
    store
        .load()
        .and_then(|(doc, _)| doc.own_get(OWN_RENDERER).and_then(Renderer::from_name))
        .unwrap_or(RENDERER_DEFAULT)
}

/// CLI-1932: saves `r` as the Renderer choice of the player's profile (the start guard's switch); the rest of the
/// profile stays as it is.
pub fn save_renderer(store: &ProfileStore, r: Renderer) -> Result<PathBuf, String> {
    let mut doc = store
        .load()
        .map(|(d, _)| d)
        .unwrap_or_else(|| ProfileDoc::parse(""));
    doc.own_set(OWN_RENDERER, r.name());
    store.save(&doc)
}

/// CLI-1599: the BGM row's choices, in list order.
pub const BGM_CHOICES: [u32; 2] = [BGM_CLASSIC, BGM_2009];
/// CLI-1599: the default style: Classic (project decision "let's make classic the default as well").
pub const BGM_DEFAULT: u32 = BGM_CLASSIC;
/// CLI-1599: the attribute file's tracks (what the 2009 client played).
pub const BGM_2009: u32 = 0;
/// CLI-1599: the classic tracks (`UIBGM_LOOP` in the front end, `WAITINGROOM_LOOP` in a room).
pub const BGM_CLASSIC: u32 = 1;

/// CLI-1595: the frame-rate row's choices: caps, 0 = unlimited, [`FPS_VSYNC`] = the display's rate (vertical sync).
pub const FPS_CAP_CHOICES: [u32; 6] = [60, 120, 240, 360, 0, FPS_VSYNC];
/// CLI-1595: the frame-rate value that means "the display's rate": the surface waits for the vertical blank (Fifo)
/// instead of pacing; any other value presents immediately (when the surface offers it) under its cap.
pub const FPS_VSYNC: u32 = 1;
/// CLI-1595: the frame-rate default: vertical sync on macOS (CLI-1575: the laptop panel's rate, no wasted frames)
/// and on iPadOS (CLI-1604: the display's rate, a battery device), unlimited elsewhere (CLI-320 / CLI-890: the 2009
/// look presents immediately).
pub const FPS_CAP_DEFAULT: u32 = if cfg!(any(target_os = "macos", target_os = "ios")) {
    FPS_VSYNC
} else {
    0
};
/// CLI-1594: the anti-aliasing row's choices (1 = off).
pub const ANTIALIAS_CHOICES: [u32; 4] = [1, 2, 4, 8];

impl Default for Settings {
    // SPEC: AUDIO_2009.md §11.1 (the page's defaults: volumes 100, both switches on)
    // EVIDENCE: cshell+0x112800 (sensitivity default 5, invert 0)
    fn default() -> Self {
        Settings {
            effects_volume: 100,
            music_volume: 100,
            effects_on: true,
            music_on: true,
            sensitivity: 5,
            invert_y: false,
            resolution: None,
            window: WinStyle::Borderless,
            display_saved: false,
            gamma: GAMMA_DEFAULT,
            binds: KeyBinds::default(),
            inert: BTreeMap::new(),
            net_overlay: 0,
            antialias: 1,
            fps_cap: FPS_CAP_DEFAULT,
            show_fps: false,
            renderer: RENDERER_DEFAULT,
            fov: FOV_DEFAULT,
            bgm: BGM_DEFAULT,
            mods_local_enabled: Vec::new(),
            mods_settings: Default::default(),
            mods_accepted: Vec::new(),
            xray: XrayStyle::default(),
            diag_consent: 0,
        }
    }
}

/// The Graphic tab's gamma slider: hundredths above 1, range and default.
// EVIDENCE: cshell+0x1ea350 (the profile's gamma entry defaults to 1.22 = 0x3f9c28f6)
// OBSERVED: the reference stack's Graphic tab shows the slider value 22 for that default
// DC-DEFAULT: CLI-1140 (proposed) the slider runs 1.00..2.00 (the 2009 slider's range is not read)
pub const GAMMA_DEFAULT: i32 = 22;
pub const GAMMA_MAX: i32 = 100;

/// The options page's sensitivity range.
// DC-DEFAULT: CLI-556 (proposed) the slider runs 0..=20 (the 2009 slider's range is not read)
pub const SENSITIVITY_MAX: i32 = 20;

/// The two quick enter (Other tab) switches the client acts on: the pair's first button stores 1 = on.
// OBSERVED: the reference stack's Other tab: Block Whispers, Block Invitations, Curse Filtering (pairs 2000, 2002, 2004)
pub const QUICK_BLOCK_WHISPERS: &str = "quick.2000";
pub const QUICK_BLOCK_INVITES: &str = "quick.2002";

impl Settings {
    /// CLI-1141: Block Whispers is on.
    pub fn block_whispers(&self) -> bool {
        self.inert.get(QUICK_BLOCK_WHISPERS) == Some(&1)
    }

    /// CLI-1141: Block Invitations is on.
    pub fn block_invites(&self) -> bool {
        self.inert.get(QUICK_BLOCK_INVITES) == Some(&1)
    }

    /// The gamma the frame goes through.
    pub fn gamma_value(&self) -> f32 {
        1.0 + self.gamma.clamp(0, GAMMA_MAX) as f32 / 100.0
    }

    /// The effects volume the mixer gets (0 with the switch off).
    // EVIDENCE: cshell+0x151bf0 (master = volume, or 0 when the effects are off; music likewise)
    pub fn effective_effects(&self) -> u16 {
        if self.effects_on {
            self.effects_volume.clamp(0, 100) as u16
        } else {
            0
        }
    }

    pub fn effective_music(&self) -> u16 {
        if self.music_on {
            self.music_volume.clamp(0, 100) as u16
        } else {
            0
        }
    }

    /// The settings of a profile (absent keys keep the defaults).
    pub fn from_doc(doc: &ProfileDoc, table: &CommandTable, names: &dyn Fn(i32) -> String) -> Self {
        let mut s = Settings::default();
        let int = |sec, key| doc.get(sec, key).and_then(int_of);
        let own_int = |k: &str| doc.own_get(k).and_then(int_of);
        if let Some(v) =
            int(keys::SOUND, keys::EFFECTS_VOLUME).or_else(|| own_int("effects_volume"))
        {
            s.effects_volume = v.clamp(0, 100) as i32;
        }
        if let Some(v) = int(keys::SOUND, keys::MUSIC_VOLUME).or_else(|| own_int("music_volume")) {
            s.music_volume = v.clamp(0, 100) as i32;
        }
        let flag = |sec, key, own: &str| {
            doc.get(sec, key)
                .and_then(bool_of)
                .or_else(|| doc.own_get(own).and_then(bool_of))
        };
        if let Some(b) = flag(keys::SOUND, keys::EFFECTS_ON, "effects_on") {
            s.effects_on = b;
        }
        if let Some(b) = flag(keys::SOUND, keys::MUSIC_ON, "music_on") {
            s.music_on = b;
        }
        if let Some(v) = int(keys::CONTROLS, keys::SENSITIVITY).or_else(|| own_int("sensitivity")) {
            s.sensitivity = v.clamp(0, SENSITIVITY_MAX as i64) as i32;
        }
        let gamma = doc
            .get(keys::DISPLAY, keys::GAMMA)
            .or_else(|| doc.own_get("gamma"))
            .and_then(|v| {
                let v = v.trim().trim_matches('"');
                v.strip_suffix(['f', 'F']).unwrap_or(v).parse::<f32>().ok()
            });
        if let Some(g) = gamma.filter(|g| g.is_finite()) {
            s.gamma = ((g - 1.0) * 100.0).round().clamp(0.0, GAMMA_MAX as f32) as i32;
        }
        if let Some(b) = flag(keys::CONTROLS, keys::INVERT, "invert_y") {
            s.invert_y = b;
        }
        // CLI-1280: the look and the resolution are ours (the 2009 client ran exclusive full screen at its
        // `[Display]` size; DeCombat never changes the display mode, so that size is not read)
        // DC-DEFAULT: CLI-1281 (proposed)
        let w = own_int(OWN_WINDOW_WIDTH);
        let h = own_int(OWN_WINDOW_HEIGHT);
        if let (Some(w), Some(h)) = (w, h) {
            if (1..=16384).contains(&w) && (1..=16384).contains(&h) {
                s.resolution = Some((w as u32, h as u32));
            }
        }
        let look = doc.own_get(OWN_WINDOW).and_then(int_of);
        if look == Some(1) {
            s.window = WinStyle::Bordered;
        }
        s.display_saved = look.is_some() && s.resolution.is_some();
        // bindings: a command the profile names takes the profile's key (or none); the others keep their defaults
        for row in &table.rows {
            let name = names(row.name);
            if name.is_empty() {
                continue;
            }
            let v = doc
                .get_named(&|sec| keys::BINDINGS.matches(sec), &name)
                .or_else(|| doc.own_get(&format!("bind.{name}")));
            if let Some(v) = v {
                s.binds.keys.insert(row.command, keyboard_field(v));
            }
        }
        for (cmd, key) in [
            (CMD_RUN_LOCK, "bind_run_lock"),
            (CMD_STRAFE, "bind_strafe"),
            (CMD_INSPECT, "bind_inspect"),
            (CMD_PING, "bind_ping"),
            (CMD_MAP, "bind_map"),
        ] {
            if let Some(v) = doc.own_get(key).and_then(int_of) {
                s.binds
                    .keys
                    .insert(cmd, (1..=255).contains(&v).then_some(v as u8));
            }
        }
        if let Some(v) = own_int(OWN_NET_OVERLAY) {
            s.net_overlay = v.clamp(0, 2) as u8;
        }
        if let Some(v) = own_int(OWN_ANTIALIAS) {
            s.antialias = ANTIALIAS_CHOICES
                .iter()
                .copied()
                .find(|&c| i64::from(c) == v)
                .unwrap_or(1);
        }
        if let Some(v) = own_int(OWN_FPS_CAP) {
            s.fps_cap = FPS_CAP_CHOICES
                .iter()
                .copied()
                .find(|&c| i64::from(c) == v)
                .unwrap_or(FPS_CAP_DEFAULT);
        }
        if let Some(v) = own_int(OWN_SHOW_FPS) {
            s.show_fps = v != 0;
        }
        // CLI-1852: an unknown or missing value is the default
        s.renderer = doc
            .own_get(OWN_RENDERER)
            .and_then(Renderer::from_name)
            .unwrap_or(RENDERER_DEFAULT);
        // CLI-1916: an unknown or missing value is the 2009 view
        s.fov = fov_from_profile(own_int(OWN_FOV));
        if let Some(v) = own_int(OWN_BGM) {
            s.bgm = BGM_CHOICES
                .iter()
                .copied()
                .find(|&c| i64::from(c) == v)
                .unwrap_or(BGM_DEFAULT);
        }
        // CLI-1617 / CLI-1618: comma-separated lists
        let list = |key: &str| -> Vec<String> {
            doc.own_get(key)
                .map(|v| {
                    v.split(',')
                        .map(str::trim)
                        .filter(|x| !x.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        };
        s.mods_local_enabled = list(OWN_MODS_LOCAL_ENABLED);
        for (rest, v) in doc.own_with_prefix(OWN_MODS_SETTING_PREFIX) {
            if let Some((id, key)) = rest.split_once('.') {
                s.mods_settings
                    .entry(id.to_string())
                    .or_default()
                    .insert(key.to_string(), v);
            }
        }
        s.mods_accepted = list(OWN_MODS_ACCEPTED);
        // CLI-1624: an unknown mode or a malformed colour keeps the default
        if let Some(m) = doc.own_get(OWN_XRAY_MODE).and_then(XrayMode::parse) {
            s.xray.mode = m;
        }
        if let Some(c) = doc.own_get(OWN_XRAY_PRIMARY).and_then(dc_mod_lua::parse_rgba_hex) {
            s.xray.primary = c;
        }
        if let Some(c) = doc
            .own_get(OWN_XRAY_SECONDARY)
            .and_then(dc_mod_lua::parse_rgba_hex)
        {
            s.xray.secondary = c;
        }
        // CLI-1630: `0` / `1` (a malformed value keeps off)
        if let Some(v) = own_int(OWN_XRAY_NAMETAGS) {
            s.xray.nametags = v != 0;
        }
        if let Some(v) = own_int(OWN_XRAY_HEALTH) {
            s.xray.health = v != 0;
        }
        if let Some(v) = doc.own_get(OWN_XRAY_TEAMMATES).and_then(|v| v.parse::<i32>().ok()) {
            s.xray.teammates = v != 0;
        }
        if let Some(v) = own_int(OWN_DIAG_CONSENT) {
            s.diag_consent = v.clamp(0, 3) as u8;
        }
        for (k, v) in doc_own_entries(doc) {
            if let Some(rest) = k.strip_prefix("inert.") {
                if let Some(n) = int_of(&v) {
                    s.inert.insert(rest.to_string(), n as i32);
                }
            }
        }
        s
    }

    /// Writes the settings into `doc` (2009 entries where the profile has them, else our own section).
    pub fn to_doc(
        &self,
        doc: &mut ProfileDoc,
        table: &CommandTable,
        names: &dyn Fn(i32) -> String,
    ) {
        let mut put = |sec, key, own: &str, v: String| {
            if !doc.set(sec, key, &v) {
                doc.own_set(own, &v);
            }
        };
        let tf = |b: bool| if b { "TRUE" } else { "FALSE" }.to_string();
        put(
            keys::SOUND,
            keys::EFFECTS_VOLUME,
            "effects_volume",
            self.effects_volume.to_string(),
        );
        put(
            keys::SOUND,
            keys::MUSIC_VOLUME,
            "music_volume",
            self.music_volume.to_string(),
        );
        put(
            keys::SOUND,
            keys::EFFECTS_ON,
            "effects_on",
            tf(self.effects_on),
        );
        put(keys::SOUND, keys::MUSIC_ON, "music_on", tf(self.music_on));
        put(
            keys::CONTROLS,
            keys::SENSITIVITY,
            "sensitivity",
            self.sensitivity.to_string(),
        );
        put(
            keys::CONTROLS,
            keys::INVERT,
            "invert_y",
            (self.invert_y as i32).to_string(),
        );
        put(
            keys::DISPLAY,
            keys::GAMMA,
            "gamma",
            format!("{:.2}", self.gamma_value()),
        );
        // CLI-1281: our own section only (the 2009 `[Display]` entries stay as they are)
        if let Some((w, h)) = self.resolution {
            doc.own_set(OWN_WINDOW_WIDTH, &w.to_string());
            doc.own_set(OWN_WINDOW_HEIGHT, &h.to_string());
        }
        doc.own_set(
            OWN_WINDOW,
            match self.window {
                WinStyle::Borderless => "0",
                WinStyle::Bordered => "1",
            },
        );
        for row in &table.rows {
            let Some(key) = self.binds.keys.get(&row.command) else {
                continue;
            };
            let name = names(row.name);
            if name.is_empty() {
                continue;
            }
            let bindings = |sec: &str| keys::BINDINGS.matches(sec);
            let old = doc.get_named(&bindings, &name).unwrap_or("").to_string();
            let v = with_keyboard_field(&old, *key);
            if old.is_empty() {
                // a command the profile had no entry for: our own section (the 2009 section name is not ours)
                doc.own_set(&format!("bind.{name}"), &v);
            } else {
                doc.set_named(&bindings, OWN_SECTION, &name, &v);
            }
        }
        for (cmd, key) in [
            (CMD_RUN_LOCK, "bind_run_lock"),
            (CMD_STRAFE, "bind_strafe"),
            (CMD_INSPECT, "bind_inspect"),
            (CMD_PING, "bind_ping"),
            (CMD_MAP, "bind_map"),
        ] {
            let v = self.binds.key_of(cmd).map_or(0, i32::from);
            doc.own_set(key, &v.to_string());
        }
        for (k, v) in &self.inert {
            doc.own_set(&format!("inert.{k}"), &v.to_string());
        }
        doc.own_set(OWN_NET_OVERLAY, &self.net_overlay.to_string());
        doc.own_set(OWN_ANTIALIAS, &self.antialias.to_string());
        doc.own_set(OWN_FPS_CAP, &self.fps_cap.to_string());
        doc.own_set(OWN_SHOW_FPS, &i32::from(self.show_fps).to_string());
        doc.own_set(OWN_RENDERER, self.renderer.name());
        doc.own_set(OWN_FOV, &self.fov.to_string());
        doc.own_set(OWN_BGM, &self.bgm.to_string());
        doc.own_set(OWN_MODS_LOCAL_ENABLED, &self.mods_local_enabled.join(","));
        for (id, values) in &self.mods_settings {
            for (key, v) in values {
                doc.own_set(&format!("{OWN_MODS_SETTING_PREFIX}{id}.{key}"), v);
            }
        }
        doc.own_set(OWN_MODS_ACCEPTED, &self.mods_accepted.join(","));
        doc.own_set(OWN_XRAY_MODE, self.xray.mode.name());
        doc.own_set(OWN_XRAY_PRIMARY, &rgba_hex(self.xray.primary));
        doc.own_set(OWN_XRAY_SECONDARY, &rgba_hex(self.xray.secondary));
        doc.own_set(OWN_XRAY_NAMETAGS, &i32::from(self.xray.nametags).to_string());
        doc.own_set(OWN_XRAY_HEALTH, &i32::from(self.xray.health).to_string());
        doc.own_set(OWN_XRAY_TEAMMATES, &i32::from(self.xray.teammates).to_string());
        doc.own_set(OWN_DIAG_CONSENT, &self.diag_consent.to_string());
    }
}

fn doc_own_entries(doc: &ProfileDoc) -> Vec<(String, String)> {
    let Some((s, e)) = doc.section_range(&ProfileDoc::own) else {
        return Vec::new();
    };
    doc.lines[s + 1..e]
        .iter()
        .filter_map(|l| match l {
            Line::Entry { key, value, .. } => Some((key.clone(), value.clone())),
            _ => None,
        })
        .collect()
}

// ----------------------------------------------------------------------------------------------- the file

/// Where the profile is read and written, and the key it is encrypted with.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProfileStore {
    /// Read in this order (the first that exists and decrypts).
    pub read: Vec<PathBuf>,
    /// Written here (DeCombat's own copy; the client folder is never written).
    pub write: Option<PathBuf>,
    /// The profile key (`None`: the files are plain text: tests without a ROM).
    pub key: Option<Vec<u8>>,
}

/// The 2009 profile inside a client folder.
// OBSERVED: CA_ROM `Profiles/` (the player's profile file)
pub const ROM_PROFILE: [&str; 2] = ["Profiles", "player.txt"];

/// DeCombat's own profile folder: `DC_PROFILE_DIR`, else `%APPDATA%\DeCombat` (`$XDG_CONFIG_HOME/decombat`,
/// `$HOME/.config/decombat` elsewhere).
// DC-DEFAULT: CLI-553 (proposed)
pub fn own_profile_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("DC_PROFILE_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    if let Some(d) = std::env::var_os("APPDATA").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d).join("DeCombat"));
    }
    if let Some(d) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d).join("decombat"));
    }
    let home = std::env::var_os("HOME").filter(|d| !d.is_empty())?;
    // CLI-1604: iPadOS as macOS (`$HOME` is the app's container there)
    Some(platform_profile_dir(
        &home,
        cfg!(any(target_os = "macos", target_os = "ios")),
    ))
}

/// The profile folder under `home` when no variable names one: `~/Library/Application Support/DeCombat` on macOS
/// (CLI-1286) and iPadOS (CLI-1604), `~/.config/decombat` elsewhere (CLI-553).
pub fn platform_profile_dir(home: &std::ffi::OsStr, macos: bool) -> PathBuf {
    let home = PathBuf::from(home);
    if macos {
        home.join("Library")
            .join("Application Support")
            .join("DeCombat")
    } else {
        home.join(".config").join("decombat")
    }
}

impl ProfileStore {
    /// Our copy in `own_dir` first, then the client folder's profile; written to our copy.
    // DC-DEFAULT: CLI-553 (proposed)
    pub fn for_client(rom: Option<&Path>, own_dir: Option<&Path>) -> ProfileStore {
        let own = own_dir.map(|d| d.join(ROM_PROFILE[0]).join(ROM_PROFILE[1]));
        let mut read: Vec<PathBuf> = own.iter().cloned().collect();
        if let Some(r) = rom {
            read.push(r.join(ROM_PROFILE[0]).join(ROM_PROFILE[1]));
        }
        ProfileStore {
            read,
            write: own,
            key: rom.and_then(dc_render::profile_key),
        }
    }

    /// The profile text and where it came from (`None`: no readable profile).
    pub fn load(&self) -> Option<(ProfileDoc, PathBuf)> {
        for p in &self.read {
            let Ok(raw) = std::fs::read(p) else {
                continue;
            };
            let plain = match &self.key {
                Some(k) => match dc_rez::decrypt_with_key(&raw, k) {
                    Ok(b) => b,
                    Err(_) => continue,
                },
                None => raw,
            };
            let text: String = plain.iter().map(|&b| b as char).collect();
            if !text.contains('[') {
                continue;
            }
            return Some((ProfileDoc::parse(&text), p.clone()));
        }
        None
    }

    /// Writes `doc` to our copy (encrypted like the 2009 file).
    pub fn save(&self, doc: &ProfileDoc) -> Result<PathBuf, String> {
        let p = self.write.clone().ok_or("no profile folder")?;
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let plain: Vec<u8> = doc
            .text()
            .chars()
            .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
            .collect();
        let bytes = match &self.key {
            Some(k) => dc_rez::encrypt_with_key(&plain, k).map_err(|e| e.to_string())?,
            None => plain,
        };
        std::fs::write(&p, bytes).map_err(|e| format!("{}: {e}", p.display()))?;
        Ok(p)
    }
}

// ----------------------------------------------------------------------------------------------- the live copy

/// The settings in effect, for the game window's key map (the menu publishes them at start and after OK).
static CURRENT: RwLock<Option<KeyBinds>> = RwLock::new(None);

/// Makes `s`'s key bindings the ones the game window maps keys with.
pub fn publish(s: &Settings) {
    if let Ok(mut c) = CURRENT.write() {
        *c = Some(s.binds.clone());
    }
    BORDERED.store(s.window == WinStyle::Bordered, Ordering::Relaxed);
    NET_OVERLAY.store(s.net_overlay as i32, Ordering::Relaxed);
    DISPLAY_SAVED.store(if s.display_saved { 2 } else { 1 }, Ordering::Relaxed);
    ANTIALIAS.store(s.antialias, Ordering::Relaxed);
    crate::pacer::set_setting_cap(s.fps_cap);
    SHOW_FPS.store(s.show_fps, Ordering::Relaxed);
    BGM_CLASSIC_ON.store(s.bgm == BGM_CLASSIC, Ordering::Relaxed);
    if let Ok(mut g) = XRAY.write() {
        *g = Some(s.xray);
    }
    publish_gamma(s.gamma);
}

/// CLI-1594: the published anti-aliasing sample count (1 = off); the next match window's renderer takes it.
static ANTIALIAS: AtomicU32 = AtomicU32::new(1);
/// CLI-1596: the published "Show FPS" switch.
static SHOW_FPS: AtomicBool = AtomicBool::new(false);

/// The anti-aliasing sample count in effect (1 = off).
pub fn antialias() -> u32 {
    ANTIALIAS.load(Ordering::Relaxed)
}

/// Whether the frame rate is drawn in the match and in the front end.
pub fn show_fps() -> bool {
    SHOW_FPS.load(Ordering::Relaxed)
}

/// CLI-1599: the published BGM style: whether the classic tracks play (the default until a profile is read).
static BGM_CLASSIC_ON: AtomicBool = AtomicBool::new(BGM_DEFAULT == BGM_CLASSIC);

/// Whether the front end plays the classic tracks (the sound director takes it when it opens).
pub fn bgm_classic() -> bool {
    BGM_CLASSIC_ON.load(Ordering::Relaxed)
}

/// CLI-1595: whether the frame-rate setting asks for vertical sync (the display's rate).
pub fn fps_vsync() -> bool {
    crate::pacer::setting_cap() == FPS_VSYNC
}

/// CLI-1140: the gamma (hundredths above 1) the window draws through; -1 until a profile or the options page set one.
static GAMMA: AtomicI32 = AtomicI32::new(-1);

/// Makes `hundredths` the gamma of the frame now (the options slider moves it before OK; Cancel puts the old back).
pub fn publish_gamma(hundredths: i32) {
    GAMMA.store(hundredths.clamp(0, GAMMA_MAX), Ordering::Relaxed);
}

/// The published gamma, `None` before any was published (the window then keeps the profile's).
pub fn published_gamma() -> Option<f32> {
    let v = GAMMA.load(Ordering::Relaxed);
    (v >= 0).then(|| 1.0 + v as f32 / 100.0)
}

/// CLI-1031: a sensitivity the match changed (its hotkeys) that the menu has not taken yet (-1: none).
static SENSITIVITY_NOTE: AtomicI32 = AtomicI32::new(-1);

/// The match changed the sensitivity to `v` (the menu takes it into the settings and the profile).
pub fn note_sensitivity(v: i32) {
    SENSITIVITY_NOTE.store(v.clamp(0, SENSITIVITY_MAX), Ordering::Relaxed);
}

/// The sensitivity the match changed since the last call.
pub fn take_sensitivity_note() -> Option<i32> {
    let v = SENSITIVITY_NOTE.swap(-1, Ordering::Relaxed);
    (v >= 0).then_some(v)
}

/// The sensitivity one hotkey step from `cur` (within the slider's range).
// DC-DEFAULT: CLI-1031 (proposed)
pub fn step_sensitivity(cur: f32, step: i32) -> f32 {
    (cur.round() as i32 + step).clamp(0, SENSITIVITY_MAX) as f32
}

/// CLI-1570: our own section's key of the network overlay's mode.
// DC-DEFAULT: CLI-1570 (proposed)
pub const OWN_NET_OVERLAY: &str = "net_overlay";
pub const OWN_ANTIALIAS: &str = "antialias";
pub const OWN_FPS_CAP: &str = "fps_cap";
pub const OWN_SHOW_FPS: &str = "show_fps";
pub const OWN_BGM: &str = "bgm";
/// CLI-1852: our own section's key of the Renderer row (`vulkan` / `dx12`).
// DC-DEFAULT: CLI-1852 (proposed)
pub const OWN_RENDERER: &str = "renderer";
/// CLI-1916: our own section's key of the Field of View row (degrees, a [`FOV_CHOICES`] entry).
// DC-DEFAULT: CLI-1916 (proposed)
pub const OWN_FOV: &str = "fov";
/// CLI-1617 / CLI-1618: our own section's keys of the enabled local mods and the accepted server mods.
// DC-DEFAULT: CLI-1617, CLI-1618 (proposed)
pub const OWN_MODS_LOCAL_ENABLED: &str = "mods.local_enabled";
/// MOD-24: `mods.setting.<mod id>.<key> = <value>`.
pub const OWN_MODS_SETTING_PREFIX: &str = "mods.setting.";
pub const OWN_MODS_ACCEPTED: &str = "mods.accepted";
/// CLI-1624: our own section's keys of the x-ray style (`box` / `chams` / `skeleton`, `rrggbbaa`).
pub const OWN_XRAY_MODE: &str = "xray.mode";
pub const OWN_XRAY_PRIMARY: &str = "xray.primary";
pub const OWN_XRAY_SECONDARY: &str = "xray.secondary";
/// CLI-1630: the name-tag and health-bar switches of the x-ray (`0` / `1`).
// DC-DEFAULT: CLI-1630 (proposed, USER REQUEST 2026-11-01) both off by default
pub const OWN_XRAY_NAMETAGS: &str = "xray.nametags";
pub const OWN_XRAY_HEALTH: &str = "xray.health";
/// CLI-1635: the team mates' x-ray switch (`0` / `1`, default on).
// DC-DEFAULT: CLI-1635 (proposed, USER REQUEST 2026-11-01)
pub const OWN_XRAY_TEAMMATES: &str = "xray.teammates";
/// CLI-1841: the diagnostic reports' consent (`0`..`3`).
// DC-DEFAULT: CLI-1841 (proposed, USER REQUEST 2026-11-02)
pub const OWN_DIAG_CONSENT: &str = "diag.consent";

/// CLI-1624: the published x-ray style (the menu publishes the profile's; `/wallhack` changes it in the match), and a
/// style the match changed that the menu has not saved yet.
static XRAY: RwLock<Option<XrayStyle>> = RwLock::new(None);
static XRAY_NOTE: RwLock<Option<XrayStyle>> = RwLock::new(None);

/// The x-ray style in effect (the default before any profile was read).
pub fn xray_style() -> XrayStyle {
    XRAY.read().ok().and_then(|g| *g).unwrap_or_default()
}

/// CLI-1625: the match changed the x-ray style (`/wallhack`): it is in effect at once, and the menu saves it.
pub fn note_xray_style(style: XrayStyle) {
    if let Ok(mut g) = XRAY.write() {
        *g = Some(style);
    }
    if let Ok(mut g) = XRAY_NOTE.write() {
        *g = Some(style);
    }
}

/// The x-ray style the match changed since the last call.
pub fn take_xray_note() -> Option<XrayStyle> {
    XRAY_NOTE.write().ok().and_then(|mut g| g.take())
}

/// CLI-1907: the settings the options over the match confirmed (OK) or the Mods tab changed, which the menu has not
/// taken yet.
static MATCH_SETTINGS_NOTE: std::sync::Mutex<Option<Settings>> = std::sync::Mutex::new(None);

/// CLI-1907: the options over the match changed the settings to `s` (in effect in the match already): the menu
/// makes them its settings and saves the profile ([`crate::menu::Menu::store_match_settings`]).
// DC-DEFAULT: CLI-1907 (proposed) the menu saves what the match's options changed, as it saves the hotkeys' changes
pub fn note_match_settings(s: Settings) {
    if let Ok(mut g) = MATCH_SETTINGS_NOTE.lock() {
        *g = Some(s);
    }
}

/// The settings the options over the match changed since the last call.
pub fn take_match_settings_note() -> Option<Settings> {
    MATCH_SETTINGS_NOTE.lock().ok().and_then(|mut g| g.take())
}

/// CLI-1570: the overlay mode the next match starts with (-1: nothing published), the mode a flag or `DC_NET_OVERLAY`
/// forces (-1: none), and a mode the match changed that the menu has not saved yet (-1: none).
static NET_OVERLAY: AtomicI32 = AtomicI32::new(-1);
static NET_OVERLAY_FORCED: AtomicI32 = AtomicI32::new(-1);
static NET_OVERLAY_NOTE: AtomicI32 = AtomicI32::new(-1);

/// `--net-overlay[=N]`: every match of this run starts with mode `mode` (the chat command still changes it).
pub fn force_net_overlay(mode: u8) {
    NET_OVERLAY_FORCED.store(mode.min(2) as i32, Ordering::Relaxed);
}

/// The mode of `DC_NET_OVERLAY` (`1` / `2`; `0` off; anything else: none).
pub fn net_overlay_env(v: Option<&str>) -> Option<u8> {
    match v?.trim() {
        "0" => Some(0),
        "1" | "on" => Some(1),
        "2" => Some(2),
        _ => None,
    }
}

/// The overlay mode a match starts with: the flag, else `DC_NET_OVERLAY`, else the last mode a match chose or the
/// profile's (0 without any).
pub fn net_overlay_start() -> u8 {
    let forced = NET_OVERLAY_FORCED.load(Ordering::Relaxed);
    if forced >= 0 {
        return forced as u8;
    }
    if let Some(m) = net_overlay_env(std::env::var("DC_NET_OVERLAY").ok().as_deref()) {
        return m;
    }
    NET_OVERLAY.load(Ordering::Relaxed).clamp(0, 2) as u8
}

/// The match changed the overlay mode (`/network`): the next match starts with it and the menu saves it.
pub fn note_net_overlay(mode: u8) {
    let m = mode.min(2) as i32;
    NET_OVERLAY.store(m, Ordering::Relaxed);
    NET_OVERLAY_NOTE.store(m, Ordering::Relaxed);
    if NET_OVERLAY_FORCED.load(Ordering::Relaxed) >= 0 {
        NET_OVERLAY_FORCED.store(m, Ordering::Relaxed);
    }
}

/// The overlay mode the match changed since the last call.
pub fn take_net_overlay_note() -> Option<u8> {
    let v = NET_OVERLAY_NOTE.swap(-1, Ordering::Relaxed);
    (v >= 0).then_some(v as u8)
}

/// The window mode in effect (the windows opened from now on take it).
static BORDERED: AtomicBool = AtomicBool::new(false);

/// The published window mode is the bordered one.
pub fn published_bordered() -> bool {
    BORDERED.load(Ordering::Relaxed)
}

/// Our own section's keys of the window look and the resolution setting (CLI-1281).
// DC-DEFAULT: CLI-1281 (proposed)
pub const OWN_WINDOW: &str = "window";
pub const OWN_WINDOW_WIDTH: &str = "window_width";
pub const OWN_WINDOW_HEIGHT: &str = "window_height";

/// CLI-1280: 0 = no settings published (no profile store: nothing is saved), 1 = published without saved display
/// values (the first window is a first launch), 2 = published with them.
static DISPLAY_SAVED: AtomicI32 = AtomicI32::new(0);

/// Whether the published settings hold a saved look and resolution (`None`: no settings were published).
pub fn published_display_saved() -> Option<bool> {
    match DISPLAY_SAVED.load(Ordering::Relaxed) {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    }
}

/// A display change the window made that the menu saves (the first launch's look and monitor size, an Alt+Enter).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DisplayNote {
    /// The look now (`Some(true)`: windowed).
    pub bordered: Option<bool>,
    /// The resolution setting now.
    pub resolution: Option<(u32, u32)>,
}

impl DisplayNote {
    /// The note's values in `s` (true when something changed); with both values saved the display counts as saved.
    pub fn apply(&self, s: &mut Settings) -> bool {
        let before = s.clone();
        if let Some(b) = self.bordered {
            s.window = if b {
                WinStyle::Bordered
            } else {
                WinStyle::Borderless
            };
        }
        if let Some(r) = self.resolution {
            s.resolution = Some(r);
        }
        s.display_saved = s.resolution.is_some();
        *s != before
    }
}

static DISPLAY_NOTE: std::sync::Mutex<Option<DisplayNote>> = std::sync::Mutex::new(None);

/// The window changed the look (`bordered`) and / or the resolution setting: the menu saves it (CLI-1280).
pub fn note_display(bordered: Option<bool>, resolution: Option<(u32, u32)>) {
    if let Ok(mut n) = DISPLAY_NOTE.lock() {
        let mut v = n.unwrap_or_default();
        if bordered.is_some() {
            v.bordered = bordered;
        }
        if resolution.is_some() {
            v.resolution = resolution;
        }
        *n = Some(v);
    }
}

/// The display change noted since the last call.
pub fn take_display_note() -> Option<DisplayNote> {
    DISPLAY_NOTE.lock().ok().and_then(|mut n| n.take())
}

/// What scan code `scan` does with the published bindings (the defaults before any were published). The one
/// function the game window maps its keys through.
// DC-DEFAULT: CLI-555 (proposed)
pub fn bound_input(scan: u8) -> Option<Bound> {
    match CURRENT.read().ok().and_then(|c| c.clone()) {
        Some(b) => b.lookup(scan),
        None => KeyBinds::default().lookup(scan),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[Display]\r\nScreenWidth\t\t= 1920\r\nScreenHeight\t\t= 1080\r\n\r\n[Controls]\r\nInvertMouse\t\t\t= 0\r\nSensitivity\t\t\t= 10\r\n\r\n[Bindings]\r\nForward = \"##17@ @ \"\r\n//Old = \"##1@ @ \"\r\nJump = \"##57@ @ \"\r\nFire = \" @##3@ \"\r\n\r\n[Sound]\r\nsoundenable = TRUE\r\nmusicenable = TRUE\r\nmusicvolume = 59\r\nsoundvolume = 61\r\n";

    fn table() -> CommandTable {
        CommandTable {
            rows: vec![
                CommandRow {
                    label: 1,
                    command: 0,
                    name: 100,
                    group: 0,
                },
                CommandRow {
                    label: 2,
                    command: 6,
                    name: 101,
                    group: 0,
                },
                CommandRow {
                    label: 3,
                    command: 8,
                    name: 102,
                    group: 1,
                },
                CommandRow {
                    label: 4,
                    command: 3,
                    name: 103,
                    group: 1,
                },
            ],
        }
    }

    fn names(id: i32) -> String {
        match id {
            100 => "Forward",
            101 => "Jump",
            102 => "Fire",
            103 => "Reload",
            _ => "",
        }
        .into()
    }

    #[test]
    fn profile_values_are_read_and_written_back_in_place() {
        let doc = ProfileDoc::parse(SAMPLE);
        let s = Settings::from_doc(&doc, &table(), &names);
        assert_eq!((s.effects_volume, s.music_volume), (61, 59));
        assert!(s.effects_on && s.music_on);
        assert_eq!(s.sensitivity, 10);
        assert!(!s.invert_y);
        // CLI-1281: the 2009 [Display] size is not our resolution setting
        assert_eq!(s.resolution, None);
        assert!(!s.display_saved);
        assert_eq!(s.binds.key_of(0), Some(17));
        assert_eq!(s.binds.key_of(6), Some(57));
        assert_eq!(s.binds.key_of(8), None, "mouse-only command");
        // reload is not in the profile: its default key stays
        assert_eq!(s.binds.key_of(3), Some(19));

        let mut t = s.clone();
        t.effects_volume = 30;
        t.music_on = false;
        t.invert_y = true;
        t.binds.bind(0, Some(200));
        t.binds.bind(CMD_INSPECT, Some(34));
        t.inert.insert("graphic.3".into(), 2);
        t.net_overlay = 2;
        t.antialias = 4;
        t.fps_cap = 120;
        t.show_fps = true;
        t.renderer = Renderer::Vulkan;
        t.bgm = BGM_2009;
        let mut d2 = doc.clone();
        t.to_doc(&mut d2, &table(), &names);
        let text = d2.text();
        assert!(text.contains("soundvolume = 30"), "{text}");
        assert!(text.contains("musicenable = FALSE"));
        assert!(text.contains("InvertMouse\t\t\t= 1"), "tabs kept: {text}");
        assert!(text.contains("Forward = \"##200@ @ \""));
        assert!(text.contains("Fire = \" @##3@ \""), "mouse field untouched");
        assert!(text.contains("//Old = \"##1@ @ \""), "comment kept");
        assert!(text.contains("[DeCombat]"));
        assert!(text.contains("bind.Reload = \"##19@ @ \""));
        let back = Settings::from_doc(&ProfileDoc::parse(&text), &table(), &names);
        assert_eq!(back.effects_volume, 30);
        assert!(!back.music_on && back.invert_y);
        assert_eq!(back.binds.key_of(0), Some(200));
        assert_eq!(back.binds.key_of(CMD_INSPECT), Some(34), "our own key is kept");
        assert_eq!(back.inert.get("graphic.3"), Some(&2));
        assert_eq!(back.net_overlay, 2, "CLI-1570 kept in our own section");
        assert_eq!(
            (back.antialias, back.fps_cap, back.show_fps),
            (4, 120, true),
            "CLI-1594..1596 kept in our own section"
        );
        assert_eq!(back.renderer, Renderer::Vulkan, "CLI-1852 kept in our own section");
        assert_eq!(s.renderer, Renderer::Dx12, "CLI-1852: DirectX 12 without a key");
        assert_eq!(back.bgm, BGM_2009, "CLI-1599 kept in our own section");
        assert_eq!(s.bgm, BGM_CLASSIC, "CLI-1599: Classic without a key");
        assert_eq!(back.effective_music(), 0);
    }

    /// CLI-1852: the Renderer row's profile word: DirectX 12 by default, Vulkan saved, anything else the default.
    #[test]
    fn the_renderer_choice_is_saved_and_unknown_values_fall_back() {
        let load = |extra: &str| {
            let text = format!("{SAMPLE}
[DeCombat]
{extra}");
            Settings::from_doc(&ProfileDoc::parse(&text), &table(), &names)
        };
        assert_eq!(RENDERER_DEFAULT, Renderer::Dx12);
        assert_eq!(Settings::default().renderer, Renderer::Dx12);
        assert_eq!(load("").renderer, Renderer::Dx12, "no key");
        assert_eq!(load("renderer = dx12
").renderer, Renderer::Dx12);
        assert_eq!(load("renderer = \"DX12\"
").renderer, Renderer::Dx12);
        assert_eq!(load("renderer = vulkan
").renderer, Renderer::Vulkan);
        for old in ["metal", "3", "", "dx11"] {
            assert_eq!(load(&format!("renderer = {old}
")).renderer, Renderer::Dx12, "{old:?}");
        }
        let mut doc = ProfileDoc::parse(SAMPLE);
        let mut s = Settings::default();
        s.renderer = Renderer::Vulkan;
        s.to_doc(&mut doc, &table(), &names);
        assert_eq!(doc.own_get(OWN_RENDERER), Some("vulkan"));
        assert_eq!((Renderer::Vulkan.label(), Renderer::Dx12.label()), ("Vulkan", "DirectX 12"));
        assert_eq!(Renderer::Dx12.backends(), wgpu::Backends::DX12);
    }

    /// CLI-1916: the Field of View row's profile number: 70..120 in steps of 5, anything else the 2009 view (90).
    #[test]
    fn the_field_of_view_is_saved_and_unknown_values_fall_back() {
        let load = |extra: &str| {
            let text = format!("{SAMPLE}\n[DeCombat]\n{extra}");
            Settings::from_doc(&ProfileDoc::parse(&text), &table(), &names)
        };
        assert_eq!(Settings::default().fov, 90);
        assert_eq!(load("").fov, 90, "no key");
        for v in FOV_CHOICES {
            assert_eq!(load(&format!("fov = {v}\n")).fov, v);
        }
        assert_eq!(FOV_CHOICES.len(), 11);
        assert_eq!((FOV_CHOICES[0], FOV_CHOICES[10]), (70, 120));
        for old in ["69", "72", "125", "0", "-90", "wide", ""] {
            assert_eq!(load(&format!("fov = {old}\n")).fov, 90, "{old:?}");
        }
        let mut doc = ProfileDoc::parse(SAMPLE);
        let mut s = Settings::default();
        s.fov = 110;
        s.to_doc(&mut doc, &table(), &names);
        assert_eq!(doc.own_get(OWN_FOV), Some("110"));
        let back = Settings::from_doc(&ProfileDoc::parse(&doc.text()), &table(), &names);
        assert_eq!(back.fov, 110);
    }

    /// CLI-1852: `--adapter` wins over the setting (and stays exclusive); the setting is a first attempt; builds
    /// without the choice keep Auto.
    #[test]
    fn the_adapter_option_wins_over_the_renderer_setting() {
        use dc_render::AdapterChoice::{Auto, Backends, Fallback};
        let dx = Backends(wgpu::Backends::DX12);
        let vk = Backends(wgpu::Backends::VULKAN);
        assert_eq!(adapter_for_run(None, Renderer::Vulkan, true), (vk, true));
        assert_eq!(adapter_for_run(None, Renderer::Dx12, true), (dx, true));
        assert_eq!(adapter_for_run(Some(dx), Renderer::Vulkan, true), (dx, false));
        assert_eq!(adapter_for_run(Some(Auto), Renderer::Dx12, true), (Auto, false));
        assert_eq!(adapter_for_run(Some(Fallback), Renderer::Dx12, true), (Fallback, false));
        assert_eq!(adapter_for_run(None, Renderer::Dx12, false), (Auto, false), "no choice off Windows");
        assert_eq!(renderer_choosable(), cfg!(windows));
    }

    #[test]
    fn the_look_and_resolution_live_in_our_own_section() {
        // CLI-1280 / CLI-1281: a first launch saves full screen at the monitor's size; the 2009 entries stay
        let doc = ProfileDoc::parse(SAMPLE);
        let mut s = Settings::from_doc(&doc, &table(), &names);
        assert!(!s.display_saved);
        let first = DisplayNote {
            bordered: Some(false),
            resolution: Some((3840, 2160)),
        };
        assert!(first.apply(&mut s));
        assert!(s.display_saved);
        assert_eq!(
            (s.window, s.resolution),
            (WinStyle::Borderless, Some((3840, 2160)))
        );
        let mut d2 = doc.clone();
        s.to_doc(&mut d2, &table(), &names);
        let back = Settings::from_doc(&ProfileDoc::parse(&d2.text()), &table(), &names);
        assert!(back.display_saved);
        assert_eq!(
            (back.window, back.resolution),
            (WinStyle::Borderless, Some((3840, 2160)))
        );
        assert_eq!(
            Settings::from_doc(&ProfileDoc::parse(SAMPLE), &table(), &names).resolution,
            back_2009_untouched(&d2)
        );
        // an Alt+Enter saves the look and keeps the resolution
        let mut t = back.clone();
        let alt = DisplayNote {
            bordered: Some(true),
            resolution: None,
        };
        assert!(alt.apply(&mut t));
        assert_eq!(
            (t.window, t.resolution),
            (WinStyle::Bordered, Some((3840, 2160)))
        );
        assert!(!alt.apply(&mut t), "the same look again changes nothing");
        // a saved windowed 1920x1080 reads back as such
        t.resolution = Some((1920, 1080));
        let mut d3 = doc.clone();
        t.to_doc(&mut d3, &table(), &names);
        let back = Settings::from_doc(&ProfileDoc::parse(&d3.text()), &table(), &names);
        assert!(back.display_saved);
        assert_eq!(
            (back.window, back.resolution),
            (WinStyle::Bordered, Some((1920, 1080)))
        );
        // a look without a resolution is not a saved display
        let mut d4 = ProfileDoc::parse(SAMPLE);
        d4.own_set(OWN_WINDOW, "1");
        assert!(!Settings::from_doc(&d4, &table(), &names).display_saved);
    }

    /// The 2009 `[Display]` entries of `d` read as before (our writes never touch them): `None` as our setting.
    fn back_2009_untouched(d: &ProfileDoc) -> Option<(u32, u32)> {
        let w = d.get(keys::DISPLAY, keys::WIDTH).and_then(int_of);
        let h = d.get(keys::DISPLAY, keys::HEIGHT).and_then(int_of);
        assert_eq!(
            (w, h),
            (Some(1920), Some(1080)),
            "2009 [Display] entries kept"
        );
        None
    }

    #[test]
    fn binding_fields() {
        assert_eq!(keyboard_field("\"##17@ @ \""), Some(17));
        assert_eq!(keyboard_field("\" @##3@ \""), None);
        assert_eq!(
            with_keyboard_field("\" @##3@ \"", Some(30)),
            "\"##30@##3@ \""
        );
        assert_eq!(with_keyboard_field("", Some(30)), "\"##30@ @ \"");
        assert_eq!(with_keyboard_field("\"##30@ @ \"", None), "\" @ @ \"");
    }

    #[test]
    fn default_binds_are_the_old_key_maps_and_a_rebind_moves_the_key() {
        let b = KeyBinds::default();
        assert_eq!(b.lookup(17), Some(Bound::Action(Action::Forward)));
        assert_eq!(b.lookup(29), Some(Bound::Action(Action::Crouch)));
        assert_eq!(b.lookup(3), Some(Bound::Slot(1)));
        assert_eq!(b.lookup(19), Some(Bound::Reload));
        assert_eq!(b.lookup(44), Some(Bound::Action(Action::Zoom)));
        assert_eq!(b.lookup(100), None);
        let mut c = b.clone();
        c.bind(6, Some(17));
        assert_eq!(c.lookup(17), Some(Bound::Action(Action::Jump)));
        assert_eq!(c.key_of(0), None, "forward lost W");
        assert_eq!(c.lookup(57), None);
    }

    /// CLI-1031 (round 11: the sensitivity hotkeys did nothing in the match): `]` / `[` are SensitivityInc / Dec by
    /// default and one press moves the value one step within the slider's range.
    #[test]
    fn the_sensitivity_keys_are_bound_and_step_within_the_range() {
        let b = KeyBinds::default();
        assert_eq!(b.lookup(27), Some(Bound::Sensitivity(1)));
        assert_eq!(b.lookup(26), Some(Bound::Sensitivity(-1)));
        assert_eq!(step_sensitivity(10.0, 1), 11.0);
        assert_eq!(step_sensitivity(10.0, -1), 9.0);
        assert_eq!(
            step_sensitivity(SENSITIVITY_MAX as f32, 1),
            SENSITIVITY_MAX as f32
        );
        assert_eq!(step_sensitivity(0.0, -1), 0.0);
    }

    #[test]
    fn store_round_trip_with_and_without_a_key() {
        let dir = std::env::temp_dir().join(format!("dc_settings_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let rom_like = dir.join("rom");
        std::fs::create_dir_all(rom_like.join("Profiles")).unwrap();
        let key = b"testkey-0123".to_vec();
        std::fs::write(
            rom_like.join("Profiles").join("player.txt"),
            dc_rez::encrypt_with_key(SAMPLE.as_bytes(), &key).unwrap(),
        )
        .unwrap();
        let mut store = ProfileStore::for_client(Some(&rom_like), Some(&dir.join("own")));
        store.key = Some(key);
        let (doc, from) = store.load().unwrap();
        assert!(from.starts_with(&rom_like));
        let mut s = Settings::from_doc(&doc, &table(), &names);
        s.music_volume = 7;
        let mut d = doc.clone();
        s.to_doc(&mut d, &table(), &names);
        let written = store.save(&d).unwrap();
        assert!(written.starts_with(dir.join("own")));
        let (doc2, from2) = store.load().unwrap();
        assert_eq!(from2, written, "our copy is read first");
        assert_eq!(Settings::from_doc(&doc2, &table(), &names).music_volume, 7);
        // the client folder's file is unchanged
        let rom_doc = ProfileDoc::parse(&String::from_utf8_lossy(
            &dc_rez::decrypt_with_key(
                &std::fs::read(rom_like.join("Profiles").join("player.txt")).unwrap(),
                store.key.as_ref().unwrap(),
            )
            .unwrap(),
        ));
        assert_eq!(rom_doc.get(keys::SOUND, keys::MUSIC_VOLUME), Some("59"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // CLI-1286: the profile folder without a variable follows the platform
    #[test]
    fn the_inspect_key_is_i_by_default_and_can_be_moved() {
        let mut k = KeyBinds::default();
        assert_eq!(k.key_of(CMD_INSPECT), Some(23));
        assert_eq!(k.lookup(23), Some(Bound::Inspect));
        // another key takes it over, and the old key does nothing any more
        k.bind(CMD_INSPECT, Some(34));
        assert_eq!(k.lookup(34), Some(Bound::Inspect));
        assert_eq!(k.lookup(23), None);
        // a key it takes from another command leaves that command unbound
        k.bind(CMD_INSPECT, Some(19));
        assert_eq!(k.key_of(3), None, "reload lost R");
        assert_eq!(k.lookup(19), Some(Bound::Inspect));
    }

    // CLI-1950: the ping key: the middle mouse button by default, kept in our own section, unbound kept as unbound
    #[test]
    fn the_ping_key_is_the_middle_button_by_default_and_round_trips() {
        let k = KeyBinds::default();
        assert_eq!(k.key_of(CMD_PING), Some(KEY_MOUSE_MIDDLE));
        assert_eq!(k.lookup(KEY_MOUSE_MIDDLE), Some(Bound::Ping));
        assert_eq!(scan_name(KEY_MOUSE_MIDDLE), "Mouse 3");
        let doc = ProfileDoc::parse(SAMPLE);
        let base = Settings::from_doc(&doc, &table(), &names);
        assert_eq!(base.binds.key_of(CMD_PING), Some(KEY_MOUSE_MIDDLE));
        for key in [Some(KEY_MOUSE_MIDDLE), Some(34), None] {
            let mut t = base.clone();
            t.binds.bind(CMD_PING, key);
            let mut d = doc.clone();
            t.to_doc(&mut d, &table(), &names);
            let text = d.text();
            assert!(
                text.contains(&format!("bind_ping = {}", key.map_or(0, i32::from))),
                "{text}"
            );
            assert!(!text.contains("##255"), "no 2009 section holds the pseudo key");
            let back = Settings::from_doc(&ProfileDoc::parse(&text), &table(), &names);
            assert_eq!(back.binds.key_of(CMD_PING), key);
        }
    }

    #[test]
    fn the_platform_profile_dir_is_application_support_on_macos() {
        let home = std::ffi::OsStr::new("/Users/me");
        assert_eq!(
            platform_profile_dir(home, true),
            PathBuf::from("/Users/me/Library/Application Support/DeCombat")
        );
        assert_eq!(
            platform_profile_dir(home, false),
            PathBuf::from("/Users/me/.config/decombat")
        );
    }
}
