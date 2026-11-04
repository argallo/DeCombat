//! Character body models of the user's ROM: the body rows of the ROM model table (model file and skin pairs),
//! the team mark table, the texture list a body model is drawn with, and the name keys of the clips the game
//! server streams for a player. Nothing here is ROM data: table names and keys are cited, clips are found by
//! the SHA-256 key of their names.

use dc_bute::ButeDoc;
use dc_rez::RezVfs;
use std::collections::BTreeMap;

/// Archive of the attribute tables.
// OBSERVED: ROM table key ATTRIBUTES.rez (the archive holding the model and mark tables)
pub const TABLE_REZ: &str = "ATTRIBUTES.rez";
/// The model table and the team-mark table.
// OBSERVED: ROM table keys MODELBUTES.TXT and MARKS.TXT (entries of ATTRIBUTES.rez)
pub const MODEL_TABLE: &str = "ATTRIBUTES\\MODELBUTES.TXT";
pub const MARK_TABLE: &str = "ATTRIBUTES\\MARKS.TXT";
/// `[Model<n>]` rows: `ModelFile`, `UpperSkin<k>`, `LowerSkin<k>`; `[TeamMark] TeamName<k>`.
// OBSERVED: ROM table key MODELBUTES.TXT [Model<n>] ModelFile, UpperSkin<k>, LowerSkin<k>; MARKS.TXT [TeamMark]
// TeamName<k>
const BODY_SECTION: &str = "Model";
const KEY_MODEL_FILE: &str = "ModelFile";
const KEY_UPPER: &str = "UpperSkin";
const KEY_LOWER: &str = "LowerSkin";
const MARK_SECTION: &str = "TeamMark";
/// `[Face<n>]` rows: `ModelFile`, `Skin<k>`.
// OBSERVED: ROM table key MODELBUTES.TXT [Face<n>] ModelFile, Skin0
const FACE_SECTION: &str = "Face";
const KEY_SKIN: &str = "Skin";
const KEY_MARK: &str = "TeamName";
/// Render-style names of a body or face row, in style-slot order.
// OBSERVED: ROM table key MODELBUTES.TXT [Model<n>] / [Face<n>] RenderStyle<k> (k = 0.., contiguous)
const KEY_STYLE: &str = "RenderStyle";
/// The hand identifier of a body row (which `PVModel<hand>` of a weapon row the character's first-person view uses)
/// and the hand skin format strings of the row (`HandsSkin<k>`, one decimal field for the skin variant).
// OBSERVED: ROM table key MODELBUTES.TXT [Model<n>] PVGunsModel, HandsSkin<k> (k = 0.., contiguous; the values carry
// one zero-padded decimal field); EVIDENCE: cshell+0x1e4760 (the model-table reader takes PVGunsModel into the row at
// +0xa8 and the HandsSkin list into the row at +0xa4), cshell+0x1e0f00 (row +0xa8 handed out), cshell+0x1e3180
// (HandsSkin entry `index` of the row, `sprintf` with one byte argument)
const KEY_PV_IDENT_OF_ROW: &str = "PVGunsModel";
const KEY_HANDS_SKIN: &str = "HandsSkin";
const KEY_SHOP_CLIP: &str = "InvenShopAni";
/// The weapon table and its third-person ("in the hand") model columns.
// OBSERVED: ROM table key WEAPONS.TXT (entry of ATTRIBUTES.rez), sections [Weapon<n>] with HHModel, HHSkin<k>,
// HHRenderStyle<k> (the same keys dc-dserver reads for the weapon object it attaches to a player)
pub const WEAPON_TABLE: &str = "ATTRIBUTES\\WEAPONS.TXT";
const WEAPON_SECTION: &str = "Weapon";
const KEY_HH_MODEL: &str = "HHModel";
const KEY_HH_SKIN: &str = "HHSkin";
const KEY_HH_STYLE: &str = "HHRenderStyle";
/// The first-person ("player view") columns of the weapon rows: `PVModel<hand>` for a hand identifier of
/// `[PVModelIdentifiers] Name<k>`, `PVSkin<k>`, `PVRenderStyle<k>`.
// OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] PVModel<hand>, PVSkin<k>, PVRenderStyle<k>; [PVModelIdentifiers]
// Name<k> (2026-10-25: 7 identifiers; 297 of 298 rows carry first-person keys; the same reading as dc-client's
// weapon table, CLI-34)
const KEY_PV_MODEL_PREFIX: &str = "PVModel";
const KEY_PV_SKIN: &str = "PVSkin";
const KEY_PV_STYLE: &str = "PVRenderStyle";
const PV_IDENT_SECTION: &str = "PVModelIdentifiers";
const KEY_PV_IDENT: &str = "Name";

