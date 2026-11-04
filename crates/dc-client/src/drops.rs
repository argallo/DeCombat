//! Dropped weapons: the type 0x17 kind-6 pickup objects a server creates where a player died, drawn with the model
//! the server names and moved the way the 2009 client moves them (a short hop and a tumbling fall onto the floor,
//! then flat), and the pickup itself, which the 2009 client never asks for: the server decides by position and
//! answers with `GS_WEAPON_GIVE` 0xb2 plus the object's removal; the client fills the slot, sets the count and
//! draws the weapon.
//!
//! The 2009 rules, read from CShell.dll (own disassembly where decompiler made no function):
//! - The pickup object (method table VA 0x374f086c: init cshell+0x282490, update cshell+0x282670, server message
//!   cshell+0x2828e0) copies the blob's three flags to +0x38 (spin), +0x3c and +0x40 (fall). For kind 6 it looks the
//!   weapon row up by the first u16 and hides the model piece named by the row's `ProjectilePiece` key when the
//!   second u16 is 0. A non-empty FX name in the blob attaches that client FX.
//! - Its update: with the spin flag the rotation turns about its own up axis by 2.0941856 x a shell time value per
//!   frame; with the fall flag a per-frame step from the client attribute section `PickupItemFX` (start speed
//!   `WeaponThrowBaseVel`, its decrease `WeaponThrowCurVel`, the tumble rates `PitchRotVel` / `YawRotVel` /
//!   `RollRotVel`), a segment test from the old to the new position; on a hit the fall stops and the rotation
//!   becomes the identity (the weapon lies flat), else the position moves and the rotation is the tumble angles.
//! - Nothing in the object, its server messages or its removal sends a message: there is no pickup request. A
//!   0xb2 sets the slot (an identical weapon object is kept, cshell+0x22da30), sets the ammunition count with the
//!   pickup-notice flag (status slot +0x38, cshell+0x62f80), which marks the slot's entries new and calls the weapon
//!   change (cshell+0x1f0700 -> cshell+0x22e930: the slot becomes the drawn weapon and its 0x7f is sent through
//!   cshell+0x1f8fa0). No pickup sound is played in that path.
//!
//! w2-sprites: every type 0x17 pickup is drawn, not only kind 6. The mode items (the flags of the flag modes,
//! kind 2; the bounty items, kind 3; the HiSec case, kind 5) are kind-1 model objects like the drops, so the engine
//! draws them with the model block of their object; the shell object adds the same spin / fall flags and the client
//! FX for every kind. The FX named in the create blob starts with the object and a 0xa6 sub 1 names a new one; a
//! kind-6 drop with 0 rounds hides its row's `ProjectilePiece` piece.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 (type 0x17 `[3 x bool1][name:zstr8(256)][kind:u32]`, kind 6 `[u16][u16]`),
// §8.3 (0xa6 to a type 0x17 object: sub 1 FX, sub 2 countdown), §4.4 (0xb2); GAME_MODES_2009.md §4.4 (pickup kinds);
// GAME_MESSAGES_2009_C2S.md §1.3 (no pickup request among the 85 send sites), §4.4 (0x7f: number key, scroll, pickup)
// DISASM: cshell+0x282490 (init: record +0x08 / +0x0c / +0x10 -> +0x38 / +0x3c / +0x40; kind 6: row of the u16 at
// record +0x44, piece name = row +0x464, hidden flag = (u16 at record +0x46 == 0); FX from the name at record +0x14;
// +0x7c := client attributes +0x15c4, +0x78 / +0x60 / +0x64 / +0x68 := 0), cshell+0x282670 (update), cshell+0x2828e0
// (server message), cshell+0x1b28cf..0x1b2946 (the five `PickupItemFX` keys into the client attributes +0x15c4 ..
// +0x15d4, each read with the default 0), cshell+0x17e613 (row key `ProjectilePiece` -> row +0x464)
// OBSERVED: capture 20261025_202429_sbs4-real (two drops of weapon 145 created on the 2009
// client's link: accepted, no session error, and the client sent no message in reply)

use std::collections::BTreeMap;

use dc_botclient::session::BotSession;
use dc_botclient::weapon::WeaponGrant;
use dc_render::{LightSet, ModelInstance, Tracker};
use dc_sim::Collider;
use dc_wire::udp::BitReader;
use glam::{EulerRot, Quat, Vec3};

use crate::arms::Arsenal;
use crate::world::{ModelRef, RemoteTracker};

/// `UP_USE` (0x59) `[target:objref16][phase:u8]`: a DeCombat client sends it with phase 0 when the use key goes down
/// over a dropped weapon; the server takes it as a request for that drop (rounds, the weapon, or a swap, DSV-170).
/// The 2009 client sends 0x59 only for use targets of its own team (mode objects) and has no pickup request.
// SPEC: GAME_MESSAGES_2009_C2S.md §7.1 (0x59 layout, cshell+0x61e40), §1.3 (no pickup request)
// DC-DEFAULT: CLI-675 (proposed) the use key's request for a drop
pub const UP_USE: u8 = 0x59;
/// The `UP_USE` target of a use key press with no drop in reach (the server looks for a door lever itself).
// DC-DEFAULT: CLI-1133 (proposed)
pub const USE_NO_OBJECT: u16 = 0xffff;
/// How far from the eye a dropped weapon can be asked for: across the floor, and down to the floor below the feet.
// DC-DEFAULT: CLI-675 (proposed) our values; the server checks its own `drops.use_radius`
pub const USE_REACH: f32 = 64.0;
pub const USE_REACH_BELOW: f32 = 160.0;
pub const USE_REACH_ABOVE: f32 = 64.0;
/// Half extents of the box a dropped weapon answers the aim ray with (x, y, z).
// DC-DEFAULT: CLI-1352 (proposed; amended) the 2009 ray meets the engine's object box; our box size is ours
pub const AIM_BOX_HALF: [f32; 3] = [20.0, 10.0, 20.0];

/// The `UP_USE` body for object `obj`, phase 0 (pressed).
// SPEC: GAME_MESSAGES_2009_C2S.md §7.1
pub fn up_use_body(obj: u16) -> dc_wire::udp::BitWriter {
    let mut b = dc_wire::udp::BitWriter::new();
    b.write_objref16(Some(obj)).write_u8(0);
    b
}

/// Shell object type of a pickup.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2 (type 0x17 `OBJ_PICKUP`)
pub const OBJ_PICKUP: u8 = 0x17;
/// Pickup kind of a weapon or item pickup with its model.
// SPEC: GAME_MODES_2009.md §4.4 (kind 6 `[item:u16][b:u16]`)
pub const PICKUP_KIND_WEAPON: u32 = 6;
/// Pickup kinds of the mode items: the flag of the flag modes, the bounty item, the HiSec case.
// SPEC: GAME_MODES_2009.md §4.4 (kind 2 `[state][team][at_base]` classes 7 / 8, kind 3 `[flag]` class 9, kind 5
// `[case_key]`); HISEC_2009.md §3.1
pub const PICKUP_KIND_FLAG: u32 = 2;
pub const PICKUP_KIND_BOUNTY: u32 = 3;
pub const PICKUP_KIND_CASE: u32 = 5;
/// Shell 0xa6 (`GS_OBJECT_NOTE`) and its sub-id that names a pickup's new client FX.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.3 (`[note_kind:u8][obj:objref16]` + class data; type 0x17 sub 1
// `[fx_name:zstr8(256)]`: the old FX is removed first, none when empty; cshell+0x2828e0 PROVEN)
pub const SHELL_OBJECT_NOTE: u8 = 0xa6;
pub const NOTE_PICKUP_FX: u8 = 1;

/// The client attribute entry and the section of the drop motion.
// DISASM: cshell+0x1b28bb (section string VA 0x374eb7a4), cshell+0x1b28d1..0x1b293a (key strings VA 0x374eb790,
// 0x374eb77c, 0x374eb770, 0x374eb764, 0x374eb758)
pub const CLIENT_ENTRY: &str = crate::sound::CLIENT_ENTRY;
const MOTION_SECTION: &str = "PickupItemFX";
const KEY_START_SPEED: &str = "WeaponThrowBaseVel";
const KEY_SPEED_DROP: &str = "WeaponThrowCurVel";
const KEY_TUMBLE: [&str; 3] = ["PitchRotVel", "YawRotVel", "RollRotVel"];

/// Spin of a pickup with the spin flag: radians per unit of the shell time value, about the object's own up axis.
// DISASM: cshell+0x2826e6..0x282718 (the value of `client_shell`+0x1770 times the float at VA 0x374f63c8, a rotation
// about the up axis of the object's rotation (cshell+0x1b850), applied and set each frame)
pub const SPIN_RATE: f32 = 2.094_185_6;

/// The fall step's fixed rate.
// DC-DEFAULT: CLI-152 (proposed) the 2009 step moves the object by a per-frame amount (frame-rate dependent); DeCombat
// runs it at a fixed 60 steps per second whatever the frame rate. OBSERVED: the 2009 client's own frame counter
// read 60 in the sbs4 side-by-side frames (reports/sidebyside4, FPS 60)
pub const STEP_HZ: u32 = 60;
/// Most fall steps one advance runs (a long stall resumes from where the object was).
// DC-DEFAULT: CLI-152 (proposed) safety cap, 60 s of steps
pub const MAX_STEPS_PER_ADVANCE: u32 = 3600;
/// A falling drop below this height stops (it fell out of the world).
// DC-DEFAULT: CLI-152 (proposed) safety floor, not a 2009 rule
pub const FALL_FLOOR_Y: f32 = -100_000.0;

