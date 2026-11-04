//! Developer CLI toolkit (`scaffold`, `validate`, `lint`, `pack`, `modset`) for DeCombat Lua mods.
//!
//! The `map` scaffold profile (MAP-5) writes a `shared` mod that registers one community map; `validate` checks
//! that every registered map's world file exists under the mod's `assets/` and parses as a version-85 world
//! (`dc_world`), with or without a ROM at hand.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use dc_mod_lua::{is_valid_mod_id, resolve_manifests, JsonValue, ModKind, ModManifest, SemVer};

mod lint;
mod stubs;
mod validate;

#[cfg(test)]
mod kind_tests;
#[cfg(test)]
mod roundtrip_tests;

pub use lint::lint_mod;
pub use validate::validate_mod;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaffoldProfile {
    Content,
    Overhaul,
    TotalConversion,
    /// A client-only cosmetic mod (HUD skin, crosshair).
    ClientVisual,
    /// A community map: a `shared` mod registering one map record whose world file dc-mapkit builds (MAP-5).
    Map,
}

impl ScaffoldProfile {
    /// Kind the profile scaffolds unless `--kind` overrides it.
    pub fn default_kind(&self) -> ModKind {
        match self {
            ScaffoldProfile::Content | ScaffoldProfile::Overhaul => ModKind::ServerRules,
            ScaffoldProfile::TotalConversion | ScaffoldProfile::Map => ModKind::Shared,
            ScaffoldProfile::ClientVisual => ModKind::ClientVisual,
        }
    }

    /// Capabilities the profile's template `main.lua` actually uses.
    pub fn capabilities(&self) -> &'static [&'static str] {
        match self {
            ScaffoldProfile::Content => &["content.weapons"],
            ScaffoldProfile::Overhaul => &["content.weapons", "events", "hooks.lobby", "save"],
            ScaffoldProfile::TotalConversion => &[
                "assets",
                "content.gameroom",
                "content.hud",
                "content.weapons",
                "hooks.combat",
            ],
            ScaffoldProfile::ClientVisual => &["assets", "content.hud"],
            ScaffoldProfile::Map => &["assets", "content.maps"],
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim() {
            "content" => Ok(ScaffoldProfile::Content),
            "overhaul" => Ok(ScaffoldProfile::Overhaul),
            "total_conversion" => Ok(ScaffoldProfile::TotalConversion),
            "client_visual" => Ok(ScaffoldProfile::ClientVisual),
            "map" => Ok(ScaffoldProfile::Map),
            other => Err(format!(
                "Unknown scaffold profile '{}': expected content, overhaul, total_conversion, client_visual or map",
                other
            )),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ScaffoldProfile::Content => "content",
            ScaffoldProfile::Overhaul => "overhaul",
            ScaffoldProfile::TotalConversion => "total_conversion",
            ScaffoldProfile::ClientVisual => "client_visual",
            ScaffoldProfile::Map => "map",
        }
    }
}

/// The map key the `map` profile registers for a mod id: lower case, `-` to `_`, at most 32 characters (the
/// mod id rule `^[A-Za-z0-9_-]+$` is wider than the map key rule `a-z 0-9 _`, MAP-4).
pub fn map_key_of_mod_id(mod_id: &str) -> String {
    mod_id
        .chars()
        .map(|c| {
            if c == '-' {
                '_'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .take(dc_mod_lua::MAP_ID_MAX)
        .collect()
}

/// A placeholder version-85 world for the `map` profile: one flat floor quad of our own, so the scaffolded mod
/// validates before dc-mapkit writes the real map over it. Made up here, nothing from the ROM.
// SPEC: FMT_DAT85.md §5 (one model, one polygon, one node), §6 (no objects)
pub fn placeholder_world() -> Vec<u8> {
    use dc_world::synth::{SynthModel, SynthWorld};
    SynthWorld {
        info: b"dc-modkit placeholder floor; build the map with dc-mapkit".to_vec(),
        extent_min: [-2048.0, -256.0, -2048.0],
        extent_max: [2048.0, 1024.0, 2048.0],
        tree_shape: vec![false],
        models: vec![SynthModel {
            name: b"level".to_vec(),
            textures: vec![b"placeholder".to_vec()],
            planes: vec![([0.0, 1.0, 0.0], 0.0)],
            // flags 1 = solid; surface id 0
            surfaces: vec![(1, 0, 0)],
            polys: vec![(0, 0, vec![0, 1, 2, 3])],
            nodes: vec![(0, 0, -2, -1)],
            points: vec![
                [-2048.0, 0.0, -2048.0],
                [-2048.0, 0.0, 2048.0],
                [2048.0, 0.0, 2048.0],
                [2048.0, 0.0, -2048.0],
            ],
            root: 0,
            box_min: [-2048.0, 0.0, -2048.0],
            box_max: [2048.0, 0.0, 2048.0],
            ..SynthModel::default()
        }],
        ..SynthWorld::default()
    }
    .build()
}

/// Errors fail `lint` and block `pack`; warnings are advisory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintDiagnostic {
    pub file: String,
    pub line: usize,
    pub message: String,
    pub severity: LintSeverity,
}

impl fmt::Display for LintDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.severity {
            LintSeverity::Error => write!(f, "{}:{}: {}", self.file, self.line, self.message),
            LintSeverity::Warning => {
                write!(f, "{}:{}: warning: {}", self.file, self.line, self.message)
            }
        }
    }
}

impl LintDiagnostic {
    pub fn error(file: &str, line: usize, message: String) -> Self {
        Self {
            file: file.to_string(),
            line,
            message,
            severity: LintSeverity::Error,
        }
    }

    pub fn warning(file: &str, line: usize, message: String) -> Self {
        Self {
            severity: LintSeverity::Warning,
            ..Self::error(file, line, message)
        }
    }

    pub fn is_error(&self) -> bool {
        self.severity == LintSeverity::Error
    }

