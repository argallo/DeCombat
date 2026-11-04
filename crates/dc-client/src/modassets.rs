//! Mod files in the client's file lookups (CLI-1612) and the HUD override keys read from the loaded mod set (CLI-1613).
//!
//! - [`ModAssets`]: the loaded mods' unpacked folders in load order. Each mod's `overrides/<virtual path>` shadows the
//!   ROM entry of that path, a later mod shadows an earlier one ([`ModAssets::register_overrides`] gives the same
//!   order to a [`RezVfs`]); `@mod:<id>/assets/<rel>` (what `mod.assets:path("assets/<rel>")` returns) names the
//!   file `<mod folder>/assets/<rel>` of a mod of the set ([`ModAssets::resolve`]).
//! - The choke points: [`read_layered`] for the HUD textures ([`crate::hud::RomTextureSource`]) and [`read_entry`]
//!   for the sound files ([`crate::sndcache`]) consult the installed set first ([`install`]); without one they read
//!   the ROM exactly as before.
//! - [`hud_overrides`]: the `mod.content.hud:override(key, value)` records as plain data, read once after the set
//!   loads (no Lua call per frame).
//!
//! Wiring (the mod runtime owns the [`ModHost`]): after the server's set has loaded,
//! `crate::modassets::install(Some(ModAssets::from_host(&host)))` and
//! `hud.overrides = crate::modassets::hud_overrides(&host)?` on the match's [`crate::hud::Hud`]; when the player
//! leaves the server, `crate::modassets::install(None)`.
//!
//! Provenance: ORIGINAL (DeCombat's own mod packaging; the 2009 client reads its archives only).
// DC-DEFAULT: CLI-1612 (proposed) mod overrides and `@mod:` assets in the client's lookups, see DC_DEFAULTS.md

use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};

use dc_mod_lua::{LuaValue, ModHost};
use dc_rez::RezVfs;

/// The scheme of a mod asset path.
pub const MOD_SCHEME: &str = "@mod:";

/// The loaded mods' folders, in load order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModAssets {
    /// `(mod id, unpacked folder)` in load order.
    mods: Vec<(String, PathBuf)>,
}

impl ModAssets {
    /// The set of `mods` (`(id, unpacked folder)`, in load order).
    pub fn new(mods: impl IntoIterator<Item = (String, PathBuf)>) -> ModAssets {
        ModAssets {
            mods: mods.into_iter().collect(),
        }
    }

    /// The mods `host` loaded from a folder, in its load order (a mod loaded from source has no files).
    pub fn from_host(host: &ModHost) -> ModAssets {
        ModAssets::new(
            host.loaded_mods()
                .into_iter()
                .filter_map(|(m, root)| root.map(|r| (m.id, r))),
        )
    }

    /// The mods of a join's recipe (the same folders and order the match loads; no Lua runs): the menu's read-only
    /// view of the set, and the fallback of a `@mod:` world load when nothing is installed (MAP-30, MAP-32).
    // DC-DEFAULT: MAP-32 (proposed) the menu's read-only view of the join's mod set, see DC_DEFAULTS.md
    pub fn from_recipe(recipe: &crate::mods::HostRecipe) -> ModAssets {
        ModAssets::new(recipe.mods.iter().map(|m| (m.id.clone(), m.dir.clone())))
    }

    pub fn is_empty(&self) -> bool {
        self.mods.is_empty()
    }

    /// The id of the first mod of the set (the folder a preview treats as every `@mod:` id, MAP-33).
    pub fn first_id(&self) -> Option<&str> {
        self.mods.first().map(|(id, _)| id.as_str())
    }

    /// The `overrides/` folders that exist, in load order (the last one shadows the others).
    pub fn override_dirs(&self) -> Vec<PathBuf> {
        self.mods
            .iter()
            .map(|(_, r)| r.join("overrides"))
            .filter(|d| d.is_dir())
            .collect()
    }

    /// Adds every `overrides/` folder to `vfs` in load order: [`RezVfs`] gives a later folder the higher priority, so
    /// a later mod shadows an earlier one and every mod shadows the ROM.
    pub fn register_overrides(&self, vfs: &mut RezVfs) {
        for d in self.override_dirs() {
            vfs.add_override_dir(d);
        }
    }

