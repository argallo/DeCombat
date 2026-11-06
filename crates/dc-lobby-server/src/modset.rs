//! The server's mod set: the `mod_set` fragment of the server-list data and the mod-set hash.
//!
//! SPEC: HOSTING_AND_MODS.md §4 item 6 (decided 2026-10-24). The fragment is
//! `{"policy":"vanilla|modded","client_visual_allowed":bool,"mods":[{"id","version","sha256","kind",
//! "requires_client","capabilities":[...],"source_repo"?}]}` in load order. The hash is SHA-256 over the UTF-8
//! text made of one line `id@version:sha256` followed by a line feed per server-declared mod (kinds
//! `server_rules` and `shared`, in load order); client-visual mods are not part of it. The empty set hashes
//! to SHA-256 of the empty string, which a vanilla server accepts only.
//!
//! The declared set comes from `server.toml` (`[[mods.set]]`, the load order the operator wrote). Where the mods
//! directory holds the mod itself (a folder with `manifest.json`), the entry is completed from the packed mod
//! (kind, capabilities) and the declared sha256 is checked against the pack; where only `<sha256>.zip` is there,
//! the entry keeps the declared facts.
//!
//! LG-270: the set is also what the lobby loads (`resolve_declared`): exactly the declared entries, in their order,
//! each from `<sha256>.zip` (unpacked into `<data dir>/mod_cache/<sha256>/`) or else from the folder `<id>/` when its
//! pack has the declared hash; a declared mod that cannot be backed stops the start. Folders that are not declared
//! are never loaded. Without a server.toml (CLI-only runs) the lobby falls back to loading every folder.

use crate::serverconf::ServerConf;
use dc_mod_lua::JsonValue;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// MOD-27: the name of an operator picture after the mod id.
pub const PREVIEW_SUFFIX: &str = ".preview.png";
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModEntry {
    pub id: String,
    pub version: String,
    pub sha256: String,
    /// `server_rules`, `shared` or `client_visual`; unknown for a declared entry known only by its zip.
    pub kind: Option<String>,
    pub requires_client: bool,
    pub capabilities: Vec<String>,
    pub source_repo: Option<String>,
    /// Browser metadata from the manifest (empty for an entry whose package could not be read).
    pub description: String,
    pub author: String,
    pub tags: Vec<String>,
    /// File names inside the zip; the client reads the images from the package it downloads.
    pub screenshots: Vec<String>,
    /// MOD-27: the manifest's command help.
    pub commands: Vec<dc_mod_lua::ModCommand>,
    /// MOD-27: SHA-256 of the operator's picture `<id>.preview.png` in the mods dir (served at `/mods/<it>.png`); no
    /// part of the mod-set hash.
    pub preview: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModSet {
    pub modded: bool,
    pub client_visual_allowed: bool,
    pub mods: Vec<ModEntry>,
}

impl ModSet {
    pub fn vanilla() -> ModSet {
        ModSet {
            modded: false,
            client_visual_allowed: false,
            mods: Vec::new(),
        }
    }

    /// The first mod that declares `world.full_visibility` (the match's culling is off), if any.
    // DC-DEFAULT: MOD-22 (proposed)
    pub fn full_visibility_mod(&self) -> Option<&str> {
        self.mods
            .iter()
            .find(|m| m.capabilities.iter().any(|c| c == "world.full_visibility"))
            .map(|m| m.id.as_str())
    }

    /// The first mod that declares `combat.no_recoil` (rooms may turn recoil off, LG-274), if any.
    // DC-DEFAULT: MOD-23 (proposed)
    pub fn no_recoil_mod(&self) -> Option<&str> {
        self.mods
            .iter()
            .find(|m| m.capabilities.iter().any(|c| c == "combat.no_recoil"))
            .map(|m| m.id.as_str())
    }

    /// The text that is hashed: `id@version:sha256` and a line feed per server-declared mod.
    pub fn hash_text(&self) -> String {
        // the same text the client hashes at login (dc-mod-lua holds the one implementation)
        dc_mod_lua::mod_set_hash_text(self.mods.iter().map(|m| dc_mod_lua::HashEntry {
            id: &m.id,
            version: &m.version,
            sha256: &m.sha256,
            kind: m.kind.as_deref(),
        }))
    }

    /// The mod-set hash (lower-case hex SHA-256).
    pub fn hash(&self) -> String {
        dc_rom::sha256_hex(self.hash_text().as_bytes())
    }

    /// The `mod_set` fragment.
    pub fn fragment(&self) -> JsonValue {
        let mut o = BTreeMap::new();
        o.insert(
            "policy".to_string(),
            JsonValue::String(if self.modded { "modded" } else { "vanilla" }.to_string()),
        );
        o.insert(
            "client_visual_allowed".to_string(),
            JsonValue::Bool(self.modded && self.client_visual_allowed),
        );
        let mods = self
            .mods
            .iter()
            .map(|m| {
                let mut e = BTreeMap::new();
                e.insert("id".to_string(), JsonValue::String(m.id.clone()));
                e.insert("version".to_string(), JsonValue::String(m.version.clone()));
                e.insert("sha256".to_string(), JsonValue::String(m.sha256.clone()));
                if let Some(k) = &m.kind {
                    e.insert("kind".to_string(), JsonValue::String(k.clone()));
                }
                e.insert(
                    "requires_client".to_string(),
                    JsonValue::Bool(m.requires_client),
                );
                e.insert(
                    "capabilities".to_string(),
                    JsonValue::Array(
                        m.capabilities
                            .iter()
                            .cloned()
                            .map(JsonValue::String)
                            .collect(),
                    ),
                );
                if let Some(r) = &m.source_repo {
                    e.insert("source_repo".to_string(), JsonValue::String(r.clone()));
                }
                let strings = |v: &[String]| {
                    JsonValue::Array(v.iter().cloned().map(JsonValue::String).collect())
                };
                if !m.description.is_empty() {
                    e.insert(
                        "description".to_string(),
                        JsonValue::String(m.description.clone()),
                    );
                    e.insert("author".to_string(), JsonValue::String(m.author.clone()));
                    e.insert("tags".to_string(), strings(&m.tags));
                    e.insert("screenshots".to_string(), strings(&m.screenshots));
                }
                if !m.commands.is_empty() {
                    e.insert(
                        "commands".to_string(),
                        JsonValue::Array(m.commands.iter().map(|c| c.to_json()).collect()),
                    );
                }
                if let Some(p) = &m.preview {
                    e.insert("preview".to_string(), JsonValue::String(p.clone()));
                }
                JsonValue::Object(e)
            })
            .collect();
        o.insert("mods".to_string(), JsonValue::Array(mods));
        JsonValue::Object(o)
    }
}

/// The set plus what was wrong with it.
#[derive(Debug)]
pub struct Built {
    pub set: ModSet,
    /// Declared mods the mods directory cannot back (missing, or a different sha256): clients would fail to join.
    pub errors: Vec<String>,
    /// Mod folders present but not declared (they load locally but are not part of the advertised set).
    pub warnings: Vec<String>,
    /// MOD-27: the operator pictures by their own SHA-256.
    pub previews: BTreeMap<String, Vec<u8>>,
}

/// The operator's picture of mod `id`: `<dir>/<id>.preview.png`, read at start, at most 1 MiB, PNG by its signature.
/// `Ok(None)`: no such file. Never part of a package or a hash; the operator's own file on their own server.
// DC-DEFAULT: MOD-27 (proposed, USER REQUEST 2026-11-01), see DC_DEFAULTS.md
pub fn read_preview(dir: &Path, id: &str) -> Result<Option<Vec<u8>>, String> {
    if !dc_mod_lua::is_valid_mod_id(id) {
        return Ok(None);
    }
    let file = dir.join(format!("{id}{PREVIEW_SUFFIX}"));
    if !file.is_file() {
        return Ok(None);
    }
    let len = std::fs::metadata(&file)
        .map_err(|e| format!("{}: {e}", file.display()))?
        .len();
    if len > dc_mod_lua::MAX_SCREENSHOT_BYTES {
        return Err(format!(
            "{}: {len} bytes is over the 1 MiB cap",
            file.display()
        ));
    }
    let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    if !bytes.starts_with(PNG_SIGNATURE) {
        return Err(format!("{}: not a PNG", file.display()));
    }
    Ok(Some(bytes))
}

/// Folders of `dir` that hold a `manifest.json`.
pub fn mod_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.join("manifest.json").is_file())
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort();
    v
}

