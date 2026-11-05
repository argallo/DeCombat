//! The world camera shot (DSV-112): the world file's camera object, created on every client as a kind-0 object
//! with the type-0x0d camera blob, and the camera statements of the world's trigger scripts that switch it on, off
//! and move it to a camera point.
//!
//! The 2009 rule, client side (DISASM, CShell): every frame the local player's slot +0x104 (cshell+0x1f83d0) walks
//! the camera objects the client created from type-0x0d blobs (`sfx_mgr`+0x3d4, filled only by the type-0x0d case
//! of the object factory cshell+0x222480 at cshell+0x22269f..0x2226a5) and reads flag word 2 of each camera's
//! server object; the first one with bit 0x20 set is **live**: the client then draws from that object's position
//! and rotation (engine camera +0x12c set at cshell+0x1f870f / cshell+0x1f89d1), sets its field of view from the
//! blob (cshell+0x1f8aab..0x1f8b22, degrees x pi / 180 at cshell+0x1f50d5), hides the first-person weapon and makes
//! the camera the sound listener (cshell+0x1f50b0, cshell+0x1f8a9a), and copies the blob's movement byte into the
//! local player's +0x80 (cshell+0x1f8502), which the movement law reads: 0 zeroes the body's velocity
//! (cshell+0x2001c0, `+0x80 == 0`). When no camera is live any more, slot +0x3c (cshell+0x1f2ca0) returns the
//! own view. So the shot is the server's: the client plays it for as long as the object's live bit is set.
//!
//! The 2009 rule, world side (OBSERVED, ROM world files read at run time; nothing of them is in this file): every
//! PvP world has one camera object and named camera points; the mode controllers' end triggers switch the camera
//! on and move it to the first camera point (the overview shot), the start triggers switch it off, and the bomb
//! worlds' explosion triggers move it through two or three camera points with delays. The ROM mode files name these
//! triggers as the room's `trig` fields (start triggers, then the end trigger). This is the shot the user remembers
//! between the rounds of Search and Destroy (project decision: play it at the end of every match).
//!
//! DeCombat (DSV-112, proposed): the server creates the world's camera objects with the world objects; at the round
//! result (the 0x46 moment) it runs the camera statements of the room's end trigger, at the round start those of
//! the start triggers (a camera that is off stays off). A statement takes effect as a change record (live bit and,
//! for a move, the point's position and rotation) to every client in the world; a late joiner gets the current
//! state in the create.
// SPEC: UDP_SESSION_2009_B.md §3.3 (create record: kind byte, create position, blob), §4.2 items 3, 6, 7 (flag block,
// position, rotation); FMT_DAT85.md §6.1 (name, position, Euler rotation), §6.3 (command texts)
// DISASM: cshell+0x217b4a..0x217be7 (type-0x0d blob reader: three u8, two u32), cshell+0x216870 (the camera
// object's init: +0x38 movement byte, +0x3c listener byte, +0x40 third byte, +0x44 / +0x48 field of view)
// DC-DEFAULT: DSV-112 (proposed)

use crate::modelobj::{self, CF_POS_SNAP, CF_ROT_SNAP};
use crate::objects::{write_record_head, PosBox, CF_FLAGS, CF_NEW, CF_POS};
use crate::world::{PropHash, WorldScripts};
use dc_wire::udp::{BitWriter, Vec72};

/// Client object type of a world camera.
// DISASM: cshell+0x222480 case 0xd (camera object, container `sfx_mgr`+0x3d4), cshell+0x2167f0 (type getter 0xd)
pub const OBJ_CAMERA_TYPE: u8 = 0x0d;
/// The user flag bit that makes a camera live on the 2009 client.
// DISASM: cshell+0x1f8445..0x1f8451 (flag word 2 of the camera's server object, `test 0x20`)
pub const UFLAG_CAMERA_LIVE: u32 = 0x20;
/// The blob's third byte (camera +0x40). No reader of it was found; every ROM camera has the candidate switches off.
// DISASM: cshell+0x217bb3 (read), cshell+0x21688a (stored at +0x40); SPEC: UNPROVEN meaning
pub const TODO_CAMERA_THIRD: u8 = 0;
/// The camera object's message sub that sets its field of view (`GS_OBJECT_NOTE` note kind 0x0d, DSV-580).
// DISASM: cshell+0x216800 (camera slot +0x30: `read(8)`, sub 1 reads three 32-bit values)
pub const CAMERA_NOTE_FOV: u8 = 1;

