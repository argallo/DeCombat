//! Run-time tables from the user's ROM (`CA_ROM`, read-only): room table, item table, rank table, GMS files.
//! Nothing here is compiled in; without a ROM every lookup falls back to an empty table or a DeCombat default.

use dc_bute::{
    ButeDoc, GameRoomData, ItemsTable, RezVfs, RoomConfig, RoomOptions, RoundTimePolicy,
};
use std::path::{Path, PathBuf};

/// Equip slots of the four weapon kinds and the backpack.
// EVIDENCE: cshell+0x259f00 (item type -> slot, via dc_bute::equip_slot_by_class_code)
pub const SLOT_PRIMARY: i64 = 0;
pub const SLOT_THROW: i64 = 3;
pub const SLOT_BACKPACK: i64 = 9;
/// Default slot kind written to inventory record +0x28: 1 primary, 2 secondary, 3 melee, 4 throw, 6 back.
// SPEC: PROTOCOL_2009.md §3.3 table 3.3b (+0x28, LIVE)
pub const DEFAULT_KIND_BACKPACK: i64 = 6;

/// A starter item: catalogue number, equip slot, default slot kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StarterItem {
    pub item_no: i64,
    pub slot: i64,
    pub default_kind: i64,
}

/// C `atoi`: optional sign and leading digits, 0 without digits.
fn atoi(s: &str) -> i64 {
    let t = s.trim_start();
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let n = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i64, |a, d| {
            a.saturating_mul(10).saturating_add(i64::from(d - b'0'))
        });
    if neg {
        -n
    } else {
        n
    }
}

/// Experience thresholds and names of the ranks.
#[derive(Clone, Debug)]
pub struct RankTable {
    /// `need[i]` = experience needed to reach rank `i + 1`.
    pub need: Vec<i64>,
    /// Rank names by rank index (may be shorter than `need.len() + 1`).
    pub names: Vec<String>,
    /// CD-41: the GP bonus of each rank by rank index (`Bonus<n>`, C `atoi`; empty without the ROM table).
    // EVIDENCE: cshell+0x267ec0 (the bonus value of a level, atoi of the level table's bonus column), cshell+0x15fb50
    // (notice 0x487 names it as awarded)
    pub bonus: Vec<i64>,
    #[allow(dead_code)]
    pub from_rom: bool,
}

impl RankTable {
    /// The client's own table: `[LevelExp]` `Experience<n>` / `LevelName<n>` in the ROM's LEVELS.TXT. The client
    /// takes the rank of an exp value as the first index whose threshold exceeds it.
    // EVIDENCE: cshell+0x267f10 (rank of exp), cshell+0x268810 (LevelExp / LevelName / Experience table load)
    pub fn from_doc(doc: &ButeDoc) -> Option<Self> {
        let sec = doc.get("LevelExp")?;
        let mut need = Vec::new();
        while let Some(v) = sec.get(&format!("Experience{}", need.len())) {
            need.push(v.trim().parse::<i64>().ok()?);
        }
        if need.is_empty() {
            return None;
        }
        let mut names = Vec::new();
        while let Some(v) = sec.get(&format!("LevelName{}", names.len())) {
            names.push(v.trim().to_string());
        }
        let mut bonus = Vec::new();
        while let Some(v) = sec.get(&format!("Bonus{}", bonus.len())) {
            bonus.push(atoi(v.trim().trim_matches('"').trim()));
        }
        Some(Self {
            need,
            names,
            bonus,
            from_rom: true,
        })
    }

    /// Fallback when the ROM has no table.
    // DC-DEFAULT: CD-05 rank_exp(n) = round(100 * 1.18^(n-1)), cumulative, ranks 1..55
    pub fn fallback() -> Self {
        let mut need = Vec::with_capacity(55);
        let mut total = 0i64;
        for n in 1..=55i32 {
            total += (100.0 * 1.18f64.powi(n - 1)).round() as i64;
            need.push(total);
        }
        Self {
            need,
            names: Vec::new(),
            bonus: Vec::new(),
            from_rom: false,
        }
    }

    /// CD-41: the GP bonus of rank `r` (0 past the table, as the client reads row 0 there and row 0 is 0).
    pub fn bonus_of(&self, r: u32) -> i64 {
        self.bonus.get(r as usize).copied().unwrap_or(0).max(0)
    }

    pub fn top_rank(&self) -> usize {
        self.need.len()
    }

    /// Experience needed to hold rank `r` (rank 0 needs 0).
    pub fn exp_for(&self, r: usize) -> Option<i64> {
        if r == 0 {
            Some(0)
        } else {
            self.need.get(r - 1).copied()
        }
    }