    pub fn to_json_value(&self) -> JsonValue {
        let mut obj = BTreeMap::new();
        obj.insert("file".to_string(), JsonValue::String(self.file.clone()));
        obj.insert("line".to_string(), JsonValue::Number(self.line as f64));
        obj.insert(
            "message".to_string(),
            JsonValue::String(self.message.clone()),
        );
        obj.insert(
            "severity".to_string(),
            JsonValue::String(
                match self.severity {
                    LintSeverity::Error => "error",
                    LintSeverity::Warning => "warning",
                }
                .to_string(),
            ),
        );
        JsonValue::Object(obj)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    pub ok: bool,
    pub mod_id: Option<String>,
    pub errors: Vec<String>,
    /// Advisory findings (ROM cross-checks that may be intentional).
    pub warnings: Vec<String>,
}

impl ValidationReport {
    pub fn to_json_value(&self) -> JsonValue {
        let mut obj = BTreeMap::new();
        obj.insert("ok".to_string(), JsonValue::Bool(self.ok));
        obj.insert(
            "mod_id".to_string(),
            match &self.mod_id {
                Some(id) => JsonValue::String(id.clone()),
                None => JsonValue::Null,
            },
        );
        obj.insert(
            "errors".to_string(),
            JsonValue::Array(
                self.errors
                    .iter()
                    .map(|e| JsonValue::String(e.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "warnings".to_string(),
            JsonValue::Array(
                self.warnings
                    .iter()
                    .map(|e| JsonValue::String(e.clone()))
                    .collect(),
            ),
        );
        JsonValue::Object(obj)
    }
}

// ============================================================================
// 1. `scaffold`
// ============================================================================

/// A 1x1 grey PNG the scaffold writes as `screenshots/main.png`; the author replaces it. Built from the PNG
/// rules (signature, IHDR, one IDAT holding a zlib stream with a single stored block, IEND), each chunk with
/// its CRC-32 over type and data.
pub fn placeholder_png() -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut typed = kind.to_vec();
        typed.extend_from_slice(data);
        out.extend_from_slice(&typed);
        out.extend_from_slice(&crc32(&typed).to_be_bytes());
    }
    // one scanline: filter byte 0, then RGB
    let raw = [0u8, 128, 128, 128];
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    let mut z = vec![0x78, 0x01, 0x01];
    z.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    z.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    z.extend_from_slice(&raw);
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&1u32.to_be_bytes());
    ihdr.extend_from_slice(&1u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB, deflate, no filter set, no interlace
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// The scaffold's placeholder description; `lint` warns while it is still there.
pub const PLACEHOLDER_DESCRIPTION: &str =
    "Replace this text: say in 20 to 500 characters what the mod changes for players.";
/// The scaffold's placeholder author.
pub const PLACEHOLDER_AUTHOR: &str = "your-name";

pub fn scaffold_mod(
    mod_id: &str,
    base_dir: &Path,
    profile: ScaffoldProfile,
    github: Option<&str>,
) -> Result<PathBuf, String> {
    scaffold_mod_kind(mod_id, base_dir, profile, None, github)
}

/// Like `scaffold_mod`; `kind` overrides the profile's default kind. The written manifest must pass
/// the same consistency checks the loader applies, so a kind that cannot hold the profile's
/// capabilities (for example `client_visual` with the `overhaul` profile) is refused.
pub fn scaffold_mod_kind(
    mod_id: &str,
    base_dir: &Path,
    profile: ScaffoldProfile,
    kind: Option<ModKind>,
    github: Option<&str>,
) -> Result<PathBuf, String> {
    if !is_valid_mod_id(mod_id) {
        return Err(format!(
            "Invalid mod id '{}': must be non-empty and match ^[A-Za-z0-9_-]+$",
            mod_id
        ));
    }

    let mut manifest = ModManifest::minimal(
        mod_id,
        mod_id,
        SemVer::new(0, 1, 0),
        kind.unwrap_or_else(|| profile.default_kind()),
    );
    manifest.capabilities = profile
        .capabilities()
        .iter()
        .map(|c| c.to_string())
        .collect();
    manifest.description = PLACEHOLDER_DESCRIPTION.to_string();
    manifest.author = PLACEHOLDER_AUTHOR.to_string();
    manifest.screenshots = vec!["screenshots/main.png".to_string()];
    // DC-DEFAULT: MAP-5 (proposed): a map mod is `shared`, the client must download the world
    if profile == ScaffoldProfile::Map && manifest.kind == ModKind::ServerRules {
        return Err(
            "Cannot scaffold profile 'map' as kind 'server_rules': the client must download the \
             world, so a map mod is kind 'shared'"
                .to_string(),
        );
    }
    manifest.check_consistency().map_err(|e| {
        format!(
            "Cannot scaffold profile '{}' as this kind: {}",
            profile.as_str(),
            e
        )
    })?;

    let mod_dir = base_dir.join(mod_id);
    fs::create_dir_all(mod_dir.join("assets"))
        .map_err(|e| format!("Failed to create assets directory: {}", e))?;
    fs::create_dir_all(mod_dir.join("overrides"))
        .map_err(|e| format!("Failed to create overrides directory: {}", e))?;
    fs::create_dir_all(mod_dir.join("screenshots"))
        .map_err(|e| format!("Failed to create screenshots directory: {}", e))?;
    fs::write(
        mod_dir.join("screenshots").join("main.png"),
        placeholder_png(),
    )
    .map_err(|e| format!("Failed to write screenshots/main.png: {}", e))?;

    let human_name = mod_id
        .split(&['_', '-'][..])
        .filter(|s| !s.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    manifest.name = if human_name.is_empty() {
        mod_id.to_string()
    } else {
        human_name.clone()
    };
    manifest.github = github.map(|s| s.to_string());

    fs::write(
        mod_dir.join("manifest.json"),
        format!("{}\n", manifest.to_json_pretty()),
    )
    .map_err(|e| format!("Failed to write manifest.json: {}", e))?;

    let main_lua = generate_main_lua_template(mod_id, profile);
    fs::write(mod_dir.join("main.lua"), main_lua)
        .map_err(|e| format!("Failed to write main.lua: {}", e))?;

    if profile == ScaffoldProfile::Map {
        let maps = mod_dir.join("assets").join("maps");
        fs::create_dir_all(&maps)
            .map_err(|e| format!("Failed to create assets/maps directory: {}", e))?;
        let world = maps.join(format!("{}.dat", map_key_of_mod_id(mod_id)));
        fs::write(&world, placeholder_world())
            .map_err(|e| format!("Failed to write {}: {}", world.display(), e))?;
    }

    let readme = generate_readme(mod_id, &manifest.name, profile);
    fs::write(mod_dir.join("README.md"), readme)
        .map_err(|e| format!("Failed to write README.md: {}", e))?;

    let luarc = stubs::generate_luarc_json();
    fs::write(mod_dir.join(".luarc.json"), luarc)
        .map_err(|e| format!("Failed to write .luarc.json: {}", e))?;

    let d_lua = stubs::generate_ca_mod_api_d_lua();
    fs::write(mod_dir.join("dc_mod_api.d.lua"), d_lua)
        .map_err(|e| format!("Failed to write dc_mod_api.d.lua: {}", e))?;

    Ok(mod_dir)
}

fn generate_main_lua_template(mod_id: &str, profile: ScaffoldProfile) -> String {
    match profile {
        ScaffoldProfile::Map => {
            let key = map_key_of_mod_id(mod_id);
            format!(
                r#"-- {mod_id} (profile: map)
-- A community map (MAPKIT_PLAN.md section 5.4). The world under assets/maps/ is a build output of dc-mapkit
-- (`dc-mapkit export` writes this mod folder); the scaffold ships a flat placeholder floor so that
-- `dc-modkit validate` passes before the first build.
---@param mod ModCapability
return function(mod)
    mod.content.maps:register({{
        id = "{key}",                          -- the map key: a-z 0-9 _ (1..32)
        name = "{key}",                        -- label shown in the room list (1..48)
        description = "A community map. Say what it is (at most 400 characters).",
        world = mod.assets:path("assets/maps/{key}.dat"),
        modes = {{ "TDM", "FFA", "ELIM", "BOMB", "FLAG" }},
        goals = {{ TDM = 100, FFA = 50 }},      -- optional default goals per mode
        round_seconds = 600,                   -- optional, else the lobby's round clock policy
        players = {{ min = 2, max = 16 }},      -- optional
        -- optional pictures (DTX under assets/, dc-mapkit export writes them):
        -- picture = mod.assets:path("assets/pictures/{key}.dtx"),
        -- loading = mod.assets:path("assets/pictures/{key}_load.dtx"),
        -- radar = {{ picture = mod.assets:path("assets/pictures/{key}_radar.dtx"),
        --           min = {{ -2048, -2048 }}, max = {{ 2048, 2048 }} }},
    }})
end
"#
            )
        }
        ScaffoldProfile::ClientVisual => format!(
            r#"-- {mod_id} (profile: client_visual)
---@param mod ModCapability
return function(mod)
    -- Cosmetic only: point a HUD slot at an original asset shipped in assets/.
    mod.content.hud:override("crosshair", mod.assets:path("assets/crosshair.dtx"))
end
"#
        ),
        ScaffoldProfile::Content => format!(
            r#"-- {mod_id} (profile: content)
---@param mod ModCapability
return function(mod)
    mod.options:define({{
        enabled = {{ type = "boolean", default = true }},
        damage_bonus = {{ type = "number", default = 4 }},
    }})

    if not mod.options:get("enabled") then
        return
    end

    local bonus = mod.options:get("damage_bonus") or 0
    -- ROM keys: the weapon row (ShotsPerClip, Range ...) and the ammo row (InstDamage ...)
    mod.content.weapons:patch("Ammo145", {{
        InstDamage = 35 + bonus,
    }})
end
"#
        ),
        ScaffoldProfile::Overhaul => format!(
            r#"-- {mod_id} (profile: overhaul)
---@param mod ModCapability
return function(mod)
    mod.options:define({{
        exp_multiplier = {{ type = "number", default = 1.5 }},
        gp_multiplier = {{ type = "number", default = 1.5 }},
    }})

    mod.content.weapons:patch(145, {{
        ShotsPerClip = 40,
    }})
    mod.content.weapons:patch("Ammo145", {{
        InstDamage = 42,
    }})

    mod.hooks:wrap("lobby.calc_exp_gp", function(base, ctx)
        local result = base(ctx)
        if type(result) == "table" then
            local exp_mult = mod.options:get("exp_multiplier") or 1.0
            local gp_mult = mod.options:get("gp_multiplier") or 1.0
            if result.exp then
                result.exp = math.floor(result.exp * exp_mult)
            end
            if result.gp then
                result.gp = math.floor(result.gp * gp_mult)
            end
        end
        return result
    end)

    mod.events:on("match.end", function(ctx)
        local matches = (mod.save:get("matches_played") or 0) + 1
        mod.save:set("matches_played", matches)
    end)
end
"#
        ),
        ScaffoldProfile::TotalConversion => format!(
            r#"-- {mod_id} (profile: total_conversion)
---@param mod ModCapability
return function(mod)
    mod.options:define({{
        hardcore_mode = {{ type = "boolean", default = true }},
    }})

    local marker_asset = mod.assets:path("assets/hitmarker.dtx")
    mod.content.hud:override("hitmarker", marker_asset)
    mod.content.gameroom:override("max_players", 16)

    mod.content.weapons:register({{
        id = 10001,
        Name = "TC Pulse Rifle",
        ShotsPerClip = 55,
    }})

    mod.hooks:wrap("combat.damage", function(base, ctx)
        local dmg = base(ctx)
        if mod.options:get("hardcore_mode") and type(dmg) == "number" then
            return dmg * 1.25
        end
        return dmg
    end)

    mod.exports.is_total_conversion = function()
        return true
    end
end
"#
        ),
    }
}

fn generate_readme(mod_id: &str, name: &str, profile: ScaffoldProfile) -> String {
    let map_note = if profile == ScaffoldProfile::Map {
        format!(
            "\nThis is a map mod (MODDING_GUIDE.md \"Map mods\"): `main.lua` registers the map `{key}` and \
             `assets/maps/{key}.dat` is its world. The scaffolded world is a flat placeholder floor; build the \
             real one with dc-mapkit (`dc-mapkit export <project>` writes this folder) or copy your own \
             version-85 world over it. `dc-modkit validate` checks that the world exists and parses.\n",
            key = map_key_of_mod_id(mod_id)
        )
    } else {
        String::new()
    };
    format!(
        r#"# {name} (`{mod_id}`)

Profile: `{profile}`

Kind and capabilities are declared in `manifest.json`; see docs/MODDING_GUIDE.md.
{map_note}
Before you publish: replace the placeholder `description` and `author` in `manifest.json` and the placeholder image
`screenshots/main.png` (PNG or JPG, at most 1 MiB each, up to 6 under `screenshots/`). The server browser shows them;
kinds `shared` and `client_visual` need at least one screenshot.

## Development Commands

```sh
dc-modkit validate .
dc-modkit lint .
dc-modkit pack . -o {mod_id}.zip
```
"#,
        name = name,
        mod_id = mod_id,
        profile = profile.as_str(),
        map_note = map_note
    )
}

// `validate` lives in validate.rs, `lint` in lint.rs, editor stubs in stubs.rs.

// ============================================================================
// 4. `pack` & Deterministic Pure-Rust ZIP `STORE` Writer
// ============================================================================

/// Computes standard IEEE 802.3 CRC-32 checksum.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

struct ZipEntryRecord {
    name: String,
    crc: u32,
    size: u32,
    local_header_offset: u32,
}

/// Writes a deterministic ZIP archive (compression method 0 = `STORE`, fixed DOS timestamp `1980-01-01 00:00:00`).
pub fn write_deterministic_zip(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    const DOS_TIME: u16 = 0x0000; // 00:00:00
    const DOS_DATE: u16 = 0x0021; // 1980-01-01

    // Classic (non-ZIP64) limits: 16-bit counts/name lengths, 32-bit sizes/offsets. Never truncate silently.
    let count = u16::try_from(entries.len()).map_err(|_| {
        format!(
            "zip: {} entries exceeds the 65535 entry limit",
            entries.len()
        )
    })?;
    let mut sorted: Vec<&(String, Vec<u8>)> = entries.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    let mut buf = Vec::new();
    let mut records = Vec::with_capacity(sorted.len());

    // 1. Local file headers + file data
    for (name, data) in sorted {
        let local_header_offset = zip_u32(buf.len(), "local header offset")?;
        let crc = crc32(data);
        let size = zip_u32(data.len(), &format!("size of {}", name))?;
        let name_bytes = name.as_bytes();
        let name_len = u16::try_from(name_bytes.len()).map_err(|_| {
            format!(
                "zip: entry name of {} bytes exceeds 65535",
                name_bytes.len()
            )
        })?;

        // Local file header signature: 0x04034b50
        buf.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        buf.extend_from_slice(&20u16.to_le_bytes()); // version needed to extract (2.0)
        buf.extend_from_slice(&0u16.to_le_bytes()); // general purpose bit flag
        buf.extend_from_slice(&0u16.to_le_bytes()); // compression method (0 = STORE)
        buf.extend_from_slice(&DOS_TIME.to_le_bytes());
        buf.extend_from_slice(&DOS_DATE.to_le_bytes());
        buf.extend_from_slice(&crc.to_le_bytes());
        buf.extend_from_slice(&size.to_le_bytes()); // compressed size
        buf.extend_from_slice(&size.to_le_bytes()); // uncompressed size
        buf.extend_from_slice(&name_len.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        buf.extend_from_slice(name_bytes);
        buf.extend_from_slice(data);

        records.push(ZipEntryRecord {
            name: name.clone(),
            crc,
            size,
            local_header_offset,
        });
    }

    // 2. Central directory headers
    let cd_offset = zip_u32(buf.len(), "central directory offset")?;
    for rec in &records {
        let name_bytes = rec.name.as_bytes();
        let name_len = name_bytes.len() as u16; // length checked in pass 1

        // Central directory file header signature: 0x02014b50
        buf.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        buf.extend_from_slice(&20u16.to_le_bytes()); // version made by
        buf.extend_from_slice(&20u16.to_le_bytes()); // version needed to extract
        buf.extend_from_slice(&0u16.to_le_bytes()); // general purpose bit flag
        buf.extend_from_slice(&0u16.to_le_bytes()); // compression method (0 = STORE)
        buf.extend_from_slice(&DOS_TIME.to_le_bytes());
        buf.extend_from_slice(&DOS_DATE.to_le_bytes());
        buf.extend_from_slice(&rec.crc.to_le_bytes());
        buf.extend_from_slice(&rec.size.to_le_bytes());
        buf.extend_from_slice(&rec.size.to_le_bytes());
        buf.extend_from_slice(&name_len.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        buf.extend_from_slice(&0u16.to_le_bytes()); // file comment length
        buf.extend_from_slice(&0u16.to_le_bytes()); // disk number start
        buf.extend_from_slice(&0u16.to_le_bytes()); // internal file attributes
        buf.extend_from_slice(&0u32.to_le_bytes()); // external file attributes
        buf.extend_from_slice(&rec.local_header_offset.to_le_bytes());
        buf.extend_from_slice(name_bytes);
    }
    let cd_size = zip_u32(buf.len(), "central directory end")? - cd_offset;

    // 3. End of central directory record (0x06054b50)
    buf.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes()); // number of this disk
    buf.extend_from_slice(&0u16.to_le_bytes()); // disk where central directory starts
    buf.extend_from_slice(&count.to_le_bytes()); // number of central directory records on this disk
    buf.extend_from_slice(&count.to_le_bytes()); // total number of central directory records
    buf.extend_from_slice(&cd_size.to_le_bytes());
    buf.extend_from_slice(&cd_offset.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes()); // comment length

    zip_u32(buf.len(), "archive size")?;
    Ok(buf)
}

/// Most entries and total unpacked bytes `read_stored_zip` accepts (the same caps as the client's package reader).
const READ_ZIP_MAX_ENTRIES: usize = 4096;
const READ_ZIP_MAX_BYTES: usize = 32 * 1024 * 1024;

/// Reads a `STORE`-only ZIP archive (what `write_deterministic_zip` writes) through its central directory.
/// Refuses compressed entries, CRC mismatches and names that are absolute or climb out (`..`, `\`, `:`).
/// Directory entries (names ending in `/`) are skipped.
pub fn read_stored_zip(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let u16_at = |o: usize| -> Result<u16, String> {
        bytes
            .get(o..o + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .ok_or_else(|| "zip: truncated archive".to_string())
    };
    let u32_at = |o: usize| -> Result<u32, String> {
        bytes
            .get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| "zip: truncated archive".to_string())
    };
    if bytes.len() < 22 {
        return Err("zip: too short for an end of central directory record".to_string());
    }
    // the end record sits in the last 22 + 65535 bytes (the comment may follow it)
    let lowest = bytes.len().saturating_sub(22 + 0xffff);
    let mut eocd = None;
    let mut i = bytes.len() - 22;
    loop {
        if u32_at(i)? == 0x0605_4b50 {
            eocd = Some(i);
            break;
        }
        if i == lowest {
            break;
        }
        i -= 1;
    }
    let eocd = eocd.ok_or_else(|| "zip: no end of central directory record".to_string())?;
    let count = u16_at(eocd + 10)? as usize;
    let mut at = u32_at(eocd + 16)? as usize;
    if count > READ_ZIP_MAX_ENTRIES {
        return Err(format!(
            "zip: {count} entries (at most {READ_ZIP_MAX_ENTRIES})"
        ));
    }
    let mut total = 0usize;
    let mut out: Vec<(String, Vec<u8>)> = Vec::with_capacity(count);
    for _ in 0..count {
        if u32_at(at)? != 0x0201_4b50 {
            return Err("zip: bad central directory record".to_string());
        }
        let method = u16_at(at + 10)?;
        let crc = u32_at(at + 16)?;
        let csize = u32_at(at + 20)? as usize;
        let usize_ = u32_at(at + 24)? as usize;
        let name_len = u16_at(at + 28)? as usize;
        let extra_len = u16_at(at + 30)? as usize;
        let comment_len = u16_at(at + 32)? as usize;
        let local = u32_at(at + 42)? as usize;
        let name_end = (at + 46)
            .checked_add(name_len)
            .ok_or_else(|| "zip: damaged archive".to_string())?;
        let name = bytes
            .get(at + 46..name_end)
            .ok_or_else(|| "zip: truncated name".to_string())?;
        let name = String::from_utf8(name.to_vec())
            .map_err(|_| "zip: an entry name is not UTF-8".to_string())?;
        at = name_end
            .checked_add(extra_len)
            .and_then(|x| x.checked_add(comment_len))
            .ok_or_else(|| "zip: damaged archive".to_string())?;
        if name.ends_with('/') {
            continue;
        }
        if name.is_empty()
            || name.starts_with('/')
            || name.contains('\\')
            || name.contains(':')
            || name.chars().any(|c| c.is_control())
            || name.split('/').any(|c| c == ".." || c.is_empty())
        {
            return Err(format!(
                "zip: entry name '{}' is not a plain relative path",
                name
            ));
        }
        if out.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name)) {
            return Err(format!("zip: entry '{}' appears twice", name));
        }
        total = total.saturating_add(usize_);
        if total > READ_ZIP_MAX_BYTES {
            return Err(format!(
                "zip: the archive unpacks to more than {READ_ZIP_MAX_BYTES} bytes"
            ));
        }
        if method != 0 || csize != usize_ {
            return Err(format!(
                "zip: entry '{}' is compressed (method {}); only STORE archives from `dc-modkit pack` are read",
                name, method
            ));
        }
        if u32_at(local)? != 0x0403_4b50 {
            return Err(format!("zip: entry '{}' has no local header", name));
        }
        let data_at = local
            .checked_add(30 + u16_at(local + 26)? as usize + u16_at(local + 28)? as usize)
            .ok_or_else(|| "zip: damaged archive".to_string())?;
        let data_end = data_at
            .checked_add(csize)
            .ok_or_else(|| "zip: damaged archive".to_string())?;
        let data = bytes
            .get(data_at..data_end)
            .ok_or_else(|| format!("zip: entry '{}' is truncated", name))?;
        if crc32(data) != crc {
            return Err(format!("zip: entry '{}' fails its CRC", name));
        }
        out.push((name, data.to_vec()));
    }
    Ok(out)
}

fn zip_u32(v: usize, what: &str) -> Result<u32, String> {
    u32::try_from(v).map_err(|_| format!("zip: {} ({} bytes) exceeds the 4 GiB limit", what, v))
}

/// One server mod-set entry (HOSTING_AND_MODS.md section 4): the id, version and SHA-256 of the packed
/// zip, plus the manifest facts a client shows before it trusts an unverified mod.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModSetEntry {
    pub id: String,
    pub version: SemVer,
    /// Lower-case hex SHA-256 of the deterministic zip.
    pub sha256: String,
    pub kind: ModKind,
    pub requires_client: bool,
    pub capabilities: Vec<String>,
    /// `owner/repo` from the manifest's `github` field, when given.
    pub source_repo: Option<String>,
    /// Browser metadata from the manifest. Screenshots are file names inside the zip, not the bytes.
    pub description: String,
    pub author: String,
    pub tags: Vec<String>,
    pub screenshots: Vec<String>,
    /// MOD-27: the manifest's command help.
    pub commands: Vec<dc_mod_lua::ModCommand>,
}

impl ModSetEntry {
    pub fn new(manifest: &ModManifest, zip_bytes: &[u8]) -> Self {
        Self {
            id: manifest.id.clone(),
            version: manifest.version,
            sha256: dc_rom::sha256_hex(zip_bytes),
            kind: manifest.kind,
            requires_client: manifest.requires_client,
            capabilities: manifest.capabilities.clone(),
            source_repo: manifest.github.clone(),
            description: manifest.description.clone(),
            author: manifest.author.clone(),
            tags: manifest.tags.clone(),
            screenshots: manifest.screenshots.clone(),
            commands: manifest.commands.clone(),
        }
    }