/// The body socket a player's held weapon hangs from: found by its name key, else socket 2.
// SPEC: UDP_SESSION_2009_B.md §3.5 / §3.5.1 (weapon child attached at socket 2, the body socket whose name key is
// c969c8aa86333373/9, socket index 2 in every ROM body that has it; CAPTURE_tdm2 REFERENCE-CONFIRMED)
pub const WEAPON_SOCKET_KEY: NameKey = ([0xc9, 0x69, 0xc8, 0xaa, 0x86, 0x33, 0x33, 0x73], 9);
pub const WEAPON_SOCKET_INDEX: usize = 2;

/// The socket a weapon attaches to on `model`: the one named by [`WEAPON_SOCKET_KEY`], else
/// [`WEAPON_SOCKET_INDEX`] when the model has that many sockets.
pub fn grip_socket(model: &dc_model::Model) -> Option<usize> {
    model
        .sockets
        .iter()
        .position(|s| name_key(s.name.as_bytes()) == WEAPON_SOCKET_KEY)
        .or_else(|| (model.sockets.len() > WEAPON_SOCKET_INDEX).then_some(WEAPON_SOCKET_INDEX))
}

/// How many times the team mark follows the two skins in a body's texture list.
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (body model block: tex_count 5 = two body skins, then one
// mark texture three times; REFERENCE, accepted by the 2009 client)
pub const MARK_REPEAT: usize = 3;

/// Name key: the first 8 bytes of SHA-256 of a name and its length (no ROM name in the tree).
pub type NameKey = ([u8; 8], usize);

pub fn name_key(name: &[u8]) -> NameKey {
    let h = dc_rom::sha256(name);
    let mut k = [0u8; 8];
    k.copy_from_slice(&h[..8]);
    (k, name.len())
}

/// The clips the game server animates a player with: upper body standing / walking (tracker 1, weight set 0),
/// lower body standing / walking (tracker 2, weight set 1).
// SPEC: SESSION_B_S2C_addendum_dserver3.md S3-5 (clip keys bc721b0c9c7aa226/7, f9c53f256431fb05/7,
// 7f007bf2ff2bcc78/3, 5f8e2d7e3aa7a17d/3; with the EU-test ROM indices 96 / 99 / 6 / 47)
pub const CLIP_UPPER_STAND: NameKey = ([0xbc, 0x72, 0x1b, 0x0c, 0x9c, 0x7a, 0xa2, 0x26], 7);
pub const CLIP_UPPER_WALK: NameKey = ([0xf9, 0xc5, 0x3f, 0x25, 0x64, 0x31, 0xfb, 0x05], 7);
pub const CLIP_LOWER_STAND: NameKey = ([0x7f, 0x00, 0x7b, 0xf2, 0xff, 0x2b, 0xcc, 0x78], 3);
pub const CLIP_LOWER_WALK: NameKey = ([0x5f, 0x8e, 0x2d, 0x7e, 0x3a, 0xa7, 0xa1, 0x7d], 3);
pub const PLAYER_CLIPS: [NameKey; 4] = [
    CLIP_UPPER_STAND,
    CLIP_UPPER_WALK,
    CLIP_LOWER_STAND,
    CLIP_LOWER_WALK,
];

/// One `[Model<n>]` row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BodyRow {
    pub model: String,
    /// Per skin variant: (upper, lower).
    pub skins: BTreeMap<u32, (Option<String>, Option<String>)>,
    /// Render-style names in style-slot order.
    pub styles: Vec<String>,
    /// The row's hand identifier (`PVGunsModel`), when it carries one.
    pub pv_ident: Option<String>,
    /// The row's hand skin format strings (`HandsSkin<k>`).
    pub hands: Vec<String>,
    /// The clip the shop / inventory screen's character plays (`InvenShopAni`), when the row names one.
    // OBSERVED: ROM table key MODELBUTES.TXT [Model<n>] InvenShopAni; EVIDENCE: cshell+0x1e4760 (the model table
    // loader reads it into the row, default literal of the code when absent)
    pub shop_clip: Option<String>,
}