/// Class and property keys (hash and length; the names are the ROM's, only their hashes are here).
// OBSERVED: class and property names of the ROM world files' camera and camera point objects (hashes only; 21 worlds,
// 22 camera objects, 67 camera points, 2026-10-25 scratch run over dc-world)
const CLASS_CAMERA: PropHash = ([0x03, 0x49, 0x4b, 0x0d, 0x1f, 0x80, 0x35, 0x22], 6);
const CLASS_CAMERA_POINT: PropHash = ([0x7b, 0x87, 0x70, 0xf7, 0x0d, 0x81, 0xe7, 0xf7], 11);
const PROP_ALLOW_MOVE: PropHash = ([0x4d, 0x69, 0xb6, 0xb7, 0x16, 0xa9, 0x80, 0x2b], 19);
const PROP_LISTENER: PropHash = ([0x50, 0xbb, 0xed, 0x85, 0x91, 0x70, 0x43, 0xbe], 10);
const PROP_FOV_X: PropHash = ([0x1c, 0x4b, 0x6d, 0x24, 0xbd, 0xb9, 0x0b, 0xbe], 4);
const PROP_FOV_Y: PropHash = ([0xc9, 0xb3, 0x54, 0xde, 0x99, 0xd7, 0x5d, 0x38], 4);

/// Words of the command texts (hash and length of the lower-case word).
// OBSERVED: command texts of the ROM world files (hashes only): the message and delayed statements and the camera
// arguments
const WORD_MSG: PropHash = ([0xe4, 0x6b, 0x32, 0x01, 0x65, 0xee, 0xc9, 0x1e], 3);
const WORD_DELAY: PropHash = ([0x6e, 0x16, 0xdb, 0x10, 0x22, 0x67, 0xe6, 0xe7], 5);
const WORD_TRIGGER: PropHash = ([0x68, 0x32, 0x59, 0xfe, 0xab, 0xbf, 0x5e, 0xb3], 7);
const WORD_ON: PropHash = ([0xb8, 0xd3, 0x1e, 0x85, 0x27, 0x25, 0xaf, 0xb1], 2);
const WORD_OFF: PropHash = ([0xb4, 0xdc, 0x66, 0xdd, 0xe8, 0x06, 0x26, 0x1b], 3);
const WORD_MOVETO: PropHash = ([0x8a, 0xa1, 0x9c, 0xf4, 0x44, 0xbb, 0x8f, 0x1d], 6);
const WORD_FOV: PropHash = ([0x86, 0x4f, 0xa3, 0x6e, 0xe7, 0xf5, 0x48, 0xaa], 3);

/// Field of view of a camera without the properties (degrees).
// DC-DEFAULT: DSV-112 (proposed) 90 / 75
pub const DEFAULT_FOV: [f32; 2] = [90.0, 75.0];
/// Nesting limit of trigger messages and statement limit of one evaluation.
const DEPTH: u32 = 8;
const STATEMENTS: u32 = 4096;

fn key(w: &[u8]) -> PropHash {
    (dc_world::hash16(w), w.len())
}

fn word(w: &[u8]) -> PropHash {
    key(&w.to_ascii_lowercase())
}

fn prop<'o, 'a>(o: &'o dc_world::Object<'a>, k: PropHash) -> Option<&'o dc_world::FieldValue<'a>> {
    o.props.iter().find(|p| key(p.name) == k).map(|p| &p.value)
}

fn prop_bool(o: &dc_world::Object<'_>, k: PropHash) -> Option<bool> {
    match prop(o, k)? {
        dc_world::FieldValue::Bool(b) => Some(*b),
        _ => None,
    }
}

fn prop_real(o: &dc_world::Object<'_>, k: PropHash) -> Option<f32> {
    match prop(o, k)? {
        dc_world::FieldValue::Real(r) if r.is_finite() => Some(*r),
        _ => None,
    }
}

/// One camera object of the world file.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraInfo {
    pub name: Vec<u8>,
    pub pos: [f32; 3],
    /// Euler angles in radians (FMT_DAT85.md §6.1 property 2).
    pub rot: [f32; 3],
    /// The players may move while the camera is live (the blob's first byte).
    pub allow_move: bool,
    /// The camera is the sound listener while live (the blob's second byte).
    pub listener: bool,
    /// Horizontal and vertical field of view in degrees.
    pub fov: [f32; 2],
    /// A community map's `dc.end_camera`: live from the round result to the next round's start, with no trigger
    /// script (DC-DEFAULT: MAP-101).
    pub end_shot: bool,
}

/// One camera point (a named place a camera moves to).
#[derive(Debug, Clone, PartialEq)]
pub struct CameraPoint {
    pub name: Vec<u8>,
    pub pos: [f32; 3],
    pub rot: [f32; 3],
}

/// The world's cameras and camera points.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorldCameras {
    pub cameras: Vec<CameraInfo>,
    pub points: Vec<CameraPoint>,
}

