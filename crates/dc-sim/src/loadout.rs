//! The per-player speed modifiers of the 2009 client, from the ROM rows a loadout selects.
//!
//! The 2009 mover scales every stance speed by `1 - p` and the sprint speed additionally by a dash factor:
//!
//! - **penalty** `p = clamp(held + model + weight + backpack + hair - speed_bonus, -1, 1)` with `held` the held
//!   weapon row's `MovePenalty`, `model` the body row's `MovePenalty` (an override row's value wins when its
//!   magnitude is above 0.01), `weight` / `speed_bonus` the figure row's `VariableWeight` / `VariableSpeed`,
//!   `backpack` the backpack row's `MovePenalty` and `hair` the head-gear row's `MovePenalty`;
//! - **sprint** `s = (model_dash + hair_dash + backpack_dash + goggle_dash + 1) x run_speed x figure_dash`, each
//!   `*_dash` the row's `DashSpeed` (the model override wins when above 0.1; the figure row's key defaults to
//!   1.4 when missing), then the same `1 - p`.
//!
//! So the held weapon changes run AND sprint speed by its own `MovePenalty` (a knife row has 0, a rifle about
//! 0.1, a machine gun up to 0.3), and the sprint factor comes from the character, not from the weapon.
//!
//! The same rows carry the sprint gauge ([`SpeedTable::stamina`], see `crate::stamina`): `max = trunc(figure
//! Maxstamina x (1 + hair + goggle + model + backpack MaxStamina))`, drain `DashGauge` and refill `GaugeRec` per
//! second from the figure row.
//!
//! The formula and the record layout live in [`SpeedTable::mods`] (no ROM, `no_std`); with feature `rom` the
//! table is read from the user's own `ATTRIBUTES.rez` at run time ([`SpeedTable::from_vfs`]). Nothing here is ROM
//! data: the key and section names are the 2009 reader's own names (cited below), every value comes from the ROM.
// EVIDENCE: cshell+0x5c270 (the derived mover's penalty: weapon row +0x2d8 of the held weapon id (slot +0xc0 of the
// object at VA 0x3759e110, table lookup cshell+0x172300) + model penalty cshell+0x1e2bc0 + figure weight
// cshell+0x1e1c80 + backpack penalty cshell+0x1de870 (index -1 = 0) + head-gear penalty cshell+0x1e16c0 - figure
// speed cshell+0x1e1b00, clamped to [-1, 1])
// EVIDENCE: cshell+0x1fd390 (target speed: sprint `(model dash cshell+0x1e2cc0 + head-gear dash cshell+0x1e1720 +
// backpack dash cshell+0x1de8d0 + goggle dash cshell+0x1deb70 + 1) x run speed x figure dash cshell+0x1e1c20`, then
// every branch `x (1 - p)`)
// DISASM: cshell+0x17dbd5 (weapon row loader: key `MovePenalty` into row +0x2d8, read as a float with the previous
// value as default)
// EVIDENCE: cshell+0x1e4760 (model-table reader: `Hair<n>` rows `MovePenalty` +0x74 / `DashSpeed` +0x7c, default 0;
// `FigureType<n>` rows `VariableSpeed` +0x24, `VariableJump` +0x28, `VariableWeight` +0x30, `DashSpeed` +0xe4
// default 1.4; `Model<n>`, then `AIModel<n>`, then `Overidable_Model<n>` rows `MovePenalty` +0x408 / `DashSpeed`
// +0x438 in ONE array (count = the three counts, overrides from index models + AI models), `FigureType` +0x48)
// EVIDENCE: cshell+0x1dfc0a (attachment-table reader: `Backpack<n>` `MovePenalty` +0x120 / `DashSpeed` +0x128,
// `Goggle<n>` `DashSpeed` +0x128, default 0)
// EVIDENCE: cshell+0x1e4760 (gauge keys: `Hair<n>` `MaxStamina` +0x78 (float); `Model<n>` / `AIModel<n>` /
// `Overidable_Model<n>` `MaxStamina` +0x434 (float); `FigureType<n>` `DashGauge` +0xd8, `DashJumpGauge` +0xdc,
// `GaugeRec` +0xe0, `Maxstamina` +0xe8 (integers, cshell+0x199110)); cshell+0x1dfc0a (`Backpack<n>` / `Goggle<n>`
// `MaxStamina` +0x124, float, default 0)
// OBSERVED: ROM table keys MODELBUTES.TXT [Model<n>] MovePenalty, DashSpeed, FigureType; [FigureType<n>]
// VariableSpeed, VariableJump, VariableWeight, DashSpeed; [Hair<n>] MovePenalty, DashSpeed; WEAPONS.TXT [Weapon<n>]
// MovePenalty; ATTACHMENTS.TXT [Backpack<n>] MovePenalty, DashSpeed, [Goggle<n>] DashSpeed (entries of
// ATTRIBUTES.rez; values read at run time)

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::params::SpeedMods;

