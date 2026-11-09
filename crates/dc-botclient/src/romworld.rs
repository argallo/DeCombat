//! Optional ROM access of the bot (the user's read-only client folder, `CA_ROM`): the 2009 client opens the
//! world named in `S2C_FILE_LIST` from its own resource archives, so the bot does the same lookup to tell
//! whether a server's announced name would load (else the real client ends with 0x29), and takes from the
//! world file what a client needs for the session: the box of `pos48` positions and the collision geometry.
//! Nothing read here is stored outside memory; only sizes and hashes are logged.
// SPEC: UDP_SESSION_2009_A_addendum_disasm.md A9 (name form: relative path, `\` or `/`, case-insensitive,
// extension required), A10; UDP_SESSION_2009_A.md §4.6 step 4 (no stream -> 0x29)
// SPEC: UDP_TRANSPORT_2009_addendum_disasm.md T14 (pos48 box = the world file's extents)

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dc_rez::RezArchive;
use dc_sim::from_world::{
    brush_volume, collision_world, ladder_volumes, selected_models, WorldFilter,
};
use dc_sim::{CollModel, CollisionWorld, ContainerVolumes, ConvexVolume};
use dc_world::{ObjectRole, ParseOptions, World, WORLD_VERSION};

use crate::objects::{kind, ExtentBox, ObjectTable};

/// The world archive inside `<rom>/Game` (the archive `dc-bute` and `dc-dserver` list worlds from).
// OBSERVED: dc-dserver world.rs WORLD_ARCHIVE (the same archive name, read at run time)
pub const WORLD_ARCHIVE: &str = "WORLDS.rez";

/// CShell rva of the u16 the 2009 client sends in its integrity answer 0xac sub 0 (PE-DATA VA 0x374d3b6c).
// SPEC: GAME_MESSAGES_2009_C2S.md §5.6 (`version` = the u16 at PE-DATA VA 0x374d3b6c)
pub const INTEGRITY_VERSION_RVA: u32 = 0x374d_3b6c - 0x3710_0000;

/// A world the bot loaded from the ROM.
pub struct LoadedWorld {
    /// The archive entry the announced name resolved to.
    pub listed: String,
    pub size: usize,
    /// First 16 hex digits of the SHA-256 of the world bytes.
    pub sha16: String,
    pub extents: ExtentBox,
    pub collision: Arc<CollisionWorld>,
    /// The ladder volumes (one per ladder brush), fed to the body's movement.
    // SPEC: MOVEMENT_2009.md §11.1, §11.4; DC-DEFAULT: BOT-61 (proposed)
    pub ladders: Arc<ContainerVolumes>,
    pub player_spawns: usize,
    /// BOT-160: the player spawn points' places (every mode's), in the file's order: the places every bot of a room
    /// reads alike, from which the Quarantine humans pick their stand.
    pub spawn_points: Vec<[f32; 3]>,
    /// BOT-110: the collision set with every linked world model in it, the linked models' place in it, and the state
    /// the server's objects gave them last ([`LoadedWorld::follow_server`]); `collision` is this set with the
    /// models the server has not made solid emptied.
    pub full: Arc<CollisionWorld>,
    pub linked: Vec<LinkedModel>,
    pub solid: Option<Vec<bool>>,
    /// BOT-111: the world's kill volumes (lower-case name, the region inside the brush's planes).
    pub kill_volumes: Vec<(Vec<u8>, ConvexVolume)>,
    /// BOT-143: the world's hazard volumes that hurt (a damage property above 0: Warhead's and Two Towers' water),
    /// as regions. They have no object on the clients and act on the server unless its start scripts switch them
    /// off (DSV-607); BOT-177: the ones the bot takes as acting (all until [`LoadedWorld::hazards_for_room`]).
    pub hazard_volumes: Vec<ConvexVolume>,
    /// BOT-177: every hazard volume that hurts, with its lower-case name.
    pub hazard_named: Vec<(Vec<u8>, ConvexVolume)>,
    /// BOT-177: the names of the hazard volumes the server spares an infected player (DSV-607 amend: poison type, below
    /// the lethal rate per second: gas, not deadly water).
    pub hazard_gas: Vec<Vec<u8>>,
    /// BOT-177: the volumes the room's start scripts switched off (names), and whether the bot is infected now.
    pub hazard_off: Vec<Vec<u8>>,
    pub infected: bool,
    /// BOT-177: the world's start scripts (the code the game server evaluates them with, `dc_world::scripts`).
    pub scripts: dc_world::scripts::StartScripts,
}

