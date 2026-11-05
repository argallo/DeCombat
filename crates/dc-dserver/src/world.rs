//! The room's world on the server side: read from the user's ROM archive (or a file for tests), parsed with
//! `dc-world` without render data, reduced to what slice 1 needs: the announced client path, the file size, the
//! world box and the player spawn points.

use std::path::Path;
use std::sync::Arc;

use dc_sim::CollisionWorld;
use dc_world::{names, Key, ObjectRole, ParseOptions, PropKey, World, WORLD_VERSION};

/// Name of the world archive inside `<rom>/Game` (the same archive `dc-bute` lists world names from).
// SPEC: LOBBY_HARNESS.md row 15 (world mapping through the ROM world archive); OBSERVED: dc-bute world_names
pub const WORLD_ARCHIVE: &str = "WORLDS.rez";
/// Extension of world entries in the archive.
// SPEC: FMT_DAT85.md §1 (version-85 `.dat` world files)
pub const WORLD_EXT: &str = ".DAT";

/// How the world's path is announced in `S2C_FILE_LIST`. The exact form the client's resource lookup accepts
/// (separator, case, extension) is UNPROVEN; the spec asks for the path exactly as `dc-rez` lists it in the
/// world archive and a capture (V-A2) to confirm. A wrong form only fails at `S2C_LOAD_WORLD` (result 0x29).
// SPEC: UDP_SESSION_2009_A.md §4.4, §9.1 P2, §10.1 item 3
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameForm {
    /// The archive entry path as listed (default).
    Listed,
    /// The listed path without its extension.
    NoExtension,
    /// The `-world` argument as the lobby passed it.
    Argument,
}

/// The default announce form.
// SPEC: UDP_SESSION_2009_A.md §4.4 (UNPROVEN, V-A2)
pub const TODO_WORLD_NAME_FORM: NameForm = NameForm::Listed;

/// One player spawn point of the world.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnPoint {
    pub pos: [f32; 3],
    /// Euler angles in radians (FMT_DAT85.md §6.1 property 2).
    pub rot: [f32; 3],
    /// Team filter text read from the user's file (the ROM team name, or `"1"` / `"2"` on a community map, MAP-22).
    pub team_filter: Option<Vec<u8>>,
}

/// Whether an object is written in DeCombat's own map vocabulary: its role's class key matched through the alias
/// half (`dc.player_spawn` and the other `dc.*` names, MAP-1), not through the ROM hash. An object without a role
/// is never a vocabulary object here (sounds, sprites and lights carry no mode or team rule).
// DC-DEFAULT: MAP-25
pub fn is_dc_object(o: &dc_world::Object<'_>) -> bool {
    o.role
        .is_some_and(|r| r.class_key().matches_ours(o.class_name))
}

/// What the dedicated server keeps of a world.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldInfo {
    /// Client resource path announced in `S2C_FILE_LIST`.
    pub announce: String,
    /// Display token for `@ready world=` (no spaces).
    pub display: String,
    /// File size in bytes (`file_info`, CD-59 proposed).
    pub size: u32,
    pub extent_min: [f32; 3],
    pub extent_max: [f32; 3],
    pub spawns: Vec<SpawnPoint>,
    pub objects: usize,
    /// SHA-256 of the world bytes (logs; identifies the file without naming ROM content).
    pub sha256: String,
    /// The geometry a reported shot is traced against (slice 4): the level model, which the client always has.
    /// `None` when the world has no level model.
    pub trace: Option<Arc<CollisionWorld>>,
    /// The surface id of each world polygon over every BSP model (V-SURF-1: the shot and blast surface byte;
    /// DSV-62, proposed). `None` when the world has no model.
    pub surfaces: Option<Arc<crate::surface::SurfaceProbe>>,
    /// Objective zones (bomb sites, flag capture zones) and capture flags of the world file (slice 4).
    pub zones: Vec<ZoneInfo>,
    pub flags: Vec<FlagInfo>,
    /// Objects linked to a named BSP model (world models, breakables, doors, sky).
    pub brushes: Vec<BrushInfo>,
    /// Kill volumes and ladders (named volumes of the world file, DSV-37 extended).
    pub volumes: Vec<VolumeInfo>,
    /// Scaled sprites (light coronas and the like).
    pub sprites: Vec<SpriteInfo>,
    /// Named client effects placed in the world.
    pub effects: Vec<EffectInfo>,
    /// Placed world sounds (looping ambiences and the like).
    pub sounds: Vec<SoundInfo>,
    /// The world's command texts: start-up commands, triggers and mode controllers.
    pub scripts: WorldScripts,
    /// The world's global directional light (DSV-40, proposed); `None` = the world has none.
    pub global_light: Option<crate::worldlight::GlobalLight>,
    /// The occluder set of the visibility culling (VIS-3, proposed); `None` when the world has no polygon for it.
    pub occluders: Option<Arc<crate::visibility::Occluders>>,
    /// The co-op objects of the world file (spawners, AI templates, AI nodes); empty in the other worlds.
    pub coop: CoopWorld,
    /// The world-settings variables (DSV-47, proposed): names and values from the world file, names the file
    /// cannot give are filled from the ROM by [`load_from_rom`].
    pub world_vars: Vec<crate::evars::WorldVar>,
    /// The mode filter text of each player spawn point (index-aligned with `spawns`; DSV-50, proposed).
    pub spawn_modes: Vec<Option<Vec<u8>>>,
    /// Whether each player spawn point is a DeCombat-vocabulary object (index-aligned with `spawns`; an empty mode
    /// filter on such a point serves every mode, MAP-21).
    pub spawn_dc: Vec<bool>,
    /// The name of each player spawn point (index-aligned with `spawns`; empty without one): the target of the
    /// world statements that lock and unlock it (DSV-569).
    pub spawn_names: Vec<Vec<u8>>,
    /// Whether the file marks each player spawn point locked (index-aligned with `spawns`; DSV-569): a locked point
    /// is no spawn candidate until a statement unlocks it.
    pub spawn_locked: Vec<bool>,
    /// The closed models of the kill volumes (the server's kill test, DSV-52, proposed).
    pub kill_shapes: Vec<crate::killvol::VolumeShape>,
    /// The hazard volumes that hurt a player inside them (DSV-607, proposed).
    pub hazards: Vec<crate::killvol::HazardShape>,
    /// The sliding doors (DSV-46, proposed).
    pub doors: Vec<crate::doors::DoorInfo>,
    /// The rotating doors players may open and the rotating switches (w1-doors DOOR-4, proposed).
    pub turns: Vec<crate::doors::TurnInfo>,
    /// The trigger objects the doors' and switches' statements fire (DSV-562, proposed).
    pub triggers: Vec<crate::doors::ScriptTrigger>,
    /// Names of the client variables the server pushes (DSV-53, proposed), filled from the user's ROM by
    /// [`load_from_rom`]; empty for a world loaded from bytes.
    pub client_vars: crate::clientvars::ClientVarNames,
    /// The breakable models and explosion objects (DSV-115, proposed; world-objects).
    pub destructibles: crate::destructibles::DestructibleWorld,
    /// The Spy Hunt places: intel spawn points and the uplink box (DSV-230).
    pub spy: SpyWorld,
    /// The event counters, timers and activation boxes the world's statements address (DSV-572, DSV-573).
    pub mission: crate::script::MissionObjects,
}

/// The Spy Hunt objects of a world (mode class 9): the item spawn points of the mode (the intel places) and its
/// activation box (the uplink where a player with all the intel transmits it).
// OBSERVED: ROM world files (2026-10-28 scratch survey of the seven Spy Hunt worlds over dc-world: 10..19 item spawn
// points whose mode filter names the Spy Hunt mode, one activation box of that mode with a team, a box extent and a
// time amount of 15.0, beside the uplink effect objects)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpyWorld {
    pub intel_points: Vec<[f32; 3]>,
    pub uplinks: Vec<UplinkInfo>,
    /// The world's trigger objects and their numbered commands (the stage-2 station scripts, DSV-323).
    pub triggers: Vec<(Vec<u8>, Vec<Vec<u8>>)>,
}

/// One uplink box.
#[derive(Debug, Clone, PartialEq)]
pub struct UplinkInfo {
    pub pos: [f32; 3],
    /// The box extent property (half extents).
    pub dims: [f32; 3],
    /// The time amount property (seconds of the transmission).
    pub secs: Option<f32>,
    pub team_text: Option<Vec<u8>>,
    pub name: Vec<u8>,
    /// The box's lock at the start, and its command texts: while a player works it, when he lets go, when the time
    /// is done (DSV-323).
    pub locked: bool,
    pub work: Vec<u8>,
    pub release: Vec<u8>,
    pub activate: Vec<u8>,
}

/// The mode filter value of the Spy Hunt objects, the activation box class and its time property (hash keys).
// OBSERVED: values, class and property names of the ROM world files (hashes only; scratch survey 2026-10-28)
pub const MODE_FILTER_SPY_HUNT: PropHash = ([0xea, 0x6e, 0xad, 0x9b, 0x6f, 0x44, 0x98, 0x48], 12);
pub const CLASS_ACTIVATE_BOX: PropHash = ([0x86, 0xea, 0x23, 0x46, 0x76, 0xd3, 0xa4, 0x8e], 15);
pub const PROP_TIME_AMOUNT: PropHash = ([0x09, 0x8f, 0x31, 0xe1, 0xd9, 0x2b, 0xb9, 0x71], 10);
/// The activation box's lock and its three command properties (hash keys).
// OBSERVED: property names of the ROM world files' activation boxes (hashes only; modes-r13 survey of WARHEAD: the
// box starts locked, its work / release commands switch the station effects, its activate command fires the
// finish trigger, locks the box and tells the mode controller the defence won)
pub const PROP_LOCKED: PropHash = ([0xa4, 0x24, 0xe3, 0x3d, 0x90, 0x93, 0x1d, 0x1e], 6);
pub const PROP_WORK_COMMAND: PropHash = ([0x8a, 0x47, 0xec, 0xc5, 0x78, 0x03, 0xaf, 0xc9], 11);
pub const PROP_RELEASE_COMMAND: PropHash = ([0x81, 0xd7, 0x59, 0x72, 0x31, 0xc2, 0xd5, 0x02], 14);
pub const PROP_ACTIVATE_COMMAND: PropHash = ([0xcd, 0x33, 0x59, 0x58, 0xab, 0x75, 0x72, 0xcb], 15);

/// The Spy Hunt objects of a parsed world.
// DC-DEFAULT: DSV-230 (proposed) the mode's item spawn points are the intel places, its activation box the uplink
pub fn spy_objects(w: &World<'_>) -> SpyWorld {
    let mut s = SpyWorld::default();
    let spy_mode = |o: &dc_world::Object<'_>| {
        o.text(PropKey::ModeFilter).map(key_of) == Some(MODE_FILTER_SPY_HUNT)
    };
    for o in &w.objects {
        let Some(pos) = o.position() else {
            continue;
        };
        if !spy_mode(o) {
            continue;
        }
        if o.role == Some(ObjectRole::ItemSpawn) {
            s.intel_points.push(pos);
        } else if key_of(o.class_name) == CLASS_ACTIVATE_BOX {
            s.uplinks.push(UplinkInfo {
                pos,
                dims: o.vec3(PropKey::BoxExtent).unwrap_or([32.0; 3]),
                secs: prop_real(o, PROP_TIME_AMOUNT),
                team_text: o.text(PropKey::Team).map(<[u8]>::to_vec),
                name: o.name().map(<[u8]>::to_vec).unwrap_or_default(),
                locked: prop_bool(o, PROP_LOCKED).unwrap_or(false),
                work: prop_text(o, PROP_WORK_COMMAND).unwrap_or_default().to_vec(),
                release: prop_text(o, PROP_RELEASE_COMMAND)
                    .unwrap_or_default()
                    .to_vec(),
                activate: prop_text(o, PROP_ACTIVATE_COMMAND)
                    .unwrap_or_default()
                    .to_vec(),
            });
        }
    }
    s
}