/// Speed keys of one body (model) row.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BodySpeed {
    /// `MovePenalty`.
    pub penalty: f32,
    /// `DashSpeed` (added to the sprint factor).
    pub dash: f32,
    /// `FigureType`: the figure row this body uses.
    pub figure: Option<u32>,
    /// `MaxStamina` (a fraction added to the gauge multiplier).
    pub max_stamina: f32,
}

/// `a` and `b` name the same model file (ASCII case and `/` vs `\` ignored).
fn same_file(a: &str, b: &str) -> bool {
    let norm = |c: u8| {
        if c == b'/' {
            0x5c
        } else {
            c.to_ascii_lowercase()
        }
    };
    a.len() == b.len() && a.bytes().zip(b.bytes()).all(|(x, y)| norm(x) == norm(y))
}

/// Speed and gauge keys of one figure row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FigureSpeed {
    /// `VariableSpeed` (subtracted from the penalty).
    pub speed_bonus: f32,
    /// `VariableJump` (the jump bonus of MOVEMENT_2009.md §6.4).
    pub jump_bonus: f32,
    /// `VariableWeight` (added to the penalty).
    pub weight: f32,
    /// `DashSpeed` (the sprint multiplier; 1.4 when the key is missing).
    pub dash_mul: f32,
    /// `Maxstamina` (integer): the full gauge before the gear terms.
    pub max_stamina: Option<i32>,
    /// `DashGauge` (integer): gauge units per second of sprint.
    pub dash_gauge: Option<i32>,
    /// `DashJumpGauge` (integer): read by the 2009 loader; no reader of it was found in the gauge law.
    pub dash_jump_gauge: Option<i32>,
    /// `GaugeRec` (integer): gauge units per second at rest.
    pub gauge_rec: Option<i32>,
}

impl Default for FigureSpeed {
    fn default() -> Self {
        FigureSpeed {
            speed_bonus: 0.0,
            jump_bonus: 0.0,
            weight: 0.0,
            dash_mul: FIGURE_DASH_DEFAULT,
            max_stamina: None,
            dash_gauge: None,
            dash_jump_gauge: None,
            gauge_rec: None,
        }
    }
}

/// Speed keys of a gear row (head gear, backpack, goggles).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GearSpeed {
    pub penalty: f32,
    pub dash: f32,
    /// `MaxStamina` (a fraction added to the gauge multiplier).
    pub max_stamina: f32,
}

/// What a player carries and wears, as row indices of the tables (`None` = no such item: its terms count 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Loadout {
    /// The held weapon (`[Weapon<n>]`).
    pub weapon: Option<u16>,
    /// The body row (`[Model<n>]`, the index into the combined body array).
    pub body: Option<u32>,
    /// The override body slot (`[Overidable_Model<n>]`).
    pub body_override: Option<u32>,
    /// The figure row; `None` uses the body row's `FigureType`.
    pub figure: Option<u32>,
    /// Head gear (`[Hair<n>]`).
    pub head: Option<u32>,
    pub backpack: Option<u32>,
    pub goggles: Option<u32>,
}

/// The speed keys of every row the formula reads.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpeedTable {
    /// `[Weapon<n>] MovePenalty` by weapon number (a row without the key is absent: 0).
    pub weapons: BTreeMap<u16, f32>,
    /// The body array in the 2009 order: `Model<n>`, then `AIModel<n>`, then `Overidable_Model<n>`.
    pub bodies: Vec<BodySpeed>,
    /// Number of `Model<n>` + `AIModel<n>` rows (the override slots start here).
    pub override_base: usize,
    pub figures: Vec<FigureSpeed>,
    pub heads: Vec<GearSpeed>,
    pub backpacks: Vec<GearSpeed>,
    pub goggles: Vec<GearSpeed>,
    /// `ModelFile` of the `[Model<n>]` rows (to find the own body row from the look the server sends).
    pub body_files: Vec<String>,
}

