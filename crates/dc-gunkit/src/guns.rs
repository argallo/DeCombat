//! The guns of the player's ROM: every weapon row with a name and a model, read at run time. Nothing of it is kept in the repo.
// DC-DEFAULT: VAR-6 (proposed) the studio lists the weapon rows that have a name in the item table and a model

use dc_render::{HandModels, ViewModels};
use dc_rez::RezVfs;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The two models of a gun.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum View {
    /// First person: what the player holds, on the large texture.
    #[default]
    FirstPerson,
    /// Third person: what other players see.
    ThirdPerson,
}

impl View {
    pub fn parse(s: &str) -> Option<View> {
        match s.to_ascii_lowercase().as_str() {
            "first_person" | "first" | "pv" => Some(View::FirstPerson),
            "third_person" | "third" | "hh" => Some(View::ThirdPerson),
            _ => None,
        }
    }

    /// The prefix of a skin key of this view.
    pub fn skin_prefix(self) -> &'static str {
        match self {
            View::FirstPerson => "PVSkin",
            View::ThirdPerson => "HHSkin",
        }
    }

    /// The skin slot a gun's main texture sits in (GUNKIT_PLAN.md section 6: slot 1 first person, slot 0 third person).
    pub fn main_slot(self) -> usize {
        match self {
            View::FirstPerson => 1,
            View::ThirdPerson => 0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            View::FirstPerson => "First person",
            View::ThirdPerson => "Third person",
        }
    }
}

/// One model of a gun: its file, skins and render styles.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GunModel {
    pub model: String,
    pub skins: Vec<String>,
    pub styles: Vec<String>,
}

/// A gun of the list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Gun {
    pub row: u32,
    pub name: String,
    /// The shop sells it (the list shows these first).
    pub on_sale: bool,
    /// The weapon table gives it a sight of its own.
    pub has_scope: bool,
    pub first_person: Option<GunModel>,
    pub third_person: Option<GunModel>,
}

impl Gun {
    pub fn model(&self, view: View) -> Option<&GunModel> {
        match view {
            View::FirstPerson => self.first_person.as_ref(),
            View::ThirdPerson => self.third_person.as_ref(),
        }
    }

    /// What an agent or a list needs to know of it.
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "row": self.row,
            "name": self.name,
            "on_sale": self.on_sale,
            "has_scope": self.has_scope,
            "first_person": self.first_person.is_some(),
            "third_person": self.third_person.is_some(),
        })
    }
}

/// The weapon row key that names the sight a gun comes with.
// OBSERVED: ROM weapon rows: 9 of 10 guns known to have a scope name one here, and none of 12 known not to; EVIDENCE: cshell+0x17bd20, as in
// dc-client attach.rs
const KEY_DEFAULT_SCOPE: &str = "DefaultScopeAttach";

/// A table value: the text between its quotes (a trailing `//` comment dropped), else the trimmed value.
fn table_text(v: &str) -> String {
    let v = v.trim();
    match v.strip_prefix('"').and_then(|r| r.split_once('"')) {
        Some((inner, _)) => inner.trim().to_string(),
        None => v.split("//").next().unwrap_or("").trim().to_string(),
    }
}

/// A name a person can read: table names in another code page are not.
fn readable(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_graphic() || c == ' ')
}

/// Reads the list: the guns on sale first, each part by name.
pub fn read_guns(rom: &Path) -> Result<Vec<Gun>, String> {
    let vfs = RezVfs::new(rom);
    let hands = HandModels::from_vfs(&vfs)?;
    let views = ViewModels::from_vfs(&vfs)?;
    let items = dc_bute::ItemsTable::from_rom(rom).map_err(|e| format!("item table: {e}"))?;
    let mut names = BTreeMap::new();
    for (tag, d) in items.doc().iter() {
        if tag.trim_end_matches(|c: char| c.is_ascii_digit()) != "WeaponItem" {
            continue;
        }
        let Some(id) = d
            .get("ItemID")
            .and_then(|v| table_text(v).parse::<u32>().ok())
        else {
            continue;
        };
        let name = d.get("ItemName").map(|v| table_text(v)).unwrap_or_default();
        if readable(&name) {
            names.entry(id).or_insert(name);
        }
    }
    let mut on_sale = BTreeSet::new();
    if let Ok(shop) = dc_bute::ShopCatalog::from_rom(rom) {
        for pid in shop.view_list() {
            let items = shop
                .product(pid)
                .map(|p| p.items.clone())
                .unwrap_or_default();
            for (index, _, _) in items {
                let Some(row) = shop.info(index) else {
                    continue;
                };
                let Ok(n) = u32::try_from(row.itemid) else {
                    continue;
                };
                on_sale.insert(n);
                let name = table_text(&row.name);
                if readable(&name) {
                    names.entry(n).or_insert(name);
                }
            }
        }
    }
    let weapons = dc_bute::WeaponsTable::from_rom(rom).ok();
    let scoped = |row: u32| {
        weapons
            .as_ref()
            .and_then(|t| t.weapon(row as i32))
            .and_then(|sec| sec.get(KEY_DEFAULT_SCOPE))
            .is_some_and(|v| !table_text(v).is_empty())
    };
    let mut guns = Vec::new();
    for (&row, name) in &names {
        let first_person = views
            .pick(row, None)
            .filter(|m| !m.model.is_empty())
            .map(|m| GunModel {
                model: m.model,
                skins: m.skins,
                styles: m.styles,
            });
        let third_person = hands.rows.get(&row).map(|m| GunModel {
            model: m.model.clone(),
            skins: m.skins.clone(),
            styles: m.styles.clone(),
        });
        if first_person.is_none() && third_person.is_none() {
            continue;
        }
        guns.push(Gun {
            row,
            name: name.clone(),
            on_sale: on_sale.contains(&row),
            has_scope: scoped(row),
            first_person,
            third_person,
        });
    }
    guns.sort_by(|a, b| {
        (!a.on_sale, a.name.to_ascii_lowercase()).cmp(&(!b.on_sale, b.name.to_ascii_lowercase()))
    });
    if guns.is_empty() {
        return Err("no guns found in this game folder".into());
    }
    Ok(guns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_value_loses_its_quotes_and_comment() {
        assert_eq!(table_text(" \"G 36\" // note"), "G 36");
        assert_eq!(table_text("12 // note"), "12");
    }

    #[test]
    fn only_a_readable_name_is_listed() {
        assert!(readable("M16A3"));
        assert!(!readable(""));
        assert!(!readable("\u{d55c}\u{ae00}"));
    }

    #[test]
    fn a_view_names_its_keys_and_main_slot() {
        assert_eq!(View::parse("pv"), Some(View::FirstPerson));
        assert_eq!(View::parse("third_person"), Some(View::ThirdPerson));
        assert_eq!(View::parse("x"), None);
        assert_eq!(View::FirstPerson.skin_prefix(), "PVSkin");
        assert_eq!(View::ThirdPerson.main_slot(), 0);
    }
}