/// World-object classes of the co-op worlds, by class-name hash (SHA-256 prefix and length): spawner, placed AI
/// character (the templates spawners and spawn tables name), and the AI patrol, guard and use-object nodes.
// SPEC: FIRETEAM_2009.md §3.4 (class hashes and roles; OBSERVED: ROM)
pub const CLASS_SPAWNER: PropHash = ([0x89, 0xbb, 0x4e, 0xfe, 0x41, 0x51, 0xb8, 0xb5], 7);
pub const CLASS_AI_CHARACTER: PropHash = ([0xa5, 0x8c, 0x4b, 0xb4, 0xde, 0xed, 0x79, 0xbb], 8);
pub const CLASS_AI_PATROL_NODE: PropHash = ([0x5d, 0x07, 0x5f, 0xec, 0x0f, 0xbe, 0xc2, 0xea], 12);
pub const CLASS_AI_GUARD_NODE: PropHash = ([0x43, 0xc6, 0xe3, 0x71, 0x08, 0x9f, 0x55, 0x53], 11);
pub const CLASS_AI_USE_NODE: PropHash = ([0xe0, 0x7d, 0x42, 0xa5, 0xd2, 0x72, 0x38, 0xd6], 15);
/// Properties of those classes (hash keys): the spawner's target (the name of the AI template it spawns), the AI
/// character's body row (the `[AIModel<n>]` row whose `Name` it equals), its right-hand weapon (a `[Weapon<n>]`
/// `Name`), its health maximum, and its template flag.
// OBSERVED: property names of the ROM co-op worlds' spawner and AI character objects (hashes only; 260 / 40
// spawners, 266 / 12 AI characters; every AI character names an AIModel row, every Map13 spawner target names an AI
// character, 2026-10-25 scratch run over dc-world)
pub const PROP_SPAWN_TARGET: PropHash = ([0x97, 0x83, 0x54, 0xdb, 0x0c, 0x00, 0xfc, 0x78], 6);
pub const PROP_AI_BODY_ROW: PropHash = ([0xf4, 0x06, 0xea, 0x5c, 0xa1, 0x30, 0xff, 0xb5], 13);
pub const PROP_AI_RIGHT_HAND: PropHash = ([0xc9, 0x69, 0xc8, 0xaa, 0x86, 0x33, 0x33, 0x73], 9);
pub const PROP_AI_MAX_HEALTH: PropHash = ([0xb6, 0x97, 0x3b, 0x50, 0x18, 0x1c, 0x00, 0xcf], 12);
pub const PROP_TEMPLATE: PropHash = ([0x05, 0x75, 0xf2, 0x9d, 0xf8, 0x88, 0xa2, 0x7e], 8);
/// The AI character's brain row (an `AIBUTES.TXT` `[Brain<n>]` `Name`) and its run-speed override (a text property,
/// empty on every ROM AI character: the attribute row's speed applies, DSV-588).
// OBSERVED: property names of the ROM co-op worlds' AI characters (hashes only; every AI character of both worlds
// names a brain row; the Cabin Fever templates name the four melee-only brains, the Desert Thunder ones ranged
// brains; the run-speed text is empty on all 278; 2026-11-05 scratch run over dc-world)
pub const PROP_AI_BRAIN: PropHash = ([0x7e, 0x4d, 0x5c, 0x46, 0x0c, 0x04, 0x45, 0x7e], 5);
pub const PROP_AI_RUN_SPEED: PropHash = ([0xc7, 0x42, 0xc5, 0xef, 0x3b, 0xcd, 0x87, 0x34], 8);
/// The word the ROM AI characters' attachment properties hold for "nothing in this slot".
// OBSERVED: ROM co-op worlds (the right-hand property of the placed live AI characters, the other attachment slots)
pub const AI_SLOT_EMPTY: &str = "<none>";

/// One spawner object of a co-op world.
#[derive(Debug, Clone, PartialEq)]
pub struct CoopSpawner {
    pub name: Vec<u8>,
    pub pos: [f32; 3],
    pub rot: [f32; 3],
    /// The name of the AI character object it spawns.
    pub target: Option<Vec<u8>>,
}

/// One placed AI character of a co-op world (in the ROM worlds nearly all are templates for spawners).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AiTemplate {
    pub name: Vec<u8>,
    pub pos: [f32; 3],
    pub rot: [f32; 3],
    /// The `[AIModel<n>]` row named by its body-row property.
    pub body_row: Option<String>,
    /// The `[Weapon<n>]` `Name` of its right-hand weapon (`None`: the slot is empty, [`AI_SLOT_EMPTY`]).
    pub right_hand: Option<String>,
    pub max_health: Option<f32>,
    pub template: bool,
    /// The command its death runs (an AI spawned from it runs it too, DSV-571).
    pub death_cmd: Vec<u8>,
    /// The `AIBUTES.TXT` `[Brain<n>]` row named by its brain property (DSV-589).
    pub brain: Option<String>,
    /// Its own run speed (units per second), when the property holds a number (DSV-588).
    pub run_speed: Option<f32>,
}

/// The co-op objects of a world.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CoopWorld {
    pub spawners: Vec<CoopSpawner>,
    pub templates: Vec<AiTemplate>,
    /// Positions of the AI patrol, guard and use-object nodes (waypoints of the AI steering).
    pub nodes: Vec<[f32; 3]>,
    /// The world's statements spawn its AI from its spawners (a mission world, DSV-571): the co-op rules follow the
    /// world's trigger network instead of a wave plan.
    pub mission: bool,
    /// The round loop of a round world (DSV-576): its spawn-table rounds follow the world's round counter.
    pub rounds: Option<crate::script::RoundNet>,
    /// The caption and message triggers the statements `play` dialogue texts on (DSV-579).
    pub texts: Vec<TextTrigger>,
    /// The music triggers the statements switch on and off (DSV-581).
    pub music: Vec<MusicTrigger>,
    /// The named item spawn points (the places the GMS weapon rows name, DSV-583).
    pub item_points: Vec<ItemPoint>,
}

/// A caption trigger (a dialogue caption with picture and speaker) or a message trigger (a banner text) of a world.
#[derive(Debug, Clone, PartialEq)]
pub struct TextTrigger {
    pub name: Vec<u8>,
    /// A caption trigger (else a message trigger).
    pub caption: bool,
    /// Its show-time property (seconds).
    pub secs: Option<f32>,
}

/// A music trigger of a world: the track file (relative to the client's game folder), looping, fade seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct MusicTrigger {
    pub name: Vec<u8>,
    pub file: Vec<u8>,
    pub looped: bool,
    pub fade: Option<f32>,
}

/// A named item spawn point.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemPoint {
    pub name: Vec<u8>,
    pub pos: [f32; 3],
    pub yaw: f32,
}

/// The text and music trigger classes of the co-op worlds and their properties (hash keys).
// OBSERVED: class and property names of the ROM co-op worlds (hashes only; SANDHOG_CP1: one caption trigger, one
// message trigger, five music triggers; CabinFever: one caption trigger, three music triggers; every caption and
// message trigger carries a real show time, every music trigger a file name, a fade time and a loop flag)
pub const CLASS_CAPTION_TRIGGER: PropHash = ([0x6d, 0x0d, 0x76, 0xd5, 0xa7, 0x91, 0x88, 0xc0], 14);
pub const CLASS_MESSAGE_TRIGGER: PropHash = ([0x45, 0xc6, 0xa1, 0xb8, 0x2c, 0xfa, 0x66, 0x03], 14);
pub const CLASS_MUSIC_TRIGGER: PropHash = ([0x93, 0xb5, 0x8e, 0x30, 0x28, 0xd9, 0xbc, 0xf0], 10);
pub const PROP_SHOW_TIME: PropHash = ([0xeb, 0x17, 0xd9, 0xdb, 0x42, 0x10, 0x7e, 0xd2], 8);
pub const PROP_MUSIC_FILE: PropHash = ([0xe3, 0x16, 0xd2, 0xad, 0x49, 0xb2, 0x58, 0x6a], 8);
pub const PROP_MUSIC_FADE: PropHash = ([0xf7, 0xa5, 0x68, 0xb4, 0xef, 0xae, 0xf3, 0xcc], 10);
pub const PROP_MUSIC_LOOP: PropHash = ([0xf2, 0xf6, 0xa0, 0x18, 0x89, 0x95, 0x1e, 0xdc], 4);

impl CoopWorld {
    pub fn is_empty(&self) -> bool {
        self.spawners.is_empty() && self.templates.is_empty()
    }

    /// The AI template of this name (ASCII case-insensitive).
    pub fn template(&self, name: &[u8]) -> Option<&AiTemplate> {
        self.templates
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
    }
}

fn text_string(t: Option<&[u8]>) -> Option<String> {
    t.filter(|t| !t.is_empty())
        .map(|t| String::from_utf8_lossy(t).trim().to_string())
        .filter(|t| !t.is_empty())
}

/// The co-op objects of a parsed world (classes and properties by hash).
// SPEC: FIRETEAM_2009.md §3.3 (the client reads none of these objects: server-only data), §3.4
pub fn coop_objects(w: &World<'_>) -> CoopWorld {
    let mut c = CoopWorld::default();
    for o in &w.objects {
        let class = key_of(o.class_name);
        let pos = o.position().unwrap_or([0.0; 3]);
        let rot = o.rotation().unwrap_or([0.0; 3]);
        if class == CLASS_SPAWNER {
            c.spawners.push(CoopSpawner {
                name: o.name().map(<[u8]>::to_vec).unwrap_or_default(),
                pos,
                rot,
                target: prop_text(o, PROP_SPAWN_TARGET)
                    .filter(|t| !t.is_empty())
                    .map(<[u8]>::to_vec),
            });
        } else if class == CLASS_AI_CHARACTER {
            let max_health = match prop(o, PROP_AI_MAX_HEALTH) {
                Some(dc_world::FieldValue::Real(r)) if r.is_finite() && *r > 0.0 => Some(*r),
                _ => None,
            };
            c.templates.push(AiTemplate {
                name: o.name().map(<[u8]>::to_vec).unwrap_or_default(),
                pos,
                rot,
                body_row: text_string(prop_text(o, PROP_AI_BODY_ROW)),
                right_hand: text_string(prop_text(o, PROP_AI_RIGHT_HAND))
                    .filter(|h| !h.eq_ignore_ascii_case(AI_SLOT_EMPTY)),
                max_health,
                template: prop_bool(o, PROP_TEMPLATE).unwrap_or(false),
                death_cmd: prop_text(o, crate::destructibles::PROP_DEATH_COMMAND)
                    .unwrap_or_default()
                    .to_vec(),
                brain: text_string(prop_text(o, PROP_AI_BRAIN))
                    .filter(|b| !b.eq_ignore_ascii_case(AI_SLOT_EMPTY)),
                run_speed: text_string(prop_text(o, PROP_AI_RUN_SPEED))
                    .and_then(|s| s.parse::<f32>().ok())
                    .or_else(|| prop_real(o, PROP_AI_RUN_SPEED))
                    .filter(|s| s.is_finite() && *s > 0.0),
            });
        } else if class == CLASS_AI_PATROL_NODE
            || class == CLASS_AI_GUARD_NODE
            || class == CLASS_AI_USE_NODE
        {
            c.nodes.push(pos);
        } else if class == CLASS_CAPTION_TRIGGER || class == CLASS_MESSAGE_TRIGGER {
            c.texts.push(TextTrigger {
                name: o.name().map(<[u8]>::to_vec).unwrap_or_default(),
                caption: class == CLASS_CAPTION_TRIGGER,
                secs: prop_real(o, PROP_SHOW_TIME).filter(|s| *s > 0.0),
            });
        } else if class == CLASS_MUSIC_TRIGGER {
            if let Some(file) = prop_text(o, PROP_MUSIC_FILE).filter(|f| !f.is_empty()) {
                c.music.push(MusicTrigger {
                    name: o.name().map(<[u8]>::to_vec).unwrap_or_default(),
                    file: file.to_vec(),
                    looped: prop_bool(o, PROP_MUSIC_LOOP).unwrap_or(false),
                    fade: prop_real(o, PROP_MUSIC_FADE).filter(|s| *s >= 0.0),
                });
            }
        } else if o.role == Some(ObjectRole::ItemSpawn) {
            if let Some(name) = o.name().filter(|n| !n.is_empty()) {
                c.item_points.push(ItemPoint {
                    name: name.to_vec(),
                    pos,
                    yaw: rot[1],
                });
            }
        }
    }
    c
}

