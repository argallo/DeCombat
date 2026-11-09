//! Doors, slice 1 (DSV-46): the world's sliding doors are created on every client in the form the reference stack used (a
//! kind-2 named brush with the type-0x39 door blob, full-precision position at the model origin, door flag words),
//! closed at the start; a door opens when a living player comes near and closes a while after the last one left,
//! moving along its own move direction and distance at the speed its opening and closing times give. Every tick a
//! door moves, the clients get one change record with its position and velocity.
//!
//! Rotating doors a player may open and rotating switches (w1-doors, [`turn_infos`], [`TurnDoor`]): turned about
//! their rotation point with the same open rule; wired into the session by the integrator (see the w1-doors report).
//!
//! Who opens a door follows the world file's own properties (DSV-270, DSV-460): a door the file does not mark
//! player-activated never opens for a player near it, only by another object's `msg <door> on` / `off`; a
//! player-activated door opens and closes with the use key only (its activation entry, the 2009 use prompt), and its
//! linked door (`DoorLink`) moves with it; only a non-player door that nothing in the world's doors drives keeps the
//! near-player rule. The doors' opening / open / closing / closed
//! commands run `msg <door> on|off|lock|unlock` and `delay <secs> (<statement>)` on the doors ([`run_commands`]).
//!
//! A door plays its opening / closing sound files at the start of each move (DSV-302).
//!
//! Rotating doors that players do not open are created closed and turn by messages only (DSV-500: WarHead's spawn
//! gate in front of a fall volume, opened by the quarantine mode's start trigger only).
//!
//! Not yet: the door as a solid for the server's own traces.
// SPEC: UDP_SESSION_2009_B.md §3.3 (kind 2 create), §4.2 items 3, 6 (flag words, full-precision position and
// velocity); GAME_MESSAGES_2009_S2C.md §8.2 (type 0x39: class-read object)
// DISASM: cshell+0x222480 case 0x39 -> cshell+0x2216d0 -> cshell+0x2e2090 (class method table VA 0x374fb06c, read slot
// +0x10 = cshell+0x2e2230: `[a:u8][b:bool1][c:u8]` after the type byte, kept at +0x38, +0x39, +0x3c; meanings
// UNPROVEN)
// OBSERVED: capture 20261025_002236_tdm4 (the tdm4 world's three sliding doors: kind 2, blob
// 25 bits `39 00 02 00` = type 0x39, a 0, b 0, c 1; flags 0x3101 / 0 / 0x00200121 or 0x14200121 / 0; full-precision
// position = the model origin; no change record of a door in the whole capture, nor in tdm1 / tdm2 on the same world:
// the moving form is not observed); tdm1 (the same creates, flags 0x3501 then a flag change to 0x3101)
// DC-DEFAULT: DSV-46 (proposed)

use crate::objects::{write_record_head, CF_POS};
use crate::world::PropHash;
use dc_wire::udp::BitWriter;

/// Client object type of a door.
// DISASM: cshell+0x222480 (factory case 0x39)
pub const OBJ_DOOR_TYPE: u8 = 0x39;
/// The three fields of the door blob as the reference stack sent them for every sliding door.
// OBSERVED: tdm4 (`[0][0][1]` on all three doors; REFERENCE, accepted); SPEC: UNPROVEN meanings (`c` equals the doors'
// AWMType property 1 in that world, PLAUSIBLY that property)
pub const TODO_DOOR_BLOB: (u8, bool, u8) = (0, false, 1);

/// Class and property keys of a sliding door (hash of the name and length).
// OBSERVED: the ROM worlds' sliding door objects (hashes only)
pub const CLASS_SLIDING_DOOR: PropHash = crate::world::CLASS_SLIDING_DOOR;
pub const PROP_MOVE_DIR: PropHash = ([0x7f, 0xb2, 0xc8, 0x61, 0x41, 0x65, 0xe7, 0xf8], 7);
pub const PROP_MOVE_DIST: PropHash = ([0xbe, 0xfb, 0xca, 0xbe, 0x08, 0x8b, 0x89, 0x6b], 8);
pub const PROP_OPENING_TIME: PropHash = ([0x4c, 0x20, 0xc0, 0x99, 0x01, 0x73, 0xe8, 0xf4], 11);
pub const PROP_CLOSING_TIME: PropHash = ([0x99, 0xad, 0xab, 0x65, 0xb3, 0x85, 0xda, 0x5a], 11);
/// The door that moves together with this one (the other wing of a double door).
// OBSERVED: the ROM world files' door objects (hashes only; Warhead's B-site pair names each other)
pub const PROP_DOOR_LINK: PropHash = ([0x6f, 0x33, 0x50, 0x79, 0x81, 0xb1, 0x72, 0xea], 8);

/// How near a living player must come to open a door (distance from its position to the door's box), and how long
/// after the last player left the door closes.
// DC-DEFAULT: DSV-46 (proposed) 96 units, 3000 ms
pub const DOOR_OPEN_RADIUS: f32 = 96.0;
pub const DOOR_CLOSE_AFTER_MS: u32 = 3000;
/// Moving time when the world gives none (an opening or closing time of 0).
// DC-DEFAULT: DSV-46 (proposed) 1000 ms
pub const DOOR_DEFAULT_MOVE_MS: u32 = 1000;

/// Property keys of a door's command texts: run when it starts opening, is open, starts closing, is closed.
// OBSERVED: the ROM world files' door objects (hashes only)
pub const PROP_OPENING_COMMAND: PropHash = ([0x7b, 0x4c, 0xfe, 0xc1, 0x0d, 0xe1, 0x6b, 0x4c], 14);
pub const PROP_OPEN_COMMAND: PropHash = ([0xba, 0x49, 0x27, 0xfc, 0xd2, 0x4c, 0x06, 0xc8], 11);
pub const PROP_CLOSING_COMMAND: PropHash = ([0x33, 0xa8, 0x57, 0x1c, 0x5a, 0x2c, 0x06, 0x30], 14);
pub const PROP_CLOSED_COMMAND: PropHash = ([0xb8, 0xe6, 0xbc, 0x51, 0x6a, 0xa6, 0x41, 0xf1], 13);
/// The command and the sound of a use on a locked door (DSV-560).
// OBSERVED: the ROM world files' door objects (hashes only; SANDHOG_CP1's locked gates name a locked sound, and four
// of them a locked command that fires the trigger unlocking the other side's gates)
pub const PROP_LOCKED_COMMAND: PropHash = ([0x9a, 0xd8, 0x77, 0xee, 0xb9, 0xc1, 0x68, 0x52], 13);
pub const PROP_LOCKED_SOUND: PropHash = ([0x31, 0xfc, 0x06, 0xf3, 0xfb, 0xfb, 0xb1, 0xa2], 11);
/// A rotating switch's command texts and sounds when it is switched on / off, and its turn times (DSV-561).
// OBSERVED: the ROM world files' rotating switch objects (hashes only; OVERDOSE's two light switches: on / off
// commands firing the room's light-on / light-off trigger, one switch sound for both, turn times 0.01 s)
pub const PROP_ON_COMMAND: PropHash = ([0x76, 0x7b, 0xe3, 0xb7, 0x8a, 0xbb, 0x9a, 0x04], 9);
pub const PROP_OFF_COMMAND: PropHash = ([0x6c, 0xc2, 0x20, 0xfe, 0x56, 0x27, 0x6b, 0xcf], 10);
pub const PROP_ON_SOUND: PropHash = ([0xf5, 0x3c, 0x58, 0x67, 0xb5, 0x9f, 0x86, 0x9b], 7);
pub const PROP_OFF_SOUND: PropHash = ([0x69, 0xf8, 0x77, 0xf2, 0x57, 0x97, 0x75, 0x77], 8);
pub const PROP_POWER_ON_TIME: PropHash = ([0xb5, 0x7e, 0x1a, 0x52, 0xe4, 0x14, 0x62, 0xc2], 11);
pub const PROP_POWER_OFF_TIME: PropHash = ([0x71, 0x5e, 0x73, 0x12, 0x24, 0xe1, 0x56, 0x37], 12);

/// Property keys of a door's sounds: the file played when it starts to open / to close, the sound's place relative
/// to the door and its radius (DSV-302).
// OBSERVED: the ROM world files' door objects (hashes only; Snow Valley's three sliding doors name one sliding-door
// file for both, place (0, 0, 0), radius 1000)
pub const PROP_OPENING_SOUND: PropHash = ([0x73, 0xb1, 0x38, 0x2e, 0xa4, 0xf8, 0x17, 0x6f], 12);
pub const PROP_CLOSING_SOUND: PropHash = ([0x69, 0x89, 0xfc, 0xb9, 0x0c, 0xcc, 0x90, 0x89], 12);
pub const PROP_SOUND_POS: PropHash = ([0xa3, 0xa7, 0xa2, 0x14, 0xa9, 0xf0, 0xbf, 0x7a], 8);
pub const PROP_SOUND_RADIUS: PropHash = ([0x8f, 0xa1, 0x2b, 0x3a, 0x5b, 0x66, 0x63, 0x35], 11);
/// A door's sound radius when the file gives none.
// DC-DEFAULT: DSV-302 (proposed) 1000 units
pub const DOOR_SOUND_RADIUS: f32 = 1000.0;

/// The sounds of a door from its world file properties.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DoorSounds {
    pub opening: String,
    pub closing: String,
    /// Where the sound plays relative to the door's position.
    pub offset: [f32; 3],
    pub radius: f32,
    /// Played when a player uses the door while it is locked (DSV-560).
    pub locked: String,
}

/// The door's sounds (empty names: none).
// DC-DEFAULT: DSV-302 (proposed) the opening / closing file at the door's start of each move
pub fn door_sounds(o: &dc_world::Object<'_>) -> DoorSounds {
    door_sounds_of(o, PROP_OPENING_SOUND, PROP_CLOSING_SOUND)
}

/// [`door_sounds`] with the given opening / closing sound keys (a switch names its on / off sounds, DSV-561).
fn door_sounds_of(o: &dc_world::Object<'_>, opening: PropHash, closing: PropHash) -> DoorSounds {
    let name = |k| String::from_utf8_lossy(&text(o, k)).trim().to_string();
    DoorSounds {
        locked: name(PROP_LOCKED_SOUND),
        opening: name(opening),
        closing: name(closing),
        offset: match prop(o, PROP_SOUND_POS) {
            Some(dc_world::FieldValue::Vec3(v)) if v.iter().all(|c| c.is_finite()) => *v,
            _ => [0.0; 3],
        },
        radius: real(o, PROP_SOUND_RADIUS)
            .filter(|r| *r > 0.0)
            .unwrap_or(DOOR_SOUND_RADIUS),
    }
}

impl DoorSounds {
    /// The file of a move that starts now (opening or closing), `None` when the door names none.
    pub fn of_move(&self, opening: bool) -> Option<&str> {
        let f = if opening {
            &self.opening
        } else {
            &self.closing
        };
        (!f.is_empty()).then_some(f.as_str())
    }
}

/// How near (units from the player's position to the door's box) the use key reaches a door.
// DC-DEFAULT: DSV-270 (proposed) 128 units
pub const DOOR_USE_REACH: f32 = 128.0;

/// Who opens a door.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenBy {
    /// A living player near it (DSV-46 / DSV-143, ours).
    #[default]
    Near,
    /// The use key of a player within [`DOOR_USE_REACH`] toggles it (every player-activated door, DSV-460), and
    /// messages.
    Use,
    /// Only another object's message (the file does not mark it player-activated).
    Message,
}

/// A door's command texts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DoorCmds {
    pub opening: Vec<u8>,
    pub open: Vec<u8>,
    pub closing: Vec<u8>,
    pub closed: Vec<u8>,
    /// Run when a player uses the door while it is locked (DSV-560).
    pub locked: Vec<u8>,
}

impl DoorCmds {
    /// The objects its commands send `on` to (immediate or delayed).
    pub fn on_targets(&self) -> Vec<Vec<u8>> {
        [&self.opening, &self.open, &self.closing, &self.closed]
            .iter()
            .flat_map(|t| parse_statements(t))
            .filter(|st| st.word == WORD_ON)
            .map(|st| st.target)
            .collect()
    }
}

fn text(o: &dc_world::Object<'_>, k: PropHash) -> Vec<u8> {
    match prop(o, k) {
        Some(dc_world::FieldValue::Text(t)) => t.to_vec(),
        _ => Vec::new(),
    }
}

fn door_cmds(o: &dc_world::Object<'_>) -> DoorCmds {
    DoorCmds {
        opening: text(o, PROP_OPENING_COMMAND),
        open: text(o, PROP_OPEN_COMMAND),
        closing: text(o, PROP_CLOSING_COMMAND),
        closed: text(o, PROP_CLOSED_COMMAND),
        locked: text(o, PROP_LOCKED_COMMAND),
    }
}

/// A rotating switch's command texts: its on command runs when it is on (as a door's open command), its off command
/// when it is off (as a door's closed command).
// DC-DEFAULT: DSV-561 (proposed)
fn switch_cmds(o: &dc_world::Object<'_>) -> DoorCmds {
    DoorCmds {
        open: text(o, PROP_ON_COMMAND),
        closed: text(o, PROP_OFF_COMMAND),
        locked: text(o, PROP_LOCKED_COMMAND),
        ..DoorCmds::default()
    }
}

/// A door object of the world for the open rule: name, the player-activate switch, the `on` targets of its commands.
pub type DoorRuleEntry = (Vec<u8>, bool, Vec<Vec<u8>>);

/// The rule entries of every sliding and rotating door object of the world.
pub fn rule_entries(w: &dc_world::World<'_>) -> Vec<DoorRuleEntry> {
    w.objects
        .iter()
        .filter(|o| {
            let c = (dc_world::hash16(o.class_name), o.class_name.len());
            c == CLASS_SLIDING_DOOR || c == CLASS_ROTATING_DOOR
        })
        .map(|o| {
            (
                o.name().map(<[u8]>::to_vec).unwrap_or_default(),
                prop_bool(o, PROP_PLAYER_ACTIVATE).unwrap_or(false),
                door_cmds(o).on_targets(),
            )
        })
        .collect()
}

/// The open rule of door `name` among the world's doors `all`.
// DC-DEFAULT: DSV-460 (proposed, user 2026-10-30 "the doors at the B site open automatically; in 2009 they are not
// proximity-triggered") amends DSV-270: every player-activated door opens by the use key. The door classes carry no
// touch property (the trigger class has one, the doors do not); the player-activate property is what enables the
// door's activate entry, the 2009 use prompt (DSV-340, CLI-1230: cshell+0x2e2230, cshell+0x2b2da0), and the door's
// activate type names the open / close row of that prompt
// OBSERVED: ROM door survey 2026-10-30 (Warhead's three B-site sliding doors: player-activated, activate type
// open / close, no touch property, an open command that closes them again 5 s later, two of them linked)
// DC-DEFAULT: DSV-270 (proposed, playtest report "the garage doors should only open when you press the button not
// activated by walking up to them") from the world file's own properties: a door not marked player-activated that
// another door's commands switch on opens only by those messages; a player-activated door whose commands switch on
// such a door is its lever / button and turns with the use key only; every other door keeps the near-player rule
// (DSV-46 / DSV-143; a non-player door that no door switches on is opened by world triggers we do not run, and a
// door that never opens would be a wall)
pub fn open_by(name: &[u8], all: &[DoorRuleEntry]) -> OpenBy {
    let Some((_, pa, _)) = all.iter().find(|e| e.0.eq_ignore_ascii_case(name)) else {
        return OpenBy::Near;
    };
    let other = |n: &[u8]| !n.eq_ignore_ascii_case(name);
    if !pa {
        let driven = all
            .iter()
            .any(|e| other(&e.0) && e.2.iter().any(|t| t.eq_ignore_ascii_case(name)));
        return if driven {
            OpenBy::Message
        } else {
            OpenBy::Near
        };
    }
    OpenBy::Use
}

