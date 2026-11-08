//! A camo on disk is a Vanilla++ pack, so there is one format: saving writes a pack the game can load, opening reads one, and
//! the examples in the repo open like any other.
// DC-DEFAULT: VAR-6 (proposed) the studio's save format is the pack itself, checked by dc-modkit before it says it is done

use crate::design::{Art, Design};
use crate::guns::{Gun, View};
use dc_bute::modpatch::VariantInfo;
use dc_mod_lua::{ModHost, ModKind, ModManifest, SemVer};
use dc_render::camo::{self, CamoPath, Circle, MODE_PAINT, MODE_REPLACE};
use dc_render::TextureImage;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The folder under the user's documents that holds saved camos.
pub const CAMOS_FOLDER: &str = "DeCombat Camos";

/// The rows a variant may use (GUNKIT_PLAN.md section 6).
pub const VARIANT_ROWS: std::ops::RangeInclusive<u32> = 500..=899;

/// A picture that follows the gun's own texture this closely is a copy or a recolour of the game's art.
pub const MAX_LIKENESS: f32 = 0.85;

/// What a save writes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SaveOptions {
    pub name: String,
    pub author: String,
    /// The variant's row (`None`: chosen from the name).
    pub row: Option<u32>,
    /// Where to write (`None`: a folder of the name under [`camos_dir`]).
    pub dir: Option<PathBuf>,
}

/// What a save did.
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    pub dir: PathBuf,
    pub id: String,
    pub row: u32,
    pub warnings: Vec<String>,
}

/// The user's documents folder.
pub fn documents_dir() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|d| !d.is_empty());
    if let Some(d) = var("DC_DOCUMENTS") {
        return Some(PathBuf::from(d));
    }
    if let Some(d) = var("USERPROFILE") {
        return Some(PathBuf::from(d).join("Documents"));
    }
    var("HOME").map(|h| PathBuf::from(h).join("Documents"))
}

/// Where saved camos live.
pub fn camos_dir() -> Option<PathBuf> {
    documents_dir().map(|d| d.join(CAMOS_FOLDER))
}

/// The packs saved there: `(name, folder)`, by name.
pub fn saved_camos() -> Vec<(String, PathBuf)> {
    let Some(dir) = camos_dir() else {
        return Vec::new();
    };
    let Ok(list) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = list
        .flatten()
        .filter_map(|e| {
            let m = ModManifest::from_file(&e.path().join("manifest.json")).ok()?;
            Some((m.name, e.path()))
        })
        .collect();
    out.sort_by_key(|a| a.0.to_ascii_lowercase());
    out
}

/// A name as lower-case words joined by `_`, what a pack id may hold.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_string()
}

/// The row a name gets when none is chosen: the same name always gets the same row, above the examples'.
pub fn row_of_name(id: &str) -> u32 {
    let first = 600u32;
    let span = VARIANT_ROWS.end() - first + 1;
    let hash = id.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ b as u32).wrapping_mul(16_777_619)
    });
    first + hash % span
}

/// Refuses art that is the game's own texture, or a recolour of it (GUNKIT_PLAN.md 3.1).
pub fn check_own_art(base: &TextureImage, art: &Art, what: &str) -> Result<(), String> {
    match camo::likeness(base, &art.image()) {
        Some(l) if l > MAX_LIKENESS => Err(format!(
            "{what} follows the gun's own texture too closely to ship ({:.0}% alike): a pack may not carry the game's art, \
             not even recoloured. Paint with a pattern instead, which keeps the gun's shading on the player's machine.",
            l * 100.0
        )),
        _ => Ok(()),
    }
}

fn lua_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn write_art(dir: &Path, name: &str, art: &Art) -> Result<(), String> {
    let (w, h) = (
        u16::try_from(art.width).map_err(|_| "art too wide")?,
        u16::try_from(art.height).map_err(|_| "art too tall")?,
    );
    let bytes = dc_tex::write_bgra8(w, h, &art.rgba).ok_or("art could not be written as a DTX")?;
    std::fs::write(dir.join("assets").join(name), bytes).map_err(|e| e.to_string())
}

