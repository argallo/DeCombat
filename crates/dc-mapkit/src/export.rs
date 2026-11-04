//! `dc-mapkit export`: the built world as a `shared` mod folder / zip (MAPKIT_PLAN.md 5.3 step 7, D2).
//! DC-DEFAULT: MAP-50 (layout and record), MAP-51 (own textures), MAP-52 (screenshot and radar).
//!
//! ```text
//! export::export(&Project, &Built, out_dir: &Path, ExportOptions { zip, no_rom_paths, project_dir })
//!     -> Result<Exported { dir, zip, sha256, files, bytes, diagnostics }, Diagnostics>
//! ```
//!
//! Writes `<out_dir>/mod/<mod id>/` from scratch (a previous export is removed first, so the folder holds
//! exactly what this project produces):
//!
//! | file | content |
//! |---|---|
//! | `manifest.json` | kind `shared`, capabilities `assets` + `content.maps`, name / version / description / author / tags / homepage from `mod_meta`, `screenshots/main.png` |
//! | `main.lua` | `mod.content.maps:register{...}` (MAPKIT_PLAN.md 5.4): `id` = the project key, `name`, `description`, `world`, `modes`, `goals`, `round_seconds`, `players`, and `picture` / `loading` when the project ships `assets/pictures/<key>.png` / `<key>_load.png`, `radar` always |
//! | `assets/maps/<key>.dat` | the built world |
//! | `assets/<own texture>.dtx` | every `{"own": ...}` PNG as a one-level 32-bit DTX, at the PNG's path with the extension `.dtx` (the build names them `@mod:<id>/assets/<...>.dtx`) |
//! | `assets/pictures/<key>_radar.dtx` | the software `top` render (256 x 256) with its world rectangle in the record |
//! | `screenshots/main.png` | the software `iso` render (640 x 480) |
//! | `README.md` | what the mod is and how to use it |
//!
//! Then `dc_modkit::pack_mod_bytes` validates, lints and packs the folder (the same bytes and SHA-256 the lobby
//! hashes); `zip` writes `<out_dir>/<mod id>.zip`. `no_rom_paths` refuses a project with any `{"rom": ...}`
//! texture (CLEANROOM §4: committed examples use own textures only); without it a ROM texture path is warned
//! `rom.unverified` (the client resolves it from its own ROM at run time). The size of the folder is compared
//! near the 32 MiB cap (`size.estimate`) and over it (`size.limit`).

