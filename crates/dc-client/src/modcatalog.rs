//! Optional mods a player can download (CLI-1634, project decision): the login lobby's `/mods/catalog.json`
//! lists them; a download is verified and goes into `mods/local`, disabled. Nothing here touches the server's mod set,
//! its hash or the rule that a vanilla server loads no mods.
//!
//! Provenance: ORIGINAL (DeCombat's own mod delivery).
// DC-DEFAULT: CLI-1634 (proposed, project decision) the catalogue, its download and Remove, see DC_DEFAULTS.md

use std::net::SocketAddr;
use std::time::Duration;

use crate::mods::{
    read_package, unpack_package, web_get, ModConfig, ModPaths, DOWNLOAD_DEADLINE, DOWNLOAD_MAX_BYTES,
};

/// The file in a mod folder that marks it as downloaded from a catalogue (its package hash): only such a folder has
/// a Remove button; a folder the player put there is theirs.
pub const DOWNLOAD_MARK: &str = ".catalog";

pub const T_NO_CATALOG: &str = "This server offers no optional mods.";
pub const T_UNREACHABLE: &str = "The list of optional mods could not be read from this server.";
pub const T_READING: &str = "Reading the list of optional mods...";
pub const T_NO_SERVER: &str = "Choose a server to see its optional mods.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub id: String,
    pub version: String,
    pub sha256: String,
    pub name: String,
    pub author: String,
    pub description: String,
    pub size: u64,
    pub screenshot: bool,
    /// MOD-27: the manifest's command help and the lobby operator's picture (its SHA-256).
    pub commands: Vec<dc_mod_lua::ModCommand>,
    pub preview: Option<String>,
}

/// What the Mods screen knows of the catalogue.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CatalogState {
    #[default]
    Unread,
    Loading,
    /// None offered or not readable: the plain line to show.
    Nothing(String),
    Ready(Vec<CatalogEntry>),
}

/// The catalogue document. A malformed entry refuses the whole document.
pub fn parse_catalog(text: &str) -> Result<Vec<CatalogEntry>, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let list = v
        .get("mods")
        .and_then(|m| m.as_array())
        .ok_or("no 'mods' list")?;
    list.iter()
        .map(|m| {
            let s = |k: &str| {
                m.get(k)
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
                    .ok_or(format!("an entry has no '{k}'"))
            };
            let e = CatalogEntry {
                id: s("id")?,
                version: s("version")?,
                sha256: s("sha256")?,
                name: s("name")?,
                author: s("author")?,
                description: s("description")?,
                size: m.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
                screenshot: m.get("screenshot").and_then(|x| x.as_bool()).unwrap_or(false),
                commands: crate::mods::commands_of(m),
                preview: crate::mods::preview_of(m),
            };
            if !dc_mod_lua::is_valid_mod_id(&e.id) {
                return Err(format!("bad mod id '{}'", e.id));
            }
            if e.sha256.len() != 64 || !e.sha256.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
                return Err(format!("bad sha256 for '{}'", e.id));
            }
            Ok(e)
        })
        .collect()
}

/// Reads the catalogue from the web port (a missing one and a dead one both end in a plain line; the reason goes to
/// the log, never an address to the screen, CLI-1623) and fetches the listed pictures next to the cache.
pub fn fetch_catalog(web: Option<SocketAddr>, paths: Option<&ModPaths>, timeout: Duration) -> CatalogState {
    let Some(web) = web else {
        return CatalogState::Nothing(T_NO_CATALOG.into());
    };
    let reply = web_get(web, "/mods/catalog.json", 256 * 1024, timeout, timeout * 2, &mut |_, _| {});
    let list = match reply {
        Ok(r) if r.status == 200 => parse_catalog(&String::from_utf8_lossy(&r.body)),
        Ok(r) if r.status == 404 => return CatalogState::Nothing(T_NO_CATALOG.into()),
        Ok(r) => Err(format!("status {}", r.status)),
        Err(e) => Err(e),
    };
    let list = match list {
        Ok(l) => l,
        Err(e) => {
            eprintln!("mods: catalog: {e}");
            return CatalogState::Nothing(T_UNREACHABLE.into());
        }
    };
    if let Some(p) = paths {
        for e in list.iter().filter(|e| e.screenshot) {
            let file = p.catalog_shot(&e.sha256);
            if file.is_file() {
                continue;
            }
            let target = format!("/mods/{}.png", e.sha256);
            let r = web_get(web, &target, dc_mod_lua::MAX_SCREENSHOT_BYTES, timeout, timeout * 2, &mut |_, _| {});
            if let Ok(r) = r.filter_ok() {
                if let Some(d) = file.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                let _ = std::fs::write(&file, &r.body);
            }
        }
    }
    if let Some(p) = paths {
        for sha in list.iter().filter_map(|e| e.preview.as_deref()) {
            crate::mods::store_preview(p, sha, &mut |target| {
                web_get(web, target, dc_mod_lua::MAX_SCREENSHOT_BYTES, timeout, timeout * 2, &mut |_, _| {})
            });
        }
    }
    if list.is_empty() {
        return CatalogState::Nothing(T_NO_CATALOG.into());
    }
    CatalogState::Ready(list)
}