/// A world model an object of the world file links to by name, in the bot's collision set.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkedModel {
    /// The object's name, lower case (the server's named brush carries it).
    pub name: Vec<u8>,
    /// Index in the collision set's model list.
    pub index: usize,
    /// A door or switch: solid only while its object stands where the server created it (closed).
    pub mover: bool,
}

/// Flag bit of a named brush that makes it solid on the clients (the client's collision rule).
// SPEC: UDP_SESSION_2009_B.md §4.2 item 3; DC-DEFAULT: CLI-230 (proposed) 0x2000 collides
pub const BRUSH_FLAG_SOLID: u16 = 0x2000;
/// A mover counts as moved (open) once its object lies this far from its created position.
// DC-DEFAULT: CLI-251 (proposed) 0.5 units
pub const MOVER_MOVED_EPS: f32 = 0.5;
/// Container code of a kill volume in a kind-9 create.
// SPEC: UDP_SESSION_2009_B.md §3.3 (kind 9 `[name:zstr8][code:u16]`); DC_DEFAULTS DSV-37 (volumes: kill volume 7)
pub const KILL_VOLUME_CODE: u16 = 7;

/// Whether each linked model is solid by the server's objects: a named brush of its name with the solid flag (the
/// 2009 client collides with what the server creates and nothing else), a door or switch only while it has not
/// moved or turned away from where it was created.
// SPEC: UDP_SESSION_2009_B.md §3.3 (kind 2 `[name]`), §3.5.1 ("the client collides with what the server creates"),
// §4.2 items 3, 6, 7 (flag words, position, rotation)
// DC-DEFAULT: BOT-110 (proposed) the bot's collision follows the server's named brushes as the client's does
pub fn server_solid(linked: &[LinkedModel], table: &ObjectTable) -> Vec<bool> {
    let mut brushes: std::collections::HashMap<Vec<u8>, &crate::objects::ClientObject> =
        std::collections::HashMap::new();
    for o in table.objects.values() {
        if o.kind == kind::BRUSH {
            if let Some(n) = &o.name {
                brushes.insert(n.to_ascii_lowercase(), o);
            }
        }
    }
    linked
        .iter()
        .map(|l| {
            let Some(o) = brushes.get(&l.name) else {
                return false;
            };
            if o.flags & BRUSH_FLAG_SOLID == 0 {
                return false;
            }
            if !l.mover {
                return true;
            }
            let moved = match (o.samples.first().map(|x| x.0), o.pos) {
                (Some(a), Some(p)) => {
                    (0..3).map(|k| (p[k] - a[k]).powi(2)).sum::<f32>().sqrt() > MOVER_MOVED_EPS
                }
                _ => false,
            };
            let turned = o.rot.is_some_and(|r| r[3].abs() < 1.0 - 1e-4);
            !moved && !turned
        })
        .collect()
}

impl LoadedWorld {
    /// Applies the server's objects to the collision set. `Some((collision, gone))` when the solid set changed: the
    /// new set and the indices of the models that left it (a body standing on one is detached).
    // DC-DEFAULT: BOT-110 (proposed)
    pub fn follow_server(
        &mut self,
        table: &ObjectTable,
    ) -> Option<(Arc<CollisionWorld>, Vec<u32>)> {
        if self.linked.is_empty() {
            return None;
        }
        let solid = server_solid(&self.linked, table);
        if self.solid.as_ref() == Some(&solid) {
            return None;
        }
        let was = self.solid.replace(solid.clone());
        let mut c = (*self.full).clone();
        let mut gone = Vec::new();
        for (i, l) in self.linked.iter().enumerate() {
            if !solid[i] && l.index < c.models.len() {
                c.models[l.index] = CollModel::new(&[], &[], None);
                if was.as_ref().is_none_or(|w| w[i]) {
                    gone.push(l.index as u32);
                }
            }
        }
        self.collision = Arc::new(c);
        Some((self.collision.clone(), gone))
    }

    /// BOT-177: takes the hazard volumes named in `off` as switched off by the room's start scripts (the others as
    /// acting). Returns whether the acting set changed.
    // DC-DEFAULT: BOT-177 (proposed)
    pub fn hazards_for_room(&mut self, off: &[Vec<u8>]) -> bool {
        self.hazard_off = off.to_vec();
        self.refresh_hazards()
    }