    /// The file `@mod:<id>/assets/<rel>` names. Refused: another scheme, an id not in the set, a path outside
    /// `assets/`, `..`, absolute paths and drive letters, a file that resolves outside the mod's folder.
    pub fn resolve(&self, uri: &str) -> Result<PathBuf, String> {
        let rest = uri
            .strip_prefix(MOD_SCHEME)
            .ok_or_else(|| format!("'{uri}' is not a {MOD_SCHEME}<id>/assets/<file> path"))?;
        let (id, rel) = rest
            .split_once('/')
            .ok_or_else(|| format!("'{uri}' names no file after the mod id"))?;
        let root = self
            .mods
            .iter()
            .find(|(m, _)| m == id)
            .map(|(_, r)| r)
            .ok_or_else(|| {
                format!("'{uri}' names mod '{id}', which is not in the loaded mod set")
            })?;
        let clean = dc_mod_lua::validate_asset_rel_path(rel)?;
        if !clean.starts_with("assets/") || clean.len() == "assets/".len() {
            return Err(format!(
                "'{uri}': a mod asset lives under assets/ (mod.assets:path(\"assets/<file>\"))"
            ));
        }
        let path = root.join(&clean);
        // a link inside the folder must not lead out of it
        if let (Ok(real), Ok(base)) = (path.canonicalize(), root.canonicalize()) {
            if !real.starts_with(&base) {
                return Err(format!("'{uri}' resolves outside the folder of mod '{id}'"));
            }
        }
        Ok(path)
    }

    /// The override file of ROM path `path` (`a\b\c.dtx`, any case), the last mod's first.
    pub fn find_override(&self, path: &str) -> Option<PathBuf> {
        let rel = path.trim().replace('\\', "/");
        let parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
        if parts.is_empty()
            || parts
                .iter()
                .any(|p| *p == "." || *p == ".." || p.contains(':'))
        {
            return None;
        }
        self.override_dirs()
            .iter()
            .rev()
            .find_map(|d| case_fold_walk(d, &parts))
    }

    /// The bytes this layer answers for `path`: `Some(Err)` for a refused or unreadable `@mod:` path, `Some(Ok)` for
    /// a mod file or an override, `None` when the ROM serves it.
    pub fn read(&self, path: &str) -> Option<Result<Vec<u8>, String>> {
        if path.starts_with(MOD_SCHEME) {
            return Some(self.resolve(path).and_then(|p| {
                std::fs::read(&p).map_err(|e| format!("'{path}': {} ({e})", p.display()))
            }));
        }
        let p = self.find_override(path)?;
        Some(std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display())))
    }
}

/// `parts` under `dir`, each part matched ignoring ASCII case (the exact spelling first).
fn case_fold_walk(dir: &Path, parts: &[&str]) -> Option<PathBuf> {
    let mut at = dir.to_path_buf();
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        let exact = at.join(part);
        let ok = |p: &Path| if last { p.is_file() } else { p.is_dir() };
        if ok(&exact) {
            at = exact;
            continue;
        }
        let want = part.to_ascii_lowercase();
        at = std::fs::read_dir(&at).ok()?.flatten().find_map(|e| {
            let p = e.path();
            (ok(&p) && e.file_name().to_string_lossy().to_ascii_lowercase() == want).then_some(p)
        })?;
    }
    // the walk took plain names only
    debug_assert!(at.components().all(|c| !matches!(c, Component::ParentDir)));
    Some(at)
}

static ACTIVE: RwLock<Option<Arc<ModAssets>>> = RwLock::new(None);

/// Makes `set` the mod files every lookup sees (`None`: the ROM alone, the state of a vanilla server).
pub fn install(set: Option<ModAssets>) {
    let set = set.filter(|s| !s.is_empty()).map(Arc::new);
    *ACTIVE.write().unwrap_or_else(|e| e.into_inner()) = set;
}

