//! The speed rows of the loadout (weapon / body / figure / gear `MovePenalty` and `DashSpeed`) read from the
//! user's own `ATTRIBUTES.rez` into dc-sim's [`SpeedTable`] (MOVEMENT_2009.md section 3). The formula lives in
//! dc-sim (`SpeedTable::mods`, no ROM); only the reading is here. Nothing here is ROM data: the key and section
//! names are the 2009 reader's own names, every value comes from the ROM at run time.
// SPEC: MOVEMENT_2009.md §3, §7

use crate::{eq_fold, ButeDoc, SectionMap};
use dc_rez::dc_vfs::Vfs;
use dc_sim::loadout::{BodySpeed, FigureSpeed, GearSpeed, SpeedTable, FIGURE_DASH_DEFAULT};

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
    while let Some(s) = section(doc, &format!("{prefix}{i}")) {
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

/// The table from the parsed ROM documents (`attachments` may be missing: no backpack / goggle terms).
pub fn from_docs(models: &ButeDoc, weapons: &ButeDoc, attachments: Option<&ButeDoc>) -> SpeedTable {
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

/// The table from the user's ROM (a file system over its archives, e.g. `RezVfs`): an error names the table
/// that could not be read.
pub fn from_vfs(vfs: &Vfs) -> Result<SpeedTable, String> {
    let read = |entry: &str| {
        ButeDoc::try_from_vfs(vfs, ATTRIBUTES_REZ, entry).map_err(|e| format!("{entry}: {e}"))
    };
    let models = read(MODELS_ENTRY)?.ok_or_else(|| MODELS_ENTRY.to_string())?;
    let weapons = read(WEAPONS_ENTRY)?.ok_or_else(|| WEAPONS_ENTRY.to_string())?;
    let attachments = read(ATTACHMENTS_ENTRY)?;
    Ok(from_docs(&models, &weapons, attachments.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_sim::Loadout;

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
        let t = from_docs(
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
