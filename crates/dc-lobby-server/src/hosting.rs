//! Mod hosting over the clan web port: `GET /mods/<sha256>.zip` and `GET /server_list.json`.
//!
//! A client that does not have a mod of the server's set fetches the package here (HOSTING_AND_MODS.md §4 item 3)
//! and refuses it unless its SHA-256 equals the name it asked for; the server therefore never sends bytes whose
//! hash differs from the requested name, and never more than the size cap. `server_list.json` is the server-list
//! document with the `mod_set` fragment and its hash (§4 item 6). Both are read-only and need no account.
//!
//! The package is the file `<mods dir>/<sha256>.zip`, or, when only the mod folder is there, the deterministic
//! pack of a folder whose pack hashes to the requested name.
// SPEC: HOSTING_AND_MODS.md §4 items 3 and 6; the size cap is the config key `mods.max_zip_bytes`

use std::io::Read;
use std::path::{Path, PathBuf};

/// Largest package sent, in bytes; `mods.max_zip_bytes` of server.toml overrides it (same default as the schema).
pub const DEFAULT_MAX_MOD_BYTES: u64 = 32 * 1024 * 1024;

/// What the web port serves besides the clan pages.
#[derive(Clone, Debug)]
pub struct Hosting {
    pub mods_dir: Option<PathBuf>,
    /// The server-list document (`modset::server_list_json`), when the server runs from a `server.toml`.
    pub server_list: Option<String>,
    /// MOD-25: the optional mods' document (`modset::build_catalog`) and their first screenshots by package hash.
    /// MOD-27: also the operators' pictures (`<mod id>.preview.png`) by the picture's own hash.
    pub catalog: Option<String>,
    pub catalog_shots: std::collections::BTreeMap<String, Vec<u8>>,
    pub max_mod_bytes: u64,
    /// LG-313: the featured entries' set documents (`/entry_mods/<entry id>.json`).
    pub entry_sets: std::collections::BTreeMap<u32, String>,
    /// LG-322: the client version document (`/client_version.json`, [`client_version_doc`]).
    pub client_version: Option<String>,
    /// LG-323: which mod folder packs to which hash, worked out once at the first request for a package with no file.
    pub folder_packs: std::sync::OnceLock<std::collections::BTreeMap<String, PathBuf>>,
}

/// `/client_version.json` of `[client]`: `{"latest","min_version","update_url"}`; `None` with neither version set.
// DC-DEFAULT: LG-322 (proposed, USER REQUEST 2026-11-02) the lobby tells clients the current and the oldest allowed version
pub fn client_version_doc(c: &dc_config::schema::ClientSection) -> Option<String> {
    if c.latest.is_empty() && c.min_version.is_empty() {
        return None;
    }
    use dc_mod_lua::JsonValue::{Object, String as S};
    let mut o = std::collections::BTreeMap::new();
    o.insert("latest".to_string(), S(c.latest.clone()));
    o.insert("min_version".to_string(), S(c.min_version.clone()));
    o.insert("update_url".to_string(), S(c.update_url.clone()));
    Some(Object(o).to_json_pretty())
}

impl Hosting {
    /// Nothing hosted (tests; the server always builds the struct from its settings).
    #[cfg(test)]
    pub fn none() -> Hosting {
        Hosting {
            mods_dir: None,
            server_list: None,
            catalog: None,
            catalog_shots: Default::default(),
            max_mod_bytes: DEFAULT_MAX_MOD_BYTES,
            entry_sets: Default::default(),
            client_version: None,
            folder_packs: Default::default(),
        }
    }
}

/// A reply of the hosting routes: status, content type, body.
#[derive(Debug, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

fn text(status: u16, msg: &str) -> Reply {
    Reply {
        status,
        content_type: "text/plain; charset=utf-8",
        body: msg.as_bytes().to_vec(),
    }
}