/// The installed set, if any.
pub fn active() -> Option<Arc<ModAssets>> {
    ACTIVE.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The texture choke point: a mod file or an override of `path` (`None`: the ROM serves it, or a refused `@mod:`
/// path, which never falls back to the ROM; see [`is_mod_path`]).
pub fn read_layered(path: &str) -> Option<Vec<u8>> {
    match active()?.read(path)? {
        Ok(b) => Some(b),
        Err(e) => {
            log_refusal(&e);
            None
        }
    }
}

/// True for a `@mod:` path (the ROM never answers one).
pub fn is_mod_path(path: &str) -> bool {
    path.starts_with(MOD_SCHEME)
}

/// The sound / model choke point: the installed mod layer first, then the ROM entry of `path` through `vfs`.
pub fn read_entry(vfs: &RezVfs, path: &str) -> Result<Vec<u8>, String> {
    if let Some(r) = active().and_then(|a| a.read(path)) {
        return r;
    }
    if is_mod_path(path) {
        return Err(format!("'{path}': no mod set is loaded"));
    }
    dc_render::read_rom_entry(vfs, path).map_err(|e| e.to_string())
}

fn log_refusal(e: &str) {
    use std::collections::BTreeSet;
    use std::sync::Mutex;
    static SEEN: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
    if SEEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(e.to_string())
    {
        eprintln!("mods: asset refused: {e}");
    }
}

/// The HUD override keys of the loaded set (CLI-1613), as the HUD applies them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HudOverrides {
    /// A texture (ROM path or `@mod:` path) drawn centred in place of the procedural aim cross / aim mark.
    pub crosshair: Option<String>,
    /// The colour of the aim cross / aim mark (and the tint of a `crosshair` texture).
    pub crosshair_colour: Option<[u8; 4]>,
    /// A texture drawn centred for [`HITMARKER_MS`] after each confirmed own hit, fading out.
    pub hitmarker: Option<String>,
    /// The HUD's size factor, 0.5..2.0 (1.0 when absent).
    pub hud_scale: Option<f32>,
    /// CLI-1633: draw this shape instead of the aim cross / aim mark (and of a `crosshair` texture).
    pub crosshair_style: Option<crate::hud::crosshair::CrosshairStyle>,
    /// CLI-1633: how the drawn crosshair follows the spread (absent: dynamic).
    pub crosshair_recoil: Option<crate::hud::crosshair::RecoilMode>,
    /// CLI-1633: the drawn crosshair's shape keys (absent: the first version's look).
    pub crosshair_length: Option<f32>,
    pub crosshair_thickness: Option<f32>,
    pub crosshair_gap: Option<f32>,
    pub crosshair_dot: Option<bool>,
    pub crosshair_outline: Option<f32>,
    pub crosshair_t_style: Option<bool>,
    pub crosshair_recoil_scale: Option<f32>,
}

impl HudOverrides {
    pub fn is_empty(&self) -> bool {
        *self == HudOverrides::default()
    }

    /// The drawn crosshair's shape from the keys, read per composed list (no Lua, no allocation).
    pub fn cross_look(&self) -> crate::hud::crosshair::CrossLook {
        let d = crate::hud::crosshair::CrossLook::default();
        crate::hud::crosshair::CrossLook {
            length: self.crosshair_length.unwrap_or(d.length),
            thickness: self.crosshair_thickness.unwrap_or(d.thickness),
            gap: self.crosshair_gap.unwrap_or(d.gap),
            dot: self.crosshair_dot.unwrap_or(d.dot),
            outline: self.crosshair_outline.unwrap_or(d.outline),
            t_style: self.crosshair_t_style.unwrap_or(d.t_style),
            recoil: self.crosshair_recoil.unwrap_or(d.recoil),
            recoil_scale: self.crosshair_recoil_scale.unwrap_or(d.recoil_scale),
        }
    }
}

/// How long a `hitmarker` picture shows after an own hit (ms), fading linearly.
// DC-DEFAULT: CLI-1613 (proposed) 250 ms
pub const HITMARKER_MS: u32 = 250;

/// The HUD overrides `host` holds, read once after the set loads. An unknown key or a value of the wrong shape is an
/// error in the guide's style (the host refuses such a mod at load already; this is the second check).
pub fn hud_overrides(host: &ModHost) -> Result<HudOverrides, String> {
    parse_hud_overrides(&host.content_overrides("hud"))
}

/// [`hud_overrides`] over the plain records.
pub fn parse_hud_overrides(
    records: &std::collections::BTreeMap<String, LuaValue>,
) -> Result<HudOverrides, String> {
    let mut out = HudOverrides::default();
    for (key, value) in records {
        dc_mod_lua::check_hud_override(key, value)?;
        match key.as_str() {
            "crosshair" => out.crosshair = value.as_str().map(|s| s.trim().to_string()),
            "hitmarker" => out.hitmarker = value.as_str().map(|s| s.trim().to_string()),
            "crosshair_colour" => {
                out.crosshair_colour = value.as_str().and_then(dc_mod_lua::parse_rgba_hex)
            }
            "hud_scale" => out.hud_scale = value.as_f64().map(|v| v as f32),
            "crosshair_style" => {
                out.crosshair_style = value
                    .as_str()
                    .and_then(crate::hud::crosshair::CrosshairStyle::parse)
            }
            "crosshair_recoil" => {
                use crate::hud::crosshair::RecoilMode;
                out.crosshair_recoil = match value {
                    LuaValue::Bool(true) => Some(RecoilMode::Dynamic),
                    LuaValue::Bool(false) => Some(RecoilMode::Static),
                    other => other.as_str().and_then(RecoilMode::parse),
                }
            }
            "crosshair_length" => out.crosshair_length = value.as_f64().map(|v| v as f32),
            "crosshair_thickness" => out.crosshair_thickness = value.as_f64().map(|v| v as f32),
            "crosshair_gap" => out.crosshair_gap = value.as_f64().map(|v| v as f32),
            "crosshair_outline" => out.crosshair_outline = value.as_f64().map(|v| v as f32),
            "crosshair_recoil_scale" => out.crosshair_recoil_scale = value.as_f64().map(|v| v as f32),
            "crosshair_dot" => out.crosshair_dot = matches!(value, LuaValue::Bool(_)).then(|| matches!(value, LuaValue::Bool(true))),
            "crosshair_t_style" => out.crosshair_t_style = matches!(value, LuaValue::Bool(_)).then(|| matches!(value, LuaValue::Bool(true))),
            _ => unreachable!("check_hud_override refuses every other key"),
        }
    }
    Ok(out)
}

/// CLI-1619: the mods of one match on the thread that draws it. The link thread's [`ModHost`] cannot cross threads,
/// so the match loads its own from the join's [`crate::mods::HostRecipe`] (same folders, order, origins and policy).
/// While it lives its files are the installed set ([`install`]); dropping it (the match ends, the player leaves)
/// installs nothing again. Vanilla (no recipe) builds none.
// DC-DEFAULT: CLI-1619 (proposed) the match's own host, installed at the match start and removed at its end
pub struct MatchMods {
    pub host: std::rc::Rc<std::cell::RefCell<ModHost>>,
    /// The set's HUD override keys (read once).
    pub overrides: HudOverrides,
}

impl MatchMods {
    /// The match's mods from the join's recipe (`None`: vanilla, or a recipe that no longer loads, logged).
    pub fn open(recipe: Option<&crate::mods::HostRecipe>) -> Option<MatchMods> {
        let r = recipe?;
        let built = r.load().and_then(|host| {
            let overrides = hud_overrides(&host)?;
            Ok((host, overrides))
        });
        match built {
            Ok((host, overrides)) => {
                install(Some(ModAssets::from_host(&host)));
                println!(
                    "dc-client: mods {} loaded for the match",
                    host.load_order().join(", ")
                );
                Some(MatchMods {
                    host: std::rc::Rc::new(std::cell::RefCell::new(host)),
                    overrides,
                })
            }
            Err(e) => {
                eprintln!("mods: the match runs without the server's mods: {e}");
                None
            }
        }
    }

    /// Gives a match's HUD the override keys, the `hud.draw` bridge (when a mod declares `hooks.hud`) and the
    /// `render.entity_visibility` bridge (when the set declares `hooks.render` and `world.full_visibility`).
    pub fn wire(&self, d: &mut crate::hud::HudDriver) {
        d.hud.overrides = self.overrides.clone();
        // the live session shows what the HUD applies; a texture that does not load is said here, not kept silently
        let (line, failed) = describe_hud_overrides(&self.overrides, &mut d.hud.assets);
        println!("{line}");
        for f in failed {
            eprintln!("mods: hud override texture not loaded: {f}");
        }
        d.hud.hook = crate::hud::modhud::LuaHudHook::new(std::rc::Rc::clone(&self.host))
            .map(|h| Box::new(h) as Box<dyn crate::hud::HudDrawHook>);
        d.xray = crate::hud::xray::LuaRenderHook::new(std::rc::Rc::clone(&self.host));
    }
}

/// The log line of the HUD open: `mods: hud overrides: crosshair=@mod:<id>/assets/<file> (32x32), colour=#4080ffff`
/// (`none` without keys). Loads the override textures through `assets` (warming its cache); the second value lists
/// each texture that did not load (the procedural cross then stays, CLI-1613), with the reason the mod layer gives.
pub fn describe_hud_overrides(
    ov: &HudOverrides,
    assets: &mut crate::hud::compose::HudAssets,
) -> (String, Vec<String>) {
    let mut parts = Vec::new();
    let mut failed = Vec::new();
    let mut tex = |key: &str, path: &str, parts: &mut Vec<String>| match assets.texture(path) {
        Some(t) => parts.push(format!("{key}={path} ({}x{})", t.width, t.height)),
        None => {
            let why = match active() {
                None if is_mod_path(path) => "no mod files are installed".to_string(),
                Some(a) => match a.read(path) {
                    Some(Err(e)) => e,
                    Some(Ok(_)) => "the file is not a texture the client decodes".to_string(),
                    None => "not in the ROM".to_string(),
                },
                None => "not in the ROM".to_string(),
            };
            parts.push(format!("{key}={path} (NOT LOADED)"));
            failed.push(format!("{key}={path}: {why}"));
        }
    };
    if let Some(p) = &ov.crosshair {
        tex("crosshair", p, &mut parts);
    }
    if let Some(c) = ov.crosshair_colour {
        parts.push(format!(
            "colour=#{:02x}{:02x}{:02x}{:02x}",
            c[0], c[1], c[2], c[3]
        ));
    }
    if let Some(p) = &ov.hitmarker {
        tex("hitmarker", p, &mut parts);
    }
    if let Some(k) = ov.hud_scale {
        parts.push(format!("hud_scale={k}"));
    }
    if let Some(s) = ov.crosshair_style {
        parts.push(format!("style={}", s.name()));
    }
    if let Some(r) = ov.crosshair_recoil {
        parts.push(format!("recoil={}", r.name()));
    }
    for (name, v) in [
        ("length", ov.crosshair_length),
        ("thickness", ov.crosshair_thickness),
        ("gap", ov.crosshair_gap),
        ("outline", ov.crosshair_outline),
        ("recoil_scale", ov.crosshair_recoil_scale),
    ] {
        if let Some(v) = v {
            parts.push(format!("{name}={v}"));
        }
    }
    if ov.crosshair_dot == Some(true) {
        parts.push("dot".to_string());
    }
    if ov.crosshair_t_style == Some(true) {
        parts.push("t_style".to_string());
    }
    let body = if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(", ")
    };
    (format!("mods: hud overrides: {body}"), failed)
}

