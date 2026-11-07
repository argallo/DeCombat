//! Player characters built from the user's ROM at run time: the character rows of the ROM model table (body model,
//! skins, render styles, face), the team mark texture, and the facts read out of the body model file itself (its
//! child models, the clip list the animation indices count through, the socket a weapon hangs on).
//!
//! Nothing here is ROM data: table names and keys are cited as `OBSERVED: ROM table key ...`, and the clips and the
//! socket the server needs are found by the SHA-256 of their names (8-byte prefix plus length, the form
//! `MOVEMENT_2009.md` §9.2 uses for the stand clip). The reference stack capture's file names never appear in the tree.
//!
//! The create form these assets feed (kind-1 model object with a type-0x18 shell blob, an animation block of three
//! trackers, an attached weapon model) is the form the reference stack used (`objects`, REFERENCE grade).

use std::collections::BTreeMap;

use dc_bute::{eq_fold, ButeDoc};
use dc_rez::RezVfs;

use crate::rom_tables::ATTRIBUTES_REZ;

/// The rows of the ROM model table a character uses: the body row, its skin variant and the face row.
///
/// The 2009 client builds its own character from its look record; how the look bytes or the loadout select these
/// rows is not specified yet, so the server takes them from the lobby's `user` line tokens (`model=`, `skin=`,
/// `face=`) or from this operator default. All zero is the choice the reference stack made for a player whose look
/// and loadout carried no character (REFERENCE, reference run 20261024_232007).
// SPEC: GAME_MESSAGES_2009_C2S.md §5.1 (look meanings UNPROVEN)
// DC-DEFAULT: DSV-24 (proposed)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CharacterChoice {
    pub model_row: u32,
    pub skin: u32,
    pub face_row: u32,
}

/// The character model table (entry of the attribute archive).
// OBSERVED: ROM table key MODELBUTES.TXT (entry of ATTRIBUTES.rez)
pub const MODELBUTES_ENTRY: &str = "ATTRIBUTES\\MODELBUTES.TXT";
/// The team mark table.
// OBSERVED: ROM table key MARKS.TXT (entry of ATTRIBUTES.rez)
pub const MARKS_ENTRY: &str = "ATTRIBUTES\\MARKS.TXT";
/// `[Model<n>]` body rows and their keys: model file, render styles `RenderStyle<k>`, the skin pair of variant `k`
/// (`UpperSkin<k>` for the upper body pieces, `LowerSkin<k>` for the legs).
// OBSERVED: ROM table key MODELBUTES.TXT [Model<n>] ModelFile, RenderStyle0..2, UpperSkin<k>, LowerSkin<k>
pub const BODY_SECTION: &str = "Model";
pub const KEY_MODEL_FILE: &str = "ModelFile";
pub const KEY_RENDER_STYLE: &str = "RenderStyle";
pub const KEY_UPPER_SKIN: &str = "UpperSkin";
pub const KEY_LOWER_SKIN: &str = "LowerSkin";
/// `[Model<n>]` keys of the damage rule: the row's skeleton number and its armour maximum.
// OBSERVED: ROM table key MODELBUTES.TXT [Model<n>] Skeleton, MaxArmor
pub const KEY_SKELETON: &str = "Skeleton";
pub const KEY_MAX_ARMOR: &str = "MaxArmor";
/// `GearId` of the body (`[Model<n>]`), head-gear (`[Hair<n>]`) and goggle (`ATTACHMENTS.TXT` `[Goggle<n>]`) rows:
/// the armour row `[Gear<g>]` of the weapon table the piece puts on (default -1: none). The ROM spells the key
/// `GearID`; the key is matched with case folding.
// EVIDENCE: cshell+0x1e4760 (model-table reader: `Model<n>` `GearId` default -1 at row +0x380, `Hair<n>` `GearId` at
// row +0x70, the face rows' `GearId` at +0x78), cshell+0x1dfc0a (attachment reader: `GearId` at row +0x120)
// OBSERVED: ROM table keys MODELBUTES.TXT [Model<n>] / [Hair<n>] GearID, ATTACHMENTS.TXT [Goggle<n>] GearID
pub const KEY_GEAR_ID: &str = "GearId";
/// `[Hair<n>]` head-gear rows (head item number - 4000) and `[Goggle<n>]` goggle rows (goggle item - 5000).
// EVIDENCE: cshell+0x1e4760 (`Hair` rows), cshell+0x1dfc0a (`Goggle` rows); the item mapping as CLI-1137 / CLI-743
pub const HAIR_SECTION: &str = "Hair";
pub const GOGGLE_SECTION: &str = "Goggle";
/// Equipment slot and item number base of the head gear and the goggle (face attachment).
// SPEC: PROTOCOL_2009.md §4.5 (item number ranges); EVIDENCE: cshell+0x259f00 (type 0xb -> slot 4, 0xc -> slot 6)
pub const LOOK_HEAD: (u8, u32) = (4, 4000);
pub const LOOK_GOGGLE: (u8, u32) = (6, 5000);
/// `[Skeleton<s>]` rows: per hit node `k` the damage multiplier and the location code (1 head, 2 torso, 3 arms,
/// 4 legs, 0 none) of the model part. The keys are `Node<k>` followed by the suffix.
// OBSERVED: ROM table key MODELBUTES.TXT [Skeleton<s>] Node<k>DamageFactor, Node<k>Location (4 skeleton rows, 47-79
// nodes each; values read at run time)
pub const SKELETON_SECTION: &str = "Skeleton";
pub const KEY_NODE: &str = "Node";
pub const NODE_FACTOR_SUFFIX: &str = "DamageFactor";
pub const NODE_LOCATION_SUFFIX: &str = "Location";
/// Upper bound of the node scan.
pub const MAX_HIT_NODES: u32 = 256;
/// `[Face<n>]` rows: model file, `Skin<k>`, `RenderStyle<k>`.
// OBSERVED: ROM table key MODELBUTES.TXT [Face<n>] ModelFile, Skin0, RenderStyle0..1
pub const FACE_SECTION: &str = "Face";
pub const KEY_SKIN: &str = "Skin";
/// `[TeamMark] TeamName<k>`: the mark texture of the body's mark pieces.
// OBSERVED: ROM table key MARKS.TXT [TeamMark] TeamName0, TeamName1
pub const TEAM_MARK_SECTION: &str = "TeamMark";
pub const KEY_TEAM_MARK: &str = "TeamName";
/// Which `TeamName<k>` the mark pieces get: the reference stack used `TeamName0` for players of both teams.
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md (reference run: both bots, teams 1 and 2, the same mark file; REFERENCE)
// DC-DEFAULT: DSV-24 (proposed)
pub const TODO_TEAM_MARK_INDEX: u32 = 0;
/// Texture slots of the body model filled with the team mark after the two skins (the mark and the two emblem
/// pieces of the ROM body models use material slots 2..4; the reference stack filled all three with the mark).
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (tex list of the body: 2 skins + 3 x the mark; REFERENCE)
pub const MARK_SLOTS: usize = 3;
/// Render styles of a body row and of a face row (`RenderStyle0..2` / `RenderStyle0..1`).
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (3 styles on the body, 2 on the face; REFERENCE)
pub const BODY_STYLES: u32 = 3;
pub const FACE_STYLES: u32 = 2;

/// Name key of a clip or socket: SHA-256 prefix of the name and its length (no ROM name in the tree).
pub type NameKey = ([u8; 8], usize);