/// The figure row's sprint multiplier when its key is missing.
// EVIDENCE: cshell+0x1e4760 (`DashSpeed` of `FigureType<n>` read with the double default 0x3ff6666660000000 = 1.4)
pub const FIGURE_DASH_DEFAULT: f32 = 1.4;
/// An override body row's penalty counts only when its magnitude exceeds this.
// EVIDENCE: cshell+0x1e2bc0 (override used when 0.01 < |row +0x408|)
pub const OVERRIDE_PENALTY_MIN: f32 = 0.01;
/// An override body row's dash counts only when it exceeds this.
// EVIDENCE: cshell+0x1e2cc0 (override used when 0.1 < row +0x438)
pub const OVERRIDE_DASH_MIN: f32 = 0.1;
/// An override body row's `MaxStamina` counts only when it exceeds this.
// DISASM: cshell+0x1e2c40 (override row +0x434 used when it compares above the binary32 0.1 at VA 0x374ce3dc, else
// the base row +0x434, else 0)
pub const OVERRIDE_STAMINA_MIN: f32 = 0.1;

fn at<T: Copy>(v: &[T], i: Option<u32>) -> Option<T> {
    i.and_then(|i| v.get(i as usize).copied())
}

impl SpeedTable {
    /// The override row of slot `slot` (index `override_base + slot` of the body array), when it exists.
    fn override_row(&self, slot: Option<u32>) -> Option<BodySpeed> {
        let s = slot? as usize;
        self.bodies.get(self.override_base.checked_add(s)?).copied()
    }

    /// Body penalty with the override rule.
    // EVIDENCE: cshell+0x1e2bc0
    pub fn body_penalty(&self, l: &Loadout) -> f32 {
        if let Some(o) = self.override_row(l.body_override) {
            if o.penalty.abs() > OVERRIDE_PENALTY_MIN {
                return o.penalty;
            }
        }
        at(&self.bodies, l.body).map(|b| b.penalty).unwrap_or(0.0)
    }

    /// Body dash with the override rule.
    // EVIDENCE: cshell+0x1e2cc0
    pub fn body_dash(&self, l: &Loadout) -> f32 {
        if let Some(o) = self.override_row(l.body_override) {
            if o.dash > OVERRIDE_DASH_MIN {
                return o.dash;
            }
        }
        at(&self.bodies, l.body).map(|b| b.dash).unwrap_or(0.0)
    }

    /// Body `MaxStamina` with the override rule.
    // DISASM: cshell+0x1e2c40
    pub fn body_stamina(&self, l: &Loadout) -> f32 {
        if let Some(o) = self.override_row(l.body_override) {
            if o.max_stamina > OVERRIDE_STAMINA_MIN {
                return o.max_stamina;
            }
        }
        at(&self.bodies, l.body)
            .map(|b| b.max_stamina)
            .unwrap_or(0.0)
    }

    /// The sprint gauge rates of a loadout; `None` when the figure row (or its `Maxstamina`) is unknown.
    // DISASM: cshell+0x1d33d0 (x87: `a = f32(head + 1.0)`, `a = f32(goggles + a)` when a goggle row is set,
    // `a = body + a` (kept on the stack), with a backpack row `a = backpack + f32(a)`; then `max = trunc(Maxstamina
    // x a)` (cshell+0x3a5f00 truncates); drain = `DashGauge` cshell+0x1e1b90, regen = `GaugeRec` cshell+0x1e1bc0,
    // `Maxstamina` cshell+0x1e1bf0; the getters give 0 for a missing row)
    // DC-DEFAULT: SIM-19 (proposed) no figure row or no `Maxstamina`: no gauge rule (the sprint never runs out);
    // a missing `DashGauge` / `GaugeRec` counts 0
    pub fn stamina(&self, l: &Loadout) -> Option<crate::stamina::StaminaRates> {
        let fig = self.figure(l)?;
        let base = fig.max_stamina?;
        let head = at(&self.heads, l.head).unwrap_or_default();
        let mut a = (head.max_stamina as f64 + 1.0) as f32;
        if let Some(g) = at(&self.goggles, l.goggles) {
            a = (g.max_stamina as f64 + a as f64) as f32;
        }
        let mut acc = self.body_stamina(l) as f64 + a as f64;
        if let Some(b) = at(&self.backpacks, l.backpack) {
            acc = b.max_stamina as f64 + (acc as f32) as f64;
        }
        let max = (base as f64 * acc) as i64;
        Some(crate::stamina::StaminaRates {
            max: max.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            drain: fig.dash_gauge.unwrap_or(0),
            regen: fig.gauge_rec.unwrap_or(0),
        })
    }

