//! The world camera shot: while the server keeps a world camera object live, the frame is drawn from that object
//! (its position and rotation as the object records place it, its field of view) instead of the own view, the
//! first-person weapon is hidden, the camera is the sound listener when its blob says so, and the body does not move
//! when the blob forbids it. The server moves the camera from point to point with change records (cuts), so the
//! camera follows the object's path exactly; a camera that is switched off returns the own view at once.
//!
//! The 2009 rule (CLI-240, proposed; DISASM, CShell):
//! - The camera objects are the objects created from type-0x0d blobs (`[0x0d][movement:u8][listener:u8][third:u8]
//!   [fov_x:f32][fov_y:f32]`, reader cshell+0x217b4a..0x217be7, init cshell+0x216870).
//! - Every frame the local player's slot +0x104 (cshell+0x1f83d0) takes the first of them whose server object has
//!   bit 0x20 in flag word 2 (the user flags, cshell+0x1f8439..0x1f8451) and draws from it: position and rotation of
//!   the object (cshell+0x1f8547..0x1f89d1; the smoothing branch at cshell+0x1f8539 needs two player flags the shot
//!   does not set and is not modelled), field of view from the blob in degrees (cshell+0x1f8aab..0x1f8b22,
//!   cshell+0x1f50d5), weapon hidden and listener set (cshell+0x1f50b0, cshell+0x1f8a9a). Its +0x80 := the movement
//!   byte (cshell+0x1f8502); the movement law zeroes the body's velocity while it is 0 (cshell+0x2001c0).
//! - With no live camera the own view returns (slot +0x3c, cshell+0x1f2ca0). The shot runs before the own camera
//!   update in every frame (cshell+0x1f4fe0 / cshell+0x1f4fea), also while dead: it takes precedence over the death
//!   camera.
// SPEC: UDP_SESSION_2009_B.md §3.3, §4.2 items 3, 6, 7 (the camera object's create and change records)
// DC-DEFAULT: CLI-240 (proposed) the shot; DSV-112 (proposed) the server side

use crate::look::ViewAngles;
use dc_botclient::objects::ClientObject;
use dc_sim::V3;
use std::collections::BTreeMap;

/// Client object type of a world camera.
// DISASM: cshell+0x222480 case 0xd, cshell+0x2167f0 (type getter 0xd)
pub const OBJ_CAMERA_TYPE: u8 = 0x0d;
/// The live bit in the camera object's user flags.
// DISASM: cshell+0x1f8445..0x1f8451
pub const UFLAG_CAMERA_LIVE: u32 = 0x20;

/// A live world camera as the frame loop needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CamShotView {
    /// The camera object's id.
    pub obj: u16,
    pub pos: [f32; 3],
    /// The object's rotation (x, y, z, w); `None` when no rotation was received.
    pub rot: Option<[f32; 4]>,
    /// The players may move while the camera is live (blob byte 1).
    pub allow_move: bool,
    /// The camera is the sound listener (blob byte 2). The 2009 client reads the same byte as the camera's kind:
    /// 1 = letterbox ([`CamShotView::letterbox`]).
    pub listener: bool,
    /// Horizontal and vertical field of view in degrees.
    pub fov: [f32; 2],
}

impl CamShotView {
    /// The camera draws the letterbox bars: its blob's second byte is 1 (the only values sent are 0 / 1).
    // DISASM: cshell+0x216887 (byte -> camera +0x3c), cshell+0x1f8aa2 / 0x1f8b22 (+0x3c -> cshell+0x1f50b0),
    // cshell+0x1f50b8 (letterbox flag = byte == 1)
    // DC-DEFAULT: CLI-610 (proposed)
    pub fn letterbox(&self) -> bool {
        self.listener
    }
}

/// The blob fields of a camera object: (movement, listener, fov).
pub fn camera_blob(o: &ClientObject) -> Option<(bool, bool, [f32; 2])> {
    if o.blob_type != Some(OBJ_CAMERA_TYPE) {
        return None;
    }
    let b = o.blob.as_ref()?;
    if b.bit_len() < 8 * 4 + 64 {
        return None;
    }
    let mut r = b.reader();
    r.read_u8();
    let allow = r.read_u8() != 0;
    let listener = r.read_u8() != 0;
    r.read_u8();
    let fov = [r.read_f32(), r.read_f32()];
    Some((allow, listener, fov))
}

/// The live camera of the session's objects: the first camera object (lowest id) with the live bit and a position.
pub fn live_camera(objects: &BTreeMap<u16, ClientObject>) -> Option<CamShotView> {
    objects.iter().find_map(|(&obj, o)| {
        if o.uflags & UFLAG_CAMERA_LIVE == 0 {
            return None;
        }
        let (allow_move, listener, fov) = camera_blob(o)?;
        let pos = o.pos.filter(|p| p.iter().all(|v| v.is_finite()))?;
        Some(CamShotView {
            obj,
            pos,
            rot: o.rot.filter(|q| q.iter().all(|v| v.is_finite())),
            allow_move,
            listener,
            fov,
        })
    })
}

