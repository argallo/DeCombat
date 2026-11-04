//! The map record of `mod.content.maps:register{...}` (MAPKIT_PLAN.md section 5.4): the key vocabulary, the
//! limits and the check the host runs at `:register` / `:patch`, so a misspelled key, a missing field or a world
//! path that is not the mod's own asset refuses the mod at load, before any lobby lists the map.
//!
//! Provenance: ORIGINAL (DeCombat's own mod API; the 2009 client plays ROM maps only).
// DC-DEFAULT: MAP-4 (proposed) the map record: keys, required fields and limits, see DC_DEFAULTS.md
// DC-DEFAULT: MAP-5 (proposed) the world is the registering mod's own asset, see DC_DEFAULTS.md

use crate::value::{LuaKey, LuaTable, LuaValue};

/// The keys a map record may carry, required ones first, in the order the guide lists them.
pub const MAP_RECORD_KEYS: &[&str] = &[
    "id",
    "name",
    "world",
    "modes",
    "description",
    "goals",
    "round_seconds",
    "picture",
    "loading",
    "radar",
    "players",
];
/// The required keys (the first entries of [`MAP_RECORD_KEYS`]).
pub const MAP_REQUIRED_KEYS: &[&str] = &["id", "name", "world", "modes"];
/// The keys a `:patch` replaces whole instead of deep-merging: lists (and the small tables that act as one
/// value) are values, so `modes = { "FFA" }` leaves exactly FFA and a shrunk `modes` can drop its `goals`.
pub const MAP_REPLACED_KEYS: &[&str] = &["modes", "goals", "players", "radar"];
/// The keys of the `radar` table.
pub const RADAR_KEYS: &[&str] = &["picture", "min", "max"];
/// The keys of the `players` table.
pub const PLAYERS_KEYS: &[&str] = &["min", "max"];

/// `id`: `a-z 0-9 _`, this many characters at most (the map key the lobby lists the map under).
pub const MAP_ID_MAX: usize = 32;
/// `name`: characters at most (the room list label).
pub const MAP_NAME_MAX: usize = 48;
/// `description`: characters at most (the room settings panel).
pub const MAP_DESCRIPTION_MAX: usize = 400;
/// `round_seconds`: both ends allowed.
pub const ROUND_SECONDS_RANGE: (u32, u32) = (30, 3600);
/// `players.min` / `players.max`: both ends allowed (16 is the room size cap, `max_players_cap`).
pub const PLAYERS_RANGE: (u32, u32) = (1, 16);
/// The extension a `world` path must carry (a version-85 world file).
pub const WORLD_EXTENSION: &str = ".dat";

/// Whether `id` is a valid map key: `a-z 0-9 _`, 1..=[`MAP_ID_MAX`] characters.
pub fn is_valid_map_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAP_ID_MAX
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// The prefix every own asset of `mod_id` carries, as `mod.assets:path` returns it.
pub fn own_asset_prefix(mod_id: &str) -> String {
    format!("@mod:{mod_id}/assets/")
}

/// The `assets/...` path inside the mod folder of an own-asset string (`@mod:<mod_id>/assets/<rest>`), or `None`
/// when the string names another mod's asset, a ROM path or a path outside `assets/`.
pub fn own_asset_rel_path<'a>(mod_id: &str, uri: &'a str) -> Option<&'a str> {
    let rest = uri.strip_prefix("@mod:")?;
    let (owner, rel) = rest.split_once('/')?;
    if owner != mod_id || !rel.starts_with("assets/") {
        return None;
    }
    Some(rel)
}

fn type_of(v: &LuaValue) -> String {
    match v {
        LuaValue::String(s) => format!("the string {s:?}"),
        other => format!("a {}", other.type_name()),
    }
}

