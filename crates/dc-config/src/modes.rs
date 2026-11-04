//! DeCombat's mode-name vocabulary: the names `modes.playable` takes, the ROM mode class each name maps to, the
//! inverse, and the `serves` rule a community map's spawn / objective sets follow (MAPKIT_PLAN.md section 4).
//!
//! The classes are the 2009 client's `GameType` constants (PROVEN); the names are ours and shared by the lobby
//! (`modes.playable`, the 0x311 masks), the game server (a `dc.*` object's `modes` filter) and map records
//! (`mod.content.maps:register{modes = {...}}`), so every reader spells a mode the same way.
// SPEC: GAME_MODES_2009.md §2 (the classes are PROVEN client constants, the names are ours)
// DC-DEFAULT: MAP-6 (proposed), see DC_DEFAULTS.md

/// Our name -> ROM mode class, in the order the names are listed to users. `CLASS10` is a class with no DeCombat
/// mode behind it (it is not playable and not a map-record mode) and stays out of [`MODE_NAMES`].
const MODE_TABLE: &[(&str, u32)] = &[
    ("TDM", 1),
    ("TDM_CLAN", 4),
    ("COOP", 5),
    ("FFA", 6),
    ("ELIM", 12),
    ("ELIM_CLAN", 13),
    ("FLAG", 7),
    ("FLAG_CLAN", 8),
    ("BOMB", 2),
    ("BOMB_CLAN", 3),
    ("INFECT", 11),
    ("BOUNTY", 9),
];

/// The DeCombat mode names: the default of `modes.playable` (GMD-26) and the vocabulary of a map record's `modes`
/// list and of a `dc.*` object's `modes` filter. Spelled exactly like this in map records; `modes.playable` and
/// the mode filters accept any letter case.
// DC-DEFAULT: GMD-26 (amended by project decision: the other modes are to be played) `modes.playable` =
// TDM, TDM_CLAN, COOP, FFA, ELIM, ELIM_CLAN, FLAG, FLAG_CLAN, BOMB, BOMB_CLAN, INFECT, BOUNTY; see DC_DEFAULTS.md
pub const MODE_NAMES: &[&str] = &[
    "TDM",
    "TDM_CLAN",
    "COOP",
    "FFA",
    "ELIM",
    "ELIM_CLAN",
    "FLAG",
    "FLAG_CLAN",
    "BOMB",
    "BOMB_CLAN",
    "INFECT",
    "BOUNTY",
];

/// True when `name` is one of [`MODE_NAMES`], spelled exactly (upper case, as a map record lists it).
pub fn is_mode_name(name: &str) -> bool {
    MODE_NAMES.contains(&name)
}

/// Our name -> ROM mode class (`GameType` of the `[Mode<n>]` / `[ClanMode<n>]` row), any letter case. A decimal
/// class number is accepted too, so a mod's own row can be listed in `modes.playable`.
pub fn mode_class_of_name(name: &str) -> Option<u32> {
    let n = name.trim();
    if let Ok(c) = n.parse::<u32>() {
        return Some(c);
    }
    let upper = n.to_ascii_uppercase();
    if upper == "CLASS10" {
        return Some(10);
    }
    MODE_TABLE
        .iter()
        .find(|(our, _)| *our == upper)
        .map(|(_, class)| *class)
}

/// The inverse of [`mode_class_of_name`]: every DeCombat name of a ROM mode class, in [`MODE_NAMES`] order (empty
/// for a class no name maps to, such as 10). The first entry is the canonical name a server uses to turn a room
/// line's `type=` class back into a name.
pub fn mode_names_of_class(class: u32) -> &'static [&'static str] {
    // The table is small; the slices below are the contiguous runs of a class, kept in one place per class.
    match class {
        1 => &MODE_NAMES[0..1],
        4 => &MODE_NAMES[1..2],
        5 => &MODE_NAMES[2..3],
        6 => &MODE_NAMES[3..4],
        12 => &MODE_NAMES[4..5],
        13 => &MODE_NAMES[5..6],
        7 => &MODE_NAMES[6..7],
        8 => &MODE_NAMES[7..8],
        2 => &MODE_NAMES[8..9],
        3 => &MODE_NAMES[9..10],
        11 => &MODE_NAMES[10..11],
        9 => &MODE_NAMES[11..12],
        _ => &[],
    }
}