/// The view angles of a rotation: its forward axis (+Z turned by the quaternion) as yaw (0 along +Z, positive toward
/// +X) and pitch (positive down), the convention of [`crate::deathcam::look_at`].
pub fn view_of(q: [f32; 4]) -> ViewAngles {
    let [x, y, z, w] = q;
    let n = (x * x + y * y + z * z + w * w).sqrt();
    let (x, y, z, w) = if n > 1e-6 {
        (x / n, y / n, z / n, w / n)
    } else {
        (0.0, 0.0, 0.0, 1.0)
    };
    // the third column of the rotation matrix
    let f = V3::new(
        2.0 * (x * z + w * y),
        2.0 * (y * z - w * x),
        1.0 - 2.0 * (x * x + y * y),
    );
    let flat = (f.x * f.x + f.z * f.z).sqrt();
    ViewAngles {
        yaw: f.x.atan2(f.z),
        pitch: (-f.y).atan2(flat),
    }
}

/// The camera shot of a frame loop: what it drew last and how long.
#[derive(Debug, Clone, Default)]
pub struct CamShot {
    /// The live camera of the last frame.
    pub current: Option<CamShotView>,
    /// Frames drawn from a world camera so far.
    pub frames: u32,
    /// Shots seen (a new shot = the first live frame after a frame without one).
    pub shots: u32,
    /// The view of the last live frame (kept when a camera has no rotation).
    view: ViewAngles,
}

impl CamShot {
    /// The camera of this frame: `None` without a live camera; else the eye, the view angles and the horizontal
    /// field of view to draw with.
    pub fn frame(&mut self, shot: Option<&CamShotView>) -> Option<(V3, ViewAngles, f32)> {
        let Some(s) = shot else {
            self.current = None;
            return None;
        };
        if self.current.is_none() {
            self.shots += 1;
        }
        self.current = Some(*s);
        self.frames += 1;
        if let Some(q) = s.rot {
            self.view = view_of(q);
        }
        // the blob's vertical field (the client-fov rule derives the horizontal one from the target aspect)
        let fov = if s.fov[1].is_finite() && s.fov[1] > 1.0 {
            s.fov[1]
        } else {
            dc_render::FOV_Y_DEFAULT_DEG
        };
        Some((V3::new(s.pos[0], s.pos[1], s.pos[2]), self.view, fov))
    }

    /// The shot is live and holds the body (the blob's movement byte is 0).
    pub fn locks_movement(&self) -> bool {
        self.current.is_some_and(|s| !s.allow_move)
    }
}

/// The view settings of a frame: the shot's vertical field of view replaces the client's while a camera is live
/// (the horizontal one follows the target aspect as the client-fov rule does; the blob's own horizontal value is
/// not used).
// DISASM: cshell+0x1f8aab..0x1f8b22 (fov from the camera blob); DC-DEFAULT: CLI-240 (proposed) the horizontal from
// the aspect (CLI-212)
pub fn view_settings(
    v: crate::frame::ViewSettings,
    fov_y: Option<f32>,
) -> crate::frame::ViewSettings {
    match fov_y {
        Some(f) => crate::frame::ViewSettings { fov_y_deg: f, ..v },
        None => v,
    }
}

/// The note sub of a world camera's field of view (`GS_OBJECT_NOTE` kind 0x0d).
// DISASM: cshell+0x216800 (the camera's note reader: `read(8)` sub, sub 1 the field of view)
pub const CAMERA_NOTE_FOV: u8 = 1;

/// `GS_OBJECT_NOTE` `[0x0d][obj:objref16][1:u8][fov_x:f32][fov_y:f32][secs:f32]`: the world camera `obj` blends
/// its field of view (degrees) to `fov` over `secs` (0: a cut).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FovNote {
    pub obj: u16,
    pub fov: [f32; 2],
    pub secs: f32,
}

/// Reads a 0xa6 body (after the shell id) as a camera field-of-view note; `None` for any other note or a short or
/// non-finite one.
// SPEC: DSV-580 (the server's `gs::camera_fov_note`)
// DISASM: cshell+0x20fd00 (note kind 0x0d: every camera whose engine object is `obj` reads the rest), cshell+0x216800
// (sub 1: `read(32)` x 3, the new horizontal and vertical field of view and the blend time)
pub fn read_fov_note(body: &[u8], bits: usize) -> Option<FovNote> {
    let mut r = dc_wire::udp::BitReader::with_bit_len(body, bits);
    if r.remaining_bits() < 8 + 16 + 8 + 96 || r.read_u8() != OBJ_CAMERA_TYPE {
        return None;
    }
    let obj = r.read_u16();
    if r.read_u8() != CAMERA_NOTE_FOV {
        return None;
    }
    let fov = [r.read_f32(), r.read_f32()];
    let secs = r.read_f32();
    (fov.iter().all(|v| v.is_finite()) && secs.is_finite()).then_some(FovNote { obj, fov, secs })
}

