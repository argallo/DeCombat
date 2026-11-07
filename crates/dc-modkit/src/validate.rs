//! `dc-modkit validate`: instantiate the real `dc_mod_lua::ModHost` on a copy of the mod and run its
//! entry. When a ROM path is supplied (the CLI passes `--rom` or `CA_ROM`), content patch ids are
//! also checked against the ROM's own tables, read through `dc_rez`/`dc_bute`. Without a ROM path
//! the ROM is never touched. A registered map's world (`mod.content.maps`) must exist under the mod's
//! `assets/` and parse as a version-85 world; that check needs no ROM (MAP-5).
// DOC: docs/MODDING_GUIDE.md section 9

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use dc_bute::{ShopCatalog, WeaponsTable};
use dc_mod_lua::{
    parse_chunk, Expr, ExprKind, MapRecord, ModHost, ModKind, ModManifest, SemVer, TableField,
    VersionReq, CONTENT_DOMAINS,
};
use dc_rez::RezVfs;

use crate::lint::walk_exprs;
use crate::ValidationReport;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A directory removed on drop.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new(prefix: &str) -> Result<Self, String> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "ca_modkit_{}_{}_{}_{}",
            prefix,
            std::process::id(),
            nanos,
            n
        ));
        fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create temp dir '{}': {}", dir.display(), e))?;
        Ok(Self(dir))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Copies regular files and directories; symlinks and special files are skipped.
fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("Failed to create '{}': {}", dst.display(), e))?;
    let entries = fs::read_dir(src)
        .map_err(|e| format!("Failed to read directory '{}': {}", src.display(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let ft = entry.file_type().map_err(|e| e.to_string())?;
        let target = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if ft.is_file() {
            fs::copy(entry.path(), &target)
                .map_err(|e| format!("Failed to copy '{}': {}", entry.path().display(), e))?;
        }
    }
    Ok(())
}

fn fail(mod_id: Option<String>, errors: Vec<String>) -> ValidationReport {
    ValidationReport {
        ok: false,
        mod_id,
        errors,
        warnings: Vec::new(),
    }
}

/// Smallest version that satisfies `req`, used for stand-in dependency mods.
fn stub_version(req: &VersionReq) -> SemVer {
    match req {
        VersionReq::Any => SemVer::new(1, 0, 0),
        VersionReq::Caret(v)
        | VersionReq::Tilde(v)
        | VersionReq::Ge(v)
        | VersionReq::Le(v)
        | VersionReq::Exact(v) => *v,
        VersionReq::Gt(v) => SemVer::new(v.major, v.minor, v.patch + 1),
        VersionReq::Lt(v) => {
            if v.patch > 0 {
                SemVer::new(v.major, v.minor, v.patch - 1)
            } else if v.minor > 0 {
                SemVer::new(v.major, v.minor - 1, 0)
            } else {
                SemVer::new(v.major.saturating_sub(1), 0, 0)
            }
        }
    }
}

/// A content call found in the mod's source whose record id is a literal.
#[derive(Debug, Clone)]
struct ContentRef {
    file: String,
    line: usize,
    domain: String,
    /// `patch` or `register`.
    verb: &'static str,
    id: i64,
}

fn literal_id(e: &Expr) -> Option<i64> {
    match &e.kind {
        ExprKind::Number(n) if n.fract() == 0.0 && n.abs() < 1e15 => Some(*n as i64),
        ExprKind::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// `mod.content.<domain>` (the base of a content verb), returning the domain.
fn content_domain(e: &Expr) -> Option<&str> {
    if let ExprKind::Field { expr, field } = &e.kind {
        if CONTENT_DOMAINS.contains(&field.as_str()) {
            if let ExprKind::Field { field: f2, .. } = &expr.kind {
                if f2 == "content" {
                    return Some(field.as_str());
                }
            }
        }
    }
    None
}

fn scan_content_refs(rel: &str, source: &str, out: &mut Vec<ContentRef>) {
    let Ok(stmts) = parse_chunk(source) else {
        return; // `lint` reports unparsable files
    };
    walk_exprs(&stmts, &mut |e| {
        let (domain, verb, args): (&str, &str, &[Expr]) = match &e.kind {
            ExprKind::MethodCall {
                receiver,
                method,
                args,
            } => match content_domain(receiver) {
                Some(d) => (d, method.as_str(), args.as_slice()),
                None => return,
            },
            ExprKind::Call { func, args } => match &func.kind {
                ExprKind::Field { expr, field } => match content_domain(expr) {
                    Some(d) => (d, field.as_str(), args.as_slice()),
                    None => return,
                },
                _ => return,
            },
            _ => return,
        };
        match verb {
            "patch" => {
                if let Some(id) = args.first().and_then(literal_id) {
                    out.push(ContentRef {
                        file: rel.to_string(),
                        line: e.line,
                        domain: domain.to_string(),
                        verb: "patch",
                        id,
                    });
                }
            }
            "register" => {
                if let Some(Expr {
                    kind: ExprKind::TableCons(fields),
                    ..
                }) = args.first()
                {
                    for f in fields {
                        if let TableField::Named(k, v) = f {
                            if k == "id" {
                                if let Some(id) = literal_id(v) {
                                    out.push(ContentRef {
                                        file: rel.to_string(),
                                        line: e.line,
                                        domain: domain.to_string(),
                                        verb: "register",
                                        id,
                                    });
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    });
}

fn collect_sources(root: &Path, cur: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = fs::read_dir(cur) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        let path = entry.path();
        if ft.is_dir() {
            collect_sources(root, &path, out);
        } else if ft.is_file() && path.extension().and_then(|s| s.to_str()) == Some("lua") {
            if let Ok(text) = fs::read_to_string(&path) {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, text));
            }
        }
    }
}

/// Checks literal content ids against the ROM tables. Only called when a ROM path was given.
fn check_against_rom(
    rom: &Path,
    mod_id: &str,
    refs: &[ContentRef],
    host: &ModHost,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let vfs = RezVfs::new(rom);
    let registered: BTreeSet<(String, i64)> = refs
        .iter()
        .filter(|r| r.verb == "register")
        .map(|r| (r.domain.clone(), r.id))
        .collect();

    if refs.iter().any(|r| r.domain == "weapons") {
        match WeaponsTable::from_vfs(&vfs) {
            Err(e) => errors.push(format!(
                "Mod '{}': cannot read the weapons table from the ROM: {}",
                mod_id, e
            )),
            Ok(table) => {
                for r in refs
                    .iter()
                    .filter(|r| r.domain == "weapons" && r.verb == "patch")
                {
                    let sec = i32::try_from(r.id).ok().and_then(|id| table.weapon(id));
                    let Some(sec) = sec else {
                        if !registered.contains(&("weapons".to_string(), r.id)) {
                            errors.push(format!(
                                "Mod '{}': {}:{}: mod.content.weapons:patch({}) names a weapon that is not in the ROM weapons table (use register to add a new one)",
                                mod_id, r.file, r.line, r.id
                            ));
                        }
                        continue;
                    };
                    if let Some(rec) = host.get_content_record("weapons", &r.id.to_string()) {
                        for (k, _) in rec.borrow().entries() {
                            if let dc_mod_lua::LuaKey::String(name) = k {
                                if name != "id" && sec.get(name).is_none() {
                                    warnings.push(format!(
                                        "Mod '{}': {}:{}: weapon {} has no field '{}' in the ROM table (the patch adds a new one)",
                                        mod_id, r.file, r.line, r.id, name
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if refs.iter().any(|r| r.domain == "shop") {
        match ShopCatalog::from_vfs(&vfs) {
            Err(e) => errors.push(format!(
                "Mod '{}': cannot read the shop catalogue from the ROM: {}",
                mod_id, e
            )),
            Ok(cat) => {
                for r in refs
                    .iter()
                    .filter(|r| r.domain == "shop" && r.verb == "patch")
                {
                    let known = i32::try_from(r.id)
                        .ok()
                        .map(|id| cat.product(id).is_some())
                        .unwrap_or(false);
                    if !known && !registered.contains(&("shop".to_string(), r.id)) {
                        warnings.push(format!(
                            "Mod '{}': {}:{}: mod.content.shop:patch({}) matches no ProductID in the ROM shop catalogue",
                            mod_id, r.file, r.line, r.id
                        ));
                    }
                }
            }
        }
    }
}

/// MAP-5: the world of a registered map is the mod's own `assets/...dat` file (the record check pinned the
/// string), it must exist in the mod folder and parse as a version-85 world whose sections tile the file
/// (`dc_world::World::parse` + `check_layout`). Runs without a ROM; textures are not resolved here.
// DC-DEFAULT: MAP-5 (proposed), see DC_DEFAULTS.md
fn check_map_world(mod_dir: &Path, rec: &MapRecord, errors: &mut Vec<String>) {
    let what = format!(
        "Mod '{}': map '{}': world '{}'",
        rec.mod_id, rec.id, rec.world
    );
    let Some(rel) = rec.world_rel_path() else {
        errors.push(format!("{what} is not this mod's own asset"));
        return;
    };
    let path = mod_dir.join(rel);
    if !path.is_file() {
        errors.push(format!(
            "{what} does not exist under the mod folder ({rel}); build it with dc-mapkit"
        ));
        return;
    }
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            errors.push(format!("{what} cannot be read: {e}"));
            return;
        }
    };
    match dc_world::World::parse(&bytes) {
        Err(e) => errors.push(format!("{what} does not parse as a version-85 world: {e}")),
        Ok(w) => {
            if let Err(e) = w.check_layout() {
                errors.push(format!("{what} has a broken section layout: {e}"));
            }
        }
    }
}

pub fn validate_mod(mod_path: &Path, rom_path: Option<&Path>) -> ValidationReport {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if !mod_path.is_dir() {
        return fail(
            None,
            vec![format!(
                "Mod directory '{}' does not exist or is not a directory",
                mod_path.display()
            )],
        );
    }

    let manifest_path = mod_path.join("manifest.json");
    if !manifest_path.is_file() {
        return fail(
            None,
            vec![format!(
                "Missing required manifest.json at '{}'",
                manifest_path.display()
            )],
        );
    }
    let manifest = match ModManifest::from_file(&manifest_path) {
        Ok(m) => m,
        Err(e) => return fail(None, vec![e]),
    };
    let mod_id = manifest.id.clone();

    if let Some(rom) = rom_path {
        if !rom.is_dir() {
            errors.push(format!(
                "Mod '{}': ROM path '{}' does not exist",
                mod_id,
                rom.display()
            ));
        }
    }

    let entry_path = mod_path.join(&manifest.entry);
    if !entry_path.is_file() {
        errors.push(format!(
            "Mod '{}': entry script '{}' declared in manifest.json does not exist at '{}'",
            mod_id,
            manifest.entry,
            entry_path.display()
        ));
        return fail(Some(mod_id), errors);
    }
    if let Some(schema) = &manifest.options_schema {
        if !mod_path.join(schema).is_file() {
            errors.push(format!(
                "Mod '{}': options_schema '{}' declared in manifest.json does not exist",
                mod_id, schema
            ));
        }
    }

    // Run the entry in a throw-away copy so nothing the host does can touch the author's tree.
    let temp = match TempDir::new("validate") {
        Ok(t) => t,
        Err(e) => {
            errors.push(format!("Mod '{}': {}", mod_id, e));
            return fail(Some(mod_id), errors);
        }
    };
    let work = temp.path().join(&mod_id);
    if let Err(e) = copy_tree(mod_path, &work) {
        errors.push(format!("Mod '{}': {}", mod_id, e));
        return fail(Some(mod_id), errors);
    }

    let mut host = ModHost::new();
    // Declared dependencies get empty stand-in mods so the entry can run standalone.
    for dep in &manifest.dependencies {
        let mut stub =
            ModManifest::minimal(&dep.id, &dep.id, stub_version(&dep.req), ModKind::Shared);
        stub.priority = -1000;
        if let Err(e) = host.load_mod_from_source(stub, "return function(mod) end", None) {
            errors.push(format!(
                "Mod '{}': cannot stand in for dependency '{}': {}",
                mod_id, dep.id, e
            ));
        }
    }

    let loaded = host.load_mod_from_dir_with_manifest(&work, manifest.clone());
    match &loaded {
        // The host's messages already carry "Mod '<id>' ..."; keep them verbatim.
        Err(e) => errors.push(if e.contains(&mod_id) {
            e.clone()
        } else {
            format!("Mod '{}': {}", mod_id, e)
        }),
        Ok(()) => {
            for e in host.mod_errors(&mod_id) {
                let line = format!("Mod '{}': {}", mod_id, e);
                if !errors.contains(&line) {
                    errors.push(line);
                }
            }
        }
    }

    // Registered maps: the world file must be there and parse (no ROM needed).
    if loaded.is_ok() {
        match host.map_records() {
            Err(e) => errors.push(format!("Mod '{}': {}", mod_id, e)),
            Ok(recs) => {
                for rec in recs.iter().filter(|r| r.mod_id == mod_id) {
                    check_map_world(&work, rec, &mut errors);
                }
            }
        }
    }

    // CRATE-6: a variant's rarity tier the lobby and the client would refuse is refused here first. Only that error:
    // the rows' ids are checked below, against the ROM tables when a ROM was given.
    if loaded.is_ok() {
        if let Err(e) = host.weapon_patches() {
            if e.contains(dc_bute::modpatch::KEY_VARIANT_RARITY) {
                errors.push(format!("Mod '{}': {}", mod_id, e));
            }
        }
    }

    // Literal content ids, checked against the ROM tables when (and only when) a ROM was given.
    if loaded.is_ok() {
        if let Some(rom) = rom_path.filter(|r| r.is_dir()) {
            let mut sources = Vec::new();
            collect_sources(&work, &work, &mut sources);
            sources.sort();
            let mut refs = Vec::new();
            for (rel, text) in &sources {
                scan_content_refs(rel, text, &mut refs);
            }
            check_against_rom(rom, &mod_id, &refs, &host, &mut errors, &mut warnings);
        }
    }

    ValidationReport {
        ok: errors.is_empty(),
        mod_id: Some(mod_id),
        errors,
        warnings,
    }
}