/// The clips the reference stack animated a player with (combined index into body + child clips):
/// upper body (tracker 1) and lower body (tracker 2), standing and walking forward.
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run, stream entries of both player objects (REFERENCE): standing
// tracker 1 / 2 = these upper / lower clips, walking (control FWD) the walk pair. The lower standing clip is also
// the stand clip of MOVEMENT_2009.md §9.2 (key 7f007bf2ff2bcc78/3, read by the 2009 client for its stand box).
pub const CLIP_UPPER_STAND: NameKey = ([0xbc, 0x72, 0x1b, 0x0c, 0x9c, 0x7a, 0xa2, 0x26], 7);
pub const CLIP_UPPER_WALK: NameKey = ([0xf9, 0xc5, 0x3f, 0x25, 0x64, 0x31, 0xfb, 0x05], 7);
pub const CLIP_LOWER_STAND: NameKey = ([0x7f, 0x00, 0x7b, 0xf2, 0xff, 0x2b, 0xcc, 0x78], 3);
pub const CLIP_LOWER_WALK: NameKey = ([0x5f, 0x8e, 0x2d, 0x7e, 0x3a, 0xa7, 0xa1, 0x7d], 3);
/// Lower-body direction clips of the three gaits, in the order of [`Dir`](crate::game::Dir) (forward, back, left,
/// right, forward-left, forward-right, back-left, back-right): run (the `RUN` control bit), walk (no `RUN`), crouch
/// (the `CROUCH` bit). The walk forward clip is [`CLIP_LOWER_WALK`]. With the user's ROM and the default body the
/// run set is combined clips 37 / 36 / 39 / 38 / 41 / 40 / 43 / 42, the walk set 47 / 46 / 45 / 44 / 49 / 48 / 51 /
/// 50, the crouch set 18 / 20 / 21 / 22 / 24 / 23 / 26 / 25 (keys found by sha256 of the clip names in a scratch run
/// over dc-model, never by name). Only the run clips carry footstep animation keys ([`ClipFact::footsteps`]), so a
/// character on a run clip makes step sounds on the clients and one on a walk or crouch clip does not.
// OBSERVED: capture 20261025_000014_tdm1 (the reference stack streamed the user's own object 408 with
// run forward 37 for `RUN` + `FWD` in 1,911 entries, 41 forward-left, 40 forward-right, 39 left, 38 right, 36 back,
// 42 back-right; walk forward 47 without `RUN` in 128, 49 forward-left; crouch forward 18 in 83 and crouch still 19
// in 36; REFERENCE), 20261025_002236_tdm4 (object 189 on both links, the same clips); MOVEMENT_2009.md §5.1 (control
// bits)
// DC-DEFAULT: DSV-28 (refinement, proposed) the lower clip from the control word
pub const CLIP_LOWER_RUN: [NameKey; 8] = [
    ([0xdc, 0xe6, 0x44, 0xba, 0x7b, 0xc1, 0xb3, 0x24], 3),
    ([0x7a, 0x68, 0xff, 0xc8, 0xf7, 0x26, 0x54, 0x3c], 3),
    ([0x44, 0x5f, 0xfc, 0xc7, 0x5d, 0x07, 0xd1, 0x52], 3),
    ([0x60, 0x4f, 0x1e, 0x65, 0xf5, 0xc9, 0x86, 0x1e], 3),
    ([0x47, 0xb6, 0x5f, 0x74, 0x48, 0x1d, 0xe8, 0x88], 4),
    ([0x6f, 0x43, 0x0a, 0x91, 0x9d, 0x3f, 0x62, 0xdf], 4),
    ([0x36, 0x4c, 0xe2, 0x9c, 0xfd, 0x97, 0x52, 0x5a], 4),
    ([0x36, 0x1c, 0x17, 0x45, 0x42, 0x53, 0x79, 0x2c], 4),
];
pub const CLIP_LOWER_WALK_DIRS: [NameKey; 8] = [
    CLIP_LOWER_WALK,
    ([0x0e, 0x6f, 0x3a, 0x82, 0x00, 0x21, 0x41, 0x48], 3),
    ([0x03, 0xe0, 0xdc, 0x52, 0x09, 0x7b, 0xce, 0x99], 3),
    ([0x1f, 0x97, 0xcb, 0xdc, 0x9e, 0xfb, 0x0d, 0xb8], 3),
    ([0x0e, 0xd4, 0x99, 0xc0, 0xa5, 0x76, 0xd3, 0x0e], 4),
    ([0xcd, 0x05, 0xda, 0xab, 0x4f, 0xe4, 0x76, 0xdd], 4),
    ([0xb5, 0x69, 0xe7, 0xd0, 0xa7, 0xf0, 0xa6, 0xcc], 4),
    ([0xc6, 0x5d, 0xf5, 0x58, 0x0d, 0x94, 0x53, 0x8d], 4),
];
pub const CLIP_LOWER_CROUCH: [NameKey; 8] = [
    ([0xe8, 0x1b, 0xae, 0xc6, 0x47, 0x8f, 0xc1, 0xca], 3),
    ([0x14, 0x67, 0x88, 0xc0, 0x23, 0xb4, 0x87, 0x8a], 3),
    ([0xab, 0xec, 0xbf, 0xa8, 0x69, 0x48, 0x04, 0x93], 3),
    ([0xda, 0x83, 0x06, 0x0b, 0x3d, 0x4f, 0x7d, 0x1c], 3),
    ([0x39, 0x5d, 0xa5, 0xb1, 0xe8, 0xe0, 0x60, 0x53], 4),
    ([0x7d, 0x2b, 0x90, 0x9c, 0xcb, 0x0f, 0xd8, 0x38], 4),
    ([0xe2, 0xb0, 0x17, 0x06, 0xc6, 0xf6, 0x07, 0xea], 4),
    ([0x08, 0xd7, 0xb5, 0x02, 0xdd, 0x10, 0x89, 0x22], 4),
];
/// The crouched standing clip (combined clip 19 on the default body).
// OBSERVED: capture 20261025_000014_tdm1 (crouch with no direction bit -> 19 in 36 entries;
// REFERENCE)
pub const CLIP_LOWER_CROUCH_STAND: NameKey = ([0x0a, 0x98, 0x0e, 0x22, 0x63, 0x73, 0x50, 0x01], 2);
/// Names (upper-cased) of the weight sets the upper-body and the lower-body tracker play on.
// EVIDENCE: cshell+0x15ef0 (the player animator's init looks its two weight sets up by name, the upper one at
// +0x40ec and the lower one at +0x40f0); OBSERVED: CA_ROM body models 2026-10-31 (the male bodies list the sets
// upper / lower / blend, the female bodies 7..9 upper / blend / lower, so a fixed index 1 is the 2-node blend set on
// a female body and its legs never move)
// DC-DEFAULT: DSV-475 (proposed) the name compares without letter case (the male bodies spell the upper set in
// lower case), a body without the set keeps the reference stack's index
pub const WEIGHT_SET_UPPER: NameKey = ([0x9c, 0xba, 0x1c, 0x65, 0x93, 0xa6, 0x2c, 0xf5], 5);
pub const WEIGHT_SET_LOWER: NameKey = ([0x86, 0x54, 0x3e, 0xdc, 0xc2, 0xf0, 0xf4, 0xcc], 5);

/// The index of the blend set whose upper-cased name has the key `key`.
pub fn weight_set_of(m: &dc_model::Model, key: NameKey) -> Option<u8> {
    m.blend_sets
        .iter()
        .position(|b| name_key(&b.name.as_bytes().to_ascii_uppercase()) == key)
        .and_then(|i| u8::try_from(i).ok())
}

