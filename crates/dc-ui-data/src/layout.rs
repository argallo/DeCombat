//! `layout_table`: the global look, per-screen constants, system font records, HUD layout set, lights,
//! interface FX and popup boxes of the 2009 UI, as typed accessors with the client's fallback rules.
//!
//! The text grammar (`[section]`, `key = value`) is parsed by `dc-bute`. Value forms (SPEC §6.1): decimal
//! integer, real, `"text"`, `(a, b, c, d)` integer 4-tuple, `(x, y)` integer pair, `<r, g, b>` real 3-vector.
//! `dc-bute` strips the quotes of string values, so the form is recovered from the text: a quoted value that
//! looks like a number reads as a number here (the client would tag it a string). String getters therefore
//! accept any stored value and return its text.
// SPEC: FMT_CRES_UI.md §5, §6.1, §6.2, §7.1, §8.2

use crate::error::UiDataError;
use crate::names::{UiNames, UiSection};
use crate::screens::UiScreen;
use dc_bute::{ButeDoc, SectionMap};
use dc_rez::RezVfs;
use std::collections::HashMap;

/// A typed attribute value. SPEC: FMT_CRES_UI.md §6.1
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i32),
    Real(f64),
    Str(String),
    Pair([i32; 2]),
    Rect([i32; 4]),
    Vec3([f64; 3]),
}

fn parse_list<T: std::str::FromStr>(inner: &str) -> Option<Vec<T>> {
    inner
        .split(',')
        .map(|p| p.trim().parse::<T>().ok())
        .collect()
}

/// Classifies attribute text by form. SPEC: FMT_CRES_UI.md §6.1
pub fn parse_value(text: &str) -> Value {
    let t = text.trim();
    if let Some(inner) = t.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        match parse_list::<i32>(inner).as_deref() {
            Some([a, b]) => return Value::Pair([*a, *b]),
            Some([a, b, c, d]) => return Value::Rect([*a, *b, *c, *d]),
            _ => {}
        }
    }
    if let Some(inner) = t.strip_prefix('<').and_then(|r| r.strip_suffix('>')) {
        if let Some([a, b, c]) = parse_list::<f64>(inner).as_deref() {
            if a.is_finite() && b.is_finite() && c.is_finite() {
                return Value::Vec3([*a, *b, *c]);
            }
        }
    }
    if let Ok(i) = t.parse::<i32>() {
        return Value::Int(i);
    }
    if t.chars().any(|c| c.is_ascii_digit()) {
        if let Ok(f) = t.parse::<f64>() {
            if f.is_finite() {
                return Value::Real(f);
            }
        }
    }
    Value::Str(text.to_string())
}

/// A borrowed section with typed getters (type mismatch = `None`, as the client's getters return the
/// caller default).
#[derive(Clone, Copy, Debug)]
pub struct SectionView<'a> {
    pub name: &'a str,
    pub map: &'a SectionMap,
}