    pub fn to_json_value(&self) -> JsonValue {
        let mut obj = BTreeMap::new();
        obj.insert("id".to_string(), JsonValue::String(self.id.clone()));
        obj.insert(
            "version".to_string(),
            JsonValue::String(self.version.to_string()),
        );
        obj.insert("sha256".to_string(), JsonValue::String(self.sha256.clone()));
        obj.insert(
            "kind".to_string(),
            JsonValue::String(self.kind.as_str().to_string()),
        );
        obj.insert(
            "requires_client".to_string(),
            JsonValue::Bool(self.requires_client),
        );
        obj.insert(
            "capabilities".to_string(),
            JsonValue::Array(
                self.capabilities
                    .iter()
                    .map(|c| JsonValue::String(c.clone()))
                    .collect(),
            ),
        );
        if let Some(repo) = &self.source_repo {
            obj.insert("source_repo".to_string(), JsonValue::String(repo.clone()));
        }
        obj.insert(
            "description".to_string(),
            JsonValue::String(self.description.clone()),
        );
        obj.insert("author".to_string(), JsonValue::String(self.author.clone()));
        let strings =
            |v: &[String]| JsonValue::Array(v.iter().cloned().map(JsonValue::String).collect());
        obj.insert("tags".to_string(), strings(&self.tags));
        obj.insert("screenshots".to_string(), strings(&self.screenshots));
        if !self.commands.is_empty() {
            obj.insert(
                "commands".to_string(),
                JsonValue::Array(self.commands.iter().map(|c| c.to_json()).collect()),
            );
        }
        JsonValue::Object(obj)
    }
}

/// Validates, lints and packs a mod into deterministic zip bytes without writing anything.
pub fn pack_mod_bytes(mod_path: &Path) -> Result<(Vec<u8>, ModSetEntry), Vec<String>> {
    let (bytes, entry) = pack_inner(mod_path, None)?;
    Ok((bytes, entry))
}

/// Packs a mod to `out_zip` and returns the zip bytes with the mod-set entry that describes them.
pub fn pack_mod_entry(
    mod_path: &Path,
    out_zip: &Path,
) -> Result<(Vec<u8>, ModSetEntry), Vec<String>> {
    pack_inner(mod_path, Some(out_zip))
}

pub fn pack_mod(mod_path: &Path, out_zip: &Path) -> Result<Vec<u8>, Vec<String>> {
    pack_mod_entry(mod_path, out_zip).map(|(bytes, _)| bytes)
}

/// Builds the `mod_set` fragment of a server-list entry from mod directories, in the deterministic
/// load order (dependencies first, then priority, then id). Refuses unpackable mods and unresolved
/// dependencies. An empty list gives the vanilla fragment.
pub fn modset_fragment(
    mod_dirs: &[PathBuf],
    client_visual_allowed: bool,
) -> Result<JsonValue, Vec<String>> {
    let mut entries: Vec<(ModManifest, ModSetEntry)> = Vec::new();
    let mut errors = Vec::new();
    for dir in mod_dirs {
        match pack_mod_bytes(dir) {
            Ok((_, entry)) => {
                let manifest = match ModManifest::from_file(&dir.join("manifest.json")) {
                    Ok(m) => m,
                    Err(e) => {
                        errors.push(e);
                        continue;
                    }
                };
                if entries.iter().any(|(m, _)| m.id == manifest.id) {
                    errors.push(format!(
                        "Mod id '{}' appears twice in the mod set",
                        manifest.id
                    ));
                    continue;
                }
                entries.push((manifest, entry));
            }
            Err(mut errs) => {
                for e in errs.iter_mut() {
                    *e = format!("{}: {}", dir.display(), e);
                }
                errors.extend(errs);
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let manifests: Vec<ModManifest> = entries.iter().map(|(m, _)| m.clone()).collect();
    let ordered = resolve_manifests(&manifests).map_err(|e| vec![e.to_string()])?;

    let mut mods = Vec::with_capacity(ordered.len());
    for m in &ordered {
        let (_, entry) = entries
            .iter()
            .find(|(em, _)| em.id == m.id)
            .expect("resolver returns only the manifests it was given");
        mods.push(entry.to_json_value());
    }

    let mut obj = BTreeMap::new();
    obj.insert(
        "policy".to_string(),
        JsonValue::String(if mods.is_empty() { "vanilla" } else { "modded" }.to_string()),
    );
    obj.insert(
        "client_visual_allowed".to_string(),
        JsonValue::Bool(!mods.is_empty() && client_visual_allowed),
    );
    obj.insert("mods".to_string(), JsonValue::Array(mods));
    Ok(JsonValue::Object(obj))
}

fn pack_inner(
    mod_path: &Path,
    out_zip: Option<&Path>,
) -> Result<(Vec<u8>, ModSetEntry), Vec<String>> {
    let val_report = validate_mod(mod_path, None);
    let lint_diags = match lint_mod(mod_path) {
        Ok(d) => d,
        Err(e) => return Err(vec![e]),
    };

    if !val_report.ok || lint_diags.iter().any(|d| d.is_error()) {
        let mut errs = val_report.errors;
        for d in lint_diags.iter().filter(|d| d.is_error()) {
            let line = d.to_string();
            if !errs.contains(&line) {
                errs.push(line);
            }
        }
        return Err(errs);
    }

    let manifest = ModManifest::from_file(&mod_path.join("manifest.json")).map_err(|e| vec![e])?;
    let out_canonical = out_zip.and_then(|z| fs::canonicalize(z).ok());
    let mut files = Vec::new();
    collect_all_mod_files(mod_path, mod_path, out_canonical.as_deref(), &mut files)
        .map_err(|e| vec![e])?;

    let zip_bytes = write_deterministic_zip(&files).map_err(|e| vec![e])?;
    let entry = ModSetEntry::new(&manifest, &zip_bytes);
    let Some(out_zip) = out_zip else {
        return Ok((zip_bytes, entry));
    };
    if let Some(parent) = out_zip.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| {
                vec![format!(
                    "Failed to create parent directory '{}': {}",
                    parent.display(),
                    e
                )]
            })?;
        }
    }
    fs::write(out_zip, &zip_bytes)
        .map_err(|e| vec![format!("Failed to write '{}': {}", out_zip.display(), e)])?;
    Ok((zip_bytes, entry))
}

fn collect_all_mod_files(
    root: &Path,
    cur: &Path,
    exclude_zip: Option<&Path>,
    out: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), String> {
    let entries = fs::read_dir(cur)
        .map_err(|e| format!("Failed to read directory '{}': {}", cur.display(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if let (Some(ex), Ok(canon)) = (exclude_zip, fs::canonicalize(&path)) {
            if ex == canon {
                continue;
            }
        }
        if path.is_dir() {
            collect_all_mod_files(root, &path, exclude_zip, out)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let data = fs::read(&path)
                .map_err(|e| format!("Failed to read '{}': {}", path.display(), e))?;
            out.push((rel, data));
        }
    }
    Ok(())
}

// ============================================================================
// CLI Entry Dispatcher
// ============================================================================

pub fn run_cli(args: &[String]) -> i32 {
    let Some(subcmd) = args.get(1).map(|s| s.as_str()) else {
        print_usage();
        return 1;
    };

    match subcmd {
        "scaffold" => cmd_scaffold(&args[2..]),
        "validate" => cmd_validate(&args[2..]),
        "lint" => cmd_lint(&args[2..]),
        "pack" => cmd_pack(&args[2..]),
        "modset" => cmd_modset(&args[2..]),
        "rom" => cmd_rom(&args[2..]),
        "--help" | "-h" | "help" => {
            print_usage();
            0
        }
        other => {
            eprintln!("Unknown subcommand '{}'", other);
            print_usage();
            1
        }
    }
}

fn print_usage() {
    eprintln!(
        "Usage:
  dc-modkit scaffold <mod_id> [--dir <path>] [--profile content|overhaul|total_conversion|client_visual|map] [--kind server_rules|shared|client_visual] [--github <owner/repo>]
  dc-modkit validate <mod_path> [--rom <CA_ROM>] [--json] [--quiet]   (a map mod: every registered world must exist under assets/ and parse)
  dc-modkit lint <mod_path> [--json] [--quiet]
  dc-modkit pack <mod_path> -o <out.zip> [--json] [--quiet]
  dc-modkit modset [<mod_path>...] [--client-visual-allowed]   (prints the server mod_set JSON fragment)
  dc-modkit rom ls [--hist] [--rez NAME]   (reads CA_ROM, read-only)
  dc-modkit rom worlds"
    );
}

/// All `.rez` files under `rom` (recursive), sorted by path.
pub fn find_rez_files(rom: &Path) -> Result<Vec<PathBuf>, String> {
    fn rec(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
        let rd = fs::read_dir(dir).map_err(|e| format!("read_dir {}: {}", dir.display(), e))?;
        for ent in rd {
            let p = ent.map_err(|e| e.to_string())?.path();
            if p.is_dir() {
                rec(&p, out)?;
            } else if p
                .extension()
                .map(|x| x.eq_ignore_ascii_case("rez"))
                .unwrap_or(false)
            {
                out.push(p);
            }
        }
        Ok(())
    }
    let mut v = Vec::new();
    rec(rom, &mut v)?;
    v.sort();
    Ok(v)
}

/// Per-extension (uppercase) `(count, bytes)` histogram over entry lists.
pub fn extension_histogram(entries: &[(String, u64, u32)]) -> BTreeMap<String, (u64, u64)> {
    let mut h: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for (name, _, size) in entries {
        let ext = match name.rsplit_once('.') {
            Some((_, e)) => e.to_ascii_uppercase(),
            None => String::new(),
        };
        let e = h.entry(ext).or_insert((0, 0));
        e.0 += 1;
        e.1 += *size as u64;
    }
    h
}

/// Whole-ROM totals: `(rez_count, total_rez_file_bytes, histogram)`; histogram bytes are entry payload bytes.
pub fn rom_histogram(rom: &Path) -> Result<(usize, u64, BTreeMap<String, (u64, u64)>), String> {
    let rezs = find_rez_files(rom)?;
    let mut all = Vec::new();
    for r in &rezs {
        all.extend(dc_rez::walk_rez(r).map_err(|e| e.to_string())?);
    }
    let h = extension_histogram(&all);
    let mut total = 0u64;
    for r in &rezs {
        total += fs::metadata(r).map_err(|e| e.to_string())?.len();
    }
    Ok((rezs.len(), total, h))
}

fn cmd_rom(args: &[String]) -> i32 {
    match cmd_rom_inner(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {}", e);
            1
        }
    }
}

fn cmd_rom_inner(args: &[String]) -> Result<(), String> {
    let rom = std::env::var_os("CA_ROM")
        .map(PathBuf::from)
        .ok_or_else(|| "CA_ROM is not set".to_string())?;
    match args.first().map(|s| s.as_str()) {
        Some("ls") => {
            let mut hist = false;
            let mut rez: Option<String> = None;
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--hist" => {
                        hist = true;
                        i += 1;
                    }
                    "--rez" => {
                        rez = Some(args.get(i + 1).ok_or("--rez requires a name")?.clone());
                        i += 2;
                    }
                    o => return Err(format!("unknown option {}", o)),
                }
            }
            let rezs = find_rez_files(&rom)?;
            let selected: Vec<&PathBuf> = match &rez {
                Some(n) => {
                    let want = n.to_ascii_lowercase();
                    let want = want.strip_suffix(".rez").unwrap_or(&want).to_string();
                    let v: Vec<&PathBuf> = rezs
                        .iter()
                        .filter(|p| {
                            p.file_stem()
                                .map(|s| s.to_string_lossy().to_ascii_lowercase() == want)
                                .unwrap_or(false)
                        })
                        .collect();
                    if v.is_empty() {
                        return Err(format!("no such rez {}", n));
                    }
                    v
                }
                None => rezs.iter().collect(),
            };
            if hist {
                let mut all = Vec::new();
                for r in &selected {
                    all.extend(dc_rez::walk_rez(r).map_err(|e| e.to_string())?);
                }
                let h = extension_histogram(&all);
                let mut file_bytes = 0u64;
                for r in &selected {
                    file_bytes += fs::metadata(r).map_err(|e| e.to_string())?.len();
                }
                let entry_bytes: u64 = h.values().map(|v| v.1).sum();
                println!("rez: {}", selected.len());
                println!("bytes: {}", file_bytes);
                println!("entry_bytes: {}", entry_bytes);
                let mut rows: Vec<_> = h.iter().collect();
                rows.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(b.0)));
                for (ext, (c, b)) in rows {
                    println!("{}\t{}\t{}", ext, c, b);
                }
            } else {
                for r in &selected {
                    for (name, off, size) in dc_rez::walk_rez(r).map_err(|e| e.to_string())? {
                        println!("{}\t{}\t{}", name, off, size);
                    }
                }
            }
            Ok(())
        }
        Some("worlds") => {
            let mut n = 0;
            for r in find_rez_files(&rom)? {
                for (name, _, size) in dc_rez::walk_rez(&r).map_err(|e| e.to_string())? {
                    if name.to_ascii_uppercase().ends_with(".DAT") {
                        println!("{}\t{}", name, size);
                        n += 1;
                    }
                }
            }
            println!("worlds: {}", n);
            Ok(())
        }
        _ => Err("usage: rom ls [--hist] [--rez NAME] | rom worlds".to_string()),
    }
}