    /// BOT-177: the bot is infected (or human again): the gas volumes the server spares the infected act for it no
    /// more (they come back when it is human). Returns whether the acting set changed.
    // DC-DEFAULT: BOT-177 (proposed)
    pub fn set_infected(&mut self, infected: bool) -> bool {
        if self.infected == infected {
            return false;
        }
        self.infected = infected;
        self.refresh_hazards()
    }

    fn refresh_hazards(&mut self) -> bool {
        let mut off = self.hazard_off.clone();
        if self.infected {
            off.extend(self.hazard_gas.iter().cloned());
        }
        let acting = acting_hazards(&self.hazard_named, &off);
        let changed = acting != self.hazard_volumes;
        self.hazard_volumes = acting;
        changed
    }

    /// The kill volumes the server created (kind 9 with the kill volume code, matched by name), as regions.
    // DC-DEFAULT: BOT-111 (proposed)
    pub fn active_kill_volumes(&self, table: &ObjectTable) -> Vec<ConvexVolume> {
        let mut v = self.active_deadly_volumes(table);
        // BOT-143: and the hazard volumes (no object of theirs reaches the client)
        v.extend(self.hazard_volumes.iter().cloned());
        v
    }

    /// BOT-154: [`Self::active_kill_volumes`] without the hazard volumes (the ones that kill; the grid takes the
    /// hazard volumes apart, `NavGrid::with_hazard_volumes`).
    // DC-DEFAULT: BOT-111 (proposed)
    pub fn active_deadly_volumes(&self, table: &ObjectTable) -> Vec<ConvexVolume> {
        let names: Vec<Vec<u8>> = table
            .objects
            .values()
            .filter(|o| o.kind == kind::ZONE && o.code == Some(KILL_VOLUME_CODE))
            .filter_map(|o| o.name.as_ref().map(|n| n.to_ascii_lowercase()))
            .collect();
        self.kill_volumes
            .iter()
            .filter(|(n, _)| names.contains(n))
            .map(|(_, v)| v.clone())
            .collect()
    }
}

/// Parses world bytes into what the bot keeps (box, collision, spawn count).
// SPEC: FMT_DAT85.md §4 (extents), §5.2 / §9.1 (collision through dc-sim), §6.2 (player spawn role)
pub fn world_from_bytes(name: &str, data: &[u8]) -> Result<LoadedWorld, String> {
    let w = World::parse_with(data, ParseOptions { render: false })
        .map_err(|e| format!("world parse: {e}"))?;
    // BOT-110: every linked model (doors and switches too) is in the set; the server's objects switch them
    // (`LoadedWorld::follow_server`); the file's own set until they arrive
    let filter = WorldFilter::default();
    let collision = Arc::new(collision_world(&w, &filter));
    let linked = selected_models(&w, &filter)
        .into_iter()
        .enumerate()
        .filter_map(|(index, m)| {
            let o = w.object_named(w.models[m].name)?;
            Some(LinkedModel {
                name: w.models[m].name.to_ascii_lowercase(),
                index,
                mover: filter.is_mover(o),
            })
        })
        .collect();
    let kill_volumes = w
        .objects_with_role(ObjectRole::KillVolume)
        .filter_map(|o| {
            let m = w.brush_of(o)?;
            // BOT-131: the server kills where the body's position lies inside the volume's closed model (a ray
            // crosses its polygons an odd number of times); a model that encloses no space (Two Towers'
            // "EndlessFall67": two polygons of one flat plane) kills nobody there, so it is no danger to the bot
            // either (as planes alone it was a slab across a whole floor that the grid kept out of)
            if !encloses_space(m) {
                return None;
            }
            let v = brush_volume(m)?;
            Some((o.name()?.to_ascii_lowercase(), v))
        })
        .collect();
    // BOT-143: the hazard volumes that hurt (dc-dserver `killvol::hazard_shapes` reads the same role and property)
    // SPEC: FMT_DAT85.md §6.2 (the hazard role, brush, name-linked)
    // DC-DEFAULT: BOT-143 (proposed)
    let hazard_named: Vec<(Vec<u8>, ConvexVolume)> = w
        .objects_with_role(ObjectRole::HazardVolume)
        .filter(|o| {
            matches!(o.prop_named(&dc_world::names::PROP_DAMAGE).map(|p| p.value),
                Some(dc_world::FieldValue::Real(r)) if r.is_finite() && r > 0.0)
        })
        .filter_map(|o| {
            let m = w.brush_of(o)?;
            let v = encloses_space(m).then(|| brush_volume(m)).flatten()?;
            Some((o.name().unwrap_or_default().to_ascii_lowercase(), v))
        })
        .collect();
    let hazard_volumes = hazard_named.iter().map(|h| h.1.clone()).collect();
    // BOT-177: the ones the server spares an infected player (dc-dserver `infect::infected_spared_by_hazard`: the
    // poison damage kind, below the lethal rate per second)
    let hazard_gas: Vec<Vec<u8>> = w
        .objects_with_role(ObjectRole::HazardVolume)
        .filter(|o| {
            let rate = match o.prop_named(&dc_world::names::PROP_DAMAGE).map(|p| p.value) {
                Some(dc_world::FieldValue::Real(r)) => r,
                _ => return false,
            };
            let poison = matches!(o.prop_named(&dc_world::names::PROP_DAMAGE_KIND).map(|p| p.value),
                Some(dc_world::FieldValue::Text(t)) if dc_world::names::DAMAGE_KIND_POISON.matches(t));
            poison && rate > 0.0 && rate < crate::qrt::INFECTED_LETHAL_HAZARD_RATE
        })
        .map(|o| o.name().unwrap_or_default().to_ascii_lowercase())
        .collect();
    Ok(LoadedWorld {
        listed: name.to_string(),
        size: data.len(),
        sha16: dc_rez::dc_rom::sha256_hex(data)[..16].to_string(),
        extents: ExtentBox {
            min: w.extent_min,
            max: w.extent_max,
        },
        full: collision.clone(),
        collision,
        ladders: Arc::new(ladder_volumes(&w)),
        player_spawns: w.objects_with_role(ObjectRole::PlayerSpawn).count(),
        spawn_points: w
            .objects_with_role(ObjectRole::PlayerSpawn)
            .filter_map(|o| o.position())
            .collect(),
        linked,
        solid: None,
        kill_volumes,
        hazard_volumes,
        hazard_named,
        hazard_gas,
        hazard_off: Vec::new(),
        infected: false,
        scripts: dc_world::scripts::start_scripts(&w),
    })
}

