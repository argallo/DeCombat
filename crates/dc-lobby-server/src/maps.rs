//! The lobby's map table: the ROM's `Map<n>` rows followed by the community maps of the loaded mod set
//! (MAPKIT_PLAN.md 6.4, D10, D11). `RomData` stays frozen in its `Arc`; the community rows live on `Core`
//! (`community_maps`) and [`MapTable`] is the view every map query goes through, branching on the index.
//!
//! The mods thread turns `ModHost::map_records()` into [`CommunityMap`] plain data ([`community_maps`]) before
//! it reports ready, so the lobby never touches the Lua state for a map.
// DC-DEFAULT: MAP-35 (table), MAP-36 (launch), MAP-37 (room line, goal and radar), MAP-39 (records as plain data),
// MAP-81 (a featured entry's records and their JSON); see DC_DEFAULTS.md

use crate::rom::RomData;
use dc_bute::{eq_fold, RoundTimePolicy};
use dc_mod_lua::{MapRecord, ModHost, RadarInfo};
use std::path::{Path, PathBuf};

/// The side of the radar picture `dc-mapkit export` writes (`assets/pictures/<key>_radar.dtx`, MAP-52): the two
/// sizes of the room line's `radar=` field, which the client divides its scale by.
pub const RADAR_PICTURE_SIZE: u32 = 256;

/// Half the side of the radar window a community map shows around the player, in picture units (a ROM row
/// carries its own value in its `.gms`; a record has none, so every community map gets this one: a quarter of the
/// picture each way).
// DC-DEFAULT: MAP-37 (proposed) the community radar window
pub const RADAR_WINDOW_HALF: f32 = 0.25;

/// The room line's `radar=` field for a record's radar: `picture|scale|origin|rotation|half|width|height`, the
/// form dc_bute writes for a ROM map's `.gms` and dc-dserver's `apply_room_intro` (game/arrival.rs) reads into
/// the 0x45 intro block. The client maps a world point `(x, z)` to picture uv
/// `(x * scale_x / width + origin_x, origin_z - z * scale_z / height)` (dc-client `hud::radar::RadarMap`), so the
/// record's world rectangle `min..max` (the picture's left-bottom and right-top, MAP-52) becomes
/// `scale_x = width / (max_x - min_x)`, `origin_x = -min_x / (max_x - min_x)`, `scale_z = height / (max_z - min_z)`,
/// `origin_z = max_z / (max_z - min_z)`; the y components are 0. `None` for an empty picture name or a rectangle
/// without area (the room line then keeps the default ROM map's radar, as before).
// DC-DEFAULT: MAP-37 (proposed) the record's radar reaches the match
pub fn radar_field(r: &RadarInfo) -> Option<String> {
    let picture = r.picture.trim();
    let w = r.max[0] - r.min[0];
    let h = r.max[1] - r.min[1];
    if picture.is_empty() || !(w > 0.0 && h > 0.0) || !w.is_finite() || !h.is_finite() {
        return None;
    }
    let size = RADAR_PICTURE_SIZE as f64;
    let scale = [size / w, 0.0, size / h];
    let origin = [-r.min[0] / w, 0.0, r.max[1] / h];
    let vec = |v: [f64; 3]| {
        v.iter()
            .map(|x| float_text(*x))
            .collect::<Vec<_>>()
            .join(",")
    };
    Some(format!(
        "radar={}|{}|{}|{}|{}|{}|{}",
        picture.replace(' ', "_"),
        vec(scale),
        vec(origin),
        float_text(0.0),
        float_text(RADAR_WINDOW_HALF as f64),
        RADAR_PICTURE_SIZE,
        RADAR_PICTURE_SIZE
    ))
}

/// A real with a decimal point (`1.0`, not `1`), the spelling dc_bute's room line uses.
fn float_text(v: f64) -> String {
    let s = format!("{v}");
    if s.contains(['.', 'e', 'E']) {
        s
    } else {
        format!("{s}.0")
    }
}

/// `line` without its `key=...` token (the room line's tokens are whitespace-separated `key=value` pairs).
fn without_token(line: &str, key: &str) -> String {
    let prefix = format!("{key}=");
    line.split_whitespace()
        .filter(|t| !t.starts_with(&prefix))
        .collect::<Vec<_>>()
        .join(" ")
}