fn text(rec: &LuaTable, key: &str, max: usize) -> Result<Option<String>, String> {
    match rec.get_str(key) {
        LuaValue::Nil => Ok(None),
        LuaValue::String(s) => {
            let n = s.chars().count();
            if s.trim().is_empty() {
                return Err(format!("'{key}' must not be empty"));
            }
            if n > max {
                return Err(format!(
                    "'{key}' is {n} characters; it must be at most {max}"
                ));
            }
            if s.chars().any(char::is_control) {
                return Err(format!("'{key}' must not contain control characters"));
            }
            Ok(Some(s))
        }
        other => Err(format!("'{key}' must be a string, got {}", type_of(&other))),
    }
}

fn picture(rec: &LuaTable, key: &str) -> Result<Option<String>, String> {
    match rec.get_str(key) {
        LuaValue::Nil => Ok(None),
        LuaValue::String(s) if !s.trim().is_empty() && !s.chars().any(char::is_control) => {
            Ok(Some(s))
        }
        other => Err(format!(
            "'{key}' must be a texture path string (a ROM path or mod.assets:path(...)), got {}",
            type_of(&other)
        )),
    }
}

fn integer(v: &LuaValue, what: &str, range: (u32, u32)) -> Result<u32, String> {
    match v.as_i64() {
        Some(n) if n >= i64::from(range.0) && n <= i64::from(range.1) => Ok(n as u32),
        _ => Err(format!(
            "{what} must be a whole number from {} to {}, got {}",
            range.0,
            range.1,
            type_of(v)
        )),
    }
}

fn no_unknown_keys(table: &LuaTable, what: &str, keys: &[&str]) -> Result<(), String> {
    for (k, _) in table.entries() {
        let name = match k {
            LuaKey::String(s) if keys.contains(&s.as_str()) => continue,
            LuaKey::String(s) => s.clone(),
            other => other.to_value().to_display_string(),
        };
        return Err(format!(
            "unknown key '{name}' in {what}; the keys are: {}",
            keys.join(", ")
        ));
    }
    Ok(())
}

/// A `{x, z}` pair of finite numbers.
fn pair(table: &LuaTable, key: &str) -> Result<[f64; 2], String> {
    let what = format!("radar.{key}");
    let LuaValue::Table(t) = table.get_str(key) else {
        return Err(format!(
            "'{what}' must be a table {{ x, z }} of two numbers"
        ));
    };
    let t = t.borrow();
    if t.len() != 2 || t.seq_len() != 2 {
        return Err(format!(
            "'{what}' must be a table {{ x, z }} of two numbers"
        ));
    }
    let mut out = [0.0; 2];
    for (i, slot) in out.iter_mut().enumerate() {
        match t.get_int(i as i64 + 1).as_f64() {
            Some(n) if n.is_finite() => *slot = n,
            _ => {
                return Err(format!(
                    "'{what}' must be a table {{ x, z }} of two numbers"
                ))
            }
        }
    }
    Ok(out)
}

/// The record's `modes` list as names, checked: a non-empty sequence of `dc_config::modes::MODE_NAMES` entries
/// without duplicates.
pub fn modes_of(rec: &LuaTable) -> Result<Vec<String>, String> {
    let vocabulary = || dc_config::modes::MODE_NAMES.join(", ");
    let LuaValue::Table(t) = rec.get_str("modes") else {
        return Err(format!(
            "'modes' must be a non-empty list of mode names; the names are: {}",
            vocabulary()
        ));
    };
    let t = t.borrow();
    let n = t.seq_len();
    if n == 0 || t.len() != n {
        return Err(format!(
            "'modes' must be a non-empty list of mode names; the names are: {}",
            vocabulary()
        ));
    }
    let mut out: Vec<String> = Vec::with_capacity(n);
    for i in 1..=n as i64 {
        let v = t.get_int(i);
        let Some(name) = v.as_str() else {
            return Err(format!(
                "'modes' entry {i} must be a mode name string, got {}",
                type_of(&v)
            ));
        };
        if !dc_config::modes::is_mode_name(name) {
            return Err(format!(
                "'modes' entry {i} {name:?} is not a mode name; the names are: {}",
                vocabulary()
            ));
        }
        if out.iter().any(|m| m == name) {
            return Err(format!("'modes' lists {name:?} twice"));
        }
        out.push(name.to_string());
    }
    Ok(out)
}

