//! MOD-21: mod content patches of the weapon table, as plain data.
//!
//! A server's mod set may change rows of the ROM weapon table (`[Weapon<n>]` and `[Ammo<n>]` of `WEAPONS.TXT`)
//! through `mod.content.weapons:patch(id, partial)` / `:register(def)`. The Lua host turns those records into a
//! [`WeaponContent`]; the lobby writes it as a small JSON document and hands the path to every game server it
//! starts (`-content`), and the client applies the same document to its own copy of the table. Neither the game
//! server nor the client runs Lua for this: they only read the document and overwrite keys.
//!
//! The document (version 1), canonical form (no spaces, rows by id, fields by key):
//!
//! ```text
//! {"version":1,"weapons":[{"weapon":145,"op":"patch","fields":{"AmmoDamage":"41"}}],
//!  "ammo":[{"ammo":3,"op":"register","fields":{"InstDamage":"30","Name":"Pellet"}}]}
//! ```
//!
//! Field values are bute value text (`41`, `0.5`, `Pellet`), stored as the bute parser stores a value (trimmed,
//! surrounding `"` removed). Apply rules: a `patch` needs the row to exist and overwrites (or adds) the keys it
//! names, other keys stay; a `register` needs the row NOT to exist and creates it with exactly its keys. Any key may
//! be set, known to the tables or not. A document that breaks a rule is refused as a whole; nothing is applied.
//! The content hash is the SHA-256 of the canonical form (lower-case hex), whatever spacing the file had.
// DC-DEFAULT: MOD-21 (proposed), see DC_DEFAULTS.md

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use crate::{ButeDoc, SectionMap, WeaponsTable};

/// The document version this module reads and writes.
pub const CONTENT_FORMAT_VERSION: u64 = 1;
/// Section prefix of a weapon row.
pub const WEAPON_SECTION: &str = "Weapon";
/// Section prefix of an ammunition row.
pub const AMMO_SECTION: &str = "Ammo";
/// Longest accepted key (bytes).
pub const MAX_KEY_LEN: usize = 64;
/// Longest accepted value (bytes).
pub const MAX_VALUE_LEN: usize = 1024;
/// Most keys one row patch may carry.
pub const MAX_FIELDS_PER_ROW: usize = 512;

/// The item numbers reserved for curated pack weapons (LG-314): the top of the client's weapon number range (a weapon
/// is numbered by its row, below 1000). The test `rom::tests::the_rom_leaves_the_pack_numbers_free` checks the ROM has
/// no row here. A curated pack may `:register` weapon rows only inside it.
// DC-DEFAULT: LG-314 (proposed, project decision), see DC_DEFAULTS.md
pub const PACK_ITEM_FIRST: i64 = 900;
pub const PACK_ITEM_LAST: i64 = 999;

/// The item numbers reserved for weapon variants (VAR-1): a variant is a row of its own with the base gun's every
/// gameplay key. The ROM's rows stop well below these, and the numbers from 1000 up mean something else to the client.
// DC-DEFAULT: VAR-1 (proposed; docs/GUNKIT_PLAN.md section 6)
pub const VARIANT_ITEM_FIRST: i64 = 500;
pub const VARIANT_ITEM_LAST: i64 = 899;
/// The key of a registered row that makes it a variant of the named row.
pub const KEY_VARIANT_OF: &str = "variant_of";
/// The skin key another player sees a gun in, numbered from 0.
const KEY_HH_SKIN: &str = "HHSkin";
/// The skin key its owner sees a gun in, numbered from 0.
const KEY_PV_SKIN: &str = "PVSkin";
/// A variant's own name and slot image in the menus (optional; the base gun's otherwise).
pub const KEY_VARIANT_NAME: &str = "variant_name";
pub const KEY_VARIANT_ICON: &str = "variant_icon";
/// The key of a variant that wears the look of another row (a model the ROM already has), by row number.
pub const KEY_VARIANT_LOOK_OF: &str = "variant_look_of";
/// A variant's rarity tier (CRATE-6): how often a Camo Crate gives it and how its opening is shown.
pub const KEY_VARIANT_RARITY: &str = "variant_rarity";

/// A variant's rarity tier (CRATE-6), lowest first. A variant without the key is [`Rarity::Common`].
// DC-DEFAULT: CRATE-6 (proposed; USER DECISION 2026-11-06 "weighted odds": rarer camos drop less often)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Rarity {
    #[default]
    Common,
    Rare,
    Epic,
    Legendary,
}

impl Rarity {
    pub const ALL: [Rarity; 4] = [
        Rarity::Common,
        Rarity::Rare,
        Rarity::Epic,
        Rarity::Legendary,
    ];

    /// The key's value: one of the names, any case.
    pub fn parse(text: &str) -> Option<Rarity> {
        let t = text.trim().trim_matches('"');
        Rarity::ALL
            .into_iter()
            .find(|r| r.key().eq_ignore_ascii_case(t))
    }