/// The body rows and team marks of the ROM.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CharacterTable {
    pub bodies: BTreeMap<u32, BodyRow>,
    /// `[Face<n>]`: model file and skins.
    pub faces: BTreeMap<u32, (String, Vec<String>)>,
    /// `[Face<n>]` render-style names in style-slot order.
    pub face_styles: BTreeMap<u32, Vec<String>>,
    pub marks: BTreeMap<u32, String>,
}

/// Numbered values `<key>0`, `<key>1`, ... of a row up to the first missing or empty one.
fn numbered(sec: &dc_bute::SectionMap, key: &str, max: u32) -> Vec<String> {
    (0..max)
        .map_while(|k| sec.get(&format!("{key}{k}")).map(|v| unquote(v)))
        .take_while(|v| !v.is_empty())
        .collect()
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').trim().to_string()
}

impl CharacterTable {
    pub fn from_docs(models: &ButeDoc, marks: Option<&ButeDoc>) -> CharacterTable {
        let mut t = CharacterTable::default();
        for (name, sec) in models.iter() {
            if let Some(rest) = name.strip_prefix(FACE_SECTION) {
                if let (Ok(n), Some(model)) = (rest.parse::<u32>(), sec.get(KEY_MODEL_FILE)) {
                    let skins = (0..8u32)
                        .map_while(|k| sec.get(&format!("{KEY_SKIN}{k}")).map(|v| unquote(v)))
                        .filter(|v| !v.is_empty())
                        .collect();
                    t.faces.insert(n, (unquote(model), skins));
                    t.face_styles.insert(n, numbered(sec, KEY_STYLE, 8));
                }
                continue;
            }
            let Some(rest) = name.strip_prefix(BODY_SECTION) else {
                continue;
            };
            if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let (Ok(n), Some(model)) = (rest.parse::<u32>(), sec.get(KEY_MODEL_FILE)) else {
                continue;
            };
            let mut skins = BTreeMap::new();
            for k in 0..64u32 {
                let up = sec.get(&format!("{KEY_UPPER}{k}")).map(|v| unquote(v));
                let low = sec.get(&format!("{KEY_LOWER}{k}")).map(|v| unquote(v));
                if up.is_some() || low.is_some() {
                    skins.insert(k, (up, low));
                }
            }
            t.bodies.insert(
                n,
                BodyRow {
                    model: unquote(model),
                    skins,
                    styles: numbered(sec, KEY_STYLE, 8),
                    pv_ident: sec
                        .get(KEY_PV_IDENT_OF_ROW)
                        .map(|v| unquote(v))
                        .filter(|v| !v.is_empty()),
                    hands: numbered(sec, KEY_HANDS_SKIN, 16),
                    shop_clip: sec
                        .get(KEY_SHOP_CLIP)
                        .map(|v| unquote(v))
                        .filter(|v| !v.is_empty()),
                },
            );
        }
        if let Some(sec) = marks.and_then(|d| d.get(MARK_SECTION)) {
            for k in 0..8u32 {
                if let Some(v) = sec.get(&format!("{KEY_MARK}{k}")) {
                    let v = unquote(v);
                    if !v.is_empty() {
                        t.marks.insert(k, v);
                    }
                }
            }
        }
        t
    }

    pub fn from_vfs(vfs: &RezVfs) -> Result<CharacterTable, String> {
        let models = match ButeDoc::try_from_vfs(vfs, TABLE_REZ, MODEL_TABLE) {
            Ok(Some(d)) => d,
            Ok(None) => return Err("model table missing".into()),
            Err(e) => return Err(format!("model table: {e}")),
        };
        let marks = ButeDoc::try_from_vfs(vfs, TABLE_REZ, MARK_TABLE)
            .ok()
            .flatten();
        Ok(CharacterTable::from_docs(&models, marks.as_ref()))
    }

    /// The body row whose model file is `path` (case-insensitive), if any.
    pub fn row_of_model(&self, path: &str) -> Option<(u32, &BodyRow)> {
        let want = path.replace('/', "\\");
        self.bodies
            .iter()
            .find(|(_, r)| r.model.replace('/', "\\").eq_ignore_ascii_case(&want))
            .map(|(n, r)| (*n, r))
    }