/// BOT-177: the hazard volumes of `named` not named in `off`.
pub fn acting_hazards(named: &[(Vec<u8>, ConvexVolume)], off: &[Vec<u8>]) -> Vec<ConvexVolume> {
    named
        .iter()
        .filter(|(n, _)| !off.contains(n))
        .map(|(_, v)| v.clone())
        .collect()
}

/// BOT-177: [`hazards_switched_off`] for a loaded world, remembered per ROM, world bytes and mode class for the
/// process (a swarm's bots of one room ask alike; the tables are read once).
pub fn hazards_switched_off_cached(rom: &Path, w: &LoadedWorld, game_type: u8) -> Option<Vec<Vec<u8>>> {
    type Key = (PathBuf, String, u8);
    type Cache = std::sync::Mutex<std::collections::HashMap<Key, Option<Vec<Vec<u8>>>>>;
    static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
    let key = (rom.to_path_buf(), w.sha16.clone(), game_type);
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return v;
    }
    let v = hazards_switched_off(rom, &w.listed, &w.scripts, &w.hazard_named, game_type);
    if let Ok(mut c) = cache.lock() {
        c.insert(key, v.clone());
    }
    v
}

/// BOT-177: the names of the hazard volumes the game server switches off in a room of mode class `game_type` on the
/// world `listed` (an archive entry or a file path; its file name names the world): the start scripts evaluated as
/// the server does (DSV-607, shared code `dc_world::scripts`) with each `trig` value the ROM's room tables give for
/// that world and mode (`dc_bute::room_triggers_for_world`). A volume counts as off only when every such room
/// switches it off; clan rows count too, and a (map, mode) pair with no mode file counts as a room that switches
/// nothing off (the lobby may still start it). `None` when no room of the tables plays that world in that mode (a
/// community map, a world file of a local run that is no ROM world): the bot then takes every hazard volume as
/// acting. Rooms of an override world or `-worldobjects all` run no scripts: the bot cannot know them, the safety
/// net (`BotSession::hazard_rule_wrong`) restores the volumes at the first hit.
// DC-DEFAULT: BOT-177 (proposed)
pub fn hazards_switched_off(
    rom: &Path,
    listed: &str,
    scripts: &dc_world::scripts::StartScripts,
    hazards: &[(Vec<u8>, ConvexVolume)],
    game_type: u8,
) -> Option<Vec<Vec<u8>>> {
    let file = listed.rsplit(['\\', '/']).next().unwrap_or(listed);
    let world = file
        .len()
        .checked_sub(4)
        .filter(|&cut| file.is_char_boundary(cut) && file[cut..].eq_ignore_ascii_case(".dat"))
        .map_or(file, |cut| &file[..cut]);
    let g = dc_bute::GameRoomData::from_rom(rom).ok()?;
    let trigs = dc_bute::room_triggers_for_world(rom, &g, world, game_type);
    if trigs.is_empty() {
        return None;
    }
    Some(hazards_off_for_rooms(scripts, hazards, &trigs))
}