/// First word of a clip key's command that makes the 2009 client play a footstep (the model-key handler compares
/// it without case and calls its footstep player; for the local player's own object the surface comes from the
/// mover).
// DISASM: cshell+0x2057a0 (model-key handler; the compare at cshell+0x205855 is the only reference of the key
// string), cshell+0x204f60 (footstep player, called at cshell+0x20588f / +0x2058b5; branch for the client's own
// object, ground probe cshell+0x1fe1f0)
pub const KEY_FOOTSTEP: NameKey = ([0x18, 0x71, 0x8b, 0x81, 0x25, 0x4f, 0x1d, 0xea], 12);
/// The death clips of a player's corpse (combined index into body + child clips): a body hit falling to the left /
/// to the right, a head hit falling to the left / to the right.
// OBSERVED: capture 20261025_065016_tdm6 (the reference stack's seven corpse objects, blob type 0x26,
// played the combined clips 86 after the head-shot kill and 87 / 88 after the body kills; with the user's ROM and the
// default body these are the clips with these name keys, found by sha256 of the clip names in a scratch run over
// dc-model, never by name); the head / body split follows that capture, the left / right pair is our reading of the
// clip pairs (UNPROVEN which side the reference stack chose, DSV-44)
pub const CLIP_DEATH_BODY_LEFT: NameKey = ([0x03, 0x04, 0x2f, 0x73, 0xb1, 0x08, 0x04, 0x46], 3);
pub const CLIP_DEATH_BODY_RIGHT: NameKey = ([0xbf, 0x96, 0x9b, 0x7e, 0xba, 0xa7, 0xf5, 0x4d], 3);
pub const CLIP_DEATH_HEAD_LEFT: NameKey = ([0x5a, 0x6e, 0x14, 0xda, 0x8d, 0x04, 0xb1, 0x44], 4);
pub const CLIP_DEATH_HEAD_RIGHT: NameKey = ([0x2b, 0x2e, 0xd1, 0x7e, 0x2b, 0xa1, 0x27, 0xb0], 4);
/// The body socket the drawn weapon model is attached to (socket index 2 of the ROM body models).
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (attachment socket 2 = the socket with this name; REFERENCE)
pub const SOCKET_WEAPON: NameKey = ([0xc9, 0x69, 0xc8, 0xaa, 0x86, 0x33, 0x33, 0x73], 9);
/// Socket index used when the body model has no socket of that name.
// SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (socket 2; REFERENCE)
pub const TODO_WEAPON_SOCKET_FALLBACK: u32 = 2;

/// The key of a name.
pub fn name_key(name: &[u8]) -> NameKey {
    let h = dc_rez::dc_rom::sha256(name);
    let mut k = [0u8; 8];
    k.copy_from_slice(&h[..8]);
    (k, name.len())
}

/// One `[Model<n>]` row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BodyRow {
    pub model: String,
    pub styles: Vec<String>,
    /// Per skin variant `k`: (`UpperSkin<k>`, `LowerSkin<k>`).
    pub skins: BTreeMap<u32, (Option<String>, Option<String>)>,
    /// `Skeleton`: the row's hit-node table.
    pub skeleton: Option<u32>,
    /// `MaxArmor`: the armour maximum of a player with this body.
    pub max_armor: Option<u32>,
    /// `FaceRelationModel<k>`: the body row this vest row stands for with face `k` (-1: itself).
    pub face_relation: Vec<i32>,
    /// `InfectedOveridable` / `HostInfectedOveridable`: the override row of an infected / infected host player
    /// with this body (`None`: the key is missing or -1).
    pub infected_override: Option<u32>,
    pub host_override: Option<u32>,
    /// `GearId`: the armour row this body wears (`None`: missing or negative).
    pub gear: Option<u32>,
}

/// One hit node of a skeleton row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitNode {
    pub factor: f32,
    pub location: u8,
}

/// `[Overidable_Model<n>]` rows: the overridable models a look's override byte selects (the infected looks of the
/// infection mode): render styles `RenderStyle0..2` over the body and the face row `FaceIndex`.
// OBSERVED: ROM table key MODELBUTES.TXT [Overidable_Model<n>] RenderStyle0..2, FaceIndex (rows 0 / 1 infected,
// 2 / 3 infected host, by name)
// EVIDENCE: cshell+0x1e3430 (the override row's face replaces the model row's face when its index is not -1)
pub const OVERRIDE_SECTION: &str = "Overidable_Model";
pub const KEY_FACE_INDEX: &str = "FaceIndex";
/// `[Model<n>] InfectedOveridable` / `HostInfectedOveridable` (the override row of the body's infected / host look)
/// and `[Overidable_Model<n>] MaxHitPoints`.
// EVIDENCE: cshell+0x1e4760 (the model-table reader stores both keys with default -1 at row +0x450 / +0x454 and
// `MaxHitPoints` at +0x458)
// OBSERVED: ROM table key MODELBUTES.TXT [Model<n>] InfectedOveridable, HostInfectedOveridable;
// [Overidable_Model<n>] MaxHitPoints, MovePenalty, MaxStamina
pub const KEY_INFECTED_OVERRIDE: &str = "InfectedOveridable";
pub const KEY_HOST_OVERRIDE: &str = "HostInfectedOveridable";
pub const KEY_MAX_HIT_POINTS: &str = "MaxHitPoints";
/// `[Model<n>] FaceRelationModel<k>`: the body row a vest row stands for with face `k`.
// OBSERVED: ROM table key MODELBUTES.TXT [Model<n>] FaceRelationModel<k>
pub const KEY_FACE_RELATION: &str = "FaceRelationModel";

/// The equipment slots and item number bases of the worn look: character item (slot 14, 10000s), vest (slot 8,
/// 3000s), uniform (slot 7, 2000s); the item's own id = number - base.
// SPEC: PROTOCOL_2009.md §4.5 (item number ranges)
pub const LOOK_CHARACTER: (u8, u32) = (14, 10000);
pub const LOOK_VEST: (u8, u32) = (8, 3000);
pub const LOOK_UNIFORM: (u8, u32) = (7, 2000);

/// The body row a player's voice family follows: the `user` line's `model=` row, else his equipment's body row
/// ([`loadout_look`]: the mercenary item, else the vest), else `None` (the operator default).
// DC-DEFAULT: DSV-420 (proposed) the voice follows the body the player is drawn with (DSV-271)
// EVIDENCE: cshell+0x1e4760 (the figure's sound template is read per body row), cshell+0x1d32e0 (the look's body row)
pub fn voice_row(
    t: &CharTables,
    named: Option<u32>,
    equipped: Option<&[(Option<u8>, u32)]>,
    face: u32,
) -> Option<u32> {
    named.or_else(|| loadout_look(t, equipped?, face).0)
}

/// The vest row of a look whose vest slot is empty: the look record starts with vest 0.
// EVIDENCE: cshell+0x12cc60 (own look record start: head 0, uniform 0, vest 0, backpack -1, goggle -1)
// DC-DEFAULT: DSV-474 (proposed, user 2026-10-31 "the two lady character models' faces are all stretched on the
// bots"), see DC_DEFAULTS.md
pub const START_VEST: u32 = 0;

/// The body row and skin variant a player's equipment puts on (`None` each: the equipment names none): the character
/// item's row when the table has it, else the vest's row (its `FaceRelationModel<face>` when set; an empty vest slot
/// is [`START_VEST`], DSV-474); the skin variant is the uniform's id.
// EVIDENCE: cshell+0x1d32e0 (body row = the character id when set, else the vest row's FaceRelationModel<face> when
// that is set, else the vest row), cshell+0x1e2f10 (skin variant = the uniform id when below the row's count, else 0),
// cshell+0x14e410 (the look record keeps the equipped items' ids per type)
// DC-DEFAULT: DSV-271 (proposed, playtest report "not sure if gear worn is properly changing speed and damage calcs")
// the server's character of a player is the shop look's body row of its `loadout=` equipment (before: the operator
// default for everyone without `model=`); the body row carries the speed penalty the client reads from the own look
// and the armour maximum (`MaxArmor`)
pub fn loadout_look(
    t: &CharTables,
    equipped: &[(Option<u8>, u32)],
    face: u32,
) -> (Option<u32>, Option<u32>) {
    let id = |(slot, base): (u8, u32)| {
        equipped
            .iter()
            .find(|(s, _)| *s == Some(slot))
            .and_then(|(_, n)| n.checked_sub(base))
            .filter(|v| *v < 1000)
    };
    let body = id(LOOK_CHARACTER)
        .filter(|c| t.bodies.contains_key(c))
        .or_else(|| {
            // DSV-474: an empty vest slot is vest row 0 (the look record's start value), so a female face takes its
            // female body through that row's relation instead of the operator's male default
            let v = id(LOOK_VEST).unwrap_or(START_VEST);
            let row = t.bodies.get(&v)?;
            Some(
                row.face_relation
                    .get(face as usize)
                    .copied()
                    .filter(|m| *m >= 0)
                    .map(|m| m as u32)
                    .filter(|m| t.bodies.contains_key(m))
                    .unwrap_or(v),
            )
        });
    (body, id(LOOK_UNIFORM))
}