impl Drop for MatchMods {
    fn drop(&mut self) {
        install(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc-modassets-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(p: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    #[test]
    fn a_later_mod_shadows_an_earlier_one_and_both_shadow_the_rom() {
        let base = temp("shadow");
        let (a, b) = (base.join("a"), base.join("b"));
        put(&a.join("overrides/Interface/HUD/x.dtx"), b"from a");
        put(&a.join("overrides/Interface/HUD/only_a.dtx"), b"only a");
        put(&b.join("overrides/interface/hud/X.DTX"), b"from b");
        let set = ModAssets::new([("a".to_string(), a.clone()), ("b".to_string(), b.clone())]);
        assert_eq!(
            set.read("Interface\\HUD\\x.dtx").unwrap().unwrap(),
            b"from b"
        );
        assert_eq!(
            set.read("INTERFACE\\HUD\\ONLY_A.DTX").unwrap().unwrap(),
            b"only a"
        );
        assert!(
            set.read("Interface\\HUD\\rom_only.dtx").is_none(),
            "the ROM serves the rest"
        );
        assert!(set.read("..\\a\\overrides\\x").is_none());
        let reversed = ModAssets::new([("b".to_string(), b), ("a".to_string(), a.clone())]);
        assert_eq!(
            reversed.read("Interface/HUD/x.dtx").unwrap().unwrap(),
            b"from a"
        );

        // the same order given to a RezVfs: the last registered folder is the first one asked
        let mut vfs = RezVfs::new(base.join("no_rom"));
        set.register_overrides(&mut vfs);
        assert_eq!(
            vfs.override_dirs(),
            &[a.join("overrides"), base.join("b").join("overrides")]
        );
        let got = vfs
            .entry("INTERFACE.rez", "Interface/HUD/only_a.dtx")
            .unwrap();
        assert_eq!(got.as_deref(), Some(&b"only a"[..]));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn mod_paths_resolve_inside_assets_of_a_loaded_mod_only() {
        let base = temp("resolve");
        let a = base.join("green");
        put(&a.join("assets/crosshair.dtx"), b"tex");
        put(&a.join("main.lua"), b"--");
        let set = ModAssets::new([("green".to_string(), a.clone())]);
        assert_eq!(
            set.resolve("@mod:green/assets/crosshair.dtx").unwrap(),
            a.join("assets/crosshair.dtx")
        );
        assert_eq!(
            set.read("@mod:green/assets/crosshair.dtx")
                .unwrap()
                .unwrap(),
            b"tex"
        );
        let refuse = |uri: &str, what: &str| {
            let e = set.resolve(uri).unwrap_err();
            assert!(e.contains(what), "{uri}: {e}");
        };
        refuse(
            "@mod:other/assets/crosshair.dtx",
            "not in the loaded mod set",
        );
        refuse("@mod:green/assets/../main.lua", "forbidden");
        refuse("@mod:green/../green/assets/crosshair.dtx", "forbidden");
        refuse("@mod:green//etc/passwd", "forbidden");
        refuse("@mod:green/C:/Windows/x.dtx", "forbidden");
        refuse("@mod:green/main.lua", "lives under assets/");
        refuse("@mod:green", "names no file");
        refuse("Interface/x.dtx", "is not a @mod:");
        // a refused path never falls back to the ROM
        assert!(set
            .read("@mod:other/assets/crosshair.dtx")
            .unwrap()
            .is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn hud_overrides_parse_and_an_unknown_key_is_refused() {
        let s = |t: &str| LuaValue::String(t.to_string());
        let mut r = BTreeMap::new();
        r.insert(
            "crosshair".to_string(),
            s("@mod:green/assets/crosshair.dtx"),
        );
        r.insert("crosshair_colour".to_string(), s("#4080ffc0"));
        r.insert("hitmarker".to_string(), s("@mod:green/assets/hit.dtx"));
        r.insert("hud_scale".to_string(), LuaValue::Number(1.5));
        let o = parse_hud_overrides(&r).unwrap();
        assert_eq!(
            o.crosshair.as_deref(),
            Some("@mod:green/assets/crosshair.dtx")
        );
        assert_eq!(o.crosshair_colour, Some([0x40, 0x80, 0xff, 0xc0]));
        assert_eq!(o.hud_scale, Some(1.5));
        r.insert("radar".to_string(), s("x"));
        let e = parse_hud_overrides(&r).unwrap_err();
        assert!(e.contains("Unknown HUD override key 'radar'"), "{e}");
        assert!(parse_hud_overrides(&BTreeMap::new()).unwrap().is_empty());
    }

    #[test]
    fn overrides_come_from_a_loaded_host() {
        let base = temp("host");
        let dir = base.join("test_crosshair");
        put(
            &dir.join("manifest.json"),
            br#"{"id": "test_crosshair", "name": "G", "version": "1.0.0", "kind": "client_visual",
                "description": "A test fixture mod for the client tests.", "author": "tests", "screenshots": ["screenshots/main.png"],
                "capabilities": ["content.hud", "assets"]}"#,
        );
        put(
            &dir.join("main.lua"),
            br#"return function(mod)
                mod.content.hud:override("crosshair", mod.assets:path("assets/crosshair.dtx"))
                mod.content.hud:override("hud_scale", 0.75)
            end"#,
        );
        put(&dir.join("assets/crosshair.dtx"), b"tex");
        let mut host = ModHost::new();
        host.load_mod_dir(&dir).unwrap();
        let o = hud_overrides(&host).unwrap();
        assert_eq!(o.hud_scale, Some(0.75));
        let set = ModAssets::from_host(&host);
        assert_eq!(
            set.read(o.crosshair.as_deref().unwrap()).unwrap().unwrap(),
            b"tex"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}

/// An in-test client-visual crosshair mod (CLI-1613 regression fixture; the repository ships no such demo since
/// 2026-11-01): the texture is our own code's output, written into a temp folder per test.
#[cfg(test)]
pub mod test_fixture {
    const SIZE: usize = 32;
    /// White texels: the mod's `crosshair_colour` (#4080ff) tints them.
    const LIT: [u8; 4] = [255, 255, 255, 255];
    const EDGE: [u8; 4] = [0, 0, 0, 160];

    /// A 32 x 32 ring reticle: a 2-pixel ring of radius 10 to 12 and a centre dot, a dark edge round each, the rest
    /// clear.
    pub fn crosshair_rgba() -> Vec<u8> {
        let lit = |x: usize, y: usize| {
            let (dx, dy) = (x as f32 - 15.5, y as f32 - 15.5);
            let r = (dx * dx + dy * dy).sqrt();
            (10.0..12.0).contains(&r) || r < 1.6
        };
        let mut out = vec![0u8; SIZE * SIZE * 4];
        for y in 0..SIZE {
            for x in 0..SIZE {
                let c = if lit(x, y) {
                    LIT
                } else if (-1i32..=1).any(|dy| {
                    (-1i32..=1).any(|dx| {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        (0..SIZE as i32).contains(&nx)
                            && (0..SIZE as i32).contains(&ny)
                            && lit(nx as usize, ny as usize)
                    })
                }) {
                    EDGE
                } else {
                    [0; 4]
                };
                out[(y * SIZE + x) * 4..][..4].copy_from_slice(&c);
            }
        }
        out
    }

    /// Writes the fixture mod `id` (client_visual, `crosshair` + `crosshair_colour` #4080ff) to `root/<id>` and
    /// returns that folder.
    pub fn write_crosshair_mod(root: &std::path::Path, id: &str) -> std::path::PathBuf {
        let dir = root.join(id);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::create_dir_all(dir.join("screenshots")).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            format!(
                r#"{{"id": "{id}", "name": "Test Crosshair", "version": "1.0.0", "kind": "client_visual",
                "capabilities": ["content.hud", "assets"], "description": "A test mod: a ring crosshair for the HUD override keys.",
                "author": "DeCombat", "tags": ["hud", "test"], "screenshots": ["screenshots/main.png"]}}"#
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("main.lua"),
            "return function(mod)
    mod.content.hud:override(\"crosshair\", mod.assets:path(\"assets/crosshair.dtx\"))
    mod.content.hud:override(\"crosshair_colour\", \"#4080ff\")
end
",
        )
        .unwrap();
        let dtx = dc_tex::write_bgra8(SIZE as u16, SIZE as u16, &crosshair_rgba()).unwrap();
        std::fs::write(dir.join("assets/crosshair.dtx"), dtx).unwrap();
        image::RgbaImage::from_pixel(16, 9, image::Rgba([30, 40, 60, 255]))
            .save(dir.join("screenshots/main.png"))
            .unwrap();
        dir
    }

    #[test]
    fn the_fixture_texture_is_readable_by_the_clients_own_reader() {
        let dtx = dc_tex::write_bgra8(SIZE as u16, SIZE as u16, &crosshair_rgba()).unwrap();
        let tex = crate::hud::compose::decode_tex(&dtx).unwrap();
        assert_eq!((tex.width, tex.height), (SIZE as u32, SIZE as u32));
        assert_eq!(tex.rgba, crosshair_rgba());
    }
}