    /// Rank held with `exp` experience.
    pub fn rank_of(&self, exp: i64) -> usize {
        self.need.iter().take_while(|&&n| exp >= n).count()
    }

    /// `--grant-rank`: a rank number or a rank name (case-insensitive).
    // SPEC: LOBBY_HARNESS.md §1 (--grant-rank; "major 3rd" matches case-insensitively)
    pub fn parse_rank(&self, text: &str) -> Option<usize> {
        let t = text.trim();
        if let Ok(n) = t.parse::<usize>() {
            return (n <= self.top_rank()).then_some(n);
        }
        let want = t.to_lowercase();
        self.names
            .iter()
            .position(|n| n.trim().to_lowercase() == want)
            .filter(|&r| r <= self.top_rank())
    }
}

pub struct RomData {
    pub dir: Option<PathBuf>,
    pub vfs: Option<RezVfs>,
    pub game_room: GameRoomData,
    /// The ROM item table (later tasks: shop, item windows).
    #[allow(dead_code)]
    pub items: Option<ItemsTable>,
    pub ranks: RankTable,
    pub starter: Vec<StarterItem>,
    /// `ItemType` of each `WeaponItem` by item number (ROM item table).
    pub weapon_kinds: std::collections::HashMap<i64, i32>,
    /// CLI-1606: the muzzle, sight and magazine socket states of each weapon by item number (ROM weapon table;
    /// a weapon without a row takes every option).
    pub socket_states: std::collections::HashMap<i64, [i32; 3]>,
    /// Shop list, colour table maxima and ROM build day (shop.rs).
    pub shop: crate::shop::ShopTables,
    /// `(Type, SubType)` of each `[Hisec<n>]` row of the ROM case table by n (hisec.rs: case row -> case item).
    // OBSERVED: ROM table key HISECS.TXT (entry of ATTRIBUTES.rez) [Hisec<n>] Type, SubType (values at run time)
    pub hisec_rows: std::collections::BTreeMap<u32, (i64, i64)>,
    /// LG-260: the faces the 2009 creation screen offers (`FacePieceId` of the `CREATEMODEL.TXT` rows with
    /// ViewType 0, in row order), for the bots' random faces.
    pub faces: Vec<i64>,
    pub notes: Vec<String>,
}

/// LG-260: the faces of `CREATEMODEL.TXT` (`[Create_Model<n>]` with ViewType 0), in row order.
// OBSERVED: ROM table CREATEMODEL.TXT [Create_Model<n>] FacePieceId, ViewType (dc-client CLI-1482 reads the same)
pub fn faces_from_doc(doc: &ButeDoc) -> Vec<i64> {
    let mut rows: Vec<(i32, i64)> = doc
        .iter()
        .filter_map(|(name, sec)| {
            let n: i32 = name.strip_prefix("Create_Model")?.parse().ok()?;
            let view: i64 = sec
                .get("ViewType")
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            if view != 0 {
                return None;
            }
            let face: i64 = sec
                .get("FacePieceId")
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            Some((n, face))
        })
        .collect();
    rows.sort_unstable();
    rows.into_iter().map(|r| r.1).collect()
}

/// xorshift64* step for the bots' picks (deterministic per seed, no dependency).
fn mix(seed: u64) -> u64 {
    let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    x.wrapping_mul(0x2545_f491_4f6c_dd1d)
}

fn slot_for_weapon_type(kind: i32) -> Option<i64> {
    dc_bute::equip_slot_by_class_code(kind)
        .map(|s| s as i64)
        .filter(|s| (SLOT_PRIMARY..=SLOT_THROW).contains(s))
}

/// The "balanced" default of one slot kind: among the unranked marked rows of the first marked row's `ItemType`,
/// the one in the middle of the damage order (the lower middle for an even count; ties by file order).
fn balanced_default(rows: &[(i32, i64, i64)]) -> Option<i64> {
    let class = rows.first()?.0;
    let mut same: Vec<(i64, usize, i64)> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.0 == class)
        .map(|(i, r)| (r.1, i, r.2))
        .collect();
    same.sort_unstable();
    same.get((same.len() - 1) / 2).map(|r| r.2)
}

