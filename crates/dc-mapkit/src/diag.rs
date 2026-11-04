//! Diagnostics: one `Diagnostic` type shared by `validate`, `apply`, `build` and the stubs (MAPKIT_PLAN.md 5.2:
//! "the diagnostic codes are one list printed by `schema`"), and the exit-code rule.
//!
//! Exit codes (DC-DEFAULT: MAP-13): 0 = no error (warnings and infos alone keep 0), 1 = at least one error
//! diagnostic, 2 = usage (bad flags, unknown verb; never produced from a `Diagnostics`).

use serde::{Deserialize, Serialize};
use std::fmt;

/// Severity of a diagnostic. Only `Error` changes the exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

macro_rules! codes {
    ($( $variant:ident = $text:literal, $sev:ident, $doc:literal; )*) => {
        /// Every diagnostic code the tool can produce. `schema` prints the list; the text form is `as_str`,
        /// and it is the ONE spelling: `--json`, the MCP tool results, the text output, `schema` and the docs
        /// all use the dotted text (`Serialize` / `Deserialize` go through it, DC-DEFAULT: MAP-13).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        pub enum Code {
            $( #[doc = $doc] $variant, )*
        }

        impl Code {
            /// The dotted text of the code (`"path.invalid"`), what `--json` prints.
            pub fn as_str(self) -> &'static str {
                match self { $( Code::$variant => $text, )* }
            }

            /// The code of its dotted text (`"path.invalid"` -> `PathInvalid`), the inverse of `as_str`.
            pub fn from_text(text: &str) -> Option<Code> {
                match text { $( $text => Some(Code::$variant), )* _ => None }
            }

            /// The severity the code carries unless the producer overrides it.
            pub fn default_severity(self) -> Severity {
                match self { $( Code::$variant => Severity::$sev, )* }
            }

            /// One line saying what the code means (the `schema` listing).
            pub fn doc(self) -> &'static str {
                match self { $( Code::$variant => $doc, )* }
            }

            /// Every code, in declaration order.
            pub const ALL: &'static [Code] = &[ $( Code::$variant, )* ];
        }

        impl Serialize for Code {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for Code {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Code, D::Error> {
                let text = <std::borrow::Cow<'de, str>>::deserialize(d)?;
                Code::from_text(&text).ok_or_else(|| {
                    serde::de::Error::custom(format!("`{text}` is not a dc-mapkit diagnostic code"))
                })
            }
        }
    };
}

