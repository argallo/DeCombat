//! CLI-1948: Vulkan implicit layers of other programs that are known to crash the game are kept out of this process.
//!
//! Why: field reports 2026-11-06 and 2026-11-08 (account 1150, GTX 960, NVIDIA 560.94, beta4 and beta6): every
//! Vulkan start died with an access violation inside `fpsmonvk64.dll`, the Vulkan layer of the overlay program
//! "FPS Monitor", during the menu's present (eight crash reports, the same fault offset). Windows loads every
//! implicit layer registered under `Software\Khronos\Vulkan\ImplicitLayers` into every Vulkan process, so the game
//! cannot avoid it by itself; the Vulkan loader honours `VK_LOADER_LAYERS_DISABLE` (a comma list of layer names)
//! set before the instance is created.
//!
//! How: at start, before any Vulkan instance, the implicit layers are read from the registry (read only: `reg query`
//! on HKLM and HKCU; the game never writes the registry) and their JSON files (`layer.name`, `layer.library_path`);
//! a layer whose library file name holds one of [`BLOCKED`] is added to `VK_LOADER_LAYERS_DISABLE` (appended to a
//! value the player set), one line names it in `dc-client.out`, and the list goes into the diagnostic report's
//! `system.vulkan_layers` so the next overlay is visible without a dump. `DC_VK_LAYERS=keep` turns the disabling off
//! (the list is still read and reported); `DC_VK_LAYERS_DIR=<folder>` reads layer JSON files from that folder instead
//! of the registry (tests, the headless check). Nothing else of the Vulkan loader's behaviour changes.
// ORIGINAL: CLI-1948 our own guard around other programs' Vulkan layers; no 2009 behaviour (the 2009 client has no Vulkan)
// DC-DEFAULT: CLI-1948 (proposed), see DC_DEFAULTS.md

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Library file name stems (compared without case) of layers known to crash the game.
pub const BLOCKED: [&str; 1] = ["fpsmonvk"];
/// The Vulkan loader's list of layer names it must not load (comma separated).
pub const DISABLE_ENV: &str = "VK_LOADER_LAYERS_DISABLE";
/// `keep`: read and report the layers but disable none.
pub const OFF_ENV: &str = "DC_VK_LAYERS";
/// A folder of layer JSON files read instead of the registry (tests, the headless check).
pub const DIR_ENV: &str = "DC_VK_LAYERS_DIR";

/// One implicit layer as registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    /// `layer.name` of its JSON (what the loader's disable list matches).
    pub name: String,
    /// `layer.library_path` of its JSON (the DLL a crash names).
    pub library: String,
    /// The JSON file the registry names.
    pub json: PathBuf,
    /// The registry value is 0 (a non-zero value disables the layer for everyone).
    pub enabled: bool,
}

impl Layer {
    /// The layer's library is on the [`BLOCKED`] list.
    pub fn blocked(&self) -> bool {
        is_blocked(&self.library)
    }
}

/// Whether a library path names a blocked layer: its file name (without folders) holds a [`BLOCKED`] stem, ignoring case.
pub fn is_blocked(library: &str) -> bool {
    let file = library
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(library)
        .to_ascii_lowercase();
    BLOCKED.iter().any(|b| file.contains(b))
}

/// `layer.name` and `layer.library_path` of a layer JSON (`None` when either is missing).
pub fn parse_layer_json(text: &str) -> Option<(String, String)> {
    let v: Value = serde_json::from_str(text).ok()?;
    let l = v.get("layer")?;
    let name = l.get("name")?.as_str()?.trim().to_string();
    let lib = l.get("library_path")?.as_str()?.trim().to_string();
    (!name.is_empty() && !lib.is_empty()).then_some((name, lib))
}

/// The `VK_LOADER_LAYERS_DISABLE` value that disables `names` on top of what `existing` already disables.
pub fn disable_value(existing: Option<&str>, names: &[&str]) -> String {
    let mut parts: Vec<String> = existing
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    for n in names {
        if !parts.iter().any(|p| p == n) {
            parts.push((*n).to_string());
        }
    }
    parts.join(",")
}

/// The registry's implicit layer entries as (JSON path, enabled), read only, both hives; empty off Windows or when
/// `reg query` is not there. One `reg query` per hive; a missing key is an empty list.
pub fn registry_entries() -> Vec<(PathBuf, bool)> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for hive in ["HKLM", "HKCU"] {
        let key = format!("{hive}\\SOFTWARE\\Khronos\\Vulkan\\ImplicitLayers");
        let Ok(o) = std::process::Command::new("reg")
            .args(["query", &key])
            .output()
        else {
            continue;
        };
        if !o.status.success() {
            continue;
        }
        out.extend(parse_reg_query(&String::from_utf8_lossy(&o.stdout)));
    }
    out
}