/// Builds the advertised set from the config and the mods directory.
pub fn build(conf: &ServerConf, mods_dir: Option<&Path>) -> Built {
    let m = &conf.cfg.mods;
    if m.policy != "modded" {
        return Built {
            set: ModSet::vanilla(),
            errors: Vec::new(),
            warnings: Vec::new(),
            previews: BTreeMap::new(),
        };
    }
    let mut packed: Vec<(String, dc_modkit::ModSetEntry)> = Vec::new();
    let mut errors = Vec::new();
    if let Some(dir) = mods_dir {
        for d in mod_dirs(dir) {
            match dc_modkit::pack_mod_bytes(&d) {
                Ok((_, e)) => packed.push((
                    d.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    e,
                )),
                Err(errs) => {
                    for e in errs {
                        errors.push(format!("{}: {}", d.display(), e));
                    }
                }
            }
        }
    }
    let mut mods = Vec::new();
    let mut previews = BTreeMap::new();
    let mut picture_notes: Vec<String> = Vec::new();
    for r in &m.set {
        let hit = packed.iter().find(|(_, e)| e.id == r.id);
        let zip = mods_dir
            .map(|d| d.join(format!("{}.zip", r.sha256)))
            .filter(|z| z.is_file());
        let mut entry = ModEntry {
            id: r.id.clone(),
            version: r.version.clone(),
            sha256: r.sha256.clone(),
            kind: None,
            requires_client: false,
            capabilities: Vec::new(),
            source_repo: None,
            description: String::new(),
            author: String::new(),
            tags: Vec::new(),
            screenshots: Vec::new(),
            commands: Vec::new(),
            preview: None,
        };
        match read_preview_of(mods_dir, &r.id) {
            Ok(Some(b)) => {
                let sha = dc_rom::sha256_hex(&b);
                entry.preview = Some(sha.clone());
                previews.insert(sha, b);
            }
            Ok(None) => {}
            Err(why) => picture_notes.push(format!("preview picture left out: {why}")),
        }
        match hit {
            Some((_, e)) => {
                if e.sha256 != r.sha256 && zip.is_none() {
                    errors.push(format!("mod '{}': declared sha256 {} but the folder in the mods directory packs to {}", r.id, r.sha256, e.sha256));
                }
                if e.version.to_string() != r.version {
                    errors.push(format!(
                        "mod '{}': declared version {} but the manifest says {}",
                        r.id, r.version, e.version
                    ));
                }
                entry.kind = Some(e.kind.as_str().to_string());
                entry.requires_client = e.requires_client;
                entry.capabilities = e.capabilities.clone();
                entry.source_repo = e.source_repo.clone();
                entry.description = e.description.clone();
                entry.author = e.author.clone();
                entry.tags = e.tags.clone();
                entry.screenshots = e.screenshots.clone();
                entry.commands = e.commands.clone();
            }
            // only the zip is there: its manifest completes the entry
            None if zip.is_some() => {
                if let Some(man) = zip.as_deref().and_then(zip_manifest) {
                    entry.kind = Some(man.kind.as_str().to_string());
                    entry.requires_client = man.requires_client;
                    entry.capabilities = man.capabilities;
                    entry.source_repo = man.github;
                    entry.description = man.description;
                    entry.author = man.author;
                    entry.tags = man.tags;
                    entry.screenshots = man.screenshots;
                    entry.commands = man.commands;
                }
            }
            None => errors.push(format!(
                "mod '{}': neither a folder with that id nor {}.zip is in the mods directory",
                r.id, r.sha256
            )),
        }
        mods.push(entry);
    }
    let mut warnings: Vec<String> = packed
        .iter()
        .filter(|(_, e)| !m.set.iter().any(|r| r.id == e.id))
        .map(|(name, e)| {
            format!(
                "mods folder '{}' (mod '{}') is not in [[mods.set]]; it is not advertised",
                name, e.id
            )
        })
        .collect();
    warnings.append(&mut picture_notes);
    Built {
        set: ModSet {
            modded: true,
            client_visual_allowed: m.client_visual_allowed,
            mods,
        },
        errors,
        warnings,
        previews,
    }
}