/// One community map as the lobby offers it: the record and the absolute path of its world on this machine.
#[derive(Clone, Debug, PartialEq)]
pub struct CommunityMap {
    pub record: MapRecord,
    /// The world file inside the owning mod's unpacked folder (`<data dir>/mod_cache/<sha>/assets/...dat` or the
    /// mod folder itself), absolute: the game server runs in its own working directory.
    pub world_path: PathBuf,
}

impl CommunityMap {
    /// The map's title picture as the room line names it: by the export's convention
    /// `assets/pictures/<world stem>_title.dtx` of the world's pack, when that file is there (MAP-102).
    pub fn title_picture(&self) -> Option<String> {
        let stem = self.world_path.file_stem()?.to_str()?;
        let file = format!("{stem}_title.dtx");
        let maps = self.world_path.parent()?;
        maps.parent()?
            .join("pictures")
            .join(&file)
            .is_file()
            .then_some(())?;
        let world = self.record.world.trim();
        let (pack, _) = world.rsplit_once("/assets/")?;
        Some(format!("{pack}/assets/pictures/{file}").replace(' ', "_"))
    }

    /// The map key (`record.id`).
    pub fn key(&self) -> &str {
        &self.record.id
    }
}

/// The community maps of a loaded host, in `ModHost::map_records` order, each checked against its owning mod:
/// the mod must be shared with clients (`requires_client`: the client must download the world, MAP-5) and the
/// world file must exist under the mod's folder. Runs on the mods thread; the result is plain data.
/// `Err` names the first faulty record (the caller treats it like a malformed declared set).
// DC-DEFAULT: MAP-39 (proposed)
pub fn community_maps(host: &ModHost) -> Result<Vec<CommunityMap>, String> {
    community_maps_each(host)?.into_iter().collect()
}

/// [`community_maps`] record by record: one `Err` per record that cannot be served, the rest as maps, in
/// `ModHost::map_records` order. The outer `Err` is a record the host cannot turn into plain data at all. A
/// featured entry's curated set leaves the faulty records out with a log line and keeps the rest (MAP-81), where
/// a declared set stops the lobby at the first one (MAP-39).
// DC-DEFAULT: MAP-81 (proposed)
pub fn community_maps_each(host: &ModHost) -> Result<Vec<Result<CommunityMap, String>>, String> {
    let records = host.map_records()?;
    if records.is_empty() {
        return Ok(Vec::new());
    }
    let loaded = host.loaded_mods();
    Ok(records
        .into_iter()
        .map(|record| check_record(record, &loaded))
        .collect())
}

/// One record against the loaded mods: the owning mod is loaded, shared, from a folder, and the world file is there.
fn check_record(
    record: MapRecord,
    loaded: &[(dc_mod_lua::ModManifest, Option<PathBuf>)],
) -> Result<CommunityMap, String> {
    let Some((manifest, root)) = loaded.iter().find(|(m, _)| m.id == record.mod_id) else {
        return Err(format!(
            "map \"{}\" is registered by mod '{}', which is not loaded",
            record.id, record.mod_id
        ));
    };
    if !manifest.requires_client {
        return Err(format!(
            "map \"{}\" is registered by mod '{}', which clients do not receive (requires_client is \
             false): a map mod must be shared (MAP-5)",
            record.id, record.mod_id
        ));
    }
    let Some(root) = root else {
        return Err(format!(
            "map \"{}\" of mod '{}': the mod was not loaded from a folder, so its world has no path",
            record.id, record.mod_id
        ));
    };
    let Some(rel) = record.world_rel_path() else {
        return Err(format!(
            "map \"{}\" of mod '{}': world {:?} is not the mod's own asset",
            record.id, record.mod_id, record.world
        ));
    };
    let world_path = absolute(&root.join(rel));
    if !world_path.is_file() {
        return Err(format!(
            "map \"{}\" of mod '{}': world file {} is missing",
            record.id,
            record.mod_id,
            world_path.display()
        ));
    }
    Ok(CommunityMap { record, world_path })
}