/// Classes and properties of the mission objects beside the triggers (hash keys): the event counter (start value,
/// per event the value and the command run when the counter gets there), the timer (its in-time and over-time
/// commands; its time is the activation box's time property).
// OBSERVED: class and property names of the ROM co-op worlds (hashes only; SANDHOG_CP1: one counter whose first event
// unlocks the final activation box after seven counts, one timer started by a checkpoint and stopped by the hostage
// room's door, four activation boxes)
pub const CLASS_EVENT_COUNTER: PropHash = ([0x6c, 0x64, 0xa5, 0x54, 0x61, 0xd6, 0x5e, 0x44], 12);
pub const CLASS_TIMER: PropHash = ([0x4c, 0x7e, 0xb7, 0x99, 0x39, 0xe9, 0x93, 0xf1], 12);
pub const PROP_COUNTER_START: PropHash = ([0x95, 0xd8, 0x70, 0x1a, 0x61, 0x3e, 0x02, 0x72], 13);
pub const PROP_IN_TIME_COMMAND: PropHash = ([0x55, 0x1f, 0xe2, 0x90, 0x9d, 0x79, 0x07, 0xf4], 14);
pub const PROP_OVER_TIME_COMMAND: PropHash = ([0xcf, 0x73, 0x9f, 0xde, 0x20, 0x49, 0x0b, 0xd1], 16);
/// The counter's events: (value, command) property keys (the ROM counters use up to 30).
pub const PROP_COUNTER_EVENTS: [(PropHash, PropHash); 32] = [
    (h8(0x207a7e0ece2a173b, 11), h8(0x149687a8df3c0a4d, 17)),
    (h8(0x82088d083e75cc1b, 11), h8(0x0c451db7ed6ddfa6, 17)),
    (h8(0xb1085aa1b7196279, 11), h8(0x0a6be5a9fc2235bd, 17)),
    (h8(0x2b3d35aca9444083, 11), h8(0xd32a3e34531301c2, 17)),
    (h8(0xf7b44b980b5cd6a6, 11), h8(0xd0e26fd58d1cf41a, 17)),
    (h8(0xdf049342947b621e, 11), h8(0xd5e0396bcd67f7b3, 17)),
    (h8(0xd0f21fca2a40fa2c, 11), h8(0x3a0daed6ca9fcac7, 17)),
    (h8(0x5ca9877e739f9c58, 11), h8(0x39bf4d60e5da0181, 17)),
    (h8(0xe6fe54a927a1237b, 11), h8(0xbd96b8f98d319560, 17)),
    (h8(0x202134b3fc00fe2b, 12), h8(0x3b39d3821b6498ab, 18)),
    (h8(0x644b8ef404dfdf97, 12), h8(0xd5b7b5405f4474e5, 18)),
    (h8(0x4a2f8fb12ff9b38d, 12), h8(0xf4c61c8a575150cc, 18)),
    (h8(0x7bc281262b4633c8, 12), h8(0x72403aec1991d8fa, 18)),
    (h8(0x24ed331bdd99f754, 12), h8(0xdcf44afafe98bbdc, 18)),
    (h8(0xc07d2b65dc0c85f4, 12), h8(0xf6ae2f794ad870d5, 18)),
    (h8(0x94f2fb4f5071bc86, 12), h8(0x5a1f1d005e918ce0, 18)),
    (h8(0x6739c134137b4515, 12), h8(0xd45d7c2768fbd3ed, 18)),
    (h8(0xac28d24af131a2fe, 12), h8(0x581e7694b0625792, 18)),
    (h8(0x691d5906901aa754, 12), h8(0x50f05498ded5242c, 18)),
    (h8(0xcdccf8fcfac4389b, 12), h8(0x8f748c36bf79872b, 18)),
    (h8(0x2e0270c72d2f6150, 12), h8(0xb86eb9e54374cfe7, 18)),
    (h8(0x293e35ce1487384b, 12), h8(0xef272f065cfd205b, 18)),
    (h8(0x7516f7f6d48f56b3, 12), h8(0x06d8aa79532724bd, 18)),
    (h8(0xa31e38264b4bf8a4, 12), h8(0xac175df6e7226588, 18)),
    (h8(0x665512ab6a1f2b0b, 12), h8(0xf94b3500feebcb29, 18)),
    (h8(0xa94b6ae8f99adb2c, 12), h8(0x49f292074cd5d671, 18)),
    (h8(0xb35eb18ec78ccb39, 12), h8(0xf9f016b3dfbfc160, 18)),
    (h8(0x1d058d723bfad71f, 12), h8(0xd96c12d5631dc15f, 18)),
    (h8(0x4ddbb0842c5559b7, 12), h8(0x56c4e507e09ca67d, 18)),
    (h8(0x28d30f220a8f11fa, 12), h8(0xe2b2ade2e12f4d6f, 18)),
    (h8(0x9989bc1d639e3611, 12), h8(0x8fb2b1dbe7ae6e69, 18)),
    (h8(0x3bfd25cc0a73cfe5, 12), h8(0x6f927701521681be, 18)),
];

const fn h8(v: u64, len: usize) -> PropHash {
    (v.to_be_bytes(), len)
}

fn prop_int(o: &dc_world::Object<'_>, k: PropHash) -> Option<i32> {
    match prop(o, k)? {
        dc_world::FieldValue::Int(i) | dc_world::FieldValue::Bits(i) => Some(*i),
        dc_world::FieldValue::Real(r) if r.is_finite() => Some(*r as i32),
        _ => None,
    }
}

/// The event counters, timers and activation boxes of a parsed world (DSV-572, DSV-573).
// DC-DEFAULT: DSV-572 (proposed) an event value below 0 is no event
pub fn mission_objects(w: &World<'_>) -> crate::script::MissionObjects {
    let mut m = crate::script::MissionObjects::default();
    let text = |o: &dc_world::Object<'_>, k: PropHash| prop_text(o, k).unwrap_or_default().to_vec();
    for o in &w.objects {
        let class = key_of(o.class_name);
        let name = o.name().map(<[u8]>::to_vec).unwrap_or_default();
        if class == CLASS_EVENT_COUNTER {
            m.counters.push(crate::script::CounterInfo {
                name,
                start: prop_int(o, PROP_COUNTER_START).unwrap_or(0),
                events: PROP_COUNTER_EVENTS
                    .iter()
                    .filter_map(|(v, c)| {
                        let v = prop_int(o, *v).filter(|v| *v >= 0)?;
                        let c = text(o, *c);
                        (!c.is_empty()).then_some((v, c))
                    })
                    .collect(),
                // DC-DEFAULT: DSV-576 (proposed) an event with a command and a value below 0 is kept as unset
                unset: PROP_COUNTER_EVENTS
                    .iter()
                    .zip(1u32..)
                    .filter_map(|((v, c), n)| {
                        prop_int(o, *v).filter(|v| *v < 0)?;
                        let c = text(o, *c);
                        (!c.iter().all(u8::is_ascii_whitespace)).then_some((n, c))
                    })
                    .collect(),
            });
        } else if class == CLASS_TIMER {
            m.timers.push(crate::script::TimerInfo {
                name,
                secs: prop_real(o, PROP_TIME_AMOUNT).unwrap_or(0.0).max(0.0),
                in_time: text(o, PROP_IN_TIME_COMMAND),
                over_time: text(o, PROP_OVER_TIME_COMMAND),
            });
        } else if class == CLASS_ACTIVATE_BOX {
            let Some(pos) = o.position() else {
                continue;
            };
            m.activates.push(crate::script::ActivateInfo {
                name,
                pos,
                dims: o
                    .vec3(PropKey::BoxExtent)
                    .filter(|d| d.iter().all(|c| c.is_finite()))
                    .map_or([32.0; 3], |d| d.map(f32::abs)),
                locked: prop_bool(o, PROP_LOCKED).unwrap_or(false),
                max: prop_int(o, crate::doors::PROP_ACTIVATIONS)
                    .filter(|n| *n > 0)
                    .map_or(0, |n| n as u32),
                secs: prop_real(o, PROP_TIME_AMOUNT).unwrap_or(0.0).max(0.0),
                work: text(o, PROP_WORK_COMMAND),
                release: text(o, PROP_RELEASE_COMMAND),
                activate: text(o, PROP_ACTIVATE_COMMAND),
            });
        }
    }
    m
}

/// Property name keys this crate reads from world objects beyond the vocabulary of `dc_world::names` (the co-op,
/// Spy Hunt, door and camera objects): SHA-256 prefix of the name and its length (no ROM text in the tree). The
/// classes and properties of docs/MAPKIT_PLAN.md section 4 are `dc_world::names` keys instead and are read through
/// [`named`] (ROM hash or DeCombat name, MAP-2, MAP-20).
// SPEC: FMT_DAT85.md §6.1, §12 (properties by name, identified by hash); OBSERVED: property names of the ROM world
// files (hashes only)
pub type PropHash = ([u8; 8], usize);

fn key_of(name: &[u8]) -> PropHash {
    (dc_world::hash16(name), name.len())
}

fn prop<'o, 'a>(o: &'o dc_world::Object<'a>, k: PropHash) -> Option<&'o dc_world::FieldValue<'a>> {
    o.props
        .iter()
        .find(|p| p.name.len() == k.1 && dc_world::hash16(p.name) == k.0)
        .map(|p| &p.value)
}

/// The value of the property `k` names (ROM hash or DeCombat alias, `Key::matches`).
// DC-DEFAULT: MAP-2, MAP-20 (the server reads the vocabulary through the keys)
fn named<'o, 'a>(o: &'o dc_world::Object<'a>, k: &Key) -> Option<&'o dc_world::FieldValue<'a>> {
    o.prop_named(k).map(|p| &p.value)
}

fn text_of<'a>(v: Option<&dc_world::FieldValue<'a>>) -> Option<&'a [u8]> {
    match v? {
        dc_world::FieldValue::Text(t) => Some(*t),
        _ => None,
    }
}

fn bool_of(v: Option<&dc_world::FieldValue<'_>>) -> Option<bool> {
    match v? {
        dc_world::FieldValue::Bool(b) => Some(*b),
        dc_world::FieldValue::Int(i) | dc_world::FieldValue::Bits(i) => Some(*i != 0),
        _ => None,
    }
}