/// A small picture of the pattern alone. Nothing of the game is in it (GUNKIT_PLAN.md 3.1, previews included).
fn swatch_png(art: Option<&Art>) -> Result<Vec<u8>, String> {
    const SIDE: u32 = 256;
    let mut px = vec![60u8; (SIDE * SIDE * 4) as usize];
    if let Some(a) = art {
        for y in 0..SIDE {
            for x in 0..SIDE {
                let i = ((y * SIDE + x) * 4) as usize;
                let j = (((y * a.height / SIDE) * a.width + x * a.width / SIDE) * 4) as usize;
                px[i..i + 4].copy_from_slice(&a.rgba[j..j + 4]);
                px[i + 3] = 255;
            }
        }
    }
    let img = image::RgbaImage::from_raw(SIDE, SIDE, px).ok_or("swatch size")?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// One skin line of the pack's `main.lua`.
fn skin_line(
    key: String,
    mode: &str,
    row: u32,
    keep: &[Circle],
    (fx, flow): (Option<camo::CamoFx>, Option<camo::CamoFlow>),
    art: &str,
    glow: Option<&str>,
) -> String {
    // the paths are built in Lua, around the pack's own asset calls: the placeholders stand where those calls go
    let path = camo::format_path(&CamoPath {
        mode,
        base_row: row as i32,
        base_key: &key,
        keep: keep.to_vec(),
        fx,
        flow,
        pattern: "\u{1}",
        fx_pattern: glow.map(|_| "\u{2}"),
    });
    let mut parts = path.split('\u{1}');
    let head = parts.next().unwrap_or_default().to_string();
    let lua = format!(
        "{} .. mod.assets:path({})",
        lua_string(&head),
        lua_string(&format!("assets/{art}"))
    );
    let lua = match glow {
        Some(g) => format!(
            "{lua} .. \"|\" .. mod.assets:path({})",
            lua_string(&format!("assets/{g}"))
        ),
        None => lua,
    };
    format!("        {key} = {lua},\n")
}

/// Writes the design for `gun` as a pack, and checks it as a curator's tools will.
pub fn save(
    design: &Design,
    gun: &Gun,
    base_of: &dyn Fn(View) -> Option<TextureImage>,
    opts: &SaveOptions,
) -> Result<Saved, String> {
    let name = opts.name.trim();
    if name.is_empty() {
        return Err("give the camo a name first".into());
    }
    let word = slug(name);
    if word.is_empty() {
        return Err("a camo's name needs a letter or a digit".into());
    }
    let id = format!("variant_{word}_{}", gun.row);
    let row = opts.row.unwrap_or_else(|| row_of_name(&id));
    if !VARIANT_ROWS.contains(&row) {
        return Err(format!(
            "row {row} is not a variant row ({}..{})",
            VARIANT_ROWS.start(),
            VARIANT_ROWS.end()
        ));
    }
    let views: Vec<View> = [View::FirstPerson, View::ThirdPerson]
        .into_iter()
        .filter(|v| gun.model(*v).is_some() && design.changes(gun.row, *v))
        .collect();
    if views.is_empty() {
        return Err("there is nothing to save yet: choose a pattern first".into());
    }
    for v in &views {
        let Some(base) = base_of(*v) else {
            continue;
        };
        if let Some(p) = &design.paint {
            check_own_art(&base, p, "The pattern")?;
        }
        if let Some(g) = &design.glow {
            check_own_art(&base, g, "The effect's pattern")?;
        }
        if let Some(f) = design.full.get(&(gun.row, *v)) {
            check_own_art(&base, f, "The full texture")?;
        }
    }
    let dir = match &opts.dir {
        Some(d) => d.clone(),
        None => camos_dir()
            .ok_or("no documents folder to save in")?
            .join(&word),
    };
    std::fs::create_dir_all(dir.join("assets")).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir.join("screenshots")).map_err(|e| e.to_string())?;

    let mut skins = String::new();
    if let Some(p) = &design.paint {
        write_art(&dir, "pattern.dtx", p)?;
    }
    // VAR-12: a flow takes the place of the light effect, so a flowing design writes neither the effect nor its pattern
    let glow_art = design.glow.as_ref().filter(|_| design.flow.is_none());
    if let Some(g) = glow_art {
        write_art(&dir, "glow.dtx", g)?;
    }
    for v in &views {
        let key = format!("{}{}", v.skin_prefix(), v.main_slot());
        let keep = design.keep_of(gun.row, *v);
        let tag = match v {
            View::FirstPerson => "first_person",
            View::ThirdPerson => "third_person",
        };
        let glow = glow_art.map(|_| "glow.dtx");
        if let Some(f) = design.full.get(&(gun.row, *v)) {
            let file = format!("full_{tag}.dtx");
            write_art(&dir, &file, f)?;
            skins.push_str(&skin_line(
                key,
                MODE_REPLACE,
                gun.row,
                keep,
                (design.fx, None),
                &file,
                glow,
            ));
        } else if design.paint.is_some() {
            skins.push_str(&skin_line(
                key,
                MODE_PAINT,
                gun.row,
                keep,
                match design.flow {
                    Some(flow) => (None, Some(flow)),
                    None => (design.fx, None),
                },
                "pattern.dtx",
                glow,
            ));
        } else {
            return Err(format!(
                "the {} has an effect but nothing to paint: choose a pattern for it",
                v.label()
            ));
        }
    }
    let shown = format!("{} {}", gun.name, name);
    let lua = format!(
        "-- Made with dc-gunkit: row {row} is row {base} with a new look and nothing else. The pack holds only its own art, which the client\n\
         -- paints over the player's own texture when the gun loads.\n\
         ---@param mod ModCapability\n\
         return function(mod)\n    \
             mod.content.weapons:register({{\n        \
                 id = \"Weapon{row}\",\n        \
                 variant_of = {base},\n        \
                 variant_name = {shown_lua},\n\
         {skins}    }})\n\
         end\n",
        base = gun.row,
        shown_lua = lua_string(&shown),
    );
    std::fs::write(dir.join("main.lua"), lua).map_err(|e| e.to_string())?;

    let version = SemVer::parse("0.1.0").map_err(|e| e.to_string())?;
    let mut manifest = ModManifest::minimal(&id, &shown, version, ModKind::Shared);
    manifest.capabilities = vec!["assets".into(), "content.weapons".into()];
    manifest.description = format!(
        "{name}: a new look for the {}. It looks different and plays exactly the same.",
        gun.name
    );
    manifest.author = if opts.author.trim().is_empty() {
        "DeCombat community".to_string()
    } else {
        opts.author.trim().to_string()
    };
    manifest.tags = vec!["variant".into(), "camo".into()];
    manifest.screenshots = vec!["screenshots/main.png".into()];
    manifest.requires_client = true;
    manifest.check_metadata()?;
    std::fs::write(dir.join("manifest.json"), manifest.to_json_pretty())
        .map_err(|e| e.to_string())?;
    let swatch = design
        .paint
        .as_ref()
        .or(design.glow.as_ref())
        .or_else(|| design.full.values().next());
    std::fs::write(dir.join("screenshots/main.png"), swatch_png(swatch)?)
        .map_err(|e| e.to_string())?;

    let report = dc_modkit::validate_mod(&dir, None);
    if !report.ok {
        return Err(format!(
            "the pack did not pass its check: {}",
            report.errors.join("; ")
        ));
    }
    let mut warnings = report.warnings;
    let lint = dc_modkit::lint_mod(&dir).map_err(|e| format!("lint: {e}"))?;
    for d in lint {
        warnings.push(d.to_string());
    }
    Ok(Saved {
        dir,
        id,
        row,
        warnings,
    })
}