    /// The figure row of a loadout: its own choice, else the body row's `FigureType`.
    // DC-DEFAULT: SIM-10 (proposed) the figure index is the body row's `FigureType` (the 2009 client takes it from
    // its loadout record +0x58, whose writer was not read)
    pub fn figure(&self, l: &Loadout) -> Option<FigureSpeed> {
        let f = l
            .figure
            .or_else(|| at(&self.bodies, l.body).and_then(|b| b.figure));
        at(&self.figures, f)
    }

    /// The [`SpeedMods`] of a loadout (the 2009 sums, summed in binary64 and rounded once like the client's x87
    /// sum, then clamped by the mover).
    pub fn mods(&self, l: &Loadout) -> SpeedMods {
        let held = l
            .weapon
            .and_then(|w| self.weapons.get(&w).copied())
            .unwrap_or(0.0);
        let fig = self.figure(l);
        let head = at(&self.heads, l.head).unwrap_or_default();
        let pack = at(&self.backpacks, l.backpack).unwrap_or_default();
        let gog = at(&self.goggles, l.goggles).unwrap_or_default();
        let (weight, bonus, jump, mul) = match fig {
            Some(f) => (f.weight, f.speed_bonus, f.jump_bonus, f.dash_mul),
            None => (0.0, 0.0, 0.0, FIGURE_DASH_DEFAULT),
        };
        // EVIDENCE: cshell+0x5c270 (sum order: held + model + weight + backpack + hair, then - speed bonus)
        let penalty = ((held as f64
            + self.body_penalty(l) as f64
            + weight as f64
            + pack.penalty as f64
            + head.penalty as f64)
            - bonus as f64) as f32;
        // EVIDENCE: cshell+0x1fd390 (sum order: model + hair + backpack + goggles, + 1 in the caller)
        let sprint_add =
            (self.body_dash(l) as f64 + head.dash as f64 + pack.dash as f64 + gog.dash as f64)
                as f32;
        SpeedMods {
            penalty,
            jump_bonus: jump,
            sprint_add,
            sprint_mul: Some(mul),
            stamina: self.stamina(l),
        }
    }

    /// No weapon and no body row (no ROM read).
    pub fn is_empty(&self) -> bool {
        self.weapons.is_empty() && self.bodies.is_empty()
    }

    /// The `[Model<n>]` row whose `ModelFile` is `file` (ASCII case and slash direction ignored).
    pub fn body_row_of_file(&self, file: &str) -> Option<u32> {
        self.body_files
            .iter()
            .position(|m| same_file(m, file))
            .map(|i| i as u32)
    }
}

#[cfg(feature = "rom")]
mod rom {
    use super::*;
    use dc_bute::{eq_fold, ButeDoc, SectionMap};
    use dc_rez::RezVfs;
    use std::string::ToString;

    /// The attribute archive and the three tables read here.
    // OBSERVED: ROM table keys ATTRIBUTES.rez entries MODELBUTES.TXT, WEAPONS.TXT, ATTACHMENTS.TXT
    pub const ATTRIBUTES_REZ: &str = "ATTRIBUTES.rez";
    pub const MODELS_ENTRY: &str = "ATTRIBUTES\\MODELBUTES.TXT";
    pub const WEAPONS_ENTRY: &str = "ATTRIBUTES\\WEAPONS.TXT";
    pub const ATTACHMENTS_ENTRY: &str = "ATTRIBUTES\\ATTACHMENTS.TXT";