    /// The value the key takes.
    pub fn key(self) -> &'static str {
        match self {
            Rarity::Common => "common",
            Rarity::Rare => "rare",
            Rarity::Epic => "epic",
            Rarity::Legendary => "legendary",
        }
    }

    /// The name players see.
    pub fn name(self) -> &'static str {
        match self {
            Rarity::Common => "Common",
            Rarity::Rare => "Rare",
            Rarity::Epic => "Epic",
            Rarity::Legendary => "Legendary",
        }
    }

    /// 0 for Common up to 3 for Legendary.
    pub fn index(self) -> usize {
        self as usize
    }
}

/// The base row a weapon row's text says it is a variant of (VAR-1).
pub fn variant_base(text: Option<&str>) -> Option<u16> {
    text?.trim().trim_matches('"').parse().ok()
}

/// The `<prefix><k>` values of a row, by `k`.
fn numbered_skins(fields: &BTreeMap<String, String>, prefix: &str) -> BTreeMap<usize, String> {
    fields
        .iter()
        .filter_map(|(key, path)| Some((key.strip_prefix(prefix)?.parse().ok()?, path.clone())))
        .collect()
}

/// True for an item number of a pack's new gun or variant (LG-314, VAR-1).
pub fn is_pack_item(item_no: i64) -> bool {
    (PACK_ITEM_FIRST..=PACK_ITEM_LAST).contains(&item_no) || is_variant_item(item_no)
}

/// True for an item number of the variant range (VAR-1).
pub fn is_variant_item(item_no: i64) -> bool {
    (VARIANT_ITEM_FIRST..=VARIANT_ITEM_LAST).contains(&item_no)
}

/// Whether a variant row may set this key (VAR-2): how the gun looks, never how it behaves. Everything else comes
/// from the base row, so a variant cannot change gameplay.
// DC-DEFAULT: VAR-2 (proposed; USER DECISION 2026-11-02 "variants don't change gameplay")
pub fn is_cosmetic_key(key: &str) -> bool {
    if [
        KEY_VARIANT_OF,
        KEY_VARIANT_NAME,
        KEY_VARIANT_ICON,
        KEY_VARIANT_LOOK_OF,
        KEY_VARIANT_RARITY,
    ]
    .contains(&key)
    {
        return true;
    }
    if key == "HHModel" || key.starts_with("PVModelHand_") {
        return true;
    }
    ["PVSkin", "HHSkin", "PVRenderStyle", "HHRenderStyle"]
        .iter()
        .any(|p| {
            key.strip_prefix(p)
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
}

/// Whether a key is what `variant_look_of` takes from the other row (VAR-5): the models and how they are drawn, and where the
/// first-person model sits. Never its sounds, ammunition, muzzle effects or anything else: a row with the same gun's look may
/// also play differently (a quieter shot is heard from less far), and a variant may not.
// DC-DEFAULT: VAR-5 (proposed; USER DECISION 2026-11-02 "variants don't change gameplay")
fn is_look_key(key: &str) -> bool {
    key == "Pos" || is_model_or_style_key(key)
}

fn is_model_or_style_key(key: &str) -> bool {
    is_cosmetic_key(key)
        && ![
            KEY_VARIANT_OF,
            KEY_VARIANT_NAME,
            KEY_VARIANT_ICON,
            KEY_VARIANT_LOOK_OF,
            KEY_VARIANT_RARITY,
        ]
        .contains(&key)
}

/// One variant of a content document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantInfo {
    pub row: i32,
    pub base: i32,
    pub name: Option<String>,
    pub icon: Option<String>,
    /// The `HHSkin<k>` the variant sets, by `k`: what another player sees it carry.
    pub hh_skins: BTreeMap<usize, String>,
    /// The `PVSkin<k>` the variant sets, by `k`: what its owner sees in first person.
    pub pv_skins: BTreeMap<usize, String>,
    /// The row whose look it wears (VAR-5).
    pub look_of: Option<i32>,
    /// Its rarity tier (CRATE-6; Common without the key).
    pub rarity: Rarity,
}

/// A refused document or apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchError(pub String);

impl fmt::Display for PatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "content patch: {}", self.0)
    }
}

impl std::error::Error for PatchError {}

fn err<T>(msg: impl Into<String>) -> Result<T, PatchError> {
    Err(PatchError(msg.into()))
}

/// What a row patch does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PatchOp {
    /// Overwrite keys of an existing row.
    Patch,
    /// Create a new row (the id must be free).
    Register,
}

impl PatchOp {
    pub fn as_str(self) -> &'static str {
        match self {
            PatchOp::Patch => "patch",
            PatchOp::Register => "register",
        }
    }

    fn parse(s: &str) -> Option<PatchOp> {
        match s {
            "patch" => Some(PatchOp::Patch),
            "register" => Some(PatchOp::Register),
            _ => None,
        }
    }
}

/// One change of a `[Weapon<n>]` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeaponPatch {
    pub weapon: u16,
    pub op: PatchOp,
    /// Raw bute key -> value text.
    pub fields: BTreeMap<String, String>,
}

/// One change of an `[Ammo<n>]` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AmmoPatch {
    pub ammo: u16,
    pub op: PatchOp,
    /// Raw bute key -> value text.
    pub fields: BTreeMap<String, String>,
}

/// The weapon-table content of a mod set: weapon and ammunition row patches, sorted by id, one entry per row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WeaponContent {
    pub weapons: Vec<WeaponPatch>,
    pub ammo: Vec<AmmoPatch>,
}