use crate::build::Built;
use crate::diag::{Code, Diagnostic, Diagnostics, Severity};
use crate::project::{check_key, check_rel_path, write_atomic, Project, TextureRef};
use crate::render_sw::{self, RenderOptions, Scene, View};
use crate::validate::{SIZE_LIMIT, SIZE_WARN};
use dc_mod_lua::{ModKind, ModManifest, SemVer};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExportOptions {
    /// Also write `<out_dir>/<mod id>.zip`.
    pub zip: bool,
    /// Refuse a project with a ROM texture path.
    pub no_rom_paths: bool,
    /// The project folder (own PNGs are read from it); `None` = the parent of `out_dir`.
    pub project_dir: Option<PathBuf>,
    /// Pack a world whose world-level checks reported errors (`--force`); without it such a build is refused
    /// before anything is written (DC-DEFAULT: MAP-50).
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Exported {
    /// The mod folder (`<out_dir>/mod/<mod id>`).
    pub dir: PathBuf,
    pub zip: Option<PathBuf>,
    /// SHA-256 of the packed zip (what the lobby's mod set carries).
    pub sha256: Option<String>,
    /// Files written into the mod folder, relative, sorted.
    pub files: Vec<String>,
    /// Bytes of the mod folder.
    pub bytes: u64,
    /// Non-fatal findings (size, unverified ROM paths, dropped modes).
    pub diagnostics: Diagnostics,
}

/// Radar picture size (DC-DEFAULT: MAP-52).
pub const RADAR_SIZE: u32 = 256;
/// Screenshot size (DC-DEFAULT: MAP-52).
pub const SCREENSHOT_SIZE: (u32, u32) = (640, 480);
/// Where the mod folder goes under `out_dir`.
pub const MOD_DIR: &str = "mod";
/// The screenshot the manifest lists.
pub const SCREENSHOT: &str = "screenshots/main.png";

/// A Lua string literal of `s` (quotes, backslashes, newlines and control characters escaped).
pub fn lua_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\{}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Cut a text to `max` characters (whole characters).
fn cut(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// The own texture's path inside the mod: the PNG path (under `assets/`) with its extension replaced by
/// `.dtx` by the ONE rule the world's texture names follow (`worldgen::dtx_rel_path`, MAP-45 / MAP-51).
pub fn dtx_path_of(rel: &str) -> String {
    let under_assets = if rel.starts_with("assets/") {
        rel.to_string()
    } else {
        format!("assets/{rel}")
    };
    crate::worldgen::dtx_rel_path(&under_assets)
}

/// PNG bytes -> DTX bytes.
/// A material's texture: as [`png_to_dtx`], but holes in a picture of a power-of-two size stay holes: paint on a wall
/// (DC-DEFAULT: MAP-110).
pub fn material_png_to_dtx(png: &[u8]) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(png)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    let (w, h) = (img.width(), img.height());
    if w <= u16::MAX as u32 && h <= u16::MAX as u32 && img.pixels().any(|p| p[3] < 128) {
        if let Some(dtx) = dc_tex::write_dxt1_cutout(w as u16, h as u16, img.as_raw()) {
            return Ok(dtx);
        }
    }
    png_to_dtx(png)
}

pub fn png_to_dtx(png: &[u8]) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(png)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 || w > u16::MAX as u32 || h > u16::MAX as u32 {
        return Err(format!("{w} x {h} is not a texture size (1..65535)"));
    }
    // opaque and a power of two: compressed with a mip chain, sharper for the bytes and no shimmer at a distance;
    // anything else as it is, 32 bits a texel (DC-DEFAULT: MAP-107)
    let opaque = img.pixels().all(|p| p[3] == 255);
    if opaque {
        if let Some(dtx) = dc_tex::write_dxt1_mipped(w as u16, h as u16, img.as_raw()) {
            return Ok(dtx);
        }
    }
    // a large opaque picture of another shape (a 1024 x 768 loading page, 3 MB as it is): the game stretches it over
    // the page anyway, so the nearest powers of two lose nothing that shows
    let down = |v: u32| {
        let p = v.next_power_of_two();
        if p > v {
            p / 2
        } else {
            p
        }
    };
    if opaque && w >= 256 && h >= 256 {
        let (pw, ph) = (down(w), down(h));
        let small = image::imageops::resize(&img, pw, ph, image::imageops::FilterType::Triangle);
        if let Some(dtx) = dc_tex::write_dxt1_mipped(pw as u16, ph as u16, small.as_raw()) {
            return Ok(dtx);
        }
    }
    dc_tex::write_bgra8(w as u16, h as u16, img.as_raw())
        .ok_or_else(|| "the texture could not be written".to_string())
}

struct Writer {
    dir: PathBuf,
    files: BTreeSet<String>,
    bytes: u64,
}

impl Writer {
    fn put(&mut self, rel: &str, bytes: &[u8]) -> Result<(), Diagnostics> {
        let path = Project::resolve(&self.dir, rel);
        write_atomic(&path, bytes).map_err(|e| {
            Diagnostics::one(Diagnostic::new(
                Code::ProjectIo,
                path.display().to_string(),
                e.to_string(),
            ))
        })?;
        self.files.insert(rel.to_string());
        self.bytes += bytes.len() as u64;
        Ok(())
    }
}

