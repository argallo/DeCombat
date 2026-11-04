//! `dc-mapkit validate` / `info`: project-level checks, fast and offline, no `.dat` written (MAPKIT_PLAN.md
//! 5.2, 5.3 step 6; DC-DEFAULT: MAP-13). `build` re-runs the same `Diagnostic` type plus the world-level sweeps.
//!
//! Checks: format and key; the paths rule and case-only duplicates under `assets/`; unique, well-formed ids and
//! `next_id`; material references; own texture files (PNG, readable) and ROM paths (warned `rom.unverified`);
//! object classes and property kinds; spawn counts per team / team-less for the declared modes; bomb-site
//! numbering; flag zones and flags per team; the pos48 extent rule (span per axis: error above 65,536, warning
//! above 32,768) and objects outside the geometry box; the package size estimate (own textures at w * h * 4,
//! world data at a rough per-polygon cost) against the 16 MiB lobby default and the 32 MiB client cap; terrain
//! sizes and solid shapes; mod metadata rules.

use crate::build::{level_of, Assets};
use crate::diag::{Code, Diagnostic, Diagnostics, Severity};
use crate::geom::{self, V3};
use crate::project::{
    check_id, check_key, check_rel_path, id_suffix, Heights, Project, PropValue, TextureRef,
};
use crate::vocab_names as v;
use crate::worldgen;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// pos48 (MAPKIT_PLAN.md 5.1): span per axis the wire can carry, and the span above which precision drops
/// below 0.125 units.
pub const EXTENT_LIMIT: f32 = 65_536.0;
pub const EXTENT_WARN: f32 = 32_768.0;
/// Package caps (HOSTING_AND_MODS.md, dc-config `mods.max_zip_bytes` default and the client's hard cap).
pub const SIZE_WARN: u64 = 16 * 1024 * 1024;
pub const SIZE_LIMIT: u64 = 32 * 1024 * 1024;
/// Bytes per world polygon in the `.dat`: three 68-byte render vertices per polygon on average plus points,
/// planes, polygon records and BSP nodes (DC-DEFAULT: MAP-13).
// OBSERVED: the templates build at 269 (flat), 272 (arena) and 289 (starter) bytes per polygon
pub const BYTES_PER_POLY: u64 = 300;
/// The radar picture `export` always writes: a 256 x 256 32-bit DTX plus its header (MAP-52).
pub const RADAR_BYTES: u64 = 256 * 256 * 4 + dc_tex::HEADER_LEN as u64;
/// Allowance for the 640 x 480 PNG screenshot `export` always writes (36 .. 118 KiB on the templates).
pub const SCREENSHOT_BYTES: u64 = 128 * 1024;

