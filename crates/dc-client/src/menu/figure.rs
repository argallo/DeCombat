//! shop-model: what the shop's character wears, from the equipped items. The 2009 shop keeps the item ids of the
//! equipment in the player's look record and builds the character from the model table rows they name: the vest (or
//! the character item) picks the body row, the uniform its skin variant, the head item the head-gear row, the face
//! attachment the goggle row, the backpack item the backpack row; the held weapon is the first weapon slot's.
//!
//! The rows are read from the user's ROM at run time (model table, attachment table, weapon table).
// EVIDENCE: cshell+0x14e410 (equip / preview of item id + type: the item's own id stored per type: head +0x180, face
// attachment +0x174, face +0x154, uniform +0x184, vest +0x17c, backpack +0x170, character 0x16 / 0x1e +0x18c),
// cshell+0x1d32e0 (body row = the character id when set, else the vest row's FaceRelationModel<face> when that is set,
// else the vest row), cshell+0x1e40f0, cshell+0x1e2f10 (skins: UpperSkin<v> / LowerSkin<v> with v = the uniform id
// when below the row's count, else 0), cshell+0x1e3430 (face = the body row's FaceIndex when set, else the look's),
// cshell+0x1e3b00 (a head-gear row without a model file stands for the face row's HairIndex row), cshell+0x1e3d60
// (head-gear model = ModelFile<figure type>), cshell+0x1e3dd0 (head-gear skin: the figure type when
// SkinForFiguretype is set, else the uniform id, 0 when out of range), cshell+0x1dfc0a (goggle and backpack rows of
// the attachment table: model, skin, style, socket), cshell+0x12cc60 (the inventory character starts from head 0,
// uniform 0, vest 0, no backpack, no goggles)

use std::collections::BTreeMap;
use std::path::Path;

use dc_bute::{ButeDoc, SectionMap};

use super::modelview::CharLook;
use crate::characters::BodyPiece;
use crate::world::ModelRef;

/// The equipment slots the look reads.
pub const SLOT_WEAPON: usize = 0;
pub const SLOT_HEAD: usize = 4;
pub const SLOT_FACE: usize = 5;
pub const SLOT_GOGGLE: usize = 6;
pub const SLOT_UNIFORM: usize = 7;
pub const SLOT_VEST: usize = 8;
pub const SLOT_BACKPACK: usize = 9;
pub const SLOT_CHARACTER: usize = 14;

/// The item number ranges of the slots (the item's own id = number - base).
// SPEC: PROTOCOL_2009.md §4.5 (item number ranges: 2000 0xe, 3000 0xf, 4000 0xb, 5000 0xc, 6000 0xd, 7000 0x10,
// 10000 0x1e)
const BASES: [(usize, i32); 7] = [
    (SLOT_HEAD, 4000),
    (SLOT_FACE, 6000),
    (SLOT_GOGGLE, 5000),
    (SLOT_UNIFORM, 2000),
    (SLOT_VEST, 3000),
    (SLOT_BACKPACK, 7000),
    (SLOT_CHARACTER, 10000),
];

/// The look record's starting values (head, uniform, vest, backpack, goggles).
// EVIDENCE: cshell+0x12cc60 (head 0, uniform 0, vest 0, backpack -1, goggles -1)
const START_HEAD: i32 = 0;
const START_UNIFORM: i32 = 0;
const START_VEST: i32 = 0;
/// The face when no face item is equipped.
// DC-DEFAULT: CLI-743 (proposed) face row 0 for a look without an account (CLI-853: the account's own face row is
// [`LookIds::with_account_face`])
const START_FACE: i32 = 0;
/// The team mark the body's mark pieces get (the default character's, CLI-105).
const MARK: u32 = crate::characters::DEFAULT_MARK;

/// The equipped item ids of the look record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LookIds {
    pub character: Option<i32>,
    pub vest: i32,
    pub uniform: i32,
    pub face: i32,
    pub head: i32,
    pub goggle: Option<i32>,
    pub backpack: Option<i32>,
    pub weapon: Option<i32>,
}