/// Picks the starter loadout from the ROM item table: per weapon slot kind the `WeaponItem` that the table marks
/// as default issue (several rows may carry the mark: `balanced_default` chooses), else the first one with no rank requirement, else the first one;
/// plus the first backpack that adds at least one slot. Granted permanent.
// DC-DEFAULT: CD-18 starter loadout from the ROM item table by slot kind (no item numbers in code);
// DC-DEFAULT: CD-18 (refinement proposed, rust3) among several "(Default)"-marked rows of one slot kind the
// middle one by `Damage` is issued, not the first in file order (the user saw the high-damage rifle); `inventory.starter` overrides;
// DC-DEFAULT: CD-48 (proposed) starter backpack = first backpack with >= 1 extra slot
pub fn pick_starter(doc: &ButeDoc) -> Vec<StarterItem> {
    // marked default rows per slot kind: (ItemType, Damage, id) in file order
    let mut marked: [Vec<(i32, i64, i64)>; 4] = Default::default();
    let mut unranked: [Option<i64>; 4] = [None; 4];
    let mut first: [Option<i64>; 4] = [None; 4];
    let mut backpack: Option<i64> = None;
    for (tag, sec) in doc.iter() {
        let id = sec.get("ItemID").and_then(|v| v.trim().parse::<i64>().ok());
        if tag.starts_with("WeaponItem") {
            let (Some(id), Some(t)) = (
                id,
                sec.get("ItemType")
                    .and_then(|v| v.trim().parse::<i32>().ok()),
            ) else {
                continue;
            };
            let Some(slot) = slot_for_weapon_type(t) else {
                continue;
            };
            let k = slot as usize;
            let name = sec
                .get("ItemName")
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            if first[k].is_none() {
                first[k] = Some(id);
            }
            let rank = sec
                .get("RankPoint")
                .and_then(|v| v.trim().parse::<i64>().ok())
                .unwrap_or(-1);
            if unranked[k].is_none() && rank <= 0 {
                unranked[k] = Some(id);
            }
            if name.ends_with("(Default)") && rank <= 0 {
                let dmg = sec
                    .get("Damage")
                    .and_then(|v| v.trim().parse::<i64>().ok())
                    .unwrap_or(i64::MAX);
                marked[k].push((t, dmg, id));
            }
        } else if tag.starts_with("BackpackItem") && backpack.is_none() {
            let slots = sec
                .get("MultyInvenSlot")
                .and_then(|v| v.trim().parse::<i64>().ok())
                .unwrap_or(0);
            if let (Some(id), true) = (id, slots >= 1) {
                backpack = Some(7000 + id);
            }
        }
    }
    let mut out = Vec::new();
    for k in 0..4 {
        if let Some(no) = balanced_default(&marked[k]).or(unranked[k]).or(first[k]) {
            out.push(StarterItem {
                item_no: no,
                slot: k as i64,
                default_kind: k as i64 + 1,
            });
        }
    }
    if let Some(no) = backpack {
        out.push(StarterItem {
            item_no: no,
            slot: SLOT_BACKPACK,
            default_kind: DEFAULT_KIND_BACKPACK,
        });
    }
    out.extend(body_defaults(doc));
    out
}

/// Default body kinds of the inventory records (the weapons use 1..=4, the backpack 6).
pub const DEFAULT_KIND_UNIFORM: i64 = 7;
pub const DEFAULT_KIND_VEST: i64 = 8;
pub const DEFAULT_KIND_HEAD: i64 = 9;

/// The default body items: the first row (ItemID 0) of the uniform table, permanent and equipped in its own slot; no
/// vest (LG-90), no head item (LG-120). (table tag, item number base of the type range, slot, default kind)
// SPEC: PROTOCOL_2009.md §4.5 (item number ranges 2000 uniform, 3000 vest, 4000 head)
// DC-DEFAULT: LG-51 (playtest report: "character doesn't show default items under body") new accounts, and old ones
// at login, own the ItemID 0 uniform ("Basic" costume), vest and head rows as default items; the ROM marks no body
// row as a default, the 2009 server's choice is not on record. Character and face have no default (no item number
// range of the Character table is on record).
// DC-DEFAULT: LG-90 (playtest report: "medium vest should not be a default, no vest is default") the ItemID 0 vest row
// is no default item: an account starts without a vest (the body list's vest row shows its empty-slot name), and
// the login repair removes the default vest an older account was given (`inventory::repair_defaults`)
// DC-DEFAULT: LG-120 (playtest report: "Normal shows as an item but default is no hat") the ItemID 0 head row is no
// default item either: the 2009 look starts from head row 0, whose model is empty and stands for the face's own hair
// (EVIDENCE: cshell+0x12cc60, cshell+0x1e3b00), so an account starts without a head item (the body list's head row
// shows its empty-slot name) and the login repair removes the default head an older account was given
pub fn body_defaults(doc: &ButeDoc) -> Vec<StarterItem> {
    let mut out = Vec::new();
    for (tag, base, slot, kind) in [("CamoItem", 2000, 7, DEFAULT_KIND_UNIFORM)] {
        let has = doc.iter().any(|(t, sec)| {
            t.starts_with(tag)
                && sec.get("ItemID").and_then(|v| v.trim().parse::<i64>().ok()) == Some(0)
        });
        if has {
            out.push(StarterItem {
                item_no: base,
                slot,
                default_kind: kind,
            });
        }
    }
    out
}

