//! The mod sets of servers this client has seen, kept between runs (CLI-1632 amendment, USER REPORT 2026-11-01: after a
//! fresh launch My Mods called a package "Unused" although the modded server declares it). A wrong "Unused" deletes a
//! package a server needs, so what was read at a join or from a server's public `server_list.json` stays known in
//! `<data>/known_mod_sets.json` until it has not been seen for [`MAX_AGE`]. The file holds the address as a key only;
//! no screen shows it (CLI-1623).
// DC-DEFAULT: CLI-1632 (proposed, amended 2026-11-01) the known sets persist, see DC_DEFAULTS.md

use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::mods::ServerModSet;

pub const FILE: &str = "known_mod_sets.json";
/// A set not seen for this long stops counting.
pub const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
/// What a stored name reads as when it is an address (CLI-1623).
const NAMELESS: &str = "A server you know";

/// A server's set and when it was last read.
#[derive(Debug, Clone, PartialEq)]
pub struct KnownSet {
    /// Data only: never drawn (CLI-1623).
    pub addr: SocketAddr,
    pub name: String,
    pub set: ServerModSet,
    pub seen: SystemTime,
}

impl KnownSet {
    /// Still counting at `now`.
    pub fn live(&self, now: SystemTime) -> bool {
        now.duration_since(self.seen).map_or(true, |age| age <= MAX_AGE)
    }
}

fn shown_name(name: &str) -> String {
    let n = name.trim();
    if n.is_empty() || n.parse::<SocketAddr>().is_ok() || n.parse::<std::net::IpAddr>().is_ok() {
        NAMELESS.to_string()
    } else {
        n.to_string()
    }
}

/// The sets of `data_dir` still counting at `now`. No file is an empty list; a corrupt file is ignored with a log line
/// (the next save replaces it), and so is one bad entry.
pub fn load(data_dir: &Path, now: SystemTime) -> Vec<KnownSet> {
    let path = data_dir.join(FILE);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            eprintln!("mods: known sets: cannot read {}: {e}", path.display());
            return Vec::new();
        }
    };
    let doc: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("mods: known sets: ignoring the corrupt {}: {e}", path.display());
            return Vec::new();
        }
    };
    let Some(list) = doc.get("servers").and_then(|s| s.as_array()) else {
        eprintln!("mods: known sets: ignoring the corrupt {}: no server list", path.display());
        return Vec::new();
    };
    list.iter()
        .filter_map(|e| match parse_entry(e) {
            Ok(k) => Some(k),
            Err(why) => {
                eprintln!("mods: known sets: skipping an entry: {why}");
                None
            }
        })
        .filter(|k| k.live(now))
        .collect()
}

fn parse_entry(e: &serde_json::Value) -> Result<KnownSet, String> {
    let addr = e
        .get("server")
        .and_then(|s| s.as_str())
        .and_then(|s| s.parse().ok())
        .ok_or("no server key")?;
    let seen = e.get("seen").and_then(|s| s.as_u64()).ok_or("no time")?;
    let set = ServerModSet::from_fragment(e.get("set").ok_or("no set")?)?;
    Ok(KnownSet {
        addr,
        name: shown_name(e.get("name").and_then(|n| n.as_str()).unwrap_or("")),
        set,
        seen: UNIX_EPOCH + Duration::from_secs(seen),
    })
}

/// Writes the list to `data_dir` through a temporary file. A failure is only logged: the sets stay known for this run.
pub fn save(data_dir: &Path, sets: &[KnownSet]) {
    let servers: Vec<serde_json::Value> = sets
        .iter()
        .map(|k| {
            serde_json::json!({
                "server": k.addr.to_string(),
                "name": k.name,
                "seen": k.seen.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
                "set": k.set.to_fragment(),
            })
        })
        .collect();
    let path = data_dir.join(FILE);
    let tmp = dc_botclient::tls::temp_path(&path);
    let body = serde_json::json!({"version": 1, "servers": servers}).to_string();
    let wrote = std::fs::create_dir_all(data_dir)
        .and_then(|_| std::fs::write(&tmp, body))
        .and_then(|_| std::fs::rename(&tmp, &path));
    if let Err(e) = wrote {
        let _ = std::fs::remove_file(&tmp);
        eprintln!("mods: known sets: cannot write {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::{Policy, ServerMod};

    fn set() -> ServerModSet {
        ServerModSet {
            policy: Policy::Modded { client_visual_allowed: true },
            community_map_base: None,
            client_fov: None,
            mods: vec![ServerMod {
                id: "hacking_mod".into(),
                version: "1.0.0".into(),
                sha256: "ab".repeat(32),
                kind: Some("shared".into()),
                requires_client: true,
                capabilities: vec!["world.full_visibility".into()],
                source_repo: Some("https://example.org/x".into()),
                name: Some("Hacking Mod".into()),
                author: Some("a".into()),
                description: Some("d".into()),
                homepage: None,
                screenshots: vec!["s.png".into()],
                tags: vec!["demo".into()],
                commands: vec![dc_mod_lua::ModCommand { command: "/x".into(), description: "does x".into() }],
                preview: Some("cd".repeat(32)),
            }],
        }
    }

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("dc_knownsets_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn entry(seen: SystemTime, name: &str) -> KnownSet {
        KnownSet { addr: "198.51.100.7:11000".parse().unwrap(), name: name.into(), set: set(), seen }
    }

    #[test]
    fn a_saved_set_comes_back_whole_and_leaves_no_temporary_file() {
        let d = dir("round");
        let seen = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        save(&d, &[entry(seen, "Modded")]);
        let got = load(&d, seen);
        assert_eq!(got, vec![entry(seen, "Modded")]);
        assert_eq!(got[0].set.hash(), set().hash());
        assert!(!dc_botclient::tls::temp_path(&d.join(FILE)).exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_set_stops_counting_after_thirty_days() {
        let d = dir("age");
        let seen = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        save(&d, &[entry(seen, "Modded")]);
        assert_eq!(load(&d, seen + MAX_AGE).len(), 1);
        assert!(load(&d, seen + MAX_AGE + Duration::from_secs(1)).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_corrupt_file_or_entry_is_ignored_and_the_next_save_replaces_it() {
        let d = dir("bad");
        let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        for body in ["{not json", "[]", "{\"servers\": 3}"] {
            std::fs::write(d.join(FILE), body).unwrap();
            assert!(load(&d, now).is_empty(), "{body}");
        }
        std::fs::write(d.join(FILE), "{\"servers\": [{\"server\": \"nowhere\"}]}").unwrap();
        assert!(load(&d, now).is_empty());
        save(&d, &[entry(now, "Modded")]);
        assert_eq!(load(&d, now).len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_stored_address_never_becomes_a_shown_name() {
        let d = dir("name");
        let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        save(&d, &[entry(now, "198.51.100.7:11000")]);
        assert_eq!(load(&d, now)[0].name, NAMELESS);
        let _ = std::fs::remove_dir_all(&d);
    }
}