    /// The render-style names of the body or face row whose model file is `path` (case-insensitive).
    pub fn styles_of_model(&self, path: &str) -> Option<Vec<String>> {
        if let Some((_, r)) = self.row_of_model(path) {
            return Some(r.styles.clone());
        }
        let want = path.replace('/', "\\");
        self.faces
            .iter()
            .find(|(_, (m, _))| m.replace('/', "\\").eq_ignore_ascii_case(&want))
            .and_then(|(n, _)| self.face_styles.get(n).cloned())
    }

    /// Texture list of a body row's model: upper skin, lower skin of variant `skin` (variant 0 when absent),
    /// then team mark `mark` repeated [`MARK_REPEAT`] times.
    pub fn body_textures(&self, row: u32, skin: u32, mark: u32) -> Option<Vec<String>> {
        let r = self.bodies.get(&row)?;
        let (up, low) = r
            .skins
            .get(&skin)
            .or_else(|| r.skins.get(&0))
            .cloned()
            .unwrap_or((None, None));
        let mut v: Vec<String> = [up, low].into_iter().flatten().collect();
        if let Some(m) = self.marks.get(&mark) {
            v.extend(std::iter::repeat_n(m.clone(), MARK_REPEAT));
        }
        Some(v)
    }

    /// The skin variant of body row `row` whose upper skin is `upper` (case-insensitive), if any.
    pub fn skin_variant_of(&self, row: u32, upper: &str) -> Option<u32> {
        let want = upper.replace('/', "\\");
        self.bodies.get(&row)?.skins.iter().find_map(|(k, (u, _))| {
            u.as_ref()
                .filter(|u| u.replace('/', "\\").eq_ignore_ascii_case(&want))
                .map(|_| *k)
        })
    }

    /// The hand skin of body row `row`: its `HandsSkin<index>` format string with the decimal field filled with
    /// `variant` (the 2009 client passes one byte); `None` when the row or the entry is missing.
    // EVIDENCE: cshell+0x1e3180 (row bound check, entry `index & 0xff` of the row's list, `sprintf(format, byte)`);
    // the index first goes through a redirect table (+0x15e8, entry stride 0x88, +0x7c, -1 = keep), which DeCombat
    // does not read (CLI-107)
    pub fn hands_skin(&self, row: u32, index: u32, variant: u8) -> Option<String> {
        let fmt = self.bodies.get(&row)?.hands.get(index as usize)?;
        Some(fill_decimal(fmt, variant as u32))
    }
}

/// `fmt` with its first `%d` / `%0<w>d` field replaced by `value` (zero-padded to `w`); `%%` gives one `%`; other
/// text is kept. The C `sprintf` subset the hand skin strings use.
pub fn fill_decimal(fmt: &str, value: u32) -> String {
    let mut out = String::with_capacity(fmt.len() + 4);
    let mut done = false;
    let mut it = fmt.chars().peekable();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        if it.peek() == Some(&'%') {
            it.next();
            out.push('%');
            continue;
        }
        let mut spec = String::new();
        while let Some(&d) = it.peek() {
            if d.is_ascii_digit() {
                spec.push(d);
                it.next();
            } else {
                break;
            }
        }
        match it.peek() {
            Some(&'d') | Some(&'u') | Some(&'i') if !done => {
                it.next();
                let width: usize = spec.trim_start_matches('0').parse().unwrap_or(0);
                if spec.starts_with('0') {
                    out.push_str(&format!("{value:0width$}"));
                } else {
                    out.push_str(&format!("{value:width$}"));
                }
                done = true;
            }
            _ => {
                out.push('%');
                out.push_str(&spec);
            }
        }
    }
    out
}