/// `ItemID -> ItemType` of the `WeaponItem` sections.
pub fn weapon_kinds(doc: &ButeDoc) -> std::collections::HashMap<i64, i32> {
    doc.iter()
        .filter(|(tag, _)| tag.starts_with("WeaponItem"))
        .filter_map(|(_, sec)| {
            let id = sec.get("ItemID")?.trim().parse::<i64>().ok()?;
            let kind = sec.get("ItemType")?.trim().parse::<i32>().ok()?;
            Some((id, kind))
        })
        .collect()
}

impl RomData {
    pub fn empty() -> Self {
        Self {
            dir: None,
            vfs: None,
            game_room: GameRoomData::from_doc(ButeDoc::new()),
            items: None,
            ranks: RankTable::fallback(),
            starter: Vec::new(),
            weapon_kinds: Default::default(),
            socket_states: Default::default(),
            shop: Default::default(),
            hisec_rows: Default::default(),
            faces: Vec::new(),
            notes: vec![
                "CA_ROM not set: room, item and rank tables are empty (DeCombat fallbacks in use)"
                    .into(),
            ],
        }
    }

    /// Opens the ROM tables. Missing pieces are noted, never fatal.
    pub fn load(dir: &Path) -> Self {
        let vfs = RezVfs::new(dir);
        let mut notes = Vec::new();
        let game_room = match GameRoomData::from_vfs(&vfs) {
            Ok(g) => g,
            Err(e) => {
                notes.push(format!("room table unavailable: {}", e));
                GameRoomData::from_doc(ButeDoc::new())
            }
        };
        let items = match ItemsTable::from_vfs(&vfs) {
            Ok(t) => Some(t),
            Err(e) => {
                notes.push(format!("item table unavailable: {}", e));
                None
            }
        };
        let ranks = ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", "ATTRIBUTES\\LEVELS.TXT")
            .and_then(|d| RankTable::from_doc(&d))
            .unwrap_or_else(|| {
                notes.push("rank table unavailable: CD-05 fallback".into());
                RankTable::fallback()
            });
        let starter = items
            .as_ref()
            .map(|t| pick_starter(t.doc()))
            .unwrap_or_default();
        let weapon_kinds = items
            .as_ref()
            .map(|t| weapon_kinds(t.doc()))
            .unwrap_or_default();
        let socket_states = match dc_bute::WeaponsTable::from_vfs(&vfs) {
            Ok(t) => t
                .option_socket_states()
                .into_iter()
                .map(|(n, st)| (n as i64, st))
                .collect(),
            Err(e) => {
                notes.push(format!("weapon table unavailable: {}", e));
                Default::default()
            }
        };
        let (shop, shop_notes) = crate::shop::ShopTables::load(&vfs, dir);
        notes.extend(shop_notes);
        let hisec_rows = ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", "ATTRIBUTES\\HISECS.TXT")
            .map(|d| crate::hisec::rows_from_doc(&d))
            .unwrap_or_else(|| {
                notes.push(
                    "HiSec case table unavailable: hisec.case_product or the first ROM case".into(),
                );
                Default::default()
            });
        let faces = ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", "ATTRIBUTES\\CREATEMODEL.TXT")
            .map(|d| faces_from_doc(&d))
            .unwrap_or_default();
        Self {
            dir: Some(dir.to_path_buf()),
            vfs: Some(vfs),
            game_room,
            items,
            ranks,
            starter,
            weapon_kinds,
            socket_states,
            shop,
            hisec_rows,
            faces,
            notes,
        }
    }

    /// LG-260: a random kit for a bot: one GP primary (slot 0) and one GP secondary (slot 1) among the products
    /// the shop lists, picked by `seed`; `None` without a shop catalogue or without such products. The text is
    /// `--start-loadout` syntax (`0:<item>,1:<item>`).
    pub fn random_kit(&self, seed: u64) -> Option<String> {
        let cat = self.shop.catalog.as_ref()?;
        let mut by_slot: std::collections::BTreeMap<i64, Vec<i64>> = Default::default();
        for pid in cat.view_list() {
            let Some(p) = cat.product(pid) else { continue };
            if p.sale_type != 0 || p.items.is_empty() {
                continue;
            }
            // one item, permanent or a period product alike: the seed writes the item itself
            let item = i64::from(p.items[0].0);
            let Some(slot) = self.slot_of_item(item) else {
                continue;
            };
            if slot == 0 || slot == 1 {
                let v = by_slot.entry(slot).or_default();
                if !v.contains(&item) {
                    v.push(item);
                }
            }
        }
        let primaries = by_slot.get(&0).filter(|v| !v.is_empty())?;
        let r1 = mix(seed);
        let mut parts = vec![format!(
            "0:{}",
            primaries[(r1 % primaries.len() as u64) as usize]
        )];
        if let Some(secondaries) = by_slot.get(&1).filter(|v| !v.is_empty()) {
            let r2 = mix(r1);
            parts.push(format!(
                "1:{}",
                secondaries[(r2 % secondaries.len() as u64) as usize]
            ));
        }
        Some(parts.join(","))
    }

