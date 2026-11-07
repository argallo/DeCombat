//! The client's sounds: what the server sends (tracked world sounds and one-shots of `S2C_OBJ_GROUP`, the player
//! sound relays and shot effects of engine id 17, the preload list) and what the client makes itself (footsteps at
//! the footstep keys of the characters' animation clips, the jump sounds, the own weapon's sounds), played through
//! dc-audio's software mixer with the 2009 distance law. A window run renders the mixer into the default output
//! device ([`AudioOut::Device`], dc-audio's cpal layer, feature `audio`); headless runs and a window run without a
//! device use the null backend (the mix goes into a scratch buffer that nobody hears), so every start is still
//! decided, logged and attenuated the same way.
//!
//! Every start goes into [`SoundDirector::log`] (and, when asked, a JSONL file) with the sound file's hash, the
//! position and the trigger. File names are ROM data: they are read at run time and only their hashes are written.
// SPEC: AUDIO_2009.md §3 (voice model, request flags), §5 (distance law, listener), §14 (server-started sounds,
// footsteps from clip keys); UDP_SESSION_2009_B.md §8.1-§8.3; GAME_MESSAGES_2009_S2C.md §8.2 (types 0x19, 0x06)

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use dc_audio::decode::Pcm;
use dc_audio::mixer::{Listener, Mixer, PlayRequest, VoiceId, VoiceKind};
use dc_audio::output::NullSink;
use dc_audio::settings::AudioSettings;
use dc_botclient::objects::SoundStart as WireSound;
use dc_bute::ButeDoc;
use dc_rez::RezVfs;
use dc_sim::V3;

use crate::world::{RemoteBody, RemoteLook};

use crate::music::{MusicCue, MusicDirector};

pub mod cues;
pub use cues::LadderSound;

// --------------------------------------------------------------------------------------------- request flags

/// Request flag bits the client uses.
// SPEC: AUDIO_2009.md §3.2 (bit values PROVEN)
pub mod flag {
    pub const AMBIENT: u16 = 0x0001;
    pub const POSITIONAL: u16 = 0x0002;
    pub const LOOP: u16 = 0x0004;
    pub const VOLUME: u16 = 0x0040;
    pub const REVERB: u16 = 0x0100;
    pub const CLIENT_ONLY: u16 = 0x0200;
    /// A local (2D) voice when the listener is the local player (always so in this client); on a tracked sound the
    /// position follows the own view object.
    pub const LOCAL_IF_OWN: u16 = 0x0800;
    pub const NO_REVERB: u16 = 0x2000;
}

/// Sound classes.
// SPEC: AUDIO_2009.md §4.2 (0 default, 1 weapons, 2 speech)
pub const CLASS_DEFAULT: u8 = 0;
pub const CLASS_WEAPONS: u8 = 1;

/// Outer radius of a footstep when no setting overrides it.
// EVIDENCE: cshell+0x202870 (outer = 1200.0, constant 0x44960000 at cshell+0x202b02, when the settings object at
// VA 0x3759c418 is absent; that object's value is not traced)
pub const FOOTSTEP_OUTER: f32 = 1200.0;
/// Priority byte of a footstep: 4 for another character, 6 for the local player's character.
// EVIDENCE: cshell+0x202a7b..0x202a80 (`lea edx, [edx + edx + 4]` with edx = "the FX is the local player's")
pub const FOOTSTEP_PRIO_REMOTE: u8 = 4;
pub const FOOTSTEP_PRIO_OWN: u8 = 6;
/// Inner radius as a share of the outer radius for client-made sounds that pass no inner radius (the 2009 play
/// helper multiplies the outer radius by a manager field whose value is not traced).
// EVIDENCE: cshell+0x27dd70 (inner = outer * manager field +0x15ac when the caller passes a negative inner)
// DC-DEFAULT: CLI-66 (proposed) 17 / 255 of the outer radius: the ratio byte of every damage and bounce sound the
// reference server started (UDP_SESSION_2009_B.md §8.3, ratio 17; PLAUSIBLY the same game-wide default)
pub const INNER_SHARE: f32 = 17.0 / 255.0;
/// Outer radius and priority of the relayed jump sound.
// EVIDENCE: cshell+0x2cbd30 (k = 15: one of two character jump sounds at the record position, outer 1000.0
// (0x447a0000), priority 2, flags 0, volume 100, class 0)
pub const JUMP_OUTER: f32 = 1000.0;
pub const JUMP_PRIO: u8 = 2;
/// Priority of a weapon sound at a position, and of the own weapon's sounds (played as local voices).
// EVIDENCE: cshell+0x1a2b50 (positional: priority 5, flags 0x100, volume 100, class 1; own: cshell+0x1b1e20 with
// priority 6, flags 0x100, class 1)
pub const WEAPON_PRIO: u8 = 5;
pub const WEAPON_PRIO_OWN: u8 = 6;
/// Outer radius of a weapon sound whose row names no radius for it.
// EVIDENCE: cshell+0x1a2b50 (`param_2 = 2000.0` before the switch; sounds 10..14 keep it)
pub const WEAPON_OUTER_DEFAULT: f32 = 2000.0;
/// The sound radii a weapon row starts with before its keys are read (fire, reload, select): a row without the key
/// keeps them.
// DISASM: cshell+0x17f9f5..0x17fa09 (the row's start values: +0x1f8 := 1000, +0x230 := 300, +0x234 := 300),
// cshell+0x17cd4a..0x17cdcc (each radius key read with the current value as its default), cshell+0x1a2b50 (sounds
// 1..3 play with +0x230, 4 / 5 with +0x234, 6..8 with +0x1f8)
// CLI-1761 (user playtest 2026-11-03 "the bomb plant sound is heard across the map": rows without the key read 0, which
// played as 2000)
pub const ROW_FIRE_RADIUS: f32 = 1000.0;
pub const ROW_RELOAD_RADIUS: f32 = 300.0;
pub const ROW_SELECT_RADIUS: f32 = 300.0;
/// The weapon sound numbers: sound `k` of the relay record.
// EVIDENCE: cshell+0x1a2b50 (switch on k - 1)
pub const SOUND_FIRE: u8 = 6;
/// Weapon sound 8, the alternate fire sound.
// EVIDENCE: cshell+0x1a2b50 (case 7 of the switch on k - 1: the row's +0x8c file at the fire radius)
pub const SOUND_ALT_FIRE: u8 = 8;

/// Whether weapon sound `k` replaces the emitter's previous one of the same weapon: the 2009 client keeps the
/// last fire and alternate-fire voice per emitting object and weapon id, kills it before it starts the next one
/// and stores the new voice, so one shooter's shots never overlap (each shot cuts the tail of the one before, and
/// a second shot record in the same frame replaces the first voice instead of doubling it).
// EVIDENCE: cshell+0x1a2b50 (sounds 6 and 8: cshell+0x1a2ac0 returns a global map, cshell+0x1a2610(object,
// weapon id) looks up map[object][weapon] and, when present, calls the engine sound interface's method +0x10 with
// the stored voice and erases it; after the play cshell+0x1a2770(object, weapon id, voice) stores the new voice),
// engine+0xdae80 (that method, slot +0x10 of the sound manager table at engine+0x222680: finds the voice in the
// active list, needs the client-only bit 0x0200 that the client-local play engine+0xdc3a0 forces, and stops and
// frees it through engine+0xdac90)
pub fn cuts_previous(k: u8) -> bool {
    k == SOUND_FIRE || k == SOUND_ALT_FIRE
}

/// The emitter of a weapon sound, as the fire-voice table keys it: the own weapon, an object of a shot record, or
/// the player of a type 0x19 relay.
// EVIDENCE: cshell+0x1a2b50 first argument: the own fire state passes its owner (cshell+0x237180), the shot effect
// its shooter object (cshell+0x2c6580, +0x38), the relay its object (cshell+0x2cbd30, +0x0c)
// DC-DEFAULT: CLI-201 (proposed) the relay keys by its player id, the shot record by its object id (the client does
// not join the two), so a relay and a shot record of one shooter do not cut each other
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Emitter {
    Own,
    Object(u16),
    Player(u32),
}

/// The last fire voice per emitter and weapon ([`cuts_previous`]).
#[derive(Debug, Clone, Default)]
pub struct FireVoices {
    map: HashMap<(Emitter, u16), VoiceId>,
}

impl FireVoices {
    /// Takes the stored voice of `(emitter, weapon)` out of the table (the caller stops it).
    pub fn take(&mut self, emitter: Emitter, weapon: u16) -> Option<VoiceId> {
        self.map.remove(&(emitter, weapon))
    }
    /// Stores the new voice of `(emitter, weapon)`.
    pub fn keep(&mut self, emitter: Emitter, weapon: u16, id: VoiceId) {
        self.map.insert((emitter, weapon), id);
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// The surface id the take-off jump sound is not played on.
// SPEC: MOVEMENT_2009.md §12.3 (one of two jump sounds unless the surface type is 0xca)
pub const SURFACE_NO_JUMP_SOUND: u8 = 0xca;

/// Flag bits the 2009 client sets on every shot effect before it masks them with the record's `b`.
// DISASM: cshell+0x2c8b3f (`or eax, 0x1810`)
pub const SHOT_FLAGS_ALWAYS: u16 = 0x1810;
/// The flag bits that let the shot effect play the shooter's fire sound.
// DISASM: cshell+0x2c8eeb..0x2c8ef2 (`test ah, 0x19` on the object's +0x50 word before the call of cshell+0x2c6580)
pub const SHOT_FLAGS_FIRE_SOUND: u16 = 0x1900;

/// Whether a type 0x06 record plays its shooter's fire sound: the 2009 client computes its effect flags as
/// `(ammo-row field | 0x1810) & !b` and calls the fire-sound routine only when a bit of 0x1900 survives. The
/// ammo-row field (a dword of the ROM ammo row's sub-record, `[[row + 0x94] + 0x18]`) is not traced and is
/// taken as 0 here, so bit 0x0100 never contributes: a record whose `b` clears both 0x1000 and 0x0800 is silent.
/// A blast-style `b` clears both. The character-hit records of DSV-86 (hit 0x0f11 / 0x0f01, continuation 0x4bf0 /
/// 0x4be0) both keep 0x1000, so both records of a hitting shot start the fire sound; the second one cuts the first
/// ([`cuts_previous`], CLI-201), so the shot is heard once (OBSERVED: the sbs4-rust and rustclient logs of
/// 2026-10-25 show two `shot_fire` starts per hitting shot).
// DISASM: cshell+0x2c8b25..0x2c8b46 (object +0x50 := (field | 0x1810) & ~(+0x54), +0x54 = the record's `b`:
// cshell+0x2c696b..0x2c696f), cshell+0x2c8eeb..0x2c8ef6 (the call of cshell+0x2c6580 under `test ah, 0x19`)
// DC-DEFAULT: CLI-84 (proposed) the untraced ammo-row field is 0, see the staged client-audio rows
pub fn fire_sound_wanted(clear: u16) -> bool {
    (SHOT_FLAGS_ALWAYS & !clear) & SHOT_FLAGS_FIRE_SOUND != 0
}

/// Whether a type 0x06 record's `c` names the own client: the 2009 fire-sound routine returns when the record's
/// byte equals the client id stored at the object's creation (0xff before any creation, so an unknown own id
/// matches nothing).
// DISASM: cshell+0x2c6580 (`return` when +0xdd == +0xdc), cshell+0x2c6a63..0x2c6a66 (+0xdc := the record's `c`),
// cshell+0x2c85b2..0x2c85c3 (+0xdd := the local client id); EVIDENCE: cshell+0x2160c0 (+0xdd starts as 0xff)
pub fn shot_names_own_client(client: u8, own: Option<u32>) -> bool {
    own.is_some_and(|id| u32::from(client) == id)
}

/// The extra effect flag a shot record must leave set for the shooter's own hit feedback: the 2009 client computes
/// the record's extra flags as `(row field | 0x4000) & !b` and, for its own shot, plays the feedback only when bit
/// 0x4000 survives. The reference stack's character-hit records (`b` 0x0f11 / 0x0f01) keep it; misses (0x4f10) and the
/// continuation record of a hit (0x4bf0 / 0x4be0) clear it, so one hit gives one feedback.
// DISASM: cshell+0x2c8b4a..0x2c8b57 (object +0x52 := (+0x52 | 0x4000) & ~b), cshell+0x2c8b78..0x2c8b91 (own shot:
// +0xdd == +0xdc; `test ah, 0x40` on +0x52, else the effect ends)
// SPEC: DC_DEFAULTS.md DSV-86 (the reference stack's hit masks; REFERENCE tdm4 / tdm6)
pub const SHOT_EXTRA_HIT_FEEDBACK: u16 = 0x4000;

/// Whether an own shot record (its `c` names the own client) plays the shooter's hit feedback.
// DISASM: cshell+0x2c8b4a..0x2c8b91 (see [`SHOT_EXTRA_HIT_FEEDBACK`]); the client-variable gate after it is on by
// default (DISASM cshell+0x2c8b97..0x2c8baf: a float client variable compared with 0.0, registered with 1.0 at
// cshell+0x2c5b20..0x2c5b37)
pub fn hit_feedback_wanted(clear: u16) -> bool {
    clear & SHOT_EXTRA_HIT_FEEDBACK == 0
}

/// The first field of the shot's ammunition record (PLAUSIBLY its id) for which the 2009 client takes another,
/// untraced feedback path (cshell+0x2c6620 with the arguments 2 and 5, a sound name built per character) instead of
/// the two feedback sounds; this client plays nothing for them.
// DISASM: cshell+0x2c8bb5..0x2c8be1 (`cmp eax, 0x25` / `cmp eax, 0x27` on [[object +0xf4]], else cshell+0x2c65f0)
pub const HIT_FEEDBACK_OTHER_PATH: [u16; 2] = [0x25, 0x27];
/// Distance below the feet a remote character's surface is looked for.
// DC-DEFAULT: CLI-67 (proposed) the 2009 client traces down from the foot socket (cshell+0x204f60, depth not
// read); this client traces from the body centre to 48 units below the feet, surface 0 when nothing is hit
pub const REMOTE_SURFACE_DEPTH: f32 = 48.0;
/// How long a sprint breath that made no voice (no ROM file) counts as playing.
// DC-DEFAULT: CLI-632 (proposed) 1 s
pub const BREATH_NO_VOICE_MS: u32 = 1000;
/// The ground probe the own footstep needs: ground within this distance below.
// SPEC: MOVEMENT_2009.md §10.2 (cshell+0x1fe1f0 finds ground within 150 units below)
pub const OWN_GROUND_PROBE: f32 = 150.0;

// --------------------------------------------------------------------------------------------- ROM tables

/// The surface table: `[Surface<n>]` rows with their id and the two right / left footstep files.
// EVIDENCE: cshell+0x27e660 (row parser: the id key at +0x20, the right-foot keys 1..2 into +0x550, the left-foot
// keys 1..2 into +0x5d0, 0x40 bytes each), cshell+0x27e260 (row by id: the first row of the list whose id matches)
// OBSERVED: ROM table SURFACE.TXT (entry of ATTRIBUTES.rez), rows `[Surface<n>]`; the id key and the two footstep key
// prefixes (each followed by 1 or 2) are matched by sha256(key)[:16] / length, as dc-audio matches its keys
pub const SURFACE_ENTRY: &str = "ATTRIBUTES\\SURFACE.TXT";
const KEY_SURFACE_ID: dc_audio::KeyId = dc_audio::KeyId::new("12156e921bdb6085", 2);
const KEY_RIGHT_FOOT: dc_audio::KeyId = dc_audio::KeyId::new("00ddf0a92a685578", 9);
const KEY_LEFT_FOOT: dc_audio::KeyId = dc_audio::KeyId::new("d4db8b3c41d3a5c6", 9);

/// Footstep files of one surface row: `[right 1, right 2]`, `[left 1, left 2]` (empty string: none).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SurfaceSounds {
    pub right: [String; 2],
    pub left: [String; 2],
}

/// Surface rows by id (the first row of an id wins).
#[derive(Debug, Clone, Default)]
pub struct SurfaceTable {
    pub rows: BTreeMap<u16, SurfaceSounds>,
}

fn text(v: Option<&String>) -> String {
    v.map(|t| t.trim().trim_matches('"').trim().to_string())
        .unwrap_or_default()
}

fn number(v: Option<&String>) -> Option<f32> {
    let t = v?.trim();
    let t = t.strip_suffix(['f', 'F']).unwrap_or(t);
    t.parse::<f32>().ok().filter(|x| x.is_finite())
}

impl SurfaceTable {
    // EVIDENCE: cshell+0x27e260 (first match in the row list); PLAUSIBLE that the list keeps the file order
    pub fn from_doc(doc: &ButeDoc) -> SurfaceTable {
        let mut rows = BTreeMap::new();
        for (name, sec) in doc.iter() {
            if !name.starts_with("Surface") {
                continue;
            }
            let mut id = None;
            let mut row = SurfaceSounds::default();
            for (k, v) in sec.iter() {
                if KEY_SURFACE_ID.matches(k) {
                    id = number(Some(v));
                    continue;
                }
                if !k.is_char_boundary(k.len().saturating_sub(1)) {
                    continue;
                }
                let (head, tail) = k.split_at(k.len().saturating_sub(1));
                let i = match tail {
                    "1" => 0,
                    "2" => 1,
                    _ => continue,
                };
                if KEY_RIGHT_FOOT.matches(head) {
                    row.right[i] = text(Some(v));
                } else if KEY_LEFT_FOOT.matches(head) {
                    row.left[i] = text(Some(v));
                }
            }
            if let Some(id) = id {
                rows.entry(id.max(0.0) as u16).or_insert(row);
            }
        }
        SurfaceTable { rows }
    }

    /// The footstep file of a surface: the left set when `left`, else the right set; `pick` 0 or 1.
    // EVIDENCE: cshell+0x27f170 (left foot -> +0x5d0, else +0x550; index from the random pick 0..1); an unknown id
    // falls back to the default row (cshell+0x27e260 logs and returns the default, cshell+0x27e1e0; PLAUSIBLY id 0)
    pub fn footstep(&self, surface: u16, left: bool, pick: usize) -> Option<&str> {
        let row = self.rows.get(&surface).or_else(|| self.rows.get(&0))?;
        let set = if left { &row.left } else { &row.right };
        let f = set[pick.min(1)].as_str();
        (!f.is_empty()).then_some(f)
    }
}

/// The weapon table entry and the client attribute entry.
// OBSERVED: ROM table keys WEAPONS.TXT (entry of ATTRIBUTES.rez), CLIENTBUTES.TXT [Character] CharJumpSound1 / 2
pub const WEAPONS_ENTRY: &str = "ATTRIBUTES\\WEAPONS.TXT";
pub const CLIENT_ENTRY: &str = "ATTRIBUTES\\CLIENTBUTES.TXT";
const SECTION_CHARACTER: &str = "Character";
const KEY_JUMP_SOUND: &str = "CharJumpSound";
/// The section and key prefix of the shooter's two hit-feedback sounds (`<prefix>1`, `<prefix>2`).
// EVIDENCE: cshell+0x201c00 (reads the two keys of the client attribute file through cshell+0x1b2c60, whose section
// is the string at VA 0x374eb864, then plays one of them, picked by cshell+0x19eb40(0, 1), as a local sound:
// cshell+0x1b1e20 with priority 6, flags 0, volume 100, class 0)
// OBSERVED: ROM table CLIENTBUTES.TXT section [Guns], keys DamageFXSound1 / DamageFXSound2
const SECTION_GUNS: &str = "Guns";
const KEY_HIT_FEEDBACK_SOUND: &str = "DamageFXSound";
/// Priority of the hit-feedback sound.
// EVIDENCE: cshell+0x201c00 (the call of cshell+0x1b1e20 with priority 6)
pub const HIT_FEEDBACK_PRIO: u8 = 6;

/// The sound files and radii of one weapon row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WeaponSounds {
    /// Sounds 1..=14 (index `k - 1`), empty where the row names none.
    pub files: Vec<String>,
    /// Outer radius per sound 1..=14.
    pub radius: Vec<f32>,
    /// The silenced fire sounds of silencer levels 1..=10 (`SilencedFireSnd<i>`), empty where the row names none.
    pub silenced: Vec<String>,
}

impl WeaponSounds {
    /// The file and outer radius of weapon sound `k`.
    pub fn sound(&self, k: u8) -> Option<(&str, f32)> {
        let i = usize::from(k).checked_sub(1)?;
        let f = self.files.get(i)?.as_str();
        (!f.is_empty()).then(|| {
            (
                f,
                self.radius.get(i).copied().unwrap_or(WEAPON_OUTER_DEFAULT),
            )
        })
    }
}

