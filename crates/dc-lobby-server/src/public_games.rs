//! Optional local match board for an external publisher (docs/LIVE_GAMES.md). DC-DEFAULT: OPS-16 (proposed):
//! `monitor.public_games_file` names a file this lobby rewrites every 30 s with the playing, unlocked matches of its
//! normal server-list entries, labelled as the game client's room list labels them. Nothing else leaves the process.
use crate::room::RoomPhase;
use crate::state::{Core, Shared};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub(crate) fn snapshot(st: &Core) -> Value {
    let entries = crate::servers::entries(st);
    let mut games = Vec::new();
    for (key, room) in &st.rooms {
        if !room.password.is_empty() || room.state != RoomPhase::Playing {
            continue;
        }
        // Only normal server-list entries. Clan channels have their own audience.
        let Some(entry) = entries.iter().find(|e| e.owns(key.channel)) else {
            continue;
        };
        let Some(game) = room.game.and_then(|id| st.games.get(&id)) else {
            continue;
        };
        if game.over || !game.ready || game.room != *key {
            continue;
        }
        let map = map_label(st, key.channel, game.map);
        let mut mode = mode_label(st, room.gun_game, game.mode);
        // The waiting room shows these two; the room list does not.
        if room.possession && !room.gun_game {
            mode.push_str(" + Possession");
        }
        if room.hardcore {
            mode.push_str(" (Hardcore)");
        }
        let players = room
            .members
            .iter()
            .filter(|m| crate::lifecycle::is_human(st, m.account))
            .count();
        games.push(json!({
            "server": entry.name, "channel": key.channel + 1 - entry.first,
            "room": u64::from(key.no) + 1, "map": map, "mode": mode,
            "players": players, "bots": room.members.len() - players, "capacity": room.max_players
        }));
    }
    json!({"version": 1, "at": crate::clock::unix_now(), "games": games})
}

/// The map column of the client's room list: a community map's record name, else the ROM row's `Name`, else `Map n`.
// EVIDENCE: cshell+0x11ed10 (the room row draws the map row's name text; the key is the 4-letter one the client pins)
fn map_label(st: &Core, channel: u32, map: u32) -> String {
    let table = st.maps_of_channel(channel);
    table
        .community_of(map)
        .map(|m| m.record.name.clone())
        .or_else(|| {
            st.rom
                .game_room
                .maps()
                .get(map as usize)?
                .get("Name")
                .cloned()
        })
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("Map {map}"))
}

/// The mode column of the client's room list: a Gun Game room says so, else the mode row's short name (`ModeSimpleName`),
/// else `Mode n`.
// EVIDENCE: cshell+0x11ed10 (the room row draws the mode row's short name; a Gun Game room is our own row, LG-305)
fn mode_label(st: &Core, gun_game: bool, mode: u32) -> String {
    if gun_game {
        return dc_wire::gungame::MODE_NAME.to_string();
    }
    st.rom
        .game_room
        .modes(false)
        .get(mode as usize)
        .and_then(|m| m.get("ModeSimpleName"))
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("Mode {mode}"))
}

/// The configured snapshot path: `None` when the board is off, an error text when the setting cannot be used.
fn target(configured: &str) -> Result<Option<PathBuf>, String> {
    let configured = configured.trim();
    if configured.is_empty() {
        return Ok(None);
    }
    let path = PathBuf::from(configured);
    if !path.is_absolute() {
        return Err("monitor.public_games_file must be an absolute path".into());
    }
    Ok(Some(path))
}

fn write_snapshot(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    // The operator supplies a service-owned directory, one destination per lobby.
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        crate::clock::unix_now()
    ));
    let tmp = PathBuf::from(tmp);
    let mut writer = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)?;
    let result = writer.write_all(bytes);
    // Only this allowlisted artifact is readable by the separate publisher.
    #[cfg(unix)]
    let result = {
        use std::os::unix::fs::PermissionsExt;
        result.and_then(|()| writer.set_permissions(fs::Permissions::from_mode(0o644)))
    };
    drop(writer);
    let result = result.and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

pub fn start(shared: &Shared) {
    let (configured, log) = {
        let st = crate::net::lock(shared);
        (
            st.rules.ops.monitor_public_games_file.clone(),
            st.log.clone(),
        )
    };
    let path = match target(&configured) {
        Ok(Some(path)) => path,
        Ok(None) => return,
        Err(why) => {
            log.line(&why);
            return;
        }
    };
    let shared = Arc::clone(shared);
    std::thread::spawn(move || {
        let mut failed = false;
        loop {
            let data = snapshot(&crate::net::lock(&shared));
            match write_snapshot(&path, data.to_string().as_bytes()) {
                Err(e) if !failed => {
                    log.line(&format!("public match board: {e}"));
                    failed = true;
                }
                Ok(()) if failed => {
                    log.line("public match board: snapshots resumed");
                    failed = false;
                }
                _ => {}
            }
            std::thread::sleep(Duration::from_secs(30));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_is_off_when_empty_and_must_be_absolute() {
        assert_eq!(target("  "), Ok(None));
        assert!(target("games.json").is_err());
        let abs = std::env::temp_dir().join("games.json");
        assert_eq!(
            target(abs.to_str().unwrap()),
            Ok(Some(abs.clone())),
            "an absolute path is used as given"
        );
    }

    #[test]
    fn a_snapshot_replaces_the_file_whole_and_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!("dc-public-games-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("default.json");
        write_snapshot(&path, b"{\"version\":1}").unwrap();
        write_snapshot(&path, b"{\"version\":2}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"version\":2}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        assert!(write_snapshot(&dir.join("missing").join("x.json"), b"x").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn labels_fall_back_to_the_clients_numbered_names_without_a_rom() {
        let st = crate::botgroups::tests::three_entries();
        assert_eq!(map_label(&st, 1, 3), "Map 3");
        assert_eq!(mode_label(&st, false, 2), "Mode 2");
        assert_eq!(mode_label(&st, true, 2), "Gun Game");
    }
}