trait FilterOk: Sized {
    fn filter_ok(self) -> Result<crate::mods::HttpReply, ()>;
}

impl FilterOk for Result<crate::mods::HttpReply, String> {
    fn filter_ok(self) -> Result<crate::mods::HttpReply, ()> {
        match self {
            Ok(r) if r.status == 200 => Ok(r),
            _ => Err(()),
        }
    }
}

/// Downloads and installs `e`. The text of an `Err` is for the player.
pub fn download(web: Option<SocketAddr>, paths: &ModPaths, e: &CatalogEntry, cfg: &ModConfig) -> Result<(), String> {
    let web = web.ok_or("The server is not known.")?;
    let cap = cfg.max_mod_bytes.min(DOWNLOAD_MAX_BYTES);
    let target = format!("/mods/{}.zip", e.sha256);
    let r = web_get(web, &target, cap, cfg.timeout, DOWNLOAD_DEADLINE, &mut |_, _| {}).map_err(|why| {
        eprintln!("mods: catalog download of {}: {why}", e.id);
        format!("'{}' could not be downloaded.", e.name)
    })?;
    if r.status != 200 {
        return Err(format!("The server did not send '{}'.", e.name));
    }
    install_bytes(&r.body, paths, e, cap)
}

/// Installs a downloaded package into `mods/local/<id>`: its bytes must hash to the entry's sha256 and its verified
/// manifest must be the entry's id and version, kind `client_visual`, without the hooks a local mod may not use. A
/// refusal writes nothing. An existing folder of that id is replaced (an update); enabling stays the player's act.
pub fn install_bytes(bytes: &[u8], paths: &ModPaths, e: &CatalogEntry, cap: u64) -> Result<(), String> {
    if bytes.len() as u64 > cap {
        return Err(format!("'{}' is larger than the size limit.", e.name));
    }
    if dc_rom::sha256_hex(bytes) != e.sha256 {
        return Err(format!(
            "'{}' does not match the hash the server listed; nothing was installed.",
            e.name
        ));
    }
    let files = read_package(bytes, cap).map_err(|why| format!("'{}': {why}", e.name))?;
    let text = files
        .iter()
        .find(|f| f.name == "manifest.json")
        .map(|f| String::from_utf8_lossy(&f.data).into_owned())
        .ok_or(format!("'{}' has no manifest.", e.name))?;
    let m = dc_mod_lua::ModManifest::from_json_str(&text).map_err(|why| format!("'{}': {why}", e.name))?;
    if m.id != e.id || m.version.to_string() != e.version {
        return Err(format!("'{}' holds another mod than the server listed.", e.name));
    }
    if m.kind != dc_mod_lua::ModKind::ClientVisual {
        return Err(format!(
            "'{}' is kind '{}'; only client-visual mods can be downloaded here.",
            e.name, m.kind
        ));
    }
    if let Some(c) = dc_mod_lua::CLIENT_VIEW_CAPABILITIES.iter().find(|c| m.has_capability(c)) {
        return Err(format!("'{}' uses '{c}', which a local mod may not.", e.name));
    }
    let dest = paths.local().join(&m.id);
    // A folder without the download mark is the player's own mod: never overwritten (review 2026-11-01).
    if dest.exists() && !dest.join(DOWNLOAD_MARK).is_file() {
        return Err(format!("You already have your own mod folder named '{}'; it was left as it is.", m.id));
    }
    unpack_package(bytes, &dest, cap).map_err(|why| format!("'{}': {why}", e.name))?;
    std::fs::write(dest.join(DOWNLOAD_MARK), &e.sha256).map_err(|why| format!("'{}': {why}", e.name))
}