/// The map record `main.lua` registers (one call, the keys in the order of MAPKIT_PLAN.md 5.4).
#[allow(clippy::too_many_arguments)]
fn main_lua(
    p: &Project,
    modes: &[String],
    picture: bool,
    loading: bool,
    radar: [f32; 4],
) -> String {
    let m = &p.mod_meta;
    let key = &p.key;
    let mut s = String::new();
    s.push_str(&format!(
        "-- {} {}: a DeCombat community map exported by dc-mapkit {} from the project {}.\n\
-- Generated file: edit the project (map.json) and export again rather than this file.\n\
---@param mod ModCapability\n\
return function(mod)\n    mod.content.maps:register({{\n",
        m.id,
        m.version,
        crate::TOOL_VERSION,
        lua_str(&p.name)
    ));
    s.push_str(&format!("        id = {},\n", lua_str(key)));
    s.push_str(&format!(
        "        name = {},\n",
        lua_str(&cut(&p.name, dc_mod_lua::MAP_NAME_MAX))
    ));
    s.push_str(&format!(
        "        description = {},\n",
        lua_str(&cut(&m.description, dc_mod_lua::MAP_DESCRIPTION_MAX))
    ));
    s.push_str(&format!(
        "        world = mod.assets:path({}),\n",
        lua_str(&format!("assets/maps/{key}.dat"))
    ));
    let list: Vec<String> = modes.iter().map(|x| lua_str(x)).collect();
    s.push_str(&format!("        modes = {{ {} }},\n", list.join(", ")));
    let goals: Vec<String> = m
        .goals
        .iter()
        .filter(|(k, _)| modes.iter().any(|x| x == *k))
        .map(|(k, val)| format!("{k} = {val}"))
        .collect();
    if !goals.is_empty() {
        s.push_str(&format!("        goals = {{ {} }},\n", goals.join(", ")));
    }
    if let Some(r) = m.round_seconds {
        s.push_str(&format!("        round_seconds = {r},\n"));
    }
    if let Some(pl) = &m.players {
        s.push_str(&format!(
            "        players = {{ min = {}, max = {} }},\n",
            pl.min, pl.max
        ));
    }
    if picture {
        s.push_str(&format!(
            "        picture = mod.assets:path({}),\n",
            lua_str(&format!("assets/pictures/{key}.dtx"))
        ));
    }
    if loading {
        s.push_str(&format!(
            "        loading = mod.assets:path({}),\n",
            lua_str(&format!("assets/pictures/{key}_load.dtx"))
        ));
    }
    s.push_str(&format!(
        "        radar = {{ picture = mod.assets:path({}),\n                  min = {{ {}, {} }}, max = {{ {}, {} }} }},\n",
        lua_str(&format!("assets/pictures/{key}_radar.dtx")),
        radar[0],
        radar[1],
        radar[2],
        radar[3]
    ));
    s.push_str("    })\nend\n");
    s
}

fn readme(p: &Project, modes: &[String], files: &BTreeSet<String>) -> String {
    let m = &p.mod_meta;
    let mut s = format!(
        "# {}\n\n{}\n\nAuthor: {}. Version {}. A DeCombat community map (a `shared` mod with the `maps` content \
domain) exported by dc-mapkit {}.\n\nModes: {}.\n\n## How to play it\n\n1. Put the packed zip (`dc-mapkit export --zip`, \
or `dc-modkit pack` on this folder) into the server's mod set (HOSTING_AND_MODS.md section 4).\n2. Start a room on a \
modded (vanilla++) lobby and pick the map `{}` from the list.\n\nClients download the package from the server and \
unpack it under their profile; nothing here needs a manual install.\n\n## Files\n\n",
        p.name,
        m.description,
        m.author,
        m.version,
        crate::TOOL_VERSION,
        modes.join(", "),
        p.key
    );
    for f in files {
        s.push_str(&format!("- `{f}`\n"));
    }
    s.push_str(
        "\nThe world under `assets/maps/` and the pictures are build outputs: edit the dc-mapkit project and \
export again.\n",
    );
    s
}