fn read_preview_of(dir: Option<&Path>, id: &str) -> Result<Option<Vec<u8>>, String> {
    dir.map_or(Ok(None), |d| read_preview(d, id))
}

/// The manifest inside a packed mod zip, when it reads and parses.
fn zip_manifest(zip: &Path) -> Option<dc_mod_lua::ModManifest> {
    let bytes = std::fs::read(zip).ok()?;
    let files = dc_modkit::read_stored_zip(&bytes).ok()?;
    let (_, m) = files.into_iter().find(|(n, _)| n == "manifest.json")?;
    dc_mod_lua::ModManifest::from_json_str(std::str::from_utf8(&m).ok()?).ok()
}

/// Where a declared mod is loaded from (LG-270).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub id: String,
    /// The unpacked zip under the cache, or the mod folder whose pack hashes to the declared sha256.
    pub dir: PathBuf,
}

/// MOD-25: the lobby's optional cosmetic mods as served: the document of `/mods/catalog.json`, the first screenshot of
/// each by package hash, and a log line per entry left out.
#[derive(Debug, Default)]
pub struct Catalog {
    pub doc: Option<String>,
    pub shots: BTreeMap<String, Vec<u8>>,
    pub notes: Vec<String>,
}

struct CatalogRow {
    name: String,
    author: String,
    description: String,
    size: u64,
    shot: Option<Vec<u8>>,
    commands: Vec<dc_mod_lua::ModCommand>,
}

fn catalog_row(
    r: &dc_config::schema::ModRef,
    dir: Option<&Path>,
    cap: u64,
) -> Result<CatalogRow, String> {
    let dir = dir.ok_or("no mods dir")?;
    let zip = dir.join(format!("{}.zip", r.sha256));
    let bytes = std::fs::read(&zip).map_err(|e| format!("{}: {e}", zip.display()))?;
    if bytes.len() as u64 > cap {
        return Err(format!("{} bytes is over mods.max_zip_bytes", bytes.len()));
    }
    if dc_rom::sha256_hex(&bytes) != r.sha256 {
        return Err("the zip does not match its sha256".to_string());
    }
    let files = dc_modkit::read_stored_zip(&bytes)?;
    let text = files
        .iter()
        .find(|(n, _)| n == "manifest.json")
        .map(|(_, d)| String::from_utf8_lossy(d).into_owned())
        .ok_or("the zip has no manifest.json")?;
    let m = dc_mod_lua::ModManifest::from_json_str(&text)?;
    if m.id != r.id || m.version.to_string() != r.version {
        return Err(format!("the zip holds '{}@{}'", m.id, m.version));
    }
    if m.kind != dc_mod_lua::ModKind::ClientVisual {
        return Err(format!("kind '{}' is not client_visual", m.kind));
    }
    let shot = m
        .screenshots
        .iter()
        .find(|p| p.to_ascii_lowercase().ends_with(".png"))
        .and_then(|p| files.iter().find(|(n, _)| n == p).map(|(_, d)| d.clone()));
    Ok(CatalogRow {
        name: m.name,
        author: m.author,
        description: m.description,
        size: bytes.len() as u64,
        shot,
        commands: m.commands,
    })
}

/// Builds the catalogue from `[[mods.catalog]]`: each entry needs `<mods_dir>/<sha256>.zip` with that hash, the
/// declared id and version, and kind `client_visual` by its manifest; any other is left out with a note and the
/// start goes on. Nothing is unpacked or loaded. `doc` stays `None` without an offered mod.
// DC-DEFAULT: MOD-25 (proposed, project decision) optional mods, not part of the set or its hash
pub fn build_catalog(
    list: &[dc_config::schema::ModRef],
    mods_dir: Option<&Path>,
    cap: u64,
) -> Catalog {
    let mut out = Catalog::default();
    let s = |t: &str| JsonValue::String(t.to_string());
    let mut rows = Vec::new();
    for r in list {
        match catalog_row(r, mods_dir, cap) {
            Ok(row) => {
                let mut e = BTreeMap::new();
                e.insert("id".to_string(), s(&r.id));
                e.insert("version".to_string(), s(&r.version));
                e.insert("sha256".to_string(), s(&r.sha256));
                e.insert("name".to_string(), s(&row.name));
                e.insert("author".to_string(), s(&row.author));
                e.insert("description".to_string(), s(&row.description));
                e.insert("size".to_string(), JsonValue::Number(row.size as f64));
                e.insert(
                    "screenshot".to_string(),
                    JsonValue::Bool(row.shot.is_some()),
                );
                if let Some(b) = row.shot {
                    out.shots.insert(r.sha256.clone(), b);
                }
                if !row.commands.is_empty() {
                    e.insert(
                        "commands".to_string(),
                        JsonValue::Array(row.commands.iter().map(|c| c.to_json()).collect()),
                    );
                }
                // DC-DEFAULT: MOD-27 (proposed) the operator's picture, by its own hash
                match read_preview_of(mods_dir, &r.id) {
                    Ok(Some(b)) => {
                        let sha = dc_rom::sha256_hex(&b);
                        e.insert("preview".to_string(), s(&sha));
                        out.shots.insert(sha, b);
                    }
                    Ok(None) => {}
                    Err(why) => out
                        .notes
                        .push(format!("'{}': preview picture left out: {why}", r.id)),
                }
                rows.push(JsonValue::Object(e));
            }
            Err(why) => out.notes.push(format!("'{}' left out: {why}", r.id)),
        }
    }
    if !rows.is_empty() {
        let mut d = BTreeMap::new();
        d.insert("version".to_string(), JsonValue::Number(1.0));
        d.insert("mods".to_string(), JsonValue::Array(rows));
        out.doc = Some(JsonValue::Object(d).to_json_pretty());
    }
    out
}