/// A sliding door of the world file.
#[derive(Debug, Clone, PartialEq)]
pub struct DoorInfo {
    pub name: Vec<u8>,
    /// The closed position (the model origin) and the model's box.
    pub closed: [f32; 3],
    pub lo: [f32; 3],
    pub hi: [f32; 3],
    /// Unit move direction and distance.
    pub dir: [f32; 3],
    pub dist: f32,
    pub open_ms: u32,
    pub close_ms: u32,
    pub open_by: OpenBy,
    pub cmds: DoorCmds,
    pub sounds: DoorSounds,
    /// A player may use it (the file marks it player-activated): its activate entry is enabled while it is not locked.
    pub usable: bool,
    /// The file marks it locked: it starts locked until a script unlocks it (DSV-560).
    pub locked: bool,
    /// The linked door's name (empty: none, DSV-460).
    pub link: Vec<u8>,
}

fn prop<'o, 'a>(o: &'o dc_world::Object<'a>, k: PropHash) -> Option<&'o dc_world::FieldValue<'a>> {
    o.props
        .iter()
        .find(|p| p.name.len() == k.1 && dc_world::hash16(p.name) == k.0)
        .map(|p| &p.value)
}

fn real(o: &dc_world::Object<'_>, k: PropHash) -> Option<f32> {
    match prop(o, k)? {
        dc_world::FieldValue::Real(r) if r.is_finite() => Some(*r),
        _ => None,
    }
}

fn secs_ms(s: Option<f32>) -> u32 {
    match s {
        Some(s) if s > 0.0 => (s * 1000.0).min(600_000.0) as u32,
        _ => DOOR_DEFAULT_MOVE_MS,
    }
}

/// The sliding doors of a parsed world (the ones linked to a model, with a move direction and a distance).
// SPEC: FMT_DAT85.md §6.4 (model link by name)
pub fn door_infos(w: &dc_world::World<'_>) -> Vec<DoorInfo> {
    let rules = rule_entries(w);
    let mut out = Vec::new();
    for o in &w.objects {
        if (dc_world::hash16(o.class_name), o.class_name.len()) != CLASS_SLIDING_DOOR {
            continue;
        }
        let Some(m) = w.brush_of(o) else {
            continue;
        };
        let dir = match prop(o, PROP_MOVE_DIR) {
            Some(dc_world::FieldValue::Vec3(v)) => *v,
            _ => continue,
        };
        let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        let dist = real(o, PROP_MOVE_DIST).unwrap_or(0.0);
        if !(len.is_finite() && len > 0.0 && dist > 0.0) {
            continue;
        }
        let open_ms = secs_ms(real(o, PROP_OPENING_TIME));
        // a closing time of 0 moves back at the opening speed (DSV-46)
        let close_ms = match real(o, PROP_CLOSING_TIME) {
            Some(s) if s > 0.0 => secs_ms(Some(s)),
            _ => open_ms,
        };
        let cmds = door_cmds(o);
        let name = o.name().map(<[u8]>::to_vec).unwrap_or_default();
        out.push(DoorInfo {
            open_by: open_by(&name, &rules),
            name,
            closed: m.origin,
            lo: m.box_min,
            hi: m.box_max,
            dir: dir.map(|c| c / len),
            dist,
            open_ms,
            close_ms,
            cmds,
            sounds: door_sounds(o),
            // DC-DEFAULT: DSV-560 (proposed) a player door the file locks is a use door that starts locked
            usable: prop_bool(o, PROP_PLAYER_ACTIVATE).unwrap_or(false),
            locked: prop_bool(o, PROP_LOCKED).unwrap_or(false),
            link: text(o, PROP_DOOR_LINK),
        });
    }
    out
}

/// The door blob `[0x39][a:u8][b:bool1][c:u8]` as the reference stack sent it.
pub fn door_blob() -> BitWriter {
    let (a, b, c) = TODO_DOOR_BLOB;
    door_blob_of(a, b, c)
}

/// The door blob is the client's activate entry: `a` = the activate-type row, `b` = disabled, `c` = the state (the
/// row field the target text comes from: 0 on, 1 off).
// DISASM: cshell+0x2e2230 (entry at +0x38: row, disabled, state; registered in the activate list), cshell+0x2b30c2..
// 0x2b3193 (the target code: the entry of the object under the cross within the activation distance; row field
// [state] is the target text id; enabled = !disabled), cshell+0x2e5a00 (row fields: on, off, two sliding states)
pub fn door_blob_of(row: u8, disabled: bool, state: u8) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(OBJ_DOOR_TYPE)
        .write_u8(row)
        .write_bit(disabled)
        .write_u8(state);
    w
}

/// The activate-type row of every door.
// OBSERVED: the ROM world files' sliding and rotating doors (2026-10-28 scratch survey over dc-world: every door of
// every world names the same activate type, the table's first row; the switches name none)
// DC-DEFAULT: DSV-340 (proposed) row 0 for every door; DSV-561 (proposed) and for every rotating switch a player
// uses (the switches name no activate type, so the 2009 target text would be none; row 0's texts are the ones the
// prompt element draws its use image for, cshell+0x82b34)
pub const DOOR_ACTIVATE_ROW: u8 = 0;
/// Activate states: open (the "close the door" text) and closed (the "open the door" text).
// DISASM: cshell+0x2e5a00 (field 0 = on, field 1 = off)
pub const ACTIVATE_ON: u8 = 0;
pub const ACTIVATE_OFF: u8 = 1;
/// Sub-ids of the door's object note (shell 0xa6 to a type 0x39 object): `[1][disabled:bool1]`, `[2][state:u8]`.
// DISASM: cshell+0x2e1a70 (slot +0x30: sub 1 -> +0x39 disabled, sub 2 -> +0x3c state, other subs ignored)
pub const NOTE_DISABLED: u8 = 1;
pub const NOTE_STATE: u8 = 2;

/// The activate entry `(disabled, state)` of a door: disabled when a player may not use it (not player-activated,
/// locked in the file or by a message); state on while it opens or stands open.
// DC-DEFAULT: DSV-340 (proposed) the state follows the move's way, sent when it changes
pub fn activate_of(usable: bool, locked: bool, opening: bool) -> (bool, u8) {
    (
        !usable || locked,
        if opening { ACTIVATE_ON } else { ACTIVATE_OFF },
    )
}

/// Change record of a moving door: `cf` = position, the full-precision position and velocity.
// SPEC: UDP_SESSION_2009_B.md §4.2 item 6 (CF_POS: full precision = raw position + raw velocity)
// DC-DEFAULT: DSV-46 (proposed) one record per tick while the door moves (the reference stack's moving form is not observed)
pub fn door_move_record(obj: u16, pos: [f32; 3], vel: [f32; 3]) -> BitWriter {
    let mut w = BitWriter::new();
    write_record_head(&mut w, CF_POS, obj);
    for c in pos {
        w.write_f32(c);
    }
    for c in vel {
        w.write_f32(c);
    }
    w
}

/// One door on the clients.
#[derive(Debug, Clone, PartialEq)]
pub struct Door {
    pub obj: u16,
    pub info: DoorInfo,
    /// How far it is open (0 = closed, `info.dist` = open).
    pub offset: f32,
    /// Opening (true) or closing.
    pub opening: bool,
    /// When a player was last near.
    pub last_near: Option<dc_net_engine::Millis>,
    /// Use / message doors: the state asked for, the lock (the use key does nothing), command texts to run.
    pub want: bool,
    pub locked: bool,
    pub fired: Vec<Vec<u8>>,
    /// The way of the move in progress (`None` at rest) and the sound files of the moves started since the last
    /// take (DSV-302).
    pub moving: Option<bool>,
    pub sounds_due: Vec<String>,
    /// When the use key last ran its locked command (DSV-560 cooldown).
    pub last_use: Option<dc_net_engine::Millis>,
}

impl Door {
    pub fn new(obj: u16, info: DoorInfo) -> Door {
        // DSV-560: the file's lock is the door's lock at the start
        let locked = info.locked;
        Door {
            obj,
            info,
            offset: 0.0,
            opening: false,
            last_near: None,
            want: false,
            locked,
            fired: Vec::new(),
            moving: None,
            sounds_due: Vec::new(),
            last_use: None,
        }
    }

    /// Asks a use / message door to open or close: the opening / closing command runs when the state changes.
    pub fn set_open(&mut self, open: bool) {
        if self.info.open_by == OpenBy::Near || self.want == open {
            return;
        }
        self.want = open;
        let c = if open {
            &self.info.cmds.opening
        } else {
            &self.info.cmds.closing
        };
        self.fired.push(c.clone());
    }

    pub fn pos(&self) -> [f32; 3] {
        let i = &self.info;
        [0, 1, 2].map(|k| i.closed[k] + i.dir[k] * self.offset)
    }

    /// The activate entry `(disabled, state)` now (DSV-340).
    pub fn activate(&self) -> (bool, u8) {
        activate_of(self.info.usable, self.locked, self.opening)
    }

    /// Distance from a point to the box the door sweeps (closed to open).
    fn distance(&self, p: [f32; 3]) -> f32 {
        let i = &self.info;
        let mut d2 = 0.0;
        for (k, &c) in p.iter().enumerate() {
            let a = i.lo[k].min(i.lo[k] + i.dir[k] * i.dist);
            let b = i.hi[k].max(i.hi[k] + i.dir[k] * i.dist);
            let e = if c < a {
                a - c
            } else if c > b {
                c - b
            } else {
                0.0
            };
            d2 += e * e;
        }
        d2.sqrt()
    }

    /// One step of `dt_ms`: the target from the players' positions, then the move. Returns the record to send when
    /// the door moved (position and velocity; velocity 0 on the step that ends the move).
    pub fn step(
        &mut self,
        now: dc_net_engine::Millis,
        dt_ms: u32,
        players: &[[f32; 3]],
    ) -> Option<([f32; 3], [f32; 3])> {
        if self.info.open_by != OpenBy::Near {
            self.opening = self.want;
        } else if players
            .iter()
            .any(|p| self.distance(*p) <= DOOR_OPEN_RADIUS)
        {
            self.last_near = Some(now);
            self.opening = true;
        } else if self
            .last_near
            .is_none_or(|t| dc_net_engine::time::since(now, t) >= DOOR_CLOSE_AFTER_MS)
        {
            self.opening = false;
        }
        let i = &self.info;
        let (target, ms) = if self.opening {
            (i.dist, i.open_ms)
        } else {
            (0.0, i.close_ms)
        };
        if (self.offset - target).abs() < 1e-4 {
            self.moving = None;
            return None;
        }
        // DSV-302: a move that starts (from rest or turning back) plays its sound
        if self.moving != Some(self.opening) {
            self.moving = Some(self.opening);
            if let Some(f) = i.sounds.of_move(self.opening) {
                self.sounds_due.push(f.to_string());
            }
        }
        let speed = i.dist / (ms.max(1) as f32 / 1000.0);
        let stepd = speed * dt_ms as f32 / 1000.0;
        let sign = if target > self.offset { 1.0 } else { -1.0 };
        let left = (target - self.offset).abs();
        let done = stepd >= left;
        self.offset = if done {
            target
        } else {
            self.offset + sign * stepd
        };
        if done {
            self.moving = None;
        }
        let vel = if done {
            [0.0; 3]
        } else {
            i.dir.map(|c| c * sign * speed)
        };
        if done && i.open_by != OpenBy::Near {
            let c = if self.opening {
                &i.cmds.open
            } else {
                &i.cmds.closed
            };
            self.fired.push(c.clone());
        }
        Some((self.pos(), vel))
    }
}

// ---- w1-doors: rotating doors and switches (DSV-143 .. DSV-145, proposed) ----
//
// The world's rotating doors turn about their rotation point by their rotation angles: the player-activated ones by
// the use key (DSV-460; the file's locked ones start locked, DSV-560), the others by messages (DSV-500); a
// player-activated rotating switch turns on and off by the use key and runs its on / off commands (DSV-561). Each tick
// a door turns, the clients get one change record with its position and rotation. No 2009 capture of a turning door
// exists: the open rule, the turn form and the record form are ours.

/// Class keys of a rotating door and of a rotating switch, and their properties (hash of the name and length): the
/// rotation angles (degrees about the object's own x, y, z axes), the rotation point (an object name; empty: the door
/// object's own position), player activation, the lock, the start-open switch, the turn-away switch.
// OBSERVED: class and property names of the ROM world files (hashes only; 2026-10-26 scratch survey over dc-world:
// 42 rotating doors on 6 worlds, every one with an empty rotation point and one non-zero angle of +-90 or 180
// degrees about a single axis; 2 rotating switches on 1 world, -10 degrees)
pub const CLASS_ROTATING_DOOR: PropHash = crate::world::CLASS_ROTATING_DOOR;
pub const CLASS_ROTATING_SWITCH: PropHash = crate::world::CLASS_SWITCH;
pub const PROP_ROTATION_ANGLES: PropHash = ([0xad, 0x84, 0xf5, 0xaf, 0xa0, 0xed, 0xb3, 0xe2], 14);
pub const PROP_ROTATION_POINT: PropHash = ([0x50, 0x27, 0x88, 0xe3, 0xa3, 0x01, 0xae, 0x8c], 13);
pub const PROP_PLAYER_ACTIVATE: PropHash = ([0x96, 0x20, 0x02, 0x85, 0x1a, 0xb3, 0xda, 0x56], 14);
pub const PROP_LOCKED: PropHash = ([0xa4, 0x24, 0xe3, 0x3d, 0x90, 0x93, 0x1d, 0x1e], 6);
pub const PROP_START_OPEN: PropHash = ([0x04, 0x6a, 0xdb, 0xfe, 0x73, 0x48, 0x11, 0x20], 9);
pub const PROP_ROTATE_AWAY: PropHash = ([0x66, 0xa7, 0xfd, 0xb1, 0x1e, 0xf3, 0x0c, 0xbe], 10);
/// A switch's start-on property (the switches' form of the start-open switch).
// OBSERVED: the ROM world files' rotating switch objects (hashes only)
pub const PROP_START_ON: PropHash = ([0x0e, 0xf0, 0xd5, 0xb4, 0xd0, 0x98, 0xb9, 0xe0], 7);

/// What a turning model of the world is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnKind {
    /// A rotating door: turns open near a player, back after.
    Door,
    /// A rotating switch: turns on and off by the use key of a player (when the file marks it player-activated) and
    /// by messages; its on / off commands run when it is on / off (DSV-561).
    Switch,
}