/// A camo read from a pack.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Opened {
    /// The gun it is for.
    pub row: u32,
    pub name: String,
    pub design: Design,
    /// What the studio cannot show of it.
    pub notes: Vec<String>,
}

/// The variants of a pack folder, through the same host the game uses.
pub fn variants_of(dir: &Path) -> Result<Vec<VariantInfo>, String> {
    let manifest = ModManifest::from_file(&dir.join("manifest.json"))?;
    let mut host = ModHost::new();
    host.load_mod_from_dir_with_manifest(dir, manifest)?;
    let content = host.weapon_patches()?;
    Ok(content.variants())
}

/// The file of a `@mod:<id>/<path>` asset in a pack folder.
fn asset_file(dir: &Path, path: &str) -> Option<PathBuf> {
    let rest = path.strip_prefix("@mod:")?;
    let (_, file) = rest.split_once('/')?;
    if file.split(['/', '\\']).any(|p| p.is_empty() || p == "..") {
        return None;
    }
    Some(dir.join(file))
}

/// Opens the first variant of a pack as a design.
pub fn open(dir: &Path) -> Result<Opened, String> {
    let list = variants_of(dir)?;
    let v = list
        .first()
        .ok_or("this pack has no weapon variant to show")?;
    let row = u32::try_from(v.base).map_err(|_| "the variant's base gun is not a row")?;
    let mut design = Design {
        name: v
            .name
            .clone()
            .unwrap_or_else(|| format!("Variant {}", v.row)),
        ..Design::default()
    };
    let mut notes = Vec::new();
    if v.look_of.is_some() {
        notes.push("it also wears another gun's model, which the studio does not show".to_string());
    }
    let skins: Vec<(View, usize, &String)> = v
        .pv_skins
        .iter()
        .map(|(k, s)| (View::FirstPerson, *k, s))
        .chain(v.hh_skins.iter().map(|(k, s)| (View::ThirdPerson, *k, s)))
        .collect();
    for (view, slot, skin) in skins {
        if slot != view.main_slot() {
            notes.push(format!(
                "skin slot {slot} of the {} is not shown",
                view.label()
            ));
            continue;
        }
        let Some(c) = camo::parse(skin) else {
            notes.push(format!(
                "the {} skin is not a camo the studio reads",
                view.label()
            ));
            continue;
        };
        let file = asset_file(dir, c.pattern)
            .ok_or_else(|| format!("{} is not a file of this pack", c.pattern))?;
        let art = Art::from_file(&file)?;
        match c.mode {
            MODE_REPLACE => {
                design.full.insert((row, view), art);
            }
            _ => design.paint = Some(art),
        }
        if let Some(g) = c.fx_pattern {
            let file =
                asset_file(dir, g).ok_or_else(|| format!("{g} is not a file of this pack"))?;
            design.glow = Some(Art::from_file(&file)?);
        }
        if let Some(fx) = c.fx {
            design.fx = Some(fx);
        }
        if let Some(flow) = c.flow {
            design.flow = Some(flow);
        }
        if !c.keep.is_empty() {
            design.keep.insert((row, view), c.keep.clone());
        }
    }
    Ok(Opened {
        row,
        name: design.name.clone(),
        design,
        notes,
    })
}