/// Resolves the declared set (`[[mods.set]]`, in its order) to the folders the lobby loads.
/// Per entry: `<mods_dir>/<sha256>.zip` (the file `/mods/<sha256>.zip` serves) when present, its SHA-256 checked
/// and unpacked into `<cache>/<sha256>/`; otherwise the folder `<mods_dir>/<id>/` when its deterministic pack
/// hashes to the declared sha256. The unpacked or found manifest must carry the declared id and version.
/// Any entry that cannot be backed is an error and the caller refuses to start.
// DC-DEFAULT: LG-270, the lobby loads only its declared mod set, see DC_DEFAULTS.md
pub fn resolve_declared(
    conf: &ServerConf,
    mods_dir: Option<&Path>,
    cache: &Path,
) -> Result<Vec<Resolved>, Vec<String>> {
    let m = &conf.cfg.mods;
    if m.policy != "modded" {
        return Ok(Vec::new());
    }
    let Some(dir) = mods_dir else {
        return Err(vec![
            "a modded server.toml needs a mods dir (--mods-dir / paths.mods_dir)".to_string(),
        ]);
    };
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for r in &m.set {
        match resolve_one(dir, cache, &r.id, &r.version, &r.sha256) {
            Ok(d) => out.push(Resolved {
                id: r.id.clone(),
                dir: d,
            }),
            Err(e) => errors.push(format!("mod '{}': {}", r.id, e)),
        }
    }
    if errors.is_empty() {
        prune_cache(cache, m.set.iter().map(|r| r.sha256.as_str()));
        Ok(out)
    } else {
        Err(errors)
    }
}

/// Marker written into a cache folder after its zip unpacked completely (LG-270).
const CACHE_COMPLETE: &str = ".complete";

/// LG-270: removes the cache folders whose hash is not in the declared set.
fn prune_cache<'a>(cache: &Path, keep: impl Iterator<Item = &'a str>) {
    let keep: Vec<&str> = keep.collect();
    let Ok(rd) = std::fs::read_dir(cache) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if e.path().is_dir() && !keep.contains(&name.as_str()) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

fn resolve_one(
    dir: &Path,
    cache: &Path,
    id: &str,
    version: &str,
    sha256: &str,
) -> Result<PathBuf, String> {
    let zip = dir.join(format!("{}.zip", sha256));
    let folder = dir.join(id);
    let root = if zip.is_file() {
        let bytes =
            std::fs::read(&zip).map_err(|e| format!("cannot read {}: {}", zip.display(), e))?;
        let got = dc_rom::sha256_hex(&bytes);
        if got != sha256 {
            return Err(format!(
                "declared sha256 {} but {} hashes to {}",
                sha256,
                zip.display(),
                got
            ));
        }
        let files =
            dc_modkit::read_stored_zip(&bytes).map_err(|e| format!("{}: {}", zip.display(), e))?;
        let out = cache.join(sha256);
        // LG-270: a folder with the marker was unpacked completely from these very bytes (its name is their hash)
        if !out.join(CACHE_COMPLETE).is_file() {
            let _ = std::fs::remove_dir_all(&out);
            for (name, data) in files {
                let p = out.join(&name);
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("cannot create {}: {}", parent.display(), e))?;
                }
                std::fs::write(&p, data)
                    .map_err(|e| format!("cannot write {}: {}", p.display(), e))?;
            }
            let m = out.join(CACHE_COMPLETE);
            std::fs::write(&m, b"").map_err(|e| format!("cannot write {}: {}", m.display(), e))?;
        }
        out
    } else if folder.join("manifest.json").is_file() {
        let (_, e) = dc_modkit::pack_mod_bytes(&folder)
            .map_err(|errs| format!("{}: {}", folder.display(), errs.join("; ")))?;
        if e.sha256 != sha256 {
            return Err(format!(
                "declared sha256 {} but the folder {} packs to {}",
                sha256,
                folder.display(),
                e.sha256
            ));
        }
        folder
    } else {
        return Err(format!(
            "neither {}.zip nor a folder '{}' is in the mods directory {}",
            sha256,
            id,
            dir.display()
        ));
    };
    let man = dc_mod_lua::ModManifest::from_file(&root.join("manifest.json"))?;
    if man.id != id {
        return Err(format!("the package's manifest names mod '{}'", man.id));
    }
    if man.version.to_string() != version {
        return Err(format!(
            "declared version {} but the manifest says {}",
            version, man.version
        ));
    }
    Ok(root)
}

/// LG-313: one featured entry's curated set: what the clients must carry and its own mod host.
pub struct EntrySet {
    pub entry: u32,
    pub set: ModSet,
    pub expected: crate::state::ExpectedMods,
    pub client: crate::modhost::ModClient,
    /// The document of `/entry_mods/<entry>.json`.
    pub doc: String,
    /// The folder of the set's `<sha256>.zip` files (a remote host gets its map pack from there, MAP-83).
    pub mods_dir: PathBuf,
}

impl EntrySet {
    /// The set's row of the 0x3f10 push.
    pub fn hashes(&self) -> dc_wire::entrymods::EntryHashes {
        dc_wire::entrymods::EntryHashes {
            entry: self.entry,
            set: self.expected.set.clone(),
            content: self.expected.content.clone(),
        }
    }

    /// MAP-83: the zip behind mod `id` of this set: its hash and its file.
    pub fn pack_of(&self, id: &str) -> Option<(&str, PathBuf)> {
        let m = self.set.mods.iter().find(|m| m.id == id)?;
        Some((
            m.sha256.as_str(),
            self.mods_dir.join(format!("{}.zip", m.sha256)),
        ))
    }
}

fn curated_entry(r: &dc_config::schema::ModRef, m: dc_mod_lua::ModManifest) -> ModEntry {
    ModEntry {
        id: r.id.clone(),
        version: r.version.clone(),
        sha256: r.sha256.clone(),
        kind: Some(m.kind.as_str().to_string()),
        requires_client: m.requires_client,
        capabilities: m.capabilities,
        source_repo: m.github,
        description: m.description,
        author: m.author,
        tags: m.tags,
        screenshots: m.screenshots,
        commands: m.commands,
        preview: None,
    }
}