/// A rotating door or switch of the world file.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnInfo {
    pub name: Vec<u8>,
    pub kind: TurnKind,
    /// The model origin (the object's closed position on the clients) and the model's box.
    pub origin: [f32; 3],
    pub lo: [f32; 3],
    pub hi: [f32; 3],
    /// The point it turns about and the open turn: a unit world axis and the angle (radians, the authored sign).
    pub pivot: [f32; 3],
    pub axis: [f32; 3],
    pub angle: f32,
    pub open_ms: u32,
    pub close_ms: u32,
    /// Turns away from the player who opens it (the turn-away switch).
    pub away: bool,
    /// Created open, and stays open (the start-open switch).
    pub start_open: bool,
    pub open_by: OpenBy,
    pub cmds: DoorCmds,
    pub sounds: DoorSounds,
    /// The linked door's name (empty: none, DSV-460).
    pub link: Vec<u8>,
    /// The file marks it locked: it starts locked until a script unlocks it (DSV-560).
    pub locked: bool,
}

fn prop_bool(o: &dc_world::Object<'_>, k: PropHash) -> Option<bool> {
    match prop(o, k)? {
        dc_world::FieldValue::Bool(b) => Some(*b),
        _ => None,
    }
}

/// Hamilton product `a * b` of quaternions (x, y, z, w).
pub fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// `v` turned by the unit quaternion `q` (x, y, z, w).
pub fn quat_turn(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let [x, y, z, w] = q;
    let c = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let u = [x, y, z];
    let t = c(u, v).map(|k| 2.0 * k);
    let t2 = c(u, t);
    [0, 1, 2].map(|k| v[k] + w * t[k] + t2[k])
}

/// The quaternion of a turn by `angle` radians about the unit `axis`.
pub fn axis_angle_quat(axis: [f32; 3], angle: f32) -> [f32; 4] {
    let (s, c) = (angle * 0.5).sin_cos();
    [axis[0] * s, axis[1] * s, axis[2] * s, c]
}

/// The rotating doors and the rotating switches of a parsed world (the ones linked to a model). A door that players
/// do not open (DSV-500) is a message door: closed until a script's `on`.
// SPEC: FMT_DAT85.md §6.1 (position, rotation properties), §6.4 (model link by name)
// DC-DEFAULT: DSV-143 (proposed, w1-doors) the turn: the object's rotation angles (x, y, z in degrees, about the
// object's own axes: world turn = object rotation * angles * object rotation inverse, Euler order of
// worldobj::euler_quat) about the rotation point (the named object's position, else the door object's position)
pub fn turn_infos(w: &dc_world::World<'_>) -> Vec<TurnInfo> {
    let rules = rule_entries(w);
    let mut out = Vec::new();
    for o in &w.objects {
        let class = (dc_world::hash16(o.class_name), o.class_name.len());
        let kind = if class == CLASS_ROTATING_DOOR {
            TurnKind::Door
        } else if class == CLASS_ROTATING_SWITCH {
            TurnKind::Switch
        } else {
            continue;
        };
        let Some(m) = w.brush_of(o) else {
            continue;
        };
        // DC-DEFAULT: DSV-560 (proposed) a player door the file locks is a use door that starts locked (it was a
        // message door before, and left out of a co-op room); DSV-561 (proposed) a player-activated switch is a use
        // switch
        let player_opens = prop_bool(o, PROP_PLAYER_ACTIVATE).unwrap_or(false);
        let locked = prop_bool(o, PROP_LOCKED).unwrap_or(false);
        let deg = match prop(o, PROP_ROTATION_ANGLES) {
            Some(dc_world::FieldValue::Vec3(v)) if v.iter().all(|c| c.is_finite()) => *v,
            _ => [0.0; 3],
        };
        let own = crate::worldobj::euler_quat(o.rotation().unwrap_or([0.0; 3]));
        let local = crate::worldobj::euler_quat(deg.map(f32::to_radians));
        let inv = [-own[0], -own[1], -own[2], own[3]];
        let q = quat_mul(quat_mul(own, local), inv);
        let s = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt();
        let (axis, angle) = if s > 1e-6 {
            ([q[0] / s, q[1] / s, q[2] / s], 2.0 * s.atan2(q[3]))
        } else {
            ([0.0, 1.0, 0.0], 0.0)
        };
        let pivot = match prop(o, PROP_ROTATION_POINT) {
            Some(dc_world::FieldValue::Text(t)) if !t.is_empty() => w
                .object_named(t)
                .and_then(|p| p.position())
                .or(o.position()),
            _ => o.position(),
        }
        .unwrap_or(m.origin);
        let (open_key, close_key) = match kind {
            TurnKind::Door => (PROP_OPENING_TIME, PROP_CLOSING_TIME),
            TurnKind::Switch => (PROP_POWER_ON_TIME, PROP_POWER_OFF_TIME),
        };
        let open_ms = secs_ms(real(o, open_key));
        let close_ms = match real(o, close_key) {
            Some(s) if s > 0.0 => secs_ms(Some(s)),
            _ => open_ms,
        };
        let (cmds, sounds) = match kind {
            TurnKind::Door => (door_cmds(o), door_sounds(o)),
            TurnKind::Switch => (
                switch_cmds(o),
                door_sounds_of(o, PROP_ON_SOUND, PROP_OFF_SOUND),
            ),
        };
        // DC-DEFAULT: DSV-500 (proposed) a rotating door the players do not open (not player-activated, or without
        // a turn) is created closed and turns only by messages: a wall in the world until a script opens it
        let open_by = match kind {
            TurnKind::Door if !player_opens || angle.abs() < 1e-4 => OpenBy::Message,
            TurnKind::Door => open_by(o.name().unwrap_or_default(), &rules),
            TurnKind::Switch if player_opens => OpenBy::Use,
            TurnKind::Switch => OpenBy::Message,
        };
        out.push(TurnInfo {
            open_by,
            cmds,
            locked,
            name: o.name().map(<[u8]>::to_vec).unwrap_or_default(),
            kind,
            origin: m.origin,
            lo: m.box_min,
            hi: m.box_max,
            pivot,
            axis,
            angle,
            open_ms,
            close_ms,
            away: prop_bool(o, PROP_ROTATE_AWAY).unwrap_or(false),
            start_open: prop_bool(
                o,
                match kind {
                    TurnKind::Door => PROP_START_OPEN,
                    TurnKind::Switch => PROP_START_ON,
                },
            )
            .unwrap_or(false),
            sounds,
            link: text(o, PROP_DOOR_LINK),
        });
    }
    out
}

/// Change record of a turning door: `cf` = position and rotation, the full-precision position, velocity 0 (the
/// position moves on an arc; the clients place it at each record) and the rotation.
// SPEC: UDP_SESSION_2009_B.md §4.2 items 6, 7 (full precision: raw position and velocity, then `quat128`)
// DC-DEFAULT: DSV-144 (proposed, w1-doors) one record per tick while it turns, velocity 0
pub fn turn_record(obj: u16, pos: [f32; 3], rot: [f32; 4]) -> BitWriter {
    let mut w = BitWriter::new();
    write_record_head(&mut w, CF_POS | crate::objects::CF_ROT, obj);
    for c in pos {
        w.write_f32(c);
    }
    for _ in 0..3 {
        w.write_f32(0.0);
    }
    dc_wire::udp::write_quat128(&mut w, rot);
    w
}

/// One rotating door or switch on the clients.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnDoor {
    pub obj: u16,
    pub info: TurnInfo,
    /// How far it is open (0 = closed, 1 = open) and which way (1: the authored angle, -1: the other way).
    pub frac: f32,
    pub sign: f32,
    pub opening: bool,
    pub last_near: Option<dc_net_engine::Millis>,
    /// Use / message doors: the state asked for, the lock, command texts to run (as [`Door`]).
    pub want: bool,
    pub locked: bool,
    pub fired: Vec<Vec<u8>>,
    /// As [`Door`]: the way of the move in progress and the sounds of the moves started (DSV-302).
    pub moving: Option<bool>,
    pub sounds_due: Vec<String>,
    /// When the use key last ran its locked command or turned the switch (DSV-560 / DSV-561 cooldown).
    pub last_use: Option<dc_net_engine::Millis>,
}

impl TurnDoor {
    pub fn new(obj: u16, info: TurnInfo) -> TurnDoor {
        let open = info.start_open;
        // DSV-560: the file's lock is the door's lock at the start
        let locked = info.locked;
        // DC-DEFAULT: DSV-561 (proposed) a switch that starts on (its start-on property, or switched on by the start
        // scripts) runs its on command once at the start, as a switch turned on by a message does
        let fired = if open && info.kind == TurnKind::Switch && info.open_by != OpenBy::Near {
            vec![info.cmds.open.clone()]
        } else {
            Vec::new()
        };
        TurnDoor {
            obj,
            info,
            frac: if open { 1.0 } else { 0.0 },
            sign: 1.0,
            opening: open,
            last_near: None,
            want: open,
            locked,
            fired,
            moving: None,
            sounds_due: Vec::new(),
            last_use: None,
        }
    }

    /// Asks a use / message door or switch to open (turning away from `from` when it has the turn-away switch) or
    /// close.
    pub fn set_open(&mut self, open: bool, from: Option<[f32; 3]>) {
        if self.info.open_by == OpenBy::Near || self.want == open {
            return;
        }
        if open && self.frac <= 0.0 {
            self.sign = from.map_or(1.0, |p| self.away_sign(p));
        }
        self.want = open;
        let c = if open {
            &self.info.cmds.opening
        } else {
            &self.info.cmds.closing
        };
        self.fired.push(c.clone());
    }

    /// The turn from the closed state now.
    pub fn rot(&self) -> [f32; 4] {
        let i = &self.info;
        axis_angle_quat(i.axis, i.angle * self.sign * self.frac)
    }

    /// The object position now: the model origin turned about the pivot.
    pub fn pos(&self) -> [f32; 3] {
        let i = &self.info;
        let d = [0, 1, 2].map(|k| i.origin[k] - i.pivot[k]);
        let t = quat_turn(self.rot(), d);
        [0, 1, 2].map(|k| i.pivot[k] + t[k])
    }

    /// The change record that puts it where it is now (for a client that joined after it moved).
    pub fn record(&self) -> BitWriter {
        turn_record(self.obj, self.pos(), self.rot())
    }

    /// The activate entry `(disabled, state)` now (DSV-340): disabled for a door only messages turn (DSV-500) or a
    /// locked one; a switch players use has one as a door (DSV-561), another switch none (`None`).
    pub fn activate(&self) -> Option<(bool, u8)> {
        // DSV-500: a door that only messages turn has its entry disabled (no use prompt)
        (self.info.kind == TurnKind::Door || self.info.open_by == OpenBy::Use).then(|| {
            activate_of(
                self.info.open_by != OpenBy::Message,
                self.locked,
                self.opening,
            )
        })
    }

    /// Radius around the pivot that the turning model sweeps.
    fn reach(&self) -> f32 {
        let i = &self.info;
        let mut r2: f32 = 0.0;
        for k in 0..8 {
            let c = [
                if k & 1 == 0 { i.lo[0] } else { i.hi[0] },
                if k & 2 == 0 { i.lo[1] } else { i.hi[1] },
                if k & 4 == 0 { i.lo[2] } else { i.hi[2] },
            ];
            let d: f32 = (0..3).map(|a| (c[a] - i.pivot[a]).powi(2)).sum();
            r2 = r2.max(d);
        }
        r2.sqrt()
    }

    /// One step of `dt_ms`: a living player within [`DOOR_OPEN_RADIUS`] of the swept sphere opens it (a closed door
    /// with the turn-away switch picks the way that moves its middle away from the nearest such player), it turns
    /// back [`DOOR_CLOSE_AFTER_MS`] after the last one left. Returns the change record when it turned. A use or
    /// message door or switch turns to the state asked for; a start-open door near players never turns.
    // DC-DEFAULT: DSV-143 (proposed, w1-doors) the sliding doors' open rule on the swept sphere, turn-away by the middle
    pub fn step(
        &mut self,
        now: dc_net_engine::Millis,
        dt_ms: u32,
        players: &[[f32; 3]],
    ) -> Option<BitWriter> {
        if self.info.kind == TurnKind::Switch && self.info.open_by == OpenBy::Near {
            return None;
        }
        if self.info.open_by != OpenBy::Near {
            return self.move_to(self.want, dt_ms);
        }
        if self.info.start_open {
            return None;
        }
        let reach = self.reach() + DOOR_OPEN_RADIUS;
        let pivot = self.info.pivot;
        let dist = |p: &[f32; 3]| {
            (0..3)
                .map(|k| (p[k] - pivot[k]).powi(2))
                .sum::<f32>()
                .sqrt()
        };
        let nearest = players
            .iter()
            .filter(|p| dist(p) <= reach)
            .min_by(|a, b| dist(a).total_cmp(&dist(b)))
            .copied();
        if let Some(p) = nearest {
            self.last_near = Some(now);
            if !self.opening && self.frac <= 0.0 {
                self.sign = self.away_sign(p);
            }
            self.opening = true;
        } else if self
            .last_near
            .is_none_or(|t| dc_net_engine::time::since(now, t) >= DOOR_CLOSE_AFTER_MS)
        {
            self.opening = false;
        }
        self.move_to(self.opening, dt_ms)
    }

    fn move_to(&mut self, open: bool, dt_ms: u32) -> Option<BitWriter> {
        self.opening = open;
        let (target, ms) = if open {
            (1.0, self.info.open_ms)
        } else {
            (0.0, self.info.close_ms)
        };
        if (self.frac - target).abs() < 1e-6 {
            self.moving = None;
            return None;
        }
        // DSV-302: a turn that starts plays its sound
        if self.moving != Some(open) {
            self.moving = Some(open);
            if let Some(f) = self.info.sounds.of_move(open) {
                self.sounds_due.push(f.to_string());
            }
        }
        let stepf = dt_ms as f32 / ms.max(1) as f32;
        self.frac = if target > self.frac {
            (self.frac + stepf).min(1.0)
        } else {
            (self.frac - stepf).max(0.0)
        };
        if (self.frac - target).abs() < 1e-6 {
            self.moving = None;
        }
        if self.info.open_by != OpenBy::Near && (self.frac - target).abs() < 1e-6 {
            let c = if open {
                &self.info.cmds.open
            } else {
                &self.info.cmds.closed
            };
            self.fired.push(c.clone());
        }
        Some(self.record())
    }

    /// Distance from a point to the model's box (closed).
    fn box_distance(&self, p: [f32; 3]) -> f32 {
        let i = &self.info;
        (0..3)
            .map(|k| (i.lo[k] - p[k]).max(p[k] - i.hi[k]).max(0.0).powi(2))
            .sum::<f32>()
            .sqrt()
    }

    /// 1 when the authored turn moves the model's middle away from `p` (or without the turn-away switch), else -1.
    fn away_sign(&self, p: [f32; 3]) -> f32 {
        let i = &self.info;
        if !i.away {
            return 1.0;
        }
        let mid = [0, 1, 2].map(|k| 0.5 * (i.lo[k] + i.hi[k]));
        let d = [0, 1, 2].map(|k| mid[k] - i.pivot[k]);
        let t = quat_turn(axis_angle_quat(i.axis, i.angle * 0.5), d);
        let moved = [0, 1, 2].map(|k| i.pivot[k] + t[k] - mid[k]);
        let toward: f32 = (0..3).map(|k| moved[k] * (p[k] - mid[k])).sum();
        if toward > 0.0 {
            -1.0
        } else {
            1.0
        }
    }
}