/// Where the example packs are (docs/PACKS_FROM_REPOSITORIES.md section 4): `DC_GUNKIT_EXAMPLES`, else `examples/`
/// next to the program (the GunKit package ships the five), else, under cargo, the repository's `examples/mods`. The
/// first folder that exists wins; `None` without one. Nothing is built into the binary any more (VAR-13).
pub fn examples_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("DC_GUNKIT_EXAMPLES") {
        let d = PathBuf::from(d);
        return d.is_dir().then_some(d);
    }
    let mut dirs = Vec::new();
    if let Some(p) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        dirs.push(p.join("examples"));
    }
    if let Some(m) = std::env::var_os("CARGO_MANIFEST_DIR") {
        dirs.push(PathBuf::from(m).join("../../examples/mods"));
    }
    dirs.into_iter().find(|d| d.is_dir())
}

/// The example camos of folder `dir`: every pack whose manifest tags hold `camo` or `variant`, by name.
pub fn examples_in(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(list) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = list
        .flatten()
        .filter_map(|e| {
            let m = ModManifest::from_file(&e.path().join("manifest.json")).ok()?;
            m.tags
                .iter()
                .any(|t| t == "camo" || t == "variant")
                .then(|| (m.name, e.path()))
        })
        .collect();
    out.sort_by_key(|a| a.0.to_ascii_lowercase());
    out
}

/// The example camos of [`examples_dir`] (none without the folder).
pub fn examples() -> Vec<(String, PathBuf)> {
    examples_dir().map(|d| examples_in(&d)).unwrap_or_default()
}