fn int_of(v: Option<&dc_world::FieldValue<'_>>) -> Option<i32> {
    match v? {
        dc_world::FieldValue::Int(i) | dc_world::FieldValue::Bits(i) => Some(*i),
        dc_world::FieldValue::Real(r) if r.is_finite() => Some(*r as i32),
        _ => None,
    }
}

fn real_of(v: Option<&dc_world::FieldValue<'_>>) -> Option<f32> {
    match v? {
        dc_world::FieldValue::Real(r) if r.is_finite() => Some(*r),
        dc_world::FieldValue::Int(i) | dc_world::FieldValue::Bits(i) => Some(*i as f32),
        _ => None,
    }
}

fn vec_of(v: Option<&dc_world::FieldValue<'_>>) -> Option<[f32; 3]> {
    match v? {
        dc_world::FieldValue::Vec3(v) | dc_world::FieldValue::Rgb(v) => Some(*v),
        _ => None,
    }
}

fn prop_text<'a>(o: &dc_world::Object<'a>, k: PropHash) -> Option<&'a [u8]> {
    text_of(prop(o, k))
}

fn prop_bool(o: &dc_world::Object<'_>, k: PropHash) -> Option<bool> {
    bool_of(prop(o, k))
}

/// The vocabulary readers: text, bool, int, real and vector of a `dc_world::names` key.
pub fn named_text<'a>(o: &dc_world::Object<'a>, k: &Key) -> Option<&'a [u8]> {
    text_of(named(o, k))
}

pub fn named_bool(o: &dc_world::Object<'_>, k: &Key) -> Option<bool> {
    bool_of(named(o, k))
}

pub fn named_int(o: &dc_world::Object<'_>, k: &Key) -> Option<i32> {
    int_of(named(o, k))
}

pub fn named_real(o: &dc_world::Object<'_>, k: &Key) -> Option<f32> {
    real_of(named(o, k))
}

pub fn named_vec(o: &dc_world::Object<'_>, k: &Key) -> Option<[f32; 3]> {
    vec_of(named(o, k))
}

/// Which objective a zone serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneKind {
    BombSite,
    FlagZone,
}

/// One objective zone of the world file.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneInfo {
    pub kind: ZoneKind,
    pub pos: [f32; 3],
    pub rot: [f32; 3],
    /// The box extent property.
    pub dims: [f32; 3],
    /// The object's name (the zone's notice text in the reference stack's record).
    pub name: Vec<u8>,
    /// The team text the zone belongs to (flag zone: its team; bomb site: the defending team).
    pub team_text: Option<Vec<u8>>,
    /// The zone number property (0 when absent).
    pub number: i32,
    /// A bomb site's planted command text (the world's script run when a bomb is planted in it; empty otherwise).
    // OBSERVED: the ROM worlds' bomb-site trigger objects (property hash `PROP_PLANTED_COMMAND`): the text switches
    // the sites' indicator effects (CLI-1340)
    pub planted_command: Vec<u8>,
}

/// One capture flag of the world file.
#[derive(Debug, Clone, PartialEq)]
pub struct FlagInfo {
    pub pos: [f32; 3],
    pub rot: [f32; 3],
    /// The ground model file the object names (`None`: no model or an empty `model`; a vocabulary flag then gets
    /// the default look, MAP-23).
    pub model: Option<String>,
    pub team_text: Option<Vec<u8>>,
    /// A `dc.capture_flag` (the class matched through the alias half, [`is_dc_object`]).
    pub dc: bool,
}

/// The blend mode of a world model (0 none, 1 translucent, 2 additive).
pub const BLEND_NONE: u8 = 0;

/// The door and switch classes in the `PropHash` shape the door reader compares (`doors.rs`; ROM-only classes,
/// scope cut D12): the hash half of the `dc_world::names` keys.
pub const CLASS_SLIDING_DOOR: PropHash = names::CLASS_SLIDING_DOOR.hash_len();
pub const CLASS_ROTATING_DOOR: PropHash = names::CLASS_ROTATING_DOOR.hash_len();
pub const CLASS_SWITCH: PropHash = names::CLASS_SWITCH.hash_len();

/// What a model-linked object is, by its class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrushClass {
    /// A plain world model.
    Plain,
    Breakable,
    /// A sky world model (with its sky table data).
    Sky,
    /// Spinning and rotating world models (moved by their server objects; static here).
    Spinning,
    Rotating,
    /// Doors and switches (moving, with a client object of their own).
    Door,
    Switch,
    /// A water volume (a container with its own movement rules).
    Water,
    /// Any other class linked to a model.
    Other,
}

/// The sky data of a sky world model: its sky table slot, the half extents of its sky box and the inner fraction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyInfo {
    pub index: i32,
    pub dims: [f32; 3],
    pub inner: [f32; 3],
}

/// One object linked to a named BSP model.
#[derive(Debug, Clone, PartialEq)]
pub struct BrushInfo {
    pub name: Vec<u8>,
    pub role: Option<ObjectRole>,
    pub pos: [f32; 3],
    pub rot: [f32; 3],
    pub visible: bool,
    pub solid: bool,
    pub rayhit: bool,
    pub blend: u8,
    /// The object's fog-disable property.
    pub fog_disable: bool,
    pub class: BrushClass,
    pub sky: Option<SkyInfo>,
}

/// The kind of a named volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeKind {
    KillVolume,
    Ladder,
}

/// A named volume (kill volume, ladder) and the container properties its client record carries.
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeInfo {
    pub name: Vec<u8>,
    pub kind: VolumeKind,
    pub pos: [f32; 3],
    pub fog_enable: bool,
    pub fog_far: f32,
    pub fog_near: f32,
    pub fog_rgb: [f32; 3],
    pub tint: [f32; 3],
    pub light_add: [f32; 3],
    pub movement_sounds: bool,
    pub current: [f32; 3],
    pub viscosity: f32,
    pub friction: f32,
    /// A kill volume with a delay before it kills.
    pub pending: bool,
}

/// A scaled sprite.
#[derive(Debug, Clone, PartialEq)]
pub struct SpriteInfo {
    pub name: Vec<u8>,
    pub pos: [f32; 3],
    pub rot: [f32; 3],
    /// The sprite file (resource path).
    pub file: String,
    pub scale: [f32; 2],
    /// Colour (0..255 per channel) and alpha (0..1).
    pub color: [f32; 3],
    pub alpha: f32,
    pub start_on: bool,
}

/// A placed client effect.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectInfo {
    pub name: Vec<u8>,
    pub pos: [f32; 3],
    pub rot: [f32; 3],
    /// The effect name the client looks up.
    pub fx: Vec<u8>,
    pub looped: bool,
    pub start_on: bool,
}

/// A placed world sound.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundInfo {
    pub name: Vec<u8>,
    pub pos: [f32; 3],
    /// The sound file (resource path).
    pub file: String,
    pub priority: u8,
    pub inner: f32,
    pub outer: f32,
    /// Volume in percent (100 = as recorded).
    pub volume: u32,
    pub pitch: f32,
    /// No direction: heard the same from every side.
    pub ambient: bool,
    pub looped: bool,
    pub start_on: bool,
    /// Played at the listener (the world files' attach switch).
    pub attached: bool,
}

/// The world's command texts (as stored, never interpreted here).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorldScripts {
    /// The start-up command objects' commands, in file order.
    pub startup: Vec<Vec<u8>>,
    /// Trigger objects: name -> their numbered commands in order.
    pub triggers: Vec<(Vec<u8>, Vec<Vec<u8>>)>,
    /// Mode controllers: name -> object-start command.
    pub controls: Vec<(Vec<u8>, Vec<u8>)>,
    /// The camera objects and camera points the trigger scripts move (DSV-112, proposed).
    pub cameras: crate::camshot::WorldCameras,
}

fn prop_real(o: &dc_world::Object<'_>, k: PropHash) -> Option<f32> {
    real_of(prop(o, k))
}

/// The class of a model-linked object.
// OBSERVED: class names of the ROM world files (hashes only; 2026-10-25 scratch survey of the 21 ROM worlds over
// dc-world: spinning 7, rotating 9, sliding doors 23, rotating doors 42, switches 2, water 4 model-linked objects)
fn brush_class(o: &dc_world::Object<'_>) -> BrushClass {
    match o.role {
        Some(ObjectRole::PlainWorldModel) => BrushClass::Plain,
        Some(ObjectRole::Breakable) => BrushClass::Breakable,
        Some(ObjectRole::SkyWorldModel) => BrushClass::Sky,
        _ if o.is_class(&names::CLASS_SPINNING_MODEL) => BrushClass::Spinning,
        _ if o.is_class(&names::CLASS_ROTATING_MODEL) => BrushClass::Rotating,
        _ if o.is_class(&names::CLASS_SLIDING_DOOR) || o.is_class(&names::CLASS_ROTATING_DOOR) => {
            BrushClass::Door
        }
        _ if o.is_class(&names::CLASS_SWITCH) => BrushClass::Switch,
        _ if o.is_class(&names::CLASS_WATER) => BrushClass::Water,
        _ => BrushClass::Other,
    }
}

/// Volumes, sprites, effects and command texts of a parsed world.
// SPEC: FMT_DAT85.md §6 (object list), §6.2 (roles: kill volume, ladder, trigger, start-up command, mode
// controller), §6.4 (model link by name)
pub fn scene_objects(
    w: &World<'_>,
) -> (
    Vec<VolumeInfo>,
    Vec<SpriteInfo>,
    Vec<EffectInfo>,
    WorldScripts,
) {
    let (volumes, sprites, effects, scripts, _) = scene_objects_with_sounds(w);
    (volumes, sprites, effects, scripts)
}

/// The world sounds of a parsed world, in file order (objects of the sound class, `dc.sound` on a community map,
/// that name a sound file).
// SPEC: FMT_DAT85.md §6 (object list, properties by name)
// OBSERVED: class and property names of the ROM world files (hashes only; 2026-10-25 scratch survey over dc-world:
// 150 objects of this class on 16 of the 21 ROM worlds, 129 of them with the start switch on, volumes 25..200,
// pitch factor 1.0 on all)
pub fn world_sounds(w: &World<'_>) -> Vec<SoundInfo> {
    w.objects
        .iter()
        .filter(|o| o.is_class(&names::CLASS_SOUND))
        .filter_map(|o| {
            let file = named_text(o, &names::PROP_SOUND_FILE)
                .filter(|t| !t.is_empty())
                .map(|t| String::from_utf8_lossy(t).into_owned())?;
            let outer = named_real(o, &names::PROP_OUTER_RADIUS)
                .unwrap_or(0.0)
                .max(0.0);
            Some(SoundInfo {
                name: o.name().map(<[u8]>::to_vec).unwrap_or_default(),
                pos: o.position().unwrap_or([0.0; 3]),
                file,
                priority: named_int(o, &names::PROP_PRIORITY)
                    .unwrap_or(0)
                    .clamp(0, 255) as u8,
                inner: named_real(o, &names::PROP_INNER_RADIUS)
                    .unwrap_or(outer)
                    .clamp(0.0, outer),
                outer,
                volume: named_int(o, &names::PROP_VOLUME).unwrap_or(100).max(0) as u32,
                pitch: named_real(o, &names::PROP_PITCH_SHIFT).unwrap_or(1.0),
                ambient: named_bool(o, &names::PROP_AMBIENT).unwrap_or(false),
                looped: named_bool(o, &names::PROP_LOOP).unwrap_or(false),
                start_on: named_bool(o, &names::PROP_START_ON).unwrap_or(true),
                attached: named_bool(o, &names::PROP_PLAY_ATTACHED).unwrap_or(false),
            })
        })
        .collect()
}