/// The look-block bytes of the gear and the equipment slot / item base each comes from: head gear (slot 4, 4000s),
/// uniform (slot 7, 2000s), backpack (slot 9, 7000s), goggle (slot 6, 5000s).
// EVIDENCE: cshell+0x1c2a90 (look bytes 8..12 -> look +0x18 .. +0x28), cshell+0x1c2230 (look start +0x18 / +0x1c /
// +0x20 = 0, +0x24 / +0x28 = -1), cshell+0x12cc60 (own look record start: head 0, uniform 0, vest 0, backpack -1,
// goggle -1, in that order); the item of each byte is PLAUSIBLE from that order; SPEC: PROTOCOL_2009.md §4.5
// DC-DEFAULT: DSV-341 (proposed)
pub const LOOK_GEAR: [(usize, u8, u32, i8); 4] = [
    (7, 4, 4000, 0),
    (8, 7, 2000, 0),
    (10, 9, 7000, -1),
    (11, 6, 5000, -1),
];

/// Writes the gear of a player's equipment into its look bytes (an empty slot: the look's start value): the clients
/// dress the character with the same pieces as the shop figure (CLI-1231). An id above 255 cannot be carried and
/// leaves the start value.
// DC-DEFAULT: DSV-341 (proposed, playtest report "remote bodies draw no gear")
pub fn gear_look(look: &mut [i8; 15], equipped: &[(Option<u8>, u32)]) {
    for (byte, slot, base, none) in LOOK_GEAR {
        let id = equipped
            .iter()
            .find(|(s, _)| *s == Some(slot))
            .and_then(|(_, n)| n.checked_sub(base))
            .filter(|v| *v < 1000);
        look[byte] = match id {
            Some(v) if v < 255 => v as u8 as i8,
            _ => none,
        };
    }
}

/// One `[Overidable_Model<n>]` row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OverrideRow {
    pub styles: Vec<String>,
    /// The face row (`None` for -1 or missing).
    pub face: Option<u32>,
    /// `MaxHitPoints` (`None`: missing or not above 0).
    pub max_hit_points: Option<u32>,
}

/// One `[Face<n>]` row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FaceRow {
    pub model: String,
    pub skins: Vec<String>,
    pub styles: Vec<String>,
}

/// The character tables of the ROM.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CharTables {
    pub bodies: BTreeMap<u32, BodyRow>,
    pub faces: BTreeMap<u32, FaceRow>,
    /// `[Overidable_Model<n>]` rows by n.
    pub overrides: BTreeMap<u32, OverrideRow>,
    /// `TeamName<k>` by `k`.
    pub team_marks: BTreeMap<u32, String>,
    /// `[Skeleton<s>]` hit nodes by `s`, in node order (the index the client reports in `UP_FIRE`).
    pub skeletons: BTreeMap<u32, Vec<HitNode>>,
    /// `[Hair<n>] GearId` by n (rows without the key are absent).
    pub hair_gear: BTreeMap<u32, u32>,
    /// `ATTACHMENTS.TXT` `[Goggle<n>] GearId` by n (rows without the key are absent).
    pub goggle_gear: BTreeMap<u32, u32>,
}

/// A row's `GearId` (case-folded key match; `None` when missing or negative).
fn gear_id(sec: &dc_bute::SectionMap) -> Option<u32> {
    let v = sec.get(KEY_GEAR_ID).or_else(|| {
        sec.iter()
            .find(|(k, _)| eq_fold(k, KEY_GEAR_ID))
            .map(|(_, v)| v)
    });
    num_f32(v).filter(|g| *g >= 0.0).map(|g| g as u32)
}

fn num_f32(v: Option<&String>) -> Option<f32> {
    let t = unquote(v?);
    let t = t.strip_suffix(['f', 'F']).unwrap_or(t);
    t.parse::<f32>().ok().filter(|x| x.is_finite())
}

/// The hit nodes of one skeleton row: `Node<k>...` for k = 0, 1, ... while the factor key exists.
fn hit_nodes(sec: &dc_bute::SectionMap) -> Vec<HitNode> {
    (0..MAX_HIT_NODES)
        .map_while(|k| {
            let factor = num_f32(sec.get(&format!("{KEY_NODE}{k}{NODE_FACTOR_SUFFIX}")))?;
            let location = num_f32(sec.get(&format!("{KEY_NODE}{k}{NODE_LOCATION_SUFFIX}")))
                .filter(|v| (0.0..=255.0).contains(v))
                .map_or(0, |v| v as u8);
            Some(HitNode { factor, location })
        })
        .collect()
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches('"').trim()
}