/// Opens the example of folder `dir` named `name` (case does not matter). An example is a starting point: its
/// files are not the author's to watch.
pub fn open_example_in(dir: &Path, name: &str) -> Result<Opened, String> {
    let (_, path) = examples_in(dir)
        .into_iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("no example named {name:?} in {}", dir.display()))?;
    let mut opened = open(&path)?;
    for art in [opened.design.paint.as_mut(), opened.design.glow.as_mut()]
        .into_iter()
        .flatten()
    {
        art.source = None;
        art.stamp = None;
    }
    Ok(opened)
}

/// Opens an example of [`examples_dir`] by name.
pub fn open_example(name: &str) -> Result<Opened, String> {
    match examples_dir() {
        Some(d) => open_example_in(&d, name),
        None => Err(format!(
            "no example named {name:?}: no examples folder (DC_GUNKIT_EXAMPLES, or examples/ next to the program)"
        )),
    }
}

/// Which art of a design each layer holds, for a status line.
pub fn describe(design: &Design) -> BTreeMap<&'static str, String> {
    let of = |a: &Art| match &a.source {
        Some(p) => format!("{} x {}, {}", a.width, a.height, p.display()),
        None => format!("{} x {}", a.width, a.height),
    };
    let mut out = BTreeMap::new();
    if let Some(a) = &design.paint {
        out.insert("pattern", of(a));
    }
    if let Some(a) = &design.glow {
        out.insert("effect_pattern", of(a));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guns::GunModel;
    use crate::patterns;
    use dc_render::camo::CamoFx;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc-gunkit-pack-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn gun() -> Gun {
        let model = GunModel {
            model: "m.ltb".into(),
            skins: vec!["a".into(), "b".into()],
            styles: Vec::new(),
        };
        Gun {
            row: 25,
            name: "Test Gun".into(),
            on_sale: true,
            has_scope: false,
            first_person: Some(model.clone()),
            third_person: Some(model),
        }
    }

    fn checker_base() -> TextureImage {
        let mut px = Vec::new();
        for y in 0..64u32 {
            for x in 0..64u32 {
                let v = ((x * 7 + y * 13) % 200) as u8;
                px.extend_from_slice(&[v, v, v, 255]);
            }
        }
        TextureImage::rgba(64, 64, px)
    }

    fn options(dir: &Path) -> SaveOptions {
        SaveOptions {
            name: "Night Tiger".into(),
            author: "A Tester".into(),
            row: Some(777),
            dir: Some(dir.to_path_buf()),
        }
    }

    #[test]
    fn a_saved_camo_passes_the_pack_checks_and_opens_as_it_was() {
        let dir = temp("round");
        let mut d = Design {
            paint: Some(patterns::generate("tiger", None, 1).unwrap()),
            glow: Some(patterns::generate("stars", None, 2).unwrap()),
            fx: Some(CamoFx {
                speed: [0.02, -0.01],
                gain: [0.1, 0.6],
                seconds: 2.0,
            }),
            ..Design::default()
        };
        let lens = Circle {
            x: 10.0,
            y: 12.0,
            radius: 5.0,
        };
        d.keep.insert((25, View::FirstPerson), vec![lens]);
        let base = checker_base();
        let saved = save(&d, &gun(), &|_| Some(base.clone()), &options(&dir)).unwrap();
        assert_eq!(saved.row, 777);
        assert_eq!(saved.id, "variant_night_tiger_25");
        let report = dc_modkit::validate_mod(&dir, None);
        assert!(report.ok, "{:?}", report.errors);

        let v = variants_of(&dir).unwrap();
        assert_eq!((v[0].row, v[0].base), (777, 25));
        assert_eq!(v[0].name.as_deref(), Some("Test Gun Night Tiger"));

        let back = open(&dir).unwrap();
        assert_eq!(back.row, 25);
        assert!(back.notes.is_empty(), "{:?}", back.notes);
        let (p, g) = (
            back.design.paint.as_ref().unwrap(),
            back.design.glow.as_ref().unwrap(),
        );
        assert_eq!(p.rgba, d.paint.as_ref().unwrap().rgba);
        assert_eq!(g.rgba, d.glow.as_ref().unwrap().rgba);
        assert_eq!(back.design.fx, d.fx);
        assert_eq!(back.design.keep_of(25, View::FirstPerson), &[lens]);
        assert!(back.design.keep_of(25, View::ThirdPerson).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_flowing_camo_saves_without_its_light_and_opens_with_its_flow() {
        let dir = temp("flow");
        let d = Design {
            paint: Some(patterns::generate("tiger", None, 1).unwrap()),
            glow: Some(patterns::generate("stars", None, 2).unwrap()),
            fx: Some(CamoFx {
                speed: [0.02, -0.01],
                gain: [0.1, 0.6],
                seconds: 2.0,
            }),
            flow: Some(camo::CamoFlow {
                speed: [0.03, 0.01],
                swirl: 0.1,
                seconds: 8.0,
            }),
            ..Design::default()
        };
        let base = checker_base();
        save(&d, &gun(), &|_| Some(base.clone()), &options(&dir)).unwrap();
        let report = dc_modkit::validate_mod(&dir, None);
        assert!(report.ok, "{:?}", report.errors);
        let lua = std::fs::read_to_string(dir.join("main.lua")).unwrap();
        assert!(lua.contains("flow=0.03,0.01,0.1,8:"), "{lua}");
        assert!(
            !lua.contains("fx=") && !dir.join("assets/glow.dtx").exists(),
            "{lua}"
        );
        let back = open(&dir).unwrap();
        assert_eq!(back.design.flow, d.flow);
        assert_eq!(back.design.fx, None);
        let t = back.design.painted(25, View::FirstPerson, &base).unwrap();
        assert_eq!(t.fx.as_ref().and_then(|f| f.flow), Some([0.1, 8.0]));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_full_texture_is_saved_as_a_replacing_skin() {
        let dir = temp("full");
        let mut d = Design::default();
        let art = patterns::generate("urban", None, 1).unwrap();
        d.full.insert((25, View::FirstPerson), art.clone());
        let base = checker_base();
        save(&d, &gun(), &|_| Some(base.clone()), &options(&dir)).unwrap();
        let v = variants_of(&dir).unwrap();
        assert!(v[0].pv_skins[&1].starts_with("@camo:replace:25:PVSkin1:"));
        assert!(v[0].hh_skins.is_empty(), "the other view stays the game's");
        let back = open(&dir).unwrap();
        assert_eq!(back.design.full[&(25, View::FirstPerson)].rgba, art.rgba);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_games_art_or_a_recolour_of_it_is_refused() {
        let dir = temp("rom");
        let base = checker_base();
        let tinted: Vec<u8> = base.levels[0]
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|c| [c[0] / 2 + 40, c[1] / 3, 0, 255])
            .collect();
        for art in [base.levels[0].clone(), tinted] {
            let d = Design {
                paint: Some(Art::from_rgba(64, 64, art.clone()).unwrap()),
                ..Design::default()
            };
            let e = save(&d, &gun(), &|_| Some(base.clone()), &options(&dir)).unwrap_err();
            assert!(e.contains("too closely"), "{e}");
            let mut d = Design::default();
            d.full.insert(
                (25, View::FirstPerson),
                Art::from_rgba(64, 64, art).unwrap(),
            );
            let e = save(&d, &gun(), &|_| Some(base.clone()), &options(&dir)).unwrap_err();
            assert!(e.contains("too closely"), "{e}");
        }
        assert!(!dir.join("manifest.json").exists(), "nothing was written");
    }

    #[test]
    fn a_save_says_what_is_missing() {
        let dir = temp("words");
        let base = checker_base();
        let mut o = options(&dir);
        let none = Design::default();
        assert!(save(&none, &gun(), &|_| Some(base.clone()), &o)
            .unwrap_err()
            .contains("nothing to save"));
        let d = Design {
            paint: Some(patterns::generate("urban", None, 1).unwrap()),
            ..Design::default()
        };
        o.name = "  ".into();
        assert!(save(&d, &gun(), &|_| Some(base.clone()), &o)
            .unwrap_err()
            .contains("name"));
        o.name = "ok".into();
        o.row = Some(100);
        assert!(save(&d, &gun(), &|_| Some(base.clone()), &o)
            .unwrap_err()
            .contains("not a variant row"));
    }

    #[test]
    fn a_saved_pack_has_no_game_file_in_it() {
        let dir = temp("clean");
        let d = Design {
            paint: Some(patterns::generate("urban", None, 1).unwrap()),
            ..Design::default()
        };
        let base = checker_base();
        save(&d, &gun(), &|_| Some(base.clone()), &options(&dir)).unwrap();
        let mut files = Vec::new();
        for e in std::fs::read_dir(dir.join("assets")).unwrap().flatten() {
            files.push(e.file_name().to_string_lossy().to_string());
        }
        assert_eq!(files, vec!["pattern.dtx".to_string()]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The five example packs (the ones the GunKit package ships) in a folder of their own, else `None` when a
    /// checkout no longer holds them.
    fn five_examples() -> Option<PathBuf> {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods");
        let dir = temp("examples");
        let _ = std::fs::remove_dir_all(&dir);
        for id in [
            "variant_candy_g36e",
            "variant_galaxy_g36e",
            "variant_tiger_l96a1",
            "variant_toxic_l96a1",
            "variant_prism_scar",
        ] {
            let from = src.join(id);
            if !from.join("manifest.json").is_file() {
                return None;
            }
            for sub in ["", "assets"] {
                let d = dir.join(id).join(sub);
                std::fs::create_dir_all(&d).unwrap();
                for e in std::fs::read_dir(from.join(sub)).unwrap().flatten() {
                    if e.path().is_file() {
                        std::fs::copy(e.path(), d.join(e.file_name())).unwrap();
                    }
                }
            }
        }
        Some(dir)
    }

    #[test]
    fn every_example_opens_and_three_of_them_move() {
        let Some(dir) = five_examples() else {
            eprintln!("the five example packs are not in examples/mods: skipped");
            return;
        };
        let list = examples_in(&dir);
        assert_eq!(list.len(), 5, "{list:?}");
        let mut moving = 0;
        for (name, _) in &list {
            let o = open_example_in(&dir, name).unwrap();
            assert!(o.design.paint.is_some(), "{name}");
            assert!([25, 27, 33].contains(&o.row), "{name}");
            // The SCAR-L (row 33) has no sight, so no lens to keep clean.
            assert!(
                o.row == 33 || !o.design.keep.is_empty(),
                "{name} keeps its lens clean"
            );
            assert!(
                o.design.paint.as_ref().unwrap().source.is_none(),
                "{name}: an example's art is not a file to watch"
            );
            moving += usize::from(o.design.fx.is_some() || o.design.flow.is_some());
        }
        assert_eq!(moving, 3, "Galaxy, Toxic and Prism");
        assert!(
            open_example_in(&dir, "scar-l prism").unwrap().design.flow.is_some(),
            "Prism flows, and the name's case does not matter"
        );
        assert!(open_example_in(&dir, "nope").is_err());
        // a folder without camo packs lists nothing; a missing folder lists nothing
        let plain = temp("examples_plain");
        std::fs::create_dir_all(plain.join("x")).unwrap();
        std::fs::write(
            plain.join("x/manifest.json"),
            r#"{"id": "x", "name": "X", "version": "1.0.0", "kind": "shared", "tags": ["map"]}"#,
        )
        .unwrap();
        assert!(examples_in(&plain).is_empty());
        assert!(examples_in(&plain.join("nowhere")).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&plain).unwrap();
    }

    #[test]
    fn a_name_becomes_a_slug_and_a_stable_row() {
        assert_eq!(slug("Night Tiger!"), "night_tiger");
        assert_eq!(slug("  __ "), "");
        let r = row_of_name("variant_a_25");
        assert_eq!(r, row_of_name("variant_a_25"));
        assert!(VARIANT_ROWS.contains(&r) && r >= 600);
    }

    #[test]
    fn an_asset_path_stays_inside_the_pack() {
        let d = Path::new("/p");
        assert_eq!(
            asset_file(d, "@mod:x/assets/a.dtx"),
            Some(d.join("assets/a.dtx"))
        );
        assert_eq!(asset_file(d, "@mod:x/../a.dtx"), None);
        assert_eq!(asset_file(d, "a.dtx"), None);
    }
}