/// After a spawn, grants within this time belong to the spawn set; later ones while alive are pickups.
// DC-DEFAULT: CLI-151 (proposed) the spawn-grant window of CLI-29 (500 ms)
pub const SPAWN_GRANT_MS: u32 = crate::arms::GRANT_WAIT_MS;
/// A grant and a drop removal this close in time are the same pickup (report pairing only).
// DC-DEFAULT: CLI-151 (proposed) 1000 ms
pub const PAIR_MS: u32 = 1000;

/// A type 0x17 create blob (type byte first).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PickupBlob {
    /// Flag 1: the object spins.
    pub spin: bool,
    /// Flag 2 (object +0x3c; no reader in the update).
    pub flag_b: bool,
    /// Flag 3: the object falls.
    pub fall: bool,
    /// Client FX attached at the create (empty: none).
    pub fx: String,
    pub kind: u32,
    /// Kind 6: the weapon row number and the rounds (0 hides the row's `ProjectilePiece` piece).
    pub item: u16,
    pub rounds: u16,
}

/// Reads a type 0x17 create blob; `None` when it is not one or is too short for its kind.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.2; DISASM: cshell+0x282490 (the record fields it copies)
pub fn read_pickup_blob(blob: &[u8], bits: usize) -> Option<PickupBlob> {
    let mut r = BitReader::with_bit_len(blob, bits);
    if r.remaining_bits() < 11 || r.read_u8() != OBJ_PICKUP {
        return None;
    }
    let spin = r.read_bit();
    let flag_b = r.read_bit();
    let fall = r.read_bit();
    let fx = String::from_utf8_lossy(&r.read_zstr8()).into_owned();
    if r.remaining_bits() < 32 {
        return None;
    }
    let kind = r.read_u32();
    let mut b = PickupBlob {
        spin,
        flag_b,
        fall,
        fx,
        kind,
        ..PickupBlob::default()
    };
    if kind == PICKUP_KIND_WEAPON {
        if r.remaining_bits() < 32 {
            return None;
        }
        b.item = r.read_u16();
        b.rounds = r.read_u16();
    }
    Some(b)
}

/// A pickup in the world as the frame loop draws it: a dropped weapon (kind 6) or a mode item (kinds 2, 3, 5).
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponDrop {
    pub obj: u16,
    /// The blob's pickup kind ([`PICKUP_KIND_WEAPON`], [`PICKUP_KIND_FLAG`], ...).
    pub kind: u32,
    /// Kind 6: the weapon row (0 for the other kinds).
    pub weapon: u16,
    pub rounds: u16,
    pub spin: bool,
    pub fall: bool,
    /// The object position and rotation `(x, y, z, w)`: the create's, then the latest record's.
    pub pos: [f32; 3],
    pub rot: [f32; 4],
    /// The first model of the object's model block (`None`: the file table did not name it).
    pub model: Option<ModelRef>,
    /// The object's trackers (the main one first).
    pub trackers: Vec<RemoteTracker>,
    /// When the client first saw the object (ms of the session clock): the motion starts there.
    pub seen_ms: u32,
}

/// One pickup the server gave the local player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickupNote {
    pub at: u32,
    pub slot: u8,
    pub weapon: u16,
    /// The slot held the same weapon before (its clip was kept and the count set), else the weapon is new.
    pub same_weapon: bool,
    pub clip: u32,
    pub stock: u32,
    /// The drop object removed with it (paired by weapon and time).
    pub drop_obj: Option<u16>,
}

/// Counters of the drops a session saw.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DropCounters {
    /// Mode-item pickups (kinds other than 6) created, and the client FX the pickups started (create blob or 0xa6
    /// sub 1).
    pub mode_items: u64,
    pub fx_started: u64,
    /// Kind-6 pickup objects created and removed.
    pub created: u64,
    pub removed: u64,
    /// Every drop seen: (object, weapon, rounds, create position).
    pub seen: Vec<(u16, u16, u16, [f32; 3])>,
    /// Drops in the world now.
    pub live: Vec<(u16, u16, [f32; 3])>,
    /// Grants taken as pickups.
    pub pickups: Vec<PickupNote>,
}

/// The session side of the drops: which kind-6 pickups exist (from the session's object table), and the grants that
/// are pickups.
#[derive(Debug, Clone, Default)]
pub struct DropTracker {
    live: BTreeMap<u16, WeaponDrop>,
    alive_since: Option<u32>,
    pub stats: DropCounters,
    /// Client FX the pickups started since the last [`DropTracker::take_fx`].
    fx: Vec<crate::fx::WorldFx>,
    /// 0xa6 sub 1 names for objects the tracker has not seen yet (the note came in the poll of the create).
    pending_fx: BTreeMap<u16, String>,
    /// What each live pickup was created as (its create blob and model block): a different one under the same id is
    /// a new object (the server reused the id of a removed one within one poll).
    idents: BTreeMap<u16, PickupIdent>,
    /// Live pickups whose engine flags hide them now ([`pickup_shown`]): tracked, not drawn.
    hidden: std::collections::BTreeSet<u16>,
}

/// Object flag bits the 2009 engine tests before it puts an object into the frame's draw lists: the first flag word's
/// visible bit 0x0001 set and the second word's bit 0x0020 clear.
// EVIDENCE: engine+0x16b4b0 (world-tree walk), engine+0x16a7f0 (attachments), engine+0x16b580 (always-visible
// objects): an object is drawn only when byte +0x84 has bit 0x01 and byte +0x88 has no bit 0x20
// SPEC: UDP_SESSION_2009_B.md §4.2 item 3 (`CF_FLAGS` `[flags][flags2]...`; an object created without the block keeps
// the reader's flags 0); RENDER_2009.md §5.1 step 1
// DC-DEFAULT: CLI-583 (proposed) +0x84 / +0x88 read as the first and second net flag words (PLAUSIBLE)
pub const OBJ_FLAG_VISIBLE: u16 = 0x0001;
pub const OBJ_FLAG2_NOT_DRAWN: u16 = 0x0020;

/// True when the engine would draw a pickup object with these flags (a capture flag the server created without flag
/// words, as in a TDM room, is not drawn).
// DC-DEFAULT: CLI-583 (proposed)
pub fn pickup_shown(o: &dc_botclient::objects::ClientObject) -> bool {
    let flags2 = o.flag_words.map_or(0, |w| w.1);
    o.flags & OBJ_FLAG_VISIBLE != 0 && flags2 & OBJ_FLAG2_NOT_DRAWN == 0
}

/// A pickup object's create identity: its blob bits and the file ids of its model block.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PickupIdent {
    blob: (Vec<u8>, usize),
    models: Vec<(u16, Vec<u16>)>,
}

impl PickupIdent {
    fn of(o: &dc_botclient::objects::ClientObject) -> PickupIdent {
        PickupIdent {
            blob: o
                .blob
                .as_ref()
                .map(|w| (w.as_bytes().to_vec(), w.bit_len()))
                .unwrap_or_default(),
            models: o
                .models
                .iter()
                .map(|e| (e.model, e.textures.clone()))
                .collect(),
        }
    }
}

/// The first model of a pickup object's model block, by the names of the file table (`None` while the table does not
/// name its model file).
pub(crate) fn pickup_model(
    o: &dc_botclient::objects::ClientObject,
    file_name: &dyn Fn(u16) -> Option<String>,
) -> Option<ModelRef> {
    let name = |f: u16| -> Option<String> { (f != 0xFFFF).then(|| file_name(f)).flatten() };
    o.models.first().and_then(|e| {
        Some(ModelRef {
            model: name(e.model)?,
            textures: e
                .textures
                .iter()
                .map(|t| name(*t).unwrap_or_default())
                .collect(),
        })
    })
}

/// Reads a shell 0xa6 body (after the shell id) that names a pickup's new client FX: `(object, name)`.
// SPEC: GAME_MESSAGES_2009_S2C.md §8.3 (type 0x17 sub 1 `[fx_name:zstr8(256)]`)
pub fn read_pickup_fx_note(body: &[u8], bits: usize) -> Option<(u16, String)> {
    let mut r = BitReader::with_bit_len(body, bits);
    if r.remaining_bits() < 40 || r.read_u8() != OBJ_PICKUP {
        return None;
    }
    let obj = r.read_u16();
    if r.read_u8() != NOTE_PICKUP_FX || r.remaining_bits() < 8 {
        return None;
    }
    Some((obj, String::from_utf8_lossy(&r.read_zstr8()).into_owned()))
}

/// The distance along the unit ray `dir` from `o` to the entry of the box around `c` (0 when `o` is inside).
pub fn ray_box(o: [f32; 3], dir: [f32; 3], c: [f32; 3], half: [f32; 3]) -> Option<f32> {
    let (mut t0, mut t1) = (0.0f32, f32::MAX);
    for i in 0..3 {
        let (lo, hi) = (c[i] - half[i], c[i] + half[i]);
        if dir[i].abs() < 1e-9 {
            if o[i] < lo || o[i] > hi {
                return None;
            }
        } else {
            let (a, b) = ((lo - o[i]) / dir[i], (hi - o[i]) / dir[i]);
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
        }
    }
    (t0 <= t1).then_some(t0)
}