    /// Section prefixes and keys (the 2009 readers' names, cited in the module header).
    pub mod key {
        pub const WEAPON: &str = "Weapon";
        pub const MODEL: &str = "Model";
        pub const AI_MODEL: &str = "AIModel";
        pub const OVERRIDE_MODEL: &str = "Overidable_Model";
        pub const FIGURE: &str = "FigureType";
        pub const HAIR: &str = "Hair";
        pub const BACKPACK: &str = "Backpack";
        pub const GOGGLE: &str = "Goggle";
        pub const PENALTY: &str = "MovePenalty";
        pub const DASH: &str = "DashSpeed";
        pub const FIGURE_OF_BODY: &str = "FigureType";
        pub const SPEED: &str = "VariableSpeed";
        pub const JUMP: &str = "VariableJump";
        pub const WEIGHT: &str = "VariableWeight";
        pub const MODEL_FILE: &str = "ModelFile";
        /// Float key of the body, head-gear, backpack and goggle rows.
        pub const MAX_STAMINA: &str = "MaxStamina";
        /// Integer keys of the figure rows (the figure's own key is spelled with a small `s`).
        pub const FIGURE_MAX_STAMINA: &str = "Maxstamina";
        pub const DASH_GAUGE: &str = "DashGauge";
        pub const DASH_JUMP_GAUGE: &str = "DashJumpGauge";
        pub const GAUGE_REC: &str = "GaugeRec";
    }

    fn section<'a>(doc: &'a ButeDoc, name: &str) -> Option<&'a SectionMap> {
        doc.get(name)
            .or_else(|| doc.iter().find(|(n, _)| eq_fold(n, name)).map(|(_, s)| s))
    }

    fn value<'a>(s: &'a SectionMap, k: &str) -> Option<&'a String> {
        s.get(k)
            .or_else(|| s.iter().find(|(kk, _)| eq_fold(kk, k)).map(|(_, v)| v))
    }

    fn num(s: &SectionMap, k: &str) -> Option<f32> {
        let t = value(s, k)?.trim().trim_matches('"').trim();
        let t = t.strip_suffix(['f', 'F']).unwrap_or(t);
        t.parse::<f32>().ok().filter(|x| x.is_finite())
    }

    fn int(s: &SectionMap, k: &str) -> Option<i64> {
        let t = value(s, k)?.trim().trim_matches('"').trim();
        t.parse::<i64>()
            .ok()
            .or_else(|| t.parse::<f64>().ok().map(|f| f as i64))
    }

    /// An integer key of a figure row as the 2009 integer reader takes it (cshell+0x199110 accepts integer-typed
    /// values only). A value in another form is taken as missing here.
    fn gauge_int(s: &SectionMap, k: &str) -> Option<i32> {
        let t = value(s, k)?.trim().trim_matches('"').trim();
        t.parse::<i32>().ok()
    }

    /// The rows `<prefix>0`, `<prefix>1`, ... up to the first missing one (the 2009 readers count this way).
    // EVIDENCE: cshell+0x1e4760, cshell+0x1dfc0a (`sprintf("%s%d")` + exists loop, then one read per row)
    fn rows<'a>(doc: &'a ButeDoc, prefix: &str) -> Vec<&'a SectionMap> {
        let mut out = Vec::new();
        let mut i = 0usize;
        while let Some(s) = section(doc, &alloc::format!("{prefix}{i}")) {
            out.push(s);
            i += 1;
        }
        out
    }

    fn gear(s: &SectionMap) -> GearSpeed {
        GearSpeed {
            penalty: num(s, key::PENALTY).unwrap_or(0.0),
            dash: num(s, key::DASH).unwrap_or(0.0),
            max_stamina: num(s, key::MAX_STAMINA).unwrap_or(0.0),
        }
    }

    impl SpeedTable {
        /// The table from the parsed ROM documents (`attachments` may be missing: no backpack / goggle terms).
        pub fn from_docs(
            models: &ButeDoc,
            weapons: &ButeDoc,
            attachments: Option<&ButeDoc>,
        ) -> SpeedTable {
            let mut t = SpeedTable::default();
            for (name, sec) in weapons.iter() {
                let Some(n) = name
                    .strip_prefix(key::WEAPON)
                    .filter(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()))
                    .and_then(|r| r.parse::<u16>().ok())
                else {
                    continue;
                };
                if let Some(p) = num(sec, key::PENALTY) {
                    t.weapons.insert(n, p);
                }
            }
            let body = |s: &SectionMap| BodySpeed {
                penalty: num(s, key::PENALTY).unwrap_or(0.0),
                dash: num(s, key::DASH).unwrap_or(0.0),
                figure: int(s, key::FIGURE_OF_BODY)
                    .filter(|&f| f >= 0)
                    .map(|f| f as u32),
                max_stamina: num(s, key::MAX_STAMINA).unwrap_or(0.0),
            };
            let base = rows(models, key::MODEL);
            let ai = rows(models, key::AI_MODEL);
            let over = rows(models, key::OVERRIDE_MODEL);
            t.bodies = base
                .iter()
                .chain(ai.iter())
                .chain(over.iter())
                .map(|s| body(s))
                .collect();
            t.override_base = base.len() + ai.len();
            t.body_files = base
                .iter()
                .map(|s| value(s, key::MODEL_FILE).cloned().unwrap_or_default())
                .collect();
            t.figures = rows(models, key::FIGURE)
                .into_iter()
                .map(|s| FigureSpeed {
                    speed_bonus: num(s, key::SPEED).unwrap_or(0.0),
                    jump_bonus: num(s, key::JUMP).unwrap_or(0.0),
                    weight: num(s, key::WEIGHT).unwrap_or(0.0),
                    dash_mul: num(s, key::DASH).unwrap_or(FIGURE_DASH_DEFAULT),
                    max_stamina: gauge_int(s, key::FIGURE_MAX_STAMINA),
                    dash_gauge: gauge_int(s, key::DASH_GAUGE),
                    dash_jump_gauge: gauge_int(s, key::DASH_JUMP_GAUGE),
                    gauge_rec: gauge_int(s, key::GAUGE_REC),
                })
                .collect();
            t.heads = rows(models, key::HAIR).into_iter().map(gear).collect();
            if let Some(a) = attachments {
                t.backpacks = rows(a, key::BACKPACK).into_iter().map(gear).collect();
                t.goggles = rows(a, key::GOGGLE).into_iter().map(gear).collect();
            }
            t
        }

        /// The table from the user's ROM (`RezVfs`): an error names the table that could not be read.
        pub fn from_vfs(vfs: &RezVfs) -> Result<SpeedTable, String> {
            let read = |entry: &str| {
                ButeDoc::try_from_vfs(vfs, ATTRIBUTES_REZ, entry)
                    .map_err(|e| alloc::format!("{entry}: {e}"))
            };
            let models = read(MODELS_ENTRY)?.ok_or_else(|| MODELS_ENTRY.to_string())?;
            let weapons = read(WEAPONS_ENTRY)?.ok_or_else(|| WEAPONS_ENTRY.to_string())?;
            let attachments = read(ATTACHMENTS_ENTRY)?;
            Ok(SpeedTable::from_docs(
                &models,
                &weapons,
                attachments.as_ref(),
            ))
        }
    }
}