impl LookIds {
    /// The look of an equipment list (item number per slot, -1 empty).
    pub fn from_equipped(eq: &[i32; 15]) -> LookIds {
        LookIds::with_account_face(eq, START_FACE)
    }

    /// The look of an equipment list for an account whose own face row is `account_face` (the face model number of
    /// its character record less the 6000 base, or the default when it names none): a face item in the face slot
    /// still wins.
    // EVIDENCE: cshell+0x1e3430 (face = the body row's FaceIndex when set, else the look's own face), cshell+0x14e410
    // (the look's face +0x154 is the character's; an equipped face item replaces it)
    pub fn with_account_face(eq: &[i32; 15], account_face: i32) -> LookIds {
        let id = |slot: usize| -> Option<i32> {
            let n = *eq.get(slot)?;
            let base = BASES.iter().find(|(s, _)| *s == slot)?.1;
            (n >= base && n < base + 1000).then_some(n - base)
        };
        LookIds {
            character: id(SLOT_CHARACTER),
            vest: id(SLOT_VEST).unwrap_or(START_VEST),
            uniform: id(SLOT_UNIFORM).unwrap_or(START_UNIFORM),
            face: id(SLOT_FACE).unwrap_or(account_face),
            head: id(SLOT_HEAD).unwrap_or(START_HEAD),
            goggle: id(SLOT_GOGGLE),
            backpack: id(SLOT_BACKPACK),
            weapon: eq
                .get(SLOT_WEAPON)
                .copied()
                .filter(|n| (0..1000).contains(n)),
        }
    }
}

/// The gear a match character wears (CLI-1231): the look record's head gear, uniform, goggle and backpack ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GearIds {
    pub head: i32,
    pub uniform: i32,
    pub goggle: Option<i32>,
    pub backpack: Option<i32>,
}

/// The look-block bytes of the gear (`GS_PLAYER_INFO` field 6): head gear, uniform, backpack, goggle; -1 = none.
// EVIDENCE: cshell+0x1c2a90 (bytes 8..12 -> look +0x18 .. +0x28), cshell+0x1c2230 (the look's start: +0x18 / +0x1c /
// +0x20 = 0, +0x24 / +0x28 = -1), cshell+0x12cc60 (the own look record's start: head 0, uniform 0, vest 0, backpack
// -1, goggle -1, in that order); the item each look field holds is PLAUSIBLE from the matching order and defaults
// DC-DEFAULT: CLI-1231 (proposed) byte 7 head gear, 8 uniform, 10 backpack, 11 goggle (as DSV-341 writes them)
pub const LOOK_HEAD: usize = 7;
pub const LOOK_UNIFORM: usize = 8;
pub const LOOK_BACKPACK: usize = 10;
pub const LOOK_GOGGLE: usize = 11;

impl GearIds {
    /// The gear of a roster look block (a byte is an item id 0..255 read unsigned; -1 = none).
    pub fn from_look(look: &[i8; 15]) -> GearIds {
        let id = |i: usize| (look[i] != -1).then_some(i32::from(look[i] as u8));
        GearIds {
            head: id(LOOK_HEAD).unwrap_or(START_HEAD),
            uniform: id(LOOK_UNIFORM).unwrap_or(START_UNIFORM),
            goggle: id(LOOK_GOGGLE),
            backpack: id(LOOK_BACKPACK),
        }
    }
}

impl Default for GearIds {
    fn default() -> Self {
        GearIds {
            head: START_HEAD,
            uniform: START_UNIFORM,
            goggle: None,
            backpack: None,
        }
    }
}

fn unquote(v: &str) -> String {
    let v = v.split("//").next().unwrap_or("").trim();
    v.trim_matches('"').trim().to_string()
}

fn int(sec: &SectionMap, key: &str) -> Option<i32> {
    sec.get(key).and_then(|v| unquote(v).parse().ok())
}