impl DropTracker {
    /// The dropped weapon (kind 6) the use key asks for from the eye point `eye`: the nearest one across the floor
    /// within [`USE_REACH`] and between [`USE_REACH_BELOW`] below and [`USE_REACH_ABOVE`] above the eye.
    // DC-DEFAULT: CLI-675 (proposed)
    pub fn use_target(&self, eye: [f32; 3]) -> Option<u16> {
        self.live
            .values()
            .filter(|d| d.kind == PICKUP_KIND_WEAPON && !self.hidden.contains(&d.obj))
            .filter(|d| {
                let dy = d.pos[1] - eye[1];
                (-USE_REACH_BELOW..=USE_REACH_ABOVE).contains(&dy)
            })
            .map(|d| {
                let h = ((d.pos[0] - eye[0]).powi(2) + (d.pos[2] - eye[2]).powi(2)).sqrt();
                (d.obj, h)
            })
            .filter(|(_, h)| *h <= USE_REACH)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(o, _)| o)
    }

    /// The dropped weapon (kind 6) the view ray from `eye` along `dir` meets within `reach` units, with the distance
    /// to the box entry: the item prompt's trigger. The nearest box wins; a hidden drop is not met.
    // DISASM: cshell+0x2b27c0 (the aim ray from the eye along the view, cshell+0x2b2da0 passes 10000.0), cshell+0x2b31a0
    // (a type 0x17 target within the activation distance -> text 0x1c6)
    // DC-DEFAULT: CLI-1352 (proposed; amended) box size ours ([`AIM_BOX_HALF`]); the own player is no drop
    pub fn aim_target(&self, eye: [f32; 3], dir: [f32; 3], reach: f32) -> Option<(u16, f32)> {
        self.live
            .values()
            .filter(|d| d.kind == PICKUP_KIND_WEAPON && !self.hidden.contains(&d.obj))
            .filter_map(|d| Some((d.obj, ray_box(eye, dir, d.pos, AIM_BOX_HALF)?)))
            .filter(|(_, t)| *t <= reach)
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// Shell 0x67: the own player state.
    pub fn on_life(&mut self, now: u32, state: u8) {
        match (state, self.alive_since) {
            (1, None) => self.alive_since = Some(now),
            (1, Some(_)) => {}
            _ => self.alive_since = None,
        }
    }

    /// Shell 0x66 kind 0x13 (the status reset at the head of every spawn set's weapons): the spawn window starts
    /// again, so the grants that follow refill the weapons. A player who lived through a round of a round mode stays
    /// in state 1 across the next round's spawn set; before, its grants counted as pickups and an identical weapon
    /// kept the last round's clip (user playtest 2026-11-03 "a weapon's clip stays empty from the previous round").
    // SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (`66 13 01 ..` before the weapons of a spawn set)
    // DC-DEFAULT: CLI-1762 (proposed)
    pub fn on_spawn_set(&mut self, now: u32) {
        if self.alive_since.is_some() {
            self.alive_since = Some(now);
        }
    }

    /// True while a 0xb2 is a pickup: alive and past the spawn set's window.
    // DC-DEFAULT: CLI-151 (proposed)
    pub fn grant_is_pickup(&self, now: u32) -> bool {
        self.alive_since
            .is_some_and(|t| now.saturating_sub(t) >= SPAWN_GRANT_MS)
    }

    /// Server 0xb2 / 0xb3 through the inventory. A pickup 0xb2 (alive, after the spawn set) is applied the 2009 way:
    /// an identical weapon in the slot keeps its clip and takes the told amount as its count, a new weapon starts
    /// with a full clip (the inventory's grant rule); then the slot is drawn (the weapon change of the pickup
    /// notice, with its select pair and 0x7f), the way the inventory draws the slot of an equip grant (0xb3).
    // EVIDENCE: cshell+0x22da30 (identical weapon kept), cshell+0x62f80 (count set; pickup flag -> cshell+0x1f0700),
    // cshell+0x22e930 (the slot becomes the drawn weapon; 0x7f through cshell+0x1f8fa0)
    // DISASM: cshell+0x60fb7..0x60fe8 (the 0xb2 handler passes the pickup flag 1 to status slot +0x38)
    // DC-DEFAULT: CLI-151 (proposed) only grants classified as pickups get the kept clip and the draw; a new weapon's
    // full clip is PLAUSIBLE (the inventory's existing grant rule)
    pub fn grant(&mut self, arsenal: &mut Arsenal, g: &WeaponGrant, now: u32) {
        let pickup = !g.equip && self.grant_is_pickup(now);
        let before = arsenal.slots.get(&g.slot).copied();
        arsenal.on_grant(g);
        if !pickup {
            return;
        }
        let same = before.is_some_and(|b| b.weapon == g.weapon);
        let Some(c) = arsenal.slots.get(&g.slot).copied() else {
            return;
        };
        let total = c.total();
        let clip = match before.filter(|_| same) {
            Some(b) => b.clip.min(total),
            None => c.clip,
        };
        let stock = total - clip;
        // the draw: an equip grant of the same slot with the clip and count just set (0xb3 draws its slot)
        arsenal.on_grant(&WeaponGrant {
            ammo: stock as f32,
            clip: Some(clip),
            equip: true,
            ..*g
        });
        self.stats.pickups.push(PickupNote {
            at: now,
            slot: g.slot,
            weapon: g.weapon,
            same_weapon: same,
            clip,
            stock,
            drop_obj: None,
        });
    }

    /// Shell 0xa6 type 0x17 sub 1: the pickup's client FX becomes `name` (an empty name only removes the old one).
    // SPEC: GAME_MESSAGES_2009_S2C.md §8.3; DC-DEFAULT: CLI-521 (proposed)
    pub fn on_fx_note(&mut self, obj: u16, name: String) {
        match self.live.get(&obj) {
            Some(d) => {
                let (pos, rot) = (d.pos, d.rot);
                self.start_fx(name, pos, rot);
            }
            None => {
                self.pending_fx.insert(obj, name);
            }
        }
    }

    fn start_fx(&mut self, name: String, pos: [f32; 3], rot: [f32; 4]) {
        if name.trim().is_empty() {
            return;
        }
        self.stats.fx_started += 1;
        // w2-fx x w2-sprites: a pickup effect plays once, tied to no object (no loop, no cut)
        self.fx.push(crate::fx::WorldFx {
            name,
            pos,
            rot,
            obj: None,
            value: 0,
        });
    }

    /// The client FX the pickups started since the last call (one [`crate::fx::FxEvent::World`] each).
    pub fn take_fx(&mut self) -> Vec<crate::fx::WorldFx> {
        std::mem::take(&mut self.fx)
    }

    /// After every session poll: the type 0x17 pickups of the object table (new ones start their motion and their
    /// client FX now, moved ones take the server's place), the ones a removal record took away, and the pairing of a
    /// removal with a pickup grant.
    // SPEC: UDP_SESSION_2009_B.md §3.4 (model block names), §5.1 (removal record); part A §4.4 (file table)
    // DC-DEFAULT: CLI-519 (proposed) every pickup kind is drawn with its object's model
    pub fn sync(&mut self, s: &BotSession, now: u32) {
        let files = &s.obs.files;
        self.sync_objects(
            &s.table.objects,
            &|f| files.get(&f).map(|x| x.1.clone()),
            now,
        );
    }

    /// [`DropTracker::sync`] over an object table whose file ids `file_name` resolves.
    pub fn sync_objects(
        &mut self,
        objects: &BTreeMap<u16, dc_botclient::objects::ClientObject>,
        file_name: &dyn Fn(u16) -> Option<String>,
        now: u32,
    ) {
        let mut present: Vec<u16> = Vec::new();
        self.pending_fx.retain(|k, _| objects.contains_key(k));
        // CLI-1072: a pickup attached to a character (a carried flag) is drawn on him, not where its object stands
        let attached: std::collections::BTreeSet<u16> = objects
            .values()
            .flat_map(|o| o.attachments.iter().map(|a| a.child))
            .collect();
        for (id, o) in objects {
            if o.blob_type != Some(OBJ_PICKUP) {
                continue;
            }
            let Some(b) = o
                .blob
                .as_ref()
                .and_then(|w| read_pickup_blob(w.as_bytes(), w.bit_len()))
            else {
                continue;
            };
            present.push(*id);
            // world-fixes: the engine's visible bit decides whether the pickup is drawn (CLI-583)
            if pickup_shown(o) && !attached.contains(id) {
                self.hidden.remove(id);
            } else {
                self.hidden.insert(*id);
            }
            let pos = o.pos.unwrap_or(o.create_pos);
            let rot = o.rot.unwrap_or([0.0, 0.0, 0.0, 1.0]);
            let ident = PickupIdent::of(o);
            // world-fixes: a pickup created under the id of one removed in the same poll (the server's lowest free id,
            // CD-66) is a new object: the old one is gone, the new one starts its model, motion and FX
            // DC-DEFAULT: CLI-582 (proposed)
            if self.idents.get(id).is_some_and(|i| *i != ident) {
                self.forget(*id, now);
            }
            if let Some(d) = self.live.get_mut(id) {
                // a later position / rotation record places the object there (a carried flag, a moved case)
                d.pos = pos;
                d.rot = rot;
                // a model whose file the table did not name at the create is named once the table has it
                if d.model.is_none() {
                    d.model = pickup_model(o, file_name);
                }
                continue;
            }
            let model = pickup_model(o, file_name);
            let trackers = o
                .anim
                .iter()
                .map(|a| RemoteTracker {
                    clip: a.anim as usize,
                    time_ms: a.time_ms as f32,
                    blend_set: a.blend_set.map(usize::from),
                })
                .collect();
            let d = WeaponDrop {
                obj: *id,
                kind: b.kind,
                weapon: b.item,
                rounds: b.rounds,
                spin: b.spin,
                fall: b.fall,
                pos,
                rot,
                model,
                trackers,
                seen_ms: now,
            };
            if b.kind == PICKUP_KIND_WEAPON {
                self.stats.created += 1;
                self.stats.seen.push((*id, b.item, b.rounds, pos));
            } else {
                self.stats.mode_items += 1;
            }
            self.live.insert(*id, d);
            self.idents.insert(*id, ident);
            // DISASM: cshell+0x282490 (a non-empty FX name at record +0x14 attaches that client FX at the create)
            let fx = self.pending_fx.remove(id).unwrap_or(b.fx);
            self.start_fx(fx, pos, rot);
        }
        let gone: Vec<u16> = self
            .live
            .keys()
            .filter(|k| !present.contains(k))
            .copied()
            .collect();
        for obj in gone {
            self.forget(obj, now);
        }
        self.stats.live = self
            .live
            .values()
            .filter(|d| d.kind == PICKUP_KIND_WEAPON)
            .map(|d| (d.obj, d.weapon, d.pos))
            .collect();
    }

    /// A pickup left the world: a weapon drop's removal is paired with a pickup grant of its weapon.
    fn forget(&mut self, obj: u16, now: u32) {
        self.idents.remove(&obj);
        self.hidden.remove(&obj);
        let Some(d) = self.live.remove(&obj) else {
            return;
        };
        if d.kind != PICKUP_KIND_WEAPON {
            return;
        }
        self.stats.removed += 1;
        if let Some(p) =
            self.stats.pickups.iter_mut().rev().find(|p| {
                p.drop_obj.is_none() && p.weapon == d.weapon && now.abs_diff(p.at) <= PAIR_MS
            })
        {
            p.drop_obj = Some(obj);
        }
    }

    /// The pickups in the world to draw now (the ones whose flags hide them are left out, CLI-583).
    pub fn objects(&self) -> Vec<WeaponDrop> {
        self.live
            .values()
            .filter(|d| !self.hidden.contains(&d.obj))
            .cloned()
            .collect()
    }
}