/// A value as the bute parser stores it: trimmed, one pair of surrounding `"` removed.
pub fn normalize_value(v: &str) -> String {
    let t = v.trim();
    let t = if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t[1..t.len() - 1].trim()
    } else {
        t
    };
    t.to_string()
}

fn check_key(k: &str) -> Result<(), PatchError> {
    if k.is_empty() || k.len() > MAX_KEY_LEN {
        return err(format!("key {k:?}: 1..{MAX_KEY_LEN} bytes"));
    }
    if k.chars()
        .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '=' | '[' | ']' | '"' | '/'))
    {
        return err(format!(
            "key {k:?}: no spaces, control characters, '=', '[', ']', '\"' or '/'"
        ));
    }
    Ok(())
}

fn check_value(k: &str, v: &str) -> Result<(), PatchError> {
    if v.len() > MAX_VALUE_LEN {
        return err(format!("value of {k:?}: at most {MAX_VALUE_LEN} bytes"));
    }
    if v.chars().any(|c| c.is_control()) || v.contains("//") {
        return err(format!(
            "value of {k:?}: no control characters or '//' (a bute line cannot hold them)"
        ));
    }
    Ok(())
}

fn check_fields(
    what: &str,
    fields: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, PatchError> {
    if fields.len() > MAX_FIELDS_PER_ROW {
        return err(format!("{what}: more than {MAX_FIELDS_PER_ROW} keys"));
    }
    let mut out = BTreeMap::new();
    for (k, v) in fields {
        check_key(&k).map_err(|e| PatchError(format!("{what}: {}", e.0)))?;
        let v = normalize_value(&v);
        check_value(&k, &v).map_err(|e| PatchError(format!("{what}: {}", e.0)))?;
        out.insert(k, v);
    }
    Ok(out)
}

/// A row change of either section, for the shared checks and writer.
struct Row<'a> {
    id: u16,
    op: PatchOp,
    fields: &'a BTreeMap<String, String>,
}

fn write_rows(out: &mut String, id_key: &str, rows: &[Row<'_>]) {
    out.push('[');
    for (i, r) in rows.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"{id_key}\":{},\"op\":\"{}\",\"fields\":{{",
            r.id,
            r.op.as_str()
        ));
        for (j, (k, v)) in r.fields.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            out.push_str(&json_string(k));
            out.push(':');
            out.push_str(&json_string(v));
        }
        out.push_str("}}");
    }
    out.push(']');
}

/// A JSON string literal (serde_json's escaping: `"`, `\`, control characters).
fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// One parsed row: its id, the operation and the field values.
type PatchRow = (u16, PatchOp, BTreeMap<String, String>);

fn parse_rows(
    v: Option<&serde_json::Value>,
    list: &str,
    id_key: &str,
) -> Result<Vec<PatchRow>, PatchError> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    let Some(arr) = v.as_array() else {
        return err(format!("\"{list}\" is not a list"));
    };
    let mut out = Vec::new();
    for (i, e) in arr.iter().enumerate() {
        let what = format!("{list}[{i}]");
        let Some(obj) = e.as_object() else {
            return err(format!("{what} is not an object"));
        };
        for k in obj.keys() {
            if k != id_key && k != "op" && k != "fields" {
                return err(format!("{what}: unknown key {k:?}"));
            }
        }
        let id = obj
            .get(id_key)
            .and_then(|x| x.as_u64())
            .filter(|&n| n <= u16::MAX as u64)
            .ok_or_else(|| PatchError(format!("{what}: \"{id_key}\" must be a number 0..65535")))?
            as u16;
        let op = obj
            .get("op")
            .and_then(|x| x.as_str())
            .and_then(PatchOp::parse)
            .ok_or_else(|| {
                PatchError(format!("{what}: \"op\" must be \"patch\" or \"register\""))
            })?;
        let Some(fields) = obj.get("fields").and_then(|x| x.as_object()) else {
            return err(format!("{what}: \"fields\" must be an object"));
        };
        let mut f = BTreeMap::new();
        for (k, v) in fields {
            let Some(s) = v.as_str() else {
                return err(format!("{what}: field {k:?} must be a string"));
            };
            f.insert(k.clone(), s.to_string());
        }
        out.push((id, op, f));
    }
    Ok(out)
}

fn sorted_unique<T>(
    list: &str,
    mut rows: Vec<T>,
    id: impl Fn(&T) -> u16,
) -> Result<Vec<T>, PatchError> {
    rows.sort_by_key(|r| id(r));
    for w in rows.windows(2) {
        if id(&w[0]) == id(&w[1]) {
            return err(format!("{list}: row {} listed twice", id(&w[0])));
        }
    }
    Ok(rows)
}