/// The values of a `reg query` listing: `    <path>    REG_DWORD    0x0` lines (the path may hold spaces).
pub fn parse_reg_query(text: &str) -> Vec<(PathBuf, bool)> {
    text.lines()
        .filter_map(|l| {
            let (path, rest) = l.split_once("REG_DWORD")?;
            let path = path.trim();
            if path.is_empty() {
                return None;
            }
            let data = rest.trim();
            let n = data
                .strip_prefix("0x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| data.parse::<u32>().ok())
                .unwrap_or(0);
            Some((PathBuf::from(path), n == 0))
        })
        .collect()
}

/// The layer JSON files of `dir` (every `*.json`, all counted as enabled): the [`DIR_ENV`] stand-in for the registry.
fn dir_entries(dir: &Path) -> Vec<(PathBuf, bool)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<(PathBuf, bool)> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("json"))
        })
        .map(|p| (p, true))
        .collect();
    v.sort();
    v
}

/// The implicit layers as registered (the folder of [`DIR_ENV`] instead when set); entries whose JSON cannot be
/// read or parsed are skipped.
pub fn implicit_layers() -> Vec<Layer> {
    let entries = match std::env::var_os(DIR_ENV).filter(|d| !d.is_empty()) {
        Some(d) => dir_entries(Path::new(&d)),
        None => registry_entries(),
    };
    entries
        .into_iter()
        .filter_map(|(json, enabled)| {
            let text = std::fs::read_to_string(&json).ok()?;
            let (name, library) = parse_layer_json(&text)?;
            Some(Layer {
                name,
                library,
                json,
                enabled,
            })
        })
        .collect()
}

static FOUND: OnceLock<(Vec<Layer>, Vec<String>)> = OnceLock::new();

/// At start, before any Vulkan instance: reads the layers, disables the blocked ones through [`DISABLE_ENV`] (unless
/// `DC_VK_LAYERS=keep`), prints one line per disabled layer. Returns the layers and the names disabled. Once per
/// process; later calls return the first result.
pub fn guard() -> &'static (Vec<Layer>, Vec<String>) {
    FOUND.get_or_init(|| {
        let layers = implicit_layers();
        let keep = std::env::var(OFF_ENV).is_ok_and(|v| v.trim().eq_ignore_ascii_case("keep"));
        let names: Vec<String> = layers
            .iter()
            .filter(|l| l.enabled && l.blocked())
            .map(|l| l.name.clone())
            .collect();
        if names.is_empty() {
            return (layers, Vec::new());
        }
        if keep {
            println!(
                "dc-client: Vulkan layer(s) {} known to crash the game stay loaded ({OFF_ENV}=keep)",
                names.join(", ")
            );
            return (layers, Vec::new());
        }
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let value = disable_value(std::env::var(DISABLE_ENV).ok().as_deref(), &refs);
        std::env::set_var(DISABLE_ENV, &value);
        for l in layers.iter().filter(|l| l.enabled && l.blocked()) {
            println!(
                "dc-client: Vulkan layer {} ({}) disabled for this process: it is known to crash the game ({DISABLE_ENV}; {OFF_ENV}=keep keeps it)",
                l.name, l.library
            );
        }
        (layers, names)
    })
}