// ---- DSV-270: the use key and the doors' commands ----

/// The command words the doors' texts use (hash and length of the lower-case word).
// OBSERVED: the ROM world files' door command texts (hashes only)
const WORD_MSG: PropHash = ([0xe4, 0x6b, 0x32, 0x01, 0x65, 0xee, 0xc9, 0x1e], 3);
const WORD_ON: PropHash = ([0xb8, 0xd3, 0x1e, 0x85, 0x27, 0x25, 0xaf, 0xb1], 2);
const WORD_OFF: PropHash = ([0xb4, 0xdc, 0x66, 0xdd, 0xe8, 0x06, 0x26, 0x1b], 3);
const WORD_LOCK: PropHash = ([0x0c, 0x03, 0x05, 0x86, 0x94, 0x5f, 0xe5, 0x04], 4);
const WORD_UNLOCK: PropHash = ([0x78, 0x76, 0x00, 0xeb, 0xe6, 0xd6, 0xc7, 0x5b], 6);
const WORD_DELAY: PropHash = ([0x6e, 0x16, 0xdb, 0x10, 0x22, 0x67, 0xe6, 0xe7], 5);

fn word_key(w: &[u8]) -> PropHash {
    let l = w.to_ascii_lowercase();
    (dc_world::hash16(&l), l.len())
}

/// The statement words of a trigger message and of a model's visible / solid switch.
// OBSERVED: the ROM world files' command texts (hashes only)
pub(crate) const WORD_TRIGGER: PropHash = ([0x68, 0x32, 0x59, 0xfe, 0xab, 0xbf, 0x5e, 0xb3], 7);
const WORD_VISIBLE: PropHash = ([0xd4, 0x2e, 0xf1, 0x49, 0x79, 0x00, 0xbc, 0x6e], 7);
const WORD_SOLID: PropHash = ([0x4b, 0x3d, 0x4a, 0x0d, 0xd7, 0x32, 0xf7, 0x91], 5);

/// A door statement: `msg <target> <word> [<value>]`, after `delay_ms`; `value` is the 0 / 1 after the word
/// (`visible 0`).
#[derive(Debug, Clone, PartialEq)]
pub struct DoorStatement {
    pub delay_ms: u32,
    pub target: Vec<u8>,
    pub word: PropHash,
    pub value: Option<bool>,
}

/// The `msg` statements of a command text, each with its delay (`delay <secs> (<msg statement>)`); other statements
/// are skipped.
pub fn parse_statements(text: &[u8]) -> Vec<DoorStatement> {
    let mut out = Vec::new();
    for st in text.split(|&b| b == b';' || b == b'\n' || b == b'\r') {
        let words: Vec<&[u8]> = st
            .split(|b| b.is_ascii_whitespace() || *b == b'(' || *b == b')')
            .filter(|w| !w.is_empty())
            .collect();
        let (delay_ms, rest) = match words.first() {
            Some(w) if word_key(w) == WORD_DELAY => {
                let secs = words
                    .get(1)
                    .and_then(|s| std::str::from_utf8(s).ok())
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|s| s.is_finite() && *s >= 0.0)
                    .unwrap_or(0.0);
                (
                    (secs * 1000.0).min(600_000.0) as u32,
                    &words[2.min(words.len())..],
                )
            }
            _ => (0, &words[..]),
        };
        if rest.len() >= 3 && word_key(rest[0]) == WORD_MSG {
            out.push(DoorStatement {
                delay_ms,
                target: rest[1].to_vec(),
                word: word_key(rest[2]),
                value: rest.get(3).map(|v| script_flag(v)),
            });
        }
    }
    out
}

/// The 0 / 1 value of a `visible` / `solid` statement: a number counts by its value (`0`, `0.0` are false), the words
/// `true` / `false` by their meaning (any case), anything else is true when it has a byte other than `0`.
// DC-DEFAULT: DSV-562 (proposed) the ROM worlds write only `0` and `1`; the start scripts read the value the same way
// (DSV-37); the shared code of the start scripts (`dc_world::scripts`)
pub use dc_world::scripts::script_flag;

/// The switch a statement asks for: `Some(true)` for `on`, `Some(false)` for `off`, `None` for any other word.
pub fn switch_word(st: &DoorStatement) -> Option<bool> {
    if st.word == WORD_ON {
        Some(true)
    } else if st.word == WORD_OFF {
        Some(false)
    } else {
        None
    }
}

/// A named model's visible / solid switch that a statement asked for (`msg <model> (visible 0|1)`, `(solid 0|1)`):
/// the session changes the model on the clients (DSV-562).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSwitch {
    pub name: Vec<u8>,
    pub visible: Option<bool>,
    pub solid: Option<bool>,
}

/// A trigger object of the world: its numbered commands (in order), how often it fires, and its box (DSV-562).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScriptTrigger {
    pub name: Vec<u8>,
    pub cmds: Vec<Vec<u8>>,
    /// How many times it fires in all (0: no limit).
    pub max: u32,
    /// The file marks it player-triggerable: a living player inside its box fires it (co-op rooms, DSV-563).
    pub touch: bool,
    pub pos: [f32; 3],
    /// Half extents of the box.
    pub dims: [f32; 3],
    /// The file locks it (a locked trigger does not fire until a statement unlocks it).
    pub locked: bool,
}

/// Property keys of a trigger: player-triggerable, the number of activations.
// OBSERVED: the ROM world files' trigger objects (hashes only; SANDHOG_CP1: the player-triggerable boxes in front
// of its locked doors fire once and unlock them; the triggers fired only by messages lie below the level)
pub const PROP_PLAYER_TRIGGERABLE: PropHash =
    ([0xc5, 0xef, 0x96, 0x7e, 0xe4, 0xd9, 0xae, 0xbb], 17);
pub const PROP_ACTIVATIONS: PropHash = ([0xfe, 0x28, 0xb2, 0x9f, 0x43, 0x83, 0x0a, 0xfd], 19);

/// The trigger objects of a parsed world (empty command texts left out, as in the start scripts).
// DC-DEFAULT: DSV-562 (proposed)
pub fn script_triggers(w: &dc_world::World<'_>) -> Vec<ScriptTrigger> {
    w.objects
        .iter()
        .filter(|o| o.role == Some(dc_world::ObjectRole::TriggerVolume))
        .map(|o| ScriptTrigger {
            name: o.name().map(<[u8]>::to_vec).unwrap_or_default(),
            cmds: dc_world::names::PROP_TRIGGER_COMMANDS
                .iter()
                .filter_map(|k| crate::world::named_text(o, k))
                .filter(|t| !t.is_empty())
                .map(<[u8]>::to_vec)
                .collect(),
            max: match prop(o, PROP_ACTIVATIONS) {
                Some(dc_world::FieldValue::Int(n)) if *n > 0 => *n as u32,
                _ => 0,
            },
            touch: prop_bool(o, PROP_PLAYER_TRIGGERABLE).unwrap_or(false),
            pos: o.position().unwrap_or([0.0; 3]),
            dims: o
                .vec3(dc_world::PropKey::BoxExtent)
                .filter(|d| d.iter().all(|c| c.is_finite()))
                .map_or([0.0; 3], |d| d.map(f32::abs)),
            locked: prop_bool(o, PROP_LOCKED).unwrap_or(false),
        })
        .collect()
}

/// The world's triggers in a room: how often each fired, its lock, whether a player stood in its box last tick.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Triggers {
    pub list: Vec<ScriptTrigger>,
    pub fires: Vec<u32>,
    pub locked: Vec<bool>,
    pub inside: Vec<bool>,
    /// How many runs stopped at the statement budget ([`TRIGGER_STATEMENTS`]).
    pub over_budget: u32,
    /// The `lock` / `unlock` statements of the runs (target, lock), for the objects outside doors and triggers: the
    /// session hands them to the player spawn points (DSV-569) and empties the list.
    pub locks: Vec<(Vec<u8>, bool)>,
    /// The variables, counters, timers and activation boxes of the world's statements and what the statements ask
    /// of the game (DSV-570..DSV-574).
    pub net: crate::script::Network,
}

/// How far around its box a player's position still touches a trigger (the body's half width).
// DC-DEFAULT: DSV-563 (proposed) 16 units (as the station box, DSV-323)
pub const TRIGGER_TOUCH_MARGIN: f32 = 16.0;
/// Rounds of nested statements (a door's commands, a trigger's commands) followed in one run.
pub const TRIGGER_DEPTH: usize = 32;
/// Most statements one run takes (texts that fire themselves twice would double every round): past it the run stops
/// and drops what is pending.
// DC-DEFAULT: DSV-562 (proposed) 1024 statements, as the start scripts' bound in spirit (worldobj SCRIPT_STATEMENTS)
pub const TRIGGER_STATEMENTS: usize = 1024;
/// Time between two locked-door uses (locked command, locked sound) or two switch turns by the use key of the same
/// door or switch.
// DC-DEFAULT: DSV-560 / DSV-561 (proposed) 500 ms
pub const USE_COOLDOWN_MS: u32 = 500;

impl Triggers {
    pub fn new(list: Vec<ScriptTrigger>) -> Triggers {
        let n = list.len();
        Triggers {
            locked: list.iter().map(|t| t.locked).collect(),
            fires: vec![0; n],
            inside: vec![false; n],
            over_budget: 0,
            locks: Vec::new(),
            net: crate::script::Network::default(),
            list,
        }
    }

    /// [`Triggers::new`] with the world's mission objects and the room's draw seed (DSV-570).
    pub fn with_network(list: Vec<ScriptTrigger>, net: crate::script::Network) -> Triggers {
        Triggers {
            net,
            ..Triggers::new(list)
        }
    }

    /// Counts the triggers the start scripts fired (`names`, one entry per fire) as fired, so that a start trigger
    /// with one activation does not fire again.
    // DC-DEFAULT: DSV-562 (proposed)
    pub fn seed(&mut self, names: &[Vec<u8>]) {
        for n in names {
            if let Some(i) = self
                .list
                .iter()
                .position(|t| t.name.eq_ignore_ascii_case(n))
            {
                self.fires[i] = self.fires[i].saturating_add(1);
            }
        }
    }

    /// The state of a new round: no trigger fired, the file's locks, nobody inside a box.
    // DC-DEFAULT: DSV-564 (proposed)
    pub fn reset(&mut self) {
        let mut net = std::mem::take(&mut self.net);
        net.reset();
        *self = Triggers::with_network(std::mem::take(&mut self.list), net);
    }

    /// Fires trigger `name` (names compare without case): its command texts, `None` when it is unknown, locked or
    /// has fired its number of times.
    pub fn fire(&mut self, name: &[u8]) -> Option<Vec<Vec<u8>>> {
        let i = self
            .list
            .iter()
            .position(|t| t.name.eq_ignore_ascii_case(name))?;
        self.fire_at(i)
    }

    /// [`Triggers::fire`] for the trigger at index `i` (a touched box fires itself, whatever its name).
    pub fn fire_at(&mut self, i: usize) -> Option<Vec<Vec<u8>>> {
        let t = self.list.get(i)?;
        if self.locked[i] || (t.max > 0 && self.fires[i] >= t.max) {
            return None;
        }
        self.fires[i] += 1;
        Some(t.cmds.clone())
    }

    fn set_lock(&mut self, name: &[u8], locked: bool) {
        for (i, t) in self.list.iter().enumerate() {
            if t.name.eq_ignore_ascii_case(name) {
                self.locked[i] = locked;
            }
        }
    }

    /// The command texts of the player-triggerable triggers a living player (at `players`) entered since the last
    /// call: a trigger fires when a player is in its box and none was the tick before.
    // DC-DEFAULT: DSV-563 (proposed) fired on the entry into the box (grown by TRIGGER_TOUCH_MARGIN)
    pub fn touch(&mut self, players: &[[f32; 3]]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for i in 0..self.list.len() {
            let t = &self.list[i];
            if !t.touch {
                continue;
            }
            let h = t.dims.map(|d| d + TRIGGER_TOUCH_MARGIN);
            let now_in = players
                .iter()
                .any(|p| (0..3).all(|k| (p[k] - t.pos[k]).abs() <= h[k]));
            let entered = now_in && !self.inside[i];
            self.inside[i] = now_in;
            if entered {
                if let Some(c) = self.fire_at(i) {
                    out.extend(c);
                }
            }
        }
        out
    }
}

/// Applies one `msg` statement to the door, rotating door or switch of that name (names compare without case): `on`
/// / `off` / `lock` / `unlock`; to a trigger: `trigger` fires it (its commands go to `texts`), `lock` / `unlock`; a
/// `visible` / `solid` switch goes to `models`.
fn apply(
    doors: &mut [Door],
    turns: &mut [TurnDoor],
    triggers: &mut Triggers,
    st: &DoorStatement,
    texts: &mut Vec<Vec<u8>>,
    models: &mut Vec<ModelSwitch>,
) {
    let w = st.word;
    if w == WORD_TRIGGER {
        if let Some(c) = triggers.fire(&st.target) {
            texts.extend(c);
        }
        return;
    }
    if w == WORD_VISIBLE || w == WORD_SOLID {
        if let Some(v) = st.value {
            let vis = w == WORD_VISIBLE;
            models.push(ModelSwitch {
                name: st.target.clone(),
                visible: vis.then_some(v),
                solid: (!vis).then_some(v),
            });
        }
        return;
    }
    if w == WORD_LOCK || w == WORD_UNLOCK {
        triggers.set_lock(&st.target, w == WORD_LOCK);
        // DC-DEFAULT: DSV-569 (proposed) the spawn points' locks; bounded like the statements of a run
        if triggers.locks.len() < TRIGGER_STATEMENTS {
            triggers.locks.push((st.target.clone(), w == WORD_LOCK));
        }
    }
    for d in doors
        .iter_mut()
        .filter(|d| d.info.name.eq_ignore_ascii_case(&st.target))
    {
        if w == WORD_ON {
            d.set_open(true);
        } else if w == WORD_OFF {
            d.set_open(false);
        } else if w == WORD_LOCK {
            d.locked = true;
        } else if w == WORD_UNLOCK {
            d.locked = false;
        }
    }
    for t in turns
        .iter_mut()
        .filter(|t| t.info.name.eq_ignore_ascii_case(&st.target))
    {
        if w == WORD_ON {
            t.set_open(true, None);
        } else if w == WORD_OFF {
            t.set_open(false, None);
        } else if w == WORD_LOCK {
            t.locked = true;
        } else if w == WORD_UNLOCK {
            t.locked = false;
        }
    }
}

/// A statement body waiting for its time (`delay`, `delayid`, `delayvariable`) and the id an `abort` names it by.
#[derive(Debug, Clone, PartialEq)]
pub struct Delayed {
    pub id: Option<Vec<u8>>,
    pub body: Vec<crate::script::Stmt>,
}

/// Statements waiting for their time.
pub type DelayedStatements = Vec<(dc_net_engine::Millis, Delayed)>;