fn numbered(sec: &SectionMap, key: &str) -> Vec<String> {
    (0..64)
        .map_while(|k| sec.get(&format!("{key}{k}")).map(|v| unquote(v)))
        .collect()
}

/// A `[Model<n>]` row's look keys.
// OBSERVED: ROM table key MODELBUTES.TXT [Model<n>] FaceRelationModel<k>, FaceIndex, FigureType
#[derive(Debug, Clone, Default)]
struct BodyLook {
    face_relation: Vec<i32>,
    face_index: Option<i32>,
    figure: usize,
    /// The row's light set of the shop screen (`ModelLight_ScreenShopInven`).
    light_set: Option<String>,
}

/// A `[Hair<n>]` row.
// OBSERVED: ROM table key MODELBUTES.TXT [Hair<n>] ModelFile<k>, Skin<k>, RenderStyle<k>, SkinForFiguretype
#[derive(Debug, Clone, Default)]
struct HeadRow {
    models: Vec<String>,
    skins: Vec<String>,
    styles: Vec<String>,
    skin_by_figure: bool,
}

/// A `[Goggle<n>]` / `[Backpack<n>]` row of the attachment table.
// OBSERVED: ROM table key ATTACHMENTS.TXT [Goggle<n>] / [Backpack<n>] Model<k>, Skin0, RenderStyle0, Socket
#[derive(Debug, Clone, Default)]
struct AttachRow {
    models: Vec<String>,
    skins: Vec<String>,
    styles: Vec<String>,
    socket: String,
}

/// The ROM tables the look is built from.
#[derive(Debug, Clone, Default)]
pub struct FigureTables {
    chars: dc_render::CharacterTable,
    bodies: BTreeMap<i32, BodyLook>,
    /// `[Face<n>] HairIndex`.
    face_hair: BTreeMap<i32, i32>,
    heads: BTreeMap<i32, HeadRow>,
    goggles: BTreeMap<i32, AttachRow>,
    backpacks: BTreeMap<i32, AttachRow>,
    hands: dc_render::HandModels,
    /// The interface lights by name and the light sets (section -> light names).
    lights: BTreeMap<String, ShopLight>,
    light_sets: BTreeMap<String, Vec<String>>,
}

/// An interface light (`[Light<n>]` of the layout table): position in the interface camera's frame, colour 0..255,
/// radius.
// OBSERVED: ROM table key LAYOUT.TXT [Light<n>] Name, Pos, Color, Radius; light sets [<set>] Light<k> = <name>
// EVIDENCE: cshell+0x1c7be0 (a light record: Name, Pos, Color / 255, Radius)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShopLight {
    pub pos: [f32; 3],
    pub colour: [f32; 3],
    pub radius: f32,
}

/// The shop screen's own light set (a body row without one).
// OBSERVED: ROM table key LAYOUT.TXT [ScreenShopInven] Light0
pub const SCREEN_LIGHT_SET: &str = "ScreenShopInven";

fn vec3(v: &str) -> Option<[f32; 3]> {
    let v = unquote(v);
    let v = v.trim_start_matches('<').trim_end_matches('>');
    let mut it = v.split(',').map(|x| x.trim().parse::<f32>().ok());
    Some([it.next()??, it.next()??, it.next()??])
}

const REZ: &str = "ATTRIBUTES.rez";
const MODEL_TABLE: &str = "ATTRIBUTES\\MODELBUTES.TXT";
const ATTACH_TABLE: &str = "ATTRIBUTES\\ATTACHMENTS.TXT";
const LAYOUT_TABLE: &str = "ATTRIBUTES\\LAYOUT.TXT";

fn rows<T>(doc: &ButeDoc, prefix: &str, f: impl Fn(&SectionMap) -> T) -> BTreeMap<i32, T> {
    doc.iter()
        .filter_map(|(name, sec)| {
            let rest = name.strip_prefix(prefix)?;
            let n: i32 = rest.parse().ok()?;
            Some((n, f(sec)))
        })
        .collect()
}