/// What `info` reports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Info {
    pub name: String,
    pub key: String,
    pub format: u32,
    pub tool_version: String,
    pub seed: u64,
    pub terrain: Option<TerrainInfo>,
    pub counts: Counts,
    pub objects_by_class: BTreeMap<String, usize>,
    pub spawns: Spawns,
    pub modes: Vec<String>,
    pub extent: Option<Extent>,
    pub size_estimate_bytes: u64,
    pub own_textures: usize,
    pub rom_textures: usize,
    pub errors: usize,
    pub warnings: usize,
    pub rom: Option<RomInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainInfo {
    pub cells_x: u32,
    pub cells_z: u32,
    pub cell_size: f32,
    pub min_y: f32,
    pub max_y: f32,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Counts {
    pub objects: usize,
    pub solids: usize,
    pub brushes: usize,
    pub prefabs: usize,
    pub imports: usize,
    pub materials: usize,
    pub polys: usize,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Spawns {
    pub team1: usize,
    pub team2: usize,
    pub any: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Extent {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub span: [f32; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RomInfo {
    pub path: String,
    pub source: crate::romlist::RomSource,
}

fn team_of(o: &crate::project::MapObject) -> &str {
    o.text(v::PROP_TEAM).unwrap_or("")
}

fn plays(p: &Project, mode: &str) -> bool {
    p.mod_meta
        .modes
        .iter()
        .any(|m| m.eq_ignore_ascii_case(mode) || m.eq_ignore_ascii_case(&format!("{mode}_CLAN")))
}

/// An axis-aligned box (min, max), or `None` when nothing spans it.
type Box3 = Option<(V3, V3)>;

/// The geometry box, the box plus every object position, and the polygon count.
fn bounds(p: &Project, h: &Heights, dir: &Path) -> (Box3, Box3, usize) {
    let mut geo: Box3 = None;
    let mut polys = 0;
    if let Ok(level) = level_of(p, h, &Assets::new(dir)) {
        polys = level.polys.len()
            + level.sky_faces.len()
            + level.brushes.iter().map(|b| b.polys.len()).sum::<usize>();
        if polys > 0 {
            geo = Some((level.extent_min, level.extent_max));
        }
    } else if let Some(t) = &p.terrain {
        let (x0, x1) = t.span_x();
        let (z0, z1) = t.span_z();
        polys = (t.cells_x * t.cells_z * 2) as usize;
        geo = Some((
            V3::new(x0, t.origin[1], z0),
            V3::new(x1, t.origin[1] + t.height_scale, z1),
        ));
    }
    let mut all = geo;
    for o in &p.objects {
        let q = V3::from_array(o.pos);
        all = Some(match all {
            None => (q, q),
            Some((lo, hi)) => (lo.min(q), hi.max(q)),
        });
    }
    (geo, all, polys)
}

/// The size estimate: own textures at w * h * 4 (the DTX writer is uncompressed 32-bit), world data per polygon,
/// objects, a fixed overhead, the radar DTX and the screenshot `export` always writes. Diagnostics name
/// missing or unreadable textures.
pub fn size_estimate(dir: &Path, p: &Project, polys: usize) -> (u64, usize, usize, Diagnostics) {
    let mut d = Diagnostics::new();
    let mut bytes: u64 = 4096
        + polys as u64 * BYTES_PER_POLY
        + p.objects.len() as u64 * 128
        + RADAR_BYTES
        + SCREENSHOT_BYTES;
    let mut own = 0;
    let mut rom = 0;
    for (i, m) in p.materials.iter().enumerate() {
        match &m.texture {
            TextureRef::Own(rel) => {
                own += 1;
                if check_rel_path(rel).is_err() {
                    continue; // reported by the path check
                }
                let path = Project::resolve(dir, rel);
                if !path.is_file() {
                    d.push(
                        Diagnostic::new(
                            Code::TextureMissing,
                            format!("materials[{i}].texture"),
                            format!("{rel} is not a file"),
                        )
                        .with_fix("put a PNG there or point the material at an existing one"),
                    );
                    continue;
                }
                match image::image_dimensions(&path) {
                    Ok((w, h)) => bytes += w as u64 * h as u64 * 4,
                    Err(e) => d.error(
                        Code::TextureFormat,
                        format!("materials[{i}].texture"),
                        format!("{rel}: {e}"),
                    ),
                }
            }
            TextureRef::Rom(rel) => {
                rom += 1;
                d.warn(
                    Code::RomUnverified,
                    format!("materials[{i}].texture"),
                    format!("ROM texture `{rel}` is checked at build time against the user's ROM"),
                );
            }
        }
    }
    (bytes, own, rom, d)
}

/// The ids of the items that repeat an earlier item, by the key `same` gives each one.
fn repeats<'a, T>(
    items: &'a [T],
    id: impl Fn(&'a T) -> &'a str,
    same: impl Fn(&T) -> String,
) -> Vec<&'a str> {
    let mut seen = std::collections::HashSet::new();
    items
        .iter()
        .filter(|x| !seen.insert(same(x)))
        .map(id)
        .collect()
}

/// Exact repeats (DC-DEFAULT: MAP-94): nothing else notices a generator that ran twice, and the doubled faces
/// flicker in the game. One diagnostic per kind, however many repeat.
fn check_duplicates(p: &Project, d: &mut Diagnostics) {
    const FIX: &str = "start the generator with `map.clear()` (or `map.clear{group = \"name\"}`), \
                       or `dc-mapkit revert` to the snapshot before the second run";
    let solids = repeats(
        &p.solids,
        |s| s.id.as_str(),
        |s| format!("{:?}|{:?}|{:?}|{:?}", s.kind, s.pos, s.size, s.rot_y_deg),
    );
    if let Some(first) = solids.first() {
        d.push(
            Diagnostic::new(
                Code::SolidDuplicate,
                "solids",
                format!(
                    "{} solids repeat another solid exactly (the first is `{first}`)",
                    solids.len()
                ),
            )
            .with_fix(FIX),
        );
    }
    let objects = repeats(
        &p.objects,
        |o| o.id.as_str(),
        |o| format!("{}|{:?}|{:?}", o.class, o.pos, o.props),
    );
    if let Some(first) = objects.first() {
        d.push(
            Diagnostic::new(
                Code::ObjectDuplicate,
                "objects",
                format!(
                    "{} objects repeat another object exactly (the first is `{first}`)",
                    objects.len()
                ),
            )
            .with_fix(FIX),
        );
    }
}

/// Every project-level check.
pub fn validate(dir: &Path, p: &Project, h: &Heights) -> Diagnostics {
    let mut d = Diagnostics::new();
    check_duplicates(p, &mut d);
    if p.format != crate::project::FORMAT {
        d.error(
            Code::ProjectFormat,
            "format",
            format!("format {} (this tool reads 1)", p.format),
        );
    }
    if !check_key(&p.key) {
        d.push(
            Diagnostic::new(
                Code::ProjectKey,
                "key",
                format!("`{}` is not 1..32 of a-z 0-9 _", p.key),
            )
            .with_fix(format!("use `{}`", crate::project::key_from_name(&p.key))),
        );
    }
    if p.name.trim().is_empty() {
        d.error(Code::ProjectName, "name", "the map name is empty");
    }

    // paths
    let mut paths: Vec<(String, String)> = Vec::new();
    if let Some(t) = &p.terrain {
        paths.push(("terrain.file".into(), t.file.clone()));
        if let Some(paint) = &t.paint {
            paths.push(("terrain.paint".into(), paint.clone()));
        }
        if t.cells_x == 0
            || t.cells_z == 0
            || t.cell_size <= 0.0
            || t.height_scale <= 0.0
            || !t.cell_size.is_finite()
        {
            d.error(
                Code::TerrainSettings,
                "terrain",
                "cells, cell_size and height_scale must be positive",
            );
        } else if h.cols != t.cols() || h.rows != t.rows() {
            d.error(
                Code::TerrainSize,
                "terrain",
                format!(
                    "heights are {} x {}, the terrain wants {} x {}",
                    h.cols,
                    h.rows,
                    t.cols(),
                    t.rows()
                ),
            );
        }
        for (i, l) in t.layers.iter().enumerate() {
            if p.material(&l.material).is_none() {
                d.error(
                    Code::MaterialMissing,
                    format!("terrain.layers[{i}]"),
                    format!("material `{}` is not defined", l.material),
                );
            }
        }
        if t.layers.is_empty() {
            d.error(
                Code::MaterialMissing,
                "terrain.layers",
                "the terrain needs a layer 0 material",
            );
        }
    }
    for (i, m) in p.materials.iter().enumerate() {
        if let TextureRef::Own(rel) = &m.texture {
            paths.push((format!("materials[{i}].texture"), rel.clone()));
        }
    }
    for (i, imp) in p.imports.iter().enumerate() {
        paths.push((format!("imports[{i}].file"), imp.file.clone()));
    }
    let mut lower: BTreeMap<String, String> = BTreeMap::new();
    for (where_, rel) in &paths {
        if let Err(e) = check_rel_path(rel) {
            d.push(
                Diagnostic::new(Code::PathInvalid, where_.clone(), e)
                    .with_fix("forward slashes, relative to the project, no `..`"),
            );
            continue;
        }
        if rel.starts_with("assets/") {
            let key = rel.to_ascii_lowercase();
            if let Some(other) = lower.get(&key) {
                if other != rel {
                    d.warn(
                        Code::PathCase,
                        where_.clone(),
                        format!("`{rel}` and `{other}` differ only by case"),
                    );
                }
            } else {
                lower.insert(key, rel.clone());
            }
        }
    }

    // ids
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut max_suffix = 0u32;
    for id in p.all_ids() {
        if !check_id(id) {
            d.error(Code::IdFormat, id, format!("`{id}` is not <prefix>_<n>"));
        }
        if !seen.insert(id) {
            d.error(Code::IdDuplicate, id, format!("`{id}` is used twice"));
        }
        if let Some(n) = id_suffix(id) {
            max_suffix = max_suffix.max(n);
        }
    }
    if p.next_id <= max_suffix {
        d.push(
            Diagnostic::new(
                Code::IdNext,
                "next_id",
                format!(
                    "next_id {} is not above the highest id {max_suffix}",
                    p.next_id
                ),
            )
            .with_fix(format!("set next_id to {}", max_suffix + 1)),
        );
    }
    let mut mat_ids: BTreeSet<&str> = BTreeSet::new();
    for (i, m) in p.materials.iter().enumerate() {
        if !mat_ids.insert(&m.id) {
            d.error(
                Code::IdDuplicate,
                format!("materials[{i}].id"),
                format!("material `{}` is defined twice", m.id),
            );
        }
    }

    // materials and solids
    let mut used: BTreeSet<&str> = BTreeSet::new();
    if let Some(t) = &p.terrain {
        used.extend(t.layers.iter().map(|l| l.material.as_str()));
    }
    used.insert(p.settings.sky_material.as_str());
    if p.material(&p.settings.sky_material).is_none() {
        d.error(
            Code::MaterialMissing,
            "settings.sky_material",
            format!("material `{}` is not defined", p.settings.sky_material),
        );
    }
    for (i, s) in p.solids.iter().enumerate() {
        used.insert(&s.material);
        used.extend(s.faces.values().map(String::as_str));
        if p.material(&s.material).is_none() {
            d.error(
                Code::MaterialMissing,
                format!("solids[{i}].material"),
                format!("material `{}` is not defined", s.material),
            );
        }
        for (f, m) in &s.faces {
            if p.material(m).is_none() {
                d.error(
                    Code::MaterialMissing,
                    format!("solids[{i}].faces.{f}"),
                    format!("material `{m}` is not defined"),
                );
            }
        }
        if s.size.iter().any(|c| !(*c > 0.0 && c.is_finite())) {
            d.error(
                Code::SolidSize,
                format!("solids[{i}].size"),
                format!("{:?}", s.size),
            );
        } else if let Err(e) = crate::project::check_solid_corners(s) {
            d.push(
                Diagnostic::new(Code::SolidShape, format!("solids[{i}].kind"), e)
                    .with_fix("use at most 255 cylinder sides / prism points"),
            );
        } else if let Err(e) = geom::solid_faces(s) {
            let code = if matches!(s.kind, crate::project::SolidKind::Cylinder { .. }) {
                Code::SolidCircle
            } else {
                Code::SolidShape
            };
            d.error(code, format!("solids[{i}].kind"), e);
        }
    }
    for (i, imp) in p.imports.iter().enumerate() {
        used.insert(&imp.material);
        if p.material(&imp.material).is_none() {
            d.error(
                Code::MaterialMissing,
                format!("imports[{i}].material"),
                format!("material `{}` is not defined", imp.material),
            );
        }
    }
    for (i, m) in p.materials.iter().enumerate() {
        if !used.contains(m.id.as_str()) {
            d.info(
                Code::MaterialUnused,
                format!("materials[{i}]"),
                format!("material `{}` is unused", m.id),
            );
        }
    }

    // objects
    let mut spawns = Spawns::default();
    let mut sites: Vec<i64> = Vec::new();
    let mut zones = [0usize; 2];
    let mut flags = [0usize; 2];
    for (i, o) in p.objects.iter().enumerate() {
        let path = format!("objects[{i}]");
        if !v::is_point_class(&o.class) {
            d.push(
                Diagnostic::new(
                    Code::ObjectClass,
                    format!("{path}.class"),
                    format!("`{}` is not a point class", o.class),
                )
                .with_fix("brush classes come from solids with a `brush` role"),
            );
            continue;
        }
        if !o.pos.iter().all(|c| c.is_finite()) {
            d.error(Code::ObjectProp, format!("{path}.pos"), "not finite");
        }
        let team = team_of(o);
        let team_ok = matches!(team, "" | "1" | "2");
        match o.class.as_str() {
            v::CLASS_PLAYER_SPAWN => {
                if !team_ok {
                    d.error(
                        Code::SpawnTeam,
                        format!("{path}.props.team"),
                        format!("`{team}`"),
                    );
                }
                match team {
                    "1" => spawns.team1 += 1,
                    "2" => spawns.team2 += 1,
                    _ => spawns.any += 1,
                }
            }
            v::CLASS_BOMB_SITE => {
                match o.props.get(v::PROP_NUMBER) {
                    Some(PropValue::Num(n)) => sites.push(*n as i64),
                    _ => d.error(
                        Code::BombNumber,
                        format!("{path}.props.number"),
                        "a bomb site needs a numeric `number` (0 or 1)",
                    ),
                }
                if o.props
                    .get(v::PROP_EXTENT)
                    .and_then(PropValue::as_vec3)
                    .is_none()
                {
                    d.error(
                        Code::ObjectProp,
                        format!("{path}.props.extent"),
                        "a bomb site needs a vec3 `extent` (half extents)",
                    );
                }
            }
            v::CLASS_FLAG_ZONE | v::CLASS_CAPTURE_FLAG => {
                let slot = match team {
                    "1" => Some(0),
                    "2" => Some(1),
                    _ => None,
                };
                match slot {
                    Some(s) => {
                        if o.class == v::CLASS_FLAG_ZONE {
                            zones[s] += 1
                        } else {
                            flags[s] += 1
                        }
                    }
                    None => d.error(
                        Code::SpawnTeam,
                        format!("{path}.props.team"),
                        format!("`{team}` (a zone or flag needs \"1\" or \"2\")"),
                    ),
                }
                if o.class == v::CLASS_FLAG_ZONE
                    && o.props
                        .get(v::PROP_EXTENT)
                        .and_then(PropValue::as_vec3)
                        .is_none()
                {
                    d.error(
                        Code::ObjectProp,
                        format!("{path}.props.extent"),
                        "a flag zone needs a vec3 `extent`",
                    );
                }
            }
            v::CLASS_POINT_LIGHT if !o.num(v::PROP_RADIUS).is_some_and(|r| r > 0.0) => {
                d.error(
                    Code::ObjectProp,
                    format!("{path}.props.radius"),
                    "a point light needs a radius > 0",
                );
            }
            _ => {}
        }
        if let Some(m) = o.text(v::PROP_MODES) {
            for name in m.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                if !v::MODE_NAMES.iter().any(|k| k.eq_ignore_ascii_case(name)) {
                    d.warn(
                        Code::ModeUnknown,
                        format!("{path}.props.modes"),
                        format!("`{name}`"),
                    );
                }
            }
        }
    }
    let team_modes = ["TDM", "ELIM", "BOMB", "FLAG"].iter().any(|m| plays(p, m));
    if team_modes {
        for (team, n) in [("1", spawns.team1), ("2", spawns.team2)] {
            if n < 8 {
                d.push(
                    Diagnostic::new(
                        Code::SpawnCount,
                        "objects",
                        format!("team {team} has {n} spawns; a full room needs 8"),
                    )
                    .with_fix(format!(
                        "add dc.player_spawn objects with props.team \"{team}\""
                    )),
                );
            }
        }
    }
    if plays(p, "FFA") && spawns.any < 16 {
        d.push(
            Diagnostic::new(
                Code::SpawnCount,
                "objects",
                format!(
                    "{} team-less spawns; FFA with 16 players needs 16",
                    spawns.any
                ),
            )
            .with_fix("add dc.player_spawn objects with props.team \"\""),
        );
    }
    if plays(p, "BOMB") {
        if sites.len() != 2 {
            d.error(
                Code::BombCount,
                "objects",
                format!("{} bomb sites; BOMB needs 2", sites.len()),
            );
        }
        let mut sorted = sites.clone();
        sorted.sort_unstable();
        if sites.len() == 2 && sorted != [0, 1] {
            d.push(
                Diagnostic::new(
                    Code::BombNumber,
                    "objects",
                    format!("bomb site numbers {sorted:?}; want 0 and 1"),
                )
                .with_fix("set props.number 0 on site A and 1 on site B"),
            );
        }
    }
    if plays(p, "FLAG") {
        for (i, team) in ["1", "2"].iter().enumerate() {
            if zones[i] != 1 || flags[i] != 1 {
                d.error(
                    Code::FlagCount,
                    "objects",
                    format!(
                        "team {team}: {} flag zones, {} flags; FLAG needs 1 each",
                        zones[i], flags[i]
                    ),
                );
            }
        }
    }
    for (i, m) in p.mod_meta.modes.iter().enumerate() {
        if !v::MODE_NAMES.iter().any(|k| k.eq_ignore_ascii_case(m)) {
            d.warn(
                Code::ModeUnknown,
                format!("mod_meta.modes[{i}]"),
                format!("`{m}`"),
            );
        }
    }

    // extents
    let (geo, all, polys) = bounds(p, h, dir);
    if let Some((lo, hi)) = all {
        let span = hi - lo;
        for (axis, s) in ["x", "y", "z"].iter().zip([span.x, span.y, span.z]) {
            if s > EXTENT_LIMIT {
                d.error(
                    Code::ExtentLimit,
                    "extent",
                    format!("{s} units on {axis} (limit {EXTENT_LIMIT})"),
                );
            } else if s > EXTENT_WARN {
                d.warn(
                    Code::ExtentLarge,
                    "extent",
                    format!("{s} units on {axis} (precision drops above {EXTENT_WARN})"),
                );
            }
        }
    }
    if let Some((lo, hi)) = geo {
        for (i, o) in p.objects.iter().enumerate() {
            let q = V3::from_array(o.pos);
            if q.x < lo.x || q.y < lo.y || q.z < lo.z || q.x > hi.x || q.y > hi.y || q.z > hi.z {
                d.warn(
                    Code::ExtentEnclose,
                    format!("objects[{i}].pos"),
                    format!("{} lies outside the geometry box", o.id),
                );
            }
        }
    }

    // size
    let (bytes, _, _, sd) = size_estimate(dir, p, polys);
    d.extend(sd);
    if bytes > SIZE_LIMIT {
        d.error(
            Code::SizeLimit,
            "size",
            format!("estimated {} MiB (cap 32 MiB)", bytes >> 20),
        );
    } else if bytes > SIZE_WARN {
        d.push(
            Diagnostic::new(Code::SizeEstimate, "size", format!("estimated {} MiB (lobby default 16 MiB)", bytes >> 20))
                .with_fix("shrink own textures (512 x 512 is 1 MiB each) or raise the lobby's mods.max_zip_bytes"),
        );
    }

    // metadata
    let m = &p.mod_meta;
    let dl = m.description.chars().count();
    if !(20..=500).contains(&dl) {
        d.error(
            Code::MetaInvalid,
            "mod_meta.description",
            format!("{dl} characters; 20..500"),
        );
    }
    let al = m.author.chars().count();
    if !(1..=100).contains(&al) {
        d.error(
            Code::MetaInvalid,
            "mod_meta.author",
            format!("{al} characters; 1..100"),
        );
    }
    if m.id.is_empty()
        || !m
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        d.error(
            Code::MetaInvalid,
            "mod_meta.id",
            format!("`{}`: letters, digits, _ and - only", m.id),
        );
    }
    if m.tags.len() > 8
        || m.tags.iter().any(|t| {
            t.is_empty()
                || t.len() > 20
                || !t
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
    {
        d.error(
            Code::MetaInvalid,
            "mod_meta.tags",
            "up to 8 tags of 1..20 characters a-z 0-9 -",
        );
    }
    d
}

/// The `info` report (runs `validate` for the counts).
pub fn info(
    dir: &Path,
    p: &Project,
    h: &Heights,
    rom: Option<(&Path, crate::romlist::RomSource)>,
) -> Info {
    let d = validate(dir, p, h);
    let (_, all, polys) = bounds(p, h, dir);
    let (bytes, own, rom_n, _) = size_estimate(dir, p, polys);
    let mut by_class: BTreeMap<String, usize> = BTreeMap::new();
    let mut spawns = Spawns::default();
    for o in &p.objects {
        *by_class.entry(o.class.clone()).or_insert(0) += 1;
        if o.class == v::CLASS_PLAYER_SPAWN {
            match team_of(o) {
                "1" => spawns.team1 += 1,
                "2" => spawns.team2 += 1,
                _ => spawns.any += 1,
            }
        }
    }
    Info {
        name: p.name.clone(),
        key: p.key.clone(),
        format: p.format,
        tool_version: crate::TOOL_VERSION.into(),
        seed: p.seed,
        terrain: p.terrain.as_ref().map(|t| {
            let (lo, hi) = h.raw_range().unwrap_or((0, 0));
            TerrainInfo {
                cells_x: t.cells_x,
                cells_z: t.cells_z,
                cell_size: t.cell_size,
                min_y: t.raw_to_y(lo),
                max_y: t.raw_to_y(hi),
            }
        }),
        counts: Counts {
            objects: p.objects.len(),
            solids: p.solids.iter().filter(|s| s.brush.is_none()).count(),
            brushes: p.solids.iter().filter(|s| s.brush.is_some()).count(),
            prefabs: p.prefabs.len(),
            imports: p.imports.len(),
            materials: p.materials.len(),
            polys,
        },
        objects_by_class: by_class,
        spawns,
        modes: p.mod_meta.modes.clone(),
        extent: all.map(|(lo, hi)| Extent {
            min: lo.to_array(),
            max: hi.to_array(),
            span: (hi - lo).to_array(),
        }),
        size_estimate_bytes: bytes,
        own_textures: own,
        rom_textures: rom_n,
        errors: d.count(Severity::Error),
        warnings: d.count(Severity::Warning),
        rom: rom.map(|(path, source)| RomInfo {
            path: path.display().to_string(),
            source,
        }),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// world-level checks (after build; MAPKIT_PLAN.md 5.3 step 6; DC-DEFAULT: MAP-47)

/// The body the spawn checks drop and clear: the game server's configured player box when no ROM body is at
/// hand, half extents [24, 53, 24] (`dc-dserver game.rs` `player_dims`; `Cylinder::from_dims` takes radius
/// `max(x, z)` and half height `y`). The real ROM stand box is [15, 50, 15] for every player model, SMALLER
/// than this, so a spawn that clears this body clears the ROM body too: the check is conservative.
// SPEC: dc-dserver game/arrival.rs `floor_point` (DSV-48: `Cylinder::from_dims(dims_of(pid))` lifted
// SPAWN_LIFT 18, dropped at most 1000 through the LEVEL-only trace world); game.rs `player_dims` [24, 53, 24]
// (the fallback when `player_dims_from_rom` finds no stand box); the ROM stand box [15, 50, 15] OBSERVED by the
// review's stand-clip scan over CA_ROM
pub const SERVER_BODY: [f32; 3] = [24.0, 53.0, 24.0];
/// Radius and half height of [`SERVER_BODY`] as the server's cylinder takes them.
pub const STAND_RADIUS: f32 = 24.0;
pub const STAND_HALF_HEIGHT: f32 = 53.0;
/// How far below a spawn the floor may lie (`spawn.floor`).
pub const FLOOR_REACH: f32 = 8.0;
/// Lift before the floor sweep: the server's `SPAWN_LIFT` (dc-dserver game/arrival.rs, DSV-48), skipped when
/// the lifted body overlaps.
pub const FLOOR_LIFT: f32 = 18.0;
/// Lift of the clearance test (a body the server drops rests a hair above the floor).
pub const CLEAR_LIFT: f32 = 1.0;

/// The collision world the server drops a spawn through: the LEVEL model only (`dc-dserver world.rs
/// trace_world`: every model no object names), no brushes, no blockers.
fn server_drop_filter() -> dc_sim::from_world::WorldFilter {
    dc_sim::from_world::WorldFilter {
        level: true,
        solid_roles: Vec::new(),
        unlabelled: false,
        movers: Vec::new(),
        blockers: false,
    }
}

/// The brushes the `ground: true` snap counts as floors but the server's drop does not: world models and
/// breakables alone.
fn brush_floor_filter() -> dc_sim::from_world::WorldFilter {
    dc_sim::from_world::WorldFilter {
        level: false,
        solid_roles: vec![
            dc_world::ObjectRole::PlainWorldModel,
            dc_world::ObjectRole::Breakable,
        ],
        unlabelled: false,
        movers: Vec::new(),
        blockers: false,
    }
}
/// Tolerance of the convexity and closedness tests (world units).
const EPS: f32 = 0.1;

fn name_text(o: &dc_world::Object<'_>) -> String {
    String::from_utf8_lossy(o.name().unwrap_or(b"?")).into_owned()
}

fn model_name(m: &dc_world::BspModel<'_>) -> String {
    String::from_utf8_lossy(m.name).into_owned()
}

/// Every world-level check over a built `.dat`: parse + layout, the model rules (one level model, brush flags,
/// every solid polygon referenced, no helper in a brush), ladders convex, kill volumes closed, bomb site numbers,
/// spawn floor / clearance through the dc-sim collision world, the extents (pos48, every model box inside) and
/// the sky (portals and a sky model where sky faces exist).
pub fn check_world(dat: &[u8]) -> Diagnostics {
    let mut d = Diagnostics::new();
    let w = match dc_world::World::parse(dat) {
        Ok(w) => w,
        Err(e) => {
            d.error(Code::WorldParse, "world", e.to_string());
            return d;
        }
    };
    if let Err(e) = w.check_layout() {
        d.error(Code::WorldLayout, "world", e.to_string());
    }
    check_models(&w, &mut d);
    check_brushes(&w, &mut d);
    check_world_objects(&w, &mut d);
    check_spawns(&w, &mut d);
    check_extents(&w, &mut d);
    check_sky(&w, &mut d);
    d
}

/// Models whose name no object carries (the level geometry, MAP-3).
pub fn level_models<'w, 'a>(w: &'w dc_world::World<'a>) -> Vec<&'w dc_world::BspModel<'a>> {
    w.models
        .iter()
        .filter(|m| w.object_named(m.name).is_none())
        .collect()
}

fn check_models(w: &dc_world::World<'_>, d: &mut Diagnostics) {
    let levels = level_models(w);
    if levels.len() != 1 {
        let names: Vec<String> = levels.iter().map(|m| model_name(m)).collect();
        d.error(
            Code::WorldLevel,
            "world.models",
            format!(
                "{} models no object names (want exactly 1): {names:?}",
                levels.len()
            ),
        );
    }
    for m in &w.models {
        let path = format!("world.models.{}", model_name(m));
        let named = w.object_named(m.name).is_some();
        if named && m.flags & worldgen::BSP_FLAG_BRUSH == 0 {
            d.error(
                Code::WorldLevel,
                &path,
                "a brush model without bsp flag bit 1",
            );
        }
        if !named && m.flags & worldgen::BSP_FLAG_BRUSH != 0 {
            d.error(
                Code::WorldLevel,
                &path,
                "the level model carries bsp flag bit 1",
            );
        }
        // every solid polygon referenced by a node
        let mut seen = vec![false; m.polygons.len()];
        for n in &m.nodes {
            if let Some(s) = seen.get_mut(n.poly as usize) {
                *s = true;
            }
        }
        let missing = m
            .polygons
            .iter()
            .enumerate()
            .filter(|(i, p)| {
                !seen[*i]
                    && m.surfaces
                        .get(p.surface as usize)
                        .is_some_and(|s| s.flags & geom::SURF_SOLID != 0)
            })
            .count();
        if missing > 0 {
            d.error(
                Code::BspUnreferenced,
                &path,
                format!("{missing} solid polygons are referenced by no BSP node"),
            );
        }
        if !m.polygons.is_empty() && !matches!(m.root, dc_world::NodeRef::Node(_)) {
            d.error(Code::BspUnreferenced, &path, "polygons but no root node");
        }
        if named {
            let helpers = m
                .polygons
                .iter()
                .filter(|p| {
                    p.corner_count < 3
                        || m.surfaces
                            .get(p.surface as usize)
                            .is_some_and(|s| s.flags == 0)
                })
                .count();
            if helpers > 0 {
                d.error(
                    Code::BrushHelper,
                    &path,
                    format!("{helpers} plane-only or flag-less polygons in a brush model"),
                );
            }
        }
    }
}

/// Every point of the model behind (or on) every polygon plane.
fn is_convex(m: &dc_world::BspModel<'_>) -> bool {
    m.polygons.iter().all(|p| {
        let Some(pl) = m.planes.get(p.plane as usize) else {
            return false;
        };
        let n = V3::from_array(pl.normal);
        m.points
            .iter()
            .all(|q| n.dot(V3::from_array(*q)) - pl.dist <= EPS)
    })
}

/// Every directed edge of the polygons has its reverse exactly once (a closed, consistently wound shell).
fn is_closed(m: &dc_world::BspModel<'_>) -> bool {
    let mut edges: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    let mut faces = 0;
    for p in &m.polygons {
        let pts = m.polygon_points(p);
        if pts.len() < 3 {
            continue;
        }
        faces += 1;
        for i in 0..pts.len() {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            if a == b {
                return false;
            }
            *edges.entry((a, b)).or_insert(0) += 1;
        }
    }
    faces >= 4
        && edges
            .iter()
            .all(|(&(a, b), &n)| n == 1 && edges.get(&(b, a)) == Some(&1))
}

fn check_brushes(w: &dc_world::World<'_>, d: &mut Diagnostics) {
    for o in w.objects_with_role(dc_world::ObjectRole::Ladder) {
        let Some(m) = w.brush_of(o) else {
            continue;
        };
        if !is_convex(m) {
            d.error(
                Code::LadderConvex,
                format!("objects.{}", name_text(o)),
                "the ladder brush is not convex",
            );
        }
    }
    for o in w.objects_with_role(dc_world::ObjectRole::KillVolume) {
        let Some(m) = w.brush_of(o) else {
            continue;
        };
        if !is_closed(m) {
            d.error(
                Code::KillvolClosed,
                format!("objects.{}", name_text(o)),
                "the kill volume brush is not closed",
            );
        }
    }
}

fn check_world_objects(w: &dc_world::World<'_>, d: &mut Diagnostics) {
    if w.object_named(worldgen::LEVEL_MODEL_NAME.as_bytes())
        .is_some()
    {
        d.push(
            Diagnostic::new(
                Code::WorldLevel,
                format!("objects.{}", worldgen::LEVEL_MODEL_NAME),
                "an object carries the level model's name",
            )
            .with_fix("rename the object"),
        );
    }
    let mut numbers: Vec<i64> = Vec::new();
    for o in w.objects_with_role(dc_world::ObjectRole::BombSite) {
        let n = o
            .prop_named(&dc_world::names::PROP_NUMBER)
            .and_then(|p| match p.value {
                dc_world::FieldValue::Int(v) => Some(v as i64),
                dc_world::FieldValue::Real(v) => Some(v as i64),
                _ => None,
            });
        match n {
            Some(n) if (0..=1).contains(&n) => numbers.push(n),
            _ => d.error(
                Code::BombNumber,
                format!("objects.{}", name_text(o)),
                "a bomb site without a `number` of 0 or 1",
            ),
        }
    }
    let mut sorted = numbers.clone();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.len() != numbers.len() {
        d.error(Code::BombNumber, "objects", "two bomb sites share a number");
    } else if !numbers.is_empty() && numbers.len() != 2 {
        d.push(
            Diagnostic::new(
                Code::BombCount,
                "objects",
                format!(
                    "{} bomb sites; BOMB plays with 2 (numbers 0 and 1)",
                    numbers.len()
                ),
            )
            .with_severity(Severity::Warning),
        );
    }
}

/// The spawn checks mirror the server's spawn placement (DSV-48, `dc-dserver game/arrival.rs floor_point`):
/// the [`SERVER_BODY`] lifted by [`FLOOR_LIFT`] (unless the lifted body overlaps) is swept down through the
/// LEVEL model alone; the floor must be met within [`FLOOR_REACH`] below the point (`spawn.floor`), a floor
/// that only a world-model / breakable brush offers is `spawn.brush_floor` (the server drops through it), and
/// the body at rest, lifted by [`CLEAR_LIFT`], must not overlap anything the player collides with (level,
/// solid brushes, blockers: `spawn.clear`).
fn check_spawns(w: &dc_world::World<'_>, d: &mut Diagnostics) {
    use dc_sim::{Collider, Cylinder, V3 as S3};
    let level = dc_sim::from_world::collision_world(w, &server_drop_filter());
    let brushes = dc_sim::from_world::collision_world(w, &brush_floor_filter());
    let all = dc_sim::from_world::collision_world(w, &dc_sim::from_world::WorldFilter::default());
    let shape = Cylinder::from_dims(S3::from_array(SERVER_BODY));
    let drop = |cw: &dc_sim::CollisionWorld, at: S3| -> Option<S3> {
        let lifted = at + S3::new(0.0, FLOOR_LIFT, 0.0);
        let start = if cw.overlaps(&shape, lifted) {
            at
        } else {
            lifted
        };
        let end = S3::new(at.x, at.y - FLOOR_REACH, at.z);
        cw.sweep(&shape, start, end)
            .map(|h| start + (end - start) * h.t)
    };
    for o in w.objects_with_role(dc_world::ObjectRole::PlayerSpawn) {
        let Some(p) = o.position() else {
            continue;
        };
        let path = format!("objects.{}", name_text(o));
        let at = S3::new(p[0], p[1], p[2]);
        let rest = match drop(&level, at) {
            Some(r) => r,
            None => {
                if drop(&brushes, at).is_some() {
                    d.push(
                        Diagnostic::new(
                            Code::SpawnBrushFloor,
                            &path,
                            "the floor under the spawn is a world_model / breakable brush top; the server \
drops a spawn through the level geometry only and would put the player through it",
                        )
                        .with_fix(
                            "make the floor a plain solid (no brush role), or move the spawn onto level \
geometry",
                        ),
                    );
                } else {
                    d.push(
                        Diagnostic::new(
                            Code::SpawnFloor,
                            &path,
                            format!(
                                "no level geometry within {FLOOR_REACH} units under the server's \
{} x {} standing body",
                                STAND_HALF_HEIGHT * 2.0,
                                STAND_RADIUS * 2.0
                            ),
                        )
                        .with_fix("apply `move` with `ground: true`, or put a floor under it"),
                    );
                }
                at
            }
        };
        if all.overlaps(&shape, rest + S3::new(0.0, CLEAR_LIFT, 0.0)) {
            d.push(
                Diagnostic::new(
                    Code::SpawnClear,
                    &path,
                    format!(
                        "the server's {} x {} standing body overlaps solid geometry where the spawn comes \
to rest",
                        STAND_HALF_HEIGHT * 2.0,
                        STAND_RADIUS * 2.0
                    ),
                )
                .with_fix("move the spawn into the open (apply `move` with `ground: true`)"),
            );
        }
    }
}

fn check_extents(w: &dc_world::World<'_>, d: &mut Diagnostics) {
    let lo = V3::from_array(w.extent_min);
    let hi = V3::from_array(w.extent_max);
    let span = hi - lo;
    for (axis, s) in ["x", "y", "z"].iter().zip([span.x, span.y, span.z]) {
        if s > EXTENT_LIMIT {
            d.error(
                Code::ExtentLimit,
                "world.extent",
                format!("{s} units on {axis} (limit {EXTENT_LIMIT})"),
            );
        } else if s > EXTENT_WARN {
            d.warn(
                Code::ExtentLarge,
                "world.extent",
                format!("{s} units on {axis} (precision drops above {EXTENT_WARN})"),
            );
        }
    }
    for m in &w.models {
        let (a, b) = (V3::from_array(m.box_min), V3::from_array(m.box_max));
        if a.x < lo.x - EPS
            || a.y < lo.y - EPS
            || a.z < lo.z - EPS
            || b.x > hi.x + EPS
            || b.y > hi.y + EPS
            || b.z > hi.z + EPS
        {
            d.warn(
                Code::ExtentModel,
                format!("world.models.{}", model_name(m)),
                "the model box reaches outside the world extents",
            );
        }
    }
}

fn check_sky(w: &dc_world::World<'_>, d: &mut Diagnostics) {
    // every sky face carries the client's sky surface id (C4): any model, not only the level
    for m in &w.models {
        let wrong = m
            .polygons
            .iter()
            .filter(|p| {
                m.surfaces.get(p.surface as usize).is_some_and(|s| {
                    s.flags & geom::SURF_SKY != 0 && s.texflags != worldgen::SURFACE_SKY
                })
            })
            .count();
        if wrong > 0 {
            d.error(
                Code::SkySurface,
                format!("world.models.{}", model_name(m)),
                format!(
                    "{wrong} sky faces (flag 0x10) carry texflags other than {:#x}",
                    worldgen::SURFACE_SKY
                ),
            );
        }
    }
    let sky_faces: usize = level_models(w)
        .iter()
        .map(|m| {
            m.polygons
                .iter()
                .filter(|p| {
                    m.surfaces
                        .get(p.surface as usize)
                        .is_some_and(|s| s.flags & geom::SURF_SKY != 0)
                })
                .count()
        })
        .sum();
    if sky_faces == 0 {
        return;
    }
    let Some(r) = &w.render else {
        return;
    };
    let portals: usize = r.world.blocks.iter().map(|b| b.sky_portals.len()).sum();
    if portals == 0 {
        d.warn(
            Code::SkyPortals,
            "world.render",
            format!("{sky_faces} sky faces but no sky portal in the level's render blocks"),
        );
    }
    let drawn = w
        .objects_with_role(dc_world::ObjectRole::SkyWorldModel)
        .any(|o| {
            o.name()
                .is_some_and(|n| r.world.models.iter().any(|m| m.name == n))
        });
    if !drawn {
        d.push(
            Diagnostic::new(
                Code::SkyModel,
                "world.render",
                format!("{sky_faces} sky faces but no dc.sky_model brush with a render world"),
            )
            .with_fix("add a solid with brush `sky` (the templates have one)"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{MapObject, Material, Solid, Terrain, TerrainLayer};
    use std::collections::BTreeMap;

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dc-mapkit-validate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        dir
    }

    fn with_codes(d: &Diagnostics) -> Vec<Code> {
        d.iter().map(|x| x.code).collect()
    }

    #[test]
    fn a_fresh_template_is_clean_and_broken_projects_name_their_codes() {
        let dir = tmp("codes");
        let made = crate::template::make(crate::template::Template::Flat, "Clean", "me", 3);
        for f in &made.files {
            std::fs::write(Project::resolve(&dir, &f.path), &f.bytes).unwrap();
        }
        let d = validate(&dir, &made.project, &made.heights);
        assert!(!d.has_errors(), "{:?}", d.items);
        let i = info(&dir, &made.project, &made.heights, None);
        assert_eq!(
            i.spawns,
            Spawns {
                team1: 8,
                team2: 8,
                any: 16
            }
        );
        assert_eq!(i.own_textures, 4);
        assert!(i.size_estimate_bytes > 4 * 256 * 256 * 4);
        assert!(i.extent.as_ref().unwrap().span[0] >= 4096.0);

        let mut p = made.project.clone();
        p.key = "Bad Key".into();
        p.next_id = 1;
        let dup = p.objects[0].id.clone();
        p.objects.push(MapObject {
            id: dup,
            ..Default::default()
        });
        p.objects.push(MapObject {
            id: "bogus".into(),
            class: "dc.nothing".into(),
            ..Default::default()
        });
        p.materials.push(Material {
            id: "missing".into(),
            texture: TextureRef::Own("assets/none.png".into()),
            ..Default::default()
        });
        p.materials.push(Material {
            id: "rom".into(),
            texture: TextureRef::Rom("x/y.dtx".into()),
            ..Default::default()
        });
        p.materials.push(Material {
            id: "case".into(),
            texture: TextureRef::Own("assets/Ground.png".into()),
            ..Default::default()
        });
        p.imports.push(crate::project::Import {
            id: "import_900".into(),
            file: "..\\x.obj".into(),
            material: "nope".into(),
            ..Default::default()
        });
        p.solids.push(Solid {
            id: "solid_901".into(),
            size: [0.0; 3],
            material: "ground".into(),
            ..Default::default()
        });
        p.mod_meta.description = "short".into();
        p.mod_meta.modes.push("RACE".into());
        let d = validate(&dir, &p, &made.heights);
        let codes = with_codes(&d);
        for c in [
            Code::ProjectKey,
            Code::IdNext,
            Code::IdDuplicate,
            Code::IdFormat,
            Code::ObjectClass,
            Code::TextureMissing,
            Code::RomUnverified,
            Code::PathCase,
            Code::PathInvalid,
            Code::MaterialMissing,
            Code::SolidSize,
            Code::MetaInvalid,
            Code::ModeUnknown,
            Code::MaterialUnused,
        ] {
            assert!(codes.contains(&c), "{c} missing in {codes:?}");
        }
        assert!(d.has_errors());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cylinder_sides_and_prism_points_stop_at_255_corners() {
        // C14: the world file counts a polygon's corners in one byte
        use crate::geom::unit_circle;
        use crate::project::{check_solid_corners, SolidKind, MAX_CORNERS};
        let dir = tmp("corners");
        let made = crate::template::make(crate::template::Template::Flat, "Corners", "me", 3);
        for f in &made.files {
            std::fs::write(Project::resolve(&dir, &f.path), &f.bytes).unwrap();
        }
        let cylinder = |sides: u32| Solid {
            id: "solid_900".into(),
            kind: SolidKind::Cylinder {
                sides,
                circle: unit_circle(sides),
            },
            pos: [0.0, 64.0, 0.0],
            size: [400.0, 128.0, 400.0],
            material: "block".into(),
            ..Default::default()
        };
        assert!(check_solid_corners(&cylinder(255)).is_ok());
        assert!(check_solid_corners(&cylinder(256)).is_err());
        let mut p = made.project.clone();
        p.solids.push(cylinder(MAX_CORNERS as u32));
        p.next_id = p.next_id.max(901);
        let d = validate(&dir, &p, &made.heights);
        assert!(!d.has_errors(), "{:?}", d.items);
        p.solids.pop();
        p.solids.push(cylinder(256));
        let d = validate(&dir, &p, &made.heights);
        let shape: Vec<_> = d.iter().filter(|x| x.code == Code::SolidShape).collect();
        assert_eq!(shape.len(), 1, "{:?}", d.items);
        assert_eq!(
            shape[0].path,
            format!("solids[{}].kind", p.solids.len() - 1)
        );
        assert!(shape[0].message.contains("256 corners"));
        let b = crate::build::build(&p, &made.heights, &Assets::new(&dir), &p.build).unwrap_err();
        assert!(b.iter().any(|x| x.code == Code::SolidShape));
        // a prism outline of 256 points (a regular polygon: convex) is refused the same way
        let points: Vec<[f32; 2]> = unit_circle(256)
            .iter()
            .map(|c| [c[0] * 200.0, c[1] * 200.0])
            .collect();
        let prism = Solid {
            id: "solid_901".into(),
            kind: SolidKind::Prism { points },
            pos: [0.0, 64.0, 0.0],
            size: [400.0, 128.0, 400.0],
            material: "block".into(),
            ..Default::default()
        };
        assert!(check_solid_corners(&prism).is_err());
        p.solids.pop();
        p.solids.push(prism);
        p.next_id = 902;
        let d = validate(&dir, &p, &made.heights);
        assert!(
            d.iter().any(|x| x.code == Code::SolidShape),
            "{:?}",
            d.items
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn spawn_bomb_flag_and_extent_rules() {
        let dir = tmp("rules");
        let mut p = Project {
            terrain: Some(Terrain {
                cells_x: 4,
                cells_z: 4,
                cell_size: 20000.0,
                layers: vec![TerrainLayer {
                    material: "g".into(),
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        p.materials.push(Material {
            id: "g".into(),
            texture: TextureRef::Own("assets/g.png".into()),
            ..Default::default()
        });
        p.settings.sky_material = "g".into();
        std::fs::write(
            dir.join("assets/g.png"),
            crate::template::textures::checker([0; 3], [255; 3], 16),
        )
        .unwrap();
        p.mod_meta.modes = vec!["BOMB".into(), "FLAG".into(), "FFA".into()];
        let mut props = BTreeMap::new();
        props.insert(v::PROP_NUMBER.to_string(), PropValue::Num(5.0));
        props.insert(v::PROP_EXTENT.to_string(), PropValue::Vec3([1.0; 3]));
        p.objects.push(MapObject {
            id: "bomb_site_1".into(),
            class: v::CLASS_BOMB_SITE.into(),
            props: props.clone(),
            ..Default::default()
        });
        props.insert(v::PROP_NUMBER.to_string(), PropValue::Num(0.0));
        p.objects.push(MapObject {
            id: "bomb_site_2".into(),
            class: v::CLASS_BOMB_SITE.into(),
            pos: [0.0, -50000.0, 0.0],
            props,
            ..Default::default()
        });
        p.next_id = 3;
        let h = Heights::for_terrain(p.terrain.as_ref().unwrap(), 0);
        let d = validate(&dir, &p, &h);
        let codes = with_codes(&d);
        for c in [
            Code::BombNumber,
            Code::FlagCount,
            Code::SpawnCount,
            Code::ExtentLimit,
            Code::ExtentLarge,
            Code::ExtentEnclose,
        ] {
            assert!(codes.contains(&c), "{c} missing in {codes:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
