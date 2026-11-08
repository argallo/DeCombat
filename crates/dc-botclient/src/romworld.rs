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
    /// off (DSV-607), so the bot takes every one as acting.
    pub hazard_volumes: Vec<ConvexVolume>,
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
    let hazard_volumes = w
        .objects_with_role(ObjectRole::HazardVolume)
        .filter(|o| {
            matches!(o.prop_named(&dc_world::names::PROP_DAMAGE).map(|p| p.value),
                Some(dc_world::FieldValue::Real(r)) if r.is_finite() && r > 0.0)
        })
        .filter_map(|o| {
            let m = w.brush_of(o)?;
            encloses_space(m).then(|| brush_volume(m)).flatten()
        })
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
    })
}

/// BOT-131: whether a volume's model encloses space as the server's point-inside test needs it: at least four
/// polygons and an extent on every axis (a closed model; one flat sheet of polygons holds no point inside).
// DC-DEFAULT: BOT-131 (proposed) 4 polygons, 0.5 units
pub fn encloses_space(m: &dc_world::BspModel<'_>) -> bool {
    m.polygons.len() >= 4 && (0..3).all(|i| m.box_max[i] - m.box_min[i] > 0.5)
}

/// Loads a world file from disk (tests and local runs against a server started with a world file).
pub fn load_world_file(path: &Path) -> Result<LoadedWorld, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
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