/// One camera's field-of-view blend: from `from` at `at` (ms) to `to` over `ms`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FovBlend {
    from: [f32; 2],
    to: [f32; 2],
    at: u32,
    ms: f32,
}

impl FovBlend {
    fn value(&self, now: u32) -> [f32; 2] {
        let t = now.saturating_sub(self.at) as f32;
        if self.ms <= 0.0 || t >= self.ms {
            return self.to;
        }
        let k = t / self.ms;
        [
            self.from[0] + (self.to[0] - self.from[0]) * k,
            self.from[1] + (self.to[1] - self.from[1]) * k,
        ]
    }
}

/// The field-of-view notes of the world cameras: each camera's field of view replaces its blob's once a note came,
/// blended linearly from the value it had when the note arrived. A camera that is switched off keeps its value for
/// its next shot (the own view returns with the player's field of view: [`view_settings`] without a shot).
// DISASM: cshell+0x216800 (sub 1: the old value kept, the new one, the blend time, elapsed := 0), cshell+0x1f8aab..
// 0x1f8b22 (per frame: past the blend time the new value, else the linear blend)
// DC-DEFAULT: CLI-1862 (proposed) the blend runs on the client clock from the note's arrival (the 2009 elapsed time
// grows only in the frames the camera draws); "from" is the value shown at the arrival
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CameraFovs {
    blends: BTreeMap<u16, FovBlend>,
}

impl CameraFovs {
    /// A note arrived at `now`; `blob_fov` is the camera's create-blob field of view (`None`: no such camera yet,
    /// the note's value stands at once).
    pub fn on_note(&mut self, n: FovNote, blob_fov: Option<[f32; 2]>, now: u32) {
        let from = self
            .blends
            .get(&n.obj)
            .map(|b| b.value(now))
            .or(blob_fov)
            .unwrap_or(n.fov);
        self.blends.insert(
            n.obj,
            FovBlend {
                from,
                to: n.fov,
                at: now,
                ms: n.secs.max(0.0) * 1000.0,
            },
        );
    }

    /// The live camera's field of view at `now`.
    pub fn apply(&self, view: &mut CamShotView, now: u32) {
        if let Some(b) = self.blends.get(&view.obj) {
            view.fov = b.value(now);
        }
    }