impl WeaponContent {
    /// Checks and orders a set of row changes: every key and value passes the field rules, values are normalized
    /// ([`normalize_value`]), rows are sorted by id and an id may appear once per section.
    pub fn new(weapons: Vec<WeaponPatch>, ammo: Vec<AmmoPatch>) -> Result<Self, PatchError> {
        let weapons = weapons
            .into_iter()
            .map(|p| {
                // CRATE-6: a tier the lobby and the client do not know is refused where the pack is read
                if let Some(r) = p
                    .fields
                    .get(KEY_VARIANT_RARITY)
                    .filter(|r| Rarity::parse(r).is_none())
                {
                    return Err(PatchError(format!(
                        "weapon {}: {KEY_VARIANT_RARITY} {r:?} is not one of common, rare, epic, legendary",
                        p.weapon
                    )));
                }
                Ok(WeaponPatch {
                    fields: check_fields(&format!("weapon {}", p.weapon), p.fields)?,
                    ..p
                })
            })
            .collect::<Result<Vec<_>, PatchError>>()?;
        let ammo = ammo
            .into_iter()
            .map(|p| {
                Ok(AmmoPatch {
                    fields: check_fields(&format!("ammo {}", p.ammo), p.fields)?,
                    ..p
                })
            })
            .collect::<Result<Vec<_>, PatchError>>()?;
        Ok(WeaponContent {
            weapons: sorted_unique("weapons", weapons, |p| p.weapon)?,
            ammo: sorted_unique("ammo", ammo, |p| p.ammo)?,
        })
    }

    /// The document's variants (VAR-1), by row.
    pub fn variants(&self) -> Vec<VariantInfo> {
        self.weapons
            .iter()
            .filter(|w| w.op == PatchOp::Register)
            .filter_map(|w| {
                let base = w.fields.get(KEY_VARIANT_OF)?.trim().parse::<i32>().ok()?;
                Some(VariantInfo {
                    row: w.weapon as i32,
                    base,
                    name: w.fields.get(KEY_VARIANT_NAME).cloned(),
                    icon: w.fields.get(KEY_VARIANT_ICON).cloned(),
                    hh_skins: numbered_skins(&w.fields, KEY_HH_SKIN),
                    pv_skins: numbered_skins(&w.fields, KEY_PV_SKIN),
                    look_of: w
                        .fields
                        .get(KEY_VARIANT_LOOK_OF)
                        .and_then(|v| v.trim().parse().ok()),
                    rarity: w
                        .fields
                        .get(KEY_VARIANT_RARITY)
                        .and_then(|v| Rarity::parse(v))
                        .unwrap_or_default(),
                })
            })
            .collect()
    }

    /// No row changes at all (vanilla content).
    pub fn is_empty(&self) -> bool {
        self.weapons.is_empty() && self.ammo.is_empty()
    }

    /// The canonical document text.
    pub fn to_json(&self) -> String {
        let mut s = format!("{{\"version\":{CONTENT_FORMAT_VERSION},\"weapons\":");
        let w: Vec<Row<'_>> = self
            .weapons
            .iter()
            .map(|p| Row {
                id: p.weapon,
                op: p.op,
                fields: &p.fields,
            })
            .collect();
        write_rows(&mut s, "weapon", &w);
        s.push_str(",\"ammo\":");
        let a: Vec<Row<'_>> = self
            .ammo
            .iter()
            .map(|p| Row {
                id: p.ammo,
                op: p.op,
                fields: &p.fields,
            })
            .collect();
        write_rows(&mut s, "ammo", &a);
        s.push('}');
        s
    }

    /// The content hash: SHA-256 (lower-case hex) of [`WeaponContent::to_json`].
    pub fn hash(&self) -> String {
        crate::dc_rom::sha256_hex(self.to_json().as_bytes())
    }

    /// Reads a document (any JSON spacing; the version must be 1; unknown keys are refused).
    pub fn from_json(text: &str) -> Result<Self, PatchError> {
        let v: serde_json::Value =
            serde_json::from_str(text).map_err(|e| PatchError(format!("not JSON: {e}")))?;
        let Some(obj) = v.as_object() else {
            return err("the document is not an object");
        };
        for k in obj.keys() {
            if !matches!(k.as_str(), "version" | "weapons" | "ammo") {
                return err(format!("unknown key {k:?}"));
            }
        }
        match obj.get("version").and_then(|x| x.as_u64()) {
            Some(CONTENT_FORMAT_VERSION) => {}
            other => {
                return err(format!(
                    "version {other:?} (this build reads {CONTENT_FORMAT_VERSION})"
                ))
            }
        }
        let weapons = parse_rows(obj.get("weapons"), "weapons", "weapon")?
            .into_iter()
            .map(|(weapon, op, fields)| WeaponPatch { weapon, op, fields })
            .collect();
        let ammo = parse_rows(obj.get("ammo"), "ammo", "ammo")?
            .into_iter()
            .map(|(ammo, op, fields)| AmmoPatch { ammo, op, fields })
            .collect();
        Self::new(weapons, ammo)
    }