fn index_of(section: &str, prefix: &str) -> Option<u32> {
    let rest = section.strip_prefix(prefix)?;
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

/// `key0, key1, ...` while present and not empty.
fn numbered(sec: &dc_bute::SectionMap, key: &str, max: u32) -> Vec<String> {
    (0..max)
        .map_while(|k| {
            sec.get(&format!("{key}{k}"))
                .map(|v| unquote(v).to_string())
                .filter(|v| !v.is_empty())
        })
        .collect()
}

impl CharTables {
    pub fn from_docs(models: &ButeDoc, marks: Option<&ButeDoc>) -> Self {
        let mut t = CharTables::default();
        for (name, sec) in models.iter() {
            if let Some(n) = index_of(name, BODY_SECTION) {
                let Some(model) = sec.get(KEY_MODEL_FILE).map(|v| unquote(v).to_string()) else {
                    continue;
                };
                let mut skins = BTreeMap::new();
                for k in 0..64u32 {
                    let up = sec
                        .get(&format!("{KEY_UPPER_SKIN}{k}"))
                        .map(|v| unquote(v).to_string());
                    let low = sec
                        .get(&format!("{KEY_LOWER_SKIN}{k}"))
                        .map(|v| unquote(v).to_string());
                    if up.is_some() || low.is_some() {
                        skins.insert(k, (up, low));
                    }
                }
                let int = |k: &str| num_f32(sec.get(k)).filter(|v| *v >= 0.0).map(|v| v as u32);
                t.bodies.insert(
                    n,
                    BodyRow {
                        model,
                        styles: numbered(sec, KEY_RENDER_STYLE, BODY_STYLES),
                        skins,
                        skeleton: int(KEY_SKELETON),
                        max_armor: int(KEY_MAX_ARMOR),
                        face_relation: (0..64)
                            .map_while(|k| {
                                num_f32(sec.get(&format!("{KEY_FACE_RELATION}{k}")))
                                    .map(|v| v as i32)
                            })
                            .collect(),
                        infected_override: int(KEY_INFECTED_OVERRIDE),
                        host_override: int(KEY_HOST_OVERRIDE),
                        gear: gear_id(sec),
                    },
                );
            } else if let Some(n) = index_of(name, OVERRIDE_SECTION) {
                let face = num_f32(sec.get(KEY_FACE_INDEX))
                    .filter(|v| *v >= 0.0)
                    .map(|v| v as u32);
                t.overrides.insert(
                    n,
                    OverrideRow {
                        styles: numbered(sec, KEY_RENDER_STYLE, BODY_STYLES),
                        face,
                        max_hit_points: num_f32(sec.get(KEY_MAX_HIT_POINTS))
                            .filter(|v| *v >= 1.0)
                            .map(|v| v as u32),
                    },
                );
            } else if let Some(n) = index_of(name, HAIR_SECTION) {
                if let Some(g) = gear_id(sec) {
                    t.hair_gear.insert(n, g);
                }
            } else if let Some(n) = index_of(name, SKELETON_SECTION) {
                t.skeletons.insert(n, hit_nodes(sec));
            } else if let Some(n) = index_of(name, FACE_SECTION) {
                let Some(model) = sec.get(KEY_MODEL_FILE).map(|v| unquote(v).to_string()) else {
                    continue;
                };
                t.faces.insert(
                    n,
                    FaceRow {
                        model,
                        skins: numbered(sec, KEY_SKIN, 8),
                        styles: numbered(sec, KEY_RENDER_STYLE, FACE_STYLES),
                    },
                );
            }
        }
        if let Some(sec) = marks.and_then(|d| d.get(TEAM_MARK_SECTION)) {
            for k in 0..8u32 {
                if let Some(v) = sec.get(&format!("{KEY_TEAM_MARK}{k}")) {
                    let v = unquote(v);
                    if !v.is_empty() {
                        t.team_marks.insert(k, v.to_string());
                    }
                }
            }
        }
        t
    }

    pub fn from_vfs(vfs: &RezVfs) -> Result<Self, String> {
        let models = match ButeDoc::try_from_vfs(vfs, ATTRIBUTES_REZ, MODELBUTES_ENTRY) {
            Ok(Some(d)) => d,
            Ok(None) => return Err("character model table entry missing".into()),
            Err(e) => return Err(format!("character model table: {e}")),
        };
        let marks = ButeDoc::try_from_vfs(vfs, ATTRIBUTES_REZ, MARKS_ENTRY)
            .ok()
            .flatten();
        let mut t = Self::from_docs(&models, marks.as_ref());
        if let Ok(Some(d)) =
            ButeDoc::try_from_vfs(vfs, ATTRIBUTES_REZ, crate::rom_tables::ATTACHMENTS_ENTRY)
        {
            t.add_attachments(&d);
        }
        Ok(t)
    }

    /// Adds the goggle rows' `GearId` of the attachment table.
    pub fn add_attachments(&mut self, doc: &ButeDoc) {
        for (name, sec) in doc.iter() {
            if let (Some(n), Some(g)) = (index_of(name, GOGGLE_SECTION), gear_id(sec)) {
                self.goggle_gear.insert(n, g);
            }
        }
    }

    /// The armour rows a player wears: the body row's (`None` when the row names none), then the head gear's and
    /// the goggle's of its equipment (`None` for an empty slot or a row without one).
    // EVIDENCE: cshell+0x1e4760, cshell+0x1dfc0a (the three tables carry a `GearId` each)
    // DC-DEFAULT: DSV-430 (proposed) the worn armour is the body row's gear row plus the head gear's and the
    // goggle's (the 2009 server's use of the ids is not visible)
    pub fn worn_gear(
        &self,
        body_row: Option<u32>,
        equipped: &[(Option<u8>, u32)],
    ) -> [Option<u32>; 3] {
        let id = |(slot, base): (u8, u32)| {
            equipped
                .iter()
                .find(|(s, _)| *s == Some(slot))
                .and_then(|(_, n)| n.checked_sub(base))
                .filter(|v| *v < 1000)
        };
        [
            body_row
                .and_then(|b| self.bodies.get(&b))
                .and_then(|r| r.gear),
            id(LOOK_HEAD).and_then(|h| self.hair_gear.get(&h).copied()),
            id(LOOK_GOGGLE).and_then(|g| self.goggle_gear.get(&g).copied()),
        ]
    }

    pub fn is_empty(&self) -> bool {
        self.bodies.is_empty()
    }
}

/// The archive and the entry of a ROM resource path: the first path component is the archive's name (every ROM
/// archive lists its entries under a top folder of its own name), the whole path is the entry.
// OBSERVED: ROM archive layout (2026-10-25: the entries of every `.rez` under `Game` start with `<archive name>\`)
// SPEC: UDP_SESSION_2009_A.md §4.4 (the client resolves a file-list name in every mounted resource tree)
pub fn archive_of(path: &str) -> Option<(String, String)> {
    let p = path.trim().replace('/', "\\");
    let p = p.trim_start_matches('\\');
    let (top, rest) = p.split_once('\\')?;
    if top.is_empty() || rest.is_empty() {
        return None;
    }
    Some((format!("{top}.rez"), p.to_string()))
}

/// A resource the client must resolve: its path as the server announces it and its size in bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    /// Upper case with `\` separators, the spelling the reference stack announced (the client upper-cases names anyway,
    /// `UDP_SESSION_2009_A.md` §4.4).
    pub path: String,
    pub size: u32,
}

/// Announced spelling of a resource path.
// SPEC: UDP_SESSION_2009_A.md §4.4 (letter case does not matter; the client keeps an upper-cased copy)
// DC-DEFAULT: VAR-3 (proposed) a pack's path (`@mod:`, `@camo:`) is ours, not the 2009 archive's: its case and separators mean
// something to the client, so it keeps its spelling
pub fn announce_spelling(path: &str) -> String {
    let path = path.trim();
    if path.starts_with('@') {
        return path.to_string();
    }
    path.replace('/', "\\")
        .trim_start_matches('\\')
        .to_ascii_uppercase()
}

/// One clip of the composed model.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ClipFact {
    pub key: NameKey,
    /// Length in ms (time of the last key frame).
    pub len_ms: u32,
    /// The clip's user dims (FMT_LTB.md §10.1 extent).
    pub extent: [f32; 3],
    /// Clip keys whose command starts with the footstep word ([`KEY_FOOTSTEP`]).
    pub footsteps: u8,
    /// The clip's family prefix key and slot in the upper-body table ([`crate::anims::classify`]).
    // DC-DEFAULT: DSV-125 (proposed)
    pub upper: Option<(NameKey, crate::anims::Slot)>,
}

/// The number of a clip's keys whose command's first word is the footstep word (any letter case).
// DISASM: cshell+0x2057a0 (case-insensitive compare of the key name)
pub fn footstep_keys(markers: &[dc_model::Marker]) -> u8 {
    let n = markers
        .iter()
        .filter(|k| {
            let t = k.command.0.to_ascii_uppercase();
            let w = t.split(|b| b.is_ascii_whitespace()).find(|w| !w.is_empty());
            w.is_some_and(|w| name_key(w) == KEY_FOOTSTEP)
        })
        .count();
    n.min(u8::MAX as usize) as u8
}

/// What the server reads out of a body model file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Rig {
    /// Child model paths (body folder + the child name the body file lists).
    pub children: Vec<String>,
    /// Clips in the order animation indices count them: the body's own clips, then each child's.
    // SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (indices 6 / 47 / 96 / 99 = body clip count 1 + child
    // clip index; REFERENCE); FMT_LTB.md §9 (child models lend their clips), §10
    pub clips: Vec<ClipFact>,
    /// Number of the body's own clips (the first entries of `clips`).
    pub own_clips: usize,
    /// Socket the weapon hangs on.
    pub grip_socket: u32,
    /// Half extents of the standing player: the user dims of the stand clip (a per-child clip override first).
    // SPEC: MOVEMENT_2009.md §9.2 (the client reads its standing box from the user dims of the clip with key
    // 7f007bf2ff2bcc78/3, cshell+0x1ff430 tail); FMT_LTB.md §10.1, §11.2 (override extent)
    pub stand_extent: Option<[f32; 3]>,
    /// The body model's socket names in index order (the attachment list names a socket by its index).
    pub sockets: Vec<String>,
    /// Index of the body's upper-body and lower-body weight sets (the blend sets named so, letter case ignored);
    /// `None` where the body has no such set (the tracker then keeps the reference stack's index).
    pub tracker_masks: [Option<u8>; 2],
}

