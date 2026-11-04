//! View-models of the panels, with no egui types: the inspector's typed rows for a selection, the marker list
//! of the viewport, the target a problem's path points at, and the editor's selection type.
// DC-DEFAULT: MAP-64 (panels: tools, inspector, problems, status)

use crate::diag::Diagnostic;
use crate::project::{MapObject, Project, PropValue};
use crate::render_sw::marker_style;
use crate::vocab_names as v;
use dc_world::vocab::class_named;
use dc_world::PropKind;

/// What the editor has selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    Object(String),
    Solid { id: String, face: Option<String> },
}

impl Selection {
    pub fn id(&self) -> &str {
        match self {
            Selection::Object(id) | Selection::Solid { id, .. } => id,
        }
    }
}

/// One inspector row of an object's class property.
#[derive(Debug, Clone, PartialEq)]
pub struct PropRow {
    pub name: String,
    pub kind: PropKind,
    pub required: bool,
    pub doc: String,
    /// The current value, or `None` when the object does not carry the property yet.
    pub value: Option<PropValue>,
}

/// The class properties of an object as rows: the vocabulary's list in its order (with the current values),
/// then any extra property the object carries that the vocabulary does not list.
pub fn prop_rows(o: &MapObject) -> Vec<PropRow> {
    let mut rows = Vec::new();
    if let Some(spec) = class_named(&o.class) {
        for p in spec.props {
            rows.push(PropRow {
                name: p.name.to_string(),
                kind: p.kind,
                required: p.required,
                doc: p.doc.to_string(),
                value: o.props.get(p.name).cloned(),
            });
        }
    }
    for (k, val) in &o.props {
        if !rows.iter().any(|r| r.name == *k) {
            rows.push(PropRow {
                name: k.clone(),
                kind: match val {
                    PropValue::Bool(_) => PropKind::Bool,
                    PropValue::Num(_) => PropKind::Real,
                    PropValue::Text(_) => PropKind::Text,
                    PropValue::Vec3(_) => PropKind::Vec3,
                    PropValue::Rgb { .. } => PropKind::Rgb,
                },
                required: false,
                doc: "not in the vocabulary".into(),
                value: Some(val.clone()),
            });
        }
    }
    rows
}

/// A marker of the viewport overlay.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkerModel {
    pub id: String,
    pub class: String,
    pub pos: [f32; 3],
    pub colour: [u8; 3],
    /// The legend key (`dc.player_spawn team 1`).
    pub key: String,
}

/// One marker per point object, coloured by class and team ([`marker_style`]).
pub fn markers(p: &Project) -> Vec<MarkerModel> {
    p.objects
        .iter()
        .map(|o| {
            let team = o.text(v::PROP_TEAM).unwrap_or("");
            let (key, colour) = marker_style(&o.class, team);
            MarkerModel {
                id: o.id.clone(),
                class: o.class.clone(),
                pos: o.pos,
                colour,
                key,
            }
        })
        .collect()
}

/// What a diagnostic's path points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProblemTarget {
    Object(usize),
    Solid(usize),
    Material(usize),
    Import(usize),
    Terrain,
    Settings,
    Meta,
}

/// Parse a diagnostic path (`objects[3].props.team`, `solids[2]`, `terrain.layers[0]`, `settings.fog_far`,
/// `mod_meta.modes[1]`) into the thing the editor should select or show.
pub fn problem_target(path: &str) -> Option<ProblemTarget> {
    let indexed = |prefix: &str| -> Option<usize> {
        let rest = path.strip_prefix(prefix)?.strip_prefix('[')?;
        let end = rest.find(']')?;
        rest[..end].parse().ok()
    };
    if let Some(i) = indexed("objects") {
        return Some(ProblemTarget::Object(i));
    }
    if let Some(i) = indexed("solids") {
        return Some(ProblemTarget::Solid(i));
    }
    if let Some(i) = indexed("materials") {
        return Some(ProblemTarget::Material(i));
    }
    if let Some(i) = indexed("imports") {
        return Some(ProblemTarget::Import(i));
    }
    if path == "terrain" || path.starts_with("terrain.") {
        return Some(ProblemTarget::Terrain);
    }
    if path == "settings" || path.starts_with("settings.") {
        return Some(ProblemTarget::Settings);
    }
    if path == "mod_meta" || path.starts_with("mod_meta.") || path == "name" {
        return Some(ProblemTarget::Meta);
    }
    None
}