/// `<64 lower-case hex digits>.zip` -> the hash part. Anything else (separators, upper case, other lengths) is
/// not a package name, so no request can reach a path outside the mods directory.
pub fn package_hash(name: &str) -> Option<&str> {
    let h = name.strip_suffix(".zip")?;
    (h.len() == 64
        && h.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then_some(h)
}

fn read_capped(path: &Path, cap: u64) -> Result<Vec<u8>, Reply> {
    let f = std::fs::File::open(path).map_err(|_| text(404, "No such package."))?;
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len > cap {
        return Err(text(413, "Package exceeds the size cap."));
    }
    let mut v = Vec::new();
    f.take(cap + 1)
        .read_to_end(&mut v)
        .map_err(|_| text(500, "Cannot read the package."))?;
    if v.len() as u64 > cap {
        return Err(text(413, "Package exceeds the size cap."));
    }
    Ok(v)
}

/// `GET /mods/<sha256>.zip`.
pub fn mod_package(h: &Hosting, name: &str) -> Reply {
    let (Some(want), Some(dir)) = (package_hash(name), h.mods_dir.as_deref()) else {
        return text(404, "No such package.");
    };
    let zip = |body: Vec<u8>| Reply {
        status: 200,
        content_type: "application/zip",
        body,
    };
    let file = dir.join(format!("{want}.zip"));
    if file.is_file() {
        return match read_capped(&file, h.max_mod_bytes) {
            Ok(b) if dc_rom::sha256_hex(&b) == want => zip(b),
            Ok(_) => text(500, "Package does not match its hash; refusing to send it."),
            Err(r) => r,
        };
    }
    // only the folder is there: send its deterministic pack when it hashes to the requested name.
    // DC-DEFAULT: LG-323 (proposed) the folders are packed once: a stream of made-up hashes costs a map lookup, not a
    // pack of every folder per request. A folder added or changed later needs a restart, as for the mod set itself.
    let packs = h.folder_packs.get_or_init(|| {
        crate::modset::mod_dirs(dir)
            .into_iter()
            .filter_map(|d| {
                let (_, entry) = dc_modkit::pack_mod_bytes(&d).ok()?;
                Some((entry.sha256, d))
            })
            .collect()
    });
    if let Some(d) = packs.get(want) {
        if let Ok((bytes, _)) = dc_modkit::pack_mod_bytes(d) {
            if dc_rom::sha256_hex(&bytes) == want {
                return if bytes.len() as u64 > h.max_mod_bytes {
                    text(413, "Package exceeds the size cap.")
                } else {
                    zip(bytes)
                };
            }
        }
    }
    text(404, "No such package.")
}

fn served(kind: &'static str, body: Vec<u8>) -> Reply {
    Reply {
        status: 200,
        content_type: kind,
        body,
    }
}

/// `GET /server_list.json`.
pub fn server_list(h: &Hosting) -> Reply {
    match &h.server_list {
        Some(j) => served("application/json", j.clone().into_bytes()),
        None => text(404, "This server publishes no server list."),
    }
}

/// Routes the hosting paths; `None` for every other path (the clan pages handle those).
pub fn route(h: &Hosting, path: &str) -> Option<Reply> {
    if path == "/server_list.json" {
        return Some(server_list(h));
    }
    // LG-322: no account needed, the client asks before its login
    if path == "/client_version.json" {
        return Some(match &h.client_version {
            Some(j) => served("application/json", j.clone().into_bytes()),
            None => text(404, "This server names no client version."),
        });
    }
    // DC-DEFAULT: LG-313 (proposed): a featured entry's set, no account needed (as `/server_list.json`)
    if let Some(id) = path
        .strip_prefix("/entry_mods/")
        .and_then(|n| n.strip_suffix(".json"))
    {
        return Some(
            match id.parse().ok().and_then(|n: u32| h.entry_sets.get(&n)) {
                Some(j) => served("application/json", j.clone().into_bytes()),
                None => text(404, "This server entry has no mod set."),
            },
        );
    }
    // DC-DEFAULT: MOD-25 (proposed): `/mods/catalog.json` and `/mods/<sha256>.png`; no account needed
    if path == "/mods/catalog.json" {
        return Some(match &h.catalog {
            Some(j) => served("application/json", j.clone().into_bytes()),
            None => text(404, "This server offers no optional mods."),
        });
    }
    if let Some(shot) = path
        .strip_prefix("/mods/")
        .and_then(|n| n.strip_suffix(".png"))
    {
        return Some(match h.catalog_shots.get(shot) {
            Some(b) => served("image/png", b.clone()),
            None => text(404, "No such picture."),
        });
    }
    path.strip_prefix("/mods/").map(|n| mod_package(h, n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc_hosting_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn hosting(d: &Path) -> Hosting {
        Hosting {
            mods_dir: Some(d.to_path_buf()),
            server_list: None,
            catalog: None,
            catalog_shots: Default::default(),
            max_mod_bytes: 1024,
            entry_sets: Default::default(),
            client_version: None,
            folder_packs: Default::default(),
        }
    }

    #[test]
    fn the_client_version_document_comes_from_the_config() {
        use dc_config::schema::ClientSection;
        assert_eq!(client_version_doc(&ClientSection::default()), None);
        let doc = client_version_doc(&ClientSection {
            latest: "0.3.0".into(),
            min_version: "0.2.0".into(),
            update_url: "decombat.com".into(),
        })
        .unwrap();
        for piece in [
            "\"latest\"",
            "\"0.3.0\"",
            "\"min_version\"",
            "\"0.2.0\"",
            "\"decombat.com\"",
        ] {
            assert!(doc.contains(piece), "{doc}");
        }
        let d = dir("ver");
        let mut h = hosting(&d);
        assert_eq!(route(&h, "/client_version.json").unwrap().status, 404);
        h.client_version = Some(doc.clone());
        let r = route(&h, "/client_version.json").unwrap();
        assert_eq!((r.status, r.body), (200, doc.into_bytes()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn package_names_are_exactly_lowercase_hex_zip() {
        let h = "ab".repeat(32);
        assert_eq!(package_hash(&format!("{h}.zip")), Some(h.as_str()));
        for bad in [
            String::new(),
            "x.zip".into(),
            format!("{h}.ZIP"),
            format!("{}.zip", h.to_uppercase()),
            format!("../{h}.zip"),
            format!("{h}.zip/"),
            format!("{}.zip", &h[1..]),
        ] {
            assert_eq!(package_hash(&bad), None, "{bad}");
        }
    }

    #[test]
    fn a_matching_package_is_sent() {
        let d = dir("ok");
        let bytes = b"PK-not-really-a-zip but hashed".to_vec();
        let sha = dc_rom::sha256_hex(&bytes);
        std::fs::write(d.join(format!("{sha}.zip")), &bytes).unwrap();
        let r = mod_package(&hosting(&d), &format!("{sha}.zip"));
        assert_eq!((r.status, r.content_type), (200, "application/zip"));
        assert_eq!(r.body, bytes);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_whose_hash_differs_from_its_name_is_never_sent() {
        let d = dir("bad");
        let name = "cd".repeat(32);
        std::fs::write(d.join(format!("{name}.zip")), b"tampered").unwrap();
        let r = mod_package(&hosting(&d), &format!("{name}.zip"));
        assert_eq!(r.status, 500);
        assert!(!String::from_utf8_lossy(&r.body).contains("tampered"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_oversize_package_is_refused() {
        let d = dir("big");
        let bytes = vec![7u8; 2048];
        let sha = dc_rom::sha256_hex(&bytes);
        std::fs::write(d.join(format!("{sha}.zip")), &bytes).unwrap();
        assert_eq!(mod_package(&hosting(&d), &format!("{sha}.zip")).status, 413);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn unknown_names_and_no_mods_dir_are_404() {
        let d = dir("none");
        let name = "00".repeat(32);
        assert_eq!(
            mod_package(&hosting(&d), &format!("{name}.zip")).status,
            404
        );
        assert_eq!(
            mod_package(&hosting(&d), "..%2f..%2fsecret.zip").status,
            404
        );
        assert_eq!(
            mod_package(&Hosting::none(), &format!("{name}.zip")).status,
            404
        );
        assert_eq!(
            route(&Hosting::none(), "/server_list.json").unwrap().status,
            404
        );
        assert!(route(&Hosting::none(), "/clan").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_mod_folder_is_packed_on_demand_when_its_pack_hashes_to_the_name() {
        let d = dir("folder");
        let m = d.join("m1");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(
            m.join("manifest.json"),
            r#"{"id": "m1", "name": "M", "version": "1.0.0", "entry": "main.lua", "kind": "server_rules", "description": "A test fixture mod for the unit tests.", "author": "tests", "capabilities": ["hooks.lobby"]}"#,
        )
        .unwrap();
        std::fs::write(m.join("main.lua"), "return function(mod) end").unwrap();
        let (bytes, entry) = dc_modkit::pack_mod_bytes(&m).unwrap();
        let mut h = hosting(&d);
        h.max_mod_bytes = 1 << 20;
        let r = mod_package(&h, &format!("{}.zip", entry.sha256));
        assert_eq!(r.status, 200);
        assert_eq!(r.body, bytes);
        assert_eq!(
            mod_package(&h, &format!("{}.zip", "11".repeat(32))).status,
            404
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_catalogue_and_its_pictures_are_served_and_absent_means_404() {
        let mut h = Hosting::none();
        assert_eq!(route(&h, "/mods/catalog.json").unwrap().status, 404);
        h.catalog = Some("{}".into());
        h.catalog_shots.insert("ab".into(), vec![1, 2]);
        assert_eq!(route(&h, "/mods/catalog.json").unwrap().body, b"{}");
        assert_eq!(route(&h, "/mods/ab.png").unwrap().body, vec![1, 2]);
        assert_eq!(route(&h, "/mods/cd.png").unwrap().status, 404);
    }

    #[test]
    fn the_server_list_is_served_as_json() {
        let mut h = Hosting::none();
        h.server_list = Some("{\"name\":\"x\"}".into());
        let r = route(&h, "/server_list.json").unwrap();
        assert_eq!(
            (r.status, r.content_type, r.body.as_slice()),
            (200, "application/json", &b"{\"name\":\"x\"}"[..])
        );
    }
}