fn attach_row(sec: &SectionMap) -> AttachRow {
    AttachRow {
        models: numbered(sec, "Model"),
        skins: numbered(sec, "Skin"),
        styles: numbered(sec, "RenderStyle"),
        socket: sec.get("Socket").map(|v| unquote(v)).unwrap_or_default(),
    }
}

impl FigureTables {
    /// [`FigureTables::from_rom`], read once per ROM folder and process.
    pub fn cached(rom: &Path) -> std::sync::Arc<FigureTables> {
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::sync::{Arc, Mutex, OnceLock};
        static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<FigureTables>>>> = OnceLock::new();
        let mut c = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        c.entry(rom.to_path_buf())
            .or_insert_with(|| Arc::new(FigureTables::from_rom(rom)))
            .clone()
    }

    pub fn from_rom(rom: &Path) -> FigureTables {
        let vfs = dc_rez::RezVfs::new(rom);
        let mut t = FigureTables {
            chars: dc_render::CharacterTable::from_vfs(&vfs).unwrap_or_default(),
            hands: dc_render::HandModels::from_vfs(&vfs).unwrap_or_default(),
            ..FigureTables::default()
        };
        if let Ok(Some(doc)) = ButeDoc::try_from_vfs(&vfs, REZ, MODEL_TABLE) {
            t.bodies = rows(&doc, "Model", |sec| BodyLook {
                face_relation: (0..64)
                    .map_while(|k| int(sec, &format!("FaceRelationModel{k}")))
                    .collect(),
                face_index: int(sec, "FaceIndex").filter(|v| *v >= 0),
                figure: int(sec, "FigureType").unwrap_or(0).max(0) as usize,
                light_set: sec
                    .get("ModelLight_ScreenShopInven")
                    .map(|v| unquote(v))
                    .filter(|v| !v.is_empty()),
            });
            t.face_hair = rows(&doc, "Face", |sec| int(sec, "HairIndex"))
                .into_iter()
                .filter_map(|(k, v)| Some((k, v?)))
                .collect();
            t.heads = rows(&doc, "Hair", |sec| HeadRow {
                models: numbered(sec, "ModelFile"),
                skins: numbered(sec, "Skin"),
                styles: numbered(sec, "RenderStyle"),
                skin_by_figure: int(sec, "SkinForFiguretype").unwrap_or(0) != 0,
            });
        }
        if let Ok(Some(doc)) = ButeDoc::try_from_vfs(&vfs, REZ, LAYOUT_TABLE) {
            for (_, sec) in rows(&doc, "Light", |sec| sec.clone()) {
                let (Some(name), Some(pos)) = (
                    sec.get("Name").map(|v| unquote(v)),
                    sec.get("Pos").and_then(|v| vec3(v)),
                ) else {
                    continue;
                };
                let colour = sec.get("Color").and_then(|v| vec3(v)).unwrap_or([0.0; 3]);
                let radius = sec
                    .get("Radius")
                    .and_then(|v| unquote(v).parse().ok())
                    .unwrap_or(0.0);
                t.lights.insert(
                    name.to_ascii_uppercase(),
                    ShopLight {
                        pos,
                        colour,
                        radius,
                    },
                );
            }
            for (name, sec) in doc.iter() {
                let names = numbered(sec, "Light");
                if !names.is_empty() && !name.starts_with("Light") {
                    t.light_sets.insert(name.to_ascii_uppercase(), names);
                }
            }
        }
        if let Ok(Some(doc)) = ButeDoc::try_from_vfs(&vfs, REZ, ATTACH_TABLE) {
            t.goggles = rows(&doc, "Goggle", attach_row);
            t.backpacks = rows(&doc, "Backpack", attach_row);
        }
        t
    }