codes! {
    ProjectFormat = "project.format", Error, "map.json `format` is not the supported version (1)";
    ProjectParse = "project.parse", Error, "map.json is not valid JSON of the project schema";
    ProjectIo = "project.io", Error, "a project file could not be read or written";
    ProjectKey = "project.key", Error, "the map key is not 1..32 characters of a-z 0-9 _";
    ProjectName = "project.name", Error, "the map name is empty";
    PathInvalid = "path.invalid", Error, "a path has a backslash, a drive letter, a leading slash or a `..` segment";
    PathCase = "path.case", Warning, "two assets/ paths differ only by letter case (one of them breaks on a case-sensitive host)";
    IdDuplicate = "id.duplicate", Error, "two placed things share one id";
    IdUnknown = "id.unknown", Error, "an op or reference names an id the project does not have";
    IdFormat = "id.format", Error, "an id is not `<prefix>_<n>` with a-z 0-9 _ only";
    IdNext = "id.next", Error, "`next_id` is not above every numeric id suffix in the project";
    MaterialMissing = "material.missing", Error, "a surface names a material the project does not define";
    MaterialUnused = "material.unused", Info, "a material is defined but no surface uses it";
    TextureMissing = "texture.missing", Error, "an own texture file is missing under the project";
    TextureFormat = "texture.format", Error, "an own texture is not a PNG the tool can read";
    RomUnverified = "rom.unverified", Warning, "a ROM texture path could not be checked (no ROM at hand)";
    ObjectClass = "object.class", Error, "an object class is not in the DeCombat vocabulary";
    ObjectProp = "object.prop", Error, "an object property has the wrong value kind";
    SpawnCount = "spawn.count", Warning, "a team has fewer than 8 spawns, or fewer than 16 team-less spawns exist";
    SpawnTeam = "spawn.team", Error, "a spawn, zone or flag has a team text other than \"1\", \"2\" or \"\"";
    SpawnFloor = "spawn.floor", Warning, "no level floor lies within 8 units below a spawn's body (checked by `build` the way the server drops a spawn)";
    SpawnBrushFloor = "spawn.brush_floor", Warning, "the floor under a spawn is a dc.world_model / dc.breakable brush top; the server's spawn drop ignores brushes and drops the player through it (checked by `build`)";
    SpawnClear = "spawn.clear", Warning, "the server's 106 x 48 standing body of a spawn is not free where it comes to rest (checked by `build`)";
    BombCount = "bomb.count", Error, "a map that plays BOMB needs exactly two bomb sites";
    BombNumber = "bomb.number", Error, "bomb sites are not numbered 0 and 1";
    FlagCount = "flag.count", Error, "a map that plays FLAG needs one flag zone and one flag per team";
    ModeUnknown = "mode.unknown", Warning, "a mode name is not a DeCombat playable mode";
    ExtentLimit = "extent.limit", Error, "the world spans more than 65,536 units on one axis (pos48 cannot carry it)";
    ExtentLarge = "extent.large", Warning, "the world spans more than 32,768 units on one axis (pos48 precision drops below 0.125)";
    ExtentEnclose = "extent.enclose", Warning, "an object lies outside the box of the world geometry (positions outside are clamped)";
    SizeEstimate = "size.estimate", Warning, "the estimated package size is above the lobby's 16 MiB default";
    SizeLimit = "size.limit", Error, "the estimated package size is above the client's 32 MiB hard cap";
    TerrainSize = "terrain.size", Error, "terrain.r16 does not hold (cells_x + 1) * (cells_z + 1) heights";
    TerrainSettings = "terrain.settings", Error, "terrain cells or cell size are zero, or the height scale is not positive";
    TerrainMissing = "terrain.missing", Error, "a terrain op needs a terrain and the project has none";
    SolidSize = "solid.size", Error, "a solid has a non-positive size";
    SolidCircle = "solid.circle", Error, "a cylinder's stored circle does not have `sides` points";
    SolidShape = "solid.shape", Error, "a prism outline has fewer than 3 points or is not convex, or a cylinder / prism has more than 255 corners (the world file stores a polygon's corner count in one byte)";
    SolidDuplicate = "solid.duplicate", Warning, "solids repeat another solid exactly, usually a generator that ran twice";
    ObjectDuplicate = "object.duplicate", Warning, "objects repeat another object exactly, usually a generator that ran twice";
    PictureMissing = "picture.missing", Warning, "the map has no preview or loading picture, so the waiting room and the loading screen stay bare";
    RenderClient = "render.client", Error, "`render --textured` could not run dc-client or got no picture from it";
    MetaInvalid = "meta.invalid", Error, "mod metadata breaks the manifest rules (id, description 20..500, author 1..100)";
    OpInvalid = "op.invalid", Error, "an op is malformed or its arguments are out of range";
    OpUnknownOp = "op.unknown", Error, "an op name is not in the schema";
    HistoryIo = "history.io", Error, "a history snapshot could not be read or written";
    HistoryMissing = "history.missing", Error, "the history entry does not exist";
    BuildFail = "build.fail", Error, "the world could not be built";
    TextureAssets = "texture.assets", Error, "an own texture is not under assets/ (the client resolves @mod: paths under assets/ only)";
    WorldParse = "world.parse", Error, "the built world does not parse back (a tool bug: report it with the project)";
    WorldLayout = "world.layout", Error, "the built world's sections do not tile the file (a tool bug: report it with the project)";
    WorldLevel = "world.level", Error, "the world does not have exactly one level model (the model no object names, bsp flag bit 1 clear), or an object is named `level`";
    BspUnreferenced = "bsp.unreferenced", Error, "a solid polygon is referenced by no BSP node (the collision walk would never find it)";
    BspBudget = "bsp.budget", Error, "the collision tree of a model ran past its node budget: polygons that straddle every candidate plane (long rotated walls crossing a common area) are duplicated into every subtree; split or realign the named solids";
    SkySurface = "sky.surface", Error, "a sky face (surface flag 0x10) carries texflags other than the client's sky surface id 0x6e (shots into it would play impacts and lay marks)";
    BrushHelper = "brush.helper", Error, "a brush model carries a plane-only or flag-less polygon (helpers belong to the level model only)";
    LadderConvex = "ladder.convex", Error, "a ladder brush is not convex (the server takes the region inside every plane of it)";
    KillvolClosed = "killvol.closed", Error, "a kill volume brush is not closed (every edge must be shared by exactly two faces)";
    ExtentModel = "extent.model", Warning, "a model's box reaches outside the world extents (positions there are clamped on the wire)";
    SkyPortals = "sky.portals", Warning, "the level has sky faces but its render block carries no sky portal (the sky pass is skipped)";
    SkyModel = "sky.model", Warning, "the level has sky faces but no dc.sky_model brush with a render world (nothing is drawn behind them)";

    RenderView = "render.view", Error, "the requested view cannot be set up (no such spawn)";
    ExportRomPath = "export.rom_path", Error, "a material uses a ROM texture path and --no-rom-paths was given";
    ExportPack = "export.pack", Error, "dc-modkit refused the exported mod folder (validate / lint / pack)";
    RomMissing = "rom.missing", Error, "no ROM folder was found (--rom, CA_ROM, <profile>/mapkit.toml, a sibling Game/ folder)";
    RomRead = "rom.read", Error, "a ROM archive or table could not be read";

    ScriptError = "script.error", Error, "a Lua generator failed: a syntax error, a runtime error or a map.* call with bad arguments (the path names the script line)";
    ScriptBudget = "script.budget", Error, "a Lua generator ran out of its instruction budget (`run --budget N`)";
    GuiLaunch = "gui.launch", Error, "the editor binary `dc-mapkit-gui` was not found next to the tool or on PATH, or could not be started";


    PlayBinaries = "play.binaries", Error, "`play` could not find dc-lobby-server, dc-dserver or dc-client (next to dc-mapkit, on PATH, in target/{debug,release}; or --binaries DIR)";
    PlayConfig = "play.config", Error, "`play`'s generated server.toml failed dc-config's check, or no free loopback ports were found";
    PlayLobby = "play.lobby", Error, "`play`'s lobby did not start, did not listen, or did not list the map";
    PlayClient = "play.client", Error, "`play`'s client did not start, or the headless run played no match on the `@mod:` world";
    NotImplemented = "not_implemented", Error, "this verb is not implemented in this stage of dc-mapkit";
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One finding: what, how bad, where (a path into map.json such as `objects[3].pos`, or a file), the message
/// and an optional mechanical fix hint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: Code,
    pub severity: Severity,
    pub path: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl Diagnostic {
    pub fn new(code: Code, path: impl Into<String>, message: impl Into<String>) -> Self {
        Diagnostic {
            code,
            severity: code.default_severity(),
            path: path.into(),
            message: message.into(),
            fix: None,
        }
    }

    pub fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }

    pub fn with_severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        };
        write!(f, "{sev} [{}] {}: {}", self.code, self.path, self.message)?;
        if let Some(fix) = &self.fix {
            write!(f, " (fix: {fix})")?;
        }
        Ok(())
    }
}