/// Most statement bodies waiting at once in a room (more are dropped).
// DC-DEFAULT: DSV-570 (proposed) 1024 (was 256 for the doors' delays, DSV-270; a mission world keeps a few hundred
// respawn and caption delays waiting)
pub const DELAYED_MAX: usize = 1024;

/// One run of statements (DSV-562 / DSV-570): the objects they act on, what is pending, the budget.
struct Run<'r> {
    doors: &'r mut [Door],
    turns: &'r mut [TurnDoor],
    triggers: &'r mut Triggers,
    delayed: &'r mut DelayedStatements,
    now: dc_net_engine::Millis,
    pending: Vec<Vec<u8>>,
    models: Vec<ModelSwitch>,
    taken: usize,
    cut: bool,
}

impl Run<'_> {
    fn exec(&mut self, list: &[crate::script::Stmt]) {
        use crate::script::Stmt;
        for st in list {
            if self.cut {
                return;
            }
            self.taken += 1;
            if self.taken > TRIGGER_STATEMENTS {
                self.cut = true;
                return;
            }
            match st {
                Stmt::Msg { target, words } => self.msg(target, words),
                Stmt::Delay { wait, id, body } => {
                    let secs = self.triggers.net.value(wait);
                    let ms = if secs.is_finite() {
                        (secs.max(0.0) * 1000.0).min(600_000.0) as u32
                    } else {
                        0
                    };
                    if ms == 0 && id.is_none() {
                        self.exec(body);
                    } else if self.delayed.len() < DELAYED_MAX {
                        self.delayed.push((
                            self.now.wrapping_add(ms),
                            Delayed {
                                id: id.clone(),
                                body: body.clone(),
                            },
                        ));
                    }
                }
                Stmt::Abort(id) => self.delayed.retain(|d| d.1.id.as_ref() != Some(id)),
                Stmt::Var { op, name, value } => {
                    let v = self.triggers.net.value(value);
                    self.triggers.net.set_var(*op, name, v);
                }
                Stmt::If { cond, then } => {
                    if self.triggers.net.test(cond) {
                        self.exec(then);
                    }
                }
                Stmt::Rand(options) => {
                    let i = self.triggers.net.draw(options.len());
                    self.exec(&options[i]);
                }
                Stmt::Other => {
                    self.triggers.net.skipped = self.triggers.net.skipped.saturating_add(1)
                }
            }
        }
    }

    /// One `msg <target> <words>` statement: the mission words (DSV-571..DSV-574) or the doors', triggers' and models'
    /// words (DSV-270, DSV-562).
    fn msg(&mut self, target: &[u8], words: &[Vec<u8>]) {
        use crate::script::{self as sc, Cue, Effect, GoalOp};
        let Some(w) = words.first().map(|w| word_key(w)) else {
            self.triggers.net.skipped = self.triggers.net.skipped.saturating_add(1);
            return;
        };
        let arg = words.get(1).map(Vec::as_slice);
        let num = arg.and_then(|a| {
            std::str::from_utf8(a)
                .ok()
                .and_then(|t| t.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite())
        });
        let net = &mut self.triggers.net;
        if w == sc::WORD_SPAWN {
            net.push_effect(Effect::Spawn {
                spawner: target.to_vec(),
                name: arg.map(<[u8]>::to_vec),
            });
            return;
        }
        if w == sc::WORD_INC || w == sc::WORD_DEC {
            let by = num.unwrap_or(1.0) as i32;
            let by = if w == sc::WORD_DEC { -by } else { by };
            if !net.count(target, by) {
                net.skipped = net.skipped.saturating_add(1);
            }
            return;
        }
        if (w == sc::WORD_START || w == sc::WORD_STOP || w == sc::WORD_TIMEAMOUNT)
            && net.timer(target, w, num, self.now)
        {
            return;
        }
        let goal = if w == sc::WORD_ADD {
            Some(GoalOp::Add)
        } else if w == sc::WORD_CHECK {
            Some(GoalOp::Check)
        } else if w == sc::WORD_FAIL {
            Some(GoalOp::Fail)
        } else {
            None
        };
        if let (Some(op), Some(n)) = (goal, num) {
            if n >= 0.0 {
                net.push_effect(Effect::Goal(op, n as u32));
            }
            return;
        }
        if w == sc::WORD_INCLIFECOUNT {
            net.push_effect(Effect::Lives(num.unwrap_or(1.0) as i32));
            return;
        }
        if w == sc::WORD_MISSIONCOMPLETE {
            if num.unwrap_or(1.0) != 0.0 {
                net.push_effect(Effect::Complete);
            }
            return;
        }
        if w == sc::WORD_AIFREEZE {
            net.push_effect(Effect::Freeze(arg.is_none_or(script_flag)));
            return;
        }
        if w == sc::WORD_HEALINPART {
            net.push_effect(Effect::HealInPart);
            return;
        }
        if w == sc::WORD_PAUSE {
            // DC-DEFAULT: DSV-578 (proposed) `pause` to an effect object freezes a cut-scene effect on the clients;
            // this server sends no effect object state: taken, nothing sent
            net.client_only = net.client_only.saturating_add(1);
            return;
        }
        // DC-DEFAULT: DSV-579..DSV-581 (proposed) what the clients are shown: the session and the co-op rules
        // resolve the names (captions, banners, cameras, music)
        if w == sc::WORD_PLAY {
            match arg {
                Some(key) => net.push_cue(Cue::Play {
                    trigger: target.to_vec(),
                    key: key.to_vec(),
                }),
                None => net.skipped = net.skipped.saturating_add(1),
            }
            return;
        }
        if w == sc::WORD_MOVETO {
            match arg {
                Some(point) => net.push_cue(Cue::MoveTo {
                    camera: target.to_vec(),
                    point: point.to_vec(),
                }),
                None => net.skipped = net.skipped.saturating_add(1),
            }
            return;
        }
        if w == sc::WORD_FOV {
            let n = |i: usize| {
                words
                    .get(i)
                    .and_then(|v| std::str::from_utf8(v).ok())
                    .and_then(|v| v.trim().parse::<f32>().ok())
                    .filter(|v| v.is_finite() && *v > 0.0)
            };
            match n(1).zip(n(2)) {
                Some((x, y)) => net.push_cue(Cue::Fov {
                    camera: target.to_vec(),
                    fov: [x, y],
                }),
                None => net.skipped = net.skipped.saturating_add(1),
            }
            return;
        }
        if w == sc::WORD_HIDECHARACTER {
            net.push_cue(Cue::HideCharacters(arg.is_none_or(script_flag)));
            return;
        }
        // DC-DEFAULT: DSV-582 (proposed) `enable` / `disable` to an AI volume: server state for the AI navigation,
        // nothing is sent
        if w == sc::WORD_ENABLE || w == sc::WORD_DISABLE {
            let name = target.to_ascii_lowercase();
            if w == sc::WORD_DISABLE {
                net.ai_volumes_off.insert(name);
            } else {
                net.ai_volumes_off.remove(&name);
            }
            return;
        }
        if w == WORD_ON || w == WORD_OFF {
            net.push_cue(Cue::Switch {
                target: target.to_vec(),
                on: w == WORD_ON,
            });
        }
        if w == sc::WORD_COOPTIMER {
            net.push_effect(Effect::MissionTimer(
                arg.is_some_and(|a| word_key(a) == WORD_ON),
            ));
            return;
        }
        if w == sc::WORD_REMOVE || w == sc::WORD_HIDDEN {
            // DC-DEFAULT: DSV-571 (proposed) a removed (or hidden) model is switched off: not visible, not solid
            let off = w == sc::WORD_REMOVE || arg.is_none_or(script_flag);
            self.models.push(ModelSwitch {
                name: target.to_vec(),
                visible: Some(!off),
                solid: Some(!off),
            });
            return;
        }
        if w == WORD_LOCK || w == WORD_UNLOCK {
            net.set_activate_lock(target, w == WORD_LOCK);
        }
        if (w == WORD_ON || w == WORD_OFF) && net.switched.len() < sc::EFFECTS_MAX {
            net.switched.push((target.to_vec(), w == WORD_ON));
        }
        let st = DoorStatement {
            delay_ms: 0,
            target: target.to_vec(),
            word: w,
            value: arg.map(script_flag),
        };
        apply(
            self.doors,
            self.turns,
            self.triggers,
            &st,
            &mut self.pending,
            &mut self.models,
        );
    }
}

/// Runs the doors' fired command texts and the delayed statements that are due at `now`; a door that changes state
/// fires its own commands in turn (bounded).
// DC-DEFAULT: DSV-270 (proposed) the doors' opening / open / closing / closed commands: `msg <door> on` opens,
// `off` closes, `lock` / `unlock` the use key; `delay <secs> (...)` later; messages to other objects are not run here
pub fn run_commands(
    doors: &mut [Door],
    turns: &mut [TurnDoor],
    delayed: &mut DelayedStatements,
    now: dc_net_engine::Millis,
) {
    run_world(
        doors,
        turns,
        &mut Triggers::default(),
        delayed,
        Vec::new(),
        now,
    );
}

/// [`run_commands`] with the world's triggers: `texts` (fired from elsewhere: the touched triggers) run first; a
/// `trigger` message runs the trigger's commands as statements of the same kind (nested up to [`TRIGGER_DEPTH`]
/// rounds); the models' visible / solid switches are returned for the session.
// DC-DEFAULT: DSV-562 (proposed) a trigger message runs every numbered command of the trigger in order (as the start
// scripts do, DSV-37), at most its number of activations, not while it is locked; `visible 0|1` / `solid 0|1` change
// the named model
pub fn run_world(
    doors: &mut [Door],
    turns: &mut [TurnDoor],
    triggers: &mut Triggers,
    delayed: &mut DelayedStatements,
    texts: Vec<Vec<u8>>,
    now: dc_net_engine::Millis,
) -> Vec<ModelSwitch> {
    // DC-DEFAULT: DSV-572 (proposed) the timers that ran out queue their over-time commands
    triggers.net.timers_due(now);
    let mut due = Vec::new();
    delayed.retain(|(t, d)| {
        if dc_net_engine::time::since(now, *t) < u32::MAX / 2 {
            due.push(d.body.clone());
            false
        } else {
            true
        }
    });
    let mut pending = texts;
    pending.append(&mut triggers.net.queued);
    let mut run = Run {
        doors,
        turns,
        triggers,
        delayed,
        now,
        pending,
        models: Vec::new(),
        taken: 0,
        cut: false,
    };
    for body in &due {
        run.exec(body);
    }
    for _ in 0..TRIGGER_DEPTH {
        let mut fired: Vec<Vec<u8>> = std::mem::take(&mut run.pending)
            .into_iter()
            .chain(
                run.doors
                    .iter_mut()
                    .flat_map(|d| std::mem::take(&mut d.fired)),
            )
            .chain(
                run.turns
                    .iter_mut()
                    .flat_map(|t| std::mem::take(&mut t.fired)),
            )
            .collect();
        fired.append(&mut run.triggers.net.queued);
        if fired.is_empty() || run.cut {
            break;
        }
        for text in fired {
            // DC-DEFAULT: DSV-570 (proposed) every statement form of the world's texts, one parser for all of them
            let list = crate::script::parse_script(&text);
            run.exec(&list);
            if run.cut {
                break;
            }
        }
    }
    if run.cut {
        // DSV-562: the budget is spent: drop what is pending (a self-feeding trigger ends here)
        run.pending.clear();
        run.triggers.net.queued.clear();
        run.triggers.over_budget = run.triggers.over_budget.saturating_add(1);
    }
    run.models
}

/// The use key of a living player at `at`: the nearest use door, rotating door or switch within [`DOOR_USE_REACH`]
/// of its box turns; when that one is locked it does not turn, its locked command runs and its locked sound plays
/// (DSV-560). True when one turned.
// DC-DEFAULT: DSV-270 (proposed); DSV-560 (proposed) a locked door in reach takes the key (the nearest wins)
pub fn use_key(
    doors: &mut [Door],
    turns: &mut [TurnDoor],
    at: [f32; 3],
    now: dc_net_engine::Millis,
) -> bool {
    let cooled = |last: Option<dc_net_engine::Millis>| {
        last.is_none_or(|t| dc_net_engine::time::since(now, t) >= USE_COOLDOWN_MS)
    };
    let near_door = doors
        .iter()
        .enumerate()
        .filter(|(_, d)| d.info.open_by == OpenBy::Use)
        .map(|(i, d)| (i, d.distance(at)))
        .filter(|(_, r)| *r <= DOOR_USE_REACH)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    let near_turn = turns
        .iter()
        .enumerate()
        .filter(|(_, t)| t.info.open_by == OpenBy::Use)
        .map(|(i, t)| (i, t.box_distance(at)))
        .filter(|(_, r)| *r <= DOOR_USE_REACH)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    let (name, link, open) = match (near_door, near_turn) {
        (Some((i, a)), t) if t.is_none_or(|t| a <= t.1) => {
            let d = &mut doors[i];
            if d.locked {
                // DSV-560: at most once per cooldown; one sound in the tick's queue
                if cooled(d.last_use) {
                    d.last_use = Some(now);
                    d.fired.push(d.info.cmds.locked.clone());
                    let snd = &d.info.sounds.locked;
                    if !snd.is_empty() && !d.sounds_due.contains(snd) {
                        d.sounds_due.push(snd.clone());
                    }
                }
                return false;
            }
            let open = !d.want;
            d.set_open(open);
            (d.info.name.clone(), d.info.link.clone(), open)
        }
        (_, Some((i, _))) => {
            let t = &mut turns[i];
            if t.locked {
                if cooled(t.last_use) {
                    t.last_use = Some(now);
                    t.fired.push(t.info.cmds.locked.clone());
                    let snd = &t.info.sounds.locked;
                    if !snd.is_empty() && !t.sounds_due.contains(snd) {
                        t.sounds_due.push(snd.clone());
                    }
                }
                return false;
            }
            // DSV-561: a switch turns at most once per cooldown
            if t.info.kind == TurnKind::Switch {
                if !cooled(t.last_use) {
                    return false;
                }
                t.last_use = Some(now);
            }
            let open = !t.want;
            t.set_open(open, Some(at));
            (t.info.name.clone(), t.info.link.clone(), open)
        }
        _ => return false,
    };
    // DC-DEFAULT: DSV-460 (proposed) the linked door (named by the used one, or naming it) moves with it
    let linked = |n: &[u8], l: &[u8]| {
        !n.eq_ignore_ascii_case(&name)
            && ((!link.is_empty() && n.eq_ignore_ascii_case(&link))
                || (!l.is_empty() && l.eq_ignore_ascii_case(&name)))
    };
    for d in doors.iter_mut() {
        if linked(&d.info.name, &d.info.link) && !d.locked {
            d.set_open(open);
        }
    }
    for t in turns.iter_mut() {
        if linked(&t.info.name, &t.info.link) && !t.locked && t.info.kind == TurnKind::Door {
            t.set_open(open, Some(at));
        }
    }
    true
}

#[cfg(test)]
mod use_tests {
    use super::*;