/// [`scene_objects`] and the world sounds.
pub fn scene_objects_with_sounds(
    w: &World<'_>,
) -> (
    Vec<VolumeInfo>,
    Vec<SpriteInfo>,
    Vec<EffectInfo>,
    WorldScripts,
    Vec<SoundInfo>,
) {
    let mut volumes = Vec::new();
    let mut sprites = Vec::new();
    let mut effects = Vec::new();
    let mut scripts = WorldScripts::default();
    for o in &w.objects {
        let name = || o.name().map(<[u8]>::to_vec).unwrap_or_default();
        let pos = o.position().unwrap_or([0.0; 3]);
        let rot = o.rotation().unwrap_or([0.0; 3]);
        match o.role {
            Some(ObjectRole::KillVolume) | Some(ObjectRole::Ladder) if w.brush_of(o).is_some() => {
                // a `dc.kill_volume` / `dc.ladder` carries none of the container properties: the defaults below
                // apply (no fog, white tint, no current, friction 1; the vocabulary has no ladder viscosity)
                // DC-DEFAULT: MAP-20
                volumes.push(VolumeInfo {
                    name: name(),
                    kind: if o.role == Some(ObjectRole::Ladder) {
                        VolumeKind::Ladder
                    } else {
                        VolumeKind::KillVolume
                    },
                    // OBSERVED: tdm4 / tdm6 (the model origin, as for brushes; REFERENCE)
                    pos: w.brush_of(o).map_or(pos, |m| m.origin),
                    fog_enable: named_bool(o, &names::PROP_FOG_ENABLE).unwrap_or(false),
                    fog_far: named_real(o, &names::PROP_FOG_FAR).unwrap_or(0.0),
                    fog_near: named_real(o, &names::PROP_FOG_NEAR).unwrap_or(0.0),
                    fog_rgb: named_vec(o, &names::PROP_FOG_COLOR).unwrap_or([0.0; 3]),
                    tint: named_vec(o, &names::PROP_TINT_COLOR).unwrap_or([255.0; 3]),
                    light_add: named_vec(o, &names::PROP_LIGHT_ADD).unwrap_or([0.0; 3]),
                    movement_sounds: named_bool(o, &names::PROP_MOVEMENT_SOUNDS).unwrap_or(true),
                    current: named_vec(o, &names::PROP_CURRENT).unwrap_or([0.0; 3]),
                    viscosity: named_real(o, &names::PROP_VISCOSITY).unwrap_or(0.0),
                    friction: named_real(o, &names::PROP_FRICTION).unwrap_or(1.0),
                    pending: named_real(o, &names::PROP_PENDING_SEC).is_some_and(|s| s > 0.0),
                });
                continue;
            }
            Some(ObjectRole::StartupCommand) => {
                if let Some(t) = named_text(o, &names::PROP_COMMAND) {
                    scripts.startup.push(t.to_vec());
                }
                continue;
            }
            Some(ObjectRole::TriggerVolume) => {
                let cmds: Vec<Vec<u8>> = names::PROP_TRIGGER_COMMANDS
                    .iter()
                    .filter_map(|k| named_text(o, k))
                    .filter(|t| !t.is_empty())
                    .map(<[u8]>::to_vec)
                    .collect();
                scripts.triggers.push((name(), cmds));
                continue;
            }
            Some(ObjectRole::ModeController) => {
                if let Some(t) = named_text(o, &names::PROP_OBJECT_START_COMMAND) {
                    scripts.controls.push((name(), t.to_vec()));
                }
                continue;
            }
            _ => {}
        }
        if o.is_class(&names::CLASS_SPRITE) {
            // the sprite's file is the ROM file-name property, `file` in the vocabulary (MAP-2)
            let Some(file) = named_text(o, &names::PROP_SPRITE_FILE)
                .filter(|t| !t.is_empty())
                .map(|t| String::from_utf8_lossy(t).into_owned())
            else {
                continue;
            };
            sprites.push(SpriteInfo {
                name: name(),
                pos,
                rot,
                file,
                scale: [
                    named_real(o, &names::PROP_SCALE_X).unwrap_or(1.0),
                    named_real(o, &names::PROP_SCALE_Y).unwrap_or(1.0),
                ],
                color: named_vec(o, &names::PROP_SPRITE_COLOR).unwrap_or([255.0; 3]),
                alpha: named_real(o, &names::PROP_ALPHA).unwrap_or(1.0),
                start_on: named_bool(o, &names::PROP_START_ON).unwrap_or(true),
            });
        } else if o.is_class(&names::CLASS_SPECIAL_FX) {
            let Some(fx) = named_text(o, &names::PROP_FX_NAME).filter(|t| !t.is_empty()) else {
                continue;
            };
            effects.push(EffectInfo {
                name: name(),
                pos,
                rot,
                fx: fx.to_vec(),
                looped: named_bool(o, &names::PROP_LOOP).unwrap_or(false),
                start_on: named_bool(o, &names::PROP_START_ON).unwrap_or(false),
            });
        }
    }
    (volumes, sprites, effects, scripts, world_sounds(w))
}

/// Zones, flags and brush objects of a parsed world.
// SPEC: FMT_DAT85.md §6.2 (roles: flag zone, bomb site, capture flag), §6.4 (model link by name)
pub fn world_objects(w: &World<'_>) -> (Vec<ZoneInfo>, Vec<FlagInfo>, Vec<BrushInfo>) {
    let mut zones = Vec::new();
    let mut flags = Vec::new();
    let mut brushes = Vec::new();
    for o in &w.objects {
        let pos = o.position().unwrap_or([0.0; 3]);
        let rot = o.rotation().unwrap_or([0.0; 3]);
        match o.role {
            Some(ObjectRole::BombSite) | Some(ObjectRole::FlagZone) => {
                let bomb = o.role == Some(ObjectRole::BombSite);
                zones.push(ZoneInfo {
                    kind: if bomb {
                        ZoneKind::BombSite
                    } else {
                        ZoneKind::FlagZone
                    },
                    pos,
                    rot,
                    dims: o.vec3(PropKey::BoxExtent).unwrap_or([0.0; 3]),
                    name: o.name().map(<[u8]>::to_vec).unwrap_or_default(),
                    // a `dc.bomb_site` without `defuse_team` = any team (the ZONE_TEAM_ANY fallback of worldobj)
                    team_text: if bomb {
                        named_text(o, &names::PROP_DEFUSE_TEAM)
                    } else {
                        o.text(PropKey::Team)
                    }
                    .map(<[u8]>::to_vec),
                    number: named_int(o, &names::PROP_NUMBER).unwrap_or(0),
                    planted_command: if bomb {
                        named_text(o, &names::PROP_PLANTED_COMMAND)
                            .map(<[u8]>::to_vec)
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    },
                });
                continue;
            }
            Some(ObjectRole::CaptureFlag) => {
                flags.push(FlagInfo {
                    pos,
                    rot,
                    // the ROM file-name property, `model` in the vocabulary (MAP-2)
                    model: named_text(o, &names::PROP_MODEL)
                        .filter(|t| !t.is_empty())
                        .map(|t| String::from_utf8_lossy(t).into_owned()),
                    team_text: o.text(PropKey::TeamFilter).map(<[u8]>::to_vec),
                    dc: is_dc_object(o),
                });
                continue;
            }
            Some(ObjectRole::KillVolume)
            | Some(ObjectRole::Ladder)
            | Some(ObjectRole::HazardVolume)
            | Some(ObjectRole::TriggerVolume) => continue,
            _ => {}
        }
        if w.brush_of(o).is_none() {
            continue;
        }
        // the blend-mode text is matched through the value keys too (`translucent` / `additive` on a community map)
        // OBSERVED: values of the ROM world files' blend-mode property (hashes only)
        let blend = match named_text(o, &names::PROP_BLEND) {
            Some(t) if names::BLEND_TRANSLUCENT.matches(t) => 1,
            Some(t) if names::BLEND_ADDITIVE.matches(t) => 2,
            _ => BLEND_NONE,
        };
        let class = brush_class(o);
        let sky = (class == BrushClass::Sky).then(|| SkyInfo {
            index: named_int(o, &names::PROP_SKY_INDEX).unwrap_or(0),
            dims: named_vec(o, &names::PROP_SKY_DIMS).unwrap_or([0.0; 3]),
            // the split value: one `sky_inner` vec3 on a community map, the ROM's three inner-percent reals else
            // DC-DEFAULT: MAP-2, MAP-20
            inner: named_vec(o, &names::PROP_SKY_INNER).unwrap_or_else(|| {
                [0, 1, 2].map(|i| named_real(o, &names::SKY_INNER_PERCENT[i]).unwrap_or(0.0))
            }),
        });
        brushes.push(BrushInfo {
            name: o.name().map(<[u8]>::to_vec).unwrap_or_default(),
            role: o.role,
            // the model's own origin (its geometry is stored in place), not the object's editor position
            // OBSERVED: tdm4 / tdm6 (the reference stack's brush and volume creates carried the BSP model origin; REFERENCE)
            pos: w.brush_of(o).map_or(pos, |m| m.origin),
            rot,
            visible: named_bool(o, &names::PROP_VISIBLE).unwrap_or(true),
            solid: named_bool(o, &names::PROP_SOLID).unwrap_or(true),
            rayhit: named_bool(o, &names::PROP_RAYHIT).unwrap_or(true),
            blend,
            fog_disable: named_bool(o, &names::PROP_FOG_DISABLE).unwrap_or(false),
            class,
            sky,
        });
    }
    (zones, flags, brushes)
}

/// The shot-trace world: the level model only (the model no object links to by name), no blocker polygons. The
/// world's named models (doors, breakables, glass) exist on a client only once the server created their objects,
/// and they may have moved or broken there, so a shot is not refused by them; the blocker polygons are player
/// clips.
// SPEC: FMT_DAT85.md §5.2, §6.4 (models linked to objects by name); dc-sim from_world (`WorldFilter::level`, OBSERVED
// one level model per ROM world); REFERENCE_FINDINGS_botduel.md F1 (the reference stack stopped shots at walls)
// DC-DEFAULT: DSV-34 (proposed)
pub fn trace_world(w: &World<'_>) -> Option<CollisionWorld> {
    let models: Vec<dc_sim::CollModel> = w
        .models
        .iter()
        .filter(|m| w.object_named(m.name).is_none())
        .map(dc_sim::from_world::coll_model)
        .collect();
    if models.is_empty() {
        return None;
    }
    Some(CollisionWorld::from_parts(models, &[]))
}

impl WorldInfo {
    /// The world box centre (fallback spawn when the world has no player spawn point).
    pub fn centre(&self) -> [f32; 3] {
        let mut c = [0.0; 3];
        for (i, v) in c.iter_mut().enumerate() {
            *v = (self.extent_min[i] + self.extent_max[i]) * 0.5;
        }
        c
    }
}