// --------------------------------------------------------------------------------------------- the motion

/// The `PickupItemFX` numbers of the client attribute file (a key the file lacks reads 0, as in the 2009 reader).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DropMotionRow {
    /// The upward speed a drop starts with (units per step).
    pub start_speed: f32,
    /// How much the speed's decrease grows per second.
    pub speed_drop: f32,
    /// Tumble rates while falling (radians per second): pitch, yaw, roll.
    pub tumble: [f32; 3],
}

fn number(v: Option<&String>) -> f32 {
    v.map(|t| t.trim().trim_matches('"').trim())
        .map(|t| t.strip_suffix(['f', 'F']).unwrap_or(t))
        .and_then(|t| t.parse::<f32>().ok())
        .filter(|x| x.is_finite())
        .unwrap_or(0.0)
}

impl DropMotionRow {
    /// The row of a parsed client attribute document.
    // DISASM: cshell+0x1b28cf..0x1b2946 (each key read with the default 0)
    pub fn from_doc(doc: &dc_bute::ButeDoc) -> DropMotionRow {
        let Some(sec) = doc.get(MOTION_SECTION) else {
            return DropMotionRow::default();
        };
        DropMotionRow {
            start_speed: number(sec.get(KEY_START_SPEED)),
            speed_drop: number(sec.get(KEY_SPEED_DROP)),
            tumble: KEY_TUMBLE.map(|k| number(sec.get(k))),
        }
    }

    /// The row of the user's ROM (all zero when the entry is missing).
    pub fn from_vfs(vfs: &dc_rez::RezVfs) -> DropMotionRow {
        dc_bute::ButeDoc::from_vfs(vfs, "ATTRIBUTES.rez", CLIENT_ENTRY)
            .map(|d| DropMotionRow::from_doc(&d))
            .unwrap_or_default()
    }
}

/// The client-side motion of one drop.
#[derive(Debug, Clone, PartialEq)]
pub struct DropMotion {
    pub pos: [f32; 3],
    pub rot: Quat,
    /// Vertical speed (units per step, positive up) and its per-step decrease.
    vy: f32,
    decrease: f32,
    tumble: [f32; 3],
    pub spin: bool,
    pub falling: bool,
    /// The fall ended on a surface.
    pub landed: bool,
    steps: u64,
    /// The first sight of the object the motion belongs to (another one under the same id starts a new motion).
    seen_ms: u32,
    /// The object place the motion last took from the server (a new record moves the motion there).
    origin: ([f32; 3], [f32; 4]),
}

fn unit_quat([x, y, z, w]: [f32; 4]) -> Quat {
    let q = Quat::from_xyzw(x, y, z, w);
    if q.is_finite() && q.length_squared() > 1e-12 {
        q.normalize()
    } else {
        Quat::IDENTITY
    }
}

impl DropMotion {
    /// The motion of a new drop: at its create position and rotation, the start speed of the row.
    // DISASM: cshell+0x282636..0x282644 (+0x7c := `WeaponThrowBaseVel`), cshell+0x28261e..0x282633 (the rest 0)
    pub fn new(d: &WeaponDrop, row: &DropMotionRow) -> DropMotion {
        DropMotion {
            pos: d.pos,
            rot: unit_quat(d.rot),
            vy: row.start_speed,
            decrease: 0.0,
            tumble: [0.0; 3],
            spin: d.spin,
            falling: d.fall,
            landed: false,
            steps: 0,
            seen_ms: d.seen_ms,
            origin: (d.pos, d.rot),
        }
    }

    /// A server record moved the object: the motion goes on from the new place and rotation.
    // DC-DEFAULT: CLI-520 (proposed)
    pub fn follow_server(&mut self, d: &WeaponDrop) {
        if self.origin == (d.pos, d.rot) {
            return;
        }
        self.origin = (d.pos, d.rot);
        self.pos = d.pos;
        self.rot = unit_quat(d.rot);
    }

    /// One step of `dt` seconds: spin, then the fall with its segment test.
    // DISASM: cshell+0x282670: spin first (rotation about its up axis, set); fall: when the speed is below 0 its
    // decrease restarts from 0; decrease += dt x `WeaponThrowCurVel`; speed -= decrease; y += speed; segment old ->
    // new (engine slot +0x64); hit: falling off, rotation := Euler (0, 0, 0); else position set (slot +0xc4), the three
    // tumble angles += dt x their rate, rotation := Euler (pitch, yaw, roll) (cshell+0x1b700)
    // DC-DEFAULT: CLI-152 (proposed) the segment test is the world collision without player-only blockers (the shot
    // trace of CLI-43); no world: no fall. The Euler order Y (yaw) X (pitch) Z (roll) is read from cshell+0x1b700's
    // matrix (PLAUSIBLE: the rotation sense of each axis is not checked against a 2009 frame); the shell time value of
    // the spin is taken as the frame time (PLAUSIBLE)
    pub fn step(&mut self, dt: f32, row: &DropMotionRow, world: Option<&dyn Collider>) {
        self.steps += 1;
        if self.spin {
            let up = self.rot * Vec3::Y;
            self.rot = (Quat::from_axis_angle(up, SPIN_RATE * dt) * self.rot).normalize();
        }
        if !self.falling {
            return;
        }
        let Some(world) = world else {
            self.falling = false;
            return;
        };
        if self.vy < 0.0 {
            self.decrease = 0.0;
        }
        self.decrease += dt * row.speed_drop;
        self.vy -= self.decrease;
        let old = self.pos;
        let new = [old[0], old[1] + self.vy, old[2]];
        let len = (new[1] - old[1]).abs();
        let hit = len > 1e-6
            && len.is_finite()
            && crate::arms::world_hit(world, old, [0.0, self.vy.signum(), 0.0], len).is_some();
        if hit || !new[1].is_finite() || new[1] < FALL_FLOOR_Y {
            self.falling = false;
            self.landed = hit;
            self.rot = Quat::IDENTITY;
            return;
        }
        self.pos = new;
        for (a, r) in self.tumble.iter_mut().zip(row.tumble) {
            *a += dt * r;
        }
        let [pitch, yaw, roll] = self.tumble;
        self.rot = Quat::from_euler(EulerRot::YXZ, yaw, pitch, roll);
    }

    /// Runs the fixed steps due between the drop's first sight and `now_ms` (any call rate gives the same motion).
    // DC-DEFAULT: CLI-152 (proposed)
    pub fn advance_to(
        &mut self,
        seen_ms: u32,
        now_ms: u32,
        row: &DropMotionRow,
        world: Option<&dyn Collider>,
    ) {
        let due = u64::from(now_ms.saturating_sub(seen_ms)) * u64::from(STEP_HZ) / 1000;
        let dt = 1.0 / STEP_HZ as f32;
        let mut n = 0;
        while self.steps < due && n < MAX_STEPS_PER_ADVANCE {
            self.step(dt, row, world);
            n += 1;
        }
    }
}

/// Where a drop is drawn this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct DropPlacement {
    pub obj: u16,
    pub kind: u32,
    pub weapon: u16,
    pub rounds: u16,
    pub model: Option<ModelRef>,
    pub pos: [f32; 3],
    pub rot: Quat,
    pub trackers: Vec<RemoteTracker>,
    pub landed: bool,
}