impl WorldCameras {
    fn camera(&self, name: &[u8]) -> Option<usize> {
        self.cameras
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
    }

    fn point(&self, name: &[u8]) -> Option<usize> {
        self.points
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(name))
    }
}

/// The camera objects and camera points of a parsed world.
pub fn world_cameras(w: &dc_world::World<'_>) -> WorldCameras {
    let mut out = WorldCameras::default();
    for o in &w.objects {
        let class = key(o.class_name);
        let end_shot = dc_world::names::CLASS_END_CAMERA.matches(o.class_name);
        if class != CLASS_CAMERA && class != CLASS_CAMERA_POINT && !end_shot {
            continue;
        }
        let name = o.name().map(<[u8]>::to_vec).unwrap_or_default();
        let pos = o.position().unwrap_or([0.0; 3]);
        let rot = o.rotation().unwrap_or([0.0; 3]);
        if end_shot {
            out.cameras.push(CameraInfo {
                name,
                pos,
                rot,
                allow_move: false,
                listener: true,
                fov: DEFAULT_FOV,
                end_shot: true,
            });
        } else if class == CLASS_CAMERA {
            out.cameras.push(CameraInfo {
                name,
                pos,
                rot,
                allow_move: prop_bool(o, PROP_ALLOW_MOVE).unwrap_or(false),
                listener: prop_bool(o, PROP_LISTENER).unwrap_or(true),
                fov: [
                    prop_real(o, PROP_FOV_X).unwrap_or(DEFAULT_FOV[0]),
                    prop_real(o, PROP_FOV_Y).unwrap_or(DEFAULT_FOV[1]),
                ],
                end_shot: false,
            });
        } else {
            out.points.push(CameraPoint { name, pos, rot });
        }
    }
    out
}

/// The type-0x0d blob: `[0x0d][allow_move:u8][listener:u8][third:u8][fov_x:f32][fov_y:f32]` (degrees).
// DISASM: cshell+0x217b78..0x217bd9 (three `read(8)`, two `read(32)` into the create record), cshell+0x216870
pub fn camera_blob(c: &CameraInfo) -> BitWriter {
    let mut w = BitWriter::new();
    w.write_u8(OBJ_CAMERA_TYPE)
        .write_u8(c.allow_move as u8)
        .write_u8(c.listener as u8)
        .write_u8(TODO_CAMERA_THIRD)
        .write_f32(c.fov[0])
        .write_f32(c.fov[1]);
    w
}

/// The flag block of a camera: no object flags, the live bit in the user word.
fn write_flags(w: &mut BitWriter, live: bool) {
    w.write_u16(0)
        .write_u16(0)
        .write_u32(if live { UFLAG_CAMERA_LIVE } else { 0 })
        .write_u32(0);
}

fn write_place(w: &mut BitWriter, pos: [f32; 3], rot: [f32; 3], pbox: &PosBox) {
    crate::stream::pos48_encode(pos, pbox).write(w);
    Vec72::encode([0.0; 3]).write(w);
    dc_wire::udp::write_quat128(w, crate::worldobj::euler_quat(rot));
}

/// A camera on the clients: its object id, the world's camera, and its current state.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveCamera {
    pub obj: u16,
    pub info: CameraInfo,
    pub live: bool,
    pub pos: [f32; 3],
    pub rot: [f32; 3],
}

impl LiveCamera {
    /// The create record: kind-0 object with the camera blob, its flag block (the current live bit), the compact
    /// position and the rotation.
    // SPEC: UDP_SESSION_2009_B.md §3.3 (`[kind_byte][create_pos][blob_len][blob]` then the change fields)
    pub fn create_record(&self, pbox: &PosBox) -> BitWriter {
        let blob = camera_blob(&self.info);
        let mut w = BitWriter::new();
        write_record_head(&mut w, CF_NEW | CF_FLAGS | CF_POS | CF_ROT_SNAP, self.obj);
        w.write_u8(modelobj::KIND_HAS_BLOB);
        for _ in 0..3 {
            w.write_f32(0.0);
        }
        w.write_u8(blob.bit_len() as u8);
        w.append(&blob);
        write_flags(&mut w, self.live);
        write_place(&mut w, self.pos, self.rot, pbox);
        w
    }

    /// The change record of a state change: flag block, position and rotation, both as cuts (`*_SNAP`).
    // SPEC: UDP_SESSION_2009_B.md §4.2 items 3, 6, 7 (the flag block precedes the position)
    pub fn change_record(&self, pbox: &PosBox) -> BitWriter {
        let mut w = BitWriter::new();
        write_record_head(
            &mut w,
            CF_FLAGS | CF_POS | CF_POS_SNAP | CF_ROT_SNAP,
            self.obj,
        );
        write_flags(&mut w, self.live);
        write_place(&mut w, self.pos, self.rot, pbox);
        w
    }
}