    /// Reads a document file.
    pub fn load(path: &Path) -> Result<Self, PatchError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| PatchError(format!("cannot read {}: {e}", path.display())))?;
        Self::from_json(&text).map_err(|e| PatchError(format!("{}: {}", path.display(), e.0)))
    }

    fn rows(&self) -> Vec<(String, PatchOp, &BTreeMap<String, String>)> {
        let w = self
            .weapons
            .iter()
            .map(|p| (format!("{WEAPON_SECTION}{}", p.weapon), p.op, &p.fields));
        let a = self
            .ammo
            .iter()
            .map(|p| (format!("{AMMO_SECTION}{}", p.ammo), p.op, &p.fields));
        w.chain(a).collect()
    }

    /// Applies the changes to a parsed weapon document. All rows are checked first (a patch needs its section, a
    /// register needs a free one); on a refusal the document is left as it was.
    pub fn apply_to_doc(&self, doc: &mut ButeDoc) -> Result<(), PatchError> {
        let rows = self.rows();
        let mut seen = BTreeSet::new();
        for (name, op, _) in &rows {
            if !seen.insert(name.clone()) {
                return err(format!("[{name}] listed twice"));
            }
            match (op, doc.contains_key(name)) {
                (PatchOp::Patch, false) => {
                    return err(format!("patch of [{name}]: the table has no such row"))
                }
                (PatchOp::Register, true) => {
                    return err(format!("register of [{name}]: the row already exists"))
                }
                _ => {}
            }
        }
        // VAR-2: a variant is checked before anything changes, and built last, from the finished base row
        let variant_of = |fields: &BTreeMap<String, String>| fields.get(KEY_VARIANT_OF).cloned();
        for (name, op, fields) in &rows {
            let Some(of) = variant_of(fields) else {
                continue;
            };
            if *op != PatchOp::Register {
                return err(format!("[{name}]: {KEY_VARIANT_OF} needs a new row"));
            }
            let base = format!("{WEAPON_SECTION}{}", of.trim());
            let Some(row) = doc.sections.get(&base) else {
                return err(format!("variant [{name}]: the table has no row [{base}]"));
            };
            if row.contains_key(KEY_VARIANT_OF) {
                return err(format!("variant [{name}]: [{base}] is a variant itself"));
            }
            if rows
                .iter()
                .any(|(n, _, f)| *n == base && variant_of(f).is_some())
            {
                return err(format!("variant [{name}]: [{base}] is a variant itself"));
            }
            if let Some(look) = fields.get(KEY_VARIANT_LOOK_OF) {
                let look = format!("{WEAPON_SECTION}{}", look.trim());
                let Some(row) = doc.sections.get(&look) else {
                    return err(format!("variant [{name}]: the table has no row [{look}]"));
                };
                if row.contains_key(KEY_VARIANT_OF) {
                    return err(format!("variant [{name}]: [{look}] is a variant itself"));
                }
            }
            if let Some(k) = fields.keys().find(|k| !is_cosmetic_key(k)) {
                return err(format!(
                    "variant [{name}]: {k:?} is not a cosmetic key; a variant may not change how the gun plays"
                ));
            }
            if let Some(r) = fields
                .get(KEY_VARIANT_RARITY)
                .filter(|r| Rarity::parse(r).is_none())
            {
                return err(format!(
                    "variant [{name}]: {KEY_VARIANT_RARITY} {r:?} is not one of common, rare, epic, legendary"
                ));
            }
        }
        let (variants, plain): (Vec<_>, Vec<_>) = rows
            .into_iter()
            .partition(|(_, _, fields)| variant_of(fields).is_some());
        for (name, op, fields) in plain {
            match op {
                PatchOp::Patch => {
                    let sec = doc
                        .sections
                        .get_mut(&name)
                        .expect("checked above: the row exists");
                    for (k, v) in fields {
                        sec.insert(k.clone(), v.clone());
                    }
                }
                PatchOp::Register => {
                    let mut sec = SectionMap::new();
                    for (k, v) in fields {
                        sec.insert(k.clone(), v.clone());
                    }
                    doc.sections.insert(name.clone(), sec);
                    doc.order.push(name);
                }
            }
        }
        for (name, _, fields) in variants {
            let of = variant_of(fields).unwrap_or_default();
            let mut sec = doc
                .sections
                .get(&format!("{WEAPON_SECTION}{}", of.trim()))
                .cloned()
                .expect("checked above: the base row exists");
            if let Some(look) = fields.get(KEY_VARIANT_LOOK_OF) {
                let look = doc
                    .sections
                    .get(&format!("{WEAPON_SECTION}{}", look.trim()))
                    .expect("checked above: the look's row exists");
                let mut worn = SectionMap::new();
                for (k, v) in sec.iter().filter(|(k, _)| !is_look_key(k)) {
                    worn.insert(k.clone(), v.clone());
                }
                for (k, v) in look.iter().filter(|(k, _)| is_look_key(k)) {
                    worn.insert(k.clone(), v.clone());
                }
                sec = worn;
            }
            for (k, v) in fields {
                sec.insert(k.clone(), v.clone());
            }
            doc.sections.insert(name.clone(), sec);
            doc.order.push(name);
        }
        Ok(())
    }
}

impl WeaponsTable {
    /// MOD-21: applies a mod set's weapon content to this table ([`WeaponContent::apply_to_doc`]).
    pub fn apply_content(&mut self, content: &WeaponContent) -> Result<(), PatchError> {
        content.apply_to_doc(&mut self.doc)
    }
}

#[cfg(test)]
mod variant_tests {
    use super::*;