    /// Two levers (each turning the other) and two shutters (message only), wired like a garage on a ROM world.
    fn garage() -> (Vec<Door>, Vec<TurnDoor>) {
        let lever_cmds = |me: &str, other: &str| {
            DoorCmds {
            opening: format!(
                "msg {other} on;\r\nmsg {me} lock;\r\nmsg {other} lock;\r\ndelay 7 (msg {me} unlock);\r\ndelay 7 (msg {other} unlock);"
            )
            .into_bytes(),
            open: b"msg ShutterA on;\r\nmsg ShutterB on;".to_vec(),
            closing: format!("msg {other} off;").into_bytes(),
            closed: b"msg ShutterA off;\r\nmsg ShutterB off;".to_vec(),
            locked: Vec::new(),
        }
        };
        let lever = |name: &str, other: &str, z: f32| TurnInfo {
            link: Vec::new(),
            locked: false,
            name: name.as_bytes().to_vec(),
            kind: TurnKind::Door,
            origin: [0.0, 50.0, z],
            lo: [-2.0, 40.0, z - 2.0],
            hi: [2.0, 60.0, z + 2.0],
            pivot: [0.0, 50.0, z],
            axis: [1.0, 0.0, 0.0],
            angle: -std::f32::consts::FRAC_PI_2,
            open_ms: 1000,
            close_ms: 1000,
            away: false,
            start_open: false,
            open_by: OpenBy::Use,
            cmds: lever_cmds(name, other),
            sounds: DoorSounds::default(),
        };
        let shutter = |name: &str, x: f32| DoorInfo {
            link: Vec::new(),
            locked: false,
            name: name.as_bytes().to_vec(),
            closed: [x, 50.0, 0.0],
            lo: [x - 60.0, 0.0, -1.0],
            hi: [x + 60.0, 100.0, 1.0],
            dir: [0.0, 1.0, 0.0],
            dist: 100.0,
            open_ms: 2000,
            close_ms: 2000,
            open_by: OpenBy::Message,
            cmds: DoorCmds::default(),
            sounds: DoorSounds::default(),
            usable: false,
        };
        (
            vec![
                Door::new(1, shutter("ShutterA", 300.0)),
                Door::new(2, shutter("ShutterB", 500.0)),
            ],
            vec![
                TurnDoor::new(3, lever("Lever", "Lever_O", 0.0)),
                TurnDoor::new(4, lever("Lever_O", "Lever", 10.0)),
            ],
        )
    }

    fn tick(
        doors: &mut [Door],
        turns: &mut [TurnDoor],
        dl: &mut DelayedStatements,
        t: u32,
        near: &[[f32; 3]],
    ) {
        for d in doors.iter_mut() {
            d.step(t, 100, near);
        }
        for x in turns.iter_mut() {
            x.step(t, 100, near);
        }
        run_commands(doors, turns, dl, t);
    }

    #[test]
    fn rules_from_the_file_properties() {
        let e = |n: &str, pa: bool, ons: &[&str]| -> DoorRuleEntry {
            (
                n.as_bytes().to_vec(),
                pa,
                ons.iter().map(|t| t.as_bytes().to_vec()).collect(),
            )
        };
        let all = vec![
            e("Shutter", false, &[]),
            e("Lever", true, &["shutter", "Lever_O"]),
            e("Lever_O", true, &["Shutter", "Lever"]),
            e("Self", true, &["Self"]),
            e("Triggered", false, &[]),
            e("Pair", true, &["Partner"]),
            e("Partner", true, &[]),
        ];
        assert_eq!(open_by(b"Shutter", &all), OpenBy::Message);
        assert_eq!(open_by(b"lever", &all), OpenBy::Use);
        assert_eq!(open_by(b"Lever_O", &all), OpenBy::Use);
        // DSV-460: every player-activated door opens by the use key, never by a player near it
        assert_eq!(open_by(b"Self", &all), OpenBy::Use, "its own close timer");
        assert_eq!(
            open_by(b"Triggered", &all),
            OpenBy::Near,
            "no door switches it on"
        );
        assert_eq!(open_by(b"Pair", &all), OpenBy::Use, "a player door");
        assert_eq!(open_by(b"Partner", &all), OpenBy::Use, "a player door");
        assert_eq!(open_by(b"Unknown", &all), OpenBy::Near);
        let s = parse_statements(b"msg A on;\r\ndelay 7 (msg B unlock);\r\nnothing here");
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].delay_ms, s[0].word), (0, WORD_ON));
        assert_eq!(
            (s[1].delay_ms, &s[1].target[..], s[1].word),
            (7000, &b"B"[..], WORD_UNLOCK)
        );
    }

    /// DSV-270: a player standing at the shutters does not open them; the use key at the lever does (both levers
    /// turn, the shutters open once the lever is open); the levers are locked for 7 s; the use key again closes the
    /// lever and the shutters follow.
    /// DSV-460: a player-activated double door (each wing names the other as its link) stays shut while a player
    /// stands at it; the use key opens both wings, its open command closes them 5 s later, and the use key on an open
    /// door closes both.
    #[test]
    fn a_player_door_opens_only_by_the_use_key_with_its_linked_wing() {
        let wing = |name: &str, link: &str, x: f32| DoorInfo {
            name: name.as_bytes().to_vec(),
            closed: [x, 0.0, 0.0],
            lo: [x - 24.0, -50.0, -4.0],
            hi: [x + 24.0, 50.0, 4.0],
            dir: [1.0, 0.0, 0.0],
            dist: 48.0,
            open_ms: 1000,
            close_ms: 1000,
            open_by: OpenBy::Use,
            cmds: DoorCmds {
                open: format!("delay 5 (msg {name} off)").into_bytes(),
                ..DoorCmds::default()
            },
            sounds: DoorSounds::default(),
            usable: true,
            link: link.as_bytes().to_vec(),
            locked: false,
        };
        let mut doors = vec![
            Door::new(10, wing("SlidingDoor0", "SlidingDoor1", 0.0)),
            Door::new(11, wing("SlidingDoor1", "SlidingDoor0", 48.0)),
        ];
        let mut turns: Vec<TurnDoor> = Vec::new();
        let mut delays = DelayedStatements::default();
        let player = [10.0, 0.0, 20.0];
        // a player at the door for 2 s: nothing moves
        for t in (0..2000).step_by(50) {
            for d in &mut doors {
                assert!(d.step(t, 50, &[player]).is_none(), "no proximity opening");
            }
        }
        assert!(doors.iter().all(|d| d.offset == 0.0));
        // the use key: both wings open
        assert!(use_key(&mut doors, &mut turns, player, 2000));
        run_commands(&mut doors, &mut turns, &mut delays, 2000);
        for t in (2050..3200).step_by(50) {
            for d in &mut doors {
                d.step(t, 50, &[player]);
            }
            run_commands(&mut doors, &mut turns, &mut delays, t);
        }
        assert!(
            doors.iter().all(|d| (d.offset - 48.0).abs() < 1e-3),
            "both open"
        );
        // the open command closes them 5 s after they were open (the player still there)
        for t in (3200..9500).step_by(50) {
            for d in &mut doors {
                d.step(t, 50, &[player]);
            }
            run_commands(&mut doors, &mut turns, &mut delays, t);
        }
        assert!(
            doors.iter().all(|d| d.offset == 0.0),
            "closed by the open command"
        );
        // a use key far away does nothing
        assert!(!use_key(&mut doors, &mut turns, [1000.0, 0.0, 0.0], 9500));
    }

    #[test]
    fn shutters_open_only_from_the_lever() {
        let (mut doors, mut turns) = garage();
        let mut dl = DelayedStatements::new();
        let at_shutter = [300.0, 50.0, 30.0];
        for k in 0..50 {
            tick(&mut doors, &mut turns, &mut dl, k * 100, &[at_shutter]);
        }
        assert!(
            doors.iter().all(|d| d.offset == 0.0),
            "walking up does not open them"
        );
        assert!(
            !use_key(&mut doors, &mut turns, at_shutter, 4900),
            "out of the levers' reach"
        );
        let at_lever = [0.0, 50.0, -60.0];
        assert!(use_key(&mut doors, &mut turns, at_lever, 5000));
        run_commands(&mut doors, &mut turns, &mut dl, 5000);
        assert!(
            turns.iter().all(|t| t.want && t.locked),
            "both levers turn, locked"
        );
        let mut t = 5100;
        while t < 9000 {
            tick(&mut doors, &mut turns, &mut dl, t, &[at_lever]);
            t += 100;
        }
        assert!(turns.iter().all(|x| x.frac == 1.0));
        assert!(
            doors.iter().all(|d| d.offset == 100.0),
            "the shutters opened"
        );
        assert!(
            !use_key(&mut doors, &mut turns, at_lever, t),
            "locked for 7 s"
        );
        while t < 12_200 {
            tick(&mut doors, &mut turns, &mut dl, t, &[at_lever]);
            t += 100;
        }
        assert!(turns.iter().all(|x| !x.locked));
        assert!(use_key(&mut doors, &mut turns, at_lever, t));
        while t < 16_000 {
            tick(&mut doors, &mut turns, &mut dl, t, &[at_lever]);
            t += 100;
        }
        assert!(turns.iter().all(|x| x.frac == 0.0));
        assert!(
            doors.iter().all(|d| d.offset == 0.0),
            "closed with the lever"
        );
    }
}

#[cfg(test)]
mod turn_tests {
    use super::*;

