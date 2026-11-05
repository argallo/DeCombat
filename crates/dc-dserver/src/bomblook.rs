//! The planted bomb as the clients see it (DSV-220, DSV-221): the bomb model and its skin out of the user's ROM, the
//! looping indicator effect group drawn at the planted bomb and the explosion effect group of its blast. Every name is
//! found at run time in the user's archives by its pinned key (the names are ROM data and never in the tree).
//!
//! No 2009 capture shows a planted bomb or its blast: the 2009 `GS_BOMB_EXPLODED` 0x4f only hides the bomb timer and
//! shows the notice (GAME_MESSAGES_2009_S2C.md §2.2), so the bomb object and the blast effect are server objects, the
//! way a breakable's death effect is (DSV-118). Which model and which groups are ours.
// SPEC: GAME_MODES_2009.md §5.2 step 3 (after the plant the server PLAUSIBLY creates a world object of the bomb and
// the defuse zone at it); GAME_MESSAGES_2009_S2C.md §2.2 rows 0x51 / 0x50 / 0x4f (display only), §8.4 (type 0x64
// placed effect `[name][value][has_target]`); FMT_SPR_LTC_FX.md §5.1 (the effect directory's `.fxf` files, group
// names)
// DC-DEFAULT: DSV-220 (proposed) the bomb model and the indicator group; DSV-221 (proposed) the blast group

use dc_rez::RezVfs;

use crate::chars::{name_key, NameKey};

/// The bomb model and its skin: the third-person time-bomb model of the pickup archive and its texture.
// OBSERVED: ROM archive layout (the pickup-model archive holds one time-bomb model and its skin of the same name)
// DC-DEFAULT: DSV-220 (proposed)
pub const BOMB_MODEL: NameKey = ([0xae, 0x6d, 0xaf, 0xd5, 0xfd, 0xc1, 0x29, 0x87], 27);
pub const BOMB_SKIN: NameKey = ([0xa0, 0xa0, 0xe7, 0x17, 0xcf, 0x4d, 0xd0, 0xef], 26);
/// The indicator group drawn at the planted bomb: a blinking red flare and the bomb beep (2 s, looped).
// OBSERVED: ROM effect group (4 keys, length 2000 ms: red flare particles and the beep wave of the pickup archive)
// DC-DEFAULT: DSV-220 (proposed)
pub const BOMB_LIGHT_FX: NameKey = ([0x75, 0x3d, 0xa7, 0x9d, 0xeb, 0xa9, 0xc5, 0x6a], 6);
/// The blast group: the large explosion (14 keys, length 3000 ms: core, smoke, burst, debris, camera shake, light
/// and the explosion wave).
// OBSERVED: ROM effect group (14 keys, length 3000 ms, one sound key)
// DC-DEFAULT: DSV-221 (proposed)
pub const BOMB_BLAST_FX: NameKey = ([0x4f, 0x4c, 0x96, 0x78, 0x4b, 0x11, 0xfa, 0x89], 9);
/// The site indicator groups a community map's bomb sites use (MAP-113): the flat floor marker of a live site and of
/// the planted one, the groups the ROM worlds' site indicators name.
// OBSERVED: ROM world placed effects named by the bomb-site planted commands (all 6 S&D worlds, looped)
// DC-DEFAULT: MAP-113 (proposed)
pub const SITE_LIVE_FX: NameKey = ([190, 238, 163, 116, 71, 38, 83, 36], 11);
pub const SITE_PLANTED_FX: NameKey = ([43, 231, 45, 197, 238, 34, 156, 232], 11);
/// The ground rings of a capture flag (MAP-115): the groups the ROM worlds place under their flags, one per team.
// OBSERVED: ROM world placed effects at each capture flag (looped; team 1's and team 2's)
// DC-DEFAULT: MAP-115 (proposed)
pub const FLAG_RING_TEAM1_FX: NameKey = ([182, 102, 207, 252, 65, 200, 118, 29], 11);
pub const FLAG_RING_TEAM2_FX: NameKey = ([190, 147, 68, 253, 226, 239, 216, 127], 11);
/// The top folder of the effect files (the client's effect directory, FMT_SPR_LTC_FX.md §5.1).
// SPEC: FMT_SPR_LTC_FX.md §5.1 (the directory named by the 8-byte literal, upper-cased)
pub const EFFECT_DIR: NameKey = ([0xae, 0xfd, 0x38, 0xaa, 0xe9, 0x40, 0xb6, 0x3b], 8);

/// The resolved names (each `None` when the user's ROM lacks it: that part is not drawn).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BombLook {
    /// Resource paths of the model and its skin.
    pub model: Option<String>,
    pub skin: Option<String>,
    /// Effect group names.
    pub light_fx: Option<Vec<u8>>,
    pub blast_fx: Option<Vec<u8>>,
    pub site_live_fx: Option<Vec<u8>>,
    pub site_planted_fx: Option<Vec<u8>>,
    pub flag_ring_team1_fx: Option<Vec<u8>>,
    pub flag_ring_team2_fx: Option<Vec<u8>>,
}