/// BOT-177: the names of `hazards` every room of `rooms` switches off; none when a room has no mode file.
pub fn hazards_off_for_rooms(
    scripts: &dc_world::scripts::StartScripts,
    hazards: &[(Vec<u8>, ConvexVolume)],
    rooms: &[dc_bute::RoomTrig],
) -> Vec<Vec<u8>> {
    if rooms.iter().any(|t| !t.mode_file) {
        return Vec::new();
    }
    let runs: Vec<_> = rooms
        .iter()
        .map(|t| dc_world::scripts::start_scripts_run(scripts.as_ref(), t.trig.as_deref()).0)
        .collect();
    hazards
        .iter()
        .filter(|(n, _)| runs.iter().all(|ov| !dc_world::scripts::left_on(ov, n)))
        .map(|(n, _)| n.clone())
        .collect()
}

/// BOT-177: whether a poison hit with the body at `at` contradicts the judged rule (some volume of `named` is off,
/// i.e. not in `acting`): not when the body stands within [`HAZARD_RULE_SLACK`] of the box of an acting volume and of
/// none judged off (that volume explains the hit); any other hit does.
// DC-DEFAULT: BOT-177 (proposed) 48 units around the volume's box
pub fn hazard_hit_contradicts_rule(
    at: [f32; 3],
    named: &[(Vec<u8>, ConvexVolume)],
    acting: &[ConvexVolume],
) -> bool {
    const SLACK: f32 = crate::session::HAZARD_RULE_SLACK;
    let near = |v: &ConvexVolume| {
        let (lo, hi) = (v.lo.to_array(), v.hi.to_array());
        (0..3).all(|k| at[k] >= lo[k] - SLACK && at[k] <= hi[k] + SLACK)
    };
    let (mut off_any, mut near_off, mut near_on) = (false, false, false);
    for (_, v) in named {
        if acting.contains(v) {
            near_on |= near(v);
        } else {
            off_any = true;
            near_off |= near(v);
        }
    }
    off_any && (near_off || !near_on)
}

/// BOT-131: whether a volume's model encloses space as the server's point-inside test needs it: at least four
/// polygons and an extent on every axis (a closed model; one flat sheet of polygons holds no point inside).
// DC-DEFAULT: BOT-131 (proposed) 4 polygons, 0.5 units
pub fn encloses_space(m: &dc_world::BspModel<'_>) -> bool {
    m.polygons.len() >= 4 && (0..3).all(|i| m.box_max[i] - m.box_min[i] > 0.5)
}

/// Loads a world file from disk (tests, local runs against a server started with a world file, community maps
/// out of the pack cache). A map built from pieces of the ROM's worlds is composed from `rom` first, the step the
/// game server runs on the same bytes (MAP-116).
// DC-DEFAULT: MAP-116
pub fn load_world_file(path: &Path, rom: Option<&Path>) -> Result<LoadedWorld, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let data = dc_world::compose::compose_with_rom(data, rom)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    world_from_bytes(&path.display().to_string(), &data)
}