/// Weapon sounds of every `[Weapon<n>]` row. Sound numbers: 1..3 the three reload sounds (reload radius), 4 select,
/// 5 deselect (select radius), 6 fire, 7 dry fire, 8 alternate fire (fire radius), 9 the silenced fire sound of a
/// weapon mod (the row's `SilencedFireSnd<level>`, kept apart in `silenced`), 10..14 the five misc sounds (2000).
// EVIDENCE: cshell+0x1a2b50 (sound switch), cshell+0x17bd20 (row parser: the reload keys 1..3 into +0x98, the misc
// keys 1..5 into +0xb4, select +0xa4, deselect +0xa8, fire +0x90, dry fire +0x94, alternate fire +0x8c; radii +0x1f8
// fire, +0x230 reload, +0x234 select; disassembly cshell+0x17bfd7..0x17c038, 0x17ca64, 0x17cb06, 0x17cd51..0x17cdcc)
// OBSERVED: ROM table keys WEAPONS.TXT [Weapon<n>] ReloadSnd1..3, SelectSnd, DeselectSnd, FireSnd, DryFireSnd,
// Alt_FireSnd, MiscSound1..5, FireSndRadius, ReloadSndRadius, SelectSndRadius
pub fn weapon_sounds(doc: &ButeDoc) -> BTreeMap<u16, WeaponSounds> {
    let mut out = BTreeMap::new();
    for (name, sec) in doc.iter() {
        let Some(n) = name
            .strip_prefix("Weapon")
            .filter(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|r| r.parse::<u16>().ok())
        else {
            continue;
        };
        let t = |k: &str| text(sec.get(k));
        let r = |k: &str, row_default: f32| number(sec.get(k)).unwrap_or(row_default);
        // CLI-1761: a row without a radius key keeps the 2009 row's start value (the bomb row names no reload radius:
        // its keypad sounds, relayed to the others during a plant, carried 300 units, not 2000)
        let (fire, reload, select) = (
            r("FireSndRadius", ROW_FIRE_RADIUS),
            r("ReloadSndRadius", ROW_RELOAD_RADIUS),
            r("SelectSndRadius", ROW_SELECT_RADIUS),
        );
        let mut files = Vec::with_capacity(14);
        let mut radius = Vec::with_capacity(14);
        for i in 1..=3 {
            files.push(t(&format!("ReloadSnd{i}")));
            radius.push(reload);
        }
        for (k, rr) in [
            ("SelectSnd", select),
            ("DeselectSnd", select),
            ("FireSnd", fire),
            ("DryFireSnd", fire),
            ("Alt_FireSnd", fire),
        ] {
            files.push(t(k));
            radius.push(rr);
        }
        files.push(String::new());
        radius.push(fire);
        for i in 1..=5 {
            files.push(t(&format!("MiscSound{i}")));
            radius.push(WEAPON_OUTER_DEFAULT);
        }
        // DISASM: cshell+0x17c985 (`SilencedFireSnd<i>`, i = 1..10, into row +0x64 + 4 x (i - 1))
        let silenced = (1..=10)
            .map(|i| t(&format!("SilencedFireSnd{i}")))
            .collect();
        out.insert(
            n,
            WeaponSounds {
                files,
                radius,
                silenced,
            },
        );
    }
    out
}

/// Sound number of the dry fire.
const SOUND_DRY_FIRE: u8 = 7;
/// A dry-fire sound of at least this length is no click (seconds).
const DRY_FIRE_NO_CLICK_S: f32 = 0.6;
/// A dry-fire sound this short or longer can stand in for one that is no click (seconds).
const DRY_FIRE_CLICK_MIN_S: f32 = 0.1;

/// Dry fire that sounds like dry fire: a row whose dry-fire file is no click takes the first row's click instead, and
/// plays nothing when there is none. `seconds` reads a file's length, `None` when it cannot be read (the row stays).
// OBSERVED: 2026-11-03, the ROM's rows name 78 distinct dry-fire files: 44 last under 0.6 s (clicks of 0.06 .. 0.28 s
// among them) and 34 last 1.87 s at full level, with the falling shape of a shot (counts only)
// DC-DEFAULT: CLI-1844 (proposed; USER REPORT 2026-11-03 "when you run out of bullets you gun still makes shooting
// sounds"); the row's own file is what cshell+0x17bd20 reads, and what the 2009 client did with these is not known
pub fn click_dry_fire(
    weapons: &mut BTreeMap<u16, WeaponSounds>,
    seconds: &mut dyn FnMut(&str) -> Option<f32>,
) {
    let i = usize::from(SOUND_DRY_FIRE) - 1;
    let mut known: BTreeMap<String, Option<f32>> = BTreeMap::new();
    let mut length = |file: &str| {
        *known
            .entry(file.to_string())
            .or_insert_with(|| seconds(file))
    };
    let mut click = None;
    for w in weapons.values() {
        let Some(file) = w.files.get(i).filter(|f| !f.is_empty()) else {
            continue;
        };
        if length(file).is_some_and(|s| (DRY_FIRE_CLICK_MIN_S..DRY_FIRE_NO_CLICK_S).contains(&s)) {
            click = Some(file.clone());
            break;
        }
    }
    for w in weapons.values_mut() {
        let Some(file) = w.files.get_mut(i).filter(|f| !f.is_empty()) else {
            continue;
        };
        if length(file).is_some_and(|s| s >= DRY_FIRE_NO_CLICK_S) {
            *file = click.clone().unwrap_or_default();
        }
    }
}

/// Footstep keys of the clips of one character model: per combined clip index, its length and its footstep keys
/// (time, foot argument: `Some(true)` the right foot, `Some(false)` the left one, `None` no argument).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClipSteps {
    pub clips: Vec<ClipKeys>,
}

/// A footstep key: time (ms) and foot argument.
pub type StepKey = (u32, Option<bool>);
/// A clip's length (ms) and its footstep keys.
pub type ClipKeys = (u32, Vec<StepKey>);

/// The footstep key word: `sha256(upper-case word)[:8]` and its length.
// DISASM: cshell+0x2057a0 (key name compared case-insensitively with the footstep key string, VA 0x374efe38)
// OBSERVED: dc-dserver chars.rs KEY_FOOTSTEP (the same key, found by hash over the ROM clip markers)
pub const KEY_FOOTSTEP: ([u8; 8], usize) = ([0x18, 0x71, 0x8b, 0x81, 0x25, 0x4f, 0x1d, 0xea], 12);

fn name_key(w: &[u8]) -> ([u8; 8], usize) {
    let h = dc_rom::sha256(&w.to_ascii_uppercase());
    let mut k = [0u8; 8];
    k.copy_from_slice(&h[..8]);
    (k, w.len())
}

/// The foot a footstep key names: argument "1" -> the right foot, any other argument -> the left foot, no argument
/// -> `None` (the client alternates).
// DISASM: cshell+0x205864..0x2058af (with an argument: foot flag := stricmp(arg, "1") != 0 (VA 0x374eeaa0 = "1");
// without: the flag toggles); EVIDENCE: cshell+0x204f60 (flag set = the left foot socket), cshell+0x27f170 (flag set =
// the left-foot files)
pub fn footstep_foot(command: &[u8]) -> Option<Option<bool>> {
    let mut words = command
        .split(|b| b.is_ascii_whitespace())
        .filter(|w| !w.is_empty());
    let first = words.next()?;
    if name_key(first) != KEY_FOOTSTEP {
        return None;
    }
    Some(words.next().map(|a| a == b"1"))
}

impl ClipSteps {
    /// The footstep keys of a model and its children (combined clip order).
    // SPEC: UDP_SESSION_2009_B.md §4.3 (tracker clip indices count the body's clips, then the children's)
    pub fn of(set: &dc_render::AnimSet) -> ClipSteps {
        let clips = (0..set.clip_count())
            .map(|i| {
                let len = set.clip_length_ms(i).unwrap_or(0);
                let keys = set
                    .clip(i)
                    .map(|c| {
                        c.markers
                            .iter()
                            .filter_map(|m| footstep_foot(&m.command.0).map(|f| (m.time_ms, f)))
                            .collect()
                    })
                    .unwrap_or_default();
                (len, keys)
            })
            .collect();
        ClipSteps { clips }
    }
}

// --------------------------------------------------------------------------------------------- frame inputs

/// One sound-related thing the session saw since the last frame.
#[derive(Debug, Clone, PartialEq)]
pub enum SoundCue {
    /// A sound record start; `file` is the file table name; `at_load` when it came with the world-load creates.
    Server {
        start: WireSound,
        file: Option<String>,
        at_load: bool,
    },
    /// A tracked sound moves.
    Move { track: u16, pos: [f32; 3] },
    /// A tracked sound ends (removal record or stop bit).
    Stop { track: u16 },
    /// A type 0x19 relay (`own`: the record names the own client id).
    Player {
        k: u8,
        weapon: u16,
        player: u32,
        pos: [f32; 3],
        own: bool,
        /// The throw form's trailing bytes (`b` the team that hears the callout, `c` the voice template).
        extra: Option<(u8, u8)>,
        /// The own team as the roster has it when the record arrived.
        own_team: Option<u8>,
    },
    /// A type 0x06 shot effect. `clear`: the record's `b` (flag bits it clears, [`fire_sound_wanted`]); `own`: the
    /// record's `c` names the own client id ([`shot_names_own_client`]), or the shooter is the own character.
    Shot {
        shooter: u16,
        weapon: u16,
        /// The record's ammunition, its trace end and the mod numbers of the shooter weapon's three sockets.
        ammo: u16,
        from: [f32; 3],
        to: [f32; 3],
        mods: [u8; 3],
        clear: u16,
        own: bool,
    },
    /// The own weapon's animation key sound `k` (the client sent `UP_WEAPON_SOUND` for it).
    OwnWeapon { k: u8, weapon: u16 },
    /// The own weapon fired one shot; `silencer` = the muzzle mod's silencer level when it picks the silenced
    /// sound (weapon sound 9: the row's `SilencedFireSnd<level>`), else weapon sound 6.
    // EVIDENCE: cshell+0x2330e0 (9 with 0 < SilencerLevel < 10, else 6), cshell+0x2368dd (file of level `l`: row
    // +0x64 + 4 x (l - 1), DISASM cshell+0x17c985 `SilencedFireSnd<i>` for i = 1..10 stored there)
    OwnFire { weapon: u16, silencer: Option<u8> },
    /// A 0x6d whose killer is the own player, not in the groin zone ([`cues::own_kill_voice`]): the own kill voice,
    /// auto-cast local row [`cues::LOCAL_ROW_ENEMY_KILL`].
    OwnKill,
    /// A shot record of the own shot that keeps the hit-feedback flag ([`hit_feedback_wanted`]): `first` is the
    /// object the shot hit (the victim's character for a character hit), `ammo` the record's ammunition id.
    OwnHit { first: u16, ammo: u16 },
    /// An `S2C_PRELOAD` list of kind `sub` with the names of its ids (`None`: not in the file table).
    Preload {
        sub: u8,
        files: Vec<(u16, Option<String>)>,
    },
    /// An interface voice line of the HUD (the kill banners, `hud::banners`): `file` from the interface sound table
    /// (`None` without a ROM), `what` the line's name for the log.
    Interface {
        file: Option<String>,
        what: &'static str,
    },
    /// A server's music order (S2C 0xd9): runs on the music clock like the menu's cues.
    Music(crate::music::MusicOrder),
    /// A shell music cue raised by the match (the first in-world update: [`MusicCue::MatchStart`]).
    MusicCue(MusicCue),
    /// A character's spawn notes: `on` (sub 0x1f) ends its step mute; the end note (sub 0x25) leaves the steps as
    /// they are (it closes the spawn window +0x53a only).
    // EVIDENCE: cshell+0x206260 (sub 0x1f clears +0x539 and sets +0x53a; sub 0x25 clears +0x53a)
    SpawnMark { obj: u16, on: bool },
    /// A character's step mute: `on` from its death note (sub 0x1d), off from sub 0x1e (or the spawn note).
    // EVIDENCE: cshell+0x206260 (sub 0x1d sets +0x539, sub 0x1e clears it), cshell+0x202870 (no step while set)
    StepMute { obj: u16, on: bool },
    /// The session's ladder volumes ([`cues::ladder_sounds`]), whenever their number changes.
    Ladders(Vec<LadderSound>),
}

/// The own player as the sounds need it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OwnAudio<'a> {
    /// The own character object's models and trackers as the server streams them (DSV-23).
    pub look: Option<&'a RemoteLook>,
    pub obj: Option<u16>,
    /// Feet (body position minus the half height).
    pub feet: [f32; 3],
    /// The mover's surface type (the 0x64 trailer's).
    pub surface: u8,
    pub control: u32,
    pub on_ground: bool,
    /// In the air since a jump take-off, until the landing (the sim's whole-flight state): no ground probe, no own
    /// steps (CLI-631).
    pub airborne: bool,
    /// The sprint gauge drains (gauge mode 1): the sprint breath plays (CLI-632).
    pub sprinting: bool,
    pub alive: bool,
    /// The jump take-off happened this frame.
    pub jumped: bool,
    /// The body's speed (units per second): the view-bob step cycle needs a moving body.
    pub speed: f32,
    /// The player state of the server's 0x67 (2, 3, 5, 6: dead or waiting; no own step).
    pub life_state: Option<u8>,
}

/// Everything a sound frame reads.
pub struct AudioFrame<'a> {
    pub now_ms: u32,
    /// Eye position and view.
    pub eye: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub cues: &'a [SoundCue],
    pub remotes: &'a [RemoteBody],
    pub own: Option<OwnAudio<'a>>,
    /// The world (surface rays); `None`: surface 0.
    pub world: Option<&'a crate::world::LoadedWorld>,
}

// --------------------------------------------------------------------------------------------- the log

/// Why a sound started.
#[derive(Debug, Clone, PartialEq)]
pub enum Trigger {
    /// A footstep key of a character clip (`own`: the local player's character).
    Footstep {
        obj: u16,
        own: bool,
        left: bool,
        surface: u16,
        clip: usize,
        key_ms: u32,
    },
    /// The own jump take-off: its step and its local jump sound.
    JumpStep,
    JumpSound,
    /// A tracked sound of the world-load group.
    LoadSound {
        track: u16,
    },
    /// A tracked sound started during play.
    TrackedSound {
        track: u16,
    },
    /// A one-shot sound record.
    OneShot,
    /// A type 0x19 relay: weapon sound `k` of `player`, or the jump sound (k 15).
    SoundRelay {
        k: u8,
        player: u32,
    },
    /// The fire sound of another player's shot effect.
    ShotFire {
        shooter: u16,
    },
    /// The own weapon's key sound and its shots.
    OwnWeapon {
        k: u8,
    },
    OwnFire,
    /// The shooter's hit feedback for its own shot that hit `first`.
    OwnHit {
        first: u16,
    },
    /// A HUD interface voice line (kill banners).
    Interface {
        what: &'static str,
    },
    /// An own step of the view-bob cycle (CLI-527): the foot of the crossing and the surface.
    OwnStep {
        left: bool,
        surface: u16,
    },
    /// Another player's shot with a silencer mod: weapon sound 9 at this level.
    ShotSilenced {
        shooter: u16,
        level: u8,
    },
    /// Another player's bullet passed the listener.
    Flyby {
        shooter: u16,
    },
    /// The own kill voice (auto-cast local row 1).
    OwnKill,
    /// The own sprint breath (auto-cast local row 0, CLI-632).
    Breath,
    /// The team callout of a throw relay: auto-cast row `row` of `player`.
    Callout {
        player: u32,
        row: u8,
    },
}

impl Trigger {
    pub fn label(&self) -> &'static str {
        match self {
            Trigger::Footstep { own: true, .. } => "footstep_own",
            Trigger::Footstep { .. } => "footstep",
            Trigger::JumpStep => "jump_step",
            Trigger::JumpSound => "jump_sound",
            Trigger::LoadSound { .. } => "load_sound",
            Trigger::TrackedSound { .. } => "tracked_sound",
            Trigger::OneShot => "one_shot",
            Trigger::SoundRelay { .. } => "relay_sound",
            Trigger::ShotFire { .. } => "shot_fire",
            Trigger::OwnWeapon { .. } => "own_weapon",
            Trigger::OwnFire => "own_fire",
            Trigger::OwnHit { .. } => "own_hit",
            Trigger::Interface { .. } => "interface",
            Trigger::OwnStep { .. } => "footstep_own",
            Trigger::ShotSilenced { .. } => "shot_silenced",
            Trigger::Flyby { .. } => "flyby",
            Trigger::Callout { .. } => "throw_callout",
            Trigger::OwnKill => "own_kill",
            Trigger::Breath => "breath",
        }
    }
}

/// One sound start.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundLogEntry {
    pub t_ms: u32,
    pub trigger: Trigger,
    /// The resource path (kept in memory for tests; never written to files).
    pub file: Option<String>,
    /// `sha256(upper-case path)[:16]` (hex).
    pub file_hash: Option<String>,
    pub pos: Option<[f32; 3]>,
    pub kind: VoiceKind,
    pub inner: f32,
    pub outer: f32,
    pub volume: u8,
    pub prio: u8,
    pub class: u8,
    pub looped: bool,
    /// Distance from the listener at the start.
    pub distance: Option<f32>,
    /// The mixer's gain at the start (`None`: no voice: the file was missing or the voice list was full).
    pub gain: Option<f32>,
    /// The voice's `(left, right)` pan gains at the start (CLI-200; the centre level on both ears for a local voice).
    pub pan: Option<(f32, f32)>,
}

impl SoundLogEntry {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "t_ms": self.t_ms,
            "trigger": self.trigger.label(),
            "detail": format!("{:?}", self.trigger),
            "file_hash": self.file_hash,
            "pos": self.pos,
            "kind": format!("{:?}", self.kind),
            "inner": self.inner,
            "outer": self.outer,
            "volume": self.volume,
            "prio": self.prio,
            "class": self.class,
            "loop": self.looped,
            "distance": self.distance,
            "gain": self.gain,
            "pan": self.pan.map(|(l, r)| [l, r]),
        })
    }
}

/// Counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SoundStats {
    pub starts: u64,
    pub voices: u64,
    pub missing_files: u64,
    pub footsteps_own: u64,
    pub footsteps_remote: u64,
    pub load_sounds: u64,
    pub tracked: u64,
    pub relay_sounds: u64,
    pub shot_fires: u64,
    /// Shot effects that played no fire sound (own records, or `b` cleared the fire-sound flags).
    pub silent_shots: u64,
    pub own_weapon: u64,
    pub preloaded: u64,
    pub stops: u64,
    /// Type 0x19 records this client does not play (the throw form).
    pub unplayed_relays: u64,
    /// Hit feedbacks of the own shots (one per own character-hit record).
    pub own_hits: u64,
    /// Fire voices stopped because the same emitter's weapon fired again ([`cuts_previous`]).
    pub fire_cuts: u64,
    /// The server's echoes of the own jump (k 15 relays naming the own client), not played (CLI-681).
    pub own_jump_echoes: u64,
    /// Footsteps not played: the character's step mute (death note) was on.
    pub step_muted: u64,
    pub ladder_muted: u64,
    /// Other players' shots with a silencer mod (played with sound 9, or silent outside levels 1..9).
    pub silenced_shots: u64,
    pub flybys: u64,
    pub callouts: u64,
    pub own_kills: u64,
}

/// Hash of a resource path for logs.
pub fn path_hash(path: &str) -> String {
    let p = path.trim().replace('/', "\\").to_ascii_uppercase();
    dc_rom::sha256_hex(p.as_bytes())[..16].to_string()
}

// --------------------------------------------------------------------------------------------- the director

/// One play decision before it reaches the mixer.
struct Play {
    file: Option<String>,
    trigger: Trigger,
    pos: Option<[f32; 3]>,
    kind: VoiceKind,
    inner: f32,
    outer: f32,
    volume: u8,
    prio: u8,
    class: u8,
    looped: bool,
    reverb: bool,
    track: Option<u16>,
}

/// The cursor of one tracker of one character: the clip and the clip time last looked at.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Cursor {
    clip: usize,
    t: f32,
}

/// Where the mix goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AudioOut {
    /// Rendered into a scratch buffer by the game clock (headless runs, `--audio null`).
    #[default]
    Null,
    /// The default output device (window runs; falls back to `Null` when no device opens or the client was built
    /// without the `audio` feature).
    Device,
}

/// The output side of a window run: device or null, and the two volumes of the 2009 options page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioConfig {
    pub out: AudioOut,
    /// Effects volume 0..=100 (the 2009 SFX master, applied in stage B and C for the master-scaled classes).
    pub effects_volume: u16,
    /// Music volume 0..=100.
    pub music_volume: u16,
    /// Seconds between two output status lines of a window run (0: only at the start and at the end).
    pub status_every_s: f64,
    /// Linear gain after the mixer on the device (1.0: the 2009 levels; device tests use a very low gain).
    pub out_gain: f32,
}

impl Default for AudioConfig {
    // DC-DEFAULT: CLI-96 (proposed) effects and music volume 100 (the 2009 options page defaults, SPEC AUDIO_2009
    // §11.1; the user's own profile is not read), a status line every 10 s
    fn default() -> Self {
        AudioConfig {
            out: AudioOut::Null,
            effects_volume: 100,
            music_volume: 100,
            status_every_s: 10.0,
            out_gain: 1.0,
        }
    }
}

enum Sink {
    Null(NullSink),
    #[cfg(feature = "audio")]
    Device(dc_audio::device::AudioDevice),
}

impl Sink {
    // one variant without the `audio` feature
    #[allow(irrefutable_let_patterns)]
    fn null_mut(&mut self) -> Option<&mut NullSink> {
        if let Sink::Null(n) = self {
            Some(n)
        } else {
            None
        }
    }
}