/// What a camera statement does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CamAction {
    On,
    Off,
    /// Move to the camera point of this index.
    MoveTo(usize),
    /// A field-of-view change: an object note to the camera (DSV-580), and the create blob of a later client.
    Fov([f32; 2]),
}

/// One camera statement of a trigger, `at_ms` after the trigger fired.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CamStep {
    pub at_ms: u32,
    pub camera: usize,
    pub action: CamAction,
}

struct Eval<'a> {
    scripts: &'a WorldScripts,
    cams: &'a WorldCameras,
    out: Vec<CamStep>,
    statements: u32,
}

impl Eval<'_> {
    fn fire(&mut self, name: &[u8], at_ms: u32, depth: u32) {
        if depth > DEPTH {
            return;
        }
        let Some(cmds) = self
            .scripts
            .triggers
            .iter()
            .find(|t| t.0.eq_ignore_ascii_case(name))
            .map(|t| t.1.clone())
        else {
            return;
        };
        for c in &cmds {
            self.run(c, at_ms, depth + 1);
        }
    }

    fn run(&mut self, text: &[u8], at_ms: u32, depth: u32) {
        for st in text.split(|&b| b == b';' || b == b'\n' || b == b'\r') {
            self.statements += 1;
            if self.statements > STATEMENTS {
                return;
            }
            let all: Vec<&[u8]> = st
                .split(|b| b.is_ascii_whitespace() || *b == b'(' || *b == b')')
                .filter(|w| !w.is_empty())
                .collect();
            let mut words = &all[..];
            let mut at = at_ms;
            // `delay <seconds> (<statement>)`
            if words.len() >= 2 && word(words[0]) == WORD_DELAY {
                let secs = std::str::from_utf8(words[1])
                    .ok()
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|s| s.is_finite() && *s >= 0.0)
                    .unwrap_or(0.0);
                at = at.saturating_add((secs * 1000.0).round().min(u32::MAX as f32) as u32);
                words = &words[2..];
            }
            if words.len() < 3 || word(words[0]) != WORD_MSG {
                continue;
            }
            let arg = word(words[2]);
            if arg == WORD_TRIGGER {
                self.fire(words[1], at, depth);
                continue;
            }
            let Some(camera) = self.cams.camera(words[1]) else {
                continue;
            };
            let action = if arg == WORD_ON {
                Some(CamAction::On)
            } else if arg == WORD_OFF {
                Some(CamAction::Off)
            } else if arg == WORD_MOVETO {
                words
                    .get(3)
                    .and_then(|p| self.cams.point(p))
                    .map(CamAction::MoveTo)
            } else if arg == WORD_FOV {
                let n = |i: usize| {
                    words
                        .get(i)
                        .and_then(|v| std::str::from_utf8(v).ok())
                        .and_then(|v| v.parse::<f32>().ok())
                        .filter(|v| v.is_finite())
                };
                n(3).zip(n(4)).map(|(x, y)| CamAction::Fov([x, y]))
            } else {
                None
            };
            if let Some(action) = action {
                self.out.push(CamStep {
                    at_ms: at,
                    camera,
                    action,
                });
            }
        }
    }
}

/// The camera statements a trigger runs (immediate and delayed ones, through the triggers it fires), in time
/// order; statements of the same moment keep their order in the text.
pub fn trigger_steps(scripts: &WorldScripts, cams: &WorldCameras, trigger: &[u8]) -> Vec<CamStep> {
    let mut e = Eval {
        scripts,
        cams,
        out: Vec::new(),
        statements: 0,
    };
    e.fire(trigger, 0, 1);
    let mut out = e.out;
    out.sort_by_key(|s| s.at_ms);
    out
}

/// The room's `trig` fields: the start triggers (fields 1 and 2) and the end trigger (field 3).
// SPEC: DSERVER_PIPE.md §3.2 (`trig`); our lobby's writer (dc-bute room line: mission start, objective start, end,
// controller)
pub fn trig_fields(trig: Option<&str>) -> (Vec<String>, Option<String>) {
    let f: Vec<&str> = trig.map(|t| t.split('|').collect()).unwrap_or_default();
    let starts = f
        .iter()
        .take(2)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let end = f
        .get(2)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    (starts, end)
}

/// The world-camera state of a session: the cameras on the clients, the room's triggers and the pending steps.
#[derive(Debug, Clone, Default)]
pub struct CamShot {
    pub cameras: Vec<LiveCamera>,
    starts: Vec<String>,
    end: Option<String>,
    /// Steps waiting for their time: (due, step).
    pending: Vec<(dc_net_engine::Millis, CamStep)>,
    /// The round start and the round end were handled.
    started: bool,
    ended: bool,
    /// The game round the two latches belong to (RND-2); `None` before the first tick.
    round: Option<u32>,
    /// Steps applied so far (logs and tests).
    pub applied: u32,
    /// Field-of-view changes applied and not yet sent: (camera object, degrees) (DSV-580).
    fov_notes: Vec<(u16, [f32; 2])>,
}