impl<'a> SectionView<'a> {
    /// Raw text of `key` (exact, then ASCII case-insensitive).
    pub fn raw(&self, key: &str) -> Option<&'a str> {
        if let Some(v) = self.map.get(key) {
            return Some(v.as_str());
        }
        self.map
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
    pub fn has(&self, key: &str) -> bool {
        self.raw(key).is_some()
    }
    pub fn value(&self, key: &str) -> Option<Value> {
        self.raw(key).map(parse_value)
    }
    /// Tag 1 getter. EVIDENCE: cshell+0x1990c0
    pub fn int(&self, key: &str) -> Option<i32> {
        match self.value(key)? {
            Value::Int(i) => Some(i),
            _ => None,
        }
    }
    /// Real getter, integers accepted. EVIDENCE: cshell+0x199430
    pub fn real(&self, key: &str) -> Option<f64> {
        match self.value(key)? {
            Value::Int(i) => Some(i as f64),
            Value::Real(f) => Some(f),
            _ => None,
        }
    }
    /// String getter (lenient, see the module note). EVIDENCE: cshell+0x1995b0
    pub fn string(&self, key: &str) -> Option<&'a str> {
        self.raw(key)
    }
    /// Integer pair getter. EVIDENCE: cshell+0x1997e0
    pub fn pair(&self, key: &str) -> Option<[i32; 2]> {
        match self.value(key)? {
            Value::Pair(p) => Some(p),
            _ => None,
        }
    }
    /// Integer 4-tuple getter. EVIDENCE: cshell+0x1996f0
    pub fn rect(&self, key: &str) -> Option<[i32; 4]> {
        match self.value(key)? {
            Value::Rect(r) => Some(r),
            _ => None,
        }
    }
    /// Real 3-vector getter. EVIDENCE: cshell+0x1998c0
    pub fn vec3(&self, key: &str) -> Option<[f64; 3]> {
        match self.value(key)? {
            Value::Vec3(v) => Some(v),
            _ => None,
        }
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&'a str, &'a str)> {
        self.map.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

/// The seven per-screen keys that fall back to screen 0. SPEC: FMT_CRES_UI.md §6.2
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScreenKey {
    TitleFont = 0,
    TitleSize = 1,
    FontIndex = 2,
    FontSize = 3,
    ItemSpacing = 4,
    ItemAlignment = 5,
    MusicIntensity = 6,
}

/// A scene light record. EVIDENCE: cshell+0x1c7be0
#[derive(Clone, Debug, PartialEq)]
pub struct LightRecord {
    pub index: u32,
    pub name: String,
    pub position: [f64; 3],
    /// Colour scaled from 0..255 to 0..1 (the client multiplies by 1/255).
    pub colour: [f64; 3],
    pub radius: f64,
}

/// An interface FX record. EVIDENCE: cshell+0x1c7d50
#[derive(Clone, Debug, PartialEq)]
pub struct FxRecord {
    pub index: u32,
    pub name: String,
    pub effect: String,
    pub position: [f64; 3],
    /// Integer parameter (meaning UNPROVEN).
    pub parameter: i32,
}

/// A raw system font record (one numbered font section). SPEC: FMT_CRES_UI.md §7.1
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FontRecord {
    pub index: u32,
    pub name: String,
    pub file: String,
    pub face: String,
    pub width: i32,
    pub height: i32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

/// The parsed layout table plus the names it is addressed by.
#[derive(Clone, Debug)]
pub struct LayoutTable {
    doc: ButeDoc,
    folded: HashMap<String, String>,
    names: UiNames,
}

/// Upper bound on numbered-section probes (hostile-input guard; the 2009 ROM has at most 28).
const MAX_NUMBERED: u32 = 4096;

impl LayoutTable {
    /// Parses layout table text.
    pub fn parse(text: &str, names: UiNames) -> Self {
        Self::from_doc(ButeDoc::parse(text), names)
    }

    /// Wraps an already parsed document.
    pub fn from_doc(doc: ButeDoc, names: UiNames) -> Self {
        let mut folded = HashMap::new();
        for name in &doc.order {
            folded
                .entry(name.to_ascii_lowercase())
                .or_insert_with(|| name.clone());
        }
        Self { doc, folded, names }
    }

    /// Loads the table named by `names.layout_path` through the archive layer (the entry is stored
    /// encrypted; `RezVfs` decrypts it). SPEC: FMT_CRES_UI.md §4, §5 step 3.1
    pub fn load(vfs: &RezVfs, names: UiNames) -> Result<Self, UiDataError> {
        let text = crate::read_table_text(vfs, &names.layout_path, "layout_table")?;
        Ok(Self::parse(&text, names))
    }

    pub fn names(&self) -> &UiNames {
        &self.names
    }
    pub fn doc(&self) -> &ButeDoc {
        &self.doc
    }

    /// Number of sections.
    pub fn section_count(&self) -> usize {
        self.doc.order.len()
    }

    /// Number of key lines kept (a repeated key in one section counts once).
    pub fn key_count(&self) -> usize {
        self.doc.iter().map(|(_, m)| m.len()).sum()
    }

    /// A section by name (exact, then ASCII case-insensitive). EVIDENCE: cshell+0x199e30 (existence test)
    pub fn section(&self, name: &str) -> Option<SectionView<'_>> {
        let real: &str = match self.doc.sections.get_key_value(name) {
            Some((k, _)) => k.as_str(),
            None => self.folded.get(&name.to_ascii_lowercase())?.as_str(),
        };
        self.doc
            .get(real)
            .map(|map| SectionView { name: real, map })
    }

    pub fn has_section(&self, name: &str) -> bool {
        self.section(name).is_some()
    }

    /// Iterates all sections in file order.
    pub fn sections(&self) -> impl Iterator<Item = SectionView<'_>> {
        self.doc.iter().map(|(name, map)| SectionView { name, map })
    }

    // ---- base constants ----------------------------------------------------------------------------

    /// The base constants section. SPEC: FMT_CRES_UI.md §6.2 family 0
    pub fn base(&self) -> Option<SectionView<'_>> {
        self.section(&self.names.base_section)
    }

    /// Base integer `i` (0..5) of the base section. EVIDENCE: cshell+0x1c5c90..0x1c5cf0
    pub fn base_int(&self, i: usize) -> Option<i32> {
        self.base()?.int(self.names.key_base_ints.get(i)?)
    }

    /// Help font index (system font table). EVIDENCE: cshell+0x1c5c90
    pub fn help_font_index(&self) -> Option<i32> {
        self.base_int(0)
    }

    /// HUD font index (system font table). EVIDENCE: cshell+0x1c5cd0
    pub fn hud_font_index(&self) -> Option<i32> {
        self.base_int(2)
    }

    /// Background colour packed `0x00RRGGBB` from the base section's colour vector (components truncated
    /// to integers and masked to 8 bits; absent key = 0). EVIDENCE: cshell+0x1c7f00
    pub fn background_colour(&self) -> u32 {
        let v = self
            .base()
            .and_then(|b| b.vec3(&self.names.key_background))
            .unwrap_or([0.0; 3]);
        let c = |x: f64| (x as i64 as u32) & 0xff;
        (c(v[0]) << 16) | (c(v[1]) << 8) | c(v[2])
    }

    // ---- per-screen keys --------------------------------------------------------------------------------

    /// The section of a screen (`None` for the 9 screens that have none).
    pub fn screen_section(&self, screen: UiScreen) -> Option<SectionView<'_>> {
        self.section(screen.name(&self.names))
    }

    /// One of the seven fallback keys: read from the screen's section when the key exists there, else from
    /// screen 0's section. A type mismatch gives `None` (the client's default 0). EVIDENCE: cshell+0x1c5e70
    pub fn screen_int(&self, screen: UiScreen, key: ScreenKey) -> Option<i32> {
        let k = &self.names.key_screen[key as usize];
        match self.screen_section(screen) {
            Some(s) if s.has(k) => s.int(k),
            _ => self.screen_section(UiScreen::Generic)?.int(k),
        }
    }

    /// A caller-named key in the screen's own section, no fallback. EVIDENCE: cshell+0x1c6110
    pub fn screen_value(&self, screen: UiScreen, key: &str) -> Option<Value> {
        self.screen_section(screen)?.value(key)
    }

    /// A caller-named string key of screen 0 only. EVIDENCE: cshell+0x1c6250
    pub fn generic_string(&self, key: &str) -> Option<&str> {
        self.screen_section(UiScreen::Generic)?.string(key)
    }

    // ---- numbered sections ----------------------------------------------------------------------------------

    fn numbered(&self, prefix: &str, n: u32) -> Option<SectionView<'_>> {
        self.section(&format!("{prefix}{n}"))
    }

    /// Count of consecutive sections `prefix + 0, prefix + 1, ...` up to the first missing one.
    pub fn count_numbered(&self, prefix: &str) -> u32 {
        let mut n = 0;
        while n < MAX_NUMBERED && self.numbered(prefix, n).is_some() {
            n += 1;
        }
        n
    }

    // ---- HUD -------------------------------------------------------------------------------------------------

    /// Number of HUD layout sets. EVIDENCE: cshell+0x1d2cb0 (step 3.2)
    pub fn hud_set_count(&self) -> u32 {
        self.count_numbered(&self.names.hud_prefix)
    }

    /// HUD set `set`, `None` when absent.
    pub fn hud_set(&self, set: u32) -> Option<SectionView<'_>> {
        self.numbered(&self.names.hud_prefix, set)
    }

    /// Raw text of a HUD key in set `set`, falling back to set 0 when the key is absent there.
    /// EVIDENCE: cshell+0x4bce0 (SPEC §6.2, §8.2)
    pub fn hud_raw(&self, set: u32, key: &str) -> Option<&str> {
        if let Some(v) = self.hud_set(set).and_then(|s| s.raw(key)) {
            return Some(v);
        }
        self.section(&self.names.hud_set0)?.raw(key)
    }

    /// Typed HUD value with the set-0 fallback.
    pub fn hud_value(&self, set: u32, key: &str) -> Option<Value> {
        self.hud_raw(set, key).map(parse_value)
    }

    /// The base HUD section (the prefix itself, 4 position keys). SPEC: FMT_CRES_UI.md §6.2 family 57
    pub fn hud_base(&self) -> Option<SectionView<'_>> {
        self.section(&self.names.hud_prefix)
    }

    /// Base HUD position `i` (0..4). EVIDENCE: cshell+0x1c8680, +0x1c86a0
    pub fn hud_base_pos(&self, i: usize) -> Option<[i32; 2]> {
        self.hud_base()?.pair(self.names.key_hud_base.get(i)?)
    }

    // ---- fonts -------------------------------------------------------------------------------------------------

    /// Number of system font records. EVIDENCE: cshell+0x1d2cb0 (step 3.4)
    pub fn font_record_count(&self) -> u32 {
        self.count_numbered(&self.names.font_prefix)
    }

    /// System font record `n`. SPEC: FMT_CRES_UI.md §7.1 (request block of cshell+0x1c6380)
    pub fn font_record(&self, n: u32) -> Option<FontRecord> {
        let s = self.numbered(&self.names.font_prefix, n)?;
        let k = &self.names.key_font;
        let flag = |key: &str| s.int(key).map(|v| v != 0).unwrap_or(false);
        Some(FontRecord {
            index: n,
            name: s.string(&self.names.key_name).unwrap_or("").to_string(),
            file: s.string(&k[0]).unwrap_or("").to_string(),
            face: s.string(&k[1]).unwrap_or("").to_string(),
            width: s.int(&k[2]).unwrap_or(0),
            height: s.int(&k[3]).unwrap_or(0),
            bold: flag(&k[4]),
            italic: flag(&k[5]),
            underline: flag(&k[6]),
        })
    }

    /// All system font records in index order.
    pub fn font_records(&self) -> Vec<FontRecord> {
        (0..self.font_record_count())
            .filter_map(|n| self.font_record(n))
            .collect()
    }

    // ---- lights, FX, transitions ---------------------------------------------------------------------------

    /// Scene light records; records without a name are dropped and numbering continues.
    /// EVIDENCE: cshell+0x1c7be0 (SPEC §5 step 3.5)
    pub fn lights(&self) -> Vec<LightRecord> {
        let n = self.count_numbered(&self.names.light_prefix);
        let [kp, kc, kr] = &self.names.key_light;
        (0..n)
            .filter_map(|i| {
                let s = self.numbered(&self.names.light_prefix, i)?;
                let name = s.string(&self.names.key_name).unwrap_or("");
                if name.is_empty() {
                    return None;
                }
                let col = s.vec3(kc).unwrap_or([0.0; 3]);
                Some(LightRecord {
                    index: i,
                    name: name.to_string(),
                    position: s.vec3(kp).unwrap_or([0.0; 3]),
                    colour: [col[0] / 255.0, col[1] / 255.0, col[2] / 255.0],
                    radius: s.real(kr).unwrap_or(0.0),
                })
            })
            .collect()
    }

    /// Interface FX records; records without a name or effect are dropped. EVIDENCE: cshell+0x1c7d50
    pub fn interface_fx(&self) -> Vec<FxRecord> {
        let n = self.count_numbered(&self.names.fx_prefix);
        let [ke, kp] = &self.names.key_fx;
        (0..n)
            .filter_map(|i| {
                let s = self.numbered(&self.names.fx_prefix, i)?;
                let name = s.string(&self.names.key_name).unwrap_or("");
                let effect = s.string(ke).unwrap_or("");
                if name.is_empty() || effect.is_empty() {
                    return None;
                }
                Some(FxRecord {
                    index: i,
                    name: name.to_string(),
                    effect: effect.to_string(),
                    position: s.vec3(&self.names.key_light[0]).unwrap_or([0.0; 3]),
                    parameter: s.int(kp).unwrap_or(0),
                })
            })
            .collect()
    }

    /// Number of screen transition sections (0 in the 2009 ROM). SPEC: FMT_CRES_UI.md §5 step 3.7
    pub fn transition_count(&self) -> u32 {
        self.count_numbered(&self.names.transition_prefix)
    }

    // ---- named sections ------------------------------------------------------------------------------------

    /// A popup box, mode HUD or constants section opened by literal.
    pub fn named(&self, s: UiSection) -> Option<SectionView<'_>> {
        self.section(self.names.section(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> LayoutTable {
        let text = "\
[Base]\r\n\
BaseInt0 = 3\r\n\
BaseInt2 = 5\r\n\
Bg = <10.0, 300.0, 2.9>\r\n\
[Scr0]\r\n\
ScrKey0 = 1\r\n\
ScrKey2 = 4\r\n\
Label = \"generic\"\r\n\
[Scr3]\r\n\
ScrKey2 = 9\r\n\
ScrKey3 = \"x\"\r\n\
Pos = (10, 20)\r\n\
Box = (1,2,3,4)\r\n\
[Hud]\r\n\
HudBase0 = (5,6)\r\n\
[Hud0]\r\n\
A = 1\r\n\
B = 2.5\r\n\
[Hud1]\r\n\
A = 7\r\n\
[Fnt0]\r\n\
Nm = \"a\"\r\n\
Face = \"F\"\r\n\
H = 14\r\n\
B = 1\r\n\
[Fnt1]\r\n\
Nm = \"b\"\r\n\
File = \"dir\\f.ttf\"\r\n\
H = 20\r\n\
I = 0\r\n\
[Fnt3]\r\n\
Nm = \"orphan\"\r\n\
[Lamp0]\r\n\
Nm = \"l0\"\r\n\
P = <1, 2, 3>\r\n\
Col = <255, 0, 51>\r\n\
R = 100\r\n\
[Lamp1]\r\n\
P = <0,0,0>\r\n\
[Lamp2]\r\n\
Nm = \"l2\"\r\n\
R = 2.5\r\n\
[Ef0]\r\n\
Nm = \"e\"\r\n\
Eff = \"boom\"\r\n\
Par = 3\r\n\
[Ef1]\r\n\
Nm = \"noeffect\"\r\n\
[megaphone]\r\n\
Q = (1,1)\r\n";
        LayoutTable::parse(text, UiNames::placeholder())
    }

    #[test]
    fn value_forms() {
        assert_eq!(parse_value("12"), Value::Int(12));
        assert_eq!(parse_value("-3"), Value::Int(-3));
        assert_eq!(parse_value("0.5"), Value::Real(0.5));
        assert_eq!(parse_value("(1, 2)"), Value::Pair([1, 2]));
        assert_eq!(parse_value("(1,2,3,4)"), Value::Rect([1, 2, 3, 4]));
        assert_eq!(parse_value("<1, 2.5, 3>"), Value::Vec3([1.0, 2.5, 3.0]));
        assert_eq!(parse_value("(1,2,3)"), Value::Str("(1,2,3)".into()));
        assert_eq!(parse_value("inf"), Value::Str("inf".into()));
        assert_eq!(parse_value("a\\b.dtx"), Value::Str("a\\b.dtx".into()));
    }

    #[test]
    fn counts_and_base() {
        let t = fixture();
        assert_eq!(t.section_count(), 15);
        assert_eq!(t.help_font_index(), Some(3));
        assert_eq!(t.hud_font_index(), Some(5));
        assert_eq!(t.base_int(1), None);
        assert_eq!(t.background_colour(), 0x000a_2c02);
    }

    #[test]
    fn screen_fallback_rules() {
        let t = fixture();
        // key in the screen's own section
        assert_eq!(
            t.screen_int(UiScreen::Screen3, ScreenKey::FontIndex),
            Some(9)
        );
        // absent there: section 0
        assert_eq!(
            t.screen_int(UiScreen::Screen3, ScreenKey::TitleFont),
            Some(1)
        );
        // present but wrong type: the client's default, no fallback
        assert_eq!(t.screen_int(UiScreen::Screen3, ScreenKey::FontSize), None);
        // screen without a section: section 0
        assert_eq!(
            t.screen_int(UiScreen::Screen5, ScreenKey::FontIndex),
            Some(4)
        );
        assert_eq!(
            t.screen_int(UiScreen::Screen5, ScreenKey::MusicIntensity),
            None
        );
        // generic accessors without fallback
        assert_eq!(
            t.screen_value(UiScreen::Screen3, "Pos"),
            Some(Value::Pair([10, 20]))
        );
        assert_eq!(t.screen_value(UiScreen::Screen5, "Label"), None);
        assert_eq!(t.generic_string("Label"), Some("generic"));
        assert_eq!(
            t.screen_section(UiScreen::Screen3).unwrap().rect("Box"),
            Some([1, 2, 3, 4])
        );
    }

    #[test]
    fn hud_sets_and_fallback() {
        let t = fixture();
        assert_eq!(t.hud_set_count(), 2);
        assert_eq!(t.hud_value(1, "A"), Some(Value::Int(7)));
        assert_eq!(t.hud_value(1, "B"), Some(Value::Real(2.5)));
        assert_eq!(t.hud_value(9, "A"), Some(Value::Int(1)));
        assert_eq!(t.hud_value(0, "Z"), None);
        assert_eq!(t.hud_base_pos(0), Some([5, 6]));
        assert_eq!(t.hud_base_pos(1), None);
    }

    #[test]
    fn fonts_lights_fx_named() {
        let t = fixture();
        assert_eq!(t.font_record_count(), 2);
        let f = t.font_records();
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].face, "F");
        assert!(f[0].bold && !f[0].italic);
        assert_eq!(f[1].file, "dir\\f.ttf");
        assert_eq!(f[1].height, 20);
        let l = t.lights();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].colour, [1.0, 0.0, 0.2]);
        assert_eq!(l[0].radius, 100.0);
        assert_eq!(l[1].index, 2);
        assert_eq!(l[1].radius, 2.5);
        let fx = t.interface_fx();
        assert_eq!(fx.len(), 1);
        assert_eq!(fx[0].parameter, 3);
        assert_eq!(t.transition_count(), 0);
        // case-insensitive section lookup
        assert_eq!(
            t.named(UiSection::Megaphone).unwrap().pair("q"),
            Some([1, 1])
        );
        assert!(t.named(UiSection::Coupon).is_none());
    }
}