/// The record's `goals` table as (mode, goal) pairs in key order, checked against the record's `modes`.
pub fn goals_of(rec: &LuaTable, modes: &[String]) -> Result<Vec<(String, u32)>, String> {
    let LuaValue::Table(t) = rec.get_str("goals") else {
        return match rec.get_str("goals") {
            LuaValue::Nil => Ok(Vec::new()),
            other => Err(format!(
                "'goals' must be a table of mode name -> goal, got {}",
                type_of(&other)
            )),
        };
    };
    let t = t.borrow();
    let mut out = Vec::new();
    for (k, v) in t.entries() {
        let LuaKey::String(mode) = k else {
            return Err(format!(
                "'goals' keys must be mode names, got {}",
                k.to_value().to_display_string()
            ));
        };
        if !modes.iter().any(|m| m == mode) {
            return Err(format!(
                "'goals' names mode {mode:?}, which 'modes' does not list"
            ));
        }
        let goal = match v.as_i64() {
            Some(n) if n > 0 && n <= i64::from(u32::MAX) => n as u32,
            _ => {
                return Err(format!(
                    "'goals.{mode}' must be a positive whole number, got {}",
                    type_of(v)
                ))
            }
        };
        out.push((mode.clone(), goal));
    }
    out.sort();
    Ok(out)
}