impl CamShot {
    /// One live camera per camera object of the world (ids from `next_id`), all off at their file places.
    pub fn plan(
        cams: &WorldCameras,
        trig: Option<&str>,
        mut next_id: impl FnMut() -> u16,
    ) -> CamShot {
        let (starts, end) = trig_fields(trig);
        CamShot {
            cameras: cams
                .cameras
                .iter()
                .map(|c| LiveCamera {
                    obj: next_id(),
                    info: c.clone(),
                    live: false,
                    pos: c.pos,
                    rot: c.rot,
                })
                .collect(),
            starts,
            end,
            ..CamShot::default()
        }
    }

    /// The room's end trigger (field 3 of `trig`).
    pub fn end_trigger(&self) -> Option<&str> {
        self.end.as_deref()
    }

    fn schedule(&mut self, scripts: &WorldScripts, cams: &WorldCameras, trigger: &str, now: u32) {
        for s in trigger_steps(scripts, cams, trigger.as_bytes()) {
            if s.camera < self.cameras.len() {
                self.pending.push((now.wrapping_add(s.at_ms), s));
            }
        }
    }

    /// The game's round number (each tick): a new round of a round mode runs the start triggers again at its start
    /// and the end trigger again at its result.
    // DC-DEFAULT: RND-2 (staged) the between-round camera shot of DSV-112 at every round
    // DC-DEFAULT: DSV-567 (proposed) every change of the number resets the latches, also into and out of round 0:
    // the warm-up's result is numbered round 0 (1 -> 0 -> 1), and round 1 must not inherit its `ended`
    pub fn track_round(&mut self, round_no: u32) {
        if self.round.is_some_and(|r| r != round_no) {
            self.started = false;
            self.ended = false;
            // steps of the last round's scripts still waiting do not run into the new round
            self.pending.clear();
        }
        self.round = Some(round_no);
    }

    /// The round started: the start triggers' camera statements are scheduled (once).
    pub fn round_started(&mut self, scripts: &WorldScripts, cams: &WorldCameras, now: u32) {
        if self.started || self.cameras.is_empty() {
            return;
        }
        self.started = true;
        for t in self.starts.clone() {
            self.schedule(scripts, cams, &t, now);
        }
        // MAP-101: the next round takes the view back from the last round's end shot
        for camera in 0..self.cameras.len() {
            if self.cameras[camera].info.end_shot && self.cameras[camera].live {
                self.pending.push((
                    now,
                    CamStep {
                        at_ms: 0,
                        camera,
                        action: CamAction::Off,
                    },
                ));
            }
        }
    }

    /// The round result went out: the end trigger's camera statements are scheduled (once).
    pub fn round_ended(&mut self, scripts: &WorldScripts, cams: &WorldCameras, now: u32) {
        if self.ended || self.cameras.is_empty() {
            return;
        }
        self.ended = true;
        if let Some(t) = self.end.clone() {
            self.schedule(scripts, cams, &t, now);
        }
        // MAP-101: a community map's end camera needs no script
        for camera in 0..self.cameras.len() {
            if self.cameras[camera].info.end_shot {
                self.pending.push((
                    now,
                    CamStep {
                        at_ms: 0,
                        camera,
                        action: CamAction::On,
                    },
                ));
            }
        }
    }

    /// Applies the due steps; returns the change records of the cameras they changed (one per camera and call, in
    /// the order of the cameras).
    pub fn due(&mut self, now: u32, points: &[CameraPoint], pbox: &PosBox) -> Vec<BitWriter> {
        let mut changed = vec![false; self.cameras.len()];
        let mut keep = Vec::new();
        for (at, s) in std::mem::take(&mut self.pending) {
            if dc_net_engine::time::later(at, now) {
                keep.push((at, s));
                continue;
            }
            let Some(c) = self.cameras.get_mut(s.camera) else {
                continue;
            };
            match s.action {
                CamAction::On => c.live = true,
                CamAction::Off => c.live = false,
                CamAction::MoveTo(p) => {
                    if let Some(p) = points.get(p) {
                        c.pos = p.pos;
                        c.rot = p.rot;
                    }
                }
                // DC-DEFAULT: DSV-580 (proposed) the camera's field of view: an object note (`gs::camera_fov_note`),
                // kept for the create blob of a client that enters later
                CamAction::Fov(f) => {
                    c.info.fov = f;
                    self.fov_notes.push((c.obj, f));
                    self.applied += 1;
                    continue;
                }
            }
            changed[s.camera] = true;
            self.applied += 1;
        }
        self.pending = keep;
        self.cameras
            .iter()
            .zip(changed)
            .filter(|(_, ch)| *ch)
            .map(|(c, _)| c.change_record(pbox))
            .collect()
    }