/// An ordered list of findings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Diagnostics {
    pub items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn one(d: Diagnostic) -> Self {
        Diagnostics { items: vec![d] }
    }

    pub fn push(&mut self, d: Diagnostic) {
        self.items.push(d);
    }

    pub fn error(&mut self, code: Code, path: impl Into<String>, message: impl Into<String>) {
        self.push(Diagnostic::new(code, path, message).with_severity(Severity::Error));
    }

    pub fn warn(&mut self, code: Code, path: impl Into<String>, message: impl Into<String>) {
        self.push(Diagnostic::new(code, path, message).with_severity(Severity::Warning));
    }

    pub fn info(&mut self, code: Code, path: impl Into<String>, message: impl Into<String>) {
        self.push(Diagnostic::new(code, path, message).with_severity(Severity::Info));
    }

    pub fn extend(&mut self, other: Diagnostics) {
        self.items.extend(other.items);
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn has_errors(&self) -> bool {
        self.items.iter().any(|d| d.severity == Severity::Error)
    }

    pub fn count(&self, severity: Severity) -> usize {
        self.items.iter().filter(|d| d.severity == severity).count()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.items.iter()
    }

    /// The process exit code these findings imply (DC-DEFAULT: MAP-13): 1 with an error, else 0.
    pub fn exit_code(&self) -> i32 {
        if self.has_errors() {
            EXIT_DIAGNOSTICS
        } else {
            EXIT_OK
        }
    }

    /// `Ok(())` without errors, else the list.
    pub fn into_result(self) -> Result<Diagnostics, Diagnostics> {
        if self.has_errors() {
            Err(self)
        } else {
            Ok(self)
        }
    }
}