fn cmd_scaffold(args: &[String]) -> i32 {
    let mut mod_id: Option<String> = None;
    let mut dir = PathBuf::from(".");
    let mut profile = ScaffoldProfile::Content;
    let mut github: Option<String> = None;
    let mut kind: Option<ModKind> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--kind" => {
                let Some(val) = args.get(i + 1) else {
                    eprintln!("error: --kind requires server_rules, shared or client_visual");
                    return 1;
                };
                match ModKind::parse(val) {
                    Ok(k) => kind = Some(k),
                    Err(e) => {
                        eprintln!("error: {}", e);
                        return 1;
                    }
                }
                i += 2;
            }
            "--dir" | "-d" => {
                let Some(val) = args.get(i + 1) else {
                    eprintln!("error: --dir requires a path argument");
                    return 1;
                };
                dir = PathBuf::from(val);
                i += 2;
            }
            "--profile" => {
                let Some(val) = args.get(i + 1) else {
                    eprintln!("error: --profile requires a profile name");
                    return 1;
                };
                match ScaffoldProfile::parse(val) {
                    Ok(p) => profile = p,
                    Err(e) => {
                        eprintln!("error: {}", e);
                        return 1;
                    }
                }
                i += 2;
            }
            "--github" => {
                let Some(val) = args.get(i + 1) else {
                    eprintln!("error: --github requires an <owner/repo> argument");
                    return 1;
                };
                github = Some(val.clone());
                i += 2;
            }
            flag if flag.starts_with('-') => {
                eprintln!("error: unknown option '{}'", flag);
                return 1;
            }
            pos => {
                if mod_id.is_none() {
                    mod_id = Some(pos.to_string());
                } else {
                    eprintln!("error: unexpected positional argument '{}'", pos);
                    return 1;
                }
                i += 1;
            }
        }
    }

    let Some(id) = mod_id else {
        eprintln!("error: missing required <mod_id>");
        return 1;
    };

    match scaffold_mod_kind(&id, &dir, profile, kind, github.as_deref()) {
        Ok(created_dir) => {
            println!("Scaffolded mod '{}' at {}", id, created_dir.display());
            0
        }
        Err(e) => {
            eprintln!("error: {}", e);
            1
        }
    }
}