    #[test]
    fn a_borrowed_look_brings_the_models_and_never_how_the_gun_plays() {
        let mut doc = ButeDoc::parse(concat!(
            "[Weapon27]\nName = \"L\"\nHHModel = \"a.ltb\"\nHHSkin0 = \"a.dtx\"\nPVSkin5 = \"only_a.dtx\"\n",
            "FireSndRadius = 3000\nFireSnd = \"loud.wav\"\nAmmoName0 = \"A\"\nPos = <1,1,1>\n",
            "[Weapon54]\nName = \"G\"\nHHModel = \"g.ltb\"\nHHSkin0 = \"g.dtx\"\nPVModelHand_St = \"g_pv.ltb\"\n",
            "FireSndRadius = 800\nFireSnd = \"quiet.wav\"\nAmmoName0 = \"G\"\nPos = <0,0,0>\n",
        ));
        let content = variant(504, &[(KEY_VARIANT_OF, "27"), (KEY_VARIANT_LOOK_OF, "54")]);
        content.apply_to_doc(&mut doc).unwrap();
        let sec = doc.get("Weapon504").unwrap();
        let get = |k: &str| sec.get(k).map(|v| v.trim_matches('"').to_string());
        assert_eq!(get("HHModel").as_deref(), Some("g.ltb"), "the look's model");
        assert_eq!(get("PVModelHand_St").as_deref(), Some("g_pv.ltb"));
        assert_eq!(get("Pos").as_deref(), Some("<0,0,0>"), "where it sits");
        assert_eq!(
            get("PVSkin5"),
            None,
            "none of the base gun's own look stays"
        );
        assert_eq!(
            get("FireSndRadius").as_deref(),
            Some("3000"),
            "heard as far as the base gun"
        );
        assert_eq!(get("FireSnd").as_deref(), Some("loud.wav"));
        assert_eq!(get("AmmoName0").as_deref(), Some("A"));
        assert_eq!(get("Name").as_deref(), Some("L"));

        let mut doc = ButeDoc::parse("[Weapon27]\nName = \"L\"\n");
        let missing = variant(504, &[(KEY_VARIANT_OF, "27"), (KEY_VARIANT_LOOK_OF, "54")]);
        assert!(missing.apply_to_doc(&mut doc).is_err());
    }

    fn doc() -> ButeDoc {
        ButeDoc::parse(concat!(
            "[Weapon25]\nName = \"G\"\n",
            "ShotsPerClip = 30\n",
            "InstDamage = 20\nPVSkin1 = \"a.dtx\"\nHHSkin0 = \"b.dtx\"\n",
        ))
    }

    fn variant(row: u16, fields: &[(&str, &str)]) -> WeaponContent {
        WeaponContent::new(
            vec![WeaponPatch {
                weapon: row,
                op: PatchOp::Register,
                fields: fields
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            }],
            Vec::new(),
        )
        .unwrap()
    }