/// Skin-list placeholders of the first-person weapon rows, by name key: skin 0 naming the character's hand skin,
/// and two arm entries the 2009 client replaces by its "no texture" file.
// EVIDENCE: cshell+0x23b6a0 (first-person model create data: model +0x4c, 32 skins of 0x80 bytes at +0x104c; skin 0
// compared with a 6-byte compare, i.e. the whole name and its NUL, then replaced by the row's hand skin
// cshell+0x1e3180; then every skin equal to one of two names replaced by a fixed texture name); DISASM:
// cshell+0x23b880 (the skin-0 name at VA 0x374f1b04), cshell+0x23b95e / +0x23b966 (the two arm names at VA 0x374f1ae4 /
// 0x374f1ad8), cshell+0x23b956 (the replacement name at VA 0x374f1af0). Names identified by SHA-256 key only.
pub const PV_HANDS_KEY: NameKey = ([0x6b, 0x72, 0x04, 0x77, 0x86, 0x86, 0x14, 0x5f], 5);
pub const PV_ARM_KEYS: [NameKey; 2] = [
    ([0xf9, 0xab, 0x7a, 0xeb, 0xcb, 0x94, 0xd1, 0x8f], 8),
    ([0x02, 0xa6, 0x79, 0x02, 0xb2, 0xe1, 0x52, 0xb2], 8),
];
/// Where the replacement name of the arm placeholders sits in the user's CShell.dll (RVA) and its name key.
// DISASM: cshell+0x23b956 (`push` of VA 0x374f1af0, image base 0x37100000)
pub const PV_NONE_TEXTURE_RVA: u32 = 0x3f1af0;
pub const PV_NONE_TEXTURE_KEY: NameKey = ([0xaf, 0xe7, 0x8f, 0x58, 0x20, 0x5e, 0x47, 0x63], 19);
/// The hand identifier a body row without `PVGunsModel` gets (the reader's default value), by name key.
// EVIDENCE: cshell+0x1e4760 (three reads of the key with one default literal)
pub const PV_DEFAULT_IDENT_KEY: NameKey = ([0x7f, 0x13, 0xc1, 0xfc, 0xbd, 0xba, 0xe1, 0x62], 8);

/// The replacement texture name of the arm placeholders, read from the user's CShell.dll by RVA and accepted only
/// when its name key matches (nothing of the ROM in the tree).
pub fn pv_none_texture(rom: &std::path::Path) -> Option<String> {
    let pe = dc_rom::PeFile::from_file(&rom.join("Game").join("CShell.dll")).ok()?;
    let s = pe.cstr(PV_NONE_TEXTURE_RVA).ok()?;
    (name_key(s.as_bytes()) == PV_NONE_TEXTURE_KEY).then_some(s)
}

/// The first-person skin list as the 2009 client resolves it: skin 0 equal to the hands placeholder becomes `hands`
/// (empty when the character has none), then every skin equal to an arm placeholder becomes `none_texture`
/// (empty when not known).
// EVIDENCE: cshell+0x23b6a0 (order: hands first, then the arm names over all 32 skins)
pub fn resolve_view_skins(
    skins: &[String],
    hands: Option<&str>,
    none_texture: Option<&str>,
) -> Vec<String> {
    skins
        .iter()
        .enumerate()
        .map(|(k, s)| {
            let key = name_key(s.as_bytes());
            if k == 0 && key == PV_HANDS_KEY {
                hands.unwrap_or_default().to_string()
            } else if PV_ARM_KEYS.contains(&key) {
                none_texture.unwrap_or_default().to_string()
            } else {
                s.clone()
            }
        })
        .collect()
}

/// The third-person model of one weapon row: model file, skins, render styles.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HandModel {
    pub model: String,
    pub skins: Vec<String>,
    pub styles: Vec<String>,
}

/// The `[Weapon<n>]` rows of the ROM weapon table that name a third-person model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HandModels {
    pub rows: BTreeMap<u32, HandModel>,
}

impl HandModels {
    pub fn from_doc(doc: &ButeDoc) -> HandModels {
        let mut rows = BTreeMap::new();
        for (name, sec) in doc.iter() {
            let Some(rest) = name.strip_prefix(WEAPON_SECTION) else {
                continue;
            };
            if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let (Ok(n), Some(model)) = (rest.parse::<u32>(), sec.get(KEY_HH_MODEL)) else {
                continue;
            };
            let model = unquote(model);
            if model.is_empty() {
                continue;
            }
            rows.insert(
                n,
                HandModel {
                    model,
                    skins: numbered(sec, KEY_HH_SKIN, 8),
                    styles: numbered(sec, KEY_HH_STYLE, 8),
                },
            );
        }
        HandModels { rows }
    }

    pub fn from_vfs(vfs: &RezVfs) -> Result<HandModels, String> {
        match ButeDoc::try_from_vfs(vfs, TABLE_REZ, WEAPON_TABLE) {
            Ok(Some(d)) => Ok(HandModels::from_doc(&d)),
            Ok(None) => Err("weapon table missing".into()),
            Err(e) => Err(format!("weapon table: {e}")),
        }
    }
}

/// The first-person model of one weapon row: model file (for the hand identifier chosen), skins, render styles.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ViewModel {
    pub model: String,
    pub skins: Vec<String>,
    pub styles: Vec<String>,
}