/// The file of a community map's announce (`@mod:<id>/<path>`) under a folder of unpacked packs (`<folder>/<any
/// name>/manifest.json` + its files, the lobby's `entry_mods/cache`): the pack whose manifest has that id.
/// `None`: not such an announce, no such pack or file, or a path that leaves the pack.
// DC-DEFAULT: BOT-101
pub fn mod_world_path(cache: &Path, announced: &str) -> Option<PathBuf> {
    let rest = announced.trim().strip_prefix("@mod:")?;
    let (id, file) = rest.split_once('/')?;
    if file
        .split(['/', '\\'])
        .any(|part| part.is_empty() || part == "..")
    {
        return None;
    }
    std::fs::read_dir(cache)
        .ok()?
        .flatten()
        .map(|pack| pack.path())
        .filter(|pack| {
            std::fs::read(pack.join("manifest.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .is_some_and(|m| m["id"] == id)
        })
        // an updated pack leaves its older copy in the cache: the newest of an id is the one in use
        .max_by_key(|pack| {
            std::fs::metadata(pack.join("manifest.json"))
                .and_then(|m| m.modified())
                .ok()
        })
        .map(|pack| pack.join(file))
        .filter(|path| path.is_file())
}

/// The user's ROM worlds.
pub struct RomWorlds {
    rom: PathBuf,
    arch: RezArchive,
}

/// The client's name normalisation for an archive lookup: leading separator skipped, either separator,
/// case-insensitive. Returns `None` for a name without an extension (it cannot name a world file).
// SPEC: UDP_SESSION_2009_A_addendum_disasm.md A9 (engine+0x1c0170: leading separator skipped, `_splitpath`,
// extension packed into the type, no extension -> type 0)
pub fn normalise(name: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(name).replace('/', "\\");
    let s = s.trim_start_matches('\\').to_ascii_uppercase();
    let file = s.rsplit('\\').next().unwrap_or(&s);
    let (base, ext) = file.rsplit_once('.')?;
    if base.is_empty() || ext.is_empty() || ext.len() > 4 {
        return None;
    }
    Some(s)
}

impl RomWorlds {
    /// Opens the world archive of `rom`.
    pub fn open(rom: &Path) -> Result<RomWorlds, String> {
        let arch = RezArchive::open(&rom.join("Game").join(WORLD_ARCHIVE))
            .map_err(|e| format!("world archive: {e}"))?;
        Ok(RomWorlds {
            rom: rom.to_path_buf(),
            arch,
        })
    }

    /// The ROM named by `CA_ROM`, if it is set and its world archive opens.
    pub fn from_env() -> Option<RomWorlds> {
        let rom = dc_rez::dc_rom::resolve_rom().ok()?;
        RomWorlds::open(&rom).ok()
    }

    pub fn rom(&self) -> &Path {
        &self.rom
    }

    /// The archive entry an announced name resolves to (`None` -> the 2009 client could not open the world).
    pub fn resolve(&self, announced: &[u8]) -> Option<String> {
        let want = normalise(announced)?;
        let want_tail = want.clone();
        self.arch
            .entries()
            .iter()
            .map(|e| e.0.as_str())
            .find(|n| {
                let n = n.replace('/', "\\");
                let n = n.trim_start_matches('\\').to_ascii_uppercase();
                // the archive root may carry the archive's own directory; accept the name as a path suffix
                // at a separator boundary
                n == want_tail || n.ends_with(&format!("\\{want_tail}"))
            })
            .map(str::to_string)
    }

    /// Loads a resolved entry: world box, collision, spawn count.
    // SPEC: FMT_DAT85.md §3 (version 85), §4 (extents); world entries stored plain or with the ROM key
    pub fn load(&self, listed: &str) -> Result<LoadedWorld, String> {
        let raw = self
            .arch
            .entry_raw(listed)
            .map_err(|e| format!("world read: {e}"))?
            .ok_or_else(|| format!("world {listed:?} vanished"))?;
        let data = if raw.starts_with(&WORLD_VERSION.to_le_bytes()) {
            raw
        } else {
            let key = dc_rez::dc_rom::crypt_key(&self.rom).map_err(|e| format!("key: {e}"))?;
            dc_rez::decrypt_with_key(&raw, &key).map_err(|e| format!("decrypt: {e}"))?
        };
        world_from_bytes(listed, &data)
    }

    /// The u16 the 2009 client puts into its integrity answer (read from the user's CShell.dll at run time; `None`
    /// on a server ROM, which keeps no section bytes of the library: the answer then carries 0, BOT-82).
    pub fn integrity_version(&self) -> Option<u16> {
        let pe =
            dc_rez::dc_rom::PeFile::from_file(&self.rom.join("Game").join("CShell.dll")).ok()?;
        let b = pe.rva(INTEGRITY_VERSION_RVA, 2).ok()?;
        Some(u16::from_le_bytes([b[0], b[1]]))
    }
}

#[cfg(test)]
mod mod_world_tests {
    use super::mod_world_path;

    #[test]
    fn a_community_announce_finds_its_pack_by_the_manifest_id() {
        let dir = std::env::temp_dir().join(format!("dc_bot_mod_world_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (folder, id) in [("aaa", "other"), ("bbb", "my_map")] {
            std::fs::create_dir_all(dir.join(folder).join("assets/maps")).unwrap();
            std::fs::write(
                dir.join(folder).join("manifest.json"),
                format!("{{\"id\": \"{id}\"}}"),
            )
            .unwrap();
            std::fs::write(dir.join(folder).join("assets/maps/w.dat"), id).unwrap();
        }
        let found = mod_world_path(&dir, "@mod:my_map/assets/maps/w.dat").unwrap();
        assert_eq!(std::fs::read_to_string(found).unwrap(), "my_map");
        for refused in [
            "@mod:my_map/assets/maps/none.dat",
            "@mod:nobody/assets/maps/w.dat",
            "@mod:my_map/../aaa/assets/maps/w.dat",
            "@mod:my_map",
            "Worlds\\Rom.dat",
        ] {
            assert_eq!(mod_world_path(&dir, refused), None, "{refused}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_rule() {
        assert_eq!(
            normalise(b"Worlds\\Snow.dat").as_deref(),
            Some("WORLDS\\SNOW.DAT")
        );
        assert_eq!(
            normalise(b"/worlds/snow.DAT").as_deref(),
            Some("WORLDS\\SNOW.DAT")
        );
        assert_eq!(normalise(b"Worlds\\Snow"), None);
        assert_eq!(normalise(b""), None);
    }

    /// BOT-177: the hazard volumes judged off leave the acting set; none off restores every one.
    #[test]
    fn hazards_for_room_keeps_the_ones_left_on() {
        let v = dc_sim::V3::from_array;
        let a = ConvexVolume::from_box(v([0.0; 3]), v([10.0; 3]));
        let b = ConvexVolume::from_box(v([20.0; 3]), v([30.0; 3]));
        let named = vec![(b"gas".to_vec(), a.clone()), (b"water".to_vec(), b.clone())];
        assert_eq!(acting_hazards(&named, &[b"gas".to_vec()]), vec![b.clone()]);
        assert_eq!(acting_hazards(&named, &[]), vec![a, b]);
        assert!(acting_hazards(&named, &[b"gas".to_vec(), b"water".to_vec()]).is_empty());
    }

    /// BOT-177: a poison hit contradicts the rule when some volume is judged off and no acting volume explains it.
    #[test]
    fn a_hazard_hit_restores_unless_an_acting_volume_explains_it() {
        let v = dc_sim::V3::from_array;
        let off = ConvexVolume::from_box(v([0.0; 3]), v([10.0; 3]));
        let on = ConvexVolume::from_box(v([500.0; 3]), v([510.0; 3]));
        let named = vec![(b"gas".to_vec(), off.clone()), (b"water".to_vec(), on.clone())];
        let acting = [on.clone()];
        // near the volume judged off
        assert!(hazard_hit_contradicts_rule([5.0; 3], &named, &acting));
        // far from every volume (a server that runs no scripts hurts anywhere)
        assert!(hazard_hit_contradicts_rule([250.0; 3], &named, &acting));
        // inside the acting volume: it explains the hit
        assert!(!hazard_hit_contradicts_rule([505.0; 3], &named, &acting));
        // nothing judged off: nothing to restore
        assert!(!hazard_hit_contradicts_rule([250.0; 3], &named, &[off, on]));
    }

    /// BOT-177: a volume is off only when every room (plain and clan rows alike) switches it off, and a room with
    /// no mode file switches nothing off.
    #[test]
    fn hazards_off_needs_every_room_and_a_mode_file() {
        let v = dc_sim::V3::from_array;
        let a = ConvexVolume::from_box(v([0.0; 3]), v([10.0; 3]));
        let hz = vec![(b"gas".to_vec(), a)];
        let scripts = dc_world::scripts::StartScripts {
            startup: Vec::new(),
            triggers: vec![(b"Quiet".to_vec(), vec![b"msg gas off".to_vec()])],
            controls: Vec::new(),
        };
        let room = |t: Option<&str>, mode_file| dc_bute::RoomTrig {
            trig: t.map(str::to_string),
            mode_file,
        };
        let off = vec![b"gas".to_vec()];
        assert_eq!(hazards_off_for_rooms(&scripts, &hz, &[room(Some("|Quiet||"), true)]), off);
        // a clan row whose trigger leaves it on keeps it acting
        let rooms = [room(Some("|Quiet||"), true), room(Some("|Loud||"), true)];
        assert!(hazards_off_for_rooms(&scripts, &hz, &rooms).is_empty());
        let rooms = [room(Some("|Quiet||"), true), room(None, true)];
        assert!(hazards_off_for_rooms(&scripts, &hz, &rooms).is_empty());
        // a mode-file-less row acts
        let rooms = [room(Some("|Quiet||"), true), room(None, false)];
        assert!(hazards_off_for_rooms(&scripts, &hz, &rooms).is_empty());
    }

    /// BOT-177 (ROM-gated, skipped without CA_ROM): on Warhead the start triggers of the TDM rooms switch every
    /// hazard volume off (the server logs "0 hazard volumes act"), the Quarantine room's leave them on; a world no
    /// room of the tables plays is unknown (every volume acts).
    #[test]
    fn rom_warhead_hazards_are_off_in_tdm_and_act_in_quarantine() {
        let Some(rom) = dc_rez::dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let r = RomWorlds::open(&rom).expect("world archive");
        let listed = r.resolve(b"WORLDS/WARHEAD.DAT").expect("the world");
        let w = r.load(&listed).expect("loads");
        assert!(!w.hazard_named.is_empty(), "Warhead has hazard volumes");
        let g = dc_bute::GameRoomData::from_rom(&rom).expect("tables");
        let class = |key: &str| -> u8 {
            g.modes(false)
                .iter()
                .find(|m| m.get("Key").is_some_and(|k| k == key))
                .and_then(|m| m.get("GameType")?.trim().parse().ok())
                .expect("mode row")
        };
        let t0 = std::time::Instant::now();
        let tdm = hazards_switched_off(&rom, &listed, &w.scripts, &w.hazard_named, class("TDM"))
            .expect("Warhead is a TDM map");
        eprintln!("TDM off {} of {} in {:?}", tdm.len(), w.hazard_named.len(), t0.elapsed());
        assert_eq!(tdm.len(), w.hazard_named.len(), "TDM: all off");
        let qrt = hazards_switched_off(&rom, &listed, &w.scripts, &w.hazard_named, crate::qrt::QRT_CLASS)
            .expect("Warhead is a Quarantine map");
        assert!(qrt.is_empty(), "Quarantine: all act");
        let mut w = w;
        assert!(w.hazards_for_room(&tdm));
        assert!(w.hazard_volumes.is_empty());
        assert!(w.hazards_for_room(&qrt));
        assert_eq!(w.hazard_volumes.len(), w.hazard_named.len());
        assert_eq!(
            hazards_switched_off(&rom, "NOT_A_ROM_WORLD.DAT", &w.scripts, &w.hazard_named, class("TDM")),
            None
        );
        // Overdose's TDM rooms leave its hazard volumes acting (the server logs "2 hazard volumes act")
        let listed = r.resolve(b"WORLDS/OVERDOSE.DAT").expect("the world");
        let w = r.load(&listed).expect("loads");
        assert!(!w.hazard_named.is_empty(), "Overdose has hazard volumes");
        let tdm = hazards_switched_off(&rom, &listed, &w.scripts, &w.hazard_named, class("TDM"))
            .expect("Overdose is a TDM map");
        assert!(tdm.is_empty(), "Overdose TDM: all act, {tdm:?}");
    }

    #[test]
    fn rom_world_names_resolve() {
        let Some(rom) = dc_rez::dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let w = RomWorlds::open(&rom).expect("world archive");
        let listed: Vec<String> = w
            .arch
            .entries()
            .iter()
            .filter(|e| e.0.to_ascii_uppercase().ends_with(".DAT"))
            .map(|e| e.0.clone())
            .collect();
        assert!(!listed.is_empty());
        let first = &listed[0];
        assert_eq!(w.resolve(first.as_bytes()).as_deref(), Some(first.as_str()));
        assert_eq!(
            w.resolve(first.to_ascii_lowercase().replace('\\', "/").as_bytes())
                .as_deref(),
            Some(first.as_str())
        );
        let no_ext = first.rsplit_once('.').unwrap().0;
        assert_eq!(w.resolve(no_ext.as_bytes()), None);
        let lw = w.load(first).expect("load");
        assert!(lw.extents.min[0] < lw.extents.max[0]);
        // a server ROM holds no shell library section bytes: no integrity version there (BOT-82)
        assert_eq!(
            w.integrity_version().is_some(),
            !dc_rez::dc_rom::server_rom::is_server_rom(&w.rom)
        );
    }
}