/// Exit code: no error.
pub const EXIT_OK: i32 = 0;
/// Exit code: at least one error diagnostic.
pub const EXIT_DIAGNOSTICS: i32 = 1;
/// Exit code: usage (bad flags, unknown verb, unreadable arguments).
pub const EXIT_USAGE: i32 = 2;

/// The `not_implemented` answer every stub verb gives in this stage.
pub fn not_implemented(verb: &str) -> Diagnostics {
    Diagnostics::one(
        Diagnostic::new(Code::NotImplemented, verb, format!("`{verb}` is not implemented in this stage of dc-mapkit"))
            .with_fix("use the data verbs (new, info, validate, apply, terrain, get, schema, snapshot, history, revert)"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_dotted_and_serialise_as_their_text() {
        let mut seen = std::collections::BTreeSet::new();
        for c in Code::ALL {
            assert!(seen.insert(c.as_str()), "duplicate code {c}");
            assert!(c
                .as_str()
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'));
            assert!(!c.doc().is_empty());
        }
        let d = Diagnostic::new(Code::PathInvalid, "imports[0].file", "backslash");
        let v = serde_json::to_value(&d).unwrap();
        // one spelling everywhere: the JSON code is the dotted `as_str` text (MAP-13)
        assert_eq!(v["code"], "path.invalid");
        assert_eq!(v["severity"], "error");
        assert!(v.get("fix").is_none());
        let back: Diagnostic = serde_json::from_value(v).unwrap();
        assert_eq!(back, d);
        for c in Code::ALL {
            assert_eq!(serde_json::to_value(c).unwrap(), c.as_str());
            assert_eq!(Code::from_text(c.as_str()), Some(*c));
            let round: Code = serde_json::from_str(&format!("\"{}\"", c.as_str())).unwrap();
            assert_eq!(round, *c);
        }
        assert_eq!(
            serde_json::to_value(Code::ScriptBudget).unwrap(),
            "script.budget"
        );
        assert!(serde_json::from_str::<Code>("\"id_unknown\"").is_err());
        assert_eq!(Code::from_text("id.unknown"), Some(Code::IdUnknown));
    }

    #[test]
    fn exit_code_follows_errors_only() {
        let mut d = Diagnostics::new();
        assert_eq!(d.exit_code(), 0);
        d.warn(Code::SpawnCount, "objects", "7 spawns");
        d.info(Code::MaterialUnused, "materials[2]", "unused");
        assert_eq!(d.exit_code(), 0);
        assert!(d.clone().into_result().is_ok());
        d.error(Code::IdDuplicate, "solids[1].id", "twice");
        assert_eq!(d.exit_code(), 1);
        assert!(d.into_result().is_err());
        assert_eq!(not_implemented("build").exit_code(), 1);
    }
}