/// The motions of the drops in the world.
#[derive(Debug, Clone, Default)]
pub struct DropWorld {
    pub row: DropMotionRow,
    motions: BTreeMap<u16, DropMotion>,
}

impl DropWorld {
    pub fn new(row: DropMotionRow) -> DropWorld {
        DropWorld {
            row,
            motions: BTreeMap::new(),
        }
    }

    /// Advances every drop to `now_ms` (new ones start at their first sight; removed ones are forgotten) and returns
    /// where each is drawn.
    pub fn advance(
        &mut self,
        drops: &[WeaponDrop],
        now_ms: u32,
        world: Option<&dyn Collider>,
    ) -> Vec<DropPlacement> {
        self.motions
            .retain(|k, _| drops.iter().any(|d| d.obj == *k));
        let row = self.row;
        drops
            .iter()
            .map(|d| {
                // world-fixes (CLI-582): a new object under a reused id starts its own motion
                let m = self
                    .motions
                    .entry(d.obj)
                    .and_modify(|m| {
                        if m.seen_ms != d.seen_ms {
                            *m = DropMotion::new(d, &row);
                        }
                    })
                    .or_insert_with(|| DropMotion::new(d, &row));
                m.follow_server(d);
                m.advance_to(d.seen_ms, now_ms, &row, world);
                DropPlacement {
                    obj: d.obj,
                    kind: d.kind,
                    weapon: d.weapon,
                    rounds: d.rounds,
                    model: d.model.clone(),
                    pos: m.pos,
                    rot: m.rot,
                    trackers: d.trackers.clone(),
                    landed: m.landed,
                }
            })
            .collect()
    }
}

/// The weapon rows' `ProjectilePiece` names: the model piece a kind-6 drop with 0 rounds hides.
// DISASM: cshell+0x17e613 (row key `ProjectilePiece` -> row +0x464), cshell+0x282490 (kind 6: the piece of the row of
// the first u16 is hidden when the second u16 is 0)
// DC-DEFAULT: CLI-523 (proposed) the name is matched with the ASCII case ignored against the model's piece names
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectilePieces {
    rows: BTreeMap<u16, String>,
}

const KEY_PROJECTILE_PIECE: &str = "ProjectilePiece";

impl ProjectilePieces {
    /// The names of a parsed weapon document (`[Weapon<n>]` sections with a non-empty key).
    pub fn from_doc(doc: &dc_bute::ButeDoc) -> ProjectilePieces {
        let mut rows = BTreeMap::new();
        for (name, sec) in doc.iter() {
            let Some(n) = name
                .strip_prefix("Weapon")
                .filter(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|r| r.parse::<u16>().ok())
            else {
                continue;
            };
            let Some(v) = sec.get(KEY_PROJECTILE_PIECE) else {
                continue;
            };
            let v = v.trim().trim_matches('"').trim();
            if !v.is_empty() {
                rows.insert(n, v.to_string());
            }
        }
        ProjectilePieces { rows }
    }

    /// The names of the user's ROM (none when the weapon table is missing).
    pub fn from_vfs(vfs: &dc_rez::RezVfs) -> ProjectilePieces {
        dc_bute::ButeDoc::from_vfs(vfs, crate::arms::ATTRIBUTES_REZ, crate::arms::WEAPONS_ENTRY)
            .map(|d| ProjectilePieces::from_doc(&d))
            .unwrap_or_default()
    }

    pub fn get(&self, weapon: u16) -> Option<&str> {
        self.rows
            .get(&crate::camo::base_weapon(weapon))
            .map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The piece a placed pickup hides: kind 6 with 0 rounds hides its row's piece, nothing else hides one.
    // DISASM: cshell+0x282490
    pub fn hidden_for(&self, p: &DropPlacement) -> Option<&str> {
        (p.kind == PICKUP_KIND_WEAPON && p.rounds == 0)
            .then(|| self.get(p.weapon))
            .flatten()
    }
}

/// The drops on the GPU (built by `characters::ModelScene::update_drops`, which owns the model loader).
pub struct DropModels {
    pub world: DropWorld,
    live: BTreeMap<u16, (ModelRef, ModelInstance)>,
    /// Objects whose model failed to load, with that model (another model under the same id is tried).
    failed: Vec<(u16, ModelRef)>,
    /// Drops drawn with a model in the last update.
    pub drawn: u32,
    /// The pieces a weapon drop without rounds hides.
    pub pieces: ProjectilePieces,
    /// Drawn drops with a hidden piece in the last update.
    pub hidden: u32,
}

impl DropModels {
    pub fn new(row: DropMotionRow) -> DropModels {
        DropModels {
            world: DropWorld::new(row),
            live: BTreeMap::new(),
            failed: Vec::new(),
            drawn: 0,
            pieces: ProjectilePieces::default(),
            hidden: 0,
        }
    }

    /// The same set with the weapon rows' `ProjectilePiece` names.
    pub fn with_pieces(mut self, pieces: ProjectilePieces) -> DropModels {
        self.pieces = pieces;
        self
    }

    /// gpu-recover: the instances of a lost GPU device are put away; the drops' motion stays.
    pub fn gpu_reset(&mut self) {
        crate::gpulost::retire(std::mem::take(&mut self.live));
        self.failed.clear();
    }

    /// Forgets the drops that are no longer placed.
    pub fn keep(&mut self, placed: &[DropPlacement]) {
        let has = |k: &u16| placed.iter().any(|p| p.obj == *k);
        self.live.retain(|k, _| has(k));
        self.failed.retain(|(k, _)| has(k));
        self.drawn = 0;
        self.hidden = 0;
    }

    /// True when the drop has no instance of model `m` yet (and its model did not fail before).
    pub fn needs(&self, obj: u16, m: &ModelRef) -> bool {
        !self.failed.iter().any(|(k, f)| *k == obj && f == m)
            && self.live.get(&obj).is_none_or(|l| &l.0 != m)
    }

    pub fn insert(&mut self, obj: u16, m: ModelRef, inst: ModelInstance) {
        self.live.insert(obj, (m, inst));
    }

    pub fn fail(&mut self, obj: u16, m: &ModelRef) {
        self.live.remove(&obj);
        self.failed.push((obj, m.clone()));
    }

    /// Places and poses one drop: its model origin at the object position, the motion's rotation, the object's main
    /// tracker at its time (the server sends no stream for the object), lit at the object position.
    // SPEC: UDP_SESSION_2009_B.md §4.4 (the create's animation block)
    // DC-DEFAULT: CLI-153 (proposed) the tracker's clip is posed at the sent time and not advanced; the model origin
    // is the object position (no box: the create carries no dims)
    pub fn pose(&mut self, p: &DropPlacement, lights: &dyn Fn(Vec3, f32) -> LightSet) {
        let Some((_, inst)) = self.live.get_mut(&p.obj) else {
            return;
        };
        inst.position = Vec3::from(p.pos);
        inst.rotation = p.rot;
        let tr: Vec<Tracker> = p
            .trackers
            .first()
            .map(|t| vec![Tracker::main(t.clip, t.time_ms)])
            .unwrap_or_default();
        inst.set_pose(&tr);
        inst.lights = lights(inst.position, inst.radius);
        let hide = self.pieces.hidden_for(p);
        if hide.is_some() || !inst.hidden_pieces().is_empty() {
            let n = inst.hide_pieces(hide.as_slice());
            self.hidden += u32::from(n > 0);
        }
        self.drawn += 1;
    }

    /// The drop instance of object `obj` (tests and reports).
    pub fn instance(&self, obj: u16) -> Option<&ModelInstance> {
        self.live.get(&obj).map(|l| &l.1)
    }