fn cmd_validate(args: &[String]) -> i32 {
    let mut mod_path: Option<PathBuf> = None;
    let mut rom_path: Option<PathBuf> = None;
    let mut json = false;
    let mut quiet = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--rom" => {
                let Some(val) = args.get(i + 1) else {
                    eprintln!("error: --rom requires a path argument");
                    return 1;
                };
                rom_path = Some(PathBuf::from(val));
                i += 2;
            }
            "--json" => {
                json = true;
                i += 1;
            }
            "--quiet" | "-q" => {
                quiet = true;
                i += 1;
            }
            flag if flag.starts_with('-') => {
                eprintln!("error: unknown option '{}'", flag);
                return 1;
            }
            pos => {
                mod_path = Some(PathBuf::from(pos));
                i += 1;
            }
        }
    }

    let Some(path) = mod_path else {
        eprintln!("error: missing required <mod_path>");
        return 1;
    };

    // The ROM is only consulted when asked for: --rom, or CA_ROM when it names a directory.
    if rom_path.is_none() {
        rom_path = dc_rom::resolve_rom().ok();
    }
    let report = validate_mod(&path, rom_path.as_deref());
    if json {
        println!("{}", report.to_json_value().to_json_pretty());
    } else if !report.ok {
        for err in &report.errors {
            eprintln!("error: {}", err);
        }
        for w in &report.warnings {
            eprintln!("warning: {}", w);
        }
    } else if !quiet {
        for w in &report.warnings {
            eprintln!("warning: {}", w);
        }
        println!(
            "Validated mod '{}' successfully.",
            report.mod_id.as_deref().unwrap_or("")
        );
    }

    if report.ok {
        0
    } else {
        1
    }
}