/// LG-313: the document of `/entry_mods/<entry>.json`, in the form of `server_list_json`. MAP-81: with the map index
/// of the set's first community map (`community_map_base`, the same for every entry, MAP-35) and the set's map records
/// (`maps`, in table order: index = base + position), so the client lists them without its own ROM agreeing.
// DC-DEFAULT: MAP-81 (proposed)
pub fn entry_set_json(
    entry: u32,
    name: &str,
    set: &ModSet,
    community_map_base: u32,
    maps: &[crate::maps::CommunityMap],
) -> String {
    let mut o = BTreeMap::new();
    o.insert(
        "community_map_base".to_string(),
        JsonValue::Number(f64::from(community_map_base)),
    );
    o.insert(
        "maps".to_string(),
        JsonValue::Array(
            maps.iter()
                .map(|m| crate::maps::map_record_json(&m.record))
                .collect(),
        ),
    );
    o.insert("entry".to_string(), JsonValue::Number(f64::from(entry)));
    o.insert("name".to_string(), JsonValue::String(name.to_string()));
    o.insert("curated".to_string(), JsonValue::Bool(true));
    o.insert("mod_set".to_string(), set.fragment());
    o.insert("mod_set_hash".to_string(), JsonValue::String(set.hash()));
    JsonValue::Object(o).to_json_pretty()
}

/// LG-313: the curated sets of `servers` (entry id = position + 1, as `servers::entries`), each loaded in its own mod
/// host, with its unpacked packs in `<data>/entry_mods/cache` and its content document in `<data>/entry_mods/<entry>`.
/// A pack that is missing, over `cap` bytes, hashes wrong, holds another id or version, or is refused by the curated
/// limit is logged and left out; an entry with no pack left stays a plain entry. The server always starts.
// DC-DEFAULT: LG-313 (proposed, project decision)
pub fn build_entry_sets(
    servers: &[dc_config::LobbyServerCfg],
    mods_dir: Option<&Path>,
    data: &Path,
    cap: u64,
    community_map_base: u32,
    log: &std::sync::Arc<crate::logs::Logger>,
) -> BTreeMap<u32, EntrySet> {
    let mut out = BTreeMap::new();
    let root = data.join("entry_mods");
    let cache = root.join("cache");
    prune_cache(
        &cache,
        servers
            .iter()
            .flat_map(|e| e.mods.iter())
            .map(|r| r.sha256.as_str()),
    );
    for (i, e) in servers
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.mods.is_empty())
    {
        let entry = i as u32 + 1;
        let left_out = |id: &str, why: &str| {
            log.line(&format!(
                "mods: entry {entry}: curated pack '{id}' left out: {why}"
            ))
        };
        let Some(dir) = mods_dir else {
            log.line(&format!(
                "mods: ERROR entry {entry} declares packs but there is no mods dir; it stays plain"
            ));
            continue;
        };
        let mut found = Vec::new();
        for r in &e.mods {
            let zip = dir.join(format!("{}.zip", r.sha256));
            match std::fs::metadata(&zip) {
                Ok(m) if m.len() > cap => {
                    left_out(
                        &r.id,
                        &format!("{} bytes is over mods.max_zip_bytes", m.len()),
                    );
                    continue;
                }
                Ok(_) => {}
                Err(_) => {
                    left_out(
                        &r.id,
                        &format!("{} is not in the mods directory", zip.display()),
                    );
                    continue;
                }
            }
            let got = resolve_one(dir, &cache, &r.id, &r.version, &r.sha256).and_then(|d| {
                dc_mod_lua::ModManifest::from_file(&d.join("manifest.json")).map(|m| (d, m))
            });
            match got {
                Ok((d, m)) => found.push((
                    r,
                    Resolved {
                        id: r.id.clone(),
                        dir: d,
                    },
                    m,
                )),
                Err(why) => left_out(&r.id, &why),
            }
        }
        let client = crate::modhost::ModClient::start_curated(
            found.iter().map(|(_, res, _)| res.clone()).collect(),
            std::sync::Arc::clone(log),
        );
        let mut client = match client {
            Ok(c) => c,
            Err(why) => {
                log.line(&format!("mods: ERROR entry {entry}: {why}; it stays plain"));
                continue;
            }
        };
        let mods: Vec<ModEntry> = found
            .into_iter()
            .filter(|(r, _, _)| client.loaded.contains(&r.id))
            .map(|(r, _, m)| curated_entry(r, m))
            .collect();
        if mods.is_empty() {
            log.line(&format!(
                "mods: entry {entry}: no curated pack loaded; it stays plain"
            ));
            continue;
        }
        if let Err(why) = client.write_content(&root.join(entry.to_string())) {
            log.line(&format!("mods: ERROR entry {entry}: {why}; it stays plain"));
            continue;
        }
        // LG-426: the entry's own switch lets the player's local client_visual mods load beside the set; it is no
        // part of the set hash or the 0x3f10 / 0x3f11 claim, and an older client refuses them under its curated policy
        // DC-DEFAULT: LG-426 (proposed)
        let set = ModSet {
            modded: true,
            client_visual_allowed: e.client_visual_allowed,
            mods,
        };
        let expected = crate::state::ExpectedMods {
            set: set.hash(),
            content: if client.client_content.is_empty() {
                dc_wire::loginmods::NO_CONTENT.to_string()
            } else {
                client.client_content.hash()
            },
        };
        log.line(&format!(
            "mods: entry {entry} ({}): {} curated pack(s) {:?}, set hash {}…",
            e.name,
            set.mods.len(),
            client.loaded,
            dc_wire::loginmods::short(&expected.set)
        ));
        let doc = entry_set_json(entry, &e.name, &set, community_map_base, &client.maps);
        out.insert(
            entry,
            EntrySet {
                entry,
                set,
                expected,
                client,
                doc,
                mods_dir: dir.to_path_buf(),
            },
        );
    }
    out
}

