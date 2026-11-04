//! The map records of a loaded mod set as plain data (`ModHost::map_records`), for the lobby's map table
//! (MAPKIT_PLAN.md sections 5.4 and 6.4) and `dc-modkit validate`.
//!
//! Provenance: ORIGINAL (DeCombat's own mod API).
// DC-DEFAULT: MAP-4 (proposed) the map record, see DC_DEFAULTS.md

use crate::mapkeys::{check_map_record, goals_of, modes_of};
use crate::value::{LuaTable, LuaValue};

/// The radar overlay of a map: the picture and the world-space rectangle (x, z) it covers.
#[derive(Debug, Clone, PartialEq)]
pub struct RadarInfo {
    pub picture: String,
    pub min: [f64; 2],
    pub max: [f64; 2],
}

/// The room sizes a map allows (both ends included).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerRange {
    pub min: u32,
    pub max: u32,
}

/// One community map, as `mod.content.maps:register{...}` declared it (plus the patches of later mods).
#[derive(Debug, Clone, PartialEq)]
pub struct MapRecord {
    /// The mod that registered the map (its `world` and pictures are that mod's assets).
    pub mod_id: String,
    /// The mods that patched the record after the registration, in load order (empty: the record is the
    /// registration as written).
    pub patched_by: Vec<String>,
    /// The map key (`a-z 0-9 _`).
    pub id: String,
    /// The room-list label.
    pub name: String,
    pub description: Option<String>,
    /// `@mod:<mod_id>/assets/...dat`, as `mod.assets:path` returned it.
    pub world: String,
    /// DeCombat mode names (`dc_config::modes::MODE_NAMES`), in the record's order.
    pub modes: Vec<String>,
    /// Default goal per mode, sorted by mode name; only modes of `modes`.
    pub goals: Vec<(String, u32)>,
    pub round_seconds: Option<u32>,
    pub picture: Option<String>,
    pub loading: Option<String>,
    pub radar: Option<RadarInfo>,
    pub players: Option<PlayerRange>,
}

fn opt_string(rec: &LuaTable, key: &str) -> Option<String> {
    match rec.get_str(key) {
        LuaValue::String(s) => Some(s),
        _ => None,
    }
}

fn pair(t: &LuaTable, key: &str) -> [f64; 2] {
    let p = t.get_str(key).as_table().expect("checked radar pair");
    let p = p.borrow();
    [
        p.get_int(1).as_f64().unwrap_or(0.0),
        p.get_int(2).as_f64().unwrap_or(0.0),
    ]
}

impl MapRecord {
    /// Converts a checked record table (see [`check_map_record`], run again here) into plain data.
    pub fn from_table(
        mod_id: &str,
        patched_by: Vec<String>,
        rec: &LuaTable,
    ) -> Result<MapRecord, String> {
        check_map_record(mod_id, rec)?;
        let modes = modes_of(rec)?;
        let goals = goals_of(rec, &modes)?;
        let radar = rec.get_str("radar").as_table().map(|t| {
            let t = t.borrow();
            RadarInfo {
                picture: opt_string(&t, "picture").unwrap_or_default(),
                min: pair(&t, "min"),
                max: pair(&t, "max"),
            }
        });
        let players = rec.get_str("players").as_table().map(|t| {
            let t = t.borrow();
            PlayerRange {
                min: t.get_str("min").as_i64().unwrap_or(1) as u32,
                max: t.get_str("max").as_i64().unwrap_or(1) as u32,
            }
        });
        Ok(MapRecord {
            mod_id: mod_id.to_string(),
            patched_by,
            id: opt_string(rec, "id").unwrap_or_default(),
            name: opt_string(rec, "name").unwrap_or_default(),
            description: opt_string(rec, "description"),
            world: opt_string(rec, "world").unwrap_or_default(),
            modes,
            goals,
            round_seconds: rec.get_str("round_seconds").as_i64().map(|n| n as u32),
            picture: opt_string(rec, "picture"),
            loading: opt_string(rec, "loading"),
            radar,
            players,
        })
    }

    /// The `assets/...` path of the world inside the owning mod's folder.
    pub fn world_rel_path(&self) -> Option<&str> {
        crate::mapkeys::own_asset_rel_path(&self.mod_id, &self.world)
    }

    /// The default goal of a mode, when the record sets one.
    pub fn goal_of(&self, mode: &str) -> Option<u32> {
        self.goals.iter().find(|(m, _)| m == mode).map(|(_, g)| *g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn a_record_table_becomes_plain_data() {
        let s = |t: &str| LuaValue::String(t.into());
        let n = |v: f64| LuaValue::Number(v);
        let mut t = LuaTable::new();
        t.set_str("id", s("valley"));
        t.set_str("name", s("Valley"));
        t.set_str("world", s("@mod:m/assets/maps/valley.dat"));
        let mut modes = LuaTable::new();
        modes.set_int(1, s("BOMB"));
        modes.set_int(2, s("TDM"));
        t.set_str("modes", LuaValue::Table(Rc::new(RefCell::new(modes))));
        let mut goals = LuaTable::new();
        goals.set_str("TDM", n(80.0));
        t.set_str("goals", LuaValue::Table(Rc::new(RefCell::new(goals))));
        t.set_str("round_seconds", n(300.0));
        let mut players = LuaTable::new();
        players.set_str("min", n(2.0));
        players.set_str("max", n(10.0));
        t.set_str("players", LuaValue::Table(Rc::new(RefCell::new(players))));
        let r = MapRecord::from_table("m", vec!["p".into()], &t).unwrap();
        assert_eq!(r.id, "valley");
        assert_eq!(r.modes, vec!["BOMB", "TDM"]);
        assert_eq!(r.goal_of("TDM"), Some(80));
        assert_eq!(r.goal_of("BOMB"), None);
        assert_eq!(r.round_seconds, Some(300));
        assert_eq!(r.players, Some(PlayerRange { min: 2, max: 10 }));
        assert_eq!(r.radar, None);
        assert_eq!(r.world_rel_path(), Some("assets/maps/valley.dat"));
        assert_eq!(r.patched_by, vec!["p"]);
        // the check runs here too
        t.set_str("name", LuaValue::Nil);
        assert!(MapRecord::from_table("m", vec![], &t)
            .unwrap_err()
            .contains("missing required key 'name'"));
    }
}