fn cmd_lint(args: &[String]) -> i32 {
    let mut mod_path: Option<PathBuf> = None;
    let mut json = false;
    let mut quiet = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => {
                json = true;
                i += 1;
            }
            "--quiet" | "-q" => {
                quiet = true;
                i += 1;
            }
            flag if flag.starts_with('-') => {
                eprintln!("error: unknown option '{}'", flag);
                return 1;
            }
            pos => {
                mod_path = Some(PathBuf::from(pos));
                i += 1;
            }
        }
    }

    let Some(path) = mod_path else {
        eprintln!("error: missing required <mod_path>");
        return 1;
    };

    let diags = match lint_mod(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {}", e);
            return 1;
        }
    };

    if json {
        let mut obj = BTreeMap::new();
        obj.insert(
            "ok".to_string(),
            JsonValue::Bool(!diags.iter().any(|d| d.is_error())),
        );
        obj.insert(
            "diagnostics".to_string(),
            JsonValue::Array(diags.iter().map(|d| d.to_json_value()).collect()),
        );
        println!("{}", JsonValue::Object(obj).to_json_pretty());
    } else if !diags.is_empty() {
        for d in &diags {
            println!("{}", d);
        }
    } else if !quiet {
        println!("Lint passed with 0 issues.");
    }

    if diags.iter().any(|d| d.is_error()) {
        1
    } else {
        0
    }
}