    /// The drop instances to draw.
    pub fn instances(&self) -> impl Iterator<Item = &ModelInstance> {
        self.live.values().map(|l| &l.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_botclient::objects::ClientObject;
    use dc_sim::{ConvexPoly, V3};
    use dc_wire::udp::BitWriter;

    fn blob(spin: bool, fall: bool, kind: u32, a: u16, b: u16) -> BitWriter {
        let mut w = BitWriter::new();
        w.write_u8(OBJ_PICKUP)
            .write_bit(spin)
            .write_bit(false)
            .write_bit(fall)
            .write_zstr8(b"");
        w.write_u32(kind);
        if kind == PICKUP_KIND_WEAPON {
            w.write_u16(a).write_u16(b);
        }
        w
    }

    /// The kind-6 blob of the server's drop (83 bits: flags 0 / 0 / 1, no FX, weapon and rounds).
    #[test]
    fn reads_the_weapon_pickup_blob() {
        let w = blob(false, true, 6, 145, 150);
        assert_eq!(w.bit_len(), 83);
        let b = read_pickup_blob(w.as_bytes(), w.bit_len()).unwrap();
        assert_eq!(
            (b.spin, b.fall, b.kind, b.item, b.rounds, b.fx.as_str()),
            (false, true, 6, 145, 150, "")
        );
        let k5 = {
            let mut w = blob(false, false, 5, 0, 0);
            w.write_u32(9);
            w
        };
        assert_eq!(
            read_pickup_blob(k5.as_bytes(), k5.bit_len()).unwrap().kind,
            5
        );
        // too short for kind 6, or another type
        assert!(read_pickup_blob(w.as_bytes(), 70).is_none());
        let mut other = BitWriter::new();
        other.write_u8(0x18).write_u32(0);
        assert!(read_pickup_blob(other.as_bytes(), other.bit_len()).is_none());
    }

    fn floor(y: f32) -> dc_sim::CollisionWorld {
        let s = 4000.0;
        let poly = ConvexPoly::from_corners(
            vec![
                V3::new(-s, y, -s),
                V3::new(-s, y, s),
                V3::new(s, y, s),
                V3::new(s, y, -s),
            ],
            dc_sim::SURF_SOLID,
        );
        dc_sim::CollisionWorld::from_parts(vec![dc_sim::CollModel::from_polys(&[poly])], &[])
    }

    fn drop_at(y: f32, fall: bool, spin: bool) -> WeaponDrop {
        WeaponDrop {
            obj: 300,
            kind: PICKUP_KIND_WEAPON,
            weapon: 145,
            rounds: 150,
            spin,
            fall,
            pos: [10.0, y, 20.0],
            rot: [0.0, 0.5f32.sin(), 0.0, 0.5f32.cos()],
            model: None,
            trackers: Vec::new(),
            seen_ms: 1000,
        }
    }

    const ROW: DropMotionRow = DropMotionRow {
        start_speed: 2.0,
        speed_drop: 6.0,
        tumble: [3.0, 1.0, 2.0],
    };

    /// The 2009 fall: up first, then down onto the floor, tumbling on the way; on the floor it lies flat (identity)
    /// above the floor, never below it.
    #[test]
    fn a_drop_hops_falls_and_lies_flat_on_the_floor() {
        let w = floor(0.0);
        let d = drop_at(16.0, true, false);
        let mut m = DropMotion::new(&d, &ROW);
        let mut top = d.pos[1];
        let mut tumbled = false;
        for _ in 0..600 {
            m.step(1.0 / 60.0, &ROW, Some(&w));
            top = top.max(m.pos[1]);
            tumbled |= m.falling && m.rot.angle_between(Quat::IDENTITY) > 0.1;
            if !m.falling {
                break;
            }
        }
        assert!(top > d.pos[1], "the hop goes up first: top {top}");
        assert!(tumbled, "it tumbles while falling");
        assert!(m.landed && !m.falling, "landed");
        assert!(
            m.pos[1] >= 0.0 && m.pos[1] < 16.0,
            "rests above the floor: {:?}",
            m.pos
        );
        assert_eq!(m.rot, Quat::IDENTITY, "flat on the floor");
        assert_eq!((m.pos[0], m.pos[2]), (10.0, 20.0), "straight down");
    }

    /// Without the fall flag (or without a world) the drop keeps its create position and rotation; with the spin flag
    /// it turns about its up axis at the binary's rate.
    #[test]
    fn no_fall_keeps_the_place_and_spin_turns_it() {
        let w = floor(0.0);
        let d = drop_at(16.0, false, false);
        let mut m = DropMotion::new(&d, &ROW);
        m.advance_to(1000, 3000, &ROW, Some(&w));
        assert_eq!(m.pos, d.pos);
        assert!(m.rot.angle_between(Quat::from_rotation_y(1.0)) < 1e-4);
        let mut s = DropMotion::new(&drop_at(16.0, false, true), &ROW);
        s.advance_to(1000, 1500, &ROW, Some(&w));
        let want = Quat::from_rotation_y(1.0 + SPIN_RATE * 0.5);
        assert!(s.rot.angle_between(want) < 1e-3, "{:?}", s.rot);
        let mut nw = DropMotion::new(&drop_at(16.0, true, false), &ROW);
        nw.advance_to(1000, 3000, &ROW, None);
        assert_eq!(nw.pos, d.pos, "no world: no fall");
    }

    /// The fixed-step motion does not depend on how often it is advanced.
    #[test]
    fn the_motion_is_independent_of_the_call_rate() {
        let w = floor(0.0);
        let d = drop_at(40.0, true, true);
        let mut a = DropMotion::new(&d, &ROW);
        let mut b = DropMotion::new(&d, &ROW);
        for t in (1000..4000).step_by(7) {
            a.advance_to(1000, t, &ROW, Some(&w));
        }
        a.advance_to(1000, 4000, &ROW, Some(&w));
        b.advance_to(1000, 4000, &ROW, Some(&w));
        assert_eq!(a, b);
        let mut world = DropWorld::new(ROW);
        let p = world.advance(std::slice::from_ref(&d), 4000, Some(&w));
        assert_eq!(p[0].pos, b.pos);
        assert!(p[0].landed);
        assert!(world.advance(&[], 5000, Some(&w)).is_empty());
    }

    fn give(slot: u8, weapon: u16, ammo: f32) -> WeaponGrant {
        WeaponGrant {
            weapon,
            ammo_kind: weapon,
            ammo,
            clip: None,
            class_pos: 0,
            slot,
            equip: false,
            parts: [-1; 4],
        }
    }

    /// CLI-151: the spawn set's grants are plain grants; a later 0xb2 while alive is a pickup: a new weapon fills its
    /// slot and is drawn (select pair + 0x7f); the same weapon keeps its clip and takes the told amount as its count.
    #[test]
    fn a_pickup_grant_keeps_the_clip_of_the_same_weapon_and_draws_the_slot() {
        let mut a = Arsenal::new(crate::arms::ArmsConfig::default());
        let mut t = DropTracker::default();
        a.on_life(1000, 1);
        t.on_life(1000, 1);
        t.grant(&mut a, &give(1, 164, 36.0), 1010);
        assert!(t.stats.pickups.is_empty(), "spawn grant");
        let aim = crate::arms::Aim {
            origin: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            control: 0,
        };
        let cmd = crate::arms::WeaponCommands::default();
        a.step(1600, &cmd, &aim, &[], None);
        assert_eq!(a.held().map(|c| c.slot), Some(1));
        // a new weapon into the empty slot 0
        t.grant(&mut a, &give(0, 145, 120.0), 5000);
        let p = t.stats.pickups[0];
        assert_eq!((p.slot, p.weapon, p.same_weapon), (0, 145, false));
        assert_eq!(p.clip + p.stock, 120, "the told amount is the slot's count");
        let out = a.step(5016, &cmd, &aim, &[], None);
        assert_eq!(a.held().map(|c| c.slot), Some(0), "drawn");
        assert!(
            out.iter()
                .any(|m| m.shell == crate::arms::up::WEAPON_CHANGE),
            "the 0x7f of the weapon change"
        );
        // the same weapon again: the clip stays, the count is the told amount
        a.slots.get_mut(&0).unwrap().clip = 7;
        t.grant(&mut a, &give(0, 145, 90.0), 9000);
        let c = a.slots[&0];
        assert_eq!((c.clip, c.stock), (7, 83));
        assert!(t.stats.pickups[1].same_weapon);
        // dead or right after a spawn: no pickup
        a.on_life(9500, 2);
        t.on_life(9500, 2);
        t.grant(&mut a, &give(2, 9, 1.0), 9600);
        assert_eq!(t.stats.pickups.len(), 2);
    }

    /// CLI-1762: a round's survivor stays in state 1 across the next round's spawn set; its status reset (0x66 kind
    /// 0x13) restarts the spawn window, so the grants refill an emptied clip instead of keeping it as a pickup.
    #[test]
    fn a_survivors_next_round_grants_refill_the_clip() {
        let mut a = Arsenal::new(crate::arms::ArmsConfig::default());
        let mut t = DropTracker::default();
        a.on_life(1000, 1);
        t.on_life(1000, 1);
        t.grant(&mut a, &give(0, 145, 30.0), 1010);
        a.slots.get_mut(&0).unwrap().clip = 0;
        // the next round, 60 s later, still alive: the spawn set's status reset, then the grant
        t.on_life(60_000, 1);
        t.on_spawn_set(60_000);
        t.grant(&mut a, &give(0, 145, 30.0), 60_010);
        assert!(t.stats.pickups.is_empty(), "a spawn grant, not a pickup");
        assert_eq!(a.slots[&0].clip, 30, "refilled");
        // a dead player's spawn window is still opened by its state 1, not by the reset
        let mut d = DropTracker::default();
        d.on_spawn_set(5);
        assert!(!d.grant_is_pickup(10_000));
    }

    /// DSV-301 on the client: a pickup grant into the drawn backpack slot (4) replaces that gun there, the slot-0
    /// gun stays, and slot 4 is drawn with the new weapon.
    #[test]
    fn a_pickup_into_the_drawn_backpack_slot_replaces_that_gun() {
        let mut a = Arsenal::new(crate::arms::ArmsConfig::default());
        let mut t = DropTracker::default();
        a.on_life(1000, 1);
        t.on_life(1000, 1);
        t.grant(&mut a, &give(0, 145, 120.0), 1010);
        t.grant(&mut a, &give(4, 25, 90.0), 1010);
        let aim = crate::arms::Aim {
            origin: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            control: 0,
        };
        let cmd = crate::arms::WeaponCommands {
            select: Some(4),
            ..Default::default()
        };
        a.step(1600, &cmd, &aim, &[], None);
        a.step(
            3000,
            &crate::arms::WeaponCommands::default(),
            &aim,
            &[],
            None,
        );
        assert_eq!(a.held().map(|c| c.slot), Some(4), "the backpack gun out");
        t.grant(&mut a, &give(4, 56, 60.0), 5000);
        a.step(
            5016,
            &crate::arms::WeaponCommands::default(),
            &aim,
            &[],
            None,
        );
        assert_eq!(a.slots[&4].weapon, 56, "the backpack slot took the new gun");
        assert_eq!(a.slots[&0].weapon, 145, "the slot-0 gun stays");
        assert_eq!(a.held().map(|c| (c.slot, c.weapon)), Some((4, 56)));
    }

    /// A type 0x17 pickup object of `kind` with the given blob tail, model file 10 / texture 11, at `pos`.
    fn pickup(
        spin: bool,
        fall: bool,
        fx: &str,
        kind: u32,
        tail: &[u32],
        pos: [f32; 3],
    ) -> ClientObject {
        let mut w = BitWriter::new();
        w.write_u8(OBJ_PICKUP)
            .write_bit(spin)
            .write_bit(false)
            .write_bit(fall)
            .write_zstr8(fx.as_bytes());
        w.write_u32(kind);
        for v in tail {
            w.write_u32(*v);
        }
        let mut o = ClientObject::default();
        o.kind = 1;
        o.pos = Some(pos);
        o.flags = OBJ_FLAG_VISIBLE;
        o.blob_type = Some(OBJ_PICKUP);
        o.blob = Some(w);
        o.models = vec![dc_botclient::objects::ModelFileEntry {
            model: 10,
            subs: Vec::new(),
            textures: vec![11],
            styles: Vec::new(),
        }];
        o
    }

    /// CLI-675: the use key pressed over a dropped weapon sends `UP_USE` `[drop object][0]` for the nearest kind-6
    /// drop within reach; a mode item, a drop out of reach, a dead player or no key press sends nothing.
    #[test]
    fn the_use_key_over_a_foreign_drop_asks_for_it() {
        use crate::input::{DeviceInput, InputSource, KeyboardMouse};
        let mut objects = BTreeMap::new();
        // weapon 145 with 30 rounds at the feet, a HiSec case at the same spot, another drop out of reach
        objects.insert(
            40u16,
            pickup(
                false,
                true,
                "",
                PICKUP_KIND_WEAPON,
                &[0x001e_0091],
                [100.0, 0.0, 100.0],
            ),
        );
        objects.insert(
            41u16,
            pickup(
                false,
                false,
                "",
                PICKUP_KIND_CASE,
                &[1],
                [101.0, 0.0, 100.0],
            ),
        );
        objects.insert(
            42u16,
            pickup(
                false,
                true,
                "",
                PICKUP_KIND_WEAPON,
                &[0x001e_0005],
                [300.0, 0.0, 100.0],
            ),
        );
        let mut t = DropTracker::default();
        t.sync_objects(&objects, &files, 1000);
        let eye = [110.0, 90.0, 100.0];
        let mut k = KeyboardMouse::default();
        k.device(DeviceInput::Use);
        let cmd = k.weapon();
        assert!(cmd.use_key);
        assert_eq!(crate::net::use_request(&cmd, &t, eye, 1), Some(40));
        let body = up_use_body(40);
        assert_eq!((body.as_bytes(), body.bit_len()), (&[40u8, 0, 0][..], 24));
        assert_eq!(UP_USE, 0x59);
        assert_eq!(crate::net::use_request(&cmd, &t, eye, 2), None, "dead");
        assert_eq!(
            crate::net::use_request(&cmd, &t, [200.0, 90.0, 100.0], 1),
            None,
            "out of reach"
        );
        let again = k.weapon();
        assert!(!again.use_key, "one request per press");
        assert_eq!(crate::net::use_request(&again, &t, eye, 1), None);
    }

    fn tracker_with_drop(pos: [f32; 3]) -> DropTracker {
        let mut objects = BTreeMap::new();
        objects.insert(
            40u16,
            pickup(false, false, "", PICKUP_KIND_WEAPON, &[0x001e_0005], pos),
        );
        let mut t = DropTracker::default();
        t.sync_objects(&objects, &files, 1000);
        t
    }

    #[test]
    fn aim_target_needs_the_view_on_the_item_within_110() {
        let t = tracker_with_drop([100.0, 0.0, 0.0]);
        let eye = [0.0, 0.0, 0.0];
        let at = t.aim_target(eye, [1.0, 0.0, 0.0], 110.0);
        assert_eq!(at.map(|a| a.0), Some(40), "looking at it within 110");
        assert!(
            t.aim_target(eye, [-1.0, 0.0, 0.0], 110.0).is_none(),
            "looking away"
        );
        assert!(
            t.aim_target(eye, [0.0, 0.0, 1.0], 110.0).is_none(),
            "looking aside"
        );
        let far = tracker_with_drop([200.0, 0.0, 0.0]);
        assert!(
            far.aim_target(eye, [1.0, 0.0, 0.0], 110.0).is_none(),
            "beyond 110"
        );
    }

    fn files(f: u16) -> Option<String> {
        match f {
            10 => Some("models/item.ltb".into()),
            11 => Some("tex/item.dtx".into()),
            _ => None,
        }
    }

    /// w2-sprites (M23): the mode items (flag kind 2, bounty item kind 3, HiSec case kind 5) are drawn like the drops,
    /// with their object's model at the object's place; a later position record moves them; the weapon-drop counters
    /// and the pickup pairing stay kind-6 only.
    #[test]
    fn mode_items_are_drawn_with_their_model_at_the_object_place() {
        let mut objects = BTreeMap::new();
        // a flag `[state][team]` and the at-base bit (only u32 words here), shown (CLI-583)
        objects.insert(
            50u16,
            pickup(
                false,
                false,
                "",
                PICKUP_KIND_FLAG,
                &[1, 1],
                [10.0, 0.0, 20.0],
            ),
        );
        objects.insert(
            51u16,
            pickup(
                false,
                false,
                "",
                PICKUP_KIND_BOUNTY,
                &[0],
                [30.0, 0.0, 40.0],
            ),
        );
        objects.insert(
            52u16,
            pickup(true, false, "", PICKUP_KIND_CASE, &[77], [50.0, 5.0, 60.0]),
        );
        let mut t = DropTracker::default();
        t.sync_objects(&objects, &files, 1000);
        let list = t.objects();
        assert_eq!(
            list.iter().map(|d| (d.obj, d.kind)).collect::<Vec<_>>(),
            vec![(50, 2), (51, 3), (52, 5)],
            "every pickup kind is drawn"
        );
        let m = list[0].model.as_ref().expect("the object's model");
        assert_eq!(
            (m.model.as_str(), m.textures[0].as_str()),
            ("models/item.ltb", "tex/item.dtx")
        );
        assert_eq!((t.stats.created, t.stats.mode_items), (0, 3));
        // drawn where the object is; the case spins in place (its blob's spin flag)
        let mut world = DropWorld::new(ROW);
        let p = world.advance(&list, 1500, None);
        assert_eq!(p[0].pos, [10.0, 0.0, 20.0]);
        assert_eq!(p[2].pos, [50.0, 5.0, 60.0]);
        assert!(
            p[2].rot.angle_between(Quat::IDENTITY) > 0.5,
            "the case turns"
        );
        // the carrier moved the flag: the next sync and placement follow the server
        objects.get_mut(&50).unwrap().pos = Some([12.0, 3.0, 25.0]);
        t.sync_objects(&objects, &files, 1600);
        let p = world.advance(&t.objects(), 1700, None);
        assert_eq!(p[0].pos, [12.0, 3.0, 25.0]);
        // removing a mode item is not a weapon pickup
        objects.remove(&51);
        t.sync_objects(&objects, &files, 1800);
        assert_eq!((t.objects().len(), t.stats.removed), (2, 0));
        assert!(t.stats.live.is_empty(), "no weapon drop live");
    }

    /// world-fixes (CLI-582, the user's "the HiSec case model did not appear but let me pick it up"): a case created
    /// under the id of a weapon drop removed in the same poll (the server's lowest free id) is drawn as the case (its
    /// own kind, model and spinning motion), not as the old drop; a case whose model file the file table names only a
    /// poll later gets its model then; a model that failed under an id does not block another model there.
    #[test]
    fn a_case_under_a_reused_id_or_a_late_file_name_gets_its_model() {
        let files2 = |f: u16| -> Option<String> {
            match f {
                12 => Some("props/case.ltb".into()),
                13 => Some("props/case.dtx".into()),
                f => files(f),
            }
        };
        let mut objects = BTreeMap::new();
        objects.insert(
            40u16,
            pickup(
                false,
                true,
                "",
                PICKUP_KIND_WEAPON,
                &[0x001e_0091],
                [0.0; 3],
            ),
        );
        let mut t = DropTracker::default();
        t.sync_objects(&objects, &files2, 1000);
        let mut world = DropWorld::new(ROW);
        let _ = world.advance(&t.objects(), 3000, None);
        // the drop goes and the case comes with the same id within one poll
        let mut case = pickup(true, false, "", PICKUP_KIND_CASE, &[1], [50.0, 20.0, 60.0]);
        case.models[0].model = 12;
        case.models[0].textures = vec![13];
        objects.insert(40u16, case.clone());
        t.sync_objects(&objects, &files2, 3100);
        let list = t.objects();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].kind, PICKUP_KIND_CASE, "the case, not the old drop");
        assert_eq!(
            list[0].model.as_ref().map(|m| m.model.as_str()),
            Some("props/case.ltb")
        );
        assert_eq!(
            list[0].seen_ms, 3100,
            "a new object from its own first sight"
        );
        assert_eq!(t.stats.removed, 1, "the old drop counts as removed");
        let p = world.advance(&list, 3600, None);
        assert_eq!(p[0].pos, [50.0, 20.0, 60.0]);
        assert!(
            p[0].rot.angle_between(Quat::IDENTITY) > 0.5,
            "its own motion: the case spins"
        );
        // a case whose model file is not named yet: no model, then the model once the table names it
        let mut late = case.clone();
        late.models[0].model = 14;
        objects.clear();
        objects.insert(41u16, late);
        let mut t = DropTracker::default();
        t.sync_objects(&objects, &files2, 1000);
        assert!(t.objects()[0].model.is_none());
        let named = |f: u16| -> Option<String> {
            match f {
                14 => Some("props/case2.ltb".into()),
                f => files2(f),
            }
        };
        t.sync_objects(&objects, &named, 1016);
        assert_eq!(
            t.objects()[0].model.as_ref().map(|m| m.model.as_str()),
            Some("props/case2.ltb"),
            "named one poll later"
        );
        // a failed model under an id does not block the next object's model there
        let mut m = DropModels::new(ROW);
        let a = ModelRef {
            model: "a.ltb".into(),
            textures: Vec::new(),
        };
        let b = ModelRef {
            model: "b.ltb".into(),
            textures: Vec::new(),
        };
        m.fail(41, &a);
        assert!(!m.needs(41, &a));
        assert!(m.needs(41, &b), "another model under the same id is tried");
    }