/// Parses world bytes (render data skipped: the server never needs it).
// SPEC: FMT_DAT85.md §12 (render data is client-only)
pub fn world_from_bytes(data: &[u8], announce: &str, display: &str) -> Result<WorldInfo, String> {
    let w = World::parse_with(data, ParseOptions { render: false })
        .map_err(|e| format!("world parse failed: {e}"))?;
    let spawns = w
        .objects_with_role(ObjectRole::PlayerSpawn)
        .filter_map(|o| {
            Some(SpawnPoint {
                pos: o.position()?,
                rot: o.rotation().unwrap_or([0.0; 3]),
                team_filter: o.text(PropKey::TeamFilter).map(<[u8]>::to_vec),
            })
        })
        .collect();
    let (zones, flags, brushes) = world_objects(&w);
    let (volumes, sprites, effects, mut scripts, sounds) = scene_objects_with_sounds(&w);
    let scripts_triggers = scripts.triggers.clone();
    // DC-DEFAULT: DSV-112 (proposed) the world's cameras and camera points
    scripts.cameras = crate::camshot::world_cameras(&w);
    let occluders = Some(crate::visibility::Occluders::from_world(&w, &brushes))
        .filter(|o| !o.is_empty())
        .map(Arc::new);
    Ok(WorldInfo {
        announce: announce.to_string(),
        display: display_token(display),
        size: data.len().min(u32::MAX as usize) as u32,
        extent_min: w.extent_min,
        extent_max: w.extent_max,
        spawns,
        objects: w.objects.len(),
        sha256: dc_rez::dc_rom::sha256_hex(data),
        trace: trace_world(&w).map(Arc::new),
        // DC-DEFAULT: DSV-62 (proposed)
        surfaces: crate::surface::SurfaceProbe::from_world(&w).map(Arc::new),
        zones,
        flags,
        brushes,
        volumes,
        sprites,
        effects,
        sounds,
        scripts,
        global_light: crate::worldlight::global_light(&w),
        occluders,
        coop: {
            // DC-DEFAULT: DSV-571 (proposed) a world whose triggers or AI death commands spawn AI is a mission world
            let mut c = coop_objects(&w);
            let triggers = crate::doors::script_triggers(&w);
            c.mission = triggers
                .iter()
                .flat_map(|t| t.cmds.iter())
                .chain(c.templates.iter().map(|t| &t.death_cmd))
                .any(|t| crate::script::spawns_ai(t));
            // DC-DEFAULT: DSV-576 (proposed) the round loop of a round world
            let deaths: Vec<&[u8]> = c.templates.iter().map(|t| t.death_cmd.as_slice()).collect();
            c.rounds = crate::script::round_network(&triggers, &mission_objects(&w), &deaths);
            c
        },
        mission: mission_objects(&w),
        world_vars: crate::evars::world_vars(&w),
        spawn_modes: w
            .objects_with_role(ObjectRole::PlayerSpawn)
            .filter(|o| o.position().is_some())
            .map(|o| o.text(PropKey::ModeFilter).map(<[u8]>::to_vec))
            .collect(),
        // DC-DEFAULT: MAP-21 (an empty mode filter on a vocabulary spawn point serves every mode)
        spawn_dc: w
            .objects_with_role(ObjectRole::PlayerSpawn)
            .filter(|o| o.position().is_some())
            .map(is_dc_object)
            .collect(),
        spawn_names: w
            .objects_with_role(ObjectRole::PlayerSpawn)
            .filter(|o| o.position().is_some())
            .map(|o| o.name().map(<[u8]>::to_vec).unwrap_or_default())
            .collect(),
        // OBSERVED: ROM world spawn points (SANDHOG_CP1: 8 of its 32 co-op start points are open, the other 24 are
        // locked in the file and unlocked group by group by the checkpoint triggers' statements; no other world
        // locks one)
        // DC-DEFAULT: DSV-569 (proposed) a start point the file locks is no spawn candidate while locked
        spawn_locked: w
            .objects_with_role(ObjectRole::PlayerSpawn)
            .filter(|o| o.position().is_some())
            .map(|o| prop_bool(o, crate::doors::PROP_LOCKED).unwrap_or(false))
            .collect(),
        kill_shapes: crate::killvol::kill_shapes(&w),
        // DC-DEFAULT: DSV-607 (proposed)
        hazards: crate::killvol::hazard_shapes(&w),
        doors: crate::doors::door_infos(&w),
        turns: crate::doors::turn_infos(&w),
        triggers: crate::doors::script_triggers(&w),
        client_vars: Default::default(),
        // DC-DEFAULT: DSV-115 (proposed)
        destructibles: crate::destructibles::destructible_world(&w),
        spy: SpyWorld {
            triggers: scripts_triggers,
            ..spy_objects(&w)
        },
    })
}

fn display_token(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .collect();
    if t.is_empty() {
        "-".into()
    } else {
        t
    }
}

fn norm(s: &str) -> String {
    s.replace('/', "\\").to_ascii_uppercase()
}

/// Finds the archive entry for a `-world` argument: the full path with or without the extension, else the base
/// name with or without the extension (ASCII case-insensitive, either separator).
pub fn find_entry<'e>(entries: &'e [(String, u64, u32)], arg: &str) -> Option<&'e str> {
    let a = norm(arg.trim());
    let with_ext = if a.ends_with(WORLD_EXT) {
        a.clone()
    } else {
        format!("{a}{WORLD_EXT}")
    };
    let base = |p: &str| p.rsplit('\\').next().unwrap_or(p).to_string();
    entries
        .iter()
        .map(|e| e.0.as_str())
        .find(|n| norm(n) == with_ext)
        .or_else(|| {
            let b = base(&with_ext);
            entries
                .iter()
                .map(|e| e.0.as_str())
                .find(|n| base(&norm(n)) == b)
        })
}

/// The announced path for a listed entry.
pub fn announce_name(listed: &str, arg: &str, form: NameForm) -> String {
    match form {
        NameForm::Listed => listed.to_string(),
        NameForm::NoExtension => {
            let n = norm(listed);
            if n.ends_with(WORLD_EXT) {
                listed[..listed.len() - WORLD_EXT.len()].to_string()
            } else {
                listed.to_string()
            }
        }
        NameForm::Argument => arg.to_string(),
    }
}

/// Loads the room's world from the user's ROM. World entries are stored plain: the raw entry is used when its
/// first word is the version 85, otherwise it is decrypted with the ROM's own key (`dc_rom::crypt_key`).
// SPEC: FMT_DAT85.md §3 (version 85); DSERVER_PIPE.md §2 (`-world`, `CA_ROM`)
pub fn load_from_rom(
    rom: &Path,
    arg: &str,
    form: NameForm,
    announce_override: Option<&str>,
) -> Result<WorldInfo, String> {
    let rez = rom.join("Game").join(WORLD_ARCHIVE);
    let arch = dc_rez::RezArchive::open(&rez).map_err(|e| format!("world archive: {e}"))?;
    let listed = find_entry(arch.entries(), arg)
        .ok_or_else(|| format!("world {arg:?} not found in the world archive"))?
        .to_string();
    let raw = arch
        .entry_raw(&listed)
        .map_err(|e| format!("world read: {e}"))?
        .ok_or_else(|| format!("world {listed:?} vanished"))?;
    let data = if raw.starts_with(&WORLD_VERSION.to_le_bytes()) {
        raw
    } else {
        let key = dc_rez::dc_rom::crypt_key(rom).map_err(|e| format!("world key: {e}"))?;
        let dec =
            dc_rez::decrypt_with_key(&raw, &key).map_err(|e| format!("world decrypt: {e}"))?;
        if !dec.starts_with(&WORLD_VERSION.to_le_bytes()) {
            return Err(format!("world {listed:?} is not a version-85 world"));
        }
        dec
    };
    let announce = match announce_override {
        Some(a) => a.to_string(),
        None => announce_name(&listed, arg, form),
    };
    let mut info = world_from_bytes(&data, &announce, arg)?;
    // DC-DEFAULT: DSV-47 (proposed) the client names of the world variables from the user's own CShell.dll
    crate::evars::resolve_names(&mut info.world_vars, rom);
    // DC-DEFAULT: DSV-53 (proposed) the step-height variable's name from the user's own CShell.dll
    info.client_vars = crate::clientvars::resolve(rom);
    Ok(info)
}

/// Loads a world file from disk (tests, local experiments). `announce` is the client path to announce. No ROM: the
/// evar and client-variable names the file cannot give stay unresolved ([`load_from_file_with_rom`]).
pub fn load_from_file(path: &Path, announce: &str) -> Result<WorldInfo, String> {
    load_from_file_with_rom(path, announce, None)
}

/// [`load_from_file`] for the `-worldfile` launch: with a ROM at hand (`-rom` or `CA_ROM`), the client names of the
/// world-settings variables and of the client variables are resolved from the user's own CShell.dll, as for a
/// world loaded out of the ROM archive. A community map names its world settings in the vocabulary (`far_z`,
/// `fog_enable`, ...), so every one of its evar rows needs this (MAP-20); without a ROM those rows are not sent.
// DC-DEFAULT: MAP-20; DSV-47 (proposed), DSV-53 (proposed)
pub fn load_from_file_with_rom(
    path: &Path,
    announce: &str,
    rom: Option<&Path>,
) -> Result<WorldInfo, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let display = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let mut info = world_from_bytes(&data, announce, &display)?;
    if let Some(rom) = rom {
        crate::evars::resolve_names(&mut info.world_vars, rom);
        info.client_vars = crate::clientvars::resolve(rom);
    }
    Ok(info)
}

/// A small synthetic world (no ROM content): a box from (-512, -64, -512) to (512, 256, 512), one model, one
/// object of a made-up class. Used by tests and the synthetic client example.
pub fn synthetic_world_bytes() -> Vec<u8> {
    use dc_world::synth::{SynthModel, SynthObject, SynthWorld};
    let names: [&[u8]; 3] = [b"Nm", b"Pos", b"Rot"];
    SynthWorld {
        info: b"synthetic".to_vec(),
        extent_min: [-512.0, -64.0, -512.0],
        extent_max: [512.0, 256.0, 512.0],
        tree_shape: vec![false],
        models: vec![SynthModel {
            name: b"main".to_vec(),
            root: -1,
            ..SynthModel::default()
        }],
        objects: vec![SynthObject::point(
            b"SynthMarker",
            names,
            b"marker01",
            [16.0, 0.0, 32.0],
            [0.0, 1.0, 0.0],
        )],
        ..SynthWorld::default()
    }
    .build()
}

/// A BSP model (tests and tools) from world-space faces: each face `(corners, surface flags, surface id)` becomes
/// one polygon with its own plane and surface record; the node tree comes from dc-sim's naive builder, written
/// with the file's child convention (`FILE_FIRST_CHILD_IS_NEGATIVE`). `flags` is the model's bsp flag word (bit 1
/// = linked to an object, MAP-3) and `origin` the model origin (the object's `pos` for a brush).
// SPEC: FMT_DAT85.md §5.2 (plane, surface, polygon, node records); dc-sim from_world (child convention)
pub fn synth_model_of(
    name: &[u8],
    flags: u32,
    origin: [f32; 3],
    faces: &[(Vec<[f32; 3]>, u32, u16)],
) -> dc_world::synth::SynthModel {
    use dc_sim::collide::{build_bsp, ConvexPoly};
    let polys: Vec<ConvexPoly> = faces
        .iter()
        .map(|(c, f, _)| {
            ConvexPoly::from_corners(c.iter().map(|&p| dc_sim::V3::from_array(p)).collect(), *f)
        })
        .collect();
    let (nodes, root) = build_bsp(&polys);
    let child = |c: dc_sim::collide::Child| c.map_or(-1, |i| i as i32);
    let mut points = Vec::new();
    let mut model_polys = Vec::new();
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for (i, (corners, _, _)) in faces.iter().enumerate() {
        let first = points.len() as u32;
        for p in corners {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
            points.push(*p);
        }
        model_polys.push((
            i as u32,
            i as u32,
            (first..first + corners.len() as u32).collect(),
        ));
    }
    dc_world::synth::SynthModel {
        flags,
        name: name.to_vec(),
        textures: vec![b"@mod:test/assets/t.dtx".to_vec()],
        planes: polys
            .iter()
            .map(|p| (p.normal.to_array(), p.dist))
            .collect(),
        surfaces: faces.iter().map(|(_, f, id)| (*f, 0, *id)).collect(),
        polys: model_polys,
        nodes: nodes
            .iter()
            .map(|(poly, neg, pos)| (*poly, 0, child(*neg), child(*pos)))
            .collect(),
        points,
        root: child(root),
        box_min: lo,
        box_max: hi,
        origin,
    }
}