fn cmd_pack(args: &[String]) -> i32 {
    let mut mod_path: Option<PathBuf> = None;
    let mut out_zip: Option<PathBuf> = None;
    let mut json = false;
    let mut quiet = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                let Some(val) = args.get(i + 1) else {
                    eprintln!("error: -o requires an output .zip path");
                    return 1;
                };
                out_zip = Some(PathBuf::from(val));
                i += 2;
            }
            "--json" => {
                json = true;
                i += 1;
            }
            "--quiet" | "-q" => {
                quiet = true;
                i += 1;
            }
            flag if flag.starts_with('-') => {
                eprintln!("error: unknown option '{}'", flag);
                return 1;
            }
            pos => {
                mod_path = Some(PathBuf::from(pos));
                i += 1;
            }
        }
    }

    let Some(path) = mod_path else {
        eprintln!("error: missing required <mod_path>");
        return 1;
    };
    let Some(out) = out_zip else {
        eprintln!("error: missing required -o <out.zip>");
        return 1;
    };

    match pack_mod_entry(&path, &out) {
        Ok((bytes, entry)) => {
            if json {
                let mut obj = BTreeMap::new();
                obj.insert("ok".to_string(), JsonValue::Bool(true));
                obj.insert("modset_entry".to_string(), entry.to_json_value());
                obj.insert(
                    "sha256".to_string(),
                    JsonValue::String(entry.sha256.clone()),
                );
                obj.insert(
                    "output".to_string(),
                    JsonValue::String(out.display().to_string()),
                );
                obj.insert("size".to_string(), JsonValue::Number(bytes.len() as f64));
                obj.insert("crc32".to_string(), JsonValue::Number(crc32(&bytes) as f64));
                println!("{}", JsonValue::Object(obj).to_json_pretty());
            } else if !quiet {
                println!(
                    "Packed {} ({} bytes) id={} version={} sha256={}",
                    out.display(),
                    bytes.len(),
                    entry.id,
                    entry.version,
                    entry.sha256
                );
            }
            0
        }
        Err(errs) => {
            if json {
                let mut obj = BTreeMap::new();
                obj.insert("ok".to_string(), JsonValue::Bool(false));
                obj.insert(
                    "errors".to_string(),
                    JsonValue::Array(errs.into_iter().map(JsonValue::String).collect()),
                );
                println!("{}", JsonValue::Object(obj).to_json_pretty());
            } else {
                for e in errs {
                    eprintln!("error: {}", e);
                }
            }
            1
        }
    }
}