    /// A panel 60 wide along +z from a hinge at the origin, 4 thick, 100 high, turning 90 degrees about +y.
    fn info(away: bool) -> TurnInfo {
        TurnInfo {
            link: Vec::new(),
            locked: false,
            name: b"Turn".to_vec(),
            kind: TurnKind::Door,
            origin: [0.0, 50.0, 30.0],
            lo: [-2.0, 0.0, 0.0],
            hi: [2.0, 100.0, 60.0],
            pivot: [0.0, 50.0, 0.0],
            axis: [0.0, 1.0, 0.0],
            angle: std::f32::consts::FRAC_PI_2,
            open_ms: 1000,
            close_ms: 1000,
            away,
            start_open: false,
            open_by: OpenBy::Near,
            cmds: DoorCmds::default(),
            sounds: DoorSounds::default(),
        }
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() < 1e-3)
    }

    #[test]
    fn quaternion_helpers() {
        let q = axis_angle_quat([0.0, 1.0, 0.0], std::f32::consts::FRAC_PI_2);
        // +90 about y turns +z to +x
        assert!(close(quat_turn(q, [0.0, 0.0, 1.0]), [1.0, 0.0, 0.0]));
        let qq = quat_mul(q, q);
        assert!(close(quat_turn(qq, [0.0, 0.0, 1.0]), [0.0, 0.0, -1.0]));
        // the Euler helper's yaw is the same turn
        let e = crate::worldobj::euler_quat([0.0, std::f32::consts::FRAC_PI_2, 0.0]);
        assert!(close(quat_turn(e, [0.0, 0.0, 1.0]), [1.0, 0.0, 0.0]));
    }

    #[test]
    fn turns_open_near_a_player_and_back_after() {
        let mut d = TurnDoor::new(9, info(false));
        assert!(d.step(0, 100, &[[1000.0, 50.0, 0.0]]).is_none());
        let mut n = 0;
        for k in 1..=12 {
            if d.step(k * 100, 100, &[[-40.0, 50.0, 30.0]]).is_some() {
                n += 1;
            }
        }
        assert_eq!(n, 10);
        assert_eq!(d.frac, 1.0);
        // open: the middle of the panel went from 30 along +z to 30 along +x
        assert!(close(d.pos(), [30.0, 50.0, 0.0]), "{:?}", d.pos());
        let r = d.record();
        assert_eq!(r.bit_len(), 8 + 16 + 192 + 128);
        // nobody near: back after 3 s, in 1 s
        let mut t = 1300;
        while t < 4200 {
            assert!(d.step(t, 100, &[]).is_none(), "{t}");
            t += 100;
        }
        while t < 6000 {
            d.step(t, 100, &[]);
            t += 100;
        }
        assert_eq!(d.frac, 0.0);
        assert!(close(d.pos(), [0.0, 50.0, 30.0]));
    }

    #[test]
    fn turns_away_from_the_player() {
        // the authored turn swings the panel to +x; a player on the +x side gets the other way
        let mut d = TurnDoor::new(9, info(true));
        d.step(0, 500, &[[40.0, 50.0, 30.0]]);
        assert_eq!(d.sign, -1.0);
        d.step(500, 500, &[[40.0, 50.0, 30.0]]);
        assert!(close(d.pos(), [-30.0, 50.0, 0.0]), "{:?}", d.pos());
        let mut e = TurnDoor::new(9, info(true));
        e.step(0, 500, &[[-40.0, 50.0, 30.0]]);
        assert_eq!(e.sign, 1.0);
    }

    #[test]
    fn switches_and_start_open_doors_do_not_turn() {
        let mut s = TurnDoor::new(
            3,
            TurnInfo {
                kind: TurnKind::Switch,
                ..info(false)
            },
        );
        assert!(s.step(0, 100, &[[0.0, 50.0, 30.0]]).is_none());
        assert_eq!(s.frac, 0.0);
        let o = TurnDoor::new(
            4,
            TurnInfo {
                start_open: true,
                ..info(false)
            },
        );
        assert_eq!(o.frac, 1.0);
        assert!(close(o.pos(), [30.0, 50.0, 0.0]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> DoorInfo {
        DoorInfo {
            link: Vec::new(),
            locked: false,
            name: b"Door".to_vec(),
            closed: [0.0, 50.0, 0.0],
            lo: [-1.0, 0.0, -30.0],
            hi: [1.0, 100.0, 30.0],
            dir: [0.0, 0.0, -1.0],
            dist: 60.0,
            open_ms: 1000,
            close_ms: 1000,
            open_by: OpenBy::Near,
            cmds: DoorCmds::default(),
            sounds: DoorSounds::default(),
            usable: true,
        }
    }

    /// DSV-340: the activate entry: disabled for a door players may not use or one that is locked; state on from the
    /// step that starts the opening, off from the step that starts the closing; the blob and the note carry it in the
    /// widths the 2009 reader takes.
    #[test]
    fn the_activate_entry_follows_the_door() {
        assert_eq!(activate_of(true, false, false), (false, ACTIVATE_OFF));
        assert_eq!(activate_of(true, false, true), (false, ACTIVATE_ON));
        assert!(activate_of(false, false, false).0);
        assert!(activate_of(true, true, false).0);
        let mut d = Door::new(1, info());
        assert_eq!(d.activate(), (false, ACTIVATE_OFF));
        d.step(0, 16, &[[0.0, 50.0, 0.0]]);
        assert_eq!(d.activate(), (false, ACTIVATE_ON));
        d.step(5000, 16, &[]);
        assert_eq!(d.activate(), (false, ACTIVATE_OFF));
        let b = door_blob_of(DOOR_ACTIVATE_ROW, true, ACTIVATE_ON);
        assert_eq!(b.bit_len(), 25);
        let mut r = b.reader();
        assert_eq!(
            (r.read_u8(), r.read_u8(), r.read_bit(), r.read_u8()),
            (0x39, 0, true, 0)
        );
        let n = crate::gs::door_note(0x123, NOTE_STATE, ACTIVATE_ON);
        let s = crate::gs::door_note(0x123, NOTE_DISABLED, 1);
        assert_eq!(n.bit_len() - s.bit_len(), 7, "state is 8 bits, disabled 1");
    }

    /// DSV-302: a door plays its opening sound once when it starts to open near a player, its closing sound once
    /// when it starts to close, none while it stands; a door without sound names plays none.
    #[test]
    fn a_door_plays_its_sounds_at_the_start_of_each_move() {
        let mut d = Door::new(
            1,
            DoorInfo {
                sounds: DoorSounds {
                    opening: "open.wav".into(),
                    closing: "close.wav".into(),
                    offset: [0.0; 3],
                    radius: 1000.0,
                    locked: String::new(),
                },
                ..info()
            },
        );
        let near = [[0.0, 50.0, 0.0]];
        let mut t = 0;
        for _ in 0..30 {
            t += 100;
            d.step(t, 100, &near);
        }
        assert_eq!(d.sounds_due, ["open.wav"], "one opening sound");
        d.sounds_due.clear();
        for _ in 0..60 {
            t += 100;
            d.step(t, 100, &[]);
        }
        assert_eq!(d.sounds_due, ["close.wav"], "one closing sound");
        let mut q = Door::new(2, info());
        q.step(100, 100, &near);
        assert!(q.sounds_due.is_empty());
    }

    #[test]
    fn blob_and_record_forms() {
        let b = door_blob();
        assert_eq!(b.bit_len(), 25);
        // the reference stack's 25 bits: 39 00 02 00
        assert_eq!(b.as_bytes(), [0x39, 0x00, 0x02, 0x00]);
        let r = door_move_record(7, [1.0, 2.0, 3.0], [0.0, 0.0, -60.0]);
        assert_eq!(r.bit_len(), 8 + 16 + 192);
    }

    #[test]
    fn opens_near_a_player_and_closes_three_seconds_after() {
        let mut d = Door::new(5, info());
        // far away: nothing
        assert!(d.step(0, 100, &[[500.0, 50.0, 0.0]]).is_none());
        // near: opens over one second in ten steps, the last with velocity 0
        let mut recs = Vec::new();
        for k in 1..=12 {
            if let Some(r) = d.step(k * 100, 100, &[[50.0, 50.0, 0.0]]) {
                recs.push(r);
            }
        }
        assert_eq!(recs.len(), 10);
        assert_eq!(recs[0].1, [0.0, 0.0, -60.0]);
        assert_eq!(recs[9].0, [0.0, 50.0, -60.0]);
        assert_eq!(recs[9].1, [0.0; 3]);
        // the player leaves at 1200: still open until 1200 + 3000
        let mut t = 1300;
        while t < 4200 {
            assert!(d.step(t, 100, &[]).is_none(), "{t}");
            t += 100;
        }
        let mut closing = 0;
        while t < 6000 {
            if d.step(t, 100, &[]).is_some() {
                closing += 1;
            }
            t += 100;
        }
        assert_eq!(closing, 10);
        assert_eq!(d.offset, 0.0);
    }
}

#[cfg(test)]
mod script_tests {
    use super::*;

    fn gate(name: &str, x: f32, locked: bool, locked_cmd: &str) -> TurnInfo {
        TurnInfo {
            name: name.as_bytes().to_vec(),
            kind: TurnKind::Door,
            origin: [x, 50.0, 30.0],
            lo: [x - 2.0, 0.0, 0.0],
            hi: [x + 2.0, 100.0, 60.0],
            pivot: [x, 50.0, 0.0],
            axis: [0.0, 1.0, 0.0],
            angle: std::f32::consts::FRAC_PI_2,
            open_ms: 100,
            close_ms: 100,
            away: false,
            start_open: false,
            open_by: OpenBy::Use,
            cmds: DoorCmds {
                locked: locked_cmd.as_bytes().to_vec(),
                ..DoorCmds::default()
            },
            sounds: DoorSounds {
                locked: "locked.wav".into(),
                ..DoorSounds::default()
            },
            link: Vec::new(),
            locked,
        }
    }

    /// DSV-569: a touched trigger's `lock` / `unlock` statements are handed on in order (the spawn points take
    /// them), with a delayed one when it is due.
    #[test]
    fn lock_statements_are_handed_on_for_the_spawn_points() {
        let cp = trigger(
            "Checkpoint",
            &[
                "msg StartA lock;\r\nmsg StartB unlock;",
                "delay 2 (msg StartC unlock);",
            ],
            1,
            Some([0.0; 3]),
        );
        let mut trig = Triggers::new(vec![cp]);
        let mut dl = DelayedStatements::new();
        let fired = trig.touch(&[[0.0; 3]]);
        run_world(&mut [], &mut [], &mut trig, &mut dl, fired, 0);
        assert_eq!(
            std::mem::take(&mut trig.locks),
            vec![(b"StartA".to_vec(), true), (b"StartB".to_vec(), false)]
        );
        run_world(&mut [], &mut [], &mut trig, &mut dl, Vec::new(), 2000);
        assert_eq!(trig.locks, vec![(b"StartC".to_vec(), false)]);
    }

    fn trigger(name: &str, cmds: &[&str], max: u32, touch: Option<[f32; 3]>) -> ScriptTrigger {
        ScriptTrigger {
            name: name.as_bytes().to_vec(),
            cmds: cmds.iter().map(|c| c.as_bytes().to_vec()).collect(),
            max,
            touch: touch.is_some(),
            pos: touch.unwrap_or([0.0; 3]),
            dims: [20.0, 40.0, 20.0],
            locked: false,
        }
    }

    /// DSV-560 / DSV-562: a locked door keeps shut on the use key, plays its locked sound and runs its locked command,
    /// whose trigger (once) unlocks the other gate; the unlocked gate opens on the use key; the entry is disabled while
    /// locked.
    #[test]
    fn a_locked_door_runs_its_locked_command_and_stays_shut() {
        let mut doors: Vec<Door> = Vec::new();
        let mut turns = vec![
            TurnDoor::new(1, gate("GateA", 0.0, true, "msg Opener trigger;")),
            TurnDoor::new(2, gate("GateB", 500.0, true, "")),
        ];
        assert_eq!(turns[0].activate(), Some((true, ACTIVATE_OFF)), "no prompt");
        let mut trig = Triggers::new(vec![trigger(
            "Opener",
            &[
                "msg GateB unlock;",
                "msg Lamp (visible 1);\r\nmsg Wall (solid 0);",
            ],
            1,
            None,
        )]);
        let mut dl = DelayedStatements::new();
        let at = [10.0, 50.0, 30.0];
        assert!(!use_key(&mut doors, &mut turns, at, 0));
        assert_eq!(turns[0].sounds_due, ["locked.wav"]);
        let models = run_world(&mut doors, &mut turns, &mut trig, &mut dl, Vec::new(), 0);
        assert!(turns[0].locked && !turns[0].want, "still shut");
        assert!(!turns[1].locked, "the trigger unlocked the other gate");
        assert_eq!(
            models,
            [
                ModelSwitch {
                    name: b"Lamp".to_vec(),
                    visible: Some(true),
                    solid: None
                },
                ModelSwitch {
                    name: b"Wall".to_vec(),
                    visible: None,
                    solid: Some(false)
                }
            ]
        );
        // the trigger fires once
        assert!(!use_key(&mut doors, &mut turns, at, 1000));
        assert!(run_world(&mut doors, &mut turns, &mut trig, &mut dl, Vec::new(), 10).is_empty());
        assert_eq!(trig.fires, [1]);
        assert!(use_key(&mut doors, &mut turns, [510.0, 50.0, 30.0], 1000));
        assert!(turns[1].want, "the unlocked gate opens");
    }

    /// DSV-561: a use switch has an enabled entry, turns on by the use key and runs its on command when on, its off
    /// command when off; a switch that starts on runs its on command at the start.
    #[test]
    fn a_use_switch_runs_its_on_and_off_commands() {
        let sw = |start: bool| TurnInfo {
            kind: TurnKind::Switch,
            cmds: DoorCmds {
                open: b"msg Lit trigger".to_vec(),
                closed: b"msg Dark trigger".to_vec(),
                ..DoorCmds::default()
            },
            start_open: start,
            angle: -0.17,
            open_ms: 10,
            close_ms: 10,
            ..gate("Switch", 0.0, false, "")
        };
        let mut trig = Triggers::new(vec![
            trigger(
                "Lit",
                &["msg LampOn (visible 1);\r\nmsg LampOff (visible 0);"],
                0,
                None,
            ),
            trigger(
                "Dark",
                &["msg LampOn (visible 0);\r\nmsg LampOff (visible 1);"],
                0,
                None,
            ),
        ]);
        let mut doors: Vec<Door> = Vec::new();
        let mut turns = vec![TurnDoor::new(5, sw(false))];
        let mut dl = DelayedStatements::new();
        assert_eq!(turns[0].activate(), Some((false, ACTIVATE_OFF)));
        assert!(use_key(&mut doors, &mut turns, [5.0, 50.0, 30.0], 0));
        let mut seen = Vec::new();
        for t in (0..200).step_by(20) {
            turns[0].step(t, 20, &[]);
            seen.extend(run_world(
                &mut doors,
                &mut turns,
                &mut trig,
                &mut dl,
                Vec::new(),
                t,
            ));
        }
        assert_eq!(turns[0].frac, 1.0);
        assert_eq!(turns[0].activate(), Some((false, ACTIVATE_ON)));
        assert_eq!(
            seen.iter()
                .map(|m| (m.name.clone(), m.visible))
                .collect::<Vec<_>>(),
            [
                (b"LampOn".to_vec(), Some(true)),
                (b"LampOff".to_vec(), Some(false))
            ]
        );
        assert!(use_key(&mut doors, &mut turns, [5.0, 50.0, 30.0], 1000));
        let mut seen = Vec::new();
        for t in (1000..1200).step_by(20) {
            turns[0].step(t, 20, &[]);
            seen.extend(run_world(
                &mut doors,
                &mut turns,
                &mut trig,
                &mut dl,
                Vec::new(),
                t,
            ));
        }
        assert_eq!(turns[0].frac, 0.0);
        assert_eq!(seen[0].visible, Some(false), "off: the lit lamp goes");
        // started on: the on command at the start
        let mut on = vec![TurnDoor::new(6, sw(true))];
        let m = run_world(&mut doors, &mut on, &mut trig, &mut dl, Vec::new(), 0);
        assert_eq!(m.first().map(|m| m.visible), Some(Some(true)));
    }

    /// DSV-579..DSV-582: `play`, camera `moveto` / `fov`, `on` / `off`, `hidecharacter` and the AI volumes' `enable` /
    /// `disable`, as the co-op worlds write them (the delayed ones at their time), become cues and server state.
    #[test]
    fn presentation_statement_forms_become_cues() {
        use crate::script::{Cue, MissionObjects, Network};
        let mut trig =
            Triggers::with_network(Vec::new(), Network::new(MissionObjects::default(), 1));
        let mut dl = DelayedStatements::new();
        let text = [
            "msg CapTRG (play Cap101);",
            "msg Ctl (hidecharacter 1);",
            "msg cam (moveto Point01);",
            "msg cam (fov 100 84);",
            "msg cam on;",
            "msg Vol7 disable;",
            "msg Vol8 disable;",
            "msg Vol8 enable;",
            "delay 4.6 (msg Ctl (hidecharacter 0));",
            "delay 4.6 (msg Music off);",
            "msg cam (fov x);",
            "msg Msg (play)",
        ]
        .join("\r\n");
        run_world(
            &mut [],
            &mut [],
            &mut trig,
            &mut dl,
            vec![text.into_bytes()],
            0,
        );
        let cues = std::mem::take(&mut trig.net.cues);
        assert_eq!(
            cues,
            vec![
                Cue::Play {
                    trigger: b"CapTRG".to_vec(),
                    key: b"Cap101".to_vec()
                },
                Cue::HideCharacters(true),
                Cue::MoveTo {
                    camera: b"cam".to_vec(),
                    point: b"Point01".to_vec()
                },
                Cue::Fov {
                    camera: b"cam".to_vec(),
                    fov: [100.0, 84.0]
                },
                Cue::Switch {
                    target: b"cam".to_vec(),
                    on: true
                },
            ]
        );
        // the AI volumes are server state; the malformed `fov` and `play` are counted, not cued
        let off: Vec<&[u8]> = trig.net.ai_volumes_off.iter().map(Vec::as_slice).collect();
        assert_eq!(off, vec![b"vol7".as_slice()]);
        assert_eq!(trig.net.skipped, 2);
        run_world(&mut [], &mut [], &mut trig, &mut dl, Vec::new(), 4_600);
        assert_eq!(
            trig.net.cues,
            vec![
                Cue::HideCharacters(false),
                Cue::Switch {
                    target: b"Music".to_vec(),
                    on: false
                }
            ]
        );
        // a new round forgets the disabled volumes
        trig.net.reset();
        assert!(trig.net.ai_volumes_off.is_empty() && trig.net.cues.is_empty());
    }

    /// DSV-570..DSV-574 on a synthetic network: variables and conditions, spawns, delays by time, by variable and by
    /// id (with `abort`), a draw, a counter whose event unlocks an activation box, a timer stopped in time, the game
    /// control words, the goal words, `remove` and the `on` / `off` names for the session.
    #[test]
    fn mission_statement_forms_run() {
        use crate::script::{
            ActivateInfo, CounterInfo, Effect, GoalOp, MissionObjects, Network, TimerInfo,
        };
        let net = Network::new(
            MissionObjects {
                counters: vec![CounterInfo {
                    name: b"Locker".to_vec(),
                    start: 0,
                    events: vec![(2, b"msg Final unlock;\r\nmsg Fence off;".to_vec())],
                    unset: Vec::new(),
                }],
                timers: vec![TimerInfo {
                    name: b"Clock".to_vec(),
                    secs: 60.0,
                    in_time: b"set Saved 1;\r\nmsg Goals (check 1);".to_vec(),
                    over_time: b"msg Goals (fail 1);".to_vec(),
                }],
                activates: vec![ActivateInfo {
                    name: b"Final".to_vec(),
                    dims: [20.0; 3],
                    locked: true,
                    max: 1,
                    secs: 1.0,
                    activate: b"msg Ctl (aifreeze 1);\r\ndelay 2 (msg Ctl (missioncomplete 1));"
                        .to_vec(),
                    ..ActivateInfo::default()
                }],
            },
            42,
        );
        let mut trig = Triggers::with_network(
            vec![
                trigger(
                    "Checkpoint",
                    &[
                        "msg Ctl (INCLIFECOUNT 1);",
                        "msg Goals (check 0);\r\nmsg Goals (add 1);",
                        "msg Clock (timeamount 30);\r\ndelay 0.01 (msg Clock Start);",
                        "msg Locker (inc 1)",
                    ],
                    1,
                    Some([0.0; 3]),
                ),
                trigger(
                    "Wave",
                    &["msg S_A (spawn _A);\r\nmsg S_B (spawn _B);"],
                    0,
                    None,
                ),
                trigger("Pick", &["Rand2 (set Side 1)(set Side 2)"], 0, None),
            ],
            net,
        );
        let mut dl = DelayedStatements::new();
        let run = |trig: &mut Triggers, dl: &mut DelayedStatements, text: &str, now: u32| {
            run_world(
                &mut [],
                &mut [],
                trig,
                dl,
                vec![text.as_bytes().to_vec()],
                now,
            )
        };
        // start-up variables, a death command counting down, the condition firing a trigger
        run(&mut trig, &mut dl, "int Left 2;\r\nint Wait 0.5;", 0);
        let death = "sub Left 1;\r\nif (Left <= 0) then (msg Wave trigger);\r\nif (Left >= 0) then (delayvariable Wait (msg S_C (spawn _C)));";
        run(&mut trig, &mut dl, death, 0);
        assert!(
            trig.net.effects.is_empty(),
            "the respawn waits for the variable's time"
        );
        run(&mut trig, &mut dl, "", 500);
        assert_eq!(
            std::mem::take(&mut trig.net.effects),
            [Effect::Spawn {
                spawner: b"S_C".to_vec(),
                name: Some(b"_C".to_vec())
            }]
        );
        run(&mut trig, &mut dl, death, 600);
        let e = std::mem::take(&mut trig.net.effects);
        assert_eq!(e.len(), 2, "the wave trigger's two spawns: {e:?}");
        assert_eq!(trig.net.var(b"left"), 0.0);
        // a draw sets one value
        run(&mut trig, &mut dl, "msg Pick trigger;", 700);
        assert!([1.0, 2.0].contains(&trig.net.var(b"Side")));
        // `delayid` and `abort`
        run(
            &mut trig,
            &mut dl,
            "delayid K 1 (msg Lamp (visible 1));\r\nabort K;",
            800,
        );
        assert!(run(&mut trig, &mut dl, "", 5000).is_empty(), "aborted");
        assert_eq!(
            std::mem::take(&mut trig.net.effects).len(),
            1,
            "the second death's respawn (left 0 is still >= 0)"
        );
        // the checkpoint box: a life, the goals, the timer, a count
        let fired = trig.touch(&[[0.0; 3]]);
        run_world(&mut [], &mut [], &mut trig, &mut dl, fired, 6000);
        assert_eq!(
            std::mem::take(&mut trig.net.effects),
            [
                Effect::Lives(1),
                Effect::Goal(GoalOp::Check, 0),
                Effect::Goal(GoalOp::Add, 1)
            ]
        );
        run(&mut trig, &mut dl, "", 6010);
        run(&mut trig, &mut dl, "msg Clock stop;", 20_000);
        assert_eq!(trig.net.var(b"saved"), 1.0, "stopped in time");
        assert_eq!(
            std::mem::take(&mut trig.net.effects),
            [Effect::Goal(GoalOp::Check, 1)]
        );
        // the second count unlocks the final box and switches the fence off
        run(&mut trig, &mut dl, "msg Locker (inc 1);", 21_000);
        assert_eq!(trig.net.activate_locked, [false]);
        assert_eq!(
            std::mem::take(&mut trig.net.switched),
            [(b"Fence".to_vec(), false)]
        );
        // held for its time: the AI freeze, the mission completes after the delay
        trig.net.activate_tick(&[(3, [0.0; 3])], 22_000);
        trig.net.activate_tick(&[(3, [0.0; 3])], 23_000);
        run(&mut trig, &mut dl, "", 23_000);
        assert_eq!(
            std::mem::take(&mut trig.net.effects),
            [Effect::Freeze(true)]
        );
        run(&mut trig, &mut dl, "", 25_000);
        assert_eq!(trig.net.effects, [Effect::Complete]);
        // `remove` switches a model off
        let m = run(&mut trig, &mut dl, "msg Wall remove;", 26_000);
        assert_eq!(
            m,
            [ModelSwitch {
                name: b"Wall".to_vec(),
                visible: Some(false),
                solid: Some(false)
            }]
        );
    }

    /// DSV-576..DSV-578 on a synthetic round world: the round loop is found (the start box nobody fires, the round
    /// counter, the kill counter whose events are unset), the start box counts round 1, the deaths count the kill
    /// counter without an event, a round end heals, pauses an effect and counts the next round by its box.
    #[test]
    fn round_world_loop_and_words() {
        use crate::script::{round_network, CounterInfo, Effect, MissionObjects, Network};
        let objects = MissionObjects {
            counters: vec![
                CounterInfo {
                    name: b"Stages".to_vec(),
                    start: 0,
                    events: vec![(1, b"set Shown 1".to_vec()), (3, b"set Shown 3".to_vec())],
                    unset: Vec::new(),
                },
                CounterInfo {
                    name: b"Kills".to_vec(),
                    start: 0,
                    events: Vec::new(),
                    unset: vec![
                        (
                            1,
                            b"delay 2 (msg Ctl (HEALINPART));\r\ndelay 12 (msg Next trigger);"
                                .to_vec(),
                        ),
                        (2, b"msg Fx pause;\r\nmsg Ctl (missioncomplete 1);".to_vec()),
                    ],
                },
            ],
            ..MissionObjects::default()
        };
        let list = vec![
            trigger("Begin", &["msg Stages (inc 1)", "msg Music on;"], 1, None),
            trigger("Next", &["msg Stages (inc 1)"], 0, None),
            trigger("Lamp", &["msg Light off"], 0, None),
        ];
        let death: &[u8] = b"msg Kills (inc 1);";
        let r = round_network(&list, &objects, &[death, b""]).expect("a round world");
        assert_eq!(
            r.start_boxes,
            [b"Begin".to_vec()],
            "Next is fired by the round end"
        );
        assert_eq!(r.round_counter, b"Stages");
        assert_eq!(r.kill_counter, b"Kills");
        assert_eq!(
            r.clear_events.iter().map(|e| e.0).collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(
            round_network(&list, &objects, &[b""]).is_none(),
            "no death command counts a counter: no round world"
        );
        let mut net = Network::new(objects, 3);
        net.round_counter = Some(r.round_counter.clone());
        let mut trig = Triggers::with_network(list, net);
        let mut dl = DelayedStatements::new();
        let mut run = |trig: &mut Triggers, text: &[u8], now: u32| {
            run_world(&mut [], &mut [], trig, &mut dl, vec![text.to_vec()], now);
            std::mem::take(&mut trig.net.effects)
        };
        // the server fires the start box at the match start: round 1, its event
        assert_eq!(run(&mut trig, b"msg Begin trigger", 0), [Effect::Round(1)]);
        assert_eq!(trig.net.var(b"shown"), 1.0);
        // AI deaths count the kill counter; its unset events never run by counting
        for _ in 0..40 {
            assert!(run(&mut trig, death, 100).is_empty());
        }
        assert_eq!(trig.net.counters[1], 40);
        assert!(trig.net.queued.is_empty());
        // the round is cleared: the server runs its round end
        let end = r.clear_events[0].1.clone();
        assert!(run(&mut trig, &end, 1000).is_empty());
        assert_eq!(run(&mut trig, b"", 3000), [Effect::HealInPart]);
        assert_eq!(run(&mut trig, b"", 13_000), [Effect::Round(2)]);
        assert_eq!(trig.net.counters[0], 2);
        // the last round end: `pause` is taken (client only), the mission completes
        let skipped = trig.net.skipped;
        let end = r.clear_events[1].1.clone();
        assert_eq!(run(&mut trig, &end, 20_000), [Effect::Complete]);
        assert_eq!((trig.net.client_only, trig.net.skipped), (1, skipped));
        // `healinpart` as the statement reads in any case
        assert_eq!(
            run(&mut trig, b"msg Ctl (healinpart)", 21_000),
            [Effect::HealInPart]
        );
    }

    /// DSV-563: a touched trigger fires when a player enters its box (not again while he stays, at most its number of
    /// activations), not while locked; a message-only trigger is never touched.
    #[test]
    fn touched_triggers_fire_on_entry() {
        let mut t = Triggers::new(vec![
            trigger("Box", &["msg Door unlock;"], 2, Some([0.0, 0.0, 0.0])),
            trigger("Msg", &["msg Door lock;"], 0, None),
        ]);
        assert!(t.touch(&[[100.0, 0.0, 0.0]]).is_empty(), "outside");
        assert_eq!(t.touch(&[[30.0, 0.0, 0.0]]).len(), 1, "within the margin");
        assert!(t.touch(&[[30.0, 0.0, 0.0]]).is_empty(), "staying");
        assert!(t.touch(&[]).is_empty());
        assert_eq!(t.touch(&[[0.0, 0.0, 0.0]]).len(), 1, "again");
        t.touch(&[]);
        assert!(t.touch(&[[0.0, 0.0, 0.0]]).is_empty(), "twice at most");
        let mut l = Triggers::new(vec![trigger("Box", &["x"], 0, Some([0.0; 3]))]);
        l.set_lock(b"box", true);
        assert!(l.touch(&[[0.0; 3]]).is_empty(), "locked");
    }
}

#[cfg(test)]
mod script_bound_tests {
    use super::*;

    fn trig(name: &str, cmds: &[&str], max: u32, touch: Option<[f32; 3]>) -> ScriptTrigger {
        ScriptTrigger {
            name: name.as_bytes().to_vec(),
            cmds: cmds.iter().map(|c| c.as_bytes().to_vec()).collect(),
            max,
            touch: touch.is_some(),
            pos: touch.unwrap_or([0.0; 3]),
            dims: [20.0, 40.0, 20.0],
            locked: false,
        }
    }

    /// Review item 1: a trigger without an activation limit that fires itself twice (and two that fire each other)
    /// would double the pending texts every round; the run stops at the statement budget and drops the rest.
    #[test]
    fn a_self_doubling_trigger_stops_at_the_budget() {
        let mut t = Triggers::new(vec![
            trig(
                "Twice",
                &["msg Twice trigger;\r\nmsg Twice trigger;\r\nmsg Lamp (visible 1);"],
                0,
                None,
            ),
            trig(
                "PingA",
                &["msg PingB trigger;\r\nmsg PingB trigger;"],
                0,
                None,
            ),
            trig(
                "PingB",
                &["msg PingA trigger;\r\nmsg PingA trigger;"],
                0,
                None,
            ),
        ]);
        let (mut doors, mut turns): (Vec<Door>, Vec<TurnDoor>) = (Vec::new(), Vec::new());
        let mut dl = DelayedStatements::new();
        for start in ["msg Twice trigger;", "msg PingA trigger;"] {
            let before = t.over_budget;
            let m = run_world(
                &mut doors,
                &mut turns,
                &mut t,
                &mut dl,
                vec![start.as_bytes().to_vec()],
                0,
            );
            assert_eq!(t.over_budget, before + 1, "cut at the budget");
            assert!(m.len() <= TRIGGER_STATEMENTS);
        }
        let total: u32 = t.fires.iter().sum();
        assert!(total as usize <= 2 * TRIGGER_STATEMENTS, "{total} fires");
        // nothing is left over for the next run
        let m = run_world(&mut doors, &mut turns, &mut t, &mut dl, Vec::new(), 10);
        assert!(m.is_empty());
    }

    /// Review item 2: two touch boxes with the same name each fire themselves (and spend their own activation).
    #[test]
    fn same_named_boxes_fire_themselves() {
        let mut t = Triggers::new(vec![
            trig("", &["msg A unlock;"], 1, Some([0.0, 0.0, 0.0])),
            trig("", &["msg B unlock;"], 1, Some([1000.0, 0.0, 0.0])),
        ]);
        assert_eq!(t.touch(&[[1000.0, 0.0, 0.0]]), [b"msg B unlock;".to_vec()]);
        assert_eq!(t.fires, [0, 1]);
        assert_eq!(t.touch(&[[0.0, 0.0, 0.0]]), [b"msg A unlock;".to_vec()]);
        assert_eq!(t.fires, [1, 1]);
    }

    /// Review item 3: use packets in a burst on a locked door run its locked command and queue its sound once per
    /// cooldown; a switch turns at most once per cooldown.
    #[test]
    fn use_key_bursts_are_cooled() {
        let info = DoorInfo {
            name: b"Gate".to_vec(),
            closed: [0.0, 50.0, 0.0],
            lo: [-1.0, 0.0, -30.0],
            hi: [1.0, 100.0, 30.0],
            dir: [0.0, 0.0, -1.0],
            dist: 60.0,
            open_ms: 100,
            close_ms: 100,
            open_by: OpenBy::Use,
            cmds: DoorCmds {
                locked: b"msg X trigger;".to_vec(),
                ..DoorCmds::default()
            },
            sounds: DoorSounds {
                locked: "locked.wav".into(),
                ..DoorSounds::default()
            },
            usable: true,
            locked: true,
            link: Vec::new(),
        };
        let mut doors = vec![Door::new(1, info)];
        let mut turns: Vec<TurnDoor> = Vec::new();
        for k in 0..100 {
            use_key(&mut doors, &mut turns, [5.0, 50.0, 0.0], k);
        }
        assert_eq!(doors[0].fired.len(), 1, "one locked command in the burst");
        assert_eq!(doors[0].sounds_due, ["locked.wav"]);
        use_key(
            &mut doors,
            &mut turns,
            [5.0, 50.0, 0.0],
            USE_COOLDOWN_MS + 50,
        );
        assert_eq!(doors[0].fired.len(), 2, "again after the cooldown");
        assert_eq!(doors[0].sounds_due.len(), 1, "one sound queued at a time");

        let sw = TurnInfo {
            name: b"Sw".to_vec(),
            kind: TurnKind::Switch,
            origin: [0.0, 50.0, 0.0],
            lo: [-2.0, 40.0, -2.0],
            hi: [2.0, 60.0, 2.0],
            pivot: [0.0, 50.0, 0.0],
            axis: [1.0, 0.0, 0.0],
            angle: -0.17,
            open_ms: 10,
            close_ms: 10,
            away: false,
            start_open: false,
            open_by: OpenBy::Use,
            cmds: DoorCmds::default(),
            sounds: DoorSounds::default(),
            link: Vec::new(),
            locked: false,
        };
        let mut doors: Vec<Door> = Vec::new();
        let mut turns = vec![TurnDoor::new(2, sw)];
        assert!(use_key(&mut doors, &mut turns, [0.0, 50.0, 10.0], 0));
        assert!(
            !use_key(&mut doors, &mut turns, [0.0, 50.0, 10.0], 100),
            "cooling"
        );
        assert!(turns[0].want);
        assert!(use_key(&mut doors, &mut turns, [0.0, 50.0, 10.0], 600));
        assert!(!turns[0].want);
    }

    /// Review item 5: the start scripts' fires count; a new round starts the counts and locks again from there.
    #[test]
    fn seeds_and_resets() {
        let mut t = Triggers::new(vec![trig("Start", &["msg D unlock;"], 1, None)]);
        t.seed(&[b"start".to_vec()]);
        assert_eq!(t.fire(b"Start"), None, "fired by the start scripts already");
        t.set_lock(b"Start", true);
        t.reset();
        assert_eq!(t.fires, [0]);
        assert_eq!(t.locked, [false], "the file's lock");
        assert!(t.fire(b"Start").is_some());
    }

    /// The statement value: numbers by their value, the words true / false, else any byte but `0`.
    #[test]
    fn statement_values() {
        for (v, want) in [
            (&b"1"[..], true),
            (b"0", false),
            (b"0.0", false),
            (b"1.0", true),
            (b"false", false),
            (b"TRUE", true),
            (b"00", false),
            (b"x", true),
        ] {
            assert_eq!(script_flag(v), want, "{:?}", String::from_utf8_lossy(v));
        }
    }
}
