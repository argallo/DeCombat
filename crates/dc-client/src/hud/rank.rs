//! The rank icon beside the name over a player's head: the 2009 world mark puts the small rank icon of the player's
//! rank (0x6b field 17b) left of the centred caption, its "_O" form while the player lives and its "_G" form while
//! the mark's dead flag is set. With DeCombat's rank 0 for everyone this is the trainee icon the arena frames show
//! over mates and enemies alike.
// EVIDENCE: cshell+0x8a760 (mark entry with team, rank, name; DISASM cshell+0x2ce8e..0x2ce9b: the 0x6b handler passes
// field 17b), cshell+0x88a40 / 0x88bf0 (icon textures: the rank texture with its extension cut at the last '.', plus
// the two forms; none for team 0 / 0xff; size 29 x 29), cshell+0x2683f0 / 0x267dc0 (rank -> the small-icon list by
// the rank-mark number, a rank past the table -> 0), cshell+0x268810 (both lists from the level table), DISASM
// cshell+0x8bb94..0x8bd03 (the O icon while the dead flag is clear, else the G icon; x = point x - icon width -
// caption width / 2, y = point y - icon height / 2 - 3), cshell+0x8a420 (drawn before the caption)

use dc_ui_data::names::{read_pinned, Pin, CSHELL_REL_PATH};
use std::path::Path;

/// Width and height of the icon item (virtual pixels).
// EVIDENCE: cshell+0x88a40 (`FUN_37443860(tex, 0x1d, 0x1d)`)
pub const ICON: i32 = 29;

/// The level table's file in the attribute archive.
// OBSERVED: ROM archive ATTRIBUTES.rez (entry of the level table)
pub const LEVEL_ENTRY: &str = "ATTRIBUTES\\LEVELS.TXT";

/// Pinned literals of the level-table reader and the icon builder.
// EVIDENCE: cshell+0x268810 (section and key literals), cshell+0x88b17 / 0x88bf0 (the two forms)
const PINS: [Pin; 6] = [
    Pin {
        what: "level texture section",
        rva: 0x3f4ba4,
        sha16: "d3b09d9f76b608c8",
        len: 13,
    },
    Pin {
        what: "small icon key",
        rva: 0x3f4b2c,
        sha16: "fb8b9105cea3d946",
        len: 9,
    },
    Pin {
        what: "level section",
        rva: 0x3f4a68,
        sha16: "9782640c0625e02f",
        len: 8,
    },
    Pin {
        what: "rank mark key",
        rva: 0x3f4a30,
        sha16: "26a6998525d5c29e",
        len: 8,
    },
    Pin {
        what: "living icon form",
        rva: 0x3d6564,
        sha16: "307dba27e0985153",
        len: 8,
    },
    Pin {
        what: "dead icon form",
        rva: 0x3d6570,
        sha16: "ed3d6db29e05af33",
        len: 8,
    },
];

/// The rank icons of the ROM.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RankIcons {
    /// The small icon of each list position (numbered keys from 0 up to the first missing one).
    pub small: Vec<String>,
    /// The rank-mark number of each rank (list position of its icon).
    pub rank_mark: Vec<String>,
    /// The two printf forms (`%s` = the icon path without its extension): alive, dead.
    pub forms: [String; 2],
}

impl RankIcons {
    /// Reads the lists from the user's level table with the keys of the user's CShell.dll.
    pub fn from_rom(rom: &Path) -> Option<RankIcons> {
        let pe = dc_rom::PeFile::from_file(&rom.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1]))
            .ok()?;
        let s: Vec<String> = PINS
            .iter()
            .map(|p| read_pinned(&pe, p))
            .collect::<Result<_, _>>()
            .ok()?;
        let vfs = dc_rez::RezVfs::new(rom);
        let doc = dc_bute::ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", LEVEL_ENTRY)?;
        let list = |sec: &str, key: &str| -> Vec<String> {
            let Some(m) = doc.get(sec) else {
                return Vec::new();
            };
            (0..1024)
                .map_while(|i| m.get(&format!("{key}{i}")))
                .map(|v| v.trim().trim_matches('"').trim().to_string())
                .collect()
        };
        Some(RankIcons {
            small: list(&s[0], &s[1]),
            rank_mark: list(&s[2], &s[3]),
            forms: [s[4].clone(), s[5].clone()],
        })
    }

    /// The icon texture of `rank` for the player `name`, alive or dead (`None` when the lists do not name one). A
    /// GM-named player (CLI-1305) takes the last small icon (the GM rank icon) whatever the rank.
    // EVIDENCE: cshell+0x2683f0 (a name starting with L"GM" -> the last entry of the small-icon list, else the entry
    // of the rank's mark number)
    pub fn texture(&self, rank: u32, dead: bool, name: &str) -> Option<String> {
        let path = if crate::menu::gm::gm_named(name) {
            self.small.last()?
        } else {
            let r = if (rank as usize) < self.rank_mark.len() {
                rank as usize
            } else {
                0
            };
            // atoi of the rank-mark value: leading digits, 0 when there are none
            let digits: String = self
                .rank_mark
                .get(r)?
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            let idx: usize = digits.parse().unwrap_or(0);
            self.small.get(idx)?
        };
        let stem = path.rfind('.').map_or(path.as_str(), |i| &path[..i]);
        Some(self.forms[dead as usize].replacen("%s", stem, 1))
    }
}

/// The icon's place for a caption centred on `at` that is `caption_w` wide (virtual pixels).
pub fn icon_place(at: [f32; 2], caption_w: f32) -> [f32; 2] {
    [
        at[0] - ICON as f32 - caption_w / 2.0,
        at[1] - (ICON / 2) as f32 - 3.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_maps_through_the_mark_number_and_cuts_the_extension() {
        let r = RankIcons {
            small: vec![
                "a\\r_0.dtx".into(),
                "a\\r_1.dtx".into(),
                "a\\r_2.x.dtx".into(),
            ],
            rank_mark: vec!["0".into(), "2".into()],
            forms: ["%s_O.dtx".into(), "%s_G.dtx".into()],
        };
        assert_eq!(r.texture(0, false, "P").as_deref(), Some("a\\r_0_O.dtx"));
        assert_eq!(r.texture(1, true, "P").as_deref(), Some("a\\r_2.x_G.dtx"));
        assert_eq!(
            r.texture(9, false, "P").as_deref(),
            Some("a\\r_0_O.dtx"),
            "past the table: rank 0"
        );
        // CLI-1305: a GM-named player's mark is the last small icon in either form
        assert_eq!(r.texture(0, false, "GMx").as_deref(), Some("a\\r_2.x_O.dtx"));
        assert_eq!(r.texture(1, true, "GMx").as_deref(), Some("a\\r_2.x_G.dtx"));
        assert_eq!(icon_place([100.0, 50.0], 40.0), [51.0, 33.0]);
    }
}