    /// Forgets the cameras whose object is gone.
    pub fn retain(&mut self, objects: &BTreeMap<u16, ClientObject>) {
        if !self.blends.is_empty() {
            self.blends
                .retain(|obj, _| objects.get(obj).is_some_and(|o| camera_blob(o).is_some()));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.blends.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_wire::udp::BitWriter;

    // the table's object has a private field: no struct literal outside its crate
    #[allow(clippy::field_reassign_with_default)]
    fn camera(live: bool, pos: [f32; 3], q: [f32; 4]) -> ClientObject {
        let mut b = BitWriter::new();
        b.write_u8(OBJ_CAMERA_TYPE)
            .write_u8(0)
            .write_u8(1)
            .write_u8(0)
            .write_f32(80.0)
            .write_f32(60.0);
        let mut o = ClientObject::default();
        o.pos = Some(pos);
        o.rot = Some(q);
        o.blob_type = Some(OBJ_CAMERA_TYPE);
        o.blob = Some(b);
        o.uflags = if live { UFLAG_CAMERA_LIVE } else { 0 };
        o
    }

    #[test]
    fn view_of_matches_the_look_convention() {
        // identity looks along +Z
        let v = view_of([0.0, 0.0, 0.0, 1.0]);
        assert!(v.yaw.abs() < 1e-6 && v.pitch.abs() < 1e-6);
        // a yaw of 90 degrees about +Y looks along +X
        let h = std::f32::consts::FRAC_PI_4;
        let v = view_of([0.0, h.sin(), 0.0, h.cos()]);
        assert!((v.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        // a positive pitch about +X looks down (positive pitch)
        let p = 0.3f32;
        let v = view_of([(p / 2.0).sin(), 0.0, 0.0, (p / 2.0).cos()]);
        assert!((v.pitch - p).abs() < 1e-5, "{v:?}");
    }

    #[test]
    fn only_a_live_camera_object_is_a_shot() {
        let mut objs = BTreeMap::new();
        objs.insert(9, camera(false, [1.0, 2.0, 3.0], [0.0, 0.0, 0.0, 1.0]));
        assert!(live_camera(&objs).is_none());
        objs.insert(12, camera(true, [4.0, 5.0, 6.0], [0.0, 0.0, 0.0, 1.0]));
        let s = live_camera(&objs).unwrap();
        assert_eq!(
            (s.obj, s.pos, s.allow_move, s.listener),
            (12, [4.0, 5.0, 6.0], false, true)
        );
        assert_eq!(s.fov, [80.0, 60.0]);
        // another object type with the bit is not a camera
        let mut other = camera(true, [0.0; 3], [0.0, 0.0, 0.0, 1.0]);
        other.blob_type = Some(0x37);
        let mut objs = BTreeMap::new();
        objs.insert(3, other);
        assert!(live_camera(&objs).is_none());
        let mut c = CamShot::default();
        assert!(c.frame(None).is_none());
        assert!(!c.locks_movement());
    }

    /// The server's 0xa6 kind-0x0d note reads back; other kinds and subs are not field-of-view notes.
    #[test]
    fn the_servers_fov_note_reads_back() {
        let w = dc_dserver::gs::camera_fov_note(0x1234, [70.0, 52.5], 1.5);
        // [S2C shell id][0xa6][body]
        let b = w.as_bytes();
        assert_eq!(b[1], 0xa6);
        let (body, bits) = (&b[2..], w.bit_len() - 16);
        assert_eq!(
            read_fov_note(body, bits),
            Some(FovNote {
                obj: 0x1234,
                fov: [70.0, 52.5],
                secs: 1.5
            })
        );
        assert_eq!(read_fov_note(body, bits - 8), None, "short");
        let mut w = BitWriter::new();
        w.write_u8(OBJ_CAMERA_TYPE)
            .write_u16(3)
            .write_u8(2)
            .write_f32(1.0)
            .write_f32(1.0)
            .write_f32(0.0);
        assert_eq!(read_fov_note(w.as_bytes(), w.bit_len()), None, "sub 2");
        let mut w = BitWriter::new();
        w.write_u8(0x17)
            .write_u16(3)
            .write_u8(1)
            .write_u32(0)
            .write_u32(0)
            .write_u32(0);
        assert_eq!(read_fov_note(w.as_bytes(), w.bit_len()), None, "kind 0x17");
    }

    /// A note blends the live camera's field of view linearly from its blob's value; 0 s is a cut; a second note
    /// starts from the value shown; a camera that is gone is forgotten; the own view has no shot field of view.
    #[test]
    fn a_fov_note_blends_the_live_camera() {
        let mut objs = BTreeMap::new();
        objs.insert(12, camera(true, [0.0; 3], [0.0, 0.0, 0.0, 1.0]));
        let view = |f: &CameraFovs, now| {
            let mut v = live_camera(&objs).unwrap();
            f.apply(&mut v, now);
            v.fov
        };
        let mut f = CameraFovs::default();
        assert_eq!(view(&f, 0), [80.0, 60.0], "the blob's value");
        let blob = camera_blob(&objs[&12]).map(|b| b.2);
        f.on_note(
            FovNote {
                obj: 12,
                fov: [40.0, 30.0],
                secs: 2.0,
            },
            blob,
            1000,
        );
        assert_eq!(view(&f, 1000), [80.0, 60.0]);
        assert_eq!(view(&f, 2000), [60.0, 45.0], "half way");
        assert_eq!(view(&f, 3000), [40.0, 30.0]);
        assert_eq!(view(&f, 9000), [40.0, 30.0], "held");
        // a second note mid-blend starts where the view is
        f.on_note(
            FovNote {
                obj: 12,
                fov: [80.0, 60.0],
                secs: 2.0,
            },
            blob,
            4000,
        );
        f.on_note(
            FovNote {
                obj: 12,
                fov: [20.0, 10.0],
                secs: 0.0,
            },
            blob,
            5000,
        );
        assert_eq!(view(&f, 5000), [20.0, 10.0], "a cut");
        // the shot's vertical value goes to the frame; without a shot the client's own settings stand
        let mut c = CamShot::default();
        let mut v = live_camera(&objs).unwrap();
        f.apply(&mut v, 5000);
        assert_eq!(c.frame(Some(&v)).map(|s| s.2), Some(10.0));
        let own = crate::frame::ViewSettings::default();
        assert_eq!(view_settings(own, Some(10.0)).fov_y_deg, 10.0);
        assert_eq!(
            view_settings(own, None),
            own,
            "the camera off: the own view"
        );
        // the camera object is gone: forgotten
        f.retain(&BTreeMap::new());
        assert!(f.is_empty());
    }
}