    /// world-fixes (CLI-583, the user's "CTF flags showing in the two spawns" of a TDM match): a pickup is drawn only
    /// while the engine would draw its object: the capture flags a server creates without flag words (the TDM form)
    /// are tracked but not drawn; flags created visible (a flag room) are drawn; a flag word change hides / shows.
    #[test]
    fn a_pickup_is_drawn_only_with_the_engine_visible_bit() {
        let mut objects = BTreeMap::new();
        let mut tdm_flag = pickup(
            false,
            false,
            "",
            PICKUP_KIND_FLAG,
            &[1, 1],
            [10.0, 0.0, 20.0],
        );
        tdm_flag.flags = 0;
        tdm_flag.flag_words = None;
        objects.insert(50u16, tdm_flag.clone());
        let mut t = DropTracker::default();
        t.sync_objects(&objects, &files, 1000);
        assert!(t.objects().is_empty(), "TDM: no flag drawn");
        // the flag room's form: visible flag words
        let mut ctf_flag = tdm_flag.clone();
        ctf_flag.flags = OBJ_FLAG_VISIBLE;
        ctf_flag.flag_words = Some((OBJ_FLAG_VISIBLE, 0, 0, 0));
        objects.insert(50u16, ctf_flag.clone());
        t.sync_objects(&objects, &files, 1016);
        assert_eq!(t.objects().len(), 1, "CTF: the flag is drawn");
        // CLI-1072: a flag attached to a character is drawn on him, not at its object's place
        let mut carrier = dc_botclient::objects::ClientObject::default();
        carrier.attachments = vec![dc_botclient::objects::Attachment {
            child: 50,
            socket: 3,
            offset: [0.0; 3],
            rot: [0.0, 0.0, 0.0, 1.0],
        }];
        objects.insert(7u16, carrier);
        t.sync_objects(&objects, &files, 1020);
        assert!(t.objects().is_empty(), "carried: not drawn as a pickup");
        objects.remove(&7);
        t.sync_objects(&objects, &files, 1024);
        assert_eq!(t.objects().len(), 1, "dropped again: drawn");
        // second word bit 0x20: not drawn
        objects.get_mut(&50).unwrap().flag_words =
            Some((OBJ_FLAG_VISIBLE, OBJ_FLAG2_NOT_DRAWN, 0, 0));
        t.sync_objects(&objects, &files, 1032);
        assert!(t.objects().is_empty());
        // a weapon drop and a case as our server creates them (flags 0x1001 / 0x0001) are drawn
        let mut drop = pickup(
            false,
            true,
            "",
            PICKUP_KIND_WEAPON,
            &[0x001e_0091],
            [0.0; 3],
        );
        drop.flags = 0x1001;
        objects.clear();
        objects.insert(60u16, drop);
        objects.insert(
            61u16,
            pickup(true, false, "", PICKUP_KIND_CASE, &[1], [5.0, 0.0, 5.0]),
        );
        t.sync_objects(&objects, &files, 2000);
        assert_eq!(t.objects().len(), 2);
    }