/// The selection a problem row click makes, when its path names a placed thing that still exists.
pub fn selection_of_problem(p: &Project, d: &Diagnostic) -> Option<Selection> {
    match problem_target(&d.path)? {
        ProblemTarget::Object(i) => p.objects.get(i).map(|o| Selection::Object(o.id.clone())),
        ProblemTarget::Solid(i) => p.solids.get(i).map(|s| Selection::Solid {
            id: s.id.clone(),
            face: None,
        }),
        _ => None,
    }
}

/// A one-line summary of a problem row.
pub fn problem_line(d: &Diagnostic) -> String {
    format!(
        "{:?} {} at {}: {}",
        d.severity,
        d.code.as_str(),
        d.path,
        d.message
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Code;
    use crate::template::{make, Template};

    #[test]
    fn inspector_rows_follow_the_vocabulary_then_the_extras() {
        let made = make(Template::Flat, "model", "", 1);
        let spawn = made
            .project
            .objects
            .iter()
            .find(|o| o.class == v::CLASS_PLAYER_SPAWN)
            .unwrap();
        let rows = prop_rows(spawn);
        assert_eq!(rows[0].name, v::PROP_TEAM);
        assert_eq!(rows[0].kind, PropKind::Text);
        assert!(!rows[0].required, "a spawn's team defaults to any");
        assert_eq!(rows[1].name, v::PROP_MODES);
        assert!(rows.iter().all(|r| r.value.is_some()));
        let site = made
            .project
            .objects
            .iter()
            .find(|o| o.class == v::CLASS_BOMB_SITE)
            .unwrap();
        let rows = prop_rows(site);
        assert!(rows
            .iter()
            .any(|r| r.name == v::PROP_EXTENT && r.required && r.kind == PropKind::Vec3));
        let mut odd = spawn.clone();
        odd.props
            .insert("custom".into(), PropValue::Rgb { rgb: [1, 2, 3] });
        let rows = prop_rows(&odd);
        let extra = rows.last().unwrap();
        assert_eq!(extra.name, "custom");
        assert_eq!(extra.kind, PropKind::Rgb);
        assert!(!extra.required);
        let ms = markers(&made.project);
        assert_eq!(ms.len(), made.project.objects.len());
        let t1 = ms
            .iter()
            .find(|m| m.key == format!("{} team 1", v::CLASS_PLAYER_SPAWN))
            .unwrap();
        assert_eq!(t1.colour, marker_style(v::CLASS_PLAYER_SPAWN, "1").1);
    }

    #[test]
    fn problem_paths_name_their_targets() {
        assert_eq!(
            problem_target("objects[3].props.team"),
            Some(ProblemTarget::Object(3))
        );
        assert_eq!(problem_target("solids[12]"), Some(ProblemTarget::Solid(12)));
        assert_eq!(
            problem_target("materials[0].texture"),
            Some(ProblemTarget::Material(0))
        );
        assert_eq!(
            problem_target("imports[1].file"),
            Some(ProblemTarget::Import(1))
        );
        assert_eq!(
            problem_target("terrain.layers[0]"),
            Some(ProblemTarget::Terrain)
        );
        assert_eq!(
            problem_target("settings.fog_far"),
            Some(ProblemTarget::Settings)
        );
        assert_eq!(
            problem_target("mod_meta.modes[1]"),
            Some(ProblemTarget::Meta)
        );
        assert_eq!(problem_target("name"), Some(ProblemTarget::Meta));
        assert_eq!(problem_target("ops[2].pos"), None);
        assert_eq!(problem_target("objects[x]"), None);
        let made = make(Template::Flat, "model", "", 1);
        let d = Diagnostic::new(Code::SpawnCount, "objects[0].props.team", "x");
        assert_eq!(
            selection_of_problem(&made.project, &d),
            Some(Selection::Object(made.project.objects[0].id.clone()))
        );
        let d = Diagnostic::new(Code::SpawnCount, "objects[9999]", "x");
        assert_eq!(selection_of_problem(&made.project, &d), None);
        let d = Diagnostic::new(Code::MaterialMissing, "solids[0]", "x");
        assert_eq!(
            selection_of_problem(&made.project, &d),
            Some(Selection::Solid {
                id: made.project.solids[0].id.clone(),
                face: None
            })
        );
        assert!(problem_line(&d).contains("material.missing"));
        assert_eq!(Selection::Object("a_1".into()).id(), "a_1");
    }
}