/// Checks a map record (`:register` definition or the merged result of a `:patch`) of the mod `mod_id`: the
/// required keys, every value's shape and limits, the key spelling (unknown keys name the vocabulary, as the
/// manifest's "Unknown capability" message does) and the world path, which must be the mod's own
/// `assets/...` file with the `.dat` extension, as `mod.assets:path` returns it.
pub fn check_map_record(mod_id: &str, rec: &LuaTable) -> Result<(), String> {
    no_unknown_keys(rec, "the map record", MAP_RECORD_KEYS)?;
    for key in MAP_REQUIRED_KEYS {
        if matches!(rec.get_str(key), LuaValue::Nil) {
            return Err(format!(
                "missing required key '{key}'; the required keys are: {}",
                MAP_REQUIRED_KEYS.join(", ")
            ));
        }
    }
    // id
    match rec.get_str("id") {
        LuaValue::String(id) if is_valid_map_id(&id) => {}
        other => {
            return Err(format!(
                "'id' must be the map key: 1 to {MAP_ID_MAX} characters of a-z 0-9 _, got {}",
                type_of(&other)
            ))
        }
    }
    // name, description
    text(rec, "name", MAP_NAME_MAX)?;
    text(rec, "description", MAP_DESCRIPTION_MAX)?;

    // world: this mod's own asset, a .dat under assets/
    match rec.get_str("world") {
        LuaValue::String(world) => {
            let prefix = own_asset_prefix(mod_id);
            let rel = own_asset_rel_path(mod_id, &world).ok_or_else(|| {
                format!(
                    "'world' must be this mod's own asset {prefix}...{WORLD_EXTENSION} as mod.assets:path(\"assets/...\") returns it, got {world:?}"
                )
            })?;
            let clean = crate::host::validate_asset_rel_path(rel).map_err(|e| {
                format!("'world' {world:?} is not a valid asset path: {e}")
            })?;
            if clean != rel || !rel.ends_with(WORLD_EXTENSION) || rel.len() <= 7 + WORLD_EXTENSION.len()
            {
                return Err(format!(
                    "'world' must be this mod's own asset {prefix}...{WORLD_EXTENSION} as mod.assets:path(\"assets/...\") returns it, got {world:?}"
                ));
            }
        }
        other => {
            return Err(format!(
                "'world' must be a string from mod.assets:path(\"assets/...{WORLD_EXTENSION}\"), got {}",
                type_of(&other)
            ))
        }
    }

    // modes, goals
    let modes = modes_of(rec)?;
    goals_of(rec, &modes)?;

    // round_seconds
    match rec.get_str("round_seconds") {
        LuaValue::Nil => {}
        v => {
            integer(&v, "'round_seconds'", ROUND_SECONDS_RANGE)?;
        }
    }

    // pictures
    picture(rec, "picture")?;
    picture(rec, "loading")?;

    // radar
    match rec.get_str("radar") {
        LuaValue::Nil => {}
        LuaValue::Table(t) => {
            let t = t.borrow();
            no_unknown_keys(&t, "'radar'", RADAR_KEYS)?;
            if picture(&t, "picture")?.is_none() {
                return Err("'radar' needs a 'picture'".to_string());
            }
            let min = pair(&t, "min")?;
            let max = pair(&t, "max")?;
            if min[0] >= max[0] || min[1] >= max[1] {
                return Err(format!(
                    "'radar' min {{ {}, {} }} must be below max {{ {}, {} }} on both axes",
                    min[0], min[1], max[0], max[1]
                ));
            }
        }
        other => {
            return Err(format!(
                "'radar' must be a table {{ picture, min = {{ x, z }}, max = {{ x, z }} }}, got {}",
                type_of(&other)
            ))
        }
    }

    // players
    match rec.get_str("players") {
        LuaValue::Nil => {}
        LuaValue::Table(t) => {
            let t = t.borrow();
            no_unknown_keys(&t, "'players'", PLAYERS_KEYS)?;
            let min = integer(&t.get_str("min"), "'players.min'", PLAYERS_RANGE)?;
            let max = integer(&t.get_str("max"), "'players.max'", PLAYERS_RANGE)?;
            if min > max {
                return Err(format!("'players' min {min} must not be above max {max}"));
            }
        }
        other => {
            return Err(format!(
                "'players' must be a table {{ min, max }}, got {}",
                type_of(&other)
            ))
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn s(t: &str) -> LuaValue {
        LuaValue::String(t.into())
    }
    fn n(v: f64) -> LuaValue {
        LuaValue::Number(v)
    }
    fn list(items: &[&str]) -> LuaValue {
        let mut t = LuaTable::new();
        for (i, it) in items.iter().enumerate() {
            t.set_int(i as i64 + 1, s(it));
        }
        LuaValue::Table(Rc::new(RefCell::new(t)))
    }
    fn table(pairs: &[(&str, LuaValue)]) -> LuaValue {
        let mut t = LuaTable::new();
        for (k, v) in pairs {
            t.set_str(*k, v.clone());
        }
        LuaValue::Table(Rc::new(RefCell::new(t)))
    }
    fn minimal() -> LuaTable {
        let mut t = LuaTable::new();
        t.set_str("id", s("valley"));
        t.set_str("name", s("Valley"));
        t.set_str("world", s("@mod:m/assets/maps/valley.dat"));
        t.set_str("modes", list(&["TDM", "FFA"]));
        t
    }

    #[test]
    fn a_minimal_and_a_full_record_pass() {
        assert_eq!(check_map_record("m", &minimal()), Ok(()));
        let mut t = minimal();
        t.set_str("description", s("A valley."));
        t.set_str("goals", table(&[("TDM", n(100.0)), ("FFA", n(50.0))]));
        t.set_str("round_seconds", n(600.0));
        t.set_str("picture", s("@mod:m/assets/pictures/valley.dtx"));
        t.set_str("loading", s("@mod:m/assets/pictures/valley_load.dtx"));
        t.set_str(
            "radar",
            table(&[
                ("picture", s("@mod:m/assets/pictures/valley_radar.dtx")),
                ("min", {
                    let mut p = LuaTable::new();
                    p.set_int(1, n(-2048.0));
                    p.set_int(2, n(-2048.0));
                    LuaValue::Table(Rc::new(RefCell::new(p)))
                }),
                ("max", {
                    let mut p = LuaTable::new();
                    p.set_int(1, n(2048.0));
                    p.set_int(2, n(2048.0));
                    LuaValue::Table(Rc::new(RefCell::new(p)))
                }),
            ]),
        );
        t.set_str("players", table(&[("min", n(2.0)), ("max", n(16.0))]));
        assert_eq!(check_map_record("m", &t), Ok(()));
        assert_eq!(modes_of(&t).unwrap(), vec!["TDM", "FFA"]);
        assert_eq!(
            goals_of(&t, &modes_of(&t).unwrap()).unwrap(),
            vec![("FFA".to_string(), 50), ("TDM".to_string(), 100)]
        );
    }

    #[test]
    fn every_rule_refuses_with_a_message_naming_the_key() {
        let check = |edit: &dyn Fn(&mut LuaTable)| {
            let mut t = minimal();
            edit(&mut t);
            check_map_record("m", &t).unwrap_err()
        };
        let e = check(&|t| t.set_str("foo", n(1.0)));
        assert!(
            e.contains("unknown key 'foo'") && e.contains("id, name, world, modes"),
            "{e}"
        );
        let e = check(&|t| t.set_str("name", LuaValue::Nil));
        assert!(e.contains("missing required key 'name'"), "{e}");
        let e = check(&|t| t.set_str("id", s("Valley")));
        assert!(e.contains("'id' must be the map key"), "{e}");
        let e = check(&|t| t.set_str("id", s(&"a".repeat(33))));
        assert!(e.contains("'id' must be the map key"), "{e}");
        let e = check(&|t| t.set_str("id", n(7.0)));
        assert!(e.contains("'id' must be the map key"), "{e}");
        let e = check(&|t| t.set_str("name", s(&"n".repeat(49))));
        assert!(
            e.contains("'name' is 49 characters; it must be at most 48"),
            "{e}"
        );
        let e = check(&|t| t.set_str("name", s("  ")));
        assert!(e.contains("'name' must not be empty"), "{e}");
        let e = check(&|t| t.set_str("description", s(&"d".repeat(401))));
        assert!(e.contains("'description' is 401 characters"), "{e}");
        // world: another mod, a ROM path, outside assets/, wrong extension, a number
        for bad in [
            "@mod:other/assets/maps/valley.dat",
            "Worlds/valley.dat",
            "@mod:m/maps/valley.dat",
            "@mod:m/assets/maps/valley.DAT",
            "@mod:m/assets/maps/valley.lua",
            "@mod:m/assets/.dat",
        ] {
            let e = check(&|t| t.set_str("world", s(bad)));
            assert!(
                e.contains("'world' must be this mod's own asset @mod:m/assets/..."),
                "{bad}: {e}"
            );
        }
        let e = check(&|t| t.set_str("world", n(1.0)));
        assert!(e.contains("'world' must be a string"), "{e}");
        // modes
        let e = check(&|t| t.set_str("modes", list(&[])));
        assert!(
            e.contains("'modes' must be a non-empty list") && e.contains("TDM, TDM_CLAN"),
            "{e}"
        );
        let e = check(&|t| t.set_str("modes", s("TDM")));
        assert!(e.contains("'modes' must be a non-empty list"), "{e}");
        let e = check(&|t| t.set_str("modes", list(&["TDM", "tdm"])));
        assert!(
            e.contains("'modes' entry 2 \"tdm\" is not a mode name"),
            "{e}"
        );
        let e = check(&|t| t.set_str("modes", list(&["TDM", "TDM"])));
        assert!(e.contains("'modes' lists \"TDM\" twice"), "{e}");
        let e = check(&|t| t.set_str("modes", table(&[("TDM", n(1.0))])));
        assert!(e.contains("'modes' must be a non-empty list"), "{e}");
        // goals
        let e = check(&|t| t.set_str("goals", table(&[("BOMB", n(5.0))])));
        assert!(
            e.contains("'goals' names mode \"BOMB\", which 'modes' does not list"),
            "{e}"
        );
        let e = check(&|t| t.set_str("goals", table(&[("TDM", n(0.0))])));
        assert!(
            e.contains("'goals.TDM' must be a positive whole number"),
            "{e}"
        );
        let e = check(&|t| t.set_str("goals", table(&[("TDM", n(1.5))])));
        assert!(
            e.contains("'goals.TDM' must be a positive whole number"),
            "{e}"
        );
        let e = check(&|t| t.set_str("goals", n(3.0)));
        assert!(e.contains("'goals' must be a table"), "{e}");
        // round_seconds
        for bad in [29.0, 3601.0, 60.5] {
            let e = check(&|t| t.set_str("round_seconds", n(bad)));
            assert!(
                e.contains("'round_seconds' must be a whole number from 30 to 3600"),
                "{e}"
            );
        }
        // pictures
        let e = check(&|t| t.set_str("picture", n(1.0)));
        assert!(e.contains("'picture' must be a texture path string"), "{e}");
        let e = check(&|t| t.set_str("loading", s("")));
        assert!(e.contains("'loading' must be a texture path string"), "{e}");
        // radar
        let e = check(&|t| t.set_str("radar", s("x")));
        assert!(e.contains("'radar' must be a table"), "{e}");
        let e = check(&|t| t.set_str("radar", table(&[("picture", s("p"))])));
        assert!(e.contains("'radar.min' must be a table { x, z }"), "{e}");
        let e = check(&|t| t.set_str("radar", table(&[("min", list(&["a", "b"]))])));
        assert!(e.contains("'radar' needs a 'picture'"), "{e}");
        let e = check(&|t| t.set_str("radar", table(&[("picture", s("p")), ("zoom", n(1.0))])));
        assert!(
            e.contains("unknown key 'zoom' in 'radar'") && e.contains("picture, min, max"),
            "{e}"
        );
        let pair = |a: f64, b: f64| {
            let mut p = LuaTable::new();
            p.set_int(1, n(a));
            p.set_int(2, n(b));
            LuaValue::Table(Rc::new(RefCell::new(p)))
        };
        let e = check(&|t| {
            t.set_str(
                "radar",
                table(&[
                    ("picture", s("p")),
                    ("min", pair(0.0, 0.0)),
                    ("max", pair(10.0, 0.0)),
                ]),
            )
        });
        assert!(
            e.contains("'radar' min { 0, 0 } must be below max { 10, 0 }"),
            "{e}"
        );
        // players
        let e = check(&|t| t.set_str("players", table(&[("min", n(4.0))])));
        assert!(
            e.contains("'players.max' must be a whole number from 1 to 16"),
            "{e}"
        );
        let e = check(&|t| t.set_str("players", table(&[("min", n(8.0)), ("max", n(4.0))])));
        assert!(e.contains("'players' min 8 must not be above max 4"), "{e}");
        let e = check(&|t| t.set_str("players", table(&[("min", n(1.0)), ("max", n(17.0))])));
        assert!(
            e.contains("'players.max' must be a whole number from 1 to 16"),
            "{e}"
        );
        let e = check(&|t| {
            t.set_str(
                "players",
                table(&[("min", n(1.0)), ("max", n(2.0)), ("bots", n(2.0))]),
            )
        });
        assert!(e.contains("unknown key 'bots' in 'players'"), "{e}");
    }

    #[test]
    fn own_asset_paths_are_recognised() {
        assert_eq!(
            own_asset_rel_path("m", "@mod:m/assets/maps/a.dat"),
            Some("assets/maps/a.dat")
        );
        assert_eq!(own_asset_rel_path("m", "@mod:n/assets/maps/a.dat"), None);
        assert_eq!(own_asset_rel_path("m", "@mod:m/overrides/a.dat"), None);
        assert_eq!(own_asset_rel_path("m", "Worlds/a.dat"), None);
        assert!(is_valid_map_id("crossfire_valley_2"));
        assert!(!is_valid_map_id("Crossfire"));
        assert!(!is_valid_map_id("a-b"));
        assert!(!is_valid_map_id(""));
    }
}