    /// w2-sprites (m54): the client FX named in a pickup's create blob starts at the object's place with the create;
    /// a 0xa6 sub 1 names a new one (also when it arrives in the poll of the create); an empty name starts none.
    #[test]
    fn pickup_fx_starts_with_the_create_and_the_object_note() {
        let mut objects = BTreeMap::new();
        objects.insert(
            60u16,
            pickup(
                true,
                false,
                "CaseGlow",
                PICKUP_KIND_CASE,
                &[5],
                [1.0, 2.0, 3.0],
            ),
        );
        let mut t = DropTracker::default();
        t.sync_objects(&objects, &files, 100);
        let fx = t.take_fx();
        assert_eq!(fx.len(), 1);
        assert_eq!(
            (fx[0].name.as_str(), fx[0].pos),
            ("CaseGlow", [1.0, 2.0, 3.0])
        );
        assert!(t.take_fx().is_empty(), "started once");
        // the note: [0x17][obj][1][name]
        let note = |obj: u16, name: &str| {
            let mut w = BitWriter::new();
            w.write_u8(OBJ_PICKUP)
                .write_u16(obj)
                .write_u8(NOTE_PICKUP_FX);
            w.write_zstr8(name.as_bytes());
            w
        };
        let n = note(60, "CaseDrop");
        let (obj, name) = read_pickup_fx_note(n.as_bytes(), n.bit_len()).unwrap();
        assert_eq!((obj, name.as_str()), (60, "CaseDrop"));
        t.on_fx_note(obj, name);
        assert_eq!(t.take_fx()[0].name, "CaseDrop");
        t.on_fx_note(60, String::new());
        assert!(t.take_fx().is_empty(), "an empty name starts nothing");
        // a note in the poll of the create replaces the blob's name
        objects.insert(
            61u16,
            pickup(false, false, "Old", PICKUP_KIND_FLAG, &[1, 2], [0.0; 3]),
        );
        t.on_fx_note(61, "New".into());
        t.sync_objects(&objects, &files, 200);
        let fx = t.take_fx();
        assert_eq!(
            fx.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            vec!["New"]
        );
        assert_eq!(t.stats.fx_started, 3);
        // another sub-id or type is not a pickup FX note
        let mut w = BitWriter::new();
        w.write_u8(OBJ_PICKUP)
            .write_u16(60)
            .write_u8(2)
            .write_u32(9);
        assert!(read_pickup_fx_note(w.as_bytes(), w.bit_len()).is_none());
    }

    /// w2-sprites (m54): a weapon drop with 0 rounds hides its row's `ProjectilePiece` piece; with rounds, or for
    /// another pickup kind, nothing is hidden.
    #[test]
    fn a_drop_without_rounds_hides_its_projectile_piece() {
        let doc = dc_bute::ButeDoc::parse(
            "[Weapon12]\nProjectilePiece = \"Rocket\"\n[Weapon13]\nProjectilePiece = \"\"\n[Ammo12]\nProjectilePiece = X\n",
        );
        let pieces = ProjectilePieces::from_doc(&doc);
        assert_eq!((pieces.len(), pieces.get(12)), (1, Some("Rocket")));
        let at = |kind: u32, weapon: u16, rounds: u16| DropPlacement {
            obj: 1,
            kind,
            weapon,
            rounds,
            model: None,
            pos: [0.0; 3],
            rot: Quat::IDENTITY,
            trackers: Vec::new(),
            landed: false,
        };
        assert_eq!(pieces.hidden_for(&at(6, 12, 0)), Some("Rocket"));
        assert_eq!(pieces.hidden_for(&at(6, 12, 1)), None);
        assert_eq!(pieces.hidden_for(&at(6, 13, 0)), None);
        assert_eq!(pieces.hidden_for(&at(2, 12, 0)), None);
    }

    /// Without the ROM keys every number is 0: the drop stays where it was created (no hop, no fall).
    #[test]
    fn a_missing_motion_row_reads_zero() {
        let doc = dc_bute::ButeDoc::parse("[Other]\nX = 1\n");
        assert_eq!(DropMotionRow::from_doc(&doc), DropMotionRow::default());
        let doc = dc_bute::ButeDoc::parse(
            "[PickupItemFX]\nWeaponThrowBaseVel = 1.5\nWeaponThrowCurVel = 2\nYawRotVel = 0.25f\n",
        );
        let r = DropMotionRow::from_doc(&doc);
        assert_eq!(
            (r.start_speed, r.speed_drop, r.tumble),
            (1.5, 2.0, [0.0, 0.25, 0.0])
        );
    }

    /// The motion row of the user's ROM reads finite numbers (values not asserted: ROM data).
    #[test]
    fn rom_motion_row_reads() {
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let r = DropMotionRow::from_vfs(&dc_rez::RezVfs::new(&rom));
        println!("ROM drop motion row {r:?}");
        assert!(r.start_speed.is_finite() && r.speed_drop.is_finite());
        // with the ROM numbers a drop 16 units above a floor comes to rest on it within 10 s
        let w = floor(0.0);
        let mut m = DropMotion::new(&drop_at(16.0, true, false), &r);
        m.advance_to(1000, 11_000, &r, Some(&w));
        println!(
            "ROM drop from 16 units: landed {} at {:?} after {} steps",
            m.landed, m.pos, m.steps
        );
    }
}