impl Rig {
    /// Index of the body socket named `name` (letter case ignored).
    pub fn socket_at(&self, name: &str) -> Option<u32> {
        self.sockets
            .iter()
            .position(|s| s.eq_ignore_ascii_case(name))
            .map(|i| i as u32)
    }

    /// Combined index of the first clip with this key.
    pub fn clip(&self, key: NameKey) -> Option<(u16, u32)> {
        self.clips
            .iter()
            .position(|c| c.key == key)
            .map(|i| (i as u16, self.clips[i].len_ms))
    }
}

pub(crate) fn read_entry(vfs: &RezVfs, path: &str) -> Result<Vec<u8>, String> {
    let (rez, entry) =
        archive_of(path).ok_or_else(|| format!("resource path {} has no archive", path.len()))?;
    match vfs.entry(&rez, &entry) {
        Ok(Some(b)) => Ok(b),
        Ok(None) => Err(format!(
            "resource not in the ROM (sha256 {})",
            short_hash(path)
        )),
        Err(e) => Err(format!("archive error: {e}")),
    }
}

/// A short hash of a path for log lines (no ROM name in logs either).
pub fn short_hash(path: &str) -> String {
    let h = dc_rez::dc_rom::sha256(announce_spelling(path).as_bytes());
    h[..6].iter().map(|b| format!("{b:02x}")).collect()
}

fn clip_facts(m: &dc_model::Model) -> Vec<ClipFact> {
    m.clips
        .iter()
        .map(|c| ClipFact {
            key: name_key(&c.name.0),
            len_ms: c.markers.iter().map(|k| k.time_ms).max().unwrap_or(0),
            extent: c.extent,
            footsteps: footstep_keys(&c.markers),
            upper: crate::anims::classify(&c.name.0),
        })
        .collect()
}

/// Rigs read so far, and the clip lists of child models (many bodies share one large child model).
#[derive(Debug, Clone, Default)]
pub struct RigCache {
    rigs: BTreeMap<String, Result<Rig, String>>,
    children: BTreeMap<String, Vec<ClipFact>>,
}

impl RigCache {
    /// The rig of a body model, read once.
    pub fn rig(&mut self, vfs: &RezVfs, body: &str) -> Result<Rig, String> {
        let k = announce_spelling(body);
        if let Some(r) = self.rigs.get(&k) {
            return r.clone();
        }
        let r = read_rig(vfs, body, &mut self.children);
        self.rigs.insert(k, r.clone());
        r
    }

    /// A rig already in the cache.
    pub fn cached(&self, body: &str) -> Option<Rig> {
        self.rigs
            .get(&announce_spelling(body))
            .and_then(|r| r.clone().ok())
    }

    /// Puts a rig in the cache (tests without a ROM).
    pub fn insert(&mut self, body: &str, rig: Rig) {
        self.rigs.insert(announce_spelling(body), Ok(rig));
    }
}

/// Reads a body model: its child models (resolved in the body's folder) and the composed clip list.
// SPEC: FMT_LTB.md §9 (child slots), §10 (clips), §11 (sockets) through dc-model
pub fn read_rig(
    vfs: &RezVfs,
    body: &str,
    child_cache: &mut BTreeMap<String, Vec<ClipFact>>,
) -> Result<Rig, String> {
    let bytes = read_entry(vfs, body)?;
    let m =
        dc_model::Model::parse_without_tracks(&bytes).map_err(|e| format!("body model: {e:?}"))?;
    let folder = body
        .trim()
        .replace('/', "\\")
        .rsplit_once('\\')
        .map(|x| x.0.to_string())
        .unwrap_or_default();
    let mut rig = Rig {
        clips: clip_facts(&m),
        own_clips: m.clips.len(),
        grip_socket: m
            .sockets
            .iter()
            .position(|s| name_key(&s.name.0) == SOCKET_WEAPON)
            .map_or(TODO_WEAPON_SOCKET_FALLBACK, |i| i as u32),
        sockets: m
            .sockets
            .iter()
            .map(|s| String::from_utf8_lossy(&s.name.0).into_owned())
            .collect(),
        tracker_masks: [
            weight_set_of(&m, WEIGHT_SET_UPPER),
            weight_set_of(&m, WEIGHT_SET_LOWER),
        ],
        ..Rig::default()
    };
    for c in &m.child_models {
        let name = c.to_text().replace('/', "\\");
        let path = if name.contains('\\') || folder.is_empty() {
            name
        } else {
            format!("{folder}\\{name}")
        };
        let key = announce_spelling(&path);
        if !child_cache.contains_key(&key) {
            let cb = read_entry(vfs, &path)?;
            let cm = dc_model::Model::parse_without_tracks(&cb)
                .map_err(|e| format!("child model: {e:?}"))?;
            child_cache.insert(key.clone(), clip_facts(&cm));
        }
        rig.clips.extend(child_cache[&key].iter().copied());
        rig.children.push(path);
    }
    let override_extent = m
        .clip_overrides
        .iter()
        .flatten()
        .find(|o| name_key(&o.clip_name.0) == CLIP_LOWER_STAND)
        .map(|o| o.extent);
    rig.stand_extent = override_extent.or_else(|| {
        rig.clips
            .iter()
            .find(|c| c.key == CLIP_LOWER_STAND)
            .map(|c| c.extent)
    });
    Ok(rig)
}

/// Size in bytes of a ROM resource (`file_info` of the file list, CD-59), 0 when unknown.
pub fn resource_size(vfs: &RezVfs, path: &str) -> Option<u32> {
    let (rez, entry) = archive_of(path)?;
    let list = vfs.list_entries(&rez).ok()?;
    list.iter()
        .find(|(n, _, _)| eq_fold(n, &entry))
        .map(|(_, _, s)| *s)
}

/// Everything one character needs on the wire, resolved from the tables and the rig.
#[derive(Debug, Clone, PartialEq)]
pub struct CharacterAssets {
    /// Body model, then its child models.
    pub body: Vec<String>,
    /// Textures of the body model: upper skin, lower skin, then the team mark `MARK_SLOTS` times.
    pub body_textures: Vec<String>,
    pub body_styles: Vec<String>,
    pub face: Option<(String, Vec<String>, Vec<String>)>,
    pub rig: Rig,
    /// The two skins of the mover body (0xa0 flag 0x0001): upper, lower.
    pub mover_skins: Vec<String>,
    /// The body row's hit nodes (empty when the row names no skeleton row) and armour maximum.
    pub hit_nodes: Vec<HitNode>,
    pub max_armor: Option<u32>,
    /// The body row's `GearId` (DSV-430).
    pub gear: Option<u32>,
}

impl CharacterAssets {
    /// Every resource path, deduplicated in first-use order (the order the file list announces them).
    pub fn paths(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut push = |p: &String| {
            if !out.iter().any(|q| eq_fold(q, p)) {
                out.push(p.clone());
            }
        };
        self.body_textures.iter().for_each(&mut push);
        self.body_styles.iter().for_each(&mut push);
        self.body.iter().for_each(&mut push);
        if let Some((m, s, st)) = &self.face {
            push(m);
            s.iter().for_each(&mut push);
            st.iter().for_each(&mut push);
        }
        out
    }
}