/// The six outward faces of an axis-aligned box (tests and tools), each with `flags` and surface id 0.
pub fn synth_box_faces(lo: [f32; 3], hi: [f32; 3], flags: u32) -> Vec<(Vec<[f32; 3]>, u32, u16)> {
    let (a, b) = (lo, hi);
    let quads: [[[f32; 3]; 4]; 6] = [
        [
            [a[0], b[1], a[2]],
            [a[0], b[1], b[2]],
            [b[0], b[1], b[2]],
            [b[0], b[1], a[2]],
        ],
        [
            [a[0], a[1], a[2]],
            [b[0], a[1], a[2]],
            [b[0], a[1], b[2]],
            [a[0], a[1], b[2]],
        ],
        [
            [a[0], a[1], a[2]],
            [a[0], a[1], b[2]],
            [a[0], b[1], b[2]],
            [a[0], b[1], a[2]],
        ],
        [
            [b[0], a[1], a[2]],
            [b[0], b[1], a[2]],
            [b[0], b[1], b[2]],
            [b[0], a[1], b[2]],
        ],
        [
            [a[0], a[1], a[2]],
            [a[0], b[1], a[2]],
            [b[0], b[1], a[2]],
            [b[0], a[1], a[2]],
        ],
        [
            [a[0], a[1], b[2]],
            [b[0], a[1], b[2]],
            [b[0], b[1], b[2]],
            [a[0], b[1], b[2]],
        ],
    ];
    quads.iter().map(|q| (q.to_vec(), flags, 0)).collect()
}

/// The surface id of the synthetic community map's floor (a made-up number).
pub const DC_SYNTH_FLOOR_SURFACE: u16 = 7;
/// The sky polygon flag (solid and sky; `dc_mapkit::geom::SURF_SKY`).
// SPEC: FMT_DAT85.md §5.2 (surface flags: 0x1 solid, 0x10 sky)
pub const SURF_SKY: u32 = 0x10;