/// A map record as the JSON object of a featured entry's set document (`/entry_mods/<entry>.json` `maps`,
/// MAP-81): every field of [`MapRecord`] under its own name in the shape serde's derive would give the struct,
/// so the client reads it back into the same type: strings as strings, `None` as `null`, `modes` and
/// `patched_by` as string arrays, `goals` as `[[mode, goal], ...]`, `radar` as `{picture, min: [x, z],
/// max: [x, z]}`, `players` as `{min, max}`.
// DC-DEFAULT: MAP-81 (proposed)
pub fn map_record_json(r: &MapRecord) -> dc_mod_lua::JsonValue {
    use dc_mod_lua::JsonValue as J;
    let s = |t: &str| J::String(t.to_string());
    let opt = |t: &Option<String>| t.as_deref().map(s).unwrap_or(J::Null);
    let strings = |v: &[String]| J::Array(v.iter().map(|t| s(t)).collect());
    let mut o = std::collections::BTreeMap::new();
    o.insert("mod_id".to_string(), s(&r.mod_id));
    o.insert("patched_by".to_string(), strings(&r.patched_by));
    o.insert("id".to_string(), s(&r.id));
    o.insert("name".to_string(), s(&r.name));
    o.insert("description".to_string(), opt(&r.description));
    o.insert("world".to_string(), s(&r.world));
    o.insert("modes".to_string(), strings(&r.modes));
    o.insert(
        "goals".to_string(),
        J::Array(
            r.goals
                .iter()
                .map(|(m, g)| J::Array(vec![s(m), J::Number(f64::from(*g))]))
                .collect(),
        ),
    );
    o.insert(
        "round_seconds".to_string(),
        r.round_seconds.map_or(J::Null, |n| J::Number(f64::from(n))),
    );
    o.insert("picture".to_string(), opt(&r.picture));
    o.insert("loading".to_string(), opt(&r.loading));
    o.insert(
        "radar".to_string(),
        r.radar.as_ref().map_or(J::Null, |rd| {
            let pair = |p: [f64; 2]| J::Array(vec![J::Number(p[0]), J::Number(p[1])]);
            let mut m = std::collections::BTreeMap::new();
            m.insert("picture".to_string(), s(&rd.picture));
            m.insert("min".to_string(), pair(rd.min));
            m.insert("max".to_string(), pair(rd.max));
            J::Object(m)
        }),
    );
    o.insert(
        "players".to_string(),
        r.players.map_or(J::Null, |p| {
            let mut m = std::collections::BTreeMap::new();
            m.insert("min".to_string(), J::Number(f64::from(p.min)));
            m.insert("max".to_string(), J::Number(f64::from(p.max)));
            J::Object(m)
        }),
    );
    J::Object(o)
}

/// An absolute form of `p` (the lobby's `--db` folder may be relative; the game server runs elsewhere). Not
/// `canonicalize`: on Windows that yields a `\\?\` path the game server's own path handling need not accept.
fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The map table: ROM rows first, community maps after them.
pub struct MapTable<'a> {
    pub rom: &'a RomData,
    pub community: &'a [CommunityMap],
}

impl<'a> MapTable<'a> {
    /// The index of the first community map: after the ROM's `Map<n>` rows. Without a ROM the 0x311 table is
    /// `dc_bute`'s synthetic fallback (indices 0..12 with every bit), so the community rows start after its
    /// highest index too, never on top of it.
    // DC-DEFAULT: MAP-35 (proposed)
    pub fn first_community_index(&self) -> usize {
        let fallback = self
            .rom
            .map_mode_masks()
            .iter()
            .map(|&(m, _)| m + 1)
            .max()
            .unwrap_or(0);
        self.rom.map_count().max(fallback)
    }