#[cfg(feature = "rom")]
pub use rom::{key, ATTACHMENTS_ENTRY, ATTRIBUTES_REZ, MODELS_ENTRY, WEAPONS_ENTRY};

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> SpeedTable {
        // made-up rows (not ROM values)
        let mut t = SpeedTable::default();
        t.weapons.insert(1, 0.0);
        t.weapons.insert(2, 0.25);
        t.bodies = alloc::vec![
            BodySpeed {
                penalty: 0.05,
                dash: 0.5,
                figure: Some(1),
                ..Default::default()
            },
            BodySpeed::default(),
            // override slot 0
            BodySpeed {
                penalty: 0.005,
                dash: 0.05,
                figure: None,
                ..Default::default()
            },
            // override slot 1
            BodySpeed {
                penalty: -0.2,
                dash: 0.6,
                figure: None,
                ..Default::default()
            },
        ];
        t.override_base = 2;
        t.figures = alloc::vec![
            FigureSpeed {
                speed_bonus: 0.0,
                jump_bonus: 0.0,
                weight: 0.0,
                dash_mul: 1.0,
                ..Default::default()
            },
            FigureSpeed {
                speed_bonus: 0.02,
                jump_bonus: 0.1,
                weight: 0.01,
                dash_mul: 1.1,
                ..Default::default()
            },
        ];
        t.heads = alloc::vec![GearSpeed {
            penalty: 0.03,
            dash: -0.1,
            ..Default::default()
        }];
        t.backpacks = alloc::vec![GearSpeed {
            penalty: 0.01,
            dash: 0.0,
            ..Default::default()
        }];
        t.goggles = alloc::vec![GearSpeed {
            penalty: 0.0,
            dash: 0.2,
            ..Default::default()
        }];
        t
    }

    #[test]
    fn penalty_and_sprint_sum_every_term() {
        let t = table();
        let l = Loadout {
            weapon: Some(2),
            body: Some(0),
            head: Some(0),
            backpack: Some(0),
            goggles: Some(0),
            ..Loadout::default()
        };
        let m = t.mods(&l);
        // 0.25 + 0.05 + 0.01 (weight) + 0.01 (backpack) + 0.03 (head) - 0.02 (speed bonus)
        assert!((m.penalty - 0.33).abs() < 1e-6, "{}", m.penalty);
        // 0.5 + (-0.1) + 0.0 + 0.2
        assert!((m.sprint_add - 0.6).abs() < 1e-6, "{}", m.sprint_add);
        assert_eq!(m.sprint_mul, Some(1.1));
        assert_eq!(m.jump_bonus, 0.1);
    }

    #[test]
    fn knife_and_rifle_differ_only_by_the_weapon_row() {
        let t = table();
        let knife = t.mods(&Loadout {
            weapon: Some(1),
            body: Some(1),
            ..Loadout::default()
        });
        let rifle = t.mods(&Loadout {
            weapon: Some(2),
            body: Some(1),
            ..Loadout::default()
        });
        assert_eq!(knife.penalty, 0.0);
        assert_eq!(rifle.penalty, 0.25);
        assert_eq!(knife.sprint_add, rifle.sprint_add);
        assert_eq!(knife.sprint_mul, rifle.sprint_mul);
    }

    #[test]
    fn override_rows_win_only_above_their_thresholds() {
        let t = table();
        let small = Loadout {
            body: Some(0),
            body_override: Some(0),
            ..Loadout::default()
        };
        assert_eq!(t.body_penalty(&small), 0.05, "|0.005| <= 0.01: base row");
        assert_eq!(t.body_dash(&small), 0.5, "0.05 <= 0.1: base row");
        let big = Loadout {
            body_override: Some(1),
            ..small
        };
        assert_eq!(t.body_penalty(&big), -0.2);
        assert_eq!(t.body_dash(&big), 0.6);
        let missing = Loadout {
            body_override: Some(9),
            ..small
        };
        assert_eq!(t.body_penalty(&missing), 0.05);
    }

    #[test]
    fn gauge_rates_sum_every_term_with_the_override_rule() {
        // made-up rows (chosen to differ from every ROM figure row), binary fractions so the sum is exact
        let mut t = table();
        t.figures[1].max_stamina = Some(120);
        t.figures[1].dash_gauge = Some(40);
        t.figures[1].gauge_rec = Some(15);
        t.heads[0].max_stamina = 0.25;
        t.goggles[0].max_stamina = 0.125;
        t.bodies[0].max_stamina = 0.5;
        t.backpacks[0].max_stamina = 0.0625;
        t.bodies[2].max_stamina = 0.05; // override slot 0: not above 0.1, the base row counts
        t.bodies[3].max_stamina = 0.75; // override slot 1
        let all = Loadout {
            body: Some(0),
            head: Some(0),
            goggles: Some(0),
            backpack: Some(0),
            ..Loadout::default()
        };
        let r = t.stamina(&all).unwrap();
        // trunc(120 x (1 + 0.25 + 0.125 + 0.5 + 0.0625)) = trunc(232.5)
        assert_eq!((r.max, r.drain, r.regen), (232, 40, 15));
        assert_eq!(t.mods(&all).stamina, Some(r));
        let small = Loadout {
            body_override: Some(0),
            ..all
        };
        assert_eq!(t.stamina(&small).unwrap().max, 232);
        let big = Loadout {
            body_override: Some(1),
            ..all
        };
        assert_eq!(t.stamina(&big).unwrap().max, 262, "trunc(120 x 2.1875)");
        let bare = Loadout {
            body: Some(0),
            ..Loadout::default()
        };
        assert_eq!(t.stamina(&bare).unwrap().max, 180, "trunc(120 x 1.5)");
        // no figure row, or a figure row without Maxstamina: no gauge rule
        assert_eq!(t.stamina(&Loadout::default()), None);
        t.figures[1].max_stamina = None;
        assert_eq!(t.stamina(&bare), None);
    }

    #[test]
    fn nothing_known_is_neutral_but_keeps_the_dash_default() {
        let m = SpeedTable::default().mods(&Loadout::default());
        assert_eq!((m.penalty, m.sprint_add, m.jump_bonus), (0.0, 0.0, 0.0));
        assert_eq!(m.sprint_mul, Some(FIGURE_DASH_DEFAULT));
    }
}