/// Builds the assets of a character from the tables and a rig reader.
pub fn character_assets(
    t: &CharTables,
    c: CharacterChoice,
    rig: impl FnOnce(&str) -> Result<Rig, String>,
) -> Result<CharacterAssets, String> {
    let row = t
        .bodies
        .get(&c.model_row)
        .ok_or_else(|| format!("no body row {}", c.model_row))?;
    let (up, low) = row
        .skins
        .get(&c.skin)
        .or_else(|| row.skins.get(&0))
        .cloned()
        .unwrap_or((None, None));
    let skins: Vec<String> = [up, low].into_iter().flatten().collect();
    let mut body_textures = skins.clone();
    if let Some(m) = t.team_marks.get(&TODO_TEAM_MARK_INDEX) {
        body_textures.extend(std::iter::repeat_n(m.clone(), MARK_SLOTS));
    }
    let rig = rig(&row.model)?;
    let mut body = vec![row.model.clone()];
    body.extend(rig.children.iter().cloned());
    let face = t.faces.get(&c.face_row).map(|f| {
        (
            f.model.clone(),
            f.skins.iter().take(1).cloned().collect(),
            f.styles.clone(),
        )
    });
    let hit_nodes = row
        .skeleton
        .and_then(|s| t.skeletons.get(&s))
        .cloned()
        .unwrap_or_default();
    Ok(CharacterAssets {
        body,
        body_textures,
        body_styles: row.styles.clone(),
        face,
        rig,
        mover_skins: skins,
        hit_nodes,
        max_armor: row.max_armor,
        gear: row.gear,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // a synthetic table with the key shape of the ROM tables (made-up values)
    const SYNTH: &str = concat!(
        "[Model0]\n",
        "ModelFile = \"BODIES\\b0.ltb\"\n",
        "RenderStyle0 = \"S\\a.ltb\"\n",
        "RenderStyle1 = \"S\\b.ltb\"\n",
        "RenderStyle2 = \"S\\c.ltb\"\n",
        "UpperSkin0 = \"SK\\u0.dtx\"\n",
        "LowerSkin0 = \"SK\\l0.dtx\"\n",
        "UpperSkin1 = \"SK\\u1.dtx\"\n",
        "LowerSkin1 = \"SK\\l1.dtx\"\n",
        "Skeleton = 1\n",
        "MaxArmor = 150\n",
        "[Face0]\n",
        "ModelFile = \"F\\f0.ltb\"\n",
        "Skin0 = \"F\\f0.dtx\"\n",
        "RenderStyle0 = \"S\\a.ltb\"\n",
        "RenderStyle1 = \"S\\a.ltb\"\n",
        "[Skeleton0]\n",
        "x = 1\n",
        "[Skeleton1]\n",
        "Node0DamageFactor = 1.0\n",
        "Node0Location = 0\n",
        "Node1DamageFactor = 3.5\n",
        "Node1Location = 1\n",
        "Node2DamageFactor = 0.5\n",
        "Node2Location = 4\n",
        "Node4DamageFactor = 9.0\n",
    );
    const MARKS: &str = "[TeamMark]\nTeamName0 = \"SK\\m0.dtx\"\nTeamName1 = \"SK\\m1.dtx\"\n";

    fn rig(_: &str) -> Result<Rig, String> {
        Ok(Rig {
            children: vec!["BODIES\\kid.ltb".into()],
            clips: vec![],
            own_clips: 0,
            grip_socket: 2,
            sockets: Vec::new(),
            stand_extent: None,
            tracker_masks: [None; 2],
        })
    }

    /// DSV-271: the equipment picks the body row (character item, else the vest's row or its face relation) and the
    /// uniform the skin variant.
    #[test]
    fn the_equipment_picks_the_body_row_and_skin() {
        let doc = r#"[Model0]
ModelFile = "B/b0.ltb"
[Model1]
ModelFile = "B/b1.ltb"
FaceRelationModel0 = -1
FaceRelationModel1 = 3
[Model3]
ModelFile = "B/b3.ltb"
[Model7]
ModelFile = "B/b7.ltb"
"#;
        let t = CharTables::from_docs(&ButeDoc::parse(doc), None);
        assert_eq!(t.bodies[&1].face_relation, vec![-1, 3]);
        let vest1 = [(Some(8), 3001), (Some(7), 2002), (Some(0), 5)];
        assert_eq!(loadout_look(&t, &vest1, 0), (Some(1), Some(2)));
        assert_eq!(
            loadout_look(&t, &vest1, 1),
            (Some(3), Some(2)),
            "face relation"
        );
        // a character item wins when the table has its row
        let with_char = [(Some(8), 3001), (Some(14), 10007)];
        assert_eq!(loadout_look(&t, &with_char, 0), (Some(7), None));
        let unknown_char = [(Some(8), 3001), (Some(14), 10009)];
        assert_eq!(loadout_look(&t, &unknown_char, 0), (Some(1), None));
        // a vest row the table lacks: nothing
        assert_eq!(loadout_look(&t, &[(Some(8), 3005)], 0), (None, None));
        // DSV-474: no vest is vest row 0, through its face relation when it has one
        assert_eq!(loadout_look(&t, &[], 0), (Some(0), None));
        assert_eq!(loadout_look(&t, &[(Some(0), 5)], 0), (Some(0), None));
        // DSV-420: the voice row is the named row, else the equipment's body row (the mercenary item)
        assert_eq!(voice_row(&t, Some(3), Some(&with_char), 0), Some(3));
        assert_eq!(voice_row(&t, None, Some(&with_char), 0), Some(7));
        assert_eq!(voice_row(&t, None, Some(&vest1), 0), Some(1));
        assert_eq!(voice_row(&t, None, None, 0), None);
    }

    #[test]
    fn rows_and_assets() {
        let t = CharTables::from_docs(&ButeDoc::parse(SYNTH), Some(&ButeDoc::parse(MARKS)));
        assert_eq!(t.bodies.len(), 1);
        assert_eq!(t.bodies[&0].styles.len(), 3);
        assert_eq!(t.bodies[&0].skins.len(), 2);
        assert_eq!(t.faces[&0].styles.len(), 2);
        assert_eq!(t.team_marks.len(), 2);
        // DSV-30: the body row's skeleton (nodes in order until the first gap) and armour maximum
        assert_eq!(
            (t.bodies[&0].skeleton, t.bodies[&0].max_armor),
            (Some(1), Some(150))
        );
        assert!(t.skeletons[&0].is_empty());
        assert_eq!(
            t.skeletons[&1],
            vec![
                HitNode {
                    factor: 1.0,
                    location: 0
                },
                HitNode {
                    factor: 3.5,
                    location: 1
                },
                HitNode {
                    factor: 0.5,
                    location: 4
                },
            ]
        );
        let a = character_assets(
            &t,
            CharacterChoice {
                skin: 1,
                ..Default::default()
            },
            rig,
        )
        .unwrap();
        assert_eq!(
            a.body,
            vec!["BODIES\\b0.ltb".to_string(), "BODIES\\kid.ltb".to_string()]
        );
        assert_eq!(a.body_textures.len(), 2 + MARK_SLOTS);
        assert_eq!(a.body_textures[0], "SK\\u1.dtx");
        assert_eq!((a.hit_nodes.len(), a.max_armor), (3, Some(150)));
        assert_eq!(a.body_textures[2], "SK\\m0.dtx");
        assert_eq!(
            a.mover_skins,
            vec!["SK\\u1.dtx".to_string(), "SK\\l1.dtx".to_string()]
        );
        // a missing skin variant falls back to variant 0; a missing row is an error
        let a = character_assets(
            &t,
            CharacterChoice {
                skin: 9,
                ..Default::default()
            },
            rig,
        )
        .unwrap();
        assert_eq!(a.body_textures[0], "SK\\u0.dtx");
        assert!(character_assets(
            &t,
            CharacterChoice {
                model_row: 3,
                ..Default::default()
            },
            rig
        )
        .is_err());
        // paths are deduplicated (the face styles repeat a body style)
        let p = a.paths();
        assert_eq!(p.iter().filter(|x| x.as_str() == "S\\a.ltb").count(), 1);
        assert_eq!(p.iter().filter(|x| x.as_str() == "SK\\m0.dtx").count(), 1);
    }

    /// DSV-430: the body, head-gear and goggle rows' `GearId` (any key case) and the rows a player's equipment wears.
    #[test]
    fn worn_gear_rows_come_from_body_head_and_goggle() {
        let models = ButeDoc::parse(concat!(
            "[Model0]
ModelFile = \"B\\a.ltb\"
GearID = 0
",
            "[Model1]
ModelFile = \"B\\b.ltb\"
GearId = 6
",
            "[Model2]
ModelFile = \"B\\c.ltb\"
",
            "[Hair0]
Name = \"none\"
",
            "[Hair5]
Name = \"helmet\"
GearID = 3
",
            "[Hair6]
Name = \"bad\"
GearID = -1
",
        ));
        let mut t = CharTables::from_docs(&models, None);
        t.add_attachments(&ButeDoc::parse(
            "[Goggle3]
Name = \"mask\"
GearID = 5
[Backpack3]
GearID = 9
",
        ));
        assert_eq!(
            (t.bodies[&0].gear, t.bodies[&1].gear, t.bodies[&2].gear),
            (Some(0), Some(6), None)
        );
        assert_eq!(t.hair_gear, [(5, 3)].into());
        assert_eq!(t.goggle_gear, [(3, 5)].into());
        let eq = [(Some(4u8), 4005u32), (Some(6), 5003), (Some(0), 145)];
        assert_eq!(t.worn_gear(Some(1), &eq), [Some(6), Some(3), Some(5)]);
        assert_eq!(t.worn_gear(Some(2), &[(Some(4), 4000)]), [None, None, None]);
        assert_eq!(t.worn_gear(None, &[]), [None, None, None]);
    }

    /// ROM-gated (DSV-430): the default body row names the calibrated armour row 0 and head row 0 none; the vest
    /// bodies, helmets and gas masks name rows of their own. Output: counts only.
    #[test]
    fn rom_gear_ids_of_bodies_heads_and_goggles() {
        let rom = dc_rez::dc_rom::rom_or_skip!();
        let vfs = RezVfs::new(&rom);
        let t = CharTables::from_vfs(&vfs).unwrap();
        assert_eq!(t.bodies[&0].gear, Some(0));
        assert!(!t.hair_gear.contains_key(&0));
        let bodies = t.bodies.values().filter(|b| b.gear.is_some()).count();
        let distinct: std::collections::BTreeSet<u32> =
            t.bodies.values().filter_map(|b| b.gear).collect();
        assert!(distinct.len() > 3, "{distinct:?}");
        assert!(t.hair_gear.len() > 5 && !t.goggle_gear.is_empty());
        eprintln!(
            "bodies {bodies}, heads {}, goggles {}",
            t.hair_gear.len(),
            t.goggle_gear.len()
        );
    }

    #[test]
    fn archive_and_spelling() {
        assert_eq!(
            archive_of("BODIES\\x\\b.ltb"),
            Some(("BODIES.rez".into(), "BODIES\\x\\b.ltb".into()))
        );
        assert_eq!(
            archive_of("/A/b.dtx"),
            Some(("A.rez".into(), "A\\b.dtx".into()))
        );
        assert_eq!(archive_of("loose.dtx"), None);
        assert_eq!(announce_spelling("a/b.Ltb"), "A\\B.LTB");
        let camo = "@camo:paint:25:HHSkin0:keep=37,30,25:@mod:pack/assets/candy.dtx";
        assert_eq!(announce_spelling(camo), camo);
        assert_eq!(name_key(b"abc").1, 3);
    }

    /// ROM-gated (DSV-28 refinement): on every player rig of the ROM the 24 direction clips and the crouched stand
    /// resolve by key; every run clip carries footstep keys, no walk or crouch clip and no stand clip does (the
    /// reason a walk-clip character is silent on the 2009 client). Output: counts only.
    #[test]
    fn rom_run_clips_carry_the_footstep_keys() {
        let rom = dc_rez::dc_rom::rom_or_skip!();
        let vfs = RezVfs::new(&rom);
        let t = CharTables::from_vfs(&vfs).unwrap();
        let mut cache = RigCache::default();
        let mut rigs = 0;
        for (n, row) in &t.bodies {
            let Ok(rig) = cache.rig(&vfs, &row.model) else {
                continue;
            };
            let steps = |k: NameKey| {
                let (i, _) = rig
                    .clip(k)
                    .unwrap_or_else(|| panic!("body row {n}: a direction clip is missing"));
                rig.clips[i as usize].footsteps
            };
            for k in CLIP_LOWER_RUN {
                assert!(steps(k) >= 1, "body row {n}: a run clip without footsteps");
            }
            for k in CLIP_LOWER_WALK_DIRS
                .iter()
                .chain(CLIP_LOWER_CROUCH.iter())
                .chain([CLIP_LOWER_CROUCH_STAND, CLIP_LOWER_STAND].iter())
            {
                assert_eq!(
                    steps(*k),
                    0,
                    "body row {n}: footsteps on a walk / crouch / stand clip"
                );
            }
            rigs += 1;
        }
        println!("{rigs} player rigs: run clips step, walk and crouch clips do not");
        assert!(rigs > 0);
    }

    /// ROM-gated: every body row, its skins, its child models, the faces and the team marks resolve in the ROM,
    /// every rig has the four clips and the weapon socket. Output: counts and hashes only.
    #[test]
    fn rom_every_character_resolves() {
        let rom = dc_rez::dc_rom::rom_or_skip!();
        let vfs = RezVfs::new(&rom);
        let t = CharTables::from_vfs(&vfs).unwrap();
        assert!(!t.bodies.is_empty() && !t.faces.is_empty() && !t.team_marks.is_empty());
        let mut cache = RigCache::default();
        let mut files = 0usize;
        for (n, row) in &t.bodies {
            let rig = cache
                .rig(&vfs, &row.model)
                .unwrap_or_else(|e| panic!("body row {n}: {e}"));
            for key in [
                CLIP_UPPER_STAND,
                CLIP_UPPER_WALK,
                CLIP_LOWER_STAND,
                CLIP_LOWER_WALK,
            ] {
                assert!(
                    rig.clip(key).is_some(),
                    "body row {n}: a player clip is missing"
                );
            }
            let e = rig.stand_extent.expect("a stand box");
            assert!(
                e.iter().all(|x| x.is_finite() && *x > 0.0),
                "body row {n}: stand box"
            );
            for k in row.skins.keys() {
                let a = character_assets(
                    &t,
                    CharacterChoice {
                        model_row: *n,
                        skin: *k,
                        face_row: 0,
                    },
                    |_| Ok(rig.clone()),
                )
                .unwrap();
                for p in a.paths() {
                    assert!(
                        resource_size(&vfs, &p).is_some(),
                        "body row {n} skin {k}: {} missing",
                        short_hash(&p)
                    );
                    files += 1;
                }
            }
        }
        // the ROM names face models it does not carry; the server leaves such a face out (character_assets callers
        // check every path), so missing faces are counted, and the default face row must resolve
        let mut missing_faces = Vec::new();
        for (n, f) in &t.faces {
            let ok = std::iter::once(&f.model)
                .chain(f.skins.iter().take(1))
                .chain(f.styles.iter())
                .all(|p| resource_size(&vfs, p).is_some());
            if !ok {
                missing_faces.push(*n);
            }
        }
        assert!(
            !missing_faces.contains(&0),
            "the default face row does not resolve"
        );
        println!("ROM face rows with a missing file: {missing_faces:?}");
        for m in t.team_marks.values() {
            assert!(resource_size(&vfs, m).is_some());
        }
        println!(
            "ROM characters: {} body rows, {} faces, {} team marks, {} distinct body models, {} resource checks",
            t.bodies.len(),
            t.faces.len(),
            t.team_marks.len(),
            cache.rigs.len(),
            files
        );
    }
}