    /// LG-260: a random face of the creation table by `seed` (`None` without the table).
    pub fn random_face(&self, seed: u64) -> Option<i64> {
        if self.faces.is_empty() {
            return None;
        }
        Some(self.faces[(mix(seed ^ 0xface) % self.faces.len() as u64) as usize])
    }

    pub fn map_count(&self) -> usize {
        self.game_room.maps().len()
    }

    pub fn mode_count(&self, clan: bool) -> usize {
        self.game_room.modes(clan).len()
    }

    /// Mode key text (e.g. the result line's `mode=`), empty when unknown.
    pub fn mode_key(&self, mode: u32) -> String {
        self.game_room
            .modes(false)
            .get(mode as usize)
            .and_then(|m| m.get("Key").cloned())
            .unwrap_or_default()
    }

    /// `(map index, mode bit mask)` pairs for 0x311.
    // DC-DEFAULT: CD-46 masks from the user's ROM room table
    pub fn map_mode_masks(&self) -> Vec<(usize, u32)> {
        self.game_room.map_mode_masks(false)
    }

    /// True when map `map` is enabled and its table has a GMS for mode row `mode` (the 0x311 mask bit).
    pub fn map_plays_mode(&self, map: u32, mode: u32) -> bool {
        mode < 32
            && self
                .map_mode_masks()
                .iter()
                .any(|&(m, mask)| m as u32 == map && mask & (1u32 << mode) != 0)
    }

    /// Default map and goal of a mode.
    // DC-DEFAULT: CD-25 the map is the mode's default map from the ROM room table
    pub fn mode_defaults(&self, mode: u32) -> (u32, u32) {
        let mut map = self.game_room.default_map(mode as usize, false).max(0) as u32;
        // DC-DEFAULT: LG-2 (proposed) when the mode row's default map has no GMS for the mode (the co-op row's default
        // map has none), the room takes the first enabled map whose table has one (LB-FT-1)
        let maps = self.game_room.maps();
        let key = self.mode_key(mode);
        let lacks = !key.is_empty()
            && maps
                .get(map as usize)
                .is_some_and(|m| !m.contains_key(&format!("GMS_{key}")));
        if lacks && mode < 32 {
            if let Some((first, _)) = self
                .game_room
                .map_mode_masks(false)
                .into_iter()
                .find(|(_, mask)| mask & (1u32 << mode) != 0)
            {
                map = first as u32;
            }
        }
        let goal = self.game_room.default_goal(mode as usize, false).max(0) as u32;
        (map, goal)
    }

    /// `[MapExp]` / `[MapMoney]` `level_correction` of the room's GMS (1.0 when absent).
    // DC-DEFAULT: CD-03 / CD-04 map factor from the ROM .gms
    pub fn map_factors(&self, map: u32, mode: u32) -> (f64, f64) {
        let Some(vfs) = &self.vfs else {
            return (1.0, 1.0);
        };
        let (_md, g) =
            dc_bute::gms_file_with_vfs(vfs, &self.game_room, map as usize, mode as usize, false);
        (
            g.get_f64("MapExp", "level_correction", 1.0),
            g.get_f64("MapMoney", "level_correction", 1.0),
        )
    }

    /// World path for a room (`--dserver-world` wins).
    pub fn world_for(&self, map: u32, mode: u32, forced: &str) -> String {
        if !forced.is_empty() {
            return forced.to_string();
        }
        match &self.dir {
            Some(d) => dc_bute::world_for_room(
                d,
                &self.game_room,
                map as usize,
                mode as usize,
                false,
                None,
            ),
            None => String::new(),
        }
    }