/// The layers for the diagnostic report's `system.vulkan_layers`: `[{name, library, enabled, disabled}]`
/// (empty before [`guard`] ran, or without layers).
pub fn report_json() -> Value {
    let Some((layers, disabled)) = FOUND.get() else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        layers
            .iter()
            .map(|l| {
                json!({
                    "name": l.name,
                    "library": l.library,
                    "enabled": l.enabled,
                    "disabled": disabled.contains(&l.name),
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const FPSMON: &str = r#"{"file_format_version": "1.0.0", "layer": {"name": "VK_LAYER_FPSMON_fpsmon",
        "type": "GLOBAL", "library_path": ".\\fpsmonvk64.dll", "api_version": "1.1.0", "implementation_version": "1",
        "description": "FPS Monitor overlay"}}"#;
    const STEAM: &str = r#"{"file_format_version": "1.0.0", "layer": {"name": "VK_LAYER_VALVE_steam_overlay_64",
        "library_path": "C:\\Program Files (x86)\\Steam\\GameOverlayRenderer64.dll", "api_version": "1.2.136"}}"#;

    #[test]
    fn a_layer_json_gives_its_name_and_library() {
        assert_eq!(
            parse_layer_json(FPSMON),
            Some(("VK_LAYER_FPSMON_fpsmon".into(), ".\\fpsmonvk64.dll".into()))
        );
        assert_eq!(parse_layer_json("{}"), None);
        assert_eq!(
            parse_layer_json(r#"{"layer": {"name": "x"}}"#),
            None,
            "no library"
        );
        assert_eq!(parse_layer_json("not json"), None);
    }

    #[test]
    fn only_the_blocked_library_stems_match_whatever_the_folder_or_case() {
        assert!(is_blocked(".\\fpsmonvk64.dll"));
        assert!(is_blocked(
            "C:\\Program Files (x86)\\Steam\\steamapps\\common\\FPS Monitor\\tools\\FPSMONVK64.DLL"
        ));
        assert!(is_blocked("fpsmonvk32.dll"));
        assert!(!is_blocked(
            "C:\\Program Files (x86)\\Steam\\GameOverlayRenderer64.dll"
        ));
        assert!(
            !is_blocked("C:\\fpsmon\\other.dll"),
            "the folder name does not count"
        );
        assert!(!is_blocked("RTSSVkLayer64.dll"));
    }

    #[test]
    fn the_disable_value_appends_to_the_players_own_list_once() {
        assert_eq!(
            disable_value(None, &["VK_LAYER_FPSMON_fpsmon"]),
            "VK_LAYER_FPSMON_fpsmon"
        );
        assert_eq!(
            disable_value(Some("VK_LAYER_X_y, "), &["VK_LAYER_FPSMON_fpsmon"]),
            "VK_LAYER_X_y,VK_LAYER_FPSMON_fpsmon"
        );
        assert_eq!(
            disable_value(Some("VK_LAYER_FPSMON_fpsmon"), &["VK_LAYER_FPSMON_fpsmon"]),
            "VK_LAYER_FPSMON_fpsmon"
        );
        assert_eq!(disable_value(Some(""), &[]), "");
    }

    #[test]
    fn a_reg_query_listing_gives_the_json_paths_and_their_enabled_flag() {
        let text = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Khronos\\Vulkan\\ImplicitLayers\r\n    \
                    C:\\Program Files (x86)\\Steam\\steamapps\\common\\FPS Monitor\\tools\\fpsmonvk64.json    REG_DWORD    0x0\r\n    \
                    C:\\Program Files (x86)\\Steam\\SteamOverlayVulkanLayer64.json    REG_DWORD    0x1\r\n\r\n";
        let v = parse_reg_query(text);
        assert_eq!(v.len(), 2);
        assert_eq!(
            v[0].0,
            PathBuf::from("C:\\Program Files (x86)\\Steam\\steamapps\\common\\FPS Monitor\\tools\\fpsmonvk64.json")
        );
        assert!(v[0].1, "0x0 = enabled");
        assert!(!v[1].1, "0x1 = disabled by the user");
        assert!(parse_reg_query(
            "ERROR: The system was unable to find the specified registry key or value.\r\n"
        )
        .is_empty());
    }

    /// The folder stand-in: one blocked and one harmless layer; the blocked one is the only one in the disable list.
    #[test]
    fn a_folder_of_layer_files_is_read_like_the_registry_and_the_blocked_one_is_named() {
        let d = std::env::temp_dir().join(format!("dc-vklayers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("fpsmonvk64.json"), FPSMON).unwrap();
        std::fs::write(d.join("steam.json"), STEAM).unwrap();
        std::fs::write(d.join("broken.json"), "{").unwrap();
        let layers: Vec<Layer> = dir_entries(&d)
            .into_iter()
            .filter_map(|(json, enabled)| {
                let (name, library) = parse_layer_json(&std::fs::read_to_string(&json).ok()?)?;
                Some(Layer {
                    name,
                    library,
                    json,
                    enabled,
                })
            })
            .collect();
        assert_eq!(layers.len(), 2, "the broken file is skipped: {layers:?}");
        let blocked: Vec<&str> = layers
            .iter()
            .filter(|l| l.blocked())
            .map(|l| l.name.as_str())
            .collect();
        assert_eq!(blocked, ["VK_LAYER_FPSMON_fpsmon"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The registry read never fails on any machine (empty when there are no layers or no `reg`).
    #[test]
    fn the_registry_read_does_not_fail() {
        let v = registry_entries();
        for (p, _) in &v {
            assert!(
                p.extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("json")),
                "{p:?}"
            );
        }
    }
}