/// Export the built world as a mod folder (and zip).
pub fn export(
    p: &Project,
    built: &Built,
    out_dir: &Path,
    opts: ExportOptions,
) -> Result<Exported, Diagnostics> {
    let mut d = Diagnostics::new();
    let project_dir = opts
        .project_dir
        .clone()
        .or_else(|| out_dir.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    let m = &p.mod_meta;

    // a world the tool itself judged broken is not packed (C18): the build's errors are the answer
    if built.diagnostics.has_errors() && !opts.force {
        let mut out = built.diagnostics.clone();
        out.push(
            Diagnostic::new(
                Code::BuildFail,
                "build",
                format!(
                    "the world-level checks reported {} errors; nothing exported",
                    built.diagnostics.count(Severity::Error)
                ),
            )
            .with_fix("fix the errors above, or pass --force to pack the world anyway"),
        );
        return Err(out);
    }

    // metadata the manifest and the record need
    if !check_key(&p.key) {
        d.error(Code::ProjectKey, "key", format!("`{}`", p.key));
    }
    if !dc_mod_lua::is_valid_mod_id(&m.id) {
        d.error(
            Code::MetaInvalid,
            "mod_meta.id",
            format!("`{}`: letters, digits, _ and - only", m.id),
        );
    }
    let version = match SemVer::parse(&m.version) {
        Ok(v) => v,
        Err(e) => {
            d.error(Code::MetaInvalid, "mod_meta.version", e);
            SemVer::new(0, 1, 0)
        }
    };
    let mut modes: Vec<String> = Vec::new();
    for (i, mode) in m.modes.iter().enumerate() {
        let upper = mode.trim().to_ascii_uppercase();
        if dc_config::modes::is_mode_name(&upper) {
            if !modes.contains(&upper) {
                modes.push(upper);
            }
        } else {
            d.warn(
                Code::ModeUnknown,
                format!("mod_meta.modes[{i}]"),
                format!("`{mode}` is not a DeCombat mode name; left out of the record"),
            );
        }
    }
    if modes.is_empty() {
        d.error(
            Code::MetaInvalid,
            "mod_meta.modes",
            "the record needs at least one DeCombat mode name",
        );
    }
    for (i, mat) in p.materials.iter().enumerate() {
        if let TextureRef::Rom(rel) = &mat.texture {
            if opts.no_rom_paths {
                d.push(
                    Diagnostic::new(
                        Code::ExportRomPath,
                        format!("materials[{i}].texture"),
                        format!("ROM texture `{rel}` (--no-rom-paths)"),
                    )
                    .with_fix("point the material at an own PNG under assets/"),
                );
            } else {
                d.warn(
                    Code::RomUnverified,
                    format!("materials[{i}].texture"),
                    format!("ROM texture `{rel}` is not shipped; players' clients resolve it from their ROM"),
                );
            }
        }
    }
    if d.has_errors() {
        return Err(d);
    }
    let world = dc_world::World::parse(&built.dat).map_err(|e| {
        Diagnostics::one(Diagnostic::new(
            Code::BuildFail,
            "build",
            format!("the built world does not parse: {e}"),
        ))
    })?;

    // a clean folder
    let dir = out_dir.join(MOD_DIR).join(&m.id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| {
            Diagnostics::one(Diagnostic::new(
                Code::ProjectIo,
                dir.display().to_string(),
                format!("remove the previous export: {e}"),
            ))
        })?;
    }
    let mut w = Writer {
        dir: dir.clone(),
        files: BTreeSet::new(),
        bytes: 0,
    };
    w.put(&format!("assets/maps/{}.dat", p.key), &built.dat)?;

    // own textures (MAP-51)
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (i, mat) in p.materials.iter().enumerate() {
        let TextureRef::Own(rel) = &mat.texture else {
            continue;
        };
        if let Err(e) = check_rel_path(rel) {
            d.error(Code::PathInvalid, format!("materials[{i}].texture"), e);
            continue;
        }
        let target = dtx_path_of(rel);
        if !seen.insert(target.clone()) {
            continue;
        }
        let src = Project::resolve(&project_dir, rel);
        let png = match std::fs::read(&src) {
            Ok(b) => b,
            Err(e) => {
                d.push(
                    Diagnostic::new(
                        Code::TextureMissing,
                        format!("materials[{i}].texture"),
                        format!("{rel}: {e}"),
                    )
                    .with_fix("put a PNG there or point the material at an existing one"),
                );
                continue;
            }
        };
        match material_png_to_dtx(&png) {
            Ok(dtx) => w.put(&target, &dtx)?,
            Err(e) => d.error(
                Code::TextureFormat,
                format!("materials[{i}].texture"),
                format!("{rel}: {e}"),
            ),
        }
    }
    if d.has_errors() {
        return Err(d);
    }

    // the world's own texture names must be files of the mod
    let prefix = format!("@mod:{}/", m.id);
    let mut named: BTreeSet<String> = BTreeSet::new();
    for model in &world.models {
        for name in &model.texture_names {
            let name = String::from_utf8_lossy(name);
            if let Some(rel) = name.strip_prefix(&prefix) {
                named.insert(rel.to_string());
            }
        }
    }
    for rel in named {
        if !w.files.contains(&rel) {
            d.push(
                Diagnostic::new(
                    Code::TextureMissing,
                    "build",
                    format!("the world names `{prefix}{rel}` but the export has no such file"),
                )
                .with_severity(Severity::Warning),
            );
        }
    }

    // pictures (MAP-52): optional room / loading pictures from the project, the radar and the screenshot from
    // the software renderer
    let key = &p.key;
    let mut picture = false;
    let mut loading = false;
    let mut title = false;
    for (flag, suffix) in [
        (&mut picture, ""),
        (&mut loading, "_load"),
        (&mut title, "_title"),
    ] {
        let rel = format!("assets/pictures/{key}{suffix}.png");
        let src = Project::resolve(&project_dir, &rel);
        if !src.is_file() {
            continue;
        }
        let png = std::fs::read(&src).map_err(|e| {
            Diagnostics::one(Diagnostic::new(
                Code::ProjectIo,
                src.display().to_string(),
                e.to_string(),
            ))
        })?;
        match png_to_dtx(&png) {
            Ok(dtx) => {
                w.put(&format!("assets/pictures/{key}{suffix}.dtx"), &dtx)?;
                *flag = true;
            }
            Err(e) => d.error(Code::TextureFormat, rel, e),
        }
    }
    // DC-DEFAULT: MAP-100a a published map should show something in the room and while it loads
    for (present, what, rel) in [
        (picture, "preview", format!("assets/pictures/{key}.png")),
        (
            loading,
            "loading",
            format!("assets/pictures/{key}_load.png"),
        ),
        (title, "title", format!("assets/pictures/{key}_title.png")),
    ] {
        if !present {
            d.push(
                Diagnostic::new(
                    Code::PictureMissing,
                    rel,
                    format!("the map has no {what} picture"),
                )
                .with_fix("`dc-mapkit pictures <dir>` draws both with the game's renderer, or add your own PNG here"),
            );
        }
    }
    let ropts = RenderOptions {
        sun_dir: crate::geom::V3::from_array(p.settings.sun_dir),
        ..Default::default()
    };
    let scene = Scene::from_world(&world, &ropts);
    let top = render_sw::render_report(&scene, View::Top, (RADAR_SIZE, RADAR_SIZE), &ropts)?;
    let radar = top.top_bounds.unwrap_or([
        scene.extent_min.x,
        scene.extent_min.z,
        scene.extent_max.x,
        scene.extent_max.z,
    ]);
    let radar_dtx = dc_tex::write_bgra8(RADAR_SIZE as u16, RADAR_SIZE as u16, &top.image.data)
        .expect("radar size is valid");
    w.put(&format!("assets/pictures/{key}_radar.dtx"), &radar_dtx)?;
    let shot = render_sw::render_report(&scene, View::Iso, SCREENSHOT_SIZE, &ropts)?;
    w.put(SCREENSHOT, &shot.image.png())?;

    // manifest and record
    let mut manifest = ModManifest::minimal(&m.id, &p.name, version, ModKind::Shared);
    manifest.capabilities = vec!["assets".into(), "content.maps".into()];
    manifest.description = m.description.clone();
    manifest.author = m.author.clone();
    manifest.homepage = m.homepage.clone();
    manifest.tags = m.tags.clone();
    manifest.screenshots = vec![SCREENSHOT.into()];
    // MOD-28: somebody else's work ships with its credit, in the pack and where players read about it
    if !m.credits.is_empty() {
        manifest.asset_policy = dc_mod_lua::AssetPolicy::Credited;
        manifest.credits = m
            .credits
            .iter()
            .map(|c| dc_mod_lua::ModCredit {
                title: c.title.clone(),
                author: c.author.clone(),
                license: c.license.clone(),
                source: c.source.clone(),
                changes: c.changes.clone(),
            })
            .collect();
        let mut text = format!("Credits for {}\n\n", p.name);
        for c in &m.credits {
            text.push_str(&format!(
                "{}\n  by {}\n  license {}\n  source {}\n  changes: {}\n\n",
                c.title, c.author, c.license, c.source, c.changes
            ));
        }
        w.put("CREDITS.txt", text.as_bytes())?;
        let mut authors: Vec<&str> = m.credits.iter().map(|c| c.author.as_str()).collect();
        authors.sort_unstable();
        authors.dedup();
        manifest.description = format!(
            "{} Uses models by {}; see CREDITS.txt.",
            manifest.description,
            authors.join(", ")
        );
    }
    if let Err(e) = manifest.check_metadata() {
        return Err(Diagnostics::one(Diagnostic::new(
            Code::MetaInvalid,
            "mod_meta",
            e,
        )));
    }
    w.put("manifest.json", manifest.to_json_pretty().as_bytes())?;
    w.put(
        "main.lua",
        main_lua(p, &modes, picture, loading, radar).as_bytes(),
    )?;
    let files_so_far = w.files.clone();
    w.put("README.md", readme(p, &modes, &files_so_far).as_bytes())?;

    // size against the caps
    if w.bytes > SIZE_LIMIT {
        d.error(
            Code::SizeLimit,
            dir.display().to_string(),
            format!("{} KiB is above the client's 32 MiB cap", w.bytes >> 10),
        );
        return Err(d);
    }
    if w.bytes > SIZE_WARN {
        d.warn(
            Code::SizeEstimate,
            dir.display().to_string(),
            format!("{} KiB is near the 32 MiB cap", w.bytes >> 10),
        );
    }

    // pack: validate + lint + deterministic zip
    let (zip_bytes, entry) = dc_modkit::pack_mod_bytes(&dir).map_err(|errs| {
        let mut out = d.clone();
        for e in errs {
            out.error(Code::ExportPack, dir.display().to_string(), e);
        }
        out
    })?;
    let mut zip = None;
    if opts.zip {
        let zpath = out_dir.join(format!("{}.zip", m.id));
        write_atomic(&zpath, &zip_bytes).map_err(|e| {
            Diagnostics::one(Diagnostic::new(
                Code::ProjectIo,
                zpath.display().to_string(),
                e.to_string(),
            ))
        })?;
        zip = Some(zpath);
    }
    Ok(Exported {
        dir,
        zip,
        sha256: Some(entry.sha256),
        files: w.files.into_iter().collect(),
        bytes: w.bytes,
        diagnostics: d,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::BuildStats;
    use crate::template::{self, Template};

    fn tmp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dc-mapkit-export-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A hand-made build output: the modkit placeholder floor, with texture names of this mod.
    fn built(mod_id: &str, extra: &[&str]) -> Built {
        use dc_world::synth::{SynthModel, SynthWorld};
        let mut textures: Vec<Vec<u8>> =
            vec![format!("@mod:{mod_id}/assets/ground.dtx").into_bytes()];
        textures.extend(extra.iter().map(|s| s.as_bytes().to_vec()));
        let dat = SynthWorld {
            info: b"dc-mapkit export test".to_vec(),
            extent_min: [-2048.0, -256.0, -2048.0],
            extent_max: [2048.0, 1024.0, 2048.0],
            tree_shape: vec![false],
            models: vec![SynthModel {
                name: b"level".to_vec(),
                textures,
                planes: vec![([0.0, 1.0, 0.0], 0.0)],
                surfaces: vec![(1, 0, 0)],
                polys: vec![(0, 0, vec![0, 1, 2, 3])],
                nodes: vec![(0, 0, -2, -1)],
                points: vec![
                    [-2048.0, 0.0, -2048.0],
                    [-2048.0, 0.0, 2048.0],
                    [2048.0, 0.0, 2048.0],
                    [2048.0, 0.0, -2048.0],
                ],
                root: 0,
                box_min: [-2048.0, 0.0, -2048.0],
                box_max: [2048.0, 0.0, 2048.0],
                ..SynthModel::default()
            }],
            ..SynthWorld::default()
        }
        .build();
        Built {
            stats: BuildStats {
                polys: 1,
                bytes: dat.len(),
                ..Default::default()
            },
            dat,
            diagnostics: Diagnostics::new(),
        }
    }

    #[test]
    fn a_flat_template_exports_to_a_packable_shared_mod_twice_the_same() {
        let dir = tmp("flat");
        let made = template::make(Template::Flat, "Export Test", "tester", 3);
        crate::cli::write_project(&dir, &made, Template::Flat).unwrap();
        let p = made.project.clone();
        let b = built(&p.mod_meta.id, &[]);
        let out = dir.join("build");
        let opts = ExportOptions {
            zip: true,
            no_rom_paths: true,
            project_dir: Some(dir.clone()),
            force: false,
        };
        let e = export(&p, &b, &out, opts.clone()).unwrap_or_else(|d| panic!("{:?}", d.items));
        assert_eq!(e.dir, out.join("mod").join(&p.mod_meta.id));
        // a template has no pictures of its own yet: only the three reminders (MAP-100a)
        assert_eq!(e.diagnostics.items.len(), 3, "{:?}", e.diagnostics.items);
        assert!(
            e.diagnostics
                .items
                .iter()
                .all(|x| x.code == Code::PictureMissing),
            "{:?}",
            e.diagnostics.items
        );
        let sha = e.sha256.clone().unwrap();
        assert_eq!(sha.len(), 64);
        let zip = e.zip.clone().unwrap();
        assert!(zip.is_file());
        for f in [
            "manifest.json",
            "main.lua",
            "README.md",
            "screenshots/main.png",
            "assets/ground.dtx",
            "assets/wall.dtx",
            "assets/block.dtx",
            "assets/sky.dtx",
        ] {
            assert!(e.files.contains(&f.to_string()), "{f} in {:?}", e.files);
            assert!(e.dir.join(f).is_file(), "{f} exists");
        }
        let key = &p.key;
        assert!(e.files.contains(&format!("assets/maps/{key}.dat")));
        assert!(e
            .files
            .contains(&format!("assets/pictures/{key}_radar.dtx")));
        // manifest: shared, the two capabilities, the screenshot
        let manifest = dc_mod_lua::ModManifest::from_file(&e.dir.join("manifest.json")).unwrap();
        assert_eq!(manifest.kind, dc_mod_lua::ModKind::Shared);
        assert_eq!(manifest.capabilities, vec!["assets", "content.maps"]);
        assert_eq!(manifest.screenshots, vec![SCREENSHOT]);
        assert_eq!(manifest.author, "tester");
        // the record: key, world path, every template mode, the radar
        let lua = std::fs::read_to_string(e.dir.join("main.lua")).unwrap();
        assert!(lua.contains(&format!("id = \"{key}\"")));
        assert!(lua.contains(&format!("assets/maps/{key}.dat")));
        assert!(lua.contains("\"TDM\", \"FFA\", \"ELIM\", \"BOMB\", \"FLAG\""));
        assert!(lua.contains("radar = { picture"));
        assert!(
            lua.contains("min = { -2"),
            "the radar rectangle covers the world: {lua}"
        );
        assert!(!lua.contains("picture = mod.assets:path(\"assets/pictures/export_test.dtx\")"));
        // the own textures are DTX files the reader opens, the radar too
        let dtx = std::fs::read(e.dir.join("assets/ground.dtx")).unwrap();
        let f = dc_tex::DtxFile::parse(&dtx).unwrap();
        assert_eq!((f.header().width, f.header().height), (256, 256));
        let radar = std::fs::read(e.dir.join(format!("assets/pictures/{key}_radar.dtx"))).unwrap();
        let f = dc_tex::DtxFile::parse(&radar).unwrap();
        assert_eq!(
            (f.header().width, f.header().height),
            (RADAR_SIZE as u16, RADAR_SIZE as u16)
        );
        // the zip the lobby would hash: same bytes on a second export
        let bytes1 = std::fs::read(&zip).unwrap();
        let e2 = export(&p, &b, &out, opts).unwrap();
        assert_eq!(e2.sha256.unwrap(), sha);
        assert_eq!(std::fs::read(&zip).unwrap(), bytes1);
        assert_eq!(e2.files, e.files);
        // the modkit validator accepts the folder on its own (record + world parse)
        let report = dc_modkit::validate_mod(&e.dir, None);
        assert!(report.ok, "{:?}", report.errors);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rom_paths_are_refused_with_the_flag_and_warned_without_and_missing_textures_are_named() {
        let dir = tmp("rom");
        let made = template::make(Template::Flat, "Rom Test", "tester", 1);
        crate::cli::write_project(&dir, &made, Template::Flat).unwrap();
        let mut p = made.project.clone();
        p.materials[1].texture = TextureRef::Rom("some/rom/wall.dtx".into());
        let b = built(&p.mod_meta.id, &["@mod:rom_test/assets/nope.dtx"]);
        let out = dir.join("build");
        let e = export(
            &p,
            &b,
            &out,
            ExportOptions {
                zip: false,
                no_rom_paths: true,
                project_dir: Some(dir.clone()),
                force: false,
            },
        )
        .unwrap_err();
        assert_eq!(e.items[0].code, Code::ExportRomPath);
        assert!(!out.join("mod").exists(), "nothing written on refusal");
        let e = export(
            &p,
            &b,
            &out,
            ExportOptions {
                zip: false,
                no_rom_paths: false,
                project_dir: Some(dir.clone()),
                force: false,
            },
        )
        .unwrap();
        assert!(e.zip.is_none());
        assert!(e.sha256.is_some());
        let codes: Vec<Code> = e.diagnostics.iter().map(|d| d.code).collect();
        assert!(codes.contains(&Code::RomUnverified), "{codes:?}");
        assert!(codes.contains(&Code::TextureMissing), "{codes:?}");
        assert!(!e.diagnostics.has_errors());
        assert!(!e.files.contains(&"assets/wall.dtx".to_string()));
        // bad metadata is refused before anything is written
        p.mod_meta.modes = vec!["nothing".into()];
        let e = export(
            &p,
            &b,
            &dir.join("build2"),
            ExportOptions {
                zip: false,
                no_rom_paths: false,
                project_dir: Some(dir.clone()),
                force: false,
            },
        )
        .unwrap_err();
        assert!(e.iter().any(|d| d.code == Code::MetaInvalid));
        assert!(e.iter().any(|d| d.code == Code::ModeUnknown));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_world_with_errors_is_not_packed_without_force() {
        // C18: export refuses a build whose world-level checks reported errors; --force packs it anyway
        let dir = tmp("force");
        let made = template::make(Template::Flat, "Force Test", "tester", 2);
        crate::cli::write_project(&dir, &made, Template::Flat).unwrap();
        let p = made.project.clone();
        let mut b = built(&p.mod_meta.id, &[]);
        b.diagnostics.error(
            Code::KillvolClosed,
            "objects.solid_7",
            "the kill volume brush is not closed",
        );
        let out = dir.join("build");
        let opts = |force: bool| ExportOptions {
            zip: true,
            no_rom_paths: true,
            project_dir: Some(dir.clone()),
            force,
        };
        let e = export(&p, &b, &out, opts(false)).unwrap_err();
        assert!(e.has_errors());
        assert!(
            e.iter().any(|d| d.code == Code::KillvolClosed),
            "{:?}",
            e.items
        );
        assert!(
            e.iter()
                .any(|d| d.code == Code::BuildFail && d.message.contains("nothing exported")),
            "{:?}",
            e.items
        );
        assert!(!out.join("mod").exists(), "nothing written on refusal");
        let ok = export(&p, &b, &out, opts(true)).unwrap_or_else(|d| panic!("{:?}", d.items));
        assert!(ok.dir.join("manifest.json").is_file());
        assert!(ok.zip.is_some());
        // a build with warnings only is exported without the flag
        let mut warned = built(&p.mod_meta.id, &[]);
        warned
            .diagnostics
            .warn(Code::SpawnFloor, "objects.ps_1", "no floor");
        assert!(export(&p, &warned, &out, opts(false)).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn helpers() {
        assert_eq!(lua_str("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
        assert_eq!(dtx_path_of("assets/ground.png"), "assets/ground.dtx");
        assert_eq!(dtx_path_of("tex/x.PNG"), "assets/tex/x.dtx");
        assert_eq!(dtx_path_of("assets/noext"), "assets/noext.dtx");
        // C19: the two inputs of the finding follow the world's naming rule
        assert_eq!(dtx_path_of("assets/v1.0/ground"), "assets/v1.0/ground.dtx");
        assert_eq!(dtx_path_of("assets/.hidden"), "assets/.hidden.dtx");
        let png = template::textures::checker([1, 2, 3], [4, 5, 6], 8);
        let dtx = png_to_dtx(&png).unwrap();
        let f = dc_tex::DtxFile::parse(&dtx).unwrap();
        assert_eq!(f.header().width, 256);
        assert!(png_to_dtx(b"not a png").is_err());
    }
}