fn cmd_modset(args: &[String]) -> i32 {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut client_visual_allowed = false;
    for arg in args {
        match arg.as_str() {
            "--client-visual-allowed" => client_visual_allowed = true,
            flag if flag.starts_with('-') => {
                eprintln!("error: unknown option '{}'", flag);
                return 1;
            }
            pos => dirs.push(PathBuf::from(pos)),
        }
    }
    match modset_fragment(&dirs, client_visual_allowed) {
        Ok(v) => {
            println!("{}", v.to_json_pretty());
            0
        }
        Err(errs) => {
            for e in errs {
                eprintln!("error: {}", e);
            }
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ca_modkit_test_{}_{}_{}",
            prefix,
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_end_to_end_scaffold_validate_lint_pack_deterministic_zip() {
        let tmp = make_temp_dir("e2e");

        for profile in [
            ScaffoldProfile::Content,
            ScaffoldProfile::Overhaul,
            ScaffoldProfile::TotalConversion,
            ScaffoldProfile::Map,
        ] {
            let mod_id = format!("sample_{}", profile.as_str());
            let mod_dir =
                scaffold_mod(&mod_id, &tmp, profile, Some("combatarms/mods")).expect("scaffold");

            // Check scaffolded files exist
            assert!(mod_dir.join("manifest.json").is_file());
            assert!(mod_dir.join("main.lua").is_file());
            assert!(mod_dir.join("README.md").is_file());
            assert!(mod_dir.join(".luarc.json").is_file());
            assert!(mod_dir.join("dc_mod_api.d.lua").is_file());
            assert!(mod_dir.join("assets").is_dir());
            assert!(mod_dir.join("overrides").is_dir());

            // Check EmmyLua type annotations in dc_mod_api.d.lua
            let d_lua = fs::read_to_string(mod_dir.join("dc_mod_api.d.lua")).unwrap();
            assert!(d_lua.contains("---@class ModCapability"));
            assert!(d_lua.contains("---@class ModContentRegistry"));
            assert!(d_lua.contains("---@class ModHooks"));
            assert!(d_lua.contains("---@class ModEvents"));

            // Validate must succeed
            let val = validate_mod(&mod_dir, None);
            assert!(val.ok, "validate failed for {}: {:?}", mod_id, val.errors);

            // Lint must succeed with 0 diagnostics
            let diags = lint_mod(&mod_dir).expect("lint");
            assert!(
                diags
                    .iter()
                    .all(|d| !d.is_error() && d.message.contains("placeholder")),
                "lint failed for {}: {:?}",
                mod_id,
                diags
            );
            assert!(mod_dir.join("screenshots/main.png").is_file());

            // Pack twice and verify byte-for-byte deterministic ZIP output
            let zip1_path = tmp.join(format!("{}_1.zip", mod_id));
            let zip2_path = tmp.join(format!("{}_2.zip", mod_id));
            let bytes1 = pack_mod(&mod_dir, &zip1_path).expect("pack 1");
            let bytes2 = pack_mod(&mod_dir, &zip2_path).expect("pack 2");
            assert_eq!(bytes1, bytes2, "ZIP output must be 100% deterministic");
            assert!(bytes1.starts_with(b"PK\x03\x04"), "valid ZIP local header");
            assert_eq!(crc32(&bytes1), crc32(&bytes2));
        }

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_lint_detects_bytecode_forbidden_identifiers_globals_and_traversal() {
        let tmp = make_temp_dir("lint_neg");
        let mod_dir = scaffold_mod("bad_mod", &tmp, ScaffoldProfile::Content, None).unwrap();

        // 1. Forbidden identifiers, undeclared global assignment, and path traversal
        let bad_lua = r#"
leaked_top_global = 42

return function(mod)
    os.execute("calc.exe")
    io.popen("whoami")
    debug.getinfo(1)
    ffi.cdef("int printf(const char*, ...);")
    getfenv(1)
    setfenv(1, {})
    package.loadlib("evil.dll", "init")
    local x = require("../secret_config")
    local y = mod.assets:path("../../windows/win.ini")
end
"#;
        fs::write(mod_dir.join("main.lua"), bad_lua).unwrap();

        // Write a binary bytecode file too
        fs::write(mod_dir.join("bytecode.lua"), b"\x1bLuaQ\x00\x01\x04").unwrap();

        let diags = lint_mod(&mod_dir).expect("lint");
        let rendered: Vec<String> = diags.iter().map(|d| d.to_string()).collect();

        assert!(rendered
            .iter()
            .any(|s| s.contains("bytecode.lua:1:") && s.contains("Binary Lua bytecode")));
        assert!(rendered
            .iter()
            .any(|s| s.contains("main.lua:2:") && s.contains("leaked_top_global")));
        assert!(rendered.iter().any(|s| s.contains("os.execute")));
        assert!(rendered.iter().any(|s| s.contains("io.popen")));
        assert!(rendered.iter().any(|s| s.contains("debug.getinfo")));
        assert!(rendered.iter().any(|s| s.contains("ffi.cdef")));
        assert!(rendered.iter().any(|s| s.contains("getfenv")));
        assert!(rendered.iter().any(|s| s.contains("setfenv")));
        assert!(rendered.iter().any(|s| s.contains("package.loadlib")));
        assert!(rendered.iter().any(|s| s.contains("require()")));
        assert!(rendered.iter().any(|s| s.contains("mod.assets:path()")));

        // `pack` must fail and refuse to emit zip when lint/validate fails
        let bad_zip = tmp.join("should_not_exist.zip");
        assert!(pack_mod(&mod_dir, &bad_zip).is_err());
        assert!(!bad_zip.exists());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_cli_runner_commands() {
        let tmp = make_temp_dir("cli_runner");
        let dir_str = tmp.to_string_lossy().to_string();

        let code = run_cli(&[
            "dc-modkit".to_string(),
            "scaffold".to_string(),
            "cli_mod".to_string(),
            "--dir".to_string(),
            dir_str.clone(),
            "--profile".to_string(),
            "overhaul".to_string(),
            "--github".to_string(),
            "example/cli_mod".to_string(),
        ]);
        assert_eq!(code, 0);

        let mod_path = tmp.join("cli_mod").to_string_lossy().to_string();
        assert_eq!(
            run_cli(&[
                "dc-modkit".to_string(),
                "validate".to_string(),
                mod_path.clone(),
                "--quiet".to_string(),
            ]),
            0
        );
        assert_eq!(
            run_cli(&[
                "dc-modkit".to_string(),
                "lint".to_string(),
                mod_path.clone(),
                "--quiet".to_string(),
            ]),
            0
        );

        let zip_path = tmp.join("cli_mod.zip").to_string_lossy().to_string();
        assert_eq!(
            run_cli(&[
                "dc-modkit".to_string(),
                "pack".to_string(),
                mod_path,
                "-o".to_string(),
                zip_path.clone(),
                "--quiet".to_string(),
            ]),
            0
        );
        assert!(Path::new(&zip_path).is_file());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn zip_rejects_too_many_entries() {
        let entries: Vec<(String, Vec<u8>)> = (0..65536)
            .map(|i| (format!("f{}", i), Vec::new()))
            .collect();
        let err = write_deterministic_zip(&entries).unwrap_err();
        assert!(err.contains("65535"));
        assert!(write_deterministic_zip(&entries[..65535]).is_ok());
    }

    #[test]
    fn zip_rejects_long_name() {
        let ok = vec![("a".repeat(65535), vec![1u8])];
        assert!(write_deterministic_zip(&ok).is_ok());
        let bad = vec![("a".repeat(65536), vec![1u8])];
        assert!(write_deterministic_zip(&bad).unwrap_err().contains("65535"));
    }

    #[test]
    fn zip_u32_checked() {
        assert_eq!(zip_u32(u32::MAX as usize, "x"), Ok(u32::MAX));
        #[cfg(target_pointer_width = "64")]
        assert!(zip_u32(u32::MAX as usize + 1, "x")
            .unwrap_err()
            .contains("4 GiB"));
    }

    #[test]
    fn extension_histogram_synthetic() {
        let e = vec![
            ("A/b.DTX".to_string(), 0, 10),
            ("c.dtx".to_string(), 0, 5),
            ("d.LTB".to_string(), 0, 7),
            ("noext".to_string(), 0, 1),
        ];
        let h = extension_histogram(&e);
        assert_eq!(h["DTX"], (2, 15));
        assert_eq!(h["LTB"], (1, 7));
        assert_eq!(h[""], (1, 1));
    }

    #[test]
    fn rom_histogram_matches_known_totals_when_rom_present() {
        let Some(rom) = std::env::var_os("CA_ROM") else {
            return;
        };
        let (n, total, h) = rom_histogram(Path::new(&rom)).unwrap();
        assert_eq!(n, 95);
        assert_eq!(total, 3_120_026_872);
        for (ext, c) in [
            ("DTX", 11272),
            ("LTB", 1610),
            ("WAV", 1529),
            ("SPR", 233),
            ("GMS", 143),
            ("LTC", 111),
            ("DAT", 21),
        ] {
            assert_eq!(h[ext].0, c, "{}", ext);
        }
    }
}