#[cfg(all(test, feature = "rom"))]
mod rom_tests {
    use super::*;
    use dc_bute::ButeDoc;

    // made-up tables in the ROM's text form (not ROM content; the gauge triple differs from every ROM figure row)
    const MODELS: &str =
        "[Model0]\nModelFile=\"A\\Body0.ltb\"\nMovePenalty = 0.2\t// note\nDashSpeed = 0.5\n \
        FigureType = 1\n\
        [Model1]\nModelFile=\"A\\Body1.ltb\"\n\
        [AIModel0]\nMovePenalty = 0.9\n\
        [Overidable_Model0]\nMovePenalty = -0.3\nDashSpeed = 0.7\n\
        [FigureType0]\nVariableSpeed = 0.0\n\
        [FigureType1]\nvariablespeed = 0.05\nVariableWeight = 0.01\nVariableJump = 0.1\n\
        Maxstamina = 120\n\
        DashGauge = 40\n\
        DashJumpGauge = 5\n\
        GaugeRec = 15\n\
        [Hair0]\nMovePenalty = 0.02\nDashSpeed = -0.1\n\
        MaxStamina = 0.25\n";
    const WEAPONS: &str =
        "[Weapon3]\nMovePenalty = 0.12\n[Weapon4]\nName=\"x\"\n[WeaponOrder]\nClass0=\"y\"\n";
    const ATTACH: &str =
        "[Backpack0]\nMovePenalty = 0.03\nDashSpeed = 0\n[Goggle0]\nDashSpeed = 0.25\n";