/// The canonical DeCombat name of a ROM mode class (the first of [`mode_names_of_class`]).
pub fn mode_name_of_class(class: u32) -> Option<&'static str> {
    mode_names_of_class(class).first().copied()
}

/// The raw name -> class mapping, for readers that want the whole table (tests, listings).
pub fn mode_table() -> &'static [(&'static str, u32)] {
    MODE_TABLE
}

/// A mode name without its `_CLAN` suffix, upper-cased.
fn base_name(name: &str) -> String {
    let upper = name.trim().to_ascii_uppercase();
    upper
        .strip_suffix("_CLAN")
        .map(str::to_string)
        .unwrap_or(upper)
}

/// Whether a spawn / objective set tagged `set_name` (a `dc.*` object's `modes` entry or a map record's mode)
/// serves a room playing `room_name`. True when the names are equal (any letter case), when they are `_CLAN`
/// twins (`TDM` serves `TDM_CLAN` and the other way round), or when the set is `BOMB` and the room is `BOMB_CLAN`,
/// `ELIM` or `ELIM_CLAN` (the bomb sites and their spawns are the elimination layout, DSV-470 semantics). An
/// `ELIM` set does not serve a `BOMB` room.
// DC-DEFAULT: MAP-6 (proposed), see DC_DEFAULTS.md; DSV-470 for the BOMB / ELIM rule
pub fn serves(set_name: &str, room_name: &str) -> bool {
    let set = base_name(set_name);
    let room = base_name(room_name);
    if set.is_empty() {
        return false;
    }
    set == room || (set == "BOMB" && room == "ELIM")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_round_trips_through_its_class() {
        for &(name, class) in MODE_TABLE {
            assert_eq!(mode_class_of_name(name), Some(class), "{name}");
            assert_eq!(
                mode_class_of_name(&name.to_ascii_lowercase()),
                Some(class),
                "{name}"
            );
            assert_eq!(mode_names_of_class(class), &[name], "class {class}");
            assert_eq!(mode_name_of_class(class), Some(name));
            assert!(is_mode_name(name));
        }
        assert_eq!(MODE_NAMES.len(), MODE_TABLE.len());
        assert_eq!(
            MODE_NAMES.to_vec(),
            MODE_TABLE.iter().map(|(n, _)| *n).collect::<Vec<_>>()
        );
    }

    #[test]
    fn numbers_and_class10_are_classes_without_a_name() {
        assert_eq!(mode_class_of_name("7"), Some(7));
        assert_eq!(mode_class_of_name(" 42 "), Some(42));
        assert_eq!(mode_class_of_name("class10"), Some(10));
        assert_eq!(mode_names_of_class(10), &[] as &[&str]);
        assert_eq!(mode_name_of_class(0), None);
        assert_eq!(mode_class_of_name("nope"), None);
        assert!(!is_mode_name("tdm"));
        assert!(!is_mode_name("CLASS10"));
        assert!(!is_mode_name(""));
    }

    #[test]
    fn serves_equal_names_clan_twins_and_bomb_for_elimination() {
        assert!(serves("TDM", "TDM"));
        assert!(serves("tdm", "TDM_CLAN"));
        assert!(serves("TDM_CLAN", "TDM"));
        assert!(serves("BOMB", "BOMB_CLAN"));
        assert!(serves("BOMB", "ELIM"));
        assert!(serves("BOMB", "ELIM_CLAN"));
        assert!(serves("BOMB_CLAN", "ELIM"));
        assert!(!serves("ELIM", "BOMB"));
        assert!(!serves("ELIM_CLAN", "BOMB_CLAN"));
        assert!(!serves("TDM", "FFA"));
        assert!(!serves("", "TDM"));
        assert!(!serves("FLAG", ""));
    }
}