/// Deletes a downloaded local mod's folder; refused for a folder without the download mark.
pub fn remove_downloaded(dir: &std::path::Path) -> Result<(), String> {
    if !dir.join(DOWNLOAD_MARK).is_file() {
        return Err("This mod was not downloaded here; remove it from the folder yourself.".into());
    }
    std::fs::remove_dir_all(dir).map_err(|e| format!("The mod could not be removed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("dc_catalog_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn pkg(root: &std::path::Path, id: &str, kind: &str, caps: &str) -> (Vec<u8>, CatalogEntry) {
        let d = root.join("src").join(id);
        std::fs::create_dir_all(d.join("screenshots")).unwrap();
        std::fs::write(d.join("screenshots/main.png"), dc_modkit::placeholder_png()).unwrap();
        std::fs::write(
            d.join("manifest.json"),
            format!(
                r#"{{"id":"{id}","name":"N","version":"1.0.0","kind":"{kind}","capabilities":[{caps}],"description":"A package for the catalogue tests.","author":"tests","screenshots":["screenshots/main.png"]}}"#
            ),
        )
        .unwrap();
        std::fs::write(d.join("main.lua"), "return function(mod) end").unwrap();
        let (bytes, entry) = dc_modkit::pack_mod_bytes(&d).unwrap();
        (
            bytes,
            CatalogEntry {
                id: id.into(),
                version: "1.0.0".into(),
                sha256: entry.sha256,
                name: "N".into(),
                author: "tests".into(),
                description: "d".into(),
                size: 1,
                screenshot: false,
                commands: Vec::new(),
                preview: None,
            },
        )
    }

    #[test]
    fn the_catalogue_parses_and_a_bad_entry_refuses_it() {
        let good = format!(
            r#"{{"version":1,"mods":[{{"id":"a","version":"1.0.0","sha256":"{}","name":"A","author":"x","description":"d","size":5,"screenshot":true}}]}}"#,
            "ab".repeat(32)
        );
        let l = parse_catalog(&good).unwrap();
        assert_eq!((l[0].id.as_str(), l[0].size, l[0].screenshot), ("a", 5, true));
        assert!(parse_catalog(&good.replace(&"ab".repeat(32), "xyz")).unwrap_err().contains("sha256"));
        assert!(parse_catalog(&good.replace("\"a\"", "\"../a\"")).unwrap_err().contains("mod id"));
        assert!(parse_catalog("{}").is_err());
    }

    #[test]
    fn a_hash_mismatch_is_refused_and_nothing_is_installed() {
        let root = temp("hash");
        let paths = ModPaths::new(&root.join("data"));
        let (bytes, mut e) = pkg(&root, "look", "client_visual", "");
        e.sha256 = "00".repeat(32);
        let why = install_bytes(&bytes, &paths, &e, DOWNLOAD_MAX_BYTES).unwrap_err();
        assert!(why.contains("does not match the hash"), "{why}");
        assert!(!paths.local().join("look").exists());
    }

    #[test]
    fn only_a_client_visual_package_without_hooks_is_installed() {
        let root = temp("kind");
        let paths = ModPaths::new(&root.join("data"));
        for (id, kind, caps, want) in [
            ("rules", "server_rules", "", "only client-visual"),
            ("both", "shared", "", "only client-visual"),
            ("hooky", "client_visual", "\"hooks.hud\"", "a local mod may not"),
        ] {
            let (bytes, e) = pkg(&root, id, kind, caps);
            let why = install_bytes(&bytes, &paths, &e, DOWNLOAD_MAX_BYTES).unwrap_err();
            assert!(why.contains(want), "{id}: {why}");
            assert!(!paths.local().join(id).exists(), "{id}");
        }
    }

    #[test]
    fn a_download_never_overwrites_a_players_own_folder() {
        let root = temp("own");
        let (bytes, e) = pkg(&root, "own_id", "client_visual", "");
        let paths = crate::mods::ModPaths::new(&root);
        let dest = paths.local().join("own_id");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("mine.txt"), "x").unwrap();
        assert!(install_bytes(&bytes, &paths, &e, DOWNLOAD_MAX_BYTES).is_err());
        assert!(dest.join("mine.txt").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_package_is_installed_marked_and_removable_a_players_folder_is_not() {
        let root = temp("install");
        let paths = ModPaths::new(&root.join("data"));
        let (bytes, e) = pkg(&root, "look", "client_visual", "");
        install_bytes(&bytes, &paths, &e, DOWNLOAD_MAX_BYTES).unwrap();
        let dir = paths.local().join("look");
        assert!(dir.join("manifest.json").is_file());
        let listed = crate::mods::list_local(&paths);
        assert!(listed[0].downloaded);
        remove_downloaded(&dir).unwrap();
        assert!(!dir.exists());
        let mine = paths.local().join("mine");
        std::fs::create_dir_all(&mine).unwrap();
        assert!(remove_downloaded(&mine).is_err());
        assert!(mine.exists());
    }

    #[test]
    fn no_web_port_is_a_plain_line() {
        assert_eq!(
            fetch_catalog(None, None, Duration::from_millis(50)),
            CatalogState::Nothing(T_NO_CATALOG.into())
        );
        // nothing listens: the line names no address
        let web = "127.0.0.1:1".parse().unwrap();
        assert_eq!(
            fetch_catalog(Some(web), None, Duration::from_millis(200)),
            CatalogState::Nothing(T_UNREACHABLE.into())
        );
    }
}