/// Plays and logs the sounds of the session.
pub struct SoundDirector {
    /// Shared with the device callback when a device is open (lock with [`SoundDirector::mixer`]).
    mixer: Arc<Mutex<Mixer>>,
    sink: Sink,
    /// The volumes and class gains in the mixer.
    pub settings: AudioSettings,
    vfs: Option<Arc<RezVfs>>,
    /// Decoded sound files, bounded in bytes; filled ahead by a decode thread in async mode (perf-hitch).
    cache: crate::sndcache::PcmCache,
    /// Weapons whose sound set was queued for decoding (async mode).
    prefetched_weapons: std::collections::HashSet<u16>,
    pub surfaces: SurfaceTable,
    pub weapons: BTreeMap<u16, WeaponSounds>,
    pub jump_sounds: [String; 2],
    /// The shooter's two hit-feedback sounds (empty where the table names none).
    pub hit_sounds: [String; 2],
    rigs: HashMap<String, Option<Arc<ClipSteps>>>,
    /// Footstep rigs read on a thread (async mode with the decode thread, perf-hitch).
    rig_worker: Option<RigWorker>,
    pub rig_stats: RigStats,
    cursors: HashMap<(u16, usize), Cursor>,
    /// The foot flag of each character FX (`true` = left), toggled by keys without an argument.
    feet: HashMap<u16, bool>,
    tracked: HashMap<u16, VoiceId>,
    /// The last fire voice per emitter and weapon; `fire_cut` off plays every fire sound to its end (the behaviour
    /// before CLI-201, kept for comparisons).
    fire_voices: FireVoices,
    fire_cut: bool,
    rng: u64,
    pub log: Vec<SoundLogEntry>,
    /// The JSONL log: a bounded writer thread, never file I/O on the frame thread (perf-hitch).
    log_file: Option<dc_botclient::logring::LogWriter>,
    pub stats: SoundStats,
    /// What the ROM tables gave (for reports).
    pub note: String,
    /// The background-music caller (front end, waiting room, loading, infection) and its decode thread.
    pub music: MusicDirector,
    /// The origin of the music clock: one monotonic clock for the menu and the match, so a fade never straddles two.
    music_epoch: std::time::Instant,
    /// Started voices for the `live` lines (bounded by [`LIVE_KEEP`]).
    live: VecDeque<LiveVoice>,
    /// Game time of the last `live` line and of the last match frame (`None`: no match frame yet).
    live_at: Option<u32>,
    last_now: Option<u32>,
    /// Music-clock time of the track now in the player (the last `play` event).
    music_play_at: Option<f64>,
    /// w2-audiocues: the silenced fire sounds per weapon, the mod rows, the team callouts, the fly-by record and the
    /// ammunition / impact rows the fly-by test reads.
    pub silenced: BTreeMap<u16, Vec<(String, f32)>>,
    pub mod_rows: BTreeMap<u16, cues::ModSound>,
    pub callouts: cues::TeamCallouts,
    pub flyby: Option<dc_audio::attrib::SoundRecord>,
    pub shot_rows: crate::fx::FxTables,
    /// Characters whose step mute is on (death note until the spawn note; their footsteps are muted).
    step_mutes: std::collections::HashSet<u16>,
    ladders: Vec<LadderSound>,
    bob: cues::BobSteps,
    /// Where the own footsteps come from (CLI-527).
    pub own_steps: OwnSteps,
    /// The last sprint breath: its voice (`None`: no voice was made) and start time (CLI-632).
    breath: Option<(Option<VoiceId>, u32)>,
    /// The fly-by voice at the 2009 shell's own position instead of the nearest point (CLI-529).
    pub flyby_shell_place: bool,
    /// The own mover's last surface other than 0 (the step player falls back to the FX's last surface).
    own_surface: u16,
    /// Spawn-note changes as they arrived: (game time, object, on), for reports and tests.
    pub spawn_log: Vec<(u32, u16, bool)>,
}

/// The source of the own footsteps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OwnSteps {
    /// The view-bob cycle of the local player (the 2009 client's first-person step source).
    #[default]
    ViewBob,
    /// The footstep keys of the own character's server-echoed clips (the behaviour before w2-audiocues, CLI-83).
    ServerEcho,
}

/// Footstep rig models waiting for the rig thread at most (perf-hitch; overflow: asked again on a later frame).
pub const RIG_PENDING: usize = 4;

/// Counters of the footstep rig reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RigStats {
    /// Read on the frame thread (inline mode).
    pub inline: u64,
    /// Handed to the rig thread, and handed back.
    pub queued: u64,
    pub built: u64,
    /// Lookups answered "not yet" (that body had no footsteps in that frame).
    pub deferred: u64,
    pub hwm_pending: usize,
}

fn rig_key(model: &str) -> String {
    model.trim().replace('/', "\\").to_ascii_uppercase()
}

fn read_rig(vfs: &RezVfs, model: &str) -> Option<Arc<ClipSteps>> {
    let f = dc_render::ModelFiles::load(vfs, model).ok()?;
    Some(Arc::new(ClipSteps::of(&f.anim_set())))
}

/// The rig thread: model paths in, footstep keys out (both channels hold at most [`RIG_PENDING`]).
struct RigWorker {
    tx: std::sync::mpsc::SyncSender<String>,
    rx: std::sync::mpsc::Receiver<(String, Option<Arc<ClipSteps>>)>,
    pending: std::collections::HashSet<String>,
}

impl RigWorker {
    fn start(vfs: Arc<RezVfs>) -> Option<RigWorker> {
        let (tx, req) = std::sync::mpsc::sync_channel::<String>(RIG_PENDING);
        let (done, rx) = std::sync::mpsc::sync_channel(RIG_PENDING);
        std::thread::Builder::new()
            .name("dc-sound-rig".into())
            .spawn(move || {
                while let Ok(model) =
                    dc_hangdump::crumb_wait!("sound rig: wait for a model", req.recv())
                {
                    let r = read_rig(&vfs, &model);
                    if dc_hangdump::crumb_wait!(
                        "sound rig: hand the keys back",
                        done.send((rig_key(&model), r))
                    )
                    .is_err()
                    {
                        return;
                    }
                }
            })
            .ok()?;
        Some(RigWorker {
            tx,
            rx,
            pending: std::collections::HashSet::new(),
        })
    }
}

/// Most log entries kept in memory.
pub const LOG_KEEP: usize = 100_000;
/// Voices kept for the `live` lines at most (the oldest once-voice is dropped first; then the oldest voice).
pub const LIVE_KEEP: usize = 256;
/// Game milliseconds between two `live` lines of a match.
// DC-DEFAULT: AUDIO-3 (staged) one `live` line per second of match time: the voices still playing (hash, loop,
// age, length) and the music player's state, so a gate can tell a stuck or unexpected loop from the log alone
pub const LIVE_EVERY_MS: u32 = 1000;

/// A started voice as the `live` lines report it.
#[derive(Debug, Clone)]
struct LiveVoice {
    id: VoiceId,
    t_ms: u32,
    dur_ms: u32,
    looped: bool,
    file_hash: Option<String>,
    trigger: &'static str,
}

impl SoundDirector {
    /// Reads the surface, weapon and client tables from the user's ROM (`None`: no files, every start is still
    /// decided and logged without a voice).
    pub fn open(rom: Option<&Path>) -> SoundDirector {
        let mut d = SoundDirector {
            mixer: Arc::new(Mutex::new(Mixer::new())),
            sink: Sink::Null(NullSink::default()),
            settings: AudioSettings::default(),
            vfs: None,
            cache: crate::sndcache::PcmCache::default(),
            prefetched_weapons: std::collections::HashSet::new(),
            surfaces: SurfaceTable::default(),
            weapons: BTreeMap::new(),
            jump_sounds: [String::new(), String::new()],
            hit_sounds: [String::new(), String::new()],
            rigs: HashMap::new(),
            rig_worker: None,
            rig_stats: RigStats::default(),
            cursors: HashMap::new(),
            feet: HashMap::new(),
            tracked: HashMap::new(),
            fire_voices: FireVoices::default(),
            fire_cut: true,
            // DC-DEFAULT: CLI-68 (proposed) the random picks (footstep variant, jump sound) come from a fixed-seed
            // xorshift (the 2009 client uses its C library generator, cshell+0x29eb40)
            rng: 0x9e37_79b9_7f4a_7c15,
            log: Vec::new(),
            log_file: None,
            stats: SoundStats::default(),
            note: "no ROM".into(),
            music: MusicDirector::none(),
            music_epoch: std::time::Instant::now(),
            live: VecDeque::new(),
            live_at: None,
            last_now: None,
            music_play_at: None,
            silenced: BTreeMap::new(),
            mod_rows: BTreeMap::new(),
            callouts: cues::TeamCallouts::default(),
            flyby: None,
            shot_rows: crate::fx::FxTables::default(),
            step_mutes: std::collections::HashSet::new(),
            ladders: Vec::new(),
            bob: cues::BobSteps::default(),
            own_steps: OwnSteps::default(),
            breath: None,
            flyby_shell_place: false,
            own_surface: 0,
            spawn_log: Vec::new(),
        };
        // The shell's class gains (class 0 = 0.75, the profile default) and the options-page volumes: without them
        // every class-0 voice ran at stage B 1.27 instead of 0.95 (the footstep gain 1.113 at ~400 units).
        // SPEC: AUDIO_2009.md §4.2 (cshell+0x1e95a0 assigns classes 0..2), §11.2 (cshell+0x1ecef0 defaults)
        d.settings.apply(&mut d.mixer());
        if let Some(rom) = rom {
            let vfs = RezVfs::new(rom);
            let doc = |e: &str| ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", e);
            if let Some(s) = doc(SURFACE_ENTRY) {
                d.surfaces = SurfaceTable::from_doc(&s);
            }
            if let Some(w) = doc(WEAPONS_ENTRY) {
                d.weapons = weapon_sounds(&w);
                click_dry_fire(&mut d.weapons, &mut |file| {
                    let bytes = crate::modassets::read_entry(&vfs, file).ok()?;
                    let info = dc_audio::decode::wav_info(&bytes).ok()?;
                    (info.rate > 0).then(|| info.frames as f32 / info.rate as f32)
                });
                d.silenced = cues::silenced_sounds(&w);
                d.mod_rows = cues::mod_sounds(&w);
                d.shot_rows = crate::fx::FxTables::from_docs(
                    Some(&w),
                    doc(crate::fx::FX_ENTRY).as_ref(),
                    None,
                );
            }
            d.callouts = cues::TeamCallouts::open(&vfs).unwrap_or_default();
            d.flyby = dc_audio::attrib::SoundTable::from_vfs(&vfs).and_then(|t| {
                t.records
                    .into_iter()
                    .find(|r| cues::FLYBY_RECORD.matches(&r.name.to_ascii_lowercase()))
            });
            if let Some(c) = doc(CLIENT_ENTRY) {
                d.music = MusicDirector::from_rom(rom, &c);
                if let Some(sec) = c.get(SECTION_CHARACTER) {
                    for (i, slot) in d.jump_sounds.iter_mut().enumerate() {
                        *slot = text(sec.get(&format!("{KEY_JUMP_SOUND}{}", i + 1)));
                    }
                }
                if let Some(sec) = c.get(SECTION_GUNS) {
                    for (i, slot) in d.hit_sounds.iter_mut().enumerate() {
                        *slot = text(sec.get(&format!("{KEY_HIT_FEEDBACK_SOUND}{}", i + 1)));
                    }
                }
            }
            d.note = format!(
                "surfaces {} weapons {} jump sounds {} hit sounds {} silenced rows {} callout rows {} flyby {}",
                d.surfaces.rows.len(),
                d.weapons.len(),
                d.jump_sounds.iter().filter(|s| !s.is_empty()).count(),
                d.hit_sounds.iter().filter(|s| !s.is_empty()).count(),
                d.silenced.len(),
                d.callouts.rows.len(),
                d.flyby.is_some()
            );
            d.vfs = Some(Arc::new(vfs));
        }
        d
    }

    /// Decodes sound files ahead on a thread (`true`; the window's default) or only when first played, inline
    /// (`false`, the default: headless runs and tests). Either way a sound plays the same samples. Switching on
    /// queues the footstep, jump and hit-feedback files of the ROM tables.
    // DC-DEFAULT: CLI-440 (proposed), see DC_DEFAULTS.md
    pub fn set_async_decode(&mut self, on: bool) {
        self.cache.set_async(on);
        if !on {
            self.prefetched_weapons.clear();
            return;
        }
        let mut files: Vec<String> = Vec::new();
        for r in self.surfaces.rows.values() {
            files.extend(r.right.iter().chain(r.left.iter()).cloned());
        }
        files.extend(self.jump_sounds.iter().cloned());
        files.extend(self.hit_sounds.iter().cloned());
        for f in files {
            self.cache.prefetch(&f);
        }
        self.cache.pump(self.vfs.as_ref());
    }

    /// Queues every sound of `weapon` once (async mode): the first sound of a weapon still decodes when it plays,
    /// its fire / reload / select sounds are ready by the time they follow.
    fn prefetch_weapon(&mut self, weapon: u16) {
        if !self.cache.is_async() || !self.prefetched_weapons.insert(weapon) {
            return;
        }
        if let Some(w) = self.weapons.get(&crate::camo::base_weapon(weapon)) {
            for f in &w.files {
                self.cache.prefetch(f);
            }
        }
    }

    /// The decoded-sound cache's counters (reports, tests).
    pub fn cache_stats(&self) -> crate::sndcache::CacheStats {
        self.cache.stats
    }

    /// Waits for every queued decode (tests and reports; BLOCKS).
    pub fn settle_decodes(&mut self) {
        self.cache.settle(self.vfs.as_ref());
    }