    #[test]
    fn rows_parse_in_the_2009_order_and_case() {
        let t = SpeedTable::from_docs(
            &ButeDoc::parse(MODELS),
            &ButeDoc::parse(WEAPONS),
            Some(&ButeDoc::parse(ATTACH)),
        );
        assert_eq!(t.weapons.get(&3), Some(&0.12));
        assert_eq!(t.weapons.get(&4), None, "a row without the key counts 0");
        assert_eq!(t.bodies.len(), 4);
        assert_eq!(t.override_base, 3);
        assert_eq!(t.bodies[0].figure, Some(1));
        assert_eq!(
            t.figures[0].dash_mul, FIGURE_DASH_DEFAULT,
            "missing key: 1.4"
        );
        assert_eq!(t.figures[1].speed_bonus, 0.05, "keys match without case");
        assert_eq!(t.body_row_of_file("a/body1.LTB"), Some(1));
        assert_eq!(t.body_row_of_file("A\\Body2.ltb"), None);
        assert_eq!(t.body_row_of_file("A\\BODY0.ltb"), Some(0));
        let m = t.mods(&Loadout {
            weapon: Some(3),
            body: Some(0),
            head: Some(0),
            backpack: Some(0),
            goggles: Some(0),
            ..Loadout::default()
        });
        // 0.12 + 0.2 + 0.01 + 0.03 + 0.02 - 0.05
        assert!((m.penalty - 0.33).abs() < 1e-6, "{}", m.penalty);
        // 0.5 - 0.1 + 0 + 0.25
        assert!((m.sprint_add - 0.65).abs() < 1e-6, "{}", m.sprint_add);
        assert_eq!(m.sprint_mul, Some(FIGURE_DASH_DEFAULT));
        assert_eq!(m.jump_bonus, 0.1);
        let o = t.mods(&Loadout {
            body: Some(0),
            body_override: Some(0),
            ..Loadout::default()
        });
        assert!((o.penalty - (-0.3 + 0.01 - 0.05)).abs() < 1e-6);
        assert!((o.sprint_add - 0.7).abs() < 1e-6);
        let f = t.figures[1];
        assert_eq!(
            (f.max_stamina, f.dash_gauge, f.dash_jump_gauge, f.gauge_rec),
            (Some(120), Some(40), Some(5), Some(15))
        );
        assert_eq!(t.figures[0].max_stamina, None);
        let g = t
            .stamina(&Loadout {
                body: Some(0),
                head: Some(0),
                ..Loadout::default()
            })
            .unwrap();
        // figure row 1 (FigureType of Model0): trunc(120 x (1 + 0.25))
        assert_eq!((g.max, g.drain, g.regen), (150, 40, 15));
    }
}