    /// CLI-1845: a gun's weapon inspect is every one of its variants' too, whatever order the packs come in, and a variant
    /// cannot set one of its own (the inspect keys are not cosmetic keys).
    #[test]
    fn a_guns_inspect_is_its_variants_inspect() {
        let patch = |weapon: u16, op: PatchOp, fields: &[(&str, &str)]| WeaponPatch {
            weapon,
            op,
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let inspect = [
            ("InspectLen", "3900"),
            ("InspectPose1", "480 16 -50 -3 -2.6 0.4 2.9"),
        ];
        let camo = [("variant_of", "25"), ("PVSkin1", "@mod:p/assets/x.dtx")];
        let c = WeaponContent::new(
            vec![
                patch(500, PatchOp::Register, &camo),
                patch(25, PatchOp::Patch, &inspect),
            ],
            Vec::new(),
        )
        .unwrap();
        let mut d = doc();
        c.apply_to_doc(&mut d).unwrap();
        for (k, v) in inspect {
            assert_eq!(
                d.sections["Weapon500"].get(k).map(String::as_str),
                Some(v),
                "{k}"
            );
        }
        assert!(!is_cosmetic_key("InspectLen") && !is_cosmetic_key("InspectPose1"));
        let own = [("variant_of", "25"), ("InspectLen", "100")];
        let refused = WeaponContent::new(vec![patch(501, PatchOp::Register, &own)], Vec::new())
            .and_then(|c| c.apply_to_doc(&mut doc()));
        assert!(refused.is_err(), "a variant may not set its own inspect");
    }

    /// VAR-2: a variant has its base row's every gameplay key, and differs only where it is allowed to.
    #[test]
    fn a_variant_keeps_every_gameplay_key_of_its_base() {
        let mut d = doc();
        let c = variant(
            500,
            &[("variant_of", "25"), ("PVSkin1", "@mod:p/assets/x.dtx")],
        );
        c.apply_to_doc(&mut d).unwrap();
        let (base, row) = (&d.sections["Weapon25"], &d.sections["Weapon500"]);
        for (k, v) in base.iter() {
            if k != "PVSkin1" {
                assert_eq!(row.get(k), Some(v), "{k}");
            }
        }
        assert_eq!(
            row.get("PVSkin1").map(String::as_str),
            Some("@mod:p/assets/x.dtx")
        );
        assert_eq!(
            base.get("PVSkin1").map(String::as_str),
            Some("a.dtx"),
            "the base is untouched"
        );
        let v = c.variants();
        assert_eq!((v[0].row, v[0].base), (500, 25));
    }

    /// CRATE-6: a variant names its rarity tier; one without the key is Common, an unknown tier is refused.
    #[test]
    fn a_variant_names_its_rarity_and_an_unknown_one_is_refused() {
        let mut d = doc();
        let c = variant(
            500,
            &[("variant_of", "25"), ("variant_rarity", "Legendary")],
        );
        c.apply_to_doc(&mut d).unwrap();
        assert_eq!(c.variants()[0].rarity, Rarity::Legendary);
        let plain = variant(501, &[("variant_of", "25")]);
        assert_eq!(plain.variants()[0].rarity, Rarity::Common);
        let e = WeaponContent::new(
            vec![WeaponPatch {
                weapon: 502,
                op: PatchOp::Register,
                fields: [("variant_of", "25"), ("variant_rarity", "mythic")]
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            }],
            Vec::new(),
        )
        .unwrap_err();
        assert!(e.0.contains("mythic"), "{e}");
        assert!(Rarity::ALL
            .iter()
            .all(|r| Rarity::parse(r.key()) == Some(*r) && Rarity::parse(r.name()) == Some(*r)));
        assert!(Rarity::Common < Rarity::Legendary);
    }

    #[test]
    fn a_variant_may_not_change_gameplay_or_stack() {
        let mut d = doc();
        let before = d.clone();
        let e = variant(500, &[("variant_of", "25"), ("InstDamage", "99")])
            .apply_to_doc(&mut d)
            .unwrap_err();
        assert!(e.0.contains("InstDamage"), "{e}");
        assert!(variant(500, &[("variant_of", "7")])
            .apply_to_doc(&mut d)
            .is_err());
        assert_eq!(d.sections, before.sections, "a refusal changes nothing");
        variant(500, &[("variant_of", "25")])
            .apply_to_doc(&mut d)
            .unwrap();
        assert!(variant(501, &[("variant_of", "500")])
            .apply_to_doc(&mut d)
            .is_err());
    }

    /// A base patched by the same document: the variant follows it, so the two still play the same.
    #[test]
    fn a_variant_follows_a_patch_of_its_base() {
        let mut d = doc();
        let c = WeaponContent::new(
            vec![
                WeaponPatch {
                    weapon: 25,
                    op: PatchOp::Patch,
                    fields: [("InstDamage".to_string(), "21".to_string())].into(),
                },
                WeaponPatch {
                    weapon: 500,
                    op: PatchOp::Register,
                    fields: [("variant_of".to_string(), "25".to_string())].into(),
                },
            ],
            Vec::new(),
        )
        .unwrap();
        c.apply_to_doc(&mut d).unwrap();
        assert_eq!(
            d.sections["Weapon500"]
                .get("InstDamage")
                .map(String::as_str),
            Some("21")
        );
    }

    #[test]
    fn only_looks_are_cosmetic_keys() {
        for k in [
            "PVSkin1",
            "HHSkin0",
            "PVRenderStyle2",
            "HHModel",
            "PVModelHand_St",
            "variant_name",
        ] {
            assert!(is_cosmetic_key(k), "{k}");
        }
        for k in [
            "InstDamage",
            "ShotsPerClip",
            "PVSkin",
            "PVSkinX",
            "ModName0",
            "Name",
            "AmmoName",
        ] {
            assert!(!is_cosmetic_key(k), "{k}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // made-up rows in the key shape of the ROM weapon table
    const DOC: &str = concat!(
        "[Ammo0]\nName = \"Round A\"\nInstDamage = 25\n",
        "[Weapon0]\nName = \"Gun A\"\n",
        "ShotsPerClip = 30\nAmmoName0 = \"Round A\"\n",
        "[Weapon145]\nName = \"Gun B\"\n",
        "ShotsPerClip = 10\n",
    );

    fn fields(kv: &[(&str, &str)]) -> BTreeMap<String, String> {
        kv.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn content() -> WeaponContent {
        WeaponContent::new(
            vec![
                WeaponPatch {
                    weapon: 145,
                    op: PatchOp::Patch,
                    fields: fields(&[("ShotsPerClip", "12"), ("AmmoDamage", "41")]),
                },
                WeaponPatch {
                    weapon: 300,
                    op: PatchOp::Register,
                    fields: fields(&[("Name", "\"Gun C\""), ("ShotsPerClip", "5")]),
                },
            ],
            vec![AmmoPatch {
                ammo: 0,
                op: PatchOp::Patch,
                fields: fields(&[("InstDamage", "30.5")]),
            }],
        )
        .unwrap()
    }

    #[test]
    fn patch_overwrites_keys_and_keeps_the_rest() {
        let mut doc = ButeDoc::parse(DOC);
        content().apply_to_doc(&mut doc).unwrap();
        assert_eq!(doc.get_str("Weapon145", "ShotsPerClip"), Some("12"));
        assert_eq!(
            doc.get_str("Weapon145", "AmmoDamage"),
            Some("41"),
            "unknown keys are allowed"
        );
        assert_eq!(doc.get_str("Weapon145", "Name"), Some("Gun B"));
        assert_eq!(doc.get_str("Ammo0", "InstDamage"), Some("30.5"));
        assert_eq!(doc.get_str("Weapon0", "ShotsPerClip"), Some("30"));
    }

    #[test]
    fn register_adds_a_new_row_in_order() {
        let mut t = WeaponsTable::from_doc(ButeDoc::parse(DOC));
        t.apply_content(&content()).unwrap();
        let row = t.weapon(300).expect("registered row");
        assert_eq!(
            row.get("Name").map(String::as_str),
            Some("Gun C"),
            "quotes stripped"
        );
        assert_eq!(row.len(), 2);
        assert_eq!(t.doc().order.last().map(String::as_str), Some("Weapon300"));
    }

    #[test]
    fn a_bad_id_refuses_the_whole_document() {
        let mut doc = ButeDoc::parse(DOC);
        let missing = WeaponContent::new(
            vec![
                WeaponPatch {
                    weapon: 145,
                    op: PatchOp::Patch,
                    fields: fields(&[("ShotsPerClip", "99")]),
                },
                WeaponPatch {
                    weapon: 7,
                    op: PatchOp::Patch,
                    fields: fields(&[("ShotsPerClip", "1")]),
                },
            ],
            vec![],
        )
        .unwrap();
        let e = missing.apply_to_doc(&mut doc).unwrap_err();
        assert!(e.0.contains("[Weapon7]"), "{e}");
        assert_eq!(
            doc.get_str("Weapon145", "ShotsPerClip"),
            Some("10"),
            "nothing applied"
        );
        let taken = WeaponContent::new(
            vec![WeaponPatch {
                weapon: 0,
                op: PatchOp::Register,
                fields: fields(&[("Name", "X")]),
            }],
            vec![],
        )
        .unwrap();
        assert!(taken
            .apply_to_doc(&mut doc)
            .unwrap_err()
            .0
            .contains("already exists"));
        // duplicate ids, a bad key, a bad value
        let dup = |op| WeaponPatch {
            weapon: 1,
            op,
            fields: BTreeMap::new(),
        };
        assert!(
            WeaponContent::new(vec![dup(PatchOp::Patch), dup(PatchOp::Register)], vec![]).is_err()
        );
        let bad_key = WeaponPatch {
            weapon: 1,
            op: PatchOp::Patch,
            fields: fields(&[("Shots PerClip", "1")]),
        };
        assert!(WeaponContent::new(vec![bad_key], vec![]).is_err());
        let bad_value = WeaponPatch {
            weapon: 1,
            op: PatchOp::Patch,
            fields: fields(&[("Name", "a\nb")]),
        };
        assert!(WeaponContent::new(vec![bad_value], vec![]).is_err());
    }

    #[test]
    fn json_round_trips_in_canonical_form_and_the_hash_is_stable() {
        let c = content();
        let j = c.to_json();
        assert_eq!(
            j,
            concat!(
                "{\"version\":1,\"weapons\":[",
                "{\"weapon\":145,\"op\":\"patch\",\"fields\":{\"AmmoDamage\":\"41\",\"ShotsPerClip\":\"12\"}},",
                "{\"weapon\":300,\"op\":\"register\",\"fields\":{\"Name\":\"Gun C\",\"ShotsPerClip\":\"5\"}}],",
                "\"ammo\":[{\"ammo\":0,\"op\":\"patch\",\"fields\":{\"InstDamage\":\"30.5\"}}]}"
            )
        );
        let back = WeaponContent::from_json(&j).unwrap();
        assert_eq!(back, c);
        assert_eq!(back.hash(), c.hash());
        assert_eq!(c.hash().len(), 64);
        // spacing and row order of the file do not change the hash
        let spaced = r#"{ "ammo": [ {"fields": {"InstDamage": "30.5"}, "op": "patch", "ammo": 0} ],
            "weapons": [ {"weapon": 300, "op": "register", "fields": {"ShotsPerClip": "5", "Name": "Gun C"}},
                         {"weapon": 145, "op": "patch", "fields": {"ShotsPerClip": "12", "AmmoDamage": "41"}} ],
            "version": 1 }"#;
        assert_eq!(WeaponContent::from_json(spaced).unwrap().hash(), c.hash());
        // a different value is a different hash
        let mut other = c.clone();
        other.ammo[0]
            .fields
            .insert("InstDamage".into(), "31".into());
        assert_ne!(other.hash(), c.hash());
        assert!(WeaponContent::default().is_empty());
    }

    #[test]
    fn malformed_documents_are_refused() {
        for bad in [
            "",
            "[]",
            "{\"version\":2,\"weapons\":[]}",
            "{\"weapons\":[]}",
            "{\"version\":1,\"weapons\":{}}",
            "{\"version\":1,\"weapons\":[{\"weapon\":70000,\"op\":\"patch\",\"fields\":{}}]}",
            "{\"version\":1,\"weapons\":[{\"weapon\":1,\"op\":\"replace\",\"fields\":{}}]}",
            "{\"version\":1,\"weapons\":[{\"weapon\":1,\"op\":\"patch\",\"fields\":{\"A\":41}}]}",
            "{\"version\":1,\"weapons\":[{\"weapon\":1,\"op\":\"patch\",\"fields\":{},\"x\":1}]}",
            "{\"version\":1,\"extra\":[]}",
        ] {
            assert!(WeaponContent::from_json(bad).is_err(), "{bad}");
        }
        assert!(WeaponContent::from_json("{\"version\":1}")
            .unwrap()
            .is_empty());
    }
}