    /// The mixer (locked; the device callback waits while it is held, so keep the guard short).
    pub fn mixer(&self) -> MutexGuard<'_, Mixer> {
        dc_hangdump::crumb_wait!("sound: lock the mixer", self.mixer.lock())
            .unwrap_or_else(|e| e.into_inner())
    }

    /// The stereo law of the 3D voices (CLI-200; [`dc_audio::mixer::PanLaw::HARD`] is the law before it).
    pub fn set_pan_law(&mut self, law: dc_audio::mixer::PanLaw) {
        self.mixer().set_pan_law(law);
    }

    /// Whether a fire sound stops the same emitter's previous one ([`cuts_previous`]; on by default).
    pub fn set_fire_cut(&mut self, on: bool) {
        self.fire_cut = on;
    }

    /// Applies volumes and class gains (the options page and profile values, SPEC AUDIO_2009 §11).
    pub fn apply_settings(&mut self, s: AudioSettings) {
        s.apply(&mut self.mixer());
        self.settings = s;
    }

    /// Applies the output config's volumes and opens its output. Returns one note line for the log.
    pub fn configure(&mut self, c: &AudioConfig) -> String {
        let mut s = self.settings.clone();
        s.effects_volume = i32::from(c.effects_volume.min(100));
        s.music_volume = i32::from(c.music_volume.min(100));
        self.apply_settings(s);
        let note = self.open_output(c.out, c.out_gain);
        format!(
            "{note}; effects volume {} music volume {}",
            self.settings.effective_effects(),
            self.settings.effective_music()
        )
    }

    /// Switches the output. `Device` opens the default output device (the mixer then renders at the device's rate
    /// from its callback); if that fails, or without the `audio` feature, the null backend stays and the note says
    /// why. Returns one note line for the log.
    pub fn open_output(&mut self, out: AudioOut, gain: f32) -> String {
        self.close_output();
        match out {
            AudioOut::Null => "audio out: null backend".into(),
            #[cfg(feature = "audio")]
            AudioOut::Device => {
                let opts = dc_audio::device::DeviceOptions {
                    gain: gain.clamp(0.0, 1.0),
                    ..Default::default()
                };
                match dc_audio::device::AudioDevice::open(self.mixer.clone(), &opts) {
                    Ok(dev) => {
                        let converted = if dev.rate == dc_audio::OUTPUT_RATE {
                            ""
                        } else {
                            ", voices converted from their file rates to the device rate"
                        };
                        let note = format!(
                            "audio out: device {} (mixer at {} Hz{converted})",
                            dev.describe(),
                            self.mixer().output_rate(),
                        );
                        self.sink = Sink::Device(dev);
                        note
                    }
                    Err(e) => format!("audio out: no device ({e}); null backend"),
                }
            }
            #[cfg(not(feature = "audio"))]
            AudioOut::Device => {
                let _ = gain;
                "audio out: built without the `audio` feature; null backend".into()
            }
        }
    }

    /// True while an output device is open.
    pub fn has_device(&self) -> bool {
        !matches!(self.sink, Sink::Null(_))
    }

    /// The output's status line: the device statistics, or the null backend's rendered time.
    pub fn output_line(&self) -> String {
        match &self.sink {
            Sink::Null(n) => {
                let rate = self.mixer().output_rate();
                format!(
                    "audio out: null backend | {:.2} s rendered at {rate} Hz",
                    n.frames as f64 / f64::from(rate)
                )
            }
            #[cfg(feature = "audio")]
            Sink::Device(d) => d.line(),
        }
    }

    /// Stops the device (no callback runs after this returns) and goes back to the null backend at 44.1 kHz.
    /// Returns the last status line of the closed output.
    pub fn close_output(&mut self) -> String {
        #[cfg(feature = "audio")]
        if matches!(self.sink, Sink::Device(_)) {
            if let Sink::Device(dev) =
                std::mem::replace(&mut self.sink, Sink::Null(NullSink::default()))
            {
                let last = dev.close();
                self.mixer().set_output_rate(dc_audio::OUTPUT_RATE);
                return last;
            }
        }
        self.output_line()
    }

    /// Window minimised: both masters read 0 until restored (SPEC AUDIO_2009 §2 "Window minimise").
    pub fn set_minimised(&mut self, m: bool) {
        self.mixer().set_minimised(m);
    }

    /// Also writes every start as one JSON line (hashes only, no ROM names).
    pub fn log_to(&mut self, path: &Path) -> std::io::Result<()> {
        self.log_file = Some(dc_botclient::logring::LogWriter::create(path)?);
        Ok(())
    }

    fn pick(&mut self) -> usize {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x >> 33) as usize & 1
    }

    fn pcm(&mut self, path: &str) -> Option<Arc<Pcm>> {
        self.cache.get(self.vfs.as_ref(), path)
    }

    /// The footstep keys of a body model, read once.
    /// The footstep keys of a body model, read once. In async mode (the decode thread is on) the model is read
    /// on the rig thread and the body has no footsteps until it arrives (a body model read plus its animation set
    /// took 131 ms on the frame thread, perf-hitch); at most [`RIG_PENDING`] models wait, a further one is asked
    /// for again on a later frame.
    // DC-DEFAULT: CLI-440 (proposed), see DC_DEFAULTS.md
    fn rig(&mut self, model: &str) -> Option<Arc<ClipSteps>> {
        let key = rig_key(model);
        self.absorb_rigs();
        if let Some(r) = self.rigs.get(&key) {
            return r.clone();
        }
        if self.cache.is_async() {
            if let Some(vfs) = self.vfs.clone() {
                if self.rig_worker.is_none() {
                    self.rig_worker = RigWorker::start(vfs);
                }
                if let Some(w) = self.rig_worker.as_mut() {
                    self.rig_stats.deferred += 1;
                    if !w.pending.contains(&key)
                        && w.pending.len() < RIG_PENDING
                        && w.tx.try_send(model.to_string()).is_ok()
                    {
                        w.pending.insert(key);
                        self.rig_stats.queued += 1;
                        self.rig_stats.hwm_pending =
                            self.rig_stats.hwm_pending.max(w.pending.len());
                    }
                    return None;
                }
            }
        }
        let r = self.vfs.as_ref().and_then(|vfs| read_rig(vfs, model));
        self.rig_stats.inline += 1;
        crate::watchdog::busy();
        self.rigs.insert(key, r.clone());
        r
    }

    fn absorb_rigs(&mut self) {
        let Some(w) = self.rig_worker.as_mut() else {
            return;
        };
        while let Ok((k, r)) = w.rx.try_recv() {
            w.pending.remove(&k);
            self.rig_stats.built += 1;
            self.rigs.insert(k, r);
        }
    }

    /// Installs a decoded sound for a resource path directly (tests, synthetic sounds; the ROM is not read for it).
    pub fn set_sound(&mut self, path: &str, pcm: Pcm) {
        self.cache.set(path, pcm);
    }

    /// Installs the footstep keys of a model path directly (tests, synthetic rigs).
    pub fn set_rig(&mut self, model: &str, steps: ClipSteps) {
        let key = model.trim().replace('/', "\\").to_ascii_uppercase();
        self.rigs.insert(key, Some(Arc::new(steps)));
    }

    fn start(&mut self, now: u32, listener: [f32; 3], p: Play) -> Option<VoiceId> {
        let pcm = p.file.as_deref().and_then(|f| self.pcm(f));
        // the event stream's sound length (gate T2)
        let dur_ms = pcm
            .as_ref()
            .map(|c| (c.frames() as u64 * 1000 / c.rate.max(1) as u64) as u32);
        let distance = p.pos.map(|q| {
            ((q[0] - listener[0]).powi(2)
                + (q[1] - listener[1]).powi(2)
                + (q[2] - listener[2]).powi(2))
            .sqrt()
        });
        let mut gain = None;
        let mut pan = None;
        let mut voice = None;
        if let Some(pcm) = pcm {
            let dur_s = pcm.duration_s();
            let req = PlayRequest {
                pcm,
                kind: p.kind,
                looped: p.looped,
                want_handle: p.track.is_some(),
                volume: p.volume.min(100),
                inner_radius: p.inner,
                outer_radius: p.outer,
                position: p.pos.unwrap_or(listener),
                class: p.class,
                reverb: p.reverb,
            };
            if let Some(t) = p.track {
                if let Some(old) = self.tracked.remove(&t) {
                    self.mixer().release(old);
                }
            }
            let played = {
                let mut m = self.mixer();
                m.play(req).map(|id| (id, m.gain_of(id), m.pan_of(id)))
            };
            if let Ok((id, g, pn)) = played {
                gain = g;
                pan = pn;
                voice = Some(id);
                self.note_live(LiveVoice {
                    id,
                    t_ms: now,
                    dur_ms: (dur_s * 1000.0).round() as u32,
                    looped: p.looped,
                    file_hash: p.file.as_deref().map(path_hash),
                    trigger: p.trigger.label(),
                });
                self.stats.voices += 1;
                if let Some(t) = p.track {
                    self.tracked.insert(t, id);
                }
            }
        } else {
            self.stats.missing_files += 1;
        }
        self.stats.starts += 1;
        let e = SoundLogEntry {
            t_ms: now,
            trigger: p.trigger,
            file_hash: p.file.as_deref().map(path_hash),
            file: p.file,
            pos: p.pos,
            kind: p.kind,
            inner: p.inner,
            outer: p.outer,
            volume: p.volume,
            prio: p.prio,
            class: p.class,
            looped: p.looped,
            distance,
            gain,
            pan,
        };
        if let Some(f) = self.log_file.as_ref() {
            f.push(e.json().to_string());
        }
        crate::events::sound_start(
            voice.map(|v| v.0),
            e.file_hash.as_deref(),
            e.trigger.label(),
            e.looped,
            dur_ms,
        );
        if self.log.len() < LOG_KEEP {
            self.log.push(e);
        }
        voice
    }

    /// The match's world is gone (the match window hands the director back to the menu): every effect voice stops,
    /// whatever started it (the tracked world sounds and the ambient loops of the world-load group, untracked loops,
    /// one-shots still playing such as the 12.7 s Quarantine intro voice, the HUD's interface voices); the music
    /// player is not a voice and keeps its track (the menu's screen-state cue decides the music next). Returns the
    /// voices stopped.
    // EVIDENCE: engine+0x1a2f0 (the server link's teardown) -> engine+0x19f20 -> engine+0x19d00 (the world teardown:
    // after the client objects are removed, engine+0xd8eb0 calls the stop of EVERY active sound instance, not only the
    // looping or object-bound ones); engine+0x1a770 (a world load runs the same teardown first); the music streams are
    // not instances (AUDIO_2009.md §8.1)
    // DC-DEFAULT: CLI-1135 (proposed, playtest report "background noise kept playing even after the game ended and we
    // went back to lobby"; AMENDED by CLI-1890, owner report 2026-11-05 "when I left the quarantine game the audio still
    // played until I fully left the lobby") the world's sounds end with the world
    pub fn end_world(&mut self) -> usize {
        self.tracked.clear();
        self.live.clear();
        let stopped = self.mixer().release_all();
        self.stats.stops += stopped as u64;
        // the per-match tables: the next match restarts its clock at 0 and reuses object ids, so nothing keyed by an
        // object or stamped with the old clock may carry over (a step mute of a body that died as the match ended
        // silenced its id's steps in the next match; stale animation cursors fired or swallowed the first keys)
        // DC-DEFAULT: CLI-1541 (proposed) the director forgets its match-keyed state when the world ends
        self.cursors.clear();
        self.feet.clear();
        self.step_mutes.clear();
        self.ladders.clear();
        self.fire_voices = FireVoices::default();
        self.bob = cues::BobSteps::default();
        self.breath = None;
        self.own_surface = 0;
        stopped
    }

    /// Starts a weapon sound; a fire sound ([`cuts_previous`]) first stops the emitter's previous one of the same
    /// weapon and is stored in its place.
    // EVIDENCE: cshell+0x1a2b50 (kill before the play, store after it only when the play returned a voice)
    fn start_weapon(
        &mut self,
        now: u32,
        listener: [f32; 3],
        cut: Option<(Emitter, u16)>,
        p: Play,
    ) -> Option<VoiceId> {
        let cut = cut.filter(|_| self.fire_cut);
        if let Some((e, w)) = cut {
            if let Some(old) = self.fire_voices.take(e, w) {
                let live = {
                    let mut m = self.mixer();
                    let live = !m.is_finished(old);
                    m.release(old);
                    live
                };
                if live {
                    self.stats.fire_cuts += 1;
                }
            }
        }
        let id = self.start(now, listener, p);
        if let (Some((e, w)), Some(id)) = (cut, id) {
            self.fire_voices.keep(e, w, id);
        }
        id
    }

    /// A footstep of a character at `feet` on `surface`.
    // EVIDENCE: cshell+0x202870 (file by surface and foot, outer 1200, priority 4 / 6, flags POSITIONAL |
    // CLIENT_ONLY (cshell+0x27dbc0 `0x102` for a plain path) plus LOCAL_IF_OWN for the own object, volume from the
    // character (+0x68 x 100), class 0); AUDIO_2009.md §3.2 (LOCAL_IF_OWN -> a local voice)
    // DC-DEFAULT: CLI-71 (proposed) footstep volume 100 (the character's volume factor +0x68 is not traced)
    #[allow(clippy::too_many_arguments)]
    fn footstep(
        &mut self,
        now: u32,
        listener: [f32; 3],
        obj: u16,
        own: bool,
        left: bool,
        surface: u16,
        feet: [f32; 3],
        clip: usize,
        key_ms: u32,
        trigger_override: Option<Trigger>,
    ) {
        // the step player's gates for every character: the step mute (+0x539: set by the death note, cleared by the
        // spawn note; not the spawn window +0x53a) mutes it, a ladder volume may switch it off or change its surface
        // EVIDENCE: cshell+0x202870 (an FX of a roster character (+0x40) returns while its +0x539 is set, under the
        // check cshell+0x1a0470; surface 200: the ladder's class-0x0b FX gates and overrides, [`cues::ladder_surface`]),
        // cshell+0x206260 (sub 0x1d sets +0x539; subs 0x1e / 0x1f clear it)
        // DC-DEFAULT: CLI-527 (proposed) the check cshell+0x1a0470 (VA 0x3759e22c +4 not 4 or 5; meaning not traced) is
        // taken as passed
        if self.step_mutes.contains(&obj) {
            self.stats.step_muted += 1;
            return;
        }
        let surface = if surface == cues::SURFACE_LADDER {
            match cues::ladder_surface(&self.ladders, feet) {
                Some(s) => s,
                None => {
                    self.stats.ladder_muted += 1;
                    return;
                }
            }
        } else {
            surface
        };
        let pick = self.pick();
        let file = self
            .surfaces
            .footstep(surface, left, pick)
            .map(str::to_string);
        if file.is_none() && self.vfs.is_some() {
            return;
        }
        let trigger = match trigger_override {
            Some(Trigger::OwnStep { left, .. }) => Trigger::OwnStep { left, surface },
            Some(t) => t,
            None => Trigger::Footstep {
                obj,
                own,
                left,
                surface,
                clip,
                key_ms,
            },
        };
        if own {
            self.stats.footsteps_own += 1;
        } else {
            self.stats.footsteps_remote += 1;
        }
        self.start(
            now,
            listener,
            Play {
                file,
                trigger,
                pos: Some(feet),
                kind: if own {
                    VoiceKind::Local
                } else {
                    VoiceKind::Positional
                },
                inner: FOOTSTEP_OUTER * INNER_SHARE,
                outer: FOOTSTEP_OUTER,
                volume: 100,
                prio: if own {
                    FOOTSTEP_PRIO_OWN
                } else {
                    FOOTSTEP_PRIO_REMOTE
                },
                class: CLASS_DEFAULT,
                looped: false,
                reverb: true,
                track: None,
            },
        );
    }

    /// The footstep keys a character's trackers passed since the last frame: `(tracker, clip, key time, foot)`.
    /// A clip change restarts the cursor without a key; a time that went back by less than half the clip is
    /// stream jitter and is held; a jump forward of more than half the clip is skipped.
    // DC-DEFAULT: CLI-69 (proposed) keys fire when the extrapolated tracker time passes them (the 2009 engine fires
    // model keys as its own animation playback passes them; the stream resets the time 20 times a second)
    fn passed_keys(
        &mut self,
        obj: u16,
        look: &RemoteLook,
        steps: &ClipSteps,
    ) -> Vec<(usize, usize, u32, Option<bool>)> {
        let mut out = Vec::new();
        for (ti, tr) in look.trackers.iter().enumerate() {
            let Some((len, keys)) = steps.clips.get(tr.clip) else {
                continue;
            };
            if *len == 0 {
                continue;
            }
            let lenf = *len as f32;
            let t = tr.time_ms.rem_euclid(lenf);
            let prev = self.cursors.insert((obj, ti), Cursor { clip: tr.clip, t });
            let Some(prev) = prev.filter(|c| c.clip == tr.clip) else {
                continue;
            };
            let delta = (t - prev.t).rem_euclid(lenf);
            if delta == 0.0 {
                continue;
            }
            if delta > lenf * 0.5 {
                // held: jitter backwards (keep the older cursor) or a skip forward (take the new one)
                if (prev.t - t).rem_euclid(lenf) < lenf * 0.5 {
                    self.cursors.insert((obj, ti), prev);
                }
                continue;
            }
            for &(k, foot) in keys {
                let kt = k as f32;
                let after = (kt - prev.t).rem_euclid(lenf);
                if after > 0.0 && after <= delta || (after == 0.0 && delta >= lenf) {
                    out.push((ti, tr.clip, k, foot));
                }
            }
        }
        out
    }

    fn foot_of(&mut self, obj: u16, foot: Option<bool>) -> bool {
        let f = self.feet.entry(obj).or_insert(false);
        match foot {
            Some(right) => *f = !right,
            None => *f = !*f,
        }
        *f
    }

    fn surface_below(
        world: Option<&crate::world::LoadedWorld>,
        centre: [f32; 3],
        depth: f32,
    ) -> u16 {
        let Some(w) = world else {
            return 0;
        };
        let from = V3::from_array(centre);
        let to = V3::new(centre[0], centre[1] - depth, centre[2]);
        w.collision
            .ray(from, to)
            .map(|h| w.surface_type(h.model, h.poly))
            .unwrap_or(0)
    }

    /// One frame: the cues, the footstep keys, the listener, the mixer.
    pub fn frame(&mut self, f: &AudioFrame<'_>) {
        let now = f.now_ms;
        let listener = f.eye;
        let fwd = crate::arms::forward(f.yaw, f.pitch);
        self.mixer()
            .set_camera(Some(Listener::from_camera(f.eye, fwd, [0.0, 1.0, 0.0])));
        for c in f.cues {
            self.cue(now, listener, c);
        }
        // remote characters
        for r in f.remotes {
            let Some(look) = r.look.as_ref().filter(|l| l.visible) else {
                continue;
            };
            let Some(model) = look.models.first().map(|m| m.model.clone()) else {
                continue;
            };
            let Some(steps) = self.rig(&model) else {
                continue;
            };
            let keys = self.passed_keys(r.obj, look, &steps);
            if keys.is_empty() {
                continue;
            }
            let feet = [r.pos[0], r.pos[1] - r.half[1], r.pos[2]];
            let surface = Self::surface_below(f.world, r.pos, r.half[1] + REMOTE_SURFACE_DEPTH);
            for (_, clip, k, foot) in keys {
                let left = self.foot_of(r.obj, foot);
                self.footstep(
                    now, listener, r.obj, false, left, surface, feet, clip, k, None,
                );
            }
        }
        // the own character
        if let Some(o) = f.own {
            self.own_frame(now, listener, &o, f.world);
        }
        // the mixer: distance law against the listener, finished voices released; the voices advance in the device
        // callback, or here by the game clock on the null backend (nobody hears that mix)
        // the decode thread: finished files into the cache, this frame's queued names to the thread
        self.cache.pump(self.vfs.as_ref());
        let mixer = self.mixer.clone();
        let mut m = dc_hangdump::crumb_wait!("sound: lock the mixer", mixer.lock())
            .unwrap_or_else(|e| e.into_inner());
        // the music runs on its own clock (the game clock restarts per match; a fade must not see that). The
        // effects update steps the music player too, so the player sits out that step: one clock per fade.
        let bgm = std::mem::take(&mut m.bgm);
        m.update(now as f64 / 1000.0);
        m.bgm = bgm;
        let music_now = self.music_epoch.elapsed().as_secs_f64();
        self.music.poll(&mut m, music_now);
        m.bgm.update(music_now);
        if let Some(n) = self.sink.null_mut() {
            n.pump(&mut m, now);
        }
        drop(m);
        self.write_music("match", now);
        self.live_line(now);
    }

    fn note_live(&mut self, v: LiveVoice) {
        if self.live.len() >= LIVE_KEEP {
            match self.live.iter().position(|l| !l.looped) {
                Some(i) => {
                    self.live.remove(i);
                }
                None => {
                    self.live.pop_front();
                }
            }
        }
        self.live.push_back(v);
    }

    /// The music player's state as a JSON value (`null`: nothing plays).
    fn music_state(&self, m: &Mixer, music_now: f64) -> serde_json::Value {
        match m.bgm.status() {
            None => serde_json::Value::Null,
            Some(st) => serde_json::json!({
                "file_hash": m.bgm.current_key().map(path_hash),
                "loop": st.looped,
                "pos_ms": (st.pos_s * 1000.0).round() as u64,
                "dur_ms": (st.dur_s * 1000.0).round() as u64,
                "age_ms": self.music_play_at.map(|t| ((music_now - t).max(0.0) * 1000.0).round() as u64),
                "gain": st.gain,
                "crossfading": st.crossfading,
            }),
        }
    }

    /// Writes the music's pending events as `music` lines of the sound log (phase `menu` or `match`; `t_ms` is the
    /// match clock of the last match frame, 0 before the first).
    fn write_music(&mut self, phase: &str, now: u32) {
        if self.music.events.is_empty() {
            return;
        }
        let music_now = self.music_now_s();
        let events = self.music.take_events();
        let mixer = self.mixer.clone();
        let m = mixer.lock().unwrap_or_else(|e| e.into_inner());
        for e in events {
            if e.op == "play" {
                self.music_play_at = Some(music_now);
            }
            let line = serde_json::json!({
                "t_ms": now,
                "music_ms": (music_now * 1000.0).round() as u64,
                "trigger": "music",
                "kind": "Music",
                "phase": phase,
                "op": e.op,
                "cue": e.label,
                "file_hash": serde_json::Value::Null,
                "music_file_hash": e.key.as_deref().map(path_hash),
                "loop": e.looped,
                "fade_s": e.fade_s,
                "dur_ms": e.dur_s.map(|d| (d * 1000.0).round() as u64),
                "music": self.music_state(&m, music_now),
            });
            if let Some(f) = self.log_file.as_ref() {
                f.push(line.to_string());
            }
        }
    }

    /// Every [`LIVE_EVERY_MS`] of match time: one `live` line with the voices still playing and the music state.
    fn live_line(&mut self, now: u32) {
        // the game clock restarts per match: a new schedule. The records stay: this runs after the frame's starts, so
        // a clear here forgot the new match's first voices (the previous match's are gone from the mixer and leave
        // the list below as finished)
        // DC-DEFAULT: CLI-1890 (proposed) the voice records survive a clock that goes back
        if self.last_now.is_some_and(|l| now < l) {
            self.live_at = None;
        }
        self.last_now = Some(now);
        if self
            .live_at
            .is_some_and(|t| now < t.saturating_add(LIVE_EVERY_MS))
        {
            return;
        }
        self.live_at = Some(now);
        let music_now = self.music_now_s();
        let mixer = self.mixer.clone();
        let m = mixer.lock().unwrap_or_else(|e| e.into_inner());
        self.live.retain(|v| !m.is_finished(v.id));
        let voices: Vec<serde_json::Value> = self
            .live
            .iter()
            .map(|v| {
                serde_json::json!({
                    "file_hash": v.file_hash,
                    "trigger": v.trigger,
                    "loop": v.looped,
                    "age_ms": now.saturating_sub(v.t_ms),
                    "dur_ms": v.dur_ms,
                })
            })
            .collect();
        let line = serde_json::json!({
            "t_ms": now,
            "music_ms": (music_now * 1000.0).round() as u64,
            "trigger": "live",
            "kind": "Live",
            "file_hash": serde_json::Value::Null,
            "voices": voices,
            "music": self.music_state(&m, music_now),
        });
        if let Some(f) = self.log_file.as_ref() {
            f.push(line.to_string());
        }
    }

    /// Seconds on the music clock.
    pub fn music_now_s(&self) -> f64 {
        self.music_epoch.elapsed().as_secs_f64()
    }

    /// Runs a background-music cue (screen state change, loading, infection start / end). Never decodes here.
    // SPEC: AUDIO_2009.md §8.2
    /// CLI-1599: the BGM style; true when it changed (the menu then cues its screen again).
    pub fn set_bgm_classic(&mut self, on: bool) -> bool {
        self.music.set_classic(on)
    }

    pub fn music_cue(&mut self, cue: MusicCue) {
        let now_s = self.music_now_s();
        let mixer = self.mixer.clone();
        let mut m = dc_hangdump::crumb_wait!("sound: lock the mixer", mixer.lock())
            .unwrap_or_else(|e| e.into_inner());
        self.music.cue(&mut m, cue, now_s);
        drop(m);
        let t = self.last_now.unwrap_or(0);
        self.write_music("menu", t);
    }

    /// The menu's interface sounds ([`crate::menu::Menu::take_sound_cues`]): local voices, logged like the match's
    /// (`t_ms`: the match clock of the last match frame, 0 before the first, as the `music` lines).
    // EVIDENCE: cshell+0x1b20c0 (the interface play helper; the front end has no listener: local voices)
    pub fn menu_cues(&mut self, cues: &[SoundCue]) {
        let now = self.last_now.unwrap_or(0);
        for c in cues {
            match c {
                SoundCue::Interface { file, what } => self.interface(now, [0.0; 3], file.clone(), what),
                // CRATE-7: the case reel's won gun, fired and racked with its own sounds
                SoundCue::OwnWeapon { k, weapon } => {
                    if let Some((file, _)) = self.weapon_file(*weapon, *k) {
                        self.stats.own_weapon += 1;
                        let cut = cuts_previous(*k).then_some((Emitter::Own, *weapon));
                        self.own_weapon(now, [0.0; 3], file, Trigger::OwnWeapon { k: *k }, cut);
                    }
                }
                _ => {}
            }
        }
    }

    /// One music step outside a match (the menu, the loading screen): installs a finished decode, advances the
    /// fade, and renders the mix on the null backend. A match frame does the same inside [`SoundDirector::frame`].
    pub fn music_tick(&mut self) {
        let now_s = self.music_now_s();
        let mixer = self.mixer.clone();
        let mut m = dc_hangdump::crumb_wait!("sound: lock the mixer", mixer.lock())
            .unwrap_or_else(|e| e.into_inner());
        self.music.poll(&mut m, now_s);
        m.bgm.update(now_s);
        if let Some(n) = self.sink.null_mut() {
            n.pump(&mut m, (now_s * 1000.0) as u32);
        }
        // r10 (CLI-922): the front end has no game frame, so nothing released the finished interface voices: after
        // 255 clicks / hover changes the active list was full and every later cue (the start countdown too) was
        // silent until a match frame ran the mixer update
        m.release_finished();
        drop(m);
        let t = self.last_now.unwrap_or(0);
        self.write_music("menu", t);
    }

    /// The own character's footsteps (only while the run or sprint bit is held, alive and over ground) and the
    /// jump take-off (a step and the local jump sound).
    // EVIDENCE: cshell+0x202870 (own object: returns unless the mover's control word has 0x400200), cshell+0x204f60
    // (own object: surface from the mover, returns without ground), cshell+0x1ff0b0 (take-off: one of the two
    // character jump sounds as a local voice, priority 6, unless the surface is 0xca; then the foot flag toggles and a
    // step plays)
    fn own_frame(
        &mut self,
        now: u32,
        listener: [f32; 3],
        o: &OwnAudio<'_>,
        world: Option<&crate::world::LoadedWorld>,
    ) {
        let obj = o.obj.unwrap_or(0xFFFF);
        let run = o.control & (dc_sim::player::bits::RUN | dc_sim::player::bits::SPRINT) != 0;
        if o.surface != 0 {
            self.own_surface = u16::from(o.surface);
        }
        // the own steps of the view-bob cycle (m38): a step at each quarter / three-quarter crossing of the phase while
        // the body moves, played by the step player with its own gates (run or sprint, a live state, the spawn note,
        // the ladder); the surface is the mover's, else the last one
        // EVIDENCE: cshell+0x2b0050 / cshell+0x2b07d0 (the step source of the local player, needs the local character
        // FX mover +0x304: here the own object), cshell+0x202870 (gates; own priority 6, LOCAL_IF_OWN)
        // DC-DEFAULT: CLI-527 (proposed) the view-bob cycle replaces the server-echoed clip keys as the own step source
        // (CLI-83 amended; [`OwnSteps::ServerEcho`] keeps the old source)
        // the step player's ground test: standing, else (not airborne after a jump) ground within 150 units below;
        // without ground the own step is not played (the user's round-5 bug: steps went on through a jump)
        // EVIDENCE: cshell+0x204f60 (own object: cshell+0x1fe1f0 answers 0 -> return before the sound),
        // cshell+0x1fe1f0 (mover +0x30 / +0x60 -> ground; +0x80 set -> none; else the 150-unit ray)
        // DC-DEFAULT: CLI-631 (proposed) "airborne after a jump" is the whole flight (the sim's `jumping`), not the
        // one-step mover +0x80 of SIM-20: the reference stack plays no step anywhere in a jump
        let ground = o.on_ground
            || (!o.airborne
                && world.is_some_and(|w| {
                    let from = V3::from_array(o.feet) + V3::new(0.0, 1.0, 0.0);
                    w.collision
                        .ray(from, from - V3::new(0.0, OWN_GROUND_PROBE, 0.0))
                        .is_some()
                }));
        let bob = self.bob.frame(now, o.control, o.speed);
        if self.own_steps == OwnSteps::ViewBob
            && o.obj.is_some()
            && ground
            && cues::own_step_allowed(o.control, o.life_state)
        {
            for left in bob {
                let surface = self.own_surface;
                self.footstep(
                    now,
                    listener,
                    obj,
                    true,
                    left,
                    surface,
                    o.feet,
                    0,
                    0,
                    Some(Trigger::OwnStep { left, surface }),
                );
            }
        }
        if o.alive && o.sprinting {
            self.breath(now, listener);
        }
        let keys = match (o.look, o.obj) {
            _ if self.own_steps != OwnSteps::ServerEcho => Vec::new(),
            (Some(look), Some(obj)) => match look.models.first().map(|m| m.model.clone()) {
                Some(model) => match self.rig(&model) {
                    Some(steps) => self.passed_keys(obj, look, &steps),
                    None => Vec::new(),
                },
                None => Vec::new(),
            },
            _ => Vec::new(),
        };
        if o.alive && run && ground {
            for (_, clip, k, foot) in keys {
                let left = self.foot_of(obj, foot);
                self.footstep(
                    now,
                    listener,
                    obj,
                    true,
                    left,
                    o.surface as u16,
                    o.feet,
                    clip,
                    k,
                    None,
                );
            }
        }
        if o.jumped && o.alive {
            // EVIDENCE: cshell+0x1ff0b0 (the take-off plays one of the two character jump sounds as a local voice
            // unless the surface is 200); the server echoes the jump as a type 0x19 k 15 relay, which the 2009 client
            // played for the own player as well (cshell+0x2cbd30 checks the own client id for k 1..14 only); ours
            // drops that echo, so one jump is one jump sound (CLI-681, amends CLI-83)
            if o.surface != SURFACE_NO_JUMP_SOUND {
                let i = self.pick();
                let file = Some(self.jump_sounds[i].clone()).filter(|s| !s.is_empty());
                if file.is_some() || self.vfs.is_none() {
                    self.start(
                        now,
                        listener,
                        Play {
                            file,
                            trigger: Trigger::JumpSound,
                            pos: Some(o.feet),
                            kind: VoiceKind::Local,
                            inner: 0.0,
                            outer: 0.0,
                            volume: 100,
                            prio: FOOTSTEP_PRIO_OWN,
                            class: CLASS_DEFAULT,
                            looped: false,
                            reverb: true,
                            track: None,
                        },
                    );
                }
            }
            if run && ground {
                let left = self.foot_of(obj, None);
                self.footstep(
                    now,
                    listener,
                    obj,
                    true,
                    left,
                    o.surface as u16,
                    o.feet,
                    0,
                    0,
                    Some(Trigger::JumpStep),
                );
            }
        }
    }

    fn cue(&mut self, now: u32, listener: [f32; 3], c: &SoundCue) {
        if let SoundCue::Player { weapon, .. }
        | SoundCue::Shot { weapon, .. }
        | SoundCue::OwnWeapon { weapon, .. }
        | SoundCue::OwnFire { weapon, .. } = c
        {
            self.prefetch_weapon(*weapon);
        }
        match c {
            SoundCue::Server {
                start,
                file,
                at_load,
            } => self.server_sound(now, listener, start, file.clone(), *at_load),
            SoundCue::Move { track, pos } => {
                if let Some(&id) = self.tracked.get(track) {
                    self.mixer().set_voice_position(id, *pos);
                }
            }
            SoundCue::Stop { track } => {
                if let Some(id) = self.tracked.remove(track) {
                    self.mixer().release(id);
                    self.stats.stops += 1;
                }
            }
            SoundCue::Player {
                k,
                weapon,
                player,
                pos,
                own,
                extra,
                own_team,
            } => {
                if *k == dc_botclient::fx::SOUND_THROW {
                    self.callout(now, listener, *player, *weapon, *pos, *extra, *own_team);
                } else {
                    self.relay_sound(now, listener, *k, *weapon, *player, *pos, *own);
                }
            }
            SoundCue::Shot {
                shooter,
                weapon,
                ammo,
                from,
                to,
                mods,
                clear,
                own,
            } => {
                // EVIDENCE: cshell+0x2c6580 (another shooter's effect: weapon sound 6, or the silenced sound of the mod
                // in the first socket, at the record position); the record's `b` gates the call (DISASM
                // cshell+0x2c8eeb..0x2c8ef6, [`fire_sound_wanted`]) and the own client skips its own records
                if *own || !fire_sound_wanted(*clear) {
                    self.stats.silent_shots += 1;
                } else {
                    self.shot_fire(now, listener, *shooter, *weapon, *from, mods[0]);
                }
                // the fly-by test follows the fire sound for every foreign shot (DISASM cshell+0x2c8efb..0x2c8f16)
                if !*own {
                    self.flyby(now, listener, *shooter, *weapon, *ammo, *from, *to);
                }
            }
            SoundCue::SpawnMark { obj, on } => {
                self.spawn_log.push((now, *obj, *on));
                // CLI-680: the spawn note ends the step mute; the spawn window itself does not mute the steps (the
                // user's round-9 bug: no own step for the 3.5 s spawn window after every respawn)
                if *on {
                    self.step_mutes.remove(obj);
                }
            }
            SoundCue::StepMute { obj, on } => {
                if *on {
                    self.step_mutes.insert(*obj);
                } else {
                    self.step_mutes.remove(obj);
                }
            }
            SoundCue::Ladders(l) => self.ladders = l.clone(),
            SoundCue::OwnWeapon { k, weapon } => {
                // EVIDENCE: cshell+0x237180 / cshell+0x1a2b50 with the last argument 1 (cshell+0x1b1e20: a local
                // voice, priority 6, flags 0x100, class 1)
                if let Some((file, _)) = self.weapon_file(*weapon, *k) {
                    self.stats.own_weapon += 1;
                    let cut = cuts_previous(*k).then_some((Emitter::Own, *weapon));
                    self.own_weapon(now, listener, file, Trigger::OwnWeapon { k: *k }, cut);
                }
            }
            SoundCue::OwnFire { weapon, silencer } => {
                // EVIDENCE: cshell+0x237180 (the fire state: weapon sound 6, or the mod's silenced sound, local)
                let file = match silencer {
                    Some(l) => self.silenced_file(*weapon, *l),
                    None => self.weapon_file(*weapon, SOUND_FIRE),
                };
                if let Some((file, _)) = file {
                    self.stats.own_weapon += 1;
                    self.own_weapon(
                        now,
                        listener,
                        file,
                        Trigger::OwnFire,
                        Some((Emitter::Own, *weapon)),
                    );
                }
            }
            SoundCue::OwnHit { first, ammo } => self.own_hit(now, listener, *first, *ammo),
            SoundCue::OwnKill => self.own_kill(now, listener),
            SoundCue::Interface { file, what } => self.interface(now, listener, file.clone(), what),
            SoundCue::Music(order) => {
                let now_s = self.music_now_s();
                let mixer = self.mixer.clone();
                let mut m = dc_hangdump::crumb_wait!("sound: lock the mixer", mixer.lock())
                    .unwrap_or_else(|e| e.into_inner());
                self.music.order(&mut m, order, now_s);
            }
            SoundCue::MusicCue(c) => {
                let now_s = self.music_now_s();
                let mixer = self.mixer.clone();
                let mut m = mixer.lock().unwrap_or_else(|e| e.into_inner());
                self.music.cue(&mut m, *c, now_s);
            }
            SoundCue::Preload { sub, files } => {
                // SPEC: UDP_SESSION_2009_A.md §4.7 (sub 5: file references handed to the sound layer, PLAUSIBLY
                // sounds); DC-DEFAULT: CLI-70 (proposed) the wave files of any list kind are decoded ahead
                // perf-hitch: in async mode the files are queued for the decode thread (counted as preloaded when
                // queued) instead of decoded inside this frame
                let _ = sub;
                for name in files.iter().filter_map(|f| f.1.as_deref()) {
                    if !name.to_ascii_uppercase().ends_with(".WAV") {
                        continue;
                    }
                    let ok = if self.cache.is_async() {
                        self.cache.prefetch(name) || self.cache.contains(name)
                    } else {
                        self.pcm(name).is_some()
                    };
                    if ok {
                        self.stats.preloaded += 1;
                    }
                }
            }
        }
    }

    /// A HUD interface voice line: a local voice without reverb at full volume.
    // EVIDENCE: cshell+0x1b20c0 (the interface play helper: the path, flags = the caller's | 0x2000 (no reverb),
    // the rest of the request zero, the sound class of a named class table entry)
    // DC-DEFAULT: CLI-177 (staged) class 0, priority 0, volume 100 (the named class entry is not traced)
    fn interface(
        &mut self,
        now: u32,
        listener: [f32; 3],
        file: Option<String>,
        what: &'static str,
    ) {
        self.start(
            now,
            listener,
            Play {
                file,
                trigger: Trigger::Interface { what },
                pos: None,
                kind: VoiceKind::Local,
                inner: 0.0,
                outer: 0.0,
                volume: 100,
                prio: 0,
                class: CLASS_DEFAULT,
                looped: false,
                reverb: false,
                track: None,
            },
        );
    }

    /// The row's silenced fire sound of silencer level `level` (1..9) and its radius.
    fn silenced_file(&self, weapon: u16, level: u8) -> Option<(String, f32)> {
        let w = self.weapons.get(&crate::camo::base_weapon(weapon))?;
        let i = usize::from(level).checked_sub(1)?;
        let f = w.silenced.get(i).filter(|f| !f.is_empty())?;
        Some((
            f.clone(),
            w.radius.get(5).copied().unwrap_or(WEAPON_OUTER_DEFAULT),
        ))
    }

    /// Another player's fire sound: weapon sound 6, or with a silencer mod in the first socket the row's silenced
    /// sound 9 (not cut by the next shot), or nothing ([`cues::remote_shot_sound`]).
    // EVIDENCE: cshell+0x2c6580, cshell+0x1a2b50 (case 9: positional at the silenced radius, priority 5, flags 0x100,
    // volume 100, class 1, no fire-voice cut)
    fn shot_fire(
        &mut self,
        now: u32,
        listener: [f32; 3],
        shooter: u16,
        weapon: u16,
        from: [f32; 3],
        socket0: u8,
    ) {
        let m = self.mod_rows.get(&u16::from(socket0)).copied();
        match cues::remote_shot_sound(m.as_ref()) {
            cues::ShotSound::Fire => {
                if let Some((file, outer)) = self.weapon_file(weapon, SOUND_FIRE) {
                    self.stats.shot_fires += 1;
                    self.weapon_at(
                        now,
                        listener,
                        file,
                        outer,
                        Some(from),
                        Trigger::ShotFire { shooter },
                        Some((Emitter::Object(shooter), weapon)),
                    );
                }
            }
            cues::ShotSound::Silenced(level) => {
                self.stats.silenced_shots += 1;
                let base = crate::camo::base_weapon(weapon);
                let file = cues::silenced_file(&self.silenced, base, level).or_else(|| {
                    self.vfs
                        .is_none()
                        .then(|| (String::new(), WEAPON_OUTER_DEFAULT))
                });
                if let Some((file, outer)) = file {
                    self.weapon_at(
                        now,
                        listener,
                        file,
                        outer,
                        Some(from),
                        Trigger::ShotSilenced { shooter, level },
                        None,
                    );
                }
            }
            cues::ShotSound::Silent => self.stats.silenced_shots += 1,
        }
    }

    /// The bullet fly-by of another player's shot: a bullet ammunition (damage type 2, `Type` 0) whose line passes
    /// the listener within [`cues::FLYBY_RADIUS`], unless the listener is near the shot's end; the sound attribute
    /// record's files, radii and kind, priority 0, volume 100, class 0.
    // EVIDENCE: cshell+0x2c73a0 (the test; cshell+0x27dd70(pos, name, -1, 0, 0, 100, 1.0, -1, 0): outer and inner from
    // the record (-1), priority 0, the record's flags), cshell+0x27dbc0 (the record: play chance, path pick, radii,
    // volume 100 when the caller passes 100)
    #[allow(clippy::too_many_arguments)]
    fn flyby(
        &mut self,
        now: u32,
        listener: [f32; 3],
        shooter: u16,
        weapon: u16,
        ammo: u16,
        from: [f32; 3],
        to: [f32; 3],
    ) {
        let rom = self.vfs.is_some();
        if rom && !self.weapons.contains_key(&crate::camo::base_weapon(weapon)) {
            return;
        }
        let row = self.shot_rows.ammo.get(&ammo);
        let (bullet, impact) = match row {
            Some(a) => (
                a.damage_type == cues::FLYBY_DAMAGE_TYPE && a.ammo_type == cues::FLYBY_AMMO_TYPE,
                a.impact
                    .as_ref()
                    .and_then(|n| self.shot_rows.impacts.get(n))
                    .map(|i| i.sound_radius),
            ),
            // without a ROM every shot counts as a bullet (decided and logged without a file)
            None => (!rom, None),
        };
        if !bullet {
            return;
        }
        let place = if self.flyby_shell_place {
            cues::flyby_shell_point
        } else {
            cues::flyby_point
        };
        let Some(pos) = place(listener, from, to, impact) else {
            return;
        };
        let rec = self.flyby.clone();
        if let Some(r) = rec.as_ref() {
            if r.play_chance < 1.0 && cues::unit(&mut self.rng) >= r.play_chance {
                return;
            }
        } else if rom {
            return;
        }
        let file = rec.as_ref().and_then(|r| {
            let u = cues::unit(&mut self.rng);
            r.pick_path(u).map(str::to_string)
        });
        let kind = match rec.as_ref() {
            Some(r) if r.local != 0 => VoiceKind::Local,
            _ => VoiceKind::Positional,
        };
        self.stats.flybys += 1;
        self.start(
            now,
            listener,
            Play {
                file,
                trigger: Trigger::Flyby { shooter },
                pos: Some(pos),
                kind,
                inner: rec.as_ref().map_or(0.0, |r| r.inner_radius),
                outer: rec.as_ref().map_or(0.0, |r| r.outer_radius),
                volume: 100,
                prio: 0,
                class: CLASS_DEFAULT,
                looped: false,
                reverb: rec.as_ref().is_none_or(|r| r.reverb != 0),
                track: None,
            },
        );
    }

    /// The team callout of a throw relay (k 0x12): auto-cast team row `row` (the record's leading byte) in voice
    /// template `c`, heard by the team `b` only, at the record position.
    // EVIDENCE: cshell+0x2cbd30 (k 0x12, [`cues::callout_heard`], [`cues::TeamCallouts::pick`]), cshell+0x27de90 (the
    // team 3D play: flags | 0x30, through cshell+0x27dd70: a plain path is positional, inner = the share of the outer)
    #[allow(clippy::too_many_arguments)]
    fn callout(
        &mut self,
        now: u32,
        listener: [f32; 3],
        player: u32,
        row: u16,
        pos: [f32; 3],
        extra: Option<(u8, u8)>,
        own_team: Option<u8>,
    ) {
        let Some((team, template)) = extra else {
            self.stats.unplayed_relays += 1;
            return;
        };
        if !cues::callout_heard(team, own_team) {
            self.stats.unplayed_relays += 1;
            return;
        }
        let row = u8::try_from(row).unwrap_or(u8::MAX);
        let n = self
            .callouts
            .rows
            .get(usize::from(row))
            .and_then(|r| r.files.get(usize::from(template)))
            .map_or(0, Vec::len);
        let pick = cues::pick_n(&mut self.rng, n);
        let (file, outer) = match self.callouts.pick(row, template, pick) {
            Some((f, r)) => (Some(f.to_string()), r),
            None if self.vfs.is_none() => (None, JUMP_OUTER),
            None => {
                self.stats.unplayed_relays += 1;
                return;
            }
        };
        self.stats.callouts += 1;
        self.start(
            now,
            listener,
            Play {
                file,
                trigger: Trigger::Callout { player, row },
                pos: Some(pos),
                kind: VoiceKind::Positional,
                inner: outer * INNER_SHARE,
                outer,
                volume: 100,
                prio: cues::CALLOUT_PRIO,
                class: CLASS_DEFAULT,
                looped: false,
                reverb: true,
                track: None,
            },
        );
    }

    fn weapon_file(&self, weapon: u16, k: u8) -> Option<(String, f32)> {
        let base = crate::camo::base_weapon(weapon);
        match self.weapons.get(&base).and_then(|w| w.sound(k)) {
            Some((f, r)) => Some((
                f.to_string(),
                if r > 0.0 { r } else { WEAPON_OUTER_DEFAULT },
            )),
            // without a ROM every weapon sound is decided and logged without a file
            None if self.vfs.is_none() => Some((String::new(), WEAPON_OUTER_DEFAULT)),
            None => None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn weapon_at(
        &mut self,
        now: u32,
        listener: [f32; 3],
        file: String,
        outer: f32,
        pos: Option<[f32; 3]>,
        trigger: Trigger,
        cut: Option<(Emitter, u16)>,
    ) {
        self.start_weapon(
            now,
            listener,
            cut,
            Play {
                file: Some(file).filter(|f| !f.is_empty()),
                trigger,
                pos,
                kind: VoiceKind::Positional,
                inner: outer * INNER_SHARE,
                outer,
                volume: 100,
                prio: WEAPON_PRIO,
                class: CLASS_WEAPONS,
                looped: false,
                reverb: true,
                track: None,
            },
        );
    }

    fn own_weapon(
        &mut self,
        now: u32,
        listener: [f32; 3],
        file: String,
        trigger: Trigger,
        cut: Option<(Emitter, u16)>,
    ) {
        self.start_weapon(
            now,
            listener,
            cut,
            Play {
                file: Some(file).filter(|f| !f.is_empty()),
                trigger,
                pos: None,
                kind: VoiceKind::Local,
                inner: 0.0,
                outer: 0.0,
                volume: 100,
                prio: WEAPON_PRIO_OWN,
                class: CLASS_WEAPONS,
                looped: false,
                reverb: true,
                track: None,
            },
        );
    }

    /// The shooter's hit feedback: one of the two feedback sounds, picked at random, as a local voice.
    // EVIDENCE: cshell+0x2c65f0 -> cshell+0x202be0 (queues the time on the local character's effect, five slots),
    // cshell+0x201c00 (the effect's update plays a due slot: one of the two keys, local, priority 6, volume 100,
    // class 0; the slot's time is the queue time plus an untraced client variable, and a slot plays once its time is
    // not before the update's time, DISASM cshell+0x201c48..0x201c53)
    // DC-DEFAULT: CLI-98 (proposed) the feedback plays in the frame the record arrives (the queue delay is not
    // modelled); nothing for the ammunition of [`HIT_FEEDBACK_OTHER_PATH`]
    fn own_hit(&mut self, now: u32, listener: [f32; 3], first: u16, ammo: u16) {
        if HIT_FEEDBACK_OTHER_PATH.contains(&ammo) {
            return;
        }
        let pick = self.pick();
        let file = Some(self.hit_sounds[pick].clone()).filter(|s| !s.is_empty());
        if file.is_none() && self.vfs.is_some() {
            return;
        }
        self.stats.own_hits += 1;
        self.start(
            now,
            listener,
            Play {
                file,
                trigger: Trigger::OwnHit { first },
                pos: None,
                kind: VoiceKind::Local,
                inner: 0.0,
                outer: 0.0,
                volume: 100,
                prio: HIT_FEEDBACK_PRIO,
                class: CLASS_DEFAULT,
                looped: false,
                reverb: true,
                track: None,
            },
        );
    }

    /// The own kill voice: a file of auto-cast local row [`cues::LOCAL_ROW_ENEMY_KILL`] in the own voice template, as a
    /// local voice; nothing when the table has no such row or file.
    // EVIDENCE: cshell+0x2c430 (case 0x6d, see [`cues::LOCAL_ROW_ENEMY_KILL`] and [`cues::OWN_KILL_PRIO`])
    // DC-DEFAULT: CLI-601 (proposed) voice template 0: the own character's template (cshell+0x223430 on the name the
    // character's model gives) is not resolved, as CLI-175 for the groin voice; a new kill voice does not wait for the
    // previous one (the 2009 handle at roster +0x14 is kept, its use not ported)
    fn own_kill(&mut self, now: u32, listener: [f32; 3]) {
        let n = self
            .callouts
            .local
            .get(usize::from(cues::LOCAL_ROW_ENEMY_KILL))
            .and_then(|r| r.files.first())
            .map_or(0, Vec::len);
        let pick = cues::pick_n(&mut self.rng, n);
        let file = match self
            .callouts
            .pick_local(cues::LOCAL_ROW_ENEMY_KILL, 0, pick)
        {
            Some(f) => Some(f.to_string()),
            None if self.vfs.is_none() => None,
            None => return,
        };
        self.stats.own_kills += 1;
        self.start(
            now,
            listener,
            Play {
                file,
                trigger: Trigger::OwnKill,
                pos: None,
                kind: VoiceKind::Local,
                inner: 0.0,
                outer: 0.0,
                volume: 100,
                prio: cues::OWN_KILL_PRIO,
                class: CLASS_DEFAULT,
                looped: false,
                reverb: true,
                track: None,
            },
        );
    }

    /// The sprint breath: while the sprint gauge drains, a file of auto-cast local row [`cues::LOCAL_ROW_BREATH`]
    /// starts as a local voice whenever the previous breath has ended, so the breaths follow each other; when the
    /// sprint stops, the breath playing runs out (it is not cut).
    // EVIDENCE: cshell+0x95850 (the stamina element's update: gauge mode 1 -> asks the sound manager about the handle
    // at element +0x88; when it answers that the sound is over, picks row 0 for the own character's template
    // (cshell+0x223a10) and plays it with cshell+0x1b1e20 at priority 6, volume 100, pitch 1.0, keeping the new handle)
    // DC-DEFAULT: CLI-632 (proposed) voice template 0 as CLI-601; a breath without a voice (no ROM file) counts as
    // playing for [`BREATH_NO_VOICE_MS`]
    fn breath(&mut self, now: u32, listener: [f32; 3]) {
        let busy = match self.breath {
            Some((Some(id), _)) => !self.mixer().is_finished(id),
            Some((None, t)) => now.wrapping_sub(t) < BREATH_NO_VOICE_MS,
            None => false,
        };
        if busy {
            return;
        }
        let n = self
            .callouts
            .local
            .get(usize::from(cues::LOCAL_ROW_BREATH))
            .and_then(|r| r.files.first())
            .map_or(0, Vec::len);
        let pick = cues::pick_n(&mut self.rng, n);
        let file = match self.callouts.pick_local(cues::LOCAL_ROW_BREATH, 0, pick) {
            Some(f) => Some(f.to_string()),
            None if self.vfs.is_none() => None,
            None => return,
        };
        let voice = self.start(
            now,
            listener,
            Play {
                file,
                trigger: Trigger::Breath,
                pos: None,
                kind: VoiceKind::Local,
                inner: 0.0,
                outer: 0.0,
                volume: 100,
                prio: cues::OWN_KILL_PRIO,
                class: CLASS_DEFAULT,
                looped: false,
                reverb: true,
                track: None,
            },
        );
        self.breath = Some((voice, now));
    }

    /// A type 0x19 relay.
    // EVIDENCE: cshell+0x2cbd30 (k 1..14: nothing when the record names the own client id, else the weapon row's
    // sound k at the record position (cshell+0x1a2b50); k 15: one of the two character jump sounds at the position,
    // also for the own player; k 0x12: the team callout, SoundDirector::callout)
    #[allow(clippy::too_many_arguments)]
    fn relay_sound(
        &mut self,
        now: u32,
        listener: [f32; 3],
        k: u8,
        weapon: u16,
        player: u32,
        pos: [f32; 3],
        own: bool,
    ) {
        match k {
            1..=14 => {
                if own {
                    return;
                }
                if let Some((file, outer)) = self.weapon_file(weapon, k) {
                    self.stats.relay_sounds += 1;
                    let cut = cuts_previous(k).then_some((Emitter::Player(player), weapon));
                    self.weapon_at(
                        now,
                        listener,
                        file,
                        outer,
                        Some(pos),
                        Trigger::SoundRelay { k, player },
                        cut,
                    );
                }
            }
            dc_botclient::fx::SOUND_JUMP if own => {
                // DC-DEFAULT: CLI-681 (proposed; amends CLI-83) the own jump plays once: the local take-off sound of
                // cshell+0x1ff0b0; the server's echo of it (k 15 naming the own client) is not played again (the 2009
                // client played both, cshell+0x2cbd30: the user heard a double jump sound, round 9)
                self.stats.own_jump_echoes += 1;
            }
            dc_botclient::fx::SOUND_JUMP => {
                let i = self.pick();
                let file = Some(self.jump_sounds[i].clone()).filter(|s| !s.is_empty());
                self.stats.relay_sounds += 1;
                self.start(
                    now,
                    listener,
                    Play {
                        file,
                        trigger: Trigger::SoundRelay { k, player },
                        pos: Some(pos),
                        kind: VoiceKind::Positional,
                        inner: JUMP_OUTER * INNER_SHARE,
                        outer: JUMP_OUTER,
                        volume: 100,
                        prio: JUMP_PRIO,
                        class: CLASS_DEFAULT,
                        looped: false,
                        reverb: true,
                        track: None,
                    },
                );
            }
            _ => self.stats.unplayed_relays += 1,
        }
    }

    /// A sound record start: the voice kind from the flags, the radii from the record.
    // SPEC: AUDIO_2009.md §3.2 (AMBIENT, POSITIONAL, LOOP, VOLUME, LOCAL_IF_OWN, NO_REVERB), §3.3 (kind choice:
    // neither bit -> local), §14.1 (world sounds: the ambient kind and the distance law with the object's radii);
    // UDP_SESSION_2009_B.md §8.2 / §8.3 (inner = ratio x outer / 255; 0x0800 follows the own view object)
    fn server_sound(
        &mut self,
        now: u32,
        listener: [f32; 3],
        s: &WireSound,
        file: Option<String>,
        at_load: bool,
    ) {
        let fl = s.flags;
        let kind = if fl & flag::LOCAL_IF_OWN != 0 {
            VoiceKind::Local
        } else if fl & flag::AMBIENT != 0 {
            VoiceKind::Ambient
        } else if fl & flag::POSITIONAL != 0 {
            VoiceKind::Positional
        } else {
            VoiceKind::Local
        };
        let trigger = match (s.track, at_load) {
            (Some(t), true) => Trigger::LoadSound { track: t },
            (Some(t), false) => Trigger::TrackedSound { track: t },
            (None, _) => Trigger::OneShot,
        };
        match trigger {
            Trigger::LoadSound { .. } => self.stats.load_sounds += 1,
            _ => self.stats.tracked += 1,
        }
        self.start(
            now,
            listener,
            Play {
                file,
                trigger,
                pos: if kind == VoiceKind::Local {
                    None
                } else {
                    s.pos
                },
                kind,
                inner: s.inner(),
                outer: s.outer as f32,
                volume: if fl & flag::VOLUME != 0 {
                    s.volume
                } else {
                    100
                },
                prio: s.prio,
                class: CLASS_DEFAULT,
                looped: fl & flag::LOOP != 0,
                reverb: fl & flag::NO_REVERB == 0,
                track: s.track,
            },
        );
    }

    /// Log entries of one trigger label.
    pub fn starts_of(&self, label: &str) -> Vec<&SoundLogEntry> {
        self.log
            .iter()
            .filter(|e| e.trigger.label() == label)
            .collect()
    }

    /// One summary line.
    /// The director's bounded resources for exit reports (perf-hitch): the sound log writer.
    pub fn bounds_line(&self) -> String {
        let log = self
            .log_file
            .as_ref()
            .map_or("off".to_string(), |w| w.stats().line());
        let r = &self.rig_stats;
        format!(
            "sound bounds: {}; footstep rigs inline {} queued {} built {} deferred lookups {} (pending high-water {}/{RIG_PENDING}); log {log}",
            self.cache.stats.line(),
            r.inline,
            r.queued,
            r.built,
            r.deferred,
            r.hwm_pending
        )
    }

    pub fn summary(&self) -> String {
        let s = &self.stats;
        format!(
            "sounds: starts {} voices {} missing {} | world {} tracked {} footsteps own {} remote {} relays {} shots {} own weapon {} own hits {} preloaded {} stops {} unplayed {} | flybys {} callouts {} silenced {} step-muted {} ladder-muted {} | {}",
            s.starts, s.voices, s.missing_files, s.load_sounds, s.tracked, s.footsteps_own, s.footsteps_remote,
            s.relay_sounds, s.shot_fires, s.own_weapon, s.own_hits, s.preloaded, s.stops, s.unplayed_relays,
            s.flybys, s.callouts, s.silenced_shots, s.step_muted, s.ladder_muted, self.note
        )
    }
}

/// Where a headless or window run writes its sound log, when asked.
#[derive(Debug, Clone, Default)]
pub struct SoundOptions {
    /// Run the sound director at all.
    pub enabled: bool,
    /// JSONL log of every start (hashes only).
    pub log: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{ModelRef, RemoteTracker};

    /// CLI-1761: a row without radius keys keeps the 2009 start values (reload / select 300, fire 1000); a key wins.
    #[test]
    fn a_weapon_row_without_radius_keys_keeps_the_row_start_radii() {
        let doc = ButeDoc::parse(
            "[Weapon16]
ReloadSnd1=\"a.wav\"
SelectSnd=\"s.wav\"
FireSnd=\"f.wav\"
             [Weapon17]
ReloadSnd1=\"a.wav\"
FireSnd=\"f.wav\"
FireSndRadius=8000
ReloadSndRadius=50
",
        );
        let w = weapon_sounds(&doc);
        assert_eq!(w[&16].sound(1), Some(("a.wav", 300.0)));
        assert_eq!(w[&16].sound(4), Some(("s.wav", 300.0)));
        assert_eq!(w[&16].sound(6), Some(("f.wav", 1000.0)));
        assert_eq!(w[&17].sound(1), Some(("a.wav", 50.0)));
        assert_eq!(w[&17].sound(6), Some(("f.wav", 8000.0)));
    }

    #[test]
    fn a_dry_fire_sound_that_is_no_click_takes_a_click() {
        let row = |dry: &str| WeaponSounds {
            files: (1..=14)
                .map(|k| {
                    if k == 7 {
                        dry.to_string()
                    } else {
                        String::new()
                    }
                })
                .collect(),
            radius: vec![100.0; 14],
            silenced: Vec::new(),
        };
        let lengths = |file: &str| match file {
            "tiny.wav" => Some(0.06),
            "click.wav" => Some(0.28),
            "bang.wav" => Some(1.87),
            _ => None,
        };
        let mut w: BTreeMap<u16, WeaponSounds> = [
            (1, row("tiny.wav")),
            (2, row("bang.wav")),
            (3, row("click.wav")),
            (4, row("gone.wav")),
            (5, row("")),
        ]
        .into_iter()
        .collect();
        click_dry_fire(&mut w, &mut |f| lengths(f));
        let dry =
            |w: &BTreeMap<u16, WeaponSounds>, n: u16| w[&n].sound(7).map(|(f, _)| f.to_string());
        assert_eq!(dry(&w, 1).as_deref(), Some("tiny.wav"), "a click stays");
        assert_eq!(dry(&w, 2).as_deref(), Some("click.wav"), "the bang goes");
        assert_eq!(dry(&w, 3).as_deref(), Some("click.wav"));
        assert_eq!(dry(&w, 4).as_deref(), Some("gone.wav"), "unread: it stays");
        assert_eq!(dry(&w, 5), None);

        // no click anywhere: silence rather than a shot
        let mut w: BTreeMap<u16, WeaponSounds> = [(2, row("bang.wav"))].into_iter().collect();
        click_dry_fire(&mut w, &mut |f| lengths(f));
        assert_eq!(dry(&w, 2), None);
    }

    fn look(clip: usize, t: f32) -> RemoteLook {
        RemoteLook {
            styles: Vec::new(),
            models: vec![ModelRef {
                model: "syn\\body.ltb".into(),
                textures: vec![],
            }],
            weapon: None,
            trackers: vec![RemoteTracker {
                clip,
                time_ms: t,
                blend_set: Some(1),
            }],
            visible: true,
            aim: None,
        }
    }

    fn remote(l: RemoteLook) -> RemoteBody {
        RemoteBody {
            obj: 190,
            pid: 2,
            name: "r".into(),
            team: 1,
            pos: [100.0, 40.0, 0.0],
            half: [16.0, 40.0, 16.0],
            yaw: None,
            look: Some(l),
        }
    }

    fn frame<'a>(now: u32, cues: &'a [SoundCue], remotes: &'a [RemoteBody]) -> AudioFrame<'a> {
        AudioFrame {
            now_ms: now,
            eye: [0.0, 33.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
            cues,
            remotes,
            own: None,
            world: None,
        }
    }

    /// A made-up rig: clip 1 of 600 ms with steps at 50 (foot "2") and 350 (foot "1"), clip 2 without keys.
    fn rig() -> ClipSteps {
        ClipSteps {
            clips: vec![
                (1000, vec![]),
                (600, vec![(50, Some(false)), (350, Some(true))]),
                (1000, vec![]),
            ],
        }
    }

    #[test]
    fn match_frames_keep_the_music_fade_on_the_music_clock() {
        // a game clock an hour ahead of the music clock must not finish a crossfade that just started
        let mut d = SoundDirector::open(None);
        let tone = Arc::new(Pcm {
            rate: dc_audio::OUTPUT_RATE,
            channels: 2,
            samples: vec![0.25; 4000],
        });
        let t0 = d.music_now_s();
        {
            let mut m = d.mixer();
            m.bgm.play("a", tone.clone(), true, 0.0, t0);
            m.bgm.play("b", tone, true, 1.0, t0);
        }
        d.frame(&frame(3_600_000, &[], &[]));
        let (cur, next) = d.mixer().bgm.gains();
        let (cur, next) = (cur.unwrap(), next.expect("crossfade still running"));
        assert!(
            cur > 0.5 && next < 0.5,
            "mid-fade near its start: {cur} {next}"
        );
        // a frame with the game clock behind the music clock does not finish it either
        d.frame(&frame(0, &[], &[]));
        assert!(d.mixer().bgm.gains().1.is_some());
    }

    // ------------------------------------------------------------------------------------- w2-audiocues

    fn own_audio(control: u32, speed: f32, surface: u8, life: Option<u8>) -> OwnAudio<'static> {
        OwnAudio {
            look: None,
            obj: Some(206),
            feet: [0.0, 0.0, 0.0],
            surface,
            control,
            on_ground: true,
            airborne: false,
            sprinting: false,
            alive: true,
            jumped: false,
            speed,
            life_state: life,
        }
    }

    /// Runs the own player for `ms` at 16 ms frames from `t0`, with `cues` in the first frame.
    fn run_own(d: &mut SoundDirector, t0: u32, ms: u32, o: OwnAudio<'_>, cues: &[SoundCue]) {
        let mut t = t0;
        while t <= t0 + ms {
            let c: &[SoundCue] = if t == t0 { cues } else { &[] };
            d.frame(&AudioFrame {
                own: Some(o),
                ..frame(t, c, &[])
            });
            t += 16;
        }
    }

    const RUN: u32 =
        dc_sim::player::bits::FWD | dc_sim::player::bits::RUN | dc_sim::player::bits::ANY_MOVE;

    /// Playtest report: "sometimes jump makes a double audio sound": the local take-off sound and the server's k 15
    /// echo of the own jump both played (two random picks of the two jump files, 3 ms apart). One press, one jump
    /// sound (CLI-681).
    #[test]
    fn one_jump_press_plays_one_jump_sound() {
        let mut d = SoundDirector::open(None);
        let echo = [SoundCue::Player {
            k: 15,
            weapon: 0,
            player: 0,
            pos: [0.0; 3],
            own: true,
            extra: None,
            own_team: None,
        }];
        let o = OwnAudio {
            jumped: true,
            ..own_audio(RUN, 300.0, 71, Some(1))
        };
        d.frame(&AudioFrame {
            own: Some(o),
            ..frame(0, &[], &[])
        });
        d.frame(&AudioFrame {
            own: Some(own_audio(RUN, 300.0, 71, Some(1))),
            ..frame(3, &echo, &[])
        });
        let jumps = d.starts_of("jump_sound").len() + d.starts_of("relay_sound").len();
        assert_eq!(jumps, 1, "one jump cue per press");
        assert_eq!(
            d.starts_of("jump_sound").len(),
            1,
            "the local take-off sound"
        );
    }

    /// Playtest report: "no sprinting breath noise". While the sprint gauge drains the breath of auto-cast local row 0
    /// plays as a local voice, one after the other; walking plays none (CLI-632).
    #[test]
    fn sprinting_plays_the_breath_and_walking_does_not() {
        let breaths = |sprinting: bool, control: u32| {
            let mut d = SoundDirector::open(None);
            let o = OwnAudio {
                sprinting,
                ..own_audio(control, 300.0, 71, Some(1))
            };
            run_own(&mut d, 0, 3000, o, &[]);
            d.log
                .iter()
                .filter(|e| e.trigger == Trigger::Breath)
                .map(|e| (e.t_ms, e.kind, e.prio))
                .collect::<Vec<_>>()
        };
        let sprint = breaths(true, RUN | dc_sim::player::bits::SPRINT);
        assert_eq!(
            sprint.len(),
            3,
            "one breath a second without a file: {sprint:?}"
        );
        assert_eq!(sprint[0], (0, VoiceKind::Local, 6));
        assert!(
            breaths(false, RUN).is_empty(),
            "walking / running: no breath"
        );
    }

    /// Playtest report: "walking-surface audio continues during a jump". Running on the ground plays the own steps; in
    /// the air after a take-off none plays (the step player's ground test); after the landing they resume (CLI-631).
    #[test]
    fn no_own_step_plays_while_airborne_after_a_jump() {
        let mut d = SoundDirector::open(None);
        let ground = own_audio(RUN, 300.0, 71, Some(1));
        let air = OwnAudio {
            on_ground: false,
            airborne: true,
            ..ground
        };
        run_own(&mut d, 0, 1000, ground, &[]);
        run_own(&mut d, 1016, 1500, air, &[]);
        run_own(&mut d, 2532, 1000, ground, &[]);
        let steps: Vec<u32> = d
            .log
            .iter()
            .filter(|e| matches!(e.trigger, Trigger::OwnStep { .. }))
            .map(|e| e.t_ms)
            .collect();
        assert!(steps.iter().any(|t| *t < 1016), "grounded steps: {steps:?}");
        assert!(
            steps.iter().all(|t| !(1016..2532).contains(t)),
            "a step in the air: {steps:?}"
        );
        assert!(
            steps.iter().any(|t| *t >= 2532),
            "steps after landing: {steps:?}"
        );
    }

    #[test]
    fn own_steps_come_from_the_view_bob_cycle_with_the_step_players_gates() {
        // running: a step every 357 ms (1.4 cycles a second, two steps a cycle), right foot first, alternating, also
        // without the own character's streamed clips (the old source needed them)
        let mut d = SoundDirector::open(None);
        run_own(&mut d, 0, 3000, own_audio(RUN, 300.0, 71, Some(1)), &[]);
        let steps: Vec<(u32, bool, u16)> = d
            .log
            .iter()
            .filter_map(|e| match e.trigger {
                Trigger::OwnStep { left, surface } => Some((e.t_ms, left, surface)),
                _ => None,
            })
            .collect();
        assert!((8..=9).contains(&steps.len()), "{steps:?}");
        assert!(!steps[0].1 && steps.windows(2).all(|w| w[0].1 != w[1].1));
        assert!(steps.iter().all(|s| s.2 == 71));
        assert_eq!(d.starts_of("footstep_own").len(), steps.len());
        let own = &d.starts_of("footstep_own")[0];
        assert_eq!((own.kind, own.prio), (VoiceKind::Local, FOOTSTEP_PRIO_OWN));
        // walking (no run or sprint bit), a dead state, or the server echo source: no own step
        for (control, life) in [
            (
                dc_sim::player::bits::FWD | dc_sim::player::bits::ANY_MOVE,
                Some(1),
            ),
            (RUN, Some(2)),
            (RUN, Some(6)),
        ] {
            let mut d = SoundDirector::open(None);
            run_own(&mut d, 0, 3000, own_audio(control, 300.0, 71, life), &[]);
            assert!(
                d.starts_of("footstep_own").is_empty(),
                "{control:#x} {life:?}"
            );
        }
        let mut d = SoundDirector::open(None);
        d.own_steps = OwnSteps::ServerEcho;
        run_own(&mut d, 0, 3000, own_audio(RUN, 300.0, 71, Some(1)), &[]);
        assert!(d.starts_of("footstep_own").is_empty(), "no streamed clips");
    }

    #[test]
    fn the_death_note_mutes_the_characters_footsteps_until_the_spawn_note() {
        // CLI-680: the death note (0x1d) mutes, the spawn note (0x1f) unmutes; the spawn window (0x1f .. 0x25) does
        // not mute
        let mut d = SoundDirector::open(None);
        let dead = [SoundCue::StepMute { obj: 206, on: true }];
        run_own(&mut d, 0, 2000, own_audio(RUN, 300.0, 71, Some(1)), &dead);
        assert!(
            d.starts_of("footstep_own").is_empty(),
            "muted after the death note"
        );
        assert!(d.stats.step_muted >= 4, "{:?}", d.stats);
        let spawn = [SoundCue::SpawnMark { obj: 206, on: true }];
        run_own(
            &mut d,
            2016,
            2000,
            own_audio(RUN, 300.0, 71, Some(1)),
            &spawn,
        );
        assert!(
            d.starts_of("footstep_own").len() >= 4,
            "steps inside the spawn window"
        );
        let n = d.starts_of("footstep_own").len();
        let end = [SoundCue::SpawnMark {
            obj: 206,
            on: false,
        }];
        run_own(&mut d, 4032, 2000, own_audio(RUN, 300.0, 71, Some(1)), &end);
        assert!(
            d.starts_of("footstep_own").len() >= n + 4,
            "steps after the end note"
        );
        assert_eq!(d.spawn_log, vec![(2016, 206, true), (4032, 206, false)]);
        // sub 0x1e clears the mute as well
        let mut d = SoundDirector::open(None);
        run_own(&mut d, 0, 1000, own_audio(RUN, 300.0, 71, Some(1)), &dead);
        let off = [SoundCue::StepMute {
            obj: 206,
            on: false,
        }];
        run_own(&mut d, 1016, 2000, own_audio(RUN, 300.0, 71, Some(1)), &off);
        assert!(d.starts_of("footstep_own").len() >= 4);
        // a remote character's clip steps are muted by its own death note, not by its spawn note
        for (cue, muted) in [
            (SoundCue::StepMute { obj: 190, on: true }, true),
            (SoundCue::SpawnMark { obj: 190, on: true }, false),
        ] {
            let mut d = SoundDirector::open(None);
            d.set_rig("syn\\body.ltb", rig());
            let mut t = 0u32;
            while t <= 1500 {
                let c = [cue.clone()];
                let r = [remote(look(1, t as f32))];
                d.frame(&frame(t, if t == 0 { &c } else { &[] }, &r));
                t += 16;
            }
            assert_eq!(d.starts_of("footstep").is_empty(), muted, "{cue:?}");
        }
    }

    #[test]
    fn a_ladder_without_movement_sounds_is_silent_and_an_override_changes_the_surface() {
        let ladder = |sounds, surface| {
            SoundCue::Ladders(vec![LadderSound {
                obj: 300,
                pos: [0.0, 50.0, 0.0],
                sounds,
                surface,
            }])
        };
        let mut d = SoundDirector::open(None);
        run_own(
            &mut d,
            0,
            2000,
            own_audio(RUN, 100.0, 200, Some(1)),
            &[ladder(false, 0)],
        );
        assert!(d.starts_of("footstep_own").is_empty());
        assert!(d.stats.ladder_muted >= 4);
        let mut d = SoundDirector::open(None);
        run_own(
            &mut d,
            0,
            2000,
            own_audio(RUN, 100.0, 200, Some(1)),
            &[ladder(true, 20)],
        );
        let s = d.starts_of("footstep_own");
        assert!(!s.is_empty());
        assert!(s
            .iter()
            .all(|e| matches!(e.trigger, Trigger::OwnStep { surface: 20, .. })));
        let mut d = SoundDirector::open(None);
        run_own(
            &mut d,
            0,
            2000,
            own_audio(RUN, 100.0, 200, Some(1)),
            &[ladder(true, 0)],
        );
        let s = d.starts_of("footstep_own");
        assert!(
            !s.is_empty()
                && s.iter()
                    .all(|e| matches!(e.trigger, Trigger::OwnStep { surface: 200, .. }))
        );
    }

    fn shot_of(ammo: u16, from: [f32; 3], to: [f32; 3], mods: [u8; 3], own: bool) -> SoundCue {
        SoundCue::Shot {
            shooter: 190,
            weapon: 145,
            ammo,
            from,
            to,
            mods,
            clear: 0x4f10,
            own,
        }
    }

    fn passing_shot(own: bool, mods: [u8; 3]) -> SoundCue {
        shot_of(4, [-2000.0, 33.0, 300.0], [2000.0, 33.0, 300.0], mods, own)
    }

    #[test]
    fn another_players_bullet_past_the_listener_plays_the_flyby() {
        let mut d = SoundDirector::open(None);
        d.frame(&frame(100, &[passing_shot(false, [0xff; 3])], &[]));
        let f = d.starts_of("flyby");
        assert_eq!(f.len(), 1);
        let p = f[0].pos.unwrap();
        assert!(
            p[0].abs() < 1e-3 && (p[2] - 300.0).abs() < 1e-3,
            "nearest point {p:?}"
        );
        assert_eq!(
            (f[0].prio, f[0].class, f[0].volume),
            (0, CLASS_DEFAULT, 100)
        );
        // the own shot, and a shot that passes beyond the radius, play none
        let mut d = SoundDirector::open(None);
        d.frame(&frame(100, &[passing_shot(true, [0xff; 3])], &[]));
        let far = SoundCue::Shot {
            shooter: 190,
            weapon: 145,
            ammo: 4,
            from: [-2000.0, 33.0, 900.0],
            to: [2000.0, 33.0, 900.0],
            mods: [0xff; 3],
            clear: 0x4f10,
            own: false,
        };
        d.frame(&frame(200, &[far], &[]));
        assert!(d.starts_of("flyby").is_empty());
        // with a ROM-like table: a projectile ammunition (Type 2) and a non-bullet damage type play none
        let mut d = SoundDirector::open(None);
        d.vfs = None;
        for (ammo, ty, dt, want) in [(4u16, 0u32, 2u32, 1usize), (5, 2, 2, 0), (6, 0, 1, 0)] {
            d.shot_rows.ammo.insert(
                ammo,
                crate::fx::AmmoFx {
                    ammo_type: ty,
                    damage_type: dt,
                    ..crate::fx::AmmoFx::default()
                },
            );
            let before = d.starts_of("flyby").len();
            let c = shot_of(
                ammo,
                [-2000.0, 33.0, 300.0],
                [2000.0, 33.0, 300.0],
                [0xff; 3],
                false,
            );
            d.frame(&frame(300, &[c], &[]));
            assert_eq!(d.starts_of("flyby").len() - before, want, "ammo {ammo}");
        }
    }

    #[test]
    fn a_silencer_in_the_shooters_first_socket_plays_its_silenced_sound() {
        let mut d = SoundDirector::open(None);
        d.mod_rows.insert(
            7,
            cues::ModSound {
                kind: 0,
                silencer: 1,
            },
        );
        d.mod_rows.insert(
            8,
            cues::ModSound {
                kind: 0,
                silencer: 0,
            },
        );
        d.silenced
            .insert(145, vec![("TEST\\SILENCED.WAV".into(), 1200.0)]);
        let near = |mods| shot_of(4, [200.0, 33.0, 0.0], [200.0, 33.0, 0.0], mods, false);
        d.frame(&frame(100, &[near([7, 0xff, 0xff])], &[]));
        let s = d.starts_of("shot_silenced");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].file.as_deref(), Some("TEST\\SILENCED.WAV"));
        assert_eq!(
            (s[0].outer, s[0].class, s[0].prio),
            (1200.0, CLASS_WEAPONS, WEAPON_PRIO)
        );
        assert!(d.starts_of("shot_fire").is_empty());
        // level 0: silent; a mod in another socket or none: the fire sound
        d.frame(&frame(200, &[near([8, 0xff, 0xff])], &[]));
        assert_eq!(d.starts_of("shot_silenced").len(), 1);
        assert_eq!(d.stats.silenced_shots, 2);
        d.frame(&frame(300, &[near([0xff, 7, 0xff])], &[]));
        assert_eq!(d.starts_of("shot_fire").len(), 1);
    }

    #[test]
    fn the_throw_relay_plays_the_team_callout_for_its_team_only() {
        let relay = |own_team| SoundCue::Player {
            k: dc_botclient::fx::SOUND_THROW,
            weapon: 6,
            player: 2,
            pos: [100.0, 33.0, 0.0],
            own: false,
            extra: Some((1, 0)),
            own_team,
        };
        let mut d = SoundDirector::open(None);
        d.callouts = cues::TeamCallouts {
            rows: (0..7)
                .map(|i| cues::CalloutRow {
                    radius: 500.0,
                    files: vec![vec![format!("TEST\\C{i}A.WAV"), format!("TEST\\C{i}B.WAV")]],
                })
                .collect(),
            local: Vec::new(),
        };
        d.vfs = None;
        d.frame(&frame(100, &[relay(Some(1))], &[]));
        let c = d.starts_of("throw_callout");
        assert_eq!(c.len(), 1);
        assert!(
            c[0].file.as_deref().is_some_and(|f| f.contains("C6")),
            "row 6"
        );
        assert_eq!(
            (c[0].outer, c[0].prio, c[0].kind),
            (500.0, 2, VoiceKind::Positional)
        );
        assert_eq!(c[0].pos, Some([100.0, 33.0, 0.0]));
        d.frame(&frame(200, &[relay(Some(2)), relay(None)], &[]));
        assert_eq!(d.starts_of("throw_callout").len(), 1, "other team: none");
    }

    #[test]
    fn only_the_footstep_word_is_a_footstep() {
        // made-up words (the key word itself is ROM data, matched by hash; ROM-gated tests use the real clips)
        assert_eq!(footstep_foot(b""), None);
        assert_eq!(footstep_foot(b"  "), None);
        assert_eq!(footstep_foot(b"SOUND_KEY 1"), None);
        assert_eq!(name_key(b"abc"), name_key(b"ABC"), "case-insensitive");
    }

    #[test]
    fn remote_footsteps_follow_the_clip_keys_at_any_frame_rate() {
        for frame_ms in [7u32, 16, 33, 100] {
            let mut d = SoundDirector::open(None);
            d.set_rig("syn\\body.ltb", rig());
            let mut t = 0u32;
            while t <= 3000 {
                let r = [remote(look(1, t as f32))];
                d.frame(&frame(t, &[], &r));
                t += frame_ms;
            }
            let steps = d.starts_of("footstep");
            // 3000 ms of a 600 ms clip with two keys: keys at 50, 350, 650, ... 2750 -> 10 (the first frame only
            // places the cursor)
            assert_eq!(steps.len(), 10, "frame {frame_ms} ms: {}", steps.len());
            let lefts = steps
                .iter()
                .filter(|e| matches!(e.trigger, Trigger::Footstep { left: true, .. }))
                .count();
            assert_eq!(lefts, 5, "foot argument 2 = the left foot");
            for e in &steps {
                assert_eq!(e.kind, VoiceKind::Positional);
                assert_eq!((e.outer, e.prio), (FOOTSTEP_OUTER, FOOTSTEP_PRIO_REMOTE));
                assert_eq!(e.pos, Some([100.0, 0.0, 0.0]), "at the feet");
            }
        }
    }

    /// A director with the test rig and a surface row 0 whose step files exist (so the steps get voices and pans).
    fn stepping() -> SoundDirector {
        let mut d = SoundDirector::open(None);
        d.set_rig("syn\\body.ltb", rig());
        let f = |n: &str| [format!("TEST\\{n}1.WAV"), format!("TEST\\{n}2.WAV")];
        d.surfaces.rows.insert(
            0,
            SurfaceSounds {
                right: f("R"),
                left: f("L"),
            },
        );
        for n in f("R").into_iter().chain(f("L")) {
            d.set_sound(
                &n,
                Pcm {
                    rate: 44_100,
                    channels: 1,
                    samples: vec![0.25; 4_410],
                },
            );
        }
        d
    }

    /// The `(left, right)` pan of the remote footsteps a director logs for a listener at the origin (eye height 33)
    /// looking along `yaw` and a remote whose feet stand at `feet`, over `ms` of match clock from `t0`.
    fn step_pans(
        d: &mut SoundDirector,
        t0: u32,
        ms: u32,
        eye: [f32; 3],
        yaw: f32,
        feet: [f32; 3],
    ) -> Vec<(f32, f32)> {
        let before = d.starts_of("footstep").len();
        let mut body = remote(look(1, 0.0));
        body.pos = [feet[0], feet[1] + body.half[1], feet[2]];
        for t in (0..=ms).step_by(16) {
            let mut b = body.clone();
            if let Some(l) = b.look.as_mut() {
                l.trackers[0].time_ms = t as f32;
            }
            let r = [b];
            d.frame(&AudioFrame {
                eye,
                yaw,
                ..frame(t0 + t, &[], &r)
            });
        }
        let steps = d.starts_of("footstep");
        assert!(steps.len() > before + 1, "steps played");
        // the first step of a run may still see the listener of the frame before
        steps[before + 1..].iter().map(|e| e.pan.unwrap()).collect()
    }

    // SPEC: AUDIO_2009.md §5.1 (identity listener: forward +Z, right +X, up +Y; left-handed); RENDER_2009 / the
    // view convention of camshot (yaw about +Y, a yaw of 90 degrees looks along +X)
    #[test]
    fn remote_steps_pan_by_the_listeners_facing_and_follow_a_turn() {
        let eye = [0.0, 33.0, 0.0];
        let mut d = stepping();
        let mut t0 = 0;
        let mut run = |d: &mut SoundDirector, yaw: f32, feet: [f32; 3]| {
            let p = step_pans(d, t0, 1500, eye, yaw, feet);
            t0 += 2000;
            p
        };
        let close = |a: f32, b: f32| (a - b).abs() < 0.02;
        // straight ahead (+Z at yaw 0): centred
        for (l, r) in run(&mut d, 0.0, [0.0, 0.0, 400.0]) {
            assert!(close(l, r), "ahead: {l} {r}");
        }
        // to the left (-X at yaw 0): the left ear louder
        for (l, r) in run(&mut d, 0.0, [-400.0, 0.0, 0.0]) {
            assert!(l > 2.0 * r, "left: {l} {r}");
        }
        // the same source after a turn of 180 degrees: the right ear louder
        for (l, r) in run(&mut d, std::f32::consts::PI, [-400.0, 0.0, 0.0]) {
            assert!(r > 2.0 * l, "left after the turn: {l} {r}");
        }
        // a turn of 90 degrees to the right (facing +X): the source at -X is behind, centred
        for (l, r) in run(&mut d, std::f32::consts::FRAC_PI_2, [-400.0, 0.0, 0.0]) {
            assert!(close(l, r), "behind: {l} {r}");
        }
        // directly right (+X at yaw 0) at several distances: the right ear louder, more so farther out
        let mut last = 0.0;
        for dist in [60.0, 200.0, 600.0, 1100.0] {
            for (l, r) in run(&mut d, 0.0, [dist, 0.0, 0.0]) {
                assert!(r > l, "right at {dist}: {l} {r}");
                assert!(r / l >= last - 1e-3, "right at {dist}: {l} {r}");
                last = r / l;
            }
        }
        assert!(last > 3.0, "far right: {last}");
    }

    // CLI-1541: the director lives across matches; the next match restarts its clock at 0 and reuses object ids
    #[test]
    fn the_next_match_starts_without_the_old_matchs_step_mutes_cursors_and_breath() {
        let mut d = SoundDirector::open(None);
        // match 1: the own body dies as the match ends (step mute on), 10 minutes in; a breath is stamped on that clock
        let dead = [SoundCue::StepMute { obj: 206, on: true }];
        run_own(&mut d, 600_000, 500, own_audio(RUN, 300.0, 71, Some(1)), &dead);
        assert!(d.starts_of("footstep_own").is_empty());
        d.breath = Some((None, 600_000));
        d.cursors.insert((9, 0), Cursor { clip: 1, t: 400.0 });
        assert_eq!(d.end_world(), 0);
        // match 2 from clock 0: the same object id steps at once
        run_own(&mut d, 0, 2000, own_audio(RUN, 300.0, 71, Some(1)), &[]);
        assert!(
            d.starts_of("footstep_own").len() >= 4,
            "muted by the last match: {:?}",
            d.stats
        );
        assert!(d.cursors.is_empty() && d.breath.is_none());
        // a director that was not told the world ended (a stamp from the old clock) still breathes at once
        d.breath = Some((None, 600_000));
        d.breath(100, [0.0; 3]);
        assert_eq!(d.breath.map(|b| b.1), Some(100));
    }

    #[test]
    fn the_next_matchs_restarted_clock_still_turns_the_listener() {
        // the director (and its mixer) lives on from one match into the next, whose clock starts at 0 again
        let mut d = stepping();
        // match 1: ten minutes, ending far away and facing +X
        let far = [8000.0, 33.0, -8000.0];
        let _ = step_pans(
            &mut d,
            600_000,
            1500,
            far,
            std::f32::consts::FRAC_PI_2,
            [8000.0, 0.0, -7600.0],
        );
        // match 2 from clock 0: a source on the left at yaw 0 favours the left ear, and after a turn the right
        let eye = [0.0, 33.0, 0.0];
        for (l, r) in step_pans(&mut d, 0, 1500, eye, 0.0, [-400.0, 0.0, 0.0]) {
            assert!(l > 2.0 * r, "left in the next match: {l} {r}");
        }
        for (l, r) in step_pans(
            &mut d,
            2000,
            1500,
            eye,
            std::f32::consts::PI,
            [-400.0, 0.0, 0.0],
        ) {
            assert!(
                r > 2.0 * l,
                "left after the turn in the next match: {l} {r}"
            );
        }
    }

    #[test]
    fn walk_clip_is_silent_and_a_clip_change_restarts_the_cursor() {
        let mut d = SoundDirector::open(None);
        d.set_rig("syn\\body.ltb", rig());
        for t in (0..2000).step_by(16) {
            let r = [remote(look(2, t as f32))];
            d.frame(&frame(t, &[], &r));
        }
        assert!(d.starts_of("footstep").is_empty());
        // switching to the run clip at time 340 does not fire the 50 ms key; 350 fires on the next frame
        let r = [remote(look(1, 340.0))];
        d.frame(&frame(2000, &[], &r));
        assert!(d.starts_of("footstep").is_empty());
        let r = [remote(look(1, 356.0))];
        d.frame(&frame(2016, &[], &r));
        assert_eq!(d.starts_of("footstep").len(), 1);
        // jitter back by 20 ms fires nothing again
        let r = [remote(look(1, 336.0))];
        d.frame(&frame(2032, &[], &r));
        let r = [remote(look(1, 360.0))];
        d.frame(&frame(2048, &[], &r));
        assert_eq!(d.starts_of("footstep").len(), 1);
    }

    #[test]
    fn server_sounds_take_kind_radii_and_loop_from_the_record() {
        let mut d = SoundDirector::open(None);
        let wind = WireSound {
            track: Some(0xd6),
            file: 8,
            flags: 0x0135,
            prio: 0,
            outer: 10000,
            inner_ratio: 255,
            volume: 100,
            pitch: 1.0,
            start_ms: 0,
            tag: 0,
            pos: Some([0.0, 0.0, 0.0]),
        };
        let pain = WireSound {
            track: Some(0x200),
            flags: 0x0132,
            prio: 2,
            outer: 500,
            inner_ratio: 17,
            pos: Some([300.0, 0.0, 0.0]),
            ..wind.clone()
        };
        let cues = [
            SoundCue::Server {
                start: wind,
                file: None,
                at_load: true,
            },
            SoundCue::Server {
                start: pain,
                file: None,
                at_load: false,
            },
            SoundCue::Player {
                k: 3,
                weapon: 145,
                player: 1,
                pos: [0.0; 3],
                own: true,
                extra: None,
                own_team: None,
            },
            SoundCue::Player {
                k: 3,
                weapon: 145,
                player: 2,
                pos: [0.0; 3],
                own: false,
                extra: None,
                own_team: None,
            },
            SoundCue::Player {
                k: 15,
                weapon: 0,
                player: 1,
                pos: [0.0; 3],
                own: true,
                extra: None,
                own_team: None,
            },
            SoundCue::Player {
                k: 15,
                weapon: 0,
                player: 2,
                pos: [0.0; 3],
                own: false,
                extra: None,
                own_team: None,
            },
        ];
        d.frame(&frame(0, &cues, &[]));
        let w = &d.starts_of("load_sound")[0];
        assert_eq!(
            (w.kind, w.looped, w.outer),
            (VoiceKind::Ambient, true, 10000.0)
        );
        assert!((w.inner - 10000.0).abs() < 1.0);
        let p = &d.starts_of("tracked_sound")[0];
        assert_eq!(
            (p.kind, p.looped, p.prio),
            (VoiceKind::Positional, false, 2)
        );
        assert!((p.inner - 500.0 * 17.0 / 255.0).abs() < 0.01);
        // the own weapon and jump relays are not played again (CLI-681); another player's are
        let relays = d.starts_of("relay_sound");
        assert_eq!(relays.len(), 2, "{relays:?}");
        assert_eq!(d.stats.own_jump_echoes, 1);
        assert_eq!(relays[0].class, CLASS_WEAPONS);
        assert_eq!((relays[1].outer, relays[1].prio), (JUMP_OUTER, JUMP_PRIO));
    }

    /// CLI-1135: the world-load ambient loop and a tracked sound stop when the match hands the director back; a
    /// one-shot interface sound started after plays.
    #[test]
    fn the_world_sounds_stop_when_the_world_ends() {
        let mut d = SoundDirector::open(None);
        d.set_sound(
            "TEST\\WIND.WAV",
            Pcm {
                rate: 44_100,
                channels: 1,
                samples: vec![0.25; 44_100],
            },
        );
        let wind = WireSound {
            track: Some(0xd6),
            file: 8,
            flags: 0x0135,
            prio: 0,
            outer: 10000,
            inner_ratio: 255,
            volume: 100,
            pitch: 1.0,
            start_ms: 0,
            tag: 0,
            pos: Some([0.0, 0.0, 0.0]),
        };
        let hum = WireSound {
            track: Some(0xd7),
            ..wind.clone()
        };
        let cues = [
            SoundCue::Server {
                start: wind,
                file: Some("TEST\\WIND.WAV".into()),
                at_load: true,
            },
            SoundCue::Server {
                start: hum,
                file: Some("TEST\\WIND.WAV".into()),
                at_load: false,
            },
        ];
        d.frame(&frame(0, &cues, &[]));
        assert!(d.starts_of("load_sound")[0].looped);
        assert_eq!(d.mixer().active_voices(), 2);
        d.frame(&frame(5000, &[], &[]));
        assert_eq!(
            d.mixer().active_voices(),
            2,
            "the loop keeps playing in the match"
        );
        assert_eq!(d.end_world(), 2);
        assert_eq!(d.mixer().active_voices(), 0);
        assert!(d.tracked.is_empty());
        assert_eq!(d.end_world(), 0);
    }

    /// CLI-1890 (owner report 2026-11-05, Quarantine: "the audio still played until I fully left the lobby"): leaving
    /// the second match of a session stops every voice that match started: the tracked world loop, an untracked loop
    /// started on the match's first frame (the old clock-back reset of the `live` lines forgot it, so the old
    /// `end_world` never found it), and a long one-shot interface voice (the Quarantine intro is 12.7 s); the waiting
    /// room's music plays again once the menu cues its screen.
    #[test]
    fn leaving_a_match_stops_every_voice_and_the_room_music_comes_back() {
        let mut d = SoundDirector::open(None);
        let music_root = std::env::temp_dir().join(format!("dc_leave_{}", std::process::id()));
        std::fs::create_dir_all(music_root.join("Game").join("BGM")).unwrap();
        std::fs::write(music_root.join("Game").join("BGM").join("room.mp3"), b"x").unwrap();
        d.music = MusicDirector::none()
            .with_loader(|_| {
                Ok(Pcm {
                    rate: 44_100,
                    channels: 2,
                    samples: vec![0.1; 8_000],
                })
            })
            .with_files(
                music_root.clone(),
                crate::music::MusicFiles {
                    waiting: "BGM\\room.mp3".into(),
                    ..Default::default()
                },
            );
        d.music.set_classic(false);
        let wav = |secs: usize| Pcm {
            rate: 44_100,
            channels: 1,
            samples: vec![0.25; 44_100 * secs],
        };
        d.set_sound("TEST\\WIND.WAV", wav(1));
        d.set_sound("TEST\\INTRO.WAV", wav(13));
        let world_loop = WireSound {
            track: Some(0xd6),
            file: 8,
            flags: 0x0135,
            prio: 0,
            outer: 10000,
            inner_ratio: 255,
            volume: 100,
            pitch: 1.0,
            start_ms: 0,
            tag: 0,
            pos: Some([0.0, 0.0, 0.0]),
        };
        let cues = [
            SoundCue::Server {
                start: world_loop.clone(),
                file: Some("TEST\\WIND.WAV".into()),
                at_load: true,
            },
            SoundCue::Server {
                start: WireSound {
                    track: None,
                    flags: flag::POSITIONAL | flag::LOOP,
                    ..world_loop
                },
                file: Some("TEST\\WIND.WAV".into()),
                at_load: false,
            },
            SoundCue::Interface {
                file: Some("TEST\\INTRO.WAV".into()),
                what: "quarantine_intro",
            },
        ];
        // match 1 ran for 5 minutes and ended
        d.frame(&frame(0, &[], &[]));
        d.frame(&frame(300_000, &[], &[]));
        d.end_world();
        // match 2: the three sounds start on its first frame (its clock starts again at 0)
        d.frame(&frame(0, &cues, &[]));
        d.frame(&frame(1_000, &[], &[]));
        {
            let m = d.mixer();
            assert_eq!((m.active_voices(), m.looping_voices()), (3, 2));
        }
        // the player leaves 1 s in: nothing of the match plays on in the room
        assert_eq!(d.end_world(), 3);
        {
            let m = d.mixer();
            assert_eq!((m.active_voices(), m.looping_voices()), (0, 0));
            assert_eq!(m.free_handles(), dc_audio::MAX_VOICES);
        }
        // the room screen cues its music (cshell+0x1fba00, state 9) and it plays, looped
        d.music_cue(MusicCue::WaitingRoom);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while d.music.loading() && std::time::Instant::now() < end {
            d.music_tick();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        d.music_tick();
        {
            let m = d.mixer();
            assert_eq!(m.bgm.current_key(), Some("BGM\\room.mp3"));
            assert!(m.bgm.is_playing() && m.bgm.status().is_some_and(|s| s.looped));
            assert_eq!(m.active_voices(), 0, "the music is no voice");
        }
        std::fs::remove_dir_all(music_root).ok();
    }

    #[test]
    fn surface_rows_pick_by_foot_and_variant_with_the_default_row_as_fallback() {
        let row = |p: &str| SurfaceSounds {
            right: [format!("{p}r1"), format!("{p}r2")],
            left: [format!("{p}l1"), format!("{p}l2")],
        };
        let mut t = SurfaceTable::default();
        t.rows.insert(0, row("a"));
        t.rows.insert(71, row("s"));
        t.rows.insert(3, SurfaceSounds::default());
        assert_eq!(t.footstep(71, false, 0), Some("sr1"));
        assert_eq!(t.footstep(71, true, 1), Some("sl2"));
        assert_eq!(
            t.footstep(55, true, 0),
            Some("al1"),
            "unknown id -> the default row"
        );
        assert_eq!(
            t.footstep(3, true, 0),
            None,
            "a row without files is silent"
        );
        // a made-up document: rows without the id key are skipped
        let doc = ButeDoc::parse("[Surface0]\nOther = 1\n");
        assert!(SurfaceTable::from_doc(&doc).rows.is_empty());
    }

    /// A made-up shot record: `clear` is the record's `b`.
    fn shot(shooter: u16, clear: u16, own: bool) -> SoundCue {
        SoundCue::Shot {
            shooter,
            weapon: 145,
            ammo: 0,
            from: [200.0, 40.0, 0.0],
            to: [200.0, 40.0, 0.0],
            mods: [0xff; 3],
            clear,
            own,
        }
    }

    #[test]
    fn the_record_flags_gate_the_fire_sound() {
        // DISASM: cshell+0x2c8b25..0x2c8b46, cshell+0x2c8eeb..0x2c8ef6 (made-up masks: a blast-style record clears
        // both 0x1000 and 0x0800; a continuation record keeps 0x1000)
        assert!(!fire_sound_wanted(SHOT_FLAGS_FIRE_SOUND));
        assert!(!fire_sound_wanted(0xffff));
        assert!(
            !fire_sound_wanted(0x1800),
            "0x1000 and 0x0800 cleared, bit 0x0100 untraced"
        );
        assert!(fire_sound_wanted(0));
        assert!(fire_sound_wanted(0x0f01), "0x1000 kept");
        assert!(fire_sound_wanted(0x1000), "0x0800 kept");
        assert!(shot_names_own_client(3, Some(3)));
        assert!(!shot_names_own_client(3, Some(4)));
        assert!(
            !shot_names_own_client(0xff, None),
            "no own id yet: nothing is own"
        );
    }

    #[test]
    fn a_blast_plays_nothing_a_hitting_shot_plays_once_and_own_records_are_silent() {
        let mut d = SoundDirector::open(None);
        // a blast record: `b` clears the fire-sound bits
        d.frame(&frame(0, &[shot(190, 0x1900, false)], &[]));
        assert!(d.starts_of("shot_fire").is_empty(), "blast: no fire sound");
        // the two records of a hitting shot: the impact (silent) and the continuation (plays)
        d.frame(&frame(
            16,
            &[shot(190, 0x1800, false), shot(190, 0x0f01, false)],
            &[],
        ));
        let fires = d.starts_of("shot_fire");
        assert_eq!(fires.len(), 1, "{fires:?}");
        assert_eq!(fires[0].pos, Some([200.0, 40.0, 0.0]));
        assert_eq!(fires[0].kind, VoiceKind::Positional);
        // the own client's record (c names the own id) plays nothing even with the bits kept
        d.frame(&frame(32, &[shot(190, 0, true)], &[]));
        assert_eq!(d.starts_of("shot_fire").len(), 1);
        assert_eq!((d.stats.shot_fires, d.stats.silent_shots), (1, 3));
    }
    #[test]
    fn only_the_hit_record_of_an_own_shot_keeps_the_feedback_flag() {
        // DISASM: cshell+0x2c8b4a..0x2c8b91; the record masks of DSV-86 (REFERENCE tdm4 / tdm6): hit 0x0f11 / 0x0f01
        // keep 0x4000, the miss 0x4f10 and the continuations 0x4bf0 / 0x4be0 clear it
        assert!(hit_feedback_wanted(0x0f11));
        assert!(hit_feedback_wanted(0x0f01));
        assert!(!hit_feedback_wanted(0x4f10));
        assert!(!hit_feedback_wanted(0x4bf0));
        assert!(!hit_feedback_wanted(0x4be0));
    }

    #[test]
    fn an_own_hit_plays_one_local_feedback_and_the_other_ammunition_path_plays_none() {
        let mut d = SoundDirector::open(None);
        d.frame(&frame(
            0,
            &[SoundCue::OwnHit {
                first: 77,
                ammo: 145,
            }],
            &[],
        ));
        let hits = d.starts_of("own_hit");
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].kind, VoiceKind::Local);
        assert_eq!(hits[0].pos, None);
        assert_eq!(
            (hits[0].prio, hits[0].volume, hits[0].class),
            (HIT_FEEDBACK_PRIO, 100, CLASS_DEFAULT)
        );
        assert_eq!(hits[0].trigger, Trigger::OwnHit { first: 77 });
        d.frame(&frame(
            16,
            &[SoundCue::OwnHit {
                first: 77,
                ammo: HIT_FEEDBACK_OTHER_PATH[0],
            }],
            &[],
        ));
        assert_eq!(d.starts_of("own_hit").len(), 1);
        assert_eq!(d.stats.own_hits, 1);
    }

    #[test]
    fn menu_cues_keep_playing_after_many_clicks() {
        // r10 (CLI-922): the front end's loop (cues, then a music step) must release finished interface voices, or
        // the countdown after a few hundred clicks / hover changes finds the active list full and stays silent
        let mut d = SoundDirector::open(None);
        let file = "ui/click.wav";
        d.set_sound(
            file,
            Pcm {
                rate: 44_100,
                channels: 1,
                samples: vec![0.1; 16],
            },
        );
        let cue = |what| SoundCue::Interface {
            file: Some(file.to_string()),
            what,
        };
        let mut out = vec![0.0f32; 2 * 256];
        for _ in 0..(dc_audio::MAX_VOICES + 45) {
            d.menu_cues(&[cue("select")]);
            // the device callback plays the short sound to its end
            d.mixer().render(&mut out, 256);
            d.music_tick();
        }
        d.menu_cues(&[cue("start_count")]);
        let last = d.log.last().expect("logged");
        assert!(
            last.gain.is_some(),
            "the countdown cue got a voice ({} active)",
            d.mixer().active_voices()
        );
        assert!(d.mixer().active_voices() <= 1);
    }

    #[test]
    fn the_director_starts_with_the_shell_class_gains_so_a_remote_step_stays_below_unity() {
        let mut d = SoundDirector::open(None);
        assert_eq!(d.mixer().classes.gain(CLASS_DEFAULT as usize), 0.75);
        assert_eq!(d.mixer().classes.gain(CLASS_WEAPONS as usize), 1.0);
        // a class-0 footstep (outer 1200, inner 17 / 255 of it) 400 units away: A 0.878 x B 0.95 = 0.834 (it was
        // 1.113 with every class gain left at 1.0, B 1.27)
        let pcm = Arc::new(Pcm {
            rate: 44_100,
            channels: 1,
            samples: vec![0.1; 4410],
        });
        let req = PlayRequest {
            class: CLASS_DEFAULT,
            ..PlayRequest::positional(
                pcm,
                [400.0, 0.0, 0.0],
                FOOTSTEP_OUTER * INNER_SHARE,
                FOOTSTEP_OUTER,
            )
        };
        let id = d.mixer().play(req).unwrap();
        let g = d.mixer().gain_of(id).unwrap();
        assert!((g - 0.8337).abs() < 1e-3, "{g}");
        // the window config: null output, effects 50 (master-scaled twice: B floor(37.5) x 127 / 100 = 46 -> 0.46, C 0.5)
        let note = d.configure(&AudioConfig {
            effects_volume: 50,
            ..AudioConfig::default()
        });
        assert!(note.starts_with("audio out: null backend"), "{note}");
        assert!(
            note.ends_with("effects volume 50 music volume 100"),
            "{note}"
        );
        assert!(!d.has_device());
        let g = d.mixer().gain_of(id).unwrap();
        assert!((g - 0.87755 * 0.46 * 0.5).abs() < 1e-3, "{g}");
        assert!(d.close_output().starts_with("audio out: null backend |"));
    }
    /// The director on a real device: only with `DC_AUDIO_DEVICE_TEST=1` (off in CI-style runs). A 0.3 s tone at
    /// an output gain of 0.02 through the director's frames, then a clean close.
    #[cfg(feature = "audio")]
    #[test]
    fn the_director_plays_through_the_device_when_enabled() {
        if std::env::var("DC_AUDIO_DEVICE_TEST").ok().as_deref() != Some("1") {
            eprintln!("director device test skipped (set DC_AUDIO_DEVICE_TEST=1)");
            return;
        }
        let mut d = SoundDirector::open(None);
        let note = d.configure(&AudioConfig {
            out: AudioOut::Device,
            out_gain: 0.02,
            ..AudioConfig::default()
        });
        eprintln!("{note}");
        if !d.has_device() {
            return;
        }
        let tone: Vec<f32> = (0..13_230)
            .map(|i| (i as f32 * 330.0 * std::f32::consts::TAU / 44_100.0).sin() * 0.5)
            .collect();
        let id = d
            .mixer()
            .play(PlayRequest::local(Arc::new(Pcm {
                rate: 44_100,
                channels: 1,
                samples: tone,
            })))
            .unwrap();
        let t0 = std::time::Instant::now();
        while t0.elapsed() < std::time::Duration::from_millis(450) {
            d.frame(&frame(t0.elapsed().as_millis() as u32, &[], &[]));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            d.mixer().is_finished(id),
            "the 0.3 s voice ended in the device callbacks"
        );
        let line = d.close_output();
        eprintln!("{line}");
        assert!(!d.has_device());
        assert_eq!(d.mixer().output_rate(), dc_audio::OUTPUT_RATE);
        assert!(line.contains(" late 0 "), "{line}");
    }

    // ----------------------------------------------------------------------------------- client-audio3d

    /// A director without a ROM whose weapon 145 fires a made-up one-second sound.
    fn armed() -> SoundDirector {
        let mut d = SoundDirector::open(None);
        let mut files = vec![String::new(); 14];
        files[usize::from(SOUND_FIRE) - 1] = "TEST\\FIRE.WAV".into();
        files[2] = "TEST\\RELOAD.WAV".into();
        d.weapons.insert(
            145,
            WeaponSounds {
                files,
                radius: vec![2000.0; 14],
                silenced: Vec::new(),
            },
        );
        for f in ["TEST\\FIRE.WAV", "TEST\\RELOAD.WAV"] {
            d.set_sound(
                f,
                Pcm {
                    rate: 44_100,
                    channels: 1,
                    samples: vec![0.25; 44_100],
                },
            );
        }
        d
    }

    #[test]
    fn a_fire_sound_cuts_the_previous_one_of_the_same_emitter_and_weapon() {
        // EVIDENCE: cshell+0x1a2b50 / 0x1a2610 / 0x1a2770 (kill the stored fire voice, store the new one)
        assert!(cuts_previous(SOUND_FIRE) && cuts_previous(SOUND_ALT_FIRE));
        assert!(!cuts_previous(3) && !cuts_previous(4) && !cuts_previous(7));
        let mut d = armed();
        // the two records of one hitting shot in one frame: two starts, one voice
        d.frame(&frame(
            0,
            &[shot(190, 0x0f11, false), shot(190, 0x4bf0, false)],
            &[],
        ));
        assert_eq!(d.starts_of("shot_fire").len(), 2);
        assert_eq!(
            d.mixer().active_voices(),
            1,
            "the second record replaced the first voice"
        );
        assert_eq!(d.stats.fire_cuts, 1);
        // the next shot 100 ms later cuts the tail of the first one; another shooter does not
        d.frame(&frame(
            100,
            &[shot(190, 0x0f11, false), shot(191, 0x0f11, false)],
            &[],
        ));
        assert_eq!(d.mixer().active_voices(), 2);
        assert_eq!(d.stats.fire_cuts, 2);
        // the own weapon: each shot cuts the previous one; a reload sound (k 3) is not cut and cuts nothing
        for t in [200, 300, 400] {
            d.frame(&frame(
                t,
                &[SoundCue::OwnFire {
                    weapon: 145,
                    silencer: None,
                }],
                &[],
            ));
        }
        d.frame(&frame(
            450,
            &[SoundCue::OwnWeapon { k: 3, weapon: 145 }],
            &[],
        ));
        assert_eq!(d.mixer().active_voices(), 2 + 1 + 1);
        assert_eq!(d.stats.fire_cuts, 4);
        // switched off: every fire sound plays to its end (the behaviour before CLI-201)
        let mut d = armed();
        d.set_fire_cut(false);
        d.frame(&frame(
            0,
            &[shot(190, 0x0f11, false), shot(190, 0x4bf0, false)],
            &[],
        ));
        assert_eq!(d.mixer().active_voices(), 2);
        assert_eq!(d.stats.fire_cuts, 0);
    }

    #[test]
    fn own_fire_is_a_local_weapons_voice_at_the_2009_level_and_remote_fire_is_placed() {
        // EVIDENCE: cshell+0x237180 -> cshell+0x1a2b50(.., 1) -> cshell+0x1b1e20 (local, priority 6, flags 0x100,
        // volume 100, class 1): the engine's stage B of class 1 at gain 1.0 is 1.27, no own-fire attenuation in the
        // shell; the mixer saturates it at the level ceiling (CLI-202) and plays it at the centre level (CLI-200)
        use dc_audio::mixer::{LEVEL_CEILING, PAN_CENTRE};
        assert!((dc_audio::law::stage_b(100, 1.0, Some(1.0)) - 1.27).abs() < 1e-6);
        let mut d = armed();
        d.frame(&frame(
            0,
            &[SoundCue::OwnFire {
                weapon: 145,
                silencer: None,
            }],
            &[],
        ));
        let own = d.starts_of("own_fire")[0].clone();
        assert_eq!(
            (own.kind, own.class, own.volume, own.prio),
            (VoiceKind::Local, CLASS_WEAPONS, 100, WEAPON_PRIO_OWN)
        );
        assert!(
            (own.gain.unwrap() - LEVEL_CEILING).abs() < 1e-5,
            "{:?}",
            own.gain
        );
        assert_eq!(own.pan, Some((PAN_CENTRE, PAN_CENTRE)));
        // without the ceiling the engine level passes through
        d.mixer().set_level_ceiling(None);
        d.frame(&frame(
            50,
            &[SoundCue::OwnFire {
                weapon: 145,
                silencer: None,
            }],
            &[],
        ));
        let g = d.starts_of("own_fire")[1].gain.unwrap();
        assert!((g - 1.27).abs() < 1e-5, "{g}");
        d.mixer().set_level_ceiling(Some(LEVEL_CEILING));
        // effects volume 50: master-scaled twice (B floor(50) x 127 / 100 = 63 -> 0.63, C 0.5)
        d.configure(&AudioConfig {
            effects_volume: 50,
            ..AudioConfig::default()
        });
        d.frame(&frame(
            100,
            &[SoundCue::OwnFire {
                weapon: 145,
                silencer: None,
            }],
            &[],
        ));
        let g = d.starts_of("own_fire")[2].gain.unwrap();
        assert!((g - 0.315).abs() < 1e-5, "{g}");
        // a remote shot 600 units to the right (listener at yaw 0: right = +X): the left ear at the floor
        let mut d = armed();
        d.frame(&frame(0, &[], &[]));
        let side = SoundCue::Shot {
            shooter: 190,
            weapon: 145,
            ammo: 0,
            from: [600.0, 33.0, 0.0],
            to: [600.0, 33.0, 0.0],
            mods: [0xff; 3],
            clear: 0,
            own: false,
        };
        d.frame(&frame(200, &[side], &[]));
        let e = d.starts_of("shot_fire")[0].clone();
        assert_eq!(e.kind, VoiceKind::Positional);
        let (l, r) = e.pan.unwrap();
        assert!(
            (l - dc_audio::mixer::PAN_FLOOR * PAN_CENTRE).abs() < 1e-4
                && r > PAN_CENTRE
                && r <= 1.0,
            "{l} {r}"
        );
    }

    #[test]
    fn the_listener_turns_with_the_view_so_the_pan_follows_the_yaw() {
        // a shot 600 units ahead on +Z heard at yaw 0 (centre), 90 degrees (+Z is then on the left), 180 (behind:
        // centre), 270 (+Z on the right); the listener reaches the panner at 10 Hz (SPEC AUDIO_2009 §5.1)
        let ahead = SoundCue::Shot {
            shooter: 190,
            weapon: 145,
            ammo: 0,
            from: [0.0, 33.0, 600.0],
            to: [0.0, 33.0, 600.0],
            mods: [0xff; 3],
            clear: 0,
            own: false,
        };
        let mut pans = Vec::new();
        for (i, deg) in [0.0f32, 90.0, 180.0, 270.0].into_iter().enumerate() {
            let mut d = armed();
            let mut f = frame(0, &[], &[]);
            f.yaw = deg.to_radians();
            d.frame(&f);
            let cues = [ahead.clone()];
            let mut f = frame(200, &cues, &[]);
            f.yaw = deg.to_radians();
            d.frame(&f);
            pans.push((i, d.starts_of("shot_fire")[0].pan.unwrap()));
        }
        let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
        let c = dc_audio::mixer::PAN_CENTRE;
        let loud = (2.0 - dc_audio::mixer::PAN_FLOOR.powi(2)).sqrt() * c;
        let fl = dc_audio::mixer::PAN_FLOOR * c;
        assert!(close(pans[0].1 .0, c) && close(pans[0].1 .1, c), "{pans:?}");
        assert!(
            close(pans[1].1 .0, loud) && close(pans[1].1 .1, fl),
            "{pans:?}"
        );
        assert!(close(pans[2].1 .0, c) && close(pans[2].1 .1, c), "{pans:?}");
        assert!(
            close(pans[3].1 .0, fl) && close(pans[3].1 .1, loud),
            "{pans:?}"
        );
    }

    #[test]
    fn remote_fire_falls_off_with_the_engine_law_and_the_mixer_cut() {
        // SPEC AUDIO_2009 §5.3: A = 1 - (d^2 - inner^2) / (outer - inner)^2 inside (inner, outer), the class-1 level
        // on top (1.27, saturated at the CLI-202 ceiling 1.0); silent beyond outer + 0.01
        let inner = 2000.0 * INNER_SHARE;
        let mut last = f32::INFINITY;
        for d_units in [400.0f32, 900.0, 1400.0, 1800.0] {
            let mut d = armed();
            d.frame(&frame(0, &[], &[]));
            let cue = SoundCue::Shot {
                shooter: 190,
                weapon: 145,
                ammo: 0,
                from: [0.0, 33.0, d_units],
                to: [0.0, 33.0, d_units],
                mods: [0xff; 3],
                clear: 0,
                own: false,
            };
            d.frame(&frame(200, &[cue], &[]));
            let g = d.starts_of("shot_fire")[0].gain.unwrap();
            let a =
                1.0 - (d_units * d_units - inner * inner) / ((2000.0 - inner) * (2000.0 - inner));
            let level = 1.27f32.min(dc_audio::mixer::LEVEL_CEILING);
            assert!(
                (g - (a.max(0.0) * level)).abs() < 1e-3,
                "{d_units}: {g} vs {a}"
            );
            assert!(g < last);
            last = g;
        }
        let mut d = armed();
        d.frame(&frame(0, &[], &[]));
        let far = SoundCue::Shot {
            shooter: 190,
            weapon: 145,
            ammo: 0,
            from: [0.0, 33.0, 2100.0],
            to: [0.0, 33.0, 2100.0],
            mods: [0xff; 3],
            clear: 0,
            own: false,
        };
        d.frame(&frame(200, &[far], &[]));
        assert_eq!(d.starts_of("shot_fire")[0].gain, Some(0.0));
    }

    /// perf-hitch: in async mode a body's footstep rig is read on the rig thread (the frame gets "not yet"), and
    /// what arrives equals the inline read. Skipped without CA_ROM; no ROM value is asserted.
    #[test]
    fn rom_async_rig_arrives_equal_to_the_inline_read() {
        let Some(rom) = dc_rom::test_rom() else {
            return;
        };
        let vfs = RezVfs::new(rom.clone());
        let Some(model) = dc_render::CharacterTable::from_vfs(&vfs)
            .ok()
            .and_then(|t| t.bodies.values().next().map(|b| b.model.clone()))
        else {
            return;
        };
        let mut inline = SoundDirector::open(Some(&rom));
        let want = inline.rig(&model);
        assert_eq!(inline.rig_stats.inline, 1);
        let mut d = SoundDirector::open(Some(&rom));
        d.set_async_decode(true);
        assert!(
            d.rig(&model).is_none(),
            "first lookup is answered from the thread later"
        );
        let t = std::time::Instant::now();
        let got = loop {
            if let Some(r) = d.rig(&model) {
                break Some(r);
            }
            assert!(t.elapsed().as_secs() < 30, "rig thread did not answer");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(got.as_deref(), want.as_deref());
        assert_eq!(
            (d.rig_stats.inline, d.rig_stats.queued, d.rig_stats.built),
            (0, 1, 1)
        );
        assert!(d.rig_stats.hwm_pending <= RIG_PENDING);
    }
}