/// The `[Weapon<n>]` rows of the ROM weapon table that name a first-person model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ViewModels {
    /// Per row the model of the first hand identifier the row carries (CLI-34; the fallback of [`ViewModels::pick`]).
    pub rows: BTreeMap<u32, ViewModel>,
    /// The hand identifiers of `[PVModelIdentifiers]` in table order.
    pub idents: Vec<String>,
    /// Per row: the unsuffixed entry (`PVModel`, `PVSkin<k>`, `PVRenderStyle<k>`) and per hand identifier its own
    /// (`PVModel<hand>`, `PVSkin<hand><k>`, `PVRenderStyle<hand><k>`; empty fields where the row has none).
    pub choices: BTreeMap<u32, ViewChoices>,
}

/// The unsuffixed first-person entry of a weapon row and the entries of its hand identifiers.
pub type ViewChoices = (ViewModel, BTreeMap<String, ViewModel>);

impl ViewModels {
    /// The rows of a parsed weapon table: per row the `PVModel<hand>` of the first hand identifier of
    /// `[PVModelIdentifiers]` the row carries.
    // DC-DEFAULT: CLI-34 (proposed), see DC_DEFAULTS.md (the first hand identifier of the table; which one the 2009
    // client picks per character is not read)
    pub fn from_doc(doc: &ButeDoc) -> ViewModels {
        let idents: Vec<String> = doc
            .get(PV_IDENT_SECTION)
            .map(|s| numbered(s, KEY_PV_IDENT, 32))
            .unwrap_or_default();
        let mut rows = BTreeMap::new();
        let mut choices = BTreeMap::new();
        for (name, sec) in doc.iter() {
            let Some(rest) = name.strip_prefix(WEAPON_SECTION) else {
                continue;
            };
            if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let Ok(n) = rest.parse::<u32>() else {
                continue;
            };
            // EVIDENCE: cshell+0x174840 (per hand identifier: `PVModel` + id, `PVSkin` + id, `PVRenderStyle` + id; no
            // id: the plain keys), cshell+0x17bd20 (called once per identifier of the list cshell+0x17bb70 reads)
            let entry = |suffix: &str| ViewModel {
                model: sec
                    .get(&format!("{KEY_PV_MODEL_PREFIX}{suffix}"))
                    .map(|v| unquote(v))
                    .unwrap_or_default(),
                skins: numbered(sec, &format!("{KEY_PV_SKIN}{suffix}"), 32),
                styles: numbered(sec, &format!("{KEY_PV_STYLE}{suffix}"), 16),
            };
            let per: BTreeMap<String, ViewModel> = idents
                .iter()
                .map(|i| (i.clone(), entry(i)))
                .filter(|(_, e)| !e.model.is_empty() || !e.skins.is_empty() || !e.styles.is_empty())
                .collect();
            let plain = entry("");
            if !per.is_empty() || !plain.model.is_empty() {
                choices.insert(n, (plain, per));
            }
            let model = idents.iter().find_map(|i| {
                let m = unquote(sec.get(&format!("{KEY_PV_MODEL_PREFIX}{i}"))?);
                (!m.is_empty()).then_some(m)
            });
            let Some(model) = model else {
                continue;
            };
            rows.insert(
                n,
                ViewModel {
                    model,
                    skins: numbered(sec, KEY_PV_SKIN, 32),
                    styles: numbered(sec, KEY_PV_STYLE, 16),
                },
            );
        }
        ViewModels {
            rows,
            idents,
            choices,
        }
    }