    /// The `room key=...` control line for the dedicated server with the operator's round-time fallback: `time=` is
    /// the mode file's limit, else `round_clock`'s seconds for the row's mode class.
    // SPEC: DSERVER_PIPE.md §3.2 (`time=<s>`)
    // DC-DEFAULT: GMD-2 (clarification) every room has a time limit, see dc_bute::RoundTimePolicy
    /// `time=` is `time_override` when given (a community map's `round_seconds`, MAP-37), else the mode file's
    /// limit, else the `round_clock` fallback. Rooms go through `MapTable::room_line`.
    #[allow(clippy::too_many_arguments)]
    pub fn room_line(
        &self,
        map: u32,
        mode: u32,
        goal: u32,
        ff: u32,
        arms: u32,
        round_clock: &RoundTimePolicy,
        time_override: Option<i32>,
    ) -> String {
        // DC-DEFAULT: DSV-180 (proposed) the room's weapon limit reaches the game server as `arms=`
        let cfg = RoomConfig {
            map: map as usize,
            mode: mode as usize,
            clan: false,
            goal: goal as i32,
            ff: ff as i32,
            arms: arms as i32,
            time_override,
            ..RoomConfig::default()
        };
        match &self.vfs {
            Some(v) => {
                let opts = RoomOptions {
                    round_clock: round_clock.clone(),
                    ..RoomOptions::default()
                };
                dc_bute::room_line_with_vfs(v, &self.game_room, &cfg, &opts)
            }
            None => {
                let time = time_override
                    .filter(|t| *t != 0)
                    .map(|t| format!(" time={t}"))
                    .unwrap_or_default();
                format!("room goal={} ff={} arms={}{}", goal, ff, arms, time)
            }
        }
    }