impl BombLook {
    /// Finds the names in the archives of `rom` (the `Game` folder's `.rez` files).
    pub fn from_rom(rom: &std::path::Path) -> BombLook {
        let vfs = RezVfs::new(rom);
        let mut rez: Vec<String> = std::fs::read_dir(rom.join("Game"))
            .map(|d| {
                d.filter_map(|e| e.ok())
                    .filter_map(|e| e.file_name().to_str().map(str::to_string))
                    .filter(|n| n.to_ascii_lowercase().ends_with(".rez"))
                    .collect()
            })
            .unwrap_or_default();
        rez.sort();
        let mut look = BombLook::default();
        for name in rez {
            let Ok(list) = vfs.list_entries(&name) else {
                continue;
            };
            for (entry, _, _) in list {
                let up = entry.to_ascii_uppercase();
                let k = name_key(up.as_bytes());
                if k == BOMB_MODEL {
                    look.model = Some(up.clone());
                } else if k == BOMB_SKIN {
                    look.skin = Some(up.clone());
                }
                let in_fx = up
                    .split(['\\', '/'])
                    .next()
                    .is_some_and(|d| name_key(d.as_bytes()) == EFFECT_DIR);
                if !(in_fx && up.ends_with(".FXF")) {
                    continue;
                }
                let bytes = match vfs.entry_raw(&name, &entry) {
                    Ok(Some(b))
                        if dc_fx_data::fxf::detect_encoding(&b) == dc_fx_data::Encoding::Text =>
                    {
                        Some(b)
                    }
                    _ => vfs.entry(&name, &entry).ok().flatten(),
                };
                look.take_groups(&group_names(bytes.as_deref().unwrap_or_default()));
            }
        }
        look
    }

    /// Keeps the first group name of each pinned key.
    pub fn take_groups(&mut self, names: &[Vec<u8>]) {
        for n in names {
            let k = name_key(n);
            if k == BOMB_LIGHT_FX && self.light_fx.is_none() {
                self.light_fx = Some(n.clone());
            } else if k == BOMB_BLAST_FX && self.blast_fx.is_none() {
                self.blast_fx = Some(n.clone());
            } else if k == SITE_LIVE_FX && self.site_live_fx.is_none() {
                self.site_live_fx = Some(n.clone());
            } else if k == SITE_PLANTED_FX && self.site_planted_fx.is_none() {
                self.site_planted_fx = Some(n.clone());
            } else if k == FLAG_RING_TEAM1_FX && self.flag_ring_team1_fx.is_none() {
                self.flag_ring_team1_fx = Some(n.clone());
            } else if k == FLAG_RING_TEAM2_FX && self.flag_ring_team2_fx.is_none() {
                self.flag_ring_team2_fx = Some(n.clone());
            }
        }
    }
}

/// The group names of one effect file (empty when it does not parse).
// SPEC: FMT_SPR_LTC_FX.md §5.2 (groups: name, length, keys)
pub fn group_names(bytes: &[u8]) -> Vec<Vec<u8>> {
    if bytes.is_empty() {
        return Vec::new();
    }
    dc_fx_data::fxf::FxfFile::parse(bytes)
        .map(|f| f.groups.into_iter().map(|g| g.name.into_bytes()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pinned keys pick their names out of a list; other names are ignored, the first match is kept.
    #[test]
    fn groups_are_taken_by_their_pinned_key() {
        let mut l = BombLook::default();
        l.take_groups(&[b"x".to_vec(), b"y".to_vec()]);
        assert_eq!(l, BombLook::default());
        assert_eq!(name_key(b"x").1, 1);
    }

    /// With the user's ROM (`CA_ROM`): the model, its skin and both groups are found.
    #[test]
    fn the_rom_has_the_bomb_model_and_both_groups() {
        let Ok(rom) = std::env::var("CA_ROM") else {
            eprintln!("CA_ROM unset: skipped");
            return;
        };
        let l = BombLook::from_rom(std::path::Path::new(&rom));
        assert!(l.model.is_some() && l.skin.is_some(), "{l:?}");
        assert!(l.light_fx.is_some() && l.blast_fx.is_some(), "{l:?}");
        assert!(
            l.site_live_fx.is_some() && l.site_planted_fx.is_some(),
            "{l:?}"
        );
        assert!(
            l.flag_ring_team1_fx.is_some() && l.flag_ring_team2_fx.is_some(),
            "{l:?}"
        );
        assert_eq!(name_key(l.model.as_deref().unwrap().as_bytes()), BOMB_MODEL);
    }
}