/// A small community map in DeCombat's own vocabulary (`dc.*` classes, lower-case property names, no ROM content),
/// used by the tests of the alias adoption (MAP-20 .. MAP-25): a level model with a floor at y 0 (surface id
/// [`DC_SYNTH_FLOOR_SURFACE`], 1,500 units each way) under a sky face at y 1,000 (flags solid + sky), team spawns
/// tagged `TDM,FFA` (two per team), one bomb spawn per team tagged `BOMB`, one team-less spawn with an empty filter
/// and one team-1 spawn tagged `flag`; an item spawn; bomb sites 0 (any team) and 1 (`defuse_team` 2); flag zones
/// and capture flags of both teams (team 1's flag without a model, team 2's with an own model path); a kill volume
/// `pit` (`pending_sec` 1.5) at (500, 0, 500), a ladder, a world model, a breakable, a sky model with a `sky_inner`
/// vec3; world settings, a global light, a sound and a sprite.
// DC-DEFAULT: MAP-1 (the vocabulary), MAP-3 (brushes by model name, the unnamed level model)
pub fn synthetic_dc_world_bytes() -> Vec<u8> {
    use dc_world::synth::{SynthObject, SynthValue, SynthWorld};
    let text = |s: &str| SynthValue::Text(s.as_bytes().to_vec());
    let solid = dc_sim::SURF_SOLID;
    let s = 1500.0;
    let level = synth_model_of(
        b"",
        0,
        [0.0; 3],
        &[
            (
                vec![[-s, 0.0, -s], [-s, 0.0, s], [s, 0.0, s], [s, 0.0, -s]],
                solid,
                DC_SYNTH_FLOOR_SURFACE,
            ),
            (
                vec![
                    [-s, 1000.0, -s],
                    [s, 1000.0, -s],
                    [s, 1000.0, s],
                    [-s, 1000.0, s],
                ],
                solid | SURF_SKY,
                0,
            ),
        ],
    );
    let brush = |name: &str, lo: [f32; 3], hi: [f32; 3]| {
        let origin = [0, 1, 2].map(|i| (lo[i] + hi[i]) * 0.5);
        (
            synth_model_of(name.as_bytes(), 2, origin, &synth_box_faces(lo, hi, solid)),
            origin,
        )
    };
    let (pit, pit_at) = brush("pit", [400.0, -100.0, 400.0], [600.0, 100.0, 600.0]);
    let (ladder, ladder_at) = brush("ladder_1", [-600.0, 0.0, -40.0], [-560.0, 300.0, 40.0]);
    let (crate_m, crate_at) = brush("crate_1", [-100.0, 0.0, -100.0], [100.0, 100.0, 100.0]);
    let (glass, glass_at) = brush("glass_1", [200.0, 0.0, -20.0], [300.0, 100.0, 20.0]);
    let (sky, sky_at) = brush(
        "sky_1",
        [-1600.0, -200.0, -1600.0],
        [1600.0, 1200.0, 1600.0],
    );
    let spawn = |name: &str, pos: [f32; 3], team: &str, modes: &str| {
        SynthObject::dc(
            "dc.player_spawn",
            name,
            pos,
            [0.0; 3],
            &[("team", text(team)), ("modes", text(modes))],
        )
    };
    let objects = vec![
        spawn("spawn_t1_a", [-800.0, 40.0, -800.0], "1", "TDM,FFA"),
        spawn("spawn_t1_b", [-800.0, 40.0, -700.0], "1", "TDM,FFA"),
        spawn("spawn_t2_a", [800.0, 40.0, 800.0], "2", "TDM,FFA"),
        spawn("spawn_t2_b", [800.0, 40.0, 700.0], "2", "TDM,FFA"),
        spawn("spawn_bomb_t1", [-900.0, 40.0, 0.0], "1", "BOMB"),
        spawn("spawn_bomb_t2", [900.0, 40.0, 0.0], "2", "BOMB"),
        spawn("spawn_any", [0.0, 40.0, -900.0], "", ""),
        spawn("spawn_flag_t1", [-700.0, 40.0, 500.0], "1", "flag"),
        SynthObject::dc(
            "dc.item_spawn",
            "item_1",
            [0.0, 40.0, 300.0],
            [0.0; 3],
            &[("modes", text(""))],
        ),
        SynthObject::dc(
            "dc.bomb_site",
            "site_a",
            [-500.0, 0.0, -500.0],
            [0.0; 3],
            &[
                ("extent", SynthValue::Vec3([128.0, 48.0, 128.0])),
                ("number", SynthValue::Int(0.0)),
                ("modes", text("BOMB")),
            ],
        ),
        SynthObject::dc(
            "dc.bomb_site",
            "site_b",
            [500.0, 0.0, -500.0],
            [0.0; 3],
            &[
                ("extent", SynthValue::Vec3([128.0, 48.0, 128.0])),
                ("number", SynthValue::Int(1.0)),
                ("modes", text("BOMB")),
                ("defuse_team", text("2")),
            ],
        ),
        SynthObject::dc(
            "dc.flag_zone",
            "zone_1",
            [-700.0, 0.0, 700.0],
            [0.0; 3],
            &[
                ("extent", SynthValue::Vec3([96.0, 48.0, 96.0])),
                ("team", text("1")),
                ("modes", text("FLAG")),
            ],
        ),
        SynthObject::dc(
            "dc.flag_zone",
            "zone_2",
            [700.0, 0.0, -700.0],
            [0.0; 3],
            &[
                ("extent", SynthValue::Vec3([96.0, 48.0, 96.0])),
                ("team", text("2")),
                ("modes", text("FLAG")),
            ],
        ),
        SynthObject::dc(
            "dc.capture_flag",
            "flag_1",
            [-700.0, 0.0, 700.0],
            [0.0; 3],
            &[("team", text("1")), ("model", text(""))],
        ),
        SynthObject::dc(
            "dc.capture_flag",
            "flag_2",
            [700.0, 0.0, -700.0],
            [0.0; 3],
            &[
                ("team", text("2")),
                ("model", text("@mod:test/assets/flag.ltb")),
            ],
        ),
        SynthObject::dc(
            "dc.kill_volume",
            "pit",
            pit_at,
            [0.0; 3],
            &[("pending_sec", SynthValue::Real(1.5))],
        ),
        SynthObject::dc("dc.ladder", "ladder_1", ladder_at, [0.0; 3], &[]),
        SynthObject::dc(
            "dc.world_model",
            "crate_1",
            crate_at,
            [0.0; 3],
            &[
                ("visible", SynthValue::Bool(true)),
                ("solid", SynthValue::Bool(true)),
                ("rayhit", SynthValue::Bool(true)),
                ("blend", text("additive")),
                ("alpha", SynthValue::Real(0.5)),
            ],
        ),
        SynthObject::dc(
            "dc.breakable",
            "glass_1",
            glass_at,
            [0.0; 3],
            &[("hit_points", SynthValue::Real(50.0))],
        ),
        SynthObject::dc(
            "dc.sky_model",
            "sky_1",
            sky_at,
            [0.0; 3],
            &[
                ("sky_index", SynthValue::Int(0.0)),
                ("sky_dims", SynthValue::Vec3([1600.0, 700.0, 1600.0])),
                ("sky_inner", SynthValue::Vec3([0.5, 0.25, 0.5])),
            ],
        ),
        SynthObject::dc(
            "dc.world_settings",
            "settings",
            [0.0; 3],
            [0.0; 3],
            &[
                ("far_z", SynthValue::Real(9000.0)),
                ("fog_enable", SynthValue::Bool(true)),
                ("fog_near", SynthValue::Real(128.0)),
                ("fog_far", SynthValue::Real(6000.0)),
                ("fog_color", SynthValue::Rgb([10.0, 20.0, 30.0])),
                ("sky_fog_enable", SynthValue::Bool(false)),
                ("sky_fog_near", SynthValue::Real(1.0)),
                ("sky_fog_far", SynthValue::Real(2.0)),
                ("wind", SynthValue::Vec3([1.0, 2.0, 3.0])),
                ("all_sky_portals", SynthValue::Bool(true)),
            ],
        ),
        SynthObject::dc(
            "dc.global_light",
            "sun",
            [0.0, 500.0, 0.0],
            [std::f32::consts::FRAC_PI_2, 0.0, 0.0],
            &[
                ("color", SynthValue::Rgb([200.0, 180.0, 160.0])),
                ("bright", SynthValue::Real(1.5)),
                ("to_ambient", SynthValue::Real(0.25)),
            ],
        ),
        SynthObject::dc(
            "dc.sound",
            "amb_1",
            [0.0, 50.0, 0.0],
            [0.0; 3],
            &[
                ("file", text("@mod:test/assets/wind.wav")),
                ("volume", SynthValue::Int(80.0)),
                ("inner_radius", SynthValue::Real(100.0)),
                ("outer_radius", SynthValue::Real(500.0)),
                ("loop", SynthValue::Bool(true)),
                ("ambient", SynthValue::Bool(true)),
            ],
        ),
        SynthObject::dc(
            "dc.sprite",
            "glow_1",
            [10.0, 60.0, 0.0],
            [0.0; 3],
            &[
                ("file", text("@mod:test/assets/glow.dtx")),
                ("scale_x", SynthValue::Real(2.0)),
                ("scale_y", SynthValue::Real(3.0)),
                ("color", SynthValue::Rgb([255.0, 128.0, 64.0])),
                ("alpha", SynthValue::Real(0.5)),
            ],
        ),
    ];
    SynthWorld {
        info: b"synthetic dc map".to_vec(),
        extent_min: [-2048.0, -256.0, -2048.0],
        extent_max: [2048.0, 2048.0, 2048.0],
        tree_shape: vec![false],
        models: vec![level, pit, ladder, crate_m, glass, sky],
        objects,
        ..SynthWorld::default()
    }
    .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_world_parses() {
        let d = synthetic_world_bytes();
        let w = world_from_bytes(&d, "Worlds\\Synth.dat", "Synth world").unwrap();
        assert_eq!(w.display, "Synth_world");
        assert_eq!(w.objects, 1);
        assert!(w.spawns.is_empty(), "made-up class has no role");
        assert_eq!(w.centre(), [0.0, 96.0, 0.0]);
        assert_eq!(w.size as usize, d.len());
    }

    /// MAP-20: the synthetic community map resolves through the vocabulary: spawns with their team texts and
    /// mode lists, zones, flags, brushes (blend text, sky inner vec3), volumes, the world variables (no file name
    /// to send), the global light, the sound and the sprite, the breakable, and the level geometry with its sky face.
    #[test]
    fn dc_world_resolves_through_the_vocabulary() {
        let d = synthetic_dc_world_bytes();
        let w = world_from_bytes(&d, "@mod:test/assets/maps/synth.dat", "dc map").unwrap();
        assert_eq!(w.spawns.len(), 8);
        assert_eq!(w.spawn_dc, vec![true; 8]);
        assert_eq!(w.spawns[0].team_filter.as_deref(), Some(&b"1"[..]));
        assert_eq!(w.spawns[2].team_filter.as_deref(), Some(&b"2"[..]));
        assert_eq!(w.spawns[6].team_filter.as_deref(), Some(&b""[..]));
        assert_eq!(w.spawn_modes[0].as_deref(), Some(&b"TDM,FFA"[..]));
        assert_eq!(w.spawn_modes[4].as_deref(), Some(&b"BOMB"[..]));
        assert_eq!(w.spawn_modes[6].as_deref(), Some(&b""[..]));
        assert_eq!(w.spawn_modes[7].as_deref(), Some(&b"flag"[..]));
        // zones: two bomb sites (number, half extents, the optional defuse team) and two flag zones (team)
        let bombs: Vec<&ZoneInfo> = w
            .zones
            .iter()
            .filter(|z| z.kind == ZoneKind::BombSite)
            .collect();
        assert_eq!(bombs.len(), 2);
        assert_eq!((bombs[0].number, bombs[0].team_text.as_deref()), (0, None));
        assert_eq!(bombs[0].dims, [128.0, 48.0, 128.0]);
        assert_eq!(
            (bombs[1].number, bombs[1].team_text.as_deref()),
            (1, Some(&b"2"[..]))
        );
        assert!(bombs[1].planted_command.is_empty());
        let zones: Vec<&ZoneInfo> = w
            .zones
            .iter()
            .filter(|z| z.kind == ZoneKind::FlagZone)
            .collect();
        assert_eq!(zones.len(), 2);
        assert_eq!(zones[0].team_text.as_deref(), Some(&b"1"[..]));
        assert_eq!(zones[1].team_text.as_deref(), Some(&b"2"[..]));
        assert_eq!(zones[0].dims, [96.0, 48.0, 96.0]);
        // flags: team 1's without a model (the default look), team 2's with an own path; both vocabulary objects
        assert_eq!(w.flags.len(), 2);
        assert_eq!(w.flags[0].model, None);
        assert!(w.flags[0].dc);
        assert_eq!(w.flags[0].team_text.as_deref(), Some(&b"1"[..]));
        assert_eq!(
            w.flags[1].model.as_deref(),
            Some("@mod:test/assets/flag.ltb")
        );
        // volumes: the kill volume (pending) and the ladder, at their model origins
        assert_eq!(w.volumes.len(), 2);
        let pit = w.volumes.iter().find(|v| v.name == b"pit").unwrap();
        assert_eq!(pit.kind, VolumeKind::KillVolume);
        assert!(pit.pending);
        assert_eq!(pit.pos, [500.0, 0.0, 500.0]);
        assert_eq!(
            (pit.friction, pit.viscosity, pit.tint),
            (1.0, 0.0, [255.0; 3])
        );
        let ladder = w.volumes.iter().find(|v| v.name == b"ladder_1").unwrap();
        assert_eq!(ladder.kind, VolumeKind::Ladder);
        assert_eq!(w.kill_shapes.len(), 1);
        assert_eq!(w.kill_shapes[0].pending_ms, 1500);
        assert!(w.kill_shapes[0].contains([500.0, 0.0, 500.0]));
        assert!(!w.kill_shapes[0].contains([500.0, 150.0, 500.0]));
        // brushes: the world model (additive), the breakable, the sky model with the split inner vec3
        let brush = |n: &[u8]| w.brushes.iter().find(|b| b.name == n).unwrap();
        let crate_1 = brush(b"crate_1");
        assert_eq!(crate_1.class, BrushClass::Plain);
        assert_eq!(crate_1.blend, 2);
        assert!(crate_1.visible && crate_1.solid && crate_1.rayhit);
        assert_eq!(crate_1.pos, [0.0, 50.0, 0.0]);
        assert_eq!(brush(b"glass_1").class, BrushClass::Breakable);
        let sky = brush(b"sky_1");
        assert_eq!(sky.class, BrushClass::Sky);
        assert_eq!(
            sky.sky,
            Some(SkyInfo {
                index: 0,
                dims: [1600.0, 700.0, 1600.0],
                inner: [0.5, 0.25, 0.5],
            })
        );
        assert_eq!(
            w.brushes.len(),
            3,
            "kill volume and ladder are volumes, not brushes"
        );
        assert_eq!(w.destructibles.breakables.len(), 1);
        assert_eq!(w.destructibles.breakables[0].hit_points, 50.0);
        // world settings: fourteen evar rows, every one without a file name (resolved from the ROM)
        let labels: Vec<&str> = w.world_vars.iter().map(|v| v.label).collect();
        assert_eq!(
            labels,
            vec![
                "far_z",
                "fog_enable",
                "fog_near_z",
                "fog_far_z",
                "fog_r",
                "fog_g",
                "fog_b",
                "sky_fog_enable",
                "sky_fog_near",
                "sky_fog_far",
                "all_sky_portals",
                "wind_x",
                "wind_y",
                "wind_z"
            ]
        );
        assert!(w.world_vars.iter().all(|v| v.name.is_none()));
        let value = |l: &str| {
            String::from_utf8(
                w.world_vars
                    .iter()
                    .find(|v| v.label == l)
                    .unwrap()
                    .value
                    .clone(),
            )
            .unwrap()
        };
        assert_eq!(value("far_z"), "9000.000000");
        assert_eq!(value("fog_enable"), "1");
        assert_eq!(value("fog_g"), "20.000000");
        assert_eq!(value("sky_fog_enable"), "0");
        assert_eq!(value("wind_z"), "3.000000");
        assert_eq!(value("all_sky_portals"), "1");
        // the global light: colour x bright, the convert-to-ambient, the direction of the pitched rotation
        let l = w.global_light.expect("global light");
        assert_eq!(l.colour, [300.0, 270.0, 240.0]);
        assert_eq!(l.extra, 0.25);
        assert!((l.dir[1] + 1.0).abs() < 1e-5, "{:?}", l.dir);
        // the sound and the sprite
        assert_eq!(w.sounds.len(), 1);
        let s = &w.sounds[0];
        assert_eq!(s.file, "@mod:test/assets/wind.wav");
        assert_eq!((s.volume, s.inner, s.outer), (80, 100.0, 500.0));
        assert!(s.looped && s.ambient && s.start_on && !s.attached);
        assert_eq!(w.sprites.len(), 1);
        let sp = &w.sprites[0];
        assert_eq!(sp.file, "@mod:test/assets/glow.dtx");
        assert_eq!(sp.scale, [2.0, 3.0]);
        assert_eq!(sp.color, [255.0, 128.0, 64.0]);
        assert_eq!(sp.alpha, 0.5);
        assert!(w.effects.is_empty());
        // the level: a ray down meets the floor (surface id 7), a ray up the sky face (flag 0x10)
        let t = w.trace.as_ref().expect("level model");
        let down = t
            .ray(
                dc_sim::V3::new(700.0, 500.0, 100.0),
                dc_sim::V3::new(700.0, -100.0, 100.0),
            )
            .expect("floor");
        assert_eq!(down.surface_flags & SURF_SKY, 0);
        let up = t
            .ray(
                dc_sim::V3::new(700.0, 500.0, 100.0),
                dc_sim::V3::new(700.0, 1500.0, 100.0),
            )
            .expect("sky");
        assert_ne!(up.surface_flags & SURF_SKY, 0);
        assert_eq!(
            w.surfaces.as_ref().unwrap().below([700.0, 10.0, 100.0]),
            Some(DC_SYNTH_FLOOR_SURFACE as u8)
        );
    }

    /// MAP-20 (ROM-gated): `-worldfile` on a community map with a ROM at hand resolves the client names of every
    /// evar row and the client variables from the user's own CShell.dll; without a ROM they stay unresolved.
    #[test]
    fn worldfile_with_a_rom_resolves_the_evar_names() {
        let rom = dc_rez::dc_rom::rom_or_skip!();
        let dir = std::env::temp_dir().join(format!("dc_map_evars_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("synth_dc.dat");
        std::fs::write(&path, synthetic_dc_world_bytes()).unwrap();
        let plain = load_from_file(&path, "@mod:test/assets/maps/synth_dc.dat").unwrap();
        assert_eq!(plain.world_vars.len(), 14);
        assert!(plain.world_vars.iter().all(|v| v.name.is_none()));
        assert!(plain.client_vars.step_height.is_none());
        let with = load_from_file_with_rom(&path, "@mod:test/assets/maps/synth_dc.dat", Some(&rom))
            .unwrap();
        assert_eq!(with.world_vars.len(), 14);
        for v in &with.world_vars {
            let n = v
                .name
                .as_ref()
                .unwrap_or_else(|| panic!("{} unresolved", v.label));
            assert_eq!((dc_world::hash16(n), n.len()), v.key, "{}", v.label);
        }
        assert!(with.client_vars.step_height.is_some());
        assert!(with.client_vars.world_north.is_some());
        assert_eq!(crate::evars::evar_messages(&with.world_vars, &[]).len(), 14);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn entry_lookup_and_name_forms() {
        let entries = vec![
            ("WORLDS\\ALPHA.DAT".to_string(), 0, 0),
            ("WORLDS\\Beta_Two.DAT".to_string(), 0, 0),
        ];
        assert_eq!(
            find_entry(&entries, "Worlds\\Alpha"),
            Some("WORLDS\\ALPHA.DAT")
        );
        assert_eq!(
            find_entry(&entries, "worlds/beta_two.dat"),
            Some("WORLDS\\Beta_Two.DAT")
        );
        assert_eq!(
            find_entry(&entries, "Beta_Two"),
            Some("WORLDS\\Beta_Two.DAT")
        );
        assert_eq!(find_entry(&entries, "Gamma"), None);
        assert_eq!(
            announce_name(
                "WORLDS\\Beta_Two.DAT",
                "Worlds\\Beta_Two",
                NameForm::NoExtension
            ),
            "WORLDS\\Beta_Two"
        );
        assert_eq!(
            announce_name(
                "WORLDS\\Beta_Two.DAT",
                "Worlds\\Beta_Two",
                NameForm::Argument
            ),
            "Worlds\\Beta_Two"
        );
        assert_eq!(
            announce_name("WORLDS\\Beta_Two.DAT", "x", NameForm::Listed),
            "WORLDS\\Beta_Two.DAT"
        );
    }
}