    /// The body row of a look.
    // EVIDENCE: cshell+0x1d32e0, cshell+0x1e40f0
    /// CRATE-8: the third-person model of weapon item `n` (a variant: its base gun with its skins), as the case reel's
    /// showcase draws it.
    pub fn weapon_model(&self, n: i32) -> Option<ModelRef> {
        let h = crate::camo::hand_model(&self.hands, u32::try_from(n).ok()?)?;
        Some(ModelRef {
            model: h.model,
            textures: h.skins,
        })
    }

    pub fn body_row(&self, ids: &LookIds) -> i32 {
        if let Some(c) = ids
            .character
            .filter(|c| self.chars.bodies.contains_key(&(*c as u32)))
        {
            return c;
        }
        self.bodies
            .get(&ids.vest)
            .and_then(|b| b.face_relation.get(ids.face.max(0) as usize))
            .copied()
            .filter(|m| *m >= 0)
            .unwrap_or(ids.vest)
    }

    /// The interface lights of a look: the body row's shop light set, else the screen's.
    // DC-DEFAULT: CLI-744 (proposed) the row's set when it names one, else [ScreenShopInven] (which set the 2009
    // screen switches on is not traced)
    pub fn lights(&self, ids: &LookIds) -> Vec<ShopLight> {
        let row = self.body_row(ids);
        let set = self
            .bodies
            .get(&row)
            .and_then(|b| b.light_set.clone())
            .unwrap_or_else(|| SCREEN_LIGHT_SET.to_string());
        let names = self
            .light_sets
            .get(&set.to_ascii_uppercase())
            .or_else(|| self.light_sets.get(&SCREEN_LIGHT_SET.to_ascii_uppercase()));
        names
            .map(|v| {
                v.iter()
                    .filter_map(|n| self.lights.get(&n.to_ascii_uppercase()).copied())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The gear pieces of a look: head gear (a row without a model file stands for the face's own hair row), goggles
    /// and backpack, for a body of figure type `figure`, skin variant `variant` and face row `face`.
    // EVIDENCE: cshell+0x1e3b00, cshell+0x1e3d60, cshell+0x1e3dd0, cshell+0x1dfc0a (see the module head)
    pub fn gear_of(
        &self,
        figure: usize,
        variant: u32,
        face: i32,
        head: i32,
        goggle: Option<i32>,
        backpack: Option<i32>,
    ) -> Vec<BodyPiece> {
        let mut pieces = Vec::new();
        // head gear: a row without a model file stands for the face's own hair row
        let head = self.heads.get(&head).and_then(|h| {
            if h.models.first().is_none_or(|m| m.is_empty()) {
                self.face_hair.get(&face).and_then(|i| self.heads.get(i))
            } else {
                Some(h)
            }
        });
        if let Some(h) = head {
            let model = h
                .models
                .get(figure)
                .or(h.models.first())
                .filter(|m| !m.is_empty());
            if let Some(model) = model {
                let k = if h.skin_by_figure {
                    figure
                } else {
                    variant as usize
                };
                let skin = h.skins.get(k).or(h.skins.first()).cloned();
                pieces.push(BodyPiece {
                    model: ModelRef {
                        model: model.clone(),
                        textures: skin.into_iter().collect(),
                    },
                    styles: h.styles.clone(),
                    socket: None,
                });
            }
        }
        for (id, table) in [(goggle, &self.goggles), (backpack, &self.backpacks)] {
            let Some(r) = id.and_then(|i| table.get(&i)) else {
                continue;
            };
            // DC-DEFAULT: CLI-743 (proposed) Model<figure type> as the head gear's (the goggle / backpack model pick
            // is not traced), Model0 when the row has no such model
            let Some(model) = r
                .models
                .get(figure)
                .or(r.models.first())
                .filter(|m| !m.is_empty())
            else {
                continue;
            };
            pieces.push(BodyPiece {
                model: ModelRef {
                    model: model.clone(),
                    textures: r.skins.clone(),
                },
                styles: r.styles.clone(),
                socket: (!r.socket.is_empty()).then(|| r.socket.clone()),
            });
        }
        pieces
    }

    /// The gear pieces of a match character: its body model file (from the server's model block) gives the body row
    /// and its figure type, its face model file the face row; head gear, uniform, goggles and backpack come from the
    /// look record (CLI-1231).
    pub fn gear_for_body(
        &self,
        body_model: &str,
        face_model: Option<&str>,
        gear: &GearIds,
    ) -> Vec<BodyPiece> {
        let same = |a: &str, b: &str| {
            a.replace('/', "\\")
                .eq_ignore_ascii_case(&b.replace('/', "\\"))
        };
        let Some((row, body)) = self.chars.row_of_model(body_model) else {
            return Vec::new();
        };
        let figure = self.bodies.get(&(row as i32)).map_or(0, |b| b.figure);
        let face = face_model
            .and_then(|f| self.chars.faces.iter().find(|(_, (m, _))| same(m, f)))
            .map_or(START_FACE, |(&k, _)| k as i32);
        let variant = if gear.uniform >= 0 && (gear.uniform as usize) < body.skins.len() {
            gear.uniform as u32
        } else {
            0
        };
        self.gear_of(figure, variant, face, gear.head, gear.goggle, gear.backpack)
    }

    /// The character's models and pieces for a look: body (with the uniform's skins), face, head gear, goggles,
    /// backpack and the held weapon at the grip socket.
    pub fn look(&self, ids: &LookIds) -> Option<(CharLook, Vec<BodyPiece>)> {
        let row = self.body_row(ids);
        let body = self.chars.bodies.get(&(row as u32))?;
        let skin_count = body.skins.len() as i32;
        // EVIDENCE: cshell+0x1e2f10 (variant = the uniform id when below the count, else 0)
        let variant = if ids.uniform >= 0 && ids.uniform < skin_count {
            ids.uniform as u32
        } else {
            0
        };
        let textures = self.chars.body_textures(row as u32, variant, MARK)?;
        let mut models = vec![ModelRef {
            model: body.model.clone(),
            textures,
        }];
        let b = self.bodies.get(&row).cloned().unwrap_or_default();
        let face = b.face_index.unwrap_or(ids.face);
        if let Some((m, skins)) = self.chars.faces.get(&(face.max(0) as u32)) {
            models.push(ModelRef {
                model: m.clone(),
                textures: skins.iter().take(1).cloned().collect(),
            });
        }
        let pieces = self.gear_of(b.figure, variant, face, ids.head, ids.goggle, ids.backpack);
        let weapon = ids
            .weapon
            .and_then(|w| crate::camo::hand_model(&self.hands, w as u32))
            .map(|h| {
                (
                    ModelRef {
                        model: h.model,
                        textures: h.skins,
                    },
                    // the body's grip socket, found by the renderer in the body file ([`grip_socket_index`])
                    GRIP_UNRESOLVED,
                )
            });
        Some((
            CharLook {
                models,
                weapon,
                styles: Vec::new(),
            },
            pieces,
        ))
    }
}

/// The name of the body socket the held weapon hangs on: the socket whose name key is the session's weapon socket.
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (the weapon object attaches at the body socket with this
// name, socket 2 of the ROM bodies; REFERENCE); the key is dc-dserver's `chars::SOCKET_WEAPON`
pub const GRIP_SOCKET_KEY: dc_render::NameKey =
    ([0xc9, 0x69, 0xc8, 0xaa, 0x86, 0x33, 0x33, 0x73], 9);

/// The weapon socket of a look before the body file is read.
pub const GRIP_UNRESOLVED: u32 = u32::MAX;
/// The socket index when the body has no socket of that name.
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (socket 2; REFERENCE)
pub const GRIP_FALLBACK: u32 = 2;

/// The index of the body's grip socket.
pub fn grip_socket_index(body: &dc_model::Model) -> u32 {
    body.sockets
        .iter()
        .position(|s| dc_render::name_key(s.name.as_bytes()) == GRIP_SOCKET_KEY)
        .map_or(GRIP_FALLBACK, |i| i as u32)
}