    /// The first-person model of weapon row `row` for a character with hand identifier `ident` (`None`: the
    /// body row has no `PVGunsModel`, which the 2009 reader defaults to one identifier, [`PV_DEFAULT_IDENT_KEY`]):
    /// the model of the identifier when it names one, else the unsuffixed model; the skins and styles of the
    /// identifier when it has any, else the unsuffixed lists. Falls back to [`ViewModels::rows`] (CLI-34) when
    /// neither names a model.
    // EVIDENCE: cshell+0x23b6a0 (model from the weapon data +0x58, replaced by the identifier entry's model when it is
    // not empty; skins from +0x174 unless the identifier entry has its own; the identifier = the character row's
    // +0xa8, cshell+0x1e0f00), cshell+0x1e4760 (default identifier)
    pub fn pick(&self, row: u32, ident: Option<&str>) -> Option<ViewModel> {
        let fallback = || self.rows.get(&row).cloned();
        let Some((plain, per)) = self.choices.get(&row) else {
            return fallback();
        };
        let ident = match ident {
            Some(i) => Some(i.to_string()),
            None => self
                .idents
                .iter()
                .find(|i| name_key(i.as_bytes()) == PV_DEFAULT_IDENT_KEY)
                .cloned(),
        };
        let own = ident.as_ref().and_then(|i| per.get(i));
        let model = own
            .map(|e| e.model.clone())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| plain.model.clone());
        if model.is_empty() {
            return fallback();
        }
        let skins = own
            .map(|e| e.skins.clone())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| plain.skins.clone());
        let styles = own
            .map(|e| e.styles.clone())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| plain.styles.clone());
        Some(ViewModel {
            model,
            skins,
            styles,
        })
    }

    pub fn from_vfs(vfs: &RezVfs) -> Result<ViewModels, String> {
        match ButeDoc::try_from_vfs(vfs, TABLE_REZ, WEAPON_TABLE) {
            Ok(Some(d)) => Ok(ViewModels::from_doc(&d)),
            Ok(None) => Err("weapon table missing".into()),
            Err(e) => Err(format!("weapon table: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // a synthetic table with the key shape of the ROM tables (made-up values)
    const SYNTH: &str = "[Model0]\nModelFile = \"B\\b0.ltb\"\nUpperSkin0 = \"S\\u0.dtx\"\nLowerSkin0 = \"S\\l0.dtx\"\n\
                         [Model3]\nModelFile = \"B\\b3.ltb\"\n[Face0]\nModelFile = \"F\\f.ltb\"\n";
    const MARKS: &str = "[TeamMark]\nTeamName0 = \"S\\m.dtx\"\n";

    #[test]
    fn styles_and_hand_models() {
        let t = CharacterTable::from_docs(
            &ButeDoc::parse(
                "[Model0]\nModelFile = \"B\\b0.ltb\"\nRenderStyle0 = \"R\\a.ltb\"\nRenderStyle1 = \"R\\b.ltb\"\n\
                 [Face2]\nModelFile = \"F\\f.ltb\"\nSkin0 = \"S\\f.dtx\"\nRenderStyle0 = \"R\\c.ltb\"\n",
            ),
            None,
        );
        assert_eq!(t.bodies[&0].styles.len(), 2);
        assert_eq!(t.styles_of_model("b/B0.ltb").unwrap().len(), 2);
        assert_eq!(t.styles_of_model("F\\F.LTB").unwrap().len(), 1);
        assert_eq!(t.styles_of_model("x.ltb"), None);
        let w = HandModels::from_doc(&ButeDoc::parse(
            "[Weapon0]\nName = \"A\"\n[Weapon3]\nHHModel = \"H\\g.ltb\"\nHHSkin0 = \"H\\g.dtx\"\nHHRenderStyle0 = \"R\\a.ltb\"\n",
        ));
        assert_eq!(w.rows.len(), 1);
        assert_eq!(w.rows[&3].skins.len(), 1);
        assert_eq!(w.rows[&3].styles.len(), 1);
    }

    #[test]
    fn first_person_rows() {
        // made-up identifiers and paths with the key shape of the ROM table
        let v = ViewModels::from_doc(&ButeDoc::parse(
            "[PVModelIdentifiers]\nName0 = \"_A\"\nName1 = \"_B\"\n\
             [Weapon1]\nPVModel_B = \"P/b.ltb\"\nPVModel_A = \"P/a.ltb\"\nPVSkin0 = \"P/s.dtx\"\n\
             PVSkin1 = \"P/h.dtx\"\nPVRenderStyle0 = \"R/x.ltb\"\n\
             [Weapon2]\nPVModel_B = \"P/c.ltb\"\n[Weapon3]\nHHModel = \"H/g.ltb\"\n",
        ));
        assert_eq!(v.rows.len(), 2);
        assert_eq!(v.rows[&1].model, "P/a.ltb");
        assert_eq!((v.rows[&1].skins.len(), v.rows[&1].styles.len()), (2, 1));
        assert_eq!(v.rows[&2].model, "P/c.ltb");
    }

    #[test]
    fn first_person_pick_by_hand_identifier() {
        // made-up identifiers and paths with the key shape of the ROM table
        let v = ViewModels::from_doc(&ButeDoc::parse(
            "[PVModelIdentifiers]\n\
             Name0 = \"_A\"\n\
             Name1 = \"_B\"\n\
             Name2 = \"_C\"\n\
             [Weapon1]\n\
             PVModel_A = \"P/a.ltb\"\n\
             PVModel_B = \"P/b.ltb\"\n\
             PVSkin0 = \"P/s.dtx\"\n\
             PVSkin_B0 = \"P/own.dtx\"\n\
             PVRenderStyle0 = \"R/x.ltb\"\n\
             [Weapon2]\n\
             PVModel = \"P/plain.ltb\"\n\
             PVModel_C = \"P/c.ltb\"\n",
        ));
        assert_eq!(v.idents.len(), 3);
        let a = v.pick(1, Some("_A")).unwrap();
        assert_eq!(
            (a.model.as_str(), a.skins.len(), a.styles.len()),
            ("P/a.ltb", 1, 1)
        );
        let b = v.pick(1, Some("_B")).unwrap();
        assert_eq!(b.model, "P/b.ltb");
        assert_eq!(
            b.skins,
            vec!["P/own.dtx".to_string()],
            "the identifier's own skins win"
        );
        assert_eq!(b.styles.len(), 1, "no own styles: the plain list");
        // an identifier the row lacks: the unsuffixed model, else the first-identifier fallback (CLI-34)
        assert_eq!(v.pick(2, Some("_A")).unwrap().model, "P/plain.ltb");
        assert_eq!(v.pick(2, Some("_C")).unwrap().model, "P/c.ltb");
        assert_eq!(v.pick(1, Some("_C")).unwrap().model, "P/a.ltb");
        assert!(v.pick(9, Some("_A")).is_none());
    }

    #[test]
    fn hand_skins_and_decimal_fields() {
        assert_eq!(fill_decimal("H/x_%02d.dtx", 3), "H/x_03.dtx");
        assert_eq!(fill_decimal("H/x_%d.dtx", 12), "H/x_12.dtx");
        assert_eq!(fill_decimal("a%%b%03dc%d", 7), "a%b007c%d");
        assert_eq!(fill_decimal("plain", 1), "plain");
        let t = CharacterTable::from_docs(
            &ButeDoc::parse(
                "[Model0]\n\
                 ModelFile = \"B/b0.ltb\"\n\
                 UpperSkin0 = \"S/u0.dtx\"\n\
                 UpperSkin4 = \"S/u4.dtx\"\n\
                 PVGunsModel = \"_B\"\n\
                 HandsSkin0 = \"H/h0_%02d.dtx\"\n\
                 HandsSkin1 = \"H/h1_%02d.dtx\"\n\
                 [Model1]\n\
                 ModelFile = \"B/b1.ltb\"\n",
            ),
            None,
        );
        assert_eq!(t.bodies[&0].pv_ident.as_deref(), Some("_B"));
        assert_eq!(t.bodies[&1].pv_ident, None);
        assert_eq!(t.hands_skin(0, 1, 4).as_deref(), Some("H/h1_04.dtx"));
        assert_eq!(t.hands_skin(0, 2, 0), None);
        assert_eq!(t.hands_skin(1, 0, 0), None);
        assert_eq!(t.skin_variant_of(0, "s/U4.DTX"), Some(4));
        assert_eq!(t.skin_variant_of(0, "s/none.dtx"), None);
        // names that are no placeholder pass through unchanged
        let skins = vec!["P/a.dtx".to_string(), "P/b.dtx".to_string()];
        assert_eq!(
            resolve_view_skins(&skins, Some("H/h.dtx"), Some("T/n.dtx")),
            skins
        );
    }

    #[test]
    fn rows_and_textures() {
        let t = CharacterTable::from_docs(&ButeDoc::parse(SYNTH), Some(&ButeDoc::parse(MARKS)));
        assert_eq!(t.bodies.len(), 2);
        assert_eq!(t.faces.len(), 1);
        let tex = t.body_textures(0, 5, 0).unwrap();
        assert_eq!(tex.len(), 2 + MARK_REPEAT);
        assert_eq!(t.row_of_model("b/b3.LTB").map(|x| x.0), Some(3));
        assert_eq!(t.body_textures(3, 0, 0).unwrap().len(), MARK_REPEAT);
        assert!(t.body_textures(9, 0, 0).is_none());
        assert_eq!(name_key(b"abc").1, 3);
    }
}