    /// Steps still waiting.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// The field-of-view changes applied since the last call: (camera object, degrees).
    pub fn take_fov_notes(&mut self) -> Vec<(u16, [f32; 2])> {
        std::mem::take(&mut self.fov_notes)
    }

    /// A camera statement of the world's statement runner (a mission or round world, DSV-580): `on` / `off`, a move to
    /// a camera point, a field of view, applied with the next [`CamShot::due`]. False when `camera` (or the point) is
    /// not in the world.
    // DC-DEFAULT: DSV-580 (proposed)
    pub fn statement(&mut self, cams: &WorldCameras, cue: &crate::script::Cue, now: u32) -> bool {
        use crate::script::Cue;
        let (name, action) = match cue {
            Cue::Switch { target, on } => (
                target,
                Some(if *on { CamAction::On } else { CamAction::Off }),
            ),
            Cue::MoveTo { camera, point } => (camera, cams.point(point).map(CamAction::MoveTo)),
            Cue::Fov { camera, fov } => (camera, Some(CamAction::Fov(*fov))),
            _ => return false,
        };
        let Some(camera) = cams.camera(name).filter(|&i| i < self.cameras.len()) else {
            return false;
        };
        let Some(action) = action else {
            return false;
        };
        self.pending.push((
            now,
            CamStep {
                at_ms: 0,
                camera,
                action,
            },
        ));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cams() -> WorldCameras {
        WorldCameras {
            cameras: vec![CameraInfo {
                name: b"Cam".to_vec(),
                pos: [1.0, 2.0, 3.0],
                rot: [0.0, 0.5, 0.0],
                allow_move: false,
                listener: true,
                fov: [90.0, 75.0],
                end_shot: false,
            }],
            points: vec![
                CameraPoint {
                    name: b"P0".to_vec(),
                    pos: [100.0, 200.0, 300.0],
                    rot: [0.2, 1.0, 0.0],
                },
                CameraPoint {
                    name: b"P1".to_vec(),
                    pos: [-5.0, 0.0, 5.0],
                    rot: [0.0, 0.0, 0.0],
                },
            ],
        }
    }

    fn scripts() -> WorldScripts {
        WorldScripts {
            triggers: vec![
                (
                    b"EndT".to_vec(),
                    vec![
                        b"msg Zone off;".to_vec(),
                        b"msg cam on;".to_vec(),
                        b"msg Cam (moveto p0);".to_vec(),
                    ],
                ),
                (b"StartT".to_vec(), vec![b"msg Cam off;".to_vec()]),
                (
                    b"Boom".to_vec(),
                    vec![b"delay 0.2 (msg Cam (moveto P1));\r\ndelay 0.2 (msg Cam on);\r\nmsg Inner trigger;\r\ndelay 2.5 (msg Cam (fov 55 41.25))".to_vec()],
                ),
                (b"Inner".to_vec(), vec![b"delay 1 (msg Cam off)".to_vec()]),
            ],
            ..WorldScripts::default()
        }
    }

    /// MAP-101: `dc.end_camera` is a camera with no script: on at the round result, off at the next round's start, and
    /// a world object of no other kind is taken for it.
    #[test]
    fn a_community_end_camera_runs_without_a_script() {
        let bytes = dc_world::synth::SynthWorld {
            info: b"m".to_vec(),
            extent_min: [-512.0; 3],
            extent_max: [512.0; 3],
            tree_shape: vec![false],
            objects: vec![
                dc_world::synth::SynthObject::dc(
                    "dc.end_camera",
                    "overview",
                    [1.0, 400.0, -300.0],
                    [0.6, 0.0, 0.0],
                    &[],
                ),
                dc_world::synth::SynthObject::dc("dc.sound", "s", [0.0; 3], [0.0; 3], &[]),
            ],
            ..Default::default()
        }
        .build();
        let w =
            dc_world::World::parse_with(&bytes, dc_world::ParseOptions { render: false }).unwrap();
        let cams = world_cameras(&w);
        assert_eq!(cams.cameras.len(), 1);
        let c = &cams.cameras[0];
        assert!(c.end_shot && !c.allow_move && c.listener);
        assert_eq!(c.pos, [1.0, 400.0, -300.0]);
        assert_eq!(c.fov, DEFAULT_FOV);

        let pbox = PosBox {
            min: [-512.0; 3],
            max: [512.0; 3],
        };
        let scripts = WorldScripts::default();
        let mut id = 100;
        let mut shot = CamShot::plan(&cams, None, || {
            id += 1;
            id
        });
        assert!(!shot.cameras[0].live, "off until the round ends");
        shot.round_started(&scripts, &cams, 0);
        assert!(
            shot.due(0, &cams.points, &pbox).is_empty(),
            "nothing to do at the first start"
        );
        shot.round_ended(&scripts, &cams, 1000);
        assert_eq!(
            shot.due(1000, &cams.points, &pbox).len(),
            1,
            "one change record"
        );
        assert!(shot.cameras[0].live);
        shot.track_round(2);
        shot.track_round(3);
        shot.round_started(&scripts, &cams, 2000);
        assert_eq!(shot.due(2000, &cams.points, &pbox).len(), 1);
        assert!(!shot.cameras[0].live, "the next round takes the view back");
    }

    #[test]
    fn trigger_statements_with_delays_and_nested_triggers() {
        let s = trigger_steps(&scripts(), &cams(), b"endt");
        assert_eq!(
            s,
            vec![
                CamStep {
                    at_ms: 0,
                    camera: 0,
                    action: CamAction::On
                },
                CamStep {
                    at_ms: 0,
                    camera: 0,
                    action: CamAction::MoveTo(0)
                },
            ]
        );
        let s = trigger_steps(&scripts(), &cams(), b"Boom");
        let acts: Vec<(u32, CamAction)> = s.iter().map(|s| (s.at_ms, s.action)).collect();
        assert_eq!(
            acts,
            vec![
                (200, CamAction::MoveTo(1)),
                (200, CamAction::On),
                (1000, CamAction::Off),
                (2500, CamAction::Fov([55.0, 41.25])),
            ]
        );
        assert!(trigger_steps(&scripts(), &cams(), b"missing").is_empty());
    }

    #[test]
    fn blob_and_records() {
        let c = &cams().cameras[0];
        let b = camera_blob(c);
        assert_eq!(b.bit_len(), 8 * 4 + 64);
        let mut r = b.reader();
        assert_eq!(r.read_u8(), OBJ_CAMERA_TYPE);
        assert_eq!((r.read_u8(), r.read_u8(), r.read_u8()), (0, 1, 0));
        assert_eq!((r.read_f32(), r.read_f32()), (90.0, 75.0));
        let pbox = PosBox {
            min: [-1000.0; 3],
            max: [1000.0; 3],
        };
        let mut shot = CamShot::plan(&cams(), Some("StartT|x|EndT|Ctrl"), || 77);
        assert_eq!(shot.end_trigger(), Some("EndT"));
        assert_eq!(shot.cameras[0].obj, 77);
        // the change record: two cf bytes (high bits), id, flag block with the live bit, pos48 + vec72, quat128
        shot.round_ended(&scripts(), &cams(), 1_000);
        let recs = shot.due(1_000, &cams().points, &pbox);
        assert_eq!(recs.len(), 1);
        assert!(shot.cameras[0].live);
        assert_eq!(shot.cameras[0].pos, [100.0, 200.0, 300.0]);
        let mut r = recs[0].reader();
        let lo = r.read_u8();
        let hi = r.read_u8();
        assert_eq!(
            ((lo & 0x7f) as u16) | ((hi as u16) << 8),
            CF_FLAGS | CF_POS | CF_POS_SNAP | CF_ROT_SNAP
        );
        assert_eq!(r.read_u16(), 77);
        assert_eq!((r.read_u16(), r.read_u16(), r.read_u32()), (0, 0, 0x20));
        assert_eq!(recs[0].bit_len(), 16 + 16 + 96 + 48 + 72 + 128);
        // once only; the start trigger switches it off
        shot.round_ended(&scripts(), &cams(), 2_000);
        assert_eq!(shot.pending(), 0);
        shot.round_started(&scripts(), &cams(), 3_000);
        assert_eq!(shot.due(3_000, &cams().points, &pbox).len(), 1);
        assert!(!shot.cameras[0].live);
        // the create of a live camera carries the live bit
        shot.cameras[0].live = true;
        let cr = shot.cameras[0].create_record(&pbox);
        let mut r = cr.reader();
        r.read_u8();
        r.read_u8();
        r.read_u16();
        assert_eq!(r.read_u8(), modelobj::KIND_HAS_BLOB);
        r.skip_bits(96);
        assert_eq!(r.read_u8(), 96);
        r.skip_bits(96);
        assert_eq!((r.read_u16(), r.read_u16(), r.read_u32()), (0, 0, 0x20));
    }

    #[test]
    fn delayed_steps_wait_for_their_time() {
        let pbox = PosBox {
            min: [-1000.0; 3],
            max: [1000.0; 3],
        };
        let mut shot = CamShot::plan(&cams(), Some("||Boom|"), || 5);
        shot.round_ended(&scripts(), &cams(), 10);
        assert!(shot.due(100, &cams().points, &pbox).is_empty());
        assert_eq!(shot.due(210, &cams().points, &pbox).len(), 1);
        assert!(shot.cameras[0].live);
        assert_eq!(shot.cameras[0].pos, [-5.0, 0.0, 5.0]);
        assert_eq!(shot.due(1_010, &cams().points, &pbox).len(), 1);
        assert!(!shot.cameras[0].live);
        // DSV-580: the field-of-view statement is no change record but a note, and the create blob keeps it
        assert!(shot.due(5_000, &cams().points, &pbox).is_empty());
        assert_eq!(shot.take_fov_notes(), vec![(5, [55.0, 41.25])]);
        assert_eq!(shot.cameras[0].info.fov, [55.0, 41.25]);
        assert_eq!(shot.pending(), 0);
    }

    /// DSV-567: the warm-up's result is numbered round 0; round 1 after it (1 -> 0 -> 1) gets its own start and end
    /// shot, as does every later round.
    #[test]
    fn the_round_after_the_warmup_result_has_its_own_end_shot() {
        let pbox = PosBox {
            min: [-1000.0; 3],
            max: [1000.0; 3],
        };
        let (c, s) = (cams(), scripts());
        let mut shot = CamShot::plan(&c, Some("|StartT|EndT|"), || 5);
        // the warm-up runs as round 1, its result as round 0
        for (round, at) in [(1, 0), (0, 1_000), (1, 2_000), (2, 3_000)] {
            shot.track_round(round);
            shot.round_started(&s, &c, at);
            shot.due(at, &c.points, &pbox);
            assert!(!shot.cameras[0].live, "round {round}: off at its start");
            shot.round_ended(&s, &c, at + 500);
            shot.due(at + 500, &c.points, &pbox);
            assert!(shot.cameras[0].live, "round {round}: the end shot");
        }
    }

    /// DSV-580: the statement runner's camera cues (a mission or round world): `on`, a move to a point, a field of
    /// view, applied with the next `due`; names the world does not have are left to the co-op rules.
    #[test]
    fn statement_cues_move_the_world_camera() {
        use crate::script::Cue;
        let pbox = PosBox {
            min: [-1000.0; 3],
            max: [1000.0; 3],
        };
        let cams = cams();
        let mut shot = CamShot::plan(&cams, None, || 9);
        let cue = |c: &str, w: &str| match w {
            "on" => Cue::Switch {
                target: c.as_bytes().to_vec(),
                on: true,
            },
            "off" => Cue::Switch {
                target: c.as_bytes().to_vec(),
                on: false,
            },
            "fov" => Cue::Fov {
                camera: c.as_bytes().to_vec(),
                fov: [100.0, 84.0],
            },
            p => Cue::MoveTo {
                camera: c.as_bytes().to_vec(),
                point: p.as_bytes().to_vec(),
            },
        };
        assert!(shot.statement(&cams, &cue("CAM", "p1"), 100));
        assert!(shot.statement(&cams, &cue("cam", "fov"), 100));
        assert!(shot.statement(&cams, &cue("Cam", "on"), 100));
        assert!(
            !shot.statement(&cams, &cue("Music", "on"), 100),
            "no camera"
        );
        assert!(
            !shot.statement(&cams, &cue("Cam", "nowhere"), 100),
            "no point"
        );
        assert!(!shot.statement(&cams, &Cue::HideCharacters(true), 100));
        assert!(shot.due(50, &cams.points, &pbox).is_empty(), "not yet due");
        let recs = shot.due(100, &cams.points, &pbox);
        assert_eq!(recs.len(), 1, "one change record for the camera");
        let c = &shot.cameras[0];
        assert!(c.live);
        assert_eq!((c.pos, c.info.fov), ([-5.0, 0.0, 5.0], [100.0, 84.0]));
        assert_eq!(shot.take_fov_notes(), vec![(9, [100.0, 84.0])]);
        // a later client gets the camera live, at the point, with the new field of view in its blob
        let blob = camera_blob(&shot.cameras[0].info);
        let mut r = blob.reader();
        r.read_u8();
        r.read_u8();
        r.read_u8();
        r.read_u8();
        assert_eq!((r.read_f32(), r.read_f32()), (100.0, 84.0));
        assert!(shot.statement(&cams, &cue("cam", "off"), 4_700));
        assert_eq!(shot.due(4_700, &cams.points, &pbox).len(), 1);
        assert!(!shot.cameras[0].live);
    }
}