/// The server-list data document: the lobby entry facts plus the `mod_set` fragment and its hash, and
/// `community_map_base`, the map index of the set's first community map (`MapTable::first_community_index`,
/// MAP-35): the client decodes a community index from it instead of from its own ROM's row count, so the two
/// sides agree even when the lobby runs without a ROM (its base is then dc_bute's 12 synthetic rows).
// DC-DEFAULT: MAP-35 (proposed) the base index is published, not derived twice
pub fn server_list_json(
    conf: &ServerConf,
    opts: &crate::cli::Options,
    set: &ModSet,
    community_map_base: u32,
) -> String {
    let mut o = BTreeMap::new();
    o.insert(
        "community_map_base".to_string(),
        JsonValue::Number(community_map_base as f64),
    );
    o.insert(
        "name".to_string(),
        JsonValue::String(conf.cfg.server.name.clone()),
    );
    o.insert(
        "address".to_string(),
        JsonValue::String(opts.lobbyip.clone()),
    );
    let mut ports = BTreeMap::new();
    ports.insert("auth".to_string(), JsonValue::Number(opts.port as f64));
    ports.insert(
        "lobby".to_string(),
        JsonValue::Number(opts.lobbyport as f64),
    );
    ports.insert("clan".to_string(), JsonValue::Number(opts.clanport as f64));
    ports.insert("http".to_string(), JsonValue::Number(opts.clanweb as f64));
    o.insert("ports".to_string(), JsonValue::Object(ports));
    // MOD-22: false when a loaded mod declares world.full_visibility (a browser flags the server)
    o.insert(
        "culling".to_string(),
        JsonValue::Bool(set.full_visibility_mod().is_none()),
    );
    o.insert("mod_set".to_string(), set.fragment());
    o.insert("mod_set_hash".to_string(), JsonValue::String(set.hash()));
    JsonValue::Object(o).to_json_pretty()
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY_HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn png() -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
        b.extend_from_slice(b"picture");
        b
    }

    /// MOD-27: the operator's picture is read only as a PNG within the cap, flagged in the set's entry by its own hash,
    /// served on the picture route, and it changes no hash and no package.
    #[test]
    fn the_operators_picture_is_flagged_and_served_and_changes_no_hash() {
        let d = std::env::temp_dir().join(format!("dc_lobby_preview_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods/hacking_mod");
        let (zip, e) = dc_modkit::pack_mod_bytes(&src).unwrap();
        std::fs::write(d.join(format!("{}.zip", e.sha256)), &zip).unwrap();
        let toml = format!(
            "[mods]
policy = \"modded\"
[[mods.set]]
id = \"hacking_mod\"
version = \"1.1.0\"
sha256 = \"{}\"
",
            e.sha256
        );
        let p = d.join("server.toml");
        std::fs::write(&p, toml).unwrap();
        let conf = ServerConf::load_unchecked(Some(&p), &[]).unwrap().unwrap();
        let served = |b: &Built, sha: &str| {
            let mut h = crate::hosting::Hosting::none();
            h.catalog_shots = b.previews.clone();
            crate::hosting::route(&h, &format!("/mods/{sha}.png"))
                .unwrap()
                .status
        };

        let plain = build(&conf, Some(&d));
        assert_eq!(plain.set.mods[0].preview, None);
        assert!(!plain.set.fragment().to_json_string().contains("preview"));
        assert_eq!(plain.set.mods[0].commands[0].command, "/wallhack");
        assert!(plain.set.fragment().to_json_string().contains("/wallhack"));

        // a real picture
        let file = d.join("hacking_mod.preview.png");
        std::fs::write(&file, png()).unwrap();
        let with = build(&conf, Some(&d));
        let sha = dc_rom::sha256_hex(&png());
        assert_eq!(with.set.mods[0].preview.as_deref(), Some(sha.as_str()));
        assert!(with
            .set
            .fragment()
            .to_json_string()
            .contains(&format!("\"preview\":\"{sha}\"")));
        assert_eq!(served(&with, &sha), 200);
        assert_eq!(with.set.hash(), plain.set.hash());
        assert!(with.warnings.is_empty(), "{:?}", with.warnings);

        // not a PNG, or over the cap: left out with a log line, nothing else changes
        for bad in [
            b"not a png".to_vec(),
            [png(), vec![0; 1024 * 1024]].concat(),
        ] {
            std::fs::write(&file, &bad).unwrap();
            let b = build(&conf, Some(&d));
            assert_eq!(b.set.mods[0].preview, None);
            assert!(b.previews.is_empty());
            assert_eq!(served(&b, &dc_rom::sha256_hex(&bad)), 404);
            assert!(
                b.warnings
                    .iter()
                    .any(|w| w.contains("preview picture left out")),
                "{:?}",
                b.warnings
            );
            assert!(b.errors.is_empty(), "{:?}", b.errors);
            assert_eq!(b.set.hash(), plain.set.hash());
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// MOD-27: a catalogue entry carries its picture and command help the same way.
    #[test]
    fn a_catalogue_entry_carries_the_operators_picture() {
        let d =
            std::env::temp_dir().join(format!("dc_lobby_catalog_preview_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods/drawn_crosshair");
        let (zip, e) = dc_modkit::pack_mod_bytes(&src).unwrap();
        std::fs::write(d.join(format!("{}.zip", e.sha256)), &zip).unwrap();
        let r = dc_config::schema::ModRef {
            id: e.id.clone(),
            version: e.version.to_string(),
            sha256: e.sha256.clone(),
        };
        let before = build_catalog(std::slice::from_ref(&r), Some(&d), 1 << 24);
        assert!(!before.doc.unwrap().contains("preview"));
        std::fs::write(d.join("drawn_crosshair.preview.png"), png()).unwrap();
        let c = build_catalog(std::slice::from_ref(&r), Some(&d), 1 << 24);
        let sha = dc_rom::sha256_hex(&png());
        assert!(c.doc.unwrap().contains(&format!("\"preview\": \"{sha}\"")));
        assert_eq!(c.shots.get(&sha), Some(&png()));
        assert!(
            c.shots.contains_key(&e.sha256),
            "the package's own picture stays"
        );
        std::fs::write(d.join("drawn_crosshair.preview.png"), b"junk").unwrap();
        let c = build_catalog(std::slice::from_ref(&r), Some(&d), 1 << 24);
        assert!(!c.doc.unwrap().contains("preview\""));
        assert!(
            c.notes
                .iter()
                .any(|n| n.contains("preview picture left out")),
            "{:?}",
            c.notes
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// MOD-25: only a package that hashes right, holds the declared id and is client_visual is offered.
    #[test]
    fn the_catalogue_offers_only_verified_client_visual_packages() {
        let d = std::env::temp_dir().join(format!("dc_lobby_catalog_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods");
        std::fs::create_dir_all(&d).unwrap();
        let pack = |name: &str| {
            let (bytes, e) = dc_modkit::pack_mod_bytes(&src.join(name)).unwrap();
            std::fs::write(d.join(format!("{}.zip", e.sha256)), &bytes).unwrap();
            dc_config::schema::ModRef {
                id: e.id.clone(),
                version: e.version.to_string(),
                sha256: e.sha256.clone(),
            }
        };
        let good = pack("drawn_crosshair");
        let shared = pack("hacking_mod");
        let mut wrong_hash = good.clone();
        wrong_hash.sha256 = "00".repeat(32);
        let c = build_catalog(&[good.clone(), shared, wrong_hash], Some(&d), 1 << 24);
        let doc = c.doc.unwrap();
        assert!(
            doc.contains("Custom Crosshair") && !doc.contains("hacking_mod"),
            "{doc}"
        );
        assert_eq!(c.notes.len(), 2, "{:?}", c.notes);
        assert!(
            c.notes.iter().any(|n| n.contains("not client_visual")),
            "{:?}",
            c.notes
        );
        assert!(c.shots.contains_key(&good.sha256));
        assert!(build_catalog(&[], Some(&d), 1 << 24).doc.is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    fn entry(id: &str, v: &str, sha: &str, kind: Option<&str>) -> ModEntry {
        ModEntry {
            id: id.into(),
            version: v.into(),
            sha256: sha.into(),
            kind: kind.map(str::to_string),
            requires_client: false,
            capabilities: Vec::new(),
            source_repo: None,
            description: String::new(),
            author: String::new(),
            tags: Vec::new(),
            screenshots: Vec::new(),
            commands: Vec::new(),
            preview: None,
        }
    }

    fn write_mod(dir: &Path, id: &str) {
        let m = dir.join(id);
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(
            m.join("manifest.json"),
            format!(
                r#"{{"id": "{id}", "name": "M", "version": "1.0.0", "kind": "server_rules", "description": "A test fixture mod for the unit tests.", "author": "tests",
                   "capabilities": ["hooks.lobby"]}}"#
            ),
        )
        .unwrap();
        std::fs::write(
            m.join("main.lua"),
            "return function(mod)\n    mod.hooks:wrap(\"lobby.calc_exp_gp\", function(base, ctx)\n        return base(ctx)\n    end)\nend\n",
        )
        .unwrap();
    }

    fn conf_with(dir: &Path, set: &[(&str, &str)]) -> ServerConf {
        let mut t = String::from("[mods]\npolicy = \"modded\"\n");
        for (id, sha) in set {
            t.push_str(&format!(
                "[[mods.set]]\nid = \"{id}\"\nversion = \"1.0.0\"\nsha256 = \"{sha}\"\n"
            ));
        }
        let p = dir.join("server.toml");
        std::fs::write(&p, t).unwrap();
        ServerConf::load_unchecked(Some(&p), &[]).unwrap().unwrap()
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc_modset_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn only_the_declared_mods_load_from_their_zip_in_order() {
        let d = tmp("declared");
        let mods = d.join("mods");
        for id in ["b_mod", "a_mod", "stray"] {
            write_mod(&mods, id);
        }
        let mut set = Vec::new();
        for id in ["b_mod", "a_mod"] {
            let (bytes, e) = dc_modkit::pack_mod_bytes(&mods.join(id)).unwrap();
            std::fs::write(mods.join(format!("{}.zip", e.sha256)), bytes).unwrap();
            set.push((id, e.sha256));
        }
        // the zip is what loads: the folder may go
        std::fs::remove_dir_all(mods.join("b_mod")).unwrap();
        let pairs: Vec<(&str, &str)> = set.iter().map(|(i, s)| (*i, s.as_str())).collect();
        let conf = conf_with(&d, &pairs);
        let cache = d.join("cache");
        let r = resolve_declared(&conf, Some(&mods), &cache).unwrap();
        assert_eq!(
            r.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["b_mod", "a_mod"]
        );
        assert!(r.iter().all(|m| m.dir.starts_with(&cache)), "{r:?}");
        let c = crate::modhost::ModClient::start_declared(
            r,
            std::sync::Arc::new(crate::logs::Logger::none()),
            Some(conf.mod_policy()),
        )
        .unwrap();
        assert_eq!(c.loaded, vec!["b_mod".to_string(), "a_mod".to_string()]);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn the_cache_keeps_complete_folders_and_drops_stale_ones() {
        let d = tmp("prune");
        let mods = d.join("mods");
        write_mod(&mods, "m1");
        let (bytes, e) = dc_modkit::pack_mod_bytes(&mods.join("m1")).unwrap();
        std::fs::write(mods.join(format!("{}.zip", e.sha256)), bytes).unwrap();
        let conf = conf_with(&d, &[("m1", &e.sha256)]);
        let cache = d.join("cache");
        let stale = cache.join("f".repeat(64));
        std::fs::create_dir_all(&stale).unwrap();
        resolve_declared(&conf, Some(&mods), &cache).unwrap();
        assert!(!stale.exists());
        let kept = cache.join(&e.sha256);
        assert!(kept.join(".complete").is_file());
        // a complete folder is not rewritten
        std::fs::write(kept.join("sentinel"), b"x").unwrap();
        resolve_declared(&conf, Some(&mods), &cache).unwrap();
        assert!(kept.join("sentinel").is_file());
        // without the marker it is unpacked again
        std::fs::remove_file(kept.join(".complete")).unwrap();
        resolve_declared(&conf, Some(&mods), &cache).unwrap();
        assert!(!kept.join("sentinel").exists() && kept.join(".complete").is_file());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_folder_loads_only_when_its_pack_has_the_declared_hash() {
        let d = tmp("folder");
        let mods = d.join("mods");
        write_mod(&mods, "m1");
        let (_, e) = dc_modkit::pack_mod_bytes(&mods.join("m1")).unwrap();
        let ok = conf_with(&d, &[("m1", &e.sha256)]);
        let r = resolve_declared(&ok, Some(&mods), &d.join("cache")).unwrap();
        assert_eq!(r[0].dir, mods.join("m1"));
        let bad = conf_with(&d, &[("m1", &"0".repeat(64))]);
        let errs = resolve_declared(&bad, Some(&mods), &d.join("cache")).unwrap_err();
        assert!(errs[0].contains("packs to"), "{errs:?}");
        // a zip whose bytes do not hash to its name is refused too
        std::fs::write(mods.join(format!("{}.zip", "1".repeat(64))), b"not it").unwrap();
        let swapped = conf_with(&d, &[("m1", &"1".repeat(64))]);
        let errs = resolve_declared(&swapped, Some(&mods), &d.join("cache")).unwrap_err();
        assert!(errs[0].contains("hashes to"), "{errs:?}");
        let missing = conf_with(&d, &[("gone", &e.sha256.replace('a', "b"))]);
        assert!(resolve_declared(&missing, Some(&mods), &d.join("cache")).is_err());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn the_server_list_entry_carries_the_browser_metadata() {
        let d = tmp("meta");
        let mods = d.join("mods");
        let m = mods.join("pretty");
        std::fs::create_dir_all(m.join("screenshots")).unwrap();
        std::fs::write(
            m.join("manifest.json"),
            r#"{"id": "pretty", "name": "Pretty", "version": "1.0.0", "kind": "shared",
               "description": "Shows the browser what this mod is about.", "author": "someone",
               "tags": ["hud", "fun"], "screenshots": ["screenshots/one.png"]}"#,
        )
        .unwrap();
        std::fs::write(m.join("main.lua"), "return function(mod) end\n").unwrap();
        std::fs::write(
            m.join("screenshots/one.png"),
            [&dc_modkit::placeholder_png()[..], b"x"].concat(),
        )
        .unwrap();
        let (bytes, e) = dc_modkit::pack_mod_bytes(&m).unwrap();
        // zip only: the entry is completed from the zip's manifest
        std::fs::write(mods.join(format!("{}.zip", e.sha256)), bytes).unwrap();
        std::fs::remove_dir_all(&m).unwrap();
        let conf = conf_with(&d, &[("pretty", &e.sha256)]);
        let built = build(&conf, Some(&mods));
        assert!(built.errors.is_empty(), "{:?}", built.errors);
        let j = built.set.fragment().to_json_string();
        for needle in [
            "\"description\":\"Shows the browser what this mod is about.\"",
            "\"author\":\"someone\"",
            "\"tags\":[\"hud\",\"fun\"]",
            "\"screenshots\":[\"screenshots/one.png\"]",
            "\"kind\":\"shared\"",
        ] {
            assert!(j.contains(needle), "{needle} not in {j}");
        }
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn empty_set_hashes_to_sha256_of_nothing() {
        let s = ModSet::vanilla();
        assert_eq!(s.hash_text(), "");
        assert_eq!(s.hash(), EMPTY_HASH);
        let f = s.fragment().to_json_string();
        assert!(
            f.contains("\"policy\":\"vanilla\"") && f.contains("\"mods\":[]"),
            "{f}"
        );
    }

    #[test]
    fn hash_covers_declared_kinds_in_order_and_skips_client_visual() {
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let c = "c".repeat(64);
        let s = ModSet {
            modded: true,
            client_visual_allowed: true,
            mods: vec![
                entry("rules", "1.0.0", &a, Some("server_rules")),
                entry("skins", "2.0.0", &c, Some("client_visual")),
                entry("hud", "0.3.1", &b, None),
            ],
        };
        let want = format!("rules@1.0.0:{a}\nhud@0.3.1:{b}\n");
        assert_eq!(s.hash_text(), want);
        assert_eq!(s.hash(), dc_rom::sha256_hex(want.as_bytes()));
        // order matters
        let mut r = s.clone();
        r.mods.swap(0, 2);
        assert_ne!(r.hash(), s.hash());
        // a client-visual mod alone leaves the empty-set hash
        let cv = ModSet {
            modded: true,
            client_visual_allowed: true,
            mods: vec![entry("skins", "2.0.0", &c, Some("client_visual"))],
        };
        assert_eq!(cv.hash(), EMPTY_HASH);
    }

    #[test]
    fn fragment_has_the_documented_shape() {
        let s = ModSet {
            modded: true,
            client_visual_allowed: true,
            mods: vec![entry("x", "1.2.3", &"d".repeat(64), Some("shared"))],
        };
        let j = s.fragment();
        let JsonValue::Object(o) = &j else { panic!() };
        assert_eq!(o["policy"].as_str(), Some("modded"));
        assert_eq!(o["client_visual_allowed"].as_bool(), Some(true));
        let JsonValue::Array(mods) = &o["mods"] else {
            panic!()
        };
        let JsonValue::Object(m) = &mods[0] else {
            panic!()
        };
        for k in [
            "id",
            "version",
            "sha256",
            "kind",
            "requires_client",
            "capabilities",
        ] {
            assert!(m.contains_key(k), "{k}");
        }
        assert!(!m.contains_key("source_repo"));
        // vanilla never advertises client_visual_allowed
        let v = ModSet {
            modded: false,
            client_visual_allowed: true,
            mods: Vec::new(),
        };
        let JsonValue::Object(o) = v.fragment() else {
            panic!()
        };
        assert_eq!(o["client_visual_allowed"].as_bool(), Some(false));
    }

    // MOD-22: world.full_visibility turns culling off: the dserver arg and the server-list flag follow it
    #[test]
    fn culling_follows_world_full_visibility() {
        let d = tmp("cull");
        let conf = conf_with(&d, &[]);
        let opts = crate::cli::parse_from(["dc-lobby-server"]).unwrap();
        let mut set = ModSet::vanilla();
        set.mods.push(entry(
            "plain",
            "1.0.0",
            &"a".repeat(64),
            Some("server_rules"),
        ));
        assert_eq!(set.full_visibility_mod(), None);
        assert!(server_list_json(&conf, &opts, &set, 19).contains("\"culling\": true"));
        let mut xray = entry("xray", "1.0.0", &"b".repeat(64), Some("shared"));
        xray.capabilities = vec!["hooks.render".into(), "world.full_visibility".into()];
        set.mods.push(xray);
        assert_eq!(set.full_visibility_mod(), Some("xray"));
        assert!(server_list_json(&conf, &opts, &set, 19).contains("\"culling\": false"));
        std::fs::remove_dir_all(&d).ok();
    }

    // C6 / MAP-35: the document names the first community index, so the client never derives it from its ROM
    #[test]
    fn the_server_list_carries_the_first_community_index() {
        let d = tmp("base");
        let conf = conf_with(&d, &[]);
        let opts = crate::cli::parse_from(["dc-lobby-server"]).unwrap();
        let set = ModSet::vanilla();
        let doc = server_list_json(&conf, &opts, &set, 12);
        assert!(doc.contains("\"community_map_base\": 12"), "{doc}");
        let doc = server_list_json(&conf, &opts, &set, 19);
        assert!(doc.contains("\"community_map_base\": 19"), "{doc}");
        std::fs::remove_dir_all(&d).ok();
    }
}