    /// The community map at `index`, when `index` is one of theirs.
    pub fn community_of(&self, index: u32) -> Option<&'a CommunityMap> {
        let first = self.first_community_index();
        (index as usize)
            .checked_sub(first)
            .and_then(|i| self.community.get(i))
    }

    /// True for an index of a community map.
    pub fn is_community(&self, index: u32) -> bool {
        self.community_of(index).is_some()
    }

    /// `(index, key)` of every community map, in table order.
    pub fn community_indices(&self) -> Vec<(u32, &'a str)> {
        let first = self.first_community_index();
        self.community
            .iter()
            .enumerate()
            .map(|(i, m)| ((first + i) as u32, m.key()))
            .collect()
    }

    /// The number of map indices a room may choose from: the ROM rows plus the community maps (0 without a ROM
    /// and without community maps, which lets any index through, as today).
    pub fn map_count(&self) -> usize {
        if self.community.is_empty() {
            self.rom.map_count()
        } else {
            self.first_community_index() + self.community.len()
        }
    }

    /// The mode class (`GameType`) of each plain mode row, by row index (the 0x311 bit positions). A row whose
    /// `EnablePlay` is not `TRUE` has no class here: the ROM rows carry no bit for it either
    /// (`dc_bute::GameRoomData::map_mode_masks`), so a community map is never offered in a mode the ROM switched
    /// off (GMD-14).
    // DC-DEFAULT: MAP-35 (proposed) EnablePlay gates the community bits like the ROM's
    fn row_classes(&self) -> Vec<Option<u32>> {
        self.rom
            .game_room
            .modes(false)
            .iter()
            .take(32)
            .map(|row| {
                let enabled = row
                    .get("EnablePlay")
                    .is_none_or(|v| eq_fold(v.trim(), "TRUE"));
                if !enabled {
                    return None;
                }
                row.get("GameType")
                    .and_then(|v| v.trim().parse::<u32>().ok())
            })
            .collect()
    }

    /// The 0x311 mode bits of a community map: every plain mode row whose class is one of the record's modes
    /// (`dc_config::modes::mode_class_of_name`). The playable-mode policy is applied where the ROM bits get it
    /// (`lobby::filter_masks`). Without mode rows (no ROM) a community map has no bits.
    // DC-DEFAULT: MAP-35 (proposed)
    pub fn mode_bits(&self, map: &CommunityMap) -> u32 {
        let classes: Vec<u32> = map
            .record
            .modes
            .iter()
            .filter_map(|n| dc_config::modes::mode_class_of_name(n))
            .collect();
        let mut bits = 0u32;
        for (i, c) in self.row_classes().into_iter().enumerate() {
            if c.is_some_and(|c| classes.contains(&c)) {
                bits |= 1 << i;
            }
        }
        bits
    }

    /// `(map index, mode bit mask)` pairs for 0x311: the ROM's, then one per community map that has any bit.
    pub fn map_mode_masks(&self) -> Vec<(usize, u32)> {
        let mut masks = self.rom.map_mode_masks();
        let first = self.first_community_index();
        for (i, m) in self.community.iter().enumerate() {
            let bits = self.mode_bits(m);
            if bits != 0 {
                masks.push((first + i, bits));
            }
        }
        masks
    }

    /// True when map `map` plays mode row `mode` (the 0x311 bit).
    pub fn map_plays_mode(&self, map: u32, mode: u32) -> bool {
        match self.community_of(map) {
            Some(m) => mode < 32 && self.mode_bits(m) & (1u32 << mode) != 0,
            None => self.rom.map_plays_mode(map, mode),
        }
    }

    /// Default map and goal of a mode: the ROM's (a community map is never a mode's default map). A row's
    /// `DefaultMapID` at or past the first community index (a `Map<n>` gap in the table: the ROM rows stop at
    /// the first missing section, the number does not) would alias a community map, so it is clamped to the LG-2
    /// fallback (the first ROM map carrying the mode's bit), else 0.
    // DC-DEFAULT: MAP-35 (proposed) a default map number never lands on a community index
    pub fn mode_defaults(&self, mode: u32) -> (u32, u32) {
        let (map, goal) = self.rom.mode_defaults(mode);
        if (map as usize) < self.first_community_index() {
            return (map, goal);
        }
        let first = self
            .rom
            .map_mode_masks()
            .into_iter()
            .find(|(_, mask)| mode < 32 && mask & (1u32 << mode) != 0)
            .map(|(m, _)| m as u32)
            .unwrap_or(0);
        (first, goal)
    }

    /// The DeCombat mode names of mode row `mode` (its class through `mode_names_of_class`).
    fn mode_names_of_row(&self, mode: u32) -> &'static [&'static str] {
        self.row_classes()
            .get(mode as usize)
            .copied()
            .flatten()
            .map(dc_config::modes::mode_names_of_class)
            .unwrap_or(&[])
    }

    /// The goal a community map's record sets for mode row `mode`, when it sets one (`goals = { TDM = 80 }`).
    /// `None` for a ROM map or a record without a goal for that mode.
    // DC-DEFAULT: MAP-37 (proposed)
    pub fn record_goal(&self, map: u32, mode: u32) -> Option<u32> {
        let m = self.community_of(map)?;
        self.mode_names_of_row(mode)
            .iter()
            .find_map(|n| m.record.goal_of(n))
    }

    /// `[MapExp]` / `[MapMoney]` factors: the ROM's for its maps, 1.0 for a community map.
    // DC-DEFAULT: MAP-36 (proposed)
    pub fn map_factors(&self, map: u32, mode: u32) -> (f64, f64) {
        if self.is_community(map) {
            (1.0, 1.0)
        } else {
            self.rom.map_factors(map, mode)
        }
    }

    /// World of a room: for a community map the ABSOLUTE path of its world file (`--dserver-world` does not
    /// apply: the room is about that world); else the ROM's (`forced` wins there).
    // DC-DEFAULT: MAP-36 (proposed)
    pub fn world_for(&self, map: u32, mode: u32, forced: &str) -> String {
        match self.community_of(map) {
            Some(m) => m.world_path.to_string_lossy().into_owned(),
            None => self.rom.world_for(map, mode, forced),
        }
    }

    /// The `room key=...` line. A community map takes the mode row's key / type and the points of the mode's
    /// default ROM map (it has no GMS row of its own), `time=` from the record's `round_seconds` when set, else
    /// today's fallbacks; the goal is the room's (the record's goal became the room's goal when the map was
    /// picked, `record_goal`); `radar=` is the record's radar ([`radar_field`]) when it has one, in place of the
    /// default ROM map's.
    // DC-DEFAULT: MAP-37 (proposed)
    #[allow(clippy::too_many_arguments)]
    pub fn room_line(
        &self,
        map: u32,
        mode: u32,
        goal: u32,
        ff: u32,
        arms: u32,
        round_clock: &RoundTimePolicy,
    ) -> String {
        match self.community_of(map) {
            Some(m) => {
                let (gms_map, _) = self.mode_defaults(mode);
                let time = m.record.round_seconds.map(|s| s as i32);
                let line = self
                    .rom
                    .room_line(gms_map, mode, goal, ff, arms, round_clock, time);
                // DC-DEFAULT: MAP-102 the default ROM map lends its rules, not its name: its title and its large
                // map stay out, and the map's own title picture takes the banner when the pack has one
                let line = without_token(&without_token(&line, "obj"), "large");
                let line = match m.title_picture() {
                    Some(title) => format!("{line} obj={title}"),
                    None => line,
                };
                match m.record.radar.as_ref().and_then(radar_field) {
                    Some(radar) => format!("{} {radar}", without_token(&line, "radar")),
                    None => line,
                }
            }
            None => self
                .rom
                .room_line(map, mode, goal, ff, arms, round_clock, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_bute::{ButeDoc, GameRoomData};

    fn record(id: &str, modes: &[&str], goals: &[(&str, u32)], secs: Option<u32>) -> CommunityMap {
        CommunityMap {
            record: MapRecord {
                mod_id: "m".into(),
                patched_by: Vec::new(),
                id: id.into(),
                name: id.into(),
                description: None,
                world: format!("@mod:m/assets/maps/{id}.dat"),
                modes: modes.iter().map(|s| s.to_string()).collect(),
                goals: goals.iter().map(|(m, g)| (m.to_string(), *g)).collect(),
                round_seconds: secs,
                picture: None,
                loading: None,
                radar: None,
                players: None,
            },
            world_path: PathBuf::from("/abs/x.dat"),
        }
    }

    // synthetic rows: one ROM map, three plain mode rows of classes 1 (TDM), 2 (BOMB) and 6 (FFA)
    const TABLE: &str = r"[Map0]
Enable=TRUE
GMS_AA=a
[Mode0]
Key=AA
GameType=1
DefaultGoal=100
[Mode1]
Key=BB
GameType=2
DefaultGoal=5
[Mode2]
Key=CC
GameType=6
";

    fn rom() -> RomData {
        let mut r = RomData::empty();
        r.game_room = GameRoomData::from_doc(ButeDoc::parse(TABLE));
        r
    }

    #[test]
    fn community_maps_follow_the_rom_rows_with_the_bits_of_their_modes() {
        let rom = rom();
        let maps = vec![
            record("valley", &["TDM", "BOMB"], &[("TDM", 80)], Some(300)),
            record("ruins", &["FFA"], &[], None),
            record("nowhere", &["BOUNTY"], &[], None),
        ];
        let t = MapTable {
            rom: &rom,
            community: &maps,
        };
        assert_eq!(t.first_community_index(), 1);
        assert_eq!(t.map_count(), 4);
        assert_eq!(t.map_mode_masks(), vec![(0, 0b001), (1, 0b011), (2, 0b100)]);
        assert!(t.map_plays_mode(1, 0) && t.map_plays_mode(1, 1) && !t.map_plays_mode(1, 2));
        assert!(t.map_plays_mode(2, 2) && !t.map_plays_mode(3, 0));
        assert_eq!(
            t.community_indices(),
            vec![(1, "valley"), (2, "ruins"), (3, "nowhere")]
        );
        assert_eq!(t.record_goal(1, 0), Some(80));
        assert_eq!(t.record_goal(1, 1), None);
        assert_eq!(t.record_goal(0, 0), None);
        assert_eq!(
            t.mode_defaults(1),
            (0, 5),
            "a community map is never the default"
        );
        assert_eq!(t.map_factors(1, 0), (1.0, 1.0));
        assert_eq!(t.world_for(1, 0, "forced"), "/abs/x.dat");
        assert_eq!(t.world_for(0, 0, "forced"), "forced");
        let line = t.room_line(1, 1, 7, 1, 0, &RoundTimePolicy::default());
        assert!(
            line.contains("time=300") && line.contains("goal=7"),
            "{line}"
        );
        let line = t.room_line(2, 2, 50, 0, 0, &RoundTimePolicy::default());
        assert!(!line.contains("time="), "no VFS, no record time: {line}");
    }

    #[test]
    fn without_a_rom_the_community_rows_start_after_the_synthetic_fallback() {
        let rom = RomData::empty();
        let maps = vec![record("valley", &["TDM"], &[], None)];
        let t = MapTable {
            rom: &rom,
            community: &maps,
        };
        assert_eq!(rom.map_count(), 0);
        assert_eq!(t.first_community_index(), 12);
        assert_eq!(t.map_count(), 13);
        assert!(t.is_community(12) && !t.is_community(11));
        // no mode rows: no bits, so the map is not in 0x311 but a room may still choose it
        assert_eq!(t.map_mode_masks().len(), 12);
        let empty = MapTable {
            rom: &rom,
            community: &[],
        };
        assert_eq!(empty.map_count(), 0, "today's no-ROM behaviour");
    }

    /// C10 / MAP-35: a mode row the ROM switched off (`EnablePlay=FALSE`) gives a community map no bit, as it
    /// gives no ROM map one; `TRUE` is read ignoring case, and a row without the key counts as enabled.
    #[test]
    fn a_mode_row_the_rom_switched_off_gives_no_community_bit() {
        const TABLE: &str = r"[Map0]
Enable=TRUE
GMS_AA=a
GMS_BB=b
[Mode0]
Key=AA
GameType=1
EnablePlay=true
[Mode1]
Key=BB
GameType=2
EnablePlay=FALSE
[Mode2]
Key=CC
GameType=6
";
        let mut rom = RomData::empty();
        rom.game_room = GameRoomData::from_doc(ButeDoc::parse(TABLE));
        let maps = vec![record(
            "valley",
            &["TDM", "BOMB", "FFA"],
            &[("BOMB", 3)],
            None,
        )];
        let t = MapTable {
            rom: &rom,
            community: &maps,
        };
        assert_eq!(
            rom.map_mode_masks(),
            vec![(0, 0b001)],
            "the ROM row has no BB bit"
        );
        assert_eq!(
            t.mode_bits(&maps[0]),
            0b101,
            "no BB bit for the community map either"
        );
        assert!(t.map_plays_mode(1, 0) && !t.map_plays_mode(1, 1) && t.map_plays_mode(1, 2));
        assert_eq!(
            t.record_goal(1, 1),
            None,
            "a switched-off row names no mode, so the record's goal for it is never applied"
        );
    }

    /// C11 / MAP-35: with a gap in the `Map<n>` sections the ROM rows stop at the gap while a mode row's
    /// `DefaultMapID` past it would alias a community index; the table clamps it to the LG-2 fallback (the first
    /// ROM map with the mode's bit), and the community map's room line takes that map's GMS too.
    #[test]
    fn a_default_map_past_a_map_row_gap_never_lands_on_a_community_index() {
        const TABLE: &str = r"[Map0]
Enable=TRUE
GMS_BB=b
[Map1]
Enable=TRUE
GMS_AA=a
[Map3]
Enable=TRUE
GMS_AA=a3
[Mode0]
Key=AA
GameType=1
DefaultMapID=3
DefaultGoal=100
[Mode1]
Key=BB
GameType=2
DefaultMapID=7
DefaultGoal=5
";
        let mut rom = RomData::empty();
        rom.game_room = GameRoomData::from_doc(ButeDoc::parse(TABLE));
        let maps = vec![
            record("valley", &["TDM"], &[], None),
            record("ruins", &["TDM"], &[], None),
        ];
        let t = MapTable {
            rom: &rom,
            community: &maps,
        };
        assert_eq!(rom.map_count(), 2, "the rows stop at the gap");
        assert_eq!(t.first_community_index(), 2);
        assert_eq!(rom.mode_defaults(0), (3, 100), "the ROM's raw number");
        assert_eq!(
            t.mode_defaults(0),
            (1, 100),
            "clamped to the first ROM map with the mode's bit"
        );
        assert_eq!(t.mode_defaults(1), (0, 5));
        assert!(t.is_community(2) && t.is_community(3));
        let line = t.room_line(3, 0, 50, 0, 0, &RoundTimePolicy::default());
        assert!(line.starts_with("room "), "{line}");
    }

    /// MAP-102: a community room takes no name from the default ROM map: `obj` and `large` stay out, and `obj` is the
    /// pack's title picture when the file is there.
    #[test]
    fn a_community_room_shows_its_own_title_not_a_rom_maps() {
        let dir = std::env::temp_dir().join(format!("dc_map_title_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets/maps")).unwrap();
        std::fs::create_dir_all(dir.join("assets/pictures")).unwrap();
        let mut m = record("yard", &["TDM"], &[], None);
        m.record.world = "@mod:pack/assets/maps/yard.dat".into();
        m.world_path = dir.join("assets/maps/yard.dat");
        assert_eq!(m.title_picture(), None, "no file, no title");
        std::fs::write(dir.join("assets/pictures/yard_title.dtx"), b"x").unwrap();
        assert_eq!(
            m.title_picture().as_deref(),
            Some("@mod:pack/assets/pictures/yard_title.dtx")
        );
        let line = "room key=TDM obj=rom_name.dtx cmt=mode.dtx large=a|b|1|2 trig=x";
        let kept = without_token(&without_token(line, "obj"), "large");
        assert_eq!(
            kept, "room key=TDM cmt=mode.dtx trig=x",
            "the mode's text stays"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// C16 / MAP-37: the record's radar (picture and world rectangle) becomes the room line's `radar=` field in
    /// the form the game server's `apply_room_intro` reads (picture | scale | origin | rotation | half | w | h), so
    /// the match's 0x45 intro carries the exported picture and mapping; a record without a radar leaves the line
    /// alone, and an existing ROM `radar=` token is replaced, not doubled.
    #[test]
    fn the_records_radar_rides_the_room_line_in_the_game_servers_form() {
        let rom = rom();
        let mut with = record("valley", &["TDM"], &[], Some(120));
        with.record.radar = Some(RadarInfo {
            picture: "@mod:m/assets/pictures/valley_radar.dtx".into(),
            min: [-100.0, -50.0],
            max: [300.0, 150.0],
        });
        let maps = vec![with, record("ruins", &["TDM"], &[], None)];
        let t = MapTable {
            rom: &rom,
            community: &maps,
        };
        let line = t.room_line(1, 0, 80, 0, 0, &RoundTimePolicy::default());
        let expect = "radar=@mod:m/assets/pictures/valley_radar.dtx|0.64,0.0,1.28|0.25,0.0,0.75|0.0|0.25|256|256";
        assert_eq!(line.matches("radar=").count(), 1, "{line}");
        assert!(line.contains(expect) && line.contains("time=120"), "{line}");
        // the game server's room-line reader takes the token and splits it as apply_room_intro does
        let dc_dserver::pipe::Control::Room(settings) = dc_dserver::pipe::parse_control(&line)
        else {
            panic!("not a room line: {line}");
        };
        let radar = settings.get("radar").expect("radar key");
        let fields: Vec<&str> = radar.split('|').collect();
        assert_eq!(fields.len(), 7, "{radar}");
        let vec3 = |s: &str| -> [f32; 3] {
            let v: Vec<f32> = s.split(',').map(|x| x.parse().unwrap()).collect();
            [v[0], v[1], v[2]]
        };
        assert_eq!(fields[0], "@mod:m/assets/pictures/valley_radar.dtx");
        let (scale, origin) = (vec3(fields[1]), vec3(fields[2]));
        assert_eq!((scale, origin), ([0.64, 0.0, 1.28], [0.25, 0.0, 0.75]));
        assert_eq!(fields[3].parse::<f32>().unwrap(), 0.0);
        assert_eq!(fields[4].parse::<f32>().unwrap(), RADAR_WINDOW_HALF);
        assert_eq!((fields[5], fields[6]), ("256", "256"));
        // the client's mapping (hud::radar::RadarMap::uv) puts the rectangle's corners on the picture's corners:
        // the left-bottom corner (min) at uv (0, 1), the right-top corner (max) at uv (1, 0)
        let w = RADAR_PICTURE_SIZE as f32;
        let uv = |x: f32, z: f32| [x * scale[0] / w + origin[0], origin[2] - z * scale[2] / w];
        let near =
            |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5;
        assert!(
            near(uv(-100.0, -50.0), [0.0, 1.0]),
            "{:?}",
            uv(-100.0, -50.0)
        );
        assert!(near(uv(300.0, 150.0), [1.0, 0.0]), "{:?}", uv(300.0, 150.0));
        // no radar: no key
        let line = t.room_line(2, 0, 80, 0, 0, &RoundTimePolicy::default());
        assert!(!line.contains("radar="), "{line}");
        // a rectangle without area or an empty picture gives no field
        assert_eq!(
            radar_field(&RadarInfo {
                picture: "p".into(),
                min: [0.0, 0.0],
                max: [10.0, 0.0]
            }),
            None
        );
        assert_eq!(
            radar_field(&RadarInfo {
                picture: " ".into(),
                min: [0.0, 0.0],
                max: [10.0, 10.0]
            }),
            None
        );
        // a ROM line's own radar token is dropped before ours is appended
        assert_eq!(
            without_token(
                "room key=AA radar=R.dtx|1,0,1|0,0,0|0.0|0.1|64|64 goal=5",
                "radar"
            ),
            "room key=AA goal=5"
        );
    }

    /// MAP-81: the record's JSON carries every field under its struct name, `None` as null, the goals as pairs.
    #[test]
    fn a_record_serialises_field_for_field_with_nulls_for_what_is_unset() {
        let mut with = record("valley", &["TDM", "BOMB"], &[("TDM", 80)], Some(300));
        with.record.description = Some("a valley".into());
        with.record.picture = Some("@mod:m/assets/pictures/valley.dtx".into());
        with.record.radar = Some(RadarInfo {
            picture: "@mod:m/assets/pictures/valley_radar.dtx".into(),
            min: [-100.0, -50.0],
            max: [300.0, 150.0],
        });
        with.record.players = Some(dc_mod_lua::PlayerRange { min: 2, max: 16 });
        let j = map_record_json(&with.record).to_json_string();
        let v = dc_mod_lua::JsonValue::parse(&j).unwrap();
        assert_eq!(v.get("id").and_then(|p| p.as_str()), Some("valley"));
        assert_eq!(v.get("mod_id").and_then(|p| p.as_str()), Some("m"));
        assert_eq!(
            v.get("world").and_then(|p| p.as_str()),
            Some("@mod:m/assets/maps/valley.dat")
        );
        assert_eq!(
            v.get("description").and_then(|p| p.as_str()),
            Some("a valley")
        );
        assert_eq!(
            v.get("modes").and_then(|p| p.as_array()).map(|a| a.len()),
            Some(2)
        );
        let goals = v.get("goals").and_then(|p| p.as_array()).unwrap();
        assert_eq!(goals.len(), 1);
        let pair = goals[0].as_array().unwrap();
        assert_eq!(pair[0].as_str(), Some("TDM"));
        assert_eq!(pair[1].as_i64(), Some(80));
        assert_eq!(v.get("round_seconds").and_then(|p| p.as_i64()), Some(300));
        assert_eq!(
            v.get("loading"),
            Some(&dc_mod_lua::JsonValue::Null),
            "unset: null, never missing"
        );
        let radar = v.get("radar").unwrap();
        assert_eq!(
            radar.get("picture").and_then(|p| p.as_str()),
            Some("@mod:m/assets/pictures/valley_radar.dtx")
        );
        assert_eq!(
            radar.get("max").and_then(|p| p.as_array()).map(|a| a.len()),
            Some(2)
        );
        assert_eq!(
            v.get("players")
                .and_then(|p| p.get("max"))
                .and_then(|p| p.as_i64()),
            Some(16)
        );
        assert_eq!(
            v.get("patched_by")
                .and_then(|p| p.as_array())
                .map(|a| a.len()),
            Some(0)
        );
        let bare = record("ruins", &["FFA"], &[], None);
        let j = map_record_json(&bare.record).to_json_string();
        let v = dc_mod_lua::JsonValue::parse(&j).unwrap();
        for k in [
            "description",
            "round_seconds",
            "picture",
            "loading",
            "radar",
            "players",
        ] {
            assert_eq!(v.get(k), Some(&dc_mod_lua::JsonValue::Null), "{k}");
        }
    }
}