    /// Slot an item number goes to when an operator lists it in `inventory.starter`.
    pub fn slot_of_item(&self, item_no: i64) -> Option<i64> {
        if (7000..8000).contains(&item_no) {
            return Some(SLOT_BACKPACK);
        }
        self.weapon_kinds
            .get(&item_no)
            .and_then(|&k| slot_for_weapon_type(k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(t: &str) -> ButeDoc {
        ButeDoc::parse(t)
    }

    #[test]
    fn starter_prefers_marked_then_unranked_then_first() {
        // synthetic fixture (no ROM data): types 1 / 0 primary, 4 secondary, 7 melee, 6 throw, 16 backpack
        let d = doc("[WeaponItem0]\nItemID=10\nItemType=1\nRankPoint=3\nItemName=A\n\
             [WeaponItem1]\nItemID=11\nItemType=0\nRankPoint=-1\nItemName=B\n\
             [WeaponItem2]\nItemID=12\nItemType=0\nRankPoint=-1\nItemName=C (Default)\n\
             [WeaponItem3]\nItemID=20\nItemType=4\nRankPoint=5\nItemName=D\n\
             [WeaponItem4]\nItemID=30\nItemType=7\nItemName=E\n\
             [WeaponItem5]\nItemID=40\nItemType=6\nItemName=F\n\
             [BackpackItem0]\nItemID=0\nMultyInvenSlot=0\n[BackpackItem1]\nItemID=4\nMultyInvenSlot=2\n");
        let s = pick_starter(&d);
        let got: Vec<(i64, i64, i64)> = s
            .iter()
            .map(|x| (x.item_no, x.slot, x.default_kind))
            .collect();
        assert_eq!(
            got,
            vec![(12, 0, 1), (20, 1, 2), (30, 2, 3), (40, 3, 4), (7004, 9, 6)]
        );
    }

    #[test]
    fn starter_takes_the_middle_damage_among_several_marked_defaults() {
        // synthetic fixture: two marked primaries (high damage first in file order), three marked secondaries,
        // a marked SMG that must not join the first primary's class
        let d = doc("[WeaponItem0]
ItemID=10
ItemType=0
Damage=48
ItemName=A (Default)
             [WeaponItem1]
ItemID=11
ItemType=0
Damage=38
ItemName=B (Default)
             [WeaponItem2]
ItemID=20
ItemType=4
Damage=33
ItemName=C (Default)
             [WeaponItem3]
ItemID=21
ItemType=4
Damage=38
ItemName=D (Default)
             [WeaponItem4]
ItemID=22
ItemType=4
ItemName=E (Default)
             [WeaponItem5]
ItemID=12
ItemType=1
Damage=1
ItemName=F (Default)
");
        let got: Vec<i64> = pick_starter(&d).iter().map(|x| x.item_no).collect();
        assert_eq!(got, vec![11, 21]);
    }

    #[test]
    fn default_map_without_a_gms_falls_to_the_first_map_that_has_one() {
        // synthetic room table (key names only): mode 0 TDM, mode 1 COOP whose default map 0 has no GMS_COOP
        let mut r = RomData::empty();
        r.game_room = dc_bute::GameRoomData::from_doc(doc("[Mode0]
Key=TDM
DefaultMapID=0
DefaultGoal=50
EnablePlay=TRUE
             [Mode1]
Key=COOP
DefaultMapID=0
DefaultGoal=7
EnablePlay=TRUE
             [Map0]
Enable=TRUE
GMS_TDM=a.gms
             [Map1]
Enable=FALSE
GMS_COOP=b.gms
             [Map2]
Enable=TRUE
GMS_COOP=c.gms
"));
        assert_eq!(r.mode_defaults(0), (0, 50), "default map has the GMS: kept");
        assert_eq!(
            r.mode_defaults(1),
            (2, 7),
            "first enabled map with GMS_COOP"
        );
    }

    #[test]
    fn rank_table_semantics() {
        let d = doc("[LevelExp]\nExperience0=100\nExperience1=300\nExperience2=600\nLevelName0=Zero\nLevelName1=One\nLevelName2=Two Words\n");
        let t = RankTable::from_doc(&d).unwrap();
        assert_eq!(t.rank_of(0), 0);
        assert_eq!(t.rank_of(99), 0);
        assert_eq!(t.rank_of(100), 1);
        assert_eq!(t.rank_of(650), 3);
        assert_eq!(t.exp_for(0), Some(0));
        assert_eq!(t.exp_for(2), Some(300));
        // CD-41: no Bonus column, no bonus
        assert_eq!(t.bonus_of(1), 0);
        let d = doc("[LevelExp]\nExperience0=100\nBonus0=\"0\"\nBonus1=\"1000\"\nBonus2=x\n");
        let b = RankTable::from_doc(&d).unwrap();
        assert_eq!(
            (b.bonus_of(0), b.bonus_of(1), b.bonus_of(2), b.bonus_of(9)),
            (0, 1000, 0, 0)
        );
        assert_eq!(t.parse_rank("two words"), Some(2));
        assert_eq!(t.parse_rank("3"), Some(3));
        assert_eq!(t.parse_rank("4"), None);
        assert_eq!(t.parse_rank("nope"), None);
    }

    #[test]
    fn fallback_rank_table_is_cd05() {
        let t = RankTable::fallback();
        assert_eq!(t.top_rank(), 55);
        assert_eq!(t.need[0], 100);
        assert_eq!(t.need[1], 100 + 118);
        assert!(t.need.windows(2).all(|w| w[0] < w[1]));
    }

    /// LG-314: the reserved pack numbers are free in the user's ROM.
    #[test]
    fn the_rom_leaves_the_pack_numbers_free() {
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let r = RomData::load(&rom);
        assert!(!r.weapon_kinds.is_empty());
        let taken: Vec<_> = r.weapon_kinds.keys().filter(|n| dc_bute::modpatch::is_pack_item(**n)).collect();
        assert!(taken.is_empty(), "{taken:?}");
    }

    #[test]
    fn rom_tables_load_when_available() {
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let r = RomData::load(&rom);
        assert!(
            r.map_count() > 0 && r.mode_count(false) > 0,
            "{:?}",
            r.notes
        );
        assert!(r.ranks.from_rom && r.ranks.top_rank() >= 55);
        // the harness grant suite's rank numbers follow the client's table (test_account_grant)
        assert_eq!(r.ranks.parse_rank("major 3rd"), Some(38));
        assert_eq!(r.ranks.parse_rank("General Of The Army"), Some(55));
        // CD-41: the level table's GP bonus column (LEVELS.TXT Bonus1 = 1000, Bonus50 = 9000, Bonus55 = 0)
        assert_eq!(
            (
                r.ranks.bonus_of(1),
                r.ranks.bonus_of(50),
                r.ranks.bonus_of(55)
            ),
            (1000, 9000, 0)
        );
        assert_eq!(r.starter.len(), 6, "{:?}", r.starter);
        let slots: Vec<i64> = r.starter.iter().map(|s| s.slot).collect();
        // LG-51: then the default uniform (LG-90: no default vest, LG-120: no default head)
        assert_eq!(slots, vec![0, 1, 2, 3, 9, 7]);
        // rust3: the ROM carries two "(Default)" rifles and two pistols; the middle / lower-middle damage ones are issued
        let nos: Vec<i64> = r.starter.iter().map(|s| s.item_no).collect();
        assert_eq!(&nos[..2], &[145, 164], "{:?}", r.starter);
        // SHOP-1: the catalogue holds products the ROM hides (Shopview 0)
        let cat = r.shop.catalog.as_ref().expect("shop list");
        assert!(r.shop.all_products.len() > cat.view_list().len());
        assert!(!r.map_mode_masks().is_empty());
    }

    /// Every map / mode pair the lobby offers (the plain room table's masks, the playable modes) maps to a world the
    /// dedicated server's loader reads; the table is printed for the launcher's help (play_rust.py --map).
    #[test]
    fn every_offered_map_loads_in_the_dserver() {
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let r = RomData::load(&rom);
        if dc_bute::world_names(&rom).is_empty() {
            println!("skipped: WORLDS.rez unavailable");
            return;
        }
        let maps = r.game_room.maps();
        let modes = r.game_room.modes(false);
        let key = |m: Option<&dc_bute::SectionMap>| {
            m.and_then(|m| m.get("Key").cloned()).unwrap_or_default()
        };
        let mut bad = Vec::new();
        for (map, mask) in r.map_mode_masks() {
            let mut line = Vec::new();
            for mode in 0..modes.len().min(32) {
                if mask & (1 << mode) == 0 {
                    continue;
                }
                let world = r.world_for(map as u32, mode as u32, "");
                let res = dc_dserver::world::load_from_rom(
                    &rom,
                    &world,
                    dc_dserver::world::TODO_WORLD_NAME_FORM,
                    None,
                );
                let tag = match &res {
                    Ok(w) => format!("{} ok ({} spawns)", key(modes.get(mode)), w.spawns.len()),
                    Err(e) => {
                        bad.push(format!(
                            "{} {} {world}: {e}",
                            key(maps.get(map)),
                            key(modes.get(mode))
                        ));
                        format!("{} FAIL", key(modes.get(mode)))
                    }
                };
                line.push(format!("{tag} {world}"));
            }
            println!("map {map} {}: {}", key(maps.get(map)), line.join("; "));
        }
        assert!(bad.is_empty(), "worlds the dserver cannot load: {bad:#?}");
    }

    #[test]
    fn world_for_room_follows_the_gms_map() {
        // HARNESS: lobby_harness::test_world_for_room (map Showdown + mode QRT -> ...warehouse_qrt;
        // map 0 + mode TDM -> Worlds\Snow...)
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let r = RomData::load(&rom);
        if dc_bute::world_names(&rom).is_empty() {
            println!("skipped: WORLDS.rez unavailable");
            return;
        }
        let maps = r.game_room.maps();
        let modes = r.game_room.modes(false);
        let key = |m: &dc_bute::SectionMap| m.get("Key").cloned().unwrap_or_default();
        let qrt = modes.iter().position(|m| key(m) == "QRT").unwrap() as u32;
        let tdm = modes.iter().position(|m| key(m) == "TDM").unwrap() as u32;
        let sd = maps
            .iter()
            .position(|m| {
                m.get("Name")
                    .map(|s| s.to_lowercase() == "showdown")
                    .unwrap_or(false)
                    || key(m).to_lowercase() == "showdown"
            })
            .unwrap() as u32;
        assert_eq!(
            r.world_for(sd, qrt, "").to_lowercase(),
            "worlds\\warehouse_qrt"
        );
        assert!(r
            .world_for(0, tdm, "")
            .to_lowercase()
            .starts_with("worlds\\snow"));
        assert_eq!(r.world_for(0, tdm, "Worlds\\X"), "Worlds\\X");
    }

    // LG-260: the creation table's faces in row order, ViewType 0 only; the random pick is stable per seed
    #[test]
    fn faces_follow_the_creation_table_and_the_random_pick_is_stable() {
        let d = doc("[Create_Model2]\nFacePieceId = 5\nViewType = 0\n[Create_Model0]\nFacePieceId = 0\nViewType = 0\n[Create_Model1]\nFacePieceId = 1\nViewType = 1\n");
        assert_eq!(faces_from_doc(&d), vec![0, 5]);
        let mut r = RomData::empty();
        assert_eq!(r.random_face(3), None);
        r.faces = vec![0, 2, 5, 7];
        let a = r.random_face(11).unwrap();
        assert_eq!(r.random_face(11), Some(a));
        assert!(r.faces.contains(&a));
        let picks: std::collections::BTreeSet<i64> =
            (0..40).filter_map(|s| r.random_face(s)).collect();
        assert!(picks.len() > 1);
    }

    #[test]
    fn rom_random_kit_gives_a_gp_primary_and_secondary_per_seed() {
        let Some(dir) = std::env::var_os("CA_ROM")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_dir())
        else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let r = RomData::load(&dir);
        assert!(r.faces.len() > 1, "faces {:?}", r.faces);
        let kit = r.random_kit(1).expect("a kit");
        let parts: Vec<&str> = kit.split(',').collect();
        assert!(
            parts[0].starts_with("0:") && parts.get(1).is_some_and(|p| p.starts_with("1:")),
            "{kit}"
        );
        for p in &parts {
            let item: i64 = p[2..].parse().unwrap();
            assert_eq!(r.slot_of_item(item), Some(p[..1].parse().unwrap()), "{kit}");
        }
        assert_eq!(r.random_kit(1), Some(kit.clone()), "stable");
        let kits: std::collections::BTreeSet<String> =
            (0..30).filter_map(|s| r.random_kit(s)).collect();
        assert!(kits.len() > 3, "{kits:?}");
    }
}
